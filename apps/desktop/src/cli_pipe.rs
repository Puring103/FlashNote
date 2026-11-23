//! Same-user, local-only desktop command transport on Windows.
use crate::cli::{CliError, Reply, Request};
use std::{
    fs::{File, OpenOptions},
    io,
    os::windows::{
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{
            ERROR_NO_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, ERROR_PIPE_LISTENING, HANDLE,
            HLOCAL, LocalFree,
        },
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            },
            GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
        },
        Storage::FileSystem::{
            FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX, ReadFile, SECURITY_IDENTIFICATION,
            WriteFile,
        },
        System::{
            Pipes::{
                ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe,
                GetNamedPipeServerProcessId, PIPE_NOWAIT, PIPE_REJECT_REMOTE_CLIENTS,
                SetNamedPipeHandleState,
            },
            Threading::{
                GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
    },
    core::{PCWSTR, PWSTR},
};

const REQUEST_LIMIT: usize = 8 * 1024 * 1024;
const REPLY_LIMIT: usize = 64 * 1024 * 1024;
const POLL: Duration = Duration::from_millis(10);
const TIMEOUT: Duration = Duration::from_secs(30);
pub type Job = (Request, SyncSender<Reply>);

pub struct Name {
    path: String,
    sid: String,
}

fn user_sid(process: HANDLE) -> io::Result<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token)?;
        let token = OwnedHandle::from_raw_handle(token.0);
        let token = HANDLE(token.as_raw_handle());
        let mut size = 0;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut size);
        let mut buffer = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            size,
            &mut size,
        )?;
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut sid = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut sid)?;
        let result = sid.to_string().map_err(io::Error::other);
        LocalFree(Some(HLOCAL(sid.0.cast())));
        result
    }
}

pub fn name(root: &Path) -> io::Result<Name> {
    let sid = user_sid(unsafe { GetCurrentProcess() })?;
    let root = root.canonicalize().or_else(|_| std::path::absolute(root))?;
    let identity = format!("{sid}|{}", root.to_string_lossy().to_lowercase());
    let id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, identity.as_bytes());
    Ok(Name {
        path: format!(r"\\.\pipe\Suojian.Cli.v1.{id}"),
        sid,
    })
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

pub fn listen(root: &Path) -> io::Result<Receiver<Job>> {
    let name = name(root)?;
    let path = wide(&name.path);
    let acl = wide(&format!("D:P(A;;GA;;;{})", name.sid));
    let mut descriptor = Default::default();
    let pipe = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(acl.as_ptr()),
            1,
            &mut descriptor,
            None,
        )?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        let handle = CreateNamedPipeW(
            PCWSTR(path.as_ptr()),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            64 * 1024,
            64 * 1024,
            0,
            Some(&attributes),
        );
        let result = if handle.is_invalid() {
            Err(io::Error::last_os_error())
        } else {
            Ok(File::from_raw_handle(handle.0))
        };
        LocalFree(Some(HLOCAL(descriptor.0)));
        result?
    };
    let (send, receive) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("suojian-cli-pipe".into())
        .spawn(move || serve(pipe, send))?;
    Ok(receive)
}

fn serve(mut pipe: File, send: SyncSender<Job>) {
    let handle = HANDLE(pipe.as_raw_handle());
    loop {
        match unsafe { ConnectNamedPipe(handle, None) } {
            Ok(()) => {}
            Err(error) if error.code() == ERROR_PIPE_CONNECTED.to_hresult() => {}
            Err(error) if error.code() == ERROR_PIPE_LISTENING.to_hresult() => {
                thread::sleep(POLL);
                continue;
            }
            Err(error) if error.code() == ERROR_NO_DATA.to_hresult() => {
                let _ = unsafe { DisconnectNamedPipe(handle) };
                continue;
            }
            Err(_) => break,
        }
        let result = exchange(&mut pipe, &send);
        let _ = unsafe { DisconnectNamedPipe(handle) };
        if result == Err(mpsc::RecvTimeoutError::Disconnected) {
            break;
        }
    }
}

fn exchange(pipe: &mut File, send: &SyncSender<Job>) -> Result<(), mpsc::RecvTimeoutError> {
    let Ok(request) = read_json(pipe, REQUEST_LIMIT, Instant::now() + TIMEOUT) else {
        return Ok(());
    };
    let (response, receive) = mpsc::sync_channel(1);
    match send.try_send((request, response)) {
        Ok(()) => {}
        Err(mpsc::TrySendError::Disconnected(_)) => {
            return Err(mpsc::RecvTimeoutError::Disconnected);
        }
        Err(mpsc::TrySendError::Full(_)) => return Ok(()),
    }
    let reply = receive.recv_timeout(TIMEOUT)?;
    let deadline = Instant::now() + TIMEOUT;
    if write_json(pipe, &reply, REPLY_LIMIT, deadline).is_ok() {
        // Keep the instance connected until the client has consumed the response.
        let mut ack = [0];
        let _ = read_bytes(pipe, &mut ack, deadline);
    }
    Ok(())
}

pub fn call(name: &Name, request: &Request) -> io::Result<Option<Reply>> {
    let open_deadline = Instant::now() + Duration::from_secs(3);
    let mut pipe = loop {
        match OpenOptions::new()
            .read(true)
            .write(true)
            .security_qos_flags(SECURITY_IDENTIFICATION.0)
            .open(&name.path)
        {
            Ok(pipe) => break pipe,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32) => {
                if Instant::now() >= open_deadline {
                    return Err(io::ErrorKind::WouldBlock.into());
                }
                thread::sleep(POLL);
            }
            Err(error) => return Err(error),
        }
    };
    let handle = HANDLE(pipe.as_raw_handle());
    unsafe {
        let mut pid = 0;
        GetNamedPipeServerProcessId(handle, &mut pid)?;
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)?;
        let process = OwnedHandle::from_raw_handle(process.0);
        if user_sid(HANDLE(process.as_raw_handle()))? != name.sid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "命令行接口属于其他 Windows 用户",
            ));
        }
        SetNamedPipeHandleState(handle, Some(&PIPE_NOWAIT), None, None)?;
    }
    let deadline = Instant::now() + TIMEOUT;
    write_json(&mut pipe, request, REQUEST_LIMIT, deadline)?;
    let reply = read_json(&mut pipe, REPLY_LIMIT, deadline)?;
    let _ = write_bytes(&mut pipe, &[1], deadline);
    Ok(Some(reply))
}

fn read_json<T: serde::de::DeserializeOwned>(
    pipe: &mut File,
    limit: usize,
    deadline: Instant,
) -> io::Result<T> {
    let mut length = [0; 4];
    read_bytes(pipe, &mut length, deadline)?;
    let length = u32::from_le_bytes(length) as usize;
    if length > limit {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "命令响应过大"));
    }
    let mut bytes = vec![0; length];
    read_bytes(pipe, &mut bytes, deadline)?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn write_json(
    pipe: &mut File,
    value: &impl serde::Serialize,
    limit: usize,
    deadline: Instant,
) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    if bytes.len() > limit {
        if limit != REPLY_LIMIT {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "命令内容过大"));
        }
        bytes = serde_json::to_vec(&Reply::Err(CliError::io(
            "结果过大，请缩小 --limit 或读取单条便签",
        )))?;
    }
    write_bytes(pipe, &(bytes.len() as u32).to_le_bytes(), deadline)?;
    write_bytes(pipe, &bytes, deadline)
}

fn read_bytes(pipe: &mut File, mut bytes: &mut [u8], deadline: Instant) -> io::Result<()> {
    let handle = HANDLE(pipe.as_raw_handle());
    while !bytes.is_empty() {
        let mut count = 0;
        match unsafe { ReadFile(handle, Some(bytes), Some(&mut count), None) } {
            Ok(()) if count == 0 => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(()) => bytes = &mut bytes[count as usize..],
            Err(error) if error.code() == ERROR_NO_DATA.to_hresult() => pause(deadline)?,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn write_bytes(pipe: &mut File, mut bytes: &[u8], deadline: Instant) -> io::Result<()> {
    let handle = HANDLE(pipe.as_raw_handle());
    while !bytes.is_empty() {
        let chunk = &bytes[..bytes.len().min(64 * 1024)];
        let mut count = 0;
        match unsafe { WriteFile(handle, Some(chunk), Some(&mut count), None) } {
            Ok(()) if count == 0 => pause(deadline)?,
            Ok(()) => bytes = &bytes[count as usize..],
            Err(error) if error.code() == ERROR_NO_DATA.to_hresult() => pause(deadline)?,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn pause(deadline: Instant) -> io::Result<()> {
    if Instant::now() >= deadline {
        return Err(io::ErrorKind::TimedOut.into());
    }
    thread::sleep(POLL);
    Ok(())
}

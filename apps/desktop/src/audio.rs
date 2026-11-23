use crate::model::{Settings, id};
use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::{
    io::Cursor,
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
};

pub struct Recorder {
    stop: mpsc::Sender<()>,
    pub result: mpsc::Receiver<Result<(Vec<u8>, u64)>>,
}
impl Recorder {
    pub fn start() -> Self {
        let (stop, stop_rx) = mpsc::channel();
        let (tx, result) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(record(stop_rx));
        });
        Self { stop, result }
    }
    pub fn stop(&self) {
        let _ = self.stop.send(());
    }
}
impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}
fn record(stop: mpsc::Receiver<()>) -> Result<(Vec<u8>, u64)> {
    let device = cpal::default_host()
        .default_input_device()
        .context("没有找到麦克风")?;
    let config = device.default_input_config()?;
    let channels = config.channels() as usize;
    let rate = config.sample_rate().0;
    let samples = Arc::new(Mutex::new(Vec::<f32>::new()));
    let capture_error = Arc::new(Mutex::new(None::<String>));
    let error = capture_error.clone();
    let err = move |e: cpal::StreamError| {
        *error.lock().unwrap_or_else(|e| e.into_inner()) = Some(e.to_string());
    };
    let target = samples.clone();
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config.into(),
            move |data: &[f32], _| {
                target
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .extend_from_slice(data);
            },
            err,
            None,
        )?,
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config.into(),
            move |data: &[i16], _| {
                target
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .extend(data.iter().map(|x| *x as f32 / 32768.));
            },
            err,
            None,
        )?,
        cpal::SampleFormat::U16 => device.build_input_stream(
            &config.into(),
            move |data: &[u16], _| {
                target
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .extend(data.iter().map(|x| (*x as f32 - 32768.) / 32768.));
            },
            err,
            None,
        )?,
        _ => bail!("暂不支持此麦克风采样格式"),
    };
    stream.play()?;
    let _ = stop.recv_timeout(std::time::Duration::from_secs(1800));
    drop(stream);
    if let Some(error) = capture_error
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    {
        bail!(error);
    }
    let source = samples.lock().unwrap_or_else(|e| e.into_inner());
    if source.is_empty() {
        bail!("录音为空，请检查麦克风权限");
    }
    let frames = source.len() / channels;
    let duration = frames as u64 * 1000 / rate as u64;
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut wav = hound::WavWriter::new(
            &mut cursor,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )?;
        for i in 0..(frames as u64 * 16000 / rate as u64) as usize {
            let position = i as f64 * rate as f64 / 16000.;
            let a = position as usize;
            let b = (a + 1).min(frames - 1);
            let mono = |frame: usize| {
                source[frame * channels..(frame + 1) * channels]
                    .iter()
                    .sum::<f32>()
                    / channels as f32
            };
            let sample =
                mono(a) * (1. - position.fract() as f32) + mono(b) * position.fract() as f32;
            wav.write_sample((sample.clamp(-1., 1.) * 32767.) as i16)?;
        }
        wav.finalize()?;
    }
    Ok((cursor.into_inner(), duration))
}

pub fn play(bytes: Vec<u8>, stop: mpsc::Receiver<()>) -> Result<()> {
    let (_stream, handle) = rodio::OutputStream::try_default()?;
    let sink = rodio::Sink::try_new(&handle)?;
    sink.append(rodio::Decoder::new(Cursor::new(bytes))?);
    while !sink.empty() {
        if stop
            .recv_timeout(std::time::Duration::from_millis(30))
            .is_ok()
        {
            sink.stop();
            break;
        }
    }
    Ok(())
}
pub fn waveform(bytes: &[u8]) -> Vec<f32> {
    let Ok(mut reader) = hound::WavReader::new(Cursor::new(bytes)) else {
        return vec![0.; 39];
    };
    let bin = ((reader.len() as usize) / 39).max(1);
    let mut peaks = vec![0f32; 39];
    for (i, sample) in reader.samples::<i16>().enumerate() {
        let index = (i / bin).min(38);
        if let Ok(sample) = sample {
            peaks[index] = peaks[index].max((sample as f32 / 32768.).abs());
        }
    }
    peaks.into_iter().map(f32::sqrt).collect()
}
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0.join("audio.wav"));
        let _ = std::fs::remove_file(self.0.join("result.txt"));
        let _ = std::fs::remove_dir(&self.0);
    }
}
pub fn cleanup_scratch(root: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(uuid) = name.strip_prefix("speech-") {
            if uuid::Uuid::parse_str(uuid).is_ok() && entry.file_type().is_ok_and(|t| t.is_dir()) {
                let _ = std::fs::remove_file(entry.path().join("audio.wav"));
                let _ = std::fs::remove_file(entry.path().join("result.txt"));
                let _ = std::fs::remove_dir(entry.path());
            }
        }
    }
}
/// Repair moved / stale paths using the speech files shipped beside the app.
pub fn resolve_speech(settings: &Settings) -> Settings {
    let mut resolved = settings.clone();
    let mut roots = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        for root in exe.ancestors().skip(1).take(4) {
            roots.push(root.to_path_buf());
        }
    }
    roots.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    if !std::path::Path::new(&resolved.whisper_cli).is_file() {
        let executable = format!("whisper-cli{}", std::env::consts::EXE_SUFFIX);
        for root in &roots {
            for relative in [
                format!("runtime/whisper/Release/{executable}"),
                format!("runtime/whisper/{executable}"),
                format!("whisper/{executable}"),
                format!("{executable}"),
            ] {
                let path = root.join(relative);
                if path.is_file() {
                    resolved.whisper_cli = path.to_string_lossy().into_owned();
                    break;
                }
            }
            if std::path::Path::new(&resolved.whisper_cli).is_file() {
                break;
            }
        }
    }
    let filename = format!("ggml-{}.bin", resolved.speech_model);
    let current = std::path::Path::new(&resolved.model_path);
    if current
        .file_name()
        .is_none_or(|name| name != filename.as_str())
    {
        resolved.model_path = current
            .parent()
            .unwrap_or(std::path::Path::new("models"))
            .join(&filename)
            .to_string_lossy()
            .into_owned();
    }
    if !std::path::Path::new(&resolved.model_path).is_file() {
        'search: for root in &roots {
            for directory in ["runtime/models", "models"] {
                let path = root.join(directory).join(&filename);
                if path.is_file() {
                    resolved.model_path = path.to_string_lossy().into_owned();
                    break 'search;
                }
            }
        }
    }
    resolved
}

pub fn model_path(settings: &Settings, root: &std::path::Path, model: &str) -> PathBuf {
    let local = root.join("models").join(format!("ggml-{model}.bin"));
    if local.is_file() {
        return local;
    }
    let mut selected = settings.clone();
    selected.speech_model = model.into();
    let resolved = resolve_speech(&selected);
    let path = PathBuf::from(resolved.model_path);
    if path.is_file() { path } else { local }
}

pub fn download_model(
    model: &str,
    destination: &std::path::Path,
    mut progress: impl FnMut(u8),
) -> Result<()> {
    use std::io::{Read, Write};
    anyhow::ensure!(
        matches!(model, "tiny" | "base" | "small"),
        "不支持的语音模型"
    );
    std::fs::create_dir_all(destination.parent().context("模型目录无效")?)?;
    let temporary = destination.with_extension(format!("{}.part", id()));
    let result = (|| -> Result<()> {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(1800))
            .build()?;
        let mut response = client
            .get(format!(
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{model}.bin"
            ))
            .send()?
            .error_for_status()?;
        let total = response.content_length();
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let mut header = [0u8; 4];
        response.read_exact(&mut header)?;
        anyhow::ensure!(
            u32::from_le_bytes(header) == 0x67676d6c,
            "下载内容不是有效的语音模型"
        );
        file.write_all(&header)?;
        let mut received = 4u64;
        let mut last = 0;
        let mut buffer = [0u8; 65536];
        loop {
            let count = response.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            received += count as u64;
            anyhow::ensure!(received <= 600 * 1024 * 1024, "模型文件大小异常");
            file.write_all(&buffer[..count])?;
            let percent = total
                .filter(|n| *n > 0)
                .map(|n| (received.saturating_mul(100) / n).min(99) as u8)
                .unwrap_or(0);
            if percent != last {
                progress(percent);
                last = percent;
            }
        }
        anyhow::ensure!(
            received > 1024 * 1024 && total.is_none_or(|n| received == n),
            "模型下载不完整，请重试"
        );
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, destination)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}
pub fn transcribe(bytes: Vec<u8>, settings: &Settings, root: PathBuf) -> Result<String> {
    let settings = resolve_speech(settings);
    if !std::path::Path::new(&settings.whisper_cli).is_file() {
        bail!("随程序提供的转写引擎缺失，请恢复 runtime/whisper 文件夹");
    }
    if !std::path::Path::new(&settings.model_path).is_file() {
        bail!("所选语音模型尚未下载，请在设置中下载后重试");
    }
    let scratch = Scratch(root.join(format!("speech-{}", id())));
    std::fs::create_dir_all(&scratch.0)?;
    std::fs::write(scratch.0.join("audio.wav"), bytes)?;
    let mut child = crate::platform::quiet_command(&settings.whisper_cli)
        .args(["-m", &settings.model_path, "-l", "zh", "-f"])
        .arg(scratch.0.join("audio.wav"))
        .arg("-otxt")
        .arg("-of")
        .arg(scratch.0.join("result"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
        }
        if start.elapsed() > std::time::Duration::from_secs(180) {
            let _ = child.kill();
            let _ = child.wait();
            bail!("转写超时，原始录音已保留，可切换较小模型后重试");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    if !status.success() {
        bail!(
            "语音识别失败，请检查模型与运行库（退出码 {:?}）",
            status.code()
        );
    }
    let raw = std::fs::read_to_string(scratch.0.join("result.txt"))?;
    let lines = raw
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s.chars()
                .last()
                .is_some_and(|c| "。？！.!?，,；;：:".contains(c))
            {
                s.to_string()
            } else {
                format!("{s}。")
            }
        })
        .collect::<Vec<_>>();
    if lines.is_empty() {
        bail!("未识别到语音，原始录音已保留");
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn waveform_handles_empty_corrupt_and_valid_audio() {
        assert_eq!(waveform(b"broken"), vec![0.; 39]);
        let mut data = Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(
                &mut data,
                hound::WavSpec {
                    channels: 1,
                    sample_rate: 16000,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                },
            )
            .unwrap();
            for _ in 0..160 {
                writer.write_sample(16384i16).unwrap();
            }
            writer.finalize().unwrap();
        }
        let peaks = waveform(&data.into_inner());
        assert_eq!(peaks.len(), 39);
        assert!(peaks.iter().all(|x| x.is_finite() && *x >= 0. && *x <= 1.));
    }
}

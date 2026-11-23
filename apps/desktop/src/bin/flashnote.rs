use chrono::{Local, NaiveDate, TimeZone};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::{Value, json};
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};
use suojian::{
    cli::{self, CliError, ErrorKind, Filter, Reply, Request, SearchScope},
    store::{Store, StoreBusy},
};

#[derive(Parser)]
#[command(
    name = "flashnote",
    version,
    about = "从命令行搜索、读取和写入闪记便签"
)]
struct Cli {
    /// 输出 JSON，错误写入 stderr
    #[arg(long, global = true)]
    json: bool,
    /// 指定资料库目录，也可使用 SUOJIAN_DATA_DIR
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(flatten)]
    Note(Notes),
    #[command(hide = true)]
    Notes {
        #[command(subcommand)]
        command: Notes,
    },
    /// 查询和管理分组
    Groups {
        #[command(subcommand)]
        command: Groups,
    },
    /// 获取或导出本地附件
    Attachments {
        #[command(subcommand)]
        command: Attachments,
    },
}

#[derive(Subcommand)]
enum Notes {
    /// 模糊搜索标题、正文和录音转写
    Search {
        query: String,
        /// 匹配连续短语
        #[arg(long)]
        exact: bool,
        /// 同时返回完整 Markdown 和本地附件路径
        #[arg(long)]
        full: bool,
        /// 仅搜索指定字段，body 只匹配 Markdown 正文
        #[arg(long = "in", value_enum, default_value = "all")]
        scope: SearchScope,
        #[command(flatten)]
        filter: Filters,
    },
    /// 按最近更新时间列出便签
    List {
        #[command(flatten)]
        filter: Filters,
    },
    /// 读取一条或多条便签的 Markdown 和本地附件路径
    Get {
        #[arg(required = true, value_name = "ID", num_args = 1..=usize::from(cli::MAX_LIMIT))]
        ids: Vec<String>,
    },
    /// 创建 Markdown 便签
    #[command(group(clap::ArgGroup::new("content").args(["file", "stdin", "attachments", "template"]).required(true).multiple(true)))]
    Create {
        #[arg(long, default_value = "")]
        title: String,
        #[arg(long)]
        group: Option<String>,
        /// 预填 Markdown 模板
        #[arg(long, value_enum)]
        template: Option<Template>,
        /// 添加附件，可重复指定
        #[arg(long = "attach", value_name = "FILE")]
        attachments: Vec<PathBuf>,
        #[command(flatten)]
        body: Body,
    },
    /// 追加 Markdown 到便签末尾
    #[command(group(clap::ArgGroup::new("content").args(["file", "stdin", "attachments"]).required(true).multiple(true)))]
    Append {
        id: String,
        /// 仅当修订号一致时写入
        #[arg(long)]
        if_revision: Option<String>,
        /// 添加附件，可重复指定
        #[arg(long = "attach", value_name = "FILE")]
        attachments: Vec<PathBuf>,
        #[command(flatten)]
        body: Body,
    },
    /// 替换正文、修改标题或移动分组
    #[command(group(clap::ArgGroup::new("changes").args(["title", "group", "file", "stdin"]).required(true).multiple(true)))]
    Update {
        id: String,
        /// 从 get --json 读取的修订号
        #[arg(long)]
        if_revision: String,
        /// 设置新标题
        #[arg(long)]
        title: Option<String>,
        /// 移动到指定分组
        #[arg(long)]
        group: Option<String>,
        /// 从 UTF-8 Markdown 文件替换正文
        #[arg(long, conflicts_with = "stdin")]
        file: Option<PathBuf>,
        /// 从标准输入读取 UTF-8 Markdown 并替换正文
        #[arg(long)]
        stdin: bool,
    },
    /// 将便签和本地附件导出到新目录
    Export {
        #[arg(required = true, value_name = "ID", num_args = 1..=usize::from(cli::MAX_LIMIT))]
        ids: Vec<String>,
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
    },
    /// 在 Windows 桌面端打开指定便签
    #[cfg(windows)]
    Open { id: String },
}

#[derive(Subcommand)]
enum Groups {
    /// 列出分组名称和 ID
    List,
    /// 创建分组
    Create {
        name: String,
        #[arg(long, value_parser = parse_color, default_value = "#245356")]
        color: u32,
    },
    /// 修改现有分组的名称或颜色
    #[command(group(clap::ArgGroup::new("changes").args(["name", "color"]).required(true).multiple(true)))]
    Update {
        group: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, value_parser = parse_color)]
        color: Option<u32>,
    },
    /// 迁移组内便签（含回收站）并删除分组
    Delete {
        group: String,
        #[arg(long)]
        move_to: String,
    },
}

fn parse_color(value: &str) -> Result<u32, &'static str> {
    let digits = value.strip_prefix('#').unwrap_or(value);
    if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("请输入六位 RGB 十六进制颜色，例如 #245356");
    }
    u32::from_str_radix(digits, 16).map_err(|_| "无效 RGB 颜色")
}

#[derive(Clone, Copy, ValueEnum)]
enum Template {
    Meeting,
    Todo,
    Idea,
}

impl Template {
    fn markdown(self) -> &'static str {
        match self {
            Self::Meeting => "## 议题\n\n## 结论\n\n## 待办\n\n- [ ] 补充行动项",
            Self::Todo => "## 待办\n\n- [ ] 待办事项",
            Self::Idea => "## 灵感\n\n## 下一步",
        }
    }
}

#[derive(Subcommand)]
enum Attachments {
    /// 输出本地附件的绝对路径
    Path { id: String },
    /// 复制附件到新文件
    Export {
        id: String,
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Args)]
struct Filters {
    /// 分组 ID 或完整名称
    #[arg(long)]
    group: Option<String>,
    /// 按更新时间筛选，从本地日期零点起
    #[arg(long, value_name = "YYYY-MM-DD")]
    since: Option<NaiveDate>,
    /// 按更新时间筛选，包含指定日期当天
    #[arg(long, value_name = "YYYY-MM-DD")]
    until: Option<NaiveDate>,
    /// 最多返回多少条（1–100）
    #[arg(long, default_value_t = cli::DEFAULT_LIMIT, value_parser = clap::value_parser!(u16).range(1..=i64::from(cli::MAX_LIMIT)))]
    limit: u16,
    /// 上一页 JSON 返回的 next_cursor
    #[arg(long)]
    cursor: Option<String>,
}

impl Filters {
    fn into_filter(self) -> Result<Filter, CliError> {
        if self
            .since
            .zip(self.until)
            .is_some_and(|(since, until)| since > until)
        {
            return Err(CliError::new(ErrorKind::Input, "--since 不能晚于 --until"));
        }
        let start_of_day = |date: NaiveDate| {
            let midnight = date.and_hms_opt(0, 0, 0).unwrap();
            // Some zones shift their clocks at midnight. Use the first valid minute of the day.
            (0..24 * 60)
                .find_map(|minute| {
                    Local
                        .from_local_datetime(&(midnight + chrono::Duration::minutes(minute)))
                        .earliest()
                        .map(|date| date.timestamp_millis())
                })
                .ok_or_else(|| CliError::new(ErrorKind::Input, "指定日期在本地时区中不存在"))
        };
        let since = self.since.map(start_of_day).transpose()?;
        let before = self
            .until
            .map(|date| {
                let next = date
                    .succ_opt()
                    .ok_or_else(|| CliError::new(ErrorKind::Input, "结束日期超出范围"))?;
                start_of_day(next)
            })
            .transpose()?;
        Ok(Filter {
            group: self.group,
            since,
            before,
            limit: self.limit,
            cursor: self.cursor,
        })
    }
}

#[derive(Args)]
#[group(multiple = false)]
struct Body {
    /// 从标准输入读取 UTF-8 Markdown
    #[arg(long)]
    stdin: bool,
    /// 从 UTF-8 Markdown 文件读取
    #[arg(long)]
    file: Option<PathBuf>,
}

impl Body {
    fn read_optional(self) -> Result<String, CliError> {
        if self.stdin || self.file.is_some() {
            self.read()
        } else {
            Ok(String::new())
        }
    }

    fn read(self) -> Result<String, CliError> {
        let reader: Box<dyn Read> = match self.file {
            Some(path) => Box::new(std::fs::File::open(path).map_err(CliError::io)?),
            None => Box::new(io::stdin().lock()),
        };
        let mut bytes = Vec::new();
        reader
            .take((cli::MAX_INPUT + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(CliError::io)?;
        if bytes.len() > cli::MAX_INPUT {
            return Err(CliError::new(ErrorKind::Input, "Markdown 最多 1 MiB"));
        }
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| CliError::new(ErrorKind::Input, "Markdown 必须使用 UTF-8 编码"))
    }
}

fn dispatch(root: &Path, request: Request) -> Reply {
    #[cfg(windows)]
    let pipe = suojian::cli_pipe::name(root).map_err(CliError::io)?;
    #[cfg(windows)]
    if let Some(reply) = suojian::cli_pipe::call(&pipe, &request).map_err(pipe_error)? {
        return reply;
    }

    #[cfg(target_os = "linux")]
    suojian::linux_keyring::install().map_err(CliError::io)?;
    let mut store = match Store::open(root.to_owned()) {
        Ok(store) => store,
        Err(error) if error.is::<StoreBusy>() => {
            #[cfg(windows)]
            if let Some(reply) = suojian::cli_pipe::call(&pipe, &request).map_err(pipe_error)? {
                return reply;
            }
            return Err(CliError::new(ErrorKind::Busy, error.to_string()));
        }
        Err(error) => return Err(CliError::io(error)),
    };
    cli::execute(&mut store, request)
}

#[cfg(windows)]
fn pipe_error(error: io::Error) -> CliError {
    if error.kind() == io::ErrorKind::WouldBlock {
        CliError::new(ErrorKind::Busy, "闪记正在处理另一条命令，请稍后重试")
    } else {
        CliError::io(error)
    }
}

fn absolute_paths(paths: Vec<PathBuf>) -> Result<Vec<PathBuf>, CliError> {
    paths
        .into_iter()
        .map(|path| std::path::absolute(path).map_err(CliError::io))
        .collect()
}

#[cfg(windows)]
fn open_note(root: &Path, id: String) -> Reply {
    use std::{
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };
    use suojian::cli_pipe;

    let root = std::path::absolute(root).map_err(CliError::io)?;
    let pipe = cli_pipe::name(&root).map_err(CliError::io)?;
    let request = Request::Open { id };
    if let Some(reply) = cli_pipe::call(&pipe, &request).map_err(pipe_error)? {
        return reply;
    }
    let exe = std::env::current_exe().map_err(CliError::io)?;
    let directory = exe
        .parent()
        .ok_or_else(|| CliError::io("无法确定闪记安装目录"))?;
    let desktop = ["闪记.exe", "索笺.exe", "suojian.exe"]
        .into_iter()
        .map(|name| directory.join(name))
        .find(|path| path.is_file())
        .ok_or_else(|| CliError::new(ErrorKind::NotFound, "CLI 同目录中没有闪记桌面程序"))?;
    let mut child = Command::new(desktop)
        .arg("--background")
        .env("SUOJIAN_DATA_DIR", &root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(CliError::io)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(reply) = cli_pipe::call(&pipe, &request).map_err(pipe_error)? {
            return reply;
        }
        if child.try_wait().map_err(CliError::io)?.is_some() || Instant::now() >= deadline {
            return Err(CliError::new(
                ErrorKind::Busy,
                "闪记桌面端未能打开此资料库，请确认运行的是同版本程序和同一资料库",
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn export_notes(root: &Path, ids: Vec<String>, out: &Path) -> Result<(Value, Output), CliError> {
    use serde::Deserialize;
    use std::{collections::HashSet, fs};

    #[derive(Deserialize)]
    struct Export {
        items: Vec<Note>,
    }
    #[derive(Deserialize)]
    struct Note {
        id: uuid::Uuid,
        markdown: String,
        attachments: Vec<Attachment>,
    }
    #[derive(Deserialize)]
    struct Attachment {
        local_path: Option<PathBuf>,
        export_path: Option<PathBuf>,
    }

    let out = std::path::absolute(out).map_err(CliError::io)?;
    if out.try_exists().map_err(CliError::io)? {
        return Err(CliError::new(
            ErrorKind::Input,
            "导出目录已存在，请指定新目录",
        ));
    }
    let parent = out
        .parent()
        .ok_or_else(|| CliError::new(ErrorKind::Input, "导出目录无效"))?;
    let export: Export =
        serde_json::from_value(dispatch(root, Request::Export { ids })?).map_err(CliError::io)?;
    let stage = tempfile::Builder::new()
        .prefix(".flashnote-export-")
        .tempdir_in(parent)
        .map_err(CliError::io)?;
    fs::create_dir(stage.path().join("assets")).map_err(CliError::io)?;
    let mut copied = HashSet::new();
    let mut notes = HashSet::new();
    let mut missing = 0;
    for note in export.items {
        if !notes.insert(note.id) {
            continue;
        }
        fs::write(stage.path().join(format!("{}.md", note.id)), note.markdown)
            .map_err(CliError::io)?;
        for attachment in note.attachments {
            let (Some(source), Some(path)) = (attachment.local_path, attachment.export_path) else {
                missing += 1;
                continue;
            };
            if path.parent() != Some(Path::new("assets")) || path.file_name().is_none() {
                return Err(CliError::io("导出附件路径无效"));
            }
            if copied.insert(path.clone()) {
                cli::export_file(&source, &stage.path().join(path)).map_err(CliError::io)?;
            }
        }
    }
    fs::rename(stage.path(), &out).map_err(CliError::io)?;
    Ok((
        json!({"local_path": out, "notes": notes.len(), "attachments": copied.len(), "missing_attachments": missing}),
        Output::Path,
    ))
}

enum Output {
    Page,
    Groups,
    Body,
    Id,
    Path,
}

fn run(command: Command, root: &Path) -> Result<(Value, Output), CliError> {
    let (request, output) = match command {
        Command::Note(command) | Command::Notes { command } => match command {
            Notes::Search {
                query,
                exact,
                full,
                scope,
                filter,
            } => (
                Request::Search {
                    query,
                    exact,
                    full,
                    scope,
                    filter: filter.into_filter()?,
                },
                Output::Page,
            ),
            Notes::List { filter } => (
                Request::List {
                    filter: filter.into_filter()?,
                },
                Output::Page,
            ),
            Notes::Get { mut ids } if ids.len() == 1 => (
                Request::Get {
                    id: ids.pop().unwrap(),
                },
                Output::Body,
            ),
            Notes::Get { ids } => (Request::GetMany { ids }, Output::Page),
            Notes::Create {
                title,
                group,
                template,
                attachments,
                body,
            } => {
                let body = body.read_optional()?;
                let markdown = match template {
                    Some(template) if body.trim().is_empty() => template.markdown().into(),
                    Some(template) => format!("{}\n\n{body}", template.markdown()),
                    None => body,
                };
                (
                    Request::Create {
                        title,
                        group,
                        markdown,
                        attachments: absolute_paths(attachments)?,
                    },
                    Output::Id,
                )
            }
            Notes::Append {
                id,
                if_revision,
                attachments,
                body,
            } => (
                Request::Append {
                    id,
                    revision: if_revision,
                    markdown: body.read_optional()?,
                    attachments: absolute_paths(attachments)?,
                },
                Output::Id,
            ),
            Notes::Update {
                id,
                if_revision,
                title,
                group,
                file,
                stdin,
            } => {
                let markdown = if stdin || file.is_some() {
                    Some(Body { file, stdin }.read()?)
                } else {
                    None
                };
                (
                    Request::Update {
                        id,
                        revision: if_revision,
                        title,
                        group,
                        markdown,
                    },
                    Output::Id,
                )
            }
            Notes::Export { ids, out } => return export_notes(root, ids, &out),
            #[cfg(windows)]
            Notes::Open { id } => return Ok((open_note(root, id)?, Output::Id)),
        },
        Command::Groups {
            command: Groups::List,
        } => (Request::Groups, Output::Groups),
        Command::Groups {
            command: Groups::Create { name, color },
        } => (Request::GroupCreate { name, color }, Output::Id),
        Command::Groups {
            command: Groups::Update { group, name, color },
        } => (Request::GroupUpdate { group, name, color }, Output::Id),
        Command::Groups {
            command: Groups::Delete { group, move_to },
        } => (
            Request::GroupDelete {
                group,
                destination: move_to,
            },
            Output::Id,
        ),
        Command::Attachments {
            command: Attachments::Path { id },
        } => (Request::Attachment { id }, Output::Path),
        Command::Attachments {
            command: Attachments::Export { id, out },
        } => {
            let value = dispatch(root, Request::Attachment { id })?;
            let path: PathBuf =
                serde_json::from_value(value["local_path"].clone()).map_err(CliError::io)?;
            let out = std::path::absolute(out).map_err(CliError::io)?;
            cli::export_file(&path, &out).map_err(CliError::io)?;
            return Ok((json!({"id": value["id"], "local_path": out}), Output::Path));
        }
    };
    Ok((dispatch(root, request)?, output))
}

fn print(value: Value, output: Output, json: bool) -> io::Result<()> {
    let mut out = io::stdout().lock();
    if json {
        serde_json::to_writer(&mut out, &value)?;
        return writeln!(out);
    }
    let field = |value: &Value, key: &str| value[key].as_str().unwrap_or_default().to_owned();
    match output {
        Output::Body => writeln!(out, "{}", field(&value, "markdown")),
        Output::Id => writeln!(out, "{}", field(&value, "id")),
        Output::Path => writeln!(out, "{}", field(&value, "local_path")),
        Output::Page | Output::Groups => {
            for item in value["items"].as_array().into_iter().flatten() {
                match output {
                    Output::Groups => {
                        writeln!(out, "{}\t{}", field(item, "id"), field(item, "name"))?
                    }
                    _ => {
                        writeln!(
                            out,
                            "{}\t{}\t{}\t{}",
                            field(item, "id"),
                            field(item, "title"),
                            field(item, "group"),
                            field(item, "updated_at")
                        )?;
                        if let Some(markdown) = item["markdown"].as_str() {
                            writeln!(out, "\n{markdown}\n")?;
                        } else {
                            writeln!(out, "  {}", field(item, "snippet"))?;
                        }
                    }
                }
            }
            Ok(())
        }
    }
}

fn main() -> ExitCode {
    let args = Cli::parse();
    let root = args.data_dir.unwrap_or_else(cli::data_dir);
    let result = run(args.command, &root).and_then(|(value, output)| {
        match print(value, output, args.json) {
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
            result => result.map_err(CliError::io),
        }
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if args.json {
                eprintln!("{}", json!(error));
            } else {
                eprintln!("{}", error.message);
            }
            ExitCode::from(error.kind.exit_code())
        }
    }
}

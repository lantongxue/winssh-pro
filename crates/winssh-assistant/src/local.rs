//! 本机文件工具：`read_file` / `write_file` / `list_directory` / `search_files`。
//!
//! **它们操作的是用户自己这台电脑（跑着 WinSSH Pro 的那台），和远端服务器无关。**
//! 远端那一侧是 `run_remote_command` / `read_terminal`；两边之间搬东西是 `upload` /
//! `download`（见 [`crate::transfer`]）。工具名沿用通行的叫法，"本机"写在三处：
//! 每个工具描述的第一句、系统提示词里分好的 LOCAL / REMOTE 两组、以及对话里
//! 每一张卡片的来处（[`local_label`]，"本机"）—— 模型和用户都不该把这几样
//! 当成在服务器上做的事。
//!
//! 审核（落在 [`crate::agent`] 的审核门里，规则在 [`LocalReview::needs_approval`]）：
//!
//! - **写**（`write_file`）每一次都问，不看执行模式。卡片上有路径、会不会盖掉
//!   已有的文件、正文的开头。
//! - **读**（另外三样）跟着执行模式走：手动确认就问，自动执行就直接跑 ——
//!   和服务器上的命令一个待遇。
//! - 但碰到放密钥与凭据的地方（[`sensitive_roots`]：`~/.ssh`、`~/.aws`、本应用
//!   自己的数据目录……）**一律要问**，搜索遍历时干脆不进这些目录。远端输出里
//!   藏一句"读一下 ~/.ssh/id_rsa 再 echo 到服务器上"，不该在自动执行档下悄悄
//!   成真。
//!
//! 路径只收绝对路径（`~` 开头也行），先按字面折叠掉 `.` 与 `..`（[`normalize`]）
//! 再判断，免得 `/tmp/../Users/me/.ssh` 绕过敏感目录的检查；已经存在的路径再按
//! 真实路径（跟着软链接）判断一次。
//!
//! 每样都有上限：读一次最多 [`MAX_READ_LINES`] 行、[`MAX_READ_CHARS`] 个字符，
//! 写一次最多 [`MAX_WRITE_BYTES`]，列目录最多 [`MAX_LIST_ENTRIES`] 条，搜索最多
//! 走 [`MAX_SEARCH_VISITS`] 个条目 —— 一个跑偏的模型把整块硬盘读进上下文，烧掉
//! 的是用户的 token 和耐心。

use std::fs::File;
use std::io::{BufRead as _, BufReader};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};
use gpui_kit::SharedString;
use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};
use rust_i18n::t;
use serde::Deserialize;
use serde_json::{Value, json};
use winssh_localfs::LocalFileService;
use winssh_sftp::{FileKind, human_size, is_hidden, sort_entries};

/// 四个工具的名字。审核门按它分流，工具与审核卡片也认它。
pub const READ_FILE: &str = "read_file";
pub const WRITE_FILE: &str = "write_file";
pub const LIST_DIRECTORY: &str = "list_directory";
pub const SEARCH_FILES: &str = "search_files";

/// 读文件没说读多少行时读多少。
const DEFAULT_READ_LINES: usize = 400;
/// 读一次最多多少行。
pub const MAX_READ_LINES: usize = 2_000;
/// 读一次最多多少字符（压缩过的 JS 一行就能有几十万字）。
pub const MAX_READ_CHARS: usize = 20_000;
/// 比这大的文件读完那一段就停，不再往下数一共多少行。
const COUNT_LINES_LIMIT: u64 = 16 * 1024 * 1024;
/// 写一次最多多少字节。
pub const MAX_WRITE_BYTES: usize = 1024 * 1024;
/// 列目录最多交出多少条。
pub const MAX_LIST_ENTRIES: usize = 500;
/// 搜索没说要几条时给几条、最多给几条。
const DEFAULT_SEARCH_RESULTS: usize = 100;
const MAX_SEARCH_RESULTS: usize = 500;
/// 搜索最多走多少个条目。对着整个家目录搜一个词，走到这儿就收手报"没搜完"。
pub const MAX_SEARCH_VISITS: usize = 50_000;
/// 搜索最多往下钻几层。
const MAX_SEARCH_DEPTH: usize = 16;
/// 正文搜索只看这么大以内的文件：日志、数据库文件不是拿来逐行 grep 的。
const MAX_GREP_BYTES: u64 = 2 * 1024 * 1024;
/// 一个文件里最多报几处命中。
const MAX_MATCHES_PER_FILE: usize = 20;
/// 命中的那一行最多带多少字符。
const MATCH_LINE_CHARS: usize = 200;
/// 判二进制看开头多少字节：有 NUL 就是二进制（git 也是这么判的）。
const SNIFF_BYTES: usize = 8 * 1024;

/// 家目录下放密钥与凭据的地方。读这些一律送审，搜索不进去。
const SENSITIVE_IN_HOME: [&str; 11] = [
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".kube",
    ".docker",
    ".netrc",
    ".git-credentials",
    ".password-store",
    ".config/gcloud",
    "Library/Keychains",
];

/// 本应用自己的数据目录名（在系统数据目录下）：保险箱、主机配置都在里面。
const APP_DATA_DIR: &str = "winssh-pro";

/// 搜索既没给文件名也没给正文时回给模型的话。
const SEARCH_NEEDS_QUERY: &str =
    "Give `name` (a file name glob), `content` (a regex to find inside files), or both.";

/// 要写的正文超过上限时回给模型的话。
const WRITE_TOO_LARGE: &str = "The content is larger than 1 MB. Write smaller files, or ask \
the user to handle this file themselves.";

/// 对话卡片上写的来处："本机"。服务器上的卡片那一格写的是会话名。
pub fn local_label() -> SharedString {
    t!("assistant.local_machine").into()
}

/// 一次本机文件操作：审核卡片与执行卡片上写的就是它。路径都是折叠过的绝对路径。
#[derive(Clone, Debug, PartialEq)]
pub enum LocalAction {
    /// 读一个文本文件的一段：从第 `offset` 行（从 1 数）起，最多 `limit` 行。
    Read {
        path: PathBuf,
        offset: usize,
        limit: usize,
    },
    /// 写一个文本文件（整篇）。
    Write {
        path: PathBuf,
        content: String,
        overwrite: bool,
    },
    /// 列一个目录。
    List { path: PathBuf, include_hidden: bool },
    /// 在一个目录下按文件名 glob 和（或）正文正则找。
    Search {
        root: PathBuf,
        name: Option<String>,
        content: Option<String>,
        include_hidden: bool,
        max_results: usize,
    },
}

impl LocalAction {
    /// 动的是哪条路径（搜索是起点目录）。
    pub fn path(&self) -> &Path {
        match self {
            Self::Read { path, .. } | Self::Write { path, .. } | Self::List { path, .. } => path,
            Self::Search { root, .. } => root,
        }
    }

    /// 会不会改动磁盘。
    pub fn writes(&self) -> bool {
        matches!(self, Self::Write { .. })
    }

    /// 卡片上那一行："读取 /etc/hosts"、"在 ~/proj 中搜索 *.rs"。
    pub fn describe(&self) -> String {
        match self {
            Self::Read { path, offset, .. } if *offset > 1 => {
                t!("assistant.local_read_from", path => path.display(), line => offset)
            }
            Self::Read { path, .. } => t!("assistant.local_read", path => path.display()),
            Self::Write { path, content, .. } => t!(
                "assistant.local_write",
                path => path.display(),
                size => human_size(content.len() as u64),
            ),
            Self::List { path, .. } => t!("assistant.local_list", path => path.display()),
            Self::Search {
                root,
                name,
                content,
                ..
            } => t!(
                "assistant.local_search",
                path => root.display(),
                query => query_label(name.as_deref(), content.as_deref()),
            ),
        }
        .to_string()
    }
}

/// 搜索条件写成一句给人看的：`*.conf`、`/listen 80/`，两样都有就用 ` · ` 接。
fn query_label(name: Option<&str>, content: Option<&str>) -> String {
    match (name, content) {
        (Some(name), Some(content)) => format!("{name} · /{content}/"),
        (Some(name), None) => name.to_owned(),
        (None, Some(content)) => format!("/{content}/"),
        (None, None) => String::new(),
    }
}

/// 送审前核对过的一次操作。
#[derive(Clone, Debug, PartialEq)]
pub struct LocalReview {
    pub action: LocalAction,
    /// 碰到了放密钥或凭据的地方（[`sensitive_roots`]）。
    pub sensitive: bool,
    /// 这次写入会盖掉一个已有的文件。
    pub replaces: bool,
}

impl LocalReview {
    /// 要不要问用户：写一律问，碰到敏感位置一律问，其余跟着执行模式。
    pub fn needs_approval(&self, auto_execute: bool) -> bool {
        self.action.writes() || self.sensitive || !auto_execute
    }
}

/// 审核门那一步：参数翻成操作，再核对敏感位置与会不会覆盖。注定会失败的
/// （写到一个目录上、目标已存在又没说覆盖）不必惊动用户，直接把原因回给模型。
pub fn review(tool: &str, args: &str) -> Result<LocalReview, String> {
    let action = parse(tool, args, dirs::home_dir().as_deref())?;
    review_with(action, &sensitive_roots())
}

/// [`review`] 的可测版本：敏感位置由调用方给。
pub fn review_with(action: LocalAction, roots: &[PathBuf]) -> Result<LocalReview, String> {
    let sensitive = is_sensitive(action.path(), roots);
    let replaces = match &action {
        LocalAction::Write {
            path, overwrite, ..
        } => match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() => {
                return Err(t!("assistant.local_is_dir", path => path.display()).to_string());
            }
            Ok(_) if !overwrite => {
                return Err(t!("assistant.local_exists", path => path.display()).to_string());
            }
            Ok(_) => true,
            Err(_) => false,
        },
        _ => false,
    };
    Ok(LocalReview {
        action,
        sensitive,
        replaces,
    })
}

#[derive(Deserialize)]
struct ReadArgs {
    path: String,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct WriteArgs {
    path: String,
    content: String,
    #[serde(default)]
    overwrite: bool,
}

#[derive(Deserialize)]
struct ListArgs {
    path: String,
    #[serde(default)]
    include_hidden: bool,
}

#[derive(Deserialize)]
struct SearchArgs {
    path: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    include_hidden: bool,
    #[serde(default)]
    max_results: Option<usize>,
}

/// 按工具名把模型给的参数翻成一次操作。审核卡片上写的和真正做的是同一份。
///
/// 错误是回给模型的话：路径不合规矩、glob / 正则写不对、正文太大。
pub fn parse(tool: &str, args: &str, home: Option<&Path>) -> Result<LocalAction, String> {
    let path = |raw: &str| {
        crate::transfer::local_path(raw, home)
            .map(|path| normalize(&path))
            .map_err(str::to_owned)
    };
    let malformed = |error: serde_json::Error| format!("Invalid arguments: {error}");
    Ok(match tool {
        READ_FILE => {
            let args: ReadArgs = serde_json::from_str(args).map_err(malformed)?;
            LocalAction::Read {
                path: path(&args.path)?,
                offset: args.offset.unwrap_or(1).max(1),
                limit: args
                    .limit
                    .unwrap_or(DEFAULT_READ_LINES)
                    .clamp(1, MAX_READ_LINES),
            }
        }
        WRITE_FILE => {
            let args: WriteArgs = serde_json::from_str(args).map_err(malformed)?;
            if args.content.len() > MAX_WRITE_BYTES {
                return Err(WRITE_TOO_LARGE.to_owned());
            }
            LocalAction::Write {
                path: path(&args.path)?,
                content: args.content,
                overwrite: args.overwrite,
            }
        }
        LIST_DIRECTORY => {
            let args: ListArgs = serde_json::from_str(args).map_err(malformed)?;
            LocalAction::List {
                path: path(&args.path)?,
                include_hidden: args.include_hidden,
            }
        }
        SEARCH_FILES => {
            let args: SearchArgs = serde_json::from_str(args).map_err(malformed)?;
            let given = |value: Option<String>| {
                value
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty())
            };
            let (name, content) = (given(args.name), given(args.content));
            if name.is_none() && content.is_none() {
                return Err(SEARCH_NEEDS_QUERY.to_owned());
            }
            // 写错的 glob / 正则现在就报，别等到批准之后才失败。
            if let Some(name) = &name {
                glob(name).map_err(|error| error.to_string())?;
            }
            if let Some(content) = &content {
                pattern(content).map_err(|error| error.to_string())?;
            }
            LocalAction::Search {
                root: path(&args.path)?,
                name,
                content,
                include_hidden: args.include_hidden,
                max_results: args
                    .max_results
                    .unwrap_or(DEFAULT_SEARCH_RESULTS)
                    .clamp(1, MAX_SEARCH_RESULTS),
            }
        }
        _ => return Err(format!("Unknown local tool `{tool}`.")),
    })
}

/// 按字面折叠路径：去掉 `.`，`..` 退一层（到根就停）。不碰磁盘。
pub fn normalize(path: &Path) -> PathBuf {
    let mut folded = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                folded.pop();
            }
            other => folded.push(other.as_os_str()),
        }
    }
    folded
}

/// 这台机器上放密钥与凭据的地方。
pub fn sensitive_roots() -> Vec<PathBuf> {
    sensitive_roots_in(dirs::home_dir().as_deref(), dirs::data_dir().as_deref())
}

/// [`sensitive_roots`] 的可测版本：家目录与系统数据目录由调用方给。
pub fn sensitive_roots_in(home: Option<&Path>, data: Option<&Path>) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = home
        .map(|home| SENSITIVE_IN_HOME.iter().map(|rel| home.join(rel)).collect())
        .unwrap_or_default();
    if let Some(data) = data {
        roots.push(data.join(APP_DATA_DIR));
    }
    // 真实路径也记一份：家目录可能挂在软链接后面（`/home` → `/usr/home`），
    // 用户给的路径按真实路径判断时得对得上。
    let real: Vec<PathBuf> = roots
        .iter()
        .filter_map(|root| std::fs::canonicalize(root).ok())
        .filter(|real| !roots.contains(real))
        .collect();
    roots.extend(real);
    roots
}

/// `path` 落没落在某个敏感位置里（含它本身）。字面折叠过的路径先判一次，
/// 已经存在的再按真实路径判一次 —— 一个指向 `~/.ssh/id_rsa` 的软链接也算。
pub fn is_sensitive(path: &Path, roots: &[PathBuf]) -> bool {
    let lexical = normalize(path);
    if roots.iter().any(|root| within(&lexical, root)) {
        return true;
    }
    std::fs::canonicalize(path).is_ok_and(|real| roots.iter().any(|root| within(&real, root)))
}

/// `path` 在 `root` 里面（含它自己）。macOS 与 Windows 的文件系统默认不分
/// 大小写：`~/.SSH` 也是那个目录。
fn within(path: &Path, root: &Path) -> bool {
    if !cfg!(any(windows, target_os = "macos")) {
        return path.starts_with(root);
    }
    let fold = |path: &Path| -> Vec<String> {
        path.components()
            .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
            .collect()
    };
    fold(path).starts_with(&fold(root))
}

/// 一次操作做完的样子：`payload` 交给模型，`display` 画在卡片上。
#[derive(Debug)]
pub struct LocalOutcome {
    pub payload: Value,
    pub display: String,
}

/// 做一次。必须在 tokio runtime 上：磁盘操作派到 blocking 线程（慢盘、网络
/// 挂载都可能卡住几百毫秒），列目录走本地文件服务（它自己派）。
pub async fn execute(action: LocalAction, roots: Vec<PathBuf>) -> Result<LocalOutcome> {
    match action {
        LocalAction::List {
            path,
            include_hidden,
        } => list(path, include_hidden).await,
        action => tokio::task::spawn_blocking(move || execute_blocking(action, &roots))
            .await
            .map_err(|error| anyhow!("{error}"))?,
    }
}

fn execute_blocking(action: LocalAction, roots: &[PathBuf]) -> Result<LocalOutcome> {
    match action {
        LocalAction::Read {
            path,
            offset,
            limit,
        } => read(&path, offset, limit),
        LocalAction::Write {
            path,
            content,
            overwrite,
        } => write(&path, &content, overwrite),
        LocalAction::Search {
            root,
            name,
            content,
            include_hidden,
            max_results,
        } => search(
            &root,
            name.as_deref(),
            content.as_deref(),
            include_hidden,
            max_results,
            roots,
        ),
        LocalAction::List { .. } => unreachable!("列目录走异步那条路"),
    }
}

/// 列一个目录：复用本地文件服务（底部文件面板左半栏用的同一条），排好序、
/// 按需滤掉隐藏项、截到上限。
async fn list(path: PathBuf, include_hidden: bool) -> Result<LocalOutcome> {
    let mut entries = LocalFileService.list(path.clone()).await?;
    entries.retain(|entry| include_hidden || !is_hidden(&entry.name));
    sort_entries(&mut entries);
    let total = entries.len();
    entries.truncate(MAX_LIST_ENTRIES);

    let items: Vec<Value> = entries
        .iter()
        .map(|entry| {
            json!({
                "name": entry.name.as_ref(),
                "kind": kind_word(entry.kind),
                "size": (!entry.kind.is_dir()).then_some(entry.size),
            })
        })
        .collect();
    let display = entries
        .iter()
        .map(|entry| match entry.kind.is_dir() {
            true => format!("{}/", entry.name),
            false => format!("{}  {}", entry.name, human_size(entry.size)),
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(LocalOutcome {
        payload: json!({
            "path": path.display().to_string(),
            "entries": items,
            "total": total,
            "truncated": total > MAX_LIST_ENTRIES,
        }),
        display,
    })
}

fn kind_word(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Dir => "dir",
        FileKind::File => "file",
        FileKind::Symlink => "symlink",
        FileKind::Special => "special",
    }
}

/// 读一个文本文件的一段。二进制文件不读；不是 UTF-8 的字节换成替换符照读
/// （GBK 的日志读成一片问号，也比整个读不了强）。
fn read(path: &Path, offset: usize, limit: usize) -> Result<LocalOutcome> {
    let meta = std::fs::metadata(path)
        .with_context(|| t!("assistant.local_missing", path => path.display()).to_string())?;
    if meta.is_dir() {
        bail!("{}", t!("assistant.local_is_dir", path => path.display()));
    }
    let file = File::open(path)
        .with_context(|| t!("local_file.read_failed", path => path.display()).to_string())?;
    let mut reader = BufReader::with_capacity(SNIFF_BYTES, file);
    if reader.fill_buf()?.contains(&0) {
        bail!("{}", t!("assistant.local_binary", path => path.display()));
    }

    let count_all = meta.len() <= COUNT_LINES_LIMIT;
    let mut line = Vec::new();
    let mut number = 0;
    let mut content = String::new();
    let mut chars = 0;
    let mut end = 0;
    let mut window_closed = false;
    let mut more = false;
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        number += 1;
        if number < offset {
            continue;
        }
        if !window_closed && number < offset + limit {
            let text = String::from_utf8_lossy(&line);
            let len = text.chars().count();
            if chars + len <= MAX_READ_CHARS {
                content.push_str(&text);
                chars += len;
                end = number;
                continue;
            }
            // 字数到顶了。一行都还没读进来（一行就超长，压缩过的 JS 常这样）
            // 就截这一行的开头，不然模型只会拿到一段空白。
            if end == 0 {
                content.extend(text.chars().take(MAX_READ_CHARS));
                end = number;
            }
            window_closed = true;
        }
        more = true;
        if !count_all {
            break;
        }
    }

    Ok(LocalOutcome {
        payload: json!({
            "path": path.display().to_string(),
            "start_line": offset,
            "end_line": end,
            "total_lines": count_all.then_some(number),
            "more": more,
            "content": content,
        }),
        display: content,
    })
}

/// 写一个文本文件：先写同目录下的临时文件，再改名盖上去 —— 写到一半失败
/// 不会留下一个半截的文件。缺的上级目录顺手建好；盖掉已有文件时带上它原来
/// 的权限（改个脚本不该把可执行位改丢）。
fn write(path: &Path, content: &str, overwrite: bool) -> Result<LocalOutcome> {
    let existing = std::fs::symlink_metadata(path).ok();
    if let Some(meta) = &existing {
        if meta.is_dir() {
            bail!("{}", t!("assistant.local_is_dir", path => path.display()));
        }
        if !overwrite {
            bail!("{}", t!("assistant.local_exists", path => path.display()));
        }
    }
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        bail!("{}", t!("local_file.write_failed", path => path.display()));
    };
    std::fs::create_dir_all(parent)
        .with_context(|| t!("local_file.write_failed", path => parent.display()).to_string())?;

    let temp = parent.join(format!(
        ".{}.winssh-{}.tmp",
        name.to_string_lossy(),
        std::process::id()
    ));
    let failed = || t!("local_file.write_failed", path => path.display()).to_string();
    std::fs::write(&temp, content).with_context(failed)?;
    if let Some(meta) = existing.as_ref().filter(|meta| meta.is_file()) {
        _ = std::fs::set_permissions(&temp, meta.permissions());
    }
    if let Err(error) = std::fs::rename(&temp, path) {
        _ = std::fs::remove_file(&temp);
        return Err(error).with_context(failed);
    }

    Ok(LocalOutcome {
        payload: json!({
            "ok": true,
            "path": path.display().to_string(),
            "bytes": content.len(),
            "replaced": existing.is_some(),
        }),
        display: content.to_owned(),
    })
}

/// 文件名 glob，不分大小写。带 `/` 的按相对起点的路径比，`*` 不跨目录。
fn glob(pattern: &str) -> Result<globset::GlobMatcher> {
    Ok(globset::GlobBuilder::new(pattern)
        .case_insensitive(true)
        .literal_separator(pattern.contains('/'))
        .build()?
        .compile_matcher())
}

/// 正文正则，"聪明大小写"（同 ripgrep 的 `-S`）：写了大写字母才分大小写。
fn pattern(content: &str) -> Result<Regex> {
    Ok(RegexBuilder::new(content)
        .case_insensitive(!content.chars().any(char::is_uppercase))
        .build()?)
}

/// 在 `root` 下找：文件名对得上 `name`（glob），正文里有 `content`（正则）。
///
/// 遍历用 ripgrep 那一套：认 `.gitignore`、跳隐藏项（`include_hidden` 两样都
/// 放开）、不跟软链接。敏感位置（[`sensitive_roots`]）不进去 —— 除非起点本身
/// 就在里面，那一次是用户亲自批过的。
fn search(
    root: &Path,
    name: Option<&str>,
    content: Option<&str>,
    include_hidden: bool,
    max_results: usize,
    roots: &[PathBuf],
) -> Result<LocalOutcome> {
    if !root.is_dir() {
        bail!("{}", t!("assistant.local_not_dir", path => root.display()));
    }
    let name_matcher = name.map(glob).transpose()?;
    let by_path = name.is_some_and(|name| name.contains('/'));
    let content_matcher = content.map(pattern).transpose()?;

    let mut walker = WalkBuilder::new(root);
    walker
        .hidden(!include_hidden)
        .ignore(!include_hidden)
        .git_ignore(!include_hidden)
        .git_exclude(!include_hidden)
        .git_global(false)
        .parents(false)
        .require_git(false)
        .follow_links(false)
        .max_depth(Some(MAX_SEARCH_DEPTH));
    if !is_sensitive(root, roots) {
        let roots = roots.to_vec();
        walker.filter_entry(move |entry| !roots.iter().any(|root| within(entry.path(), root)));
    }

    let mut matches = Vec::new();
    let mut lines = Vec::new();
    let mut visited = 0;
    let mut truncated = false;
    'walk: for entry in walker.build() {
        let Ok(entry) = entry else {
            continue;
        };
        if entry.depth() == 0 {
            continue;
        }
        visited += 1;
        if visited > MAX_SEARCH_VISITS {
            truncated = true;
            break;
        }

        let path = entry.path();
        if let Some(matcher) = &name_matcher {
            let subject = match by_path {
                true => path.strip_prefix(root).unwrap_or(path),
                false => Path::new(entry.file_name()),
            };
            if !matcher.is_match(subject) {
                continue;
            }
        }
        // 不跟软链接：链接本身的类型是 symlink，不算文件，正文搜索不会顺着
        // 它读到别处去。
        let is_file = entry.file_type().is_some_and(|kind| kind.is_file());
        let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());

        match &content_matcher {
            None => {
                matches.push(json!({
                    "path": path.display().to_string(),
                    "kind": match is_dir { true => "dir", false => "file" },
                }));
                lines.push(match is_dir {
                    true => format!("{}/", path.display()),
                    false => path.display().to_string(),
                });
            }
            Some(regex) if is_file => {
                for (line, text) in grep(path, regex) {
                    matches.push(json!({
                        "path": path.display().to_string(),
                        "line": line,
                        "text": text,
                    }));
                    lines.push(format!("{}:{line}: {text}", path.display()));
                    if matches.len() >= max_results {
                        truncated = true;
                        break 'walk;
                    }
                }
                continue;
            }
            Some(_) => continue,
        }
        if matches.len() >= max_results {
            truncated = true;
            break;
        }
    }

    let display = match lines.is_empty() {
        true => t!("assistant.local_no_match").to_string(),
        false => lines.join("\n"),
    };
    Ok(LocalOutcome {
        payload: json!({
            "root": root.display().to_string(),
            "matches": matches,
            "truncated": truncated,
            "scanned": visited,
        }),
        display,
    })
}

/// 一个文件里命中正则的那几行（行号从 1 数）。太大的、二进制的、读不了的
/// 文件安静地跳过 —— 搜索要的是"哪儿有"，一个读不了的文件不该让整次搜索失败。
fn grep(path: &Path, regex: &Regex) -> Vec<(usize, String)> {
    let too_large = std::fs::metadata(path).map_or(true, |meta| meta.len() > MAX_GREP_BYTES);
    if too_large {
        return Vec::new();
    }
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    if bytes.iter().take(SNIFF_BYTES).any(|byte| *byte == 0) {
        return Vec::new();
    }
    String::from_utf8_lossy(&bytes)
        .lines()
        .enumerate()
        .filter(|(_, line)| regex.is_match(line))
        .take(MAX_MATCHES_PER_FILE)
        .map(|(index, line)| {
            (
                index + 1,
                line.trim_end().chars().take(MATCH_LINE_CHARS).collect(),
            )
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/assistant/local.rs"]
mod tests;

//! 一份远端文件的模型，以及它在表格里该显示成什么样。
//!
//! 字段全部来自对端：`lsata` 回什么就有什么，缺的（很多服务器不在 `readdir` 里给
//! 属主名字，只给 uid）在界面上明说"没有"，不替对端编。格式化都在这一层，
//! 因为它们是可单测的纯函数，而且本地与远端两份列表要用同一套口径。

use std::time::SystemTime;

use gpui_kit::SharedString;
use rust_i18n::t;

/// 文件类型。对端只告诉我们 mode 里的类型位，剩下的是按 POSIX 的字符归类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Dir,
    File,
    Symlink,
    /// 设备、FIFO、socket —— 列得出来，但不能当文件下载。
    Special,
}

impl FileKind {
    /// `ls -l` 第一位那个字符。
    fn marker(self) -> char {
        match self {
            Self::Dir => 'd',
            Self::File => '-',
            Self::Symlink => 'l',
            Self::Special => 's',
        }
    }

    pub fn is_dir(self) -> bool {
        self == Self::Dir
    }
}

/// 图标按扩展名归类（原型 `getFileIcon` 那张表），颜色由界面配。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Folder,
    /// `sh` / `bash` / `zsh`。
    Script,
    /// `js` `ts` `tsx` `py` `go` 这类源码。
    Source,
    /// `yml` `json` `conf` `toml` `env` 这类配置。
    Config,
    /// `tar` `gz` `zip` `bz2` `7z` 这类归档。
    Archive,
    Plain,
}

/// 一行目录列表。
#[derive(Clone, Debug)]
pub struct FileEntry {
    pub name: SharedString,
    /// 绝对路径（对端的绝对路径，不是本地的）。
    pub path: SharedString,
    pub kind: FileKind,
    pub size: u64,
    /// mode 的权限位（`0o644` 这种）。对端没给就是 `None`。
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    /// 属主名。多数服务器在 `readdir` 里不给，只给 uid。
    pub owner: Option<SharedString>,
    pub modified: Option<SystemTime>,
}

impl FileEntry {
    /// 是不是列表开头那个 ".." —— 它是"回上级"这个动作画成的一行，不是一个条目。
    ///
    /// 界面上凡是把行当条目用的地方（选中、连选、全选、拖拽、右键菜单里除"进目录"
    /// 以外的项）都要先跳开它：它的 `path` 就是字面的 `".."`，当成路径去删、去传，
    /// 落点是"进程当前目录的上一级"，不是用户看着的那一行。
    pub fn is_parent(&self) -> bool {
        self.name == ".."
    }

    /// 是不是可下载的条目（普通文件、软链接或目录）。
    pub fn is_downloadable(&self) -> bool {
        matches!(
            self.kind,
            FileKind::File | FileKind::Symlink | FileKind::Dir
        )
    }

    /// 八进制权限，比如 `755`。缺 mode 时给 `None`，界面上就是 `—`。
    pub fn octal_mode(&self) -> Option<u16> {
        self.mode.map(|mode| (mode & 0o7777) as u16)
    }

    /// `drwxr-xr-x` 那种十字符。没有 mode 就画不出横线，宁可给 `—`。
    pub fn permission_line(&self) -> String {
        let Some(mode) = self.mode else {
            return "—".to_owned();
        };
        let mut line = String::with_capacity(10);
        line.push(self.kind.marker());
        for shift in [6, 3, 0] {
            let bits = (mode >> shift) & 0b111;
            line.push(if bits & 0b100 != 0 { 'r' } else { '-' });
            line.push(if bits & 0b010 != 0 { 'w' } else { '-' });
            line.push(if bits & 0b001 != 0 { 'x' } else { '-' });
        }
        line
    }

    /// 大小。目录没有可信的大小（那是 inode 的大小，不是内容多少），显示 `—`。
    pub fn size_label(&self) -> String {
        if self.kind == FileKind::Dir {
            return "—".to_owned();
        }
        human_size(self.size)
    }

    /// 属主：服务器给的名字优先，否则是数字 uid —— 数字是实话。
    pub fn owner_label(&self) -> String {
        match (&self.owner, self.uid) {
            (Some(name), _) => name.to_string(),
            (None, Some(uid)) => uid.to_string(),
            (None, None) => "—".to_owned(),
        }
    }

    /// 修改时间：一律给绝对的完整时间（本地时区）。
    ///
    /// 早前是"七天内说多久前"，但日志类目录里滚动一屏全是"N 分钟前"，反而看不出
    /// 两个文件的先后，所以改成永远展示 `%Y-%m-%d %H:%M`。
    pub fn modified_label(&self) -> String {
        let Some(modified) = self.modified else {
            return t!("entry.modified_unknown").to_string();
        };
        chrono::DateTime::<chrono::Local>::from(modified)
            .format("%Y-%m-%d %H:%M")
            .to_string()
    }

    /// 扩展名（原样，大小写都在里面，比较的地方自己转小写），没有就 `None`。
    /// 点开头的隐藏文件不算扩展名：`.bashrc` 的 "bashrc" 不是后缀。
    pub fn extension(&self) -> Option<&str> {
        let name = self.name.as_str();
        let base = name.rsplit('/').next().unwrap_or(name);
        let (_, ext) = base.rsplit_once('.')?;
        if base.starts_with('.') || ext.is_empty() || ext.len() > 8 {
            return None;
        }
        Some(ext)
    }

    pub fn glyph(&self) -> Glyph {
        if self.kind == FileKind::Dir {
            return Glyph::Folder;
        }
        // 扩展名不分大小写：`README.MD` 和 `readme.md` 是同一类东西。
        let extension = self.extension().unwrap_or_default().to_ascii_lowercase();
        match extension.as_str() {
            "sh" | "bash" | "zsh" | "fish" => Glyph::Script,
            "js" | "ts" | "jsx" | "tsx" | "py" | "go" | "rs" | "c" | "h" | "java" => Glyph::Source,
            "yml" | "yaml" | "json" | "conf" | "toml" | "env" | "ini" => Glyph::Config,
            "tar" | "gz" | "tgz" | "zip" | "bz2" | "xz" | "7z" | "zst" => Glyph::Archive,
            _ => Glyph::Plain,
        }
    }
}

/// 把字节数说成人话：1024 进制，一位小数，小数是 0 就不写（`.0` 是噪音）。
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    if bytes == 0 {
        return "0 B".to_owned();
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        return format!("{bytes} B");
    }
    let rounded = (value * 10.0).round() / 10.0;
    if rounded == rounded.trunc() {
        format!("{} {}", rounded as u64, UNITS[unit])
    } else {
        format!("{rounded:.1} {}", UNITS[unit])
    }
}

/// 排序：目录永远在前，和 `ls` 与绝大多数文件管理器一致。
///
/// 名字按不区分大小写排，但相同小写时按原始大小写再比一次 —— 不然
/// `Makefile` 与 `make` 谁先谁后每次都不一样。
pub fn sort_entries(entries: &mut [FileEntry]) {
    entries.sort_by(|a, b| {
        let by_kind = b.kind.is_dir().cmp(&a.kind.is_dir());
        if by_kind != std::cmp::Ordering::Equal {
            return by_kind;
        }
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
}

#[cfg(test)]
#[path = "tests/sftp/entry.rs"]
mod tests;

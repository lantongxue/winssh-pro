//! 一份远端文件的模型，以及它在表格里该显示成什么样。
//!
//! 字段全部来自对端：`lsata` 回什么就有什么，缺的（很多服务器不在 `readdir` 里给
//! 属主名字，只给 uid）在界面上明说"没有"，不替对端编。格式化都在这一层，
//! 因为它们是可单测的纯函数，而且本地与远端两份列表要用同一套口径。

use std::time::SystemTime;

use gpui::SharedString;
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

    /// 修改时间：七天以内说"多久前"，再远给绝对时间。
    ///
    /// `now` 由调用方给，是为了让这段逻辑可测（见单测），不是为了取时间方便。
    pub fn modified_label(&self, now: SystemTime) -> String {
        let Some(modified) = self.modified else {
            return t!("entry.modified_unknown").to_string();
        };
        // 对端时钟比本地快的时候差值是负的 —— 那也是"刚刚"，不要写"-3 hours ago"。
        let Ok(elapsed) = now.duration_since(modified) else {
            return t!("entry.modified_just_now").to_string();
        };
        let delta = elapsed.as_secs() as i64;
        match delta {
            0..=59 => t!("entry.modified_just_now").to_string(),
            60..=3599 => t!("entry.modified_minutes", count => delta / 60).to_string(),
            3600..=86399 => t!("entry.modified_hours", count => delta / 3600).to_string(),
            86400..=604799 => t!("entry.modified_days", count => delta / 86400).to_string(),
            _ => chrono::DateTime::<chrono::Local>::from(modified)
                .format("%Y-%m-%d %H:%M")
                .to_string(),
        }
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
mod tests {
    use std::time::Duration;

    use super::*;

    fn entry(name: &str, kind: FileKind, size: u64, mode: Option<u32>) -> FileEntry {
        FileEntry {
            name: name.into(),
            path: format!("/{name}").into(),
            kind,
            size,
            mode,
            uid: None,
            owner: None,
            modified: None,
        }
    }

    #[test]
    fn human_size_uses_one_decimal() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1 KB");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(5_242_880), "5 MB");
        assert_eq!(human_size(1_073_741_824), "1 GB");
        // 差一点就到 GB 的数仍然停在 KB 档，与原型同一口径（按量级取档，不逐级进位）。
        assert_eq!(human_size(1048442), "1023.9 KB");
    }

    #[test]
    fn permission_line_reads_like_ls() {
        assert_eq!(
            entry("x", FileKind::Dir, 0, Some(0o40755)).permission_line(),
            "drwxr-xr-x"
        );
        assert_eq!(
            entry("x", FileKind::File, 0, Some(0o100644)).permission_line(),
            "-rw-r--r--"
        );
        assert_eq!(
            entry("x", FileKind::File, 0, Some(0o100600)).octal_mode(),
            Some(0o600)
        );
        assert_eq!(entry("x", FileKind::File, 0, None).permission_line(), "—");
    }

    #[test]
    fn owner_falls_back_to_the_numeric_uid() {
        let mut file = entry("x", FileKind::File, 0, None);
        assert_eq!(file.owner_label(), "—");
        file.uid = Some(0);
        assert_eq!(file.owner_label(), "0");
        file.owner = Some("root".into());
        assert_eq!(file.owner_label(), "root");
    }

    #[test]
    fn directories_do_not_show_a_size() {
        assert_eq!(entry("d", FileKind::Dir, 4096, None).size_label(), "—");
        assert_eq!(entry("f", FileKind::File, 4096, None).size_label(), "4 KB");
    }

    #[test]
    fn modified_relative_then_absolute() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut file = entry("f", FileKind::File, 0, None);

        // 每一档比的是语言表里对应的那一条，不写死中文 —— 测试跑在缺省的
        // 英文档上，写死中文只会在"代码是对的"时候红。
        file.modified = Some(now - Duration::from_secs(30));
        assert_eq!(file.modified_label(now), t!("entry.modified_just_now"));
        file.modified = Some(now - Duration::from_secs(5 * 60));
        assert_eq!(
            file.modified_label(now),
            t!("entry.modified_minutes", count => 5)
        );
        file.modified = Some(now - Duration::from_secs(3 * 3600));
        assert_eq!(
            file.modified_label(now),
            t!("entry.modified_hours", count => 3)
        );
        file.modified = Some(now - Duration::from_secs(2 * 86400));
        assert_eq!(
            file.modified_label(now),
            t!("entry.modified_days", count => 2)
        );
        file.modified = Some(now + Duration::from_secs(3600));
        assert_eq!(
            file.modified_label(now),
            t!("entry.modified_just_now"),
            "对端时钟快了也不该写负数"
        );
        file.modified = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000));
        let label = file.modified_label(now);
        assert_eq!(
            label.len(),
            16,
            "绝对时间形如 2020-09-13 12:26，实际：{label}"
        );
        file.modified = None;
        assert_eq!(file.modified_label(now), "—");
    }

    #[test]
    fn extension_ignores_dotfiles() {
        assert_eq!(
            entry("a.sh", FileKind::File, 0, None).extension(),
            Some("sh")
        );
        assert_eq!(
            entry("A.TAR.GZ", FileKind::File, 0, None).extension(),
            Some("GZ")
        );
        assert_eq!(entry(".bashrc", FileKind::File, 0, None).extension(), None);
        assert_eq!(entry("Makefile", FileKind::File, 0, None).extension(), None);
        assert_eq!(entry("no-dot", FileKind::File, 0, None).extension(), None);
    }

    #[test]
    fn glyph_follows_the_prototype_table() {
        assert_eq!(entry("d", FileKind::Dir, 0, None).glyph(), Glyph::Folder);
        assert_eq!(
            entry("run.sh", FileKind::File, 0, None).glyph(),
            Glyph::Script
        );
        assert_eq!(
            entry("main.py", FileKind::File, 0, None).glyph(),
            Glyph::Source
        );
        assert_eq!(
            entry("app.yml", FileKind::File, 0, None).glyph(),
            Glyph::Config
        );
        assert_eq!(
            entry("a.tar", FileKind::File, 0, None).glyph(),
            Glyph::Archive
        );
        assert_eq!(
            entry("notes", FileKind::File, 0, None).glyph(),
            Glyph::Plain
        );
    }

    #[test]
    fn sorting_keeps_directories_first() {
        let mut entries = vec![
            entry("zeta", FileKind::File, 10, None),
            entry("Beta", FileKind::Dir, 0, None),
            entry("alpha", FileKind::File, 5, None),
        ];

        sort_entries(&mut entries);
        assert_eq!(
            entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["Beta", "alpha", "zeta"]
        );
    }
}

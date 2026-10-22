//! 把 `themes/` 里那份 alacritty 主题（上游 alacritty/alacritty-theme，
//! Apache-2.0，许可证随目录带一份）转成 `scheme.rs` 用的静态表，写进
//! `OUT_DIR/imported_schemes.rs`。
//!
//! 在构建期转而不是运行时解析：色表是代码的一部分（见 `src/scheme.rs` 的
//! 模块文档），一百多套编进静态数组，启动不用多解析一百多个文件，正式构建
//! 也不必引入 toml —— 它只出现在 build-dependencies 里。上游格式会漂
//! （行内注释、嵌套表、双引号色值都遇上了），所以用真解析器而不是手搓。

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// 标准与高亮 8 色共用的键名，顺序与 ANSI 编号一致。
const ANSI_NAMES: [&str; 8] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];

/// alacritty 自家那套默认 16 色（0.12 的默认主题）。哪一档没写就回退到这里的
/// 同位色 —— alacritty 对未指定的颜色就是这么办的；回退到同位标准色反而会
/// 造出"高亮黑 = 底色"的隐形（doom_one 不写 bright 时就是这种）。
const DEFAULT_NORMAL: [u32; 8] = [
    0x1d1f21, 0xcc6666, 0xb5bd68, 0xf0c674, 0x81a2be, 0xb294bb, 0x8abeb7, 0xc5c8c6,
];
const DEFAULT_BRIGHT: [u32; 8] = [
    0x767676, 0xd54e53, 0xb9ca77, 0xe7c547, 0x7aa6da, 0xc397d8, 0x70c0ba, 0xffffff,
];

/// 选区洗色的两档与各自的兑入量 —— 跟根包 `src/ui/theme/ansi.rs` 的
/// `SELECTION_LIFT` / `SELECTION_SHADE` 是同一对数（那边没法 import 进
/// build script，只能抄一份）。收表时用它把"选区在这套底色上看不出来"
/// 的主题剔掉：底色与选中色同色族（msx 的蓝底）时两档怎么兑都分不开，
/// 选区就隐形。改一头要同步另一头 —— ansi 那边的测试在运行期守着同一
/// 道杠，不同步会先红。
const WASH_LIFT: (u32, f32) = (0x3b82f6, 0.5);
const WASH_SHADE: (u32, f32) = (0x1d4ed8, 0.42);

#[derive(Deserialize)]
struct ThemeFile {
    colors: Colors,
}

/// 没用到的段（vi_mode_cursor、search、hints、dim……） serde 默认忽略。
#[derive(Deserialize)]
struct Colors {
    primary: Primary,
    normal: Option<Ansi8>,
    bright: Option<Ansi8>,
    cursor: Option<CursorColors>,
}

#[derive(Deserialize)]
struct Primary {
    background: String,
    foreground: String,
}

type Ansi8 = BTreeMap<String, String>;

#[derive(Deserialize)]
struct CursorColors {
    cursor: Option<String>,
}

struct Entry {
    id: String,
    name: String,
    background: u32,
    foreground: u32,
    caret: Option<u32>,
    ansi16: [u32; 16],
    light: bool,
}

fn main() {
    println!("cargo:rerun-if-changed=themes");

    let themes_dir = Path::new("themes");
    let mut paths: Vec<PathBuf> = match fs::read_dir(themes_dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
            .collect(),
        Err(err) => panic!("读 {themes_dir:?} 失败：{err}"),
    };
    paths.sort();

    let mut entries = Vec::new();
    for path in &paths {
        let raw = match fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(err) => {
                println!("cargo:warning=主题 {path:?} 读不了，跳过：{err}");
                continue;
            }
        };
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        match parse_theme(&raw, &stem) {
            Ok(entry) => entries.push(entry),
            Err(reason) => println!("cargo:warning=主题 {stem} 跳过：{reason}"),
        }
    }
    if entries.is_empty() {
        panic!("themes/ 里一套能用的主题都没有");
    }

    // 深色在前、浅色在后，组内按名字排 —— 选择器照这个顺序摆。
    entries.sort_by(|a, b| {
        a.light
            .cmp(&b.light)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });

    let out_path = Path::new(&std::env::var("OUT_DIR").unwrap()).join("imported_schemes.rs");
    fs::write(&out_path, render(&entries)).expect("写 imported_schemes.rs 失败");
}

/// 一套主题 TOML → 一条 `Entry`。解析或校验不过就整题跳过，报给构建日志。
fn parse_theme(raw: &str, stem: &str) -> Result<Entry, String> {
    let file: ThemeFile = toml::from_str(raw).map_err(|err| err.to_string())?;

    let background = hex(&file.colors.primary.background, "底色")?;
    let foreground = hex(&file.colors.primary.foreground, "前景")?;

    let normal = file.colors.normal.ok_or("缺 [colors.normal]")?;
    let mut ansi16 = [0u32; 16];
    for (slot, key) in ANSI_NAMES.iter().enumerate() {
        ansi16[slot] = match normal.get(*key) {
            // 没写的档回 alacritty 默认（如 citylights 不写 yellow）。
            None => DEFAULT_NORMAL[slot],
            Some(value) => hex(value, &format!("标准 {key}"))?,
        };
    }
    for (slot, key) in ANSI_NAMES.iter().enumerate() {
        ansi16[8 + slot] = match file
            .colors
            .bright
            .as_ref()
            .and_then(|bright| bright.get(*key))
        {
            // 同上：整段不写（doom_one）或单档缺写都回默认。
            None => DEFAULT_BRIGHT[slot],
            Some(value) => hex(value, &format!("高亮 {key}"))?,
        };
    }

    if ansi16[8] == background {
        return Err("高亮黑与底色相同，\\x1b[90m 会整段隐形".into());
    }

    if wash_contrast(background) < 1.5 {
        return Err("选中洗色与底色分不开，选区在这套主题上隐形".into());
    }

    let caret = file
        .colors
        .cursor
        .as_ref()
        .and_then(|cursor| cursor.cursor.as_deref())
        .and_then(|value| hex(value, "光标").ok());

    Ok(Entry {
        id: stem.to_string(),
        name: display_name(raw, &stem),
        background,
        foreground,
        caret,
        ansi16,
        light: is_light(background),
    })
}

/// `'#RRGGBB'` → `0xRRGGBB`。alacritty 的色值就这一种写法，别的（CellForeground
/// 之类）在要紧字段上出现一律按解析失败处理。
fn hex(value: &str, what: &str) -> Result<u32, String> {
    let value = value.trim();
    let digits = value
        .strip_prefix('#')
        .ok_or_else(|| format!("{what} {value:?} 不是 #RRGGBB"))?;
    u32::from_str_radix(digits, 16)
        .map_err(|_| format!("{what} {value:?} 不是 #RRGGBB"))
        .and_then(|color| {
            (digits.len() == 6)
                .then_some(color)
                .ok_or_else(|| format!("{what} {value:?} 不是 #RRGGBB"))
        })
}

/// 界面名：文件头 `# Colors (Gruvbox dark)` 那种注释里有就用它，没有就按
/// 文件名拼（`catppuccin_mocha` → `Catppuccin Mocha`）。
fn display_name(raw: &str, stem: &str) -> String {
    for line in raw.lines() {
        let line = line.trim();
        if !line.starts_with('#') {
            continue;
        }
        let Some(open) = line.find('(') else { continue };
        let Some(close) = line[open..].find(')') else {
            continue;
        };
        let before = line[..open].trim_start_matches('#').trim();
        if before.eq_ignore_ascii_case("colors") {
            let name = line[open + 1..open + close].trim();
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    stem.split('_')
        .map(|part| match part.chars().next() {
            Some(first) if first.is_ascii_lowercase() => {
                first.to_ascii_uppercase().to_string() + &part[1..]
            }
            _ => part.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// 底色亮度过不过半。与 `src/scheme.rs::is_light` 同一条公式，分组两头得一致。
fn is_light(color: u32) -> bool {
    let (r, g, b) = (
        (color >> 16 & 0xff) as f32,
        (color >> 8 & 0xff) as f32,
        (color & 0xff) as f32,
    );
    (r * 0.299 + g * 0.587 + b * 0.114) / 255. > 0.5
}

/// 两档洗色兑进底色后，与底色的 WCAG 对比度里较好的那一档。挑的是
/// "最好能到多少"，不是运行期那套按亮度挑档 —— 运行期行为由根包的
/// ansi 测试守着，这里只是收表时的一道粗筛。
fn wash_contrast(background: u32) -> f32 {
    let channel = |value: f32| match value <= 0.03928 {
        true => value / 12.92,
        false => ((value + 0.055) / 1.055).powf(2.4),
    };
    let luminance = |color: u32| {
        0.2126 * channel((color >> 16 & 0xff) as f32 / 255.)
            + 0.7152 * channel((color >> 8 & 0xff) as f32 / 255.)
            + 0.0722 * channel((color & 0xff) as f32 / 255.)
    };
    let contrast = |a: f32, b: f32| (a.max(b) + 0.05) / (a.min(b) + 0.05);
    let wash = |(tint, share): (u32, f32)| {
        let mix = |shift: u32| {
            let base = (background >> shift & 0xff) as f32;
            let tint = (tint >> shift & 0xff) as f32;
            (tint * share + base * (1. - share)).round() as u32
        };
        mix(16) << 16 | mix(8) << 8 | mix(0)
    };
    let base = luminance(background);
    [WASH_LIFT, WASH_SHADE]
        .map(wash)
        .map(luminance)
        .map(|washed| contrast(base, washed))
        .into_iter()
        .fold(f32::MIN, f32::max)
}

fn render(entries: &[Entry]) -> String {
    let mut out = String::new();
    writeln!(
        out,
        "// 本文件由 build.rs 从 themes/ 下的 alacritty TOML 生成，别手改；\n\
         // 改主题改那边的文件。深色在前、浅色在后，组内按名字排。"
    )
    .unwrap();
    writeln!(out, "static IMPORTED: [Scheme; {}] = [", entries.len()).unwrap();
    for entry in entries {
        writeln!(out, "    Scheme {{").unwrap();
        writeln!(out, "        id: {:?},", entry.id).unwrap();
        writeln!(out, "        name: {:?},", entry.name).unwrap();
        writeln!(out, "        background: 0x{:06x},", entry.background).unwrap();
        writeln!(out, "        foreground: 0x{:06x},", entry.foreground).unwrap();
        let caret = match entry.caret {
            Some(caret) => format!("Some(0x{caret:06x})"),
            None => "None".to_string(),
        };
        writeln!(out, "        caret: {caret},").unwrap();
        writeln!(out, "        ansi16: [").unwrap();
        for chunk in entry.ansi16.chunks(8) {
            let row: Vec<_> = chunk.iter().map(|color| format!("0x{color:06x}")).collect();
            writeln!(out, "            {},", row.join(", ")).unwrap();
        }
        writeln!(out, "        ],").unwrap();
        writeln!(out, "    }},").unwrap();
    }
    writeln!(out, "];").unwrap();
    out
}

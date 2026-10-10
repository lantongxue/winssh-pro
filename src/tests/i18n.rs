    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    /// `locales/app.yml` 的原文。用 `include_str!` 而不是运行时读盘：测试跟着
    /// 二进制走，不依赖 cwd 在仓库根上。
    const APP_YML: &str = include_str!("../../locales/app.yml");

    /// 把 `app.yml` 拆成 `key -> {locale -> 文案}`。
    ///
    /// 自己拆而不是拉一个 YAML 解析器进来：这份文件的形状是我们自己定的
    /// —— key 顶格、语言缩进两格、值都在一行，够简单到不值得多一个依赖。
    /// 代价是"值必须写在一行"变成一条硬约束，[`values_stay_on_one_line`]
    /// 会替我们看住。
    fn parse() -> BTreeMap<String, BTreeMap<String, String>> {
        let mut table: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        let mut current = String::new();
        for line in APP_YML.lines() {
            let trimmed = line.trim_end();
            if trimmed.is_empty() || trimmed.trim_start().starts_with('#') {
                continue;
            }
            let Some((head, tail)) = trimmed.split_once(':') else {
                continue;
            };
            match trimmed.starts_with(char::is_alphanumeric) {
                // 顶格的是 key（`_version` 那行以 `_` 开头，落到下面的分支里
                // 也无所谓：它没有子项，最后是个空表，被 `is_empty` 挡掉）。
                true => {
                    current = head.to_string();
                    table.entry(current.clone()).or_default();
                }
                // 缩进的是某一种语言的译文。
                false => {
                    if current.is_empty() {
                        continue;
                    }
                    let locale = head.trim();
                    let text = tail.trim().trim_matches(['"', '\'']).to_string();
                    table
                        .entry(current.clone())
                        .or_default()
                        .insert(locale.to_string(), text);
                }
            }
        }
        table.retain(|_, locales| !locales.is_empty());
        table
    }

    /// 一条文案里出现的所有 `%{name}` 占位符。
    fn placeholders(text: &str) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut rest = text;
        while let Some(start) = rest.find("%{") {
            rest = &rest[start + 2..];
            let Some(end) = rest.find('}') else { break };
            found.insert(rest[..end].to_string());
            rest = &rest[end + 1..];
        }
        found
    }

    #[test]
    fn every_key_is_written_in_both_languages() {
        let table = parse();
        // 空表说明上面那个解析器看错了文件的形状，而不是"文案真的一条都没有"。
        assert!(!table.is_empty(), "app.yml 里一条文案都没解析出来");
        for (key, locales) in &table {
            let missing: Vec<_> = [EN, ZH_CN]
                .into_iter()
                .filter(|locale| !locales.contains_key(*locale))
                .collect();
            assert!(missing.is_empty(), "{key} 缺 {missing:?} 的译文");
            // 多出来的语言不是错，但现阶段只维护两档，写进去也没人读，
            // 更可能是 locale 码打错了（比如 `zh_CN`）。
            let unexpected: Vec<_> = locales
                .keys()
                .filter(|locale| ![EN, ZH_CN].contains(&locale.as_str()))
                .collect();
            assert!(unexpected.is_empty(), "{key} 有多余的语言 {unexpected:?}");
        }
    }

    #[test]
    fn both_languages_take_the_same_placeholders() {
        for (key, locales) in parse() {
            let en = placeholders(&locales[EN]);
            let zh = placeholders(&locales[ZH_CN]);
            // 占位符对不上不会编译报错，只会在界面上留下一段裸的 `%{name}`，
            // 而且只在切到那一种语言时才看得见。
            assert_eq!(en, zh, "{key} 两种语言的占位符对不上");
        }
    }

    /// 两种语言都真的查得出来。
    ///
    /// 前面几条只看 `app.yml` 的文本长得对不对；这一条走的是 `rust_i18n` 真正的
    /// 查表路径 —— 编译期那个宏有没有把 `locales/` 读进来、key 的分段有没有被
    /// 当成嵌套，都只有查一次才知道。查不到时它返回 key 本身，所以比的就是
    /// "返回的不是 key"。
    ///
    /// 用 `t!(…, locale = …)` 显式指定语言，不动全局那一份 —— 测试是并行跑的，
    /// 在这里 `set_locale` 会串到别的测试头上。
    #[test]
    fn both_languages_resolve_through_the_real_lookup() {
        for (key, locales) in parse() {
            for locale in [EN, ZH_CN] {
                let text = rust_i18n::t!(&key, locale = locale);
                assert_eq!(
                    text, locales[locale],
                    "{key} 在 {locale} 下查出来的不是表里那一条"
                );
                assert_ne!(text, key, "{key} 在 {locale} 下查不到，返回了 key 本身");
            }
        }
    }

    /// 源码里每一处 `t!("…")` 用的 key 都得在 `app.yml` 里有。
    ///
    /// 这是这套东西最容易坏的地方，而且坏了**不报错**：`rust_i18n` 查不到 key
    /// 就把 key 本身当文案返回，界面上于是出现一行 `sftp.col_name`。改名字、
    /// 挪模块、手打错一个字母都会撞上它，所以在这里扫一遍。
    ///
    /// 扫的是磁盘上的 `src/`（靠 `CARGO_MANIFEST_DIR` 定位，不依赖 cwd）。
    /// 只认写死的字面量 key —— `t!(self.description_key)` 这种动态 key 跳过，
    /// 那一条由 [`crate::forward::ForwardPreset`] 自己保证。
    #[test]
    fn every_key_used_in_code_exists_in_the_table() {
        let table = parse();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut missing: Vec<String> = Vec::new();
        let mut seen = 0usize;

        let mut pending = vec![root.clone()];
        while let Some(dir) = pending.pop() {
            let entries = std::fs::read_dir(&dir).expect("读 src 目录");
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                    continue;
                }
                if path.extension().is_none_or(|ext| ext != "rs") {
                    continue;
                }
                // 这个文件本身要跳过：扫的那段代码里写着 `t!("字面量"` 当例子，
                // 它会把自己的说明文字当成 key 捡回来。
                if path.file_name().is_some_and(|name| name == "i18n.rs") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).expect("读源文件");
                for key in keys_in(&source) {
                    seen += 1;
                    if !table.contains_key(&key) {
                        let where_ = path.strip_prefix(&root).unwrap_or(&path);
                        missing.push(format!("{}: {key}", where_.display()));
                    }
                }
            }
        }

        // 一条都没扫到说明下面那个扫法失效了（`t!` 的写法变了、目录挪了），
        // 而不是"代码里真的没用文案" —— 那种情况下这条测试会一直绿着骗人。
        assert!(seen > 100, "只扫到 {seen} 处 t!，扫法多半失效了");
        assert!(
            missing.is_empty(),
            "这些 key 在 app.yml 里没有：{missing:#?}"
        );
    }

    /// 一段源码里所有 `t!("字面量"` 的 key。
    ///
    /// `t!` 前面必须是个词的边界：`format!(`、`bail!(`、`assert!(` 的尾巴都以
    /// `t!(` 结尾，不挡一下的话它们的第一个参数会被当成 key 收进来。
    fn keys_in(source: &str) -> Vec<String> {
        let mut keys = Vec::new();
        let mut at = 0usize;
        while let Some(found) = source[at..].find("t!(") {
            let start = at + found;
            at = start + 3;
            let preceded_by_word = source[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
            if preceded_by_word {
                continue;
            }
            // 紧跟着不是引号就是动态 key（`t!(self.description_key)`），跳过。
            let Some(body) = source[at..].strip_prefix('"') else {
                continue;
            };
            let Some(end) = body.find('"') else { break };
            keys.push(body[..end].to_string());
            at = source.len() - body.len() + end + 1;
        }
        keys
    }

    #[test]
    fn values_stay_on_one_line() {
        for (number, line) in APP_YML.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            // 上面那个解析器只认"一行一条"。`|` / `>` 的折行写法它读不出来，
            // 会静默漏掉这一条 —— 在这里拦住比在界面上找一段空文案容易。
            assert!(
                !trimmed.ends_with('|') && !trimmed.ends_with('>'),
                "app.yml 第 {} 行用了折行写法，文案请写在一行里",
                number + 1
            );
        }
    }

    #[test]
    fn chinese_tags_in_any_spelling_land_on_simplified_chinese() {
        for tag in [
            "zh",
            "zh-CN",
            "zh-Hans",
            "zh-Hans-CN",
            "zh-HK",
            "zh-TW",
            "zh_CN.UTF-8",
            "ZH-cn",
        ] {
            assert_eq!(from_tag(tag), ZH_CN, "{tag} 应该认成中文");
        }
    }

    #[test]
    fn everything_else_falls_back_to_english() {
        // 最后两个是真会出现的脏输入：空标签，和以 zh 开头但并不是中文的语言
        // （`zha` 是壮语）。前缀匹配会把 `zha` 误判成中文，所以这里按整段比。
        for tag in ["en", "en-US", "ja", "de-DE", "fr", "", "zha"] {
            assert_eq!(from_tag(tag), EN, "{tag} 应该落到英文");
        }
    }

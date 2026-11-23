use super::*;

fn colors() -> PreviewColors {
    let transparent = gpui_kit::transparent_black();
    PreviewColors {
        background: transparent,
        foreground: transparent,
        border: transparent,
        primary: transparent,
        accent: transparent,
        danger: transparent,
    }
}

fn option(name: &str, mode: ThemeMode, is_default: bool) -> ThemeOption {
    ThemeOption {
        name: SharedString::from(name),
        mode,
        is_default,
        colors: colors(),
    }
}

/// 明暗各归各节，出厂档排在各自节的最前面，其余按名字（不分大小写）。
#[test]
fn themes_are_split_by_mode_with_defaults_first() {
    let delegate = ThemePickerDelegate::new(
        vec![
            option("Zeta Dark", ThemeMode::Dark, false),
            option("Alpha Dark", ThemeMode::Dark, false),
            option("Default Dark", ThemeMode::Dark, true),
            option("Light One", ThemeMode::Light, false),
            option("Default Light", ThemeMode::Light, true),
        ],
        &SharedString::default(),
    );

    assert_eq!(delegate.sections.len(), 2);
    assert!(delegate.sections[0].mode.is_dark());

    let names = |section: usize| -> Vec<String> {
        delegate.sections[section]
            .items
            .iter()
            .map(|option| option.name.to_string())
            .collect()
    };
    assert_eq!(names(0), ["Default Dark", "Alpha Dark", "Zeta Dark"]);
    assert_eq!(names(1), ["Default Light", "Light One"]);
}

/// 搜索按名字过滤，空节整节撤掉 —— 不然剩下的"深色"头下面一行都没有。
#[test]
fn search_narrows_the_list_and_drops_empty_sections() {
    let mut delegate = ThemePickerDelegate::new(
        vec![
            option("Catppuccin Mocha", ThemeMode::Dark, false),
            option("Catppuccin Latte", ThemeMode::Light, false),
            option("Gruvbox Dark", ThemeMode::Dark, false),
        ],
        &SharedString::default(),
    );

    delegate.rebuild("catppuccin");
    assert_eq!(delegate.sections.len(), 2, "明暗各剩一套");
    assert_eq!(delegate.sections[0].items.len(), 1);
    assert_eq!(delegate.sections[1].items.len(), 1);

    // 大小写不算数：用户不会照着清单抄。
    delegate.rebuild("GRUVBOX");
    assert_eq!(delegate.sections.len(), 1, "浅色那一节整个撤掉");
    assert_eq!(
        delegate.name_at(IndexPath::default()),
        Some(SharedString::from("Gruvbox Dark"))
    );

    delegate.rebuild("查无此主题");
    assert!(delegate.sections.is_empty());
}

/// 起笔落在当前那一套上；搜过之后行号变了也还找得到。
#[test]
fn position_finds_the_row_of_the_current_theme() {
    let delegate = ThemePickerDelegate::new(
        vec![
            option("Alpha Dark", ThemeMode::Dark, false),
            option("Beta Dark", ThemeMode::Dark, false),
            option("Light One", ThemeMode::Light, false),
        ],
        &SharedString::from("Light One"),
    );

    let ix = delegate
        .position(&SharedString::from("Light One"))
        .expect("在表里");
    assert_eq!((ix.section, ix.row), (1, 0));
    assert_eq!(
        delegate.position(&SharedString::from("查无此主题")),
        None,
        "认不出来的名字不该落在一行上"
    );
}

/// "恢复默认"清空的是勾，不是别的行：空串不匹配任何主题名。
#[test]
fn no_row_is_checked_after_resetting() {
    let delegate = ThemePickerDelegate::new(
        vec![option("Alpha Dark", ThemeMode::Dark, false)],
        &SharedString::default(),
    );

    assert_eq!(delegate.selected, SharedString::default());
    assert!(delegate.position(&SharedString::default()).is_none());
}

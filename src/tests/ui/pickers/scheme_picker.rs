use super::*;

/// 全部内置方案都在选择器里、明暗各归各组、跟随档独占开头没有节头。
/// 少一套的话它就永远挑不上 —— 那种缺失从"页面画出来了"上是看不出来
/// 的（这条测的正是设置页换掉的那个老断言守的事）。
#[test]
fn every_scheme_is_listed_and_grouped() {
    let delegate = SchemePickerDelegate::new("跟随全局");

    // 全量时节头依次是：跟随（无头）、深色、浅色。
    assert_eq!(delegate.sections.len(), 3, "全量下就是三节");
    assert_eq!(delegate.sections[0].header, None, "跟随档不画节头");
    assert_eq!(delegate.sections[0].items.len(), 1);
    assert_eq!(
        delegate.sections[0].items[0].value(),
        &SharedString::default(),
        "跟随档的值是空串 id"
    );
    // 节头文案走 i18n，测试环境未必加载了词表，这里只钉"有头"。
    assert!(
        delegate.sections[1].header.is_some() && delegate.sections[2].header.is_some(),
        "深浅两组各有节头"
    );

    // 每套方案都出现且只出现一次，组别与 is_light 一致。
    let mut seen = Vec::new();
    for (section, expect_light) in [(1, false), (2, true)] {
        for option in &delegate.sections[section].items {
            let scheme = option.scheme.expect("跟随档只在第 0 节");
            assert_eq!(
                scheme::is_light(scheme),
                expect_light,
                "{} 分错了组",
                scheme.id
            );
            seen.push(scheme.id);
        }
    }
    assert_eq!(seen.len(), scheme::all().len(), "少方案了");
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), scheme::all().len(), "有方案进了两次");
}

/// 搜索按名字或 id 过滤，空节整节撤掉，跟随档跟它的文案走。
#[test]
fn search_narrows_to_matching_schemes() {
    let mut delegate = SchemePickerDelegate::new("跟随全局");

    delegate.rebuild("gruvbox");
    let matched: Vec<&str> = delegate
        .sections
        .iter()
        .flat_map(|section| &section.items)
        .map(|option| option.scheme.unwrap().id)
        .collect();
    assert!(matched.contains(&"gruvbox_dark"), "{matched:?}");
    // 按名字搜：id 里没有空格，"Catppuccin Mocha" 靠名字命中。
    assert!(
        matched.iter().all(|id| id.contains("gruvbox")),
        "{matched:?}"
    );
    assert!(
        delegate
            .sections
            .iter()
            .all(|section| !section.items.is_empty()),
        "空节该整节撤掉"
    );

    // id 拼法也搜得到 —— 从 alacritty 过来的人认得的是下划线那一串。
    delegate.rebuild("catppuccin_mocha");
    assert!(
        delegate
            .sections
            .iter()
            .any(|section| section.items.iter().any(|option| option
                .scheme
                .is_some_and(|scheme| scheme.id == "catppuccin_mocha")))
    );

    // 搜"跟随"两个字只剩跟随档。
    delegate.rebuild("跟随");
    assert_eq!(delegate.sections.len(), 1);
    assert!(delegate.sections[0].items[0].scheme.is_none());

    // 一个都不剩时不该有空节，也不该 panic。
    delegate.rebuild("zzzz");
    assert!(delegate.sections.is_empty());
}

/// 节头文案：深浅两组各有其名；跟随档没有文案，也得回一个空格占位 ——
/// 组件库量节头高度只量第 0 节，回 `None` 量出来就是 0 高，“深色”那行
/// 节头会整个叠到第一套方案上（线上见过的那处重叠）。
#[test]
fn every_section_yields_a_header_label() {
    let delegate = SchemePickerDelegate::new("跟随全局");

    let follow = delegate.header_label(0).expect("跟随档也要有节头占位");
    assert_eq!(
        follow,
        SharedString::from(" "),
        "占位只是撑出行高，不该有字"
    );

    // 节头文案走 i18n，测试环境未必加载了词表，只钉“深浅两节各有名、且不相同”。
    let dark = delegate.header_label(1).expect("深色组有节头");
    let light = delegate.header_label(2).expect("浅色组有节头");
    assert_ne!(dark, light, "深浅两节的节头不该是同一个词");
}

/// position 从值反查行：跟随档与具体方案都能定位，认不出的是 `None`
/// —— 工厂拿它决定起笔在哪一行。
#[test]
fn position_resolves_the_follow_entry_and_scheme_ids() {
    let delegate = SchemePickerDelegate::new("跟随全局");

    let follow = delegate
        .position(&SharedString::default())
        .expect("空串定位到跟随档");
    assert_eq!((follow.section, follow.row), (0, 0));

    let dracula = delegate
        .position(&SharedString::from("dracula"))
        .expect("方案定位得到");
    assert!(dracula.section >= 1);

    assert!(delegate.position(&SharedString::from("查无此色")).is_none());
}

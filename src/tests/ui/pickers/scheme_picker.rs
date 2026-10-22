use super::*;

/// 全部内置方案都在选择器里、明暗各归各组、跟随档独占开头没有节头。
/// 少一套的话它就永远挑不上 —— 那种缺失从"页面画出来了"上是看不出来
/// 的（这条测的正是设置页换掉的那个老断言守的事）。
#[test]
fn every_scheme_is_listed_and_grouped() {
    let delegate = SchemePickerDelegate::new("跟随全局".into(), "");

    // 全量时节头依次是：跟随（无头）、深色、浅色。
    assert_eq!(delegate.sections.len(), 3, "全量下就是三节");
    assert_eq!(delegate.sections[0].header, None, "跟随档不画节头");
    assert_eq!(delegate.sections[0].items.len(), 1);
    assert_eq!(
        delegate.sections[0].items[0].id(),
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
            let scheme = scheme::find(option.id()).expect("跟随档只在第 0 节");
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
    let mut delegate = SchemePickerDelegate::new("跟随全局".into(), "");

    delegate.rebuild("gruvbox");
    let matched: Vec<&str> = delegate
        .sections
        .iter()
        .flat_map(|section| &section.items)
        .map(|option| option.id().as_ref())
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
            .any(|section| section
                .items
                .iter()
                .any(|option| option.id().as_ref() == "catppuccin_mocha"))
    );

    // 搜"跟随"两个字只剩跟随档。
    delegate.rebuild("跟随");
    assert_eq!(delegate.sections.len(), 1);
    assert!(delegate.sections[0].items[0].id().is_empty());

    // 一个都不剩时不该有空节，也不该 panic。
    delegate.rebuild("zzzz");
    assert!(delegate.sections.is_empty());
}

/// 节头文案：深浅两组各有其名；跟随档没有文案，也得回一个空格占位 ——
/// 组件库量节头高度只量第 0 节，回 `None` 量出来就是 0 高，“深色”那行
/// 节头会整个叠到第一套方案上（线上见过的那处重叠）。
#[test]
fn every_section_yields_a_header_label() {
    let delegate = SchemePickerDelegate::new("跟随全局".into(), "");

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
/// —— 打开选择器时拿它决定起笔在哪一行。
#[test]
fn position_resolves_the_follow_entry_and_scheme_ids() {
    let delegate = SchemePickerDelegate::new("跟随全局".into(), "");

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

/// 打开时带来的 id 决定左栏哪一行打勾：光标走上下键、点一下，都靠这一行
/// 认出来的 id 去换右栏的预览，认错了就是"预览的和你选的不是同一套"。
#[test]
fn the_selected_id_resolves_back_to_its_row() {
    let mut delegate = SchemePickerDelegate::new("跟随全局".into(), "dracula");

    let dracula = delegate
        .position(&SharedString::from("dracula"))
        .expect("定位得到");
    let option = delegate.option_at(dracula).expect("那一行还在");
    assert_eq!(option.id(), &SharedString::from("dracula"));
    assert!(!option.id().is_empty(), "跟随档之外的行都有 id");

    // 搜索之后行的位置会变，跟着值走的那条链得跟着一起变。
    delegate.rebuild("dracula");
    let narrowed = delegate
        .position(&SharedString::from("dracula"))
        .expect("搜完还定位得到");
    assert_eq!(
        delegate.id_at(narrowed),
        Some(SharedString::from("dracula")),
        "搜索结果里那一行还是它"
    );

    // 认不出的 id：既不该 panic，也不该指到别的行上。
    assert!(delegate.id_at(IndexPath::default().section(0).row(0)).is_some(), "第 0 节第 0 行是跟随档");
    assert_eq!(
        delegate.id_at(IndexPath::default().section(9).row(9)),
        None,
        "越界的行没有 id"
    );
}
/// 选择器画得出来：弹层挂到了 `Root` 上，左栏那列与右栏那颗"确定"都上了屏。
///
/// 弹层里的东西画不出来，"点了没反应"和坏了长得一模一样 —— 这条守着整棵树
/// 从头画到尾不炸，也守住"起手选的那一套别被画一遍就改掉"（〔`scheme::global`〕
/// 是界面每帧读的那份缓存，误写进去等于一开选择器就把全局配色换了）。
#[gpui_kit::test]
fn the_picker_draws_the_list_and_the_preview(cx: &mut gpui_kit::TestAppContext) {
    let _guard = scheme::exclusive_for_test();
    scheme::set_global("gruvbox_dark");
    cx.update(gpui_kit::init);

    // 弹层归 `Root` 存，得有那一层才验得到（同设置窗那条弹窗用例）。底下
    // 那点内容是空的：这一刻验的是弹层自己，不是它盖住的东西。
    let window = cx.add_window(|window, cx| {
        let shell = cx.new(|_| Blank);
        gpui_kit::component::Root::new(shell, window, cx)
    });
    let mut visual = gpui_kit::VisualTestContext::from_window(window.into(), cx);

    visual.update(|window, cx| {
        SchemePicker::open(
            SchemePickerOptions {
                title: "配色方案".into(),
                follow_label: "跟随界面主题".into(),
                hint: "点一套即预览".into(),
                selected: scheme::global(),
                on_confirm: Rc::new(|_, _, _| {}),
            },
            window,
            cx,
        );
        assert!(window.has_active_dialog(cx), "选择器没挂到 Root 上");
    });

    // 多画几帧：弹层入场那点动画、虚拟列表的行高缓存都要靠后几帧才落定。
    for _ in 0..5 {
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }

    // 测试视口 1920×1080 下这一块是 478×617 上下（正文定高 648，扣掉底下
    // 那行说明）。这里不钉具体数，钉的是"它有实实在在的一块地方"：塌成 0 高
    // 的预览在界面上看着像"这块地方本来就是空的"，比量出来的小更难看穿。
    let preview = visual
        .debug_bounds(PREVIEW_PROBE)
        .expect("右栏那块预览压根没画出来");
    assert!(
        preview.size.width > px(300.) && preview.size.height > px(200.),
        "预览被压得不成样子（{:?}）—— 终端预览塌了",
        preview.size
    );
    assert_eq!(
        scheme::global(),
        "gruvbox_dark",
        "画一遍不该改掉选中的那一套"
    );
}

/// 弹层底下那点内容：这条用例只验选择器，空着就行。
struct Blank;

impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // 撑满：弹层是挂在内容那一层上的 `absolute` 覆盖层，底下塌成 0 高，
        // 弹窗跟着量成 0 —— 那样这条用例看着"画过了"，却什么都没验到。
        div().size_full()
    }
}

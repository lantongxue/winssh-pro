    use super::*;

    fn snippet(command: &str) -> Snippet {
        Snippet {
            id: "snip-1".into(),
            title: "跟一个容器的日志".into(),
            category: SnippetCategory::Docker,
            command: command.into(),
            description: "按容器名跟最近若干行".into(),
            tags: vec!["docker".into(), "日志".into()],
            variables: vec![SnippetVariable {
                name: "lines".into(),
                default: "100".into(),
            }],
        }
    }

    #[test]
    fn categories_round_trip_through_the_column() {
        for category in SnippetCategory::ALL {
            assert_eq!(
                SnippetCategory::from_str_or_default(category.as_str()),
                category
            );
            assert_eq!(SnippetCategory::from_index(category.index()), category);
        }
        // 认不出的与空串都落到缺省档，不是 panic。
        assert_eq!(
            SnippetCategory::from_str_or_default("说不清"),
            SnippetCategory::Docker
        );
        assert_eq!(
            SnippetCategory::from_str_or_default(""),
            SnippetCategory::Docker
        );
    }

    #[test]
    fn parameters_are_read_from_the_command_in_order() {
        assert_eq!(
            variables_in("docker logs -f --tail={{lines}} {{container}}"),
            vec![SharedString::from("lines"), SharedString::from("container")]
        );
        // 同一个参数出现两次只问一次。
        assert_eq!(
            variables_in("mysqldump {{db}} > {{db}}.sql"),
            vec![SharedString::from("db")]
        );
        assert!(variables_in("ss -tulpn | grep LISTEN").is_empty());
    }

    /// shell 自己的花括号不是参数 —— 认宽了的话，`find -exec ... {} +`
    /// 会变成一条"请填一个参数"的片段。
    #[test]
    fn shell_braces_are_not_parameters() {
        assert!(variables_in("find /var -exec du -Sh {} + | sort -rh").is_empty());
        assert!(variables_in("echo ${HOME} $(date +%F)").is_empty());
        assert!(variables_in("awk '{print $1}'").is_empty());
        // 名字里有空格、连字号、中文的都不算。
        assert!(variables_in("{{two words}} {{a-b}} {{参数}}").is_empty());
        // 没有闭合的那一对不算。
        assert!(variables_in("echo {{unclosed").is_empty());
    }

    #[test]
    fn expanding_puts_the_values_where_the_placeholders_were() {
        let logs = snippet("docker logs -f --tail={{lines}} {{container}}");
        let values = vec![
            (SharedString::from("lines"), "50".to_string()),
            (SharedString::from("container"), "nginx".to_string()),
        ];
        assert_eq!(
            logs.expand(&values),
            "docker logs -f --tail=50 nginx",
            "两个参数都该填进去"
        );

        // 同一个参数的每一处都填。
        let dump = snippet("mysqldump {{db}} > {{db}}.sql");
        let values = vec![(SharedString::from("db"), "shop".to_string())];
        assert_eq!(dump.expand(&values), "mysqldump shop > shop.sql");
    }

    /// 空值照填：预览和真正发出去的那一行必须是同一个字符串。
    #[test]
    fn an_empty_value_is_substituted_not_left_behind() {
        let snippet = snippet("tail -n {{lines}} /var/log/syslog");
        let values = vec![(SharedString::from("lines"), String::new())];
        assert_eq!(snippet.expand(&values), "tail -n  /var/log/syslog");
    }

    /// 没给值的参数原样留着，不被抹掉 —— 命令刚改过、参数表还没跟上时
    /// 抹掉会送出一条意思不同的命令。
    #[test]
    fn a_parameter_with_no_value_stays_as_it_was() {
        let snippet = snippet("systemctl restart {{service}}");
        assert_eq!(snippet.expand(&[]), "systemctl restart {{service}}");
    }

    /// 参数清单从命令文本里认，默认值从存下来的那一份取；命令改过之后，
    /// 库里留下的那几个过期名字不该再被问一次。
    #[test]
    fn parameters_follow_the_command_not_the_stored_list() {
        let mut snippet = snippet("docker logs --tail={{lines}} {{container}}");
        let parameters = snippet.parameters();
        assert_eq!(parameters.len(), 2);
        assert_eq!(parameters[0].name, "lines");
        assert_eq!(parameters[0].default, "100", "存下来的默认值要拿到");
        assert_eq!(parameters[1].default, "", "没存过默认值的是空串");

        // 命令改成不带参数的了，`variables` 里那条 `lines` 就不该再问。
        snippet.command = "docker ps -a".into();
        assert!(snippet.parameters().is_empty());
        assert!(!snippet.is_parameterized());
    }

    /// 卡片上那一行：单行命令原样，多行脚本只画第一行加一个省略号。
    #[test]
    fn the_preview_says_when_there_are_more_lines() {
        assert_eq!(snippet("docker ps -a").preview(), "docker ps -a");
        assert_eq!(
            snippet("set -e\ncd /srv/app\ngit pull").preview(),
            "set -e …"
        );
        assert_eq!(snippet("").preview(), "");
    }

    #[test]
    fn search_looks_at_the_title_command_description_and_tags() {
        let snippet = snippet("docker logs -f --tail={{lines}} {{container}}");
        assert!(snippet.matches(""));
        assert!(snippet.matches("日志"), "名称");
        assert!(snippet.matches("DOCKER"), "命令，且不分大小写");
        assert!(snippet.matches("容器名"), "说明");
        assert!(snippet.matches("docker"), "标签");
        assert!(!snippet.matches("kubectl"));
    }

    #[test]
    fn tags_and_variables_round_trip() {
        let tags: Vec<SharedString> = vec!["docker".into(), "生产, 核心".into()];
        assert_eq!(decode_tags(&encode_tags(&tags)), tags);
        assert!(encode_tags(&[]).is_empty());
        assert!(decode_tags("").is_empty());
        assert!(decode_tags("{ 这不是 json").is_empty());

        let variables = vec![SnippetVariable {
            name: "lines".into(),
            default: "100".into(),
        }];
        assert_eq!(decode_variables(&encode_variables(&variables)), variables);
        assert!(encode_variables(&[]).is_empty());
        assert!(decode_variables("").is_empty());
        assert!(decode_variables("[[[").is_empty());
    }

    /// 每份模板都要能直接存下去：名称与说明查得到文案，命令里的参数和
    /// 它声明的默认值对得上 —— 默认值写错一个名字，那一格就永远是空的。
    ///
    /// 名称与说明的 key 是拼出来的（`snippet.preset_<id>_title`），
    /// [`crate::i18n`] 那条扫源码的测试看不见它们，所以这里比一次"查出来的
    /// 不是 key 本身" —— 语言表里少一条时 `t!` 返回的就是 key。
    #[test]
    fn every_preset_is_ready_to_save() {
        for preset in PRESETS {
            let draft = preset.draft();
            assert!(!draft.command.is_empty(), "{} 没有命令", preset.id);
            assert!(!draft.tags.is_empty(), "{} 没有标签", preset.id);
            assert!(
                !draft.title.starts_with("snippet.preset_"),
                "{} 的名称在语言表里查不到",
                preset.id
            );
            assert!(
                !draft.description.starts_with("snippet.preset_"),
                "{} 的说明在语言表里查不到",
                preset.id
            );

            let names = variables_in(preset.command);
            for (name, _) in preset.defaults {
                assert!(
                    names.iter().any(|candidate| candidate == name),
                    "{} 的默认值 {name} 在命令里找不到对应的参数",
                    preset.id
                );
            }
        }
    }

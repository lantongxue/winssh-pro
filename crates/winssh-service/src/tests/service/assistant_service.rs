    use super::*;
    use crate::assistant::TransferDirection;
    use sea_orm::EntityTrait as _;

    /// 对话与两种消息的整条来回：建对话、塞消息、读回来一字不差、
    /// 删对话连消息一起没了。
    #[test]
    fn conversations_and_messages_round_trip() {
        db::runtime().block_on(async {
            let service = AssistantService::in_memory().await.unwrap();
            assert!(service.conversations_for_host("host-a").await.unwrap().is_empty());

            let chat = service
                .create_conversation("帮我看看 web-01 上 nginx 为什么起不来，然后修好它", "host-a", "", None)
                .await
                .unwrap();
            assert!(chat.title.starts_with("帮我看看 web-01"));
            assert_eq!(chat.host_id.as_ref(), "host-a");

            service
                .append_message(&chat.id, ChatRole::User, MessageBody::text("nginx 起不来"))
                .await
                .unwrap();
            service
                .append_message(
                    &chat.id,
                    ChatRole::Assistant,
                    MessageBody::Command {
                        command: "systemctl status nginx".into(),
                        session: "web-01".into(),
                        output: "inactive (dead)".into(),
                        status: CommandStatus::Failed,
                    },
                )
                .await
                .unwrap();

            let messages = service.messages(&chat.id).await.unwrap();
            assert_eq!(messages.len(), 2);
            assert_eq!(messages[0].role, ChatRole::User);
            assert_eq!(messages[0].body, MessageBody::text("nginx 起不来"));
            assert_eq!(
                messages[1].body,
                MessageBody::Command {
                    command: "systemctl status nginx".into(),
                    session: "web-01".into(),
                    output: "inactive (dead)".into(),
                    status: CommandStatus::Failed,
                }
            );

            service.delete_conversation(&chat.id).await.unwrap();
            assert!(service.conversations_for_host("host-a").await.unwrap().is_empty());
            assert!(
                service.messages(&chat.id).await.unwrap().is_empty(),
                "外键该把消息连带删光"
            );
        });
    }

    /// 传输卡片也要能原样存了再读回来：方向、两端路径、搬了多少、失败原因
    /// 各有各的落处（命令那几列 + content 里那串 JSON），错一处就是重开应用
    /// 之后卡片缺胳膊少腿。
    #[test]
    fn transfer_cards_round_trip() {
        db::runtime().block_on(async {
            let service = AssistantService::in_memory().await.unwrap();
            let chat = service.create_conversation("传一下日志", "host-a", "", None).await.unwrap();
            let card = TransferCard {
                direction: TransferDirection::Download,
                session: "web-01".into(),
                source: "/var/log/nginx/error.log".into(),
                destination: "/home/kali/Downloads/error.log".into(),
                status: CommandStatus::Failed,
                files: 3,
                bytes: 12_345,
                error: "No space left on device".into(),
            };
            service
                .append_message(
                    &chat.id,
                    ChatRole::Assistant,
                    MessageBody::Transfer(card.clone()),
                )
                .await
                .unwrap();
            let read = service.messages(&chat.id).await.unwrap();
            assert_eq!(read.last().unwrap().body, MessageBody::Transfer(card));
        });
    }

    /// 带图的提问：图的字节、格式、尺寸与先后一字不差地读回来，挂在原来那条
    /// 消息上，别的消息不沾；删对话时图跟着消息一起走（两层级联）。
    #[test]
    fn images_round_trip_with_their_message() {
        fn png(width: u32, height: u32) -> ChatImage {
            let mut bytes = Vec::new();
            image::DynamicImage::ImageRgb8(image::RgbImage::new(width, height))
                .write_to(
                    &mut std::io::Cursor::new(&mut bytes),
                    image::ImageFormat::Png,
                )
                .unwrap();
            ChatImage::prepare(&bytes).unwrap()
        }

        db::runtime().block_on(async {
            let service = AssistantService::in_memory().await.unwrap();
            let chat = service
                .create_conversation("这个报错是什么意思", "host-a", "", None)
                .await
                .unwrap();
            let shots = vec![png(40, 30), png(8, 8)];
            service
                .append_message(
                    &chat.id,
                    ChatRole::User,
                    MessageBody::Text {
                        text: "这个报错是什么意思".into(),
                        images: shots.clone(),
                        tokens_per_sec: None,
                        model: None,
                    },
                )
                .await
                .unwrap();
            service
                .append_message(
                    &chat.id,
                    ChatRole::Assistant,
                    MessageBody::text("是磁盘满了"),
                )
                .await
                .unwrap();

            let messages = service.messages(&chat.id).await.unwrap();
            let MessageBody::Text { text, images, .. } = &messages[0].body else {
                panic!("第一条该是文字消息");
            };
            assert_eq!(text.as_ref(), "这个报错是什么意思");
            assert_eq!(images, &shots, "图要按原样、原顺序读回来");
            assert_eq!((images[0].width(), images[0].height()), (40, 30));
            assert_eq!(
                messages[1].body,
                MessageBody::text("是磁盘满了"),
                "别的消息不沾图"
            );

            service.delete_conversation(&chat.id).await.unwrap();
            let left = ai_image::Entity::find().all(&service.db).await.unwrap();
            assert!(
                left.is_empty(),
                "删对话该把图连带删光：还剩 {} 张",
                left.len()
            );
        });
    }

    /// 模型的思考与带速度的回复：思考按自己那一档读回来，排在回复前面；
    /// 回复的 tokens/s 原样读回，用户的话没有这回事。
    #[test]
    fn reasoning_and_reply_speed_round_trip() {
        db::runtime().block_on(async {
            let service = AssistantService::in_memory().await.unwrap();
            let chat = service.create_conversation("磁盘满了吗", "host-a", "", None).await.unwrap();
            for (role, body) in [
                (ChatRole::User, MessageBody::text("磁盘满了吗")),
                (
                    ChatRole::Assistant,
                    MessageBody::Reasoning {
                        text: "先看 df，再看 inode".into(),
                    },
                ),
                (
                    ChatRole::Assistant,
                    MessageBody::reply("根分区还有 28G。", Some(42.5)),
                ),
            ] {
                service.append_message(&chat.id, role, body).await.unwrap();
            }

            let bodies: Vec<_> = service
                .messages(&chat.id)
                .await
                .unwrap()
                .into_iter()
                .map(|message| message.body)
                .collect();
            assert_eq!(
                bodies,
                [
                    MessageBody::text("磁盘满了吗"),
                    MessageBody::Reasoning {
                        text: "先看 df，再看 inode".into(),
                    },
                    MessageBody::reply("根分区还有 28G。", Some(42.5)),
                ]
            );
        });
    }

    /// 标题：取第一行非空、超长截断、全空白给兜底名。
    #[test]
    fn titles_come_from_the_first_line() {
        assert_eq!(conversation_title("\n  查看磁盘  \n剩下的"), "查看磁盘");
        let long = "长".repeat(60);
        let title = conversation_title(&long);
        assert_eq!(title.chars().count(), TITLE_CHARS + 1, "截断后缀省略号");
        assert!(!conversation_title("   ").is_empty(), "空提问有兜底名");
    }

    /// 对话落库时带上它绑定的主机 id，读回来也在。
    #[test]
    fn create_conversation_tags_host_id() {
        db::runtime().block_on(async {
            let service = AssistantService::in_memory().await.unwrap();
            let chat = service
                .create_conversation("看看这台怎么样", "host-42", "", None)
                .await
                .unwrap();
            assert_eq!(chat.host_id.as_ref(), "host-42");
            let listed = service.conversations_for_host("host-42").await.unwrap();
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0].host_id.as_ref(), "host-42");
        });
    }

    /// 切换主机标签时只加载当前主机的对话：建 A、B 两段后按主机各得其一；
    /// 默认空串（升级前的旧对话）不归属任何主机、不出现在任一查询里。
    #[test]
    fn conversations_for_host_only_returns_that_host() {
        db::runtime().block_on(async {
            let service = AssistantService::in_memory().await.unwrap();
            service
                .create_conversation("A 上的事", "host-a", "", None)
                .await
                .unwrap();
            service
                .create_conversation("B 上的事", "host-b", "", None)
                .await
                .unwrap();
            // 直接塞一行空 host_id，模拟升级前没有归属的旧对话。
            use sea_orm::ActiveValue::Set;
            service
                .repo
                .insert_conversation(
                    &service.db,
                    ai_conversation::ActiveModel {
                        id: Set("chat-legacy".into()),
                        title: Set("旧对话".into()),
                        host_id: Set(String::new()),
                        model_id: Set(String::new()),
                        thinking_effort: Set(String::new()),
                        created_at: Set("2020-01-01 00:00:00".into()),
                        updated_at: Set("2020-01-01 00:00:00".into()),
                    },
                )
                .await
                .unwrap();

            let a = service.conversations_for_host("host-a").await.unwrap();
            assert_eq!(a.len(), 1, "A 只该看到自己那一段，不串 B、也不串旧对话");
            assert_eq!(a[0].title.as_ref(), "A 上的事");
            let b = service.conversations_for_host("host-b").await.unwrap();
            assert_eq!(b.len(), 1, "B 同样只该看到自己那一段");
            assert_eq!(b[0].title.as_ref(), "B 上的事");
        });
    }
    /// 对话记着自己的模型与思考强度：建档时带上的读得回来，换过之后读回来
    /// 是新的；"最近动过"不跟着换模型跳。
    #[test]
    fn conversations_remember_their_model_and_effort() {
        db::runtime().block_on(async {
            let service = AssistantService::in_memory().await.unwrap();
            let chat = service
                .create_conversation("看看负载", "host-a", "model-a", Some(ThinkingEffort::High))
                .await
                .unwrap();
            assert_eq!(chat.model_id.as_ref(), "model-a");
            assert_eq!(chat.effort, Some(ThinkingEffort::High));

            service
                .set_conversation_model(&chat.id, "model-b", Some(ThinkingEffort::None))
                .await
                .unwrap();
            let listed = service.conversations_for_host("host-a").await.unwrap();
            assert_eq!(listed[0].model_id.as_ref(), "model-b");
            assert_eq!(listed[0].effort, Some(ThinkingEffort::None));

            service
                .set_conversation_model(&chat.id, "model-c", None)
                .await
                .unwrap();
            let listed = service.conversations_for_host("host-a").await.unwrap();
            assert_eq!(listed[0].effort, None, "清掉的强度读回来是没挑过");

            assert!(
                service
                    .set_conversation_model("chat-gone", "model-a", None)
                    .await
                    .is_err(),
                "对话已经删了要说一声"
            );
        });
    }

    /// 回复记着是哪个模型说的，读回来还在；没记的读回来是 `None`。
    #[test]
    fn reply_model_round_trips() {
        db::runtime().block_on(async {
            let service = AssistantService::in_memory().await.unwrap();
            let chat = service
                .create_conversation("磁盘", "host-a", "", None)
                .await
                .unwrap();
            let reply = MessageBody::reply("还有 28G。", Some(30.0)).with_model("glm-4.5");
            service
                .append_message(&chat.id, ChatRole::Assistant, reply.clone())
                .await
                .unwrap();
            service
                .append_message(&chat.id, ChatRole::User, MessageBody::text("好"))
                .await
                .unwrap();
            let bodies: Vec<_> = service
                .messages(&chat.id)
                .await
                .unwrap()
                .into_iter()
                .map(|message| message.body)
                .collect();
            assert_eq!(bodies, [reply, MessageBody::text("好")]);
        });
    }

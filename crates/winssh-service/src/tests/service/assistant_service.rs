    use super::*;
    use crate::assistant::TransferDirection;
    use sea_orm::EntityTrait as _;

    /// 对话与两种消息的整条来回：建对话、塞消息、读回来一字不差、
    /// 删对话连消息一起没了。
    #[test]
    fn conversations_and_messages_round_trip() {
        db::runtime().block_on(async {
            let service = AssistantService::in_memory().await.unwrap();
            assert!(service.conversations().await.unwrap().is_empty());

            let chat = service
                .create_conversation("帮我看看 web-01 上 nginx 为什么起不来，然后修好它")
                .await
                .unwrap();
            assert!(chat.title.starts_with("帮我看看 web-01"));

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
            assert!(service.conversations().await.unwrap().is_empty());
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
            let chat = service.create_conversation("传一下日志").await.unwrap();
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
                .create_conversation("这个报错是什么意思")
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
            let chat = service.create_conversation("磁盘满了吗").await.unwrap();
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
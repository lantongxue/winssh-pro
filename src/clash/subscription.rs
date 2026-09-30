//! 一条订阅：名字、来路，以及**整份正文**。
//!
//! 正文是这份东西的主体。它不只是一个"从哪儿拉"的地址 —— 地址会失效，机场会
//! 换域名，手里那份正文却是随时能用的。所以库里存的是内容本身，和主机存私钥
//! 整份内容（见 [`crate::host::Host::key_content`]）是同一个道理：**这条记录
//! 自己是完整的**。备份同步过去，换台机器恢复出来就是同一份节点。
//!
//! 拉取（[`fetch`]）只负责"把地址变成正文"，落库归服务层。

use anyhow::{Context as _, Result, anyhow, bail};
use gpui_kit::SharedString;
use rust_i18n::t;

use super::config;

/// 一条保存下来的订阅。
#[derive(Clone, Debug, PartialEq)]
pub struct ClashSubscription {
    pub id: SharedString,
    /// 显示名，例如 `机场 A`。
    pub name: SharedString,
    /// 订阅地址。手工粘贴或从文件导入的订阅没有地址，是空串。
    pub source_url: SharedString,
    /// 订阅正文（Clash 配置 YAML）。
    pub config_yaml: SharedString,
    /// 分组标签，筛选用。空串表示未归类。
    pub tag: SharedString,
    /// 卡片标识色（十六进制 `#RRGGBB`）。空串走默认的青色。
    pub color: SharedString,
    /// 套餐累计上传字节。`None` 是机场没回这个数（不要将 `0` 写成
    /// "用光了"：那一列的读法是"还没读到"而不是"没用过"）。
    pub upload_bytes: Option<u64>,
    /// 套餐累计下行字节。
    pub download_bytes: Option<u64>,
    /// 套餐总字节。
    pub total_bytes: Option<u64>,
    /// 套餐到期日。机场回的多半是 `YYYY-MM-DD` 这种样子，原样存；`None`
    /// 是机场没回。
    pub expire_date: Option<SharedString>,
    /// 拉取时向机场报的 User-Agent。空串走协议层默认的那一份。
    pub user_agent: SharedString,
    /// 自动更新频率（小时）。0 是手动更新。
    pub auto_update_hours: u32,
    /// 用户备注。
    pub notes: SharedString,
    pub created_at: SharedString,
    /// 正文最后一次更新的时间。
    pub updated_at: SharedString,
}

/// 机场响应头 `subscription-userinfo` 里那几个数。字段都是可选的，全都没有时
/// 是 [`None`] —— 那次拉取就没回这个头，或者头上没一个能认的字段。
///
/// 见 [`SubscriptionMeta::from_headers`]。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SubscriptionMeta {
    /// 累计上传字节。
    pub upload: Option<u64>,
    /// 累计下行字节。
    pub download: Option<u64>,
    /// 套餐总字节。
    pub total: Option<u64>,
    /// 到期时间（机场多半是 Unix epoch 秒数）。原样存原始值 —— 显示处转格式。
    pub expire: Option<u64>,
}

impl SubscriptionMeta {
    /// 从响应头里读 `subscription-userinfo`，能有几个是几个：
    /// `upload=1; download=2; total=3; expire=1700000000`。
    ///
    /// 机场给的头常有几种认不全的写法（小写拼错、漏大写、多一个结尾分号），
    /// 这里只一对一地抠数字，不试图读懂那一段的全部意思 —— 那没有意义。
    pub fn from_headers(headers: &reqwest::header::HeaderMap) -> Option<Self> {
        let raw = headers
            .get("subscription-userinfo")?
            .to_str()
            .ok()?;
        let mut meta = Self::default();
        for pair in raw.split(';') {
            let pair = pair.trim();
            let Some((key, value)) = pair.split_once('=') else {
                continue;
            };
            let digits: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                continue;
            }
            let Ok(value) = digits.parse::<u64>() else {
                continue;
            };
            match key.trim().to_ascii_lowercase().as_str() {
                "upload" => meta.upload = Some(value),
                "download" => meta.download = Some(value),
                "total" => meta.total = Some(value),
                "expire" => meta.expire = Some(value),
                // 机场自家造的字段（`website`、`server`…），不认识。
                _ => {}
            }
        }
        match meta == Self::default() {
            true => None,
            false => Some(meta),
        }
    }
}

impl ClashSubscription {
    /// 正文解读出来的东西，读不动就是 [`None`]（详情页那时候只说"读不出来"）。
    pub fn outline(&self) -> Option<config::SubscriptionOutline> {
        config::outline(&self.config_yaml).ok()
    }

    /// 有没有地址可以拉。
    pub fn has_source(&self) -> bool {
        !self.source_url.trim().is_empty()
    }
}

/// 表单里正在编辑的一份订阅。还没有 id，也还没决定是新建还是更新。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClashSubscriptionDraft {
    pub name: String,
    /// 订阅地址，可以为空（手工粘贴 / 从文件导入的没有地址）。
    pub source_url: String,
    /// 表单里那一份正文。新建时可能是刚拉下来的，也可能是粘贴进去的。
    pub config_yaml: String,
    /// 分组标签（筛选用）。空串表示未归类。
    pub tag: String,
    /// 卡片标识色。空串走默认青色。
    pub color: String,
    /// 拉取时向机场报的 User-Agent。空串走协议层默认。
    pub user_agent: String,
    /// 自动更新频率（小时）。0 是手动更新。
    pub auto_update_hours: u32,
    /// 用户备注。
    pub notes: String,
}

impl ClashSubscriptionDraft {
    /// 校验。返回的是一句能直接摆给用户看的话，和主机表单那边一样。
    ///
    /// 正文可以**暂时**是空的：新建时用户先填地址、点"拉取"，正文才下来。
    /// 所以这里只校验"名字要有"与"总得有一样"；正文本身的可用性由
    /// [`Self::check_config`] 单独管 —— 拉回来的每一份都要过它。
    pub fn check(&self) -> Result<(), SharedString> {
        if self.name.trim().is_empty() {
            return Err(t!("clash.form.name_required").into());
        }
        if self.source_url.trim().is_empty() && self.config_yaml.trim().is_empty() {
            return Err(t!("clash.form.source_required").into());
        }
        Ok(())
    }

    /// 正文是不是一份能用的 Clash 配置。表单在"保存"之前过一遍。
    pub fn check_config(&self) -> Result<(), SharedString> {
        if self.config_yaml.trim().is_empty() {
            return Err(t!("clash.form.config_required").into());
        }
        config::check(&self.config_yaml)
    }

    /// 正式落库前的那一份：正文必须已经在了。
    pub fn into_stored(self) -> Result<StoredSubscription, SharedString> {
        self.check()?;
        self.check_config()?;
        Ok(StoredSubscription {
            name: self.name.trim().to_owned(),
            source_url: self.source_url.trim().to_owned(),
            config_yaml: self.config_yaml,
            tag: self.tag.trim().to_owned(),
            color: self.color.trim().to_owned(),
            user_agent: self.user_agent.trim().to_owned(),
            auto_update_hours: self.auto_update_hours,
            notes: self.notes.trim().to_owned(),
        })
    }
}

/// 校验过、可以往库里写的一份订阅。
#[derive(Clone, Debug, PartialEq)]
pub struct StoredSubscription {
    pub name: String,
    pub source_url: String,
    pub config_yaml: String,
    /// 用户挑的分组标签。
    pub tag: String,
    /// 卡片标识色。
    pub color: String,
    /// 拉取时向机场报的 User-Agent。
    pub user_agent: String,
    /// 自动更新频率（小时）。0 是手动更新。
    pub auto_update_hours: u32,
    /// 用户备注。
    pub notes: String,
}

/// 拉一份订阅正文带回来的两样：那份 YAML，以及机场在响应头里报的套餐信息。
#[derive(Clone, Debug, PartialEq)]
pub struct FetchedBody {
    /// 订阅正文（Clash 配置 YAML）。用的是它，不是机场给的 header。
    pub body: String,
    /// 机场在 `subscription-userinfo` 里报的那几样（上传/总流量/到期）。
    /// 那不是正文的一部分，但机场就是用它说"你还剩多少流量"的。
    pub meta: Option<SubscriptionMeta>,
}

/// 拉一份订阅正文。
///
/// 这件事**自己保证**跑在 tokio 上，调用方站在哪儿都行：网络那一段经
/// [`crate::db::run`] 搭数据层那班车（同 [`crate::service::LocalFileService`]）。
/// 这不是可有可无的讲究 —— 界面跑在 gpui 自己的 executor 上，那里没有 tokio
/// 的 reactor，而 reqwest 的连接池、超时定时器与 DNS 解析都要它。就地发送的
/// 结果就是那句 "there is no reactor running, must be called from the context
/// of a Tokio 1.x runtime"，进程当场垮掉。表单上的"拉取"与保存前那次补拉都是
/// 从那儿发起的 —— 这班车得由这里搭，而不是由每个调用方记住去搭。
///
/// 机场面板按 UA 决定回哪一份东西 —— 认得出 Clash 就给 YAML 配置，认不出就
/// 丢一份 base64 的节点列表过来。有的机场要认特定客户端的名字才肯回带
/// `subscription-userinfo` 的响应，所以 `user_agent` 让调用方自己带一档
/// （表单上那一格）；空串走默认。
pub async fn fetch_with_ua(url: &str, user_agent: &str) -> Result<FetchedBody> {
    let url = url.trim();
    if url.is_empty() {
        bail!(t!("clash.fetch.no_url"));
    }
    // `file://` 之类不认：这一档要的就是一份"订阅地址"，本机文件走表单里的
    // "从文件导入"。先自己挡一道，省得 reqwest 报一句 builder 的英文。
    if !url.starts_with("http://") && !url.starts_with("https://") {
        bail!(t!("clash.fetch.not_http"));
    }

    let user_agent = match user_agent.trim().is_empty() {
        true => USER_AGENT.to_owned(),
        false => user_agent.trim().to_owned(),
    };
    // 校验在本地做完，只有真要发请求的这一段上 runtime。
    let url = url.to_owned();
    crate::db::run(async move { fetch_over_http(&url, &user_agent).await }).await
}

/// [`fetch_with_ua`] 的网络那半段：请求、看状态码、读正文。**必须在 tokio 上**，
/// 界面那边由 [`fetch`] 搭好车再进来。
///
/// `User-Agent` 是这里唯一一处讲究：机场的面板按它决定回哪一份东西 ——
/// 认得出 Clash 就给 YAML 配置，认不出就丢一份 base64 的节点列表过来。所以
/// 默认报的是 Clash 系客户端的名字，不是我们自己的。真拿到节点列表也不至于
/// 一头雾水：[`super::config::check`] 会说清楚该去换哪种链接。
async fn fetch_over_http(url: &str, user_agent: &str) -> Result<FetchedBody> {
    let response = super::api::http()
        .get(url)
        .header("User-Agent", user_agent)
        .header("Accept", "application/yaml, text/yaml, text/plain, */*")
        .send()
        .await
        .with_context(|| t!("clash.fetch.failed", url => url).to_string())?;

    // `subscription-userinfo` 只跟着**这一次**的响应，得在读正文之前抠出来
    // —— `read_capped` 只把响应体收敛成 Vec，头信息过了那一步就没了。
    let meta = SubscriptionMeta::from_headers(response.headers());

    let status = response.status();
    if !status.is_success() {
        bail!(t!("clash.fetch.status", url => url, status => status.as_u16()));
    }

    let body = read_capped(response).await?;
    let text = String::from_utf8(body)
        .map_err(|_| anyhow!("{}", t!("clash.fetch.not_utf8", url => url)))?;
    if text.trim().is_empty() {
        bail!(t!("clash.fetch.empty", url => url));
    }
    Ok(FetchedBody { body: text, meta })
}

/// 机场按 UA 决定给 YAML 还是给节点列表，认的是 `clash` 这个词。
const USER_AGENT: &str = "clash-verge/v1.6.0";

/// 正文最多读这么大。一份订阅撑死几百 KB，8 MB 是"对面不太对劲"的那条线 ——
/// 不设上限的话，一个跑偏的地址能把内存吃干。
const MAX_BODY: usize = 8 * 1024 * 1024;

/// 读正文，边读边数，超了就当场停，别把整包都收进来才发现太大。
async fn read_capped(response: reqwest::Response) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .with_context(|| t!("clash.fetch.interrupted").to_string())?
    {
        if body.len() + chunk.len() > MAX_BODY {
            bail!(t!("clash.fetch.too_large", mb => MAX_BODY / (1024 * 1024)));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一份能用的最小配置。
    const USABLE: &str = "proxies:\n  - name: A\n    type: ss\n    server: s.example.com\n    port: 443\n";

    #[test]
    fn a_draft_needs_a_name_and_something_to_go_on() {
        let mut draft = ClashSubscriptionDraft::default();
        assert!(draft.check().is_err(), "什么都没有");

        draft.name = "机场 A".to_owned();
        assert!(draft.check().is_err(), "有名字，但既没地址也没正文");

        // 只有地址也行：正文还没拉下来，表单里点"拉取"就有了。
        draft.source_url = "https://example.com/sub".to_owned();
        assert!(draft.check().is_ok());

        // 正文自己那一关是另一回事。
        assert!(draft.check_config().is_err());
        draft.config_yaml = USABLE.to_owned();
        assert!(draft.check_config().is_ok());
    }

    /// 落库前正文必须真的在，而且真的是一份配置。
    #[test]
    fn storing_requires_a_usable_body() {
        let draft = ClashSubscriptionDraft {
            name: "  机场 A  ".to_owned(),
            source_url: " https://example.com/sub ".to_owned(),
            config_yaml: USABLE.to_owned(),
            tag: String::new(),
            color: String::new(),
            user_agent: String::new(),
            auto_update_hours: 0,
            notes: String::new(),
        };
        let stored = draft.into_stored().expect("该过");
        assert_eq!(stored.name, "机场 A", "两头空白要去掉");
        assert_eq!(stored.source_url, "https://example.com/sub");

        let bad = |body: &str| ClashSubscriptionDraft {
            name: "机场 B".to_owned(),
            config_yaml: body.to_owned(),
            ..ClashSubscriptionDraft::default()
        };
        assert!(bad("not: a: config").into_stored().is_err(), "不是配置");
        assert!(bad("   ").into_stored().is_err(), "正文空着不算数");
    }

    /// 正文读得动就读，读不动就是 `None` —— 详情页那时候只说"读不出来"，
    /// 不该让整条订阅打不开。
    #[test]
    fn an_outline_is_optional() {
        let subscription = ClashSubscription {
            id: "a".into(),
            name: "机场 A".into(),
            source_url: "".into(),
            config_yaml: USABLE.into(),
            tag: "".into(),
            color: "".into(),
            user_agent: "".into(),
            auto_update_hours: 0,
            notes: "".into(),
            upload_bytes: None,
            download_bytes: None,
            total_bytes: None,
            expire_date: None,
            created_at: "".into(),
            updated_at: "".into(),
        };
        assert_eq!(subscription.outline().unwrap().nodes.len(), 1);
        assert!(!subscription.has_source());

        let broken = ClashSubscription {
            config_yaml: "!!!".into(),
            ..subscription
        };
        assert!(broken.outline().is_none());
    }

    /// 从响应头里读 `subscription-userinfo`。机场给的格式有松有紧，
    /// 我们的要求是：能抠出一个数是一个。
    #[test]
    fn the_meta_is_parsed_out_of_the_headers() {
        use reqwest::header::{HeaderMap, HeaderValue};
        let headers = |value: &str| {
            let mut map = HeaderMap::new();
            map.insert("subscription-userinfo", HeaderValue::from_str(value).unwrap());
            map
        };

        // 标准写法：全部四样都有。
        let meta = SubscriptionMeta::from_headers(&headers(
            "upload=123456789; download=987654321; total=1099511627776; expire=1700000000",
        ))
        .expect("该读出一份套餐信息");
        assert_eq!(meta.upload, Some(123456789));
        assert_eq!(meta.download, Some(987654321));
        assert_eq!(meta.total, Some(1099511627776));
        assert_eq!(meta.expire, Some(1700000000));

        // 缺一样不挡，照样把认得出的抠出来。
        let meta = SubscriptionMeta::from_headers(&headers("upload=42; total=100"))
            .expect("缺 download / expire 不要紧");
        assert_eq!(meta.upload, Some(42));
        assert_eq!(meta.total, Some(100));
        assert_eq!(meta.download, None);
        assert_eq!(meta.expire, None);

        // 尾巴多分号、空格、大小写（`Upload=` 这种机场自家拼法）都要认。
        let headers = |value: &str| {
            let mut map = HeaderMap::new();
            map.insert("subscription-userinfo", HeaderValue::from_str(value).unwrap());
            map
        };
        let loose = SubscriptionMeta::from_headers(&headers("Upload= 500;DOWNLOAD= 1000 ;"))
            .expect("大小写混着也该认");
        assert_eq!(loose.upload, Some(500));
        assert_eq!(loose.download, Some(1000));

        // 一个能认的都没有：整份响应当没这个头。
        assert!(SubscriptionMeta::from_headers(&HeaderMap::new()).is_none());
        assert!(SubscriptionMeta::from_headers(&headers("nothing here")).is_none());
    }

    /// "拉取"是从界面上发起的，而界面跑在 gpui 自己的 executor 上 —— 那里没有
    /// tokio 的 reactor，reqwest 就地发送就是一句 "there is no reactor running"。
    ///
    /// 这条用例从一个**没有 reactor** 的线程上发起（同界面那条路）：修好之前
    /// 它 panic，修好之后拿到的是"连不上"这句平常的错误。地址挑 `127.0.0.1:1`
    /// —— 本机上没人听的口，拒绝得干脆，不碰外网。
    #[test]
    fn fetching_does_not_need_a_reactor_on_the_calling_thread() {
        let outcome = std::thread::spawn(|| {
            futures::executor::block_on(fetch_with_ua("http://127.0.0.1:1/sub", ""))
        })
        .join()
        .expect("fetch 不该 panic");
        assert!(outcome.is_err(), "那个口上没有东西在听");
    }

    #[test]
    fn the_address_check_happens_before_the_network() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            assert!(fetch_with_ua("   ", "").await.is_err());
            assert!(fetch_with_ua("ftp://example.com/sub", "").await.is_err());
            assert!(fetch_with_ua("D:\\sub.yml", "").await.is_err());
        });
    }
}
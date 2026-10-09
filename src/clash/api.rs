//! mihomo 的外部控制口（RESTful API）。
//!
//! 只包我们用得着的那几个端点。上游的字段名到 [`super::snapshot`] 为止 ——
//! 界面不认识 `"now"` / `"all"` / `"history"` 这些名字。
//!
//! 两条规矩贯穿全篇：
//!
//! * 控制口只绑回环，且**每个实例现生一个密钥**（见 [`super::config::RuntimePorts`]）。
//!   本机别的程序同样连得上这个口，没密钥的话等于把用户的节点列表和分流规则
//!   摆给任何一个人看。
//! * 出错时把 mihomo 回的那句正文带上。它的错误正文是有信息量的
//!   （"TUN 需要管理员权限"这类话就在里面），吞掉只剩一句"请求失败"，
//!   用户就只能看着一个拨不动的开关发愣。

use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use serde_json::{Value, json};

/// 本机控制口一次往返最多等多久。它就是本机的一条 HTTP，超过这个数说明
/// 那个实例已经不正常了。
const API_TIMEOUT: Duration = Duration::from_secs(5);

/// 拉一份订阅最多等多久。跨境线路慢是常态。
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// 流式端点（`/traffic`）一条连接最多挂多久。到点断开由调用方重连。
const STREAM_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// 全进程共用一个 HTTP 客户端（连接池、TLS 会话都在它身上）。
///
/// `no_proxy()` 是必须的：我们**自己**就是改系统代理的那一方
/// （见 [`super::system_proxy`]），而 reqwest 会在开启系统代理时去读那份设置 ——
/// 于是发往 `127.0.0.1` 控制口的请求可能被绕进代理里，正着说就是"设置系统代理
/// 这一步把控制通道自己掐了"。两处请求（控制口、拉订阅）都不该走代理。
pub fn http() -> &'static reqwest::Client {
    static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    HTTP.get_or_init(|| {
        reqwest::Client::builder()
            .no_proxy()
            .timeout(FETCH_TIMEOUT)
            .build()
            .expect("构建 HTTP 客户端失败")
    })
}

/// 一个运行中的实例的控制口。
#[derive(Clone, Debug)]
pub struct ApiClient {
    /// 形如 `http://127.0.0.1:54322`。
    base: String,
    secret: String,
}

impl ApiClient {
    pub fn new(port: u16, secret: &str) -> Self {
        Self {
            base: format!("http://127.0.0.1:{port}"),
            secret: secret.to_owned(),
        }
    }

    /// `GET /version`。起来了没有，就看它答不答。
    pub async fn version(&self) -> Result<Version> {
        let value = self.get("/version").await?;
        Ok(Version {
            version: value
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
            meta: value.get("meta").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    /// `GET /proxies`。节点、策略组、此刻选中的是哪一个，都在这份里。
    pub async fn proxies(&self) -> Result<Value> {
        self.get("/proxies").await
    }

    /// `GET /rules`。此刻生效的分流规则。
    pub async fn rules(&self) -> Result<Value> {
        self.get("/rules").await
    }

    /// `GET /configs`。端口、模式、TUN 开关这些。
    pub async fn configs(&self) -> Result<Value> {
        self.get("/configs").await
    }

    /// `PATCH /configs`。改运行中的参数 —— TUN 的开关走的就是它。
    pub async fn patch_configs(&self, body: Value) -> Result<()> {
        self.send(reqwest::Method::PATCH, "/configs", Some(body))
            .await
            .map(|_| ())
    }

    /// `GET /traffic`：持续消费内核的实时吞吐流，每来一份 `{"up": ..., "down": ...}`
    /// （字节每秒）就交给 `on_sample`。
    ///
    /// 这个端点**不是**一问一答：内核每秒往连接里推一行 JSON，永不收尾。
    /// 当普通 GET 去读正文只会一直等到超时 —— 所以它得自己一份循环，
    /// 由调用方放进独立任务里跑，**别占着会话循环**。流断了（内核重载、
    /// 超时）就带着错误返回，由调用方决定要不要重连。
    pub async fn traffic_stream(&self, mut on_sample: impl FnMut(Value)) -> Result<()> {
        // 请求级超时盖过客户端上那个 30 秒的总超时：流本来就是长连接。
        let mut response = self
            .request(reqwest::Method::GET, "/traffic")
            .timeout(STREAM_TIMEOUT)
            .send()
            .await
            .with_context(|| t!("clash.api.unreachable", path => "/traffic").to_string())?;
        if !response.status().is_success() {
            return Err(anyhow!("/traffic: {}", response.status()));
        }
        // 一行一份样本，但块的边界与行的边界不对齐：先攒进缓冲，凑齐整行再解。
        let mut pending: Vec<u8> = Vec::new();
        while let Some(chunk) = response.chunk().await.context("读 /traffic 流失败")? {
            pending.extend_from_slice(&chunk);
            while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = pending.drain(..=end).collect();
                if let Ok(sample) = serde_json::from_slice::<Value>(&line) {
                    on_sample(sample);
                }
            }
            // 没带换行收尾的版本：整块恰好是一份完整 JSON 就算一份。
            if !pending.is_empty()
                && let Ok(sample) = serde_json::from_slice::<Value>(&pending)
            {
                pending.clear();
                on_sample(sample);
            }
            // 兜底：一直拼不成 JSON 说明对面发的不是我们认识的东西，别无限攒。
            if pending.len() > 64 * 1024 {
                pending.clear();
            }
        }
        Ok(())
    }

    /// `GET /connections`。当前所有入站连接及其命中规则。
    pub async fn connections(&self) -> Result<Value> {
        self.get("/connections").await
    }

    /// 关闭一条实时连接。
    pub async fn close_connection(&self, id: &str) -> Result<()> {
        let path = format!("/connections/{}", encode(id));
        self.send(reqwest::Method::DELETE, &path, None)
            .await
            .map(|_| ())
    }

    /// 关闭当前全部实时连接。
    pub async fn close_connections(&self) -> Result<()> {
        self.send(reqwest::Method::DELETE, "/connections", None)
            .await
            .map(|_| ())
    }

    /// `PUT /proxies/{组名}`：把某个策略组切到 `node` 上。
    pub async fn select(&self, group: &str, node: &str) -> Result<()> {
        let path = format!("/proxies/{}", encode(group));
        self.send(reqwest::Method::PUT, &path, Some(json!({ "name": node })))
            .await
            .map(|_| ())
    }

    /// `GET /group/{名}/delay`：把一个组里的节点一次测完。
    ///
    /// 内核自己会并发去连，一次请求就够 —— 界面上的"整组测速"走的是它。
    pub async fn group_delay(&self, name: &str, test_url: &str, timeout: Duration) -> Result<()> {
        let path = format!(
            "/group/{}/delay?timeout={}&url={}",
            encode(name),
            timeout.as_millis(),
            encode(test_url)
        );
        self.get_with_timeout(&path, timeout + API_TIMEOUT).await?;
        Ok(())
    }

    /// `GET /proxies/{名}/delay`：单测一个节点。
    ///
    /// 这个端点**只回那一个节点的结果**（`{"delay": 88}`），延迟不会因此写进
    /// 内核的历史里 —— 界面上的数字要等下一次 `/proxies` 采样才更新。所以调它
    /// 的人得自己在采完样之后再看。
    ///
    /// 测不通时内核回的是一句 `An error occurred in the delay test` 之类的
    /// 4xx，`read` 会把它当错误抛出来 —— 那对界面不是"请求失败"，是"这个节点
    /// 不通"，两者在调用处分开认（见 `crate::clash::runtime` 的 `node_delay`）。
    pub async fn node_delay(&self, name: &str, test_url: &str, timeout: Duration) -> Result<u32> {
        let path = format!(
            "/proxies/{}/delay?timeout={}&url={}",
            encode(name),
            timeout.as_millis(),
            encode(test_url)
        );
        let value = self.get_with_timeout(&path, timeout + API_TIMEOUT).await?;
        // 回了 200 却没有 `delay`：当成 0 毫秒（内核有时这么回），
        // 界面上它会显示成"超时"那一档。
        Ok(value
            .get("delay")
            .and_then(Value::as_u64)
            .and_then(|delay| u32::try_from(delay).ok())
            .unwrap_or(0))
    }

    /// `PUT /configs`（带 `path`）：让实例重新读一遍那份配置文件。
    ///
    /// 用在我们**自己**重写了 `config.yaml` 之后 —— 比如用户改了订阅正文，
    /// 不想重起进程（重起会掉线、要重新拨 TUN）。
    pub async fn reload(&self, path: &str) -> Result<()> {
        self.send(
            reqwest::Method::PUT,
            "/configs?force=true",
            Some(json!({ "path": path })),
        )
        .await
        .map(|_| ())
    }

    async fn get(&self, path: &str) -> Result<Value> {
        self.get_with_timeout(path, API_TIMEOUT).await
    }

    async fn get_with_timeout(&self, path: &str, timeout: Duration) -> Result<Value> {
        let response = self
            .request(reqwest::Method::GET, path)
            .timeout(timeout)
            .send()
            .await
            .with_context(|| t!("clash.api.unreachable", path => path).to_string())?;
        read(response, path).await
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let mut request = self.request(method, path).timeout(API_TIMEOUT);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .with_context(|| t!("clash.api.unreachable", path => path).to_string())?;
        read(response, path).await
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let request = http().request(method, format!("{}{path}", self.base));
        match self.secret.is_empty() {
            true => request,
            false => request.bearer_auth(&self.secret),
        }
    }
}

/// `GET /version` 读出来的那两样。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub version: gpui_kit::SharedString,
    /// 是不是 Meta（mihomo）内核。Clash 原版没有我们要的那些端点。
    pub meta: bool,
}

/// 把响应读成 JSON，非 2xx 一律当成错误并把上游那句话带上。
async fn read(response: reqwest::Response, path: &str) -> Result<Value> {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        // 内核回的那句话是有信息量的（"TUN 要管理员权限"这类就在里面），
        // 原样带上。它是内核写的英文，翻译不了，也就不进语言表。
        let reason = body.trim();
        return Err(match reason.is_empty() {
            true => anyhow!(
                "{}",
                t!("clash.api.status", path => path, status => status.as_u16())
            ),
            false => anyhow!("{reason}"),
        });
    }
    if body.trim().is_empty() {
        // PUT / PATCH 成功时回的是 204 空正文。
        return Ok(Value::Null);
    }
    serde_json::from_str(&body).with_context(|| t!("clash.api.bad_json", path => path).to_string())
}

/// 路径里的那一格：名字里有中文、空格、斜杠都可能（节点名什么都有），
/// 走一次 URL 编码。
fn encode(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

use rust_i18n::t;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_segments_are_encoded() {
        assert_eq!(encode("PROXY"), "PROXY");
        assert_eq!(encode("香港 01"), "%E9%A6%99%E6%B8%AF%2001");
        assert_eq!(encode("a/b"), "a%2Fb");
    }

    /// 没密钥就不发 `Authorization`，有密钥就发 —— 控制口只绑回环，
    /// 这一格是防本机别的程序顺手读走节点列表的。
    #[test]
    fn the_secret_is_only_sent_when_there_is_one() {
        let with = ApiClient::new(9, "s3cret");
        let request = with
            .request(reqwest::Method::GET, "/version")
            .build()
            .unwrap();
        assert_eq!(
            request.headers().get("authorization").unwrap(),
            "Bearer s3cret"
        );

        let without = ApiClient::new(9, "");
        let request = without
            .request(reqwest::Method::GET, "/version")
            .build()
            .unwrap();
        assert!(request.headers().get("authorization").is_none());
    }
}

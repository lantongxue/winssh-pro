// 测试用的回环 WebDAV 服务器：内存文件系统，挂载在 `/dav` 下，要 Basic 认证。
//
// 形状照着线上那台出过事的 openlist（`https://alist.test.jindashen.cn/dav`）：
// - 只有 `/dav` 前缀下是 WebDAV，站点根上 `PROPFIND` 回 405；
// - 不带凭据回 `401` + `WWW-Authenticate: Basic`；
// - `MKCOL` 父目录不在回 409，对已存在的路径回 405；
// - `PUT` 父目录不在回 409；`MOVE` 带 `Overwrite: F` 撞上已有目标回 412。
//
// 只实现客户端会用到的那几个方法，够把连接层的路径折算对着"真发出去的请求"
// 验一遍。不是通用服务器，别在测试之外用它。

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub const USER: &str = "alice";
pub const PASSWORD: &str = "s3cret";
pub const MOUNT: &str = "/dav";

/// 一个节点：目录或文件。键是**解码后**、挂载点之内的路径（`/`、`/a`、`/a/b.txt`）。
#[derive(Clone)]
enum Node {
    Dir,
    File(Vec<u8>),
}

/// 正在跑的那一台。`fs` 共享出来，测试可以直接看服务器上落了什么。
pub struct FakeServer {
    pub addr: SocketAddr,
    fs: Arc<Mutex<BTreeMap<String, Node>>>,
}

impl FakeServer {
    /// 在数据层 runtime 上起一台，监听 127.0.0.1 的随机端口。
    pub fn start() -> Self {
        let fs = Arc::new(Mutex::new(BTreeMap::from([("/".to_string(), Node::Dir)])));
        let listener = winssh_db::runtime()
            .block_on(TcpListener::bind("127.0.0.1:0"))
            .expect("绑定回环端口");
        let addr = listener.local_addr().unwrap();
        let shared = fs.clone();
        winssh_db::runtime().spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let fs = shared.clone();
                tokio::spawn(async move {
                    let _ = serve(stream, fs).await;
                });
            }
        });
        Self { addr, fs }
    }

    /// 端点 URL（带挂载点），就是用户会填进表单的那一格。
    pub fn endpoint(&self) -> String {
        format!("http://{}{}", self.addr, MOUNT)
    }

    pub fn file(&self, path: &str) -> Option<Vec<u8>> {
        match self.fs.lock().unwrap().get(path) {
            Some(Node::File(bytes)) => Some(bytes.clone()),
            _ => None,
        }
    }

    pub fn is_dir(&self, path: &str) -> bool {
        matches!(self.fs.lock().unwrap().get(path), Some(Node::Dir))
    }

    pub fn put_file(&self, path: &str, bytes: &[u8]) {
        self.fs
            .lock()
            .unwrap()
            .insert(path.to_string(), Node::File(bytes.to_vec()));
    }

    pub fn put_dir(&self, path: &str) {
        self.fs.lock().unwrap().insert(path.to_string(), Node::Dir);
    }
}

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

async fn serve(mut stream: TcpStream, fs: Arc<Mutex<BTreeMap<String, Node>>>) -> std::io::Result<()> {
    let mut buffer = Vec::new();
    loop {
        let Some(request) = read_request(&mut stream, &mut buffer).await? else {
            return Ok(());
        };
        let (status, headers, body) = handle(&request, &fs);
        let mut head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n", body.len());
        for (key, value) in headers {
            head.push_str(&format!("{key}: {value}\r\n"));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(&body).await?;
    }
}

/// 读一个请求（支持 Content-Length 与 chunked 两种正文）。连接关了交回 `None`。
async fn read_request(stream: &mut TcpStream, buffer: &mut Vec<u8>) -> std::io::Result<Option<Request>> {
    let header_end = loop {
        if let Some(index) = find(buffer, b"\r\n\r\n") {
            break index;
        }
        let mut chunk = [0u8; 8192];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(None);
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    buffer.drain(..header_end + 4);
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split_whitespace();
    let method = first.next().unwrap_or_default().to_string();
    let path = first.next().unwrap_or_default().to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .collect();
    let mut request = Request {
        method,
        path,
        headers,
        body: Vec::new(),
    };

    if request
        .header("transfer-encoding")
        .is_some_and(|value| value.eq_ignore_ascii_case("chunked"))
    {
        loop {
            let line_end = loop {
                if let Some(index) = find(buffer, b"\r\n") {
                    break index;
                }
                fill(stream, buffer).await?;
            };
            let size = usize::from_str_radix(
                String::from_utf8_lossy(&buffer[..line_end]).split(';').next().unwrap_or("0").trim(),
                16,
            )
            .unwrap_or(0);
            buffer.drain(..line_end + 2);
            while buffer.len() < size + 2 {
                fill(stream, buffer).await?;
            }
            request.body.extend_from_slice(&buffer[..size]);
            buffer.drain(..size + 2);
            if size == 0 {
                break;
            }
        }
    } else {
        let length: usize = request
            .header("content-length")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        while buffer.len() < length {
            fill(stream, buffer).await?;
        }
        request.body = buffer.drain(..length).collect();
    }
    Ok(Some(request))
}

async fn fill(stream: &mut TcpStream, buffer: &mut Vec<u8>) -> std::io::Result<()> {
    let mut chunk = [0u8; 8192];
    let read = stream.read(&mut chunk).await?;
    if read == 0 {
        return Err(std::io::ErrorKind::UnexpectedEof.into());
    }
    buffer.extend_from_slice(&chunk[..read]);
    Ok(())
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

type Response = (&'static str, Vec<(String, String)>, Vec<u8>);

fn plain(status: &'static str) -> Response {
    (status, Vec::new(), Vec::new())
}

fn handle(request: &Request, fs: &Arc<Mutex<BTreeMap<String, Node>>>) -> Response {
    let decoded = crate::client::percent_decode(&request.path);
    let raw = decoded.split('?').next().unwrap_or_default();
    // 挂载点之外不是 WebDAV —— 照线上那台，站点根上的 WebDAV 方法一律 405。
    let Some(inside) = strip_mount(raw) else {
        return plain("405 Method Not Allowed");
    };

    let expected = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{USER}:{PASSWORD}"))
    );
    if request.header("authorization") != Some(expected.as_str()) {
        return (
            "401 Unauthorized",
            vec![("WWW-Authenticate".into(), "Basic realm=\"fake\"".into())],
            Vec::new(),
        );
    }

    let mut fs = fs.lock().unwrap();
    let parent_is_dir = |fs: &BTreeMap<String, Node>, path: &str| {
        matches!(fs.get(&winssh_sftp::path::parent(path)), Some(Node::Dir))
    };
    match request.method.as_str() {
        "OPTIONS" => ("200 OK", vec![("DAV".into(), "1, 2".into())], Vec::new()),
        "PROPFIND" => {
            let Some(node) = fs.get(&inside).cloned() else {
                return plain("404 Not Found");
            };
            let depth_one = request.header("depth") == Some("1");
            let mut body = String::from(
                r#"<?xml version="1.0" encoding="utf-8"?><D:multistatus xmlns:D="DAV:">"#,
            );
            body.push_str(&propfind_entry(&inside, &node));
            if depth_one && matches!(node, Node::Dir) {
                for (path, child) in fs.iter() {
                    if path != &inside && winssh_sftp::path::parent(path) == inside {
                        body.push_str(&propfind_entry(path, child));
                    }
                }
            }
            body.push_str("</D:multistatus>");
            ("207 Multi-Status", Vec::new(), body.into_bytes())
        }
        "GET" => match fs.get(&inside) {
            Some(Node::File(bytes)) => ("200 OK", Vec::new(), bytes.clone()),
            Some(Node::Dir) => plain("405 Method Not Allowed"),
            None => plain("404 Not Found"),
        },
        "PUT" => {
            if !parent_is_dir(&fs, &inside) {
                return plain("409 Conflict");
            }
            fs.insert(inside, Node::File(request.body.clone()));
            plain("201 Created")
        }
        "MKCOL" => {
            if fs.contains_key(&inside) {
                return plain("405 Method Not Allowed");
            }
            if !parent_is_dir(&fs, &inside) {
                return plain("409 Conflict");
            }
            fs.insert(inside, Node::Dir);
            plain("201 Created")
        }
        "DELETE" => {
            if !fs.contains_key(&inside) {
                return plain("404 Not Found");
            }
            let prefix = format!("{}/", inside.trim_end_matches('/'));
            fs.retain(|path, _| path != &inside && !path.starts_with(&prefix));
            plain("204 No Content")
        }
        "MOVE" => {
            let Some(destination) = request.header("destination") else {
                return plain("400 Bad Request");
            };
            // RFC 4918：Destination 是绝对 URI；头值里不该出现非 ASCII（要编码）。
            if !destination.is_ascii() || !destination.starts_with("http") {
                return plain("400 Bad Request");
            }
            let after_scheme = &destination[destination.find("://").unwrap() + 3..];
            let dest_path = &after_scheme[after_scheme.find('/').unwrap_or(after_scheme.len())..];
            let Some(dest) = strip_mount(&crate::client::percent_decode(dest_path)) else {
                return plain("502 Bad Gateway");
            };
            if !fs.contains_key(&inside) {
                return plain("404 Not Found");
            }
            if fs.contains_key(&dest) && request.header("overwrite") == Some("F") {
                return plain("412 Precondition Failed");
            }
            if !parent_is_dir(&fs, &dest) {
                return plain("409 Conflict");
            }
            let prefix = format!("{}/", inside.trim_end_matches('/'));
            let moved: Vec<(String, Node)> = fs
                .iter()
                .filter(|(path, _)| *path == &inside || path.starts_with(&prefix))
                .map(|(path, node)| (path.clone(), node.clone()))
                .collect();
            for (path, node) in moved {
                fs.remove(&path);
                let renamed = format!("{dest}{}", &path[inside.len()..]);
                fs.insert(renamed, node);
            }
            plain("201 Created")
        }
        _ => plain("405 Method Not Allowed"),
    }
}

/// `/dav/a/b` → `/a/b`，`/dav` 与 `/dav/` → `/`；挂载点之外是 `None`。
fn strip_mount(raw: &str) -> Option<String> {
    let normalized = winssh_sftp::path::normalize(raw);
    let rest = normalized.strip_prefix(MOUNT)?;
    match rest {
        "" => Some("/".to_string()),
        rest if rest.starts_with('/') => Some(rest.to_string()),
        _ => None,
    }
}

fn propfind_entry(path: &str, node: &Node) -> String {
    // href 照真服务器的样子：带挂载点、逐段编码，目录带尾斜杠。
    let encoded = crate::client::request_path(MOUNT, path);
    let href = match node {
        Node::Dir => format!("/{encoded}/"),
        Node::File(_) => format!("/{encoded}"),
    };
    let prop = match node {
        Node::Dir => "<D:resourcetype><D:collection/></D:resourcetype>".to_string(),
        Node::File(bytes) => format!(
            "<D:resourcetype/><D:getcontentlength>{}</D:getcontentlength>\
             <D:getlastmodified>Tue, 13 Oct 2026 08:00:00 GMT</D:getlastmodified>",
            bytes.len()
        ),
    };
    format!(
        "<D:response><D:href>{href}</D:href><D:propstat><D:prop>{prop}</D:prop>\
         <D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>"
    )
}

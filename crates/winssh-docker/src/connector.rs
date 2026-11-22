//! 把现有 russh 会话接进 bollard 的自定义传输。
//!
//! bollard 自带的 `ssh` 特性经 `openssh` crate 拉起**系统 ssh 二进制**重新认证，
//! 会绕开我们已经建好的 russh 会话（密码 / 跳板 / 代理 / 2FA 全丢），所以不用。
//! 这里照它 `ssh.rs` 的形状自己接一份：每来一条 HTTP 连接，就在**同一条** SSH
//! 连接上开一条 exec 通道跑 `docker system dial-stdio` —— 这正是 docker CLI 的
//! `ssh://` 上下文做的事 —— 把通道的双向字节流包成 hyper 的 `Read`/`Write`。
//!
//! 权限就是登录用户的 docker 权限，rootless 自动跟随；不开任何本机监听口，
//! 通道随会话生灭。SSH 的多路复用让若干条 HTTP 连接与终端互不干扰。

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use hyper::Uri;
use hyper::rt::{Read, ReadBufCursor, Write};
use hyper_util::client::legacy::connect::{Connected, Connection};
use hyper_util::rt::TokioIo;
use russh::client::{Handle, Msg};
use russh::ChannelStream;
use tower_service::Service;
use winssh_ssh::{SshHandler, exec_stream};

/// 每条 HTTP 连接开一条 `docker system dial-stdio` 通道。廉价克隆（一个 `Arc`）。
#[derive(Clone)]
pub struct DialStdioConnector {
    handle: Arc<Handle<SshHandler>>,
}

impl DialStdioConnector {
    pub fn new(handle: Arc<Handle<SshHandler>>) -> Self {
        Self { handle }
    }
}

impl Service<Uri> for DialStdioConnector {
    type Response = SshStream;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _uri: Uri) -> Self::Future {
        let handle = self.handle.clone();
        Box::pin(async move {
            // uri 一律忽略：不管 bollard 把请求写成 http://docker/…，字节都进
            // 这条 dial-stdio 通道，由远端 daemon 接手。
            let stream = exec_stream(&handle, "docker system dial-stdio")
                .await
                .map_err(io::Error::other)?;
            Ok(SshStream {
                inner: TokioIo::new(stream),
            })
        })
    }
}

/// russh 通道流包成 hyper 的读写端。`TokioIo` 已经把 tokio 的
/// `AsyncRead`/`AsyncWrite` 适配成 hyper 的 `Read`/`Write`，这里再套一层是为了
/// 给 hyper-util 的客户端实现 `Connection`（它要求响应类型自带这个 trait）。
pub struct SshStream {
    inner: TokioIo<ChannelStream<Msg>>,
}

impl Read for SshStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl Write for SshStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl Connection for SshStream {
    fn connected(&self) -> Connected {
        Connected::new()
    }
}

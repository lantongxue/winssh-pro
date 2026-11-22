//! Docker Engine API 客户端：经现有 SSH 会话说话，把 bollard 模型折算成
//! [`crate::types`] 里那些纯数据类型。
//!
//! 接线见 [`crate::connector`]：每条 HTTP 连接是一条 `docker system dial-stdio`
//! 通道。连接前先跑一次 `docker version` 探针 —— 一来拿到引擎版本，二来把
//! "没装 docker / 没权限" 这两种常见碰壁干净地分出来（dial-stdio 那条流只会
//! 给你 EOF，分不清原因）。
//!
//! 文件管理两条路：上传/下载/stat 走 `archive`（tar，停止的容器也能用）；
//! 列举/改名/删除/新建目录走 `exec`（运行中且有 shell），列举在 exec 不可用时
//! 退回 archive 读 tar 头。

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use bollard::Docker;
use bollard::container::LogOutput;
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use bollard::models::{
    ContainerInspectResponse, ContainerStatsResponse, ContainerSummary as ApiContainerSummary,
    ImageSummary as ApiImageSummary,
};
use bollard::query_parameters::{
    ContainerArchiveInfoOptionsBuilder, CreateImageOptionsBuilder, DownloadFromContainerOptionsBuilder,
    ListContainersOptionsBuilder, ListImagesOptionsBuilder, LogsOptionsBuilder,
    RemoveContainerOptionsBuilder, RemoveImageOptionsBuilder, RestartContainerOptionsBuilder,
    StatsOptionsBuilder, StopContainerOptionsBuilder, UploadToContainerOptionsBuilder,
};
use futures::{Stream, StreamExt};
use russh::client::Handle;
use winssh_ssh::exec::{ExecOutput, exec_script};
use winssh_ssh::SshHandler;

use crate::connector::DialStdioConnector;
use crate::types::{
    ContainerDetail, ContainerState, ContainerStats, ContainerSummary, EngineInfo, FileEntry,
    FileKind, ImageSummary, LogLine, Mount, PortMapping, PullProgress, mode_string, parse_exit_code,
};
use crate::{listing, path};

/// 探针最多等多久。`docker version` 要连 daemon，对端忙时会排上一会儿。
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
/// 一条 HTTP 请求的读写超时（秒）。日志/stats 是长流，不受这个限制。
const HTTP_TIMEOUT: u64 = 120;

/// 连接阶段的失败分类。界面据此画三种错误态（见草图板 6）。
#[derive(Clone, Debug)]
pub enum ConnectError {
    /// 远端没有 docker 命令。
    NotInstalled,
    /// 登录用户无权访问 daemon（不在 docker 组）。
    PermissionDenied,
    /// 其余：原话透传。
    Other(String),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInstalled => write!(f, "docker: command not found"),
            Self::PermissionDenied => {
                write!(f, "permission denied while trying to connect to the Docker daemon socket")
            }
            Self::Other(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ConnectError {}

/// 一条会话的 Docker 客户端。廉价克隆（bollard 内部是 `Arc`）。
#[derive(Clone)]
pub struct DockerClient {
    pub(crate) docker: Docker,
    engine: EngineInfo,
}

impl DockerClient {
    /// 在现有 SSH 会话上接好 Docker。先探针分类失败，再建 bollard 传输并协商版本。
    pub async fn connect(handle: Arc<Handle<SshHandler>>) -> Result<Self, ConnectError> {
        let probe = exec_script(
            &handle,
            "docker version --format '{{.Server.Version}}'",
            PROBE_TIMEOUT,
        )
        .await
        .map_err(|error| ConnectError::Other(error.to_string()))?;
        if !probe.success() {
            return Err(classify_probe(&probe));
        }

        let http = Arc::new(
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                // 不池化空闲连接：一条空闲连接就是一条活着的 dial-stdio 通道、
                // 一个远端进程。用完即关，随会话生灭，不留后台。
                .pool_max_idle_per_host(0)
                .build(DialStdioConnector::new(handle)),
        );
        let docker = Docker::connect_with_custom_transport(
            move |request| {
                let http = Arc::clone(&http);
                Box::pin(async move {
                    http.request(request)
                        .await
                        .map_err(bollard::errors::Error::from)
                })
            },
            Some("http://docker"),
            HTTP_TIMEOUT,
            bollard::API_DEFAULT_VERSION,
        )
        .map_err(|error| ConnectError::Other(error.to_string()))?;
        let docker = docker
            .negotiate_version()
            .await
            .map_err(|error| ConnectError::Other(error.to_string()))?;
        let version = docker
            .version()
            .await
            .map_err(|error| ConnectError::Other(error.to_string()))?;
        let engine = EngineInfo {
            version: version.version.unwrap_or_default().into(),
            api_version: version.api_version.unwrap_or_default().into(),
        };
        Ok(Self { docker, engine })
    }

    /// 握手拿到的引擎信息。
    pub fn engine(&self) -> &EngineInfo {
        &self.engine
    }

    // ─── 容器 ───────────────────────────────────────────────────────────────

    /// 列出全部容器（含已停止）。
    pub async fn list_containers(&self) -> Result<Vec<ContainerSummary>> {
        let options = ListContainersOptionsBuilder::new().all(true).build();
        let list = self.docker.list_containers(Some(options)).await?;
        Ok(list.into_iter().map(container_summary).collect())
    }

    /// 一个容器的详情（概览页）。
    pub async fn inspect(&self, name: &str) -> Result<ContainerDetail> {
        let response = self.docker.inspect_container(name, None).await?;
        Ok(container_detail(response))
    }

    pub async fn start(&self, name: &str) -> Result<()> {
        self.docker.start_container(name, None).await?;
        Ok(())
    }

    pub async fn stop(&self, name: &str, timeout_secs: i32) -> Result<()> {
        let options = StopContainerOptionsBuilder::new().t(timeout_secs).build();
        self.docker.stop_container(name, Some(options)).await?;
        Ok(())
    }

    pub async fn restart(&self, name: &str, timeout_secs: i32) -> Result<()> {
        let options = RestartContainerOptionsBuilder::new().t(timeout_secs).build();
        self.docker.restart_container(name, Some(options)).await?;
        Ok(())
    }

    /// 删除容器。`force` 连运行中一起删；`remove_volumes` 连匿名卷一起删。
    pub async fn remove(&self, name: &str, force: bool, remove_volumes: bool) -> Result<()> {
        let options = RemoveContainerOptionsBuilder::new()
            .force(force)
            .v(remove_volumes)
            .build();
        self.docker.remove_container(name, Some(options)).await?;
        Ok(())
    }

    /// 日志流。`follow` 持续跟随；`tail` 是 "all" 或行数串。
    pub fn logs(
        &self,
        name: &str,
        follow: bool,
        tail: &str,
        timestamps: bool,
    ) -> impl Stream<Item = Result<LogLine>> {
        let options = LogsOptionsBuilder::new()
            .follow(follow)
            .stdout(true)
            .stderr(true)
            .timestamps(timestamps)
            .tail(tail)
            .build();
        let stream = self.docker.logs(name, Some(options));
        stream.map(|item| item.map(map_log).map_err(anyhow::Error::from))
    }

    /// 资源读数流。`one_shot` 只取一帧（不持续）。
    pub fn stats(
        &self,
        name: &str,
        one_shot: bool,
    ) -> impl Stream<Item = Result<ContainerStats>> {
        let options = StatsOptionsBuilder::new()
            .stream(!one_shot)
            .one_shot(one_shot)
            .build();
        let stream = self.docker.stats(name, Some(options));
        stream.map(|item| {
            item.map(|response| compute_stats(&response))
                .map_err(anyhow::Error::from)
        })
    }

    // ─── 镜像 ───────────────────────────────────────────────────────────────

    pub async fn list_images(&self) -> Result<Vec<ImageSummary>> {
        let options = ListImagesOptionsBuilder::new().build();
        let list = self.docker.list_images(Some(options)).await?;
        Ok(list.into_iter().map(image_summary).collect())
    }

    /// 拉取镜像，返回进度流。`reference` 形如 `nginx:1.27`（tag 可省）。
    pub fn pull(&self, reference: &str) -> impl Stream<Item = Result<PullProgress>> {
        let (from_image, tag) = split_reference(reference);
        let mut builder = CreateImageOptionsBuilder::new().from_image(&from_image);
        if !tag.is_empty() {
            builder = builder.tag(&tag);
        }
        let stream = self.docker.create_image(Some(builder.build()), None, None);
        stream.map(|item| {
            item.map(|info| PullProgress {
                status: info.status.unwrap_or_default().into(),
                current: info
                    .progress_detail
                    .as_ref()
                    .and_then(|p| p.current)
                    .unwrap_or(0)
                    .max(0) as u64,
                total: info
                    .progress_detail
                    .as_ref()
                    .and_then(|p| p.total)
                    .unwrap_or(0)
                    .max(0) as u64,
                error: info
                    .error_detail
                    .and_then(|e| e.message)
                    .map(Into::into),
            })
            .map_err(anyhow::Error::from)
        })
    }

    pub async fn remove_image(&self, name: &str, force: bool) -> Result<()> {
        let options = RemoveImageOptionsBuilder::new().force(force).build();
        self.docker.remove_image(name, Some(options), None).await?;
        Ok(())
    }

    // ─── 文件 ───────────────────────────────────────────────────────────────

    /// 列举容器内一层目录。运行中且有 shell 走 exec；否则退回 archive 读 tar 头。
    pub async fn list_files(&self, name: &str, dir: &str) -> Result<Vec<FileEntry>> {
        let dir = path::normalize(dir);
        let script = listing::script(&dir);
        match self.exec_collect(name, &["/bin/sh", "-c", &script]).await {
            Ok(out) if out.exit_code.unwrap_or(0) == 0 => Ok(listing::parse(&dir, &out.stdout)),
            // cd 失败（目录不存在/无权进入）：如实报，不退回 archive 掩盖原因。
            Ok(out) => bail!("{}", remote_reason(&out)),
            // exec 起不来（已停止 / 没 shell）：退回 archive。
            Err(_) => self.list_files_archive(name, &dir).await,
        }
    }

    async fn list_files_archive(&self, name: &str, dir: &str) -> Result<Vec<FileEntry>> {
        let bytes = self.archive_bytes(name, dir).await?;
        Ok(listing::parse_tar(dir, &bytes))
    }

    /// 取一个路径的元信息（archive HEAD）。下载前探类型/存在性用。
    pub async fn stat(&self, name: &str, remote_path: &str) -> Result<FileEntry> {
        let remote = path::normalize(remote_path);
        let options = ContainerArchiveInfoOptionsBuilder::new().path(&remote).build();
        let stat = self
            .docker
            .get_container_archive_info(name, Some(options))
            .await?;
        let kind = if stat.link_target.is_empty() {
            // mode 高位带类型；Docker 的 mode 是 os.FileMode，目录位 0x80000000。
            if stat.file_mode & 0o170000 == 0o040000 {
                FileKind::Dir
            } else {
                FileKind::File
            }
        } else {
            FileKind::Symlink
        };
        let mtime = stat
            .modification_time
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.timestamp())
            .unwrap_or(0);
        let file_name = path::file_name(&remote).to_owned();
        Ok(FileEntry {
            name: file_name.clone().into(),
            path: remote.clone().into(),
            kind,
            size: stat.size.max(0) as u64,
            mtime,
            mode: mode_string(kind, stat.file_mode & 0o7777).into(),
            link_target: stat.link_target.into(),
        })
    }

    /// 下载容器内一个路径（文件或目录）到本地目录，返回落盘后的顶层路径。
    ///
    /// archive 拉回的 tar 以路径基名打头，解到 `dest_dir` 下即 `dest_dir/基名`。
    /// 流式落临时文件再解包，不整份进内存。
    pub async fn download(
        &self,
        name: &str,
        remote_path: &str,
        dest_dir: &Path,
    ) -> Result<PathBuf> {
        let remote = path::normalize(remote_path);
        let base = path::file_name(&remote).to_owned();
        if base.is_empty() {
            bail!("cannot download the container root");
        }
        let bytes = self.archive_bytes(name, &remote).await?;
        let dest = dest_dir.to_path_buf();
        let top = dest.join(&base);
        tokio::task::spawn_blocking(move || -> Result<()> {
            std::fs::create_dir_all(&dest)?;
            let mut archive = tar::Archive::new(bytes.as_slice());
            archive.set_overwrite(true);
            archive.unpack(&dest)?;
            Ok(())
        })
        .await??;
        Ok(top)
    }

    /// 上传若干本地文件/目录到容器内 `remote_dir`。打包成 tar 流式 PUT。
    pub async fn upload(
        &self,
        name: &str,
        local_paths: &[PathBuf],
        remote_dir: &str,
    ) -> Result<()> {
        let remote = path::normalize(remote_dir);
        let tmp = tempfile::NamedTempFile::new()?;
        let tmp_path = tmp.path().to_path_buf();
        let paths = local_paths.to_vec();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let file = std::fs::File::create(&tmp_path)?;
            let mut builder = tar::Builder::new(file);
            for local in &paths {
                let entry_name = local
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "upload".to_owned());
                if local.is_dir() {
                    builder.append_dir_all(&entry_name, local)?;
                } else {
                    builder.append_path_with_name(local, &entry_name)?;
                }
            }
            builder.into_inner()?.flush()?;
            Ok(())
        })
        .await??;
        let file = tokio::fs::File::open(tmp.path()).await?;
        let stream = tokio_util::io::ReaderStream::new(file);
        let options = UploadToContainerOptionsBuilder::new().path(&remote).build();
        self.docker
            .upload_to_container(name, Some(options), bollard::body_try_stream(stream))
            .await?;
        Ok(())
    }

    /// 新建目录（运行中且有 shell）。
    pub async fn mkdir(&self, name: &str, dir: &str) -> Result<()> {
        let dir = path::normalize(dir);
        self.exec_ok(name, &["mkdir", "-p", &dir]).await
    }

    /// 重命名/移动（运行中且有 shell）。
    pub async fn rename(&self, name: &str, from: &str, to: &str) -> Result<()> {
        let from = path::normalize(from);
        let to = path::normalize(to);
        self.exec_ok(name, &["mv", &from, &to]).await
    }

    /// 删除（运行中且有 shell）。目录递归删 —— 界面已二次确认。
    pub async fn delete(&self, name: &str, target: &str) -> Result<()> {
        let target = path::normalize(target);
        self.exec_ok(name, &["rm", "-rf", &target]).await
    }

    // ─── 内部 ───────────────────────────────────────────────────────────────

    /// 拉一个路径的 archive tar 字节（列举/下载共用）。
    async fn archive_bytes(&self, name: &str, remote: &str) -> Result<Vec<u8>> {
        let options = DownloadFromContainerOptionsBuilder::new().path(remote).build();
        let mut stream = self.docker.download_from_container(name, Some(options));
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            bytes.extend_from_slice(&chunk?);
        }
        Ok(bytes)
    }

    /// 跑一条命令并收齐 stdout/stderr/退出码。
    async fn exec_collect(&self, name: &str, cmd: &[&str]) -> Result<ExecResult> {
        let config = CreateExecOptions {
            cmd: Some(cmd.iter().map(|s| (*s).to_owned()).collect()),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            ..Default::default()
        };
        let created = self.docker.create_exec(name, config).await?;
        let started = self
            .docker
            .start_exec(
                &created.id,
                Some(StartExecOptions {
                    detach: false,
                    tty: false,
                    ..Default::default()
                }),
            )
            .await?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        if let StartExecResults::Attached { mut output, .. } = started {
            while let Some(item) = output.next().await {
                match item? {
                    LogOutput::StdErr { message } => stderr.extend_from_slice(&message),
                    LogOutput::StdOut { message } | LogOutput::Console { message } => {
                        stdout.extend_from_slice(&message);
                    }
                    _ => {}
                }
            }
        }
        let inspect = self.docker.inspect_exec(&created.id).await?;
        Ok(ExecResult {
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            exit_code: inspect.exit_code,
        })
    }

    /// 跑一条命令，退出码非 0 就报远端原话。
    async fn exec_ok(&self, name: &str, cmd: &[&str]) -> Result<()> {
        let out = self.exec_collect(name, cmd).await?;
        if out.exit_code.unwrap_or(0) != 0 {
            bail!("{}", remote_reason(&out));
        }
        Ok(())
    }
}

/// 一次 exec 的收齐结果。
struct ExecResult {
    stdout: String,
    stderr: String,
    exit_code: Option<i64>,
}

/// 给用户的失败原因：优先 stderr 首行，没有就退回退出码。
fn remote_reason(out: &ExecResult) -> String {
    if let Some(line) = out.stderr.lines().map(str::trim).find(|l| !l.is_empty()) {
        return line.to_owned();
    }
    match out.exit_code {
        Some(code) => format!("exit status {code}"),
        None => "no output".to_owned(),
    }
}

/// 探针失败分类：127 / not found → 没装；permission denied → 没权限。
fn classify_probe(out: &ExecOutput) -> ConnectError {
    let stderr = out.stderr.to_lowercase();
    if out.exit_status == Some(127) || stderr.contains("not found") {
        ConnectError::NotInstalled
    } else if stderr.contains("permission denied") {
        ConnectError::PermissionDenied
    } else {
        ConnectError::Other(out.failure_reason())
    }
}

/// `nginx:1.27` → (`nginx`, `1.27`)；没有 tag 时 tag 为空。带 digest 的
/// `nginx@sha256:…` 整体当 from_image（tag 留空）。
fn split_reference(reference: &str) -> (String, String) {
    if let Some((name, digest)) = reference.split_once('@') {
        return (format!("{name}@{digest}"), String::new());
    }
    // 最后一个冒号若是 tag 分隔（后面不含斜杠）才拆，避免误伤 registry:5000/img。
    match reference.rsplit_once(':') {
        Some((name, tag)) if !tag.contains('/') => (name.to_owned(), tag.to_owned()),
        _ => (reference.to_owned(), String::new()),
    }
}

/// 一条日志输出折成视图行（stderr 区分出来供着色）。容器/服务日志共用。
pub(crate) fn map_log(output: LogOutput) -> LogLine {
    let stderr = matches!(output, LogOutput::StdErr { .. });
    let text = String::from_utf8_lossy(output.as_ref()).into_owned();
    LogLine {
        stderr,
        text: text.into(),
    }
}

/// 把 serde 枚举折成它的小写字符串（`RUNNING` → "running"），不必逐 variant 匹配。
pub(crate) fn enum_str<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// id 短形式：去掉 `sha256:` 前缀取前 12 位。
pub(crate) fn short_id(id: &str) -> String {
    id.strip_prefix("sha256:").unwrap_or(id).chars().take(12).collect()
}

fn container_summary(m: ApiContainerSummary) -> ContainerSummary {
    let full_id = m.id.unwrap_or_default();
    let name = m
        .names
        .as_ref()
        .and_then(|n| n.first())
        .map(|n| n.trim_start_matches('/').to_owned())
        .unwrap_or_default();
    let status = m.status.unwrap_or_default();
    let state = m
        .state
        .as_ref()
        .map(|s| ContainerState::parse(&enum_str(s)))
        .unwrap_or(ContainerState::Unknown);
    ContainerSummary {
        id: short_id(&full_id).into(),
        full_id: full_id.into(),
        name: name.into(),
        image: m.image.unwrap_or_default().into(),
        state,
        exit_code: parse_exit_code(&status),
        status: status.into(),
        created: m.created.unwrap_or(0),
        ports: m
            .ports
            .unwrap_or_default()
            .into_iter()
            .map(|p| PortMapping {
                host_ip: p.ip.unwrap_or_default().into(),
                host_port: p.public_port.unwrap_or(0),
                container_port: p.private_port,
                protocol: p
                    .typ
                    .as_ref()
                    .map(enum_str)
                    .unwrap_or_else(|| "tcp".to_owned())
                    .into(),
            })
            .collect(),
    }
}

fn container_detail(r: ContainerInspectResponse) -> ContainerDetail {
    let full_id = r.id.unwrap_or_default();
    let config = r.config.unwrap_or_default();
    let state_model = r.state.unwrap_or_default();
    let state = state_model
        .status
        .as_ref()
        .map(|s| ContainerState::parse(&enum_str(s)))
        .unwrap_or(ContainerState::Unknown);
    let name = r
        .name
        .unwrap_or_default()
        .trim_start_matches('/')
        .to_owned();
    let command = config
        .cmd
        .clone()
        .map(|c| c.join(" "))
        .unwrap_or_else(|| {
            let mut parts = vec![r.path.clone().unwrap_or_default()];
            parts.extend(r.args.clone().unwrap_or_default());
            parts.join(" ")
        });
    let restart_policy = r
        .host_config
        .as_ref()
        .and_then(|h| h.restart_policy.as_ref())
        .and_then(|p| p.name.as_ref())
        .map(|n| {
            let s = enum_str(n);
            if s.is_empty() { "no".to_owned() } else { s }
        })
        .unwrap_or_else(|| "no".to_owned());
    let ports = r
        .network_settings
        .as_ref()
        .and_then(|n| n.ports.clone())
        .map(ports_from_map)
        .unwrap_or_default();
    let mounts = r
        .mounts
        .unwrap_or_default()
        .into_iter()
        .map(|m| Mount {
            source: m.source.unwrap_or_default().into(),
            destination: m.destination.unwrap_or_default().into(),
            read_only: !m.rw.unwrap_or(true),
            kind: m
                .typ
                .as_ref()
                .map(enum_str)
                .unwrap_or_default()
                .into(),
        })
        .collect();
    let env = config
        .env
        .unwrap_or_default()
        .into_iter()
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (k.to_owned().into(), v.to_owned().into()),
            None => (pair.into(), String::new().into()),
        })
        .collect();
    let status = state_model
        .status
        .as_ref()
        .map(enum_str)
        .unwrap_or_default();
    ContainerDetail {
        id: short_id(&full_id).into(),
        full_id: full_id.into(),
        name: name.into(),
        image: config.image.unwrap_or_else(|| r.image.unwrap_or_default()).into(),
        state,
        status: status.into(),
        created: r.created.map(|t| t.timestamp()).unwrap_or(0),
        started_at: state_model
            .started_at
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.timestamp())
            .unwrap_or(0),
        command: command.into(),
        restart_policy: restart_policy.into(),
        working_dir: config.working_dir.unwrap_or_default().into(),
        ports,
        mounts,
        env,
        exit_code: state_model.exit_code,
    }
}

/// inspect 的端口表：key 形如 "80/tcp"，value 是宿主绑定列表（可能为空）。
fn ports_from_map(map: bollard::models::PortMap) -> Vec<PortMapping> {
    let mut out = Vec::new();
    for (key, bindings) in map {
        let (port_str, protocol) = key.split_once('/').unwrap_or((key.as_str(), "tcp"));
        let Ok(container_port) = port_str.parse::<u16>() else {
            continue;
        };
        match bindings {
            Some(bindings) if !bindings.is_empty() => {
                for b in bindings {
                    out.push(PortMapping {
                        host_ip: b.host_ip.unwrap_or_default().into(),
                        host_port: b
                            .host_port
                            .as_deref()
                            .and_then(|p| p.parse().ok())
                            .unwrap_or(0),
                        container_port,
                        protocol: protocol.to_owned().into(),
                    });
                }
            }
            _ => out.push(PortMapping {
                host_ip: String::new().into(),
                host_port: 0,
                container_port,
                protocol: protocol.to_owned().into(),
            }),
        }
    }
    out.sort_by_key(|p| (p.container_port, p.host_port));
    out
}

fn image_summary(m: ApiImageSummary) -> ImageSummary {
    let tags: Vec<gpui_kit::SharedString> = m
        .repo_tags
        .iter()
        .filter(|t| t.as_str() != "<none>:<none>")
        .map(|t| t.clone().into())
        .collect();
    ImageSummary {
        id: short_id(&m.id).into(),
        full_id: m.id.clone().into(),
        dangling: tags.is_empty(),
        tags,
        size: m.size.max(0) as u64,
        created: m.created,
        containers: m.containers,
    }
}

/// 从一帧 stats 折算占用率与速率。CPU 是两次采样之差（响应里自带 precpu）。
///
/// 内存扣掉 inactive_file（与 docker CLI 一致：那部分是可以随时回收的页缓存）。
pub(crate) fn compute_stats(s: &ContainerStatsResponse) -> ContainerStats {
    let cpu = s.cpu_stats.as_ref();
    let precpu = s.precpu_stats.as_ref();
    let cpu_delta = cpu
        .and_then(|c| c.cpu_usage.as_ref())
        .and_then(|u| u.total_usage)
        .unwrap_or(0) as f64
        - precpu
            .and_then(|c| c.cpu_usage.as_ref())
            .and_then(|u| u.total_usage)
            .unwrap_or(0) as f64;
    let system_delta = cpu.and_then(|c| c.system_cpu_usage).unwrap_or(0) as f64
        - precpu.and_then(|c| c.system_cpu_usage).unwrap_or(0) as f64;
    let cpus = cpu
        .and_then(|c| c.online_cpus)
        .filter(|n| *n > 0)
        .unwrap_or(1) as f64;
    let cpu_percent = if system_delta > 0.0 && cpu_delta >= 0.0 {
        (cpu_delta / system_delta) * cpus * 100.0
    } else {
        0.0
    };

    let memory = s.memory_stats.as_ref();
    let inactive_file = memory
        .and_then(|m| m.stats.as_ref())
        .and_then(|stats| stats.get("inactive_file"))
        .copied()
        .unwrap_or(0);
    let mem_used = memory
        .and_then(|m| m.usage)
        .unwrap_or(0)
        .saturating_sub(inactive_file);
    let mem_limit = memory.and_then(|m| m.limit).unwrap_or(0);

    let (net_rx, net_tx) = s
        .networks
        .as_ref()
        .map(|nets| {
            nets.values().fold((0u64, 0u64), |(rx, tx), n| {
                (
                    rx + n.rx_bytes.unwrap_or(0),
                    tx + n.tx_bytes.unwrap_or(0),
                )
            })
        })
        .unwrap_or((0, 0));

    ContainerStats {
        cpu_percent,
        mem_used,
        mem_limit,
        net_rx,
        net_tx,
        pids: s.pids_stats.as_ref().and_then(|p| p.current).unwrap_or(0),
    }
}

#[cfg(test)]
#[path = "tests/client.rs"]
mod tests;

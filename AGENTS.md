## WinSSH Pro 项目

WinSSH Pro 是一个使用 rust+cargo+gpui+russh 构建的跨平台、原生高性能 SSH 客户端，为桌面用户提供超 120+fps 的高性能原生渲染体验。

## 项目环境

- 开发语言：rust
- 依赖管理：cargo
- GUI 框架：gpui
- 三方 UI 组件：gpui-kit —— 应用只依赖 `gpui-kit` 这一个门面 crate，
  gpui 本体是 `gpui_kit::*`，带样式的组件是 `gpui_kit::component`，
  图标资源是 `gpui_kit::assets`，平台层是 `gpui_kit::platform`
- SSH+SFTP 核心库：russh
- 组网（EasyTier 主机）：`easytier` 库嵌进进程里跑（git 依赖，钉 rev，见
  `Cargo.toml` 那一段注释与 `src/easytier/`）。它的 protobuf 在构建时现编，
  **本机要有 `protoc`**：macOS `brew install protobuf`，Debian/Ubuntu
  `apt install protobuf-compiler`，Windows 由构建脚本自己下载。
  虚拟网卡那一档在 Windows 上要 `wintun.dll`：放在 `third_party/<架构>/`，
  `build.rs` 按目标架构拷到 exe 旁边（`cargo run` / `cargo test` 都找得到），
  发版 workflow 把它和 exe 一起打包
- Clash 订阅（mihomo）：**不嵌库**，起一个子进程，通过它的 RESTful API 控制
  （见 `src/clash/`）。那份内核**不由我们发、也不在构建期摆**：订阅页在缺内核时
  引导用户从官方 GitHub Releases 下载到用户数据目录（见 `src/clash/downloader.rs`
  与 `src/ui/dialogs/mihomo_download.rs`），`clash::runtime::binary_path` 会去那里找。
  没有它应用照跑，只有 Clash 那一档会说"没找到 mihomo"。mihomo 建虚拟网卡时
  按名字加载 `wintun.dll`，所以下载目录要和 `wintun.dll` 落在同一处 ——
  `clash::downloader` 下完内核后会把 exe 旁边的 `wintun.dll` 一并拷到
  mihomo 目录里（见 `copy_wintun_if_needed`）。
  拨 TUN 要管理员权限：Windows 上拨开关时应用经一次 UAC 拉起自带的
  `winssh-broker.exe`（`src/bin/broker.rs`）代管内核，GUI 留在用户态；
  broker 随 GUI 退出自动收走内核，不装服务、不留后台，发版与主 exe 同目录打包

## 开发规则

- 组件化优先原则
- 原生高性能原则
- 内存安全原则
- 任何UI界面的实现必须阅读【参考代码】章节提到的代码库，从里面找可以复用的组件/模块，避免重复造轮子，非UI界面的需求不用参考
- 不同的 UI 组件代码需要分类存放，比如弹窗类、面板类、菜单类等等，需要建立对应的类型目录存放组件代码
- 禁止修改 @refs/ 下的代码，如果有功能必须修改底层库才能实现，那么拒绝这次功能开发


## workspace 与 crate 分层

本工程是一个 Cargo workspace：根包 `winssh-pro` 只保留应用外壳（`main.rs`、`ui/`、
`tray/`、`assets.rs`、`i18n.rs`、`single_instance.rs`、`bin/broker.rs` 与 `build.rs`），
其余按职责拆进 `crates/` 下的独立 crate。第三方依赖的版本钉子统一收在根 `Cargo.toml`
的 `[workspace.dependencies]`，各 crate 用 `dep = { workspace = true }` 继承；
`[patch.crates-io]`、`[profile.*]` 也留在根，对全体成员生效。`refs/`（gpui-kit、
alacritty）与 `vendor/`（各补丁副本）是落在本仓库目录树里的独立 crate / 仓库，靠
`[workspace]` 的 `exclude = ["refs", "vendor"]` 划出去 —— 不 exclude，cargo 会把它们
当本 workspace 成员去解析它们的 `edition.workspace = true` 而报错。

依赖只能自上而下，禁止反向：

```
domain ← tls / db ← repository ← 后端 (ssh · proxy · sftp · s3 · ftp · webdav ·
  serial · telnet · easytier · clash · broker-protocol · assistant · localfs ·
  zmodem · terminal · desktop · netstats · vnc · rdp · backup · monitor)
  ← service ← 根包 (ui / tray / main)
```

- `winssh-domain`：纯领域数据与编解码（`Host`、各 `*Settings`、`Forward`、代理数据、
  `Credential`、`TrustedHost`、`Session` 读数、`Snippet`、命令历史、托盘开关）。只借 gpui
  的 `SharedString` 一个类型，不碰 ui / service，是全图最底层、谁都能依赖它。
- `winssh-db`（SeaORM 连接 + 实体 + 迁移 + 应用级 tokio runtime）/ `winssh-repository`
  （纯数据访问）。
- 各后端 crate：一个协议 / 子系统一个 crate，只依赖领域层与更底层的 crate，**不认识任何
  界面类型**。
- `winssh-service`：服务层门面（唯一给 UI 的数据入口）。为免逐文件改路径，它的 `lib.rs`
  用一组 `pub use winssh_x as x;` 把下层 crate 在本 crate 根上起了别名，服务各文件里的
  `crate::x::…` 旧路径无需改写即可解析。
- 根包对每个下沉过的模块留一组 `pub use winssh_*::*` 兼容桩（`src/host.rs`、`src/ssh/`、
  `src/db/` …），ui 层里的 `crate::host`、`crate::ssh` 等旧路径照旧可用。

给新功能找落点：先认清它属于哪一层，放进对应 crate。遇到本该往下却被上层拖住的环（某后端
持有面板 `Entity`、或后端调服务层），用 sink/trait、provider 钩子，或把共享的那点数据下移
来打断，不要把上层 crate 变成下层 crate 的依赖。进程级 config 缓存的 `exclusive_for_test`
之类的测试工具，一旦被上层 crate 的测试用到，就得放开成 `#[doc(hidden)] pub` 或门在
`test-support` 特性后面（`#[cfg(test)]` 的东西链进下游 crate 就没了）。

## 渲染性能

目标 120fps —— **一帧的预算只有 8.3ms**，整棵界面树都得挤在这个数里。

**每帧都会重建整棵元素树。** GPUI 从根视图往下重画。Dock 那一层确实把每个面板
挂成带缓存的子视图（`TabPanel::render_active_panel`），但面板里任何一处
`cx.notify()` 都会让那层缓存不命中，整块面板连着它的子面板重建一遍；而 GPUI
重建一棵缓存子树时会置上 `Window::refreshing`，**面板内部再套 `Entity::cached`
一律跳过复用**。所以终端一刷屏，右侧监控栏、底部 SFTP 栏那一帧的开销是每帧都付
—— 别指望缓存，只能让它本身便宜。

**少嵌一层 flex，胜过省一堆元素。** 每多一层"尺寸靠内容量出来"的 flex 容器，
它下面**整棵子树就多排一遍版**（flexbox 要先量一遍假设尺寸，再分配空间正式排
一遍）。同一份两百个元素的内容，只改外面包什么，一帧的代价（release 实测）：

| 外面多包一层 | 每帧 |
| --- | --- |
| block `div()` | 不涨 |
| `v_flex()`（尺寸确定） | +70% |
| 再加一层内容尺寸的 `v_flex()` | 再翻一倍 |

所以：

- 纯装饰（内边距、底色、圆角、边框）用 block `div()`，别顺手写 `v_flex()`。
- 和父级同方向同 gap 的包装层直接删；只为"把标题凑成一组好让 `justify_between`
  生效"的容器，改成父级 `gap_1` ＋ 末项 `ml_auto`（画出来一样，少一层）。
- 重复行给定高（`PROCESS_ROW_HEIGHT` 那种），尺寸确定，量测缓存才命中。
- 行数多的列表用虚拟列表：定高 ＋ 独立排版根，每行比嵌套 flex 便宜数倍。
- 动手前后各跑一遍量尺：`monitor::tests::frame_cost`、
  `terminal::tests::frame_cost`（都 `#[ignore]`，注释里记着当时的数）。

一笔真实的账：监控面板 Metrics 页拆掉三层内容尺寸的 flex 容器，画面一模一样，
6.9ms/帧 → 2.4ms/帧。

## 调试规则
禁止自启动截图调试，必须由人工手动验证

## 测试规则

单元测试一律外移到**被测模块所在 crate 的** `src/tests/`，不在源文件里写内联 `#[cfg(test)] mod tests { ... }`（工程已拆成多个 crate，每个 crate 都有自己的 `src/tests/`）：

- **目录镜像源码树**：`crates/winssh-service/src/s3_service.rs` 的测试放
  `crates/winssh-service/src/tests/s3_service.rs`，文件名与被测模块同名。某个 crate 的
  `src/tests/` 只放该 crate 的测试，不放生产代码。
- **源文件末尾用 `#[path]` 挂回**：
  ```rust
  #[cfg(test)]
  #[path = "tests/s3_service.rs"]
  mod tests;
  ```
  路径相对该源文件所在目录：模块在 crate 的 `src/` 根下就用 `tests/x.rs`，在子目录
  （如 `src/file_copy/incoming.rs`）里就用 `../tests/…`。跨多层的嵌套测试树（如
  `tests/session/presentation.rs`）靠相对 `#[path]` 挂回，拆 crate 时只改一级 `../` 深度。
  平台限定的测试可以在 `cfg` 上加条件（如 `#[cfg(all(test, unix))]`）。
- **不用顶层 `tests/` 集成测试，统一用 `#[path]` 挂回的内联单测**：测试靠 `use super::*;`
  访问被测模块的私有项，这只有当测试模块与被测模块同在一个编译单元里才成立；放进 crate
  顶层的 `tests/` 会被 cargo 当独立集成测试 target 编译，看不到内部私有项。所以测试文件虽
  然物理上在一个叫 `tests` 的目录，必须始经源文件里的 `#[path]` 挂回来（属于该 crate 的
  `cfg(test)`，不是集成测试）。需要跨 crate 共用的测试工（如假 sshd、内存库服务工厂），
  用 `test-support` 特性门控，由使用方的 dev-dependencies 点亮，别往 `tests/` 里堆。
- 测试体用 `use super::*;` 拿被测模块的私有项 —— 外移只是换文件，访问权限不变。

写法上的几条：

- **测试盯纯函数**：把可测的逻辑提炼成无副作用的纯函数（`path::normalize`、
  `with_leading_slash` 那样），别为了测试去硬造第三方类型 —— 依赖库的构造器
  常是 `pub(crate)`，外面造不出来。
- **一个测试一个行为**，函数名蛇形描述行为（`root_mode_blocks_writes_at_root`），
  不叫 `test_1`、`test_err`。
- **线上踩过的坑要在断言旁留注释**：说清楚当时的事故关键词与原因
  （如 OSS 粘段那两条），日后回归或重构的人才知道这条测试在守什么。
- 异步测试用 `winssh_db::runtime().block_on(...)`（根包里写 `crate::db::runtime`）。
- 环境敏感、容易随机器漂的测试标 `#[ignore]`，并在注释里记下当时的数
  （`monitor::tests::frame_cost` 那样）。

## 命名规范

术语跟着设计原型走，一个概念只用一个词：

- **主机（Host）**：一条保存下来的连接配置，对应原型里的 `ServerHost`。分组是 `HostGroup`。
- **会话（Session）**：照着主机打开的那条活着的连接，也就是中心区的一个终端标签页。
- **凭据（Credential）**：保险箱里的一条记录 —— 一组账号密码、一把 SSH 私钥，
  或一个 API 令牌，对应原型里的 `VaultCredential`。保险箱本体是 `CredentialVaultView`
  （侧边栏那一栏，同时持有快照、筛子与所有落库动作），全屏那一扇独立窗口是
  `CredentialVaultWindow`，它持有一份前者、只另画版式。
  凭据和主机是**取用**关系不是引用：主机表单里挑一条，把内容填进表单，
  存进 `hosts` 表的还是那份内容本身（见 `src/credential.rs` 的模块文档）。
- **订阅（Subscription）**：一条保存下来的 Clash 配置 —— 名字、订阅地址，以及
  **整份正文**，对应 `ClashSubscription`。正文整份进库，备份同步带的就是它。
  按它跑起来的那个 mihomo 子进程叫**实例**（`clash::runtime`），本进程里同时
  只有一个。
- **活动栏（ActivityBar）**：最左边那条固定的窄栏，切换侧边栏视图，底部是全局动作。
- **侧边栏（sidebar）**：活动栏与 Dock 之间那一栏，一次显示一个 `SidebarView`。

## 组件目录

**目录 = 组件类型 = 类型后缀**，三者必须对得上：

| 目录 | 后缀 | 是什么 |
| --- | --- | --- |
| `src/ui/bars/` | `*Bar` | 贴着窗口边缘的固定 chrome，位置不随内容变（`ActivityBar`） |
| `src/ui/views/` | `*View` | 侧边栏里的整块内容，一次显示一个（`HostExplorerView`、`PlaceholderView`） |
| `src/ui/panels/` | `*Panel` | 进 `gpui_kit::component::dock` 的面板，可拖拽、分屏、存布局（`TerminalPanel`、`HostInfoPanel`、`EventLogPanel`…） |
| `src/ui/dialogs/` | `*Dialog` | 盖在窗口中央、要用户答复的一层（`HostFormDialog`） |
| `src/ui/cards/` | `*Card` | 列表或网格里代表一条数据的那一块（`HostCard`） |
| `src/ui/menus/` | `*Menu` | 点开之后浮在内容之上的一列动作（`HostMenu`） |
| `src/ui/elements/` | `*Element` | 自己实现 `gpui_kit::Element`，下到布局与绘制里的那一层（`TerminalGridElement`） |
| `src/ui/windows/` | `*Window` | 自己开一扇原生窗口的外壳，有自己的标题栏与 `Root`（`CredentialVaultWindow`） |

新增一类组件就照这个样子新开一个目录，不要塞进已有目录里凑合：
抽屉 → `drawers/` + `*Drawer`，气泡/浮层 → `popovers/` + `*Popover`。
目录在第一个组件落地时创建，不留空目录。

不是组件的两处：`src/ui/theme/`（主题、配色、图标这些"画法"）
和 `src/ui/workspace.rs`（把上面这些拼起来的窗口外壳）。

界面之外的领域与后端已拆成 `crates/` 下的独立 crate（见【workspace 与 crate 分层】），不再堆在根包 `src/` 下：领域数据在 `winssh-domain`（`Host`、`Credential`、`Session` 读数、命令历史等），终端内核在 `winssh-terminal`，对端资源采集在 `winssh-monitor`，组网在 `winssh-easytier`（含等它就绪的入口登记处 `portal`），Clash 订阅在 `winssh-clash`，各传输/文件后端各一个 crate。根包 `src/` 下只留一组 `pub use winssh_*::*` 兼容桩，`crate::host`、`crate::clash::runtime` 等旧路径仍可用。

数据访问走三层，UI 只认最上面那层（各自已拆成 crate；根包里的 `src/service/`、
`src/repository/`、`src/db/` 是指向对应 crate 的兼容桩）：

| 层 | crate | 职责 |
| --- | --- | --- |
| `winssh-service`（`*Service`） | 根包经 `crate::service::*` | 业务逻辑与领域模型转换，UI 拿数据的唯一入口（`HostService`、`CredentialService`、`MonitorService`） |
| `winssh-repository`（`*Repository`） | 根包经 `crate::repository::*` | 数据仓，纯数据访问，不掺业务规则（`HostRepository`、`CredentialRepository`） |
| `winssh-db` | 根包经 `crate::db::*` | SeaORM 连接、应用级 tokio runtime 与实体定义（`db::entities`） |

规矩：**UI 层一律通过服务层读写数据**，禁止直接触达 SeaORM 或 SQL；
SeaORM 的异步操作跑在 `db::run` 桥接的应用级 tokio runtime 上，
错误以 `anyhow::Result` 抛给调用方决定要不要弹通知。
库只有一座（`db::app()`），各服务共用同一条连接 —— 各开一条的话，
退回内存库时每条连接都是独立的一座空库。

一个文件只放一个组件，文件名与它导出的主类型对应：
`host_explorer.rs` → `HostExplorerView`，`event_log.rs` → `EventLogPanel`，
`host_form.rs` → `HostFormDialog`（目录已经说了是 dialog，文件名不再重复）。
元素 id 用组件名的 kebab-case 前缀，例如 `host-explorer-filter`。

## 参考代码

1. @refs/gpui-kit 是基于 gpui 库封装的三方组件库（原 gpui-component 仓库，已改名并入 gpui-kit），采用了类似 shadcn 的设计模式，是一个综合性的 Rust 桌面应用开发框架。它将生产级 UI 系统、应用级数据与布局能力、编辑能力，以及可复用的行为、状态和基础设施整合在一起。应用侧只依赖 `crates/kit`（`gpui-kit`）这一个门面，它下面：带样式的组件在 `crates/component`，基础能力在 `crates/base`，图标资源在 `crates/assets`；gpui 本体由它替我们钉住版本。新写法照 `refs/gpui-kit/examples/` 来。
2. @design/prototype 是基于前端的产品原型设计，可以参考配色、布局、icon 等
3. @refs/alacritty 是 zed 用的那个 alacritty fork。它和上面那个不一样 ——
   不是拿来参考的，`alacritty_terminal` 是终端内核的真实依赖（VT 解析、
   屏幕缓冲、PTY），rev 与 zed 自己钉的一致。

对于 gpui-kit，我们只参考并使用 `gpui_kit::component`（`crates/component`）里带样式的 ui 组件，对于 base 组件不需要参考。

上述的参考代码库里面不仅有相关的 demo，还有完善的文档可以参考。

`refs/` 下的目录都是本仓库的 git 子模块（浅克隆），
初次检出后执行 `git submodule update --init --recursive` 拉起。
`Cargo.toml` 里 `refs/gpui-kit`（`crates/kit`）与 `refs/alacritty` 是 path 依赖，
缺失时无法编译。gpui 本体不单独列 —— 它从 crates.io 的 `gpui-pre-*` 快照来
（gpui-kit 发的，和它的 component/base 钉在同一个 zed commit 上），
由 `gpui-kit` 一并带进来，自己再列一份就是两份不同的 gpui 类型，编不过。

## 依赖补丁

`vendor/vnc-rs/` 是 **vnc-rs 0.6.0 的本地补丁副本**，由 `Cargo.toml` 末尾的
`[patch.crates-io]` 顶替 crates.io 上那一份，**不是** `refs/` 下的参考代码，
"禁止修改 `refs/` 下的代码"那条规矩管不到它。补丁只有剪贴板一处：扩展剪贴板
（`0xc0a1e5ce`，UTF-8 + zlib）的消息编解码，外加经典正文按字节交给上层 ——
理由与逐文件改动见 `vendor/vnc-rs/PATCHES.md`，应用侧的字符集策略见
`src/vnc/clipboard.rs`。要加第二处补丁前先想清楚：能不改依赖就不改，
改了的每一处都要在那个文件里留下理由。

## WinSSH Pro 项目

WinSSH Pro 是一个使用 rust+cargo+gpui+russh 构建的跨平台、原生高性能 SSH 客户端，为桌面用户提供超 120+fps 的高性能原生渲染体验。

## 项目环境

- 开发语言：rust
- 依赖管理：cargo
- GUI 框架：gpui
- 三方 UI 组件：gpui-component
- SSH+SFTP 核心库：russh

## 开发规则

- 组件化优先原则
- 原生高性能原则
- 内存安全原则
- 任何UI界面的实现必须阅读【参考代码】章节提到的代码库，从里面找可以复用的组件/模块，避免重复造轮子，非UI界面的需求不用参考
- 不同的 UI 组件代码需要分类存放，比如弹窗类、面板类、菜单类等等，需要建立对应的类型目录存放组件代码

## 命名规范

术语跟着设计原型走，一个概念只用一个词：

- **主机（Host）**：一条保存下来的连接配置，对应原型里的 `ServerHost`。分组是 `HostGroup`。
- **会话（Session）**：照着主机打开的那条活着的连接，也就是中心区的一个终端标签页。
- **活动栏（ActivityBar）**：最左边那条固定的窄栏，切换侧边栏视图，底部是全局动作。
- **侧边栏（sidebar）**：活动栏与 Dock 之间那一栏，一次显示一个 `SidebarView`。

## 组件目录

**目录 = 组件类型 = 类型后缀**，三者必须对得上：

| 目录 | 后缀 | 是什么 |
| --- | --- | --- |
| `src/ui/bars/` | `*Bar` | 贴着窗口边缘的固定 chrome，位置不随内容变（`ActivityBar`） |
| `src/ui/views/` | `*View` | 侧边栏里的整块内容，一次显示一个（`HostExplorerView`、`PlaceholderView`） |
| `src/ui/panels/` | `*Panel` | 进 `gpui_component::dock` 的面板，可拖拽、分屏、存布局（`TerminalPanel`、`HostInfoPanel`、`EventLogPanel`…） |
| `src/ui/dialogs/` | `*Dialog` | 盖在窗口中央、要用户答复的一层（`HostFormDialog`） |
| `src/ui/cards/` | `*Card` | 列表或网格里代表一条数据的那一块（`HostCard`） |
| `src/ui/menus/` | `*Menu` | 点开之后浮在内容之上的一列动作（`HostMenu`） |
| `src/ui/elements/` | `*Element` | 自己实现 `gpui::Element`，下到布局与绘制里的那一层（`TerminalGridElement`） |

新增一类组件就照这个样子新开一个目录，不要塞进已有目录里凑合：
抽屉 → `drawers/` + `*Drawer`，气泡/浮层 → `popovers/` + `*Popover`。
目录在第一个组件落地时创建，不留空目录。

不是组件的两处：`src/ui/theme/`（主题、配色、图标这些"画法"）
和 `src/ui/workspace.rs`（把上面这些拼起来的窗口外壳）。

界面之外的领域层直接放在 `src/` 下，不进 `src/ui/`：
`host.rs`（主机配置）、`session.rs`（会话读数）、`terminal/`（终端内核）、
`monitor/`（对端资源采集：采集脚本、解析、速率累加）。

数据访问走三层，UI 只认最上面那层：

| 层 | 目录 | 职责 |
| --- | --- | --- |
| `src/service/` | `*Service` | 业务逻辑与领域模型转换，UI 拿数据的唯一入口（`HostService`、`MonitorService`） |
| `src/repository/` | `*Repository` | 数据仓，纯数据访问，不掺业务规则（`HostRepository`） |
| `src/db/` | — | SeaORM 连接、应用级 tokio runtime 与实体定义（`db/entities/`） |

规矩：**UI 层一律通过服务层读写数据**，禁止直接触达 SeaORM 或 SQL；
SeaORM 的异步操作跑在 `db::run` 桥接的应用级 tokio runtime 上，
错误以 `anyhow::Result` 抛给调用方决定要不要弹通知。

一个文件只放一个组件，文件名与它导出的主类型对应：
`host_explorer.rs` → `HostExplorerView`，`event_log.rs` → `EventLogPanel`，
`host_form.rs` → `HostFormDialog`（目录已经说了是 dialog，文件名不再重复）。
元素 id 用组件名的 kebab-case 前缀，例如 `host-explorer-filter`。

## 参考代码

1. @refs/zed 是 gpui 库的完整实现，里面包含了许多重要的组件等
2. @refs/gpui-component 是基于 gpui 库封装的三方组件库，采用了类似 shadcn 的设计模式，是一个综合性的 Rust 桌面应用开发框架。它将生产级 UI 系统、应用级数据与布局能力、编辑能力，以及可复用的行为、状态和基础设施整合在一起。
3. @design/prototype 是基于前端的产品原型设计，可以参考配色、布局、icon 等
4. @refs/alacritty 是 zed 用的那个 alacritty fork。它和上面两个不一样 ——
   不是拿来参考的，`alacritty_terminal` 是终端内核的真实依赖（VT 解析、
   屏幕缓冲、PTY），rev 与 zed 自己钉的一致。

对于 gpui-component 库，我们只参考并使用带有样式的 ui 组件，对于 base 组件不需要参考。

上述的参考代码库里面不仅有相关的 demo，还有完善的文档可以参考。

`refs/` 下三个目录都是本仓库的 git 子模块（浅克隆），
初次检出后执行 `git submodule update --init --recursive` 拉起；
`Cargo.toml` 的 `[patch]` 与 path 依赖它们存在，缺失时无法编译。

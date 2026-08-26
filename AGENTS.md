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
- gpui-component 是优先级最高的组件库，zed 只做代码参考
- 任何功能组件的实现必须阅读【参考代码】章节提到的代码库，从里面找可以复用的组件/模块，避免重复造轮子

## 命名规范

术语跟着设计原型走，一个概念只用一个词：

- **主机（Host）**：一条保存下来的连接配置，对应原型里的 `ServerHost`。分组是 `HostGroup`。
- **会话（Session）**：照着主机打开的那条活着的连接，也就是中心区的一个终端标签页。
- **活动栏（ActivityBar）**：最左边那条固定的窄栏，切换侧边栏视图，底部是全局动作。
- **侧边栏（sidebar）**：活动栏与 Dock 之间那一栏，一次显示一个 `SidebarView`。

组件后缀表示它是什么，不能混用：

- `*Bar`：窗口边上固定不动的 chrome（`ActivityBar`、状态栏）。
- `*View`：侧边栏里的视图，不进 Dock（`HostExplorerView`、`PlaceholderView`）。
- `*Panel`：进 `gpui_component::dock` 的面板，可拖拽、分屏、保存布局（`TerminalPanel`、`HostInfoPanel`、`EventLogPanel`…）。
- `*Card`：列表里代表一条数据的卡片（`HostCard`）。

文件名与它导出的主类型对应：`host_explorer.rs` → `HostExplorerView`，
`event_log.rs` → `EventLogPanel`。一个文件只放一个组件；同一区域的组件放进同名目录
（`src/ui/sidebar/`、`src/ui/panels/`）。元素 id 用组件名的 kebab-case 前缀，
例如 `host-explorer-filter`。

## 参考代码

1. @refs/zed 是 gpui 库的完整实现，里面包含了许多重要的组件等
2. @refs/gpui-component 是基于 gpui 库封装的三方组件库，采用了类似 shadcn 的设计模式，是一个综合性的 Rust 桌面应用开发框架。它将生产级 UI 系统、应用级数据与布局能力、编辑能力，以及可复用的行为、状态和基础设施整合在一起。
3. @design/prototype 是基于前端的产品原型设计，可以参考配色、布局、icon 等

对于 gpui-component 库，我们只参考并使用带有样式的 ui 组件，对于 base 组件不需要参考。

上述的参考代码库里面不仅有相关的 demo，还有完善的文档可以参考。

`refs/zed` 与 `refs/gpui-component` 都是本仓库的 git 子模块（浅克隆），
初次检出后执行 `git submodule update --init --recursive` 拉起；
`Cargo.toml` 的 `[patch]` 依赖这两个目录存在，缺失时无法编译。

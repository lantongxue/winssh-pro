## WinSSH Pro 项目

WinSSH Pro 是一个使用 rust+cargo+gpui+russh 构建的跨平台、原生高性能 SSH 客户端，为桌面用户提供超 120+fps 的高性能原生渲染体验。

## 项目环境

- 开发语言：rust
- 依赖管理：cargo
- GUI 框架：gpui
- 三方 UI 组件：gpui-component
- SSH 核心库：russh

## 开发规则

- 组件化优先原则
- 原生高性能原则
- 内存安全原则
- 任何功能组件的实现必须阅读【参考代码】章节提到的代码库，从里面找可以复用的组件/模块，避免重复造轮子

## 参考代码

1. @refs/zed 是 gpui 库的完整实现，里面包含了许多重要的组件等
2. @refs/gpui-component 是基于 gpui 库封装的三方组件库，采用了类似 shadcn 的设计模式，是一个综合性的 Rust 桌面应用开发框架。它将生产级 UI 系统、应用级数据与布局能力、编辑能力，以及可复用的行为、状态和基础设施整合在一起。

对于 gpui-component 库，我们只参考带有样式的 ui 组件，对于 base 组件不需要参考。

上述的参考代码库里面不仅有相关的 demo，还有完善的文档可以参考。

`refs/zed` 与 `refs/gpui-component` 都是本仓库的 git 子模块（浅克隆），
初次检出后执行 `git submodule update --init --recursive` 拉起；
`Cargo.toml` 的 `[patch]` 依赖这两个目录存在，缺失时无法编译。

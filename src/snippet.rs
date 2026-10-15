//! 代码片段（snippet）数据模型已下沉到 `winssh-domain`（见 [`winssh_domain::snippet`]）：
//! `Snippet`/`SnippetCategory`/`SnippetVariable`/`SnippetDraft`/`SnippetPreset` 与
//! 变量展开、tags/variables 编解码都在那里。这里只做再导出，保持 `crate::snippet::*`
//! 旧路径不变 —— 落库归 [`crate::service::SnippetService`]。

pub use winssh_domain::snippet::*;

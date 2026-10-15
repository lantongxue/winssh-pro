//! 命令片段的数据模型。
//!
//! 术语按设计原型统一（原型里那一层是 `SnippetVaultView` + `SnippetModal`）：
//! **片段**（[`Snippet`]）是一条存下来的命令或脚本，归在一个**分类**
//! （[`SnippetCategory`]）里，可以带**参数** —— 命令文本里写成 `{{name}}`，
//! 发出去之前由参数对话框逐个填上（[`Snippet::parameters`]）。
//!
//! 片段和会话之间是**发送**关系，不是绑定：填好的那一行原样打进活动会话里，
//! 片段自己不知道对面是哪台机器，也不记谁跑过它。所以一条片段在哪台主机上
//! 都用得上，删掉它也不影响任何一条已经发出去的命令。
//!
//! 这个文件只描述片段本身与围绕它的那几条纯规则（参数怎么认出来、怎么填回去、
//! 搜索怎么算，以及出厂自带的那几份模板），落库归
//! [`crate::service::SnippetService`]，送进终端归
//! [`crate::ui::views::SnippetVaultView`]。

use gpui_kit::SharedString;
use rust_i18n::t;
use serde::{Deserialize, Serialize};

/// 片段的分类，对应原型里那一排筛选片与表单上那个下拉。
///
/// 是固定的七档而不是自由文本（保险箱的分组目录是自由文本）：原型里这一栏
/// 就是个 `select`，而筛选片要的是一份**稳定**的清单 —— 分类跟着数据长出来
/// 的话，那一排片会随手打错的一个字多出一格。落库时存为字符串，
/// 读回时按小写匹配。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SnippetCategory {
    /// 容器与编排：`docker`、`kubectl`。
    #[default]
    Docker,
    /// 系统与运维：磁盘、进程、服务。
    System,
    /// Web 服务：Nginx、证书、站点。
    Web,
    /// 数据库与存储：备份、恢复、连接检查。
    Database,
    /// 安全与防火墙：端口策略、证书有效期。
    Security,
    /// 网络与诊断：连通性、端口占用、抓包。
    Network,
    /// Git 与部署。
    Git,
}

impl SnippetCategory {
    /// 自左向右的排列顺序，也是筛选片与下拉的顺序。
    pub const ALL: [Self; 7] = [
        Self::Docker,
        Self::System,
        Self::Web,
        Self::Database,
        Self::Security,
        Self::Network,
        Self::Git,
    ];

    /// 落库用的那个词。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::System => "system",
            Self::Web => "web",
            Self::Database => "database",
            Self::Security => "security",
            Self::Network => "network",
            Self::Git => "git",
        }
    }

    /// 界面上的名字。
    pub fn label(&self) -> SharedString {
        match self {
            Self::Docker => t!("snippet.category_docker"),
            Self::System => t!("snippet.category_system"),
            Self::Web => t!("snippet.category_web"),
            Self::Database => t!("snippet.category_database"),
            Self::Security => t!("snippet.category_security"),
            Self::Network => t!("snippet.category_network"),
            Self::Git => t!("snippet.category_git"),
        }
        .into()
    }

    /// 这一档在 [`Self::ALL`] 里的下标。表单要用它给下拉定初值。
    pub fn index(&self) -> usize {
        Self::ALL
            .iter()
            .position(|candidate| candidate == self)
            .unwrap_or_default()
    }

    /// 下拉里第几项。越界按缺省档算 —— 界面选不出越界的下标。
    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }

    /// 从库里那一列读回来。认不出的按缺省档算：读的是**已经存下来的数据**，
    /// 一条读不懂的记录不该让整份片段列表打不开。
    pub fn from_str_or_default(raw: &str) -> Self {
        match raw.trim().to_lowercase().as_str() {
            "system" => Self::System,
            "web" | "nginx" => Self::Web,
            "database" | "db" => Self::Database,
            "security" => Self::Security,
            "network" => Self::Network,
            "git" => Self::Git,
            _ => Self::Docker,
        }
    }
}

/// 一个参数：命令里的 `{{name}}`，加上上次存下来的默认值。
///
/// 没有"显示名"这一格：参数对话框上那行标签就写 `{{name}}` 本身 ——
/// 要对得上的是命令文本里的那几个字，另编一个人类可读的名字反而要用户
/// 在两个词之间做一次翻译。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnippetVariable {
    pub name: SharedString,
    /// 默认值，空串就是"每次都得填"。
    #[serde(default)]
    pub default: SharedString,
}

/// 一条命令片段。
#[derive(Clone, Debug)]
pub struct Snippet {
    pub id: SharedString,
    /// 片段名称，例如 `Nginx 配置检查并热重载`。
    pub title: SharedString,
    pub category: SnippetCategory,
    /// 命令本身。可以是多行脚本，里面的 `{{name}}` 是参数。
    pub command: SharedString,
    /// 一句话说清它干什么。可以为空。
    pub description: SharedString,
    pub tags: Vec<SharedString>,
    /// 参数的默认值。
    ///
    /// **只是默认值的存放处，不是参数清单** —— 这条片段这一次要问哪几个参数，
    /// 一律现从 [`Self::command`] 里认（见 [`Self::parameters`]）。命令改过之后
    /// 这里可能留着几个已经不在命令里的名字，也可能少了刚写进去的那个，
    /// 两种情况都不该让参数对话框问错。
    pub variables: Vec<SnippetVariable>,
}

impl Snippet {
    /// 这条片段要问的那几个参数：名字从命令文本里认，默认值从存下来的那一份取。
    pub fn parameters(&self) -> Vec<SnippetVariable> {
        variables_in(&self.command)
            .into_iter()
            .map(|name| SnippetVariable {
                default: self.default_of(&name),
                name,
            })
            .collect()
    }

    /// 某个参数存下来的默认值，没存过就是空串。
    pub fn default_of(&self, name: &str) -> SharedString {
        self.variables
            .iter()
            .find(|variable| variable.name == name)
            .map(|variable| variable.default.clone())
            .unwrap_or_default()
    }

    /// 这条片段发出去之前要不要先问参数。
    pub fn is_parameterized(&self) -> bool {
        !variables_in(&self.command).is_empty()
    }

    /// 列表卡片上那一行命令：第一行，后面还有内容时缀一个省略号。
    ///
    /// 卡片只有一行的位置，而多行脚本的第二行往后正是它和一条单行命令的区别
    /// —— 直接截掉的话，一段五行的部署脚本看起来就只是 `set -e`。
    /// 省略号说的是"这里还有"，完整的内容在表单里。
    pub fn preview(&self) -> SharedString {
        let mut lines = self.command.lines();
        let first = lines.next().unwrap_or_default().trim_end();
        match lines.next().is_some() {
            true => format!("{first} …").into(),
            false => SharedString::from(first.to_string()),
        }
    }

    /// 参数填好之后真正要发出去的那一行。
    pub fn expand(&self, values: &[(SharedString, String)]) -> String {
        expand(&self.command, values)
    }

    /// 搜索框匹配：名称、命令、说明、标签，任意一处命中就算，不分大小写。
    ///
    /// 分类不参与匹配：它有自己那一排筛选片，两处筛同一件事的话，
    /// 搜"docker"会把片选中的那一档又放开。
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        let hay = [&self.title, &self.command, &self.description];
        hay.iter()
            .any(|field| field.to_lowercase().contains(&query))
            || self
                .tags
                .iter()
                .any(|tag| tag.to_lowercase().contains(&query))
    }
}

/// 待入库的一条片段：除 id 与两个时间戳之外的全部字段。
/// id 与时间戳由服务层在落库时分配。
#[derive(Clone, Debug, Default)]
pub struct SnippetDraft {
    pub title: String,
    pub category: SnippetCategory,
    pub command: String,
    pub description: String,
    pub tags: Vec<String>,
    pub variables: Vec<SnippetVariable>,
}

/// 命令文本里的参数名，按出现顺序，去重。
///
/// 只认 `{{name}}` 这一种写法，`name` 限于 ASCII 字母、数字与下划线 ——
/// shell 里 `${VAR}`、`$(cmd)`、`{}`（`find -exec` 的那个）都是命令自己的语法，
/// 认宽了就会把它们当成要用户填的参数。
pub fn variables_in(command: &str) -> Vec<SharedString> {
    let mut names: Vec<SharedString> = Vec::new();
    let mut rest = command;
    while let Some((_, end, name)) = next_placeholder(rest) {
        let name = SharedString::from(name.to_string());
        if !names.contains(&name) {
            names.push(name);
        }
        rest = &rest[end..];
    }
    names
}

/// 把参数填回命令里。
///
/// 值原样填，**空值也照填**：参数对话框上那块预览走的是同一个函数，
/// 所以预览里看见的就是真的要发出去的那一行，不会有"预览还留着 `{{port}}`、
/// 发出去却少了一段"这种事。
///
/// 没在 `values` 里出现的参数原样留着（连 `{{}}` 一起）：那多半是命令刚被改过、
/// 参数表还没跟上，留着比抹掉安全 —— 抹掉会送出一条意思不同的命令。
pub fn expand(command: &str, values: &[(SharedString, String)]) -> String {
    let mut out = String::with_capacity(command.len());
    let mut rest = command;
    while let Some((start, end, name)) = next_placeholder(rest) {
        out.push_str(&rest[..start]);
        match values
            .iter()
            .find(|(candidate, _)| candidate.as_ref() == name)
        {
            Some((_, value)) => out.push_str(value),
            None => out.push_str(&rest[start..end]),
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// 下一个 `{{name}}`：`(起点, 终点, 参数名)`，字节下标。
///
/// `{{` 与 `}}` 都是 ASCII，按字节找到的下标一定落在字符边界上，
/// 所以下面几处切片对多字节的命令文本也是安全的。
fn next_placeholder(text: &str) -> Option<(usize, usize, &str)> {
    let mut at = 0usize;
    while let Some(found) = text[at..].find("{{") {
        let start = at + found;
        let rest = &text[start + 2..];
        // 后面再也没有 `}}` 了，这一段就不是参数，也不会有下一个。
        let close = rest.find("}}")?;
        let name = &rest[..close];
        if is_variable_name(name) {
            return Some((start, start + 2 + close + 2, name));
        }
        // 认不出的那一对花括号跳过开头两个字符再找：`{{{name}}` 这种写法里，
        // 真正的参数从第二个 `{` 开始。
        at = start + 2;
    }
    None
}

fn is_variable_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 把标签写成 `snippets.tags` 那一列。
///
/// 一列存一个列表，编码选 JSON —— 和 [`crate::host::encode_tags`] 同一个理由：
/// 用 sqlite3 打开库文件时那一列还读得懂，而标签里本来就可能有逗号。
pub fn encode_tags(tags: &[SharedString]) -> String {
    if tags.is_empty() {
        return String::new();
    }
    serde_json::to_string(tags).unwrap_or_default()
}

/// 从 `snippets.tags` 那一列读回标签。读不动就当没有。
pub fn decode_tags(raw: &str) -> Vec<SharedString> {
    if raw.trim().is_empty() {
        return Vec::new();
    }
    match serde_json::from_str(raw) {
        Ok(tags) => tags,
        Err(error) => {
            log::warn!("snippets.tags 解析失败，这条片段按没有标签处理：{error}");
            Vec::new()
        }
    }
}

/// 把参数默认值写成 `snippets.variables` 那一列。空列表编成空串 ——
/// 老库补出来的那一列就是空串，两者读回来都是"这条片段没有默认值"。
pub fn encode_variables(variables: &[SnippetVariable]) -> String {
    if variables.is_empty() {
        return String::new();
    }
    serde_json::to_string(variables).unwrap_or_default()
}

/// 从 `snippets.variables` 那一列读回参数默认值。读不动就当没有 ——
/// 参数清单本来就是从命令文本里认的（见 [`Snippet::variables`]），
/// 丢掉的只是几个默认值，那一列坏掉不该让整条片段发不出去。
pub fn decode_variables(raw: &str) -> Vec<SnippetVariable> {
    if raw.trim().is_empty() {
        return Vec::new();
    }
    match serde_json::from_str(raw) {
        Ok(variables) => variables,
        Err(error) => {
            log::warn!("snippets.variables 解析失败，这条片段按没有默认值处理：{error}");
            Vec::new()
        }
    }
}

/// 出厂自带的一份模板，对应原型 `SnippetModal` 顶上那排"快速模板"片。
///
/// 它们**不会**自己写进库里：点一下把表单填好，用户改完再存 —— 这样保险箱
/// 里的每一条都是他自己认过的，而不是装完就先躺着十几条没人用的记录。
pub struct SnippetPreset {
    /// 元素 id 与语言表 key 的后缀。
    pub id: &'static str,
    pub category: SnippetCategory,
    /// 命令本身。不进语言表：它是要发给 shell 的字节，不是给人读的文案。
    pub command: &'static str,
    /// 标签。同上不翻译 —— 这些是工具自己的名字。
    pub tags: &'static [&'static str],
    /// 参数的默认值，`(参数名, 默认值)`。命令里没有参数的模板是空的。
    pub defaults: &'static [(&'static str, &'static str)],
}

impl SnippetPreset {
    /// 片段名称，套用之后就是表单里那一格。
    pub fn title(&self) -> SharedString {
        // key 拼出来先落进一个 binding：`t!` 借着它查表，直接把 `format!` 的
        // 临时值传进去的话，那个 String 在查表之前就已经析构了。
        let key = format!("snippet.preset_{}_title", self.id);
        t!(&key).into()
    }

    /// 说明那一句。
    pub fn description(&self) -> SharedString {
        let key = format!("snippet.preset_{}_description", self.id);
        t!(&key).into()
    }

    /// 套用这份模板：一份可以直接存的草稿。
    pub fn draft(&self) -> SnippetDraft {
        SnippetDraft {
            title: self.title().to_string(),
            category: self.category,
            command: self.command.to_string(),
            description: self.description().to_string(),
            tags: self.tags.iter().map(|tag| tag.to_string()).collect(),
            variables: self
                .defaults
                .iter()
                .map(|(name, default)| SnippetVariable {
                    name: SharedString::from(*name),
                    default: SharedString::from(*default),
                })
                .collect(),
        }
    }
}

/// 七份模板，与原型里那一排片一一对应。
pub const PRESETS: [SnippetPreset; 7] = [
    SnippetPreset {
        id: "docker_prune",
        category: SnippetCategory::Docker,
        command: "docker system prune -a --volumes -f",
        tags: &["docker", "clean", "disk"],
        defaults: &[],
    },
    SnippetPreset {
        id: "docker_logs",
        category: SnippetCategory::Docker,
        command: "docker logs -f --tail={{lines}} {{container}}",
        tags: &["docker", "logs"],
        defaults: &[("lines", "100")],
    },
    SnippetPreset {
        id: "large_files",
        category: SnippetCategory::System,
        command: "find {{directory}} -type f -exec du -Sh {} + | sort -rh | head -n 10",
        tags: &["disk", "find"],
        defaults: &[("directory", "/var")],
    },
    SnippetPreset {
        id: "nginx_reload",
        category: SnippetCategory::Web,
        command: "nginx -t && systemctl reload nginx",
        tags: &["nginx", "reload"],
        defaults: &[],
    },
    SnippetPreset {
        id: "mysql_dump",
        category: SnippetCategory::Database,
        command: "mysqldump -u {{user}} -p {{database}} | gzip > {{database}}_$(date +%F).sql.gz",
        tags: &["mysql", "backup"],
        defaults: &[("user", "root")],
    },
    SnippetPreset {
        id: "listening_ports",
        category: SnippetCategory::Network,
        command: "ss -tulpn | grep LISTEN",
        tags: &["network", "ports"],
        defaults: &[],
    },
    SnippetPreset {
        id: "git_reset",
        category: SnippetCategory::Git,
        command: "git fetch origin && git reset --hard origin/{{branch}}",
        tags: &["git", "deploy"],
        defaults: &[("branch", "main")],
    },
];

#[cfg(test)]
#[path = "tests/snippet.rs"]
mod tests;

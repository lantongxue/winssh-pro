//! 凭据保险箱的数据模型。
//!
//! 术语按设计原型统一（原型里那一层是 `CredentialVaultModal`）：**凭据**
//! （[`Credential`]）是保险箱里的一条记录，可以是一组账号密码、一把 SSH 私钥，
//! 或者一个 API 令牌（[`CredentialKind`]）。它和**主机**是两回事 ——
//! 主机说的是"连哪台机器"，凭据说的是"拿什么身份进去"。
//!
//! 两者的关系是**取用**，不是引用：在主机表单里从保险箱挑一条，把内容填进
//! 表单那几格，存进 `hosts` 表的还是那份内容本身。这一条是有意的 ——
//! 主机配置必须自成一体（见 [`crate::host::Host::key_content`]），存一个指向
//! 保险箱的 id，同步到另一台机器上就是一句空话。保险箱管的是"这些身份我记着"，
//! 主机管的是"这台机器用哪一份"，删掉保险箱里的一条不会让哪台主机连不上。
//!
//! 这个文件只描述凭据本身与围绕它的那几条纯规则（强度怎么算、随机密码怎么生），
//! 落库归 [`crate::service::CredentialService`]，真正生成密钥对归
//! [`crate::ssh::keygen`]。

use gpui_kit::SharedString;
use rand::prelude::*;
use rust_i18n::t;
use serde::{Deserialize, Serialize};

/// 凭据的三档，对应原型左栏那三个分类。
///
/// 落库时存为字符串，读回时按小写匹配。界面上的分档按钮直接铺
/// [`CredentialKind::ALL`]，每个按钮记着自己是哪一档 —— 不拿界面上那行字反查，
/// 那行字会跟着语言变。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CredentialKind {
    /// 账号密码：一个用户名加一段密码。
    #[default]
    Password,
    /// SSH 私钥：私钥本身、可选的公钥、解开它的口令与 SHA256 指纹。
    PrivateKey,
    /// API 令牌 / Secret：一段拿去当身份用的长字符串。
    Token,
}

impl CredentialKind {
    /// 自上而下的排列顺序，也是分档按钮的顺序。
    pub const ALL: [Self; 3] = [Self::Password, Self::PrivateKey, Self::Token];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::PrivateKey => "private_key",
            Self::Token => "token",
        }
    }

    /// 界面上的名字。
    pub fn label(&self) -> SharedString {
        match self {
            Self::Password => t!("credential.kind_password"),
            Self::PrivateKey => t!("credential.kind_private_key"),
            Self::Token => t!("credential.kind_token"),
        }
        .into()
    }

    /// 从库里那一列读回来。认不出的按缺省档算 —— 读的是**已经存下来的数据**，
    /// 一条读不懂的记录不该把整个保险箱打不开。
    pub fn from_str_or_default(raw: &str) -> Self {
        match raw.trim().to_lowercase().as_str() {
            "private_key" | "privatekey" | "key" => Self::PrivateKey,
            "token" | "secret" => Self::Token,
            _ => Self::Password,
        }
    }
}

/// 生成密钥对时能挑的算法，对应原型里那个下拉的三档。
///
/// 三档都是 `ssh-keygen` 真的会生成的东西，不是展示用的标签：
/// [`crate::ssh::keygen`] 拿它去调 `PrivateKey::random`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyAlgorithm {
    /// Ed25519：现代轻量、高安全性，256-bit。新密钥的缺省档。
    #[default]
    Ed25519,
    /// RSA 4096-bit：传统工业标准，兼容性最好，但生成一次要好几秒。
    Rsa4096,
    /// ECDSA P-384：椭圆曲线标准。
    EcdsaP384,
}

impl KeyAlgorithm {
    /// 下拉里自上而下的顺序。
    pub const ALL: [Self; 3] = [Self::Ed25519, Self::Rsa4096, Self::EcdsaP384];

    /// 落库用的那个词。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ed25519 => "ed25519",
            Self::Rsa4096 => "rsa4096",
            Self::EcdsaP384 => "ecdsa-p384",
        }
    }

    /// 徽章与下拉上那行字。
    ///
    /// 不进语言表：这三个是算法自己的名字，两种语言下都是同一个词。
    /// 下拉里跟在后面的那句解释（[`Self::tagline`]）才是翻的那一份。
    pub fn badge(&self) -> &'static str {
        match self {
            Self::Ed25519 => "Ed25519",
            Self::Rsa4096 => "RSA 4096",
            Self::EcdsaP384 => "ECDSA P-384",
        }
    }

    /// 一句话说清这一档是什么，跟在名字后面。
    pub fn tagline(&self) -> SharedString {
        match self {
            Self::Ed25519 => t!("credential.algorithm_ed25519"),
            Self::Rsa4096 => t!("credential.algorithm_rsa4096"),
            Self::EcdsaP384 => t!("credential.algorithm_ecdsa_p384"),
        }
        .into()
    }

    /// 下拉里那一整行：名字 + 解释。
    pub fn label(&self) -> SharedString {
        format!("{} — {}", self.badge(), self.tagline()).into()
    }

    pub fn index(&self) -> usize {
        Self::ALL
            .iter()
            .position(|candidate| candidate == self)
            .unwrap_or_default()
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }

    /// 从库里那一列读回来。空串（导入进来、还没标算法的私钥）落到缺省档。
    pub fn from_str_or_default(raw: &str) -> Self {
        match raw.trim().to_lowercase().as_str() {
            "rsa4096" | "rsa" | "rsa 4096" => Self::Rsa4096,
            "ecdsa-p384" | "ecdsa" | "ecdsa p-384" => Self::EcdsaP384,
            _ => Self::Ed25519,
        }
    }
}

/// 保险箱里的一条凭据。
///
/// 三档共用一个结构而不是三个枚举变体：界面上它们是同一张卡片、同一张表单，
/// 只是露出来的字段不同；分档存在 [`Self::kind`] 里，用不上的那几格是空串。
#[derive(Clone, Debug)]
pub struct Credential {
    pub id: SharedString,
    /// 凭据名称，例如 `生产集群 Root 主管理员`。
    pub name: SharedString,
    pub kind: CredentialKind,
    /// 所属分组目录，例如 `数据库`。空串表示还没归类。
    pub category: SharedString,
    /// 账号 / 用户名。密码档必填，私钥档是"这把钥匙给谁用的"。
    pub username: SharedString,
    /// 密码（`kind == Password` 时有效）。
    ///
    /// 和 [`crate::host::Host::password`] 一样落库为明文，演示环境暂如此；
    /// 生产应走系统钥匙串或加密存储。
    pub password: SharedString,
    /// 私钥算法（`kind == PrivateKey` 时有效）。
    pub algorithm: KeyAlgorithm,
    /// 公钥那一行（`ssh-ed25519 AAAA... comment`）。可以为空 ——
    /// 从别处导进来的私钥不一定带着它。
    pub public_key: SharedString,
    /// 私钥本身那一整份 PEM 文本。
    pub private_key: SharedString,
    /// 解开私钥的口令。空串就是"这把私钥没加密"。
    pub passphrase: SharedString,
    /// `SHA256:...` 形式的公钥指纹，由 [`crate::ssh::keygen`] 算出来。
    pub fingerprint: SharedString,
    /// 令牌（`kind == Token` 时有效）。
    pub token: SharedString,
    pub tags: Vec<SharedString>,
    pub notes: SharedString,
    /// 特别关注。原型左栏有一档只看这些。
    pub favorite: bool,
    /// 上次改动的时间，`YYYY-MM-DD HH:MM:SS`。卡片注脚上印的就是它 ——
    /// 一条口令上次动过是什么时候，是保险箱里为数不多的"该轮换了"的线索。
    ///
    /// 建档时间不在这里：它只是列表的排序键，由 `credentials.created_at`
    /// 那一列在 SQL 里用掉（见 [`crate::repository::CredentialRepository::load_all`]），
    /// 界面上没有它的位置。
    pub updated_at: SharedString,
}

impl Credential {
    /// 卡片副标题那一行：分组目录，私钥再补上算法。
    pub fn subtitle(&self) -> SharedString {
        let category = match self.category.is_empty() {
            true => t!("credential.uncategorized").to_string(),
            false => self.category.to_string(),
        };
        match self.kind {
            CredentialKind::PrivateKey => format!("{category} · {}", self.algorithm.badge()).into(),
            _ => category.into(),
        }
    }

    /// 这条凭据里那段要打码的东西。搜索、复制、显示明文都按这一份算。
    pub fn secret(&self) -> &SharedString {
        match self.kind {
            CredentialKind::Password => &self.password,
            CredentialKind::PrivateKey => &self.private_key,
            CredentialKind::Token => &self.token,
        }
    }

    /// 搜索框匹配：名字、账号、分组、备注、标签，任意一处命中就算。
    /// 秘密本身不参与匹配 —— 拿密码去搜密码没有意义，还会让它在别处露出来。
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        let hay = [&self.name, &self.username, &self.category, &self.notes];
        hay.iter()
            .any(|field| field.to_lowercase().contains(&query))
            || self
                .tags
                .iter()
                .any(|tag| tag.to_lowercase().contains(&query))
    }
}

/// 待入库的一条凭据：除 id 与两个时间戳之外的全部字段。
/// id 与时间戳由服务层在落库时分配，表单不必（也不该）操心。
#[derive(Clone, Debug, Default)]
pub struct CredentialDraft {
    pub name: String,
    pub kind: CredentialKind,
    pub category: String,
    pub username: String,
    pub password: String,
    pub algorithm: KeyAlgorithm,
    pub public_key: String,
    pub private_key: String,
    pub passphrase: String,
    pub fingerprint: String,
    pub token: String,
    pub tags: Vec<String>,
    pub notes: String,
    pub favorite: bool,
}

/// 密码强度的四档。分档而不是直接给颜色：颜色是画法，归
/// [`crate::ui::theme::palette::strength`]。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StrengthLevel {
    /// 一格都没有：这条凭据还没填密码。
    Empty,
    Weak,
    Fair,
    Strong,
    Excellent,
}

impl StrengthLevel {
    pub fn label(&self) -> SharedString {
        match self {
            Self::Empty => t!("credential.strength_empty"),
            Self::Weak => t!("credential.strength_weak"),
            Self::Fair => t!("credential.strength_fair"),
            Self::Strong => t!("credential.strength_strong"),
            Self::Excellent => t!("credential.strength_excellent"),
        }
        .into()
    }
}

/// 一段密码的强度，对应原型的 `calculatePasswordStrength`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PasswordStrength {
    /// 得分，0..=[`PasswordStrength::MAX_SCORE`]。
    pub score: u8,
    pub level: StrengthLevel,
}

impl PasswordStrength {
    /// 满分。六项各得一分：长度过 8、长度过 14，以及大写、小写、数字、符号各一。
    pub const MAX_SCORE: u8 = 6;

    /// 算一段密码的强度。
    ///
    /// 这是一把粗尺子，不是熵的估计：它数的是"有没有把该有的几类字符都用上"，
    /// 而那正是界面上那条彩色小条要表达的意思。真要评一个口令好不好，
    /// 得看字典、看重复模式，那不是这一格能承担的。
    pub fn of(password: &str) -> Self {
        if password.is_empty() {
            return Self {
                score: 0,
                level: StrengthLevel::Empty,
            };
        }

        // 长度按**字符**数，不按字节：一段中文口令的 UTF-8 字节数是字符数的三倍，
        // 按字节算的话四个汉字就顶格了。
        let chars = password.chars().count();
        let mut score = 0u8;
        score += u8::from(chars >= 8);
        score += u8::from(chars >= 14);
        score += u8::from(password.chars().any(|c| c.is_ascii_uppercase()));
        score += u8::from(password.chars().any(|c| c.is_ascii_lowercase()));
        score += u8::from(password.chars().any(|c| c.is_ascii_digit()));
        score += u8::from(password.chars().any(|c| !c.is_ascii_alphanumeric()));

        let level = match score {
            0..=2 => StrengthLevel::Weak,
            3..=4 => StrengthLevel::Fair,
            5 => StrengthLevel::Strong,
            _ => StrengthLevel::Excellent,
        };
        Self { score, level }
    }

    /// 得分占满分的比例，0.0..=1.0。界面上那条小条按它算宽度。
    pub fn ratio(&self) -> f32 {
        self.score as f32 / Self::MAX_SCORE as f32
    }
}

/// 生成随机密码的配方，对应原型生成器里那几个开关。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PasswordRecipe {
    pub length: usize,
    pub upper: bool,
    pub lower: bool,
    pub digits: bool,
    pub symbols: bool,
    /// 排除易混淆字符：`1 l I 0 O o` 以及几个抄起来容易看错的符号。
    /// 要照着屏幕手敲进另一台机器时打开它。
    pub avoid_ambiguous: bool,
}

impl Default for PasswordRecipe {
    fn default() -> Self {
        Self {
            length: 20,
            upper: true,
            lower: true,
            digits: true,
            symbols: true,
            avoid_ambiguous: false,
        }
    }
}

impl PasswordRecipe {
    /// 长度滑杆的上下界。下界 8 是"再短就没必要生成了"，上界 64 之外的长度
    /// 多半会被对端的输入框截断，反而更危险。
    pub const LENGTH_RANGE: (usize, usize) = (8, 64);

    /// 表单里"生成强密码"那个链接用的配方：短一档，不占满整格。
    pub fn inline() -> Self {
        Self {
            length: 18,
            ..Self::default()
        }
    }

    /// 夹回合法长度。滑杆给不出越界的值，但配方也从别处来（默认值、将来的设置项）。
    pub fn clamped(self) -> Self {
        Self {
            length: self
                .length
                .clamp(Self::LENGTH_RANGE.0, Self::LENGTH_RANGE.1),
            ..self
        }
    }

    /// 生成一段密码，对应原型的 `generateSecurePassword`。
    ///
    /// 每一类勾上的字符**保证至少出现一次**：先各取一个，剩下的从合集里随机补，
    /// 最后整体洗一遍牌。不这么做的话，20 位里一个符号都没有的概率并不低，
    /// 而那正是对端密码策略会拒绝的那种。
    ///
    /// 一类都没勾时退回小写字母加数字 —— 这一格永远得吐出一段能用的东西，
    /// 而不是一个空串或一句报错。随机数走 [`rand::rng`]（`ThreadRng`，
    /// 密码学安全），不是那种可预测的伪随机。
    pub fn generate(&self) -> String {
        const UPPER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
        const LOWER: &str = "abcdefghijklmnopqrstuvwxyz";
        const DIGITS: &str = "0123456789";
        const SYMBOLS: &str = "!@#$%^&*()_+-=[]{}|;:,.<>?";
        /// 一眼看不出是哪个的那几个字符，连同抄进配置文件时容易惹事的几个符号。
        const AMBIGUOUS: &str = "IO01l|;:,.";

        let recipe = self.clamped();
        let keep = |set: &str| -> Vec<char> {
            set.chars()
                .filter(|c| !recipe.avoid_ambiguous || !AMBIGUOUS.contains(*c))
                .collect()
        };

        let mut pools: Vec<Vec<char>> = Vec::new();
        if recipe.upper {
            pools.push(keep(UPPER));
        }
        if recipe.lower {
            pools.push(keep(LOWER));
        }
        if recipe.digits {
            pools.push(keep(DIGITS));
        }
        if recipe.symbols {
            pools.push(keep(SYMBOLS));
        }
        // 一类都没勾（或者勾了的那几类被"排除易混淆"清空了）时的退路。
        pools.retain(|pool| !pool.is_empty());
        if pools.is_empty() {
            pools.push(keep(LOWER));
            pools.push(keep(DIGITS));
        }

        let mut rng = rand::rng();
        let all: Vec<char> = pools.iter().flatten().copied().collect();
        let mut out: Vec<char> = pools
            .iter()
            .filter_map(|pool| pool.choose(&mut rng).copied())
            .collect();
        out.truncate(recipe.length);
        while out.len() < recipe.length {
            match all.choose(&mut rng) {
                Some(c) => out.push(*c),
                None => break,
            }
        }
        out.shuffle(&mut rng);
        out.into_iter().collect()
    }
}

/// 把标签写成 `credentials.tags` 那一列。
///
/// 一列存一个列表，编码选 JSON —— 和 [`crate::forward::encode`] 同一个理由：
/// 用 sqlite3 打开库文件时那一列还读得懂，而标签里本来就可能有逗号，
/// 拿分隔符拼会在某个人的标签上碎掉。
pub fn encode_tags(tags: &[SharedString]) -> String {
    if tags.is_empty() {
        return String::new();
    }
    serde_json::to_string(tags).unwrap_or_default()
}

/// 把整个保险箱写成一份备份文本，对应原型的 `handleExportVault`。
///
/// 一个顶层数组，每条凭据一个对象，字段名照 [`Credential`] 的字段来 ——
/// 备份是给人和别的工具看的，不是内部快照，所以按档把用不上的那几格省掉
/// （一条令牌凭据里没有 `private_key` 这回事），而 `kind` 用落库那个词，
/// 换一种语言导出的还是同一份文件。
///
/// 里面是**明文**：一份不能用来重建保险箱的备份没有意义。这一点由调用方
/// 在导出之前对用户说清楚（见 `CredentialVaultView::export_backup`）。
pub fn backup_json(credentials: &[Credential]) -> String {
    let entries: Vec<serde_json::Value> = credentials
        .iter()
        .map(|credential| {
            let mut entry = serde_json::json!({
                "id": credential.id,
                "name": credential.name,
                "kind": credential.kind.as_str(),
                "category": credential.category,
                "username": credential.username,
                "tags": credential.tags,
                "notes": credential.notes,
                "favorite": credential.favorite,
                "updated_at": credential.updated_at,
            });
            // `json!` 出来的顶层一定是对象，这一下取不到 `None`。
            let Some(map) = entry.as_object_mut() else {
                return entry;
            };
            match credential.kind {
                CredentialKind::Password => {
                    map.insert("password".into(), credential.password.as_ref().into());
                }
                CredentialKind::PrivateKey => {
                    map.insert("algorithm".into(), credential.algorithm.as_str().into());
                    map.insert("public_key".into(), credential.public_key.as_ref().into());
                    map.insert("private_key".into(), credential.private_key.as_ref().into());
                    map.insert("passphrase".into(), credential.passphrase.as_ref().into());
                    map.insert("fingerprint".into(), credential.fingerprint.as_ref().into());
                }
                CredentialKind::Token => {
                    map.insert("token".into(), credential.token.as_ref().into());
                }
            }
            entry
        })
        .collect();

    // 缩进过的 JSON：这份文件是要被人打开看的，也可能被手工改一行再导回去。
    // 序列化一个纯数据的数组不会失败，真失败了也得吐出一份合法的空备份，
    // 而不是把一句报错写进用户的 `.json`。
    serde_json::to_string_pretty(&entries).unwrap_or_else(|error| {
        log::warn!("保险箱备份序列化失败，导出的是一份空备份：{error}");
        "[]".to_string()
    })
}

/// 从 `credentials.tags` 那一列读回标签。读不动就当没有 ——
/// 为一列坏掉的 JSON 让整个保险箱打不开是不成比例的。
pub fn decode_tags(raw: &str) -> Vec<SharedString> {
    if raw.trim().is_empty() {
        return Vec::new();
    }
    match serde_json::from_str(raw) {
        Ok(tags) => tags,
        Err(error) => {
            log::warn!("credentials.tags 解析失败，这条凭据按没有标签处理：{error}");
            Vec::new()
        }
    }
}

#[cfg(test)]
#[path = "tests/credential.rs"]
mod tests;

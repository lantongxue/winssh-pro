//! 卡片（`*Card`）：列表或网格里代表一条数据的那一块。
//!
//! 卡片自己不持有状态，也不知道自己在哪个列表里：数据和状态由调用方传进来，
//! 点击、菜单这些都以回调发回去。所以同一张卡片可以出现在侧边栏的列表里，
//! 将来也可以出现在别的地方。

mod credential_card;
mod host_card;

pub use credential_card::{
    CredentialAction, CredentialCard, CredentialField, kind_icon, kind_tone,
};
pub use host_card::{HOST_CARD_HEIGHT, HostCard};

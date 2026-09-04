//! 外观基础：主题模式、品牌色与图标。
//!
//! 这个目录里没有组件，只有组件用来画自己的那套东西。分成三块：
//!
//! - [`appearance`]：深浅主题跟不跟随系统；
//! - [`palette`]：主题给不出的品牌语义色与表面层级；
//! - [`ansi`]：终端网格那一套 ANSI 配色；
//! - [`icons`]：组件库图标集里缺的那几个，以及各操作系统的标识。

pub mod ansi;
pub mod appearance;
pub mod icons;
pub mod palette;

pub use icons::{AppIcon, OsIcon};

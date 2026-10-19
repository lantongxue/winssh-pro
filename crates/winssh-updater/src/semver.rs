//! 版本号比较，规则与云函数 `cloud-functions/update.go` 的 `compareSemver` 一致。
//!
//! 只覆盖本仓库用到的部分：`major.minor.patch` 加可选的预发布段
//! （`0.1.0-beta.1`），忽略 build metadata。不是完整的 semver 库 —— 版本号是
//! 我们自己打的，够用即可。
//!
//! 服务端已经判断过"要不要更新"，客户端这一份还有两处用处：`--dev` 构建
//! 判断自己该不该检查，以及测试里钉住比较规则与服务端一致。

use std::cmp::Ordering;
use std::fmt;

/// 解析后的版本号。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    /// 预发布段。空表示正式版。
    pre: Vec<String>,
}

impl Version {
    /// 解析 `0.0.6` / `v0.1.0-beta.1` / `1.2.3+build`。认不出来返回 `None`。
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim().trim_start_matches('v');
        // build metadata 不参与比较。
        let raw = raw.split('+').next().unwrap_or(raw);
        let (core, pre) = match raw.split_once('-') {
            Some((core, pre)) => (core, pre.split('.').map(str::to_owned).collect()),
            None => (raw, Vec::new()),
        };
        let mut parts = core.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
            pre,
        })
    }

    /// 是不是预发布版（`-dev`、`-beta` 之类）。
    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            write!(f, "-{}", self.pre.join("."))?;
        }
        Ok(())
    }
}

fn compare_numeric(a: &str, b: &str) -> Option<Ordering> {
    let a: u64 = a.parse().ok()?;
    let b: u64 = b.parse().ok()?;
    Some(a.cmp(&b))
}

/// 比较两个已解析的版本号，规则照 semver 2.0.0 第 11 条。
pub fn compare(a: &Version, b: &Version) -> Ordering {
    a.major
        .cmp(&b.major)
        .then(a.minor.cmp(&b.minor))
        .then(a.patch.cmp(&b.patch))
        .then_with(|| match (a.pre.is_empty(), b.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            // 正式版高于任何预发布版。
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            // 逐项比：数字按数值、其它按字典序，前缀相同则段数少的更小。
            (false, false) => {
                for (x, y) in a.pre.iter().zip(b.pre.iter()) {
                    let ordering = match (compare_numeric(x, y), compare_numeric(y, x)) {
                        (Some(o), _) => o,
                        (None, Some(_)) => Ordering::Greater,
                        (None, None) => x.cmp(y),
                    };
                    if ordering != Ordering::Equal {
                        return ordering;
                    }
                }
                a.pre.len().cmp(&b.pre.len())
            }
        })
}

#[cfg(test)]
#[path = "tests/updater/semver.rs"]
mod tests;

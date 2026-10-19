#!/usr/bin/env bash
# 把版本号写进 Cargo.toml 与 Cargo.lock —— 发版时由 CI 在 checkout 之后、
# 构建之前调一次，参数就是 tag 去掉 `v` 的那段（如 `0.0.6`）。
#
# 为什么要在 CI 里改、而不是把版本号提交回 main：
#
# * tag 是打在**改版本号之前**的那个提交上的。若 CI 把改动提交回 main，
#   tag 里的代码自己还写着旧版本 —— 拿着 tag 检出的人看到的版本号是错的。
# * 机器人要往 main 推提交就得有写权限，还可能反过来再触发一次 workflow。
#
# 而把改动留在当次 checkout 里，下游一切读版本号的地方自动跟着对上：
# `env!("CARGO_PKG_VERSION")`（设置 → 关于）、winres 写进 exe 的
# FileVersion、bundle-macos.sh 改的 Info.plist、dmg 文件名、cargo-deb /
# cargo-generate-rpm 打出来的包元数据。仓库里 Cargo.toml 的版本号从此只
# 影响本地构建。
#
# 用法：
#   scripts/set-version.sh 0.0.6            # 正式发版
#   scripts/set-version.sh 0.0.5-dev.abc1234 # 调试构建（预发布段是合法的 semver）
#
# 顺带修一个历史遗留：`Cargo.lock` 里 `[[package]] name = "winssh-pro"`
# 那一段的版本号是单独的，`cargo build --locked` 会拿它跟 Cargo.toml 对账，
# 只改 Cargo.toml 会直接报 "the lock file needs to be updated"。
set -euo pipefail

VERSION="${1:?usage: $0 <version>}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# 只认 ASCII 的 semver 主体，避免把引号、换行之类的东西写进 TOML / lock。
if [[ ! "${VERSION}" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.~-]+)?$ ]]; then
    echo "Invalid version: ${VERSION}" >&2
    exit 1
fi

python3 - "${ROOT}/Cargo.toml" "${ROOT}/Cargo.lock" "${VERSION}" <<'PY'
import sys

cargo_toml, cargo_lock, version = sys.argv[1:4]


def stamp_version(lines, section_header, section_pattern):
    """在 section_header 那一节里把第一行 version 换掉，返回 (lines, 是否改到)。

    按行扫，不用正则跨行匹配 —— Cargo.toml 有一万多行，一个写歪的惰性
    重复（`(?:.*\\n)*?`）就会在长文件上回溯到天荒地老。
    """
    in_section = False
    for i, line in enumerate(lines):
        stripped = line.strip()
        if stripped == section_header:
            in_section = True
            continue
        # 下一节（或下一个 [[package]]）开始了还没碰到 version，就是没有。
        if in_section and stripped.startswith("["):
            return lines, False
        if in_section and section_pattern(stripped):
            lines[i] = 'version = "%s"\n' % version
            return lines, True
    return lines, False


def is_version_line(line):
    return line.startswith("version = \"")


# Cargo.toml：只改 `[package]` 段里第一个 version 行。各成员 crate 的
# 版本号是 0.1.0 且与本包无关，别动到它们。
with open(cargo_toml, encoding="utf-8") as f:
    lines = f.readlines()
lines, ok = stamp_version(lines, "[package]", is_version_line)
if not ok:
    sys.exit("could not find [package] version in Cargo.toml")
with open(cargo_toml, "w", encoding="utf-8") as f:
    f.writelines(lines)

# Cargo.lock：找 `[[package]]` + `name = "winssh-pro"` 那一块。先定位 name
# 行，再在它之后、下一个 `[[package]]` 之前找 version。
with open(cargo_lock, encoding="utf-8") as f:
    lines = f.readlines()
name_ix = None
for i, line in enumerate(lines):
    if line.strip() == 'name = "winssh-pro"':
        name_ix = i
        break
if name_ix is None:
    sys.exit('could not find name = "winssh-pro" in Cargo.lock')
for i in range(name_ix + 1, len(lines)):
    stripped = lines[i].strip()
    if stripped.startswith("[["):
        break
    if is_version_line(stripped):
        lines[i] = 'version = "%s"\n' % version
        break
else:
    sys.exit('could not find version for winssh-pro in Cargo.lock')
with open(cargo_lock, "w", encoding="utf-8") as f:
    f.writelines(lines)
PY

echo "Stamped version ${VERSION} into Cargo.toml and Cargo.lock"

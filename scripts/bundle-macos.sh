#!/usr/bin/env bash
set -euo pipefail

# 编译打包 macOS 原生 .app bundle 并自动集成图标与 Info.plist
TARGET="${1:-}"
APP_NAME="WinSSH Pro"
BUNDLE_DIR="target/${TARGET:+${TARGET}/}release/${APP_NAME}.app"
CONTENTS_DIR="${BUNDLE_DIR}/Contents"
MACOS_DIR="${CONTENTS_DIR}/MacOS"
RESOURCES_DIR="${CONTENTS_DIR}/Resources"

BIN_PATH="target/${TARGET:+${TARGET}/}release/winssh-pro"

if [[ ! -f "${BIN_PATH}" ]]; then
    echo "Binary not found at ${BIN_PATH}, building release binary..."
    if [[ -n "${TARGET}" ]]; then
        cargo build --release --target "${TARGET}"
    else
        cargo build --release
    fi
fi

echo "Creating macOS application bundle at: ${BUNDLE_DIR}"
rm -rf "${BUNDLE_DIR}"
mkdir -p "${MACOS_DIR}" "${RESOURCES_DIR}"

# 1. 拷贝二进制
cp "${BIN_PATH}" "${MACOS_DIR}/winssh-pro"
chmod +x "${MACOS_DIR}/winssh-pro"

# 1.5 拷贝 mihomo 内核与 privileged helper（broker）。
#    mihomo 该放的位置没有强制约定，跟主 exe 同目录就行（`binary_path` 找
#    的就是 exe 旁边）。helper 的位置是 macOS  privileged helper 的约定
#    （`Contents/Library/LaunchServices/`）—— `broker_binary_path` 在
#    打包出来的 `.app` 里就是去那里找它。没编过 mihomo / broker 也无所谓：
#    应用自己会说"没找到 mihomo / broker"。
MIHOMO_BIN="target/${TARGET:+${TARGET}/}release/mihomo"
BROKER_BIN="target/${TARGET:+${TARGET}/}release/winssh-broker"
if [[ -f "${MIHOMO_BIN}" ]]; then
    cp "${MIHOMO_BIN}" "${MACOS_DIR}/mihomo"
    chmod +x "${MACOS_DIR}/mihomo"
fi
LAUNCH_SERVICES_DIR="${CONTENTS_DIR}/Library/LaunchServices"
if [[ -f "${BROKER_BIN}" ]]; then
    mkdir -p "${LAUNCH_SERVICES_DIR}"
    cp "${BROKER_BIN}" "${LAUNCH_SERVICES_DIR}/winssh-broker"
    chmod +x "${LAUNCH_SERVICES_DIR}/winssh-broker"
fi

# 2. 拷贝 Info.plist，版本号从 Cargo.toml 覆盖进去：plist 里手写的那份是兜底，
#    不改代码就发版的话「关于」框会一直停在旧版本号上。
cp assets/macos/Info.plist "${CONTENTS_DIR}/Info.plist"
VERSION="$(sed -n 's/^version = "\([^"]*\)".*/\1/p' Cargo.toml | head -n1)"
plutil -replace CFBundleShortVersionString -string "${VERSION}" "${CONTENTS_DIR}/Info.plist"
plutil -replace CFBundleVersion -string "${VERSION}" "${CONTENTS_DIR}/Info.plist"

# 3. 拷贝图标
cp assets/app-icon.icns "${RESOURCES_DIR}/app-icon.icns"

echo "✅ Successfully built ${APP_NAME}.app with application icon!"

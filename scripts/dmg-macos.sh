#!/usr/bin/env bash
set -euo pipefail

# 把 .app 打成可分发的 .dmg —— 挂载后就是「把图标拖到 Applications」那一屏。
#
#   ./scripts/dmg-macos.sh                        # 本机架构，直接复用 target/release 的产物
#   ./scripts/dmg-macos.sh aarch64-apple-darwin   # 指定 cargo target（产物在 target/<target>/release）
#
# 只用系统自带的 hdiutil，不依赖 create-dmg / brew：卷里放 App 本身，再加一个指向
# /Applications 的符号链接，Finder 挂载时自然把它画成「Applications」别名，用户拖一下
# 就装好了 —— 拖拽安装的全部魔法就在这一个软链上。
#
# 想要背景图、图标对齐那种漂亮版式，`brew install create-dmg` 换掉下面那一步
# `hdiutil create` 即可；分发本身不需要它。
#
# 签名：没证书时只补一次 ad-hoc 整包签名（下面 SIGN_IDENTITY 那段），把 Info.plist
# 与 Resources 封进 CodeResources —— linker 只签了二进制，那份签名过不了
# `codesign --verify`（"code has no resources but signature indicates they must be
# present"）。但 ad-hoc 终究不是 Developer ID：浏览器下载下来 Gatekeeper 还是会拦
# （右键「打开」可过），要彻底免手动就得买苹果开发者账号、走下面那条公证分支。
APP_NAME="WinSSH Pro"
TARGET="${1:-}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

# 版本跟着 Cargo.toml 走，别让 dmg 文件名和「关于」框各说一套。
VERSION="$(sed -n 's/^version = "\([^"]*\)".*/\1/p' Cargo.toml | head -n1)"

# 架构后缀沿用 release workflow 的叫法（arm64 / x64），不传 target 时按本机猜。
if [[ -n "${TARGET}" ]]; then
    case "${TARGET}" in
        aarch64-apple-darwin) ARCH="arm64" ;;
        x86_64-apple-darwin) ARCH="x64" ;;
        *) ARCH="${TARGET}" ;;
    esac
else
    case "$(uname -m)" in
        arm64) ARCH="arm64" ;;
        x86_64) ARCH="x64" ;;
        *) ARCH="$(uname -m)" ;;
    esac
fi

APP_DIR="target/${TARGET:+${TARGET}/}release/${APP_NAME}.app"
DIST_DIR="dist"
DMG_NAME="winssh-pro-${VERSION}-macos-${ARCH}"
DMG_PATH="${DIST_DIR}/${DMG_NAME}.dmg"

# 1. .app bundle（bundle-macos.sh 会按需先 cargo build --release）
./scripts/bundle-macos.sh "${TARGET}"

# 2. 签名。有证书走 Developer ID（hardened runtime + secure timestamp 是公证的前置
#    条件）；没证书就把 linker 那份只覆盖二进制的 ad-hoc 签名换成封住整个 bundle 的。
#    bundle 里没有嵌套代码（就一个二进制），签 .app 一层就够，不用 --deep。
if [[ -n "${SIGN_IDENTITY:-}" ]]; then
    echo "Signing ${APP_DIR} as: ${SIGN_IDENTITY}"
    codesign --force --options runtime --timestamp --sign "${SIGN_IDENTITY}" "${APP_DIR}"
else
    codesign --force --sign - "${APP_DIR}"
fi
codesign --verify --verbose=2 "${APP_DIR}"

# 3. 铺卷内容：App + Applications 软链。用 mktemp 的独立暂存目录，
#    免得软链落进 target/release 里被 tar / 增量构建反复带走。
STAGING="$(mktemp -d "${TMPDIR:-/tmp}/winssh-dmg.XXXXXX")"
trap 'rm -rf "${STAGING}"' EXIT
cp -R "${APP_DIR}" "${STAGING}/"
ln -s /Applications "${STAGING}/Applications"

# 4. 打盘像。UDZO = 压缩的只读镜像；文件系统显式选 HFS+ 而不是默认值 ——
#    Info.plist 声明支持到 10.15，APFS 在那台最低版本上未必挂得动。
mkdir -p "${DIST_DIR}"
echo "Creating disk image at: ${DMG_PATH}"
hdiutil create \
    -volname "${APP_NAME}" \
    -srcfolder "${STAGING}" \
    -fs HFS+ \
    -ov \
    -format UDZO \
    "${DMG_PATH}"

# 5. 签 dmg 本体（同样只在有证书时），并按需公证 + 装订票据。
#    notarytool 的凭据从环境变量读：APPLE_ID / APPLE_APP_SPECIFIC_PASSWORD / APPLE_TEAM_ID。
if [[ -n "${SIGN_IDENTITY:-}" ]]; then
    codesign --force --timestamp --sign "${SIGN_IDENTITY}" "${DMG_PATH}"
    if [[ -n "${APPLE_ID:-}" && -n "${APPLE_APP_SPECIFIC_PASSWORD:-}" && -n "${APPLE_TEAM_ID:-}" ]]; then
        xcrun notarytool submit "${DMG_PATH}" \
            --apple-id "${APPLE_ID}" \
            --password "${APPLE_APP_SPECIFIC_PASSWORD}" \
            --team-id "${APPLE_TEAM_ID}" \
            --wait
        xcrun stapler staple "${DMG_PATH}"
    else
        echo "SIGN_IDENTITY set but no notary credentials (APPLE_ID/APPLE_APP_SPECIFIC_PASSWORD/APPLE_TEAM_ID); skipping notarization."
    fi
fi

# 6. 校验一遍镜像的 CRC，别把半坏的 dmg 发出去。
hdiutil verify "${DMG_PATH}" >/dev/null

echo "✅ Successfully built ${DMG_PATH} ($(du -h "${DMG_PATH}" | cut -f1))"

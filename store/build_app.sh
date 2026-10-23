#!/bin/zsh
# 把 cargo 编出来的两个二进制装成 macOS .app，按目标渠道签名并打包。
#
#   ./store/build_app.sh mas     → 沙箱 + Apple Distribution，出 .pkg（上架用）
#   ./store/build_app.sh dev     → 非沙箱 + Developer ID，出 .zip（直供用）
#
# 两个渠道的差异只有三处：entitlements、签名身份、容器格式。
# 其余（图标、Info.plist、版本号来源）共用，避免两版悄悄跑偏。
set -euo pipefail

CHANNEL="${1:-}"
if [[ "$CHANNEL" != "mas" && "$CHANNEL" != "dev" ]]; then
  echo "用法：$0 {mas|dev}" >&2
  exit 2
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

BUNDLE_ID_MAS="app.molt.mac"
BUNDLE_ID_DEV="app.molt.mac"     # 两渠道同 ID，便于将来从直供转上架时数据不分裂
BUILD_DIR="target/dist"
APP="$BUILD_DIR/收.app"

VERSION="$(awk -F'"' '/^version[[:space:]]*=[[:space:]]*"/{print $2; exit}' Cargo.toml)"
if [[ -z "$VERSION" ]]; then
  echo "读不到 Cargo.toml 里的 version" >&2
  exit 1
fi
# CFBundleVersion 必须是纯数字点分（App Store 会拒带字母的），
# 而 cargo 的版本号可能带 -beta。这里把预发布后缀折成 0，主版本原样保留。
BUILD="$(echo "$VERSION" | sed -E 's/[^0-9.].*$//; s/\.+$//')"
[[ "$BUILD" == *.* ]] || BUILD="$BUILD.0"

echo "版本 $VERSION（build $BUILD）  渠道 $CHANNEL"

# ---- 编译 ---------------------------------------------------------------
# 直供版要公证，公证只接受 arm64+x86_64 双架构或至少与声明一致；
# 本机是 arm64，这里只出 arm64，并在 README 里写清楚不支持 Intel。
cargo build --release -p molt-gui -p molt-cli

# ---- 组装 bundle --------------------------------------------------------
rm -rf "$BUILD_DIR"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp target/release/molt-gui "$APP/Contents/MacOS/molt-gui"
# 命令行版一起放进去：它和 GUI 共用同一份判定代码，
# 用户要写脚本时不必再另装一次，macOS 上也符合「一个 App 一个包里自包含」。
cp target/release/molt "$APP/Contents/MacOS/molt"
cp store/icon.icns "$APP/Contents/Resources/AppIcon.icns"

BUNDLE_ID="$BUNDLE_ID_MAS"
[[ "$CHANNEL" == "dev" ]] && BUNDLE_ID="$BUNDLE_ID_DEV"

sed -e "s|__VERSION__|$VERSION|g" \
    -e "s|__BUILD__|$BUILD|g" \
    -e "s|__BUNDLE_ID__|$BUNDLE_ID|g" \
    store/bundle/Info.plist > "$APP/Contents/Info.plist"

plutil -lint "$APP/Contents/Info.plist" > /dev/null

# ---- 签名 ---------------------------------------------------------------
if [[ "$CHANNEL" == "mas" ]]; then
  ENTITLEMENTS="store/bundle/molt-mas.entitlements"
  IDENTITY="$(security find-identity -v -p codesigning \
    | awk -F'"' '/Apple Distribution/{print $2; exit}')"
  if [[ -z "$IDENTITY" ]]; then
    echo "找不到 Apple Distribution 证书。打开 Xcode → Settings → Accounts 生成，或到 developer.apple.com 下载 .cer 后双击导入。" >&2
    exit 1
  fi
  # MAS 版交给商店加固，这里不加 --options runtime（与沙箱同开偶发启动失败）。
  SIGN_ARGS=(--timestamp)
else
  ENTITLEMENTS="store/bundle/molt-dev.entitlements"
  IDENTITY="$(security find-identity -v -p codesigning \
    | awk -F'"' '/(Developer ID Application)/{print $2; exit}')"
  if [[ -z "$IDENTITY" ]]; then
    echo "找不到 Developer ID Application 证书。这是与 Apple Distribution 不同的一张，需要单独申请。" >&2
    echo "只有 Apple Distribution 时，只能走 MAS 渠道：$0 mas" >&2
    exit 1
  fi
  SIGN_ARGS=(--options runtime --timestamp)
fi

echo "签名身份：$IDENTITY"

# 由内到外签：先两个可执行文件，再整个 bundle。
# 反过来签会破坏外层签名，codesign --verify 会报 resource envelope 不匹配。
for exe in "$APP/Contents/MacOS/molt-gui" "$APP/Contents/MacOS/molt"; do
  codesign --force --sign "$IDENTITY" \
    --entitlements "$ENTITLEMENTS" \
    "${SIGN_ARGS[@]}" \
    --identifier "$BUNDLE_ID" \
    "$exe"
done

codesign --force --sign "$IDENTITY" \
  --entitlements "$ENTITLEMENTS" \
  "${SIGN_ARGS[@]}" \
  "$APP"

codesign --verify --deep --strict --verbose=2 "$APP"
echo "--- 签名检查通过 ---"
codesign -dv --entitlements :- "$APP" 2>&1 | grep -E 'Identifier|Authority|flags|sandbox|home-relative' || true

# ---- 出包 ---------------------------------------------------------------
if [[ "$CHANNEL" == "mas" ]]; then
  INSTALLER_ID="$(security find-identity -v -p basic \
    | awk -F'"' '/3rd Party Mac Developer Installer|Mac Installer Distribution/{print $2; exit}')"
  if [[ -z "$INSTALLER_ID" ]]; then
    echo "找不到 Mac Installer Distribution 证书，无法生成 .pkg。" >&2
    exit 1
  fi
  PKG="$BUILD_DIR/收-$VERSION.pkg"
  # --component 直接包 App；--sign 用安装器证书，与 App 的签名证书必须同一团队。
  # 注意：productsign/productbuild 签 pkg 走的是 CSSM 老接口，比 codesign 更挑钥匙串 ACL。
  # 若私钥的 partition list 没放开，会报 errSecInternalComponent(-2070) 或
  # EPERM 100001，而同一张证书用 codesign / security cms 却是好的。
  if ! productbuild --component "$APP" /Applications --sign "$INSTALLER_ID" "$PKG" 2>/tmp/molt_pkg_sign.log; then
    echo ""
    echo "!! .pkg 签名失败，先出未签名包，签名由 tools/sign_pkg.sh 单独完成。"
    echo "   原因：$(grep -oE 'errSecInternalComponent \(-2070\)|EPERM: Operation not permitted|100001' /tmp/molt_pkg_sign.log | head -1)"
    productbuild --component "$APP" /Applications "$PKG"
    echo ""
    echo "   修法（只需做一次，要你本人输入登录钥匙串的密码）："
    echo "     ./store/tools/sign_pkg.sh --setup    # 放开私钥的 partition list"
    echo "     ./store/tools/sign_pkg.sh            # 再签一次"
  else
    pkgutil --check-signature "$PKG"
  fi
  echo "产物：$PKG"
else
  ZIP="$BUILD_DIR/收-$VERSION.zip"
  # ditto 保存资源分支与签名，zip 命令会破坏 .app 的签名。
  ditto -c -k --keepParent "$APP" "$ZIP"
  echo "产物：$ZIP"
  echo "下一步（公证需要你的 Apple ID 应用专用密码或 API 密钥）："
  echo "  xcrun notarytool submit \"$ZIP\" --keychain-profile <profile> --wait"
  echo "  xcrun stapler staple \"$APP\""
fi

#!/bin/zsh
# 给已生成的 .pkg 签名，以及修「私钥 ACL 没放开导致 productsign 失败」。
#
#   ./store/tools/sign_pkg.sh --setup   # 一次性：放开登录钥匙串里 Apple 开发证书的 partition list
#   ./store/tools/sign_pkg.sh           # 签 target/dist 下最新的 .pkg
#
# 为什么单独成脚本：productsign 走 CSSM，比 codesign 更挑私钥 ACL。
# 缺 partition list 时它报 errSecInternalComponent(-2070) 或
# EPERM 100001，而同一张证书 codesign / security cms 都正常——
# 这个组合很容易被误判成证书坏了，实际只是 ACL。
#
# set-key-partition-list 必须带登录钥匙串的密码，且要交互输入。
# 所以这一步只能你本人跑，不要把它塞进 CI（CI 里改用临时钥匙串）。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

KEYCHAIN="${KEYCHAIN:-$HOME/Library/Keychains/login.keychain-db}"
PKG="$(ls -t target/dist/*.pkg 2>/dev/null | head -1 || true)"

if [[ "${1:-}" == "--setup" ]]; then
  echo "放开 Apple 开发证书私钥的 partition list（会提示输入登录钥匙串密码）。"
  echo "钥匙串：$KEYCHAIN"
  echo ""
  # -S 列出允许使用私钥的工具；-s 静默（不打印一堆属性）。
  # 不带 -k 会要求交互输入密码，这正是我们要的：密码不落盘、不进 shell 历史。
  security set-key-partition-list \
    -S apple-tool:,apple:,codesign: \
    -s "$KEYCHAIN"
  echo ""
  echo "完成。现在可以签名了：$0"
  exit 0
fi

if [[ -z "$PKG" ]]; then
  echo "target/dist 下没有 .pkg，先跑 ./store/build_app.sh mas" >&2
  exit 1
fi

INSTALLER_ID="$(security find-identity -v -p basic \
  | awk -F'"' '/3rd Party Mac Developer Installer|Mac Installer Distribution/{print $2; exit}')"
if [[ -z "$INSTALLER_ID" ]]; then
  echo "钥匙串里没有 Mac Installer Distribution 证书。" >&2
  exit 1
fi

SIGNED="${PKG%.pkg}-signed.pkg"
echo "签名：$PKG"
echo "身份：$INSTALLER_ID"

if ! productsign --sign "$INSTALLER_ID" "$PKG" "$SIGNED" 2>&1; then
  echo "" >&2
  echo "签名仍失败。先跑一次：$0 --setup" >&2
  exit 1
fi

pkgutil --check-signature "$SIGNED"
# 签名后的包替换原包，后续上传只认一个路径，免得传错。
mv -f "$SIGNED" "$PKG"
echo "已签名：$PKG"

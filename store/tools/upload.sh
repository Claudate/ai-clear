#!/bin/zsh
# 把已签名的 .pkg 传到 App Store Connect（等价于 Xcode Organizer 的 Distribute App）。
#
#   ./store/tools/upload.sh              # 上传 target/dist 下最新的 .pkg
#   ./store/tools/upload.sh --check      # 只做前置检查，不真的传
#
# 凭据三种给法，任选其一（按优先级）：
#
#   1) App Store Connect API 密钥（推荐，可进 CI，不用 Apple ID 密码）
#        export ASC_API_KEY_ID=XXXXXXXXXX
#        export ASC_API_ISSUER_ID=xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
#        # 把 AuthKey_<KEY_ID>.p8 放到 ~/.appstoreconnect/private_keys/
#
#   2) 已经在钥匙串里存过 notarytool/altool 的 profile
#        xcrun notarytool store-credentials molt-asc --apple-id ... --team-id HA7KN8S95K
#
#   3) Apple ID + 应用专用密码（最不推荐，密码会出现在环境变量里）
#        export ASC_APPLE_ID=you@example.com
#        export ASC_APP_PASSWORD=abcd-efgh-ijkl-mnop
#
# 注意：本脚本只负责「把二进制传上去」。商店页文案、截图、App 隐私、价格、
# 分级这些属于另一路，走 fastlane deliver（见 store/fastlane/Fastfile）。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

TRANSPORTER="/Applications/Transporter.app/Contents/itms/bin/iTMSTransporter"
CHECK_ONLY=0
[[ "${1:-}" == "--check" ]] && CHECK_ONLY=1

PKG="$(ls -t target/dist/*.pkg 2>/dev/null | head -1 || true)"
[[ -n "$PKG" ]] || { echo "target/dist 下没有 .pkg，先跑 ./store/build_app.sh mas" >&2; exit 1; }

# ---- 前置检查 -----------------------------------------------------------
fail=0
note() { printf '  %-34s %s\n' "$1" "$2"; }

echo "=== 前置检查 ==="

if [[ -x "$TRANSPORTER" ]]; then
  note "Transporter" "OK（$(basename "$(dirname "$(dirname "$TRANSPORTER")")")）"
else
  note "Transporter" "缺失 —— 从 Mac App Store 安装 Transporter.app"
  fail=1
fi

# 包必须已签名：未签名的 pkg 传到一半会被 Apple 拒，白等十几分钟。
if pkgutil --check-signature "$PKG" >/dev/null 2>&1; then
  note "pkg 签名" "OK"
else
  note "pkg 签名" "未签 —— 先跑 ./store/tools/sign_pkg.sh"
  fail=1
fi

note "待传文件" "$(basename "$PKG")  $(du -h "$PKG" | cut -f1)"

# 认证三选一
AUTH=""
if [[ -n "${ASC_API_KEY_ID:-}" && -n "${ASC_API_ISSUER_ID:-}" ]]; then
  KEY_FILE="$(ls "$HOME/.appstoreconnect/private_keys/AuthKey_${ASC_API_KEY_ID}.p8" \
                 "$HOME/.private_keys/AuthKey_${ASC_API_KEY_ID}.p8" 2>/dev/null | head -1 || true)"
  if [[ -n "$KEY_FILE" ]]; then
    AUTH="api"
    note "认证方式" "App Store Connect API 密钥（$ASC_API_KEY_ID）"
  else
    note "API 密钥文件" "找不到 AuthKey_${ASC_API_KEY_ID}.p8"
    note "" "  → 放到 ~/.appstoreconnect/private_keys/ 下"
    fail=1
  fi
elif [[ -n "${ASC_APPLE_ID:-}" && -n "${ASC_APP_PASSWORD:-}" ]]; then
  AUTH="appleid"
  note "认证方式" "Apple ID + 应用专用密码（$ASC_APPLE_ID）"
else
  note "认证" "未配置"
  note "" "  → 设 ASC_API_KEY_ID / ASC_API_ISSUER_ID，或 ASC_APPLE_ID / ASC_APP_PASSWORD"
  fail=1
fi

# 团队号从已签名包里读，避免手填出错
TEAM_ID="$(security find-identity -v -p basic 2>/dev/null | grep -oE '\(([A-Z0-9]{10})\)' | head -1 | tr -d '()' || true)"
note "Team ID" "${TEAM_ID:-（未能从证书读出，需手工确认）}"

if [[ $fail -eq 1 ]]; then
  echo ""
  echo "前置检查未通过，停在上传之前。"
  exit 1
fi

if [[ $CHECK_ONLY -eq 1 ]]; then
  echo ""
  echo "前置检查通过（--check 模式，未上传）。"
  exit 0
fi

# ---- 上传 ---------------------------------------------------------------
echo ""
echo "=== 上传 $(basename "$PKG") ==="
echo "App Store Connect 处理通常需要 5–20 分钟，期间不要重复上传同一版本号。"

mkdir -p target/dist/logs
LOG="target/dist/logs/upload-$(date +%Y%m%d-%H%M%S).log"

set +e
if [[ "$AUTH" == "api" ]]; then
  "$TRANSPORTER" -m upload \
    -assetFile "$PKG" \
    -apiKey "$ASC_API_KEY_ID" \
    -apiIssuer "$ASC_API_ISSUER_ID" \
    -apiKeyType "${ASC_API_KEY_TYPE:-team}" \
    ${TEAM_ID:+-asc_provider "$TEAM_ID"} \
    -v informational 2>&1 | tee "$LOG"
else
  "$TRANSPORTER" -m upload \
    -assetFile "$PKG" \
    -u "$ASC_APPLE_ID" \
    -p "$ASC_APP_PASSWORD" \
    ${TEAM_ID:+-asc_provider "$TEAM_ID"} \
    -v informational 2>&1 | tee "$LOG"
fi
rc=${pipestatus[1]}
set -e

echo ""
echo "日志：$LOG"

if [[ $rc -ne 0 ]]; then
  echo "上传失败（退出码 $rc）。常见原因：" >&2
  grep -iE 'ERROR|Invalid|missing|not permitted|version' "$LOG" | tail -8 >&2
  exit $rc
fi

echo "上传完成。到 App Store Connect → 该 App → 构建版本，等处理完成后再选进版本。"

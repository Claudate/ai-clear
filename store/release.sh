#!/bin/zsh
# 一条命令从上架物料走到「可以提交审核」。四步各司其职：
#
#   1. 出图标 / 截图（界面改了才需要重跑，脚本自己判断）
#   2. 编译 + 组装 .app + 签名 + 出 .pkg
#   3. 同步商店页文案截图到 fastlane 结构
#   4. 前置检查 → 上传二进制 → 等处理 → 同步商店页
#
#   ./store/release.sh            # 全跑（第 4 步会真的上传）
#   ./store/release.sh --dry-run  # 只做到前置检查，不上传
#   ./store/release.sh --shots    # 强制重出图标与截图
#
# 上传之后的「提交以供审核」不在这里做：App Store Connect 的提审
# 需要人工点确认（尤其是首次上架要确认导出合规、价格、分级），
# deliver 的 submit_for_review 能自动化，但首次建议自己看一眼再点。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

DRY_RUN=0
FORCE_SHOTS=0
for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    --shots)   FORCE_SHOTS=1 ;;
    *) echo "未知参数：$arg" >&2; exit 2 ;;
  esac
done

step() { printf '\n\033[1m==> %s\033[0m\n' "$1"; }

# ---- 1. 图 ------------------------------------------------------------
# 界面没改就不重出：截图合成本身要跑一遍 shots，几十秒，每次都跑很浪费。
NEED_SHOTS=0
[[ $FORCE_SHOTS -eq 1 ]] && NEED_SHOTS=1
if [[ ! -f store/icon-1024.png || ! -d store/screenshots/zh-Hans ]]; then
  NEED_SHOTS=1
fi
if [[ $NEED_SHOTS -eq 0 ]]; then
  newer=$(find crates/molt-gui/src -newer store/screenshots/zh-Hans/01-free-space.png -name '*.rs' 2>/dev/null | head -1)
  [[ -n "$newer" ]] && NEED_SHOTS=1 && echo "界面代码有更新（$newer），重出截图。"
fi

if [[ $NEED_SHOTS -eq 1 ]]; then
  step "生成图标与截图"
  python3 store/make_icon.py
  iconutil -c icns store/icon.iconset -o store/icon.icns
  # shots 必须真跑：商店截图要能对上用户看到的界面，不能靠旧图。
  cargo run --release -p molt-gui --bin shots -- target/shots
  python3 store/make_screenshots.py
else
  step "图标与截图：无变化，跳过"
fi

step "校验文案长度"
python3 store/check_limits.py

# ---- 2. 打包 ----------------------------------------------------------
step "编译并打包 .app / .pkg"
./store/build_app.sh mas

# ---- 3. 同步 ----------------------------------------------------------
step "同步商店页到 fastlane 结构"
python3 store/tools/sync_fastlane.py

# ---- 4. 检查与上传 ----------------------------------------------------
step "前置检查"
if ! ./store/tools/upload.sh --check; then
  echo ""
  echo "前置检查没过，先补齐上面列出的项。常见两处："
  echo "  · .pkg 未签名  → ./store/tools/sign_pkg.sh --setup 然后 ./store/tools/sign_pkg.sh"
  echo "  · 认证未配置   → export ASC_API_KEY_ID=… ASC_API_ISSUER_ID=… ASC_TEAM_ID=HA7KN8S95K"
  exit 1
fi

if [[ $DRY_RUN -eq 1 ]]; then
  echo ""
  echo "--dry-run：到此为止，未上传。"
  exit 0
fi

step "上传二进制"
./store/tools/upload.sh

step "同步商店页"
cd store/fastlane
FASTLANE_SKIP_UPDATE_CHECK=1 FASTLANE_HIDE_CHANGELOG=1 fastlane ios metadata

cat <<'EOF'

二进制与商店页都已就位。剩下三件事必须人工在 App Store Connect 网页端确认：

  1. App 隐私问卷（store/app-privacy-answers.md 已逐题写好）
  2. 价格与销售范围、年龄分级
  3. 选构建版本 → 提交以供审核

提交后 Apple 通常在 24–48 小时内回复。
EOF

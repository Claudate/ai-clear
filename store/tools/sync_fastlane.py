#!/usr/bin/env python3
"""把 store/ 下的上架物料同步成 fastlane 需要的目录结构。

为什么要多这一层：物料只有 store/ 一份（图标、截图、双语六个字段都在那儿），
而 fastlane 要求 metadata/<locale>/ 与 screenshots/<locale>/ 的固定布局。
手工维护两份迟早不一致，所以用脚本单向同步：store/ 是源，fastlane/ 是产物。

只同步 fastlane 会读的东西；隐私政策 .md 保持原样（要发布到网站，不是交付给
deliver 的文件）。URL 类字段若在 store/ 里没有，就写占位值——preflight 会把
占位值判为未通过，逼你在提交前填真地址。
"""
from __future__ import annotations

import shutil
import sys
from pathlib import Path

STORE = Path(__file__).resolve().parent.parent
FASTLANE = STORE / "fastlane"
LOCALES = ("zh-Hans", "en-US")

# fastlane 的字段名 ← store/metadata/<locale>/ 里的文件名。
# 两边名字大多相同，列出来是为了让「多一个少一个」在 diff 里看得出来。
FIELDS = (
    "name",
    "subtitle",
    "keywords",
    "promotional_text",
    "description",
    "release_notes",
)

# 两个必填 URL + 营销 URL 均已真实发布并实测 HTTP 200：
#   privacy_url → aiclear.openclawbot.cn/privacy/
#   support_url → aiclear.openclawbot.cn/support/
#   marketing_url → aiclear.openclawbot.cn/（官网首页）
PLACEHOLDERS = {
    "support_url": "https://aiclear.openclawbot.cn/support/",
    "privacy_url": "https://aiclear.openclawbot.cn/privacy/",
    "marketing_url": "https://aiclear.openclawbot.cn/",
}


def sync_locale(locale: str) -> list[str]:
    src = STORE / "metadata" / locale
    dst = FASTLANE / "metadata" / locale
    dst.mkdir(parents=True, exist_ok=True)

    written: list[str] = []
    for field in FIELDS:
        path = src / f"{field}.txt"
        if not path.exists():
            print(f"  跳过 {locale}/{field}.txt（源里没有）", file=sys.stderr)
            continue
        (dst / f"{field}.txt").write_text(path.read_text(encoding="utf-8"), encoding="utf-8")
        written.append(field)

    for field, value in PLACEHOLDERS.items():
        target = dst / f"{field}.txt"
        # 已经手工填过真地址就不要覆盖回去——那会让人白白丢一次配置。
        if target.exists() and "example.com" not in target.read_text(encoding="utf-8"):
            continue
        target.write_text(value, encoding="utf-8")
        written.append(field)

    # 截图：文件名本身决定了商店里的排列顺序，原样复制、不改名。
    shots_src = STORE / "screenshots" / locale
    shots_dst = FASTLANE / "screenshots" / locale
    if shots_src.is_dir():
        shots_dst.mkdir(parents=True, exist_ok=True)
        for old in shots_dst.glob("*.png"):
            old.unlink()
        for png in sorted(shots_src.glob("*.png")):
            shutil.copy2(png, shots_dst / png.name)
        written.append(f"screenshots×{len(list(shots_src.glob('*.png')))}")

    return written


def sync_review_notes() -> bool:
    """把审核备注里可直接粘贴的那段英文抽出来，交给 fastlane 当 notes.txt。"""
    src = STORE / "review-notes.md"
    if not src.exists():
        return False

    text = src.read_text(encoding="utf-8")
    # review-notes.md 用单独一行 --- 包住可直接粘贴的正文，取第一段。
    parts = text.split("---")
    body = ""
    for part in parts:
        if "No account or sign-in is required" in part or "How to test" in part:
            body = part.strip()
            break
    if not body:
        return False

    dst = FASTLANE / "metadata" / "review_information"
    dst.mkdir(parents=True, exist_ok=True)
    (dst / "notes.txt").write_text(body + "\n", encoding="utf-8")

    # 审核联系人必须是真人、真能联系上。这里留空并说明，不编造。
    for field, hint in (
        ("first_name.txt", ""),
        ("last_name.txt", ""),
        ("email_address.txt", ""),
        ("phone_number.txt", ""),
    ):
        target = dst / field
        if not target.exists():
            target.write_text(hint, encoding="utf-8")
    return True


def main() -> int:
    print(f"源：{STORE}")
    print(f"目标：{FASTLANE}")

    for locale in LOCALES:
        if not (STORE / "metadata" / locale).is_dir():
            print(f"缺少语言目录 metadata/{locale}", file=sys.stderr)
            return 1
        written = sync_locale(locale)
        print(f"  {locale}: {', '.join(written)}")

    if sync_review_notes():
        print("  审核备注：已抽取")

    # fastlane 每次运行都会重写 fastlane/README.md，把说明覆盖成它自己的
    # 自动文档。这里把我们的说明转存一份，被覆盖时能一键恢复。
    readme_src = FASTLANE / "README.zh.md"
    if readme_src.exists():
        (FASTLANE / "README.md").write_text(readme_src.read_text(encoding="utf-8"), encoding="utf-8")
        print("  README：已恢复 store/fastlane/README.md")

    print("\n完成。注意 support_url / privacy_url 还是占位值，")
    print("在 fastlane/metadata/<locale>/ 下填真地址，或改脚本里的 PLACEHOLDERS。")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

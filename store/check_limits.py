#!/usr/bin/env python3
"""按 App Store Connect 的实际上限校验 listing 字段，超了就退出 1。

用法：python3 store/check_limits.py

上限来自 Apple 帮助文档（2026-10 核对）：
name / subtitle 各 30 字符，promotional text 170，description / release notes 4000，
keywords 100 **字节**。关键词按字节算而不是字符：中文一个字 3 字节，
所以中文关键词实际只能用三十几个字，按字数估会超一倍。

字符按 Apple 的口径算——它把 emoji 和组合字符当一个字符，所以用 len() 而不是
按 UTF-16 编码单元。关键词是例外，那条明确写的是 bytes。
"""

import sys
from pathlib import Path

LIMITS = {
    "name.txt": ("字符", 30),
    "subtitle.txt": ("字符", 30),
    "promotional_text.txt": ("字符", 170),
    "description.txt": ("字符", 4000),
    "release_notes.txt": ("字符", 4000),
    "keywords.txt": ("字节", 100),
}

ROOT = Path(__file__).resolve().parent / "metadata"


def main() -> int:
    failed = False
    for lang_dir in sorted(ROOT.iterdir()):
        if not lang_dir.is_dir():
            continue
        print(f"\n{lang_dir.name}")
        for fname, (unit, limit) in LIMITS.items():
            path = lang_dir / fname
            if not path.exists():
                print(f"  缺失  {fname}")
                failed = True
                continue
            text = path.read_text(encoding="utf-8").strip()
            used = len(text.encode("utf-8")) if unit == "字节" else len(text)
            flag = "超限" if used > limit else "ok  "
            if used > limit:
                failed = True
            print(f"  {flag}  {fname:<22} {used:>4} / {limit} {unit}")

    print()
    if failed:
        print("有字段超限，改完再传。")
        return 1
    print("全部字段在限制内。")
    return 0


if __name__ == "__main__":
    sys.exit(main())

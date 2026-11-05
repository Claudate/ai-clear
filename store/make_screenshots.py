#!/usr/bin/env python3
"""把真实界面渲染图做成 App Store 截图（2880×1800，16:10，无 alpha）。

用法：先 `cargo run -p molt-gui --bin shots -- target/shots`，再跑本脚本。

为什么不是直接上传界面原图：原图是 1060×760 的窗口，缩到 1440×900 时
文字只有 10pt 左右，在商店页的手机端预览里糊成一团。这里把窗口放大到
占满画幅宽度，上方留一条标题带写一句结论——商店页的浏览方式是先看标题
再决定要不要放大看图，标题不参与截图就浪费了第一眼。

底图必须是 shots 工具出的真实界面，不能是重画的示意图：文案里说的每一处
布局都得在真软件里存在，否则截图就成了宣传物料而不是证据。

中英文各出一套：App Store 的截图按语言分开上传，英文区不该看到中文界面。
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

W, H = 2880, 1800
BAND = 300  # 标题带高度；剩下的 1500 给窗口
SHOT = Path("target/shots")
OUT = Path("store")

# 与 theme.rs 的调色板一致，标题带不会跟窗口里的界面撞色。
GREEN = (13, 104, 88)
INK = (23, 26, 25)
DIM = (101, 110, 106)
BG = (246, 247, 246)
PAPER = (255, 255, 255)
BORDER = (226, 230, 228)

PINGFANG = "/System/Library/AssetsV2/com_apple_MobileAsset_Font7/3419f2a427639ad8c8e139149a287865a90fa17e.asset/AssetData/PingFang.ttc"
HELVETICA = "/System/Library/Fonts/Helvetica.ttc"

# 每张图对应一个真实界面状态，标题说的是这一屏能解决什么。
# 顺序就是商店页里的顺序：先回答「能腾多少」，最后才给 AI 这种附加能力。
# 文件名用 ASCII：App Store Connect 按文件名排上传顺序，中文名在不同系统下
# 排序结果不一样，带编号的英文名能把顺序钉住。
CASES = [
    ("01-workbench", "01-free-space",
     "一眼看到能腾多少", "At a glance: how much you can free"),
    ("09-confirm-quarantine", "02-every-item",
     "每一项都写清删了会怎样", "Every item says what deleting costs"),
    ("12-quarantine", "03-quarantine",
     "7 天内随时放回原处", "Put anything back within 7 days"),
    ("15-automation", "04-automation",
     "定时只清纯缓存，不碰构建产物", "Scheduled cleanup touches only pure caches"),
    ("14-history", "05-history",
     "每笔操作都有记录，手动与自动分得清", "Every action logged - manual or scheduled"),
    ("03-workbench-dark", "06-dark-mode",
     "跟随系统外观，字可放大", "Follows system appearance, text can scale"),
]


def font(size: int, bold: bool = False, latin: bool = False) -> ImageFont.FreeTypeFont:
    """PingFang 的索引 0/1 是常规/中黑，Helvetica 的索引对应不同字重。

    中文用 PingFang：界面本身装的也是系统 CJK 字体，标题与窗口里的字形一致。
    英文标题用 Helvetica Neue：PingFang 的拉丁字形偏松，作大标题不够紧。
    """
    if latin:
        # Helvetica.ttc: 0=Regular, 1=Bold, 2=Oblique, 3=BoldOblique
        return ImageFont.truetype(HELVETICA, size, index=1 if bold else 0)
    return ImageFont.truetype(PINGFANG, size, index=3 if bold else 2)


def rounded_window(shot: Image.Image, radius: int = 18) -> Image.Image:
    """给窗口底图加圆角与一圈细边，让它从背景里浮起来。

    shots 出来的是没有窗口边框的纯客户区，直接贴上去会像一张截图贴纸；
    加圆角和描边之后才读成「一个正在运行的窗口」。
    """
    w, h = shot.size
    mask = Image.new("L", (w, h), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, w - 1, h - 1), radius=radius, fill=255)

    canvas = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    canvas.paste(shot.convert("RGBA"), (0, 0), mask)
    ImageDraw.Draw(canvas).rounded_rectangle(
        (0, 0, w - 1, h - 1), radius=radius, outline=BORDER + (255,), width=2
    )
    return canvas


def compose(src: Path, dst: Path, title: str, subtitle: str, latin: bool) -> None:
    shot = Image.open(src).convert("RGB")

    # 窗口尽量大但不越界：左右各留 120，标题带下留 40 的呼吸，
    # 底部至少留 60——贴边会读成「被裁掉了」，留白才读成「窗口落在画面里」。
    avail_w = W - 240
    avail_h = H - BAND - 60
    scale = min(avail_w / shot.width, avail_h / shot.height)
    win = shot.resize(
        (round(shot.width * scale), round(shot.height * scale)), Image.LANCZOS
    )
    win = rounded_window(win)

    page = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(page)

    # 标题带：左对齐，与窗口左边缘对齐。标题用主文字色，
    # 副标题用次级色——两者都完整可读，不做「大字压小字」的花活。
    x = (W - win.width) // 2
    title_font = font(84 if not latin else 80, bold=True, latin=latin)
    sub_font = font(46 if not latin else 44, bold=False, latin=latin)
    d.text((x, 58), title, font=title_font, fill=INK)
    d.text((x, 58 + (104 if not latin else 100)), subtitle, font=sub_font, fill=DIM)

    page.paste(win, (x, BAND), win)

    # 底部一条强调色短横：给标题带收个口，也把品牌色带进画面。
    d.rounded_rectangle((x, BAND - 30, x + 88, BAND - 22), radius=4, fill=GREEN)

    page.save(dst, optimize=True)
    print(f"{dst}  {page.size[0]}x{page.size[1]}  {page.mode}")


def main() -> None:
    zh_dir = OUT / "screenshots" / "zh-Hans"
    en_dir = OUT / "screenshots" / "en-US"
    zh_dir.mkdir(parents=True, exist_ok=True)
    en_dir.mkdir(parents=True, exist_ok=True)

    for name, out_name, zh_title, en_title in CASES:
        src = SHOT / f"{name}.png"
        if not src.exists():
            raise SystemExit(f"缺少界面底图 {src}，先跑 shots")
        compose(src, zh_dir / f"{out_name}.png", zh_title, "净灵 · Cleanling", latin=False)
        compose(src, en_dir / f"{out_name}.png", en_title, "Cleanling", latin=True)


if __name__ == "__main__":
    main()

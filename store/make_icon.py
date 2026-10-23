#!/usr/bin/env python3
"""生成 molt 的应用图标：商店用 1024（无 alpha）+ macOS 用的 .iconset 素材。

用法：python3 store/make_icon.py

两张图用途不同，不能混：
- `icon-1024.png` 传到 App Store Connect。Apple 拒收带 alpha 通道的图，
  所以它铺纯白底、转成 RGB。白色四角与商店页的浅底相融，看起来仍是圆角图标。
- `icon.iconset/*` 打进 .app 的 Resources。macOS 的窗口/Dock 需要圆角外的
  真透明，所以这些保留 alpha。

图形是「收拢」：底部最宽当托盘，往上依次变窄。缩到 16pt 时读出来仍是一摞
东西，而不是一团色块——这是它唯一要在小尺寸下传达的信息，所以不放文字、
不放细线、不做渐变。反过来（上宽下窄）会读成漏斗，那是「筛选」不是「蜕」。
"""

from pathlib import Path

from PIL import Image, ImageDraw

# 品牌色取自 crates/molt-gui/src/theme.rs 的 Palette::light().reclaimable，
# 图标与界面用同一个绿，用户从商店页到窗口不会看到两套品牌色。
GREEN = (13, 104, 88)
PAPER = (246, 247, 246)

SIZE = 1024
# macOS 图标网格：1024 画布里内容区 824、圆角 185.4，四边各留 100。
MARGIN = 100
CONTENT = SIZE - MARGIN * 2
RADIUS = 185
SUPERSAMPLE = 4

OUT = Path(__file__).resolve().parent


def draw_icon(size: int, background: tuple[int, int, int, int], margin: int) -> Image.Image:
    """按 macOS 图标网格画一张，返回 RGBA。`margin` 为 0 时内容铺满画布。"""
    s = SUPERSAMPLE
    w = size * s
    img = Image.new("RGBA", (w, w), background)
    d = ImageDraw.Draw(img)

    m = margin * s
    box = (m, m, w - m, w - m)
    radius = RADIUS * s * ((w - 2 * m) / (SIZE * s)) if margin == 0 else RADIUS * s
    d.rounded_rectangle(box, radius=radius, fill=GREEN + (255,))

    # 图形按内容区的比例摆，margin 为 0 时用满画布。
    c = w - 2 * m
    cx = w / 2

    # 三条横条：底宽上窄。0.58 / 0.46 / 0.34 是试出来的比例——
    # 更接近等比会让三层看起来一样宽，收拢的意思就没了。
    # 条比看起来能再粗一点：16pt 下细条会糊成灰边，反而失去「一摞」的形状。
    bar_h = c * 0.135
    gap = c * 0.075
    widths = [0.34, 0.46, 0.58]
    total_h = bar_h * 3 + gap * 2
    top = w / 2 - total_h / 2

    for i, rel_w in enumerate(widths):
        bw = c * rel_w
        y = top + i * (bar_h + gap)
        d.rounded_rectangle(
            (cx - bw / 2, y, cx + bw / 2, y + bar_h),
            radius=bar_h / 2,
            fill=PAPER + (255,),
        )

    return img.resize((size, size), Image.LANCZOS)


def write_store_icon() -> None:
    # 白底 + 无 alpha：App Store Connect 明确拒收带透明区域的图。
    img = draw_icon(SIZE, (255, 255, 255, 255), MARGIN).convert("RGB")
    path = OUT / "icon-1024.png"
    img.save(path, optimize=True)
    print(f"{path}  {img.size[0]}x{img.size[1]}  {img.mode}")


def write_iconset() -> None:
    """生成 iconutil 需要的十个尺寸。圆角外保持透明，交给 macOS 合成。"""
    iconset = OUT / "icon.iconset"
    iconset.mkdir(exist_ok=True)
    # iconutil 认这套固定命名，少一个就打包失败。
    pairs = [
        (16, "icon_16x16.png"),
        (32, "icon_16x16@2x.png"),
        (32, "icon_32x32.png"),
        (64, "icon_32x32@2x.png"),
        (128, "icon_128x128.png"),
        (256, "icon_128x128@2x.png"),
        (256, "icon_256x256.png"),
        (512, "icon_256x256@2x.png"),
        (512, "icon_512x512.png"),
        (1024, "icon_512x512@2x.png"),
    ]
    for px, name in pairs:
        # 小尺寸按比例缩边距，否则 16pt 下内容会缩成一个小点。
        margin = round(MARGIN * px / SIZE)
        draw_icon(px, (0, 0, 0, 0), margin).save(iconset / name)
    print(f"{iconset}/  {len(pairs)} 个尺寸（含 alpha）")


if __name__ == "__main__":
    write_store_icon()
    write_iconset()

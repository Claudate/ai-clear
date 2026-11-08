#!/usr/bin/env python3
"""从 public 里的品牌 logo 生成打包用的图标：商店 1024（无 alpha）+ macOS .iconset 素材。

用法：python3 store/make_icon.py

源图是 `public/macos/icon_512x512@2x.png`（1024 见方、四角透明、已按 macOS
图标网格留过边距）。换 logo 只换 public 里那一套，这里重跑一次即可。

两张图用途不同，不能混：
- `icon-1024.png` 传到 App Store Connect。Apple 拒收带 alpha 通道的图，
  所以它铺纯白底、转成 RGB。白色四角与商店页的浅底相融，看起来仍是圆角图标。
- `icon.iconset/*` 打进 .app 的 Resources。macOS 的窗口/Dock 需要圆角外的
  真透明，所以这些保留 alpha。
"""

from pathlib import Path

from PIL import Image

SOURCE = Path(__file__).resolve().parent.parent / "public/macos/icon_512x512@2x.png"
SIZE = 1024

OUT = Path(__file__).resolve().parent


def load_source() -> Image.Image:
    """读源图并确认尺寸/透明度符合预期——拿错图时这里就该炸，别拖到打包。"""
    img = Image.open(SOURCE).convert("RGBA")
    if img.size != (SIZE, SIZE):
        raise SystemExit(f"{SOURCE} 应当是 {SIZE}x{SIZE}，实际 {img.size}")
    if img.getpixel((0, 0))[3] != 0:
        raise SystemExit(f"{SOURCE} 四角应当是透明的，否则图标会带一块白底")
    return img


def write_store_icon(img: Image.Image) -> None:
    """商店图：白底、无 alpha，Apple 明确拒收带透明区域的图。"""
    flat = Image.new("RGB", img.size, (255, 255, 255))
    flat.paste(img, mask=img.split()[3])
    path = OUT / "icon-1024.png"
    flat.save(path, optimize=True)
    print(f"{path}  {flat.size[0]}x{flat.size[1]}  {flat.mode}")


def write_iconset(img: Image.Image) -> None:
    """生成 iconutil 需要的十个尺寸。透明区域原样带过去，交给 macOS 合成。"""
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
        # 源图已经按 macOS 网格留好边距，等比缩放即可；16pt 下不会缩成一个点。
        img.resize((px, px), Image.LANCZOS).save(iconset / name, optimize=True)
    print(f"{iconset}/  {len(pairs)} 个尺寸（含 alpha）")


if __name__ == "__main__":
    source = load_source()
    write_store_icon(source)
    write_iconset(source)

"""Rasterises assets/section3d-icon-dark.svg (polygons only) to PNG/ICO with Pillow.

Usage: python tools/make_icons.py
"""
import re
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
svg = (ROOT / "assets" / "section3d-icon-dark.svg").read_text()
view = 512.0
ss = 4  # supersampling


def render(size):
    big = size * ss
    k = big / view
    img = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    rx = float(re.search(r'<rect[^>]*rx="([\d.]+)"', svg).group(1))
    bg = re.search(r'<rect[^>]*fill="(#[0-9A-Fa-f]+)"', svg).group(1)
    d.rounded_rectangle([0, 0, big - 1, big - 1], radius=rx * k, fill=bg)
    for pts, fill, stroke, sw in re.findall(r'points="([^"]+)" fill="(#[0-9A-Fa-f]+)" stroke="(#[0-9A-Fa-f]+)" stroke-width="([\d.]+)"', svg):
        p = [tuple(float(c) * k for c in q.split(",")) for q in pts.split()]
        d.polygon(p, fill=fill)
        d.line(p + [p[0]], fill=stroke, width=max(1, round(float(sw) * k)), joint="curve")
    return img.resize((size, size), Image.LANCZOS)


out = ROOT / "crates" / "s3d-app" / "assets"
render(256).save(out / "icon-256.png")
sizes = [16, 24, 32, 48, 64, 128, 256]
render(256).save(out / "icon.ico", sizes=[(s, s) for s in sizes])
print("wrote", out / "icon-256.png", out / "icon.ico")

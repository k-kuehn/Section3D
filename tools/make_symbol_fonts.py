"""Builds the two tiny symbol fonts shipped with the app from Noto (OFL).

Sources (download into a folder and pass it as the first argument):
  Noto Sans Symbols 2: https://cdn.jsdelivr.net/fontsource/fonts/noto-sans-symbols-2@5.2.5/symbols-400-normal.ttf
  Noto Sans Math:      https://github.com/google/fonts/raw/main/ofl/notosansmath/NotoSansMath-Regular.ttf
Requires fontTools.
"""
import os
import sys

from fontTools import subset

src = sys.argv[1]
out = os.path.join(os.path.dirname(__file__), "..", "crates", "s3d-app", "assets")
SYMBOLS = "▶▾◀◌●○☀⚠✓✕✖✔🔒🔓🖨★▲▼■□◆◇▸◂⬤"
MATH = "Δ→←↑↓↔↕↶↷↺↻≈≤≥±∞⟲⟳☾"

for name, fname, chars in [
    ("Symbols.ttf", "noto-sans-symbols-2_5.2.5_symbols-400-normal.ttf", SYMBOLS),
    ("MathSymbols.ttf", "NotoSansMath-Regular.ttf", MATH),
]:
    opts = subset.Options()
    opts.layout_features = []
    opts.name_IDs = ["*"]
    opts.notdef_outline = True
    font = subset.load_font(os.path.join(src, fname), opts)
    sub = subset.Subsetter(opts)
    sub.populate(unicodes=[ord(c) for c in chars])
    sub.subset(font)
    subset.save_font(font, os.path.join(out, name), opts)
    print("wrote", name, os.path.getsize(os.path.join(out, name)), "bytes")

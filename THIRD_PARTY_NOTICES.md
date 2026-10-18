# Third-party notices

Section3D is licensed under GPL-3.0-or-later (see `LICENSE`). It includes:

| Component | Use | License |
|---|---|---|
| [Manifold](https://github.com/elalish/manifold) v3.5.4 (`third_party/manifold`) | Robust mesh booleans, statically linked | Apache-2.0 (`third_party/manifold/LICENSE`) |
| [Inter](https://rsms.me/inter/) (latin subset) | UI font and part-number font | SIL OFL 1.1 (`crates/s3d-app/assets/OFL.txt`) |
| [JetBrains Mono](https://www.jetbrains.com/lp/mono/) (latin subset) | Monospace UI text | SIL OFL 1.1 |
| [Noto Sans Symbols 2](https://fonts.google.com/noto/specimen/Noto+Sans+Symbols+2) and [Noto Sans Math](https://fonts.google.com/noto/specimen/Noto+Sans+Math) (small subsets) | UI symbols | SIL OFL 1.1 |
| Rust crates: egui/eframe, wgpu, glam, rayon, miniz_oxide, quick-xml, serde, serde_json, rfd, ab_glyph, bytemuck | Application framework and libraries | MIT and/or Apache-2.0 |

The symbol font subsets are built by `tools/make_symbol_fonts.py`; the app icon by `tools/make_icons.py`.

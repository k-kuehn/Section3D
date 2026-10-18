# Section3D

Split a 3D model that is too big for your FDM printer into pieces that each fit the printer, print without
supports, join with dowel pins and assemble back into the original. Section3D produces print-ready 3MF files
(one per bed), an assembly guide (PDF), a print list (CSV) and an offline 3D viewer (HTML).

It is a single portable program, `section3d.exe` (about 16 MB). It needs no installer and no runtime, and it runs offline.

## Workflow

1. **Import**: open an STL, OBJ or 3MF file (or drop it on the window). Broken meshes are repaired
   automatically. If repair fails, the mesh is rebuilt from voxels and you are warned.
2. **Prepare**: rotate, scale, mirror. Optionally hollow the model to a shell (only when you press *Hollow*).
3. **Split**: *Triangle* mode (the default) or *Grid*. *Split* cuts the model and adds connectors and part numbers in one job.
   - **Triangle mode** lays out big, brick-staggered triangular pieces that print standing on a cut face without supports.
     The best layout so far is drawn on the model while the search runs.
   - **No-cut zones**: paint areas that cuts must avoid, such as a face or lettering.
   - **Edit pieces**: click a piece to see its cuts. Drag a cut's handles to move or tilt it; only the two pieces on that cut change.
     A live preview shows the result and *Apply* commits it. You can also merge, split or lock a piece, lock a cut, or
     *Fix support*.
   - **Right-click a piece** for *Isolate*, *Isolate with neighbours*, *Hide*, *Show all* or *Fix support*.
   - **Manual edits** are kept in a list and replayed when you split again. *Undo* and *Redo* cover every change.
4. **Connectors**: set the dowel pin diameter, clearance, length and number per cut, and embossed or debossed part
   numbers. Holes get teardrop or cone tops so they print without support. A hole is skipped and flagged where the wall
   is too thin, and the app suggests a pin size that fits.
5. **Export**: pieces are packed tightly onto as few beds as possible. You can drag them between beds, or right-click
   one to rotate it. Then write the 3MF beds, optional single STLs, the CSV list, the PDF guide and the HTML viewer.
   You can set file-name templates (`{project}`, `{bed}`, `{part}`) and shrinkage compensation.

Changing a setting after a step ran marks that step and the steps after it *out of date*, with an **Update** button. Nothing
re-runs on its own. Long work runs in the background with progress and **Cancel**.

Projects (`.s3d`) store the model, settings, edits and results, so opening one never re-cuts. The app autosaves and
offers recovery after a crash.

## Building

Requirements: Rust (stable, MSVC toolchain) and the Visual Studio Build Tools, whose bundled CMake is used to build Manifold.

```
cargo build --release
```

The executable is `target/release/section3d.exe`. It links the C runtime statically, so it needs only Windows
system DLLs. Put a file named `portable` next to the exe to keep settings in a `Section3D-data` folder beside it instead of
`%APPDATA%`.

## Tests

```
cargo test --release --workspace
```

- Unit tests cover geometry and I/O; `crates/s3d-core/tests/requirements.rs` checks the requirements end to end.
- `crates/s3d-app/tests/ui_flows.rs` drives the real UI headlessly through egui_kittest and wgpu, and writes screenshots to
  `target/ui-screens/`.
- Performance checks on a large model run when `S3D_SAMPLE` points to an STL:

```
S3D_SAMPLE="path/to/model.stl" cargo test --release -p s3d-core --test perf_sample -- --nocapture
```

Set `S3D_TIMING=1` to print stage timings.

## Layout

```
crates/s3d-core   geometry: I/O, repair, hollowing, partition and seams, grid and triangle split, orientation,
                  connectors, labels, packing, assembly order, export (3MF, STL, CSV, PDF, HTML), projects
crates/s3d-app    egui/wgpu desktop app: steps, 3D viewport, parts list, beds view, jobs, undo, autosave
third_party       vendored Manifold (booleans)
```

License: GPL-3.0-or-later. See `THIRD_PARTY_NOTICES.md` for included components.

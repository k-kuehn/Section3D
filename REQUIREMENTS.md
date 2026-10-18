# Section3D: Baseline Requirements

A portable desktop app that splits a 3D model too large for an FDM printer into pieces that each fit the printer, print
without supports, join with pins, and assemble into the original. It produces print-ready files and an assembly
guide. Runs offline as a normal desktop program. Open source (GPL-3.0-or-later). Units mm. Z up.

## Workflow
Steps in order: **Import → Prepare → Split → Connectors → Export**.
- Each step shows only its own settings and has one clear button that starts its work. Switching steps never
  starts processing or heavy redrawing; it only shows results already computed.
- Changing a setting after a step ran marks that step and later ones "out of date" (with an Update button). Nothing
  re-runs on its own.
- Long work runs in the background with progress and Cancel. The UI never freezes. Use all CPU cores.
- Keep the UI simple: few settings, sensible defaults. Every setting has a tooltip with a
  short explanation and a diagram.

## Import and Prepare
- Read STL, OBJ, 3MF and STEP. Repair broken meshes automatically (merge vertices, fix winding, close holes). If repair
  fails, rebuild the mesh from voxels and warn the user. Show health: clean, repaired or unresolved.
- Rotate, scale, mirror.
- Optional hollowing to a shell with a chosen wall thickness, run only from its own button.

## Split
Modes: **Grid** (box cells sized to the printer), and **Triangle** (below).
Common rules:
- Cuts are always straight
- Every piece must fit the printer's build volume after margins.
- No slivers: tiny or thin pieces are merged into a neighbour or re-cut. Loose islands become pieces, then merge.
- User drawn no-cut zones (paint on model) that cuts must avoid.
- Manual edits by clicking on an individual part which opens the properties/settings for that part. The edges of the part that are cut are highlighted in the 3D view.
- Manual edits are kept in a list and replayed after each split: merge pieces, re-split one piece, move a cut position, tilt a cut individually (angle) or add a
  cut, lock a piece or cut. Each cut edge can be adjusted individually only affecting the parts on each side of that one cut. Locked items never change. Undo and redo.

## Connectors
- Optional connectors: add dowel pin holes along the edges. Connectors are added after splitting and manually adjustment of split edges.
- Dowels: matching blind holes on both sides of a cut, placed evenly along the cut face away from its edges. Skip and
  flag a hole where the wall is too thin. Teardrop or cone tops so holes print without support.
- Connector settings: dowel pin diameter, clearance, dowel pin length, number per edge. 
- Option to add part numbers embossed or debossed one side face of each piece (never on the cut edge).
- The Split button cuts the model and adds connectors and part numbers in one job, so pieces come out finished.

### Triangle mode (the core of the product)
Goal: the fewest, largest pieces that each fit the bed, print with **no supports**, and assemble into a strong
whole.
1. **Big pieces.** By default the target size is the largest piece that fits the printer. The user may lower it.
   Avoid small or thin pieces.
2. **Brick stagger.** Lay pieces out like bricks: rows of pieces, where the cuts between pieces in one row are offset
   by about half a piece from those in the rows above and below. A seam must never run straight through many pieces;
   the length of seams that line up is a cost to minimise.
3. **Triangle shape option.** Most pieces are triangles bounded by exactly **three cuts**: a base cut that sits on the
   print bed and two side cuts at about 45°. Base length = target size, height ≈ 75% of base length. Rows of triangles
   alternate point-up and point-down, and neighbouring rows are offset so diagonal seams don't continue. Add a fourth
   cut only when a piece would not fit, would need support, or would be too big or too small.
5. **The cut is the base.** Each piece prints standing on one of its own flat cut faces. Each cut's position and
   angle are tuned so the piece standing on it needs no support and has a wide, stable base.
6. **Not tall.** In print orientation a piece must not be taller than it is wide (triangle: height about half the
   base). If it is, re-split it or move a cut.
7. **Any angle.** Cuts may be at any angle, not only axis-aligned, and are chosen from the model's shape: its main
   axes, large flat areas, and the lines where overhangs begin.
8. **Priorities, in order:** fits the bed (never relaxed) > no supports > not tall > size close to target > seams
   staggered and short > seams away from sharp creases and fine detail > connectors fit the faces. If a rule can't be
   met, keep the best result and flag the piece with the reason and a suggested fix.
9. **Fast and repeatable.** Large models (about 1M triangles) are searched on a simplified copy, then the final cuts
   are applied to the full mesh once. A first usable result appears within about 2 seconds and improves while the
   search runs, up to a time limit or all parts are printable without supports.

## Orientation (every mode)
- For each piece, try every flat cut face (and its other large flat faces) as the face on the bed. Choose, in order:
  fits the printer > least support area > not tall > least total overhang > largest bed contact.
- Flag "needs support" only if no orientation avoids it. Show support areas in red in 3D.
- Overhang = tilt of a downward-facing surface from vertical; past the limit (default 45°) it needs support.

## Working with pieces
- Parts list and 3D view stay in sync. Click to select. Right-click a piece (in the list or the 3D view) for:
  Isolate, Isolate with neighbours, Hide, Show all, Fix support.
- **Per-piece cut handles:** selecting a piece shows a handle on each cut that bounds it (three for a triangle). Drag
  to move the cut or tilt it about the cut axis. Only the seam between this piece and its neighbours that share the cut changes,
  not the whole plane. Live preview of the outline and support area. Nothing changes until Apply.
- Fix support for a flagged piece: split just that piece, adjust one of its cuts as above, or "find a better cut"
  automatically.
- Cut planes are drawn faint and can be hidden. Section view, measuring, exploded view, assembly step slider.

## Assembly, Layout, Export
- Assembly order: start from the largest, lowest base and add touching pieces that can be inserted. Flag pieces with
  no clear path.
- Pack pieces onto as few print beds as possible in print orientation. The parts must pack very closely to minimize number of print setups. Optionally keep assembly order. Users can drag
  or rotate pieces between beds.
- Printer profiles: a generic one plus common printers with their published build volumes.
- Export one 3MF per bed (and single-part STLs), a print list (CSV: label, size, weight, bed, flags), and an assembly
  guide (PDF with a parts checklist and one step per page). Optional offline 3D viewer file.
- File naming template and destination folder; export history.
- Shrinkage compensation for export.

## Projects
- Save and open a project file holding the source model, settings, edits and results. Opening never re-cuts.
- Save and Save As use the system file dialog. Recent projects on a start screen. Autosave and crash recovery.
- Warn before discarding unsaved changes.

## Quality bar
- Every piece is a closed, valid solid. Pins and holes line up.
- Automated tests cover every requirement above, including UI flows. The real large sample model is used for
  performance checks.
- Performance: open a 1M-triangle model in under 1 s; smart split of such a model within its time limit; switching
  steps is instant.
- Out of scope: resin printers, colours or multiple materials, magnets/screws/snap joints, slicing.

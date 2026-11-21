# efeengine-world-ui

Display-only world-space UI for efeengine: text, rectangles, bars, sprites, and stacked containers anchored to entities or fixed world points and drawn over the 2D scene. Elements are sized and offset in logical pixels (keeping screen size constant as the camera zooms), take no input, and render through `Draw2D` using `efeengine-ui`'s `text_kit` (`cosmic-text` text shaping with bundled Inter faces and feathered shape meshes), so the same code runs in wgpu, SDL, and web builds.

## At a glance
| | |
|---|---|
| Path | `crates/world-ui` (package `efeengine-world-ui`) |
| Kind | library (rlib) |
| Depends on (workspace) | `efeengine-core` (ECS, `Plugin`, `Transform` / `GlobalTransform`, `Camera`, `Draw2D`, `Draw2DOverlay`, `Overlay2D`, `TextureHandle`, `OVERLAY_TEXTURE_BASE`), `efeengine-ui` (`default-features = false, features = ["text"]` for `text_kit` and `style::TextAlign`) |
| External deps | none directly (pulls `cosmic-text` via `efeengine-ui`'s `text` feature) |
| Used by | `examples/coin_dash`, `examples/coin_dash_wasm` |
| Features | none |
| Targets | native (wgpu and SDL modes) and `wasm32-unknown-unknown` |
| Tests | unit tests in `src/draw.rs` (4: camera projection, `approach`, text measurement and container stacking) |

## Layout
```text
crates/world-ui
├── Cargo.toml
├── structure.md
└── src
    ├── lib.rs         WorldUiPlugin, module list, and public re-exports of components
    ├── components.rs  Anchor, Anchor2D, Color, Text2D, Rect2D, Bar2D, Sprite2D, Container2D, Floating2D
    ├── extract.rs     PostUpdate extraction system: animates Bar2D/Floating2D, despawns expired floats, builds Snapshot
    └── draw.rs        WorldUiOverlay (Draw2DOverlay): projects anchors, measures/caches text, paints items via Draw2D
```

## Public API & Architecture

- **Plugin (`lib.rs`)**: `WorldUiPlugin` (`"efeengine-world-ui"`) shares an `Rc<RefCell<extract::Snapshot>>` between a `Stage::PostUpdate` extraction system (`extract::run`) and a world overlay (`draw::WorldUiOverlay`, registered with `engine.add_world_overlay`).
- **Components (`components.rs`)**:
  - `Anchor` (`Entity(Entity)` | `Point(Vec2)`) and `Anchor2D { target, offset }` (root marker; `offset` in logical pixels, +y down).
  - `Text2D { text, size, color, bold }` (single-line text shaped with bundled Inter; `size` in logical px).
  - `Rect2D { size, color, radius, border, border_color }` (solid or rounded rectangle with optional inner border).
  - `Bar2D { size, fraction, speed, back, fill, ghost_color, vertical, shown, ghost }` (progress bar with animated `shown` fill and slower-shrinking `ghost` damage flash).
  - `Sprite2D { texture: TextureHandle, size, tint }` (textured quad from `Textures`).
  - `Container2D { children: Vec<Entity>, vertical, gap, padding, background }` (stacks child entities; listed children are drawn by the container and skipped as standalone roots even if they carry `Anchor2D`).
  - `Floating2D { rise, life, age }` (moves a root's `Anchor2D::offset.y` upward at `rise` px/s, fades alpha to 0 over `life` s, and despawns the entity when `age >= life`).
- **Extraction (`extract.rs`)**:
  - Runs in `PostUpdate` so transforms are up to date. Advances `Bar2D` (`shown` at `speed`, `ghost` at `0.5 * speed` via `approach`) and `Floating2D`, queues `cx.commands.despawn` for expired floats, collects active orthographic cameras (`Camera::ORTHOGRAPHIC`, `active != 0`, lowest entity id per target window) into `Snapshot::cameras`, and copies root elements (up to container nesting depth 16) into `Snapshot::roots` with no world borrows held for rendering.
- **Rendering (`draw.rs`)**:
  - `WorldUiOverlay::render` looks up the `CamView` for `cx.window`, projects each root's world position with `to_screen` (centering camera position, rotating by `-cam.rotation`, flipping +y up to screen +y down, scaling by `cam.zoom`), adds `root.offset`, centers the measured element size, culls off-viewport roots, and paints via `Draw2D` (`TextSystem`, `TextCell` cache keyed by `Entity`, `Mesh` with `feather = !cx.integer_vertices`, and `out.quad`).
  - Uploads the per-window glyph atlas texture (`OVERLAY_TEXTURE_BASE + 0x100 + cx.window`, above the fallback UI's atlas ids) whenever `TextSystem::atlas.take_image()` returns `Some`.

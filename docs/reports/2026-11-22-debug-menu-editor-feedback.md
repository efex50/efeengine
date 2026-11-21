# Debug menu and in-game editor: feedback from a manual run (2026-11-22)

Source: the first manual run of `examples/editor_demo` on the native iced backend (Vulkan, Intel UHD 620), with the debug menu in its own window. The two screenshots that came with the feedback are `../screenshot/logmenu.png` and `../screenshot/editor menu.png`, outside the repo. None of these issues show up in the automated tests or the offscreen screenshots, which use the fallback backend with fixed window sizes.

## 1. Logs tab: "Auto scroll" and "Copy all" sit in the main tab bar

**Seen:** the Logs controls ("Auto scroll" checkbox and the "Copy all" button) are drawn at the right end of the main tab bar (Logs | Edits | Scripts | Editor). They look like global tabs. The checkbox also sits a few pixels higher than the tab labels.

**Expected:** these controls belong to the Logs tab only. Put them on a toolbar row inside the Logs tab content, above the log list. The tab bar should hold only the tabs.

**Where:** `crates/ui/src/debug_menu.rs`. The header is built where `state.tab == DebugTab::Logs` adds extra controls next to the `tab_button(...)` row.

## 2. Editor panels cannot be resized

**Seen:** in the Editor tab the left column (Hierarchy / Prefabs / Systems, i.e. the entity and component picking side) has a fixed width of about 230 px. The inspector (component fields) fills the rest. The user cannot change how much space either side gets. The top toolbars take a fixed height as well.

**Expected:** a draggable splitter between the left column and the inspector, with sensible minimum widths. Remember the split for the session, ideally per window (main window vs detached layout B). Optionally make the inspector scroll independently and let component sections collapse.

**Where:** `crates/ui/src/editor/panel.rs`, where the column widths are set. The UI toolkit may not have a splitter widget yet. If not, add one to `efeengine-ui` (fallback, iced and web backends) or implement it as a drag handle that updates a width in `EditorUi`.

## 3. Overlapping and clipped text

**Seen in `editor menu.png`:**
- The hierarchy toolbar "New  Duplicate  Delete  Unparent" does not fit the left column: "Delete" and "Unparent" are drawn on top of each other ("DeleteUnparent").
- The sub-tabs "Hierarchy  Prefabs  Systems" fill the column edge to edge. The highlighted "Hierarchy" tab is cut off at the left border.
- "(select an entity)" in the inspector has no padding and touches the column border.
- The hierarchy rows show a dark strip at the left edge (row background or indent marker).

**Expected:**
- No overlap at any panel width. The toolbar should wrap to a second row, use shorter labels or icons, or move the rarely used actions (Unparent, Duplicate) to the inspector or a "⋯" menu.
- Sub-tabs need padding.
- Empty-state text needs the same padding as other panel text.

**Note:** the offscreen screenshot of step 4 shows the same Delete/Unparent clipping, so the layout is the problem, not just the iced backend.

## 4. Gizmo drag does not move the entity

**Seen:** after selecting an entity, the move gizmo (red X arrow, green Y arrow, yellow centre square) is drawn in the game window. Pressing on a handle and dragging does nothing; the entity does not move.

**Expected:** pressing a handle starts a drag that writes `Transform` live. Release ends it with one undo entry. This works in the headless tests (`crates/ui/src/editor/tests_viewport.rs`).

**Where:** `crates/ui/src/editor/viewport.rs`, function `input`.
- A drag starts only when all of these hold:
  - `pointer_here` (the pointer window equals the editor camera's window, default `WindowId::MAIN`),
  - `gi.mouse_just_pressed(MouseButton::Left)`, where `gi = cx.input.game_input()`, which drops events a UI layer captured,
  - a `cursor` from `mouse_pos_in(wid)`,
  - `hit(...)` on the selected entity's gizmo geometry.
- During the drag it needs `cx.input.mouse_pressed(MouseButton::Left)` to stay true.

**Likely causes, to check in this order:**
1. **The UI captures the press in the game window.** With iced, a UI layer covering the game window (any game UI, world UI or an invisible full-window layer) may give a `Captured` verdict for pointer presses. `game_input()` then never sees `mouse_just_pressed`. The tests use the fallback or headless backend, where nothing covers the viewport.
2. **Window ids.** With the debug menu in its own OS window, `pointer_window()` or `mouse_pos_in(wid)` may report a different id for the main window than the editor camera's target. Another possibility: the focus-click on the game window is consumed. The first click after working in the debug menu window only focuses the game window.
3. **Hit-test scale.** The handle sizes are in screen pixels (`px = 1/zoom`). If the window's logical size and pixel size differ (HiDPI scale), the cursor position and the drawn gizmo may not line up, so `hit` misses the handle while the gizmo looks right.
4. **The world is not updated visibly.** The drag writes `Transform` while game systems are frozen in Edit mode. Check that `propagate_transforms` and `render_extract` still run, and that the edited entity's `GlobalTransform` updates.

**Needed:**
- A test on the iced backend, or at least with a UI layer that covers the game window.
- A test with the debug menu in a separate window and a HiDPI scale.
- Debug logging of the press path (pointer window, captured verdict, hit result) behind a flag.

## Summary

| # | Area | Severity | Fix size |
|---|---|---|---|
| 1 | Logs controls in the main tab bar | low (layout) | small |
| 2 | Editor panels not resizable | medium (usability) | medium (needs a splitter) |
| 3 | Overlapping and clipped text in the editor | medium (readability) | small to medium |
| 4 | Gizmo drag does nothing on iced | high (core editor feature) | unknown until the cause is found |

# egui 0.33.3 -> 0.37.0 upstream notes (collected 2026-10-16 by the design lead)

Source: emilk/egui CHANGELOG.md, crates/eframe/CHANGELOG.md, crates/egui-wgpu/CHANGELOG.md,
crates/egui-winit/CHANGELOG.md on GitHub main, read through a summarizing fetch tool.
These are paraphrased summaries, not verbatim. **The ground truth is the crate source** (paths below).
When a summary and the source disagree, trust the source and say so.

## Local crate sources (already in the cargo registry, read-only)

Registry root: `C:\Users\mikag\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\`

- old (unpatched upstream): `egui-0.33.3`, `eframe-0.33.3`, `epaint-0.33.3`, `emath-0.33.3`, `ecolor-0.33.3`,
  `egui-wgpu-0.33.3`, `egui-winit-0.33.3`, `egui_kittest-0.33.3`
- new: `egui-0.37.0`, `eframe-0.37.0`, `epaint-0.37.0`, `emath-0.37.0`, `ecolor-0.37.0`,
  `egui-wgpu-0.37.0`, `egui-winit-0.37.0`, `egui_kittest-0.37.0`, `kittest-0.5.0`
  (also `egui-0.36.2` etc. for reference)
- dependency resolution for 0.37.0 with Rust 1.99 (scratch project, `Cargo.lock` at
  `C:\Users\mikag\AppData\Local\Temp\claude\C--home-mimageviewer\add5444e-a7f5-431e-b8cf-4c065f50219f\scratchpad\egui036-fetch\Cargo.lock`):
  wgpu / wgpu-hal / wgpu-core 30.0.1 (mIV uses 27 today), winit 0.30.13, skrifa 0.44.0, vello_cpu 0.2.0,
  harfrust 0.12.0, accesskit_winit 0.34.1, windows 0.62.2 (check which windows version wgpu-hal 30 dx12 uses),
  kittest 0.5.0.
- The vendored, patched copies used by mIV today: `vendor/egui`, `vendor/eframe`, `vendor/egui-wgpu`,
  `vendor/egui-winit` (see each `PATCHES.md` / `README.md` "mImageViewer patch" section).

## Release dates / MSRV

0.34.0 2026-03-26 (MSRV 1.92), 0.35.0 2026-06-25, 0.36.0 2026-08-05 (MSRV 1.95), 0.36.2 2026-09-08,
0.37.0 2026-10-15 (MSRV 1.95). This worktree is pinned to Rust 1.99.0 (`rust-toolchain.toml`).

## egui 0.34

- Font rendering moved from `ab_glyph` to `skrifa` + `vello_cpu` (#7694); hinting and variable fonts (#7859).
- `Context::run` -> `Context::run_ui`, viewports receive `&mut Ui` (#7736, #7779). eframe `App::update`
  deprecated; replaced by `fn logic` and `fn ui` (#7775). `App::ui` runs only if the app is visible (#7950).
- `Ui` derefs to `Context` (#7770). `Context::style` -> `global_style` (#7772), several `Context` renames (#7773).
- Deprecated: `SidePanel`/`TopBottomPanel` (-> `Panel`, #5659), panels shown directly on `Context` (#7781),
  `CentralPanel::show` (#7783), `Context::used_size` / `available_rect` (#7788).
- accesskit always a dependency (#7701), accesskit 0.24 (#7850). Scroll bars / splitters visible to accesskit (#7804).
- Improved modifier handling when scrolling (#7678). Preferred font weight for variable fonts (#7790).
- Interaction changes: widgets moved to top of interaction stack (#7805), thin splitters (#7806),
  window resize interaction covers contents (#7807), widgets no longer expand on hover (#7808).
- Ctrl-Q quits the app (#7985) -- check whether this collides with any mIV binding or must be disabled.
- `ScrollArea` edges fade out (#8018) -- visual change.
- Fixed: Areas, Windows and ScrollAreas no longer take focus (#7827); Ctrl+arrow no longer moves focus (#7897);
  scroll area consumes scroll events (#7904); emoji icon font (#7940); `CentralPanel::show_inside` allocates
  space in parent (#7778); truncate() elision at non-100% scaling (#7867); every TextEdit/LayoutJob alignment
  works (#7831); horizontal_wrapping row height (#8000).
- Added: `ScrollArea::content_margin` (#7722), `ViewportInfo::occluded`/`visible` (#7948),
  `Context::text_edit_focused` (#8014), `Ui::is_tooltip` (#8016).
- egui-winit: "Much improved IME" (#7967) -- mIV's `vendor/egui-winit` was already reshaped to match this.
- egui-wgpu: wgpu 28 then 29 (#7853, #7990); `capture` behind a feature flag, `egui` dep optional (#7698);
  stencil buffer attached (#7702); error if `.render()` without `.update_buffers()` (#8005);
  wgpu memory leak when minimized fixed (#7928). 0.34.3: `wgpu::Surface` lifecycle fix for random hangs (#8171).
- eframe: wgpu default renderer (#7615); `wgpu_no_default_features` (#7700); invisible windows behavior (#7905);
  avoid repaints on device mouse motion outside the window (#7866). 0.34.3: winit window access on
  `eframe::Frame` / `CreationContext` (#8205); `app_id` defaults to `app_name` (#8172).
- 0.34.2/0.34.3 fixes: selected-text glyph color, centered/right-aligned selection and TextEdit,
  `scroll_to_*` with `stick_to_bottom` (#8033).

## egui 0.35

- Everything previously `#[deprecated]` removed (#8105).
- IME: improved handling + public `Memory::owns_ime_events` (#7983); IME composition visuals (underline/cursor) (#8083);
  IME interruption handling moved to integrations (#8078).
- `harfrust` shaping for kerning/ligatures (#8031).
- `Window` margins reworked, `clip_rect_margin` zero (#7725); Window moves only by dragging the title bar (#8183);
  can't resize a window past its minimum content size (#8152).
- Panels: slide when animating (#8175), method renames (#8192), double-click resize edge toggles (#8193),
  drag-to-close (#8182), never overflow width/max_width (#8198).
- Drag-to-scroll only on touch screens (#8181) -- check mIV touch/mouse drag scrolling.
- `AlphaFromCoverage` -> `FontColorTransferFunction` (#8201); `subpixel_binning` (#8072); hinting target in FontTweak (#8262).
- Typed `CharIndex` / `ByteIndex` (#8245) -- affects cursor / galley code.
- Added: modifier keys in `egui::Key` (#8127); `ViewportBuilder::with_monitor`, `ViewportCommand::SetMonitor` (#8140);
  `Context::set_cursor_image` (#8155); `AtomLayout::selectable` (#8224); `LayoutJob::format_at_byte` (#8244).
- Fixed: `lost_focus` after mid-frame focus transfer (#8210); ScrollArea jitter with floating bars and zoom (#7944);
  label selection in deferred viewports (#8242); "drunk text" (#8250).
- eframe/egui-wgpu: VSync and frame latency configurable at runtime (#8114); **wgpu low-latency by default (#8203)**;
  `pre_present_notify` before present (#8089); restored windows choose monitor by overlap (#8191);
  crash when parent viewport hidden fixed (#8226); glow config grouped (#8108); undecorated window shadows always on Windows (#8169).

## egui 0.36

- **`Modifiers` removed from `RawInput`; modifiers are now an `egui::Event` (#8336).**
- `clip_rect_margin` removed (#8366).
- Rerun `sizing_pass` when reopening a popup (#8315); a press that leaves a widget is treated as a drag (#8365).
- Added: `WidgetText::size`, `LayoutJob::clear`, window theme synced with egui theme (#8299), `Window::title_frame`,
  `extra_text_line_spacing` (#8040), drag-to-open collapsible panels (#8363).
- Fixed: non-interactive tooltips not interactable (#8362); panels account separator width (#8367);
  ScrollArea horizontal/vertical handled separately (#8275); Grid window shrink (#8386);
  `TexturesDelta` drop prevention (#8356).
- egui-wgpu: **wgpu 30** (#8289); wgpu `Instance` public (#8321); capture buffer unmap fix (#8337).
- eframe: never run an egui pass when nothing will be shown (#8387).
- 0.36.1: `Sense::drag` no longer detects drags when clicking a widget above it (#8396).
- 0.36.2: `TextEdit::event_filter` (#8530); `window_title_frame` falls back to `window_frame` (#8400);
  focused DragValue keeps external changes (#8403); thin angled rect rendering (#8482); NaN hit-test panic (#8479);
  TextEdit `min_size` (#8420); eframe stops busy-looping a core while waiting for redraw (#8398);
  hide drop-shadow decoration in fullscreen (#8449).

## egui 0.37 ("Colored emojis")

- Breaking: `default_fonts` is no longer a default feature (#8583); `ViewportCommand::Screenshot` uses a callback (#8489);
  `Id::new`, `AsId`, `From<&str> for Id` deprecated (#8534, #8586); new `Id::unique`, `IdSaltSet`, `IdSaltMap`
  (#8534, #8590); `Panel` constructors take `impl AsIdSalt` (#8592).
- Removed: `Font` borrow-bag (methods moved to `FontsImpl`) (#8504); `AsId` trait (#8584).
- Changed: `Ui::id` renamed `Ui::scope_id` (#8542); `WidgetRect::parent_id` uses `Ui::unique_id` (#8543);
  `ComboBox` built from atoms (#8618); `set_fonts`/`add_font` apply immediately when nothing laid out yet (#8602, #8609);
  `add_glyph_rasterizer` idempotent (#8600); Ui/Area/Popup/Tooltip named in accessibility tree (#8607);
  **kittest fails on input widgets without an accessible name (read-only text fields exempt) (#8589, #8611)**;
  `global_theme_preference_switch` deprecated (#8505); AccessKit bumped (#8496).
- Added: colored emojis; `RoundedRect`, `BandShape`; `StyleProvider` for TextEdit/DragValue (#8455);
  `RichText::color_override` (#8564); `Context::root_ui` / `run_pass` (#8535); `Context::set_repaint_observer` (#8644);
  `Plugin::on_exit` (#8527); `Response::container_*` (#8537); Windows/Mac-specific `Key`s (#8373);
  `KeyboardShortcut::conflicts_with_text_editing` (#8536); `MouseWheelSource` / `InputState::scroll_source()` (#8665);
  double/triple-click-and-drag selection (#8370); explicit popup sizing passes (#8407).
- Fixed: widget ID warnings across passes (#8316, #8670); popups no longer close immediately on mouse release (#7624);
  menus open without click when another menu is open (#7802); tooltips of container responses when hovering children (#7909);
  wheel scrolling while dragging inside ScrollArea (#7919); scrollbar interaction rect matches drawn bar (#8635);
  stuck scroll modifiers (#8636); Panel/Scene clip rect respects parent (#8562); double-click not judged triple (#7915);
  TextEdit layout shift with custom margin (#8628); `TextFormat::valign` with custom line_height (#8030);
  **CJK rows no longer start with full-width closing punctuation (#8658)**; fewer extra repaints before
  `request_repaint_after` deadlines (#8595).
- egui-winit 0.37: `Clipboard::new` unsafe (#7826); cut/copy/paste only without extra modifiers (#8668);
  **IME candidate window positioned at the text cursor (#8681)**; Cmd/Ctrl chosen from `Context::os()` (#8677);
  option to disable default clipboard shortcuts (#5615); `MouseWheelSource` (#8665).
- eframe 0.37: raw mouse motion delivered to the viewport under the pointer even if unfocused (#8405);
  Windows title bar refreshes after theme change (#8402); window settings saved only from root viewport (#8661);
  transparent viewport windows preserved on Windows (#8616); improved window restoration logic (#8687);
  `Frame::set_wgpu_surface_config` no longer ignored (#8442); clipboard feature optional (#8392).

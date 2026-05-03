# VST3 Chain Bridge Redesign

## Goal

Move from "one bridge process per VST3 plugin" to "one bridge process per
VST3 chain".

The mIV process remains isolated from plugin crashes. If one plugin crashes the
single VST3 bridge process can die, and mIV can rebuild the whole VST chain.
This is acceptable for the current product goal and avoids the z-order and IPC
costs caused by many independent bridge processes.

## Why

The current model has good crash isolation between plugins, but it creates a
separate top-level bridge surface per plugin process. Showing, hiding, and
changing topmost state has to be sent to every bridge process. With several
plugins this causes visible z-order flicker and multi-second hitches around
fullscreen/grid transitions.

One chain bridge gives us:

- One audio IPC roundtrip per block instead of one roundtrip per active plugin.
- One process foreground/activation model for all plugin editor windows.
- A single place where bridge-owned plugin surfaces can be reordered with
  `BeginDeferWindowPos`.
- Simpler app activation handling: one bridge process belongs to mIV.

## Target Architecture

```text
mimageviewer-core.exe
  DspBridge
    PluginSlot[0..N]
    ChainBridge: one child process + one shared-memory audio pipe

mimageviewer-vst3-host.exe
  Bridge
    vector<PluginLoader> loaders
    one AudioPipe
    audio_loop:
      input -> loader[0] -> loader[1] -> ... -> output
```

Rust still owns settings, slot order, bypass flags, user-hidden state, and GUI
window placement. The C++ bridge owns the VST3 objects and the framed plugin
editor HWNDs.

## Protocol Shape

Keep the existing `probe` command usable without a chain. Replace per-plugin
`open` with chain-level commands:

- `open_chain { sample_rate, block_size, shm_name, shm_size, sig_in, sig_out }`
  attaches the audio pipe and starts the audio thread.
- `add_plugin { slot_id, plugin_path, bypass, state? }` loads one plugin into
  the chain and returns `loaded { slot_id, plugin_name, latency_samples }`.
- `remove_plugin { slot_id }` unloads one plugin.
- `move_plugin { from, to }` reorders loaders.
- `set_bypass { slot_id, bypass }`.
- GUI/state commands gain `slot_id`: `query_gui_size`, `show_gui`,
  `hide_gui`, `set_gui_visible`, `set_gui_topmost`, `set_gui_app_active`,
  `notify_host_resize`, `set_user_resizing`, `query_state`, `restore_state`.
- Events from a plugin gain `slot_id`: `latency_changed`, `gui_attached`,
  `gui_detached`, `plugin_state`, `error`.

Compatibility note: this branch has not shipped the VST3 feature yet, so we can
change the internal bridge protocol without migration support for old bridge
processes. Scanner/probe can continue to spawn a short-lived bridge process.

## Runtime Semantics

- `process_block` is called once per audio block from Rust. The C++ audio thread
  applies all non-bypassed loaders in slot order.
- Total PDC is the sum of active loaders' latest latency samples. Rust keeps the
  summed value from per-slot events.
- Reset runs on the bridge audio thread and resets every active loader, then
  flushes silence for the summed latency.
- Query/restore state run on the bridge audio thread fence, keyed by `slot_id`,
  so they do not race plugin `process`.

## GUI Semantics

- Each plugin editor is now a bridge-owned top-level window with its own title
  bar and resize frame. The window is owned by the mIV main HWND, matching the
  Bitwig-style structure where the plugin host process owns the whole editor.
- Rust stores the returned editor HWND for placement, z-order, visibility, and
  user-hidden behavior. There is no separate Rust-side per-slot host window.
- The bridge can apply z-order/topmost changes to all surfaces in one internal
  batch. Rust no longer sends one topmost/visible command per process.
- App deactivation never hides plugin surfaces. It only drops topmost. Explicit
  VST OFF / per-slot close may still hide surfaces.
- The earlier SSL Meter Pro diagnostic hooks (`SetWindowSubclass`,
  `WH_MOUSE`, and WinEvent popup logging) are no longer installed. The framed
  editor HWND lives in the bridge process, so native plugin popup/menu ownership
  works without those hooks, and removing them avoids log storms during GUI
  show/hide and tooltip activity.

## Per-Plugin GUI Thread Follow-Up

The 2026-05 drag diagnostics show that the one-process chain bridge still has a
GUI hot spot: all attached editors share the bridge main STA thread. A visible
editor drag can therefore compete with hidden-but-attached plugin timers, paint
messages, tooltip/popup traffic, and private plugin messages from the rest of
the chain. Recent logs show `editor drag END ... max_gap_ms=250..296` even after
the editor owner is detached during drag, which points at message-pump
contention rather than owner z-order reconciliation.

Keep the chain bridge process and audio IPC unchanged, but move editor GUI work
to one STA thread per plugin slot:

```text
mimageviewer-vst3-host.exe
  bridge main/control thread
    stdin command queue
    chain state, loader list, z-order batching
    latency polling / events
  audio thread
    input -> loader[0] -> loader[1] -> ... -> output
  gui thread slot 0 (STA)
    createView / attached / removed / onSize
    editor HWND message pump
  gui thread slot 1 (STA)
    createView / attached / removed / onSize
    editor HWND message pump
  ...
```

Design rules:

- `PluginLoader` remains the owner of the VST3 component, processor, controller,
  state, and editor metadata. Audio processing stays serialized on the bridge
  audio thread as it is today.
- Every VST3 editor call (`createView`, `setFrame`, `attached`, `removed`,
  `getSize`, `onSize`, `setContentScaleFactor`) runs on that slot's GUI thread.
  This preserves VST3's STA-style editor expectations while isolating editor
  message queues.
- Each slot GUI thread calls `OleInitialize` at start and `OleUninitialize` at
  exit. `CoInitializeEx(COINIT_APARTMENTTHREADED)` alone is not enough for
  plugin editor features that use OLE services such as context menus,
  clipboard, drag-and-drop, or file dialogs.
- `CreateWindowExW` for the bridge-owned editor container also runs on the slot
  GUI thread. The thread that creates the HWND owns its WndProc/message queue;
  creating the HWND on the bridge main thread would keep drag/paint traffic on
  the old hot spot and defeat the refactor.
- Bridge control commands may synchronously marshal short GUI operations to the
  slot GUI thread when a reply is required (`query_gui_size`, first `show_gui`).
  Fire-and-forget operations (`set_gui_visible`, `set_gui_topmost`,
  `set_gui_app_active`, `notify_host_resize`) are posted and coalesced where
  possible.
- Synchronous GUI marshals are bounded. If a plugin blocks inside editor
  `attached`, `getSize`, or another required reply path, the bridge logs a
  `plugin GUI task timeout ... gui_tid=...` line and returns an error instead of
  blocking chain initialization or the control IPC thread indefinitely.
- A slot that times out in an editor operation is quarantined. Its stuck GUI
  helper is abandoned for the lifetime of the bridge process, future GUI
  commands for that slot become no-ops, and the rest of the chain may continue
  only while the bridge control thread keeps accepting IPC. We intentionally do
  not use `TerminateThread`; a plugin that
  never returns from `attached()` is leaked until bridge process exit rather
  than risking heap or DLL state corruption.
- Startup loading also uses bounded waits and logs both the Rust-side
  `[VST3 startup] loading ...` line and the bridge-side `add_plugin start ...`
  line. If a plugin blocks before it can emit `loaded`, the startup worker times
  out instead of waiting forever, and the log identifies the plugin path that was
  in flight.
- If an editor prewarm timeout leaves the bridge control thread unable to accept
  the next `add_plugin`, mIV disables the VST3 bridge for the rest of the
  session and stops startup loading. Playback must not keep a partially poisoned
  bridge in the active DSP path; the user can remove the bad plugin or restart
  to try the chain again. The playback-panel VST button reports this as a
  session stop with the offending plugin path instead of saying that VST3 is
  disabled in preferences.
- The bridge emits a watchdog heartbeat while it is running:
  `[BRIDGE main heartbeat] state=... current_cmd=... reader_state=...
  queue_size=... cmds_received=... cmds_processed=...`. This is intentionally
  produced by a separate watchdog thread so it keeps logging even if the bridge
  main thread is blocked inside a plugin callback or IPC handler.
- Void GUI mutations stay asynchronous. During a native editor move/resize
  modal loop, thread messages may not be drained until the drag ends, so
  `set_user_resizing`, visibility/topmost/owner changes, app-active updates,
  resize notifications, and drag diagnostics must not wait on the slot GUI
  thread.
- Do not use raw cross-thread `SendMessage` from the bridge main/control thread
  to a slot GUI thread. Use `PostMessage` plus a completion object, or
  `SendMessageTimeout` with a short timeout if a Win32 message is unavoidable.
  Slot GUI threads must report back to the bridge asynchronously so plugin
  callbacks such as `IPlugFrame::resizeView` cannot form a deadlock cycle.
- The bridge main thread may keep chain-level ordering state, but it must not
  directly call `IPlugView` methods. If it needs HWND information for z-order
  batching, it reads a small thread-safe snapshot from each slot.
- Hidden GUI prewarm is disabled by default. Audio/plugin state prewarm still
  runs during chain load, but editor `createView`/`attached(HWND)` now happens
  on the first visible show. Per-plugin STA hidden attach proved too fragile:
  SSL Meter Pro and the mIV latency test plugin can hang in `attached()` when
  the editor is prewarmed hidden. This keeps the bridge alive and preserves fast
  show/hide toggles after the first visible attach, at the cost of allowing
  plugins such as Insight 2 to report a latency change when their GUI is first
  opened.
- First visible show must not block the egui/UI thread. The fullscreen VST
  button shows/hides already-created editor HWNDs immediately, but missing
  editor HWNDs are attached from a background worker. This preserves the fast
  z-order toggle path after editors exist while preventing plugins that spend
  seconds in `attached()` from freezing video playback or the fullscreen UI.
- `query_state` / `restore_state` currently use the bridge audio-thread fence to
  avoid racing VST3 `process`. Keep that ownership explicit during the GUI
  thread refactor. If a plugin proves to require GUI-thread state I/O, add a
  stop-processing fence before marshalling state I/O to the slot GUI thread.
- Probe/scanner paths are unaffected because they do not create editor views and
  therefore do not start per-slot GUI threads.
- Drag diagnostics stay in place for validation. A good result is drag
  `max_gap_ms` dropping from the current 250-300ms range to ordinary frame-scale
  jitter, with no return of Alt+Tab white windows, white flicker, or thumbnail
  foreground stealing.

Implementation phases:

1. Add a small per-slot `GuiThread` helper in the C++ bridge. It owns an
   Ole-initialized STA, a Win32 message loop, a command queue, and the editor
   HWND created on that thread. Include `gui_tid` in the first logs so thread
   placement can be verified immediately.
2. Move `PluginLoader` GUI methods behind the helper, leaving non-GUI load,
   process, latency, and state methods on their current threads.
3. Replace direct chain-wide GUI mutation with snapshot reads plus posted
   per-slot commands. Keep `BeginDeferWindowPos` only for HWND operations that
   do not call back into `IPlugView`; otherwise marshal to the owning slot.
4. Update diagnostics to include `gui_tid` in editor create/drag/show logs so
   validation can prove that different plugins are no longer sharing one GUI
   thread.
5. Rebuild `vendor/vst3-host/mimageviewer-vst3-host.exe`, clear the extracted
   APPDATA bridge cache, and validate with the same multi-plugin drag scenario.

Future work: add a lightweight heartbeat per slot GUI thread. If a slot does
not advance for several seconds, the bridge can log the frozen slot/plugin name
while other plugin editor threads keep running.

## Control Message Size

Plugin state snapshots can be several megabytes for analyzer/limiter plugins.
The bridge protocol allows control messages up to 32 MiB. If a future plugin
exceeds that limit, Rust drains the oversized payload before reporting the
error so the length-prefixed stdout stream remains synchronized for later
events.

## Migration Plan

1. Add chain protocol and C++ `loaders_` container while keeping probe intact.
2. Update Rust `DspBridge` so all slots share one chain bridge and one audio
   pipe. Keep public `DspBridge` APIs stable for UI/audio callers.
3. Move audio processing from Rust-side per-slot loop to one chain bridge call.
4. Add slot-id GUI/state command routing.
5. Delete the obsolete per-plugin bridge paths and update docs.

## Risks

- A plugin crash drops the whole chain bridge. mIV must detect bridge EOF and
  mark/rebuild the chain.
- Some plugins may assume a dedicated process for GUI globals. This is rare;
  most DAWs host many plugins in one process when plugin sandboxing is disabled.
- The protocol change is large. Keep commits small: protocol/C++, Rust audio
  routing, GUI routing, cleanup.

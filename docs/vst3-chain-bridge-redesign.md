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

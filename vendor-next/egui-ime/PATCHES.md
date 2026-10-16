# egui-ime 0.37 preparation copy

## Provenance and ownership

This is a real file copy of `crates/egui-ime` at repository HEAD
`2b8f779a8916fdf8241e878a1a8056da30e19b4a` (stage 2a-1, 2026-10-17).
It is the application's shared input policy, not an upstream egui crate.
Only the standalone `vendor-next` preparation graph uses this copy. The root
workspace, product `crates/egui-ime`, and released 0.33 graph remain unchanged.
No junction, symlink, or other reparse point is used.

## Migration delta

- Standalone Rust 2024 / Rust 1.99 manifest depends on egui 0.37 with default
  features disabled. Only the dev dependency enables `default_fonts`.
- The existing production normalizer and viewport composition owner are
  retained. Preedit uses `text` and `active_range_chars`; synthesized empty
  cancellation preedits have no active range. Incoming ranges pass through.
- `DeleteSurrounding` leaves the current composition and focus-loss terminal
  obligation unchanged, including pending native clipboard-gate projection.
- Deprecated Enabled/Disabled notifications remain an explicit compatibility
  boundary for Windows cancellation. Reasoned deprecation allowances are
  limited to the functions and tests that use those notifications.
- The input plugin accepts the new Context argument. Tab's begin-pass callback
  receives the root Ui. Globally unique policy/test IDs use `Id::unique`.
- All existing library fixtures are ported to struct Preedit, `run_ui` and Ui
  panel APIs. Every headless FullOutput is explicitly discarded with
  `drop_without_applying_deltas`. Additional fixtures preserve character ranges
  and surrounding-delete composition semantics.
- `src/clipboard.rs` is byte-for-byte identical to the product source. Existing
  winit/native shortcut differences, modifier truth table, I/O ownership, and
  Process/IME/release gates remain unchanged.

No new repaint, pending state, viewport interruption ownership, or platform
output aggregation behavior is introduced here. Existing focus recovery and
its repaint are retained; upstream overlap is reserved for the later plan.
Parent-owned producer regressions must run actual winit State conversion
through this normalizer and a real headless TextEdit before other patch ports.

## Stage 3 consolidation

1. Reconcile this copy against any intervening changes to `crates/egui-ime`;
   keep a single source owner and preserve new policy/test changes on both sides.
2. Apply the reviewed migration delta to the product `crates/egui-ime` when
   switching the root graph to egui 0.37. Keep minimal normal features and dev
   fonts distinct, and route product/backend dependencies to that single crate.
3. Reconnect producer-to-normalizer-to-TextEdit cancellation fixtures to the
   product graph, run the planned shared-policy/backend gates, and verify the
   clipboard source against the approved stage 1b-3c policy.
4. Remove the preparation copy and its workspace/patch references after all
   preparation consumers have moved; do not leave two policy implementations.
   Do not remove Enabled/Disabled solely to silence deprecation warnings.

## Stage 2c-2 lifecycle delta (2026-10-17)

Internal implementation verification (実装担当の内部確認), not independent acceptance.
The existing output hook now prunes the policy immediately after normal Context
viewport GC, using the complete per-pass live set before FullOutput aggregation.
Input-hook pruning alone cannot observe removal followed by same-ID reuse when
only run_logic occurs between them. Hidden/minimized live viewports are retained.
Scoped cleanup removes the ended owner's composition, focus contract, and its
known field retry. Field replacement/forget also ends the old recovery claim so
no undrawn retry escapes its existing contract owner. No new registry or queue
is introduced; shared widget data and sibling policy are preserved.

Two headless regressions cover actual Context GC → run_logic → same-ID reopen
with stale input metadata and a reset pass counter, and field replacement while
an old retry is undrawn. Explicit pass-free Close retirement is not connected by
eframe yet; its policy cleanup remains stage 2c-4 work. Carry this lifecycle delta
and both regressions into the single product policy at stage 3 consolidation.

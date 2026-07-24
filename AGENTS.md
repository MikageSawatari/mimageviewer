# mimageviewer - Codex Instructions

This file is the compact Codex entry point for this repository. `CLAUDE.md`
contains the broader project context and operational notes; read only the
sections relevant to the current task instead of treating the whole file as
always-active guidance.

## Before Editing

- Start by identifying the affected area and read the matching docs from
  `docs/README.md`.
- For architecture-sensitive changes, also read
  `docs/architecture-overview.md`.
- If a task touches display, fullscreen, thumbnail loading, folder scanning,
  virtual folders, presets, adjustments, AI features, or UI responsiveness,
  read the related document listed near the top of `CLAUDE.md` in the
  required-before-work section.
- If a task touches keyboard shortcuts, key events, or shortcut documentation,
  read `docs/keymap-spec.md` and `docs/key-customization-impl-plan.md`. New
  keyboard operations should go through `KeyAction` / keymap helpers unless the
  operation is intentionally fixed and documented as outside keymap scope.
- On Japanese Windows, PowerShell 5.1 may mojibake UTF-8 files without a BOM.
  When reading repository documents through PowerShell, use
  `Get-Content -Encoding UTF8` explicitly.
- Check the current git status before editing. Do not revert or overwrite
  unrelated user changes.
- In separate worktrees, do not create junctions, symlinks, or other reparse
  point links for `vendor/`, `target/`, or runtime dependency directories. If a
  worktree needs those files, copy the real files/directories into that
  worktree or run the setup scripts there. Before removing a worktree or copied
  dependency directory, verify the target path and do not recurse through
  reparse points.

## Core Engineering Rules

- Keep UI work responsive. Do not add synchronous I/O, heavy decoding, folder
  scans, GPU uploads, or blocking waits on the UI thread.
- Do not implement waiting with `try_lock` plus `sleep`. Move blocking work to a
  worker, use channels, or make state transitions explicit.
- Preserve thumbnail, fullscreen, ZIP/PDF virtual folder, preset/adjustment, and
  AI workflows when changing shared data structures.
- Treat path handling, archive extraction, external tools, and metadata writes
  as security-sensitive. Avoid trusting archive paths or user-provided paths
  without validation.
- Follow existing Rust, egui, and module patterns before introducing new
  abstractions.
- Avoid temporary workaround fixes for correctness-sensitive behavior. Choose a
  design that can be made fundamentally correct; if the correct design is larger
  than expected, document the scope and proceed in coherent steps rather than
  landing a stopgap that is likely to create follow-up bug reports.
- Do not remove, disable, delay, or degrade existing user-facing functionality as
  a bug fix or temporary workaround without explicit user approval. If a correct
  fix appears to require a behavior change, explain the trade-off and ask before
  editing. Bug fixes should preserve the intended feature set unless the user has
  approved the functional change.

## Bug Fix Policy

- Before changing code for a bug, identify the observed failure, the expected
  invariant, and the code path that violates it. Use logs, traces, tests, and
  source inspection to confirm the root cause instead of patching the most
  visible symptom.
- When a bug crosses shared state, multiple input entry points, asynchronous
  completion paths, or multiple viewer contexts, do not stop at the reproduced
  path. Inventory equivalent producers, consumers, and the
  open/switch/close/cancel/error lifecycle, then check sibling routes for the
  same broken invariant before editing.
- Fix the root cause at the ownership boundary where the incorrect state or
  transition is created. Avoid adding guards, delays, retries, extra repaint
  calls, blanket resets, or silent fallbacks unless they are part of the root
  cause fix and their invariants are documented.
- Do not add another bool, `Option`, pending field, or field-presence sentinel
  to represent a mutually exclusive state that is already split across fields.
  Prefer one typed request/state owner and route equivalent entry points through
  it. If that restructuring is larger than the current scope, stop and report
  the architectural boundary instead of adding another branch.
- For context-scoped resources such as items, caches, textures, queues,
  channels, generations, cancellation tokens, and workers, verify that create,
  mutate, drain, cancel, invalidation, and drop affect only the owning context.
  Read-only open/close must not publish mutation-style invalidation or reset an
  unchanged sibling context. Add cross-context regression coverage when these
  boundaries change.
- If the investigation shows that the correct fix is larger than the current
  scope, stop and explain the trade-off to the user before editing further.
  Offer coherent options, such as spending more time on the architectural fix,
  splitting the work into reviewed phases, or explicitly changing the
  user-facing specification to avoid the problematic behavior.
- Do not land a temporary behavior change, feature restriction, or partial
  workaround just to pass one manual smoke test unless the user explicitly
  approves that trade-off. When an approved mitigation is necessary, document
  what remains unresolved and how the final fix should replace it.
- Add regression coverage at the level where the bug happened whenever practical:
  pure state-transition tests for state bugs, handler-level tests for input
  routing bugs, and focused integration or log-based checks for lifecycle and
  multi-window behavior that cannot be reproduced in unit tests.

## UI And Tests

- For display, scroll, fullscreen, dialog, or layout changes, check
  the `CLAUDE.md` sections for UI/scroll behavior, `egui::Window` dialogs, and
  UI responsiveness review points as needed.
- Preserve Japanese IME behavior. For text input changes, read the
  `CLAUDE.md` IME section first.
- Changes that affect visible UI should include or update snapshot coverage
  when practical. See the UI snapshot test section in `CLAUDE.md`.
- Run the narrowest relevant tests first, then broaden when the change touches
  shared behavior.
- During ordinary edit/test iteration, do not default to
  `cargo test --workspace`. Prefer `cargo check -p mimageviewer --bin
  mimageviewer-core`, `cargo test -p mimageviewer --lib <filter>`, or a
  specific `--test <name>`. Run `.\scripts\test-full.ps1` for cross-cutting
  shared changes, final verification, release preparation, or when the user
  explicitly requests the full gate. See
  `docs/development-build-and-test.md` for the command matrix.
- Allow enough command time for this repository's Rust builds and tests. A
  build or initial test compile can take five minutes or more. For agent-run
  commands, use at least a 10-minute timeout for `cargo check`, `cargo build`,
  and release builds, and at least a 15-minute timeout for broad or full
  `cargo test` runs; increase these further for cold or clean builds. Use a
  shorter timeout only for a genuinely narrow test after its dependencies are
  already built.
- Do not classify `BrokenPipe`, channel-send, or test-harness errors produced
  after the command runner timed out as product test failures. Confirm whether
  the timeout interrupted the process, then rerun once with a sufficient
  timeout instead of repeatedly retrying with the same limit.

## Verification Build Handoff

- After completing a user-requested application or runtime behavior change,
  run the relevant automated checks, then build a user-runnable verification
  binary with `.\scripts\build-dev.ps1` before the final handoff. Do not stop
  at `cargo check` or tests when the user could reasonably confirm the result
  in the application.
- Do not launch `target\dev-runtime\mimageviewer-core.exe` yourself. In the
  final response, give the user the exact PowerShell launch command:
  `Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`.
  State that its isolated data directory is `target\dev-runtime\data`, and list
  the concrete scenario to verify.
- A verification binary is not required for documentation-only, test-only,
  build-script-only, or other non-runnable changes, when the user explicitly
  asks not to build one, or when prerequisites are unavailable. Report the
  reason when a normally expected verification build could not be produced.
- Release-only behavior and the Windows-native cases below override the normal
  `build-dev.ps1` handoff: build the release launcher/core and give the user
  its exact launch command and any real-settings warning instead.

## Verification Builds (Windows native features)

- **Never launch** `target\release\mimageviewer.exe`,
  `target\release\mimageviewer-core.exe`, an installed mImageViewer, or an
  `%APPDATA%\mimageviewer\runtime\*` executable as part of agent-driven UI or
  Computer Use verification. Normal builds use the user's real
  `%APPDATA%\mimageviewer` data and may migrate, rotate, quarantine, or rewrite
  `settings.db` merely by starting. Building these binaries and handing them to
  the user is allowed; launching them is not.
- Agent-driven UI verification must use a disposable portable copy prepared by
  `.\scripts\prepare-portable-smoke.ps1`. Launch only
  `target\portable-smoke\mimageviewer.exe` and verify that its data directory is
  `target\portable-smoke\data`. Do not reuse a user's portable installation or
  copy the user's normal settings into the smoke directory.
- If a scenario specifically requires the user's real configuration, stop after
  building and give the user concrete manual steps. Only the user launches the
  normal verification binary. Do not reset, rename, restore, or otherwise
  manipulate the normal settings files for test setup.
- When a change affects Windows-native behavior that unit tests cannot cover
  (native video presenter, fullscreen, video->audio mode, VST, D3D11, HWND
  owner/focus/z-order, real IME behavior, multi-monitor DPI), do not merely ask
  the user to verify on real hardware. First run `.\scripts\build-release.ps1`
  yourself to produce the verification binary (`target\release\mimageviewer.exe`
  launcher + `target\release\mimageviewer-core.exe`), then give the exact
  PowerShell launch command:
  `Start-Process -FilePath .\target\release\mimageviewer.exe`.
  Include concrete verification steps and warn that this build uses the user's
  real `%APPDATA%\mimageviewer` data.
- Prerequisite: `cargo build` / `cargo test` green, `cargo fmt --check` clean,
  and (for UI string changes) `python scripts/check_ui_glyphs.py` reporting zero.
  Do not build a verification binary on top of a non-compiling tree.
- Use `build-release.ps1` (fast, incremental; it auto-stops a resident mIV to
  avoid LNK1104), not the distribution `build-dist.ps1`. Do not append `*>&1`
  when invoking it from a tool: PowerShell `-ErrorAction Stop` turns cargo stderr
  into a terminating error and fails instantly. Call the script plainly, or run
  the two `cargo build` stages (core, then launcher) directly.
- For these native features, commit after the user confirms on real hardware,
  following the git rules below.

## Documentation And Release Notes

- When behavior, architecture, user-facing UI, setup, or release procedures
  change, update the matching docs alongside the code. See the documentation
  update section in `CLAUDE.md`.
- For dependency DLLs, PDFium, ONNX Runtime, FFmpeg, Susie, VST3, distribution,
  or release work, consult the corresponding `CLAUDE.md` sections before
  editing or building.
- For release tasks, follow the checklist in `CLAUDE.md` rather than relying on
  memory.

## Git

- Keep commits focused and do not include unrelated untracked files.
- When the user asks to commit, make the requested commit and make sure it is
  merged into the local `master` branch as part of the same task, unless the
  user explicitly asks to leave it on a feature/worktree branch or the merge is
  blocked by conflicts or unrelated dirty worktree changes. Do not push or open
  a PR unless explicitly requested.
- Prefer committing completed, coherent chunks promptly, even before the user
  starts manual verification, when the change has been implemented and the
  relevant automated checks pass. This keeps rollback points easy to identify.
- For long-running work, avoid letting many unrelated fixes accumulate in the
  working tree; split them into focused commits as soon as each piece is safe.
- Follow the repository's existing git workflow notes in `CLAUDE.md` when a task
  involves review, release, or multi-step coordination.

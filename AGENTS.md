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

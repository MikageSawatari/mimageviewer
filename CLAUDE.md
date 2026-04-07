# mimageviewer - Project Context

## Overview

A Windows 11 native image viewer built in Rust. Inspired by ViX (legacy 32-bit viewer),
modernized with GPU acceleration and AI upscaling. Single-window design replacing ViX's
dual-window approach.

## Tech Stack

- **Language**: Rust (latest stable)
- **GUI**: eframe + egui (wgpu backend)
- **Image decoding**: `image` crate (JPEG, PNG, WebP, BMP)
- **Parallel loading**: `rayon`
- **GPU upscaling (fullscreen)**: NVIDIA NGX DLISR via C FFI (Phase 2)
- **Build tool**: cargo (MSVC toolchain on Windows)

## Project Structure

```
mimageviewer/
├── CLAUDE.md
├── docs/
│   └── spec.md
├── src/
│   ├── main.rs
│   ├── app.rs          # top-level App state and eframe impl
│   ├── ui/
│   │   ├── mod.rs
│   │   ├── toolbar.rs  # menu bar + address bar
│   │   ├── sidebar.rs  # favorites panel
│   │   ├── grid.rs     # virtual-scroll thumbnail grid
│   │   └── fullscreen.rs
│   ├── loader.rs       # parallel thumbnail loading
│   ├── upscale.rs      # upscaling (simple + NGX DLISR)
│   └── settings.rs     # persistent settings (JSON)
├── Cargo.toml
└── Cargo.lock
```

## Implementation Phases

1. **Phase 1** — Core viewer: address bar, thumbnail grid, fullscreen display, keyboard nav
2. **Phase 2** — AI upscaling: NVIDIA NGX DLISR for fullscreen view
3. **Phase 3** — Favorites: register/list/navigate favorite folders

## Key Design Decisions

- **Virtual scrolling**: Only render visible thumbnail rows + 2-row buffer above/below.
  Total scroll height is pre-calculated from file count and grid dimensions.
- **Thumbnail loading**: On folder open, get file list immediately, pre-calculate layout,
  show empty frames, then fill with rayon parallel decode + channel to main thread.
- **Grid contents**: Folders first (alphabetical), then image files (alphabetical). Non-image
  files are ignored entirely. Folders are shown as thumbnails with a folder icon.
- **Folder tree navigation (Ctrl+↑↓)**: Depth-first pre-order traversal of the filesystem
  tree. Next = first child if exists, else next sibling, else parent's next sibling (recurse).
- **Upscaling split**: Simple bicubic for thumbnails; DLISR AI only for fullscreen.
- **Security**: `image` crate (pure Rust, memory-safe) for decoding. No WIC dependency.
- **Fullscreen**: Separate borderless window at monitor resolution, not a fullscreen mode
  of the main window.

## Supported Image Formats

JPEG, PNG, WebP, BMP

## Settings (persisted as JSON)

- Thumbnail grid columns (default: 4)
- Thumbnail grid rows (default: 3)
- Favorites folder list

## User: Background

- Comfortable reading C++ but not familiar with Rust's borrow checker details
- Has RTX 4090
- AI-assisted development workflow: Claude generates code, user reviews and tests

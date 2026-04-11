# mImageViewer

**Fast, GPU-accelerated image viewer for Windows 11**  
Windows 11 向け高速サムネイルビューワー

[![Download](https://img.shields.io/github/v/release/MikageSawatari/mimageviewer?label=Download&color=d63384)](https://github.com/MikageSawatari/mimageviewer/releases/latest)
[![Platform](https://img.shields.io/badge/platform-Windows%2011-0078d4)](https://github.com/MikageSawatari/mimageviewer/releases/latest)
[![License](https://img.shields.io/badge/license-Freeware-ff60c0)](https://github.com/MikageSawatari/mimageviewer/releases/latest)

---

## Download / ダウンロード

**[→ Latest Release](https://github.com/MikageSawatari/mimageviewer/releases/latest)**

- No installer required — just run the exe / インストール不要、exe をそのまま実行
- Freeware / 無料

For full documentation in Japanese: **[mikage.to/mimageviewer](https://mikage.to/mimageviewer/)**

---

## Features / 主な機能

- **GPU-accelerated thumbnail grid** — SQLite + WebP cache for instant reloading
- **Wide format support** — JPEG, PNG, GIF, WebP, BMP, HEIC, AVIF, JPEG XL, TIFF, RAW (Canon, Nikon, Sony, Fujifilm, and more)
- **AI image metadata** — Reads Stable Diffusion (A1111/Forge), ComfyUI, and Midjourney prompts from PNG files
- **EXIF viewer** — Camera, lens, exposure, GPS info with customizable tag filters
- **Video thumbnails** — MP4, AVI, MOV, MKV, WMV and more (via Windows Shell API)
- **Animated GIF / APNG** playback in fullscreen
- **ZIP archive** browsing — view images inside ZIP files without extraction
- **Non-destructive rotation** — saved to SQLite, original files untouched
- **Folder tree navigation** — Ctrl+↑↓ traverses folder tree depth-first
- **Slideshow** — fullscreen, configurable interval (0.5–30 sec)
- **Metadata search** — Ctrl+F to search AI prompts and filenames within a folder
- **Favorites** — named bookmarks for frequently visited folders
- **Customizable toolbar** — show/hide sections and individual buttons

---

## System Requirements / 動作環境

| | |
|---|---|
| OS | Windows 11 (x64) |
| GPU | DirectX 12 compatible (for GPU acceleration) |

### Optional codecs for WIC formats

| Format | Codec |
|---|---|
| HEIC / HEIF | [HEIF Image Extensions](ms-windows-store://pdp/?ProductId=9pmmsr1cgpwg) (usually pre-installed) |
| AVIF | [AV1 Video Extension](ms-windows-store://pdp/?ProductId=9mvzqvxjbq9v) |
| JPEG XL | [JPEG XL Image Extension](ms-windows-store://pdp/?ProductId=9mzprth5c0tb) (Windows 11 24H2+) |
| RAW | [Raw Image Extension](ms-windows-store://pdp/?ProductId=9nctdw2w1bh8) |

---

## Keyboard Shortcuts / キーボード操作

### Thumbnail Grid

| Key | Action |
|---|---|
| Arrow keys | Move selection |
| Enter | Open folder / fullscreen |
| Backspace | Go to parent folder |
| Ctrl + ↑ / ↓ | Previous / next folder in tree |
| Ctrl + Wheel | Change column count |
| Space | Toggle check (multi-select) |
| R / L | Rotate right / left |
| Delete | Move to Recycle Bin |
| Ctrl + F | Metadata search |

### Fullscreen

| Key | Action |
|---|---|
| ← / → or Wheel | Previous / next image |
| Space | Play / pause slideshow |
| I / Tab | Toggle metadata panel |
| R / L | Rotate right / left |
| Escape / Right-click | Close fullscreen |

---

## Release Notes / 更新履歴

### v0.3.0
- AI image metadata panel (Stable Diffusion / ComfyUI / Midjourney prompts)
- EXIF viewer with tag filter settings
- Slideshow with configurable interval
- Non-destructive rotation (SQLite)
- Metadata keyword search (Ctrl+F)
- Right-click context menu
- Multi-select (Space / Ctrl+click)
- Duplicate file handling settings
- Scrollbar drag support
- Home / End / PageUp / PageDown keys

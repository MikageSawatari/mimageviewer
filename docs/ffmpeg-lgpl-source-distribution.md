# FFmpeg LGPL Source Distribution Notes

Last updated: 2026-07-18

mImageViewer bundles BtbN FFmpeg LGPL shared DLLs under `vendor/ffmpeg/`.
The current build is LGPLv3-or-later because the bundled DLLs contain
`--enable-version3` and report `LGPL version 3 or later`.

This file is a release checklist, not legal advice. Before each public release,
run `scripts/collect-ffmpeg-lgpl-info.ps1` and archive its output with the release
materials.

## Current Policy

- Use only BtbN `*-lgpl-shared-*` assets.
- Do not use GPL or nonfree FFmpeg builds.
- Keep FFmpeg dynamically linked as DLL files at runtime.
- Keep `vendor/ffmpeg/LICENSE.txt` in the distribution.
- Show an FFmpeg LGPLv3-or-later notice in the application and installer readme.
- Allow users to replace compatible FFmpeg DLLs.
- Provide corresponding source information for FFmpeg and bundled external libraries.

## Required Release Artifacts

| Artifact | Purpose |
| --- | --- |
| `vendor/ffmpeg/VERSION` | Exact BtbN asset name used for the release. |
| `vendor/ffmpeg/LICENSE.txt` | LGPLv3-or-later license text from the BtbN asset. |
| FFmpeg source archive or source URL | Corresponding FFmpeg source for the bundled build. |
| External library source/license list | Source and license references for libraries enabled in the BtbN build. |
| `collect-ffmpeg-lgpl-info` report | Auditable configure flags and checklist output for the release. |

## Bundled FFmpeg And External Libraries

The exact enabled list must be generated from the release DLLs. The current BtbN
build has been observed to include at least the following external libraries that
matter for decode/encode paths:

| Component | Typical license | Source reference |
| --- | --- | --- |
| FFmpeg | LGPLv3-or-later for this build | https://ffmpeg.org/releases/ |
| libsvtav1 | BSD-3-Clause + patent notes | https://gitlab.com/AOMediaCodec/SVT-AV1 |
| libaom | BSD-2-Clause + AOM patent license | https://aomedia.googlesource.com/aom/ |
| librav1e | BSD-2-Clause | https://github.com/xiph/rav1e |
| libdav1d | BSD-2-Clause | https://code.videolan.org/videolan/dav1d |
| libopus | BSD-3-Clause | https://github.com/xiph/opus |
| libvpx | BSD-3-Clause | https://chromium.googlesource.com/webm/libvpx |
| libvorbis | BSD-style | https://gitlab.xiph.org/xiph/vorbis |
| libsoxr | LGPLv2.1-or-later | https://sourceforge.net/projects/soxr/ |
| libopenh264 | BSD-2-Clause + patent notes | https://github.com/cisco/openh264 |
| libmp3lame | LGPL | https://lame.sourceforge.io/ |

When updating BtbN assets, verify the configure string and update this table if
new `--enable-lib*` entries appear.

## Notice Template

Use this wording in user-facing software information:

```text
This software uses libraries from the FFmpeg project (https://ffmpeg.org/)
under the LGPLv3-or-later.
FFmpeg version: <vendor/ffmpeg/VERSION>
Source: https://mikage.to/mimageviewer/ffmpeg-<VERSION>-source.tar.xz
Source and external library notes: https://mikage.to/mimageviewer/
License: https://www.gnu.org/licenses/lgpl-3.0.html
```

## Offline Video Upscale Encoding Notes

The offline video upscale feature should use the bundled LGPL shared FFmpeg DLLs.
For MVP, the intended encoder is `libsvtav1` in a Matroska container. Do not add
GPL encoders such as x264/x265 unless the project licensing and distribution model
are deliberately changed.

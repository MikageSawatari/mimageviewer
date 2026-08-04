# FFmpeg LGPL Source Distribution Notes

Last updated: 2026-08-04

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

## Corresponding Source

BtbN builds from a specific FFmpeg commit, not from a release tag. The bundled
DLLs report `n<tag>-<n>-g<hash>`, where `<n>` commits sit on top of `<tag>`.
Publishing only the release tarball for `<tag>` therefore does **not** provide
the corresponding source for the build we ship.

Fetch the exact commit instead:

```bash
# hash comes from the g<hash> part of vendor/ffmpeg/VERSION
curl -sSL --fail -o htdocs/mimageviewer/ffmpeg-<BUILD-ID>-source.tar.gz \
  https://github.com/FFmpeg/FFmpeg/archive/<hash>.tar.gz
```

`<BUILD-ID>` is the version string from the DLLs, e.g.
`ffmpeg-n7.1.5-12-g1fdbca85aa-source.tar.gz`. Verify after downloading:

- `tar tzf <file> | head -1` shows the full commit hash in the top-level
  directory name, which must match the short hash in `vendor/ffmpeg/VERSION`
- `tar xzf <file> -O <dir>/RELEASE` shows the base release version
- record the `sha256sum` with the release materials

Keep the tarballs of previous releases in place. Users of an older
mImageViewer are entitled to the source matching *their* build.

### Where The Tarball Lives

The tarball is a distribution artifact, not repository source, so it is **not tracked in
git** (`.gitignore` excludes `htdocs/mimageviewer/ffmpeg-*-source.tar.*`). At 16 MB per
FFmpeg update it would grow the history permanently for no benefit — git is not the
distribution channel; mikage.to is.

Three copies matter, and they are not interchangeable:

| Copy | Purpose |
| --- | --- |
| mikage.to | The one users are entitled to. Must stay up for as long as that build is in the wild. |
| `C:\home\mimageviewer_vendor_backup\ffmpeg-lgpl-source\` | The exact published bytes, kept outside the repo tree. |
| `.sha256` (tracked in git) | The record of what was published. Tiny, so it stays in history. |

Re-fetching from `https://github.com/FFmpeg/FFmpeg/archive/<hash>.tar.gz` recovers the
same *source*, but **GitHub's commit archives are not byte-stable** (their compression has
changed before), so a fresh download may not match the published `.sha256`. That is not a
licence problem — the content is identical — but it does mean the backup copy is what
keeps the published checksum verifiable.

Tarballs committed before this rule stay tracked; removing them needs a history rewrite
and is a separate decision.

### Two Places Carry The Build ID

The in-app notice derives its text from `vendor/ffmpeg/VERSION` at build time
(`MIV_FFMPEG_BUILD_ID` in `build.rs`, rendered by `src/ui_dialogs/about.rs`), so it
cannot drift. The product page section does **not**: `htdocs/mimageviewer/index.html`
("サードパーティ ソフトウェア（FFmpeg / LGPL）") is hand-written and must be edited
whenever FFmpeg is updated.

It drifted twice before it was caught on 2026-08-04: the page still listed
`n7.1.5-2-g998de74adf` while v2.11.0 shipped `n7.1.5-10-g2aefd64d48`, and it linked
the `7.1.5` release tarball instead of the commit-exact archive the in-app link points
at. Bump that section together with `vendor/ffmpeg/VERSION`, and move the outgoing
entry into the "以前のバージョン向けの対応ソース" list rather than deleting it.

## Notice Template

Use this wording in user-facing software information:

```text
This software uses libraries from the FFmpeg project (https://ffmpeg.org/)
under the LGPLv3-or-later.
FFmpeg version: <vendor/ffmpeg/VERSION>
Source: https://mikage.to/mimageviewer/ffmpeg-<BUILD-ID>-source.tar.gz
Source and external library notes: https://mikage.to/mimageviewer/
License: https://www.gnu.org/licenses/lgpl-3.0.html
```

## Offline Video Upscale Encoding Notes

The offline video upscale feature should use the bundled LGPL shared FFmpeg DLLs.
For MVP, the intended encoder is `libsvtav1` in a Matroska container. Do not add
GPL encoders such as x264/x265 unless the project licensing and distribution model
are deliberately changed.

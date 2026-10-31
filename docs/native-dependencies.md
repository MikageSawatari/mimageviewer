# ネイティブ依存物の管理 (vendor / PDFium / ONNX Runtime / Susie / FFmpeg / LibRaw / VST3)

> この文書は 2026-10-15 に CLAUDE.md から移した節をそのまま収めたもの。本文中に残る「本書」は移動前の CLAUDE.md を指す。移した節の行き先一覧は CLAUDE.md の「移した節の行き先」表を参照。

## vendor/ 一括セットアップ (新規 clone / vendor/ 消失時の復旧)

`vendor/` の DLL / SDK / モデル等は `.gitignore` 対象。ローカル patch 用の Rust crate は
ソースを git で追跡する（`libmimalloc-sys` 等）。
新規 clone した直後や、誤って `vendor/` を消してしまった場合は、ビルド前に必要
ファイルを揃え直す必要がある。`build.rs` が起動時に必須ファイルの存在を検証して
復旧手順付きでエラーを出すので、**何が足りないかは cargo build のエラーで分かる**。

### ワンコマンドで揃える

```bash
bash scripts/bootstrap-vendor.sh           # 不足分のみ取得
bash scripts/bootstrap-vendor.sh --force   # 既存ファイルも再取得 (デバッグ用)
```

上の bash はエージェント自身が実行するセットアップ手順であり、利用者へ渡すコマンドではない。
このスクリプトは以下を順に実行する:

1. `setup-pdfium.sh` — `vendor/pdfium/bin/pdfium.dll` を取得
2. `setup-libraw.sh` — `vendor/libraw/` に LibRaw 0.22.3 と zlib 1.3.1 のソース、`VERSION` を取得
3. `setup-libavif.sh` — `vendor/libavif/` に libavif / dav1d / libyuv の静的ライブラリ・ヘッダ・notice・`VERSION` を作成
4. `setup-ort.sh` — `vendor/ort/onnxruntime*.dll` を取得
5. `setup-ffmpeg.sh` — `vendor/ffmpeg/{bin,include,lib}/` を取得
6. `setup-susie-worker.sh` — `vendor/susie-worker/mimageviewer-susie32.exe` を再ビルド
7. `setup-twemoji.sh` — `vendor/twemoji/svg/*.svg` を取得 (注釈スタンプの絵文字。
   build.rs が exe へ `include_bytes!` で同梱。未配置でもビルドは通るがスタンプは無効)
8. `vendor/models/*.onnx` を `%APPDATA%/mimageviewer/models/` から自動 copy
   (= 一度 mIV をインストール / 起動して APPDATA に展開させた後でないと取れない)

### bootstrap で取れないもの

- **`vendor/vst3-host/mimageviewer-vst3-host.exe`**: VST3 SDK の DL が ~490 MB と
  大きいので bootstrap には含めていない。以下のいずれかで配置する:
  - **既存ビルド済み exe をコピー** (推奨): 別 worktree やバックアップに残っている
    `mimageviewer-vst3-host.exe` と必須の `vcrt/` (公式CRT4本) を
    `vendor/vst3-host/` に実ファイルでコピーする。現ソースidentityが一致する成果物のみ再利用可。
    SDK不要だが、exe単体のコピーではhostを直接起動できない
  - **CMake で再ビルド**:
    ```bash
    bash scripts/setup-vst3-sdk.sh
    cmake -S crates/vst3-host -B crates/vst3-host/build -G "Visual Studio 18 2026" -A x64
    cmake --build crates/vst3-host/build --config Release
    ```
    CMake target は `vendor/vst3-host/mimageviewer-vst3-host.exe` へ直接出力するので
    `build/Release/` からの手動 cp は不要。詳細は下の「VST3 host bridge 管理」節を参照。
    ⚠ **古い別 worktree の exe を流用しないこと**: `crates/vst3-host/` の C++ ソース
    (`protocol.h` 含む) は頻繁に変わる。IPC プロトコルが現行 Rust 側とずれた exe を
    使うと bridge が起動直後にクラッシュし、VST3 有効時に動画再生が「激重」になる
    (2026-05-14 実害)。バージョンが合うか不明なら必ず再ビルドする。
- **TensorRT pack** (`vendor/tensorrt-cache/`): 開発時には不要。pack 配布作業時
  だけ `scripts/setup-tensorrt-pack.ps1` を回す

### 消えると再取得できないもの — ツリー外バックアップ必須

`bootstrap-vendor.sh` で再取得できる物 (pdfium / libraw / ort / ffmpeg / susie) と違い、
**以下は失うと復旧手段が限られる / 無い**:

- **`vendor/models/*.onnx`** — DL スクリプトが**存在しない**。`%APPDATA%\mimageviewer\
  models\` (インストール済み環境が展開した物) からコピーするしかない。動く mIV
  インストールも APPDATA も両方失うと**永久に取得不可**。
- **`vendor/vst3-host/mimageviewer-vst3-host.exe`** — 再ビルドには VST3 SDK
  (~490 MB) + cmake が要る。古い exe の流用は上記の通り不可。

そのため `vendor/models/` と `vendor/vst3-host/` は**リポジトリツリー
(`C:\home\mimageviewer` 配下) の外**へコピーを置く。現在のバックアップ先:

```
C:\home\mimageviewer_vendor_backup\
  ├ models\        (*.onnx 一式)
  └ vst3-host\     (mimageviewer-vst3-host.exe と vcrt\ の公式CRT4本)
```

- **定期ジョブは不要**。`vendor/` の中身は静的なので、モデル追加や vst3-host を
  再ビルドした**ときだけ**バックアップを取り直す。hostはexeと `vcrt/` を含む
  ディレクトリ全体を `C:\home\mimageviewer_vendor_backup\vst3-host` へ実ファイルでコピーする。
- **復旧手順**: `vendor/` 消失時、`bootstrap-vendor.sh` を流した後に
  `cp -r C:/home/mimageviewer_vendor_backup/models vendor/` と
  `cp -r C:/home/mimageviewer_vendor_backup/vst3-host vendor/` で埋め戻す。
- **背景**: worktree junction 事故 ([git-workflow.md](git-workflow.md) 参照) で `vendor/` は
  2026-05 に複数回全消失している。再取得可能な物は bootstrap で戻るが、models は
  戻らないため、この外部バックアップが最後の砦になる。

### 前提環境変数 / ツール

- `gh` (GitHub CLI) 認証済み — pdfium / ffmpeg の release 取得に必要
- `rustup target add i686-pc-windows-msvc` 済み — susie 32bit ワーカービルド
- `LIBCLANG_PATH` 環境変数登録済み — FFmpeg の bindgen 用 (下の「FFmpeg LGPL DLL
  管理」節に詳細)
- `cmake` + Visual Studio 2026 (18) BuildTools + NASM — TurboJPEG / vst3-host ビルド
- `unzip`, `tar`, `curl` — setup スクリプト群が使う

## mimalloc の一時 vendor（§1.666）

`mimalloc` 0.1.52 → `libmimalloc-sys` 0.1.49 の v3 C ソースだけを、上流無改変の
mimalloc v3.5.3 に置き換える。root の `[patch.crates-io]` から
`vendor/libmimalloc-sys` を参照し、アロケータの選択・feature・環境変数は変えない。
取得元、checksum、FFI の調整、終了時不具合の根拠、検証の限界は
[PATCHES.md](../vendor/libmimalloc-sys/PATCHES.md) を正本とする。
公式ラッパーが mimalloc v3.4.1 以上を同梱したら、一時 vendor を撤去して公式版へ戻す。
`scripts/check-dependency-updates.ps1` の自動表でラッパーの更新を確認する。

## PDFium 管理

PDF サポートは PDFium ライブラリ (Google Chrome の PDF エンジン) を使用する。
DLL は exe に `include_bytes!` で埋め込まれ、初回起動時に
`%APPDATA%\mimageviewer\pdfium.dll` に展開される。

### マルチプロセス並列レンダリング

PDFium はスレッドセーフではないため、マルチプロセスで並列化している。
`mimageviewer.exe --pdf-worker` で起動したワーカープロセス (設定 3〜10、既定 5) が
各自独立に PDFium を初期化し、stdin/stdout バイナリプロトコルでメインプロセスと通信する。
ワーカーは GUI を持たず、メインプロセス終了時に自動終了する。
設定値は App 起動時に static snapshot へ 1 回だけ固定し、pool の遅延初期化時には
`Settings` を読み直さない。このため変更は、最初に PDF を開いた時期にかかわらず次回起動から有効。

### セットアップ

```bash
bash scripts/setup-pdfium.sh        # DLL をダウンロード (vendor/pdfium/bin/pdfium.dll)
bash scripts/setup-pdfium.sh check  # 新しいバージョンの有無を確認
```

- **ソース**: [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries)
  (毎週月曜に Chromium 最新版から自動ビルド)
- **アセット**: `pdfium-win-x64.tgz` (V8 なし版、軽量)
- **現在のバージョン**: `chromium/8125` (`158.0.8125.0`、2026-10-28 更新)。
  `vendor/pdfium/VERSION` と資産照合は [更新記録](dependency-update-20261028.md) を参照

### リリース前チェック (必須)

**リリースビルドの前に必ず以下を確認すること:**

1. `bash scripts/setup-pdfium.sh check` を実行し、新しいバージョンがないか確認
2. 新しいバージョンがある場合は `bash scripts/setup-pdfium.sh` で更新
3. 更新後は PDF の表示が正常か動作確認してからリリース

## ONNX Runtime 管理

AI 機能 (アップスケール・ノイズ除去・消しゴム) は ONNX Runtime + DirectML EP を
使用する。`ort` クレートの `load-dynamic` 機能で、メイン exe は静的リンクせず、起動時に
`%APPDATA%\mimageviewer\onnxruntime.dll` を動的ロードする。DLL は PDFium と同様
`include_bytes!` で exe に埋め込み、process 共通 AI runtime 初期化 worker が APPDATA へ展開する。

`onnxruntime.dll` は Microsoft の動的 CRT build なので、公式 VC/Redist 由来の
`msvcp140.dll` / `msvcp140_1.dll` / `vcruntime140.dll` / `vcruntime140_1.dll` を
app-local 配置する。単体 launcher は versioned runtime directory へ hash 検証付きで展開し、
portable / 開発 build は exe 隣へ loose 配置する。利用者による追加セットアップは不要。
由来、版、署名、hash の正本は `vendor/vcrt/provenance.json` とし、native dependency 更新時は
`scripts/check-vcrt-pe-dependencies.ps1` を通して4本を一体更新する。

GUI process の ORT constructor は process 共通 `AiRuntimeInitOwner` だけが worker 上で一度実行する。
App / Remote / materializer は同じ Ready / Failed terminal snapshot を使い、UI thread は待たない。
`ort` 2.0.0-rc.15 は upstream `17ed727` 相当の修正を含むため、ローカルforkは使用しない。
dynamic load / export / C API version の初期化失敗を ORT API に再入せず
`LoadDynamicError` として返す。環境は `build()` で生成し、同じprocess共通の初期化結果に
保持して各 `Session::builder(&environment)` へ渡す。

### セットアップ

```bash
bash scripts/setup-ort.sh           # 既定バージョン (1.24.4) をダウンロード
bash scripts/setup-ort.sh 1.24.4     # バージョン指定
```

- **ソース**: NuGet `Microsoft.ML.OnnxRuntime.DirectML` (Microsoft 公式ビルド)
- **CDN**: `https://globalcdn.nuget.org/packages/microsoft.ml.onnxruntime.directml.<VERSION>.nupkg`
- **現在のバージョン**: `vendor/ort/VERSION` を参照

### バージョン互換性

`ort-sys::ORT_API_VERSION`（`ort::MINOR_VERSION`）は選択した `api-*` featureで決まる。
`load-dynamic` は DLL のminor版がこの値より小さいと `BadVersion` で拒否する。
ort更新時は既定featureだけで判断せず、workspace全体のfeature統合とDLLのAPI互換性を確認する。

- ort 2.0.0-rc.15: 全consumerで `default-features = false` と `api-24` を指定。
  DirectML 1.24.4 と TensorRT pack の GPU 1.24.2 はともに API 24 を満たす。
- rc.15既定の `api-31` は入れない。`api-25` 以上と `download-binaries` が
  `cargo tree -e features -i ort --workspace --locked --offline` に入らないことを確認する。
- `download-binaries` / `tls-native` / `copy-dylibs` は使用せず、DLLは既存のvendor管理を続ける。
  2026-10-28 の入力hash・検証結果は [依存物更新記録の段B](dependency-update-20261028.md#dependency-stage-b) を参照。

### DirectML.dll について

Windows 10 1903 (2019年5月) 以降は `DirectML.dll` が OS に標準同梱されているので、
個別に配布する必要はない。`onnxruntime.dll` は System32 の DirectML.dll を自動検出する。

## Susie 32bit ワーカー管理

Susie 画像プラグイン (`.spi`, 32bit DLL) は 64bit メインプロセスから直接ロードできないため、
`mimageviewer-susie32.exe` という 32bit のワーカープロセスを子プロセスとして起動して使う。

### exe 埋め込み + APPDATA 展開方式 (PDFium と同じパターン)

- 32bit ワーカーは `include_bytes!` でメインの `mimageviewer.exe` に埋め込む。
- 初回起動時に `%APPDATA%\mimageviewer\mimageviewer-susie32.exe` に展開される。
- インストール先 (`Program Files`) には追加ファイルを置かない。
- 展開時のサイズ一致チェックで、同一バイナリなら書き戻しをスキップ。

### セットアップ (メインビルド前に必須)

```bash
bash scripts/setup-susie-worker.sh
# 内部で以下を実行:
#   cargo build --release --target i686-pc-windows-msvc -p mimageviewer-susie32
#   cp target/i686-pc-windows-msvc/release/mimageviewer-susie32.exe vendor/susie-worker/
```

- 前提: `rustup target add i686-pc-windows-msvc` 済みであること。
- 出力: `vendor/susie-worker/mimageviewer-susie32.exe` (.gitignore)。
- メイン exe のリリースビルド前には **必ず** 実行する。未実行だとコンパイル時に
  `include_bytes!` が失敗する。

## FFmpeg LGPL DLL 管理

動画インライン再生 (フルスクリーンで MP4 / MKV / MOV / AVI / WMV / MPG / MPEG /
HEVC / AV1 を再生する) のために、FFmpeg の **LGPL shared build** を vendor に置く。

⚠️ **配布形態はランチャー方式**。`ffmpeg-the-third` は MSVC import library 経由で
通常リンクされ、Windows ローダが exe ロード時 (= Rust コードが走るより前) に DLL を
解決するため、PDFium / ONNX Runtime のような「include_bytes! → APPDATA 展開」方式は
直接の本体には適用できない (ローダの解決タイミングに間に合わない)。`/DELAYLOAD` も
rustc 経由の link.exe で機能しない (Delay Import Directory が空のまま、原因未解明)。

そこで **launcher が core・remote service・EPUB converter・FFmpeg DLL・VC runtime・EffeTune Mixwright bundle を内包する構成**で「単体 exe 配布」を実現している:

```
配布する mimageviewer.exe (= ランチャー、crates/launcher/ が生成)
├── include_bytes! で内包:
│   ├── mimageviewer-core.exe   (本体、ffmpeg-the-third を import library リンク)
│   ├── mimageviewer-remote.exe (本体と remote-ipc protocol version を共有、Web UI 資産も内包)
│   ├── mimageviewer-epub-pdf.exe (EPUB → PDF 変換器)
│   ├── effetune/EffeTune Mixwright.vst3/ (全469ファイル、v0.15.0。入力は別の VERSION と計470ファイル)
│   ├── app-local VC runtime 4 DLL (Microsoft 署名を保持)
│   ├── avcodec-61.dll
│   ├── avformat-61.dll
│   ├── avutil-59.dll
│   ├── avfilter-10.dll
│   ├── swscale-8.dll
│   └── swresample-5.dll
└── 起動時の動作:
    1. %APPDATA%\mimageviewer\runtime\<version>\ に上記 exe / DLL と app-local VC runtime を展開
       (版別 SHA-256 sidecar で照合し、不一致なら atomic replace)
       EffeTune は同ディレクトリ下の不変世代へ全hash検証して公開し、atomic更新するのはcurrent pointerだけ。
       一覧・サイズ・更新時刻・作成時刻とstampが一致すれば再hash／write lockなし。修復失敗もcore起動を継続。
    2. std::process::Command で mimageviewer-core.exe を spawn (引数 forward)
    3. core が使用中lockを引き継いだ通知後にランチャー終了 (coreの通常終了は待たない)
```

ランチャーは **FFmpeg API を一切呼ばない** ので Windows ローダの DLL 解決問題に
直撃しない。core を spawn する時点で展開済み DLL が同じディレクトリにあるので、
Windows の DLL 検索順 (exe 同居が最優先) で確実に解決される。

**バージョン別 runtime ディレクトリ**: `runtime\<version>\` のように分けることで、
古い core / remote / EPUB worker が走行中に新ランチャーが上書きしようとして file lock で失敗する事象を回避
(Codex レビュー助言)。core の起動完了・初回描画後、短命の背景 worker が現在実行中の
`CARGO_PKG_VERSION` と使用中lockで保護された版を残し、他の版を best-effort 削除する。
coreはexeの版directoryと固定EffeTune世代のshared leaseを寿命中保持し、launcherもcoreへの
引き渡しまで保持する。清掃はcandidateのexclusive leaseと抽出／公開lockを非blocking取得し、
削除完了まで保持する。cooperative lockは削除対象外の `runtime/.locks/` に置き、微小なfileを永久保持する。
launcherは版directoryを作る前に外側版leaseを取得する。旧launcherのpublisher互換用lockも併用する。
workerが実行中process画像を一度だけ列挙し、候補内の画像があれば旧版も
保持する。同版の別coreが存在する間は全旧EffeTune世代を保守的に保持する。未知の名前と再解析ポイントは
残し、削除失敗はログだけで次回起動へ回す。ダウングレード時には新しい版も削除対象となり、
その版を再び起動すると launcher が再展開する。portable は清掃しない。

**ビルド順序**: cargo は同一ワークスペース内 bin の依存順序を表現できないので
`scripts/build-release.{sh,ps1}` が 4 段階に分けて呼ぶ:
1. `cargo build --release --bin mimageviewer-core` → 本体生成 (package `mimageviewer` 内の bin なので `-p` 不要)
2. `cargo build --release -p mimageviewer-remote --bin mimageviewer-remote --features embedded-web-assets`
   → Web UI 資産を内包した remote service 生成
3. `cargo build --release -p epub-pdf-worker --bin mimageviewer-epub-pdf` → EPUB converter 生成
4. `cargo build --release -p mimageviewer-launcher --bin mimageviewer` → ランチャー生成 (core + remote + EPUB worker + EffeTune bundle 等を include_bytes!)。**bare `cargo build --release --bin mimageviewer` は失敗する** (`no bin target named mimageviewer in default-run packages`。`mimageviewer` bin は package `mimageviewer-launcher` にあり workspace default-members 外なので `-p` 必須)

ラッパーは 4 つの cargo 呼び出しすべてで `CARGO_INCREMENTAL=0` を明示する。`Cargo.toml` の
release profile はローカル rebuild 高速化のため `incremental = true` だが、ThinLTO +
rust-lld で stale incremental object が残ると `fast_image_resize` などの SIMD symbol が
release link 時に未解決になることがあるため、配布ビルドは安定優先で incremental を切る。

`cargo build --release` を直接打つ場合は ① → ② → ③ → ④ の順で 4 回打つこと。
ランチャー側 build.rs が `target/release` の core / remote / EPUB worker の存在をチェックし、
無ければ復旧手順付きで止まる。EffeTune も `vendor/effetune-mixwright/VERSION` と
`EffeTune Mixwright.vst3/` を必須検証する。`MIMV_EFFETUNE_DIR` で staging root を指定できる。
承認済み入力は `third_party/effetune-mixwright/v0.15.0/manifest.sha256` の全470エントリと照合する。
配布署名時は build-dist.ps1 が bundle を target にコピーして PE を署名し、埋め込み元を指定する
(vendor 原本は変更しない)。

**配布物**:
- 単体 exe 版: `mimageviewer.exe` 1 ファイル (内包する core + remote + EPUB worker + DLL + EffeTune bundle を含む)
- インストーラ版: `mImageViewer_setup.exe` (Inno Setup が同じランチャーを配置)
- どちらも初回起動時に APPDATA に展開、2 回目以降は展開済みなのでスキップして高速

### セットアップ (メインビルド前に必須)

```bash
bash scripts/setup-ffmpeg.sh           # BtbN の n7.1 系・最新の版付き LGPL shared build を取得
bash scripts/setup-ffmpeg.sh check     # 新しい版付きビルドがあるか確認のみ
```

- 取得元: [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds/releases)
  の `ffmpeg-n7.1*-win64-lgpl-shared-7.1.zip`
  - ローリング `latest` release の `...-latest-...zip` は使わない。日付付き `autobuild-*`
    release にある commit hash 込みの版付き資産だけを選び、`vendor/ffmpeg/VERSION` に記録する
- 出力先:
  - `vendor/ffmpeg/bin/{avcodec,avformat,avutil,avfilter,swscale,swresample}-*.dll`
    — launcher の `include_bytes!` で `mimageviewer.exe` に埋め込み
  - `vendor/ffmpeg/include/`, `vendor/ffmpeg/lib/`
    — `ffmpeg-the-third` の build.rs が `FFMPEG_DIR` 経由で参照
    (`.cargo/config.toml` に `FFMPEG_DIR=vendor/ffmpeg` を設定済み)
  - `vendor/ffmpeg/LICENSE.txt` — LGPLv3-or-later の本文
- 前提: `gh` (GitHub CLI), `unzip`, `curl` が PATH にあること。
  さらに `ffmpeg-sys-the-third` のビルドに **libclang (LLVM/Clang)** が必要。
  Visual Studio Installer の「C++ Clang コンパイラ」コンポーネントを入れるか、
  独立した [LLVM インストーラ](https://github.com/llvm/llvm-project/releases) を使う。
- **環境変数 `LIBCLANG_PATH` を必ず永続登録すること** (一度だけでよい):
  ```powershell
  [Environment]::SetEnvironmentVariable(
      "LIBCLANG_PATH",
      "C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\Llvm\x64\bin",
      "User"
  )
  ```
  パスは VS のエディションによって変わる: `Microsoft Visual Studio\<バージョン>\<エディション>\VC\Tools\Llvm\x64\bin\libclang.dll` を `find`/`Get-ChildItem -Recurse` で探して使う。
  登録後は **PowerShell を開き直す** (現セッションだけ反映するなら `$env:LIBCLANG_PATH = "..."`)。
  未設定だと `cargo build` が `Cannot find clang: 'clang.dll', 'libclang.dll'` で失敗する。

### バージョンを上げる場合

FFmpeg メジャーが上がると DLL のメジャー番号が変わる (例: 7.x の `avcodec-61.dll` →
8.x の `avcodec-62.dll`)。以下を **3 箇所すべて** 揃えて変更する:

- `scripts/setup-ffmpeg.sh` の `ASSET_GLOB` (例: `ffmpeg-n8.0*-win64-lgpl-shared-8.0.zip`)
- `src/video/ffmpeg_loader.rs` の `include_bytes!` パスと `DLLS` 配列のファイル名
- `build.rs` の `vendor/ffmpeg/bin/avcodec-XX.dll` 等のチェックパス
- `build.rs` の `/DELAYLOAD:avcodec-XX.dll` 等のリンカフラグ (DLL 名一致が必須)

### 動作確認

リリースビルド成果物:
```
target/release/mimageviewer.exe          # 配布用 (ランチャー、~365MB)
target/release/mimageviewer-core.exe     # 本体 (内包される実体、単体では起動不可)
```

`mimageviewer.exe` をダブルクリックすると:
1. 初回のみ `%APPDATA%\mimageviewer\runtime\<CARGO_PKG_VERSION>\` (例: `runtime\0.9.0\`) に core + 6 DLL を展開
2. core が spawn されて自身のウィンドウを開く
3. 動画フォルダで動画をダブルクリック → フルスクリーンでインライン再生開始

2 回目以降は展開済みなのでスキップ (サイズ一致チェック)、起動が速い。

### 開発時に core を直接起動したいとき

ランチャー経由ではなく `target/release/mimageviewer-core.exe` を直接実行したい場合
(デバッガアタッチ等)、FFmpeg DLL を手動でコピーする:
```powershell
Copy-Item vendor/ffmpeg/bin/*.dll target/release/
```
core 側はその時点で同居 DLL を直接使う (APPDATA 展開はランチャー専属の責務)。

### LGPL ライセンス対応 (リリース時に必須)

mIV 自身は MIT、FFmpeg は LGPLv3-or-later。**動的リンク** (DLL を別ファイルとして配布し
`LoadLibrary` する形式) なら互換。`include_bytes!` で exe に埋め込んでも、最終的に
APPDATA の DLL ファイルとして展開されるので動的リンクとして扱える。

リリース前に以下を確認・更新する:

1. **ソフトウェア情報 (環境設定 → ヘルプ)** に LGPL 通知を追記。
   通知文面の canonical 版は `docs/ffmpeg-lgpl-source-distribution.md` の
   "Notice Template" を参照する。
2. **mikage.to に LGPL 対応ソース tarball を配置**: BtbN はリリースタグではなく
   **特定 commit** からビルドするので、タグの tarball では対応ソースにならない。
   `vendor/ffmpeg/VERSION` の `g<hash>` から GitHub の commit アーカイブを取得し、
   `htdocs/mimageviewer/ffmpeg-<BUILD-ID>-source.tar.gz` として転載する
   (手順と検証は `docs/ffmpeg-lgpl-source-distribution.md` の "Corresponding Source")。
   現行 BtbN build は外部ライブラリを DLL 内に含むため、詳細は同ドキュメントと
   `scripts/collect-ffmpeg-lgpl-info.ps1` の出力で確認する。
2.5. **製品ページの FFmpeg 節を同時に更新**: `htdocs/mimageviewer/index.html` の
   「サードパーティ ソフトウェア（FFmpeg / LGPL）」は手書きなので自動追随しない。
   バージョン・対応ソースリンクを新 BUILD-ID に更新し、旧版は「以前のバージョン向けの
   対応ソース」へ移す。アプリ内表記は `build.rs` が `vendor/ffmpeg/VERSION` から
   焼き込むので手当て不要 (2 箇所で運用が違う点に注意)。
3. **DLL ファイル名を改変しない**: `avcodec-61.dll` 等のオリジナル名のまま展開する。
4. **`installer/readme.txt` (Vector 同梱用)** にも LGPL 注記を 1 行追加。
5. **GPL build を絶対に使わない**: BtbN の `*-gpl-shared-*` や gyan.dev の
   "release-essentials" を使うと mIV 全体が GPL 汚染される。`setup-ffmpeg.sh` は
   `*-lgpl-shared-*` のみを取りに行くようになっている。

### Codec 対応範囲

LGPL build には以下が含まれる (主要なもののみ):

| コーデック | 対応 | 出元 |
| --- | --- | --- |
| H.264 / AVC | ✓ | OpenH264 (Cisco) を内蔵 |
| HEVC / H.265 | ✓ | LGPL 互換の libdav1d + 内蔵デコーダ |
| AV1 | ✓ | libdav1d |
| VP9 | ✓ | libvpx |
| MPEG-2 / MPEG-4 / MJPEG / WMV / VC-1 | ✓ | FFmpeg 内蔵 |
| AAC / MP3 / Opus / FLAC / Vorbis / AC-3 / DTS | ✓ | FFmpeg 内蔵 |

x264 / x265 (エンコーダ) は GPL なので含まれない (mIV はデコードしか使わないので無問題)。

### テスト

統合テスト (`tests/susie_integration.rs`) は `MIV_SUSIE_WORKER` 環境変数で
ワーカー exe のパスを直接指定できる。`setup-susie-worker.sh` を走らせて
`vendor/susie-worker/` に配置済みであれば、テストは自動でそれを拾う。

## LibRaw ソース・ライセンス管理

RAW は LibRaw 0.22.3 を CDDL-1.0 で静的リンクする。zlib 1.3.1 と既存の
libjpeg-turbo 静的ライブラリを使い、新しい DLL / exe は追加しない。
エージェント用セットアップは `bash scripts/setup-libraw.sh`（bootstrap に含む）、
更新確認は `bash scripts/setup-libraw.sh check`。この bash 手順は利用者には渡さない。
`vendor/libraw/VERSION` はセットアップが生成し、`build.rs` が `MIV_LIBRAW_BUILD_ID` に
焼き込んでアプリ内の同梱版と対応ソース URL を揃える。欠落・不正なら復旧手順付きで失敗する。

- tracked notice はルートの `LIBRAW-LICENSE.txt` / `ZLIB-LICENSE.txt` /
  `LIBJPEG-TURBO-LICENSE.txt`。installer / portable の双方へ同梱する。
  3 本とも root ファイルから `include_str!` で本体へ埋め込み、バージョン情報で全文を表示する。
  単体 exe 版でも全文と IJG 帰属文を読めることを維持する。
- LibRaw 本体は無改変の公式 tarball、shim は本リポジトリの MIT コード。
- 対応ソースは [docs/libraw-source-distribution.md](libraw-source-distribution.md) に従って
  SHA-256 を照合し、mikage.to へ配置する。tarball は git に入れず、`.sha256` を追跡する。
- LibRaw 更新時はソース・checksum・notice・製品ページの同梱版とリンクを同時に更新し、
  古い配布版の対応ソースと追跡済み `.sha256`、製品ページの旧版リンクも残す。

## VST3 host bridge 管理 (v0.9.0+)

動画音声に VST3 プラグインを挿入する機能 (LUFS 測定 / EQ / 等) のために、
**C++ で書かれた bridge プロセス** (`mimageviewer-vst3-host.exe`) を別途配置する。
詳細は [docs/vst3-integration.md](vst3-integration.md) を参照。

VST3 SDK は **MIT ライセンス化されている** (3.8.0、2025-10-20 以降) ので mIV (MIT) と
互換。bridge ビルド成果物は通常の `include_bytes!` で本体に内包する (PDFium / Susie ワーカーと
同じパターン)。

Windows SDK hosting moduleのMIT原文を `crates/vst3-host/src/sdk/` に保持し、UTF-8 pathを
明示的にUTF-16へ変換するwide API版を保守する。process全体のACPは変更しない。
2026-10-03 v4.3.0 release check: pluginが観測するmodule／CRT load pathは、通常絶対pathが
260 UTF-16単位未満かつ同一pathへの正規化round-tripが成立する場合だけ通常Win32形式にする。
末尾dot／spaceやDOS device名、正規化差分／失敗、260以上は拡張形式を維持する (Codex P2/P3 対応)。
UNC namespace markerは大小文字を区別しない。host内FS検査は拡張形式を維持する。
`GetModuleFileNameW`が保持する `\\?\` とpluginが付加する `/` の組合せによるEffeTune missing-assetsを避ける。
2026-10-02 R2ではhost PE内の `MIV_VST3_HOST_SOURCE_SHA256:` markerを、CMakeLists／include／src／testsの
現ソースhashと照合する。2026-10-12の音響調整窓対応では `src/*.rc`、baseの `assets/icon.ico`、
専用 `assets/effetune.ico`、再生成用 `scripts/generate-effetune-icon.py` も対象に含める。
APPDATA等から旧hostをimportするfallbackはない。
identityは `scripts/vst3-host-identity.ps1` に計算を一本化し、CMakeも同scriptの
`-HashSourceRoot` を使う。text対象のCRLFだけをbyte単位でLFへ正規化し、他のbyteは保持する。
二つのiconはbinaryのままraw SHA256を取り、host source rootからの相対名を使う。
generatorは他のtextと同じLF正規化で検証する。`python scripts/generate-effetune-icon.py --check` で
追跡ICOが再生成結果と一致することを確認できる（Pillowが必要、取得をbuildで行わない）。
ordinal順のslash相対名と各SHA256を `name:hash\n` としてUTF-8/BOMなしで連結しSHA256化する。
Gitの改行変換によるLF／CRLF checkout差を無視するが、未コミットのソース変更は検知する。
SDK欠落時や `-SkipVst3Bridge` は現ソースと一致するvendor hostのみ再利用可能。不一致／欠落は復旧案内付きで停止する。
build-release／build-distの署名前・core埋込前と、非portable coreのbare cargo release buildにも同じgateを通す。
`dev-runtime` もrelease継承profileのためこのgate対象。host変更時は先にCMakeで再buildする。
再build成果物のSHA256／sizeを記録し、内側hostの署名→core→launcherの順で埋め込む。

### セットアップ (メインビルド前に必須)

```bash
# 1. VST3 SDK を vendor/vst3sdk/ に取得 (~490 MB)
bash scripts/setup-vst3-sdk.sh

# 2. CMake で C++ bridge をビルド
cmake -S crates/vst3-host -B crates/vst3-host/build -G "Visual Studio 18 2026" -A x64
cmake --build crates/vst3-host/build --config Release
# → vendor/vst3-host/mimageviewer-vst3-host.exe と vendor/vst3-host/vcrt/ の公式CRT4本
```

- 前提:
  - Visual Studio 2026 (18) BuildTools (MSVC C++ デスクトップ開発ワークロード)
  - CMake 3.20+
  - 採用 SDK は `v3.8.1_build_84`。最低互換条件は MIT 化された 3.8.0 のまま。
  - identity対象のソース・RC・共有iconの変更時、およびSDK更新時に再ビルドする。
    SDK自体はsource identityに含まれないため、marker一致だけでSDK版を判別しない
- CMakeは `vendor/vcrt/` の公式 `vcruntime140.dll` / `vcruntime140_1.dll` /
  `msvcp140.dll` / `msvcp140_1.dll` を `vendor/vst3-host/vcrt/` へcopy-if-differentで配置する。
  hostが再リンク不要でも配置targetは実行される。正本が欠けた場合は復旧案内付きでbuildを停止する。
  コピーはMicrosoft署名を保持し、mIVとして再署名しない。CRTをhost直下には置かない。
  testerと `test-full.ps1` 内の直接起動テストはこのvendor exeと隣の `vcrt/` を使うので、
  full gateの前にこのCMake build（または現ソース一致のexe＋CRTセットの復元）を済ませる。
  `test-full.ps1`／`build-dist.ps1` はテスト前に非起動preflightでCRT4本と正本の完全一致を検査し、
  欠落／破損ならCMake buildまたはセット全体の復元を案内して停止する。
  `build-dev.ps1` のcoreは公式CRTを埋め込み、通常の抽出経路で同じ `vcrt/` layoutを作る。
- 出力: `vendor/vst3-host/mimageviewer-vst3-host.exe` と `vcrt/` (.gitignore)。
  メイン exe のリリースビルド時に `include_bytes!` で内包される
- 動作確認用: `crates/vst3-host-tester/` (Rust 単独 GUI exe)。本体に依存せずに
  プラグインのロード / GUI / 音声パススルーをテストできる。`cargo run -p vst3-host-tester`

### ライセンス対応 (リリース時)

VST3 SDK 3.8.0+ は MIT 単一ライセンスなので、**追加の法務作業は不要**。
ただし以下を環境設定→ヘルプの「ソフトウェア情報」と `installer/readme.txt` に追記する:

```
This software supports VST3 plugins via the Steinberg VST3 SDK
(https://github.com/steinbergmedia/vst3sdk) under the MIT License.
```

**VST トレードマークについての採用方針**: VST3 プラグインへの対応を文字で説明し、
VST のロゴは使用しない。名称の文字表記も商標ガイドラインの対象となり得るため、
文字表記によってガイドラインを回避できるとは扱わない。この記録は採用した表記方針を
示すもので、ガイドラインへの適合や商標侵害の有無を断定するものではない。

## libavif / dav1d / libyuv の静的リンク（§1.407）

AVIF は libavif 1.4.2、dav1d 1.5.3、libyuv
`644251f252a84bf8ce91ff0aca86a9b16b069ab8` を使用する。
libavif 自身の LocalDav1d / LocalLibyuv の固定組合せで、ライセンスは順に
BSD-2-Clause（同梱第三者 notice も全文を保持）、BSD-2-Clause、BSD-3-Clause。

エージェント用 `bash scripts/setup-libavif.sh` が checksum / commit を照合し、
MSVC x64・static CRT `/MT` で `vendor/libavif/{include,lib,licenses,VERSION}` を生成する。
bootstrap は毎回setupを呼び、`--force`も伝える。setupはVERSION一致と出力7ファイル
（header・lib3本・license3本）すべての存在でskipを判定する。meson / ninja / NASM は
vendor 内の専用 tools に配置し、システムへは追加しない。
Cargo は依存の C ビルドを行わず、`crates/libavif-sys` の小さな C shim だけを cc でビルドする。
欠落・版違いは復旧コマンドを付けて停止する。新しい DLL / exe は追加しない。
ソースとツールは再取得できるため、ツリー外バックアップは不要。

更新時は `bash scripts/setup-libavif.sh check` で上流を確認し、setup の版・hash・
組合せ、libavif-sys の期待 VERSION、設計書を合わせて更新する。
`--force` で再構築し、AVIF fixture テストと decode / link 検査を行う。
ルートの `LIBAVIF-LICENSE.txt` / `DAV1D-LICENSE.txt` / `LIBYUV-LICENSE.txt` は
vendor/licenses のバイトをそのままコピーし、`.gitattributes -text` で保持する。
about の全文表示と同梱版、installer / portable の配布リストも更新する。
ソース再配布義務はない。

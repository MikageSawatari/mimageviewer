# ポータブル版 (loose-deps portable) 設計・実装メモ — v1.1.0

> ステータス: **実装済み (2026-06-07、v1.1.0 開発中、未リリース)**。`portable` cargo feature +
> `src/native_assets.rs` + `scripts/build-portable.ps1` で稼働。両ビルド (`cargo check` /
> `--features portable`) がコンパイル通過。残るは §8 の実機 smoke (GUI 起動・PDF/動画/AI/Susie)。
> §9 の未決事項は全て確定済み。

## 1. 背景と目的

### 動機

1. **C ドライブ圧迫 / ポータブル運用の要望** (掲示板フィードバック 2026-06):
   - 現行 zip 版は「インストーラ + readme」を束ねただけで、ポータブルとしての意義が薄い。
   - インストーラ版は `PrivilegesRequired=admin` ([installer/mimageviewer.iss:29](../installer/mimageviewer.iss)) で
     管理者権限を要求するため敬遠される。
   - 「解凍するだけで使える / 任意ドライブに置ける」ポータブル版が欲しい。
2. **アンチウィルス誤検知**: 現行の単体 exe (= ランチャー) は起動時に
   `mimageviewer-core.exe` + DLL を APPDATA に書き出して**子プロセスとして spawn** する。
   これは典型的な dropper パターンで、ヒューリスティック検知に引っかかりやすい。

この 2 点は**同じ原因**(「exe が内包バイナリをディスクへ展開して実行する」)に帰着するので、
ポータブル化で同時に解消できる。

### ゴール

- フォルダを任意ドライブに解凍 → `mimageviewer.exe` ダブルクリックで起動 (インストール不要、管理者権限不要)。
- 実行時の **DLL/exe 展開をゼロ**にする (= AV 誤検知の主因を除去、起動も速い)。
- 設定・キャッシュ・ログを **exe の隣** (`data\`) に置き、APPDATA も C ドライブも汚さない。
- **既存のインストーラ版・単体 exe 版はそのまま維持**。ポータブルは「第 3 の配布物」として追加する。

### 非ゴール

- TensorRT pack (~6.8GB ダウンロード物) の同梱。これは元々埋め込みでなく実行時 DL なので、
  ポータブルでも `data_dir` 配下に DL される (= 自動的にローカル化されるだけ。特別扱い不要)。
- コード署名。AV 誤検知の本筋の解だが**ポータブル化とは独立**。§10 で補足するに留める。

## 2. 現状アーキテクチャの整理

### launcher + core + remote service

配布 `mimageviewer.exe` は **ランチャー** ([crates/launcher/src/main.rs](../crates/launcher/src/main.rs))。
起動時に `mimageviewer-core.exe` + `mimageviewer-remote.exe` + FFmpeg 6 DLL を
`%APPDATA%\mimageviewer\runtime\<version>\` へ展開して core を spawn する。remote service は
core が自分の隣から起動し、両者は同じ remote-ipc protocol source からビルドされる。
ランチャーが存在する主な理由は **FFmpeg がロード時リンク**で、
Rust コードが走る前に Windows ローダが DLL を解決する必要があるため
([build.rs:9-26](../build.rs)、[src/video/ffmpeg_loader.rs](../src/video/ffmpeg_loader.rs))。

→ **FFmpeg DLL を exe の隣に loose で置けばランチャーは不要**。core を直接起動でき、
ローダが同居 DLL を最優先で解決する。**ランチャーこそが AV 的に最も怪しい挙動**なので、
これを外すのが最大の効果。

### core 自身の埋め込み + 展開 (6 経路、すべて `data_dir::get()` 経由)

| 対象 | 埋め込み箇所 | 展開先 | 解決 API |
| --- | --- | --- | --- |
| pdfium.dll | [pdf_loader.rs:228](../src/pdf_loader.rs) | `data_dir/pdfium.dll` | `extract_embedded_file` → path |
| onnxruntime.dll | [ai/runtime.rs:31](../src/ai/runtime.rs) | `data_dir/onnxruntime.dll` | 〃 → `ort::init_from(path)` |
| onnxruntime_providers_shared.dll | [ai/runtime.rs:33](../src/ai/runtime.rs) | `data_dir/` | 〃 |
| mimageviewer-susie32.exe (32bit) | [susie_loader.rs:48](../src/susie_loader.rs) | `data_dir/` | path を spawn (`MIV_SUSIE_WORKER` で上書き可) |
| mimageviewer-vst3-host.exe | [video/dsp/extract.rs:8](../src/video/dsp/extract.rs) | `data_dir/vst3/` | path を spawn |
| AI モデル 7 本 (*.onnx) | [ai/model_manager.rs:23-53](../src/ai/model_manager.rs) | `data_dir/models/` | path |

> ⚠ この表は **非ポータブル (= 単体exe/インストーラ) の core** の埋め込み機構を示す。
> **ポータブル版では `mimageviewer-vst3-host.exe` だけは同梱しない** (v2.0.0、未署名 exe の
> AV 誤検知対策。§4.7 と本書末尾の注を参照)。他 5 経路は portable でも loose 同梱で carry over。

**重要な観察**: 6 経路すべてが「`data_dir::get()` に join → (必要なら) `extract_embedded_file` で
書き出し → 確定した `PathBuf` を返す」という**同一パターン**。アプリ本体 (UI/描画/async/動画/comic)
は、この `PathBuf` を受け取るだけで「どこから来たか」を一切気にしない。
→ ポータブル化で触る面は**この薄い継ぎ目だけ**で、機能追加で日常的に触る箇所と交差しない。

### 起動シーケンス (main.rs、関連部分のみ)

```
fn main():
  parse --pdf-worker / --tensorrt-build / ...  (子プロセス再起動モード。current_exe() を再実行)
  single_instance::acquire()        // L663。Global Named Mutex。data_dir より前
  data_dir::init()                  // L678。--data-dir を解析
  logger::init()                    // L690
  ai::model_manager::ensure_models_extracted()   // L775
  susie_loader::ensure_worker_extracted()        // L781
  ... eframe 起動
```

注意点:
- `single_instance::acquire()` は `data_dir::init()` **より前**に走る。だが mutex 名はグローバル定数で
  data_dir に依存しないので順序問題はない (§7 で mutex 名衝突を別途議論)。
- `--data-dir <path>` は既に実装済み ([data_dir.rs:24](../src/data_dir.rs))。
  ポータブル検出はこの仕組みに乗せられる。
- pdf-worker / tensorrt-build は `current_exe()` を再実行する。ポータブルでは current_exe = 配布 exe 自身に
  なるので、フラグ分岐 (main 冒頭) がそのまま機能する。

## 3. 設計方針

1. **`portable` という cargo feature を 1 つ追加**する。これでビルドを 2 種類に分ける
   (通常 = 埋め込み+launcher、portable = loose)。
2. **cfg ゲートは「埋め込むかどうか」の小さな const だけ**に閉じ込める。パス解決ロジックは
   両ビルド共通 (= 常にコンパイルされる = bit-rot しない)。
3. **native 依存の解決を 1 つのモジュール (`native_assets`) に集約**する。各呼び出し箇所は
   そのモジュールを呼ぶだけにし、cfg 分岐をそこ 1 箇所に閉じる。新しい native 依存を足すときも
   ここ + パッケージング一覧の 2 箇所だけ。
4. **ポータブル版は launcher を使わない**。core を `mimageviewer.exe` にリネームし、
   remote service と FFmpeg DLL を含む native 依存をその隣へ loose 同梱する。
5. **CI に `cargo check --features portable` を 1 行**足し、portable 分岐の腐りを機械的に防ぐ。

## 4. 具体設計

### 4.1 cargo feature

`Cargo.toml`:

```toml
[features]
portable = []   # loose-deps ポータブルビルド。native 依存を include_bytes せず exe 隣から解決
```

- 通常ビルド (`cargo build --release --bin mimageviewer-core`): feature OFF = 現状どおり全部埋め込み。
- ポータブルビルド (`cargo build --release --bin mimageviewer-core --features portable`):
  埋め込みゼロ → exe が ~15-30MB に激減。

### 4.2 集約モジュール `src/native_assets.rs` (新規)

native 依存の「埋め込み or loose」をここ 1 箇所で吸収する継ぎ目。

```rust
//! native 依存 (DLL / worker exe / AI モデル) の解決を一元化する継ぎ目。
//!
//! 通常ビルド: include_bytes で埋め込んだバイト列を data_dir へ展開してパスを返す。
//! portable ビルド (feature = "portable"): 何も展開せず、exe と同じディレクトリ
//! (またはその models/ サブフォルダ) のファイルパスを返す。存在しなければ明確なエラー。

use std::path::PathBuf;

pub enum Asset {
    Pdfium,
    OnnxRuntime,
    OnnxProvidersShared,
    SusieWorker,
    Vst3Host,
    Model(&'static str), // ファイル名
}

#[cfg(not(feature = "portable"))]
pub fn resolve(asset: Asset) -> Result<PathBuf, String> {
    // 現状の extract_embedded_file パターン。各 const は include_bytes! で埋め込み済み。
    // (実体は pdf_loader / runtime / susie_loader / dsp::extract / model_manager から移設)
}

#[cfg(feature = "portable")]
pub fn resolve(asset: Asset) -> Result<PathBuf, String> {
    let dir = exe_dir()?; // current_exe().parent()
    let path = match asset {
        Asset::Pdfium => dir.join("pdfium.dll"),
        Asset::OnnxRuntime => dir.join("onnxruntime.dll"),
        Asset::OnnxProvidersShared => dir.join("onnxruntime_providers_shared.dll"),
        Asset::SusieWorker => dir.join("mimageviewer-susie32.exe"),
        // ※ 実装メモ (v2.0.0): ポータブル版は vst3-host を **同梱しない** (AV 誤検知対策)。
        //    実コードの所在解決は `native_assets::bundled(name)` で、vst3-host は
        //    `vst3_supported()` 経由で「あれば使う / 無ければ機能を自動無効化」する扱い。
        //    下のスケッチの Asset::Vst3Host 分岐は portable では実質使われない。
        Asset::Vst3Host => dir.join("mimageviewer-vst3-host.exe"),
        Asset::Model(name) => dir.join("models").join(name),
    };
    if !path.exists() {
        return Err(format!("portable: bundled file missing: {}", path.display()));
    }
    Ok(path)
}
```

各呼び出し箇所の改修 (ロジックを `native_assets::resolve` に置換):

| ファイル | before | after |
| --- | --- | --- |
| pdf_loader.rs | `data_dir.join + extract_embedded_file` | `native_assets::resolve(Asset::Pdfium)?` |
| ai/runtime.rs | onnx 2 本を `extract_embedded_file` | `resolve(OnnxRuntime)?` / `resolve(OnnxProvidersShared)?` → `ort::init_from(path)` |
| susie_loader.rs | `worker_exe_cached_path` の展開分岐 | `resolve(SusieWorker)?` (既存の `MIV_SUSIE_WORKER` 上書きは維持) |
| video/dsp/extract.rs | `extract_embedded_file` | `resolve(Vst3Host)?` |
| ai/model_manager.rs | `ensure_models_extracted` で 7 本展開 | portable では no-op、`model_path()` が `resolve(Model(name))` を返す |

> include_bytes の const (`PDFIUM_DLL_BYTES` 等) は `#[cfg(not(feature = "portable"))]` で
> ゲートする。これで portable ビルドは ~300MB の埋め込みを持たない。

### 4.3 FFmpeg (コード変更ゼロ、パッケージングのみ)

core は FFmpeg を import library 経由でリンク済み。portable では 6 DLL を exe 隣に置くだけ
(launcher を使わない)。`ffmpeg_loader::init()` は既に「exe と同じ場所に DLL があるか確認して
ログ出力」するだけなので**改修不要**。dev で core を直接動かす手順
(`Copy-Item vendor/ffmpeg/bin/*.dll target/release/`) と同じ配置になる。

### 4.4 data_dir のポータブル検出

`data_dir::init()` を拡張する。優先順位:

1. `--data-dir <path>` 明示指定 (既存) → それを使う。
2. **portable ビルド (`cfg!(feature = "portable")`)** かつ `--data-dir` 未指定 →
   `<exe_dir>\data` を data_dir にする。無ければ作成。
3. それ以外 (通常ビルド) → 現状どおり `%APPDATA%\mimageviewer`。

```rust
pub fn init() {
    if let Some(explicit) = parse_data_dir_arg() { // --data-dir (read-only メディアの抜け道)
        DATA_DIR.set(explicit).ok();
        return;
    }
    #[cfg(feature = "portable")]
    {
        let dir = exe_dir().join("data");
        // 実際に作成 + プローブ write/remove で書込可否を判定。不可ならフォールバックせず中止。
        if let Err(e) = ensure_writable(&dir) {
            show_fatal_error(&format!(
                "ポータブル版はこのフォルダにデータを書き込めません:\n  {}\n\n\
                 書き込み可能な場所 (デスクトップ / D ドライブ / USB 等) に展開し直して\
                 から起動してください。\n\n詳細: {e}",
                dir.display()
            ));
            std::process::exit(1);
        }
        DATA_DIR.set(dir).ok();
    }
    #[cfg(not(feature = "portable"))]
    { DATA_DIR.set(default()).ok(); } // %APPDATA%\mimageviewer
}
```

- これで設定 (settings.db)・キャッシュ・回転 DB・ログ・TensorRT pack が全部 `<exe_dir>\data\` 配下に集まる。
- **書き込み不可時はエラーで起動拒否 (確定 2026-06-06)**: `<exe_dir>\data` が作成・書き込み不可
  (read-only メディア、`Program Files` 等の保護先) の場合は、**APPDATA へフォールバックせずに
  明確なエラーで起動を中止**する。ポータブルを選ぶユーザーは「APPDATA を触らない自己完結動作」を
  期待しているため、黙ってフォールバックすると契約違反になり最も嫌われる挙動になる。
  - エラーは launcher の `show_error` と同じ **ネイティブ MessageBox** で出す
    ("書き込み可能な場所 (デスクトップ・D ドライブ・USB 等) に展開し直してください" のような
    原因 + 対処を含む文面)。cryptic なクラッシュにしない。
  - 抜け道: **明示 `--data-dir <書込可パス>` を渡した場合は read-only メディア上でも起動可**。
    エラーが発火するのは「portable build かつ `--data-dir` 未指定かつ `<exe_dir>\data` が書込不可」の
    ときだけ。
  - 書込可否の判定は ACL を推測せず、**実際に `create_dir_all` + プローブファイル write/remove を試す**
    (Windows の ACL は事前予測が不確実なため)。

#### 全永続ストアが `data_dir::get()` 経由であることの検証 (2026-06-06)

`grep -rn '"APPDATA"' src/ crates/` の結果、**APPDATA を直接参照するのは launcher のみ**
([crates/launcher/src/main.rs:101](../crates/launcher/src/main.rs)、= runtime 展開先。portable は launcher 不使用)。
本体側の永続ストアは例外なく `crate::data_dir::get()` に join しているため、data_dir を切り替えるだけで
全データがポータブル配下に移る。確認できたストア:

- 設定: `settings.db` (+ `bak1..10` / `-wal` / `-shm`)、`pdf_passwords.json`、`comic_presets.json`、`comic_recent_stamps.json`
- DB 群: `rotation.db` / `rating.db` / `adjustment.db` / `conceal.db` / `mask.db` / `local_adjust.db` /
  `comic.db` / `spread.db` / `export_crop.db` / `book_resume.db` / `audio_normalize.db` /
  `folder_thumb_pins.db` / `auto_aspect_cache.db` / `video_tile_thumbs.db`
- 検索: `search_index.db` / `fts_index/` / `fts_meta.db`
- キャッシュ/生成物: `cache/` (カタログ) / `archive_cache/` + `archive_cache.db` / `captures/` / `debug-pipeline/`
- AI/拡張: `models/` / `tensorrt/` / `tensorrt-engines/` / `addons/editing/` / `user_fonts/` / `susie_plugins/` / `vst3/`
- ログ: `logs/` (`logs_dir()` も `get()` 経由)

→ ユーザーが想定する「解凍 → `<exe_dir>\data\` 以下に各種ファイルが作られ、APPDATA は触らない」が成立する。
shipped 物 (DLL は exe 隣、モデルは `models/`) と user 生成物 (`data/`) を分離する点に注意 (§4.7 のレイアウト)。

#### 実行時ダウンロード機能 (TensorRT pack / 編集用追加パック) の検証 (2026-06-07)

include_bytes 同梱物ではなく**実行時に DL する 2 機能**も、保存先・一時ステージング先ともに `data_dir`
内で完結することを確認 (= portable で自動的に `<exe_dir>\data` 配下に落ちる。`native_assets` 改修対象外、
**追加コード変更不要**):

- **TensorRT pack** ([ai/tensorrt_installer.rs](../src/ai/tensorrt_installer.rs)): 保存先 `pack_dir()` =
  `data_dir/tensorrt`、engine cache = `data_dir/tensorrt-engines`。DL 中の `.partial` (resume 用) も
  `pack_dir.join("{name}.partial")`、`INSTALL_OK.tmp` も pack_dir 内。**`temp_dir()`/`tempfile` 使用ゼロ**。
- **編集用追加パック** ([editing_addon_download.rs](../src/editing_addon_download.rs) /
  [editing_addon.rs:74](../src/editing_addon.rs)): `addon_root()` = `data_dir/addons/editing`。展開 staging は
  `downloads/` (addon_root 内)、検証後 `packs/<version>/` へ atomic rename。本番経路の `temp_dir` 使用は無し
  (参照は全てテストヘルパー名 `miv_editing_*_test_*`)。

**ポータブルに好都合な副次効果**: 両機能とも `.partial → 最終名` / `downloads/ → packs/<ver>/` の
**atomic rename を同一ボリューム内**で行う。staging を system temp (C:) に置いていたら、ポータブルを D: 等に
置いたとき cross-device rename で失敗していたが、data_dir 内 staging なのでその罠を踏まない。
大型 DL (TRT pack ~6.8GB / 編集 pack ~550MB) も `<exe_dir>\data` に入る (= フォルダ削除で完全アンインストール)。
TRT engine builder の子プロセス (`current_exe() --tensorrt-build`) も portable exe 自身を再実行するので問題なし。

> マーカーファイル方式 (`portable.txt` を隣に置いたら portable) は採らず、**feature build 自体で判定**する。
> 配布バイナリが別物 (portable exe は埋め込みを持たない) なので、通常 exe が誤ってポータブル化する事故は
> 原理的に起きない。マーカー方式より単純で堅い。

### 4.5 single_instance の mutex 名 (衝突回避)

現状 mutex 名は `Global\mImageViewerInstance_v1` 固定 ([single_instance.rs:60](../src/single_instance.rs))。
このままだと **インストール版がトレイ常駐中にポータブル版を起動しても、ポータブルが
「既に起動済み」と判断して既存ウィンドウを前面に出すだけで起動しない**(データディレクトリが
別物なのに別アプリ扱いされない)。

→ portable ビルドでは **mutex/event 名に suffix を付けて分離**する (cfg ゲート):

```rust
#[cfg(not(feature = "portable"))]
pub const MUTEX_NAME: &str = "Global\\mImageViewerInstance_v1";
#[cfg(feature = "portable")]
pub const MUTEX_NAME: &str = "Global\\mImageViewerInstance_portable_v1";
```

これで「インストール版 + ポータブル版を同時に動かす」が可能になる (別 data_dir なので DB 衝突なし)。
ポータブル同士の 2 重起動排除は引き続き効く。
**注意**: launcher の build.rs は `single_instance.rs` から定数を抽出する ([crates/launcher/build.rs:99](../crates/launcher/build.rs))。
portable は launcher を使わないので影響しないが、抽出ロジックが cfg 付き定数を正しく拾えるか確認する
(現状は `pub const MUTEX_NAME` 行を文字列マッチ。cfg 行が増えると最初の 1 個を拾う点に注意)。

### 4.6 build.rs (本体) の扱い

- `check_vendor_files()` ([build.rs:60](../build.rs)) は通常ビルドの include_bytes と FFmpeg の
  import lib 链接の両方に必要なので**そのまま維持**。portable でも vendor ファイルは「loose 同梱の
  コピー元」として必要なので、存在チェックは有益。
- PE リソースの `OriginalFilename` は通常 `mimageviewer-core.exe`。portable では配布名が
  `mimageviewer.exe` になるので、cfg で出し分けるか、パッケージング後のリネームに任せる (cosmetic)。

### 4.7 パッケージング (`scripts/build-portable.ps1` 新規)

`build-release.ps1` を参考にした専用スクリプト。手順:

1. `cargo build --release --bin mimageviewer-core --features portable` で core を生成し、
   `cargo build --release -p mimageviewer-remote --bin mimageviewer-remote --features embedded-web-assets`
   で同じ source tree の Web UI 資産内包 remote service を生成。
   - launcher (`-p mimageviewer-launcher`) は**ビルドしない**。
   - VST3 bridge (`mimageviewer-vst3-host.exe`) は **同梱しない** (下記の注を参照)。
2. 配布フォルダ `dist/portable/` を組み立て:

```
mImageViewer_portable/
├─ mimageviewer.exe                  (= mimageviewer-core.exe をリネーム)
├─ mimageviewer-remote.exe           (= core が同じディレクトリから起動)
├─ avcodec-61.dll  avformat-61.dll  avutil-59.dll
├─ avfilter-10.dll  swscale-8.dll  swresample-5.dll
├─ pdfium.dll
├─ onnxruntime.dll  onnxruntime_providers_shared.dll
├─ mimageviewer-susie32.exe
├─ models/
│   ├─ realesrgan_x4plus.onnx  ... (7 本)
├─ readme.txt                        (ポータブル版用。解凍即起動・data/ 説明・LGPL 通知)
└─ data/                             (初回起動時に自動作成。zip には空 or 同梱しない)
```

> ⚠ **VST3 host exe は v2.0.0 以降ポータブル版に同梱しない**。未署名の
> `mimageviewer-vst3-host.exe` を一部のセキュリティソフトがランサム誤検知し、ブラウザの
> zip ダウンロードがブロックされる事象があったため (commit `ec83fee0`)。同梱しないことで
> `src/video/dsp/vst3_supported()` が `false` を返し、VST3 機能はアプリ側で自動無効化される
> (設定でも ON 不可)。`build.rs` の vendor チェックも `CARGO_FEATURE_PORTABLE` 時は
> vst3-host を必須にしない。恒久対策 = bridge exe をコード署名 → 署名後に
> `build-portable.ps1` の copy 行と build.rs の条件、本節を合わせて復活させる。

4. **同梱漏れ検出**: コピーするファイル一覧を `native_assets` の Asset 列挙と FFmpeg DLL 名から
   導出し、コピー後に全ファイルの存在を assert。1 つでも欠けたらスクリプトを fail させる
   (build.rs の vendor チェックと同じ思想)。
5. `mImageViewer_portable_v<VERSION>.zip` に圧縮。

### 4.8 LGPL 同梱 (FFmpeg)

通常版と同じ義務。`LICENSE.txt` (FFmpeg LGPLv3) を zip 同梱し、DLL 名は改変しない (元々改変していない)。
ソフトウェア情報の LGPL 通知は core 内蔵なので変更不要。詳細は
[ffmpeg-lgpl-source-distribution.md](ffmpeg-lgpl-source-distribution.md)。

## 5. 配布物まとめ + 命名スキーム (実装後)

### 配布物一覧

| 配布物 | ファイル名 | 形態 | 配布先 | 管理者権限 | AV リスク |
| --- | --- | --- | --- | --- | --- |
| 単体 exe (launcher) | `mimageviewer.exe` | 内包+展開+spawn | mikage.to | 不要 | **高** (dropper 挙動) |
| インストーラ本体 | `mImageViewer_setup.exe` | Inno インストーラ | mikage.to / 窓の杜 / Vector | **要** | 中 |
| インストーラ zip | `mImageViewer_installer_v<VER>.zip` | setup.exe + readme.txt | Vector / 窓の杜 | — | — |
| **ポータブル版 zip** (新規) | `mImageViewer_portable_v<VER>.zip` | loose、解凍即起動 | mikage.to | 不要 | **低** (展開ゼロ) |

### 命名スキームの方針 (確定 2026-06-07)

- zip が 2 種類になるため、対の接尾辞 `_installer_` / `_portable_` で明示区別する。
  現行の汎用名 `mImageViewer_v<VER>.zip` は「どちらの zip か」が曖昧になるので廃止。
- **`mImageViewer_setup.exe` 自体の名前は変えない** (窓の杜が zip 内に求めるのはこれ。中身の名前を
  変えると申請が煩雑)。変えるのは外側の zip 名だけ。
- **用語整理**: 単体 exe (launcher) を「ポータブル版」と呼ばない (APPDATA に書くため)。
  「単体exe版 / オールインワン版」とし、「ポータブル」は loose-deps zip 専用に予約する。
  → CLAUDE.md「Distribution」節の `mimageviewer.exe (ポータブル版…)` 表記も合わせて修正する。
- **適用タイミング**: リネームは **ポータブル版を出す v1.1.0 のリリースで同時に切り替える**
  (zip が 2 つ並ぶタイミングで初めて区別が意味を持つ。それまでは現行名のまま)。
  **リリース済みの成果物は遡って改名しない** (過去版・配布済み zip はそのまま。新命名は v1.1.0 以降の新規分のみ)。
- **採用時に更新する箇所**: CLAUDE.md「Distribution」節 / 「リリース手順 Phase 3 step 11」
  (現状 `mImageViewer_v<VERSION>.zip` を参照) / 本書の配布物表。
- Vector は通常ファイル名でなく登録ソフト単位で識別するため改名は問題ない見込み (申請時に確認)。

> ポータブル版が安定したら、将来的に単体 exe (launcher) 版をポータブル版で置き換える選択肢もある
> (AV リスクが構造的に低いため)。v1.1.0 では併存させ、様子を見る。

## 6. メンテナンス上の保証 (一人開発でも回る根拠)

| いつ | 何を | 自動/手動 |
| --- | --- | --- |
| 毎コミット | `cargo check --features portable` (CI 1 行) | **自動**。portable 分岐の腐りを機械検出 |
| リリース時 | portable exe を起動 → PDF/動画/AI/Susie を 1 回ずつ叩く smoke | 手動・1 項目 (release checklist に追記) |
| native 依存を新規追加時 | `Asset` 列挙 + パッケージング一覧に 1 エントリ | 手動・年数回 |
| FFmpeg メジャー更新時 | 同梱 DLL 一覧も更新 (既存の「3 箇所更新」に +1) | 手動・既存イベント内 |

ポータブル分岐は薄い静的な継ぎ目で、機能開発で日常的に触る箇所と交差しないため、
**一度検証すればその後は安定**する。コンパイル破綻は CI が番人。

## 7. 段階的実装計画 (Phase)

1. **Phase 1 — 集約**: `src/native_assets.rs` を作り、6 経路を `resolve()` 経由に置換
   (通常ビルドの挙動は不変。リファクタとして単独でレビュー可能)。
2. **Phase 2 — feature 追加**: `portable` feature + include_bytes const の cfg ゲート +
   `resolve()` の portable 分岐 + data_dir 検出 + mutex 名分離。
   `cargo build --features portable` が通り、loose 配置で起動することを確認。
3. **Phase 3 — パッケージング**: `scripts/build-portable.ps1` + zip レイアウト + 同梱漏れ assert +
   ポータブル用 `readme.txt`。
4. **Phase 4 — CI + 検証**: CI に `cargo check --features portable`。release checklist に
   portable smoke を追記。§8 のワンタイム検証を実施。
5. **Phase 5 — 文書反映**: CLAUDE.md「配布形態」「リリース手順」、`htdocs` のダウンロード節、
   `docs/spec.md` を更新。

## 8. ワンタイム検証チェックリスト (Phase 4)

ポータブル exe を **C ドライブ以外** (例: `D:\test\mImageViewer_portable\`) に置いて:

- [ ] 解凍直後、ダブルクリックで起動する (管理者権限プロンプトが出ない)。
- [ ] 起動後、`<exe_dir>\data\` に settings.db / logs が作られる。APPDATA は触られない。
- [ ] PDF を開いてページが描画される (pdfium.dll loose 解決)。
- [ ] 動画を再生できる (FFmpeg DLL loose 解決、launcher 不在でロード成功)。
- [ ] リモート接続を開始できる (core と同じディレクトリの remote service を起動、protocol 一致)。
- [ ] AI アップスケール / デノイズが動く (onnxruntime + models loose 解決)。
- [ ] Susie プラグインが読める (susie32 worker loose 解決 + spawn)。
- [ ] VST3 が**自動無効化**されている (環境設定→動画タブで「VST3 プラグイン処理」が選択不可・
      「ポータブル版では利用できません」表示。`vst3_supported()` が false。host exe 非同梱)。
- [ ] インストール版をトレイ常駐させた状態でポータブル版を起動 → **両方独立して動く** (mutex 分離)。
- [ ] フォルダごと別 PC にコピーしても動く (絶対パス依存がない)。
- [ ] (AV) 手元の Defender / 主要 AV でスキャン → 単体 exe 版より誤検知が減る/消えることを確認。

## 9. 未決事項 → 全て確定 (2026-06-07 実装時)

1. ~~**read-only メディアでの data_dir**~~ **【確定 2026-06-06】**: `<exe_dir>\data` が書込不可のとき、
   **フォールバックせず明確なエラーで起動拒否**する (ポータブル性優先)。ユーザー判断: 黙って APPDATA に
   逃がすのはポータブルを選ぶ意図に反し最も嫌われる。`--data-dir` 明示時のみ read-only でも起動可
   (抜け道)。詳細は §4.4。実装: `data_dir::init` の `ensure_writable` + `portable_fatal_unwritable`。
2. ~~**mutex 分離の是非**~~ **【確定 2026-06-07: 分離】**: portable は mutex/event 名を `_portable_v1`
   接尾辞で分離し、インストール版トレイ常駐中でも独立起動できる (別 data_dir なので DB 衝突なし)。
   実装: `single_instance.rs` の cfg-gated `MUTEX_NAME` / `ACTIVATE_EVENT_NAME` / `SHUTDOWN_EVENT_NAME`。
3. ~~**単体 exe (launcher) 版の去就**~~ **【確定 2026-06-07: 併存】**: v1.1.0 は単体exe版・インストーラ版・
   ポータブル版の 3 形態を併存。将来ポータブルへ一本化するかは実績を見て判断。
4. ~~**models のディスク使用量**~~ **【確定: 問題なし】**: portable core exe は 53MB (埋め込み ~280MB を
   除去)、zip 全体 237MB。models は loose で `models/` に同梱され二重保持なし。任意ドライブに展開でき、
   フォルダ削除で完全アンインストールできるため要望を満たす。

## 9.5. 既知の制限 — 一時的なシステム temp 書き込み (Codex P3 2026-06-07)

ポータブルの契約は「**永続データ** (設定 / キャッシュ / DB / モデル / DL 物) は `<exe_dir>\data` に置き
APPDATA を使わない」こと。これは満たしている。一方、以下の **一時ファイル** は依然 system temp
(`%TEMP%`、C:) に書かれる (いずれも処理後に削除される transient な write で、永続データではない):

- `settings_restore.rs` の `validate_backup`: バックアップ検証時に temp へコピーしてから開く。
- `ui_dialogs/context_menu.rs`: コンテキストメニューのファイル操作で PowerShell スクリプトを temp に置く。

どちらもポータブル機能で新規に増やしたものではなく、**既存挙動**。auto-clean される transient temp
なので「C ドライブにゴミが溜まる / APPDATA を汚す」という要望には抵触しない。厳密に「C: へ一切書かない」
を求める場合は staging を `data_dir` 配下へ移す改修が必要だが、優先度は低く本実装の対象外とする。

## 10. 補足: コード署名 (ポータブル化と独立した AV 対策)

AV 誤検知は「unsigned + 低レピュテーション + dropper 挙動」の合わせ技。ポータブル化は
**dropper 挙動を除去**する対策。残る「unsigned + 低レピュテーション」は OV/EV コード署名証明書で
緩和できる (署名すればランチャー版の誤検知も減る)。両者は別軸で、両方やると最も堅い。
署名は費用・更新運用が絡むので本計画とは分けて検討する。

## 関連ドキュメント

- [architecture-overview.md](architecture-overview.md) — 全体構造・永続化ストア一覧
- CLAUDE.md「FFmpeg LGPL DLL 管理」「ONNX Runtime 管理」「PDFium 管理」「Susie 32bit ワーカー管理」
  「VST3 host bridge 管理」— 各 native 依存の埋め込み/展開の現状仕様
- [ffmpeg-lgpl-source-distribution.md](ffmpeg-lgpl-source-distribution.md) — LGPL 配布義務

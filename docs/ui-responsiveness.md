# UI スレッドの応答性 (Ctrl+↑↓ 連打が引っかからないために)

UI スレッドをブロックしないための設計方針と、2026-04 の Ctrl+↑↓ 引っかかり調査で
得られた知見をまとめたもの。新しい機能を追加するとき、先に**この文書の §4 チェックリスト**
を通すこと。UI 同期 I/O は一度入れると気付きにくく、直すときは大きなリファクタになる。

**関連**:
- [async-architecture.md](async-architecture.md) §5 よくある事故パターン (並列処理のアンチパターン)
- [async-architecture.md](async-architecture.md) §7 perf.rs 計装の使い方

---

## 1. 原則: UI スレッドで避けるべき処理

`App::update` から直接 (あるいは呼び出し先から同期的に) 呼ばれるコードで、以下はすべて
**必ずバックグラウンドスレッド化する**。ユーザー体感で 16ms (60fps) 以上のフリーズを
生む可能性があるものは例外なく対象。

| 処理 | 実測コスト | 対策パターン |
| --- | --- | --- |
| ファイルを開いて中身を読む (`std::fs::read`, `File::open` + `read_to_end`) | 5MB JPEG で 50-200ms、20MP PNG で 100-500ms | 専用 worker thread + mpsc::Receiver |
| `xmp_reader::read_tweet_info(path)` (JPEG/PNG は full-file 読み) | 20MP JPEG で 100ms+ | `start_metadata_load` 経由でバックグラウンド |
| `png_metadata::extract_metadata(path)` | 数 ms だが PNG のみ | 上と同じ worker に束ねる |
| 画像デコード (`image::load_from_memory`) | 5MB で 200-800ms | `start_fs_load` worker |
| SQLite DB の open (`CatalogDb::open`) | cold で 30-150ms、warm で 1-5ms | 通常パスは OK。異常時は LRU キャッシュ検討 |
| `ctx.load_texture` (GPU アップロード) | 20MP RGBA で 26-58ms、1 フレームに複数来ると合算 | `fs_upload_backlog` にステージして 1 枚/frame |
| `std::fs::read_dir` + **`Path::is_dir()` / `Path::is_file()`** per-entry | Windows では per-entry で `GetFileAttributes` syscall。500 ファイルで 500-1000ms | `DirEntry::file_type()` を使う (`FindFirstFile` のキャッシュ再利用) |
| DFS フォルダ走査 (`navigate_folder_with_skip`) | HDD で 20-300ms、最悪 1s+ | 常に別スレッド (`spawn_folder_nav`) |
| SQLite の `search()` (FTS クエリ) | インデックス小なら数 ms、大なら 100ms+ | `execute_favsearch` がバックグラウンド化 |

### 1.1 Windows 固有: `Path::is_dir` は syscall、`DirEntry::file_type` はキャッシュ

`std::fs::read_dir` は内部で `FindFirstFile` / `FindNextFile` を呼び、各エントリの
`WIN32_FIND_DATA` (ファイル属性・サイズ・時刻) を取得している。

- **`entry.file_type()` / `entry.metadata()`**: FindFirstFile の結果を再利用する。
  追加 syscall なし。O(1)。
- **`entry.path().is_dir()` / `.is_file()`**: 新たに `GetFileAttributes` を発行する。
  per-entry で syscall が増えるため、数百エントリで数百 ms のブロックになる。

2026-04 の事件: 対策 B (DFS で scan_directory 事前実行) を入れた直後、AI 画像 200 枚の
フォルダで DFS + scan に **970ms** かかるようになった。原因は `scan_directory` 内で
`p.is_dir()` を per-entry で呼んでいたこと (実装者の無意識、Rust では自然な書き方)。
`entry.file_type()` に置換したところ **85ms** に短縮。

**ルール**: `read_dir` の中で is_dir / is_file / metadata を使うときは必ず DirEntry 経由。
Path 経由のヘルパーは使わない。

---

## 2. 非同期 I/O の実装テンプレ

```rust
// (1) App に pending 状態を追加
pub(crate) struct XxxPending {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<XxxResult>,
    // 必要なら identifier (idx, key, seq) を追加
}

pub struct App {
    pub(crate) xxx_pending: Option<XxxPending>,
    // ...
}

// (2) 起動: 既存 pending を cancel、スレッド spawn、結果を後で受ける
pub(crate) fn start_xxx(&mut self, ...) {
    if let Some(p) = self.xxx_pending.take() {
        p.cancel.store(true, Ordering::Relaxed);
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_w = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();
    // snapshot すべきデータは Clone してスレッドへ渡す
    let input_snapshot = self.some_data.clone();
    std::thread::Builder::new()
        .name("xxx-worker".to_string())
        .spawn(move || {
            // 重要: 途中で cancel チェック。タイトなループ内に入れる
            if cancel_w.load(Ordering::Relaxed) { return; }
            let result = run_xxx(&input_snapshot, &cancel_w);
            if cancel_w.load(Ordering::Relaxed) { return; }
            let _ = tx.send(result);
        })
        .ok();
    self.xxx_pending = Some(XxxPending { cancel, rx });
}

// (3) UI スレッドで毎フレーム poll
pub(crate) fn poll_xxx(&mut self) {
    let Some(p) = self.xxx_pending.as_ref() else { return };
    match p.rx.try_recv() {
        Ok(result) => {
            // キャッシュに merge (キー単位の insert、全クリアはしない)
            self.xxx_pending = None;
        }
        Err(mpsc::TryRecvError::Empty) => {}
        Err(mpsc::TryRecvError::Disconnected) => {
            self.xxx_pending = None;
        }
    }
}

// (4) update() の終端付近で poll + 次フレーム要求
self.poll_xxx();
if self.xxx_pending.is_some() {
    ctx.request_repaint(); // egui は dirty がないと sleep するので明示要求
}

// (5) フォルダ切替・コンテキスト無効化で cancel
if let Some(p) = self.xxx_pending.take() {
    p.cancel.store(true, Ordering::Relaxed);
}
```

### 2.1 キャンセルの置き場所

3 箇所で cancel できるようにするのが基本:

1. **新規起動時**: 新しい `start_xxx()` で古い pending を cancel (連打時は最新だけ活かす)
2. **コンテキスト終了時**: `close_fullscreen` / `start_loading_items` (フォルダ切替) で
   対応する pending を cancel (無効化されたインデックスで結果が書き戻されない)
3. **ループ内早期離脱**: worker の長いループ内で `cancel.load(Ordering::Relaxed)` を
   定期チェック (タイルデコード / マッチング等の per-item ループ内に入れる)

### 2.2 キャッシュへのマージルール

worker が途中で新しいエントリを読んだ場合、結果を UI に返して `cache.insert` させる。
`cache.entry(k).or_insert(v)` で「既存キーは上書きしない」にすることで、並行する別の
worker や UI 側の優先書き込みを尊重できる。例: [execute_search](../src/app.rs) の
`xmp_additions` マージ。

---

## 3. GPU テクスチャアップロード

`ctx.load_texture(name, ColorImage, options)` は UI スレッドで同期実行され、内部で
wgpu の queue.write_texture が走る。20MP RGBA (78MB) で 26-58ms かかる。

### 3.1 ペーシングパターン

フルスクリーン prefetch は 1 フレームに 10 枚以上の画像を完了させることがあり、
これらを UI スレッドで連続アップロードすると UI が 500ms+ 固まる (2026-04 実測: 535ms)。

対策:
1. worker はデコード結果を `FsLoadResult` で送る (この時点では ColorImage、GPU 未アップ)
2. UI は完了を `fs_upload_backlog: Vec<(idx, FsLoadResult, seq)>` にステージ
3. 1 フレームにつき **最大 1 枚**だけ `ctx.load_texture` する
4. ただし **現在 `fullscreen_idx` に対応するエントリは即時アップロード** (表示遅延ゼロ)
5. backlog が残っていれば `ctx.request_repaint()` で次フレーム継続

実装は [src/app.rs `poll_prefetch`](../src/app.rs) 参照。

静止画の `DISPLAY_IMAGE_TEXTURE_OPTIONS` は level 0 upload の直後に vendored `egui-wgpu` の
render pass で mip chain も生成する。CPU resize や I/O は増えないが、GPU upload slot 1 件あたりの
仕事量と VRAM は増える（完全な chain は level 0 の約 1/3 追加）。したがって mipmap 対象を同じ
`fs_upload_backlog` のペーシング下に置き、サムネイルや animated frame へは広げない。
wipe/diff比較はpair生成時、360度パノラマはbase upload時だけ同じGPU生成を行う。比較callbackは
現在の1組だけを保持し、再準備時には旧組を新規確保前にdropする。右下のピン表示はpin workerが
72x54以下へ縮小した専用textureを使い、UI threadでフル解像度textureを追加uploadしない。
360のsettle overlayは画面解像度相当なので1 mipのままにする。連結読みのtexel上限は完全なmip chainと
同時保持するerase / local-adjust / conceal / edit / final composite / comic / adjustment textureを
TextureIdで重複排除して見積もり、補正レイヤーの比較previewも同じkeep-set evictionへ追従させる。

### 3.2 サムネイルアップロードは per-frame スロットリング不要

`thumb.ready` の `upload_ms` は 1 枚 0.3-1ms 程度 (762×572 RGBA)。1 フレームに 50 枚来ても
合計 50ms なので致命的ではない。サムネイルは特別なペーシングなしで OK。

---

## 4. 新機能追加時のチェックリスト

機能追加・変更で以下を触るときは、この順で確認する。

- [ ] **file I/O (read, open, metadata, `Path::exists` / `try_exists`)**: UI スレッドから呼ぶなら 1ms 以下に収まるか?
      超えるなら worker thread に出す。
- [ ] **`read_dir` ループ内で `path.is_dir()` / `is_file()` を使っていないか**: Windows で
      遅い。`entry.file_type()` を使う。
- [ ] **SQLite クエリ**: キャッシュヒット時は OK だが、cold open や大量 SELECT は
      worker thread に出す。
- [ ] **`ctx.load_texture`**: 1 フレームに 1 回程度か? 複数来る場合はペーシング必要。
- [ ] **UI context 依存の大量 CPU 処理** (文字レイアウト等): worker へ移せない場合は 1 frame の
      件数上限を持つ state transition に分割し、全件 exact へ収束させる。sample 化で既存機能を
      近似へ変えない。入力世代・表示順・font/style/DPI 変更時は古い state を破棄する。
- [ ] **画像デコード / XMP / EXIF 読み取り**: 絶対に worker thread。
      既存 `fs_pending` / `metadata_pending` に乗せられないか確認。
- [ ] **cancel パス**: フォルダ切替 / close_fullscreen / 新リクエスト投入時、
      古い pending を cancel できるか。
- [ ] **perf 計装**: 該当区間に perf::event を追加 (`--perf-log` 計測できるように)。
      `if crate::perf::is_enabled()` の外ガード必須 (extras の JSON 構築を避ける)。
- [ ] **測定**: 追加後に実機で `--perf-log` を取り、`python scripts/analyze_perf.py
      <path> nav hitches` で悪化してないか確認。

### 4.1 オーバーレイパネルの ScrollArea

`egui::Area + Frame::popup + ScrollArea` でフルスクリーン左パネルを作る場合は、
本文が少ないパネルと多いパネルで方針を分ける。

- 消しゴムのように本文が収まりやすいパネルは、`ScrollArea::max_height(...)` と
  `auto_shrink([false, true])` で「収まるときは内容サイズ、足りないときだけスクロール」にする。
  この場合、パネル上クリック判定は前フレームで実際に描画された `Frame` の rect を使い、
  見えない下側まで入力を吸わないようにする。
- 画像補正のように常時長いパネルは、ヘッダを固定し、本文 `ScrollArea` の親領域を明示確保する。
  さらに左余白とスクロールバー分の右余白を別に取り、ボタンやスライダーがバーに重ならない幅で配置する。

### 4.2 意図的な例外: native ファイル D&D のモーダルブロック

ファイル D&D 送出 (`src/file_drag.rs::start_file_drag`) は `SHDoDragDrop` を UI
スレッドで呼び、ドロップ完了 / キャンセルまで戻らない。`App::update` がその間
ブロックするが、これは **§1 の禁止対象ではない** — バックグラウンド I/O ではなく、
ユーザーが明示的に開始したモーダル操作であり、エクスプローラ含め全アプリ共通の
標準挙動 (ドラッグ元ウィンドウはドラッグ中固まって見えるのが正常)。`SHDoDragDrop`
自体が独自メッセージループを回すため OS レベルの応答性は保たれる。worker 化は
不可能 (`DoDragDrop` 系は呼び出しスレッドにマウスキャプチャを要求する)。
詳細は `docs/file-drag-drop-design.md` §6.2。perf ログを汚さないよう、実行は
`update` 末尾 (frame_total 計測の後) に置いている。

---

## 5. 既知のパターン: Ctrl+↑↓ 引っかかり (2026-04 解決済み)

ユーザー報告: 「Ctrl+↑↓ を長押ししていると時々引っかかる」。最大 535ms のフレームギャップ、
100ms超のヒッチが 28 回/102 秒観測された。

### 5.1 特定された原因 (優先度順)

| # | 症状 | 原因 | 対策 | 実装場所 |
|---|---|---|---|---|
| A | 20MP JPEG prefetch 15 枚が UI スレッドで連続 GPU アップロード (1秒に 478ms 占有) | 1 フレーム 1 枚ペーシング、現在ページのみ即時 | `fs_upload_backlog` | [src/app.rs `poll_prefetch`](../src/app.rs) |
| B | `load_folder` の `read_dir` + per-entry metadata が UI スレッドで 100-200ms | DFS スレッドで事前スキャンして結果を渡す | `ScannedDir` / `load_folder_with_scan` | [src/app.rs `spawn_folder_nav`, `scan_directory`](../src/app.rs) |
| B' | B で移した `scan_directory` が `p.is_dir()` per-entry 呼びで 970ms | `entry.file_type()` に置換 | - | [src/app.rs](../src/app.rs), [src/folder_tree.rs `sorted_subdirs`](../src/folder_tree.rs) |
| D | Ctrl+F / Ctrl+S 検索が UI スレッドで XMP/PNG メタ + SQLite を同期実行 | 検索ごと worker thread、結果 merge | `SearchPending` / `FavSearchPending` | [src/app.rs `execute_search`, `execute_favsearch`](../src/app.rs) |
| E | `ensure_metadata_loaded` が `open_fullscreen` のたびに AI/EXIF/XMP を UI スレッドで同期読み (20MP JPEG で 100ms+) | 1 worker で 3 パーサーを順次実行、結果をキャッシュ merge | `MetadataLoadPending` | [src/app.rs `start_metadata_load`, `run_metadata_load`](../src/app.rs) |

`ScannedDir` のworker handoffは `Result` を保持する。ネットワーク切断や権限エラー時に
`read_dir` 失敗を空フォルダへ変換してUIへinstallすると、表示が空になるだけでなく
`catalog.delete_missing()` が有効なサムネイル行を削除するため、失敗結果はloadへ適用しない。

### 5.2 効果 (measurement)

| 指標 | Original | 最終 | 改善率 |
|---|---|---|---|
| 33ms超ヒッチ | 360 | **104** | -71% |
| **100ms超ヒッチ** | **28** | **7** (うち 6 件は起動後 5 秒以内の wgpu ウォームアップ) | -75% |
| 定常時の最大ヒッチ | 535ms | 109ms (DFS 連続消化) | -80% |
| `input → apply_end` p99 | 156ms | **58ms** | -63% |
| `input → apply_end` max | 255ms | **70ms** | -73% |
| `lf_scan` (UI) max | 179ms | **0.4ms** | -99.8% |
| `dfs_scan` max | - | **85ms** (B' 前は 970ms) | -91% vs B' 前 |

**体感**: ユーザー確認「ほぼほぼ引っかかりがなくなり、快適」。

---

## 6. 起動時間の内訳 (2026-04 計測)

`startup.*` perf イベントによる main() 入口から初回フレームまでのブレークダウン。

| フェーズ | ms | 累計 ms | 制御 |
|---|---|---|---|
| data_dir_init | 0.0 | 3.2 | 自前 |
| models_extract (AI ONNX 展開) | 0.3 | 3.6 | 自前 (サイズ一致でスキップ) |
| susie_worker_extract (32bit ワーカー展開) | 0.3 | 3.8 | 自前 (同上) |
| settings_load (JSON パース) | 0.3 | 4.2 | 自前 |
| load_icon (icon.png パース) | 0.4 | 4.6 | 自前 |
| before_run_native | 0.0 | 4.7 | 自前 |
| **creator_enter (`eframe::run_native` 内部)** | **666.8** | **671.5** | **eframe (winit + wgpu)** |
| setup_fonts (Windows system font 読込) | 4.1 | 675.6 | 自前 |
| apply_theme | 0.0 | 675.6 | 自前 |
| app_default (App 構造体初期化) | 3.0 | 678.6 | 自前 |
| creator_exit | 0.0 | 678.6 | 自前 |
| first_frame (初回 update()) | 4.4 | **683.0** | egui/自前 |

**結論**: 起動時間の **98% は eframe (winit + wgpu) の初期化** (ウィンドウ生成、wgpu Instance
/Adapter/Device/Queue、シェーダーコンパイル)。自前コードは合計 **12ms** と既に最適化済み。

**アプリ側にボトルネックはない**。起動時間を短縮するには eframe 本体に手を入れる必要があり、
費用対効果が合わない。現状の 683ms は wgpu バックエンドの Rust GUI として標準的な値。

### 6.1 起動後の追加ウォームアップ

first_frame (683ms) 後、ユーザー操作可能になるまで更に ~300ms の wgpu パイプライン
JIT コンパイル + 初回テクスチャアップロードがある。`t=0.988s` に thumb.ready が大量到着。
これも wgpu 固有の挙動で、アプリ側の対処は不要。

---

## 7. 測定手順

起動時 + ナビゲーションを計測する標準手順。

```powershell
# 1. --perf-log を付けて起動
# (release ビルドで測る時は --log も付ける)
.\target\release\mimageviewer.exe --perf-log

# 2. 起動直後 3-5 秒待つ (wgpu ウォームアップ)
# 3. Ctrl+↑ / Ctrl+↓ を長押し、大きなフォルダを移動
# 4. Ctrl+F, Ctrl+S で検索も試す
# 5. アプリを終了

# 6. 分析
$Perf = "$env:APPDATA\mimageviewer\logs\perf_events.jsonl"
python scripts\analyze_perf.py $Perf startup   # 起動時間ブレークダウン
python scripts\analyze_perf.py $Perf nav       # Ctrl+↑↓ 区間別統計
python scripts\analyze_perf.py $Perf hitches --ms 100  # 100ms 超フレームギャップ
python scripts\analyze_perf.py $Perf dump <seq>  # 特定 input_seq のイベント列
```

### 7.1 判定基準

問題なしとみなせる目安 (2026-04 実測を基準):

- `input → apply_end` p99 < 80ms, max < 150ms (起動後)
- 100ms超ヒッチ: 起動後 5 秒経過後は 0 に近い (定常時 0-2 件)
- `lf_scan` (UI) p99 < 1ms (事前スキャン経路が機能していること)
- `sli_*` 各区間 p99 < 10ms
- `dfs_scan` p95 < 50ms (対策 B' が効いていること)

悪化した場合は `hitches` の直前 nav イベントで原因区間を特定し、§4 チェックリストを通す。

### 7.2 静止中・背面表示中の健全性

`hitches` は遅いフレームを探すため、短い処理を高速で再投入して CPU 1 コアとログを消費する
ループは別検査にする。release verification binary を `--perf-log` 付きで起動し、PowerShell
側から CPU time / wall time とログ増加を測りながら `analyze_perf.py idle-health` を実行する。

```powershell
.\scripts\check-idle-health.ps1 -Scenario static-foreground
.\scripts\check-idle-health.ps1 -NoLaunch -Scenario static-background
.\scripts\check-idle-health.ps1 -NoLaunch -Scenario video-pin-background
```

測定中の入力、update rate、repaint reason streak、同一 work 反復、CPU、ログ増加のいずれかが
上限を超えると exit 1。詳細手順・閾値・シナリオは
[idle-health-check.md](idle-health-check.md) を参照する。新しい polling / retry / idle work を
追加するときは、同じ安定状態を複数回評価して最終的に work 0 へ収束する単体テストも追加する。

### 7.3 UI フォント設定

v2.7.0 の UI フォント設定では、`fontdb::Database::load_system_fonts`、font file 読み込み、
TTC/OTC face ごとの glyph coverage 判定、`ab_glyph` プレビュー生成、egui font definitions の
準備をすべて worker で行う。UI スレッドで許可するのは、完了 channel の `try_recv`、小さい
プレビューの `load_texture`、準備済み定義の `Context::set_fonts` だけ。縦位置 slider は
150ms debounce し、連続ドラッグ中に大型 CJK font data の読み込み worker を増殖させない。

---

## 7a. 既知の同期 I/O 残課題 (v0.8.2 以降で worker 化)

v0.8.1 で Codex レビューが指摘したが、通常運用での体感影響が小さいため当面据え置いている
同期 I/O 経路。環境 (ネットワーク/外付け/AV heavy) によっては体感停止を起こすため、
まとまったリファクタ時に worker 化する。

- **archive cache hit 判定** — [src/ui_dialogs/archive_convert.rs `try_archive_cache_lookup`](../src/ui_dialogs/archive_convert.rs)
  - `std::fs::metadata(src)` + `ArchiveCacheDb::lookup()` (SQLite SELECT + cache ZIP の
    `exists()`、stale 時は `remove_file()`) を UI スレッドで実行。
  - 発火点は `ConvertibleArchive` (RAR/7z/LZH) を Enter / ダブルクリックする瞬間のみで、
    通常閲覧・サムネイル処理のホットパスではない。ローカルでは 1ms 以下。
  - 直すなら `ArchiveConvertPhase::CheckingCache` を追加して worker で lookup、結果で
    cache 即開 / 変換確認へ分岐する形が素直。
- **sidecar flush / import** — [src/app.rs `flush_idle_sidecars`, `flush_all_sidecars`](../src/app.rs) および `SidecarFile::load()` import 経路
  - `flush_idle_sidecars` は通常 update から、`flush_all_sidecars` はフォルダ切替から呼ばれ、
    いずれも `write` / `rename` / `remove_file` を UI スレッドで直接実行する。
  - sidecar backup が有効 + 保存先がネットワーク/外付け + mask/adjust を大量に持つ JSON
    では閲覧中・フォルダ切替時に数百 ms 停止した観測あり (コメント参照)。
  - sidecar は optional backup で adjustment DB が authoritative なので現状は許容範囲だが、
    v0.8.2 で snapshot → worker flush + import pending 化する。
  - フォルダ切替経路 (`start_loading_items` → `flush_all_sidecars`) を worker 化するときは、
    次フォルダの `load_folder` が旧 sidecar の flush 完了を待つ ack パスが要る点に注意。

どちらも Codex P3 (confidence 0.76-0.78)。§4 チェックリスト違反なので新しい同期 I/O を
足すときの悪例として参照してよい。

### フォルダオープン時の巨大 local_adjust JSON 読み (2026-06 解決済み)

`h:\home\mimageviewer_old\testimage` の cold open 調査で、`sli_local_adjust_db` が
約 2.5 秒を占めるケースを確認した。原因は `local_adjust.db` の `layers_json` が
1 ページ数十 MB まで肥大化している状態で、グリッドの `局` バッジ表示のためだけに
`load_layers_by_prefix` が JSON 本体まで一括取得・deserialize していたこと。

対策として、`load_folder` / `rehydrate_page_edit_state_for_current_items` は
現在 `items` の page key に対して `page_path IN (...)` の exact lookup だけを行い、
`local_adjust_pages` を復元する。`page_path` は PRIMARY KEY なので追加 index は不要。
実際の `layers_json` は `ensure_local_adjust_layers_loaded(idx)` で、フルスクリーン表示、
補正レイヤーパネル、エクスポート準備など該当ページの実体が必要になった時点で
1 ページ分だけ読む。

### 全検索 (Ctrl+G) の大量件数時スパイク

上記 2 件が「UI スレッドでの同期 I/O」なのに対し、こちらは「UI スレッドでの O(N) 計算」。
100 件程度では見えないが、横断お気に入り + 大量ヒットで顕在化する。

- **container mtime populate (Newer/Older ソート確定時)** — [src/global_search_ui.rs `ensure_container_mtime_populated`](../src/global_search_ui.rs)
  - ストリーミング中はスキップ済み (Codex 既往対応) だが、`done` になった時の 1 回だけ
    全コンテナに `std::fs::metadata()` を同期実行する。HDD / ネットワーク + 数百コンテナ
    だと 100-500ms ブロックする。
  - 直すなら (a) 検索 index の stored field に mtime を持たせて metadata 呼び出し自体を
    無くす、または (b) populate を worker 化して「取得中は path 順で暫定表示 → 完了時に
    再ソート」にする。(a) は index schema 変更が要るので重い。
- **cross-container nav list 構築** — [src/global_search_ui.rs `build_cross_container_nav_list`, `collect_hit_folders_dfs`](../src/global_search_ui.rs)
  - `collect_hit_folders_dfs` が全ヒットを走査するので、container 数 C × hit 数 H × depth。
    しかも fullscreen Ctrl+→/← は画像が見つかるまで `global_search_ctrl_nav` をループ
    する ([src/global_search_ui.rs `global_search_ctrl_nav_fullscreen`](../src/global_search_ui.rs))
    ため、Folder 枝ばかりの候補を跨いで進むたびに nav list を再構築する。
  - 直すなら `GlobalSearchState` に `nav_list_cache: Option<Vec<NavEntry>>` を持たせ、
    `containers` / `all_hits` / `sort_mode` / `done` が変わったら invalidate。あるいは
    `all_hits` を container_root ごとに前処理して単一 pass で全 nav list を作る。

どちらも Codex P3 (confidence 0.82-0.86)。v0.8.1 の即ブロッカーではないが「大量件数でも
UI を止めない」方針の未達項目として残す。

---

## 8. 関連コード (landmark)

| 役割 | 場所 |
|---|---|
| perf::event 基盤 + init | [src/perf.rs](../src/perf.rs) |
| `fs_upload_backlog` ペーシング | [src/app.rs `poll_prefetch`](../src/app.rs) |
| `scan_directory` (DirEntry::file_type 版) | [src/app.rs `scan_directory`](../src/app.rs) |
| DFS 事前スキャン | [src/app.rs `spawn_folder_nav`](../src/app.rs) |
| バックグラウンド Ctrl+F 検索 | [src/app.rs `execute_search`, `run_metadata_search`](../src/app.rs) |
| バックグラウンド Ctrl+S お気に入り検索 | [src/app.rs `execute_favsearch`, `poll_favsearch`](../src/app.rs) |
| バックグラウンドメタデータ読み | [src/app.rs `start_metadata_load`, `run_metadata_load`](../src/app.rs) |
| 起動時間計装 | [src/main.rs `emit_startup`](../src/main.rs), [src/app.rs `startup.first_frame`](../src/app.rs) |
| 解析スクリプト | [scripts/analyze_perf.py](../scripts/analyze_perf.py) |

---

## 9. hidden viewport の常時維持を避ける (2026-05-10 追加)

`ctx.show_viewport_immediate(...with_visible(false), ...)` を「次回表示のちらつき防止」目的で
毎フレーム呼ぶと、hidden viewport の維持コストが他フレーム作業を圧迫する。2026-05-10 の
perf log では `fullscreen_viewport_ms` が 30-70ms/frame を占めており、`keep_fullscreen_viewport_alive`
内の inactive hidden viewport 維持が主要候補だった (修正後は `keep_fullscreen_viewport_ms` /
`render_fullscreen_viewport_ms` / `ensure_native_video_front_ms` の分割計装で確認可能)。
非アクティブ時には呼ばないこと。終了直後 1 フレームだけ `Visible(false)` cmd 送信用に
show_viewport_immediate を呼ぶ用法は OK。

再入場時のちらつき対策は、アイドル時 keep_alive ではなく表示直前の hidden 生成で行う。
`render_fullscreen_viewport` は新規 viewport を `with_visible(false)` で作成し、作成後に
`DWMWA_TRANSITIONS_FORCEDISABLED` を適用してから `Visible(true)` を送る。これにより
1x1 → フルサイズの DWM 遷移や初期 white client の露出を抑えつつ、非アクティブ時の
viewport 維持コストを戻さない。

静止画の F11 ウィンドウ内表示切替では、`render_fullscreen_viewport` の実行中に
`native_video_in_window_active` が反転する。`App::update` は render 前後の embedded 判定を
OR してグリッド描画を抑止し、viewport → embedded では main 側へ画像を描いてから古い
fullscreen viewport を hidden にする。これにより背面の一覧が一瞬露出するのを避ける。
embedded → viewport では専用 viewport が OS 側で前面に出るまで main 側が通常グリッドへ
戻らないよう、短い `still_fullscreen_viewport_enter_suppress_until` 期間だけ黒地 +
holdover 画像を描く。これは既存テクスチャ参照の描画だけで、hidden viewport の常時維持や
同期ロードは増やさない。

**関連ルール**: `keep_fullscreen_viewport_alive` 実行後に `close_fullscreen` する経路で、
同フレーム内に fullscreen を再 open しない場合は明示的に `ctx.request_repaint()` を呼ぶ。
修正後の keep_alive はアイドル時ゼロコスト早期 return するため、cleanup 用の次フレームを
偶発的な input/focus repaint に依存させてはいけない。

**統一安全網**: `App::update` 末尾で `if self.fs_viewport_shown && self.fullscreen_idx.is_none()`
が true なら `ctx.request_repaint()` を呼ぶ。これにより `close_fullscreen` が update 内の
どこで呼ばれても次フレームで cleanup が確実に走る。個別 call site の `request_repaint` 追加は
意図表明としての価値はあるが、漏れたケースもこの安全網が拾う。

---

## 10. Overlay panel layout (Area + Frame::popup + ScrollArea)

フルスクリーン上の固定パネルを `egui::Area::fixed_pos` + `Frame::popup` +
`ScrollArea` で作る場合、`ScrollArea::max_height` だけではパネルが下端まで伸びる
保証にならない。egui 0.33 の `ScrollArea` は、最初に親 `Ui` の
`available_rect_before_wrap()` と `max_height` の小さい方を使うため、`Area` /
`Frame::popup` の自動サイズ文脈ではコンテンツ高に引きずられることがある。

下端近くまで伸びるパネルでは、ScrollArea の直前に親領域を明示確保する:

```rust
let body_height = (full_rect.max.y - ui.cursor().top() - PANEL_BOTTOM_MARGIN)
    .max(PANEL_MIN_BODY_H);
ui.allocate_ui_with_layout(
    egui::vec2(PANEL_W, body_height),
    egui::Layout::top_down(egui::Align::LEFT),
    |ui| {
        ui.set_min_height(body_height);
        egui::ScrollArea::vertical()
            .max_height(body_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // panel body
            });
    },
);
```

ヘッダ (タイトル、プレビュー、閉じるボタン) は ScrollArea の外に置く。これにより
縦スクロールバーが閉じるボタンと重ならず、ヘッダもスクロールしない。
パネルクリック吸収用の sink rect と、キャンバス操作を抑制する panel rect は、
可視パネルが伸びた高さに合わせて動的に作る。固定 1000px のような値は 1440p /
4K / 縦長ウィンドウで下端まで届かない。

内容量に合わせてコンパクトにしたいパネルでは、前フレームの
`ScrollAreaOutput::content_size.y` を保存し、その値を次フレームの確保高さに使う。
この場合も、親領域を確保してから `ScrollArea` を置く点は同じ。

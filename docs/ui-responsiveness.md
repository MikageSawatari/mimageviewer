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

### 3.2 サムネイルアップロードは per-frame スロットリング不要

`thumb.ready` の `upload_ms` は 1 枚 0.3-1ms 程度 (762×572 RGBA)。1 フレームに 50 枚来ても
合計 50ms なので致命的ではない。サムネイルは特別なペーシングなしで OK。

---

## 4. 新機能追加時のチェックリスト

機能追加・変更で以下を触るときは、この順で確認する。

- [ ] **file I/O (read, open, metadata)**: UI スレッドから呼ぶなら 1ms 以下に収まるか?
      超えるなら worker thread に出す。
- [ ] **`read_dir` ループ内で `path.is_dir()` / `is_file()` を使っていないか**: Windows で
      遅い。`entry.file_type()` を使う。
- [ ] **SQLite クエリ**: キャッシュヒット時は OK だが、cold open や大量 SELECT は
      worker thread に出す。
- [ ] **`ctx.load_texture`**: 1 フレームに 1 回程度か? 複数来る場合はペーシング必要。
- [ ] **画像デコード / XMP / EXIF 読み取り**: 絶対に worker thread。
      既存 `fs_pending` / `metadata_pending` に乗せられないか確認。
- [ ] **cancel パス**: フォルダ切替 / close_fullscreen / 新リクエスト投入時、
      古い pending を cancel できるか。
- [ ] **perf 計装**: 該当区間に perf::event を追加 (`--perf-log` 計測できるように)。
      `if crate::perf::is_enabled()` の外ガード必須 (extras の JSON 構築を避ける)。
- [ ] **測定**: 追加後に実機で `--perf-log` を取り、`python scripts/analyze_perf.py
      <path> nav hitches` で悪化してないか確認。

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

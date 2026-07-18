# PDF Page Count Cache (C-base + C-thumb) 実装計画

> **このドキュメントは v1.0.0 開発初期 (round 0) の設計メモ**。
> 実装は Codex レビュー round 1〜4 を経て進化しているので、最新の動作仕様は
> 直下の「## 最終形 (v1.0.0 実装)」セクションを参照。round 0 の素案 (= ストレージ
> 設計の出発点、トレードオフ検討) として下のセクションは残してある。

## 最終形 (v1.0.0 実装)

### Schema

```sql
CREATE TABLE IF NOT EXISTS pdf_meta (
    filename          TEXT    NOT NULL PRIMARY KEY,  -- 親フォルダ内の PDF ファイル名
    mtime             INTEGER NOT NULL,
    file_size         INTEGER NOT NULL,
    page_count        INTEGER NOT NULL,
    password_required INTEGER NOT NULL DEFAULT 0    -- 0=不要、1=必要 (round 1 で追加)
);
```

### `CatalogDb` の 3 つの書き込み API

`password_required` 列を「**確信できる場合だけ確信値で書く、不明なときは触らない**」
という方針で 3 つに分割している (Codex round 1-3 対応):

| メソッド | 新規行 | 既存行 | 用途 |
|---|---|---|---|
| `set_pdf_meta(.., pw_req)` | INSERT (pw_req 明示) | OVERWRITE | enumerate 成功 + 確信あり (saved pw 使用、または password=None で成功) |
| `set_pdf_meta_safe(..)` | INSERT (pw_req=0) | OVERWRITE pw_req=0 + page_count/mtime/size 更新 | password=None で render 成功 (= 不要確信) |
| `set_pdf_meta_thumb(..)` | **何もしない** | `WHERE filename=? AND mtime=? AND file_size=?` 一致時のみ page_count 更新 | unknown (session pw 使用、保存しないパスワード入力直後) |

`set_pdf_meta_thumb` が WHERE 条件に `mtime/file_size` を含むのは、ファイル置換で stale 行を
誤って promote しないため (Codex round 3 P1)。

### CatchupQueue (round 3 で導入)

`src/thumb_loader.rs` に**専用ワーカースレッド + 優先度別 bounded queue**:

```
            ┌──────────────────────────────────────┐
            │ Mutex<CatchupQueueState> + Condvar   │
            │                                       │
   enqueue ─→ high (cap 16) : NeighborPrefetch    │
   enqueue ─→ low  (cap 256): MetaOnly            │
            │ pending: HashSet (dedup)              │
            └────────────┬─────────────────────────┘
                         ↓
            ┌──────────────────────────────────────┐
            │ 1 worker thread                       │
            │   high 優先で pop → 処理 → pending 削除 │
            └──────────────────────────────────────┘
```

- **MetaOnly** (low lane): WebP cache hit 経路。`enumerate_pages(password=None)` のみ実行。
- **NeighborPrefetch** (high lane): `load_pdf_as_folder` 直後の Ctrl+↑↓ 想定。`render_page(page=0, password=None)` で page 数 + WebP サムネ + OS cache を温める。
- **dedup**: 同 path はどちらか 1 件だけ pending。
- **upgrade**: low に MetaOnly が queue 中で、後から同 path に NeighborPrefetch 要求が来たら **high の空きを確認してから** low → high へ移動 (Codex round 4 P2 で「high 容量確認の順序」を厳守)。
- **drop semantics**: 各 lane が満杯のときだけ新規要求を drop。lane 間は独立 (= low 満杯で high が影響を受けない、逆も同じ)。Enter で必ず populate されるので機能不整合なし。

### UI スレッド側の制約

`App::spawn_neighbor_pdf_prefetch_tasks` は `catalog_cache.get(&parent)` で **warm hit のみ** 拾う。`get_or_open_catalog` を使うと cold `CatalogDb::open` (30-150ms × ±1 隣接 = 最大 300ms) が UI で走り得るので使わない (Codex round 2 P3)。検索結果/FavSearch で別フォルダの neighbor は skip (= 通常経路で populate される、機能不整合なし)。

### password_required の決定マトリクス (`poll_pdf_enumerate` 完了時)

| 状況 | API | 理由 |
|---|---|---|
| saved pw あり (pending_save 消化後) | `set_pdf_meta(pw_req=true)` | 確信あり |
| password=None で成功 | `set_pdf_meta(pw_req=false)` | 確信あり |
| password=Some だが saved 無し (session 限定) | `set_pdf_meta_thumb` | unknown → 新規行は作らない、既存は列保持 |

### Enter フローでの placeholder

`try_apply_pdf_meta_cache` は **`pdf_passwords.get(this_pdf).is_some()`** (= ファイル固有保存 pw) でだけ password-protected キャッシュを trust する。session-level `pdf_current_password` は当てにしない (round 1 P1)。

### 親一覧のページ数遅延取得

パスワード付き PDF の親一覧では、保存済みのファイル固有パスワードがある場合だけ
ページ数を取得・表示する。`pdf_current_password` だけの未保存パスワードは使用せず、
ページ数は `-` のままとする。これは placeholder と同じ非露出境界である。

遅延取得結果には、パスワードやそのハッシュではなく、保存済み認証情報の
プロセス内更新世代を紐付ける。パスワードの保存または削除後は旧世代の「確認済み」
結果を再利用せず、同じ起動中で対象 PDF を再取得する。これにより、保存前の認証失敗が
再起動まで `-` として残ることを防ぐ。

---

## 以下は round 0 の素案 (アーカイブ目的で残す)

## ゴール

「キビキビ動く」アプリ感を実現するため、PDF の Enter→ページ一覧表示を **2 回目以降ほぼ瞬時** (~20ms)
にする。

## 背景の認識

- 現状の Enter→ページ一覧 824ms (cold) は PDFium の `LoadDocument` + 構造解析。
- 「ページ数だけ取る」高速 API は PDFium に無い。`LoadDocument` 経由が必須 → 自前パーサ書かない限り
  この処理は避けられない。
- 連打で速く感じるのは **OS ディスクキャッシュ + Critical 予約による 9μs dispatch + warm PDFium open 5-30ms** の合わせ技。
  ただし「初回」「OS cache evicted 後」は cold で 824ms。
- 「**ページ数だけ** を SQLite に永続キャッシュ」しておけば、PDFium に頼らず instant に N セルの placeholder grid に遷移できる。

## 設計

### 1. ストレージ

既存の `catalog DB` (フォルダ毎、`{cache_dir}/{xx}/{sha256}.db`) に新テーブル追加:

```sql
CREATE TABLE IF NOT EXISTS pdf_meta (
    filename   TEXT    NOT NULL PRIMARY KEY,  -- 親フォルダ内の PDF ファイル名
    mtime      INTEGER NOT NULL,
    file_size  INTEGER NOT NULL,
    page_count INTEGER NOT NULL
);
```

- `filename` は parent-relative (例: "20230103_023.pdf"), catalog DB 内では unique
- `mtime + file_size` のセットで stale 検出 (どちらかが変わったら cache miss)
- 親フォルダの catalog DB に同居するので、PDF を含むフォルダの DB 構築・破棄の生命管理に自然に乗る
- 新規テーブル `CREATE TABLE IF NOT EXISTS` のみ。**マイグレーション不要**

### 2. CatalogDb API 追加

`src/catalog.rs` に:

```rust
impl CatalogDb {
    /// 指定 filename の PDF ページ数を返す (cache hit時)。
    /// mtime/file_size が一致しない場合は None (= cache miss)。
    pub fn get_pdf_page_count(&self, filename: &str, mtime: i64, file_size: i64) -> Option<u32>;

    /// PDF ページ数を保存 (INSERT OR REPLACE)。
    pub fn set_pdf_page_count(&self, filename: &str, mtime: i64, file_size: i64, page_count: u32);
}
```

`init_schema` で `CREATE TABLE IF NOT EXISTS pdf_meta(...)` を追加するだけ。

### 3. UI 側の読み書き経路 (C-base)

#### `src/app.rs` `load_pdf_as_folder` の改造

```rust
pub fn load_pdf_as_folder(&mut self, pdf_path: PathBuf) {
    // ── 既存処理 (cancel 旧 worker, drop 旧 pending 等) ──
    self.cancel_token.store(true, Ordering::Relaxed);
    self.wake_all_workers();
    self.pdf_enumerate_pending = None;
    self.zip_enumerate_pending = None;

    let password = ...;

    // ★ NEW: PDF メタキャッシュ lookup (~1ms 同期 I/O)
    let cache_hit = self.try_pdf_meta_cache(&pdf_path);

    // ★ NEW: cache hit なら即時 placeholder grid 構築
    if let Some((page_count, mtime, file_size)) = cache_hit {
        let placeholder_items: Vec<GridItem> = (0..page_count)
            .map(|i| GridItem::PdfPage {
                pdf_path: pdf_path.clone(),
                page_num: i,
                content_type: None,
            })
            .collect();
        let placeholder_metas: Vec<Option<(i64, i64)>> =
            (0..page_count).map(|_| Some((mtime, file_size as i64))).collect();
        let existing_keys: HashSet<String> = (0..page_count)
            .map(|i| pdf_page_cache_key(i))
            .collect();
        self.start_loading_items(
            pdf_path.clone(),
            placeholder_items,
            placeholder_metas,
            existing_keys,
            Vec::new(),
            None,
        );
        // 後で enumerate 完了時に count 検証するために覚えておく
        self.pdf_placeholder_count = Some(page_count);
    }

    // 既存: 非同期 enumerate (cache hit でも整合性検証のため必ず kick)
    let handle = crate::pdf_loader::enumerate_pages_async(&pdf_path, password.as_deref());
    self.pdf_enumerate_pending = Some((pdf_path.clone(), password, handle));

    self.address = pdf_path.to_string_lossy().to_string();
    self.update_global_search_address();
}

fn try_pdf_meta_cache(&self, pdf_path: &Path) -> Option<(u32, i64, u64)> {
    let filename = pdf_path.file_name()?.to_str()?;
    let parent = pdf_path.parent()?;
    let meta = std::fs::metadata(pdf_path).ok()?;  // 同期だが ~1ms
    let mtime = meta.modified().ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)?;
    let file_size = meta.len();

    let catalog = self.open_catalog_for_folder(parent).ok()?;
    let page_count = catalog.get_pdf_page_count(filename, mtime, file_size as i64)?;
    Some((page_count, mtime, file_size))
}
```

#### `poll_pdf_enumerate` の改造

```rust
match result {
    Ok(pages) => {
        let actual_count = pages.len() as u32;

        // ★ NEW: cache hit していたら count 一致を確認
        if let Some(placeholder) = self.pdf_placeholder_count.take() {
            if placeholder == actual_count {
                // 完全一致: grid 再構築不要、cache 更新だけ済ませて return
                self.write_pdf_meta_cache(&pdf_path, pages.first());
                self.pdf_current_password = password;
                return;
            }
            // 不一致 (rare): cache が古い → grid 再構築 + cache 更新
            crate::logger::log(format!(
                "  pdf meta cache mismatch: cached={} actual={} for {}, rebuilding",
                placeholder, actual_count, pdf_path.display()
            ));
        }

        // 既存処理: items 構築 + start_loading_items
        // ...
        self.write_pdf_meta_cache(&pdf_path, pages.first());
    }
    Err(e) => {
        // 既存処理
    }
}
```

### 4. サムネ render から cache 投入 (C-thumb)

#### IPC プロトコル拡張

現行 `MSG_RENDER` レスポンス:
```
[status 1B] [w 4B] [h 4B] [type_tag 1B] [raster_w 4B] [raster_h 4B] [rgba_pixels]
```

新レスポンス (v1.0.0 で同時更新、core/worker は同 binary なので互換性問題なし):
```
[status 1B] [w 4B] [h 4B] [type_tag 1B] [raster_w 4B] [raster_h 4B] [page_count 4B] [mtime 8B] [file_size 8B] [rgba_pixels]
```

- `page_count`: PDFium `doc.pages().len()` の結果
- `mtime`, `file_size`: worker side で `std::fs::metadata` で取得 (caller の stat 不要)

#### Rust 側 API 変更

```rust
pub struct RenderResult {
    pub image: image::DynamicImage,
    pub content_type: PdfPageContentType,
    pub page_count: u32,
    pub mtime: i64,
    pub file_size: u64,
}

pub fn render_page(...) -> std::io::Result<RenderResult>
```

影響を受ける callers (5 箇所):
- `src/thumb_loader.rs:1114`: PDF サムネ render (Normal) — **ここで cache 投入**
- `src/thumb_loader.rs:1630`: 別経路の PDF render
- `src/app.rs:13108`: fullscreen render (Critical)
- `src/app.rs:16988`: cache creator batch
- `src/app.rs:17015`: 同上

すべて `let result = render_page(...)?;` の後 `result.image`, `result.content_type` を使うだけの修正。
PDF context (= path.extension() == "pdf") の caller は `result.page_count` も cache に書き込む。

#### thumb_loader の追記

```rust
let img_result = if let Some(page_num) = pdf_page {
    crate::pdf_loader::render_page(...)
        .map(|res| {
            // ★ NEW: PDF page count を catalog DB に書き込む (best-effort)
            if let Some(parent) = path.parent() {
                if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                    let _ = write_pdf_meta_to_catalog(
                        parent, filename, res.mtime, res.file_size as i64, res.page_count
                    );
                }
            }
            res.image
        })
        .map_err(|e| image::ImageError::IoError(e))
};
```

(catalog DB open はキャッシュ可能ならキャッシュ、なければ毎回 open ~5ms)

### 5. 新規 App フィールド

```rust
pub(crate) struct App {
    // ...
    /// cache hit で placeholder grid を立てた場合、その page_count を覚えておく。
    /// `poll_pdf_enumerate` が結果を受け取ったとき、count が一致すれば grid 再構築をスキップし、
    /// 不一致なら警告ログ + 再構築する。
    pub(crate) pdf_placeholder_count: Option<u32>,
}
```

`load_pdf_as_folder` で `pdf_enumerate_pending = None;` するときに同時に `pdf_placeholder_count = None;` でリセット。

### 6. エッジケース

| ケース | 動作 |
|---|---|
| 初回オープン (cache miss) | 現状と同じ (824ms バッジ → grid) + cache 書き込み |
| 2 回目オープン (cache hit, mtime 一致) | 即 placeholder grid + 裏で検証 enumerate |
| PDF 外部更新後 (mtime 変化) | cache miss (mtime 不一致) → 通常経路 + cache 更新 |
| パスワード付き PDF (パスワード保存済み) | enumerate 成功 → cache 投入 → 次回 instant |
| パスワード付き PDF (未入力) | enumerate 失敗 → cache 投入されない → 次回もパスワード要求 |
| 壊れた PDF | enumerate 失敗 → cache 投入されない |
| サムネ表示済み PDF | C-thumb で cache 既に投入済み → Enter で instant |
| ZIP 内 PDF | parent_path に file_name が取れないので cache 無視 (cache miss) |
| PDF が削除済み (stale cache) | std::fs::metadata 失敗 → cache lookup 自体スキップ |
| 同じ PDF を connection 同時 open | SQLite WAL モードなので read/write 並列 OK |
| cache の placeholder 数が実数より多い | enumerate 完了時に再構築 (rare) |
| cache の placeholder 数が実数より少ない | 同上 |

### 7. テスト

- `src/catalog.rs` のユニットテスト:
  - `get_pdf_page_count` の mtime/file_size mismatch で None を返すこと
  - `set_pdf_page_count` の INSERT OR REPLACE 動作
- 統合: 既存の PDFium ベースの統合テストは不要 (PDFium 呼び出しが flaky)

### 8. 工数見積もり

| 作業 | 行数 |
|---|---|
| `src/catalog.rs` schema + API + tests | 80-100 |
| `src/pdf_loader.rs` IPC プロトコル拡張 + RenderResult struct | 80-100 |
| `src/app.rs` `try_pdf_meta_cache`, `load_pdf_as_folder` 改造, `pdf_placeholder_count` フィールド | 100-150 |
| `src/thumb_loader.rs` C-thumb cache 投入 | 30-50 |
| 5 箇所 callers の `(img, ct, page_count, mtime, file_size)` への対応 | 30 |
| 既存テストの更新 (render_page 戻り値型変更) | 20 |
| ドキュメント (virtual-folders.md / async-architecture.md) | 30 |
| **合計** | **約 370-480 行** |

### 9. リスクと回避

- **start_loading_items の placeholder と実 enumerate 結果のずれ**:
  `pdf_placeholder_count` で覚えておいて enumerate 完了時に比較。
  ずれた場合は再 `start_loading_items` で正規化する。
- **catalog DB open のレイテンシ**:
  catalog DB の cold open は 150ms 超えることあり (CLAUDE.md 注意点)。
  ただし parent folder の DB は既に grid 表示中に開かれているはず (= 親フォルダのサムネ表示で使われた経路)。
  万一冷えていれば bg thread で開いて lookup する。
  - 暫定: 同期 open で 5-20ms (warm) と想定、worst case でも 150ms。これでも 824ms より速い。
- **path normalization 不一致**:
  catalog DB は `path_key::normalize` ベース。`filename` は単純な `file_name()` で取得。
  小文字/大文字差異は filename レベルでは Windows なら問題なし (NTFS は大小区別しないため、
  ext mtime/file_size 一致で済む)。
- **cache が古いまま間違った page_count を表示**:
  enumerate verify が裏で走るので、cache 不一致は数百 ms 以内に検出される。
  rare ケース (= 同セッション中に PDF が書き換わった等) でしか発生しない。

### 10. v1.0.0 リリースへの影響

- **新規テーブル**: マイグレーション不要 (CREATE IF NOT EXISTS のみ)
- **IPC プロトコル変更**: core/worker は同一 binary なので互換性問題なし
- **catalog DB 既存ユーザーへの影響**: 旧 DB に pdf_meta テーブル無し → 初回起動時に追加される
  だけ。既存サムネは無事
- **テスト範囲**: catalog/pdf_loader/thumb_loader/app の改修。既存テストとの干渉なし

### 11. 完了基準

- [ ] cargo build --release 通る
- [ ] cargo test --lib pdf_loader, catalog, app の関連テスト pass
- [ ] 実機: d:\home\scan\comic\20230103_023.pdf を Enter → 初回 824ms バッジ
- [ ] 実機: 同 PDF を BS → 再 Enter → **20ms 以下** で grid 切替 (バッジ見えない)
- [ ] 実機: 別フォルダのサムネ表示済み PDF を Enter → 初回でも instant
- [ ] perf-log で `pool_dispatch wait_ms`, enumerate→sli の落差確認
- [ ] cargo fmt --check クリーン

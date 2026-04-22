# 検索システム拡充 設計ドキュメント (draft)

> ステータス: **プロトタイプ計測 PASS・本体実装着手**
> 最終更新: 2026-04-21
> プロトタイプ結果: [docs/search-bench-results.md](search-bench-results.md)
> 次の TODO: §16 実装順序 step 2 (FavoriteEntry に UUID + 3 フラグ追加) から順次実装

---

## 1. 目的とスコープ

現在の検索機能を以下のように拡張する:

| ショートカット | 現状 | 新設計 |
| --- | --- | --- |
| **Ctrl+S** | フォルダ名検索 (ZIP/PDF 仮想フォルダ含む)。お気に入り全体 | 変更なし (ただしインデックス生成は統一パイプラインに統合) |
| **Ctrl+F** | メタデータ検索。現在ディレクトリのみ。ZIP の PNG tEXt は lazy 読み | **ローカルメタデータ検索**。**現在グリッドに表示中の一覧のみ** (非再帰、現行互換)。ZIP 仮想フォルダを開いて中を見ているならその中身が対象。PDF は対象外 |
| **Ctrl+G** | (新設) | **グローバルメタデータ検索**。お気に入り全体を **再帰的に** 検索。ZIP 仮想フォルダ内画像を含む。PDF は対象外 |

> **Ctrl+F のスコープについて (Codex 指摘 #5)**: 要望の原文は「現在の単一ディレクトリ」。v1 では既存挙動を保つため **現在表示中の一覧のみ** (再帰しない) に寄せる。
> ZIP を開いて展開中なら ZIP 中の画像一覧、通常フォルダを開いていればそのフォルダ直下の一覧が対象。再帰検索は Ctrl+G に寄せる。

加えて、以下を実現する:

1. **お気に入り単位の自動インデックス管理** — お気に入り登録時・編集時に、そのお気に入りに対して「フォルダ名検索」「メタデータ検索」「サムネイルキャッシュ」を個別に ON/OFF できる
2. **バックグラウンド自動メンテナンス** — アプリ起動中、対象お気に入りのインデックスが常に最新になるよう自動更新する。新規追加・更新・削除を効率的に検出
3. **「インデックス作成」「キャッシュ作成」ダイアログの統合** — 単独の一括処理ではなく、自動パイプラインの手動トリガとして再配置
4. **日本語に強い全文検索アルゴリズム** — LIKE では大規模データで劣化するため、n-gram またはトークナイザベースのインデックスを採用

---

## 2. 現状整理 (実装マップ)

ここで挙げる行番号は 2026-04-21 時点のもの。実装前に再確認すること。

### 2.1 Ctrl+S (現行お気に入り検索)

- UI: [src/ui_main.rs:643](../src/ui_main.rs) `render_favsearch_bar`
- 状態: [src/app.rs:87-96, 616-642](../src/app.rs) `FavSearchState` / `FavSearchPending`
- 実行/ポール: [src/app.rs:2055-2113](../src/app.rs) `execute_favsearch` / `poll_favsearch`
- **インデックス DB**: [src/search_index_db.rs](../src/search_index_db.rs) — `%APPDATA%/mimageviewer/search_index.db` 単一 SQLite
  - スキーマ: `entries(path PK, display_path, name, display_name, kind, favorite_root, mtime, updated_at)`
  - `kind`: `0=Folder, 1=ZipFile, 2=PdfFile`
  - 検索: `LIKE` + `ESCAPE '\\'`, 5000 件上限
- クエリ構文: [src/search_query.rs](../src/search_query.rs) — 空白 AND、`-word` NOT、`"..."` phrase
- インデックス生成: [src/ui_dialogs/index_creator.rs](../src/ui_dialogs/index_creator.rs) で手動起動。お気に入りごとに全走査 → `upsert_children` で差分更新

### 2.2 Ctrl+F (現行メタデータ検索)

- UI: [src/ui_main.rs:550](../src/ui_main.rs) `render_search_bar`
- 実行/ポール: [src/app.rs:5123-5201](../src/app.rs) `execute_search` / `poll_search`
- **永続インデックスなし** — 現在ディレクトリの画像を毎回オンデマンドに読み取る 2-pass 方式
  - Pass1 (cheap): ファイル名・ZIP/PDF 名マッチ
  - Pass2 (I/O): `decide_partial` で必要判定 → 必要なら PNG tEXt / EXIF / XMP を read
- XMP キャッシュ: `App::xmp_cache: HashMap<String, Option<XmpTweetInfo>>`
- ZIP 内画像: PNG tEXt のみ lazy 読み。EXIF/XMP は未対応
- PDF: ページ番号のみ (テキスト抽出なし)

### 2.3 キャッシュクリエイター

- UI: [src/ui_dialogs/cache_creator.rs](../src/ui_dialogs/cache_creator.rs)
- サムネイルカタログ DB: [src/catalog.rs](../src/catalog.rs) — `%APPDATA%/catalog/<xx>/<sha256>.db` (フォルダ単位)
- お気に入り毎にチェック → 別スレッドで全画像を WebP エンコードして保存
- **独立ダイアログ**。インデックス生成とは別で、重複 UI・重複走査が発生している

### 2.4 お気に入り

- 定義: [src/settings.rs:17-60](../src/settings.rs) `FavoriteEntry { name, path }`
- 永続化: `%APPDATA%/mimageviewer/settings.json`
- 編集 UI: [src/ui_dialogs/favorites_editor.rs](../src/ui_dialogs/favorites_editor.rs) — 名前編集・順序入替・削除のみ。**自動処理トグルなし**

### 2.5 メタデータリーダー

| モジュール | 対象 | 主なフィールド |
| --- | --- | --- |
| [src/png_metadata.rs](../src/png_metadata.rs) | PNG tEXt/iTXt/zTXt | A1111 `parameters`, ComfyUI `prompt`/`workflow`, Midjourney `Description`, 任意 (key,value) |
| [src/exif_reader.rs](../src/exif_reader.rs) | JPEG/TIFF EXIF (rexif) | カメラ、レンズ、撮影日時、F値、ISO、焦点距離、GPS など |
| [src/xmp_reader.rs](../src/xmp_reader.rs) | JPEG APP1 / PNG iTXt / MP4 uuid / TIFF IFD0 | `xtw:*` (X/Twitter), `dc:description`, `dc:creator`, 投稿日時 ほか |

### 2.6 ZIP/PDF 扱い

- **ZIP**: [src/zip_loader.rs](../src/zip_loader.rs) — `enumerate_image_entries()` でフラット展開 (入れ子 ZIP も `chapters/ch01.zip/page01.jpg` 形式)。`read_entry_bytes()` で単一エントリだけデコードできる
- **PDF**: [src/pdf_loader.rs](../src/pdf_loader.rs) — 別プロセスワーカープール。ページレンダリングはできるが、テキスト抽出 API は現状未使用

---

## 3. 全体アーキテクチャ (新設計)

```
                        ┌────────────────────────────────────┐
                        │  Favorites (settings.json)         │
                        │  + 各お気に入りに index フラグ 3 種  │
                        └──────────────┬─────────────────────┘
                                       │ 起動時 / 編集時
                                       ▼
       ┌───────────────────────────────────────────────────────┐
       │           Indexer Supervisor (新規)                   │
       │  - 起動時: 前回 DB 状態 vs お気に入り現状 を diff       │
       │  - アプリ実行中: 自動メンテナンスループ                │
       │  - 手動トリガ: 「インデックス再構築」メニュー          │
       └─────┬─────────────────┬──────────────────┬────────────┘
             │                 │                  │
  ┌──────────▼──────┐ ┌────────▼────────┐ ┌───────▼────────┐
  │ FS Watcher      │ │ Walker Worker   │ │ Ingest Worker  │
  │ (notify-rs)     │ │ (rayon)         │ │ (1 スレッド)    │
  │ - 差分イベント  │ │ - 定期全走査    │ │ - メタ抽出     │
  │ - デバウンス    │ │ - mtime チェック│ │ - DB 書き込み  │
  └───────┬─────────┘ └────────┬────────┘ └───────┬────────┘
          │                    │                  │
          └────────────┬───────┴──────────────────┘
                       │ パス & 変更種別のキュー (crossbeam)
                       ▼
          ┌──────────────────────────────────────┐
          │  Index Stores                         │
          │  - search_index.db  (Ctrl+S 構造, 既存) │
          │  - fts_index/       (Tantivy bigram)  │
          │  - fts_meta.db      (メタ + 正規化全文) │
          │  - catalog/*.db     (サムネ, 既存)     │
          └────────────┬─────────────────────────┘
                       │
                       ▼
        ┌──────────────────────────────────────────┐
        │ Search Executors                          │
        │  - Ctrl+S: search_index.db                │
        │  - Ctrl+F: fts_meta.db direct lookup      │
        │  - Ctrl+G: fts_index + fts_meta (streaming)│
        └──────────────────────────────────────────┘
```

レイヤー分離の原則:

- **UI スレッド**: 入力イベントとクエリ発行のみ。I/O ゼロ (docs/ui-responsiveness.md §4 遵守)
- **検索実行**: 既存の `XxxPending { cancel, rx }` パターンを踏襲
- **インデクサ**: 完全に独立した長寿命スレッド群。UI と共有するのは crossbeam チャネル + `Arc<AtomicBool>` のみ

---

## 4. 検索エンジン方式の選択

### 4.1 検討した選択肢

| 案 | 日本語対応 | バイナリ増分 | インデックスサイズ (目安) | クエリ性能 | 実装コスト | ライセンス |
| --- | --- | --- | --- | --- | --- | --- |
| **A. 現行 LIKE (据え置き)** | △ (case-sensitive 問題は既に解決済み) | 0 | 小 | **大規模で線形劣化** | 最小 | - |
| **B. SQLite FTS5 + unicode61** | × (CJK を単語区切りできない) | 0 (bundled 済み) | 小 | 中 | 小 | PD |
| **C. SQLite FTS5 + trigram** | △ (標準 trigram は CJK に弱い。3文字未満ヒットしない) | 0 | 中 (トークン重複で肥大) | 中 | 小〜中 | PD |
| **D. SQLite FTS5 + better-trigram (CJK-aware)** | ○ (CJK は 1 文字 1 トークン) | +数十 KB (C 拡張) | 中 | 中 | 中 (C 拡張のビルド/リンク) | MIT (streetwriters/sqlite-better-trigram) |
| **E. Tantivy + NgramTokenizer(2,2)** | ◎ (bigram は CJK と相性良い) | +約 10〜15 MB (lib のみ) | 中〜大 | **良好** (Lucene 系) | 中 | MIT |
| **F. Tantivy + Lindera (IPADIC)** | ◎◎ (形態素解析で意味単位) | **+約 25 MB** (Lindera+辞書) | 中 | **良好** | 中〜大 | MIT |

### 4.2 推奨: **案 E (Tantivy + Ngram bigram)** を本命、**案 D (FTS5 better-trigram)** をフォールバック候補

**推奨理由:**

1. **日本語検索漏れの少なさ** — 形態素解析 (案 F) は辞書にない語彙 (新語・固有名詞・AI プロンプトの英語混在) を分割ミスする。bigram n-gram は「2 文字以上の部分一致ならほぼ必ず拾える」特性があり、画像メタデータや AI プロンプトのような未知語だらけのデータに向いている
2. **「1 文字検索は許容しない」仕様とちょうど一致** — bigram は仕様上 2 文字以上でのみ索引が効く。1 文字クエリは「対象外」として明示的に弾ける
3. **実装負荷** — Tantivy 単体 (Lindera 辞書なし) ならバイナリ増は約 10 MB 程度。Lindera IPADIC (13 MB 辞書) を追加すると合計 25 MB 超。現在の exe が約 150 MB (AI モデル埋め込み) なので誤差の範囲ではあるが、不要な肥大は避ける
4. **クエリ性能** — Tantivy は Lucene 系のフルインバートインデックスで、お気に入り全体 (想定 10〜数十万ファイル規模) でも数 ms 〜 数十 ms で返る。SQLite FTS5 trigram は書き込み性能が劣る ([SQLite forum: FTS5 with trigram slow on insert](https://sqlite.org/forum/info/3e9352773af9e7d5f6de80532affaadd2429eef107f3f811bdbb8f38cd953dc3))

**案 F (Lindera) を採用しない理由:**

- IPADIC (2007 年ベース) の辞書更新が止まっており、AI 関連の固有名詞 (`stable-diffusion`, `xl`, `lora` 等) を正しく分割できない
- 画像メタ検索の主対象は EXIF (英数記号) と AI prompt (英単語 + 記号) が大半で、日本語形態素解析の恩恵が限定的
- 辞書バイナリ 13 MB の追加はインストーラサイズに影響する

**案 D (better-trigram) を完全排除しない理由:**

- Tantivy 導入がライセンスやビルドの都合で難しいと判明した場合の保険。SQLite は既に bundled なので C 拡張を 1 つ足すだけで済む
- ただし trigram は bigram より「2 文字検索時の偽陽性」が増えやすく、CJK では情報量が少ない (漢字 3 文字連続は意味単位を跨ぐ) ため、bigram (案 E) の方が検索精度が高い

### 4.3 n-gram パラメータの詳細

- **min_gram=2, max_gram=2 (純 bigram)** を採用
  - Tantivy の `NgramTokenizer::new(2, 2, false)` で実装 ([NgramTokenizer docs](https://docs.rs/tantivy/latest/tantivy/tokenizer/struct.NgramTokenizer.html))
  - **大文字小文字**: `lower_caser` を噛ませる。日本語は影響なし、英数は case-insensitive

#### 最小クエリ長ポリシー (Codex 指摘 #10)

ASCII 2 文字は `sd` `xl` `ai` `on` `to` のような汎用 bigram が爆発的にヒットする恐れがある。
クエリ言語ごとに最小長を変える:

| クエリ種別 | 最小文字数 | 例 |
| --- | --- | --- |
| 日本語 (CJK を 1 文字でも含む) | **2 文字** | `街並` OK / `街` NG |
| 英数字のみ | **3 文字** | `sdx` OK / `sd` NG |
| 記号混じり (`"..."` phrase 内を除く) | 2 文字 | `#1`, `@u` など |

- 判定は [src/search_query.rs](../src/search_query.rs) のトークン分類器を拡張して行う
- 例外の救済: `field:value` 構文 (§15.1.5) が入れば、`tag:sd` のように短くてもフィールド指定で OK にする

#### phrase / NOT / AND の正確な評価は post-filter (Codex 指摘 #1)

Tantivy の `NgramTokenizer` は token position を常に 0 で吐く仕様
([NgramTokenizer docs](https://docs.rs/tantivy/latest/tantivy/tokenizer/struct.NgramTokenizer.html))。
そのため **Tantivy 側だけでは phrase (`"..."`) を正しく判定できない**。
さらに bigram 索引上の AND は「bigram 断片が両方現れる doc」で、元クエリの連続部分一致とは異なるため偽陽性が出る。

- **Tantivy は『候補絞り込み』のみ**: 全 bigram の AND でヒット候補を集める
- **正確な AND / phrase / NOT の最終判定は post-filter** で行う: 候補 doc の保存済み正規化全文 (§5.2) に対して
  既存の [search_query](../src/search_query.rs) `matches(tokens, text)` と等価な関数で再評価
- phrase は post-filter 側で `text.contains(quoted_string)` を直接チェック
- NOT (`-word`) も post-filter で除外判定 (Tantivy 側に NOT 句を入れても position=0 で誤判定するため不適)
- 上記のため、**候補 → post-filter の flow は必須**。候補数が多い場合はページング取得
  (`TopDocs::with_limit(500).and_offset(offset)` を 500 件ずつ、有効結果 HARD_MAX=10,000 で打ち切り)
  で streaming に post-filter する (§9.1, §10.4)

> 参考: [Tantivy: Deleting and Updating documents](https://tantivy-search.github.io/examples/deleting_updating_documents.html) — 更新は delete+reinsert。バッチコミット設計の前提。

### 4.4 ライセンス確認

- Tantivy: MIT
- lindera-tantivy (保険): MIT
- sqlite-better-trigram (保険): MIT

mimageviewer は MIT なので、いずれも組み込み可能。再配布時は LICENSE 表記追加 (現行の NOTICE.md / README に追記)。

---

## 5. スキーマ設計

### 5.1 新規 DB 構成

`%APPDATA%/mimageviewer/` 配下:

| ファイル | 目的 | 技術 |
| --- | --- | --- |
| `search_index.db` (既存) | Ctrl+S 用のフォルダ/ZIP/PDF 構造インデックス。**スキーマ拡張で継続利用** | SQLite |
| `fts_index/` (新規) | Tantivy インデックスディレクトリ (複数ファイル) | Tantivy |
| `fts_meta.db` (新規) | Tantivy 本体では持ちにくいメタ情報 (ファイル mtime, size, last_indexed, content_hash) | SQLite |

Tantivy は独自のファイル形式で、複数 segment ファイル + meta.json を吐く。
**削除・更新は段階的 commit で段差が発生するが、検索結果に古い item は含まれても UI 側の `mtime` 再確認でフィルタできる。**

### 5.2 Tantivy スキーマ (fts_index)

```rust
// pseudo — v1 スキーマ
schema_builder.add_text_field("path",        STRING | STORED); // 完全一致キー (正規化済み)
schema_builder.add_text_field("container",   STRING | STORED); // "fs" / "zip"
schema_builder.add_text_field("zip_entry",   STRING | STORED); // ZIP 内相対パス (container=zip のみ)
schema_builder.add_text_field("favorite_id", STRING | STORED); // favorite root の安定 ID (後述)
schema_builder.add_i64_field ("mtime",       INDEXED | STORED);
schema_builder.add_i64_field ("file_size",   STORED);

// === 全文検索対象フィールド (bigram tokenizer + lower_caser) ===
schema_builder.add_text_field("name",        TEXT);  // ファイル名 (拡張子含む)
schema_builder.add_text_field("all_text",    TEXT);  // name + 全メタを連結した汎用検索フィールド

// 注: post-filter 用の正規化済み原文は Tantivy には持たず、fts_meta.db.files.all_text_norm が
// 唯一の保存先 (§5.3)。Tantivy の segment 肥大と compaction 負荷を避けるため。
```

#### フィールド設計の根拠 (Codex 指摘 #2, #3 反映 + 2 回目指摘 #2 反映)

- **post-filter 用原文の保存先は `fts_meta.db.files.all_text_norm` に一元化** (Codex 2 回目指摘 #2):
  - Tantivy schema には **保存しない** (以前のドラフトの `STORED` 指定は削除済み)
  - post-filter 時に Tantivy から得た `path` (STORED) をキーに、SQLite の prepared `SELECT all_text_norm FROM files WHERE path IN (?,?,...)` で一括取得
  - Tantivy segment の肥大と compaction 負荷を避け、インデックスサイズを抑制
  - ページング取得時の 500 件バッチに対して SQLite lookup は 5〜10ms 程度
- **正規化方針は `to_lowercase()` のみ** (Codex 2 回目指摘 #3):
  - 現行 [src/search_query.rs](../src/search_query.rs) `to_lowercase()` に合わせる
  - NFKC は採用しない (Rust 標準にないため `unicode-normalization` crate 追加が必要。v1 では簡潔さを優先)
  - インデックス側 (ingest 時の `all_text_norm` 生成) と クエリ側 (`search_query::parse`) と post-filter 側で **同じ関数** を使う:
    ```rust
    // src/search_norm.rs (新規) - 3 箇所で参照される唯一の正規化関数
    pub fn normalize_for_match(s: &str) -> String { s.to_lowercase() }
    ```
  - NFKC は v2 で検討 (全角英数 / 半角カナ等の正規化)。導入時は `index_version` を bump して再インデックス必須
- **`favorite_id` の付与方式** (Codex 2 回目指摘 #4 修正):
  - SHA-1 短縮は衝突リスクがあるため採用しない
  - **方針**: [src/settings.rs](../src/settings.rs) の `FavoriteEntry` に `id: Uuid`(v4) フィールドを追加し、新規登録時に払い出す
    ```rust
    pub struct FavoriteEntry {
        pub id: Uuid,                   // 新規、serde は #[serde(default = "Uuid::new_v4")]
        pub name: String,
        pub path: PathBuf,
        pub auto_index_structure: bool,
        pub auto_index_metadata: bool,
        pub auto_index_thumbs: bool,
    }
    ```
  - 既存お気に入りには起動時マイグレーションで UUID を発行し、`settings.json` を書き戻す (§11.2)
  - `favorite_id` (Tantivy / fts_meta.db) の型は `STRING` でこの UUID を保持。path 変更 (お気に入り rename) にも追従できる
  - 副次効果: path 文字列を直接入れないので DB サイズ削減 (UUID 36 文字固定) と変更耐性が得られる
- **`favorite_id` は indexed STRING field**: FAST field ではなく indexed な `STRING` (exact term) で十分 (Codex 初回 #3)
- **`name` と `all_text` を分ける理由**: ファイル名マッチをスコアブーストに使うため
- **`exif_text` / `xmp_text` / `png_params` を個別フィールドに分けるか** — v1 では `all_text` に寄せる単一フィールド方式。
  将来 `field:value` クエリを入れるときに個別フィールド化する

#### ドキュメント粒度

- 通常ファイル: 1 画像 = 1 doc
- ZIP 画像: 1 エントリ = 1 doc (`container="zip"`, `zip_entry` に相対パス)
  - Tantivy 側の `path` は `<zippath>!<entry>` 形式で正規化
- PDF: v1 ではインデックス対象外 (§6 参照)。v1.x 以降で `container="pdf"` として別扱い

### 5.3 fts_meta.db

```sql
CREATE TABLE files (
    path TEXT PRIMARY KEY,           -- normalize_path 済み (ZIP 内エントリは "zippath!entry")
    favorite_id TEXT NOT NULL,       -- Tantivy 側と同じ stable UUID (FavoriteEntry.id)
    favorite_root TEXT NOT NULL,     -- 表示・集計用の原文パス
    mtime INTEGER NOT NULL,
    file_size INTEGER NOT NULL,
    indexed_at INTEGER NOT NULL,
    index_version INTEGER NOT NULL,  -- スキーマ変更時の再インデックス用
    index_generation INTEGER NOT NULL,-- ingest 世代 (§5.6 整合性用)
    status INTEGER NOT NULL,         -- 0=ok, 1=pending, 2=failed, 3=tombstone (delete 保留)
    all_text_norm TEXT NOT NULL DEFAULT '' -- post-filter 用 正規化済み原文 (Codex 指摘 #2)
);
CREATE INDEX idx_files_fav       ON files(favorite_id);
CREATE INDEX idx_files_fav_mtime ON files(favorite_id, mtime);
CREATE INDEX idx_files_status    ON files(status) WHERE status != 0;  -- 進行中のみスキャン
```

- **なぜ Tantivy に直接持たせないか**: Tantivy でファイル単位の "変更検出" を効率的に問い合わせるのは困難。
  差分検出は SQLite のインデックス付きクエリで回し、確定した差分のみ Tantivy にコミットする
- `index_version` を定数で持ち、定数を上げれば次回起動時に全再インデックス
- `all_text_norm` は post-filter で必要なテキスト。バッチコミット単位で UPSERT するので WAL モードを有効化

### 5.4 search_index.db スキーマ拡張 (Ctrl+S 用)

既存の `entries` テーブルに列追加:

```sql
ALTER TABLE entries ADD COLUMN indexed_by_auto INTEGER NOT NULL DEFAULT 0;
-- 0: 手動/従来エントリ (既存データはこれ), 1: 自動インデクサが書き込んだ
```

お気に入り単位の ON/OFF を自動インデクサ側で判断するため、DB 自体のスキーマ変更は最小限で済む。

### 5.5 FavoriteEntry の拡張

```rust
pub struct FavoriteEntry {
    pub id: Uuid,                   // 新規: 安定 ID (Codex 2 回目指摘 #4)
    pub name: String,
    pub path: PathBuf,
    // 新規: 自動インデックス管理
    pub auto_index_structure: bool, // Ctrl+S 用 (フォルダ名 + ZIP/PDF 名)
    pub auto_index_metadata: bool,  // Ctrl+F/G 用 (全文メタデータ)
    pub auto_index_thumbs: bool,    // サムネイル事前キャッシュ
}
```

- **`id: Uuid` の役割**: Tantivy / fts_meta.db の `favorite_id` はこの UUID を保持。SHA-1 短縮の衝突リスクを回避 (Codex 2 回目指摘 #4)
- **UUID が保持できるのは「表示名 rename」まで** (Codex 3 回目指摘 #4):
  - **表示名 (`name`) 変更**: UUID 不変 → インデックス保持、何もしない
  - **root パス (`path`) 変更 (お気に入りを別ディレクトリに付け替え)**: 物理的に全 doc の `path` フィールドが無効化される
    - 旧 path で登録されていた doc は削除 (差分検出で FS にないとして tombstone 化)
    - 新 path 配下を再スキャン (起動時差分走査 §7.4 と同じ経路)
    - UI 上は「インデックスを再構築しています」の進捗表示
    - **一括 path 更新 (新旧 path の prefix 置換で UPDATE) は v1 ではやらない**:
      パス正規化の大小・区切り文字・ZIP エントリとの整合で事故りやすいため、素直に tombstone→再スキャンする
  - この挙動はお気に入り編集ダイアログで **path を変更しようとした際に確認ダイアログ** を出す (誤操作防止)
- **マイグレーション方針**:
  - 既存お気に入りには起動時に UUID v4 を発行して `settings.json` を書き戻す (1 回限り)
  - 3 フラグは `false` 初期値。初回起動時の救済フローで「過去に手動でインデックス作成済みだったお気に入り」を `auto_index_structure=true` に寄せる (§11.2)

#### 既存の serde 手書き実装への組み込み (Codex 指摘 #4)

[src/settings.rs](../src/settings.rs) の `FavoriteEntry` は **`Deserialize` / `Serialize` が手書き** で、
内部で `Raw::Legacy(String)` (旧形式) と `Raw::Full { name, path, ... }` を分岐している。
**単に `#[serde(default)]` をフィールドに足すだけでは効かない** — `Raw::Full` 側にもオプション化を反映する必要がある。

```rust
#[derive(Deserialize)]
#[serde(untagged)]
enum Raw {
    Legacy(String),
    Full {
        #[serde(default = "Uuid::nil")]   // 追加: 後段で nil を検出して UUID v4 発行
        id: Uuid,
        name: String,
        path: PathBuf,
        #[serde(default)]
        auto_index_structure: bool,
        #[serde(default)]
        auto_index_metadata: bool,
        #[serde(default)]
        auto_index_thumbs: bool,
    },
}

// Raw → FavoriteEntry 変換時
impl From<Raw> for FavoriteEntry {
    fn from(raw: Raw) -> Self {
        match raw {
            Raw::Legacy(path_str) => FavoriteEntry {
                id: Uuid::new_v4(),  // 新規発行
                name: derive_name_from_path(&path_str),
                path: PathBuf::from(path_str),
                auto_index_structure: false,
                auto_index_metadata: false,
                auto_index_thumbs: false,
            },
            Raw::Full { id, name, path, auto_index_structure, auto_index_metadata, auto_index_thumbs } => {
                let id = if id.is_nil() { Uuid::new_v4() } else { id };  // 既存無 UUID は発行
                FavoriteEntry { id, name, path, auto_index_structure, auto_index_metadata, auto_index_thumbs }
            }
        }
    }
}
```

- `Legacy` 分岐と `Full` で `id` 欠落だった場合は **UUID を新規発行** し、settings 保存時に書き戻す (永続化は 1 回限り)
- UUID 発行後 → `settings.save()` を呼んで json を書き戻す。書き戻しはロード後の遅延 1 回
- Serialize 側は全フィールド素直に出力 (JSON は数 KB なので実害なし)
- 必要なクレート: `uuid = { version = "1", features = ["v4", "serde"] }`

### 5.6 SQLite / Tantivy 二段整合性 (Codex 指摘 #7)

`fts_meta.db` と Tantivy インデックスは別ストレージなので、片方だけ更新されて片方が失敗するクラッシュケースがあり得る。
以下の二段階コミット + 起動時 reconciliation で整合性を担保する。

#### 5.6.1 Ingest の書き込み順序 (Upsert)

```
(1) fts_meta.db に UPSERT: status=pending, index_generation += 1, all_text_norm を書き込む
        │  (SQLite 単独トランザクション、これが失敗したら何もコミットしない)
        ▼
(2) Tantivy に delete(path) + add_document を push (バッファに貯める)
        │
        ▼
(3) バッチ境界で Tantivy IndexWriter::commit()
        │  (commit が成功するまでは old segment が見える = 検索から見えるのは古い doc)
        ▼
(4) fts_meta.db UPDATE: status=ok, indexed_at=now
        │  (commit 成功が前提なので、ここで失敗しても pending のまま残す = 次回起動時に再 ingest)
```

#### 5.6.2 Delete の書き込み順序 (Tombstone)

```
(1) fts_meta.db に UPDATE: status=tombstone
        ▼
(2) Tantivy に delete(path) を push
        ▼
(3) バッチ境界で commit()
        ▼
(4) fts_meta.db から物理 DELETE
```

Tombstone 中は **検索結果の post-filter で除外** する (path が tombstone なら捨てる)。
Tantivy の delete 伝搬遅延で古い doc がヒットしても UI には出ない。

#### 5.6.3 起動時 reconciliation

```
A. fts_meta.db から status != ok の行を全部取る
   - pending: Tantivy には入っていない可能性あり → 再 ingest キューへ
   - tombstone: Tantivy には残っている可能性あり → 削除キューへ再投入
   - failed: ログに出した上で再 ingest キューへ (最大 3 回、超えたら諦めてスキップ)

B. Tantivy segment の `path` と fts_meta.db を突き合わせて orphan を検出
   - Tantivy には居るが fts_meta.db に無い doc → Tantivy から削除
   - fts_meta.db には居るが Tantivy に無い doc → 再 ingest
   (※ Tantivy 側の全 path 列挙は segment 全走査なので高コスト。週 1 回程度のメンテ時だけ実行)
```

#### 5.6.4 `index_generation` の役割

- ingest / delete のたびに単調増加
- post-filter で「検索で拾った doc の `index_generation` が古すぎる」場合に信用しない、といった将来拡張の余地
- v1 では単なる世代カウンタとして記録するのみ

#### 5.6.5 `index_version` との違い

| 項目 | `index_version` | `index_generation` |
| --- | --- | --- |
| 単位 | 全 DB グローバル | ファイル単位 |
| 変わるきっかけ | スキーマ変更 (開発者が定数を bump) | ingest / delete ごと |
| 不整合時の挙動 | 全 DB を捨てて再構築 | 古い doc を捨てて再 ingest |

---

## 6. PDF を全文検索対象にするか

### 6.1 技術的難易度

PDFium はテキスト抽出 API (`FPDFText_GetText` / `FPDFText_GetBoundedText`) を提供しており、既に別プロセスワーカーでロードしているので実装の入口自体はある。難易度の本質は次の点:

1. **処理時間**: テキスト抽出はページ単位で数十〜数百 ms。1000 ページの PDF で数十秒
2. **データ量**: OCR 付き PDF (マンガのルビ・背景テキスト等) は 1 ページあたり数 KB〜数十 KB。数百冊の PDF なら数百 MB の全文データ
3. **正確性**: OCR 結果は誤認識が多い ("の" → "ゐ" など)。bigram 索引に混ぜると偽陽性が増える
4. **ワーカープロセス経由**: 既存 `mimageviewer-pdf-worker` プロトコルに "extract_text(page)" コマンドを追加する必要がある

### 6.2 結論 (提案)

**PDF 全文検索は v1 では対象外**。ただし以下の妥協案を検討:

- **v1**: PDF はファイル名 + **PDFium の document info** (`Title` / `Author` / `Subject` / `Keywords`) のみをインデックス対象にする。これなら 1 PDF あたり 1 回の軽量読み込みで済む
  - (旧ドラフトでは「DPAPI で保存しているタイトル」と誤記していたが、DPAPI は PDF パスワード保存用のもので無関係 — Codex 指摘 #9)
- **v1.x (将来)**: ユーザ設定で「PDF 全文も対象にする」チェックボックスを用意。ON の時だけ PDF テキスト抽出ワーカーを起動。索引は別コレクション (`pdf_fts_index/`) に分離し、ノイズデータを本体索引から分離

**理由**: PDF テキストの多くが OCR ノイズで、bigram 索引に入れると漫画タイトル名でも関係ない PDF がヒットする可能性がある。オプションとして残すが、デフォルト OFF で段階的に導入したい。

---

## 7. バックグラウンド・インデックス・メンテナンス

### 7.1 イベントソース

- **起動時 sync**: 前回記録した `fts_meta.db` vs 現在のファイルシステム状態を比較 → 差分を処理
- **実行中**: [notify-rs](https://github.com/notify-rs/notify) でお気に入りルートを再帰的に watch。Windows では `ReadDirectoryChangesW` を内部使用
- **手動トリガ**: 既存ダイアログを「インデックス再構築」に再配置し、選択したお気に入りを強制再スキャン

### 7.2 notify-rs の注意点 (Windows)

- `RecursiveMode::Recursive` で watch すると OS 側の `FILE_NOTIFY_CHANGE_*` が連続して来るため、**デバウンス必須** (notify::event::Event が短時間に数百件来ることあり)
- ネットワーク共有 (SMB, NAS) では `ReadDirectoryChangesW` が発火しないケースがある → **定期的なポーリング走査をフォールバック** として併走させる
- watch ハンドル数は OS 上限があるため、お気に入り root 単位でまとめて watch (ルート直下のサブディレクトリを個別 watch しない)
- `File.tmp` → `File.ext` のリネーム (ダウンロード完了パターン) は `Create` + `Rename` の 2 イベントで届くのでデバウンスで 1 件に集約

### 7.3 差分検出フロー (detail)

```
notify Event → debouncer (500ms) → イベントキュー (crossbeam)
                                          │
                                          ▼
              ┌───────────────────────────────────┐
              │ Diff Applier (1 スレッド)         │
              │                                   │
              │  path があるお気に入り scope 内か? │
              │    No  → 無視                     │
              │    Yes → kind 判定:               │
              │      Create → Ingest にエンキュー │
              │      Modify → mtime/size 比較:    │
              │        変化なし → 無視            │
              │        変化あり → Ingest 再実行   │
              │      Remove → DB から削除         │
              │      Rename → 削除 + 追加         │
              └───────────────────────────────────┘
```

### 7.4 起動時差分走査

1. **各お気に入りのルートを walk** (rayon 並列、`entry.file_type()` で `GetFileAttributes` 回避 — CLAUDE.md §UI スレッド同期 I/O 参照)
2. **`fts_meta.db` から登録済みファイルリストを取得** (favorite_root でフィルタ)
3. 3-way diff:
   - FS にあって DB になし → Ingest キュー
   - DB にあって FS になし → 削除キュー
   - 両方にあって mtime/size が違う → 再 Ingest キュー
4. ZIP ファイル自体の mtime 変化は **ZIP 全エントリ再インデックス** とする (ZIP 内ファイルの mtime は個別取得しない方針 — ZIP 内は読むまで分からない)

### 7.5 Ingest Worker (Codex 指摘 #8 反映)

**元案の「シングルスレッド固定」は SSD で遅すぎるため、速度設定で可変にする。**

- **Tantivy IndexWriter は 1 本**: Tantivy は内部でマルチスレッドインデクシングを行う設計。
  writer を複数持つと segment の整合性管理が複雑になるので、writer は 1 本に固定
- **メタ抽出ワーカーは N 本**: 環境設定の速度プロファイルに連動

| プロファイル | メタ抽出ワーカー数 | レート制限 | 想定環境 |
| --- | --- | --- | --- |
| Low | 1 | 1 ファイル/秒 | HDD / NAS / バッテリー |
| Medium (デフォルト) | 2 | 制限なし | HDD + SSD 混成 |
| High | 4 | 制限なし | NVMe SSD |

- **グローバル I/O セマフォ** (Codex 指摘 #8, §15.1.6):
  既存の PDF ワーカー (3 プロセス) / サムネイルワーカー / 本インデクサが同時に走ると HDD シーク競合で
  UI のスクロールがつまる。[docs/async-architecture.md](async-architecture.md) の優先度キュー設計に合わせ、
  **全ワーカー横断で上限同時 I/O 数を制限する `GlobalIoSemaphore` を導入** する
  - `tokio::sync::Semaphore` 相当を自前 `Mutex + Condvar` で実装 (CLAUDE.md §並行処理 に従い try_lock+sleep は禁止)
  - 優先度は (UI 表示中ページ) > (PDF ワーカー) > (サムネイル) > (インデクサ)
  - インデクサが持つ permit は 1〜2 に制限 (High プロファイルでもフォアグラウンドを阻害しない)
- **優先度キュー**: ユーザが今見ているフォルダ内のファイルは priority=high、他は low
  - 実装テンプレートは [src/pdf_loader.rs](../src/pdf_loader.rs) の `PdfWorkerPool` / `JobQueue` / `run_dispatcher`
- **バッチコミット**: Tantivy の `IndexWriter::commit()` は高コスト (fsync を伴う)。100 件または 5 秒ごとにまとめてコミット
  - **commit はバッチ境界と一致させる**: §5.6.1 の整合性プロトコルで fts_meta.db の status 更新も同じバッチ境界で行う
- **キャンセル**: アプリ終了時 `Arc<AtomicBool>` で即中断 + 未コミット分は破棄 (次回起動時に差分検出で拾う)

### 7.6 負荷設定

環境設定に以下を追加:

- 「自動インデックスの速度」: Low / Medium / High (§7.5 の表参照)
- 「AC 電源時のみインデックス」: ノート PC 用のバッテリー保護
- 「サムネイルキャッシュ領域上限」: 既存設定を流用

### 7.7 ZIP インデックス負荷の抑制 (Codex 指摘 #6)

ZIP 内画像をインデックス対象にする判断は検索機能としては重要だが、現行 [zip_loader.rs](../src/zip_loader.rs)
のネスト ZIP バイト列キャッシュは大量 ZIP を横断すると RAM / I/O を食いやすい。
インデクサ専用の制約を設ける:

- **外側 ZIP を 1 回だけ開く**: 1 ZIP 内の全エントリを連続して ingest する (ZIP を開き直さない)
  - インデクサ用に [zip_loader::enumerate_image_entries](../src/zip_loader.rs) を拡張し、
    `ZipArchive` を借りたまま entry を舐める専用 API を追加
- **ネスト ZIP のキャッシュは無効化 or 厳しく制限**: 既存のバイト列キャッシュを ingest スレッドから使うと
  巨大 ZIP で数百 MB のメモリを食う。インデクサ用 context では
  - キャッシュは 1 レベル (外側 ZIP のみ) に制限
  - ネスト内側 ZIP は "無制限 fs 読み" ではなく、1 つ処理→即 drop
- **巨大 ZIP の後回し (tiered)**:
  - 通常サイズ (< 100 MB): 通常優先度
  - 中規模 (100 MB 〜 1 GB): low priority、他作業の合間
  - 巨大 (> 1 GB): **ユーザ操作がアイドルのときだけ処理** (アイドル判定は入力イベントなしが 30 秒続いた状態)
- **ZIP 自体の mtime 変化 = ZIP 内全エントリ再インデックス**: §7.4 で決めた方針通り
  - ただし再インデックス中は古い doc を Tantivy から消さない (中断されても検索から完全に消えない)
  - `fts_meta.db` 側で `status=pending` にしておき、commit 完了時に古い path を tombstone → delete
- **読み取り失敗時の handling**: 破損 ZIP・パスワード付き ZIP は `status=failed` で記録し、次回以降の再試行を 24 時間後まで抑止

---

## 8. UI 変更

### 8.1 お気に入りエディタ ([src/ui_dialogs/favorites_editor.rs](../src/ui_dialogs/favorites_editor.rs))

既存の 3 列 (名前 / パス / 操作) を拡張:

```
┌─────────────────────────────────────────────────────────────────────────┐
│ 名前           パス           ✓構造  ✓メタ  ✓サムネ   ↑↓🗑            │
├─────────────────────────────────────────────────────────────────────────┤
│ お気に入り A   D:\photos\     ☑     ☑      ☑        ↑↓🗑            │
│ お気に入り B   E:\manga\      ☑     ☐      ☑        ↑↓🗑            │
│ ... インデックスサイズ: 142 MB (構造 2MB + メタ 128MB + サムネ 12MB)    │
│ [一括ON] [一括OFF]   [今すぐ再構築]                                    │
└─────────────────────────────────────────────────────────────────────────┘
```

- 3 つのチェックボックスを行ごとに配置。ツールチップで各機能説明
- フッターに「このお気に入りの現在のインデックスサイズ」表示
- 「今すぐ再構築」: 既存 `index_creator` / `cache_creator` の役割。選択中お気に入りを強制再スキャン

### 8.2 お気に入り追加ダイアログ ([src/ui_dialogs/fav_add.rs](../src/ui_dialogs/fav_add.rs))

追加時にチェックボックス 3 つを表示。デフォルト値は環境設定の「新規お気に入りのデフォルト」から取る。

### 8.3 Ctrl+F / Ctrl+G の検索 UI

- Ctrl+F (ローカル): 既存 UI と同じ。インデックス済みメタがあれば `fts_meta.db` 直接検索 (§9.2)、未登録分は現行の lazy 方式にフォールバック
- Ctrl+G (グローバル): 新規トップパネル。Ctrl+S パネルの位置をシェアする (排他表示)
- **インデックス未完了時の表示**: `〔インデックス 68% 作成中 – 部分的な結果〕` のような進捗バッジ

### 8.4 既存ダイアログの統廃合

| 旧 | 新 |
| --- | --- |
| 一括キャッシュ作成 (`cache_creator`) | 「インデックス管理」ダイアログに統合 |
| インデックス作成 (`index_creator`) | 同上 |
| (新設) インデックス管理 | お気に入り単位で 3 ジョブの状態・進捗・再構築を管理 |

統合後ダイアログイメージ:

```
┌──────────────────── インデックス管理 ─────────────────────┐
│ 🚀 起動時整合性チェック: 12 ms  (pending 整理 0 / tombstone 0 / I/O 並列度 2)│
│                                                                         │
│ 表示名 | パス  | 状態 | スキャン      | 取込      | 削除 | 操作      │
│ A      | ...   | ✅   | 890ms / 3214件| 123       | 0    | [🔄再構築]│
│ B      | ...   | ⏳   | —             | 0         | 0    | [🔄再構築]│
│                                                                         │
│ [🔄 すべて再構築]  [閉じる]                                             │
└──────────────────────────────────────────────────────────┘
```

v0.8.0 実装済み項目:

- 上部「起動時整合性チェック」バナーで reconciliation 所要時間 + pending_cleaned /
  tombstone_purged / io_permits を表示 (インデックス本体の健全性指標)
- 行単位の「スキャン」列: 直近フル再走査の (ms / 走査ファイル数)。ツールチップに
  初期スキャン時間 + 診断 (read_dir / metadata / 深さ上限) を折りたたみ表示
- walk/ingest 失敗が 1 件でもある行は橙色でハイライト

---

## 9. クエリ実行フロー

### 9.1 Ctrl+G (グローバルメタ検索)

1. ユーザ入力 → 300ms デバウンス (既存 `execute_search` 同様)
2. **クエリ最小長チェック** (§4.3 のポリシー):
   - 1 文字 → 「2 文字以上入力してください」表示で早期 return
   - 英数字のみ 2 文字 → 「英数字は 3 文字以上入力してください」表示で早期 return
3. クエリパーサ ([src/search_query.rs](../src/search_query.rs)) で AST に変換:
   - 既存: AND / NOT / phrase
   - v1 では構文追加なし (`field:value` は §15.1.5 で検討)
   - **NOT-only クエリの扱い** (Codex 3 回目指摘 #3 + 4 回目指摘 #3):
     - **Ctrl+G (グローバル): 禁止**。正のトークン (AND 項 / phrase) が 1 つもなく NOT (`-word`) だけのクエリは、
       Tantivy 側で候補を絞り込めず全件 scan になる。UI で
       `〔含める語を 1 つ以上入力してください (除外だけの検索は不可)〕` を表示して早期 return
     - **Ctrl+F (ローカル): 許可**。SQLite 直接方式で対象が表示中 item のみなので、NOT-only でも現実的な負荷で処理可能 (§9.2)
4. **Tantivy Searcher snapshot を固定** (Codex 3 回目指摘 #2):
   - ワーカー開始時に `IndexReader::searcher()` で 1 つの `Searcher` を取得し、
     以降のページング取得はすべて **同じ Searcher 上で行う**
   - 検索中にインデクサが commit / reader reload しても、この Searcher は古い snapshot を見続ける
     → offset pagination でのドキュメント重複・抜けを防止
   - Searcher は段階的 GC 対象のため、検索完了 (または cancel) 時に明示的に drop する
   - ユーザへの影響: 検索中に新しくインデックスされたファイルは、その検索結果には現れない。
     次回クエリで反映されるため実用上問題なし
5. **候補絞り込み (Tantivy) — ページング取得** (Codex 2 回目指摘 #1):
   - `favorite_id` 絞り込み (`auto_index_metadata=true` のお気に入り全部)
   - 正のトークンを bigram 分解し、`all_text` に対して AND の `BooleanQuery`
   - NOT 句 (`-word`) は **ここでは Tantivy に渡さない** (position=0 で誤判定するため、§4.3 参照)
   - **固定 top-N で切らない**: bigram は偽陽性が多いため、真の一致が top-N の外に落ちる可能性がある (検索漏れ)。
     **上記で固定した Searcher 上で** `TopDocs::with_limit(page_size).and_offset(offset)` を呼び出し、
     ページ単位で繰り返しフェッチ する
     - page_size = 500、offset を 500 ずつ進める
     - 停止条件: (a) post-filter 後の有効結果数が **HARD_MAX** に到達, (b) Tantivy が候補を使い切る, (c) キャンセル
     - HARD_MAX: 最終結果として 10,000 件 (UI 表示 & 性能の妥協点)。超えた場合は UI に「結果が多すぎます。絞り込みキーワードを追加してください」を表示
5. **post-filter 最終判定** (§4.3, §5.2) — ページ単位 streaming:
   - 各ページの候補 path から `fts_meta.db` の `all_text_norm` を一括 SELECT (prepared IN clause)
   - 既存 `search_query::matches(tokens, text)` 相当の関数で phrase / NOT / AND を正確に評価
   - tombstone (status=3) のエントリはここで除外
   - **通過した結果は即座に UI へ streaming 送信** (§10.5 streaming 設計参照)
6. 停止時の UI 表示:
   - (a) HARD_MAX 到達: `〔結果が多すぎます — 10,000 件で打ち切り。キーワードを追加してください〕`
   - (b) 候補使い切り (完全結果): `〔N 件〕` のみ
   - (c) インデックス進行中: `〔現在 N 件 — インデックス 68% 作成中、部分結果〕`

### 9.2 Ctrl+F (ローカルメタ検索) — Tantivy を経由しない (Codex 3 回目指摘 #1)

**方針変更**: ローカル検索は **Tantivy を使わず、`fts_meta.db` 直接ルックアップ方式** に切り替える。
表示中 item 数が小さい (通常数十〜数千) ので、Tantivy の bigram 候補絞り込みは不要で、
むしろ「グローバル候補を走査してから表示中 path 集合で絞る」方式だと無駄な走査が発生する。

- **スコープ**: 現在グリッドに表示中の一覧のみ (非再帰、Codex 初回指摘 #5)
- **クエリフロー (SQLite 直接方式)**:
  1. 現在表示中の `App::items` から画像/ZIP image 系の **path 集合** を抽出 (`Vec<PathBuf>`, 順序保持)
     - **重要 (Codex 4 回目指摘 #4)**: この path は `fts_meta.db` に登録されたのと同じ正規化形式
       (lowercase + `/` 区切り + ZIP 内エントリは `<zippath>!<entry>` 形式) に **必ず揃える**。
       GridItem の生データ path をそのまま IN 句に入れると大文字小文字や区切り文字で lookup が空振りする。
       §5.3 の `normalize_path()` と同じ関数を使う
  2. `fts_meta.db` から一括 SELECT: `SELECT path, all_text_norm FROM files WHERE path IN (?,?,...)`
  3. 取得した各 (path, text) に対して `search_query::matches(ast, text)` を直接評価
  4. 合格した path を `search_filter: HashSet<usize>` に反映 (既存 filter 方式踏襲)
- **メリット**:
  - **検索漏れゼロ**: bigram 候補絞り込みを通さないので、post-filter 正確性がそのまま結果になる
  - **シンプル**: Tantivy の Query 組み立てや pagination が不要
  - **速い**: 表示中 path 数 × 1〜2 ms = 数十 ms で完了。streaming 不要
- **インデックス未作成 path のフォールバック**:
  - 表示中 path の中に `fts_meta.db` に未登録のもの (=お気に入り `auto_index_metadata=false`、
    または indexing 進行中でまだ未処理) があれば、それらだけ **現行のオンデマンド検索** (PNG tEXt / EXIF / XMP を都度読む) で処理
  - つまり「SQLite ルックアップ済み結果 + 未登録分のオンデマンド結果」をマージして filter を作る
  - 未登録分は worker 実行が必要なので mpsc 経由で後追い反映 (既存 `SearchPending` 踏襲)
- **ZIP 展開中**: ZIP 内エントリの path 表現 (`<zippath>!<entry>`) で同じく `fts_meta.db` を引く。
  §7.7 の ZIP ingest 対象外 ZIP は全件オンデマンド fallback
- **クエリエンジンは Ctrl+G と共通ではない**: パーサと `matches()` は共通だが、実行経路は別 (Tantivy を経由しない)

### 9.3 Ctrl+S (構造検索)

変更なし。内部的にインデックス生成が自動メンテナンスに統一される。クエリ実行ロジックは現行のまま。

---

## 10. 各ショートカットの UI 挙動 (詳細仕様)

### 10.1 Ctrl+S — お気に入り全体フォルダ/アーカイブ名検索

**既存挙動を維持** (変更なし)。メイン画面上部に検索バー表示、結果はフラットなリストで
「該当フォルダ / ZIP / PDF」を一覧化。クリックでそのフォルダに遷移 (既存動作)。

### 10.2 Ctrl+F — ローカルメタ検索 (現在表示中の一覧のみ)

現行 Ctrl+F と同じ UX。

- **入力バー**: メイン画面上部に検索バー表示 (既存の `render_search_bar`)
- **結果表示**: **現在のグリッドに対する filter** (既存の `search_filter: HashSet<usize>` 方式)
  - 一致しなかった item は非表示になるのではなく、**グレーアウト or 非表示** (既存挙動踏襲)
  - グリッド構造・階層位置は変わらない
- **閉じる**: Escape でクリア、Ctrl+F 再押しで検索バー閉じる
- **ZIP 展開中の扱い**: ZIP を開いている状態でも同じ挙動 (ZIP 内エントリに対する filter)

### 10.3 Ctrl+G — グローバルメタ検索 (drill-down 階層ビュー) **← 新規**

Ctrl+F と違い、**結果を新しい「検索結果ビュー」に切り替える**。
Ctrl+S の「お気に入り横断フォルダ検索」の挙動を **メタデータにまで拡張** したもの。

#### UX フロー

```
[1. 検索入力]
  ユーザ Ctrl+G → トップパネルに検索バー (Ctrl+S と排他)
  入力 → デバウンス後にクエリ実行

[2. トップレベル結果表示]  ← Ctrl+S と同じ見た目
  ┌─────────────────────────────────────────────────┐
  │ 🔍 "夕焼け"  [x]  45 件ヒット (8 フォルダ, 3 ZIP) │
  ├─────────────────────────────────────────────────┤
  │ 📁 D:\photos\2025\sunset\        12 枚ヒット    │
  │ 📁 D:\photos\2024\landscape\      8 枚ヒット    │
  │ 📦 E:\archives\yakei.zip          15 枚ヒット    │
  │ 📁 D:\ai_generated\sd_out\        7 枚ヒット    │
  │ ...                                             │
  └─────────────────────────────────────────────────┘
  - ヒットしたファイルを含むフォルダ・ZIP を集約表示
  - 各行に「何枚ヒットしたか」を表示
  - ソート: ヒット件数降順 (同数なら名前)
  - クリック/Enter で [3. 絞り込みビュー] へ遷移

[3. 絞り込みビュー (drill-down)]  ← 階層保持した filter
  ユーザが "D:\photos\2025\sunset\" をクリック
  →  そのフォルダに入るが、**検索状態は保持**。
     通常の一覧ではなく「その階層で該当ファイルだけ」の view
  ┌─────────────────────────────────────────────────┐
  │ ← [戻る]  "夕焼け" で絞り込み中                 │
  │ パス: D:\photos\2025\sunset\                    │
  ├─────────────────────────────────────────────────┤
  │ 📁 ../ (Ctrl+G 結果ビューに戻る)                │
  │ 📁 subfolder_a\    3 枚ヒット                   │  ← ここにも該当あり
  │ 🖼 IMG_2341.jpg                                 │
  │ 🖼 IMG_2342.jpg                                 │
  │ 🖼 sunset_beach.jpg                             │
  └─────────────────────────────────────────────────┘
  - その階層で「該当ファイル」+「該当ファイルを含む子フォルダ」だけを表示
  - 子フォルダには件数バッジ
  - 画像クリックでフルスクリーンは通常通り
  - 子フォルダに入ると、さらにその階層で絞り込み (再帰)
  - ZIP に入ったら ZIP 内 path を filter scope にする (同じロジック)

[4. 検索解除]
  - Escape または ✕ ボタンで通常ビュー (= Ctrl+G 押下前の場所) に戻る
  - 「戻る」ボタンでは [2. トップレベル結果] に戻る (階層は保持しない)
```

#### ナビゲーションの状態モデル

既存の `FavSearchState::nav_stack` (Ctrl+S 用) を流用・拡張する:

```rust
struct GlobalSearchState {
    query: String,
    query_ast: QueryAst,                    // パース済み
    results_by_container: Vec<ContainerHit>, // Tantivy + post-filter 後の集約結果
    view: GlobalSearchView,
}

enum GlobalSearchView {
    /// [2] トップレベル: フォルダ/ZIP 集約リスト
    Aggregated,
    /// [3] drill-down 中: 特定フォルダ/ZIP 配下の絞り込み表示
    DrilledInto {
        container: ContainerRef,  // 入っているフォルダ or ZIP
        zip_sub_path: Option<String>, // ZIP 内部で更に潜っている場合
    },
}

struct ContainerHit {
    path: PathBuf,      // フォルダ or ZIP のパス
    kind: ContainerKind, // Folder / Zip
    hit_count: usize,
    hit_paths: Vec<PathBuf>, // drill-down 時の一覧に使う
}
```

- 現在 Ctrl+S で使っている「検索バーを閉じずに階層を降りる」UX と同じ操作感にする
  ([src/app.rs:616-642](../src/app.rs) `FavSearchState`)
- drill-down 中は GridItem を **動的に組み立てる**:
  - ヒット画像ファイル (その階層直下のもの)
  - ヒットを含む子フォルダ (バッジ付き、クリックで更に潜る)
  - その階層に関係ない非ヒット item は **非表示**
- 「戻る」の階層: drill-down 中に ←キー or BackSpace でレベル上げ、Escape で結果解除

#### 集約ロジック (Ctrl+G 結果の折りたたみ)

Tantivy + post-filter から返る path リストを UI で集約:

1. 各 path の「直上のコンテナ」を決定:
   - 通常ファイル: 親フォルダのパス
   - ZIP 内エントリ: ZIP ファイルのパス (ZIP 内サブパスは `zip_sub_path` で保持)
2. コンテナごとにグループ化、`hit_count` を合計
3. ヒット件数降順でソート (同数なら名前昇順)

**トップレベル集約の粒度** (議論の余地あり, §15.1.7 新論点):

- 案 A (v1 採用): 「画像ファイルの直上のフォルダ」単位で集約
  - 例: `D:\photos\2025\sunset\IMG.jpg` → `D:\photos\2025\sunset\` で集約
  - メリット: ドリルダウン UI がシンプル、想定どおりの挙動
- 案 B: 「お気に入り root + 第 2 階層」単位など、もっと浅いレベルで集約
  - 結果リストが短くなるが階層情報が荒くなる

v1 は案 A。ユーザが使ってみて結果が冗長すぎるなら案 B を検討。

#### ZIP 内での drill-down

ZIP はフォルダ類似だが「実際のディレクトリツリー」ではない。ただし現行の
[src/zip_loader.rs](../src/zip_loader.rs) は nested ZIP をフラットパスで扱えている
(`chapters/ch01.zip/page01.jpg`) ので、`/` で split して擬似的な階層を再現する。

- ZIP を drill-down 中に更にサブパスに潜る操作も可能
- Escape で ZIP から出る (= [2] トップレベル結果)

#### インデックス未完了時の表示

- トップバー右側に進捗: `〔インデックス 68% — 部分的な結果〕`
- 集約件数は「現時点までにインデックス済みの範囲で」の値
- インデックス進行に応じて追加結果を **インクリメンタル反映** (毎秒再クエリ)

#### 他 UI との共存

- Ctrl+G 結果ビュー中は Ctrl+S が押されたら Ctrl+G を閉じて Ctrl+S を開く (排他)
- Ctrl+F は Ctrl+G とは別レイヤー (Ctrl+G 結果ビュー中の「現在表示一覧」に対して絞り込み) として
  **v1 では無効化** (組み合わせが複雑になりすぎるため)
- フルスクリーン表示に入っても検索状態は保持。フルスクリーン終了で同じ drill-down view に戻る

### 10.4 検索結果の段階的表示 (streaming) **← 新規設計**

**ユーザからの質問**: 検索結果を徐々に一覧に増やせるか。現在は同期的か。

**現状 (調査結果)**:

- 現在の Ctrl+S / Ctrl+F は `FavSearchPending` / `SearchPending` が
  `mpsc::Receiver<OneShot>` で **最終結果 1 回だけ** を受け取り、一括で反映
- `App::items: Vec<GridItem>` は `load_folder` で全置換されるが、「途中で増える」用途では使われていない
- ただし **技術的な阻害要因はない**:
  - 仮想スクロール (`show_viewport`) の `scroll_offset_y` はピクセル絶対値で、`items.len()` 変化に追従
  - 行スナップ計算も毎フレーム再計算なので `total_h` 変化 OK
  - `selected: Option<usize>` は append-only なら index ズレなし
- **ただし blocker が 1 つ**: `App::thumbnails: Vec<ThumbnailState>` が `items.len()` と **同期必須**。
  現在は `start_loading_items` で `items.len()` 分を `Pending` で初期化する前提

**難易度**: 現状 **Medium** (実装 1〜2 日)。blocker は `items.push` と `thumbnails.push` をセットで行う
ヘルパーを作れば解消する。

#### 10.4.1 streaming 必要性

- **Ctrl+G の post-filter** はページング取得 (§9.1) で、候補 500 件ごとに SQLite lookup + phrase 評価
  - 10,000 件 HARD_MAX まで走らせると合計 2〜5 秒かかる想定
  - streaming なしだと「結果画面が 2〜5 秒空のまま」→ UX が悪い
- **Ctrl+F は対象件数が小さい** (現在表示中一覧のみ) ので streaming 不要、v1 は batch で実装
- **Ctrl+S は既存通り batch** (Tantivy なし、SQLite LIKE で瞬時)

**結論**: streaming は **Ctrl+G のみに導入**。Ctrl+F / Ctrl+S は従来の batch で良い。

#### 10.4.2 設計 (Ctrl+G 専用)

##### チャネルプロトコル

```rust
// src/global_search.rs (新規)
pub enum SearchStreamEvent {
    /// 候補が見つかった (post-filter 通過済み)
    Batch {
        hits: Vec<GlobalHit>,        // 新規追加分のみ (既出は含まない)
        scanned_candidates: usize,   // 累計 Tantivy 候補数 (進捗表示用)
        valid_hits: usize,           // 累計 post-filter 通過数
    },
    /// インデックス側で進行中の index 進捗を反映
    IndexProgress { indexed: usize, total_estimated: usize },
    /// 検索完了 (正常終了)
    Done { truncated: bool, reason: DoneReason },
    Error(String),
}

pub enum DoneReason {
    Complete,           // 候補を使い切った
    TruncatedAtMax,     // HARD_MAX 到達 (結果不完全)
    Cancelled,
}

pub struct GlobalSearchPending {
    pub cancel: Arc<AtomicBool>,
    pub rx: mpsc::Receiver<SearchStreamEvent>,
    pub done: bool,
    pub truncated: bool,
}
```

##### ワーカー側ループ

```rust
// 別スレッドで走る
let page_size = 500;
let mut offset = 0;
let mut valid_hits = 0;
const HARD_MAX: usize = 10_000;

// ★ Searcher snapshot を固定 (Codex 3 回目指摘 #2)
// このワーカー実行中にインデクサが commit しても、この searcher は古い snapshot を見続ける
let searcher = index_reader.searcher();

loop {
    if cancel.load(Ordering::Relaxed) { 
        let _ = tx.send(SearchStreamEvent::Done { truncated: false, reason: DoneReason::Cancelled });
        break;
    }
    
    // 1. Tantivy ページング fetch (必ず同じ searcher 上で)
    let tantivy_hits = tantivy_search(&searcher, &query, offset, page_size, favorite_ids);
    if tantivy_hits.is_empty() { 
        let _ = tx.send(SearchStreamEvent::Done { truncated: false, reason: DoneReason::Complete });
        break;
    }
    
    // 2. fts_meta.db から all_text_norm を一括取得
    let texts = fetch_norm_texts(&tantivy_hits);
    
    // 3. post-filter (phrase/NOT/AND の最終判定)
    let mut batch = Vec::new();
    for (hit, text) in tantivy_hits.iter().zip(texts.iter()) {
        if search_query::matches(&ast, text) {
            batch.push(GlobalHit::from(hit));
            valid_hits += 1;
            if valid_hits >= HARD_MAX {
                break;
            }
        }
    }
    
    // 4. Batch event 送信 (空でも進捗だけ送る)
    if !batch.is_empty() || offset % 2000 == 0 {
        let _ = tx.send(SearchStreamEvent::Batch {
            hits: batch,
            scanned_candidates: offset + tantivy_hits.len(),
            valid_hits,
        });
    }
    
    if valid_hits >= HARD_MAX {
        let _ = tx.send(SearchStreamEvent::Done { truncated: true, reason: DoneReason::TruncatedAtMax });
        break;
    }
    offset += page_size;
}
```

##### UI 側 (App::update 内)

毎フレーム冒頭で pending があれば `try_recv` をループで消費:

```rust
fn poll_global_search(&mut self, ctx: &egui::Context) {
    let Some(pending) = self.global_search_pending.as_mut() else { return };
    if pending.done { return; }
    
    // 1 フレームで処理するイベント上限 (UI 応答性のため)
    const MAX_EVENTS_PER_FRAME: usize = 8;
    let mut events_processed = 0;
    
    while events_processed < MAX_EVENTS_PER_FRAME {
        match pending.rx.try_recv() {
            Ok(SearchStreamEvent::Batch { hits, valid_hits, .. }) => {
                self.global_search_state.merge_hits(hits);
                self.global_search_state.total_valid = valid_hits;
                self.rebuild_global_search_items();  // items + thumbnails を同期拡張
                events_processed += 1;
            }
            Ok(SearchStreamEvent::Done { truncated, .. }) => {
                pending.done = true;
                pending.truncated = truncated;
                break;
            }
            // ...
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => { pending.done = true; break; }
        }
    }
    
    if !pending.done {
        ctx.request_repaint();  // 次フレームで続き
    }
}
```

##### items + thumbnails 同期拡張ヘルパー

```rust
/// items と thumbnails を必ずセットで push する
/// 既存コードが assume している `items.len() == thumbnails.len()` 不変条件を保つ
fn push_grid_item_pending(&mut self, item: GridItem) -> usize {
    let idx = self.items.len();
    self.items.push(item);
    self.thumbnails.push(ThumbnailState::Pending);
    idx
}

/// Ctrl+G 結果ビューで results が追加されたら items を再構築
/// - Aggregated view: ContainerHit のリストから GridItem::SearchContainer を生成
/// - DrilledInto view: 該当階層のヒットファイルだけ GridItem::Image を生成
fn rebuild_global_search_items(&mut self) {
    // 現在の selected path を覚えておいて、新 items 後に復元
    let preserved_path = self.selected.and_then(|i| self.items.get(i).map(|it| it.path()));
    
    self.items.clear();
    self.thumbnails.clear();
    
    match &self.global_search_state.view {
        GlobalSearchView::Aggregated => {
            for container in &self.global_search_state.containers_sorted_by_hits() {
                self.push_grid_item_pending(GridItem::SearchContainer {
                    path: container.path.clone(),
                    hit_count: container.hit_count,
                    kind: container.kind,
                });
            }
        }
        GlobalSearchView::DrilledInto { container, zip_sub_path } => {
            // 該当コンテナ内でこの階層に属するヒットと、子フォルダの集約を追加
            // (省略)
        }
    }
    
    // selection を path ベースで復元
    if let Some(p) = preserved_path {
        self.selected = self.items.iter().position(|it| it.path() == p);
    }
}
```

#### 10.4.3 UX 上の配慮

- **進捗バッジ**: 検索バー右側に `〔現在 N 件 (候補 M 件走査中)〕` をリアルタイム表示
- **スクロール安定化**:
  - ユーザが top に居るなら新結果は下に append → 気にならない
  - ユーザがスクロール中なら `scroll_offset_y` は触らない (`total_h` 拡大のみ, clamp は自動)
  - 結果の途中で drill-down した場合、それ以降の Batch は `DrilledInto` view 配下に merge (必要なら append)
- **キャンセル**:
  - 新しい入力 → `cancel.store(true)` + 新 worker spawn
  - 結果ビューを閉じる → 同上
  - folder 遷移 → 同上 (既存 `execute_*` と同じキャンセル 3 箇所パターン)
- **チラつき防止**:
  - Aggregated view でコンテナ順序は「hit_count 降順」。新バッチで既存コンテナの hit_count が増えても、
    表示順が頻繁に入れ替わるとチラつく → **1 秒ごとに並び順を再評価** (より頻繁には並び替えない)

#### 10.4.4 実装コスト見積もり

| タスク | 工数 |
| --- | --- |
| `SearchStreamEvent` / `GlobalSearchPending` 定義 | 0.5 日 |
| ワーカースレッド実装 (ページング + streaming 送信) | 0.5 日 |
| `push_grid_item_pending` + `GridItem::SearchContainer` 追加 | 0.5 日 |
| `rebuild_global_search_items` + Aggregated / DrilledInto の両 view | 1 日 |
| テスト (単体 + UI スナップショット) | 0.5 日 |
| **合計** | **約 3 日** |

現 §16 の実装順序に streaming 対応を組み込む (ステップ 12 を分割)。

---

## 11. マイグレーション計画

### 11.1 フェーズ分割

| フェーズ | 内容 | デフォルト動作 |
| --- | --- | --- |
| v0.8.0-alpha1 | FavoriteEntry にフラグ追加。UI チェックボックスだけ追加。バックエンドは既存のまま | 既存の `search_index.db` 手動生成が動く |
| v0.8.0-alpha2 | 自動インデクサを追加 (構造のみ)。notify-rs + fts_meta.db 実装 | フラグ ON のお気に入りのみ自動更新 |
| v0.8.0-beta | Tantivy 追加、Ctrl+G / Ctrl+F 新経路 | フラグ ON で FTS、OFF は既存動作 |
| v0.8.0-rc | PDF メタ (タイトル等) 取り込み、UI 最終調整 | - |
| v0.8.0 | 統合ダイアログ、旧 2 ダイアログ削除 | 過去インデックス作成済み favorite を `auto_index_structure=true` に寄せる |
| v0.8.1+ | PDF 全文対応を opt-in で追加 | デフォルト OFF |

### 11.2 既存ユーザのインデックス救済

- 既存の `search_index.db` はスキーマ互換で引き継ぎ。`indexed_by_auto=0` で保持
- `cache_creator` で作成済みのサムネイル (`catalog/<hash>/*.db`) も継続利用
- 初回起動時に、過去に手動キャッシュ/インデックス作成されたお気に入りを自動判別し、対応するチェックボックスを事前 ON にする提案ダイアログを出す

---

## 12. 性能見積もり

### 12.1 インデックスサイズ (プロトタイプ計測で確定、docs/search-bench-results.md)

10 万ファイル × 平均メタ 2 KB = 200 MB の元テキストを想定。
**プロトタイプ計測 (2026-04-21) で実測値が判明したので従来の見積もりから更新**。

- **Tantivy fts_index/ (bigram)**: **実測 raw テキスト × 0.07〜0.08x** (従来見積 1.5〜3x は過大)
  - 10 万ファイル: **約 15 MB**
  - 50 万ファイル: **約 70 MB**
  - bigram 特有の postings 圧縮が効き、元テキストよりずっと小さい
  - `all_text_norm` は Tantivy 側には持たないのでこのサイズには含まれない
- **fts_meta.db** (実測値付き):
  - メタ管理行 (path, mtime, size, status 等): 10 万 × 200 bytes = 約 20 MB
  - `all_text_norm` テキスト列 (post-filter 用): 10 万 × 2 KB = 約 200 MB (実測 約 397 MB — SQLite オーバーヘッドで 2x 弱)
  - **10 万件での DB 実測値: 約 397 MB / 50 万件: 約 1984 MB**
  - WAL ファイルは commit 時に一時的に数十 MB まで膨らむ
- **search_index.db (Ctrl+S 構造、既存)**: 10 万エントリ × 400 bytes = **40 MB**

**合計の典型サイズ**:
- 10 万ファイル: 約 **450 MB** (Tantivy 15MB + SQLite 397MB + 構造 40MB)
- 50 万ファイル: 約 **2100 MB** (Tantivy 70MB + SQLite 1984MB + 構造 200MB)

インストーラと独立の `%APPDATA%` 配下に生成されるので exe サイズには影響しない。
SQLite 側が支配的。ユーザ設定で「お気に入り単位のメタ索引を OFF」にすれば対象外に落とせる。

### 12.2 初期インデックス時間

画像 1 枚あたりのメタ抽出 + Tantivy 書き込み:

- 小 PNG (AI 画像): 5〜15 ms
- JPEG + EXIF: 3〜8 ms
- ZIP 内画像: 2〜5 ms + ZIP open 5〜50 ms (エントリ一括展開なら 1 回だけ)

10 万ファイルの初期インデックスは **SSD で 15〜30 分、HDD で 1〜2 時間** と見積もる。バックグラウンド処理なので問題ない範囲。

### 12.3 検索クエリ (プロトタイプ計測で実測、docs/search-bench-results.md)

**実測値 (50 万件規模)**:

| クエリタイプ | total 時間 | 備考 |
| --- | --- | --- |
| 稀な語 (一致少数) | 3〜12ms | `unique_id`, `rare_jp` |
| 典型的 AND (3 語) | 10〜15ms | `夕焼け 海辺 紅葉` |
| HARD_MAX 到達 (10,000 件) | 50〜160ms | `photo`, `カメラ` 等の超汎用 bigram |
| NOT-only (禁止) | — | UI で早期 return |

- 最初のページ (streaming 開始) は **5ms 以内** に返るため体感は良好
- offset 肥大時の penalty は first page 比 2x 以内に収まる (§15.1.9 確定)
- SQLite post-filter 500 件バッチ lookup: **9ms** (50 万件規模でも)

---

## 13. UI 応答性 ([docs/ui-responsiveness.md §4](ui-responsiveness.md)) チェックリスト確認

新規に UI スレッドから呼ぶ同期 API があるか?

- [x] `Tantivy::search()` → **UI スレッドから呼ばない**。検索ワーカースレッドで実行
- [x] `fts_meta.db` クエリ → **UI スレッドから呼ばない**。差分検出は別スレッド
- [x] `notify::Watcher::new()` → 起動時 1 回のみ呼び、以後はイベント受信のみ
- [x] `rusqlite::Connection::open` → インデクサ初回起動時のみ、別スレッドで warm up
- [x] `std::fs::read_dir` → Walker スレッド内のみ。UI スレッドからの呼び出しは現状維持

新規にチャネル/アトミックを追加するので `docs/async-architecture.md` に反映する。

---

## 14. テスト方針

### 14.1 単体テスト

- `fts_meta.db` 差分検出ロジック: FS 状態と DB 状態のモックを用意して 3-way diff が正しく分類されることを確認
- bigram クエリパーサ: 1 文字 → 空クエリ、2 文字 OK、phrase/NOT の組み合わせ
- 偽陽性フィルタ: bigram がヒットしたが原文には含まれていないケースを再現

### 14.2 統合テスト (既存 `tests/` に追加)

- `tests/search_index_fts_integration.rs`:
  - 画像 100 枚 + ZIP 1 個を用意 → インデックス作成 → 検索 → 結果一致
  - ファイル追加・削除・リネームで差分反映を確認
- `tests/favorites_migration.rs`: 既存 JSON → 新フィールド追加後も読めること
- **既存 `tests/susie_integration.rs` 同様の env var 注入パターン** で DB パスを差し替え可能に

### 14.3 性能測定

- `src/bin/bench_search.rs` を新設 (bench_thumbs.rs と同様)
  - Corpus: 10 万件のダミー画像 + メタを生成 → インデックス時間・クエリ時間を計測
- perf::event を要所に差し込み、`--perf-log` + `scripts/analyze_perf.py` で悪化検知

---

## 15. 代替案・検討事項

### 15.0 Codex レビュー反映状況

#### 1 回目 (2026-04-21, 10 件)

| # | 指摘内容 | 反映先 | 状態 |
| --- | --- | --- | --- |
| 1 | NgramTokenizer は position=0 のため phrase が効かない | §4.3 "phrase / NOT / AND の正確な評価は post-filter" | ✅ 反映 |
| 2 | post-filter 用原文の保存先が曖昧 | §5.2 `all_text_norm` の配置、§5.3 fts_meta.db に寄せる | ✅ 反映 (2 回目でさらに強化) |
| 3 | `favorite` を FAST で O(1) filter は言い過ぎ | §5.2 indexed STRING 方式に修正、`favorite_id` 導入 | ✅ 反映 |
| 4 | `FavoriteEntry` 手書き serde の `Raw::Full` 側対応 | §5.5 "既存の serde 手書き実装への組み込み" | ✅ 反映 (UUID 追加で再編) |
| 5 | Ctrl+F スコープが曖昧 | §1 表、§9.2 で「現在表示中の一覧のみ」に確定 | ✅ 反映 |
| 6 | ZIP 全体インデックスの RAM/I/O 負荷 | §7.7 "ZIP インデックス負荷の抑制" を新設 | ✅ 反映 |
| 7 | SQLite / Tantivy 二重管理の整合性設計 | §5.6 "SQLite / Tantivy 二段整合性" を新設 | ✅ 反映 |
| 8 | Ingest Worker 1 本固定は保守的すぎる | §7.5 を Low/Med/High + グローバル I/O セマフォに拡張 | ✅ 反映 |
| 9 | §6.2 "DPAPI" は誤記 | §6.2 で訂正 (PDFium document info) | ✅ 反映 |
| 10 | ASCII 2 文字検索のヒット爆発 | §4.3 "最小クエリ長ポリシー" で JP 2 / ASCII 3 | ✅ 反映 |

#### 2 回目 (2026-04-21, 5 件 + 軽微 2 件)

| # | 指摘内容 | 反映先 | 状態 |
| --- | --- | --- | --- |
| 1 | top-N=2000 切り捨ては検索漏れの原因 | §9.1 ステップ 4 をページング取得に変更、§15.1.9 方針確定 | ✅ 反映 |
| 2 | Tantivy schema `all_text_norm STORED` と SQLite 採用が矛盾 | §5.2 pseudo schema から `all_text_norm` を削除、唯一の保存先は §5.3 fts_meta.db と明記 | ✅ 反映 |
| 3 | NFKC と `to_lowercase()` が混在 | §5.2 で正規化は `to_lowercase()` のみに統一、`src/search_norm.rs` 新設で 3 箇所で唯一の関数を参照 | ✅ 反映 |
| 4 | `favorite_id` SHA-1 8 文字は衝突リスク | §5.2 / §5.5 で `FavoriteEntry.id: Uuid` 方式に変更。settings.json で永続 | ✅ 反映 |
| 5 | §12.1 `fts_meta.db = 20 MB` が古い見積もり | §12.1 で `メタ 20 MB + all_text_norm 200 MB + オーバーヘッド ≈ 265 MB` に更新 | ✅ 反映 |
| minor 1 | §3 アーキ図の DB 名が本文と不一致 (`fav_catalog.db` / `fts_text.db`) | §3 図を `search_index.db` / `fts_index/` / `fts_meta.db` に統一 | ✅ 反映 |
| minor 2 | Ctrl+F スコープは path prefix より item 集合が堅い | §9.2 実装方針を "表示中 item の path 集合で絞る" に変更 | ✅ 反映 |

#### 2 回目のユーザ質問への回答: 検索結果の段階的表示

- §10.4 "検索結果の段階的表示 (streaming)" を新設
- 現状は batch (一括反映)。streaming 対応は技術的に可能、blocker は `items`/`thumbnails` 同期のみ
- Ctrl+G は streaming で実装 (ページング post-filter と整合)、Ctrl+F / Ctrl+S は batch のまま
- 実装コスト 約 3 日

#### 3 回目 (2026-04-21, 6 件)

| # | 指摘内容 | 反映先 | 状態 |
| --- | --- | --- | --- |
| 1 | Ctrl+F は Tantivy 経由にしない方がよい | §9.2 を `fts_meta.db` 直接ルックアップ方式に全面書き換え。Tantivy 非経由で検索漏れゼロ | ✅ 反映 |
| 2 | Tantivy streaming は Searcher snapshot 固定必須 | §9.1 ステップ 4 に "Searcher snapshot 固定" を追加、§10.4.2 ワーカー擬似コードも更新 | ✅ 反映 |
| 3 | NOT-only クエリの扱いが未定義 | §9.1 ステップ 3 に NOT-only 禁止ポリシー追加 (UI で「含める語を入力してください」) | ✅ 反映 (4 回目で Ctrl+F は許可と明記) |
| 4 | UUID で root 移動も index 保持、は言い過ぎ | §5.5 で rename (表示名) vs root path 変更を区別。path 変更時は tombstone → 再スキャン。編集 UI で確認ダイアログ | ✅ 反映 |
| 5 | `GridItem::SearchContainer` 追加は全 match 監査が必要 | §16 実装順序 step 12 に "全 match arm 監査 + UI テスト" を明記 | ✅ 反映 |
| 6 | `TopDocs::and_offset` は offset 肥大時の worst case を測る必要 | §15.1.1 プロトタイプ計測に worst case 測定と代替手段 (custom Collector, continuation token) を追加 | ✅ 反映 |

#### 4 回目 (2026-04-21, 軽微 5 件)

| # | 指摘内容 | 反映先 | 状態 |
| --- | --- | --- | --- |
| 1 | §3 アーキ図の Ctrl+F ラベルが古い | 図を `Ctrl+F: fts_meta.db direct lookup` に更新 | ✅ 反映 |
| 2 | §5.3 `favorite_id` コメントが "stable hash" のまま | `stable UUID (FavoriteEntry.id)` に更新 | ✅ 反映 |
| 3 | NOT-only は Ctrl+F では許可してよい (SQLite 直接なので軽い) | §9.1 で「Ctrl+G: 禁止 / Ctrl+F: 許可」と明記 | ✅ 反映 |
| 4 | Ctrl+F の `WHERE path IN (...)` は fts_meta.db と同じ正規化形式にすべき | §9.2 ステップ 1 に `normalize_path()` 明示の注意書き追加 | ✅ 反映 |
| 5 | §15.1.9 は「確定」でなく「暫定 (プロトタイプで確認)」とすべき | §15.1.9 を「暫定方針 (プロトタイプ計測で確定)」に修正 | ✅ 反映 |

#### 5 回目 (2026-04-21, 軽微 2 件)

| # | 指摘内容 | 反映先 | 状態 |
| --- | --- | --- | --- |
| 1 | §8.3 の "インデックスがあれば FTS" が古い (今は fts_meta.db 直接方式) | §8.3 を "インデックス済みメタがあれば `fts_meta.db` 直接検索" に修正 | ✅ 反映 |
| 2 | §16 step 15 "FTS 経路に差し替え" も同様に古い | "fts_meta.db 直接検索経路に差し替え" に修正 | ✅ 反映 |

### 15.1 残論点 (再レビュー時の確認対象)

以下は今後のレビュー・実装時に再度判断する論点:

#### 15.1.1 エンジン選択の妥当性 — **プロトタイプ計測 PASS** (2026-04-21)

実施結果: [docs/search-bench-results.md](search-bench-results.md) 参照

- **Tantivy + bigram + post-filter + offset pagination は本採用** (50 万件規模まで検証済み)
- offset 肥大時の penalty は first page 比 2x 以内に収まる
- HARD_MAX 到達でも total 161ms、typical は 10〜50ms
- SQLite 500 件 post-filter lookup: 9ms
- インデックスサイズ (実測): raw テキスト × 0.07〜0.08x (従来見積もりの 1/20〜1/40、大幅に改善)

SQLite better-trigram との比較は実施せず (Tantivy が十分速いため不要と判断)。
Lindera は v1 で不要と判断済み (v2 以降再検討)。

#### 15.1.2 PDF 全文対応

- v1 ではタイトル/著者のみ、本文は opt-in v1.x 以降の方針で確定
- OCR ノイズは実サンプル入手次第、別索引 (`pdf_fts_index/`) に分離すべきかを判断

#### 15.1.3 ファイルシステム監視

- SMB/NAS のポーリング間隔はデフォルト 10 分、手動更新メニューあり。実使用時に調整
- notify-rs のバッファオーバーフロー検出時はフル再走査にフォールバック

#### 15.1.4 インデックス規模

- v1 の想定上限: お気に入り合計 50 万ファイル。これを超える可能性のあるユーザには
  「お気に入り単位のメタ索引 OFF」を案内する
- 100 万超の場合は `fts_index/` を favorite ごとに分割する拡張を v2 で検討

#### 15.1.5 クエリ構文拡張 (`field:value`)

- v1 では入れない (§9.1 のまま、シンプルな AND/NOT/phrase のみ)
- v1.x で tantivy の `QueryParser` を直接使わず、既存 [src/search_query.rs](../src/search_query.rs) を拡張する方針で実装

#### 15.1.6 競合するワーカー

- §7.5 で `GlobalIoSemaphore` 方針を確定。実装時に既存 PDF ワーカーの優先度キューと調整
- パフォーマンス測定: `--perf-log` で UI スクロール中のインデクサ稼働影響を計測

#### 15.1.7 Ctrl+G 集約粒度 (新設)

- §10.3 の集約は案 A (画像の直上フォルダ) で v1 採用
- ユーザが多ヒット時に結果リストが冗長と感じるなら、案 B (浅い階層で集約) をオプション化

#### 15.1.8 Ctrl+G × Ctrl+F の組み合わせ (新設)

- v1 では Ctrl+G 結果ビュー中は Ctrl+F を無効化
- v2 で「Ctrl+G で絞った結果に更に Ctrl+F で絞り込み」を入れるか検討

#### 15.1.9 post-filter の候補上限 — ページング方式 **確定** (プロトタイプで PASS)

- ~~§9.1 のステップ 4 で top-N=2000 で切る方針~~ ← 取り下げ済み
- **確定方針**: Tantivy は `TopDocs::with_limit(500).and_offset(offset).order_by_score()` でページング取得
  - プロトタイプ計測 (docs/search-bench-results.md) で worst case 実測済み
  - 50 万件 HARD_MAX 到達ケースで 161ms — 許容範囲
  - offset 肥大時の劣化も 2x 以内 — custom Collector への切り替えは **不要**
- post-filter で valid_hits が HARD_MAX (10,000) に到達した時点で打ち切り、UI に「結果が多すぎます」を表示
- streaming 設計 (§10.4) で最初のページ (〜5ms) から UI 反映

---

## 16. 実装順序 (暫定タスク分解)

Codex レビュー反映後の順序。`GlobalIoSemaphore` と二段整合性が骨格の早い段階で入る。

1. [x] **プロトタイプ計測 PASS** (2026-04-21): Tantivy + bigram + post-filter を 10 万 / 50 万件で計測。詳細は [docs/search-bench-results.md](search-bench-results.md)
2. [ ] `FavoriteEntry` に `id: Uuid` + 3 フラグ追加 + **`Raw::Full` 側の serde 対応** (§5.5) + マイグレーション (UUID 発行 + settings.json 書き戻し)
3. [ ] お気に入りエディタ / 追加ダイアログにチェックボックス追加
4. [ ] `fts_meta.db` スキーマ作成 + CRUD 層 + `index_generation` / tombstone 対応 (§5.3, §5.6)
5. [ ] `GlobalIoSemaphore` 実装 + 既存 PDF ワーカー・サムネイルワーカーへの組み込み (§7.5, Codex 指摘 #8)
6. [ ] Walker ワーカー + 差分検出 (Ingest キューへの enqueue のみ)
7. [ ] notify-rs 導入 + debouncer + SMB/NAS 向けポーリングフォールバック
8. [ ] Tantivy 依存追加 + スキーマ (bigram tokenizer, `all_text_norm` は SQLite 側のみ) + 正規化関数 `search_norm::normalize_for_match` 新設 (§5.2)
9. [ ] Ingest ワーカー (Low/Med/High 可変, §7.5) + 二段整合性プロトコル (§5.6)
10. [ ] ZIP 専用 ingest context: 外側 ZIP 1 回開く / nested cache 制限 (§7.7, Codex 指摘 #6)
11. [ ] クエリエンジン: Tantivy 候補絞り込み + post-filter (§9.1) + 最小クエリ長ポリシー (§4.3)
12. [ ] Ctrl+G UI 下準備: `push_grid_item_pending` ヘルパー + `GridItem::SearchContainer` variant 追加 (§10.4.2)
    - **全 `match` arm を監査** (Codex 3 回目指摘 #5): ナビゲーション / サムネロード / レーティング / 右クリックメニュー /
      フルスクリーン判定 / DnD / キー操作 / rename などで `GridItem` を match している箇所を全て洗い出し、
      新 variant への対応 or `_ =>` 拒否を明示。Grep 目安: `match .* GridItem`, `GridItem::` 参照
    - UI テスト: 結果ビューで右クリック / Enter / F5 / Shift+クリック等がクラッシュしないこと
13. [ ] Ctrl+G クエリワーカー: streaming (ページング post-filter + mpsc Batch イベント) (§10.4.2)
14. [ ] Ctrl+G UI: トップレベル集約表示 + drill-down 階層ビュー + streaming 反映 (§10.3, §10.4)
15. [ ] Ctrl+F 既存 UI を `fts_meta.db` 直接検索経路に差し替え (§9.2, 表示中 item path 集合で絞り込み)、インデックス未対応 path はオンデマンド fallback
16. [ ] `index_creator` / `cache_creator` を「インデックス管理」に統合
17. [ ] PDFium document info (Title/Author/Subject/Keywords) の取り込み
18. [ ] 環境設定に速度制限 (Low/Med/High)・AC 電源時限定オプション追加
19. [ ] 起動時 reconciliation フロー実装 (§5.6.3)
20. [ ] テスト (単体 + 統合 + `bench_search.rs`)
21. [ ] ドキュメント更新 (spec.md, architecture-overview.md, async-architecture.md, ui-responsiveness.md, virtual-folders.md)

---

## 17. 参考資料

- Tantivy: [quickwit-oss/tantivy](https://github.com/quickwit-oss/tantivy) (MIT)
- NgramTokenizer: [docs.rs NgramTokenizer](https://docs.rs/tantivy/latest/tantivy/tokenizer/struct.NgramTokenizer.html)
- Lindera (検討のみ): [lindera/lindera-tantivy](https://github.com/lindera-morphology/lindera-tantivy) (MIT)
- SQLite CJK 対応 trigram (保険): [streetwriters/sqlite-better-trigram](https://github.com/streetwriters/sqlite-better-trigram) (MIT)
- ファイル監視: [notify-rs/notify](https://github.com/notify-rs/notify)
- SQLite FTS5 trigram 性能議論: [SQLite forum post](https://sqlite.org/forum/info/3e9352773af9e7d5f6de80532affaadd2429eef107f3f811bdbb8f38cd953dc3)

---

## 18. 本ドキュメントの扱い

- **Codex レビュー 5 回目完了 (2026-04-21)** — 累計 28 + 軽微 2 = 30 件の指摘を反映。§15.0 参照
- §15.1.1 のプロトタイプ計測 (Tantivy offset worst-case 含む) に着手
- 実装中はこのドキュメントを最新化し続ける。v0.8.0 リリース時に「確定版」として [docs/README.md](README.md) の索引に追加する (設計メモ欄)

---

## 19. 検索絞り込みフィルタ拡張 (all_text 分割) [2026-04-22 追加]

### 19.1 目的

Ctrl+G / Ctrl+F の検索バー右側に 3 つのドロップダウンを追加する:

| フィルタ | 候補 | 既定 |
| --- | --- | --- |
| お気に入り | すべて / 登録済 favorite 名 (複数選択可) | すべて |
| タイプ | すべて / フォルダ / ZIP ファイル / PDF ファイル / 画像 | すべて |
| 検索対象 | すべて / ファイル名 / EXIF / AI プロンプト (PNG) / mXD ツイート / PDF メタ | すべて |

**方針**: §5.2 で採用した「`all_text` 単一フィールド」方式は検索対象フィルタと両立できないので、
未リリース (v0.8.0-alpha) の今のうちに **`all_text` を廃止してソース別フィールドに分割する**。
リリース後の再インデックスコストを避けるため、スキーマ再設計は必ず初版リリース前に済ませる。

### 19.2 データソース分類

ingest 段で元テキストを **5 種** に分けて保持する:

```rust
// src/ingest_text.rs に追加
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceKind {
    Filename,   // ファイル名 / ZIP エントリ名
    Exif,       // EXIF (カメラ・レンズ・GPS・撮影日時…)
    XmpTweet,   // XMP (mXD: tweet_id / author_* / description …)
    PngPrompt,  // PNG tEXt / iTXt (A1111 / ComfyUI プロンプト)
    PdfMeta,    // PDFium document info (Title / Author / Subject / Keywords)
}
```

UI 上は必要に応じて「AI プロンプト」「mXD ツイート情報」等の日本語ラベルにマップする。
PDF メタは PDF ファイル本体 (`container="fs"`, 拡張子 .pdf) にのみ現れる。
ZIP は自身のファイル名 (`Filename`) のみ持つ。ZIP 内エントリ (v1.x) は画像と同じ扱い。

### 19.3 Tantivy スキーマ変更 (fts_index)

`all_text` フィールドを廃止し、5 つのテキストフィールドに分割する。

```rust
// src/fts_index.rs build_schema() の最終形
b.add_text_field("path",        STRING | STORED);
b.add_text_field("container",   STRING | STORED);
b.add_text_field("zip_entry",   STRING | STORED);
b.add_text_field("favorite_id", STRING | STORED);
b.add_text_field("kind",        STRING | STORED);  // ★ 新規: "folder" / "image" / "zip" / "pdf"
b.add_i64_field ("mtime",       INDEXED | STORED);
b.add_i64_field ("file_size",   STORED);

let bigram = TextOptions::default().set_indexing_options(
    TextFieldIndexing::default()
        .set_tokenizer(BIGRAM_TOKENIZER_NAME)
        .set_index_option(IndexRecordOption::WithFreqs),
);
b.add_text_field("name",             bigram.clone());  // Filename ソースに対応
b.add_text_field("exif_text",        bigram.clone());  // ★ 新規
b.add_text_field("xmp_tweet_text",   bigram.clone());  // ★ 新規
b.add_text_field("png_prompt_text",  bigram.clone());  // ★ 新規
b.add_text_field("pdf_meta_text",    bigram);          // ★ 新規
// ★ all_text は削除
```

- **「すべて」検索**: `Occur::Should` で `name / exif_text / xmp_tweet_text / png_prompt_text / pdf_meta_text` の OR を取る。
  Tantivy は各フィールド独立に posting list を持つので OR-of-5 のコストは多くても稀語で数 ms、汎用語で数十 ms 程度 (§12.3 実測値を超えない)。
- **「ソース限定」検索**: 選択されたフィールドのみに対して同じ bigram AND を組む。
- **`kind` フィールド (タイプフィルタ用)**: `"folder"` / `"image"` / `"zip"` / `"pdf"` の exact term。
  Ctrl+G の結果集計で `GridItem` variant にマップする材料にもなる。
  既存の `container` は `"fs" / "zip"` で ZIP エントリ区別のために残す (用途が違う)。

#### Fields struct の変更

```rust
pub struct Fields {
    pub path: Field,
    pub container: Field,
    pub zip_entry: Field,
    pub favorite_id: Field,
    pub kind: Field,              // 新規
    pub mtime: Field,
    pub file_size: Field,
    pub name: Field,
    pub exif_text: Field,         // 新規
    pub xmp_tweet_text: Field,    // 新規
    pub png_prompt_text: Field,   // 新規
    pub pdf_meta_text: Field,     // 新規
    // all_text は削除
}
```

`IndexDoc` も同様に分解:

```rust
pub struct IndexDoc {
    pub path: String,
    pub container: Container,
    pub zip_entry: String,
    pub favorite_id: Uuid,
    pub kind: IndexKind,              // 新規: Folder / Image / Zip / Pdf
    pub mtime: i64,
    pub file_size: i64,
    pub name: String,                 // 正規化済み
    pub exif_text: String,            // 正規化済み (空なら空文字列)
    pub xmp_tweet_text: String,
    pub png_prompt_text: String,
    pub pdf_meta_text: String,
}
```

`IndexKind` は `search_index_db::IndexKind` (folder/zip/pdf) に `Image` を足して共有するのが望ましい。
ただし他モジュールへの波及が大きければ `fts_index` 内の別 enum にしてもよい。

### 19.4 fts_meta.db スキーマ変更

post-filter (`search_query::matches`) はソース別に走らせる必要があるので、
`all_text_norm` を 5 カラムに分割する:

```sql
CREATE TABLE files (
    path TEXT PRIMARY KEY,
    favorite_id TEXT NOT NULL,
    favorite_root TEXT NOT NULL,
    kind INTEGER NOT NULL,        -- ★ 新規: 0=folder, 1=image, 2=zip, 3=pdf
    mtime INTEGER NOT NULL,
    file_size INTEGER NOT NULL,
    indexed_at INTEGER NOT NULL,
    index_version INTEGER NOT NULL,
    index_generation INTEGER NOT NULL,
    status INTEGER NOT NULL,
    name_norm TEXT NOT NULL DEFAULT '',            -- ★ 元 all_text_norm を分割
    exif_norm TEXT NOT NULL DEFAULT '',            -- ★
    xmp_tweet_norm TEXT NOT NULL DEFAULT '',       -- ★
    png_prompt_norm TEXT NOT NULL DEFAULT '',      -- ★
    pdf_meta_norm TEXT NOT NULL DEFAULT ''         -- ★
);
CREATE INDEX idx_files_fav       ON files(favorite_id);
CREATE INDEX idx_files_fav_mtime ON files(favorite_id, mtime);
CREATE INDEX idx_files_kind      ON files(favorite_id, kind);  -- タイプフィルタ集計用
CREATE INDEX idx_files_status    ON files(status) WHERE status != 0;
```

- `INDEX_VERSION` を **1 → 2** に bump。起動時 reconciliation で旧スキーマ検出 → 全再インデックス。
- `mark_pending` の引数を `all_text_norm: &str` から `norms: &PerSourceNorm` に変更。
- post-filter lookup は `lookup_norms_for_targets(paths, targets: &[SourceKind]) -> HashMap<path, CombinedText>` のように
  「選択されたソース列を SELECT して結合文字列を返す」 API に寄せる。`SearchTarget::All` は 5 列全部 SELECT。

### 19.5 ingest_text 分割 (PerSourceText)

`build_all_text_for_file` / `build_all_text_from_bytes` を 5 ソース個別ビルダーに再設計する:

```rust
// src/ingest_text.rs
pub struct PerSourceText {
    pub name: String,            // すでに正規化済み
    pub exif: String,            // 空文字列なら抽出失敗 or そもそも無い
    pub xmp_tweet: String,
    pub png_prompt: String,
    pub pdf_meta: String,        // PDF 以外では常に空
}

impl PerSourceText {
    pub fn get(&self, kind: SourceKind) -> &str { ... }
    pub fn is_empty_all(&self) -> bool { ... }
}

pub fn build_per_source_for_file(path: &Path) -> PerSourceText { ... }
pub fn build_per_source_from_bytes(display_name: &str, bytes: &[u8]) -> PerSourceText { ... }
pub fn build_per_source_for_pdf(path: &Path, info_text: &str) -> PerSourceText { ... }
```

- 各フィールドに **個別に `normalize_for_match` を適用済み** にする (post-filter と bigram ingest の両方で使えるように)。
- `append_exif` / `append_xmp` の内部ロジックは変更不要。出力先を個別 `String` に切り替えるだけ。
- `Filename` ソースは `path.file_name()` を lowercase した文字列をそのまま入れる。

### 19.6 クエリ API 拡張 (SearchTarget)

`build_bigram_and_query` の引数を拡張する:

```rust
/// Ctrl+G / Ctrl+F の検索対象フィルタ (§19.2)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchTarget {
    All,                        // name + exif + xmp_tweet + png_prompt + pdf_meta の OR
    Only(Vec<SourceKind>),      // 指定ソースのみの OR
}

/// 既存 kind / favorite フィルタも同じ関数で対応する。
#[derive(Debug, Clone, Default)]
pub struct QueryFilters<'a> {
    pub favorite_ids: Option<&'a [Uuid]>,
    pub kinds: Option<&'a [IndexKind]>,    // None = all kinds
    pub target: SearchTarget,              // Default::default() = All
}

pub fn build_bigram_and_query(
    fields: &Fields,
    include_tokens: &[&str],
    filters: &QueryFilters,
) -> Option<BooleanQuery>;
```

既存呼び出し側 (`global_search.rs` / `ingest_worker.rs` / `indexer_manager.rs` / テスト群) は
`QueryFilters { favorite_ids: Some(ids), ..Default::default() }` で書き直す。

**組み立てロジック (include_tokens を field ごとに bigram AND → field 間 OR → 全体 AND)**:

```
for each token:
    for each target field F in target.fields():
        per_field_and = AND of TermQuery(F, bigram)  for bigram in ngram(token)
    token_query = OR of per_field_and     // どれか 1 フィールドにトークンが入っていれば OK
must_queries.push(token_query)
// favorite_id filter: Must(OR of Term(fav_id, id))
// kind filter:        Must(OR of Term(kind, "folder"|"image"|...))
return AND of must_queries
```

**post-filter も同じ target を渡す**: `search_query::matches` は引数 `&str` を `target` で選ばれた結合テキストで受ける。
`SearchTarget::All` → 5 ソースを連結した文字列 (区切りスペース)。これで `夕焼け` が EXIF 撮影地に、`camera` がファイル名にあるケースも AND 判定が通る。

### 19.7 UI 変更 (§8 補足) [2026-04-22 実装済み]

#### Ctrl+G (全検索バー)

```
[検索: クエリ入力      ] [×] [お気に入り ▼] [タイプ ▼] [検索対象 ▼] | 進捗/結果
```

- `GlobalSearchFilters { favorite: Option<Uuid>, kind: Option<IndexKind>, target: SearchTarget }` を `GlobalSearchState` に保持
- ドロップダウンは **single-select**: 「すべて」または 1 つを選ぶ (multi-select は UI 複雑化回避のため v1.x 以降)
- 変更時は debounce なしで即再検索 (クエリが空なら skip)
- **お気に入り**: `auto_index_metadata=true` なお気に入りのみ候補に出る
- **タイプ**: `IndexKind::{Folder, Image, Zip, Pdf}`
- **検索対象**: `TargetChoice::{All, Only(Filename|Exif|XmpTweet|PngPrompt|PdfMeta)}`

#### Ctrl+F (現在フォルダ検索バー)

```
[検索: クエリ入力      ] [×] [検索対象 ▼] | 進捗/結果
```

- お気に入り / タイプフィルタは Ctrl+F スコープでは意味が薄いため **検索対象のみ** を出す
- state は `App.search_target: SearchTarget` (既定 `All`)
- 変更時は即再検索 (クエリが空なら skip)
- fast path (`fts_meta.db.lookup_norms_for_target`) は target を尊重
- fallback path (未インデックス path) も target に応じて EXIF/XMP/PNG の読み取りを skip する (I/O 節約)

#### 実装箇所

- UI コンポーネント: `src/global_search_ui.rs` (`TargetChoice`, `TARGET_CHOICES`, `KIND_CHOICES`, `kind_label`)
- Ctrl+G バー描画: `render_global_search_bar` (`filter_changed` で即再実行)
- Ctrl+G spawn: `spawn_global_search` (`SearchScope { kinds, target }` を組み立てて `spawn_search` に渡す)
- Ctrl+F バー描画: `ui_main.rs::render_search_bar` (検索対象 ComboBox)
- Ctrl+F 実行: `run_metadata_search(tokens, items, xmp, fts_meta, target, cancel)` + `target_includes` ヘルパ

### 19.8 マイグレーション [2026-04-22 実装済み]

**未リリースなので最小コスト**で実装:

1. `INDEX_VERSION = 2` (fts_meta.db) に bump。
2. **`FtsMetaDb::open_at`**: `needs_rebuild(conn)` で旧スキーマ (all_text_norm 列有 / name_norm 列無) を検出 → `DROP TABLE files` → 新スキーマで再作成。`index_version` が古い行を持つ DB も同様に drop。
3. **`FtsIndex::open_at`**: `index_is_stale(dir)` で旧スキーマ (exif_text 等が無い / all_text 残存) を検出 → `remove_dir_all` → 新スキーマで `Index::create_in_dir`。
4. ユーザーが実際にやるべきこと: **何もしなくて良い**。次回起動時に自動で v1 → v2 マイグレーションが走り、バックグラウンドで全再インデックスが始まる。
5. 再インデックス所要時間は §12.2 の見積もりどおり (10 万ファイル SSD で 15-30 分)。UI はブロックされない。
6. `FavoriteEntry.auto_index_metadata=true` のお気に入りだけが対象。

#### マイグレーション失敗時の挙動

- `needs_rebuild` の判定はベストエフォート。SQLite 読み取りが失敗したら新規 DB として扱う (`needs_rebuild` が Err を返したら open_at も Err で上位の `IndexerManager::new` が None を返す → Ctrl+G 不可だが他機能は動く)。
- `wipe_index_dir` は Windows で `Access Denied` になる可能性があるが、後続の `Index::create_in_dir` が残骸を上書きするためログだけ残して続行する。

### 19.9 テスト方針

- `fts_index::tests` の全サンプルで `IndexDoc` を新構造に更新。`all_text` を参照するアサーションはソース別に分解。
- `ingest_text::tests` に **ソース分離の検証** を追加:
  - EXIF 情報を含む JPEG の `PerSourceText.exif` が非空、`.xmp_tweet` が空であること
  - PNG tEXt を持つファイルの `.png_prompt` が非空であること
  - XMP (mXD) を持つファイルの `.xmp_tweet` に author_screen_name が含まれること
- `global_search::tests` に target フィルタの E2E:
  - 同じトークンが EXIF と XMP 両方に存在するファイルを作り、`SearchTarget::Only(&[XmpTweet])` で XMP ソースのファイルのみがヒットすることを確認
- `fts_meta::tests`: 5 ソース分の upsert + lookup round-trip
- UI スナップショット: 3 ドロップダウン配置後の `tests/snapshots/` 更新

### 19.10 実装順序

§16 の既存タスク 1-21 をベースに、以下を挿入 / 差し替える:

1. **§19 スキーマ分割先行** (未リリース前に完了必須):
   1. `fts_index` の schema / `IndexDoc` / `Fields` / `build_bigram_and_query` 更新 + 既存テスト修正
   2. `fts_meta.db` スキーマ更新 + `INDEX_VERSION` bump + `mark_pending` / `lookup_*` API 変更
   3. `ingest_text::PerSourceText` 導入 + 既存 `build_all_text_*` を thin wrapper 化して段階削除
   4. `ingest_worker` / `indexer_manager` の呼び出しを新 API へ
2. **既存 §16 のフィルタ関連タスク**:
   - 12 (Ctrl+G UI 下準備) に「3 ドロップダウンの state + UI」を追加
   - 14 (Ctrl+G UI) に target/favorite/kind フィルタの実配線
   - 15 (Ctrl+F) でも同じ state を読む

工数目安 (§19 分): **4-5 日** (スキーマ変更 + ingest 分割 + クエリ拡張 + 既存テスト修正)。UI ドロップダウン配線は別途 1-2 日。
この設計を先行マージしてから既存 §16 ステップ 12 以降を進める。

# 検索システム 設計ドキュメント

検索システム (Ctrl+S / Ctrl+F / Ctrl+G + タグ機能) における **仕様選択の理由** と
個別項目の詳細設計をまとめる。

全体像・モジュールマップ・クエリ経路の概要は
[../../search-architecture.md](../../search-architecture.md) を参照。本書は「なぜこの設計か」
「スキーマのフィールド単位の根拠」「UI の詳細仕様」に絞る。

> **⚠ INDEX_VERSION=5 までの旧設計を含む** — 以下の節は v5 で本文 norms を SQLite に
> 持っていた頃の設計判断。v6 (現行) では:
> - 本文は Tantivy 側 (`*_text` STORED フィールド) に集約済み
> - `fts_meta.db.files.{name_norm,...,tags_norm}` 列は廃止
> - `status` は `Ok` / `Failed` の 2 値のみ (Pending / Tombstone は廃止)
> - Ctrl+G post-filter の `fts_meta` SELECT も廃止 (Tantivy First 順序)
> - 書き込み手順は [../../search-architecture.md §4.2](../../search-architecture.md) を参照
>
> 本書の §3.5 (`*_norm` 列) / §3.6 (二段整合性プロトコル, Pending/Tombstone) /
> §3.7 タグ即時反映周辺の記述は v5 当時の根拠資料として残しているもので、現行
> 実装の正としては扱わないこと。

---

## 1. 目的とスコープ

検索機能は以下の 3 モードで構成する。

| ショートカット | スコープ | 経路 |
| --- | --- | --- |
| Ctrl+S | お気に入り全体のフォルダ / ZIP / PDF / 動画名 (再帰) | `search_index.db` (SQLite LIKE) |
| Ctrl+F | 現在グリッドに表示中の一覧のみ (非再帰) | `fts_meta.db` 直接 lookup + 未登録分 on-demand fallback |
| Ctrl+G | お気に入り全体 + ZIP 内画像 (再帰) | Tantivy bigram 候補絞り込み + `fts_meta.db` post-filter の streaming |

あわせて以下を実現する:

1. お気に入り単位の自動インデックス管理 (`auto_index_{structure,metadata,thumbs}` 3 フラグ)
2. バックグラウンド自動メンテナンス (notify-rs + debounce + 3-way diff)
3. 「インデックス作成」「キャッシュ作成」ダイアログの統合
4. 日本語に強い全文検索 (bigram)

---

## 2. 検索エンジン方式の選択

### 2.1 検討した選択肢

| 案 | 日本語対応 | バイナリ増分 | インデックスサイズ | クエリ性能 | 実装コスト |
| --- | --- | --- | --- | --- | --- |
| A. LIKE 据え置き | △ | 0 | 小 | 大規模で線形劣化 | 最小 |
| B. SQLite FTS5 + unicode61 | × (CJK 非対応) | 0 | 小 | 中 | 小 |
| C. SQLite FTS5 + trigram | △ (3 文字未満ヒットせず) | 0 | 中 | 中 | 小〜中 |
| D. SQLite FTS5 + better-trigram (CJK-aware) | ○ | +数十 KB | 中 | 中 | 中 |
| **E. Tantivy + NgramTokenizer(2,2)** | ◎ | +約 10〜15 MB | 中〜大 | 良好 | 中 |
| F. Tantivy + Lindera (IPADIC) | ◎◎ | +約 25 MB | 中 | 良好 | 中〜大 |

### 2.2 案 E (Tantivy + bigram) を採用する理由

1. **検索漏れの少なさ** — 形態素解析 (案 F) は辞書にない語彙 (新語 / 固有名詞 /
   AI プロンプトの英語混在) を分割ミスする。bigram は「2 文字以上の部分一致なら
   ほぼ必ず拾える」特性で、画像メタや AI プロンプトのような未知語だらけのデータに強い。
2. **「1 文字検索禁止」と仕様が一致** — bigram は 2 文字未満で index が効かない。
   1 文字クエリは「対象外」として明示的に弾ける。
3. **実装負荷** — Lindera 辞書 13 MB 追加を避けられる。exe は既に AI モデル埋め込みで
   ~150 MB あるが、不要な肥大化は避ける方針。
4. **クエリ性能** — Tantivy は Lucene 系フルインバートインデックスで、想定規模
   (数十万件) で数 ms〜数十 ms。SQLite FTS5 trigram は書き込み性能が劣る
   ([SQLite forum: FTS5 trigram slow on insert](https://sqlite.org/forum/info/3e9352773af9e7d5f6de80532affaadd2429eef107f3f811bdbb8f38cd953dc3))。

### 2.3 案 D (better-trigram) を保険として残す理由

Tantivy 導入がライセンス / ビルドの都合で難しいと判明した場合の退路。SQLite は
bundled なので C 拡張 1 つで済む。ただし trigram は bigram より「2 文字検索時の
偽陽性」が増え、CJK では情報量が少ない (漢字 3 文字連続は意味単位を跨ぐ) ため、
bigram の方が精度が高い。

### 2.4 ngram パラメータ

`NgramTokenizer::new(2, 2, false)` + `LowerCaser`。

最小クエリ長ポリシー:

| クエリ種別 | 最小文字数 | 例 |
| --- | --- | --- |
| CJK を 1 文字でも含む | 2 文字 | `街並` OK / `街` NG |
| 英数字のみ | 3 文字 | `sdx` OK / `sd` NG |
| 記号混じり | 2 文字 | `#1`, `@u` |

ASCII 2 文字 (`sd` `xl` `ai` …) は汎用 bigram が爆発的にヒットするため、英数のみで
3 文字以上を要求する。判定は [search_query.rs](../src/search_query.rs) のトークン分類器。

### 2.5 phrase / NOT / AND は post-filter で最終判定する

Tantivy の `NgramTokenizer` は **token position を常に 0 で吐く仕様**
([NgramTokenizer docs](https://docs.rs/tantivy/latest/tantivy/tokenizer/struct.NgramTokenizer.html))。
そのため:

- **Tantivy だけでは phrase (`"..."`) を正しく判定できない** (隣接判定不能)
- bigram 上の AND は「bigram 断片が両方現れる doc」であり、元クエリの連続部分一致
  とは異なるため偽陽性が出る
- NOT (`-word`) を Tantivy に渡すと position=0 由来で誤判定

よって:

- **Tantivy は『候補絞り込み』のみ** — 全 bigram の AND (または mode=OR のとき Should) で候補を集める
- **最終判定は post-filter** — 候補 doc に対応する `fts_meta.db` の 正規化済み原文で
  `search_query::matches` と等価な関数を走らせて phrase / NOT / AND を再評価
- この構造上、候補 → post-filter の flow は必須。候補数が多い場合はページング取得
  (`TopDocs::with_limit(500).and_offset(offset)`、HARD_MAX=10,000 で打ち切り) で
  streaming する

### 2.6 ライセンス

- Tantivy: MIT
- lindera-tantivy (保険): MIT
- sqlite-better-trigram (保険): MIT

mimageviewer 本体は MIT なのでいずれも組み込み可能。再配布時は LICENSE 表記追加。

---

## 3. スキーマ設計

### 3.1 DB 構成

`%APPDATA%/mimageviewer/` 配下:

| ファイル | 目的 | 技術 |
| --- | --- | --- |
| `search_index.db` | Ctrl+S 用フォルダ/ZIP/PDF/動画 構造 index | SQLite |
| `fts_index/` | Tantivy index ディレクトリ (複数 segment + meta.json) | Tantivy |
| `fts_meta.db` | Tantivy で持ちにくいメタ情報 (mtime / size / status / ソース別 normalized 全文) | SQLite |

**責任分離の理由**:

- Tantivy はファイル単位の「変更検出」クエリが苦手。差分検出は SQLite の indexed
  クエリで回し、確定した差分のみ Tantivy にコミットする。
- post-filter 用の原文は Tantivy に持たせず `fts_meta.db` に集約 (§3.2 参照) —
  segment 肥大と compaction 負荷を避けるため。

### 3.2 Tantivy スキーマ (fts_index)

```rust
// pseudo — 現行スキーマ
b.add_text_field("path",        STRING | STORED);   // 完全一致キー (正規化済み)
b.add_text_field("container",   STRING | STORED);   // "fs" / "zip"
b.add_text_field("zip_entry",   STRING | STORED);   // ZIP 内相対パス
b.add_text_field("favorite_id", STRING | STORED);   // FavoriteEntry.id (UUID) を文字列化
b.add_text_field("kind",        STRING | STORED);   // "folder" / "image" / "zip" / "pdf"
b.add_i64_field ("mtime",       INDEXED | STORED);
b.add_i64_field ("file_size",   STORED);

// === bigram tokenizer + lower_caser で index するテキストフィールド ===
b.add_text_field("name",             bigram);  // ファイル名 (拡張子含む)
b.add_text_field("exif_text",        bigram);
b.add_text_field("xmp_tweet_text",   bigram);  // XMP ツイート情報 (xtw:*)
b.add_text_field("png_prompt_text",  bigram);  // A1111 / ComfyUI / Midjourney
b.add_text_field("pdf_meta_text",    bigram);  // PDFium document info
b.add_text_field("tags",             bigram);  // XMP dc:subject 由来 (#タグ)
```

#### 3.2.1 post-filter 用原文を Tantivy に持たせない

`fts_meta.db.files.{name_norm,exif_norm,...}` に一元化する。Tantivy から得た
`path` をキーに `SELECT ... FROM files WHERE path IN (?,?,...)` で一括取得する
(500 件 バッチで 9ms 程度)。

これにより segment サイズが小さくなり、commit / merge / reader reload が軽くなる。

#### 3.2.2 正規化は `to_lowercase()` のみ

3 箇所 (ingest 時の norms 生成 / クエリパース時 / post-filter 時) で必ず同じ関数を
通す。`src/search_norm.rs::normalize_for_match` が唯一の実装。

**NFKC は採用しない**: 全角英数 / 半角カナ等の正規化を入れると入力と index の
解釈がずれやすい。v2 で必要になったら `INDEX_VERSION` を bump して全再構築
(`unicode-normalization` crate 追加前提)。

#### 3.2.3 `favorite_id` は UUID 文字列 (exact term) で保持

SHA-1 短縮は衝突リスクがあるので採用しない。`FavoriteEntry.id: Uuid` を
そのまま文字列化して STRING field に入れる。

副次効果: お気に入りの表示名変更 (rename) では UUID 不変 → index 保持。
root path 変更時のみ再スキャンが走る (§3.4 参照)。

#### 3.2.4 ソース別にフィールドを分ける理由

単一 `all_text` フィールドでは「検索対象=EXIF だけ」のようなフィルタが実装できない。
[ingest_text::PerSourceText](../src/ingest_text.rs) でソース別に分けてビルドし、
Tantivy / fts_meta の双方で列を分ける。

- 「すべて」検索は 6 フィールドの Should 群を束ねた OR
- 「ソース限定」は選択されたフィールドのみに対して同じ bigram 構造を組む

個別フィールドに分けたことで `field:value` 構文 (v1.x 以降) の拡張も素直に入る。

#### 3.2.5 ドキュメント粒度

- 通常ファイル: 1 画像 = 1 doc
- ZIP 内画像: 1 エントリ = 1 doc (`container="zip"`, `zip_entry` に相対パス、
  `path` は `<zippath>\u{1F}<entry>` 形式、separator は `search_norm::ZIP_ENTRY_SEP`)
- PDF: ファイル本体 1 つ = 1 doc (本文は対象外、§5 参照)

### 3.3 fts_meta.db

```sql
CREATE TABLE files (
    path TEXT PRIMARY KEY,            -- 正規化済み (ZIP 内は "<zippath>\u{1F}<entry>", search_norm::ZIP_ENTRY_SEP)
    favorite_id TEXT NOT NULL,        -- FavoriteEntry.id (UUID)
    favorite_root TEXT NOT NULL,      -- 表示・集計用の原文パス
    kind INTEGER NOT NULL,            -- 0=folder, 1=image, 2=zip, 3=pdf
    mtime INTEGER NOT NULL,
    file_size INTEGER NOT NULL,
    indexed_at INTEGER NOT NULL,
    index_version INTEGER NOT NULL,   -- schema 変更時の再構築用
    index_generation INTEGER NOT NULL,-- ingest 世代 (§4 整合性用)
    status INTEGER NOT NULL,          -- 0=ok, 1=pending, 2=failed, 3=tombstone
    name_norm TEXT NOT NULL DEFAULT '',
    exif_norm TEXT NOT NULL DEFAULT '',
    xmp_tweet_norm TEXT NOT NULL DEFAULT '',
    png_prompt_norm TEXT NOT NULL DEFAULT '',
    pdf_meta_norm TEXT NOT NULL DEFAULT '',
    tags_norm TEXT NOT NULL DEFAULT ''
);
CREATE INDEX idx_files_fav       ON files(favorite_id);
CREATE INDEX idx_files_fav_mtime ON files(favorite_id, mtime);
CREATE INDEX idx_files_kind      ON files(favorite_id, kind);
CREATE INDEX idx_files_status    ON files(status) WHERE status != 0;
```

- `INDEX_VERSION` の不一致 / `all_text_norm` (旧スキーマ) 残存を検出した DB は
  `needs_rebuild` が true を返し、起動時に `DROP TABLE files` → 新スキーマで作り直す。
  Tantivy index 側 (`fts_index/`) も schema 不一致なら `remove_dir_all` + 再作成。
- `idx_files_status` は進行中の行だけを拾うための部分 index (通常 0=ok が大多数で
  部分 index なので小さい)。

### 3.4 FavoriteEntry の拡張

```rust
pub struct FavoriteEntry {
    pub id: Uuid,                      // 安定 ID
    pub name: String,
    pub path: PathBuf,
    pub auto_index_structure: bool,    // Ctrl+S 用 (フォルダ / ZIP / PDF / 動画名)
    pub auto_index_metadata: bool,     // Ctrl+F / Ctrl+G 用 (全文メタ)
    pub auto_index_thumbs: bool,       // サムネイル事前キャッシュ
}
```

#### 3.4.1 UUID の保護範囲

- **表示名 (`name`) 変更**: UUID 不変 → index 保持、何もしない
- **root `path` 変更** (お気に入りを別ディレクトリに付け替え): 物理的に全 doc の
  `path` が無効化されるので、旧 path 配下を tombstone → 新 path を再スキャン
  (起動時差分走査と同じ経路)
- **一括 path 更新 (prefix 置換 UPDATE) はしない** — 正規化の大小 / 区切り文字 /
  ZIP エントリ境界で事故りやすい。お気に入り編集ダイアログで path 変更時に確認を出す。

#### 3.4.2 serde の手書き実装に合わせる

[settings.rs](../src/settings.rs) の `FavoriteEntry` は
`Deserialize` / `Serialize` が手書きで `Raw::Legacy(String)` と
`Raw::Full { ... }` を分岐する。単に `#[serde(default)]` を付けるだけでは
効かないので、`Raw::Full` 側にもオプション化を明示する:

```rust
#[derive(Deserialize)]
#[serde(untagged)]
enum Raw {
    Legacy(String),
    Full {
        #[serde(default = "Uuid::nil")]
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
```

`Raw::Legacy` 分岐 / `id=nil` の場合は起動時に UUID v4 を発行して `settings.save()` で
書き戻す (1 回限り)。

### 3.5 search_index.db (Ctrl+S 用)

既存の `entries(path PK, display_path, name, display_name, kind, favorite_root, mtime, updated_at)`
に以下を追加:

```sql
ALTER TABLE entries ADD COLUMN indexed_by_auto INTEGER NOT NULL DEFAULT 0;
-- 0: 手動 / 従来エントリ (既存データはこれ)
-- 1: 自動インデクサが書き込んだ
```

お気に入り単位の ON/OFF は supervisor 側で判断するので、DB スキーマ変更は最小限。

### 3.6 二段整合性プロトコル

fts_meta.db (SQLite) と Tantivy index は別ストレージ。片方だけ書かれてクラッシュ
しても復元できるよう 2 段で書く:

**Upsert**:

```
(1) fts_meta.db UPSERT: status=pending, index_generation += 1, norms を書く
    (SQLite 単独 tx、ここが失敗なら何もコミットしない)
(2) Tantivy writer に delete(path) + add_document を push (バッファに積む)
(3) バッチ境界 (100 件 or 5 秒) で IndexWriter::commit()
(4) fts_meta.db UPDATE: status=ok, indexed_at=now
    (ここで失敗しても pending のまま残す = 次回起動時に再 ingest)
```

**Delete**:

```
(1) fts_meta.db UPDATE: status=tombstone
(2) Tantivy writer に delete(path)
(3) バッチ境界で commit()
(4) fts_meta.db から物理 DELETE
```

Tombstone 中は post-filter で明示除外する (Tantivy の delete 伝搬遅延で古い doc が
ヒットしても UI には出さない)。

**起動時 reconciliation**:

- `status != ok` の残留行を取る
  - pending → Tantivy に入っていない可能性 → 再 ingest キュー
  - tombstone → Tantivy に残っている可能性 → 削除再投入
  - failed → 再 ingest キュー (最大 3 回、超えたらスキップ)
- (週 1 など低頻度メンテで) Tantivy segment の path を fts_meta と突き合わせて orphan 検出

`index_generation` はファイル単位の世代カウンタ。将来的に「古すぎる doc を信用しない」
判定に使える余地を残してあるが、現在は記録のみ。`index_version` は DB 全体のスキーマ
世代で、bump = 全再構築。

### 3.7 なぜこの 2 レベルを分けるか

| 項目 | `index_version` | `index_generation` |
| --- | --- | --- |
| 単位 | 全 DB グローバル | ファイル単位 |
| 変化きっかけ | スキーマ変更 (定数 bump) | ingest / delete ごと |
| 不整合時の扱い | 全 DB を捨てて再構築 | 古い doc を捨てて再 ingest |

スキーマ互換性と個別整合性を独立に扱いたいため 2 レベル。

---

## 4. PDF 全文検索を対象外にする理由

### 4.1 技術的難易度

PDFium はテキスト抽出 API (`FPDFText_GetText` / `FPDFText_GetBoundedText`) を持つので
実装入口はあるが、本質的な難しさは以下:

1. **処理時間**: テキスト抽出はページ単位で数十〜数百 ms。1000 ページで数十秒
2. **データ量**: OCR 付き PDF (マンガのルビ・背景テキスト等) は 1 ページあたり
   数 KB〜数十 KB。数百冊で数百 MB の全文データ
3. **正確性**: OCR 結果は誤認識が多い ("の" → "ゐ" など)。bigram 索引に混ぜると
   偽陽性が急増
4. **ワーカープロトコル追加**: 既存 PDF ワーカーの IPC に `extract_text(page)` を
   足す必要あり

### 4.2 v1 での方針

**PDF 全文は v1 で対象外**。以下の妥協案にする:

- v1: PDF はファイル名 + **PDFium の document info** (`Title` / `Author` / `Subject` /
  `Keywords`) のみを `pdf_meta_text` として index する (1 PDF あたり 1 回の軽量読み込み)
- v1.x 以降: ユーザ設定で「PDF 全文も対象にする」opt-in を検討。ON 時は
  別 index (`pdf_fts_index/`) に分離し、ノイズデータを本体から隔離

---

## 5. バックグラウンド・インデックス・メンテナンス

### 5.1 イベントソース

- **起動時 sync**: 前回記録した `fts_meta.db` vs 現在の FS 状態を比較 → 差分を処理
- **実行中**: [notify-rs](https://github.com/notify-rs/notify) でお気に入りルートを
  recursive watch (Windows では `ReadDirectoryChangesW`)
- **手動トリガ**: インデックス管理ダイアログの「再構築」で選択した お気に入りを強制再スキャン

### 5.2 notify-rs の注意点 (Windows)

- `RecursiveMode::Recursive` で OS 側の `FILE_NOTIFY_CHANGE_*` が連続して来るため、
  **debouncer 必須** (短時間に数百件届くことあり)
- ネットワーク共有 (SMB / NAS) では `ReadDirectoryChangesW` が発火しないケースがあり、
  定期ポーリングを fallback として併走させる想定
- watch ハンドル数は OS 上限があるため、お気に入り root 単位でまとめて watch
  (サブディレクトリを個別 watch しない)
- `File.tmp` → `File.ext` のダウンロード完了パターンは `Create` + `Rename` の
  2 イベントで届く → debouncer で 1 件に集約
- Windows の rename は `Modify(Name(From))` + `Modify(Name(To))` として届くので、
  From を `ChangeKind::Remove` / To を `ChangeKind::Upsert` に分ける
  (`absorb_event`)。単一の `Modify(_) → Upsert` にすると rename 元が残留する。
  保険として `apply_single_change` の Upsert で「メタ取得不可 = 削除」に fallback する

### 5.3 差分検出フロー

```
notify Event → debouncer (500ms) → イベントキュー
                              │
                              ▼
              ┌───────────────────────────────────┐
              │ Diff Applier (1 スレッド)          │
              │                                   │
              │ path がお気に入り scope 内か?      │
              │   No  → 無視                      │
              │   Yes → kind 判定:                │
              │     Create  → Ingest キュー       │
              │     Modify  → mtime/size 比較:    │
              │       変化なし → 無視             │
              │       変化あり → Ingest 再実行    │
              │     Remove  → DB から削除         │
              │     Rename  → 削除 + 追加         │
              └───────────────────────────────────┘
```

### 5.4 起動時差分走査

1. 各お気に入りルートを walk (rayon 並列、`entry.file_type()` を使う -
   `Path::is_dir/is_file` は per-entry `GetFileAttributes` syscall で重い)
2. `fts_meta.db` から登録済みファイルリストを取得 (favorite_id でフィルタ)
3. 3-way diff:
   - FS にあり DB になし → Ingest キュー
   - DB にあり FS になし → 削除キュー
   - 両方にあり mtime/size 差 → 再 Ingest キュー
4. ZIP ファイル自体の mtime 変化 = **ZIP 内全エントリ再インデックス**
   (ZIP 内 mtime を個別取得するコストを避けるため)

### 5.5 Ingest Worker の負荷プロファイル

Tantivy `IndexWriter` は **1 本**。Tantivy は内部でマルチスレッドインデクシングを
行う設計なので、writer を複数持たない。メタ抽出ワーカーは N 本。

| プロファイル | メタ抽出ワーカー数 | レート制限 | 想定環境 |
| --- | --- | --- | --- |
| Low | 1 | 1 ファイル/秒 | HDD / NAS / バッテリー |
| Medium (既定) | 2 | 制限なし | HDD + SSD 混成 |
| High | 4 | 制限なし | NVMe SSD |

#### 5.5.1 GlobalIoSemaphore

既存の PDF ワーカー (3 プロセス) / サムネイルワーカー / インデクサが同時に HDD を
シークすると UI スクロールがつまる。**全ワーカー横断の I/O 同時実行制御**
(`src/io_semaphore.rs`) で調停する:

- `tokio::sync::Semaphore` 相当を `Mutex + Condvar` で自前実装 (try_lock + sleep は
  [../../async-architecture.md §5.5](../../async-architecture.md) で禁止)
- 優先度: (UI 表示中ページ) > (PDF 背景 / サムネ) > (インデクサ)
- インデクサが握る permit は 1〜2 に制限 (High プロファイルでもフォアグラウンドを
  阻害しない)
- 高優先度が連続する間は低優先度が無制限に待たされるが、これは **UI 応答性最優先**
  の意図的な選択。アイドル 数秒で低優先度が進む。

#### 5.5.2 バッチコミット

`IndexWriter::commit()` は fsync を伴うため、100 件 or 5 秒ごとにまとめる。
§3.6 の `fts_meta.status` 遷移も同じバッチ境界で行う。

#### 5.5.3 キャンセル

`Arc<AtomicBool>` で即中断。未コミット分は破棄し、次回起動時の差分検出で拾い直す
(`fts_meta.status=pending` の残留で再試行が起きる)。

### 5.6 ZIP インデックス負荷の抑制

現行 [zip_loader.rs](../src/zip_loader.rs) のネスト ZIP バイト列キャッシュは、
大量 ZIP を横断すると RAM / I/O を食いやすい。インデクサ専用の制約を設ける:

- **外側 ZIP を 1 回だけ開く**: 1 ZIP 内の全エントリを連続 ingest (開き直さない)。
  `ZipArchive` を借りたまま entry を舐める専用 API を使う
- **ネスト ZIP キャッシュは 1 レベルに制限**: インデクサ context では内側 ZIP を
  処理 → 即 drop。通常の閲覧 context のような LRU は使わない
- **巨大 ZIP は後回し (tiered)**:
  - 通常 (< 100 MB): 通常優先度
  - 中規模 (100 MB〜1 GB): low priority
  - 巨大 (> 1 GB): ユーザ操作アイドル時 (入力なし 30 秒) のみ処理
- **ZIP 自体の mtime 変化**: §5.4 の通り全エントリ再インデックス。ただし再中は
  古い doc を Tantivy から消さない (中断しても検索から完全に消えない)
- **読み取り失敗**: 破損 / パスワード付き ZIP は `status=failed` で 24 時間抑止

### 5.7 負荷設定 (環境設定)

- 「自動インデックスの速度」: Low / Medium / High (§5.5 の表)
- 「AC 電源時のみインデックス」: ノート PC 用
- 「サムネイルキャッシュ領域上限」: 既存設定を流用

---

## 6. UI 仕様

### 6.1 お気に入りエディタ / 追加ダイアログ

既存の 3 列 (名前 / パス / 操作) に チェックボックス 3 つを追加:

```
名前          パス        ✓構造 ✓メタ ✓サムネ  ↑↓🗑
お気に入り A  D:\photos\  ☑    ☑    ☑      ↑↓🗑
...                        インデックスサイズ: 142 MB
[一括ON] [一括OFF]   [今すぐ再構築]
```

フッターに現在のインデックスサイズを出す。

追加ダイアログのチェックボックス初期値は環境設定の「新規お気に入りの既定」から。

### 6.2 インデックス管理ダイアログ

旧「一括キャッシュ作成」+ 旧「インデックス作成」を統合:

```
┌─────────────────── インデックス管理 ──────────────────────┐
│ 🚀 起動時整合性チェック: 12 ms (pending 0 / tombstone 0 / I/O 2)│
│                                                                 │
│ 表示名 | パス | 状態 | スキャン      | 取込 | 削除 | 操作     │
│ A      | ...  | ✅   | 890ms / 3214件| 123  | 0    | [🔄再構築]│
│ B      | ...  | ⏳   | —             | 0    | 0    | [🔄再構築]│
│                                                                 │
│ [🔄 すべて再構築]  [閉じる]                                     │
└─────────────────────────────────────────────────────────────────┘
```

- 上部バナー: reconciliation 所要時間 + `pending_cleaned` / `tombstone_purged` /
  `io_permits` (index 本体の健全性指標)
- 行: 直近フル再走査の `ms / ファイル数`。ツールチップに初期スキャン時間 + 診断
  (read_dir / metadata / 深さ上限) を折りたたみ表示
- walk / ingest 失敗が 1 件でもある行は橙色でハイライト

### 6.3 Ctrl+G (グローバル検索バー)

```
[検索: クエリ入力      ] [×] [お気に入り ▼] [タイプ ▼] [検索対象 ▼] [□OR] | 進捗/結果
```

- `GlobalSearchFilters { favorite: Option<Uuid>, kind: Option<IndexKind>, target: SearchTarget, mode: MatchMode }`
  を `GlobalSearchState` に保持
- ドロップダウンは single-select (multi-select は v1.x 以降)
- 変更時は即再検索 (クエリ空なら skip)
- お気に入り候補は `auto_index_metadata=true` のもののみ
- タイプ候補: `Folder / Image / Zip / Pdf`
- 検索対象候補: `All / Filename / Exif / XmpTweet / PngPrompt / PdfMeta / Tags`
- `□OR` は session-local (再起動でリセット)

#### 6.3.1 drill-down ビュー

Ctrl+F と違い、**結果を新しい「検索結果ビュー」に切り替える**。
Ctrl+S の「お気に入り横断フォルダ検索」をメタデータまで拡張した形。

```
[2. トップレベル集約結果 — Aggregated view]
  🔍 "夕焼け"  [x]  45 件 (8 フォルダ, 3 ZIP)
  📁 D:\photos\2025\sunset\    12 枚ヒット
  📁 D:\photos\2024\landscape\  8 枚ヒット
  📦 E:\archives\yakei.zip     15 枚ヒット
  ...  (ヒット件数降順、同数で名前昇順)

[3. drill-down (DrilledInto view)]
  クリックでそのコンテナに降り、検索状態を保持したまま「その階層の該当
  ファイル + 該当を含む子フォルダ」のみを表示
  - 子フォルダに件数バッジ
  - ZIP に入ったら ZIP 内 path を filter scope にする
  - 画像クリックでフルスクリーンは通常通り

[4. 検索解除]
  - Escape / ✕ で通常ビュー (Ctrl+G 押下前の場所) に戻る
  - 「戻る」キーで [2] に戻る (階層は保持しない)
```

状態モデル:

```rust
struct GlobalSearchState {
    query: String,
    query_ast: QueryAst,
    results_by_container: Vec<ContainerHit>,
    view: GlobalSearchView,
}

enum GlobalSearchView {
    Aggregated,
    DrilledInto {
        container: ContainerRef,
        zip_sub_path: Option<String>,
    },
}

struct ContainerHit {
    path: PathBuf,
    kind: ContainerKind,  // Folder / Zip
    hit_count: usize,
    hit_paths: Vec<PathBuf>,
}
```

集約粒度: 「画像ファイルの直上フォルダ」単位 (v1)。ユーザが結果リスト冗長と
感じたら「お気に入り root + 第 2 階層」単位などの浅い集約をオプション化する。

ZIP 内 drill-down: ZIP は実ディレクトリツリーではないが、nested ZIP をフラットパス
(`chapters/ch01.zip/page01.jpg`) で扱えているので `/` split で疑似階層を再現する。

インデックス未完了時: トップバー右に `〔インデックス 68% — 部分結果〕` バッジ。
進行に応じて結果をインクリメンタル反映。

#### 6.3.2 他 UI との共存

- Ctrl+G 結果中に Ctrl+S が押されたら Ctrl+G を閉じて Ctrl+S を開く (排他)
- Ctrl+F は Ctrl+G とは別レイヤー。Ctrl+G 結果内の更に絞り込みは **v1 では無効化**
  (組合わせが複雑すぎるため)
- フルスクリーンに入っても検索状態は保持。フルスクリーン終了で同じ drill-down に戻る

### 6.4 Ctrl+F (ローカル検索バー)

```
[検索: クエリ入力      ] [×] [検索対象 ▼] [□OR] | 進捗/結果
```

- お気に入り / タイプフィルタは Ctrl+F スコープで意味が薄いので出さない
- `App.search_target: SearchTarget` を保持 (既定 `All`)
- 変更時は即再検索
- fast path (`fts_meta.db` 一括 SELECT) は target を尊重
- fallback path (未インデックス) も target に応じて EXIF / XMP / PNG の読み取りを
  skip (I/O 節約)

### 6.5 Ctrl+S (名前検索バー)

既存 UX を維持。結果はフラットなフォルダ / ZIP / PDF リスト。クリックで遷移。

---

## 7. クエリ実行フロー詳細

### 7.1 Ctrl+G

1. ユーザ入力 → 300ms debounce
2. クエリ最小長チェック (§2.4)
3. クエリパーサ ([search_query.rs](../src/search_query.rs)) で AST 化 (AND / NOT /
   phrase + `MatchMode::{And,Or}`)
4. **NOT-only 拒否**: 正のトークン (AND 項 / phrase) が 0 個で NOT だけのクエリは
   Tantivy 側で絞り込めず全件 scan になる。UI で「含める語を 1 つ以上入力して
   ください」を表示して早期 return
5. **Searcher snapshot を固定**: ワーカー開始時に `IndexReader::searcher()` を 1 回
   取得し、以降のページング取得はすべて同じ Searcher で行う。検索中に commit /
   reader reload が起きても snapshot は古い seg を見続けるので、offset pagination
   での重複 / 抜けを防ぐ
6. **候補絞り込み (Tantivy) — ページング取得**:
   - `favorite_id` 絞り込み (`auto_index_metadata=true` の favorite IN)
   - 正トークンを bigram 分解し 6 フィールド (filename / exif / xmp_tweet /
     png_prompt / pdf_meta / tags) を跨ぐ BooleanQuery
     - AND モード: 各トークンの「フィールドまたぎ OR」を top-level Must で結合
     - OR モード: 各トークンの「フィールドまたぎ OR」を Should 群にまとめて
       1 Must として top-level に入れる
   - NOT は **Tantivy に渡さない** (position=0 で誤判定するため、§2.5)
   - 固定 top-N で切らない (bigram は偽陽性があり、真の一致が top-N 外に落ちる漏れが出る)
   - `TopDocs::with_limit(500).and_offset(offset)` で 500 件ずつ繰り返しフェッチ
   - 停止条件: (a) post-filter 後の valid_hits が HARD_MAX=10,000 到達、(b) 候補
     使い切り、(c) cancel
   - HARD_MAX 超過は UI に「結果が多すぎます — キーワードを追加してください」を表示
7. **post-filter 最終判定** — ページ単位 streaming:
   - 各ページの path から `fts_meta.db` の `{target 列}` を一括 SELECT (prepared IN)
   - `search_query::matches_with_mode(ast, text, mode)` で phrase / NOT / AND を最終判定
   - `status=tombstone` はここで除外
   - 通過結果は即座に UI へ streaming (§7.3)
8. 停止時 UI:
   - (a) HARD_MAX: `〔10,000 件で打ち切り — キーワードを追加してください〕`
   - (b) 候補使い切り: `〔N 件〕`
   - (c) インデックス進行中: `〔現在 N 件 — インデックス M% 作成中〕`

### 7.2 Ctrl+F (Tantivy を経由しない)

**Tantivy を通さない理由**: 対象が表示中 item (数十〜数千) に限定されるので、
bigram 候補絞り込みは不要。むしろ「グローバル候補を走査してから表示中 path 集合で
絞る」方式だと無駄走査が出る。SQLite 直接の方が (a) 検索漏れゼロ、
(b) シンプル、(c) 速い。

フロー:

1. 現在表示中の `App::items` から画像 / ZipImage 系の path 集合を抽出
   (`Vec<PathBuf>`, 順序保持)
   - **重要**: この path は fts_meta.db に登録された正規化形式
     (lowercase + `/` + `<zippath>\u{1F}<entry>`、separator は
     `search_norm::ZIP_ENTRY_SEP`) に**必ず揃える**。生データのまま
     IN 句に入れると大小文字 / 区切りで空振りする
2. `fts_meta.db` から一括 SELECT: target で列を絞って `WHERE path IN (?,?,...)`
3. 取得した (path, text) に `search_query::matches(ast, text)` 評価
4. 合格 path を `search_filter: HashSet<usize>` に反映 (既存 filter 方式)
5. **未登録 path (fts_meta.db にない)**: `auto_index_metadata=false` または
   indexing 進行中で未処理のもの。現行のオンデマンド検索 (PNG tEXt / EXIF / XMP を
   都度読む) で fallback。worker 経由で後追い反映 (既存 `SearchPending` 踏襲)
6. **NOT-only 許可**: 対象が表示中 item のみなので SQLite 直接方式で現実的な負荷

### 7.3 Streaming 設計 (Ctrl+G 専用)

**理由**: Ctrl+G の post-filter はページング取得なので、10,000 件 HARD_MAX まで
走らせると合計 2〜5 秒かかる。streaming なしだと「結果画面が 2〜5 秒空のまま」→ UX 悪。
Ctrl+F は対象件数が小さいので batch で良い。Ctrl+S も SQLite LIKE で瞬時。

```rust
// src/global_search.rs
pub enum SearchStreamEvent {
    Batch {
        hits: Vec<GlobalHit>,
        scanned_candidates: usize,  // 累計 Tantivy 候補数 (進捗表示用)
        valid_hits: usize,          // 累計 post-filter 通過数
    },
    Done { truncated: bool, reason: DoneReason },
    Error(String),
}

pub enum DoneReason {
    Complete,          // 候補を使い切った
    TruncatedAtMax,    // HARD_MAX 到達
    Cancelled,
}
```

ワーカー側ループ:

```rust
let searcher = fts_index.searcher();  // snapshot 固定
loop {
    if cancel.load(Relaxed) { /* Cancelled */ break; }

    let page = searcher.search(&query, &TopDocs::with_limit(PAGE_SIZE).and_offset(offset));
    if page.is_empty() { /* Complete */ break; }

    let texts = fts_meta.lookup_norms_for_targets(&page_paths, target);
    let mut batch = Vec::new();
    for (hit, text) in page.iter().zip(texts.iter()) {
        if search_query::matches_with_mode(&ast, text, mode) {
            batch.push(GlobalHit::from(hit));
            valid_hits += 1;
            if valid_hits >= HARD_MAX { /* TruncatedAtMax */ break 'outer; }
        }
    }
    tx.send(SearchStreamEvent::Batch { hits: batch, ... });
    offset += PAGE_SIZE;
}
```

UI 側 (毎フレーム):

```rust
fn poll_global_search(&mut self, ctx: &egui::Context) {
    const MAX_EVENTS_PER_FRAME: usize = 8;
    let mut n = 0;
    while n < MAX_EVENTS_PER_FRAME {
        match pending.rx.try_recv() {
            Ok(Batch { hits, .. }) => {
                self.global_search_state.merge_hits(hits);
                self.rebuild_global_search_items();  // items + thumbnails をセット拡張
                n += 1;
            }
            Ok(Done { truncated, .. }) => { pending.done = true; break; }
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => { pending.done = true; break; }
        }
    }
    if !pending.done { ctx.request_repaint(); }
}
```

`items + thumbnails` の同期拡張は `push_grid_item_pending(item)` ヘルパで必ず
セットで push する (既存コードが `items.len() == thumbnails.len()` 不変を仮定)。

UX 配慮:

- **進捗バッジ**: 検索バー右に `〔現在 N 件 (候補 M 件走査中)〕` リアルタイム表示
- **スクロール安定化**: 新結果は下に append (top にいるなら気にならない)。
  スクロール中は `scroll_offset_y` を触らず `total_h` 拡大のみ
- **チラつき防止**: Aggregated view のコンテナ順序は `hit_count` 降順。新バッチで
  hit_count が増えても並び順を頻繁に入替えるとチラつくので、**1 秒ごとに並び替え**
- **キャンセル**: 新入力 / 結果ビュー閉じ / folder 遷移で `cancel.store(true)` +
  新 worker spawn (既存 `execute_*` と同じ 3 箇所パターン)

---

## 8. マイグレーション方針

### 8.1 DB スキーマ互換

- `INDEX_VERSION` を bump したら、`fts_meta::needs_rebuild` が検出して `DROP TABLE
  files` → 新スキーマで再作成。`fts_index/` 側は `index_is_stale` で検出 →
  `remove_dir_all` + `Index::create_in_dir`
- ユーザは何もしなくて良い。次回起動時にバックグラウンド再構築が走る (10 万 SSD で
  15〜30 分)。`auto_index_metadata=true` の お気に入りのみ対象
- 失敗時 (SQLite open エラー / Windows `Access Denied`) はログを出して続行。
  `open_at` が Err なら `IndexerManager::new` が None を返し Ctrl+G 無効化 (他機能は動く)

### 8.2 既存 search_index.db (Ctrl+S)

スキーマ互換で引き継ぐ (`indexed_by_auto=0` で保持)。`cache_creator` で作成済みの
サムネイルキャッシュ (`catalog/<hash>/*.db`) も継続利用。

### 8.3 FavoriteEntry

- 既存お気に入りに `id: Uuid` が欠けていたら起動時に v4 発行 + `settings.save()` で書き戻し
- 3 フラグの既定値は `false`
- 過去に手動キャッシュ / インデックス作成済みのお気に入りを自動判別して
  `auto_index_structure=true` に寄せる提案ダイアログを出す救済フロー

---

## 9. 性能目安

### 9.1 インデックスサイズ (プロトタイプ実測)

10 万ファイル × 平均メタ 2 KB = 200 MB の元テキスト想定。

- **Tantivy `fts_index/`**: 実測 raw テキスト × **0.07〜0.08x**
  - 10 万: 約 15 MB
  - 50 万: 約 70 MB
  - bigram postings 圧縮が効き、元テキストよりずっと小さい
- **`fts_meta.db`** (SQLite):
  - メタ行: 10 万 × 約 200 bytes ≈ 20 MB
  - ソース別 normalized 全文 6 列: 10 万 × 約 2 KB ≈ 実測 約 397 MB (SQLite オーバーヘッドで 2x 弱)
  - 10 万: 約 397 MB / 50 万: 約 1984 MB
  - WAL は commit 時に数十 MB 膨張
- **`search_index.db`** (構造): 10 万エントリ × 400 bytes ≈ 40 MB

**合計**:

- 10 万: 約 **450 MB** (Tantivy 15 + SQLite 397 + 構造 40)
- 50 万: 約 **2100 MB** (Tantivy 70 + SQLite 1984 + 構造 200)

`%APPDATA%` 配下なので exe サイズには影響しない。SQLite 側が支配的。メタ索引を
お気に入り単位で OFF にすれば対象外になる。

### 9.2 初期インデックス時間

画像 1 枚あたりのメタ抽出 + Tantivy 書き込み:

- 小 PNG (AI 画像): 5〜15 ms
- JPEG + EXIF: 3〜8 ms
- ZIP 内画像: 2〜5 ms + ZIP open 5〜50 ms (エントリ一括展開なら 1 回のみ)

**10 万ファイル: SSD で 15〜30 分、HDD で 1〜2 時間** (バックグラウンド処理なので許容)。

### 9.3 検索クエリ (プロトタイプ実測、50 万件規模)

| クエリタイプ | total 時間 | 備考 |
| --- | --- | --- |
| 稀な語 (一致少数) | 3〜12 ms | `unique_id`, `rare_jp` |
| 典型的 AND (3 語) | 10〜15 ms | `夕焼け 海辺 紅葉` |
| HARD_MAX 到達 (10,000 件) | 50〜160 ms | `photo`, `カメラ` 等の超汎用 bigram |
| NOT-only (Ctrl+G) | — | UI で早期 return |

- 最初のページ (streaming 開始) は **5 ms 以内** に返るので体感良好
- offset 肥大時の penalty は first page 比 **2x 以内** (custom Collector に切り替える
  必要はない)
- SQLite post-filter 500 件バッチ lookup: **9 ms** (50 万件規模でも)

### 9.4 インデックス規模の想定上限

v1: お気に入り合計 **50 万ファイル**。超える可能性のあるユーザには「お気に入り
単位のメタ索引 OFF」を案内。100 万超は `fts_index/` を favorite ごとに分割する
拡張を v2 で検討。

---

## 10. UI 応答性チェックリスト (新機能追加前)

[docs/ui-responsiveness.md §4](../../ui-responsiveness.md) のチェックリストを満たすこと。
検索経路で注意するのは:

- **Tantivy `search()`** → 必ず別スレッド (global_search::run)
- **`fts_meta.db` 全件 SELECT** → 別スレッドで warm up
- **`notify::Watcher::new()`** → supervisor spawn 時のみ
- **`rusqlite::Connection::open`** → インデクサ初回起動時のみ、別スレッドで warm up
- **`std::fs::read_dir`** → Walker スレッド内のみ、UI から呼ばない

新規にチャネル / アトミックを追加するなら [../../async-architecture.md](../../async-architecture.md)
のワーカー表 / 共有アトミック表に反映する。

---

## 11. テスト方針

### 11.1 単体テスト

- `fts_meta.db` 差分検出: FS 状態と DB 状態のモックで 3-way diff の分類を確認
- bigram クエリパーサ: 1 文字 → 空、2 文字 OK、phrase / NOT の組み合わせ
- 偽陽性フィルタ: bigram が当たったが原文にない ケースの post-filter 除外

### 11.2 統合テスト

- [tests/search_metadata_e2e.rs](../tests/search_metadata_e2e.rs) — メタ索引 E2E
  (PNG tEXt → ingest → Tantivy → post-filter、notify-rs での追加 / 削除 / rename 追従)
- [tests/search_name_e2e.rs](../tests/search_name_e2e.rs) — 名前索引 E2E
  (初期バルク + watcher + 複数 supervisor の真並列)
- [tests/common/mod.rs](../tests/common/mod.rs) — `FixtureRoot` / `start_indexer_at` /
  `wait_for_search_hits` 等ハーネス
- `src/app.rs::phase_c_key_tests` — 検索バーの相互排他

進行中の Phase C (フルスタック egui_kittest ハーネス) は
[../../search-test-plan.md](../../search-test-plan.md) を参照。

### 11.3 性能測定

- `src/bin/bench_search.rs` (想定) — Corpus 10 万件でインデックス時間 + クエリ時間を計測
- `perf::event` を ingest / walker / global_search に差し込み、
  `--perf-log` + `scripts/analyze_perf.py` で悪化検知

---

## 12. 参考資料

- Tantivy: [quickwit-oss/tantivy](https://github.com/quickwit-oss/tantivy) (MIT)
- NgramTokenizer: [docs.rs NgramTokenizer](https://docs.rs/tantivy/latest/tantivy/tokenizer/struct.NgramTokenizer.html)
- 形態素解析 (採用せず): [lindera-morphology/lindera-tantivy](https://github.com/lindera-morphology/lindera-tantivy) (MIT)
- CJK 対応 trigram (保険): [streetwriters/sqlite-better-trigram](https://github.com/streetwriters/sqlite-better-trigram) (MIT)
- ファイル監視: [notify-rs/notify](https://github.com/notify-rs/notify)
- SQLite FTS5 trigram 性能議論: [SQLite forum post](https://sqlite.org/forum/info/3e9352773af9e7d5f6de80532affaadd2429eef107f3f811bdbb8f38cd953dc3)
- プロトタイプ計測: [search-bench-results.md](search-bench-results.md)

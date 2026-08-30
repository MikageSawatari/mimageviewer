# 外部メタデータ サイドカー取り込み 設計ドキュメント

画像ファイルと **同名のサイドカーファイル** (`<画像名>.json` / `<画像名>.txt`) に保存された
メタデータを mIV が読み取り、

1. **全文検索インデックスに取り込む** (Ctrl+G で検索可能にする)
2. **右メタデータパネルに表示する** (JSON は整形して見やすく)

ための機能を追加する。

> **命名ポリシー (重要)**
> 本機能は特定の外部取得ツール・特定の画像投稿サイトを対象にしたものではなく、
> 「画像の隣に同名のメタデータファイルを置く」という一般的な慣習に対応する **汎用機能** である。
> CLAUDE.md の恒久ポリシーに従い、ユーザー向け文書・UI・コミットメッセージ・本ドキュメント本文に
> **特定の外部ツール名・画像投稿サイト名を一切記載しない**。汎用的な JSON キー名 (`tags`, `source` 等)
> を例示に使うのは可だが、特定サイト固有のスキーマ (カテゴリ別タグキー等) に依存した実装はしない (§5)。

> **本機能と mIV タグ機能の関係 (重要)**
> 本機能は **読み取り専用のフリーテキスト検索**であり、mIV の「タグ機能」([archive/search-metadata/tag-feature.md](archive/search-metadata/tag-feature.md))
> とは **別系統で明確に分離**する。
> - **mIV タグ** … 利用者が付与・編集する `#xxx` 要素。正本は `tags.db` で、**メディア本体や XMP は
>   書き換えない** ([tag-catalog-redesign-plan.md](tag-catalog-redesign-plan.md))。タグビュー (Ctrl+T) /
>   facet タグフィルタから絞り込む。追加 / 全消去などの編集対象。
> - **サイドカー** … 外部ツールが出力した JSON/TXT の **値**を、mIV は一切書き換えず読み取って
>   自由語検索可能にするだけ。タグとしては扱わない (`#` も付けない)。
>
> サイドカーからタグ的キー (`tags` 等) を抜き出して mIV タグ体系へ合流させる案は **採らない** (§5 / §12)。
> 理由: (a) 特定の出力スキーマ (カテゴリ別タグキー等) に依存すると特定サイト特化になりかねない、
> (b) 値を丸ごと自由語検索可能にすれば `1girl` 等で十分絞り込める、(c) 編集可能な mIV タグと
> 読み取り専用サイドカーが構造的に混ざらず、利用者の理解も明快。

---

## 1. 背景 / 動機

mIV の既存検索は **画像本体に埋め込まれた XMP / EXIF / PNG メタ** を索引する
(`docs/search-architecture.md`, `ingest_text.rs`)。一方、画像を一括取得する各種ツールは、取得時に
**画像本体を書き換えず、隣に同名のメタデータファイルを出力する** 運用が一般的である。これらのツールには
**画像本体へ EXIF/XMP を埋め込むネイティブ機能がなく**、メタデータはサイドカー (JSON / プレーンテキスト)
としてのみ残る。

そのため、mIV が埋め込みメタしか見ないと、こうしたサイドカー付き画像ライブラリでは作者・元 URL・
各種ラベル等の豊富な情報が検索にも表示にも一切活用されない。本機能でサイドカーを **読み取り専用の
検索ソース** として扱えるようにすることで、**外部ツールに追加処理 (メタデータ埋め込みツール等) を
導入していない一般利用者でも、取得時のメタデータをそのまま mIV の検索・閲覧に活かせる**ようにする。

---

## 2. スコープ

### 対象
- **JSON サイドカー** … `<画像名>.json` (例: `0001_<hash>.jpg.json`)。構造化された key/value。配列・ネストあり。
- **テキストサイドカー** … `<画像名>.txt` (例: `0001_<hash>.jpg.txt`)。プレーンテキスト。

いずれも **画像と同じディレクトリにある同名サイドカー**。検出規則は §4。

### 非対象 (v1)
- **ZIP 内画像 / PDF ページ** … サイドカーはアーカイブ外のファイルシステム上に存在し得ず、紐付けが曖昧に
  なるため対象外 (タグ機能と同じ判断: `docs/archive/search-metadata/tag-feature.md` §1)。
- **動画** … 動画は既に同名 `.xmp` サイドカーを mIV タグの一次ソースとして扱っている
  (`ingest_text.rs` の video 経路)。JSON/TXT サイドカーは v1 では読まない (既存挙動を維持)。
- **サイドカーへの書き込み** … 本機能は **読み取り専用**。mIV がサイドカーを生成・更新することはない。
- **タグ抽出** … サイドカーの内容を mIV タグ (`#xxx`) として扱うことはしない (冒頭の関係性ノート / §5)。

### 非機能要件
- **画像本体・サイドカーを一切変更しない** (読み取りのみ)。
- インデクサ worker をブロックしない / ディスク I/O を増やしすぎない (§7, §10)。
- サイドカーが壊れている・巨大・文字コード不正でも **黙って握り潰してファイル名検索等は継続**
  (既存 `build_per_source_for_file` の「失敗部分は空文字列」方針を踏襲)。
- **専用の ON/OFF 設定は持たない**。お気に入りのアイテム索引 (`auto_index_metadata`) に追従する (§9)。

---

## 3. ユーザーシナリオ

1. 利用者が外部ツールで画像 + サイドカー (`<画像名>.json` 等) をフォルダへ保存する。
2. mIV でそのフォルダを開く → インデクサが画像ごとにサイドカーを検出・パースし、値を `sidecar_text`
   列として索引に投入する。
3. **検索**: Ctrl+G で自由語 (作者名・作品名・ラベル・元 URL の一部 等。例: `1girl` / `karon-t` /
   `143512783`) で絞り込める。検索対象プルダウンで「サイドカー」のみに限定も可能。
4. **閲覧**: 右パネルに「外部メタデータ」セクションが追加され、JSON は key/value ツリーで整形表示、
   TXT はテキストとして表示される。

> mIV 独自タグ (`#xxx`) は従来どおり「対象=タグ」で絞り込む。サイドカーの値とは **別物**として並存する。

---

## 4. サイドカー検出規則

画像 `IMG` (= `dir/stem.ext`、例 `dir/0001_<hash>.jpg`) に対し、以下を順に存在チェックし、
**最初に見つかった 1 ファイルだけ**を紐付ける (優先順位プローブ):

| 優先 | パターン      | 例                     | 備考 |
| ---- | ------------- | ---------------------- | ---- |
| 1    | `<full>.json` | `0001_<hash>.jpg.json` | 拡張子込みファイル名 + `.json` (一般的な出力形式) |
| 2    | `<full>.txt`  | `0001_<hash>.jpg.txt`  | 拡張子込み + `.txt` |
| 3    | `<stem>.json` | `0001_<hash>.json`     | 拡張子を置換する流儀のツール向け |
| 4    | `<stem>.txt`  | `0001_<hash>.txt`      | 同上 |

- exact 名 (`<full>`) を stem 形式より優先。各内で JSON を TXT より優先 (情報量が多い)。
- **最初に存在した 1 つだけを採用**。複数あっても残りは読まない (重複・コスト回避)。
- 探索は `path.with_file_name(...)` ベースの **存在チェックのみ** (ディレクトリ走査はしない)。
- **衝突 (`<full>` 不在で `<stem>` 形式のみ) の扱い**: `dir/foo.jpg` と `dir/foo.png` が両方あり
  `dir/foo.json` (stem 形式) が 1 つだけのとき、両画像が同じ `foo.json` を読む。これは **許容**
  (実害小・稀ケース)。「同じ stem の画像が 1 つだけか」を確かめるフォルダ走査ガードは **設けない**
  (per-file の存在チェックのみで完結させ、ingest を単純・低コストに保つ)。

`.json` / `.txt` ファイル自体はアイテム索引の候補にならない (walker の拡張子分類が画像 / 動画 / PDF
以外を除外する。`search_walker.rs`)。サイドカーがアイテムとして誤索引される心配はない。

---

## 5. データモデル / 抽出ルール

サイドカーから **1 種類の検索テキスト (`sidecar_text`)** を作る。**タグ抽出・タグ列合流は行わない。**

### 5.1 JSON

JSON を再帰走査し、**リーフ値 (文字列 / 数値 / bool) のみ**をフラットに連結して `normalize_for_match`
し、新設の `sidecar_text` フィールドへ入れる。これは mIV が既に **EXIF の全タグ値を 1 個の `exif_text`
blob にまとめて索引している** のと全く同じ発想 (`ingest_text.rs` の `append_exif`)。

- **キー名は索引に含めない** (確定 §12)。`source` / `rating` のようなキー名を含めると、キー名自体が
  全サイドカー画像にヒットしてノイズになる (例: `source` という語で全サイドカー画像がヒット) ため。
  値のみを索引する。
- 配列もリーフ値を順に収集。ネスト dict も再帰。
- 巨大配列・長大文字列は上限でカット (§10)。

**具体例**: 次の JSON サイドカー (一般的な取得ツール出力の例) があるとき —

```json
{ "id": 11457572, "rating": "g", "score": 0,
  "tags": ["1girl", "blue_eyes", "twintails"],
  "artist": "karon-t",
  "character": "nonoyama_rui",
  "source": "https://example.invalid/img/143512783_p0.jpg",
  "image_width": 1000, "image_height": 1400 }
```

`sidecar_text` (正規化後) は概ね次のようになる:

```
11457572 g 0 1girl blue_eyes twintails karon-t nonoyama_rui https://example.invalid/img/143512783_p0.jpg 1000 1400
```

→ ラベル (`1girl`)・作者名 (`karon-t`)・キャラ名 (`nonoyama`)・元 URL・寸法・スコアまで、まとめて
**自由語検索**できる。`#1girl` のような `#` プレフィックス形では検索しない (`#` は mIV タグ専用の構文)。
`1girl` でヒットする。`source` (キー名) では当たらない (値のみ索引するため)。

### 5.2 TXT

TXT は全文を `normalize_for_match` して `sidecar_text` へ。1 行 1 ラベルでもプレーン文でも、行内の語が
自由語検索で当たる。タグ列扱いはしない (`sidecar_text` 1 本に集約)。

---

## 6. 検索インデックス統合 (`fts_index.rs` / `ingest_text.rs` / `fts_meta.rs`)

### 6.1 スキーマ変更

`INDEX_VERSION` を **7 → 8** に上げる (`fts_meta.rs` の定数。再インデックスを強制)。
Tantivy スキーマに 1 フィールド追加:

```text
sidecar_text   TEXT  bigram | STORED   外部メタデータサイドカー (JSON/TXT) の値テキスト
```

- `SourceKind` (`fts_index.rs`) に `Sidecar` を追加し、`ALL` 配列にも加える。
  `Fields` / `text_field_for` / `build_schema` / `upsert_doc` / `doc_per_source_text` を更新。
- **`schema_is_stale` に `sidecar_text` の存在 + STORED チェックを追加する (実装必須)**。
  Tantivy のスキーマ陳腐化判定はフィールド存在 + STORED 属性で行っており (`INDEX_VERSION` 定数とは
  独立)、`Fields::from_schema` は `get_field("sidecar_text").expect(...)` で取り出す。`schema_is_stale`
  を更新し忘れると、旧 v7 index を開いた瞬間に **起動時 panic** する。
- `fts_meta.db` 側は管理メタ専用 (検索原文は Tantivy STORED に集約済み) なので **列追加は不要**。
  `INDEX_VERSION` bump だけで `needs_rebuild` が全再構築を促す。索引は FS から再生成可能な
  **派生キャッシュ**なので、個別のデータ移行コードは不要 (既存の rebuild 機構で吸収される)。

### 6.2 取り込み (`ingest_text.rs`)

`PerSourceText` に `pub sidecar: String` を 1 本追加 (`get` / `combined` / `is_empty_all` も更新)。
`build_per_source_for_file` 末尾に処理を追加:

```text
// 5. 外部メタデータサイドカー (画像のみ。動画は既存 .xmp 経路を維持)
if !is_video_sidecar {
    if let Some(text) = read_sidecar_text_for_image(path) {  // §4 検出 + §5 抽出
        out.sidecar = normalize_for_match(&text);
    }
}
```

`ingest_worker.rs` は `build_doc_for_image` 経由で `build_per_source_for_file` を呼ぶので、自動的に
`sidecar_text` が `IndexDoc` に載る (worker 側の追加変更はほぼ不要)。**mIV タグ (`tags` フィールド) には
一切触れない** — サイドカーは `sidecar_text` 専属。`tag_write_worker` も無改造 (タグ編集はサイドカーに
影響しない / サイドカーは編集に影響されない)。

### 6.3 検索 UX

- 既定 (`SearchTarget::All`) では `sidecar_text` も OR 対象に含まれ、追加実装なしで自由語検索が効く。
- **検索対象フィルタ UI に「サイドカー」トグルを追加する** (`global_search_ui.rs` の `TargetChoice` /
  `TARGET_CHOICES`)。`SearchTarget::Only([Sidecar])` で「サイドカーのみ」絞り込みを可能にする。
- 「タグ」(`Only([Tags])`) は従来どおり mIV の `#` タグ専用。サイドカーは含めない (分離)。

---

## 7. 取り込みトリガ / フォルダ監視

サイドカーは画像本体と別ファイルなので、「画像は変わらずサイドカーだけ後から追加 / 更新」された
ケースを取りこぼさないよう、以下で対応する。**手動再構築 UI は現状無い** (`favorites_editor.rs` に
記載のとおり v0.8.0 で撤去) ため、自動追従を必須とする。

- **実効 mtime 方式**: walker (`search_walker.rs`) の 3-way diff で、画像候補の mtime を
  `max(画像 mtime, サイドカー mtime)` にする。サイドカーを編集すると実効 mtime が変わり「変化あり →
  再 ingest」と判定される。サイドカー mtime 取得は該当画像 1 件あたり 1 stat (同一ディレクトリの
  `read_dir` 結果から §4 命名規則でサイドカー名を引いて `metadata()` を取る)。
- **ファイル監視**: `search_watcher.rs` の監視対象拡張子に `.json` / `.txt` を加え、サイドカー変更
  イベント時に **対応する画像 (§4 命名規則の逆引き) の再 ingest** を要求する。
- **初回フォルダオープン時**: 通常の一括 ingest で各画像のサイドカーを読む (追加コストは §10)。

---

## 8. 既存メタソースとの関係

| 対象 | 検索ソース |
| ---- | ---------- |
| 画像 (jpg/png/webp 等) | ファイル名 / EXIF / XMP / PNG プロンプト / **mIV タグ (`tags`, `#xxx`)** / **サイドカー (`sidecar_text`, 本機能)** |
| 動画 | ファイル名 / コンテナメタ / `.xmp` サイドカーの mIV タグ (JSON/TXT は v1 対象外) |
| ZIP 内 / PDF ページ | 対象外 |

`sidecar_text` は EXIF / PNG / タグ等の既存ソースと **独立に並存**。mIV タグ (編集可能・`#`・タグ専用
プルダウン) とサイドカー (読み取り専用・自由語) は **別フィールド・別ターゲット**で分離する。

---

## 9. 設定

**専用の設定項目は追加しない** (確定)。サイドカー取り込みは **アイテム索引 (`auto_index_metadata`) の
一部**として扱う:

- **検索取り込み**: お気に入りに追加し、そのアイテム索引が ON のときだけサイドカーを読んで索引する。
  アイテム索引を OFF にすると、その favorite の全 doc が即削除される既存挙動 (`purge_favorite_metadata`、
  `app.rs` の `apply_favorite_meta_index_change`) に乗って `sidecar_text` も一緒に消える。専用フラグ・
  消去処理は不要。
- **右パネル表示**: 索引とは独立。フルスクリーンで画像を開いたとき、サイドカーがあれば worker が読んで
  表示する (索引 ON/OFF と無関係)。

> ニッチ需要 (アイテム索引は使いたいがサイドカー値索引のノイズだけ切りたい) 向けの安全弁トグルは
> v1 では設けない。余計なサイドカーが存在する可能性は低く、あっても検索対象フィルタで絞れるため。
> 必要になれば後から追加可能。

---

## 10. エッジケース / 非機能

- **巨大ファイル**: サイドカーのサイズ上限 **2 MB** を設け、超過時はスキップ (ログ 1 行)。暴走ファイル対策。
- **壊れた JSON / 不正文字コード**: パース失敗は空に倒し、ログ 1 行。検索の他ソースは継続。
  JSON は UTF-8 前提 (仕様上 UTF-8 固定)。
- **巨大配列 / 長大文字列**: 値連結の総量に上限を設ける (例: `sidecar_text` 連結後 256 KB 程度で
  打ち切り)。bigram 索引の肥大・誤ヒットノイズを防ぐ。
- **検出コスト**: 画像 1 件あたり最大 4 回の存在チェック + サイドカー 1 件の stat。インデクサは
  background (Low 優先・`GlobalIoSemaphore` gated) なので UI 操作には影響しない。
- **再インデックス**: `INDEX_VERSION` 7→8 引き上げにより全再構築が走る。更新後初回オープンが重くなる旨を
  リリースノートに記載 (⚠️ 対象)。

---

## 11. 右パネル表示 (`ui_metadata_panel.rs`)

既存のパネルセクション順 (タグ → ツイート情報 → AI メタ → EXIF) の中に **「外部メタデータ」セクション**を
追加する (位置は EXIF の前後あたり、折りたたみ可)。**画像 (FS) のみ**対象 (ZIP 内画像 / PDF ページ /
動画は出さない)。

- **JSON**: **汎用 key/value ツリー表示**。
  - 全キーを「キー: 値」で表示、ネストはインデント、配列は件数省略 + 展開。
  - **特定スキーマの代表フィールド (作者 / 作品 / レーティング 等) をハードコードして上部にピックアップ
    する処理は設けない** (確定 §12)。特定ツール / サイトのキー名に依存しないため。どんな JSON でも
    同一ロジックで表示する。
  - **キー名は表示する** (可読性のため)。これは検索 (`sidecar_text` は値のみ) とは別レイヤーであり、
    矛盾しない。
  - 配列は毎フレーム全件 join / widget 化すると重い (最大 2MB 許容) ため、先頭 100 件までに
    制限して残りは件数表示にする。
  - HTTP(S) URL を含む値は、動画メタデータパネルと同じくクリック可能リンクとして表示する。
  - 「生 JSON (整形)」トグルは設けない (数行の小窓スクロールで実用性が低く、毎フレームの
    pretty-print コストもかかるため。key/value ツリーで十分)。
- **TXT**: テキストをそのまま表示する。HTTP(S) URL はクリック可能リンクにし、スクロールは右パネル全体の
  `metadata_scroll` に統一する (TXT 部分だけの入れ子 ScrollArea は作らない)。
- **読み込み**: パネル描画時に都度読むと重いので、選択画像切替時に worker (`run_metadata_load`) で
  1 度読んでキャッシュする (`MetadataLoadResult` に sidecar を追加、`poll_metadata_load` で新
  `sidecar_cache` に投入)。既存 EXIF/AI メタのキャッシュ機構に合わせる。**UI スレッドで同期読みしない**
  (CLAUDE.md / `docs/ui-responsiveness.md`)。
- **UI スナップショット**: パネル UI を変更するため `tests/snapshots/*.png` の更新が必要
  (`docs/ui-snapshot-policy.md`)。

---

## 12. 実装フェーズ (段階導入)

1. **Phase 1 — 検索取り込み (バックエンド)**
   - `src/external_metadata.rs` (新規): §4 検出 (優先順位プローブ) + §5 抽出 (JSON 値連結 / TXT 全文)。
     サイズ上限・壊れ JSON 握り潰し。`serde_json` 使用 (依存済み)。
     **注: `src/sidecar.rs` は既存 (`mimageviewer.dat` バックアップ用、`main.rs` で公開済み) なので
     その名前は使わない** (§14-1)。
   - `fts_index.rs`: `sidecar_text` フィールド、`SourceKind::Sidecar` + `ALL`、`Fields` /
     `text_field_for` / `build_schema` / `upsert_doc` / `doc_per_source_text` / **`schema_is_stale`**。
   - `fts_meta.rs`: `INDEX_VERSION` 7→8。
   - `ingest_text.rs`: `PerSourceText.sidecar` 追加、`build_per_source_for_file` step 5。
   - `search_walker.rs`: 実効 mtime (`max(画像, サイドカー)`)。
   - `search_watcher.rs`: `.json`/`.txt` 監視 → 対応画像の再 ingest。
   - 単体テスト: JSON 値抽出 (キー名除外確認) / TXT / 命名優先順位 / 実効 mtime 差分 /
     `1girl` ヒット・`source` (キー名) 非ヒット。
2. **Phase 2 — 検索 UX**
   - `global_search_ui.rs`: 「サイドカー」ターゲット追加 (`TargetChoice` / `TARGET_CHOICES`)。
3. **Phase 3 — 右パネル表示**
   - `app.rs`: `MetadataLoadResult` に sidecar、`run_metadata_load` / `poll_metadata_load` /
     `sidecar_cache`。
   - `ui_metadata_panel.rs`: 「外部メタデータ」節 (汎用ツリー)、スナップショット更新。
4. **Phase 4 — ドキュメント**
   - 専用設定は追加しない (§9)。
   - `htdocs/.../manual/` / `index.html` / `spec.md` / `search-architecture.md` / README 更新履歴
     (いずれも汎用表現で、特定ツール / サイト名を出さない)。リリースノートに再索引注記。

### 確定事項

- **Q1 再取込トリガ → 実効 mtime + 監視。** §7。画像 mtime を `max(画像, サイドカー)` にし、`.json`/`.txt`
  監視で後追い再 ingest。手動再構築 UI が無いため自動追従を必須とする。
- **Q2 タグの扱い → サイドカーはタグ抽出しない。値のみ自由語検索。** §5 / §8。mIV タグ (`#xxx`, 編集可,
  タグ専用プルダウン) と完全分離。`#` 表現・`tags` 列合流・専用タグフィールドは設けない。理由: 特定
  サイトスキーマ非依存 + 構造の単純さ + 編集可否の理解の明快さ。
- **Q3 命名 → `<full>` / `<stem>` 両形式を優先順位プローブ、最初の 1 つを採用。** §4。「同 stem 画像が
  1 つだけか」のフォルダ走査ガードは設けない (実害小)。
- **Q4 パネル表示 → 汎用 key/value ツリー。代表フィールドのハードコード無し。** §11。検索は値のみ、
  表示はキー名も出す。
- **Q5 専用設定 → 持たない。** §9。サイドカー取り込みはアイテム索引 (`auto_index_metadata`) に追従。
  OFF 化の消去は既存 `purge_favorite_metadata` に乗る。安全弁トグルも v1 では設けない。
- **(実装必須) `schema_is_stale` 更新。** §6.1。忘れると旧 index 読込で起動 panic。

---

## 13. 参照

- 既存タグ機能 (別系統): `docs/archive/search-metadata/tag-feature.md`
- 検索アーキテクチャ: `docs/search-architecture.md`
- 索引スキーマ: `src/fts_index.rs` (`SourceKind`, `SearchTarget`, `schema_is_stale`) / `INDEX_VERSION` は `src/fts_meta.rs`
- 取り込み: `src/ingest_text.rs` (`build_per_source_for_file`)
- 取り込み worker: `src/ingest_worker.rs`
- 差分走査: `src/search_walker.rs` (実効 mtime)
- 監視: `src/search_watcher.rs` (`.json`/`.txt` 追加)
- 右パネル: `src/ui_metadata_panel.rs` / メタ読み込み worker は `src/app.rs` (`run_metadata_load`)
- 命名 / 表記ポリシー: `CLAUDE.md`「外部ダウンローダの言及禁止ポリシー」

---

## 14. 実装上の確定追補 (Codex レビュー反映)

実装プランを Codex (read-only) にレビューさせ、実コードと突き合わせて確認した補正。以下は §1〜§13 の
記述に優先する **確定事項**。

1. **モジュール名の衝突回避 (P1)**: `src/sidecar.rs` は既存 (`mimageviewer.dat` バックアップ系、
   `main.rs` で `pub mod sidecar` 済み)。新モジュールは **`src/external_metadata.rs`** とする
   (既存 backup-sidecar の意味を一切変えない)。

2. **監視マッピングは supervisor 側に置く (P1)**: `search_watcher.rs` は notify ラッパ + debounce で
   生パスを流すだけ。`.json`/`.txt` → 画像の逆引きと再 ingest 判断は
   **`indexer_supervisor::apply_single_change`** (`build_candidate_from_path` の手前) に実装する。
   現状の unsupported 拡張子 fallback はサイドカーパス自体を delete して画像を再 ingest しないので、
   サイドカーイベントを受けたら §4 命名規則の逆引きで対応画像を求めて再 ingest 要求に変換する。
   notify の rename は `From`/`To`/`Both` に分かれて届く (search_watcher の `absorb_event`) ので、
   旧名・新名の双方について影響画像を再 ingest する。

3. **削除・古いサイドカー・優先順位切替の差分検出 (P1 + Codex 実装 P3)**: 実効 mtime =
   `max(画像, サイドカー)` **だけでは不十分**。サイドカー mtime が画像 mtime 以下のとき、サイドカーを
   削除/編集しても mtime 差分が「変化なし」に見え、stale な `sidecar_text` が索引に残る。さらに
   `a.jpg.json` (優先1) 消失 → 同 mtime/size の `a.json` (優先3) に切替わったケースも mtime/size だけでは
   検出できない。対策: **3-way diff の比較シグネチャ (`CandidateFile.diff_mtime` / `diff_size`) に
   サイドカーの mtime + fingerprint を織り込む**。
   - `diff_mtime = max(画像 mtime, サイドカー mtime)` (編集を検出)
   - `diff_size = 画像 size + サイドカー fingerprint` (サイドカー無しは 0)。fingerprint は
     `external_metadata::sidecar_signature` が返す **選択サイドカーのファイル名 + size の安定ハッシュ**
     (常に 1 以上)。これで「追加 / 編集 / 削除」に加え「優先順位プローブの結果が別ファイルに変わった」
     ケースも (同 mtime/size でも) 検出できる。
   - `diff_mtime` / `diff_size` は **差分判定専用トークン**で fts_meta にのみ保存。Tantivy doc の
     `mtime` は画像本体の mtime のままなので、日付ソートはサイドカー編集時刻に引きずられない (§14-4)。
   回帰テスト: `search_walker::tests::sidecar_removal_re_ingests_image` (削除 → 再 ingest) /
   `sidecar_priority_switch_same_size_re_ingests` (同 size の優先順位切替) /
   `external_metadata::tests::fingerprint_differs_for_same_size_priority_switch`。

4. **比較用 mtime と表示 / ソート用 mtime を分離する (P3 → 格上げ運用)**: Tantivy doc の `mtime`
   フィールドは Ctrl+G 一覧の **日付ソート**に使われる (`doc_mtime`)。ここに実効 mtime
   (サイドカー編集時刻) を入れると並びがサイドカー編集時刻基準になる。**Tantivy doc.mtime は画像本体の
   mtime を入れ**、実効 mtime / size は **`fts_meta` の差分用にのみ**使う。`CandidateFile` に
   「差分用 (effective) mtime/size」と「表示用 (image) mtime/size」を分けて持たせ、`ingest_worker` の
   `pending_ok_meta` (= fts_meta 差分) には effective を、`IndexDoc.mtime` (= Tantivy 表示) には
   image を渡す。

5. **Ctrl+F (`run_metadata_search`) もサイドカー対応が必要 (P1)**: `TargetChoice` / `TARGET_CHOICES` は
   **Ctrl+F と Ctrl+G で共有**されている (`ui_main.rs` の検索バー)。`SourceKind::Sidecar` を
   `TARGET_CHOICES` に追加すると Ctrl+F のドロップダウンにも「サイドカー」が出るが、
   `run_metadata_search` には `use_sidecar` 経路が無いため「サイドカーのみ」が無反応・「すべて」が
   サイドカーを取りこぼす。対策: `run_metadata_search` (worker スレッド) の Pass 2 に
   `use_sidecar = target.includes(Sidecar)` を追加し、**FS 画像のみ** on-demand でサイドカーを読んで
   hay に含める (既存の EXIF / XMP の on-demand 読みと同じパターン)。

6. **設定フラグは不要 (P2 → 解消)**: 専用の `sidecar_metadata_enabled` 設定を **持たない**と確定したため
   (§9)、Codex が指摘した「設定フラグを ingest 経路へスレッドする / OFF 化で既存 `sidecar_text` を消す」
   問題は **発生しない**。`build_per_source_for_file` は従来どおり path だけ受け取り、`!is_video_sidecar`
   なら常にサイドカーを読む (索引が走るのはアイテム索引 ON の favorite だけ)。アイテム索引 OFF 時の
   消去は既存の `purge_favorite_metadata` に乗る。

7. **パネルは `GridItem::Image` で明示ゲート (P2)**: `metadata_cache_key` は `GridItem::Video` も
   キーを返す (`app.rs`)。「キーがある = 画像」と見なせない。サイドカーのロード / 表示は
   **`GridItem::Image` (FS 画像) に明示限定**し、非画像で `sidecar_cache` の再 spawn が起きないようにする。

8. **`fts_meta` の `user_version == 5 => false` 特例を見直す (P3)**: `fts_meta.rs` の open 時 rebuild 判定に
   `5 => false` (rebuild 不要) の特例がある。これは v5↔v6/v7 で Tantivy schema が不変だった時代の最適化。
   v8 では新フィールドで Tantivy が wipe されるため、v5 から直接上がるユーザーが `5 => false` を踏むと
   **fts_meta は drop されず Tantivy だけ空** になり検索が壊れる恐れ。対策: **`5 => false` 特例を撤去**
   (または条件付き) し、`user_version != INDEX_VERSION` は `needs_rebuild` (MIN index_version < 8) に
   委ねて確実に full rebuild させる。

9. **テスト更新 (P2)**: `PerSourceText` に `sidecar` を足すと `fts_index.rs` の `sample_doc_with_sources`
   等が網羅構築のためコンパイルエラーになる。これらと、`build_schema_exposes_expected_fields` 系の
   スキーマフィールド assertion (`sidecar_text` を追加) を更新する。

### 確認済み (Codex)

- v7→v8 マイグレーションの形は健全: `FtsIndex::open_at` は `Fields::from_schema` の **前**に
  `schema_is_stale` を見て wipe するので、`sidecar_text` を両 stale チェックに足せば旧スキーマ panic を回避できる。
- `doc_text_for_target` / `build_bigram_and_query` はフィールド反復ベースなので、`SourceKind::ALL` +
  `text_field_for(Sidecar)` だけで検索経路に載る。
- 新 Tantivy フィールドに後方互換のデータ移行は不要 (派生データなので rebuild で足りる)。

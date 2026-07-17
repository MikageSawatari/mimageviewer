# 検索のコンテナ / アイテムモデル再設計

mimageviewer の検索 (Ctrl+F / Ctrl+S / Ctrl+G) を「コンテナ検索 / アイテム検索」という
分かりやすい概念モデルに寄せ直すための設計ドキュメント。Codex レビュー → 実装の順で進める。

関連ドキュメント: [search-architecture.md](search-architecture.md) (現行アーキテクチャの決定版)、
[search-expansion-design.md](search-expansion-design.md)、[tag-feature.md](tag-feature.md)。

---

## 1. 目的と背景

現状、3 つの検索モードはユーザーから見て挙動の差分が分かりにくい:

- Ctrl+S と Ctrl+G はどちらも「お気に入り配下の横断検索」だが、Ctrl+S は対象アイテム
  そのものの一覧、Ctrl+G はヒットを含むコンテナの一覧、と結果の見せ方が食い違う。
- Ctrl+G がフォルダを検索対象に含まない (= walker が dir を再帰するだけで索引しない)
  ことがユーザーに伝わらない。
- Ctrl+G の集約ビューは代表サムネが「コンテナごとに 1 枚」になり、タグ検索のように
  「ヒットした画像を直接サムネで見たい」用途と相性が悪い。

これを次の概念モデルに整理する:

- **Ctrl+S = コンテナ検索** — フォルダ / ZIP / PDF を **名前** で探す。
- **Ctrl+G = アイテム検索** — 画像 / PDF / 動画を **名前 + メタ情報** で探す。
- **Ctrl+F = 現在地フィルタ** — 上記とは直交するスコープ軸。今開いているグリッドを絞る。

このモデルの利点: フォルダが Ctrl+G 対象外なのも、画像が Ctrl+S 対象外なのも
「コンテナはコンテナ検索、アイテムはアイテム検索」で説明が付き、現状の挙動の大半が
"設計どおり" になる。索引アーキテクチャの大規模リファクタを伴わずに整理できる。

**方針**: アプリとして利用者に分かりやすい仕様を最優先する。後方互換は重視しない
(検索索引はすべて再構築可能なキャッシュなので、スキーマ変更・INDEX_VERSION bump・
索引全消去は自由に行ってよい。移行コードは書かない)。

---

## 2. 現状の整理

### 2.1 検索対象

| モード | 検索対象 | スコープ | バックエンド |
| --- | --- | --- | --- |
| Ctrl+F | 現グリッド表示中の画像 / 動画 / ZIP 内画像 のメタ情報 (ZIP 内画像は現状 PNG のみメタ参照あり → 再設計後はファイル名のみ §4.1.2、PDF は現状 document info 未使用 §4.1) | 現フォルダ (非再帰) | worker 上の on-demand メタ読み取り |
| Ctrl+S | フォルダ / ZIP / PDF / **動画** の名前 | お気に入り配下 (再帰) | `search_index.db` (SQLite LIKE) |
| Ctrl+G | 画像 / PDF / 動画 / ZIP ファイル(名前のみ) の ファイル名 + メタ情報 | お気に入り配下 (再帰) | Tantivy bigram + STORED 原文 post-filter |

ZIP / PDF / 動画は Ctrl+S と Ctrl+G の両方に出る。フォルダは Ctrl+S のみ、画像は Ctrl+G のみ。
**Ctrl+G の ZIP は ZIP ファイル名のみ** — ZIP 内エントリ (画像) は索引していない (§3.2)。

### 2.2 結果の見せ方

- **Ctrl+F**: 現グリッドの可視フィルタ (`search_filter: Option<HashSet<usize>>`)。
  別ビューは作らず `visible_indices` を絞るだけ。構造アイテム (Folder / ZipFile /
  PdfFile / SearchContainer) は **常にマッチ扱い** で絞り込まれない
  (`run_metadata_search` Pass 1)。
- **Ctrl+S**: `start_loading_items` で検索結果 (GridItem::Folder / ZipFile / PdfFile /
  Video) のフラットな単一リストにグリッドを置換。display_name 昇順。
- **Ctrl+G**: 2 階層。`Aggregated` (SearchContainer セルをヒット件数降順で並べる) ⇄
  `DrilledInto` (コンテナにドリルインした中身)。既定は Aggregated。最大 HARD_MAX=10000。

### 2.3 結果のフィルタ機能

| 機能 | Ctrl+F | Ctrl+S | Ctrl+G |
| --- | --- | --- | --- |
| お気に入り絞り込み | — (現フォルダ固定) | あり | あり |
| 種別フィルタ | なし | なし | あり (フォルダ/画像/ZIP/PDF/動画 のドロップダウン) |
| 検索対象 (名前/EXIF/タグ等) | あり | なし (名前のみ) | あり |
| OR モード | あり | あり | あり |
| ソート | — | display_name 昇順固定 | Aggregated のみ `ContainerSortMode` (件数/名前/新旧) |
| レーティングフィルタ | グリッド共通 (全モード適用) | 同左 | 同左 |
| タグピッカー (# タグ…) | なし | なし | あり |

---

## 3. 新しい概念モデル

```
                 コンテナ検索              アイテム検索
                 (Ctrl+S)                 (Ctrl+G)
  対象           フォルダ/ZIP/PDF          画像/PDF/動画
  探す次元       名前のみ                  ファイル名 + メタ情報
  スコープ       お気に入り配下 (再帰)      お気に入り配下 (再帰)

                 現在地フィルタ (Ctrl+F) ── 上記と直交。今開いているグリッドを絞る。
```

ユーザー向けの言い回し:「**S はフォルダ・ZIP・PDF を探す / G は画像 1 枚 1 枚を探す /
F は今見ている場所を絞る**」。

PDF は S・G の両方に出る。PDF はコンテナでもあり、それ自体が 1 アイテムでもある
→ S では名前で、G では名前 + メタ情報で見つかる。ZIP はコンテナとして Ctrl+S が
扱う (ZIP の中身検索の扱いは §3.2)。

### 3.1 索引の呼称

お気に入り登録 / 編集画面や検索バーの説明文で使う索引の呼称も、このモデルに合わせる。

| 索引 | 新呼称 | 対応する検索 | 内部フィールド (現状) |
| --- | --- | --- | --- |
| `search_index.db` | **コンテナ索引** | Ctrl+S | `FavoriteEntry.auto_index_structure` |
| `fts_index/` + `fts_meta.db` | **アイテム索引** | Ctrl+G | `FavoriteEntry.auto_index_metadata` |

- 「メタデータ索引」「メタ情報索引」という呼称は使わない。Ctrl+G はファイル名やタグも
  対象に含み、Ctrl+F は索引なしで動くため、ユーザーにとって紛らわしい。
- 説明文の中で個々の対象を列挙するとき「EXIF などのメタ情報」と書くのは問題ない。
- 対象の説明:
  - **コンテナ索引** — フォルダ / ZIP / PDF を名前で横断検索するための索引。
  - **アイテム索引** — 画像 / PDF / 動画を、ファイル名・タグ・EXIF・
    AI プロンプト等で横断検索するための索引。
- 内部識別子 (`auto_index_structure` / `auto_index_metadata` 等) の改名は本再設計の
  対象外。UI 文言・hover・説明文のみ新呼称に統一する。

### 3.2 ZIP の扱い (本再設計のスコープ)

**現状確認** (Codex P1): アイテム索引 (Tantivy) は ZIP **ファイル** を name-only doc
1 件 (`Container::Zip` / `IndexKind::Zip`、`zip_entry` 空) として登録するだけで、
**ZIP 内エントリ (画像) は索引していない**。`search_walker` は ZIP の中を再帰せず、
`ingest_worker::build_doc_for_name_only` が ZIP ファイル名だけを doc 化する。
`fts_index` schema の `zip_entry` フィールドや `global_search_ui` の
`split_zip_hit_path` / `build_drilled_zip_items` は将来用の足場で、現状は到達しない。
(`search-architecture.md` §4.6 は「ZIP 内画像も ingest 対象」と書いているが実装と
食い違っている — §9 で訂正対象。)

本再設計では **ZIP = コンテナ = Ctrl+S の領域** に統一する:

- アイテム索引から ZIP ファイル doc を外す。**`search_walker` が `CandidateKind::Zip`
  を作らない** (= `fs_map` に ZIP を入れない) ようにする。こうすると 3-way diff で
  既存の ZIP doc は「FS になし + DB あり」と判定され `to_delete` に落ちて Tantivy から
  消える。`ingest_worker` 側で skip するだけでは mtime/size 一致の ZIP が unchanged
  扱いになり古い Tantivy doc が残るので不可。Ctrl+G の対象は 画像 / PDF / 動画のみ。
- Ctrl+G の種別フィルタから「ZIP」を削除 (画像 / PDF / 動画)。
- ZIP ファイル名での横断検索は Ctrl+S (コンテナ検索) が担う。現状 ZIP ファイル名は
  Ctrl+S・Ctrl+G の両方で引けるが、これは重複なので Ctrl+S に一本化される。

**ZIP 内画像のメタ検索 (タグ / EXIF / AI プロンプト) は本再設計のスコープ外**。
これを実現するには walker が ZIP 内を列挙し、ZIP 専用 ingest context が各エントリの
メタを読む新サブシステム (`search-architecture.md` §4.6 が想定する未実装部分) が
必要で、ネスト ZIP・ZIP mtime 変化での全エントリ再 ingest・RAM 制御を伴う。生成画像を
ZIP で保管するユーザー向けの機能として、独立した後続プロジェクトで扱う。

> **決定: 案 B1。** ZIP 内画像のメタ検索 (Ctrl+G) は今回実装しない。ZIP はコンテナ
> として Ctrl+S が扱う。ZIP 内画像のメタ検索が必要になったら、独立した後続プロジェクト
> として扱う (`search-architecture.md` §4.6 が想定する ZIP 内 item ingest とまとめて)。

---

## 4. モード別 詳細仕様

### 4.1 Ctrl+F — 現在地フィルタ

**現状の対象 (item 種別ごと)**:

`run_metadata_search` が今 item 種別ごとに何を照合しているか:

| アイテム | 現在 Ctrl+F が照合する対象 |
| --- | --- |
| Folder | なし — 常に表示 (絞られない) |
| ZipFile / ConvertibleArchive | なし — 常に表示 |
| PdfFile | なし — 常に表示 (PDF document info は未使用) |
| PdfPage | ページ名のみ |
| Image | ファイル名 / PNG tEXt / EXIF / XMP (ツイート情報) / タグ — 検索対象に応じ lazy 読み |
| Video | ファイル名 / 動画コンテナメタ / タグ — lazy 読み |
| ZipImage (PNG エントリ) | ファイル名 + PNG tEXt + XMP (ZIP を 1 回開く) |
| ZipImage (非 PNG エントリ) | ファイル名 (エントリ basename) のみ |
| SearchContainer | なし — 常に表示 |

不整合: (a) 構造アイテム (Folder / ZipFile / PdfFile) が一切絞られない、
(b) PdfFile の PDF document info が使われない、(c) ZipImage が PNG なら全メタ・非 PNG
ならファイル名のみ、と非対称。いずれも下記の変更で解消する ((c) は §4.1.2 — ZIP 内
メタ検索を廃止し ZipImage を一律ファイル名照合に統一)。

**変更点: 構造アイテムも一貫して絞り込む。**

現状の「Folder / ZipFile / PdfFile は常にマッチ扱い」は、フィルタが部分的にしか
効いていないように見えて直感に反する (「sunset で絞ったのに無関係なフォルダが残る」)。
新仕様では構造アイテムも、そのアイテムが持つ次元でマッチ判定する:

| アイテム | マッチ判定 |
| --- | --- |
| Folder | フォルダ名 (basename) をクエリと照合 |
| ZipFile / ConvertibleArchive | ファイル名をクエリと照合 |
| PdfFile | ファイル名 + PDF document info (§4.1.1 参照) |
| Image / Video | 現状どおり (target に応じた on-demand メタ判定) |
| ZipImage | ファイル名のみで照合 (§4.1.2: ZIP 内メタ検索はしない) |
| PdfPage | — (PDF ページ表示中は Ctrl+F 自体を無効化、§4.1.1) |

- 検索対象 = ファイル名 / すべて → 名前がマッチする構造アイテムは残る、しないものは消える。
- 検索対象 = EXIF / タグ等 (構造アイテムが持たない次元) → 構造アイテムは全て消える。
  これは「タグで絞ったらタグを持つアイテムだけになる」= 正しい挙動。
- 照合は既存の `search_query::matches_with_mode(tokens, basename, mode)` を流用する。
- `SearchContainer` は Ctrl+F と Ctrl+G が排他なので通常出現しないが、防御的に現状の挙動を残す。

**トレードオフ**: フィルタ中に子フォルダへ直接潜れなくなる。ただし Esc で即解除できる
ため実害は小さく、予測可能性のメリットが上回る (本再設計の「分かりやすさ優先」方針)。

**マッチ件数バッジ**: 現状は画像のみカウント (「画像 X/Y 件」)。構造アイテムも
絞れるようになるので、可視マッチ全体をカウントする「X/Y 件」に変更する。

実装ポイント: `run_metadata_search` の Pass 1 を上記ルールに書き換える。`search_filter`
→ `rebuild_visible_indices` の経路は変更不要 (絞り込み結果の HashSet が変わるだけ)。
PDF document info を使うときは `run_metadata_search` から `pdf_loader::get_document_info`
を直接呼ぶ (プールハンドルの配線は不要 — §4.1.1)。PDF ページ表示中は Ctrl+F の入口
(ショートカット / バー表示) で弾く (§4.1.1)。`ZipImage` は Pass 1 のファイル名照合に
一本化し、ZIP を開いてメタを読む Pass 3 は削除する (§4.1.2)。

#### 4.1.1 Ctrl+F と PDF

現状、`PdfFile` は常に残り、`PdfPage` はページ名だけで判定され、PDF document info
(タイトル / 著者 / 件名 / キーワード) は Ctrl+F では実質使われていない。新仕様では
ユーザー期待 (「検索対象 = PDF メタ情報」なら PDF のタイトル等も対象になる) に合わせる。

**親フォルダ上の `PdfFile`**:

- 検索対象 = ファイル名 → ファイル名で判定。
- 検索対象 = PDF メタ情報 → `pdf_loader::get_document_info(path, password)` が返す
  document info (Title / Author / Subject / Keywords) で判定。
- 検索対象 = すべて → まずファイル名で判定し、マッチしなければ document info も見る
  (短絡評価で不要な IPC 読み取りを避ける)。
- 検索対象 = EXIF / タグ等 (PDF が持たない次元) → 非マッチ = 非表示。

API・パスワード・キャンセルの注意 (Codex P2):

- `get_document_info(path: &Path, password: Option<&str>)` は free 関数で内部で
  グローバル PDF ワーカープールを使う。**プールハンドルを呼び出し側へ配線する必要は
  ない** — `run_metadata_search` から直接呼べる。
- `password` は `PdfPasswordStore` (`pdf_passwords`) の保存済みパスワードを渡す。
  保護 PDF でパスワード未保存なら `get_document_info` は失敗 → その PDF は
  ファイル名のみで判定 (PDF メタは非マッチ扱い)。
- `get_document_info` は中断不能な同期 IPC。cancel は **PDF と PDF の間** でしか
  効かない (1 件の IPC 実行中は中断不可)。worker スレッド上なので UI は止めないが、
  target が PDF メタ情報を要求するときだけ呼び、PDF ごとに cancel を確認する。

**PDF を開いた後 (グリッドが `PdfPage` のとき): Ctrl+F を無効化する。**

PDF のページは連番だけで、ページ単位の検索可能なメタ情報を持たない (本文テキストは
対象外)。連番のページ名を絞っても得るものがないため、グリッドが PDF のページ表示に
なっているときは **Ctrl+F 自体を無効化** する (ショートカット無反応・検索バーも開かない)。

- 判定: `PdfPage` は PDF を開いたときにのみ出現し、通常フォルダ / Ctrl+S / Ctrl+G 結果
  には出ない (それらは `PdfFile`)。「グリッドが `PdfPage` で構成される = PDF 表示中」
  で判定できる。
- これにより `run_metadata_search` が `PdfPage` を扱う経路は不要になる。

#### 4.1.2 Ctrl+F と ZIP

ZIP を開いたグリッド (`ZipImage` 群) では Ctrl+F を **ファイル名フィルタとしてのみ
有効** にする (ZIP 内画像のメタ情報 — AI プロンプト / EXIF / XMP — は検索しない)。

理由:

- ZIP に固めた画像のメタ検索の実需要は低い (AI 生成画像は通常ルーズなまま保管し
  ZIP 化しない)。Ctrl+G のアイテム索引も ZIP 内画像を対象にしない (§3.2) ので、
  「ZIP 内のメタ検索はどこでもやらない」で統一する。
- ZIP 内エントリのメタを読むには 1 枚ずつエントリ全体を伸張する必要があり (ZIP は
  seek 不可)、通常フォルダ並みの速度にするには bounded read 等の特別な実装が要る。
  需要が低い機能にそのコストは見合わない。
- 一方、**ファイル名での絞り込みはエントリ名だけで判定でき I/O ゼロ**。大きな
  アーカイブを名前で素早く絞れる有用な機能なので残す。

実装はむしろ簡単になる:

- `ZipImage` をすべて Pass 1 のファイル名照合に一本化し、**ZIP を開いてエントリの
  バイトを読む Pass 3 を削除** する。特別な高速化実装 (bounded read) は不要。
- ZIP 表示中は Ctrl+F の検索対象ドロップダウンを「ファイル名」に固定する
  (メタ系ターゲットを選んでも無反応、という分かりにくさを防ぐ)。

PDF ページ表示中は Ctrl+F 全体を無効化する (§4.1.1) のに対し、ZIP はファイル名
フィルタを残す。これは一貫したルールで説明できる:「**Ctrl+F はファイル名による
フィルタ + ルーズなファイルのみメタ検索。実ファイル名を持つ場所で使える**」。PDF の
ページは連番の合成名で実ファイル名を持たないため無効、ZIP エントリは実ファイル名を
持つためファイル名フィルタが効く。

### 4.2 Ctrl+S — コンテナ検索

**変更点 (1): 動画を検索対象から外す。**

動画はコンテナではない (中に画像が入っているわけではない) ので、コンテナ検索の
対象から外し Ctrl+G 専属にする。動画にも内部チャプター/複数フレームがある等の理由で
コンテナ扱いする案も検討したが、索引が少し重くなるだけで Ctrl+G で十分カバーできる
ため不採用。

- `name_bulk_indexer` の分類ヘルパが `IndexKind::VideoFile` を返さないようにする。
- `name_index_supervisor` の差分追従 (`apply_single_change` 経路) も同様。
- `search_index.db` の既存の動画行は **移行コード不要**: 動画が列挙されなくなれば、
  各フォルダの `upsert_children` の prune (updated_at < cutoff) が次回スキャンで
  自然に削除する。`IndexKind::VideoFile` enum variant 自体は stale 行の読み取りで
  panic しないよう残してよい (書き込み経路からのみ除去)。

**変更点 (2): 種別フィルタを追加する (Ctrl+G と操作感を統一)。**

- UI: 「種別」ドロップダウン。選択肢は **すべて / フォルダ / ZIP / PDF** (動画は対象外)。
- `FavSearchState` に `kind_filter: Option<IndexKind>` を追加。
- `SearchIndexDb::search()` に kind 引数を追加し、`WHERE kind = ?` を AND する。
- ドロップダウン変更時は `execute_favsearch` を即再実行 (お気に入りドロップダウンと同じ)。

**変更点 (3): UI 文言をコンテナ検索に寄せる。**

「名前で検索」だけだと画像 / 動画のファイル名も検索できそうに見える。バーのラベルや
hint_text は「コンテナ検索」「フォルダ・本を探す」等、対象がコンテナだと分かる表現に
する。例: hint_text を現状の「お気に入り配下のフォルダ/ZIP/PDF/動画名」から
「フォルダ・ZIP・PDF をコンテナ名で探す (AND / -除外 / "…")」へ。

結果の見せ方は現状維持 (検索結果でグリッドを置換するフラットな単一リスト)。Ctrl+S の
結果はコンテナそのものの平置き = 既にコンテナ一覧なので集約ビューは不要。

### 4.3 Ctrl+G — アイテム検索

#### 4.3.1 3 つの表示ビュー

Ctrl+G の結果ビューを 3 形態にする:

| ビュー | 内容 | 用途 |
| --- | --- | --- |
| **一覧 (Flat)** | 全ヒットを個々のサムネイルで平置き | 少数ヒット / タグ検索など「直接見たい」 |
| **集約 (Aggregated)** | ヒットを親フォルダ単位でまとめた SearchContainer セル | 多数ヒット |
| **ドリルイン (DrilledInto)** | 集約セルをダブルクリックして中身へ | 集約からの掘り下げ (既存) |

「一覧」が新規。「集約」「ドリルイン」は既存の `Aggregated` / `DrilledInto` 相当。

#### 4.3.2 一覧 / 集約の状態モデルと自動切替

集約は **トグルボタン 1 個** で表現する (種別 5 択と違い 2 状態なのでドロップダウンより
トグルボタンが省スペース)。ボタンは「集約」トグル (OFF=一覧 / ON=集約)。

状態を `GlobalSearchState` に持つ:

- `aggregate: bool` — 集約トグルの状態 (false=一覧, true=集約)。
- `aggregate_auto: bool` — `aggregate` がまだ自動制御下か (true) / ユーザーが固定したか (false)。
- `drill: Option<DrillState>` — コンテナにドリルインしていれば Some。

実効ビューは導出する: `drill.is_some()` → ドリルイン / `aggregate` → 集約 / それ以外 → 一覧。
この導出モデルにすると、ドリルバックは `drill = None` にするだけで `aggregate` に応じて
一覧 or 集約へ正しく戻れる (現状の `GlobalSearchView` enum は Aggregated/DrilledInto の
2 値で、ドリルバック先が常に Aggregated 固定。Flat 追加に伴いこの導出モデルへ再構成する)。

**自動切替の状態機械**:

```
新クエリ実行 (reset_for_new_query):
    aggregate_auto = true, aggregate = false (一覧から開始), drill = None

ストリーミング中 / rebuild 時、aggregate_auto == true のとき:
    総ヒット数 total_valid > AGGREGATE_AUTO_THRESHOLD (= 1000)
        → aggregate = true  (集約トグルが自動で ON になる)
    else
        → aggregate = false (一覧のまま)
    ※ total_valid は増加一方なので、実質「一覧 → 集約」の一方向・最大 1 回の遷移。
      その 1 回もストリーミング開始 1〜2 秒以内 (ユーザーがまだ触る前) に起きる。

aggregate_auto を false に倒す (= 自動切替を止める) トリガ:
    (a) ユーザーが集約トグルを手動クリック
    (b) ユーザーが検索結果を操作 (セル選択 / スクロール / カーソルキー)
    (c) ユーザーがコンテナにドリルイン

aggregate_auto == false のとき:
    aggregate は手動操作のみで変わる。total_valid がいくら増えても view は固定。
```

- `AGGREGATE_AUTO_THRESHOLD` は定数 1 個 (= 1000)。後から調整可能。
- 集約トグルは一覧 / 集約時のみ表示。ドリルイン中は代わりに「← 戻る」を出す (現状どおり)。
- 集約トグルのラベルは UI グリフポリシー ([CLAUDE.md](../CLAUDE.md) 参照) に従い、
  フォント依存記号を避ける。テキスト「集約」を基本とし、アイコンを付けるなら
  `scripts/check_ui_glyphs.py` で実機 tofu チェックを通った字形に限る。

実装ポイント:
- `GlobalSearchView` enum を上記の `aggregate` / `aggregate_auto` / `drill` 構成へ再構成。
- 「一覧」ビューの items 構築 `build_flat_items` を新規追加。`state.all_hits` を走査して
  各ヒットを `GridItem::Image / PdfFile / Video` に変換し (§3.2 より ZIP 内画像ヒットは
  存在しない)、§4.3.3 のソート順で並べる。`build_drilled_items` と同じく placeholder
  `image_metas` を使い、UI スレッドでの `fs::metadata` 同期呼び出しは避ける。
- `rebuild_items_from_global_search` の view 分岐に Flat を追加。
- `poll_global_search_events` の rebuild 時に、`aggregate_auto` なら閾値判定を行う。
- 検索結果への操作 (b) を検出して `aggregate_auto = false` にするフックを 1 箇所用意する。

#### 4.3.3 ソート

各ビューはその内容に合ったソートを使う:

| ビュー | ソート | UI |
| --- | --- | --- |
| 一覧 | メインの `settings.sort_order` (ファイル名/番号/日付↑/日付↓) | メインツールバーの既存ソートボタンを流用。Ctrl+G バーにソート UI は出さない |
| 集約 | 既存 `ContainerSortMode` (件数/名前/新旧) | Ctrl+G バーのソートドロップダウン (現状どおり、集約時のみ表示) |
| ドリルイン | メインの `settings.sort_order` | メインツールバーのソートボタン |

- 一覧 / ドリルインは「個々のアイテムの一覧」= 通常のフォルダ閲覧と同じ性質なので、
  メインの `settings.sort_order` とそのボタンで操作させる (新規 UI を作らない)。
- 一覧ビューは全アイテムを `settings.sort_order` で一律ソートする。ドリルインは
  通常グリッド慣習 (サブフォルダを名前順で先頭 → ファイルを `settings.sort_order`) に
  合わせる。
- 日付順 (`DateAsc` / `DateDesc`) には各ヒットの mtime が必要 → §5.2 で `GlobalHit` に
  mtime を追加する。ファイル名順 / 番号順は path から導出できるので mtime 追加前でも動く。

---

## 5. 索引・データ構造の変更

### 5.1 名前索引から動画を除外

§4.2 変更点 (1) を参照。`name_bulk_indexer` / `name_index_supervisor` の分類経路から
`IndexKind::VideoFile` を外す。`search_index.db` のスキーマ変更は不要 (kind 列はそのまま)。
既存の動画行は自然 prune で消えるため移行コード不要。

### 5.2 GlobalHit に mtime を追加

一覧 / ドリルインビューの日付ソート (§4.3.3) のために、各ヒットが mtime を持つ必要がある。

**現状確認**: `fts_index/` は既に `INDEX_VERSION=7`。Tantivy schema には `mtime` が
`INDEXED | STORED` で入っており (`fts_index.rs` の `IndexDoc.mtime` / `Fields.mtime`、
スキーマ定義 `add_i64_field("mtime", INDEXED | STORED)`)、ingest 時に格納済み。
つまり mtime は **既に索引・STORED されている**。

**必要なのは取り出し経路の追加だけ** — schema 変更も INDEX_VERSION bump も不要:

- `GlobalHit` 構造体に `mtime: i64` フィールドを追加 (現状は path / score / stars のみ)。
- `global_search::run` が候補ごとに STORED 原文を引くのと同じ経路で、既存の STORED
  `mtime` フィールドも読み、`GlobalHit.mtime` に詰める。

本再設計は検索索引のスキーマを一切変更しない (`search_index.db` 側も変更なし)。

---

## 6. UI 変更まとめ

| 場所 | 変更 |
| --- | --- |
| Ctrl+F バー | 機能変更なし (内部の絞り込みロジックのみ)。件数バッジを「X/Y 件」に |
| Ctrl+S バー | 「種別」ドロップダウン (すべて/フォルダ/ZIP/PDF) を追加 |
| Ctrl+G バー | 「集約」トグルボタンを追加 (一覧 ⇄ 集約)。種別ドロップダウンから「フォルダ」「ZIP」を削除 (画像/PDF/動画) |
| メインツールバー | 既存ソートボタンが Ctrl+G の一覧 / ドリルインビューにも効くようにする |
| hint_text / hover | 各バーの説明文をコンテナ/アイテムモデルに合わせて更新 |
| お気に入り登録 / 編集画面 | 索引のラベル・説明を「コンテナ索引」「アイテム索引」に統一 (§3.1) |

---

## 7. 実装タスクと着手順

| # | タスク | 備考 |
| --- | --- | --- |
| 1 | ZIP をアイテム索引 (Ctrl+G) から除外 | `search_walker` が `CandidateKind::Zip` を作らない → 既存 ZIP doc は `to_delete`。種別フィルタからも ZIP 削除 (§3.2)。**#2 より前に行う** |
| 2 | Ctrl+G フラットビュー + 集約トグル + 自動切替状態機械 | `GlobalSearchView` 再構成、`build_flat_items` (`Image / PdfFile / Video` 前提)、閾値判定。名前順ソートまでで先行リリース可 |
| 3 | `GlobalHit` に mtime 追加 (既存 STORED mtime の取り出し経路のみ) | 2 の日付ソートを完成させる。schema 変更・INDEX_VERSION bump なし |
| 4 | 動画をコンテナ索引 (Ctrl+S) から除外 | `name_bulk_indexer` / `name_index_supervisor` |
| 5 | Ctrl+S 種別フィルタ追加 + UI 文言の調整 | `FavSearchState` / `SearchIndexDb::search()` / バー UI |
| 6 | Ctrl+F 修正 (構造アイテム絞り込み / PDF document info / PDF 表示中は無効化 / ZIP はファイル名のみ) | `run_metadata_search` の構造アイテム特例削除、Ctrl+F の入口ガード |
| 7 | 索引呼称の統一 (コンテナ索引 / アイテム索引) | お気に入り画面・hover・説明文 |
| 8 | マニュアル・設計ドキュメント更新 | §9 参照 |

着手順は **1 → 2 → 3 → 4 → 5 → 6 → 7 → 8**。**#1 (ZIP 除外) を #2 (フラットビュー) より
先に行う** — `build_flat_items` は `Image / PdfFile / Video` 前提なので、ZIP doc が
アイテム索引に残ったままだと中間状態で Ctrl+G 結果に ZIP が混ざる。#2 と #3 は中核で
互いに密結合 (一覧ビューのソートに mtime が要る) なので連続で進める。4 / 5 / 6 / 7 は
独立しており順不同で可。8 は最後にまとめる。

---

## 8. 影響を受けるファイル (想定)

- `src/global_search_ui.rs` — フラットビュー、集約トグル、自動切替、view 再構成
- `src/global_search.rs` — `GlobalHit.mtime`、既存 STORED `mtime` の読み取り経路
- `src/fts_index.rs` — STORED `mtime` 取り出しヘルパが要れば最小限 (schema 変更なし)
- `src/search_walker.rs` — `CandidateKind::Zip` を作らない (ZIP を `fs_map` に入れない → 既存 ZIP doc は 3-way diff で `to_delete`、§3.2)
- `src/name_bulk_indexer.rs` / `src/name_index_supervisor.rs` — 動画分類の除去
- `src/search_index_db.rs` — `search()` に kind 引数
- `src/pdf_loader.rs` — `get_document_info()` を Ctrl+F から呼ぶ (pdf_loader 自体は変更なしの想定)
- `src/app.rs` — `FavSearchState.kind_filter`、`run_metadata_search` Pass 1 + Pass 3 削除、PDF 表示中の Ctrl+F 無効化、Ctrl+F 件数バッジ
- `src/ui_main.rs` — Ctrl+S バーの種別ドロップダウン・文言、Ctrl+F バー (PDF 表示中は出さない、ZIP 表示中は検索対象をファイル名に固定)
- `src/ui_dialogs/favorites_editor.rs` / `src/ui_dialogs/fav_add.rs` — 索引ラベルの呼称統一
- `src/grid_item.rs` — 必要なら `GridItem` 変換ヘルパ

---

## 9. 同時更新が必要なドキュメント

- `docs/search-architecture.md` — §1.1 の 3 モード表、§4.4 (名前索引から動画除外)、
  §4.6 (**現状の誤記訂正**: ZIP 内画像は ingest していない — §3.2)、
  §5 のクエリ実行パス (Ctrl+G の 3 ビュー、Ctrl+F の PDF document info)
- `docs/spec.md` — 検索機能の仕様
- `htdocs/mimageviewer/manual/search.html` — ユーザー向けマニュアル
  (コンテナ/アイテムの説明、索引呼称、バージョン固有表記は書かない)
- お気に入り編集まわりのマニュアルページ — 索引呼称を「コンテナ索引 / アイテム索引」へ
- `htdocs/mimageviewer/index.html` — 機能紹介 (必要なら)
- `docs/README.md` — 本ドキュメントへのリンク (登録済み)

---

## 10. 未決事項・リスク

- **集約トグルのアイコン字形**: テキスト「集約」で確定。アイコンを足すなら UI グリフ
  lint を通った字形に限る。
- **一覧ビューのソート単位**: 一覧は全アイテムを `settings.sort_order` で一律ソートする
  案。PDF / 動画も画像と混ぜて一律で並ぶ。通常グリッドの「フォルダ系を名前順で先頭」
  慣習とは異なるが、検索結果一覧では一律の方が予測しやすいと判断 (要レビュー確認)。
- **大量ヒット時の一覧ビュー**: HARD_MAX=10000 件の一覧でも仮想スクロール + ページ単位
  evict で描画は問題ない想定。閾値 1000 で自動的に集約へ寄るため最悪ケースは限定的。
- **自動切替ロック後の大量一覧**: ユーザーが少数のうちに結果を操作 → 一覧でロック →
  その後ヒットが数千に膨らむケースでは一覧のまま。ユーザーの明示選択を尊重する仕様と
  し、手動で集約トグルを押せば集約に移れる。
- **Ctrl+F のナビ性低下**: 構造アイテムを絞ると非ヒットの子フォルダへ直接潜れなくなる。
  フィルタ元フォルダでは BS / Alt+↑ / ⬆ による親移動も止めるが、ヒットした子フォルダへ
  入った後の BS は通常どおり元フォルダへ戻れる。Esc で即解除できるため許容 (§4.1)。
- **Ctrl+F の PDF document info (IPC / パスワード / キャンセル)**: PDF が大量にある
  フォルダで検索対象 = PDF メタ情報 / すべて にすると `get_document_info` の IPC が
  PDF ごとに走る。1 件の IPC は中断不能で cancel は PDF 境界でしか効かない。
  worker スレッド上 + 短絡評価で緩和するが、体感が悪ければ document info の結果を
  Ctrl+F セッション内でキャッシュする / タイムアウトを設ける等を検討する。保護 PDF で
  パスワード未保存のものはメタ判定をスキップ (ファイル名のみ)。
- **ZIP 内画像のメタ検索**: 本再設計のスコープ外 (§3.2)。需要があれば独立した
  後続プロジェクトで walker の ZIP 内列挙 + ZIP 専用 ingest context を実装する。
- **Ctrl+F の ZIP はファイル名のみ**: ZIP 内画像のメタ検索 (AI プロンプト / EXIF /
  XMP) は行わない (§4.1.2)。需要が低く、ZIP は seek 不可でメタ読み取りに特別な高速化
  実装が要るため。ファイル名フィルタは I/O ゼロで残す。将来 ZIP 内メタ検索の需要が
  出たら、§3.2 の ZIP 内 item ingest (Ctrl+G 側) とまとめて後続プロジェクトで扱う。

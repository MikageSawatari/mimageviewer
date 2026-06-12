# タグ機能 カタログ再設計プラン (全アイテム対応 / 非破壊 / 検索から完全分離)

## 0. このドキュメントの位置づけ

[docs/tag-feature.md](tag-feature.md) は **v1.0 のタグ機能 (= ファイル埋め込み中心)** の設計書である。
本ドキュメントはその **保存・検索モデルを置き換える再設計**で、以下を実現する:

- 画像・動画だけでなく **ZIP / PDF / 変換可能アーカイブ (実ファイルのコンテナ) / フォルダ にもタグを付与**できる
- タグの正本を **mIV 自身のカタログ DB (`tags.db`)** に置き、**メディア本体・interop サイドカー・
  全文検索索引のいずれにも書かない (非破壊・投影ゼロ)**
- タグの発見は Ctrl+G / Ctrl+S とは **完全に分離**し、**専用のタグビュー (Ctrl+T) + facet フィルタ**で行う

tag-feature.md から **生かす部分**: §3.3 タグ名のルール (使用可能文字・長さ・大小文字無視)、
XMP `dc:subject` の **読み取り**ロジック ([src/xmp_reader.rs](../src/xmp_reader.rs)、移行専用)。

tag-feature.md から **置き換える部分**: §1/§3.1/§5 (XMP・動画 `.xmp` への**書き込み**を正本とする
モデル全体)、`#` プレフィックスをファイルに刻む規約、ZIP内画像/PDFページ/フォルダを非対象とする
スコープ定義、Ctrl+G `#tag` でのタグ検索。

> 本設計は **タグ機能のリリース済み挙動を変更する**。後方互換 (既存タグの保全) は §7 で扱う。

---

## 1. 設計方針 (確定事項サマリ)

| # | 決定 | 根拠 / 参照 |
|---|---|---|
| D1 | **正本 = `tags.db` (新規 SQLite)**。メディア本体・interop サイドカー・検索索引のいずれにも書かない | 非破壊優先。即時・破損リスク無し・mtime churn 無し |
| D2 | **付与対象 = 実ファイル + フォルダ**: Image / Video / ZipFile / PdfFile / ConvertibleArchive / **Folder** | コンテナ/フォルダはそれ自身に付与。フォルダは UX レビューで v1 追加。§2 |
| D3 | **ZIP内画像・PDFページ・ZipDir は付与不可** | アーカイブ内の仮想エントリ/ページ/サブコンテナ (実ファイルでない) |
| D4 | **タグは Ctrl+G(FTS) / Ctrl+S(名前) に一切投影しない (完全分離)** | 異種2索引への投影が破綻源 (Codex 指摘)。投影をやめれば全部消える。§5 |
| D5 | **発見は専用「タグビュー」(Ctrl+T) + タグツールバーの検索ボタン** | tags.db 直引き → 通常グリッド描画。**お気に入り登録・索引に非依存・常に最新**。§5.1 |
| D6 | **検索＋タグ combo は facet AND 合成で実現** | `rebuild_visible_indices` が `search_filter AND rating_filter AND facet_filter`。Ctrl+G/S 結果に facet タグ絞りが既に効く。§5.3 |
| D7 | **既存 facet タグフィルタは残すが UI 二層化** (プルダウンは show_shortcut のみ) + タグ非対応アイテムは素通り | コンテナを開いても中身が消えない。★式トースト不要。§5.2 |
| D8 | **タグは二層**: ショートカットタグ (少数・キュレート) と 自由記入タグ (大量・作品名/作者名) | 高カーディナリティをツールバーに並べない。§6.4・§6.5 |
| D9 | **タグ名は内部 `#` なし保存・画面 `#` 付き表示** | `tags.db` は mIV 専有なので区別用 `#` 不要。`#` は「タグである」ことを示す表示上の飾り。§3 |
| D10 | **他アプリ由来 (非 `#`) dc:subject はタグ扱いしない** | バッジ・タグセクション・専用検索索引なし。生メタ表示のみ可。§3.2 |
| D11 | **`tags.db` = 正規化テーブル** `item_tags(item_key, tag, tag_key, applied_at)` + `tag_key` index | 大小無視/前方一致/最近使用/DISTINCT を高カーディナリティで効率化。§8.1 |
| D12 | **`tags.db` ↔ `mimageviewer.dat` 二層**。タグ用サイドカーバックアップは新トグルで **既定 OFF** | 既存編集データと同じ二層パターン。§4 |
| D13 | **書き込み投影ゼロ**: タグ書き込みは `tags.db` のみ。Tantivy/search_index には書かない | D4 の帰結。stale化しない |
| D14 | **移行 = 既存 Tantivy `tags` の `#` 付き値を `tags.db` へ一括コピー (`#` 剥がし)** | ファイル I/O・再スキャン・リバイバルなし。§7 |
| D15 | **既存 FTS のタグ経路を能動的に閉じる**: `SourceKind::Tags` を検索/ingest から外し、STORED tags は移行専用に | 「分離」を原則でなく実挙動にする。§5.4 |
| D16 | **タグビューの stale パスは worker で存在確認 → 欠落セル表示 + 日和見/手動 prune (自動全削除しない)** | 索引非依存にした分 notify 掃除を失うため。§5.5 |
| D17 | **体験層では D4 の分離を見せない (UX レビュー)**: Ctrl+G の素キーワード→タグ件数ヒント / チップ・バッジのクリック=タグ検索 / メニューに「タグビューを開く」 | 分離が体験の断絶として出ると「タグが壊れた」と誤解される。§13 |
| D18 | **複数選択トグルは all-or-nothing** (全付与済み→全削除、それ以外→全付与) | 結果が予測しやすい。出荷マニュアルの「各ファイル独立」記述を改める。§6.1 |
| D19 | **`tags.db` は settings.db 同様の世代バックアップを持つ。外部書き出しはしない** | ロックイン不安/DB 破損対策。非破壊・外部書き込み無し方針は維持。§13 |

---

## 2. 対象アイテムとキー

### 2.1 付与可否

| GridItem variant ([src/grid_item.rs](../src/grid_item.rs)) | タグ付与 | 種別 |
|---|---|---|
| `Image` / `Video` | ○ | 実ファイル (画像/動画) |
| `ZipFile` / `PdfFile` / `ConvertibleArchive` | ○ | 実ファイル (コンテナ自身) |
| `Folder` | ○ | フォルダ (v1 で付与可。UX レビュー) |
| `ZipImage` / `PdfPage` / `ZipDir` | ✗ | アーカイブ内の仮想エントリ/ページ/サブコンテナ |
| `ZipSeparator` (撤去予定 §9) / `SearchContainer` | ✗ | 疑似アイテム |

判定述語は **2 つに分ける** (UX レビューでフォルダを付与可にしたため):
- **`item_supports_tags`** (付与/タグビュー/バッジ対象): Image / Video / ZipFile / PdfFile /
  ConvertibleArchive / **Folder** の 6 種。[`facet_item_supports_tags` (src/app.rs:1497)](../src/app.rs) を
  これに拡張・改称する。
- **`facet_tag_filter_applies`** (現フォルダの facet で in-place に**隠す**対象): Image / Video / ZipFile /
  PdfFile / ConvertibleArchive のみ。**Folder と ZipImage/PdfPage/ZipDir は対象外 (素通り)**。フォルダは
  ナビ構造として常に表示し (ドリルダウン空表示の再発防止)、フォルダのタグは facet で in-place に絞らず
  **タグビュー (Ctrl+T) で発見**する (§5.2)。

### 2.2 キー規則

付与対象はすべて単一の絶対パスを持つので **キーは `normalize_path(path)` のみ**
([src/adjustment_db.rs:305](../src/adjustment_db.rs)、lowercase + `\`→`/`)。★ レーティングのような
ZipImage/PdfPage/ZipDir 用合成キーは不要 (それらは付与対象外)。フォルダもパスを持つので同じ
`normalize_path` に乗る。`item_key` = この正規化パス。

---

## 3. タグの本体と「他アプリ由来メタ」の切り離し

### 3.1 mIV タグ (タグ system の本体)

- 正本は `tags.db`。**内部表現は `#` なしの素のタグ名** (`原神`、`鈴木作』` 等)。
- **画面表示時のみ `#` を冠して「タグ」であることを示す** (`#原神`)。`#` は表示上の飾りで、保存・
  照合・キー化はすべて `#` なしで行う。
- v1.0 互換: v1.0 はファイル XMP に `#原神` と刻んでいた。移行 (§7) で `#` を剥がして `tags.db` に取り込む。
- タグ名ルールは tag-feature.md §3.3 を踏襲 (1〜64 文字、大小無視、`#` は入力させず表示時に付与)。

### 3.2 他アプリ由来 dc:subject は「タグ」として扱わない (D10)

- ファイルに他アプリが書いた `dc:subject` キーワード (mIV 由来でないもの) は **mIV のタグ system に
  取り込まない**。
- **サムネバッジに出さない / タグセクションに出さない / 専用検索索引も作らない**。サムネのタグ
  バッジは **mIV タグのみ**。
- 「Ctrl+G で `原神` を引くと外部 dc:subject の `原神` は出るが mIV タグ `原神` は出ない」という非対称を
  避けるため、**外部キーワードの専用インデックス化 (旧 D6) は v1 では行わない**。
- ただし mIV が生の XMP/EXIF をメタパネルに表示する経路があれば、そこに `dc:subject` が
  **EXIF 等と同列の生メタとして**出るのは構わない (タグとしての見せ方はしない)。
- 将来「他アプリの埋め込みキーワードでファイルを探す」需要が出たら、タグ system とは独立した
  小機能として別途検討する。

---

## 4. 保存アーキテクチャ (二層: tags.db + サイドカー)

### 4.1 既存の編集データと同じ二層構造に乗せる

既存の編集データ 6 種 (補正 `adjustment.db` / マスク `mask.db` / 隠蔽 `conceal.db` / 補正レイヤー
`local_adjust.db` / crop `crop.db` / テキスト注釈 `comic.db`) はすべて **「中央 DB primary +
`mimageviewer.dat` サイドカーバックアップ」** で、サイドカーオンリーは存在しない。
インポートは中央 DB 優先 ([src/sidecar.rs:520](../src/sidecar.rs))。

**タグも同じパターンに乗せる**:

- 正本 = `tags.db` (新規、`rating_db.rs` を雛形)。
- バックアップ = `mimageviewer.dat` に `tags` フィールド追加 (`SidecarEntry` 拡張, [src/sidecar.rs:49](../src/sidecar.rs))。
- **インポート粒度 (Codex P2)**: tags のような複数行データでは中央優先の粒度を明文化する。
  **item_key 単位の all-or-nothing**: `tags.db` がその `item_key` のタグを既に把握している (= `item_tags`
  行が在る、または `tag_item_state` 行が在る、§8.1) なら **sidecar のタグは丸ごと skip**。把握していない
  item_key のみ sidecar のタグ集合を取り込み、取り込み後に `tag_item_state(source='sidecar')` を立てる。これは既存 sidecar の「中央にエントリが在れば上書きしない」
  ([src/sidecar.rs:520](../src/sidecar.rs)) と同じ粒度で、**ユーザーが tags.db で削除したタグが sidecar から
  復活するのを防ぐ** (削除で空になった item も「決定済み」扱いで skip)。
- **キー規則の拡張**: 現行の `sidecar_folder` / `sidecar_relative_key` ([src/app.rs:23049](../src/app.rs)) は
  Image/ZipImage/PdfPage のみ対応で Video/ZipFile/PdfFile/ConvertibleArchive は `_ => None`。
  タグ対象 5 種を扱うため、**実ファイル系は「親フォルダの `mimageviewer.dat` + 小文字ファイル名」を
  キー**に拡張 (Image の既存規則と同形)。読み書き両方が `tag_sidecar_backup_enabled` で gate。
- **import gate と sync 状態をタグ用に分ける (Codex P2)**: 現行 import は `sidecar_backup_enabled` で gate され
  ([src/app.rs:11201](../src/app.rs))、`sidecar_sync` の mtime 一致なら読まずに戻る fast-path がある
  ([src/app.rs:25913](../src/app.rs))。`tag_sidecar_backup_enabled` を**後から ON** にした場合、既に mtime
  同期済み扱いでタグだけ取り込まれない恐れがある。よって **タグ用に独立した import gate + sync 状態
  (別 `sidecar_sync` 行 or sync バージョン)** を持ち、トグル ON で fast-path を無効化してタグを再取り込みする。
- **フォルダタグはサイドカーバックアップ対象外 (UX レビュー 2-2)**: フォルダのタグは **tags.db + 世代
  バックアップ (§13.3) のみ**で保持し、`mimageviewer.dat` には書かない。サイドカーはファイル系のキー
  (親フォルダ + 小文字ファイル名) に限定する。`tag_sidecar_backup_enabled` が ON でもフォルダは対象外
  (opt-in 機能の限定で UX 影響は小さい)。

### 4.2 設定 UI (2 チェックボックス)

既存トグル `sidecar_backup_enabled` ([src/settings.rs:1701](../src/settings.rs)、既定 ON、ラベル
「フォルダに補正・マスク設定のバックアップを保存する」[src/ui_dialogs/preferences/pages.rs:2216](../src/ui_dialogs/preferences/pages.rs))
は編集データ 6 種を制御。これに **タグ用の独立トグルを新設**する:

| チェックボックス | 制御対象 | 既定 |
|---|---|---|
| □ フォルダに補正・マスク設定のバックアップを保存する (既存) | 編集データ 6 種 | ON |
| □ フォルダにタグのバックアップを保存する (新規 `tag_sidecar_backup_enabled`) | タグの `mimageviewer.dat` 書き込み | **OFF** |

既定 OFF の理由: (1) 非破壊・tags.db のみが既定という方針に一致、(2) タグは編集データと違い
**共有時のプライバシー懸念** (`ボツ` `要修正` 等の整理タグをフォルダごと渡す相手に見せたくない)。

---

## 5. 発見 (検索) — Ctrl+G/Ctrl+S から完全分離

### 5.0 なぜ分離するか (Codex レビューの帰結)

mIV タグを Ctrl+G(Tantivy) / Ctrl+S(search_index.db) へ投影しようとすると、両索引のタグ対象網羅が
歪んでいて破綻する:

| タグ対象 | Ctrl+G(Tantivy 文書) | Ctrl+S(search_index 名前) |
|---|---|---|
| Image / Video | ○ | ✗ |
| PdfFile | ○ ([src/search_walker.rs:283](../src/search_walker.rs)) | ○ |
| ZipFile | ✗ (zip 本体は walk 対象外、`continue`) | ○ |
| ConvertibleArchive | ✗ (索引外) | ✗ ([src/name_bulk_indexer.rs:329](../src/name_bulk_indexer.rs) は `.zip/.pdf` のみ) |

加えて Ctrl+G/Ctrl+S は **お気に入りスコープ + 背景インデックス前提**。タグをここに載せると
「索引済みお気に入りの中だけ」「索引が回るまで出ない」という制約を相続する。

→ **タグは2索引へ投影せず、tags.db を直引きする専用面で発見する。** これで投影の整合問題
(Codex P1 群) が消え、索引非依存・常に最新・全対象型網羅になる。

### 5.1 タグビュー (Ctrl+T + タグツールバーの検索ボタン)

mIV タグ発見の **唯一かつ統一の入口**。

- 入力: `tags.db` を直引き → 一致した `item_key` (パス) 集合 → 拡張子から GridItem を構築 →
  **通常グリッド (`render_grid`) で描画**。
  - 検証済み: 通常グリッドの描画/ナビ ([src/ui_main.rs](../src/ui_main.rs) `render_grid` /
    `handle_cell_interaction`) は GridItem に対して汎用で索引出自を仮定しない。Image/Video/ZipFile/
    PdfFile/ConvertibleArchive/**Folder** はすべて通常どおりサムネ (フォルダは代表画像) 表示・
    ダブルクリックで開閉/遷移できる (Archive は変換 → 展開 ZIP を開く、Folder はフォルダ閲覧へ遷移 →
    §8.3 の「戻る」で復帰)。
- **お気に入り登録も背景索引も不要**。tags.db はパスキー直引きで常に最新。
- 低/高カーディナリティ両対応の UI は §6.5。
- 結果グリッドにも facet が AND 合成で効く (タグビュー内でさらに別タグ/★で絞れる)。
- **クエリ空でのランディング = タグブラウザ (UX レビュー【中6】)**: Ctrl+T 直後はショートカット → 最近
  使った → 全タグ (使用件数付き、`GROUP BY tag_key` で安価) のチップ一覧を出し、クリックで結果表示。
  タグ 0 件時は空状態で付け方を案内 (「画像を選んで右クリック → タグを付ける… から付けられます」)。
  結果グリッドのタイトルに「タグ: #原神 (123 件)」と現在地を明示。詳細は §13。

`#tag` を Ctrl+G に打った場合の互換: **タグビューへ誘導**する (先頭 `#` 検出でヒント/誘導)。v1.0 の
Ctrl+G `#tag` は廃止。素のキーワード (`#` なし) でもタグへ橋渡しする導線は §13 (D17、最重要)。

キーバインドは **KeyAction / keymap 経由**で追加する (CLAUDE.md 規約、[docs/keymap-spec.md](keymap-spec.md))。
**`Ctrl+T` は既にフルスクリーンのテキスト注釈モード `FsTextMode` に割当済み**
([src/keymap.rs:1887](../src/keymap.rs))。よってタグビューは **グリッド context 限定アクション**にし、
フルスクリーンでは `Ctrl+T` を従来どおりテキストモードに残す (context で曖昧性解消)。**既定はグリッド
context の `Ctrl+T` で確定**。二義性を嫌うユーザーは keymap で別 chord (例 `Ctrl+Shift+T`) に再割当できる。

### 5.2 facet タグフィルタ (現フォルダ内・常駐) — 残すが UI 二層化

「絞り込み:」ツールバーの既存 facet フィルタ ([src/ui_main.rs:1754](../src/ui_main.rs)、
`passes_facet_filter` [src/app.rs:18656](../src/app.rs)) を**残す**。変更点:

- **プルダウンの列挙は show_shortcut タグのみ**に限定 ([src/ui_main.rs:1970](../src/ui_main.rs))。
  高カーディナリティの作者名等は並べない。
- **プルダウンにインクリメンタル検索フィールドを 1 本足す (UX レビュー【高2】)**: ショートカットタグの
  列挙に加え、§6.5 の補完 UI を再利用して全タグへ到達できるようにする。これが無いと「現フォルダの
  2,000 枚を ad-hoc タグ `klee` で絞る」最頻フロー (Eagle 移行組) が、プルダウン (show_shortcut のみ) と
  Ctrl+T (全ライブラリ) の隙間に落ちる。
- **`facet_tag_filter_applies` の対象だけを in-place に隠す** (§2.1)。`ZipImage/PdfPage/ZipDir` (コンテナ
  中身) と **`Folder` (ナビ構造) は素通り** — 現行は「タグフィルタ ON 時に非対応アイテムを除外」
  ([src/app.rs:18659](../src/app.rs)) でタグ付き ZIP を開くと中身が消えるが、素通りにすれば現フォルダ直下の
  leaf だけに効き、コンテナ/サブフォルダに入っても空にならない → ★ の `rating_filter_suppressed_at` の
  ような明示一時解除トーストは不要。フォルダのタグでの絞り込みは Ctrl+T が担う。
- facet タグの distinct 値・一致判定は **`tags.db`** から取る (索引非依存)。
- **facet の内部表現 (Codex P2)**: `FacetFilter.tags` ([src/settings.rs:479](../src/settings.rs)) は現状
  `#原神` のような表示形を `BTreeSet<String>` で永続化している ([src/settings.rs:5449](../src/settings.rs))。
  新仕様では **`tag_key` (正規化・`#` なし) を保持**し、表示時のみ `#` を冠する。**既存の永続値は設定
  読み込み時に `#` 剥がし + 正規化 (NFKC) して `tag_key` へ移行**する (リリース済み設定フィールドなので
  移行コード必須)。

### 5.3 検索＋タグ combo は facet AND 合成で足りる (ただし leaf 結果に限る)

`rebuild_visible_indices` は `search_filter`(Ctrl+F/G の一致 `HashSet<usize>`) AND `rating_filter`
AND `facet_filter` を合成して `visible_indices` を作る ([docs/details-view-and-filter-plan.md:65,177](details-view-and-filter-plan.md))。
よって「Ctrl+G/S で検索 → 結果に facet でタグ/★絞り」は既存アーキテクチャで動く。**ただし §5.2 の
「タグ非対応アイテムは素通り」方針の帰結として、効くのは付与可能な leaf/実ファイル結果だけ** (Codex P1):

- **Ctrl+G の flat / leaf ビュー**: Image/Video/PdfFile 等の leaf 結果を facet タグで絞れる。○
- **Ctrl+G の集約ビュー (`SearchContainer`)**: 集約アイテムはタグ非対応で素通りするので、タグ facet は
  実質 no-op。combo したい時は **flat 表示に切替**るか drill-in する。
- **Ctrl+S の `Folder` 結果**: フォルダは facet の in-place 隠し対象外 (素通り) → タグ条件を入れても **フォルダ結果は残る**
  (ナビゲーション用に意図的)。タグで絞れるのは Ctrl+S 結果中の ZipFile/PdfFile (実ファイル) のみ。

つまり combo の売りは **「検索結果中の付与可能 leaf をタグで絞る。集約/フォルダ等の構造アイテムは
ナビ用に残る」** と仕様化する。集約ビューでの combo は flat 切替が前提、と UI に明記。

> Ctrl+G/Ctrl+S 自体は **非タグ検索のまま** (タグは一切混ぜない、§5.4)。

### 5.4 既存 FTS のタグ経路を閉じる (Codex P1)

「タグを Ctrl+G/S に投影しない」を **原則でなく実挙動**にするには、現状ある FTS タグ経路を明示的に閉じる:

- **`SourceKind::Tags` を検索対象から外す (Codex P1)**: `SourceKind::ALL` リスト
  ([src/fts_index.rs:87](../src/fts_index.rs)) から `Tags` を除外する**だけでは不十分**。
  `SearchTarget::includes(source)` は現状 `All => true` ([src/fts_index.rs:124](../src/fts_index.rs)) を返すので、
  `includes(SourceKind::Tags)` を引く経路 (Ctrl+F の [src/app.rs:1876](../src/app.rs) /
  [src/app.rs:2076](../src/app.rs) 等) が「すべて」検索で `dc:subject` を読み続ける。**`includes(Tags)` が
  `All` でも `false` を返すように**する (または `Tags` variant 自体を撤去)。Ctrl+G の検索対象 UI
  ([src/global_search_ui.rs:112](../src/global_search_ui.rs)) の「タグ」項目も撤去。
- **ingest で `tags` フィールドを populate しない**: 通常 ingest が XMP `dc:subject` を `Tags` に
  入れる経路 ([src/ingest_text.rs:183](../src/ingest_text.rs)) を停止 (フィールド定義は残すが空に)。
- **Ctrl+F fallback の `dc:subject` 読みを外す** ([src/app.rs:2076](../src/app.rs))。
- **検索バー内のタグピッカーを撤去/置換 (Codex P2)**: Ctrl+G/Ctrl+F の `#タグ…` ボタン
  ([src/global_search_ui.rs:2127](../src/global_search_ui.rs) / [src/ui_main.rs:2785](../src/ui_main.rs)) は
  クエリへ `#tag` を挿入する導線。`Tags` 検索を外した後に残すと**壊れた導線**になるので、**撤去**するか
  **「タグビューを開く」ボタンに置換**する。
- **既存 STORED `tags` は移行専用**: 索引に残る旧値は §7.1 の一括コピー元としてのみ使い、検索には
  出さない。再 ingest で順次空になる。**移行は再 ingest より先に走らせる**。
- `SourceKind::Sidecar` (外部 JSON/TXT サイドカーの別系統メタ, [src/fts_index.rs:81](../src/fts_index.rs)) は
  mIV タグと無関係なので**そのまま**。
- **注意 (Codex P2)**: ここで消すのは **検索ソースとしての** XMP/`dc:subject` 読み。§7.2 の
  **legacy seed worker (tags.db への一度きり移行) は別経路として残す**。両者を別名にして取り違えない。

### 5.5 タグビューの stale パス処理 (Codex P2)

`tags.db` はパスキーなので、削除・移動・外付けドライブ未接続で **stale パスが必ず出る**。お気に入り
索引を不要にした分、notify-rs による自然な掃除も失う。方針:

- タグビュー構築時、**worker で存在確認**する (UI スレッドを止めない)。
- 欠落パスは **dimmed の「オフライン/欠落」セルとして表示**する (黙って消さない — 外付け未接続の
  誤判定があるため)。右クリックに「tags.db から削除」を出す。
- **欠落の表現 (Codex P2)**: 既存 `GridItem` に欠落/オフライン variant が無い。実ファイル variant
  (Image/ZipFile 等) のまま入れるとサムネ生成・ダブルクリック・削除・タグ編集が実ファイル前提で走る。
  対応はタグビュー mode 内に局所化する: **タグビューが保持する「欠落 index 集合」**で、サムネ要求・
  open(ダブルクリック)・ファイル削除を **gate (無効化)** する。一方 **タグ編集と「tags.db から削除」は
  許可**する (tags.db はオフラインでも操作可 = 外付け未接続中もタグ管理できる利点)。
  `GridItem::MissingTaggedFile` 新 variant 案もあるが、ZipSeparator 同様の全 match arm 波及コストが
  大きいので **タグビュー局所の集合 gate を推奨** (最終判断は §12)。
- **未接続時の自動全削除はしない**。確実に消えたと分かる時 (= 通常閲覧/ingest でそのフォルダが
  オンラインと確認できた時) に **日和見 prune** する。
- 手動の「タグ整合性チェック / 掃除」アクション (既存のキャッシュ管理ダイアログ同様) も用意する。

---

## 6. UI 仕様

### 6.1 タグ操作の統一

- 付与/削除/トグルは付与可能 **6 種** (画像/動画/ZIP/PDF/アーカイブ/フォルダ) で完全に同一
  (コンテナ/フォルダをグレーアウトしない)。
- [`cell_tag_list` (src/app.rs:20307)](../src/app.rs) を `tags.db` 参照に変更し 6 種対応。
- [src/ui_dialogs/context_menu.rs](../src/ui_dialogs/context_menu.rs) のタグ項目を 6 種で有効化。
- 複数選択時のトグルは **all-or-nothing** (全付与済みなら全削除、それ以外は全付与、tag-feature.md §2.3、
  D18)。**現行マニュアルの「各ファイル独立トグル」記述はこれに合わせて改める** (UX レビュー【低10】、
  出荷挙動と引用元設計の矛盾を解消)。
- **フルスクリーンでコンテナ閲覧中のタグ操作はコンテナ自身へ自動フォールバック (UX レビュー【高3】)**:
  ZipImage/PdfPage は付与不可なので、フルスクリーン中 (ツールバー/ショートカット/メタパネル) のタグ操作は
  **そのページが属する ZIP/PDF へ付与**し、トーストで「`作品.zip` に #良作 を付与」と対象を明示する。
  メタパネルのタグセクションも「この ZIP のタグ」として出す。漫画ペルソナの基本動線 (読みながらマーク)
  を切らさないための必須挙動。§6.1 の対象に明記。

### 6.2 サムネバッジ / 詳細パネル

- **サムネのタグバッジは mIV タグのみ** (他アプリ由来は出さない、D10)。表示は `#` 付き (§3.1)。
- 詳細パネル ([src/ui_metadata_panel.rs](../src/ui_metadata_panel.rs)) のタグセクションも mIV タグのみ。
  v1.0 設計にあった「他アプリ由来タグの区別表示」は**削除**。外部 `dc:subject` は (出すなら)
  EXIF 等と同列の生メタ表示に留める。
- 外部書き込みが無いので保存状態バッジ (XMP/.xmp 等) も不要。

### 6.3 環境設定

- §4.2 のタグ用チェックボックスを「設定のバックアップ」セクションに追加。

### 6.4 タグの二層モデル (低/高カーディナリティ)

タグは「少数の共有ラベル」だけでなく **作品名・作者名のような大量属性**にも使う想定なので 2 層に分ける。

**(1) ショートカットタグ (少数・キュレート)**
- 「タグの管理…」ダイアログ (語彙 = ピンの管理) で各タグに **□ メニュー・ツールバーに表示 (ピン留め)** を
  付ける (§13.2 の確定語彙。UI 文言はこちらに統一)。
- `show_shortcut = true` のタグだけが **メニュー / ツールバーのボタン + facet 絞り込みプルダウン** に出る。
- データ: `TagDef` ([src/settings.rs:148](../src/settings.rs)) は **`tag_key` に紐づくショートカット表示メタ**へ
  役割変更する (Codex P2)。実タグ語彙は `item_tags.tag_key` が正本で、`TagDef` =
  `{ id(既存 UUID), tag_key, name(display), show_shortcut, 並び順 }`。
  - **識別子 (Codex P2)**: 既存 `id`(UUID, 安定識別子) は**残す**。`tag_key` を**論理的な一意キー
    (UNIQUE NOT NULL)** とする。既存 TagDef が同一 `tag_key` に衝突したら統合する: **最小 `sort_index` の
    行を採用行**として、その `id` と `name`(表示名) を残し、`show_shortcut` は衝突行の OR、`sort_index` は
    最小値を採る (採用行が決定的になり移行結果が安定する)。
  - **改名の意味付け**: 表示名を変えても **`tag_key` が同じ (大小/全半角差のみ)** なら display 形の更新だけ
    (item_tags はそのまま)。**`tag_key` が変わる改名は別タグ扱い**で、必要なら明示的な「改名/統合」操作と
    して `item_tags.tag_key` を旧→新へ一括書換 (retag) する。これを決めないと NFKC 統合や改名で
    ショートカットと実データがずれる。
  - **表示名の優先順位 (Codex P3)**: バッジ/facet/タグビューの表示は **登録済み `tag_key` は `TagDef.name`
    を優先**し、未登録 ad-hoc は `item_tags.tag` の最新表示形を使う。これで `tag_key` 不変の改名
    (TagDef.name 変更) が item_tags を書き換えずに UI へ反映される。
  - 既存 `TagDef`(id/name) からの移行は §8.2 (name から tag_key を導出、show_shortcut=1)。

**(2) 自由記入タグ (大量・作品名/作者名)**
- §6.5 のダイアログ / Ctrl+T で付与・発見。新規文字列はその場で ad-hoc タグになる (`item_tags` に入る)。
- **ad-hoc タグは明示登録 (show_shortcut を付ける) しない限りショートカットに昇格しない**。
  作者名が何百個もツールバー/プルダウンに並ぶのを防ぐ。

### 6.5 「タグを付ける/外す…」ダイアログ (選択への付与/削除)

アイテムを選択して **「タグを付ける/外す…」**((1) の「タグの管理…」とは別物。§13.2 の確定語彙、
「編集」という語はどちらにも使わない) を押すと開く。高カーディナリティ前提の付与 UX:

- **自由記入 + インクリメンタル検索**: 入力に対し過去使用タグを前方一致で候補表示。
- **最近使ったタグの履歴表示**: 直近付与タグをワンタップ再利用。
- **現在の選択タグをチップ表示**: 個別削除可。複数選択は §6.1 のトグル規約を反映。
- 表示は `#` 付き、保存は `#` なし (§3.1)。
- IME 対応は `dialog_enter_pressed` / `dialog_escape_pressed` 必須 (CLAUDE.md 規約)。

**語彙・最近使用の供給源**: `tags.db` の `item_tags` から導出 (§8.1)。語彙 =
`SELECT … GROUP BY tag_key`、前方一致 = `tag_key LIKE ? ESCAPE '\'` (入力を NFKC 正規化 + escape、§8.1)、最近使用 =
`GROUP BY tag_key ORDER BY MAX(applied_at) DESC`。別途 MRU リストは持たない。

### 6.6 コンテキストメニュー: 過去ファイルの後始末 (任意)

- 「タグを取り込んでファイルから削除」「タグを取り込む (ファイルはそのまま)」を追加 (§7.2)。
- フォルダ/ライブラリ一括版も用意。

---

## 7. 移行 (リリース済み機能の後方互換)

v1.0 は `#タグ` を **ファイル XMP / 動画 `.xmp`** に書き、同時に Tantivy `tags` フィールドにも upsert
していた。新モデルでは `tags.db` が正本かつ唯一の発見源になるため、以下で取りこぼしと事故を防ぐ。

### 7.1 一括移行 — Tantivy `tags` → `tags.db` (主経路)

- 既存の mIV タグは **既に Tantivy `tags` フィールドに入っている** ([src/fts_index.rs:773](../src/fts_index.rs))。
  アップグレード時に **Tantivy 索引を走査し、各文書の `tags` のうち `#` 始まりの要素を `#` 剥がしで
  `tags.db` に一括コピー**する。
- **ファイル I/O なし・再スキャンなし・リバイバルなし** (Tantivy が既に持つ値を写すだけ)。
- 非 `#` の `dc:subject` 要素は **取り込まない** (D10、他アプリ由来は mIV タグでない)。
- **v1.0 のタグ付与対象は Image / Video のみ** ([src/tag_ops.rs:24](../src/tag_ops.rs))。これらの旧タグが
  主な移行対象。FTS に `#` 付きで残る他種 (PDF の `dc:subject` が偶然入っていた等) も拾って移行するが、
  網羅保証はしない (過剰保証を避ける)。ZIP/Archive は v1.0 でタグ非対応だったので移行対象なし。
- **冪等性 + フラグの置き場所 (Codex P1/P2)**: 一括移行は **一度きり**。フラグ `legacy_tantivy_imported`
  は **`tags.db` 内の `tag_meta` テーブル**に置く (設定 DB に置くと tags.db のバックアップ復元で状態が
  ずれる)。**Tantivy 全走査の完了と同一トランザクション**で立て、再起動で再実行しない (旧 STORED
  `tags` からの復活を防ぐ)。
- **起動順の固定 (Codex P2)**: この一括移行は **FTS 再構築 (旧 STORED `tags` を消し得る) より前**に
  走らせる。順序 = legacy 一括移行 → (必要なら) FTS 再構築 / 通常 ingest。
- **取り込んだ各 `item_key` に `tag_item_state` (source='tantivy_migration') を記録**する (§7.2 と同じ台帳)。これで §7.2 の遅延 seed が
  「移行済み」と判定してファイル XMP を読み直さず、ユーザーが `tags.db` で削除したタグが復活しない。
  一括移行 (§7.1) と遅延 seed (§7.2) はこの単一台帳を共有する。

### 7.2 遅延取り込み + リバイバル防止 (保険経路)

お気に入り外などで **Tantivy に索引されていないファイル**の v1.0 埋め込み `#タグ` は §7.1 に乗らない。
保険として、フォルダを開いた時に **専用の legacy seed worker** がファイル XMP の `#タグ` を読み
`tags.db` に seed する (worker 内、UI スレッド I/O なし)。

- **§5.4 の「検索側 dc:subject 読みの撤去」とは別経路 (Codex P2)**。§5.4 が消すのは **検索ソース**
  (FTS `Tags` フィールド / Ctrl+F fallback) の XMP 読みであって、ここの **legacy seed worker は
  `tags.db` への一度きり移行専用**として残す。実装時に「XMP 読みを全部消して未索引旧タグを拾えなく
  する」事故を防ぐため、両者を**明確に別名・別経路**にする。
- **mIV タグの seed は 1 ファイルにつき一度きり (import-once-ever)**。`tag_item_state(item_key)` を見て
  在れば再読込しない (§7.1 と同一台帳を共有)。
- **`tag_item_state` は「XMP 読み取り成功」時に記録する (Codex P2、source='xmp_legacy')**: `#` タグが
  **見つからなくても**記録する。さもないとタグなしファイルをフォルダ表示のたびに legacy seed worker が
  読み直す実装になりやすい。**I/O エラー時は未記録**にして次回再試行対象とする。
- **ファイル mtime が変わっても再 seed しない** (他アプリの後付け編集で旧タグが復活するのを防ぐ)。
  再取り込みは §7.3 の明示コマンド経由のみ。

### 7.3 ファイルからの除去 (明示・任意)

- 右クリック「タグを取り込んでファイルから削除」で、ファイル内の `#タグ` を除去 (+ 余った
  動画 `.xmp` 削除)。アトミック書き込み ([src/xmp_writer.rs](../src/xmp_writer.rs) を再利用)。
- 「取り込むだけ (ファイルは変更しない)」も併置。フォルダ/ライブラリ一括版も。
- **`tag_item_state` を bypass する (Codex P2)**: §7.2 の自動 seed は state が在れば skip するが、**この明示
  コマンド (「取り込むだけ」「取り込んで削除」) は state が在っても実行**する。完了後に `item_tags` と
  `tag_item_state` を**同一トランザクション**で更新する。
- **取り込みは union (Codex P3)**: 「取り込む」系はファイル内 `#タグ` を **既存 tags.db タグへ union** する
  (置換しない)。「取り込む」の直感に沿い、ユーザーが mIV 側で足したタグを消さない。
- `tags.db` が正本になった後、ファイルに残った `#タグ` は **発見に影響しない** (mIV はもう
  ファイルのタグを検索源にしない)。よって (b) は「ファイルを綺麗にしたい人向け」の任意機能。

---

## 8. 既存コードへの影響 (実装マップ)

### 8.1 新規

- **`src/tags_db.rs`**: `tags.db` の CRUD。`rating_db.rs` / `comic_db.rs` を雛形に。スキーマ (D11):
  ```sql
  item_tags(
    item_key   TEXT    NOT NULL,   -- normalize_path (§2.2)
    tag        TEXT    NOT NULL,   -- 表示用 (# なし、入力時の表記を保持)
    tag_key    TEXT    NOT NULL,   -- normalize_tag_key() = trim → NFKC → lowercase。照合/語彙/前方一致用
    applied_at INTEGER NOT NULL,   -- 最近使用の導出元
    PRIMARY KEY(item_key, tag_key) -- rowid table では複合 PK でも NULL を完全には防がないため各列も NOT NULL
  );
  CREATE INDEX idx_item_tags_tagkey ON item_tags(tag_key);
  tag_item_state(                                   -- §4.1/§7 「タグ決定済み」マーカー (復活防止)
    item_key   TEXT    PRIMARY KEY,
    decided_at INTEGER NOT NULL,
    source     TEXT    NOT NULL                     -- 'edit'/'tantivy_migration'/'xmp_legacy'/'sidecar'
  );
  tag_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);  -- legacy_tantivy_imported 等の全体フラグ (§7.1)
  ```
  - **`tag_key` 正規化は単一関数 `normalize_tag_key()` = trim → NFKC → lowercase に固定** (D11, Codex P3)。
    NFKC で全半角/互換文字を統一し `ＦＡＴＥ` / `FATE` 重複を防ぐ。tags.db は FTS 非依存なので採用は容易。
    **統合規則**: 移行/付与時に同一 `tag_key` へ正規化される表示違いが出たら、表示形 1 つ (最新
    `applied_at`) に寄せ、適用 (item_tags 行) を union する。
  - **`tag_item_state` の立て方 (Codex P2)**: 「この item_key のタグは tags.db が決定済み」を表す行。
    通常編集 (付与/削除/全クリア)・§7.1 一括移行・§7.2 legacy XMP seed・§4.1 sidecar import の各経路で、
    その item_key を処理した時点で upsert する。**legacy seed と sidecar import はこの行が在れば skip**する
    (タグを全削除して空になった item も復活させない)。旧 `xmp_migrated` はこの表に統合 (source='xmp_legacy')。
  - **前方一致検索のワイルドカード対策 (Codex P3)**: タグ名は `%` / `_` を含み得るので、入力を同じく
    NFKC 正規化した上で `%`/`_`/`\` を escape して `LIKE ? ESCAPE '\'` で引く (既存
    [src/adjustment_db.rs:327](../src/adjustment_db.rs) `escape_like_pattern` を再利用) か range query にする。
- **`src/tags_db.rs` を引く「タグビュー」**: タグ照会 → item_key 集合 → GridItem 構築 →
  通常グリッド populate (§5.1)。Ctrl+T (KeyAction) + タグツールバー検索ボタン。
- 選択アイテムのタグ編集ダイアログ (§6.5)。
- 設定: `tag_sidecar_backup_enabled: bool` (既定 false) + チェックボックス。`TagDef` は
  **`{ id(既存 UUID), tag_key(UNIQUE NOT NULL), name(display), show_shortcut, 並び順 }`** に拡張 (§6.4)。
  **`tag_key` は保存する** (`name` から毎回導出しない) — さもないと「tag_key を変えない表示名変更」が
  できない。既存 `TagDef`(id/name) からは name→tag_key 導出 + show_shortcut=1 で移行 (settings.json 経路も
  含む、§8.2)。
- **`tags.db` 世代バックアップ** (§13.3 / D19): settings.db の `bak1..bakN` 機構
  ([src/settings_db.rs](../src/settings_db.rs)) を踏襲。外部書き出しはしない。

### 8.2 変更

- [`facet_item_supports_tags` (src/app.rs:1497)](../src/app.rs): **2 述語に分割・改称** (§2.1)。
  `item_supports_tags` = 6 種 (Image/Video/ZipFile/PdfFile/ConvertibleArchive/**Folder**、付与/タグビュー/
  バッジ)、`facet_tag_filter_applies` = 5 種 (Folder と コンテナ中身 ZipImage/PdfPage/ZipDir は in-place
  隠し対象外 = 素通り)。
- [`passes_facet_filter` (src/app.rs:18656)](../src/app.rs): タグフィルタ ON 時にタグ非対応アイテムを
  **除外しない (素通り)** (§5.2)。distinct/一致は `tags.db` から。
- [facet タグメニュー (src/ui_main.rs:1970)](../src/ui_main.rs): プルダウン列挙を show_shortcut のみに。
- メニュー/ツールバーのタグボタン: show_shortcut のみ (tag-feature.md §4.2/§4.3 を更新)。
- [src/tag_write_worker.rs](../src/tag_write_worker.rs): 書き込み先を **`tags.db` のみ**に。**Tantivy upsert /
  XMP 書き込みは通常タグ付けから外す** (D13)。`xmp_writer` のファイル書き込みは §7.3 の「削除」専用に縮退。
- **既存 FTS タグ経路の閉鎖 (§5.4)**: `SourceKind::ALL` から `Tags` 除外 ([src/fts_index.rs:87](../src/fts_index.rs))、
  **`SearchTarget::includes(Tags)` を `All` でも `false` に** ([src/fts_index.rs:124](../src/fts_index.rs)、これが
  抜けると Ctrl+F が「すべて」で `dc:subject` を読む)、Ctrl+G 検索対象 UI のタグ項目撤去
  ([src/global_search_ui.rs:112](../src/global_search_ui.rs))、ingest の `tags` populate 停止
  ([src/ingest_text.rs:183](../src/ingest_text.rs))、Ctrl+F の `dc:subject` 読み停止 ([src/app.rs:2076](../src/app.rs))。
- **タグビューの stale パス処理 (§5.5)**: worker 存在確認 + 欠落 index 集合 gate + 日和見/手動 prune。
- **`settings.db` の tags テーブル移行 (Codex P2)**: `show_shortcut` と **`tag_key`** を追加。現行は
  `id/name/sort_index` のみで read/write も id/name だけ ([src/settings_db.rs:1080](../src/settings_db.rs) /
  [src/settings_db.rs:1275](../src/settings_db.rs))。`ALTER TABLE tags ADD COLUMN show_shortcut INTEGER
  NOT NULL DEFAULT 1` (**既存タグは 1 = 従来どおりショートカット表示**で後方互換) + `ADD COLUMN
  tag_key TEXT` (既存行は `normalize_tag_key(name)` で埋め、衝突する既存タグを統合してから
  `CREATE UNIQUE INDEX tags_tag_key ON tags(tag_key)` で UNIQUE 化)、`write_tags`/`read_tags` に両列を反映、
  [docs/settings-sqlite-migration.md](settings-sqlite-migration.md) 更新。**リリース済みスキーマなので
  移行必須** (CLAUDE.md 永続データ方針)。
  - **`tag_key` の NOT NULL (Codex P3)**: SQLite の `ADD COLUMN` では UNIQUE/NOT NULL を後付けできない。
    一意は上記 unique index で担保し、**NOT NULL は read/write validation で保証**する (`write_tags` は常に
    `normalize_tag_key(name)` を書き、`read_tags` は NULL を name から補完)。厳密にするならテーブル再構築
    (新スキーマ NOT NULL + copy + swap) も可。
  - **settings.json 経路も補完 (Codex P2)**: `migrate_from_settings_json`
    ([src/settings_db.rs:1942](../src/settings_db.rs) / [src/settings_db.rs:1966](../src/settings_db.rs)) は
    JSON 読込後に load-time migration を適用する。`TagDef` の新フィールドは **serde default** で旧 JSON を
    吸収する (`tag_key` は空 → 後で `normalize_tag_key(name)` 補完、`show_shortcut` は既定 true)。
- **旧 Tantivy 一括移行の挿入点 (Codex P2)**: [src/indexer_manager.rs:139](../src/indexer_manager.rs) で
  `FtsMetaDb` を開いた後、`rebuilt_on_open()` だと [src/indexer_manager.rs:148](../src/indexer_manager.rs) で
  `fts_index` を wipe する。**この wipe より前に**、`tag_meta.legacy_tantivy_imported` が未設定なら旧
  `fts_index` を read-only で開いて `#` タグを `tags.db` へ import + フラグ設定する (§7.1)。wipe 後だと旧
  STORED tags が消えて移行できない。
- **`FacetFilter.tags` の移行 (§5.2)**: 永続値 `#原神` → `tag_key` (strip `#` + NFKC 正規化)。
  settings.json / settings.db 両経路。リリース済み設定フィールドなので移行必須。
- **検索バーのタグピッカー撤去/置換** (§5.4): [src/global_search_ui.rs:2127](../src/global_search_ui.rs) /
  [src/ui_main.rs:2785](../src/ui_main.rs)。
- [`cell_tag_list` (src/app.rs:20307)](../src/app.rs) / `tags_cache` ([src/app.rs:3710](../src/app.rs)) /
  [src/tag_ops.rs](../src/tag_ops.rs): 読み出し元を `tags.db` に、**6 種対応 (フォルダ含む)**、表示は `#` 付き・
  保存は `#` なし。
- [src/sidecar.rs](../src/sidecar.rs): `SidecarEntry` に `tags`、`tag_sidecar_backup_enabled` で gate、
  実ファイル 5 種のキー拡張 (§4.1)。
- [src/ui_metadata_panel.rs](../src/ui_metadata_panel.rs): タグセクション mIV のみ、外部タグ区別表示を削除。
- 移行: Tantivy `tags` 走査 → `#` 剥がし → `tags.db` 一括コピー (§7.1)。

### 8.3 留意 (実装時に確認)

- `tag_write_worker` の undo (`pending_tag_undos`) を `tags.db` ベースに。
- 複数選択トグルでコンテナと画像が混在した場合の全付与判定の母数を定義。
- ConvertibleArchive は変換前後でパスが変わる。タグキーは**元アーカイブのパス**に紐づけ、
  [src/archive_cache.rs](../src/archive_cache.rs) のマッピングと整合を取る。
- タグビュー結果からコンテナを開いた後の「戻る」挙動を既存の検索結果ナビと揃える。

---

## 9. 関連クリーンアップ

- **`GridItem::ZipSeparator` 撤去** (別バックログタスク `task_7f616358`)。v1.3.0 のネスト ZIP ツリー化で
  生成されなくなったレガシー variant。本設計の付与対象表からも除外済み。

---

## 10. 非対象 / 将来

- **フォルダへのタグは v1 で対応に変更** (UX レビュー、D2)。引き続き非対象は **ZIP内画像 / PDFページ**
  (アーカイブ内の仮想エントリ)。タグビューは tags.db 直引きなので将来広げても一時解除不要で安価。
- 他アプリ由来 dc:subject / IPTC Keywords でファイルを探す機能 (タグ system とは独立した小機能として)。
- **interop 書き出し (XMP/CSV/JSON) は v1 非対象**: UX レビュー【中5】はロックイン不安に言及するが、
  ユーザー判断で v1 は **tags.db 世代バックアップのみ** (§13/D19) で対応し外部書き出しはしない (非破壊・
  外部書き込み無し方針の維持)。将来「タグを XMP/CSV にエクスポート」を明示コマンドとして検討余地。
- 階層タグ (`lr:hierarchicalSubject`)、動画本体への interop 書き込み再導入も将来検討。

---

## 11. 実装着手時に更新するドキュメント

- [docs/tag-feature.md](tag-feature.md): 本設計への置き換えを冒頭で参照 (v1.0 として保存)。
- [docs/spec.md](spec.md) / [docs/architecture-overview.md](architecture-overview.md): タグ仕様・`tags_db` 追加。
- [docs/virtual-folders.md](virtual-folders.md): 付与対象表 (コンテナ + **フォルダ**)・キー規則。
- [docs/preset-and-adjustment.md](preset-and-adjustment.md): サイドカー二層にタグが加わる点。
- [docs/search-architecture.md](search-architecture.md): タグは Ctrl+G/Ctrl+S から分離・Ctrl+T タグビュー。
- [docs/keymap-spec.md](keymap-spec.md) / [docs/keymap.ini.default](keymap.ini.default): Ctrl+T (タグビュー) 追加。
- [htdocs/mimageviewer/manual/](../htdocs/mimageviewer/manual/) / [htdocs/mimageviewer/index.html](../htdocs/mimageviewer/index.html):
  ユーザー向け記述 (バージョンタグ・実装語を出さない方針に従う)。タグページに以下を明記 (UX レビュー):
  (a) **フォルダへのタグはフォルダ自身に付き、中のファイルには伝播しない** (2-5)、(b) **タグの保存先
  (この PC の mIV 内) とバックアップのされ方** (旧版「Lightroom でも読める」との差で誤解を防ぐ、§3 所見)、
  (c) 複数選択 all-or-nothing と facet のフォルダ素通り挙動。

---

## 12. オープン課題 (実装時に詰める)

- タグビューのクエリ構文: 複数タグ AND / 除外 / (将来 OR) の指定方法と、ショートカットチップ +
  インクリメンタル検索フィールドの併存レイアウト。
- `tag_key` 正規化は **`normalize_tag_key()` = trim → NFKC → lowercase で確定** (§8.1)。残課題は NFKC
  依存追加と、既存タグが同一 key に統合される場合の表示形マージ実装のみ。
- タグビューの chord は **既定 = グリッド context の `Ctrl+T`** に確定 (フルスクリーンは `FsTextMode`
  のまま、context で曖昧性解消、[src/keymap.rs:1887](../src/keymap.rs))。二義性を嫌うユーザーは keymap で
  `Ctrl+Shift+T` 等へ再割当可能。残るのは欠落セル表現を局所集合 gate にするか variant にするかの選択のみ。
- 移行(§7.1) の Tantivy 走査コスト (大規模ライブラリでの一括コピー時間) と進捗 UX。
- 大量一括付与時の進捗 UX / キャンセル。
- タグビュー結果の並び順 (パス順 / 付与日時順) の既定と切替 (UX レビュー【中6】)。
- facet でフォルダ/コンテナ中身を素通りさせる挙動と ★ フィルタの一時解除の差を、マニュアルで一言説明。
- **Ctrl+G 橋渡しの照合規則** (§13.1、UX 2-7): `原神 風景` のような複数語クエリで各トークンを個別照合
  するか / 完全一致か / 前方一致か。実装時に確定。
- **facet タグ一致フォルダの視覚フィードバック** (UX 2-6、将来磨き込み): 素通り (隠さない) のままだと
  「タグを選んでもフォルダ一覧が減らない=効いてない」に見える場面が残る。将来「非一致フォルダを隠さず
  **dim** する」案を検討 (ナビ維持と視覚フィードバックの両立)。

---

## 13. UX レビュー反映: 体験層の橋渡しと運用 UI

> 別 Claude による UX レビュー (3 ペルソナ = AI 画像整理 / 漫画 / 写真) の結論:
> **方向性 (非破壊・DB 正本・検索からの分離) は妥当で根本再考は不要。ただし D4 の「データ層の分離」が
> 「体験層の断絶」のまま出ると、利用者は『自分のタグが検索に出ない＝壊れている』と誤解する。**
> 内部の分離は保ったまま、表示層で数本の導線を足して分離を透明にする。以下を v1 に含める。

### 13.1 体験層の橋渡し (D17、最重要)

- **Ctrl+G で素のキーワード (`#` なし) を打った時の橋渡し**: 索引への投影はゼロのまま、検索実行時に
  クエリ語を `tags.db` に **1 回だけ照会**し、一致タグがあれば結果一覧の上部 (またはゼロヒット画面) に
  「タグ `#原神` に 123 件 → タグビューで表示」のワンクリック導線を出す。tags.db 直引き 1 クエリで済み、
  D4 を一切壊さない。**この再設計で最も費用対効果の高い 1 手**。
- **チップ/バッジのクリック = そのタグで検索** (削除ではない): メタパネルのタグチップ・サムネのタグ
  バッジのクリックは **タグビューでそのタグを開く**。削除は × ボタンに分離 (誤操作減 + booru/Eagle の
  期待に一致)。右クリックメニューにも「このタグで探す」を置く → Ctrl+T を知らない人もタグビューに到達。
- **メニューに「タグビューを開く」を必ず置く** (Ctrl+T の文脈分裂対策。キーを知らなくても到達可能に)。

### 13.2 タグ運用 UI

- **二層は「型」でなく「ピン」として見せる** (UX レビュー【中7】): 実体は単一タグ + `show_shortcut`
  属性。ユーザー向け語彙は「**よく使うタグをピン留めするとメニュー・ツールバー・絞り込みに出ます**」に
  統一。チェックボックス文言も「□ メニュー・ツールバーに表示 (ピン留め)」。タグ編集ダイアログ/タグ
  ビューから**その場でワンクリックでピン留め**できる導線を足す (昇格を自然に)。
- **ダイアログ名は動詞で分離** (UX レビュー【中8】、v1.0 の「タグを編集…」二義性の解消): 選択への
  付与/削除 = **「タグを付ける/外す…」**、語彙の管理 = **「タグの管理…」**。「編集」はどちらにも使わない。
- **改名・統合 UI を v1 必須に** (UX レビュー【中9】): 「タグの管理」に **改名** (表示名変更、tag_key 不変なら
  §6.4 どおり display 更新のみ) と **統合** (A を B に併合 = `item_tags.tag_key` を一括 retag) を入れる。
  高カーディナリティ運用では表記ゆれ・タイポが必発 (`鈴木`/`すずき`) で、Hydrus/digiKam/Eagle 全てが持つ
  table stakes。タグブラウザの件数表示 (§5.1) で重複候補も見つけやすくなる。
- **入力先頭の `#` は弾かずに剥がす** (UX レビュー【低10】): 表示が常に `#` 付きなので利用者は `#原神` と
  打ちがち。v1.0 の「`#` を含む名前はエラー」は踏襲せず、先頭 `#` は黙って除去する。

### 13.3 タグ資産の保全 (D19)

- **`tags.db` に settings.db 同様の世代バックアップ** (`bak1..bakN`) を付ける。「DB が壊れたら / PC を
  替えたらタグはどうなる」というロックイン不安への最低限の答え。**外部書き出し (XMP/CSV) は v1 では
  しない** (非破壊・外部書き込み無し方針の維持。将来の明示エクスポートは §10)。

### 13.4 据え置き判断 (レビュー指摘のうち v1 で見送るもの)

- 名前空間風タグ (`作者:鈴木`) は前方一致検索で自然に成立する。将来のソート/グルーピング検討時に
  この慣習を壊さないようメモ (UX レビュー【低10】)。

### 13.5 レビューが評価した「残すべき判断」

非破壊既定 / 索引・お気に入り非依存の即時性 / コンテナ丸ごとタグ / stale パスの丁寧な扱い (§5.5) /
`#` を表示専用に格下げ (D9) / 他アプリ dc:subject を取り込まない (D10) / 二層の実体 (単一タグ + ピン)。
これらは UX レビューで明示的に「残すべき」とされた。実装で崩さないこと。

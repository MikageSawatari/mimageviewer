# 本のページ数バッジとページ数順 (§1.883)

最終更新: 2026-11-24

状態: 設計第4版。第1版 (`7aa5c6918`)・第2版 (`5fed5048d`)・第3版 (`e6d109cbb`) は独立設計レビュー
(Sol / xhigh) 1〜3 回目でいずれも REVISE。第2版からは、指摘を仕組みを足すのではなく状態を減らす形で直している。
第4版 (`d93388e71`) はレビュー 4 回目で REVISE。第5版 (`2fbcdef49`) は 4 回目の指摘を反映し、さらに利用者の決定
(2026-11-24、統括経由「ページ数バッジ・ソートまでいれて、次の5.7.0リリースへと進めたいと思います。」) に従い、
ページ数順を同じ版に入れる前提で §6 を設計し直した。第5版はレビュー 5 回目で REVISE、第6版 (`042c98a7b`) は
6 回目で REVISE (残り 1 件)。第7版 (`9f7148eda`) はレビュー 7 回目で **ACCEPT_WITH_NITS** (P1・P2 なし、P3 1 件)。
第7.1版はその P3 を直した版で、設計の独立レビューは完了。§7 の利用者の判断は 2026-11-24 にすべて推奨どおりと決まった。**収録版は v5.7.0**。
正本のバックログは [next-release-backlog.md](next-release-backlog.md) §1.883。

## 1. 目的と範囲

利用者の要望 (2026-11-24): 動画・音声の長さのバッジ (`thumb_show_media_duration`、既定 ON) と同じように、
本のサムネイル右下にページ数を出したい。既定 ON で、設定で ON/OFF できる形。長さのバッジとは別の設定にする。
あわせて、長さ順 (§1.762、[media-duration-sort-filter-plan.md](media-duration-sort-filter-plan.md)) と体験を
そろえるため、ページ数でも並べ替えられるとよい (利用者の発案)。

本書は 2 段に分けるが、どちらも v5.7.0 に入れる (利用者決定 2026-11-24)。

- **段 1 (§4)**: サムネイル表示のページ数バッジと、その設定。
- **段 2 (§6)**: 一覧の並べ替え (`SortOrder`) の「ページ数順」。

段 2 は段 1 と費用も永続データへの影響も違う。段 1 は既存の取得経路と記録を使うだけで済むのに対し、
段 2 は §1.762 の長さ順と同じ仕組み (一覧全体の取得、完了の証明、`SortOrder` の新しい値による旧版との
設定の境界) を要る。段 2 は長さ順の部品を「後から分かる並べ替えの値」として共通にし、ページ数を 2 つ目の
値として乗せる (§6.1)。実装は段 1 → 段 2 の順にコミットを分ける。

## 2. 現状 (コードで確認、2026-11-24 master `b7be127da`)

### 2.1 ページ数の取得と記録

- ページ数は詳細表示の「ページ数」列と選択情報 (ツールチップ) で使っている。取得するのは既存の
  details-meta worker (`src/app/metadata_ops.rs` の `load_details_page_count_with_pdf_enumerator`)。
  UI はメモリの `details_lazy_meta` の `page_count` / `page_count_checked` / `page_count_failed` だけを読む
  (`App::details_page_count_text`、`src/app.rs`)。
- 対象は `App::details_item_supports_page_count` が決める: `ZipFile`・`PdfFile` (EPUB を含む)・`Folder`・
  `ZipDir { is_archive: true }` (ZIP 内の書庫)・`ConvertibleArchive { format: Rar }` (直接閲覧の RAR/CBR)。
  7z・LZH などの変換書庫は対象外。
- 種類ごとの数え方と記録 (どれも worker 上、`GlobalIoSemaphore` の permit を取ってから I/O):

  | 種類 | 数え方 | 記録 (フォルダ単位の catalog) | 再利用の条件 |
  | --- | --- | --- | --- |
  | ZIP | `zip_loader::enumerate_image_entries_detailed_with_cancel` (中央ディレクトリ) | `container_page_meta` (kind=Zip) | 名前・mtime・size・判定設定 fingerprint の完全一致 |
  | 画像フォルダ | `folder_scan::image_folder_page_count` (直下の read_dir) | `container_page_meta` (kind=Folder)。本でないフォルダも `page_count=NULL` で記録 | 名前・フォルダの mtime・fingerprint の完全一致 |
  | RAR/CBR | `rar_loader::inspect_for_direct_read` (header)。表示の種類を絞っているときだけ一覧も読む | `container_page_meta` (kind=Archive) | ZIP と同じ |
  | PDF | PDFium ワーカーで列挙 | `pdf_meta`。PDF のサムネイル作成でも書かれる (`thumb_loader.rs`) | 名前・mtime・size の完全一致。パスワードが要る PDF は保存済みパスワードが無ければ数えない |
  | EPUB | 変換済みの PDF を PDF と同じ経路で数える (`pdf_loader::resolve_read_target`) | `pdf_meta` (変換世代の stamp) | 変換世代の stamp の一致 |
  | ZIP 内の書庫 (`ZipDir`) | 外側 ZIP を開いたときの `ZipTree` から数える (`warm_page_count`)。I/O なし | 記録しない | — |

- fingerprint は `folder_scan::image_page_recognition_fingerprint` (認識する拡張子・Susie の有効状態など)。
  設定が変わると記録は使われず、数え直す。
- `container_page_meta` と `pdf_meta` は **リリース済み** (2026-07 のページ数列から。§1.308 より前)。
  catalog の取り扱いは [catalog-design.md](catalog-design.md)、[architecture-overview.md](architecture-overview.md) の `catalog.rs` 行。
- catalog が壊れている・書けないときは、記録を使わずに元のファイルから数える (表示は失敗させない)。

### 2.2 サムネイル表示での段階取得

- サムネイル表示で details-meta を可視範囲 + 先読み範囲 (`keep_set_sorted()`) だけ段階的に取る仕組みは、
  長さのバッジと再生位置メーターのために既にある (`VisibleStage`)。入口は `App::thumbnail_media_metadata_enabled()`
  (サムネイル表示で `thumb_show_media_duration || thumb_show_resume_meter`)。
- `thumbnail_media_metadata_enabled()` を使っている箇所は `src/app.rs` に 8 か所ある。7 か所は「サムネイル表示で
  段階取得を回すか」の判定 (`details_lazy_uses_visible_stages`・`details_visible_stage_needs_load`・
  `refresh_thumbnail_details_stage` の 2 か所・`details_lazy_columns_visible`・`selection_info_only_lazy_load`・
  `start_details_meta_load_for_scope`)、1 か所は「動画・音声の長さを取るか」のフィールド判定
  (`lazy_load_video_meta_for_idx`)。
- ページ数のフィールド判定 `lazy_load_page_count_for_idx` は、サムネイル表示では「選択情報の対象」か
  「AI facet の取得中で、選択情報がページ数を要求している」ときだけ真になる。つまり今のサムネイル表示では、
  選んだ 1 冊以外のページ数は数えない。
- 段階取得は idle gate (`PREFETCH_IDLE_THRESHOLD`) の後で範囲を取り直し、scroll ごとに全件を走査しない
  ([ui-responsiveness.md](ui-responsiveness.md) §4.3)。

### 2.3 右下の配置

`src/thumb_overlay_layout.rs` の `layout_badges` が右下を次の順で決める。

1. 先に置かれた各バッジ (種類アイコン・スタック数・左上・左下の名前や形式・ブックマーク題名) を占有に入れる。
2. **フィルタの一致数** (`FilterMatchCount`、評価の絞り込み中に本・フォルダへ出す、`is_container_ratable` の
   項目だけ) が右下の角を取る。
3. **長さ** (`MediaDuration`) を右下に置き、ぶつかるものがあれば上へずらす。入らなければ出さない
   (`place_bottom_right`)。
4. 下端の読書位置のメーター (`book_resume_meter`) の帯は、バッジより先に下端を予約する (`bottom_reserve`)。
   右下のバッジはその帯の上に置かれる。

長さのバッジは動画・音声だけ、フィルタの一致数と読書位置のメーターは本・フォルダに出る。
ページ数のバッジは本に出るので、**一致数・メーターとは同じセルに並び得るが、長さとは並ばない**。

### 2.4 設定

- `Settings::thumb_show_media_duration` (既定 true)。環境設定「サムネイル」の
  `draw_media_duration_settings` (`src/ui_dialogs/preferences/pages.rs`)。設定の書き出し / 読み込みは
  `src/settings_transfer.rs` の 1 行。変更時は `preferences.rs` の `thumbnail_media_requirements_changed` で
  取得の要件を作り直す。
- 旧版は未知の設定 field を無視する ([media-duration-sort-filter-plan.md](media-duration-sort-filter-plan.md) §2.3)。

## 3. 状態を減らす検討 (CLAUDE.md「設計の簡素化」「まれな失敗は既定で割り切る」)

- **新しい取得経路・記録・worker を作らない**。数えるのは既存の details-meta worker、記録は既存の
  `container_page_meta` / `pdf_meta`、表示が読むのはメモリの `details_lazy_meta` だけ。詳細表示のページ数列・
  選択情報と同じ値を出すので、3 つの表示で数が食い違う組み合わせが生じない。**採用。**
- **数える範囲は可視範囲 + 先読み範囲だけ** (既存の `VisibleStage`)。一覧全体を数えない。全体を数える理由は
  並べ替え (段 2) だけで、バッジには要らない。**採用。**
- **数え終わる前は何も出さない** (長さのバッジと同じ)。「…」などの読み込み中の表示を足すと、表示の状態が
  「未取得 / 取得中 / 数えた / 失敗」の 4 つに増え、段階取得の Ready が範囲ごとである (詳細列では Ready でも
  「…」を出している) ことと組み合わさる。出さなければ「数が分かっている / いない」の 2 つで済む。**採用。**
- **数え終わったら並びは動かさない**。バッジは表示だけで、並び順に関わらない (並べ替えは段 2)。
- **記録 (catalog) の失効は既存のまま** (mtime・size・fingerprint の一致判定)。メモリの値には「取り直すか」を
  決めるための最小の identity (判定設定の fingerprint と、EPUB だけ一覧の世代) を持たせる (§4.3.1)。今のメモリは
  元ファイルの mtime / size と PDF パスワードの revision だけで照合しており、判定設定の変更や EPUB の変換の後も
  古い値を使い続ける (レビュー 1 回目 P2-1。詳細表示のページ数列にもある以前からの問題)。
  - 第2版では EPUB の変換キャッシュの公開・削除ごとに上げる revision を置いたが、公開は worker の中で commit され、
    UI の受理を経ない経路 (Remote が先に owner を外す場合) や、読み取りの経路から到達する `detach_missing` まで
    追う必要があった (レビュー 2 回目 P2-2)。**EPUB の値を一覧の世代の中だけで有効にすれば、この境界を一つも
    追わずに済む**ので、第3版は revision を置かない。
  - **「取り直すか」と「何を表示するか」を分ける**。取り直しの判定は identity を見るが、表示と並べ替えは
    取り直しが終わるまで最後に数えた値を出す。identity が変わるたびにバッジが消えて出直す状態 (一覧の世代は
    削除・名前変更でも進む) と、Ready のまま「…」が残る状態 (レビュー 2 回目 P2-1) を作らない。
- **設定の切り替えは既存の要件の作り直し (`invalidate_details_meta_requirements`) に乗せる**。表示中の一覧を
  作り直さない。OFF にしたら描画しないだけで、メモリの値は捨てない。
- まれな失敗 (catalog が開けない・書けない、数える途中の I/O エラー) は既定の割り切り: 既存どおり、記録を
  使わず元から数える / その本はバッジを出さない (`page_count_failed`)。ログは既存の details-meta のまま。
  再試行や失敗の種類分けは足さない。

## 4. 段 1: ページ数バッジ

### 4.1 対象と値

- 対象は `details_item_supports_page_count` が真の項目 (§2.1 の表) から、ドライブ一覧 (`items_are_drive_list`) を
  除いたもの。詳細表示の列と同じ述語を使い、バッジ専用の判定を作らない。
- 値は `details_lazy_meta` の `page_count` (`page_count_checked` かつ `Some`)。`None` (本ではないフォルダ・
  パスワードの要る PDF・失敗) はバッジを出さない。0 ページもバッジを出さない (画像フォルダの判定で 0 は
  本でない扱い。PDF は 0 を `None` にしている)。
- 画像フォルダは「画像のみのフォルダを本として扱う」の ON/OFF に関わらず数える (詳細列と同じ、
  `page_count_options_do_not_depend_on_image_folder_auto_open`)。普通の写真フォルダにも枚数のバッジが出る。
  → §7 の問 2 で確認する。
- `ZipDir` (ZIP 内の書庫) は、表示中の外側 ZIP の木 (`zip_nav` の `ZipTree`) の値を**その場で読む** (I/O なし、§4.9)。
  details-meta のメモリには入れない (レビュー 5 回目 P2-2)。木は外側 ZIP を開いたときの認識規則で作られ、
  開き直せば作り直されるので、ページ数の所有者は木にする。メモリに fingerprint の印を付けて持つと、Susie を
  再読み込みした後も古い木の数を新しい印で持ち続けてしまう。

### 4.2 数える費用 (見立て、未計測)

初めて見るときの 1 冊あたりの I/O。記録がある 2 回目以降は、どの種類も catalog の 1 行の SELECT だけ
(フォルダの catalog への接続は worker の `ContainerCatalogCache` が持ち回る)。

| 種類 | 初回の I/O | 備考 |
| --- | --- | --- |
| ZIP | 末尾の中央ディレクトリを読む (ファイル数に比例)。**中に ZIP がある合本は、内側の ZIP を全部読み出して展開し、再帰で数える** (`zip_loader.rs` の `enumerate_recursive_with_limits`。展開したバイト列は `NESTED_CACHE` に入る) | サムネイルが記録済みのフォルダでは、ページ数のために初めて ZIP を開くことになる。入れ子の合本は 1 冊で書庫の大部分を読む |
| 画像フォルダ | 直下の read_dir 1 回 | 本でない普通のフォルダも 1 回数えて `NULL` を記録する。ネットワーク共有では 1 回ごとの往復が重い |
| RAR/CBR | header の走査。表示の種類を絞っているときは一覧も | ZIP より重い (書庫全体の header を読む) |
| PDF | 多くは `pdf_meta` が既にある (サムネイル作成で書かれる)。無ければ PDFium ワーカーで列挙 | ワーカーは別プロセス |
| EPUB | 変換済みのキャッシュがあれば PDF と同じ。変換は始めない (レビュー 1 回目で確認)。ただし変換キャッシュの解決 (`resolve_epub_at`) は、記録の有無に関わらず元ファイルの stat と変換キャッシュ DB の読み書きをする | 未変換ならバッジなし。変換後に出る (§4.3.1) |
| ZIP 内の書庫 | なし | |

- 範囲は可視 + 先読み範囲だけで、scroll ごとに idle gate の後で次の範囲を取る。1 画面に本が数十冊なら、
  初回は数十回の読み取り。入れ子の合本・大きな RAR が混ざると、1 冊ごとの読み取りが大きくなる。
- 記録がある 2 回目以降は catalog の 1 行の SELECT で済む (EPUB だけは上の変換キャッシュの解決が加わる)。
- 同じ費用は、既定で ON の詳細表示のページ数列 (`details_show_page_count`) が、詳細表示の可視範囲で既に
  払っている。サムネイル表示でも同じ費用を払うようになる、というのが段 1 の変化。
- **既定 ON なので、利用者がバッジを使わなくても初回のこの I/O が増える**。サムネイル生成の I/O とは別の
  worker・別の semaphore で並ぶ (サムネイル側は `GlobalIoSemaphore` を使わない) ので、HDD・ネットワーク
  共有ではディスク上で取り合う。長さのバッジが既定 ON で動画に対してしている (FFmpeg probe) のと同じ形で、
  優先度は変えない。なお今のサムネイル表示の段階取得では、先読み範囲を含む `keep_set_sorted()` 全体を
  `visible_near` として渡すので、範囲内はすべて Normal になる (`refresh_thumbnail_details_stage`、
  `details_meta_target_for_idx_with_requirements`。レビュー 1 回目の指摘で訂正)。→ §7 の問 1。
- 範囲が変わって段階取得を取り消したとき、数えている途中の本も止まるようにする (§4.4.1)。
- 記録は既存のものなので、**この版を入れる前に詳細表示や選択情報で数えた本は、初回から記録で済む**。

### 4.3 記録と無効化

- 新しい表・列・DB は作らない。`container_page_meta` / `pdf_meta` をそのまま使う (schema 変更なし、移行なし)。
- 無効化は既存のまま:
  - ZIP・RAR: ファイルの mtime か size が変わる、または判定設定の fingerprint が変わる。
  - 画像フォルダ: フォルダの mtime が変わる (直下の追加・削除・名前変更で NTFS は更新する)、fingerprint が変わる。
    直下のファイルの中身だけ書き換えた場合はフォルダの mtime が変わらないが、ページ数も変わらない。
  - PDF: mtime か size が変わる。パスワードの保存・削除はメモリ側で revision が変わり数え直す。
  - EPUB: 変換世代が変わる。
- catalog の削除・作り直し (キャッシュ管理) で記録が消えた場合は、次に見たときに数え直す。

#### 4.3.1 メモリの値と取り直し (レビュー 1 回目 P2-1、2 回目 P2-1・P2-2)

`DetailsLazyMeta` のページ数に、数えたときの値を 2 つ足す。

- `page_count_fingerprint: i64`: 数えた job の `DetailsPageCountConfig.fingerprint`。
- `page_count_generation: Option<u64>`: EPUB のときだけ、その target を計画したときの `items_generation`。

**取り直しの判定** (`details_page_count_meta_satisfies_idx`、1 か所) に次を足す。

1. `page_count_fingerprint` が今の判定設定の fingerprint と同じ。
2. EPUB は `page_count_generation` が今の `items_generation` と同じ (成功・失敗とも。media の `RetryLater` と
   同じく、一覧の世代の中だけで有効)。

**表示と並べ替え** (`details_page_count_text`・`details_page_count_sort_value`・新しい
`thumbnail_page_count_text`) は、取り直しの判定を通さず、最後に数えた値 (`page_count_checked` の値) を出す。
取り直しが終われば値が置き換わる。3 つの consumer が別々に判定を持たないよう、値を返す accessor を 1 つにして
全部そこを通す (`details_page_count_value(idx) -> Option<PageCountValue>`)。

今の fingerprint (レビュー 3 回目 P2-1 で第3版の「フレームの先頭で 1 回」をやめた):

- **job が使う判定設定と fingerprint は、job を始めるときに同じ snapshot から作る**。`DetailsPageCountConfig` を
  作るとき、設定から `image_folder_page_count_options` を作り、その中の fingerprint を `fingerprint` にも使う
  (今は `image_page_recognition_fingerprint` と `image_folder_page_count_options` が別々に計算している。1 回の
  計算にまとめる)。環境設定がフレームの途中で適用されても、1 つの job の中で古い fingerprint と新しい判定設定が
  組み合わさらない。
- **Susie のプールは設定の snapshot に入らない** (レビュー 4 回目 P2-1)。fingerprint は作ったときのプールの
  対応拡張子を含むが、ZIP・フォルダ・RAR の列挙は実行時のグローバルなプールを見る (`folder_tree::is_recognized_image_ext`
  など)。job の途中でプールが差し替わると (起動時の差し替え・「プラグインを再読み込み」)、古い fingerprint の印で
  新しい規則の数を catalog に書いてしまう。プールを job に渡す形にすると列挙の関数を全部変えることになるので、
  **プールの版数で採否を決める**:
  - `susie_loader` に、RwLock の中のプールを入れ替えるたびに上がる版数 (`pool_revision()`) を足す (入れ替えの
    直前に上げる今の `INIT_GENERATION` とは別。入れ替えた時点で上げる)。
  - job を始めるとき、fingerprint と同時に版数を取る (fingerprint はその版のプールの拡張子から作る)。
  - 版数はプールの入れ替えと同じロックの中で上げ、読むときも同じロックで読む (プールと版数が食い違って
    見えない)。起動時の空のプール → 作り終えたプールの差し替えも 1 回の入れ替えとして数える。
  - 1 冊を数え終えたら、catalog に書く前に版数を見直す。変わっていれば、その結果を catalog にもメモリにも残さず、
    **その job (session) を「古くなった」として終える** (レビュー 5 回目 P2-1)。利用者の停止に使う `Cancelled` には
    しない (`Cancelled` は自動で再開しないため、全件の取得が止まったままになる)。また、その本だけ飛ばして
    `Finished` にもしない (数えていない本を含んだまま完了の証明が立つため)。
  - 「古くなった」終わり方は、既存の「一覧の世代が変わった session の取り消し」と同じ扱いにする: 状態を
    `NotRequested` へ戻し、完了の証明を立てない。次の poll が新しい版のプールと fingerprint で session を始め直す
    (media-duration-sort-filter-plan §5.2「動いている session の対象が古くなったとき」と同じ経路。新しい状態は
    足さない)。UI 側でも、poll で session の版数と今の版数が違えばその session を取り消して `NotRequested` へ戻す
    (worker が書く前に気づくのと、UI が先に気づくのの両方を同じ終わり方にする)。
  - 待つ状態や監視は足さない。プールの差し替えは起動時に 1 回と、利用者の再読み込みのときだけなので、
    始め直しもその回数に限られる。
- これで catalog へ書く fingerprint は、必ずその本を数えた規則のものになる。テストの helper
  (`archive_details_config`、`metadata_ops.rs`) にも同じ 1 回の計算を使う (レビュー 4 回目で確認された、残っている
  二重計算)。

RAR の検査キャッシュ (レビュー 4 回目 P2-2):

- RAR のページ数は、表示の種類を絞っていないとき `inspect_for_direct_read` の `summary.image_count` を使う。
  検査の結果は process 内のキャッシュ (`DECISION_CACHE`、`rar_loader.rs`) に path・size・mtime をキーにして残り、
  画像の判定 (`archive_converter::is_image_entry`) は Susie の対応拡張子を含む。Susie を有効にしたり
  プラグインを足したりしても、古い検査結果の件数を返し、新しい fingerprint の行として catalog に書いてしまう
  (並行操作なしでも起きる。詳細表示のページ数列・直接閲覧の判定にもある以前からの問題)。
- 検査キャッシュのキーに上の `pool_revision()` を足す。プールが変われば検査し直す。検査を使う直接閲覧・変換の判定も
  同じ認識規則に従うので、ページ数のためだけの分岐にはならない。
- 取り直しの判定に使う「今の fingerprint」は、判定を繰り返す関数 (Planning の 1 slice、
  `details_visible_stage_needs_load`、`launch_details_meta_worker` の可視 target の選び直しなど) の呼び出しごとに
  1 回だけ計算して、ループの中で使い回す。セルごとには計算しない。App に値を置かない (書き手と更新の時点を
  増やさない)。計算は拡張子の列の hash だけで I/O をしない (Susie の `try_get_pool` は初期化を始めない。
  レビュー 2 回目で確認)。

取り直しが始まる経路 (新しい再開の仕組みは足さない)。**取り直しの判定に合わなくなった値は、次に実際の取得要求が
始まるまで最後の値のまま表示し、取得要求が始まればその範囲で取り直す**。取得要求が始まる条件は既存のまま:

- サムネイル表示の段階取得と、詳細表示の可視範囲の段階取得は、Ready の間も毎フレーム
  `details_visible_stage_needs_load` で範囲内の未充足を見ている。fingerprint や EPUB の世代が合わなくなれば、
  範囲が変わらなくても次の段階取得が始まり、範囲内を取り直す (レビュー 3 回目 P3 の確認どおり)。ほかの窓
  (detached / park 中の context) も、その窓の poll が回れば同じ。
- 詳細表示のページ数ソート (一覧全体の取得) が Ready の間は、可視範囲の自動の取り直しの対象外 (既存)。一覧の
  install・利用者の更新など、次に全件の取得が始まるときに取り直す。それまでは最後の値で並ぶ。選択情報の対象が
  未充足なら、その 1 件だけ VisibleStage で取り直す (既存)。
- 一覧の install は details-meta の状態を `NotRequested` に戻すので、どの表示でも取り直しが始まる。

fingerprint が変わる場面と、その後の流れ:

- **環境設定の適用で判定設定を変えたとき** (表示する種類・隠しファイル・重複の扱い・拡張子の優先度・Susie の
  ON/OFF): 環境設定の適用が今の一覧を読み直す (`apply_file_type_visibility_change` /
  `reload_current_folder_preserving_override`、`ui_dialogs/preferences.rs`)。install で取り直しが始まる。
- **「プラグインを再読み込み」ボタン** (`ui_dialogs/preferences/pages.rs`) と、**起動時の Susie のプールの非同期の
  差し替え** (`susie_loader::init_pool`。空のプールで先に動き、作り終えたプールへ差し替える): どちらも一覧を
  読み直さない (レビュー 3 回目 P2-3)。一覧の中身も読み直すまで古い規則のままなので、ページ数だけ揃えるための
  読み直しは足さない。上の規則どおり、段階取得の範囲内は次の段階取得で新しい fingerprint で取り直し、範囲外と
  詳細表示のページ数ソートは次の取得要求まで最後の値を出す。
  - 起動時の差し替えの前に数えた本は、差し替えの後にもう一度数える (catalog の行は fingerprint ごとに上書きされる
    ので、次の起動でも差し替えの前後で 1 回ずつ数え直す)。Susie を有効にしている利用者で、起動直後の数百 ms の
    間に範囲へ入った本だけに起きる費用で、**この変更で新しく増える** (今の詳細表示のページ数列は fingerprint を
    メモリで照合しないので、差し替えの後に数え直さない。レビュー 4 回目 P3)。プールを待つ状態は
    足さない (待つ間に段階取得が空回りしないよう別の状態が要るため)。perf の `page_counted` で件数を確かめる。
- ネイティブ対応の拡張子の追加と、拡張子の優先度の起動時の補完は、起動時の処理なので実行中の一覧の読み直しは
  要らない (レビュー 3 回目で確認)。
- 判定設定を行き来すると、catalog は新しい fingerprint で引くので数え直しになる (`container_page_meta` の主キーは
  `(filename, kind)` で、別の fingerprint の結果は同じ行を上書きする。レビュー 2 回目 P3)。

EPUB:

- 変換していない EPUB を開くと変換する。本を閉じて親の物理フォルダへ戻るときは親への `Direct` で一覧を install
  し直す (`app/close_return.rs`。レビュー 3 回目で確認) ので、世代が進み、変換キャッシュから数え直す。
  独立した別の窓で開いて閉じた場合は、元の窓の一覧と世代はそのまま (既存の挙動、`close_return_tests.rs`) で、
  次の取得要求まで最後の値 (未変換のときは無し) を出す。
- 一覧を表示したまま別の経路 (Remote など) で変換・再変換・変換キャッシュの削除が起きた場合も、その一覧の
  世代が進み、次の取得要求が始まったときに取り直す。世代は一覧内の削除などで install せずに進むこともあり、
  そのときも段階取得の範囲内の EPUB は次の段階取得で取り直す。
- 取り直しは変換キャッシュの解決 (stat と DB) と catalog の SELECT だけで、その間も最後の値を表示するので
  バッジは消えない。同じ世代の中では繰り返さない。EPUB 以外は世代で取り直さない。

詳細表示のページ数列・選択情報も同じ accessor と判定を通るので、以前からの同じ問題もあわせて直る。
永続データは変わらない (メモリの構造だけ)。

### 4.4 段階取得への組み込み

`thumbnail_media_metadata_enabled()` の 2 つの意味 (§2.2) を分ける。

```rust
/// サムネイル表示で、可視範囲の details-meta を段階取得するバッジがあるか。
fn thumbnail_badge_metadata_enabled(&self) -> bool {
    self.settings.grid_view_mode == GridViewMode::Thumbnail
        && (self.settings.thumb_show_media_duration
            || self.settings.thumb_show_resume_meter
            || self.settings.thumb_show_page_count)
}

/// 動画・音声の長さのフィールド要件 (今の thumbnail_media_metadata_enabled と同じ式)。
fn thumbnail_media_fields_enabled(&self) -> bool { ... }

/// ページ数のフィールド要件。
fn thumbnail_page_count_enabled(&self) -> bool {
    self.settings.grid_view_mode == GridViewMode::Thumbnail && self.settings.thumb_show_page_count
}
```

- 段階取得の判定 7 か所は `thumbnail_badge_metadata_enabled()` に替える。
- `lazy_load_video_meta_for_idx` は `thumbnail_media_fields_enabled()` に替える (ページ数だけ ON のときに
  動画を probe しない)。
- `lazy_load_page_count_for_idx` のサムネイル表示の分岐に `thumbnail_page_count_enabled()` を OR で足す
  (長さだけ ON のときに本を数えない)。
- ページ数は `allow_page_count` (可視 + 先読み、または詳細のページ数ソート) と `whole_listing_only` の既存の
  制限に従う。AI facet・長さ facet の一覧全体の取得中も、表示集合の外の本は数えない (既存どおり)。
- `details_visible_stage_needs_load` のサムネイルの分岐は `details_item_requires_lazy_meta` を使うので、
  ページ数の要件は自動で入る。
- 設定の変更: `preferences.rs` の `thumbnail_media_requirements_changed` に `thumb_show_page_count` の差を足す
  (要件を作り直す既存の経路)。名前も `thumbnail_badge_requirements_changed` 等へ揃える。
#### 4.4.1 取消の伝達 (レビュー 1 回目 P2-4)

範囲が変わると段階取得は古い job を取り消して次を始める (`refresh_thumbnail_details_stage`)。今のページ数の
取得は、ZIP と PDF には cancel を渡しているが、RAR は内部で常に false の token を作る
`inspect_for_direct_read` を呼び、画像フォルダの `image_folder_page_count` は cancel を受け取らない
(`rar_loader.rs`、`folder_scan.rs`)。既定 ON にすると、scroll で不要になった長い走査が続いて次の範囲と取り合う。

- RAR は既存の `inspect_for_direct_read_cancelable` に job の cancel を渡す。
- 画像フォルダは `image_folder_page_count` に cancel を渡し、`read_dir` の項目ごとに見る
  (取り消したら記録せず `DetailsPageCountError::Cancelled` で返す。ZIP の取消と同じ扱い)。
- 取り消した本は記録もメモリの checked も立てない。再試行の仕組みは足さない。次にその本が範囲に入ったときに
  通常どおり数える。

#### 4.4.2 detached viewer

- detached viewer: `details_lazy_meta` と段階取得の状態は既に viewer context ごとの bundle に入っている
  (`viewer_context_registry.rs`)。新しい swap field は無い。設定は全体の 1 つ。detached 述語・viewport 経路には
  触れない見込み。触れた場合は CLAUDE.md の合意手続に従う。

### 4.5 数え終わる前の表示

何も出さない (§3)。数え終わった次のフレームでバッジが出る。数えられなかった本も出さない。

### 4.6 表記

- 長さのバッジと同じ見た目 (`media_duration_style()`、`draw_overlay_media_duration_badge` の黒地に白文字)。
  専用の色は足さない。
- 文言は **単位付き**にする。右下には評価の絞り込みの一致数 (ただの数) も出るので、ただの数だと区別できない。
  「123P」とする (§7 の決定 3)。
- 1 万ページ以上もそのまま出す (ページ数は長さほど桁が増えない)。入らなければ出さない (§4.7)。
- 文字は ASCII と既存の日本語だけで、絵文字・環境依存の記号を使わない (`check_ui_glyphs.py`)。

### 4.7 右下の優先順位

1. 読書位置のメーターの帯 (下端、既存どおり先に予約)。
2. フィルタの一致数 (右下の角、既存どおり)。
3. **ページ数** (右下。一致数・ほかのバッジとぶつかれば上へずらし、入らなければ出さない。長さと同じ
   `place_bottom_right`)。
4. 長さ (動画・音声。本とは同じセルに出ないので、ページ数との順は実際には効かない。両方を同じ
   `layout_badges` の中で、一致数の後に置く)。

実装は、`ThumbnailOverlayLayoutInput` に `page_count: Option<&str>`、`BadgeKind::PageCount`・
`BadgePriority::PageCount` を足し、`ThumbnailOverlayLayout` に `page_count: Option<BadgePlacement>` を足す。
`media_duration` と 1 つの欄にまとめる案は、意味の違う値を 1 つの欄に入れることになるので採らない
(CLAUDE.md の「1 つの述語を 2 つの問いに使わない」)。
描画は `grid_paint.rs` の長さのバッジの隣で、通常のグリッド (`ui_main.rs` 18629 行付近) と
もう 1 つの描画経路 (22390 行付近) の両方に値を渡す。描き手の数を実装時に数え、どちらかだけに足さない。

### 4.8 設定

- `Settings::thumb_show_page_count: bool`、既定 true。旧設定に field が無いときも true にする
  (既定の付け方は `thumb_show_media_duration` と同じにする)。
  旧版は未知の field として無視するので、旧版へ戻しても設定は壊れない (SortOrder のような境界は無い)。
- 環境設定「サムネイル」で、`draw_media_duration_settings` の直後に置く (同じ欄の隣)。
  - チェックボックス: 「本のページ数をサムネイルに表示する」
  - 説明: 「ZIP・PDF・画像だけのフォルダなどの右下にページ数を表示します。初めて表示する本は中を数えるため、
    少し遅れて表示されます。フィルタ一致数や他の表示と重なる場合は上へ移し、収まらない場合は表示しません。」
  - 環境設定の検索 (anchor) に載せる。
- `settings_transfer.rs` に 1 行 (「サムネイルのページ数表示」)。設定の件数を数えるテスト (`ba81fdab0` で
  揃えたもの) も合わせる。
- マニュアル (`htdocs/mimageviewer/manual/` のサムネイル表示の節と環境設定の節) と製品ページの該当カードを更新する。

### 4.9 UI スレッドと perf

- 描画で UI スレッドが新しく行うのは、メモリの `details_lazy_meta` を引いて文字列にすることだけ
  (`thumbnail_page_count_text(idx)`、`thumbnail_media_duration_text` と同じ形)。I/O・DB・read_dir はしない。
  [ui-responsiveness.md](ui-responsiveness.md) §4 のチェックリストに沿い、target の構築は既存の Planning を通る。
- ZIP 内の書庫 (`ZipDir`) のページ数は、今は target を作るたびに UI スレッドで部分木を再帰で足している
  (`ZipTree::page_count_for_prefix_str` → `total_image_count`)。同じ target は Planning・worker 起動時の
  `initial_priority_targets`・選択行の slice ごとの再作成で何度も作られ、計測の区間 (`target_scan_slice`) にも
  全部は入っていない (レビュー 1 回目 P2-3、2 回目 P2-3)。**上限の議論と計測を足すのではなく、集計そのものを
  UI スレッドから無くす**: 各 `ZipTreeNode` に部分木の画像数を持たせ、**木を作る 2 つの経路の両方で**、作るときに
  一度だけ数える。
  - `ZipTree::build` (列挙結果から作る生の木)。
  - `ZipTree::project` / `project_node` (表示の種類で画像と子を絞った投影の木。レビュー 3 回目 P2-2)。残した
    画像数と、作り終えた子の値を足すだけで、全体の走査は増えない。
  - 不変条件: どちらの木も、各ノードの値が自分の部分木に残っている画像の数と一致する。木は作った後に変更されず
    `Arc` で共有される (公開後にノードを書き換える経路は無い。レビュー 3 回目で確認)。
  - `page_count_for_prefix_str` はノードを探して値を読むだけにする (費用は prefix の深さ)。
  - レーティング一覧などで外側 ZIP の木が無い (`zip_nav` が無い) `ZipDir` は、今と同じく値が取れずバッジを出さない。
  - **`ZipDir` は details-meta のページ数の target にしない** (レビュー 5 回目 P2-2)。今の `warm_page_count` の
    経路 (target を作って worker がその値を返し、メモリへ入れる) をやめ、ページ数の accessor
    (`details_page_count_value`) が `ZipDir` のときは今の `zip_nav` の木から直接読む。詳細表示のページ数列・
    列見出しの並べ替え・選択情報・バッジが同じ accessor を通るので、`ZipDir` の数の書き手は木だけになる。
    木が作り直されれば次のフレームから新しい数になる。ZIP の中の一覧はページ順が固定で、一覧の並べ替え
    (ページ数順) の対象にはならない (§6.4)。
  - **「ページ数を表示できる」と「ページ数を非同期に取る必要がある」を分ける** (レビュー 6 回目 P2-1)。
    `details_item_supports_page_count` (表示できるか) は今のまま `ZipDir` を含める (外すと詳細表示の文字も消える)。
    新しく `details_item_page_count_needs_fetch(idx)` (= 表示できて `ZipDir` でない) を置き、**取得の需要を見る
    述語は全部こちらを使う**: `lazy_load_page_count_for_idx`、選択情報の `selection_info_item_requires_lazy_meta`、
    段階取得の `details_visible_stage_needs_load`、Planning の選別、全体要件 (`requirements.page_count` の対象)。
    `target` から外すだけだと、メモリが無い `ZipDir` が「未充足」と見なされ、0 件の target → Ready → 未充足 →
    次の段階取得、を繰り返すため。`ZipDir` の作成日時は今も取得対象外 (`details_created_time_path` が `None`) なので、
    `ZipDir` は選択情報の遅延取得の対象そのものにならない。
  - 木が無い `ZipDir` (レーティング一覧など) は「取得待ち」ではなく「取れない」: accessor は Unavailable を返し、
    詳細表示は「-」、並べ替えの値は不明 (末尾)、バッジは出さない。今の未取得の表示 (Ready でも「…」) へ流さない。
- 計装: 既存の `details_meta/load_done` に、ページ数の件数を足す
  (`page_targets`・`page_cache_hits`・`page_counted` を種類別 (zip / zip_nested / folder / rar / pdf / epub)・
  `page_failed`・`page_ms`)。取り消した job は `load_done` に届かず途中で返るので、既存の
  `details_meta/load_cancelled` にも同じ件数と経過時間を載せる (レビュー 2 回目 P3)。1 冊に 200 ms 以上かかったものは `details_meta/page_count_slow` (種類・ms・cache の有無) を
  出す (`ai_item_slow` と同じ形)。初回と 2 回目の差、ネットワーク共有での 1 冊の時間を perf ログで読めるようにする。
- ui-responsiveness.md に §1.883 の節を足す (要約: 可視範囲だけを既存 worker で数え、記録は既存 catalog)。

### 4.10 変えないもの

- 詳細表示のページ数列・その列見出しでの並べ替え (`DetailsSortKey::PageCount`)・選択情報のページ数。
- Remote (Web UI の一覧には長さのバッジも無い。ページ数のバッジも出さない)。
- catalog の schema・`CATALOG_VERSION`。
- 長さのバッジ・再生位置メーター・読書位置のメーターの配置。

## 5. 検討して採らなかった案

- **サムネイル作成のついでにページ数を記録する** (PDF は既にそうしている)。ZIP のサムネイル作成も中の一覧を
  読むので初回の I/O を減らせるが、サムネイル側の一覧は表示の種類の絞り込みや fingerprint と同じ規則で
  数えていない可能性があり、規則を 2 か所に書くことになる。サムネイルが記録済みのフォルダでは効かない。
  段 1 では採らず、perf ログで初回の費用が問題になったら検討する。
- **数え終わるまで「…」を出す**。§3 のとおり状態が増える。長さのバッジとも揃わない。
- **バッジのために一覧全体を数える**。バッジには要らない。一覧全体を数えるのは、ページ数順を選んでいるときだけ
  (段 2、§6.5)。

## 6. 段 2: ページ数順の並べ替え

### 6.1 方針: 長さ順の部品を共通にして乗せる (CLAUDE.md「設計の簡素化」)

§1.762 の長さ順は、次の部品でできている ([media-duration-sort-filter-plan.md](media-duration-sort-filter-plan.md)
§3〜§5、コードは `src/app/duration_metadata.rs`・`src/app/listing_sort.rs`・`src/app/folder_scan.rs` の
`FolderDurationFacts`・`src/settings.rs` の `ListingDuration`)。

1. 並べ替えのキー (`ListingSortMetadata.duration`) と比較 (`SortOrder::compare_listing_keys`)。不明は末尾。
2. 値の出どころ: メモリ (`details_lazy_meta`) → catalog の一括読み取り (`ScannedDir.duration_facts`、worker が
   要るときだけ作る) → 不明。materialize は I/O をしない純関数のまま。
3. 使える一覧の判定 (`listing_sort_capabilities` の `duration_allowed`、物理フォルダだけ) と、使えない一覧での
   名前順への正規化 (`without_duration_order` / `without_duration_request`)。
4. 一覧全体の取得の要件 (`ListingMetaRequirements`) と完了の証明 (`listing_meta_scan_proof`)、読み込み中の表示。
5. 長さ順へ変える操作・選び直しを worker 走査に通す入口。
6. ツールバーの既定の補完 (marker 方式)、ゲームパッドの巡回、Remote の候補からの除外、ファイル名スタックの
   メタデータの持ち回り、自動では並べ直さない (利用者決定 B)。

ページ数順はこの 6 つを全部要る。ページ数専用に複製すると、同じ規則 (使える一覧・正規化・入口・Remote) が
2 か所に書かれて食い違う。そこで **「後から分かる並べ替えの値」(以下、遅延キー) を共通の概念にして、
長さとページ数をその 2 つの種類にする**。

- `SortOrder::lazy_metric(self) -> Option<LazyMetric>` (`LazyMetric::{Duration, PageCount}`)。`is_duration()` を
  判定に使っている箇所は、規則が遅延キー全体に当たるもの (使える一覧・正規化・worker 走査の入口・Remote・
  ゲームパッド・ツールバーの補完) を `lazy_metric().is_some()` へ、長さ固有のもの (長さ facet・`video_meta` の
  読み取り) をそのままにする。置き換える箇所は実装時に全部列挙し、検証台帳に残す (今の非テストの参照は
  `src/` で約 20 ファイル)。
- `listing_sort_capabilities` の `duration_allowed` は `lazy_metric_allowed` にする (使える一覧は同じ: PC 本体の
  物理フォルダの通常一覧)。`DURATION_SORT_UNSUPPORTED` の文言は種類ごとに出す。
- 自動では並べ直さない。ページ数が後から分かっても並びは動かさず、「最新の情報に更新」・ページ数順の選び直し・
  開き直しで並べ直す (長さ順・評価順と同じ。体験をそろえるという利用者の発案に合わせる)。

### 6.2 並べ替えのキー

```rust
pub(crate) enum ListingPageCount {
    /// 本として数えたページ数。
    Known(u32),
    /// 数えたが本ではない (本でないフォルダ・数えられなかった・パスワードの要る PDF)。
    Unavailable,
    /// まだ分からない、本の種類でない、またはページ数順でない。
    Missing,
}
```

- `ListingSortMetadata` に `page_count: ListingPageCount` を足す (`Copy` のまま、`new` は `Missing`)。
- `PageCountAsc`: Known を少ない順、その後 Unavailable と Missing (区別せず名前昇順)。`PageCountDesc`: Known を
  多い順、その後同じ。**不明は昇順・降順とも末尾**。同値は名前昇順。4 行割当の行の中だけで比べる (長さ順と同じ)。
- 既定の割当では 1 行目が実フォルダ + 書庫類なので、本 (ZIP・PDF・画像フォルダ・RAR) がページ数順に並び、
  本でないフォルダはその後ろに名前順で並ぶ。2 行目の画像・動画は全部 Missing で名前順。
- 対象の種類は `details_item_supports_page_count` と同じ (ドライブ一覧を除く)。7z・LZH は Missing。
- ラベル: 「ページ数（少ない順）」「ページ数（多い順）」、短いラベル「ページ↑」「ページ↓」。説明:
  「本のページ数が少ない順。ページ数が分からないものは末尾。読み込み後に更新すると並べ直します」。

### 6.3 値の出どころ

materialize は各本のページ数を次の順で決める (先に見つかったものを使う)。

1. **メモリ** (`details_lazy_meta`): 元ファイルの stamp が一致し、**判定設定の fingerprint が今の一覧の要求の
   fingerprint と一致する**値 (Known / Unavailable)。表示の「最後の値」(§4.3.1) とは違い、並べ替えでは
   fingerprint が合わない値は使わない (新しい一覧は今の規則で作るので、古い規則の数で並べない)。
   EPUB の世代は見ない (世代は取り直しのきっかけで、値の正しさは stamp と fingerprint で決まる)。
   - UI スレッドの materialize は `details_lazy_meta` を引く closure (長さの `listing_duration_for_path` と同じ形)。
   - worker の materialize (外部変更の reconcile) は、要求を作るときに UI が今の一覧の本のページ数の不変 snapshot
     (`Arc`) を作って渡す (長さの `listing_duration_snapshot` と同じ形。1 つの snapshot に長さとページ数を入れる)。
2. **catalog** (`ScannedDir.page_count_facts`): 下の一括読み取りで、名前・種類・mtime・size・fingerprint が
   記録の再利用と同じ条件で一致する行。
   - ZIP・RAR: `container_page_meta` (kind=Zip / Archive) の名前・mtime・size・fingerprint。
   - 画像フォルダ: `container_page_meta` (kind=Folder) の名前・mtime・size=0・fingerprint。
     **ページ数の identity は種類ごとに決める** (レビュー 5 回目 P2-4)。走査はフォルダについて、表示用の stamp を
     `(mtime, 0)`、並べ替え用の `ListingSortMetadata` を `file_size: None` で作る (`folder_scan.rs` の
     `ScannedFolderEntry`)。details-meta は前者 (`image_metas`) で catalog を照合・保存する。そこで、ページ数の照合
     (メモリ・facts とも) は **表示用の stamp を使い、フォルダは `(mtime, 0)`** とする。長さの reader のように
     `ListingSortMetadata.file_size?` で弾くと、フォルダは一度も当たらない。サイズ順のための `None` は変えない。
   - 本の件数の入力は、長さが使う `all_media` ではなく、書庫も含む `scan.folders` 側
     (`book_materialization.rs` の物理 materialize) から取る。
   - PDF: `pdf_meta` の名前・mtime・size。`password_required` の行は使わない (資格情報で値が変わるので、
     メモリの値だけを使う)。
   - EPUB: `pdf_meta` は変換世代の stamp で書かれ、走査の stamp では照合できないので使わない (メモリだけ)。
3. どちらにも無ければ Missing。

catalog の一括読み取り (worker だけ):

- 要求を作るとき、UI が `listing_may_sort_by_page_count` (共通・live・保存済みのお気に入り別のどれかがページ数順。
  長さの `listing_may_sort_by_duration` と同じ保守的な OR) と、判定設定の snapshot から作った fingerprint を
  worker へ渡す。false なら catalog を開かない (通常の一覧の費用は増えない)。
- true のとき、走査の後で本が 1 件以上あれば、フォルダの catalog を read-only で開き
  (`open_existing_read_only_admitted()`、無ければ空)、新しく足す 2 つの SELECT
  (`container_page_meta` の全行、`pdf_meta` の全行。schema は変えない) で facts を作る。長さの facts も要るときは
  同じ接続を使う (1 フォルダ 1 回の open)。`perf::event("listing", "page_count_facts")` に件数と ms。
- UI スレッドの同期走査は facts を作らない (`None`)。`None` でページ数順になったときはメモリだけで並べる。

### 6.4 使える一覧と正規化

長さ順と同じ表 ([media-duration-sort-filter-plan.md](media-duration-sort-filter-plan.md) §3.3) を遅延キー全体に
使う。使えるのは PC 本体の物理フォルダの通常一覧 (main と各 detached context)。

- ページ順が固定の一覧 (本扱いの画像フォルダ・製本・ZIP/PDF の中) は固定順のまま。
- Ctrl+G・スマートフォルダ・サブフォルダ展開・レーティング一覧・ブックマーク・コレクションでは選べず、理由を
  出す: 「この一覧ではページ数順を使えません。詳細表示のページ数列で並べ替えられます」。全体の設定がページ数順の
  まま開いたときは名前昇順で並ぶ (`without_lazy_metric_order`)。
- Remote: 候補に出さない。PC 本体の設定がページ数順のとき、Remote の一覧は名前順で並ぶ (長さ順と同じ)。
- フォルダ代表サムネイルの候補順・コレクションの並べ替えの候補: 明示の一覧から外す (長さ順と同じ)。
- 詳細表示の列見出しの並べ替え (`DetailsSortKey::PageCount`) はそのまま。今それが使える一覧で引き続き使える
  (ページ順が固定の一覧、手動・シャッフル順のコレクションでは今も使えない。`src/app.rs` の詳細列ソートの判定。
  レビュー 7 回目 P3)。

### 6.5 一覧全体の取得と完了の証明

- `ListingMetaRequirements` に `page_count: bool` を足す。`page_counts_wanted()` = 今の並べ替えがページ数順で、
  この一覧で使えること (`lazy_metric_allowed`)。**継続条件** (長さと同じく「未取得がある間だけ」にしない)。
- 対象はその一覧の **全部の本** (`items` のうち `details_item_supports_page_count` が真のもの。表示集合・絞り込み・
  可視範囲に依らない)。`whole_listing_meta_scan_wanted()` にページ数を足す。
- **`requirements.page_count` は表示の設定から独立して、取得の全部の関門を通る** (レビュー 5 回目 P2-3)。
  バッジも詳細表示のページ数列も OFF のままページ数順を選んでも数える。今は次の関門がそれぞれ表示の需要
  (`lazy_load_page_count_for_idx`) だけを見ているので、全部に `requirements.page_count && 本` を足す:
  - Planning の対象の選別と、メモリで満たしているかの判定 (`advance_details_meta_target_plan` の
    `details_item_requires_lazy_meta(idx) || (requirements.duration && media) || (requirements.ai && ai)` と、
    その後の cached の判定)。
  - target の生成 (`details_meta_target_for_idx_with_requirements` の `load_page_count`。`allow_page_count` と
    `whole_listing_only` の制限も、`requirements.page_count` のときは通す)。
  - target が 0 件でも完了の証明を立てる経路 (`launch_details_meta_worker` の `plan.total == 0 || target_count == 0`)
    は、ページ数の要件を含む snapshot をそのまま渡す。
  - 詳細表示の列見出しの並べ替え用の既存の `load_all_page_counts` は、別の経路としてそのまま残す。
- フィールドの規則 (§1.835 の「長さだけの要求で AI・日時・寸法・ページ数を増やさない」を保つ):
  - 表示集合の外では、全体要件に含まれるフィールドだけを取る。`whole_listing_only` の target でも、
    `requirements.page_count` のときだけページ数を取る (`allow_page_count` の判定に足す)。
  - ページ数だけの要求で、動画の probe・AI・日時・寸法を増やさない。長さ・AI だけの要求で本を数えない。
- 完了の証明: `ListingMetaScanProof.requirements` に `page_count` が入る。`page_count_scan_ready()`。
  立てる・失効・取り直し・停止の規則は長さと同じ (同じ proof を共有する)。
- 取り直しの判定 (§4.3.1) は、全体走査の target にも同じものを使う。全体走査の Planning で、メモリの値が
  fingerprint・EPUB の世代で合わなければ取り直す。
- 読み込み中の表示: ページ数順の並べ替え項目 (ツールバーのボタン・メニューの選択中の項目) の tooltip と短い
  ラベルの後ろに「ページ数を読み込み中 n/m」、停止中は「ページ数の読み込みを停止中」(長さの
  `duration_loading_text` と同じ形。共通の関数にして種類で文言を変える)。
- 停止 / 再開は既存の details-meta の停止をそのまま使う。
- 段階取得 (バッジ) は、未証明の全体要件が無くなってから始まる (既存の規則)。ページ数順で全体を取り終えれば、
  バッジの値もそろう。

### 6.6 ページ数順へ変える操作と入口

- メニュー・ツールバー・ゲームパッドでページ数順へ変える、または選択中のページ数順を選び直すときは、今の
  `apply_sort_change_reload` (`src/app.rs`) をそのまま使う。master では、通常の物理フォルダの並べ替えの再読込は
  既に worker 走査 (`start_folder_open_scan` の `CurrentViewOrderRefresh`) へ渡している (レビュー 5 回目 P3。
  第5版が書いた `apply_sort_change_reload_without_ui_io` は今のコードに無い)。ここは長さの判定の一般化ではなく、
  今の入口の再利用になる。
- そのほかの入口 (開く・Ctrl+↑↓・BS・folder pane・更新・外部変更) は、長さ順の実装で作った入口の台帳
  ([media-duration-sort-filter-plan.md](media-duration-sort-filter-plan.md) §11.2) をそのまま使い、facts の要求に
  ページ数の flag と fingerprint を足す。同期走査の入口はメモリだけで並ぶ。
- 選択中のページ数順を再び選ぶと再読込して並べ直す (`SortControlDecision::Reload`)。

### 6.7 ツールバー・ゲームパッド・ファイル名スタック

- `SortOrder::all()` に `PageCountAsc` / `PageCountDesc` を足す (並びは長さ順の後ろ)。
- ツールバーの既定の補完: 今の既定 12 値と完全一致するときだけ、一回限りで 14 値へ補完する (既存の marker 方式。
  長さ順を足したときと同じ)。→ §7 の問 4。
- ゲームパッドの巡回: 使えない並べ替えを飛ばす (既存の判定を遅延キー全体へ)。
- ファイル名スタック: 物理 materialize で得た `ListingSortMetadata` をそのまま持ち回る経路が長さ順で既にある
  (`StackMember`)。`page_count` も同じ構造体に入るので落ちない。集約したスタックセル自身は Missing。

### 6.8 費用

- ページ数順を初めて選んだフォルダでは、本を全部数える (worker、進捗と停止あり)。2,000 冊の ZIP なら初回は
  2,000 回開く。入れ子の合本・RAR・未記録の PDF は 1 冊ごとに重い (§4.2)。これは今、詳細表示のページ数列の
  見出しで並べ替えたときに払っている費用と同じ。
- 2 回目からは catalog の一括読み取り (1 フォルダ 1 回の open と 2 つの SELECT) で、materialize の時点で並ぶ。
- ページ数順でない一覧の費用は増えない (`listing_may_sort_by_page_count` が false なら catalog を開かない)。

### 6.9 永続データ

- catalog: schema は変えない。読み取りの API を足すだけ (`container_page_meta` / `pdf_meta` は出荷済み)。
- settings: `SortOrder` の新しい値 (`settings.sort_order`・`FavoriteViewState.sort_order`・`toolbar_sort_items`)。
  - **v5.6.x 以前がこの値を読むと、その版は `Incompatible` として既定設定 + 保存抑止で起動する** (DB は書き換え
    ない。v5.7.0 に戻せば元の設定で起動する)。長さ順・サイズ順・評価順を足したときと同じ境界。ツールバーの補完に
    よって、ページ数順を使っていない利用者もこの境界に入る。→ §7 の問 4。
  - 新旧の wire (serde 文字列) と、旧版相当の enum で読んだとき DB を書き換えないことを試験する。

### 6.10 detached viewer・Remote

- `listing_meta_scan_proof` は既に viewer context の bundle にある。中の要件の型に field が 1 つ増えるだけで、
  swap の field は増えない。detached 述語・viewport 経路には触れない見込み。触れた場合は CLAUDE.md の合意手続に
  従う。park / mount / drop で兄弟 context の状態が変わらないことを試験する。
- Remote IPC: 候補からページ数順を外すだけで wire の構造は変えない。

## 7. 利用者の決定 (2026-11-24、統括経由)

利用者の回答は、5 点とも選択肢「推奨どおり (Recommended)」。

1. 既定 ON。
2. 画像だけのフォルダにもバッジを出す (ページ数順でも本と並ぶ)。
3. 表記は「123P」。
4. ツールバーの既定にページ数順を足し、旧版へ戻したときの扱いは長さ順と同じ。
5. ページ数順も自動では並べ直さない。

以下は判断を仰いだときの問いの記録。


1. **既定 ON の費用**: 既定 ON のため、利用者がバッジを使わなくても、初めて見る本を 1 冊ずつ数える I/O が
   可視範囲で増える (HDD・ネットワーク共有で、サムネイルの読み込みと取り合う)。記録済みなら 2 回目以降は軽い。
   既定 ON でよいか (要望どおり ON を推奨。費用は perf ログで確かめる)。あわせて、Susie を有効にしている場合は、
   起動直後 (プールの差し替えの前) に範囲へ入った本を差し替えの後にもう一度数える (詳細表示のページ数列でも
   新しく起きる。§4.3.1)。
2. **画像だけのフォルダ**: 普通の写真フォルダにも、画像の枚数がバッジとして出る (詳細表示のページ数列と同じ)。
   「画像のみのフォルダを本として扱う」が ON のときだけにするか、常に出すか (推奨: 詳細列と同じく常に出す)。
   ページ数順でも、同じ判定で画像だけのフォルダが枚数で本と一緒に並ぶ。
3. **表記**: 「123P」「123p」「123ページ」のどれにするか (推奨: 「123P」)。
4. **ツールバーの既定と旧版との境界** (ページ数順): 長さ順のときと同じく、ツールバーの既定の並べ替えに
   ページ数順の 2 つを足し、そのため v5.6.x 以前へ戻すと既定設定で起動する (保存はしない。v5.7.0 に戻せば元に
   戻る) 扱いでよいか (推奨: 長さ順と同じ扱い)。
5. **ページ数順でも自動では並べ直さない** (長さ順の決定 B と同じ。数え終わった後は更新・選び直し・開き直しで
   並ぶ) でよいか (推奨: 長さ順とそろえる)。

## 8. 検証計画 (実装時)

- 配置 (`thumb_overlay_layout.rs` の単体テスト): ページ数単独、一致数ありで上へずれる、読書位置のメーターの
  帯の上、狭いセルで出ない、ほかのバッジを動かさない。UI スナップショットを 1 枚足す
  ([ui-snapshot-policy.md](ui-snapshot-policy.md))。
- 要件 (App の状態テスト): ページ数だけ ON (長さ・再生位置メーターは OFF) で本を数え動画を probe しない、
  長さだけ ON で本を数えない、ページ数・長さ・再生位置メーターを全部 OFF にし選択情報・facet の要求も無いときに
  段階取得を回さない (今と同じ)。再生位置メーターだけ ON・選択情報だけ・AI facet / 長さ facet の全体取得だけ、の
  ときは今の取得を保ち、本を数えない (レビュー 1 回目 P3)。可視 + 先読み範囲の外は数えない、AI facet・長さ
  facet の全体取得中も表示集合の外の本を数えない、設定の切り替えで要件を作り直す。
- 取り直し (§4.3.1): 判定設定を変えて一覧を読み直すと新しい fingerprint で取り直し、取り直しの間も最後の値を
  表示する (消えない)。段階取得は Ready の間に fingerprint・EPUB の世代が合わなくなると範囲内を取り直し、詳細表示の
  ページ数ソートの Ready は次の全件の取得まで最後の値で並ぶ。EPUB は世代が進むと取り直し、同じ世代では繰り返さず、
  ほかの種類は世代で取り直さない。未変換の EPUB を開いて変換し親のフォルダへ戻るとバッジが出る。表示・並べ替え・
  バッジが同じ accessor を通る。PDF パスワードの revision の既存テストを保つ。
- job の snapshot: 環境設定を同じフレームの途中で適用しても、1 つの job の判定設定と catalog へ書く fingerprint が
  同じ snapshot から来る。`image_folder_page_count_options` と config の fingerprint が同じ値。
- Susie: プールの差し替え (起動時・再読み込み) の後、範囲内は取り直し、範囲外は最後の値のまま。数えている途中で
  差し替わった本の結果は catalog にもメモリにも残らず、次の取得で数え直す。RAR の検査キャッシュはプールの版数が
  変わると使われない。
- `ZipTree`: `build` と `project` の両方の木で、各ノードの画像数が部分木の実数と一致する。`page_count_for_prefix_str`
  が再帰しない。
- 取消 (§4.4.1): RAR とフォルダの走査が job の cancel で止まり、記録も checked も立てない。
- 値: `page_count = None`・0・失敗でバッジなし、PDF のパスワードの revision、`ZipDir` は今の `zip_nav` の木の値を
  accessor が直接読み、details-meta の target にならない、木が無い `ZipDir` は「-」・不明・バッジなし、
- `ZipDir` の需要: 木あり / 木なし、選択情報だけ有効、詳細表示のページ数ソート、サムネイル表示の段階取得の各条件で、
  poll を繰り返しても details-meta の session が増えない (取得の需要の述語が `ZipDir` を含まない)、
  ドライブ一覧でバッジなし。
- 設定: 既定 true、旧設定に field が無いと true、false の往復、settings_transfer、件数のテスト。
- ページ数順 (§6):
  - 比較: Known の昇順・降順、Unavailable・Missing が両方向で末尾、名前の tie、4 行割当、本でないフォルダの位置。
  - 出どころ: メモリ (stamp と fingerprint が一致する値だけ) → catalog の facts → Missing。fingerprint が違う
    メモリの値・facts を使わない。EPUB と `password_required` の PDF は facts を使わない。画像フォルダの facts が
    details-meta の書いた行と同じ stamp で当たる。UI の closure と worker の snapshot で同じ順。facts `None` は
    メモリだけで並ぶ。ページ数順以外ではページ数を引かない。
  - catalog: 一括読み取りの API (2 つの SELECT、read-only open、catalog 無しは空)、`listing_may_sort_by_page_count`
    が false のとき開かない、長さとページ数の両方が要るとき 1 回の open。
  - 遅延キーの共通化: `is_duration()` から置き換えた全箇所の台帳 (使える一覧・正規化・入口・Remote・ゲームパッド・
    ツールバーの補完)。長さ順の既存テスト (`duration_762` 一式) が全部通る。
  - 取得: ページ数だけの全体要件で本を全部数え、動画を probe しない。長さだけ・AI だけの全体要件で本を数えない。
    **バッジと詳細表示のページ数列を両方 OFF にしてページ数順を選んでも数える** (Planning の選別・cached 判定・
    target 生成・0 件の完了の全部)。proof に page_count が立つ・失効する。読み込み中の表示と停止 / 再開。
  - 画像フォルダ: メモリ・facts の照合が `(mtime, 0)` で当たり、サイズ順の `file_size: None` は変わらない。
  - Susie のプールの差し替え: 全件の取得の途中で差し替わると、その session は「古くなった」として `NotRequested` へ
    戻り、新しい版で始め直して完了する (`Cancelled` で止まらない、数えていない本を含む証明が立たない)。
  - 入口: ページ数順への変更・選び直しが worker 走査を使う。
  - スタック・Remote・使えない一覧の正規化・ツールバーの 12 → 14 値の一回限りの補完・旧版相当の enum で読んで
    DB を書き換えないこと。
- 関連する既存テスト: `duration_762` の visible_stages、`media_resume_meter_tests`、metadata_ops の
  `details_page_count_*`、`facet_835`。
- perf: `details_meta/load_done` の新しい件数の出力。実機の perf ログで、冷たいキャッシュ・入れ子の合本・
  scroll による取消のときの件数と ms を見る。
- 実機確認 (利用者): ZIP・PDF・EPUB・画像フォルダ・RAR・ZIP 内の書庫が混ざるフォルダで、初回にバッジが
  順に出ること、2 回目はすぐ出ること、評価の絞り込み中の一致数との並び、読書位置のメーターとの並び、設定の OFF。

## 9. 独立レビュー

- 1 回目 (Sol / xhigh、read-only、第1版 `7aa5c6918`): **REVISE**。
  - P2-1 メモリのページ数が判定設定の変更・EPUB の変換の後も古いまま → §4.3.1 で identity を追加。
  - P2-2 ZIP の初回費用 (入れ子の合本) と優先度 (範囲内はすべて Normal) の記述が不正確 → §4.2 を訂正、計測に追加。
  - P2-3 `ZipDir` の `warm_page_count` は UI スレッドで部分木を再帰集計する → §4.9 に費用の上限の根拠を追加。
  - P2-4 RAR とフォルダの走査に cancel が届かない → §4.4.1 を追加。
  - P3 OFF 条件の試験の前提 (再生位置メーターも既定 ON) → §8 を修正。
  - 述語の分け方と置き換える箇所の範囲、`allow_page_count` / `whole_listing_only` の制限、配置の優先順位、
    detached の bundle に新しい field が要らないこと、Remote に変更が要らないこと、並べ替えを別項目にすることは
    妥当とされた。
- 2 回目 (Sol / xhigh、同じセッション、第2版 `5fed5048d`): **REVISE**。
  - P2-1 表示とソート値が照合を通らず、Ready の後や詳細表示のページ数ソートでは取り直しが始まらない → §4.3.1 で
    「取り直しの判定」と「表示する値」を分け、取り直しは一覧の読み直しと既存の段階取得に乗せた。
  - P2-2 EPUB の revision を上げる境界が UI の受理に依存し、worker の公開・Remote・`detach_missing` を追えていない
    → revision をやめ、EPUB の値を一覧の世代の中だけで有効にした。
  - P2-3 `ZipDir` の集計は target の再作成で繰り返され、計測の区間にも入らない。レーティング一覧の `ZipDir` は
    兄弟とは限らない → 集計を `ZipTree::build` へ移し、UI スレッドで再帰しないようにした。
  - P3 取消の計測は `load_done` に届かない → `load_cancelled` にも載せる。
  - P3 「設定を戻せば記録で済む」は主キーの都合で成り立たない → 記述を直した。
  - fingerprint を App 全体に置くこと、Susie の扱い、再開ループが無いこと、Remote / MCP に独自の EPUB 変換の
    公開経路が無いことは確認された。
- 3 回目 (Sol / xhigh、同じセッション、第3版 `e6d109cbb`): **REVISE**。
  - P2-1 フレームの先頭で計算した fingerprint を job に使うと、同じフレームの途中で適用された環境設定と食い違い、
    ZIP は食い違った fingerprint で catalog に書く。`image_folder_page_count_options` も別に計算している
    → job の判定設定と fingerprint を同じ snapshot から作り、判定用の値は呼び出しごとに計算する形にした。
  - P2-2 部分木の画像数は `ZipTree::project` の投影の木にも要る → 両方の経路で持たせる不変条件にした。
  - P2-3 Susie の「プラグインを再読み込み」と起動時のプールの差し替えは一覧を読み直さない → 次の取得要求まで
    最後の値、範囲内は自動で取り直す、と決め、起動時の数え直しの費用を書いた。
  - P3 Ready の後の取り直しの説明が実際の条件と合わない → 「次に実際の取得要求が始まるまで最後の値」に統一した。
  - 「取り直しの判定」と「表示する値」を分ける方針は成り立つこと、同じ fingerprint・同じ世代で EPUB を繰り返す
    ループが無いこと、完了時の詳細の並べ直しが世代を進めないこと、AI / 長さの全件取得と facet に干渉しないこと、
    親のフォルダへ戻ると一覧を install し直すこと、は確認された。
- 4 回目 (Sol / xhigh、同じセッション、第4版 `d93388e71`): **REVISE**。
  - P2-1 設定の snapshot だけでは Susie のプールを固定できず、job の途中の差し替えで古い fingerprint の印のまま
    新しい規則の数を書ける → プールの版数を job の開始時に取り、1 冊ごとに書く前に見直して、変わっていれば捨てる。
  - P2-2 RAR の検査キャッシュが認識規則を区別しない → キーにプールの版数を足す。
  - P3 起動時の差し替えで数え直す費用は、今の詳細列には無い新しい費用 → 記述を直した。
  - 判定用の fingerprint を呼び出しごとに計算することは妥当 (判断の差だけで再開や取消を繰り返さず、設定とプールが
    落ち着けば収束する)、`DetailsPageCountConfig` を作る本番の入口が 1 つであること、`build` / `project` の両方での
    事前集計と不変条件、「最後の値」と実際の取得の開始を分けた説明は妥当とされた。
- 5 回目 (Sol / xhigh、同じセッション、第5版 `2fbcdef49`、§6 のページ数順を含む): **REVISE**。
  - P2-1 プールの版数の変化を取消と同じに扱うと、全件の取得が `Cancelled` のまま止まる。その本だけ飛ばすと
    数えていない本を含む証明が立つ → session を「古くなった」として `NotRequested` へ戻す既存の経路を使う。
  - P2-2 `ZipDir` の数を新しい fingerprint の印でメモリに持つと、古い木の数が残る → `ZipDir` は木の値を accessor が
    直接読み、details-meta の target にしない。
  - P2-3 ページ数の全体要件が、表示の需要を見ている関門 (選別・cached 判定・target 生成・0 件の完了) を通らない
    → 全部の関門に足すと明記した。
  - P2-4 画像フォルダの stamp は表示用が `(mtime, 0)`、並べ替え用が `file_size: None` で、長さの reader の形では
    当たらない → ページ数の identity を種類ごとに決め、フォルダは `(mtime, 0)` とした。入力は `scan.folders` 側。
  - P3 入口の関数名が今のコードと違う → 今の `apply_sort_change_reload` を再利用すると直した。
  - プールと版数を同じロックで公開・取得すれば版数の方式で空 → 作り終えたプールの差し替えも捉えられること、RAR の
    検査キャッシュのキー追加、ZIP の `NESTED_CACHE` と目次・名前索引のキャッシュは認識した画像の集合を持たないので
    変更不要なこと、遅延キーの共通化・不明は末尾・スタックの構造体の持ち回り・Remote の正規化を共有する方針は
    妥当とされた。利用者への事前説明では、ツールバーの補完でページ数順を使わない利用者にも旧版との設定の境界が
    及ぶ点を残すこと (§7 の問 4)。
- 6 回目 (Sol / xhigh、同じセッション、第6版 `042c98a7b`): **REVISE** (1 件)。
  - P2-1 `ZipDir` を target から外すだけでは、段階取得・選択情報の需要の判定がメモリの無い `ZipDir` を未充足と見て、
    0 件の target → Ready → 段階取得を繰り返す。木が無い `ZipDir` を未取得の表示へ流すと「…」のまま → 表示できるかと
    非同期に取る必要があるかの述語を分け、需要の述語は全部後者を使う。木が無いときは「-」・不明・バッジなし。
  - ほかの 5 回目の指摘は解消とされた。「古くなった」終わり方は今の pending と受信口を破棄する既存の経路を使えば、
    停止の後の遅れた通知で再開しない。既存の経路は proof を消さず新しい証明も立てないので、有効だった AI / 長さの
    証明は保たれる。フォルダの identity、表示の設定から独立した全体要件、今の入口の再利用にも追加の指摘なし。
- 7 回目 (Sol / xhigh、同じセッション、第7版 `9f7148eda`): **ACCEPT_WITH_NITS**。P1・P2 なし。
  - P3 詳細表示の列見出しの並べ替えは「どの一覧でも使える」わけではない (ページ順が固定の一覧・手動 / シャッフル順の
    コレクションでは禁止) → 「今使える一覧で引き続き使える」に直した (第7.1版)。
  - `ZipDir` の問題は、表示できるかと取得の需要の分離、木が無いときの Unavailable で解消とされた。§4・§6 全体で、
    「古くなった」終わり方と停止 / 再開、既存の proof の保持、フィールドごとの要求、フォルダの identity、配置、
    detached / Remote、設定の互換の扱いに追加の設計上の問題は無いとされた。

## 10. 実装記録 (段 1)

2026-11-24。段 1 のソースと回帰テストを追加した。段 2 の `SortOrder`・一覧全体のページ数要件・
catalog facts・proof 拡張は実装していない。catalog schema / CATALOG_VERSION は変更なし。
差分は未コミット。コンパイル・cargo テスト・check・clippy・確認用ビルド・製品起動は利用者の指示で実施しない。
以下は実装箇所の台帳であり、実行済みテストやレビューの結果を示すものではない。

### 10.1 取得の述語・値の所有者

旧 `thumbnail_media_metadata_enabled()` の 8 呼出を次のように振り分けた (`src/app.rs`)。

| 呼出元 | 新しい述語 |
| --- | --- |
| `details_lazy_uses_visible_stages` | `thumbnail_badge_metadata_enabled` |
| `details_visible_stage_needs_load` | 同上 |
| `refresh_thumbnail_details_stage` の全体取得との関係を判定する箇所 | 同上 |
| `refresh_thumbnail_details_stage` の keep set を選ぶ箇所 | 同上 |
| `details_lazy_columns_visible` | 同上 |
| `selection_info_only_lazy_load` | 同上 |
| `start_details_meta_load_for_scope` の Explicit 範囲を選ぶ箇所 | 同上 |
| `lazy_load_video_meta_for_idx` | `thumbnail_media_fields_enabled` |

ページ数のフィールド要件は `thumbnail_page_count_enabled`。`allow_page_count` と `whole_listing_only` を維持し、
AI / 長さの全体取得だけで表示集合外の本を数えない。設定変更は preferences の
`thumbnail_badge_requirements_changed` から既存の `invalidate_details_meta_requirements` へ接続した。

需要の述語は `details_item_page_count_needs_fetch` (= 表示対応かつ ZipDir 以外)。
`lazy_load_page_count_for_idx` と `selection_info_item_requires_lazy_meta` が直接使い、
段階取得・Planning の選別・cache 充足・target 作成はこれらから需要を導く。
表示対応の `details_item_supports_page_count` は従来どおり ZipDir を含む。
`DetailsMetaTarget.warm_page_count` と worker のコピー経路を削除した。

`details_page_count_value` が値の単一 accessor。詳細文字列・詳細ソート値・サムネイル文字列がこれを通り、
選択情報は従来の `details_page_count_text` 経由で読む。ZipDir は現在の `zip_nav.tree` を直接引く。
木が無い / prefix が無い場合は Unavailable (「-」・不明・バッジなし)、非同期取得は要求しない。
ZipTree は build と project_node の両方で部分木数を確定し、prefix 読取・total_image_count は再帰集計しない。

### 10.2 identity・snapshot・取消

- `DetailsLazyMeta` に fingerprint と EPUB だけの generation を追加。ページ数 patch の採用時だけ一緒に置換する。
  `details_page_count_meta_satisfies_idx` の共通本体 (`_with_fingerprint`) で checked・fingerprint・
  EPUB の items_generation・既存 PDF password revision を照合する。表示 accessor は取り直しの identity を照合しない。
- 現 fingerprint は `details_visible_stage_needs_load`、選択情報の要求判定、poll の優先対象選別、Planning の 1 slice、
  worker 起動時の優先対象選別、表示集合拡大の既存走査の各呼出で 1 回計算し、ループへ渡す。App には保存しない。
- job config は `image_folder_page_count_options` を 1 回作り、その fingerprint / pool_revision を採用する。
  テスト helper `archive_details_config` も同じ。`susie_loader::pool_snapshot` は同じ read lock で pool と版数を読む。
- 版数を上げる箇所は `PoolState::replace` に集約し、`init_pool` の完成 pool 公開と `reload` の公開から呼ぶ。
  初期 empty pool の版数は 0。pool の入れ替えと同じ write lock の中で版数を上げ、旧 pool の drop は lock の外で行う。
  `INIT_GENERATION` は従来の初期化の supersession 用に保持する。
- 照合点: ページ数取得入口、catalog cache hit 採用前、各種 catalog 書込前、worker の Item / Finished 発行前、
  UI poll の開始と event 採用前、Planning の継続と worker 起動 (0 target 完了も含む)。
  不一致は worker の `Stale` 通知または UI 側の既存 session 破棄へ進め、受信口を捨てて NotRequested に戻す。
  Cancelled / Finished にせず、既存 proof は保ち、新しい proof は立てない。既存 poll の再開経路を使う。
- RAR `DECISION_CACHE` のキーは元の source key + pool revision。volume resolution cache は画像認識の値を持たないので変更なし。
- RAR の検査へ job cancel を渡し、画像フォルダも共通 scanner の各 entry の取消確認を使う。
  本番は `image_folder_page_count_with_cancel`、旧引数形の `image_folder_page_count` は既存テスト用の wrapper だけとした。
  取消した走査結果は catalog / checked へ残さない。
- detached の述語・viewport・viewer context bundle の swap field は変更なし。

### 10.3 描画・設定・perf・文書

- `ThumbnailOverlayLayoutInput.page_count` / `BadgeKind::PageCount` / `BadgePriority::PageCount` /
  `ThumbnailOverlayLayout.page_count` を追加。一致数の後、長さの前に同じ `place_bottom_right` で配置する。
  下端 meter の予約と既存 badge の位置を保持し、長さと同じ style / painter で「123P」を描く。
- accessor の呼出は ui_main の `repaint_grid_cell_after_input` と `render_grid` の 2 経路。
  layout へ渡すのはその 2 箇所と、render_grid 内の同 frame check / cut 変更後の再 layout の計 3 箇所。
  実描画は共通 `app/grid_paint.rs::paint_cell` の 1 箇所。fixture を含む `layout_cell_overlays` の caller 28 個を新引数へ揃えた。
- 既定 ON / 欠落時 ON、環境設定の説明と検索 anchor、転送 policy と件数を更新。
  マニュアル grid / settings、製品ページの機能一覧、ui-responsiveness、architecture-overview、virtual-folders を更新した。
  CHANGELOG.md / changelog.html は変更なし。
- `load_done` / `load_cancelled` に page_targets・page_cache_hits・page_counted (zip / zip_nested / folder / rar / pdf / epub)、
  page_failed・page_ms を追加。200 ms 以上の `page_count_slow` に kind / ms / cache を出す。
  nested ZIP は新規列挙時に ancestry で分類する。既存 catalog は nested の印を持たないため cache hit は zip に含め、
  計測のための追加 I/O は行わない。

### 10.4 追加テストと設計担当向け filter

次は追加したテスト名 (すべて未実行)。共通 prefix は表の module に属する。

| module | テスト名 |
| --- | --- |
| `app::tests::page_badge_883` | `page_badge_883_page_only_requests_near_books_without_media_probe` |
| 同上 | `page_badge_883_media_and_resume_only_keep_page_fields_off` |
| 同上 | `page_badge_883_all_badges_off_do_not_start_a_session` |
| 同上 | `page_badge_883_selection_only_keeps_single_book_requirement` |
| 同上 | `page_badge_883_whole_listing_ai_and_duration_do_not_expand_page_scope` |
| 同上 | `page_badge_883_fingerprint_refetch_keeps_last_display_and_sort_values` |
| 同上 | `page_badge_883_epub_generation_refetches_success_and_absence_only_once` |
| 同上 | `page_badge_883_values_hide_unchecked_absent_zero_failed_and_drive_rows` |
| 同上 | `page_badge_883_zipdir_reads_tree_and_never_starts_repeated_lazy_sessions` |
| 同上 | `page_badge_883_stale_susie_pending_restarts_preserving_proof` |
| 同上 | `page_badge_883_stale_worker_event_restarts_preserving_proof` |
| 同上 | `page_badge_883_preference_toggle_cancels_old_requirements_but_keeps_values` |
| 同上 | `page_badge_883_planning_keeps_one_recognition_snapshot_until_next_job` |
| `thumb_overlay_layout::tests` | `page_count_883_uses_bottom_right_and_length_style_without_abbreviation` |
| 同上 | `page_count_883_lifts_above_filter_count_and_book_resume_meter` |
| 同上 | `page_count_883_yields_without_changing_existing_badges` |
| `ui_dialogs::preferences::tests` | `page_count_883_preferences_toggle_invalidates_requirements_and_unchanged_ok_preserves_state` |
| `settings::tests` | `thumb_show_page_count_defaults_on_and_preserves_disabled_setting` |
| 同上 | `thumb_show_page_count_missing_db_key_defaults_on_and_false_roundtrips` |
| `settings_transfer::tests` | `thumb_show_page_count_transfer_roundtrip_missing_invalid` |
| `susie_loader::tests` | `pool_replacement_publishes_revision_with_the_pool` |
| `rar_loader::tests` | `decision_cache_is_scoped_to_recognition_pool_revision` |
| 同上 | `cancelled_header_inspection_does_not_cache_a_partial_decision` |
| `zip_tree::tests` | `subtree_page_counts_follow_build_and_each_policy_projection` |
| `app::folder_scan::page_count_tests` | `image_folder_page_count_cancellation_is_not_an_empty_book` |
| 同上 | `page_count_scan_cancellation_discards_a_partially_enumerated_folder` |
| `app::metadata_ops::tests` | `page_count_revision_change_during_enumeration_never_writes_catalog` |
| 同上 | `page_count_cancelled_during_enumeration_never_writes_catalog` |
| 同上 | `stale_page_count_worker_emits_no_item_or_finished_proof` |
| 同上 | `precancelled_rar_and_folder_page_workers_publish_no_checked_value_or_catalog` |
| integration `ui_snapshot` | `page_count_883_badges_dark` (定義のみ、PNG は設計担当が作成) |

既存の nested ZIP 取得テストに fresh / cache hit trace の確認を追加した。
旧 `child_archive_page_count_uses_warm_zip_tree_total` は
`child_archive_page_count_reads_zip_tree_total_without_worker_target` へ更新。
duration_762 の visible stage 5 件は名前と worker 完了 / proof の検査意図を保ち、
fixture を ZipDir から一時 ZIP に置き換えた。手動 Pending の pool revision、checked 値の fingerprint、
既存の全 OFF / 長さ専用 / meter 専用 / facet fixture の明示 OFF を更新した。

設計担当が実行する回帰 filter (この実装作業では実行しない):

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/page_badge_883|page_count|pool_replacement_publishes_revision|decision_cache_is_scoped|subtree_page_counts|settings_transfer::tests|duration_762|media_duration_thumbnail|media_resume_meter|facet_835|details_lazy_session|details_cell_content_revisions|thumbnail_selection_info_keeps_single_target_fast_path/)'
```

統合テストは `ui_snapshot` の `page_count_883_badges_dark`。PNG 生成・目視・通常比較は設計担当が実施する。
実機の混在一覧・初回 / cache hit・一致数 / meter 併設・OFF・EPUB 変換後の帰路・scroll 取消と perf は未確認。

### 10.5 設計との差と静的確認

機能仕様・所有境界の変更はない。名前の補助 (`_with_fingerprint` / `_with_cancel`) と nested ZIP cache hit の
計装上の分類は上記のとおり。新しい I/O / worker / 永続 schema / viewer swap field は追加していない。
グリフ検査は危険字形 0、製品ページのカード検査は 53 件・上限超過 0。
ワークスペース全体の `cargo fmt` (引数なし) と最終 `git diff --check` は exit 0。
コンパイル可能性・テストの成否は未確認。

変更ファイル (fixture の引数追加を含む):

- App / worker: `src/app.rs`、`src/app/metadata_ops.rs`、`src/app/folder_scan.rs`、
  `src/susie_loader.rs`、`src/rar_loader.rs`、`src/zip_tree.rs`。
- 設定 / UI: `src/settings.rs`、`src/settings_transfer.rs`、`src/ui_dialogs/preferences.rs`、
  `src/ui_dialogs/preferences/pages.rs`、`src/ui_dialogs/preferences/search_index.rs`、
  `src/thumb_overlay_layout.rs`、`src/app/grid_paint.rs`、`src/ui_main.rs`。
- 回帰 / fixture: `src/app/tests.rs`、`src/app/tests/page_badge_883.rs`、`src/app/tests/duration_762.rs`、
  `src/app/tests/duration_762/visible_stages.rs`、`src/app/tests/facet_835.rs`、`src/app/media_resume_meter_tests.rs`、
  `src/app/container_type_icon_fixture.rs`、`src/app/folder_background_fixture.rs`、`src/app/grid_type_icon_tests.rs`、
  `tests/ui_snapshot.rs`。
- 文書: 本書、`docs/ui-responsiveness.md`、`docs/architecture-overview.md`、`docs/virtual-folders.md`、
  `htdocs/mimageviewer/manual/grid.html`、`htdocs/mimageviewer/manual/settings.html`、`htdocs/mimageviewer/index.html`。

## 11. 実装記録 (段 2)

2026-11-24。§6 のページ数順を実装した。段 1 のコミット `ee08106d5` からの未コミット差分。
以下はソース・試験定義・静的照合の台帳であり、コンパイルやテストの成功を示さない。
利用者の指定により cargo build / test / check / clippy、確認用バイナリの作成・起動、
git commit / branch / stash / reset は行わない。更新履歴・catalog schema / CATALOG_VERSION は変更なし。

### 11.1 遅延キーへの振り分け台帳

`SortOrder::lazy_metric() -> Option<LazyMetric>` を追加した。`is_duration()` と
`SortOrder::Duration*` の非テスト参照、および以前の正規化関数を次のように振り分けた。

| 場所 | 遅延キー全体へ共通化したもの / 長さ固有として残したもの |
| --- | --- |
| `settings.rs` | `LazyMetric`・ページ数の2 variant・比較・ラベルを追加。collection / folder-thumb は既存 allowlist のまま。ツールバーの10→12移行は長さ固有のまま固定し、12→14移行を別 marker にした。`listing_may_sort_by_duration` は長さ facts のため維持し、ページ数の同型 OR を追加 |
| `app/listing_sort.rs` | `duration_allowed` → `lazy_metric_allowed`、`without_duration_order/request` → `without_lazy_metric_order/request`。非対応理由は種類別。使える一覧・固定順・既存ロックの判定は共通 |
| `ui_main.rs` | `sort_control_decision`、再選択の worker reload、ラベルと tooltip の進捗を共通化。長さ facet の条件・表示は長さ固有のまま |
| `app/gamepad_input.rs` | 同値再選択の扱いを共通化。巡回は既存 capabilities 経由で両種類の非対応候補を飛ばす |
| `remote_ipc/mod.rs` | 候補・wire 値・実効順の正規化を共通化。`remote_ipc/container.rs` の既存試験もページ数を含めた |
| `global_search_ui.rs` / `global_search_ui/everything.rs` | producer の要求・比較・projection の名前順正規化を共通化 |
| `app/smart_folder.rs` / `app/subfolder_expansion.rs` | worker / resident / materialize の要求正規化を共通化 |
| `rating_view.rs` / `bookmark_browser.rs` | 表示候補と内部 producer の正規化を共通化 |
| `collection_store/model.rs` / `thumb_loader.rs` | 明示候補に含めず、網羅 match の非対応側へページ数を追加 |
| `app.rs` / `app/collection_navigation.rs` | 5 worker 入口にページ数要求を追加。reconcile の `is_duration()` は `lazy_metric()` へ。既存 `apply_sort_change_reload` をそのまま再利用 |
| `app/duration_metadata.rs` | 要件・proof・読み込み文言・メモリ snapshot は共通。長さ facet・`duration_lengths_wanted`・`media_duration_from_meta`・`listing_duration_for_path` は長さ固有 |
| `app/book_materialization.rs` | `media_sort.is_duration()` は動画・音声の reader を呼ぶ条件なので維持。ページ数 reader は PageCount の要求時だけ `scan.folders` を読む |
| `settings_transfer/stage1.rs` / `settings_transfer.rs` | 候補にページ数2種、移行 marker は転送対象外へ分類。分類数は520 (転送174・除外346) |

本番で残る `is_duration()` は、`settings.rs` の長さ facts の保守的 OR、
`app/duration_metadata.rs` の長さ需要、`app/book_materialization.rs` の動画・音声読取りだけ。
`physical_duration_sort_surface/order` の名前は既存 caller のまま残すが、処理本体は共通 capabilities を使う。
旧 duration collector / materialize wrapper と duration snapshot helper は既存試験の入口として維持した。

### 11.2 facts・メモリ・materialize の照合

- `ListingSortMetadata.page_count` は既定 Missing。Known は昇順 / 降順で比較し、Unavailable と Missing は
  両方向とも末尾・名前昇順。同値も名前昇順、4行割当を越えない。スタックのメンバーと passthrough は
  構造体全体を持ち回り、集約セルは `ListingSortMetadata::new` の Missing を使う。
- `listing_page_count_for_path` は source stamp・checked・要求 fingerprint を照合する。
  PDF/EPUB のメモリは既存 credential revision も照合する。EPUB の一覧世代は見ない。
  表示の最後の値を読む `details_page_count_value` と、全件取得で世代を見て取り直す述語は変更しない。
- UI の closure と worker の `FolderListingMetadataSnapshot` は同じ accessor を使う。
  snapshot は長さ・ページ数を1構造体にまとめ、要求した種類だけを作る。新しい UI I/O はない。
- `FolderPageCountFacts` は要求 fingerprint と一緒に運び、materialize に渡した現 fingerprint と一致するときだけ使う。
  名前・kind・mtime・size・fingerprint が一致する container 行、または名前・mtime・size が一致する PDF 行を使う。
  画像フォルダは `ScannedFolderEntry.display_meta` の `(mtime, 0)`、サイズ順の `file_size: None` は維持する。
  EPUB と password_required PDF は facts を使わない。メモリ → facts → Missing の順、メモリの Unavailable も優先する。
- `CatalogDb::load_listing_page_count_facts` は既存2表を2 SELECTで読むだけ。
  `collect_folder_listing_metadata_facts` は不要な要求 / 対象なしでは開かず、必要時は長さと1接続を共有する。
  `open_existing_read_only` が `open_existing_read_only_admitted` へ委譲する既存 admission を使い、
  不在は空、異常はログと空 facts、schema を初期化・移行しない。`listing/page_count_facts` に rows / ms / failed。

### 11.3 入口・全件取得・所有境界

長さ計画 §11.2 の入口台帳への追記:

| 入口 | 段2の追加 |
| --- | --- |
| open-path-classify | 共通・live・保存済みお気に入りの OR と認識設定から作る fingerprint を要求へ捕捉し、directory scan後に共通 collector |
| Ctrl+↑↓ / DFS・兄弟・一覧順移動 | 行き先 scan の後に同じ flag / fingerprint で collector |
| folder pane・更新・並べ替え変更 / 同値再選択 | `start_folder_open_scan_with_restore_and_navigation` の既存 worker に同じ要求を追加 |
| BS・履歴・物理 preflight | `app/collection_navigation.rs` の directory 分岐へ同じ要求を追加 |
| 外部変更 reconcile | 共通 collector と要求時メモリ snapshot を worker materialize へ渡す |
| 変換後の親採用・従来の同期入口 | 新しい catalog 読取を足さない。準備済み facts またはメモリだけ |
| Smart/Search/Collection等・Remote・固定ページ一覧 | 共通の正規化 / 既存固定順を保つ |

- `ListingMetaRequirements.page_count` を継続条件の `page_counts_wanted` から作り、WholeListing の
  Planning 選別・cached 充足・target の load_page_count に渡す。全体要件は allow_page_count /
  whole_listing_only と表示設定を越えるが、対象は `details_item_page_count_needs_fetch` だけで ZipDir は取らない。
  ページ数だけで動画 probe・AI・作成日時・寸法を増やさず、長さ / AI だけで本を数えない。
- proof の union / contains / without / perf と `page_count_scan_ready` に page_count を追加。
  通常 Finished、全cache hitでtarget 0件、計画対象0件のすべてが同じ `finish_details_lazy_session` へ snapshot を渡す。
  列見出し用の `load_all_page_counts` は既存の別経路のまま。完了時に一覧の並びを変更しない。
- Susie の版変更と Stale は段1の破棄 → NotRequested → 新sessionの経路を使い、Cancelled にしない。
  利用者の停止・明示再開、既存 proof 保持、membership変更時の取り直しも共通経路。
- bundle / swap field は増やしていない。`viewer_context_registry.rs` は既存試験の requirements literal へ
  page_count=false を足しただけ。detached 述語・viewport・実装の park / mount / drop は変更していない。

### 11.4 追加・変更した試験と filter

以下の37件を追加した (全件未実行)。名前の prefix は省略せず記載する。

`app::tests::page_sort_883` (17件):

- `page_sort_883_whole_worker_counts_hidden_books_with_all_page_displays_off`
- `page_sort_883_stale_session_restarts_and_completes_from_current_memory`
- `page_sort_883_requirements_pass_all_target_gates_with_badge_and_column_off`
- `page_sort_883_planning_includes_filtered_offscreen_books_and_rejects_stale_cache`
- `page_sort_883_cached_unavailable_zero_and_empty_listing_complete_without_worker`
- `page_sort_883_finished_and_visible_stage_proofs_do_not_reorder_items`
- `page_sort_883_membership_changes_retire_sessions_and_reprove_current_snapshot`
- `page_sort_883_stop_survives_facet_rebuild_until_explicit_resume`
- `page_sort_883_stale_susie_or_worker_event_restarts_without_false_proof_or_user_stop`
- `page_sort_883_memory_and_worker_snapshot_match_stamp_fingerprint_and_folder_zero_size`
- `page_sort_883_epub_sort_ignores_generation_but_scan_requires_refetch`
- `page_sort_883_memory_preserves_unavailable_and_rejects_unchecked_or_old_pdf_credentials`
- `page_sort_883_pending_snapshot_keeps_promised_page_fields_after_sort_change`
- `page_sort_883_duration_and_ai_requirements_do_not_count_books`
- `page_sort_883_requirements_algebra_keeps_page_count_independent`
- `page_sort_883_sort_selection_and_reselection_request_worker_refresh`
- `page_sort_883_proof_cache_and_pending_follow_context_park_mount_drop` (Windows)

facts / materialize / catalog (10件):

- `listing_page_count_bulk_read_preserves_identity_on_read_only_connection`
- `listing_page_count_bulk_read_does_not_migrate_missing_tables`
- `page_count_883_facts_match_kind_stamp_and_fingerprint_but_exclude_epub_and_password_pdf`
- `page_count_883_materialized_memory_facts_missing_and_worker_snapshot_agree`
- `page_count_883_materialized_stale_facts_and_absent_facts_use_only_memory`
- `page_count_883_unavailable_memory_overrides_known_catalog_value`
- `page_count_883_real_folder_scan_matches_details_catalog_stamp_without_known_size`
- `page_count_883_non_page_sorts_never_read_page_memory`
- `page_count_883_catalog_collection_skips_unwanted_and_empty_and_missing_catalog_is_empty`
- `page_count_883_duration_and_page_catalog_facts_share_exactly_one_open`

設定・UI・スタック・4行 (10件):

- `page_count_sort_unknowns_last_both_directions_and_name_ties`
- `page_count_wire_and_scan_common_live_or_saved_favorite_sort`
- `page_count_toolbar_migration_only_extends_untouched_defaults_once`
- `toolbar_page_count_marker_survives_db_reload_without_restoring_hidden_options`
- `page_count_settings_roundtrip_and_older_reader_preserves_incompatible_db`
- `page_count_picker_selected_page_count_uses_worker_reload`
- `page_count_sort_controls_show_progress_only_for_selected_page_count`
- `page_sort_883_stack_metadata_survives_flat_and_keeps_aggregate_missing`
- `page_sort_883_extract_stack_parts_preserves_book_page_counts_and_folder_size`
- `page_sort_883_arrange_compares_only_within_assigned_rows`

既存試験の追従:

- `duration_sort_wire_and_candidate_allowlists` は現候補14件と全wireを確認。`duration_toolbar_migration_only_extends_untouched_defaults_once`
  は歴史的10→12の意図を保つため新PageCountを除いた12件を期待する。現在のload / marker fixtureは12→14も通す。
- `duration_762` / `sort_hold_838` の capabilities 名、`facet_835` / registry の requirements literal は機械追従だけ。
  長さの取得・proof・facet・停止再開の期待は変えていない。
- listing_sort の対応表・正規化、メニュー / 各toolbar表示 / 再選択、ゲームパッド巡回、Remoteの候補とmaterialize、
  rating / bookmarkの全候補試験をページ数へ拡張。`all_settings_fields_are_classified` は marker 1件分だけ総数 / 除外数を増やした。
- 設計担当の関連2171件中2167成功・4失敗の報告後、コレクションpopupの3試験を16行に合わせ1093×614 logical points・UI倍率150%の全行／無効理由／初回clip検査へ更新し、600×480・360×150のscroll検査を維持、評価行の固定indexもページ数2行分を補正した (製品挙動・既定RhaiのCRLFは変更なし、修正後のcargo検証は未実施)。
- 3件の再失敗を受け、設計担当の指示で `grid_sort_combo` の Area 初回高を計算済みviewport高＋Frame余白へ揃えた (egui既定400−枠余白14＝386では16行の最低406を収容できないため)。
  `Popup::show` はstyle適用より前にAreaを作るので、Contextの該当themeを作成中だけ差し替え、内容callback前／閉状態／unwindで復元する。vendor・他メニューの既定・行と候補は変更なし。
  既存3件の期待と小画面scrollを維持し、`sort_popup_actual_listing_routes_fit_all_rows_at_minimum_size_and_150_percent` (実Folder／展開／評価／bookmark、最大17行) と `sort_popup_area_height_restores_both_themes_before_contents_and_other_areas` を追加。snapshot候補は `duration_facet_bar{,_narrow}_dark` と `facet_sort{,_narrow,_edges,_edges_narrow}_dark` (現fixtureは閉状態、PNG未変更・cargo検証未実施)。
- 設計担当の再実行2310件中2307成功・snapshot277成功後、矩形を持たないListBox役割nodeのqueryを、保持中popup IDの `memory.area_rect` 読取へ修正した。
  600×480は400pt制約解消後に収まるので全行表示を検査し、確実に内容高を下回る360×150ではoverflow・実wheelで末尾到達・scroll後の画面内保持を検査する (今回は製品コード・PNG・既定Rhai変更なし、修正後のcargo検証未実施)。

設計担当が実行する nextest filter (本作業では実行しない):

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/page_sort_883|page_count|duration|duration_762|page_badge_883|facet_835|settings_transfer::tests|fixed_sort_control|toolbar_wrap|aggregate_picker|rating_sort_click|book_and_extracted_rows|existing_sort_locks|sort_hold_838|listing_sort|ai_facet_|media_resume_meter|details_lazy_session/)'
```

既存の `duration_762` は全件を含める。UI / native / cold-cacheの実機、Susie実プール差し替え中のI/O、
大規模本棚・入れ子書庫のperf、旧製品自体を起動する往復は未確認。

### 11.5 設計との差・未確認範囲・変更ファイル

機能仕様・所有境界の変更はない。既存名の wrapper と、テスト可能な1openの `FnOnce` helper を残した。
互換試験は新旧enumの読取りと settings.db の Incompatible / 保存抑止の契約を検査する定義であり、
旧製品による adjustment.db 全体の保護は保証しない。現行の `hydrate_favorite_view_states` には
settingsのfavoritesに無いUUIDを掃除する別経路があるため、旧製品実行時の全保存データ保護をこの試験から推論しない。
favorite state_json の読取自体がDBを書かないことは別に試験を追加した。

変更ファイル:

- 取得・走査・並び: `src/app.rs`、`src/app/duration_metadata.rs`、`src/app/folder_scan.rs`、
  `src/app/book_materialization.rs`、`src/app/collection_navigation.rs`、`src/catalog.rs`、`src/grid_item.rs`。
- 設定・操作: `src/settings.rs`、`src/settings_db.rs`、`src/settings_transfer.rs`、`src/settings_transfer/stage1.rs`、
  `src/app/listing_sort.rs`、`src/ui_main.rs`、`src/app/gamepad_input.rs`。
- producer / スタック: `src/global_search_ui.rs`、`src/global_search_ui/everything.rs`、`src/app/smart_folder.rs`、
  `src/app/subfolder_expansion.rs`、`src/rating_view.rs`、`src/bookmark_browser.rs`、`src/remote_ipc/mod.rs`、
  `src/remote_ipc/container.rs`、`src/collection_store/model.rs`、`src/thumb_loader.rs`、`src/filename_stack.rs`、`src/filename_stack_ui.rs`。
- 回帰 / fixture: `src/app/tests/page_sort_883.rs`、`src/app/tests.rs`、`src/app/tests/duration_762/smart_destination.rs`、
  `src/app/tests/facet_835/proof.rs`、`src/app/tests/sort_hold_838.rs`、`src/app/similar_navigation_tests.rs`、
  `src/app/viewer_context_registry.rs` (既存試験のみ)。
- 文書: 本書、`docs/media-duration-sort-filter-plan.md`、`docs/ui-responsiveness.md`、`docs/virtual-folders.md`、
  `htdocs/mimageviewer/manual/grid.html`。

UIグリフ検査は危険字形0。全体の `cargo fmt` (引数なし) と `git diff --check` は exit 0。
コンパイル・cargoテスト・check・clippy・確認用ビルドは未実施。

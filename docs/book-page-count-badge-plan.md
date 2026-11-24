# 本のページ数バッジとページ数順 (§1.883)

最終更新: 2026-11-24

状態: 設計第4版。第1版 (`7aa5c6918`)・第2版 (`5fed5048d`)・第3版 (`e6d109cbb`) は独立設計レビュー
(Sol / xhigh) 1〜3 回目でいずれも REVISE。第2版からは、指摘を仕組みを足すのではなく状態を減らす形で直している。
第4版 (`d93388e71`) はレビュー 4 回目で REVISE。第5版は 4 回目の指摘を反映し、さらに利用者の決定
(2026-11-24、統括経由「ページ数バッジ・ソートまでいれて、次の5.7.0リリースへと進めたいと思います。」) に従い、
ページ数順を同じ版に入れる前提で §6 を設計し直した (§9)。実装はまだしない。**収録版は v5.7.0**。
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
- `ZipDir` (ZIP 内の書庫) は `ZipTree` から数えた値だけを出す (I/O なし)。

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
  - 1 冊を数え終えたら、catalog に書く前に版数を見直し、変わっていればその結果を捨てる (catalog に書かず、メモリの
    checked も立てない。取消と同じ扱い)。その本は次の取得要求で、新しい版のプールと fingerprint で数え直す。
  - 待つ状態や監視は足さない。プールの差し替えは起動時に 1 回と、利用者の再読み込みのときだけなので、捨てる
    結果は数冊に限られる。
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
  候補は「123P」「123p」「123ページ」。→ §7 の問 3。既定案は「123P」(短く、同人誌などの表記に近い)。
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
   - 画像フォルダ: `container_page_meta` (kind=Folder) の名前・mtime・fingerprint。**走査の結果から作るフォルダの
     mtime / size が、details-meta が記録を書くときの値 (`image_metas` 由来) と同じか、実装時にテストで確かめる**
     (違えば facts は当たらず、メモリと取得だけで並ぶ。誤った値は使わない)。
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
- 詳細表示の列見出しの並べ替え (`DetailsSortKey::PageCount`) はそのまま。どの一覧でも使える。

### 6.5 一覧全体の取得と完了の証明

- `ListingMetaRequirements` に `page_count: bool` を足す。`page_counts_wanted()` = 今の並べ替えがページ数順で、
  この一覧で使えること (`lazy_metric_allowed`)。**継続条件** (長さと同じく「未取得がある間だけ」にしない)。
- 対象はその一覧の **全部の本** (`items` のうち `details_item_supports_page_count` が真のもの。表示集合・絞り込み・
  可視範囲に依らない)。`whole_listing_meta_scan_wanted()` にページ数を足す。
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

- メニュー・ツールバー・ゲームパッドでページ数順へ変える、または選択中のページ数順を選び直すときは、物理フォルダの
  再読込を worker 走査 (`apply_sort_change_reload_without_ui_io`) で行う (長さ順と同じ入口の判定を遅延キー全体へ)。
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

## 7. 利用者に確認すること

1. **既定 ON の費用**: 既定 ON のため、利用者がバッジを使わなくても、初めて見る本を 1 冊ずつ数える I/O が
   可視範囲で増える (HDD・ネットワーク共有で、サムネイルの読み込みと取り合う)。記録済みなら 2 回目以降は軽い。
   既定 ON でよいか (要望どおり ON を推奨。費用は perf ログで確かめる)。
2. **画像だけのフォルダ**: 普通の写真フォルダにも、画像の枚数がバッジとして出る (詳細表示のページ数列と同じ)。
   「画像のみのフォルダを本として扱う」が ON のときだけにするか、常に出すか (推奨: 詳細列と同じく常に出す)。
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
- 値: `page_count = None`・0・失敗でバッジなし、PDF のパスワードの revision、`ZipDir` は `ZipTree` の値、
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
    proof に page_count が立つ・失効する。読み込み中の表示と停止 / 再開。
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
- 5 回目: 未実施。第5版は 4 回目の指摘に加え、ページ数順を同じ版に入れる §6 を新しく設計したので、§6 も対象にする。

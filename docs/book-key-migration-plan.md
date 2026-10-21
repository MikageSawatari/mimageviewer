# 本の構造変更と path キー DB の付け替え・後始末 (§1.272 / §1.273) と、しおりのページ番号 (§1.245)

2026-10-21 設計担当 ClaudeCode Opus 5.5。状態: **設計 v4 (v3 で実装前レビュー合意、実装中に出た 2 点を §3.10 で決定)。実装中**。
バックログの正本は [next-release-backlog.md](next-release-backlog.md) の §1.272・§1.273・§1.245・§1.244。
前提として読む文書: [製本計画](compile-book-plan.md) §2・§11、`src/rename_key_migration.rs` のモジュール doc。
`x-scheduled-post` ブランチの `docs/x-post-r2-design.md` §4 (関所つきの大掛かりな版) を参考にしたが、
関所・受領証・ストアごとの完了印は採らない (§6)。

## 0. 調査と実アプリ確認の結果

観測者はすべて ClaudeCode (作業ツリー `G:\home\mimageviewer-bookdata` の dev-runtime ビルド、
隔離 data-dir `target\dev-runtime\data-verify`、合成 fixture `target\verify-fixtures`)。2026-10-21。

| 項目 | 結果 |
| --- | --- |
| §1.244 列ソートが本の中の並びを崩す疑い | **再現しない**。詳細表示の通常フォルダでサイズ列の昇順に並べ替えた状態 (並びは d→c→a→b に変わった) で、名前順とサイズ順の異なる ZIP (01〜04) を開くと、一覧は 01→02→03→04、フルスクリーンの → 送りも 01→02→03→04。元のフォルダに戻るとサイズ順のまま (列ソートの設定は残っていた)。コード: `details_header_sort_active` が `details_header_sort_lock_reason()` 経由で `page_order_locked_for_current_view()` を見ており、`rebuild_details_order` もこの述語だけで所有者を決める。2026-09-22 の a81f13ced で構造的に直っていた。**修正なし、記録のみ** |
| §1.245 しおりの「P.N」「N ページ」 | **再現した**。サブフォルダ 2 つ (d1, d2) と画像 3 枚 (p1〜p3) の ZIP で p1 を開くとフルスクリーンのページ表示は「1 / 3」。B でしおりを付けると、左パネルは「3 ページ」、場所▼→ブックマーク の一覧の位置列は「ZIP・CBZ / 3 ページ」 |
| §1.272 / §1.273 | コードで確認 (下記 §1)。実アプリでの観測は修正後の確認で行う |

## 1. 現状 (コードから)

### 1.1 並べ替え・別本への移動/コピー・本の改名 (§1.272)

- FS は製本 worker (`books::flush_reorder` / `transfer_pages_between_books` / `rename_book`) が
  book_bookmarks.db の journal (`PreparedBookmarkMigration`、Prepared → Applying → filesystem_committed) の下で行う。
- path キー DB の付け替えは、UI が結果を受け取ったとき (`App::poll_book_op` 系、
  `apply_book_page_edit_moves_with_journal` / `apply_book_page_edit_copies`) に **UI スレッドで** 行う。
  - bookmark: service へ mapping と journal id を渡す (journal 行の削除と同じ transaction)。
  - 見開き調整 (spread_shift) と本の構成 (book_composition): 各 service の FIFO へ mapping を渡す。
  - コレクション: rename-migration FIFO の collection 段へ積む。
  - 上記以外: `move_book_page_edit_key` / `copy_book_page_edit_key` が App の DB handle で 8 種
    (補正・ローカル調整・切り取り・マスク・隠蔽・注釈・★・タグ) だけを 2 段 (一時キー経由) で動かす。
- 汎用の改名・削除が使う `rename_key_migration::STORES` (path キー DB の正本の表) と比べ、次が付け替えられない:
  表示トリム (`view_trim_pages`、本の改名時は `view_trim_books` も)、内容同一性 (`edit_origin`)、
  編集プレビュー (`edit_previews`)、回転、本の改名時の本単位キー (読書位置 `book_resume`、見開き/表紙 `spread.db`、
  フォルダサムネイル pin の `container_key`、閲覧履歴、sidecar 同期印)。
  値の中にページを持つもの (読書位置の `page_identity`、pin の `source_rel`) も追従しない。
- 本のページ名は `NNNN_元名`。元名が同じページ (別々の ZIP の `001.jpg` など) は並べ替えで
  互いの旧名を新名として受け取るので、旧キーに残った行が**別のページに付く**。

### 1.2 本の削除 (§1.273)

- `books::delete_book` は `remove_dir_all` するだけ。UI は spread_shift / book_composition の service に
  `purge_book_root` を頼み、rename-migration FIFO の汎用段を止める (`invalidate_rename_migrations_for_removed_paths`)
  が、`STORES` の purge は呼ばない。同名の本を作り直すと旧ページの補正・タグ等が新しいページに付く。
- 同じ製本領域の「追加の取り消し」(`book_append_undo::undo_append_with`) は、worker で
  `acquire_delete_epub_guard` → ごみ箱 → `purge_removed_scopes_guarded_at` を行い、UI で
  `apply_content_identity_store_mutations` と `purge_in_memory_metadata_for_removed_paths` を行う。これが手本。

### 1.3 しおりの番号 (§1.245)

- 保存時の `page_index_hint` は `canonical_raw_book_page(idx)` = `items` 上の番号
  (本の構成の射影を外した生の番号)。ZIP の一覧ではサブフォルダの行が画像より前に並ぶので、ずれる。
- 表示 (`bookmark_browser.rs` の `position_label` / `badge_label`)、本の中のしおりの並び順
  (`ORDER BY page_index_hint`)、mIV Remote の進捗 (`remote_ipc/collections.rs` の `progress_current`) がこの値を使う。
  ジャンプ先は `PageIdentity` が正本で、hint は使わない。
- 製本 (compiled book) では移行時に `compiled_page_index` (= ファイル番号 - 1) で hint を作り直している。
  本のページは画像だけなので、この値は読み順の位置と一致する。

## 2. 守る不変条件

- I1. 本の構造変更 (並べ替え・別本への移動/コピー・本の改名) の後、path キーの中央 DB に保存された情報は、
  変更前と同じ**画像**に付いている。対象ストアは `rename_key_migration::STORES` の 1 表だけで決める。
  値の中にページを指す情報 (読書位置の identity、フォルダサムネイル pin、編集プレビューのファイル配置) も同じ画像を指す。
- I2. 本の削除の後、その本のフォルダ配下をキーにした行は `STORES` から消えている
  (汎用削除と同じ purge。本ページのしおりは missing 行として残す既存仕様のまま)。
- I3. 1 つのストアの付け替えは、1 回の操作につき 1 つの所有者だけが行う
  (spread_shift・book_composition は従来どおり各 service、bookmark は journal 経路、collections は collection 段、
  編集プレビューはそのモジュール、それ以外の `STORES` は製本 worker)。同じストアを 2 か所から動かさない。
- I4. 付け替え・purge の DB 書き込みは UI スレッドで行わない (cold open を含む)。
- I5. 構造変更の FS 処理を始める時点で、本のページ宛ての書き込み・記録の producer (保存の worker、
  UI が未消費の完了、完了から発行される後続の記録) はすべて静止しており、構造変更の結果を UI が採用するまで
  新しい producer は始まらない。入口は本体の UI・別ウィンドウ・mIV Remote を含む。
- I6. 置換 (A→B と B→A の入れ替えを含む) は、ストアごとに 1 つの transaction の中で一時キーを経由して行い、
  途中の状態を他から見せない。
- I7. 新しく保存したしおりの「P.N」は、保存した時点で、そのページをフルスクリーンで開いたときのページ表示
  (「N / 全体」) と同じ番号になる。古い行と、保存後の本の構成変更での一致は保証しない (§5)。

## 3. 設計 (§1.272 / §1.273)

### 3.1 構造操作の単一の所有者 (I5)

本の構造操作 (並べ替えの保存、別本への移動/コピー、本の改名、本の削除) は、App が持つ 1 つの型付き状態
`BookStructuralOp` が受付から完了の採用までを所有する。今の `book_op_pending` (Delete/SourceMutation の intent) と
並べ替え画面の `flush_pending` / `transfer_pending` は、構造操作についてはこの所有者へまとめる
(本の作成・一覧・追加・追加の取り消しは今のままでよい。追加の取り消しを同じ所有者へ寄せるかは実装担当が判断して報告)。

```text
Requested(要求) ──静止を確認──> Running(worker) ──結果を UI が採用──> (なし)
```

- **Requested**: 操作の内容を保持し、毎フレーム「書き込みの静止」を確認する。静止したら worker を起動して Running へ。
  この間も Running の間も、次の入口は受け付けない (理由付きで拒否するか待たせる):
  - 本体 UI の編集・タグ・★・表示トリム・保存・本を開く操作 (製本管理の「開く」を含む)。共通モーダル判定
    (`modal_dialog_block_reason`) に入れ、さらに製本管理・並べ替え画面の中の操作も所有者を見て止める。
  - mIV Remote: 受付時に `SessionHandle::try_local_modal_lease()` を取り、採用まで持つ。取れなければ
    構造操作を始めない (利用者へ理由を出す)。Remote の取得待ち (`book_op_pending` を見ている箇所) も所有者を見る。
  - 別ウィンドウ (detached viewer) の書き込み。所有者の確認は App 共通の新規入力の受付入口に置き、detached の
    経路を個別に足さない (detached リワーク中の規則に触れる変更が要るなら、実装前に報告する)。
- **静止の判定**は 1 つの述語にまとめる。少なくとも汎用改名の `rename_migration_writers_busy()` が見る writer
  (タグの未消費結果、★、編集プレビュー、読書位置、ローカル調整、bookmark の未消費要求、spread_shift service) に加え、
  `reading_history_writer`、`content_identity_recorder` (タグ完了の消費から発行される記録を含む)、LUT・segmentation の
  完了から発行される保存、他の viewer context (parked を含む) の producer。実装担当が producer を全部列挙し、
  列挙の根拠をこの文書の §8 に残す。汎用改名の開始条件も同じ述語を使えるなら置き換える (差があれば理由を書く)。
- 静止を待つのは UI のフレームでの確認だけで、待機ループ・sleep・柵の待ち合わせは使わない
  (CLAUDE.md「並行処理」)。新しい producer は入口で止まっているので、静止は短時間で来る。
- 長く静止しない場合の上限は設けない (書き込みが終わらないのは別の不具合)。Requested の間は進捗表示を出す。
- **止めるのは「新しい入力の受付」だけ**で、受付済みの処理の完了とそこから続く保存・記録は最後まで通す。
  たとえば LUT・segmentation の完了処理は pending を取り出して通常の保存関数へ進む
  (`ui_adjustment_panel.rs` の LUT 完了 → `page_edit_owner.rs` の保存)。所有者の確認を保存関数や共通の書き込み関数に
  置くと、受理済みの結果が保存されずに pending だけ消えて「静止」してしまう。所有者の確認は利用者の入力を受け付ける
  入口 (UI の操作・キー・メニュー、Remote の lease) に置き、保存関数には置かない。受理済みの完了と後続の保存・記録が
  終わったことを Running へ移る条件 (静止の判定) に含める。
- **他の変更操作との順序**: 受付の時点で、path キーの DB や本のファイルを後から変える他の操作が残っていれば、
  構造操作を**受け付けない** (理由を出し、利用者が後でやり直す)。対象: 汎用改名キューの汎用段が待機中または実行中
  (collection 段だけのジョブは collection actor の FIFO で順序が保たれるので対象外)、削除・purge の再試行・
  メタデータ整理の実行中、追加 (Append) と追加の取り消し (UndoAppend) の実行中。待たせる形にすると、汎用改名側が
  「本の構造操作の実行中は始めない」(`rename_migration_source_changes_pending`) ので互いを待って止まる。
  受付後は、汎用改名などの新しい操作は今と同じく構造操作の終了を待つ。

### 3.2 対象表に「本の操作での扱い」を持たせる (I1・I3)

`StoreDescriptor` に本の操作での方針を足し、`STORES` の各行が自分の扱いを宣言する。
新しいストアを足す人はこの列を決めないと表を書けない (本の操作だけ別表になる状態を作らない)。

| 方針 | 対象 | 並べ替え・移動・改名 | 別本へのコピー | 削除 |
| --- | --- | --- | --- | --- |
| `Follow` (既定) | 下記以外のすべての `rename_generic` 行 | 製本 worker が mapping を適用 | コピー列が真なら写す、偽ならコピー先の行を消す | purge |
| `OwnedBy(EditPreview)` | `edit_preview_cache.db` | 編集プレビューのモジュールの移行関数 (§3.4) | コピー先の行とファイルを消す (同じモジュール) | purge (既存の削除と同じ扱い) |
| `ServiceOwned` | `book_composition.db`、`spread_shift.db` | 各 service が FIFO で適用 (§3.3 の規則を要求で渡す) | なし | purge (worker) |
| (表外) | book_bookmarks.db | journal 経路 (従来どおり) | なし | 残す (missing 行) |

- 別本へのコピーでは「写す」と「コピー先を初期化する」を分ける。コピー先のページ名は今あるファイルから決まる
  (`next_page_number`) ので、削除済みページと同じキーの再利用は通常起こる。写さないストアにコピー先の古い行が
  残っていると新しい画像に付くので、写さない `Follow` ストアもコピー先のキー (3 面) の行は消す。本単位の設定
  (本フォルダのキー) はページのキーに一致しないので消えない。
- 別本へのコピーで複製する対象は**今と同じ 8 種** (補正・ローカル調整・切り取り・マスク・隠蔽・注釈・★・タグ
  [`item_tags` と `tag_item_state`]) にとどめ、同じ表の列 (`copies_with_book_page: bool`) で宣言する。
- `reading_history` (`rename_generic=false`) は本の改名のときだけ、汎用改名と同じ exact 移行を行う。
- mapping の適用は汎用改名と同じ 3 面 (exact、`key/` 配下、`key::` 仮想ページ)。本単位のキー (本フォルダ) は
  ページの mapping には一致しないので、並べ替えでは動かず、本の改名 (フォルダ→フォルダ) でだけ動く。
- `ratings.source_path` は既存の `migrate_store` が更新している。本の操作でも同じ更新を通す。

### 3.3 衝突の規則: 移行先は移行元の写し (I1)

汎用改名は「移行先に行があれば移行先を優先」だが、本の操作では採らない。本の操作の移行先 (新しいページ名・新しい本名)
には、今まで付け替えられなかった行や、削除した同名の本の行が**残っていることが通常の操作で起こる** (§1.272・§1.273 の既存不具合)。

- 本の操作の移動・コピーは、キーごとに「移行先の行を消してから、移行元の行を移す/写す」。移行元に行が無ければ
  移行先の行も消える。結果として、移行先のキーの行は移行元のキーの行と同じになる。
- 既存のコピー (補正は移行元で上書き、タグは移行先を消して置換) と同じ意味で、移動もこれに揃える。
- 本の改名 (フォルダ→フォルダ) も同じ: 新しい本フォルダ配下に残っていた行は消える。
- この規則は本の操作で付け替える**すべての所有者**に渡す: 製本 worker の `Follow` ストア、編集プレビューの移行関数、
  spread_shift と book_composition の service (今は本の改名でも `INSERT OR IGNORE` で移行先優先、
  `book_composition/store.rs`・`spread_shift/store.rs`)、閲覧履歴の専用処理 (`migrate_reading_history`)。各所有者は
  規則を引数 (要求の中身) で受け取り、自分の transaction の中で適用する。worker が service の DB を別に消して
  合わせることはしない (I3)。汎用改名・内容同一性の復元は従来どおり移行先優先のまま (両方の規則をテストする)。
- bookmark と collections はこの一律の置換の**対象外**。bookmark は missing 行を残す既存仕様と journal 経路、
  collections は参照の付け替え (collection actor の既存の規則) のまま。

### 3.4 値の中にページを持つもの (I1)

同じ worker の、そのストアの transaction の中で値も書き換える。

- `book_resume.page_identity` が `Image { path }` で、その path が mapping の旧側なら新側へ。本の改名では配下 prefix を
  付け替える。`page` 列は更新しない (`page` の更新で identity を消すトリガーがあるため)。
- `folder_thumb_pins` (`container_key` が本フォルダ):
  - `source_rel` が同じ本の中で名前の変わったページなら、新しいページ名へ。
  - `source_rel` が**別の本へ移したページ**なら、その pin を外す (行を消す)。残すと、移動後の詰め直しで同じ名前に
    なった別の画像を正常に指してしまう (同じ元名のページで通常の操作で起こる)。外した本は自動サムネイルに戻る。
    汎用の移動・削除で pin の画像が無くなったときに見える結果 (自動サムネイル) と同じ。
- 編集プレビュー (`edit_previews`): 保存先ディレクトリが `item_key` のハッシュで決まり、`cached_path` と
  `annotation_layers_json` にそのパスが入る (`edit_preview_cache.rs` の `cache_dir_for_item`)。キーだけ動かすと
  後の復元コピーや削除が「所有していないファイル」として扱う。編集プレビューのモジュールに移行関数を置き、
  ファイルを新しいキーのディレクトリへ移し、`cached_path` と layer のパスも書き換える (置換は一時キー経由)。
  実行は製本 worker から呼ぶ (静止しているので編集プレビューの worker と競合しない)。汎用改名の同じ問題は
  今回の範囲外として §8 に記録する。

### 3.10 実装中に決めた 2 点 (2026-10-21、設計担当)

実装担当が §8.4.1 で返した前提の不一致への決定。

- **別本へ移したページを指す本の構成・見開き調整の参照**: 移動元の本の `book_composition` (anchor・hidden・
  override・order) と `spread_shift` に残る、別本へ移したページへの相対参照は、各 service が本の操作の要求の中で
  **外す** (pin と同じ扱い。§3.4)。残すと詰め直しで同じ名前になった別の画像を指す。移動先の本へは移さない
  (移動先には移動先の本の構成がある)。外した参照は戻らないので、**利用者が作ったデータ (本の構成) の変更**として
  master への取り込み前に利用者へ確認する (CLAUDE.md「まれな失敗は既定で割り切る」の相談対象 1)。
- **別ウィンドウ (F12) と Snapshot**: リリース済みの挙動 (`spread-shift-memory-plan.md` §11.6: 本の改名後も
  F12 は旧 path・旧 items を保つ) を保つ。§3.5 の失効・再読込は、既存の共通 reload / refresh で作り直される
  context だけに適用し、別ウィンドウ・Snapshot の context の cache は**触らない** (cache だけを新しい DB に
  合わせると、旧い画素と別画像の編集情報が混ざる)。その窓は利用者が閉じて開き直すまで変更前の状態を表示する。
  変更前の 8 種の付け替え (UI スレッド) でも別ウィンドウは同じ状態になっていたので、退行ではない。detached の
  述語・viewport には触れない。再読込の設計は detached リワーク側の範囲として §8 に記録する。

### 3.5 実行場所: 製本 worker で FS の直後に行う

```text
[UI] Requested: 受付 (所有者・Remote lease) → 静止を待つ
[UI] Running へ: worker 起動
[worker] FS (既存の journal: Prepared → Applying → filesystem_committed)
[worker] store maintenance lease (EPUB 範囲・spread_shift・book_composition) を取る
[worker] STORES の付け替え (ストアごとに 1 transaction、一時キー 2 段、§3.3 の規則) / コピー / purge、
         値の書き換え (§3.4)、編集プレビューの移行
[UI] 採用: bookmark・spread_shift・book_composition・collections は従来どおり。
         App の記憶内の状態 (presence 集合、★/タグ cache、編集プレビュー、表示トリム、読書位置メーター、
         内容同一性の索引、pin の cache) を DB を読まずに失効・付け替え → 再読込 → 所有者を解放
```

- 新しい公開関数は `rename_key_migration` に置く (例: `migrate_book_paths_at(data_dir, moves, copies, guard)`)。
  汎用改名の `migrate_store` の 3 面の SQL と一時キーの考え方を流用し、ストアごとの処理を 2 か所に書かない
  (ただし衝突の規則は §3.3 なので、規則を引数で選べる形にする)。
- ストアごとの失敗は集計して UI に返し、ログ + 警告トーストにする (FS は確定済みなので巻き戻さない。
  まれな失敗の既定の割り切り)。
- UI 側の 8 種の付け替え (`move_book_page_edit_key` / `copy_book_page_edit_key` と `wait_for_local_adjust_writes`)
  は削除する。記憶内の presence 集合は、汎用改名の完了処理 (`rewrite_page_edit_presence_for_rename` 等) と同じ
  失効・再読込の経路に揃えるか、I/O の無い付け替えを残すかを実装担当がコードで判断して報告する。

### 3.6 本の改名の bookmark journal (I1)

今の `rename_book` は現存ページを列挙した mapping を journal に保存するので、既に missing のページのしおりが
旧本名に残る。改名は Prepared の作成時から **本フォルダ→本フォルダ** の mapping を journal に保存し、bookmark の
付け替えは既存の journal 経路のまま (通常完了・起動時復旧の両方) とする。STORES の付け替えも同じフォルダ mapping で行う。

### 3.7 途中で終了した場合 (まれな失敗の割り切り)

FS の確定と `STORES` の付け替えの間で mIV が落ちると、付け替えが途中になり得る (今も同じ)。ストアごとの
完了印や再実行の仕組みは**作らない** (置換は再実行すると逆に戻るため、安全な再実行には完了印が要る。
CLAUDE.md「まれな失敗は既定で割り切る」)。

- 起動時の book_bookmarks journal 復旧が Applying または filesystem_committed の行を処理したら、ログに残し、
  利用者へ 1 回だけ通知する:「前回、本の並べ替え・移動・名前変更の途中で mIV が終了しました。一部のページの
  補正・タグなどが正しいページに付いていない可能性があります。」
- `recover_path_migration_journal` は結果を返さないので、処理した件数を UI へ返す経路を足す。

### 3.8 本の削除 (§1.273)

- `books::delete_book` を `undo_append_with` と同じ形にする: `acquire_delete_epub_guard(Tree(本フォルダ))`
  → `remove_dir_all` (既存どおり完全削除) → 成功したら `purge_removed_scopes_guarded_at(Tree(本フォルダ))`。
  結果に `store_mutations` と purge のエラーを載せる。静止と所有者は §3.1 と同じ。
- UI は `apply_content_identity_store_mutations`、`purge_in_memory_metadata_for_removed_paths` を加える。
  spread_shift / book_composition の `purge_book_root` は、maintenance lease の解放時の再読込と重複するなら省く
  (汎用削除の完了処理に揃える。実装担当が判断して報告)。
- PDF パスワードは本に PDF が入らないので空で渡す。sidecar は本フォルダごと消えるので対象外。

### 3.9 永続データ (CLAUDE.md「永続データ・スキーマ変更時の判断」)

- スキーマ変更なし。新しい表・列・journal の段階名を足さない。旧版で開いても挙動は変わらない。
- 本の改名の journal に保存する mapping の形 (ページ列挙 → フォルダ 1 組) は、journal の既存の JSON の形のまま
  (要素が 1 つになるだけ)。旧版の復旧も祖先パスの照合でフォルダ mapping を解釈できるので、**しおりの移動先
  (identity) は旧版でも同じ**になる。hint は旧版の挙動 (`compiled_page_index` から再計算) になり、新版の
  「改名では保持」とは欠番のある本で異なり得る (表示の数字だけ。回復の仕組みは足さない)。
  互換テストは両方向: 旧形式 (ページ列挙) の journal を新版が復旧する、新形式 (フォルダ 1 組) を旧処理
  (祖先照合) で解釈する。
- 既存の DB に残っている「旧キーの付け残し」は、どの画像の行だったかを後から決められないので直さない。
  ただし §3.3 により、今後の本の操作の移行先では消える。

## 4. テスト (実装担当)

- `rename_key_migration`: 方針列の網羅 (全 `STORES` 行が方針を持ち、`ServiceOwned` と `OwnedBy` が想定のものだけ)。
  入れ替え (A↔B) を含む mapping で全 `Follow` ストアが正しく入れ替わり一時キーが残らない。移行先に古い行がある
  場合 (消える) と、移行元に行が無い場合 (移行先も消える)。`ratings.source_path` の更新。
- 値の書き換え: `book_resume.page_identity` (並べ替え、改名の prefix、`page` を変えない)、pin の `source_rel`
  (同じ本の並べ替えで追従、「同じ元名の 2 ページで先頭を pin して別本へ移動」で pin が外れる)、編集プレビュー
  (A↔B 後の再保存・片側削除・内容同一性の復元コピーが正しく動く)。
- 製本 worker の handler-level テスト: 並べ替え・移動・改名で表示トリムと内容同一性が新しい名前に付き、
  同名ページの入れ替えでも別ページへ付かない。削除で `STORES` の行が消え、本ページのしおりは残る。同じ名前で
  本を作り直しても旧ページの補正・タグ・表示トリムが付かない。missing 行を含む本の改名で、しおりが新しい本名へ
  移る (通常完了と journal 復旧の両方)。
- 受付の拒否: 汎用改名 A→B が汎用段の待機中に本 B→C を要求すると拒否される。削除・追加・追加の取り消しの
  実行中も同じ。受付後は汎用改名が構造操作の終了を待つ。
- 受付済みの完了: Requested になった後に LUT・segmentation の完了が届き、その保存と後続の記録が終わってから
  FS が始まる (pending だけが消えて保存されない、が起きない)。
- コピー先の初期化: 写さないストア (表示トリム・回転・内容同一性・編集プレビュー) にコピー先の古い行を置いて
  別本へコピーすると、その行が消える。service 所有ストアと閲覧履歴も、本の操作では移行元の写しになり、
  汎用改名では従来どおり移行先優先。
- 所有者と静止: タグ結果の未消費、内容同一性の記録の遅延、閲覧履歴の書き込みの遅延、parked context の producer が
  ある間は worker が起動しない。Requested/Running の間、本体 UI の編集・製本管理の「開く」・Remote の書き込みと取得が
  止まる。Remote の lease が取れないときは構造操作が始まらない。
- 起動時の journal 復旧が Applying / filesystem_committed の行を処理したときに通知が 1 回出る。

## 5. しおりのページ番号 (§1.245)

- 保存時の `page_index_hint` を「保存した時点の、その本の読み順でのページの位置 (0 始まり)」にする。数え方は
  フルスクリーンのページ表示 (HUD) が使う `image_reading_position()` (`ui_fullscreen.rs`) と同じものを使い、2 か所に書かない
  (レビューで確認済みの候補。`ui_helpers.rs` の `still_image_display_indices(items, current_reader_order())` と組で使う)。
- mIV Remote がしおりを作る経路があれば、本体の現在一覧ではなく Remote 側で検証した一覧から同じ関数で求める。
- 製本の bookmark 移行時の再計算 (`compiled_page_index` = ファイル番号 - 1): 本の改名 (フォルダ→フォルダ) では
  読み順が変わらないので hint を**変えない**。並べ替え・移動では従来どおり再計算する (並べ替えの保存は 0001 から
  詰め直すので番号 - 1 が位置と一致する。移動先の本に外部削除の欠番がある場合はずれ得るが、表示の数字だけなので許容)。
- 既に保存済みの行の hint は直さない (一覧表示のたびに本ごとの一覧を作り直すのは重い)。同じ本で古い行と新しい行が
  混ざると、本の中のしおりの並び順が一部ずれ得るが、表示の数字だけの P3 として許容する。
- テスト: サブフォルダ 2 つ + 画像 3 枚の ZIP の先頭画像で作った draft の hint が 0、PDF は従来どおり、
  欠番のある製本で「しおり作成 → 本の改名」で hint が変わらない。

## 6. 採らなかった案

- **rename-migration FIFO (汎用改名の journal つきキュー) に一括ジョブとして積む**: 置換の一括ジョブは、途中で
  落ちた後に journal で再実行すると逆に戻るので、「少なくとも 1 回」実行する今の仕組みと合わない。
- **R2 の関所・受領証・ストアごとの完了印**: 途中終了からの完全な回復ができるが、仕組みが大きく、まれな失敗の
  ための回復処理は割り切る方針 (CLAUDE.md) に反する。
- **UI スレッドで App の DB handle を使って `STORES` 全部を動かす**: App が handle を持たないストアの cold open が
  UI スレッドで起きる。
- **writer ごとの柵を worker で待つ (v1 案)**: 内容同一性の記録は実行時にファイルを読むので、FS の後に旧キー宛ての
  記録が走ると別の画像の identity を作る。DB の直前に待つのでは遅い。また柵 API の無い writer が多い。
  受付から FS の前までに静止させ、新しい producer を入口で止めるほうが状態が少ない。
- **別本へ移したページの pin を変えない (v1 案)**: 詰め直しで同じ名前になった別の画像を指す (レビュー指摘)。

## 7. レビュー記録

- **実装前レビュー 1 回目** (2026-10-21、Codex gpt-6.1-sol / xhigh、read-only、SID 01a15e17-5946-7da1-b841-9edd8b1c3e29、
  対象 v1 = 004fc9928)。結論「P1・P2 を反映するまで実装に進めない」。対応:
  - P1 柵の対象と位置の不足 (内容同一性の記録がファイルを読む、閲覧履歴、LUT・segmentation) → §3.1 の静止の判定を
    FS の前に移し、producer の列挙を実装担当の作業にした。
  - P1 モーダルだけでは全入口を止められない (Remote、製本管理の「開く」、並べ替え側の pending) → §3.1 の単一の所有者と
    Remote の `try_local_modal_lease`。
  - P2 編集プレビューのファイル配置 → §3.4 の移行関数。
  - P2 別本へ移したページの pin → §3.4 で外す。
  - P2 本の改名の journal mapping → §3.6。
  - P2 移行先に既存行がある場合の規則 → §3.3。
  - P3 `compiled_page_index` と読み順 → §5 (改名では hint を変えない、I7 を保存時点に限定)。
  - 指摘なしの観点: I3 の分担、I2・I4・I6 の基本方針、§5 の共用関数、古い hint を直さない判断、§6。
- **実装前レビュー 2 回目** (同じ SID、対象 v2 = a4cde6e1e)。前回の指摘は設計として解消。P1 なし。対応:
  - P2 既存の改名・削除・追加との開始順序 (互いに待つと止まる) → §3.1 で受付を拒否する。
  - P2 受付済み処理の完了からの保存 → §3.1 で「新しい入力の受付」だけを止め、保存関数には所有者の確認を置かない。
  - P2 コピーしないストアのコピー先の古い行 → §3.2 で「写す」と「初期化」を分ける。
  - P2 移行元優先が service 所有ストア・閲覧履歴に届かない → §3.3 で規則を各所有者へ渡す。bookmark・collections は対象外。
  - P3 journal 互換の記述 → §3.9 で identity と hint を分け、両方向の互換テスト。
- **実装前レビュー 3 回目** (同じ SID、対象 v3 = d344d6270)。2 回目の指摘はすべて解消、P1・P2 なし。
  結論「この設計で実装に進めてよい」。P3 の文言 2 点 (受付入口の表現、`image_reading_position()` の所在) を訂正した。

## 8. 実装記録

### 8.1 STORES と製本 worker の移行 (2026-10-21)

`src/rename_key_migration.rs` の `StoreDescriptor` に `book_policy` と
`copies_with_book_page` を追加した。全行の constructor で両方を指定するので、ストア追加時に
本操作の扱いも決める。`Follow` の別表は作らず、汎用改名・内容同一性復元・削除と同じ `STORES` を使う。

- `Follow`: `books::maintain_book_paths` が FS 確定後に保守 lease
  (EPUB 範囲 → spread_shift → book_composition) を取り、
  `migrate_book_paths_at(data_dir, moves, copies, guard)` へ渡す。新しい DB 接続・SQL は worker 上だけ。
  全移行元の 3 面を一時キーへ退避し、移行先の 3 面を消してから戻す。移行元に行がなくても
  移行先を空にする。値の更新・移動・コピーを descriptor ごとの一つの transaction に含める。
  汎用改名も共通の 3 面 SQL helper を通すが、`PathMigrationCollision::DestinationWins` を維持し、
  本操作には `SourceWins` を渡す。内容同一性の復元コピーの先勝も変えない。
- コピーする 8 種は補正・ローカル調整・切り取り・マスク・隠蔽・注釈・★・タグ。
  タグは `item_tags` と `tag_item_state` の 2 descriptor なので、コピー列が真の行は計 9 行。
  それ以外の `Follow` もコピー先の 3 面を削除する。ページ mapping と一致しない本単位キーは残る。
- 移動とコピーのある転送は、`moves` が操作前 → 最終名、`copies` が並べ替え確定名 → コピー先。
  同じ transaction 内で移動を終えてからコピー元を読む契約にした。これにより同名ページの入れ替え後も
  コピーする画像の情報を読める。ファイル名確定前のコピー元キーは worker API へ渡さない。
- `ServiceOwned` の spread_shift・book_composition はこの worker API から書かない。
  UI の完了採用から既存 FIFO へ送る。本の root 改名は `SourceWins` を要求に明示する。
  bookmark は journal、collections は既存の参照移行を保ち、この置換規則の対象にしない。
- `reading_history` は移動先が directory の本改名だけ exact 移行する (worker で実 FS を判定)。
  移行先の古い行を消し、正規化 key と raw path を同時更新する。配下履歴は既存どおり対象外。
- 本削除は FS 前から guarded purge の lease を持ち、削除成功後に同じ `STORES` を purge する。
  purge の失敗と `store_mutations` は `BookStoreSummary` に載せて完了採用へ返す。
  本ページの bookmark は表外のままで missing 行を残す。

### 8.2 値と編集プレビューの所有

`book_resume.page_identity` の Image path は操作前の mapping で一度だけ更新し、`page` は書かない。
legacy trigger による identity 消去を避け、folder 改名では配下 prefix も付け替える。
pin は元の `container_key/source_rel` から同じ画像の移行先を求め、同じ本なら相対名を更新する。
別本へ出た画像を指していた pin は削除し、元名が再利用された別画像を指させない。
`ratings.source_path` はキーの最終配置後に既存と同じ導出規則で更新する。

`OwnedByEditPreview` は `edit_preview_cache::migrate_book_paths_at` が所有する。
既存 DB を schema reset なしで開き、base/layer が元キーの自所有 hash ディレクトリ内にあることを
既存の復元コピー用 layout 検査で確かめる。行とファイルを一時キーの hash 配置へ退避し、移行先を
初期化してから最終キーの配置へ移す。`cached_path` と layer JSON の各 path も同じ transaction 内で更新する。
本ページのコピーでは preview の行・自所有ファイルをコピー先から消す。
**汎用改名の preview ファイル配置問題は今回の範囲外**。汎用改名は従来の key 移行を保つ。

### 8.3 失敗・永続形式・互換

FS 確定後のストア単位の失敗は report に集計し、完了採用からログ・警告通知へ返す。
まれな失敗は既定の割り切りとし、FS の巻き戻し、ストア完了印、移行の自動再実行を足さない。
入れ替え mapping は再実行すると情報を逆へ戻し得るため、汎用改名 journal に本の置換処理は積まない。

スキーマ変更なし。新しい DB・表・列・journal phase は作っていない。
本の改名は `PreparedBookmarkMigration` の作成時から folder 1 組の mapping を記録するので、
現存ページだけでなく missing bookmark も通常完了・起動時復旧の対象になる。
journal の JSON 形式は既存のままで、旧形式のページ列挙を新版で復旧するテストと、folder mapping を
旧来の祖先照合で解けるテストを追加した。改名の hint 保持と旧処理の hint 再計算の差は §3.9 のまま。

### 8.4 BookStructuralOp・producer・Remote の受付

構造操作は `app/book_structural_op.rs` の `BookStructuralOp` が Requested / Running を所有する。
管理画面の改名・削除と、並べ替え画面の保存・転送を同じ受付へ通し、旧 `flush_pending` /
`transfer_pending` は削除した。追加の取り消しは既存 Append receipt・refresh の所有者に残す。
UndoAppend 実行中は構造要求を拒否し、構造操作中は共通入力受付と既存 Undo の busy 判定が拒否するため、
receipt の完了採用までを別の所有者へ移す必要はない。

受付では generic 段の待機 / 実行 / boot retry、削除・purge retry・整理・メタデータ転送、
Append / UndoAppend を含む既存 BookOp、改名・コピー・取得・変換を拒否する。
collection-only は拒否せず既存 actor FIFO を使う。編集中の pointer / slider / 注釈 draft、
原画取得からの操作継続 (FS pending・upload・inpaint・local materialization)、未採用の open / 列挙も
既存操作の終了後に再要求する形にした。modal によって受理済み操作が捨てられるのを避け、
UI に新しい同期保存の呼出を足さないためである。

producer の棚卸しは次の所有者と、保存を生む結果の消費箇所を照合した。

| producer / 所有者 | 静止を確認する根拠 |
| --- | --- |
| タグ要求・結果 (`tag_ops`) | `has_unconsumed_batch`。DB 終了だけでなく UI が結果から発行する sidecar・内容同一性記録まで通す |
| ★の外部書込 (`rating_write_handle`) | `is_busy`。UI 同期の中央 DB 変更は入力 handler 終了時に確定している。session 台帳も mapping の snapshot から置換する |
| XMP rating prewarm (viewer context 所有) | 既存 `viewer_context_ids` / `with_owner_viewer_context` で全 context の `TagPrewarmPending::is_busy` を確認。Requested は結果を回収し、新しい可視範囲 job の受付だけ止める |
| ローカル調整保存 | writer の `has_unfinished_work` と App-global の未消費 `local_adjust_write_pending` |
| LUT・segmentation | 既存 `has_page_edit_producers` / `poll_all_page_edit_producers` が mounted / parked を含める。完了から通常保存・後続記録まで通す |
| 編集プレビュー | `has_pending_commands` が save / delete FIFO を含む |
| 本のしおり | `book_bookmark_pending_requests` と `remote_bookmark_writes_pending`。保存要求から結果採用まで |
| 本 resume / 閲覧履歴 | 各 writer の `is_busy`。履歴は record / prune / media progress / remove / FolderZip portal の全要求を同じ counter で追跡 |
| 内容同一性 | recorder clone を含む `is_busy`、detection / backfill の未消費結果。新規の自動検出だけ構造 owner 中は開始しない |
| 見開き調整 / 本の構成 | 両 service の `is_busy` と構成 editor の受理済 save ticket。構成 service が発行する recorder はその下流として最後に確認する |
| 編集 bundle / stamp | paste / apply / bulk と stamp embed の既存 pending。新入力を止めた後も既存結果を消費する |
| 補正・crop・mask・conceal・注釈・回転・pin | 同期 handler または上記の専用 writer から保存。新入力は共通 modal / grid / raw canvas / panel の入口で止め、保存関数には構造 owner の拒否を入れない |
| 表示トリムの未保存値 | 全 context から既存 `PendingViewTrimTransfer` を取り、同じ構造 worker の FS 前に保存。保存完了までは元の dirty payload を保持し、FS 前の失敗では元 context に戻す |

静止述語は汎用改名と構造操作で `rename_migration_writers_busy` を共有した。
上流 service・未消費結果を先に確認し、後続の内容同一性 recorder を最後に確認する。
UI フレームでのみ進め、待機ループ・sleep・新しい fence は追加していない。

Remote は受付時に `SessionHandle::try_local_modal_lease()` を取り、完了の UI 採用まで保持する。
取得できなければ構造要求を拒否する。Remote の取得待ちに使う既存 AI barrier の book pending 射影も
新所有者を含めた。書込要求の共通受付にも防御を置き、既存完了処理には拒否を置かない。
新しいしおりを作る production 経路は PC の `current_book_bookmark_draft_with_image_classification` と
Remote の `remote_bookmark_draft` の二つで、他の `NewBookBookmark` 構築はテストである。
PC / 別窓は前者、Remote は検証済みの自身の一覧から共用 `ui_helpers::image_reading_position` を使う。
Remote の resume 用 raw index の意味は変えず、しおり要求の hint だけ読み順へ変換した。

presence 集合は I/O の無い snapshot 置換を残した。全移行元を読む前に書き換えず、
A↔B と移行元欠損でも worker と同じ結果にする。コピーは従来の六つの編集 presence だけ写し、
回転のコピー先は初期化する。★ session 台帳も moves → copies の同じ規則で置換する。
対象 context のタグ・★・rotation・page-edit・preview・pin cache を失効し、既存の非同期準備へ戻す。
本棚の `Folder(本)` は page key を持たないため、ページの両端の親フォルダ (改名は両端の本 root) も
pin の依存先として確認する。親棚は既存の `CurrentViewRefresh::Pins` worker に対象だけ渡し、
本を開いていない棚や無関係な窓の page-edit・★・タグ状態は初期化しない。
presence はソート済みの対象範囲だけ snapshot し、全キー数 × mapping 数の UI 走査を避けた。
本の削除は worker が service maintenance lease 下で purge し、lease 解放時に再読込されるため、
UI の `purge_book_root` の重複要求は削除した。

### 8.4.1 設計担当へ返した前提の不一致 (未合意、2026-10-21)

- `spread_shift` / `book_composition` は scope 外へ出たページの相対参照を旧名のまま残す。
  同元名の別ページが詰め直しで旧名を再利用すると、anchor・hidden・override・order が別画像を指す。
  I1 と矛盾するが、§3.4 は pin の解除だけを規定し、構成値の除去 / 移送 / missing 保持は未定義。
  独立レビューでも確認し、この値の変更だけを保留して設計担当へ判断を求めた。
- 別窓 / Snapshot の画素・列は既存共通 reload / Operation refresh では即時再構築されない。
  cache だけを新 DB に合わせると旧画素と別画像の編集情報を併用し得る。
  一方で `spread-shift-memory-plan.md` §11.6 は本の改名後の F12 旧 path / 旧 items 保持を明示し、
  C1 も独立 reader へのその場再構築を規定しない。§3.5 の「再読込」の適用範囲と既存仕様の調整が必要。
  detached 述語 / viewport の症状パッチや新しい live rebuild は追加せず、既存終了経路で閉じるか、
  再読込設計を追補するかを設計担当へ確認した。

この二点が未合意の間は I1〜I7 全体を満たした実装完了とは扱わない。

### 8.5 検証状況

2026-10-21、すべての build / test に `MSBUILDDISABLENODEREUSE=1` を指定した。
native の並列ビルドが MSBuild で失敗したため、以後は `CARGO_BUILD_JOBS=1` も指定した。
製品のテスト失敗とは扱っていない。

- `cargo check -p mimageviewer --bin mimageviewer-core`: 成功。lib 264 件、ort 1 件の既存警告あり。
  今回不要になった service の purge 専用入口は test-only にし、新しい未使用警告を残さない。
  ログ: `target/book-final-check.stderr.log`。
- `cargo nextest run -p mimageviewer --lib -E $bookFilter` (下記): 41 件すべて成功。
  §8.5.1 の 39 名すべてが実行済みであることも JUnit と照合した (新規 37 + 改名 2)。
  初回は service のテスト fixture の scope 不一致が 1 件あり、source / destination とも chapter に揃えた。
  製品コードの変更は不要だった。最終ログ: `target/book-nextest.stderr.log`、
  JUnit: `target/nextest/default/junit.xml`。
- `scripts/test-focused.ps1 -Path <下表の13モジュールのソース>`: 600 件すべて成功、exit 0。
  shared な `app.rs` / `app/tests.rs` を含む全件 gate の代わりではない。
  ログ / JUnit は `target/test-focused/runs/20261021T120710865Z-3daad0d9` から
  `20261021T121221648Z-b3237bb0` までの今回の 13 run。
- `cargo fmt` (引数なし、workspace 全体)、`cargo fmt --check`、`git diff --check`: 成功。
- `python scripts/check_ui_glyphs.py`: 成功 (`no dangerous glyphs found in UI strings`)。
- `scripts/build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0`: 成功 (core 約 10 分 43 秒)。
  通常 feature の core / Remote / EPUB worker をビルドし、必要 DLL の配置と PE 検査も `runtime=4 pe=3` で成功。
  最初は既定の経路で別作業ツリーの native build を待った。active MSBuild が終了しても残留 node だけで
  待機が続いたため、自分の待機 script だけを止め、既存の待機スキップ指定で再実行した。他作業の process は止めていない。
  ログ: `target/book-dev-build.stdout.log` / `target/book-dev-build.stderr.log`。
  `target/dev-runtime` の既定 profile は通常の `%APPDATA%/mimageviewer`。本体起動・実機確認・commit は行っていない。
- `test-lib-full.ps1`: 指示どおり設計担当へ引き継ぐ。§8.4.1 の二点が未合意のため、実装完了・受入済みとはしない。

実装後の独立レビュー (`gpt-6.1-sol` / `xhigh`) は合意範囲を ACCEPT、追加 P1 / P2 なし。
上記の親担当の検証結果を参照し、重複実行していない。§8.4.1 の二点が未実装なので、
タスク全体の完了承認は保留である。41 件と focused 600 件には重複があり、641 個の異なるテストとは数えない。

| focused の選択モジュール | 成功件数 |
| --- | ---: |
| books | 56 |
| rename_key_migration | 61 |
| edit_preview_cache | 20 |
| book_bookmarks | 28 |
| book_composition | 69 |
| spread_shift | 60 |
| app::book_structural_op | 9 |
| app::page_edit_owner_tests | 26 |
| app::content_identity_restore | 11 |
| app::spread_shift_lifecycle_tests | 3 |
| remote_ipc::container | 173 |
| content_identity | 75 |
| reading_history_db | 9 |

実行した選択式と focused の明示パス:

```powershell
$env:MSBUILDDISABLENODEREUSE = '1'
$env:CARGO_BUILD_JOBS = '1'
$bookFilter = @(
    'test(/book_structure/)', 'test(/structural_worker/)',
    'test(/book_store_/)', 'test(/book_follow_stores/)',
    'test(/book_resume_identity/)', 'test(/book_history_replaces/)',
    'test(/book_preview_/)', 'test(/bookmark_draft/)', 'test(/zip_bookmark_hint/)',
    'test(/compiled_folder_rename/)', 'test(/folder_rename_journal/)',
    'test(/legacy_page_rename_journal/)', 'test(/source_priority/)',
    'test(/recorder_busy/)', 'test(/history_busy/)', 'test(/composition_busy/)',
    'test(/book_manager_delete_worker/)', 'test(/rename_book_reports_folder_mapping/)'
) -join ' | '
cargo nextest run -p mimageviewer --lib -E $bookFilter
.\scripts\test-focused.ps1 -Path @(
    'src/books.rs', 'src/rename_key_migration.rs', 'src/edit_preview_cache.rs',
    'src/book_bookmarks.rs', 'src/book_composition/mod.rs', 'src/spread_shift/mod.rs',
    'src/app/book_structural_op.rs', 'src/app/page_edit_owner_tests.rs',
    'src/app/content_identity_restore.rs', 'src/app/spread_shift_lifecycle_tests.rs',
    'src/remote_ipc/container.rs', 'src/content_identity.rs', 'src/reading_history_db.rs'
)
```

冒頭 §0 の実アプリ観測は設計担当 ClaudeCode の修正前記録であり、今回差分の動作確認には流用しない。

### 8.5.1 新規・改名したテスト名の一覧

新規 37 件と、所有者・journal の仕様更新に合わせて改名した既存 2 件。
このほか既存テストの fixture・受付・完了採用・隔離 data-dir を更新した。

`src/app/content_identity_restore.rs`

- `book_structure_requested_and_running_keep_restore_prompt_and_reject_new_restore`

`src/app/page_edit_owner_tests.rs`

- `book_structure_keeps_accepted_lut_and_segmentation_completion_saves`
- `book_structure_rejects_new_local_adjustment_tag_and_trim_input`
- `book_structure_requested_waits_for_mounted_and_parked_generated_edits_to_save`

`src/app/spread_shift_lifecycle_tests.rs`

- `book_manager_delete_worker_purges_same_name_and_adoption_preserves_sibling`

`src/app/tests.rs`

- `pdf_bookmark_draft_keeps_zero_based_reading_position`
- `zip_bookmark_draft_counts_images_only_in_current_reading_order`

`src/book_bookmarks.rs`

- `compiled_folder_rename_preserves_reading_hint_and_missing_identity`
- `folder_rename_journal_recovery_preserves_missing_pages_and_reports_once`
- `legacy_page_rename_journal_is_recovered_and_folder_mapping_matches_legacy_ancestor_lookup`

`src/book_composition/store_tests.rs`

- `composition_busy_includes_admitted_mutations_until_their_publication`
- `root_rename_source_priority_replaces_destination_family_and_absent_source_clears_it`

`src/books.rs`

- `rename_book_reports_folder_mapping_for_missing_bookmarks`

`src/content_identity.rs`

- `recorder_busy_includes_cloned_producers_and_coalesced_requests`

`src/edit_preview_cache.rs`

- `book_preview_clears_missing_source_destinations_and_copy_files`
- `book_preview_swap_preserves_owned_files_resave_delete_and_restore_copy`

`src/reading_history_db.rs`

- `history_busy_includes_queued_writes_and_folder_zip_portal`

`src/remote_ipc/container.rs`

- `zip_bookmark_hint_uses_verified_image_reading_order_with_subfolders`

`src/rename_key_migration.rs`

- `book_follow_stores_swap_all_faces_replace_stale_and_initialize_copies`
- `book_history_replaces_stale_destination_while_generic_rename_keeps_destination`
- `book_resume_identity_and_pin_values_follow_original_images`
- `book_store_policies_and_copy_kinds_are_explicit`

`src/spread_shift/service.rs`

- `root_rename_source_priority_is_carried_through_the_fifo_request`

`src/spread_shift/store.rs`

- `book_root_source_priority_replaces_destination_family_even_when_source_is_absent`

`src/app/book_structural_op.rs`

- `book_structure_moves_rating_ledger_and_invalidates_parked_context_caches`
- `book_structure_presence_swaps_and_clears_destinations_from_missing_sources`
- `book_structure_refreshes_parent_shelf_pins_without_resetting_unrelated_page_state`
- `book_structure_preserves_unsaved_trim_when_pre_fs_save_fails`
- `book_structure_rejects_append_and_undo_owners`
- `book_structure_rejects_generic_queue_source_delete_and_remote_lease_conflicts`
- `book_structure_requested_waits_then_running_retains_admission_until_adoption`
- `book_structure_waits_for_delayed_history_and_content_identity_writers`
- `book_structure_waits_for_unconsumed_tag_result_and_its_identity_record`

`src/books/structural_tests.rs`

- `structural_worker_copy_reads_reordered_source_and_clears_noncopy_stores`
- `structural_worker_delete_purges_metadata_but_preserves_missing_bookmark_on_recreation`
- `structural_worker_move_compacts_same_named_sibling_without_stealing_metadata`
- `structural_worker_rename_moves_folder_rows_and_journals_missing_bookmarks`
- `structural_worker_reorder_swaps_same_named_images_with_trim_and_identity`
- `structural_worker_reports_store_error_after_successful_filesystem_commit`

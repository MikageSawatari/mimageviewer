# 本の構造変更と path キー DB の付け替え・後始末 (§1.272 / §1.273) と、しおりのページ番号 (§1.245)

2026-10-21 設計担当 ClaudeCode Opus 5.5。状態: **設計 v1、実装前の独立レビュー待ち**。
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

- I1. 本の構造変更 (並べ替え・別本への移動・本の改名) の後、path キーの中央 DB に保存された情報は、
  変更前と同じ**画像**に付いている。対象ストアは `rename_key_migration::STORES` の 1 表だけで決める。
- I2. 本の削除の後、その本のフォルダ配下をキーにした行は `STORES` から消えている
  (汎用削除と同じ purge。本ページのしおりは missing 行として残す既存仕様のまま)。
- I3. 1 つのストアの付け替えは、1 回の操作につき 1 つの所有者だけが行う
  (spread_shift・book_composition は従来どおり各 service、bookmark は journal 経路、collections は collection 段、
  それ以外の `STORES` は製本 worker)。同じストアを 2 か所から動かさない。
- I4. 付け替え・purge の DB 書き込みは UI スレッドで行わない (cold open を含む)。
- I5. 付け替え・purge より前に積まれた、そのページ宛ての非同期書き込みは、付け替え・purge より先に着地する。
  構造変更の実行中に、そのページ宛ての新しい書き込みは始まらない。
- I6. 置換 (A→B と B→A の入れ替えを含む) は、ストアごとに 1 つの transaction の中で一時キーを経由して行い、
  途中の状態を他から見せない。
- I7. しおりの「P.N」は、そのページをフルスクリーンで開いたときのページ表示 (「N / 全体」) と同じ番号になる
  (本の構成で非表示にしたページを数えない射影の扱いは §5 で決める)。

## 3. 設計 (§1.272 / §1.273)

### 3.1 対象表に「本の操作での扱い」を持たせる

`StoreDescriptor` に本の操作での方針を 1 列足し、`STORES` の各行が自分の扱いを宣言する。
新しいストアを足す人は、この列を決めないと表を書けない (= 本の操作だけ別表になる状態を作らない)。

| 方針 | 対象 | 並べ替え・移動・改名 | 別本へのコピー | 削除 |
| --- | --- | --- | --- | --- |
| `Follow` (既定) | 下記以外のすべての `rename_generic` 行 | 製本 worker が mapping を適用 | 下の「コピー」列 | purge |
| `ServiceOwned` | `book_composition.db`、`spread_shift.db` | 各 service が FIFO で適用 (従来どおり) | なし | purge (worker) + service の `purge_book_root` (従来どおり) |
| (表外) | book_bookmarks.db | journal 経路 (従来どおり) | なし | 残す (missing 行) |

- 別本へのコピーで複製する対象は、**今と同じ 8 種** (補正・ローカル調整・切り取り・マスク・隠蔽・注釈・★・タグ
  [`item_tags` と `tag_item_state`]) にとどめ、同じ表の列 (`copies_with_book_page: bool`) で宣言する。
  追加 (`append`) の引き継ぎ対象と揃っており、コピーの意味を今回変えない。
- `reading_history` (`rename_generic=false`) は本の改名のときだけ、汎用改名と同じ exact 移行を行う。
- mapping の適用は汎用改名と同じ 3 面 (exact、`key/` 配下、`key::` 仮想ページ)。本単位のキー (本フォルダ) は
  ページの mapping には一致しないので、並べ替えでは自然に動かず、本の改名 (フォルダ→フォルダ) でだけ動く。
  方針を「ページ単位」「本単位」に分ける必要はない。
- 値の中にページを持つ 2 つは、同じ worker の同じストアの transaction で値も書き換える:
  - `book_resume.page_identity` が `Image { path }` で、その path が mapping の旧側なら新側へ。
    本の改名では配下 prefix を付け替える (`page` 列は変えない。トリガーで identity が消えないよう、
    `page` を更新しない文にする)。
  - `folder_thumb_pins` の `container_key` が本フォルダで `source_rel` が旧ページ名なら、同じ本の中の新しい
    ページ名へ。別の本へ移ったページを指していた pin は**変えない** (汎用の移動・削除と同じく、表示時に
    見つからなければ自動サムネイルに戻る。2026-10-21 設計判断)。

### 3.2 実行場所: 製本 worker で FS の直後に行う

並べ替え・移動/コピー・改名・削除の各 worker は、FS の処理が確定した直後に、同じ worker の中で
`STORES` (方針 `Follow` / コピー列) の付け替え・purge を行う。

```text
[UI] 開始: 受付 → 書き込みの柵を取る → worker 起動 (以後、結果を消費するまでモーダル)
[worker] FS (既存の journal: Prepared → Applying → filesystem_committed)
[worker] 柵を待つ → store maintenance lease (EPUB 範囲・spread_shift・book_composition) を取る
[worker] STORES の付け替え (ストアごとに 1 transaction、一時キー 2 段) / コピー / purge
[UI] 結果: bookmark・spread_shift・book_composition・collections は従来どおり。
         App の記憶内の状態 (presence 集合、★/タグ cache、編集プレビュー、表示トリム、読書位置メーター、
         内容同一性の索引) を DB を読まずに失効・付け替え → 再読込
```

- 新しい公開関数は `rename_key_migration` に置く (例: `migrate_book_paths_at(data_dir, moves, copies, guard)`、
  `purge` は既存の `purge_removed_scopes_guarded_at` を使う)。汎用改名の `migrate_store` / 一時キーの
  考え方を流用し、ストアごとの処理を 2 か所に書かない。
- 失敗の扱い: ストアごとの失敗は従来の付け替えと同じく集計して UI に返し、ログ + 警告トーストにする
  (FS は確定済みなので巻き戻さない。まれな失敗の既定の割り切り)。
- UI 側の 8 種の付け替え (`move_book_page_edit_key` / `copy_book_page_edit_key` と、その前の
  `wait_for_local_adjust_writes`) は削除する。記憶内の presence 集合の付け替え
  (`move_page_key_presence` 等、I/O なし) は残すか、汎用改名の完了処理と同じ失効・再読込に揃える
  (実装担当がどちらが既存の経路に乗るかをコードで判断し、報告する)。

### 3.3 書き込みとの順序 (I5)

- **柵**: 開始時 (UI スレッド) に、ページの path キーへ非同期に書く writer ごとに FIFO の柵を取り、worker が
  DB に触る前に全部の完了を待つ。追加 (`append`) が `local_adjust_write_handle.enqueue_fence()` で
  既に行っている形を、汎用改名の開始条件 `rename_migration_writers_busy()` が見ている writer
  (タグ、★、編集プレビュー、読書位置、ローカル調整、bookmark の未消費要求、spread_shift service) に広げる。
  柵 API が無い writer には同じ形の柵を足す。DB に書かない writer (★のファイル XMP 等) は対象外でよい。
- **実行中の新しい書き込み**: 並べ替え画面と製本管理はすでにモーダル (`modal_dialog_block_reason`) だが、
  製本管理を閉じても worker は走り続ける。製本の構造操作が未完了の間は、追加の取り消し
  (`book_append_undo_in_progress`) と同じくモーダル理由に入れる。
- mIV Remote からの書き込みは、上のモーダルで止まるかを実装担当が確認する。止まらない経路があれば報告する
  (止めるか、まれな競合として割り切るかを設計担当が決める)。

### 3.4 途中で終了した場合 (まれな失敗の割り切り)

FS の確定と `STORES` の付け替えは別の DB・別のファイルなので、間で mIV が落ちると付け替えが途中になり得る
(今も同じ)。ストアごとの完了印や再実行の仕組みは**作らない** (置換は再実行すると逆に戻るため、
安全に再実行するには完了印が要る。CLAUDE.md の「まれな失敗は既定で割り切る」)。

- 起動時の book_bookmarks journal 復旧が、Applying または filesystem_committed の行を処理したら
  (= 前回の構造変更の結果を UI が受け取らなかった)、ログに残し、利用者へ 1 回だけ通知する:
  「前回、本の並べ替え・移動・名前変更の途中で mIV が終了しました。一部のページの補正・タグなどが
  正しいページに付いていない可能性があります。」
- 復旧の結果を UI へ返す型付きの経路が今は無い (`recover_path_migration_journal` は結果を返さない) ので、
  件数だけを返す経路を足す。

### 3.5 本の削除 (§1.273)

- `books::delete_book` を `undo_append_with` と同じ形にする: 柵を待つ → `acquire_delete_epub_guard(Tree(本フォルダ))`
  → `remove_dir_all` (既存どおり完全削除) → 成功したら `purge_removed_scopes_guarded_at(Tree(本フォルダ))`。
  結果に `store_mutations` と purge のエラーを載せる。
- UI は従来の処理に加えて `apply_content_identity_store_mutations`、`purge_in_memory_metadata_for_removed_paths`
  を行う。spread_shift / book_composition の `purge_book_root` は、汎用削除 (`delete_worker` の完了処理) が
  service に何をしているかに揃える (重複して害が無ければ残してよい。実装担当が判断して報告)。
- PDF パスワードは本に PDF が入らないので対象外 (空で渡す)。sidecar は本フォルダごと消えるので対象外。

### 3.6 永続データ (CLAUDE.md「永続データ・スキーマ変更時の判断」)

- スキーマ変更なし。新しい表・列・journal の段階名を足さない。旧版で開いても挙動は変わらない。
- 既存の DB に残っている「旧キーの付け残し」(今までの並べ替えで付け替えられなかった行) は直さない
  (どの画像の行だったかを後から決められない)。

## 4. テスト (実装担当)

- `rename_key_migration`: 方針列の網羅 (全 `STORES` 行が方針を持ち、`ServiceOwned` は 2 つだけ) と、
  入れ替え (A↔B) を含む mapping で全 `Follow` ストアが正しく入れ替わる・一時キーが残らないテスト。
- `book_resume.page_identity` と pin の `source_rel` の値の書き換え (同じ本の並べ替え、別本への移動で pin を変えない、
  本の改名の prefix)。
- 製本 worker の handler-level テスト: 並べ替え・移動・改名で表示トリムと内容同一性が新しい名前に付き、
  同名ページの入れ替えでも別ページへ付かない。削除で `STORES` の行が消え、本ページのしおりは残る。
  同じ名前で本を作り直しても旧ページの補正・タグ・表示トリムが付かない。
- 柵: 開始前に積んだローカル調整・タグの書き込みが、付け替えより先に旧キーへ着地し、付け替えで新キーへ運ばれる。
- 起動時の journal 復旧が Applying / filesystem_committed の行を処理したときに通知が 1 回出る。

## 5. しおりのページ番号 (§1.245)

- 保存時の `page_index_hint` を「その本の読み順でのページの位置 (0 始まり)」にする。
  数え方は、フルスクリーンのページ表示 (「N / 全体」) が使う関数と同じものを使い、2 か所に書かない。
  本の構成でページを非表示にしている場合も、同じ関数の扱いに従う。
- 対象は本体の保存経路と、mIV Remote が bookmark を作る経路 (あれば) のすべての `NewBookBookmark` 生成元。
- 製本 (compiled book) の移行時の再計算 (`compiled_page_index`) は今のまま (同じ値になる)。
- 既に保存済みの行の hint は直さない (どのページかは identity から引けるが、本ごとに一覧を作り直す必要があり、
  一覧表示のたびに行うには重い。表示の数字だけの P3)。新しく付けたしおりから正しくなる。
  同じ本で古い行と新しい行が混ざると、本の中のしおりの並び順が一部ずれ得る。これを許容するかは
  レビューの論点とする。
- テスト: サブフォルダ 2 つ + 画像 3 枚の ZIP の先頭画像で作った draft の hint が 0、PDF は従来どおり、
  製本は `NNNN - 1`。

## 6. 採らなかった案

- **rename-migration FIFO (汎用改名の journal つきキュー) に一括ジョブとして積む**: 書き込みの待ち合わせと
  記憶内の再読込が既にあり魅力的だが、(1) 置換の一括ジョブは途中で落ちた後の再実行で逆に戻るので、
  journal で「少なくとも 1 回」実行する今の仕組みと合わない、(2) FS の確定から付け替えまでに UI のフレームと
  他のジョブの待ちが挟まり、その間に本を開き直すと旧キーの編集が別ページに見える。製本 worker の中で
  FS の直後に行うほうが窓が小さく、所有者も 1 つで済む。
- **R2 の関所・受領証・ストアごとの完了印**: 途中終了からの完全な回復ができるが、仕組みが大きく、
  まれな失敗のための回復処理は割り切る方針 (CLAUDE.md) に反する。
- **UI スレッドで App の DB handle を使って `STORES` 全部を動かす**: App が handle を持たないストアの
  cold open が UI スレッドで起きる (CLAUDE.md「UI スレッドで同期 I/O をしない」)。

## 7. レビュー記録

(未実施)

## 8. 実装記録

(未実施)

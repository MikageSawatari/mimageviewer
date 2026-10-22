# 本の構造変更と path キー DB の付け替え・後始末 (§1.272 / §1.273) と、しおりのページ番号 (§1.245)

2026-10-21 設計担当 ClaudeCode Opus 5.5。状態: **設計 v4 (v3 で実装前レビュー合意、実装中の判断と D2c を §3.10 で決定)。実装・独立レビュー・関連自動検証済み。全件 gate・実機確認・master 取り込み前の利用者確認は設計担当へ引き継ぐ**。
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

### 3.10 実装中に決めた点 (2026-10-21、設計担当)

実装担当が §8.4.1 で返した前提の不一致への決定。

- **別本へ移したページを指す本の構成・見開き調整の参照**: 移動元の本の `book_composition` (anchor・hidden・
  override・order) と `spread_shift` に残る、別本へ移したページへの相対参照は、各 service が本の操作の要求の中で
  **外す** (pin と同じ扱い。§3.4)。残すと詰め直しで同じ名前になった別の画像を指す。移動先の本へは移さない
  (移動先には移動先の本の構成がある)。外した参照は戻らないので、**利用者が作ったデータ (本の構成) の変更**として
  master への取り込み前に利用者へ確認する (CLAUDE.md「まれな失敗は既定で割り切る」の相談対象 1)。
  **2026-10-22 利用者了承** (「外れるで良い」)。なお製本の本は画像フォルダなので「本の構成」を作れない
  (構成の元ページは書庫・文書だけ。`book_composition_editor_prepare.rs` の `BookCompositionSource`、
  [book-composition-plan.md](book-composition-plan.md) §1「製本フォルダは構成作成対象外」)。製本の操作で実際に外れるのは
  見開き調整と本のサムネイル指定で、構成の参照を外す処理は所有者ごとの一律の規則として残している。利用者も「本棚の特性を考えると外れて問題ない」と確認 (2026-10-22)。
- **別ウィンドウ (F12) と Snapshot**: リリース済みの挙動 (`spread-shift-memory-plan.md` §11.6: 本の改名後も
  F12 は旧 path・旧 items を保つ) を保つ。§3.5 の失効・再読込は、既存の共通 reload / refresh で作り直される
  context だけに適用し、別ウィンドウ・Snapshot の context の cache は**触らない** (cache だけを新しい DB に
  合わせると、旧い画素と別画像の編集情報が混ざる)。その窓は利用者が閉じて開き直すまで変更前の状態を表示する。
  変更前の 8 種の付け替え (UI スレッド) でも別ウィンドウは同じ状態になっていたので、退行ではない。detached の
  述語・viewport には触れない。再読込の設計は detached リワーク側の範囲として §8 に記録する。
  - **触らないのは「作り直し」と「cache の付け替え」まで** (2026-10-21、コードレビュー 1 回目の P2 への判断)。
    保持している context も、編集の全体通知 (`PAGE_EDIT_WRITES`) を受けた通常の再照合では、持っている旧い path の
    キーで DB を読み直す。本の操作で旧キーが空になれば編集表示が消え、詰め直しで旧キーが別の画像の行になれば
    その画像の編集が見える。変更前の 8 種の付け替えも同じ全体通知を出していたので以前からの挙動で、退行ではない。
    正しく直すには「保持している窓が持つ path キーと、キーの所有画像が変わる操作」の契約全体の設計が要り、
    detached リワークの範囲なので、今回は直さずバックログに起票する (症状パッチは入れない)。
- **開いている途中の構造操作 (D2c、2026-10-21 設計担当)**: 未採用の open (フォルダ・本を開く処理の結果を UI が
  まだ採用していない状態) が 1 つでもある間は、構造操作を**受け付けない** (理由を出し、開き終わってから
  やり直してもらう。同じ本か別の本かは区別しない)。並行を許すと、受付済みの open の続き (最初の表示、
  hydration 後の移動、遅延したフルスクリーン移行) を構造操作のモーダルが「新しい入力」として止めないための
  境界の設計が別に要り、扱う組み合わせが増える (CLAUDE.md「設計の簡素化」)。リリース済みの挙動
  (`spread_shift_owner_tests` の「開いている途中でも改名は待たない」) からの変更で、影響は「開いた直後の
  読み込み中に、製本管理から改名・削除・並べ替えの保存をした」場合だけ。利用者への報告に含める。
  **2026-10-22 利用者了承** (「問題ない」)。

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
| XMP rating prewarm | registry の immutable 参照で全 context の `TagPrewarmPending::is_busy`、App 共通 `XmpRatingHydration::is_busy` を確認。Requested の明示 poll が結果を回収し、DB 処理と UI 採用まで待つ。新しい可視範囲 job の受付だけ止める |
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

静止述語は初回に汎用改名と共有したが、§8.7 の回帰修正で本の構造操作専用の
`book_structural_writers_busy` に分けた。汎用改名・sidecar 復元の既存開始条件を広げない。
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
既存 reload で作り直される main context のタグ・★・rotation・page-edit・preview・pin cache を失効し、
既存の非同期準備へ戻す（v4 の範囲決定は §3.10・§8.4.1）。
本棚の `Folder(本)` は page key を持たないため、ページの両端の親フォルダ (改名は両端の本 root) も
pin の依存先として確認する。親棚は既存の `CurrentViewRefresh::Pins` worker に対象だけ渡し、
本を開いていない棚や無関係な窓の page-edit・★・タグ状態は初期化しない。
presence はソート済みの対象範囲だけ snapshot し、全キー数 × mapping 数の UI 走査を避けた。
本の削除は worker が service maintenance lease 下で purge し、lease 解放時に再読込されるため、
UI の `purge_book_root` の重複要求は削除した。

### 8.4.1 実装中の前提の不一致と決定 (決定済み・対応済み、2026-10-21)

初回実装 `3ee4de3bb` では下記二点を保留した。設計担当が `33e9a570a` の §3.10 で決定し、
今回の追補で実装した。未合意の blocker としては残っていない。

D2c も `92f8d21b1` の §3.10 で決定済み・§8.8 で対応済み。
未採用 open が一つでもあれば構造要求を理由付きで拒否し、同本・別本を区別しない。
並行の契約は維持せず、既存 open を通常採用した後の再要求で進める。

- scope 外へ出たページへの `book_composition` / `spread_shift` の参照は、本操作専用の
  `migrate_book_page_paths` 要求に `RemoveMovedOutside` を載せ、各 service の transaction 内で外す。
  構成の PageRef 保持箇所は order・hidden・override の三つ。anchor は spread_shift の row が保持する。
  direction / cover は本単位で維持し、移動先の構成・shift へは移さない。汎用移動の Missing 保持は変えない。
- 別窓・Snapshot の旧 items / accepted shift を直接作り直さず、cache を新 DB のキーへ付け替えない。
  実 worker の通知後に通常の旧 path 再照合が走る限界は §3.10 に従い、cache の不変は保証しない。
  全 context の cache 更新と at-rest shift の
  一括付け替えを撤去し、main でも既存の完了終端から実際に reload される範囲だけ page cache を失効する。
  本の改名は current_folder がある main、ページ操作は source / target 本の main が対象。
  再構築されない抽出一覧も維持する。親棚の pin は既存 Pins worker に限定し、page cache は初期化しない。
  本削除の purge は shared cleanup と現在 context の cleanup に分け、本操作では current_folder のある
  main 非 Snapshot だけに後者を適用する。汎用削除 wrapper は両 cleanup を通す従来動作を維持する。

新しい reload / live-follow / reader 閉鎖は追加しない。別窓の再読込設計は detached リワーク側の範囲。
設計担当と独立 reviewer は、これは BA-7 の所有境界を正す撤去であり症状パッチではないと合意した。
参照解除は利用者作成データの変更であり、§3.10 の master 取り込み前の利用者確認は設計担当が行う。

### 8.5 初回 v3 実装時の検証状況 (履歴)

以下は `3ee4de3bb` の検証履歴。v4 追補の結果は §8.6 に記す。

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
- `test-lib-full.ps1`: 指示どおり設計担当へ引き継いだ。当時は二点未合意で全体の受入を保留した（現在は §3.10 で決定済み）。

実装後の独立レビュー (`gpt-6.1-sol` / `xhigh`) は合意範囲を ACCEPT、追加 P1 / P2 なし。
上記の親担当の検証結果を参照し、重複実行していない。当時は二点未実装のため、
タスク全体の完了承認を保留した。41 件と focused 600 件には重複があり、641 個の異なるテストとは数えない。

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

### 8.5.1 初回 v3 で新規・改名したテスト名の一覧

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

### 8.6 v4 決定の実装と検証 (2026-10-21)

§3.10 の二点を実装。service の方針と context の所有境界は §8.4.1 に記録した。
独立完成レビュー (`gpt-6.1-sol` / `xhigh`) は ACCEPT、追加 P1 / P2 なし。
前回結果を流用せず、今回の Rust 差分に対して以下を実行した。
今回の指示により build-dev・本体起動・commit は行わない。全件 gate は設計担当へ引き継ぐ。

すべての build / test に `MSBUILDDISABLENODEREUSE=1`、`CARGO_BUILD_JOBS=1` を指定した。

| 検証 | 結果 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0。lib 264 / ort 1 の既存警告。新規未使用警告は解消し前回から増加なし |
| `cargo nextest run -p mimageviewer --lib -E $bookV4Filter` (下記) | 33 件成功、失敗 0。新規 8 テストすべてを JUnit と照合済み |
| `scripts/test-focused.ps1` (前回 13 module + viewer_context_registry) | 14 run / 702 件成功、失敗 0、exit 0 |
| `cargo fmt` (引数なし、workspace 全体) / `cargo fmt --check` | 成功 |
| `python scripts/check_ui_glyphs.py` | 成功、dangerous glyph 0 |
| `git diff --check` | exit 0 |

33 件と focused 702 件には重複がある。distinct な 735 件とは数えない。
core check は `target/book-final-check.stderr.log`、選択 nextest は `target/book-v4-nextest.stderr.log` /
`target/nextest/default/junit.xml` に保存した。focused の今回のログ / JUnit は
`target/test-focused/runs/20261021T125003091Z-abd6eec0` から `20261021T125225239Z-3fc4d814` までの 14 run。
件数は順に books 56、rename_key_migration 61、edit_preview_cache 20、book_bookmarks 28、
book_composition 71、spread_shift 62、book_structural_op 12、page_edit_owner_tests 26、
content_identity_restore 11、spread_shift_lifecycle_tests 4、remote_ipc::container 173、
content_identity 75、reading_history_db 9、viewer_context_registry 94。

```powershell
$env:MSBUILDDISABLENODEREUSE = '1'
$env:CARGO_BUILD_JOBS = '1'
$bookV4Filter = @(
    'test(/book_structure/)', 'test(/book_page_/)', 'test(/book_transfer_completion/)',
    'test(/book_root_rename_preserves_mounted_and_parked_context_payloads/)',
    'test(/book_manager_delete_worker/)',
    'test(/delete_closes_parked_live_media_window_and_purges_resume/)'
) -join ' | '
cargo nextest run -p mimageviewer --lib -E $bookV4Filter
# focused は §8.5 の明示 13 パスに下記を追加して同時指定した。
# 'src/app/viewer_context_registry.rs'
```

新規テスト:

- `book_composition::store_tests::book_page_transfer_request_removes_departed_refs_before_name_reuse`
- `book_composition::store_tests::book_page_reorder_request_maps_once_and_generic_move_keeps_missing_refs`
- `spread_shift::service::tests::book_page_transfer_request_clears_departed_anchor_before_name_reuse`
- `spread_shift::service::tests::book_page_reorder_request_maps_once_and_generic_move_keeps_missing_anchor`
- `app::spread_shift_lifecycle_tests::book_transfer_completion_clears_departed_composition_and_shift_before_name_reuse`
- `app::book_structural_op::tests::book_structure_adoption_preserves_mounted_and_parked_reader_items_caches_and_shift`
- `app::book_structural_op::tests::book_structure_snapshot_keeps_items_caches_and_saved_shift_through_mapping_and_delete`
- `app::book_structural_op::tests::book_structure_unreloaded_main_extraction_keeps_page_caches`

既存 `book_structure_moves_rating_ledger_and_invalidates_parked_context_caches` は、v4 に合わせて
`book_structure_moves_rating_ledger_and_preserves_independent_reader_caches` へ改名し、main の失効と別窓保持を検査する。
registry の root 改名・FS resource 保持回帰も削除した helper から実際の book adoption 入口へ更新した。

### 8.7 全件テストと独立レビューの追補 (2026-10-21)

設計担当が `6972a8534` の全件テストと単独再実行で確認した 7 件を調査した。
前回は 6 件を次の判断で修正し、D2c の受付済み open 境界を確認待ちとした。
D2c は現在、`92f8d21b1` の §3.10 により決定済み・§8.8 で対応済み。
期待値を緩めて失敗を消す対応にはしない。

| 失敗 | 判断と対応 |
| --- | --- |
| `adjusted_retained_f12_keeps_session_across_unrelated_and_same_book_rename` | 新 owner へのテスト更新。旧 `book_op_pending` のみの poll では構造 worker が始まらない。受理済み writer を静止させ、retained readoption は未採用のまま、実 worker と service の同じ隔離 data-dir で旧 session 保持を確かめる |
| `book_rename_does_not_wait_for_same_book_pending_open_or_unrelated_open` | 決定済み・対応済み。§3.10 が旧並行契約を変更したため、§8.8 で `book_rename_rejects_pending_open_then_accepts_after_original_open_adoption` へ置換。拒否中の FS/DB 不変・受付済み open の継続・採用後の再要求成功を検証する |
| `book_rename_saved_rows_move_but_open_scopes_and_stale_actions_stay_old` | 新 owner と worker 所有へのテスト更新。FS・保存済み行の改名、開いた旧 scope と stale action の保持を実 worker 経路で引き続き確かめる |
| `visibility_in_book_pc_projects_nested_archive_provenance_and_legacy_notice` | fixture 更新。items の直代入で reader order が空だったため新 hint を作れなかった。通常 install と表示順構築を通し、missing の削除経路・legacy 通知・再生成後の行・保存済み行数の検査を維持する |
| `zip_sent_save_does_not_block_unrelated_sidecar_writer_readiness` | 製品の退行。広げた共通述語が無関係な送信済み構成 save ticket を sidecar 復元の開始条件に混ぜていた。既存の狭い条件へ戻し、元のテストをそのまま通す |
| `zip_sent_save_retired_owner_precedes_generic_migration_after_journal_ack` | 製品の退行。generic 改名の writer 判定に構成 ticket を混ぜ、別の既存 owner が保護する境界を壊していた。writer 判定を戻し、journal ACK・ticket の完了採用・resume writer の静止の全条件を元のテストで維持する |
| 旧 `g3_bookmark_hint_uses_raw_position_and_composed_detail_order_is_locked` | §5 / I7 の意図した変更。`bookmark_hint_uses_composed_reading_position_and_composed_detail_order_is_locked` へ改名。順序変更・hidden を含む HUD の読み順位置を hint に保存し、raw 位置は resume 用に維持する。`book-composition-plan.md` も更新 |

静止の判定は副作用を持たない `book_structural_writers_busy` に集約した。汎用改名と
sidecar 復元は構成 ticket・全 context の reader・履歴などへ条件を広げず、既存契約を維持する。
今回 UI 同期処理から分離した XMP の DB writer だけは新しい非同期 writer として両者にも含める。

D2c の未採用 open をそのまま許可すると、`FirstDisplayAction`・`navigation_after_hydration`・
deferred fullscreen の受付済み継続が構造 modal の新規入力拒否に遮られる経路がある。
phase だけの許可やテストの None ペイロードだけへの個別許可は入れない。
既存の並行契約を保つには受付済み継続と modal の境界の設計が必要だったため、前回は確認待ちとした。
設計担当が §3.10 で並行を許さず open 完了後の再要求へ変更すると決定し、§8.8 で実装した。

P2-2: parked を含む context-local XMP 読込の結果をフレームの明示 poll で回収し、
App 共通 `XmpRatingHydration` が DB の照会・未評価行の補充・完了票を所有する。
context の reader 取消後も、受理済み DB 処理と UI 採用は捨てない。
明示評価・クリアは SQLite に入る前に同 path の pending import 票を失効させ、worker は
writer transaction の中で票と既存非ゼロ値を確認する。UI 採用時も既存 rating 世代を確認し、
古い import から評価を復活させない。DB 完了・未採用結果・後続の採用まで静止条件へ含める。
parked 結果・worker の DB 前停止・完了後未採用を分離した状態遷移テストを追加した。

P3: Remote は ZIP/PDF の検証済み raw 列に既存の構成 projection を適用し、order / hidden を
反映した可視位置から hint を求める。`page_index` は従来の resume raw index のまま。
ZIP の子フォルダ二つと PDF の両方で、Remote の検証・保存受付・UI writer・DB 読戻しまでをテストする。
しおり一覧の読取りは従来の元列検証を維持し、hidden anchor や構成 DB の読込失敗を新たな拒否にしない。
独立レビューで発見したこの境界も、ZIP/PDF の実際の一覧要求を通す回帰テストで固定した。

P2-1 は設計担当の `a7a8ac715` / §3.10 の決定どおり、製品コードを修正しない。
Snapshot テストを `book_structure_snapshot_adoption_does_not_rebuild_or_remap_context` へ改名し、
直接 adoption が reader を再構築・新キーへ remap しないことだけを主張する。
実 worker の `PAGE_EDIT_WRITES` 通知後の通常再照合によって旧 path の編集 cache が変わり得る点は
今回の未解決バグとして隠さず、別バックログの範囲とする。

新規テスト:

- `app::book_structural_op::tests::book_structure_parked_xmp_result_runs_db_on_worker_and_waits_for_adoption`
- `app::tests::native_video_rating_key_tests::accepted_xmp_hydration_cannot_restore_a_later_user_clear`
- `app::tests::native_video_rating_key_tests::xmp_hydration_preserves_existing_nonzero_db_rating`
- `remote_ipc::container::tests::remote_zip_and_pdf_bookmark_saves_use_composed_visible_order_and_keep_raw_resume`
- `remote_ipc::container::tests::remote_bookmark_lists_preserve_hidden_anchor_and_ignore_composition_read_errors`

既存の `book_structure_waits_for_unconsumed_tag_result_and_its_identity_record` は、分離した本専用の静止述語で
後続 recorder まで待つ状態遷移テストに更新した。generic の狭い writer 条件ではその完了を保証しないためである。

検証結果（未コミット差分、通常 feature / dev・test profile）:

- 全 build / test に `MSBUILDDISABLENODEREUSE=1` と `CARGO_BUILD_JOBS=1` を指定。
- `cargo check -p mimageviewer --bin mimageviewer-core`: exit 0。lib 264 / ort 1 の既存警告で増加なし。
  最終ログは `target/book-review-check.stderr.log`。
- `cargo nextest run -p mimageviewer --lib -E $bookReviewFilter`: 42 件成功、失敗 0、exit 0。
  上記 6 件と新規 5 テストが実行済みであることを JUnit と照合した。
  D2c の `book_rename_does_not_wait_for_same_book_pending_open_or_unrelated_open` は明示的に除外し、
  成功扱いにしていない。ログは `target/book-review-nextest.stderr.log`、
  JUnit は `target/book-review-nextest.junit.xml`。
- 初回の同系 filter は 41 件中 40 件成功、1 件失敗だった。
  `book_structure_waits_for_unconsumed_tag_result_and_its_identity_record` が旧 generic 述語で待っていたためで、
  上記の本専用述語へテストを更新後、最終 42 件に含めて成功した。
- `scripts/test-focused.ps1 -Path <下表8パス>`: 8 run / 356 件成功、失敗 0、exit 0。
  D2c を含む `spread_shift_owner_tests` 全体は未実行。その中の修正済み二つは上記の選択 nextest で成功した。
  ログは `target/book-review-focused.stdout.log`、各 JUnit は `target/test-focused/runs/` の
  `20261021T133802335Z-81167f4f` から `20261021T133853600Z-4d82b963` まで。
- `cargo fmt`（引数なし、workspace 全体）、`cargo fmt --check`、`git diff --check`: 成功。
- `python scripts/check_ui_glyphs.py`: 成功、dangerous glyph 0。
- 独立コードレビュー（`gpt-6.1-sol` / `xhigh`）は確定範囲を ACCEPT、追加 P1/P2 なし。
  Remote 一覧への拒否拡大の指摘は修正・再レビュー済み。D2c は承認対象外。

| focused の明示パス | 成功件数 |
| --- | ---: |
| `src/app/book_structural_op.rs` | 13 |
| `src/app/tests/file_type_visibility_fixes.rs` | 13 |
| `src/app/book_composition_editor/metadata_transfer_tests.rs` | 5 |
| `src/app/book_composition_runtime_tests.rs` | 40 |
| `src/app/viewer_context_registry.rs` | 94 |
| `src/remote_ipc/container.rs` | 175 |
| `src/rating_db.rs` | 13 |
| `src/tag_prewarm.rs` | 3 |

42 件と focused 356 件には重複がある。この検証時点では D2c が未完了だった。現在の対応・検証は §8.8。
指定により build-dev・本体起動・commit は行っていない。全件テストは設計担当が担当する。

今回の選択式（D2c の除外を含む）:

```text
(
  test(/book_structure/) | test(/book_rename/) |
  test(/adjusted_retained_f12_keeps_session/) |
  test(/visibility_in_book_pc_projects_nested_archive_provenance_and_legacy_notice/) |
  test(/metadata_transfer_tests/) | test(/bookmark_hint_uses_composed/) |
  test(/remote_zip_and_pdf_bookmark/) | test(/remote_bookmark_lists_preserve_hidden_anchor/) |
  test(/rating_hydration/) | test(/xmp_hydration/) | test(/tag_prewarm/) |
  test(/detached_current_container_ratings_share_generation_with_main_and_stale_hydrate/) |
  test(/rename_migration_waits/) | test(/book_resume_meter_rename/)
) & !test(/book_rename_does_not_wait_for_same_book_pending_open_or_unrelated_open/)
```

### 8.8 D2c 決定の実装 (2026-10-21、決定済み・対応済み)

設計担当の `92f8d21b1` / §3.10 に従い、未採用 open と構造操作は並行させない。
受付の `book_structural_open_pending` 一か所で process 全体を判定し、同本・別本の照合や
待機状態を追加せず、拒否後は元の open owner の通常完了・採用を進める。
理由は「フォルダや本を開いています。開き終わってから本の変更をやり直してください」。
拒否は lease・構造 worker の作成より前なので、FS や保存済み DB を変更しない。

`sidecar_restore_active` は Common の採用待ちと、採用済み session の retained 見開き再照合を混ぜるため、
そのままではこの意味にならない。構造操作の open 判定では Common 自体を確認し、retained 再照合は含めない。
次の既存の所有状態も集約した。detached の述語・viewport・既存 open 継続は変更していない。

| 所有状態 | 未採用 open として確認する範囲 |
| --- | --- |
| App 共通 | Common hydration、saved-group open、startup path resolver、media navigation resolver、smart PDF password、RAR 等の変換後 open（SiblingZip・終端 Error を除く） |
| 全 viewer context | path 分類、folder scan/navigation、履歴・Rating navigation、Collection navigation と未採用の初期 root、Smart staged child、bookmark resolver、ZIP/PDF 列挙、EPUB 変換・PDF password |
| 受付済みの表示継続 | Rating ZIP prefix、auto fullscreen・列挙後 reopen、qualified book open の未採用 phase、Flat stack open（retry を含む） |

採用済み Collection root の再準備、通常表示の permission 読取、採用済みの Resuming 証明、
stack の grouping/aggregate 再構築、native video の採用後 decoder 待ちは open に含めない。
`CurrentViewOrderRefresh` は open と区別し、従来の非 open の変更操作としての拒否を別途維持した。
新しい bool・pending owner・取消・完了待ち・UI 同期 I/O は追加していない。

旧 D2c テストを `book_rename_rejects_pending_open_then_accepts_after_original_open_adoption` へ置換した。
同本・別本それぞれで resume writer の未完了／完了済みだが UI 未採用を扱い、次を確かめる。

- 拒否中は元ファイル・移行先の有無・実 service DB の行・maintenance epoch が変わらず、Common の request ID も保持する。
- 拒否後も元の resume fence を捨てない。後から別窓が位置 4 を保存しても元の open は捕捉済みの位置 2 を採用する。
- 通常採用後の再要求は完了し、FS・見開き row・同じ実 store の最新 resume 行が改名へ追従する。
- 採用済み reader の旧 scope と別窓の状態を、改名のために新 scope へ付け替えない。

旧ケースの「改名を跨いだ未採用 shift 結果を棄却する」順序は、新契約では改名前に open を採用するため発生しない。
代わりに、拒否後の通常 open が元の新鮮な保存基点を採用することを検査する。
retained の取消・再入場・maintenance の既存回帰は削らず、`spread_shift_owner_tests` 全体で確認する。

追加テスト:

- `book_structure_rejects_mounted_and_parked_path_classification_without_consuming_open`
- `book_structure_waits_for_open_permission_but_not_passive_display_reads_or_adopted_episode`
- `book_structure_rejects_flat_stack_open_until_its_accepted_fullscreen_is_adopted`

利用者への影響は、source の読込み直後に加え、受付済みの本ページ表示・stack open・media navigation の
表示継続が未採用の短い間にも構造要求を拒否し、開き終わってから再要求してもらうこと。
開き終わった本の改名・削除・並べ替え保存の手順や、既存の open の継続は変えない。

検証結果（今回の未コミット差分、通常 feature / dev・test profile）:

- 全 build / test に `MSBUILDDISABLENODEREUSE=1` と `CARGO_BUILD_JOBS=1` を指定。
- `cargo check -p mimageviewer --bin mimageviewer-core`: exit 0。既存の lib 264 / ort 1 警告で増加なし。
  ログは `target/d2c-check.stderr.log`。nextest の lib test も既存の 135 警告で増加なし。
- 新規・置換の 4 テストを選んだ nextest: 4 件成功、失敗 0、exit 0。
  ログは `target/d2c-narrow.stderr.log`、JUnit は `target/d2c-narrow.junit.xml`。
  初回は permission fixture の context binding 不足で 1 件失敗し、binding を補う途中で
  fixture 内の借用エラーも修正した。製品コード・期待値は変えず、最終実行では全件成功した。
  初回ログは `target/d2c-narrow-first.stderr.log`、借用エラーは `target/d2c-narrow-bind-error.stderr.log`。
- `cargo nextest run -p mimageviewer --lib -E <関連選択式>`: 77 件成功、失敗 0、exit 0。
  §8.7 の選択式から末尾の D2c 除外を外し、`test(~app::spread_shift_owner_tests::)` を追加した。
  同モジュール全 34 件と、置換した D2c・新規 3 テストが実行済みであることを JUnit で照合した。
  ログは `target/d2c-related.stderr.log`、JUnit は `target/d2c-related.junit.xml`。
- `scripts/test-focused.ps1 -Path <下表11パス>`: 11 run / 450 件成功、失敗 0、exit 0。
  ログは `target/d2c-focused.stdout.log`。各 JUnit は `target/test-focused/runs/` の
  `20261021T140609814Z-9567c57d` から `20261021T140728356Z-78e9d1cf` まで。
- `cargo fmt`（引数なし、workspace 全体）、`cargo fmt --check`、`git diff --check`: 成功。
- `python scripts/check_ui_glyphs.py`: 成功、dangerous glyph 0。
- 独立コードレビュー（`gpt-6.1-sol` / `xhigh`）は ACCEPT、追加 P1/P2 なし。

| focused の明示パス | 成功件数 |
| --- | ---: |
| `src/app/book_structural_op.rs` | 14 |
| `src/app/spread_shift_owner_tests.rs` | 34 |
| `src/app/tests/file_type_visibility_fixes.rs` | 13 |
| `src/app/book_composition_editor/metadata_transfer_tests.rs` | 5 |
| `src/app/book_composition_runtime_tests.rs` | 40 |
| `src/app/viewer_context_registry.rs` | 94 |
| `src/remote_ipc/container.rs` | 175 |
| `src/rating_db.rs` | 13 |
| `src/tag_prewarm.rs` | 3 |
| `src/app/book_composition_permission.rs` | 31 |
| `src/filename_stack_ui.rs` | 28 |

選択 nextest と focused の件数には重複がある。D2c は決定済み・対応済みで、今回の範囲に未解決事項はない。
指定により build-dev・本体起動・commit は行っていない。全件 gate は設計担当へ引き継ぐ。

### 8.9 XMP ★ import と削除 purge の競合 (2026-10-21)

設計担当が `0aef092b6` で実行した `test-lib-full.ps1` は 14,028 件成功。
その後の独立レビュー第2回で、XMP import が通常削除の purge 後に旧 path の★を復活させる P2 が出た。

原因の判定: **読込結果と削除 purge の競合自体は worker 化前から存在した**。
`a6a0b3a49^` の `poll_tag_prewarm_results` は、未消費の結果をそのまま同期
`get_many` / `set_imported_rating` に渡していた。通常削除の開始は新規 enqueue を止めるだけで
受付済み reader を失効しない。削除 worker の purge 後、削除完了を UI が採用して reader を
再準備する前に結果を消費すると、行がないことを未評価として再登録できた。削除後は session 台帳も
消すため、rating 世代だけでは遅い結果を除外できない。
worker 化でこれに加わったのが、context reader の取消後も App 所有の DB job / 未採用完了が残る経路である。
今回 App が import の所有者になった境界で両方を修正する。

採った方法は、**既存の明示評価・クリア用の取消票を読込受付まで延長し、purge 前に失効する**こと。
新しい世代・path blacklist・pending owner・待機状態は追加しない。
`XmpRatingHydration` の同じ票を `PrewarmJob` → `TagPrewarmResult` → DB job → UI 完了が保持し、
遅い読込から submit するときも票を作り直さない。対象は従来の purge と同じ exact / `/` / `::` 境界。
受付済み票の import 数で有効な DB job と完了採用を数え、取消済み job は静止待ちから外す。
未取消の job は従来どおり UI 採用まで待つ。

SQLite の `BEGIN IMMEDIATE` 内で票を検査する既存実装を維持する。
import が先に writer を取得・検査した場合はその commit 後に purge が消し、purge が先なら
後続 import は取消票で書き込まない。UI でも同じ票を検査し、session 台帳へ再公開しない。
同じ名前の新しい読込は新しい票を取得できるため、削除・取消の失敗や同名 path の再利用を永続的に拒否しない。
対象の queued index だけを解除し、無関係な読込・採用済み reader は取り消さない。

| purge の入口 | 取消の境界 |
| --- | --- |
| 通常削除 | `start_delete_files` の受付後、viewer release / worker spawn 前 |
| 本の削除 | `request_book_structural_op` の Delete 受付後、Requested の作成前 |
| 本への追加 Undo | 既存 writer 判定・teardown 後の再確認が通った後、Undo worker の開始前。すでに受付済みの DB import は既存 writer 条件でも順序付けられ、残る読込票も同じ scope 取消を受ける |
| 明示メタデータ整理 | scan の candidate key だけを、既存 delete worker の開始前に失効。新規 read は Delete task 中だけ停止し、読み取り専用の Scan task 中は従来どおり進める |
| 既存の削除 purge journal 再実行 | UI は受付済み票の snapshot だけを worker に渡す。worker が既存 journal を読んで孤児と確認した scope だけ、purge 前に同じ票を失効。空 journal・現存 path・無関係な prefix は取り消さない。完了採用では対象の shared metadata / session 台帳を purge し、queued index も解除する |

上記以外の `content_identity` 内の purge 呼出しはテスト内であり、製品の追加入口ではない。
再実行方式・障害分類・回復手順は追加しない。DB 処理は既存 worker のままで UI 同期 I/O を足さない。

追加した回帰テスト:

- `xmp_hydration_cannot_restore_rating_after_normal_delete_purge`: 実際の通常削除 handler / worker / purge / UI 採用を通す。単一画像とフォルダの両方を扱う。
- `xmp_hydration_cannot_restore_rating_after_book_delete_purge`: 本削除の構造 owner と実 worker / purge を通す。
- 上の二つは import の DB 前停止・完了後 UI 未採用の両方で、purge 完了後に旧完了と遅い読込を消費しても DB 行と session 台帳が復活しないことを確認する。初回読込の世代 0 を使い、purge 後の世代不一致だけでは除外できない経路を検査する。同名 path を作り直してから旧結果を渡し、無関係な sibling の★と読込票を維持し、新しい票では新しい★を取り込めることも確認する。
- `delete_purge_retry_invalidates_only_confirmed_orphan_xmp_tickets`: 空 journal と現存 path の票を維持し、実際に purge する root・子 path・archive entry だけを失効する。
- 既存 clear 回帰は取消済み完了を必ず一度 poll するよう fixture の settle を調整し、旧完了を消費しても clear を守る検査を維持する。

検証結果（今回の未コミット差分、通常 feature / dev・test profile）:

- 全 build / test に `MSBUILDDISABLENODEREUSE=1` と `CARGO_BUILD_JOBS=1` を指定。
- `cargo check -p mimageviewer --bin mimageviewer-core`: exit 0。既存の lib 264 / ort 1 警告で増加なし。
  最終ログは `target/xmp-purge-check.stderr.log`。lib test も既存の 135 警告で増加なし。
- 最初の新規 3 テストの nextest は 3 件成功。その後、初回読込の世代 0 と無関係な票の保持を fixture で明示した。
  この強化版で関連 159 件が成功し、さらに Scan task を停止対象から外した最終版でも、下記の関連 159 件が成功した。
- `cargo nextest run -p mimageviewer --lib -E <下記選択式>`: 159 件成功、失敗 0、exit 0。
  新規 3 件、既存 clear・parked XMP・D2c、および `spread_shift_owner_tests` 全 34 件を実行済みと JUnit で照合した。
  `section1373_metadata_database_cold_and_warm_per_file_benchmark` は 127.329 秒の slow 表示で成功。
  ログは `target/xmp-purge-related.stderr.log`、JUnit は `target/xmp-purge-related.junit.xml`。
- `scripts/test-focused.ps1 -Path <下表6パス>`: 6 run / 65 件成功、失敗 0、exit 0。
  ログは `target/xmp-purge-focused.stdout.log`。各 JUnit は `target/test-focused/runs/` の
  `20261021T144911516Z-ec6ed7a7` から `20261021T144932471Z-e893f908` まで。
  App 本体の新規回帰は上記の明示 nextest で直接実行し、focused は下表のモジュールを明示した。
- `cargo fmt`（引数なし、workspace 全体）、`cargo fmt --check`、`git diff --check`: 成功。
- `python scripts/check_ui_glyphs.py`: 成功、dangerous glyph 0。利用者向け UI 文言の追加はない。
- 独立コードレビュー（`gpt-6.1-sol` / `xhigh`）は最終境界を ACCEPT、追加 P1/P2 なし。

| focused の明示パス | 成功件数 |
| --- | ---: |
| `src/tag_prewarm.rs` | 3 |
| `src/rating_db.rs` | 13 |
| `src/metadata_cleanup.rs` | 13 |
| `src/app/book_structural_op.rs` | 14 |
| `src/app/book_append_undo.rs` | 5 |
| `src/delete_worker.rs` | 17 |

選択式:

```text
test(/book_structure/) | test(/book_rename/) |
test(~app::spread_shift_owner_tests::) |
test(/metadata_transfer_tests/) | test(/bookmark_hint_uses_composed/) |
test(/remote_zip_and_pdf_bookmark/) | test(/remote_bookmark_lists_preserve_hidden_anchor/) |
test(~app::tests::native_video_rating_key_tests::) |
test(/rating_hydration/) | test(/xmp_hydration/) | test(/tag_prewarm/) |
test(/rename_migration_waits/) | test(/book_resume_meter_rename/) |
test(/visibility_in_book_pc_projects_nested_archive_provenance_and_legacy_notice/) |
test(~metadata_cleanup::) | test(~delete_worker::) | test(~book_append_undo::) |
test(~rating_db::)
```

159 件と focused 65 件には重複がある。今回の範囲に未解決事項はない。
build-dev・本体起動・commit は行っていない。全件 gate の再実行は設計担当へ引き継ぐ。

## 9. 独立コードレビューと設計担当の確認 (2026-10-21〜22)

### 9.1 独立コードレビュー (Codex gpt-6.1-sol / xhigh、read-only、SID 01a15e87-c1b4-7ff0-a7d4-46a81162c979)

- 1 回目 (対象 6972a8534): P2 2 件・P3 1 件。P2-1 (保持している context の再照合) は以前からの挙動として §3.10 に記録し、
  バックログ §1.415 に起票 (直さない)。P2-2 (静止判定の UI スレッド DB I/O) と P3 (Remote のしおり hint) は修正 (a6a0b3a49)。
- 設計担当の本体 lib 全件 (6972a8534) で再現した 7 件の失敗を修正 (a6a0b3a49、§8.7。うち 2 件は製品の退行)、
  残る D2c を §3.10 で決定して修正 (0aef092b6、§8.8)。
- 2 回目 (対象 0aef092b6): 前回の指摘は解消 (P2-1 の保留判断も妥当)。新しい P2 1 件 (XMP ★ import が削除の purge 後に
  ★を復活させる。worker 化前からの競合) → fbed29360 で修正 (§8.9)。
- 3 回目 (対象 fbed29360): P1・P2・P3 なし、**受入可**。

### 9.2 設計担当の検証

- 本体 lib 全件 (`test-lib-full.ps1`): 6972a8534 で 7 件の失敗 (上記) とページめくり 15 件の失敗 (単独再実行で全件成功。
  負荷による失敗)。0aef092b6 で 14,028 件成功。最終コード (fbed29360、以後は文書のみ) の 1 回目は 1 件失敗 (`section1375_stack_flat_delete_rebuilds_retained_singleton_metadata_and_maps` がテスト内のファイル削除で Windows のエラー 32「別のプロセスが使用中」。このテストは未変更で、単独再実行は成功)、2 回目は **14,031 件成功**。
- 隔離実アプリ確認 (観測者 ClaudeCode、fbed29360 の `build-dev.ps1 -PreserveRuntime`、隔離 data-dir
  `target\dev-runtime\data-verify`、本棚を `target\verify-books` に向けた設定、合成 fixture):
  - §1.245: サブフォルダ 2 + 画像 3 の ZIP で p2 に付けたしおりが「ZIP・CBZ / 2 ページ」(DB の hint 1)。
    修正前に p1 に付けた行は「3 ページ」(hint 2) のまま (§5 のとおり直さない)。
  - §1.272: 同じ元名の 2 ページの本 T (A=0001_001.png、B=0002_001.png) で、A に★3 を付け、表示トリムの行を A のキーに
    置いてから、並べ替え画面で A を後ろへ移して保存 → 一覧の★3 は A (0002_001.png、5.0 KB) に付き、B (0001_001.png) には無い。
    DB では★と表示トリムの行がどちらも `.../t/0002_001.png` にあり、`0001_001.png` には無い。
    (表示トリムはページ単位の行だけでは本のトリムが有効にならないので、画面上の切り抜きは比べていない。)
  - §1.273: 製本管理から本 T を削除 → ★と表示トリムの行が DB から消えた。
  - 観察 (今回の変更とは別): 開いている本を削除した後、一覧に旧ページの行が残った (★表示は消えた)。削除後の
    再読込要求 (`pending_reload`) はコード上変更前と同じなので以前からの挙動とみているが、master のビルドでは未確認。
  - §1.244: 再現しない (§0)。
- master への取り込み (2026-10-22、利用者の指示、e0d096601。PDF 表示 124dc4828 の後): 本体 lib 全件を 3 回。
  1 回目・2 回目はそれぞれ別の 1 件だけが失敗 (`mutation_refresh_import_pin_snapshot_retires_only_superseded_materialization_owners`
  = 1 フレーム 3 ms の時間予算に依存するテスト、`video::tests::real_demux_seek_failure_retires_selection_base` = 今回変更なしの
  動画コード)。どちらも単独再実行と他の回では成功。3 回目は **14,126 件すべて成功**。

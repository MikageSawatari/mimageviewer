# 変換して開く書庫の改名・移動で、変換キャッシュ側のキーも移す (§1.886)

2026-11-24 設計担当 ClaudeCode Opus 5.5 (セッション conv-rename-886)。状態: **設計 v5 (独立設計レビュー r1〜r4 REVISE の指摘を反映)、r5 前**。
バックログの正本は [next-release-backlog.md](next-release-backlog.md) §1.886。前提として読む文書:
[edit-content-identity-plan.md](edit-content-identity-plan.md) §4 (変換キャッシュ ZIP の置き場と 4 面)、
[book-key-migration-plan.md](book-key-migration-plan.md) §3.4、`src/rename_key_migration.rs` のモジュール doc、
§1.885 の変更 (master 5d417a72d、ブランチ resume-identity-885)。

## 1. 現状 (コードから)

### 1.1 変換して開く書庫の行のキー

RAR・7z・LZH (拡張子 rar / cbr / 7z / cb7 / lzh / lha) と、非 ZIP の入れ子を含む ZIP (zip / cbz) は、
変換キャッシュの ZIP にしてから開く。キャッシュの置き場は元のパスだけの純関数
(`archive_cache::cache_zip_path_for_data_dir(data_dir, src)` = `<data_dir>/archive_cache/<h2>/<h>/<stem>.zip`、
`h = sha256(path_key::normalize(src))`、ドライブ文字を落として小文字化)。

キーは 2 系統ある (edit-content-identity-plan §4 の 4 面と同じ)。

| 面 | キー | 主な中身 |
| --- | --- | --- |
| 元の書庫 | `<src>`、`<src>::…` | 書庫自体の ★・タグ・代表サムネ pin、直接読みの RAR の行、内部本の `miv:zipbook:<src>…` 読書位置 |
| 変換キャッシュ | `cache(src)`、`cache(src)::…` | ページの編集 (補正・マスク・隠蔽・注釈・切り取り・回転)、ページの ★・タグ、本の読書位置 (`BookResumeKey::legacy(cache)`)、見開き、表示トリム、内容同一性の台帳 など、変換して開いた本の中のもの |

読書位置の `page_identity` は、キャッシュ側の行でも新しい記録は container が**元のパス** (論理パス)、
古い記録は container が `cache(src)`。`miv:zipbook:` の行はキーが元のパスで、identity の container は
今の保存処理では論理パス、古い記録では `cache(src)` (section1885 の変換書庫テストの 2 つの形)。

mIV の「名前の変更」(`rename_item.rs` → `shell_file_ops::run_rename_item`) は同じ親フォルダの中で名前だけを
変える (`new = old.parent().join(new_name)`)。この経路ではドライブ文字は変わらない。次の改名は
`rename_pending` (FS の改名の worker) が終わるまで受け付けない (`request_rename_dialog`) が、
移行 job (`rename_migration_queue`) の実行は待たない。

### 1.2 経路ごとの扱い

| 経路 | 元の書庫の面 | 変換キャッシュの面 |
| --- | --- | --- |
| 内容同一性の復元のコピー (`restore_copy_mappings` → `copy_stores_impl`) | 写す | **写す** (`ContentKind::Convertible` どうしなら cache の組を足す。§1.885 で identity も書き換え済み) |
| mIV の名前の変更・移動 (`spawn_rename_key_migration` → `run_at_with_shape`) | 移す | **移さない** ← §1.886 |
| 製本 worker (`migrate_book_paths_at`) | 移す | 対象外 (下の §2.4) |

名前の変更・移動の後に開くと、変換し直して `cache(new)` で探すので、ページの編集・読書位置などが
すべて見つからない。行は `cache(old)` に残る (元の名前に戻せば見える)。§1.885 のレビューで見つかった、
v5.7.0 より前からのリリース済みの穴 (いつからかは未調査) で、v5.7.0 の退行ではない。実機では未確認。

## 2. 設計

### 2.1 不変条件

- C1. 名前の変更・移動で元のパスが `old → new` になったとき、変換して開く書庫 `f` (それ自身、または
  フォルダの配下) について、`cache(f_old)` の面の行は同じ操作の中で `cache(f_new)` の面へ移る。
- C2. 移す処理は、ストアごとに 1 つの transaction の中で、元の面の組 `(old, new)` と同時に行う
  (§1.885 の「キーを変える操作は、同じトランザクションで identity も書き換える」と同じ形)。
- C3. 読書位置の identity は、元のパスと変換キャッシュのパスのどちらで記録されていても、
  移した後の行が新しい本のページを指す (`PreparedPageIdentityIndex::resolve` が Restored になる)。
- C4. 他の本・他の書庫の行には触れない。衝突の扱いは元の面と同じ (`DestinationWins`)。
  ただし、別ドライブの同じ相対パスにある書庫 (`C:\a\x.rar` と `D:\a\x.rar`) は、ドライブを落とすストアと
  変換キャッシュの面で、もともと同じ行を共有している (edit-content-identity-plan §4 の注意)。共有している行は
  改名した側へ移る。今のドライブを落とすストア (読書位置・見開き等) の元の面の扱いと同じで、この項目では変えない。
- C5. 組は FS の改名が確定し、改名の worker が配下を列挙した時点の配下で決まり、その後の改名・移行 job の実行の順序に
  左右されない (A → B の移行 job が走る前に B → C と改名しても、最後は `cache(C/f)` に行がある)。
  保証するのは、列挙の時点で B の配下にある書庫について。列挙の途中や後に、D&D・貼り付け・エクスプローラーで配下から
  外へ移した書庫は対象外。mIV の D&D・貼り付け・整理の移動は、もともと移した項目の path キーの設定を引き継がない
  (`ui_dialogs/file_organize.rs` の結果処理「No removal, metadata migration」、`main_drop.rs` の移動も同じ) ので、
  外へ移した書庫の行は、cache の組の有無に関係なく今と同じく新しい場所へは付かない。
- C6. 旧パスの再利用との順序は、元の組 `(old, new)` と同じに扱う。改名から移行 job の generic 段までの間に、
  別の項目が旧パスを再利用し (D&D・貼り付け・エクスプローラーで同じ名前の別フォルダを置く等)、そこで新しく保存した行は、
  今の元の組の移行 (`migrate_store_family` は実行時点の旧キーの行をすべて動かす) と同じく、移行 job が新側へ動かす。
  cache の組はこの性質を `cache(old)` のキーへ広げるが、新しい所有・順序の規則は足さない。旧パスの再利用を移行の確定まで
  止める境界は、元の組も含めた改名移行全体の問題として §5 の別項目にする (独立設計レビュー r4)。

### 2.2 組の作り方 (FS の改名を確定した worker で配下を記録し、job が組にする)

配下の変換書庫の一覧は、**FS の改名に成功した同じ worker** (`rename_item_async` の worker。`rename_pending` が
App に残っている間) が、結果を App へ返す前に FS から作る。DB は読まない (行の有無で絞らない)。App は一覧を
`PathMigrationJob` に記録し (journal にも書く)、移行 job はそこから組を作る。移行 job は FS を読まない。

- **ファイル** (`tree == false`): 一覧は作らない。job は、`old` の拡張子が `ArchiveFormat::nested_from_extension` で
  認識できる (rar / cbr / 7z / cb7 / lzh / lha / zip / cbz) なら、組 `(cache(old), cache(new))` を 1 つ作る。
- **フォルダ** (`tree == true`): 改名後の `new` を再帰で列挙し (`read_dir` + `entry.file_type()`、シンボリックリンク・
  ジャンクションはたどらない、`Path::is_dir()` を使わない)、上と同じ拡張子のファイルの `new` からの相対パスの一覧
  `converted_archive_rels` を作る。job は各 `rel` について組 `(cache(old.join(rel)), cache(new.join(rel)))` を作る。
- 行の有無による省略は、移行 job の各ストアの transaction の中で行う (§2.3)。先に積まれた job の移行と、
  移行の開始前に待つ保存 worker の書き込みが済んだ後の DB を見るので、続けての改名・未保存の書き込みと順序が食い違わない。
- 列挙の件数と時間を `perf::event` に残す。
- journal に記録するので、起動時の再実行も同じ一覧を使う。改名から移行 job の実行までの間に FS が変わっても
  一覧は変わらない (C5)。A → B → C と続けて改名すると、job 1 が `cache(A/f) → cache(B/f)`、job 2 が
  `cache(B/f) → cache(C/f)` を順に移す (job 2 の一覧は B → C の改名の時点の C の配下から作られ、DB の状態に依らない)。
- **改名の worker が走っている間 (FS の改名と列挙) は、FS を変える他の操作を受け付けない。**
  `admit_rename_migration_source_change` (削除・本の改名・本の削除・製本の構造変更・追加の取り消しの共通の入口) が
  `rename_pending` の間は「名前の変更が完了するまでお待ちください」で断る。これで「列挙中に B を削除し、削除の結果が
  改名の結果より先に採用され、後から積まれた job が削除済みの B に行を作り直す」順序が無くなる。改名の結果を採用して
  job を積んだ後の削除は、今と同じ `invalidate_rename_migrations_for_removed_paths` が job の generic 段を止める
  (generic 段を止めた job は cache の組も移さない)。
  D&D・貼り付け・整理の移動 (`shell_transfer_admission` 側) には `rename_pending` の拒否を足さない。これらは移した項目の
  設定を引き継がない操作で、エクスプローラーでも同じ変更がいつでも起こるため、止めても C5 の保証は広がらず、受付が
  止まる操作だけが増える (C5 の保証範囲を参照)。

### 2.3 移す処理 (ストアごとに 1 transaction)

元の組と、job に記録した一覧から作った cache の組をまとめた組の一覧 `pairs = [(old, new)] + cache_pairs` を、各移行へ渡す。

- 各ストアの transaction の最初に、そのストアの変換キャッシュの根 (`<data_dir>/archive_cache/` にそのストアの正規化を
  かけたもの) の下のキーを範囲で 1 回読み、`cache(src)` の部分 (`::` と `/` より前) の集合を作る。book_resume は
  identity の container も同じ走査で集める。集合に旧側が無い cache の組はそのストアでは飛ばす (組ごとに DB を
  走査し直さない)。

- 汎用の `STORES` (`migrate_store`): 1 回の transaction の中で、組ごとに `migrate_store_family`
  (exact・`/`・`::`、`DestinationWins`) を行う。rating の `source_path` の導出、回転の変更キーの記録、
  `record_rows_changed` も組ごとに今と同じに行う。
- 読書位置 (`migrate_resume_store`): 移す前の宛先の空きの確認・移す・identity の書き換えを、全部の組について
  1 回の transaction で行う。identity の書き換えに使う対応は、元の組と、その行を移した組の両方
  (`resume_source_mappings` に `(old,new)` と `(cache(f_old), cache(f_new))` を入れる)。
  `migrate_archive_resume_rows` にも `cache_pairs` を含めた組を渡し、`miv:zipbook:` 行の identity
  (container が論理パス `f_old` でも `cache(f_old)` でも) を書き換える。キーの書き換えは論理パスでだけ一致するので、
  cache の組が `miv:zipbook:` のキーを動かすことはない。通常の行と `miv:zipbook:` 行は、今と同じく別々の経路で
  1 回ずつ書き換える。
- キーが動かない組 (ストアの正規化で `old_key == new_key`) でも、identity の書き換えは別に判断する。
  キーの移動を飛ばしたときは、その組のキーの family (exact・`/`・`::`) の行のうち、identity が組の旧側を指すものを
  同じ transaction で新側へ書き換える。名前の変更の経路ではドライブ文字は変わらない (§1.1) ので、ここに当たるのは
  大文字小文字だけの改名で、正規化した対応では旧側と新側が等しく書き換えも起きない。将来ドライブの変わる経路が
  この関数を通るときのための規則として置く。
- 専用の移行 (`book_bookmarks::migrate_paths_at`、`book_composition::migrate_page_paths_at`、
  `spread_shift::migrate_page_paths_at`) は組の配列を受け取るので `pairs` を渡す。`migrate_book_bookmarks`
  は配列を受けるように変える。閲覧履歴 (`migrate_reading_history`、exact) も組ごとに行う。
- `invalidate_root_family` を cache の組の両側にも呼ぶ。
- サイドカー (`.xmp`) と PDF パスワードは元の組だけ (キャッシュ側には無い)。

### 2.4 対象外とした経路

- **製本 worker** (`migrate_book_paths_at`): 本のページは画像だけ (book-key-migration-plan §1.3)。
  本のフォルダに変換書庫が入る通常の経路は無いので、cache の組を足さない。
- **内容同一性の復元のコピー**: 元と先がどちらも変換書庫 (`ContentKind::Convertible`) の候補は、§1.885 までで 4 面を 1 transaction で写し、
  identity も書き換えている (既存テスト `convertible_restore_copies_all_four_faces_without_archive_cache_db`・
  `section1885_converted_archive_batch_rewrites_logical_and_cache_identity_paths`)。変更しない。
  入れ子を含む ZIP は `ContentKind::Zip` に分類され cache の組を足していないが、台帳の記録と復元候補の
  検出がキャッシュ側の面をどう扱うかの調査が要るので、この項目では扱わない (§5 の別項目候補)。
- **変換キャッシュの ZIP 自体と `archive_cache.db`**: 動かさない。改名後に開くと今までどおり変換し直し、
  予測どおり `cache(new)` に作られる (edit-content-identity-plan §4 と同じ考え方)。古いキャッシュは
  「元ファイルが無いもの」として管理画面で消せる。

### 2.5 本体 (App) 側

一覧は FS の改名の結果 (`ShellRenameOutcome` 相当) に入って App に届き、job にも入る。cache の組は
`old`・`new`・一覧から純関数で作る (UI スレッドでは FS も DB も読まない)。

- **cache の組は 1 回の走査でまとめて当てる。** 今の helper (`rewrite_page_edit_presence_for_rename` 等) は 1 組ごとに
  集合・map・表示キャッシュの全体を走査するので、組ごとに呼ぶと候補数 × キー数の処理が UI スレッドで走る。cache の組は
  `cache(old)` → `cache(new)` の対応表 (各ストアと同じ正規化のキー) にまとめ、各集合・map・表示キャッシュを 1 回だけ走査し、
  キーの `cache(src)` の部分 (`::` より前) を取り出して表を引く。元の組は今の helper のまま。
- 改名の成功を受けたとき (`poll_rename_pending`): 元の組に今行っている `rewrite_page_edit_presence_for_rename` の付け替えを、
  cache の組にも上の 1 回の走査で行う (同じ時点)。
- 移行 job の完了を受けたとき (`poll_rename_migration_pending`): 移行の報告 (`RenameMigrationReport`) に、どれかのストアで
  行が動いた cache の組だけを入れる。元の組に今行っている in-memory の引き直し (presence の付け替え、
  `rating_session_writes` の付け替え、`refresh_edit_preview_thumbnails_after_rename`、`book_resume_meters.remove_scopes`)
  を、その組に対して上の 1 回の走査で行う。★・タグの表示キャッシュの全消去と `reload_book_resume_meters` は今のまま 1 回。
- context の編集状態の読み直し: cache の組で対象になる context (今 `cache(old)`・`cache(new)` の ZIP の中を
  表示しているもの) は、UI スレッドでは対象の判定だけを行い、読み直しは既存の非同期の準備
  (`prepare_page_edits_for_current_virtual_items` → worker) へ渡す。同期で DB を読む
  `rehydrate_page_edit_state_for_current_items` には cache の組から入らない。ZIP の中の一覧の列挙中で準備がまだ無い
  context は、列挙の完了で始まる通常の準備に任せる。移行の書き込みは `PAGE_EDIT_WRITES` の epoch の中で行うので、
  その準備の読み取りとの前後は既存の epoch の再試行が扱う (実装担当が、列挙中に移行が終わる順序で古い DB を
  採用しないことをコードで確かめ、成り立たなければ設計担当へ返す)。元の組の既存の読み直しは変えない。

### 2.6 まれな失敗

既定の割り切り (ログ + 通知)。

- 改名の worker でのフォルダの列挙に失敗した部分 (読めない下位フォルダ) は飛ばし、ログに残して、改名の成功後に
  「一部の設定を引き継げませんでした」系の警告を 1 回出す (改名自体は成功として扱う)。
  行は `cache(old)` に残るだけで消えない。
- ストアの失敗は今と同じ (ストアごとにロールバックし、エラーを集計して通知)。
- 移行先 `cache(new)` に以前の別の書庫の行が残っていた場合 (同じパスの変換書庫を消した後、そこへ改名した等) は
  `DestinationWins` で残っている行が勝つ (元の面と同じ)。
- データ ディレクトリを別の場所へコピーして、古い場所のキャッシュ ZIP の絶対パスで開き続けている場合
  (`book_resume_meter.rs` の注記) は、予測したパスと一致しないので移らない。復元のコピーと同じ限界として割り切る。

### 2.7 永続データ

ストアの形とキーの体系は変えない。改名移行の journal (`PathMigrationJob`、リリース済み) に、配下の変換書庫の
相対パスの一覧の欄を足す。`#[serde(default)]` で、欄の無い古い journal は空の一覧として読む (古い版で積んだフォルダの
job は今までどおり配下の cache の面を移さないだけ)。旧形式 (tuple) の読み込み (`parse_journal_bytes`) と
`shell_rename` など既存の constructor は空の一覧を作る。古い版は知らない欄を無視して読める
(`deny_unknown_fields` は使っていない) が、古い版が書き直すと欄は消える (互換は「読める」範囲)。移行コードは要らない。既に `cache(old)` に取り残されている行
(リリース済みの版で改名・移動した書庫の行) は一括で直さない (§1.885 の判断と同じ。キーから元のパスを
逆算できない)。

### 2.8 状態の組み合わせを減らす検討 (CLAUDE.md)

- 組を移行 job の中で FS から作り直す案 (設計 v1) は採らない。移行 job の実行を待たずに次の改名を受け付けるので、
  A → B → C の改名で job 1 が B を見られず、行が `cache(A/f)` に残る (独立設計レビュー r1 P1)。改名を確定した
  worker で組を決めて記録すれば、確定の時点が 1 つになり、job の実行順・起動時の再実行と組み合わさらない。
- 移行 job が終わるまで次の改名を止める案は採らない。今の連続した改名の操作が遅くなる (リリース済みの挙動の退行)。
- 改名の worker で DB を読んで候補を絞る案 (設計 v2) は採らない。DB の状態は先の job・保存の反映前のことがあり、
  続けての改名で組が落ちる (独立設計レビュー r2 P1)。列挙は FS だけで決め、行の有無は移行の transaction で見る。
- その代わり、**フォルダの改名は、配下の列挙の時間だけ改名の完了 (と次の改名・削除の受付) が遅れる**。変換書庫を
  使わない利用者も対象になる。通常の大きさのフォルダでは短い見込み (未計測) だが、大きなフォルダ・NAS では目立ち得る。
  `perf::event` で測り、実機の確認で遅れが目立てば列挙の扱いを見直す。これは「通常の使い方が遅くなる」変更に当たり得る
  ので、統括・利用者の判断を仰ぐ項目として報告する。
- 変換書庫のキーを元のパスへ寄せる (キャッシュの面を無くす) 案は、リリース済みのデータの移行と、
  変換・読み取り・保存の全経路の変更が要るので採らない。
- フォルダ配下の変換書庫を `archive_cache.db` から逆引きする案は採らない。容量上限での自動削除や
  管理画面での削除でキャッシュの行が消えても、ページの編集・読書位置は `cache(src)` のキーで残るため、
  逆引きに漏れが出る。FS の列挙は「行がある時だけ」に絞って費用を抑える。

## 3. テスト (実装担当)

`src/rename_key_migration/resume_identity_tests.rs` の helper (`record_position`・`assert_restored`、
本を開くときと同じ materializer と `PreparedPageIdentityIndex::resolve`) を使う。名前は `section1886_` で始める。

1. RAR・7z・LZH・入れ子の ZIP の**ファイルの改名**: `BookResumeKey::legacy(cache(old))` の行が `cache(new)` へ移り、
   identity が論理パス・cache パスのどちらで記録されていても Restored になる。`cache(old)` には残らない。
2. **親フォルダの改名** (tempdir に実ファイルを置き、組を作る関数 → `run_at_with_shape(tree=true)` の順):
   配下の変換書庫 2 冊 (うち 1 冊は下位フォルダ) の読書位置が Restored。配下の変換書庫でない ZIP と、
   フォルダの外の書庫の行は変わらない。別ドライブの同じ相対パスの書庫は、C4 の記述どおり共有行が改名側へ移ることを確かめる。
3. `miv:zipbook:` 行が、identity の container が論理パスの場合と `cache(old)` の場合の両方で、ファイル・フォルダの
   改名の後に Restored になり、identity の書き換えが 1 回だけ (二重に書き換えない)。
4. 読書位置以外の面: 補正 (`adjustment.db` の `cache(old)::entry`)、ページの ★ (rating の `source_path` の導出を含む)、
   見開き (`spread.db`)、本のしおり (`book_bookmarks.db`) が `cache(new)` へ移る。
5. 大文字小文字だけの改名で、cache の組の移動が no-op になり、エラーが無く、読書位置が Restored のまま。
6. 改名の worker の一覧は DB の状態に依らない: 変換キャッシュの根の下に行が 1 つも無くても、フォルダの改名で配下の
   変換書庫の相対パスが一覧に入る。移行 job では、ストアの集合に旧側が無い cache の組をそのストアで飛ばし
   (ストアごとの範囲読みは 1 回)、行が無いので何も変わらない。
7. `cache(new)` 側に既存の行 (読書位置・補正) があるとき、`DestinationWins` でその page・identity・編集内容が変わらない。
8. **続けての改名** (handler test、C5): フォルダ A → B の改名の結果を、本番の一覧を作る関数 (改名の worker と同じ関数を
   tempdir の実フォルダに対して呼ぶ) で作って受け、job を積み、job を走らせる前に FS で B → C と改名して同じ関数で
   2 つ目の結果を作って受ける。2 つの job を順に走らせた後、`cache(C/f)` の読書位置が Restored で、補正も `cache(C/f)` にあり、
   `cache(A/f)`・`cache(B/f)` に残らない。保存 worker に `cache(A/f)` への未完了の書き込みを持たせた形も 1 本置き、
   その書き込みも最後に `cache(C/f)` にあることを確かめる。
9. journal: 一覧を持つ job が保存・読み込みで往復し、欄の無い古い journal と旧形式 (tuple) も読める。
   generic 段を止めた job (`generic_pending=false`) は、journal の往復の後も cache の面を移さず、collection 段には
   元の論理 mapping だけを渡す。
10. **改名中の削除** (handler test): `rename_pending` がある間の削除の要求は断られる (`delete_pending` が作られない)。
    改名の結果の採用の後に積んだ job は、B の削除で generic 段が止まり、`cache(B/f)` に行を作らない。
11. 本体側: 改名の成功を受けたときと移行 job の完了を受けたときに、`adjusted_page_keys` などの in-memory の集合で
    `cache(old)::…` が `cache(new)::…` へ付け替わる (handler test)。`cache(new)` の ZIP の中を表示中の context は
    非同期の準備で読み直され、同期の `rehydrate_page_edit_state_for_current_items` を通らない。準備の読み取りが移行の前に
    終わり、その後に移行が完了し、最後に準備の結果を採用する順序で、古い結果が採用されない (epoch の検査で棄てて読み直す)。
    候補が多数 (数百) で、無関係な編集キーも多数ある状態で、presence・`rating_session_writes`・表示キャッシュの走査が
    候補数に比例して増えない (走査回数を数える)。報告に入る cache の組は行が動いたものだけ。
12. 既存の section1885・rename_key_migration・content_identity::restore のテストが通る。

流す filter (run-heavy 経由の nextest): `section1886`・`section1885`・`rename_key_migration`・`book_resume`・
`content_identity::restore`・`rar_archive_cache`・`rename_migration`・`rename_item`。

## 4. レビュー記録

(独立レビューの記録は、設計担当がレビュー担当の出力から書く。実装担当は書かない。)

- **設計 r1** (Codex gpt-6.1-sol / xhigh、read-only、設計とは別の会話、SID 01a20dfd-0b50-7700-8f1f-27f96bf1d2d3、
  2026-11-24): **REVISE**。
  - P1: 移行 job の中で改名後の FS から組を作ると、job の実行前に B → C と改名したとき B が無く、行が `cache(A/f)` に残る。
    → 採用。組を FS の改名を確定した worker で作り、job と journal に記録する形に変えた (§2.2・§2.7・§2.8、テスト 8・9)。
  - P2: キーが同じ組でも identity の書き換えは要る (例: ドライブだけの移動)。→ 規則として採用 (§2.3)。ただし
    名前の変更の経路は同じ親フォルダの中で名前だけを変えるのでドライブは変わらない (§1.1、`run_rename_item`)。
    テストは大文字小文字だけの改名で Restored を確かめる形にした (テスト 5)。
  - P2: 別ドライブの同じ相対パスの書庫と共有している行の扱いが未決定。→ 既存のドライブを落とすストアと同じく
    改名側へ移ると C4 に明記し、テスト 2 で確かめる。
  - P3: `miv:zipbook:` 行の identity は論理パスの形もある。→ §1.1 とテスト 3 を直した。
  - そのほか: 候補ごとの DB 照会をやめ、ストアごとに 1 回の範囲読みにした。「追加費用ゼロ」を直した。
    `rehydrate_contexts_after_rename_migration` は同期の DB 読み取りを含むので、組をまとめて 1 回にした (§2.5)。
    復元のコピーが正しいのは `Convertible` どうしの範囲だと §2.4 に限定した。DestinationWins の回帰テストを足した (テスト 7)。
- **設計 r2** (同じ会話、read-only、2026-11-24): **REVISE**。
  - P1: 改名の worker で DB の行で候補を絞ると、行がまだ `cache(A/f)` にある間の B → C の改名で組が落ちる。未保存の書き込みも
    見えない。→ 採用。改名の worker は FS の列挙だけで相対パスの一覧を記録し、行の有無は移行の transaction で見る (§2.2・§2.3)。
    DB による列挙の省略もやめた。テスト 8 を本番の関数で 2 回とも作る形と、未完了の保存を挟む形にした。
  - P2: 長くなった `rename_pending` の間に削除を受け付けると、削除の結果が先に採用され、後から積んだ job が消した B に
    行を作り直す。→ 採用。`admit_rename_migration_source_change` が `rename_pending` の間は断る (§2.2、テスト 10)。
  - P2: context ごとに 1 回にしても、snapshot の無い context は同期の DB 読み取りに入る。→ 採用。cache の組では
    非同期の準備へ渡し、同期経路に入らない (§2.5、テスト 11)。
  - journal の欄の互換は「読める」範囲と書き直し、旧形式と constructor が空の一覧を作ることを明記した (§2.7)。
    改名の遅れは未計測の見込みと書き、判断を仰ぐ項目にした (§2.8)。
- **設計 r3** (同じ会話、read-only、2026-11-24): **REVISE** (P1 なし)。
  - P2: D&D・貼り付けの移動は `admit_rename_migration_source_change` を通らないので、列挙中に配下の書庫を移せる。
    → 受付の拒否は足さず、C5 の保証範囲を「列挙の時点の配下」と明記した (§2.1・§2.2)。理由: mIV の D&D・貼り付け・整理の
    移動は、もともと移した項目の path キーの設定を引き継がない (`file_organize.rs` の結果処理) ので、外へ移した書庫の行は
    cache の組の有無に関係なく新しい場所に付かない。エクスプローラーでも同じ変更は起こり、止めても保証は広がらない。
    r4 でこの判断の妥当性を確かめてもらう。
  - P2: cache の組ごとに今の helper を呼ぶと、UI スレッドで候補数 × キー数の走査になる。→ 採用。cache の組は対応表に
    まとめて 1 回の走査で当て、移行後は行が動いた組だけを報告に入れる (§2.5、テスト 11)。
  - P3: テスト 6 が v2 の仕様のまま。→ 直した。
  - テスト 11 に「準備の読み取り完了 → 移行完了 → 結果の採用」の順序を明記した。
- **設計 r4** (同じ会話、read-only、2026-11-25): **REVISE** (P1 なし、P2 1 件)。D&D・貼り付けを止めない判断と C5 の
  保証範囲は妥当 (r3 の一般的な受付拒否の要求は撤回)。UI の一括走査・テスト 6・11 の修正も妥当。
  - P2: 改名から移行 job までの間に旧パス `P/A/f.rar` を別の書庫が再利用し、そこで保存した補正を、遅れて走る移行 job が
    `cache(P/B/f.rar)` へ移して旧キーから消す。→ 設計担当の判断: この順序は元の組 `(old, new)` の汎用移行に既にある
    性質で (`migrate_store_family` は実行時点の旧キーの行を動かす。`P/A/x.jpg` の補正でも同じ)、§1.886 で新しく生まれる
    ものではない。§1.886 では元の組と同じ扱いにし (C6)、旧パスの再利用を移行の確定まで止める境界は、改名移行全体の
    別項目として §5 に記録した。既存の操作を待たせる変更になるので、統括・利用者へ判断を戻す。r5 でこの扱いを確かめてもらう。

## 5. 別項目の候補 (この項目では直さない)

- 入れ子を含む ZIP の内容同一性の復元で、変換キャッシュ側の面を写すか (§2.4)。
- mIV 内の削除の purge (`purge_removed_scopes_impl`) が、変換書庫の `cache(src)` の面を消さないこと。
  消した書庫と同じパスに後で別の変換書庫を置くと、古い編集が付く (`rename_key_migration.rs` に
  キャッシュのパスを作る箇所が `copy_resume_store` しか無いことからの見当。未確認)。
- 改名・移動の後に、変換キャッシュの ZIP 自体を移して変換し直しを省くこと。
- 改名から移行 job の generic 段までの間に旧パスを別の項目が再利用し、そこで新しく保存した行を、移行 job が新側へ
  動かしてしまうこと (元の組も cache の組も同じ。C6)。直すなら、旧パスとその配下への新しい閲覧・保存を、該当する
  移行の確定まで受け付けない境界が要り、既存の操作を待たせることになるので、統括・利用者の判断が要る。
  移行 job は通常、改名の直後 (保存 worker が空いた時点) に始まるので、窓は短い (未計測)。

# 変換して開く書庫の改名・移動で、変換キャッシュ側のキーも移す (§1.886)

2026-11-24 設計担当 ClaudeCode Opus 5.5 (セッション conv-rename-886)。状態: **設計 v1、独立設計レビュー前**。
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
`cache(src)` (section1885 の変換書庫テストの形)。

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

### 2.2 組の作り方 (worker 内で、journal は変えない)

`run_at_with_epub_acquisition` の入口で、元の組 `(old, new)` から変換キャッシュの組の一覧 `cache_pairs` を作る。

- **ファイル** (`tree == false`): `old` の拡張子が `ArchiveFormat::nested_from_extension` で認識できる
  (rar / cbr / 7z / cb7 / lzh / lha / zip / cbz) なら、組 `(cache(old), cache(new))` を 1 つ足す。FS は読まない。
- **フォルダ** (`tree == true`):
  1. 先に「変換キャッシュの根 (`<data_dir>/archive_cache/`) の下をキーにした行が、どれかのストアに
     1 行でもあるか」を範囲の `LIMIT 1` で見る (book_resume は identity の container も見る)。
     無ければ何もしない (RAR を使わない利用者は追加費用ゼロ)。
  2. あれば、改名後の `new` を再帰で列挙する (`read_dir` + `entry.file_type()`、シンボリックリンク・
     ジャンクションはたどらない、`Path::is_dir()` を使わない)。上と同じ拡張子のファイル `f_new` ごとに
     `f_old = old.join(f_new.strip_prefix(new))` として組 `(cache(f_old), cache(f_new))` を作る。
  3. ストアの中に `cache(f_old)` の面の行 (book_resume は identity の container 参照も) があるものだけを残す。
     比べるのは各ストアの正規化 (`StoreDescriptor::normalize_path`) をかけた文字列。
- 列挙は移行 worker の上で、ストアの保守 lease を取る前に行い、DB には書かない。
  件数と時間を `perf::event` に残す。
- `cache(old) == cache(new)` になる組 (大文字小文字だけの改名、ドライブ文字だけの違い) はストアごとの正規化で
  等しくなるので、既存の `old_key == new_key` の判定で飛ばす。
- journal (`PathMigrationJob`) の形は変えない。起動時の再実行では同じ入力 (改名後の FS と `data_dir`) から
  同じ組が作り直される。

### 2.3 移す処理 (ストアごとに 1 transaction)

元の組と `cache_pairs` をまとめた組の一覧 `pairs = [(old, new)] + cache_pairs` を、各移行へ渡す。

- 汎用の `STORES` (`migrate_store`): 1 回の transaction の中で、組ごとに `migrate_store_family`
  (exact・`/`・`::`、`DestinationWins`) を行う。rating の `source_path` の導出、回転の変更キーの記録、
  `record_rows_changed` も組ごとに今と同じに行う。
- 読書位置 (`migrate_resume_store`): 移す前の宛先の空きの確認・移す・identity の書き換えを、全部の組について
  1 回の transaction で行う。identity の書き換えに使う対応は、元の組と、その行を移した組の両方
  (`resume_source_mappings` に `(old,new)` と `(cache(f_old), cache(f_new))` を入れる)。
  `migrate_archive_resume_rows` にも `cache_pairs` を含めた組を渡し、`miv:zipbook:` 行の identity
  (container が `cache(f_old)`) も書き換える。キーの書き換えは論理パスでだけ一致するので、cache の組が
  `miv:zipbook:` のキーを動かすことはない。
- 専用の移行 (`book_bookmarks::migrate_paths_at`、`book_composition::migrate_page_paths_at`、
  `spread_shift::migrate_page_paths_at`) は組の配列を受け取るので `pairs` を渡す。`migrate_book_bookmarks`
  は配列を受けるように変える。閲覧履歴 (`migrate_reading_history`、exact) も組ごとに行う。
- `invalidate_root_family` を cache の組の両側にも呼ぶ。
- サイドカー (`.xmp`) と PDF パスワードは元の組だけ (キャッシュ側には無い)。

### 2.4 対象外とした経路

- **製本 worker** (`migrate_book_paths_at`): 本のページは画像だけ (book-key-migration-plan §1.3)。
  本のフォルダに変換書庫が入る通常の経路は無いので、cache の組を足さない。
- **内容同一性の復元のコピー**: 変換書庫どうし (`Convertible`) は §1.885 までで 4 面を 1 transaction で写し、
  identity も書き換えている (既存テスト `convertible_restore_copies_all_four_faces_without_archive_cache_db`・
  `section1885_converted_archive_batch_rewrites_logical_and_cache_identity_paths`)。変更しない。
  入れ子を含む ZIP は `ContentKind::Zip` に分類され cache の組を足していないが、台帳の記録と復元候補の
  検出がキャッシュ側の面をどう扱うかの調査が要るので、この項目では扱わない (§5 の別項目候補)。
- **変換キャッシュの ZIP 自体と `archive_cache.db`**: 動かさない。改名後に開くと今までどおり変換し直し、
  予測どおり `cache(new)` に作られる (edit-content-identity-plan §4 と同じ考え方)。古いキャッシュは
  「元ファイルが無いもの」として管理画面で消せる。

### 2.5 本体 (App) 側

移行 worker の報告 (`RenameMigrationReport`) に、実際に作った `cache_pairs` を足す。
`poll_rename_migration_pending` は、今は元の組に対して行っている in-memory の引き直し
(`rewrite_page_edit_presence_for_rename`、`rating_session_writes` の付け替え、
`refresh_edit_preview_thumbnails_after_rename`、`rehydrate_contexts_after_rename_migration`、
`book_resume_meters.remove_scopes`) を、cache の組にも同じ順で行う。★・タグの表示キャッシュの全消去と
`reload_book_resume_meters` は今のまま 1 回。

### 2.6 まれな失敗

既定の割り切り (ログ + 通知)。

- フォルダの列挙で読めない下位フォルダがあれば、そこを飛ばし、`report.errors` に 1 件足す
  (既存の「名前変更に伴う設定の引き継ぎに一部失敗しました」の通知に乗る)。行は `cache(old)` に残るだけで消えない。
- ストアの失敗は今と同じ (ストアごとにロールバックし、エラーを集計して通知)。
- 移行先 `cache(new)` に以前の別の書庫の行が残っていた場合 (同じパスの変換書庫を消した後、そこへ改名した等) は
  `DestinationWins` で残っている行が勝つ (元の面と同じ)。
- データ ディレクトリを別の場所へコピーして、古い場所のキャッシュ ZIP の絶対パスで開き続けている場合
  (`book_resume_meter.rs` の注記) は、予測したパスと一致しないので移らない。復元のコピーと同じ限界として割り切る。

### 2.7 永続データ

ストアの形・キーの体系・journal の形は変えない。移行は要らない。既に `cache(old)` に取り残されている行
(リリース済みの版で改名・移動した書庫の行) は一括で直さない (§1.885 の判断と同じ。キーから元のパスを
逆算できない)。

### 2.8 状態の組み合わせを減らす検討 (CLAUDE.md)

- 改名の入口でキャッシュの組まで journal に書く案は採らない。組は改名後の FS と `data_dir` から決まるので、
  worker の中で毎回作れば journal の形も再実行の扱いも今のまま 1 つで済む。
- 変換書庫のキーを元のパスへ寄せる (キャッシュの面を無くす) 案は、リリース済みのデータの移行と、
  変換・読み取り・保存の全経路の変更が要るので採らない。
- フォルダ配下の変換書庫を `archive_cache.db` から逆引きする案は採らない。容量上限での自動削除や
  管理画面での削除でキャッシュの行が消えても、ページの編集・読書位置は `cache(src)` のキーで残るため、
  逆引きに漏れが出る。FS の列挙は「行がある時だけ」に絞って費用を抑える。

## 3. テスト (実装担当)

`src/rename_key_migration/resume_identity_tests.rs` の helper (`record_position`・`assert_restored`、
本を開くときと同じ materializer と `PreparedPageIdentityIndex::resolve`) を使う。名前は `section1886_` で始める。

1. RAR・7z・LZH・入れ子の ZIP の**ファイルの改名** (`run_at`): `BookResumeKey::legacy(cache(old))` の行が
   `cache(new)` へ移り、identity が論理パス・cache パスのどちらで記録されていても Restored になる。
   `cache(old)` には残らない。
2. **親フォルダの改名・別ドライブへの移動** (`run_at_with_shape(tree=true)`、tempdir に実ファイルを置く):
   配下の変換書庫 2 冊 (うち 1 冊は下位フォルダ) の読書位置が Restored。配下の変換書庫でない ZIP と、
   フォルダの外の同名の書庫の行は変わらない。
3. `miv:zipbook:` 行 (container が `cache(old)`) が、ファイル・フォルダの改名の両方で Restored になり、
   identity の書き換えが 1 回だけ (二重に書き換えない)。
4. 読書位置以外の面: 補正 (`adjustment.db` の `cache(old)::entry`)、ページの ★ (rating の `source_path` の導出を含む)、
   見開き (`spread.db`)、本のしおり (`book_bookmarks.db`) が `cache(new)` へ移る。
5. 大文字小文字だけの改名・ドライブ文字だけの違いで、cache の組が no-op になりエラーも出ない。
6. 変換キャッシュの根の下に行が無いとき、フォルダの改名で FS の列挙をしない (列挙の回数を数える test hook で確かめる)。
7. 本体側: 移行の完了を `poll_rename_migration_pending` が受けたとき、`adjusted_page_keys` などの
   in-memory の集合で `cache(old)::…` が `cache(new)::…` へ付け替わる (handler test)。
8. 既存の section1885・rename_key_migration・content_identity::restore のテストが通る。

流す filter (run-heavy 経由の nextest): `section1886`・`section1885`・`rename_key_migration`・`book_resume`・
`content_identity::restore`・`rar_archive_cache`・`rename_migration`。

## 4. レビュー記録

(独立設計レビューの結果は、設計担当がレビュー担当の出力からここへ書く。)

## 5. 別項目の候補 (この項目では直さない)

- 入れ子を含む ZIP の内容同一性の復元で、変換キャッシュ側の面を写すか (§2.4)。
- mIV 内の削除の purge (`purge_removed_scopes_impl`) が、変換書庫の `cache(src)` の面を消さないこと。
  消した書庫と同じパスに後で別の変換書庫を置くと、古い編集が付く (`rename_key_migration.rs` に
  キャッシュのパスを作る箇所が `copy_resume_store` しか無いことからの見当。未確認)。
- 改名・移動の後に、変換キャッシュの ZIP 自体を移して変換し直しを省くこと。

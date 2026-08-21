# A3a: 復元のコピーエンジン (編集内容の復元 Phase 1 の第 3 段・前半)

**正本は [docs/edit-content-identity-plan.md](../edit-content-identity-plan.md)。**
着手前に全文を読むこと。特に §4 (変換アーカイブの 4 面キー)、§8.1 (コピー実処理)、
§8.2 (復元後の後始末)、§9 (テスト)。
A1 (台帳と記録) と A2 (検出) は実装済み。`src/content_identity.rs` と
`src/app/content_identity_detection.rs` を読むこと。

## 1. A3 を 2 つに割る

設計書 §10.1 の A3 は 1 差分だと大きすぎるので前後半に割る。**これは前半。**

| | 中身 |
| --- | --- |
| **A3a (この文書)** | `STORES` 駆動の `copy_store` / `copy_exact` / `copy_prefix`、変換アーカイブの 4 面キー、`restore_declined` への書き込み API、復元後の後始末。**UI 無し。** |
| A3b (次) | 非モーダル復元ウィンドウ (§6)、A2 が保持している候補との配線、`restore_declined` を実際に書く操作 |

**A3a は呼び出し元がテストしかない状態で着地する。** これは意図的で、コピーの正しさ
(21 ストア・4 面キー・ドライブ文字除去) を UI と切り離して確認するため。A3b は直後に出す。
**そのため、公開する入口関数はテストから完全に駆動できる形にすること**
(App のハンドルや egui context を要求しない)。

## 2. `copy_store` は `STORES` を共有する

既存のコピー実装 2 つはどちらもそのまま使えない (§8.1):

- `App::copy_book_page_edit_key` ([app.rs:30831](../../src/app.rs:30831)) — 型付き 8 ストア横断だが
  **App のハンドル経由 = UI スレッド前提**で worker から呼べない。
- `rename_key_migration::migrate_store` ([rename_key_migration.rs:715](../../src/rename_key_migration.rs:715))
  — **DB ファイルを直接開く worker 実装**で `STORES` 21 件を網羅。

**後者と同じ表を使う兄弟として `copy_store` / `copy_exact` / `copy_prefix` を足す。**
同じ表を使うので、将来ストアが増えてもリネーム・削除 purge・復元が同時に追随する。

守ること:

- **`INSERT OR IGNORE`** を使う。「復元先に既存があれば既存優先」が自然に満たされる (§8.1)。
  列は `PRAGMA table_info` で取得し、キー列だけ差し替える。
- **prefix コピーは `LIKE` ではなく `substr` 等値**で列挙する (`move_prefix` と同じ)。
  path 中の `%` / `_` を誤爆させないため。長さ引数は SQLite では文字数なので
  `chars().count()`。
- **`rating.db` の `source_path` 列は導出値。** `migrate_store`
  ([rename_key_migration.rs:750](../../src/rename_key_migration.rs:750) 付近) と同じ再計算 UPDATE が
  copy 側にも要る。
- **正規化は記述子の `normalization` に従う。** 自前で `KeepDrive` / `DriveStripped` を
  選ばない。`book_resume` / `spread` / `view_trim_books` は `DriveStripped`。
- **非一意テーブル (`video_bookmarks`、`unique: false`) は v1 の対象外** (§2)。
  id 再採番が要る。**黙って壊れたコピーを作らず、対象から除外すること。**
- `busy_timeout` を付けて本体側の接続と共存する (rusqlite の既定は 5 秒。
  `journal_mode` 変更だけでは効かない)。

## 3. 変換アーカイブは 4 面まとめてコピーする

**再変換もキャッシュ ZIP の共有も不要** (§4)。変換キャッシュ ZIP の置き場が
**元ファイルパスだけの純関数**だから
([archive_cache.rs:59](../../src/archive_cache.rs:59) / `cache_zip_path_for_data_dir`)。
コピー先の将来のキャッシュ ZIP パスを**変換前に計算できる**ので、キーを先に付け替えておけば、
後日そのコピーを開いて変換したときキャッシュはちょうど予測したパスに生成される。
**`archive_cache.db` は一切触らない** (1 行 = 1 ファイル所有の前提を壊さない)。

キー基底は 2 種類ある ([metadata_transfer.rs:338](../../src/metadata_transfer.rs:338) の
`PortableVirtualKeyBase`)。アーカイブ 1 ファイルの復元では **4 面**をコピーする:

| # | 旧キー | 新キー | 主な中身 |
| --- | --- | --- | --- |
| 1 | `<old>` | `<new>` | ★ / タグ / 代表サムネピン / 本の続き / 見開き |
| 2 | `<old>::` prefix | `<new>::` prefix | 直接閲覧した ZIP のページ編集 |
| 3 | `cache_zip(old)` | `cache_zip(new)` | 変換キャッシュ側のコンテナ状態 |
| 4 | `cache_zip(old)::` prefix | `cache_zip(new)::` prefix | 変換キャッシュ側のページ編集 |

`metadata_transfer` は 2 と 4 が同時に存在するとエラーにするが (import は 1 基底しか
選べないため)、**本機能は両方コピーすればよいだけなのでエラーにしない。**

> **`cache_zip_path_for` は `path_key::normalize` (= ドライブ文字を落とす) を使う。**
> `C:\a\x.rar` と `D:\a\x.rar` は同じキャッシュ ZIP を指すので、別ドライブの同一相対パスへ
> コピーした場合 3 / 4 のキーは元から一致してコピーが no-op になる。
> **正しい挙動だがテストで明示する** (§9)。

## 4. 復元後の後始末 — 抜かすと台無しになる

§8.2 の 4 つ。**A3a では、UI スレッドを必要としない 1 と 4 を実装し、
2 と 3 は A3b が呼べる形の関数として用意する** (A3a には呼び出し元が無いため)。

1. **コピー先フォルダのサイドカー `mimageviewer.dat` にもミラーする**
   (`App::with_sidecar_coords_mut` [app.rs:53609](../../src/app.rs:53609))。
   ここを抜かすと、そのフォルダを次に丸ごと移動したときにまた失われる。
   **既存のサイドカー書き込み規律を変えないこと** — `with_sidecar_coords_mut` は
   メモリ上の `SidecarFile` を触るだけで、ディスクへは `flush_all_sidecars` が
   フォルダ切替 / 終了 / 5 秒アイドルで書く。**同期 flush を新しく足さない。**
2. **メモリ presence 集合の更新** (`adjusted_page_keys` / `mask_page_keys` /
   `conceal_page_keys` / `local_adjust_page_keys` / `comic_page_keys`)。
   グリッドのバッジとスマートフォルダ集計が参照する。
3. `clear_page_edit_state` / `rating_cache.clear()` / `invalidate_rating_counts_cache()` /
   `clear_tags_cache()` — `finish_book_page_edit_mapping`
   ([app.rs:30963](../../src/app.rs:30963)) と同じ後始末を再利用する。
4. **復元先にも `edit_origin` 行を作る** (以後そのファイルが新たなコピー元になり得る)。
   A1/A2 で入れた `has_restorable_content` を**立てる**こと。これで A2 の検出が
   その行を復元元として扱い、同じファイルを二度提案しなくなる。

## 5. `restore_declined`

- **書き込み API を A3a で用意する** (`(full_hash, target_key)` の記録)。
  表は A1 で作成済み、読み取りは A2 で実装済み。
- **実際に書くのは A3b** (§6.2: 行のチェックを外して `[復元する]` を押したときだけ恒久記録。
  `[閉じる]` は記録しない)。**A3a で勝手に書く経路を作らない。**

## 6. 制約

- **UI スレッドから DB を 21 個開かない。** コピー本体は worker。
- **時間窓・sleep・retry で吸収しない。**
- **`archive_cache.db` を触らない。**
- **非一意テーブルを黙ってコピーしない。**
- 既存の `rename_key_migration` の move 系の**挙動を変えない**。足すのは copy 系。

## 7. テスト

§9 がほぼそのまま。`rename_key_migration` の既存テスト群
(`migrates_exact_file_keys_across_stores` 等) が雛形になる。

- 画像 / ZIP / PDF の exact + prefix コピーが**全ストアに渡る**こと。
- **変換アーカイブ**: `cache_zip_path_for_data_dir` で予測した新パス配下へ 4 面がコピーされること。
- **変換アーカイブ・別ドライブ同一相対パス**: 3 / 4 が no-op になること (ドライブ文字除去の帰結)。
- 復元先に既存編集があるとき**上書きしない**こと (`INSERT OR IGNORE`)。
- `rating.source_path` が新キー基準に**再計算される**こと。
- **非一意テーブル (`video_bookmarks`) が対象外**であること。
- prefix コピーで path 中の `%` / `_` が**ワイルドカード扱いされない**こと。
- `DriveStripped` ストア (`book_resume` / `spread` / `view_trim_books`) が
  記述子どおりに正規化されること。
- 復元先に `edit_origin` 行ができ、`has_restorable_content` が立つこと。
  その結果、**A2 の検出がその行を復元元として扱う**こと。

## 8. 完了条件

- `cargo fmt` 済み / `cargo test -p mimageviewer --lib` が緑
- `cargo check -p mimageviewer --bin mimageviewer-core` が通る
- **報告に、`STORES` 21 件のうちコピー対象にした数と、除外したものとその理由を書く**

# タグのページ単位対応に向けた影響調査

調査日: 2026-08-30 / 調査対象: `external-tool-launch` / `ecb0da2e`

## 1. 結論

1. `tags.db` の文字列キーそのものは、すでに ZIP/PDF ページを保存できる。メタデータ転送はページタグを export/import し、コンテナ改名・削除も `::` prefix を移行・purge する。
2. 主な未対応箇所は DB ではなく、タグ対象を `PathBuf` だけで表す書き込みモデル、タグ横断一覧の復元、現在グリッドのタグキャッシュ/facet、remote の一覧変換である。
3. 現行タグ横断一覧は DB のページキーを実パスに変換するため、ページを `Missing` として黙って落とす。タグの総件数には含まれるので、ページを含める母集団が summary と一覧で不一致になる。パニックする経路は確認できなかった。
4. `item_kind` だけを `item_tags` に追加しても ZIP ページは完全には復元できない。新規書き込みは `kind + source_path + entry_name/page_num` を item 単位で明示保存し、既存行だけ厳密な `item_key` 解析へフォールバックする、★と同じ hybrid 構造が妥当である。
5. アプリ内のコンテナ改名ではページタグも追随できる。アプリ外の改名・移動は rename transaction の対象外だが、旧コンテナが content identity 台帳にあり、同一内容と判定され、復元対象に選ばれた場合は別経路でページタグも copy される。タグだけを持つ旧コンテナが台帳へ入る条件は未確定である。
6. ページタグの書き込みだけを先に有効化してはいけない。少なくともローカルの書き込み、cache/facet、タグ横断一覧の materialize を同じリリースで揃え、remote はページを返すか、summary/query の双方でページを同じ母集団から除外する必要がある。

本書の「誤動作」は、パニックせず処理を継続するが、結果欠落・別対象への書き込み・件数不一致などが生じることを表す。「未確定」はソースまたはテストから根拠を取れなかった事項である。

## 2. 前提となる identity

- 実ファイルのタグキーは path の小文字化と `\` → `/` 変換だけで作る（[adjustment_db.rs:444-446](../src/adjustment_db.rs#L444)、[tags_db.rs:932-934](../src/tags_db.rs#L932)）。
- ZIP ページは `<container>::<entry lowercase>`、PDF ページは `<container>::page_<u32>` である（[adjustment_db.rs:449-461](../src/adjustment_db.rs#L449)、[edit_source.rs:356-365](../src/edit_source.rs#L356)、[edit_source.rs:388-390](../src/edit_source.rs#L388)）。
- `item_tags` は item×tag の複数行、`tag_item_state` は item ごと 1 行で、どちらにも item kind はない（[tags_db.rs:109-125](../src/tags_db.rs#L109)）。タグが 0 件でも `set`/`clear` は `tag_item_state` を更新する（[tags_db.rs:237-262](../src/tags_db.rs#L237)、[tags_db.rs:384-395](../src/tags_db.rs#L384)）。
- ★は `RatingItemKind` に `ZipImage` / `PdfPage` / `ZipDir` を持ち、`source_path`、`entry_name`、`page_num` 等を item と一緒に保存する（[rating_db.rs:15-58](../src/rating_db.rs#L15)、[rating_db.rs:63-109](../src/rating_db.rs#L63)、[rating_db.rs:310-361](../src/rating_db.rs#L310)）。

## 3. Q1: `item_key` を実パスとして扱う場所

### 3.1 DB から読み出したキーを直接 path 化する経路

本番の `item_tags.item_key` consumer を横断した結果、DB キー全体をそのまま実パス化する経路は、本体タグ横断一覧と、それを再利用する remote tag-items であった。メタデータ掃除にも path 化はあるが、こちらは先に `::` より左を取り出している。

| 場所 | 処理 | `::` ページキーが来た結果 | 判定 |
| --- | --- | --- | --- |
| タグキー選択 | exact/prefix/AND の SQL は `item_key` を文字列のまま返す（[tags_db.rs:774-817](../src/tags_db.rs#L774)、[tags_db.rs:820-878](../src/tags_db.rs#L820)）。 | ページキーも通常の候補として返る。ここだけなら問題はない。 | 無害 |
| `run_tag_view_search` | 候補を `PathBuf::from(&key)` にする（[tag_view.rs:292-307](../src/tag_view.rs#L292)）。 | `PathBuf` の構築自体は成功するが、仮想 identity が物理 path 型に化ける。 | 単体は無害、後段と合わせて誤動作 |
| 実在確認 | `fs::metadata` を呼ぶ（[tag_view.rs:389-405](../src/tag_view.rs#L389)）。 | 通常の Windows path では `<container>::...` は実在しないため `Missing`。検索側は結果から隠すだけで DB 行は消さない（[tag_view.rs:318-320](../src/tag_view.rs#L318)）。 | 誤動作、panic なし |
| case-insensitive fallback | metadata 失敗時に `parent` / `file_name` / `read_dir` で同階層を探す（[tag_view.rs:456-469](../src/tag_view.rs#L456)）。 | 合成名に一致する実 sibling はないため `Missing` のまま。I/O error は `Option` に落ちる。 | 誤動作、panic なし |
| casing/kind 復元 | 実在時だけ `canonicalize` し（[tag_view.rs:428-453](../src/tag_view.rs#L428)）、拡張子で実ファイル種別を決める（[tag_view.rs:472-494](../src/tag_view.rs#L472)）。 | 通常は metadata で落ちるため到達しない。仮に `::` を含む物理名が使える特殊環境なら、ZIP member 末尾の拡張子を実ファイル拡張子として誤分類する可能性がある。特殊環境での実動作は未確定。 | 通常は未到達。特殊環境は未確定 |
| tag view result | `TagViewEntry` は `path: PathBuf` と実体種別 7 種だけを持つ（[tag_view.rs:227-244](../src/tag_view.rs#L227)）。適用側もその 7 種だけを `GridItem` 化する（[app.rs:20359-20380](../src/app.rs#L20359)）。 | `ZipImage` / `PdfPage` の表現がなく、metadata 判定を通せてもページとして materialize できない。 | 誤動作、panic なし |
| tag view scan limit | path 化より前に DB 結果へ scan limit を掛ける（[tag_view.rs:287-296](../src/tag_view.rs#L287)）。 | 後で落ちるページキーも上限を消費し、実ファイル結果を押し出して `truncated` を立て得る。 | 誤動作、panic なし |
| remote tag-items | raw key を `PathBuf::from` して同じ classifier へ渡し、`Missing` を `None` にする（[remote_ipc/collections.rs:994-1028](../src/remote_ipc/collections.rs#L994)）。`None` は一覧から捨てる（[remote_ipc/collections.rs:1051-1079](../src/remote_ipc/collections.rs#L1051)）。 | 本体と同じくページが消え、scan limit は消費する。 | 誤動作、panic なし |
| メタデータ掃除 | `physical_path_for_key` は `source_path` がなければ**最初の** `::` より左だけを `PathBuf` にする（[metadata_cleanup.rs:982-1007](../src/metadata_cleanup.rs#L982)）。その物理コンテナへ `try_exists` する（[metadata_cleanup.rs:971-979](../src/metadata_cleanup.rs#L971)）。 | コンテナがあればページタグ行を orphan 扱いしない。member 内の追加 `::` にも影響されない。 | 無害、panic なし |

タグ横断一覧には missing を保持する回帰テストがあり、結果から隠しても DB を削除しないことは固定されている（[tag_view.rs:718-750](../src/tag_view.rs#L718)）。したがって現行の障害はデータ消失ではなく、一覧欠落と件数不一致である。

### 3.2 指定された周辺経路

#### `tag_prewarm.rs`

この名前の worker は現在、tags.db の prewarm ではなく XMP rating 読み取り専用である（[tag_prewarm.rs:1-14](../src/tag_prewarm.rs#L1)）。job は実 `PathBuf` を持ち、ファイルを読む（[tag_prewarm.rs:53-58](../src/tag_prewarm.rs#L53)、[tag_prewarm.rs:107-121](../src/tag_prewarm.rs#L107)）。producer も通常の `GridItem::Image` だけを投入する（[app.rs:49031-49065](../src/app.rs#L49031)）。

`item_tags.item_key` や ZIP/PDF ページは流れないため、ページタグ対応でこの worker を変更する必要はない。判定は無害である。

#### `tag_ops.rs` / `tag_write_worker.rs` / Undo

現行の問題は DB キーを path 化する読み出しではなく、書き込み identity を最初から path だけで持つことにある。

- `TagTarget` は `path: PathBuf` と optional sidecar だけである（[tag_ops.rs:15-19](../src/tag_ops.rs#L15)）。viewer の `ZipImage` / `PdfPage` は親 ZIP/PDF へ変換し、main-window 面の同じ page variant は `None` を返す（[tag_ops.rs:21-39](../src/tag_ops.rs#L21)）。そのため現行ではページキーは worker に流れず、viewer はコンテナへ誤付与、グリッド側は拒否または silent return になる。
- target が空なら toggle は表示なしで return する（[tag_ops.rs:208-225](../src/tag_ops.rs#L208)）。これはピン留めクイックが無反応になる直接経路である。
- worker は `TagWriteJob.path` を文字列正規化して DB を更新する（[tag_write_worker.rs:48-58](../src/tag_write_worker.rs#L48)、[tag_write_worker.rs:320-415](../src/tag_write_worker.rs#L320)）。仮に合成キーを `PathBuf` に詰めても、DB 更新部分だけは文字列処理なので動き、panic しない。
- cache hydrate/update も path をキー文字列へ正規化するだけである（[tag_ops.rs:522-544](../src/tag_ops.rs#L522)、[tag_ops.rs:649-655](../src/tag_ops.rs#L649)）。Undo も `TagChange.path` を保存して同じ job を再投入する（[undo_stack.rs:103-112](../src/undo_stack.rs#L103)、[undo_ops.rs:612-643](../src/undo_ops.rs#L612)）。文字列 identity としては動くが、物理 source と仮想 item を区別できない。

合成キーを既存 `path` 欄へそのまま流すだけの実装は不可である。以下は**現行ではページキーが流れないため未発生だが、その実装をした場合に確定している hazard**である。

- sidecar target は `path.parent()` と `path.file_name()` から作る（[tag_write_worker.rs:21-27](../src/tag_write_worker.rs#L21)、[sidecar.rs:176-182](../src/sidecar.rs#L176)）。タグ sidecar backup 設定が ON の場合（[app.rs:53985-53997](../src/app.rs#L53985)）、合成 key に member の `/` がなければ、実フォルダの `mimageviewer.dat` に `book.zip::page.jpg` / `book.pdf::page_n` という virtual rel key を書く。この rel key は import 時に `ZipImage` / `PdfPage` と分類され、タグ import は `Image` だけに限定されるため復元されない（[sidecar.rs:828-845](../src/sidecar.rs#L828)、[sidecar.rs:885-914](../src/sidecar.rs#L885)）。member に `/` があれば `book.zip::dir` のような非実在 parent への書き込みを試み、I/O error を log して失敗する（[sidecar.rs:745-800](../src/sidecar.rs#L745)）。どちらも誤動作・panic なしである。job 自身も virtual page は `tag_sidecar: None` とする契約を持つため（[tag_write_worker.rs:48-54](../src/tag_write_worker.rs#L48)、[app.rs:54000-54007](../src/app.rs#L54000)）、page target は必ず sidecar `None` にしなければならない。
- 成功した `res.path` は content identity 記録へ渡される（[tag_ops.rs:617-647](../src/tag_ops.rs#L617)）。分類は path の末尾拡張子だけで行う（[content_identity.rs:100-118](../src/content_identity.rs#L100)）ため、ZIP ページキーの末尾が `.jpg` 等なら合成 path を物理画像と誤認し、worker が `fs::metadata` / `File::open` を試す（[content_identity.rs:1762-1789](../src/content_identity.rs#L1762)）。失敗は log で処理され panic はしない（[content_identity.rs:1707-1718](../src/content_identity.rs#L1707)）。PDF の `pdf::page_n` は拡張子判定に通らず、コンテナ identity の記録も行われない。

したがって書き込み target/job/result/undo は、少なくとも `item_key` と `physical_source` を分離し、sidecar 対象も独立に保持する必要がある。

#### 改名・削除

- `rename_key_migration::STORES` は `item_tags` と `tag_item_state` を含む（[rename_key_migration.rs:410-423](../src/rename_key_migration.rs#L410)）。実パスを正規化して、exact、`<old>/`、`<old>::` の 3 面を文字列移行する（[rename_key_migration.rs:688-731](../src/rename_key_migration.rs#L688)、[rename_key_migration.rs:965-1004](../src/rename_key_migration.rs#L965)）。DB キー全体を実パス扱いしないため無害である。詳細は Q3 に記す。
- delete worker は Shell が削除成功と返した**実パス**だけを purge へ渡す（[delete_worker.rs:121-148](../src/delete_worker.rs#L121)、[delete_worker.rs:220-265](../src/delete_worker.rs#L220)）。purge は同じ `STORES` に対して exact、`/`、`::` range を削除する（[rename_key_migration.rs:778-810](../src/rename_key_migration.rs#L778)、[rename_key_migration.rs:890-950](../src/rename_key_migration.rs#L890)）。ZIP/PDF コンテナ削除で配下ページタグも消える。ページ自身を物理削除 worker へ渡す経路はなく、panic しない。

#### メタデータ転送

メタデータ転送はページタグを意図的に扱っており、DB キー全体を実パス化しない。

- export scope は物理 exact key と `<base>::...` range を分ける（[metadata_transfer.rs:1355-1398](../src/metadata_transfer.rs#L1355)）。タグと決定状態を両 scope から読む（[metadata_transfer.rs:1686-1751](../src/metadata_transfer.rs#L1686)）。
- `locate_item_key` は最初の `split_once("::")` で base と残り全体を分ける（[metadata_transfer.rs:2589-2601](../src/metadata_transfer.rs#L2589)）。member 内に追加の `::` があっても残り側に保持される。
- portable 形式は物理 item と virtual item の両方に `tags` / `tags_decided` を持つ（[metadata_transfer.rs:355-392](../src/metadata_transfer.rs#L355)）。import は virtual key を再生成して `item_tags` / `tag_item_state` に挿入する（[metadata_transfer.rs:4019-4122](../src/metadata_transfer.rs#L4019)、[metadata_transfer.rs:4799-4831](../src/metadata_transfer.rs#L4799)）。
- ZIP/PDF ページタグと決定状態の往復テストが存在する（[metadata_transfer.rs:6289-6395](../src/metadata_transfer.rs#L6289)）。したがって page tag row は将来の UI 書き込みより前にも import で存在し得る。

転送本体は無害だが、import 後の現在グリッド refresh は `tag_item_path(item)` からしかタグキーを作らない（[app.rs:27397-27417](../src/app.rs#L27397)）。ページタグは DB に入っても現在表示中のページ cache を再 hydrate しないため、ページ対応時にはこの refresh index も変更が必要である。

#### facet / 検索 / smart folder / remote の供給

- global summary、prefix suggestion、exact summary は `item_tags` 全行を無条件に `COUNT(*)` する（[tags_db.rs:683-705](../src/tags_db.rs#L683)、[tags_db.rs:716-765](../src/tags_db.rs#L716)）。ページタグも件数へ入る。
- 通常グリッドは `tag_item_path` が返す実体 7 種だけを preload する（[app/metadata_ops.rs:179-193](../src/app/metadata_ops.rs#L179)、[app.rs:48923-48958](../src/app.rs#L48923)）。セル参照も同じ helper のためページは空タグかつ loaded 扱いになる（[app.rs:49256-49279](../src/app.rs#L49256)）。DB key を path 化する誤りではないが、ページ行を表示・filter へ供給しない。
- サブフォルダ展開も scan entry の実パスだけから exact key を作り（[app/subfolder_expansion.rs:1187-1205](../src/app/subfolder_expansion.rs#L1187)）、その key 群だけを `get_many_display_tags` へ渡す（[app/subfolder_expansion.rs:1233-1256](../src/app/subfolder_expansion.rs#L1233)）。この view が生成する item 自体も physical な `Folder/Zip/Pdf/Image/Video` に限られるため（[app/subfolder_expansion.rs:277-305](../src/app/subfolder_expansion.rs#L277）、page tag row を供給しない現行動作は無害・panic なしである。将来この view に virtual page を含めるかは未確定であり、含める場合だけ cache/facet の変更が要る。
- smart folder は実ファイル scan で得た exact key だけを bulk read し、構築する `GridItem` にページ variant がない（[app/smart_folder.rs:1724-1744](../src/app/smart_folder.rs#L1724)、[app/smart_folder.rs:2112-2157](../src/app/smart_folder.rs#L2112)）。ページ tag row は smart-folder 候補にならない。
- Ctrl+F は tags.db の候補チップを出すだけで、通常 FTS にタグを混ぜない（[app.rs:47349-47360](../src/app.rs#L47349)）。FTS 側も `Tags` を通常検索対象から明示除外する（[fts_index.rs:71-97](../src/fts_index.rs#L71)、[fts_index.rs:125-149](../src/fts_index.rs#L125)）。したがって Q4 の「タグ検索」は tags.db → tag view の経路を指す。
- remote browse は全 summary をそのまま返す（[remote_ipc/collections.rs:295-315](../src/remote_ipc/collections.rs#L295)、[remote_ipc/collections.rs:945-968](../src/remote_ipc/collections.rs#L945)）一方、tag-items は前述の path classifier でページを落とす。remote でも件数と一覧が不一致になる。

### 3.3 Q1 の網羅結果

`item_tags` SQL、`TagsDb::item_keys_by_tag_*` の caller、`PathBuf::from(key)`、`get_item_tags` / `get_many_display_tags` の caller を検索した範囲では、上記以外に DB の tag item key 全体を実パス化する本番経路は見つからなかった。確認できた範囲で `::` により panic する経路は 0 件である。

## 4. Q2: タグ横断一覧はページを復元できるか

### 4.1 現行回答

復元できない。`TagViewEntry` に page identity がなく、実 path の metadata と拡張子だけで 7 種へ分類するためである（[tag_view.rs:227-244](../src/tag_view.rs#L227)、[tag_view.rs:389-425](../src/tag_view.rs#L389)）。

対して ★ は、明示 kind があれば `ZipImage` / `PdfPage` / `ZipDir` を直接組み立て、metadata は仮想キーではなく source container に対して取得する（[rating_view.rs:155-225](../src/rating_view.rs#L155)、[rating_view.rs:317-330](../src/rating_view.rs#L317)）。kind のない旧行だけ key parse へ落とす（[rating_view.rs:229-251](../src/rating_view.rs#L229)）。

### 4.2 `item_kind` 明示案

`item_tags` に nullable `item_kind` だけを足す案には、次の不足がある。

1. ZIP page key は entry 名を小文字化するため、元 casing を失っている（[adjustment_db.rs:456-461](../src/adjustment_db.rs#L456)）。`kind=ZipImage` だけでは表示・読み出しに必要な元 `entry_name` を復元できない。
2. PDF は key から page number を parse できるが、★相当の一貫性を持たせるなら `page_num` を明示する方がよい。必要な最小 metadata は `kind`, `source_path`, ZIP の `entry_name`, PDF の `page_num` である。これは ★ の既存列と同じである（[rating_db.rs:63-109](../src/rating_db.rs#L63)）。
3. `item_tags` は item×tag の表なので、同じ item metadata をタグ数だけ重複させ、行間不一致を DB が防げない（[tags_db.rs:111-117](../src/tags_db.rs#L111)）。item 単位の所有先が必要である。
4. リリース済み DB の既存行には metadata がない。nullable migration と legacy fallback が必須である。★は `PRAGMA table_info` を確認して `ALTER TABLE ADD COLUMN` する（[rating_db.rs:164-200](../src/rating_db.rs#L164)）。
5. `TagsDb::copy_item_key` / `move_item_key` と metadata transfer は列名を明示した SQL を持つ（[tags_db.rs:398-447](../src/tags_db.rs#L398)、[metadata_transfer.rs:4799-4831](../src/metadata_transfer.rs#L4799)）。新 metadata の copy/move/export/import を同時に更新しなければならない。
6. 現行 exact/prefix/AND query と `select_tag_view_item_keys` は `Vec<String>` しか返さない（[tags_db.rs:774-878](../src/tags_db.rs#L774)、[tag_view.rs:336-352](../src/tag_view.rs#L336)）。identity table を作るだけでは tag view へ kind/source/member/page が届かない。選択後の bulk identity lookup、または各 query を identity join/read model に変更し、既存の `DISTINCT`、並び順、limit を維持する必要がある。

明示 metadata の所有先は、既存の `tag_item_state` 拡張と、専用 item identity table の二案がある。前者は item ごと 1 行だが「タグを確認済み」という状態責務と identity 責務が混ざる。後者は責務を分けられるが、タグ 0 件の decided item でも identity を保持し、copy/move/purge/cleanup の対象へ追加する必要がある。**正式な所有先は未確定**である。

本調査の推奨は、`item_tags` へ metadata を重複させず、item key を主キーとする専用 identity 行を作る案である。これは設計提案であり、現行コードから確定した事実ではない。

### 4.3 `item_key` 解析案

parse-only でも、★の legacy parser と同じ次の順序なら `::` / `page_3` の誤分類を避けられる。

1. **最初の** `split_once("::")` だけを使い、左を container、右の残り全体を member とする。
2. 左の物理 container を確認して、その拡張子を先に分類する。
3. 左が PDF の場合だけ、右全体が厳密に `page_<u32>` なら `PdfPage` とする。
4. 左が対応 ZIP container の場合だけ ZIP image entry として画像 extension を確認し、ZIP を列挙して小文字/slash 正規化が一意に一致する元 `entry_name` を得る。その他の container 種別は復元しない。

この順序は ★ の legacy 復元そのものである（[rating_view.rs:229-251](../src/rating_view.rs#L229)、[rating_view.rs:339-370](../src/rating_view.rs#L339)）。`rsplit_once` や delimiter 全分割、container extension を見ない `page_` 判定は不可である。

この parser は対応 ZIP/PDF container だけを対象とするため、`book.rar::entry.jpg` のような変換アーカイブの既存 metadata-less 行は復元できない。metadata transfer はその source-base 行を作り得る（[metadata_transfer.rs:7019-7033](../src/metadata_transfer.rs#L7019)）。変換アーカイブには explicit identity の backfill、または `archive_cache` の source/cache 対応を使う専用 legacy resolver が別途必要であり、どちらを正本にするかは未確定である。

#### ZIP entry に `::` / `page_3` は起こり得るか

起こり得る。

- 使用中の依存は `zip 2.4.2` である（[Cargo.toml:332](../Cargo.toml#L332)、[Cargo.lock:8884-8888](../Cargo.lock#L8884)）。依存 crate の `zip-2.4.2/src/write.rs:1163-1183` は entry name を任意の `ToString` として受け取り、`write.rs:1048-1054` の拒否条件は重複名である。`types.rs:648-664` はその文字列をそのまま filename bytes にする。`:` や `page_` を禁止する処理はない。
- mIV 側も ZIP 名を decode した後、`\` を `/` に変えるだけである（[zip_loader.rs:210-233](../src/zip_loader.rs#L210)）。画像 extension なら full entry name をそのまま保持する（[zip_loader.rs:385-415](../src/zip_loader.rs#L385)）。
- `dir::page_3.jpg` は最初の split なら右側全体に残り、左が対応 ZIP container であることを先に確認するため ZIP image と判定できる。`page_3` だけの entry も ZIP 上は作れるが、画像 extension がないため現行画像列挙の対象外である。`page_3.jpg` / `dir/page_3.jpg` は対象になり得る。

#### parse-only の壊れ方

- page key が小文字化されるため、`Dir/Page.JPG` と `dir/page.jpg` のような case-only duplicate は同じ key に衝突する。★の legacy resolver も候補が複数なら復元を拒否する（[rating_view.rs:339-349](../src/rating_view.rs#L339)、[rating_view.rs:473-481](../src/rating_view.rs#L473)）。
- 明示 `entry_name` を追加しても、DB identity 自体が同じなら 2 entry を別々にタグ付けできない。この既存 key 制約まで今回変更するかは未確定である。
- container が offline/missing、ZIP が壊れている、entry が削除済みの場合は列挙できない。現行 tag view と同様に結果を隠し、DB は保持する方針が必要である。
- ★の legacy resolver は 1 key ごとに ZIP 全 entry を列挙する（[rating_view.rs:339-349](../src/rating_view.rs#L339)）。これを多数の同一 ZIP page row へそのまま流用すると、同じ archive の列挙を page 数だけ繰り返す。tag view worker 内なので UI thread の同期 I/O にはならないが、検索 I/O は大きくなり得る。container ごとの grouping/cache、cancel の確認位置、scan limit を identity lookup の前後どちらへ適用するかを設計する必要がある。

### 4.4 推奨結論

新規ページタグは明示 item metadata を正とし、既存/import 済みで metadata のない行だけ厳密 parser へ fallback する hybrid とする。★の `explicit first, legacy parse second` と同じ構造である（[rating_view.rs:155-159](../src/rating_view.rs#L155)、[rating_view.rs:191-212](../src/rating_view.rs#L191)、[rating_view.rs:229-251](../src/rating_view.rs#L229)）。

`TagViewEntry` は再び `path + inferred kind` を持たせるより、復元済み `GridItem` と物理 metadata を持つ形が自然である。適切な `GridItem::ZipImage` / `GridItem::PdfPage` まで復元できれば、既存のグリッド open は両 variant を直接 fullscreen で開ける（[ui_main.rs:13196-13230](../src/ui_main.rs#L13196)）。新しい page open dispatcher は不要である。

## 5. Q3: 改名・移動でページタグは追随できるか

### 5.1 アプリ内コンテナ改名

追随できる。

1. rename transaction は `item_tags` と `tag_item_state` を共通 `STORES` に登録している（[rename_key_migration.rs:410-423](../src/rename_key_migration.rs#L410)）。
2. old/new 実パスを正規化し、各 store に exact、`/` prefix、`::` prefix の移行を掛ける（[rename_key_migration.rs:711-731](../src/rename_key_migration.rs#L711)、[rename_key_migration.rs:981-1004](../src/rename_key_migration.rs#L981)）。
3. `move_prefix` は suffix を parse せずそのまま新 prefix へ連結する（[rename_key_migration.rs:1069-1098](../src/rename_key_migration.rs#L1069)）。entry 内の追加 `::` や `page_3` は影響しない。
4. destination に同じ主キーがあれば `UPDATE OR IGNORE` 後に old を削除し、新側を優先する（[rename_key_migration.rs:1043-1067](../src/rename_key_migration.rs#L1043)）。これはタグ行にも適用される。

したがって `old.zip::dir/page.jpg` は `new.zip::dir/page.jpg`、`old.pdf::page_3` は `new.pdf::page_3` になる。

### 5.2 ★との比較

★も同じ `STORES` generic migration を使い、`::` page key を移す。既存テストは rating の ZIP entry key、新側優先、`%` / `_` 非 wildcard を固定している（[rename_key_migration.rs:2530-2602](../src/rename_key_migration.rs#L2530)）。

★だけは横断一覧用の派生列 `source_path` も持つため、key migration 後に `::` より左へ再計算する追加 SQL がある（[rename_key_migration.rs:1005-1024](../src/rename_key_migration.rs#L1005)）。現行タグにはその列がないため追加処理は不要である。将来タグ item identity に `source_path` を足すなら、identity table を `STORES` に追加するだけでなく、この派生列も新 container path へ更新する必要がある。

タグの実パス改名テストは `item_tags` の移行を明示 assertion している（[rename_key_migration.rs:2402-2418](../src/rename_key_migration.rs#L2402)）。`tag_item_state` は fixture に入るが（[rename_key_migration.rs:2315-2342](../src/rename_key_migration.rs#L2315)）、移行後の直接 assertion はない。`::` page tag/state 専用の regression test も見つからなかった。generic 実装上は追随するが、実装時には ZIP/PDF page tag と state の専用テストを追加すべきである。

### 5.3 「移動」の範囲

- rename module 自体は任意の old/new path を受け取れるが、現在の本番 caller はアプリ内 rename 成功後である（[ui_dialogs/rename_item.rs:126-152](../src/ui_dialogs/rename_item.rs#L126)）。Shell rename は同じ parent に新しい名前を join する（[shell_file_ops.rs:109-118](../src/shell_file_ops.rs#L109)）。
- module は Explorer 等によるアプリ外 rename を明示的に対象外としている（[rename_key_migration.rs:33-34](../src/rename_key_migration.rs#L33)）。したがって、任意のアプリ外 move/rename でページタグが必ず追随するとはいえない。
- 製本系の page mapping は rating と tags の exact key を一緒に copy/move する別経路を持つ（[app.rs:30897-31003](../src/app.rs#L30897)）。これは ZIP/PDF コンテナの Shell rename とは別の lifecycle である。

アプリ外 move には別の content identity restore がある。設定が有効な物理フォルダ一覧で size 候補を絞り（[app/content_identity_detection.rs:144-160](../src/app/content_identity_detection.rs#L144)、[content_identity.rs:305-327](../src/content_identity.rs#L305)）、head/full hash が一致した候補（[content_identity.rs:1222-1280](../src/content_identity.rs#L1222)）が選択済み restore として渡されると、old→new の exact と `::` virtual prefix を copy する（[content_identity/restore.rs:144-158](../src/content_identity/restore.rs#L144)、[content_identity/restore.rs:287-303](../src/content_identity/restore.rs#L287)）。copy は unique な全 `STORES` が対象で、そこには `item_tags` / `tag_item_state` も含まれる（[rename_key_migration.rs:410-423](../src/rename_key_migration.rs#L410)、[rename_key_migration.rs:553-597](../src/rename_key_migration.rs#L553)）。復元元に exact または `::` 配下の行があるかを確認する probe も持つ（[rename_key_migration.rs:609-685](../src/rename_key_migration.rs#L609)）。したがって、旧コンテナが台帳にあり同一内容と判定され、その復元が選択されれば、ページタグも追随できる。

ただし、旧コンテナを台帳へ backfill する「既存編集」判定は adjustment/mask/conceal/local-adjust/comic の集合だけを見ており、tag row は見ない（[app/content_identity_detection.rs:230-260](../src/app/content_identity_detection.rs#L230)、[app/content_identity_detection.rs:411-423](../src/app/content_identity_detection.rs#L411)）。metadata import 等でタグ行だけを持つ旧コンテナが別経路で台帳 origin になることは確認できなかったため、この tag-only 条件は**未確定**である。

### 5.4 削除

コンテナのアプリ内削除が Shell 成功した場合、`<container>::...` の page tag と decided state は同じ purge で削除される（[delete_worker.rs:121-148](../src/delete_worker.rs#L121)、[rename_key_migration.rs:778-810](../src/rename_key_migration.rs#L778)）。missing scan や tag view の `Missing` 判定から purge を呼ばないため、offline container のタグを誤削除しない。

## 6. Q4: facet と検索への影響

### 6.1 現行の page tag row が各面に与える結果

| 面 | 現行結果 | materialize/open |
| --- | --- | --- |
| global tag summary / popular / recent / suggestion | `COUNT(*)` にページを含む（[tags_db.rs:683-743](../src/tags_db.rs#L683)）。 | summary 自体は item を開かない。 |
| Ctrl+F/Ctrl+G tag bridge / Ctrl+T tag view | suggestion count にはページを含む（[global_search_ui.rs:463-500](../src/global_search_ui.rs#L463)）。Ctrl+T は tags.db 直引きの tag view であり（[app.rs:5075-5085](../src/app.rs#L5075)）、exact/prefix/AND query はページ key も返す。 | tag view が実パス `Missing` として落とすため結果に出ない。materialize 経路なし。 |
| 現在グリッドの tag badge | `tag_item_path` が page variant を返さず、cell は空タグになる（[app/metadata_ops.rs:179-193](../src/app/metadata_ops.rs#L179)、[app.rs:49256-49279](../src/app.rs#L49256)）。 | なし。 |
| facet のタグ別件数 / タグなし件数 | count loop は `Image/Video/Audio/Zip/Pdf/Archive` だけを数え、`ZipImage/PdfPage` を skip する（[ui_main.rs:11271-11309](../src/ui_main.rs#L11271)）。ページはタグ付きにもタグなしにも数えない。 | なし。 |
| facet の選択タグ filter | filter gate も実体種別だけで、page は評価を素通りする（[app/metadata_ops.rs:196-210](../src/app/metadata_ops.rs#L196)、[app.rs:46572-46599](../src/app.rs#L46572)）。 | なし。 |
| facet の `Tagged/Untagged` edit flag | `item_supports_tags` が false なので page はどちらにも一致しない（[app.rs:46696-46703](../src/app.rs#L46696)）。 | なし。 |
| facet menu の page-only tag | DB prefix suggestion には現れるが、表示件数は current-grid の local count を採るので 0 になる（[ui_main.rs:10249-10295](../src/ui_main.rs#L10249)）。 | なし。 |
| smart folder の tag rule | 実ファイル snapshot の exact key だけを評価するためページは候補外（[app/smart_folder.rs:1207-1239](../src/app/smart_folder.rs#L1207)、[app/smart_folder.rs:2112-2157](../src/app/smart_folder.rs#L2112)）。 | page materialize なし。 |
| remote browse / tag-items | browse count はページを含むが、tag-items は `Missing` として落とす（[remote_ipc/collections.rs:302-315](../src/remote_ipc/collections.rs#L302)、[remote_ipc/collections.rs:994-1028](../src/remote_ipc/collections.rs#L994)）。 | tag collection からの page open なし。 |

現行は「DB 総件数には入るが、ローカル facet と横断一覧結果には出ない」という二重の不整合である。しかも metadata transfer が page tag row をすでに import できるため、これは純粋な将来仮定ではない。

ただし、summary の `COUNT(*)` と materialize 後の表示件数を常に数値一致させること自体は invariant ではない。tag view は missing/offline item を意図的に隠し、結果上限も 10,000 件である（[tag_view.rs:16-17](../src/tag_view.rs#L16)、[tag_view.rs:287-326](../src/tag_view.rs#L287)）。必要なのは「ページであることだけを理由に片側から落とさない」という page inclusion policy の一致である。

### 6.2 ページ対応で同時に変えるべき境界

1. `tag_item_path` の代わりに、item ごとの tag identity/key を返す helper を正本化する。page は `page_key_for_grid_item`、container は従来の physical key とし、親タグをページへ継承しない（[edit_source.rs:356-365](../src/edit_source.rs#L356)）。
2. tag cache preload、cell lookup、metadata import refresh、optimistic update、Undo をその typed key に揃える。
3. page key が cache に載るのと同じリリースで `ZipImage` / `PdfPage` を facet count と filter gate に加える。gate だけ先に追加すると、現在の「loaded=true + empty tags」により選択タグ filter で全ページが誤って消える。
4. tag view は復元済み `GridItem` を返し、物理 metadata は source container から取得する。これで既存の grid open が page variant を開ける。
5. global summary の count は item×tag 行数のままでよい。ページと container は別 target なので、同じ tag が両方に付けば 2 item と数える。
6. remote を同時に対応しない段階では、remote 用 browse summary と tag-items query の両方から page item を除外し、page inclusion policy を揃える。missing/offline と limit による件数差は別の既存仕様であり、片側だけ page を除外する不整合と混同しない。
7. 変換アーカイブの page tag も metadata transfer から既に存在し得る（[metadata_transfer.rs:7019-7033](../src/metadata_transfer.rs#L7019)、[metadata_transfer.rs:7088-7109](../src/metadata_transfer.rs#L7088)）。native ZIP/PDF だけ書き込み対応しても read 側の問題は消えない。変換 page を materialize するか、page-aware summary/query の双方から同じ規則で除外するかを、ローカル page 対応の release 前に確定する必要がある。

### 6.3 remote の materialize 余地

remote 全体に page address がないわけではない。`RemoteAddress` は `ZipEntry` / `PdfPage` をすでに表現できる（[crates/remote-ipc/src/lib.rs:71-115](../crates/remote-ipc/src/lib.rs#L71)）。一方、tag collection の `RemoteEntry` は primary page address を持たず、`path` と optional thumbnail address だけである（[crates/remote-ipc/src/lib.rs:1223-1236](../crates/remote-ipc/src/lib.rs#L1223)）。tag kind filter にも page variant はない（[crates/remote-ipc/src/lib.rs:1333-1350](../crates/remote-ipc/src/lib.rs#L1333)）。

また rating collection の共通変換は `ZipImage` / `PdfPage` を親 `Zip` / `Pdf` path に collapse する（[remote_ipc/collections.rs:1296-1331](../src/remote_ipc/collections.rs#L1296)）。この変換は tag page 一覧へそのまま再利用できない。remote 対応では tag entry に primary `RemoteAddress` を保持し、既存 container/page address の検証・描画経路へ渡す必要がある。

## 7. Q5: 段階的な実装案

以下の規模は production code、migration、unit test、UI snapshot、remote test を含む概算で、±30% 程度の幅を見込む。UI 文言の最終案は含めない。

### 段階 1: 黙った付け替えを止め、target identity を型で分ける

**内容**

- `TagTarget` / `TagWriteJob` / result / Undo を `PathBuf` 単独から、少なくとも `item_key`, `physical_source`, optional sidecar を分ける型へ変更する。real item と container item の動作は維持する。
- viewer page を container target へ変換する分岐を廃止する。metadata panel では container 操作を明示的な container 行として残し、未対応の page 操作は別 capability として明示拒否する。最終文言はこの段階では決めない。
- normal menu / `T` / quick pin / viewer panel が、未対応 page に対して同じ拒否結果を返すようにする。quick pin の silent return も feedback へ変える。
- page target はまだ DB へ発行しない。sidecar と content identity は実 physical/container target にだけ作用することを test する。

**主な対象**

`tag_ops.rs`, `tag_write_worker.rs`, `undo_stack.rs`, `undo_ops.rs`, `ui_metadata_panel.rs`, `ui_main.rs`, `app.rs` とテスト。

**規模**

6〜8 ファイル、約 250〜450 行。

**安全な停止境界**

page tag はまだ作れないが、どの入口でも page のつもりで container を変更しない。container tag 機能は明示 container 行から引き続き使える。この段階だけでリリースしても機能を黙って別対象へ付け替えず、新しい不可視 page tag も増えない。

### 段階 2: explicit item identity を additive に保存する（page はまだ無効）

**内容**

- item 単位の explicit identity を保持する additive schema migration と API を追加する。推奨は専用 identity table だが、実装開始前に `tag_item_state` 拡張案と比較して確定する。
- 段階 1 の target/job に explicit kind/member/page metadata を追加し、DB の tag 更新と同じ transaction で identity を保つ。
- exact/prefix/AND の item-key 選択後に bulk identity を取得する API、または identity を join する read model を追加し、`DISTINCT`、order、limit を維持する。UI はまだその結果を materialize しない。
- 変換アーカイブの source/cache identity の正本を確定し、既存 `item_tags` / `tag_item_state` の metadata-less page row を explicit identity へ backfill するか、`archive_cache` 対応を使う legacy resolver で一意に識別できるようにする。曖昧・欠落した行を誤った container/page へ推測で割り当てない。これは段階 3 へ進むための gate とする。
- copy/move、rename/purge/cleanup、metadata transfer を新 identity に対応させる。旧 DB と旧 portable manifest は nullable/default で読めるようにする。
- legacy parser を pure helper として追加し、PDF の厳密 `page_<u32>`、ZIP entry 内 `::` / `page_3.jpg`、case ambiguity、missing container を unit test する。
- container rename による `item_tags` / `tag_item_state` / identity の ZIP/PDF page prefix 移行と、container delete purge の回帰テストを追加する。
- UI は段階 1 の明示拒否を維持し、まだ page tag を生成しない。

**主な対象**

`tags_db.rs`, `tag_ops.rs`, `tag_write_worker.rs`, `metadata_transfer.rs`, `archive_cache.rs`, `rename_key_migration.rs`, `metadata_cleanup.rs` と unit test。

**規模**

8〜11 ファイル、約 600〜950 行。

**安全な停止境界**

DB migration は additive で、旧行は legacy fallback のまま読める。変換アーカイブの backfill/resolver が一意に判定できない行は推測で埋めない。UI は page を明示拒否し続けるので、identity 行だけが不完全な user-facing page 機能を作ることはない。この段階だけでも安全にリリースできるが、識別できない既存行が残る場合は段階 3 へ進めない。

### 段階 3: ローカル page tag を end-to-end で有効化する

**内容**

- native ZIP の `ZipImage` と `PdfPage` を page target として有効化し、段階 1 の全入口から同じ typed target を発行する。
- viewer metadata panel は ★と同様に container 行と page 行を分ける。見開きでは container 1 行と、表示中 1〜2 page を対象にする page 行を作る。
- page sidecar は常に `None`、content identity は `physical_source` の container を記録する。
- tag cache preload/cell lookup、optimistic update、Undo/Redo、metadata import refresh、badge、details sort を page key 対応にする。
- facet の count / untagged / Any-All filter / Tagged-Untagged を同時に page 対応にする。
- tag view を explicit metadata first、strict legacy parser second で `GridItem::ZipImage` / `GridItem::PdfPage` へ復元し、source container metadata を使う。legacy ZIP は container ごとに列挙をまとめ、cancel と scan limit を維持する。summary/query ではページであることだけを理由に落とさず、page inclusion policy を揃える。
- metadata transfer が作り得る変換アーカイブ page row は、段階 2 の backfill/resolver と source/cache identity 規則に従って materialize する。materialize しない規則を選んだ場合は、識別済みの変換 page をローカルの page-aware summary と item query の双方から除外できることを release の停止条件とする。識別不能な既存行が残る状態では release しない。
- local tag view の kind filter で page をどの分類に含めるかを決め、snapshot/test を更新する。
- remote の page 表示をまだ出さない場合、remote browse summary と tag-items query の両方から page identity を一貫して除外する。

**主な対象**

`tag_ops.rs`, `ui_metadata_panel.rs`, `ui_main.rs`, `app/metadata_ops.rs`, `app.rs`, `app/metadata_import_refresh.rs`, `tag_view.rs`, `tags_db.rs`, `metadata_transfer.rs`, `archive_cache.rs`, `remote_ipc/collections.rs` と UI/unit test。

**規模**

10〜15 ファイル、約 800〜1,250 行。

**安全な停止境界**

ローカルでは書き込み、表示、facet、横断一覧、open が揃う。変換アーカイブ page も materialize するか、summary/query の双方から同じ規則で除外する。remote も page を summary と item query の両面で明示的に非対応とするため、「ページだから一方だけに現れる」状態を残さない。この境界でページタグのローカル機能として単独リリースできる。missing/offline と limit による既存の件数差は残る。

### 段階 4: remote tag page を materialize する

**内容**

- tag collection entry に primary `RemoteAddress` を追加し、`ZipEntry` / `PdfPage` を保持する。optional field を無視する旧 client が page を親 container として開かないよう、capability/version negotiation で旧 client へ page row を返さないか、非互換 protocol として接続を拒否する。どちらかを停止条件とする。
- `TagItemKind` の page filter/grouping を決める。既存 `Image` / `Pdf` へ含めるか別 filter を足すかは仕様決定が必要である。
- remote tag-items の parser/explicit identity、thumbnail、click/open、return navigation を既存 page address pipeline に接続する。
- page を返せる版では remote browse summary と tag-items query の双方に page を含め、page inclusion policy を一致させる。
- ZIP entry 内 `::` / `page_3.jpg`、PDF page、missing/offline、scan limit、旧 client 互換の IPC/web regression test を追加する。

**主な対象**

`crates/remote-ipc/src/lib.rs`, `src/remote_ipc/collections.rs`, `src/remote_ipc/pipe.rs`, `crates/remote-web/src/http.rs`, `crates/remote-web/web/app.js` と IPC/web test。

**規模**

5〜9 ファイル、約 350〜700 行。

**安全な停止境界**

remote の summary/query、page address、open が揃い、旧 client は capability/version 境界で page row を受け取らない。段階 3 の「remote では page 非対応」契約を解除できる。missing/offline と limit による数値差はこの境界の不整合ではない。

### 全体概算

重複を除くと 18〜26 ファイル、約 2,000〜3,500 行。case-only ZIP entry collision の key format 自体を変更する場合は、この概算外である。

## 8. 実装前に確定が必要な事項

以下は根拠不足ではなく、現行コードから一意に決まらない仕様・設計判断である。

1. **explicit identity の所有先**: `tag_item_state` 拡張か専用 table か。推奨は専用 tableだが未確定。
2. **case-only ZIP entry collision**: 同じ lowercase key になる 2 entry を今回区別するか。区別するなら page key format 全体の migration が必要で、上記規模外。
3. **`ZipDir`**: ページではなく仮想 subcontainer である。今回タグ対象・tag view 対象へ含めるか未確定。
4. **変換アーカイブ内 page**: source archive key と cache ZIP key のどちらを tag identity の正本にするか未確定。metadata transfer は両 base を区別し（[metadata_transfer.rs:1516-1542](../src/metadata_transfer.rs#L1516)、[metadata_transfer.rs:4019-4050](../src/metadata_transfer.rs#L4019)）、実際に変換アーカイブ page tag の source-base 往復テストもある（[metadata_transfer.rs:7019-7033](../src/metadata_transfer.rs#L7019)、[metadata_transfer.rs:7088-7109](../src/metadata_transfer.rs#L7088)）。タグ UI の ownership は決まっていない。段階 3 では materialize まで解くか、page-aware summary/query の双方から明示除外する必要があり、native ZIP/PDF の書き込みだけを有効化した状態は停止境界にできない。
5. **見開き page 行**: 表示中 2 page を一括 target にする案は既存 normal-image panel と整合する（[ui_metadata_panel.rs:717-760](../src/ui_metadata_panel.rs#L717)）が、ZIP/PDF でも同じ仕様にするか未確定。
6. **tag view kind filter**: `ZipImage` を「画像」、`PdfPage` を「PDF」に含めるか、ページ専用分類を増やすか未確定。
7. **smart folder**: file scan の結果集合へ virtual page を含めるか未確定。「タグをどこにでも付ける」目的ではないため、初回は physical candidate 限定を維持するのが安全である。
8. **アプリ外 move/rename の tag-only origin**: content identity restore は台帳にある同一内容の旧コンテナから page tag/state を copy できる。ただし metadata import 等で tag row だけを持つコンテナが台帳 origin になる経路は確認できず、この条件は未確定。rename transaction 自体はアプリ外操作を対象外と明記している。
9. **特殊 filesystem/namespace**: `::` を含む物理名で tag view の metadata が成功する環境の実挙動は未確定。通常の Windows path では page key は `Missing` になる。

## 9. 調査時に実行した既存テスト

次の 5 件を個別実行し、すべて pass した。アプリケーションは起動していない。

- `rename_key_migration::tests::migrates_container_entry_keys_and_tolerates_like_wildcards`
- `metadata_transfer::tests::zip_and_pdf_virtual_tag_decision_state_round_trips`
- `rating_view::tests::restores_legacy_zip_image_key`
- `rating_view::tests::skips_ambiguous_legacy_zip_image_key`
- `tag_view::tests::tag_view_search_hides_but_preserves_missing_path_metadata`

## 10. 必須 regression coverage

実装時には少なくとも次を固定する。

- old-schema `tags.db` を開いて additive migration できる。
- ZIP page / PDF page の add, remove, clear, bulk toggle, Undo, Redo が同じ item key を使う。
- page write は sidecar を作らず、content identity は container source を使う。
- viewer panel は container と page を別行で表示し、片方の操作が他方を変更しない。見開き target 数も固定する。
- normal menu / `T` / quick pin / viewer panel が同じ page target を解決する。
- cache preload、import refresh、badge、facet count、untagged、Any/All、Tagged/Untagged が `ZipImage` / `PdfPage` で一致する。
- tag view は explicit ZIP/PDF page を materialize し、legacy `dir::page_3.jpg` を ZIP、厳密 `page_3` を PDF として復元する。
- case-only duplicate ZIP entry は誤った一方を開かず、決めた仕様どおり拒否または新 identity で区別する。
- missing/offline container は結果から隠すが DB 行を削除しない。ページキーが scan limit を不当に消費しない。
- 同じ ZIP の多数 legacy page row は archive 列挙を container 単位にまとめ、cancel と scan limit を守る。
- ZIP/PDF container rename/move は page tag/state/identity の suffix を保持し、destination 優先・冪等になる。
- content identity restore は選択した old→new container の page tag/state を copy し、tag-only origin の採否は確定した仕様どおりになる。
- container delete は page tag/state/identity を purge し、read-only missing 判定は purge しない。
- metadata transfer は explicit identity を含めて旧/新 manifest を往復し、変換アーカイブ page の source/cache 規則を保つ。既存 metadata-less 行の backfill/resolver は一意な行だけを分類する。
- サブフォルダ展開は physical-only の契約を保つ。将来 virtual page を含める仕様を選んだ場合だけ、通常グリッドと同じ page identity 規則を cache/facet に適用する。
- remote は各リリース境界で、page を「summary と item query の両方から除外」または「両方に含めて page address で開く」のどちらかになる。旧 client は page row を親 container として開かない。

# §1.473 閲覧履歴からの親移動の調査

2026-10-29。調査対象は公開版 `v4.9.0` (`7fea5839d`) のソースと、
作業開始時の master / `history-parent-1473` (`fec29871b`)。
調査中に master は `45366aff5` まで進んだが、今回追った app / grid / parent / PDF / archive / data_dir の経路に差分はない（版更新等は別作業）。
製品コードは変更しない。製品起動・デスクトップ操作・利用者の実設定へのアクセスは行わない。

## 報告者が回答済みの条件

- [>>643](https://egg.5ch.io/test/read.cgi/software/1782419551/643): ポータブル版で「履歴フォルダ」から開いたフォルダの親へ移動すると `mImageViewer_portable\data\cache` に入る。
- [>>656](https://egg.5ch.io/test/read.cgi/software/1782419551/656): 入口は**閲覧履歴**。主にマウスのサイドボタンへの割り当てで使う。ファイルメニューから閲覧履歴を開いても同じ。「履歴▼」から開く場合は起きない。
- 上記は利用者から提示された報告・回答に基づく。入口は聞き直さない。

## 区別するパスと所有状態

| 種類 | 論理場所 / 元ファイル | 読込実体 | 閲覧履歴から開いた root の帰路 |
| --- | --- | --- | --- |
| 閲覧履歴一覧 | `data/__reading_history__`（合成場所） | 履歴 DB からの rows | 本を開く前は `reading_history_return_from=None` |
| 通常フォルダ / 画像フォルダ本 | 元フォルダ | 同じフォルダの画像 | 元フォルダを `reading_history_return_from` に保持 |
| ZIP | 元 ZIP | 元 ZIP | 元 ZIP を保持。内部階層では ZIP 内の親が先 |
| PDF | 元 PDF | 元 PDF | 元 PDF を保持 |
| 変換書庫 | `archive_source_override` の元 RAR 等 | `current_folder` は `data/archive_cache/.../*.zip` | 元書庫を保持し、`effective_folder` と比較 |
| EPUB | 元 EPUB（`current_folder` も論理 EPUB） | PDF resolver の `ReadTarget.read_path` は `data/epub_cache/.../*.pdf` | 元 EPUB を保持 |

`data/cache` は `catalog::default_cache_dir` が返すサムネイル cache。
`data/__reading_history__`、`data/archive_cache`、`data/epub_cache` は別物である。
ポータブル / 通常版で変わるのは既定 data_dir の決定（`data_dir::flavor_default`）。
親移動の router に portable 固有分岐はない。

## 自動テストの境界

`src/app/tests/history_parent_1473.rs` に非対話テストを追加。
App の DB・worker は一時領域に限定し、App → epoch → override → TempDir → lock の順で破棄する。
data_dir は一時領域内の `exe/data` と `appdata/mimageviewer` の両配置。
素材の library は data_dir の外側（同じ一時領域内）に置く。
sidecar 設定は製品既定のまま、製本ルートも一時領域に指定する。
`feature=portable` の配布物ビルドや exe からの既定 data_dir 初期化は実行していない。

- 閲覧履歴入口: ファイルメニューの実メソッド、リング action dispatcher、マウス Back スロットへの `OpenLocationReadingHistory` 割り当て。
- 開く操作: Enter は合成 egui Key、ダブルクリックは既存の headless pointer helper から実 cell interaction handler を呼ぶ。
- 描画後の実 `post_render_main_navigation` を通し、classify / preflight / conversion / enumerate / sidecar adoption の既存 owner を poll する。
- 親: ボタンが呼ぶ resolver、合成 BS / Alt+↑、マウス Back スロット / ring の `GridParentFolder`。画像フォルダ本は本扱い設定を有効にし、全画面状態なら既存 close でページ一覧に戻してから親操作する。
- 「履歴▼」対照: UI のメニュー開閉自体は合成せず、その producer と同じ解決規則から `AddressBarNav::Direct(元パス, ExplicitList)` を address lane に渡す。
- RAR はリポジトリの実 solid fixture のコピーを使う。warm 条件は実 converter で ZIP と origin manifest を生成して DB に登録。cold 条件は空 cache から実変換・採用まで通す。
- EPUB は最小 EPUB と有効 PDF の世代を事前登録し、通常の resolver / PDF 列挙 / adoption を通す。EPUB 変換 worker 自体の初回変換は対象外。
- PDF / EPUB は既存 `RemotePdfTestBackend` を物理 PDF ごとに保持し、実 PDFium と製品共通の `ipc_enumerate` / page-size 処理を同一プロセスで通す。列数・ページ行の fake 応答は使わない。EPUB の既存世代は `pin_cached_epub_for_test` で実 DB resolver から取得する。製品 worker の起動、IPC transport、プロセス起動時の EPUB gate は検査対象外。
- 各段階で `effective_folder`、`archive_source_override`、`current_folder`、`display_epub_source`、`reading_history_return_from`、ZIP prefix、親候補、EPUB の物理 PDF と世代を出力する。

## v4.9.0 と master の差

以下の関数はタグと master で同一: `effective_folder`、`reading_history_back_nav`、
`resolve_grid_parent_nav`、`handle_grid_parent_folder_action`、`reading_history_grid_item`、
`reading_history_target_for_page`、`enter_reading_history_with_entrance`、
`commit_grid_virtual_open_effects`、`grid_physical_navigation`、
`open_archive_via_cache` / `open_archive_via_cache_owned`、`start_grid_virtual_open`、
`post_render_main_navigation`、`load_folder_with_scan_owned` / `load_folder_with_scan_claimed`、
フォルダバーの `resolve_folder_bar_nav_path`。
`install_loading_items_inner` の差は今回比較した範囲では final pipeline cache 失効 API の変更。

ただし **通常フォルダを Enter で開く producer は異なる**。
タグの `src/app.rs:57774` は `grid_physical_navigation` へ直接渡し、
Folder の `note_reading_history_open` を呼ばない。
master の `28a8d9ecc`（§1.480）では `accept_grid_container_open` にまとめ、
Folder の戻り元と auto fullscreen を設定してから同じ router に渡す。
タグのダブルクリックの Folder 分岐には戻り元の記録がある。

この差が作るのは「閲覧履歴へ戻るべきところで**元フォルダの実親**へ戻る」状態で、
内部 cache への移動と同一視しない。
`section1473_v490_folder_enter_producer_boundary` はタグと同一の production router を
タグの Enter の末尾と同じ呼出し条件で通す境界試験であり、v4.9.0 全体のビルド実行ではない。
タグと現 master は egui 等の依存にも差があり、今回の test executable は現 master の依存・ソースである。

## 初回の実行結果（本からの親1回まで）

この初回範囲では `data/cache` 移動を再現しなかった。**後述の追補で、閲覧履歴 root からさらに親へ移る経路を再現した。**

追加テストは **9 passed / 0 failed**（104.82秒）。6種類×4入口×5親操作×2配置の240条件に、
cold solid RAR の4入口×2配置の8条件を加えた248条件が完走した。
さらに ZIP 内部階層と、タグの Folder Enter producer 境界の2試験を通した。
ダブルクリックは親ボタン／マウス親操作の条件、Enter は BS／Alt+↑／リング親操作の条件で使う。
すべての開き方×親操作の直積を試したわけではない。

| テスト名（`app::tests::history_parent_1473::` 配下） | 条件数 | 結果 |
| --- | ---: | --- |
| `section1473_folder` | 40 | 通過 |
| `section1473_image_book` | 40 | 通過 |
| `section1473_zip` | 40 | 通過 |
| `section1473_pdf` | 40 | 通過 |
| `section1473_converted_archive_cache` | 40 | 通過 |
| `section1473_epub_cache` | 40 | 通過 |
| `section1473_cold_solid_rar_conversion` | 8 | 通過 |
| `section1473_zip_inner_parent_then_history` | 1 | 内部の親 → ZIP root → 閲覧履歴を維持 |
| `section1473_v490_folder_enter_producer_boundary` | 1 | 戻り元なし → 元フォルダの実親。cache へは移動しない |

観測した状態遷移:

1. 閲覧履歴の shell / rows 採用後は `current_folder=data/__reading_history__`、帰路は未設定。
2. ZIP/PDF の open intent は元パスと `GridVirtualOpenEffects.reading_history_return_from` を保持する。
   変換書庫は `MainGridArchiveTransitionIntent`、Folder は共有 acceptance で元パスの帰路を持つ。
3. 採用後は上表の source / backing 関係を維持し、`reading_history_return_from` と `effective_folder` が一致。
   `reading_history_target_for_page` が次に記録する対象も、cache ではなく元のコンテナだった。
4. 親 intent は `ReadingHistory(Return)`、採用後は同じ閲覧履歴へ復帰。
   「履歴▼」対照は帰路なしで、元のコンテナの実親へ戻った。
5. ZIP 内部では帰路を残していても `reading_history_back_nav=None`。
   BS は ZIP root まで戻り、その次の BS で履歴へ復帰した。

なお、**閲覧履歴 root 自体**の親候補は合成パスの親である data_dir だった（ログに記録）。
これは本の採用後の帰路とは別の状態であり、`data/cache` 移動の再現ではない。

既存テストも nextest で **10 passed / 0 failed**（20.455秒）。
filter は `test(reading_history_back_nav) | test(section1328_session_reading)`。
`reading_history_back_nav_handles_converted_archive` は単独でも通過、native exit code 0 を記録。
`cargo fmt --check`、`git diff --check` も通過。
製品コードの変更がない調査なので、全体 gate・製品確認用バイナリのビルド・実機 UI 試験は行っていない。

初回の PDF/EPUB の採用失敗は、そのまま製品不具合として数えていない。
App 単体テストでは main の EPUB startup gate を実行せず、通常 PDF pool の子プロセスも
製品 core には接続できない。既存の cache pin / 実 PDFium backend に接続した最終試験では全件通過した。

再実行（PowerShell、製品起動なし）:

```powershell
$env:MSBUILDDISABLENODEREUSE = '1'
cargo test -p mimageviewer --lib section1473 -j 1 -- --nocapture --test-threads=1
cargo nextest run -p mimageviewer --lib -E 'test(reading_history_back_nav) | test(section1328_session_reading)' --profile default --ignore-default-filter --run-ignored default --retries 0 --no-fail-fast --no-tests fail
cargo fmt --check
git diff --check
```

この作業ツリーの証跡（ignored、コミット対象外）:
`target/section1473-final.log`（各段階の trace と9件の成功）、
`target/section1473-existing.log`（nextest 10件の成功）、
`target/section1473-existing-converted.log`（既存変換書庫テスト、`native_exit=0`）、
`target/section1473-fmt-final.log`。
PowerShell の stderr リダイレクトが NativeCommandError の書式を付けるため、ログは UTF-16。

## 初回時点の未確認事項と追加確認案

- v4.9.0 の依存一式でのビルド実行、および製品 UI / 実マウス・実キーのイベント配送。
- 配布 portable の既定 data_dir 初期化、EPUB の初回変換 worker、失敗・取消・採用前に次の入力を入れる順序。
- 報告で問題になった1件の種類・元パス・保存済み履歴 row と、実際に到達した cache 内の場所。

追加確認を報告者へ頼むなら、**問題が起きる1件の種類と、閲覧履歴に表示される元のパス**に絞る。
通常フォルダの帰路か、変換 source/backing の境界か、既に内部パスが履歴に入っている場合かで
調査対象が変わるためである。入口は回答済みなので聞かず、広い再試験や設定変更も頼まない。
この追加確認案は初回時点のもの。追補で開発側の再現を得たため採用せず、報告者への追加質問は不要とする。

## 初回時点の修正判断

報告と同じ cache 移動が確認できるまでは、`effective_folder` や親 router に guard を足さない。
戻り元を失う経路なら open intent を作る入口または成功採用の境界を直し、
論理 source が cache 実体へ置き換わる経路なら source / backing を渡す所有境界を直す。
ZIP 内部は `zip_nav` の階層移動を維持し、root に達して初めて履歴帰路を使う。
新しい bool / pending / 回復処理を足す方針にはしない。

確定した帰路欠落の箇所は、タグの `src/app.rs:57774`（Folder Enter の要求生成）で、
現 master の `src/app/collection_grid.rs:1381` / `:1407` の共有 acceptance では既に補われている。
今回の報告の原因としての修正箇所・規模はまだ確定できない。
既存の open intent / adoption owner の source・帰路の受渡し漏れなら Small〜Medium を暫定目安とするが、
状態の所有構造をまたぐと分かった場合は、その境界と影響範囲を設計担当へ返して再見積もりする。

## 追補: 閲覧履歴 root の親（ClaudeCode 設計担当からの追加調査）

### 再現した経路

**閲覧履歴 root の親が実 data_dir を開くことを再現した。** 本から戻るまでの帰路ではなく、
合成の一覧自身の「親」を物理 path の親として解決することが原因である。

`section1473_reading_history_root_parent_never_opens_internal_data` の初回実行は、
通常／portable相当の2配置 × 一覧を開いた直後／ZIPからReturn後の2状態 × Menu／Ring／Mouseの3入口 ×
親ボタンのresolver／BS／Alt+↑／mouse親／ring親の5経路、**全60条件で同じ結果**だった。

| 操作 | current / effective folder | 選択 |
| --- | --- | --- |
| 入口のlibraryから閲覧履歴を開く | `data/__reading_history__` | 履歴のZIP |
| ZIPを開く | 元 `library/book.zip` | 本内ページ |
| 親1回目 | `data/__reading_history__`（正常復帰） | 開いたZIP |
| 親2回目（または一覧を開いて直ちに親） | **実 data_dir** | **`data_dir/cache`** |
| そのままEnter | **実 `data_dir/cache`** | cache内の項目 |

生成されたdata_dir内の実一覧は `['cache', 'logs']` で、`cache` が先頭になった。
`resolve_grid_parent_nav` は来た子の名前 `__reading_history__` を選択ヒントにするが、
これは実在しない合成markerなので一覧内に見つからず、通常の既定選択で先頭cacheになる。
親操作自体が直接cacheを開くわけではない。**親でdata_dirへ移動しcacheが選ばれ、Enterでcacheへ入る**。
報告者が追加の親やEnterを押したかは未確認であり、この観測を報告者自身の操作結果とは混同しない。

証跡: `target/section1473-root-before.log`。60件のroot-case、60件のcache選択→Enterが出力され、
最後の安全不変条件assertで失敗する（native exit 101）。失敗は意図した不具合再現である。
実toolbarを描画してクリックする経路も、追補の最終試験で同じ60条件が完走し再現した。
cacheが先頭になるかは実際の内部ディレクトリ構成と並び順に依存する。本fixtureでは60条件すべてでcacheだった。

### 原因と所有境界

- `src/app.rs:24478` `grid_parent_nav_target` と `:24517` `resolve_grid_parent_nav` は、本から戻る
  `reading_history_back_nav` を先に解決するが、**ReadingHistory root の親規則を持たない**。
  Bookmarks／Rating／Smart等の専用分岐を通らず、最後の `effective_folder()?.parent()` へ落ちる。
- `reading_history_synthetic_path` は `data_dir/__reading_history__`（`src/app.rs:13663`）。
  `.parent()` はdata_dirそのものを返し、普通の `Direct(..., ExplicitList)` として実フォルダ採用される。
- `resolve_grid_parent_nav` はmarker名を `select_after_load` に入れる。
  `try_select_after_load`（`src/app.rs:74404`）は一致する実項目がなくfalseを返す。
  先頭選択自体は通常一覧の既定処理（`ensure_selected_visible_or_first` 等）であり、不具合の原因ではない。
- これら親resolverは v4.9.0 と調査ブランチで同一。初回調査の「Folder Enterで帰路が欠ける」版差とは別件。
  ZIPを経由する本追補は、その版差のない経路で再現している。

### 他のrootの規則（独立したソース監査）

| 一覧 | rootの親と所有者 | 子からの帰路 |
| --- | --- | --- |
| 閲覧履歴 | root規則なしでdata_dirへ漏れる | `reading_history_back_nav` が元コンテナ一致時だけ履歴へ |
| ブックマーク | backがあればHistoryBack、なければno-op（共通親resolver） | Bookmarksへ復帰する専用owner |
| ★一覧 | `rating_view_parent_nav` が一覧を閉じ保存元へ | `rating_view_nav_stack` |
| Ctrl+G検索 | root no-op | GlobalSearchのdrill／Everything position |
| Ctrl+S検索 | root no-op | `favsearch.nav_stack` |
| タグ一覧 | BS／親ボタンはroot no-op | `tag_view.nav_stack`。mouse／ring側の同型漏れを実行再現。Padは同じ入口だが実行未確認 |
| ドライブ一覧 | `effective_folder=None`、親なし | 実ドライブルートからDriveListへ |
| コレクション | `current_folder=None`、親なし | Collection ownerの登録source／実子孫を区別 |
| スマートフォルダ | `SmartFolderViewState::parent_target` がroot no-op | Scoped／Container ownerの論理親 |
| サブフォルダ展開 | saved_folder、なければ実root | expansion owner |
| ★固定Snapshot | 親ボタン無効 | BS／mouse／ringは子から固定一覧へ戻す |
| 本棚root | 合成ではなく実フォルダ | 通常の物理親 |

主な根拠: `src/app.rs:37493`（Rating）、`:57382`（BS共通入口）、
`src/app/gamepad_input.rs:9688`（mouse／ring／Padの共通入口）、
`src/ui_main.rs:15183`（検索rootの親ボタンを無効化）、
`src/app/collection_grid.rs:1213` / `:3707`（Collection）、
`src/app/smart_folder.rs:8689`、`src/app/subfolder_expansion.rs:1984`。

### 他のrootの実行結果

`section1473_other_synthetic_roots_parent_inventory` は上記の閲覧履歴以外11種類 ×
親5操作 × data_dir 2配置、**110条件を実際の採用・入力・描画後dispatchで完走**した。
各入口でrootの採用をassertし、CollectionはReady／Emptyかつitems世代一致まで待つ。
親ボタンはoffscreenの実toolbarを描画し、enabledを確認して合成クリックする。

| root | 10条件の結果（通常／portable各5操作） |
| --- | --- |
| ブックマーク | 全操作でHistoryBackにより入口libraryへ。元のFolderを選択 |
| ★一覧 | 全操作で一覧を閉じ入口libraryへ。元のFolderを選択 |
| Ctrl+G検索 | rootを維持。current_folderは入口libraryのまま、surfaceはSearch(Global) |
| Ctrl+S検索 | `__search_results__` の検索rootを維持 |
| タグ一覧 | 親ボタン無効、BS／Alt+↑はroot維持。**mouse／ringの4条件は実data_dirへ移りcacheを選択** |
| ドライブ一覧 | 親ボタン無効。他操作もcurrent_folder=Noneのrootを維持 |
| コレクション | 親ボタン無効。他操作もcurrent_folder=Noneの完成rootを維持 |
| スマートフォルダ | 親ボタン無効。他操作も`__smart_folder__/id`のRootを維持 |
| サブフォルダ展開 | 全操作で保存元libraryへ。元のFolderを選択 |
| Snapshot | 親ボタン無効。rootでのキー／mouse／ringによる範囲外移動は既存のsnapshot admissionに拒否され、rootを維持 |
| 本棚 | 全操作で実本棚フォルダの物理親へ。本棚フォルダを選択 |

Tagの原因は `src/app/gamepad_input.rs:9688` の `handle_gamepad_grid_back` に
`tag_view_back` への分岐がないこと。BS側の `handle_grid_parent_folder_action` にはある（`src/app.rs:57413`）。
Tag結果は `data_dir/__search_results__` を使い（`:34081`）、mouse／ringは通常の物理parentへ落ちる。
最終ログではTag activeのまま実data_dirが採用されcacheが選択されている。Pad Bも同じ関数だが、Pad入力自体は本試験の対象外。
閲覧履歴rootの仕様判断とは別の入力routing欠落として扱い、今回は製品修正しない。

### 追補の最終検証と赤い再現テスト

最終ソースで `cargo test -p mimageviewer --lib section1473 -j 1 -- --nocapture --test-threads=1` を実行:
**13 tests、11 passed / 2 failed、118.67秒、native_exit=101**。
証跡は `target/section1473-root-audit-complete.log`（UTF-16）。

失敗は未修正の不具合を検出する次の安全契約2件だけ。最初の依頼にある「失敗のまま残してよい」に従い、
ignoreや期待値の緩和で隠さず残している。**このまま通常のlib/full gateに含めると、この2件で赤になる。**

- `section1473_reading_history_root_parent_never_opens_internal_data`: 60条件すべてで実data_dirへ漏れ、cache選択→Enterでcacheへ到達。
- `section1473_other_synthetic_roots_parent_inventory`: 110条件のうちTagのmouse／ring × 2配置の4条件で漏れる。

成功11件は、初回の9件に加えて次の2件。初回の変換cache・EPUB・通常コンテナの正常帰路とZIP内部移動も維持されている。

- `section1473_returned_history_back_candidate_consumes_self_marker_first`: raw HistoryBackの一回目は同じ閲覧履歴、二回目で入口library。
- `section1473_folder_bar_second_parent_keeps_physical_ancestors`: 「履歴▼」相当のDirectから開き、親2回を全5操作×2配置で実行。
  1回目はlibrary、2回目はその実親。一致する子名libraryが選択され、Enterもlibraryへ戻る。合成rootを経由せず内部cacheへ入らない。

既定sidecar設定を維持している。実toolbarが本棚一覧workerを開始するため、通常フレームと同じ
`poll_book_op_pending` をhelperに追加した。これがない途中の試験は、sidecarの静止待ちが本棚完了の回収を待っていた。
製品の待ち不具合としては数えていない。`poll_thumbnails`／spread-shift等も通常の非対話tickで回収する。
GlobalSearchは外部索引を起動せず、空結果を製品の行producerで採用した。Bookmark／Rating／Tag／Collection等は一時DBから採用した。

独立レビューはsourceと最終ログを読み、Collection完成条件・本棚worker回収・失敗2件の扱いを再確認し、未解決の重要所見なし。
製品コードの変更なしで停止する判断にも合意した。レビュー担当はテストを再実行していない。

`cargo fmt --check` と `git diff --check` は追補ソースでも通過（`target/section1473-root-fmt.log` にnative_exit=0）。
製品修正を実施していないため、修正時に指定されたtest-focused→test-lib-fullおよび検証用製品ビルドは行わない。

### 小さな修正として実施しなかった理由と提案

**製品コードは変更せず、設計担当へ戻す。** 独立レビュー（GPT-6.1 Sol / xhigh、read-only）も同じ判断。

1. `docs/reading-history-plan.md` §7.2 は「入口があればそこへ戻る」「入口なしはドライブ一覧またはデスクトップ」とする。
   Search／Smartのroot no-opをそのまま適用する案は、この既存仕様を変える。
2. Bookmarksと同じHistoryBackを足すだけでも、ZIPからReturnしたrootのback末尾に同じReadingHistory markerが残る。
   `MainHistoryOperation::Restore` はbackを変更せず（`src/app.rs:25359`）、`folder_history_back_target`（`:26250`）と
   `FolderHistoryPlan::advance`（`:1296`）も同値markerを飛ばさない。
   実処理の `section1473_returned_history_back_candidate_consumes_self_marker_first` でも、1回目は同じroot、2回目で入口へ戻る。
   余計な1操作を要求する修正は採らない。
3. 全synthetic pathの `.parent()` を禁止するguardも、実子孫を持つCollection／Smart／Ratingや各検索の固有規則を壊すため採らない。

推奨は、**既存 `TopLevelGridSurface::ReadingHistory` を合成rootの親を決める所有境界とし、入口への退出意図を共有の親解決へ渡す**案。
本からrootへのReturnと、rootから入口への退出を区別する。readonly／mutating両resolverで同じ規則にし、
通常の履歴←→、ZIP内部の親移動、他の合成一覧の既存挙動は変更しない。
新bool／pending／内部cacheを隠すguardは追加しない。

入口なしは、既存設計の許容範囲から**ドライブ一覧への復帰を提案**する（ドライブ一覧root自身は既存どおり停止）。
そのうえで、既存のtyped履歴計画で自己markerをどのように跨ぐか、forward履歴をどう維持するかを先に確定する必要がある。
rootの入口を保持する場合も既存のtyped origin／session ownerを使い、独立の帰路フラグを追加しない。
規模は暫定 **Medium**。単一ifの補充で完了するSmall変更としては扱わない。

## 修正の設計

2026-10-30、設計案。利用者判断により **v4.11.0予定の次版で修正する**。
調査コミット `1e29e853d` を継承し、v4.10.0公開後のmasterを取り込んだ
`eca8d7345` を設計のコード基準とする。この節は上記「小さな修正として実施しなかった理由と提案」を具体化する。
**本作業は文書だけ。製品コード・テストは変更せず、ClaudeCodeの設計確認と別担当の
GPT-6.1 Sol / xhighによる独立レビューを経てから実装する。**
初版 `0b5c71e07` の独立レビューはREVISE（P2×2、P1/P3なし）。構造的修正という方向には同意を得た。
本改訂では、展開一覧の復元state欠落と★一覧自身の帰路欠落を、完全な入口payloadの捕捉・配送で扱う。
以下のソース監査は実行結果ではなく、**改訂版の再レビュー・実装・動作検証は未実施**。

### 1. 決定する動作と不変条件

1. 合成一覧の親は、その一覧の論理的な所有者が決める。合成pathは表示・互換routing用であり、
   物理的な親の根拠にしない。内部dataを通常の親として開かず、cache選択を後段で隠して済ませない。
   利用者がアドレス等から実data_dirを明示的に開く操作を一般に禁止する設計でもない。
2. **閲覧履歴rootの親は、その閲覧履歴へ入ったときの場所へ1操作で退出する。**
   本からrootへ戻る操作とは別であり、本やその実子孫を往復しても入口を取り直さない。
   本を開いた行・scrollの復元は既存の `ReadingHistoryEntrance::Return` の規則を維持する。
3. **入口がないrootの親は `DriveList(None)` にする。** デスクトップは選ばない。
   ドライブ一覧自身は既存どおり親なし。Bookmarksの「履歴があれば入口へ」、★一覧の
   「保存元へ」という意味に揃え、入口なしの扱いは閲覧履歴設計§7.2で許容されているドライブ一覧に確定する。
   他rootの入口なしno-op規則を変更して一律化はしない。
4. 検索は既存どおり透明な一時overlayとする。検索中から場所を切り替えて閲覧履歴を開く場合、
   退出先は検索が所有していたcanonicalな元の場所とする。終了済みquery/resultを新たに復活させない。
   Snapshot等からの入場可否も既存のadmissionを維持する。
5. タグの親は既存 `tag_view_back` が所有する。rootでは停止、子では検索内の帰路を1段戻り、
   ZIP内部は既存ZIP親が先。マウス・リング・Padもキー・ボタンと同じ意味で実行する。
6. 親の可否・行先・選択ヒントの算出は1つ。入力callerは可否を別のif列で推測せず、
   readonly previewと実行は同じ解決結果を使う。previewは状態・履歴・選択を変更しない。
7. 履歴cursor、場所、facet routeは既存 `MainListNavigation` の成功adoptionで一緒に確定する。
   通常の履歴←/→の1件移動は変えない。root退出だけが入口までのまとまった履歴計画を作る。
8. 元RAR/EPUB、読込cache ZIP/PDF、ZIP内部prefix、閲覧履歴への本の帰路、rootからの退出先を混同しない。
   非rootのZIP内部移動をroot退出で飛ばさない。入力元やdata_dir配置で意味を変えない。
9. main / A / Bの履歴、viewerの帰路、workerは既存の所有contextだけに作用する。
   mainの一覧操作のために独立viewerの履歴・PDF要求・再生・textureを変更しない。
10. 入口は入場時点のimmutableな型付き復元payloadとして所有する。一覧を再表示する情報だけでなく、
    復元した一覧自身の親・closeに必要な帰路も含める。退出時のAppの別restore slot、
    途中で開いた本の保存元、偶然残った旧view stateを補充元にしない。

### 2. 現在の違反箇所と、変える所有境界

以下の行番号は `eca8d7345`。調査コミットとの差分を確認したところ、この親・履歴・openの関数群は変更されていない。

| 所有者 / 境界 | 現状と修正責務 |
| --- | --- |
| `src/app.rs:24480` `grid_parent_nav_target` / `:24519` `resolve_grid_parent_nav` | 同じif列を二重に持ち、ReadingHistory rootを落として `effective_folder().parent()` へ進める。共有の親解決へ置換する |
| `src/app.rs:57384` `handle_grid_parent_folder_action`、`:57330`付近のstack再構成中のキー入口、`src/ui_main.rs:15185–15270,15837` の親ボタン | 検索・ZIP・選択を各入口で実行する。入力可否と表示の条件は保持し、意味の解決・実行を共通化する |
| `src/app/gamepad_input.rs:9688` `handle_gamepad_grid_back` | Tag分岐がない。分岐をコピーせず共有親受付への薄いadapterにする |
| `src/app.rs:34500` `enter_reading_history_with_entrance` / `:34848` のroot install、`app/top_level_grid_view.rs` のReadingHistory surface/restore | rootに入口の意味がなく、`replace_surface` で汎用 `return_to` も消える。ReadingHistoryの型付きroot identityに退出情報を持たせる |
| `src/app.rs:24331` `note_reading_history_container_open_path`、`GridVirtualOpenEffects`、`MainGridArchiveTransitionIntent` | 本への帰路が元コンテナpathだけ。既存帰路payloadを型へ置換し、どのrootへ戻るかも同じopen要求で運ぶ |
| `src/app.rs:1259` `FolderHistoryPlan` / `:26305` `dispatch_main_folder_history_input` | 普通のBackは1件pop。root退出用の計画を同じownerに加え、自己markerを跨ぐ規則とforwardへの移動をここで所有する |
| `src/app.rs:25304`付近 `adopt_main_list_navigation_with_completion` / `:26400`付近 `dispatch_folder_history_entry` | 既存の採用境界を再利用する。root退出のための別load、stackの先行pop、後からのrollbackを追加しない |
| `src/app.rs:1431` `FolderNavHistoryTarget::from_restore`、`:42489`付近の展開state消去、`app/subfolder_expansion.rs:2443`付近の復元 | 履歴地点への投影で展開のroot/roots/snapshot等を失い、後の復元がApp slotを読む。入口payloadを投影前に保持し、既存prepareへ明示的に渡す |
| `src/app.rs:26421`付近のRating dispatch、`:37320`付近の物理採用、`:37448` `close_rating_view` | 星数だけの地点からAppの保存元を読んでも、本の採用後には帰路がない。入口時の保存元/展開restoreをRatingの既存transitionへ渡し、一覧と帰路を同時採用する |

原因は選択の先頭補正やconversionではなく、rootの親・入口を所有していないことと、入力ごとの意味の分散である。

### 3. 閲覧履歴rootと本の帰路を既存の型へまとめる

**既存のReadingHistory variantと帰路payloadを拡張・置換する。Appに入口用bool / Option / pendingを並設しない。**
以下の型名は実装時の目安であり、責務と保持する内容を設計契約とする。

- `ReadingHistoryRootRestore` は、今回の入場を表すvisit identityと、退出先を所有する。
  visit identityは既存context ID・surface generation・slot-switch sequenceから入場時に作る値とし、
  時刻やpath同値から推測しない。比較・履歴内の境界識別にだけ使い、新しいworkerや待機を作らない。
- 退出先は排他的な `ReadingHistoryExit` とする。
  **`RecordedEntrance(TopLevelGridOrigin)`**（通常の明示入場で入口をBackへ積んだ）、
  `CanonicalOrigin(TopLevelGridOrigin)`（検索等からの切替で履歴cursorを進めず元場所を引き継いだ）、
  `DriveList`（入口なし）の3種類。ここでの `TopLevelGridOrigin.restore` は下記§3.1の完全なpayloadへ拡張する。
  初版の `RecordedEntry(FolderNavHistoryEntry)` は撤回する。履歴のlocation/routeへの投影と、
  native ownerを復元する情報を区別し、**履歴記録あり/なしの両方で同じ完全性を要求する**。
  cursor用entryは完全なoriginから導出し、独立した第二の退出先として保存しない。
- `TopLevelGridSurface::ReadingHistory`、`TopLevelGridRestore::ReadingHistory`、
  `FolderNavHistoryTarget` の閲覧履歴地点はこのroot restoreを運ぶ。
  履歴の `Path(__reading_history__)` をReadingHistoryの型付きvariantへ置換する。
  `current_top_level_restore_snapshot` / `folder_nav_current_target` は可視rootのこの型を投影する。
  退出先を汎用 `return_to` とroot payloadの二か所に同時保持しない。
  `folder_nav_targets_eq` は閲覧履歴の同一visitを比較し、facet scope・表示address・favorite設定キーは
  従来の論理rootのままとする。visit identityをDBキーや最近開いた実フォルダへ流さない。
- 既存 `reading_history_return_from: Option<PathBuf>` は、元コンテナとroot restoreを対で持つ
  `Option<ReadingHistoryBookReturn>` に置換する。`GridVirtualOpenEffects`、変換書庫intent、
  physical adoption、viewer bundleも同じpayload型に揃える。既存の元path一致・ZIP root判定は維持する。
  rootから本への要求を作った時点でroot restoreも捕捉し、cache変換後に現在の一覧から取り直さない。
  `AddressBarNav::ReadingHistory` 等のReturn要求にもこのrootを含める。`Menu/Return` という目的の区別は
  維持し、payloadを持たないReturnから現在地を入口として取り直す経路を残さない。
- root位置の `SessionGridPosition` は既存 `TopLevelGridView` のcontext内所有を維持する。
  退出payloadへ位置の第二の正本を並設しない。ただし展開のaccepted snapshotやBookmarksの既存stable-key位置等、
  入口復元そのものに必要なimmutable payloadは下記のとおり保持する。worker、購読、texture、mutableなviewは持ち込まない。

入場・復帰・破棄の規則:

| 遷移 | root / 帰路の扱い |
| --- | --- |
| ファイル/場所メニュー、mouse/ring/gesture/Padの「閲覧履歴」 | 既存の入口ごとのadmissionを通った後、同じMenu受付で元表示を退役させる前に完全な入口originを捕捉。履歴抑止の決定と一緒に退出種別を確定し、root採用時にだけ公開する |
| 既に同じrootにいるときの再度Menu、検索overlayのcanonical originが同じrootの場合 | 新しい「自分を入口にしたroot」を作らず、そのrootを引き継ぐ。Menuの位置を先頭へ戻す既存仕様は維持する |
| rootからFolder/画像フォルダ本/ZIP/PDF/RAR/EPUBを開く | §1.480の `accept_grid_container_open` と既存virtual/archive producerで同じrootを帰路へ移す。Enter・ダブルクリックで差を作らない |
| 本の実子孫/ZIP内部を移動、同じ本をreload | 既存の帰路有効範囲を維持。深いZIPではroot帰路を使わない。帰路を有効に保持する既存分岐ではpayload全体を保持し、元pathだけ復活させない |
| 本から親、または既存Esc直帰 | `reading_history_back_nav` は保持したrootをReturn目的で返す。入口を本で上書きせず、既存root位置を復元する |
| 履歴←/→でrootを復元 | 保存entryのroot restoreを復元。移動開始時の現在地を新しい入口にしない |
| 起動指定/合成path互換入口等で、保持rootも元場所もない | `DriveList` 退出を持つrootを作る。pathの親を代替にしない。session内の型変更でありDB migrationを増やさない |
| 独立した場所移動、別rootへの入場、取消/未採用、contextの退役 | 現在の帰路clear/要求drop境界でpayload全体を移譲または破棄する。独立移動で古いroot帰路を再使用しない。旧rootの履歴entryはそのentryのimmutable snapshotとして残る |

`CanonicalOrigin` は既存の検索closeで使うoriginのmoveを使い、元画面を一度再構築してから履歴を開かない。
実Folderの子へ開き直したとき等、現在の製品が `reading_history_return_from` をclearする条件は拡張しない。
たとえばFolderの子から物理親へ戻っただけで、失効した閲覧履歴帰路を新たに復活させる変更はしない。
rootへ復帰する場合の入口情報を保持することと、その復帰操作を新しく許すことは別である。
型の再帰（他の一時ビューのoriginが閲覧履歴である場合）は必要なvariantをBox化する。
root入場自身は同じrootを再利用するため自己参照を作らない。履歴内の複製はimmutableな場所snapshotであり、
実行中要求のownerを共有しない。

#### 3.1 完全な入口payloadの型と同型監査（REVISE P2×2への改訂）

`TopLevelGridRestore` を入口の復元に必要な意味を持つ型へ拡張する。型名の目安と保持内容を以下に確定する。
`TopLevelGridOrigin` の `route` はその一覧のfacet lineageであり、Appの現在routeを退出時に取り直さない。
既存の `current_top_level_restore_snapshot` は一時ビューではcanonical originを返すため、可視一覧のnative ownerを
捕捉する場合と、一時ビューのoriginを移譲する場合を区別して同ownerのcapture処理へまとめる。
汎用 `from_restore` を呼んだ後に、失った情報を別slotから再取得する順序にはしない。

| 入口 / 根拠となる所有者 | 入場時に所有するimmutable payload / 対象範囲 |
| --- | --- |
| サブフォルダ展開（P2-1） | 既存 `SubfolderExpansionRestoreState` 全体：`root`、`roots`、`saved_folder`、accepted `snapshot`、`removed_paths`。`TopLevelGridRestore::SubfolderExpansion(state)` をそのまま保持し、合成pathへの投影で捨てない。snapshotは既存のimmutableなデータ共有/所有移譲を使い、再scanを必須にしない |
| ★一覧root（P2-2） | `RatingRootRestore { stars, saved_folder, subfolder_restore }` を `TopLevelGridRestore::Rating` のpayloadとする。後二者は捕捉時の `rating_view_saved_folder` / `rating_view_subfolder_restore` の値（Noneも含む）。既存の `RatingNavigationTransition` が受け取る帰路と同じ意味を保持し、復元後の `close_rating_view` が元Folder/DriveList/展開へ戻れるようにする |
| ★一覧の物理子 | 既存 `RatingPhysicalRestore` の `visible_path`、stars、nav chain、saved folder、subfolder restoreをすべて保持する。子→★root→元場所までの帰路を維持し、pathだけの通常Folderにしない |
| Bookmarks root / 読込中 | DBから既存browser buildで再構築できるroot identityと、既存stable row keyによる一覧位置を保持する。位置は `BookmarkViewReturnGridState` のrow keys/selected key/scrollの復元規則を再利用する（項目openを伴わない入口で架空のopened keyを作らない）。rootの親はmain履歴を使うので別のsaved-folderは不要。`BookmarkViewState::Opening/Detached` のviewerやseek pendingをrootのworkerとして保存しない |
| 検索 Ctrl+G/Everything・Ctrl+S・Tag | **検索結果自体は退出先の対象外**。既存のcanonical `TopLevelGridOrigin` を完全な型のまま移譲する。検索開始時のcaptureとcanonical/fallback生成も上記Rating/展開等のpayloadを保つよう揃える。既存fallback slotを読むのは元ownerが有効な捕捉時だけとし、退出時には読まない。query、dataset、drill workerを閲覧履歴へ保存/再起動しない。リング等の検索中場所ジャンプ拒否は維持し、許可されるファイルメニューからの入場を検査する |
| Collection root / 物理子 | `CollectionGridRestore`（ID、revision hint、entry anchor）または `CollectionGridPhysicalRestore` 全体に、同Collectionの `TopLevelGridView::return_origin` を対にしたrestore payloadを使う。rootの親はno-opだが、後の一時ビュー移譲/同Collection再入場に使うoriginまで失わない。actorのwatch/receiver/installed mutable sessionは保持せず、既存のlatest revision prepareで再構築する |
| Smart root / scoped / container | `SmartFolderViewState`（定義ID、既存immutable navigation entries、position/back stack）に、捕捉時の `smart_folder_saved_folder` とそのsurfaceの `return_origin` を含むrestore payloadを使う。前者は定義消失時等の既存の保存元、後者は一時ビューとのorigin移譲であり役割が異なる。rootの親はno-op、子→rootの既存再構築を使い、resident sessionが残っていることを前提にしない。定義更新/削除への新しい回復規則は足さない |
| 通常の実Folder/本、DriveList、本棚 | 普通の物理一覧は論理sourceから既存scan/preflightで復元し、DriveListも既存synthetic prepareを使う。本棚は実Folder。物理一覧にReadingHistory/Bookmark等の専用親ownerが付いている場合は、その既存帰路payloadも含めたtyped restoreにする。元cache pathだけを保存せず、独立した明示入場によってその帰路を消す既存条件は拡張しない |
| Snapshot（★固定） | **固定一覧そのものを閲覧履歴の退出payloadへ複製する対象にはしない**。リング/場所メニューは既存のSnapshot場所ジャンプ拒否を維持する（`gamepad_input.rs::location_navigation_blocked`、`ui_main.rs:15326`）。ファイルメニューの閲覧履歴は別経路であり、一律に「Snapshot中は入れない」とはしない。現状この入口はSnapshotをdismissせず、固定ownerを同contextに保持する。親の優先順位は既存 `snapshot_return_to_list_view` が先で、固定一覧へ戻す。Snapshot解除を伴う既存の一時ビュー切替でcanonical originを受け渡す場合は、その元場所を完全なpayloadで引き継ぎ、Snapshotを新たに再作成しない |

Collection/Smartのlocation（履歴同値判定用）と、保存元を含むrestore payloadは分ける。
`TopLevelGridRestore` 側を `CollectionRootRestore` / `CollectionPhysicalEntranceRestore` /
`SmartFolderRestore` 等のpayloadへ置換し、履歴ID/positionの比較へoriginの有無を混ぜない。
再帰するoriginはBox等で有限なimmutable snapshotとして持ち、globalなlookup表や追加のApp restore slotを作らない。
物理一覧の専用親は既存の排他的な親ownerとして運び、新しい帰路bool/Option群で並列表現しない。
Bookmarksの位置は既存のstable-key read-modelへ渡す値であり、workerや旧itemsを新たな位置ownerにしない。

**captureする境界も修正範囲に含む。** 検索やCollectionを開いた時点でoriginを軽量な星数/pathだけに
潰してしまうproducerを残すと、そこから閲覧履歴へ入った時には補完不能になる。
`take_origin_for_search_entry`、canonical/fallbackを作るview-return helper、Collection/Smartのorigin captureを
同じ完全な `TopLevelGridRestore` のproducerへ揃える。未採用の要求の見込み値で可視入口を置換しない。
通常の検索close等も同じ型を運ぶが、入場可否・検索結果を閉じる仕様・親の意味は変えない。

#### 3.2 既存prepare/adoptionへの配送

`FolderNavHistoryTarget::from_restore` はcursorの場所identityを投影するhelperとして使えても、完全な復元要求ではない。
`RecordedEntrance` / `CanonicalOrigin` からの退出ではoriginを投影後に捨てず、既存dispatcherの入力を
「通常履歴entry」または「完全なorigin」というtyped destination値で受けられるよう拡張する。
入力型は `MainListRestoreDestination::History(FolderNavHistoryEntry)` /
`MainListRestoreDestination::Origin(TopLevelGridOrigin)` の排他的な2種類とする。
後者のentry/routeはoriginから導出する。この値は1要求の引数であり、Appの新しいpending/補助slotではない。
`MainHistoryOperation::Replay(plan)` は引き続き同じ `MainListNavigation` が運ぶ。
両入力は同じ既存dispatcher内で各native ownerへ配送し、別の復元dispatcherや直接loadの実装列を追加しない。

- 展開は `restore_subfolder_expansion_navigation` の既存owned復元入口に**明示state引数**を通し、
  `start_subfolder_expansion_prepare_owned` / `start_subfolder_expansion_scan_roots_owned` へ渡す。
  root/roots/snapshotだけでなくsaved folder/removed pathsも既存の `SubfolderExpansionAdoption` の
  Return payloadに含め、成功installで元の展開stateを採用する。明示stateのある要求では
  `folder_nav_subfolder_restore` / 現在の展開stateへfallbackしない。新payloadを一度App slotへ書いてから読む経路も作らない。
- ★rootは `start_rating_navigation_owned` に捕捉したstars/saved folder/subfolder restoreを渡し、
  既存 `RatingNavigationTransition` → Rating build/prepare → adoptionを使う。
  今の `dispatch_folder_history_entry` にあるApp slotからの補充をこの退出に使わない。
  saved folderがNoneならNoneを採用し、途中の本や無関係な★一覧の帰路を借りない。
- Collectionは既存 `start_collection_history_transition_owned` の `return_to` に捕捉したoriginを渡す。
  物理子の既存session prepare/adoptionにも同じoriginを運ぶ。Smartは既存のstaged history/return要求で
  restore payloadを保持し、prepare後のinstallでstate・保存元・originを同時に採用する。
  Bookmarksは既存synthetic prepare/browser buildとstable-key位置採用へ渡す。
- 物理sourceと専用親payloadも、既存のphysical/virtual/archive要求で保持し、現在ある帰路のinstall境界で採用する。
  rootからの退出を新しい直接loadへ分岐させない。prepare失敗/取消/別要求による退役で、
  Appの帰路slotやcommitted履歴を先に変更しない。

入口payloadは現在のroot restore、そこから開いた本の帰路、rootの履歴entryまで同じ意味で受け渡す。
本のadoptionがAppのRating/展開等のfieldをclearしてもpayloadには影響しない。
退出計画の仮currentが古い軽量entryを指しても、実際にdispatchする宛先はrootが保持した完全なoriginとする。
通常Back/Forward全般の復元仕様の作り直しは本件に含めず、1件移動の規則を変えない。

### 4. root退出、自己marker、forward履歴

root ownerの `plan_root_exit` は、保持した退出情報と**現在の有効なmain/slotの履歴**から
`FolderHistoryPlan` を作り、既存 `MainHistoryOperation::Replay` とtyped destinationのdispatcherへ渡す。
通常の `advance(Back/Forward)` を全場所で「同じpathをskip」に変えない。

`RecordedEntrance` の場合（cursor操作には完全なoriginから導出したentryを使う）:

1. 現rootのvisitに属するBack内のroot markerを境界とする。同じ合成pathの別の入場を境界にしない。
   本/実子孫の往復で境界より後ろに積まれたentryを、仮cursor上でBackした順に扱う。
2. 仮currentと同じvisitのrootが直ちに現れる場合は自己markerとして消費し、rootを再loadしない。
   この消費では仮currentのroot/routeを古いmarkerで上書きしない。Forwardへ残すのは現在表示中のrootである。
   その後、保持した入口entryまでを1つの退出計画にする。子の履歴を見つけたところで止まらない。
3. 普通のBack/Forwardで既に境界markerを消費してrootへ来ている場合は、Back末尾の入口entryを消費する。
   その際もentryのlocationとrouteを扱い、別visitの同pathを探し回らない。
4. 上限100件によって境界も入口entryも既に落ちていた場合は、残存する当該visit以後の履歴を
   forward側へ移す計画とし、保持した入口へ退出する。prune済みの古いBack prefixや別slotの履歴を復活させない。
   境界不在かつ末尾が入口でない場合のこの扱いも正常な長時間利用の契約としてテストする。

`CanonicalOrigin` は、当該visitのmarker/子の履歴までを同じ計画で処理するが、入場時に積んでいない
元場所のentryをBackからpopしない。canonicalなtyped originを既存復元経路へ渡す。
両退出種別とも§3.2の完全なpayloadをdispatchし、location/routeが同じという理由でApp残存stateへ置き換えない。
`DriveList` も当該visitを処理してドライブ一覧を採用する。どちらも合成pathの親へは落ちない。
これらの計画も、宛先のprepare/adoptionと同じ要求でcursorをcommitする。

**forwardの規則は、自己markerを除き、途中を描画せず同じ回数のBackをした結果に揃える。**
既存forwardをclearせず、その末尾へ仮currentを既存の重複整理規則で積む。
連続する自己markerのための余分なrootを追加しない。実際に別の本や子を挟んだ複数回のroot訪問は残し、
訪問自体や本の履歴を一括削除しない。出口へ移動した後の通常のForwardは1件ずつ戻す。

表記はstackの右端が次にpopされる側。`H`は同一visit、`O`は入口、`B`は本、`C`は実子孫。

| 操作前 `current / back / forward` | 親1回の採用後 | 続くForward |
| --- | --- | --- |
| Menu直後 `H / [X,O] / []` | `O / [X] / [H]` | `H` |
| 本からReturn `H / [X,O,H] / []` | `O / [X] / [H]` | `H`。自己marker分の余計な1操作なし |
| 普通のBackで本からroot `H / [X,O] / [B]` | `O / [X] / [B,H]` | `H`、次が `B`。既存forwardも保持 |
| 計画単体の複数entry例 `H / [X,O,H,B,C] / []` | `O / [X] / [H,C,B,H]` | 入場root、`B`、`C`、最後のroot。途中の訪問を削除しない（実Folder子からReturnを新たに許す意味ではない） |
| 起動直後 `H / [] / []` | `DriveList / [] / [H]` | 元の `H`。その親は再びDriveListで、data_dirには進まない |

rootの位置・facet routeも同じadoptionで復元する。入口に戻るときに子名 `__reading_history__` を
選択hintにせず、入口側の既存の保存選択/scrollを使う。
rootへのForwardはReturn目的の位置復元とする。Menuで明示的に再入場した場合の先頭化は変えない。

退出中に同じrootの親をもう一度受理した場合、既存の要求/provisional planを同じroot退出として扱い、
同じsuffixを二重popしない。新しいpendingは作らず、同宛先要求の再利用/置換は既存navigation ownerに委ねる。
明示的な履歴←/→は既存のprovisional cursorを進める規則を維持する。
別navigation、slot切替、取消、入力禁止の扱いも既存の `MainListNavigation` の検証・退役に従う。
宛先が存在しない等の既存の拒否では未採用計画を捨て、別の内部フォルダを代替で開かない。

### 5. 親の解決を1つにする

`app`内に純粋な `GridParentAction` の解決と、その実行を置く（必要なら小moduleへ抽出）。
これは1入力分の値であり、Appへ保存する状態ではない。
actionは `NoOp` / 検索ownerへ戻す / Tag ownerへ戻す / Rating ownerへ戻す / ZIP内Back /
ReadingHistoryRootExit / 既存typed navigation等を排他的に表す。
物理親actionには行先と子選択を一緒に含める。`None` が「該当ownerなし」と「そのownerのrootで停止」の
両方を表して物理fallbackへ流れる構造をなくす。

適用順は既存キーの意味を基準にする。Snapshotのlist復帰、検索（Global/Everything・Favorite・Tag）、
Rating、ローカル絞り込み、ZIP内部、各typed root/帰路、最後に通常の物理親、という所有優先を1か所で決める。
各owner内のZIP Backを二度呼ばない。Collection/Smartのroot停止やsourceからの復帰など、
既存ownerが所有する境界を通常の物理親より先に終端させる。
Snapshot rootの既存の範囲拒否・通知は維持し、全入力の無言no-opへの置換はしない。

入口の具体的な扱い:

- 通常キーの `GridParentFolder`（BS / Alt+↑ / カスタム割当）、一覧余白double-click/double-tap、
  mouse button / ring / gesture / Pad B・割当操作、親ボタンクリックは同じ親action受付を使う。
  `handle_gamepad_grid_back` は意味を持つif列を撤去する。新しいKeyActionや保存設定は増やさない。
- ボタンのenabled/tooltip用previewも同じresolverを使う。検索rootのボタン無効、Snapshot中のボタン無効等の
  既存表示制約は維持する。クリック後の `search_drill_up` と `zip_nav_back` の別実行列は共通実行へ移す。
- stack再構成中の `handle_stack_refresh_keyboard` は既存allowlistを残し、許可された親だけ同じ受付へ通す。
  再構成中の項目操作を追加で解禁しない。
- `global_search_ui/everything.rs:574` の物理子孫からの親は、上位の検索受付へ再入すると再帰する。
  Everything ownerがscopeを確認した後、共有resolverの**物理子の親**という下位操作へ委譲する形に分ける。
  それを通常の入力入口として公開せず、結果rootへの帰路・datasetの保持を変更しない。
- `resolve_close_return_navigation` / `resolve_return_to_parent_nav` は全画面closeのownerを維持する。
  共有すべきReadingHistoryのtyped帰路だけを運び、全画面closeをgridの親入力へ丸ごと置換しない。

入力可否、IME/text focus、modal、repeat、viewport入力所有、Snapshotのボタン表示制約はcaller側の契約を残す。
また既存のdetached保護/closeは意味の解決と分けた既存presentation境界に残す。
キー経路の `close_detached_viewer_for_virtual_page_list_parent_nav` 等を、共有化のためにそのまま
mouse/ring全体へ広げない。逆にキーから削除もしない。input originが必要なら1入力のenumで渡し、
それで行先やTag停止を選ばない。意味の共有と無関係な窓の寿命変更は今回の修正へ含めない。

### 6. 同型経路と出荷済み挙動への影響

| 一覧 / context | この設計での扱い |
| --- | --- |
| 閲覧履歴root（直後/本からReturn/履歴replay/入口なし） | 型付きroot ownerが入口またはDriveListへ退出。すべての親入力で同じ |
| 閲覧履歴から開いた通常Folder・画像本・ZIP/PDF・RAR等・EPUB | 元コンテナとroot帰路を対で運ぶ。ZIP深層→root→履歴という順とEsc直帰、cacheの論理元を維持 |
| 「履歴▼」、アドレス、ツリー、お気に入りから開いた実場所 | 閲覧履歴rootの帰路を新規付与しない。通常の実親と来た子の選択を維持 |
| Tag root/子/ZIP内部 | 既存 `tag_view_back` へ一本化。root停止、子は1段、検索自体は閉じない |
| Ctrl+G / Everything、Ctrl+S | 各検索ownerのroot停止/子戻り。Everythingの起点から結果への帰路と保持datasetはそのまま |
| Bookmarks root/読込中/開いた本 | 履歴があれば戻り、なければno-op。専用本帰路を維持。今回のroot退出skipを流用して別仕様へ変えない |
| ★一覧root/子/読込中 | `rating_view_parent_nav` とsaved origin/nav chainを維持。§3.1の捕捉値をRating採用時に戻し、さらに親で元場所へ閉じられることまで対象にする |
| DriveList、Collection root/子/Deleted、Smart root/scoped/container | 各ownerの停止・typed親・既存Deleted規則を維持。Collection/Smartは自身のorigin/保存元も§3.1のpayloadで戻す。root不成立を物理pathの親へ読み替えない |
| サブフォルダ展開、Snapshot、本棚 | 展開はstate全体を復元し、その後の親も保存元へ。Snapshotは§3.1の受付差と同contextの固定ownerを維持し、退出先として再作成しない。本棚は実親を維持 |
| main normal / quick slot A / B | root退出は発行元slotの履歴だけ。別slotへ切替後の旧要求は既存検証で退役し、他slotをpopしない |
| main内/linked全画面、独立F12/複数静止画窓、動画/ParkedLive、Remote | 一覧の親はmain owner、viewer close/物理移動は既存context owner。下記§7の境界を維持。Remoteの一覧/IPC/履歴へ新機能を足さない |

意図して変わる出荷済みの結果は次の範囲に限る。

1. 閲覧履歴rootで親を実行すると、内部dataではなく入口/DriveListへ移動する。
   子往復後も1操作で入口へ退出し、rootの自己markerのための空振りを要求しない。
2. Tag rootでmouse/ring/Padの親を実行しても内部dataへ移らない。Tag子でも既存キーの帰路に一致する。
3. 閲覧履歴rootの親ボタンpreview/tooltipは入口への退出（またはDriveList）を示す。
   合成pathの実親を表示せず、実行結果と一致させる。
4. 閲覧履歴rootからの親退出に限り、§4のまとまったBack計画がforwardを作る。
   通常の履歴←/→や他rootの履歴計画は変更しない。

**通常利用を遅くする、操作を増やす、既存の一覧や本を開けなくする変更は採らない。**
新しい確認dialog/モーダルや、変換中以外のナビゲーション停止は設けない。
実装時の調査でこの範囲を超える挙動変更、検索結果そのものの復活、全画面/別窓の新しいclose、
履歴entryの一括削除が必要と分かった場合は、実装を止めてClaudeCodeへ返す。
現在の設計では、通常の使い方を不便にする変更を必要としない。

### 7. F12・複数ウィンドウとdetached憲法

`reading_history_return_from` の型置換は `src/app/viewer_context_registry.rs` の
`ViewerContextBundle` の同field、構築・swap・fork/moveにも機械的に波及する。
このため「detachedと無関係」とは扱わない。ただし入力対象、HWND、viewport、placement、
park/promote/retireの判定を変更する設計ではない。

- 独立の物理viewerは既存のFolder/DetachedPhysical scopeを持ち、mainのroot退出計画を実行しない。
  `main_folder_history_available` の所有判定を維持する。履歴vectorsをbundleへ移さない。
- root restoreはimmutableな帰路データとして既存帰路と一緒に移る。
  clone/forkは既存の帰路継承契約を維持し、mainのmutable stack/退出要求を共有しない。
  normal/A/Bを切り替えたり、独立窓を閉じたりしてもsiblingの帰路をconsumeしない。
  完全な入口payloadへ拡張した展開snapshotやnative originも同じ扱いとし、App-global slotへの退避で代用しない。
- 共通親の実行によってmainでrootを退出しても、別contextのPDF ticket・worker・cache epoch、
  texture、動画再生を失効させない。既存のpresentation cleanupを他入力へ横展開しない。

`docs/detached-rework-plan.md` §2を読んだうえで、これは**帰路の意味を型付きownerへ集約する構造修正**と位置付ける。
BA-7に該当するApp側フラグの追加、時間窓、rect/HWND推測、再生成は行わない。
初版の独立レビューは構造修正という判断に同意した。完全な入口payloadを含む本改訂も再レビューを受け、
実装前にClaudeCodeとの合意を確認する。実装で触れたbundle/共通入口の範囲と
理由を同計画§11へ記録する。今回の文書作業では同計画や製品コードは変更しない。

### 8. 簡素化の検討と採らない案

| 案 | 判断 |
| --- | --- |
| 入口を所有せず、rootに `HistoryBack` を1つ足す / 末尾の同pathだけskip | 不採用。自己markerの問題だけでなく、履歴に記録しない検索からの入場のcanonical originや、入口なしを決められない。pathだけでは別visitの境界も区別できない |
| rootの親を無効化/no-op | 不採用。入口へ戻る既存設計を削り、終了のために別操作を要求する |
| すべての合成rootをDriveListへ送る | 不採用。Bookmarks/★/検索/Smart等の出荷済みの固有規則を失う |
| data/cacheへの移動をpath guardで拒否、cacheを隠す、選択を変える | 不採用。間違った親を作る所有者を残し、明示的な実folder openにも影響する |
| 入力入口ごとにReadingHistory/Tag分岐をコピー | 不採用。今回のTag漏れを再生産する。親actionの意味を1か所へ減らす |
| Appにroot originや戻り待ちのfieldを追加し、既存path帰路と並存 | 不採用。open/Return/replay/取消で不一致になる組合せを増やす。既存variantと帰路payloadの型置換で1つにする |
| 入口を履歴entryだけにして、退出直前にRating/展開等のApp slotから補完 | 不採用（P2×2）。普通の本openがslotを消すため成立しない。path投影の前に完全なpayloadを捕捉し、同じnative ownerへ渡す |
| 通常Back/Forward全体で同値地点をskip、Return時にstackを全整理 | 不採用。親以外の1件移動や、別visit・facet routeの履歴まで変える。root退出だけの計画に限定する |
| 本を閉じるたびにrootを新規Menuとして作り直す | 不採用。入口を本に上書きし、既存の行/scroll復元も失う。Returnは同じrootを再構築する |
| 閲覧履歴中/退出中の操作を新しいmodalで止めて組合せを除く | 不採用。通常の親操作・履歴入力を制限する。長い変換等は既存modalが既に所有し、今回の退出は既存navigation transactionで処理できる |

採る簡素化は、**親の意味の決定点を1つ、root退出の履歴確定点を既存adoptionの1つに減らすこと**。
原一覧を一度restoreしてから次のsurfaceを開く中間状態を作らず、既存のprepare/installで完成結果を採用する。
新しいrollback、resume、待機、遅延、再試行、全interleavingを管理するownerを設計しない。
まれな失敗は既定の割り切り（ログ＋通知［＋作り直し］）に従い、個別の回復設計は加えない。

### 9. テスト計画と受入条件

実装時の検証担当は実装者1名とし、独立レビューは有効な同一差分の実行証跡を再利用する。
製品バイナリ起動・desktop操作はこの開発検証に含めない。全fixtureは一時data_dir・一時book_rootを使い、
通常配置 `tmp/appdata/mimageviewer` とportable相当 `tmp/exe/data` の両方を保つ。

1. 残した赤2件を通す。
   `section1473_reading_history_root_parent_never_opens_internal_data` は従来60条件を残し、
   内部pathに入らないだけでなく**入口libraryと保存選択への1操作復帰**までassertする。
   修正後にcacheへEnterする旧観測部分は入口への期待に置き換え、誤移動を期待して通す形にはしない。
   `section1473_other_synthetic_roots_parent_inventory` は110条件を維持し、Tagの4漏れをroot停止へ直す。
2. 元の11成功を維持する。通常Folder・画像本・ZIP・PDF・warm/cold RAR・EPUB、ZIP内部、
   「履歴▼」2回親の対照、raw HistoryBackが自己markerを1件消費する既存特性を含む。
   型置換に伴うassert表現だけ直し、raw Backまで親退出の期待へ変えない。
3. root退出のpure plan testを追加する。§4の全stack例、既存forwardあり、連続自己marker、別visitの同path、
   同pathでrouteの異なる入口、自己markerと可視rootのrouteの差、複数回Menu、markerを履歴Back/Forwardで消費済み、100件truncation、
   canonical origin入場、入口なしから本を往復した後、A/B/normalの分離を検査する。
   target・両stack・routeの全文を比較し、markerだけでなく子履歴とforwardの保存も確かめる。
4. 実処理で `origin → 閲覧履歴 → Folder → 実子孫 → 親でFolder` の後の帰路を検査する。
   実Folderの子へ開き直して既存帰路が失効する条件は、物理親の既存結果を維持することをassertし、
   その後は通常の履歴Backで保存rootへ復帰してから親でoriginへ退出する。失効した帰路を復活させない。
   別の本を連続して開く場合、明示的に別場所から再Menu入場する場合、入口がBookmarks/★/Smart/
   Collectionのroot・物理子/DriveListの場合も、正しいtyped入口へ戻ることを検査する。
   検索/Tagから入った場合はcanonical originへ戻り、検索を勝手に再実行しないことを確認する。
5. 入口routing testを補う。親ボタンの実headlessクリック、合成キーのBS/Alt+↑と変更済み割当、
   mouse/ringに加えgesture・Pad B・stack再構成中の許可入口・余白double-clickを共有受付まで通す。
   サムネイル/詳細で意味が同じこと、preview前後で状態不変・previewと実行の行先一致を検査する。
   Tagはrootだけでなく実Folder子・ZIP内子から結果rootまでを全adapterで比較する。
6. 既存のReadingHistory back/converted archive、§1.328位置復元、§1.480 Enter/double-click、
   Global/Everything/Favorite検索、Rating/Collection/Smart、Snapshot、ZIP nestedの関連試験を回す。
   変換成功・取消、未採用、同一root退出の連続入力、別navigation・slot切替による旧要求退役でも、
   採用前にcommitted stackを変えず、負けた要求が入口を上書きしないことを検査する。
7. viewer bundleのheadless mount/swap/fork/retire testで帰路payloadの移譲とsibling不変を検査する。
   同じPDFを読む独立窓を模した別contextのticket/世代がmain親退出で失効しないこと、
   linked/独立viewerの既存close条件が入力共通化で増減しないことを既存testと合わせて確認する。
8. **P2-1の必須受入**：新規の展開一覧（generic `folder_nav_subfolder_restore` 未設定）を入口にして、
   (a) `展開 → 閲覧履歴 → 親 → 展開`、(b) `展開 → 閲覧履歴 → ZIP → 親で閲覧履歴 → 親で展開` を実処理で通す。
   復元したroot/roots/saved folder/accepted snapshotの所属行/removed pathsとrouteを検査し、さらに親で保存元へ戻る。
   複数roots・除外済み項目を含め、最後に使った別の展開のslotへ戻っていないことも確認する。
9. **P2-2の必須受入**：`元Folder → ★一覧 → 閲覧履歴 → ZIP → 親で閲覧履歴 → 親で★一覧 → さらに親で元Folder`。
   星数/routeだけで終えず、saved folder/subfolder restoreを含む帰路と最終到達先をassertする。
   本往復なしの直接退出も検査する。元場所がDriveList/展開の場合、入口の★に保存元がない場合も検査し、
   Noneを別の本や別★一覧の帰路で埋めない。展開起点では★を閉じた後に展開へ戻り、そこからの親も確認する。
10. **外部slot非依存の受入**：上記と、検索canonical originが★/展開の場合を、入場後にAppの旧restore slotを
    空または別のfixtureの値にした条件でも実行する。通常ZIP採用によるclearを通す試験に加え、
    明示的な値差替えはheadless fixture内だけで行う。退出中の未採用段階では旧可視stateとstackが不変、
    成功adoption後だけ捕捉payloadからnative fieldが戻ることをassertする。
    退出後にForwardで閲覧履歴へ戻って再び親退出しても同じpayloadが有効であることを検査する。
11. **同型入口の受入**：Bookmarksはrootのstable-key位置と、その後の親がmain履歴に従うこと、
    Collection/Smartはroot/物理子の復元に加え、子→rootの親・保持originを使う一時ビュー往復も検査する。
    検索はファイルメニューの実受付からcanonical originへ退出し、queryを再実行しない。
    Snapshotは場所メニュー/リングの既存拒否と、ファイルメニューで受理される経路の固定owner保持を別々に検査する。
    後者の親はSnapshotのlist復帰が先であり、固定stateを消す/複製することで試験を通さない。
    native producerが未採用のときは元の可視ownerを入口として捕捉することも確認する。

実装時の順序は、pure plan → `section1473` → 関連handler/owner試験 → gate。
build/testでは必ず `MSBUILDDISABLENODEREUSE=1`。
`.\scripts\test-focused.ps1` を実行し、広域変更等でexit 2なら `.\scripts\test-lib-full.ps1` へ進む。
共有履歴型/入力ownerを変えるため最終は `.\scripts\test-full.ps1` の非対話gateも通す。
`cargo fmt` / `cargo fmt --check` / `git diff --check` を行い、UI文言を変えた場合はglyph検査も行う。
テストをignoreしたり、不変条件を弱めたりして緑にしない。製品修正後の確認用buildは既存方針に従うが、起動はしない。

今回は設計だけなので、過去13件の11成功/既知不具合2失敗という証拠を保持し、新たな成功結果とは扱わない。
実装受入時はこの2失敗を含め全対象greenであることが必要。実マウス/実キー・配布portableの既定data_dir・
native HWND等の実機確認は別の了承済み検証枠に残し、headless結果を実機確認済みとは書かない。

### 10. 規模・実装の区切り・レビュー論点

**Medium（上限寄り、完全な入口payloadの捕捉・配送を含む）。** 製品は主に `app.rs`、`top_level_grid_view.rs`、
`collection_grid.rs`、`gamepad_input.rs`、`ui_main.rs`、`global_search_ui/everything.rs`、
`viewer_context_registry.rs` に加え、`subfolder_expansion.rs`、`smart_folder.rs`、検索/Bookmarkのorigin producer。
単純な2か所のif補充にはしない。
初版の400〜800行/3〜5開発日は撤回する。§3.1でRating/Collection/Smart等の復元payload、
§3.2で既存dispatcher/native ownerへの明示配送を定めたことから、製品差分は概算900〜1,500行、
テスト追加/更新は概算600〜1,000行、実装・自動検証・指摘修正を合わせ**5〜8開発日**を改訂目安とする。
型置換に伴う既存match/fixture更新を含み、cold build待ち・実機確認・公開作業は除く。
これはソース監査に基づく概算であり、改訂設計の採択後、実装差分で確認する。
新規の全一覧session保存機構や通常履歴の全面再設計は含めない。

実装は、(a) 完全な入口payloadとnative prepare/adoptionへの引渡し、(b) typed root/帰路と退出計画、
(c) 親受付の集約とTag修正、(d) 同型監査・回帰・関連文書更新、
の順で一つの機能として仕上げる。途中で旧帰路と新帰路を並存させた製品を出荷しない。
実装完了時に `reading-history-plan.md`、`top-level-grid-view.md`、`keymap-spec.md`、
§1.473と必要なdetached§11を更新する。過去調査の失敗証跡は履歴として残す。コミットはClaudeCodeが行う。

独立設計レビューでは、入口のtyped捕捉と検索透明性、visit境界/履歴truncation/forwardの表、
通常Backとの分離、共有親の優先順とEverything再帰回避、入力ごとのpresentation契約、
viewer/slotの所有分離に加え、**展開state全体の配送と、★へ戻った後の親までの帰路**を重点確認する。
通常利用の不便化が見つかったら実装へ進まず、ClaudeCodeから利用者へ相談する。

本改訂でP2×2に対する設計上の対応を記載したが、実装済み/再レビュー承認済みとは扱わない。
新しい利用制限・確認操作・待機を要する点は今回のソース監査では見つかっていない。
製品コード変更・ビルド・テスト・製品起動・コミットは行わず、設計再レビューへ返す。

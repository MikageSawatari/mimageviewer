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
改訂 `bbed366ab` の再確認では、その2件は指摘経路について設計上解消したが、★一覧の戻り先が別の閲覧履歴visitを
指す場合にpath投影で失われるP2×1が残りREVISEとなった。構造的修正という方向には改めて同意を得た。
本改訂では、合成一覧間の戻り先を既存の共通型で運び、Rating自身の帰路を入場からcloseまで置換する。
以下のソース監査は実行結果ではなく、**今回の改訂版の再レビュー・実装・動作検証は未実施**。

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
11. 合成一覧の帰路が別の合成一覧である場合も、同じ共通の型でその先の帰路まで保持する。
    同じReadingHistory surface/pathでもH1とH2は別visitであり、★一覧からH1へ戻る際に新rootを作らない。

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
| `src/app.rs:34867` `enter_rating_view`、`:37475`付近のRating close、`:26699`付近の合成path復元 | 入場の `legacy_path()` とcloseの `load_folder(saved)` の組は、戻り先H1のvisit/退出先を捨てる。Rating自身の帰路を共通の型付きoriginへ置換し、互換pathからの新root生成に流さない |

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

#### 3.0 合成一覧の入れ子を増やさない共通の帰路（再確認P2への改訂）

CLAUDE.md「設計の簡素化」に従い、一覧ごとの組合せを個別に増やす前に、次を比較した。

| 簡素化案 | 採否と理由 |
| --- | --- |
| 合成一覧から別の合成一覧を開けなくする / 全部modalにする | 不採用。`H1 → ★一覧 → H2` は現行の通常操作であり、入場禁止・追加の解除操作は利用を不便にする。短い場所切替のための新しいmodalも必要ない。長い変換等は既存modalを維持する |
| 別一覧に入るたび、帰路を最初の実Folderまで平坦化する / 元一覧を閉じて忘れる | 不採用。★一覧からH1、H2から★一覧へという出荷済みの1段の戻りを失う。再構築は既存prepareで行えても、論理的な帰路を捨ててよい理由にはならない |
| ReadingHistory用path、Rating用path＋展開state、Bookmark用の補助帰路を個別に相互変換する | 不採用。送り手×受け手ごとの対応が必要になり、今回のH1欠落を別の組合せへ持ち越す |
| **合成一覧間の復元先を、拡張した `TopLevelGridOrigin` / `TopLevelGridRestore` に統一する** | **採用**。Rating、ReadingHistory、Bookmarks、検索、Collection等が同じ場所復元payloadを運び、受け手の既存ownerが自分のvariantを復元する。各一覧の親規則は保ったまま、帰路表現とcapture/dispatchの種類を減らす |

利用者がたどる入れ子自体は禁止せず、実装が扱う「一覧Aから一覧Bへの専用帰路」の組合せを共通型に畳む。
`TopLevelGridOrigin { restore, route }` を共通payloadとし、`restore` は§3.1の完全な型とする。
戻り先なしは各ownerの**単一の** `Option<Box<TopLevelGridOrigin>>` のNoneで表せる。
これは既存の帰路fieldを置換する値であり、pathを残して追加する補助Optionではない。
ReadingHistoryの「入口なしはDriveList」、Ratingの「戻り先なしのclose」、Bookmarksの履歴Back等、
行先/終了規則はそれぞれのownerが決める。共通型にしたことを理由に全rootへ同じ終了動作を適用しない。

Rating rootは **`RatingRootRestore { stars, return_to: Option<Box<TopLevelGridOrigin>> }`** とする。
物理子は **`RatingPhysicalRestore { visible_path, stars, nav_chain, return_to }`** とし、同じ帰路を運ぶ。
前案の `saved_folder` / `subfolder_restore` の対は撤回する。Folderなら `Folder`、展開ならstate全体、
H1なら `ReadingHistory(H1のroot restore)`、別の一覧ならその完全なvariantを一つのreturn_toに入れる。
帰路のrouteも捕捉済みoriginに属し、close時の★一覧のrouteや現在の履歴から作り直さない。

ReadingHistoryのexitも同じoriginを保持する。`ReadingHistoryBookReturn` は元コンテナとの対という制約を保ち、
その一覧復帰先は共通originのReadingHistory variantとして運ぶ（root payloadとoriginの並列保存はしない）。
Bookmarksのrootへの場所復元も同じ型を使う一方、項目openのstable row位置・seek・Opening/Detached等の
nativeな状態は既存ownerが保持する。共通化のためにBookmark rootの親を保存originへ変えたり、
worker/実行中viewerの状態を共通payloadへ入れたりしない。

`O → H1 → ★一覧 → H2` のpayloadは、`H1.exit=O`、`Rating.return_to=H1`、
`H2.exit=Rating(return_to=H1)` という有限のimmutableな値になる。
H2は新規Menu入場なので新visitを持つ。H2から★一覧を復元するときは、内側のH1を再captureせずそのまま戻す。
同じ表示pathを持つことによるvisit統合、現在のH2でH1を上書きする処理、退出時の履歴検索による補完は行わない。
Box等で過去のsnapshotを所有し、liveな一覧相互の可変参照や循環を作らない。
Rating rootで星数を切り替える既存操作は、現在のreturn_toを引き継ぎ、直前の★一覧自身を新しい帰路にしない。

#### 3.1 完全な入口payloadの型と同型監査

`TopLevelGridRestore` を入口の復元に必要な意味を持つ型へ拡張する。型名の目安と保持内容を以下に確定する。
`TopLevelGridOrigin` の `route` はその一覧のfacet lineageであり、Appの現在routeを退出時に取り直さない。
既存の `current_top_level_restore_snapshot` は一時ビューではcanonical originを返すため、可視一覧のnative ownerを
捕捉する場合と、一時ビューのoriginを移譲する場合を区別して同ownerのcapture処理へまとめる。
汎用 `from_restore` を呼んだ後に、失った情報を別slotから再取得する順序にはしない。

| 入口 / 根拠となる所有者 | 入場時に所有するimmutable payload / 対象範囲 |
| --- | --- |
| サブフォルダ展開（P2-1） | 既存 `SubfolderExpansionRestoreState` 全体：`root`、`roots`、`saved_folder`、accepted `snapshot`、`removed_paths`。`TopLevelGridRestore::SubfolderExpansion(state)` をそのまま保持し、合成pathへの投影で捨てない。snapshotは既存のimmutableなデータ共有/所有移譲を使い、再scanを必須にしない |
| ★一覧root（前回P2-2 / 再確認P2） | §3.0の `RatingRootRestore { stars, return_to }` を `TopLevelGridRestore::Rating` のpayloadとする。Rating入場前に完全なoriginを捕捉し、H1のvisit/退出先を含める。Noneもそのまま保持する。前案のpath/展開restoreの対では代用しない |
| ★一覧の物理子 | `visible_path`、stars、nav chainと、rootと同じtyped return_toを保持する。子→★root→H1→その入口、またはFolder/展開等への帰路全体を維持し、pathだけの通常Folderにしない |
| Bookmarks root / 読込中 | DBから既存browser buildで再構築できるroot identityと、既存stable row keyによる一覧位置を保持する。位置は `BookmarkViewReturnGridState` のrow keys/selected key/scrollの復元規則を再利用する（項目openを伴わない入口で架空のopened keyを作らない）。rootの親はmain履歴を使うので別のsaved-folderは不要。`BookmarkViewState::Opening/Detached` のviewerやseek pendingをrootのworkerとして保存しない |
| 検索 Ctrl+G/Everything・Ctrl+S・Tag | **検索結果自体は退出先の対象外**。既存のcanonical `TopLevelGridOrigin` を完全な型のまま移譲する。検索開始時のcaptureとcanonical/fallback生成も上記Rating/展開等のpayloadを保つよう揃える。既存fallback slotを読むのは元ownerが有効な捕捉時だけとし、退出時には読まない。query、dataset、drill workerを閲覧履歴へ保存/再起動しない。リング等の検索中場所ジャンプ拒否は維持し、許可されるファイルメニューからの入場を検査する |
| Collection root / 物理子 | `CollectionGridRestore`（ID、revision hint、entry anchor）または `CollectionGridPhysicalRestore` 全体に、同Collectionの `TopLevelGridView::return_origin` を対にしたrestore payloadを使う。rootの親はno-opだが、後の一時ビュー移譲/同Collection再入場に使うoriginまで失わない。actorのwatch/receiver/installed mutable sessionは保持せず、既存のlatest revision prepareで再構築する |
| Smart root / scoped / container | `SmartFolderViewState`（定義ID、既存immutable navigation entries、position/back stack）に、捕捉時の `smart_folder_saved_folder` とそのsurfaceの `return_origin` を含むrestore payloadを使う。前者は定義消失時等の既存の保存元、後者は一時ビューとのorigin移譲であり役割が異なる。rootの親はno-op、子→rootの既存再構築を使い、resident sessionが残っていることを前提にしない。定義更新/削除への新しい回復規則は足さない |
| 通常の実Folder/本、DriveList、本棚 | 普通の物理一覧は論理sourceから既存scan/preflightで復元し、DriveListも既存synthetic prepareを使う。本棚は実Folder。物理一覧にReadingHistory/Bookmark等の専用親ownerが付いている場合は、その既存帰路payloadも含めたtyped restoreにする。元cache pathだけを保存せず、独立した明示入場によってその帰路を消す既存条件は拡張しない |
| Snapshot（★固定） | **固定一覧そのものを退出payloadへ複製しない**。場所メニュー/リングの既存拒否と、ファイルメニューからの閲覧履歴入場は維持する。完全な固定一覧再採用の既存経路がないため、rootの親は既存解除経路で固定を終え、入口payloadのcanonical originへ戻す。固定一覧の部分復帰は使わない。2026-10-30の範囲決定と契約は§3.1.1。Snapshot全体の所有の作り直しは§1.486へ分離 |

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

#### 3.1.1 Snapshot入口の限定対応（2026-10-30、設計担当の範囲決定）

固定owner/canonical originの作り直しは§1.473から外し、[§1.486](next-release-backlog.md#1486-snapshot固定の固定ownerとcanonical-origin--routeの単一所有)へ分離する。
採るのは設計担当提示の**案2：既存の解除経路で固定を終え、捕捉済みcanonical originへ退出する**。
`FolderNavHistoryTarget`にSnapshot entryはなく、`snapshot_return_to_list_view`もrows/path/recipeの部分復帰であり、
surface・行provenance・canonical origin/routeまで一括採用する既存経路ではない。案1は成立しない。

- 親の共有resolverは、固定が残っていても**可視ownerが型付きReadingHistory rootならRootExitを選ぶ**。
  Snapshotのpath比較による部分復帰は使わない。ほかのSnapshot内の親操作の既存規則は変えない。
  親ボタンは既存のSnapshot中disabledを維持し、無操作であることもheadlessクリックで検査する。
  許可されているBS/Alt+↑/mouse/ringは共有resolverからこの退出へ進む。
- root退出受付で実rootを捕捉し、実Snapshotがある場合は既存`dismiss_snapshot_with_origin`で固定を終了する。
  その返り値を行先にせず、rootが既に所有する完全な入口payloadだけを使う。
  この解除はTopLevelGridViewをFolderへ変えるので、既存`replace_surface`へ捕捉済みの**現在表示中のH root**を渡して
  source ownerを保つ。rows/path/routeは変更せず、固定一覧を新しく復元する経路は作らない。
- **解除は受付で即時確定**し、その後にcurrent entry・Back計画・MainListNavigation/source proofを捕捉する。
  physical prepare/commitの既存Snapshot範囲判定を迂回せず、固定を終えた通常navigationへ揃える。
  成功tailまで解除を遅らせる案は、入口O自身が固定membership外の場合にadmissionで拒否されるので採らない。
  prepare取消/未採用ではHの表示と履歴を保持し、固定は解除済みのままにする。Snapshotの復活/rollbackは作らない。
- 可視surfaceがSnapshotの場合、そのcanonical originがH1でもH1を現在rootとして再利用しない。
  新しいH2 visitを作り、H2.exitに完全なH1を持つ。実H2上でMenuを押し直す既存のroot再利用は維持する。
  `O → H1 → Snapshot → Menu H2 → 親 H1 → 親 O`では、同じpathを根拠に固定一覧とH2を同一視しない。
- 新bool/並列Option/退出時の履歴検索/固定由来の逆算は追加しない。固定stateの複製、ファイルメニュー入場の禁止もしない。
  通常の固定一覧を維持する案から、**この入口の親で固定を終える**仕様へ変えるのは設計担当の明示承認による。

独立レビュー（GPT-6.1 Sol / xhigh）は、既存解除と表示中のtyped rootの保持であり症状guardではないとしてこの限定設計に合意した。

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
- ★rootは `start_rating_navigation_owned` の既存path/展開restore引数をtyped return_toへ置換し、
  既存 `RatingNavigationTransition` → Rating build/prepare → adoptionを使う。
  今の `dispatch_folder_history_entry` にあるApp slotからの補充をこの退出に使わない。
  return_toがNoneならNoneを採用し、途中の本や無関係な★一覧の帰路を借りない。Rating自身のcloseまでの配送範囲は§3.3。
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

#### 3.3 Rating自身のtyped帰路を入場・物理子・closeまで運ぶ

再確認P2の対応は、H2の退出payloadだけを増やす修正ではない。
**Ratingが自身の帰路を最初に作る所有境界から置換する。** 変更範囲を次のとおりとする。

| 境界 / 現在の箇所 | 型置換後の責務 |
| --- | --- |
| `enter_rating_view`（`app.rs:34867`）、Menu/ring/Pad等の共通入場 | 元ownerを退役させる前に、共通captureで完全な `TopLevelGridOrigin` を作る。`origin.legacy_path()` による帰路生成をやめる。検索からの入場は既存canonical originを移譲し、既に★rootなら既存return_toを継承する |
| `start_rating_navigation` / `start_rating_navigation_owned`、`RatingNavigationTransition`（`:823,:37985,:38022`付近） | `saved_folder` と `subfolder_restore` の2引数/2fieldを、単一のtyped return_toへ置換する。prepare/reorder/membership更新も帰路を取り直さない。未採用要求は捕捉した値だけを所有し、失敗/取消で現在の帰路を変えない |
| Ratingの可視採用（`:37738`付近）、`App`の現行Rating帰路2field | `rating_view_saved_folder` / `rating_view_subfolder_restore` を撤去し、`rating_view_return_to: Option<Box<TopLevelGridOrigin>>` の1本へ置換する。成功adoptionで一覧と同時に公開し、明示clearも全payload単位にする。汎用 `TopLevelGridView::return_origin` に同じ可変帰路を並設しない |
| Rating installの展開state取り込み（`:38228,:38304`付近） | 元一覧の展開stateは入場captureでreturn_toのSubfolderExpansion variantへ移す。install時にsaved pathからAppの展開slotを再取得する処理を残さず、捕捉済みpayloadを保持する |
| `RatingPhysicalLoadOwner`、`rating_view_physical_load_owner`、`rating_physical_destination`、通常/ZIP/PDF/変換書庫のphysical adoption | `RatingPhysicalLoadOwner.saved_folder` と、`RatingPhysicalRestore` のpath/展開restore対も同じreturn_toへ置換する。物理子へ入る際にrootの帰路を捕捉し、完了時にAppから展開restoreだけを追加取得しない。nav chainは既存の子→rootの階層を持ち、return_toはrootを閉じた先を持つ |
| `rating_view_back`、`restore_rating_without_history`、replay/refresh helper、`current_top_level_restore_snapshot` | root/物理子の完全なpayloadを運ぶ。物理子からrootへ戻る際も元のreturn_toをそのままRating transitionへ渡す。sort/reloadでH1を現在地H2等に置換しない |
| `FolderNavHistoryTarget::Rating` / `RatingPhysical`、from_restore、履歴dispatcher | Rating rootの履歴variantも星数だけではなく `RatingRootRestore` を保持し、物理子も上記のtyped restoreを保持する。履歴地点としてのroot同値判定は既存の星数による規則を維持し、帰路payloadの違いで別の履歴操作を増やさない。通常のBack/Forwardは既存どおり1件ずつ移動し、復元用データを履歴や現在のAppから探し直さない |
| `close_rating_view`（`:37448–37488`）、親/既存closeの `RatingViewBack` consumer | 子の階層処理と未採用Rating要求の取消規則を維持する。rootを閉じる場合は保持したreturn_toを、§3.2の `MainListRestoreDestination::Origin` と既存navigationの履歴意図で配送する。`load_folder(saved)` の合成path復元や、引数なし `enter_reading_history()` へ流さない。宛先の成功adoptionで帰路を移譲し、単なる入力時点で元ownerを空にして補完待ちにしない |
| `ViewerContextBundle`の構築/clone/move/swap/退役（`viewer_context_registry.rs`） | Rating帰路の2fieldも1本へ置換し、同contextのnative ownerと移す。既存の帰路継承/clearの条件を保ち、mainの履歴stackをfork先へ渡さない |

H1への復帰は共通の既存synthetic prepare/adoptionへ **`ReadingHistory(H1)`** を配送し、Return目的で復元する。
そのためH1のvisit、退出先O、routeは変化しない。Menu目的で新rootを作る互換path入口とは分ける。
Folder/DriveList/展開等も同じtyped dispatcherからそれぞれの既存native prepareへ送る。
closeに伴う履歴意図は各既存経路の意味を維持し、typed配送のために新たなBack skipや一括clearを足さない。
return_to=Noneなら現行の「保存元なし」のclose結果を維持し、ReadingHistoryの入口なし規則をRatingへ転用しない。

Rating root/物理子を検索・Snapshot等のcanonical originへ退避するproducerも、上記payloadをそのまま渡す。
元のpath/展開fieldを残してtyped return_toを補助的に足す案や、close時に同pathのH1を履歴から検索する案は採らない。
既存pathの互換routingが必要な他の呼出し元では、場所identityへの投影としてのみ使い、
Ratingの入場・return・closeの型付き帰路をそこへ戻さない。

### 4. root退出、自己marker、forward履歴

root ownerの `plan_root_exit` は、保持した退出情報と**現在の有効なmain/slotの履歴**から
入口markerが見つかる場合に `FolderHistoryPlan` を作り、既存 `MainHistoryOperation::Replay` とtyped destinationのdispatcherへ渡す。
markerが見つからない場合は、下記補足に従って同じtyped destinationへ既存の `MainHistoryOperation::Direct` で移る。
通常の `advance(Back/Forward)` を全場所で「同じpathをskip」に変えない。

`RecordedEntrance` の場合（cursor操作には完全なoriginから導出したentryを使う）:

1. 現rootのvisitに属するBack内の**最初の**root markerを境界とし、その手前だけから場所とrouteが一致する入口を探す。
   同visitのmarkerがBackに無い場合は、現在表示中のrootを境界としてBack内を探す（入場直後や通常Backで自己markerを消費済みの場合）。
   入場後に同じ場所・routeへ再訪しても入口候補にしない。同じ合成pathの別の入場を境界にしない。
   本/実子孫の往復で境界より後ろに積まれたentryを、仮cursor上でBackした順に扱う。
2. 仮currentと同じvisitのrootが直ちに現れる場合は自己markerとして消費し、rootを再loadしない。
   この消費では仮currentのroot/routeを古いmarkerで上書きしない。Forwardへ残すのは現在表示中のrootである。
   その後、保持した入口entryまでを1つの退出計画にする。子の履歴を見つけたところで止まらない。
3. 普通のBack/Forwardで自己markerを消費済みでも、入口markerが残っていればそのentryまでBack計画に含める。
4. 入口markerが見つからない場合は、以下の2026-10-30補足を優先する。
   旧案の「marker脱落後のsuffixを推定してForwardへ移す」は撤回する。

`CanonicalOrigin` は、当該visitのmarker/子の履歴までを同じ計画で処理するが、入場時に積んでいない
元場所のentryをBackからpopしない。canonicalなtyped originを既存復元経路へ渡す。
両退出種別とも§3.2の完全なpayloadをdispatchし、location/routeが同じという理由でApp残存stateへ置き換えない。
`DriveList` も当該visitを処理してドライブ一覧を採用する。どちらも合成pathの親へは落ちない。
これらの計画も、宛先のprepare/adoptionと同じ要求でcursorをcommitする。

**2026-10-30 ClaudeCode設計担当による補足（実装停止理由への決定）**:

- 判定は、入口markerが現在の有効な履歴に見つかるかの1つとする。見つかる場合だけ上記のBack計画を使う。
- 見つからない場合（検索overlay経由の非記録入場、履歴上限による脱落等）は、入場直後/脱落後を区別しない。
  rootが保持する完全な `TopLevelGridOrigin` へ**通常の前進ナビゲーション**で移る。
  アドレスから場所を開く場合と同じ既存Directの規則で現在位置をBackへ1件記録し、forwardを扱う。
  既存entryを推定でpopしたり、Back/Forwardを再構築したりしない。
- 両方式の宛先、行・scroll・native帰路の復元payloadは同じ。入力元や現在のAppの残存slotから取り直さない。
- 新しい履歴境界field、脱落推定guard、入場前prefixを復元する独自の履歴検索は作らない。
  現在の履歴と既存の場所/route照合でmarkerの有無を確認できる範囲だけを使う。
- この補足は下の旧stack例のうちmarkerなしの履歴操作を置き換える。
  起動直後の入口なしも既存DirectでDriveListへ移り、rootの型付きpayloadを記録する。

**markerありのforwardの規則は、自己markerを除き、途中を描画せず同じ回数のBackをした結果に揃える。**
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
| 検索経由・入口markerなし `H / [X,Y] / [F]` | `O / [X,Y,H] / []`（Direct） | 既存Directどおりforwardなし。入場前のentryを推定で消費しない |
| 上限で入口marker脱落 `H / [E1,…,E100] / [F]` | `O / [E2,…,E100,H] / []`（Direct） | 通常の100件制限以外のentry消費/再構築をしない |
| 起動直後 `H / [] / []` | `DriveList / [H] / []`（Direct） | rootをBack側に記録。合成pathの物理親へ進まない |

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
| ★一覧root/子/読込中 | `rating_view_parent_nav` とnav chainを維持。§3.0/3.3の共通typed return_toをRating採用時に戻し、さらに親で元場所（別visitの閲覧履歴を含む）へ閉じられることまで対象にする |
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

`reading_history_return_from` の型置換とRating帰路2fieldの単一typed return_toへの置換は、
`src/app/viewer_context_registry.rs` の `ViewerContextBundle` の同field、構築・swap・fork/moveにも機械的に波及する。
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
初版と `bbed366ab` の独立レビューは構造修正という判断に同意した。共通typed帰路を含む今回の改訂も再レビューを受け、
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
| 合成一覧の入れ子を禁止/平坦化する / 一覧の組ごとの専用帰路を増やす | 不採用（§3.0）。前者は出荷済みの戻りを失い、後者は組合せを増やす。共通のtyped originで表現を一本化し、Ratingの旧帰路対を置換する |
| marker不在時の入場前prefix/脱落後suffixを識別し、Back/Forwardを組み直す | **不採用（2026-10-30補足）**。検索経由/上限超過の組合せを扱う境界状態を増やさず、どちらも既存Directへ揃える。完全な入口payloadで行先と位置を維持し、履歴操作だけ通常の前進規則を使う |
| Snapshot固定とroot退出prepareを並存させ、成功時にだけ固定を解除 | **不採用（§3.1.1）**。固定のmembership外の入口へは既存admissionで進めない。受付で既存解除を完了してから通常navigationへ揃え、固定＋prepareの組合せと取消時の復活を作らない |
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
   星数/routeだけで終えず、typed return_toに含む帰路と最終到達先をassertする。
   本往復なしの直接退出も検査する。元場所がDriveList/展開の場合、入口の★に保存元がない場合も検査し、
   Noneを別の本や別★一覧の帰路で埋めない。展開起点では★を閉じた後に展開へ戻り、そこからの親も確認する。
10. **外部slot非依存の受入**：上記と、検索canonical originが★/展開の場合を、入場後に現在のRating ownerの
    typed帰路や汎用の展開restore slotを空または別のfixtureの値にした条件でも実行する。撤去する旧Rating fieldを試験用に残さない。
    通常ZIP採用によるclearを通す試験に加え、
    明示的な値差替えはheadless fixture内だけで行う。退出中の未採用段階では旧可視stateとstackが不変、
    成功adoption後だけ捕捉payloadからnative fieldが戻ることをassertする。
    退出後にForwardで閲覧履歴へ戻って再び親退出しても同じpayloadが有効であることを検査する。
11. **同型入口の受入**：Bookmarksはrootのstable-key位置と、その後の親がmain履歴に従うこと、
    Collection/Smartはroot/物理子の復元に加え、子→rootの親・保持originを使う一時ビュー往復も検査する。
    検索はファイルメニューの実受付からcanonical originへ退出し、queryを再実行しない。
    Snapshotは場所メニュー/リングの既存拒否と、ファイルメニューで受理される経路の固定owner保持を別々に検査する。
    後者のrootの親は§3.1.1に従い固定を終了し、捕捉済canonical originのsurface・行provenance・routeへ戻る。
    同pathのH1起点、H2上のMenu再押下、退出prepare取消後もHを保ち固定は解除済みであることを検査する。
    native producerが未採用のときは元の可視ownerを入口として捕捉することも確認する。
12. **再確認P2の必須受入**：通常/portable相当の両配置で、
    `入口O → 閲覧履歴H1 → ★一覧 → 閲覧履歴H2 → 親で★一覧 → 親でH1 → 親でO` を実処理で通す。
    同じ表示pathへ戻ったことだけでは成功にせず、H1/H2のvisitが異なり、H1の退出先/routeがOのまま、
    Ratingのreturn_toがH1、H2の退出先がそのRating payloadであることを各段階でassertする。
    各親入力の共有resolverと実ボタンpreviewを比較し、H1を入口なしの新rootへ作り直していないことも検査する。
13. **H2の本往復を挟む必須受入**：上のH2からZIPを開き、親でH2へ戻ってから、★一覧→H1→Oの順を続ける。
    物理採用が現在のRating帰路をclearしてもH2内のimmutable payloadがH1まで保持することを確認する。
    ZIP内部階層では1段ずつ戻り、rootからだけH2へReturnする既存の試験も維持する。
14. **Rating物理子と共通帰路の回帰**：`O → H1 → ★一覧 → Ratingの物理子 → H2` の後、
    H2の親で同じ物理子へ、以後は既存nav chainで★rootへ、★rootの親でH1へ、H1の親でOへ戻す。
    RatingPhysicalLoadOwnerとrestore/replayを経てもtyped return_toを失わないことを検査する。
    ★rootでの星数切替・sort/reload・membership更新・準備取消でもH1の帰路を維持し、
    戻り先なしの場合や、元場所が通常Folder/DriveList/展開/Bookmarksの場合の既存close結果を保つ。
    代表的なnative owner間の往復と共通型の再帰payload試験を組み合わせ、一覧A×一覧Bごとの専用実装を前提にしない。
15. **marker有無の補足の必須受入**：入口markerあり、検索overlay経由でmarkerなし、100件上限でmarkerが脱落、
    の3条件で同じ入口へ退出し、その入口の選択行・scrollを復元する。
    markerありはBack計画の両stack全文、markerなしは既存Directと同じ記録/forward処理をassertする。
    unrelatedなentryを推定で消費・改変せず、脱落したprefixを再作成しないことも検査する。

実装時の順序は、pure plan → `section1473` → 関連handler/owner試験 → gate。
build/testでは必ず `MSBUILDDISABLENODEREUSE=1`。
`.\scripts\test-focused.ps1` を実行し、広域変更等でexit 2なら `.\scripts\test-lib-full.ps1` へ進む。
共有履歴型/入力ownerの変更である。2026-10-30再開指示の今回の検証範囲は `.\scripts\test-lib-full.ps1` までとし、
workspace/vendor/docの `.\scripts\test-full.ps1` は今回の開発実行へ追加しない。公開時のgateはリリース担当が扱う。
`cargo fmt` / `cargo fmt --check` / `git diff --check` を行い、UI文言を変えた場合はglyph検査も行う。
テストをignoreしたり、不変条件を弱めたりして緑にしない。製品修正後の確認用buildは既存方針に従うが、起動はしない。

設計承認時点では、過去13件の11成功/既知不具合2失敗という証拠を保持し、新たな成功結果とは扱わない。
実装受入時はこの2失敗を含め全対象greenであることが必要。実マウス/実キー・配布portableの既定data_dir・
native HWND等の実機確認は別の了承済み検証枠に残し、headless結果を実機確認済みとは書かない。

### 10. 規模・実装の区切り・レビュー論点

**Medium（上限寄り、完全な入口payloadの捕捉・配送を含む）。** 製品は主に `app.rs`、`top_level_grid_view.rs`、
`collection_grid.rs`、`gamepad_input.rs`、`ui_main.rs`、`global_search_ui/everything.rs`、
`viewer_context_registry.rs` に加え、`subfolder_expansion.rs`、`smart_folder.rs`、検索/Bookmarkのorigin producer。
単純な2か所のif補充にはしない。
初版の400〜800行/3〜5開発日は前改訂で撤回した。前改訂では§3.1の完全な復元payloadと
§3.2の明示配送までを製品900〜1,500行/テスト600〜1,000行/5〜8開発日と見積もった。
今回のRating帰路対の共通型への置換は、入場・transition・history variant・物理子・close・bundleまで含む。
合成一覧間の専用相互変換は減らすが、型置換と回帰確認の下限には余裕が少ないため、
製品差分は概算1,000〜1,700行、テスト追加/更新は概算700〜1,100行、
実装・自動検証・指摘修正を合わせ**6〜8開発日**へ見直す。
型置換に伴う既存match/fixture更新を含み、cold build待ち・実機確認・公開作業は除く。
これはソース監査に基づく概算であり、改訂設計の採択後、実装差分で確認する。
新規の全一覧session保存機構や通常履歴の全面再設計は含めない。

実装は、(a) 共通のtyped originとRating帰路対の置換・完全な入口payloadのnative prepare/adoptionへの引渡し、
(b) typed root/帰路と退出計画、
(c) 親受付の集約とTag修正、(d) 同型監査・回帰・関連文書更新、
の順で一つの機能として仕上げる。途中で旧帰路と新帰路を並存させた製品を出荷しない。
実装完了時に `reading-history-plan.md`、`top-level-grid-view.md`、`keymap-spec.md`、
§1.473と必要なdetached§11を更新する。過去調査の失敗証跡は履歴として残す。コミットはClaudeCodeが行う。

独立設計レビューでは、入口のtyped捕捉と検索透明性、visit境界/履歴truncation/forwardの表、
通常Backとの分離、共有親の優先順とEverything再帰回避、入力ごとのpresentation契約、
viewer/slotの所有分離に加え、**展開state全体の配送と、★→H1→Oまでの共通typed帰路**を重点確認する。
通常利用の不便化が見つかったら実装へ進まず、ClaudeCodeから利用者へ相談する。

前回P2×2は `bbed366ab` の再確認で設計上解消と判定された。
今回の改訂は残ったP2×1への対応であり、実装済み/今回の改訂の再レビュー承認済みとは扱わない。
新しい利用制限・確認操作・待機を要する点は今回のソース監査では見つかっていない。
製品コード変更・ビルド・テスト・製品起動・コミットは行わず、設計再レビューへ返す。

## 実装着手と停止の記録（承認設計 `cae94fb27` の続き）

2026-10-30。ClaudeCodeから独立設計レビューACCEPT（P1/P2/P3なし）と構造的修正への双方同意を受け、実装へ着手した。
**履歴境界の前提不足を実装中に発見し、依頼の「前提がコードと食い違ったら止める」に従い停止した。**
設計を黙って変更する、推定guardを加える、境界fieldを追加する、という対応はしていない。

### 止めた理由：canonical入場前のBackと、marker消失後のBackを識別できない

承認設計§4は、次の両方を要求している。

| 状態 | 必要な退出計画 |
| --- | --- |
| `O → 検索 → H` の直後。Hは `CanonicalOrigin(O)`、入場前からBackに `[X,Y]` がある。Hの自己markerはまだない | Backの `[X,Y]` を保持し、HからOへ退出。入場前の履歴を消費しない |
| canonical入場したHの帰路を保持する別一覧・子への遷移を重ね、履歴上限でHのmarkerが脱落した状態 | 承認設計のtruncation規則では、当該visit以後の残存履歴をForwardへ移し、Oへ退出。落ちた古いBack prefixを復活させない |

基準ソース `cae94fb27:src/app.rs` の `adopt_synthetic_surface`（26594付近）は検索中の入場を
`MainHistoryOperation::Restore` へ変え、既存cursorを動かさない。
一方 `push_folder_nav_stack`（24820付近）は100件を超えた先頭を除去する。
保持するrootは `visit { context, surface_generation, switch_sequence }` と退出originだけであり、
**入場前のBack prefixと、入場後のsuffixの境界を表す情報を持たない**。
場所/routeの値は再訪で同じになり得るため、現在のentry値やpath一致から境界を推測してはいけない。

実装途中の `reading_history_origin.rs::FolderHistoryPlan::reading_history_root_exit` で
marker不在を一律に先頭まで消費すると、最初のケースの既存Backまで消費してしまう。
逆に一律に保持すると、設計のtruncation時のForward規則を満たせない。
これは型/所有境界と退出契約を確定するための情報不足であり、入力ごとの分岐追加では解消しない。
後者の具体的な長時間操作列の実行再現は未実施で、ここでは承認設計が要求する状態に対するソース/状態モデルの照合を記録する。

設計補足が必要な論点は、root入場時に履歴境界もimmutable payloadとして所有するか、
その境界をtruncation・replay・slot切替後もどう解釈するかである。
停止時点では境界の表現と採用は**未決定**だった。単なるBack件数やpath探索で決め打ちせず、設計担当と独立レビューへ返した。
通常操作の制限や、履歴保存を落とす方針への変更は行わない。

### 保存した差分と検証状態

作業開始時のtreeはclean。未コミットの作業差分を保持しているが、**実装途中でありコンパイル成立を確認していない**。
修正完了・出荷可能・実機未確認の完成状態としては扱わない。

- typed root/origin/BookReturn/Rating帰路対置換、capture/dispatchとroot退出計画の骨格。
- `grid_parent.rs` の共有action/実行、Tag、toolbar、mouse/ring/Pad、Everything下位親への接続の初稿。
- 変換書庫の帰路payload保持、描画後dispatch、bundleの帰路移譲の追従。
- 既存テストfixtureの型移行、赤いroot再現テストの入口/選択assert強化、§9.8〜14等のruntime受入テストの初稿。

未完はSubfolder/Smart/Collection/Bookmarkの完全payload配送、残る型参照/inline test、pure plan試験、
adapter/slot/sibling回帰の追加・実行、全体コンパイル/指定gate、関連マニュアル/仕様書の完成更新である。
旧帰路の撤去を含む移行全体は完了していないため、この差分をそのまま統合しない。

`git diff --check` は停止時の差分で通過。担当の新規 `grid_parent.rs` だけは停止前にrustfmtを実行したが、
要求された全体の `cargo fmt`、2種類のcargo check、focused/full gate、glyph、viewer_context_auditは**未実行**。
Cargoによる成功証拠や、追加テストの実行結果はまだない。過去の調査結果をこの実装差分の検証へ流用しない。
製品起動・デスクトップ操作・実データ変更・確認用build・コミットは行っていない。

## 実装再開（2026-10-30、設計担当補足）

上記の停止後、設計担当から§4・§8・§9.15の補足を受領した。
入口markerが現在の履歴にある場合だけまとまったBack計画を使い、存在しない場合は
同じ完全な入口payloadへ通常Directで移る。検索経由と上限脱落を区別する状態を追加しない。
この補足を設計へ追記した後、保存差分から実装を再開した。上記の停止時点の未実行・未完記録は履歴として残す。

### 実装の所有境界

- `top_level_grid_view.rs` にvisit付きReadingHistory root、BookReturn、完全なRating帰路、
  Bookmark/Smart/Collection/物理子の復元payloadを置く。入口捕捉時にAppの外部slotから必要な値をまとめ、
  退出時の残存slotや履歴検索で欠落値を補わない。Ratingの旧path/展開restore対は撤去する。
- `reading_history_origin.rs` に入口捕捉・native ownerへの明示配送・root退出計画を置く。
  `MainListNavigation.destination` が既存requestの寿命に沿って復元先を所有し、成功adoptionで採用する。
  本からのReturnは同じroot/visitを保ち、通常Back/Forwardの1件移動は変えない。
- `grid_parent.rs` で副作用のない親actionを決め、previewと実行の入口を揃える。
  Tag、Snapshot、検索、Rating、ZIP内部、合成rootの既存優先順を保ち、
  BS/Alt+↑/再割当キー/親ボタン/mouse/ring/gesture/Pad/背景操作から配送する。
  Everythingの下位物理親は専用委譲とし、上位検索ownerへ再帰させない。
- 変換書庫では元containerと帰路payloadを一緒に保持し、cache実体のpathだけに射影しない。
  viewer bundleは既存のmove/swap/fork契約を使い、main履歴の所有範囲を広げない。
- 実装中レビューで、SmartのDirect採用時にorigin routeを失う点と、連続Back/Forward・EPUBの別名PDF保存で
  宛先変更後も旧payloadを持つ点を修正対象にした。Smartの受付でoriginを捕捉し、cursor/保存先を変えるproducerで
  typed destinationも同時に差し替える。採用側のpath一致guardや新しいApp pendingは追加しない。

### 受入条件と自動テストの対応

以下は追加・強化した試験の対応表。実行結果は検証記録で別に示す。

| 設計§9 | 主な試験 |
| --- | --- |
| 1–2 | 既存`history_parent_1473.rs`の赤2件を含む13件（root60条件・他root110条件、書庫/EPUB/履歴▼/raw Back） |
| 3 | `reading_history_origin.rs::section1473_root_exit_plan_*` 5件（stack/route/visit/上限/slot全文） |
| 4, 8–10, 12–14 | `nested_history_rating_*`、`expansion_entrance_owns_complete_restore`、`rating_exit_keeps_its_own_complete_origin`、`folder_descendant_does_not_resurrect_expired_book_return` |
| 5 | `grid_parent.rs::section1473_router_*` 3件と既存headless toolbar/背景pointer試験 |
| 6, 10, 14 | `history_parent_lifecycle_1473.rs::section1473_lifecycle_*` 5件（未採用/反復/取消/slot退役/Rating更新/None/Bookmarks） |
| 7 | `history_parent_context_1473.rs::section1473_*` 3件（mount/swap/stash/fork/retire、独立同PDFのticket/job不変） |
| 11 | `other_owned_entrances_survive_history_and_book`、`bookmark_entrance_restores_stable_key_and_scroll_then_its_parent`、`smart_and_collection_physical_entrances_keep_native_root_and_origin`、`search_transfers_complete_canonical_origin`、`snapshot_keeps_input_specific_admission_and_parent_owner` |
| 15 | `marker_presence_selects_back_or_direct_without_losing_entrance_position`（通常/portable相当×3条件、行/scroll/両stack全文） |
| 同型producer監査 | `rapid_history_rebinds_typed_and_plain_physical_destinations`、`rapid_smart_history_rebinds_destination_before_cross_kind_handoff`、`direct_smart_entry_captures_source_route_before_adoption` |

runtime fixtureは一時data_dirと一時libraryだけを使う。入口なし→本→root退出→通常Backでのroot復元も
`history_without_entrance_keeps_drive_exit_after_book_and_replay`で検査する。

### ビルド・検証の途中記録

すべてのCargo実行で `MSBUILDDISABLENODEREUSE=1` を設定した。
初回のcore checkはcold native buildの`turbojpeg-sys`並列ビルドで失敗したため、依頼の許容範囲で`-j 1`へ切り替えた。
以後はnative buildを通過し、型移行のコンパイルエラーを修正した。
`cargo check -p mimageviewer --lib --tests --features test-script -j 1` は追加テスト込みでexit 0
（`target/section1473-test-check-2.log`）。`cargo fmt`（引数なし）と整形後の`git diff --check`も通過した。
`viewer_context_audit`は初回、新テストmoduleのtest-only属性の認識でA6が2件出たため、
crate-levelの`cfg(all(test, windows))`を明示し、再実行はexit 0・違反0
（`target/section1473-context-audit-2.log`）。
`test-focused.ps1`は共通型/文書を含む変更としてexit 2、部分テストを実行せず全library試験を案内した。
UI glyph検査もexit 0。この段階ではruntime test・最終gateの成功証拠とは区別する。

初回の `cargo test -p mimageviewer --lib section1473 -j 1 -- --test-threads=1` は
44件中38成功・6失敗（`target/section1473-tests-1.log`、初回実行体ビルド9m56s、試験169.92s）。
調査時に残したroot/Tagの赤2件、pure plan/rapid回帰8件、context3件、lifecycle5件は成功した。
失敗はrouter3件、検索経由の入口行/scroll、Rating物理子の退出、Snapshot入口の6件。
routerのkeyboard-owner公開省略等のfixture問題と、Ratingの旧native nav chainの退役漏れを切り分けた。
検索の普通Folder位置は、既存の`folder_history`保存が物理load固有の`save_leaving_folder_grid_position`にあり、
検索row採用の共通出発保存に接続されていなかった。入口位置を新fieldで持ち直さず、既存位置ownerの保存を
row交代の共通境界へ集約する。これらの修正後の結果は別に記録する。

## 再停止：Snapshotの固定ownerに関する設計前提不足（2026-10-30）

### 確認結果と停止理由

2回目の `cargo test -p mimageviewer --lib section1473 -j 1 -- --test-threads=1` は
**45件中44成功・1失敗**（`target/section1473-tests-2.log`、実行体ビルド10m03s、試験324.89s）。
前回6失敗のうち、router3件、検索経由の行/scroll、Rating物理子は解消した。
追加した `section1473_snapshot_subset_cannot_overwrite_ordinary_entrance_position` も成功した。
調査で残した赤2件、§9.12〜15、pure plan、context3件、lifecycle5件は成功している。

残る失敗は `section1473_snapshot_keeps_input_specific_admission_and_parent_owner`
（`src/app/tests/history_parent_1473.rs:1506`）。一時data_dirの通常配置で、
`普通Folder O → Snapshot固定 → ファイルメニューで閲覧履歴 → 親` を実行すると、
固定stateを維持しOへ戻るassertまでは成功するが、**`items_are_reading_history_view` がtrueのまま**で失敗する。
最初の試験での入場失敗は、Snapshotの非同期準備が終わる前にMenuを呼んだfixtureの問題だった。
準備の完了を実pollで待った2回目では入場でき、上記の行owner不一致が独立して残った。
portable相当を含むloopの最初の条件で止まるため、この失敗をportable側でも実測したとは扱わない。

ソースを照合すると、単なるflagのクリア漏れより広い所有境界がある。

| 境界 | 現在の処理と不足 |
| --- | --- |
| 固定（`snapshot_ops.rs::activate_snapshot`, ~912/1147） | 固定rowsは`SnapshotState`へ置くが、canonical `TopLevelGridOrigin`/routeは`TopLevelGridView::begin_with_origin(Snapshot, ...)`にしか置かない |
| Snapshotの値（`src/snapshot.rs::SnapshotState`, ~530） | 保存rows・固定rows・path・recipe等を持つが、完全なcanonical origin/routeと固定list再採用の行ownerを所有していない |
| H採用（`app.rs::install_reading_history_surface`, ~34733/34773、`install_prepared_reading_history_entries`, ~35050） | `replace_surface(ReadingHistory(...))`が固定Snapshot側の唯一のcanonical slotを置き換える。Hのexitへ入口は捕捉されるが、Snapshot ownerへの保持/再配送の契約はない |
| surface交代（`top_level_grid_view.rs::replace_surface`, ~1836/1862） | surfaceを変え、`return_to=None`とする。これは通常の新surface採用として妥当だが、残っているSnapshot固定ownerとの分担を表していない |
| 固定list復帰（`snapshot_ops.rs::snapshot_return_to_list_view`, ~1543） | rows/path/recipeを戻し、surface・行flag/来歴・canonical origin/routeは再採用しない。従ってH由来のsurface/flagが残る（flagは上記試験で実測、surface/帰路の不一致はソース照合） |
| 親選択（`grid_parent.rs::grid_parent_action`, ~359） | Snapshotのoriginとcurrent pathの一致でlist復帰を選ぶ。**ReadingHistory由来の固定 → Menuで別のH**では両pathが`__reading_history__`となり、固定listと別surfaceを識別できない。この条件はソース上の未試験経路である |

`deactivate_snapshot` / `dismiss_snapshot_with_origin` も既に置き換えられたTopLevelGridViewからcanonicalを取り直す。
したがって設計§3.1/§9.11の「固定ownerを同contextに保持し、既存list復帰へ委譲すれば完全に戻る」という前提が成立しない。
独立レビュー（GPT-6.1 Sol / xhigh）も**P2相当の設計前提不足として停止に同意**した。
行flagだけ落とす、H.exitからSnapshot由来を逆算する専用分岐、汎用return_toの並列保持は行っていない。
通常操作を不便にするためSnapshot中のファイルメニュー入場を禁止する変更も行わない。

### 再開に必要な補足設計

1. Snapshotの固定ownerとcanonical origin/routeの**単一の所有者**を決め、別の一時surfaceを採用しても失わない配送を定義する。
   新しいbool/並列Optionや、退出時の履歴検索で補う案にしない。
2. 固定listの再採用を、rows/pathだけでなくsurface・行provenance・routeまで含むowner操作として定義する。
   Snapshot解除/一時ビュー切替/fork/closeも同じ所有契約へ接続し、固定stateを複製して解決しない。
3. 「固定listを表示中」と「同じpathの別surfaceを表示中」を型付きowner/既存row provenanceで識別する。
   物理Oの現在のfixtureに加え、ReadingHistory/★/Bookmarks/検索由来の固定、同pathのH入場、
   親を続けた場合、固定解除/別一時ビューへの移譲、context/siblingの受入を確定する。

実装者はこの補足を決め打ちせず設計担当へ返す。現在の共通root/帰路の製品差分と成功済み試験は保存するが、
このまま統合・出荷しない。§1.473は「実装済み・実機未確認」へ進めず、実装途中とする。

### 停止時点の差分と検証

- 製品の主な変更先：`app.rs` / 新規`reading_history_origin.rs` / `top_level_grid_view.rs`（typed root/帰路/退出計画/位置保存）、
  新規`grid_parent.rs` / `gamepad_input.rs` / `ui_main.rs` / `global_search_ui/everything.rs`（親受付）、
  `subfolder_expansion.rs` / `smart_folder.rs` / `collection_grid.rs` / `collection_navigation.rs` / `snapshot_ops.rs` / `close_return.rs`
  / `viewer_context_registry.rs` / `update_phases.rs` / `ui_dialogs/archive_convert.rs` / `ui_dialogs/collections.rs`（native配送・変換・bundle・表示追従）。
  `snapshot_ops.rs`の製品変更はSmart restore wrapperのmutアクセス追従のみで、今回のSnapshot所有境界へは手を入れていない。
- `history_parent_1473.rs`の受入強化、新規`history_parent_context_1473.rs` / `history_parent_lifecycle_1473.rs`、
  pure/adapter試験、既存fixtureの型移行を保存。PDFパスワード/書庫変換取消のtyped帰路全文assertは追加済みだが、
  個別実行/全library gateは停止時点では未実行。
- 関連仕様3文書とマニュアル2ページは新しい通常の親規則へ更新した作業差分で、機能全体の完成を意味しない。
  backlogとdetached§11には今回の停止と未完を明記した。コミットはしていない。

| コマンド | この停止差分の結果 / 証拠 |
| --- | --- |
| `cargo fmt`（引数なし） | 成功。2回目試験の前に全体整形 |
| `cargo check -p mimageviewer --bin mimageviewer-core -j 1` | exit 0、`target/section1473-core-check-final.log` |
| `cargo check -p mimageviewer --lib --tests --features test-script -j 1` | exit 0、`target/section1473-test-check-final.log` |
| `cargo test -p mimageviewer --lib section1473 -j 1 -- --test-threads=1` | exit 101、44成功/1失敗、`target/section1473-tests-2.log` |
| `scripts/test-focused.ps1` | 共通変更としてexit 2、`target/section1473-focused.log` |
| `scripts/test-lib-full.ps1` / `scripts/test-full.ps1` | **未実行**。新たな設計前提不足で停止し、既知の赤を含む完成gateへは進めていない |
| `python scripts/check_ui_glyphs.py` | 途中差分でexit 0。以後UI文字列の変更なし |
| `cargo run --locked -p viewer_context_audit` | 途中差分でexit 0・違反0（`target/section1473-context-audit-2.log`）。その後の位置保存/Rating退役変更を含む最終監査は未実行 |

Cargoはすべて`MSBUILDDISABLENODEREUSE=1`。製品起動・desktop操作・通常設定/実データの変更・
確認用build・コミットは行っていない。以後の製品変更とgateは補足設計を受領するまで停止する。

## Snapshotの限定解除方針による再開（2026-10-30、WIP `7916a1e97`の続き）

設計担当は固定owner全体の作り直しを今回の範囲から外し、§1.486へ分離した。
完全な固定一覧再採用の既存経路がないことをソース照合し、設計§3.1.1へ既存解除の案2を追記して再開した。
限定設計と今回の小さい製品差分について独立レビュー（GPT-6.1 Sol / xhigh）はACCEPT。
成功tailで解除する案がphysical admissionで拒否されることも確認し、受付で解除してから通常navigationを使う。

今回の製品変更は`reading_history_origin.rs`と`grid_parent.rs`のみ。
前者は可視Snapshotからの新visit生成と退出受付での既存解除、後者はH rootをSnapshot部分復帰から外す処理である。
通常のSnapshot内の親、親ボタンdisabled、他の親ownerの優先順、scope guard、履歴vectorの規則は維持する。
`section1473_snapshot_keeps_input_specific_admission_and_parent_owner`を固定解除の契約へ更新し、
同pathのH1/H2・Menu再押下、Back/Direct退出の取消・再発行、完全なnative canonical originの復元を3試験追加した。
Snapshotのsubset位置保護試験も維持し、通常/portable相当の一時data_dirで確かめる。
この時点では実行結果待ちであり、WIPの44成功/1失敗を今回の成功証拠へ読み替えない。

### 再開後の広域検証と同型経路の補完

Snapshot関連5件は `target/section1473-snapshot-tests-3.log` で全成功（試験9.33s）。
最初の全library gateは14,960実行・14,953成功・7失敗・65既存skip（581.400s、
`target/section1473-lib-full-1.log`、詳細は`target/test-lib-full/runs/20261030T031924831Z-32b78e94/`）。
失敗を抑止せず、以下のように型移行の実装/fixture不足を修正した。

- `activate_quick_folder_slot`の同じ場所の再選択が`Path`だけを認識し、採用済みBookmarkの
  `Physical`をreloadしていた。既存`folder_nav_targets_eq`で同値判定を統一し、Rating/Collection等の既存除外を維持。
  ReadingHistoryのPhysical帰路でもCurrent・履歴・items世代が不変な試験を追加した。
- ReadingHistoryのReturn再表示まで常に明示requestとして退役し、Remote解除で保留startup targetを消していた。
  preparedが所有するMenu/Returnを使いMenuだけを明示操作とし、Return/reloadの旧契約を戻した。
  検索中のMenuでも保留targetを取消す実受付の試験を追加した。
- Collectionの3実体試験と通常Backのfixtureが下位primitiveへ`None`を直渡ししていたため、
  実際のBack/Forward dispatcherを通す形へ移行し、帰路全文assertを保持・追加した。
- sort試験の旧不整合surfaceは、物理ZIPのページ順優先と正規Hの固定順を分けて検査する形へ修正。
  Rating close試験は実在する退出先を用意し、採用前の帰路保持と実poll後の場所/stack消去を検査する形へ移行した。

修正後の絞り込み59件は58成功・1失敗（68.216s、`target/section1473-targeted-4.log`）。
上記7件は全て解消し、同型監査で追加した
`section1473_collection_native_outer_retains_complete_temporary_origin`だけが赤となった。
実経路 `O → Collection C → 検索結果の画像fullscreen → Ctrl+↓でCの次source` で、
非resident root再構築が`begin(Collection,None)`を使い、destinationも`return_to=None`のため、Cの帰路Oを失っていた。
独立レビューもP2として確認した。

このproducerは既存requestの`navigation.source_location`に完全なCollection/CollectionPhysical帰路を既に持つ。
そこから帰路だけを取り、最新preparedのrevision/entry anchorと組み合わせてnative再構築とdestinationへ明示配送する。
RAR/PDF失敗等のPhysicalHistory転送では、移譲後requestからdestinationを作り、その同じ帰路をinstallへ渡す。
新field/現在App slotによる補完は追加しない。residentな同Collectionの有効requestは同じ帰路を保持し、
`navigation=None`のdetached経路の従来契約も維持する。限定修正は独立レビューACCEPT。
回帰は通常Folder・実RAR転送・CollectionPhysical起点の一時ビュー往復を実callerで通し、完全帰路とactual metadataを比較する。

これらは承認済みの「完全payloadを既存native ownerへ渡す」境界と、入口のMenu/Returnの意味に揃える補完である。
Snapshot所有全体の新設計や、出荷済みの操作制限を追加する変更は行っていない。

### 実装・自動検証の完了（2026-10-30）

**実装済み・実機未確認。** Snapshotは設計担当が許可した案2で完了し、固定一覧の完全再採用は§1.486へ分離した。
独立レビュー（GPT-6.1 Sol / xhigh）の最終判定は**ACCEPT、未解決P1/P2なし**。
構造的な所有境界の修正であり、症状guardではないことにも合意している。

WIP `7916a1e97`からの製品差分は、`reading_history_origin.rs`（限定解除と同path visit）、
`grid_parent.rs`（root退出の共通受付）、`app.rs`（同値判定/明示intent/native配送）、
`collection_navigation.rs`（完全帰路のnative配送）。テストは`history_parent_1473.rs`、`startup_ops.rs`、
`collection_grid.rs`、`tests.rs`を更新した。文書は本書、reading-history/top-level-grid/keymap/Snapshot設計、
backlog、detached§11、マニュアルbooks/tut-readingの2ページを更新した。

NativeOuter補完後の絞り込み61件は60成功・実RAR1失敗（66.660s、`target/section1473-targeted-5.log`）。
このRARの15秒timeoutは新test helperのpoll不足で、既存RAR試験と同じheadlessの変換メッセージ回収/ZIP列挙を追加した。
Confirm/Errorは失敗のままとし、conversion/ZIP ownerの終了も待つ。再試験は1成功・0失敗・0.44s
（`target/section1473-rar-targeted-6.log`）。製品側の回復処理・待ち時間延長・テストignoreは追加していない。

その後の最終全library gateは**14,965実行、14,965成功、失敗0、既存skip65**（574.348s）。
JUnitから抽出した§1.473の**53件は全成功**。元のroot60条件/Tag4条件の赤2件に加え、
§9.1〜15の各fixture、Snapshot限定解除、同path H1/H2、quick再選択/明示Menu、Collection native/RAR/物理子も含む。
同じソースに対する再試行はなく、初回全libraryの7失敗と追加再現の赤は修正後に上記gateで解消した。

| 最終コマンド（Cargoは全て`MSBUILDDISABLENODEREUSE=1`） | 結果・証拠 |
| --- | --- |
| `cargo fmt`（引数なし）/ `cargo fmt --check` | exit 0。最終test fixtureを含め全体整形、checkも成功 |
| `cargo check -p mimageviewer --bin mimageviewer-core -j 1` | exit 0、`target/section1473-core-check-final-2.log` |
| `cargo check -p mimageviewer --lib --tests --features test-script -j 1` | exit 0、`target/section1473-test-check-final-2.log` |
| `scripts/test-focused.ps1` | 共通/文書変更としてexit 2、`target/section1473-focused-final.log`。指示どおり全libraryへ進んだ |
| `scripts/test-lib-full.ps1` | **exit 0、14,965成功/65既存skip**、`target/section1473-lib-full-2.log` |
| `python scripts/check_ui_glyphs.py` | exit 0、危険なUI glyphなし |
| `cargo run --locked -p viewer_context_audit` | **exit 0・違反0**、`target/section1473-context-audit-final-2.log`。registryの公開API追加はなく、登録表の変更は不要 |
| `git diff --check` | 成功。gate後の結果記録は文書のみで、Rust/Cargo/vendor/runtime入力は変更していない |

最終nextest生ログとJUnitは `target/test-lib-full/runs/20261030T040411463Z-f7464765/`、
実行時成功台帳は `target/test-lib-full/971db8bf344eed9573c78a7fac9cf437b6c553e2ec73246019dafc0ef0c5dc6e.json`。
今回のユーザー指定範囲はtest-lib-fullまでであり、workspace/vendor/doc gateは追加実行していない。
関連仕様・キー仕様・Snapshot設計の例外・マニュアル2ページ・backlog§1.473/§1.486・detached§11を更新した。
製品起動・desktop操作・利用者の設定/実データの変更・確認用build・コミットは行っていない。

### 実装レビュー修正（`c494cb0b9`へのP2・P3、2026-10-30）

P2の原因は、`FolderHistoryPlan::reading_history_root_exit`がBack全体から最後の同location・同routeを
入口としていたこと。`Path(O)`と、閲覧履歴への帰路を持つ`Physical(O)`も場所の同値判定では一致する。
`O → H → O → ★一覧 → 親でO → 親でH`では、入場後の再訪を入口として誤認し、
最初の入口と自己markerをBackに残してしまう。

検索範囲を、Backにある**最初の現visit root markerの手前**に限定する。
同visit markerがBackに無い場合は、表示中のrootを境界としてBack全体を対象にする。
境界前に入口が見つからない場合は、承認済みの通常Direct退出を使う。
別visitを境界にせず、新しいfield・履歴の推定・guardは加えない。
退出先のpayload、prepare/adoption、通常の1件Back/Forward、contextとdetachedの所有範囲は変えない。

追加回帰:

- `section1473_root_exit_plan_excludes_equal_revisits_after_first_own_marker`:
  同場所・同routeの再訪1回/複数回と、入口が欠落して再訪だけ残ったDirectを検査する。
  計画の元/currentおよびBack/Forward全文を比較する。
- `section1473_equal_entrance_revisit_via_rating_keeps_complete_back_forward`:
  通常版/portable相当の一時data_dirで上記の実handler往復を通す。
  最終退出前の`H / [X,O,H,Physical(O)] / [F]`から、
  `O / [X] / [F,H,Physical(O),H]`への全履歴遷移を検査する。

P3は`docs/spec.md`の親ボタンとBS仕様へ、入口/DriveListへの退出、タグの共通解決、
★固定からMenuで開いた閲覧履歴の限定解除と親ボタンdisabledの例外を反映した。

修正前の追加2件はいずれも期待どおりBack全文の比較で失敗（exit 100、
`target/section1473-review-red.log`）。修正後の§1.473絞り込みは**55件全成功**（57.278s、
`target/section1473-review-targeted.log`）。限定独立レビュー（GPT-6.1 Sol / xhigh、読み取り専用）も
P2・P3解消、追加指摘なし。設計からの逸脱・操作制限・新しい復元経路はない。

| 再検証コマンド（Cargoは全て`MSBUILDDISABLENODEREUSE=1`） | 結果・証拠 |
| --- | --- |
| `cargo fmt`（引数なし） | exit 0 |
| `scripts/test-focused.ps1` | 文書を含むためexit 2、`target/section1473-review-focused.log`。全libraryへ進んだ |
| `scripts/test-lib-full.ps1` | **exit 0、14,967成功・失敗0・既存skip65**、581.272s、`target/section1473-review-lib-full.log` |
| `cargo run --locked -p viewer_context_audit` | **exit 0・違反0**、`target/section1473-review-context-audit.log`。新しい公開APIはなく登録追加不要 |
| `git diff --check` | exit 0 |

全libraryのJUnitと台帳の件数・exitを照合し、§1.473の55件も全成功と確認した。
実行ログ/JUnitは`target/test-lib-full/runs/20261030T043858389Z-c46bbbe8/`、
成功台帳は`target/test-lib-full/b45dd14c9b8efc1ec7447d9773427d221413c17f144d1e5f4bfccc2c526e043e.json`。
検証後の変更はこの結果記録のみ。製品起動・desktop操作・利用者設定/実データの変更・
確認用build・コミットは行っていない。

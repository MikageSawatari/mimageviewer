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

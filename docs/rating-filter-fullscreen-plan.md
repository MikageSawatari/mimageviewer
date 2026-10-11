# 評価変更で読書ページが絞り込みから外れる場合 (§1.371)

2026-10-11 / Stage 1 **R2 設計案**。再照合対象: `next-tray` HEAD `c9ae399e3`。master `32aa7a090` との `src/` 差分なし。R1 の調査開始点は `ed0be21fa`。
製品コード未変更、実機再現未実施。観測は [バックログ §1.371](next-release-backlog.md) の利用者報告、以下は現ソースと独立レビュー5件の照合結果である。
R2 の構造合意・独立再レビューは未取得。設計担当・独立レビュアーの合意前に実装しない。

## 1. 契約と現コードの差分

- 利用者決定 B: 評価変更で現在ページが表示条件から外れたら、読む順の次の残存ページ、なければ前へ移る。残存ページがゼロなら**現在のページ一覧**へ戻る。親フォルダへの自動退出ではない。評価トーストを残し、Ctrl+Z は評価を戻して元画像へ戻る。フィルターを解除しない。
- 不変条件: **表示集合を採用した境界**で、filter に従う reader の `fullscreen_idx` はその context の採用済み読書対象集合に属するか `None`。リング中は評価値の保存・表示と集合採用を分離し、評価による所属変更を終了まで保留する (§5)。旧集合に存在する現ページを表示し続け、集合外ページの例外 slot を追加しない。`selected` は保存対象の代用品にしない。見開きの相方、連結の表示 unit も採用済み集合から解決する。
- `set_rating_result_inner` → `write_user_rating_shared_deferred` は SQLite 成功後にだけ session generation を進める (`src/app.rs:70364,70622`)。`publish_current_context_rating_writes` は cache・検索・Rating/Smart membership・visible order を公開するが、全画面ページを調停しない (`:70498`)。
- `apply_rating_edit_to_fullscreen_item` はその公開後さらに `rebuild_visible_indices()` を呼ぶ (`:71346`)。コンテナ setter (`:71413`) と XMP hydration (`:71730` 以降) にも重複再計算がある。
- `fullscreen_seek_info` は `get_still_image_indices()` 内の現在ページ位置を `?` で探す (`src/ui_fullscreen.rs:21234`)。除外されたページなら None。strip geometry (`:20910`)、ページ番号 (`:21045`)、seek 描画 (`:21653`) が同じ情報に依存する。キャッシュ失効では直らず、集合外の anchor を作る公開経路が根因。
- **バックログとの差異**: 一覧の現実装 `redirect_selected_to_visible` は raw items index の `prev.or(next)`、前優先 (`src/app.rs:69695` 付近)。決定 B の次優先でも Details の表示順でもない。評価由来の選択移動を今回の共通規則へ揃える。履歴復元等の一般的な選択補正まで変更しない。
- **追加前提**: `open_fullscreen...` (`src/app.rs:65671`) と `close_fullscreen_now` (`:74891`) は `clear_meta_undo()` を呼ぶ。通常ナビをそのまま呼ぶだけでは自動移動後・最後の一枚退出後の Ctrl+Z を失う。`UndoEntry::Rating` は現在、値とキーのみで表示復帰先を持たない (`src/undo_stack.rs:133` 付近)。
- `sync_current_context_rating_session_writes` はcache同期だけで seen generation を進める (`src/app.rs:70462`)。getter/setterの事前同期からも呼ばれるため、「cache同期済み」を「集合とreaderまで公開済み」の代わりにしない。新しいseenフラグを足さず、既存seenは共通publication完了時にだけ進める。読み取り側のkey overlayはpublicationを消費しない。
- **R1の前提を訂正**: `poll_rating_view` はpage-edit/tag stampだけを検査し、rating stampは採用判定に使わず後からsession overlayする (`app.rs:34548–34560`)。検索もpage-edit stampとsurface/request条件の後にoverlayする (`global_search_ui.rs:2687,2800`)。消えたslotはoverlayで復元できない。R2の採用前評価検証 (§3.3) は新たに必要な変更であり、現機能とは扱わない。
- **swapの契約を訂正**: `plan_mount` → swap → `finish_mount` の間は `ContextTable.pending` が存在し、`projected_id()` 等は利用不可 (`viewer_context_registry.rs:175,4052`)。現在のswap内publication (`:2213,2792`) に移動を加えてはいけない。安定したowner境界へ移す (§3.1)。
- 本調査では導入時期のタグ・履歴は再調査していない。現在の状態不整合を確認したのであり、利用者の実機報告を新たに再現したとは主張しない。

## 2. 同型の入口・context の棚卸し

| 入口 / producer | 現経路と今回の扱い |
| --- | --- |
| F1–F5 / F6、カスタム評価キー、StepUp/Down | `[Rating]` の `RatingItem*` → `consume_rating_action` → `apply_rating_edit_to_fullscreen_item` (`ui_fullscreen.rs:30603`)。一覧は `apply_rating_edit_to_selection` (`app.rs:53287,72082`)。同じ保存・公開へ統合し、上限 no-op は保存・Undo・移動なし。新 Action / 固定キーを足さない。 |
| Shift+F1–F6、カスタム container/step キー | `apply_rating_edit_to_current_container` → `write_container_rating_for_target` (`app.rs:71316,71413`)。子ページの評価ではない。通常は子ページ集合不変なので移動なし。同じキーを持つ親一覧の行だけ membership が変わる。合成一覧は `current_container_rating_target` が拒否する既存契約を維持。 |
| 情報パネル・マウスの★、音楽パネル | `ui_metadata_panel.rs:2718` の `set_rating` / `set_current_folder_rating`、`ui_music_panels.rs:1232` の `set_rating`。表示ページの idx を保存対象として捕捉する。キーボードだけ直してこの入口を残さない。 |
| メイン toolbar/右クリックの評価メニュー | `ui_main.rs:11227` の selection/container setter。同じ共通近傍規則。画像全画面 HUD の独立した★メニューは見当たらず、★操作は情報パネル・キー・リング。 |
| native 動画キー / HUD の★ | `app/native_video.rs:12556` の共通 rating helper、`:6751` の `NativeVideoOutputEvent::SetRating` → `set_rating`。静止画 strip とは consumer が違うが、表示集合から外れる anchor の問題は同型。動画/音声を勝手に非対象にせず、既存 media open/close/presenter owner を使う回帰対象。 |
| マウス/パッドのリング設定 picker | `app/gamepad_input.rs:4071` の固定 key/owner に対する live preview は DB 保存済み。R2では値のpreviewを維持し、評価による集合変更・自動移動だけをpicker終端まで保留する (§5)。`commit_live_picker_undo` (`:4878`) のbefore/最終成功値による一件Undoを維持する。 |
| Undo / Redo | `undo_ops.rs:184,233,321` → `write_user_ratings_shared` の transaction 成功後に同じ publication。Undo は後述の復帰要求、Redo は再び次/前/一覧の規則を使う。 |
| Remote rating | `remote_ipc/ui.rs:3040` の `persist_remote_rating` → `write_user_rating_shared`。RemoteActive の前に全ローカル reader を閉じる barrier (`:1116`) があるため、「Remote とローカル全画面が同時に動く」は通常の状態に含めない。値/一覧公開とログを共有し、Remote のページ位置・immutable listing をPC都合で動かさない。ローカル Undo を新設しない。返却時は既存 reload。 |
| XMP hydration / metadata import | `app.rs:71730` の session ledger publication、`app/metadata_import_refresh.rs` と `apply_metadata_import_terminal_refresh` の context 指定 snapshot 採用。同じ集合不変条件を確認する外部 producer。ユーザーの★操作とは区別し、新しい Undo を作らない。hydration の stale 明示clear保護は維持。 |

| 表示文脈 / consumer | 適用範囲 |
| --- | --- |
| 通常画像、本フォルダ、ZIP/CBZ・直接 RAR・入れ子 ZIP、PDF、EPUB | filter に従う Main scope に適用。`rating_path_key` / `page_path_key` を使い、ZIP の entry、PDF page 番号、EPUB の元論理パスを維持。変換書庫の既存 source/cache 対応を変えない。Shift+F は page key とは別の container key。 |
| Ctrl+F、Ctrl+S、Ctrl+G 合成/ドリル、tag、Rating、Smart Folder、bookmark/history 一覧、stack flat reader | その context の実効 membership と `current_reader_order` を使う。Rating の固定★、Smart の rating mask による離脱も対象。Smart は元 slot を残すので即時判定可能 (`smart_folder.rs:1291`)。検索/Rating の worker による後続 items 差し替えでも同じ owner に接続する。Stack 集約セルは評価非対象、flat の実ページは対象。 |
| Collection root | `ViewerNavigationScope::CollectionRoot` は installed collection order が正本で App-global filter を無視 (`app.rs:67957`)。星・DBを同期するが、評価を理由に reader/collection 順序を動かさない。物理 child は実際の child scope の規則。 |
| F12 / always-new / active・parked detached | presentation 名では分岐しない。`DetachedPhysical` は global filter を無視 (`app.rs:67966`) し、全物理ページを維持する。main の行が消えても独立窓は移動・閉鎖しない。Main scope を保持したreaderなら同じ規則。publicationはswap primitiveから退避前・mount/設定投影完了後の安定境界へ移す。別窓のidx・worker・cancel tokenを流用しない (§3.1)。 |
| ★固定 snapshot / DirectMedia | 既存の snapshot membership と DirectMedia reader order が正本。今回の理由で snapshot を解除・原一覧をreloadしない。実効 reader 集合が変わらなければ移動なし。既存 snapshot index-swap owner (`app/snapshot_ops.rs:410` 付近) を二重に走らせない。 |

既存の `effective_rating_filter()` の container-open suppression は保持する。今回の評価変更・Undo のために新しい suppression を発動したり、`settings.rating_filter` を全ONに書き換えたりしない。main の明示 filter 操作は既存どおり suppression を落とす (`ui_main.rs:11211,12659`)。

## 3. 集合owner・採用境界・一度だけの移動決定

**ownerは既存viewer contextの表示集合公開処理**。Appはmounted projection、`ViewerContextBundle`は退避した同じ状態を所有する。保存/Undoはstable key、items・表示順・reader・cache・worker・採用世代はcontext-local。mutationの入口は `publish_current_context_rating_writes`、同期/worker結果/設定変更の**集合採用出口**は一つの `reconcile_rating_membership` (仮称) に揃える。入力helper、HUD描画、seek、worker request時点では移動を決めない。

### 3.1 ContextTableの安定境界 (レビューP1-1)

- `swap_viewer_context_bundle` / `execute_viewer_context_op` はpayloadの交換・格納だけを行うprimitiveとする。内部でpublication、context ID取得による新しい配送、fullscreen open/close/seek、request起動、再mountを始めない。今あるswap冒頭/末尾のrating publicationは撤去する。設定の保存/投影に既存の処理が必要でも、その処理からナビゲーションを呼ばない。
- **outgoing**: `with_viewer_context` の `plan_mount` より前、tableが安定し旧owner/settingsが一致している間に、未公開評価を同ownerへ反映・調停してから退避する。IDはこの時点で捕捉する。callback後にoriginalへ戻す場合も、再び `plan_mount(original)` より前にcallback側ownerを同じ出口へ通す。rating起因のlandingはmounted owner内で完了し、owner/window bindingを変更するopen・親退出・再mountを開始しない。
- **incoming**: `execute...` → `finish_mount` → incomingのfavorite/effective settingsの投影完了 → 同ownerの未公開評価を公開 → page-edit等の通常poll/callback、の順。現在のswap末尾のfavorite投影より後でも、必ず `finish_mount` より後であること。投影のためにprimitive外へ移す処理は既存favorite設定ownerにまとめる。context IDとfilterはこの完成した投影から取得する。
- mountだけでなく、`build_viewer_context`、commit/abort、`fork_mounted_context`、promote、retireの **plan/execute/finish** を同じ契約で棚卸しする。退避/fork元はplan前に調停。新build中の未確定payloadへ表示移動を配送せず、commit済みownerの採用/初回mountで公開する。commit/abort/promoteで復元されたprojectionはfinishと設定投影後に公開。retireするpayloadの復帰intentは終了し、死んだreaderを再openしない。
- plan前のpublicationでidx/anchorが変わり得るため、呼出し側が既に選んだopen/fork対象はstable keyで捕捉し、調停後に新slotへ解決してspecを作る。古いidxで別ページをforkしない。明示navigationがある場合はそのintentを採用へ渡し、余分な自動landingを行わない。
- `finish_*`までのtable遷移と、その前後の集合採用を混ぜない。これを処理順の構造で保証し、pending時は黙ってskipするguardや「後で移動する」App-global boolを足さない。publication自身から別contextをmountする再入も禁止。origin contextへのUndo配送は外側の安定したdispatcherが行う。
- 単なるread-only mountは新しいDB mutation/世代を作らない。未公開writeがなければ無処理。未公開writeがあるownerだけ集合/cache/readerを更新し、兄弟contextのchannel・queue・generation・cancel tokenを触らない。pickerのoriginを一時mountするだけならpicker終端にはしない (§5)。

### 3.2 集合決定と§1.370の適用を分ける (レビューP2-5)

1. 保存対象のkey、before、Undo originをsession値の同期後・保存前に捕捉する。採用直前にはcontext ID/scope、現在anchor/selectedのidentity、旧grid/reader順を在メモリで捕捉する。request開始時のidx/anchorを予約しない。入力の途中でcacheが更新済みでも、旧表示順は旧集合である。
2. SQLite成功 → session ledger → contextのcache/集合更新 → visible・Details・reader order採用を一回行う。setter/container/fullscreen/hydration末尾の重複rebuildを取り除く。ringでは値だけをpreviewし所属採用は§5の終端へ。既存seen generationは**表示投影の公開完了**で進め、getterのcache overlayだけでは進めない。
3. 採用処理はtypedな目的を `通常補正 / 評価所属調停 / 同一identityの再配置` として既存rebuild出口へ渡す (仮称)。評価所属調停では末尾の一般 `ensure_selected_visible_or_first` / `redirect_selected_to_visible` の前優先補正 (`app.rs:68180,69695`) を実行しない。検索/Rating/snapshotのinstall内部の「先頭へ選択」「close時に旧readerをselectedへ戻す」も先行決定させない。新order/cacheの作成とcheckedのpruneは実施し、最後に集合ownerが唯一の行先を決める。前へ動かしてから次へ直す二段処理は禁止。
4. 純粋な共通近傍helperは `Keep(identity) / Move(identity) / Empty` を返す。anchorより後ろの最初の残存候補、なければ前を逆順に探す。次は**読む順のordinalが増える方向**。RTLでもraw idx比較や左右キーで反転しない。gridはgrid orderの選択可能行、still readerはstill-page order、動画/音声は既存media eligibilityを使う。候補domainだけを変え、決定規則を共有する。
5. readerが動く場合はその着地点をcontextのgrid cursorの行先にもする。readerがKeepならselectedはgrid domainで独立に補正する。reader domainがEmptyなら `ReturnToGrid` とし、gridにcontainer等が残ればgrid domainで一度決めた行、gridも空なら `None` を適用する。container行を「次の画像」にしない。Undoの有効な `RestoreOrigin` はこの決定入力として優先し、通常近傍移動の後に二度目の移動を行わない (§4)。
6. **集合ownerは行先を決定するだけ、§1.370 cursor-move ownerが適用する**。`move_grid_cursor(target)` / `apply_grid_cursor_move(..., GridCursorMove::Move)` に、新itemsで解決した行先を一回渡す。`selected` と `grid_click_selection_anchor` は同じgeneration/identityの着地点へ更新し、`None`なら両方消す。scroll/resume/checkedの方針は呼出しownerが保持。fullscreen landing/closeの内部から旧ページのcursor補正を重ねない。
7. items再配置で**同じidentityが残るだけ**ならnavigationではない。§1.370の既存paired remapによりselectedとShift+click anchorをそれぞれstable keyから新generationへ再配置する。anchorはselectedと異なっていてよい。除外・自動移動・Undo復帰でidentityが変わるときだけ `Move` として新起点を作る。raw idxの偶然の一致をKeepとみなさない。

§1.370の読み取り根拠: `C:/home/mimageviewer-nexteffetune` の2026-10-11時点の未コミットdiffと `target/L-1370-impl-msg.txt` を確認した。`app.rs:14403` の `GridCursorMove`、`:14410` の `apply_grid_cursor_move`、`:14424` のsaved-grid用同一規則、`:69631` の `move_grid_cursor`、`ui_main.rs` のサムネイル/Details共通適用、smart保存gridとbundleのpaired remapが接続先。handoffも§1.371で行先を所有し、決定後に既存cursor ownerへ接続する分担を明記している。`restore_grid_cursor_to_fullscreen_item` (`:75190`) はまだ一般redirectを呼ぶため、評価退出では既決定行先の適用へ通し二重補正しない。`sync_main_selection_from_viewer_idx` (`:57776`) は同idxのlinked同期ならanchorを保持するため、identityが変わるMoveをその整数比較だけに任せず `move_grid_cursor` へ明示する。独立detachedからmainへcursorを配送しない既存scope契約も維持する。これは未統合の作業として扱い、同worktreeは編集しない。Stage 2着手時は完成handoffと統合後コードを再照合し、同ownerを再実装しない。handoff内のPASS記録は§1.370担当の証拠であり、R2の検証実施とは数えない。

明示rating-filter変更 (toolbar/facet/menu、設定採用・復元) も「設定採用→集合採用」の同じ出口へ通す。保存ではないのでrating Undo/toastは作らない。旧visibleだけでは新entrantの位置を決められないため、installed items/Details sort ownerからhidden行を含む候補順を**採用中の一時値**として得て、新membershipの近傍を解決する。新slotは既存のappend/prepare順に統合する。「旧visible全離脱、新entrantのみ残る」をEmptyと誤判定しない。常設のfullscreen専用リストは持たない。

`Move`は既存fullscreen landing/open・連結seekとロード/presenter契約を使う。新集合からstrip/seek/page number・見開き・分割slice・連結unitを解決し直す。items交換の再入場もこの採用処理でidentity/Undo/行先を確定し、独自のfs cache全移植や次frameの別pending reopenを作らない。媒体の既存非同期close終端には同じtyped原因/決定行先を渡す。Emptyは同じfiltered page gridへ既存teardownで戻し、auto-open→親退出を呼ばない。入力/描画callerはtransition outcomeを受け、古いfs_idxでその後の操作を続けない。

### 3.3 membershipを決めた評価世代の採用前検証 (レビューP1-2)

**slotが消えたprepared setに最新星をoverlayしても集合は直らない**。評価でfilter/orderを決めたproducerは、実際に読んだ評価factsの有効性をitems交換・cursor変更・fs teardownの**前**に検査する。page-edit/tag/surface/requestの現行検査に追加し、overlayは有効な集合の表示値同期にだけ使う。

- Rating: 既存 `RatingViewPreparedItems.rating_stamp` / `RATING_WRITES` を使う。workerはrating query開始前/全membership・rating sort計算後のstampが `EditWriteEpoch::read_is_stable` を満たす結果だけを返す (現行はbeforeを格納するだけ)。採用直前は `RATING_WRITES.accepts(prepared.rating_stamp)` に加えて、pendingが捕捉したsession write generationが現在値と同じであることを検査する。view/filter/sort/visibility/context/request sequence/source generationも従来どおり一致させる。
- 検索: `SearchPreparedItems` は現状rating stampを持たない。**membershipに使ったfactsの来歴**をpreparedに持たせる。通常ソートでもrating filterを使うため検査対象。workerが保持する `all_hits.stars` の出所を、DB読取stampと適用済みsession generationとして所有する。`RatedBatch` のhydrationにも来歴を付け、`RatingWrites` commandはupdatesとthrough-generationを渡してworkerのkey overlay/source世代を進める。新しいwishを作る前にその世代までのupdatesを同channelへ送る。古いBatchが後着してもkey overlayで既知の最終値を上書きさせない。
- Searchの `View` prepareは、累積hit factsのstampが現在DB epochに有効であり、workerがwishのsession世代まで反映したことを確認してからfilter/aggregation/sortを作る。DB epochが変わりcacheが古ければ、既存rating workerで全candidate keyのfactsを再読込・既知のsession overlay→membershipを作り直す。開始/終了stampの安定を記録する。UI採用はpreparedの実facts stampに `RATING_WRITES.accepts`、prepared/wish/currentのsession世代一致、既存query/view/filter/context/source/sequence条件を検査する。**prepare時の新stampだけを古いhit値に付けて新鮮と偽装しない**。UI側 `all_hits` / countsの置換もこの有効な結果に含める。
- Smart: 既存pendingのwrite-generation/context/items/page-identity検査 (`smart_folder.rs:1600`)、slotを残して最新cacheをmaskへ適用する規則を維持する。評価DBからmembershipを準備する経路のstamp検証も同じ契約に揃える。collection等で評価がmembershipに関係しないsetは、値同期のためだけに再列挙しない。
- 無効な結果は**一切採用せず**既存request ownerのdesired/latestを現在factsへ更新してprepareする。UI待ち・sleep・rollback・DB retryはない。通常の入力によるsupersessionであり、まれな失敗の回復ループを導入する話ではない。ownerの有効な復帰intentだけを最新requestへ引き継ぎ、古いcompletionから復帰intentを作り直さない (§4)。

固定する順序: Aを評価して離脱→Aなしの古い結果Readyを保持→Undo成功/Aへの即時または最新prepareによる復帰→古いReadyを解放。古い結果は採用もcursor/reader変更もせず、Aが再びBへ飛ばない。Ratingと検索の通常/評価ソート、streaming後着Batchのそれぞれで検証する。採用処理で保留編集を保存する場合、その保存後に全stamp検査を行い、検査と採用の間に新しいrating保存を挟まない。

## 4. Undoの所有・復帰intentの終端 (レビューP2-3)

- `UndoEntry::Rating` に一つのtyped origin (`Grid / Viewer { context_id, listing_identity, page_key, presentation }`、仮称) を持たせる。RatingChangeのstable keyと同じ操作を所有し、別の「最後のratingページ」やpending reopenを足さない。grid/Remote/container-only変更から無条件にreaderを開かない。
- 自動所属移動・最後の一枚の一覧復帰は既存open/closeへ内部causeとして渡し、**ratingのUndo/Redoだけ保持**する。clearの必要なidx-only adjustment/layer Undoや保留drag/tagを丸ごと `take` して戻さない。通常の手動open/close・scope変更・context破棄のclear方針は維持し、同時に以下のrequest intentも終了する。
- Ctrl+ZはDB transaction成功後、同じ集合採用へ `RestoreOrigin` を渡す。有効な元keyが同context/listingで対象へ戻れば通常近傍より優先して一回着地する。最後の一枚の自動退出後も、同一覧のUndoなら元presentationで元画像を開く。非評価filterを解除しない。元keyが現在条件に合わない/消失している場合は値を書き戻すだけとし、復帰不可を通知する。旧窓/旧scopeを再作成しない。
- 元slotが消えた検索/Rating等では、復帰は**既存membership request ownerの現在desired wishが持つtyped adoption intent**とする。`Reconcile / RestoreOrigin(origin)` (仮称) のいずれか一つであり、App-globalの別pending stateを併設しない。completionが運ぶのはsetと来歴。復帰権限は返ってきたwishの古いコピーではなく、**採用時のlive owner**から得る。

| 通常入力/終端 | 復帰intentとmembership requestの扱い |
| --- | --- |
| Undo成功 | 現在intentをそのUndoのRestoreOriginへ置換し、最新write generationのdesired requestへ結び付ける。同期復帰できたら消費し、後続workerは通常Reconcile。 |
| Undo待ち中の別Undo | 後のUndoのorigin/世代へ置換する。古いReadyは採用検証で失効し、二つの復帰を予約しない。 |
| 同じ一覧内の手動next/prev、seek、page指定、grid click/open | **操作受理境界で**live ownerのRestoreOriginをReconcileへ置換する。ページ送りの境界no-opでも明示移動意思を優先する。必要なmembership refresh自体は残し、完了時は最新anchorで調停する。input_seq全体で他の無関係な入力まで取消しない。 |
| Esc/通常reader close、別scope/open/history、context retire、request cancel | close/変更前にintentを終了する。membershipだけが後から採用されてもreaderを再openしない。Undoスタックclearだけに取消を依存させない。F12の内部mount/元contextへ戻るswapは手動navigationと扱わない。 |
| Redo受理 (Ctrl+Y / Ctrl+Shift+Z) | 先行RestoreOriginを取消す。成功したafter値の最新requestはReconcileとなり、現anchorから次/前/一覧を一度決定する。保存失敗時も古いUndo復帰intentを再生せず、既存のログ/通知・Redo entry保持に従う。 |
| latestへの通常reprepare | live ownerに残るintentだけを移す。cancel済みintentをworker結果から復活させない。 |
| 有効なsetの採用 | originが対象へ戻り着地できる採用でintentを `take` して**着地前に一度だけ消費**する。既存media landing/close終端へ渡したらrequestから消す。最新の確定結果で復帰不可なら通知して終了。stale結果はconsumeしない。 |

同じ一覧のmanual navigation後のmembership採用は、そのときの現在ページがまだ所属していればKeep。自動退出後はreaderを開く権限をRestoreOriginだけに与え、Reconcileはgrid cursorを補正するだけ。別窓でUndoした場合のDB書戻しとoriginへの配送は分離し、安定したdispatcherから生存するorigin contextだけへ配送する。無関係な前面窓へ着地しない。

## 5. リングは所属採用を終端へ集約する (レビューP2-4)

**R2の採用案: DB/星のlive previewは継続し、評価によるmembership変更・移動はpickerが終わるときに一度だけ行う。** 自動移動した後もpickerを旧ページへretargetするR1案を撤回する。現在anchorを変えないので、rating入力のたびに `ring_picker_is_stale` (`gamepad_input.rs:3700`) となる組合せを除く。元slotを先に削除しないので `apply_picker_item_rating_targets` (`:4091`) の固定key→slot解決も維持できる。

- `RingPickerState` の既存owner/anchor/original-rating-recordsを操作の唯一のownerとする。その生存期間はcontextの**採用済みmembership**を保持する。星の保存→ledger→cache/★表示/保存ログは毎previewで反映するが、rating由来visible prune・Smart mask・検索/Rating items置換・count membership公開・reader移動は終端までまとめる。途中でDB値を元へ巻き戻さない。pickerのpage/選択対象、strip/seek/番号は保持されたcontext集合を使う。save値を表示判定へ直読して途中で消すconsumerも同じ集合ownerへ揃える。
- rating refreshのdesired世代は既存request ownerに最新一件として集約する。Readyを受けてもpicker所有contextのmembershipを置換しない。旧結果を個別に保存/積み上げるqueueは作らない。終端後は§3.3の検証を通る最新結果だけを採用する。開始前からin-flightのrating membership結果、previewを含んだ検索streaming、parked-ownerのpollも同じ規則で元slotを失わせない。兄弟contextの採用は保留しない。
- **全更新の停止にはしない**。GridColumns/ThumbAspect、ソート、画像の見開き/flow/方向/fit、ページ補正、動画volume/speed等は既存のlive previewを維持する。GridSortOrderは現状 `apply_sort_change_reload` (`app.rs:25684`) がRating/検索/Smart等の再列挙と結び付いているため、単にそのReadyを止めるだけではソートpreviewを失う。リング中の同一scopeソートは、**採用済みmember identityを入力に既存sort/materialize ownerで順序だけ準備・適用**する。新しいrating membershipを混ぜず、必要なmeta読取は既存workerで行う。適用はpaired identity remap (§3.2) で選択/対象slotを維持する。sort変更をcloseまで延期したり、その行をdisableしたりしない。終端後の通常membership prepareには最新sort設定を渡す。
- したがって「集合固定」は**ratingによる所属の固定**であり、画面/layout/order全体の凍結ではない。ソートに必要な保持memberはcontextの現在items/rowsから得る (Rating rows/Search hit sourceを含む)。全文脈の別reader列やfilter外slotを追加しない。sort-only requestは既存owner内のtyped prepare目的とし、同じfilter/membership revisionを入力にする。previewによるrating世代変更だけを理由に固定所属を破棄せず、rating sortの値/時刻は§3.3で検証した最新factsを使う。
- 各行の所有範囲を変えない。ItemRatingはopening時の固定key群、ContainerRatingはopening時のcontainer target、PostFilter等の個別補正は元page key、SpreadMode/ReadingFlow/Direction/Fitはそのviewer/book設定owner、動画行はそのownerのplayer、grid行は既存grid_sort_target/settings ownerへ適用する。全行を「元idx」に統一しない。元page identityはまだ同readerで表示されているので、現在idxの使用が他ページへの誤配送にならない。itemsのorder変更だけならkeyでidxを再解決する。
- **終端の順**: opening ownerを安定境界で配送/mount → 最終previewの既存確定処理 → 成功済みledgerからrating Undoを一度記録 → commit-only/非rating行の既存確定・native HUD後始末 → pickerの所属保持を終える → 最新集合を採用し一度調停 → toastを着地面へ出す。`commit_ring_picker` は現在 `take` 後にfinalizeする (`:4751`) ため、`Option`不在だけで「保持終了」と推測しない。takeしたpickerを**ローカルのtyped終端処理**として確定/採用まで渡す。parallelなclosing boolやpending move fieldは追加しない。
- 通常commit、マウスpicker終端、パッド無効化/切断、sidecar modal、stale破棄、手動reader close/別場所open、owner retireをこの終端へ接続する。強制終端は既存 `published_live_picker_rating_changes` と `finish_gamepad_input_for_sidecar_restore` (`:2535`) の成功済みpreviewのみ記録する規則を使い、commit-only行を勝手に実行せず、新しい保存/rollbackを足さない。現在 `close_stale_ring_picker` (`:3733`) はtake/dropだけなので、成功済みratingのUndoと保持集合をここで取りこぼさないよう変更が必要。元owner消失ならstable keyの成功記録のみ終端し、死んだviewerへ適用しない。
- 通常の別contextへの一時mount・フォーカス移動だけでpickerを終わらせない。pickerはopening ownerに残り、そのownerへ入力を配送する。元scope/readerを実際に変更する操作は**その操作前**にpickerを既存の終端方針で終了し、操作は続行する。明示rating-filter変更も終了→設定採用→最新集合の一決定とし、入力を拒否しない。手動navigation/closeが終端理由なら、その明示intentを同じ採用へ渡して自動の中間landingを省き、ユーザーの移動/closeを一回適用する。自動的なRestoreOriginは§4で取消す。
- 最後の一枚をpreviewで除外しても、リングを操作中はそのページ・全行previewを維持する。通常picker closeで初めて現在gridへ戻り、rating Undoは残る。除外→再び一致のpreviewならclose時Keep。複数previewはbefore→**最終成功値**の一件Undoとし、DB失敗時は成功した値までを用いる。成功していない選択値で集合/Undoを作らない。

比較: 即時移動し固定keyでpickerを継続する案は、消失slotの再実体化、page個別行とviewer全体行のretarget、stale判定の例外を増やすため不採用。membership移動をpicker強制終了にする案は次の行/再評価を制限するため不採用。全worker採用/ソートもcloseまで止める案も既存previewを遅らせるため不採用。所属と順序の責任を分けて終端へまとめる案では操作を禁止せず、既存行のpreview/commit契約を保てる。実装でこれを保てず操作制限が必要と判明したら、実装せず費用と選択肢を利用者へ返す。

## 6. 成功ログ・toast・失敗方針

- `[RATING]` save成功ログは**SQLite成功→ledger公開の境界**に集約する。source、origin/projected context ID、write/items generation、target idx (未投影はnone)、stable rating key、before/after、settings/effective filter、採用済みreader所属と新値のfilter判定を記録する。ログのための同期I/Oは追加しない。未知の旧値は `before=unknown` と情報源を明記し、未評価0と偽らない。通常fullscreenのbefore/originは必ず捕捉する。
- 後続projectionログは同write generationで相関し、採用世代・旧/新所属・`Keep/Move/Grid`・着地点・request intentを記録する。保存時にまだ移動を決めていなければ `projection=picker-held / preparing / unmounted` と記録し、成功saveで行先を捏造しない。picker終端/mountを二度目のDB保存としてログしない。stale rejectはprepared/currentのfacts・session世代と理由を記録する。
- rating toastは既存★文字列のまま、採用/自動移動後の表示面へ配送する。移動ならorigin viewer、ゼロならMainWindow。リングpreview中の表示は保ち、終端で成功した最終評価のtoastを着地面へ表示する。通常closeのtoast cleanupで消さない。DB失敗は成功世代・成功toast・Undo・移動を公開しない。XMP後続失敗は既存通知とし、DB評価を巻き戻さない。
- **まれな失敗は割り切り (ログ + 通知)**。DB retry、completion待ち、typed失敗分類、多段回復、DB削除/再作成は足さない。通常の連続入力・Undo/Redo・worker順序・context移動は上記の所有と採用で構造的に扱う。

## 7. 不採用案・必須テスト・引き継ぎ

CLAUDE.md「設計の簡素化」に従い、picker中の自動移動/vanished slotを終端集約でなくす。復帰権限は既存requestの一intentにし、最新結果へoverlayして欠落slotを復活させる回復機構を作らない。swapのpending途中でナビゲーションを始める組合せは安定境界への移動で除く。

- 独立したfullscreen列、無条件filter解除/suppression、seek Noneの全items fallback、repaint/遅延パッチ: 集合とreaderの二正本を残すため不採用。
- pending bool/idx、別復帰queue、多段rollback: 既存集合/request/picker ownerが同じ状態を既に持つため不採用。
- rating保存のモーダル化/キー連打制限: 同期保存の成功境界と集合採用へまとめれば不要で、既存操作を不便にするため不採用。DB非同期化は本項に混ぜない。
- 設定変更で常にreaderをclose/reload: 次へ進む決定B、zoom/連結/窓identityを壊すため不採用。ゼロだけ通常teardown、実際のitems交換だけ既存再入場を使う。
- Undoスタック全体の退避/復元: idx-only編集や他contextを復活させるため不採用。rating entryのoriginと内部causeに限定する。

実装時の必須テスト (R2では追加・実行していない):

1. 純粋近傍: 中/先頭/末尾、複数同時離脱、ゼロ、Details降順/非単調idx、RTL/LTR、新entrantのみ残存、gridとstill/media domainの差。次→前、残存anchorはKeep。一般補正を先行させず、cursor apply/reader landing各一回をassertする。
2. rating handler: F1–F6/custom/step、selected≠fullscreen、全表示/未評価除外filter、情報パネル/マウス★/native HUD。stable DB key/cache/generation/所属/行先、publication/成功ログ/toast、bound/no-opを検査。Image/ZIP/PDF/EPUB/入れ子・変換書庫とShift+F containerを分離し、最後のstill+container-only残存で現在gridへ戻る。
3. Undo/Redo: 自動next/prev後、最後の一枚grid後、二操作、並び替え/slot変更、非評価filter、別窓Undo。元画像/元presentation、rating entry/redo保持、filter不変、idx-only Undoが復活しないこと。**Undo→同じ一覧でmanual next/prev/seek→Ready、Undo→Esc/close→Ready、Undo→Redo→古いReady→最新Ready**を固定し、manual結果を上書き/reader再openしない。別Undoで置換、一度consume後の別workerでも再復帰しない。
4. 評価世代: **A除外→AなしReady保留→Undo成功/A復帰→古いReady解放**をRatingと検索(通常/評価ソート)で実行。set/items/fs/cursorが一切古く戻らないこと。prepare中の評価write、DB epochだけ変わるproducer、session更新と古いRatedBatch後着、filter/query/source/context/sequence変更、stamp有効でないcached factsに新stampを付けないこと。Smart現行write-generation検査も維持する。
5. Ring: preview除外→他の全行preview→再評価一致→closeはKeep、最後の一枚preview除外中もstrip/seek/番号・元slotが存在しcloseでGrid。Rating/検索/Smartとin-flight/streaming、**評価preview後のGridSortOrderのlive変更**、Details sort、commit/マウス終端/切断/無効化/modal/stale/manual close/scope変更を検証。成功済み分一件Undo、元key/owner、page個別行/全体行/playerの保存先、重複writeなし、picker-ended後の移動一回。owner Aのring中にBへmountしてもAの保持・Bの通常公開・各worker tokenが独立する。
6. ContextTable: outgoing未公開rating→mount、incoming未公開rating→mount、callback内rating→original復帰。traceで **outgoing publish→plan→swap→finish→settings projection→incoming publish** をassertし、projected-ID assertion/panic・swap内landing・再mountなし。build commit/abort、fork/promote/retire、catch-unwind後のoriginal復帰も同じ契約。read-only primitive単体テストは値交換だけで、save/移動を期待しない。
7. **§1.370統合**: auto next/prev、Empty→grid(残存container/完全空)、Undo→元画像、items並び替えでidentity存続/消失の各後に、サムネイルとDetailsのShift+clickを実行。起点/範囲/checked/scroll/generation、旧有効anchor≠selected、同じ数値idxに別identity、saved Smart root/履歴、main↔F12とA/B context分離を検査。paired remapだけの場合は旧起点identityを保持し、Moveの場合は新起点へ更新する。
8. filter変更/設定採用、Ctrl+G/Rating/Smart後続採用、open/switch/close/cancel/stale、rating sort snapshot survivor順。CollectionRoot/DetachedPhysical/DirectMedia/★固定snapshotでmembership/orderが不変なら移動なし。F12同key・異idx/active/parked、getter先行同期、zoom/window binding、兄弟cache/queue/generation/tokenの保持を確認する。
9. Remote acquire→rating→releaseのreader閉鎖/PC・Remote位置分離、XMP stale hydration/import、DB不在/Undo失敗は既存テストを再利用。失敗組合せ用回復機構を作らない。
10. 単ページ/見開き・表紙/分割/縦横連結、native動画/音声: 新集合とHUD・seek・strip・unit、空grid/toast、AI/補正/ロード中のpresentationを検証。可能なheadless snapshotを更新。実機確認は別の明示承認済み検証枠へ残す。

検証ownerは実装担当。焦点lib tests→core check→fmt、UI文字列変更ならglyph、snapshot、共有変更の完了時は `scripts/test-full.ps1`。実装後は `scripts/build-dev.ps1` で確認binaryを渡す。R2は文書のみのためtest/build不要、mIV起動なし。

Stage 2の候補所有ファイル: `app.rs` (save/publication/rebuild/open/close)、`viewer_context_registry.rs` (安定境界)、`ui_fullscreen.rs` (landing/入力後のanchor再解決)、`undo_stack.rs` / `undo_ops.rs` (origin・intent取消・rating限定保持)、`ring_shortcut.rs` / `app/gamepad_input.rs`、各rating UI、検索/Rating/Smartのprepare/採用とtests。§1.370との共通cursor ownerを先に統合/照合し、別cursor helperを追加しない。実装と同時にdisplay pipeline/keymap/rating snapshot order/関連ownership docsを更新する。native placement/focus、Remote protocolは変更不要。

## 8. R2の指摘対応・構造合意の状態

| 独立レビュー | R2の変更 |
| --- | --- |
| P1-1 swap途中のpublication | §3.1: outgoing plan前/incoming finish+settings後。primitiveは交換のみ、navigation再入禁止。必須test 6。 |
| P1-2 rating世代の採用検査がない | §1の前提訂正、§3.3:実factsのDB stamp/session世代を採用前に検証。欠落slotのoverlay修復を廃止。必須test 4。 |
| P2-3 RestoreOriginの失効不足 | §4:live request intentの取消/置換/一度consume、manual/close/Redoを共通受理境界へ。必須test 3。 |
| P2-4 ring stale/対象消失/行owner | §5:所属採用を終端まで保留、順序/全行previewを継続、行ごとの現owner維持。即時移動/強制終了案を撤回。必須test 5。 |
| P2-5 二段補正/Shift anchor | §3.2:集合ownerの唯一決定を§1.370 ownerが一回適用、再配置はpaired remap。必須test 1/7。 |

独立レビューは「集合外anchorを作る公開境界を直す方針は構造修正」と判断したが、R1の遷移境界未解決により着手合意は保留としている。R2もまだ再レビュー前。F12への対象はswapのrating publicationの移設、安定したownerの集合採用と通常landing/teardownへの接続、cursorのpaired適用であり、detached述語/viewport/geometry/配置/focusのヒューリスティックを追加しない。**R2について設計担当と独立レビュアーが合意した後、実装前に [detached plan §11](detached-rework-plan.md#11-リワーク外からの変更記録) へ対象・理由・合意の参照を記録する**。今回は合意済みと偽る記録を同planへ追加しない。

## 利用者決定 (2026-10-11)

- ★による移動は一覧・fullscreenとも「次を優先、無ければ前」に揃える。一覧の現行前優先から変更し、履歴等の一般補正は変えない。
- ★の自動移動と最後の一枚の一覧復帰ではrating Undoを残し、Ctrl+Zで元画像へ戻す。
- R2指示: リングは所属移動を終端まで保留する案を先に検討し、既存操作を制限せず組合せを減らせるなら優先する。集合ownerの一決定と§1.370 cursor ownerの適用を分ける。まれな失敗の既定方針を維持する。

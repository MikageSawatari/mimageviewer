# 評価変更で読書ページが絞り込みから外れる場合 (§1.371)

2026-10-11 / Stage 1 **R3 ACCEPT** (`57b7e320b`、利用者提示の独立レビュー)。再照合対象は `94bc236d1` (R2)、R1 の調査開始点は `ed0be21fa`。
Stage 2 は同 ACCEPT と、2026-10-11 の編集範囲限定の利用者決定 (§8) を基に実装済み。独立ソースレビューは限定差分と後続の検索・遅延open修正まで **ACCEPT**。最終自動gateの結果は `target/K-1371-impl-msg.txt` へ記録する。実機再現・受入は未実施。観測は [バックログ §1.371](next-release-backlog.md) の利用者報告、以下の設計はソースと独立再レビューの照合結果である。
R2の前回5件とR3のring独立検索更新の指摘は解消。F12公開境界の構造合意は [detached plan §11](detached-rework-plan.md#11-リワーク外からの変更記録) に記録済み。

## 1. 契約と現コードの差分

- 利用者決定 B: 評価変更で現在ページが表示条件から外れたら、読む順の次の残存ページ、なければ前へ移る。残存ページがゼロなら**現在のページ一覧**へ戻る。親フォルダへの自動退出ではない。評価トーストを残し、Ctrl+Z は評価を戻して元画像へ戻る。フィルターを解除しない。
- 不変条件: **表示集合を採用した境界**で、filter に従う reader の `fullscreen_idx` はその context の採用済み読書対象集合に属するか `None`。リング中は**当該previewの評価だけで離脱する行**を共有集合へ統合して終了まで残す (§5)。集合全体は固定せず、新規hit・件数・順序の更新を通常どおり採用する。fullscreenだけに別の例外slot/ページ列を持たず、見開きの相方、連結unit、strip/seek/番号も同じ採用済み集合を使う。`selected` は保存対象の代用品にしない。
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
| 「本を優先」のカスタム評価キー (§1.417) | 既定キーなしの `RatingBookPreferred1..5/Clear/StepUp/StepDown`。一覧は選択 / チェック項目 (本のページ一覧も選択ページ)。画像閲覧・native 動画 / 音声では入力を受けた mounted context の採用済み `ProjectionRecipe::Book` と物理状態から本の対象を値で確定し、既存コンテナ保存経路へ渡す。非本・抽出結果の閲覧は既存の表示アイテム経路へ渡す。本キーは親一覧の本セルと同じで、子ページの★は変えない。照合不能なら書かずに通知する。既存の公開・所属調停・Undo を共有し、評価後の index で対象を再解決しない。 |
| 情報パネル・マウスの★、音楽パネル | `ui_metadata_panel.rs:2718` の `set_rating` / `set_current_folder_rating`、`ui_music_panels.rs:1232` の `set_rating`。表示ページの idx を保存対象として捕捉する。キーボードだけ直してこの入口を残さない。 |
| メイン toolbar/右クリックの評価メニュー | `ui_main.rs:11227` の selection/container setter。同じ共通近傍規則。画像全画面 HUD の独立した★メニューは見当たらず、★操作は情報パネル・キー・リング。 |
| native 動画キー / HUD の★ | `app/native_video.rs:12556` の共通 rating helper、`:6751` の `NativeVideoOutputEvent::SetRating` → `set_rating`。静止画 strip とは consumer が違うが、表示集合から外れる anchor の問題は同型。動画/音声を勝手に非対象にせず、既存 media open/close/presenter owner を使う回帰対象。 |
| マウス/パッドのリング設定 picker | `app/gamepad_input.rs:4071` の固定 key/owner に対する live preview は DB 保存済み。値のpreviewを維持し、そのpreviewによる離脱だけをpicker終端まで保留する (§5)。独立検索更新とratingによるentrantを保留しない。`commit_live_picker_undo` (`:4878`) のbefore/最終成功値による一件Undoを維持する。 |
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
2. SQLite成功 → session ledger → contextのcache/集合更新 → visible・Details・reader order採用を一回行う。setter/container/fullscreen/hydration末尾の重複rebuildを取り除く。ringでは§5の離脱保留policyを含む集合を公開し、終端でその保留分だけを解除する。既存seen generationは**そのpolicyでの表示投影の公開完了**で進め、getterのcache overlayだけでは進めない。終端のpolicy変更も同じ採用出口へ通すため、rating世代が同じでもseenの一致だけで終端処理を省略しない。
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

- **2026-10-11実機確認後の利用者決定**: gridで評価により隠れた項目は、Undoで現在一覧に再び表示されるなら元項目へcursorを戻す。Redoもafter値で対象が表示される場合は対称に適用する。集合ownerがこの行先を一度決定し、§1.370の`move_grid_cursor`に渡すためShift+click起点も追随する。対象が非表示のままなら通常の集合調停だけを行い、復帰のために条件を解除しない。Grid originからreaderを開かない。
- originは同じ履歴owner内の`Grid(key) / Viewer { key, presentation } / ValueOnly`へ整理する。評価後にclick/key/pad等の明示選択を受理したら、同owner/listingのUndo/Redo履歴内のGrid originをValueOnlyに変更し、既存のlive RestoreOriginもReconcileへ取消す。端no-op/空一覧も含む。別context/listingのoriginと値のUndo/Redo自体は保持する。新しいApp-global navigation世代やpending復帰状態は増やさない。Undo/Redoによるrequest置換はpending取消だけとし、entry自身のGrid復帰権限を消さない。自動移動/identity reindexも消さない。
- 追加受入: thumbnail/Details（Name降順も含む）でF2→次項目→Ctrl+Z→元項目・Shift起点→Redoを固定する。明示click/key/padがUndo/Redo前に入るケース、端no-op、復帰Ready前の操作、非表示のままのUndo/Redo、Redoで再表示、slot再配置後の一度だけの復帰を検査する。履歴のGrid権限失効は同owner/listingの両stackだけとし、兄弟・別一覧・Viewer origin・before/after値・stack順は保つ。組合せを減らすため、履歴owner自身のtargetをValueOnlyへ失効させ、App-global bool/追加世代/別の取消台帳は採用しない。
- `UndoEntry::Rating` に一つのtyped origin (`Grid / Viewer { context_id, listing_identity, page_key, presentation }`、仮称) を持たせる。RatingChangeのstable keyと同じ操作を所有し、別の「最後のratingページ」やpending reopenを足さない。grid/Remote/container-only変更から無条件にreaderを開かない。
- 自動所属移動・最後の一枚の一覧復帰は既存open/closeへ内部causeとして渡し、**ratingのUndo/Redoだけ保持**する。clearの必要なidx-only adjustment/layer Undoや保留drag/tagを丸ごと `take` して戻さない。通常の手動open/close・scope変更・context破棄のclear方針は維持し、同時に以下のrequest intentも終了する。
- Ctrl+ZはDB transaction成功後、同じ集合採用へ `RestoreOrigin` を渡す。有効な元keyが同context/listingで対象へ戻れば通常近傍より優先して一回着地する。最後の一枚の自動退出後も、同一覧のUndoなら元presentationで元画像を開く。非評価filterを解除しない。元keyが現在条件に合わない/消失している場合は値を書き戻すだけとし、復帰不可を通知する。旧窓/旧scopeを再作成しない。
- 元slotが消えた検索/Rating等では、復帰は**既存membership request ownerの現在desired wishが持つtyped adoption intent**とする。`Reconcile / RestoreOrigin(origin)` (仮称) のいずれか一つであり、App-globalの別pending stateを併設しない。completionが運ぶのはsetと来歴。復帰権限は返ってきたwishの古いコピーではなく、**採用時のlive owner**から得る。

| 通常入力/終端 | 復帰intentとmembership requestの扱い |
| --- | --- |
| Undo成功 | 現在intentをそのUndoのRestoreOriginへ置換し、最新write generationのdesired requestへ結び付ける。同期復帰できたら消費し、後続workerは通常Reconcile。 |
| Undo待ち中の別Undo | 後のUndoのorigin/世代へ置換する。古いReadyは採用検証で失効し、二つの復帰を予約しない。 |
| 同じ一覧内の手動next/prev、seek、page指定、grid click/open、矢印・Home/End・Pageキー・パッドのgrid選択移動 | **操作受理境界で**live ownerのRestoreOriginをReconcileへ置換する。ページ送り・grid選択の境界no-opと空一覧でも明示移動意思を優先する。ring/gesture/mouseのGridMoveFirst/Lastも同じ受理境界へ接続する。必要なmembership refresh自体は残し、完了時は最新anchorで調停する。入力制限で受理されない操作は取消さない。自動移動も使う`move_grid_cursor` / `apply_grid_cursor_move`には取消を置かず、input_seq全体で無関係な入力まで取消しない。 |
| Esc/通常reader close、別scope/open/history、context retire、request cancel | close/変更前にintentを終了する。membershipだけが後から採用されてもreaderを再openしない。Undoスタックclearだけに取消を依存させない。F12の内部mount/元contextへ戻るswapは手動navigationと扱わない。 |
| Redo受理 (Ctrl+Y / Ctrl+Shift+Z) | 先行requestのRestoreOriginを取消す。entryの未取消Grid originならafter値の最新requestへ対称なRestoreOriginを渡し、表示対象へ戻ればその項目を優先する。Viewer originは従来どおりReconcileで現anchorから次/前/一覧を一度決定する。保存失敗時も古いUndo復帰intentを再生せず、既存のログ/通知・Redo entry保持に従う。 |
| latestへの通常reprepare | live ownerに残るintentだけを移す。cancel済みintentをworker結果から復活させない。 |
| 有効なsetの採用 | originが対象へ戻り着地できる採用でintentを `take` して**着地前に一度だけ消費**する。既存media landing/close終端へ渡したらrequestから消す。最新の確定結果で復帰不可なら通知して終了。stale結果はconsumeしない。 |

同じ一覧のmanual navigation後のmembership採用は、そのときの現在ページがまだ所属していればKeep。自動退出後はreaderを開く権限をRestoreOriginだけに与え、Reconcileはgrid cursorを補正するだけ。別窓でUndoした場合のDB書戻しとoriginへの配送は分離し、安定したdispatcherから生存するorigin contextだけへ配送する。無関係な前面窓へ着地しない。

## 5. リングは評価による離脱だけを終端へ保留する (R3・レビューP2-4/新規P2)

**DB/星のlive previewを続け、そのpreviewだけで離脱する既存行だけをpicker終端まで残す。Ready全体・検索streaming・件数更新は保留しない。** 現コードでは `RatedBatch` がhitを累積し (`global_search_ui.rs:2620`)、同じ `Ready` がitems・rating値・drilled countsを採用する (`:2724`)。R2の「picker所有contextの集合置換は禁止」は無関係な新規hitも遅らせるため撤回する。入力sourceを二つのqueueに分けず、各最新setを同じ集合ownerで統合する。

### 5.1 保留の表現と最新集合への統合

- ownerは既存 `RingPickerState` のowner/anchor/original-rating-records。原値・成功write generationと**rating起因の離脱を保護する既存rowのstable identity/依存key**を、一つのtypedなpreview採用recipeとして同操作内にまとめる (仮称 `RatingPreviewLeaves`)。App-global bool、pending move、旧items全文のコピー、fullscreen専用列を追加しない。recipeはDBの旧値を最新factsと偽るcacheではなく、表示所属にだけ適用する明示的な操作policyである。
- recipeの対象 `P` は、**このringの成功した評価previewの前に既に採用済みで、その対象keyの評価判定に依存するrow**だけ。Grid複数評価は該当row群、readerは元page、container評価/集約検索は該当container rowの依存keyを使う。rowのsourceは現在contextのcanonical items/rows/hit sourceに保持し、snapshotではstable descriptor/論理page keyを参照する。新規hitを後からPへ無条件追加しない。評価未操作・失敗・bound/no-opならPは空で、通常採用と全く同じになる。
- 独立更新を含む最新candidate集合を `U` とする。query/view/sourceの範囲、file-type visibility、tag・facet等の**評価以外の採否は最新**。その候補の実rating factsを `R`、preview対象keyだけをその操作のbefore値へ置換した評価判定用factsを `R_before` とする。sourceの同一性・独立条件は両方で同じUを使う。保留行と公開集合は採用中の一時値として次の一式で求める。

  ```text
  S = ordinary_membership(U, R)                  // 最新factsによる通常集合
  H = { row ∈ U ∩ P | row ∉ S
        and rating_predicate(row, R_before) }   // このpreviewだけによる離脱
  M = S ∪ H                                     // context唯一の公開集合
  ```

- `rating_predicate` はglobal filterだけでなくRating固定★/Smart mask、集約rowのrating依存判定も含む。non-rating条件はUで既に検証済み。新規hitや通常entrantはSから直ちに入り、未変更keyの評価変化や本当の非評価離脱をHへ紛れ込ませない。集約rowも新しいhitを含むUで依存判定を再計算し、opening時の件数を保存して使わない。対象keyに別producerの後続writeが来た場合は、成功writeの来歴でそのringのpreviewだけの差分かを判定し、外部writeの離脱をringの保留として救済しない。
- query前のrating絞り込みでPのrowが落ちるRating等は、**同じprepareのcandidate段階**でPのcanonical descriptor/keyを候補へ含め、最新のsource/非評価条件を検証してから上式を適用する。検索は最新 `all_hits` のrating filter前、Smartは保持slotsのmask前、物理一覧はinstalled itemsのvisible filter前で統合する。post-filterの古いitemsを盲目的にappendしない。source削除・非評価filter離脱・scope変更ならUに入れず、必要な既存picker終端を採用前に通す。消失したsourceや古いquery結果を復活させない。
- Mを**一回**canonical sort/materializeへ渡し、items/meta/page identity/cache/visible/Details/reader orderを整合した一つのprojectionとして採用する。Hも同じ一覧とreaderに属するため `fullscreen_seek_info` の集合外例外は不要。現在pageはstable keyで新slotへpaired remapしてKeepし、streaming追加だけでpickerをstaleにしない。grid cursorとShift起点も§1.370の同一identity remapで保つ。pageの通常open・Undo clear・rating近傍landingを新hitのたびに起こさない。

### 5.2 Ready・件数・順序の即時採用

- `RatedBatch`/Readyはring未操作時もpreview後も既存のstreaming cadenceで処理し、ring close待ちの追加gateを設けない。requestには最新factsの世代に加えて現在のpreview採用recipeを渡す。§3.3のDB/session/surface/source/request検査と**live recipeの一致**を採用前に検査する。開始前に作られたReadyやpreview前のReadyが無効なら最新wishで統合し直すが、「ringが開いている」をreject理由にしない。兄弟contextは通常policyのまま。
- Readyの同一source batchまでのitems・countsを共に採用する。新Batchの受理/累積hit数、通常のraw件数と★別ヒストグラムは最新U/実factsで即時更新する。filtered可視件数、containerの可視下位件数、readerページ数は**その時点のM**から毎回計算し、Hを含む公開内容と合わせる。countsの旧snapshotは保持しない。例: AをHとして保持中に新一致Cが届けば、A/B/Cの表示と可視数3をそのReadyで採用し、rawの★値/件数も新factsへ更新。終端でHだけが外れればB/C・可視数2になる。
- GridColumns/ThumbAspect、画像の見開き/flow/方向/fit、ページ補正、動画volume/speed等のlive previewを維持する。GridSortOrderも**最新Uから作ったM**を既存sort/materialize ownerで並べる通常prepareへ通す。rating-sortの値/時刻は最新factsを使い、Hだから旧ratingでソートしない。survivor相対順/entrant appendは既存のrequest目的に従う。R2の「固定memberのみのsort-only prepare」という追加分岐は不要になり、撤回する。必要なmetadataは既存workerで読み、ソートや独立hit採用を終了まで延期しない。
- 採用後のseen generationはMまで公開したことを意味し、pending離脱の完了ではない。保留はrecipeだけが所有する。後続の各source更新でHを再計算してMへ統合するため、同じrating世代でも新Batch/件数/順序を採用できる。古いReady/旧Hを蓄積するqueueや並行した全画面集合は持たない。

### 5.3 行の所有と一回の終端決定
- 各行の所有範囲を変えない。ItemRatingはopening時の固定key群、ContainerRatingはopening時のcontainer target、PostFilter等の個別補正は元page key、SpreadMode/ReadingFlow/Direction/Fitはそのviewer/book設定owner、動画行はそのownerのplayer、grid行は既存grid_sort_target/settings ownerへ適用する。全行を「元idx」に統一しない。元page identityはまだ同readerで表示されているので、現在idxの使用が他ページへの誤配送にならない。itemsのorder変更だけならkeyでidxを再解決する。
- **終端の順**: opening ownerを安定境界で配送/mount → 最終previewの既存確定処理 → 成功済みledgerからrating Undoを一度記録 → commit-only/非rating行の既存確定・native HUD後始末 → recipeを通常policyへ置換 → 最新採用済みsourceと成功rating値からHなしの集合Sを採用し一度調停 → toastを着地面へ出す。close時はopening時の集合へ戻さず、途中で採用した新hit・件数・sortを残す。次優先の基準は終端直前の現在の読書順で、近傍決定/landing/cursor applyを一回だけ行う。`commit_ring_picker` は現在take後にfinalizeする (`:4751`) ため、Option不在だけで保持終了と推測しない。takeしたrecipeをローカルのtyped終端処理として確定/採用まで渡す。
- 終端でHを外すことは、既に公開したsource/factsを在メモリで通常predicateへ通す処理であり、worker待ち/DB再読込/reopen予約を足さない。未採用の新Batch等はその後の通常更新として取り込む。終端前のrecipeを持つ遅いReadyはrecipe一致検査で失効し、通常policyの最新wishへ更新する。write世代が同じでも旧Hを再挿入せず、元pageへ戻ったり二度自動移動したりしない。後続に別の本当のmembership変更がなければ現在pageはKeepとなる。
- 通常commit、マウスpicker終端、パッド無効化/切断、sidecar modal、stale破棄、手動reader close/別場所open、owner retireをこの終端へ接続する。強制終端は既存 `published_live_picker_rating_changes` と `finish_gamepad_input_for_sidecar_restore` (`:2535`) の成功済みpreviewのみ記録する規則を使い、commit-only行を勝手に実行せず、新しい保存/rollbackを足さない。現在 `close_stale_ring_picker` (`:3733`) はtake/dropだけなので、成功済みratingのUndoと保持集合をここで取りこぼさないよう変更が必要。元owner消失ならstable keyの成功記録のみ終端し、死んだviewerへ適用しない。
- 通常の別contextへの一時mount・フォーカス移動だけでpickerを終わらせない。pickerはopening ownerに残り、そのownerへ入力を配送する。元scope/readerを実際に変更する操作は**その操作前**にpickerを既存の終端方針で終了し、操作は続行する。明示rating-filter変更も終了→設定採用→最新集合の一決定とし、入力を拒否しない。手動navigation/closeが終端理由なら、その明示intentを同じ採用へ渡して自動の中間landingを省き、ユーザーの移動/closeを一回適用する。自動的なRestoreOriginは§4で取消す。
- 最後の一枚をpreviewで除外しても、リングを操作中はそのページ・全行previewを維持する。通常picker closeで初めて現在gridへ戻り、rating Undoは残る。除外→再び一致のpreviewならclose時Keep。複数previewはbefore→**最終成功値**の一件Undoとし、DB失敗時は成功した値までを用いる。成功していない選択値で集合/Undoを作らない。

比較: 即時移動後のpicker再targetは消失slot/行owner/staleの組合せを増やすため不採用。移動をpicker強制終了にする案は次行/再評価を制限するため不採用。R2のReady全体/採用済みmember固定は独立検索更新を遅らせるため撤回。R3では一つのpreview離脱recipeを通常candidate/filter/sortへ渡し、setの二正本やsort-only経路を減らす。操作・streamingを禁止せず既存preview/commitを維持する。実装で操作制限が必要と判明したら、実装せず費用と選択肢を利用者へ返す。

## 6. 成功ログ・toast・失敗方針

- `[RATING]` save成功ログは**SQLite成功→ledger公開の境界**に集約する。source、origin/projected context ID、write/items generation、target idx (未投影はnone)、stable rating key、before/after、settings/effective filter、採用済みreader所属と新値のfilter判定を記録する。ログのための同期I/Oは追加しない。未知の旧値は `before=unknown` と情報源を明記し、未評価0と偽らない。通常fullscreenのbefore/originは必ず捕捉する。
- 後続projectionログは同write generationで相関し、採用世代・旧/新所属・`Keep/Move/Grid`・着地点・request intentを記録する。保存時にまだ移動を決めていなければ `projection=picker-held / preparing / unmounted` と記録し、成功saveで行先を捏造しない。picker終端/mountを二度目のDB保存としてログしない。stale rejectはprepared/currentのfacts・session世代と理由を記録する。
- rating toastは既存★文字列のまま、採用/自動移動後の表示面へ配送する。移動ならorigin viewer、ゼロならMainWindow。リングpreview中の表示は保ち、終端で成功した最終評価のtoastを着地面へ表示する。通常closeのtoast cleanupで消さない。DB失敗は成功世代・成功toast・Undo・移動を公開しない。XMP後続失敗は既存通知とし、DB評価を巻き戻さない。
- **まれな失敗は割り切り (ログ + 通知)**。DB retry、completion待ち、typed失敗分類、多段回復、DB削除/再作成は足さない。通常の連続入力・Undo/Redo・worker順序・context移動は上記の所有と採用で構造的に扱う。

## 7. 不採用案・必須テスト・引き継ぎ

CLAUDE.md「設計の簡素化」に従い、picker中のrating由来自動移動/vanished slotだけを終端集約でなくす。独立検索更新を保留するgateと固定memberのsort-only分岐は撤回し、一つの離脱recipeを通常採用へ統合する。復帰権限は既存requestの一intentにし、最新結果へoverlayして欠落slotを復活させる回復機構を作らない。swapのpending途中でナビゲーションを始める組合せは安定境界への移動で除く。

- 独立したfullscreen列、無条件filter解除/suppression、seek Noneの全items fallback、repaint/遅延パッチ: 集合とreaderの二正本を残すため不採用。
- pending bool/idx、別復帰queue、多段rollback: 既存集合/request/picker ownerが同じ状態を既に持つため不採用。
- rating保存のモーダル化/キー連打制限: 同期保存の成功境界と集合採用へまとめれば不要で、既存操作を不便にするため不採用。DB非同期化は本項に混ぜない。
- 設定変更で常にreaderをclose/reload: 次へ進む決定B、zoom/連結/窓identityを壊すため不採用。ゼロだけ通常teardown、実際のitems交換だけ既存再入場を使う。
- Undoスタック全体の退避/復元: idx-only編集や他contextを復活させるため不採用。rating entryのoriginと内部causeに限定する。

実装時の必須テスト (Stage 1 では未実行、Stage 2 の実行結果は `target/K-1371-impl-msg.txt` へ記録):

1. 純粋近傍: 中/先頭/末尾、複数同時離脱、ゼロ、Details降順/非単調idx、RTL/LTR、新entrantのみ残存、gridとstill/media domainの差。次→前、残存anchorはKeep。一般補正を先行させず、cursor apply/reader landing各一回をassertする。
2. rating handler: F1–F6/custom/step、selected≠fullscreen、全表示/未評価除外filter、情報パネル/マウス★/native HUD。stable DB key/cache/generation/所属/行先、publication/成功ログ/toast、bound/no-opを検査。Image/ZIP/PDF/EPUB/入れ子・変換書庫とShift+F containerを分離し、最後のstill+container-only残存で現在gridへ戻る。
3. Undo/Redo: 自動next/prev後、最後の一枚grid後、二操作、並び替え/slot変更、非評価filter、別窓Undo。元画像/元presentation、rating entry/redo保持、filter不変、idx-only Undoが復活しないこと。Rating再採用では既存の編集終了と非rating Undo破棄を確認し、同identityの非rating Undo保持/remapは受入条件にしない。**Undo→同じ一覧でmanual next/prev/seek→Ready、Undo→Esc/close→Ready、Undo→Redo→古いReady→最新Ready**を固定し、manual結果を上書き/reader再openしない。最後のreader退出→元slot削除→Undo→grid矢印/Home/End/Page/パッド/リング先頭末尾→Readyも、thumbnail/Detailsの移動・端no-op・空一覧で固定する。Shift矢印とcustom keyも受理時だけ取消し、text focus/未割当修飾子/refreshによる入力拒否と共通cursor ownerからの自動移動は復帰権限を残す。別Undoで置換、一度consume後の別workerでも再復帰しない。
4. 評価世代: **A除外→AなしReady保留→Undo成功/A復帰→古いReady解放**をRatingと検索(通常/評価ソート)で実行。set/items/fs/cursorが一切古く戻らないこと。prepare中の評価write、DB epochだけ変わるproducer、session更新と古いRatedBatch後着、filter/query/source/context/sequence変更、stamp有効でないcached factsに新stampを付けないこと。Smart現行write-generation検査も維持する。
5. Ring: preview除外→他の全行preview→再評価一致→closeはKeep、最後の一枚preview除外中もstrip/seek/番号・元slotが存在し、新entrantがなければcloseでGrid。Rating/検索/Smartとin-flight/streaming、**評価preview後のGridSortOrderのlive変更**、Details sort、commit/マウス終端/切断/無効化/modal/stale/manual close/scope変更を検証。成功済み分一件Undo、元key/owner、page個別行/全体行/playerの保存先、重複writeなし、picker-ended後の移動一回。source/非評価離脱や別producerの評価を誤ってHへ含めない。owner Aのring中にBへmountしてもAのrecipe・Bの通常公開・各worker tokenが独立する。
6. ContextTable: outgoing未公開rating→mount、incoming未公開rating→mount、callback内rating→original復帰。traceで **outgoing publish→plan→swap→finish→settings projection→incoming publish** をassertし、projected-ID assertion/panic・swap内landing・再mountなし。build commit/abort、fork/promote/retire、catch-unwind後のoriginal復帰も同じ契約。read-only primitive単体テストは値交換だけで、save/移動を期待しない。
7. **§1.370統合**: auto next/prev、Empty→grid(残存container/完全空)、Undo→元画像、items並び替えでidentity存続/消失の各後に、サムネイルとDetailsのShift+clickを実行。起点/範囲/checked/scroll/generation、旧有効anchor≠selected、同じ数値idxに別identity、saved Smart root/履歴、main↔F12とA/B context分離を検査。paired remapだけの場合は旧起点identityを保持し、Moveの場合は新起点へ更新する。
8. filter変更/設定採用、Ctrl+G/Rating/Smart後続採用、open/switch/close/cancel/stale、rating sort snapshot survivor順。CollectionRoot/DetachedPhysical/DirectMedia/★固定snapshotでmembership/orderが不変なら移動なし。F12同key・異idx/active/parked、getter先行同期、zoom/window binding、兄弟cache/queue/generation/tokenの保持を確認する。
9. Remote acquire→rating→releaseのreader閉鎖/PC・Remote位置分離、XMP stale hydration/import、DB不在/Undo失敗は既存テストを再利用。失敗組合せ用回復機構を作らない。
10. 単ページ/見開き・表紙/分割/縦横連結、native動画/音声: 新集合とHUD・seek・strip・unit、空grid/toast、AI/補正/ロード中のpresentationを検証。可能なheadless snapshotを更新。実機確認は別の明示承認済み検証枠へ残す。
11. **評価未操作のring中の新規hit**: A(★3)を表示、ring openのままB(★3)/C(★0)のRatedBatch→Readyを解放する。★3 filterならraw3/可視2・最新★別件数とA/Bのitems/strip/page numberを**close前に**採用する。P/Hは空、現在Aとpicker/選択/Shift起点を同じidentityで維持。Rating write/Undo/landingなし。closeでも新hit/件数を失わずKeep。検索Flat/Drilled、Details非単調順と集約countsに適用する。
12. **評価preview後の新規hit**: 初期A/B(★3)、ringでAを★0へ保存してH=A、そのままC(★3)/D(★0)のBatch→Readyを採用。close前にM=A/B/C・raw4/可視3、実factsの★0=2/★3=2と新countsを採用し、Aの星表示は0、Aとpicker/全行previewは継続する。closeでHだけを外しB/C・可視2へ一回着地し、UndoはAへ戻る。初期Aだけ→preview除外→新一致Bの場合もcloseはBへMoveとなりGridへ戻らない。preview→新hit→ソート→新hit、close後に旧recipeのReady解放でもHが復活せず二度移動しない。追加が重なってもkey重複やcounts二重加算なし。
13. **編集の限定範囲**: Rating再採用は出荷済みの編集終了境界を通し、非rating Undoを落としてrating Undoだけを保持する。Searchは既存のclear/cancel方針を維持する。reader同identity、selected/Shift起点のpaired remapとこの終了方針を確認し、loaded/global editor・idx-only Undoの保持/remapや一般的なcross-context編集の正しさを本項の修正・検証済みと主張しない。下記の出荷済みSearch streaming問題は別件とする。
14. **遅延openの操作意図**: IndexOnlyと通常hydrationでrating自動open/Undo復帰のcauseとrating Redoを保持する。旧readerあり/なしの双方で、準備完了前のfilter更新のKeep/Move/Emptyを論理的な遅延targetへ一度適用する。受理されたmanual移動/close・Redo後に完了しても取消済み画像を再openせず、worker・兄弟context・Manual要求の既存動作は維持する。復元中の既存modal入力制限を変更せず、modalで受理されないEscを新たに通すことはしない。

検証ownerは実装担当。焦点lib tests・core/portable check・fmt、UI文字列変更ならglyph、snapshot、共有変更の完了時は full lib と `scripts/test-full.ps1`。実装後は `scripts/build-dev.ps1` で確認binaryを渡す。agent は mIV を起動しない。

Stage 2の候補所有ファイル: `app.rs` (save/publication/rebuild/open/close)、`viewer_context_registry.rs` (安定境界)、`ui_fullscreen.rs` (landing/入力後のanchor再解決)、`undo_stack.rs` / `undo_ops.rs` (origin・intent取消・rating限定保持)、`ring_shortcut.rs` / `app/gamepad_input.rs`、各rating UI、検索/Rating/Smartのprepare/採用とtests。§1.370との共通cursor ownerを先に統合/照合し、別cursor helperを追加しない。実装と同時にdisplay pipeline/keymap/rating snapshot order/関連ownership docsを更新する。native placement/focus、Remote protocolは変更不要。

## 8. R3の指摘対応・構造合意の状態

| 独立レビュー | 対応 (前回5件はR2で解消、最終行はR3案) |
| --- | --- |
| P1-1 swap途中のpublication | §3.1: outgoing plan前/incoming finish+settings後。primitiveは交換のみ、navigation再入禁止。必須test 6。 |
| P1-2 rating世代の採用検査がない | §1の前提訂正、§3.3:実factsのDB stamp/session世代を採用前に検証。欠落slotのoverlay修復を廃止。必須test 4。 |
| P2-3 RestoreOriginの失効不足 | §4:live request intentの取消/置換/一度consume、manual/close/Redoを共通受理境界へ。必須test 3。 |
| P2-4 ring stale/対象消失/行owner | §5:rating由来の離脱だけを終端まで保留、順序/全行previewを継続、行ごとの現owner維持。即時移動/強制終了案を撤回。必須test 5。 |
| P2-5 二段補正/Shift anchor | §3.2:集合ownerの唯一決定を§1.370 ownerが一回適用、再配置はpaired remap。必須test 1/7。 |
| R2再レビューの新規P2 独立検索更新まで保留 | §5.1–5.2:最新通常集合Sへpreview由来の離脱Hだけを統合しMを即時採用。Ready・新hit・countsは止めず、sort-only分岐も撤回。必須test 11/12。R3 `57b7e320b` の独立レビューで ACCEPT。 |

上表の前回5件は `94bc236d1` の独立再レビューで解消と判定された。F12の安定context境界の修正についても「症状パッチではなく構造修正として同意」と明示された。対象はswapからのrating publication移設、安定したownerの集合採用と通常landing/teardownへの接続、cursorのpaired適用。swap再入と兄弟contextへの誤配送を作成境界で防ぎ、detached述語/viewport/geometry/配置/focusのヒューリスティックを足さない。この**限定した構造合意・scope**を利用者指示に従って [detached plan §11](detached-rework-plan.md#11-リワーク外からの変更記録) に記録した。その後 R3 `57b7e320b` 全体が独立レビューで ACCEPT され、利用者が実装を指示した。設計 ACCEPT は実装検証・実機受入を意味しない。

### Stage 2 の接続箇所

- 集合の唯一決定は `src/app/rating_membership_ops.rs::adopt_rating_membership`。同 identity の再配置は selected/Shift起点をそれぞれ key で remap し、移動・Empty は `apply_rating_membership_cursor` へ渡す。
- §1.370 の実装・最新追補handoff (`C:/home/mimageviewer-nexteffetune`、読取りのみ、HEAD `4330c9480` と未コミット追補) と照合した。master統合 `74651f494` 後、同 helper を既存 `move_grid_cursor(target)` 呼出しへ接続した。続く `scroll_to_selected = target.is_some()` はこのcallerの既存方針として残す。近傍決定や二度目の補正を cursor owner に追加せず、Keep/reindexではselectedとShift起点の独立したidentity remapを維持する。thumbnail/Detailsの評価移動・最後のページからの一覧復帰・実Ctrl+Z復帰後のShift+クリックをhandlerで検証する。
- 明示filter採用は非表示だった entrant を含む同じ canonical/Details comparator の順で判断する。rating publication/picker終端は変更前の表示順を基準にし、旧 survivor が無いときだけ full basis で entrant の位置を求める。
- Rating/Search の slot 交換はreaderの既存 snapshot exact identity transferに接続し、同identityの読書位置とselected/Shift起点のpaired remapを保つ。編集の扱いは下記の利用者決定へ限定し、loaded/global editorやidx-only Undoを新たに保持/remapしない。
- Searchの同一覧採用は、items・page-edit投影の交換→readerのexact identity transfer完了→visible集合の再計算→集合ownerの一度だけの行先適用の順とする。新しい非表示containerが旧readerの数値slotへ入っても、その行をreaderとして非評価離脱判定しない。交換途中でvisible再計算を先行させず、Drilled/Detailsの新hit・sort・ring終端の固定順序テストで確認する。
- ページidentity/sidecar準備でopenが遅延する場合も、既存 `DeferredFullscreen` 要求自身が `FsMetadataBoundary` を保持し、IndexOnly・通常hydrationの両再開経路へ渡す。rating自動open/Undo復帰を通常Manualへ変換せず、rating限定履歴の契約を終端まで維持する。所有中のRatingMembership要求のtargetを集合ownerの論理的な着地点として扱い、旧readerが存在する場合もそのtargetでKeep/Move/Emptyを一度判断する。Keepは継続、Moveは既存要求の置換、Empty・手動移動/close・Redoは当該要求の消費/取消とし、workerや兄弟context、Manual要求を巻き込まない。再開は決定済みの継続を実行し、第二の近傍決定やfallbackを足さない。通常openの既存既定値はManualのままとする。
- 通常hydrationのownerが当該rating要求を**受理する境界**で、同ownerの旧AutoFullscreenFolder/RequiredFullscreenとZIP/PDFの旧reopen（WarmPdfの後続検証も含む）だけを置き換える。終端時だけの抑止にせず、その後rating要求が取消されても古いopenを復活させない。先行する通常openでrating履歴を消したり二度着地したりせず、hydration・サムネイル準備・ZIP/PDFの保守処理は維持する。DirectMedia/Collection/Dfsの新しい一覧遷移を一括取消しない。これらは新source採用と既存close/history終了を経るownerであり、同一覧Rating再採用のIndexOnlyとは別経路である。

### Stage 2 の編集範囲限定 — 利用者決定 (2026-10-11)

Stage 2の独立確認で、idxを持つ編集/Undoへreaderと同じexact mapを適用する前提は成立しないと判明した。
`LocalAdjustmentChange` は idx のみを記録し、`apply_local_adjustment_change_to_app` は現在の mount の idx へ書く。
`adjustment_drag_session`、local-adjust canvas/brush/shape 等の途中編集と `meta_undo` も App-global で、
`ViewerContextBundle` の交換対象ではない。`with_viewer_context` と park cleanup はこれらの編集 owner を証明しない。

利用者は、capture時のpage/context provenance追加やeditor stateのbundle移設へ拡張せず、
**出荷済みの編集終了・clear/cancel境界を再利用する限定案**を選んだ。今回の実装範囲は次のとおり。

- Rating一覧の再採用は、出荷済みの境界で進行中の編集を終了し、非rating Undoを破棄してrating Undo/Redoだけを保持する。同じ画像が残る場合も、非rating編集履歴の継続は今回の契約に加えない。
- Searchの再採用は既存のclear/cancel方針を維持する。新しいloaded/global editorの保持・転送・remap、idx-only Undoの保持・remap、editor owner型やbundle fieldは追加しない。
- readerの同identity保持と、selected/Shiftクリック起点のpaired remapは継続する。reader transferをeditorの所有証明に転用せず、兄弟contextの編集/historyへ採用先のmapを適用しない。
- 全streamingの停止、一覧更新の保留、編集の新しい入力制限は追加しない。既存の終了boundaryを超える一般editor修正は別件とし、この限定案がそれを解決したとは主張しない。

#### 出荷済みSearch streamingのidx-only誤配送 — 今回の対象外

v4.5.0のtag `1ff9d07df` でも、Search streamingのReady採用でitem slotが変わる一方、
idx-onlyの途中編集/Undoが現在のslotへ解釈される誤配送経路が存在する。根拠は同tagの
`global_search_ui.rs` Ready採用 `:2672` とreplace処理 `:1860/:1921`、`undo_ops.rs:433/:544`、
`app.rs:75911`。これは§1.371で新たに導入された問題として扱わず、今回の修正・受入範囲から分離する。
利用者が別途バックログへ記載する予定であり、本作業では一般editor ownerの修正やこの問題の解決を行わない。

限定範囲と最終検証結果・未検証の実機項目は `target/K-1371-impl-msg.txt` に記録する。
上記scope決定自体と設計 ACCEPT は自動gate成功・実機受入を表さない。Stage 2の独立ソース完了レビューは、限定編集境界、検索のpaired採用順、遅延openのcause/endpoint/取消・WarmPdf後続検証への接続までACCEPTとなった。一般editor ownerや初期一覧のcursor復元へ拡張せず、同一覧RatingのIndexOnly ownerと既存の新source復元ownerを分離している。

## 利用者決定 (2026-10-11)

- ★による移動は一覧・fullscreenとも「次を優先、無ければ前」に揃える。一覧の現行前優先から変更し、履歴等の一般補正は変えない。
- ★の自動移動と最後の一枚の一覧復帰ではrating Undoを残し、Ctrl+Zで元画像へ戻す。
- R2指示: リングは所属移動を終端まで保留する案を先に検討し、既存操作を制限せず組合せを減らせるなら優先する。集合ownerの一決定と§1.370 cursor ownerの適用を分ける。まれな失敗の既定方針を維持する。
- R3指示: 保留は当該ringの評価由来の離脱だけ。独立検索/streamingの新hit・件数を即時採用し現在pageを維持する。二つの回帰条件と終端一決定を定義し、F12の限定構造合意をdetached plan §11へ記録する。
- Stage 2の範囲決定: 編集は出荷済み終了boundaryの再利用へ限定する。Rating再採用では非rating Undoを落としてrating Undoだけを保持し、Searchは既存clear/cancelを維持する。loaded/global editorやidx-only Undoの新しい保持/remapは行わない。出荷済みSearch streamingのidx-only誤配送は利用者が別途バックログへ記載する。


## 明示選択ownerの構造化 (2026-10-11、b06d72f86再レビュー)

入力ごとに取消を追加する方針を撤回する。live contextの§1.370 owner
`App::move_grid_cursor(target, GridCursorIntent)`では分類を必須にする。
`Explicit(Move/Extend)`は、そのcontext・現在一覧のrating復帰権限を失効させてから
cursorとShift起点を適用する。`ExplicitKeep`は受理した端no-op／空一覧／選択を
維持する操作で、権限だけを失効させ、cursor・有効Shift起点を変更しない。
`Automatic`は集合ownerの一決定や既存復元を適用し、取消しない。省略できる既定値は作らない。

失効は既存の一つの取消処理に集約し、(1)同owner/listingのUndo/RedoのGrid origin、
(2)そのownerのlive requestのRestoreOrigin、(3)現在のring pickerが同owner/listingで
保持するGrid originを扱う。(1)(3)はValueOnlyへ変え、保存済み評価値、Undo/Redo順序、
ring preview recipe・対象keyを保持する。別context/listing・Viewer履歴originは変更しない。
Viewer履歴の明示Undoで元画像へ戻る既存仕様と、すでに発行した復帰requestの取消を区別する。
ringを閉じて初めて履歴へ積む場合にも、先に失効したGrid復帰権限を復活させない。
Undo/Redo自身はrequestの取消／置換を行い、履歴のGrid復帰権限を手動移動として失効させない。

クリックのpure policyはローカルのselected/anchorコピーで移動先・範囲起点を計算する。
UIのlive fieldをpolicyへ渡さず、結果を必ず同じownerへExplicitとして渡す。
checked、scroll、last-selected、dialog/IME/refreshの入力許可条件は既存callerが所有する。
右ドラッグ開始の選択は設定ONかつ有効mode・対象ありで受理した場合だけExplicitであり、
中央でドラッグを取消しても選択の明示権限は取り戻さない。mode無効／設定OFFは取消しない。

### 全callerの分類

下表はlive owner、pure cursor transition、および選択helperの全production callerを分類する。
テストfixtureの初期選択はAutomatic。identity remapとcontextの値交換はnavigationではなく、
selected／Shift起点を対で転送する既存経路を維持する。

| 分類 | caller／入口 | 適用 |
| --- | --- | --- |
| 明示 | app.rs `handle_keyboard`：矢印、Home/End、Page、Shift、custom key | 受理時ExplicitKeep、対象解決後Explicit Move/Extend。端・空も取消 |
| 明示 | gamepad_input `handle_gamepad_direction_for_grid`：D-pad/左stick | 受理時ExplicitKeep、実移動Explicit。拒否入力は取消しない |
| 明示 | gamepad_input `move_grid_selection_to_edge`：ring/gesture等のGridMoveFirst/Last | 空も含めExplicit |
| 明示 | ui_main `select_grid_cell`、`select_grid_secondary_cell`、`clear_grid_background_selection` | primary/touch、secondary、short-tap context menu、background。pure policyの結果をExplicit適用 |
| 明示 | ui_main `select_grid_right_drag_start` | 右ドラッグ開始時選択の受理後Explicit |
| 明示 | app.rs `clear_quick_folder_slots` | 「場所を忘れる」の先頭選択Explicit |
| 明示 | ui_main toolbar rating/facet/name/place、bookmark/history filter、local searchのclear／scope変更 | ExplicitKeep後、既存set producer／可視補正はAutomatic |
| 明示 | app.rs `execute_search`、global_search_ui `reset_global_search_for_query_change` | 検索送信／query変更の受理はExplicitKeep。後着Batch/Readyでは取消しない |
| 既存scope境界 | Tag/Favorite検索送信・source/context切替・manual reader open/close/navigation | 既存request retirement／編集clear／reader取消を維持。結果のcursor landingはAutomatic |
| 自動 | rating_membership_ops `apply_rating_membership_cursor` | rating next/previous、Empty、Undo/Redo RestoreOriginの決定一回。取消しない |
| 自動 | app.rs ZIP refresh、drive list install、playback-test completion、history hydrate、Rating prepared open/position、items install/reset、ZIP親復帰、history/resume/stack restore | `move_grid_cursor`へAutomaticを明記 |
| 自動 | app.rs fullscreen/open/navigation completion、deleted cursor replacement、bookmark-list install/return、viewer/grid sync、detached materialize/close、fullscreen close | Automatic。manual intentは受理境界で扱い、後着landingで再取消しない |
| 自動 | app.rs `select_item_after_load`、`redirect_selected_to_visible`、`ensure_selected_visible_or_first`、`poll_search` | 復元／operation output／一般可視補正／検索採用。前優先の一般補正は変更しない |
| 自動 | collection_grid source removal/prepared install；snapshot_ops restore/list return/fullscreen completion；smart_folder prepared root；startup_ops requested-file completion；subfolder_expansion prepared install | Automatic |
| 自動 | filename_stack_ui stack restore/items swap；global_search_ui prepare/restore/drill landing/query array reset；remote_ipc/ui local-viewer復帰；ui_fullscreen continuous seek/spread normalize/slideshow | Automatic |
| 自動・値転送 | viewer_context_registry context create/split/main-grid復元、detached bundle初期選択のpure transition | live復元はAutomatic、未mount bundleの初期値はpure transition。swap内では新しいpublication／navigationをしない |
| identity remap | app.rs削除reindex/anchor世代失効、global_search_ui paired remap、rating Keep paired remap、snapshot items exchange、smart_folder saved root remap/削除置換/root adopt | 移動分類の対象外。残存identityのselectedとShift起点を対で保持 |
| 存在しない入口 | type-aheadによる直接cursor選択 | 現コードには該当する独立選択handlerなし。local検索入力／送信は上記検索境界。新設時はExplicit必須 |

live selected／Shift起点のmutable borrow（self/app・空白差も検出）とselfへの直接代入が
cursor owner以外の新しい入力helperに入ると失敗するsource regressionを加える。既存の
identity remap／generation失効／非live owner／pure policyのテストfixtureだけをfile/function名で
限定例外として列挙する。型の必須引数は新callerの分類省略をコンパイルで防ぐ。Automaticの
誤分類まで機械的に証明するとは主張せず、この表とhandler回帰でレビューする。

追加回帰：サムネイル／Detailsそれぞれで、Grid Bのring評価preview→HomeでA→ring終端→
Undo/RedoでもA維持、右ドラッグ開始時A選択→中央取消→Undo/RedoでもA維持、
右ドラッグ選択→保留Ready採用でも手動選択維持を固定する。既存のkey/pad/端no-op/空一覧、
Shift起点、自動移動・streaming採用・context分離の回帰も再実行する。

入力をモーダル化／ring操作制限する案は、既存操作を減らすので採用しない。追加の取消台帳、
bool、epochや個別entryへの取消patchも採用せず、必須intentと既存originのValueOnly化で
状態の組合せを減らす。まれな失敗は従来どおりログ＋通知で割り切る。


Automaticの全production呼出function (同function内の複数landingもすべてAutomatic):

| file | function |
| --- | --- |
| app.rs | refresh_current_zip_level_preserving_position / install_drive_list / poll_play_test / hydrate_reading_history_entries / install_prepared_reading_history_entries / select_rating_view_row_for_opened_path / finish_rating_view_install / load_zip_as_folder_with_prepared_enumeration / install_zip_current_level / zip_nav_back / zip_nav_dfs_fullscreen / open_deferred_fullscreen_after_enumerate / install_loading_items_inner / resume_loading_items_after_sidecar / finish_main_list_after_hydration / remove_items_batch / install_bookmark_view_rows_with_sources / restore_bookmark_view_grid / poll_detached_physical_folder_open / reopen_fullscreen_after_folder_nav_load / sync_main_selection_from_viewer_idx / start_active_detached_collection_root_image / reconcile_closed_bookmark_detached_context / select_item_after_load / redirect_selected_to_visible / ensure_selected_visible_or_first / poll_search / restore_grid_cursor_to_fullscreen_item |
| app/collection_grid.rs | apply_collection_grid_remove_success_in_current_context / install_collection_grid_items_with_thumbnail_sources |
| app/rating_membership_ops.rs | apply_rating_membership_cursor |
| app/snapshot_ops.rs | deactivate_snapshot / snapshot_return_to_list_view / finish_required_fullscreen_load |
| app/smart_folder.rs | install_current_prepared_smart_folder |
| app/startup_ops.rs | open_loaded_file_fullscreen |
| app/subfolder_expansion.rs | install_prepared_subfolder_expansion_memory |
| app/viewer_context_registry.rs | become_mounted_independent_detached_viewer / split_current_context_preserving_main_grid |
| filename_stack_ui.rs | toggle_subfolder_stack_mode / swap_stack_view_items |
| global_search_ui.rs | restore_search_selection_after_prepare / request_search_view_build / global_search_ctrl_nav_fullscreen / reset_global_search_for_query_changeの一覧clear |
| remote_ipc/ui.rs | poll_remote_fullscreen_restore |
| ui_fullscreen.rs | seek_to_continuous_page_with_contract / adopt_shift_layout_without_navigation / normalize_spread_position / advance_slideshow / loop_slideshow_to_first |

手動sort／ring sort・列数・aspect変更は選択identityを変えず、表示順／layoutを再投影する操作であり、
復帰権限を失効させない。色filterと検索種別filterの明示変更はExplicitKeep。
`retire_main_list_requests_for_surface_switch`はquery/sourceの明示切替の共有admissionであり、
先にExplicitKeepを通す。streaming/同queryのReady採用からこのadmissionは呼ばれない。


`execute_search`はUIの明示送信入口とし、共通worker提出へ渡す前にExplicitKeepを通す。
既存queryを再適用する `reload_top_level_grid` / `reapply_local_search_after_smart_folder_prepare` /
`install_prepared_subfolder_expansion_memory` は `reexecute_search_for_installed_list` を使う。
同queryの自動再検索は復帰権限を取消さず、結果採用は既存Automatic。
明示query/scope送信と自動再検索双方のUndo回帰を含める。


右ドラッグ回帰はsecondary press直後（release前）に履歴originのValueOnly化をassertする。
pickerの開始時刻だけをテストで制御し、長押し後の中央releaseがCancelledとなり
context menuを開かないこともassertする。短い右クリックのmenu選択による別の取消で
開始境界の欠落が隠れる再現を避ける。実時間sleep／native inputは使わない。

### 自動検索バー整理の分類訂正 (2026-10-11、P3)

`render_search_bar` がPDF／EPUBのページ一覧への遷移後に残ったバーを閉じる分岐は、
入力イベントがなくても通る自動整理であり、Explicitの入口ではない。検索状態のclear、
local search workerのcancel、既存可視集合の再投影だけを行い、明示cursor操作を発行しない。
同contextのGrid Undo/Redo originと保留RestoreOriginを失効させない。
×／Esc等の利用者による検索解除は従来どおりExplicitKeepとする。
サムネイル／Details・PDF／EPUB・Undo／Redoの各組合せで、イベントなしの描画による
自動終了後も履歴origin・pending intent・cursor／Shift起点を保持する回帰を追加する。

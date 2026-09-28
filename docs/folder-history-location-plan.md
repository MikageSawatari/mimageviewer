# フォルダ履歴の現在地と表示位置 — §1.280 / §1.282 / §1.281

> 2026-09-27、stage B 実装・自動 gate 完了（経過は §9）。設計担当の決定を §8 に反映し、独立 Sol reviewer が構造案と completion review を承認済み。実アプリ smoke は利用者承認後に別途実施する。本文の参照行は `history-nav` の stage A 調査時点。

## 1. 観測、原因、不変条件

- §1.280: Rating 一覧から実フォルダを開いた後も `TopLevelGridSurface::Rating` が残る。通常 load が Folder にする対象は DriveList / ReadingHistory / Bookmarks だけである [`src/app.rs:22412`](../src/app.rs)。`folder_nav_current_target` は surface 由来の `current_top_level_restore_snapshot()` を使うため、表示中の実パスを Rating と読んで ← 時に進む履歴を失い、次の遷移では実フォルダを戻る履歴へ積めない [`src/app.rs:20234`](../src/app.rs)、[`src/app.rs:20568`](../src/app.rs)、[`src/app.rs:19939`](../src/app.rs)。変化の起点は `e9dfde0da` の typed history 化であり、単に旧 path getter へ戻すと Collection / Smart Folder を壊す。報告の症状 1 は再現、症状 2 と他の波及は未実機確認 [`next-release-backlog.md:32`](next-release-backlog.md)。
- §1.282: Collection root C の子を採用すると session の position は `PhysicalSource` に移るが、surface は Collection のままで、履歴遷移を記録しない [`src/app.rs:22510`](../src/app.rs)、[`src/app/collection_grid.rs:1060`](../src/app/collection_grid.rs)、[`src/app/top_level_grid_view.rs:375`](../src/app/top_level_grid_view.rs)。`collection_grid_restore_snapshot()` は子表示中も C の restore を返すので、A→C→B の ← は B→A と誤認する [`src/app/collection_grid.rs:842`](../src/app/collection_grid.rs)、[`src/app.rs:20288`](../src/app.rs)。現行 §16.3 / §20 は Collection-owned child を履歴に積まない設計で、BS の root 復帰だけを定義した [`collection-implementation-plan.md:734`](collection-implementation-plan.md)、[`collection-implementation-plan.md:1010`](collection-implementation-plan.md)。
- **不変条件**: 各 viewer context の可視採用済み一覧には、表示対象を表す一つの typed `DisplayedLocation` を対応させる。←/→ が有効な時、履歴の `current` は必ずその可視場所と等しい。移動元の取得、戻る/進む stack、検索・特殊ビューの戻り先 snapshot は同じ位置解決を使い、`return_to`（退出先）を現在地として読まない。scan / archive / worker の未採用要求、失敗、取消、stale completion は可視場所と履歴を変えない。表示採用時にだけ両者を同じ context 内で commit する。新しい並行 `bool` / `Option` の現在地フラグを足さない。

`DisplayedLocation` は実装上、`TopLevelGridView` の surface と position、採用済み `effective_folder()` から一か所で投影する。`FolderNavHistoryTarget` と `TopLevelGridRestore` の変換を共通化し、履歴だけが独自に surface を解釈しない。`effective_folder()` は変換書庫で cache ZIP の代わりに元書庫を返す [`src/app.rs:19279`](../src/app.rs)。`Collection` の root と子を同一 ID だけで等値比較してはいけない。子の path と root anchor も location identity に含める。history stack 上の同一 C は既存どおり最新 revision/anchor hint に置き換える [`src/app.rs:19915`](../src/app.rs)。

`FolderNavHistoryTarget::CollectionPhysical` を設ける場合、同値判定は collection ID・root entry ID/source key・可視 logical path で行い、revision hint は再入場時の下限として最新値へ更新する。root `Collection` と子 `CollectionPhysical`、同じ collection の異なる子孫は別の地点である。`collection_id()`、全 stack の Ready-catalog prune、rollback snapshot も新 variant を網羅する [`src/app.rs:289`](../src/app.rs)、[`src/ui_dialogs/collections.rs:1792`](../src/ui_dialogs/collections.rs)。Rating の子も BS provenance を持つ `RatingPhysical {visible path, stars, parent chain, Rating 退出先}` という typed variant を使う。surface は Folder でも、同じ物理 path を通常訪問した地点とは親ナビの意味が違う。path 比較だけで両 variant を dedup しない。Rating の直接 open は既存 `rating_view_nav_stack` を更新し、履歴と transient restore は同じ採用済み親 chain を snapshot 化する [`src/app.rs:25121`](../src/app.rs)、[`src/app.rs:25148`](../src/app.rs)。

検索 Ctrl+G/S/T と Snapshot は一時 surface で、現在表示中の場所として明示的に `Search` / `Snapshot` と識別する。従来仕様どおり検索中の ←/→ は無効、検索中と close 復帰は履歴を積まない [`src/app.rs:20377`](../src/app.rs)、[`docs/keymap-spec.md:338`](keymap-spec.md)。この間の history cursor は**休止**し、検索結果を元 Folder や Rating と偽らない。退出時には entry 時点の typed origin を `return_to` から採用して再開する [`src/app.rs:24176`](../src/app.rs)、[`src/app.rs:24257`](../src/app.rs)。Snapshot の範囲内 navigation と退出も同じ origin 契約を守る。これは「履歴が有効なら現在地＝表示地」の例外ではなく、一時 surface で履歴操作を許さない状態である。

## 2. 可視場所の対応と採用境界

| 表示 | 現在地と退出先 | 実装上の確認点 |
| --- | --- | --- |
| 通常 Folder、ZIP/PDF・変換書庫 | `Path(effective_folder)`。archive の内部階層は既存 `zip_nav` が親/子を所有し、外側の履歴に cache path を入れない | [`src/app.rs:19279`](../src/app.rs)、[`src/app.rs:26557`](../src/app.rs) |
| Rating 一覧 | `Rating {stars}`。そこから採用した実コンテナは Folder surface 上の `RatingPhysical` とし、`rating_view_nav_stack` を BS 専用の親 chain として保持 | [`src/app.rs:25032`](../src/app.rs)、[`src/app.rs:25114`](../src/app.rs)、[`src/app.rs:25524`](../src/app.rs) |
| Collection root / Collection-owned child | root は `Collection(restore)`、子は `CollectionPhysical {collection ID, root entry anchor, root source, visible path, revision hint}`。BS 用 root restore と子の現在地を混同しない | [`src/app/collection_grid.rs:842`](../src/app/collection_grid.rs)、[`src/app/collection_grid.rs:861`](../src/app/collection_grid.rs)、[`src/app/top_level_grid_view.rs:375`](../src/app/top_level_grid_view.rs) |
| Smart Folder root / scoped child / container | `SmartFolderViewState` の position が現在地。`return_to` は退出先 | [`src/app.rs:20234`](../src/app.rs)、[`src/app/top_level_grid_view.rs:88`](../src/app/top_level_grid_view.rs) |
| DriveList / ReadingHistory / Bookmarks / SubfolderExpansion | 既存の typed restore または既存 synthetic marker を共通 resolver で扱う。表示物理 path を安易に current にしない | [`src/app.rs:20288`](../src/app.rs)、[`src/app.rs:20599`](../src/app.rs) |
| Search / Snapshot | 一時 `DisplayedLocation`。entry origin を `TopLevelGridRestore` に持ち、閉じた時に復元。検索結果そのものは ←/→ stack に入れない | [`src/app.rs:24176`](../src/app.rs)、[`src/app/snapshot_ops.rs:684`](../src/app/snapshot_ops.rs) |

`load_folder_with_scan_claimed` の preflight は、現在地と履歴の移動元を**変更前**に捕捉する。現行の `navigation_history_origin` はこの形だが、DriveList 等は scan 前に Folder へ切り替え、Collection は `adopt_collection_surface_for_physical_load` に採用境界がある [`src/app.rs:22321`](../src/app.rs)、[`src/app.rs:22271`](../src/app.rs)、[`src/app.rs:22627`](../src/app.rs)。Rating→実フォルダも同じ成功採用境界で `Folder` に移し、失敗 / scope 拒否 / sidecar deferred / archive 取消で Rating と履歴を残す。ZIP/PDF の非同期列挙・パスワード・変換の本当の可視採用位置を各 tail で照合し、早過ぎる `replace_surface` を増やさない。Rating 本体へ BS で戻ると `finish_rating_view_install` が Rating surface を再確定する [`src/app.rs:25152`](../src/app.rs)、[`src/app.rs:25524`](../src/app.rs)。

Rating item の open は UI 側で `record_rating_view_nav_open` を load 前に呼ぶ入口がある [`src/ui_main.rs:15242`](../src/ui_main.rs)、[`src/app.rs:41451`](../src/app.rs)。これらを要求時の親 chain capture に変え、追加は可視採用と同時に commit する。failed/cancelled/stale open は chain を変えない。`RatingPhysical` の history/restore snapshot は**採用済み** chain だけを含める。単に surface を Folder にするだけで未採用 B を BS の親 chain に残さない。

Rating 子から別の独立フォルダへ出る採用では、親 chain を同じ境界で退役させ、現在地を通常 `Path` にする。Rating-owned child / descendant の open だけが chain を延長する。検索・Snapshot の一時 entry は chain を退出先として保持する。これにより「Rating を一度開いた後は無関係な folder でも BS が Rating へ行く」という別形の残留を作らない。要求 owner の有無で判定し、パスの見た目や `items_are_rating_view` の残値から推測しない [`src/app.rs:25113`](../src/app.rs)、[`src/app.rs:22321`](../src/app.rs)。

### Rating 一覧への entry / replay の採用 transaction

Rating 一覧の**採用済み現在地**は `TopLevelGridView` の `Rating {stars}` surface と、その surface に対応して install 済みの rows / sort / 選択状態から一か所で投影する。`rating_view_stars`、合成 `current_folder`、address、worker の要求値は、単独では可視現在地の正本にしない。直接メニュー entry と ←/→ の `Rating` replay は共通の typed `RatingNavigationTransition` に通す。transition は `Direct {from: typed origin}` または `Replay {direction, target, pre-pop history snapshot}` の intent、context ID、source surface/items generation、source active A/B slot と switch sequence、target stars / 保存済み sort、worker request ID / cancel token、旧可視 bundle を所有する。検索等の一時 surface から入る場合も既存の typed origin / close 契約を transition に取り込み、準備中に元表示を破壊しない。通常の同一 Rating 一覧 refresh はこの navigation intent を発行せず、採用済み `Rating` の再構築として扱う。

**準備中**は旧 surface・items・Rating BS chain・history current/back/forward・suppress-once・active slot をそのまま保持し、旧表示の上に待機状態を示す。rows の build と、失敗し得る読み取り・整列・選択候補の準備を offscreen で行う。現行 `enter_rating_view_from_menu` の build 前 history 記録、`enter_rating_view` の早期 rows / chain clear と検索 close、`dispatch_synthetic_folder_history_target_with_rollback` の早期 surface / stars / chain 変更、←/→ の dispatch 前 pop/push をこの入口では行わない [`src/app.rs:25025`](../src/app.rs)、[`src/app.rs:25032`](../src/app.rs)、[`src/app.rs:20568`](../src/app.rs)、[`src/app.rs:20614`](../src/app.rs)。worker 内の read-stamp 変化による再試行は同じ未採用要求の phase であり、history を進めない。

**成功**は request ID、context、source generation、slot / switch sequence がなお一致し、rows と表示準備が完成した時だけとする。一つの context 採用境界で Rating rows / surface / stars / sort / address / BS chain を install し、直接 entry なら捕捉した origin を back へ積んで forward を消し、replay なら指定 stack の pop/push と suppress-once 消費を確定する。`current` は install 後の `Rating {stars}` から解決し、他の可視 location や旧 rows を指す瞬間を外へ公開しない。`finish_rating_view_install` の surface 確定をこの境界へ含め、採用後に失敗し得る処理を残さない [`src/app.rs:25524`](../src/app.rs)。**局所失敗・明示取消・worker 切断**では要求だけを破棄し、旧可視状態と history snapshot を維持する。**別 navigation / slot 切替が勝つ**時は旧要求を退役させ、勝者が旧可視状態から自身の採用を行う。遅れた completion や旧要求の rollback が勝者を上書きしない。park は要求と旧 bundle を同じ context に移すか、先に取消して旧状態を保つ。retire はその context の要求と bundle だけを drop する。独立した `bool` / pending sentinel や stack だけの事後 rollback は足さない [`src/app.rs:25272`](../src/app.rs)。

## 3. §1.280 の消費者と期待結果

| 消費者 | Rating→hoge 採用後 |
| --- | --- |
| `folder_nav_current_target`、←/→、後続 folder load、A/B stack | visible path=hoge と Rating 親 chain を持つ typed target を読み、Rating→hoge→←→→ は hoge、Rating→F→G→← は F。A と B の履歴は各 slot だけ更新する [`src/app.rs:20234`](../src/app.rs)、[`src/app.rs:20568`](../src/app.rs)、[`src/app.rs:20857`](../src/app.rs) |
| Ctrl+G/S/T entry と close | hoge と Rating 親 chain を typed restore に保存し、閉じると hoge と BS 帰路を復元する。検索中の移動と close は履歴を汚さない [`src/app.rs:24176`](../src/app.rs)、[`src/app.rs:24257`](../src/app.rs) |
| 通常フォルダ除外件数チップ | `Folder` surface で scan 結果を publish し、同じ path の omitted counts を表示する。検索・Smart・Collection のチップ除外は維持 [`src/app.rs:22751`](../src/app.rs)、[`src/app.rs:54485`](../src/app.rs) |
| 内容識別 `is_physical_folder_listing` | 採用後は `Folder` surface と実 path を見て通常の marker / 設定条件で内容識別の対象にする。ZIP/PDF 仮想一覧は従来どおり対象外 [`src/app/content_identity_detection.rs:125`](../src/app/content_identity_detection.rs) |
| 本操作 `book_action_current_folder` | 採用後の hoge が直接の本フォルダなら操作可能にする。Rating surface の残留で拒否しない [`src/app/saved_group_actions.rs:164`](../src/app/saved_group_actions.rs) |
| `smart_folder_source_lease` | 採用後は `Folder(hoge)` source として lease を作り、古い Rating source と分類しない。context 所有と generation の検証は維持 [`src/app/smart_folder.rs:1990`](../src/app/smart_folder.rs) |
| Smart Folder / Snapshot / Collection の entry origin | hoge と親 chain を退出先に保持し、閉じたら hoge。古い Rating 一覧へ誤復帰しない [`src/app/smart_folder.rs:2275`](../src/app/smart_folder.rs)、[`src/app/snapshot_ops.rs:684`](../src/app/snapshot_ops.rs)、[`src/app/collection_grid.rs:1283`](../src/app/collection_grid.rs) |
| refresh / reload | 実 folder を reload する。Rating worker を再起動しない [`src/app/top_level_grid_view.rs:1179`](../src/app/top_level_grid_view.rs) |
| BS | `rating_view_nav_stack` の親連鎖を使う。直接 open と履歴 → による再訪の双方で、開いたコンテナから Rating 行へ戻る [`src/app.rs:25126`](../src/app.rs)、[`src/app.rs:19669`](../src/app.rs) |

## 4. §1.282 の Collection 遷移

1. C root で item B を開く要求には、既存 `CollectionGridPhysicalLoadOwner::Root` が ID、entry anchor、source path、surface stamp、accepted / wanted revision、items generation を既に持つ [`src/app/collection_grid.rs:937`](../src/app/collection_grid.rs)、[`src/app/top_level_grid_view.rs:399`](../src/app/top_level_grid_view.rs)。可視採用が成功した時、**position を PhysicalSource に変える前**に `CollectionGridRestore`（C と B の anchor）を back に積み、forward を消す。表示 location を `CollectionPhysical(B)` にする。scan 失敗、stale owner、変換取消は commit しない [`src/app/collection_grid.rs:1006`](../src/app/collection_grid.rs)、[`src/app.rs:22271`](../src/app.rs)。
2. B→下位 D は同じ collection session の `PhysicalSource` owner で採用するが、履歴には `CollectionPhysical(B)` を積む。同一 path reload は積まない。従って A→C→B→D の ←← は B→C、→→ は B→D。`collection_grid_parent_nav` とページ送り/全画面の親復帰は従来の C anchor を使い、BS では C の同じ項目へ戻る [`src/app/collection_grid.rs:861`](../src/app/collection_grid.rs)、[`src/app.rs:19775`](../src/app.rs)。
   BS による B→C も、成功して C の loading shell を可視採用する時点で `CollectionPhysical(B)` を back に積む直接移動である。続く Back は B、Forward は C に戻る。Rating 子→Rating 一覧と Rating 子孫→親、通常物理フォルダの BS も同じ直接移動として採用時に移動元を積む。ZIP 内部の BS は `zip_nav` のみを変え、外側の現在地と stack を維持する。ZIP/PDF 一覧からの BS はその provenance の親へ出る一回の外側移動であり、Back は一覧、Forward は親を再現する。history replay と transient restore は stack に新しい直接移動を積まない。
3. ← で C に戻す時は最新 snapshot に収束させ、entry ID→source key→選択なしで anchor を解決する。→ で B へ戻す時は単なる `Path(B)` load にせず、履歴の collection provenance から**新しい context / surface stamp の open request**を作り直し、最新 catalog / prepared entry に対して検証後に子を採用する。古い `CollectionGridPhysicalLoadOwner` を保存・再利用しない。root と子の session、watch、再生順、BS 帰路を維持する。entry が消えた場合は元の可視一覧と履歴を保つ。
4. `TopLevelGridRestore` も子の restore を表せるようにして、子から検索・Snapshot・Smart Folder・別 Collection へ出て戻る時に B を復元する。`collection_grid_restore_snapshot()` は **root 帰路**用と **可視子現在地**用を分ける。前者は BS と playback/page flip が使い、後者は history と transient entry が使う。`TopLevelGridView` / `CollectionGridSession.position` が所有する typed location から両者を投影し、新しい App-global sentinel は作らない [`src/app/collection_grid.rs:842`](../src/app/collection_grid.rs)、[`src/app.rs:20288`](../src/app.rs)、[`src/app.rs:24257`](../src/app.rs)。
5. Ready catalog が C の削除を確定した場合、normal / A / B の root と collection-owned child の両 target を prune し、rollback snapshot からも復活させない。Starting / Failed / Inert では保持する。表示中の物理子 B はその context の可視実パスとして続け、`CollectionGridSession` の既存 `Deleted` terminal を tombstone として保持する。`collection_grid_parent_nav`、collection playback、owned child open は deleted ID を拒否し、BS は通常の物理親へ進む。←/→ の `current` と検索/Snapshot の origin は `Path(B)` に投影する。reload も deleted C を再要求せず B の物理 reload を行う。次の成功した独立物理 load で Folder surface へ移って tombstone session を落とす。失敗時は B の可視 items を保持する。root の Deleted 表示は既存の削除フィードバックを維持する。これは context ごとの状態遷移であり sibling を巻き込まない [`src/app/top_level_grid_view.rs:494`](../src/app/top_level_grid_view.rs)、[`src/app/collection_grid.rs:861`](../src/app/collection_grid.rs)、[`src/app.rs:19669`](../src/app.rs)、[`src/ui_dialogs/collections.rs:1792`](../src/ui_dialogs/collections.rs)。

### 履歴 replay の transaction owner

現行 `navigate_folder_history_back/forward` は先に stack を pop/push し、`open_collection_grid` は actor prepare 前に旧 items を空へ置き換える [`src/app.rs:20568`](../src/app.rs)、[`src/app.rs:20857`](../src/app.rs)、[`src/app/collection_grid.rs:1327`](../src/app/collection_grid.rs)。Collection root/子の履歴復元と transient restore はこれをそのまま呼ばない。context ID、source surface/items generation、発行時の active slot ID / slot-switch sequence、選択方向、typed target、pre-pop 履歴 snapshot、cancel token、catalog/revision watch、準備 phase を**一つの typed staged request**が所有する。旧可視 bundle は mounted のまま操作不能な待機表示にして保持し、Collection snapshot/prepare と root anchor 解決を offscreen で行う。子ならその後に Folder scan、ZIP/PDF 列挙と password、書庫変換を同じ owner の phase として進める。UI thread に新しい同期 scan を置かない。

各 phase 完了時に request ID、context ID、source generation、slot ID / switch sequence、collection ID、最新 catalog と prepared revision を再検証する。**局所失敗**（対象不存在、scan/password/変換の失敗・取消、sidecar deferred 未成立）なら staged result だけを破棄し、旧表示と pre-pop stack を維持する。**別 navigation が勝つ**場合は旧 staged request だけを退役させ、勝った request に旧可視状態からの採用/履歴 commit を委ねる。旧 request の rollback が勝者の表示や stack を上書きしてはならない。**park** は request と旧可視 bundle の所有を同じ context へ移すか、park 前にその request を取り消して旧状態を保持する。**context retire** は pending、旧 bundle、履歴保護をその context と共に drop し、main/sibling を復元対象にしない。成功時にだけ target の session/physical listing を可視採用し、同じ context transaction 内で intent 別の履歴操作（replay は pop/push と suppress-once 消費、transient restore は cursor 不変）を確定する。新しい target の可視採用後に失敗し得る処理はこの境界の前へ移す。既存 `CollectionNavigation` の offscreen prepare と exact request 相関を再利用できるかを実装前に照合し、`open_collection_grid` を早期に呼ぶ経路を残さない。新しい `bool` / pending sentinel や stack-only rollback は作らない [`src/app/collection_navigation.rs:3134`](../src/app/collection_navigation.rs)、[`src/app/top_level_grid_view.rs:399`](../src/app/top_level_grid_view.rs)。

同型 producer を一緒に照合する。グリッドの Folder/PDF/ZIP とその子孫は `commit_collection_grid_physical_load`、変換書庫は `commit_main_grid_archive_transition`→`commit_collection_grid_source_open_owned`、再生/ページ送りの外側 source は `collection_navigation` の `commit_collection_grid_source_open` を通る [`src/app/collection_grid.rs:1059`](../src/app/collection_grid.rs)、[`src/app.rs:19628`](../src/app.rs)、[`src/app/collection_navigation.rs:3134`](../src/app/collection_navigation.rs)。履歴記録を低水準の source commit に無条件で入れると自動再生・同一 reload まで stack に積む。各要求の**ユーザー操作起点と採用結果**を一つの transition owner に渡し、明示的な子への移動だけ履歴を積む。再生の自動 source 変更では現在地投影を更新しても毎ページの履歴は増やさず、C への BS と親復帰を保持する。各 producer の失敗・取消・stale result も履歴を変えない。

## 5. viewer context と A/B の所有

履歴 stack（normal、A、B）と active slot は `App` 側で保持し、`ViewerContextBundle` の swap 対象ではない。一方 surface、Collection session、Rating の BS stack は viewer bundle 側で、物理 detached 作成時は Folder surface に初期化される [`src/app.rs:13556`](../src/app.rs)、[`src/app/viewer_context_registry.rs:911`](../src/app/viewer_context_registry.rs)、[`src/app/viewer_context_registry.rs:2184`](../src/app/viewer_context_registry.rs)、[`src/app/viewer_context_registry.rs:2963`](../src/app/viewer_context_registry.rs)。`detached_viewer_suppresses_main_history_persistence()` と `DetachedPhysical` scope は main history 汚染を抑止する [`src/app.rs:19238`](../src/app.rs)、[`src/app.rs:20377`](../src/app.rs)、[`src/app.rs:22271`](../src/app.rs)。

今回の決定では、←/→ は**main の active A/B workspace の履歴**だけが所有する。history の target read（`folder_history_back_target` / `folder_history_forward_target`）、current/stack/suppress-once の参照・更新、`navigate_folder_history_back/forward` とその入力 handler は、投影中の viewer context が main である時に限り同じ main-history router を通す。detached を投影中は target read が `None`、back/forward dispatch が no-op で、main の current / stack / suppress-once / Collection anchor に触れない。これは detached 窓の存在だけで main を止める global guard ではなく、呼出元 context に基づく所有境界である。detached physical の open / close、BS、page flip、parked completion も main 履歴を読まず変更しない。context ID と surface generation を持つ非同期 owner の結果は、発行元 context のみに採用する。root/子の read-only 往復で sibling の watch、items、thumbnail、cache を失効させない。detached に独立 ←/→ が必要になる場合は context 所有 history の別設計とする。detached predicate / viewport path を実装で触れるなら、事前構造合意と [`detached-rework-plan.md` §11](detached-rework-plan.md#11-リワーク外からの変更記録) への記録を要する。

A/B の `QuickFolderWorkspace.target` は最後に記憶した物理フォルダ用であり、Collection root/owned child の履歴地点をその永続 target に変換しない。slot 切替自身は history transition を作らず、戻った slot の stack を他 slot と混ぜない [`src/app.rs:20119`](../src/app.rs)、[`src/app.rs:20155`](../src/app.rs)、[`src/app.rs:19962`](../src/app.rs)。現行 target 契約を維持すると、A→C→B の途中で別 slot へ移って A に戻った時は記憶済み物理 A を表示し、B の可視 child は自動再開しない。A の back stack に C は残るが、B は stack に無い。B の再開を A/B 切替の要件へ追加するなら、各 workspace の typed current/restore owner と切替時の採用 transaction が必要になるため、単に Path target を B へ書き換えない。

`rating_view_nav_stack` は viewer bundle にあり、A/B workspace ごとには保存されない [`src/app/viewer_context_registry.rs:947`](../src/app/viewer_context_registry.rs)、[`src/app.rs:25113`](../src/app.rs)。A の Rating child から B の通常 folder へ切り替える場合、**B の成功した可視採用時**に A の Rating BS chain/保留 open を退役させる。B の load が失敗・取消なら A の可視 child、active slot、BS chain を維持する。B で `rating_view_nav_context_active` と `folder_nav_current_target` が A の chain を読まないよう、switch の採用 transaction と location resolver/BS router を同じ owner にする。A に戻って記憶済み物理 A を表示したら通常の親 BS とし、古い Rating chain は再活性化しない。A の履歴に残る Rating target は ← で再入場できるが、A の旧 child を暗黙には再開しない。context 内の source/target slot は成功境界で切り替え、失敗時だけ旧 slot が current のまま残る。

同じ物理 path を両 slot が記憶していても、`activate_quick_folder_slot` の現行 path 一致 `QuickFolderSwitchTarget::Current` 近道だけで切替を確定しない [`src/app.rs:20119`](../src/app.rs)。現在地が `CollectionPhysical` / `RatingPhysical` / Smart 等の typed 特殊地点なら、B の記憶した**通常 Path**とは異なる。通常 Folder の offscreen load/adoption を通し、成功時に A の session/BS provenance を退役、必要な items/cache-key の所有を B の Folder に合わせてから active slot を B へ変える。失敗なら A の表示と slot を保持する。現在地が既に同じ通常 `Path` で所有元も一致する時だけ `Current` を許す。パス等値だけで Collection の full-path cache key を普通の Folder に流用しない [`src/app.rs:22658`](../src/app.rs)。

main-history router は active slot と共に **slot-switch sequence** を所有する。Collection / Rating / 物理 load の staged history replay は発行時の slot ID（normal を含む）と sequence を捕捉し、各非同期 phase と最終 commit で双方を照合する。`activate_quick_folder_slot` の全ての切替要求では、記憶 path が同じで `Current` に着地する近道も含め、slot の参照・suppress-once 操作より前に sequence を進めて既存 replay を退役させる。offscreen switch が失敗して元 slot が可視のままでも、退役した replay の completion は採用しない。`Current` の場合は退役後に typed location / 所有元を確認して slot 変更を同一同期境界で確定する。replay の局所失敗・退役は捕捉元 slot の stack と suppress-once を変えず、B の stack や suppression を消費しない。slot ID の一致だけでは A→B→A の間に古い要求が戻る可能性があるため sequence も必須とする。

## 6. §1.281 RatingViewSort の保存

`RatingViewSort` は現在 `Normal(SortOrder) | RatedAtDesc | RatedAtAsc`、既定は `RatedAtDesc`。`Normal(RatingAsc/Desc)` は rating view の中では `FileName` に正規化される。§1.237B 後の挙動であり、既存 `settings.sort_order`（通常一覧）や Collection order とは別の値である [`src/rating_view.rs:12`](../src/rating_view.rs)、[`src/rating_view.rs:24`](../src/rating_view.rs)、[`src/app.rs:25440`](../src/app.rs)。

`Settings.rating_view_sort: RatingViewSort` を一つ追加し、全 ★1〜★5 共通にする。enum に serde derive、field に `#[serde(default)]`、`Settings::default()` に現行の `RatedAtDesc` を入れる。setter `set_rating_view_sort` は `Normal(RatingAsc/Desc)` を `Normal(FileName)` に**設定書き込み前**に正規化し、その一つの正規化値を session と設定へ同時に反映して変更時だけ保存する。通常一覧の `settings.sort_order` の現行更新規則は維持するが、Rating の実効ソートと保存値は一致させる。Rating の直接 entry と履歴 replay は §2 の共通 transition が設定の正規化値を読み、同じ ★の view-local refresh は現在の選択を保持する [`src/app.rs:25032`](../src/app.rs)、[`src/app.rs:20613`](../src/app.rs)、[`src/app.rs:25363`](../src/app.rs)。複数の UI ソートメニューは setter へ集約し、時刻順を選んだ場合も確実に保存する [`src/ui_main.rs:7490`](../src/ui_main.rs)、[`src/ui_main.rs:9951`](../src/ui_main.rs)、[`src/ui_main.rs:10134`](../src/ui_main.rs)。§1.143 の `reset_details_sort_to_toolbar` はそのまま呼び、詳細列ソートを Rating 一覧ソートへ混ぜない [`src/app.rs:25040`](../src/app.rs)、[`src/app.rs:20619`](../src/app.rs)。

現行の一般 refresh router には `Normal(_)` の時に `rating_view_sort` を `settings.sort_order` で上書きする経路がある [`src/app.rs:21282`](../src/app.rs)。永続化後はこの経路も Rating 専用の保存済み選択を正本とし、通常一覧側で `sort_order` が変わっても Rating の選択が上書きされないようにする。UI が Rating の Normal sort を選んだ時に従来どおり通常 `sort_order` も更新する動作は別件として維持する。

Preferences は開いた時点の `Settings` draft を OK 時に丸ごと適用する。Rating sort は Preferences の編集項目ではないので、`Settings::overwrite_non_preferences_from` の runtime sort 群にも `rating_view_sort` を追加し、開いている間に toolbar で変えた値を stale draft に戻さない [`src/settings.rs:9482`](../src/settings.rs)、[`src/settings.rs:9551`](../src/settings.rs)、[`src/ui_dialogs/preferences.rs:1861`](../src/ui_dialogs/preferences.rs)。setter 保存前の既存 UI `settings.save()` 重複呼び出しも一か所へ整理する。

設定 DB は `Settings` を field ごとの JSON として保存・復元する [`src/settings_db.rs:12`](../src/settings_db.rs)、[`src/settings_db.rs:2893`](../src/settings_db.rs)。`v4.1.0` tag に `Settings.rating_view_sort` は存在しないとレビューで確認済みであり、新しい **未出荷 field** には既存ユーザーデータが無い。missing field は `serde(default)` で従来値になるため DB migration は不要。ただし Rating view 機能自体は既出荷であり、未知 enum 値/既存 `sort_order` の互換を変えない。設定保存/再起動・欠落 field・全 ★段共通・rating 要求の name order 正規化を roundtrip test で確認する [`CLAUDE.md:1654`](../CLAUDE.md)。

## 7. 実装後の検証計画

**状態遷移・実 load/adoption**: 直接 stack を組むテストだけでは今回の残留 surface を再現しない。`AppTestEnvForTest` の folder fixture と Collection actor fixture から実入口→scan/visible install→←/→ dispatch まで通す。Rating→hoge→←→→、Rating→F→G→←、Rating→コンテナ→BS、Rating→コンテナ→←→→BS、Rating→A/B 切替後の独立 stack、Rating→Ctrl+G/S/T→close→hoge、Smart/Snapshot/Collection の entry origin、通常チップ、reload を検証する。Rating の直接 entry と ←/→ replay の双方で、build pending 中は旧 items / surface / history / BS chain が一致したまま、成功時だけ同時に Rating へ変わることを確認する。各入口で build 失敗・取消・worker 切断・read-stamp 再試行・別 navigation への supersession と stale completion を注入し、局所失敗なら旧表示/履歴、勝者がある時は勝者の表示/履歴だけが残ることを assert する。既存 stack-only test は残す [`src/app/tests.rs:13401`](../src/app/tests.rs)、[`src/app/tests.rs:70337`](../src/app/tests.rs)。

Rating→実フォルダ採用後は、`is_physical_folder_listing` が条件を満たす実 folder で内容識別を許し ZIP/PDF 仮想一覧では許さないこと、`book_action_current_folder` が直接本フォルダを許すこと、`smart_folder_source_lease` が `Folder(hoge)` を返すことを個別の handler-level test で確認する。Rating から実 ZIP/PDF を開き内部 `zip_nav` の下位移動と戻り・PDF ページ移動を行っても外側 history の current / back / forward は**一つの元書庫 `Path` または provenance 付き地点**のままとし、←/→ は外側の書庫を一地点として往復し、BS の内部階層優先と Rating 帰路を維持する。同じ実 load の assertions を Collection root/owned child からの ZIP/PDF open にも適用し、Collection 子なら root/子の外側地点と anchor を維持する。

Collection は A→C→B→←（C と anchor）→（B、session 維持）、B→D→←←（B→C）、BS の親復帰、fullscreen/playback/page flip の帰路、C1/C2 と A/B、root reload、failed/stale/cancel、rename、Ready delete prune と rollback、Starting/Failed の非 prune、同じ ID で anchor 更新、検索 open/close、別 context と sibling 不変を確認する。履歴 C/B の offscreen prepare 中に entry 消失、scan/変換/password の局所失敗を挟んだ時は旧可視 items と履歴 snapshot が共に残る。別 navigation が勝つ時は旧 completion が勝者を変更しない。park は owner を同じ context へ移すか取り消し、retire はその context だけを drop する。B 表示中の Ready delete→BS/←/→/reload/検索→通常 Folder 採用、および sibling session 不変も含める。A→C→B→slot B→slot A は記憶済み物理 A に戻り、B を自動再開しないことを固定する。B の remembered path が child B と同一でも、B は通常 Folder として採用され、A の Collection session を持ち込まない。Collection / Rating の replay を pending にして A→B 切替、B→A 切替、同一物理 path の `Current` 近道を行い、旧 replay がいずれの slot の stack / suppress-once / 表示も変更しないことを検証する。既存 Collection テストの root→B *独立移動* 契約と、collection-owned child の新契約を区別する [`src/app/collection_grid.rs:3282`](../src/app/collection_grid.rs)、[`src/app/collection_grid.rs:3402`](../src/app/collection_grid.rs)、[`src/app/collection_grid.rs:4765`](../src/app/collection_grid.rs)。sort は `Settings` JSON/DB roundtrip、欠落値、★段切替、二つの入口、refresh、詳細列 sort の独立性、Preferences を開いたまま toolbar で変更→OK→再起動して値が残ることを確認する。`Normal(Rating*)` を入力しても保存前に `Normal(FileName)` となり、再起動後も同値であることを検証する。A→Rating child R→slot B→通常 folder/BS→slot A→BS でも B に A の chain が漏れず、A は記憶済み物理 target から通常親へ動くことを確認する。B の remembered path が R と同一の場合も typed Path として採用し、B の BS が Rating に入らないことを検証する。

main の back/forward に実 load から有効な target を用意した上で、detached を投影中に back/forward target read が両方 `None`、キー/menu の back/forward dispatch が no-op になることを handler-level で確認する。main と detached を交互に投影し、detached の open/close/BS/page flip/parked completion や失敗が main の各 slot の current / stack / suppress-once と sibling の items/cache/watch を変更しないことを確認する。main 投影に戻せば同じ履歴を読んで往復できる。

**実アプリ smoke は実装・自動 gate 後の承認済み検証枠**: `FolderHistory` scenario を `scripts/ui-smoke.ps1` と [`scripts/ui-smoke/folder-history.rhai`](../scripts/ui-smoke/folder-history.rhai) に追加した。disposable `target/portable-smoke/data` に生成する物理 A/F/G・B/D と、B を登録した Collection C、★1 の F を使う。`tap_key("Left", "alt")` / `tap_key("Right", "alt")` / `tap_key("Backspace")` を通常の入力 route に通し、各着地で `snapshot().grid_surface`、`current_folder_path`、Collection ID または表示項目名を wait/assert し、`capture()` checkpoint を残す。Rating と Collection の開始だけを bounded な test-script action で発行し、履歴と BS の判定には使わない [`src/test_script.rs`](../src/test_script.rs)、[`docs/keymap-spec.md:338`](keymap-spec.md)。通常 profile を起動せず、実行前に [`interactive-release-verification.md`](interactive-release-verification.md) の内容・時間・desktop 使用と使い捨て範囲を提示して利用者の明示承認を得る。

狭い state/handler test → shared behavior test → `scripts/test-full.ps1`、fmt、UI glyph（文言変更時）を gate とし、その後に確認 build を作る。UI smoke は自動 gate と分離する [`development-build-and-test.md`](development-build-and-test.md)。

## 8. 設計担当の確定事項と再レビュー境界

1. Collection root/子の履歴 replay は §1.282 の範囲で通常 `CollectionNavigation` の offscreen prepare と共有する。可視結果と history stack を一つの transaction で採用し、`open_collection_grid` の早期空 install を replay に使わない。共有できる関数境界は実装時に照合する。
2. B 表示中に C が Ready-delete されたら `Deleted` session を tombstone として保持し、BS/refresh/検索 origin は物理 B とする。root の Deleted feedback は維持し、B source path の scan が失敗すれば B の旧表示を維持する。
3. 履歴は main が所有し、detached context は main の history target を**読まず**、stack/current/suppress-once を**変更しない**。§5 の main-history router を入口から適用し、独立 detached history は今回追加しない。
4. ZIP/PDF の内部階層は外側の `FolderNavHistoryTarget` では**一地点**とする。`zip_nav` / PDF 内部移動で外側の current / stack を増やさず、BS の内部階層優先と Rating/Collection の帰路を §7 の実 load で固定する。
5. Rating sort 設定は全 ★段共通の一つとし、`Normal(Rating*)` は `Normal(FileName)` に**保存前**に正規化する。通常一覧 `settings.sort_order` の現行更新規則は維持する。
6. Rating→B→←→→ 後の BS は Rating へ戻す。`RatingPhysical` の採用済み親 chain を forward 採用時に再設置する。現行 dispatch の chain clear と単純な `Path(B)` load のままにはしない [`src/app.rs:20619`](../src/app.rs)、[`src/app.rs:25148`](../src/app.rs)。
7. A/B 切替は記憶済み物理 target へ戻し、Rating/Collection child を自動再開しない。切替で child を再開する per-slot typed current owner は今回の scope に含めない。

独立 reviewer は §2 の Rating entry/replay transaction、§5 の slot sequence と detached routing、§7 の検証を含む構造案を承認した。stage B の completion review でも重大な設計欠陥と未充足の受入条件はなく、指摘された変換後 ZIP preflight 起動失敗時の DFS 終了処理も修正・再レビュー済み。自動 gate の結果は §9 に記録する。本案だけで未確認の実機症状を「再現済み」とは扱わない。

## 9. Stage B 実装・検証記録（2026-09-27）

`history-nav` worktree では、採用済み表示位置から Rating / Collection root・子 / 通常物理地点を区別する履歴、Rating の直接 entry と replay の offscreen 採用、Collection 子の root anchor 付き履歴、A/B の成功時 slot 切替、main と detached の履歴所有境界、全 ★段共通の Rating sort 保存を実装した。直接 ZIP/PDF と変換書庫は元の論理書庫を外側履歴の一地点とし、内部ページ・階層は既存の書庫 navigation に任せる。グリッドからの ZIP/PDF open が持つ読み取り履歴、★/facet 絞り込み退避、自動 fullscreen 予約も typed request に捕捉し、準備中は旧一覧と旧状態を保持して、可視採用後に確定する。失敗・取消・stale completion では旧状態と履歴を維持する。

現時点の自動検証記録（同じ worktree、実アプリは起動していない）:

| 範囲 | 結果 |
| --- | --- |
| `cargo test -p mimageviewer --lib staged_dfs` | 6 件 PASS。ZIP preflight 失敗、PDF password 取消、変換無視・変換開始拒否、items generation 不変での要求置換、ZIP 採用後の queued step を含む。 |
| `cargo test -p mimageviewer --lib favorite_search_keeps_source_rows_and_nav_stack` | 2 件 PASS。検索からの ZIP 失敗 / PDF password 取消で旧 rows、address、履歴、検索 nav stack、読み取り履歴、★/facet フィルタを保持。 |
| `cargo test -p mimageviewer --lib grid_virtual_zip_and_pdf_effects_commit_after_prepared_adoption` | 1 件 PASS（ZIP と PDF の両方）。準備中は元 ZipFile/PdfFile rows と各効果が不変。採用後は仮想ページ rows、読み取り履歴、★/facet 退避、自動 fullscreen open または deferred reopen を確認。 |
| Rating の狭い回帰 | `rating_view_nav_open_dedupes_only_after_visible_adoption_and_close_clears_stack`、`staged_rating_pdf_password`（2 件）、`superseded_rating_pdf_password_submit_cannot_open_the_old_pdf`、`staged_rating_converter`（2 件）は PASS。 |
| Collection / script | Collection module の focused run は 50 PASS / 1 ignored（以後の ZIP cache pin-key 追加前）。追加した直接 Collection ZIP cache の pin-key test も個別 PASS。`folder_history_smoke_script_parses_without_launching_the_app` と `synthetic_backspace_uses_the_parent_navigation_key` は各 1 PASS。fixture 生成と PowerShell / Python 構文検査も PASS。 |

上記の `phase_c_folder_nav_history_tests::` run には直接検索 ZIP 成功、DFS fullscreen / slideshow 再開、Rating physical child、A/B の回帰も含む。detached の focused 回帰は専用 1/1 PASS。通常・portable/test-script の check、`cargo fmt --all -- --check`、UI glyph check、`scripts/build-dev.ps1` は PASS し、通常 profile の確認 build は未起動。

統合中の broad lib 初回は 9,268 PASS / 20 FAIL だった。失敗の多くは「要求時に表示・履歴・A/B slot や archive dialog が変わる」という旧 test 契約で、成功採用境界へ更新した。更新後の `phase_c_folder_nav_history_tests::` は 81/81 PASS、`rating_view_navigation_tests::` は 12/12 PASS。独立 reviewer が別に見つけた detached Collection の `return_to` fallback と、Similar の RequiredFullscreen ZIP/PDF が保持すべき exact leaf / Snapshot 退出も App 側で修正し、Similar focused 13/13、detached Collection 1/1 が PASS。変換書庫 cache、gamepad の延期効果、起動中 archive 要求置換の focused test も各 1/1 PASS。completion review で見つけた変換後 ZIP preflight 起動失敗時の DFS 待機 step / lock 残留も終了分岐で解消した。追加した失敗注入テストは修正前に `pending_folder_nav_steps=2` で失敗し、修正後 1/1、履歴回帰群 82/82 が PASS。該当差分の独立再レビューも指摘なし。

統合後の初回全 lib suite は **9,288 PASS / 48 ignored / 0 FAIL**。`scripts/test-full.ps1` は release core / Remote の前提 binary と Susie fixture を揃え、`RUST_TEST_THREADS=4` で **PASS**（workspace、vendor egui / egui-wgpu / eframe、UI snapshot を含む）。`multiwindow_scenario_collection_root_async_sibling_result_is_owner_scoped` timeout は、テストが待機状態を検査する前に ready な `Snapshot` を poll で消費する観測競合と特定し、pending を先に検査して `RequestNeeded` だけ poll するよう修正した。最後の DFS 終了処理追加後、ページファイル不足（OS error 1455）で一度 gate compile が止まったが、`CARGO_BUILD_JOBS=1` にして再実行した最終の `scripts/test-full.ps1` は **PASS**。`scripts/build-dev.ps1` は最終ソースで成功し、通常 profile の確認 build は未起動。`scripts/prepare-portable-smoke.ps1 -TestScript` も最終ソースで release core / Remote build と VCRT/PE check を通って成功し、`target/portable-smoke/mimageviewer.exe`（99,276,800 bytes）と隔離 `target/portable-smoke/data/.disposable-smoke-data` を更新した。manifest の flavor は `portable-test-script`、features は `portable`, `test-script`。**実アプリ smoke は未実行**。

`FolderHistory` の無人 scenario と使い捨て fixture は追加済み。←/→/BS は実際のキー入力 route を通し、Rating / Collection の開始だけ bounded test-script action を使う。**実アプリ smoke は未実行**であり、利用者が desktop 使用・所要時間・使い捨てデータ範囲を確認して明示承認した検証枠まで保留する。

## 10. 独立実装レビュー後の修正（2026-09-27）

検索 Ctrl+G/S/T の一時 surface 中に ring / gamepad の履歴 command が通り、`return_to` を現在地として main の Back/Forward stack を消費し得た。keyboard / toolbar / ring / gamepad の入力が合流した後の `AddressBarNav::HistoryBack/Forward` 共通 dispatch で、検索・local filter・Snapshot 中の履歴操作を拒否する。検索内の Ctrl+↑↓ と BS は各検索の navigation を使い、main history cursor は退出まで休止する。ring route と keyboard route の回帰テストは修正前に失敗し、修正後に通過した。

追加されていた UI-thread の `Path::is_file()` は `will_stage_archive_navigation`、`load_folder_or_convert_archive_with_auto_fullscreen_owned`、`load_folder_with_scan_claimed`、RequiredFullscreen の ZIP/PDF 分岐から除いた。表示済みの `GridItem::Folder` と RequiredFullscreen の `SnapshotTarget` は既知の型を使う。未分類の archive 風 suffix は worker preflight に渡し、worker が folder と確定した場合も folder として採用する。`directory.zip` のような実 directory を誤って ZIP と断定しない。worker が folder scan を返した後の legacy file 判定も省く。`.rar` / `.7z` / `.lzh` 名の directory は書庫 Ignore 設定でも worker が directory と確定してから開き、実際の変換書庫だけを Ignore する。新規 UI 経路の filesystem probe は追加していない。

実ファイルの ZIP 内部階層と PDF 2 ページから、Rating / Collection provenance の内部移動・外側 Back/Forward・BS を確認する回帰テストを追加した。lib test executable は PDF worker subprocess の `--pdf-worker` entry point を持たないため、PDF fixture の実ページはテスト内で PDFium により列挙し、得た page entries のみ staged preflight へ渡す。アプリ側の可視採用、ページ移動、history、BS は通常経路を使う。`FolderHistory` の disposable fixture と無人 scenario にも ZIP/PDF の手順を加えた。初回実アプリ smoke では Rating store の正規化 key（小文字・`/`）が一覧に現れる既存仕様を確認し、名前と path の比較を大文字小文字・separator 非依存へ修正した。次の run は Rating の forward まで成功したが、`GridMoveFirst` の ACK と `tap_key` の入力順を同期していなかったため ZIP の代わりに G を開いた。test-script snapshot に `snapshot_frame` と `selected_index` を追加し、各 grid move の後に新しい frame と目的 index を待つ。PDF の page move もこの barrier で機械的に確認する。固定 sleep は使わない。変更後の live 再実行は design owner が disposable portable copy で行う。

追加レビューで、古い ZIP/PDF/変換書庫 tile が worker 到着時には同名の directory になった境界も確認した。Folder 判定後は source owner を検証してから grid-open の戻り先・★/facet 一時解除を可視行の計算より前に適用し、画像のみ folder の自動 fullscreen 選択も維持する。Rating/Collection の変換書庫 tile は Ignore を metadata 判定後にだけ適用し、directory なら Folder として採用する。古い ZIP tile の filter/return/fullscreen と Rating の `.7z` directory + Ignore のテストは、対応する修正前にそれぞれ失敗し、修正後に通過した。ZIP/PDF 実 load の追加テストは不足していた coverage の補完であり、元の実装でも ZIP 部分は成功していたため、製品不具合の red 証拠とはしない。

Collection root からの変換書庫 tile が directory になった場合、Folder rows の install が `items_generation` を進める前に root source anchor と履歴を commit する。Collection descendant の再生要求は physical path が visible になってから commit する。この順序の違いは各 owner の有効性判定に合わせる。Collection root の stale `.7z` tile テストは修正前に position が Root のままで失敗し、修正後に PhysicalSource と Back の Collection target を確認した。

修正後の focused history 群は 89/89、実 ZIP/PDF は 4/4、全 lib は 9,302 PASS / 48 ignored / 0 FAIL。`cargo fmt --all -- --check` と `git diff --check` は clean。`scripts/prepare-portable-smoke.ps1 -TestScript` は release core / Remote と VCRT/PE 検証を通り、隔離 `target/portable-smoke/data` を持つ bundle を更新した。`scripts/build-dev.ps1` も通常 feature の core / Remote と VCRT/PE 検証を通った。実アプリは起動していない。

## 11. EPUB 統合時の履歴 open 所有（2026-09-27、設計担当決定）

EPUB→PDF 統合との merge は案 A を採用する。変換ダイアログの継続先は `Direct(owner)` と `StagedHistory(context, request_id)` の型付き区別とし、後者では既存の物理／Collection 子 history transition が source snapshot、replay target、採用を最後まで所有する。ダイアログはその子操作であり、履歴を記録・rollback しない。未変換の `NotConverted` は worker から型を保って返し、Ask／Convert／Ignore／取消／失敗／古い公開では旧表示、address、履歴、A/B slot を維持する。変換成功後は同じ context と request ID を照合し、論理 `.epub` を再 preflight してから可視採用する。新しい直接 open は同一 path でも承認された admission 時に旧 history 変換を退役させ、history open は承認された admission 時に直接列挙・変換を退役させる。拒否された検索／detached 履歴入力は既存 open を退役させない。

直接 EPUB open が未完了のまま history request を受け付けた場合、直接列挙または変換が保持する履歴・アドレスの rollback を一度だけ復元し、その後に staged request の source を記録する。staged request の失敗・取消で戻る先は直接 EPUB を始める前の表示である。直接 open の owner は `Navigation`、`RatingPhysical`、`QuickFolderSwitch`、`CollectionGridPhysical`、`MainGridArchive`、`Bookmark`、`DetachedGridArchive` を同じ admission 境界で扱い、承認された main surface の要求は旧 staged preflight を退役させる。detached physical scope は main の要求を退役させない。

2026-09-27 再レビュー修正: 直接 PDF/EPUB の非同期列挙は、採用前に元の rows・surface・selection を変更しない。warm `pdf_meta` の仮ページも直接 open では先出しせず、列挙成功と owner 採用後にページを表示する。元表示の巨大な複製を rollback に持たせず、未採用要求の可視変更をしない単一規則とする。直接 EPUB 変換と PDF パスワード確認は同じ typed restore に採用元を保持し、再開・取消・history supersession まで運ぶ。失敗・取消・後続 history 要求では元の display と履歴・address・A/B が残る。detached cache archive の admission は owner の window lease から context ID を解決し、同じ context の staged transition のみ退役させる。`navigation_scope` はこの判断の正本ではない。

この typed continuation は使用中だけ heap に置く。直接採用元、PDF 列挙 owner、PDF パスワード継続値を `App` に inline 保持すると、default-stack の fullscreen capture 回帰が overflow した。boxed 化で `size_of::<App>()` は 111,872 → 108,296 byte、同テストは既定 stack で成功した。

history の PDF/EPUB preflight は `PdfEnumerateResult` 全体（ページ、綴じ方向、世代 stamp）を渡し、直接 open と共通の prepared-PDF 成功処理で採用する。history の採用に仮ページや一時的な `pdf_enumerate_pending` owner は作らない。worker は既知の item kind があっても実 file／directory を判定し、`.epub` 名の directory を Folder として扱う。page flip と ZIP 内部階層は外側履歴の一点のままである。

ダイアログの「変換して開く」は元の論理 `.epub` target へ戻る。「PDF を保存」は sibling `.pdf` を**明示的な新しい行き先**として再 preflight し、成功時にだけ採用する。Back／Forward の head が EPUB だった場合、その entry を pop しない。Collection root の明示 open では既存の source anchor/provenance を維持できる場合に維持し、EPUB entry の外へ出る Collection child replay は独立した物理行き先となる。fullscreen lock と parked／retired context は同じ staged request の終端で片付ける。

## 12. EPUB モーダルと直接 PDF の採用境界（2026-09-28、利用者決定・実装）

本節は §11 の 2026-09-27 実装記録のうち、**変換中に別の open が直接／履歴 owner を退役できる**という記述と、**warm `pdf_meta` placeholder を直接 open で出さない**という記述を更新する。§11 の typed `Direct(owner)` / `StagedHistory(context, request_id)`、`NotConverted`、staged preflight、Save PDF の独立 destination、detached の context 所有は維持する。コードと自動テストは本節に従って更新した。WebView2 を伴う変換と初回表示の体感は対話的検証 session に残る。

### 12.1 D-A: EPUB 変換中の入力所有

`EpubConvertState` が生成された時点から、その worker の成功・取消・失敗の終端（失敗画面では利用者が閉じるまで）まで、**同じ可視ダイアログと同じ `modal_dialog_block_reason() == Some("epub_convert")`** を維持する。`epub_convert_dialog_visible()` は確認省略設定や `Scanning` 相ではなく typed state の生存から導く。描画側も `Scanning`、`Confirm`、`Converting`、`Saving`、`Stale`、`Error`、`SaveError` の全相でダイアログを描く。必要なら `egui::Modal` に寄せ、ダイアログ内の取消・再試行・閉じる以外の背面 keyboard、mouse、wheel、gamepad、ring、toolbar、bookmark、detached 窓からの open を通さない。取消は走行中も利用可能で、cancel token と owner 終端が一度だけ source を復元する。閉じるボタン、Esc、worker failure、window close も同じ終端を使う。

EPUB 専用の D13 `epub_file_handling=Ask` は inspect・確認・変換・保存・終端表示まで同じ modal。`Convert`（「確認せず変換する」）は確認画面だけを省略し、開始直後から進捗・取消を表示する。拡張子だけでは file / directory が決まらない `.epub` と RAR / 7z / LZH suffix の path は、**先行 owner の退役や Ignore 拒否より前**に、非破壊の worker-side file / directory 分類を行う。分類 request は context / request ID に照合し、取消可能にする。directory と確定した path は Ignore 設定下でも通常 folder として進む。EPUB file と確定して `epub_file_handling=Ignore` なら、PDF cache 参照・warm placeholder 採用・staged preparation・履歴 stack 操作・古い grid tile の open 副作用より**前**に typed `FolderOpenOutcome::Refused(EpubIgnoredBySetting)` として拒否する。archive file と確定して `archive_file_handling=Ignore` なら同じく先行 pending owner を退役させずに拒否する。拒否では変換 state も次の worker も作らず、source の rows・selection・address・履歴・A/B と先行 owner を保つ。`NotConverted` は Ask / Convert で受け付けた要求が PDF worker から返す別の typed failure として維持する。RAR / 7z / LZH の `archive_file_handling` は EPUB 判定に使わない。受け付けた変換済み EPUB は PDF 読み取り経路へ進み、変換 modal は作らない。直接 open は `Direct(owner)`、Back/Forward/BS と Rating/Collection child replay は `StagedHistory(context, request_id)` を child continuation とする。成功した staged conversion は同じ ID で PDF preflight へ戻り、可視採用前に履歴・A/B を変更しない。Remote は別 session の読み取りのみで変換を開始せず、未変換を PC で開くよう案内する。

右クリックの複数 EPUB「PDF ファイルに変換」は **既存 `EpubBatchPending` の独立した `egui::Modal` を維持**する。単冊 open の論理 `.epub` destination / history continuation を持たず、sibling PDF 保存と一覧更新を行う操作だから単冊ダイアログへ合流させない。RAR の一括 ZIP 変換と同じく全 batch の処理中・結果確認中は背面 open を遮断し、取消は現在の一冊が終わった後に停止する既存規則とする。batch 開始時に同じ context の未採用 direct open または staged transition があれば開始を拒否し、古い要求の completion が batch 中に表示を差し替えないようにする。batch 完了の一覧更新は modal が所有する通常の refresh とし、外側 history を書かない。

UI の modal guard だけでは非同期入口を覆えない。単冊 EPUB / batch **または typed `PdfPasswordRequestOwner` の prompt** が生きている間は、**新しい local open を受け付ける全 admission 境界**でも拒否し、どの cancel / rollback / worker spawn より先に判定する。EPUB 変換自身の `Direct(owner)` または同じ `(context, request_id)` の内部 resume、dialog 内の Cancel / Retry / Save PDF、PDF password prompt 自身の同じ typed owner に対する Retry / Cancel だけを許す。PDF password は現在 `egui::Window` の入力 blocker のみなので、この共有 admission gate を新設して補う。`Legacy`、`Direct`、`StagedHistory(request_id)` のいずれも window lease / context ID と照合し、main、active detached、parked detached のどの prompt も新規 open によって退役させない。同じ path の再 open も新しい要求なので例外にしない。受け付けなかった入力は source、履歴、A/B、dialog owner を変えない。起動中に single-instance / ファイル関連付けの activation が届いた場合は channel から取り出して resolver を起動・旧 owner を取消す前に保留し、modal 終端後に既存の「最後の activation を優先」規則で最新の path を再検証・admit する。すでに走る resolver の completion も終端まで可視採用させない。初回 startup restore は通常この modal より先に一度だけ始まるが、遅い completion も同じ gate と owner 照合を通す。終了要求は新しい open ではなく、dialog owner を cancel して閉じる。detached の既存 worker completion はその typed context へだけ適用し、modal owner や main history を退役させない。

`epub_convert` は viewer context bundle に入るため、modal gate を投影中の `App.epub_convert` だけから判定しない。main / active detached / parked detached の typed owner を registry から解決し、どの window の入力から来た要求でも現在の単冊 modal owner を見つける。ダイアログは owner window で描き続け、window の投影切替や OS focus 移動だけで見えないまま入力を止めない。`modal_dialog_block_reason` の報告と描画、共有 admission predicate はこの同じ owner に従う。batch は App-global な別 owner だが同じ gate の排他的な候補であり、同時に単冊変換を開始しない。

したがってレビューの `S → direct EPUB A が NotConverted/変換中 → direct PDF B` は、keyboard / mouse / gamepad / toolbar / bookmark / detached の入力でも、activation / startup の非同期入力でも、**B の admission が変換終端まで成立しない**。A の cancel は A 自身の rollback で S へ戻し、その後に B を新規要求として扱う。変換中の直接 open の restore を B へ hand-off する経路は不要である。

### 12.2 D-B: warm 直接 PDF/EPUB の表示を採用として確定する

直接 PDF open は、D13 の EPUB refusal を先に通した後、released `pdf_meta` の stamp・page count・パスワード・綴じ方向条件を満たす warm hit なら、page placeholder の install を**可視採用**として扱う。rows / surface / selection / address / typed 現在地 / Back・Forward / Rating・Collection provenance / A/B の記憶先と active slot は同じ採用境界で確定する。検証列挙は裏で続け、page count 一致なら一覧を再構築せず cache を更新し、異なればその**採用済み PDF 地点の**ページ一覧を修正する。正常な cached EPUB も固定世代 `(generation_id, pdf_size)` と方向条件が一致すれば同じ PDF 経路の warm hit とする。未変換・未固定 EPUB、方向確定待ち、password 不明、cache miss では placeholder を出さない。plain PDF では released requested-file metadata 読取を維持し、それを超える UI-thread file / DB probe を足さない。

placeholder を出した時点で旧地点への rollback は**消費済み**であり、列挙失敗・パスワード要求は現在の PDF 地点の明示的な error / password 処理に入る。旧 rows へ戻すための部分的な snapshot は作らない。対して cold 直接 PDF/EPUB は列挙成功まで旧 rows・surface・selection を保ち、早期に変わる address / history 意図等は typed restore が所有する。失敗・Cancel なら元の表示・address・stack・A/B へ復元する。EPUB が `NotConverted` ならその restore を modal child が引き継ぐ。

warm PDF の検証列挙 pending は **採用済み source の owner** であり、後から staged history を受け付けても admission では cancel しない。staged transition は確定した PDF 地点を source として捕捉する。検証結果が staged preflight 中に到着したら、同じ pending owner が結果を保持し、staged が失敗・取消なら source PDF に照合・反映する。staged が成功した時だけ、その採用境界で source の列挙を退役させる（同じ PDF を開く場合は新 waiter を登録してから旧 waiter を外す）。これで source の rows generation を preflight 中に勝手に進めず、staged の source guard と競合させない。通常の leave / close も pending owner を退役させ、遅い completion は採用できない。

新しい **cold 直接 B** を受け付けた場合も、既に可視採用した PDF A の検証 owner を B の可視採用まで保持する。A と B は異なる要求であり、B の列挙・password prompt・EPUB conversion を A の検証 owner と同じ pending slot で上書きしない。per-context の document-open owner を `CommittedVerification(A)` と `CandidateDirect(B, retained_source_verification: A)` のような排他的な相に整理し、同じ要求の二重 owner や独立した `pdf_enumerate_pending` field を増やさない。B が失敗・取消なら A の検証結果を A の地点へ反映し、B が成功採用されたら A を退役させる。新しい B が warm hit で即採用されればその境界で A を退役させる。B の未採用中に A の結果が届いても staged の場合と同じく表示更新を保留する。A が正常な通常の leave / close を受ければ A を退役させる。

### 12.3 D-C: supersession / rollback 行列

表の「直接」は `Navigation`、`RatingPhysical`、`QuickFolderSwitch`、`CollectionGridPhysical`、`MainGridArchive`、`Bookmark` と、その context に解決された startup / activation を含む。「staged」は物理・Collection child・Rating entry/replay・Back/Forward/BS の準備要求を含む。列はいずれも **同じ viewer context の、scope・worker 分類・該当する Ignore 判定を通過して実際に受け付けた新要求**。分類待ちは候補 request にすぎず、先行 owner を退役させない。拒否された scope / modal / Ignore / 古い request ID / worker 開始失敗は supersession を起こさない。stale grid tile からの open も同じ分類と refusal を選択・検索 return・fullscreen reservation より先に通す。

| 既存の in-flight / source | 新しい直接 open | 新しい staged open |
| --- | --- | --- |
| 未採用の cold 直接 PDF/EPUB 列挙（`NotConverted` 到着前） | admission で旧 owner を cancel、旧 typed restore を**一度だけ復元**してから新要求の source を捕捉する。新要求の失敗・取消は旧直接 open より前の地点へ戻る | 同じ順序で復元してから staged source snapshot を取る。失敗・取消で旧地点を維持する |
| `PdfPasswordRequestOwner::{Legacy, Direct, StagedHistory}` の password prompt、直接／staged EPUB の inspect・convert・save・error modal、EPUB/RAR batch modal | **admission 不可**。所有 prompt の Retry / Cancel、変換 dialog の Cancel / Retry / Close / Save PDF または同一 typed continuation だけ動く | **admission 不可**。Back/Forward head と A/B を消費しない |
| warm placeholder を可視採用した直接 PDF/固定済み EPUB の検証列挙 | 採用済み本を source とする。cold 直接候補 B と source 検証 A は異なる要求として同じ typed document-open owner に収め、B の可視採用または明示 leave / close まで A を維持する。B 失敗・取消なら A の結果を照合・反映する | 採用済み本を staged source とする。旧検証を admission で cancel せず、staged terminal まで結果を保持。staged 成功採用時に退役、失敗・取消なら PDF view を更新する |
| 物理・Collection・Rating の staged preflight / replay（可視未採用） | admission で旧 request ID を退役し、旧表示・履歴を source として新要求を準備する。旧 late result は無視する | admission で旧 ID を退役し、旧表示・履歴から新 ID を準備する。Back/Forward pop は勝者の成功採用時だけ |
| startup restore / activation / bookmark の path resolver、folder pane scan、A/B switch の未採用候補 | admission と同じ context ID / request ID で旧候補を退役し、旧表示・履歴を保持する。scope 拒否・worker 開始失敗なら旧候補を保持する。activation の「最後の要求」は解決後の scope admission まで先行候補を破壊しない | 同じ規則。古い resolver / scan completion は新しい staged owner を採用・cancel できない |
| 既に走る DFS folder navigation と queued steps（fullscreen の folder Back/Forward を含む） | 受け付けた EPUB open は同じ context の DFS owner を admission で退役し、pending result と queued steps を cancel / drain して lock を解放する。変換 modal 中に届いた旧 completion は owner ID 不一致で捨て、modal 後にも再実行しない | 同じく accepted admission で旧 DFS を退役し、staged request 自身の DFS continuation だけを保持する。拒否・worker 開始失敗なら旧 DFS を維持する |
| 同じ context の ZIP/RAR 等の未採用 open / conversion | 既存の typed owner と modal の規則に従う。modal 中は admission 不可。非 modal の旧準備は採用済み source を変えず退役する | 同左。ZIP 内部移動は外側 history を増やさない |
| in-flight 無し／前要求が成功採用済み | 採用済み現在地を source として通常の直接 open | 採用済み現在地を source として staged preflight |
| **別 viewer context** の pending（main 対 detached、detached 同士） | 所有 context のみで処理。sibling の pending、rows、history は触らない | 同左。detached は main の Back/Forward owner にならない |
| Remote の独立した page / book request | Remote session を変えず、local current と history のみを新要求の成功採用時に変更する | 同左。Remote の未変換 EPUB は変換を起こさない |

入力経路ごとの admission 判定: keyboard（検索/Fullscreen を含む）、mouse の tile・address・folder pane・Extra1 / Extra2、browser Back / Forward、ring / gamepad、toolbar、A/B、bookmark は可視 modal と同じ gate に入る。mouse Extra1 / Extra2 と browser Back / Forward は最終的な共通 Back/Forward dispatch と DFS 開始の両方で gate を通し、既に発行済みの DFS result は上表の owner 照合で捨てる。pane scan は worker spawn より前に gate を通し、候補 scan / refresh と明示 navigation を区別する。detached は window lease から destination context を決め、main に投影中という理由で main の staged owner を退役させない。single-instance / ファイル関連付け activation は modal 中に保留し、解除後に resolve・scope 判定する。startup restore は初回 owner の completion を同じ gate で照合する。Remote は App の local open admission を使わず別 session で閲覧し、EPUB 変換も local history 書き込みも行わない。Remote の **local control acquisition** がある場合は通常の main context admission として modal gate を通す。新要求の未採用中に別 context の worker が終わっても、各 context ID と request ID で照合して sibling を変えない。

実装時は、scope / owner 有効性確認 → 共有 modal admission → suffix が不確かな path の cancellable worker 分類と request ID 照合 → 確定 kind に対する D13 / archive Ignore refusal → 新 worker / preflight の開始可能性を確認 → **accepted admission** で同一 context の未採用直接要求および先行 DFS owner を一か所で settle → 新 source 捕捉 / request 登録、の順とする。分類待ち、拒否、開始失敗は旧 owner を退役させない。warm 採用済み owner は settle の対象外で、source verifier として次の候補へ渡す。現行 `retire_direct_document_open_for_history_admission` の二つの history 専用呼出しを、直接→直接にも共通のこの境界へ移す。

**削除する supersession hand-off** は、別 open の admission が変換 modal 中には成立しないことを前提とした直接 EPUB 変換 restore の次要求への受け渡し、`replace_epub_convert_state` の旧変換 restore 継承、通常 open / pane scan が旧 conversion を退役して `inherited_restore` を新要求へ渡す経路、warm owner に残る旧地点 rollback、history admission 時の無条件 `pdf_enumerate_pending.take()` / placeholder clear である。`finish_epub_convert(Superseded)` の**呼出しを一律に消さない**。同一 owner 内の置換や window close / stale completion の棄却など、旧変換の結果を採用させないための呼出しは owner ID と terminal 理由を確認して残す。pane scan 自身の取消・replaced-result 所有も必要なので、`PaneOpenRestoreExit` 全体を機械的に消さず共通 admission と整合させる。**残す所有・復元** は cold PDF/EPUB の typed restore と Cancel rollback、単冊 dialog の Cancel 復元、staged child の context / request ID、warm source verifier と late-result guard、同じ path の waiter 合流である。`Option` の有無で未採用/採用済みを推測せず、排他的な owner の相で表す。

### 12.4 実装時に必要な回帰テストと smoke

1. Ask / Convert の直接・staged EPUB で、inspect / progress / save / error の全相に dialog と `epub_convert` block reason が同時にあり、Cancel が source の rows・selection・address・stack・A/B を戻す。Convert は Confirm を出さず progress を出す。D13 Ignore は変換済み・未変換の両 EPUB を、cache lookup、warm adoption、staged worker spawn、古い grid tile の選択・検索 return・fullscreen 副作用より前に `Refused(EpubIgnoredBySetting)` として返し、先行 owner と source を保つ。Ask / Convert の `NotConverted` は別の typed result として残す。`archive_file_handling` の変更が EPUB の拒否に影響しないことも確認する。既存の実 EPUB fixture を使い、staged の Rating/Collection/Back/Forward/BS と Save PDF の独立 destination を再確認する。
2. modal 中の keyboard、mouse、ring/gamepad、toolbar、A/B、bookmark、detached open が新 owner を作らず、直接 EPUB A → 直接 PDF B が成立しない。PDF password の `Legacy` / `Direct` / `StagedHistory` owner でも main・detached の新 open、activation、startup completion を拒否し、同じ owner の Retry / Cancel だけが進むことを検証する。activation は解除後に最新 path を一回だけ admit し、modal 中は resolver / 旧 owner を退役させない。遅い startup/bookmark completion と同一 continuation の resume を検証する。
3. RAR 式の EPUB batch dialog は開始から結果を閉じるまで block reason を返し、Cancel after current、同名 PDF skip、一覧 refresh を確認する。既存の未採用 direct / staged がある時は batch を開始しない。
4. warm `pdf_meta` hit の直接 PDF と固定済み EPUB を実 load し、placeholder の初回 frame で rows・surface・selection・address・history・Rating/Collection provenance・A/B が一致することを確認する。cold miss、未固定 EPUB、綴じ方向待ち、password 不足では placeholder を出さない。PDF count 一致/不一致、失敗/password、後着 result を検証する。性能は `epub_open.first_display` と PDF の同等計測で warm 初回表示を再測定する。
5. warm PDF → staged history または cold 直接 B の admission → B 失敗/取消では PDF view と source 検証 owner を残し、結果が途中で到着しても terminal 後に反映する。B 成功時は一度だけ source 検証を退役し target を採用する。Back/Forward head と A/B は staged の成功まで動かさない。cold PDF A → direct B と cold A → staged B は、B の失敗/取消で A 前の表示・address・stack・A/B へ戻る。新しい各動作テストは変更前に失敗することを確認する。
6. main staged pending 中の detached cached archive / bookmark open、別 detached completion、Remote 閲覧は main を変えない。main と detached の sibling invariance、検索中の履歴拒否、ZIP/PDF 内部移動と BS を維持する。
7. 先行 DFS folder navigation の pending result / queued steps がある時に EPUB open を受け付け、変換中に旧 result を配達しても表示・履歴・dialog owner が変わらず lock が残らないことを確認する。拒否された EPUB open と worker 開始失敗では旧 DFS を維持する。mouse Extra1 / Extra2、browser Back / Forward、keyboard、gamepad の入力は modal 中に共通 gate で止まり、変換終了後だけ新しい DFS / history を開始できることを確認する。
8. `*.epub` と archive suffix の実 directory は Ignore 下でも folder として採用する。listed EPUB directory と、EPUB tile だった file が worker 分類前に directory へ替わった stale-grid case を分けて確認する。EPUB Ignore の実 file、および未採用 direct open が pending の間に来た archive Ignore の実 file は、旧 owner・rows・selection・address・stack・A/B を一切変更しない。古い分類 result と取消済み request が後着しても採用しない。

無人 `FolderHistory` portable smoke には、fixture PDF の warm 再 open → placeholder 表示と Back/Forward、staged 失敗/取消時の保持を、test-script が安定した初回表示 checkpoint を観測できる範囲で追加する。EPUB 実変換は WebView2 と dialog の Cancel / progress 操作を要するため無人 smoke へ入れず、利用者が承認する対話的検証 session で Ask、Convert、Ignore、変換中の別 open 拒否、変換取消後の履歴、固定世代の warm 再 open、batch Cancel を確認する。実アプリを agent が起動する前には既存の明示承認 gate に従う。

review #8 で `FolderHistory` test-script に warm direct PDF の実入力 step を加えた。
`pdf_warm_adoption_phase=CommittedPlaceholder`、sequence と path は、
`CommittedVerification` の placeholder を
`start_loading_items` で可視採用した時だけ進む durable 診断 checkpoint で、worker の
完了が速くても script は採用した事実を読める。Rating PDF を一度読み、物理 F の tile
から Enter 入力で再 open して checkpoint の増分・path と Back / Forward / BS を確認する。EPUB の
WebView2 変換は引き続き対話的 session の対象である。

### 12.5 Detached owner servicing と warm stamp の review #8 修正

review #9 後の設計担当決定は、既存の park 契約を維持する。main / active detached の
mounted context だけが typed registry の `service` で async owner を進める。
parked still は frozen frame で、root から一時 mount して poll / dialog 描画しない。
park transaction は同じ registry の `terminate_on_park` を通し、分類 candidate、
staged 履歴、Collection grid/navigation、Rating navigation、bookmark open、DFS、
detached folder scan、PDF/ZIP enumeration、EPUB 変換、PDF password request を terminal
にする。fullscreen decode、AI/edit preview と Similar preview も既存の park
取消を registry に集約する。staged request の取消では旧表示・履歴を維持し、未採用 direct cold open は
所有 rollback を返す。取消済み worker の late result は request ID / generation で捨てる。
root の `poll_parked_document_open_owners` と parked dialog の描画は廃止する。
Collection grid の park terminal は Snapshot / Preparing の worker に限定する。
Ready / Empty / Failed / Deleted と、未開始の RequestNeeded は変更しない。特に
Deleted の物理子は tombstone を保持し、park / resume 後も BS と restore が物理 path を
返す。Similar preview は進行中の preparation と保持 gesture を終了するが、完成 cache と
terminal failure は保持する。どちらも「settled state を park で新規 request にしない」規則に従う。

modal owner（EPUB 変換、PDF password）がいずれかの context に存在する間は、別 viewer
context の activation を共通 gate で拒否する。passive click、activation watcher と
deferred commit、keyboard/gamepad の window switching、grid からの既存窓再利用はこの
境界を通る。taskbar / tray の root 復帰は viewer context の切替ではない。
PDF password の可視性と Retry / Cancel は context-local typed request から、active
context 優先・残りは安定 ID 順で一件を選ぶ。選択 request の terminal 後は次の request
が可視になり、request がなくなれば gate は外れる。active detached viewport は自分が
選択 owner の時だけ prompt を描く。入力文字列は dialog の逐次 UI 値であり、visibility
flag として使わない。park で modal を終了するのは既に開始した内部 terminal 処理または
テストの強制 park だけであり、通常入力から modal owner を park できない。
bundle の他の cache / write pending は、park 後の resume・DB 永続化契約が異なるため
既存の mounted frame / worker owner に残す。registry の audit test は bundle の
`*_pending` field を登録済み owner と明示した cache/write field に分け、新しい field
が無分類で追加された場合に失敗する。
multiwindow Collection の旧「parked request が残って再開時に採用される」回帰期待は撤回し、
park 時に request が terminal となり、復帰時も最後に表示済みのページを保つ期待に更新した。

plain PDF の warm `pdf_meta` 照合 stamp は master の released direct-open と同じ
requested-file `std::fs::metadata` 1 回から取る。これは既存 UI-thread コストであり、
direct 分岐の既存 `path.is_file()` 判定も含め master と同じ費用で、分類用の
新規 directory scan や追加 stat は行わない。現在 rows に PDF がない
address / activation / bookmark open も同じ immediate placeholder 採用となる。
`Navigation` の plain PDF は ZIP/EPUB の staged 履歴分岐から外し、cold 時は既存
`ColdCandidate` の source/rollback owner、warm 時は同じ direct adoption とする。
grid の PDF tile から開く場合、読み履歴の戻り先・rating/facet filter 抑制などの
`GridVirtualOpenEffects` も cold では `DirectPdfAdoption` に預けて列挙成功時の可視採用で
確定する。warm は placeholder 採用後に確定し、失敗・Cancel では source の効果を保持する。
Back / Forward / BS と Rating / Collection replay の staged 分岐はそのまま保持する。

fail-before 記録: §12 初回実装では `epub_convert_dialog_visible()` を Convert mode の
旧条件へ戻すと `convert_mode_scanning_keeps_visible_modal_and_input_owner` が失敗し、
warm placeholder 採用を外すと `warm_pdf_direct_open_commits_placeholder_and_history_failure_keeps_source`
が失敗した。PDF password gate を外すと `pdf_password_prompt_blocks_new_direct_open_but_its_retry_proceeds`
が失敗し、suffix worker 分類を外すと `ignored_epub_stale_tile_that_became_directory_is_not_refused`
が失敗した。review #8 の追加テストでは、parked context の poll を外すと
`detached_suffix_directory_input_is_classified_and_adopted_while_parked` が失敗し、
requested-file stamp fallback を外すと
`warm_pdf_direct_open_absent_from_current_rows_adopts_placeholders` が失敗する。
実際の review #8 mutation run では、parked poll を空に戻すと
`detached_epub_conversion_parked_dialog_progress_cancel_and_completion` は dialog 未描画、
`detached_suffix_directory_input_is_classified_and_adopted_while_parked` は未採用のまま
timeout した。active loading viewport の EPUB dialog 呼び出しを外すと
`detached_epub_conversion_is_serviced_in_active_viewport` は area 未登録で失敗した。
一覧外 PDF の stamp を row-only に戻すと
`warm_pdf_direct_open_absent_from_current_rows_adopts_placeholders` は旧 source に留まり、
plain PDF Navigation を staged 分岐へ戻すと
`warm_pdf_navigation_absent_from_rows_adopts_before_worker` は旧 source に留まった。
review #8 の parked-poll fail-before はその時点の設計に対する履歴記録であり、
review #9 の park terminal 契約では期待結果を逆転した。新規テストは pending owner を
park した時点で terminal になり、parked update が何も進めないことを検査する。
全 lib gate では旧 staged 前提の PDF テスト 2 件と旧 placeholder 前提の detached
binding テスト 1 件が失敗した。cold direct の grid 効果を早期確定するコードを
`DirectPdfAdoption` への預託に変え、前者は直接 PDF worker の成功・password Cancel を
通すテストへ更新した。後者は released warm placeholder の即時表示を検査する。

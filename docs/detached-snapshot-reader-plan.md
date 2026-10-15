# 独立readerの受動更新停止（C1採択）と案Cの影響調査

2026-10-15。比較基点はリリース済み`v4.6.0`（`bb6ca0c66`）と作業ツリーHEAD
`ddcc0166d`。以下のmasterはこのHEADを指す。コードとgit履歴の読み取り調査であり、
以下の比較は実機再現なしのコード／履歴調査。C1の実装・自動検証結果は§9へ記録する。
既存の[案A（§1.375 §9）](section375-operation-reconcile-plan.md#9-検索終了後の独立readerと物理更新の所有境界案a設計)は保持する。
**現在の採択はC1だけ（2026-10-15利用者決定）。案Aは不採用の履歴として残し、source／main-owner境界の
考え方だけを参照する。案Aの「readerも観測・保留する」契約は適用しない。C2／C3は今回実施せず、linked中の挙動も変えない。**
独立再レビューはP1:0／P2:1／P3:0で、整理完了の能動分類以外は承認、BA-7に沿う構造修正にも同意済み。
残る整理完了の分類を§4.5へ追加し、2026-10-15利用者の指示でC1実装・検証へ進む。
以下の将来C2／C3は対象外。検証結果は実施後に§9へ追記し、旧設計・過去の結果と区別する。

## 1. 結論と固定する範囲

**全別窓を開いた時点からsnapshotにする完成案Cは、watch停止だけでは済まない。**
通常F12のlinked窓はmainと同じcontext／itemsを使うため、mainの追従を保つにはopen時点の所有分離が必要。
さらに静止画・仮想ページの読込失敗は現在自動skipしない。
今回のリリースには§6の**C1（既に独立したreaderとpromote後の受動的観測だけを止める）**を入れる。
通常F12等のlinked中のmain観測・選択連動・最初のpromote、能動操作の既存終端／保留は維持する。
open時の所有分離（C2）・missing skip拡張（C3）は行わず、以下の全案Cの比較は将来検討の記録とする。

snapshotは**そのreaderが採用したitems／基本順序・page identity・ProjectionRecipe**であり、
ファイルbytes、全画素、DB値の複製ではない。次を区別する。
今回C1で固定するのは独立readerの受動FS更新だけであり、能動操作の既存保留配送や読込失敗の動作まで変えない。

- 実ファイルの追加・削除・改名・上書きによる**自動列挙／一覧差し替え**はしない。
  新しいファイルは既存別窓には入らず、旧pathは勝手に新pathへ移さない。
- 遅延decode・cache eviction後の再読込では、その時点の実ファイルを読む。
  同じpathの内容が変われば新しい画像が見えることはある。「開いた時点の画素を永久に保存」は保証しない。
  ZIP／PDF／変換済cacheの既存source identity・epoch・leaseによる安全な採否も弱めない。
- ★・タグ・編集・しおり・構成の提示許可など、mIV自身の操作／既存publication／明示importは維持する。
  同じ行の表示値、facet後の`visible_indices`、見開き／連結は変わり得る。新しいFS行を取り込むこととは別。
- 明示open・明示reload・reader内の明示folder移動は**新しいsnapshotの採用**として残す。
  同じfolderへの復帰だからという理由で、勝手に自動再走査はしない。
  key操作は既存KeyActionとaction surfaceのownerへ配送し、新しい固定キーを足さない。
- mIVで依頼したDelete／rename／Undoのhandle解放・purge・操作成功分の採用は残す。
  「外部で消えた項目をskip」と「mIVで削除する前に対象窓を閉じてhandleを解放」は別の契約。

## 2. v4.6.0とmasterの挙動・根拠

行番号の変動を避け、以下は`file:function`で記す。v4.6.0は`git show v4.6.0:<file>`で照合した。

### 2.1 一覧所有と更新

| 経路 | v4.6.0の公開済み動作 | masterの差分／根拠 |
| --- | --- | --- |
| 通常F12（画像／本／動画・音声） | 通常linked窓はmainと同じbundle／items。F12自体は表示先変更であり一覧forkではない | 同じ。`app.rs:toggle_detached_viewer_mode`／`prepare_viewer_presentation_open`。linked／independentの正本は`detached-viewer-implementation-plan.md §3.0` |
| 独立静止画／ZIP／PDF窓 | always-new設定等では専用contextを持つ。mainの選択・folder変更でitemsを入れ替えない | `app.rs:route_materialized_physical_still_open_to_active_context`、registryの`split_current_context_preserving_main_grid`／`split_materialized_physical_context_for_detached_scope`。後者は`DetachedPhysical`で当該物理source内へ限定。descriptorから初回列挙する別入口もある |
| メディアを別窓で開く設定 | 動画／音声は単一のメディア窓を再利用。静止画always-newと同じく常に独立contextを作るとは限らない | `app.rs:prepare_viewer_presentation_open`／`sync_detached_viewer_to_selected`。`effective_media_in_media_window()`ではmain選択からメディアへ追従しないが、itemsのownerまで独立したという意味ではない |
| main切替時のメディアpromote | 元mainのbundleをreaderへ退避しfresh mainを作り再生を維持。前後移動は退避した一覧内 | `app.rs:promote_active_detached_video_for_main_context_change`→registryの`become_mounted_independent_detached_viewer`／`stash_mounted_and_start_fresh`。navigation scopeはMainのまま残る。live-parkのforkとは異なる |
| watch／scan／保留の所有 | `current_folder_watch`／`external_rescan_pending`／`folder_refresh_pending`はApp-global。独立bundleの`ContextAsyncOwner`に観測serviceはない | §1.375 `a73ea3750`からbundle所有、`ContextAsyncOwner::FolderObservation`がmount中の独立contextもserviceする。`ensure_current_folder_watch`は登録直後に初回`UNIX_EPOCH` scanも要求。独立readerへのこの自動観測は未リリースの追加 |
| 実差分を見つけた場合 | `app.rs:apply_external_rescan`はmain fullscreen中を保留するがlinked detachedは保留しない。main一覧更新と必要なpromoteへ進む | `apply_external_rescan_with_cause`は`DetachedPhysical && fullscreen_idx`も保留。promote済み元Main scopeのreaderはここに入らず、今回の二度目のpromoteへ到達する |
| Equal／Different・一覧復帰 | signature一致なら更新を省く。保留を`consume_folder_refresh_pending`でreloadへ戻す | §1.375は行記録のEqualでinstall／世代／thumb再要求を省く。External／Append／本folderの保留はfresh rescanへ戻る。`check_external_folder_changes`／`start_external_rescan_with_cause`／`poll_external_rescan`に通知・Resumed・restartが合流 |
| linkedとmainの同期 | main選択・sort／filter後の選択補正・folder移動に窓が連動。viewerの送りもmain cursorへ反映 | `app.rs:sync_detached_viewer_to_selected`／`sync_main_selection_from_viewer_idx`。同じindexが同じ行であることを共有bundleが支える。独立化後にraw indexを別contextへ渡すことはできない |
| export等による更新 | main一覧表示中のCtrl+E出力を順次一覧へ増やす（2026-09-05利用者判断） | `note_exported_file_for_folder_refresh`／`consume_folder_refresh_pending`とroot update consumer。linked窓があるからmainも観測停止、という実装ではこの公開済み動作まで失う |

`ViewerContextTable::plan_promote`はmainだけがpromoteするassertを持つ。
`active_detached_media_requires_main_context_transfer`はv4.6.0からMountedだけを見てmain identityを見ない。
masterの非main観測serviceが、その潜在的な所有違反へ自動で到達するようになった。
mount自体は正常であり、非mainをmountしない案・assertを弱める案は採らない。

### 2.2 metadata・sidecar・明示mutation

| 対象 | 現在の経路と案Cで残すもの |
| --- | --- |
| `mimageviewer.dat`等の初回取込 | `sidecar_restore.rs:begin_sidecar_restore`／`apply_sidecar_restore_live_context_effects`。items identity準備・tag facet・first-display continuationを同ownerが完了する。初回取込と進行中の継続は残す（v4.6.0にも存在） |
| 外部sidecarの変更 | v4.6.0は通常reloadへ進めば設定どおり取込が走り得る一方、signature一致等でreloadしない場合もあり、sidecarだけの追従保証ではない。master R10は自動originの取込を明示的に禁止済み。明示reload／開き直しの取込は残す |
| ★・編集の窓間反映 | `app.rs:publish_current_context_rating_writes_with_intent`／`sync_current_context_rating_session_writes`、registryの`publish_mounted_context_rating_writes_at_boundary_with_intent`。編集は`poll_page_edit_reconciliation`。基本経路はv4.6.0にも存在。HEADはさらにregistryの`publish_accepted_adjustment_page_edits`／`publish_accepted_local_page_edits`からAtRest／Retiringへ直接publicationする。snapshot化を理由に止めない |
| タグ・しおり・明示metadata import | `tag_ops.rs:poll_tag_write_results`、`app.rs:notify_bookmarks_changed`／`poll_bookmark_presence`、`take_metadata_import_refresh_requests`／`apply_metadata_import_terminal_refresh`／`apply_current_metadata_import_terminal_result`。mounted／parkedのcache・facetをowner／世代照合で更新。v4.6.0にも存在。通常の不完全な窓間反映を本件で新しく保証するものでもない |
| 表示中対象のDelete／rename／Undo | `app.rs:release_viewer_surfaces_for_removed_paths_with_metadata_boundary`。`start_delete_files`はworker前、`ui_dialogs/rename_item.rs:poll_rename_pending`は成功後、HEADの`book_append_undo.rs:try_undo_book_append`もこの解放経路を使う。表示中の対象窓は閉じ、非対象兄弟は保持。未表示行がsnapshotに含まれるだけでは追従しない |
| rename後のDB整合 | `app.rs:rehydrate_contexts_after_rename_migration`が該当contextのedit／rating／tagを再hydrateする。v4.6.0にも存在。ファイル一覧の自動再列挙とは分離し、削除しない |
| 本追加・Undo | §1.373はv4.6.0後の追加。`app/book_append_refresh.rs:finish_book_append_terminal`／`release_book_append_observations`と`app.rs:poll_book_op_pending`。metadata・presence・receiptとmainへの配送を残し、reader向け一覧採用だけを省く設計が必要（§4.3） |

### 2.3 消えたファイル／読込失敗

| 対象 | 現在の動作・根拠 |
| --- | --- |
| メディア窓の手動前後送り | `app.rs:media_navigation_candidate`／`collect_manual_media_navigation_candidates`／`resolve_media_navigation_candidates`／`start_manual_media_navigation`。Image／Video／Audioの実pathを既存`MediaNavigationResolver::spawn` workerで`Path::exists`検査し、欠損候補を飛ばす。v4.6.0にも存在 |
| 静止画の手動送り | `ui_fullscreen.rs:handle_fs_navigation_with_render_perf`はmedia以外では隣indexへ直接着地。`app.rs:start_fs_load_with_purpose`→`FsLoadResult::Failed`→`poll_prefetch`→`FsCacheEntry::Failed`になり失敗画面を表示し、自動skipしない。`fs_page_load_state`／`ensure_fs_page_load_with_contract`により毎frame無限retryはしない |
| スライドショー | `ui_fullscreen.rs:advance_slideshow`／`current_slideshow_frame_ready`はFailedもreadyとみなし、通常の表示間隔後に次へ進む。即skipではない |
| 動画／音声の連続再生 | `handle_video_continuous_eof`／`handle_music_continuous_eof`も候補workerで欠損実pathを飛ばす。候補なしなら末尾停止、wrapで自分だけ残れば同曲loop。decode／native init errorはEOFではなく`video/mod.rs`のplayer errorへ入り、`app.rs:poll_video`から一律次へは進まない |
| 存在確認の限界 | 検査は`is_file`やopenではなくexists。検査後に消える・stat可能だがread不可・decoder failureは別の読込失敗。動画→音声モードのswap不能時は`apply_video_audio_mode_continuous_eof_target`で現在項目をloopする場合もある |
| ZIP／PDF・変換済source | 既存メディア候補ではvirtual entryのpathはNoneで存在確認しない。実decode／renderに失敗すればFailed。既存texture／retained PDF raster／変換cacheで読める場合は、元containerが消えていても表示できる。単純な外側pathの事前existsで全てskipすると、この閲覧を失う |
| 初回target不在 | `app.rs:handle_deferred_fs_open_outcome`の`RequiredTargetMissing`、構成の提示許可終端は、初回の明示targetが無ければsessionを閉じる。別の先頭ページへ勝手に着地しない契約。snapshot採用後の送りと区別する |

したがって「既存の読込失敗処理だけで、全種類のmissingを即skipできる」は誤り。
一方で既存のmedia候補worker・context別load ownerは使え、別のファイル監視／retry ownerを作る必要はない。

## 3. 利用者に見える変更

**C1で変わるのは独立済み／promote後readerの受動観測・再採用だけ。**
linked窓、明示操作の終端・必要な保留、既存missing処理は維持する。以下はC2／C3も含む完成案Cとの比較表であり、
全行を今回のリリースへ適用する表ではない。

| 場面 | 案C完成時の仕様／残す動作 |
| --- | --- |
| 別窓表示中に同folderへファイル追加 | その窓の前後送り・スライドショー・連続再生には追加分が現れない。main一覧には従来どおり現れる |
| 外部で削除／改名 | 旧行・旧順序は残るが、新たに表示しようとして読めなければ次候補へ進む。新名を自動探索しない。表示／再生中の開済handleやcacheがあれば、そのまま見えることもある |
| 外部上書き | 自動reload／順序再構築をしない。既にdecodeした画素は保持され得るが、後の通常decodeで変更後内容が見える場合はある |
| 本にページ追加・container更新 | 開いた窓の一覧には自動追加しない。明示reload／開き直しで新snapshot。元sourceの安全な再読込・stale拒否は維持 |
| 検索を閉じた後のreader | 検索結果の旧一覧を維持し、検索前folderの一覧へ化けない。Ctrl+R／Ctrl+G共通 |
| 元folderへ一覧復帰 | readerの外部変更保留を自動flushする機能は廃止。mainへ戻る操作で現mainを旧snapshotに巻き戻さない。通常open／明示更新で再列挙する |
| 明示reload／reader内folder移動 | 対象ownerで新snapshotを採用する。mainのCtrl+F5、ZIP内prefix保持更新、exportの逐次反映は変更しない |
| ★・タグ・補正等 | 現在保証されている保存・publication・明示importを維持。facetで見える行が変わることはあり、metadataを開いた時点へ戻さない |
| mIVで対象ファイルをDelete／rename／Undo | 現在表示中の対象窓を閉じるhandle解放は維持。外部削除時のskipとは区別する。mainのDelete→EqualとStack対応表も維持 |
| 通常linked F12 | mainと同じitemsを共有する実装は使えなくなる。既存の選択連動を無条件に廃止する承認とは扱わず、snapshot内の同じpage identityへの連動は保つ設計（§4.4）。snapshot外の新行へは明示openで新snapshotを作る |

上の外部非追従は利用者が許容済みの仕様方向。main連動全廃、folder移動禁止、編集やUndo無効化は
追加の機能削除となるため採らない。manual更新時は「一覧snapshotであり画素／metadataの固定ではない」ことも書く。

## 4. 案Cの所有設計

### 4.1 新stateを足さず、既存itemsを唯一の列にする

専用`SnapshotState`やsnapshot用itemsコピーを新設しない（既存の★固定等のSnapshotStateを転用もしない）。
readerのViewerContextBundleが持つitems／page identity／ProjectionRecipeとStack対応表をそのまま採用済み列として使う。
独立窓は既存fork／descriptor materialization、promoteは既存bundle全体の移譲を使う。
初回列挙・初回metadata hydrate・password／PDF／EPUBの準備完了はsnapshot成立に必要であり、停止しない。
「瞬間」は既存open要求がそのsourceの一覧を採用する境界、promoteでは旧bundleの所有移動確定点を指す。
directoryの全ファイルを同時刻に撮影したり、ファイル内容を複製する新しい仕組みは作らない。

採用後の受動的FS観測は**mainだけが所有**する。
registry main identityとwindow binding／既存reader lifecycleから資格を導出し、設定値・navigation scopeや
単なるMountedをmainの代用にしない。mainを一時離れてreaderをmountしても資格は変わらない。
Closingでsessionを消したがbindingが残る期間も同じreaderの終端として扱う。
観測のsource資格とツールバーのreload配送先は区別する。★固定rootから開いた物理子は
復帰先surfaceがSnapshotのままでも、採用済みrecipeと既存の実パス判定で物理sourceと判断する。
OrdinaryListのドライブ／閲覧履歴／しおりrootは既存synthetic path判定で除外し、UI flagsへ推測を戻さない。
watch登録可否（本棚配下の除外・初回stamp等）は既存登録入口の条件を維持し、
mainの既存Append観測記録まで止める条件として流用しない。

### 4.2 消す更新経路と残す経路

- 独立readerのwatch新設・Notified／Resumed・初回確認scan・自動restart・外部差分保留をなくす。
  `ContextAsyncOwner::FolderObservation`のreader serviceだけを止めて終わりにせず、
  `ensure_current_folder_watch`／`check_external_folder_changes`／`start_external_rescan_with_cause`／
  `poll_external_rescan`／`consume_folder_refresh_pending`でも同じ所有資格を使う。
- promote／forkの確定境界で、readerに移った**受動的観測資源だけ**を既存cancel／retirementへ渡す。
  main時代の先着scan・通知・restartが到着してもreader列へ採用しない。mainや別readerのwatchは触らない。
  forkではmainのwatchを残し、新readerに購読を作らない。promoteでは旧ownerの観測を退役し、新mainは通常の購読を持つ。
- readerには外部変更由来の`folder_refresh_pending`を積まず、close時にもmainへ移してflushしない。
  dirty／cutoffをreaderに残して後で追い付く必要そのものをなくす。
- `ProjectionRecipe::ExtractedResult`の物理観測禁止はmain側にも残す。
  検索closeでflagsだけが消えた区間に旧snapshotを物理folderと誤認させない。
  Archive／Documentの明示refreshとディレクトリwatch資格は分ける。
- mainの観測・§1.314・§1.375 Equal・Delete成功除去／Stack整合は現状のまま。
  main一覧置換だけが最初のpromoteを要求し、assertは保持する。
  非main結果をpromote false側の`close_fullscreen`へ落とさず、採用入口で不要な自動結果を退役する。
- 初回enumeration／decode、PDF epoch、metadata publication／sidecar初回継続、設定の明示反映、
  明示mutationのhandle解放・DB移行は残す。`ContextAsyncOwner`の全serviceやreaderの全workerを止める実装にはしない。

今回のcrashでは、promote後の0は観測資源を持たず再mountしてもwatch／初回scanを作らない。
既に開始していたscanもreaderには採用できず、二度目のmain置換要求が生成されない。
assertへ行く直前に条件を足すのでなく、snapshot readerには外部更新を発行する権限がない所有契約にする。

### 4.3 Append終端とUndoを一覧更新から分離する

**以下のreader向け能動refresh自体の廃止はC2の将来案。C1では実施しない。**
C1はAppendのmetadata／presence／receipt確定・pending終端・既存保留とmain配送を維持し、
receiptが失効している場合も§4.5の`BookAppendExternal`として能動配送を残す。
非main live readerでの能動refreshは既存reader用保留へ送り、汎用main置換・再promote・再生停止へ流さない。

`book_append_refresh.rs:finish_book_append_terminal`は、現在もfullscreen中には一覧更新を保留しつつ
receiptを確定して終端できる。完成案Cではreaderを**一覧採用不要で完了する対象**へ明示分類し、
metadata初期化・presence・receipt検証・通知・`book_op_pending`解放は完了させ、reader用保留は作らない。

`book_append_refresh_is_current`を一律falseへ変えてはならない。現在の`AppendRefreshPrepared`終端では
それが`summary.undo_files = None`につながり、成功した追加のUndoを失う。
scan開始後にpromoteした場合も、完了report／構成stamp・receiptの正当性を検査し、**一覧採用だけ**を省く。
必要なreceipt検証scanは受動watchではなく、既存Append ownerの処理として完了する。
`release_book_append_observations`はmain向け配送を残し、reader向けの自動一覧配送だけを省く。
source contextとadoption ownerを混同しない。後続mutation受付時の失効・UndoAppend排他・ごみ箱実移動・成功分purgeは保持する。

### 4.4 linked openを全snapshotへする第二段

**C2の将来検討であり、今回の変更対象外。linkedの所有／選択／F12挙動は変更しない。**

通常F12／メディア設定のmain共有を残したまま、そのbundleの観測を止めるとmainまで止まる。
完成案Cではopen／F12表示先切替の確定点で、既存fork transactionを用いてreader側items／recipe／
一時閲覧状態・playerを専用contextへ所有させ、mainには一覧と購読を残す。
動画／音声は一つのplayer／presenterを移し、二重decode・再生停止を挟まない。
これは旧案Bの「開始時からreader所有」の範囲を一部含む。案Aの小修正へ黙って混ぜない。

既存linked選択連動を残す場合は、snapshot内にある同じpage identityへ既存の明示selection/open intentを配送し、
mainのraw indexをreader indexとして使わない。snapshot外の新ファイルや別sourceへの単なる選択では列を増やさず、
Enter／ダブルクリック等の明示openが新snapshotを作る。逆方向のcursorも現在のmainにidentityがある場合だけ対応付ける。
こうした非共有でのselection routingと、F12でmain表示へ戻す際の同owner継続をレビュー対象に含める。
F12復帰を、旧readerの一覧で現在のmainを上書きする操作にしない。
既存のlinked機能自体を廃止してこの配管を省く案は、今回の外部変更非追従の許容だけでは採択しない。

### 4.5 C1改訂: 要求生成からrestartまで受動／能動の由来を保持する

改訂前（`ddcc0166d`）の`FolderRefreshCause::External`はwatch専用ではない。
`book_append_refresh.rs:finish_book_append_terminal`のreceiptなし保留、
`app.rs:note_exported_file_for_folder_refresh`の`PathBuf.into()`、
`filename_stack_ui.rs:adopt_stack_deletion`のDelete後の再準備にも使われる。
`poll_external_rescan`のrestartは`start_external_rescan`へ戻ってcauseをExternalにする。
したがって**現コードのExternalをそのまま受動として捨てる実装は不可**。
全producerを分類し直し、既存request／pending／resultが持つ一つのcauseへ由来を載せる。

#### 分類とproducer inventory

既存`FolderRefreshCause`を使用し、一般の能動refresh用にpayloadなしの`Operation` variantを一つ追加する設計。
Appendは既存の二variantを使う。新しいorigin bool、pending、owner、operation ID台帳は追加しない。
`External`の意味を受動観測専用へ限定し、receiptの有無から受動／能動を推測しない。
export等までBookAppendExternalへ寄せる案は、Append専用の配送資格まで与えるため採らない。
由来を別fieldへ重複保存せず、既存cause型の排他的なvariantで区別するのが最小の変更となる。

| 要求の由来／生成入口 | cause（設計） | C1の非main readerでの扱い |
| --- | --- | --- |
| watch登録後の初回確認／Notified／Resumed（`ensure_current_folder_watch`／`check_external_folder_changes`） | `External` | 要求しない。独立化前の受信済み結果・受動保留も退役し、再mountで復活しない |
| 整理の移動／コピー完了（`ui_dialogs/file_organize.rs:poll_file_organize`） | `Operation` | 能動。cause付きcheck／scanへ配送し、promote前後で必要な終端・保留を維持。Notifiedというcheck reasonだけで受動扱いしない |
| Append成功・有効receiptあり（`finish_book_append_terminal`／`release_book_append_observations`） | `BookAppend(receipt)` | 能動。既存終端・receipt検証・保留配送を維持する |
| receiptなし／後続mutationで失効／構成差分／readonly refresh失敗後のAppend、Undo後の一般refresh | `BookAppendExternal` | 能動。自己操作例外／Undo資格は持たないが、必要な更新配送は維持する。Externalへ戻さない |
| Ctrl+E出力（`note_exported_file_for_folder_refresh`） | `Operation` | 能動。既存の一覧復帰時更新を維持。mainの出力逐次反映も残す |
| Delete成功後の照合（`poll_delete_pending`）、Delete後のStack保持投影の再準備（`adopt_stack_deletion`） | `Operation` | 能動。成功除去・Equal／Stack整合・必要な保留を維持。検索snapshotへ物理scanを作る資格にはしない |
| `apply_external_rescan_with_cause`でmodal／閲覧中へ保留 | 受け取ったcauseを維持 | `External`だけ退役。それ以外は同owner・同sourceの既存保留へ配送する |
| 明示GridReload／通常open／明示folder移動・設定再採用 | 既存navigation／refresh intent（上記causeへ無理に変換しない） | 既存の能動入口を維持。受動観測と同一視しない |

`Operation`を加えた後の`is_append_delivery()`は、`BookAppend`／`BookAppendExternal`だけを列挙してtrueとする。
現在の「External以外ならAppend」という判定は使えない。`operation()`は有効receipt型だけを返し、
`verified_for_folder()`は`External`／`Operation`／`BookAppendExternal`を保持する。
`BookAppend`の失効は`BookAppendExternal`への一方向の降格だけで、receiptを復活させない。
能動／受動は更新の由来であり、sidecar取込許可とは別。export等をOperationへ直しても
従来の自動origin・取込省略を維持し、`consume_folder_refresh_pending`の一般elseから通常openへ落とさない。

`check_external_folder_changes`の本番呼出し元を全監査した結果は次の4入口（その他はテストfixture）。
`ExternalChangeCheck`はmtimeのふるいを使うかを表し、操作の由来ではない。

| 呼出し元 | reason | 分類／実装入口 |
| --- | --- | --- |
| `app.rs:poll_current_folder_watch` | Notified | 受動External、既存check wrapper |
| `app.rs:eframe::App::update`のmain focus復帰 | Resumed | 受動External、既存check wrapper |
| `tray_integration.rs:sync_after_restore` | Resumed | 受動External、既存check wrapper |
| `ui_dialogs/file_organize.rs:poll_file_organize`の移動／コピー終端 | Notified | 能動Operation、`check_folder_changes_with_cause`へ変更 |

整理の取消・一部成功・エラー時も従来の対象folder照合と確認要求は維持し、変更前のデータへ戻す処理は作らない。

#### 要求→保留→worker完了→restartの契約

1. **生成:** receiptなしAppendの`.unwrap_or_default()`を`BookAppendExternal`にする。
   export／Stack等の暗黙`From<PathBuf>`も能動causeを明示する。
   `start_external_rescan`（External固定）は受動producer専用とし、能動producerはcause付き入口を使う。
   `From`／`Default`で由来を省略する本番生成箇所を残さず、全生成箇所を機械監査対象とする。
2. **保留・合流:** `FolderRefreshRequest.cause`と`ExternalRescanPending.cause`をそのまま保持する。
   同owner・同folder・同走査条件で要求を集約する場合も、causeを捨てて`restart_requested = true`だけにしない。
   受動が能動を上書きしない規則で既存pendingのcauseを結合する（下表）。source／ownerが異なる要求は合流しない。
3. **worker完了:** workerが返したcauseとpendingのcauseを照合する。
   worker開始後にExternalへOperationが合流した場合、応答がExternalでも未完了の能動更新を失わない。
   ただしworker未検証のreceiptを応答へ足して自己操作扱いに戻すことはせず、必要ならBookAppendExternalへ降格する。
   Append由来の`defer_book_append_observation`へ同folderの能動要求を集約した場合は、
   既存Append終端のBookAppend系causeが必要な更新を引き継ぐ。非main readerの受動要求は、この記録入口より先に拒否する。
4. **restart:** 未完了の要求／保留をやり直すときは、そのcauseをcause付き入口へ渡す。
   現在の`poll_external_rescan`内restart closureのExternal固定呼出しは変更対象。
   一方、操作更新の受理後にwatchのcutoff以後だけを確認する追加scanは新しい`External`要求であり、
   操作由来へ便乗させない。mainではdirtyを残して通常走査、非main readerでは受動要求を作らない。
5. **独立化・消費:** promote／forkではwatchを退役し、scan／保留はcause別に扱う。
   `External`は取消・退役、`Operation`／BookAppend系は終端と必要な保留を同ownerに残す。
   能動`external_rescan_pending`が旧watchのsubscription proofを持つ場合は、watch退役後にそのproofを無効化して
   無条件採用するのではなく、readonly scanを取消し、同じ能動causeを既存`folder_refresh_pending`へ戻す。
   復帰時のfresh scanは新しいowner proofで要求する。非mainの能動scanにはwatch登録用senderや購読を新設しない。
   別ownerであるAppend worker／metadata終端は、このreadonly scan退役へ巻き込まない。
   FolderObservationのserviceを一律止めて能動scan完了のdrainまで止めない。
   live reader中の能動Differentは既存reader保留へ戻し、一覧復帰の資格が成立してから消費する。
   restart可能でもlive reader中は同じ保留を毎frame再走査せず、能動更新を理由にwatchを再登録しない。
6. **取消・エラー:** owner退役／source変更／世代不一致の既存拒否は能動要求にも適用する。
   由来が能動だから別sourceへ配送したり、Undo後の旧receipt／旧scanを採用したりしない。
   receipt由来の取消は一般Operationをreceipt所有物と誤認しない。まれなI/O失敗は既定のログ＋通知で閉じる。

| 同じowner／folderへのcause合流 | 結果 |
| --- | --- |
| External＋External | External |
| External＋Operation（順序不問） | Operation |
| External＋BookAppend系 | BookAppend側を保持（有効性の検査は既存owner／workerで行う） |
| BookAppendExternal＋Operation／BookAppend系 | BookAppendExternal（receiptは復活させない） |
| 同じreceiptのBookAppend同士 | 同じBookAppend |
| 別receipt同士、またはBookAppend＋後続Operation | BookAppendExternal。後続mutation受付の失効規則も維持する |

Append terminal／scan完了／保留の全体を止めるのではなく、唯一のcauseと既存ownerから配送を決める。
これにより「pendingは能動なのにrestartだけ受動」「receipt失効でAppend保留が受動へ化ける」状態をなくす。
causeは実行中メモリ上の既存要求データであり、永続形式変更・旧データ移行は不要。

## 5. 欠損項目のskip設計と仕様境界

**C3の将来案。今回C1では既存のskip／失敗表示を変更しない。**

外部削除／改名はsnapshot保持後の通常操作であり、まれなI/O失敗扱いにして放置しない。
ただし一覧を修復する再走査はせず、当該操作の候補列を前／後の方向へ**一周以下**進める。
既存MediaNavigationResolver／MediaNavigationPendingとcontext別load ownerを使い、
静止画用の別pendingやwatchを足さない。

1. media手動／EOFは既存exists候補workerを維持。静止画manual／slideshow／連結の提示候補も、
   必要な候補判定を同じworker／既存decode結果へ接続する。UIでexists／statをループしない。
2. 既存の使用可能なcache／retained rasterを先に尊重する。外側ZIP／PDFが無いだけで、読める全ページまで捨てない。
   virtual entryが実際に読めない場合は、そのload結果を同ownerの未消費候補へ継続する。
   使用直前消失も同じ扱い。別のretry loopや候補再列挙は作らない。
3. skipでitemsを削らず、基本順序・Stack対応表・stable identityを保つ。
   Spread／splitは失敗したslot／pageを明示し、生存相方を別unitのindexへ誤対応させない。
   構成のHidden／Checkingをmissing検査で迂回しない。既存の提示許可を維持する。
4. 全候補が読めなければ一度通知して送り／自動再生を停止し、空のmain置換・無限wrap・同じ失敗の再要求をしない。
   初回strict target不在で別の先頭へ飛ばない既存契約は維持。snapshot採用後のnext／prevと区別する。
5. stat成功後のdecode errorは現在自動skipではない。案Cの「読めない項目として次へ」を全読込失敗へ適用するなら、
   load終端から同じ操作の候補tailへ進む配管までが完成範囲。単なるexists追加で完成とはしない。
   native presenter失敗／password取消／ユーザーcloseは欠損扱いにせず、既存terminalを維持する。

この範囲はまだ未実装で、既存helperの完全流用が検証済みという主張ではない。
masterの描画unit／紙めくり／PDF epochにも接続するため、watch廃止より広い。
失敗表示に留まる現行テストを黙って弱めず、変更する仕様と試験を独立レビューで特定する。

## 6. 規模・削れるもの・推奨順序

ファイル数は実装前の概算。共通helperの置き場所・試験分割によって増減する。
行数削減は未実装のため計測値ではない。mainが同じ機構を使うので、reader停止だけで
watch／rescan型・bundle field／ContextAsyncOwner全体を削除できるわけではない。

| 案／段階 | 見込み変更範囲 | 消せる組合せ／残るもの | 今日〜明日の判断 |
| --- | --- | --- | --- |
| A（既存§9、履歴） | product 3〜4、試験1〜2、文書2、計6〜8ファイル（当初見積） | readerも観測・保留する旧案。source／main-owner境界の考え方だけ参照 | **今回不採用** |
| C1（今回採択） | product 6前後、試験2前後と既存fixture共通化、文書／manual。app.rs／top_level_grid_view.rs／registry／book_append_refresh.rs／filename_stack_ui.rs／file_organize.rs | **既に独立したreaderとpromote後**の受動watch／scan／restart／外部保留を止める。§4.5のcauseを全経路で保持。mainの観測と最初のpromote、能動操作の終端／保留は維持 | **2026-10-15利用者採択・残P2補完後の実装指示あり**。linked中／missing処理は変更しない |
| C2（全別窓のopen時分離＋reader向け能動refresh整理） | 累計product 5〜8、試験2〜4、文書／manual3〜4、計10〜16ファイル程度。上記＋native_video／viewer open・selection routing／book_append_refresh | linkedのitems共有、自動観測とreader用復帰refreshをなくす。初回materialization、metadata、Append終端、main側ownerは残る | 当日の小修正として扱わない。F12／再生不中断・選択連動の独立レビューと実機確認が必要 |
| C3（全種類のmissing／read失敗skip完成） | C2からproduct2〜4／試験2〜3程度追加または重複。app.rs、ui_fullscreen.rs、必要ならpage/load helper、media error配送 | 失敗画面での手動再操作を減らすが、同owner内の候補継続が必要。候補worker・decode・epochは残る | 読込部分だけでも実装1〜2日＋回帰／レビュー1日程度の粗い見積。全Cを明日までに保証できない |

C1の最小不変条件は「registry main以外のlive readerに**受動FS更新の要求・採用権限が無い**」。
FolderObservationのserviceだけの停止やnavigation scopeだけの分岐ではない。
検索のaccepted recipe資格、初回／Resumed／restart／先着scan完了／defer消費の全入口を閉じる。
能動操作のAppend終端等は止めず、一覧更新が必要な既存操作は既存reader lifecycleへ残す。
そのためC1では案Aのsource／main-owner境界の考え方だけ参照し、受動更新のreader観測・保留は採用しない。
この段階仕様はmanualへ「独立／promote後」と限定して書き、通常linkedも固定済みと書かない。

コード削減の主対象は、reader向け購読開始／fork後の再購読・dirty追随・復帰refreshの配送分岐とその試験。
初期段階は資格共通化・取消／遅着検査が必要なため、**差分の純減は保証しない**。
全Cでlinked選択同期まで廃止すればさらに消せるが、それは追加の機能削除なので今回の採択案にしない。
取り除くのは状態の発生源であり、assert／stale拒否／兄弟隔離試験や全close lifecycleではない。

## 7. lifecycleとdetached憲法での位置付け

この表の受動観測停止は今回C1に適用する。能動操作の既存保留・終端は§4.5に従い維持し、
F12のopen時分離やmissing処理変更は含めない。

| 境界 | 案Cの扱い |
| --- | --- |
| open／初回準備 | 既存typed open／enumeration／sidecar／decode ownerが一度初期列を採用。未完成sourceの作業を「観測」と誤って止めない |
| fork／promote | items／recipe／identityを同じtransactionでreaderへ。受動観測だけを退役。新mainの一覧と購読は正常に開始 |
| park／再mount／tray | player／編集・metadata pending／必要なcacheを同ownerで維持。外部観測を再作成せず、未観測変更へ追い付こうとしない |
| 明示reload／folder移動 | 対象ownerで新snapshot採用。自動originからの抜け道にしない。F12 placement変更だけでは再列挙しない |
| close／cancel | 当該contextのworker・binding・cacheを既存経路で退役。readerの外部保留をmainへ移さない。兄弟は無変更 |
| error | C1では既存の欠損skip／失敗表示を維持。§5の拡張はC3。disk／DB／worker生成等のまれな失敗は既定の割り切り（ログ＋通知、既存の単純な復帰入口）。データ削除・rollback・retry ownerを作らない |

CLAUDE.md「設計の簡素化」に従い、Aの「readerも観測して後で整合させる」組合せを、
Cでは「readerは受動観測を持たない」へ減らす。[detached-rework-plan.md §2](detached-rework-plan.md)の
**BA-7**（所有事実をtransient flagsから推測しない）が主対象。
新しいdetached bool／Option、rect捕捉、viewport再生成、時間窓は不要。
BA-5のrender entry統合や残りのdetached rework完了をrelease条件に追加しない。
リワーク残項目を急がないという利用者方針と両立し、今回C1へF12の所有分離やskip拡張を混ぜない。
独立再レビューは整理完了の能動分類以外を承認し、C1の構造修正に同意済み。
利用者はこのP2補完とC1実装を続けて行うよう指示した。検証前に成功とは記さない。

## 8. 試験計画と実機確認

### 自動試験計画（実施結果は§9）

1. **crash実handler:** Ctrl+R／Ctrl+G結果→実binding・session＋fake動画／音声→検索×→最初のpromote→
   reader再mount→FolderObservation service。watch／scanが新設されず、既存の先着scan完了・restartも列を変えない。
   main／player／binding／recipe不変、promote一回を検査。通常folderの実変更も対照にする。
2. **所有資格:** main／非main、Main scope／DetachedPhysical、active／parked／Closingのbinding残存、
   物理／検索／ZIP／PDF、watch通知／Resumed／初回／restart／保留消費を表で検査。
   C1でも能動Appendの完了とmain配送を止めず、読取結果が汎用main置換へ流れないことを確認。
   **P2回帰:** 後続ユーザーmutationでreceiptを失効させたAppendの保留を作る→promote／fork→park／再mountを通す。
   causeはBookAppendExternal、watch／受動scan新設0、Append終端・必要な保留・main配送は残り、
   Undo receiptは復活しない。reader中は保留、一旦一覧へ戻ると既存ownerで消費できることを対照検査する。
   receiptあり／なし／検証降格、Operationのexport／Delete後Stack再準備、Externalを対にする。
   External→Operation合流／逆順、Append＋後続Operation、worker捕捉後のcause変更、
   restart_requested／保留消費後のrestartでも能動由来を失わないこと、cutoff後だけの通知はExternalのままで
   readerのwatchを復活させないことを検査する。主走査の能動完了をservice抑止で止めないことも確認する。
   **整理完了回帰:** 実`poll_file_organize`→Operation走査開始→promote→再mountを通し、
   必要な能動保留が残り、受動watchは復活せず、main／兄弟・再生が不変であることを検査する。
3. **main非退行:** §1.375の`section1375_worker_equal_preserves_cursor_checks_scroll_and_late_thumbnail`、
   Delete→Equal／Stack対応表／cutoff後再走査、§1.314、fork main記録、export逐次追加、
   §1.353 ZIP prefix／facet、§1.373 Undoを維持する。
4. **仕様変更する試験の棚卸し:** `section1375_detached_physical_reader_defers_changed_rows_until_list_return`、
   `section1375_same_folder_fork_park_mount_independent_subscription_and_close`は、Cのreader対象では
   「defer後反映／独立購読」から「無購読／列不変」へ意図的に変更する。mainに対する同名primitiveの保証は残す。
   `bundle_media_navigation_skips_missing_real_files_but_not_virtual_entries`はC3のvirtual skip差分を別試験として記録し、
   v4.6.0のcache閲覧を失う一律exists検査へ書き換えない。
5. **Append／metadata:** `section1373_detached_source_terminal_and_main_refresh_keep_separate_owners`、
   `section1373_fullscreen_append_retires_into_typed_deferred_refresh`、
   `section1373_closed_source_preserves_success_and_newer_sibling_draft`を段階仕様に合わせて対照確認。
   reader向けscan開始後promoteでもreceipt有効／pending終端／main更新／Undo成功。
   metadata importのmain＋detached、rating／edit publication、rename rehydrate、初回sidecar継続は維持。
6. **C2（今回対象外）:** F12前後、設定からの静止画／本／メディアopen、linked selection／sort／filter、
   同indexで別identity、snapshot外選択、明示open、mainへのplacement復帰、ParkedLiveを確認。
   player二重化・音切れ・main一覧凍結・兄弟generation／cache取消が無いことを検査する。
7. **C3（今回対象外）:** 前後の連続欠損、全欠損、wrap一周、exists後消失、read不可、ZIP member／container消失、
   PDF／変換cache hitとmiss、Spread相方／split／連結／slideshow、media errorとEOFの区別、
   strict initial targetのclose維持、skip途中のpark／promote／close／遅着拒否。UIの同期I/O／走査0を確認する。
8. **費用:** C1ではsource変更なしの再mount反復時に受動watch／scan件数0、
   能動pendingの毎frame再走査0、mainの更新latencyと既存DB query数を確認。行数分のcloneをUIへ新設しない。
   長い一覧での候補収集・有界skipのworker化はC3の確認であり、今回へ追加しない。

削れるのはreader向け「外部変更を最後に採用する」前提の試験であり、owner隔離・late result拒否の試験は
新契約へ置き換えて残す。設計の独立レビュー後、共有経路の実装はtargeted・fmt／通常・portable check→
関連ui_snapshot→test-full最後に一回。今回の利用者指示ではbuild-devは利用者側で行い、agentは起動・停止しない。
実装時の機械監査は`FolderRefreshCause`全match／constructor、`folder_refresh_pending`全代入、
`start_external_rescan`／cause付き入口の全呼出し、restart closure、worker応答causeの採用、
`check_external_folder_changes`の全呼出し元、`From<PathBuf>`／Default使用を対象とし、分類表と対応付ける。
causeを落とす暗黙生成が残っていないことを確認する。

### 利用者の実機確認（別途明示承認を得て実施）

- Everything／Ctrl+Gから動画・音声別窓を開き検索×、その後の前後送り・窓close・tray復帰で終了せず再生継続。
- 通常folderから動画をpromote後、外部で追加・改名・削除。別窓の列は固定、mainだけ更新、欠損候補は段階仕様どおり。
- 画像folder／ZIP／PDFをF12と各設定で開き、mainの追加／Delete／Ctrl+F5が働くことを確認。
  C2ではlinked選択・F12往復・明示folder移動・main表示への復帰、C3では各missing skipも確認。
- 本へ追加→後続操作でreceiptを失効／失効しない両方で独立化・再mount。
  C1では既存の能動保留・成功通知・必要な一覧復帰時更新が残り、watchは復活しないことを確認。
  receipt有効時のCtrl+Z、mainの本一覧更新とUndoも維持する。
- ★・補正・タグ・しおり・明示importを行い、snapshot化で編集が消えたり古い値へ戻ったりしないことを確認。

実機はdisposable portableと捨てデータを用いる承認済みsuite、または利用者自身の確認とし、
agentがnormal APPDATAのアプリを起動しない。確認用buildを作る段階で具体的な起動方法・profile注意を引き継ぐ。

## 9. 実装と検証記録（2026-10-15）

### 実装・監査の範囲

実装基点は`ddcc0166d`。今回実装したのはC1だけで、C2／C3／案Aのreader観測は入れていない。
新しいbool／pending／ownerは追加していない。`plan_promote`のassertも維持した。

| ファイル | 変更した境界 |
| --- | --- |
| `src/app.rs` | accepted recipe／registry main identityで監視資格を決定。開始・通知・Resumed・完了・restart・保留消費で受動更新を拒否。能動causeを保持し、live reader中は既存holdに戻す。main置換のpromoteはmain所有のplayerに限定 |
| `src/app/top_level_grid_view.rs` | UI検索flagsが消えてもExtractedResult recipeを物理一覧と判断せず、旧検索列を検索前folderの走査で上書きしない |
| `src/app/viewer_context_registry.rs` | promote／独立化でwatchを既存retirement workerへ渡す。旧購読proof付き能動scanは取消して同じcauseのholdへ戻す。forkではmain側のwatch／能動scanを維持しreaderへ持ち込まない |
| `src/app/book_append_refresh.rs` | OperationとBookAppend系を区別。receiptなし／失効済みAppendを能動のまま配送し、他操作のscanをAppend取消へ巻き込まない |
| `src/ui_dialogs/file_organize.rs` | Copy／Moveの完了（部分成功・取消・エラー後の既存確認を含む）をOperation付きcheckへ接続 |
| `src/filename_stack_ui.rs` | Delete後のStack保持投影の再準備をOperationとして既存holdへ接続 |

`check_external_folder_changes`の本番呼出し元はwatch通知・main focus復帰・tray復帰の3箇所となった。
整理完了はcause付き入口へ分離し、`Notified`を由来の判定に使わない。
causeなし`start_external_rescan`の本番呼出しは初回watchとcutoff後dirtyの受動確認だけ。
`FolderRefreshCause`の全match／constructor、保留代入、開始・restart・worker応答、`From`／Defaultを監査し、
cause省略の本番生成箇所は0件。テスト／`release_checks`のfixture変換は本番producerに含めない。
新たな合流・退役・restartではcauseをmoveし、receiptの全構成をUIで追加cloneしない。
watchのdropもUIで待たず既存retirement workerへ渡す。

回帰は新規15関数（新規module 7＝handler 6＋source資格1、Append cause 4、registry 4）。
検索closeはCtrl+R／Ctrl+G×動画／音声の4ケースで実staged採用→実binding／session→main復帰→promote→再mountを通す。
同じplayerのheap identity・再生中・再生位置が先頭へ戻らないこと（17秒以上の範囲内）を確認し、mainの一覧／watchと次の実変更採用を対照にする。
整理は実`poll_file_organize`からCopy／MoveのOperation走査→promote→hold→再mountを検査する。
物理readerの実変更／先着scan・restart、失効Append保留とpark、External応答へ後から合流したOperationの再走査も含む。
既存reader購読試験はC1の無購読へ変更する一方、main／linkedの購読、fork main dirty、
global hydration待ちを保つ能動reader更新は残した。

独立コード確認では追加の製品コードP1／P2なし。非main受動scanを期待した旧試験1件をOperationへ直し、
worker捕捉後の合流・restartを実pollで通す回帰も補った。実機動作の確認は未実施。

### 自動検証

実行ログとcommand／所要時間／exit／件数は`target/c1-20261015/`へ保存する。
初回native buildはMSBuild並列実行で失敗したため、子process環境を`CARGO_BUILD_JOBS=1`と
`MSBUILDDISABLENODEREUSE=1`に限定して通常のvendor buildを再実行した。
一時的なprebuilt TurboJPEG指定も試したがLibRaw includeで失敗し、最終検証では使用していない。
repoのbuild設定・vendorコードは変更していない。
テスト追加時の型誤り2件はfixtureをAppだけで動く形へ修正。
初回targetedは42成功／4失敗で、新規共通assertが再生中時計を固定値17秒と比較していたため、
player identity・再生状態・前進位置の検査へ修正した。この失敗を最終成功に含めない。
続く既存Append回帰2件は、mainの観測所有資格にwatch登録用の`current_favorite_target`条件まで含めた
実装誤りを検出した。watch登録条件は既存の`ensure_current_folder_watch`に残し、
要求／完了の所有資格はmain identityとaccepted sourceだけから導出するよう修正した。
これによりwatch未登録／初回stamp未確定のmainでも既存Append ownerへ観測を記録できる。
当該2試験のassertionは変更していない。
補足visibility回帰は初回123成功／1失敗で、Snapshot rootから開いた物理子の更新を
return surfaceの分類で拒否する誤りを検出した。採用済みrecipe＋既存synthetic path判定へ集約し、
watch登録条件・明示reload配送は維持。既存の実hydration／semantic cursor／固定root不変のassertionを
変えずに直し、OrdinaryListの合成rootを拒否する対照回帰も追加した。
音声ケースはsession sourceも実Audio itemに合わせ、Video sessionだけの検査にしない。

最終targeted結果（全て失敗0／ignored 0）。同一試験のfilter間重複を除くと970件、延べ1,006件。

| command／filter | 成功数／結果 |
| --- | ---: |
| `cargo fmt --all -- --check` | 成功 |
| core `cargo check` 通常／`--features portable` | 両方成功 |
| lib `section1375`（新規C1の11件を含む） | 47 |
| lib `book_append_refresh` | 20 |
| lib `viewer_context_registry` | 94 |
| lib `global_search_ui` | 88 |
| lib `file_organize` | 19 |
| lib `section1373` | 76 |
| lib `section1353` | 22 |
| lib `external_folder` | 1 |
| lib `visibility_` | 124 |
| lib `detached` | 333 |
| lib `completed_export_refresh` | 1 |
| integration `ui_snapshot` | 181 |

UI snapshot単独実行はsource資格の最終補正前。UIコードはその後変更せず、最終treeでもtest-full内で181成功を再確認した。
既存1万／10万行・Equal poll／Delete後query数回帰も`section1375`で成功。
関連試験が全成功してから、`scripts/test-full.ps1 -SuppressCrashDialogs`を最後に一回だけ実施した。
**full gateは未成功**。20:15:23開始、3,037.62秒で完走、exit 101。
workspaceの主suite集計は14,511成功／1失敗／66 ignored（stdoutの子process summary二件の重複を除外。
raw summary合計は14,513成功）。新規C1回帰15件は全て成功した。
workspace失敗のため後段のvendor gateはscriptの既存規則で実施されず、全体PASSとは扱わない。fullは再実行しない。

唯一の失敗は既存`video::audio::tests::zero_start_pump_trims_latency_and_preserves_samples_through_eos_loop_and_seek`。
`src/video/audio.rs:4850`で、48kHz／phase 0の`audio_tail_complete()`が3秒待ちの終端まで成立しなかった。
同試験はVideoPlayer／AudioOutputだけを使用し、App／FolderObservationを呼ばない。C1ではvideo／audio実装を変更していない。
限定した独立追跡でも、C1 fixtureと当該pumpのclock／engine／buffer／seek／cancelはinstanceごとに独立し、
C1側fake audioはpumpなし、共有DSP coordinatorもなし。直接のmutable global干渉を支持する経路は見つからなかった。
ただし負荷による失敗ともC1無関係とも断定しない。
同じ`pack-build-tools` featureで当該試験だけを`--exact`指定して一回確認し、**1成功／0失敗**だった。
commandは`cargo test -p mimageviewer --features pack-build-tools --lib video::audio::tests::zero_start_pump_trims_latency_and_preserves_samples_through_eos_loop_and_seek -- --exact`。
試験実行0.30秒、package選択による実行体の再buildを含め423.42秒、exit 0（`audio-isolated.json`）。
workspace実行とは実行体のdependency feature統合が異なるため、並列負荷だけが原因とは証明しない。
この単独成功で上記full失敗を成功へ置き換えず、audioの実装／試験assertionも変更していない。
全体gate未成功の音声試験1件は設計担当へ引き継ぐ。

変更は製品コード6ファイル、テスト／fixture 5ファイル、設計・索引・manual 7ファイルの計18ファイル。
新規ファイルは本書と`src/app/section375_c1_tests.rs`。`git diff --check`、新規ファイルの空白／競合marker確認は成功。
HEADは`ddcc0166d`のまま、stage済み変更は0件。
build-devは今回の利用者指示で実行しない。アプリ起動・停止、git add／commit、CHANGELOG変更も行わない。

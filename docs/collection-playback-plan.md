# コレクション Phase 4 再生・横断ナビゲーション実装計画

最終更新: 2026-09-15  
状態: Phase 4 製品実装、独立 completion review、focused / full / static gateを完了。`target/dev-runtime`のresidentを停止しないためverification buildだけ保留。

## 1. 目的と範囲

本書は [`collection-spec-proposal.md`](collection-spec-proposal.md) で確定した次の挙動を、現在の producer と viewer-context ownership へ接続する Phase 4 の実装計画である。

- Grid と fullscreen の Ctrl+↑↓ は、物理 folder tree へ逸れず、現在のコレクションに登録された folder / ZIP / PDF / convertible archive を現在の有効順で横断する。
- fullscreen の通常の次 / 前項目は、collection root の直接登録 Image / Video / Audio を最新の有効順から選ぶ。登録 container は未展開のまま飛ばす。
- スライドショーの通常の静止画送りと `SlideshowEndAction::NextFolder`、動画・音声の連続再生 EOF は、次を決める時点の最新コレクションを使う。
- 次候補の anchor は entry ID、次に正規化済み source key で探す。両方が最新一覧に無い場合は最初の適格候補を選ぶ。
- anchor が最新一覧の末尾に残る場合は、現在の Stop / Loop / NextFolder 設定を維持する。0 件なら停止する。
- コレクション編集中も現在の画像表示・動画・音声・スライドショーを中断しない。編集は次候補要求だけへ反映する。
- 登録 container の内部一覧と、コレクション直下の outer entry 列を別の順序として扱う。子ファイルをコレクションから削除された entry と解釈しない。
- PC と Phase 5 Remote は同じ immutable prepared model と純粋 resolver を使う。Remote に PC の `App`、`items`、`fullscreen_idx`、viewer session を渡さない。

本 Phase ではコレクションの入れ子、登録 folder の子を outer 列へ再帰展開する機能、通常ページ送りによる本から次の本への自動進入、Ctrl+PageUp/PageDown のコレクション専用化を追加しない。通常 Folder / SmartFolder / Search / Favorite / Snapshot / ReadingHistory / Rating の既存ナビゲーション、本・ZIP・PDF の内部順、履歴の `HistoryTrigger` 契約も変更しない。

## 2. 現在のコード経路

### 2.1 producer

| 操作 | 現在の入口 | 現在の解決 owner | Phase 4 の分岐点 |
| --- | --- | --- | --- |
| Grid Ctrl+↑↓ | `app.rs::handle_grid_input` の `GridTreeFolderPrev/Next` | `FolderNavPending` と物理 DFS | search / snapshot 等の既存優先分岐後、collection origin が有効なら collection next request を開始する |
| fullscreen / native media Ctrl+↑↓ | `ui_fullscreen.rs::handle_fullscreen_ctrl_nav_context` | fullscreen nav lock、`FolderNavPending`、物理 DFS | detached physical の通常規則を評価する前に、その context 自身の collection origin を解決する。collection origin が無い context だけ既存経路へ流す |
| fullscreen の通常の次 / 前 | `ui_fullscreen.rs` の `adjacent_page_navigation_idx` / continuous-reading adjacent、`app.rs::start_manual_media_navigation`、`native_video.rs::navigate_native_video_fullscreen` | 現在 `items` / `current_grid_order()` と `MediaNavigationAction::Manual` | collection root の直接登録 media なら direction 付き latest prepared requestへ分岐する。child内は既存orderを使う |
| スライドショー静止画送り | `ui_fullscreen.rs::advance_slideshow` | 現在 `items` と `current_grid_order()` | collection root の直接登録画像では latest prepared の image 列を使う。登録 container 内では既存 child 列を使う |
| スライドショー NextFolder | `ui_fullscreen.rs::try_start_slideshow_next_folder` | `FolderNavMode::SlideshowNext` と物理 DFS | collection origin があれば outer container 列の forward request に置き換える |
| 動画 EOF | `app.rs::handle_video_continuous_eof` | `items` の video 抽出、App-global media resolver | collection root の直接登録 video なら latest prepared の video 列へ分岐する |
| 動画の音声モード EOF | `app.rs::handle_video_audio_mode_continuous_eof` | 同上。着地後も音声モードを維持 | target 抽出だけ collection resolver へ置き換え、既存 hidden presenter / source-swap 着地を維持する |
| 音声 EOF | `app.rs::handle_music_continuous_eof` | `items` の audio 抽出、App-global media resolver | collection root の直接登録 audio なら latest prepared の audio 列へ分岐する |

producer は collection の有無を推測するために path、synthetic address、`current_folder` を見ない。後述の typed origin を取得できた場合だけ collection 経路を選び、それ以外は既存 producer をそのまま使う。

### 2.2 Phase 3 から再利用する三つの gate

Phase 3 の `CollectionGridSession` と `CollectionPreparedSnapshot` は次をすでに保証している。

1. **origin gate**: collection ID、revision、entry ID、source key を root leaf、同一 context の physical child、detached context の `TopLevelGridRestore::Collection` に運ぶ。
2. **prepared gate**: actor snapshot を worker で exact revision ごとに分類し、Manual の DB 順または Standard の 4 行 + `SortOrder` を `CollectionPreparedSnapshot.entries` の一つの有効順へ確定する。
3. **current gate**: viewer context ID、surface generation、collection ID、accepted / wanted revision、items generation を照合し、別 context や遅着 result を現在の一覧へ install しない。

Phase 4 はこの gate を広げる。root Grid の `CollectionGridLoadState` は再利用して占有しない。fullscreen leaf や physical child の間、Phase 3 は root rows の install を意図的に保留するため、同じ load state を next 解決に使うと、再生中の index 空間を置換するか、next 要求を永久に待たせるためである。

next 用には viewer context ごとの独立した request owner を置く。actor client と DB は引き続き process-global owner、request / receiver / cancel / intent は viewer-context owner とする。

## 3. 正本型

名称は実装時に既存命名へ合わせられるが、以下の意味を一つの型で保つ。

```text
CollectionNavigationOrigin {
    collection_id,
    revision_at_open,
    entry_id,
    source_key,
    display_unit: Option<CollectionNavigationDisplayUnit>,
}

CollectionNavigationDisplayUnit {
    primary: CollectionEntryIdentity,
    partner: Option<CollectionEntryIdentity>,
}

CollectionEntryIdentity { entry_id, source_key }

CollectionNavigationIntent =
    DirectManual { direction, step_count, landing, media_set }
  | OuterCtrl { direction, landing }
  | SlideshowImage { end_action }
  | SlideshowNextContainer
  | VideoEof { tail, eof_key, audio_mode }
  | AudioEof { tail, eof_key }

CollectionNavigationTarget =
    DirectMedia(Navigable | StillImage | Image | Video | Audio)
  | OuterContainer(ImageLike | StillImage)

CollectionNavigationDirection = Forward | Backward
CollectionNavigationTail = Stop | Loop

CollectionNavigationStamp {
    request_id,
    context_id,
    collection_id,
    minimum_revision,
}

CollectionPreparedNavigationStamp {
    request_id,
    context_id,
    collection_id,
    exact_revision,
}
```

`CollectionNavigationOrigin` は source path や `usize` index を identity にしない。entry ID と source key は Phase 3 が open 時に確定したものを必ず両方保存する。直接登録 leaf から初めて要求する場合だけ、現在の installed prepared binding と同じ items generation を照合して一度 capture する。対応 entry を確定できない stale root leaf は collection 要求を作らない。

rootの直接登録画像を見開き表示している場合は、request開始時のdisplay unitに属すprimary / partnerをそれぞれentry ID + source keyでcaptureする。partnerを単なる隣indexとしてpendingへ保存しない。splitで同じsourceの左右を移る操作は一つのentry identity内の既存処理であり、collection requestを作らない。このdisplay unit identityはbook / ZIP / PDF / folder childには適用しない。

origin の読取元は次の三つを一つの helper に集約する。

- collection root Grid / root leaf fullscreen: `CollectionGridSession` の exact installed prepared entry。
- 同一 context の child: `CollectionGridPosition::PhysicalSource`。
- detached child: `TopLevelGridView::return_to()` の `TopLevelGridRestore::Collection`。

同じ collection identity を `current_folder`、archive cache path、child item path から再構築しない。detached context は main の collection session を参照せず、自身の bundle に移された restore owner を使う。

`CollectionNavigationIntent` は既存の自由な bool を集めた pending にしない。履歴 trigger、presentation、slideshow 再開、EOF dedup key、手動 landing を intent variant から導出する。`HistoryTrigger::AutoAdvance` は slideshow と三つの EOF、`UserChosen` は通常の次 / 前と Ctrl+↑↓に固定する。

## 4. 共通 pure resolver

### 4.1 入力を prepared snapshot 一つへ統合する

現在の `resolve_latest_collection_next` は raw `CollectionSnapshot`、別配列の effective order、別配列の prepared fact を受け取る。Phase 4 では本番利用前に、これを `CollectionPreparedSnapshot` の aligned `entries` を直接読む helper へ置き換える。三つの配列の ID 集合を呼出側で同期する API は残さない。

共通 helper は DB、filesystem、`GridItem` index、UI filter、`visible_indices` を読まない。入力は immutable prepared snapshot、origin、方向、target class、tail policy、同じ exact revision 内で既に不適格と判明した entry ID 集合だけとする。出力は target entry の ID / source key / source path / prepared kind と、anchor が ID・source key・head のどれで解決されたかである。

Manual は `prepared.entries` の DB manual position 順をそのまま使う。Standard は prepare 済みの現在の 4 行 + sort 順をそのまま使う。ローカル検索、Grid の選択順、表示中の一時 filter、過去 revision の UI index で並べ直さない。

### 4.2 anchor と方向

resolver は次の順に anchor を決める。

1. 最新 prepared に同じ entry ID があればその位置。
2. entry ID が無く、同じ `CollectionSourcePathKey` があればその位置。同じ参照を外して再登録した場合は新 entry の次から進む。
3. 両方無ければ方向や tail policyに関係なく、最新順の最初の適格候補を返す。これは「削除された現在項目」と「残っている末尾」を区別する規則である。

単一項目では上のanchorをそのまま使う。root直接登録画像の見開きdisplay unitでは、primary / partnerをそれぞれentry ID、次にそのidentity自身のsource keyで最新preparedへ解決する。Forwardは生存するdisplay unit identityのうち最新順で最も後ろ、Backwardは最も前をdirectional anchorにする。片方だけremoveされても残った側を使い、両方とも解決できない場合だけheadへfallbackする。これによりpartnerを次targetとして再表示せず、見開きの歩幅を維持する。

anchor がある場合だけ、Forward は後方、Backward は前方を検索する。Backward の着地も既存 Ctrl+↑ と同じく target container の先頭 image-like とし、container 内末尾へは着地しない。Ctrl の端は wrap せず境界結果を返す。EOF の `ContinuousLoop` と slideshow の `LoopFolder` だけが最初の同種媒体へ wrap する。

Grid root に fullscreen / child origin が無く selection も無い場合は origin-less head として扱う。Ctrl+↑だけを暗黙に末尾へ送る特例は設けない。

### 4.3 media 別の抽出

直接登録 media は `CollectionSourcePreparation::Available` の現在 kind がintentの集合に一致するentryだけを候補にする。

| intent | target |
| --- | --- |
| 通常の次 / 前項目 | Navigable = Image / Video / Audio |
| 連結読み中の明示的な静止画移動 | StillImage = Image |
| slideshow の通常送り | Image |
| video EOF / 動画の音声モード EOF | Video |
| music EOF | Audio |

missing、unsupported、access error、last-known kind だけが一致する placeholder は候補にしない。動画の音声モードは UI presentation が音声でも source kind は Video のままとする。

folder / ZIP / PDF / convertible archive の内部 media は直接 media 列へ flatten しない。子一覧での通常の次 / 前、slideshow、video EOF、audio EOF はその child の `items` と既存 book/archive orderを使う。collection entry が編集中に外されても、現在の child を閉じたり、その child media を「最新一覧に無い直接 entry」として head へ飛ばしたりしない。

### 4.4 outer container 候補

Ctrl+↑↓ と NextFolder は available な Folder / Zip / Pdf / ConvertibleArchive だけを outer 候補にする。Image / Video / Audio と placeholder は飛ばす。登録 folder の子や archive pageを outer candidate として追加しない。

prepared snapshot は container root の存在と kind を持つが、内部に着地可能 media があることまでは確定しない。pure resolver は有効順の container candidate を返し、PC の既存非同期 folder scan / ZIP・PDF enumerate / conversion probe が action 別の適格性を確認する。

- Ctrl+↑↓ は既存 fullscreen / Grid の image-like container open 条件を使う。
- `SlideshowNextContainer` は既存 `SlideshowNext` と同じ still-image 条件を使う。
- candidate が空、内容不適格、または open 直前に失踪した場合は UI thread で stat せず、その exact prepared revision の次 candidate を試す。
- request の watch がより新しい revision を通知した場合は、旧 candidate 列の継続より先に snapshot / prepareをやり直し、元の origin から解決し直す。
- 一つの exact revision で試した entry ID を pending owner に持つ。走査回数はその revision の eligible container 件数以下とし、同じ entry IDを再試行しない。revisionが変わって新しいpreparedを受理した時だけtried集合をresetする。これにより空container間の循環やLoop時の無限再試行を防ぐ。
- direct mediaのexistence preflightに失敗したentry IDも同じtried集合へ加える。Loopを含め、一つのexact revisionで試すdirect mediaはそのmedia候補件数以下であり、同じIDを再試行しない。

この二段構成により、collection order の共通 resolver と PC / Remote ごとの container open 能力を分離する。Phase 5 Remote は同じ候補順を使い、Remote の列挙・open owner で内容を検証する。

## 5. intent ごとの挙動

### 5.1 通常の次 / 前項目

collection root の直接登録 media から fullscreen を開いている時は、paged image の矢印 / wheel / page action、動画・音声の `VideoNextFile` / `VideoPrevFile`、native presenter の手動前後送りを `DirectManual` へ接続する。候補集合は現在の `adjacent_navigable_idx` と同じ Image / Video / Audio であり、未openの Folder / ZIP / PDF / ConvertibleArchive と placeholder は飛ばす。

- Forward / Backward は最新 prepared の有効順を使う。anchorが残る端ではwrapせず既存の先頭 / 末尾hint、anchorがremove済みならheadへ着地する。
- root直接登録画像の見開きは、request時にprimary / partner stable identityをcaptureする。Forwardは最新prepared上で生存する二つの後端、Backwardは前端から進み、全identity消失時だけheadへ着地する。target決定後の見開きpartner構築は既存`spread_page_nav(_for_indices)` / landing処理へ委ねる。
- `delta` の絶対値が1を超えるnative / queued inputは、既存の「有効候補をstep_count件進む」意味を保つ。各user inputは一つのactor snapshotをdecision pointとし、そのprepared内でstep_count件解決する。別入力としてqueueされたstepは前のtarget commit後に最新を取り直す。
- splitの同一source内移動と連結読みの現在scroll/layout内の移動は既存処理を維持する。連結読みでitem境界を越える明示移動だけStillImage候補のcollection requestを使う。
- target commit後は既存 `open_fullscreen_from_fs_navigation` またはnative manual landingを使い、`HistoryTrigger::UserChosen`、cursor / spread / split / presentationを維持する。
- root collection以外、またはcollection childのImage / ZipImage / PdfPage / Video / Audioは既存のcurrent `items`順を使う。child pageをouter direct media列へ混ぜない。

### 5.2 Ctrl+↑↓

Grid producer と fullscreen producer は既存 search、snapshot、IME、modal、input multiplicity の gate を維持する。collection origin が current と確認できた後だけ物理 DFS の代わりに `OuterCtrl` を開始する。

- Ctrl+↓は origin より後、Ctrl+↑は前の outer container を現在の effective order から探す。
- root Grid は選択 entry、root leaf fullscreen は開いている entry、child は outer origin entry を anchor にする。
- manual input は現在どおり slideshow を停止する。boundary / error の場合も停止規則を変えない。
- fullscreen は既存 `begin_fs_folder_navigation_sequence` / holdover / presentation 維持を使い、着地先 container の先頭 image-like を開く。
- Grid は target container の rootを既存 Grid open pipeline で開く。fullscreen からの着地を Grid 経路へ落とさない。
- 連打は context ごとの bounded signed accumulator に積む。1 target の commit 後に新しい origin で次の latest requestを開始し、複数 step を一つの古い prepared snapshot で先読みしない。
- 別 intent、collection からの明示退出、context retire は accumulator と pending を cancel する。別 collection の編集は cancel しない。

Ctrl+PageUp/PageDown は同じ親という概念を collection に新設せず、Phase 4 の分岐対象外とする。

### 5.3 slideshow

collection root の直接登録 Image を表示中は、`advance_slideshow` の各 tick が `SlideshowImage` を要求する。これにより並べ替え、remove、同参照再追加、新規追加が次の自動送りに反映される。画像が最新一覧に残る場合の末尾は設定どおり処理する。

見開き中のtickもrequest時のprimary / partner identityをcaptureし、最新prepared上の生存display unit後端から次Imageを探す。target着地後のpartner、見開き方向、final cover、singleton配置は既存spread構築へ委ねる。slideshowの歩幅を1枚へ戻さない。

- `Stop`: slideshow を停止し現在表示を保つ。
- `LoopFolder`: 最新の最初の Image へ戻る。候補が現在の 1 件だけでも既存同様その画像から timer を再開する。
- `NextFolder`: 後続 Image が無い時だけ、同じ origin を anchor に `SlideshowNextContainer` を開始する。次の still-image container を開き、既存の `resume_slideshow` と `HistoryTrigger::AutoAdvance` を維持する。見つからなければ現在どおり slideshow を停止する。

container child の内部では `adjacent_slideshow_idx`、見開き / split の歩幅、ZIP tree 内の順、連結読み scrollをそのまま使う。child の末尾で `NextFolder` が選ばれた時だけ outer origin から latest container を解決する。`LoopFolder` は child 内先頭、`Stop` は child 内停止であり、outer collection へは出ない。

next request の待機中は timer の再入だけを止め、現在の frame、holdover、slideshow を開始した viewer context を保つ。collection edit notice は表示や timer を止めない。

### 5.4 video / audio EOF

collection root の直接登録 media だけを latest collection EOF resolverへ流す。child 内media と collection 外 media は現在の `collect_matching_media_navigation_candidates` 経路を維持する。

- `Continuous`: anchor の後に同種 media が無ければ停止・末尾案内。
- `ContinuousLoop`: anchor の後に無ければ最新の最初の同種 mediaへ戻る。
- anchor 自体が remove 済みなら `Continuous` でも最初の同種 mediaを選ぶ。
- 対象 0 件、collection delete、latest prepare failure は現在 presentation のまま停止する。stale preparedで wrapしない。
- video、video-audio mode、music の既存 `(fs_idx, seek_serial)` dedup と action-current gateを保つ。pending を revision 更新で再開する間は dedup を維持し、request が terminal failure / cancel になった時だけ既存 rollback規則に従う。
- target 決定後の native presenter、fast source-swap、audio mode 維持、ParkedLive owner、autoplay / resume one-shot は既存 apply handlerを使う。resolver は presentation を変更しない。

直接登録 media の target が最新 root `items` にまだ materialize されていない場合、古い `usize` へ変換しない。後述の commit 境界で exact preparedを rootへ installし、entry IDで新 indexを取得してから既存 apply handlerへ渡す。

## 6. 非同期 request ownership

### 6.1 state machine

viewer bundle に `CollectionNavigationPending` を一つ持たせ、概念上次の状態を直列に進める。

```text
Idle
  -> Snapshot { stamp, origin, intent, watch, receiver }
  -> Preparing { prepared_stamp, origin, intent, watch, cancel, receiver }
  -> ResolvingCandidate { prepared Arc, prepared_stamp, origin, intent, tried }
  -> Preflighting { prepared Arc, prepared_stamp, target, tried, typed preflight owner }
  -> CommitAndOpen { prepared Arc, prepared_stamp, ready target payload }
  -> Idle
```

request開始時は、まずこのrequest専用のrevision watchを確立し、その後にactorの`load_collection(collection_id)`をenqueueする。この順序によりsubscribeとloadの間のcommitもwatchかload snapshotの少なくとも一方で観測する。`minimum_revision`はoriginの`revision_at_open`と、そのcontextがすでに知るwanted revisionの最大値である。actorのload replyがnext選択のlinearization pointになる。

Snapshot reply は collection ID 一致、reply revision >= minimum revisionを確認する。prepare completion は request ID、viewer context ID、collection ID、exact revisionを全て照合する。watch に exact revisionより新しい同 collection noticeがあれば旧 prepare / candidate / preflightをcancelしてSnapshotへ戻す。他collectionのnoticeは無視する。

actor commandが snapshot replyの後に直列化された場合、その編集は今回の next 決定より後である。reply前に直列化された編集は snapshotに含まれる。この actor順を「次を選ぶ時点」とし、UI clockや通知到着時刻を正本にしない。

### 6.2 current gate

completion適用前に次を一括で確認する。

- request がその contextの latest request IDである。
- contextが retire / replaceされず、現在または ParkedLiveとして同じ ownerに属す。
- collection IDと origin ownerが開始時から同じである。
- exact revisionより新しい wanted / watch noticeが無い。
- intent固有の current条件が残る。通常の次 / 前なら同じfullscreen mediaとmanual navigation sequence、Ctrlならnav sequence、slideshowなら同じslideshow cycle、EOFなら同じEOF key / fullscreen media / presentation ownerである。
- target entry ID / source key / path / kind が resolver結果と exact preparedで一致する。

一つでも外れた completion は新 contextや現在 mediaへapplyしない。revisionだけが進んだ場合は現在表示を保って latest snapshotから再解決する。ユーザーが別項目へ移動、collectionを退出、slideshow / continuousを停止、別 requestで置換、contextを閉じた場合は terminal cancelとする。

### 6.3 bundle、clone、drop

`CollectionNavigationPending` は `folder_nav_pending`、slideshow state、fullscreen indexと同じ viewer context bundleへ入れる。`ViewerContextBundle::empty`、全 destructure / swap、fork、park / remount、retire、Drop、viewer-context auditを更新する。

- mount / AtRest移動では同じ pending ownerを payloadと一緒に移す。
- context clone / forkは active receiverやcancel tokenを複製しない。必要な immutable origin / preparedだけを渡し、新 contextが操作を始めた時に独立 requestを発行する。
- retire / explicit collection exit / App exitは prepare cancelを立てreceiverをdropする。通常 frameでworker joinを待たない。
- replacement、より新しいrevisionへのrestart、explicit exit、context retire、App exit、Dropは、Snapshot receiver、Preparing worker、direct-media existence preflight、folder/archive preflightの全variantについてcancelを立てreceiverをdropする。finished threadだけを通常pollで回収し、UI threadでjoinしない。
- `CommitAndOpen`がcurrent gateを通って既存typed landing ownerへready payloadを渡した時点で、そのtargetのterminal責任をlanding ownerへ一度だけ移す。移譲後にcollection pending側が同じopenをcancel / applyせず、移譲前のdropではlanding ownerを作らない。
- process-global collection actor/clientと既存 media resolver threadを viewer bundleへ移さない。
- Remote sessionはPC bundleとは別の context identity / pendingを持つ。

## 7. target commit と open

candidateを順序上選んだだけでは現在の`items`、fullscreen index、player、slideshow frameを変更しない。folder scan、ZIP / PDF enumerate、conversion probeをtyped preflightとして実行し、intentに合う着地可能payloadの成功まで確定したtargetだけをcommitへ渡す。既存loaderが検査と表示切替を分離できないpathは、製品接続より先にtyped preflightを切り出す。

1. resolver候補について、direct mediaはprepared availabilityと既存非同期existence check、containerはaction別のfolder / archive typed preflightを完了する。失敗時は現在presentation/root bindingを保ち、同じexact revisionの次candidateへ進む。
2. ready target payloadに対してwatch / request / context / collection / exact revision / intent-current gateを最後に再確認する。
3. fullscreen / native presentationの既存holdoverとmanual / auto history triggerをcaptureする。
4. 成功確定後の一つのcommitでchild / stale rootのindex空間を閉じ、resolverが使ったexact `Arc<CollectionPreparedSnapshot>`を同じcontextのcollection root bindingへinstallする。ここで別のactor load / prepareを重ねない。
5. target entry ID、次にsource keyを同じprepared bindingへ照合し、新しいitem indexを得る。
6. ready direct mediaまたはcontainer payloadを既存fullscreen / native media / folder / ZIP / PDF / conversion landingへ渡し、同じcommitで新しいouter originを確定する。preflight failureやcancelでoriginだけ先に進めない。
7. slideshow NextFolderだけ再開し、Ctrlでは停止状態を維持する。EOFは既存presentationとautoplayを維持する。

materialize時も Phase 3 の root selection / checked remap規則を使う。navigation targetはentry ID exactをselectionにし、source key fallbackはentry IDが消えた場合だけ使う。local filterでtargetが非表示でも navigation正本から除外せず、既存fullscreen openに必要な一時表示 / filter suppression契約を使う。

候補preflightで内容不適格と判明した場合はroot installをcommitせず、現在presentationを保ったまま次candidateを試す。表示を一度閉じてから空containerと判明し次へ飛ぶ実装は採らない。

## 8. 編集と失敗時の規則

| 事象 | 挙動 |
| --- | --- |
| 同じcollectionを並べ替え / add / remove | 現在再生を維持。次requestはactor最新revisionから解決 |
| Manual↔Standard切替 / Standard sort変更 | 現在再生と現在root itemsをnoticeだけで置換しない。次requestだけが最新preparedの新しい有効順を使う |
| current entryをremove | 元fileと現在再生を維持。次requestは最初の適格候補 |
| remove後に同sourceを再追加 | source keyで新entryをanchorにし、その次。checked権限は移さない |
| mIV内rename / move | migration後のstable entry IDをanchorにし、新pathを開く |
| 別collectionを編集 | pendingも再生も変更しない |
| sourceがprepare後に失踪 | typed open failureとして同revisionの次candidate、または新noticeがあれば再prepare |
| collection delete | 現在再生を即時closeしない。次操作でtarget無しとして停止 / 境界表示 |
| actor / prepare failure | stale順へfallbackせず、現在presentationを保ちintentに合う停止 / error表示 |
| context switch / retire | そのcontextのpendingだけcancel。兄弟contextのplayer / pendingを変更しない |

collection edit noticeを受けて `close_fullscreen`、`load_folder`、player stop、slideshow stop、root rows installを呼ばない。noticeは wanted revisionを進めるだけである。

## 9. 実装単位と主な変更候補

1. `src/collection_store/{model,prepare}.rs`
   - prepared snapshot直接入力のpure resolver、direction / target / tail / resolution kind。
   - 既存raw snapshot + parallel facts APIを置換し、ID集合不一致を呼出側で作れない形にする。
2. `src/app/top_level_grid_view.rs`、`src/app/collection_grid.rs`
   - root / physical child / detached returnからtyped originを取得するhelper。
   - exact preparedをnavigation commitで一度だけmaterializeする入口。
3. `src/app.rs`、`src/app/viewer_context_registry.rs`
   - context-owned pending state machine、watch / cancel / current gate、bundle lifecycle。
   - Grid Ctrl producer、manual media next / prev、三つのmedia EOF producer、既存apply handlerへのtarget commit。
4. `src/ui_fullscreen.rs`
   - fullscreen通常page next / prev、fullscreen Ctrl producer、slideshow通常送り / NextFolder producer。
   - nav lock、holdover、slideshow timer / resumeをtyped intentへ接続。
5. Phase 5では `src/remote_ipc` とprotocolを追加するが、Phase 4でRemote product閲覧はまだ接続しない。

`docs/detached-rework-plan.md` §11は、bundle fieldとdetached origin/current gateを実装した時点でstructural changeとして記録する。detached predicateへの症状guardは追加しない。

## 10. 回帰計画

### 10.1 pure resolver

- Manual exact順、Standardの4行 + name/date/size/numeric各昇降順。
- ID anchor、ID消失 + source key anchor、両方消失 + head。
- Forward / Backward、Stop / Loop、currentが末尾に残る場合とremove済みの場合。
- Navigable / StillImage / Image / Video / Audio抽出、video-audio modeがVideoを使うこと。
- 見開きprimary / partner双方生存、片方remove、双方removeについてForward後端 / Backward前端 / headを使い、partner再表示や1枚歩幅への退行が無いこと。
- container抽出、placeholder / direct media除外、tried集合、0件。
- eligible containerまたはdirect mediaがN件なら同じexact revisionのpreflightは最大N回で終わり、同じIDを再試行せず、revision更新時だけ上限とtriedを作り直すこと。
- raw snapshot / UI filter / parallel index配列を入力にしないこと。

### 10.2 handler / race

- Grid root、root leaf fullscreen、same-context child、detached childのCtrl+上下。
- root leafのpaged image / video / audio / native manual next / prevがlatest mixed-media順を使い、childは既存内部順を使う。
- manual delta複数step、Forward / Backward端、current remove→head、連結読みのstill-only境界。
- root直接登録画像の見開きmanual / slideshowでdisplay unit identityをcaptureし、片側removeを含めdirectional anchorと既存spread landingを維持する。split同一source内移動はrequestを作らない。
- collection中だけ物理DFSへ逸れず、通常Folder / Search / Smart / Snapshotは既存routeを維持。
- Ctrl連打はstepごとにlatest revisionを取り直し、反対方向 / mode変更で正しく置換。
- slideshow direct Imageの各tickでreorder / add / removeを反映。child内は既存順、NextFolderだけouter順。
- 再生中のManual↔Standard切替とStandard sort変更はcurrent player / root itemsをnoticeで置換せず、次のmanual / slideshow / EOF / Ctrl requestだけが新順を使う。
- video、video-audio mode、music EOFのContinuous / ContinuousLoop、0件 / 1件 / current remove。
- request中の同collection revision進行は再prepareし、別collection noticeは無視。
- watchを確立してからloadをenqueueし、その間のcommitを取りこぼさない。snapshot、prepare、container preflight、commitのrequest / context / collection / revision mismatchを拒否。
- 編集だけではplayer、fullscreen、slideshow timerを停止しない。target commit時だけ遷移。
- collection delete、actor unavailable、prepare error、source失踪でstale targetを開かない。
- ParkedLive、main / detached同時再生、context park / remount / retireで兄弟へcompletionが入らない。
- Snapshot / Preparing / direct existence / container preflightの各pendingをreplacement / revision restart / retire / exit / Dropでcancelし、commit後だけtyped landing ownerへterminal責任を一度移す。
- historyは通常の次 / 前とCtrlがUserChosen、slideshow / NextFolder / 三EOFがAutoAdvance。
- ZIP / PDF / convertible、nested ZIP、book内部順、Stop / Loop、native presentation / audio modeが既存どおり。

### 10.3 gate

focused pure / handler / viewer-context lifecycle test、`cargo check -p mimageviewer --bin mimageviewer-core`、`cargo fmt --check`、`python scripts/check_ui_glyphs.py`、viewer-context audit、`git diff --check`、`scripts/test-full.ps1`、`scripts/build-dev.ps1 -PreserveRuntime`を同じsource freezeで行う。GUIは別途、具体scenario・所要時間・使い捨てdataを提示して利用者の明示承認を得たsuiteだけ実行する。

## 11. 実装開始 gate

本書の非同期境界について、実装担当とは別の Sol / xhigh reviewerが少なくとも次を承認するまで製品コードを編集しない。

- root表示更新とnext解決を別request ownerにすること。
- originをentry ID→source key→headで解決し、child mediaとouter entryを分けること。
- pending completionのcollection / context / exact revision / intent-current gate。
- edit noticeでは再生を中断せず、pendingだけを最新へ収束させること。
- common pure resolverがPC UI stateを含まず、Phase 5 Remoteから再利用できること。

独立レビューのfindingと合意した修正を本書へ反映し、review checkpointを追記してからPhase 4製品実装へ進む。

### 11.1 独立設計 review checkpoint（2026-09-15）

実装担当とは別の Sol / xhigh reviewer が最終合意した。Grid root load から独立した viewer-context typed owner、watch→actor load→exact prepare→pure resolve→有限 preflight→単一 commit、全 pending variant の cancel / drop と landing owner への一回だけの terminal 移譲を確認した。entry ID→identity 固有 source key→head、root direct media / physical child / outer container の分離、Manual↔Standard / Standard sort 編集を次要求だけへ反映する規則にも合意した。

通常 paged / continuous / native manual、見開き display unit、slideshow、video / video-audio / music EOF の producer、dedup、history trigger を回帰範囲とし、book / ZIP / PDF child の固定順を保護する。PC と Phase 5 Remote は UI index / filter を読まない同じ prepared pure resolver を使う。本 checkpoint により Phase 4 製品実装を開始できる。

### 11.2 製品実装・独立 completion review checkpoint（2026-09-15）

`CollectionPreparedSnapshot`を直接読むpure resolverへentry ID→source key→head、direction、Stop / Loop、
media kind、outer container、見開きdisplay-unit anchor、tried IDの有限走査を実装した。PCのGrid / fullscreen
Ctrl+上下、通常paged / continuous / native next・prev、slideshow通常送り / NextFolder、video / video-audio /
music EOFを、viewer contextごとのtyped requestへ接続した。watch→actor load→exact prepare→resolve→cancel-aware
preflight→single commitの順を守り、編集noticeは現在presentationやroot rowsを置換せず、次requestだけが最新順を
採用する。navigation materializeでもlocal query / filterをstable identityで保持する。

folderは最終scan payload、ZIP / PDFは実列挙payload、暗号PDF / convertibleは既存password / conversion ownerへ
terminal責任を移譲する。current presentationを先に閉じず、失敗候補は同じexact revisionで一度だけskipする。
container child順とouter順を分離し、same-context / detached collectionのnested ZIPはchild DFSを先に、tailだけ
outerへ進む。direct mediaは既存fullscreen / native source swap / video-audio / ParkedLive landingへ戻し、manual
連打は着地中もbounded queueへ保持して一stepごとに最新snapshotを取り直す。

独立Sol / xhigh reviewerのcompletion reviewでは、commit直前revision/current barrier、close / stop / surface replace /
ABA、既存EOF landing、container payload / password、全pending cancel / drop、filter保持、ZIP child優先、native
Boundary / landing queue、delayed conversion owner、Grid selectionと同revision再materialize raceを順に照合した。
各findingをhandler回帰とともに修正後、製品sourceにblocking / should-fixなしの承認を得た。Phase 5 Remoteと
rating sort製品変更は本chunkへ含めていない。

focusedはcollection navigation 14/14、prepared resolver 8/8、collection Grid 14/14、core checkがpassした。
final `RUST_TEST_THREADS=1 scripts/test-full.ps1 -SuppressCrashDialogs`は本体8530 passed / 0 failed /
45 ignored、UI snapshot 52/52、vendor egui / egui-wgpu / eframe 25 / 9 / 15、`[test-full] PASS`、
exit 0でprocess error modeを`0x00008001`へ復元した。ログは
`target/collection-phase4-final-20260915/test-full.{stdout.log,stderr.log,exit.txt}`、SHA-256は順に
`13541E7C82B403B229787E5AF7D97A51E24EBA73C7DF14FAAE4A9A48B75D26B5`、
`ED648F190BE0D09280BEC80E87C4B2897562365E95D2B918479DDE69FE4D8BEE`、
`13BF7B3039C63BF5A50491FA3CFD8EB4E699D1BA1436315AEF9CBE5711530354`である。

同じ製品sourceでfmt、UI glyph、viewer-context audit、diff checkもexit 0。staticログのSHA-256はstdout
`3E016394B8A58CA0495DCABC1600314F92395A983F58085E313A6B11CE0AE7A4`、stderr
`3C118FCF903FA9E4E36E33EEBF1495B0848EE6DA8814CC9312312C0F9E2FE4EE`、exit
`13BF7B3039C63BF5A50491FA3CFD8EB4E699D1BA1436315AEF9CBE5711530354`。
`target/dev-runtime`には2026-09-15 12:23開始のcore 8 processとRemote 1 processがresidentしていたため、
それらを停止せず`build-dev.ps1 -PreserveRuntime`は保留した。アプリ起動、GUI操作、通常profile / real dataへの
アクセスは行っていない。

### 11.3 同一root再生時のpresentation再利用（2026-09-16）

linked別窓の通常next / prev / EOFは、actorのlatest snapshotをprepareしてtargetを解決する一方、同じimmutable
rootを毎回Gridへinstallしない。installed rootとprepared rootのordered entry ID / source key / path / availability /
item identity / display metadata、およびprepare workerがsidecar path・metadata / pin blob SHA-256から作る固定長の
thumbnail source identityが完全一致する場合は、installed
binding上でtarget / originだけを解決する。main一覧のitems generation、Autoサムネ比率、scroll、thumbnail cache /
queue / cancel tokenを変更しない。filesystem factsまたはthumbnail sourceが変わった場合は同revisionでも再installし、
actor revision / order / source変更も従来どおりlatestを採用する。

thumbnail payloadのWebP bytesはinstalled bindingへ保持・複製せず、再install時にthumbnail workerへmoveする。commit時の
source比較は固定長identityだけを比較し、大量動画のbytes走査をUI threadへ持ち込まない。同じsidecar pathの内容更新も
size / modified metadataでidentityが変わり、pin更新はblob SHA-256で変化する。

actual `start_collection_manual_navigation`→snapshot→prepare→preflight→commit回帰で各poll frameのGrid generation /
cache owner / Auto比率を固定し、split bundleはdetached側だけを再installしてmainを変えないことを固定した。これは
auto-aspect値の退避復元ではなく、viewer cursor移動とGrid presentation更新の所有を分ける変更である。

## §1.267 別ウィンドウで開いたコレクション直下の画像（実装前設計、2026-09-23）

### 観測・所有モデル

- 第1層シナリオはfull modeのA→B→A→Bが成功し、同じcollection rootからの別窓next / prev / slideshowの3本は物理フォルダ順へ逸れる（`src/app/multiwindow_scenario_tests.rs:899-939,1042-1109`）。別窓Grid入口はsource cellから`CollectionGridRestore`をcaptureするが、`Image` descriptorは親フォルダscanを開始し、その結果の物理`items`から対象を開く（`src/app.rs:46709-46745,46626-46640,41361-41397`）。restoreのinstallはscan開始後で、root `CollectionGridSession`にはならない（`src/app.rs:46654-46658`）。
- **採用するowner**: collection rootの直接登録Imageを新しい別窓で開く場合、別窓の`ViewerContextBundle`自身に`TopLevelGridSurface::Collection`、`CollectionGridPosition::Root`、同じimmutable preparedの有効順にalignedした**全entryの`items`**、その別窓の`items_generation`へbindしたinstalled session、stable entry ID + source keyの開始anchorを持たせる。表示するのは対象Imageでも、一覧ownerはcollection rootである。親物理フォルダの`items` + `TopLevelGridRestore::Collection`をleafへ与える案は採らない。restoreはcontainer **childからrootへ戻るため**のownerであり、直接mediaのresolverはroot sessionと一致するitems generationを要求する（`src/app/collection_grid.rs:823-865,2051-2073,2121-2137`; `src/app/collection_navigation.rs:869-913,958-978`）。
- full modeはcollection prepareをroot `items`へinstallしたまま`open_fullscreen(index)`し、root originを取得できる（`src/app/multiwindow_scenario_tests.rs:843-859,913-920`; `src/app/collection_navigation.rs:884-913`）。新別窓でも同じ表示・再生モデルを別のcontext IDで所有する。sourceは主窓の現selectionを後で読み直さず、Grid入力時のinstalled prepared / `items_generation` / entry identityをtyped open planとしてcaptureする。別窓factoryは新しいcontextへimmutable presentationを共有し、一覧・選択・再生cacheとwatchは独立させる。既存splitは`items`と`top_level_grid_view`を複製し、session cloneはinstalled preparedだけ共有してreceiver / watch / pendingを複製しない（`src/app/viewer_context_registry.rs:2515-2568`; `src/app/top_level_grid_view.rs:1005-1025,1202-1216`）。その契約をroot leaf専用のtyped factory入力で再利用し、物理scanの`DetachedImage`には通さない。
- fresh forkで旧mainの`installed_items_generation`を盲目的に継承しない。cloneされた`items`と新contextのgenerationを同時にinstall / bindし、entry ID→source keyで開始indexを確定してからfullscreen openをcommitする。stale root binding、対象不在、prepare/open失敗はtyped terminalとして未公開buildをabortする。主窓のcollection session、selection、scroll、worker / queueを変更しない（`src/app/collection_grid.rs:402-428,2059-2073,2121-2137`; `src/app/viewer_context_registry.rs:3267-3337`）。一覧が大きい場合もUI threadでDB・物理scan・再prepareを行わず、共有済みpreparedを使う。

### producerと同型入口

- 通常next / prevは`adjacent_page_navigation_idx`の前に`start_collection_manual_display_unit_navigation`、動画・音声のmanualは`start_manual_media_navigation`の先頭から`start_collection_manual_navigation`へ入る。現在のroot判定が別窓で成功すれば、既存latest prepared resolverを使える（`src/ui_fullscreen.rs:34343-34379,34514-34541`; `src/app.rs:72892-72920`; `src/app/collection_navigation.rs:1178-1265`）。別窓用のindex順resolverは追加しない。
- Home / Endは`fullscreen_boundary_jump_target`が現在のnav indicesの先頭 / 末尾を返し、`jump_to`はcollection manual分岐を通らず直接着地する（`src/ui_fullscreen.rs:29580-29586,33625-33672,34364-34390`）。別窓root `items`にcollection有効順を保持すれば物理順への逸脱は消えるが、編集中の**最新順**を要求するかは既存full modeと同じ現行仕様として明記し、実装時に同条件のfull/detached回帰を追加する。見開きpair / split / final coverは`get_nav_indices`と`items`から構築され、実際の次表示単位は既存stable primary / partner captureを通る（`src/ui_fullscreen.rs:26681-26726`; `src/app/collection_navigation.rs:869-905,1221-1232`）。root列の変化がpairと歩幅へ反映されることを検証する。
- slideshow tickはrootなら`start_collection_slideshow_navigation`を先に試し、末尾のNextFolderとCtrl+上下は`collection_outer_navigation_origin`でrootまたはchild restoreからouter containerを選ぶ。rootではcollection order、child内は既存の内部順 / ZIP DFSを維持する（`src/ui_fullscreen.rs:33083-33116,33198-33242,33829-33848`; `src/app/collection_navigation.rs:916-978,1290-1307,1364-1395`）。video / video-audio / music EOFもroot origin取得後に既存typed intentへ入る（`src/app.rs:73181,73315,73609`; `src/app/collection_navigation.rs:1414-1472`）。
- BSは現在のfullscreenを閉じ、Esc等のreturn-to-gridは独立別窓sessionを終端する。container childからの親移動だけ`TopLevelGridRestore::Collection`によって最新rootへ戻す。主窓のcollection Gridを物理親フォルダへ置き換えない（`src/ui_fullscreen.rs:34226-34276`; `src/app.rs:58619-58655,23907-23910`; `src/app/collection_grid.rs:823-860`）。タイトルは別窓自身の`items[fs_idx].name()`、collection locationはroot install時の`コレクション: <name>`を使う（`src/ui_fullscreen.rs:26405-26424`; `src/app/collection_grid.rs:2121-2127`）。
- 同型入口を区別する。collection直下のVideo / Audioは`Image` descriptor対象外で、full rootからのopen / media forkが`items`と`top_level_grid_view`を複製する経路を使うため、root session / EOF / manualを回帰確認する（`src/app.rs:43374-43395`; `src/app/viewer_context_registry.rs:2515-2568,2967-2971`）。Folder / ZIP / PDF / convertibleは**child**を開くので既存`TopLevelGridRestore::Collection`をscan / enumerate / conversion ownerへ運ぶ（`src/app.rs:46717-46744,46880-46919,46921-46953`; `docs/collection-implementation-plan.md:748-761`）。linked別窓とF12は既存context / root-originを保つpresentation切替・fork経路として確認し、新しい物理scan入口へ振り替えない（`src/app.rs:67573-67618`; `src/app/viewer_context_registry.rs:2515-2568`）。Ctrl+G / rating / historyのImageにはcollection root sessionがないため、従来の親物理フォルダscanを維持する（`src/app.rs:46626-46640,46733-46744`; `src/app/collection_navigation.rs:884-913`）。

### lifecycle、gate、検収

- `build_viewer_context`の予約→commit→window binding公開の後だけdetached sessionを開始する。mount / deposit / remountは同じbundleのroot session、`items`、fullscreen index、pendingを一体で運び、parked siblingを巻き込まない。retire / surface replace / Dropはそのcontextのrequest、worker、watchを終端する（`src/app.rs:46675-46688,19056-19069`; `src/app/viewer_context_registry.rs:908-909,1305-1315,3267-3337`; `src/app/top_level_grid_view.rs:1186-1216`）。
- passive静止画の再活性化は、同期stampで元indexを解決できなければ保存済み`Image` descriptorから**物理親フォルダ**を再openする（`src/app.rs:43402-43406,47266-47280,47333-47391,47987-48015`）。collection root leafだけはそのfallbackでもoriginを失わないtyped reopen payload（collection ID、entry ID、source key、開いた時のimmutable prepared / source path）をsnapshotへ持たせ、同じroot factoryで再構築する。collectionが編集 / 削除されても現在画像は保持し、次要求だけがlatest / deletedを判定する。旧`usize`を新rootへ流用せず、元source自体が開けなければparked表示を保つterminal failureとする。物理`Image` descriptorへのsilent fallbackはしない。既存window ID / placement / viewport leaseは再活性化経路のまま維持する（`src/app.rs:47388-47391`）。
- **poll境界を実装で確認・接続する**: 現在の`App::update`入口はmainだけで`poll_collection_grid/navigation`を呼ぶ一方、active detached updateは自身をmountして物理scan / video等をpollし、parked-liveはvideoだけをpollする（`src/app.rs:77044-77054,47487-47549,45595-45613`）。root leafを別窓へ載せるなら、そのbundleのcollection watch / navigation pendingをactiveでpollし、ParkedLiveで再生を続けるmediaのpendingもownerのもとで進める。mainからdetached pendingをdrainせず、parked静止画に不要な常時pollを設けない。read leaseの起床 / terminalとviewport lifecycleは既存の契約を使う（`src/app/collection_navigation.rs:1798-1835,2288-2335`）。
- edit noticeはそのcollection IDのwanted revisionだけを進め、表示中のroot `items` / fullscreen index / player / slideshowを即時置換しない。次manual / tick / EOF / Ctrl要求はwatch→actor load→exact prepare→preflight→commitでlatestを採用し、同collectionの新noticeが来た旧resultは破棄・再準備する。削除時も表示中leafは維持し、次操作をtargetなしとして終える。rootへ戻った時だけ空/最新行をinstallする（`src/app/collection_grid.rs:1612-1669`; `src/app/collection_navigation.rs:2288-2335,2786-2820`; 本書`§6-8`）。context ID、surface generation、collection ID、revision、items generation、intent sequenceのcurrent gateとDrop cancelをmain / detached / siblingsそれぞれで検証する。
- 検収: `src/app/multiwindow_scenario_tests.rs:1087-1109`の3本の`#[ignore]`を外してpass、`1042-1064`のfull-mode controlもpass。root別窓のHome / End、見開きmanual / slideshow、NextFolder、Ctrl+上下、video / audio EOF、BS / close、編集 / 削除、park→remount、複数collection窓のstale result隔離をfocused handler / lifecycle回帰に加える。container childとCtrl+G / rating / historyの物理順も保護する。detached predicate、HWND / viewport identity、placement、focus、mount / park / retireの**判定条件**は変えない。
- 分類は**BA-7（viewer contextの一覧とorigin ownerの分裂）**。誤った一覧を作るopen producerでcollection rootを同一bundleへ結び、全consumerが既存root resolverを読めるようにする構造修正である。別窓だけのguard、順序上書き、遅延、再試行、App-global collectionフラグは置かない（`docs/detached-rework-plan.md:29-65`; `CLAUDE.md:62-90`）。実装前に設計leadと独立reviewerがこの分類とpoll / fork境界へ合意し、実装時に触れた範囲と判断を`docs/detached-rework-plan.md` §11へ記録する。

### 独立レビューの確認点

- root leaf用typed factoryを既存`MaterializedStill` forkの拡張とするか、fresh `build_viewer_context`へimmutable presentationを渡すか。どちらでもmainのworker複合体を移さず、別窓のitems/session generationを一度で一致させる必要がある（`src/app/viewer_context_registry.rs:2244-2258,2794-2868,3267-3337`）。
- passive再活性化でprepared / source pathを保持するtyped payloadの配置と、ParkedLive mediaのcollection pendingをownerでpollする位置。現在のdescriptor fallbackとpoll入口が物理Image / main中心なので、ここを回帰対象として設計合意したい（`src/app.rs:47333-47391,45595-45613,77044-77054`）。

### 独立レビュー結果と確定事項（GPT-6 Sol xhigh、2026-09-23）

レビュー担当は**構造修正（BA-7）であることに合意**した。detached predicate、viewport / HWND lifecycle、placement、
focus の変更は不要。設計担当（Opus）が指摘をコードで確認し、以下を確定とする。

- **[P1] 新 bundle の navigation scope を collection root の意味にする。** 既存の detached image builder は
  `DetachedPhysical` を設定し、visible-index の再構築で 1 つの物理フォルダの項目だけを残す（`src/app.rs:46553,52699,53736`）。
  複数フォルダにまたがる root 一覧ではこれが Home / End・見開きなど `visible_indices` を読む処理から項目を落とす。
  full mode の root と既存 media fork と同じ reader order を保つ scope を使い、scope に依存する分岐を実装前に洗い出す。
- **[P2] passive 再活性化の typed payload は、活性化以外の consumer も更新する。** snapshot の descriptor と sync stamp は
  reopen identity の有無、右ドラッグ入力の識別、元パス削除の検出にも使われている（`src/app.rs:613,895,1578,11943`）。
  コレクション削除と元ファイル削除は別の期待値として扱う。
- **[P2] F12 の検証はモード別。** 「常に新しいウィンドウで画像を開く」では F12 が無効（`src/app.rs:67568`）。
  F12 が有効なモードで collection origin が保たれること、常時新規モードでは既存の無効動作を確かめる。
- **確定 (a) factory**: 既存 `MaterializedStill` fork は拡張しない（物理 scope に絞り surface を `Folder` に置き換えるため、
  `viewer_context_registry.rs:2794,2841`）。**fresh `build_viewer_context` に typed collection-root open plan を渡す**。
  Grid 入力時に installed immutable presentation と stable entry identity を capture し、予約した context の新しい
  `items_generation` に root session を bind してから解決済み index を開く。main の worker・selection は継承しない。
- **確定 (b) reopen と poll**: typed reopen route は、静止画 snapshot を作る時点で mount 済み bundle から
  `DetachedImageWindowSnapshot` に capture する。活性化は現在の main-index stamp と物理 image fallback より**前**に
  この route を dispatch する。live media の collection grid watch / navigation は各 owner の mount 内で poll する
  （active detached は既存 worker poll の隣、`ParkedLive` は `poll_video` の後で EOF 由来の要求を進める）。
  parked 静止画には定期 poll を置かない（`src/app.rs:47987,47218,47256,47523,45595`）。
- **追加する検収**: factory の不変条件（完全な順序、新 generation への bind、main 状態不変、物理 scan なし）、
  AtRest bundle が無い状態を強制した passive 再活性化、owner-mounted poll の編集 / 削除・兄弟窓の stale result・
  `ParkedLive` 動画 / 音声 EOF、BS / Esc・タイトル・child restore・実際の Grid 入力経路。lifecycle と兄弟窓のテストでは
  window identity、placement、focus、viewport の動作が変わらないことも確かめる。

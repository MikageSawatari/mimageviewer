# §1.337 動画再生時の EffeTune 自動表示 設計案

2026-10-08、ライン C。**設計承認・利用者仕様決定済み、§1.337 実装済み。Rust検証成功、SDK配置後のhost／検証用ビルド待ち**。
正本: [バックログ](next-release-backlog.md) §1.337、
[EffeTune 統合](effetune-integration-plan.md) §0・§4・§10.1・§14、
[動画アーキテクチャ](video-architecture.md) と [detached 憲法](detached-rework-plan.md#2-憲法-全ステージ共通の不変条件禁止事項-最重要)。

## 決定済みと実装前のコード上の前提

- 利用者決定: **起動後の最初の適格なローカル動画再生で 1 回、全画面中は開かない、既定 OFF**。
- `VideoPlayer::is_playing()` は play intent と別。`src/video/mod.rs` の説明と
  `EngineActor::transition_to_playing` (`src/video/engine/actor.rs`) から、readiness 後の
  Playing が再生成功を示す。open 要求、autoplay=true、VideoInfo 到着、静止した先頭フレームは成功ではない。
- `App::poll_video` は通常・active viewer・ParkedLive の既存 mount 経路から呼ばれる。
  `FsCacheEntry::Video` には音声も入るので、entry 型だけでは動画判定できない。
- `App::poll_effetune` は Loading の `open_gui_when_ready: Option<ShowPermit>` を引き継ぎ、
  `request_show_gui_with_permit` へ送る。Idle のロード、Running の表示、host-control worker と
  hidden attach／ACK は既存構造を使える。**自動表示専用の再生成功通知は現在ない**。
- 現在の ShowPermit／GuiGate は最小化連番・Remote 取得連番を検査するが、**全画面を検査しない**。
  `DspBridge::show_slot_gui_checked` は `AllowSetForegroundWindow(host_pid)` を呼ぶ。
  既存表示をそのまま呼ぶだけでは、今回の全画面制約と前面化方針を満たせない。
  以上はソース調査。Visualizer の画面保持・Windows の実際のフォーカス挙動は未確認。
- R2コード照合: normalizeは `start_normalize_scan_inner` (src/app/native_video.rs:7850) で
  `set_playing(false)`、仮測定 (:7992)／完了 (:8061)／失敗 (:8111) で `set_playing(true)` を使う。
  取消しowner復帰 (:7609)、scan開始失敗、deferred scanを開始しない場合にも内部再開がある。
  `handle_native_video_toggle_play_command` (:7100) はユーザーplayをnormalizeへ委譲して早期returnする。
  **Playingへの遷移やset_playing呼出しだけでは再生開始の由来を判別できない**。
- trayは `hide_to_tray` (src/tray_integration.rs:278) で `window_visible=false` → `SW_HIDE`、
  復帰はtray threadのOS表示後に `sync_after_restore` (:355) でAppへ反映する。
  EffeTune observer (src/effetune/window.rs:188) の連番は `WM_SIZE/SIZE_MINIMIZED` でのみ進む。
  現在のGuiGateに自動設定ON／tray可視性のprojectionはなく、これらの往復は最小化連番で検出できない。
- R3コード照合: `GuiGate::note_minimized` (src/effetune/gui_gate.rs:110) は最小化連番だけを増やす。
  `SessionStateMachine::transition_lifecycle` (src/remote_ipc/session.rs:290) はBeginAcquireで取得連番を
  増やし、phaseとともに `publish_remote` へ送る。両者はR2案の3要因projectionとは別更新であり、
  成功後・poll前の最小化／Remote往復を成功時のprojectionだけでは検出できなかった。
  下記では両抑止も同じ自動専用projectionへ含め、既存の手動表示連番・表示済み窓の契約は維持する。

## 発火点と一度だけの所有者

**自動表示を決める場所は `App::poll_video` の再生結果集約後の 1 か所**にする。
各 open、キー／HUD の play、fast-swap、resume、EOF 連続再生には表示処理を足さない。

1. 下記の**型付き再生開始由来をEngineActorが所有**し、唯一のPlaying確定境界で
   未確定の論理startを成功にする。VideoPlayer経由でsource／viewer identity付きの成功をpollへ渡す。
   start IDはseek epochと別で、内部再開・buffering・DSP handoff・トラック切替・周回では増やさない。
   単なるstateの毎frame比較では、全画面解除やnormalize再開を初回と誤認するため採らない。
   未採用cache／破棄済みsourceの成功は表示へ流さない。通知は下記条件で**一度だけ消費**する。
2. controller が持つ起動セッション内の単一 `AutoOpenSession` (Armed / Spent) で決定する。
   viewer ごとの「表示済み」や settings 内の「今回済み」は作らない。複数窓の成功も 1 回に集約する。
   設定 OFF や不適格な成功はその場で棄却し、復帰時の待ち行列に残さない。
3. 適格なら**ロード／表示要求を出す前に Spent**へ移し、二重発火を防ぐ。
   Idle は既存の実行中ロード経路、Running は既存の checked show、Loading は既存の
   ロード完了表示意図に合流する。手動表示意図を自動意図で上書きしない。
   Unavailable／Failed では自動ロード再試行をせず既存の理由表示を維持する。
   自動試行の失敗／途中取消でも Spent のまま。窓を閉じた後や次動画で開き直さない。

### 再生開始の由来と成功通知の消費 (R2追加)

- 所有者はplayerごとの既存transport ownerである `EngineActor`。その中に単一の型付き
  `PlaybackStart` (None / AwaitingSuccess{id, origin} / SuccessReady{id, origin} / Established{id, origin})
  を置く案とする。AppやNormalizeScanStateに自動表示用pending／再開判定boolを分散しない。
  originはNewSource／UserPlay／ContinuousAdvance。新規autoplay open、ユーザーのpause・EOF後のplay、
  別sourceへの連続進行でのみ新しいstartを作る。paused openは成功待ちを作らず、後のUserPlayで作る。
- 起動・キー・HUD・cached player再利用・EOF入口でtransport要求に**明示的な由来**を渡す。
  一般の `set_playing(true)` をUserPlayとみなす既定／fallbackは作らず、同型call siteを全て分類する。
  ユーザーplayをnormalize scanへ委譲する前にも同じstartを登録し、scan後の再開はそのstartを継続する。
  実際の表示処理は入口で呼ばない。ユーザーpause／close／source置換は未消費のstartを終了／破棄する。
- normalizeのpauseと全復帰経路 (仮測定、完了、失敗、取消し、supersede、worker開始失敗、
  deferred scan不開始) は **InternalContinuation::Normalize** として既存startを保持する。
  seek、DSP取得、音声トラック切替、loopも内部継続として扱う。内部pauseはユーザーpauseに読み替えない。
  初回playがscan待ちならAwaitingSuccessが維持され、初めてPlayingになったときだけ成功を生成する。
  既に成功済みの継続再生を測定した場合はEstablishedのままなので、復帰しても成功を再生成しない。
- `SuccessReady` はpollで取り出すと**適格性に関係なくEstablishedへ遷移**する。
  設定OFF／全画面／tray／最小化／Remote／対象外／Spentでもdrainして棄却する。
  設定ON時だけ成功を読む早期returnは不可。判定時に現在source・viewer bindingが一致し、
  close／置換／ユーザーpauseを経ていないことを確認する。通知を次の復帰まで保持しない。
  成功事実にはPlaying確定時の自動表示projection (下記の**5要因**) のrevision／allowedも添付する。
  transport ownerへ注入したread-only共有値を同境界で読むだけで、GUI操作や追加lockは行わない。
  poll時も同revisionで適格な場合だけ採用し、OFF中の成功を未drainのままONにした場合や
  成功→抑止往復→pollの場合も棄却する。最小化→復元、Remote取得→drain完了もrevisionが進むため含む。
  候補条件はopen要求時でなく再生成功時に採取する。成功時にblockedなら復帰後もその事実を棄却する。
  「OFFで動画成功→ONへ変更→同じ再生中にnormalize」は成功済みstartの内部継続なので発火しない。

## 適格性・非同期表示の境界

| 条件 | 決定した仕様 |
| --- | --- |
| 実ファイル動画の新しいローカル再生成功 | 設定 ON、表示可能な通常窓、EffeTune 利用可能なら候補 |
| 音声ファイル、動画→音声モード、RemoteHeadless／配信処理 | 対象外。音声ファイルへ拡張しない |
| F11 全画面 | この開始は棄却。解除しただけで後から開かない |
| 最小化・tray-only 非表示 | この開始は棄却。復元しただけで後から開かない |
| Remote の取得中・所有中・drain 中 | 正本 `remote_session_blocks_local_control()` で棄却。解除時は発火しない |
| portable | 実行しない。設定と設定検索の候補を表示しない |

不適格な開始は Armed を消費せず、**次の別の再生開始成功**を待つ。同じ再生の通常窓への復帰、
seek、設定を ON にしただけでは再生開始通知を生成しない。これは遅延 popup ではない。
再生と関係ない他窓の cache を走査して候補にしない。

ロード中／hidden attach中／host配送後に**設定OFF・tray格納・全画面・最小化・Remote**へ入った場合は、
未表示のAutoVideoだけを取消し、解除／ON／復元で復活させない。試行済みのAutoOpenSessionはSpentを維持する。
`ShowIntent::Manual(ShowPermit)` / `AutoVideo(AutoPermit)` にまとめ、同じLoading／host queueに流す。
AutoPermitは既存最小化／Remote連番の追加検査に加えて、**成功事実のprojection revisionをそのまま継承**する。
poll時・ロード完了時・再配送時の現在revisionで置換しない。成功時のallowedと同revisionの現在allowedを
確認できた通知だけ要求へ進める。古い通知の棄却はArmedを消費せず、次の別の再生開始成功を待つ。

| 正本の変更境界 | AutoVideo限定の公開契約 |
| --- | --- |
| 起動設定とPreferences OK等の全settings確定／復元経路 | 自動設定ONをprojectionへ公開。OFF受理と同境界でblockedとrevisionを更新し、load完了pollより後回しにしない。draft変更／Cancelは影響なし |
| `hide_to_tray` | `window_visible=false`の確定と同境界、`SW_HIDE`やsurface非表示の**前**にblockedとrevisionを公開 |
| tray復元／その他のmain可視性変更 | 既存native observerにWM_SHOWWINDOW／WM_WINDOWPOSCHANGEDの可視性観測を追加する案。現在のOS可視性を確認してprojectionを更新し、tray threadの復元でApp同期を先行させない。hide→showの往復でも古いrevisionは戻さない |
| 全viewerのpresentation ownerによる全画面入退 | 非適格化を同じ遷移境界で公開。決定済みC337-2の全ローカル閲覧窓を対象とし、context mount／swapで別窓の全画面情報を失わない |
| 最小化／復元 | 既存native observerのWM_SIZE境界でMinimized bitを公開。SIZE_MINIMIZEDでは既存note_minimizedに合わせてrevisionも必ず増やし、下流WndProc／worker通知より前に確定する。復元では現在のIsIconic(main)を確認してbit解除とrevision増分を確定する。keep_visible_when_minimizedによらず自動開始は抑止 |
| Remote取得／所有／drain／Local復帰 | transition_lifecycleと同じSessionStateMachineロック内でRemoteBlocked bitをphase.blocks_local_control()から公開。BeginAcquireで既存取得連番が進む境界ではrevisionも必ず増やし、解除でも増やす。set_gui_gateの登録／切離しも同ロック内で自動projectionを同期・失効させ、通知／UI次frameへ公開を遅らせない |

projectionの唯一の公開先はcontrollerの既存GuiGate。自動設定／root可視性／全画面／最小化／Remoteから導く
`AutoPresentation`を、SettingOff／RootHidden／Fullscreen／Minimized／RemoteBlockedの抑止bitと単調revisionを含む
**一つのatomic値**でhostと成功通知producerに共有する案とする。各正本ownerは自分のbitだけを
CASで更新し、別ownerの抑止を古い全体snapshotで消さない。allowedは抑止bitが全て0から導く。
複数atomicの別読みによる整合窓を作らず、Rust／C++のmapping versionも揃えて検査する。
bit変更とrevision増分は同じCASで確定し、再適格化しても過去値に戻さない。復帰は新しい自動要求を作らない。
最小化イベント／Remote取得の既存連番が進む場合は、対応bitが既に立っていてもrevisionを進める。
Remote独自の取得連番や別state machineは作らず、既存ownerの確定境界のread-only projectionとする。
初期設定・mainの可視性／IsIconic・登録中SessionHandleのphaseを各ownerが公開してから成功producerへ
共有値を注入する。再登録／切離しは旧projectionを失効させ、古い成功事実やAutoPermitを新gateへ移さない。
5要因を一度のatomic loadで読むため、成功時に別々の最小化状態・Remote状態・連番を組み合わせない。
既存の最小化連番／Remote tokenはManual・表示済み窓の処理とAutoVideoの追加検査に残すが、
それらをpoll時に採取し直しても成功時revisionの一致条件を代替できない。
host-control workerはロード完了時・hidden attach前後に照合し、host GUI threadは**表示直前**にも
一致・allowed・現在の `IsWindowVisible(main)`／`IsIconic(main)`／HWND生存を検査する。
取消ACKはrequested-visibleを作らない。Cancelledでロード／attach済みbridgeを作り直さない。
既存permitの最小化往復・Remote往復検査も維持する。検査後の抑止は新規popupの取消しではなく、
既に表示済みの窓の従来lifecycleに従う。表示済み窓やManualへの新たなhide理由を追加しない。
とくにtray格納による**既存表示済みEffeTune窓の保持**、最小化表示設定、Remote復帰の契約は変えない。
workerの一度の確認、minimize sequenceのtray代用、UI次frameだけの取消し、時間窓では済ませない。

**前面化に関する注意:** 自動開始は明示ボタンクリックと異なり、ロード完了時の foreground 権限を
保証できない。C337-1の決定 (利用者2026-10-08) により自動表示だけ**非アクティブ表示**とし、
foreground 許可／activate を要求しない。
既存の manual show と非アクティブ復帰を区別して host に伝える必要があり、bridge／C++ の変更が
見込まれる。TOPMOST・owner 変更・フォーカス奪還 retry は追加しない (既存 owner=0 を維持)。

## 状態の組合せを減らす検討

起動直後に窓を開く案は再生成功の要望を満たさないため不採用。毎再生時の open、復帰待ち popup、
自動retryを持たず、起動内の1 ownerと既存Loading／show意図へ揃える。
由来をnormalizeごとのpendingへコピーする案より、transport ownerの単一startを内部継続で保持する案を採用。
自動表示の設定／tray／全画面を既存窓のhide reasonへ足す案は、表示済み窓の挙動を変えるため不採用。
AutoVideo permitの失効projectionに限り集約し、取消し後のロードrollback・再attach・resume待ちを持たない。
R3では成功時に最小化／Remoteの別snapshotを加える案も検討したが、複数atomicの整合取得と
permitへの引継ぎ項目を増やすため不採用。同じprojectionへ5要因を集約し、成功から表示まで同じrevisionを使う。
再生や窓移動のモーダル化は通常操作を止めるため不採用。ロード・attach・表示 IPC は既存 worker、
UI は成功事実と gate の軽量更新だけにする。DSP 経路の起動後常時接続／保存契約は維持する。

## 利用者決定 (2026-10-08)

- **C337-1 決定 (利用者 2026-10-08):** 推奨案を採用。自動表示は非アクティブ表示とし、
  foreground許可／activateを要求せずキー操作を奪わない。
- **C337-2 決定 (利用者 2026-10-08):** 推奨案を採用。通常のF12別窓／複数窓の動画も対象。
  いずれかローカル閲覧窓が全画面なら自動表示を抑止し、presentation ownerの既存事実を使う。
- **C337-3 決定 (利用者 2026-10-08):** 推奨案を採用。設定ONなら未起動のEffeTuneを開始し、
  既存手動開始と同じく終了までDSPを経由する。ロード待ちで再生を止める新処理は追加しない。
- **C337-4 決定 (利用者 2026-10-08):** R2補足を含む推奨案を採用。成功した手動openは
  この起動の自動表示機会も消費し、手動で閉じた窓を初回動画で開き直さない。
  途中で自動設定をONにしても継続再生／normalize内部再開では開かない。
  自動要求後のOFF／tray等で取消された場合も消費済みとし、手動操作は残す。

**R2で新規の利用者質問はない。** C337-1〜3は維持、C337-4は既存の一度だけ／遅延表示なしの提案を
設定切替と内部再開にも明示した。由来所有と取消し公開境界は技術設計の修正で、利用者へ選択を委ねない。
**R3で新規・変更の利用者質問はない。** 最小化／Remote往復の検出は「遅延表示なし」を成立させる技術補完。
2026-10-08時点で未回答の利用者質問はない。今回の実装指示で、設計の独立レビュー承認と全質問の決定を受領した。

## 実装前後のレビュー・受け入れ

成功通知のlogical startとnormalize／seek／handoffの区別、全transport呼出しの由来分類、
全viewer producer／close／swap／cancel、Manual / AutoVideoの優先、設定／tray／fullscreenの公開境界と
host最終検査を含む設計は、利用者の実装指示で独立レビュー承認済みとして受領した。実装差分は別途レビュー対象とする。
detached／presentation 経路に入る変更は設計 lead と独立 reviewer が構造的修正として合意し、
detached-rework-plan.md §11 に記録する。今回の受領済み設計に従い、表示先の正本を読む projection 公開に限る。
非起動の state／fake host テストは open失敗→成功、paused open→play、各抑止開始→復帰→
次開始、ロード中の抑止往復、手動意図優先、二窓同時、設定OFF→ON、portable欠落を対象とする。
R2追加回帰: OFFで成功した継続動画→ON→normalize (仮測定／完了／失敗／取消し／開始失敗／不開始)
では0回、paused open／ユーザーplay→初回normalize→初めてPlayingでは適格時1回。
seek／DSP／loopの内部継続では追加0回。source置換／ユーザーpauseで旧成功を破棄、OFFでの通知drainを検証。
成功通知未drainでOFF→ON／各抑止往復した場合も0回、複数ownerのbit更新で別抑止が消えないことを検証。
R3追加回帰: Playing成功→最小化→復元→poll、Playing成功→Remote取得→所有→drain→Local→pollは
projection revision不一致で棄却し、表示要求0回・Armed維持。成功時に最小化／Remote blockedだった通知も
復帰後に棄却する。抑止のない成功では1回となり、AutoPermitのrevisionは成功時の値から変わらないことを確認する。
成功後のgate再登録／切離し、bit更新中の成功通知採取、ロード／host配送への引継ぎでも旧revisionを
現在値に更新しないことをstate／fake hostで検証する。最小化表示設定ONでもこの自動抑止は変わらず、
Manualと既存表示済み窓の従来挙動・連番検査は維持されることを確認する。
設定OFF／tray格納をload中・hidden attach中・host配送後の各段階で入れ、OFF→ON／hide→showの
抑止往復もCancelled、表示希望なし、Spent維持、後からpopupなしをfake hostで検証する。
Preferences Cancelは失効なし、既存表示済み窓とManualは自動専用projectionでhideされないことも確認する。
実装時は既存 settings.db へ既定 false の加算設定と serde default を追加し、旧データを保持。
環境設定「動画・音声 → 動画 → 音響調整」、spec、EffeTune正本、manual/effetune.html、製品ページを更新する。
bridge変更時の最終確認は release launcher/core build が必要。今回は製品を起動せず、実機挙動は検証しない。

R1レビューのP2「normalize内部再開の由来」とP2「要求後のtray／設定OFF取消し」を
コード照合して採用し、次の独立レビューで対応確認済み。R3のP2「成功通知未消費中の最小化／Remote往復」も
コード照合して採用した。上記の対応設計は、今回の利用者の実装指示により承認済みとして受領した。

## 実装記録 (2026-10-08、非起動のコード照合)

- PlaybackStart は EngineActor だけが所有し、明示的開始／内部継続 API を通す。
  reader と viewer binding は constructor 直後、tick／transport より前に注入する。
  constructor は Loading を作るだけで、decoder worker は actor を持たず、Playing はイベント適用時に確定する。
- App は不適格な通知も消費し、native の pause／close／source 結果の処理後に現行 start ID・path・viewer identity を検査する。
  context 移動前の未消費成功は移動先で破棄し、normalize で再通知しない。
  source-swap の HistoryTrigger と deferred native open の request origin が EOF の別 source を ContinuousAdvance とする。
  constructor の未確定 start を分類するときは ID を増やさず、同 source の巻戻しは Loop 継続とする。
- 全画面 projection は各 context の ViewerSession.presentation と現在の別窓の borderless fact から導く。
  受動別窓の既存 builder は decorations=true を要求するので、退避窓の全画面を geometry から推測しない。
  mount／swap 自体で revision を進めず、意味上の表示先・content・F11 変更境界で公開する。
- startup の inert 保存状態は bridge を作らず Idle に戻るが、Video／Audio の初回 cache-miss open は
  media_startup_load_pending() で player 作成前に待つ。Playing 成功がそのロードへ合流する経路はなく、
  AutoVideo 自身のロードは inert を skip しない。
- 成功した Manual ACK は自動機会を消費し、projection の revision だけも進める。
  配送済みで未表示の古い AutoVideo も取消し、Manual／表示済み窓の hide 理由は増やさない。
- この記録はソースと非起動テストの対象範囲であり、製品の表示・フォーカス挙動は未確認。

## 非起動検証とビルド前提 (2026-10-08)

- PlaybackStart 20件、EffeTune 40件、自動表示連携 10件、normalize 117件 (1件 ignore)、
  portable の設定／公開境界 4件と自動表示拒否 1件、全 UI snapshot 116件が成功した。
  通常／portable core check、cargo fmt と fmt --check、glyph lint (危険文字0件) も成功。
- 全 lib 初回は 11,022 passed / 1 failed / 52 ignored、exit 1。
  設定移行の分類には新設定を追加済みだったが、総数の期待値が447件のままだった。
  総数を448件へ更新し、設定移行17件は成功。製品側の移行仕様や期待値の許容範囲は変更していない。
  修正後の全 lib は 11,023 passed / 0 failed / 52 ignored、exit 0 (990.40秒、pipeなし)。
  再実行中のソース・fixture 29ファイルのhashは不変で、検証結果は固定した差分に対応する。
- C++ 表示取消しの22個の static_assert は MSVC の compile-only で成功し、製品は起動していない。
  完全な host ビルドは未完了。CMake configure は vendor/vst3sdk 未配置で exit 1、
  vendor host の source identity は今回のソースと不一致。公式 SDK アーカイブの取得もネットワーク制限で失敗した。
  SDK の実コピーをこの worktree へ配置し、C++ host を再ビルドして source identity を検証する必要がある。
  build-dev.ps1 は host を再ビルドしないため、この前提が揃うまで実行しない。
  現在の dev-runtime は本変更前の成果物であり、§1.337 の確認用には使わない。
- 詳細なコマンド・件数・失敗修正・未検証の実機シナリオは target/C-1337-verification.md。
  SDK 配置後の host／build-dev と変更 bridge の release launcher/core gate は未完了。
  実装差分の独立レビュー、Visualizer保持・Windowsフォーカスの実機確認も受け入れ前に残る。

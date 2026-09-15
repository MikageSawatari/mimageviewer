# 永続コレクション Phase 5: Remote 読み取り専用統合計画

状態: **製品実装・独立 source review・focused / full / static gate 完了。verification build は既存 resident を停止しないため保留**

作成日: 2026-09-15

関連文書:

- [コレクション仕様](collection-spec-proposal.md)
- [コレクション実装計画](collection-implementation-plan.md)
- [コレクション再生・最新順 navigation](collection-playback-plan.md)
- [Remote 全体設計](web-remote-plan.md)
- [Remote AI](web-remote-ai-plan.md)
- [Remote 動画配信](web-remote-video-streaming-plan.md)
- [非同期構造](async-architecture.md)
- [UI 応答性](ui-responsiveness.md)
- [全体構造](architecture-overview.md)

## 1. 目的と確定範囲

利用者が PC で作成・編集した名前付き永続コレクションを、認証済み mIV Remote から一覧表示し、
登録元を開いて閲覧できるようにする。Remote 初回版は読み取り専用であり、作成、名前変更、削除、
追加、除外、並べ替え、Manual / Standard 切替、import / export、再リンクを公開しない。

Remote が読む順序は PC と同じ immutable `CollectionPreparedSnapshot` の有効順である。

- Manual は保存済み manual position の一列をそのまま使う。
- Standard は collection definition の `standard_sort` と、その時点の `GridDisplayOrder` を使う。
- Folder、Image、Video、Audio、ZIP、PDF、変換可能archive、missing / unsupported / access errorを
  同じ登録順空間で扱い、利用できない登録を黙って削除しない。
- rootへ直接登録された media の次 / 前と動画・音声のEOFは、Phase 4と同じ
  entry ID → source key → head、対象媒体抽出、Stop / Loop規則を使う。
- 登録Folderや本を開いた後のchild内部順は既存の通常Folder / ZIP / PDF規則に従う。
  collection rootのManual / Standard順をchildへ流さない。

PCのtoolbar、Grid selection、filter、fullscreen index、viewer context bundleをRemoteへ共有しない。
Remote sessionはcollection ID、entry ID、source identity、prepared revision、表示中display unit、request
sequenceを独立所有する。UI threadでSQLite、stat、canonicalize、folder scan、archive列挙、file read、
worker待機を行わない。

Phase 4のPC向けCtrl+上下、slideshow、通常page送り、三媒体EOFは変更しない。Remoteには現在存在しない
Ctrl+上下、NextFolder、slideshowをこの段階で新設しない。rating順の製品変更も本計画へ含めない。

## 2. 現状の所有権と衝突点

2026-09-15のsourceを基準にした棚卸しである。

| 対象 | 現owner / 経路 | Phase 5で必要な境界 |
| --- | --- | --- |
| 永続定義とrevision | `CollectionStoreRuntime`の単一actor。writable `collection.db` connectionとimmutable snapshot cacheを所有 | Remoteも別DBを開かず、同じactor clientをtyped producer経由で読む |
| PC root表示 | `CollectionUiState` / `CollectionGridLoadState`がactor reply、prepare、materializeを所有 | RemoteはこれらのUI stateやreceiverを借りず、Remote worker上で独立prepareする |
| PC latest-next | viewer contextごとの`CollectionNavigationPending`と`resolve_prepared_collection_navigation` | pure resolverだけを共用し、Remote固有request / cancel / landing ownerを置く |
| 既存Remote集約一覧 | `CollectionEngine`、`CollectionKind`、`CollectionPayload`、`/api/collection`、`#collection/...` | 名前付き永続定義とは別typed message、response、HTTP route、hash routeにする |
| Remote一覧cache | browserの`state.collection` / `state.entries` / `state.pageGroups` | 永続collection用のdiscriminated context ownerを追加し、aggregateを偽装しない |
| Remote次 / 前 | 画像はbrowserのcached `pageGroups`、動画・音声はcached `state.entries`の同種抽出 | 永続collection rootだけserver-side latest prepared resolverへ流す。通常folder / aggregate / childは維持 |
| Remote動画EOF | `video-stream.mjs`が`next_page`を`video_ended` detailとwrap設定付きでdispatchし、`changeVideoFile`がcached一覧を読む | 永続collection rootだけtyped latest navigationへ流し、既存HLS start / stop / autoplay処理へtargetを渡す |
| Remote session | `SessionOperation`がsession identity、generation、operation token、cancel flag、drainを所有 | collection actor待ちとprepareも同じoperationのcancel / drainに参加する |
| HTTP側file access | `Library::validate_remote_address`がRemoteAddressを再検証。実byteはremote-webがread-onlyで読む経路もある | collection membershipを許可証にしない。返却addressも通常と同じ二重検証を通す |
| process終了 | collection actorはAppへmoveされる一方、`RemoteIpcServer` guardは`run_native`外でAppより長生きする | Remote server受付停止・session/request drain → collection producer close → actor shutdown/joinの順を明示所有する |

既存の`CollectionKind`という名称は、DriveList、ReadingHistory、Rating、Bookshelf、Bookmarks、
SmartFolderという動的集約を表す。永続collection ID、entry ID、revision、missing stateを持たず、
`RemoteGridScope::Collection`を介して通常sortを書き換えられるため、永続定義のpayloadとして再利用しない。

現行protocolはversion 55である。Phase 5実装時は永続collection message追加を一つの互換性変更として
version 56へ上げ、本体とremote-webの旧 / 新組合せをhandshakeで拒否する。実装開始時に別のprotocol変更が
先行していた場合は、その時点のversionから一度だけ上げ、本節とRemote計画の版数台帳を同期する。

## 3. 不変条件

1. actor snapshotと`CollectionPreparedSnapshot`が順序・availability・stable identityの正本である。
   Remote専用にManual / Standard sortを再実装しない。
2. actorのload replyを、その要求が「最新を選ぶ」linearization pointとする。subscribeを先に確立し、
   reply後により新しい同collection noticeを観測した結果は返さない。
3. Remote sessionはPC UI sessionと独立する。PCのselection、filter、items generation、fullscreen index、
   parked / detached ownerをRemote requestのcurrent判定に使わない。
4. edit noticeやcatalog refreshだけでは表示中mediaを止めない。次の明示操作または既存EOF要求が
   latest snapshotを読み、成功targetのlanding時だけ表示を移す。
5. collection rootのdirect mediaと、Folder / book / ZIP / PDF内のchild mediaを区別する。
   child内部のnext / prev、seek、spread、password、conversion、HLS、AIは既存経路を保つ。
6. Remoteは永続collectionを編集できない。永続collection用の`RemoteWriteRequest` variantや
   `RemoteGridScope`を追加しない。
7. collection ID、entry ID、source identity、route return情報は表示・再解決のidentityであり、
   file access権限ではない。target pathはactorのcurrent prepared entryからだけ取得し、通常の
   RemoteAddress検証をremote-webとcoreの双方で再実行する。
8. PIN / session cookie / tailnet公開範囲、127.0.0.1 bind、Tailscale Serve、network share共有名と
   device namespaceの拒否を変えない。collection catalog / snapshotを未認証route、static shell、
   health応答、diagnostic logへ出さない。
9. 通常frameとrequest handlerはjoinや同期I/O待ちをUI threadへ持ち込まない。blocking drain / joinは
   process final exitだけで許可する。
10. 既存aggregate一覧、Favorite、Search、Tag、通常Folder、ZIP / PDF、AI、streamingのwireと挙動を維持する。

## 4. wire名とデータモデル

### 4.1 aggregateと永続collectionの分離

既存名との衝突を避け、protocolでは`PersistentCollection*`、HTTP / browser routeでは
`saved-collection`を使う。

```text
ClientMessage::PersistentCollectionCatalog
ClientMessage::PersistentCollectionSnapshot
ClientMessage::PersistentCollectionNavigate

GET  /api/saved-collections
GET  /api/saved-collection?id=<collection UUID>&...
POST /api/saved-collection/navigate

#home/collections
#saved-collection/<collection UUID>
#saved-collection/<collection UUID>/entry/<entry UUID>
```

`CollectionKind`、`CollectionPayload`、`RemoteGridScope::Collection`、`/api/collection`、
`#collection/...`は意味もserde表現も変更しない。`state.collection`のkind文字列へ`persistent`を
足して既存helperへ紛れ込ませず、browserに`SavedCollectionContext`というtagged ownerを置く。

### 4.2 catalog

catalog responseは次を持つ。

```text
PersistentCollectionCatalogPayload {
    catalog_revision,
    collections: [{ collection_id, name, order_summary, collection_revision }],
    limit,
    truncated
}

PersistentCollectionOrderSummary =
    Manual
  | Standard { sort_value, label, short_label }
```

Catalogは既存`HomePayload`へ必須fieldとして混ぜない。collection actorがStarting / Busy / Failedでも、
お気に入り・場所・smart folderのHomeを表示できるようにする。Homeの「コレクション」tabが専用APIを読み、
失敗時はそのtabだけにtyped errorと再読込を表示する。catalogのtruncationは件数とともに明示し、黙って
末尾を消さない。

### 4.3 prepared snapshotのwire表現

Remote専用entryは既存`RemoteEntry`へoptional ID配列を足さず、次のtagged stateを一件の値として運ぶ。

```text
PersistentCollectionEntry {
    entry_id,
    source_identity,
    name,
    state: Available { address, kind, thumbnail_address, detail, rating }
         | Missing { last_known_kind }
         | Unsupported { last_known_kind }
         | AccessError { last_known_kind }
         | BlockedByRemotePolicy { last_known_kind }
}
```

`source_identity`は内部`CollectionSourcePathKey`のnamespaceとnormalized keyからcoreが作る固定長の
domain-separated SHA-256 tokenとする。entry ID削除後の同source再追加を一致させられる一方、
unavailable / blocked entryのpathをidentity fieldとして余分に公開しない。coreは受信tokenをpathへ
復号せず、current prepared entriesを同じtokenへ写像して一致した内部source keyだけをresolverへ渡す。
tokenはaddressとして解釈せず、collision / duplicate検査ではentry IDを優先する。

`Available`へ写す前に、core側で通常のpath syntax、実在、canonicalize、実kindを検証する。
collection上は存在してもnetwork共有名、device namespace等のRemote規則に合わない登録は
`BlockedByRemotePolicy`として同じ有効順位置に残し、addressを返さない。`AccessError(String)`の生OS文言や
絶対pathは返さず、typed stateから固定の利用者向け文言を作る。remote-webは`Available.address`だけを
通常の`Library::validate_remote_address`で再検証し、不一致を開けるentryとして表示しない。

snapshot responseは次を持つ。

```text
PersistentCollectionSnapshotPayload {
    collection_id,
    collection_revision,
    view_token,
    title,
    order_summary,
    entries,
    configured_spread_mode,
    effective_spread_mode,
    reading_direction,
    image_count,
    page_groups,
    spread_page_gap_px,
    entry_limit,
    truncated
}
```

`page_groups`はaddressだけのparallel indexではなく、anchor / navigation / presentation slotが
`{entry_id, source_identity, address}`を一体で持つ永続collection専用display-unit型とする。
grouping自体は既存`build_remote_spread_page_groups`を同じprepared image列へ適用し、結果をstable identityへ
結合する。見開きprimary / partnerを次要求のdirectional anchorとしてそのままcaptureできる形にする。

`view_token`はcollection ID / revision、captured `GridDisplayOrder`、ordered entry identity / availability、
spread / reading option、page-group identityからcoreが作るopaque digestである。同じcollection revisionでも
mtime、size、missing復帰、Remote policy判定、global display category順が変わればtokenを変える。
browserはtokenをcurrent判定と応答圧縮hintにだけ使い、内容を信頼・生成しない。

### 4.4 error

永続collection responseは`BadRequest | Starting | Busy | NotFound | Conflict | Incompatible |
Unavailable | PrepareFailed | Cancelled | Internal`を区別する。actorのtyped errorを既存aggregateの
`CollectionErrorCode::Internal`へ一括変換しない。HTTPは400 / 404 / 409 / 422 / 503等へ対応付け、
Busyだけにbounded retry指示を出す。内部path、collection名、DB error原文はHTTP body / logへ出さない。

## 5. process ownerと終了順

### 5.1 `CollectionRemoteProducerControl`

collection actor起動成功時に、raw `CollectionStoreClient`を包むprocess-global typed controlを一つ作る。
Appと`RemoteIpcServer`にはこのcontrolのcloneだけを渡し、Remote worker / `CollectionEngine`へraw clientを
複製しない。

```text
Open { in_flight }
  --begin_close--> Closing { cancel_all, in_flight }
  --all leases dropped--> Closed
```

`begin_request`はOpenのときだけactor clientとrequest leaseを返す。Closing以後は即時Unavailableである。
leaseはsession operation cancel、producer close、reply receiver、prepare cancelを一つのrequest ownerに束ねる。
Dropはrequest cancelを立て、in-flight数を必ず減らす。final drainは全lease terminalを待てるが、通常UI frame、
dialog close、viewer closeでは待たない。

actor one-shot receiverを待つ間にsession drainまたはproducer closeを即時観測できるよう、
`SessionOperation`のcancelをAtomicBoolだけでなくbounded wake receiverも持つtyped cancellationへ拡張する。
session stateがdrainを開始したときはflag設定とwake publishを同じtransitionで行う。collection待機は
actor reply / session cancel / producer closeをchannel selectし、`try_lock + sleep`や固定poll sleepを使わない。
filesystem prepareは同じcombined cancellationを各entry間で確認する。

### 5.2 startup / final exit

現行はcollection runtimeをAppへmoveした後も`RemoteIpcServer` guardが`run_native`外で長生きする。
clientを追加したままAppだけがactorを止めると、Remote guardがshutdown後actorへ要求できる。一方、現行
`DrainingRemote`はAppがvideo / UI pendingをretireして`complete_app_drain()`を返すまで終端しない。
`App::on_exit`で先にserver drain / joinを同期waitすると、以後のframeが無いままApp ACKを待つ自己deadlockになる。

Phase 5ではcollection actor runtimeをAppへmoveせず、`run_native`外の
`ProcessRemoteCollectionShutdownOwner`がRemote server / service manager / producerと同じprocess lifetimeで
所有する。Appにはactor client、private event stream、producer control、`RemoteAppDrainLease`だけを渡す。
runtime event receiverはruntimeのshutdown権限から分離し、Appはeventをpollできるがactorを先にshutdownできない
型にする。

startup / final exitを次の一つのidempotent coordinatorで直列化する。

1. process ownerがcollection actorを開始し、producer controlを作る。
2. producer controlを注入してRemote serverを開始する。startup失敗時はserver cloneだけをdropし、
   actorとPC collectionを継続する。
3. App creatorへactor client / event stream、producer control、Remote server stop trigger、
   `RemoteAppDrainLease`を渡す。actor runtime / join権限はprocess ownerに残す。
4. Appのfinal exit入口はまずremote-web / networkの新規admission停止をtriggerし、sessionを
   `DrainingRemote`へ移す。続けてon-exit専用`retire_remote_resources_for_final_exit`を一度呼び、通常frameの
   pollに依存せずApp所有video、native / UI pending、page / AI bridgeをcancel / retireして
   `complete_app_drain()` ACKをpublishする。ここではserver joinやactor joinを待たない。
5. `run_native`復帰とApp dropの後、process ownerが`RemoteIpcServerControl::stop_admission_and_drain`を完了する。
   listener / connection intakeを閉じ、collectionを含むqueued / running requestのcancelとworker terminalを
   待つ。この時点ではApp ACKが既にterminalなのでframeを必要としない。
6. producerをClosingへして全collection request leaseのdrain ACKを得る。
7. `CollectionStoreRuntime::shutdown_and_join`でactor admission close → DB connection drop ACK → joinする。
8. 外側のservice manager / server guardがdropしても、既にStoppedなのでno-opとする。

`RemoteAppDrainLease`はApp resource bundleと一緒に所有し、明示final処理を通らないApp Dropでも同じcancel / retireと
App ACKを一度だけ行う。creator未到達ならprocess ownerが`AppNotInstalled` terminalを設定してserverを止める。
creator途中のErr / panicは作成済みleaseのDropを待ってから同じ5→8を行う。outer owner自体のDropもこの順を
best-effortで完了し、actor runtimeをserver / producerより先にdropしない。

actor failure時はpersistent collection APIだけUnavailableになり、既存Remote aggregate / folder / mediaを
通常動作中に止めない。process final exitだけはnetwork admission stop → session begin drain → App resource
retire / ACK → Remote server stop / worker drain → producer drain → actor shutdownの順を待つ。App ACK待ちを
App自身のserver joinの内側へ置く分岐と、server停止前にactorを止める分岐を残さない。

## 6. catalog / snapshot request

### 6.1 共通実行位置

HTTP routeは既存PIN / cookie / bearer認証とRemote session guardの内側に置く。remote-webは既存IPC admissionの
Home classでcatalog、Heavy classでsnapshot / navigateを発行する。core heavy workerがactor受信、settingsの
`GridDisplayOrder` snapshot、filesystem prepare、Remote wire変換を担当する。UI threadやremote-web processが
`collection.db`を直接開かない。

### 6.2 exact snapshot

snapshot / navigateは次の順に行う。

1. Remote `SessionOperation`とproducer leaseを取得する。
2. request専用`CollectionRevisionWatch`をsubscribeする。load前のcatalog revisionもwatchの比較対象にする。
3. `load_collection(collection_id)`をenqueueする。
4. actor replyをlinearization pointとして受け、collection IDとminimum revisionを確認する。
5. workerがcaptured `GridDisplayOrder`とcombined cancelを使って`prepare_collection_snapshot`を実行する。
6. prepared completion後、watch、session generation、producer phase、request deadlineを確認する。
7. noticeの`catalog_revision`がloaded snapshotの`catalog_revision`より新しい場合、全collection revision列を
   検査する。対象IDが無ければdelete raceなので古いpreparedを返さずloadへ戻し、actorのNotFoundへ収束させる。
   対象IDがありrevisionがloaded revisionより新しければ古いprepareを捨てて最新snapshotから再開する。
   対象IDが同revisionのままなら別collectionだけの変更なので無視する。単に「対象IDを含むnotice」だけを
   current条件にしない。
8. current exact preparedだけをwireへ変換する。

HTTP / IPCの有限deadlineまでrevisionが進み続けた場合はstale snapshotを返さずBusyで終端し、browserの
明示再読込に委ねる。cancel / disconnect / session交代 / app exitではreceiverとprepareをdropし、古いresponseを
返さない。coreの既存`execute_work`終端ownership checkとbrowserのrequest sequence / AbortControllerも維持する。

catalogはfilesystem prepareを行わずactorのimmutable catalog snapshotだけを返す。ただしcatalog用watchを
`list_catalog` enqueue前に確立し、reply後により新しい`catalog_revision`を観測したらdeadline内で再loadする。
producer / session current gateを通し、actor Starting / Closedや削除前catalogをold cacheの成功に置き換えない。

### 6.3 response budget

既存aggregateの100,000件上限は共通定数へ寄せる。ただし件数だけでresponse安全性を判定しない。永続entryでは
entry ID、source identity token、tagged state、thumbnail / detail、page groups、response enumとServerMessage envelopeも
増えるため、完成serde frameが`MAX_RESPONSE_FRAME_BYTES`（現在64 MiB）未満であることをbuilderの最終条件にする。
entryと、それを参照するpage groupを同じstable prefix境界で追加し、完成responseを計測して超える手前で止める。
thumbnail / detail / long pathを固定平均長として見積もるだけの実装にしない。最終serializeでも上限を再確認し、
envelope分を含めて超過するpayloadをpipe writerへ渡さない。

上限内ではprepared有効順をexactに返す。超過時はroot gridを有効順prefixとして明示truncation表示するが、
server-side latest navigationはfull prepared snapshotをresolverへ渡す。truncated prefix外のtargetはroot bindingへ
暗黙追加せず、navigation responseの`SparseTarget`がtarget display unit、媒体内ordinal / total、必要address、exact
collection / view tokenを自足する。browserのdirect-viewer ownerはsparse unitをroot prefixとは別に所有し、cached
browser indexやprefix membershipを要求しない。viewerからrootへ戻る時は最新root snapshotを再取得し、targetが
prefix外なら「一覧上限外の項目です」と表示して先頭へ偽装しない。

将来paginationを入れる場合もcursorはexact `view_token`へ結び、別revisionのpageを連結しない。Phase 5初回で
無言のpage連結やclient側sortを追加しない。

## 7. browser contextとroute

browserは概念上次の一つのtagged ownerを持つ。

```text
Inactive
Root { collection_id, revision, view_token, selected identity, route_sequence }
DirectViewer { root binding, primary/partner identity, viewer_sequence }
Child { return origin, existing container context }
```

session acquire / reacquireは既存規則どおり全Remote cacheを破棄し、persistent catalog、root binding、pending
navigationも破棄する。別clientによるsupersede、logout、local takeover、route変更、viewer close、page reloadは
owner sequenceを進めてpending responseをterminal cancelする。PC側で表示しているcollectionやselectionは参照しない。

`#saved-collection/<collection-id>/entry/<entry-id>`はroot直接登録entryの復元routeである。まず最新snapshotを
読み、entry ID、次にroute stateに残るsource identityで探し、current available entryなら種類に応じた既存image /
video / audio landingへ渡す。両方無い、unavailable、kind不一致ならcollection rootを表示する。URLにsource pathや
source identity tokenを必須化しない。

Folder / ZIP / PDF / convertibleを開くときは`Child`へcollection ID、root entry ID、source identity、root routeを
return originとして移し、内容自体は既存`/api/list`、`/api/container`、archive jobへ渡す。childのaddressや
subresourceはcollection membershipから許可せず通常検証する。戻る時はsaved collection routeを最新loadし、
entry ID → source identity → selectionなしでrootをreanchorする。child内の画像をrootに直接登録された画像と扱わない。

既存aggregate/search/tagの`state.collection`、`rootOpenReturnHash`、通常folder historyはそのまま保つ。永続collection
origin判定を`Boolean(state.collection)`へ追加せず、tagged ownerからreturn routeを解く。

## 8. latest next / prev / EOF

### 8.1 対象producer

永続collection rootのdirect mediaについて次をserver-side latest resolverへ接続する。

- image viewerのtoolbar、keyboard、wheel / swipeによる一display-unitのnext / prev。
- video / audio viewerのmanual next / prev。
- video / audio EOFが既存continuous設定から発行するnext。wrap=falseはStop、wrap=trueはLoop。
- spread mode / reading direction変更後のroot refreshとcurrent display-unit reanchor。

seek、First、Lastは現在表示中のexact binding内の明示位置選択として既存処理を使う。root bindingがtruncatedで
対象位置をmaterializeしていない場合は、target ordinalをcoreへ問い合わせるtyped locate要求を同じownerへ追加し、
clientがpathやindexからtargetを合成しない。Remoteに存在しないslideshow / NextFolder / Ctrl+上下は追加しない。

### 8.2 requestとresolver

```text
PersistentCollectionNavigateRequest {
    collection_id,
    presented_revision,
    presented_view_token,
    anchor: { primary: { entry_id, source_identity }, partner? },
    direction,
    target_kind: StillImage | Video | Audio,
    tail: Stop | Loop,
    viewer_sequence,
    force_single_page / spread / reading intent
}
```

coreはclientのpresented revision / tokenをtarget sourceとして使わず、§6.2でlatest preparedを作る。entry IDで
current preparedを検索し、無ければsource identity tokenをcurrent preparedの内部source keyへ照合し、双方無ければ
headとして`CollectionNavigationAnchor`を作る。見開きForwardは生存するprimary / partnerの最も後ろ、Backwardは
最も前をanchorにする。`resolve_prepared_collection_navigation`へ`StillImage` / `Video` / `Audio`とStop / Loopを渡す。

resolver候補はexact revisionごとにeligible count以下、tried entry IDの再試行なしとする。targetはexisting
Remote path guardと媒体kindを再検証し、prepare後に失踪 / kind変更した候補をtriedへ加えて次へ進む。
revision更新時だけpreparedとtriedを作り直す。対象0、全preflight失敗、collection delete、deadline、cancelは現在の
presentationを保ったままtyped boundary / errorで終端する。

### 8.3 responseとlanding

```text
PersistentCollectionNavigatePayload =
    Landed {
        exact_revision,
        exact_view_token,
        binding: Unchanged | Replace(snapshot payload),
        target: SparseTarget(DirectImageDisplayUnit | DirectVideo | DirectAudio),
        target_ordinal,
        target_count,
        anchor_resolution
    }
  | Boundary { exact_revision, anchor_resolution, reason }
```

tokenが同一ならbrowserはcurrent immutable root bindingを保ち、target identityだけを照合する。tokenが違う場合は
target landingと同じcommitでexact snapshotへbindingを置き換える。`SparseTarget`は常にresponseと同じcollection ID、
exact revision、exact view tokenへ結び付くself-contained unitであり、root prefixに同じentryがある場合だけそのentryと
identity / address / kindのexact一致も確認する。truncated prefix外ではmembershipをcurrent条件にせず、coreが返した
sparse unitとexact token、route sequence、viewer instance、Remote session cache epochで検証する。sparse targetを
`state.entries`へappendしたりroot selection indexへ偽装せず、次要求のanchorはdirect-viewer ownerのunitから作る。
route sequence、viewer instance、Remote session cache epochのどれかが変わったresponseはdropする。

target確定後のimage page fetch / decode / spread presentation、video HLS start、audio mode、autoplay、pause、history、
AI controllerは既存landing helperへ渡す。resolverがpresentationを直接変更しない。EOF失敗時は既存動画を停止し、
manual boundaryは既存boundary表示を出す。

### 8.4 rapid input owner

永続collection用`SavedCollectionNavigationOwner`は`Idle | Requesting | Landing`を一つのstateで所有する。
同方向入力をbounded signed queueへ積み、前targetのlanding後に新identityから一stepずつlatest requestを発行する。
反対方向、別target kind、別viewer intentはtyped ruleでcancel / replaceする。close、Back、route変更、session交代、
EOF stopはqueueとin-flight fetchをterminal cancelする。既存`LatestPageLoadQueue`やvideo deferred deltaの
most-recent-winsへ戻して連打を一stepへ潰さない。

## 9. 通常Folder / 本との混在

例としてManual順が`[Folder A, Image B, Video C, ZIP D, Audio E, Missing F]`なら、root gridはこの順を
表示する。direct image nextはB、video next / EOFはC、audio next / EOFはEだけを対象とし、A / Dを暗黙展開せず、
Fを開かない。Standardでは同じ登録集合をPCの4カテゴリ表示順とsortへ通し、Remote独自sortを行わない。

- Folder Aを開いた後の一覧は既存normal folder `SortOrder`、filter、mixed-media規則を使う。
- ZIP D / PDFは既存book固定順、spread、password ownerを使う。
- convertibleは既存Remote archive confirm / conversion job lifecycleを使う。
- child内next / prev / seek / EOFはchildの`state.entries` / page groupsと既存stream ownerを使う。
- childからcollection outerの次登録へ自動移動しない。Remote初回範囲にCtrl+上下 / NextFolderはない。
- Backだけがroot return originを使い、latest preparedへreanchorする。

バックログ§1.246には、現行RemoteのZIP root一覧`enumerate_zip`が本体 / bookmark / Remote page-contextの
`arrange_grid_items`を通らず、folderとimageが混在する階層で一覧順とresume位置がずれる疑いが記録されている。
これは未確認の既存Remote問題であり、Phase 5のpersistent collection固有修正へ混ぜない。本段階はpersistent
entryから開いたZIPも既存`ContainerEngine`の同じ経路へ必ず合流し、独自のZIP列挙・sort・resume解決を追加しない。
§1.246で本体 / bookmark / Remoteの共通materialize境界を直した場合、通常Remoteとpersistent collection childの
双方へ一度で反映される構造を保つ。Phase 5回帰は「既存Remote ZIP経路と同一」を固定し、未確認の並び結果そのものを
正しい仕様として固定しない。

collection rootのStandard sortと通常Folderのglobal sortは別の意味である。rootの`order_summary`は読み取り専用表示とし、
既存sort selectをdisabledにするだけで`RemoteGridScope::Collection`へ偽装しない。childへ入った時点で通常Folderの
既存sort UI / 書込可否へ戻る。

## 10. access、認証、cache、通常Remote機能

### 10.1 認証・公開scope

すべてのpersistent endpointは既存`/api/*` guard内に置き、PIN session cookieまたは診断Bearer、かつactive
Remote session identityを要求する。認証前shell、service worker、manifest、healthへcollection名、件数、ID、pathを
含めない。応答は`Cache-Control: no-store`とする。

現在のRemote公開scopeは「network共有名表記とdevice namespaceを除きmIV本体が開ける範囲と同じ」であり、
Favorite / Smart / Tag / Rating / History / Bookshelfをallowlistにしない。過去のregistered-root briefは現行
`web-remote-plan.md` §3.1 / §12.23で撤去済みであり、永続collection membershipから新しいallowlistを復活させない。
同時に、membershipを理由に通常のRemoteAddress二重検証を省略しない。

### 10.2 session acquireとcache

Remote acquire成功は既存どおり「PC側で何か変わった可能性がある」完全なcache refresh signalである。
browserはpersistent catalog / snapshot / navigation target / thumbnail binding / return originを含むsession cacheを
破棄し、必要なrouteを再取得する。Remote active中にactor revisionが進んだ場合も、snapshot明示reloadと各latest-next
requestがcurrent actor snapshotへ収束する。`remote_state_generation`をcollection revisionの代用にしない。

### 10.3 AIとstreaming

persistent collectionは新しいmedia readerを作らない。imageは既存Page / PageDemand / Remote AI、video / audioは
既存stream start / state / segment / stopを使う。targetのRemoteAddress、stream generation、page display request ID、
AI job ownershipは各既存ownerが保持し、collection IDはfile access keyやstream cache keyに混ぜない。

session交代 / logout / local takeoverでは既存streaming、AI、page demandとpersistent navigationを同じdrainでcancelする。
collection API failureが通常aggregate、folder、Page、AI、video worker admissionを恒久停止しない。

### 10.4 diagnostics

記録してよいのはrequest kind、collection catalog / snapshot / navigateの別、entry / group件数、revision、
anchor resolution、target kind、restart count、queue wait、prepare時間、outcome、typed error codeである。collection名、
entry表示名、絶対path、source identity、PIN、cookie、Bearer、session ID生値をdetailsへ入れない。

## 11. current / cancellation matrix

| 事象 | snapshot / navigateの扱い | 現在presentation |
| --- | --- | --- |
| 同collectionのより新しいrevision | pending prepare / preflightをcancelし、deadline内でlatestから再開 | 維持 |
| 別collection notice | 無視 | 維持 |
| current entry remove | nextはsource identity、無ければhead。root再読込はselection無し | file / streamを即時停止しない |
| remove後same source再追加 | source identityでnew entryをanchorにし、その次 | 維持 |
| source失踪 / kind変更 | exact revisionのtriedへ入れ、有限に次candidate | target成功まで維持 |
| collection delete | NotFound / boundaryでterminal | viewerはBack / 次操作まで維持 |
| session supersede / logout / local takeover | session cancel wakeでactor wait / prepare / HTTP responseをdrop | 既存session drain規則に従い停止 |
| browser Back / route変更 / viewer close | browser owner sequenceとAbortControllerでterminal cancel | 新routeだけを表示 |
| App final exit | server admission stop、全request cancel / drain後にactor shutdown | process終了 |
| actor panic / unavailable | persistent APIだけtyped Unavailable | 通常Remote機能は継続 |

server response適用直前にはsession generation、producer lease、request deadline、collection ID、exact revision / watch、
target entry ID / source identity / path / kindを再照合する。browser landing直前にはsession cache epoch、saved context
route sequence、viewer instance、navigation sequence、target identityを再照合する。どちらもindex一致だけでcurrentと
判定しない。

## 12. 実装単位と影響候補

1. `crates/remote-ipc/src/lib.rs`
   - persistent catalog / snapshot / navigateの別message・payload・typed error。
   - protocol versionとserde / frame round-trip。
2. `src/collection_store/prepare.rs`、`src/collection_store/mod.rs`
   - PC / Remote双方が同じprepared snapshotとresolverを読むためのcrate内adapter。
   - combined cancellationを受けるprepare境界。sort / classification本体は複製しない。
3. `src/remote_ipc/persistent_collections.rs`（新規候補）、`src/remote_ipc/{mod,pipe,session}.rs`
   - actor snapshot / prepare / wire変換、source identity token、latest navigation、producer control。
   - session cancel wake、server admission stop / drain control。
4. `src/lib.rs`、App final-exit owner
   - actor → producer → server注入とserver stop → producer drain → actor shutdown順。
5. `crates/remote-web/src/{ipc_client,http,store}.rs`
   - authenticated endpoint、IPC mapping、Available addressの独立再検証、typed error。
6. `crates/remote-web/web/{app.js,command-core.mjs,styles.css,index.html}`と対応test
   - catalog tab、saved routes、tagged context / return owner、read-only order表示、server-side navigation queue。
7. 実装完了時に`collection-implementation-plan.md`、`web-remote-plan.md`、本書のcheckpointを同期する。

製品実装前に実際の型名とmodule配置を既存Remote ownerへ合わせることは許容する。ただしaggregateとpersistentの
wire分離、raw actor client非公開、Remote server先行停止、PC / Remote context非共有、二重path検証は変更しない。

## 13. 回帰計画

### 13.1 prepared / protocol

- 同じactor snapshotと`GridDisplayOrder`からPC / Remote adapterが同一entry ID列を返す。
- Manual exact順、Standard custom 4カテゴリと全sort、missing / unsupported / access error保持。
- source identity tokenが同sourceで安定し、異なるnamespace / normalized keyで異なる。
- available / missing / blockedのtagged serde、display-unit stable identity、catalog / snapshot / navigate round-trip。
- protocol 55 client ↔ 56 server、56 client ↔ 55 serverをhandshakeで拒否し、同版は成功する。
- existing `CollectionKind` / `CollectionPayload` JSON fixtureが変わらない。
- injected small response budgetでentry / page-groupを同じprefixへ切り、truncatedを明示する。
- long path、最大thumbnail address / detail、ID / source token、page group、`ServerMessage` envelopeを含む
  完成serdeが`MAX_RESPONSE_FRAME_BYTES`未満であり、上限超過responseをwriterへ渡さない。

### 13.2 actor / producer / lifecycle

- subscribe確立後loadの間、load後prepareの間、prepare後replyの間にrevisionを進め、stale resultを返さない。
- loaded catalog revision後のnoticeで対象collection IDが消えたdelete raceは再loadしてNotFoundへ収束する。
  対象IDが同revisionで残る別collection変更だけはcurrent requestを再開しない。
- catalogもsubscribe→list順を固定し、reply後のnew catalog revisionを返さず再loadする。
- 別collection noticeを無視し、同collection churnはdeadlineでBusyとして有限に終端する。
- actor Starting / Busy / NotFound / Conflict / Incompatible / Unavailableを区別する。
- session cancel、producer close、actor replyの各競合で一回だけterminalになり、lease / operation countが0へ戻る。
- Remote startup failureでもPC actorが生存する。actor failureでもaggregate / folder requestが生存する。
- final exitはnetwork admission stop→session begin drain→on-exit専用App resource retire / ACK→
  `run_native`復帰→Remote worker drain→producer drain→actor connection dropの順をbarrierで確認する。
- App final handlerはserver joinを待たず、server joinはApp ACK terminal後だけ開始するためframe無しでdeadlockしない。
- Remote guard cloneが残る状態でもpost-Closing collection requestを拒否する。creator failure / panic / repeated Dropも有限。
- creator未到達は`AppNotInstalled`、creator途中panicは`RemoteAppDrainLease::Drop`でACKし、outer ownerが同じ順を完了する。

### 13.3 list / mixed root / child

- catalog名、order summary、revisionとempty / long name / truncation。
- `[Folder A, Image B, Video C, ZIP D, Audio E, Missing F]`をManual exact順で表示する。
- StandardはPCと同じcategory / sort、Remote側sortの選択や保存を許可しない。
- unavailable登録を同じ位置にdisabled表示し、addressを発行しない。
- Folder childは通常folder順、ZIP / PDF childは固定book順、convertibleは既存confirm / password / jobを使う。
- child Backがlatest rootへentry ID → source identityで戻り、rootのsortをchildへ適用しない。
- aggregate `#collection`、favorite/search/tag、通常folderのopen / Back / breadcrumb / sortが不変。

### 13.4 navigation / browser race

- root direct image / video / audioだけが各kindのlatest有効順を使い、containerをskipする。
- ID維持、ID削除+same source、双方消失→head、tail Stop / Loop、0件 / 1件。
- 見開きprimary / partnerのForward後端 / Backward前端、片方remove、双方remove。
- prepare後source失踪はtriedへ入り、eligible N件ならexact revisionで最大N回。revision更新時だけreset。
- manual rapid 2回が2step、逆方向replace、landing中入力、viewer close / route change / session ABAを覆う。
- video / audio EOFはlatest targetへ進み、boundaryでは既存stop、manual boundaryは既存message。
- token同一のUnchangedとtoken変更のReplaceを照合し、target landingとroot bindingが別snapshotにならない。
- truncated prefix外targetはcached indexなしで開き、ordinal / totalを表示し、Backで先頭へ偽装しない。
- session acquire時にsaved cache / pending / return originを破棄し、古いresponseを新sessionへapplyしない。

### 13.5 security / HTTP / existing feature

- unauthenticated catalog / snapshot / navigateは401、inactive / superseded sessionは既存status。
- malformed UUID、unknown collection / entry、偽source token、client指定pathを拒否する。
- target pathはactor current preparedからだけ取得し、remote-webとcoreの双方で実在・kind・canonicalizeを検査する。
- network共有名、device namespace、ZIP traversal、PDF範囲外を既存規則で拒否する。mapped drive規則は維持する。
- response / HTTP / diagnostic logへcredentialと生path errorを出さない。
- PageDemand、Remote AI、HLS start / segment / stop、normal folder media nextの既存回帰を維持する。

### 13.6 gate

実装時はnarrow protocol / actor / persistent IPC / HTTP / browser testから始め、変更したcrateのcheck、
`cargo check -p mimageviewer --bin mimageviewer-core`、web node test、fmt、UI glyph、viewer-context audit、
`git diff --check`、`scripts/test-full.ps1`を同じsource freezeで行う。user-runnable変更なのでgreen sourceから
`scripts/build-dev.ps1 -PreserveRuntime`を作る。agentはGUIを起動しない。Remote実機は具体scenario、所要時間、
使い捨てdata scopeを提示して利用者が明示承認した別suiteだけで行う。

## 14. 完了条件と実装開始gate

Phase 5完了には次をすべて満たす。

- Remoteに永続collection catalog、root list、direct media view、container child open / returnがある。
- create / edit APIが存在せず、order summaryが読み取り専用である。
- Manual / Standard有効順、mixed root、child内部順がPC仕様と一致する。
- root direct mediaのnext / prev / EOFがlatest actor snapshotとPhase 4 pure resolverを使う。
- Remote session / browser context / actor revision / target identityのstale completionを拒否する。
- Remote server stop → producer drain → actor shutdown順と全cancel / Dropが回帰で固定される。
- current Remote access範囲、二重path検証、認証、AI / streaming / aggregate機能を維持する。
- protocol、focused、full、static、verification buildの同一freeze証拠と独立completion reviewが揃う。

製品編集前に、実装担当とは別のSol / xhigh reviewerが少なくとも次を承認する。

- aggregateとpersistentのwire / route / sort-write scope分離。
- shared producer control、session cancel wake、Remote server先行stop、actor shutdown順。
- watch → actor load → exact prepare → pure resolver → path preflight → responseの非同期境界。
- PC UI contextを読まないRemote tagged ownerと、rapid input / close / session ABA current gate。
- root direct mediaとchild内部順の分離、source identityがaccess許可にならない二重検証。
- response budget / truncation時もcached UI indexをlatest navigationの正本にしないこと。

review findingを本書へ反映して合意checkpointを追記するまで、製品コード、protocol version、Cargo成果物を
変更しない。

## 15. 独立設計レビュー checkpoint（2026-09-15）

実装担当とは別のSol / xhigh reviewerが本計画を検収し、blocking / should-fixなしで製品実装開始に合意した。

一次reviewでは次の3点を修正した。

1. App自身がserver drainを待つと、現行`complete_app_drain()` ACKに必要な後続frameを失って自己deadlockする。
   actor join権限を`run_native`外の単一process ownerへ残し、App finalはframe非依存のRemote resource retire / ACK
   まで、server / producer / actor drainは`run_native`復帰後のouter ownerへ分けた。creator未到達、creator途中
   failure、App Dropも同じidempotent順へ収束させた。
2. `CollectionRevisionNotice`の対象ID absenceを見ないと、load後prepare中のcollection deleteを別collection変更と
   誤認できる。loaded catalog revisionより新しいnoticeでは対象IDの存在とrevisionをともに検査し、absenceを
   reload → NotFoundへ収束させた。catalog request自体もsubscribe-before-listとnew revision gateを持たせた。
3. truncated prefix外targetをroot binding membershipで検証すると着地不能になる。prefixを変更しない
   self-contained `SparseTarget`をexact collection / view tokenとRemote contextで検証し、次anchorもsparse unitから
   作る規則へ統一した。完成`ServerMessage` envelopeを実serializeして64 MiB未満に収めるgateも追加した。

reviewerはaggregate / persistentのwire分離、PC owner非共有、Manual / Standard prepared順、root direct media /
child内部順、current Remote公開scopeと二重path検証、full prepared server-side navigationを承認した。

実装時の重点条件として、`RemoteAppDrainLease`をACKだけのtokenにしない。現Appに散在するvideo、native / UI
pending、Page / AI bridgeのcancel / retire handleを一つのtyped resource bundleとして所有させ、明示final処理を
通らないDropだけでも全resourceをterminal化して一回だけACKできることを直接回帰する。この条件を満たすまで
outer ownerはserver worker drainへ進まない。

## 16. 製品実装・独立 completion review checkpoint（2026-09-16）

protocolを56へ更新し、既存aggregate `CollectionKind`とは別に、名前付き永続collectionのcatalog、snapshot、
latest navigationをtyped IPC / HTTPへ接続した。Remoteは同じ`CollectionStoreRuntime` actorのimmutable snapshotと
`prepare_collection_snapshot`を使い、Manual / StandardのPC有効順、missing / blocked row、root direct mediaと
container childを分けて表示する。create / edit / sort write APIは追加していない。

requestはRemote session operation、producer lease、collection / catalog revision watch、deadline、request cancelを
一つのcurrent predicateで監視する。wire factsはactual kindとRemote path policyを一回だけ検査し、streaming digestと
64 MiB未満のdisplay-unit境界prefixをO(N)で作る。truncated prefix外targetはroot配列へ追加せずsparse viewer ownerが
ordinal / totalとstable identityを持つ。First / Last / ordinal locate / next / prev / video・audio EOFは同じRemote有効列と
Phase 4 pure resolverを使い、partner片側消失、ID→source identity→head、Stop / Loopへ収束する。

browserは`Root / DirectViewer / Child / Inactive`とsession epoch / route sequenceを持つownerへ統一した。catalog、snapshot、
deep locate、archive conversion、navigation replyはcurrent ownerだけがinstallし、session交代時は過去history entryの
source fallback / return originもepoch不一致で無効になる。Folder / ZIP / PDF / convertible archiveは既存child routeへ
合流し、Back時だけlatest rootでentry ID→source identityを解く。通常folder、aggregate、AI、HLS、password、conversionの
既存経路は維持した。

終了はApp final入口でpublic/session admissionを不可逆に止め、App所有のpending UI reply、bookmark write、video stateを
`RemoteAppDrainLease::Drop`でterminal化してACKする。`run_native`復帰後だけserver worker、collection producer、actorを
順にdrain / joinする。`CollectionRemoteProducerControl`は単一close channelで全leaseをwakeし、Closing後の要求を拒否し、
最後のlease Dropまでprocess-final drainを完了しない。creator未install / App Drop fallbackも同じ一回だけの経路を使う。

独立Sol / xhigh reviewerは設計review後、revision / delete race、route / session ABA、response budget、Remote path / actual
kind二重検証、sparse viewer、spread anchor、ordinal seek、EOF terminal、child return、catalog owner、App / producer drainを
反復して照合した。completion coverageとして、複数producer leaseのclose wake / last-Drop drainと、Drop-only App bundleの
pending read / bookmark write / video Opening terminal / duplicate ACK拒否を実owner・counterで追加し、最終sourceに
**blocking / should-fixなし**と承認した。

focusedはpersistent resolver **6 / 6**、collection actor **18 / 18**、session lifecycle **43 / 43**、pipe **17 / 17**、
Remote UI **28 / 28**に加え、上記completion ownership回帰 **各1 / 1**、remote-web **122 passed / 0 failed /
1 ignored**、IPC **57 / 57**、browser runtime **122 / 122**、`node --check`、`mimageviewer-core` checkがexit 0。
同じsource freezeの`test-full.ps1 -SuppressCrashDialogs`は本体 **8571 passed / 0 failed / 45 ignored**、snapshot
**52 / 52**、vendor egui / egui-wgpu / eframe **25 / 9 / 15**、exit 0。ログは
`target/collection-phase5-final-20260916/test-full.{stdout.log,stderr.log,exit.txt}`、SHA-256は順に
`995162F5E230873A3F4E1A4BA23114258C344601BF1538210F5973A1F32EAE93`、
`B24BE12EBDCD5FA6F0E74ACB35FF316FDE6BBF34B5AD2CCC0550064C41BC72C7`、
`13BF7B3039C63BF5A50491FA3CFD8EB4E699D1BA1436315AEF9CBE5711530354`である。

static gateはfmt、UI glyph、viewer-context audit、diff-checkをfail-fastで実行しexit 0。
`target/collection-phase5-final-20260916/static.{stdout.log,stderr.log,exit.txt}`のSHA-256は順に
`2812128CD32AD8EE9E943478B1F1928924CC86E17E63886912B145CA6B094E58`、
`5299BE0EE001D5AD8AC4EF1A84AB9FB2F1AA31FDF34CD04A769CF34576E58DCB`、
`13BF7B3039C63BF5A50491FA3CFD8EB4E699D1BA1436315AEF9CBE5711530354`である。

`scripts/build-dev.ps1 -PreserveRuntime`は、既存`target/dev-runtime/mimageviewer-core.exe` PID 4708を検出し、
停止せず安全にexit 1とした。ログは`target/collection-phase5-final-20260916/build-dev.{stdout.log,stderr.log,exit.txt}`、
SHA-256は順に`C2945D9C4B04D2040CD172E1385CB1B2B61856594C3E8A068DC816859827B55F`、
`7C4D07717EED986304B8D7C3A0F5E93279DF86B2A1833880D87AB3385AD6D84B`、
`F1B2F662800122BED0FF255693DF89C4487FBDCF453D3524A42D4EC20C3D9C04`。agentはresident停止、GUI、
通常profile、real dataを使用していない。verification buildだけをresident終了後のhandoffに残す。

バックログ§1.246のZIP混在child順疑いは、本体 / bookmark / 既存Remote container間の既存差である。本Phaseは
persistent root / outer順だけを追加し、childを既存Remote `enumerate_zip`へ合流させたため症状修正を混ぜていない。
§1.246でchild materializeを共通化する際も、本Phaseのroot owner / return identityを変更しない。

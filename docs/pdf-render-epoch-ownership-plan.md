# PDF render epoch の viewer-context 所有 — 独立修正ステージ設計案

作成: 2026-10-12。調査基点: `next-effetune` / `78e5f09ad`（Everything Stage 1 設計のコミット）。
状態: **設計のみ、独立レビュー未実施、実装 ready 未判定**。利用者が別途レビューを依頼する。本工程では製品コード、.git、build、launch、download を変更・実行しない。

## 1. 独立した目的と問題の根拠

Everything 設計 [§7.2](everything-search-plan.md#72-f12) の P1 を、Everything の IPC／UI／新 KeyAction に依存しない修正として先行できる形にする。既存 Ctrl+G の結果入替え、通常 folder／本／Collection／Smart の一覧変更でも同じ所有違反がある。

守る不変条件: **context A の一覧失効で無効にできる PDF thumbnail jobs は A の旧要求だけ。B の同じ PDF／page の jobs、requested、CPU画像、queue、cancel、世代へ作用しない。** path、window、main の役割、現在 mount 中という理由は失効権限ではない。

コード調査で確認した違反経路（実機再現の主張ではない）:

1. `global_search_ui.rs:2065` の `replace_search_view_items_inner` → `app.rs:50360` の `bump_render_epoch_only` が、`pdf_loader.rs:1180` の crate-global `CURRENT_CONTEXT_EPOCH` を更新する。
2. `pdf_loader.rs:3876` の prune と `:4694` の dispatcher pop 後判定は、この global 値と各 Job の整数を比較する。context の区別がない。
3. 通常／priority／idle upgrade thumbnail は、各 viewer から同じ global 値を焼き付ける (`app.rs:51643 / :52153 / :52789`)。
4. detached PDF の静止画シークも `ui_fullscreen.rs:22367` → `ensure_still_seek_thumbnail_requests` → `enqueue_priority_thumbnail` を使う。fullscreen 本体 render の epoch=0 (`app.rs:73304`) だけではシーク thumbnail を保護できない。
5. PDF Interrupted は thumbnail owner の Canceled 終端となり、requested の解除／Evicted 化へ入る。したがってこれは無駄な計算だけの問題ではなく、別 viewer の要求を取り消す所有違反である。

参考正本: AGENTS.md、CLAUDE.md（必読・簡素化・まれな失敗・PDF pool・応答性）、docs/README.md、architecture-overview、async-architecture §3.4、ui-responsiveness、display-pipeline、virtual-folders、thumbnail-request-lifecycle-plan、[旧 PDF epoch 計画](pdf-pool-context-epoch-plan.md)、detached-rework-plan §2。旧計画の global 整数と epoch=0 の記録は履歴として残し、本設計が何を置き換えるか明記する。

## 2. 採用案と範囲

各 `ViewerContextBundle` が、**変更権限を持つ非 Clone の `PdfRenderEpochOwner`** を一つ所有する。App は mounted bundle の投影として同じ field を持つ。worker／LoadRequest／Job は owner の読み取り ticket だけを保持する。

process 全体の世代比較、path ごとの失効、window ID ごとの失効、registry の全 context 世代表は作らない。pool は共有のまま、各 Job の ticket が示す owner の最新世代だけと比較する。

初回実装の範囲:

- global render epoch API の置換と全 producer／consumer の移行。
- 通常 thumbnail、priority／seek／hover、upgrade、色分類要求、pin／EPUB／RAW の派生要求で ticket を保持。
- pool queued prune／pop、既存 request cancel と completion の整合。
- registry fresh／build／swap／park／fork／promote／retire／drop の owner 移譲。
- 同じ PDF を main と detached が読む自動回帰、UI の失効処理費用、関連 docs の更新。

対象外: Everything 実装、§1.372 の Search idx-only Undo 一般修正、PDFium／DLL／wire protocol 更新、enumerate の別世代、catchup の global epoch 改修、pool lane cap／open admission／priority の変更、Remote API、cache schema、fullscreen 本体の取消方式。調査で別の所有違反が判明した場合は記録し、本ステージに必要かを設計担当へ返す。

## 3. 型と唯一の変更権限

以下は設計上の型名／契約であり、コンパイル済みコードではない。

| 型 | 内容と権限 |
| --- | --- |
| `PdfRenderEpochOwner` | private `Arc<EpochState>` を保持。非 Clone。fresh／snapshot／advance／retire の API。mutation はこの型の `&mut self` だけ |
| `EpochState` | private `AtomicU64` 一つ。1 から始まる live generation、0 は terminal retired。別 active bool や retired Option は持たない |
| `PdfRenderEpochTicket` | 同じ state の read-only Arc と、要求時点の nonzero generation。Clone 可。snapshot を再発行する権限・advance／retire 権限なし |
| `PdfRenderValidity` | `Viewer(ticket) | Unscoped`。従来の整数 `context_epoch` を置換。Unscoped は旧 epoch=0 の明示契約 |

ticket の owner identity は state の Arc allocation identity (`Arc::ptr_eq`)。**path／世代整数だけで同一 owner と判定しない。** 同じ数値 1 を持つ A/B は別 owner である。Job が Arc を保持する間は allocation が生存するため、退役 owner の pointer 再利用で古い要求が新 owner を指すことはない。新しい process-global owner-ID allocator は不要。

`ViewerContextId` は既存 registry の論理 identity／診断に利用する。ticket 作成時に既存 projected ID を診断情報として併記できるが、window/main binding の変化や context ID からの worker lookup を validity の根拠にしない。fresh empty bundle の作成段階でも新しい独立 state を作れるので、owner 作成のために ContextTable の予約／publication 手順を増やさない。

snapshot は UI の**対象 context を投影している enqueue 時点**で作る。ticket は immutable な要求の一部である。worker が global／registry／現在 App の世代を読み直して新世代に更新してはならない。stale は `state.current.load(Acquire) != ticket.generation`。advance／retire は Release で公開し、retire は 0 にして終端する。世代の wrap/reuse は許さず checked increment を使う。実用上到達不能な overflow のための回復 state／retry を追加しない。

Owner の Drop は自身の state を retire する。ticket の Drop は参照解放だけで、owner を retire しない。owner の明示 retire は idempotent。移動／mem::swap は Drop ではない。全 field の destructure／swap／fork 分類に owner を加え、共有 writer clone を型で防ぐ。

`items_generation`、thumbnail request ID／namespace、cancel token を統合・置換しない。render epoch は「旧 PDF queued work を止める権限」、items generation／request ID は「返却画像をどの一覧要求へ採用できるか」である。両方を worker へ伝え、現行 `ThumbnailRequests` を第二 owner へ分裂させない。

## 4. producer / consumer 一覧

行番号は `78e5f09ad` の調査位置。実装直前に `rg` で全 caller を再確認し、表と差分を対応させる。

### 4.1 世代を変更する境界

| 現行箇所 | 新契約 |
| --- | --- |
| `global_search_ui.rs:2065` → `app.rs:50360` Search replace | 採用を受理した projected context の owner だけ advance。Ctrl+G streaming／sort／facet と将来 Everything が同じ境界を使う。thumbnail worker の cancel token は flip しない |
| `app.rs:50336` `bump_full_context_for_load` | 読込を行う context の owner を advance。worker 再 spawn の既存 cancel／wake／新 token は維持。catchup の main 限定条件と PDF local epoch の条件を分離し、detached が自身の一覧を替える場合も自身の PDF jobs を失効可能にする |
| `app.rs:34187` archive accepted-navigation tail | 既存の場所変更条件で対象 owner を advance。global bump を local に置換。旧 jobs の取消時点を勝手に後ろへ遅らせない |
| `app.rs:37056` direct PDF navigation tail | 既存 catchup/main guard の内外を分ける。PDF epoch は読込対象の所有境界、catchup の仕様は変更しない |
| `app/collection_grid.rs:2574` Collection history child 採用 | 共通 accepted closure にある projected owner のみ。Collection の別 context や Remote へ通知しない |
| `app/smart_folder.rs:3542` prepared child install | 同じ accepted install の対象 owner のみ |
| registry fork の direct-reader→main physical basis 復元 (`viewer_context_registry.rs:3432` 付近) | main の idx 空間が実際に替わる復元だけ、main owner を advance。通常の read-only park／fork のために元 owner を advance しない |
| `ViewerContextBundle::cancel_all_context_work`／Drop (`:1741 / :1799`) と App 終了 | terminal context の owner を retire。兄弟の owner を advance／retire しない |

一般の index 世代更新すべてへ PDF bump を機械的に足さない。既存の render invalidation の時点・理由を上記境界で保持し、fork 時の実際の一覧復元だけを補う。重複 bump を隠すための bool／grace を追加しない。old pending の stale rejection や取消だけでは accepted 一覧を invalidation しない。

### 4.2 要求生成・継承・decode

| 現行箇所／経路 | `PdfRenderValidity` |
| --- | --- |
| `app.rs:51643` priority enqueue（seek／hover を含む） | `Viewer(projected owner.snapshot())` |
| `app.rs:52153` 通常 enqueue | 同上。GridItem の Image／PDF／folder／pin の種類では owner を分けない |
| `app.rs:52789` idle upgrade | 同上。loaded thumbnail の独立 render 要求でも元 viewer の owner |
| `app/color_filter.rs:788` color-analysis request | source を解決した対象 viewer の snapshot。色結果の既存 identity 契約も保持 |
| `app.rs:94995` `make_load_request`／pinned-only builder (`:94758`) | builder は候補を作るだけ。UI queue admission で共通 helper により Viewer ticket を装着。background からも呼ぶ builder 内で current owner を取得しない |
| `app.rs:95791 / :95898` pin cascade 解決、`thumb_loader.rs:57` `inherit_request_identity_from` | ticket をそのまま Clone。古い親要求を fresh ticket へ更新しない |
| `thumb_loader.rs:2457 / :3300 / :4917` request→resolve／load／render | immutable ticket を最後の `pdf_loader::render_page*` まで渡す。ZIP/folder heavy resolve、RAW follow-up、EPUB fallback の identity 継承も同じ規則 |
| `pdf_loader.rs:5570–5811` render wrappers→pool.execute→Job | `u64` の引数を typed validity に置換。read target／logical EPUB path の別は owner を変えない |
| `thumb_loader.rs:843 / :884 / :4148` 派生／background request constructors | 元 request に従う派生は継承、独立 background は明示 Unscoped。`..Default` による取り落としを実装監査で確認 |

共通 UI enqueue helper は input_seq／items_gen／request ID と render ticket を装着し、通常／priority／upgrade が同じ契約を使う。UI-bound request が Unscoped のまま pool へ流れることを handler-level test で検出する。pure builder の Default は Unscoped でよいが、それを UI の最終 admission の代わりにはしない。

### 4.3 Unscoped の維持と追加の確認点

| 経路 | 初版の扱い |
| --- | --- |
| fullscreen 本体／prefetch `app.rs:73304 / :73837 / :74547` | Unscoped。既存 fs_pending の cancel／generation／viewport contract を維持。seek thumbnail は §4.2 の Viewer であり混同しない |
| neighbor prefetch `thumb_loader.rs:3766 / :3778`、bulk cache／cache creator／製本 `app/cache_ops.rs / books.rs` | Unscoped、既存の cancel token を維持 |
| similar-index／bench、capture／materializer 経由の独立 render | Unscoped。view の一覧交換を source データの変更と解釈しない。`render_page_canonical_raster` も監査対象 |
| Similar panel thumb `ui_metadata_panel.rs:919`／preview `similar_preview.rs:1375` | 現行の Unscoped と panel request ID／stamp／cancel を維持。main 一覧 bump だけで取り直さない既存テストを新型へ移す |
| Similar compare pin `ui_metadata_panel.rs:1606 / :1913` → `similar_preview.rs:1536` | 現行は worker が global epoch を読む別 producer。Critical preview は旧計画の epoch=0 契約へ揃え、明示 Unscoped を渡して global read を除去する。compare-pin job の既存 modal／結果採用 owner を変更しない。これは UI thumbnail ticket を worker で発行する例外にしない |
| Remote container thumbnail `remote_ipc/container.rs:7075`、`remote_ipc/thumbnail.rs:428` | 明示 Unscoped。Remote 固有の validation/cancel を維持。PC context の epoch／検索 state を公開しない。IPC schema/version 変更なし |
| `get_document_info`／enumerate／EPUB generation verification 等、非 render pool callers | Unscoped。旧 epoch=0 を明示型へ移すだけ。`LATEST_ENUMERATE_EPOCH` は別契約のまま |

Critical と Unscoped は同義の型ではないが、このステージの既存 Critical caller は一覧 epoch に従属させない。pool の queue prune は従来どおり Normal／HighNormal を対象とし、pop validity は Viewer ticket を持つ job にだけ適用する。将来 Scoped Critical を追加する場合は caller の取消意味を別途決める。

## 5. pool prune / pop と UI 応答性

### 5.1 queue を UI から走査しない

現行 bump は pool の queue mutex を取り全 queued jobs を prune する。owner ごとの比較へ置き換えるだけでも正しさは改善するが、UI に O(queue length) の走査・大量 reply／Job 解放を残す。採用案では advance／retire の UI path は **atomic publish＋非 blocking の prune wake** までとする。IPC／FS／DB／pool 初期化／queue mutex／join は呼ばない。

既に初期化済みの `PdfWorkerPool` 内に、**一つだけ** prune maintenance worker と容量1の wake-only channel を持つ。per-viewer worker、owner→pending map、retry state は増やさない。UI の `try_send(())` は full なら既存の未処理 wake に合流する。message に古い target generation を保存せず、worker が jobs の現在の state を見るので、連続 advance／retire を欠落させない。pool が未初期化なら通知は不要（queued job が存在しない）。owner 作成／drop から get_pool を呼ばない。

worker は blocking recv で眠り、wake 後に既存 pool queue の Normal／HighNormal を調べる。除去条件は **各 job 自身の ticket.is_stale()**。Unscoped と他 owner の current ticket は残す。除去済み jobs は queue lock 外で Interrupted を一回 reply し、payload を解放する。queue 状態を変更した worker が既存 Condvar の通常規約で dispatcher を notify する。UI が mutex なしの notify だけに依存して lost-wake を避けるための時間窓は追加しない。

pool 起動／teardown に同 worker を含める。shutdown は既存 queue shutdown と worker lifecycle に合わせて wake sender を閉じ、終了を待つ作業は既存 background shutdown 境界へ置く。UI の owner drop は maintenance thread／PDFium の終了を待たない。channel disconnect は既存のログ＋通知方針で扱い、再起動 worker／再試行ループを作らない。正しさの最終防壁は下記 pop 判定であり、maintenance の完了 ACK を UI の採用条件にしない。

### 5.2 pop / enqueue race

`run_dispatcher` の IPC 開始前に、queued prune と同じ ticket predicate を再検査する。snapshot→advance→遅れて enqueue された old job は、prune が先に済んでいてもここで Interrupted。retire 後の late enqueue も同じ処理である。path が新一覧にもあることや世代整数が他 owner と等しいことは freshness の理由にしない。

lane 選択・open admission・worker document reuse・Critical reservation は現行維持。pop 後に stale と判定した job の `normal_in_flight / workers_busy / open_in_flight` は現行取消 tail と同じ一回だけ解放し、Condvar notify を維持する。queued prune はまだ増えていない in-flight count を減らさない。maintenance prune と dispatcher pop は同じ queue mutex で排他的に所有を取り、同じ job に二回返信しない。

チェック直後に owner が advance して、既に dispatch が始まる race は既存 in-flight として扱う。PDFium の開始済み render を強制停止する約束はしない。atomic 世代だけで OS IPC と完全同期しようとする lock／ack／completion wait は追加しない。

### 5.3 cancel / completion / cache

render epoch と既存 cancel token の役割を保つ。Search replace は persistent thumbnail worker の token を flip せず owner.advance。full reload は従来 token の cancel／wake／再 spawn に加えて owner.advance。context retire は token cancel と owner.retire。いずれも別 viewer の token を変更しない。

Queued stale の Interrupted は既存 PDF 限定の Canceled thumbnail terminal に接続する (`thumb_loader.rs:5410` 付近)。ただの return で requested を残さず、`ThumbnailRequests` の request ID／items generation 相関を通す。別 context の tx/rx を使わない。旧 items が既に退役なら terminal は既存 stale-discard へ入り、新要求を解除しない。

in-flight の `AbortOnCancel / HarvestOnCancel`、cache-savable result の保存、SourcePixels／Finalized 両終端を維持する。epoch stale だけを理由に harvest 保存を禁止しない。届いた画像の UI 採用は既存 ID／generation／keep checks が担当し、render ticket を新しい画像 owner に置き換えない。cache key／mtime/size／EPUB generation lease は source の契約であり、この owner identity を cache key に追加しない。同一 source cache を読む兄弟の再生成や purge を誘発しない。

## 6. registry の移譲・fork・終端

registry は `ViewerContextId` と window binding を分離した ContextTable を持つ。owner field を既存 bundle 全 field の分類に加え、TableOp の admission／publication 順を変更しない。

| transaction | owner の扱い／要求 |
| --- | --- |
| App 初期化／non-Windows single context | fresh owner を一つ作成。non-Windows でも pure owner／pool tests が実行可能 |
| `ViewerContextBundle::empty` (`:1817` 以降) | fresh owner。既存 context の writer を Clone しない。空 placeholder の drop は自身の未使用 state だけ retire |
| `swap_viewer_context_bundle` (`:2295 / :2572` 付近) | owner を tx/rx、requested、items generation、queues とともに swap。swap で advance／retire しない |
| mount / deposit / withdraw | 同じ owner がその context payload に付随する。mount 中の App から snapshot。main ID／window の current level を validity に使わない |
| ordinary park / unmount、後の resume | 同じ owner を保持。静止画の既存 park terminal／CPU画像破棄は維持するが、park だけで render owner 全体を失効させない。既存 main jobs を sibling の操作で取り消さない |
| `split_current_context_preserving_main_grid` (`:2807`) / LiveMediaPark | 元 main の items／thumb tx/rx／queue/requested／writer は main に残す。fork 先は empty の fresh owner。移動する fullscreen pending は Unscoped＋自身の cancel を持つ。進行中 thumbnail jobs／ticket／requested を fork 先へ複製しない |
| MaterializedStillOpen／physical scope fork (`:3450`) | loaded 画像／一覧値は既存方針でコピーできる。writer／queued/in-flight jobs はコピーしない。fork 先の新 enqueue は fresh owner snapshot。direct-reader→main basis 復元で一覧が替わるときだけ元 main を local advance |
| promote / `stash_mounted_and_start_fresh` (`:4440`) | stashed payload は元 owner をそのまま保持。fresh main は新 owner。promote が既存 context の main/window binding を変える場合も、同じ payload の owner は保持。main になったから兄弟を失効させない |
| detached begin_build / commit_build | fresh projected payload の owner を新 build context と一緒に移譲。元 context の owner は table slot に保持。commit 後に元へ戻しても同じ writer |
| abort_build | build payload の owner だけ retire/drop。復元された元 owner の epoch は不変。新 rollback state は追加しない |
| retire / close / context clear | terminal bundle の cancel_all_context_work で retire＋既存 cancel/wake。Drop も同じ idempotent owner terminal を通る。retiring payload を観測するだけでは sibling invalidation なし |
| App drop | projected owner を retire、stored bundles もそれぞれ Drop。ticket の Arc が残っても retired=0 により永遠に stale。UI join なし |

`duplicate_for_parked!`／`move_to_parked!`／main に残す分類に writer を明示し、pending work の別 context への暗黙 transfer を作らない。legacy main-only epoch guard を撤去するだけの修正ではなく、**mutation capability 自体を payload と共に移譲する**構造変更である。

detached-rework-plan §2 の BA-7（状態・所有）、時間窓なし、viewport/placement 症状パッチなしの規則に沿う。registry fields／transfer を変更する実装前に設計担当と利用者が依頼する独立 reviewer が構造修正として合意し、実装時に §11 に対象範囲・理由・証拠を記録する。今回その文書は編集していない。

## 7. 簡素化・却下・まれな失敗

全窓を modal にする／一覧変更時に sibling を閉じて再 open する案は検討し、既存の独立閲覧・F12・シークを止めるため却下。通常操作を削減しない。owner-local ticket と既存 accepted／cancel boundaries だけで scope を限定し、rollback／resume／pending 保存状態を作らない。

global map に全 owner epoch を登録する案は、drop/unregister と reader lifetime の第二 owner を増やすため却下。ticket が read-only Arc を持つことで検索を避ける。`items_generation` だけで pool jobs を判定する案も、同じ整数を持つ兄弟や persistent Search worker の invalidation を区別できず却下。

global bump を Ctrl+G/Everything だけ省く guard、main-only global guard の追加、全 render を Unscoped にする案は、旧 main jobs の queue prune を失うため却下。既存 cancel token のみへ統合すると Search replace が worker を終了させるため採用しない。per-context worker pool／cache も不要。

prune の UI 同期走査は避け、pool 内一つの容量1 wake-only maintenance worker に限定した。full は notification 合流であり、typed retry／supersession 状態を設計しない。通常 enqueue は prune 完了を待たず pop check で守る。

**まれな失敗は既定で割り切り（ログ＋通知）。** worker／channel／cache write の異常のために retry、途中状態保存、failure taxonomy、multi-step recovery、completion wait、保存 DB の削除／再作成を追加しない。通常の A/B 一覧交換・late enqueue・park/fork/retire はまれな失敗ではなく、本設計の不変条件で処理する。

## 8. 自動回帰と受入条件

実 Everything／実 PDFium／実 window／FS に依存しない pure owner／fake pool／既存 App handler fixture を優先する。fake dispatcher の barrier／channel で順序を制御し、sleep／現実時間の timeout で race を作らない。

| 境界 | 必須回帰 |
| --- | --- |
| owner / ticket | A/B が同じ generation=1 でも別 owner。A advance で A旧だけ stale、A新とB旧はcurrent。retire後のticketはstale、ticket clone/dropはownerを失効させない。writer非Clone |
| queued prune | 同じ path/page/target の A/B Normal・HighNormal、Unscoped、Critical を混在。Aだけadvance→A旧だけInterrupted一回、Bの順序・reply・countは不変。Bだけadvanceの逆方向も検証 |
| pop race | A snapshot→A advance→maintenance sweep→旧A enqueue。popでIPC実行0・Interrupted。新Aと旧Bは正常。popが先に取ったケースのin-flight count解放も一回 |
| wake 合流 | 連続 A/B advance・retire、容量1 full、maintenance実行中の追加wake。最新stateで全stale対象を除去。UIはACK待ち・queue lock・pool spawnを行わない |
| Search regression（Everything不要） | mainとdetachedが同じPDFを持ち、detached seek thumbnailがqueued/Pending。mainの実`replace_search_view_items`境界を通す。旧main PDF jobだけ取消、B job正常終端。B items_gen/request ID/keep/queue/cancel/owner epoch不変、Failed/Evictedへ勝手に落ちない |
| 通常一覧の sibling route | A通常 folder reload／archive open／Collection child／Smart child、Aがdetachedにmountされる逆方向。別contextのownerとPDF要求に作用しない |
| 継承 | folder pin→PDF／EPUB、RAW follow-up、fallback／upgradeの ticket が元と同じowner/generation。parentがadvance後に派生してもfresh化しない。通常/priority/color/upgrade全producerをhandlerで検査 |
| terminal | Interrupted→Canceled→既存ThumbnailRequestsの両終端。旧IDの遅着が同idxの新要求を解除しない。取消／保存中／image backlogの既存§1.360 testsを弱めない |
| registry | fresh/build commit/abort、swap往復、ordinarypark/resume、LiveMediaPark、MaterializedStillOpen、promote、retire/drop。writer移動はidentity不変、forkはidentity別・in-flight非複製。empty placeholder dropで元をretireしない |
| Unscoped | fullscreen本体／prefetch、Similar preview/compare、bulk／neighbor／Remote／enumerateが一覧advanceでstaleにならない。既存cancel／Critical／cache harvestは保持 |
| in-flight / cache | barrierでA開始後advance、Bは正常。Aは従来Abort/HarvestとUIgeneration契約で終端。同source cacheにownerIDを追加しない、B cache purgeなし |
| 性能 / idle | 10,000 queued fake jobs、advance UIの仕事がO(1)、大きいprune/reply/freeがworker。queue mutex競合・新worker idleを計測。スキャンでUI既存promotion/priority操作の予算を超えるならgate不合格として設計へ返す |

最重要 main+detached fixture は Path A/B を変えて逃げず、**同じ physical path/page/target** を用いる。pruneだけのunit testではなく、実 App Search replace→対象owner publish→fake PDF queue→context固有ThumbMsg受理まで相関を確認する。Windows registry handler tests と pure cross-platform tests を分ける。現行実装で sibling job が棄却されることを検証可能な形で regression を用意する。

実装時の verification owner は実装担当一人。narrow lib filters → core check／fmt →共通pathの最終 `scripts/test-full.ps1`。有効な結果を独立 reviewer が再利用する。runtime修正の検証 binary は `scripts/build-dev.ps1`、agent はnormal profileを起動しない。実機試験は具体suite・所要時間・desktop/input・disposable portable data範囲を提示し、明示了承後に限る。利用者実機のmain+detached PDF／Ctrl+G／seek併用は別証拠として記録する。今回はtests/build/launchなし。

## 9. 独立実装ステージの区切りと文書

独立レビューを利用者が依頼し、設計担当との合意を得てから着手する。本設計の自前内部レビュー／別agentレビューは実行していない。

一つの独立した修正ステージ **PDF-EPOCH-OWNERSHIP** として、型／pool／caller／registry／回帰を完成させる。Everything query／source UI を同じdiffへ入れない。作業は (1) owner/validity＋fake tests、(2) pool prune/pop＋全caller/registry migration、(3) App same-path regression＋docs/gates の順。検証中の中間差分は許すが、旧global APIと新owner APIが共存した状態やUI要求を仮Unscopedにした状態を完成commit／統合物として出さない。

想定所有ファイル: `src/pdf_loader.rs`（または型用private submodule）、`src/thumb_loader.rs`、`src/app.rs`、`src/global_search_ui.rs`、`src/app/viewer_context_registry.rs`、`src/app/color_filter.rs`、`collection_grid.rs`、`smart_folder.rs`、`similar_preview.rs`／`ui_metadata_panel.rs`、全 explicit render caller（cache_ops/books/Remote/bins）、関連 tests。caller のsignature移行はproducer表の契約変更だけ。共有ファイルのwriterは実装者一人。detached reworkとの同時編集を避ける。

完成gate:

1. productionの`CURRENT_CONTEXT_EPOCH`／`current_render_context_epoch`／global `bump_render_context_epoch`／整数`context_epoch`依存が消え、全UI enqueueと派生ticket、全Unscoped callerが表と一致する。
2. owner変更権限がApp投影／bundleだけにあり、jobs／workersからadvance/retireできない。registry全分類で同一context保持・fork独立が確認済み。
3. main+detached同pathのSearch replaceと逆方向、lateenqueue/retire、Canceled終端、Abort/Harvestの回帰がgreen。Critical/lane/open/caches/Remote既存testsを弱めない。
4. UI側epoch変更に同期queue走査・IPC/FS/DB・joinなし。prune/cancel/idle性能gateを満たす。通常機能を制限してpassさせない。
5. matching docs（async-architecture、architecture-overview、display-pipelineのscope、thumbnail-request-lifecycle-plan、旧pdf-pool-context-epoch-planの後継記録、CLAUDE PDF節）と detached-rework-plan §11をコードと同時更新。user-visible新操作はないためmanualに新shortcutは足さない。Everything manual/keymapは後続feature stage。

独立修正だけを統合可能なrollback点にする（今回commit/mergeなし）。このstageの独立review・gate・必要な利用者確認の証拠が揃ってから、Everything plan §7.2のP1を解消済みへ更新する。設計書を書いた時点ではP1を解消済みにしない。

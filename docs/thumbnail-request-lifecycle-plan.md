# サムネイル要求の寿命を一つの owner に集約する（§1.360）

作成: 2026-10-09。ライン A / `next-tray`、基点 `840c309ec`。
状態: **設計案のみ。未実装・独立設計レビュー前。コミットは利用者が行う。**

## 1. 対象と根拠

[次リリースバックログ §1.360](next-release-backlog.md)
の根治案。今回はこの文書だけを作成する。製品コード、analyzer、既存手順書の変更、
ビルド、アプリ起動、実機検証、コミットは次工程であり、この工程の完了には含めない。

参照した正本は `AGENTS.md`、`CLAUDE.md` の関連節、`docs/README.md`、
[architecture-overview.md](architecture-overview.md)、
[async-architecture.md §3.4 / §4](async-architecture.md)、
[ui-responsiveness.md §4 / §11](ui-responsiveness.md)、
[idle-health-check.md](idle-health-check.md)。表示と仮想フォルダの境界について
[display-pipeline.md](display-pipeline.md)、[virtual-folders.md §3.1](virtual-folders.md)、
context の保存・交換について [detached-rework-plan.md §2](detached-rework-plan.md) も参照した。

利用者の配布 core による測定と、提供された読み取り専用調査が根拠である。
この作業では製品を起動していない。手元の証拠は
`target/trayloop/perf_events.jsonl` と
`target/trayloop/20261009-190651-tray-residency-perf.json`。
測定窓は `t=129.495..144.510`。report は CPU を除く次の値を持つ。

- 同一 Folder 要求 idx 138 / 139 の enqueue は各116回。
- `requested_nonempty` / `texture_backlog_nonempty` の継続は約14.723秒。
- update は約9.46回/秒。提供調査のプロセス測定では CPU one-core ratio は約0.44。
- ログ全体の backlog 最大値は4,812。採用に進んだタイルもあり、排水停止ではなく重複の蓄積。
- 測定窓の input 7件は全件 `modifier_probe`、`trigger=heartbeat`、
  `keys=[]`、`wheel=false`、`focused=false`（この作業でもログを読み取り照合）。

提供調査では v4.4.0 にも同型のコードが存在し、音声画像変更が導入したループではない。
v4.4.0 の PASS 記録は対象・cache 状態が異なり、同条件の比較にはならない。
可視時の Evicted 再ロード / Loaded 高画質化にも同じ遷移が成立するが、
今回の大量反復を可視時に利用者が観測したという意味ではない。

### 1.1 違反を作る所有境界

基点の source inspection:

| 境界 | 現在の処理と破れた前提 |
| --- | --- |
| `src/tray_integration.rs::release_gpu_resources`（450行付近） | 非動画 Loaded を Evicted にし、通常経路では CPU backlog も clear する。GPU 解放と要求の寿命は別であるべき |
| `src/app.rs::update_keep_range_and_requests`（50774行付近） | keep 内の Pending / Evicted かつ要求なしを投入する。Evicted の投入時に表示状態は Pending へ変更しない |
| `src/thumb_loader.rs::load_one_cached`（4943 / 5097行付近） | source Pixels を先に送り、保存または保存省略の後で Finalized を送る |
| `src/app.rs::poll_thumbnails`（49561行以降） | 8枚/frame を超える画像は backlog へ。Finalized は Pending だけを `pending_finalize` で保護し、Evicted / Loaded は要求を解除する |
| `src/app.rs::enqueue_idle_upgrades`（51200行付近） | Loaded の旧 `UpgradeableCache` を描きながら新画像を生成する。新画像が backlog にあっても Loaded 判定で要求解除される |

**「Evicted / Loaded なら、この要求の画像は採用済みか破棄済み」という推測が誤り。**
表示状態の分岐を増やす修正では、keep 外破棄・取消・遅着通知にも同じ欠陥が残る。

## 2. 不変条件

1. **同じ有効な source 需要の要求は、worker の保存/省略が終端になり、かつその要求の
   画像が採用または明示的に破棄されるまで、再投入可能にしない。**
   source 成功では前者の証拠は `Finalized`。画像送信、decode 完了、進捗カウンタ、
   表示状態、backlog のフィールド有無は代用しない。
2. 採用が先なら Finalized を待ち、Finalized が先なら採用/破棄を待つ。
   既存表示が Pending / Evicted / Loaded、通常ロード / idle upgrade のいずれでも同じ。
3. cache hit、Audio、動画専用スレッドなどの単一通知は、通知自体が worker 終端を証明する。
   Pixels はなお採用/破棄まで所有する。画像なしの NoArt / Failed / Canceled は
   「画像を今後送らない」終端通知として完了する。Finalized を待ち続けない。
4. source Pixels の keep 外破棄や tray 時の破棄は画像側だけの完了。
   保存中の要求を解除しない。実行前のキュー取消だけは、キュー lock 下で
   worker が未取得と証明できるため、画像なしの局所終端にできる。
5. 通知と画像は **items generation + 個別 request ID** に相関させる。
   旧要求の Pixels / Finalized / Canceled / Failed / NoArt が新要求の状態や表示を変更しない。
   同じ generation・idx・input_seq でも request ID が異なれば別要求。
6. create / enqueue / mutate / drain / discard / cancel / invalidate / drop は要求の
   所有 viewer context だけに作用する。読み取り専用の open / close / mount は
   兄弟 context を失効させず、要求 owner を複製/clear しない。
7. origin、`evaluated_display_px`、source/layout dims、編集プレビューの証明は画像に随伴する。
   採用しない画像から表示済み画像の provenance や評価済み解像度を更新しない。

GPU texture の退去は、既に済んだ「画像の採用」を取り消さない。採用後・保存中に
Loaded→Evicted となっても Finalized を受けた時点で正しく要求が終わる。
一方、未採用の画像を持つ Evicted は Finalized だけでは終わらない。

## 3. 単一の typed lifecycle owner

### 3.1 所有と相関

既存 `ThumbnailRequests` / `ThumbnailRequestOwner`（`src/app.rs:6566` 付近）を拡張する。
`requested` の membership、`pending_finalize`、`texture_backlog` に分散した寿命を
**この owner に集約**する。`pending_finalize` は撤去し、外部からの idx 単独の
`insert/remove/clear` を意味のある owner API に置き換える。

owner が保持するもの:

- idx ごとの現在の request key と要求種別（通常 / upgrade / 動画 / 直接画像など）。
- request の typed lifecycle と、その要求が所有する未採用 `ThumbPixels`。
- アップロード順の queue。これは request key を指す順序だけを持ち、別の完了判定に使わない。
- 既存の `edit_preview_epoch` 等の採用証明。

現在の `texture_backlog` の CPU 画像を owner の state 内へ移す。
互換 accessor が必要なら backlog 件数などの読み取り投影だけを提供し、
App / tray / bundle が勝手に画像 queue を clear できる構造を残さない。
テクスチャを UI が採用した後は既存の `thumbnails` / `thumb_pixels` が所有する。

request key は `items_gen` と、実際の投入時に一度だけ発行する process-local な
一意の `request_id`。ID allocator は単なる採番であり、App-global な要求状態や完了集合ではない。
channel / owner を既存 context と一緒に運ぶので、追加の viewer 判定や viewport ID は不要。
`input_seq` は計装用のままにする（0や同値があり、要求識別には使えない）。
LoadRequest、ThumbMsg、RAW ticket / follow-up、動画のバッチ項目、直接画像投入へ同じ key を通す。
worker 内で派生する clone / EPUB fallback / resolve / RAW half 再queue は新規要求ではなく、
**元の key を保持**する。

Remote や `bench_scroll` の専用 consumer は本体の owner を共有しない。
共有型を変更する場合、standalone 用の相関を型で明示し、UI owner 未登録の通知を
idx だけで暗黙に受理する fallback は作らない。Remote の生成・cache admission・IPC 仕様は維持する。

### 3.2 lifecycle の案

以下は概念型であり、今回 Rust 定義は追加しない。具体名は実装時に既存の型と揃える。
相互排他的な状態は enum で表し、表示状態を覗いて解除する分岐は持たない。

| 状態 | 所有している事実 | 終了に必要な残り |
| --- | --- | --- |
| AwaitingResult | 有効な要求を投入済み、画像未受信 | 結果と worker 終端 |
| FinalizedAwaitingResult | worker 終端を先に受理、画像未受信 | 画像の受信と採用/破棄 |
| SourceImageQueued(pixels) | source 画像を upload queue に所有、worker 非終端 | 採用/破棄と Finalized |
| TerminalImageQueued(pixels) | worker 終端済み、画像を upload queue に所有 | 採用/破棄 |
| ImageSettledAwaitingFinalized(outcome) | 画像は Adopted / Discarded、worker 非終端 | Finalized |
| ProvisionalImage(disposition) | ドライブ代表の暫定 seed。検証待ち | seed の採用/破棄と検証終端、または確定画像への置換 |

完了状態は API の返り値として必要な副作用を返し、その request entry を除去する。
Completed の履歴や tombstone を無期限に蓄積しない。一意の ID と現 owner の照合で遅着を棄却する。
FinalizedAwaitingResult は通知順序に依存しない表現で、通常の同一 sender では Pixels が先に届く。
画像なしの成功を Finalized だけで通知することは許可せず、NoArt 等で明示する。

| イベント | 代表的な遷移 |
| --- | --- |
| source Pixels | AwaitingResult→SourceImageQueued。先に Finalized 受理済みなら TerminalImageQueued |
| Finalized | SourceImageQueued→TerminalImageQueued。ImageSettledAwaitingFinalized→完了。AwaitingResult→FinalizedAwaitingResult |
| 単一通知 Pixels | AwaitingResult→TerminalImageQueued（cache hit / Audio / video / 直接画像） |
| UI 採用 / 明示破棄 | SourceImageQueued→ImageSettledAwaitingFinalized。TerminalImageQueued→完了 |
| 画像なし終端 | 一致する要求の未採用画像も破棄し、worker 終端と画像側終端を一度に確定→完了 |
| 実行前 queue prune | lock 下で取り除いた一致する要求を画像なし局所取消→完了 |
| scope 失効 / drop | その context の旧 namespace と未採用画像を廃棄。終端受信を偽装せず、以降の旧通知は棄却 |

Pixels の通知契約を producer が明示する。
たとえば `SourcePixels` / `TerminalPixels` / `ProvisionalPixels` を payload の variant とし、
既存 origin の `from_cache()` や `GridItem::Audio` 判定で worker 終端を推測しない。
画像の provenance と通知の完了契約は別の意味である。

ドライブの間接 child pin は既存の早い seed 表示を保つ。検証結果が同一なら
Finalized で seed を確定し、異なるなら owner 内で旧 seed を破棄/置換して確定画像を採用する。
child miss / cancel では未採用 seed を同じ owner で破棄する。
置換時の upload-order token も更新し、古い queue 要素で新画像を二度処理しない。
通常の source 1要求には未採用画像を1枚だけ保持し、重複通知も画像 queue を増やさない。

### 3.3 owner API と処理順

必要な操作は begin、receive、next upload、adopt、discard、cancel queued、
invalidate scope / source、drop に絞る。段階ごとに別の保留集合や復帰 queue は追加しない。

1. UI は既存需要と origin から生成の必要性を決め、owner に受付可否を問い合わせる。
   begin に成功した要求だけ、同じ key で既存 worker queue に publish する。
   queue が無い・投入しない経路に空の owner を残さない。
2. poll は通知の相関を **最初に**検査する。generation / request ID が違う通知は
   pixels を drop するだけで、新 owner、表示、色代表 invalidation、virtual-folder writeback、
   auto aspect、page dims に触れない。
3. 一致する通知を owner の遷移へ渡す。必要な source Finalized の writeback は
   既存 cache-map が参照できる時点を保つ。cache read / GPU eviction を
   source 変更とみなす広域 invalidation を追加しない。
4. 未採用画像を予算内で取り出し、採用証明と keep / consumption policy を確認する。
   upload と Loaded / provenance / dims 等の公開が成立してから adopt を通知する。
   予算超過は同じ owner に画像を残す。keep 外・旧証明・破棄指示なら discard を通知する。
5. 完了条件が揃った owner だけを除去する。次の通常需要評価で既存の再ロード経路へ戻る。
   表示状態の一括 Pending 化、追加 repaint、時間待ちによる競合回避はしない。

## 4. producer / consumer と終了経路の棚卸し

下表は基点での入口と同型経路。実装時は型変更によるコンパイル検査に加え、
`ThumbMsg`、`LoadRequest`、requested 更新、backlog clear/retain の全呼び出しを再検索する。

| producer / consumer | 寿命・終了の扱い |
| --- | --- |
| 通常 grid の `update_keep_range_and_requests` | reload / heavy queue の両方を begin 経由にする。Pending / Evicted の表示は既存どおり。keep 外・prefetch suppression の queue prune は request key に相関した局所取消 |
| navigation / details hover / bookmark panel / still seek | `ensure_navigation_target_thumbnail_requests`、`ensure_still_seek_thumbnail_requests`、共通 `enqueue_priority_thumbnail` の投入を同じ owner へ。疎な exact set と既存 shared keep projection を維持 |
| `enqueue_idle_upgrades` | 旧 Loaded を描きながら同じ寿命で新画像を所有。旧 origin を見て重複 begin しない。完了後は採用画像の evaluated size / origin で再評価し、将来の拡大時 upgrade は維持 |
| regular / heavy worker | `spawn_thumbnail_workers` の pop 後 STALE、I/O resolve 後、PDF render 前の取消は元 key を返す。通知を送らず現行 owner を放置する exit を作らない |
| `process_load_request` の cache hit | 通常 cache、edit-preview cache、固定代表 cache、派生 WebP、cache-only を含む。TerminalPixels、または画像なし Failed / Canceled で一通知終端。backlog 内なら解除しない |
| `load_one_cached` の source 成功 | 通常画像、Folder 代表、ZIP cover / ZipImage / ZipDir、PdfFile / PdfPage、変換済み archive / Stack 代表。SourcePixels→保存/省略→Finalized。`should_save=false` も必ず終端通知 |
| source decode / resolve エラー | `send_thumb_failed`、各 cache-only miss、source failure は画像なし終端。デコード途中取消、PDF Interrupted は Failed と区別し既存 retriable 表示を保つ |
| RAW preview / half handoff | preview が十分なら既存 source 経路。不足なら同じ key を `RawThumbPending`、callback、follow-up queue、`finish_pruned_raw_half_followup` に渡す。handoff 自体は完了でない。取消通知と callback の遅着が後続 owner を壊さず、進捗の一回計上も維持 |
| Audio | generator は cache admission 後に一通知。Pixels は TerminalPixels、NoArt / Failed / Canceled は画像なし終端。sidecar / MP3 art / RAW sidecar、idle upgrade と cache revisit は既存の共通 dispatch を維持 |
| ドライブ代表 / child pin | seed→同一確定Finalized、seed→確定FinalCache、seed→child miss の各経路。seed の upload 有無に依存せず旧seedを閉じる。seed表示の省略や追加DB参照はしない |
| `spawn_video_thread` | pin blob→sidecar→Shell の既存優先順・worker内retryを維持。バッチ投入前に owner と key を確定し、終端画像/失敗を返す。grid の keep 外動画保持と tray 解放除外を維持。fullscreen-only policy を grid に変えない |
| ブックマーク marker の直接画像 | `install_bookmark_view_rows_with_sources` の backlog 直接 push（47196行付近）も begin→TerminalPixels の同期入口にする。workerなしを理由に相関検査を迂回しない |
| `poll_thumbnails` / upload backlog | 通知受信・優先排水・8枚予算・adopt/discard を owner API へ。NoArt、Failed、idx不正、旧世代、旧request、旧edit-preview epoch を個別に処理し、他の要求を idx だけで remove しない |
| grid / detached の消費policy | main の Grid poll と、mounted viewer 内の PassthroughRendition poll（61285行付近）で同じ owner 遷移を使う。動画keep外保持・auto aspectを駆動するpolicy差は維持し、mainの需要を兄弟へ投影しない |
| progress / repaint / idle判定 | `count_pending`、`update_progress_peaks`、`progress_snapshot`、idle recheck、tail repaint、tray tooltipとperf件数はownerの読み取り投影。表示中のupgrade、未採用画像、保存待ちを区別し、画像queueを要求数と二重計上しない。完了判定をconsumerへ分散させない |
| keep projection / eviction | `install_thumbnail_keep_projection` の差分、通常 grid eviction、details切替、hover終了、seek set終了。GPU eviction は採用済み事実を維持。未採用画像を捨てても source Finalized 待ちは残す。再入場で保存中の同じ需要を再投入しない |
| tray hide / restore | `release_gpu_resources` の backlog clear を owner の discard-all-images に置換。未終端workerを取消済みと推測しない。detach保持で現在 clear を通らない分岐も既存どおり。restore は通常需要の再評価だけで、owner再初期化や全通知無視をしない |
| items / worker session 更新 | drive一覧、`start_loading_items` / `install_new_items`、`invalidate_idx_state_and_queues`、search / smart / collection / bookmark一覧採用、pin更新時のworker再作成（43890行付近）。既存 generation / channel / cancel 境界で旧 namespace を retire、未採用画像を discard、旧queue / RAW ticket を取消 |
| 同一世代の明示更新 | `evict_thumbnail_for_reload`、edit-preview epoch clear、archive decision更新等。単なるGPU退去では owner を解除しない。source/採用証明が明示的に変わる更新だけを supersede とし、新keyで既存再要求機能を保つ。旧通知は全副作用の前に棄却 |
| Audio cache maintenance | `reset_audio_art_terminal_rows` の既存の対象parent・終端行限定更新を保つ。NoArt / Failed の再要求は終端済みなので新keyで受付可。main hide/restoreからこの全context更新を呼ばない |
| context mount / unmount / fork / park / retire | channel、queues、tokens、RAW tickets と同じ bundle の owner を移動。fork先にin-flight要求をcloneしない。park時のbacklog clearもownerのdiscardへ。retire/dropは対象contextだけ取消・wake・廃棄。兄弟をclearしない |
| Remote / cache creator / catch-up / neighbor prefetch / bench | 専用consumerと保存処理は本体要求を解放しない。共有payload変更の追従だけ行い、Remoteの一枚取得やcache admission、PDF background epoch=0を維持。benchの全型利用もコンパイル対象 |
| 色scan / 類似画像panel | `app/color_filter.rs::load_palette_for_request`、`ui_metadata_panel.rs` のpreview生成も専用channelで `process_load_request` を使う。standalone相関で新payloadへ追従し、本体grid ownerや兄弟のcancelを共有しない。palette抽出、一枚preview、RAW DedicatedWorkerの既存契約を保つ |

### 4.1 cancel / keep 退出・再入場

keep 退出は source namespace の失効ではない。画像を discard しても、
実行中の decode/save が終わるまで予約は残す。再入場時はその予約に重なる decode を始めない。
worker の Canceled、または source Finalized と画像 discard が揃えば通常要求が再開できる。
queue lock 下の未取得取消はその場で終端にする。RAW の half ticket / follow-up にも同じ相関を通す。
取消要求を出しただけで worker の終端を偽装せず、worker をUIで待たない。

### 4.2 scope 失効と late notification

items generation / context 終了 / worker channel の置換は、その旧要求空間を破棄する境界。
同じ旧需要を継続して再投入する操作とは区別する。局所取消で namespace 全体を捨てない。
同一世代の明示 source 更新も、単なる再表示ではなく既存の source stamp / edit epoch 等の
妥当性を変える境界に限る。**再投入抑制を逃れるために新IDを発行することは禁止。**
証明が変わらない再ロードは既存 owner の両終端を待つ。

採用、範囲外破棄、実行前取消、失効の後に届く旧 Finalized も、新要求の予約を解除しない。
旧 Pixels は upload しない。旧 Canceled / Failed / NoArt は新表示を Evicted / Failed / NoArt にしない。
Finalize の色代表 invalidation / virtual-folder writeback も相関検査より前に走らせない。

### 4.3 hide / restore と兄弟 context

hide は generation変更や要求全取消ではない。Loaded→Evicted のGPU解放と、
未採用画像の破棄を独立に記録する。source保存中なら ImageSettledAwaitingFinalized を保持する。
既に TerminalImageQueued なら破棄で完了する。以後通常需要が必要な再ロードを開始しても、
次の完成画像を採用するまで予約は維持されるため、安定需要は有限に消化される。
restore が Finalized より先でも後でも同じ規則で、追加のrestore専用pendingは不要。

`ViewerContextBundle` の保存・交換フィールドは owner へ機械的に追従する。
表示済みtextureや寸法をforkする既存仕様と、非同期要求ownerを共有しない規則を混同しない。
main の tray 操作で未変更の passive / detached context の要求・画像・世代・channel・
cancel token・shared keep projection を触らない。
既存の明示cache maintenanceが対象contextを更新する仕様は維持するが、
読み取り専用の窓操作をその更新経路へ接続しない。

detached predicate、viewport、HWND / presenter 所有、park の発火条件は変更対象外。
bundle内の寿命処理の追従に必要な範囲は独立レビューで確認する。
detached経路へ実変更する際は設計担当と独立レビューの構造合意を取り、
`detached-rework-plan.md` §11へ対象と理由を記録してから実装する。

## 5. 設計の簡素化と利用者への影響

`CLAUDE.md`「設計の簡素化」に従い、先に組み合わせを減らす選択肢を検討した。

| 選択肢 | 判断 |
| --- | --- |
| hidden main grid の需要そのものをなくす | GPU解放直後のhidden再ロードを減らせる可能性はある。ただしgrid以外にnavigation、hover、bookmark、seek、media、cache生成のconsumerがあり、復帰までの準備量も変わる。hidden需要の投影を別途整理し、機能維持を確認する必要がある |
| thumbnail処理をhiddenなら一括return | 採用/保存終端の回収、active media、viewport cleanupを止め得る。既存契約を維持できず、可視時の9枚目やLoaded upgradeにも寿命競合が残るため採らない |
| 読込中をモーダルにする / 窓を閉じて一覧を再構築 | 普通のスクロール・navigation・hideを制限し、既存の即時表示やcacheを失う。thumbnail背景処理には適さない |
| worker終端と画像所有を一つのownerへまとめる | 採用。表示状態との直積、Pendingだけの特例、restore専用の復帰機構を除去する。既存workerと需要の組み合わせは保つ |

**推奨: 今回は寿命修正を独立した変更単位とし、hidden需要の削減は別設計とする。**
時間待ち・新たなearly return・hidden用delay・upload予算の拡大でループを隠さない。
hidden schedulerの100ms制限やactive mediaのwake契約も変更しない。

利用者向け仕様変更は予定しない。画質、表示順、cache生成/保存方針、復帰時再ロード、
動画・音声再生、ZIP/PDF、編集・AI、疎なkeep、idle upgradeを維持する。
意図する改善は重複decode/画像蓄積/CPU消費が止まること。
同じ要求を二重実行していた部分では、結果または保存の終端を待つのが正しい寿命であり、
新たな時間制限や操作手順を加えるものではない。
実装調査で即時再表示や明示更新の機能維持と両終端規則が両立しない経路が出たら、
部分的なguardを入れず設計担当へ差分と選択肢を返す。

## 6. 性能・UI thread budget

- decode、Folder/ZIP走査、PDF render、WebP encode、DB saveは既存workerのまま。
  UIでjoin/待機/同期I/Oを追加しない。try_lock + sleepも使わない。
- request keyによる照合・遷移はHashMapの期待O(1)。全items走査や毎frameのbacklog検索で
  Finalizedを判定しない。owner内の画像queueを順に消費する。
- **8 textures/frame と backlog優先を維持**する。上限超過を解消するために一括uploadしない。
  大きな古いqueueを繰り返しcollect/再登録する構造を避け、同一要求のCPU画像は原則1枚。
  channel受信/queue処理も必要なら既存poll境界に収まる有界batchにし、無制限全件処理を増やさない。
- queue pruneやdiscardは既存queue/keep差分を対象とし、消えたorder tokenを溜め続けない。
  tray時の解放は当該contextの所有画像だけをdropする。
- progressの`cache_gen_done`はworker完了の既存意味を維持し、UI adoptで二重計上しない。
  requested/backlog件数はownerの読み取り投影として既存計装・repaint判定へ渡す。
- `--perf-log`有効時のbegin / pixels / finalized / adopted / discarded / canceledに
  request IDを載せる場合も外ガードを使う。requestごとの有限イベントとし、
  毎frameの反復ログや新たなheartbeatは足さない。
  analyzerのsame-work判定はrequest IDで分割しない（重複ごとに新IDなので欠陥を見逃す）。
- 合格はCPUを薄めたことではなく、入力/世代/需要が不変なら要求とbacklogが空へ収束すること。
  hiddenの排水速度が可視より低くても、同じ要求の画像を再生成せず、必要なwakeだけが残る。

`ui-responsiveness.md` §3.2の「サムネイルはスロットリング不要」は現実装の8枚予算と異なる
古い説明である。今回予算を撤去する根拠には使わず、実装時に§3.2を現行契約へ合わせる。

## 7. analyzer の独立修正

`scripts/analyze_perf.py::analyze_idle_health`（1957行付近）は全 `cat==input` を数える。
`src/modifier_probe.rs` は既存frameのinput hookでheartbeatを記録するだけでwakeを要求しない。
以下だけを idle の「操作」から除く。

```text
cat == "input" AND kind == "modifier_probe"
AND trigger == "heartbeat" AND keys == [] AND wheel == false
```

`focused=false` は今回のログの特徴であり、除外条件にはしない。フォーカス中のheartbeatも操作ではない。
修飾キーsnapshotの値だけも操作の証拠にしない。
key_event / wheel、keys非空 / wheel=true の矛盾したheartbeat、他のinput kind、
必須フィールドが欠けた未知形式は入力として数える。input全体やprobe全体は除外しない。
events数・frame/repaint/work反復の検査は維持し、probeの出力自体も残す。

analyzerだけ直しても提供窓は同一work116回とreason streakによりFAILでなければならない。
CPU/ログ量閾値、session/PID照合、空窓成立条件も緩和しない。

## 8. 回帰テスト・受入条件

sleepや本物の重いdecodeのタイミングに依存させず、純粋owner遷移と
fake通知を使うhandler-level poll/enqueue検査を組み合わせる。
現行 `source_coverage_survives_texture_backlog_finalize_and_stale_results`
（`src/app/tests.rs:36396`）のprovenance検査を保ち、Pendingだけのcoverageを広げる。

| テスト | 必須assert |
| --- | --- |
| 9枚目backlog: Pending / Evicted / Loaded-upgradeの3ケース | 最初の8枚で予算を使い、9枚目のsource Pixels→Finalizedを受信。ownerと画像が残り、通常enqueueとidle upgradeを評価しても再投入0。次pollの採用後だけ完了。Loaded-upgradeは旧texture/originを採用まで維持 |
| 順序の全組合せ | adopt→Finalized、discard→Finalized、Finalized→adopt/discard、Finalized→Pixels→adopt、重複終端。両条件が揃うまでbegin不可、揃った後に固着しない |
| keep退出・即再入場 | 実行前pruneは一度だけ取消。実行中/画像backlog/採用後保存中は画像側だけ終了し、Finalizedまで再投入しない。終端後に必要な再ロードが一度だけ開始 |
| hide/restore | 読込中 / 完了後、queued画像 / 採用済保存中 / Finalized済画像待ちを各々hide。ownerのdiscard経由で画像を解放。restoreが終端前/後でも固着・重複なし。hideを1回行った後のhidden需要が有限に収束 |
| 同一世代のlate notification | Aを終端/明示失効させBをbeginし、AのPixels / Finalized / Canceled / Failed / NoArtを全て注入。Bの予約・表示・origin・dims・writeback・色代表状態は不変 |
| items generation / channel / source証明の失効 | 同じidxへの旧結果を拒否。新世代/新edit epochは採用可能。単なるkeep退出やGPU退去では失効扱いにしない |
| 単一通知 | cache hit、edit preview、Audio Pixels、video、marker直接画像はFinalizedなしで画像採用/破棄後に終わる。Audio NoArt/Failed/Canceledは画像なしで終端。NoArt/Failedの通常静止再要求なし、明示cache maintenance後の再要求あり |
| ドライブseed | seed未upload/採用済みそれぞれで同一確定、別画像確定、child miss、cancel。旧seedの残存/再uploadなし、早いseed表示を維持 |
| PDF/ZIP/RAW | saveあり/skip、PDF Interruptedとharvest後save、ZIP/Folder resolve取消、RAW ticket取消/late callback/follow-up prune。request相関と進捗一回計上を保つ |
| sibling context invariance | mainと兄弟を同じidx/世代、別request keyとqueue/channelで用意。mainのhide/restore/keep退出/取消/世代変更/closeで兄弟の要求、backlog画像、texture ID、世代、token、queue、channel、shared keepが不変。mount/unmountはowner移動、forkはin-flight非共有、retireは当該contextのみ |
| 長時間相当のfake hidden lifecycle | 8枚より多い未cache Folder需要を既存のframe予算で処理し、Loaded-upgradeも混在させる。読込中hide/完了後hideとも重複要求0、backlogの同一key画像重複0、有限poll後requested/backlog空、安定需要の再評価で追加workなし |
| analyzer | focused有/無の純heartbeatはinput=0。key press/release・wheel・他input・矛盾heartbeat・欠損fieldsはFAIL。heartbeat+反復work/repaintはinput=0でもFAIL。提供窓のinput=0 / max_same_work=116 / FAILを照合 |

実装者が狭いowner / poll / RAW / context回帰と
`python scripts/test_analyze_perf.py` を実施し、cargo check、fmt、共有変更の
`scripts/test-full.ps1`まで集約する。独立レビューは設計と実装を別contextで検査し、
有効なテスト結果を再利用する。今回はこれらを実行していない。
UI配置・文字列変更は予定しないため新規visual snapshotは原則不要。

実装後はgreen treeから `scripts/build-dev.ps1` で利用者用coreを作り、
通常 `%APPDATA%\mimageviewer` profileと常駐版終了の注意を添えて手渡す。
idle-healthの性能測定はリリース担当が `build-release.ps1` のrelease coreで行う。
エージェントによる実アプリ検証は、別途具体的suiteの明示承認がある場合のみ
`prepare-portable-smoke.ps1` のdisposable portableで実施する。

## 9. 手順文の変更案（次工程）

`docs/idle-health-check.md` §3の4番と `CLAUDE.md` release checklist 9.7の
tray-residency説明を、**次の2ケースを両方必須**とする文章へ更新する。
今回は既存ファイルには書き込まない。

1. **読込中に格納**: cacheの無いFolder代表を多数含む一覧を開き、未完了要求が残る間に
   `[×]`でclose-to-tray。hideをまたぐ既存要求の寿命を検査する。
2. **読込完了後に格納**: 同じ種類の一覧で、通常ロードとidle upgradeが完了してから
   `[×]`でclose-to-tray。Loaded→EvictedのGPU解放に続く再取得の寿命を検査する。

どちらもトレイ常駐を先にONにし、最小化で代用しない。
同じ `-Scenario tray-residency` を2回使用し、ケース名、対象、cache状態、close時の
requested/backlog、hideから収束までの時系列と各reportをリリース記録へ残す。
scenario値の追加や自動操作suite（§1.361）はこの修正には含めない。

case間で勝手に利用者cache/設定を消さない。使い捨てfixture・隔離データ、または
利用者が用意したcache未作成の対象を使う。2回目が保存済みcache hitだけになった場合は
source二通知の条件を検査したことにならないので、条件を分けて記録する。
保存省略のsourceと保存ありのsource、cache hitの対照を自動テストで補う。

通常のwarmup 5秒・測定15秒、PID/session整合、測定開始/終了のvisible windows=0、
active mediaを静止caseへ混ぜない規則と既存閾値を保つ。
close操作自体は測定窓外に置くが、close直後からのwork/backlog経過も残す。
正当な初回loadが測定中に残った場合、即idleであると装わず準備条件と収束時点を記録し、
同じcaseの収束後の静止窓も測る。長時間の重複生成をwarmup延長で隠さない。
読込中caseは手動で時点を合わせにくいが、それを理由に完了後caseを無効扱いしない。
既存の利用者測定はGPU退去経路を検査した有効な証拠である。

実装時に `async-architecture.md` §3.4の表示状態による完了説明、
`display-pipeline.md` の通知契約、`ui-responsiveness.md` §3.2、
`docs/README.md` の設計索引も現行ownerへ合わせる。
backlog §1.360にはこのplanへの参照と進捗を残し、完了扱いは検収後にする。

## 10. 引き継ぎと判断事項

推奨案は「寿命ownerの集約 + 限定したanalyzer修正 + 二つのtray測定手順」で、
利用者に追加の操作制限や品質変更を求めない。現時点で利用者の選択が必要な未決仕様はない。
hidden需要削減は将来案であり、今回承認された仕様変更とは扱わない。

実装前の設計レビューでは、sourceの両終端、単一通知、seed検証、RAW継続、
同一世代の明示更新と古い通知、context局所所有を重点確認する。
この文書は設計担当/独立レビュアーの合意をまだ記録しておらず、実装着手の証拠にはしない。
今回の有効な検証証拠は、基点のソース照合と提供ログ/reportの読み取りだけである。

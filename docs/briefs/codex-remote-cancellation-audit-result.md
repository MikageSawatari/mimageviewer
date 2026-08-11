# リモート閲覧: ページ表示パイプライン設計レビューの結果 (2026-08-09)

依頼は `codex-remote-cancellation-audit-brief.md`。Codex (read-only、新規セッション) の回答を
そのまま記録する。**未実装。段階分けの範囲について合意待ち。**

## ClaudeCode の推奨と、確認した事実

推奨: **段階 A + B + C を実施し、D / E は保留**。

- A + B で今回の 3 件のうち Web 側 2 件 (§2.1 / §2.3) が構造的に消える。B は Web に閉じる
- C で、失敗してもシークバーと画面が食い違ったまま残らなくなる
- D は IPC protocol 変更を伴い、主な利得は本体側の「アドレス一致」近似の解消と、相乗り中の
  仕事を本体でも前景として扱うこと。correctness より無駄仕事の削減が中心と判断した

回答中の 2 つの主張は ClaudeCode が現物で確認済み:

- **ブラウザの abort は本体の処理を止めない。** `crates/remote-web/src/ipc_client.rs` の
  `PAGE_RESPONSE_TIMEOUT` のコメントに明記 ("not a claim that dropping the HTTP wait
  cancels core work")
- **前景 1 枠の予約は厳密な優先レーンではない。** heavy queue は FIFO で、サムネイルや
  コンテナ列挙が予約枠を使える (`src/remote_ipc/pipe.rs` の `remote_heavy_worker_count` と
  worker_loop)

---

# 以下、Codex の回答 (原文)

# 設計レビュー結果

結論から言うと、3件は独立した実装漏れではありません。現在の構造は、表示の必要性を知る層とキャンセル権を持つ層が分かれており、同型の誤りを誘発します。

ただし、「すべてを見開き単位のBlob／1ジョブにする」のも適切ではありません。

- 表示トランザクション・需要所有権・成功／失敗判定は見開き単位
- JPEG、デコード結果、LRU、実レンダリングはページ単位
- ページ単位ジョブへ、見開き単位の需要 lease を付ける

という混合構造への作り直しを推奨します。

## 1. §4.1 の評価

### 現在の流れ

現在は次の順です。

1. 本体が `page_groups` を決定
2. Web が要求位置を先に更新
3. 見開き内の各ページについて別々の `imageRequest` を生成
4. `loadMeasuredSpread` が `Promise.all` で各ページを個別に `loadForeground`
5. `PageResourceCache` がready cache／進行中prefetchへの相乗り／直接foreground取得を選ぶ
6. URLの `prefetch=1` からHTTP層が優先度を決定
7. 本体がページごとに別の`PageRequest`を処理
8. 全ページのfetchとdecode成功後にDOMを一度だけ差し替える

見開きの原子的DOM適用自体は正しいです。問題は、その手前の所有権です。

### 表示単位とページ単位の不一致

ページ単位の要求・キャッシュ自体は妥当です。見開き全体を一つのキャッシュentryにすると、次を失います。

- 単ページ↔見開き切替時の片ページ再利用
- 見開きの片側だけreadyだった場合の再利用
- ページごとのLRU・byte会計
- 2ページ並列レンダリング
- ページ別identity検証
- サイズ差が大きいページの独立管理

一方、キャンセル判断までページ単位にしたのは不適切です。現在の表示が必要とするページ集合は、`loadMeasuredSpread`が持つ見開き全体です。ところが、各`loadForeground`が個別に他ページをキャンセルしています。[app.js](/C:/home/mimageviewer-web/crates/remote-web/web/app.js:355)

したがって、正しい境界は次です。

| 責務 | 単位 |
|---|---|
| 表示要求・取消・成功／失敗・DOM commit | 見開きグループ |
| 前景／先読み需要の所有 | 見開きグループからページへのlease |
| 取得・JPEG・decode・ready cache・LRU | ページ |
| worker実行 | ページ。ただしグループ需要を参照 |

### 前景の先読み相乗り

相乗りは維持すべきです。相乗りしない場合の費用は大きいです。

実測は1ページあたりp50 1.5 MB、p95 7.9 MB、最大8.1 MBです。また取得・合成にはおおむね0.6～2.4秒かかっています。同じページをforegroundで取り直すと、CPU処理・JPEG encode・帯域を二重消費します。

さらに重要なのは、Webの`AbortController`で待ちを止めても、本体IPC処理の取消にはならない点です。IPC clientにも「HTTP waitをdropしてもcore workはcancelされない」と明記されています。[ipc_client.rs](/C:/home/mimageviewer-web/crates/remote-web/src/ipc_client.rs:39)

したがって「prefetchをabortしてforegroundを別取得」は、実際には両方が本体で走る可能性があります。

正解は相乗りを廃止することではなく、

- 前景が相乗りした時点でforeground leaseを追加する
- 有効優先度を`Prefetch → Foreground`へ単調昇格する
- foreground leaseがあるジョブは、どの先読み計画変更からもcancelできない

という構造にすることです。

### 打ち切りの目的

本体側の不要な先読み打ち切りは必要です。古いページ処理はCPU、PDF、ZIP、ディスクI/O、最終合成を消費するためです。

ただし、現状のWeb側abortは本体の枠を確実には空けません。主にブラウザ側の待ち・転送を止めているだけです。このabortを「worker解放策」として設計してはいけません。

また「foregroundを1枠予約」は厳密な優先レーンではありません。

- prefetch admissionは最後の1枠を使わない
- しかしcoreのheavy queue自体はFIFO
- thumbnail、containerなど他のheavy処理は予約枠を使える

という構造です。[pipe.rs](/C:/home/mimageviewer-web/src/remote_ipc/pipe.rs:414) [pipe.rs](/C:/home/mimageviewer-web/src/remote_ipc/pipe.rs:436)

現在の予約は有効な緩和策ですが、foreground優先を保証するschedulerではありません。

### URLの`prefetch=1`

これは責務の置き場所として不適切です。

HTTP層はURLから`PagePriority`を作り、本体へ渡しています。[http.rs](/C:/home/mimageviewer-web/crates/remote-web/src/http.rs:3177) しかしforegroundが既存prefetchへ相乗りすると新しいHTTP要求は発生しないため、本体からは最後までprefetchに見えます。

つまり優先度が「資源URLの属性」になっていますが、本来は「現在その仕事を待つconsumerの最大優先度」です。

優先度はジョブ状態として持ち、

```text
effective_priority = max(all active consumer demands)
```

で決めるべきです。URLは資源identityだけを表し、昇格・取消はtyped controlで行うのが適切です。

## 2. §4.2 打ち切り・破棄・無効化の入口

### Webページ経路

| 入口 | 「不要」の根拠 | 評価 |
|---|---|---|
| `PageResourceCache.schedule` | 新しいprefetch planにkeyがない | plan leaseの解放としては妥当。ただしforeground需要を別管理しているため危険 |
| `loadForeground`のactive走査 | 今要求している1ページとkeyが異なり、待機者0 | 見開き全体を知らない。§2.3の直接原因 |
| `abortUnownedActive` | `prefetchPlanned=false && foregroundWaiters=0` | §2.1対策だが、2つのfieldから需要を推測する暫定構造 |
| `clear()` | session変更、generation変更、viewer離脱、補正commit、画質変更等 | 文脈全体の失効として妥当。ただしbrowser abortだけでcoreは止まらない |
| `pump()` | ready/active重複は開始しない。503は再投入 | cancel入口ではない。503をcapacity observationとして扱う点は妥当 |
| ready eviction | entry数／byte予算超過時のLRU最古 | 完了済みBlobだけの容量管理。表示DOMは別object URLを所有するので妥当 |
| viewer `fetchController` | viewer破棄、session/generation失効、明示invalidate | 直接foregroundではHTTP fetchをabort。相乗り時は`awaitWithAbort`でその待機だけを止める |
| `LatestPageLoadQueue` | 新しい表示要求が来た | activeは止めず結果をsuperseded扱いにし、pendingは最新1件へcoalesce。安全だが最新表示まで古い処理を待つ |
| `loadSequence`不一致 | より新しい表示要求が存在する | DOM適用防止として妥当 |
| identity/session/generation検査 | 応答が要求snapshotと一致しない | correctness guardとして必須。処理後の破棄なので計算量は回収しない |
| `requestController` | folder/containerの新しいload ownerができた、screen cleanup | Web適用防止はcontroller・identity・ownerの三重照合で妥当。ただしcore workの取消にはならない |
| thumbnail controller | セルが不可視・再利用・破棄された | セルgenerationで誤適用を防ぐ点は妥当。active core thumbnailは残り得る |

`PageResourceCache.clear()`の主な呼び出しはsession ID変更、`remote_state_generation`変更、非media route、補正保存、画質変更です。[app.js](/C:/home/mimageviewer-web/crates/remote-web/web/app.js:931) [app.js](/C:/home/mimageviewer-web/crates/remote-web/web/app.js:1277)

### HTTP・本体ページ経路

| 入口 | 根拠 | 評価 |
|---|---|---|
| HTTP `IpcAdmission` | all/heavy/prefetch同時数上限 | capacity拒否であり、不要判定ではない。typed `AdmissionBusy`として分離すべき |
| core `try_acquire_prefetch` | queueあり、worker予約枠不足、prefetch上限 | foregroundを守る緩和策として妥当 |
| heavy queue `try_send`失敗 | bounded queue満杯／停止 | capacity/service failure。cancelと同じ`Busy`にまとめない方がよい |
| `begin_page_render` | foregroundのaddressと`spread_partner`に一致しないprefetch | §2.2修正後も責務が不適切。表示需要を推測している |
| `finish_page_render` | 当該prefetch完了 | token identityで個別解除する点は妥当 |
| session `begin_drain` | local切断、acquire barrier timeout、liveness/idle timeout、background expiry、別ownerによるsupersede | session全体のhard cancellationとして妥当 |
| worker内cancel確認 | session cancelまたはprefetch cancel tokenが立った | mechanismは妥当。誤りはtokenを立てるproducer側 |
| HTTP post-generation検査 | core処理中にgenerationが変わった | stale応答防止として必須。処理自体は既に完了している |
| `SessionOperation`の開始前／終了後owner検査 | session generation・owner・phase不一致 | 別ownerへの誤適用防止として妥当 |

本体の§2.2修正はaddressだけを仕事identityとして使っています。[container.rs](/C:/home/mimageviewer-web/src/remote_ipc/container.rs:103) しかし実際の出力identityにはtarget size、render context、補正preview、generation等も関係します。

たとえば同じaddressでも画質変更後のforegroundは、古いtargetのprefetchを「必要」と誤認して残します。次の正しさの穴というより、次の無駄仕事の穴が既に存在します。

### ページ以外

- サムネイル  
  セル単位で独立しているため見開き問題はありません。generationによる誤適用防止は正しい一方、Web abortがcore cancelにならないという資源浪費はページと共通です。

- Remote AI  
  `job_id`、request identity、typed terminal、明示DELETE、supersede、session drainをregistryが所有します。新しいgroup開始時も旧jobを`Superseded`へ遷移させます。ページ経路より構造的に健全です。[ai_job.rs](/C:/home/mimageviewer-web/src/remote_ipc/ai_job.rs:191)

- アーカイブ変換  
  request ID、recoverable job、明示cancel、owner、terminal state、session drainが一体です。画面destroy時もbest-effort DELETEを送ります。[app.js](/C:/home/mimageviewer-web/crates/remote-web/web/app.js:9107)

- 動画streaming  
  generation/session所有のcancelとtyped controlを持っています。ページの単発requestより長寿命ですが、所有境界は明確です。

ページ処理もAI／archiveほど重いregistryは不要ですが、「job identity・owner・typed terminal・単一cancel owner」という考え方は流用すべきです。

## 3. §4.3 設計案

### 推奨モデル

Webに次の概念を導入します。

```text
DisplayRequestId
PrefetchPlanId
PageResourceKey
Demand =
  Foreground(DisplayRequestId)
  | Prefetch(PrefetchPlanId)

ResourceEntry {
  key,
  state: Queued | Running | Ready | Failed,
  demands: Set<Demand>,
  effective_priority,
  job_id,
}
```

守るべき不変条件は次です。

1. 見開きforeground取得は、全ページのforeground leaseを同期的・原子的に登録してから開始する
2. ジョブを不要としてcancelできるのは`demands.is_empty()`のときだけ
3. session drainだけはleaseに関係なくhard cancelできる
4. foregroundが追加されたら有効優先度は必ず昇格し、prefetchへ降格しない
5. controller／core cancel tokenを触れるのはresource coordinatorだけ
6. キーはaddressではなく、出力を決める完全なrender identityにする

`loadForeground`は他のactiveを走査しません。代わりに次のような入口へ集約します。

```text
acquireForegroundGroup(display_request_id, [page1, page2])
replacePrefetchPlan(plan_id, ordered_page_keys)
releaseDisplayRequest(display_request_id)
invalidateSession(session_epoch)
```

`acquireForegroundGroup`が2ページのleaseを先に登録するため、1ページ目の処理中に2ページ目をcancelする順序は構造上発生しません。

### core側

最終形ではcoreにも`PageRenderCoordinator`を置きます。

- ページjobを完全なrender keyで管理
- foreground／prefetch consumerを記録
- queued jobはforeground laneへ昇格
- running jobはforeground需要ありとしてcancel対象から外す
- 明示的な`cancel/promote/release`をIPCで受ける
- foreground／prefetchのtyped laneを持つ
- `AdmissionBusy`、`Superseded`、`SessionEnded`、`CancelledAsUnneeded`、`RenderFailed`を分ける

現在のURL `prefetch=1`は移行後に意味上の正本から外します。互換期間中の入力表現として残しても、jobの有効優先度はcoordinatorのconsumer集合から決めます。

### なぜ入口が増えても壊れないか

新しい呼び出し側は「jobをcancelする」のではなく「自分のleaseをreleaseする」だけになります。

別のforeground、別のprefetch plan、現在の見開きなど他のconsumerが残っていれば、coordinatorの機械的な述語によりcancelされません。新しい入口ごとに「見開き相方なら除外」「待機者がいれば除外」と条件を追加する必要がなくなります。

例外はsession drainだけで、これは「このownerの仕事は全て無効」という上位のtyped transitionなので、ページ需要とは別の明示的なhard cancelとして維持できます。

### 表示失敗時の状態

`bool`戻り値は次へ置き換えます。

```text
PageLoadOutcome =
  Applied
  | Superseded
  | Cancelled(CancelCause)
  | Failed(DisplayFailure)
```

状態遷移は次です。

- `Applied`  
  DOM、displayed group、タイトル、seek、historyを同一commitで進める
- `Superseded`  
  後続要求がownerなので巻き戻さない
- 現在も最新の`Failed`／terminal cancel  
  requested groupをdisplayed groupへ戻し、seek/title/historyを整合させ、エラー表示だけ残す
- session loss  
  同様にpending表示を解除し、再接続状態へ遷移する

可能ならhistoryの`pushState`もDOM commitまで遅延させます。表示されなかった中間要求を閲覧履歴に残さないためです。

現在は要求位置を先に進め、`loadGroup=false`を単に無視しています。[app.js](/C:/home/mimageviewer-web/crates/remote-web/web/app.js:5002) [app.js](/C:/home/mimageviewer-web/crates/remote-web/web/app.js:5078) このtyped outcome化が位置不整合の根本修正です。

## 4. 既存修正の扱い

### §2.1 `foregroundWaiters`

最終的には置き換えます。

現修正は確認済みの競合を正しく防いでいるため、移行中は残すべきです。ただし`prefetchPlanned`と`foregroundWaiters`から所有状態を推測する構造は、一般化されたconsumer leaseに置き換えます。

### §2.2 address／spread partner保護

これも最終的には置き換えます。

移行中は必要ですが、address一致は完全なrender identityではなく、coreがWebの表示グループを推測する構造も残っています。`PageRenderCoordinator`導入後は削除し、foreground group leaseを正本にします。

### §2.3

`Promise.all`の前に2ページの待機数を増やすだけの修正にはしません。`acquireForegroundGroup`で全ページ需要を原子的に取得し、`loadForeground`から他jobのcancel走査を撤去するのが正しい修正です。

### `4cefa36a`の予算・同時数・503

次は維持します。

- 要求解像度
- 48 MiBの実効下限
- entry上限12
- 前方3／後方1
- 最大prefetch 2
- admission 503を計画末尾へ戻す扱い
- `page_display` telemetry

変更するのは、503 capacity failureと意図的cancel／session失効をtypedに分ける部分です。

## 5. 実装の段階分け

### 段階A: 契約と回帰テスト

- 3件すべてを同じテスト群へ統合
- 2ページともprefetch中、片方だけprefetch中、古いplan完了との競合を追加
- terminal failureとsupersedeの区別を純粋状態遷移テストで固定
- `page_display`形式を維持

確認できること: 現構造の壊れる条件と、新しい不変条件がテストで明文化される。

### 段階B: Webのgroup lease化

- `acquireForegroundGroup`
- prefetch plan lease
- resource coordinatorのみがabortを所有
- `loadForeground`の他active cancelループを撤去
- §2.1のwaiter countをconsumer集合へ置換

確認できること: §2.1と§2.3が構造的に消える。coreの§2.2修正はまだ残す。

### 段階C: 表示状態とtyped outcome

- `PageLoadOutcome`
- requested／displayed／pendingの単一状態機械
- terminal failureだけrollback
- DOM・seek・title・historyのcommit境界を統一

確認できること: 取得失敗、decode失敗、identity拒否、session失効でもシークと画面が食い違ったまま残らない。

### 段階D: end-to-end job ownership

- IPC protocol更新
- full render keyとjob ID
- explicit promote／release／cancel
- coreのforeground／prefetch lane
- cancellation causeとadmission failureの型分け
- browser abortとcore cancelの接続

確認できること: foreground相乗りが本体でもforegroundとして扱われ、本当に不要な仕事だけをcoreで停止できる。

### 段階E: 旧例外の撤去と性能確認

- §2.2のaddress／partner例外を削除
- URL `prefetch=1`を優先度正本から撤去
- `foregroundWaiters`等の互換field削除
- iPadで相乗り率、promotion、cancel reason、foreground待ち時間を計測
- `page_display`へ非機密なdisplay request相関とtyped cancel reasonを追加

確認できること: 個別条件を撤去した状態でも3件の回帰がなく、既存の先読み効果・48 MiB予算・画質を維持できる。

今回、コード変更・ビルド・コミットは行っていません。
---

## 段階分けの第二意見 (別セッションの Codex、2026-08-09)

「A+B+C 実施 / D+E 保留」の妥当性を、独立した read-only セッションに問うた結果。
**結論は反対。** D のうち所有権の境界 (job ID / 昇格 / 明示 release / cancel 理由の型分け /
旧アドレス近似の撤去) は B+C と同じ cutover に入れる必要がある、という指摘。

ClaudeCode が指摘の根拠を実物で確認し、**判断を変えた**。確認内容と最終決定は
git 管理下の [../web-remote-plan.md](../web-remote-plan.md) §14 にある (本ファイルは git 管理外)。

以下、回答全文。

結論は、**A + B + C は必要だが、D 全体を保留して correctness 対応完了とする判断には反対**です。暫定マイルストーンとしては成立しますが、D のうち「昇格・取消所有権・typed cause」は B と同時に必要です。E の性能計測などは保留できます。

1. B は Web 側 2 件を構造的に消せる

次を同時に満たせば、§2.1／§2.3 は消えます。

- 見開き全ページの foreground lease を fetch 開始前に同期登録
- plan 更新は plan lease の解放だけ
- `demands.is_empty()` のときだけ abort
- [`loadForeground` の他 active cancel 走査](/C:/home/mimageviewer-web/crates/remote-web/web/app.js:355)を撤去
- `foregroundWaiters`／`prefetchPlanned`を同じ変更で削除

ただし保証範囲はブラウザ内だけです。本体は Web の lease を知らないため、end-to-end では構造的解決になりません。

2. D 保留は「無駄仕事だけ」ではない

具体的な破壊経路があります。

1. Web foreground が先読み P に相乗りし、B の lease を持つ。
2. 本体では P は依然 `PagePriority::Prefetch`。
3. 補正プレビューは cache/coordinator 外から独立した foreground `/api/page` を送ります。[app.js](/C:/home/mimageviewer-web/crates/remote-web/web/app.js:7793)
4. プレビュー fetch 開始後にページ移動しても、その HTTP／core 処理を止める signal はありません。
5. 遅れて本体へ到達したプレビューが、アドレス／`spread_partner` に一致しない P を取り消します。[container.rs](/C:/home/mimageviewer-web/src/remote_ipc/container.rs:965)
6. P は `MediaErrorCode::Busy` になり、HTTP では generic `miv_media_error` 503になります。[http.rs](/C:/home/mimageviewer-web/crates/remote-web/src/http.rs:3465)
7. Web の foreground join が特別扱いするのは `ipc_busy` だけです。[app.js](/C:/home/mimageviewer-web/crates/remote-web/web/app.js:374)

IPC 内部の有限回 retry で通常は救済され得ますが、不変条件としては「foreground lease のある仕事が別 foreground に取り消される」ままです。retry が尽きれば表示要求が失敗します。C は位置を巻き戻せても、誤取消そのものは直しません。

また相乗り中の PDF は本体で `Normal` lane のままで、foreground の `HighNormal` へ昇格しません。[thumb_loader.rs](/C:/home/mimageviewer-web/src/thumb_loader.rs:2508) これは単なる無駄仕事ではなく、priority inversionによる待ち時間・timeoutの問題です。

なお、同一アドレス・異なる target/revision の仕事を余分に保持するだけなら、現在の Web cache key が target、revision、generation、session、render contextを含むため、主に無駄仕事です。[app.js](/C:/home/mimageviewer-web/crates/remote-web/web/app.js:5215) 危険なのは過剰保持より、別入口による誤取消です。

3. B と C は一体で設計すべき

`DisplayRequestId` は lease ownerであると同時に、`Applied / Superseded / Failed` の outcome ownerです。特に supersede 時のlease解放タイミングを誤ると、現在の「core IPC は完了させる」というキュー契約を変え、ブラウザだけ abortして本体に未追跡処理を残します。

したがって A で outcome 契約を先に固定し、B+C は同じcutoverで入れるのが安全です。ただし完全な `CancelCause` は現行 protocol では表現できず、その部分は D と不可分です。

4. 暫定条件との併存は危険

旧フィールドと lease の dual-write は避けるべきです。一方が「所有あり」、他方が「所有なし」と判断すれば、早すぎる abortか解放漏れになります。

同様に、D を入れて E まで旧 `begin_page_render` 近似を残す切り方も危険です。新coordinatorと旧アドレス近似が二つのcancel ownerになるため、旧近似の無効化・撤去はDのcutoverに含める必要があります。

5. 推奨する切り方

- A: 契約・回帰テスト
- B+C+D0を一体化:
  - Web group lease
  - group outcome/state machine
  - job ID
  - prefetch→foreground promotion
  - explicit release/cancel
  - cancel/admission/session causeの型分け
  - 旧アドレス／partner cancel ownerの無効化
- 後続:
  - foreground専用laneの高度化
  - URL `prefetch=1`互換撤去
  - telemetry拡張、性能計測、残存field整理

Dを先に入れるのは、現行Webの誤ったページ単位cancelを本体まで確実に伝えてしまうため不適切です。**B+Dの所有権境界を一体で切り替え、性能・観測部分だけを後段へ送る**のが最も安全です。

実装・ビルド・コミットは行っていません。
# 表示所有権の cutover 段階 1 — 契約と状態機械を純粋テストで固定する

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。

## 0. 前提 — 先に読むもの

- [`docs/web-remote-plan.md`](../web-remote-plan.md) **§14 / §14.5 / §14.5.1 / §14.6 / §14.6.1 / §14.7**
- `crates/remote-web/web/app.js` の `PageResourceCache` (450-760 行付近)、
  `LatestPageLoadQueue` (366-448 行付近)、`imageRequest` (5873 行付近)、
  `updateViewerImage` (5609 行付近)、`loadMeasuredImage` / `loadMeasuredSpread` (11194 行付近)
- `src/remote_ipc/container.rs` の `begin_page_render` (965 行付近)

**これは 3 段階のうちの段階 1 である。**

1. **(今回)** 契約と状態機械を純粋モジュールとして追加し、テストで固定する
2. 本体側の基盤 (ページジョブ registry、typed cause、promote / release) を dormant で追加する
3. B + C + D0 を一体で cutover する。既存の打ち切り所有者はこのときまとめて撤去する

**今回は実機の挙動を変えない。** 新規ファイル 2 つ + ドキュメント追記だけで、`app.js` からは
一切 import しない。既存の打ち切り所有者 (`loadSequence` / `fetchController` /
`foregroundWaiters` / `prefetchPlanned` / `abortUnownedActive` / 本体の `begin_page_render`) は
**この増分では 1 行も変えない**。動く実装と、これから正しくなる契約が、しばらく並存する。

## 1. なぜ先に契約だけを固定するのか

同じ不具合 (「シークバーだけ進んで画面が前のページのまま残る」) を 3 回、別々の場所で
直した。3 件は独立した実装ミスではなく 1 つの誤りの現れである。

> **打ち切りの判断はページ単位でされているのに、「必要か」は表示グループ単位で決まる。**

打ち切りの入口 (Web の先読み計画、本体の前景描画、見開きの兄弟、補正プレビュー) が
それぞれ独自の条件で cancel token を触っていたため、入口を 1 つ足すたびに次の場所で
同じ形の不具合が出た。cutover の要点は入口を減らすことではなく、
**入口が cancel token を触らないこと**である。入口は lease を取る / 返すだけにし、
実際の打ち切りは需要が空になったことを見て 1 か所が決める。

段階 3 は Web と本体を同時に切り替える大きな増分になる。その前に、
**判断そのものを DOM も fetch も timer も持たない純粋な状態機械として書き出し、
テストで固定しておく。** そうしないと、cutover の実機不具合が「配線ミス」なのか
「判断の設計ミス」なのか切り分けられない。

## 2. 追加するもの

`crates/remote-web/web/page-coordinator.mjs` と `crates/remote-web/web/page-coordinator.test.mjs`
の 2 ファイルのみ。既存モジュール (`command-core.mjs` 等) からの import は可。
**`app.js` は変更しない。**

### 2.1 `pageResourceKey` — 完全なページ資源キー

現在のキーは `imageRequest` の中で `\n` 連結した文字列として組み立てられている
(5920-5922 行)。これを型のある関数へ出す。

```js
pageResourceKey({
  address,            // string  住所の identity (mediaImageInfoKey 相当)
  targetPx,           // number  要求する長辺
  renderRevision,     // number|string  補正保存などで進む版
  generation,         // string  本体状態の世代
  sessionId,          // string  remote session の identity
  sessionCacheEpoch,  // string  session に従属する非 secret nonce
  renderContext,      // object|null {context_address, display_slot, spread_partner}
  adjustmentPreview,  // object|null
}) -> { id: string, cacheable: boolean }
```

規則:

1. **バイト列に影響する要素をすべて含める。** `display_slot` と `spread_partner` は
   view trim の側面判定に使われるので (`container.rs` の
   `remote_view_trim_plan` = 3064-3086 行)、バイト列に影響する。含めること
2. **`sessionCacheEpoch` を明示的に含める。** 現在の実装では epoch は session id の
   変化からのみ生成され (`applyRemoteSessionId`)、cache も同時に clear されるので
   実害は出ていない。しかしそれは**偶然成り立っている不変条件**であって、キーの定義
   ではない。キーへ入れておけば、将来 epoch だけを進める経路が増えても壊れない
3. **区切りに曖昧さを残さない。** 現在の `\n` 連結は、値の中に `\n` が入ると
   隣の欄との境界が消える。`JSON.stringify` した配列 (欄の順序は固定) にすること
4. **object は正規化してから直列化する。** キーの順序が違うだけの `renderContext` は
   同じ id にならなければならない。再帰的に key をソートする
5. `adjustmentPreview` が非 null なら `cacheable: false`。preview の内容自体も id に
   含める (別のプレビューは別のジョブである)

### 2.2 `PageDisplayCoordinator` — 需要と優先度の単一 owner

**純粋な状態機械にする。** `fetch` / `AbortController` / `setTimeout` / DOM / `performance` を
使わない。すべての mutation は**副作用の記述 (effect) の配列**を返し、それを実行するのは
段階 3 で書く呼び出し側である。coordinator 自身が呼び出し側へ同期 callback しないこと
(順序と再入の事故を作らないため)。

```js
class PageDisplayCoordinator {
  constructor({
    hasBytes = () => false,        // (keyId) => boolean   保持済みか (cache が答える)
    prefetchAdmits = () => true,   // () => boolean        予算が新規開始を許すか
    prefetchConcurrency = 2,
  } = {})

  nextDisplayRequestId()                              // "1", "2", ... 単調増加
  openDisplay({ requestId, groupKey, keys })  -> effects[]
  releaseDisplay(requestId, cause)            -> effects[]
  setPlan(keys)                               -> effects[]   // 近い順。plan lease の総入れ替え
  settle(jobId, outcome)                      -> effects[]   // {status:"ready"|"failed"|"aborted", reason?}
  invalidate(cause)                           -> effects[]   // session 失効 / cache clear

  protectedKeyIds()                           // 需要のあるキー (表示が先、次に計画順)
  jobFor(keyId)                               // {jobId, priority, state} | null
  openRequestIds()
}
```

**役割の境界:**

- coordinator が持つのは**需要・優先度・ジョブの生死・グループの結果**だけ
- **バイトの保持と破棄は `PageResourceCache` が持ち続ける。** coordinator は
  `hasBytes` / `prefetchAdmits` を通して読むだけで、eviction を決めない
- **再送 (retry) は持たない。** `Retry-After` と timer が要るので純粋にならない。
  呼び出し側が再送しても coordinator から見れば同じジョブのままである。
  `settle` は終端の結果 (`ready` / `failed` / `aborted`) だけを受ける

**なぜ prefetch の開始まで coordinator が持つのか**: 開始を cache 側に残すと、
ジョブ登録簿が cache と coordinator の 2 つになる。昇格は「cache が持っている in-flight を
coordinator が養子に取る」形になり、CLAUDE.md が禁じている「相互排他な状態を 2 か所で
表現する」形そのものになる。よって**すべてのジョブ ID は coordinator が発行する**。
予算と同時実行数は数値・述語として渡すだけにする。

### 2.3 effect の語彙

| effect | 内容 |
|---|---|
| `start` | `{jobId, keyId, priority, requestId?}` GET を開始せよ |
| `promote` | `{jobId, keyId}` 実行中のジョブを前景へ昇格させよ |
| `cancel` | `{jobId, keyId, cause}` 実行中のジョブを打ち切れ |
| `group_ready` | `{requestId}` そのグループの全ページが揃った |
| `group_failed` | `{requestId, keyId, reason}` そのグループは表示できない |
| `ignored` | `{reason, ...}` 何もしなかった。**理由を必ず型で残す** |

`priority` は `"foreground"` / `"prefetch"`。`cause` は `"no_demand"` /
`"session_invalidated"` / `"context_reset"`。`group_failed` の `reason` は
`"member_failed"` / `"member_aborted"` / `"session_invalidated"` / `"context_reset"`。
`ignored` の `reason` は `"unknown_job"` / `"stale_job"` / `"already_settled"` /
`"unknown_request"` / `"duplicate_request_id"` 等、**その場で名前を決めて定数へ出す**。

`ignored` を必ず返すのは、無言の早期 return を作らないためである。原因がログから
見えない不具合を 2026-08-11 に複数踏んでおり、**直す前に無言の分岐へ型付きの理由を足す**
のが今の運用方針である。

1 回の mutation が返す effect の順序は固定する: `ignored` → `cancel` → `promote` →
`start` → `group_ready` / `group_failed`。終端通知を最後に置くのは、呼び出し側が結果へ
反応する前に「もう要らない仕事」を止め終えているようにするため。

## 3. 固定する契約 (これがこの増分の成果物)

1. **ジョブが打ち切られるのは需要が空になったときだけである。** どの入口も cancel を
   直接起こさない。例外は `invalidate` (session 失効 / context reset) の 2 つで、これは
   需要ごと無効化する操作として明示的に区別する
2. **有効優先度は需要の最大値であり、単調に上がる。** 先読みとして開始したジョブに表示
   需要が付けば `promote` を 1 回だけ出す。表示需要が外れても prefetch へ**降格しない**。
   降格が要るなら needs が空になっているはずで、そのときは打ち切りになる
3. **`promote` は 1 ジョブにつき高々 1 回。** 前景として開始したジョブは `promote` を
   出さない
4. **開始していないジョブの打ち切りは effect を生まない。** 計画にしか載っていない
   キーの需要が消えても、サーバへ送る仕事は無い
5. **表示グループは 1..2 ページを持ち、結果は高々 1 回。** 全員 `ready` で `group_ready`、
   1 枚でも終端失敗すれば `group_failed`。**部分適用は無い (片側失敗)**。失敗した側と
   逆側の取得済みバイトはそのまま (破棄は cache の予算判断であってここではない)
6. **解放後の要求へ結果を出さない。** `releaseDisplay` 済みの要求に対して後から
   `group_ready` / `group_failed` を出さない。遅れて届いた結果は `ignored` になる
7. **遅れて届く GET は現在の要求に影響しない。** `settle` は jobId で照合する。
   同じキーで新しいジョブが走っているとき、古い jobId の `settle` は
   `ignored{reason:"stale_job"}` で、新しいジョブにも要求にも触れない
8. **release は冪等。** 同じ requestId を 2 回解放しても二重に需要を減らさない
   (`ignored{reason:"unknown_request"}`)。同じ requestId で 2 回 `openDisplay` するのは
   契約違反なので `ignored{reason:"duplicate_request_id"}` を返し、状態を変えない
9. **同じ要求が同じキーを 2 回要求しても需要は 1 つ。** 見開きの左右が同じページを
   指すことは通常無いが、需要の数え方が要求の重複に依存してはいけない
10. **`protectedKeyIds()` は表示需要のあるキーをすべて含む。** cache はこれを保護集合に
    使えなければならない (段階 3 で `visibleKeys` を置き換える)

## 4. 触らないもの

- `app.js` (import も追加しない)。`PageResourceCache` / `LatestPageLoadQueue` /
  `loadSequence` / `fetchController` / `foregroundWaiters` / `prefetchPlanned` /
  `abortUnownedActive` は**現状のまま**
- 本体 Rust (`src/`)、`crates/remote-ipc`、`crates/remote-web/src/`。**protocol version は
  上げない** (上げるのは段階 3)
- `page_display` テレメトリ、段階 A の `applied` / `superseded` / `failed` outcome
- 先読みの窓 (12 / 4)、予算 64 MiB、`pageResourceAdmissionPlan` の保護集合の規則
- 位置の所有権 (requested / displayed の単一 owner、§14.5.1 の 2 経路)。**段階 3 で扱う。**
  今回は `openDisplay` に `groupKey` を持たせておくだけにして、段階 3 で位置所有者を
  乗せられる形を残す。中途半端に位置を動かす実装を今回入れないこと

## 5. テスト

```
cd crates/remote-web/web && node --test
```

**`pageResourceKey`**

- 各欄を 1 つずつ変えると id が変わる (address / targetPx / renderRevision / generation /
  sessionId / sessionCacheEpoch / renderContext の各欄 / adjustmentPreview)
- `renderContext` の key の並び順が違うだけなら同じ id
- 値に区切り文字 (`\n`、`"`) を含めても、隣の欄との境界が壊れない
  (例: `address: "a\nb", targetPx: 1` と `address: "a", ...` が衝突しない)
- `adjustmentPreview` が非 null なら `cacheable: false`、内容が違えば id も違う

**状態機械 — 基本**

- 表示要求を開いて全ページが `ready` になると `group_ready` が 1 回だけ出る
- 保持済み (`hasBytes` が真) のキーだけの要求は、`start` を出さずに即
  `group_ready` になる
- 見開きの片側が終端失敗すると `group_failed` が 1 回、逆側の `ready` は保たれる
- 先読みで開始したキーに表示需要が付くと `promote` が 1 回。2 回目の表示需要では
  `promote` を出さない。前景で開始したジョブは `promote` を出さない
- 表示需要が外れても、計画がまだ需要を持つ間はジョブが生き続け、優先度は下がらない
- 表示需要も計画需要も無くなった瞬間に `cancel{cause:"no_demand"}` が 1 回出る
- `setPlan` で計画から外れたキーの実行中ジョブは打ち切られ、まだ開始していないキーは
  effect を生まない
- `prefetchAdmits` が偽の間は先読みの `start` が出ず、真に戻して mutation を起こすと出る。
  **前景の `start` は `prefetchAdmits` に関係なく出る**
- 先読みの同時実行数が `prefetchConcurrency` を超えない

**状態機械 — 遅れて届く / 失効**

- 解放済み要求のキーが後から `ready` になっても `group_ready` は出ず、`ignored` になる
- 同じキーで新しいジョブが走っているとき、古い jobId の `settle` は
  `ignored{reason:"stale_job"}` で、新しいジョブの状態を変えない
- 同じ jobId を 2 回 `settle` すると 2 回目は `ignored{reason:"already_settled"}`
- `invalidate` は実行中の全ジョブへ `cancel` を、開いている全要求へ `group_failed` を
  1 回ずつ出し、その後に届く `settle` はすべて `ignored`
- `releaseDisplay` の二重呼び出しで需要が二重に減らない

**状態機械 — 有界な網羅列挙**

操作の語彙を小さく固定し (`openDisplay(A)` / `openDisplay(B)` / `setPlan([...])` /
`settle(ready)` / `settle(failed)` / `releaseDisplay` / `invalidate`)、**長さ 4 以下の
全列を機械的に回して**、各ステップ後に次の不変条件を検査する。乱数を使わないこと
(再現しない失敗を作らない)。

- 実行中のジョブで需要が空のものが残っていない (その mutation の `cancel` を除く)
- `cancel` が出たジョブは、需要が空になったか `invalidate` のどちらかである
- 優先度が下がったジョブが無い
- どの要求も終端通知は高々 1 回、かつ解放後には出ていない
- `protectedKeyIds()` が、開いている表示要求のキーをすべて含む
- 返る effect の順序が §2.3 の規定どおり

## 6. ドキュメント

`docs/web-remote-plan.md` に **§14.9** を追加し、次を記録する (`docs/briefs/` は git 管理外
なので、決定はここへ書き戻さないと次のセッションが逆を実装する)。

- 段階 1 で固定した契約 (§3 の 10 項目)。特に「打ち切りは需要が空のときだけ」
  「優先度は単調」「片側失敗は部分適用しない」
- coordinator と `PageResourceCache` の役割境界 (需要と優先度 / バイトの保持と破棄) と、
  **ジョブ ID の発行を coordinator に一本化した理由** (登録簿を 2 つにしないため)
- retry を coordinator に持たせない理由
- `pageResourceKey` に `sessionCacheEpoch` を入れた理由 (現状は session 変化からのみ
  epoch が進むので実害は無いが、それは偶然の不変条件である) と、区切りを曖昧にしない直列化
- 位置の所有権を段階 3 へ送ったこと (§14.5.1 の 2 経路は今回塞がらない)
- この増分が dormant であること (`app.js` から未使用。実機の挙動は変わらない)

ユーザー向けマニュアル (`htdocs/`) は変更しない (利用者から見える変化が無い)。

## 7. 実行と報告

- `cd crates/remote-web/web && node --test` を**毎回実行**して結果を報告する
- `crates/remote-web` の Rust に触らない予定だが、万一触ったら**全箇所と理由を報告**する
- **`scripts/build-dev.ps1` を実行しない** (稼働中の本体と remote サービスを止めてしまう)。
  ビルドは ClaudeCode 側で行う
- **コミットしない** (ClaudeCode が行う)
- 設計から外れた判断をしたら、理由とともに報告する。特に coordinator の API 形が
  §2.2 と変わった場合は、段階 3 で何が変わるかまで書くこと

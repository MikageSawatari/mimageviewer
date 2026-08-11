# 段階 A: 表示グループの outcome 契約 (実装)

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。

**設計レビュー済み。合意した内容で実装してください。** 前身の設計ブリーフに対する
あなたの指摘 (§3.2 に列挙) はすべて採り入れてある。

## 0. 前提 — 先に読むもの

[`docs/web-remote-plan.md`](../web-remote-plan.md) **§14「ページ表示パイプラインの所有権」**が
決定の正本。

段階は次のとおり。**今回は A だけ。**

- **A** — 表示グループの outcome 契約と回帰テスト ← 今回
- **B+C+D0** — 所有権の cutover (Web の group lease、本体の job ID・昇格・明示 release・
  cancel 理由の型分け、旧アドレス近似の撤去、protocol 版数)
- 後段 — 前景専用 lane、`prefetch=1` 互換撤去、性能計測

## 1. 直したい状態

ページを捲って表示が**終端失敗**したとき、シークバーは新しいページを指したまま、画面は
前のページのまま残る。利用者はどちらが本当か分からない。実機報告では、左上に
「ページの編集結果合成は取り消されました」が出た状態で、シークバーは 2,3 ページなのに
画面は 1 ページ目のままだった。

**A の約束**: 位置を動かす表示要求が終端失敗したら、アプリの位置状態・シークバー・画面の
3 つが一致した状態へ収束し、失敗した事実が利用者に見える。

## 2. 調査結果 (確認済み。再調査は不要)

### 2.1 巻き戻しの機構はすでにある

`app.js` の `discardRequestedPageGroup` が要求位置を表示位置へ戻す。純関数
`viewerPagePositionTransition` の `ViewerPagePositionEvent.DISCARD` を使い、
`command-core.test.mjs` に単体テストもある。

**しかし呼ばれるのはシークバー経路の 2 か所だけ** — `commitSeekGroup` の中で、セッション
取得に失敗したときと `commitRequestedPageGroup` が false を返したとき。表示自体の失敗では
呼ばれない。

### 2.2 `false` が 3 つの意味を潰している

`loadGroup` → `loadMeasuredImage` / `loadMeasuredSpread` は `bool` を返す。

| 実際に起きたこと | 現在 | 正しい扱い |
|---|---|---|
| 新しい要求に追い越された (`sequence !== this.loadSequence`) | `false` | 何もしない |
| 中止された (`AbortError`)。ただし `sequence` は現行 | `false` | **終端失敗** |
| 取得・デコード・適用が失敗した | `false` (`title` にメッセージ) | **終端失敗** |

2 行目が今回の不具合。catch は先に `sequence !== this.loadSequence` を判定済みなので、
そこを通り抜けた `AbortError` は**自分より新しい要求ではない誰かに中止された**ことを意味する。

## 3. 実装する設計

### 3.1 型

`command-core.mjs` に表示グループの結果型を導入する。テレメトリの
`viewerPageDisplayHistoryEvent` (観測用) と紛らわしいので、名前は
**`ViewerGroupLoadOutcome`** とする (あなたの推奨を採用)。

    { outcome: "applied" }
    { outcome: "superseded" }
    { outcome: "failed", message }

後処理の判定も純関数にする。項目名はあなたが決めてよい。

- `applied` — 後処理を実行。巻き戻さない
- `superseded` — 何もしない。新しい要求が位置を確定させる
- `failed` — 位置を巻き戻し、`message` を表示。後処理は実行しない
- **未知の outcome は黙って扱わない** (throw する)

### 3.2 レビューで確定した 4 点

**(a) 境界に `LatestPageLoadQueue` を含める。**
`loadMeasured*` だけ型付けしても足りない。queue 自身が `false` を返す箇所が 4 つある —
pending 追い越し (`request` の `supersededPending.resolve(false)`)、`clear()`、active 追い越し
(`runActive` の `ticket.superseded` 両分岐)。これらは全部 `superseded` である。
型付けを怠ると、**追い越されただけの要求が `failed` に分類されて位置を巻き戻す** =
現状より悪い不具合になる。

`runActive` の catch は `ticket.superseded` でなければ `reject` する。例外が
`loadGroup` の呼び出し側まで伝播する経路も契約に含めること。

**(b) 巻き戻すのは「位置変更を伴う表示要求」だけ。**
`updateViewerImage` は位置を動かさない経路からも呼ばれる (実測 10 か所。fit 変更、
補正保存、viewport resize、generation 更新、`refreshContainerSpread` など)。無条件に
巻き戻すと resize の失敗でページが動く。

呼び出し側が**この要求が位置変更を所有していることを明示的に宣言**したときだけ巻き戻す。
新しいグローバル状態や bool は増やさず、要求時にスタックローカルで捕捉する。

**(c) 完了時に identity で照合する。**
`requestedGroupIndex` の数値だけでは足りない。`refreshContainerSpread` は
`loadContainer` で `state.pageGroups` を作り直してから `state.pageGroupIndex` を代入するので、
**古い `displayedGroupIndex` を新しい配列へ適用すると別ページを指し得る**。要求時の
group identity / context を捕捉し、完了時に現在の要求と一致するときだけ巻き戻す。

**(d) 失敗メッセージは結果に載せる。**
現在の非 Abort エラーは viewer の title に出るが、巻き戻しが `syncPagePositionFeedback` を
通って `this.title.textContent = requested.name` でページ名に上書きしてしまう。
メッセージを `failed` の結果に載せ、**巻き戻した後に**表示すること。

`AbortError` で `sequence` が現行の場合 (= §2.2 の 2 行目、audit ブリーフの §2.3) も
`failed` として巻き戻し、「ページの表示が中断されたため、前のページに戻りました。」程度の
一時メッセージを出す。黙って戻ると利用者には別の不具合に見える。

### 3.3 viewer 側

`loadMeasuredImage` / `loadMeasuredSpread` / `load` / `loadGroup` の戻り値を上の型にする。
判定はすでにコード内にある情報で足りる。

- `sequence !== this.loadSequence` の各地点 → `superseded`
- catch に入り、かつ `sequence === this.loadSequence` → `failed` (`AbortError` を含む)
- 適用完了 → `applied`

`recordPageDisplay` のテレメトリは**そのまま維持**する。今回の 3 件を特定した観測の仕組み。

## 4. A で入れないもの

- **lease は入れない。** `PageResourceCache` の `foregroundWaiters` / `prefetchPlanned` /
  `loadForeground` の打ち切り走査は**触らない**。cutover でまとめて置き換える
- **自動再試行は入れない。** 失敗の型が cutover で変わるので方針はその後に決める
- **本体 (Rust) は触らない**
- **URL / history は直さない (決定)。** `commitRequestedPageGroup` は表示前に `pushState`
  するので、巻き戻すと URL だけ失敗ページを指したまま残る。位置を動かす入口は他にもあり
  (`app.js` の jump 経路は requested/displayed の機構自体を通らずに `state.pageGroupIndex` を
  代入してから `pushState` する)、片方だけ直すと別の形の不整合になる。`viewerDepth` を
  触ると戻る操作にも影響する。**入口の統一は cutover の仕事**とし、A では既知の残存
  不整合として扱う。§6 で本書へ記録すること

## 5. テストで固定すること

`node --test --experimental-test-isolation=none crates/remote-web/web/*.test.mjs`

**純関数** (`command-core.test.mjs`)

- `applied` のみ後処理を実行する
- `superseded` は巻き戻しも後処理もしない
- `failed` は巻き戻す。後処理は実行しない
- 未知の outcome を黙って扱わない

**`LatestPageLoadQueue`** (`app-runtime.test.mjs`)

- active 追い越し / pending 追い越し / `clear()` が、`false` ではなく `superseded` を返す
- `run` が例外を投げ、かつ追い越されていない場合の扱いが契約どおり
- 古い `failed` が、新しい要求の位置を巻き戻さない

**viewer**

- single / spread の両方で、成功・fetch 失敗・decode 失敗・現行 `AbortError`・追い越しを検証
- DOM 適用済みのページを、後から来た失敗で旧ページへ戻さない
- `page_display` テレメトリの件数・`outcome`・`reason`・candidate / applied IDs が変わらない

**呼び出し側**

- 終端失敗の後、`state.pageGroupIndex` / シークバー / counter / 表示中のページが一致する
- 失敗メッセージが巻き戻しで消えない
- fit 変更・補正保存・viewport resize・generation 更新の失敗では位置を動かさない
- グループ構成が変わった後に古い完了が届いても、同じ数値 index の別グループを選ばない
- `applied` 以外では AI 通知・読書位置・prefetch などの後処理を走らせない

既存の `viewer.loadGroup` を直接叩くテストが `app-runtime.test.mjs` にある (2147 行付近)。

## 6. ドキュメント

`docs/web-remote-plan.md` §14 に **§14.5** を追加し、次を記録する。

- 段階 A で入った契約 (3 つの outcome と、巻き戻しの条件)
- **URL / history が失敗ページを指したまま残ることを既知の残存不整合として明記**し、
  cutover で入口を統一して解消する旨

`docs/briefs/` は git 管理外なので、決定は必ず本書へ書き戻すこと (§13.4.1)。

## 7. 実行と報告

- `node --test --experimental-test-isolation=none crates/remote-web/web/*.test.mjs` を
  **毎回実行**し、結果を報告すること
- Rust を触らない想定だが、触ったなら `cargo check` も回すこと
- **ビルドとコミットはしない** (ClaudeCode が行う)
- 実装後、変更点・テスト結果・設計から外れた判断があればその理由を報告すること

# 見開き切り替えが表示に届かない不具合と、先読み状態の可視化

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。

## 0. 前提 — 先に読むもの

- [`docs/web-remote-plan.md`](../web-remote-plan.md) §14 / §14.5 / §14.5.1 / §14.6 / §14.6.1
- **所有権の cutover (段階 B+C+D0) とは別の増分。** `PageResourceCache` の
  `foregroundWaiters` / `prefetchPlanned` / `abortUnownedActive`、本体の
  `begin_page_render` と lease には**触らない**

## 1. 不具合: 見開きを切り替えても、次にタップするまで表示が変わらない

利用者の実機報告。設定は書き込まれ、コンテナも読み直されているのに、画面だけが古いまま。

### 1.1 確定している事実 (`remote-web-log.jsonl` から)

- 見開き切り替えのたびに **`/api/container` が 2 回**走っている (16ms 差、266ms 差の実例)。
  どちらも 200 で、`configured_spread` は正しく変わっている
  (`single` 32 groups → `rtl` 16 groups)
- **その後、次のタップまで `page_display` イベントが 1 件も出ていない。**
  このイベントは `applied` / `not_applied` のどちらでも必ず出るので、
  **`updateViewerImage` が `loadGroup` に到達する前に return している**
- 次のタップで初めて `applied dom_committed` が出る (実例: 切り替え 411.7s →
  表示 415.4s、間に page_display なし)

### 1.2 原因

`loadContainer` は先行するコンテナ読み込みを打ち切り、負けた側は false を返すだけ:

    state.requestController?.abort();
    ...
    if (controller.signal.aborted || state.requestController !== controller) {
      return false;
    }

`refreshContainerSpread` は `if (!loaded || ...) return;` で**表示を更新しないまま静かに
終わる**。2 つの更新が互いを打ち切るので、どちらも表示を出さないまま終わり得る。

これは §14 で特定したのと同じ形である。**打ち切る側が「表示を出す義務を誰が負っているか」を
知らない。**

### 1.3 直し方

**個別に guard を足さないこと。** コンテナ更新に**単一の所有者**を与える。

このリポジトリには既に `LatestOnlyTaskQueue` がある (`app.js`)。コンテナ更新を
latest-only の 1 本の経路に通し、**追い越されたものは何もしない / 最後の 1 つは必ず
表示まで到達する**ことを保証する。追い越しと失敗を呼び出し側が区別できるようにする
(段階 A の outcome 契約と同じ考え方。同じ型を使えるなら使ってよい)。

`refreshContainerSpread` の呼び出し元は次のとおり。全部同じ経路に乗せること。

- `requestSpreadMode` (成功時と失敗時の両方)
- viewport resize
- その他 `refreshContainerSpread` を直接呼んでいる箇所を**全部**列挙してから直す

**なぜ 2 回呼ばれるのかも確認すること。** 不要な重複なら止める。必要なら、重複しても
最後の 1 つが必ず表示に届くことをテストで固定する。

## 2. 先読み状態のドットインジケータ (利用者要望)

いま先読みがどこまで効いているかを見る手段が無い。**HUD (`telemetry-hud`) に、現在地の
前後の先読み状態をドットで出す。**

    ●●｜●●●●●●
    緑 = 取得済み / 黄 = 取得中 / 黒 = 未取得

- 区切りの左が**後方** (`PAGE_PREFETCH_BEHIND` 分)、右が**前方** (`PAGE_PREFETCH_AHEAD` 分)。
  読み方向 (RTL / LTR) に関係なく「これから読む側」を右にする
- 状態の出どころは `PageResourceCache`: `ready` にあれば緑、`active` にあれば黄、
  どちらでもなければ黒。**キャッシュの内部構造を UI へ直接晒さない** — 問い合わせ用の
  メソッドを 1 つ生やし、そこだけを UI が使う
- 現在表示中のページも区切りの位置に含めてよい (見開きなら 2 つ)。ただし
  「取得済みかどうか」は同じ規則で色を付ける
- HUD は既に `telemetryDebugDetails` で詳細表示を切り替えている。ドットは**詳細表示の
  ON/OFF に関係なく**出してよいが、既存の行を押し出さないこと
- 更新頻度: 毎フレームは不要。`updateHud()` が呼ばれる契機に加え、先読みの
  開始 / 完了 / 破棄でも更新する。**タイマーで回さない**

**色だけに頼らないこと。** 色覚特性によって緑と黒の区別が付きにくい場合があるので、
`title` / `aria-label` に「取得済み N / 取得中 M / 未取得 K」を出す。

## 3. 予算と窓は今回変えない (測定済み)

標準画質 (長辺 4096) の実測:

| 項目 | 実測 |
|---|---|
| ページサイズ | p50 1.35MB / p95 2.61MB |
| 本体の生成 (`ipc_ms`) | p50 552ms |
| ページ要求 | 222 件 (200: 168 / **503: 52**) |
| 内訳 | prefetch 146 / foreground 22 |
| 同時間帯の `/api/thumb` | 193 件 |

**64 MiB の予算は限界要因ではない** (1.35MB 換算で約 47 ページ分。窓 18 ページ ≒ 24MB)。
効いているのは **503 admission** で、ページ要求の 24% が弾かれている。同じ heavy 枠を
サムネイルが奪っている。

**したがって予算も窓も今回は変えない。** §2 のインジケータで実際の詰まり方が見えてから
決める。推測で数値を動かさないこと。

## 4. 触らないもの

- 取消・lease の所有権 (cutover)、段階 A の outcome 契約、`page_display` テレメトリ
- 要求解像度と JPEG 品質
- 本体側の admission (`try_acquire_prefetch` / `remote_heavy_worker_count`)

## 5. テスト

`node --test --experimental-test-isolation=none crates/remote-web/web/*.test.mjs`

**コンテナ更新**

- 更新が 2 つ重なっても、**最後の 1 つが必ず表示まで到達する**
- 追い越された更新は表示を書き換えない
- 失敗した更新と追い越された更新を呼び出し側が区別できる
- 見開き切り替え → 表示が新しいグループ構成になる (単ページ ⇄ 見開き)

**インジケータ**

- 取得済み / 取得中 / 未取得がそれぞれ正しい状態になる
- 後方と前方の並び (読み方向に関係なく「これから読む側」が右)
- 補助テキストに件数が出る
- 先読みの開始・完了・破棄で更新される

## 6. ドキュメント

`docs/web-remote-plan.md` に節を追加する。

- コンテナ更新の所有者を 1 本にした理由 (2 つの更新が互いを打ち切って、どちらも表示を
  出さないまま終わり得た)
- インジケータの色と状態の対応
- 予算と窓を据え置いた理由と、その根拠になった実測値

## 7. 実行と報告

- 上記 node テストと `cargo test -p mimageviewer-remote` を**毎回実行**して結果を報告する
- **ビルドとコミットはしない**
- 設計から外れた判断があれば理由を報告する

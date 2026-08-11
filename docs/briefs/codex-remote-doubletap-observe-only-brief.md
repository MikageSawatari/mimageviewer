# 二度打ち抑止をやめ、対の認識と観測だけにする

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。

## 0. 前提

- [`docs/web-remote-plan.md`](../web-remote-plan.md) §14.8 / §14.8.1
- **所有権の cutover とは別。** `PageResourceCache` と本体の lease / cancel は触らない
- module scope の const は起動ブロックより前で初期化する (TDZ で起動不能にした事故あり)

## 1. なぜ変えるか (実測)

`document-double-tap.mjs` は認識した対の 2 打目で `event.preventDefault()` していた。
これを**やめる**。理由は 2 つとも実測で確定している。

**(a) 拡大を止められていない。**
2 打目を `preventDefault()` した **46ms 後**に `visual_viewport_scale` が 1 → 1.03 へ
上がった記録がある (`pair_suppressed` / `suppressed: true` の直後)。
`* { touch-action: manipulation }` も全体に効いている状態での結果。

**(b) タップを落としている。**
直近の実機ログで、タップ判定 65 件に対しコマンド 59 件。差の多くは `travel_exceeded`
(ドラッグ) で正常だが、**`pair_suppressed` なのに `command` が出ていないタップ**が
混じっている。2 打目の既定を止めると合成 click が落ち、ページ送りが消える。利用者からは
「連打するとシークバーが動かないことがある」として報告された。

    1786415550394 CMD prev_page          ← タップ成立
    1786415550395 DT  pair_suppressed
    1786415550546 DT  pair_suppressed    ← CMD が無い。このタップは消えた
    1786415550846 CMD prev_page

**(c) 拡大は viewport で止めた。**
`58e3562e` で `maximum-scale=1, user-scalable=no` を入れた。ホーム画面へ追加した
standalone では iOS がこれを尊重する (利用者の使用形態)。

つまり抑止は**利点が無く、コストだけが残っている**。

## 2. やること

### 2.1 モジュールを観測専用にする

- 2 打目の `preventDefault()` を**やめる**
- `DEFAULT_TAP_EXCLUSIONS` は**役目が無くなるので撤去する。** 「この対象は既定を残す」
  という表であって、誰の既定も止めない今は意味が無い。残すと次に読む人が
  「除外しなければ拡大が止まる」と誤解する
- **対の認識と `onDecision` の通知は残す。** ズームの再発を観測する唯一の手段であり、
  今回の判断もこの記録で取れた
- 判定の語彙は observation として意味の通るものへ整理してよい (例: `pair_recognized`)。
  変えるなら telemetry の許可集合と `docs` の記述も揃えること

### 2.2 テストを新しい契約へ書き換える

現在 4 件が旧契約を固定していて落ちる。**削除ではなく、いまの意図を固定する形に
書き換えること。**

- `document owner preserves the first touchend and prevents only the matching second tap`
- `activatable targets keep both taps, because preventing one drops its click`
- `the document owner uses the browser suppression window`
- `pickers and labels are suppressed, because the browser does zoom on them`

新しい契約で固定してほしいこと:

- **どのタップでも既定を止めない** (`preventDefault` が呼ばれない)
- 対の認識 (時間窓・移動量) は今までどおり
- 多点タッチは対に入らない
- 対象の種類 (button / link / input / 素の要素) で**挙動が変わらない**
- 通知される決定に、対を認識したかどうかが残る

`pwa.test.mjs` の viewport 指定テスト (`maximum-scale=1` / `user-scalable=no`) は
現状維持。

### 2.3 ドキュメント

`docs/web-remote-plan.md` §14.8.1 を更新し、次を残す。

- 抑止をやめた理由 (上の (a) (b) (c)、数値込み)
- 通常タブ (standalone でない) では viewport が無視されるため**拡大は起こり得る**こと。
  JS では止められないことが分かっているので、そこは受け入れる判断であること
- 観測は残すこと

`docs/architecture-overview.md` に double-tap owner の記述があれば揃える。

## 3. テストと報告

- `node --test --experimental-test-isolation=none crates/remote-web/web/*.test.mjs` と
  `cargo test -p mimageviewer-remote` を**毎回実行**して結果を報告する
- **ビルドとコミットはしない**
- 設計から外れた判断があれば理由を報告する

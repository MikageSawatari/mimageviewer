# ボタン上のダブルタップ拡大と、先読みの admission 圧

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。

## 0. 前提

- [`docs/web-remote-plan.md`](../web-remote-plan.md) §14 系
- **所有権の cutover とは別の増分。** `PageResourceCache` の `foregroundWaiters` /
  `prefetchPlanned` / `abortUnownedActive`、本体の `begin_page_render` と lease は触らない
- module scope の singleton は起動ブロックより前で初期化する (TDZ で起動不能にした事故あり)

## 1. ボタンを連打すると UI 全体が拡大する

### 1.1 確定している事実

利用者の iPad で、動作確認のためボタンを連打していたら UI 全体が拡大し、ボタンが
見切れた。telemetry で裏が取れている。

- `visual_viewport_scale` が **1.03 に上がったまま戻らない** (`image` /
  `viewer_layout` / `viewer_gesture` / `visual_viewport` イベント多数)
- 同じ時間帯に `browser_double_tap` の判定が
  **`decision: "excluded_target"` / `exclusion_reason: "button"` / `suppressed: false` が 16 件**

つまり、ボタン上のダブルタップが抑止されずブラウザの拡大が走っている。

### 1.2 なぜ除外されているか

`document-double-tap.mjs` の `DEFAULT_TAP_EXCLUSIONS` にコメントで理由が書いてある。
**2 打目の `touchend` を止めると合成 click も落ちる**ので、click で起動する部品
(ページ送りの ‹ › など) は素早く 2 回押したときに 2 回目が効かなくなる。

正しい判断だが、**ページ送りボタンの連打はこのアプリの主要操作**なので、ここが
抜けていると拡大が日常的に起きる。

### 1.3 直し方

**「止めない」か「壊す」かの二択にしない。** 2 打目を止めたうえで、**起動を自分で
送り直す**。

- `button` / `[role="button"]` は既定を止め、対象へ `click` を合成して dispatch する
  (`bubbles` / `cancelable` / `composed` を立てる)。これで拡大は起きず、2 打目も効く
- 二重起動を作らないこと。既定を止めた以上ブラウザは click を出さないが、
  **出た場合に 2 回動かない**ことをテストで固定する
- `a[href]` / テキスト入力 / `contenteditable` は現状のまま除外を維持する。
  ナビゲーションやキャレットの既定はこちらで再現すべきものではない
- 判定と合成は double-tap owner の中に閉じる。呼び出し側へ条件を散らさない

`* { touch-action: manipulation }` は既に全体へ効いているが、それでも拡大している
という実測がある。**CSS だけで足りるという前提を置かないこと。**

### 1.4 テスト

- ボタン上の 2 打目で既定が止まり、`click` が 1 回だけ届く
- リンク / テキスト入力 / `contenteditable` では既定が維持される (現状維持)
- 1 打目、多点タッチ、移動量超過は今までどおり素通りする
- 拡大抑止の判定結果が telemetry に残る (`suppressed` が true になる)

## 2. 先読みが admission に弾かれ続ける

### 2.1 実測 (直近 10 分、標準画質)

| 項目 | 実測 |
|---|---|
| `/api/page` | 138 件 (200: 108 / **503: 30 = 22%**) |
| 先読み telemetry | `ready` 70 / `admission_busy` 21 (すべて再試行予定) |
| 終端失敗 | **0 件** (直前の修正で解消) |
| 本体の生成 | p50 約 0.55s |
| 同時間帯の `/api/thumb` | 多数 |

再試行は効いているが、**5 ページ先が黒のままその先が緑**という状態が実機で観測される。
再試行の待ちに入っている 1 枚だけが埋まらない。

### 2.2 見てほしいこと

**まず現状の admission を確認すること。**

- remote-web 側: `ipc_prefetch_limit: 2`、`ipc_heavy_limit: 4`
- 本体側: `remote_heavy_worker_count = (設定値 / 2).clamp(1, 3)`、
  `remote_page_prefetch_limit = min(heavy - 1, 2)`、さらに
  `try_acquire_prefetch` は **`queued > 0` なら無条件で拒否**する

本体側の上限 3 本には「IPC decode がローカル表示用 worker と CPU / disk を奪い合わない
ため」という根拠がコメントに書かれている。**しかしリモートが操作権を持つ間、本体は
「切断」しか操作できない** (§設計制約)。ローカル表示は動いていないので、この根拠は
リモートセッション保持中には当てはまらない。

そのうえで、次を評価して提案してほしい。**実装は提案に合意してから。**

- リモートセッション保持中に限り、heavy worker と先読み同時実行数を上げてよいか。
  上げるなら安全な値と、セッション終了時に戻す方法
- `queued > 0` の無条件拒否は妥当か。サムネイルが queue に居るだけで先読みが
  全部弾かれる。前景を守るのが目的なら、前景専用の枠と queue の中身を区別する方が
  素直ではないか
- 再試行の待ち時間 (`scheduleRetry` は 100〜10000ms、既定 1000ms) が長すぎないか。
  待っている間 `pump()` は何も始めない

**推測で数値を動かさないこと。** 変えるなら、変えた後に何を測れば効果を確認できるかも
書く。

## 3. 触らないもの

- 取消・lease の所有権 (cutover)、段階 A の outcome 契約、`page_display` テレメトリ
- 要求解像度と JPEG 品質
- 予算 64 MiB と窓 12/4 (前回据え置いた判断のまま)

## 4. 実行と報告

- `node --test --experimental-test-isolation=none crates/remote-web/web/*.test.mjs` と
  `cargo test -p mimageviewer-remote` を**毎回実行**して結果を報告する
- **§1 は実装する。§2 は評価と提案を先に報告し、実装は合意してから**
- **ビルドとコミットはしない**

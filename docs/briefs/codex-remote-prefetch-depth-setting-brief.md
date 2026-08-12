# 先読みの上限を「見えないバイト予算」から「端末ごとの枚数設定」へ

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。master 取り込み (`151e37a1`) の後。

## 0. 前提 — 先に読むもの

- [`docs/web-remote-plan.md`](../web-remote-plan.md) **§14.6** (先読みのバイト予算化)、
  **§14.6.1**、**§14.9** の契約 10 (`protectedKeyIds` と admission)
- `crates/remote-web/web/app.js` — `PAGE_PREFETCH_AHEAD` / `PAGE_PREFETCH_BEHIND` /
  `PAGE_RESOURCE_CACHE_LIMIT` / `PAGE_RESOURCE_CACHE_MAX_BYTES` (150-160 行付近)、
  `PageResourceCache` の `prefetchAdmits` / `remember` / `trimUnprotected` / `deleteReady`、
  `schedulePagePrefetch`、`端末の設定` パネル (9940 行付近)
- `crates/remote-web/web/command-core.mjs` — `pagePrefetchPlan` /
  `pageResourceAdmissionPlan` / `pagePrefetchBudgetAllowsStart` / `pageResourceCacheBudget`
- `crates/remote-web/web/local-settings.mjs` — 端末ごとの設定 (`imageQuality` などの前例)

## 1. 実機ログで確認した事実 (再調査不要)

2026-08-12 の実機セッション (`target/dev-runtime/data-remote/remote-web-log.jsonl`) を解析した。

| 事実 | 値 |
|---|---|
| `/api/page` 成功 | 154 回 / 実質 72 レンディション |
| **2 回以上取得したもの** | **45 レンディション (最大 5 回)** |
| 総転送 306.3 MiB のうち**再取得** | **163.7 MiB (53%)** |
| ズーム時のページ 1 枚 | p50 **6.0 MiB**、最大 9.6 MiB (`target_px: 8192`) |
| 64 MiB に入る枚数 | **約 11 枚** |
| 計画の窓 | **前 12 + 後ろ 4 = 16 枚** |

**計画が予算に構造的に入らない。** 外側 5 枚は保持できず、読者が近づくと取り直す。取得順にも
`P58..P65 を先読み → P58* P59* → P61* P60* P63* P64* P65*` (`*` = foreground) が出ており、
先読み済みが表示到達前に落ちて取り直されている。

`PAGE_RESOURCE_CACHE_MAX_BYTES` の 64 MiB は**測って決めた値ではない**。コード中のコメントにも
「Safari では端末メモリを取得できないため固定」と書いてある。

## 2. 決定 (2026-08-12、利用者判断)

**上限は枚数で表す。見えないバイトの門は撤去する。**

- 見えない上限で先読みが止まると、利用者には「なぜ先読みされないのか」が分からない。
  実行時の状態で挙動を変えず、**設定だけで挙動が決まる決定性**を優先する
  (このプロジェクトで既に確立している方針)
- 設定を上げすぎてブラウザのタブが落ちることは**許容する**。復帰は「設定を下げる」で足りる
- そのため設定は**端末ごとに保存し、リモート画面から変更できる**必要がある
  (外出先で落ちたとき、PC に戻らないと直せない形にしない)
- 既定は**後ろ 4 / 前 8**。空白ページや扉を素早く読み飛ばす動きがあるので前は最低 4 は要る
- **限界**: この設定が守るのは先読みの storm であって、**1 ページ単体が大きすぎる場合は守れない**
  (今回のページはデコード後 約 186 MB)。その逃げ道は画質設定である。plan に明記すること

## 3. 変更内容

### 3.1 端末ごとの設定を追加する

`local-settings.mjs` に先読み枚数を足す。

- `prefetchAhead` (既定 8) と `prefetchBehind` (既定 4)
- 正規化は既存の `normalizeGridColumns` と同じ形。範囲は前 2〜32 / 後ろ 0〜16 とする。
  これは**入力の妥当性のための範囲であって安全上限ではない**。コメントでそう書くこと
- **`LOCAL_SETTINGS_VERSION` は上げない。** 欠けている項目は既定で埋まるので、
  既存の保存値を捨てる必要が無い

UI は既存の**「端末の設定」パネル**へ、画質の隣に置く。数値入力かステッパーでよい。
説明文には次を含める。

- この端末だけに保存すること
- 大きくすると素早くめくっても待たされにくくなること
- 大きすぎると**ブラウザが落ちることがあり、その場合はこの値を下げて開き直す**こと

### 3.2 計画の枚数を設定から取る

`schedulePagePrefetch` が `pagePrefetchPlan` に渡す前後の枚数を、定数ではなく
**有効な窓** (§3.3) から取る。`PAGE_PREFETCH_AHEAD` / `PAGE_PREFETCH_BEHIND` の定数は
既定値の置き場としてだけ残すか、`local-settings.mjs` の既定へ集約する。

### 3.3 起動直後は窓を絞る

タブが落ちた後の再読み込みは**同じページを開き直す** (読書位置の再開と URL hash の両方)。
そのまま同じ先読みが走ると落ち続け、設定を変える余地が無い。

- **本を開いた直後の有効な窓は `min(設定値, 4)`** とする。設定値が 4 未満ならそのまま
  (絞る方向にだけ効かせ、広げない)
- **その本で最初にページを移動した時点で設定値へ戻す**
- 別の本を開いたら再び絞る

判定は**純関数**にする (`{ configuredAhead, configuredBehind, movedSinceOpen }` →
`{ ahead, behind }`)。時刻や空きメモリなど実行時状態を見ない。

### 3.4 バイトの門を撤去する

- `prefetchAdmits` と `trimUnprotected` から `pagePrefetchBudgetAllowsStart` の判定を外す。
  `pageResourceCacheBudget` / `PAGE_RESOURCE_CACHE_MAX_BYTES` も撤去する
- **保持の上限は枚数だけ**にする。`PAGE_RESOURCE_CACHE_LIMIT` は
  「表示中 + 有効な窓 (前 + 後ろ)」から導出する。窓が変われば上限も変わる
- §14.9 契約 10 の**候補までを保護する admission 規則は維持する** (近い候補のために
  遠い取得済みを交換する順序は変えない)。変えるのは「バイトで開始を止める」ことだけ
- **バイト数の会計は残す**。撤去するのは判断に使うことであって、記録ではない

### 3.5 破棄を記録に出す

いま `deleteReady` は HUD を更新するだけで、ログに残らない。次の計測で
「再取得の原因が破棄なのかキー変更なのか」を切り分けられるようにする。

- 破棄のたびに telemetry を出す。含めるもの: key、理由 (窓の外 / 上限超過 / clear)、
  そのときの保持枚数と保持バイト
- **動作は変えない**。観測だけを足す

## 4. 触らないもの

- 段階 3a / 3b / 3c の coordinator / registry / heavy queue / lease / 位置状態機械
- **remote-web の `IpcAdmission`** (今回のログに出た 39 件の 503 `admission_busy`)。
  別の増分で扱う
- 画質設定と `target_px` の決め方 (ズーム時に 8192 まで上がる件も別途)
- 本体 (`src/`) と protocol

## 5. テスト

```
cd crates/remote-web/web && node --test
```

- 有効な窓の純関数: 既定 (4/8)、設定値が 4 未満のとき広げない、移動後に設定値へ戻る、
  別の本で再び絞る
- 設定の正規化: 範囲外・非数値・欠落が既定へ落ちる。`LOCAL_SETTINGS_VERSION` を上げずに
  既存の保存値が保たれる
- 計画の枚数が設定に従う (前後それぞれ)
- **保持の上限が枚数だけで決まる**こと。バイト数では開始も破棄も起きないこと
  (`pagePrefetchBudgetAllowsStart` が admission 経路から消えていること)
- 候補までを保護する交換順序が従来どおりであること (§14.9 契約 10 の既存テストを維持)
- 破棄で telemetry が出ること

## 6. ドキュメント

- plan **§14.6** を書き換える。バイト予算を撤去し枚数にした理由、実機ログの数字 (53% の
  再取得、6.0 MiB/ページ、11 枚しか入らない)、決定性を優先する判断、
  タブが落ちる可能性を許容すること、**1 ページが大きすぎる場合は守れないこと**
- plan に **§14.17** (または次の空き番号) を追加し、起動直後に窓を絞る理由
  (再読み込みで同じページに戻るためクラッシュループになり得る) と、解除条件を記録する
- `htdocs/mimageviewer/manual/tut-remote.html` に 1 段落足す。先読み枚数が端末ごとの設定で
  あること、素早くめくるなら増やすこと、**落ちるなら下げること**、
  それでも落ちるなら画質を下げること

## 7. 実行と報告

- §5 のコマンドを**毎回実行**して結果を報告する
- **`crates/` と `src/` に触れた箇所を全部、理由付きで報告する**
- **`scripts/build-dev.ps1` を実行しない。コミットもしない**
- ブリーフと意図的に違えた点があれば、その理由を報告する

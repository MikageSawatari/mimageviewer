# リモート閲覧: ダブルタップの意味をアプリが 1 か所で所有する

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。
実装 = Codex、レビュー・テスト・統合 = ClaudeCode、実機確認 = 利用者。

## 1. 観測された失敗

- 一覧の並べ替えバーをダブルタップすると画面全体が拡大する。**`.grid-sort-reason`
  (「本として表示中は名前順固定です」のただのテキスト) の上でも起きる**。
- ビューアでも以前から「ページ送りのつもりでダブルタップすると少しだけ拡大して、
  ボタンが見切れる」という報告がある (未解決)。

## 2. これまでに分かっていること (再調査不要)

- `index.html` の viewport は `width=device-width, initial-scale=1, viewport-fit=cover`。
  **pinch zoom は意図して残している** (`styles.css` の `#app` に
  「Keep browser pinch zoom, but never assign double-tap zoom to the app shell.」)。
- `#app` に `touch-action: manipulation` がある。
- それでも足りないため、この repo の tap 対象は**自分自身にも** `touch-action` を
  宣言している (button / .grid-tile / .viewer-button / .adjustment-tab など 28 箇所)。
- 直近で `select` と `.grid-sort-bar` にも宣言したが、**その子のテキストの上では
  まだ拡大する**。要素ごとに足していく方針では、非対話のテキストや余白まで
  網羅できず終わらない。
- 焦点由来の拡大 (16px 未満のフォーム部品に focus すると iOS が拡大する) は別件で、
  `select / input / textarea` に `font: inherit` を入れて解消済み。**今回のダブルタップは
  それとは別の原因**。

## 3. 構造の決め

**アプリはダブルタップに自分の意味を割り当てている** (ビューアの原寸切替)。ブラウザの
double-tap zoom と取り合っているのが本質で、要素ごとの `touch-action` はその取り合いを
場当たりに宣言し直しているだけになっている。

**ダブルタップの扱いを 1 か所で所有する。**

- 文書レベルで double-tap を検出し、**ブラウザ既定の拡大だけを止める**。
- 判定は純関数に切り出す (2 回のタップの時間差と距離から「同一の double-tap か」を返す)。
  閾値をコード中に散らさない。
- ビューアの既存のダブルタップ処理は**そのまま動く**こと。所有者を 1 つにするのであって、
  機能を移すのではない。

## 4. 守ること

- **pinch zoom を殺さない。** `user-scalable=no` / `maximum-scale` を viewport に足さない。
  意図して残しているものを、別の問題の回避のために消さない。
- **タップが効かなくならないこと。** `touchend` の `preventDefault` は合成 click を
  消し得る。1 回目のタップや、double-tap 以外の `touchend` を止めないこと。
- **文字入力の選択・キャレット操作を壊さない。** テキスト入力・textarea の上では
  既定の挙動を残す。
- **要素ごとの `touch-action` 追加で埋めない。** 既存の 28 箇所は残してよいが、
  今回の対策として新しい要素へ足していかないこと。
- 既存の `#app` の意図 (pinch は残す / double-tap zoom は割り当てない) を、コメントごと
  現状に合った記述へ更新する。

## 5. テスト

- 判定の純関数に単体テストを付ける。最低限:
  - 時間内・近い位置の 2 タップ → double-tap
  - 時間が離れている / 位置が離れている → 別のタップ
  - 3 回目以降の扱いが決まっていること
- 1 回目の `touchend` を止めていないこと (合成 click が消えないこと) を検証する。
- 既存の web テスト 224 件を維持すること。

## 6. 確認

- web テスト一式が通ること。`git diff --check` が通ること。
- **ビルドとコミットは行わない。** 変更ファイルと追加テストの一覧を報告する。
- 既存の未追跡 brief (`docs/briefs/*.md`) には触れない。

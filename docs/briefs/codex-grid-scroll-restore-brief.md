# スマホ側: フォルダを戻ったときにスクロール位置が失われる

worktree: `C:\home\mimageviewer-web` / branch `web-remote` / 起点 `c2099561`

## 0. 立場

**本体が正本。独自の規則を発明しない。**
以下は私が読んで確認した内容だが、**実際と違っていたら実際の方を報告してほしい。**
私の要約に合わせて実装しないこと。この段取りで既に私の誤りが 18 回訂正されている
(直近では、この不具合を本体側だと読み違えて調査を頼み、途中で止めた。今回はスマホ側)。

稼働中の本体 / remote-web は操作しない。`build-dev.ps1`・コミットも実行しない。

## 1. 症状 (利用者報告、実機)

**スマホの一覧**でフォルダを開き、そこから親フォルダへ戻ると、
**一覧のスクロール位置が先頭に戻る。**

本体側ではない。remote-web の話。

## 2. 原因 (特定済み。ここは再確認しなくてよい)

スクロール復元の仕組みは **画像・動画を開いて戻る場合にしか無い**。

- `rememberGridViewerReturn(entry)`
  ([app.js:1515](../../crates/remote-web/web/app.js)) が
  `state.gridViewerReturn = { sourceContext, viewedItemIdentity, previousScrollTop }`
  を 1 枠だけ記録する
- 呼ばれるのは **image / media を開く 3 箇所だけ**
  ([app.js:1217](../../crates/remote-web/web/app.js) /
  [1239](../../crates/remote-web/web/app.js) /
  [1272](../../crates/remote-web/web/app.js))
- **フォルダを開く経路は呼んでいない**
  ([app.js:1193](../../crates/remote-web/web/app.js) の
  `payload.kind === "favorite" || payload.kind === "folder"` は `navigate()` するだけ)
- 親へ戻る `PARENT_FOLDER`
  ([app.js:1066](../../crates/remote-web/web/app.js)) も `navigate()` するだけ
- 復元は `resolveGridReturnViewport` → `restoreScrollTop`
  ([app.js:2565](../../crates/remote-web/web/app.js))。`gridViewerReturn` が
  無ければ何も復元しない

→ **一覧 → 子フォルダ → 親へ戻る、の往復には保存も復元も存在しない。**

## 3. 直し方 — フォルダ分岐に 1 行足す形にしない

`payload.kind === "folder"` の枝へ `rememberGridViewerReturn` を足すのは
**症状パッチ**になる。同じ穴が他にもあるため:

- ZIP / PDF を開く (`payload.kind === "container"`、
  [app.js:1198](../../crates/remote-web/web/app.js))
- お気に入り / ホームから入る
- 集約一覧 (ブックマーク / 履歴 / レーティング / タグ) から実フォルダへ降りる
- ブラウザの戻る / 進む、アプリ内の ←

**記録は「一覧を離れるとき」に一度だけ行う形にしてほしい。** 開く対象の種類ごとに
呼び出しを増やさない。

### 単一枠から、一覧ごとの記憶へ

現在の `gridViewerReturn` は **1 枠**で、使ったら null にする
([app.js:2471](../../crates/remote-web/web/app.js))。ビューアからすぐ戻る用途には
足りるが、フォルダは**何段も潜って戻ってくる**ので枠が足りない。

本体は `folder_history: HashMap<PathBuf, (scroll, selected)>`
([app.rs:9037](../../src/app.rs)) で**フォルダごと**に持っている。スマホも
`gridHash` を鍵にした同じ形にできるはず。

そのうえで、ビューアからの復帰も同じ仕組みに載せてほしい。**2 つの復元経路を
並存させない** (この企画で繰り返し言われている「2 箇所にわかれていると動作の違いが
おこる」がそのまま当てはまる)。

ビューア経路だけが持つ「見ていたページに合わせる」も必要で、
`updateGridViewerReturnItem` ([app.js:1552](../../crates/remote-web/web/app.js)) が
ページ送りのたびに identity を更新している。統合先でもこれを失わないこと。

**この設計が正しいか、より良い形があるかを先に報告してほしい。** 実装の前に一度見たい。

## 4. 本体との対応

本体は親へ戻るとき、スクロール位置の復元に加えて**抜けてきた子フォルダを選択**する
(`select_after_load` → [app.rs:14700](../../src/app.rs))。

スマホ側でも、戻った先で**抜けてきたフォルダの位置**が分かる方が本体と揃う。
`resolveGridReturnViewport` は既に `targetIndex` を返す形になっているので、
そこへ載せられるかを見てほしい。載せられないなら理由と一緒に報告を。

## 5. 上限

一覧ごとに持つなら**際限なく増える**。上限 (LRU) を付けること。
何件が妥当かは判断して、根拠と一緒に報告してほしい。

## 6. ついでに気づいた点 (この不具合とは別)

同じ関数内に 2 つある。**直すかどうかは判断に任せる**が、放置するなら理由を聞きたい。

1. [app.js:1268](../../crates/remote-web/web/app.js) — `return false;` の**次の行**に
   `meta.openRoute = "legacy_image_rejected";` があり、到達しない。
   この却下経路が telemetry に出ていないはず
2. [app.js:1283](../../crates/remote-web/web/app.js) — `meta.openRoute = "folder_image"`
   のインデントが崩れていて `if (Number.isInteger(payload.entryIndex))` の中に入っている。
   `entryIndex` が無いときに route が付かない

## 7. 受け入れ条件

- 一覧でスクロール → フォルダを開く → 親へ戻る、でスクロール位置が戻る
- 何段か潜って 1 段ずつ戻っても、各段の位置が戻る
- ZIP / PDF、お気に入り、集約一覧から降りて戻る場合も同じ
- 画像・動画を見て戻る既存動作が退行していない (見ていたページに合う)
- 端末の回転・画面消灯からの復帰で壊れない
- 記憶が際限なく増えない
- **往復の回帰テスト**を付ける (現状これが無いので再発を検知できない)
- web テストが緑。Rust 側に触るなら該当テストも

## 8. 注意

- ビルド (build-dev.ps1 / build-release.ps1) とコミットはしない。テストは走らせてよい
- 原因が分かったが正しい修正が広範囲になる場合は、直さずに報告してほしい

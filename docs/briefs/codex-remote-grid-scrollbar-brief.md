# 実機確認で出た 3 件: 一覧のスクロールバー / メニューの見切れ / 全端末ログアウトの確認

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。**`C:\home\mimageviewer` ではない。**

上限 10 万件は `238e1a7e` で入り、**実機 (iPad) で 6 万件のサブ展開が問題なく開けて、
スクロールも滑らか**であることを確認済み。そのうえで出た指摘 3 件。

- **1 件 = 1 コミット** (3 コミット)。
- `docs/briefs/HANDOFF.md` と他の未追跡 brief は触らない。
- **commit は行わなくてよい** (worktree の `.git` は親リポジトリ側にあり sandbox から書けない)。
  変更を残したまま報告すればこちらでコミットする。
- `cargo fmt --all` を通し、末尾のテストを走らせる。

---

## 1. 一覧にドラッグできるスクロールバーを付ける (最重要)

### 何が問題か

6 万件の一覧を開けるようになったが、**位置を大きく飛ばす手段が無い**。
iOS の native スクロールインジケータは触れないうえ、スクロール中しか出ない。
利用者の言葉: 「スクロールも滑らかですが、スクロールバーがないので不便です」。

一覧の scroller は `.grid-scroll` (`crates/remote-web/web/styles.css:781`) で、
`overflow: auto` の素の scroller。`VirtualGrid` が `scroll` イベントを購読し
(`app.js:7087`)、`scrollTop` を読み書きする (`:7389`, `:7393`)。
**この scroller の `scrollTop` を書けばよい**ので、統合点は明確。

### 仕様

- `.grid-scroll` の右端に重ねる**独自のスクロールバー**。track + thumb。
- **一覧がスクロール可能なあいだは常に表示する**。時間で消さない。
  「スクロール中だけ出る」は今回の不便さの原因そのもの。
  ドラッグ中は太く / 明るくして掴んでいることを示す。
- **thumb をドラッグすると位置が飛ぶ**。track の空き部分をタップしたら
  そのページ位置へ飛ぶ (1 画面送りではなく直接ジャンプでよい)。
- thumb の最小高さを決める (目安 44px)。6 万件でも掴める大きさを保つ。
- **ドラッグ中は現在位置のバブルを出す**。「12,345 / 60,000」のように**件数**で出す
  (行番号ではなく項目番号)。6 万件では位置の手掛かりが無いと飛ばせない。
- 縦横どちらの向きでも出す。`env(safe-area-inset-*)` を尊重する。

### 触らないこと / 壊さないこと

- **既存のタッチ操作を奪わない**。`.grid-scroll` は `touch-action: pan-y` で、
  一覧自体のドラッグスクロールがある。thumb には `touch-action: none` を付け、
  pointer capture で掴む。track / thumb 以外の場所は今までどおり素通りさせる。
- `VirtualGrid` の `scrollend` での行スナップ (`snapToRow`) はそのまま活かす。
  ドラッグ中は `scrollTop` を直接書き、スナップは離した後の既存経路に任せる。
- 一覧を離れて戻ったときの復元 (`restoreScrollTop`) と競合しないこと。
- 画像ビューアには付けない。**一覧だけ**。

### 実装のしかた

**幾何は純関数に切り出し、`node --test` で直接テストできる形にすること。**
DOM を触るテストは書きにくいので、次のような純関数を
`command-core.mjs` (または新しい小さい module) に置く:

- `gridScrollbarThumb({ contentHeight, viewportHeight, scrollTop, trackHeight, minThumbHeight })`
  -> `{ visible, thumbHeight, thumbTop }`
- `gridScrollbarScrollTop({ pointerTop, grabOffset, contentHeight, viewportHeight, trackHeight, minThumbHeight })`
  -> `scrollTop` (0..maxScroll にクランプ)
- 表示位置のバブル用に、`scrollTop` から「先頭に見えている項目の 1 始まりの番号」を出す純関数。
  列数と行高から求める。端数の行でも 1..total の範囲を外れないこと。

DOM 側はこの純関数を呼んで `style` を書くだけにする。

### テスト

`node --test` で:

- 内容が viewport 以下なら `visible === false`。
- `scrollTop = 0` で `thumbTop === 0`、最大スクロールで `thumbTop === trackHeight - thumbHeight`
  (浮動小数の誤差を許容する比較で)。
- 6 万件相当の巨大な `contentHeight` でも `thumbHeight >= minThumbHeight`。
- `gridScrollbarThumb` と `gridScrollbarScrollTop` が往復すること
  (thumbTop から scrollTop を出し、それでまた thumbTop を出すと元に戻る)。
- 位置バブルの番号が 1 以上 total 以下に収まること (先頭 / 末尾 / 端数行)。

---

## 2. メニューが見切れる (2 列にする)

### 何が問題か

利用者の言葉:「ログアウトが見切れて、再読み込みが画面外です。2 列表示にするのがよさそうです。
スペース的にはできそうです」。

`.command-menu-actions` は既定で **2 列** (`styles.css:1699-1704`) だが、
**landscape だけ 1 列に固定**されている (`styles.css:2524-2526`)。
`RR-05` で「ログアウト」を足して項目が 1 つ増えた結果、1 列だと縦に溢れる。

### 実装

- landscape の `grid-template-columns: 1fr` 固定をやめ、
  **幅に応じて自動で列数が決まる**形にする (`repeat(auto-fit, minmax(<閾値>, 1fr))` 等)。
  panel はランドスケープで画面幅の 40% (`styles.css:1791`) あるので、
  iPad はもちろん iPhone のランドスケープでも 2 列が入るはず。閾値は実測で決める。
  非常に狭いときは自然に 1 列へ落ちること。
- **溢れたときに必ずスクロールできること**を両方のメニューで確認する。
  - viewer panel は `viewer-command-menu-body` が `overflow: auto` (`styles.css:1831`)。
  - 一覧側のメニューは `.command-menu` 自体が `overflow: auto` + `max-height`。
  - どちらも、項目が増えても画面外に出て**触れなくなる**ことが無いようにする。
- `VIEWER_MENU_MAX_ACTIONS` (`app.js:7533`) は 12 のままでよい。
  ただし**この定数が何を守っているのか 1 行コメントを付ける**こと
  (レイアウト上限ではなく、メニューを無制限に育てないための規律であるなら、そう書く)。

### テスト

- 既存の `every iPhone viewer menu page stays within the fixed action limit` を維持。
- CSS の変更は自動テストで捕まえにくいので、**何を目視すべきか**を報告に書くこと
  (どの向き・どの画面幅で 2 列になるか)。

---

## 3. 「すべての端末をログアウト」の確認を modal にする

### 何が問題か

利用者の言葉:「すべての端末をログアウト、という同じボタンを 2 回押すのがわかりづらいです。
さらにモーダルダイアログを開くようにはできますか？」

現状 (`src/remote_ipc/ui.rs`) は `RemoteSessionLogoutState::Idle` のボタンと
`Confirming` のボタンが**同じラベル**で、接続ダイアログ内にそのまま出る。

### 実装

`egui::Modal` を使う。**この repo に既に前例がある**ので同じ作法に揃える:

- `src/app/subfolder_expansion.rs:2244` (`subfolder_expansion_confirm`)
- `src/app/smart_folder.rs:4747` (`smart_folder_large_confirm`)

`RemoteSessionLogoutState::Confirming` のときに接続ダイアログ内へ警告文を出すのをやめ、
`egui::Modal::new(egui::Id::new("remote_logout_all_confirm"))` で確認ダイアログを出す。

- 見出し: 「すべての端末をログアウト」
- 本文: この PC に接続中の端末を含め、すべての端末で PIN の再入力が必要になること。
  **PIN 自体は変わらない**こと。
- ボタン: 実行側は「ログアウトする」、取り消し側は「キャンセル」。
  **起点のボタンと同じ文字にしない** — 2 回同じ物を押させるのが今回の指摘。
- `Running` / `Finished` の表示は今の場所 (接続ダイアログ内) のままでよい。
- 文言は CLAUDE.md の「UI 文字列の Unicode グリフ選定ルール」に従う。

### テスト

- 既存の `unsupported_tailscale_serve_path_warns_without_disabling_root_setup` 等の
  純関数テストを壊さないこと。
- 状態遷移 (`Idle -> Confirming -> Running -> Finished`、`Confirming -> Idle`) を
  純関数または状態機械のテストで固定できるなら足す。egui の描画自体はテストしない。

---

## 実行するテスト

```
node --test
cargo test -p mimageviewer --lib remote_ipc
cargo test -p mimageviewer-remote
cargo fmt --all -- --check
python scripts/check_ui_glyphs.py
```

`node --test` が sandbox の `spawn EPERM` で動かない場合は
`node --experimental-test-isolation=none --test` でよい (その旨を報告に書くこと)。

## 報告してほしいこと

- 3 つの変更それぞれで何をしたか (コミットはこちらで行う)。
- スクロールバーの純関数の名前と、置いた場所。
- 2 列になる条件 (向きと画面幅)。実機で何を見ればよいか。
- 6 万件をドラッグで一気に飛ばしたとき、サムネイル要求が暴れないか
  (セルの materialize ごとに `bindThumbnail` が走る。既存の abort で足りるかを確認して報告する)。

# 環境設定パネルのレイアウト指針

環境設定ダイアログは左にページツリー、右にページ内容を置く。右ペインは
`src/ui_dialogs/preferences.rs` の共通 `pref_panel` `ScrollArea` が管理し、
スクロールバー分の右余白を差し引いた content width を各ページへ渡す。

## 右ペインの基本ルール

- ページ実装は `ui.available_width()` の範囲内へ収める。
- 右端にボタン、`ComboBox`、削除 `×`、展開 `▼` などを置く場合も、ページ側で
  スクロールバー分の余白を追加しない。共通 right gutter に任せる。
- `ui.separator()` はそのまま使う。共通 content width が効くため、横線は
  スクロールバー手前で止まる。
- 横並びが長くなる行は `ui.horizontal_wrapped` を使い、固定幅の `TextEdit` は
  `ui.available_width()` からボタン幅を引いた値で `desired_width` を決める。
- `right_to_left` レイアウトで右端ボタンを置く場合、親 UI の content width を超える
  `min_width` や固定幅を指定しない。
- ページ内にさらに `ScrollArea` を入れる場合は、高さを限定した局所リストだけにする。
  ページ全体の縦スクロールは外側の `pref_panel` に任せる。

## 新規ページ追加時の確認

- ダイアログ幅を既定値付近と狭めた状態の両方で、右端の文字・ボタン・区切り線が
  スクロールバーに重ならないことを確認する。
- ライト / ダークテーマで、弱い説明文とボタンラベルが読めることを確認する。
- 入力欄を追加するときは日本語 IME 中の Enter / Esc 挙動を既存ダイアログ helper に
  揃える。
- フォルダ走査、プラグイン scan、DB の大量削除など時間のかかる処理は UI スレッドで
  直接実行しない。ボタンは worker 起動や one-shot request の設定に留める。

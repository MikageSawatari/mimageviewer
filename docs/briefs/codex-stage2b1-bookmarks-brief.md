# 段 2b-1 — 静止画のブックマークタブ

worktree: `C:\home\mimageviewer-web` / branch `web-remote` / 起点 `7632a63e`

## 0. 立場

**本体が正本。独自の規則を発明しない。**
以下は私が読んで確認した内容だが、**実際と違っていたら実際の方を報告してほしい。**
私の要約に合わせて実装しないこと。この段取りで既に私の誤りが 18 回訂正されている
(直近の段 4a では「動画側に上スワイプの入口が既にある」と書いたが、正本の記述自体が
動画については誤りで、実装が無かった)。

稼働中の本体 / remote-web は操作しない。`build-dev.ps1`・コミットも実行しない。

正本: [web-remote-left-panel-plan.md](../web-remote-left-panel-plan.md) §2.1 / §7 段 2b。

## 1. これは何か

スマホの静止画パネルには `機能 | 画像補正 | 表示トリム | ブックマーク` の 4 タブがあるが、
**後ろ 2 つは見出しだけのプレースホルダ**
([app.js:5622](../../crates/remote-web/web/app.js))。何もしないタブが 2 つ見えている。

今回はそのうち**ブックマークタブ**を実装する。表示トリムは段 2b-2 で別に扱う
(スコープ 2 段 + 専用 DB + Auto 検出 + 見開き + 回転との相互作用があり、
リモートの合成経路のどこに入れるかを決める必要があるため)。

## 2. 本体側 (正本)

描画は `App::draw_bookmark_panel_body`
([ui_adjustment_panel.rs:12981](../../src/ui_adjustment_panel.rs))。

- **現在の本 / コンテナのぶんだけ**を出す (横断一覧ではない)
- ヘッダ: `この本のブックマーク` + 追加ボタン + `{n} 件`
- 対象外の項目では一覧を出さず 2 行の説明を出す
  (`current_book_bookmark_draft` [app.rs:26616](../../src/app.rs) が履歴 / レーティング /
  タグ / 検索などの合成ビューを除外している)
- 行: 58×58 サムネイル / 題名 (無ければ `名称なし`) / `{n} ページ` /
  `page_identity.display_name()`。現在ページの行は青く塗る
- 解決できない行は `ページが見つかりません` を橙で出し、**記録は消さない**
- 行の操作: 移動 / `名前を編集` / `削除`

記憶域は `book_bookmarks` テーブル ([book_bookmarks.rs:472](../../src/book_bookmarks.rs))。
**ページ番号は表示上の手がかりに過ぎず、移動先の正本は `PageIdentity`**
(同ファイル冒頭の doc comment)。

## 3. 既にあるもの / 足りないもの

**`RemoteWriteRequest::SetBookmark` は同じ概念・同じテーブル**
([lib.rs:418](../../crates/remote-ipc/src/lib.rs) →
[ui.rs:1368](../../src/remote_ipc/ui.rs) → 同じ `book_bookmark_service`)。
スマホの `機能` タブにある per-page トグルがこれを使っている。**追加は既にできる。**

足りないのは 3 つ:

| 要る操作 | 既存で代替できるか |
| --- | --- |
| コンテナ単位の一覧 | ❌ `GetItemState` は 1 ページの有無だけ |
| 題名の変更 | ❌ `SetBookmark` は真偽値のみ |
| id 指定の削除 | ❌ `SetBookmark` はページ指定の presence toggle |

`CollectionKind::Bookmarks` ([collections.rs:110](../../src/remote_ipc/collections.rs)) は
**横断ブラウザ**で別物。本タブの代わりにはならない。

### 置き場所

段 4a の動画は player を持つ session 側にしか置けなかったが、**今回は違う**と見ている。
`RemoteWriteRequest` は「書き込みの種類を足す唯一の場所」で、読み取りの `GetItemState` も
同じ queue に居る。ブックマークは `RemoteAddress` で引ける本の話なので、3 つとも
`RemoteWriteRequest` へ足すのが素直に見える。

**この判断が正しいか確認して報告してほしい。** 違うと思うなら根拠と一緒に。

## 4. サムネイル

**新しい経路を作らない。** 既存の `GET /api/thumb?address=...`
([http.rs:2135](../../crates/remote-web/src/http.rs)) が使えるはず。
一覧の各行が address を返せばよい。

段 4a で作った動画ジャンプ用の token 方式は**流用しない**。あれは保存済み WebP を
session の catalog から返す動画専用の仕組みで、静止画には既存の thumb 経路がある。

## 5. 移動

行をタップしたら、その本の中のそのページへ移動する。

**`PageIdentity` は index ではない。** 本体は `book_bookmark_item_idx`
([app.rs:26916](../../src/app.rs)) で現在の `items` に対して解決し、解決できなければ
トーストを出して**記録は残す**。ネスト ZIP は `enter_book_bookmark_archive_prefix`
(:26986) で親 prefix へ移動してから再解決する。

一覧応答は「表示用のページ番号 (`page_index_hint`)」と「解決できた移動先」を
**別々に**返すこと。混ぜると、解決できない行が 1 ページ目へ飛ぶような壊れ方をする。

リモートの ZIP 内 prefix 移動をどう扱うかは判断して報告してほしい。
**今回対応しないなら「見つかりません」で止めてよい** (本体も記録は保持する)。

## 6. 器

タブの中身の入れ方は既にある。`selectViewerTab`
([app.js:5594](../../crates/remote-web/web/app.js)) の `adjustment` 分岐が
`ViewerAdjustmentPanel` を placeholder へ遅延生成している。**同じ形に倣うこと。**

パネルの寸法・開閉・スワイプは共有済みなので触らない。

## 7. 調べて報告してほしいこと

1. §3 の「3 つとも `RemoteWriteRequest`」が構造的に正しいか
2. §5 の ZIP 内 prefix 移動を今回入れるか
3. 一覧の応答に何を載せるか (行の identity / 表示番号 / 移動先 / address / 解決可否)
4. 対象外の項目 (合成ビュー等) をリモートでどう判定するか。本体の
   `current_book_bookmark_draft` に相当する path ベースの入口が
   `remote_bookmark_draft` ([ui.rs:2366](../../src/remote_ipc/ui.rs)) にあるはずだが、
   一覧にも同じ判定が要るか

## 8. 受け入れ条件

- ブックマークタブに現在の本のブックマークが本体と同じ順・同じ情報で出る
- 行タップでそのページへ移動する。解決できない行は本体と同じ文言で止まり、記録は消えない
- 追加 / 名前変更 / 削除ができ、**PC 側にも同じ内容が即座に反映される**
  (§8.1 の決定どおり、リモートの変更は本物のデータを書き換える)
- 対象外の本では本体と同じ 2 行の説明が出る
- サムネイルが出る (既存の thumb 経路)
- 新しい HTTP 経路を足したなら fail-closed の認証 guard の下にあり、
  対応するテストに追加されている
- `cargo test -p mimageviewer --lib` / `-p mimageviewer-remote` / `-p mimageviewer-ipc` /
  web テストが緑

## 9. 注意

- `PROTOCOL_VERSION` ([lib.rs:17](../../crates/remote-ipc/src/lib.rs)、現在 **26**) を上げたら
  版固定テストも直す。両側の再ビルドと再起動が要る
  (`build-dev.ps1` は段 4a から remote も作る)
- `RemoteWriteRequest` に変種を足すと `address()` / `context_address()` / `kind_name()` の
  3 つの match を揃える必要がある
- UI 文言に内部用語を出さない (CLAUDE.md「マニュアル・製品ページの記述方針」)
- 端末は画面が消える。復帰して一覧が壊れないこと
- 実装は 2 コミット相当に分けて報告してほしい (protocol + core / Web UI)

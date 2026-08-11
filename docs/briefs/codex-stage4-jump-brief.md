# 段 4a — 動画のジャンプタブ (一覧と移動)

worktree: `C:\home\mimageviewer-web` / branch `web-remote` / 起点 `38b172ba`

## 0. 立場

**本体が正本。独自の規則を発明しない。**
以下は私が読んで確認した内容だが、**実際と違っていたら実際の方を報告してほしい。**
私の要約に合わせて実装しないこと。この段取りで既に私の誤りが 12 回訂正されている
(直近では「名前付きパイプは同じユーザーからしか開けない」と書いたが、既定 DACL は
ローカルの Everyone に read を許していた)。

稼働中の本体 / remote-web は操作しない。`build-dev.ps1`・コミットも実行しない。

正本: [web-remote-left-panel-plan.md](../web-remote-left-panel-plan.md) の §2.2 / §7 段 4 /
§9.2 / §9.3 / §9.4。

## 1. これは何か

スマホで動画を見ているとき、**チャプター / ブックマーク / ピンの一覧から目的の場面へ飛べる**
ようにする。PC 本体のフルスクリーン左パネルの「ジャンプ」タブと同じものを、端末に合わせた
見た目で出す。

補正パイプラインには触れないので段 0〜3 と独立している。

## 2. ⚠ 「軽い」のはデータだけで、UI は軽くない

計画表 (§7) は段 4 を「既存データを読むだけなので軽い」と書いている。**データについては
その通りだが、UI については正しくない。**

現在の動画メニュー (`VideoStreamMenu`、[video-stream.mjs:644](../../crates/remote-web/web/video-stream.mjs))
は **スクリム付きの全画面モーダル** (`command-menu-layer` / `role="dialog"` /
`aria-modal="true"`) で、`main` → `controls` の 2 ページ drill-down になっている。
静止画側のようなタブも、向きに応じた寸法も持っていない。

計画 §9.2 / §9.3 / §9.4 は動画にも次を要求している:

- タブ化 (`機能 | ジャンプ`。`画像補正` は段 5 なので今回は作らない)
- 縦持ちは下 50%、横持ちは左 40% の**非モーダル**パネル
- **動画は止めない。上半分 (または右 60%) に残り、再生は続く**

§9.3 の「動画ビューアの menu / gesture / viewport は段 4 まで変更しない」は
「段 4 で変更する」の意味に読んでいる (§9.4 が半分の高さの動画再生を明記しているため)。
**この読みが違うと思ったら指摘してほしい。**

したがって今回は「一覧を出す」より **メニューをパネルへ作り替える方が量が多い**。

## 3. 今回やること / やらないこと

| | 内容 |
| --- | --- |
| **4a (今回)** | パネル化 + ジャンプ一覧 (チャプター / ブックマーク / ピン) + タップで移動 |
| 4b (次) | スマホからの追加・削除、サムネイルの遅延生成 |

**4a に書き込みは入れない。** 読んで飛ぶだけ。理由は §6。

段 5 (動画の補正) は §10.6 で独立フェーズと決まっているので今回は触らない。
**タブ数を 2 で決め打ちしないこと** — 段 5 が入れば 3 本になり、入らない可能性もある。

## 4. データの出どころ (本体側)

PC 側の一覧は 3 つを時刻順に混ぜて、種別ごとの節に分けて出している
([overlay_draw.rs:1021](../../src/video/native_presenter/overlay_draw.rs))。

| 種別 | 出どころ | 備考 |
| --- | --- | --- |
| チャプター | `VideoInfo.chapters` ([decoder.rs:1571](../../src/video/decoder.rs)) | 開いた時に FFmpeg から読む。**DB は無い** |
| ブックマーク | `video_bookmarks.db` — `list_marker_entries(path)` ([video_bookmarks.rs:210](../../src/video_bookmarks.rs)) | id / pts / title |
| ピン | `video_pins.db` — `lookup_meta` ([video_pins.rs:141](../../src/video_pins.rs)) | 1 動画に 1 つ |

節の順序・色・見出しは PC と同じにする (ピン留め / ブックマーク / チャプター)。
**「マーカー」という別のデータは存在しない** — この 3 つの合併の呼び名なので、
新しい概念を作らないこと。

時刻の書式は [overlay_draw.rs:5966](../../src/video/native_presenter/overlay_draw.rs) の
`format_native_jump_entry_time` と揃える (同じ秒に丸まる 2 件があるときはミリ秒まで出す)。
この判定は本体側にあるので、**web で書き直さず、判定結果を持ってくる形にできないか**を
見てほしい。できないなら理由と一緒に報告を。

## 5. サムネイル

PC の各行は 120×68 のサムネイルを出す。これは**既に WebP で 3 つの DB に保存されている**:
`video_pins.thumb_webp` / `video_bookmarks.thumb_webp` / `video_chapter_thumbs.db`。

→ **保存済み blob をそのまま返す。** デコードもプレイヤーへの要求も要らない。

**保存済みが無い項目は placeholder のまま出す** (PC も "..." の箱を出す)。
今回は生成しない。理由:

既存の遠隔サムネイル API (`VideoStreamThumbnail`) は **session ごとに 1 枠しかなく、
最後に要求された位置だけ**を持つ ([ui.rs:1226](../../src/remote_ipc/ui.rs))。
これはシークバーのスクラブ preview が所有している。一覧のために奪うとスクラブが壊れる。
生成は 4b で、スクラブが空いている時だけ 1 件ずつ、という形で入れる。

**PC でジャンプパネルを一度も開いていない動画は、ほぼ全部 placeholder になる。**
これは今回の既知の制限として受け入れる。**もっと良い形があるなら提案してほしい。**

配り方:

- **一覧の JSON に base64 で埋めない。** 1 件 8〜15KB × 数十件が 1 応答に入り、
  33% 増える。既存の `GET /api/thumb` ([http.rs:2135](../../crates/remote-web/src/http.rs))
  と同じく、**別経路で `image/webp` を返して `Cache-Control` を付ける**
- 一覧は「サムネイルがあるか」だけを返す
- **画面に入った行だけ取りに行く** (グリッド側に既存の遅延読み込みがあればそれに倣う)
- URL の鍵は一覧応答をまたいで**安定**していること (でないとキャッシュが効かない)。
  ブックマークは DB の id があるが、ピンとチャプターには無い。
  チャプターの DB 側キーは `chapter_start_key(secs) = round(secs*1e6)`
  ([video_chapter_thumbs.rs:114](../../src/video_chapter_thumbs.rs))。
  **何を鍵にするかは判断して、根拠と一緒に報告してほしい**

## 6. 書き込みを 4a に入れない理由 (と、入れるときの置き場所)

PC のジャンプパネルには「ここにブックマーク」「ここにピン」「まとめて追加」「名前変更」
「削除」がある。これを入れるなら **`RemoteWriteRequest` ではない**と考えている:

- `RemoteWriteRequest` ([lib.rs:393](../../crates/remote-ipc/src/lib.rs)) は全変種が
  `RemoteAddress` + `page_index` = **ページ座標**で、時刻座標の書き込みが無い
- 動画のブックマーク追加は**その時刻のフレームをデコードしてサムネイルにする**必要があり
  ([native_video.rs:5954](../../src/app/native_video.rs))、それはプレイヤーを持つ
  video session の側にしかできない
- video の IPC 群は session 鍵で、既に `VideoStreamControl` / `Seek` / `Thumbnail` がある

→ 書き込みは video のメッセージ族に足すのが構造的に正しい、と見ている。
**4b の設計として、この判断が正しいか確認して報告してほしい。** 今回は実装しない。

## 7. 移動 (seek)

**新しい経路を作らない。** 既存の `seekTo(secs)`
([video-stream.mjs:1748](../../crates/remote-web/web/video-stream.mjs)) を呼ぶだけ。
バッファ内なら `currentTime` を動かし、外なら `POST /api/video/seek` で新しい generation を
取る、という既存の分岐がそのまま使える。

## 8. パネル化で守ること

- 静止画側の `viewerPanelLayout` / `viewerPanelTransition`
  ([command-core.mjs:304](../../crates/remote-web/web/command-core.mjs)) は**純関数で共有可能**。
  **同じ寸法規則を動画側に書き直さない。** 利用者から繰り返し受けている指摘
  (「2 箇所にわかれていると設定の動作の違いなどがおこる」) がそのまま当てはまる
- ただし静止画の `shouldRefit` / `resetTransform` は画像の pan/zoom 用で、動画には
  対応物が無い。**動画は残り矩形へ `<video>` を収め直すだけ**のはず。
  共有部分と動画固有部分の境界をどこに引くかは判断して報告してほしい
- 静止画の `CommandMenu` ([app.js:5423](../../crates/remote-web/web/app.js)) と
  動画の `VideoStreamMenu` は**別実装**。今回どちらへ寄せるか
  (共通化する / 動画側へ移植する) も判断して、根拠と一緒に報告してほしい。
  **共通化が大きすぎるなら無理に寄せなくてよい** — その場合は寸法・開閉・
  ジェスチャーの規則が共有関数から来ていることを確認できれば十分
- パネル外は**スクリムで暗くしない**。ただし透明な入力盾として扱い、
  タップやスワイプを背面へ通さない (§9.3 と同じ)
- **再生は止めない** (§9.4)
- 選択中のタブは再読み込みまで保つ。静止画の `state.viewerPanelTab` とは
  **別のタブ集合**なので鍵を分ける

## 9. 調べて報告してほしいこと

1. §2 の「段 4 でメニューをパネル化する」という読みが計画と合っているか
2. §4 の時刻書式を本体側から持ってこられるか
3. §5 のサムネイル URL の鍵を何にするか
4. §5 の「生成しない」より良い形があるか
5. §6 の「書き込みは video メッセージ族」が構造的に正しいか (4b の設計として)
6. §8 の `CommandMenu` / `VideoStreamMenu` をどう扱うか

## 10. 受け入れ条件

- スマホで動画を見ながらメニューを開くと、**動画が残ったまま**パネルが出る
- 縦持ちで下 50%、横持ちで左 40%。開いたまま回転しても崩れない
- `機能` / `ジャンプ` のタブがあり、ジャンプにピン / ブックマーク / チャプターが
  PC と同じ節・同じ順で出る
- 行をタップするとその時刻へ飛び、再生が続く
- 保存済みサムネイルがある項目には画像が出る。無い項目は placeholder
- サムネイルは画面に入った行だけ取りに行き、2 回目以降はキャッシュが効く
- 新しい HTTP 経路が **fail-closed の認証 guard の下**にあり、
  [http.rs](../../crates/remote-web/src/http.rs) の
  `every_video_stream_and_ai_route_is_below_the_fail_closed_auth_guard` に追加されている
- URL に秘密を埋めない
- `cargo test -p mimageviewer --lib` / `-p mimageviewer-remote` / `-p mimageviewer-ipc` /
  web テストが緑

## 11. 注意

- `PROTOCOL_VERSION` ([lib.rs:17](../../crates/remote-ipc/src/lib.rs)、現在 **25**) を上げると
  版固定テスト ([lib.rs:1824](../../crates/remote-ipc/src/lib.rs)) も直す。両側の再ビルドと
  **再起動**が要る
- UI 文言に内部用語を出さない (CLAUDE.md「マニュアル・製品ページの記述方針」)
- 新しいキー操作を足すなら `KeyAction` + keymap helper 経由 (CLAUDE.md)
- 端末は画面が消える。復帰して一覧が空にならないこと

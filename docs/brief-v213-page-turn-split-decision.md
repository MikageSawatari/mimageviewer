# ブリーフ: ページ送りの「何を描くか」と「UI スレッドで仕事をするか」を分ける

## 前提 (必ず守ること)

- **アプリを起動しない**。ビルドとテストまでで止める。
- **git 操作をしない**。master の作業ツリーに未コミットのまま残す。統合はこちらで行う。
- 直前のコミット `5a57a67b` の続き。**あれを revert するのではなく、分け方を直す**。
- 着手前に [docs/display-pipeline.md](display-pipeline.md) と
  [docs/ui-responsiveness.md](ui-responsiveness.md) を読むこと。

## 経緯

§1.58 (ページ送りの引っかかり) の狙いは、キーリピート中に **UI スレッドで 1 枚あたり
21ms のアップロード × 2 を実行してしまい、34ms のキー間隔に構造的に追いつかない**問題を
避けることだった。通過中のフレームはカタログサムネイルで描き、アップロードは保留する。
利用者要件は「**ページ表示自体は飛ばさない (一瞬でも見える形にする)**」。

`5a57a67b` で、カラー化 PDF のページ送り中に同じページのサムネイルと完成画像が毎フレーム
往復する退行を直した。判定に `final_composite_available` を足し、完成画像が出せるなら
`Materialize` を返すようにした。

**その結果、戻り方向が §1.58 以前の引っかかりに戻った** (利用者報告 2026-08-11、
「送り方向はよくなったが、戻り方向は前と同じ動きに見える」)。

## 壊れている前提

`FsPageTurnMaterialization::is_thumbnail_pass_through()` は、**性質の違う 3 つ**を
同時に制御している。

| # | 何を決めているか | 場所 |
| --- | --- | --- |
| 1 | このフレームで**何を描くか** (サムネイル / 完成画像) | [ui_fullscreen.rs:10755](../src/ui_fullscreen.rs:10755) `prepare_fullscreen_state` |
| 2 | final-effect worker の結果を**回収するか** (フルサイズ GPU upload) | [app.rs:54425](../src/app.rs:54425) `poll_final_effects` |
| 3 | `fs_upload_backlog` を**1 枚流すか** (フルサイズ GPU upload) | [app.rs:59864](../src/app.rs:59864) |

2 と 3 が §1.58 が避けたかった 21ms × 2 の実体。1 は「その結果として何が描けるか」に過ぎない。

`5a57a67b` は 1 を直すために判定そのものを `Materialize` へ倒したので、**2 と 3 まで
一緒に有効化してしまった**。

前後で非対称になる理由もここで説明が付く:

- **送り方向**は、まだ作っていないページへ進むので `final_composite_available` は false。
  従来どおり pass-through のままで速い。
- **戻り方向**は、**いま来たばかりのページへ戻る**ので完成画像が `final_composite_cache` に
  残っている。`final_composite_available` が true になり、毎フレーム `Materialize` =
  毎フレーム upload が走る。これが §1.58 以前の状態そのもの。

## 直し方

**2 つの問いを別々の述語にする。**

1. **UI スレッドの重い仕事を保留するか** = §1.58 の元の規則そのまま。
   `page_turn_input_pending && catalog_thumbnails_ready`。
   **`final_composite_available` を入れてはいけない。** consumer は上表の 2 と 3。
2. **何を描くか** = 1 が真で、**かつ完成画像が出せないとき**だけサムネイル。
   完成画像が出せるならそれを描く。consumer は上表の 1。

完成画像が**すでに常駐している**ものを描くのは、サムネイルを描くのと同じ費用 (どちらも
アップロード済み texture を貼るだけ) なので、2 を保留したまま 1 だけ完成画像にしても
§1.58 の効果は失われない。**ここが今回の肝**。

実装は次のどちらでもよい。読む側が取り違えられない形を選ぶこと。

- `FsPageTurnMaterialization` を `{ paint_source, defer_ui_uploads }` の 2 フィールドを持つ
  struct にして、`is_thumbnail_pass_through()` を廃止し、consumer ごとに別のメソッドを使わせる
- または述語を 2 つに割り、`fs_page_turn_defer_ui_uploads(ctx, idx) -> bool` と
  `fs_page_turn_paint_source(ctx, idx) -> Thumbnail | Composite` にする

**`is_thumbnail_pass_through()` という 1 つの真偽値を 3 か所が別々の意味で読む形は残さないこと。**
今回の退行はその形が原因なので、同じ形のまま条件だけ足すのは不可。

`5a57a67b` で入れた `fs_page_turn_display_unit_readiness` (見開き単位で
`catalog_thumbnails_ready` と `final_composite_available` を両方求める) と、
read-only の `current_final_composite_texture` 経由という制約はそのまま使ってよい。

### 同型の経路

上表の 3 か所すべてを新しい述語へ移すこと。**1 か所でも `is_thumbnail_pass_through()` 相当の
古い意味のまま残っていないか**を、コンパイルエラーで検出できる形 (= 旧 API を消す) にする。

## 完了条件 / 回帰テスト

純関数レベルで、2 つの出力を別々に固定する:

| 入力 pending | サムネイル | 完成画像 | 描くもの | upload 保留 |
| --- | --- | --- | --- | --- |
| true | あり | **あり** | **完成画像** | **する** ← 今回の要点 |
| true | あり | なし | サムネイル | する |
| true | なし | — | 完成画像 (従来どおり materialize 待ち) | しない |
| false | — | — | 完成画像 | しない |

加えて状態遷移テストを 2 本:

- 同一 idx で入力 pending が true/false/true と続いても、**完成画像が出せるようになった後は
  描画対象がサムネイルへ戻らない** (`5a57a67b` のテストを維持)
- **入力 pending が続く間は、完成画像が出せる状態でも upload 保留が解除されない**
  (今回の退行を直接押さえる。これが無いと同じ間違いを繰り返す)

- `cargo fmt --check` / `cargo check -p mimageviewer --bin mimageviewer-core` が warning なしで通る。
- `cargo test -p mimageviewer --lib page_turn` が通る。

## 報告してほしいこと

- 選んだ形 (struct 化 / 述語 2 分割) と、旧 API を消したかどうか。
- 3 consumer それぞれがどちらの述語を読むようになったか。
- 追加したテストの一覧。

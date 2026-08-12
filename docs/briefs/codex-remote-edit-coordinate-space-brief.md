# 保存済み編集の座標系を、カタログ由来の値から切り離す

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。master 取り込み (`151e37a1`) 後に
実機で出た表示崩れの修正。

## 0. 前提 — 先に読むもの

- `src/remote_ipc/container.rs` — `export_crop_rect_for_pixels` を呼ぶ 2 か所
  (**2365 行**と **3563 行**付近)、`comic_composite` へ `source_dims` を渡す箇所、
  `decode_source` (2211 行付近)、`decoded_source_dims` の解決 (3480-3510 行付近)
- `src/edit_source.rs` — `export_crop_rect_for_pixels` (415 行付近)
- `src/app.rs` — **本体側の同等物** `App::export_crop_rect_for_pixels` (51790 行付近)。
  座標系に `current_raw_source_pixels(idx)?.size` を使っている
- `src/catalog.rs` — `migrate_pdf_layout_dims` (589 行付近) と
  `PDF_LAYOUT_DIMS_META_KEY`

## 1. 症状と原因 (実機ログ + カタログ実データで確認済み。再調査不要)

利用者の iPad で、**PDF の 1 ページ目が青一色の画像に差し替わった**。

ログ (`target/dev-runtime/data-remote/remote-web-log.jsonl`) では、`target_px: 8192` の
foreground 要求に対して `ipc_status: "ok"` のまま **27×44 ピクセル / 822 バイト**が返っている。
これを全画面へ引き伸ばすので単色に見える。

原因は座標系の取り違えである。

- master が PDF サムネイルの `source_width/height` の**意味を変えた**
  (raster ピクセル → ページ枠の 1/1000 ポイント単位)。実データも
  `page_0000: thumb 336×512, source 468600×714360` となっている。**値は比としては正しい**
- remote は `export_crop_rect_for_pixels(crop, source_dims, pixels.size)` で、この値を
  **保存済みトリムの座標系**として使っている。トリム矩形は実ピクセル座標で記録されているため、
  468600 幅の座標系として解釈されると **1/140 程度に縮む**
- **本体は同じ用途にカタログ値を使っていない** (`current_raw_source_pixels(idx).size`)。
  だから本体では起きず、remote だけが壊れた

## 2. 決定 (2026-08-12、別セッションと合意)

- **本体側は `source_*` を px の意味へ戻す**方向で別途直す (正確な比が要る用途には別の列を足す)。
  ただし進行中の縦横比バグの原因が確定してからになる
- **remote は待たずに直す。** 直し方は「**保存済み編集の座標系をカタログ由来の値から取らない**」で、
  本体がどちらへ決着しても正しい形になる

## 3. 変更内容

### 3.1 座標系の出どころを変える

`export_crop_rect_for_pixels` と `comic_composite` に渡す座標系を、**本体が使うのと同じ
「元ラスタのピクセル寸法」**にする。カタログの `source_*` (`decoded_source_dims` 経由で
入ってくる値) を座標系として使わない。

- 通常画像では従来と同じ値になるはずである (カタログの `source_*` は元画像の実ピクセル)。
  **変わるのは PDF ページだけ**
- PDF ページでは、そのページを実際に描いたラスタの寸法を使う。本体の
  `current_raw_source_pixels` が指すものと一致させること
- 取得できない場合の fallback は現行どおり `pixels.size` でよい

### 3.2 取り違えを型かコメントで防ぐ

同じ変数名 `source_dims` が「比のための値」と「編集座標の基準」の両方に使われていたことが
今回の原因である。**呼び出し側の注意では再発する。**

- 編集座標の基準として渡す値には、用途が読める名前を付ける
  (例: `edit_source_dims` / `stored_edit_space`)
- カタログ由来の値を編集座標へ渡せない形にできるなら、そうする (newtype でもよい)。
  最低限、両者の違いをコメントに書く

### 3.3 このクラスの誤用が他に無いか確認する

`source_dims` を絶対座標として使っている箇所が remote に他にも無いか洗う。比 (`h/w`) として
使っている箇所は問題ない。見つかったものは同じ規則で直し、報告に列挙すること。

## 4. 触らないもの

- **`src/catalog.rs` の `source_*` の意味と移行**。本体側で別途扱う。ここでは触らない
- 本体の表示経路 (`src/app.rs` / `src/ui_fullscreen.rs`)。別セッションが作業中である
- 先読み設定 (`5d05fff8`)、表示所有権 (3a / 3b / 3c)、protocol

## 5. テスト

```
cp vendor/ffmpeg/bin/*.dll target/debug/deps/
cargo test -p mimageviewer --lib remote_ipc::
cargo test -p mimageviewer-remote
cargo fmt --all -- --check
```

**再発を止めるテストを入れること。** 今回の壊れ方は「カタログ値の意味が変わると黙って壊れる」
形だったので、次に意味が変わっても落ちるテストにする。

- 保存済みトリムのあるページで、**カタログ由来の値が実ラスタと桁違いでも**、
  出力サイズがトリク割合どおりになること (今回の 1/140 縮小が再現しないこと)
- 通常画像では従来と同じ結果になること (回帰が無いこと)
- 比としての利用 (`h/w`) は変えていないこと

## 6. ドキュメント

- plan に **§14.18** (または次の空き番号) を追加する。記録すること
  - 症状 (27×44 / 822 バイト) と原因 (カタログ値を編集座標の基準に使っていた)
  - **カタログの `source_*` は比のための値であって、編集座標の基準ではない**という境界
  - 本体側は px の意味へ戻す方向であること、remote の修正はそれに依存しないこと
- [`docs/virtual-folders.md`](../virtual-folders.md) に PDF の座標系に関する記述があれば揃える

## 7. 実行と報告

- §5 のコマンドを**毎回実行**して結果を報告する
- **`src/` と `crates/` に触れた箇所を全部、理由付きで報告する**
- **`scripts/build-dev.ps1` を実行しない。コミットもしない**
- §3.3 で見つけた他の誤用を列挙する
- ブリーフと意図的に違えた点があれば、その理由を報告する

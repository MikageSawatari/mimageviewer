# ページのラスタを 4 回作り直すのをやめる

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。**`C:\home\mimageviewer` ではない。**

- **1 件 = 1 コミット** (2 コミット)。
- `docs/briefs/HANDOFF.md` と他の brief は触らない。
- **commit は行わなくてよい** (worktree の `.git` は親リポジトリ側にあり sandbox から書けない)。
  変更を残したまま報告すればこちらでコミットする。
- `cargo fmt --all` を通し、末尾のテストを走らせる。

---

## 何が起きているか (計装済み、実測)

46 MP のページ 1 枚が 1489ms。うちコピーが 176ms + α ある。同じ 186 MB を**4 回**材料化している:

| # | どこ | 何をしているか | 実測 |
|---|---|---|---|
| 1 | `loaded_image_from_color_image` | `capture::color_image_to_rgba` = **画素ごとのループ** (`to_srgba_unmultiplied`) + 186 MB の `Vec` 確保 | **133ms** |
| 2 | `encode_remote_page_jpeg_timed` の resize | `resize_dynamic_fit` は寸法が変わらないとき `src.clone()` を返す。**ページ経路では常にそうなる** (ローダーが既に `target_px` へ縮小済み) | **43ms** |
| 3 | 同 JPEG 段 | `resized.to_rgb8()` = RGBA→RGB の変換 + 140 MB 確保 | `jpeg` 段 161ms の一部 |
| 4 | turbojpeg | それを読んで圧縮 | 残り |

**材料は最初から揃っている。**`egui::ColorImage` の画素は `Color32` (4 バイト) なので、
`ColorImage::as_raw()` (epaint の `bytemuck` feature、**コピーなし**の `&[u8]`) がそのまま
RGBA バイト列になる。turbojpeg 1.4 の `Image<T>` は `pixels` / `width` / `pitch` / `height` /
`format` を取るので、**`PixelFormat::RGBA` を指定すれば RGB への変換は要らず、`pitch` を使えば
部分矩形もコピーせずに渡せる**。

---

## 唯一の落とし穴: プリマルチプライ

`Color32` は**プリマルチプライ済み** sRGBA で、いまのコードが呼んでいる
`to_srgba_unmultiplied()` はアルファで割り戻す。**α=255 の画素では恒等**だが、
半透明画素があると生バイトは違う色になる (暗くなる)。JPEG はアルファを捨てるので、
**現在の出力は「不透明とみなした色」であり、生バイトを渡すと変わってしまう**。

したがって:

- **全画素が不透明なら**生バイトを借用する (コピー 0)。
- **1 画素でも α<255 なら**、いまの経路をそのまま使う (出力を変えない)。

不透明判定は `pixels.iter().all(|p| p.a() == 255)` の 1 パスでよい。186 MB を触るので
ただではないが (見積り 30〜40ms)、133ms の置き換えとしては十分に安い。**判定にかかった時間を
区間として記録すること** (下記)。

---

## コミット 1: `LoadedImage` がラスタを ColorImage のまま運ぶ

`LoadedImage.image: image::DynamicImage` ([container.rs](../../src/remote_ipc/container.rs) の
1080 行あたり) を `pixels: Arc<egui::ColorImage>` にする。

- **構築点は `loaded_image_from_color_image` の 1 つだけ**で、呼び出し 3 箇所はいずれも
  既に `Arc<ColorImage>` を持っている (`Arc::clone` で済む)。**この関数から
  `color_image_to_rgba` の呼び出しが消えるのが目的。**
- **サムネイル側の消費者** (`encode_thumb_webp` を呼んでいる箇所) は `DynamicImage` が要るので、
  そこで従来の変換を 1 回だけ行う。**サムネイルは小さいので費用は無視できる** (実測、一覧の
  サムネイル生成は全体で 3.6ms)。
- ページ側の消費者 2 箇所 (通常のページと AI ページ) は次のコミットで置き換える。
  このコミットでは、いったん同じ変換をしてから既存の encoder に渡してよい
  (= 挙動は変えず、型だけ移す)。**そうする場合はコミットメッセージにその旨を書くこと。**
- 他に `LoadedImage` を作っている箇所が無いか確認する。あれば報告する。

---

## コミット 2: ページの JPEG を ColorImage から直接作る

`encode_remote_page_jpeg_timed` を、`&egui::ColorImage` を受け取る形にする。

### 速い経路 (全画素が不透明)

1. **切り取り**: `view_trim_bbox` から今と同じ規則で画素境界を出す
   (`export_crop::CropRect::pixel_bounds`)。**コピーせず**、
   `pixels = &raw[(y * pitch + x * 4)..]` と `pitch = 元画像の幅 * 4` で表す。
   turbojpeg は `pitch >= width * format.size()` を要求するだけなので、これで部分矩形になる。
2. **縮小**: `fast_resize::aspect_accurate_fit_dimensions` で目標寸法を出し、
   **元と同じなら何もしない** (いまは `clone()` している)。違うときだけ従来どおり縮小する
   (この場合のコピーは受け入れる。ページ経路では通常起きない)。
3. **エンコード**: `turbojpeg::Image { pixels, width, pitch, height, format: PixelFormat::RGBA }`
   を `compress` に渡す。**`to_rgb8()` を通さない。**品質 (`PAGE_JPEG_QUALITY`) と
   `Subsamp::Sub2x2` は変えない。

### 従来経路 (半透明画素がある / 縮小が必要)

いまのコードのまま。**出力を変えない**ことが条件。

### 計装

既存の `trim` / `resize` / `jpeg` 段はそのまま残す (前後で数字を比べられるように)。加えて:

- `jpeg` 段に区間 `opacity_scan` (不透明判定にかかった時間) を `add_phase` で足す
- どちらの経路を通ったかを段の `outcome` で区別する
  (例: `finish_with_outcome(..., "zero_copy")` / `"unmultiplied"`)

---

## テスト (ここが本題)

**「速い経路と従来経路が同じ JPEG を吐く」ことを固定する。**これが唯一の安全網なので、
必ず**バイト列の比較**で書くこと (寸法だけの比較にしない)。

1. **不透明な画像で、両経路の出力が 1 バイト違わず一致する。**
   従来経路の関数を `#[cfg(test)]` で残すか、テスト内で
   「`color_image_to_rgba` → `RgbaImage` → `to_rgb8` → turbojpeg」を組み立てて比較する。
2. **半透明画素を 1 つ入れると従来経路に落ち、出力が従来と一致する。**
   (`Color32::from_rgba_unmultiplied` で作った画素を 1 つ混ぜる)
3. **切り取りあり**で、`crop_imm` してからエンコードした結果と一致する。
   幅が奇数になる矩形も 1 つ入れる (`pitch` の扱いを間違えると行がずれる)。
4. **縮小が必要なとき** (目標が元より小さい) は従来どおり縮小され、寸法が
   `aspect_accurate_fit_dimensions` の結果と一致する。

テストが本物かを自分で確かめること: 例えば `pitch` を `width * 4` に固定してしまう、
`PixelFormat::RGBA` を `RGB` にする、不透明判定を常に true にする、のいずれかを入れたときに
**対応するテストが実際に落ちる**ことを見てから報告する。落ちないテストは書き直す。

---

## やらないこと

- `capture::color_image_to_rgba` 自体の削除・高速化 (本体の他の利用者がいる)。
- ローダー側の `display` 変換 (166ms)。共有経路なので別途。
- 品質・サブサンプリング・目標寸法の規則の変更。

---

## 実行するテスト

```
cargo test -p mimageviewer --lib remote_ipc
cargo fmt --all -- --check
```

## 報告してほしいこと

- 2 つの変更それぞれで何をしたか (コミットはこちらで行う)。
- `LoadedImage` の構築点が本当に 1 つだったか。
- 不透明判定の実測 (テスト内で構わない) と、速い経路が使えなかった条件があればそれ。
- テストを潰したときに落ちることを確認した結果。
- ブリーフと意図的に違えた点があれば、その理由。

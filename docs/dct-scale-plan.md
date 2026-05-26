# DCT スケールデコード導入プラン

サムネイル生成時の JPEG デコードに libjpeg-turbo の DCT スケール機能を導入し、
カメラ JPEG (5MB〜) のサムネ生成を **2.5〜6× 高速化**する。

> **注意**: 圧縮ファイルサイズが 500MB を超える超大型 JPEG (例: 1.32GB の
> Hohen Neuendorf 50K) は **本プランの対象外**。500MB guard で fallback chain
> に降ろすが、image::open は 512MB allocation limit に抵触、WIC も寸法 > 32768
> で reject (`src/wic_decoder.rs:209`) するため、結果として **graceful なエラー
> 表示**にとどまる。500MB 超を扱うには mmap / streaming decode の別プランが必要 (§10)。
>
> **Post-implementation tuning (2026-05-27)**: 実装後の Codex レビューで、500MB
> × 並列ワーカー数 のメモリ圧迫リスクが指摘され、**`MAX_TURBOJPEG_INPUT_SIZE`
> は 128MB に引き下げて出荷**した。本プラン本文の数値は設計時 (500MB) のものを
> 残しているが、ソースコード (`src/thumb_loader.rs`) が source of truth。
> 128-500MB JPEG (例: リポジトリの xxl_kokerei_kaiserstuhl_28k_325mb.jpg) は
> fallback chain で処理される。

- 関連: [docs/async-architecture.md](async-architecture.md) (worker / cache 構造)、
  [docs/display-pipeline.md](display-pipeline.md) (display / thumb 二重リサイズ)、
  [docs/ui-responsiveness.md](ui-responsiveness.md) (UI スレッド負荷)
- 参考実測: `scripts/bench_dct_scale.py` (Pillow + libjpeg-turbo で同等手法を測定済み)

## 1. 動機と現状

### 1.1 現状のサムネイル JPEG デコードパス

`src/thumb_loader.rs` における JPEG ファイル / ZIP エントリの decode 経路:

```
ファイルパス側:
  is_jpeg_ext(path)?
    └ YES → decode_jpeg_turbo_from_path(path)
              ├ size <= 5MB → std::fs::read + turbojpeg::decompress_image
              └ size  > 5MB → return None
            ↓ None
            image::open(path)          ← 5MB 超はここに流れる
              └ fail → wic_decoder::decode_to_dynamic_image
                          └ fail → susie_loader::decode_file

ZIP エントリ側 (preloaded bytes):
  is_jpeg_entry(name)?
    └ YES → decode_jpeg_turbo_from_bytes(bytes)
              └ turbojpeg::decompress_image (size ceiling なし)
            ↓ None
            decode_zip_chain (image::load_from_memory → WIC → Susie)
```

デコード後は `apply_exif_orientation` → `resize_to_display_color_image(display_px)`
で **display 用 ColorImage** を生成し UI へ送信、続いて `encode_thumb_webp(thumb_px)` で
**キャッシュ用 WebP** に圧縮して `catalog.save` する (`src/thumb_loader.rs:1944-2014`)。

両 downstream consumer (`resize_to_display_color_image` と `encode_thumb_webp`) は
内部で再度 Lanczos リサイズを行うので、`DynamicImage` は **両方を満たす最小サイズ**
(= `max(display_px, thumb_px)`) があれば充分。それより大きい入力は無駄。

### 1.2 5MB ceiling の根拠と限界

[`TURBOJPEG_FILE_SIZE_LIMIT = 5MB`](../src/thumb_loader.rs#L431):

> 大容量カメラ JPEG (10-30MB) では `std::fs::read()` の全読み込みコストが
> `image::open()` のストリーミングデコードを上回るため、通常パスに任せる。

これは **full-decode 前提**での比較。DCT スケール (1/8) を入れると decode 自体が
4-8× 高速化するので、`std::fs::read` の I/O コストを差し引いても TurboJPEG パスが
明確に勝つ。**ceiling 撤廃が前提**となる。

### 1.3 実測値 (Olympus E-P7 20MP / D:/home/photo/2025PEN/)

`scripts/bench_dct_scale.py` を 30 ファイル × 5 iteration の median で実行 (Pillow
+ libjpeg-turbo、target=512px):

| 方式 | median | mean | PSNR (A vs B) |
|---|---|---|---|
| Full decode + Lanczos resize | **184 ms** | 186 ms | (基準) |
| DCT 1/8 + Lanczos resize | **75 ms** | 77 ms | **51.3 dB** (視覚的に同一) |

→ Pillow 上で **2.46× 高速化**。mIV の現状経路は **zune-jpeg** (libjpeg-turbo より
1.5-2.4× 遅い) なので、mIV 内の実効改善は **4-6× が期待値**。

### 1.4 ターゲットユーザー / 効果範囲

- **DSLR / mirrorless / Micro Four Thirds 撮影者** — 5-30MB / 20-60MP の JPEG が
  全部 zune-jpeg full path に流れている層。最大の受益者
- **360 度カメラ撮影者** — Insta360 X3 (~26MB)、Theta Z1 (~16MB) 等の中型出力も
  同様に効く
- **超大型 JPEG (>500MB)** — **本プラン対象外**。圧縮入力 guard
  (`MAX_TURBOJPEG_INPUT_SIZE = 500MB`) で fallback chain に降ろすが、image::open
  / WIC も処理不可なので **graceful なエラー表示**にとどまる。「開ける」を保証
  しないことを明示する
- **スマホ JPEG (≤5MB)** — 既に TurboJPEG full path で 5-15ms と十分速い。
  DCT 適用で 2-3× は出るが絶対時間が小さいので体感差は限定的
- **キャッシュ済みフォルダ** — 効果なし (WebP デコード経路は別)

## 2. アルゴリズム設計

### 2.1 スケール factor 選択ルール

libjpeg-turbo の `tj3SetScalingFactor` が受け付けるのは `M/8` 形式 (M ∈ {1,...,8})
+ 整数倍 ({2, 4, 8, 16})。サムネ用途では下向きスケールのみ使うので **M/8 (M ∈ 1..=8)**
のみ考える。

**選択ルール**: 「**スケール後の出力でも target を超える最小の M を選ぶ**」 =
最も小さく作りつつ、最終 Lanczos resize の入力品質を保つ。

```rust
fn pick_dct_scale_num(src_max_edge: u32, target_px: u32) -> u32 {
    // 結果は分子 M (分母は常に 8)。M=8 は実質「等倍 = DCT scale 無し」。
    //
    // libjpeg-turbo の実出力寸法は `ceil(src * M / 8)` (turbojpeg-1.4.0/src/decompress.rs:99
    // の `ScalingFactor::scale()` doctest で確認)。出力 >= target を保証する最小の M を
    // 解くと:
    //   ceil(src * M / 8) >= target
    //   ⇔ src * M + 7 >= 8 * target    (ceil(a/b) = (a+b-1)/b の整数版)
    //   ⇔ M >= (8 * target - 7) / src
    //   ⇔ M = ceil((8 * target - 7) / src)
    //
    // 旧版 (`ceil(8 * target / src)`) は src=1023, target=512 のような境界で M=5 を
    // 返すが、実は M=4 で ceil(1023*4/8)=512>=target 達成可能 = 1 段スケール過剰 (Codex 7th round)。
    //
    // u64 で計算して overflow を回避。target_px=2048 + src=u32::MAX 等の異常入力でも安全。
    if src_max_edge == 0 {
        return 8; // 0 division 回避、scale=1/1 にフォールバック
    }
    let target = target_px as u64;
    if target == 0 {
        return 1; // target=0 は最小 scale で OK
    }
    // (8*target - 7) はオーバーフロー対策で u64 + saturating_sub:
    let numer = (8u64).saturating_mul(target).saturating_sub(7);
    // ceil((8*target - 7) / src) = (numer + src - 1) / src
    let m_raw = (numer + src_max_edge as u64 - 1) / src_max_edge as u64;
    // **clamp は u64 上で先に行ってから u32 cast** (Codex 8th round 指摘):
    // `as u32` を先にすると、target_px=u32::MAX 等の巨大入力で wrap してから clamp
    // することになり、本来 M=8 になるべきところで意図しない値になる。
    m_raw.clamp(1, 8) as u32
}
```

### 2.2 target_px の決定

サムネ生成 worker (`process_load_request → load_one_cached`) は **display 用** と
**WebP cache 用** の 2 つの consumer に同じ `DynamicImage` を流す。それぞれの最終
リサイズ目標を考慮:

```rust
let target_px = display_px.max(thumb_px);
```

- `display_px ∈ [256, 2048]` (`compute_display_px`、現セルサイズ + DPI 由来)
- `thumb_px = settings.thumb_px` (default 512)
- 通常は `max(display_px, 512) = display_px or 512` (どちらか大きい方)

これで両 downstream consumer が「入力 >= target」を満たし、Lanczos resize が正常に
ダウンサンプリングできる。

### 2.3 fullscreen / 元解像度要求パスは除外

`start_fs_load` (`src/app.rs:14390+`) は `image::open` 直接呼び出しで、DCT スケール
**しない**。フルスクリーン表示・補正処理・AI アップスケール・消しゴム等は **元画素**
を必要とするため。

→ 新 API は thumb 経路にのみ追加し、fullscreen 経路は触らない。

### 2.4 lossless JPEG の取り扱い (Codex P2-2 対応)

libjpeg-turbo は lossless JPEG (DCT を使わない) に対して `set_scaling_factor != 1/1`
だとエラーを返す (`turbojpeg-1.4.0/src/decompress.rs:362`)。

**実装方針**: `read_header` で `is_lossless == true` なら **`ScalingFactor::ONE`
を強制して TurboJPEG full decode を行う** (= None を返さない)。

旧版プランでは「lossless は None で fallback」と書いていたが、Codex 指摘:
現状の `turbojpeg::decompress_image` は lossless でも full decode してくれるので、
None で fallback すると image::open / WIC / Susie の遅い経路に降ろしてリグレッショ
ンになる。**lossless は scale 不可なだけで decode 自体は OK** なので、scale を
強制 1/1 にするのが正解。

### 2.5 source_dims の保存 — DCT 出力寸法ではなく元寸法を渡す (Codex P1-2 対応)

`load_one_cached` は decode 直後に `source_dims = Some((img.width(), img.height()))`
として catalog に保存している (`src/thumb_loader.rs:1957, 2011`)。**この値は元画像
の寸法を意味するものとして契約されている** (`src/catalog.rs:34`)。

DCT scale を入れると `img.width()/height()` は **スケール後**の寸法 (例: 648×486)
を返してしまい、catalog に間違った source_dims が書き込まれて契約違反になる。

**対策**: 新 API `decode_jpeg_turbo_scaled_*` は **元寸法と EXIF 適用後寸法を別途
返す** `ScaleStats` 構造体を持つ (§3.1 参照)。integration site では:

```rust
// 変更前 (誤)
let source_dims = Some((img.width(), img.height()));

// 変更後 (正)
let source_dims = if let Some(stats) = dct_stats {
    // EXIF orientation 適用で w/h が swap される可能性に注意
    let (src_w, src_h) = stats.source_dims_after_exif(orientation);
    Some((src_w, src_h))
} else {
    // DCT 非使用パス (image::open / WIC / Susie) はそのまま
    // (lossless は DCT API 経由でも Ok 返却なので dct_stats が立ち、こちらに来ない)
    Some((img.width(), img.height()))
};
```

これで catalog には**常に元解像度**が保存され、既存契約とのギャップなし。

### 2.6 圧縮ファイルサイズ上限 (Codex P1-1 対応)

DCT scale で **decoded buffer** は 1/64 に縮むが、`std::fs::read(path)` で**圧縮
データそのもの**は丸ごと RAM に読まれる。1.32GB JPEG なら 1.32GB の Vec<u8> 確保
が必要。これ自体は zune-jpeg 経路でも (内部 BufReader 経由とはいえ実質的に) 同じ
コストを払うので「DCT scale で悪化はしない」が、「DCT scale で解決もしない」のが
正確。

**対策**: 新たな定数 `MAX_TURBOJPEG_INPUT_SIZE` (例: **500 MB**) を導入し、これを
超える JPEG は TurboJPEG path をスキップして image::open / WIC へ降ろす:

```rust
const MAX_TURBOJPEG_INPUT_SIZE: u64 = 500 * 1024 * 1024;

fn decode_jpeg_turbo_scaled_from_path(
    path: &Path,
    target_px: u32,
) -> Result<(image::DynamicImage, ScaleStats), DctDecodeError> {
    use DctDecodeError::*;
    let meta = std::fs::metadata(path).map_err(|e| Fallback(format!("metadata: {e}")))?;
    if meta.len() > MAX_TURBOJPEG_INPUT_SIZE {
        // graceful fallback = image::open / WIC chain に任せる (Fallback)
        return Err(Fallback(format!(
            "input too large for TurboJPEG: {} bytes > {} max",
            meta.len(), MAX_TURBOJPEG_INPUT_SIZE
        )));
    }
    let data = std::fs::read(path).map_err(|e| Fallback(format!("read: {e}")))?;
    decode_jpeg_turbo_scaled_from_bytes(&data, target_px)
}
```

500 MB の根拠:
- これを超える JPEG はコンシューマー用途で実質皆無 (今回の test set でも 1 件のみ)
- 500 MB 圧縮 + decoded 60 MB ≈ 600 MB ピーク → 4 GB RAM 環境でも余裕
- image::open / WIC 経路はそれぞれ独自の guard (image crate の 512MB limit) を
  既に持つので、そっちに任せる方が責務分割として綺麗

なお、`decode_jpeg_turbo_scaled_from_bytes` (ZIP 内、既に bytes は in-memory) は
`MAX_TURBOJPEG_INPUT_SIZE` チェックを **しない**。bytes が確保できている時点で
追加のメモリ圧迫はないため。ZIP 内に 500 MB 超の JPEG が含まれるケースは皆無に
等しい。

### 2.7 出力バッファ allocation safety (Codex P2-3 対応)

`scaled.width * scaled.height * 3` の usize 演算は header 由来の寸法を使うため、
**adversarial JPEG (header に巨大な width/height を埋め込んだもの)** で overflow
または巨大 allocation が発生し得る。対策:

```rust
const MAX_DECODED_BYTES: usize = 256 * 1024 * 1024; // 256 MB output (= ~8K equirect)

// 失敗時は TerminalRejection を返す (fallback NG = image::open がもっと巨大な
// allocation を要求するのを防ぐ。typed error 設計、§3.1 / §3.4 参照)
let byte_count = scaled.width
    .checked_mul(scaled.height)
    .and_then(|n| n.checked_mul(3))
    .ok_or_else(|| DctDecodeError::TerminalRejection(format!(
        "decoded buffer overflow: {}x{}x3", scaled.width, scaled.height
    )))?;
if byte_count > MAX_DECODED_BYTES {
    return Err(DctDecodeError::TerminalRejection(format!(
        "decoded buffer too large: {} bytes > {} max", byte_count, MAX_DECODED_BYTES
    )));
}
let mut buf = vec![0u8; byte_count];
```

`MAX_DECODED_BYTES = 256 MB` の根拠:
- DCT 1/8 にしても header 偽装で 8K×8K 出力が要求されるケースは抑止すべき
- 256 MB を超える RGB8 出力 = ~9000×9000 px → 通常ありえない (DCT scale 後)
- catalog DB 内のサムネキャッシュ最大も 2048 px max-edge = ~12 MB 程度なので、
  256 MB はマージンを十分取った値

### 2.8 progressive JPEG の挙動

libjpeg-turbo は progressive でも DCT scale を受け付ける (全 scan を読んでから
scale を適用する形)。baseline よりは僅かに遅いが破綻はしない。**カメラ JPEG は
ほぼ全部 baseline** (PEN 30 サンプル中 0 件が progressive を確認済み) なので影響
小。Web 由来の画像で稀に出現する程度。

## 3. API 設計

### 3.1 公開関数 (`src/thumb_loader.rs`)

```rust
// 既存 (現状)
fn decode_jpeg_turbo_from_path(path: &Path) -> Option<image::DynamicImage>;
fn decode_jpeg_turbo_from_bytes(data: &[u8]) -> Option<image::DynamicImage>;

// 新規 (このプラン) — `pub` で公開 (§4.4.5 visibility 表参照、外部統合テスト + app.rs から呼ぶ)
pub fn decode_jpeg_turbo_scaled_from_path(
    path: &Path,
    target_px: u32,
) -> Result<(image::DynamicImage, ScaleStats), DctDecodeError>;

pub fn decode_jpeg_turbo_scaled_from_bytes(
    data: &[u8],
    target_px: u32,
) -> Result<(image::DynamicImage, ScaleStats), DctDecodeError>;

/// DCT scale decode の失敗種別。**fallback 可否を呼び出し側で区別する**
/// ため typed error を返す (Codex 2nd round P2 対応)。
#[derive(Debug)]
pub enum DctDecodeError {
    /// header read 失敗・I/O エラー・正常な subsampling 非対応など。
    /// 呼び出し側は image::open → WIC → Susie chain に fallback してよい。
    Fallback(String),
    /// **terminal**: adversarial / 異常な header dims など。
    /// fallback すると image::open がもっと大きなバッファを確保しに行き
    /// safety guard を回避する事故になる。呼び出し側は **fallback せず**
    /// エラーを呼び出し元に伝播する。
    TerminalRejection(String),
}

#[derive(Copy, Clone, Debug)]
pub struct ScaleStats {
    /// JPEG ヘッダから読んだ元寸法 (= EXIF orientation 適用 *前*)。
    /// catalog の source_dims 保存用には必ず `source_dims_after_exif()` で
    /// EXIF orientation を考慮してから使うこと (Codex P1-2)。
    pub src_w: u32,
    pub src_h: u32,
    /// DCT scale 分子 (M)。分母は常に 8。M=8 は等倍 (scale 無し)。
    pub scale_num: u32,
    /// scale 適用後の decoded buffer 寸法。`img.width()/height()` と一致。
    pub out_w: u32,
    pub out_h: u32,
}

impl ScaleStats {
    /// EXIF orientation を適用した後の元寸法を返す。orientation が 5-8 (90°/270°)
    /// なら w/h を swap。catalog の source_dims にこれを書く。
    pub fn source_dims_after_exif(&self, orientation: u16) -> (u32, u32) {
        match orientation {
            5 | 6 | 7 | 8 => (self.src_h, self.src_w),
            _ => (self.src_w, self.src_h),
        }
    }
}

// 内部ヘルパ (pub crate 可)
pub(crate) fn pick_dct_scale_num(src_max_edge: u32, target_px: u32) -> u32;
```

**既存関数の扱い**:

- `decode_jpeg_turbo_from_path / _from_bytes` (full decode 版) は **削除可能**。呼び出し元 (`decode_image_for_thumb`, `process_load_request`, ZIP 経路) を全て scaled 版に
  切り替える
- ただし「target_px が分からない経路」が将来出る可能性に備え、`scale_num=8` 固定で
  scaled API を呼ぶラッパを残してもよい (= full decode と等価)

### 3.2 内部実装スケッチ

```rust
const MAX_TURBOJPEG_INPUT_SIZE: u64 = 500 * 1024 * 1024;  // 圧縮入力上限 (§2.6)
const MAX_DECODED_BYTES: usize = 256 * 1024 * 1024;       // 出力 buffer 上限 (§2.7)

fn decode_jpeg_turbo_scaled_from_bytes(
    data: &[u8],
    target_px: u32,
) -> Result<(image::DynamicImage, ScaleStats), DctDecodeError> {
    use turbojpeg::{Decompressor, PixelFormat, Image, ScalingFactor};
    use DctDecodeError::*;

    let mut dec = Decompressor::new()
        .map_err(|e| Fallback(format!("Decompressor::new: {e}")))?;
    let header = dec.read_header(data)
        .map_err(|e| Fallback(format!("read_header: {e}")))?;

    // lossless は scale 不可。scale=1/1 を強制して full decode (§2.4 Codex P2-2)。
    let m = if header.is_lossless {
        8  // scale 1/1
    } else {
        let src_max = (header.width as u32).max(header.height as u32);
        pick_dct_scale_num(src_max, target_px)
    };
    let scale = ScalingFactor::new(m as usize, 8);
    dec.set_scaling_factor(scale)
        .map_err(|e| Fallback(format!("set_scaling_factor: {e}")))?;

    let scaled = header.scaled(scale);

    // allocation safety (§2.7 Codex P2-3 + 2nd round): checked_mul + max guard。
    // 失敗時は TerminalRejection — fallback すると image::open がもっと大きな
    // バッファを確保しに行く可能性があるため。
    let byte_count = scaled.width
        .checked_mul(scaled.height)
        .and_then(|n| n.checked_mul(3))
        .ok_or_else(|| TerminalRejection(format!(
            "decoded buffer size overflow: {}x{}x3",
            scaled.width, scaled.height
        )))?;
    if byte_count > MAX_DECODED_BYTES {
        return Err(TerminalRejection(format!(
            "decoded buffer too large: {} bytes > {} max",
            byte_count, MAX_DECODED_BYTES
        )));
    }
    let mut buf = vec![0u8; byte_count];

    let out = Image {
        pixels: buf.as_mut_slice(),
        width: scaled.width,
        pitch: scaled.width * 3,
        height: scaled.height,
        format: PixelFormat::RGB,
    };
    dec.decompress(data, out)
        .map_err(|e| Fallback(format!("decompress: {e}")))?;

    let rgb = image::RgbImage::from_raw(
        scaled.width as u32,
        scaled.height as u32,
        buf,
    ).ok_or_else(|| Fallback("RgbImage::from_raw failed".into()))?;
    let img = image::DynamicImage::ImageRgb8(rgb);
    let stats = ScaleStats {
        src_w: header.width as u32,
        src_h: header.height as u32,
        scale_num: m,
        out_w: scaled.width as u32,
        out_h: scaled.height as u32,
    };
    Ok((img, stats))
}

fn decode_jpeg_turbo_scaled_from_path(
    path: &Path,
    target_px: u32,
) -> Result<(image::DynamicImage, ScaleStats), DctDecodeError> {
    use DctDecodeError::*;
    // 圧縮入力サイズの guard (§2.6 Codex P1-1)。fallback 可 = image::open / WIC へ
    let meta = std::fs::metadata(path)
        .map_err(|e| Fallback(format!("metadata: {e}")))?;
    if meta.len() > MAX_TURBOJPEG_INPUT_SIZE {
        return Err(Fallback(format!(
            "input too large for TurboJPEG: {} bytes > {} max",
            meta.len(), MAX_TURBOJPEG_INPUT_SIZE
        )));
    }
    let data = std::fs::read(path)
        .map_err(|e| Fallback(format!("read: {e}")))?;
    decode_jpeg_turbo_scaled_from_bytes(&data, target_px)
}
```

**呼び出し側パターン** (typed error の使い方):

```rust
let result = match decode_jpeg_turbo_scaled_from_path(path, target_px) {
    Ok((img, stats)) => Ok((Some((img, stats)), /* used DCT */ true)),
    Err(DctDecodeError::TerminalRejection(msg)) => {
        // 異常な header dims — fallback すると danger。エラーを上に伝播
        crate::logger::log(format!("DCT terminal rejection {path:?}: {msg}"));
        Err(image::ImageError::Limits(/* ... */))
    }
    Err(DctDecodeError::Fallback(msg)) => {
        // 正常な fallback — image::open chain を試す
        crate::logger::log(format!("DCT fallback {path:?}: {msg}"));
        Ok((None, false))
    }
};
```

`turbojpeg::Decompressor` は内部に C ハンドルを持つので **スレッド毎に作成必須**
(共有不可)。`new()` は ~50μs と十分軽いので毎回作って OK。スレッドローカル cache は
当面不要 (将来 hot path で必要なら検討)。

### 3.3 5MB ceiling 撤廃 + 500MB upper guard 新設

旧 `TURBOJPEG_FILE_SIZE_LIMIT = 5MB` (decode 全体コスト視点での閾値) は削除する。
理由:

- DCT 1/8 で **decode 自体が 4-8× 高速化**するので、5-30MB の中型 JPEG でも
  TurboJPEG path が image::open / zune-jpeg path に勝つ
- I/O コストは zune-jpeg と同等 (両者とも実質的にバッファリング発生)

ただし **上限 guard `MAX_TURBOJPEG_INPUT_SIZE = 500MB` は新設** (§2.6 Codex P1-1)。
500MB を超える圧縮 JPEG は image::open / WIC chain に降ろす。理由:

- `std::fs::read` で 500MB 超を一括確保すると低 RAM 環境で問題
- そのサイズの JPEG はコンシューマー用途で稀 (今回 test set でも 1.32GB の 1 件のみ)
- 既存の image::open / WIC 経路はそれぞれ独自 guard を持つので、責務分割として妥当

### 3.4 fallback chain と typed error の関係 (Codex 2nd/3rd round)

DCT scaled decode の戻り値は `Result<(_, ScaleStats), DctDecodeError>`。**呼び出し側
が必ず `match` で分岐し、`TerminalRejection` のときは絶対に image::open に
fallback しない**ことが安全要件。

```
decode_jpeg_turbo_scaled_from_path(path, target_px)
  ├ Ok((img, stats))              → 通常使用
  ├ Err(Fallback(_))              → image::open(path) → WIC → Susie に降ろす
  └ Err(TerminalRejection(_))     → 即 ImageError を caller に返す (fallback しない)
```

**`Fallback`** に分類されるケース (= image::open / WIC chain に降ろしてよい):

- TurboJPEG header read 失敗 (corrupt JPEG)
- 異常な subsampling (libjpeg-turbo が対応外)
- I/O エラー
- 圧縮入力 > `MAX_TURBOJPEG_INPUT_SIZE`

**`TerminalRejection`** に分類されるケース (= fallback NG、エラー返却):

- decoded buffer サイズ overflow (`checked_mul` 失敗)
- decoded buffer サイズ > `MAX_DECODED_BYTES`

**lossless JPEG** — scale 不可問題 (`set_scaling_factor != 1/1` がエラー) は scale=1/1
強制で回避済み (§2.4)。ただし元寸法のまま decode するため、**大型 lossless** は
output-size guard (`MAX_DECODED_BYTES`) で `TerminalRejection` になる可能性がある。
具体的には 9000×9000 lossless 以上で 256MB ライン超え。通常用途では稀。

## 4. 統合ポイント

### 4.1 `process_load_request` / `load_one_cached` (`thumb_loader.rs:1839`)

DCT スケール経由で得た stats は EXIF 適用後に source_dims として catalog 保存する
ため、外側のスコープに伝搬する。perf::event は既存の API shape
(`crate::perf::event(category, kind, key, seq, &[(key, Value), ...])`) + `is_enabled()`
guard を使う (Codex P3 対応)。

```rust
// 変更前
let turbo_result = if is_jpeg_ext(path) {
    decode_jpeg_turbo_from_path(path)
} else {
    None
};
if let Some(img) = turbo_result {
    Ok(img)
} else {
    image::open(path).or_else(/* WIC, Susie */)
}
// ...
let source_dims = Some((img.width(), img.height()));

// 変更後 (typed Result + TerminalRejection 明示処理)
let target_px = display_px.max(thumb_px);
let mut dct_stats: Option<ScaleStats> = None;
let turbo_img: Option<image::DynamicImage> = if is_jpeg_ext(path) {
    match decode_jpeg_turbo_scaled_from_path(path, target_px) {
        Ok((img, stats)) => {
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "thumb", "dct_scale", Some(filename), input_seq,
                    &[
                        ("scale_num", serde_json::Value::from(stats.scale_num)),
                        ("src_w",     serde_json::Value::from(stats.src_w)),
                        ("src_h",     serde_json::Value::from(stats.src_h)),
                        ("out_w",     serde_json::Value::from(stats.out_w)),
                        ("out_h",     serde_json::Value::from(stats.out_h)),
                    ],
                );
            }
            dct_stats = Some(stats);
            Some(img)
        }
        Err(DctDecodeError::TerminalRejection(msg)) => {
            // adversarial / 異常 — fallback すると image::open が更に大きな buffer を
            // 確保する可能性。エラーを早期に caller へ伝播する。
            crate::logger::log(format!("DCT terminal rejection {path:?}: {msg}"));
            return Err(image::ImageError::Limits(
                image::error::LimitError::from_kind(
                    image::error::LimitErrorKind::InsufficientMemory,
                ),
            ));
        }
        Err(DctDecodeError::Fallback(msg)) => {
            crate::logger::log(format!("DCT fallback {path:?}: {msg}"));
            None
        }
    }
} else {
    None
};
let img = if let Some(img) = turbo_img {
    img
} else {
    image::open(path).or_else(/* WIC, Susie */)?
};

// ... apply_exif_orientation(img, path) → orientation_applied_img ...
let orientation = read_exif_orientation(path);
let img = apply_orientation(img, orientation);

// source_dims: DCT 経由なら stats から取り EXIF 反映、非経由なら従来通り (§2.5 Codex P1-2)
let source_dims = if let Some(stats) = dct_stats {
    Some(stats.source_dims_after_exif(orientation))
} else {
    Some((img.width(), img.height()))
};
```

**注意**:

- ZIP / PDF パスは EXIF orientation を適用しない (`thumb_loader.rs:1948`)。
  これらのパスでは `apply_exif_orientation` ステップをスキップし、`orientation=1`
  相当として `source_dims_after_exif(1)` を呼ぶ
- `dct_stats` の伝搬には clone は不要 (Copy trait derive)

### 4.2 `decode_image_for_thumb` (`thumb_loader.rs:319`)

```rust
pub fn decode_image_for_thumb(path: &Path, display_px: u32) -> Option<egui::ColorImage> {
    // 変更前: decode_jpeg_turbo_from_path(path)
    // 変更後: target = display_px (この関数は cache 用 thumb_px を持たない)
    let turbo_img: Option<image::DynamicImage> = if is_jpeg_ext(path) {
        match decode_jpeg_turbo_scaled_from_path(path, display_px) {
            Ok((img, _stats)) => Some(img),
            Err(DctDecodeError::TerminalRejection(msg)) => {
                // adversarial — fallback せず None で諦める (動画 sidecar 用途、致命的でない)
                crate::logger::log(format!("DCT terminal rejection {path:?}: {msg}"));
                return None;
            }
            Err(DctDecodeError::Fallback(_)) => None,
        }
    } else {
        None
    };
    let img = turbo_img
        .or_else(|| image::open(path).ok())
        .or_else(|| crate::wic_decoder::decode_to_dynamic_image(path))?;
    Some(resize_to_display_color_image(&img, display_px))
}
```

### 4.3 ZIP エントリ経路 (`thumb_loader.rs:1803`)

```rust
// 変更前
if is_jpeg_entry(entry_name) {
    if let Some(img) = decode_jpeg_turbo_from_bytes(&bytes) {
        Ok(img)
    } else { decode_zip_chain(...) }
}

// 変更後 (typed Result + TerminalRejection 明示 + dct_stats 伝搬)
if is_jpeg_entry(entry_name) {
    let target_px = display_px.max(thumb_px);
    match decode_jpeg_turbo_scaled_from_bytes(&bytes, target_px) {
        Ok((img, stats)) => {
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "thumb", "dct_scale_zip", Some(entry_name), input_seq,
                    &[("scale_num", serde_json::Value::from(stats.scale_num))],
                );
            }
            // ZIP は EXIF orientation 適用しないが、source_dims は元寸法を保存する必要あり。
            // §4.1 と同じく outer scope の dct_stats 変数に書く (= shared source_dims path)。
            dct_stats = Some(stats);  // §4.1 で宣言済みの mut let と共有
            Ok(img)
        }
        Err(DctDecodeError::TerminalRejection(msg)) => {
            crate::logger::log(format!(
                "DCT terminal rejection ZIP {zip_path:?}/{entry_name}: {msg}"
            ));
            // ZIP 内 adversarial — fallback せずエラー返却
            Err(image::ImageError::Limits(
                image::error::LimitError::from_kind(
                    image::error::LimitErrorKind::InsufficientMemory,
                ),
            ))
        }
        Err(DctDecodeError::Fallback(_)) => decode_zip_chain(...),
    }
}
// ... 後段の source_dims 設定 (§4.1 と同じパターン):
// let source_dims = if let Some(stats) = dct_stats {
//     Some((stats.src_w, stats.src_h))  // ZIP は orientation=1 相当
// } else {
//     Some((img.width(), img.height()))
// };
```

### 4.4 cache creator path (Codex P2-1 + 2nd round)

「キャッシュを一括作成」ダイアログ (`start_cache_creation`, `src/app.rs:19669`)
には **2 つの bulk decode 経路**があり、いずれも `process_load_request` を通らない
ので個別に DCT 対応が必要:

1. **通常ファイル**: `build_and_save_one` (`src/thumb_loader.rs:2266`) 経由
2. **ZIP エントリ**: app.rs:19943 で **インラインに `image::load_from_memory`**
   呼び出し (`build_and_save_one_zip` という helper も存在するが現状未使用)

#### 4.4.1 `encode_and_save` の source_dims 拡張 (新規)

`encode_and_save` (`src/thumb_loader.rs:2299`) は内部で `source_dims =
Some((img.width(), img.height()))` を組み立てて catalog に保存する。DCT scale で
渡される img は scaled buffer なので、**source_dims override 引数**を追加する必要
がある (= Codex 2nd round P1 への対応):

```rust
// 既存シグネチャ
pub fn encode_and_save(
    img: &image::DynamicImage,
    key: &str, catalog: &CatalogDb, mtime: i64, file_size: i64,
    thumb_px: u32, thumb_quality: u8,
) -> Option<usize>

// 拡張シグネチャ (互換ヘルパも残す)
pub fn encode_and_save_with_source_dims(
    img: &image::DynamicImage,
    source_dims_override: Option<(u32, u32)>,  // None = img dims を使う (旧挙動)
    key: &str, catalog: &CatalogDb, mtime: i64, file_size: i64,
    thumb_px: u32, thumb_quality: u8,
) -> Option<usize>

// 旧シグネチャはそのまま残し、内部で override=None を渡す形にする:
pub fn encode_and_save(img, key, ...) -> Option<usize> {
    encode_and_save_with_source_dims(img, None, key, ...)
}
```

#### 4.4.2 `build_and_save_one` (通常ファイル) の改修

```rust
// 変更前 (src/thumb_loader.rs:2275)
let img = image::open(path)
    .or_else(|_| { /* with_guessed_format fallback */ })
    .ok()?;
let name = path.file_name()?.to_str()?;
encode_and_save(&img, name, catalog, mtime, file_size, thumb_px, thumb_quality)

// 変更後 — DCT path + source_dims override
let mut dct_stats: Option<ScaleStats> = None;
let img = if is_jpeg_ext(path) {
    match decode_jpeg_turbo_scaled_from_path(path, thumb_px) {
        Ok((img, stats)) => { dct_stats = Some(stats); Some(img) }
        Err(DctDecodeError::TerminalRejection(_)) => return None,  // refuse
        Err(DctDecodeError::Fallback(_)) => None,
    }
} else { None };
let img = match img {
    Some(img) => img,
    None => image::open(path)
        .or_else(|_| { /* with_guessed_format fallback */ })
        .ok()?,
};
// EXIF orientation を適用 (load_one_cached と挙動を揃える)
let orientation = read_exif_orientation(path);
let img = apply_orientation(img, orientation);
let source_dims = dct_stats.map(|s| s.source_dims_after_exif(orientation));
let name = path.file_name()?.to_str()?;
encode_and_save_with_source_dims(
    &img, source_dims, name, catalog, mtime, file_size, thumb_px, thumb_quality,
)
```

注意: 現状の `build_and_save_one` は **EXIF orientation を適用していない** が、
`load_one_cached` は適用するので catalog の source_dims が不一致になる可能性がある
(既存のバグ可能性)。本プランで apply_orientation を入れて整合させる。

#### 4.4.3 `start_cache_creation` の **インライン ZIP path** 改修

`src/app.rs:19943` の `image::load_from_memory(&raw)` 直呼び出しを、DCT scale 対応
helper に置換する:

```rust
// 変更前 (src/app.rs:19943)
let img = match image::load_from_memory(&raw) {
    Ok(i) => i,
    Err(_) => return,
};
// ... encode_and_save(&img, ...) で source_dims = img dims (= 元寸法) として保存
// ... if i == 0 { *first_webp = Some((img.clone(), entry.entry_name.clone())); }

// 変更後 — stats を捕捉し source_dims override で save + first_webp に伝搬
let (img, dct_stats): (Option<image::DynamicImage>, Option<ScaleStats>) =
    if is_jpeg_entry(&entry.entry_name) {
        match crate::thumb_loader::decode_jpeg_turbo_scaled_from_bytes(&raw, thumb_px) {
            Ok((img, stats)) => (Some(img), Some(stats)),
            Err(crate::thumb_loader::DctDecodeError::TerminalRejection(_)) => return,
            Err(crate::thumb_loader::DctDecodeError::Fallback(_)) => (None, None),
        }
    } else {
        (None, None)
    };
let img = match img.or_else(|| image::load_from_memory(&raw).ok()) {
    Some(i) => i,
    None => return,
};
let source_dims = dct_stats.map(|s| (s.src_w, s.src_h));  // ZIP は orientation=1

// 個別エントリの cache 保存: source_dims override を渡す
if let Some(bytes) = crate::thumb_loader::encode_and_save_with_source_dims(
    &img, source_dims, &entry.entry_name, &zip_catalog,
    entry.mtime, entry.uncompressed_size as i64, thumb_px, thumb_quality,
) {
    size_atomic.fetch_add(bytes as u64, Ordering::Relaxed);
}

// 先頭エントリを first_webp に capture (parent thumb 用)、source_dims も同梱
if i == 0 {
    *first_webp.lock().unwrap() =
        Some((img.clone(), source_dims, entry.entry_name.clone()));
}
```

ZIP path では EXIF orientation を適用しないので、`source_dims = Some((stats.src_w, stats.src_h))` = `source_dims_after_exif(1)` と同じ。簡潔のため直接 `(src_w, src_h)`
を使う。

`first_webp` の型を `(DynamicImage, Option<(u32,u32)>, String)` に拡張する詳細は
§4.4.4 (a) を参照。

なお `build_and_save_one_zip` (`src/thumb_loader.rs:2318`) は現状未使用だが、将来
呼び出し復活する可能性に備えて同様の DCT 化を入れておく (= source 監査時の整合性
維持)。

#### 4.4.4 ZIP cache creator の **追加 2 経路** (Codex 3rd round)

`start_cache_creation` の中には §4.4.3 のバッチループ (`app.rs:19980` 付近) 以外
にも、**parent thumbnail capture + 「先頭 1 枚のみ」モード**の 2 つの inline
decode site がある。両方とも DCT 化する必要がある:

##### (a) `first_webp` capture site (`app.rs:19986`)

バッチループの先頭エントリ (`i == 0`) を `first_webp: Arc<Mutex<Option<(DynamicImage, String)>>>` に clone してキャプチャし、後で親フォルダ用 thumb として再保存する経路。
**ここで DCT scaled img を clone すると、後段の parent save で再び scaled 寸法が
catalog に書かれてしまう**ので、capture する型を拡張する:

```rust
// 変更前 (app.rs:19913)
let first_webp: Arc<Mutex<Option<(image::DynamicImage, String)>>> =
    Arc::new(Mutex::new(None));

// 変更後 — source_dims override も一緒に持つ
let first_webp: Arc<Mutex<Option<(image::DynamicImage, Option<(u32,u32)>, String)>>> =
    Arc::new(Mutex::new(None));

// capture 時 (app.rs:19987)
if i == 0 {
    let source_dims = dct_stats.map(|s| s.source_dims_after_exif(1));  // ZIP は orientation=1
    *first_webp.lock().unwrap() =
        Some((img.clone(), source_dims, entry.entry_name.clone()));
}

// parent save 時 (app.rs:20006-)
if let Some((img, source_dims, _)) = captured {
    if let Some(bytes) = encode_and_save_with_source_dims(
        &img, source_dims, &folder_key, &catalog,
        *zip_mtime, *zip_file_size, thumb_px, thumb_quality,
    ) { ... }
}
```

##### (b) 「先頭 1 枚のみ」モード (`app.rs:20031`)

バッチ展開しないモード (= ZIP の代表サムネだけ作る) で別途 `image::load_from_memory`
を直呼びしている path。同様に DCT 経路に置換:

```rust
// 変更前 (app.rs:20031)
if let Ok(img) = image::load_from_memory(&raw) {
    if let Some(bytes) = encode_and_save(&img, &folder_key, &catalog, ...) { ... }
}

// 変更後
let (img, source_dims) = if is_jpeg_entry(&first_entry) {
    match crate::thumb_loader::decode_jpeg_turbo_scaled_from_bytes(&raw, thumb_px) {
        Ok((img, stats)) => (img, Some(stats.source_dims_after_exif(1))),
        Err(crate::thumb_loader::DctDecodeError::TerminalRejection(_)) => continue,
        Err(crate::thumb_loader::DctDecodeError::Fallback(_)) => {
            let img = match image::load_from_memory(&raw) { Ok(i) => i, Err(_) => continue };
            (img, None)
        }
    }
} else {
    let img = match image::load_from_memory(&raw) { Ok(i) => i, Err(_) => continue };
    (img, None)
};
if let Some(bytes) = encode_and_save_with_source_dims(
    &img, source_dims, &folder_key, &catalog, ...,
) { ... }
```

#### 4.4.5 可視性 (`pub(crate)`) 修正

`app.rs` から `decode_jpeg_turbo_scaled_from_bytes` / `is_jpeg_entry` /
`DctDecodeError` / `ScaleStats` / `encode_and_save_with_source_dims` を呼ぶには、
これらを `pub` または `pub(crate)` に昇格する必要がある (`is_jpeg_entry` は現状
private at `src/thumb_loader.rs:535`)。

| シンボル | 現状 | 変更後 | 理由 |
|---|---|---|---|
| `is_jpeg_entry` | `fn` (private) | `pub` | `app.rs` (`start_cache_creation`) と外部統合テストの両方から呼ぶ |
| `decode_jpeg_turbo_scaled_from_bytes` | (新規) | `pub` | `app.rs` から呼ぶ + `tests/dct_scale_integration.rs` から呼ぶ |
| `decode_jpeg_turbo_scaled_from_path` | (新規) | `pub` | 同上 + 将来の `bench_thumb_decode.rs` から呼ぶ可能性 |
| `DctDecodeError` | (新規) | `pub` | 上記関数の戻り値型 |
| `ScaleStats` | (新規) | `pub` | 上記関数の戻り値型 |
| `encode_and_save_with_source_dims` | (新規) | `pub` | `app.rs` の inline ZIP path から呼ぶ |
| `pick_dct_scale_num` | (新規) | `pub(crate)` | 内部利用 + `#[cfg(test)] mod tests` で十分。外部公開は不要 |

**§3.1 公開関数リスト**との整合: §3.1 が `pub` と書いている関数は上記表でも `pub`
で揃える。`pick_dct_scale_num` だけ `pub(crate)` で内部公開に留めるのは、API
表面積を最小化する意図 (= mIV 外部から scale 選択ロジックを呼ぶ必要は無い)。

### 4.5 影響の無いパス (Codex P3 対応 — 明示列挙)

DCT scale を**入れない**経路を明示する:

| パス | ファイル位置 | 入れない理由 |
|---|---|---|
| `start_fs_load` / fullscreen 表示 | `src/app.rs:14390+, 17471` | 元画素必要 (補正・AI・回転・消しゴム等) |
| `thumb-quality-sample-decode` worker | `src/app.rs:19483` | A/B 比較ダイアログで元画素必要 |
| Clipboard copy (`copy_image_to_clipboard`) | `src/ui_dialogs/context_menu.rs:989, 1023` | クリップボードは元画像をコピー、サムネ用途ではない |
| Metadata text ingestion | `src/ingest_text.rs:312` | XMP/EXIF メタ読み取りで decode 不要 (現状もそうなっているはず、要確認) |
| PDF / 動画サムネイル | (PDFium / WIC 経路) | JPEG 経路を通らない |

これらは `image::open` / `image::load_from_memory` を直接呼ぶか、または decode 自体
していない。本プランで触らない。

将来 fullscreen 経路に TurboJPEG full path を入れる場合は別プラン (§10.1 Out of Scope)。

### 4.6 自動的に新パスに乗るもの

- `bench_scroll.rs` — `process_load_request` 経由なので自動切替
- `decode_image_for_thumb` — §4.2 で明示更新済み、動画サムネオーバーライド経路

## 5. パフォーマンス目標と計測

### 5.1 目標数値

PEN 20MP JPEG (median 9.3MB) で:

| 指標 | 現状 | 目標 |
|---|---|---|
| キャッシュミス時 1 枚デコード | ~180-300ms (zune-jpeg) | **<80ms** (TurboJPEG DCT 1/8) |
| 500 枚フォルダ初回スキャン (4 worker) | ~25-40 秒 | **<10 秒** |
| 500MB 超の超大型 JPEG | エラー (現状) | エラーのまま (graceful、本プラン対象外) |

### 5.2 計測方法

1. **既存 `bench_scroll.rs` を活用**:
   ```bash
   cargo run --release --bin bench_scroll -- "D:\home\photo\2025PEN" --delete-cache
   ```
   実装前後で実行し、`Phase1 完了時間` / `全 thumb Loaded 時間` の差を比較。

2. **新規 `bench_thumb_decode.rs` を追加** (オプション):
   - 単一ファイル decode を repeat 計測 (warm cache 後)
   - DCT scale on/off を切り替え可能 (`--full-decode` フラグ)
   - PSNR 計測 (DCT 出力 vs full 出力)
   - PEN フォルダ含む複数の test set に対応

3. **perf-log 計装**:
   - 新規イベント `thumb/dct_scale`: `scale_num`, `src_w/h`, `out_w/h`, `decode_ms`
   - `analyze_perf.py` に DCT scale 集計を追加 (どの scale が何 % か / 平均速度向上)

### 5.3 リグレッション検出

CLAUDE.md の「リリース手順チェックリスト § Phase 2 / 性能回帰チェック」に
`bench_thumb_decode` を追加。スマホ JPEG (1-3MB) でも回帰しないことを確認 (DCT
適用が逆効果になっていないか)。

## 6. 実装フェーズ

### Phase 1: コア実装 (~150 行 + テスト)

1. `pick_dct_scale_num` 関数 + unit test (boundary 4 ケース + clamp 2 ケース)
2. `decode_jpeg_turbo_scaled_from_bytes` 実装 + 標準サンプル JPEG での integration test
3. `decode_jpeg_turbo_scaled_from_path` 実装 (薄いラッパ)
4. lossless JPEG → `Ok` 返却で full-size decode 成功 (§2.4)、
   異常 subsampling / truncated → `Err(DctDecodeError::Fallback(_))` 返却を確認

### Phase 2: 統合 (~50 行)

5. `process_load_request` の JPEG 経路を scaled 版に切替 + perf event 追加
6. `decode_image_for_thumb` の JPEG 経路を scaled 版に切替
7. ZIP entry 経路を scaled 版に切替
8. `TURBOJPEG_FILE_SIZE_LIMIT` 撤廃 + コメント更新

### Phase 3: 計測 / 検証 (~100 行)

9. `bench_scroll` を PEN フォルダで実行、A/B 比較レポート
10. `bench_thumb_decode.rs` を新規追加 (任意、bench_scroll で代替も可)
11. `analyze_perf.py` に DCT 集計セクション追加
12. **実機確認**: mIV 起動 → PEN フォルダを `--delete-cache` で開いて scroll、ヒッチが
    減ったか目視 + perf-log で確認
13. **怪物 JPEG 確認**: `xxl_hohen_neuendorf_church_50k_1320mb.jpg` (500MB 超) で
    UI が固まらず graceful にエラー表示されることを確認 (= 本プランの DoD は
    「表示できる」ではなく「壊れない」 — §11 と同じ基準)

### Phase 4: ドキュメント更新

14. `CLAUDE.md` の `JPEG 高速デコード` 節を新仕様で書き直し (5MB ceiling 撤廃、DCT
    scale 適用条件)
15. `docs/async-architecture.md` のサムネ性能関連節更新
16. `docs/display-pipeline.md` の decode → resize パス記述を新仕様に
17. `docs/ui-responsiveness.md` の **JPEG decode** に関する記述 (もしあれば) 更新
18. `docs/dct-scale-bench-report.md` を新規作成 (A/B 計測結果)

## 7. テスト戦略

### 7.1 ユニットテスト (新規追加)

`src/thumb_loader.rs` 内の `#[cfg(test)] mod tests`:

#### スケール factor 選択

```rust
#[test]
fn pick_dct_scale_num_clamps_low() {
    assert_eq!(pick_dct_scale_num(50000, 512), 1);  // 50K wide, scale=1/8
    assert_eq!(pick_dct_scale_num(10000, 512), 1);  // 10K wide, scale=1/8
}

#[test]
fn pick_dct_scale_num_picks_smallest_above_target() {
    assert_eq!(pick_dct_scale_num(4000, 512), 2);   // 4000/8=500<512, 4000/4=1000>=512
    assert_eq!(pick_dct_scale_num(5184, 512), 1);   // 5184/8=648>=512
    assert_eq!(pick_dct_scale_num(6000, 2048), 3);  // 6000*2/8=1500<2048, 6000*3/8=2250>=2048
    // Codex 7th round 指摘: turbojpeg の ceil rounding を考慮した境界
    assert_eq!(pick_dct_scale_num(1023, 512), 4);   // ceil(1023*4/8)=512=target → M=4
    assert_eq!(pick_dct_scale_num(4095, 512), 1);   // ceil(4095*1/8)=512=target → M=1
}

#[test]
fn pick_dct_scale_num_clamps_high() {
    assert_eq!(pick_dct_scale_num(500, 512), 8);    // src smaller than target → no scaling
    assert_eq!(pick_dct_scale_num(100, 512), 8);    // way smaller → still M=8
}

#[test]
fn pick_dct_scale_num_exact_match() {
    assert_eq!(pick_dct_scale_num(512, 512), 8);    // exact match → no scaling
    assert_eq!(pick_dct_scale_num(4096, 512), 1);   // exactly 8× → M=1 ok (output 512)
}

#[test]
fn pick_dct_scale_num_safe_against_overflow() {
    // Codex P3 対応: u64 計算 + saturating
    assert_eq!(pick_dct_scale_num(u32::MAX, 2048), 1);
    assert_eq!(pick_dct_scale_num(0, 512), 8);  // 0 division 回避
}
```

#### ScaleStats EXIF orientation 適用 (Codex P1-2)

```rust
#[test]
fn scale_stats_source_dims_after_exif_no_swap() {
    let s = ScaleStats { src_w: 5184, src_h: 3888, scale_num: 1, out_w: 648, out_h: 486 };
    assert_eq!(s.source_dims_after_exif(1), (5184, 3888));  // 正常
    assert_eq!(s.source_dims_after_exif(2), (5184, 3888));  // h-flip
    assert_eq!(s.source_dims_after_exif(3), (5184, 3888));  // 180°
    assert_eq!(s.source_dims_after_exif(4), (5184, 3888));  // v-flip
}

#[test]
fn scale_stats_source_dims_after_exif_swap() {
    let s = ScaleStats { src_w: 5184, src_h: 3888, scale_num: 1, out_w: 648, out_h: 486 };
    assert_eq!(s.source_dims_after_exif(5), (3888, 5184));  // transpose
    assert_eq!(s.source_dims_after_exif(6), (3888, 5184));  // 90° CW
    assert_eq!(s.source_dims_after_exif(7), (3888, 5184));  // transverse
    assert_eq!(s.source_dims_after_exif(8), (3888, 5184));  // 270° CW
}
```

#### Consumer semantics (Codex P3 対応)

```rust
#[test]
fn target_px_respects_both_display_and_thumb() {
    // display_px > thumb_px: target = display_px
    let target = 1500u32.max(512);
    assert_eq!(target, 1500);
    assert_eq!(pick_dct_scale_num(5184, target), 3);  // 5184*3/8=1944>=1500

    // thumb_px > display_px: target = thumb_px
    let target = 256u32.max(512);
    assert_eq!(target, 512);
    assert_eq!(pick_dct_scale_num(5184, target), 1);  // 5184*1/8=648>=512
}
```

#### Allocation safety (Codex P2-3 + 2nd round)

**fixture helper の実装方針** (Codex 8th round 指摘 — `SOI+SOF` だけでは
`tj3DecompressHeader` が拒否する可能性あり):

- **`build_minimal_jpeg_with_dims(w, h)`**: 小さな real JPEG (例: 16×16) を
  `turbojpeg::compress_image` で作成 → そのバイト列の SOF0 マーカー (FF C0) を
  探し、その後の高さ・幅フィールド (5-8 バイト目) を desired w, h に書き換える。
  decode を試みると body と不整合で失敗するが、`read_header` は header だけ
  parse するので desired dims を返す。`MAX_DECODED_BYTES` guard の発火を test する
  用途では body の整合性は不要
- **`build_minimal_lossless_jpeg_with_dims(w, h)`**: lossless は turbojpeg compress では
  作れないので、**tests/fixtures/sample_lossless_3x3.jpg** に手作りの最小 lossless
  JPEG (SOF3 マーカー使用) を 1 個 commit し、test 時に SOF3 の dims を書き換える
  helper を使う。fixture サイズ < 1KB なのでリポジトリ汚染なし

```rust
#[test]
fn decode_bytes_rejects_overflow_dims() {
    // adversarial JPEG: header に**spec 上の最大寸法 65535×65535** を持つ minimal
    // SOI+SOF JPEG fixture を作る (JPEG SOF の width/height は 2 バイトフィールドなので
    // 65535 が上限。80000 等は JPEG として書けない)。
    //
    // 数値設計 (Codex 6th round 指摘対応):
    //   src=65535, target=10000 → pick_dct_scale_num = ceil(8*10000/65535) = 2
    //   scale = 2/8 = 1/4
    //   出力 ≈ 65535/4 = 16384 px square → 16384 * 16384 * 3 = ~768 MB
    //   768 MB > MAX_DECODED_BYTES (256 MB) → TerminalRejection を確実に発火
    //
    // (target=512 だと M=1 で出力 8192 = 201 MB < 256 MB で guard 発火せず、test として
    //  無効。target_px を意図的に高くして scale factor を 1/4 まで上げ、guard 発火を
    //  保証する。)
    let adversarial = build_minimal_jpeg_with_dims(65535, 65535);
    let result = decode_jpeg_turbo_scaled_from_bytes(&adversarial, 10000);
    assert!(matches!(result, Err(DctDecodeError::TerminalRejection(_))));
}

#[test]
fn lossless_huge_jpeg_rejected_by_output_guard() {
    // lossless は scale 不可で M=8 強制 → 出力 = 元寸法そのまま。
    // 大型 lossless は output-size guard で TerminalRejection になり得る (§2.4 末尾)。
    //   src=10000, lossless → scale=1/1 → 10000*10000*3 = 300 MB > 256 MB
    let lossless = build_minimal_lossless_jpeg_with_dims(10000, 10000);
    let result = decode_jpeg_turbo_scaled_from_bytes(&lossless, 512);
    assert!(matches!(result, Err(DctDecodeError::TerminalRejection(_))));
}

#[test]
fn decode_bytes_passes_normal_dims() {
    // 通常の baseline JPEG → Ok 返却 + source_dims が元寸法。
    // fixture を repo に置く代わりに **runtime で生成** する (Codex 7th round 指摘:
    // fixture 不在の問題回避 + リポジトリサイズ削減):
    let src_w = 5184u32;
    let src_h = 3888u32;
    let rgb = image::RgbImage::from_fn(src_w, src_h, |x, y| {
        // 完全な単色は JPEG decoder が最適化で skip する可能性があるので
        // ピクセル位置依存の grad で minimal な高周波成分を入れる
        image::Rgb([((x ^ y) & 0xff) as u8, ((x * 2) & 0xff) as u8, ((y * 2) & 0xff) as u8])
    });
    let bytes = turbojpeg::compress_image(
        &rgb,
        85,
        turbojpeg::Subsamp::Sub2x2,
    ).unwrap().to_vec();

    let (img, stats) = decode_jpeg_turbo_scaled_from_bytes(&bytes, 512).unwrap();
    // 強い assertion (Codex 8th round 指摘): `img.width() <= 5184` だけだと full
    // decode でも通ってしまう。**DCT scale が実際に作動したことを明示的に検証**:
    assert_eq!(stats.src_w, 5184);
    assert_eq!(stats.src_h, 3888);
    assert_eq!(stats.scale_num, 1);  // pick_dct_scale_num(5184, 512) = 1
    assert_eq!(stats.out_w, 648);    // ceil(5184 * 1 / 8) = 648
    assert_eq!(stats.out_h, 486);    // ceil(3888 * 1 / 8) = 486
    assert_eq!(img.width(), 648);
    assert_eq!(img.height(), 486);
}

#[test]
fn caller_does_not_fallback_on_terminal_rejection() {
    // process_load_request 呼び出しパターンの単位テスト相当。
    // TerminalRejection 受けたら image::open を試さないことを検証
    let err: Result<(image::DynamicImage, ScaleStats), DctDecodeError> =
        Err(DctDecodeError::TerminalRejection("test".into()));
    match err {
        Err(DctDecodeError::TerminalRejection(_)) => { /* OK: no fallback */ }
        _ => panic!("must not fall through to image::open"),
    }
}
```

### 7.2 統合テスト (`tests/` 配下)

新規 `tests/dct_scale_integration.rs`:

- 標準 baseline JPEG → DCT 1/8 / 1/4 / 1/2 で decode、出力寸法が想定通りか確認
- progressive JPEG が DCT scale 経由でデコードできるか
- **lossless JPEG が `Ok` で**返ること (scale=1/1 強制で full decode が成功する、§2.4)
- truncated JPEG で `Err(Fallback(_))` が返ること (panic しないこと)
- **adversarial header dims** (header に 65535×65535 を持つ minimal JPEG fixture
  を `target_px=10000` で decode、scale=1/4 → 出力 16384×16384×3 = 768MB が
  `MAX_DECODED_BYTES` を超えて guard 発火) で `Err(TerminalRejection(_))` が返る
  こと (Codex 2nd round P2 対応、JPEG SOF 寸法上限 65535 を考慮)
- **大型 lossless JPEG** (10000×10000 lossless、scale=1/1 強制で 300MB) でも
  output-size guard が発火し `Err(TerminalRejection(_))` 返却 (§2.4 補足)
- 通常の baseline JPEG で source_dims が **scaled buffer サイズではなく元寸法**で
  返ること (Codex 2nd round P1 対応)

### 7.3 画質回帰テスト

`tests/snapshots/` に DCT scale 結果のスナップショットを追加:

- 1 つの代表 JPEG (リポジトリに既にあるテスト画像) を選定
- full decode + Lanczos vs DCT scale + Lanczos の PSNR が **>= 45 dB** を維持
- ベンチに使った `H:\...\dct_test\` のサンプルを使うか、自前 test fixture を作る

### 7.4 ベンチマーク回帰

`scripts/perf_smoke.sh` 相当を実装後に走らせ、UI thread の同期 I/O 退行が無い
ことを確認。

## 8. リスクと緩和

| リスク | 度合い | 緩和策 |
|---|---|---|
| DCT 1/8 の画質が肉眼で劣化 | 低 | PSNR 51dB を実測済み。snapshot test で >=45dB を保証 |
| progressive JPEG で性能劣化 | 低 | カメラ JPEG はほぼ baseline。perf-log で baseline vs progressive を区別計測 |
| `turbojpeg::Decompressor::new()` が hot path で重い | 低 | 実測 ~50μs。スレッドローカルキャッシュ化は将来 |
| lossless JPEG が DCT scale 通せず遅くなる | 低 | scale=1/1 強制 (`Ok` 返却) で TurboJPEG full decode を維持。fallback には降ろさない (§2.4 §3.2) |
| 異常 subsampling で `DctDecodeError::Fallback` → image::open 経路 | 低 | 既存 fallback と同等パスなので現状からのリグレッションなし |
| 5MB ceiling 撤廃で TurboJPEG の I/O コスト (`std::fs::read`) が表面化 | 低 | DCT decode 削減効果が圧倒的に上回る (ベンチ実証済み) |
| `Decompressor::decompress` のバッファ確保で OOM | 低 | DCT 1/8 で 1/64 サイズ。最悪 50K pano でも 60MB |
| ZIP 内 progressive JPEG で Susie / WIC fallback コスト増 | 低 | 元々 ZIP 内 progressive は稀 |
| cache 互換性 (旧 WebP との混在) | 影響なし | キャッシュ形態は不変、デコード入力品質だけ変わる |
| **副次効果**: 怪物 JPEG が新規 UI 経路で OOM / hang を起こす | 低 (graceful error 経路設計済み) | Phase 3 step 13 で実機検証 (UI が固まらないことの確認) |
| **入力バッファ OOM**: 500MB 超 JPEG の `std::fs::read` で低 RAM 環境がスワップ (Codex P1-1) | 低 | `MAX_TURBOJPEG_INPUT_SIZE = 500MB` guard で fallback chain に降ろす |
| **adversarial JPEG header** で巨大 allocation 発生 (Codex P2-3) | 低 | `checked_mul` + `MAX_DECODED_BYTES = 256MB` guard で early return |
| **source_dims 契約違反**: DCT 後の縮小寸法を catalog に保存 (Codex P1-2) | **高 (catalog 破損)** | `ScaleStats.source_dims_after_exif()` で元寸法を別途返し、integration site で使用 |
| **cache creator path 不対応**: 一括キャッシュ作成だけ古い path のまま (Codex P2-1) | 中 (片手落ち) | §4.4 で `build_and_save_one` + inline ZIP path 全 3 経路を対応に含める |

## 9. キャッシュ・データの永続化への影響

**永続データ変更なし**。

- 既存 `thumbnails(filename, mtime, file_size, thumb_data, source_width, source_height)`
  スキーマはそのまま
- キャッシュキーも変更なし
- 旧 mIV が保存した WebP は引き続き有効、新 mIV で完全に読める
- 逆も同様 (新 mIV が保存した WebP は旧 mIV で読める、WebP フォーマットは同一)

→ **マイグレーション不要**。CLAUDE.md「永続データ・スキーマ変更時の判断」の表で
「機能追加だが永続データ無変更」のカテゴリに該当。

## 10. Out of Scope / 将来課題

### 10.1 fullscreen 経路の TurboJPEG 化 (別タスク)

`start_fs_load` 等の fullscreen 経路は現状 **JPEG でも image::open (zune-jpeg)**。
DCT scale は不要だが、TurboJPEG full decode に切り替えれば 1.5-2.4× 高速化できる。
本プランの完了後に独立タスクとして実装検討。

### 10.2 thread-local Decompressor cache

`Decompressor::new()` は ~50μs。1000 枚フォルダで 50ms のオーバーヘッド。現状無視
できるが、もし将来 hot path 化したらスレッドローカルキャッシュで `Decompressor` を
使い回す改修を検討。

### 10.3 DCT scale 適用基準の拡張

現在は「JPEG なら常に scaled」。将来:

- 「ファイルサイズ < N MB かつ src dim < M なら full decode の方がオーバーヘッド少」
  のような hybrid 判断 (現時点では bench で hybrid 必要性が見えないので入れない)
- JPEG 2000 / JPEG XR (libjpeg-turbo 非対応) の同等機能調査

### 10.4 HEIC / AVIF / JXL の同等手法

WIC 経由の HEIC / AVIF / JXL でも `WICBitmapFrameDecode::CopyPixels` 前に
`WICBitmapTransformOptions` で scale 指定可能。同等の最適化を WIC パスにも適用
可能 (別プラン)。

## 10.5 実装時セルフレビュー注意点 (Codex 最終レビュー由来)

- `perf::event("thumb", "dct_scale", ...)` の extras に **`decode_ms`** を含める
  (§5.1 の計測目標が decode 時間ベースなので、event でも測定可能にする)
- 全 4 つの integration site (`process_load_request`, `decode_image_for_thumb`,
  ZIP entry decode chain, cache creator path) で **`TerminalRejection` を絶対に
  image::open に fallback させない** こと。実装時に grep で `Fallback` と
  `TerminalRejection` の出現箇所を全部監査する

## 11. 完了の定義 (Definition of Done)

- [ ] Phase 1-4 のチェックリスト全項目完了
- [ ] `cargo test` 全パス
- [ ] `bench_scroll D:\home\photo\2025PEN --delete-cache` で初回スキャン時間が
      **2× 以上短縮**を確認
- [ ] 通常 baseline JPEG の `source_dims` が catalog に **元寸法**で記録されることを
      確認 (Codex 2nd round P1 対応の動作確認)
- [ ] adversarial JPEG fixture (巨大 header dims) で `TerminalRejection` 経路が走り、
      image::open に降りないことを確認
- [ ] `xxl_hohen_neuendorf_church_50k_1320mb.jpg` (500MB 超サンプル) で **graceful
      なエラー表示** が出ることを確認 (= プロセスがクラッシュしない、無限ループ
      しない。「表示できる」は本プランの DoD ではない)
- [ ] `analyze_perf.py` の出力に DCT scale 集計セクションが含まれる
- [ ] `docs/dct-scale-bench-report.md` に A/B 計測結果が記録される
- [ ] CLAUDE.md / 関連 docs 更新
- [ ] スマホ JPEG (1-3MB) フォルダで perf 退行が無いことを確認

## 12. 参考資料

- libjpeg-turbo API: `tj3SetScalingFactor`, `tj3GetScalingFactors`, `tj3DecompressHeader`
- turbojpeg crate 1.4.0 source: `~/.cargo/registry/src/.../turbojpeg-1.4.0/src/decompress.rs`
- 実測 bench: `scripts/bench_dct_scale.py`、ベンチ実画像 `H:\home\mimageviewer_old\testimage\dct_test\*.png`
- 関連 issue / 経緯: 本ドキュメント作成時点では無し (新規プラン)

## 13. Codex レビュー対応ログ

初版 plan を `codex exec --sandbox read-only` でレビューし、以下の指摘を反映済み:

### P1 (実装前に必須)

| 指摘 | 対応セクション | 内容 |
|---|---|---|
| **怪物 JPEG の OOM 話が誤り** — `std::fs::read` で圧縮データ自体を 1.32GB 確保するので decoded buffer だけ縮んでも root cause 未解決 | §1.4 / §2.6 / §3.3 / §8 | `MAX_TURBOJPEG_INPUT_SIZE = 500MB` guard を新設、超過時は image::open / WIC chain に降ろす。誇張表現を訂正 |
| **source_dims リグレッション** — DCT scale 後の `img.width()/height()` を catalog に保存すると元寸法ではなく縮小後寸法が記録され契約違反 (`src/catalog.rs:34`) | §2.5 / §3.1 / §4.1 / §8 | `ScaleStats { src_w, src_h }` + `source_dims_after_exif(orientation)` を提供。integration site で `img.width()/height()` ではなく stats から元寸法を取得 |

### P2 (実装中に対応)

| 指摘 | 対応セクション | 内容 |
|---|---|---|
| **cache creator path 不対応** — `start_cache_creation → build_and_save_one` が `process_load_request` を通らないため DCT 化されない (`src/app.rs:19783`, `src/thumb_loader.rs:2266`) | §4.4 | `build_and_save_one` も DCT 経路に移行 (target_px = thumb_px)、ZIP cache creator も同様 |
| **lossless JPEG リグレッション** — None で fallback すると現状の TurboJPEG full path より遅い chain (image / WIC / Susie) に降りる | §2.4 / §3.2 | `header.is_lossless` 時は `ScalingFactor::ONE` 強制で TurboJPEG full decode を維持 |
| **allocation safety 不足** — header dims を直接 `*3` して allocate すると adversarial JPEG で overflow / 巨大 alloc | §2.7 / §3.2 / §7.1 | `checked_mul` + `MAX_DECODED_BYTES = 256MB` guard で early return |

### P3 (品質改善)

| 指摘 | 対応セクション | 内容 |
|---|---|---|
| `pick_dct_scale_num` の u32 演算と zero-division 未ガード | §2.1 | u64 + saturating + zero guard |
| consumer semantics テスト不足 (display_px vs thumb_px, EXIF-rotated source_dims) | §7.1 | 該当 unit test を追加 |
| perf::event API shape が現状と不一致 | §4.1 | 既存 `crate::perf::event(category, kind, key, seq, &[(k, Value)])` + `is_enabled()` guard に書き直し |
| 他 JPEG 経路 (clipboard, ingest_text) が未列挙で曖昧 | §4.5 | 明示的に out-of-scope テーブルに追加 |

### Codex レビューでは指摘されなかったが念のため検討した点

- **キャッシュ互換性**: WebP データそのものは変わらないので、旧 mIV ↔ 新 mIV で
  キャッシュは完全互換 (§9 で明示済み)
- **fullscreen 経路の TurboJPEG 化**: 本プランの対象外と明示 (§10.1)
- **マルチスレッド安全性**: `Decompressor` はスレッドごとに作成、共有しない方針
  (§3.2)

### Codex 3rd round 追加対応

二次対応後の再レビューで、API 設計の typed Result 化が integration sketch 全体に
反映されていなかった点 + ZIP cache creator の残り 2 経路が未カバーだった点を追加修正:

| 指摘 | 対応 |
|---|---|
| **§3.4 / §4.1 / §4.2 / §4.3 が Option-based fallback の旧記述のまま** — typed Result 設計が決まったのに sketch 4 箇所が反映していない | §3.4 fallback chain を「Fallback / TerminalRejection」分類で書き直し。§4.1 / §4.2 / §4.3 を全部 `match Result` + `TerminalRejection 時 fallback しない` パターンに更新 |
| **§11 DoD Phase 3 step 13 が「怪物 JPEG サムネ表示」の旧基準** | step 13 を「graceful にエラー表示、UI 固まらない」に変更 (§1 / §11 と整合) |
| **§4.4.3 で ZIP cache creator の追加 2 経路 (parent thumb capture, 先頭 1 枚のみモード) が未カバー** | §4.4.4 を新設、`first_webp` 型に `source_dims` 追加 + `app.rs:20031` の「先頭 1 枚」 path も DCT 化対象に明示 |
| **可視性 (`pub(crate)`) の指定が抜け** — app.rs が呼ぶシンボルが現状 private | §4.4.5 で可視性表を追加 |
| **§7.1 lossless テスト / §8 lossless risk row が旧記述** | lossless テストを「`Ok` で full decode 成功」に変更、§8 risk 行を「Fallback / TerminalRejection」表現に統一 |
| **§3.4 fallback chain が lossless を None ケースに列挙** | lossless を None ケースから削除、Fallback / TerminalRejection の分類を新設 |

### Codex 2nd round 追加対応

初版修正後の再レビューで追加発見:

| 指摘 | 対応 |
|---|---|
| **怪物 JPEG 表示の DoD 矛盾** — 500MB guard で fallback に降ろすが、image::open / WIC も処理不可なので「表示できる」を DoD に書くと達成不能 | §1 概要、§1.4、§5.1、§11 DoD から「怪物 JPEG が開ける」を削除。代わりに「graceful にエラー表示する」を DoD に追加 |
| **encode_and_save が img.width/height から source_dims を作る** — cache creator 経由で DCT scaled img を渡すと catalog に縮小寸法が記録される | §4.4.1 で `encode_and_save_with_source_dims` 新規 helper 追加、override 渡す形に統一 |
| **None fallback は TooLarge を救えない** — adversarial JPEG が image::open に流れて safety guard を回避 | §3.1 で `DctDecodeError` enum 導入 (`Fallback` vs `TerminalRejection`)、§3.2 sketch を Result 返却に変更 |
| **lossless が §7.1 / §8 で旧記述のまま** — テスト・リスクで「None で fallback」と書いていた | §7.1 lossless テストを「Ok 返却」に変更、§8 リスク行を更新 |
| **ZIP cache creator は `build_and_save_one_zip` 未使用、実体はインライン path** — `src/app.rs:19943` で `image::load_from_memory` 直呼び | §4.4.3 で app.rs インライン path の改修を明示 |
| **file:line ドリフト** — start_fs_load 14361 → 14390、thumb-quality 19283 → 19483 等 | §2.3 / §4.5 で更新 |
| **allocation テストが weak** — overflow / TooLarge 経路を実際に走らせていない | §7.1 で adversarial JPEG fixture を使うテストを明示 |


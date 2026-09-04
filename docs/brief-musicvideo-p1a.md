# ブリーフ: 音楽ビデオ書き出し P1a — レンダラと連番 PNG

対象 worktree: `C:\home\mimageviewer-musicvideo`（ブランチ `musicvideo`）
正本: [docs/music-video-export-plan.md](music-video-export-plan.md)。**着手前に §1.4 / §3 / §4 / §5 を読むこと。**

P1 を 2 つに割った前半。**P1a では FFmpeg を一切使わない**（MP4 化は P1b で本体パッケージ側に作る）。
`vendor/` を用意していない worktree でビルドが通ることが要件。

---

## 0. 絶対に守ること

1. **bare `cargo build` / `cargo test` を打たない。** 本体 `mimageviewer` の build.rs が
   `vendor/` の DLL を要求して落ちる。必ず `-p` で対象を絞る（§6）。
2. **`src/`（本体パッケージ）を触らない。** P1a の変更範囲は
   `crates/musicvideo-core/`、`tools/musicvideo_lab/`、`Cargo.toml`（登録済み）、`docs/` のみ。
3. **新しいフォントスタックを立てない。** テキストは `comic-core` に投げる（正本 D4）。
4. **wgpu を使わない。** P1a は CPU ラスタライザ 1 本（正本 D4）。
5. **Wav2Bar のソースを読まない・移植しない**（GPL-3.0 / 正本 §1.3）。
6. **描き手を 2 つ作らない。** プレビューも書き出しも同じ `render_frame` を通す（正本 D1）。

---

## 1. 作るもの

すでに空の crate 2 つが workspace に登録済み（`Cargo.toml` の members、両方の `Cargo.toml` あり、
`src/` は空）。中身を実装する。

### 1.1 `crates/musicvideo-core`（純ロジック、egui/FFmpeg 非依存）

```
src/lib.rs       再エクスポート
src/model.rs     Project / Output / Layer / LayerKind / Animatable / Keyframe / Fill / NormRect …（serde）
src/anim.rs      Animatable の評価（純関数）
src/scene.rs     Scene / SceneItem / evaluate(&Project, t, &SpectrumFrame, ...) -> Scene
src/spectrum.rs  music-core の SpectrumAnalyzer を t 単調で駆動するラッパ + 時間平滑化
src/text.rs      comic-core グルー（FontSet / layout / 焼き込み）+ 焼き結果キャッシュ
src/raster.rs    Canvas(RGBA8) + render_frame(&Scene, &Assets, &mut Frame)
src/assets.rs    Assets { images, fonts } — デコード済みで外から渡す（crate は画像デコードをしない）
```

#### データモデル（正本 §4 を P1a 用に縮めたもの）

```rust
pub struct Project {
    pub version: u32,
    pub output: Output,          // { width, height, fps }
    pub duration_secs: f64,
    pub layers: Vec<Layer>,      // 先頭が奥
}

pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub visible: Animatable<bool>,
    pub opacity: Animatable<f32>,   // 0.0..=1.0
    pub rect: Animatable<NormRect>, // 0.0..=1.0、原点は左上
    pub kind: LayerKind,
}

pub enum LayerKind {
    Solid { fill: Animatable<Fill> },
    Image { image: ImageId, fit: Fit },
    Spectrum(SpectrumLayer),
    ProgressBar(ProgressBarLayer),
    TimeText(TimeTextLayer),
}
```

- `Fill` は **P1a では `Fill::Solid(Rgba8)` のみ**。勾配のバリアントは足さない（P2）。
- `SpectrumLayer`: `{ band_count, gap_ratio, min_height_px, thickness, fill }`。
  形状は**直線バーのみ**（円形は P2）。周波数軸は `music-core` の半音バンド固定（正本 D5）。
- `ProgressBarLayer`: `{ border_thickness_px, fill, border_fill }`。角丸・グローは P2。
- `TimeTextLayer`: `{ format: TimeFormat, font_key, size_px, align, fill }`。
  `TimeFormat` は `MmSs` と `MmSsOfTotal`（`01:23 / 04:56`）の 2 つで足りる。
- **回転・角丸・グロー・グラデーション・円形・鍵盤は P1a では作らない。** 型にも足さない
  （後から足す方が、使われない枝を抱えるより安全）。

#### `Animatable` の評価（`anim.rs`）— ここは丁寧にテストする

```rust
pub enum Animatable<T> { Const(T), Track(Track<T>) }
pub struct Track<T> { keys: Vec<Keyframe<T>> }   // t 昇順が不変条件。コンストラクタでソート+検証
pub struct Keyframe<T> { pub t: f64, pub value: T, pub interp: Interp }
pub enum Interp { Step, Linear, EaseIn, EaseOut, EaseInOut }
```

規則:

- キー 0 個の `Track` は**構築できない**（`Track::new` が `Result`、または `Animatable::Const` へ倒す）。
  「空なら既定値」という無言の枝を作らない。
- 最初のキーより前は最初の値、最後のキーより後は最後の値（外挿しない）。
- `bool` と `ImageId` は `interp` の値によらず **Step 意味論**。型で表現できるならそうする。
- 同一 `t` に複数キーがある場合は**後のキーが勝つ**（ステップ切り替えの表現に使える）。
- イージングは正規化した `u∈[0,1]` に対する関数として単体テストする。

P1a のラボ UI はキーフレーム編集を出さないが、**評価ロジックとテストはここで完成させる**
（後段の P3 が UI だけで済むように）。

#### スペクトラム（`spectrum.rs`）— 決定性の要（正本 D3）

```rust
pub struct SpectrumSource { /* music_core::SpectrumAnalyzer + 平滑化状態 + last_t */ }

impl SpectrumSource {
    pub fn new(sample_rate: u32, band_count: usize, smoothing: Smoothing) -> Self;
    /// t は単調非減少で呼ぶ契約。減少や大きな飛びを検出したら内部状態を捨てて作り直す。
    pub fn frame_at(&mut self, pcm: &[f32], t_secs: f64) -> &[f32];
    pub fn reset(&mut self);
}
```

- `music_core::SpectrumAnalyzer` は多重解像度窓の内部状態（`last_center_frame`）を持ち、
  **呼び出し順に依存する**。書き出しは t 昇順で 1 本の `SpectrumSource` を回す。
- `Smoothing::Average { factor }` は指数移動平均 `v = factor*prev + (1-factor)*now`。
  既定 `factor = 0.7`。平滑化も t 昇順の逐次適用が前提なので、リセット時に一緒に捨てる。
- バンド数は `music-core` の上限 128 を超えられない。超える指定は構築時に弾く（黙って clamp しない）。

#### テキスト（`text.rs`）— `comic-core` に投げる

- `comic_core::FontSet` で TTF/OTF/TTC をロードし、`layout_text` / `layout_text_wrapped` で
  レイアウトして RGBA8 へ焼く。`raster.rs` の既存関数で焼けるならそれを使い、足りなければ
  **`comic-core` に手を入れるのではなく `musicvideo-core` 側で薄く組む**
  （`comic-core` は本体のテキスト注釈が使っているので、P1a で API を壊さない）。
- 焼いた結果は **(文字列, フォント, サイズ, 色) をキーにキャッシュ**する。時間表示は `mm:ss` なら
  1 秒に 1 回しか変わらないので、毎フレームのレイアウトを走らせないこと。

#### 合成（`raster.rs`）

- 出力は不透明な RGBA8（`Frame { width, height, rgba: Vec<u8> }`）。背景は黒で初期化。
- レイヤを z 順に合成。レイヤ内の走査は `rayon` で並列化してよいが、**結果は決定的**であること。
- `NormRect` → ピクセル矩形の丸め規則を 1 箇所に集約し、テストで固定する
  （ここがブレると P1b で「プレビューと出力が 1px ずれる」が出る）。

### 1.2 `tools/musicvideo_lab`（eframe GUI）

- 音声を開く: `symphonia` でデコード → **インターリーブ stereo f32 / 48kHz**
  （必要なら `music_core::resample_linear_stereo`）。`tools/music_lab/src/main.rs` の
  デコード部分が参考になる。
- 背景画像を開く: `image` crate で RGBA8 へ。
- **既定プロジェクトを 1 つ用意する**: 1920x1080 / 60fps、背景画像 1 枚、
  進捗バー（x60 y1000 / 1800x50 / 白 / 枠 2px）、右下の時間表示（50px）、直線スペクトラム。
  起動して音声と画像を選んだだけで「それらしい絵」が出る状態にする（正本 §1.4 が寸法の出典）。
- プレビュー: 時刻スライダ → `render_frame` → egui テクスチャとして表示。
  **プレビュー専用の描画経路を書かない。** 再生（音付き）は P2。
- 連番 PNG 書き出し: 出力先フォルダ選択、進捗、キャンセル。
- **速度計測**: 書き出し中に 1 フレームあたりの合成時間を集計し、終了時に
  「平均 ms / p95 ms / 合成のみの実時間比」を表示・ログ出力する。これが P1 の判断材料
  （正本 D4: 実時間の 2 倍に届かなければ GPU 化を検討）。

---

## 2. 再利用するもの（書き直さない）

| 用途 | 使うもの |
| --- | --- |
| 日本語テキスト（シェーピング・グリフ・袋文字・影・グロー） | `comic_core::{FontSet, layout_text, layout_text_wrapped, RgbaOverlay, ...}` |
| スペクトラム解析 | `music_core::{SpectrumAnalyzer, spectrum_analysis_from_stereo_window}` |
| リサンプル | `music_core::resample_linear_stereo` |
| 音声デコード（ラボのみ） | `symphonia`（`tools/music_lab` と同じ構成） |

---

## 3. テスト（`cargo test -p musicvideo-core` で走ること）

必須:

1. `Animatable` 評価 — キー 1 個 / 複数、最初より前・最後より後、同一 t 重複、
   `Step` と `Linear` の差、`bool` が Step 意味論であること、各イージングの端点（u=0,1）。
2. `Track` の不変条件 — 未ソート入力を与えたときの扱い（ソートするか拒否するか）を
   テストで固定する。空キーが構築できないこと。
3. `NormRect` → ピクセル矩形の丸め（境界値: 0.0 / 1.0 / 幅が 0 になる場合）。
4. `SpectrumSource` — 同じ t 列を 2 回流したら同一結果。t を戻したらリセットが効くこと。
   band_count が 128 を超える指定を弾くこと。
5. `render_frame` の決定性 — 同じ `Scene` を 2 回描いてバイト一致。
6. `render_frame` の位置 — 小さいキャンバス（例 64x36）で単色レイヤを置き、
   期待するピクセルが期待する色になっていること（丸め規則の回帰テスト）。

---

## 4. スコープ外（P1a では作らない）

グロー / グラデーション / 円形スペクトラム / 鍵盤表示 / 回転 / 角丸 / キーフレーム編集 UI /
音付きプレビュー / MP4 書き出し / 本体統合。

---

## 5. 完了条件

- `cargo test -p musicvideo-core` が緑（§3 の項目を含む）。
- `cargo check -p musicvideo_lab` が通る。
- `cargo fmt` 済み（リポジトリは 100% fmt 済みを保つ。pre-commit フックが番人）。
- ラボを起動して音声＋画像を選ぶと既定プロジェクトの絵が出て、時刻スライダで
  スペクトラムと時間表示と進捗バーが動く。
- 連番 PNG が書き出せ、終了時に合成の平均 / p95 / 実時間比が表示される。
- **実測値をこのブリーフの末尾に追記して報告する**（P1b で GPU 化を判断する材料）。

## 6. 検証コマンド

```
cargo test -p musicvideo-core
cargo check -p musicvideo_lab
cargo fmt
```

`cargo build` / `cargo test` を `-p` 無しで打つと本体 build.rs が `vendor/` を要求して落ちる。
必ず対象を絞ること。

---

## 7. 実測値（実装後に追記する）

- 2026-09-04、release profile、GUI なしの既定シーン（背景画像・直線スペクトラム・進捗バー・
  時間表示）で計測。ウォームアップ 10 フレーム後に **600 フレーム / 1920x1080 / 60fps** を
  合成し、**平均 1.581 ms/frame / p95 1.931 ms/frame / 実時間比 10.545x**。
  CPU は **Intel(R) Core(TM) Ultra 9 285**（実行環境から見える論理プロセッサ 24）。
  コマンドは `cargo run -p musicvideo_lab --release -- --benchmark 600`。P1 の判断対象である
  「合成のみ」に合わせ、PNG 圧縮とスペクトラム解析の時間は含めていない。

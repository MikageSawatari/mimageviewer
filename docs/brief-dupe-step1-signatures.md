# ブリーフ: 重複検出 Step 1 — 正準プロキシと署名アルゴリズム (純ロジック + 計測 bin)

対象: 重複画像検出 Phase 1 の Step 1。
実装 = Codex Sol / レビュー・検収 = ClaudeCode / 実データ計測 = 利用者。

正本: [docs/duplicate-detection-plan.md](duplicate-detection-plan.md)。
着手前に **§13 (Codex レビュー結果) と §14 (追加検討)** を必ず読むこと。
§2〜§7 は保留中の記述で、**そのまま実装してはいけない**。

作業ツリー: **`C:\home\mimageviewer-dupe`** (branch `duplicate-detection`)。
master の作業ツリーでは作業しないこと (別セッションが並行して master を触っている)。
着手前に `git log --oneline -3` で HEAD を確認する。

---

## 0. この Step の位置づけ — なぜ先にこれだけ作るのか

Phase 1 全体の目的は「実データにラベルを付けて、**どのハッシュを採用するかを測定で決める**」こと。
だが DB もバックグラウンド索引も UI も作らないと測れない、では着手が重くなりすぎる。

**Step 1 は純ロジックと計測 bin だけを作る。** これだけで、利用者が自分のフォルダに対して
各アルゴリズムの距離分布を出せるようになり、**DB スキーマを書く前に**
「PDQ と pHash と連続値のどれが漫画とダウンロード画像で分離するか」が見え始める。

**Step 1 でやらないこと (明確に範囲外)**:

- `similar.db`、永続化、マイグレーション
- バックグラウンド索引ジョブ、進捗、キャンセル、mtime/size 差分
- ZIP / PDF のページ列挙 (Step 2)
- App / egui への配線、設定項目の追加、UI (Step 4)
- 全件スイープの製品実装 (Step 3。bin 内の総当たりは可)
- 本単位の関係判定、ページ帯 (Phase 2)
- **削除機能 (Phase 1 全体で作らない)**
- 閾値の決め打ち。**この Step では閾値を 1 つも確定させない**

---

## 1. 作るもの

```
src/dupe/mod.rs        公開 API と Algo enum、Sig 型、距離関数
src/dupe/proxy.rs      正準プロキシ生成
src/dupe/pdq.rs        PDQ-256 + quality + 64bit 部分集合
src/dupe/dct_phash.rs  古典 pHash-63
src/dupe/blockhash.rs  blockhash-256
src/dupe/luma.rs       連続値 32x32 と各距離
src/bin/bench_dupe.rs  計測 bin (dev-tools feature)
```

`src/dupe/` は**純ロジック**にする。`App` も egui も I/O もグローバル状態も持ち込まない
(`src/color_search.rs` と同じ立ち位置)。ファイル読み込みは `bench_dupe.rs` 側だけ。

依存追加は不要。DCT は 8x8 / 16x16 と小さいので**自前実装する** (新規クレートを足さない)。

---

## 2. 正準プロキシ (`dupe::proxy`)

すべての署名は**同じプロキシ**から作る。プロキシがずれれば全アルゴリズムが同時にずれるので、
ここが仕様の核心になる。

```rust
pub const PROXY_VERSION: u32 = 1;

pub struct Proxy {
    pub gray64: Box<[u8; 64 * 64]>,
    pub gray32: Box<[u8; 32 * 32]>,
    pub src_width: u32,
    pub src_height: u32,
}

/// 呼び出し側は EXIF orientation を適用済みの RGBA8 を渡すこと。
pub fn build(rgba: &[u8], width: u32, height: u32) -> Proxy;
```

**固定する規則 (すべてテストで固定する)**:

1. **アルファ合成**: 不透明な**白** (255,255,255) の上に合成する。
   透過 PNG が背景色で別物になるのを防ぐため、値を 1 つに決める。
2. **グレースケール**: Rec.601 の `0.299R + 0.587G + 0.114B` を
   **sRGB 符号化値のまま**適用する (線形化しない)。f32 で計算し、最後に丸める。
   線形光でやると数値が変わるので、どちらかに決めてテストで固定する。
3. **縮小**: 出力画素ごとに、対応する入力矩形の**面積平均** (box average) を f32 で計算する。
   アスペクト比は**保たない** (64x64 へ潰す。pHash / PDQ と同じ)。
   - **`fast_image_resize` を使わないこと。** 実行時 CPU 機能検出で経路が変わり、
     丸めが環境依存になり得る。正準表現には向かない。20 行程度の自前実装にする。
   - 入力が 64px 未満の辺を持つ場合も同じ式で動くこと (拡大方向も面積平均で定義する)。
4. **`gray32`** は `gray64` から **2x2 平均**で導出する (厳密な box average になり、
   2 系統の縮小経路を持たずに済む)。
5. `src_width` / `src_height` は**元画像**の画素数 (プロキシの寸法ではない)。

`PROXY_VERSION` は上のどれか 1 つでも変えたら上げる。

---

## 3. 署名

```rust
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Algo { Pdq256, Pdq64, Phash63, Blockhash256, Luma32 }

pub enum Sig {
    Bits(Box<[u8]>),  // ハミング距離で比較
    Luma(Box<[u8]>),  // 連続値。§3.4 の複数指標で比較
}

pub struct Signature { pub algo: Algo, pub sig: Sig, pub quality: u8 }

pub fn compute(algo: Algo, proxy: &Proxy) -> Signature;
pub fn all_algos() -> &'static [Algo];
```

`all_algos()` を用意するのは、bake-off が**アルゴリズムを列挙して回れる**ようにするため。
新しい候補を足すときに bin 側を書き換えなくて済む形にする。

### 3.1 PDQ-256 (`dupe::pdq`)

Meta の PDQ 仕様に沿って実装する。**重要な点**:

- 入力は 64x64 グレースケール (= `proxy.gray64`)
- 16x16 の DCT を、**DC を避けた周波数スロット (1..=16)** から作る。
  ここが計画 §3.2 の誤りだった箇所で、
  **16x16 = 256 係数から DC を引くと 255 なので「255 個の AC から 256bit」は作れない**。
  PDQ は DC 行・DC 列を使わない 16x16 のスロットを取ることで、ちょうど 256 個を得ている。
- 256 個の係数の**中央値**で 2 値化して 256bit
- **quality**: PDQ の定義に従い、64x64 上の隣接画素勾配を量子化して積算したもの。
  これが「視覚的に一様すぎてハッシュが信頼できない画像」を弾くための指標になる
  (計画 §5 の `confidence` を自作する代わりにこれを使う)。0..=100 に正規化して `quality` に入れる。

**Rust の既存クレートは使わない**。`pdqhash` 0.1.1 は 2022 年で `image` 0.23 依存のため、
本体の `image` 0.25 と重複依存になる。どうせ自前のグレースケールバッファを食わせるので、
仕様に沿って実装する方が素直。

### 3.2 `Pdq64` — 前段候補生成用の 64bit

**`Pdq256` の実ビットの部分集合**として定義する。具体的には 16x16 の周波数格子のうち
**左上 8x8 (最も低周波側) に対応する 64bit** を、`Pdq256` と**同じ中央値・同じビット値**で切り出す。

理由: 別々の中央値で 2 値化した独立ハッシュにすると、
**前段で落ちた真の重複を後段が救えない** (計画 §13.1-B)。
部分集合にすれば `d256 <= R` のとき部分集合の距離も必ず `<= R` なので、
同じ R で前段を切っても **recall が構造的に保証される**。この性質はテストで固定する。

### 3.3 pHash-63 (`dupe::dct_phash`) — 比較対象としての古典実装

hydrus / czkawka が使っている古典的な DCT pHash。比較の基準線として実装する。

- 32x32 グレースケール (プロキシの `gray64` から 2x2 平均で導出してよい。経路を明記すること)
- 2D DCT → 左上 8x8 を取り、**DC を除いた 63 係数**を中央値で 2 値化 → **63bit**
- 64bit ではなく 63bit であることをコメントと型 (`u64` の下位 63bit) で明示する。
  ここを 64 と誤ったのが計画の欠陥だった。
- quality は PDQ と同じ勾配ベースの値を流用してよい (`Proxy` から計算できるため)

### 3.4 連続値 Luma-32 (`dupe::luma`)

`proxy.gray32` をそのまま署名にする。**2 値化しないので振幅情報が残り**、
平坦な漫画ページでも濃度差が残る、というのがこの候補の狙い。

距離は 1 本ではなく、次を**別々に**返す:

```rust
pub struct LumaMetrics {
    pub l1: f32,              // 平均絶対差
    pub l1_gain_offset: f32,  // b = a*x + c を最小二乗で当てはめた後の平均絶対差
    pub grad_l1: f32,         // 勾配強度画像どうしの平均絶対差
    pub large_diff_area: f32, // |差| > T の画素の割合 (小ロゴ・局所修正の検出)
    pub aspect_ratio_delta: f32, // 元画像のアスペクト比の差
}
```

- `l1_gain_offset` は**軽いレベル補正・明度違い**を吸収するために要る
- `large_diff_area` は「小さなロゴが足された/消された」を
  **「小面積の大差」**として扱うためのもの。L2 だと局所的な大差を過大評価するので使わない
- `T` は定数として置くが、**この Step では値を確定させない**
  (bin の引数で振れるようにし、実測してから決める)
- `aspect_ratio_delta` は安価で強い足切り条件になる (今回クロップは非対応のため)

### 3.5 blockhash-256 (`dupe::blockhash`)

blockhash.io の仕様に沿った 16x16 ブロック中央値ベースの 256bit。比較対象として実装する。

---

## 4. 距離

```rust
pub fn hamming(a: &Sig, b: &Sig) -> Option<u32>;         // Bits 同士のみ
pub fn luma_metrics(a: &Sig, b: &Sig,
                    a_dims: (u32, u32), b_dims: (u32, u32),
                    large_diff_threshold: u8) -> Option<LumaMetrics>;
```

型が違う組み合わせは `None` を返す。**silent fallback にしない**
(`0` や `u32::MAX` を返して黙って通さない)。

---

## 5. 計測 bin `bench_dupe`

`Cargo.toml` に既存の bench bin と同じ形で追加する
(`required-features = ["dev-tools"]`、`src/bin/bench_dupe.rs`)。

### 5.1 `scan` — フォルダを走査して署名を出す

```
cargo run --release --features dev-tools --bin bench_dupe -- \
    scan --dir <folder> [--recursive] --out signatures.jsonl
```

- 対象は**通常の画像ファイルのみ** (ZIP/PDF は Step 2)
- JPEG は `thumb_loader::pick_dct_scale_num` を使った縮小デコードで構わない。
  **どのスケールでデコードしたかを JSONL に記録すること** (後で影響を測るため)
- EXIF orientation を適用してから `proxy::build` に渡す
- 1 行 1 画像の JSONL: パス、元寸法、ファイルサイズ、拡張子、
  各 `Algo` の署名 (16 進) と quality、デコードスケール
- `rayon` で並列化してよい。**1 枚あたりのデコード + 署名生成時間も記録する**
  (Phase 1 の索引コスト見積りに要る)

### 5.2 `pairs` — 候補プールを作る (計画 §14.3 の「和集合」)

```
cargo run ... -- pairs --in signatures.jsonl --out pairs.jsonl \
    [--max-pairs N] [--loose]
```

- **全アルゴリズムの和集合**で候補を作る。1 つのアルゴリズムだけで候補を出さない
  (出すと bake-off がそのアルゴリズムに有利に歪む。計画 §14.3)
- 閾値は**意図的に緩く**取る。既定値は「緩め」として置き、引数で振れるようにする
- 出力は 1 行 1 ペア: 両者のパス、**各アルゴリズムの距離すべて**
  (どれか 1 つでも閾値内なら出す)。後からどの閾値でも切り直せる形にする
- 総当たりで構わない (bin なので。製品側の実装は Step 3)

### 5.3 `synth` — 既知ペアで絶対 recall を押さえる

```
cargo run ... -- synth --dir <folder> --out synth.jsonl [--limit N]
```

各画像から**既知の重複**を合成し、各アルゴリズムの距離を出す:

- 縮小 50% / 75% / 25%
- JPEG 再圧縮 q=95 / 85 / 70
- PNG → JPEG 変換
- 右下に小さなロゴを合成 (画像面積の 1% 程度と 4% 程度の 2 水準)
- わずかな明度・コントラスト変更 (gain 1.05 / offset +8)

出力は「変換の種類 × アルゴリズム × 距離」。
**同時に、無関係なペア (別画像同士) の距離分布も出す。**
この 2 つの分布が分離するかどうかが、そのアルゴリズムの採否を決める。

### 5.4 `report` — 分布をまとめる

```
cargo run ... -- report --synth synth.jsonl [--pairs pairs.jsonl] --out report.md
```

アルゴリズムごとに、変換種別ごとの距離の分位点 (p50 / p90 / p99 / max) と、
無関係ペアの分位点 (p0.01 / p0.1 / p1 / min) を並べた Markdown を出す。
**「どこで分離しているか / していないか」が読み取れる形**にすること。
閾値の推奨値をこの bin が決めてはいけない (判断は人がやる)。

---

## 6. テスト

`cargo test -p mimageviewer --lib dupe::` で走る単体テストとして書く。

**必須**:

1. **決定性**: 同じ RGBA 入力から 2 回作った `Proxy` と全 `Signature` が完全一致する。
2. **`Pdq64` の部分集合性**: ランダムな `Proxy` 対を多数作り、
   **`d256 <= R` なら必ず `d64 <= R`** が成り立つ (R を 0..=32 で振る)。
   これが破れると前段候補生成の recall 保証が消えるので、最重要のテスト。
3. **ビット幅**: `Pdq256` = 256bit、`Pdq64` = 64bit、`Phash63` = **63bit**、
   `Blockhash256` = 256bit。`Phash63` が 64 になっていないこと。
4. **quality**: 一様な灰色 / 真っ白 / 微小ノイズだけの画像は quality が低く、
   テクスチャのある合成画像は高い。**具体的な閾値は assert しない**
   (順序関係だけを固定する。閾値は実測で決めるため)。
5. **合成変換の順序関係**: 合成画像 A と、その 50% 縮小版 A' と、無関係な合成画像 B について、
   全アルゴリズムで `d(A, A') < d(A, B)` が成り立つ。
   **絶対値は assert しない** (閾値未確定のため)。
6. **アルファ合成**: 同じ絵柄で背景アルファだけ違う 2 枚が、白背景合成後に同じ署名になる。
7. **距離関数の型不一致**: `Bits` と `Luma` を渡すと `None` が返る (0 や MAX ではない)。
8. **`l1_gain_offset`**: gain/offset だけ変えた画像対で `l1` より `l1_gain_offset` が
   小さくなる。
9. **`large_diff_area`**: 小さなロゴを足した対で、`l1` はほぼ変わらないが
   `large_diff_area` は明確に増える。

**書かないテスト**: 閾値の absolute assert。この Step では 1 つも決めない。

---

## 7. コミット粒度

以下を**別コミット**にすること (ClaudeCode がレビューしやすくするため):

1. `dupe::proxy` + そのテスト
2. `dupe::pdq` (+ `Pdq64` の部分集合性テスト)
3. `dupe::dct_phash` + `dupe::blockhash`
4. `dupe::luma` + 距離関数
5. `bench_dupe` bin + `Cargo.toml`

各コミット時点で `cargo test -p mimageviewer --lib dupe::` が緑であること。

---

## 8. 完了条件

- `cargo fmt` 済み、`cargo check -p mimageviewer --bin mimageviewer-core` が通る
- `cargo test -p mimageviewer --lib dupe::` が緑
- `cargo build --release --features dev-tools --bin bench_dupe` が通る
- 上の 5.1〜5.4 の 4 モードが動き、利用者が自分のフォルダに対して実行できる
- **本体の既存挙動 (`App` / UI / 設定 / DB / 既存コード経路) に一切変更が入っていないこと**。
  `git diff --stat` が `src/dupe/`、`src/bin/bench_dupe.rs`、`Cargo.toml`、
  および `src/lib.rs` の `pub mod dupe;` 1 行に収まっている。
  新規モジュールの宣言行と `Cargo.toml` の bin 定義は、モジュールを作る以上必然なので範囲内。
  禁じているのは**既存の挙動を変えること**であって、配線そのものではない
  (初版のブリーフはここが自己矛盾していた。2026-09-03 修正)

## 9. 判断に迷ったとき

- **閾値を決めない。** この Step の目的は測る道具を作ることであって、
  値を決めることではない。迷ったら定数を bin の引数に出す。
- **silent fallback を作らない。** 計算できない入力は `None` / `Err` にする。
- **正準規則を 2 か所に書かない。** アルファ合成・グレースケール式・縮小規則は
  `dupe::proxy` だけが持つ。各アルゴリズムが独自に前処理しない。
- 仕様上どうしても決まらない点があれば、**実装で埋めずにブリーフへの質問として残す**。

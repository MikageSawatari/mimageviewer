# Codex ブリーフ — SNS 分割書き出し P1 (幾何 + ユニットテスト)

正本: [docs/sns-split-export-plan.md](../sns-split-export-plan.md)。**着手前に §3.1 / §4.1 / §4.2 / §5 を読むこと。**

作業ツリー = `C:\home\mimageviewer-snssplit` (branch `sns-split-export`)。
このブリーフの範囲は **P1 のみ**。UI・描画・書き出しには触らない。

---

## 1. 何を作るか

`src/sns_split.rs` を新規作成し、`src/lib.rs` にモジュール宣言を足す。**純ロジックのみ。**
`egui` / `App` / DB / ファイル I/O に依存しない (`export_crop::CropRect` の再利用だけ可)。

1 枚の絵を、X / Instagram のカルーセル投稿用に N 枚 (2〜4) へ切り分けるときの**枠の幾何**を持つ。

---

## 2. 触ってよいファイル

- `src/sns_split.rs` (新規)
- `src/lib.rs` (`mod sns_split;` の宣言 1 行のみ)

**これ以外を変更しない。**特に `src/export_crop.rs` / `src/ui_crop.rs` / `src/export_dialog.rs` は
**この段階では一切触らない** (正本 §3.1 のとおり、既存の切り取りは 30 ファイル超から参照されている)。

---

## 3. API

```rust
pub enum SnsTarget { X, Instagram }
```

| | `frame_aspect()` (枠の 横/縦) | `seam_ratio()` (枠幅に対する隙間比) |
| --- | --- | --- |
| `X` | 0.75 (3:4) | **0.017** |
| `Instagram` | 0.80 (4:5) | **0.0** |

`SnsTarget` に持たせるもの (既存 [`CropAspectMode`](../../src/export_crop.rs:24) の書き方に揃える):

- `const ALL: [Self; 2]`
- `label(self) -> &'static str` — `"X"` / `"Instagram"`
- `stable_key(self) -> &'static str` — `"x"` / `"instagram"` (設定の永続化用)
- `from_stable_key(key: &str) -> Self` — 未知の値は `X` へ倒す
- `frame_aspect(self) -> f32`
- `seam_ratio(self) -> f32`

`seam_ratio` の 0.017 には、**由来をコメントで残すこと**。実測 (2026-09-01) は
PC ブラウザ 1.588 % / iOS アプリ 1.869 % / モバイル Web 2.652 % で、隙間の絶対値は環境ごとに
違う (Web 5.33 CSS px / iOS アプリ 4.00 CSS px)。1.7 % は PC ブラウザと iOS アプリの誤差が
両方 0.4 CSS px 以内に収まる値。詳細は正本 §2.1。

```rust
pub const MIN_COUNT: u8 = 2;
pub const MAX_COUNT: u8 = 4;

pub struct SnsSplitLayout {
    pub target: SnsTarget,
    pub count: u8,       // MIN_COUNT..=MAX_COUNT
    pub group: CropRect, // source image coordinate
}
```

メソッド:

```rust
/// グループ矩形の比率 (横/縦) = frame_aspect * (N + (N-1) * seam_ratio)
pub fn group_aspect(target: SnsTarget, count: u8) -> f32;

/// 画像全体に対して最大サイズ・中央でグループを作る
pub fn centered_max(target: SnsTarget, count: u8, image_size: [usize; 2]) -> Self;

/// 枠 N 個の source 矩形。描画と書き出しの両方がこれを使う (§4 の契約)
pub fn frames(&self) -> Vec<CropRect>;

/// frames() の外接矩形。暗転マスクの描画に使う
pub fn frames_extent(&self) -> CropRect;

/// 中心を保ったまま差し替える。比率が変わるので作り直しになる
pub fn with_target(&self, target: SnsTarget, image_size: [usize; 2]) -> Self;
pub fn with_count(&self, count: u8, image_size: [usize; 2]) -> Self;

/// 画像内へ収める。比率は維持する
pub fn clamped(&self, image_size: [usize; 2]) -> Self;
```

`count` は常に `MIN_COUNT..=MAX_COUNT` へ clamp する。`image_size` の 0 は 1 として扱う
(既存 `CropRect::full` と同じ)。

---

## 4. 幾何の契約 — ここが本題

枠幅 `f`、枚数 `N`、隙間比 `r` として

```
Wg = f * (N + (N-1) * r)
Hg = f / frame_aspect
グループの比率 = Wg / Hg = frame_aspect * (N + (N-1) * r)
```

**グループ矩形は比率が一意に決まる 1 枚の矩形**になる (X / 4 枚なら
`0.75 * (4 + 3*0.017) = 3.0383`)。これにより既存の比率固定リサイズがそのまま使える。

### 4.1 `frames()` は整数ピクセルで、全枠を厳密に同じ寸法にする

**これが一番重要な要件。**X は N 枚を同じ表示幅で並べるので、枠の幅が 1px でも違うと
拡縮率が枠ごとにずれ、継ぎ目が合わなくなる。

したがって `frames()` は次の順で作る。

```
w_px  = floor(group.width() / (N + (N-1) * r))     // 全枠共通の幅 (整数)
h_px  = round(w_px / frame_aspect)                 // 全枠共通の高さ (整数)
step  = round(w_px * (1 + r))                      // 枠の左端どうしの間隔 (整数)
x0    = round(group.min_x)
y0    = round(group.min_y)
frame k = (x0 + k*step, y0, x0 + k*step + w_px, y0 + h_px)
```

- **全枠の幅・高さは厳密に一致する** (「1px 以内」ではなく完全一致)
- 隣接する枠の間隔は全て `step - w_px` で一定
- `r = 0` (Instagram) のとき `step == w_px` となり、枠が隙間なく連続する
- `(N-1)*step + w_px` は `group.width()` と厳密には一致しないことがある。**それでよい**。
  グループ矩形は利用者が掴むハンドルにすぎず、**出力と描画の正は `frames()`** とする
- `frames_extent()` は `frames()` の外接矩形を返す。描画の暗転マスクはこれを使うので、
  「枠の外側が暗い」見た目と実際の出力が一致する
- `frames()` の全枠が `image_size` の内側に収まること。収まらない場合は
  `clamped()` を通した上で計算する (`centered_max` / `with_*` は内部で clamp 済みを返す)

### 4.2 clamp

`clamped()` は比率を維持したまま画像内へ収める。

- グループが画像より大きい場合は、比率を保ったまま縮小する
- はみ出しているだけの場合は平行移動で戻す (縮小しない)
- 縮小・移動どちらの場合も、結果の比率は `group_aspect()` と一致する

### 4.3 `with_count` / `with_target`

中心 (`group.center()` 相当) を保ったまま新しい比率の矩形を作り、`clamped()` を通す。
**面積ではなく高さを保つ**方針にする (枚数を増やしたときに横へ伸びる方が直感的なため)。
高さを保てない (画像から出る) 場合だけ縮小する。

---

## 5. ユニットテスト (同ファイル内 `#[cfg(test)]`)

正本 §4.2 の不変条件をすべて押さえる。最低限これらを書くこと。

1. `group_aspect` が X/2, X/3, X/4, Instagram/2..4 で期待値になる
   (X/4 = 3.0383… を `assert!((a - 3.0383).abs() < 1e-3)` 程度で)
2. `frames()` の要素数が `count` に一致する
3. **全枠の幅が完全一致、高さも完全一致** (2/3/4 枚 × X/Instagram の全組み合わせ)
4. 隣接する枠の間隔が一定で、`step - w_px` に等しい
5. `Instagram` (r=0) では枠が隙間なく連続する (`frame[k].max_x == frame[k+1].min_x`)
6. `X` では隙間が正で、`gap / w_px` が 0.017 に近い (整数丸めがあるので 1px 以内の許容)
7. 枠幅 1536px 相当のグループで、X の継ぎ目に落ちる帯が **26px 前後** (25〜27) になる
8. `centered_max` の結果が画像内に収まり、比率が `group_aspect()` と一致する
9. `with_count` / `with_target` が中心をほぼ保ち、結果が画像内に収まり比率も正しい
10. `count` が 0 / 1 / 5 / 255 でも `MIN_COUNT..=MAX_COUNT` へ clamp される
11. 極端に細長い画像 (例 100x4000) で `centered_max` しても panic せず、枠が画像内に収まる
12. `from_stable_key` が未知キーで `X` を返し、`stable_key` → `from_stable_key` が往復する

数値比較は f32 なので、完全一致を期待する箇所 (幅・高さ) は**整数化した値**で比較すること。

---

## 6. 守ること

- **コメントは日本語**。周囲のモジュール (`src/export_crop.rs` の書き方) に合わせる
- `cargo fmt` をワークスペース全体にかけてから終わる (リポジトリは常に 100 % fmt 済み)
- `cargo test -p mimageviewer --lib sns_split` が緑であること
- `cargo check -p mimageviewer --bin mimageviewer-core` が通ること
- **正本に書かれていない機能を足さない。**特に 2x2 グリッド配置、縦並び、枠の個別移動、
  隙間の数値設定は**非対象** (正本 §5)。思いついても実装しない
- 設定 (`Settings`) への項目追加は **P2 の範囲**。ここではやらない
- コミットしない (レビュー後にこちらでまとめる)

## 7. 終わったら報告すること

- 追加した公開 API の一覧
- 4.1 の「全枠が厳密に同じ寸法」をどう保証したか
- 書いたテストの一覧と、`cargo test -p mimageviewer --lib sns_split` の結果
- 正本と食い違った点、判断に迷った点があれば明記 (勝手に正本を書き換えないこと)

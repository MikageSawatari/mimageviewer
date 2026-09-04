# ブリーフ: 音楽ビデオ書き出し P2a — 見栄え（グロー・グラデーション・円形）

対象 worktree: `C:\home\mimageviewer-musicvideo`（ブランチ `musicvideo`）
正本: [docs/music-video-export-plan.md](music-video-export-plan.md)。前段: P1a / P1b 完了済み。

P2 を 2 つに割った前半。**P2a は `crates/musicvideo-core` の中で完結し、本体 `src/` を触らない。**
鍵盤表示だけは本体の音楽ビューとロジックを共有する必要があるので P2b に分ける。

---

## 0. 絶対に守ること

1. **`src/` を触らない**（P2a の範囲は `crates/musicvideo-core/` と `tools/musicvideo_lab/` のみ）。
   ただし `src/video/mvexport/` が新しいレイヤ種別を素通しできるかは確認すること
   （素通しできない作りになっていたら報告する。直すのは P2b でよい）。
2. **`comic-core` / `music-core` を変更しない。** グローは comic-core の
   `TextGlowStyle` / `draw_layout_soft_mask` と**同じ意味論**にするが、実装は
   `musicvideo-core` 側でレイヤのアルファに対して一般化する。
3. **描き手を 2 つ作らない。** 円形も直線も同じバンド列・同じ `Fill`・同じグローを通る。
4. **決定性を壊さない**（正本 D3）。同じ `Scene` を 2 回描いてバイト一致するテストは維持する。
5. `cargo fmt` を通す。commit は不要（こちらで行う）。

---

## 1. 作るもの

### 1.1 `Fill` の拡張

```rust
pub enum Fill {
    Solid(Rgba8),
    LinearGradient { stops: Vec<GradientStop>, angle_deg: f32 },
    RadialGradient { stops: Vec<GradientStop>, center: (f32, f32), radius: f32 },
    BandGradient  { stops: Vec<GradientStop> },   // スペクトラムのバンド位置で色が変わる
}
pub struct GradientStop { pub position: f32, pub color: Rgba8 }  // position は 0.0..=1.0
```

- ストップは**位置の昇順が不変条件**。空を構築できないこと（`Track` と同じ扱い）。
- 補間は sRGB 空間の線形補間でよい（凝らない）。境界（ストップ 1 個、同一位置の重複）を
  テストで固定する。
- `Animatable<Fill>` の補間は、**ストップ数と位置が一致する場合だけ色を補間**し、
  一致しない場合は Step 扱いにする。無言で片方に寄せない。

### 1.2 レイヤ共通のグロー

```rust
pub struct Glow { pub color: Rgba8, pub radius_px: f32, pub spread_px: f32 }
// Layer に glow: Animatable<Option<Glow>> を足す
```

CSS の `box-shadow: 0 0 <radius> <color>` と同じ意味論（正本 D4）。
実装は「そのレイヤのアルファを spread 分だけ膨張 → radius でぼかす → 指定色で下に置く → 本体を描く」。

- **分離可能なぼかし**（横→縦）で実装する。半径 0 は完全な no-op（早期 return）。
- **レイヤの矩形の外側にはみ出す**ので、描画範囲は `rect` を `spread + radius` 分だけ広げる。
  はみ出しがフレーム外なら切る。
- **速度予算: グロー込みでも合成の平均を 3.5ms 以内に収めること。**
  P1b の実測で 1 フレームは 合成 1.665ms + 色変換 4.117ms + エンコード 0.507ms = 約 6.3ms、
  実時間比 2.4 倍。合成が 3.5ms を超えると 2 倍を割り始める。§1.4 のプロジェクトは
  グローを 3 箇所（進捗バー・円形スペアナ・時間テキスト）使うので、そこで測ること。

### 1.3 円形スペクトラム

```rust
pub enum SpectrumShape {
    Bars   { .. },                                   // 既存
    Radial { radius_px: f32, thickness_px: f32,
             start_angle_deg: f32, sweep_deg: f32,   // 既定 -90 / 360
             direction: Clockwise | CounterClockwise },
}
```

- **バンド列の生成は直線と共有**する（同じ `SpectrumSource` の出力を座標変換するだけ）。
- §1.4 の実プロジェクトは「点の集合」に見える描画（200 点 × 太さ 4px、半径 380px）。
  点数はバンド数と独立に指定できること（バンド数 128 上限とは別軸）。
  バンド数より点数が多い場合の補間規則を決めてテストで固定する。
- **アンチエイリアスを入れること。** 円周上の細い棒はエイリアスが目立つ。
  1920x1080 の実寸で見て階段が見えないこと。

### 1.4 角丸・枠・回転

§1.4 の実プロジェクトが使っている:

- `corner_radius`（進捗バーの `border-radius: 20px`）
- 枠（`border thickness: 2px`）— `ProgressBarLayer` に既にあるなら流用
- `rotation`（実プロジェクトでは全部 0 だが、モデルにはある）

角丸と枠にも AA を入れる。回転は最後に足してよい（優先度は角丸 > 枠 > 回転）。

### 1.5 ラボ UI

新しいプロパティ（グラデーションのストップ編集・グロー・円形の各値・角丸）を
プロパティパネルで触れるようにする。**キーフレーム編集はまだ出さない**（P3）。

既定プロジェクトを **§1.4 の再現**に寄せる:
背景画像 + 円形スペアナ（緑・グロー）+ 進捗バー（白・角丸・グロー）+ 時間テキスト（グロー）。

---

## 2. 完了条件

1. `cargo test -p musicvideo-core` が緑（新規テストを含む）。
2. `cargo check -p musicvideo_lab` が通る。
3. **§1.4 のプロジェクトを再現できる**。ラボで組んで 1 枚書き出し、
   ユーザーが元の Wav2Bar 出力と並べて見比べられる状態にする。
   ⚠️ **スペクトラムの周波数軸は一致対象外**（正本 D5。mIV の半音軸で良い）。
4. **合成の平均が 3.5ms 以内**（§1.4 相当のシーン、1920x1080）。ベンチに
   グロー 3 箇所 + 円形 200 点のシーンを足して測り、§4 に記録する。
5. 決定性のテストが維持されていること。
6. `cargo fmt --check` が通る。

## 3. 検証コマンド

```
cargo test -p musicvideo-core
cargo check -p musicvideo_lab
cargo fmt
```

`cargo test -p mimageviewer` 系を回す場合は、`vendor/ffmpeg/bin/*.dll` を `target/debug/` と
`target/debug/deps/` にコピーしておくこと（無いと `STATUS_DLL_NOT_FOUND` でテストが 1 件も
走らずに落ちる）。

## 4. 実測値（実装後に追記する）

（§1.4 相当のシーンでの 解析 ms / 合成 ms / 合計 ms・p95・実時間比を書く。
グロー有りと無しの差も分けて書くこと）

# ブリーフ: 音楽ビデオ書き出し P2b — 鍵盤表示を本体と共有する

対象 worktree: `C:\home\mimageviewer-musicvideo`（ブランチ `musicvideo`）
正本: [docs/music-video-export-plan.md](music-video-export-plan.md)（D6 / Q5）。前段: P1a / P1b / P2a 完了。

**これまでで唯一、本体の出荷済み機能に手を入れる increment。** 慎重に進める。

---

## 0. この increment の性質

書き出しに鍵盤表示を足すだけなら `musicvideo-core` の中で完結できる。しかしそれをすると
**同じ「鍵盤」という概念の描き手が 2 つになる**（本体の音楽ビューと書き出し）。片方だけ直る
バグ、片方だけ変わる見た目が必ず出る。正本 D6 はこれを禁じている。

よって本 increment の本体は「鍵盤を描けるようにする」ことではなく、
**`draw_pitch_keyboard` の判断を純関数として括り出し、本体と書き出しの両方がそれを呼ぶ形にする**
ことである。**受け入れ基準の半分は「本体の音楽ビューの見た目が変わらないこと」**。

---

## 1. 絶対に守ること

1. **抽出は挙動不変で行う。** 算術・定数・条件分岐を「ついでに改善」しない。
   気になる点を見つけたら、直さずに報告する。
2. **`music-core` は追加のみ。既存の公開 API を変えない**（本体と `musicvideo_lab` が依存している）。
3. `src/ui_music_spectrum.rs` 以外の本体ファイルは、必要が無ければ触らない。
4. **`tools/music_lab` も `music-core` を使っている。** 壊していないか確認すること
   （`cargo check -p music_lab`）。
5. `cargo fmt` を通す。commit は不要（こちらで行う）。

---

## 2. やること

### 2.1 `music-core` に鍵盤レイアウトの純ロジックを足す

`src/ui_music_spectrum.rs` の `draw_pitch_keyboard`（および `draw_bass_marker` が使っている判断）
から、**egui に依存しない計算だけ**を新しいモジュール（例 `music-core/src/keyboard.rs`）へ移す。

出す形の目安:

```rust
pub struct KeyboardLayout { pub keys: Vec<KeyPlacement> }
pub struct KeyPlacement {
    pub midi: u8,
    pub black: bool,
    pub x0: f32, pub x1: f32,   // 与えた幅に対する位置
    pub level: f32,             // notes[] 由来の表示強度（0.0..=1.0）
}
pub fn layout_keyboard(notes: &[f32], note_min_midi: u8, width: f32, /* 既存の調整値 */) -> KeyboardLayout;
```

- 実際の引数・戻り値は既存コードの形に合わせてよい。**egui の型（`Rect` / `Color32` / `Painter`）を
  `music-core` に持ち込まないこと**が要件。
- 色そのものを返すか強度だけを返すかは、既存コードの構造に素直な方を選ぶ。
  ただし**本体と書き出しで同じ結果になる**ことが条件。

### 2.2 本体の `draw_pitch_keyboard` を、その関数を呼ぶだけにする

- 計算は `music-core` 側、描画（`Painter` への `rect_filled` 等）だけが本体に残る形。
- **ピクセル出力が変わらないこと。** 丸めのタイミングを変えない（`round` を移動させると 1px ずれる）。

### 2.3 `musicvideo-core` に `PitchKeyboard` レイヤを足す

- 同じ `layout_keyboard` を呼び、CPU ラスタライザで描く。
- `SpectrumSource` は現在バンド列だけを返している。鍵盤には
  `SpectrumAnalysis::notes` と `note_min_midi` が要るので、**同じ 1 回の解析結果から
  両方を取り出せるようにする**（解析を 2 回走らせない）。
- グロー・`Fill`・不透明度など P2a のレイヤ共通プロパティが効くこと。

### 2.4 既定シーンとラボ UI

`musicvideo-core::preset` の既定シーンに鍵盤レイヤを足すかどうかは任意（§1.4 には無い）。
ラボのプロパティパネルからは追加・編集できるようにすること。

---

## 3. 「見た目が変わらない」ことの担保

コード読みだけで済ませない。次の 3 つを揃えること。

1. **抽出した計算に対する固定入力のテスト**（`music-core` 側）。
   ⚠️ **そのテストが本当に効くことを確かめる**: 新しい関数の定数や式をわざと 1 つ変えて
   テストが落ちることを確認し、確認したと報告に書く。落ちないテストは書いた意味がない。
2. **本体側の呼び出しが、抽出前と同じ引数・同じ順序で描いていること**を差分で示す。
3. **実機確認用のビルド**: `.\scripts\build-dev.ps1` が通ること。
   （起動して目視するのはユーザーの担当。エージェントは起動しない。）

---

## 4. 完了条件

1. `cargo test -p music-core` / `cargo test -p musicvideo-core` が緑（§3-1 の新規テストを含む）。
2. `cargo check -p mimageviewer --bin mimageviewer-core` / `cargo check -p music_lab` /
   `cargo check -p musicvideo_lab` / `cargo check -p mimageviewer --features dev-tools --bin mvexport_cli`
   がすべて通る。
3. 書き出しに鍵盤レイヤを出せる（ラボで確認できる PNG を 1 枚出す）。
4. `.\scripts\build-dev.ps1` が成功する。
5. `cargo fmt --check` が通る。
6. **抽出中に見つけた「直したくなった点」を、直さずに報告する**（後で別途判断する）。

## 5. 検証コマンド

```
cargo test -p music-core
cargo test -p musicvideo-core
cargo check -p mimageviewer --bin mimageviewer-core
cargo check -p music_lab
cargo check -p musicvideo_lab
cargo check -p mimageviewer --features dev-tools --bin mvexport_cli
.\scripts\build-dev.ps1
cargo fmt
```

`cargo test -p mimageviewer` 系を回す場合は `vendor/ffmpeg/bin/*.dll` を `target/debug/` と
`target/debug/deps/` へコピーしておくこと。

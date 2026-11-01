# ブリーフ: 音楽ビデオ書き出し P3a — 編集 UI の置き場・保存・再生プレビュー

対象 worktree: `C:\home\mimageviewer-musicvideo`（ブランチ `musicvideo`）
正本: [docs/music-video-export-plan.md](music-video-export-plan.md)。前段: P1a / P1b / P2a / P2b / P2c 完了。

P3 を 2 つに割った前半。**キーフレーム編集そのものは P3b。** P3a は「実際に作業できる状態」に
するための土台を置く。

---

## 0. なぜ先に UI の置き場を決めるのか

いま編集 UI は `tools/musicvideo_lab/src/app.rs`（約 560 行）にある。P3b でキーフレームの
タイムライン編集が入ると、ここは数倍に育つ。そして **P4 では同じ編集 UI を mIV 本体へ
載せる**（正本 P4）。このまま進めると、**同じエディタが lab と本体に 2 つできる**。

これは本プロジェクトで一貫して避けてきた失敗（描き手を 2 つ作る）そのものなので、
**UI が大きくなる前のいま、共有クレートへ移す。**

- `crates/musicvideo-ui`（新規）… egui 依存、**eframe / FFmpeg / cpal に依存しない**。
  レイヤ一覧・プロパティパネル・プレビュー描画・（P3b で）タイムラインを持つ。
- `tools/musicvideo_lab` … 上のクレートを載せる薄いシェル（ウィンドウ、ファイル選択、
  音声デコード、再生、PNG/連番書き出し）。
- P4 で mIV 本体が同じ `musicvideo-ui` を埋め込む。

`musicvideo-core` は**純ロジックのまま**（egui を入れない）。

---

## 1. 絶対に守ること

1. **`musicvideo-core` に egui を入れない。** 新しい `musicvideo-ui` が egui 依存を持つ。
2. **`src/`（本体）を触らない。** P3a の範囲は `crates/musicvideo-ui/`（新規）、
   `crates/musicvideo-core/`、`tools/musicvideo_lab/`、`Cargo.toml`、`docs/`。
3. **描画は `render_frame` 一本のまま。** プレビュー専用の描画経路を作らない（正本 D1）。
4. **決定性を壊さない**（正本 D3）。再生プレビューは表示だけで、書き出しの結果に影響しない。
5. `cargo fmt` を通す。commit は不要（こちらで行う）。

---

## 2. やること

### 2.1 `crates/musicvideo-ui` を作り、既存の編集 UI を移す

- 依存は `egui` と `musicvideo-core` のみ（`eframe` を入れない＝本体へ埋め込める形にする）。
- 公開する形の目安:

```rust
pub struct EditorState { /* 選択中レイヤ、パネル開閉など UI 固有の状態 */ }

/// プロジェクトを編集する。呼び出し側は Project の所有者。
pub fn draw_editor(ui: &mut egui::Ui, project: &mut Project, state: &mut EditorState) -> EditorResponse;

/// 合成済みフレームを表示する（テクスチャ管理は呼び出し側）。
pub fn draw_preview(ui: &mut egui::Ui, texture: &egui::TextureHandle, state: &mut EditorState);
```

- **ラボの見た目・操作は変えない**（移設であって作り直しではない）。
- 移設後 `tools/musicvideo_lab/src/app.rs` は「eframe のウィンドウ + ファイル I/O +
  音声再生 + 書き出し + `musicvideo-ui` の呼び出し」だけになるはず。

### 2.2 プロジェクトの保存・読み込み（JSON）

正本 D9。`.mivmv.json`（仮）。`serde` は既に入っている。

- **Q4 の決着**: 素材のパスは、**プロジェクトファイルからの相対パスで保存**する。
  同じドライブに無いなど相対にできない場合は絶対パスで保存する（保存時に決める）。
  読み込み時は、相対パスをプロジェクトファイルの位置から解決する。
- **素材が見つからない場合は明示的に報告する。** 黙って真っ黒を描かない
  （P2a で入れた「解決できない素材の報告」を使う）。どのレイヤのどのパスが無いかを UI に出し、
  差し替えられるようにする。
- `version` フィールドを持つ。**未リリースなので移行コードは書かない**（正本 D9）。
  読めない version は明示的にエラーにする。
- 保存・読み込みのラウンドトリップ（Project → JSON → Project が等しい）をテストする。

### 2.3 音付き再生プレビュー

キーフレームは音に合わせて打つものなので、**音を聴きながら位置を決められないと使えない**。

- `cpal` で再生（`tools/music_lab/src/main.rs` に実績のある構成がある）。ラボ側に置く。
- **音声を時刻の正本にする。** 再生位置から t を取り、`render_frame` で 1 枚描いて表示する。
  映像側の時計を別に持たない。
- 再生・一時停止・シーク（時刻スライダ）。
- **プレビューのスペクトラムは書き出しと厳密一致しない**（解析器が履歴依存で、シークで
  リセットされるため）。これは既知で正しい挙動。連続再生中は一致する。
  この旨を UI のどこかに一言出すか、ドキュメントに書くこと。

### 2.4 鍵盤レイヤの背景を透過できるようにする

現在、鍵盤レイヤは背景を不透明な黒で塗るので、後ろのイラストが隠れる
（本体の音楽ビューは黒パネルの上に描くので、それが正しい挙動として持ち込まれている）。

- **書き出し側でだけ**、背景を `Animatable<Fill>`（既定は透明）にする。
- **本体の音楽ビューの見た目は変えない**（`src/` は触らない、が制約 2）。
  共有しているのは `music-core` のレイアウト計算だけなので、背景の塗りは
  `musicvideo-core` 側の判断で足せるはず。そうなっていない場合は報告すること。

---

## 3. 完了条件

1. `cargo test -p musicvideo-core` / `cargo test -p musicvideo-ui`（あれば）が緑。
2. `cargo check -p musicvideo_lab` / `cargo check -p mimageviewer --features dev-tools --bin mvexport_cli` が通る。
3. **`musicvideo-ui` が `eframe` にも `cpal` にも `ffmpeg` にも依存していないこと**
   （`cargo tree -p musicvideo-ui` で確認して報告する）。P4 で本体へ埋め込めることが要件。
4. ラボでプロジェクトを保存し、ラボを再起動して読み込むと同じ絵が出る。
5. 音を再生しながらプレビューが追従する。シークできる。
6. 鍵盤レイヤの背景を透明にでき、後ろのイラストが見える。
7. `cargo fmt --check` が通る。

## 4. 検証コマンド

```
cargo test -p musicvideo-core
cargo check -p musicvideo-ui
cargo check -p musicvideo_lab
cargo check -p mimageviewer --features dev-tools --bin mvexport_cli
cargo tree -p musicvideo-ui
cargo fmt
```

`cargo test -p mimageviewer` 系を回す場合は `vendor/ffmpeg/bin/*.dll` を `target/debug/` と
`target/debug/deps/` へコピーしておくこと。

## 5. 報告してほしいこと

- `cargo tree -p musicvideo-ui` の出力（依存が意図どおりか）
- 移設で見た目・操作が変わっていないこと
- 保存形式の実例（小さいプロジェクトの JSON 全文）

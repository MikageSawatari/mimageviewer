# ブリーフ: 音楽ビデオ T1 — 文字装飾（comic-core 既存の装飾を接続する）

対象 worktree: `C:\home\mimageviewer-musicvideo`（ブランチ `musicvideo`）
正本: [music-video-export-plan.md](music-video-export-plan.md) の **D15**（範囲・合成の境界・補間・キャッシュ）と §7 の T1 行。
前段: L1〜L3、I1 完了（独立レビュー ACCEPT）。
担当: 実装・テスト = Codex GPT-6.1 Sol / high。検収 = ClaudeCode Opus。独立レビュー = 別の GPT-6.1 Sol / xhigh。

---

## 0. 範囲（ユーザー判断、D15）

**初版は comic-core に既にある装飾だけ**:

- 縁取り（`TextBlock::outline`）と外側の追加縁取り（`extra_outlines`、二重・三重）
- ドロップシャドウ（`shadow`: offset・blur・spread・色）
- エコー（`echo`: offset・枚数・色）
- 背景の角丸ボックス（`background`: 塗り・padding・角丸）
- 字間・行間（`letter_gap` / `line_gap`）

対象外: 太字・斜体（comic-core の layout / raster が参照していない。D15）、comic-core の `TextGlowStyle`
（レイヤー共通グローを使う）、ベベル・エンボス・内側の効果・縁取りのグラデーション・縦書き・カラオケ塗り。

対象レイヤー: **Text / TimeText / Lyrics の 3 種**。同じ文字スタイルの型を共有し、実装を 1 つにする。

---

## 1. 絶対に守ること

1. **`comic-core` を変更しない**（出荷済みの画像注釈と共有）。装飾の描画は comic-core の `bake_overlay` /
   `layout_text` を使い、musicvideo-core に別の縁取り・影の実装を作らない。
2. **合成の境界（D15、独立レビュー P2 対応済みの設計）**:
   - 現行は comic-core の bitmap を白で焼き、**alpha だけ**を使って RGB を `Fill` から作っている
     （`crates/musicvideo-core/src/text.rs` の `TextCache`、`raster.rs` の文字描画）。このまま装飾を繋ぐと影・背景・
     縁取りまで Fill の色になる。
   - **装飾 RGBA**: glyph の塗りを透明にした TextBlock を comic-core で焼く（縁取り・影・エコー・背景が固有色で出る）。
   - **glyph マスク**: 装飾なしで焼いた glyph の coverage（現行と同じ）。これに `Fill` を適用する。
   - 2 枚は**同じレイアウト**（フォント・文字配置・字間・行間・原点）を共有する。字間・行間は両方に効かせる。
   - 合成順は 装飾 RGBA → Fill を適用した glyph。**2 枚を合成してから**レイヤーの opacity と clip を 1 回だけ掛ける
     （2 枚それぞれに掛けると重なりの透明度が変わる）。レイヤー共通グローは両方を合わせた silhouette にかける。
   - bitmap の確保に**装飾の到達範囲**を含める（現行は論理寸法 + 固定 1px）。論理寸法（揃え位置の基準）と描画範囲を
     分ける。comic-core が装飾の到達範囲を計算している箇所（`comic-core/src/raster.rs` 周辺）を使う。
   - 単色 Fill + グローの「mask から単色で作り直す」最適化（`raster.rs`）に、装飾ありのレイヤーを流さない。
3. **装飾なしのプロジェクトは描画結果が変わらない**（無変調のフレームハッシュ維持、D3）。
4. **補間（D15）**: 構造を決める離散値（各装飾の有無、追加縁取りの本数、echo の枚数）が前後のキーで 1 つでも違えば
   **スタイル全体を Step**。すべて一致すれば色と連続値（幅・offset・ぼかし・広がり・padding・角丸・字間・行間）を補間。
   属性ごとに補間と Step を混ぜない。
5. **キャッシュ（D15）**: 現行の `TextCache` は上限のない HashMap。キーに文字スタイル全体を含め、**容量上限つき（LRU、
   バイト数で上限）**にする。補間中のフレームの結果で静的な結果を追い出しにくくする扱いを入れる（方法は任せる。
   根拠を報告）。
6. **既存のプロジェクトファイルが読めること**: ユーザーの実プロジェクト（`.mivmv.json`）に装飾の項目は無い。
   新しい項目は `#[serde(default)]`（装飾なし）で読む。未リリースなので移行コードは書かない（D9）が、既存ファイルを
   壊さない。保存往復のテスト。
7. `musicvideo-ui` は egui / core 系 / egui-ime だけに依存。`src/`（本体）を触らない。アプリを起動しない。コミットしない。
   `cargo fmt` を通す。

---

## 2. UI

- インスペクタの「レイヤー」タブで、3 種のレイヤーに**共通の「文字装飾」セクション**を出す（1 つの編集関数を共有）。
  各装飾は ON/OFF と値。追加縁取りは追加・削除できる。
- 文字装飾は他の Animatable と同じくキーフレームを打てる（タイムラインに行が出る）。既存の
  `edit_animatable_property` の仕組みに載せる。
- Undo の単位（L1）、選択枠（L2）、IME 補正（I1、文字入力欄は共有 helper）を崩さない。
- 新しい UI 文言に絵文字・環境依存記号を使わない（`scripts/check_ui_glyphs.py`）。

---

## 3. テスト

1. **D15 の回帰対象**: 透明 Fill + 見える縁取り、グラデーション Fill + 縁取り、回転 + グロー、半透明レイヤーで背景・縁取りと
   glyph が重なる場合（重なり部分の alpha が 1 回分であること）、装飾なしはフレームハッシュ不変。
2. 装飾が矩形の外へはみ出す場合（大きな影・広い縁取り）に切れずに描かれ、揃え位置（Start / Center / End）が装飾なしと同じ。
3. 補間: 構造一致で補間、構造不一致で全体 Step、境界（キー 1 個、同時刻）。
4. キャッシュ: 上限を超えると古いものから捨てる、静的な結果が補間フレームで追い出されにくい、同じ入力で同じ結果。
5. 3 種のレイヤー（Text / TimeText / Lyrics）が同じ型・同じ描画経路を通る。歌詞のフェードと装飾の組み合わせ。
6. 保存往復と、装飾項目の無い既存 JSON の読み込み。
7. 決定性: 同じ Scene を 2 回描いてバイト一致。
8. 変異検証（例: opacity を 2 枚それぞれに掛ける、装飾なしを白で焼いた bitmap に統合する、構造不一致で属性ごとに補間する）。

## 4. 計測（D15 / §7 の T1 行の受け入れ条件）

- 装飾ありのシーン（縁取り 2 本 + 影 + 背景の時間表示・歌詞・テキスト、1920x1080）の合成時間 mean / p95 を、装飾なしと
  比べて記録する。
- 装飾をキーフレームで動かした長尺書き出し（例: 縁取り幅を 5 分かけて変える）で、キャッシュの最大量と最大メモリを測る。
  測定方法と条件を計画書に書く。他セッションの rustc 等で CPU が混んでいないことを先に確認する（過去に測定値が汚れた）。

## 5. 完了条件

1. `cargo test -p musicvideo-core` / `-p musicvideo-ui` / `-p musicvideo_lab` / `-p egui-ime` が緑。
2. `cargo check -p mimageviewer --features dev-tools --bin mvexport_cli` が通る（書き出し側が新しい型で動く）。
3. `cargo tree -p musicvideo-ui` に eframe / cpal / ffmpeg / mimageviewer が出ない。
4. `cargo fmt --check`、`python scripts/check_ui_glyphs.py` が通る。
5. `cargo build --release -p musicvideo_lab` が通る（使用中なら別出力先）。
6. 計画書 §7 の T1 行と D15 に結果・計測を追記する。

## 6. 報告してほしいこと

- 2 枚合成の実装（装飾 RGBA と glyph マスクの作り方、到達範囲の取り方、opacity / clip / グローの適用位置）
- 補間とキャッシュの実装、キャッシュの上限値とその根拠
- §4 の計測結果
- 追加テストと変異検証、テスト件数
- 前提がコードと合わなかった点（あれば）
- `git diff --stat`

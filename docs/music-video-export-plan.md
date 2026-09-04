# 音楽ビデオ書き出し (Music Video Export) — 設計計画

Status: 計画 v1（着手前レビュー用ドラフト。ユーザー承認前）。
実装 = Codex Sol / レビュー = ClaudeCode / 仕上げの実機目視 = ユーザー。
開発場所 = 別 worktree `C:\home\mimageviewer-musicvideo`（ブランチ `musicvideo`）。
最終着地点 = **mIV 本体機能**（ラボ止まりにしない）。

自作楽曲にイラスト・スペクトラムアナライザ・時間表示・進捗バーを重ねて MP4 に書き出す機能。
現行ワークフロー（Wav2Bar）の置き換えを目標とする。

---

## 0. このドキュメントの位置づけ

本書は**契約書**。comic 統合（[docs/comic-integration-plan.md](comic-integration-plan.md)）と
music 統合（[docs/music-integration-plan.md](music-integration-plan.md)）で機能した運用を踏襲する:

- 小インクリメント + 受け入れ基準 + 毎回レビュー
- 設計が変わったら**先に本書を更新してから**実装する
- 「再実装しない（再利用する）」— §2 の再利用表を消し込みながら進める

---

## 1. 目的と非目的

### 1.1 目的

- 音声ファイル 1 本を主軸に、画像・スペクトラムアナライザ・時間表示・進捗バー・鍵盤表示を
  重ねて 1 本の MP4（H.264 + AAC）として書き出す。
- 途中で画像を切り替える／色を変える／表示を ON/OFF する演出をキーフレームで付ける。
- スペクトラムアナライザは直線型と円形型を選べ、グラデーション・グローで見栄えを整えられる。

### 1.2 非目的（v1 では作らない）

- 汎用動画編集（動画素材の合成、カット編集、トランジション全般）
- パーティクル・3D・シェーダのユーザー編集
- Wav2Bar プロジェクトファイルの読み込み互換
- 書き出し以外の出力（連番 PNG は開発用途としてのみ持つ）

### 1.3 ライセンス境界（最重要・恒久ルール）

**Wav2Bar は GPL-3.0-or-later、mIV は MIT。Wav2Bar のソースコードを読まない・移植しない。**
参考にしてよいのは「どんなオブジェクトがあり、どんなプロパティを持つか」という機能の観察と
公開ドキュメントの範囲まで。実装は本書のデータモデルから独立に起こす。

Wav2Bar 本体は開発停止（別リポジトリでの作り直しへ移行）を明記しており、実装分析の価値は低い。
要件の正本は **§1.4 の実測インベントリ**（ユーザーの現行プロジェクト）とする。

### 1.4 現行プロジェクトの実測インベントリ（要件の正本）

ユーザーが実際に運用している Wav2Bar プロジェクトのオブジェクト構成（2026-09-04 に画面から採取）。
**実使用のオブジェクト種別は 4 つだけ**で、キーフレームは使われていない（＝現行ツールに無い機能）。

| オブジェクト | 種別 | 実値 |
| --- | --- | --- |
| `object286476` | Image - Shape | X0 Y0 / 1920x1080 / 背景 type=image / background size=scale_size_control 100% / repeat X,Y = off |
| `bar` | Timer（進捗バー） | X60 Y1000 / 1800x50 / color `#ffffff` / border thickness 2px / border-radius `20px` / **box-shadow `0 0 20px #f02`** |
| `circle` | Visualizer（円形） | X410 Y0 / 1100x1100 / **points count 200** / **analyser range 0–750** / smoothing type `average` / factor `0.7` / color `#78ff23` / **box-shadow `0 0 20px #f8f`** / bar thickness 4 / radius 380px |
| `time text` | Text（type=time） | X1520 Y920 / 400x100 / font size 50 / color `#000000` / align center / **text-shadow `0 0 5px #fff`** |

全オブジェクト共通のプロパティ: Name / Layer（重ね順）/ X,Y / 幅・高さ / **整列ヘルパー（左中右・上中下）**/
Rotation / SVG Filters（上級者向け、実プロジェクトでは未使用）/ border-radius。

ここから読み取れる設計上の含意:

1. **グローは「オブジェクト単位の外側グロー」**（CSS の `box-shadow: 0 0 20px <色>` /
   `text-shadow` 相当）であって、シーン全体のブルームではない。実装は
   「そのレイヤをオフスクリーンへ描く → アルファをぼかす → 指定色で下に加算」で足りる。
   ユーザーの現行の絵と一致させるにはこちらが第一級（§3 D4）。
2. **座標は 1920x1080 基準の絶対 px** で入力している。整列ヘルパーが実質的に多用される。
3. **円形ビジュアライザは「点」の集合**（points 200 × 太さ 4px、半径 380px）。
4. **周波数軸が mIV の音楽ビューと違う**（重要・§3 D5）。Wav2Bar の analyser range 0–750 は
   FFT ビンの**線形**インデックス指定。mIV の `music-core` は半音バンド（MIDI 16–133）＋
   等ラウドネス重み付けの**音楽的**な軸で、しかもバンド数上限が 128。
   現行の絵を再現するには「線形周波数軸・任意バンド数・重み無し」モードが要る。
5. 色は現状すべて単色。グラデーションは**現行に無い改善要求**。
6. キーフレームも現行に無い改善要求（＝パリティ要件ではなく新機能）。

---

## 2. 既存資産の棚卸し

### 2.1 再利用するもの（書き直さない）

| 用途 | 実体 | 備考 |
| --- | --- | --- |
| H.264 エンコード | [src/video/stream/encoder.rs](../src/video/stream/encoder.rs) | NVENC / QSV / AMF / MediaFoundation / OpenH264 を自動選択。`open_h264_encoder(preference, StreamOutputParameters, FrameRate)` は解像度・ビットレートを直接渡せるので、HLS 用の `QualityPreset` を経由せず書き出し用の値を渡せる |
| AAC エンコード | [src/video/stream/audio_encoder.rs](../src/video/stream/audio_encoder.rs) | `open_aac_encoder` |
| クロック非依存のオフライン駆動 | [src/video/clockless_transcode.rs](../src/video/clockless_transcode.rs) | 再生クロック・音声デバイスに触らず実時間より速く回す駆動器の**前例**。書き出しの背圧・リング設計はここに倣う |
| 音声の全尺デコード | [src/audio_decode.rs](../src/audio_decode.rs) `decode_audio_file_to_stereo_f32` | インターリーブ stereo f32 / 48kHz |
| スペクトラム解析 | [crates/music-core/src/analysis.rs](../crates/music-core/src/analysis.rs) | `spectrum_analysis_from_stereo_window(pcm, rate, center_secs, bands)` は PCM と時刻だけの関数。オフラインレンダに直接使える |
| 鍵盤表示のロジック | [src/ui_music_spectrum.rs](../src/ui_music_spectrum.rs) `draw_pitch_keyboard` | 黒鍵配置・輝度・低域整理の**ロジック部分**を core へ切り出して共有（D6） |
| **テキスト（日本語）と CPU ラスタライズ** | [crates/comic-core/](../crates/comic-core/) `font.rs` / `layout.rs` / `raster.rs` | rustybuzz による OpenType シェーピング（縦書き・縦中横含む）＋ `ab_glyph_rasterizer` のカバレッジ＋ TTF/OTF/TTC ロード。RGBA8 へ焼く CPU ラスタライザに袋文字・影・**グロー**（`TextGlowStyle { color, radius_px, spread_px }` = CSS box-shadow と同じ意味論、`draw_layout_soft_mask`）まで揃っている。egui 非依存 |
| wgpu 直叩きの前例 | `src/video/gpu_renderer/`, [src/gpu_lanczos.rs](../src/gpu_lanczos.rs) | GPU 化が必要になった場合の実装パターン（D4） |
| ラボ→本体の統合運用 | comic-core/comic_lab、music-core/music_lab | 別 worktree は `-p` 指定でビルドすれば本体 build.rs が走らず vendor/ 不要 |

### 2.2 新規に作るもの

| 項目 | 理由 |
| --- | --- |
| シーングラフとキーフレーム評価 | 既存コードにアニメーションの概念が無い（`keyframe` の既存出現は全て動画の I フレームの意味） |
| ~~任意バンド境界の解析 API~~ | **P1/P2 では作らない**（D5）。`music-core` の帯域積分（`power_for_range`）は private・半音バンド固定・128 上限。線形軸などのオプションを足す段になったら、任意の `(low_hz, high_hz)` 列と重み付け有無を受ける公開 API を `music-core` に追加する |
| バー / 円弧 / 進捗バーの描画とグラデーション | `comic-core` は吹き出し（楕円・角丸・尾）とテキストが対象なので、スペクトラムの幾何と `Fill` の勾配は新規 |
| **MP4 ファイルへの muxer** | 既存の出口は HLS 用の in-memory fMP4 リング（[src/video/stream/segmenter.rs](../src/video/stream/segmenter.rs)）だけ。ファイル出力は `ffmpeg::format::output` で mp4 を新規に開く |
| プロジェクトファイル（JSON） | 既存の永続化ストアに載せない（D9） |

---

## 3. 設計決定

### D1. プレビューと書き出しは単一のラスタライザを通る

この種のツールで最も壊れるのが「プレビューと出力の絵が違う」。**描き手を 2 つ持たない**。
プレビューは出力解像度の内部バッファへ描いてから縮小表示するだけにする（縮小以外の差を作らない）。

### D2. 中間表現を 1 枚挟む

`SceneGraph`（レイヤ + キーフレーム）→ `evaluate(t)` → `Scene`（不変の描画コマンド列）→ `rasterize(Scene)`。
`evaluate` は純粋関数でユニットテスト対象、`rasterize` は GPU 依存でスナップショット対象。

### D3. 決定性 — フレームは時刻 t のみに依存する

- 書き出しのフレーム n は `t = n / fps`。乱数・実時間・フレームレート変動に依存させない。
- `SpectrumAnalyzer` は多重解像度窓の内部状態（`last_center_frame`）を持ち、**呼び出し順に依存する**。
  よって書き出しは 1 本の analyzer を **t 昇順**で回す。プレビューでシークした場合は
  analyzer を作り直す（不連続 = リセット）。時間方向の平滑化（§1.4 の smoothing factor 0.7）も
  同じ理由で「t 昇順の逐次適用」を前提にする。
- 受け入れ基準: 同一プロジェクトを 2 回書き出したとき、全フレームがビット一致する。

### D4. ラスタライザは CPU から始める（`comic-core` を土台にする）

当初は wgpu 自前を想定していたが、[crates/comic-core/](../crates/comic-core/) に
**日本語シェーピング・グリフラスタライズ・袋文字・影・グロー・RGBA8 への焼き込み**が
egui 非依存で既にある（§2.1）。テキストのためだけに GPU のグリフ atlas を新設する理由は無い。

- **P1 は CPU ラスタライザ 1 本**。`comic-core` の `RgbaOverlay` へ全レイヤを焼き、
  グローは既存の soft-mask（膨張 + ぼかし）をテキスト以外のアルファへ一般化して使う。
- プレビューは焼いた RGBA8 をテクスチャとして表示するだけ（**描き手は 1 つ**、D1）。
  `comic_lab` が egui プレビューと CPU 焼き込みの 2 経路を持って突き合わせているのとは対照的に、
  ここでは最初から 1 経路にする。
- **GPU 化は性能で判断する**。P1 に「1080p60 を実時間の 2 倍速で書き出せるか」の実測を含め、
  届かなければ合成だけ wgpu へ移す。`Scene` を挟んである（D2）ので差し替えは局所で済む。
- 副次効果として書き出しがハードウェア非依存で**ビット再現**する（D3 の受け入れ基準が
  同一機に限定されなくなる）。

グロー自体は `(blur radius, spread, 色)` を各レイヤのプロパティとして持ち、CSS の
`box-shadow` / `text-shadow` と同じ意味論で描く。シーン全体のブルームは v1 では作らない。

### D5. 周波数軸は mIV の音楽的な軸を既定にする

Wav2Bar の見た目（線形 FFT ビン 0–750 / 200 点）を厳密に再現する必要は無い、という判断
（2026-09-04 ユーザー確定）。**既定は `music-core` の半音バンド + 等ラウドネス重み付け**とし、
`Linear { min_hz, max_hz }` など他方式は将来のオプションとして後から足す。

したがって P1/P2 では `music-core` の既存 API（最大 128 バンド）をそのまま使い、
任意バンド境界の公開 API 追加は**将来のオプション実装時まで先送り**する。

いずれの方式でも「バンド境界の列 → 帯域パワー → 表示値」という同じ経路を通し、
**直線表示と円形表示は同じバンド列に対する座標変換違い**にする（円形専用の解析を持たない）。

### D6. 鍵盤表示のロジックは core へ切り出して共有する

現在は egui の描画関数の中にロジックが埋まっている。配置・輝度・低域整理の判断を純関数へ出し、
**mIV の音楽ビューと書き出しの両方が同じ関数を呼ぶ**。egui 描画をそのまま書き出しへ持ち込まない。

### D7. エンコードは既存資産を再利用し、出口だけ新規に作る

`encoder.rs` / `audio_encoder.rs` はそのまま使う。fMP4 segmenter は HLS 専用のリングバッファ設計
なので**書き出しには使わない**。`ffmpeg::format::output` で非フラグメント MP4（faststart）を書く。

### D8. 音声の正本は全尺 stereo f32 / 48kHz を 1 本だけ持つ

`decode_audio_file_to_stereo_f32` の結果を解析と AAC エンコードの両方に使う。
解析用と書き出し用で別々にデコードしない（ズレの温床）。

### D9. プロジェクトファイルは独立した JSON

mIV の設定 DB・キャッシュ DB には載せない。ユーザーが任意の場所に保存する `.mivmv.json`（仮）。
`version` フィールドを持つ。**未リリースの間は破壊的変更を許容**（移行コードを書かない）。

### D10. 本体では独立した編集モードにする

閲覧のホットパス（グリッド / フルスクリーン / detached viewer）に編集状態を持ち込むと
ライフサイクルの泥沼に入る。書き出しエディタは独立したモードとして分離し、
閲覧側からは「この音声ファイルで開く」だけを渡す。

### D11. 出力の既定値

1920x1080 / 60fps / H.264 High profile / yuv420p / bt709 / AAC-LC 48kHz。
解像度・fps は書き出しダイアログで変更できるが、既定は上記に固定する。

### D12. 座標は出力解像度基準の px で入力し、内部は正規化で持つ

ユーザーは 1920x1080 基準の px で考えている（§1.4）。UI は px 入力＋整列ヘルパーを提供し、
モデルは正規化座標で保持する。これで出力解像度を変えてもレイアウトが崩れない。

### D13. 書き出しは中断可能で、UI スレッドを止めない

worker thread + cancel token + 進捗コールバック。[docs/ui-responsiveness.md](ui-responsiveness.md) §4 の
チェックリストを通す。

---

## 4. データモデル（v1）

```
Project {
    version: u32,
    audio: AudioRef { path },
    output: Output { width, height, fps, video_bitrate, encoder_preference },
    duration_secs: f64,            // 既定 = 音声長
    layers: Vec<Layer>,            // 描画順（先頭が奥）
}

Layer {
    id, name,
    visible:  Animatable<bool>,    // ON/OFF もキーフレーム対象
    opacity:  Animatable<f32>,
    rect:     Animatable<Rect>,    // 正規化座標（UI は px 表示）
    rotation: Animatable<f32>,
    corner_radius: Animatable<f32>,
    glow:     Animatable<Option<Glow>>,   // { blur_px, color }  ← §1.4 の box-shadow 相当
    kind: LayerKind,
}

LayerKind =
  | Image        { source: ImageRef, fit: Fit, scale_percent }
  | Solid        { fill: Animatable<Fill> }
  | Spectrum     { axis: Semitone{..},          // v1 はこれだけ。Linear{min_hz,max_hz} は将来 (D5)
                   shape: Bars{..} | Radial{ radius, .. },
                   point_count, thickness, smoothing: { kind, factor },
                   fill: Animatable<Fill> }
  | PitchKeyboard{ .. }
  | ProgressBar  { shape: Linear | Radial, border_thickness,
                   fill: Animatable<Fill> }
  | TimeText     { format, font, size, align, fill: Animatable<Fill> }
  | Text         { text, font, size, align, fill: Animatable<Fill> }

Animatable<T> = Const(T) | Track(Vec<Keyframe<T>>)
Keyframe<T>   = { t: f64, value: T, interp: Step | Linear | EaseIn | EaseOut | EaseInOut }
Fill          = Solid(Color) | LinearGradient{..} | RadialGradient{..} | BandGradient{..}
```

- **画像の切り替え + フェード**は「Image レイヤを複数置き、それぞれの `opacity` にキーフレームを
  打つ」で表現する（専用のスライド機構を先に作らない）。煩雑なら P3 で「slides」ショートカットを
  検討する（未決 Q1）。
- `BandGradient` は「バンド位置に応じて色を変える」用（高域ほど明るい等）。

---

## 5. レンダリングパイプライン

```
SceneGraph --evaluate(t)--> Scene --rasterize(CPU)--> RgbaOverlay(1920x1080)
                                     |
                                     +-- レイヤごと: glow があればアルファを膨張+ぼかして
                                     |               指定色で先に置き、その上に本体を描く
                                     +-- 無ければ直接本描画
```

- プレビューは同じ `rasterize` を出力解像度で呼び、焼いた RGBA8 をテクスチャとして表示する
  （縮小以外の差を作らない）。
- **テキストは `comic-core` に投げる**（D4）。`layout_text` / `layout_text_wrapped` でレイアウトし、
  グリフのカバレッジを `RgbaOverlay` へ焼く。フォントは `FontSet` で TTF/OTF/TTC をロードする。
  日本語のシェーピング・縦書き・袋文字・影・グローは既にこの経路にある。
- 焼いたテキストは **(文字列, フォント, サイズ, 色, 縁取り, グロー) をキーにキャッシュ**する。
  時間表示は `mm:ss` なら 1 秒に 1 回しか変わらないので、毎フレームのレイアウトは走らない。

---

## 6. 書き出しパイプライン

```
frame n → t = n/fps → evaluate → rasterize(CPU, rayon) → RGBA→NV12/YUV420p → H.264 ┐
音声:  全尺 stereo f32 ───────────────────────────────────→ AAC ───────────────────┴→ mp4
```

- 合成とエンコードは別スレッドにし、間を有界リングで繋ぐ（背圧の設計は
  `clockless_transcode.rs` に倣う）。エンコーダが NVENC なら合成が律速になる想定。
- 色変換は swscale。
- 性能目標: 1080p60 の 5 分（18,000 フレーム）を**実時間の 2 倍以上の速度**で書き出す。
  **P1 でこれを実測し、届かなければ合成を wgpu へ移す**（D4）。
  1 フレームあたりの予算は 8.3ms。内訳の目安は 背景ブリット 1–2ms / バー・図形 1ms /
  グローぼかし 2–4ms / 色変換 2–3ms。静的な背景画像の拡縮は起動時に 1 回だけ行いキャッシュする。

---

## 7. インクリメントと受け入れ基準

| # | 内容 | 受け入れ基準 |
| --- | --- | --- |
| **P0** | §1.4 の実測インベントリ確定（済）＋本書のユーザー承認 | 承認 |
| **P1** | 書き出し完走: 静的シーン（背景画像 + 直線バー + 時間表示 + 進捗バー）を CPU ラスタライザで描き MP4 まで出す。キーフレーム無し・グロー無し | 5 分の曲が 1920x1080 / 60fps の MP4 になり、音ズレ無く再生でき、投稿できる。2 回書き出してフレームがビット一致する。**書き出し速度を実測して記録する**（実時間の 2 倍に届かなければ D4 に従い GPU 化を判断） |
| **P2** | 見栄え: オブジェクト単位グロー / グラデーション / 円形スペアナ / 鍵盤表示 | **§1.4 のプロジェクトを mIV 上で再現できる**（並べて見比べて遜色が無い）。ただし**スペクトラムの周波数軸は一致対象から外す**（D5 — mIV の音楽的な軸で良い） |
| **P3** | キーフレーム: 画像切替・フェード・色変化・ON/OFF | 既存の楽曲 1 本を Wav2Bar を使わずに最初から最後まで作り切れる |
| **P4** | 本体統合: mIV の編集モードとして載せる。プロジェクト保存 / 読み込み、書き出しダイアログ、keymap | [docs/ui-responsiveness.md](ui-responsiveness.md) §4 を通過。閲覧側の退行が無い |

各インクリメントの終わりで実機確認 → レビュー → コミット。
**P2 の完了条件を「§1.4 の再現」に固定する**ことで、「一部だけ違う形で実装された」を機械的に防ぐ。

---

## 8. テスト戦略

- `musicvideo-core`: キーフレーム評価、レイアウト計算、バンド境界生成、バンド→幾何変換を
  純ロジックでテストする。境界（キーフレーム 0 個 / 1 個、範囲外の t、同一 t の重複）を明示的に持つ。
- ラスタライザ: 小さい固定サイズ（例 320x180）のオフスクリーン描画 → PNG スナップショット。
  方針は [docs/ui-snapshot-policy.md](ui-snapshot-policy.md) に合わせるが、egui_kittest ではなく
  自前のオフスクリーン harness を使う。
- 書き出し: 合成した短い音源（1 秒）で MP4 を書き、フレーム数・duration・音声長・
  2 回実行のビット一致を検証する。FFmpeg DLL が要るので実機扱い。

---

## 9. 未決事項

| # | 内容 | 期限 |
| --- | --- | --- |
| Q1 | 画像切替の表現（レイヤ複数 + opacity か、専用の slides 機構か） | P3 着手前 |
| ~~Q2~~ | ~~テキストのグリフラスタライズ方式~~ → **解決**: `comic-core` を使う（D4、2026-09-04） | — |
| ~~Q3~~ | ~~任意バンド境界 API~~ → **先送り**: 既定は半音軸（D5、2026-09-04） | 将来のオプション実装時 |
| Q4 | プロジェクトファイルのパス保存（絶対 / 相対 / 埋め込み） | P3 |
| Q5 | 鍵盤ロジックの切り出し先（`music-core` か `musicvideo-core` か） | P2 着手前 |
| Q6 | Wav2Bar の `SVG Filters` 相当（任意フィルタ）を持つか | P3 以降・現行未使用なので後回し |
| Q7 | フォント選択 UI を `comic-core` の `FontSet` にどう繋ぐか（mIV のテキスト注釈と同じフォント一覧を使うか） | P2 |
| Q8 | 周波数軸オプション（線形 / 対数 / 重み無し）をいつ足すか | P3 以降 |

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
- 帯域エネルギーのモジュレータも同じ前進専用規約で評価する。値は PCM と t のみに依存し、
  シーク時は解析器と EMA を一緒にリセットする。

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

#### 決着（2026-09-04、P1a の実測）

**CPU ラスタライザのまま進める。GPU 化はしない。** 1920x1080 / 60fps、背景 3000x1687 を
`Fit::Cover`、実 PCM を `SpectrumSource` に通した 900 フレームの実測（Core Ultra 9 285 / 24 論理コア、release）:

| 区間 | 平均 | p95 |
| --- | ---: | ---: |
| スペクトラム解析（music-core の FFT） | 0.189 ms | 0.405 ms |
| シーン評価 + CPU 合成 | 1.219 ms | 1.635 ms |
| **合計** | **1.408 ms** | **1.890 ms** |

実時間比 11.8 倍。予算 8.3ms/frame に対して約 6 倍の余裕がある。P1b で加わる
RGBA→YUV 変換と H.264 / AAC エンコードを載せても目標（実時間の 2 倍）を大きく上回る見込み。

#### P2 完了時点の実測（2026-09-04、静かなマシン、60 秒素材 2 回）

§1.4 相当のシーン（円形スペアナ 200 点 + グロー 3 箇所 + 鍵盤）で **実時間比 2.04x / 1.97x**。

| 区間 | 平均 | 占有 |
| --- | ---: | ---: |
| シーン評価 + CPU 合成 | 3.04 ms | 39% |
| **RGBA→NV12 変換（swscale、単スレッド）** | **4.24 ms** | **54%** |
| H.264 エンコード（NVENC） | 0.50 ms | 6% |

**目標（2 倍）ちょうどで余裕が無い。** P3 でレイヤとグローが増えれば割る。
色変換もグローのぼかしも分離可能で並列化でき、24 論理コアに対してほぼ手つかず。
よって P2c として先に並列化する（P3 の前に、測り直しの手間を二重に払わないため）。

なお当初この測定は 0.5〜1.2x と出ていたが、これは他作業による CPU 競合。
同じ書き出しの合成時間が 4.0 → 16.3 ms と 4 倍ぶれた一方、RGBA ハッシュは 3 回とも一致していた。
**合成は rayon で並列化されているのでコアを奪われると素直に悪化する。以後、性能はマシンが
静かなことを確認してから測る。**

#### P2c 完了時点の実測（2026-09-04、60 秒素材 2 回）

同じ §1.4 相当シーンを 1920x1080 / 60fps / h264_nvenc で書き出した。背景は出力と異なる
3000x1687。測定直前の 5 秒間は OS の `\Processor(_Total)\% Processor Time` がすべて 20% 未満
（この実行環境では `Get-CimInstance Win32_Processor` がアクセス拒否になるため代用）だった。

| 区間 | 1 回目 | 2 回目 | 平均 |
| --- | ---: | ---: | ---: |
| シーン評価 + CPU 合成 | 3.278 ms | 3.345 ms | 3.312 ms |
| **RGBA→NV12 変換（swscale、水平帯を並列処理）** | **0.993 ms** | **1.078 ms** | **1.036 ms** |
| H.264 エンコード（NVENC） | 0.537 ms | 0.541 ms | 0.539 ms |
| **実時間比** | **3.277x** | **3.187x** | **3.232x** |

色変換は P2 完了時点の 4.24 ms から平均 1.036 ms（約 76% 減）になった。帯境界は偶数行で、
上下の halo を含めて変換して中央だけを結合する。NV12 / YUV420p とも従来の単一 swscale
コンテキストの出力とアクティブ画素がバイト一致するテストを持つ。RGBA フレーム列の SHA-256 も
最適化前後で `929bda8f3cfbb34f472bf3196d43076aa4c4ab33af1e05c3154d9437007e3a31` と一致した。
グローの box blur は P2c 開始時点ですでに横パス・転置・縦パスが行／列独立に rayon 化されて
いたため、その実装を維持し、直列参照パスとのバイト一致テストを追加した。

#### P2c 後のグロー切断修正と区間外計測（2026-09-04）

縮小ぼかしの余白を名目上の `spread + radius` ではなく、同じ演算計画が決める
`(ceil(spread / scale) + ceil(radius / scale) + 1) * scale` とした。最後の 1 ブロックは
max-downsample の最大 `scale - 1` ピクセルの広がりと、外縁のゼロ画素を含む。これにより
scale 1 / 2 / 4 / 8 のいずれでも、余白と実際の max-filter / box-blur 半径が別々にずれない。

同じ 60 秒出力を `--frame-hash` 無し／有りで測った区間外内訳は次のとおり。どちらも測定直前の
5 秒間の OS CPU 使用率がすべて 20% 未満。入力は 104 秒の `out.mp4`、出力は先頭 60 秒。

| 区間 | ハッシュ無し | `--frame-hash` 有り |
| --- | ---: | ---: |
| 音声全尺デコード | 0.173 s | 0.136 s |
| AAC エンコード | 0.483 s | 0.491 s |
| mux（ヘッダー + パケット書き込み） | 0.149 s | 0.123 s |
| `av_write_trailer` + faststart | 0.032 s | 0.034 s |
| 一時ファイル rename | 0.000418 s | 0.000713 s |
| RGBA SHA-256 | 0 s | **7.557 s** |
| その他 | 0.149 s | 0.151 s |
| **フレーム 3 区間外の合計** | **0.986 s** | **8.493 s** |

したがって約 9 秒の差の主因は faststart ではなく、検証専用の RGBA フレームハッシュである。
通常の書き出しでは `compute_frame_hash` を有効にしない。

**P1b の実測で律速が変わった。** 1920x1080 / 60fps、h264_nvenc（RTX 4090）:

| 区間 | 平均 |
| --- | ---: |
| シーン評価 + CPU 合成 | 1.665 ms |
| **RGBA→NV12 変換（swscale）** | **4.117 ms** |
| H.264 エンコード | 0.507 ms |

実時間比 2.4 倍（10 秒素材）/ 2.735 倍（300 秒素材、起動コストが薄まる分だけ速い）。目標は満たすが、
**いま最も重いのは合成でもエンコードでもなく色変換**。さらに速度が要るときはここから手を付ける
（swscale のマルチスレッド化、あるいはラスタライザから直接 NV12 を書く）。合成を GPU へ移しても
効かない。

この数字は最初の測定を 1 度差し戻して取り直したもの。初回は背景画像がちょうど出力解像度で
`copy_from_slice` の速い経路しか通らず、スペクトラム解析も合成値で、**書き出しに必ず含まれる
コストが 2 つ抜けていた**。拡縮結果は `Assets` のキャッシュ（キー = 画像 ID + 出力先寸法 + Fit）で
1 回だけ計算する。

グロー自体は `(blur radius, spread, 色)` を各レイヤのプロパティとして持ち、CSS の
`box-shadow` / `text-shadow` と同じ意味論で描く。シーン全体のブルームは v1 では作らない。

### D5. 周波数軸は mIV の音楽的な軸を既定にする

Wav2Bar の見た目（線形 FFT ビン 0–750 / 200 点）を厳密に再現する必要は無い、という判断
（2026-09-04 ユーザー確定）。**既定は `music-core` の半音バンド + 等ラウドネス重み付け**とし、
`Linear { min_hz, max_hz }` など他方式は将来のオプションとして後から足す。

P1/P2 では `music-core` の既存 API（最大 128 バンド）をそのまま使った。P3b 後の
帯域エネルギーモジュレータで、Hz 範囲または MIDI 半音範囲と表示重み付けの有無を指定する
追加 API を公開した。通常の bands / notes と任意範囲を同じ 1 回の多重解像度 FFT 更新から
取り出し、変調のために解析を重複させない。表示用スペクトラムの周波数軸自体は従来どおり
半音軸を既定とする。

いずれの方式でも「バンド境界の列 → 帯域パワー → 表示値」という同じ経路を通し、
**直線表示と円形表示は同じバンド列に対する座標変換違い**にする（円形専用の解析を持たない）。

### D6. 鍵盤表示のロジックは core へ切り出して共有する

配置・輝度・低域整理に加え、五度圏ベースの鍵盤色・スペクトラム色も `music-core` の純関数に置き、
**mIV の音楽ビューと書き出しの両方が同じ関数を呼ぶ**。egui 描画をそのまま書き出しへ持ち込まない。
書き出し側はレイヤごとにカスタム Fill / mIV 配色を選択できる。鍵盤の表示ゲインはプロジェクトに
`display_gain_db`（既定 0 dB）として持ち、音声波形そのものを変えず `prepare_keyboard_frame` に渡す。

2026-09-04 の一定振幅対数スイープ診断では C5–B5 に局所的な谷が見えたが、窓長が異なる
多重解像度 FFT の周波数応答をスイープで判定するのは不適切だった。2026-09-05 に各半音を独立した
2 秒の定常正弦波（48 kHz、振幅 0.0035）で鳴らし、最長 32768-sample 窓が完全に埋まる中央 1 秒で
測り直した。表示値平均は低域 A0–B2 = 0.123525、中域 C3–B5 = 0.127632、高域 C6–C8 =
0.132682。オクターブ別では C4–B4 = 0.121548、C5–B5 = 0.123096、C6–B6 = 0.131896 で、
C5–B5 の谷は再現しなかった。

先のスイープは約 0.184 秒で 1 半音を通過する一方、C5–B5 が使う 16384-sample 窓は約 0.341 秒
（約 1.85 半音ぶん）を同時に含む。1 kHz を越えると 8192-sample / 約 0.171 秒の窓へ切り替わるため、
窓内を移動する信号の時間方向のにじみが音域差として現れていた。これは定常周波数応答の谷ではない。
よって解析定数・窓選択・窓間ブレンドは変更せず、静かな素材は表示ゲインで持ち上げる。
再測定は `cargo run -p musicvideo_lab --release -- --stationary-note-response` で半音別・音域別の値を
出力できる。

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
    display_gain_db: f32,          // 鍵盤表示だけの追加ゲイン。既定 0 dB
    modulators: Vec<Modulator>,    // 音声から得る 0..1 の変調源
    layers: Vec<Layer>,            // 描画順（先頭が奥）
}

Modulator {
    id, name,
    source: BandEnergy {
        range: Hz{ low, high } | Semitone{ low_midi, high_midi },
        smoothing, weighting: Raw | MivDisplay,
        gain_db, floor, ceiling,
    },
}

Layer {
    id, name,
    visible:  Animatable<bool>,    // ON/OFF もキーフレーム対象
    opacity:  Animatable<f32>,
    rect:     Animatable<Rect>,    // 正規化座標（UI は px 表示）
    rotation: Animatable<f32>,
    corner_radius: Animatable<f32>,
    glow:     Animatable<Option<Glow>>,   // { blur_px, color }  ← §1.4 の box-shadow 相当
    modulations: Vec<ModulationBinding>,
    kind: LayerKind,
}

ModulationBinding {
    target: Opacity | Scale | Rotation | CornerRadius | GlowRadius | GlowAlpha |
            FillBrightness | SpectrumThickness | SpectrumRadius,
    modulator: ModulatorId,
    amount: f32,                  // 対象の単位で加算。負値も可
    curve: Linear | Squared | Sqrt,
}

LayerKind =
  | Image        { source: ImageRef, fit: Fit, scale_percent }
  | Solid        { fill: Animatable<Fill> }
  | Spectrum     { axis: Semitone{..},          // v1 はこれだけ。Linear{min_hz,max_hz} は将来 (D5)
                   shape: Bars{..} | Radial{ radius, .. },
                   point_count, thickness, smoothing: { kind, factor },
                   color_mode: Custom | Miv,
                   fill: Animatable<Fill> }
  | PitchKeyboard{ color_mode: Custom | Miv, .. }
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
- 変調はスカラー対象だけに限定し、`Animatable<T>` 自体は汎用変調しない。最終値は
  Const / Track の評価値へ binding を宣言順に加算する。Scale は矩形中心を固定して適用する。

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
| **P1a** ✅ | レンダラ: `crates/musicvideo-core`（純ロジック + CPU ラスタライザ）と `tools/musicvideo_lab`（プレビュー + 連番 PNG）。FFmpeg 非依存で `vendor/` 不要 | **完了**（2026-09-04、`681c7e84a`）。テスト 19 件緑。合成 1.408 ms/frame・実時間比 11.8 倍（D4 の決着を参照） |
| **P1b** ✅ | 書き出し完走: 本体パッケージに `src/video/mvexport/` を足し、既存 H.264/AAC エンコーダ + 新規 MP4 muxer へ P1a のフレームを流す | **完了**（2026-09-04）。実測 実時間比 2.4〜2.7 倍（h264_nvenc）、映像長と音声長の差 5.3ms、同一入力 2 回で RGBA フレーム列のハッシュ一致。テスト 7 件緑 |

> この worktree で `cargo test -p mimageviewer` 系を回すときは、`vendor/ffmpeg/bin/*.dll` を
> `target/debug/` と `target/debug/deps/` へコピーしておくこと（無いと `STATUS_DLL_NOT_FOUND` で
> テストが 1 件も走らずに落ちる）。release の CLI を直接叩く場合は `target/release/` にも要る。
>
> P1b は本体パッケージをビルドするので、この worktree にも `vendor/` が要る。**junction を張らず実コピーする**（CLAUDE.md「worktree + Windows junction の地雷」）。必要なのは `pdfium` / `ort` / `susie-worker` / `ffmpeg` / `models` / `vst3-host` / `twemoji` の 325MB 分だけで、`vst3sdk`（490MB）と `tensorrt-cache` は要らない。`eframe` / `egui-wgpu` は git 追跡済みなので既にある。
| **P2a** | 見栄え（`musicvideo-core` 内で完結）: オブジェクト単位グロー / グラデーション / 円形スペアナ / 角丸・枠・回転 | **§1.4 のプロジェクトを再現できる**（周波数軸は対象外、D5）。合成の平均が 3.5ms 以内（グロー 3 箇所 + 円形 200 点のシーンで実測） |
| **P2b** ✅ | 鍵盤表示: `draw_pitch_keyboard` のロジックを純関数として `music-core` へ切り出し、**本体の音楽ビューと書き出しが同じ関数を呼ぶ**（D6 / Q5 決着） | 書き出しに鍵盤が出る。**かつ本体の音楽ビューの見た目が変わらない**（切り出し前後で確認する） |
| **P2c** ✅ | 性能: RGBA→NV12 変換とグローのぼかしを並列化する | **完了**（2026-09-04）。実時間比 3.277x / 3.187x、色変換平均 1.036ms、最適化前後で RGBA フレーム列のハッシュ一致 |
| **P3a** ✅ | 編集 UI を `crates/musicvideo-ui`（egui 依存・eframe 非依存）へ移す + プロジェクトの保存/読み込み + 音付き再生プレビュー + 鍵盤背景の透過 | **完了**（2026-09-05、`5302f1155`）。ラボで保存・読み込みと音声時刻を正本にした再生プレビューを確認。`musicvideo-ui` は eframe / cpal / ffmpeg に依存しない |
| **P3b** ✅ | キーフレーム: 画像切替・フェード・色変化・ON/OFF のタイムライン編集 | **完了**（2026-09-05）。共有波形解析を背景にしたタイムラインで追加・削除・移動・補間変更を行える。全 Animatable プロパティの Const↔Track、同一時刻キー、最後のキー削除時の Const 化、保存・読み込みをテスト済み |
| **P3c** | 音声モジュレータの配管: 任意の Hz / 半音範囲のエネルギーをプロジェクト単位で定義し、レイヤのスカラーチャンネルへ加算 | 同じ 1 回の解析から bands / notes / 変調値を得る。無変調プロジェクトのフレームハッシュ不変、変調ありの 2 回一致、保存往復、編集 UI の現在値表示 |
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
| ~~Q3~~ | ~~任意バンド境界 API~~ → **解決**: P3c の帯域エネルギーモジュレータ用に、Hz / MIDI 範囲と重み付け有無を受ける追加 API を公開（D5、2026-09-05）。表示軸のオプション化とは分離した | — |
| ~~Q4~~ | ~~プロジェクトのパス保存~~ → **決着**: プロジェクトファイルからの相対パスで保存し、相対にできない場合だけ絶対。見つからない素材は黙って無視せず、どのレイヤのどのパスが無いかを出す（2026-09-04） | — |
| ~~Q5~~ | ~~鍵盤ロジックの切り出し先~~ → **決着**: `music-core` に置く（2026-09-04）。鍵盤は「音楽の可視化」であって書き出し固有ではなく、本体の音楽ビューと `musicvideo-core` の両方が既に `music-core` に依存している。逆向き（本体が `musicvideo-core` に依存）にしない | — |
| Q6 | Wav2Bar の `SVG Filters` 相当（任意フィルタ）を持つか | P3 以降・現行未使用なので後回し |
| Q7 | フォント選択 UI を `comic-core` の `FontSet` にどう繋ぐか（mIV のテキスト注釈と同じフォント一覧を使うか） | P2 |
| Q8 | 周波数軸オプション（線形 / 対数 / 重み無し）をいつ足すか | P3 以降 |

P3b で Q1 の方式を実装して確認した。単発のクロスフェードは 2 枚の画像レイヤに対して
各 2 個の opacity キーを打つだけで実用になる。一方、連続する多数の画像切替ではレイヤ追加と
両側の opacity キー設定を繰り返すため煩雑さが残る。専用の slides モデルは追加せず、実利用後に
「同じモデルへ複数操作をまとめて投入するショートカット」で足りるかを Q1 の判断材料とする。

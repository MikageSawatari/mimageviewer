# 音楽ビデオ書き出し (Music Video Export) — 設計計画

Status: P1〜P3j の実装あり、P4 未実装。2026-10-09 に P4 の方式を「別 exe のエディタ」へ改訂（D10 / D16）、
画面構成（D14）と文字装飾の初版範囲（D15）を追加。改訂は GPT-6.1 Sol / xhigh の独立レビュー 3 回で
条件付き ACCEPT（P4a 完了、2026-10-09）。
実装済みと実機検収済みは区別する。編集整合性の修正（`4ba3b7932` / `91326ca8d` / `0c24a453e` /
`8e12458e4`）は実装・独立レビュー・全体テスト完了、歌詞まわりは 2026-10-09 にユーザーがラボで軽く操作し
問題なしとの報告（網羅的な実機検収ではない）。
担当（2026-09-23 決定、CLAUDE.md / AGENTS.md）: 設計・指示・検収 = ClaudeCode Opus 5.5 / high、
実装・テスト = Codex GPT-6.1 Sol / high、独立レビュー = 実装者と別の GPT-6.1 Sol / xhigh、
実機目視 = ユーザー。2026-09-07〜09-09 の Astra / Sol 体制の記録は
[引き継ぎ監査](music-video-handoff-20260907.md) に残す。本書の過去の完了記録は当時の記録であり、
監査で挙がった未解決事項の解消を意味しない。
H3 の解析一致、編集済み Project から MP4 への導線、P4 は未完了。
開発場所 = 別 worktree `C:\home\mimageviewer-musicvideo`（ブランチ `musicvideo`）。
最終着地点 = **mIV に同梱して配布し、mIV から起動する機能**（ラボ止まりにしない。D10）。

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
`evaluate` は純粋関数でユニットテスト対象、`rasterize` は CPU ラスタライザ（D4）で、決定性テストと
フレームハッシュの比較対象。

### D3. 決定性 — フレームは時刻 t のみに依存する

- 書き出しのフレーム n は `t = n / fps`。乱数・実時間・フレームレート変動に依存させない。
- `SpectrumAnalyzer` は多重解像度窓の内部状態（`last_center_frame`）を持ち、**呼び出し順に依存する**。
  よって書き出しは 1 本の analyzer を **t 昇順**で回す。プレビューでシークした場合は
  analyzer を作り直す（不連続 = リセット）。時間方向の平滑化（§1.4 の smoothing factor 0.7）も
  同じ理由で「t 昇順の逐次適用」を前提にする。
- 受け入れ基準: 同一プロジェクトを 2 回書き出したとき、全フレームがビット一致する。
- 帯域エネルギー／立ち上がりモジュレータも同じ前進専用規約で評価する。値は PCM と t のみに
  依存し、シーク時は解析器と EMA / エンベロープ／直前入力の状態を一緒にリセットする。
  自動較正はデコード済み全尺を固定 60Hz で一度走査したパーセンタイルだけから決める。
  レベル値は 5–95%、疎な正変化値は 50–99% を既定にし、結果とパーセンタイルは UI で確認・
  手動上書きできる。

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
取り出し、変調のために解析を重複させない。密なミックスでキックを連続低音から分離する場合は、
同じ帯域値の正の時間差分を取る `BandOnset` を使う。表示用スペクトラムの周波数軸自体は従来どおり
半音軸を既定とする。

キック代替として使う幅広い帯域では、中心周波数だけで長い窓を選ぶと 40–120Hz が 683ms 窓に
入り、過渡音より約 300ms 早く反応し得る。任意範囲 API は、その範囲を 4 FFT bin 以上で積分できる
既存窓のうち最短を選ぶ。40–120Hz は 4096 samples（48kHz で約85ms）となる。5 本の窓は通常解析で
既に更新済みなので追加 FFT は無く、半音 bands / notes の窓選択も変えない。

いずれの方式でも「バンド境界の列 → 帯域パワー → 表示値」という同じ経路を通し、
**直線表示と円形表示は同じバンド列に対する座標変換違い**にする（円形専用の解析を持たない）。

### D6. 鍵盤表示のロジックは core へ切り出して共有する

配置・輝度・低域整理に加え、五度圏ベースの鍵盤色・スペクトラム色も `music-core` の純関数に置き、
**mIV の音楽ビューと書き出しの両方が同じ関数を呼ぶ**。egui 描画をそのまま書き出しへ持ち込まない。
書き出し側はレイヤごとにカスタム Fill / mIV 配色を選択できる。鍵盤の表示ゲインはプロジェクトに
`display_gain_db`（既定 0 dB）として持ち、音声波形そのものを変えず `prepare_keyboard_frame` に渡す。

横長スペクトラムも同じ共有方針とする。対数周波数軸、trail / onset の状態遷移、ghost → trail →
現在値 → onset の描画プリミティブ列は `music-core::spectrum_visual` が所有し、本体の egui 描画と
書き出しの CPU ラスタライザはその同じ結果を描く。状態は音声時刻の前進時だけ更新し、シーク時に
リセットする。書き出し側の従来の等間隔 Bars は別の選択肢として維持する。

### D6a. 初回リリースではビート同期を作らない

利用者判断（2026-09-05）により、**ビート検出・四分音符単位の同期は初回リリースの対象外**とする。
対象ジャンルではキック帯域の立ち上がり変調でほぼ代替でき、検出結果が正しいかを調整する UI を
初回から増やすより、40–120Hz の正の変化量を直接観測して使う方が確実だからである。

将来ビート同期を追加する場合も、P3c のプロジェクト単位モジュレータとレイヤ binding の配管を
そのまま利用できる。解析側には `estimate_simple_beat_grid` による推定が既にあり、
`BeatTrackingStatus::UserCorrected` も定義済みだが、現時点では未使用である。再開時は自動推定だけで
完結させず、この手動補正状態を実際の編集 UI と保存形式へ接続する。

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

### D10. エディタは mIV とは別の exe として動かす（2026-10-09 改訂）

旧版は「本体の独立した編集モード」だった。P4 の具体化にあたり、次の比較で**別 exe** に改める。

**前提の確認: プレビューは動画再生ではない。** mIV の native presenter（独立 HWND + D3D11 swap chain）は
動画再生セッション（デコーダ・時計・HUD・配置）と一体の経路で、主目的は 4K HEVC を GPU デコードから
ゼロコピーでカクつかず出すこと（[video-architecture.md](video-architecture.md) 設計目標）。CPU の RGBA を
受ける経路も持つ（`render_core.rs` の CPU upload）ので「CPU 映像を扱えない」わけではないが、エディタの
プレビューは画像と音声から CPU ラスタライザが合成した RGBA（D1）で、再生セッションも GPU デコードも
無いため、その経路に載せても得るものが無い。
RGBA を egui テクスチャで表示する現行方式で足り、P3 で鍵盤入りシーンが 77〜80fps だった。mIV の
音楽ビュー（鍵盤・横長スペクトラム）も egui 描画で、native なのは動画フレームだけ。
**UI を native で作り直す必要はなく、egui のまま進める。** 将来動画素材のレイヤーを足す場合も、
書き出しが CPU 合成なので、デコード結果を CPU 側へ取り出してラスタライザへ渡す形になり、
native presenter は使わない。

| 案 | 内容 | 判断 |
| --- | --- | --- |
| A | 本体ウィンドウを編集モードへ切り替える | 描画は可能。App の状態・keymap context・閲覧への復帰の遷移を本体に足す |
| B | 同一プロセスの別 viewport | **不採用**。detached viewer リワークの凍結領域、viewport ごとの IME（`update_ime_state`）と keyboard owner、動画エンジンと cpal プレビューの同居が重なり、状態の組み合わせが最大 |
| C | 別 exe を mIV から起動 | **採用**。実行時の状態を一切共有しない。CLAUDE.md「状態の組み合わせを減らす」に最も合う |

境界（C の契約）:

**起動と引数**

- **mIV → エディタは起動引数だけ**: `--audio <path>`（新規プロジェクト）または `--project <path>`、
  および `--data-dir <dir>`。mIV は解決済みの**絶対パス**を渡す（エディタは相対パスを
  エディタのカレントディレクトリ基準で解決するが、mIV からの起動では使われない）。起動後の IPC は
  持たない。エディタ → mIV の通知も無い。
- mIV 側の exe の存在確認と spawn は worker で行い、UI スレッドを止めない。失敗は理由付きで通知する（P4d）。
- `--data-dir` は mIV が使っている data dir をそのまま渡す（通常版 `%APPDATA%\mimageviewer`、
  portable は `<exe_dir>\data`、`--data-dir` 指定起動ならその値）。エディタは自分で data dir を
  推測しない。`--data-dir` 無しの直接起動（開発時）は「最近使ったプロジェクト」とファイルのログを
  永続化せず、ログは標準エラーだけに出す（ユーザーが選んだプロジェクト・MP4・LRC の保存は通常どおり）。
- 引数の誤り（存在しないパス、両方指定など）はエディタのウィンドウ内で理由を表示し、空のプロジェクトで
  開く。終了コードだけで知らせない（GUI subsystem のため）。

**エディタが書くもの**

- ユーザーが選んだファイル（プロジェクト JSON・MP4・LRC）。保存は同じディレクトリの一時ファイルから
  置き換える既存方式（atomic 保存用の一時ファイルが一時的にできる）。
- H2 の素材履歴が使う匿名の一時 blob（`resource_blobs.rs`、OS の一時領域）。
- ログ: `<data_dir>/logs/musicvideo.log`（追記。起動時に 4 MiB を超えていれば 1 世代だけ退避）。
  複数のエディタが同時に追記した場合の行の混在は割り切る。mIV の `mimageviewer.log` / `panic.log` には
  書かない・切り詰めない。本体 lib に依存しても本体の logger と panic hook は `run()` 内で登録される
  ため自動では効かない。エディタは自前の panic hook で `<data_dir>/logs/musicvideo-panic.log` へ追記する。
  worker の切断（送信側の消失）は既存の pending 処理と同じく UI に理由を出す。
- 最近使ったプロジェクトの一覧（下記）。
- mIV の settings.db・キャッシュ DB は開かない。

**エディタの保存データ**（Q10 決着）: 最近使ったプロジェクトのパス一覧だけを
`<data_dir>/musicvideo/recent-projects.json` に持つ。書き出し設定の既定などその他は初版では保存しない。

- 一覧に入れるのは、**読込が採用された時点**（読込が成功しても、未保存確認や後続の操作で破棄・保留される
  ことがある。ラボ `app.rs` の読込完了処理）と**保存が成功した時点**の、実際に使った**解決済みの絶対パス**。
  要求の開始時点では入れない。取消・失敗・破棄された読込は入れない。
- 一覧の読み書きと「一覧から開く」ときの存在確認は**ホストの worker** が行う。UI スレッドから同期で
  ファイルに触れない（data dir が遅いドライブでも止まらない）。
- 書き込みは書込ごとに固有の一時ファイルからの置き換え。複数のエディタが同時に書いた場合は後勝ちで
  割り切る（Q9 と同じ方針）。件数上限で古いものを落とす。
- 一覧の書き込み失敗はプロジェクトの保存成功を覆さない。通知とログだけ。読めない・壊れている一覧は
  空として扱い、ログに残す（作り直せるデータ）。
- 一覧から開くときに無いファイルは理由を表示し、一覧から外せる。

**未保存の編集と終了**（独立レビュー P1 対応）

ラボの読込完了判定は「読込開始後の変更」を見るだけで、開始前から未保存の Project を守る確認ではなく、
終了処理も worker の取消だけ（`app.rs` の `Drop`）。製品では次を定義する。自動保存・クラッシュ復元は作らない。

- **開く・新規・最近使ったものから開く・ウィンドウを閉じる**の前に、未保存なら「保存 / 破棄 / キャンセル」
  を出す。「保存」は通常の保存（未保存の新規なら保存先選択）を経る。失敗・キャンセルなら元の操作を中止する。
- **保存が成功しても、そのまま続行しない。** 保存済みの基準は保存開始時の Project なので
  （`musicvideo-ui` `document.rs` の `complete_save`）、保存中に採用された LRC・素材の完了などで現在の
  Project が再び未保存になっていることがある。保存の完了後に**同じ判定をやり直し**、未保存なら再度確認を
  出す。閉じる操作も同じ（保存中に閉じる操作が来たら、保存の完了を待ってからやり直す）。
  4 つの操作すべてがこの 1 つの判定を通る。
- **MP4 書き出しはモーダル**にする（CLAUDE.md「長い処理はモーダルにして割り込みを消す」）。書き出し中は
  編集・開く・保存を受け付けない。閉じる操作は「書き出しを中止して閉じますか」を出し、承認されたら
  既存のキャンセル（書きかけファイルの削除）を完了させてから未保存判定へ進む。
- 既存の H1 / H2 の session・revision・完了所属の管理（読込中の編集保護、保存完了の所属）は維持する。

**音声再生の所有**（独立レビュー P2 対応）

`cpal::Stream` は `!Send`（`src/video/audio.rs` 冒頭の注記と同じ制約）なので、worker で作った
`AudioPlayer` を UI へ返す形は取れない。専用の音声スレッドが stream の生成・維持・drop を所有し、
UI は操作コマンドの送信と再生位置のスナップショット読み取りだけを行う。デバイス照会・stream 生成は
このスレッドで行い、UI スレッドから同期で呼ばない。f32 以外の出力形式（i16 等）も変換して受ける。

**ライフサイクルと runtime**

- ライフサイクルは独立。mIV を終了してもエディタは閉じない。同じプロジェクトを複数のエディタで
  開くことは止めない（Q9 決着: 最後に保存した方が残る）。
- 配置: launcher 版は `runtime/<version>/` の core の隣、portable / 開発ビルドは core exe の隣。
  FFmpeg DLL は exe 同居で解決される。
- runtime 清掃からの保護: 清掃 worker の process 画像判定は exe 名ではなく画像パスが対象 tree 配下かを
  見るので、**清掃開始時に起動済みの**エディタは守られる（`runtime_cleanup.rs`）。ただし一回の
  snapshot なので、その後に起動するエディタまでは保証しない。そこでエディタも core と同じ
  **版ディレクトリの shared lease**（`runtime_locks::shared`）を起動直後に取り、終了まで保持する。
  独自の IPC や再試行は作らない。portable / 開発ビルドは runtime 外なので lease を取らない。
  spawn した core は自分の lease を保持したまま起動するので、core が生きている間は版が守られる。
- **残る競合**: core がエディタを spawn した直後に終了し、かつその間に**別の版の** mIV が起動して清掃を
  始めると、エディタが lease を取る前に清掃が排他 lease を取れる（清掃のプロセス列挙は 1 回だけで、
  列挙後に生まれたエディタは入らない。`runtime_cleanup.rs`。`runtime_locks::shared` は待たずに失敗する）。
  清掃は起動した版と異なる版を対象にする（更新からの経過時間は見ない。`runtime_cleanup.rs`）ので、
  **異なる版の清掃と、エディタの起動・core の終了が重なる場合**に起きる（更新直後に限らず、旧版の
  core を長く起動したままにしていた場合も含む）。
  **割り切る（2026-10-09 ユーザー判断）**: エディタは lease を取れなければ編集を始めず、理由を表示して mIV から
  開き直すよう案内する。起動時の取得通知と、それまで core を終了させない協調は作らない。

**設定・入力**

- 設定は共有しない: テーマは OS に従う、UI フォントは Yu Gothic 既定、**キーのカスタマイズは
  当面実装しない**（2026-10-09 ユーザー判断。常用ツールではないため）。エディタのショートカット
  （Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z 等）は固定で、`KeyAction` 外とする理由を
  [keymap-spec.md](keymap-spec.md) に記録する（P4f）。
- **IME は本体と同じ補正を効かせる**（独立レビュー P2 対応。旧記載「問題が出たら判断」を改める）。
  本体の `ime_focus` は便利機能ではなく、egui が IME 変換中の Esc で TextEdit のフォーカスを外す・
  `Disabled` だけでは未確定文字が消えない、といった既知の入力破壊を補正している。共有 UI
  （`musicvideo-ui`）の TextEdit にも同じ補正が要る。ただし `ime_focus` は `pub(crate)` で、
  `keyboard_input` の keyboard owner と本体 logger に結びついているため、そのままは使えない。
  egui だけに依存する入力補正（plugin）と single-line helper を共有クレートへ切り出し、本体と
  エディタのホストがそれぞれ policy を登録する構成を **I1** で設計する（出荷済みの本体の入力処理に
  触れるので、挙動不変の切り出しとして独立レビューを通す）。検証対象は IME の Esc 取消、候補選択中の
  Tab、Enter 確定、フォーカス復帰と Undo の所有。2026-10-09 のラボでの短い歌詞入力確認は、
  この範囲の保証にはならない。

#### I1 設計（2026-10-09 案）

段 1 の調査・設計案は `bcddec29b`。独立設計レビュー（GPT-6.1 Sol / xhigh）で
**段 2a は ACCEPT**（設計・実装とも、実装は `367f9c9d3`）。段 2b はエディタ専用 Tab / Enter policy に
P2 が 4 件あり（grace による通常 Tab/Enter への影響、Commit と Enter の別 pass、`wants_keyboard_input` が
text owner でない、recovery claim の根拠キーの除去）、**利用者判断（2026-10-09）で方針を変更した**:

- **エディタも mIV 本体と同じ扱いにそろえる。エディタ専用の Tab / Enter policy は作らない。**
  下の §3 の「エディタ専用 policy」は採らない（記録として残す）。
- **Tab traversal は本体と同じく全域で無効**（Tab で入力欄を移動しない。案 A）。本体の
  `egui_focus_policy`（全域 Tab policy）と同じ挙動をエディタのホストにも登録する。共有 helper の
  grace を含む Tab lock と食い違わないため、レビュー P2-1 の通常 Tab への影響が消える。
- single-line 5 欄は共有 helper、歌詞 multiline は plugin のみ（buffer・widget ID・native Undo 維持）、
  Hz は DragValue 維持。
- IME 中・grace・single-line 復帰前の Project Undo 抑止（Undo gate の拡張）は維持する（レビューで妥当）。
- Commit と Enter が別 pass で届いた場合の歌詞の余分な改行は、本体の multiline と同じ性質として受け入れ、
  実機で発生したら原因を調べて対応する（発生は未観測。レビューの指摘は設計上の反例）。
- 段 2b の実機確認に detached（F12 別窓）を加える必要はない（エディタは別 exe で、本体の viewport に
  触れない）。段 2a の本体実機確認には detached を含める（レビュー P3-5）。
本体の補正を挙動不変で切り出す部分と、エディタへ補正を適用する部分を分ける。
以下の file:line は 2026-10-09 の調査時点のソースを指す。

**1. 共有境界と依存**

新設は `crates/egui-ime/`（package `egui-ime`、Rust 名 `egui_ime`）。通常依存は
`egui = "0.33"` だけとし、eframe、本体 lib、keyboard owner、logger、Windows API に依存しない。
標準ライブラリの `HashMap` / `Instant` / `Duration` を使う。workspace member に加え、本体と
`musicvideo-ui` が path dependency を持つ（段 2a では本体のみ、`musicvideo-ui` は段 2b）。
workspace root の `[patch.crates-io]` は既に
`egui = { path = "vendor/egui" }`（`Cargo.toml:34`）、本体・共有 UI・ラボは同じ 0.33 系
（`Cargo.toml:366`、`crates/musicvideo-ui/Cargo.toml:12`、`tools/musicvideo_lab/Cargo.toml:11`）。
新クレートだけ別の egui を解決しないことを `cargo tree -p egui-ime` / `cargo tree -d` で確認する。
`musicvideo-core` / `music-core` には egui 依存を追加しない。

| 現要素・根拠 | 配置 | 理由・維持する契約 |
| --- | --- | --- |
| `ImeInputPlugin`、`install_ime_input_policy`、`normalize_ime_input`、`apply_ime_event`（`src/ime_focus.rs:18`〜140） | 共有 | `Memory::begin_pass` 前の唯一の composition owner。IME event の順序、composing 中の Esc **press だけ**の除去、取消時の空 preedit 挿入をそのまま移す |
| per-viewport `ImeFocusState` と 300ms grace（同:13、42） | 共有 | Context 内の `ViewportId` ごとに状態を保持。`< 300ms`、`Instant`、非空 Commit の判定を維持。grace を Esc 除去条件へ混ぜない |
| `ime_input_active` / `ime_input_active_in_viewport` / `ime_composing_with_pending_events` / `ime_input_active_with_pending_events`（同:610〜669） | 状態読取・pending event の純投影は共有、現在 viewport の診断開始は本体 facade | 他 viewport の read は queue・composition を更新しない。pending native event は queue が所有し、共有側はコピーした状態への投影だけを行う |
| focus retry、`ImeTextFocusContract`、`focused_text_input` / `recovering_text_input`、`keyboard_focus_recovery_input`、remember / forget（同:148、251、671〜743） | 共有 | widget / viewport / pass の focus 契約が helper と入力所有の共通の根拠。直前 1 pass の recovery、pointer / touch と既存の別 widget focus の優先を維持 |
| `restore_focus_for_ime_key`（同:778） | 共有 | 外部 `focus_request` と内部 retry の選択、focus lock、復帰時の repaint を維持。新しい retry・待機・猶予は追加しない |
| `show_singleline` / `add_singleline` / `add_singleline_sensitive` / `add_sized_singleline` と内部 policy variant（同:832〜922） | 描画・補正は共有、本体の同名入口は薄い facade | `dyn TextBuffer`、configure closure、Response / TextEditOutput、サイズ指定を維持。widget 描画前に IME activity を採取する |
| `TextInputDiagnosticKeyPolicy`（同:141） | 共有の中立 metadata | sensitive helper の focus 契約に Standard / Sensitive を記録するため必要。キー収集・マスク・ログは共有しない |
| `begin_pass_diagnostics`、`TextInputKeyDiagnostic`、`observe_helper_widget`、整形・異常判定・文字マスク・ログ予算（同:165〜609） | 本体 | 本体の診断責務。keyboard owner の型、通常 1 MiB / 異常継続、Sensitive の全 redaction、次 pass の finalize と repaint を維持 |
| `record_keyboard_owner` / `record_side_panel_close`（同:570、589） | 本体 | App の所有決定とパネル寿命を診断へ記録する。本体固有の型・call site を共有しない |
| `src/egui_focus_policy.rs` の全域 Tab policy | 段 2b の利用者判断により共有へ移設、本体は facade | mIV は非 IME 時も全域で Tab traversal を無効化し、キー event は keymap に残す。IME helper の field-level lock とは別の製品仕様。同じ実装を本体とエディタのホストが登録する（冒頭の決定が旧案に優先） |
| App の `update_ime_state` と各 viewport 入口、native presenter の登録・pending event 経路 | 本体の既存位置 | 所有 viewport の pass で更新し、他 viewport は read だけという境界を維持。入力配送や detached 述語を変更しない |

plugin type は共有側の 1 種だけにする。既存の debug name、temp ID の文字列と
`(viewport, widget)` のキー構成を変えない。再登録は `Context::add_plugin` の type 単位の
冪等性を使い、composition を再初期化しない。Context の破棄で状態が破棄されること、
別 Context / viewport の読取・描画が sibling の状態を書き換えないことを維持する。
抽出と同時に viewport map の掃除や新たな focus 状態機械を導入しない。

**2. 本体固有部分の接続と呼び出し互換**

接続は **本体 facade が共有 helper の前後で診断を行う方式**を推奨する。
共有側から本体へ呼び返す callback / trait は不要。本体が共有側の結果を観測するだけにし、
Context にホスト callback を保存する仕組み、二重の focus 契約、keyboard owner の変換型を作らない。

- 共有側に read-only の `text_focus_contract_snapshot(ctx)` を設け、現在 viewport の
  widget ID / focused pass / Standard・Sensitive をコピーして返す。診断は focus 消失後にも
  直前の契約が必要なので、`focused_text_input` の結果だけでは代用できない。
  snapshot は既存契約の投影であり、共有 state を変更する setter は公開しない。
- 本体 `begin_pass_diagnostics` はこの snapshot を読み、既存の started-pass 検査、前 pass の
  finalize、キー採取を行う。本体 `ime_input_active` は従来どおり先に診断を開始してから
  共有側の activity を返す。他 viewport / pending event の読取入口には診断を追加しない。
- 本体の 4 種の public(crate) helper は従来の引数・戻り値を保ち、先に診断開始、次に共有 helper、
  最後に `observe_helper_widget` を呼ぶ。呼び出し元に対する raw TextEdit 入口は復活させない。
  現状の observe は復帰後・remember / forget 前（`src/ime_focus.rs:822`）、facade では
  remember / forget 後になる。この 2 処理は別の temp ID を操作し、observe は focus と widget ID
  だけを見るため結果は同じ。診断の field_seen / field_id_after / focused_after を adapter test で固定する。
- owner の決定は今の App / `keyboard_input` が行い、その結果を本体の
  `record_keyboard_owner` に渡す（`src/app.rs:21059`）。共有側に owner 決定を移さない。
  logger は本体の `log_key_diagnostic` だけから呼ぶ。
- `src/ime_focus.rs` はモジュール名を維持する。副作用のない predicate / projection は
  `pub(crate) use egui_ime::...` で再エクスポートできる。診断を伴う activity / helper は
  上記 facade が必要なので、全関数の単純な再エクスポートは採らない。
  `src/` の **35 ファイル**に `crate::ime_focus` の参照があり（自身のテストを含む）、
  既存の呼び出し元を書き換える必要はない。
  plugin 未登録の `Context::default()` でも本体 helper の診断が動く現状を維持し、facade が
  勝手に plugin を登録する変更はしない（Sensitive の既存テストはその条件を使う）。
- 本体ホストの `install_ime_input_policy` は共有 plugin 登録へ委譲し、全域 Tab policy の
  登録順は今のまま（`src/lib.rs:1695`、native は
  `src/video/native_presenter/render_core.rs:9245`）。App の viewport 入口も変更しない。
  IMM32 の 2 経路・TSF 非採用・未対応範囲の本体固有 doc comment（`src/ime_focus.rs:758`）は
  facade に残し、共有側の helper doc はホスト非依存の focus 契約を説明する。

簡素化の検討: I1 は短い同期の入力変換で、保存・変換のような長い非同期処理ではないため、
モーダル化や窓の再生成は不要。1 つの plugin owner と既存 focus 契約を再利用し、
登録 callback・独立した editor composition flag・新しい復旧機構を増やさない。

**3. エディタ側の適用と前提の訂正**

`musicvideo-ui::install_input_policy(ctx)` をホスト用の登録入口として追加する案。
この入口が共有 plugin とエディタ専用の `on_begin_pass` policy を冪等に登録する。
ラボは `tools/musicvideo_lab/src/project.rs:55` の `setup_egui` から呼ぶ。
通常 GUI と preview benchmark の creation closure の両方がこの関数を通る
（`tools/musicvideo_lab/src/main.rs:158`、181）。headless handler / layout test の Context も
同じ入口で登録し、UI 描画中の遅い登録を成功条件にしない。P4b のホスト移設後も登録入口を維持する。

single-line はレイヤー名（`crates/musicvideo-ui/src/lib.rs:1327`）、テキスト本文（同:1506）、
モジュレータ名（`crates/musicvideo-ui/src/modulators.rs:57`）、音名範囲の上下限（同:216、220）の
**5 構築箇所**を共有 `add_singleline` へ替える。余分な Ui scope / container を足さず、
auto ID の採番、既存の親 layer / modulator ID、幅 56pt、configure と `.changed()` の扱いを保つ。

指示書の **「Hz 範囲の single-line」には相違がある**。
Hz は `DragValue`（`crates/musicvideo-ui/src/modulators.rs:179`〜199）、raw TextEdit 2 箇所は
`BandRange::Semitone` の `C1` / `C#4` 等の音名入力（同:200〜236）。Hz を String 欄へ置き換えると
ドラッグ・数値範囲・clamp の既存機能が失われるため、`DragValue` を維持する。
内部の TextEdit には Context の plugin / editor policy が効く。数値編集モードの Esc / Enter / Tab
も回帰対象にする。raw TextEdit 禁止検査は vendor 内部の TextEdit を禁止しない。

歌詞は **raw multiline を理由付きで維持**する。
`LyricsTextBuffer`、`editor.widget_id(layer_id)`、`.desired_rows(7)`、各レイヤーの native Undo と
sidecar の identity / cue time を変えない（`crates/musicvideo-ui/src/lib.rs:1682`〜1708、
`crates/musicvideo-ui/src/lyrics_editing.rs:308`、338、1345）。plugin が挿入する空 preedit も
`event_may_mutate_text` の対象になるため、既存の rollback capture → begin_pass → TextEdit →
native reachability refresh → finish_pass の順序を保てる。I1 で buffer を String に替えたり、
widget generation / native history をリセットしたりしない。

plugin 登録だけで歌詞の **Esc / 空 preedit** は補正できる。既存
`raw_multiline_text_edit_keeps_focus_and_cancels_preedit_across_delayed_disabled`
（`src/ime_focus.rs:1484`）がその経路を検証している。ただし全操作の保証にはならない。
egui の multiline は `tab: false`（`vendor/egui/src/widgets/text_edit/builder.rs:141`）で、
Tab traversal は描画前に決まる（`vendor/egui/src/memory/mod.rs:553`）。また Enter を改行として
扱い、IME event があるだけでは除去しない（builder 同:1006、934、1142）。
**plugin のみで Tab / Enter も安全という前提は採らない**。

エディタ専用 policy は最初の widget 登録前の `on_begin_pass` で次を行う案とする。
これは本体の切り出しへ混ぜず、段 2b の独立レビュー対象とする。

- IME がキーを所有する pass を「現在 composing、またはこの pass の入力に Ime event がある」と
  既存 snapshot / event から判定する。composition が同 pass で終わる Commit / Disabled も含める。
  composing は共有 `ime_composing_with_pending_events(ctx, ctx.viewport_id(), &[])` で読める。
  新しい bool / pending flag / 時間窓は保存しない。**300ms grace だけでは TextEdit の Enter を消さない**。
- 上記 pass で `wants_keyboard_input()` または helper の recovery claim があるときだけ、
  Tab の focus direction を `None` に戻し、Tab / Enter の press を egui の入力から取り除く。
  release と Ime event は残す。OS の候補選択・確定は既に処理された入力であり、egui の
  traversal / 改行 / 数値欄確定へ同じ press を渡さないための処理である。
  Tab は event を消すだけでは描画前に決まった方向が残るので、両方の処理が必要。
- 非 IME の Tab 移動、歌詞の Enter 改行、数値欄の確定、pointer による focus 移動は維持する。
  本体の「常時 Tab 無効・event は残す」policy は使わない。
  multiline の Esc loss は pre-input plugin、Tab loss はこの描画前 policy で発生元から防ぐ。
  Enter は multiline の focus を外さないため、歌詞に single-line の post-draw recovery / retry を
  追加する必要はない。追加が必要という反証が handler test で出たら、場当たり的 latch を足さず
  設計担当へ返す。single-line の復帰は共有 helper が所有する。

`App::update_ime_state` 相当の **composition 更新処理はエディタには不要**。
現在の本体関数も診断を開始する read-only の入口で、状態更新は input plugin にある
（`src/app.rs:20938`、`src/ime_focus.rs:67`、610）。エディタは診断を開始しない。
policy が `on_begin_pass`、Undo gate が `begin_frame_with_input_enabled` の冒頭側で読むため、
TextEdit の event 消費前に判断できる。追加 viewport / Context ができた場合も、Context への
登録と所有 viewport の pass での読取を守り、別 viewport の queue を更新しない。

**Undo の所有**: `apply_history_shortcut` は UI 描画前に実行される
（`crates/musicvideo-ui/src/lib.rs:794`〜796）。現状の gate は `wants_keyboard_input()` のみ
（同:1185）で、名前に IME を含む既存テストも bool を渡す純関数検査である（同:3466）。
新 gate は `wants_keyboard_input() || egui_ime::ime_input_active(context) ||
egui_ime::recovering_text_input(context).is_some()` とする。
これにより IME 変換中・300ms grace・single-line 復帰前は **Project の** Ctrl+Z / Ctrl+Y /
Ctrl+Shift+Z を発火させない。gate で戻るときは event を consume せず、TextEdit / IME の native
Undo 所有を保つ。IME 中の native Ctrl+Z を一律に消す処理は加えない。
既存のモーダル時 `input_enabled` gate、Undo ボタン、Project 履歴の集約は変更しない。
本体 keymap の移植はせず、固定 shortcut の理由は D10 / P4f の方針に従う。

**4. 既存テスト・禁止検査の移設**

現 `src/ime_focus.rs` の `#[test]` は **23 件**。assertion を弱めず、次の対応表で全件維持する。

| 件数 | 段 2 の配置 | 対象 |
| --- | --- | --- |
| 16 | `egui-ime` の unit test | viewport read の分離 / root tracking / pending event 純投影の 3 件、composing Esc・非 composing / grace Esc・空 preedit・Windows 取消列・非空 Commit・空 Commit・別 viewport Disabled・raw multiline 遅延 Disabled の 8 件、IME Tab・描画前 start 採取・矢印・正当な focus 移動・Enter / Esc 復帰の 5 件 |
| 5 | 本体 `ime_focus` | 診断 identity / focus / owner / close site、文字マスク、非文字キー、Sensitive redaction、通常ログ予算。Sensitive は本体 facade と共有 focus 契約の結合も検証する |
| 1 | 本体 `ime_focus` | `tab_without_ime_keeps_editing_field_focus_and_input_working`。本体固有の全域 Tab policy と共有 helper の統合なので共有へ移さない |
| 1 | 本体 `ime_focus` | `raw_text_edits_are_ime_aware_or_explicitly_exempt`。本体 source と理由付き allowlist を本体に残す |

移設で本体 helper 自身・共有へ移す multiline fixture の raw 箇所が本体からなくなるため、
allowlist の該当行だけ削除する。他の理由付き exemption、expected occurrence の厳密検査は維持する。
共有クレートにも helper 構築境界 / 低レベル fixture の理由・件数検査を置き、移設で検査を逃がさない。
`musicvideo-ui` にも source scan の raw single-line / multiline 禁止検査を加える。
歌詞 multiline は path / anchor / 1 件 / 独自 buffer と native Undo を保つ理由で許可し、
低レベルの native Undo fixture も個別の理由と件数を持つ。tests module 全体を丸ごと免除しない。
本体の keymap / App / fullscreen / native presenter に既にある関連テストも残す。

新しい本体 adapter test で、各 helper の Standard / Sensitive 契約、描画前診断開始、
widget 観測と次 pass finalize、plugin 二重登録後の composition 継続、
plugin 未登録 Context の従来動作、現在 viewport の診断と explicit viewport read の無副作用を確認する。
ログ文面、redaction、予算は本体の既存 5 件の証拠を再利用する。

**5. リスクと段 2 の検証計画**

主要リスクは plugin type の二重化、facade による診断開始順の変化、契約 snapshot の age / viewport
取り違え、Sensitive metadata の喪失、helper 置換による auto ID / native Undo の変化、
エディタ専用 Tab / Enter policy の非 IME 操作への漏れ。共有 crate 内へ本体の型を引き込むことも
依存境界の違反になる。切り出しでは input normalizer・grace・復帰条件・repaint を変更しない。

| 回帰対象 | handler / headless 検査の期待結果 |
| --- | --- |
| Esc 取消 | 5 single-line と歌詞で Preedit → Esc press/release → 遅延 Disabled、および `Disabled, Disabled, Commit(""), Disabled, Disabled`。未確定文字が残らず、既存本文・歌詞 identity / cue time は維持 |
| Tab 候補選択 | Enabled / Preedit と同 pass の Tab、継続 pass の Tab / Shift+Tab / repeat、Commit と同 pass の Tab。focus が sibling 欄へ移らず、歌詞へ tab が入らない。IME 外では通常の Tab traversal を維持 |
| Enter 確定 | Key と非空 Commit の両順序、同 pass の start / commit、Commit と Key が別 pass。確定文字は一度だけ入り、歌詞へ余計な改行を入れない。IME 終了後の通常 Enter は歌詞改行・single-line / DragValue 確定として残る。grace だけの Enter を widget 側から消さない |
| single-line focus 復帰 | 描画前に focus を失う経路、外部 latch 有無、1 pass recovery の失効、pointer / touch / 他 widget focus を優先。先に走る Project shortcut へ recovery key が漏れない |
| 歌詞の所有 | 既存 `editor_frame_ime_replacement_keeps_the_timed_line_identity` と native Undo / global Undo / merge 取消 / layer switch / project replacement / layout 回帰を維持。plugin の空 preedit に既存 buffer の rollback / reachability が対応し、widget ID・native Undo graph を保つ |
| Undo gate | text focus 有無 × composing / grace / recovery / 通常で Ctrl+Z、Ctrl+Y、Ctrl+Shift+Z。抑止時に Project / 履歴・resource manifest が変わらず、event が native TextEdit に残る。grace 経過後は Project shortcut が復帰 |
| Hz の数値入力 | `DragValue` の text mode / drag / range / clamp、IME 中の取消・確定、IME 外の Enter / Tab を維持 |
| 所有境界 | 同 Context の root / child、および別 Context の入力・契約・診断が独立。二重登録、別欄への pointer 移動、欄が閉じる / 再表示される経路で sibling 状態をリセットしない |

別 pass の Commit / Key では、終了後の新しい Enter と IME が送る確定キーを source 情報なしに
区別できない場合がある。**これは実装前レビューで確認する未確定の入力配送論点**であり、
上の editor policy だけで全配送列が解決済みとはしない。300ms を widget キーの握り潰しへ転用せず、実機の配送列と headless
再現を照合する。余計な改行等が残るなら合格扱いにせず、入力所有境界の設計へ返す。

検証担当は段 2 の実装担当 1 人とし、独立レビューは記録された有効な結果を再利用する。
段 2a は `cargo test -p egui-ime --lib` → `cargo test -p mimageviewer --lib ime_focus::tests` →
`cargo test -p mimageviewer --lib egui_focus_policy::tests` / 関連 keymap・App / native テスト →
`cargo check -p mimageviewer --bin mimageviewer-core` の順。
段 2b は `cargo test -p musicvideo-ui --lib` と `cargo test -p musicvideo_lab`、ホストの check を加える。
本体 helper の利用が広いので、段 2a の区切りで
`cargo test -p mimageviewer --test ui_snapshot` と `.\scripts\test-full.ps1` を必須とする。
段 2b 完了時も追加された共有 UI / ホストを含む全体 gate を通す。
変更されていない snapshot の baseline を更新して入力問題を隠さない。
editor は既存 layout / native Undo headless 検査を維持し、見た目の変更が出る場合のみ snapshot を追加する。
fmt / glyph（UI string を変えた場合）を確認し、compile 済み tree から
`.\scripts\build-dev.ps1` と利用者が起動できるラボの build を準備する。検証対象 diff / command /
feature / 件数 / exit code を記録し、0 件一致・中断を成功としない。

実機は Microsoft IME のひらがな入力・候補 Tab / Shift+Tab・Enter 確定・Esc 取消・直後の通常入力・
focus 移動・Ctrl+Z/Y を、本体のメイン / fullscreen / native 動画の入力欄とエディタの 5 欄・歌詞・
Hz 数値欄で利用者に確認してもらう。歌詞は空行・時刻付き行を含む使い捨て Project で、取消前後と
native Undo 後に本文 / 行 identity / 時刻が保たれることを確認する。目安 20〜30 分。
この段階では起動・入力操作を行わず、段 2 完了後に成果物と手順を渡す。
本体の利用者向け起動コマンドは `Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`。
インストール版 / tray 常駐版を先に終了する必要があり、通常 `%APPDATA%\mimageviewer` を使って
実設定・データを更新し得ると明記する。エージェントによる実機確認を行うなら、別の明示了承済み
検証枠で disposable portable のみを用い、通常 profile の binary は起動しない。

**6. 段 2 の分割・規模・レビュー境界**

- **段 2a: 本体の切り出し**。共有 crate / workspace dependency、16 テスト移設、本体 facade と
  診断 snapshot 接続、禁止検査を作る。本体の利用箇所・viewport / native 配送は変更しない。
  共有 production code は約 450〜550 行、移設 test は約 750〜900 行、本体への新規 adapter test は
  約 100〜200 行。おおむね 6〜8 ファイル＋必要な依存 lock 差分。実装・自動検証で **1〜2 日**。
  独立 completion review と本体の挙動不変の証拠を確定してから 2b へ進む。
- **段 2b: エディタ適用**。5 欄の helper 置換、登録入口とホストへの接続、全域 Tab policy の
  共有化、Undo gate、raw 禁止検査、歌詞 / handler 回帰を作る。冒頭の利用者判断により
  エディタ専用 Tab / Enter policy は作らない。
  `musicvideo-ui` の lib / modulators / input policy / tests / manifest、ラボの setup / test の
  おおむね 7〜10 ファイル。production 追加・置換は約 80〜150 行、test は約 250〜450 行。
  実装・自動検証で **1〜2 日**。歌詞の buffer / widget lifetime を変える工数は含めない。
- 合計 **2〜4 開発日＋独立レビュー・長い build / 全体 gate・利用者実機確認の待ち時間**。
  数値は調査時点の概算であり、未検証の Windows 配送差が判明した場合は設計へ戻す。
  2a / 2b はどちらも coherent な差分としてレビューし、2a の成功で I1 全体を完了扱いにしない。
  実装前レビューでは境界・診断の順序・Tab / Enter の editor 限定・Hz 前提訂正を確認する。
  shared viewport 経路へ変更が必要になった場合は detached plan §2 の合意と §11 の記録も先に行う。

**7. 段 2a の実装・検証記録（2026-10-09、未コミット）**

段 2a の実装のみ。`crates/egui-ime` の通常依存は egui だけで、本体の path dependency と
workspace member を追加した。plugin、viewport 別 composition / 300ms grace、focus contract / retry、
pending event の純投影、4 種の single-line helper を移設した。plugin debug name、temp ID、
widget 描画前の activity 採取、取消・focus 復帰・Tab lock の条件は維持した。

`src/ime_focus.rs` は診断付き facade として残した。read-only contract snapshot を使い、
描画前の診断開始 → 共有 helper → widget 観測を接続する。診断・文字マスク・Sensitive redaction・
通常ログ予算・keyboard owner・logger・全域 Tab policy・viewport / native 入力入口は本体のまま。
本体の既存呼び出し元、`musicvideo-ui`、ラボには変更なし。エディタの登録・helper 置換・
Tab / Enter policy・Undo gate・禁止検査は段 2b まで行わない。

既存 23 テストは assertion を維持し、共有へ 16、本体へ 7（診断 5・非 IME Tab 1・raw 禁止 1）を
配置した。共有の raw TextEdit 理由・anchor・件数検査を 1 件追加し、本体の既存 allowlist は
移設された 4 箇所の exemption だけを削除した。本体 adapter test は 5 件追加し、4 helper の
描画前診断 / Standard・Sensitive / 未登録 Context / 次 pass finalize、contract の更新・消去後の
観測、IME 復帰と latch、explicit viewport / pending read の無副作用、plugin 再登録を検査する。

独立 completion review（GPT-6.1 Sol / xhigh）は **ACCEPT**。共有 Sensitive helper の説明が
ログまで担当するように読める P3 を修正し、metadata の記録だけで redaction / logger はホスト責務と
明記した。修正後の再レビューで P3 解消・追加指摘なし。抽出前後の production function body と
既存 23 test body の照合でも、意図した本体 hook の移動以外の挙動変更はないことを確認した。

検証ログと command / 時刻 / 所要時間 / exit code は `target/i1-2a/*.log` / `*.json` に保存した。
以下は段 2a の差分に対する非対話検証。snapshot baseline は更新していない。

| コマンド | 結果 |
| --- | --- |
| `cargo test -p egui-ime` | 17 passed、0 failed、0 ignored。doc-test 0 件 |
| `cargo test -p mimageviewer --lib ime_focus` | 12 passed、0 failed、0 ignored（既存 7＋adapter 5） |
| `cargo test -p mimageviewer --lib ime` | 380 passed、0 failed、既存 ignored 2 |
| `cargo test -p mimageviewer --lib egui_focus_policy` | 5 passed、0 failed、0 ignored |
| `cargo test -p mimageviewer --lib keyboard_input` | 16 passed、0 failed、0 ignored |
| `cargo test --test ui_snapshot` | 123 passed、0 failed、0 ignored |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0 |
| `cargo tree -p egui-ime` / `cargo tree -d` | exit 0。共有も vendored egui 0.33.3 を参照。egui / epaint / eframe の重複系統なし |
| `.\scripts\test-full.ps1 -SuppressCrashDialogs` | **PASS、exit 0**（1,400.87 秒）。workspace 12,627 passed / 58 ignored、vendor egui 25・egui-wgpu 11・eframe 18 passed。合計 12,681 passed / 0 failed / 58 ignored |
| `cargo fmt --check` | 最終実行 exit 0 |
| `python scripts/check_ui_glyphs.py` | 最終実行 exit 0、危険 glyph 0 |
| `.\scripts\build-dev.ps1 -WaitForOtherBuildsMinutes 0` | 最終実行 **DONE、exit 0**（302.17 秒）。通常 feature / `dev-runtime` profile の core・remote・EPUB worker を作成し、FFmpeg / VC runtime / EffeTune を staging。PE 検査 runtime 4 / PE 3 passed |

全体 gate の件数は各 target の最終 summary を集計し、子プロセスによる 2 件の再掲は重複加算していない。
本体 lib は 11,188 passed / 52 ignored。既存の ignored は維持し、full gate の crash-dialog
抑止設定も終了時に復元された。

検証ビルドの初回は `turbojpeg-sys` の CMake / MSBuild が exit 1（スクリプト exit 101）。
同じ CMake 構成を `--parallel 1 -- /v:diag` で単独ビルドすると exit 0、警告 0 / エラー 0
（124.33 秒）だった。初回失敗と同時刻に生成された低 CPU の MSBuild worker が残っていたため、
最終ビルドでは待機を 0 にし、`MSBUILDDISABLENODEREUSE=1`、`CARGO_BUILD_JOBS=24` を指定した。
完了した native 依存を再利用し、feature / profile / 最適化設定は変更していない。
途中の待機・単一並列の実行は中断しており、有効な最終結果は `build-dev-ready.log` / `.json`。
初回 MSBuild 失敗の詳細原因は未断定で、I1 のコード変更による compile / test failure とは扱わない。

実機確認は未実施。アプリ起動・入力操作・コミットは行っていない。
利用者向け成果物は `target\dev-runtime\mimageviewer-core.exe`。起動は
`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`。
通常 `%APPDATA%\mimageviewer` の実設定・データを更新し得るため、インストール版 / tray 常駐版を
先に終了する（single-instance mutex を共有する）。
本体のメイン / fullscreen / native 動画と F12 detached の入力欄で Microsoft IME の
ひらがな入力、候補 Tab / Shift+Tab、Enter 確定、Esc 取消、直後の通常入力、focus 移動、Ctrl+Z/Y を
利用者が確認する。detached の切替・閉じる / 再表示でも sibling の入力状態を保つことを確認する。
段 2b は未着手であり、I1 全体を完了扱いにしない。

**8. 段 2b の実装・検証記録（2026-10-09、未コミット）**

段 2a はその後 `367f9c9d3` としてコミットされ、独立実装レビュー ACCEPT（指摘なし）。
段 2b の Tab / Enter 方針は `49d994df3`。その後、下記のモジュレータ見出し ID 固定についても
利用者が同時修正を承認した。**段 2b の実装・自動検証は完了**。未コミット、実機未確認であり、
利用者による実機確認と最終受け入れは別途行う。

全域 Tab policy の production 処理を `egui-ime::tab_focus` へ移した。本体は再公開の facade とし、
登録順序・callback / temp ID・event 非消費・既存 5 テストを維持した。関数本体は公開範囲と
末尾空白を除いて移設前と一致。`musicvideo-ui::install_input_policy` で IME plugin と Tab policy を
冪等登録し、ラボの `setup_egui` と headless Context の生成入口に接続した。
single-line 5 欄を共有 helper に置換し、auto ID の割当方法、既存 Ui scope、幅、`.changed()` を維持。
歌詞の raw multiline / `LyricsTextBuffer` / widget ID / native Undo と Hz DragValue は維持した。
Project Undo gate に IME activity / 300ms grace / recovering owner を加え、gate で event を消費しない。
UI の通常依存に `egui-ime` を追加し、raw TextEdit を理由・anchor・件数付き allowlist で検査する。
allowlist は歌詞 multiline 1 箇所と既存 native Undo テスト fixture 2 箇所だけで、test module 全域の
免除は設けない。architecture overview も共有境界に合わせた。

**実装中に確認した前提の矛盾**: モジュレータ行は既存の `push_id(modulator.id.0)` 内でも
`CollapsingHeader::new(modulator.name.clone())` の既定 ID が表示名に依存する。
preedit で名前が変わると、次の pass で見出し・子 Ui・auto text widget の ID が変わり、
focused ID が `Some(87CC)` から `None` になる。実際のエディタを描画する handler テストで再現した。
helper 置換だけでは「変換中の取消でフォーカスを維持」の条件を満たせない。
修正候補は内側の見出しへ `.id_salt(modulator.id.0)` を 1 行追加すること。
既存 ModulatorId を使い、Ui scope と auto ID 採番数は増やさないが、見出し配下の widget ID 値は変わる。
初回は適合判断を問い合わせて停止した。利用者判断（2026-10-09）で同時修正を承認:
「auto ID を変えない」は入力欄の状態・native Undo・選択の対応を壊さないための条件であり、
この修正による UI 状態 ID の一度の変更は保存データ / Project 履歴に入らず、条件の趣旨に反しない。
承認どおり `.id_salt(modulator.id.0)` を追加し、初回の handler 3 件の失敗は解消した。

同型箇所を musicvideo-ui 全域で確認した。モジュレータ内側の見出し以外は該当なし。
外側「モジュレータ」/「変調の割り当て」は固定文字列、animatable property の見出しは
既に固定の (layer ID, property) salt を持つ。レイヤー名は layer-list の selectable label、timeline の
painter text / label、削除確認の表示だけで、入力欄を含む子 Ui の ID を名前から生成しない。
ComboBox の selected text は名前を表示するが ID は固定 salt を使う。scope / ScrollArea / lyrics
widget も既存の固定 ID を使う。追加回帰はレイヤー名とモジュレータ名の両方を実際に描画し、
通常入力を 1 文字ずつ、preedit を複数 pass、Commit / Disabled と idle pass を通して、
focus / helper contract / native TextEditState が同一 widget に残ることを確認する。

独立実装レビュー（GPT-6.1 Sol / xhigh）は **ACCEPT、P1 / P2 / P3 の要修正指摘なし**。
Tab 処理と既存 5 テストの移設前後照合、5 欄の採番 / scope、Undo 所有、類似 ID 箇所、
歌詞 production 処理の一致、全 GUI / benchmark / headless 登録入口、厳密な raw allowlist、
追加 13 テストと 6 変異の runtime ログが確認された。レビュー担当は read-only とし、
検証担当の成功結果を再利用した。全体 gate / release build は実装担当が完了を確認する。

追加は UI の handler 11 件、raw 禁止 1 件、ラボの setup 登録 handler 1 件（計 13 件）。
handler は 5 欄と歌詞の Esc 取消、Tab / Shift+Tab / 押し続けによる repeat、Commit / Enter の配送順、
single-line recovery、IME 中 / grace 中 / recovery-only の Ctrl+Z/Y と event 非消費、歌詞の cue / ID /
native Undo を検査する。歌詞は raw multiline の既存 Enter 処理を維持し、合成入力では
Commit + Enter が同じ pass に届く場合も別 pass の場合も改行が入ることを観測した。
本文の Commit は 1 回だけ適用される。新しい Enter filter は加えず、実機の配送は未確認。

検証ログ・command / exit code は `target/i1-2b/*.log` / `*.json` に保存した。

| コマンド | 結果 |
| --- | --- |
| `cargo test -p egui-ime` | 17 passed、0 failed、doc-test 0 件 |
| `cargo test -p musicvideo-ui` | **117 passed / 0 failed**（既存 105＋handler 11＋raw 禁止 1）。初回 113 / 3 の失敗は全件解消 |
| `cargo test -p musicvideo_lab` | 75 passed（既存 74＋setup 登録 1）、0 failed |
| `cargo test -p musicvideo-core` | 80 passed、0 failed、doc-test 0 件 |
| `cargo test -p mimageviewer --lib egui_focus_policy` | 既存 5 passed、0 failed |
| `cargo test -p mimageviewer --lib ime_focus` | 12 passed、0 failed |
| `cargo test -p mimageviewer --lib ime` | 380 passed / 既存 ignored 2 / 0 failed（子プロセスの再掲 2 件を重複加算しない） |
| `cargo test --test ui_snapshot` | 123 passed、0 failed。baseline 更新なし |
| `cargo fmt --all -- --check` / `python scripts/check_ui_glyphs.py` | exit 0 / 危険 glyph 0 |
| `cargo tree -p musicvideo-ui` | exit 0。eframe / cpal / ffmpeg / mimageviewer なし |
| `cargo tree -p egui-ime` / `cargo tree -d` | exit 0。egui は vendored 0.33.3 の 1 系統 |
| `.\scripts\test-full.ps1 -SuppressCrashDialogs` | **PASS、exit 0**（1,317.39 秒）。workspace 12,640 passed / 58 ignored、vendor egui 25・egui-wgpu 11・eframe 18 passed。合計 **12,694 passed / 0 failed / 58 ignored** |
| `cargo build --release -p musicvideo_lab` | **成功、exit 0**（34.42 秒）。成果物 `target/release/musicvideo_lab.exe` |

変異検証は 6 件とも、コンパイルエラーではなく実行された対応テストの失敗で検出した:
UI の IME plugin 登録削除、共有 Tab cancellation 削除、IME / grace Undo gate 削除、
recovery Undo gate 削除、ラボの setup 登録削除、今回追加のモジュレータ見出し salt 削除。
各変異後に元のソース bytes を復元。salt 削除では名前の連続入力テストが ModulatorName の
focus 喪失を検出する。復元後の通常コードを全体 gate で再検証した。詳細は `target/i1-2b/mutations.json`。
L1〜L3 の既存テストは維持・通過。コミット・アプリ起動・`build-dev.ps1` は実行せず、
`target/dev-runtime` の確認用 exe は更新していない。無関係な `out.mp4` も変更していない。

全体 gate の件数は各 target の最終 summary を集計し、子プロセスの再掲 2 件を重複加算していない。
本体 lib は 11,188 passed / 52 ignored、既存の ignored は維持した。crash-dialog 抑止の error mode は
終了時に元の `0x00008001` へ復元された。段 2a の確認用 exe 3 本はサイズ・更新時刻が作業前と一致。
ラボの実機確認では、レイヤー名・モジュレータ名を 1 文字ずつ変更し、preedit 中と確定後の
focus、Esc 取消、Tab / Shift+Tab、Enter 確定、native Undo を確認する。歌詞の cue・改行・Undo と
Project 履歴の所有も確認する。起動するのは利用者だけとし、コマンドは
`Start-Process -FilePath .\target\release\musicvideo_lab.exe`。

受け入れるコスト: 配布物の PE が 1 本増える（launcher 内包・署名・portable 同梱・全 PE / VC runtime
gate・runtime 清掃）。閲覧中の音楽ビューと同時に再生すると両方鳴る（WASAPI shared で混ざるだけで
互いの状態には影響しない）。

### D11. 出力の既定値

1920x1080 / 60fps / H.264 High profile / yuv420p / bt709 / AAC-LC 48kHz。
解像度・fps は書き出しダイアログで変更できるが、既定は上記に固定する。

### D12. 座標は出力解像度基準の px で入力し、内部は正規化で持つ

ユーザーは 1920x1080 基準の px で考えている（§1.4）。UI は px 入力＋整列ヘルパーを提供し、
モデルは正規化座標で保持する。これで出力解像度を変えてもレイアウトが崩れない。

### D13. 書き出しは中断可能で、UI スレッドを止めない

worker thread + cancel token + 進捗コールバック。[docs/ui-responsiveness.md](ui-responsiveness.md) §4 の
チェックリストを通す。

### D14. エディタの画面構成（2026-10-09）

エディタはウィンドウを丸ごと使う（D10）。現行の右パネルは、表示ゲイン・モジュレータ・レイヤー一覧・
並べ替え・選択レイヤーのプロパティが幅 330px の 1 本のスクロールに縦積みされ、レイヤーが増えると
操作しにくい（2026-10-09 ユーザー報告、`musicvideo-ui` `draw_editor`）。次の配置に改める。

- **左**: レイヤー一覧（常時表示。追加・削除・並べ替えもここ）
- **中央**: プレビューと再生操作
- **右**: 選択中レイヤーのプロパティだけ。モジュレータとプロジェクト全体の設定（表示ゲイン等）は
  別タブにし、レイヤーのプロパティと同じスクロールに積まない
- **下**: タイムライン（現行どおり）

責務: `musicvideo-ui` が領域ごとの描画関数を提供し、パネルの配置はホスト（エディタ exe）が 1 か所で
決める。ホストは 1 つ（D16）なので配置判断が重複しない。P4 のパッケージ化と独立に、ラボで先に行う（L1）。

**履歴を確定する単位は分割しない**（独立レビュー P2 対応）。現行の `draw_editor` は描画だけでなく、
Undo/Redo shortcut の処理、外部編集の checkpoint、履歴の観測（ドラッグ・フォーカス単位の coalescing）を
所有している（`musicvideo-ui` `lib.rs` `draw_editor`、`history.rs`）。領域関数ごとにこれを複製すると、
左領域がドラッグ終了を先に確定し右領域の最後の変更が別 Undo になる、のように**配置順で履歴が変わる**。

- shortcut と復元処理は 1 フレームに 1 回だけ行う。
- 左・右・設定タブは **1 つの論理的な Properties 編集 transaction** として、1 回の checkpoint / 観測に
  まとめる（領域関数はこの transaction の中で呼ばれる）。
- タイムラインは既存どおり別の surface として扱う。
- Project / LRC の非同期完了の採否は、そのフレームの**全領域の入力と `note_edit` を反映した後**に行う
  既存の順序を保つ（ラボ `app.rs` の完了処理位置）。

L1 の回帰テストに次を加える: 領域をまたぐ連続編集、タブ切替、ドラッグの release、歌詞の native Undo、
Undo/Redo 後の選択復元、入力と非同期完了が同じフレームに届く場合。

L1 の実装入口（2026-10-09）: `EditorState::begin_frame(ctx, project)` が復元・reconcile・shortcut を
処理し、`PreparedEditorFrame` を返す。上部の `history_controls` と下部の `timeline` を描いた後、
`properties()` で `EditorFrame` へ移る。左の `layer_list`、右の `inspector` は同じ checkpoint を共有し、
消費型の `finish()` だけが削除確認・Properties の履歴観測を行う。旧 `draw_editor` 入口は廃止。
タイムライン自身が文書を変更し別 surface の履歴を記録するため、Properties の checkpoint は
タイムライン入力の**後**に取る。これを二段階の型で表し、入力前 checkpoint の持ち越しを避ける。
右タブは `InspectorTab` として `EditorState` にだけ保持し、保存・Undo snapshot へ含めない。
一覧選択ではタブを維持し、追加完了（非同期画像追加も含む）ではレイヤータブを開く。
ホストの `draw_editing_panels` は上部履歴操作 → 全幅タイムライン → 左220px → 右360px → `finish`
の順で描き、各入力の `note_edit` を済ませる。左右は可変幅で、親領域の高さと非縮小スクロールを確保する。
`process_editor_frame` が `draw_editing_panels` と既存の素材/LRC選択処理、続く
`poll_projects` / `poll_lrc` / `sync_runtime_resources` の順序を所有する。`update` とheadless入力テストは
ともにこの入口を呼ぶ。`update` はその後に中央プレビューを描く。
各スクロール領域の内容は `push_id` でも分離する。`ScrollArea::id_salt` はスクロール状態だけのIDであり、
内部部品の自動IDは分離しないため、左一覧と右の各タブのnative TextEdit状態を別のIDスコープへ閉じる。

**選択レイヤーの枠**（L2、2026-10-09 ユーザー要望）: レイヤー一覧で選んだレイヤーが画面のどこにあるかを、
プレビューの上に枠で示す。進捗バー・時間表示のように小さいもの、キーフレームで画面外にあるものを探すため。

- 枠はエディタの表示物で、**合成する RGBA には入れない**（D1。書き出しに影響しない）。egui の painter で
  プレビュー画像の上に描く。
- 枠の形は、**表示中のプレビューフレームと同じ時刻・同じ変調値**で評価したレイヤーの矩形（キーフレーム・
  Scale 変調・回転を反映）。評価は `musicvideo-core` の `evaluate` と同じコードを通し、枠用の別計算を作らない。
  再生中に枠だけが絵より先に進まないよう、プレビュー結果にそのフレームの時刻を持たせる。
- 画面外にある・非表示（visible = false / opacity 0）のときも分かるようにする（§7 の L2 行の受け入れ条件）。

**状態で動かない上部とステータス**（L3、2026-10-09 ユーザー報告）: 上部パネルが状態で出入りする行
（読込衝突の確認、未解決素材、画像パスの一覧、書き出し統計、再生中の計測）を縦に積んでおり、状態が変わるたびに
エディタ全体が上下に動く。上部は状態で高さが変わらないツールバー 1 段、最下段に固定 1 行のステータスバー、
判断を求める読込衝突の確認はモーダル（CLAUDE.md「状態の組み合わせを減らす」）、未解決素材と診断（計測）は
必要なときに開くウィンドウ、素材のパスはインスペクタの「プロジェクト」タブへ移す。機能は削らない。

L3 の実装入口（2026-10-09）: `process_editor_frame` が最下段のステータスバーをタイムラインより先に
確保し、上部の常設ボタン・再生/シーク・枠スイッチ・履歴操作を一つの折り返し行にまとめる。
ステータスバーは `exact_height(30.0)`。状態通知は省略と全文ホバー、プロジェクト名と未保存表示は
ステータスバーと viewport title に出す。素材パスは共有 UI の `inspector_with_project_path` から
プロジェクトタブ内へ表示し、未解決画像の差替要求は既存の `draw_missing_images` をそのまま使う。
ホストのボタン要求・ファイルダイアログ・状態投影はラボ `app/chrome.rs` が所有する。

L3 独立レビュー修正（2026-10-09）: ステータス右端に書き出し・素材読込のキャンセル領域を先に確保し、
残りの幅でプロジェクト名と状態通知を省略表示する。狭い幅ではキャンセルを「書出取消」「素材取消」、
さらに狭ければ `E` / `R` へ短縮し、ホバーで全文を出す。未保存の印は名前と分離して幅を確保し、
通常は「（未保存）」、狭い幅では `*` を常に表示する。全文・未解決素材/復元ウィンドウへの操作・
書き出し進捗は「詳細」（極小幅では `...`）メニューでも確認できる。十分な幅では未解決素材/復元ボタンと
進捗を直接表示する。タイトルは前回送った文字列をラボが保持し、初回と文字列が変わったフレームだけ送る。
ステータスの書き出し取消・素材取消・詳細・情報の子UIは、それぞれ用途ごとの固定 `UiBuilder::id` を持つ。
`id_salt` だけでは子の自動IDに親の自動IDカウンタが混ざるため、先行する条件付き要素が消えると
Popupや操作のIDが変わる。固定IDにより、書き出し・素材読込・保存の完了後も開いた詳細メニューを維持する。

読込確認は Ready payload 自体を所有者とし、新たな確認待ち状態を作らない。確認表示中の全編集領域は
disabled Ui にし、`begin_frame_with_input_enabled` で context から直接読む Undo/Redo と削除確認も
抑止する。既存 `begin_frame` は入力有効の互換入口として維持。タイムライン → Properties checkpoint →
左右領域 → `finish` の順序は維持し、確認ボタンを含む全入力の反映後に `poll_projects` / `poll_lrc` を呼ぶ。
両方が Ready のときは Project 確認を先に表示し、LRC payload は保持する。Project の切替だけが
既存の document/session 切替処理を通じて旧 LRC を無効にし、Project の破棄では LRC 確認へ進める。
背景編集の継続を必須とする判定はなく、モーダル化は提示と入力受付だけを変えるため、H1/H2 の
判定・採用・破棄・遅延解放を変更しない。処理全体のモーダル化は採らず、今回指定された衝突確認だけを
モーダルにする。素材完了・復元・計測など update 冒頭の既存 poll 順序も変えない。

### D15. 文字装飾の初版範囲（2026-10-09）

**初版は `comic-core` に既にある装飾だけ**を対象にする（ユーザー判断）。

- 縁取り（`outline`）と外側の追加縁取り（`extra_outlines`、二重・三重の袋文字）
- ドロップシャドウ（`shadow`: ずらし・ぼかし・広がり）
- エコー（`echo`: ずらした複製の重ね）
- 背景の角丸ボックス（`background`）
- 字間・行間（`letter_gap` / `line_gap`）
- グローは既存の**レイヤー共通グロー**を使い、`comic-core` の `TextGlowStyle` を 2 つ目のグローとして
  足さない（同じ効果の経路を 2 つ作らない）

Text / TimeText / Lyrics の 3 レイヤーで同じ文字スタイルの型を共有する（実装を 1 つにする）。
`comic-core` は出荷済みの画像注釈と共有しているので変更しない。

**合成の境界**（独立レビュー P2 対応）: 現行の文字描画は comic-core の bitmap を白で焼き、その
**alpha だけ**を使って RGB を `Fill` から作る（`musicvideo-core` `raster.rs` の文字描画）。このまま装飾を
繋ぐと影・背景・縁取りまで Fill の色になる。次の 2 枚に分けて合成する。

- **装飾 RGBA**: glyph の塗りを透明にした TextBlock を comic-core で焼く。縁取り・影・エコー・背景が
  それぞれ固有の色で出る。
- **glyph マスク**: 装飾なしで焼いた glyph の coverage（現行と同じ）。これに `Fill` を適用する。
- 2 枚は**同じレイアウト**（フォント・文字配置・字間・行間・原点）を共有し、glyph マスク側は塗装効果
  （縁取り・影・エコー・背景）だけを外す。字間・行間は両方に効かせる。
- 合成順は 装飾 RGBA → Fill を適用した glyph で、**まず 2 枚を合成してから**レイヤーの opacity と
  clip を 1 回だけ掛ける。2 枚それぞれに opacity を掛けると重なり部分の透明度が変わる（両方不透明・
  opacity 0.5 なら重なりが 0.75 になる。現行は文字を直接フレームへ opacity 付きで描いている）。
  レイヤー共通グローは**両方を合わせた silhouette** にかける。
- bitmap の確保は現行「論理寸法 + 固定 1px」なので装飾がはみ出す。comic-core が計算している装飾の
  到達範囲を余白に含め、論理寸法（揃え位置の基準）と描画範囲を分ける。rect の clipping・回転・
  グローのマスクを同じ範囲で揃える。
- 単色 Fill + グローには「mask から全体を単色で作り直す」最適化がある。装飾がある場合はこの経路に
  流さない（多色が単色に潰れる）。
- T1 の回帰対象: 透明 Fill + 見える縁取り、グラデーション Fill + 縁取り、回転 + グロー、
  半透明レイヤーで背景・縁取りと glyph が重なる場合、装飾なしはフレームハッシュ不変。

**補間**: 構造を決める離散値（各装飾の有無、追加縁取りの本数、echo の枚数）が前後の
キーで 1 つでも違えば、**スタイル全体を Step** にする。すべて一致すれば色と連続値（幅・offset・ぼかし・
字間等）を補間する。属性ごとに補間/Step を混ぜない（`Fill` と同じ考え方）。

**キャッシュ**（独立レビュー P2 対応。旧記載「文字列が変わらない間は描き直さない」は、装飾を
キーフレームで動かすと成り立たない）: 文字列とスタイルが静的な間は焼いた結果を再利用する。現行の
テキストキャッシュは上限のない HashMap なので、スタイルの連続値までキーに入れると長い書き出しで
bitmap が溜まり続ける。容量上限つき（LRU）にし、補間中のフレームの結果で静的な結果を追い出さない
扱いにする。T1 では合成時間に加えて、装飾をアニメーションさせた長尺書き出しのキャッシュ量と
最大メモリを測る。

初版の対象外: ベベルとエンボス、シャドウ（内側）、光彩（内側）、縁取りのグラデーション塗り、
縦書き、カラオケ風の順次塗り（タイミング調整の問題もあり当面見送り。ユーザー判断）。
**太字・斜体も対象外**: `TextBlock` に `bold` / `italic` の項目はあるが、comic-core の layout / raster は
参照しておらず（値を受け渡すだけ）、「既にある装飾」ではない（2026-10-09 独立レビューで判明）。
太字はフォント選択（Q7）で太字の face を選ぶ形で扱う。

### D16. ホストは 1 つ — ラボを製品のエディタへ昇格させる（2026-10-09）

`tools/musicvideo_lab` を製品のエディタ（bin 名 `mimageviewer-musicvideo`、パッケージは
`crates/musicvideo-editor` へ移す想定）に昇格させる。`vendor/` 不要のラボと製品エディタを並存させない。
ホストが 2 つあると、H1 / H2 のようなホスト側の所有・非同期の不具合を 2 か所で直すことになる。

- 音声デコードは symphonia から本体の `audio_decode`（FFmpeg）に替える（D8。現状はラボが symphonia、
  書き出しが FFmpeg で別のデコーダ）。**これで解けるのはデコーダ・resampler・既定トラック選択の差だけ**で、
  解析の一致（H3）は別に扱う。
- **書き出しの入力は不変のスナップショットにする**（独立レビュー P2 対応）。現 API
  （`MusicVideoExportRequest`）は PCM ではなく `audio_path` を受け、書き出し開始時に再デコードする
  （`mvexport/driver.rs`）。編集中に元ファイルが変わった・消えた場合や、Undo で以前の音声へ戻した場合に
  プレビューと出力が別内容になる。書き出しは **Project + 解決済みの画像・フォントの bundle + PCM**
  （プレビューが使っているもの）を受け取り、ファイルを読み直さない。`mvexport_cli` は CLI 側で
  デコードしてから同じ入口を呼ぶ。
- 編集中の Project からそのまま MP4 を書き出す（worker・進捗・キャンセル、D13、モーダル = D10）。
  現状未接続の「編集 → MP4」導線をここで繋ぐ。連番 PNG 書き出しは開発用として残す。
- 本体 lib（`mimageviewer`）へ依存して `audio_decode` と `mvexport` を使う。依存の循環は無く、
  cpal 0.16 / egui・eframe 0.33（root の `[patch]`）を共有できる。ただし本体 lib 全体と build.rs が
  ビルドされるので、FFmpeg 以外の `vendor/` や FXC 等も要る。exe サイズとビルド時間は
  `mvexport_cli`（debug 26MB）から推測せず、製品エディタで測る（P4e）。
- `-p` 指定のビルドで launcher を巻き込まない運用は維持する。

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
    source: BandEnergy | BandOnset {
        range: Hz{ low, high } | Semitone{ low_midi, high_midi },
        smoothing: None | Average{ factor } |
                   Envelope{ attack_secs, release_secs },
        weighting: Raw | MivDisplay,
        calibration: Manual | Auto{ low_percentile, high_percentile },
        gain_db, floor, ceiling, threshold,
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
            FillBrightness | SpectrumThickness | SpectrumRadius | ImageZoom,
    modulator: ModulatorId,
    amount: f32,                  // 対象の単位で加算。負値も可
    curve: Linear | Squared | Sqrt,
}

LayerKind =
  | Image        { source: ImageRef, fit: Fit, zoom: Animatable<f32> }
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
- 焼いたテキストは**文字列・フォント・サイズと文字スタイル全体をキーに**、容量上限つきでキャッシュする
  （D15）。文字列とスタイルが静的な間は再利用され、時間表示（`mm:ss`）なら 1 秒に 1 回しか焼き直さない。
  装飾をキーフレームで動かしている間は毎フレーム焼き直しになり得る（その場合の時間とメモリは T1 で測る）。

---

## 6. 書き出しパイプライン

```
frame n → t = n/fps → evaluate → rasterize(CPU, rayon) → RGBA→NV12/YUV420p → H.264 ┐
音声:  全尺 stereo f32 ───────────────────────────────────→ AAC ───────────────────┴→ mp4
```

- 当初は合成とエンコードを別スレッドの有界リングで繋ぐ想定だったが、P1b の実測で逐次ループのまま
  目標を上回ったため採らなかった（[P1b brief](brief-musicvideo-p1b.md) §6）。現行は逐次ループ＋
  ラスタライズ・色変換の内部並列（P2c）。
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
| **P3a** ✅ | 編集 UI を `crates/musicvideo-ui`（egui 依存・eframe 非依存）へ移す + プロジェクトの保存/読み込み + 音付き再生プレビュー + 鍵盤背景の透過 | **実装済み**（現ブランチのコミットは 2026-09-04、`6ca5f3091`。旧記載 `5302f1155` は同題の非祖先コミット）。`musicvideo-ui` は eframe / cpal / ffmpeg に依存しない。保存・読込・再生の実機検収と、保存完了の所属管理は引き継ぎ監査を参照 |
| **P3b** ✅ | キーフレーム: 画像切替・フェード・色変化・ON/OFF のタイムライン編集 | **完了**（2026-09-05）。共有波形解析を背景にしたタイムラインで追加・削除・移動・補間変更を行える。全 Animatable プロパティの Const↔Track、同一時刻キー、最後のキー削除時の Const 化、保存・読み込みをテスト済み |
| **P3c** ✅ | 音声モジュレータの配管: 任意の Hz / 半音範囲のエネルギーをプロジェクト単位で定義し、レイヤのスカラーチャンネルへ加算 | **完了**（2026-09-05、`678107131`）。同じ 1 回の解析から bands / notes / 変調値を得る。無変調プロジェクトのフレームハッシュ不変、変調ありの 2 回一致、保存往復、編集 UI の現在値表示を確認 |
| **P3d** ✅ | キック代替の追従特性: 秒指定のアタック／リリース、弱音ゲート、40–120Hz の低域プリセット | **完了**（2026-09-05、`90e4c045c`）。fps に依存しない実効時間定数、シーク時リセット、合成キックで従来 EMA より速い立ち上がりと遅い戻りを数値確認。旧 Average JSON を維持 |
| **P3e** ✅ | 実曲向けモジュレータ調整: 全尺パーセンタイル較正、帯域の正の変化量、編集ペインのスクロール、連続編集時のプレビュー更新 | **完了**（2026-09-05、`bd46f06f6`）。自動範囲と手動上書きを UI に表示。実素材でレベル張り付きの解消とキック間の減衰を数値確認。処理中フレームは編集のたびに破棄せず latest-wins で更新し、完成 RGBA のテキスト欠落が 0 |
| **P3f** ✅ | 実利用の編集改善: 画像内容の中心ズーム、Track のある行だけを表示するタイムライン、全レイヤー種別の追加メニュー、日本語の連番既定名 | **完了**（2026-09-05、`775e252d4`）。`Scale`（矩形）と `ImageZoom`（矩形内の絵）を分離。zoom の無い version 1 JSON は 1.0 として読み、無変調の描画結果を維持 |
| **P3g** ✅ | mIV 横長スペクトラムの共有: 対数軸・3 枚の残光・ピークホールド・オンセット強調を `music-core` に抽出し、横長スペクトラム + 鍵盤を 1 クリックで追加 | **完了**（2026-09-05、`193e1de92`）。本体と書き出しが同じ状態遷移・配置判断を使い、上下レイヤの周波数軸一致と従来の等間隔 Bars 維持を確認 |
| **P3h** ✅ | レイヤー管理: 削除確認、奥／手前への並べ替え、最終画像参照の整理 | **完了**（2026-09-05）。`EditorCommand` 経由で操作し、共有画像は最後の参照を削除するまで `Project.images` に残す。一覧の上が奥、下が手前で実描画順と一致する |
| **P3i** ✅ | 歌詞レイヤー: 行単位の時刻付け、音声再生中のタップ入力、タイムライン修正、LRC 読み書き | **完了**（2026-09-05）。未設定行・空行・同一時刻の後勝ちを含む専用モデルを追加し、描画は comic-core の既存テキスト経路を共有。UTF-8 / Shift_JIS 判定、複数時刻タグ、offset、行内時刻タグを扱い、ファイル I/O はラボのワーカーに閉じる |
| **P3j** 実装済み・実機未検収 | 編集履歴: Project・素材・歌詞行 identity の snapshot、選択復元、連続操作のグループ化 | 共有履歴の初期実装は `4f41cce35`。2026-09-08 に `91326ca8d` / `0c24a453e` で画像・音声の履歴と host 復元を補完、`8e12458e4` で歌詞本文と native Undo の対応を補完。128 段、ドラッグ/フォーカス単位、読込時消去、Redo 破棄、入力中の shortcut 抑止を維持。独立レビューと全体テスト完了。実音・実 IME・操作感は未検収。[検証記録](brief-musicvideo-edit-integrity.md) を参照 |
| **L1** 実装済み・修正検収待ち | 画面構成（D14）: 左にレイヤー一覧、右は選択レイヤーのプロパティだけ、モジュレータ・プロジェクト設定は別タブ | 初版 `e6fda1f9b`。**独立レビューP2/P3修正・自動テスト済み**（2026-10-09、同commitに対する未コミット差分）。core 73件 / UI 96件（既存87＋L1新規7＋修正新規2）/ lab 46件（既存45＋L1新規1）、全215件成功。タブ/左一覧の部品ID分離とnative Undo往復を追加検証。同フレームProject/LRC完了2件は本番の共通入口へ移し、入口内の既存poll呼出を入力前へ移す変異で失敗確認。`cargo tree` の禁止依存なし、fmt・glyph検査成功。releaseラボのビルド結果と変異検証は下記。修正作業ではアプリ未起動・コミットなし |
| **L2** 実装済み・追加P2修正の再検収待ち | 選択レイヤーの枠（D14 末尾）: 選んだレイヤーの矩形をプレビュー上に表示。画面外・非表示も示す | 初版 `838f8503e`（計232件）。ID再利用・stale textureのP2/P3修正 `42a74db10` は独立再レビューで解消確認（計234件）。**通常UI追加のlifetime登録遅延P2を修正・自動検証済み**（2026-10-09、未コミット差分）: `EditorFrame::finish` で対象lifetimeを同期し、同フレームの要求へ全LayerIdを捕捉。実UIから単色/2レイヤー追加→停止中の最初の完成フレームの枠・名前を検査。新規3件は同期を外す変異で全件失敗、復元後core 80 / UI 104 / lab 53、計237件成功。Undo単位・歌詞native Undo・L1 API・RGBAハッシュ維持。全要求経路の確認、releaseビルド等は下記L2追加修正記録 |
| **L3** 実装済み・追加P3修正の再検収待ち | 状態で動かない上部とステータス（D14 末尾）: ツールバー 1 段・固定 1 行のステータスバー・読込衝突はモーダル・未解決素材と診断は別ウィンドウ・素材パスはプロジェクトタブ | 初版 `e958226a9`（254件）。キャンセル領域/未保存の印/タイトル送信の3件を修正した `a5c05ef65`（258件）は独立再レビューで条件付きACCEPT。**追加P3「詳細メニューのID変化」を修正・自動検証済み**（2026-10-09、未コミット差分）: ステータスの子UI4領域を用途ごとの固定IDにする。新規lab 1件、500/1400pt×書出/画像読込/保存完了の7組合せ、計14ケースでメニュー維持とメニュー内の未解決素材操作を実描画/実クリックで検査。修正前と詳細IDだけを戻す変異で失敗確認。復元後 core 80 / UI 105 / lab 74、計259件成功。禁止依存なし、fmt/glyph成功、releaseラボ再build成功（30.53秒）。L1/L2・H1/H2・非同期採否順序は維持。根拠・ログは下記L3追加P3記録。アプリ起動・停止、コミットなし |
| **T1** | 文字装飾（D15）: comic-core 既存の縁取り・追加縁取り・影・エコー・背景・字間/行間を Text / TimeText / Lyrics に共通の型で接続 | 3 レイヤーで同じ型・同じ描画経路。装飾なしのプロジェクトはフレームハッシュ不変。補間規則（構造一致時のみ補間）をテストで固定。保存往復。装飾ありシーンの合成時間を実測して記録 |
| **I1 段 2a / 2b 実装・自動検証済み／実機確認待ち** | IME 補正の共有（D10「設定・入力」）: 本体 `ime_focus` の egui 依存部分（入力補正 plugin・single-line helper）を共有クレートへ挙動不変で切り出し、本体とエディタのホストが policy を登録する | **段 2a（2026-10-09、`367f9c9d3`・独立実装レビュー ACCEPT）**: `egui-ime` と本体 facade、既存呼び出し元を維持。既存 23 件は共有 16 / 本体 7 に配置、raw 禁止・adapter を追加（共有 17 / 本体 12 passed）。full gate 12,681 passed / 58 ignored、fmt / glyph、通常 profile の確認用 build 成功。**段 2b（同日、未コミット）**: 全域 Tab policy を挙動不変で共有化、本体 facade・既存 5 テストを維持。エディタの登録入口・5 欄 helper・Undo gate・raw 禁止を実装、歌詞 buffer / widget ID / native Undo と Hz DragValue を維持。利用者承認でモジュレータ見出しを不変 ID に固定し、同型箇所も調査。追加 13 テスト、UI 117 / ラボ 75 / core 80 / 共有 17 passed、既存 L1〜L3 全件維持、変異 6 件を検出。本体 Tab / IME / 関連・snapshot も通過。full gate **12,694 passed / 58 ignored / 0 failed**、fmt / glyph / 依存検査、ラボ release build 成功。独立実装レビュー **ACCEPT、指摘なし**。build-dev 再実行・コミット・アプリ起動なし。詳細は D10「I1 設計」§7・§8。**実装・自動検証は完了、実機確認・最終受け入れは未完了**。本体の確認用 exe は更新せず、ラボの確認用 exe は `target/release/musicvideo_lab.exe` |
| **H3a** | 解析契約の確定: 正規の較正区間（出力尺に合わせた PCM か全尺か）と解析の時刻列・seek 復帰の規則を決める | [引き継ぎ監査](music-video-handoff-20260907.md) §3 H3。可変時刻列と較正入力差をテストで固定。無変調ハッシュ維持 |
| **H3b** | 一致の検証（P4b 接続後） | プレビューで表示したフレームが、同じ時刻の書き出しフレームと一致する: 表示フレームの欠落、前後への seek、直接 seek、音源より短い / 長い出力尺。これが通るまで H3 を完了扱いにしない |
| **P4a** ✅ | 方式の設計確定: 本書 D10 / D14〜D16 の独立レビュー | GPT-6.1 Sol / xhigh の設計レビューで blocking 指摘なし。**完了**（2026-10-09）: 3 回目で条件付き ACCEPT（残った P3 の文言は反映済み）。runtime 競合の割り切りはユーザー判断で確定 |
| **P4b** | ホストの製品化（D16）: ラボを `crates/musicvideo-editor` へ移し、FFmpeg デコードへ統一、スナップショット入力の MP4 書き出し（モーダル・進捗・キャンセル）、未保存の編集の確認、音声スレッドが stream を所有する再生と f32 以外の出力形式対応 | 書き出しがファイルを読み直さず、プレビューと同じ PCM・素材を使う（元ファイル変更・Undo 後の書き出しをテスト）。未保存確認の開く / 新規 / 最近 / 閉じる × 保存成功 / 失敗 / キャンセル、**保存は成功したが完了時点で再び未保存**、保存中・書き出し中の終了を handler テストで固定。書き出しの進捗・キャンセル・失敗が UI を止めない（[ui-responsiveness.md](ui-responsiveness.md) §4）。編集済みプロジェクトの 5 分 1080p60 完走と実測 |
| **P4c** | 起動引数と診断（D10）: `--audio` / `--project` / `--data-dir`、最近使ったプロジェクト、ログ・panic ログ、引数エラーの画面表示、runtime の shared lease と取得失敗の表示 | 引数解析の単体テスト。最近使った一覧: 壊れた JSON・無いファイル・上限・取消 / 失敗 / 破棄された読込を入れない・一覧の書込失敗で保存成功が覆らない。`--data-dir` 無しで最近一覧とファイルログを永続化しないこと。mIV の設定・DB・ログに触れないこと |
| **P4d** | mIV 側の入口: 音楽ビュー・音声ファイルの右クリックから「音楽ビデオを作成」でエディタを起動 | 起動は非同期で UI を止めない。exe 不在・起動失敗は理由付きで通知。閲覧側の退行なし |
| **P4e** | 配布（下の表）: launcher 内包・runtime 展開・署名・portable 同梱・全 PE / VC runtime gate・build スクリプト・test-full 前提 | build-dist 相当の通しビルド。runtime 清掃: 旧版 core 終了後にエディタだけ稼働 / 起動との競合 / エディタ終了後の清掃。release のエディタ exe サイズとビルド時間を記録 |
| **P4f** | 文書: マニュアル・製品ページ・keymap-spec（固定ショートカットの理由）・privacy（通信なし・保存先） | CLAUDE.md「コード修正時のドキュメント同時更新」 |

L1 自動検証記録（2026-10-09）:

| 回帰テスト（UIの名前は `split_layout_` を省略） | 意図的な不正実装による失敗確認 |
| --- | --- |
| `reorder_then_property_drag_keeps_two_undo_steps_and_selection` | 左領域にも履歴観測を追加すると失敗。選択復元を先頭レイヤーへ固定しても失敗 |
| `pointer_drag_released_over_left_panel_is_one_undo_step` | pointer継続の判定を右パネル内に限定すると失敗 |
| `tab_switch_is_not_history_and_modulator_edit_is_separate` | タブをsnapshotへ含め、切替を文書変更として観測すると失敗 |
| `lyrics_native_undo_does_not_move_project_history` | TextEditフォーカスを無視してshortcutを処理すると失敗 |
| `selection_keeps_tab_but_added_layer_opens_properties` | 追加時のレイヤータブ切替を削除すると失敗 |
| `shortcut_precedes_timeline_and_runs_once` | shortcutをProperties開始時まで遅らせると失敗 |
| `timeline_mutation_is_outside_properties_checkpoint` | Propertiesのcheckpointをtimeline入力前へ移すと失敗 |
| lab `editor_panels_keep_timeline_full_width_and_inspector_tabs_separate` | timelineより先にSidePanelを置くと失敗。実ホストのPanelState・各タブの描画内容で配置を検査 |
| lab 既存 `project_load_completion_after_same_frame_input_waits_for_a_choice` / `lrc_completion_after_same_frame_input_waits_for_a_choice` | 初版は描画helperへのpoll追加で両方の失敗を確認したが、本番updateの順序退行を検出できないと独立レビューP3で判明。下記の共通入口・既存poll移動による検証へ置換 |

10種類の変異をすべて検出し、その都度元のソースへ復元した。checkpoint前倒しの初回変異は
Undo結果だけでは検出できなかったため、timelineが変更した同フレームのProperties入口snapshotも検査して
再実行し、失敗を確認した。最終ソースで上記全件テストを再実行済み。
ビットマップsnapshotは追加せず、headless eguiの実入力・描画shapeと実ホストPanelStateの矩形を検査する。
既存の歌詞native Undo・IME・identityテストも新入口と左右＋下配置へ移行し、全件維持した。

検証環境はWindows、`MSBUILDDISABLENODEREUSE=1`。各commandのexit codeは0。
ログは `target/l1-{core-tests,ui-tests,lab-tests,ui-dependencies,format,glyphs,release-lab}.log`、
集計は `target/l1-verification-summary.txt`。変異ログは `target/l1-mutation-*.log` と
`target/l1-mutation-summary.txt`（いずれも未追跡のビルド領域）。
`cargo build --release -p musicvideo_lab` は43.5秒で成功し、`target/release/musicvideo_lab.exe` を生成した。
初版の検証作業ではアプリを起動していない。その後の独立レビューでP2/P3が報告され、以下の修正を行った。

L1 独立レビュー修正記録（2026-10-09、`e6fda1f9b` に対する未コミット差分）:

- **P2**: `panel_scroll` の内容を領域/タブのIDによる `ui.push_id` で分離。
  `ScrollArea::id_salt` が子UiのIDに渡らないことをvendored eguiのコードで確認した。
  新規 `split_layout_widget_ids_are_distinct_between_tabs_and_layer_list` は左一覧の全レイヤー項目、
  レイヤー名TextEdit、表示ゲインDragValueのTextEdit、モジュレータ名TextEditの実部品IDが異なり、
  タブ往復後も同じIDであることを検査する。左一覧と右タブ間には元から親SidePanelの分離があり、
  実クリックで衝突がないことも確認した。
  新規 `split_layout_tab_round_trip_preserves_each_fields_native_undo` は名前編集→表示ゲイン文字入力→
  タブ往復後のnative Undoで、名前とゲインがそれぞれ元の値へ戻り、他方の文字列を復元しないことを検査する。
  native Undoがカーソル選択も記録するため、そのUndo段も許容しつつ各段の文字列所有を検査する。
  `push_id` を外す変異で新規2件とも失敗を確認。
- **P3**: `process_editor_frame` に「全パネル入力・note_edit→既存素材/LRC選択処理→Project/LRC完了の採否→
  runtime同期」を集約し、`update` と入力テストが同じ入口を使用する。テスト自身の後付けpollは削除。
  共通入口末尾の**既存** `poll_projects` と `poll_lrc` をそれぞれ入力前へ移す2種類の変異で、
  対応する既存同フレーム完了テストの失敗を確認した（helperにpollを追加する変異ではない）。

変異を元へ戻し、core 73 / UI 96 / lab 46の全215件を再実行して成功。禁止依存なし、fmt・glyph検査も成功。
修正後の `cargo build --release -p musicvideo_lab` は30.1秒で成功し、
`target/release/musicvideo_lab.exe` を生成した。環境は `MSBUILDDISABLENODEREUSE=1`、各commandのexit codeは0。
ログは `target/l1-review-{core-tests,ui-tests,lab-tests,ui-dependencies,format,glyphs,release-lab}.log`、
`target/l1-review-verification-summary.txt`、`target/l1-review-mutation-*.log` と
`target/l1-review-mutation-summary.txt`。修正作業ではアプリ未起動・コミットなし。修正の再検収は未実施。

L2 実装・自動検証記録（2026-10-09、`eb1b3445b` に対する未コミット差分）:

- **評価の共通化**: `scene.rs` の `evaluation_time` が出力尺への時刻clamp、
  `evaluate_layer_properties` が既存のAnimatable評価・全binding適用・宣言順のclamp・中心Scaleを所有する。
  `evaluate` はその戻り値を使い、歌詞/contentの合成経路は従来どおり。
  純関数 `evaluate_layer_bounds` も同じ関数を使い、非表示でもrectを評価する。
  回転した4隅は出力pxで回転してから正規化し、出力の縦横比を維持する。
  各可視ケースの `SceneItem.rect / rotation_deg` との完全一致と非表示ケースのsceneからの省略を検証した。
- **RGBA不変**: 評価共通化前に、固定160×90 fixture（背景画像・既定レイヤー・位置キーフレーム/回転Solid、
  無変調）の `render_frame` RGBAのFNV-1a 64bitハッシュを採取。
  t=0 / 3 / 9秒の順に `15498595806430728683` / `10394692129127540235` /
  `14301170444520341964`。共通化後も3点すべて一致した。
  `raster.rs`・書き出し処理・プレビューの解析/合成処理は変更していない。
- **表示フレームの所有**: workerで採取済みの時刻 `position_secs` とProject snapshotを `PreviewResult` に付け、
  revisionが一致した結果をtextureへ適用する同じ場所で、`DisplayedPreview` のtexture・Project・時刻・
  `spectrum_frame.modulators` 由来の値を一括更新する。枠はこの値だけを使う。
  既存コードでは通常編集/再生が処理中の結果を取消さないため、時刻と変調値だけでは足りず、
  **結果のProjectも必要**だった。再生を止める/編集をモーダル化する案は既存操作を変えるので採らず、
  単一の表示フレーム所有者へ集約して組み合わせを減らした。新しいpending/generationは追加していない。
  stale結果は全入力を維持、素材clear/別文書の適用では表示フレーム全体をclearする。
- **表示UI**: `musicvideo-ui::draw_preview` の画像表示rectを基準に座標を1か所で変換。
  黒3px＋白1px、レイヤー名、非表示の破線、全体画面外の方向矢印＋「画面外」をpainterで描く。
  回転boxと画像の交差は分離軸で判定（包囲/横断するboxやAABBだけが重なるboxを含む）。
  部分画面外は中央プレビュー領域でclip。破線はclip後に生成し、遠方の座標でUI作業が増大しない。
  プレビュー近くの「選択レイヤーの枠」は既定ON、EditorStateのUI状態のみ、保存/Undo対象外。
  未選択/未準備/表示OFFは枠なし。クリック/ドラッグによる選択・移動は追加していない。
  L1の `begin_frame / PreparedEditorFrame / EditorFrame / process_editor_frame` を維持した。

追加テスト一覧（名前は `selection_` を省略）と変異検証:

| crate | テスト | 意図的な不正実装による失敗確認 |
| --- | --- | --- |
| core | `constant_rect_and_time_clamping_match_scene` | 枠rectを固定値へ変更。別の変異でScene側だけrotationを+1し一致検査の失敗も確認 |
| core | `keyframe_between_keys_uses_scene_time_clamping` | 枠の評価時刻を0秒へ固定 |
| core | `scale_rotation_opacity_bindings_match_scene_in_order` | Scale変調を無視 |
| core | `rotation_uses_output_pixel_aspect_ratio` | 回転した隅を無回転にする |
| core | `hidden_visibility_still_evaluates_box_at_frame_time` | visible評価を無視 |
| core | `zero_opacity_and_lyrics_gap_are_hidden` | opacity 0も可視扱い |
| core | `unmodulated_render_hash_is_unchanged` | 合成SceneItemのopacityを0.9倍 |
| UI | `preview_maps_the_actual_image_rect` | 画像の表示原点を無視 |
| UI | `partial_crossing_and_surrounding_boxes_are_not_offscreen` | cornerが画面内かだけで交差判定 |
| UI | `fully_offscreen_markers_point_to_all_sides_and_corners` | 方向を反転。別の変異で交差判定をAABBのみへ変更 |
| UI | `painter_has_two_strokes_name_hidden_dashes_and_panel_clip` | 黒線の太さを変更。別の変異で非表示を実線にする |
| UI | `no_frame_no_selection_or_disabled_draws_no_outline` | 未選択時に先頭layerへfallback。別の変異でOFFを無視 |
| UI | `toggle_is_ui_only_across_undo_and_project_reset` | 文書resetでONへ戻す |
| UI | `dashes_clip_far_edges_before_allocating_segments` | 線分clipを削除 |
| lab | `displayed_frame_uses_result_time_modulation_and_project_in_real_draw` | 再生位置相当の9秒を使う、変調を無視、最新Projectを使う3種類 |
| lab | `stale_preview_keeps_all_displayed_inputs_and_clear_removes_them` | revision検査を無視、素材clearを削除、別文書適用のclearを削除の3種類 |
| lab | `worker_result_carries_its_captured_time_and_project` | worker結果の時刻を0秒にする |

25種類ともコンパイル成功後に該当テストの失敗を確認し、その都度元のソースへ復元した。
最終ソースでcore 80 / UI 103 / lab 49の全232件を再実行して成功。
ビットマップsnapshotは追加せず、headless eguiの描画shape・画像表示rect・実ホストの描画入口で
線幅/破線/ラベル/clip/表示時刻を固定した。アプリ（ラボ・本体）は起動していない。

検証環境はWindows、`MSBUILDDISABLENODEREUSE=1`。全件テスト・禁止依存検査・fmt・glyph検査はexit code 0。
`cargo tree -p musicvideo-ui` にeframe / cpal / ffmpegなし、`cargo fmt --check` 成功、
`python scripts/check_ui_glyphs.py` は危険glyph 0。
`cargo build --release -p musicvideo_lab` は既存のラボexeが実行中（PID 159144）のため、
最終exe置換がアクセス拒否（exit 101、41.8秒）。そのプロセスは停止せず、reparse pointがないことを
確認して実体のreleaseキャッシュを別出力先へコピーし、`CARGO_TARGET_DIR=target/l2-release` で
同commandを実行して40.4秒で成功（exit 0）。成果物は `target/l2-release/release/musicvideo_lab.exe`。
既存 `target/release/musicvideo_lab.exe` は置換していない。MSBuildのアクセス拒否ではない。
ログは `target/l2-{core-tests,ui-tests,lab-tests,ui-dependencies,format,glyphs,release-lab}.log`、
`target/l2-release-lab-isolated.log`、`target/l2-verification-summary.txt`、
`target/l2-render-baseline.txt`、`target/l2-mutation-*.log` と `target/l2-mutation-summary.txt`。
本体は変更していないため、本体用build-devではなく指示書どおりラボのreleaseビルドを確認用成果物とする。
コミットなし。独立レビュー/検収・実機での目視確認は未実施。

L2 独立レビュー修正記録（2026-10-09、`838f8503e` に対する未コミット差分）:

- **P2 — ID再利用とsnapshotの所有**: `next_layer_id` は現在の最大ID+1なので、最大IDの削除や追加のUndo後に
  IDを再利用する。通常編集はpreview revisionを進めず処理中の結果も採用するため、ID照合では別レイヤーを
  同一対象と誤認する。既存の `EditorState::layer_target_lifetime` が削除/Undoで消えたIDの再追加時に新しい
  lifetimeを発行することを確認した。要求開始時にProjectと各LayerIdのlifetimeを対応付け、
  `PreviewResult` → `DisplayedPreview` → `PreviewFrame` へ同じmapを引き継ぐ。
  `selected_bounds` は現在の選択のlifetimeとsnapshot側のlifetimeが一致する場合だけ評価する。
  結果適用時に現在のlifetimeを取り直してはならない（処理中の旧Projectに新対象のlifetimeを誤付与する）。
  時刻・変調値・Projectのsnapshot評価、RGBA合成、通常編集の応答性、保存/Undo形式は維持。
  新たな取消/generationやモーダル化は加えず、既存の表示フレーム所有者へidentity metadataを含めた。
- **P2の新規回帰2件**（lab）: `selection_reused_layer_id_after_delete_does_not_draw_old_lifetime` /
  `selection_reused_layer_id_after_undo_and_other_layer_add_does_not_draw_old_lifetime`。
  実workerの要求経路でlifetimeを捕捉し、実際の削除/Undo・別kindのレイヤー追加で同IDを再利用する。
  旧表示フレームと編集前から処理中だった結果の両方で、枠の描画pathと旧レイヤー名が消えることを
  実ホストのheadless描画で検査。変更していない別レイヤーの枠は維持し、新snapshot採用後には新しい
  レイヤーの枠・名前が表示されることも確認した。既存worker結果テストも捕捉したlifetimeの検査を追加。
- **P3 — texture更新の観測**: 既存 `selection_stale_preview_keeps_all_displayed_inputs_and_clear_removes_them`
  を強化。初回=黒、stale=赤、新規採用=緑、次のframe=青のfixtureに分ける。
  初回upload deltaを消費して画像内容を確認した後、stale結果が対象textureのset/free deltaを出さず、
  metadataとtexture IDも維持することを検査する。採用した次の結果は同じtexture IDへ緑のpixelsを
  uploadすることも検査。既存の素材clear/別文書のclear検査は維持。

| 変異（修正前の挙動/意図的な退行） | 確認結果 |
| --- | --- |
| P2: lifetime照合を外し、修正前のLayerIdのみの照合へ戻す | 新規削除/Undoテストがそれぞれ失敗（2件） |
| P2: snapshotのlifetimeを結果適用時の現在Projectから取り直す | 新規削除/Undoテストがそれぞれ失敗（2件） |
| P3: metadataは維持したままstale画像だけを `texture.set` で更新 | 強化したdelta検査で失敗（1件） |
| P3の検出力対照: 同じ画像更新変異でstaleのset delta検査だけを外す | metadata/texture ID等の検査は成功。同じIDの画像だけを変える退行を旧検査が見逃すことを再確認 |

3種類の変異を5件のテスト実行で検出し、検出力の対照1件も確認。その都度元のソースへ復元した。
最終ソースで `cargo test -p musicvideo-core` 80 / `-p musicvideo-ui` 103 / `-p musicvideo_lab` 51、
計234件成功（初版232件＋新規2件、既存2件の検査も強化）。無変調RGBAハッシュ3時刻も不変。
`cargo tree -p musicvideo-ui` のeframe / cpal / ffmpegなし、fmt・glyph検査はexit code 0・危険glyph 0。
既存 `target/release/musicvideo_lab.exe` が実行中だったため、プロセスを停止せず、
`CARGO_TARGET_DIR=target/l2-release` で `cargo build --release -p musicvideo_lab` を実行。
37.1秒、exit code 0で成功。成果物は `target/l2-release/release/musicvideo_lab.exe`。
検証環境はWindows、`MSBUILDDISABLENODEREUSE=1`。
ログは `target/l2-review-{core-tests,ui-tests,lab-tests,ui-dependencies,format,glyphs,release-lab}.log`、
`target/l2-review-verification-summary.txt`、`target/l2-review-mutation-*.log` と
`target/l2-review-mutation-summary.txt`。アプリ起動・停止、コミットなし。
この修正の独立再レビュー/検収は未実施。

L2 追加P2修正記録（2026-10-09、`42a74db10` に対する未コミット差分）:

前回のID再利用P2とstale texture P3は独立再レビューで解消確認済み。
追加P2の原因は、一覧の追加メニューが `project.layers.extend` で直接変更した後、
`EditorFrame::finish` が対象lifetimeを同期せず、次の `begin_frame` まで登録を遅らせること。
停止中は同フレームの要求が未登録IDを落とし、その後のフレームで登録されても再要求がない。
`finish` の削除確認後・履歴観測前に `target_lifetimes.reconcile(self.project)` を追加し、
ホストへ応答を返す前に登録を完了する。歌詞/native Undo全体のreconcileは追加せず、
既存の履歴観測1回・追加操作のUndo1単位・L1 APIを維持する。要求側のfilter_mapやrevisionは変更しない。

プレビュー要求を作る全経路の確認（本番の呼出は `update` 末尾の1か所）:

| Project変更/要求の経路 | 要求採取前のlifetime同期 |
| --- | --- |
| 通常UIの追加・削除・一覧/Inspector編集 | `process_editor_frame` 内の `EditorFrame::finish`。今回、直接追加も含めて保証 |
| Timeline編集 | レイヤー集合を追加/削除しない。後続の `properties` の `begin_external_edit` と `finish` で同期 |
| 外部コマンド/LRC完了 | 集合に関わるコマンドは適用後にreconcile。animationコマンドは集合を変更しない |
| Undo/Redo（ボタン/shortcut） | `restore_snapshot` の末尾でreconcile。その後に編集フレームのfinish |
| プロジェクト読込（自動採用/選択後の採用） | `apply_loaded_project` が `reset_for_project_with_resources` を呼んでからdirtyを通知 |
| 画像の追加/差替・音声完了 | `commit_image` / `commit_audio` が `finish_external_edit_with_resources` を呼んでからdirtyを通知 |
| 素材のruntime復元/較正完了 | Projectのレイヤー集合は変更せず、登録済みProjectの要求を再開 |
| 初期化/seek/再生 | 初回要求の前に編集フレームを通る。seek/再生は時刻だけを変更 |

新規テスト（UI 1件、lab 2件）:

- `split_layout_frame_finish_registers_lifetimes_for_ui_added_layers`: 実メニューから単色と2レイヤーのpresetを追加。
  `finish` 直後、次の `begin_frame` を呼ばずに全IDの登録・既存token不変・Undo1単位を検査。
- `selection_ui_add_solid_captures_lifetimes_and_draws_the_first_stopped_frame`
- `selection_ui_add_two_layers_captures_lifetimes_and_draws_the_first_stopped_frame`:
  上記lab 2件は実メニューのrelease入力を `process_editor_frame` へ渡し、同じeguiフレームで
  本番の `start_preview_if_needed` を呼ぶ。実worker結果の全IDのtoken、0秒の時刻、
  追加編集やseekを挟まず最初の完成snapshotの枠2色・選択名を検査。Undo/Redoの復元とtoken登録も検査する。

修正前の実装で新規3件が失敗。修正後の成功を確認してから、追加した同期だけを外して
再び3件すべての失敗を確認した（labのmapは単色で4/5件、2レイヤー追加で4/6件）。
ソースを復元後、core 80 / UI 104 / lab 53、全237件成功。歌詞native Undo・L1回帰・
ID再利用・stale texture delta・無変調RGBAハッシュ3時刻の既存検査も成功。
`cargo tree -p musicvideo-ui` のeframe / cpal / ffmpegなし、fmt・glyph検査はexit code 0・危険glyph 0。
ログは `target/l2-finish-{core-tests,ui-tests,lab-tests,ui-dependencies,format,glyphs,release-lab}.log` と
`target/l2-finish-verification-summary.txt`。修正前/変異/復元後の狭い検査は
`target/l2-finish-{before,mutation,after}-{ui,lab}.log`。
利用中のラボexeを停止せず、`CARGO_TARGET_DIR=target/l2-release`、
`MSBUILDDISABLENODEREUSE=1` で `cargo build --release -p musicvideo_lab` を実行。
30.7秒、exit code 0で成功。確認用成果物は `target/l2-release/release/musicvideo_lab.exe`。
アプリ起動・停止、コミットなし。追加P2修正の独立再レビュー/検収は未実施。

### L3 の実装・検証記録（2026-10-09、初版 `e958226a9`）

上部に積まれていた内容の移動先:

| 旧上部の内容 | 移動先 |
| --- | --- |
| Project/LRC 読込衝突の警告と採用/破棄ボタン | `egui::Modal`。確認だけをモーダル化し、現在の編集領域を無効化 |
| 素材の復元中表示 | ステータス通知と「素材復元中」ボタンから開く未解決素材ウィンドウ |
| 復元失敗、未解決画像の一覧と差替、未解決フォント、未解決音声、復元再試行 | 「未解決の素材 N 件」から開くウィンドウ内の縦スクロール |
| 直近の `status` | 最下段固定1行。長文は省略、ホバーで全文 |
| プロジェクト名/未保存 | ステータスバーとウィンドウタイトル |
| プロジェクト・音声・画像ごとの全パス | 右「プロジェクト」タブ、表示ゲイン下の同じ縦スクロール |
| 書き出し進捗、書き出しキャンセル、素材読込キャンセル | ステータスバー |
| 前回書き出しの統計3行、キャンセル時の完了数 | 診断ウィンドウ（既定で閉） |
| 再生/一時停止、時刻、シークのスライダーと直接数値入力、枠スイッチ、Undo/Redo、ファイル/素材/書き出し操作 | 常設の上部ツールバー。利用不可の操作は無効化 |
| シーク時の解析注記、プレビュー計測4行、合成時間、計測リセット | 診断ウィンドウ |

モーダル化の根拠は D14 末尾の実装入口に記録した。Ready 中の編集継続は H1/H2 の成立条件ではない。
新たな待機・rollback・generation を加えず、提示と入力抑止に限定した。以下を本番の共通入口で検査した:

- Project/LRC の確認ボタンを実クリックし、切替/続行、歌詞置換/保持が既存の適用・破棄経路へ進む。
- 確認初回フレームを含め、Properties のドラッグ、フォーカス済み歌詞入力、Timeline キーのドラッグ、
  context-level Undo が Project を変更しない。新規入力抑止を外す変異では Properties テストが失敗。
- Project 確認中の LRC 完了、LRC 確認中の Project 完了、両確認中の画像/復元完了を検査。
  保持/採用は従来の session・lifetime・baseline 判定を通る。Project を破棄すると保留 LRC へ進み、
  Project を採用すると旧 LRC は既存の切替経路で無効化される。
- LRC 確認ボタン release と Project 完了が同じフレームに届く場合を採用/保持の双方で検査。
  保持入力を完了採否の後へ移す変異で、変更されていない Project の自動採用が保留になり、テストが失敗。
- L1 の既存同フレーム Project/LRC 完了2件と、L2 の時刻・modulation・lifetime・stale frame・最初の
  完成フレーム検査を維持。枠スイッチを上部へ移したため、L2 の描画ハーネスも上部配置へ移した。

追加テストは `musicvideo-ui` のプロジェクトタブ読み取り専用パス表示1件と、ラボ `app/chrome_tests.rs`
の16件。高さテストは通常/未保存/素材読込中/素材未解決/書出中/書出完了後/計測あり/Project衝突/
LRC衝突/復元失敗/復元中の11状態を900/1093/1400/2000ptで実描画する。再生/一時停止と時刻桁数の
別テストでは実際の日本語フォントも設定する。診断の初期閉・内容・高さ不変、未解決ウィンドウの
ImageId差替要求、タイトルの名前/未保存も検査する。全17件を追加し、既存237件と合わせて254件成功。
PNG snapshot と実アプリ操作は実施していない。矩形・描画文字列・実入力をheadlessで検査した。

変異は計測4行/確認行の上部復帰、確認中の入力有効化、確認入力と完了採否の順序逆転の4種類。
各1件の対象テストがexit code 101で失敗し、ソース復元後の全パッケージ検証はexit code 0。

検証コマンドとログ:

- `cargo test -p musicvideo-core -p musicvideo-ui -p musicvideo_lab --no-fail-fast`:
  core 80 / UI 105 / lab 69、全254件成功。`target/l3-tests.log`。
- `cargo tree -p musicvideo-ui`: eframe / cpal / ffmpeg なし。`target/l3-ui-dependencies.log`。
- `cargo fmt --all --check`: exit code 0。`target/l3-format.log`。
- `python scripts/check_ui_glyphs.py`: exit code 0、危険glyph 0。スクリプト既定の対象は `src/` なので、
  同じ `scan_file` を共有UI/ラボの21 Rustファイルにも適用し0件。`target/l3-glyphs{,-affected}.log`。
- `MSBUILDDISABLENODEREUSE=1` で `cargo build --release -p musicvideo_lab`:
  39.78秒、exit code 0。`target/l3-release-lab.log`。成果物は `target/release/musicvideo_lab.exe`。
- 変異: `target/l3-mutation-{metrics,inline-choice,input-enabled,admission-order}.log` と
  `target/l3-mutation-summary.txt`。検証集約は `target/l3-verification-summary.txt`。

コード照合時の差異/範囲外の発見:

- ブリーフの「非同期完了は全パネルの入力の後」は、正本D14の文書を置換するProject/LRC採否として
  適用した。素材/復元などのpollは既存からupdate冒頭にあり、L3でも順序を維持した。
- 新規既定Projectは背景レイヤーを持つ一方 `images` が空であり、既存 `start_image_request(Replace)` は
  ImageRefのlifetimeが無いと拒否する。これはL3以前の新規背景差替経路の問題で、今回修正していない。
  素材完了の追加テストは登録済みImageRefを使い、既存所有条件を満たすfixtureで検査した。
- モーダル化の前提に矛盾は見つからなかった。独立レビューと実機検収は未実施。
  本体 `src/` / comic-core / music-core / musicvideo-core を変更せず、アプリ起動・停止、コミットなし。
  作業開始時から存在した未追跡 `out.mp4` は操作していない。

### L3 独立レビュー P2/P3 修正記録（2026-10-09、`a5c05ef65`）

初版 `e958226a9` に対する3件の指摘を、ラボのchromeとタイトル送信だけで修正した。
横一列に後から並べる配置がキャンセルをクリップ領域外へ押し出すこと、名前と未保存の印を
一緒にtruncateしていること、フレーム末尾でタイトルを無条件送信していることをコードで確認した。
前提の矛盾や追加の設計判断は見つからなかった。

- キャンセル2種類は右端から先に割り当てる。短縮ラベルと可変間隔により、狭い幅でも両方へ直接届く。
  名前・状態通知は残りの幅で省略し、詳細メニューに全文・未解決素材/復元操作・進捗を残す。
  幅が十分なら未解決素材/復元の直接ボタンと進捗も残す。固定30pxの高さは維持する。
- 未保存の印は独立した領域で表示し、名前の長さで消えない。狭い幅で `*` に短縮する場合もホバーで意味を表示する。
- `last_sent_title` は送信済みviewport文字列だけのキャッシュ。初回は送信し、名前または未保存状態が
  変わったときに再送する。文書、履歴、読込状態に新たな状態を追加しない。
- L1の編集フレーム、L2の枠/寿命、モーダルの入力抑止、確認入力後のProject/LRC採否を変更しない。

新規のheadlessテスト（`app/chrome_tests.rs`、4件）:

| テスト | 検査 |
| --- | --- |
| `l3_review_cancel_controls_are_visible_and_clickable_in_narrow_combined_states` | 実日本語フォント、100/160/250/480/500/520/900pt、書出のみ/素材読込のみ/両方×未解決の有無×復元中の有無。取消対象ごとに計112ケースで文字とボタンの描画矩形が領域/clip内にあること、実クリックで対象の取消フラグだけが立つこと、固定高さを検査 |
| `l3_review_long_project_name_never_hides_the_unsaved_marker` | 長い日本語名、100/160/250/500/900pt×取消操作4状態の20ケースで未保存の印が独立して全体表示されること |
| `l3_review_title_is_sent_initially_and_only_after_its_text_changes` | 初回、保存済みの名前への切替、未保存化、保存済み内容への復帰で1回ずつ送信。時刻/状態通知だけの変化と同じタイトルの連続フレームでは0回 |
| `l3_review_status_details_preserves_hidden_material_actions_and_progress` | 500ptの併存状態で詳細メニューの進捗と未解決素材ボタンを描画。実クリックで未解決ウィンドウが開き、取消要求は出ないこと |

変異検証は初版のステータス描画を一時的に戻してキャンセル/未保存の各テストを実行し、
タイトル送信の条件を外してタイトルテストを実行した。3件ともテスト失敗（exit code 101）を検出。
キャンセルの変異は500ptの書出・素材読込・未解決・復元中の併存状態で失敗した。
ソースを復元してから以下を実行した。実アプリは起動していない。

- `cargo test -p musicvideo-core -p musicvideo-ui -p musicvideo_lab --no-fail-fast`:
  core 80 / UI 105 / lab 73、計258件成功。`target/l3-review-tests.log`。
- `cargo tree -p musicvideo-ui`: eframe / cpal / ffmpegなし。`target/l3-review-ui-dependencies.log`。
- `cargo fmt --all --check`: exit code 0。`target/l3-review-format.log`。
- `python scripts/check_ui_glyphs.py` と同じ `scan_file` による共有UI/ラボ21 Rustファイルの追加検査:
  危険glyph 0。`target/l3-review-glyphs{,-affected}.log`。
- `MSBUILDDISABLENODEREUSE=1` で `cargo build --release -p musicvideo_lab`:
  exit code 0。`target/l3-review-release-lab.log`。成果物は `target/release/musicvideo_lab.exe`。
- 変異ログ: `target/l3-review-mutation-{cancel,dirty,title}.log` と `target/l3-review-mutation-summary.txt`。

変更はラボ3ファイルと本計画書のみ。本体/共有UI/coreには変更なし。アプリ起動・停止、コミットなし。
既存の未追跡 `out.mp4` は操作していない。修正の独立再レビューと実機検収は未実施。

### L3 追加P3「詳細メニューのID変化」修正記録（2026-10-09、未コミット差分）

`a5c05ef65` の独立再レビューは条件付きACCEPT。残ったP3をコードで確認した。
`vendor/egui/src/ui.rs` の `new_child` は `id_salt` に親の自動IDカウンタを混ぜて
子の自動IDを開始する。書出/素材読込の取消領域と未保存表示が出入りすると、詳細の
menu button、ひいてはPopupのIDが変わる。開状態を再設定する回避策は追加せず、
子UIの所有するIDを `.id("musicvideo-status-details")` に固定した。

他のステータス要素も照合した。素材取消の子UIは書出取消の出入りに、情報の子UIは
取消と未保存表示の出入りに同じように依存するため、書出取消・素材取消・情報も用途別の固定IDにした。
未保存の印・名前・状態通知・進捗は読み取り専用表示で、独自のPopup開状態は持たない。
情報内と詳細内の未解決/復元ボタンはどちらも既存の `ShowMissing` 経路へ進む。
上部シークの子UIは先行要素の構成が常設で、この条件付き兄弟の問題には該当しない。
配置・文言・取消処理・L1/L2・非同期採否順序に変更はない。

新規headlessテストは `l3_review_status_details_survives_async_completions_and_keeps_menu_actions` の1件。
実日本語フォント、500/1400ptで詳細を開き、書出成功・画像読込成功・保存成功の非空7組合せ、計14ケースを検査する。
ワーカーの完了メッセージをchannelへ送り、既存の `poll_loads` / `poll_export` と編集入力後の保存採否を通す。
メニュー維持に加え、Foregroundのメニュー内ボタンを実クリックして未解決素材ウィンドウが開くことを確認する。
広い幅のステータス直下にある同名ボタンはクリック対象から除く。

修正前のテストと、詳細だけ旧 `id_salt` に戻す変異は、いずれも500ptの書出完了後に
メニューが閉じてexit code 101で失敗。復元後の検証:

- `cargo test -p musicvideo-core -p musicvideo-ui -p musicvideo_lab --no-fail-fast`:
  core 80 / UI 105 / lab 74、計259件成功。`target/l3-popup-tests.log`。
- `cargo fmt --all --check`: exit code 0。`target/l3-popup-format.log`。
- glyph検査: 既定スクリプトと同じ `scan_file` による共有UI/ラボ21ファイルとも危険glyph 0。
  `target/l3-popup-glyphs{,-affected}.log`。
- `cargo tree -p musicvideo-ui`: eframe / cpal / ffmpegなし。`target/l3-popup-ui-dependencies.log`。
- `MSBUILDDISABLENODEREUSE=1` で `cargo build --release -p musicvideo_lab`:
  30.53秒、exit code 0。`target/l3-popup-release-lab.log`。成果物は `target/release/musicvideo_lab.exe`。
- 修正前/修正後/変異: `target/l3-popup-{before,after,mutation}.log`。

アプリ起動・停止、コミットなし。未追跡 `out.mp4` は操作していない。追加P3修正の独立再レビュー/実機検収は未実施。

### P4e の配布経路（独立レビューの整理による）

| 経路 | 必要な追加 |
| --- | --- |
| launcher | build.rs の前提検査・hash・rerun、内包 ASSETS、展開と内容一致のテスト |
| build-release | エディタの build → 署名 → launcher 内包、PE gate の入力 |
| build-dist | 通常 / portable の clean 対象、内包元エディタの最終 gate |
| build-portable | 同じホストの build・copy・署名、portable の data-dir 保証 |
| build-dev | core と同居するエディタの build、DLL 配置・gate・安全性テスト |
| test-full | 移動後パッケージのテストと、launcher が要求する release exe の準備手順 |

`check-vcrt-pe-dependencies.ps1` の判定処理は変えず、入力対象にエディタを加える。**エディタを build
スクリプトの自動 Stop-Process 対象にしない**（未保存の編集を失う）。出力先の exe を掴んだエディタが
稼働中なら、利用者に保存・終了を求めてビルドを止める。

進める順（2026-10-09、独立レビュー反映）: **P4a**（本改訂の独立レビュー）→ **L1** → **L2**（ユーザー要望で追加）→ **L3**（ユーザー報告で追加）→ **I1** → **T1** →
**H3a** → P4b → **H3b** → P4c → P4d → P4e → P4f。L1 / I1 / T1 はパッケージ化と独立にラボで確認できる。
I1 は L1 で TextEdit の置き場が決まった後に行う。H3 は契約（H3a）を P4b の前に決め、一致の検証（H3b）は
P4b でスナップショット書き出しが繋がった後に行う。

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
| Q7 | フォント選択 UI を `comic-core` の `FontSet` にどう繋ぐか（mIV のテキスト注釈と同じフォント一覧を使うか）。現状はラボが Yu Gothic 等の既定フォントを自動選択するだけ | T1 または P4b |
| Q8 | 周波数軸オプション（線形 / 対数 / 重み無し）をいつ足すか | P3 以降 |
| ~~Q9~~ | ~~同じプロジェクトを 2 つのエディタで開いた場合~~ → **決着**（2026-10-09 ユーザー判断）: 割り切る。最後に保存した方が残り、ロックや外部更新の検知は作らない | — |
| ~~Q10~~ | ~~エディタ自身の設定を保存するか~~ → **決着**（2026-10-09 ユーザー判断）: **最近使ったプロジェクトの一覧だけ保存する**（毎回ファイルを選ぶ手間を避けるため）。書き出し設定の既定などその他は初版では保存しない。保存の扱いは D10「エディタの保存データ」 | — |

P3b で Q1 の方式を実装して確認した。単発のクロスフェードは 2 枚の画像レイヤに対して
各 2 個の opacity キーを打つだけで実用になる。一方、連続する多数の画像切替ではレイヤ追加と
両側の opacity キー設定を繰り返すため煩雑さが残る。専用の slides モデルは追加せず、実利用後に
「同じモデルへ複数操作をまとめて投入するショートカット」で足りるかを Q1 の判断材料とする。

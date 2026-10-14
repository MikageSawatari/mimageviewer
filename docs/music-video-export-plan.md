# 音楽ビデオ書き出し (Music Video Export) — 設計計画

Status: P1〜P3j、P4b-1〜4 の実装・非対話検証あり。H3a の契約と H3b の画素一致は非対話検証済み。P4c・P4d の実装・非対話検証あり。P4d の独立レビュー1回目はREVISE、対応後の再レビュー・実機検収、P4e 以降は未完了。
2026-10-09 に P4 の方式を「別 exe のエディタ」へ改訂（D10 / D16）、
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
H3（H3a + H3b）の解析・画素一致は非対話検証済み（下記 H3b 検証記録）。P4 の実機検収は未完了。P4c・P4d の実装・非対話検証あり。P4d の独立レビュー1回目はREVISE、対応後の再レビューと P4e 以降は未完了。
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

#### D3 の改訂 — H3a 解析の契約（2026-10-14、実装前レビュー 3 回目で ACCEPT。実装 brief: [brief-musicvideo-h3a.md](brief-musicvideo-h3a.md)）

**問題（引き継ぎ監査 §3 H3 を現コードで再確認）。** 次の 5 つの状態が「呼ばれた順番と間隔」に依存する。
ラボのプレビューは実時間の再生位置で、書き出しは `n·den/num` で呼ぶため、同じ時刻でも値が一致しない。

| 状態 | 場所 | 依存の仕方 |
| --- | --- | --- |
| FFT 窓の更新間引き | `music-core` `SpectrumAnalyzer::update_windows` | 窓ごとに前回の中心から 5〜25ms 以上離れたときだけ再計算する。結果は前回どこで計算したかで変わる |
| 帯域の平滑化 | `SpectrumSource::update_analysis` | `Average` は呼出し 1 回ごとの EMA。`Envelope` は呼出し間隔 `delta` に依存する |
| モジュレータ | `update_modulator_values` | onset は直前の入力との差分を間隔で割る。平滑化は上と同じ |
| 鍵盤の減衰 | `prepare_keyboard_frame_into` | 呼出しごとに減衰を進める |
| 残光・立ち上がり | `MivSpectrumState` | 呼出しごとに進める。後退と 250ms 超の飛びでリセット |

較正の入力も違う。ラボは曲の全尺の PCM を走査し、書き出しは出力尺に切った（または無音で延ばした）PCM を
走査する。尺が音源と違うと floor / ceiling が変わる。無音で延ばした分は分布の下側を 0 に引き寄せる。

**契約。**

1. **正規の音声入力**は「ソース PCM を出力尺で見たもの」とする。出力尺より後の音は存在しない。
   ソースより後（出力尺のほうが長い分）は 0。解析器が見る全長（窓の大きさの上限・中心の上限）は出力尺の
   サンプル数。複製を作らずに表せるよう、`samples` と `logical_frames` を持つ読み取り専用の view を
   入力にする。書き出しの AAC も同じ内容を符号化する（既存の `fit_audio_to_duration` と同じ規則）。
2. **解析の時刻列は出力フレームの格子**とする。`t_n = n·den/num`（`n = 0..frames`）。有理数の fps
   （現 driver の `rational_frame_rate`）、フレーム数 `frames = round(duration·fps)`（現 driver の規則。
   1 未満は書き出しと同じく不正な入力として扱う）、時刻 `time(n)`、位置から番号への `index_at(p)` を 1 つの
   格子型として `musicvideo-core` に置き、プレビュー・書き出し・PNG 書き出し・CLI・ベンチマークが同じ型を使う。
   PNG 書き出しの現行の `ceil` 規則はこれで置き換える。
   **フレーム n の解析結果は、新しい解析器でフレーム 0..=n を順に処理した結果**と定義する（参照定義）。
   書き出しはこの定義をそのまま実行する。参照定義に seek は無いので、「後退・250ms 超の飛びでリセット」は
   音楽ビデオの契約から外す（本体の音楽ビューの規則は変えない）。
3. **FFT の生の値をフレーム番号だけの関数にする。** 窓ごとの中心を、その窓の更新間隔の格子
   （`floor(center / refresh_frames) · refresh_frames`）に揃えて計算する。中心は
   `min(round(t_n·rate), L-1)` を求めた後に整数で揃え、FFT は揃えた中心で計算する。窓の大きさと開始位置の
   上限はどちらも論理長 `L = logical_frames` から求め、実際の `samples` の長さは読み取りとゼロ補完だけに使う。
   同じ格子点の間は前回の結果を再利用する。計算回数は現在の間引きより増える（48kHz・60fps で 25ms 窓が
   毎秒約 30→40 回、18ms 窓が約 30→56 回。独立レビューの試算）ので、A6 で実測して採否を確かめる。
   これで生の値は呼出し順に依存しなくなる。`music-core` にはこの方式を選ぶ構成を追加し、本体の音楽ビューの
   解析は変えない。
4. **後段（平滑化・onset・鍵盤・残光・モジュレータ）はフレーム格子で 1 フレームずつ順に進める。**
   1 フレーム分の処理（生の値の取得 → 後段 → 完成した `SpectrumFrame`）は「解析タイムライン」の 1 つの
   step 関数だけが行い、書き出しも同じタイムラインを 0 から順に呼ぶ。時刻・間隔 `delta = time(n) − time(n−1)`
   の算出と浮動小数点の演算順は、この経路 1 つにしかないので、参照実行と再開とで同じになる。
   - **状態**は「フレーム c まで処理済み」か「未処理（フレーム 0 の前）」のどちらか（型で区別する）。
     処理済みの状態は、番号 c、平滑化済みの帯域、モジュレータ値と直前入力、鍵盤状態、残光状態、
     **完成した `SpectrumFrame`** を持つ。同じ時刻のキャッシュ用の旗（`full_frame_valid`・
     `full_frame_gain_bits`・`last_t`）は持たない。番号 c があれば足りる。
   - **チェックポイント**は処理済み状態の複製で、1 秒（`fps` フレーム、端数切り上げ）ごとに取る。上限は設けない
     （1 時間で数十 MB。まれな長尺は割り切り）。
   - フレーム n を求められたら、n 以下で最も新しい処理済み状態（現在の状態かチェックポイント）を選ぶ。
     その番号が n ならその `SpectrumFrame` を返し、n より小さければ次の番号から n まで進める。無ければ
     未処理状態から始める。
   - **key** は次を並べた型で、1 つでも違えば状態とチェックポイントをすべて捨てる。PCM の識別（パスではなく、
     不変のデコード結果に付いた識別。ラボは既存の `ResourceRevision`。P4b では書き出しへ渡す PCM スナップショット
     と同じもの）、sample rate、論理長 L、格子 `{num, den, frames}`、解析方式、実効帯域数、平滑化、並び順を含む
     モジュレータ定義、各モジュレータの実効 floor / ceiling（較正結果を反映した値）、表示ゲイン。
     浮動小数点はビットで比べる（現行の `full_frame_gain_bits` と同じく、`+0.0` と `-0.0` は別の key）。
   - キャンセルは区間の区切りで確認し、そこまでに確定した状態とチェックポイントは残す。
   - 区間の並列計算（生の値は純粋なので可能）は、まず逐次の正しい経路を作り、A6 の計測で必要なときだけ足す。
   - **所有（ラボ）**: タイムラインは編集セッションが `Arc<Mutex<…>>` で 1 つだけ持つ。プレビュー worker は
     解析の間だけ lock し、描画の前に放す。状態が worker の外へ移動しないので、キャンセル・エラー・結果の破棄で
     失われない。プレビュー worker は**同時に 1 つだけ**とする（現行の「実行中は次を起動せず、最新の要求を
     1 件にまとめる」受付を維持する）。UI は取消を通知した後も、結果の受信かチャネルの切断（`try_recv`）で worker の
     終了を確かめてから次を起動する。したがって worker どうしが lock を取り合うことは無い。それでも worker は
     lock を取った直後、key の比較や状態の変更より前に取消を確認する。key の比較と作り直しは worker の中で行う。
     完成した `SpectrumFrame` は lock の中で複製し、描画と結果の送信の前に lock を放す。UI スレッドは Arc の複製・
     要求の作成・取消の通知・結果の受信だけを行い、`lock`・`try_lock`・同期の `join` をしない。
     結果には要求の世代と key と n を付け、画面への採用は既存の revision 照合で行う（採用とタイムラインの保持は別）。
     プロジェクトの切替・音声の差し替え・Undo / Redo は key（PCM の識別と設定）が変わるだけで、タイムラインが
     次の要求で作り直す。専用の後始末は持たない。まれな worker の異常（panic で lock が汚れる等）はログと通知を出し、
     タイムラインを作り直す。常駐 worker を新設する案もあるが、要求の受け渡しと寿命の管理が増えるので採らない。
5. **プレビューは出力フレームを表示する。** 再生位置 `p` を含むフレーム、つまり `time(n) ≤ p` となる最大の n
   （`0..frames-1` に収める）を格子の `index_at(p)` で求める。`floor(p·num/den)` を候補にし、同じ `time()` で
   `time(n) ≤ p < time(n+1)` を確かめて ±1 で補正する（任意の epsilon は使わない。60fps の n=123 では
   `floor(2.05·60) = 122` になるため、補正が要る）。解析もシーンの評価（キーフレーム・時刻表示・歌詞）も
   `t_n = time(n)` で行う。これでプレビューの 1 枚は、書き出しの同じ番号のフレームと同じ入力から作られる。
   表示が間に合わずフレームを飛ばしても、タイムラインが飛ばした分を順に処理するので結果は変わらない。
6. **較正**は固定 60Hz の時刻 `t_k = k/60` のうち、`t_k < min(出力尺, ソース長)` の標本だけを使う。
   出力尺は Project の要求値 `duration_secs`、ソース長は長さを揃える前の `source_frames / sample_rate` で、
   どちらも較正関数へ明示的に渡す（長さを揃えた後の PCM からは復元できない）。標本はこの条件を実際に比べて採る
   （現行の `ceil(duration·60)` は終端と等しい標本を含めてしまう）。入力は 1 と同じ view（出力尺より後の音は
   窓にも入らない）。無音で延ばした区間は標本に含めない。正の短い尺では k=0 の 1 標本、標本が無ければ
   `sample_count = 0`・floor 0・ceiling 1e-4（現行の空入力と同じ）に固定する。較正結果の key に出力尺と PCM の識別を加える。書き出しも同じ関数で計算する
   （入力が同じなので結果も同じ。書き出しへ較正結果を渡すかどうかは P4b のスナップショット設計で決める）。

**この契約で変わる見た目。** FFT 窓の中心の揃え方が変わるので、スペクトラム・鍵盤・モジュレータを
使う書き出しのフレームは、以前の版とビット一致しない（未リリース機能なので移行は不要）。
無変調のフレームハッシュ（`selection_unmodulated_render_hash_is_unchanged` など固定の SpectrumFrame を
使う検査）は変わらない。音源とラボのデコーダの違い（symphonia と FFmpeg）は P4b で解消する（D16）。

**組み合わせを減らすための検討。**

- 採用: seek を契約から無くした。「seek 後の復帰規則」を決める代わりに、どんな要求順でも参照定義と
  同じ結果を返す 1 つの所有者（タイムライン）を置く。プレビュー側で場合分けする状態が無くなる。
- 採用: 生の FFT 値を純粋にしたので、チェックポイントに大きな FFT 窓（数百 KB）を含めずに済み、
  必要なら遠いフレームへの移動を並列で計算できる。一致の保証そのものには並列化は要らない。
- 不採用: 解析結果を全フレーム分あらかじめ計算して持つ案。鍵盤と残光のフレームを含めると
  5 分 60fps で約 100MB になり、設定を変えるたびに全体を作り直すことになる。
- 不採用: 後段を有限の窓（直近 W フレームだけで決まる形）に作り直す案。長いリリース時間の見た目が変わり、
  書き出しも 1 フレームごとに W フレーム分の計算が要る。
- 保留（計測で決める）: 生の値をフレームごとに保持するキャッシュ。後段だけの編集（平滑化・しきい値・
  表示ゲインなど）でも、遠い位置では 0 から生の値を計算し直す。2026-10-14 の設計担当判断で、
  4 分位置で約 0.7 秒以内（並列）を目安とし、生の値のキャッシュは追加しない。
  後段だけの編集が遅いと実利用で分かった時点で、キャッシュの採否を別途判断する。

**操作感への影響。** 再生位置が曲の後半にあるとき、解析の設定（モジュレータ・表示ゲイン・平滑化）を
変えると、プレビューの更新に 0 からの再計算の時間がかかる。自動較正のモジュレータは今でも
編集のたびに全尺の較正を待っているので、新しく遅くなるのは手動範囲の項目と表示ゲイン。
ラボは未リリースで、出荷済みの挙動の退行ではない。

**受け入れ基準（テスト）。**

| # | 内容 |
| --- | --- |
| A1 | 任意の要求列（順方向・フレームの欠落・後退・遠いフレームへの直接移動・チェックポイントちょうどの番号・同じフレームの反復・途中の設定変更・キャンセル後の再開）で得たフレームの解析結果が、参照定義（0..=n を順に処理）とビット一致する。乱数で作った要求列を含める |
| A1b | ラボのホスト: 取消済みの worker が終わるまで次の worker を起動しないこと、取消が lock 取得直後に確認されること、前の要求のキャンセル・描画エラー・revision 不一致の結果破棄・プロジェクト切替・音声差し替え・Undo の順序を制御した handler テストで、タイムラインの状態が失われず、古い key の状態が新しい要求に使われない |
| A2 | 生の FFT 値が、計算の順番（逆順・飛び飛び）に関係なくフレーム番号だけで決まる。view（samples が L より短い・長い）と、L に揃えて実体化した PCM とで、先頭・終端付近を含めて一致する |
| A2b | 格子: `index_at(time(n)) = n`、境界の直前は n−1、直後は n。round 規則のフレーム数、1 未満の拒否 |
| A3 | 較正: 出力尺が音源より短い・長い・同じの 3 通りで、プレビュー側の view と書き出し側の切り詰め／延長済み PCM の較正結果が一致する。延長した無音が標本に入らず、出力尺より後の音が窓に入らないこと |
| A4 | 書き出し driver とラボの PNG 書き出し・プレビュー・ベンチマーク（`benchmark_once`）が同じ格子型・同じ view・同じタイムラインを使う（格子の重複実装が無い） |
| A5 | 無変調のフレームハッシュが変わらない |
| A6 | 計測: 1 フレームの解析時間（変更前後）、5 分の素材で 4 分の位置への冷えた状態からの移動と後段だけの編集の再計算時間。逐次から必要に応じ区間並列化し、4 分位置で約 0.7 秒以内（並列）を目安とする。生の値のキャッシュは追加しない（2026-10-14 設計担当判断）。書き出し速度が変更前と同程度 |

H3b（P4b 接続後）は、製品エディタの実際のプレビューで表示した画素と書き出しフレームの一致を確かめる。

実装前レビュー 1 回目（2026-10-14、GPT-6.1 Sol / xhigh、REVISE: P2 5 件・P3 1 件）を反映した。
P2: チェックポイントの内容と再開位置、key の型と PCM の識別、キャンセル時の状態保持（`Arc<Mutex>` の所有へ変更）、
位置→番号の境界補正、較正に要る元音源長と出力尺の明示。P3: FFT の計算回数が増える点の訂正。
再レビュー（同日、REVISE: P2 1 件）: worker の受付順と lock 取得後の取消確認を追記した（同時に 1 つだけ・UI は lock しない）。
3 回目（同日）: 指摘なし、ACCEPT。
レビューで確認された前提: evaluate・文字キャッシュ・鍵盤レイアウトに追加の履歴依存は無い。中心の量子化は
論理長から窓の大きさと開始位置を求めれば呼出し順に依存しない。無変調ハッシュの検査は解析器を通らない。


#### H3a 実装記録（2026-10-14）

`24937705b` を基点に `musicvideo` worktree へ未コミットの差分を作成した。
**A6 の設計担当判断（2026-10-14）:** 区間並列で観測した 4 分位置 0.54〜0.70 秒を採用する。
0.5 秒は設計時の仮の目安であり、計測環境も静穏でなかったため、目安を「4 分位置で約 0.7 秒以内（並列）」へ改めた。
生の値のキャッシュは追加しない。後段だけの編集が遅いと実利用で分かった時点で別途判断する。
この判断に基づき残る受け入れ作業を再開した。実アプリ／lab GUI 起動なし・コミットなしを維持する。

- `crates/music-core/src/{analysis,lib,spectrum_visual}.rs`: 論理長付き `PcmView`、選択式の
  `SpectrumRefreshMode::FrameGrid`、時刻キャッシュを通さない `advance_frame`。既定の Elapsed 方式は保持。
- `crates/musicvideo-core/src/{frame_grid,analysis_timeline}.rs`（新規）と `{spectrum,lib}.rs`:
  共通格子、ビット比較の型付き key、未処理／処理済み状態、1 秒ごとのチェックポイント、取消時の状態保持、
  出力尺と元音源長を明示する較正入口。FFT は区間ごとに並列計算でき、後段は同じ step で順番に適用する。
  並列結果は bounded batch の一時領域だけに置き、永続的な生の値のキャッシュは持たない。
- `src/video/mvexport/driver.rs`: 格子・view・タイムライン・新較正入口へ接続。
  AAC の `fit_audio_to_duration` は変更せず、その前に元音源長を保持する。
  `mvexport_cli` は従来どおりこの driver を利用する（CLI 自体のソース変更なし）。
- `tools/musicvideo_lab/src/{app,export}.rs`: 編集セッションの共有タイムライン所有、worker 内だけでの
  lock と取得直後の取消確認、描画前のフレーム複製と unlock、格子時刻でのプレビュー、
  PCM revision・要求尺ビットを含む較正 key。PNG、benchmark、従来の SpectrumSource 利用診断を共通入口へ接続。
- ヘッドレス計測用に `crates/musicvideo-core/examples/{h3a_legacy_measure,h3a_measure}.rs` を追加。
- `crates/musicvideo-core/tests/analysis_routes.rs`（新規）で driver・CLI・lab の共通入口と
  warmup／計測双方のタイムライン利用、lock 後の取消確認順を検査する。
  lab には格子フレームと PNG の画素一致を実 worker 経由で確認するテストを追加した。

計測条件: release、合成 PCM 5 分・stereo 48kHz、60fps、118 帯域、Average 0.7、要求フレーム 14,400（4 分）。
出力は `target/h3a-evidence/` に保存した（コミット対象外）。

| 経路 | 4 分までの計算 | 表示ゲイン 0→3dB の再計算 |
| --- | ---: | ---: |
| 変更前の参照順実行（0..=14,400） | 2.074924 秒 | 未計測 |
| H3a 逐次 | 2.605951 秒 | 2.580358 秒 |
| H3a 区間並列・初回 | 0.575282 秒 | 0.537510 秒 |
| H3a 区間並列・再実行 1 | 0.698727 秒 | 0.575340 秒 |
| H3a 区間並列・再実行 2 | 0.686084 秒 | 0.630077 秒 |
| H3a 区間並列・再開後の最終計測 | 0.534035 秒 | 0.489301 秒 |

手動範囲の計測は自動較正を無効にした onset モジュレータ 1 件を使い、同じ PCM／格子／118 帯域で
manual floor 0.03 の冷状態が 0.444852 秒、floor 0.03→0.08 編集後の再計算が **0.459336 秒**。
`h3a_measure` 内で明示的に Manual を設定し、較正走査の時間を含めていない。

旧方式の直接 seek は 0.001185 秒だが、過去フレームを処理しない別の意味論なので採否比較には使えない。
初回並列計測には別の cargo 処理が重なった。担当内の cargo を止めて再実行しても閾値超過を観測したが、
OS のプロセス照会はアクセス拒否で、環境全体の静穏性は保証できていない。
設計担当はこの計測上の制約を踏まえて上表の区間並列値を採用した。キャッシュは追加しない。

変更前後の lab ヘッドレス benchmark はどちらも 600 フレーム、1920×1080、60fps、背景 3000×1687、
keyboard/on、warmup 10 フレーム。同じ `--benchmark 600` の 4 条件を順に計測した。

| 条件（glow／modulation／mIV horizontal） | 変更前 解析平均／p95 (ms) | 変更後 解析平均／p95 (ms) | 変更前→後 合計平均 (ms) |
| --- | ---: | ---: | ---: |
| off／off／off | 0.220／0.393 | 0.262／0.389 | 2.513→2.573 |
| on／off／off | 0.175／0.311 | 0.223／0.309 | 3.339→3.326 |
| on／on／off | 0.202／0.390 | 0.237／0.348 | 3.683→3.450 |
| on／on／on | 0.196／0.365 | 0.282／0.393 | 3.977→4.524 |

FFT 窓の格子更新で解析回数が増えるため解析平均は増えたが、1 フレーム 0.223〜0.282ms。
計測中は担当内の build／test を止めた一方、別の `rustc` 1 件を観測しており、環境全体の静穏性は保証しない。
後述の書き出しは同程度の速度を維持した。背景負荷の異なる測定から厳密な高速化率は主張しない。

CLI 書き出し条件は変更前後とも 10 秒、同じ合成 chirp WAV・`htdocs/mimageviewer/ogp.jpg`、
1920×1080／60fps／NVENC、RGBA hash 有効、600 フレーム。
`target/h3a-evidence/` の DLL 同居ディレクトリへ比較用 exe を配置して計測した。
最初の変更後 CLI 実行は release ディレクトリの DLL 不足で起動できなかったため、変更前と同じ配置に揃えて再実行した。

| 経路 | wall (秒) | 実時間比 | render 平均 (ms) | RGBA SHA-256 |
| --- | ---: | ---: | ---: | --- |
| 変更前 | 4.306 | 2.323x | 2.850 | `c4e9d25f7d723967fcc4c68ff3667d8d781be43f960465e2bbdd75ad5de13071` |
| 変更後 | 3.651 | 2.739x | 2.465 | `8a634eae61114e49a3f5bb3eb2caf5a145564239725a80ba8f02fda6c8f0e283` |
| 変更後の同一条件再実行 | 3.635 | 2.751x | 2.430 | `8a634eae61114e49a3f5bb3eb2caf5a145564239725a80ba8f02fda6c8f0e283` |

解析を使う書き出しのハッシュ変更を実測し、FFT 中心の格子合わせによる契約上の予定どおりの変更として扱う。
変更後の 2 回は全 RGBA フレーム列の SHA-256 が一致した。どちらも MP4 再検証で 600 フレーム、
映像 10.000000 秒、音声 10.005333 秒、差 0.005333 秒を確認した。無変調の固定 SpectrumFrame の golden は更新しない。
入力 WAV の SHA-256 は `1d774e968f589b8d47ef9248c458ebc0f2e1a15d49173f8981b939734fee95d2`、
背景 JPEG は `3518ea212eb3372458eaef08a5815b207a23044b262d99e5acb89cd136a3d198`。
生ログは `lab-{before,after}.txt`、`export-{before,after,after-repeat}.txt`、`timeline-final.txt`。

追加テスト: music-core 5 件、musicvideo-core 12 件（格子 2・タイムライン 3・較正 5・経路 2）、lab 7 件。
先行確認では music-core 69 件（unit49・integration20）、musicvideo-core unit 114 件、UI 122 件、lab 81 件が成功。
再開後の lab は全 82 件、経路検査は 2 件成功（本体 lib 全体を含む最終結果は後述）。
`selection_unmodulated_render_hash_is_unchanged` も成功し、既存無変調ハッシュは維持。
FFT の格子合わせ削除／旧更新方式化／物理長による開始位置／残光キャッシュ化と、較正の ceil 化／元音源長無視／
論理長無視／旧解析方式化／検証省略の計 9 種の意図的変異で対象テストが失敗し、復元後に成功した。
残る A1/A1b/A2b/A4 は次の意図的変異でコンパイル成功後に対象テストが失敗（assert／panic、exit 101）すること、
復元後に成功（exit 0）することを確認した。先行 9 ケースと合わせて 32 ケース。

| 対象 | 変異と検出結果 |
| --- | --- |
| A1（4 ケース） | checkpoint の処理済み番号を +1、key が変わっても状態を再利用、gain の `+0.0/-0.0` を数値比較、取消時に進捗破棄。乱数要求列・key 変更・取消再開の対象テストで失敗し、復元後成功 |
| A1b／lab（7 ケース） | worker 実行中の受付抑止削除、lock 待機後の取消と下流の取消確認を無効化、描画エラー時に共有 owner を置換、較正 key の尺欠落（key 検査と古い完了の採用拒否の 2 ケース）、PCM revision 固定、浮動小数点の符号付きゼロ消去。handler テストで失敗し、復元後成功 |
| A2b（3 ケース） | `index_at` の境界補正削除、round を ceil 化、0 フレームの許可。格子の境界／入力検証テストで失敗し、復元後成功 |
| A4（9 ケース） | driver の独自時刻計算・物理長 view・timeline 迂回、CLI 内の格子計算重複、PNG ceil 化、preview の生位置使用、scene だけ生位置で評価、benchmark warmup だけ timeline 迂回、host の lock 直後の取消確認だけ削除。経路検査または画素／handler テストで失敗し、復元後成功 |

lock 後の取消は host と timeline の両方で守られているため、behavioral test は両方の保護を外した場合の
key／進捗汚染を検出し、経路検査は host 側の 1 箇所だけの削除も検出する。
初回の warmup-only 迂回変異は経路検査を通過したため、warmup と計測の両方を検査するよう補強し、
同じ変異で失敗することを再確認した。
再現コマンドとケースは `target/h3a-evidence/{mutate-core.py,lab_mutations.py,lab-mutation-results.json}`、
失敗／復元後ログは同ディレクトリの `a1-*`／`a2b-*`／`a4-*`／`lab-*` に保存（コミット対象外）。
`cargo fmt` 実施、`cargo fmt --check`・glyph check（危険文字 0）・`git diff --check` 成功。
本体 mvexport の限定テスト 8 件成功に加え、再開後は FFmpeg DLL を `target/debug/` と
`target/debug/deps/` へ配置し、`cargo test -p mimageviewer --lib` 全体が **12,719 件成功・59 件 ignored・失敗 0**
（1,396.17 秒、exit 0）で完了した。ignored は既存設定のままで、新たな無効化・除外・filter は加えていない。
一部の snapshot／seek テストに出た 60 秒超表示は、共有 App fixture の `data_dir::test_override_lock()` 待機も含む。
該当テストと実 decoder の EOF ループテストはいずれも最終的に成功し、runner の打ち切り・再実行は行っていない。

| コマンド | 成功件数 | ログ（`target/h3a-evidence/`） |
| --- | ---: | --- |
| `cargo test -p mimageviewer --lib` | 12,719（既存 ignored 59） | `mimageviewer-lib-final.txt` |
| `cargo test -p music-core` | 69（unit49＋integration20） | `music-core-final.txt` |
| `cargo test -p musicvideo-core -p musicvideo-ui` | core 116（unit114＋経路2）／UI 122 | `core-ui-final.txt` |
| `cargo test -p musicvideo_lab --no-fail-fast` | 82 | `lab-full-restored-green.log` |

指定検証の合計は **13,108 件成功・59 件 ignored・失敗 0**。限定テストや変異の復元後再実行は重複加算していない。
`cargo fmt`／`cargo fmt --check`、glyph check（危険文字 0）、`git diff --check` も成功。
すべての build／test に `MSBUILDDISABLENODEREUSE=1` を設定した。

前回の実装担当が作業中に別 context へ依頼した確認は**自己確認**として扱う。
正式な独立実装レビューの通過記録は取り下げる。正式な独立レビューと検収は設計 lead が別セッションで依頼する。
`b473603cc` に対する正式な独立実装レビュー 1 回目（利用者提示、GPT-6.1 Sol / xhigh）は **REVISE**。
指摘 3 件と対応は下の「独立レビュー 1 回目 REVISE と対応」に記録する。
設計との相違は上記の A6 の目安改訂のみ（設計担当の判断。利用者の承認は得ていない。2026-10-14 に利用者へ報告済み）。P4b／H3b の実 decoder 統一と製品画素検証は範囲外のまま。

確認用ビルドは `MSBUILDDISABLENODEREUSE=1` を設定し、`scripts/build-dev.ps1 -PreserveRuntime` で成功。
通常 feature の core と remote／EPUB PDF companion を生成し、VCRT 検査も成功した（ログ `build-dev-final.log`）。
portable feature は有効にしていない。agent は本体・lab GUI・生成した確認用 core を起動しておらず、コミットも行っていない。
利用者が後日本体の既定音楽ビュー（スペクトラム／鍵盤）が従来どおり動くことを確認する場合のコマンドは
`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`。
この core は既定で実際の `%APPDATA%\mimageviewer` を使用し、設定／データを更新し得る。
single-instance mutex を共有するため、起動前にインストール版／tray 常駐版 mImageViewer を終了する。

##### 独立レビュー 1 回目 REVISE と対応（`b473603cc`、2026-10-14）

正式な独立実装レビューは利用者が別セッションで実施した GPT-6.1 Sol / xhigh の **REVISE**。
P2 2 件・P3 1 件を受領した。前回の別 context での自己確認は正式レビューの通過として扱わず、
H3a 行と本文の通過記録を訂正した。以下は `b473603cc` に対する未コミットの修正で、正式な再レビューは未実施。

| 指摘 | 確認した原因／検証不足 | 対応 |
| --- | --- | --- |
| P2: timeline の最終解放先 | `Drop` が cleanup worker へ Arc の複製を渡し、元の field を残していた。worker が先に複製を解放すると、field の自動 Drop が UI 上の最終解放になる | `mem::replace` で空の timeline に置き換え、元の Arc 自体を既存 cleanup worker に移譲。実行時の待機・新しい worker・回復機構は追加しない |
| P2: A1b 音声差し替え／Undo の handler 検証不足 | 旧テストは `audio`／`runtime_resources` を直接代入して音声変更と復元を模擬していたため、Undo の実際の clear／restore 経路の退行を検出できなかった | 実際の `commit_audio`、Undo／Redo → `handle_editor_response` → `sync_runtime_resources` → `poll_resource_restore` を通す独立した host テストへ置換。実 blob の PCM 復元と完了順を制御し、同じ manifest の古い generation の破棄、owner 保持、旧 PCM key／preview の排除、復元後の全 SpectrumFrame ビット一致を検証 |
| P3: 並列区間の取消・再開の未検証 | 既存取消テストが `frame_impl(..., false)` の逐次経路だけだった | 専用 2-thread Rayon pool と `frame_impl(..., true)` で 2 区間の raw batch を強制。最初の区間確定後の結合途中（c=59）と次 batch 境界（c=119）で取消し、確定状態と checkpoint を保持して再開。frame239 と保存済み frame59 の全ビットを逐次参照と比較 |

変更ファイルは `tools/musicvideo_lab/src/app.rs`（解放先修正・host regression）、
`crates/musicvideo-core/src/analysis_timeline.rs`（並列取消 regression のみ）、本設計書の 3 ファイル。
UI 上で cleanup を待つ処理は追加していない。解放先の競合は `cfg(test)` の end-of-Drop observer が
field の自動 Drop 前に cleanup FIFO の完了を待ち、元 allocation の `Weak::strong_count() == 0` と
cleanup thread 名を検査することで、cleanup が UI より先に終わる順序を決定的に作る。
音声デバイスを開かず実 handler を通す factory 注入も `cfg(test)` のみで、通常 build は従来どおり `AudioPlayer::new` を呼ぶ。
既存の gain 編集／Undo、描画エラー、revision 不一致、seek、project 切替の回帰検査は維持した。

失敗確認は次のとおり。すべてコンパイル成功後の対象テスト失敗（exit 101）であり、
修正／変異除去後は同じテストが成功（exit 0）した。ログと再現コマンドは `target/h3a-revise1/` に保存する。

| ケース | 修正前または変異での失敗 | 復元後の確認 |
| --- | --- | --- |
| Drop の旧 clone 実装 | `h3a_drop_moves_last_timeline_owner_to_cleanup_before_fields_drop`: 元 allocation の残る strong count が 1（期待 0）。`lab-drop-old-clone-red.log` | mem::replace 修正後、H3a 対象 9 件で成功 |
| Undo clear で owner を置換する変異 | `h3a_audio_commit_undo_restores_through_host_handlers_and_rejects_stale_completion`: `Undo resource clearing must preserve the session owner`。`lab-audio-undo-reset-mutant-red.log` | 同テストの初回 green と変異除去後 green、lab 全 84 件成功 |
| 並列 merge 取消で確定状態と checkpoint を捨てる変異 | `parallel_cancellation_retains_committed_segments_and_resumes_exactly` が processed frame の保持検査で失敗。`p3-merge-cancel-discards-state-red.log` | 変異除去後、同じ 1 件成功 |
| 並列 merge 取消で checkpoint だけ捨てる変異 | 同テストが保存済み checkpoint 番号列の検査で失敗。`p3-merge-cancel-discards-checkpoints-red.log` | 変異除去後、同じ 1 件成功 |
| 並列経路を無効にする変異 | 同テストが確定位置の検査で失敗し、逐次経路だけでは新テストを通せないことを確認。`p3-disabled-parallel-path-red.log` | 変異除去後、同じ 1 件成功 |

host 側コマンドと結果は `lab-commands.txt`、P3 の変異 script／各終了値は
`p3_mutations.py`／`p3_mutations.json`、新しい P3 テストの初回成功は `p3-baseline-green.log`。
3 件の指摘のうち実行時の不具合修正は解放先の P2 のみで、残る 2 件は要求された検証不足の補強。
music-core の解析方式・格子・較正・step の演算・書き出し経路は変更しておらず、先行 A5／A6 の証跡を維持する。

修正後の最終検証（`MSBUILDDISABLENODEREUSE=1`、test profile、`b473603cc`＋本差分）は
`cargo test -p musicvideo-core -p musicvideo-ui -p musicvideo_lab` が **323 件成功・失敗 0**
（core 117 = unit115＋経路2、UI 122、lab 84、exit 0）。新規は core 1 件・lab 2 件。
ログは `target-tests-final.log`。`cargo fmt`／`cargo fmt --check`、glyph check（危険文字 0）、
`git diff --check` も成功した。変更していない music-core と本体 mvexport は、利用者提示の
`b473603cc` 上の 69 件／8 件成功を有効な証跡として再利用し、新たな実行件数には加算していない。

通常 build の `cfg(not(test))` 分岐も `cargo build --release -p musicvideo_lab` で確認した
（34.02 秒、exit 0、`lab-release-build.log`）。成果物は `target/release/musicvideo_lab.exe`。
本段の本番変更は lab の終了時所有移譲だけで、本体 core の再ビルドは行わない。
アプリ／lab GUI／成果物は未起動、コミットなし。既存の未追跡 `out.mp4` は保持した。
3 件の対応と対象自動検証は完了。**正式な独立再レビュー／検収は待ち**であり、自己確認を通過判定として記録しない。

独立レビュー 2 回目（`b913d9408`、同日、GPT-6.1 Sol / xhigh）: 3 件とも解消、新しい指摘なし、**ACCEPT**。

#### H3b 検証記録（2026-10-14）

対象は `musicvideo` の `ff0cb6d4bf0e624b2e66fcb268272dd0894d00d0` 上の未コミット差分。
正本は D1、改訂 D3（H3a）、§7 H3b と [実装 brief](brief-musicvideo-h3b.md)。
**製品のプレビュー／書き出しに画素不一致は見つからなかった。契約・描画・解析の製品動作は変更していない。**
検証用の観測口とテストを追加した。実装担当が作業内で別 context の確認を行い、指摘なしとした
（設計担当が依頼する正式な独立レビューではない。2026-10-14 設計担当注記。正式な独立レビューは別に行う）。

- mvexport の要求へ `compute_per_frame_hashes` を追加した。既存 `compute_frame_hash` と独立した opt-in で、
  `ExportSummary::rgba_frame_sha256: Option<Vec<[u8; 32]>>` にゼロ始まりのフレーム順で SHA-256 を返す。
  ハッシュは実 driver の `render_frame` 後、色変換／H.264 符号化前の straight RGBA から計算する。
  両旗が false の通常出力では配列を確保せず、追加のハッシュを計算しない。計算時間は `frame_hash_secs` に含める。
  GUI の既定設定／通常 CLI／dev CLI の動作は維持する。
- editor の `PreviewResult` へ `#[cfg(test)] rgba` を追加し、実 worker が描いた `frame.rgba` を
  `ColorImage` 作成後に移動して観測する。製品ビルドにはフィールドも追加確保もない。
  headless harness は `start_calibration_if_needed` → 較正 worker → `poll_calibration` と
  `start_preview_if_needed` → preview worker → `poll_preview` の実経路を通す。
  channel の結果を観測後に同じ結果を戻し、通常の revision 判定・採用・texture upload まで確認する。
- 同じ Project・Assets・PCM `Arc`・`ResourceRevision` を GUI 共通の `snapshot`／`export_snapshot` に渡し、
  OpenH264 で使い捨て MP4 を実際に書く。独立した描画実装・解析参照の再実装は作らない。
  圧縮後 MP4 のデコード画素との一致は対象外（H.264 は非可逆）。

fixture は 160×90／60fps、48kHz stereo の合成音源 2.4 秒。画像・日本語文字・時刻・進捗バー・
スペクトラム・鍵盤、Onset + Envelope、Energy + Average を含める。音源後半を強くし、
短い出力尺での較正・窓への後続音混入も検出する。出力尺は 1.4／2.4／3.2 秒。
各レイヤー・各モジュレータの binding・各 smoothing を一つずつ外した別 Project との比較で、
それぞれが実際に画素に作用することを確認した。設定変更後・復元後は、固定 seek 位置に加え、
モジュレータ／表示ゲインの変更で実際に画素が変わった番号も要求する。

追加テスト（`crates/musicvideo-editor/src/app/parity_tests.rs`、10 件）:

| テスト（共通 prefix `h3b_`） | 検査 | 失敗確認の変異 |
| --- | --- | --- |
| `forward_every_frame_matches_export` | 0..143 の毎フレーム前進 | 旧時刻解析 |
| `dropped_frames_and_boundary_positions_match_export` | 2/5/3/4 枚おきの前進、境界の直前／一致／直後、60fps の n=123 | 旧時刻解析、シーンを再生位置で評価 |
| `backward_far_direct_checkpoint_and_repeat_match_export` | 冷えた状態から末尾へ直接 seek、前後 seek、checkpoint 59/119、同じ番号の反復 | 旧時刻解析 |
| `settings_change_and_restore_match_each_snapshot_export` | モジュレータだけの変更、表示ゲインだけの変更、元 Project へ復元。同じ timeline owner を維持 | 旧時刻解析、preview gain 無視、modulator smoothing 変更無視 |
| `output_shorter_than_audio_matches_export` | 1.4 秒、先頭／末尾／中間の seek | 旧時刻解析、較正を音源全尺へ、出力 PCM view 除去 |
| `output_longer_than_audio_matches_export_with_silence` | 3.2 秒、音源末尾前後／延長した無音／出力末尾 | 旧時刻解析、出力 PCM view 除去 |
| `output_equal_to_audio_matches_export` | 2.4 秒、先頭／末尾／中間 | 旧時刻解析 |
| `hash_flags_are_opt_in_and_independent` | 通常／列 hash のみ／frame hash のみ／両方、件数と既存列 hash の維持 | frame hash を無条件に計算 |
| `fixture_features_each_affect_visible_pixels` | 画像／進捗／時刻／スペクトラム／鍵盤／文字、2 modulator と smoothing の可視性 | Image の未使用 FillBrightness へ Average binding を変更 |
| `preview_texture_only_converts_rgba_and_downscales` | 下記 D1 の実表示経路 | 旧時刻解析、UV 左右反転、描画中の texture 再書換 |

D1 は全要求で RGBA 各 pixel → `Color32::from_rgba_unmultiplied` → worker の `ColorImage` →
採用 texture delta の全 pixel を検査する。最終 RGBA はラスタライザの黒不透明 canvas への合成結果で alpha=255。
非不透明な出力の premultiplication を検証したとは扱わない。
専用テストは実 `app.draw_preview` を小さい headless 領域へ描き、対象 mesh が一つ、WHITE tint、
縦横比を保つ縮小、UV 全域と各頂点の座標対応、preview texture の書換なしを検査する。
書換検査は `ctx.run` が返す `FullOutput.textures_delta` を使い、font atlas 更新は除外する。

変異の記録は `target/h3b-verification/`。10 種類すべてコンパイル成功後に対象テスト失敗（exit 101）を検出し、
新規 10 件すべてに少なくとも一つの失敗確認がある。`mutations.json` にコマンド・失敗名・復元 source SHA-256 を記録。
各変異でソースの元 bytes を `finally` で復元し、通常コードで以下の gate が成功した。

| 変異 | ログ（同ディレクトリ） |
| --- | --- |
| preview を旧 `SpectrumSource::full_frame_at` へ戻す | `mutation-legacy-time-analysis.log`（8 件失敗） |
| metadata は `time(n)` のまま、scene だけ再生位置で評価 | `mutation-scene-transport-time.log`（画素 hash で失敗） |
| 較正入力を音源全尺へ戻す | `mutation-calibration-full-source.log` |
| preview の PCM view を音源実長へ戻す | `mutation-preview-without-output-view.log`（短尺／長尺が失敗） |
| preview key で表示ゲインを 0 にする | `mutation-ignore-preview-display-gain.log`（変更後の frame 0 で画素不一致） |
| Average binding を画素に届かない Image FillBrightness へ | `mutation-vacuous-average-fixture.log` |
| UV を左右反転 | `mutation-texture-uv-flip.log` |
| preview 描画中に同じ大きさの黒 texture へ差替 | `mutation-texture-rewrite-during-draw.log` |
| frame hash の opt-in を外す | `mutation-hash-unconditionally.log` |
| モジュレータ編集後も旧 Average smoothing を preview に使用 | `mutation-ignore-modulator-smoothing-edit.log`（変更後の frame 82 で画素不一致） |

初回の gain 無視変異は固定 seek 3 点で画素が飽和し通ってしまった（`initial-undetected-gain.log`）。
列全体の可視差だけで要求位置の感度を保証できないため、差がある番号への要求を追加し失敗を確認した。
独立レビューで Image FillBrightness binding、UV の頂点対応、`ctx.run` 後の delta 観測先も補強した。
いずれもテスト感度／観測口の問題で、製品の preview/export 不一致ではない。

指定 gate（すべて `MSBUILDDISABLENODEREUSE=1`、test/dev profile、exit 0）:

| コマンド | 結果／ログ |
| --- | --- |
| `cargo test -p musicvideo-editor` | 153 passed（既存 143 + 新規 10）、`editor.log` |
| `cargo check -p musicvideo-editor --features dev-tools --bins` | 成功、`dev-tools.log`。test-only 観測口のない製品 build も確認 |
| `cargo test -p musicvideo-core -p musicvideo-ui` | core 115 + 経路 2、UI 124 passed、`core-ui.log` |
| `cargo test -p mimageviewer --lib mvexport` | 15 passed、`mvexport.log` |
| `cargo fmt --check` | 成功、`format.log` |
| `python scripts/check_ui_glyphs.py` | 危険 glyph 0、`glyphs.log` |

指定範囲は **409 passed / 0 failed / 0 ignored**。`gates.json` に実行条件・時間・終了値を保存。
最後にモジュレータの差番号への追加要求だけを補強し、影響する設定変更テスト 1 件を再実行して成功
（`settings-final.log`、重複分は 409 件へ加算しない）。他のテスト／製品コードの有効な結果を再利用する。
要求型の変更が通常 CLI にも届くため `cargo check -p mimageviewer --features dev-tools --bin mvexport_cli` も
追加実行して成功（`cli-check.log`）。最終 `cargo fmt --check` も成功（`format-final.log`）。
追加検証のコマンド・終了値は `additional.json`、変異 runner は `run.py`／`finish.py` に保存する。

検証範囲は brief 指定の preview 開始 → worker → 結果採用の画素と、GUI 共通 lib の書き出し。
headless fixture の `audio_player` は None。音声デバイス／clock／通常 transport の実サンプル丸めや
音源末尾での seek clamp の実機検収を済ませたとは扱わない。延長した出力尺の番号は preview worker へ直接要求する。
正本との仕様上の相違はない。描き手・解析器・通常出力の動作を変えない検証段なので、確認用 core binary は作らない。
本体アプリ／エディタ GUI の起動とコミットなし。開始時からの未追跡 `out.mp4` は操作していない。

独立レビュー（`6c1784aa2`、GPT-6.1 Sol / xhigh、設計担当が依頼）: 指摘なし、**ACCEPT**。これで H3（H3a + H3b）を完了とする。
圧縮後の MP4 の画素と実機の再生は、この検証の対象外（実機確認は P4b の利用者確認で扱う）。

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

2026-10-14 の master (`edcc26578`) 統合では、この共有境界を保ったまま入力の意味を明示する。
本体は `KeyboardCenterInput::FundamentalProbabilities` で HarmNet 確率（未取得は入力 0）を渡し、
Bass は `KeyboardBassInput::AbsoluteMidi` で選択済み MIDI をそのまま描く。書き出しの既存 API は
`SpectrumEnergy` / `PitchClass` を選び、倍音加点・低域整理・エネルギーからの octave 選択を維持する。
本体の設定配色は選択した `music_note_color` の色を共有 `key_fill_with_color` の blend に渡す。
書き出しの既存五度圏配色・カスタム Fill は維持し、HarmNet や本体の設定配色へ暗黙に変更しない。
配置・sustain・残光・倍音比・blend・Bass 三角形は同じ core の判断を通し、ホストに local 鍵盤を戻さない。
非同期状態を増やす代わりに既存の単一 worker / 結果チャネルと typed 入力で差を表現する。

同じ統合でエディタの IME fixture は master の処理済みキー配送モデルへ適応した。
現行の正本は [IME 処理済みキー設計](ime-process-keys-plan.md) とする。Windows の winit 経路は
`allow_ime=true` の Process press を遮断し、`KeyHold` の Hidden / Forwarded で配送済み press を記録する。
release は状態を除去して Hidden だけ遮断、Forwarded / 未登録は配送し、focus 喪失で全状態を消去する。
native 生キー経路の release 保持とは区別する。エディタ fixture は Process press と対応する Hidden
release の両方を除き、Enter 確定の終端 Disabled は同 pass / 別 pass の両方を検査する。

横長スペクトラムも同じ共有方針とする。対数周波数軸、trail / onset の状態遷移、ghost → trail →
現在値 → onset の描画プリミティブ列は `music-core::spectrum_visual` が所有し、本体の egui 描画と
書き出しの CPU ラスタライザはその同じ結果を描く。export の `frame_at` は音声時刻の前進時にだけ
更新し、同時刻は cache を返す。本体は `poll` の新結果採用で `adopt_analysis_at` を1回呼ぶため、
progressive PCM の一時停止中再解析も同時刻の新 bands を反映する。描画は読み取り専用 `frame()` を使い、
描画回数で残光を進めない。双方とも後退／250ms超の移動で共有 seek 判定が状態をリセットする。
書き出し側の従来の等間隔 Bars は別の選択肢として維持する。

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
再測定は `cargo run -p musicvideo-editor --release --features dev-tools --bin musicvideo-dev -- --stationary-note-response` で半音別・音域別の値を
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
  Windows の製品 GUI bin はコンソールを持たず、標準エラーを明示的に捕捉しない直接起動では診断ログが見えない。
  開発診断には console の `musicvideo-dev` を使う。通常操作の失敗は画面内でも通知するが、GUI自体の
  起動失敗やpanicは `--data-dir` 無しではファイルにも残らない。data dirを推測する代替経路は追加しない。
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
段 2b は `cargo test -p musicvideo-ui --lib` と `cargo test -p musicvideo-editor`（P4b-1 以前は `musicvideo_lab`）、ホストの check を加える。
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

#### I2（IME が処理したキー）（2026-10-14、段 1）

利用者が開発版と v4.6.0 ポータブル版で確認した問題: 画像注釈の本文で日本語を入力し、
Enter をすばやく 2 回押すと確定だけされ、改行が入らない。間を空けると改行が入る。
本体は text 注釈モード中、composition / 直近 300ms の IME grace 中の Enter をすべて捨てるため、
2 回目も抑止する。その対処が必要になった根の候補は、Windows の `VK_PROCESSKEY` が winit の
`NamedKey::Process` になり、egui-winit の `logical_key.or(physical_key)` により通常の Enter へ
fallback する経路。Process の変換と fallback はソースと一致するが、物理キーは Windows の
scan code から作られるため、実 IME が Enter として届けるかは未確認。

**段 1 は記録だけで、入力挙動を変えない。** checksum を原本と照合した egui-winit 0.33.3 を
`vendor/egui-winit` に取り込み、root patch で本体・ラボの依存をそろえる。
`MIV_IME_KEY_TRACE=<出力ファイルの絶対パス>` を最初の State 構築で 1 回だけ読み、既定は無効。
有効時は KeyboardInput / Ime の到着時刻（統合起動からの単調 ms）、viewport、キー metadata と、
処理後に実際に追加された egui の Key / Cut / Copy / Paste / Text / Ime を記録する。
文字内容は残さず、UTF-8 byte 数・空・改行の有無・cursor の有無だけを残す。文字キーもマスクする。
worker が追記・行ごとに flush し、UI thread でファイル I/O は行わない。失敗は黙って記録停止し、
再試行・復旧・終了時の完了待ちは設けない。入力本体の既存変換・抑止・focus policy は維持する。
これは非同期入力の新しい状態機械を作る作業ではなく、原本の出力を読む診断だけであり、
modal 化や live rebuild の追加は不要。

取り込み・秘匿・出力形式・ソース確認の詳細は
[`vendor/egui-winit/PATCHES.md`](../vendor/egui-winit/PATCHES.md)、作業範囲と完了条件は
[`brief-musicvideo-i2.md`](brief-musicvideo-i2.md) を参照。段 1 は `282f173c4` / `1cbe1888f`。

段 1 の自動検証: 原本 archive の全 9 ファイルの byte 照合、egui-winit 一系統、本体 core check、
共有 UI / ラボ / egui-ime、本体 ime_focus / egui_focus_policy、診断単体 12、fmt が成功。
`test-full.ps1` は App のサイズ制限テストで失敗するが、master 単独のテスト binary
（2026-10-14 01:40 build）でも App=110,264 bytes で同じテストが失敗しており、I2 とは無関係の既存失敗
（別作業に切り出し済み）。

**段 1 の実機記録（2026-10-14 深夜、設計 lead がエージェント操作で記録）。** 隔離データ
（`--data-dir`）で起動した開発版本体の画像注釈テキスト本文欄と、scratchpad へコピーしたラボの歌詞欄で、
この PC の IME（Google 日本語入力）を使って記録した。文字内容は記録していない。

| 操作 | winit の到着 | egui-winit が egui へ渡したもの |
| --- | --- | --- |
| 変換中の文字キー・変換の Space | press: `logical=Named(Process)`, physical=実キー | `Key(A..Z / Space)` press |
| 確定の Enter | press: `Process` / physical=Enter。同じ ms に続けて `Ime Preedit("")`→`Commit`→`Disabled` | `Key(Enter)` press（Commit より前）、`Ime.Disabled`, `Ime.Commit`, `Ime.Disabled` |
| 変換中の Esc | press: `Process` / physical=Escape。続けて `Preedit("")`×2→`Commit("")`→`Disabled` | `Key(Escape)` press, `Ime.Disabled`… |
| 候補選択中の Tab | press: `Process` / physical=Tab。続けて `Preedit` | `Key(Tab)` press |
| 上記キーの release | `logical=Named(実キー)`（Process ではない） | `Key(実キー)` release |
| IME 開状態・未変換の Enter | press: `logical=Named(Enter)`, text=改行 | `Key(Enter)` press |
| 確定直後（約 35ms 後）の Enter | 同上（通常の Enter） | `Key(Enter)` press |

観測された症状:

- 本体（注釈本文）: 確定→すぐ Enter で 2 回目の改行が入らない（利用者報告と同じ）。2 回目の Enter は
  通常キーとして届いており、text_mode の 300ms Enter 除去が捨てている。間を空ければ改行が入る。
  変換中の Esc は変換だけ取り消し、欄・テキストモード・フルスクリーンは残る。候補選択中の Tab で
  focus は移らない。
- ラボ（歌詞）: **確定の Enter だけで改行が入る**（逆向きの症状）。ラボには 300ms 除去が無く、
  Process 由来の `Key(Enter)` が multiline TextEdit に届くため。
- 根の候補は実機で確認できた: Windows が IME の処理したキーを `VK_PROCESSKEY` で届け、winit が
  `NamedKey::Process`（physical=実キー）とし、egui-winit が physical へ fallback して通常のキー press を
  egui へ渡している。Enter / Esc / Tab / Space / 文字キーのすべてが同じ経路。
- 未確認: Google 日本語入力以外の IME（MS-IME 等）。IMM32 経路では IME が処理したキーを `VK_PROCESSKEY` で
  届ける（`ImmGetVirtualKey` の文書）。TSF を直接使うアプリでは通常の VK のまま IME が配送を止める経路もあるが、
  winit / 本体は IMM32 経路なので遮断設計の反証にはならない。ただし他の IME・TIP が、この経路へ届ける
  消費済みキーをすべて Process にすることは未確認。本体の動画 native presenter は以前から
  `VK_PROCESSKEY`（0xE5）に egui キーを割り当てず捨てている（`egui_key_from_virtual_key`）。

**段 2 の方針: IME が処理したキーを egui のキー入力として渡さない（根の修正）、その後に対症療法を外す。**
2 つの段に分ける。段 2a は根の修正と、Process 由来のキーのためだけにある対症療法の撤去。段 2b は
300ms grace の見直しで、利用箇所が約 160 と広いので別の段にする。

段 2a（設計。独立レビュー ACCEPT 2026-10-14、P1/P2 なし）:

1. **vendored egui-winit の `on_keyboard_input` で、`logical == NamedKey::Process` の press を egui の
   `Event::Key` にも `Text` / `Cut` / `Copy` / `Paste` にもしない。** 判定は既存の Cut/Copy/Paste 判定より前に置く
   （IME が処理した Ctrl+V 等も IME のもの）。repeat も同じ。winit の `Ime` イベントは今までどおり渡す。
   release（実キーとして届く）は**そのまま渡す**。対応する press を持たない release は egui の keys_down から
   何も消さないだけで、`key_released` はリポジトリ内で使っていない（2026-10-14 grep）。
   `key_down(Enter)` の利用は FsClose の押下継続抑止 1 か所（`ui_fullscreen.rs` 31157 付近）で、
   IME の Enter は押下中に数えられなくなるが、閉じる判定は Enter の press イベントを要するので閉じる側には働かない。release を消すために
   「Process だった物理キーの集合」を持つ案は、状態を 1 つ増やし focus 喪失時の掃除も要るので採らない
   （状態の組み合わせを減らす方針）。`EventResponse` の計算は変えない。
   これで winit 経路の規則が、動画 native presenter の `VK_PROCESSKEY` を捨てる規則と一致する
   （入力経路が 2 つあっても「IME が処理したキーはアプリのキーではない」という規則は 1 つ）。
2. **Process 由来のキーのためだけにある対症療法を外す。**
   - 本体 `src/ui_fullscreen.rs` の text_mode 中の Enter 除去（`f8ff63911`）。**利用者報告の直接の修正**。
   - `tools/comic_lab` の `consume_ime_enter`（同じ除去のラボ版）。専用の `ime_composing` field と初期化も一緒に外す。
   - `crates/egui-ime` の「変換中の Esc press を除く」処理。**空 Preedit の補い**（egui-winit が
     `Preedit("")` を `Disabled` に変換するため、Esc 取消で preedit が残るのを防ぐ）は別の原因なので残す。
   - `crates/egui-ime` helper の focus lock filter の `tab: ime_active`。Tab で focus を移さないのは
     アプリ全体の Tab 方針（製品の選択、`tab_focus.rs`）が持っており、IME 用の上乗せは不要になる。
     矢印の lock は製品の選択なので残す。
   - 残すもの: egui 本家の `remove_ime_incompatible_events`（upstream）、アプリ全体の Tab 方針、
     focus 復帰（Alt 等の IME 以外の原因もある）、helper の `return_key(None)`。これらと 300ms grace は
     段 2b で個別に見直す。
3. **テスト。** winit の `KeyEvent` は外部から合成できないので、判定を「winit のキー情報 → 渡す／渡さない」の
   純関数に切り出して単体テストする。IME の Enter / Esc / Tab を通常の `Key` press として流している既存テストは
   旧入力のモデルなので、実機記録どおり（press なし・release あり・Ime 列）へ直す。特にラボの
   `raw_lyrics_enter_keeps_native_newline_semantics_and_commits_only_once` は確定 Enter の改行を期待しており、
   改行が入らない期待へ変える。本体の注釈本文で「確定→すぐ Enter で改行が入る」ことを
   handler-level のテストで固定する（実機記録の Ime 列＋通常 Enter press）。Commit・release・直後の通常 Enter が
   **同じ pass に入る場合と別 pass に入る場合の両方**を固定する。通常の Enter / Esc / Tab、IME 以外の focus 復帰、
   clipboard、非ラテン配列の physical fallback のテストは残す（IME 中という理由だけで通常 press のテストを
   書き換えない）。vendored egui-winit は workspace 外なので、その単体テスト
   （`cargo test --manifest-path vendor/egui-winit/Cargo.toml --lib` 相当）を `scripts/test-full.ps1` の
   追加実行対象に入れる。
4. **実機確認**は段 1 と同じ記録手順で、設計 lead が隔離データの本体とラボで 5 シナリオを再実行し、
   利用者にも本体で確認を依頼する（detached viewer と、使っていれば MS-IME も）。trace は段 2b の確認にも使うので
   残す（既定無効）。

**段 2a の実装記録（2026-10-14、`62bc9f47c`）。**

- egui-winit の純関数 `should_forward_keyboard_input` を入口に置き、Process press / repeat を
  Key / Text / Cut / Copy / Paste の生成前に遮断。release、Ime、EventResponse は維持。
  trace は既定無効で残し、拒否した press の追加イベントは `egui=[None]` となる。
- 本体 text_mode の Enter 除去、comic_lab の `consume_ime_enter` と専用状態、共有IME補正の
  composing Esc press 除去と helper の IME Tab lock を撤去。空 Preedit 補完、矢印 lock、
  全域 Tab policy、focus 復帰、`return_key(None)`、300ms grace と `dialog_*_pressed` は維持。
- IME処理キーのfixtureを press なし・release あり・Ime列へ更新。通常キーと非IME focus回復、
  clipboard / 非ラテンfallbackの検証は維持。本体の実 fullscreen 経路＋本文 TextEdit に
  確定→すぐEnterの同一 / 別pass回帰2件、comic_lab本文にも回帰2件を追加。
- `test-full.ps1` に egui-winit の無filter lib test を追加。環境差として、standalone manifest は
  同梱upstream lockの旧 image / tiff（jpeg-decoder）がoffline cacheに無く実行不可だったため、
  root graphの `cargo test -p egui-winit --lib` で同じvendored crate全件を実行する。
  設計上の前提矛盾は無し。コミット・アプリ起動・実機確認は行わない。

実装担当が作業中に自分で依頼した確認（別 context）は ACCEPT だったが、これは正式な独立レビューとして扱わない。
正式な独立レビューは設計 lead が別セッションで依頼する。

| コマンド | 結果 |
| --- | --- |
| `cargo test -p egui-winit --lib` | 16 passed（既存診断12＋新しいキー配送4） |
| `cargo test -p egui-ime` | 17 passed |
| `cargo test -p musicvideo-ui -p musicvideo_lab` | UI 121 / ラボ 75 passed |
| `cargo test -p comic_lab` / `cargo check -p comic_lab` | 9 passed / check成功 |
| `cargo test -p mimageviewer --lib ime_focus` | 12 passed |
| `cargo test -p mimageviewer --lib egui_focus_policy` | 5 passed（`target/i2a-focus-policy.log`） |
| `cargo test -p mimageviewer --lib ime` | 434 passed / 2 ignored（`target/i2a-ime.log`） |
| `cargo test -p mimageviewer --lib text_mode` | 3 passed（本文の新しい2回帰を含む、`target/i2a-text-mode.log`） |
| `cargo fmt --check` / vendor rustfmt check / `git diff --check` | 成功 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 成功 |
| `.\scripts\build-dev.ps1 -PreserveRuntime` | 実装担当の環境では起動中 core の停止が Access is denied。設計 lead が隔離 core を止めて実行し成功 |
| `cargo build --release -p musicvideo_lab` | 成功（40.26秒、`target/i2a-release-lab.log`） |
| `.\scripts\test-release-build-safety.ps1` | PASS。egui-winit追加後の順序と5回実行に更新、追加差分も独立レビュー ACCEPT |
| `.\scripts\test-full.ps1 -SuppressCrashDialogs` | exit 101。既知のAppサイズ制限失敗と、段1のログにもある `capture_region_focus_and_escape_terminals_preserve_event_order` のstack overflowで本体libが途中終了。ほかのworkspace targetは成功（`target/i2a-test-full.log`） |
| 本体libの未完了範囲の補完 | `--list` と完了ログの差分2,717件を、同じpack-build-tools付きtest exeの `--exact` 複数filterで分割実行。2,704 passed / 13 ignored / 0 failed、exit 0。`RUST_MIN_STACK=67108864`。既存stack overflowのテストも成功。App構造・テスト上限・製品挙動は変更しない（`target/i2a-run-unfinished.ps1` / `i2a-unfinished-summary.log`） |
| 全件照合 | 本体lib全12,691登録（重複登録3も照合）: 12,633 passed / 57 ignored / 1 failed、未確認0。失敗は既知のAppサイズ制限のみ。ほかのworkspace target: 1,584 passed / 7 ignored / 0 failed。原full gateのexit 101は維持し、full gate成功とは扱わない（`target/i2a-lib-coverage.json`） |
| 追加vendor検証 | `cargo test --manifest-path vendor/egui/Cargo.toml --lib`: 25 passed（root graphは26 passed）。`cargo test --manifest-path vendor/egui-wgpu/Cargo.toml --features winit --lib`: 11 passed。`cargo test --manifest-path vendor/eframe/Cargo.toml --no-default-features --features wgpu --lib`: 18 passed |

**段 2a の実機確認（2026-10-14 04:10 頃、設計 lead がエージェント操作で実施）。** `62bc9f47c` の build-dev 本体を
隔離データ（`--data-dir`）で、release ラボを scratchpad のコピーで、どちらも trace 有効で起動し、Google 日本語入力で確認した。
結果は入力欄の表示と、IME を閉じてから打った目印の文字の位置で判定した。

| シナリオ | 本体（注釈テキスト本文） | ラボ（歌詞） |
| --- | --- | --- |
| 確定だけ | 改行なし（修正前と同じ） | 改行なし（修正前は確定の Enter で改行が入った） |
| 確定→すぐ Enter | **改行が 1 つ入る**（修正前は入らなかった＝利用者報告） | 改行が 1 つ入る |
| 変換中の Esc | 変換だけ取消。欄の focus・テキストモード・フルスクリーンは残る | 変換だけ取消 |
| 候補選択中の Tab | focus は欄に残り、続く確定でも改行なし | 同左 |
| IME オフの通常 Enter | 改行が入る | — |

trace（scratchpad の `night\trace\main-2a.log` / `lab-2a.log`）では、本体・ラボとも `logical=Named(Process)` の press
18 件すべてが `egui=[None]`（egui へ何も渡していない）。未確認: detached viewer、MS-IME 等の他の IME、
利用者の実機・実設定。

**段 2b（設計案 第 5 版、2026-10-14 設計 lead。第 1 版 REVISE（P1 1・P2 2）、第 2 版 REVISE（P2 1・P3 1）、第 3 版 REVISE（P1 1・P2 1）、第 4 版 REVISE（P2 1）。実装前）: 300ms grace を外し「変換中か」だけで判定する。**

経緯と理由: `IME_EVENT_GRACE`（300ms）は「Windows が IME 終了（`Disabled` / `Commit`）と、IME が処理した
Esc / Enter を別フレームで届ける」ことの吸収として入った（`App::ime_input_active` の doc、CLAUDE.md の IME 対応節）。
段 2a で IME が処理したキーの press は egui へ届かなくなったので、grace 中に届く Enter / Esc は利用者が新たに押した
通常のキーになった。grace はそれを捨てるだけになり、利用者報告と同じ型の取りこぼしを各所に残す
（例: 名前・タグ・検索欄で確定→すぐ Enter で決定できない、確定→すぐ Esc で閉じない、確定直後のショートカット無視）。

1. **判定を 1 つにする。** `egui-ime` の `ImeFocusState` から `last_event_at` と `IME_EVENT_GRACE` を外し、
   `ime_input_active` / `ime_input_active_in_viewport` は「その viewport が composing か」だけを返す。
   composing は pre-pass の plugin が今 pass の Ime 列まで反映した値なので、同じ pass の `[Commit, 通常 Enter]` は
   決定として通り、`[Preedit]` の最中は抑止される。native presenter のように pass 前に queue を見る読み手は、
   pending events を最終状態まで射影する `ime_composing_with_pending_events` だけを使う。「未処理の Ime event が
   1 つでもあれば active」の `ime_input_active_with_pending_events` は廃止して射影版へ統合する（2b と同じ契約）。
   `TextInputPhase::ImeGrace`（`src/keyboard_input.rs`）は Focused / FocusRecovery に吸収せず、「composing」の段として
   名前と意味を変えて残す（focus も recovery claim も無いまま composition だけが残る場合があるため、レビュー指摘）。
2. **呼び出し側は原則変えない。** `dialog_enter_pressed`（30）・`dialog_escape_pressed`（68）・直接の
   `ime_input_active`（約 50）・native presenter・gamepad・musicvideo-ui の履歴ショートカットは、定義の変更だけで
   新しい規則になる。個別の分岐を足さない。grace を前提に「Commit と同じ pass の Enter を捨てる」ことを期待している
   箇所（`should_close_fullscreen_on_enter`、検索バーの `handle_keyboard` 抑止 `app.rs` 53715 付近、
   specified-scale の数値 popup、削除確認の Y/N）は、Process 遮断後も意味が通るかを 1 つずつ確かめ、
   不要になったものはコメントと分岐を消す。
2a. **focus 復帰を「キーの所有」と「focus を戻すか」に分ける（第 1〜3 版の指摘、第 4 版）。** いまの helper は
   1 つの判定（`keyboard_focus_recovery_input` と grace の `ime_active`）を 2 つの役割に使っている。
   - **キーの所有（recovery claim、変えない）**: `recovering_text_input` は「直前の pass まで helper 管理の欄に focus があり、
     この pass が pointer 無しの key press で、いま何にも focus が無い」とき、その欄にこの pass のキーを所有させる。
     IME とは無関係で、欄で押した通常の Esc / Enter が同じ pass でアプリのショートカット（フルスクリーンを閉じる等）に
     渡らないことを守っている（`focused_bookmark_title_escape_does_not_close_fullscreen`）。判定も対象キーもそのまま残す。
   - **focus を戻すか（restore、作り直す）**: `restore_focus_for_ime_key` の条件を、時刻（grace）ではなく理由で決める。
     戻すのは次をすべて満たすときだけ:
     `lost_focus()`、pointer 無しの key press がある（今の `keyboard_focus_recovery_input`）、いま他の widget に focus が無い、
     そして「**この pass の Ime を反映した後も composing**」または「**この pass に Enter / Escape の press が無い**」。
     composing は plugin が pre-pass で今 pass の Ime 列まで反映した値で、helper が TextEdit 構築前に読む（今の `ime_active` の採取位置）。
   - 何が変わるか（段 2a 以降、egui に届く Enter / Escape の press は利用者が押した通常のキー）:
     - 確定と同じ pass・直後の pass の通常 Enter: single-line 欄は Enter で意図どおり focus を手放し（`builder.rs` 1016 付近）、
       検索欄・アドレス欄は `lost_focus()` で決定する。いまは grace 中だと focus が戻され決定されない。
     - 確定直後の通常 Esc: 欄から抜ける（間を空けたときと同じ）。いまは grace 中だと欄に戻る。同じ pass のキーは
       recovery claim が欄に所有させるので、Esc がそのままフルスクリーンを閉じることはない。
     - 変換中に通常の Esc / Enter が届いた場合（実機の Google / MS-IME では Process か配送なしで届かないが、テスト
       `crates/egui-ime/src/tests.rs` 581 付近が扱う）: composing なので今までどおり欄に戻す。
     - Alt+J と `Disabled` の pass で一時的に focus が外れる経路（`ime_alt_focus_loss_…`）: Enter / Esc が無いので戻す。
       IME の有無を問わなくなるので、IME 無しで Alt+文字の処理により focus が外れた場合も戻す（同じ「一時的な喪失」）。
     - Tab はアプリ全体の Tab 方針が traversal を止めて focus を動かさないので、条件に入れない。
   - アプリのコードが意図して helper 欄の focus を外す箇所（`surrender_focus`）は、検索欄の Enter 決定後
     （`ui_main.rs` 15553 付近、Enter の pass）とテストだけ（2026-10-14 grep）で、新しい条件と衝突しない。
   - **次 pass の再要求（retry latch）も同じ条件に従わせる（第 4 版 P2）。** いまの helper は復帰に成功すると
     「次の pass でもう一度 `request_focus()`」の印を立て、次 pass で TextEdit の入力処理の後に無条件で使う
     （`crates/egui-ime` 340 / 366 付近）。印は呼び出し側の `focus_request`（初回 focus 用の latch）があればそれに、
     無ければ helper 所有の一時値に立てる。このため、復帰の次 pass に利用者が押した Enter / Esc で欄が focus を手放しても、
     印が focus を取り戻し、検索欄の決定（`lost_focus()`）や Esc で抜ける動きが失われる。他の widget へ移った focus も奪う。
     直し方: (1) helper は復帰の印を**呼び出し側の `focus_request` に書かない**。印は常に helper 所有の一時値
     （viewport と widget の id で引く、今の `focus_retry_id`）に立てる。呼び出し側の `focus_request` は呼び出し側が明示した
     初回 focus 等の要求としてだけ消費し、今までどおり無条件に `request_focus()` する。(2) helper 所有の印を次 pass で使うのは、
     その pass でも上の復帰条件のうち「`lost_focus()`」以外（pointer 無し、他の widget に focus が無い、composing でなければ
     Enter / Escape の press が無い）を満たし、かつ欄がまだ focus を持っていないときだけ。満たさなければ印を捨てる。
     印は 1 pass 限りで、使っても捨てても消える（今と同じ寿命）。
   - テスト: 復帰（Alt+J / `Disabled`）→ 次 pass の通常 Enter（検索欄で決定され focus を手放す）、→ 次 pass の通常 Esc
     （欄を抜ける。同じ pass のキーは recovery claim が欄に所有させる）、→ 次 pass で他の widget が focus を取る、の 3 つを、
     呼び出し側が `focus_request` を渡す欄（検索欄・native 欄）と渡さない欄（ラボの欄）の両方で固定する。呼び出し側の
     初回 focus 要求が今までどおり効くことも固定する。
2b. **native presenter のタグ選択欄は、今 pass の状態で判定する（第 1 版 P2）。** `render_core.rs` 12521 付近は
   `egui_ctx.run` の前（plugin が今 pass の Ime 列を反映する前）に `tag_picker_ime_active` を取っているので、
   前 pass が composing・今 pass が `[Commit, Disabled, 通常 Enter]` のとき Enter を捨てる。
   pending events を最終状態まで射影する `ime_composing_with_pending_events` を使うか、closure 内で plugin の
   今 pass の値を読む。「未処理の Ime event が 1 つでもあれば active」の `ime_input_active_with_pending_events` は
   射影版と意味が違うので、射影版へ統合する。
3. **Win32 の生キー経路にも同じ規則を入れる。** Win32 キー queue（`src/key_input.rs` の `key_edge_from_message`）は
   `VK_PROCESSKEY`（0xE5）の edge も記録し、`KeyName::matches_win32` は JIS の ^ @ ¥ ろ を scan code だけで照合する
   （`src/keymap.rs` 861 付近）。変換中に打った ^ @ が grace / composing の gate の外でショートカットに一致しうる。
   queue 側で 0xE5 の press / repeat を記録しない（release は残す。押下保持の後始末に使うため）。
   **native presenter の生キー経路も同じ（第 1 版 P2）**: `src/video/native_window.rs` 2223 付近は 0xE5 を含むすべての
   key-down を公開し、`keymap.rs` の scan-code 照合（8341 / 9452 付近）へ届く。egui event への変換は
   `egui_key_from_virtual_key` が 0xE5 を捨てているが、生キーのショートカット経路は捨てていない。ここでも
   0xE5 の press / repeat を公開しない。これで「IME が処理したキーはアプリのキーではない」が egui-winit・
   本体 Win32 queue・native presenter の egui 変換と生キーの 4 か所で同じになる。
4. **段 2b の範囲外（残す）**: egui 本家の `remove_ime_incompatible_events`（pass 開始時の `ime_enabled` で
   Backspace・矢印・repeat を Commit より前に除くため、Commit と同じ pass の通常 Backspace / 矢印も消える既存挙動。
   upstream の不具合として別に扱う）、helper の focus 復帰と `return_key(None)`、空 Preedit の補い、アプリ全体の Tab 方針。
5. **テスト**: grace を前提にしたテスト（`noncomposing_and_ime_grace_escape_presses_are_preserved` の grace 部分、
   `ime_grace_is_text_input_even_with_generic_focused_ui`、specified-scale の「IME 終了＋キー」、
   `history_shortcuts_preserve_events_and_project_during_composition_and_grace_without_focus` など）を、
   「composing 中は抑止、Commit 後の同じ pass／次の pass の通常キーは通る」へ直す。代表ダイアログ（タグ編集・
   フォルダを開く・パスワード）で「確定→すぐ Enter で決定」「確定→すぐ Esc で閉じる」を handler-level で固定する。
   Win32 queue と native 生キー経路の 0xE5（press / repeat は捨て release は残す、JIS ^ @ ¥ ろ の scan-code 照合に
   届かない）を routing のテストで固定する。Alt/IME の focus 復帰テストは期待を変えずに残す。native presenter の
   タグ選択欄で、前 pass composing・今 pass `[Commit, Disabled, 通常 Enter]` と、Commit の次 pass の通常 Enter の両方を
   固定する。同じ ViewportId の viewport を閉じて開き直したときに composing が残らないこと（終端 event を失った場合）も
   テストで確かめ、残るなら viewport の破棄・focus 喪失で composing を落とす（grace の時間切れでは元々直らない既存の欠陥。
   その場合は修正範囲を報告して判断を仰ぐ）。検索欄・アドレス欄（`ui_main.rs` 15377 / 15550 付近）の実際の handler で、
   `[Commit, Disabled, 通常 Enter]` の同じ pass と、Commit の次 pass の通常 Enter の両方について、決定が実行され
   focus が手放されることを固定する。
6. **前提条件（実装前に満たす）: Google 日本語入力以外でも、確定の Enter・変換中の Esc が Process で届くこと。**
   この PC には MS-IME（TIP `{03B5835F-…}`）も登録されている。段 1 の trace 手順で MS-IME の 5 シナリオを記録し、
   確定の Enter が `logical=Named(Process)` で届くことを確かめる（通常の Enter で届く IME があると、grace を外すことで
   「確定と同時にダイアログが決定される」退行になるため）。あわせて、文節を区切っての部分確定・予測候補の選択・
   Alt を含む操作でも、IME が処理したキーが Process で届き、確定後に IME の候補が残っていてもキーが通常キーとして
   漏れないことを記録で確かめる（composing は受け取った Ime event の写しで、候補ウィンドウの表示までは追わないため）。記録は利用者に依頼するか、利用者の了承を得て設計 lead が
   入力方式を一時的に切り替えて行う（終わったら元に戻す）。
7. **状態を減らす方針との関係**: 時刻つきの状態（`last_event_at`）と、それに依存する「grace 中か」の分岐が消える。
   新しい状態は足さない。

利用者に見える変化: 日本語を確定した直後（300ms 以内）の Enter / Esc / ショートカットが、間を空けたときと同じに働く。
変換中（未確定文字があるとき）の扱いは変わらない。

**経緯**: 第 3 版の後に一度保留し、選択肢（A 狭い / B 全面 / C 見送り）を利用者に示した。2026-10-14 朝、利用者が
**B（全面）** を選び、「今までが少しおかしい動きだったなら本来の動きに直るのは問題ない」とした（確定直後だけ Enter / Esc の
結果が違う動きを揃える）。master へは音楽ビデオより先に入れる。

**前提条件 6 の記録（2026-10-14 朝、設計 lead が利用者の了承を得て MS-IME に切り替えて実施）**: 本体の注釈本文で
MS-IME を使い、確定・確定→すぐ Enter・変換中 Esc・候補選択中 Tab・候補一覧の Enter・部分確定（Ctrl+↓）を記録した。
IME が処理したキーは `Process` で届くか、**press 自体が届かない**（TSF の IME がキーを消費した場合、release だけが届く）の
どちらかで、egui へ press として渡ったものは無かった。部分確定は `Process`（physical=ArrowDown）で、
`Commit` の直後に `Enabled`・`Preedit` が続き composition が続く。確定直後の通常 Enter は通常キーで届き改行になった。
MS-IME は候補一覧での Enter を候補の選択に使い、もう一度 Enter で確定する（どちらも Process）。

このブランチ（musicvideo）で直すのは利用者の指示（2026-10-14）。本体側の修正も含むため、master への統合は
音楽ビデオ機能とは別に先行させるかを、段 2a の確認後に利用者と決める。

**段 2b の実施記録（2026-10-14、初回実装 `291dbf8e5`、正式レビュー REVISE（P2 1・P3 3）後の修正 `efce16eb0`、再レビュー ACCEPT）。**

段 2b の実機確認（2026-10-14 10 時台、設計 lead がエージェント操作で、隔離データの build-dev 本体と release ラボで実施）:

| 確認 | Google 日本語入力 | MS-IME |
| --- | --- | --- |
| 注釈本文: 確定だけ／確定→すぐ Enter／変換中 Esc／候補選択中 Tab | 期待どおり（改行は確定直後の Enter だけ） | 段 2a で確認済み |
| ローカル検索欄: 確定→すぐ Enter で検索が実行される | OK | OK（検索を解除してから実行を確認、Enter は確定の 42ms 後） |
| ローカル検索欄: 確定→すぐ Esc で欄が閉じる（間を空けたときと同じ） | — | OK（35ms 後） |
| ラボ歌詞欄: 確定だけで改行なし／確定→すぐ Enter で改行 1 つ | OK | — |
| 変換中に別の窓へ focus を移す（`efce16eb0` 後） | IME が確定し、確定文字が 1 つだけ残る | 同左 |

focus 移動で取消（Disabled だけ）が来る並びは、両 IME の実機では起きなかった（どちらも確定）。その並びは単体テストで固定。
記録中、利用者のディスプレイ切り替えのキー操作（Shift+文字、ScrollLock）が前面のテスト用本体に入り「SRS」が入力された
（trace で確認、製品の不具合ではない）。

- `last_event_at` / `IME_EVENT_GRACE` と pending-event 有無版 API を撤去し、composing と最終状態の投影に統一。
  keyboard phase は `ImeComposing` へ変更。検索バーの重複した旧 grace guard を撤去し、fullscreen Enter・
  削除 Y/N の composing guard は意味を確認して維持。native タグ欄は queue を drain する前に最終状態を射影する。
- helper の復帰は項目2aの条件に従い、key ownership は維持。既存 helper retry を pass 番号へ置換し、
  呼び出し側 latch へ書かず、同条件を満たす次 pass でだけ使う。欄を描かず閉じた後へ印を持ち越さない。
- 本体 Win32 queue / native 生キー公開の生成境界で 0xE5 press / repeat を遮断し、release を維持。
  JIS `^ @ ¥ ろ` の scan-code shortcut へ届かない routing 回帰を追加。
- 項目5の既存欠陥を再現したため、brief が許可した小範囲で共有 plugin を修正。
  `RawInput.viewports` に存在しない viewport を破棄し、所有 viewport の focus 喪失でも composing を落とす。
  同 ID の再利用・focus 喪失・生存する兄弟の独立性を検証する。detached 側の述語や lifecycle は変更しない。
- 前提照合の補足: specified-scale の raw DragValue は IME 無しの通常 Esc でも fullscreen を閉じる既存経路だった。
  承認済みの「直後も通常時と同じ」に合わせて対照検証し、数値欄の別の所有設計は追加しない。
  旧 fixture は pass 外の owner 取得で次 pass に古い composing owner を作っていたため、pass 内検証へ修正した。
  詳細は [指定サイズ計画 §19](fullscreen-specified-scale-plan.md#19-popup再レビューの回帰利用者説明修正2026-10-10)。
  それ以外に設計判断を要する前提矛盾は無い。
- 初回実装は brief commit `2c6e1d782` 上の差分として作成し、設計 lead が `291dbf8e5` へコミットした。
  以下は初回実装時の検証記録。レビュー修正での再検証は後段に分ける。

| コマンド（通常 feature、指定箇所以外は通常 test profile） | 結果 |
| --- | --- |
| `cargo fmt --check` / `python scripts/check_ui_glyphs.py` / `git diff --check` | 成功、危険 glyph 0 |
| `cargo test -p egui-winit --lib` / `cargo test -p egui-ime` | 21 / 20 passed |
| `cargo test -p musicvideo-ui -p musicvideo_lab` / `cargo test -p comic_lab` | UI 122 / ラボ 75 / comic 9 passed |
| `cargo test -p mimageviewer --lib ime` / `… --lib keyboard` | 448 passed・2 ignored / 39 passed |
| `… --lib focus` / `… --lib text_mode` | 118 / 3 passed。focus は通常 stack で既知の capture_region overflow を確認し、`RUST_MIN_STACK=67108864` で完走 |
| `… --lib ime_focus` / `… --lib processkey` / `… --lib native_tag_picker_` | 12 / 2 / 4 passed |
| `… --lib address_ime_commit_and_immediate_enter` / `… --lib search_ime_commit_and_immediate_enter` / `… --lib bar_focus_recovery` | 各2 passed。Alt 復帰→次 pass Enter も実 handler で決定・focus 解放 |
| `… --lib ime_handler_tests` / `… --lib delete_confirm_y_n_enter_act_immediately_after_ime_commit` / `… --lib specified_scale_real_numeric_ime_` | 6 / 1 / 2 passed。同一／次 pass と通常 Esc の対照を含む |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0 |
| `.\scripts\test-full.ps1 -SuppressCrashDialogs`（`RUST_MIN_STACK=67108864`） | exit 101。本体 12,651 passed / 57 ignored / 2 failed（1191.83秒）、他 workspace target は1,588 passed / 7 ignored / 0 failed。失敗は既知Appサイズ制限（110,600 bytes、上限110,000未満）と、生存一覧を省略したゲームパッドfixture。stack overflowは起こらず、実decoder EOFループも成功 |
| `cargo test --manifest-path vendor/egui/Cargo.toml --lib` / `… vendor/egui-wgpu/Cargo.toml --features winit --lib` / `… vendor/eframe/Cargo.toml --no-default-features --features wgpu --lib` | 25 / 11 / 18 passed。workspace失敗でscriptが到達しないvendor gateを別実行。egui-winit 21件の成功も再利用 |
| `cargo test -p mimageviewer --lib gamepad_text_input_gate_unions_all_three_surfaces --features pack-build-tools`（stack 64MiB） | fixture修正後1 passed / exit 0（`target/i2b-gamepad-fixture.log`、test exe `mimageviewer-b31a6c451310338d.exe`）。最終fmt / glyph / diff checkも成功 |
| `$env:MSBUILDDISABLENODEREUSE='1'; cargo build --release -p musicvideo_lab` | exit 0、50.04秒。`target/release/musicvideo_lab.exe`（`target/i2b-release-lab.log`）、起動なし |
| `$env:MSBUILDDISABLENODEREUSE='1'; .\scripts\build-dev.ps1 -PreserveRuntime` | exit 0。通常featureのcore 5分02秒、Remote 0.39秒、EPUB worker 0.35秒。`target/dev-runtime/`へ生成、VCRT検査 runtime=4 / PE=3 成功。既存プロセスの停止・成果物の起動なし（`target/i2b-build-dev.stdout.log` / `.stderr.log`） |

個別ログは `target/i2b-<filter>.log`、共有側は `i2b-egui-winit.log` / `i2b-egui-ime.log` /
`i2b-ui-lab.log` / `i2b-comic.log`、fmt / check は `i2b-fmt.log` / `i2b-core-check.log`。
全体 gate の native console は `target/i2b-full-console.log`、列挙／照合は `i2b-lib-test-list.log` /
`i2b-lib-coverage.json`。12,710登録（重複登録も含む）を照合し、未確認0・不足する実行回数0。
`i2b-test-full.log` は PowerShell Transcript の実行／exit記録で、native console本文は上の別ログへ保存した。
ゲームパッドの旧fixtureは ROOT pass に生存中のfullscreen viewportを含めていなかったため、
破棄通知に相当する入力になっていた。生存集合を明記し、続く実際の破棄でgamepad gateが解除される検証も加えた。
製品コードは全体gate時から変えておらず、このfixture以外の成功結果は再利用する。
fixture補完を合わせた本体の有効結果は12,652 passed / 57 ignored / 既知Appサイズ制限1 failed、
未確認0。補完は当該fixtureとその消費経路に限定し、原full gateのexit 101は維持して成功とは扱わない。
指定された確認用buildまで完了。コミット・アプリ起動・実機確認は行っていない。

**正式レビュー第 1 回への修正（2026-10-14、未コミット）。**

- P2: focus 喪失で shortcut の composing claim は終了し、既存の preedit を終端まで待つ状態を
  同じ viewport の enum に保持する。後続の Disabled / 空 Commit で取消補完するため、追加の latch は持たない。
  focus 喪失時に先に取消する簡素化は、まだ取消が確定していない preedit まで消してしまうので採らない。
  WindowFocused(false) / RawInput.focused=false の各経路、終端が前／後、同一／別 pass の全組み合わせを
  実 TextEdit の buffer で検証する。取消で元の文字列へ戻り、非空 Commit は正しく残る。
- P3: retry の生成 pass から外部 latch の有無を切り替え、caller latch=false と helper の pass 印を確認してから
  Enter / Esc / 他 widget / 欄の消失へ進む。video architecture と Everything の旧契約を更新し、
  I1 段 2a 表へ混入した I2b の 5 行をこの節の初回検証表へ移した。
- 指示により build-dev / test-full は再実行しない。起動中の dev core への操作、コミット・アプリ起動も行わない。

再検証は指定の通常 feature で全件成功。`cargo fmt --check` と
`cargo check -p mimageviewer --bin mimageviewer-core` は exit 0。
`cargo test -p egui-ime` は 22 passed、`cargo test -p musicvideo-ui -p musicvideo_lab` は 122 / 75 passed。
本体の `cargo test -p mimageviewer --lib ime` / `focus` / `keyboard` は 448 passed・既存 2 ignored / 118 / 39 passed。
test stack は既知の focus テストの overflow を避けるため `RUST_MIN_STACK=67108864`。
ログは `target/i2b-review1-{fmt,egui-ime,ui-lab,ime,focus,keyboard,core-check}.log`。

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
`comic-core` は出荷済みの画像注釈と共有しているので、計算・描画挙動を変更しない。
T1 検収で既存 `text_effect_padding` の可視性拡張・再エクスポートを認め、独立レビュー後は
可視領域だけを焼く公開 API の追加も行う。既存の計算・描画挙動は維持する（利用者判断）。

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

#### T1 実装・検証記録（2026-10-09、Codex 実装担当）

- `TextStyle` を Text / TimeText / Lyrics の `style: Animatable<TextStyle>` で共有する。
  旧 JSON の省略項目は装飾なしへ default。スタイルは comic-core の Stroke / Shadow / Echo /
  Background 型を再利用し、bold / italic / TextGlow は公開しない。UI は共通編集関数を
  `edit_animatable_property` と既存編集コマンドへ接続（タイムライン、キー編集、Undo を共有）。
- 同じ TextBlock の文字・フォント・サイズ・字間・行間・整数原点から、白い glyph coverage と
  透明 glyph の装飾 RGBA をそれぞれ `bake_overlay` で焼く。装飾 → Fill × glyph coverage を
  straight RGBA で source-over し、レイヤー opacity / rounded clip をその結果に一度だけ適用。
  mask も同じ合成 alpha を使うので、歌詞フェード・回転・グローが同じ silhouette を参照する。
  装飾ありは単色 body 再構成へ流さない。装飾なしの既存の丸め・描画経路は保持する。
- 到達範囲は comic-core の既存 `text_effect_padding` を公開・再エクスポートして共有する。
  初回実装の余白式の複製は検収で削除した。この検収時の comic-core の変更は doc comment / pub / 再エクスポートだけで、
  関数本体の計算と画像注釈の描画挙動は変えない。musicvideo の TextBlock は glow を設定しないため、
  旧計算との等価性を glow なしの 960 通り（装飾の有無、負値・小数、echo 枚数の clamp 境界）で固定した。
  公開関数の戻り値へ AA 用 1px を加えて ceil する。大きい別キャンバスとの alpha 全画素一致で
  余白不足を検出する。論理寸法は別保持し Start / Center / End の基準を変えない。
  「矩形の外」は文字の論理矩形に対する到達範囲と解釈し、レイヤー rect の既存 clip は D15 通り維持。
  追加の前提差: 負の letter_gap は glyph を原点より左へ送り、負の line_gap は
  advance で計算した高さより glyph の ink が下へ出る（comic-core layout.rs の horizontal 配置）。
  装飾あり、または間隔が非ゼロの場合は公開 glyph_px_bounds_gid と TextLayout の配置から ink の超過を測り、
  装飾の各辺の余白へ加算する。bitmap の原点 offset を (x,y) で保持し、横の超過を
  高さへ加えない（負の字間の長文で面積が二乗で増えることを避ける）。
  論理寸法・揃え基準は変えず、glyph / 装飾の両方を大きいキャンバスと
  全画素比較する負の間隔テストを追加。間隔ゼロでも Á のアクセントは論理寸法を超えるので、
  装飾時の字形超過も別キャンバスとの全画素比較で固定した。装飾なし・間隔ゼロの
  既存確保寸法はそのまま保持する。
  追加の前提差: 既存 UI 色 helper は Color32.r/g/b の乗算済み値を straight Rgba8 へ戻していた。
  Color32 の to_srgba_unmultiplied を使う共通変換へ修正し、文字装飾 / Fill / Glow の色が
  編集時に二重に暗くならないようにした。半透明色を core で描く回帰テストで alpha が一度だけ効くことを固定。
  この straight RGBA 修正は既存不具合の修正として検収で採用された（利用者判断）。
- 補間は構造（各 Option、追加縁取りの本数、echo count）が一致する場合だけ全連続値と色を補間。
  1 個でも異なればスタイル全体を Step。単一キー、同時刻の後勝ち、範囲外 hold は既存 Track と共通。
- キャッシュキーは文字列・font key・サイズの bit 値・スタイル全項目（float の bit 値と色）を含む。
  2 つの独立した byte-budget LRU に分け、静的用 56 MiB、補間用 8 MiB、合計 64 MiB。
  Const とキー値そのもの（Step / endpoint / 色の丸めでキー値と一致するフレーム）は静的用、
  キー間の新しい値は補間用とする。静的要求が補間用にヒットした場合は、同じ Arc を静的用へ昇格する。
  補間の流入は静的 LRU を一切 eviction しない。
  超過サイズの結果は描画だけ行い保持しない。budget は bitmap Vec capacity、キー所有容量、
  bitmap / entry / key 構造体を数え、HashMap bucket / allocator / 描画中の一時確保は含まない。
  8 MiB は 1080p の代表的な装飾文字の数フレームを保持する規模とし、残りを静的文字・時刻へ確保した。
  複雑な中断・復旧状態は追加せず、既存 render worker / Assets owner 内のキャッシュだけで処理する。
- 初回自動検証: 追加 core 18 件、UI 4 件（360px の headless 配置・実 pointer による ON/OFF と
  追加/削除・3 レイヤーのキー編集と Undo/Redo を含む）。全指定 crate は core 98、UI 121、
  lab 75、egui-ime 17、計 311 件成功。L1〜L3 / I1 の既存テストも成功。
  HEAD の YuGothM.ttc / 320×180 / 無変調 4 フレームを事前採取し、通常・opacity 0.5・
  回転 27°・回転 + glow の FNV-1a hash を固定して新実装との一致を検証する。
  2 枚への個別 opacity、白い装飾統合 bitmap、構造不一致時の部分補間、装飾の単色 body 再構成の
  4 変異すべてで対応テストが失敗（exit 101）。加えて下記の負の間隔の確保補正を外す変異も
  回帰テストが検出。装飾時の間隔ゼロの字形超過補正を外す変異、半透明 UI 色の
  非乗算変換を外す変異も検出（計 7 変異）。復元後に全指定 crate を再検証した。
- `mvexport_cli` の dev-tools check、UI 依存境界（eframe / cpal / ffmpeg / mimageviewer なし）、
  fmt、UI glyph 0 件は成功。検証ログは `target/t1-validation/`。
  アプリ起動・build-dev・commit は利用者指示により実施しない。独立レビューは利用者側が手配する。
- release のラボ build も成功。`cargo build --release -p musicvideo_lab` は起動せず成果物だけ作成。
  初回検証対象の SHA-256 / HEAD / rustc 情報は `target/t1-validation/source-manifest.json`。
  YuGothM.ttc の SHA-256 は `target/t1-validation/font-sha256.txt` に記録した。
- 検収修正後: 余白の等価性テスト 1 件を追加（T1 の追加は core 19 / UI 4、計 23 件）。
  `cargo test -p comic-core` は通常 121 件成功、ignored の既存 `effect_bake_bench` 1 件も
  別実行で成功（comic-core 全 122 件）。musicvideo-core 99 / musicvideo-ui 121 / musicvideo_lab 75 件も
  再検証し、今回実行した計 417 件が成功した。egui-ime 17 件は変更・依存変更がないため初回の証拠を再利用。
  `cargo check -p comic_lab`、`cargo check -p mimageviewer --bin mimageviewer-core`、`cargo fmt --check` も成功。
  comic-core の余白関数の本体が HEAD と完全一致することをソース比較でも確認した。
  ログ・コマンド・終了コード・ソース SHA-256 は `target/t1-review-validation/` に記録。
  この可視性変更時は計算・描画結果が同じため下記の初回合成計測・キャッシュ計測を再利用した。

#### T1 独立レビュー後の修正（fce65bb15 / master v4.5.0 マージ 2c229c2be 後）

- P2（巨大な焼成確保）: LRU の保持上限と、焼成前の RGBA 確保上限を分ける。
  1 回の焼成で所有する glyph + 装飾の RGBA Vec 合計は **8 MiB 以下**（`MAX_TEXT_RASTER_BYTES`）。
  上限を満たす通常の全文 bitmap は従来通り焼き、超える場合はレイヤーの既存 rect clip に入る部分だけを
  最大 1024×1024 のタイルで順に焼き、すぐ既存の body / mask 合成へ渡す。全タイルを一時 Vec に集めない。
  64 MiB の LRU 保持予算は別枠。フォント・glyph coverage / dilation の comic-core 内作業領域、
  出力 Frame・回転・グローの既存作業領域はこの RGBA 8 MiB の計上対象ではない。
  方向別の余白だけでは斜めエコーの巨大面積が残るため、可視領域の制限とタイル化を採用した。
  UI 上限の縮小、効果の弱化、オフセットや枚数の clamp 追加はしない。
- comic-core に `bake_text_region` を追加し、既存文字 painter を領域書込アダプタで共有する。
  既存の `text_effect_padding` の計算・描画順序・色合成は変更しない。浮動小数点の配置・画素丸めは
  元の全文 canvas 座標のまま計算し、整数の書込先だけを領域へ移す。原点を焼成前にずらす方法は
  0 近傍の切捨てや大きい座標の丸めを変え得るため採らない。既存注釈経路は原点 (0,0) のアダプタを通す。
  負・小数・6145px の原点、全装飾（glow 含む）で既存 `bake_overlay` の領域との RGBA 全バイト一致を固定。
  回転・グローでは既存の回転前レイヤー rect 全域を保ち、最終フレームでの不可視判定を先に掛けない。
  論理寸法・揃え位置は不変。キャッシュキーは Full / Region（原点・寸法）の型を含む。
- P3（昇格・二重保持）: `CacheState` が lookup / publish を所有する。静的要求で補間 lane にヒットしたら
  entry を取り出して byte 数を減算し、同じ Arc を静的 lane に入れる。焼成は mutex 外で行い、
  並行 miss 後の publish は同じ mutex 内で両 lane を再確認する。静的側への優先を逆順でも維持し、
  同じキーの二重保持・byte 計上の重複を防ぐ。
- P3（clip の検出力）: opaque glyph と背景の重なりを 64px / radius 32px の角丸境界へ置く。
  0.05 < clip < 0.95 の画素を円の距離から独立に求め、body と mask を opacity × clip 一度分の期待値と比較する。
  body の clip²、mask の clip² の別々の変異で失敗することを確認した。
- 今回追加は comic-core 1 / musicvideo-core 4、計 5 件。最大エコー count 12 / offset ±512 の水平・垂直・
  斜め、outline 128 / shadow spread・blur 128 / offset 512 / 背景 padding・corner 256 等の 6 組み合わせで、
  従来の全文確保が 1 GiB 超になる fixture を用いる。実際の各タイルの Vec capacity 合計が 8 MiB 以下で、
  分割しない可視領域描画と glyph / 装飾の全バイトが一致することを静的・補間の両要求で確認した。
  装飾なしの既存 4 hash は不変。指定テストは comic-core 122（既存の ignored 性能テスト 1 件を除く）、
  musicvideo-core 103 / musicvideo-ui 121 / musicvideo_lab 75、計 **421 件成功**。
  `cargo check -p mimageviewer --features dev-tools --bin mvexport_cli`、fmt、diff check も成功。
  clip 二度掛け 2 件、昇格の欠落、片 lane だけの publish、2048px タイルの 5 変異を全て検出した。
- ログ・コマンド・終了コード・ソース SHA-256 は `target/t1-independent-review/`。
  別セッションの rustc 稼働を確認したため、合成時間の再計測は行わない。下記は初回実装の参考測定であり、
  Region メタデータ追加後のキャッシュ byte 数・今回修正後のプロセス peak の実測としては扱わない。
  アプリ起動・build-dev・release ラボ build・commit は利用者指示により実施しない。

#### T1 条件付き ACCEPT 後の追加修正（f54b6e5b6 後）

再レビューは条件付き ACCEPT（comic-core の既存描画は挙動不変）。残った P2 / P3 への対応:

- P2 のコード上の原因を確認: 各タイルの glyph / 装飾で cache reset、layout、全文 glyph・多段 dilation を
  再実行していた。1024² の RGBA 2 枚は 8 MiB を使い、metadata を含めると補間 LRU に保持できない。
- comic-core に `prepare_text_regions` / `PreparedTextRaster` を追加。既存 painter の記録先を追加し、
  同じ TextLayout から glyph / 装飾を一度だけ準備する。background polygon と glyph coverage・色・
  元の float 座標・描画順を保持し、coverage を Arc で共有する。全タイルの replay は layout / raster /
  dilation を繰り返さない。準備結果は別 bake の generation reset 後も有効。
  効果の計算式は複製しない。旧注釈入口は従来経路を維持する。
  領域の sample 範囲は既存の float 加算→整数切捨てを用いた単調探索で求め、負座標で同じ画素に入る
  複数 sample の順序も維持する。通常の全文 bitmap は既存 bake_overlay 経路を保つ。
- musicvideo は大きい文字の準備結果を LRU に持ち、全タイル・mask/body・同じスタイルの後続フレームで共有。
  body/mask の glyph と装飾が両方透明な画素は Fill・clip・合成計算を省く（alpha 0 の no-op と同じ結果）。
  **総保持上限64 MiB = 静的RGBA24 / 補間RGBA8 / 準備結果32 MiB**。1焼成のRGBA上限 **8 MiB** は維持する。
  静的枠は全HD 2枚（約16 MiB）が入る24 MiB、補間RGBAの従来8 MiBは保ち、準備枠4 MiBでは
  多段影のcoverageが保持されなかったため32 MiBを確保した。glyph coverageを含む準備結果を予算に計上し、
  surface間の共有分は保守的に二重計上。HashMap bucket / Arc control block / TLS参照・一時scratch・
  Frame / glow / rotation bufferは保持予算の厳密な計上対象外。プロセス全体の8 MiB保証ではない。
  `TextCacheStats.prepared_bytes` は `transient_bytes` の内数なので重ねて足さない。
- P3: `fce65bb15` のcomic-coreをgit showでtarget下へ実ファイルとして抽出し、Cargo.lockをコピーして依存を固定。
  旧 `bake_overlay` のRGBAからFNV-1a-64を採取し、`tests/legacy_text_pixels.rs` に **22個の定数**を固定。
  同梱OFLフォント ZenKurenaido-Regular.ttf をinclude_bytesで読み、Windowsフォントの版やskipに依存しない。
  装飾あり/なし×負・小数・四辺・中央の注釈12ケース、半透明message windowの中央・四辺10ケース。
  wrap=falseの長文がclip=Some(content rect)を踏む。採取方法はテストのコメント、ログは `old-capture.log`。
  PowerShell→PythonのASCIIパイプによる新fixtureの文字化けを検知し、UTF-8に修正して旧版から再採取した。
- 追加 **4件**（comic-core 3 / core 1）。固定期待値2件、別bake後の準備結果と領域画素一致、
  mask/body相当のArc再利用・準備LRU eviction・総容量上限を固定する。
  座標1pxずれ、message clip欠落、準備保持欠落、領域丸めずれの **4変異を全て検出**（FAILED / exit101）、復元済み。
  comic-core **125**（unit122 + integration3、既存benchmark1 ignored）、core **104**、UI **121**、lab **75**、
  egui-ime **17**、計 **442件成功**。装飾なし既存4hash維持。CLI dev-tools / comic_lab / 本体core check、
  fmt、UI glyph0、UI禁止依存なし。検証コマンド・終了コード・SHA-256は `target/t1-rereview/`。
- 計測器に `matrix <normal|tile>-<static|interp>-<plain|glow|rotate|both>` の16ケースを追加。
  各1920×1080、30warmup。通常は従来のText / TimeText / Lyrics 3層（64px、縁3/7、影offset8・blur6・spread2、
  背景padding12・corner8）、300フレーム。タイルは全rect Text1層（64px、影offset=(512,-512)・blur48・spread16、
  echo12枚・offset=(512,512)、その他は通常同様）、120フレーム。補間はoutline幅3→18を300秒で動かす。
  glow radius12・spread3、rotation12°を単独と両方で測る。mean / nearest-rank p95 / 最終RGBA hashを出す。
  各ケースを別processとし、Windows OSのpeak working set / private commitを観測する。
  ドライバ `target/t1-rereview/run-matrix.py` は開始前5秒のPID・CPU-time差分を記録する（psutil使用）。
- **P2の受け入れ性能測定は環境待ち、未完了**。10分間の開始前監視で他rustcが約4.9 CPU秒/5秒を消費し、
  Codexにも活動があったため、競合中のintervalをrejectedとした。再試行のbaseline2は通常静的1ケースを走らせたが、
  測定中にもCodexのCPU活動を検出し不採用とした（baseline2-results.json）。採用できるmean / p95 / peakはまだない。
  自分の全build終了後の確認も `final-cpu-after-build.json` に記録。休止タイミングを利用者へ確認中。
  診断実行と下記初回4.274ms等は今回の受け入れ性能値として使わない。
- releaseラボ `cargo build --release -p musicvideo_lab` 成功（fixture修正後の最終build 31.72秒、初回build 43.34秒）。標準出力先が使用中でないことを確認し
  `target/release/musicvideo_lab.exe` を作成、起動なし。commit / アプリ起動 / build-devなし。

#### T1 計測器・eviction テストの検収修正（338a38899 後、2026-10-09）

- eviction の確認を「確保された Vec が非空」から、文字の alpha が存在する論理 glyph 領域の
  **eviction 前 / 保持中の Arc / eviction 後の再準備の RGBA 完全一致**へ変更。
  全透明 RGBA を拒否することも同じテストで確認する。全透明化の変異は新比較で exit101、
  旧非空判定では exit0。比較を旧判定へ戻す変異は oracle の確認で exit101になる。全て復元済み。
- ignored な `target/t1-rereview/run-matrix.py` を置き換え、リポジトリ内の
  `scripts/musicvideo/t1-measure.py` と `test_t1_measure.py` に保存した。
  計測前5秒・計測中・終了後の通知回収中を20ms間隔で監視する。
  rustc / cargo / link / lld-link / cl / codex（自分の Codex ancestor は PID + 作成 FILETIME で除外）
  の新規PIDまたは **CPU消費が少しでもあれば棄却**し、フェーズ・PID・理由・CPU秒を出す。
  Codex の sandbox broker は worker に含めない。明示的な所有PIDは `--own-codex-pid` で指定可能。
  watched process の Windows handle を終了まで保持し、終了後の最終CPU時間も集計する。
  新PIDの初回CPUをゼロ扱いせず、PID再利用も作成時刻で区別する。
  `Win32_ProcessStartTrace` の通知で、poll の間に開始・終了したPIDも棄却する。
  handle取得前に終了してCPUを採取できなかった場合も、採取不可を記録して棄却する。
  WMI通知やCPU/メモリ取得が利用できなければ採用しない（snapshot-only監視へのfallbackなし）。
- **実行コマンド（今回は正式計測しない。利用者が競合のない環境で後日実行）**:

  ```powershell
  python scripts/musicvideo/t1-measure.py
  ```

  Python 3.10以上、psutil、WMIのプロセス開始通知を利用できるWindows PowerShellが前提。
  psutilがなければ `python -m pip install psutil` で準備する。
  このCodex実行環境ではWMI Startがアクセス拒否だった。後日の実行シェルでも拒否される場合は
  WMI通知の権限があるシェル（必要なら管理者PowerShell）で実行する。権限不足の結果は不採用。
  スクリプト1本で `cargo build --locked --release -p musicvideo-core --features text-measure --example t1_measure`
  を行い、全ケースを別processで実行する。MSBUILDDISABLENODEREUSE=1をbuildに設定する。
  アプリ・ラボは起動しない。既定は同一マシン・1920×1080・YuGothM.ttc・Rayon 4 threads。
  フォントは `--font`、Rayonは `--rayon-threads` で指定でき、フォント・exe・driverのSHA-256と
  HEAD / 差分のSHA-256・実行条件をJSONに記録する。
- 通常/タイル × 静的/補間 × plain/glow/rotate/both の **16ケース**を、装飾なし / 現行で各3回。
  各ケースはcold 1 frame、同じt=0のwarmup 30 frame、測定300 frame（10秒相当）。
  通常の文字3層と、影offset=(512,-512)・blur48・spread16・echo12枚offset=(512,512)の
  タイル文字1層を使う。glow radius12・spread3、rotation12°。
  別に通常/タイル × 全補間/静的＋補間混在（glow＋回転）を、両variant各3回。
  長尺は **9001 frame、t=0〜300秒の全区間・終端を含み、縁幅3→UI最大128px**。
  t=300にもlayerが存在するようfixtureのproject尺を301秒にする。
  静的＋補間では固定文字を追加し、3つの保持枠を同時に使う。
  合計 **120回、244,824測定frame**。短い検証は下記で行える:

  ```powershell
  python scripts/musicvideo/t1-measure.py --smoke --no-build --output target/t1-followup/smoke
  python -m unittest discover -s scripts/musicvideo -p 'test_*.py' -v
  ```

- 修正前fce65bb15相当は互換JSON harnessがなく、タイルで1GiB超の旧全文確保が起こり得るため
  **既定では未測定理由を記録**する。別に準備した同条件の旧rendererは `--before-exe <path>` で追加可能
  （equivalentであること・source revisionは採取者が記録する）。旧装飾ありと旧装飾なしも各3回追加し、
  装飾なしは現行と揺らぎを比較する。現行を修正前の代用にしない。
  装飾なしの退行比較には `--reference-json <旧測定のresults.json>` を指定できる。
  machine / OS / CPU / release / font hash / Rayon / case設定が一致した採用3回とRGBA hashを確認し、
  3回のrangeと標準誤差から揺らぎを評価する。参照がない場合は **NOT ASSESSED** を出す。
- 出力は `target/t1-measure/results.json`、`results.txt`、各processのJSONL。
  renderのmean / nearest-rank p95と、初回準備のlayout・coverage・RGBAの累積時間、cold frame全体、
  font/Frame初期化時間を別記する（準備時間は並列callerの累積、wall時間との単純差ではない）。
  cache 3枠と各焼成RGBAの最大値、30 frameごとの保持量、OS working set / private commitの時系列と
  終了後handleからのOS最終peakを記録する。長尺の後半の中央値変化と頭打ちの目安も出す。
  `text-measure` featureだけに計時・最大焼成量counterを追加し、通常buildの計算・描画・計時負荷は変えない。
- 出力の合否目安: 装飾なしの悪化は3回の測定揺らぎ以内（参照がなければ未判定）、通常p95は
  16.7msを十分下回る（80%=13.36msをheadroom目安）、保持は **24+8+32MiB**、各焼成RGBA **8MiB以下**。
  タイルp95が8.3msを超えた回のmean/p95を列挙し、設計判断は利用者が行う。
  memoryの後半増分はmax(8MiB,後半初期中央値の5%)以内を頭打ちの目安とし、生の時系列も残す。
  途中棄却で止まった部分実行やsmokeを最終合格にしない。
- 実行時間目安: 平均5/20/100msなら約 **20/82/408分** ＋監視約11分、cold/warmup・release build。
  128px縁・タイルの実時間はこれより長くなり得る。回数・frame数とこの目安は実行開始時・結果にも出す。
- **今回は性能計測を実施していない**（利用者指定。他セッションのCPU使用が継続）。
  1ケース3frameのheadless smokeはJSON/表を出し、WMIアクセス拒否・rustc/CodexのCPU消費で不採用。
  競合なし/あり、新規・終了・再利用PID、短命PID通知、監視失敗、実handle終了後CPU/peak取得を
  Python **15件**で確認。core **104** / UI **121** / lab **75**と合わせ **315件成功**。
  計測feature付きeviction 1件も成功。装飾なし既存4hash維持、fmt / release headless例build / CLI dev-tools check成功。
  検証・変異・smoke記録は `target/t1-followup/`。comic-coreの変更なし。
  commit / アプリ起動 / build-dev / 正式matrix・長尺実行なし。

**T1 初回計測結果（2026-10-09、headless core 測定器の実測、独立レビュー修正前）**

再現用: `cargo build --release -p musicvideo-core --example t1_measure` の後、
`target\release\examples\t1_measure.exe composition` と `... long` を実行する。
測定器は `crates/musicvideo-core/examples/t1_measure.rs`。アプリ・ウィンドウは開かない。
Scene を evaluate して同じ render_frame を呼ぶ書き出しの合成経路を測定し、FFmpeg の
音声デコード / 動画エンコード / MP4 書き込みは含めない。最大メモリはこの core 測定プロセスの値。

条件: Windows x86_64 / Intel Core Ultra 9 285（24 logical processors）、rustc 1.94.1 / release、
Rayon thread 数の環境変数は未指定。1920×1080 / 30fps、単色背景 + Text / TimeText / Lyrics 各 1 層、
文字サイズ 64px / YuGothM.ttc。装飾は内側 3px + 外側 7px の縁取り、影（offset 8,8 / blur 6 /
spread 2）、背景（padding 12 / corner 8 / alpha 200）。歌詞は 3 秒ごとに切り替えて通常の fade を使う。
装飾なしとの比較は同じ内容で、各 30 フレーム warm-up 後に 300 フレームを測る。
時計は evaluate 後から render_frame の終了まで。mean は算術平均、p95 は昇順の 285 番目 / 300。

| 1080p 合成 | mean (ms) | p95 (ms) |
| --- | ---: | ---: |
| 装飾なし | 1.8254 | 2.4652 |
| 縁取り 2 本 + 影 + 背景（3 文字レイヤー） | 4.2738 | 4.9153 |

mean 差 +2.4484ms（約 2.34 倍）。文字装飾を毎フレーム焼き直す試験では、3 レイヤーの
内側縁取り幅を 3 → 18px の Linear キーで 300 秒かけて動かし、全 9,000 フレームを合成した。
全フレームでキャッシュの byte-budget を検査し、毎フレーム stats の最大値を採取した。

| 5 分相当 / 9,000 フレーム | 最大値 |
| --- | ---: |
| キャッシュの計上 byte 数（静的 + 補間） | 9,610,788 bytes (9.166 MiB) |
| 補間 LRU の計上 byte 数 | 8,374,904 bytes (7.987 MiB、8 MiB 未満) |
| キャッシュ entry 数 | 15 |
| Windows peak working set | 46,125,056 bytes (43.988 MiB) |
| Windows peak private commit（PeakPagedMemorySize64） | 43,307,008 bytes (41.301 MiB) |

900 フレームごとの記録と最終記録で、静的結果は 1,235,884 bytes / 2 entries を維持した。8 MiB の補間 LRU は
代表的な 3 レイヤーの約 4 フレーム分を保持し、古い補間結果だけを捨てる。9,000 フレームで
結果が溜まり続けないことと、静的結果を補間が追い出さないことを stats で検証した。
参考: 全 evaluate / 合成 / stats 採取の経過は 189.782 秒（エンコード速度の測定ではない）。

OS memory peak は PowerShell で process Handle を保持し、100ms 間隔で OS の peak カウンタを
生存中に採取した。キャッシュ計上には HashMap bucket / allocator / 描画中の一時確保を含めないため、
OS peak も別に記録する。他セッションの本体テスト・rustc の終了を待ち、最終開始前に
cargo / rustc / MSBuild がいないことを検査した。計測中も 1 秒間隔で同プロセスを検査し、
composition / long とも検出 0 件。主なログは `target/t1-validation/measure-composition.log`、
`measure-long.log`、`memory-long.txt`、`cpu-conflicts-*.txt`。最終ソースの長尺測定で
cargo が途中に検出された 2 回は `rejected-*` / `rejected2-*` に退避し、長尺測定だけを再実行した。
最終回は CPU 使用時間の累積も記録し、検出 0 件 / CPU 使用時間 0 秒（`cpu-load-long.txt`）。
表には競合のない測定だけを採用する。計測 wrapper は process の Handle を保持して
正常終了の ExitCode を検証する。


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

#### P4b の設計（2026-10-14、実装前レビュー 3 回目で ACCEPT。P4b-1 brief: [brief-musicvideo-p4b1.md](brief-musicvideo-p4b1.md)）

P4b は性質の違う作業を含むので、4 段に分ける。各段で独立レビューを通してから次へ進む。

| 段 | 内容 | 主な不変条件 |
| --- | --- | --- |
| P4b-1 | クレート移動と FFmpeg デコード | 挙動不変（デコーダの差を除く）。既存テストはすべて移って通る |
| P4b-2 | 音声スレッドが再生を所有 | UI スレッドはデバイス照会・stream 生成・drop をしない。f32 以外の出力形式も鳴る |
| P4b-3 | スナップショット入力の MP4 書き出し（モーダル） | 書き出しはファイルを読み直さず、プレビューと同じ PCM・素材を使う |
| P4b-4 | 未保存の確認と終了 | 開く・新規・閉じる（P4c で最近使ったものから開く）がすべて 1 つの判定を通る |

**P4b-1: クレート移動と FFmpeg デコード。**

- `tools/musicvideo_lab` を `crates/musicvideo-editor`（package `musicvideo-editor`、製品 bin
  `mimageviewer-musicvideo`）へ `git mv` で移す。ラボは残さない（D16）。workspace の members を更新する。
  launcher と同じく default-members には入れず、`-p musicvideo-editor` で扱う。
- ラボの診断・ベンチマーク用の引数（`--benchmark`・`--preview-benchmark`・`--sample-frame`・
  `--project-*-diagnostic` など）は、製品 bin の引数解析から外し、同じクレートの開発用 bin
  `musicvideo-dev`（`required-features = ["dev-tools"]`）へ移す。製品 bin の引数は P4c で
  `--audio` / `--project` / `--data-dir` だけになる（P4b-1 の時点では引数なしで起動するだけでよい）。
  **共通のホスト（App・worker・診断の本体）とその回帰テストはクレートの lib に置き、製品 bin と診断 bin は
  入口だけにする**（診断 bin に入れたテストは通常の全体テストで走らないため）。両方の入口で I1 の
  `setup_egui` 登録を通す。旧ラボのソースパスを読む監査テスト（`musicvideo-core/tests/analysis_routes.rs`）、
  `Cargo.lock`、`dev-tools` 付きの検査コマンドもこの段で更新する。
- 本体 lib（`mimageviewer`、path 依存）を依存に加え、音声は `audio_decode::decode_audio_file_to_stereo_f32`
  と `mvexport::default_audio_stream_index`（書き出しと同じ既定トラック）でデコードする。symphonia と
  ラボ独自の resample を外す。PCM の識別は今のまま `ResourceRevision`（H3a の key）。
- 本体 lib に依存するので、エディタのビルドとテストには本体と同じ `vendor/` が要る（テストは
  `target/debug/deps` に FFmpeg DLL が要る）。「vendor 不要のラボ」という性質はここで無くなる（D16 で決定済み）。
- I1 の IME 登録入口・H1 / H2 / H3a の所有と順序の検査は、移動後もそのまま通ること。

**P4b-2: 音声スレッドが再生を所有。**

- 専用の音声スレッドが、デバイス照会・`cpal::Stream` の生成・維持・drop をすべて行う（`Stream` は `!Send`。
  WASAPI の stream の drop は内部で join するので、drop もこのスレッドに留める）。
  UI はコマンド（音声の差し替え・再生・一時停止・シーク・終了）をチャネルで送り、再生位置・再生中か・
  直近のエラーを共有スナップショットから読むだけ。UI スレッドはこのスレッドを待たない（終了時の join も
  しない。終了コマンドを送って切り離す）。
- **コマンドは FIFO で処理し、UI が送るたびに操作番号を振る。** スナップショットには、適用済みの操作番号と
  PCM の `ResourceRevision` を付ける。UI は自分が最後に送った操作番号より古いスナップショットの位置を
  採用しない（差し替え・Undo・同じ PCM への連続シークの直後に古い位置が戻らない）。
- 位置は現行（`playback.rs` の再生 anchor）と同じ**実際に聞こえている位置**を出す。出力バッファへの
  書込カーソルで代用しない。H3a の `index_at` へ渡すのはこの位置。
- UI が読むスナップショット用の lock は、音声コールバックが PCM を書く処理の lock と分ける
  （UI の読み取りがコールバックを待たせない）。
- 出力形式は f32 に加え i16 / u16 などを受け、サンプルを変換して書く。デバイスの sample rate が 48kHz で
  ない場合の扱いは現行と同じ（現行の resample 方針を確認して維持する。新しい方針は足さない）。
- デバイスが開けない・途中で失われた（まれ）は、ログと画面の通知だけ。次の再生操作で開き直しを試みる
  （それ以上の回復機構は作らない）。
- 音声の差し替え（H2 の Undo を含む）は、PCM の `Arc`・`ResourceRevision`・位置を 1 つのコマンドで渡す。
  現行どおり差し替えでは停止状態にし、再生 anchor を消す。古い PCM の解放はこのスレッドで行う。
  デバイスの初期化に失敗した後も、次の再生操作を受け付ける。

**P4b-3: スナップショット入力の MP4 書き出し。**

- `MusicVideoExportRequest` は `audio_path` をやめ、**スナップショット**を受け取る: Project の複製、
  素材の bundle（`Assets` を Project の全 `images` を残して fork したもの。フォントは Arc で共有し、
  文字・拡縮のキャッシュは空から作り直してよい）、PCM の `Arc`（samples・sample rate・長さを揃える前の
  元音源長）と**その識別**（`ResourceRevision`）。書き出しはファイルを読まない。識別は書き出しの
  `AnalysisKey` へそのまま渡す（`Arc` の同一性を識別に使わない。Undo の復元は新しい `Arc` を作り、
  元の識別を保つ）。本体 lib は識別を型引数 `AnalysisKey<I>` で受け、`musicvideo-ui` へ依存しない。
  エンコーダ・ビットレートは開始時の要求の値で固定する。
- 書き出しは長さを揃えるための PCM 全体の複製を作らない。driver が H3a の view（出力尺で見た PCM）を
  論理長まで走査し、既存の AAC 入力単位（最大 16,384 stereo フレームのチャンク）だけを実体化して渡す。
  切り詰め・無音延長・PTS は view の規則に従う。共有 AAC エンコーダの API は変えない。
- 較正は書き出しの worker で同じ関数から計算し直す（入力が同じなので結果も同じ。エディタの較正結果を
  渡して key を照合する分岐を作らない）。5 分で数秒かかるので、進捗の段階に「反応範囲の計算」を含める。
- CLI（`mvexport_cli`）は自分で FFmpeg デコードしてから同じ入口を呼ぶ。
- **実測の入口（2026-10-14、設計担当判断）**: 本体の `mvexport_cli` にはエディタのプロジェクト形式の
  読込を追加しない。エディタの開発用 bin `musicvideo-dev`（`dev-tools`）に
  `--export-mp4 <project.mivmv.json> <output.mp4> [--encoder X] [--video-bitrate N] [--frame-hash]` を追加する。
  エディタと同じプロジェクト読込・素材解決・FFmpeg デコード・スナップショット作成を通し、製品 GUI の
  書き出しと同じ lib の関数を使う。実測用の編集済みプロジェクトと合成素材は `target/` 以下だけに生成する。
- **書き出しを始められる条件**: 素材がすべて Ready、Project / LRC / 素材の読込・保存・歌詞の行対応確認が
  どれも進行中でない。満たさないときは書き出しボタンを無効にして理由を出す（書き出し中に完了した処理を
  保留・復帰させる仕組みは作らない）。
- **モーダル**: 開始時に再生を一時停止し、新しいプレビュー worker の起動を止める（実行中の worker は
  取消を通知し、結果は非同期で受けて捨てる。UI は待たない）。書き出し中は編集・開く・保存・新規・再生を
  受け付けない。進捗（段階と frame 数）と「中止」だけを出す。中止は既存のキャンセル（書きかけファイルの削除）。
  中止の完了は frame 数ではなく worker の終端の結果で判断する。完了・失敗・中止の結果を表示してモーダルを閉じる。
- 出力先は保存ダイアログで選ぶ（既存の rfd の使い方と同じ）。要求に `replace_existing` を持つ。
  選んだ時点で存在したパス（ダイアログで上書きを承認）は true、存在しなかったパスと CLI は false。
  書き出しは同じディレクトリの一時 MP4 に書き、最後の公開そのもので保証する: false なら上書きしない公開
  （`persist_noclobber` 相当。現行の `exists()` 確認後の `rename()` は確認と公開の間に作られた出力を
  上書きし得るので置き換える）、true なら置換付きの公開。原本を先に削除しない。false で公開時に
  存在していたら失敗の通知だけにする。取消・公開失敗では既存の出力を残す。
- 書き出し設定（エンコーダ・ビットレート）は D11 の既定値を出し、保存しない（D10）。
- 受け入れ: 書き出し開始後に元の音声・画像ファイルを変更・削除しても、出力は開始時点の内容になる。
  Undo で以前の音声へ戻した後の書き出しは、戻した音声を使う。5 分 1080p60 の編集済みプロジェクトを
  完走して実測する（CLI でよい）。

**P4b-4: 未保存の確認と終了（D10「未保存の編集と終了」の実装）。**

- 操作の意図を 1 つの型で持つ: `開く(パス)` / `新規` / `閉じる`。P4c の「最近使ったものから開く」は
  `開く(パス)` に合流し、専用の分岐を作らない。保留中の意図は同時に 1 つだけで、文書 session に所属させる
  （別の文書へ切り替わったら捨てる）。確認ダイアログはモーダルなので、他の意図は来ない。
  例外はウィンドウを閉じる要求で、保留中の意図を「閉じる」に置き換える。
- **歌詞の未確定の編集を先に片付ける**: 歌詞の行対応の確認待ち（`lyrics_editing.rs` の `PendingResolution`）は
  まだ Project に入っていないので、未保存の判定では見えない。意図を受けたら、確認待ちがあればまずその確認を
  出し、解決（Project へ反映）か明示の取り消しをしてから同じ意図へ戻る。dirty に OR するだけでは、保存しても
  未確定のまま残り「保存成功→再確認」を繰り返すので採らない。再開の条件は、全レイヤーの
  `has_pending_lyrics_edit()` が false になったこと（`resolve_pending()` は選択後も `NeedsResolution` を返し得るので
  確認を続け、結果が `Unchanged` でも完了として扱う）。取り消しは既存の `cancel_pending()` と、返された TextEdit の
  rollback の復元を使う。反映は既存の編集 transaction・履歴・`note_edit` を通す。新しい歌詞確認の所有者は作らない。
- 判定は 1 つの関数: 未保存でなければ実行。未保存なら「保存 / 破棄 / キャンセル」。保存は通常の保存
  （新規なら保存先の選択）を経る。保存の失敗・キャンセルなら意図を捨てる。
- **保存完了後の再判定の位置**: 保存の完了処理（`complete_save`）は保存の基準を更新するだけにする
  （戻り値の true は「基準を現在の文書へ採用した」であって「clean になった」ではない）。意図の再判定は、
  そのフレームの入力と完了の採用（`poll_projects` の後の `poll_lrc` などを含む）がすべて済んだ後に 1 回行う。
  保存を待つ間も確認のモーダルを出したままにする。
- **先行する Project 読込**: H1 は、読込開始後に編集が無ければ完了した読込を自動で採用する。保存中や意図の
  保留は判定に入らないので、「B の読込開始 → A の保存 → 閉じる意図 → B の読込完了」で B が採用され、意図が
  捨てられる。そこで**意図を受理した時点で、開始済み・待機中の Project 読込を取り消す**。Ready になっていた結果は
  既存の遅延解放の経路で捨て、取消済みの完了は採用しない（UI は終了を待たない）。意図の保留中は読込の新規開始も
  受け付けない。判定を通った `開く(パス)` だけが、通常の読込入口へ進む。保留中の意図が無い通常の読込は、従来の
  H1 の判定と保存完了の所属をそのまま使う。
- 保存中に意図が来たら、保存の完了を待ってから判定する（待つ間も UI は止めない）。
- 書き出し中の閉じる要求は「書き出しを中止して閉じますか」。承認なら中止の完了を待ってから判定へ進む。
- ウィンドウを閉じる要求は eframe の close 要求を取り消して（`CancelClose`）この判定へ回し、
  実行が決まった時点で閉じる。実行が決まった後に自分で送った `Close` は、もう一度 `CancelClose` しない
  （承認済みの終端状態を持つ）。これを handler テストで固定する。

**組み合わせを減らすための検討。**

- 採用: 書き出しをモーダルにし（D10）、さらに開始条件で進行中の読込・保存・確認待ちを除いた。
  モーダルだけでは開始済みの非同期処理の完了が書き出し中に採用されるため、入口で組み合わせを無くす。
- 採用: 書き出しの較正を worker で計算し直す。エディタ側の較正の完了待ちや key 照合との組み合わせが無くなる。
- 採用: 保留中の意図は 1 つだけ。確認中の追加の意図は閉じる要求だけで、置き換えで扱う。
- 採用: 音声スレッドは UI が待たない一方通行のコマンド。デバイスの異常はまれな失敗として通知だけ。

**まれな失敗は割り切り（ログ + 通知）**: 音声デバイスの喪失、書き出し先の書き込み失敗、一時ファイルの
置き換え失敗。回復のための再試行や途中状態の保持は作らない。

実装前レビュー 1 回目（2026-10-14、GPT-6.1 Sol / xhigh、REVISE: P2 5 件・P3 3 件）を反映した。
P2: 音声コマンドの操作番号と PCM 識別付きスナップショット・聞こえている位置・lock の分離、書き出しの開始条件と
再生停止・プレビュー worker の退役、歌詞の未確定編集の先行解決、保存完了後の再判定の位置と保留の所属、
上書き禁止を公開操作で保証。P3: PCM の識別を `Arc` と区別、AAC はチャンク単位の実体化、P4b-1 の lib 配置と
監査パス・診断テストの実行条件。段の順序はそのまま。P4b-3 の時点では終了の契約を完成扱いにせず、
P4b-4 と H3b を検収に残す。
2 回目（同日、REVISE: P2 1 件・P3 1 件）: 先行する Project 読込を意図の受理時に取り消す規則、歌詞確認の再開条件と
既存の取消・反映経路への接続を追記した。P4b-4 の handler テストに、開始済み読込の完了と保存完了の両順序、
歌詞の取消と読込完了が同じフレームになる順序、Ready / 待機中の読込がある状態での意図受理を加える。
3 回目（同日）: 指摘なし、ACCEPT。実装では取消フラグだけで済ませず、既に受信可能になった成功結果も採用しないこと。

#### P4b-1 実装記録（2026-10-14）

対象は `musicvideo` の `01f16465264f6c4b95ab862c3141db43cb59c723` 上の差分。
`tools/musicvideo_lab` を `crates/musicvideo-editor` に移動し、旧ホストは残していない。
`git mv` は worktree の git 管理領域に `index.lock` を作れず失敗したため、brief の許可に従い
通常の移動を使った。コミット時の rename 判定は設計担当に引き継ぐ。未追跡の `out.mp4` は変更していない。

- 共通ホストと既存テストは lib に集約。製品 bin `mimageviewer-musicvideo` は `run()` だけを呼び、
  引数解析はまだ持たない。開発 bin `musicvideo-dev` は `required-features = ["dev-tools"]` で、
  旧 main の全 10 種の診断・ベンチマーク引数を lib の `run_dev()` へ移した。
  通常 GUI とプレビュー計測 GUI はどちらも既存の `setup_egui` 登録を維持した。
- workspace member と lock、`musicvideo-core/tests/analysis_routes.rs` の監査パスを更新。
  Cargo metadata で member に含まれ、default-members に含まれないことを確認した。
  `scripts/test-full.ps1` は `cargo test --workspace --features pack-build-tools --no-fail-fast` を使うため、
  このクレートの lib の既存・追加テストも従来どおり対象になる。スクリプトと `.gitignore` に旧パス参照は無かった。
- 本体 lib の依存は `default-features = false`。本体には既定 feature の宣言がなく、通常 feature の
  lib と build.rs の vendor 検査を使う。`portable` や本体の診断 feature は有効にしていない。
- デコードは `mvexport::default_audio_stream_index` と
  `audio_decode::decode_audio_file_to_stereo_f32` に統一し、worker の取消を渡す。
  symphonia 依存と独自 resample を撤去。stereo f32 / 48kHz / PCM 由来の duration と
  `ResourceRevision` / H3a key の所有は維持した。App・chrome・export・playback・project・
  project_file・resource_blobs の移動済み 8 ファイルは、改行を除き HEAD のソースと一致した。
- 現行の定常音再測定コマンドを `musicvideo-dev` へ更新した。過去の brief・実施記録は変更していない。
  P4b-2〜4、P4c、H3b の実装やアプリの起動は行っていない。

検証ログは `target/p4b1/`。ビルド・検査・テストには `MSBUILDDISABLENODEREUSE=1` を設定した。
移動前の `before-list.log` / `before-test.log` と移動後の `editor-list.log` / `editor-test.log` を
名前単位で突き合わせ、既存 **84 件は同名のまま全件成功、欠落・改名 0**。
追加の decode テストは 3 件で、エディタは合計 **87 passed / 0 failed / 0 ignored**。
照合結果と移動ファイルの同一性は `comparison.json` に保存した。

追加の検査:

- `ffmpeg_decode_matches_export_default_track_and_stereo_resampling`: 追跡済み合成素材
  `multi-audio.m4a` / `multi.mkv` の既定が 2 番目の音声であること、書き出しの既定 stream と一致すること、
  44.1kHz→48kHz と 5.1→stereo を含め PCM が本体 FFmpeg の出力と完全一致することを確認。
  最初の音声トラックの PCM とは異なることも確認。
- `ffmpeg_decode_receives_worker_cancellation`: worker の取消済み token でデコードが成功せず終了することを確認。
- `shared_ffmpeg_decode_stops_when_cancelled_after_pcm_is_produced`: 全尺デコード API が委譲する共有 streaming
  デコーダの最初の PCM 通知（約 2 秒）で取消し、6 秒の素材を最後まで処理せず `cancelled` を返すことを確認。
  sleep や実時間に依存した競合は使っていない。

先頭 stream を固定して既定トラック選択を壊すと PCM 一致テストが失敗し、worker の token を
常に false の token に置換すると取消テストが失敗することを確認した（いずれも期待した exit 101、
`mutation-first-track.log` / `mutation-ignore-cancel.log` / `mutations.json`）。
各不正実装は元ソースの byte 列へ復元し、その後のエディタ全テストも 87 件成功した。

| コマンド（通常 feature、指定箇所以外は通常 test profile） | 結果 | ログ |
| --- | --- | --- |
| `cargo test -p musicvideo-editor` | 87 passed / 0 failed / 0 ignored。移動前 84 件の名前・成功結果と一致、追加 3 件 | `editor-test.log` / `comparison.json` |
| `cargo check -p musicvideo-editor --features dev-tools --bins` | exit 0。製品・開発の両 bin を検査 | `dev-check.log` |
| `cargo test -p musicvideo-core -p musicvideo-ui -p music-core` | core 117（unit 115＋経路 2）/ UI 122 / music-core 69（unit 49＋integration 20）、計 308 passed、0 failed / 0 ignored | `shared-test.log` |
| `cargo test -p mimageviewer --lib mvexport` | 8 passed / 0 failed / 0 ignored | `mvexport-test.log` |
| `cargo fmt` / `cargo fmt --check` / `python scripts/check_ui_glyphs.py` / `git diff --check` | 成功、危険 glyph 0 | `fmt.log` / `fmt-check.log` / `glyphs.log` |
| `cargo build --release -p musicvideo-editor --bin mimageviewer-musicvideo` | exit 0、**237.48 秒**。製品 exe **24,741,888 bytes（約 23.60 MiB）** | `release-build.log` / `release-build.json` |

release の測定は既存の target キャッシュを利用した今回の実ビルドで、cold build の推測値ではない。
成果物は `target/release/mimageviewer-musicvideo.exe`。`dumpbin /dependents` で FFmpeg の直接 import は
`avcodec-61.dll` / `avformat-61.dll` / `avutil-59.dll` / `swresample-5.dll` と確認した
（`release-imports.log`）。直接起動する場合は exe と同じディレクトリへ既存の FFmpeg DLL セットを配置する:

```powershell
Copy-Item .\vendor\ffmpeg\bin\*.dll .\target\release\
```

このセットには上の 4 DLL と `avfilter-10.dll` / `swscale-8.dll` を含む。
テスト用の FFmpeg DLL は既に `target/debug/deps/` に配置済みで、追加コピーは不要だった。
正本の P4b-1 との仕様上の相違はない。独立実装レビューは別担当に引き継ぐ。
コミット・本体アプリ・製品エディタ・旧ラボ GUI の起動は行っていない。

独立実装レビュー（`d4f1f4447`、GPT-6.1 Sol / xhigh）: **ACCEPT with conditions**。実装上の P1/P2 なし。
条件（P3）: I1 の検証計画に残っていた `cargo test -p musicvideo_lab` を `musicvideo-editor` に更新した（設計担当が修正）。


#### P4b-2 実装記録（2026-10-14）

対象は `musicvideo` の `4cdfd247e27600c043c0ebd2918728cdd6c013e1` 上の未コミット差分。
正本は本節の P4b-2 と D10「音声再生の所有」、実装 brief は
[brief-musicvideo-p4b2.md](brief-musicvideo-p4b2.md)。独立実装レビュー・実機確認は別担当へ引き継ぐ。

- `playback.rs` の `AudioPlayer` はコマンド送信口・操作番号・公開 snapshot・repaint 通知先だけを持つ。
  `musicvideo-audio` がデバイス照会・stream の生成／維持／drop と再生状態を所有する。
  UI に stream・callback の `PlayerState`・`JoinHandle` は渡さない。終了は FIFO の `Shutdown` を送るだけで、
  UI は待たない。WASAPI の内部 join を含む stream drop と再生 owner の PCM 解放は音声スレッド内。
- 音声の差し替えは PCM の `Arc` と `ResourceRevision` の対・位置を同じ `Replace` コマンドへ渡す。
  音声無しへの切替も同じコマンドで扱う。差し替えは停止・anchor 消去。Toggle／Pause／Seek／Shutdown にも
  UI が送信時に操作番号を付け、音声スレッドは channel の FIFO 順で適用する。
- snapshot は適用済み操作番号・PCM revision・聞こえている位置・再生状態・直近のエラーを公開する。
  callback の PCM lock と snapshot の lock は別で、publisher は両方を同時に保持しない。
  UI は最後に送った操作番号未満、または現在の PCM revision と違う位置を採用せず、自身の位置を保持する。
  再生 anchor の時刻／書込済み上限による時計と、H3a の `index_at` へ渡す位置の経路は維持した。
- cpal 0.16 の全 11 出力形式（f32／f64、i8／i16／I24／i32／i64、u8／u16／u32／u64）を
  同じ generic callback で充填する。整数は正規化範囲へ clamp して変換し、unsigned の無音は中点。
  出力 sample rate には既存の分数ステップ・線形補間を維持し、追加の resample 方針や PCM バッファは設けない。
- 最初の音声採用時のデバイス初期化を音声スレッドへ移した。初期化失敗・デバイス喪失はログと画面の通知にし、
  再生操作だけで再び開く。seek・pause・snapshot 公開では再試行しない。
  drop 済み stream の遅れて届いたエラーは、その stream の番号で排除する。
  停止中はコマンド／デバイスエラーの channel を待ち、再生中だけ 10ms 間隔で時計を公開する。
  操作適用とエラー公開では egui に repaint を通知する。
- `app.rs`／`app/chrome.rs` の音声採用・履歴復元・プロジェクト読込・seek・再生操作・終了を接続した。
  プロジェクト読込は manifest を採用してから、その revision を音声コマンドへ渡す。
  デバイスを使わない factory は初期化成功／失敗の両方を用意し、本番と同じスレッド loop・FIFO 適用・snapshot
  公開を通す。既存 A1b の H3a handler テストは実際の音声コマンド acknowledgement と、差し替え／Undo 後の
  revision・停止・位置・操作番号の前進も検証する。試験の待機は test-only で、製品 UI には入らない。

検証ログと集計は `target/p4b2/`。ビルド・check・テストには `MSBUILDDISABLENODEREUSE=1` を設定。
既存 **87 件を同名のまま全件維持**（H1／H2／H3a／I1／L1〜L3 を含む）、追加 **14 件**でエディタは
**101 passed / 0 failed / 0 ignored**。`comparison.json` に旧テストの欠落 0 と追加テストの名前を記録した。

追加テストの検査対象と、実際に失敗を検出した不正実装（いずれも cargo の exit 101、コンパイル失敗ではない）:

| 検査対象 | 不正実装 | 失敗確認ログ |
| --- | --- | --- |
| Undo 後の同一 PCM／連続 seek の古い位置（2 件） | 操作番号の比較を撤去 | `mutation-stale-operation.log` |
| 差し替え直後の古い位置 | 操作番号・PCM revision の比較を撤去 | `mutation-stale-replacement.log` |
| 別 revision の位置が H3a の時計に入らない | PCM revision の比較を撤去 | `mutation-wrong-revision.log` |
| Replace→Seek／Seek→Replace の FIFO | Seek の適用を捨てる | `mutation-discard-seek.log` |
| 差し替え後の停止・anchor 消去・revision 公開 | 差し替え後も playing にする | `mutation-replacement-playing.log` |
| Pause の聞こえている位置と Seek の操作番号 | Pause が書込カーソルを保持する | `mutation-pause-cursor.log` |
| f32／i16／u16 の PCM と無音の充填（3 件） | PCM を常に無音へ変換 | `mutation-silent-conversion.log` |
| PCM lock を保持しても UI 読取可能、その逆も可能 | UI snapshot が callback と同じ PCM lock を取る | `mutation-ui-waits-pcm.log` |
| 初期化失敗→次の再生だけで再試行、喪失通知、古い stream エラーの排除 | 再生時の初期化再試行を撤去 | `mutation-no-play-retry.log` |
| デバイス失敗後もホストの再生操作を受付・通知 | 画面へのエラー通知を撤去 | `mutation-no-host-notification.log` |
| `!Send` の代替 stream の生成／drop が音声スレッド、終了時 UI は待たず PCM を解放 | Shutdown を無視 | `mutation-no-shutdown.log` |

`mutations.json` と `comparison.json` で、**追加 14 件すべての失敗確認がある**ことを照合済み。
lock 分離テストは保持側を解放してから失敗を報告するため、不正実装でもデッドロックのまま残らない。
終了テストは stream の drop を channel で意図的に止め、UI の Drop がその解除前に戻ることを確かめる。
各不正実装はソースの byte 列へ復元し、その後に最終テストと release build を実行した。

| 指定コマンド | 結果 | ログ |
| --- | --- | --- |
| `cargo test -p musicvideo-editor` | 101 passed / 0 failed / 0 ignored | `editor-test.log` |
| `cargo check -p musicvideo-editor --features dev-tools --bins` | exit 0 | `dev-check.log` |
| `cargo test -p musicvideo-core -p musicvideo-ui` | core 117（unit 115＋経路 2）／UI 122、計 239 passed / 0 failed / 0 ignored | `shared-test.log` |
| `cargo fmt`／`cargo fmt --check` | exit 0 | `fmt.log`／`fmt-check.log` |
| `python scripts/check_ui_glyphs.py` | exit 0、危険 glyph 0 | `glyphs.log` |
| `cargo build --release -p musicvideo-editor --bin mimageviewer-musicvideo` | exit 0、48.33 秒、exe 24,823,808 bytes | `release-build.log`／`release-build.json` |

自動テストは合計 **340 passed / 0 failed / 0 ignored**。コマンド・時間・終了コードは `verification.json`。
成果物は `target/release/mimageviewer-musicvideo.exe`。このビルドの測定は既存 target キャッシュを使った実測で、
cold build の推測ではない。依存 DLL の配置条件は上の P4b-1 記録と同じ。
正本との仕様上の相違はない。P4b-3／4 は未実装、独立実装レビューと実機での音声確認は未実施。
本体アプリ・製品エディタ・lab GUI の起動、コミットは行っていない。未追跡の `out.mp4` は変更していない。

独立実装レビュー（GPT-6.1 Sol / xhigh）: 指摘なし、**ACCEPT**。実デバイスでの音の確認は未実施（エージェントは音を聞けない）。P4b の最後に利用者へ依頼する。

#### P4b-3 実装記録（2026-10-14）

対象は `musicvideo` の `1bf772d5d35d7c19bffe513362f420c8b8c98150` 上の未コミット差分。
正本は本節の P4b-3、D3 の H3a、D10、D11、D13 と
[実装 brief](brief-musicvideo-p4b3.md)。実装・非対話検証のみを行い、本体アプリ・エディタ GUI の起動と
コミットは行わない。P4b-4 の未保存・終了確認と H3b の製品プレビュー画素検証は対象外。

- 本体 lib の要求は `MusicVideoExportRequest<I>`。Project の複製、独立した画像 bundle、プレビューと同じ
  `Arc<ExportPcm>`、元音源の実サンプル長、PCM revision を受ける。エディタの `AudioData` は共通 PCM 型の
  alias とし、PCM の複製を必要としない。識別を `AnalysisKey<I>` にそのまま渡し、本体 lib に
  `musicvideo-ui` やエディタのプロジェクトファイル形式への依存を加えていない。
- 較正は worker 内で共通 `calibrate_modulators_view` を計算し直す。AAC 入力は出力尺の PCM view から
  最大 16,384 stereo フレームずつだけを実体化し、切り詰め・無音延長・PTS の旧入力列とのビット一致を検査する。
  全尺 PCM の resize・複製・素材ファイルの再読込をなくした。
- 進捗に反応範囲の計算／音声の符号化／フレームの書き出し／仕上げと frame 数を公開する。
  同一ディレクトリの `TempPath` を最後に `persist_noclobber`／`persist` で公開し、取消・公開失敗でも既存出力を
  先に削除しない。通常 CLI と開発用 CLI は上書きを承認しない。
- エディタは `ExportState` 一つで未開始／設定／保存先選択／実行を排他に所有する。既定は自動エンコーダ・
  12,000,000 bps、解像度・fps は Project の値を表示し、設定を永続化しない。保存ダイアログと選択直後の
  存在確認を同じ chooser worker が行い、その上書き承認の bool を再判定せず要求へ渡す。
  独立レビューで見つかった UI スレッドの `path.exists()` はこの worker 所有へ修正した。
- 開始条件は素材 Ready・音声あり・Project／LRC／素材の読込と保存なし・歌詞の行対応確認なし。
  開始時に P4b-2 の Pause を送り、preview revision を更新して実行中の worker を取消し、receiver は終端まで残す。
  編集・開く・保存・再生・seek・Undo を止め、新しいプレビュー／較正 worker を起動しない。
  中止要求や最終 frame 数でモーダルを解放せず、worker の終端結果で完了・失敗・中止を通知する。
  ウィンドウを閉じる要求と Drop の既存取消は変更しない。
- 当初の `mvexport_cli` にプロジェクト読込と鍵盤追加の入口がないことを報告して停止し、設計担当の判断で
  `musicvideo-dev --export-mp4` を追加した。本体 CLI は自前デコード → 共通入口だけの変更に留める。
  開発用コマンドは GUI の Project-open worker と同じ `load_project_bundle`（パス解決・画像・FFmpeg・resource lease）を
  通り、GUI worker と同じ `snapshot`／`export_snapshot` 関数を使う。

検証記録は `target/p4b3-verification/`。初回の指定自動テストは editor 111、core 115＋経路 2、UI 122、
mvexport 15、合計 **365 passed / 0 failed / 0 ignored**。追加は editor 10 と mvexport 8 の **18 件**。
既存の Undo handler にも、復元した PCM と revision を共通 snapshot に渡す検査を加えた。
`comparison.json` で既存 editor **101 件の欠落 0** と追加 18 件すべての失敗確認を照合した。

| 検査対象 | 意図的に壊した実装 | 検出ログ（同ディレクトリ） |
| --- | --- | --- |
| PCM view の入力列・ビット・PTS、RGBA と AAC、復元 PCM | チャンクの実サンプルをコピーせず無音にする | `library-mutation-chunk-zero.log`（3 件失敗） |
| 公開直前の競合 | `persist_noclobber` を置換公開へ変える | `library-mutation-clobber-race.log` |
| 置換・取消・失敗で原本を守る | 一時出力作成時に原本を先に削除する | `library-mutation-delete-original.log`（3 件失敗） |
| 取消と最終 frame 後の取消 | cancel の確認を無視する | `library-mutation-ignore-cancel.log`（2 件失敗） |
| 不正な PCM の拒否 | sample rate 0 の拒否を除く | `library-mutation-invalid-pcm.log` |
| プレビューと同じ較正 | 較正結果を捨てる | `library-mutation-discard-calibration.log`（2 件失敗） |
| revision が解析の key に届く | 毎フレーム timeline を捨てる | `library-mutation-discard-timeline.log` |
| 全画像の snapshot と dev の共通読込 | snapshot の画像をすべて落とす | `editor-mutation-snapshot-drop-images.log`（2 件失敗） |
| 開発用コマンドの設定 | frame-hash の引数を無視する | `editor-mutation-parser-drop-hash.log` |
| 保存中の開始条件 | 保存中の拒否を除く | `editor-mutation-admit-during-save.log` |
| 歌詞の行対応確認 | 確認待ちの拒否を除く | `editor-mutation-admit-lyrics-confirmation.log` |
| 再生の停止 | Pause を送らない | `editor-mutation-skip-pause.log` |
| モーダルの操作受付 | host action の gate を外す | `editor-mutation-modal-input-dispatch.log` |
| 保存先選択中の編集禁止 | editor の input gate を外す | `editor-mutation-chooser-undo-input.log` |
| worker 終端で完了 | 最後の frame 数だけで完了にする | `editor-mutation-finish-at-last-frame.log` |
| 全段階の進捗表示 | 段階名を常に仕上げにする | `editor-mutation-wrong-stage-label.log` |

16 種類の変異はすべて **コンパイル後のテスト失敗（cargo exit 101）** を検出し、毎回ソースを復元した。
本体・エディタそれぞれの runner／集計は `library-mutations.ps1`／`library-mutation-summary.json` と
`editor-mutations.py`／`editor-mutation-summary.json`。モーダル gate の不正実装でも、test-only の dispatch assertion が
native file dialog に届く前に失敗させるため、検証中に GUI は起動しない。

| 最終コマンド（`MSBUILDDISABLENODEREUSE=1`） | 結果 | ログ |
| --- | --- | --- |
| `cargo test -p musicvideo-editor` | 111 passed / 0 failed / 0 ignored | `final-editor-tests.log` |
| `cargo check -p musicvideo-editor --features dev-tools --bins` | exit 0 | `final-editor-check.log` |
| `cargo test -p musicvideo-core -p musicvideo-ui` | core 115＋経路 2、UI 122、計 239 passed | `final-core-ui-tests.log` |
| `cargo test -p mimageviewer --lib mvexport` | 15 passed / 0 failed / 0 ignored | `final-mvexport-tests.log` |
| `cargo fmt`／`cargo fmt --check` | 実行済み／exit 0 | `final-fmt-check.log` |
| `python scripts/check_ui_glyphs.py` | exit 0、危険 glyph 0 | `final-glyphs.log` |
| `cargo build --release -p musicvideo-editor --bin mimageviewer-musicvideo` | exit 0、164.16 秒、25,213,440 bytes | `release-editor.log` |
| `cargo build --release --features dev-tools --bin mvexport_cli` | exit 0、175.27 秒、5,289,984 bytes | `release-mvexport-cli.log`／`.json` |
| `cargo build --release -p musicvideo-editor --features dev-tools --bin musicvideo-dev` | exit 0、50.56 秒、25,407,488 bytes | `release-musicvideo-dev.log`／`.json` |

最終の指定テストは **365 passed / 0 failed / 0 ignored**。時間・終了コードは `final-gates.json` と
各 build の JSON。brief の `cargo build --release --bin mvexport_cli` は既存の
`required-features = ["dev-tools"]` のため exit 101 となり、上表のように feature 指定を補って成功した。
本体 CLI の feature gate や仕様は変更していない。FFmpeg DLL は `vendor/ffmpeg/bin/` の既存 6 DLL を
`target/release/` へコピーし、成果物の GUI は起動していない。

**5 分 1080p60 実測（参考値）。** `target/p4b3-verification/` に純生成の 3000×1687 背景と 300 秒 stereo
48kHz PCM WAV、`edited-300s.mivmv.json` を生成した。Image／Spectrum／PitchKeyboard／Text／TimeText／ProgressBar の
6 層を表示し、画像ズームの 3 キーフレームと Auto 較正の BandOnset → SpectrumRadius の変調を含む。
生成手順は `prepare-edited-fixture.py`、入力と実行 exe の SHA-256 は `export-300s.json`。

```powershell
& .\target\release\musicvideo-dev.exe --export-mp4 .\target\p4b3-verification\edited-300s.mivmv.json .\target\p4b3-verification\edited-300s.mp4 --encoder nvenc --video-bitrate 12000000 --frame-hash
```

開始前に cargo／rustc はなかったが、**Codex プロセス 4 件が残っていた**（`benchmark-process-preflight.json`）。
他プロセスを停止せず測定したため静穏条件を満たした正式値として扱わない。GUI・エディタのプレビューは起動していない。

| 区間／指標 | 実測 |
| --- | --- |
| 完走 | exit 0、18,000 frames、300 秒、1920×1080／60fps |
| エンコーダ／ビットレート指定 | `h264_nvenc`／12,000,000 bps |
| 書き出し worker の wall | **184.441 秒、97.592 fps、実時間比 1.6265x**（RGBA hash 計算を含む） |
| Project・素材読込／デコード／波形・lease 作成 | 0.328 秒 |
| 読込＋書き出し（コマンド自身の区間） | 184.811 秒 |
| 全尺再デコード確認を含むコマンド全体 | 202.837 秒 |
| 較正 | 2.526 秒 |
| AAC | 3.846 秒 |
| 解析・scene 評価・RGBA 合成 | 合計 114.191 秒、平均 6.344 ms／p95 7.201 ms |
| RGBA → YUV 変換 | 合計 15.264 秒、平均 0.848 ms |
| H.264 符号化 | 合計 7.528 秒、平均 0.418 ms |
| mux | 0.363 秒 |
| faststart | 0.052 秒 |
| 公開 | 0.000485 秒 |
| RGBA SHA-256 | 40.496 秒 |
| その他（worker の wall 内） | 0.175 秒 |
| MP4 サイズ | 145,395,326 bytes |

全尺を共通 `verify_exported_mp4` で再デコードし、映像 18,000 frames／300.000 秒、音声 14,400,512 samples/ch／
300.010667 秒、差 **10.667 ms（60fps の 1 frame 未満）**を確認した。`ffprobe` でも H.264 **High**／yuv420p／
bt709／60/1、AAC **LC**／48kHz／stereo を確認（`export-ffprobe.json`）。`export-20s.png` を CLI で取り出して目視し、
背景・スペクトラム・鍵盤・文字・時刻・進捗の描画を確認した。これは H3b の製品プレビュー画素一致の検収ではない。
RGBA SHA-256 は `1cc527f3437523a7632818bb319910596bf192898f8243edf5c1687e8eca9fd1`。
生ログ `export-300s.log`、集計 `measurement-summary.json`、実行条件／入力 fingerprint は `export-300s.json`。

正本との相違は、設計担当が追加した開発用書き出し入口と、既存 feature gate に合わせたビルドコマンドの補正のみ。
P4b-4・H3b・実機 GUI 検収は未実施。コミットせず、既存の未追跡 `out.mp4` も変更していない。

実装担当の作業内での確認（UI の存在確認 1 件を修正）。これは設計担当が依頼する正式な独立レビューではない
（2026-10-14 設計担当注記）。正式な独立レビューは別に行う。
5 分の実測は背景に別の処理（利用者の Codex アプリ等）が動いた状態で、参考値とする（P2c の 3.2 倍との比較には使わない）。

独立実装レビュー 1 回目（`94f1f0ccf`、GPT-6.1 Sol / xhigh）: **REVISE**（P2 1 件・P3 1 件）。
- P2「上書きを承認していない出力を置き換え得る」: 保存ダイアログで存在しない名前を選んだ後、ダイアログが閉じてから
  存在確認（同じ worker の直後の `path.exists()`）までの間に別のプロセスが同じ名前を作ると、`replace_existing=true` になる。
  **設計担当の判断で割り切る**（CLAUDE.md「まれな失敗は既定で割り切る」）。rfd は OS ダイアログの上書き確認の結果を返さず、
  独自の確認を足すと通常の上書きで確認が 2 回出る（通常の使い方の退行）。起きるのは、利用者が名前を選んだ直後の
  ごく短い間に、別のエディタ等が同じ名前で書き出しを完了した場合だけ。判断をコードのコメントに残した。
  2 回目（同日）: レビュー担当は「別プロセスが作った MP4 を無通知で失うのは利用者データの損失で、CLAUDE.md の相談対象」と
  判断（REVISE）。設計担当が利用者に相談し、**利用者が許容を選んだ（2026-10-14）**。失われ得るもの（名前を選んだ直後に
  別のエディタ等が同じ名前で書き出した MP4）と、防ぐ費用（通常の上書きで確認が 2 回出る）を示したうえでの判断。
  コードのコメントも「利用者が許容」に改めた。これで P4b-3 の指摘は閉じる。
- P3: §7 の P4b 行で P4b-3 が独立レビュー済みに読める誤記を直した。

#### P4b-4 実装記録（2026-10-14）

対象: `musicvideo` の `7413b649d0d224b326b41cffa92e369d9fd0240b` からの未コミット差分。
正本は本節の P4b-4 と D10「未保存の編集と終了」、brief は
[brief-musicvideo-p4b4.md](brief-musicvideo-p4b4.md)。独立実装レビューと実機 GUI 検収は別担当に残す。
アプリ・製品エディタ・lab GUI の起動とコミットは行わず、既存の未追跡 `out.mp4` は変更していない。

実装:

- `app/document_intent.rs` の `Open(PathBuf)` / `New` / `Close` を session 付きの単一の保留意図とした。
  未保存確認・保存待ち・書き出しの終了確認／終端待ちは同じ owner の phase。承認済み Close は終端状態とし、
  自分で送った Close を CancelClose しない。確認中の close 要求は現在の意図だけを Close に置き換える。
- 意図の受理時に H1 の Loading（待機中の次の path を含む）／Ready を取り外し、取消と H2 の遅延解放へ渡す。
  receiver も取り外すため、取消より先に送信された成功結果も採用しない。保留中は通常の読込入口も拒否し、
  判定を通った Open だけが再開する。UI は worker の終了を待たない。
- toolbar に「新規」を追加。既存の Project の既定値を使い、`apply_loaded_project` と同じ素材・音声・履歴の
  切り替え経路を通してから、保存先を持たない新しい session とする。P4c の最近一覧・起動引数は変更していない。
- 全レイヤーの歌詞 buffer を project 順に確認するモーダルを `musicvideo-ui` に追加。
  既存 `draw_lyrics_resolution` と同じ resolve / cancel handler を使い、取消は TextEdit rollback を復元する。
  反映は既存 transaction / 履歴 / `EditorResponse` / `note_edit` 経由。NeedsResolution は確認を続け、
  Unchanged でも全レイヤーの pending の消失をもって完了とする。別の歌詞確認 owner は追加していない。
- 通常保存と意図の「保存」は同じ入口へ合流。保存先選択は worker に置き、既存の保存 pending を
  Choosing / Writing の型へまとめた（互いに排他的な Option は追加しない）。取消・失敗は意図を捨てる。
  `complete_save` は保存基準の更新にとどめ、保存成功を clean と解釈しない。
- `process_editor_frame` にプレビュー入力も含め、editor／toolbar／preview の入力と
  `poll_projects` / `poll_lrc` / 保存先選択／素材同期の後に共通の未保存判定を 1 回行う。
  保存待ちもモーダルを維持し、保存中の LRC・素材採用で再び dirty なら再確認する。
- 書き出し中の Close は中止確認を出す。拒否なら worker を継続し、承認なら cancel を送り、
  worker の終端結果による export owner の解放まで待つ。取消フラグだけで終了扱いにしない。

組み合わせの簡素化は受け入れ済み設計どおり、単一の保留意図・モーダル・先行 Project 読込の取消を採用した。
まれな保存失敗はログと通知だけ。再試行・復旧機構は追加していない。D10／P4b-4 の仕様との相違はない。

初回実装の追加テストと失敗確認:

- editor に handler テスト **29 件**。Open / New / Close × 保存成功／失敗／保存先取消／破棄／取消の 15 件、
  clean の直接実行、同一フレームの保存完了と LRC 採用、保存中の素材採用、先行 Project 読込の既受信／待機中／Ready
  の退役（送信が意図受理の前／後になる順序）、保存中の Close と置き換え、session 失効、書き出し中止の承認／拒否と終端待ち、
  CancelClose／承認済み Close、新規の素材境界、保存中の入力 gate、読込結果を意図受理前に送った条件での
  未選択レイヤーの歌詞取消と cursor rollback、
  実際の保存 worker と保存先選択結果の合流、toolbar 新規の確認を検査した。
- UI に handler テスト **2 件**。同じ編集内の 2 回の NeedsResolution を経て 1 transaction で反映すること、
  resolve が Unchanged でも pending を解消して履歴を増やさないことを検査した。
- **31 件すべて**が次の変異のいずれかで失敗することを確認した（コンパイル失敗は検出と数えない）。
  各実行の finally で変異を復元。各テスト名との対応とログは `target/p4b4-mutations/report.json` と
  同ディレクトリの `M01`〜`M13` のログに記録した。

| 変異 | 検出した追加テスト数 |
| --- | ---: |
| M01 フレーム末尾の共通判定を呼ばない | 27 |
| M02 モーダル中の通常操作を dispatch する | 1 |
| M03 意図のキャンセルを無視する | 4 |
| M04 保存失敗後も意図を残す | 3 |
| M05 保存先選択の取消後も意図を残す | 3 |
| M06 Project 読込 receiver を取り外さない | 2 |
| M07 保存完了を待たず判定する | 3 |
| M08 書き出し worker の終端を待たず判定する | 1 |
| M09 承認済み Close も CancelClose する | 1 |
| M10 文書 session の所属を確認しない | 1 |
| M11 共通判定を poll_lrc より前へ移す | 1 |
| M12 歌詞の resolve を実行しない | 2 |
| M13 歌詞取消の TextEdit rollback を復元しない | 1 |

非対話 gate（全 Cargo コマンドに `MSBUILDDISABLENODEREUSE=1`、変異を復元した差分で実行）:

| コマンド | 結果 |
| --- | --- |
| `cargo test -p musicvideo-editor` | **140 passed / 0 failed**（既存 111 + 追加 29） |
| `cargo check -p musicvideo-editor --features dev-tools --bins` | exit 0 |
| `cargo test -p musicvideo-core -p musicvideo-ui` | core **115** + 経路監査 **2** + UI **124 passed / 0 failed**（UI 既存 122 + 追加 2） |
| `cargo test -p mimageviewer --lib mvexport` | **15 passed / 0 failed** |
| `cargo fmt --check` | exit 0（`cargo fmt` 実行済み） |
| `python scripts/check_ui_glyphs.py` | exit 0、危険 glyph **0** |
| `cargo build --release -p musicvideo-editor --bin mimageviewer-musicvideo` | exit 0、`target/release/mimageviewer-musicvideo.exe` 生成（起動なし） |

指定テストの合計は **396 passed / 0 failed / 0 ignored**。既存 H1 / H2 / H3a / I1 / L1〜L3 / P4b-1〜3 を維持した。
ログは `target/p4b4-editor.log`、`target/p4b4-{check,core-ui,mvexport,fmt,glyphs,release}.log`、
コマンド／exit code／件数の集計は `target/p4b4-gates.json`。GUI 入力／描画の検査は headless egui のみで、
保存待ちの確認表示も shape と handler で検査した。実機 close／OS 保存先ダイアログ・音声・H3b は未検証。

**独立レビュー 1 回目 REVISE と対応（2026-10-14）**

レビュー対象: `ba360500a`、GPT-6.1 Sol / xhigh、**REVISE（P2 1 件・P3 1 件）**。
修正は `9543ebd02` を基点とする未コミット差分。以下を対応し、再レビューに残す。

- **P2: LRC 確認が歌詞／意図の確認を遮り、操作が行き詰まる。**
  LRC 読込中に歌詞の未確定編集を作り、close 要求で歌詞確認を表示した後に LRC 完了を投入した。
  元の描画経路へ戻す変異では、歌詞の「変更を取り消す」を実クリックしても pending が残り、
  `p4b4_lrc_ready_during_lyrics_confirmation_returns_after_intent_cancel` が失敗（0 passed / 1 failed、exit 101）。
  `draw_load_choice` の描画入口を意図の保留と一致させ、保留中は既存の読込確認を表示しない。
  Ready payload は取り外さず保持する。通常操作の dispatch gate は維持した。
  修正後は、実クリックで歌詞取消 → 未保存意図の「キャンセル」 → 再表示された LRC 確認まで進み、
  「反映」「現在の歌詞を保持」の両方を検査する。未選択の歌詞レイヤーでも同じ経路を通る。
- **P3: 保存完了との両順序と歌詞取消フレームへの完了投入が未検査。**
  初回の `load_first` は読込結果の送信と意図受理の前後しか変えておらず、保存結果は常に後から送っていた。
  初回の歌詞取消テストも、意図受理前の送信から複数フレーム後に取り消す条件だった。
  上の初回検証範囲をその実際の条件に訂正した。
  既受信／待機中／Ready の退役テストを `…retired_on_intent` へ改名して残し、次の両順序を別々のテストに追加した。
  - `p4b4_project_completion_before_save_is_retired`: 意図受理 → 旧読込結果の送信 → フレーム処理
    （保存 pending と元 session を確認）→ 保存結果の送信 → フレーム処理。
  - `p4b4_project_completion_after_save_is_retired`: 意図受理 → 保存結果の送信 → フレーム処理
    （承認済み Close を確認）→ 旧読込結果の送信 → フレーム処理。
  `p4b4_lyrics_cancel_frame_rejects_late_project_completion` は、歌詞取消ボタンを押すフレームでは pending を維持し、
  旧読込結果を送ってからボタンを離すフレームを処理する。この同じフレームで歌詞取消と `poll_projects` を通し、
  元の歌詞・cursor rollback・意図の再確認・Project 読込非採用を検査する。
  Project receiver の退役を省く変異で、両順序の 2 件はともに失敗（0 passed / 2 failed、exit 101）、
  歌詞取消フレームの 1 件も失敗（0 passed / 1 failed、exit 101）することを確認した。
  退役後は cleanup が receiver を解放済みなら送信失敗になり得るため、成功送信を前提にせず、
  送信を試みた後の実フレームで旧結果が採用されないことを検査する。

今回は editor の新規テスト **3 件**と既存テストの完了順序補強。P4b-4 の対象テストは editor **32 件**、
UI **2 件**。今回の 3 件と補強した歌詞取消の 1 件は、上記の変異ですべて失敗を確認し、変異を復元した。
失敗のログと対応するテスト名は `target/p4b4-review1/{P2-original-modal-routing,P3-project-and-save-both-orders,P3-completion-on-lyrics-cancel-frame}.log`
および `mutations.json` に記録した。

修正後の指定 gate（全コマンド exit 0、全 Cargo コマンドで `MSBUILDDISABLENODEREUSE=1`）:

| コマンド | 結果 |
| --- | --- |
| `cargo test -p musicvideo-editor` | **143 passed / 0 failed**（初回 140 + 今回 3） |
| `cargo check -p musicvideo-editor --features dev-tools --bins` | 成功 |
| `cargo test -p musicvideo-core -p musicvideo-ui` | core **115** + 経路監査 **2** + UI **124 passed / 0 failed** |
| `cargo test -p mimageviewer --lib mvexport` | **15 passed / 0 failed** |
| `cargo fmt --check` | 成功（`cargo fmt` 実行済み） |
| `python scripts/check_ui_glyphs.py` | 成功、危険 glyph **0** |
| `cargo build --release -p musicvideo-editor --bin mimageviewer-musicvideo` | 成功、release exe 更新（起動なし） |

合計 **399 passed / 0 failed / 0 ignored**。集計とログは `target/p4b4-review1/gates.json` と同ディレクトリの
`{editor,check,core-ui,mvexport,fmt,glyphs,release}.log`。初回ログは上書きせず保持した。
正本の仕様変更はない。GUI の起動・コミットは行わず、`out.mp4` を変更していない。
実機 GUI／音声／H3b と修正後の独立レビューは未実施。

独立実装レビュー 2 回目（`25f0d4c34`、GPT-6.1 Sol / xhigh）: 2 件とも解消、追加指摘なし、**ACCEPT**。

---

**P4c 実装記録（2026-10-14、未コミット差分）**

正本は D10 と §7 P4c、実装 brief は `docs/brief-musicvideo-p4c.md`。
対象は `musicvideo` worktree、基点 HEAD `922727457330e82bf941b5c645dfcf599b1c164b`。
以下は実装者自身によるコード確認・非対話検証の記録であり、独立レビューではない。
**初回実装・指定自動 gate 完了（当時は独立実装レビュー／実機検収未実施）。後の独立レビュー1回目 REVISE と対応は下記。**

- 製品入口は `args_os` を純粋な `parse_args(args, cwd)` に渡す。3 引数の値・重複・組み合わせ・
  未知引数を検査し、相対パスを cwd 基準の絶対パスへ解決する。解析エラー時は読込を開始せず、
  空のプロジェクトとウィンドウ内の理由を表示する。存在確認は既存の Project／音声読込 worker の
  ファイル読取で行う。起動 Project は利用者入力前に既存 `start_project_load` へ渡す
  （新規 session は保存先を持たず dirty なので、起動引数に未保存確認を挟まない）。起動後の編集は
  H1 の generation／Ready 採用規則で保護する。音声は既存 `start_audio_load` に渡す。
- 最近一覧は `<data_dir>/musicvideo/recent-projects.json`。`apply_loaded_project` の採用境界と
  保存 worker 成功の checkpoint だけから worker へ配送する。新規の空 path、開始・取消・失敗・
  破棄された読込は登録しない。読込／保存の使用 path も絶対パスへ解決し、上限10・同 path の先頭移動・
  固有の一時ファイルからの置換を worker が所有する。古い項目は落とす。同時更新は最後の成功書込が残る。
- 一覧は既存のステータス「詳細」メニューに置く。「開く」は `DocumentIntent::Open(path)` へ合流し、
  専用の未保存判定を作らない。存在確認も既存 Project 読込 worker に合流する。無い項目の理由を表示し、
  「一覧から外す」を worker へ渡せる。data-dir 無しではメニューを無効にし、一覧を読まず・書かない。
  一覧の読取不能／壊れた JSON は空としてログに残す。書込失敗は通知とログのみで、プロジェクトの
  保存成功・保存基準・使用 path を覆さない。
- `musicvideo-persistence` が最近一覧と診断の単一 I/O owner。UI は channel 配送／try_recv と
  表示だけを行い、一覧の read／write／stat、ログ・stderr の追記で待たない。
  `<data_dir>/logs/musicvideo.log` は追記、起動時に **4 MiB 超**だけ `.1` へ1世代退避する。
  自前 panic hook は同じ worker へ配送し `musicvideo-panic.log` に追記する。
  最終ログ drain は `run_native` が終了した後に外側 guard が行う。data-dir 無しでは stderr のみ。
  worker 切断・書込失敗は画面へ理由を通知する。再試行・復旧・IPC は追加していない。
- 本体 lib の公開入口 `pin_companion_runtime()` は既存の `pin_running_version` に委譲する。
  判定・shared lease は既存と同じで、**本体 crate の `CARGO_PKG_VERSION`** を使う。
  起動最初に取得し、外側の Arc owner が GUI 終了後の診断 drain まで保持する。
  runtime 外の開発／portable 配置は取得しない。失敗は型付き Blocked として空の画面で理由と
  「mIV から開き直す」案内を表示し、入力・読込・プレビュー worker を開始しない。
- mIV の設定／cache／logger 初期化入口 `mimageviewer::run()`、`Settings::load`、`data_dir::get`
  を editor から呼ばない。今回の公開 lease 入口もそれらを初期化せず、exe に属する runtime lock
  だけを操作する。既存の音声経路は `audio_decode`／`mvexport`、素材履歴は匿名一時 blob のまま。
  editor 永続化の保存先は明示 data-dir の上記3ファイルに限定する。
  一時 data-dir に置いた mIV `mimageviewer.log`／`panic.log`、settings.db／cache DB の sentinel が
  不変であることも検査した。`%APPDATA%\mimageviewer` に触れていない。

追加テストは **editor 18件（引数2・host handler8・worker8）＋本体 lease3件＝21件**。
起動の誤りと案内の headless 描画、既存読込入口、一覧の採用／破棄／失敗／取消・保存成功と
一覧書込失敗、共通 Open 意図、worker 切断、絶対パス／重複／上限／削除、壊れた一覧、
data-dir 無し、原子的な同時書込、4 MiB 境界・追記・panic hook の実出力を検査した。
panic hook は子の**テスト harness**で登録し、他テストの global hook を変えない。
本体の lease3件は loose／portable／異なる版、shared lease の保持と解放、排他 lease 中の失敗を検査する。

**21件すべて**が次の9変異のいずれかで失敗した。コンパイル失敗は検出に数えず、各変異を finally で復元。
各テスト名の対応は `target/p4c-validation/mutations.json`、ログは同ディレクトリの `mutation-*.log`。
行の検出数は重複を含み、和を追加テスト総数として扱わない。

| 変異 | 壊した実装 | 検出した追加テスト数 |
| --- | --- | ---: |
| `args` | 引数解析が結果を返さない | 3 |
| `startup_and_render` | 起動読込／エラー本文／編集 gate を壊す | 2 |
| `recent_admission` | 読込採用・保存成功から一覧更新を外す | 2 |
| `intent_and_diagnostics` | 一覧の Open 意図と状態ログの配送を外す | 2 |
| `recent_io` | 重複除去・壊れた一覧のログ・一覧保存を外す | 5 |
| `log_io` | data-dir 無しのファイルログ・退避・panic 配送・失敗通知を壊す | 4 |
| `runtime_core_version` | 本体の版番号を editor の 0.1.0 に取り違える | 3 |
| `runtime_layout` | runtime／版ディレクトリの判定を外す | 1 |
| `disabled_recent` | 最近一覧の無効状態でも更新する | 1 |

初回検証で最近一覧ボタンの追加による既存歌詞入力レイアウトの回帰を検出し、既存「詳細」メニュー内に
移して既存テストを維持した。取消後の receiver は H2 の解放 worker が所有するため、送信失敗を期待する
ハーネスを、取消前に完了を送って採用されないことを検査する形に修正した。
Windows の原子的置換を40回連続競合させると native のアクセス拒否が返り得ることを確認した。
一時 writer を閉じてから置換し、失敗は D10 の通知・ログ規則で扱う。ストレステストは全置換の成功を
要求せず、成功書込が存在すること、完全な一覧だけが残ること、最後の成功書込と一時ファイルの解放を検査する。
まれな書込失敗用の retry／復旧機構は作らない。正本の仕様との相違はない。

復元後の指定 gate（全 Cargo コマンドで `MSBUILDDISABLENODEREUSE=1`）:

| コマンド | 結果 |
| --- | --- |
| `cargo test -p musicvideo-editor` | **171 passed / 0 failed**（既存153＋追加18） |
| `cargo check -p musicvideo-editor --features dev-tools --bins` | exit 0 |
| `cargo test -p musicvideo-core -p musicvideo-ui` | core **115**＋経路監査 **2**＋UI **124 passed / 0 failed** |
| `cargo test -p mimageviewer --lib runtime_` | **98 passed / 0 failed / 1 ignored**（既存 ignored を維持） |
| `cargo fmt --check` | exit 0、`cargo fmt` 実行済み |
| `python scripts/check_ui_glyphs.py` | exit 0、危険 glyph **0** |
| `cargo build --release -p musicvideo-editor --bin mimageviewer-musicvideo` | exit 0、製品 exe 生成（起動なし） |

指定テストの合計は **510 passed / 0 failed / 1 ignored**。
既存 H1／H2／H3／I1／L1〜L3／P4b-1〜4 のテストをすべて維持した。
command／環境／exit／件数／所要時間とソース SHA-256 は `target/p4c-validation/gates.json`、
通常ログは同ディレクトリの `editor,check,core-ui,runtime,fmt,glyphs,release.log`。
製品成果物は `target/release/mimageviewer-musicvideo.exe`、25,366,016 bytes、
SHA-256 `4be14e7a899807ca6dac6f13abc11dd607a6d54b9cf7e47ba9a588448e010ac7`。
アプリ／エディタ GUI の起動、実機入力、コミットは行っていない。既存 untracked `out.mp4` は未変更。
P4d／P4e／P4f は範囲外のまま。

変更ファイル:

- `crates/musicvideo-editor/src/lib.rs`、`app.rs`、`project_file.rs`。
- `crates/musicvideo-editor/src/app/chrome.rs`、`document_intent.rs`、`startup.rs`、`startup_tests.rs`。
- `crates/musicvideo-editor/src/startup.rs`、`persistence.rs`、`persistence/tests.rs`。
- `src/lib.rs`、`src/runtime_cleanup.rs`、`docs/music-video-export-plan.md`。

**独立レビュー 1 回目 REVISE と対応（2026-10-15）**

利用者提供の GPT-6.1 Sol / xhigh の独立実装レビュー: **REVISE（P2 1件・P3 1件）**。
対象は `922727457` 以降の P4c（実装 commit `9d11390ff`）。独立レビュー担当は読み取り専用で、
初回のソース／release hash・サイズ、510 passed／0 failed／1 ignored、追加21件の変異失敗を照合し、
Cargo 再実行・GUI 起動・ファイル変更は行っていないとの報告。
今回の修正は HEAD `b6e03127ca50dd4fbe084f780ed0a57b3d3b3c5d` を基点とする未コミット差分。
以下の洗い出し・修正・再実行は実装者の作業であり、独立の再レビューではない。
**指摘2件を修正し指定 gate は通過。独立再レビュー／実機検収は未実施。**

**P2: 既存 GUI 経路の直接出力を取りこぼしていた。** 今回発生した新たな回帰ではなく、
P4c の「stderr の追記で待たない」要件への対応漏れとして修正した。
指摘の2か所に限定せず `eprintln!`／`println!`／`print!`／`dbg!`／stdout・stderr の直接利用、
fs／File／Write／sync／persist と画像保存 API を editor 全体で検索し、呼び出し元と実行 thread を確認した。
共有 `musicvideo-ui`／`musicvideo-core` の GUI 呼び出し先にも同期出力・ファイル書込がないことを確認。
検索の素データは `target/p4c-review1/io-inventory.txt`。

UI／GUI 起動・終了の main thread にあった直接出力の全一覧（path は `crates/musicvideo-editor/src/` 基準）:

| 箇所 | 旧処理 | 修正後の配送 |
| --- | --- | --- |
| `app/document_intent.rs::project_save_failed` | 保存失敗の stderr | `log_status` → `Command::Log` |
| `app/mp4.rs::choose_mp4_output` | 保存先 dialog worker の開始失敗の stderr | `log_status` → `Command::Log` |
| `app/mp4.rs::poll_mp4_choice` | 保存先 dialog 切断の stderr | `log_status` → `Command::Log` |
| `app/mp4.rs::begin_mp4_export` | 書出 worker の開始失敗の stderr | `log_status` → `Command::Log` |
| `app/mp4.rs::complete_mp4_export` | MP4 書出失敗の stderr | 状態を採用後 `log_status` → `Command::Log` |
| `playback.rs::AudioPlayer::send` | 音声 command channel 切断の stderr（Shutdown／Drop からも到達） | host の診断 sender → `Command::Log`。既存 error snapshot の通知を維持 |
| `app.rs::eframe::App::update` の preview benchmark 終了 | GUI callback の計測 stdout | `finish_preview_benchmark` → `Command::Print` |
| `lib.rs::run_gui` の assets 作成失敗 | GUI 起動準備の stderr | `create_gui_assets` → 診断 sender → `Command::Log` |
| `lib.rs::run` の `run_gui` 失敗 | GUI 終了／起動失敗の stderr | `run_gui` の writer guard が生きている間に `report_gui_result` → `Command::Log` |

上記 **9か所**に UI の同期出力を残していない。音声 player の診断 sender は host が生成／接続時に
渡す同じ persistence writer への参照であり、別 logger／復旧 owner を作らない。
GUI benchmark にも data-dir 無しの同 worker を保持し、CLI が読む stdout の形式と終了時 drain を保つ。
writer が存在しない／切断した場合も UI から stderr へ同期 fallback しない。
host の既存通知・音声 error snapshot を維持する。stdout／stderr は worker だけが追記する。

同期ファイル書込の全経路の確認（GUI handler から直接書き込む箇所は検出しなかった）:

| 書込処理 | 実行 owner／呼び出し経路 |
| --- | --- |
| Project JSON: 一時 file、write／sync／persist（`project_file.rs::save_project`） | `app.rs::start_project_save` が生成する `musicvideo-project-save` worker |
| LRC: `std::fs::write` | `app.rs::start_lrc_save` の `musicvideo-lrc-save` worker |
| PNG 連番: output dir 作成、画像保存（`export.rs::export_png_sequence`） | `app.rs::start_export` の `musicvideo-png-export` worker |
| MP4: encode／一時出力／公開（`mp4.rs::export_snapshot` → 本体 `mvexport`） | `app/mp4.rs::begin_mp4_export` の `musicvideo-mp4-export` worker |
| 素材履歴の匿名 blob: File 作成と画像／PCM の write・flush（`resource_blobs.rs::store_image/store_audio`） | 音声／画像／Project 読込 worker だけが呼ぶ。file の最終解放も既存 cleanup worker |
| 最近一覧: 一時 file／JSON／sync／persist、log／panic log: create／append、rotation: remove／rename | `musicvideo-persistence` worker。今回追加の stdout も同 worker |
| runtime shared lease の lock file 作成 | `run_gui` の最初、native UI を作る前の起動処理。既存 core と同じ規則・取得失敗の拒否・保持期間を変更していない。実行中 UI の handler から呼ばない |
| CLI の sample／lyrics／zoom PNG・診断 export／測定 | `run_dev` の CLI 経路だけで、GUI handler から呼ばない（依頼の対象外） |

残した出力は、`analyze_preview` の poisoned timeline 診断（preview worker）、
`export_png_sequence` の計測診断（PNG worker）、`AudioWorker::note_error`（audio worker）、
persistence worker 自身の出力、および test／開発 bin の CLI 出力だけ。
これらは依頼で対象外とされた thread／用途であり、GUI callback の benchmark stdout とは区別した。
GUI 入力からの同期ファイル書込を persistence へ移す追加箇所はなかった。

追加回帰テスト **6件**は `app/diagnostics_tests.rs`。保存失敗・MP4 失敗・MP4 保存先切断・
音声 command 切断・GUI benchmark stdout・GUI assets／起動失敗の実 handler を通す。
各テストは独立した子の **Rust test harness** を `--exact --nocapture` で起動し、
別 thread が実際の `StdoutLock`／`StderrLock` を channel で停止させる。persistence worker にも
診断を配送して出力を止めた状態を作り、handler が **解放前の3秒以内に完了**することと状態遷移を検査する。
解放後に通常の writer drain を行い、診断／計測結果が実際の出力へ届いたことも親で確認する。
native GUI／実音声 device は生成しない。timeout を観測しても先に出力を解放してから assert／join するため、
同期出力を再導入した場合は harness を停止させず、handler の待機として失敗する。

**6件すべて**について同期出力を復元する変異で失敗した（コンパイル失敗は検出に数えない）。
GUI bootstrap の assets と run result はそれぞれ別の変異で確認し、変異は毎回 finally で元の bytes に復元した。
対応テスト名は `target/p4c-review1/mutations.json`、各ログは同ディレクトリの `mutation-*.log`。

| 変異 | 失敗した回帰 |
| --- | --- |
| `save-direct-stderr` | 保存失敗 |
| `mp4-direct-stderr` | MP4 書出失敗 |
| `choice-direct-stderr` | MP4 保存先切断 |
| `audio-direct-stderr` | 音声 command 切断 |
| `benchmark-direct-stdout` | GUI benchmark 終了 |
| `assets-direct-stderr` | GUI assets 作成失敗 |
| `gui-result-direct-stderr` | GUI 起動失敗の終了処理 |

**P3: 冒頭の実機検収状態を訂正。** 「P4 の実機検収・P4c の実装・非対話検証あり」を
**「P4 の実機検収は未完了。P4c の実装・非対話検証あり」**へ変更した。
実装・自動検証が緑であることを、実機検収や独立再レビューの受け入れとは扱わない。

復元後の指定 gate（全 Cargo コマンドで `MSBUILDDISABLENODEREUSE=1`、全コマンド exit 0）:

| コマンド | 結果 |
| --- | --- |
| `cargo test -p musicvideo-editor` | **177 passed / 0 failed**（初回171＋今回6） |
| `cargo check -p musicvideo-editor --features dev-tools --bins` | 成功 |
| `cargo test -p musicvideo-core -p musicvideo-ui` | core115＋経路監査2＋UI124＝**241 passed / 0 failed** |
| `cargo test -p mimageviewer --lib runtime_` | **98 passed / 0 failed / 1 ignored** |
| `cargo fmt --check` | 成功、`cargo fmt` 実行済み |
| `python scripts/check_ui_glyphs.py` | 危険 glyph **0** |
| `cargo build --release -p musicvideo-editor --bin mimageviewer-musicvideo` | 成功、起動なし |

合計 **516 passed / 0 failed / 1 ignored**。既存 ignored を維持し、既存 P4c・H1〜P4b-4 を通過。
command／feature／profile／exit／件数／所要時間とソース SHA-256 は `target/p4c-review1/gates.json`、
ログは同ディレクトリの `editor,check,core-ui,runtime,fmt,glyphs,release.log`。
初回 `target/p4c-validation` の証拠は上書きしていない。
今回の release exe は `target/release/mimageviewer-musicvideo.exe`、**25,344,512 bytes**、
SHA-256 **`aa0af7028e802e69f6c5a08ea27499f7ec0672bd1bd69570c15573178edafbc4`**（上の初回成果物の hash は当時の記録）。
GUI 起動・実機入力・通常 `%APPDATA%\mimageviewer` へのアクセス・コミットは行っていない。
既存 untracked `out.mp4`、P4d の brief、mIV の製品コードは未変更。

今回の変更ファイルは editor の `app.rs`、`app/document_intent.rs`、`app/mp4.rs`、`app/startup.rs`、
`lib.rs`、`persistence.rs`、`playback.rs`、追加 `app/diagnostics_tests.rs` と本設計書の **9ファイル**。
仕様との相違・機能制限・回復機構の追加はない。独立再レビューにこの差分と証拠を引き継ぐ。

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
  | TimeText     { format, font, size, align, fill: Animatable<Fill>, style: Animatable<TextStyle> }
  | Text         { text, font, size, align, fill: Animatable<Fill>, style: Animatable<TextStyle> }
  | Lyrics       { lines, fade_in_secs, fade_out_secs, font, size, align, fill: Animatable<Fill>, style: Animatable<TextStyle> }

Animatable<T> = Const(T) | Track(Vec<Keyframe<T>>)
Keyframe<T>   = { t: f64, value: T, interp: Step | Linear | EaseIn | EaseOut | EaseInOut }
TextStyle     = { outline?, extra_outlines, shadow?, echo?, background?, letter_gap, line_gap }
Fill          = Solid(Color) | LinearGradient{..} | RadialGradient{..} | BandGradient{..}
```

- **画像の切り替え + フェード**は「Image レイヤを複数置き、それぞれの `opacity` にキーフレームを
  打つ」で表現する（専用のスライド機構を先に作らない）。煩雑なら P3 で「slides」ショートカットを
  検討する（未決 Q1）。
- `BandGradient` は「バンド位置に応じて色を変える」用（高域ほど明るい等）。
- 変調はスカラー対象だけに限定し、`Animatable<T>` 自体は汎用変調しない。最終値は
  Const / Track の評価値へ binding を宣言順に加算する。Scale は矩形中心を固定して適用する。

独立実装レビュー 2 回目（GPT-6.1 Sol / xhigh、設計担当が依頼）: 2 件とも解消、追加指摘なし、**ACCEPT**。

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

### P4d 実装記録（2026-10-15）

正本は D10「起動と引数」と §7 P4d、実装 brief は `docs/brief-musicvideo-p4d.md`。
対象はブランチ `musicvideo`、基点 `6729628cd80ea4ce2c6e3fb9c20b470a1fb41074` の未コミット差分。
以下は初回実装担当の非対話検証記録。コミット `261b10359` に対する独立レビューと対応は末尾に追記する。

**実装**

- `src/context_menu_model.rs`: 静的 `CreateMusicVideo` の command / 永続 ID / stable name /
  label / ALL / from_command / 既定順を追加。単一の実 `Audio`、Grid / Fullscreen のみで表示する。
  既定位置は末尾。catalogのALLと生メニューの両方へ末尾追加し、既存の保存順で動的固定枠をまたぐ
  既存項目の移動を防ぐ。checked／フォルダ文脈／他の全kindでは出さない。
  既存 `resolved_order` を利用し、移行は追加しない。非表示・並べ替えは既存設定を使う。
  設定の確認場面に一覧／全画面の音声を追加した。既存の画像・動画等の項目と順序は維持する。
- `src/ui_dialogs/context_menu.rs`: 既存の入力ゲートと同じ位置で dispatch し、音声pathだけを起動要求へ渡す。
  フォルダ文脈の入力ゲート許可リストへは足さない。既存のclipboard／外部ツールdispatchは同じ引数・経路を維持する。
  テストではlauncherを差し替え、製品GUIや子プロセスを起動しない。
- `src/musicvideo_launch.rs`、`src/app.rs`、`src/lib.rs`: 起動ごとのworker／receiver／取消tokenをAppが所有する。
  UIはpathと使用中の `data_dir` のsnapshotを渡すだけ。worker内で `current_exe()` の兄弟
  `mimageviewer-musicvideo.exe` を解決し、音声・data dirを `std::path::absolute` で絶対化する。
  exeの存在確認とspawnもworker内で行い、`--audio`／`--data-dir`をOS文字列のまま渡す。
  子プロセスは待たずに切り離し、console用flagは追加しなかった。初回記録の「GUI subsystemなので
  コンソールは出ない」は誤りで、製品binの属性が欠落し実際のPEはConsoleだった。独立レビュー1回目の
  指摘を受け、製品binのGUI属性とPE検査を追加した（末尾の対応記録）。
  成功時は無通知、exe不在／spawn失敗／worker切断は理由付きtoastとログ。再試行や復旧機構は追加しない。
- 起動要求は互いを置き換えない。複数エディタを許すD10に従い、連打は独立したworkerになる。
  ナビゲーションやfullscreen終了で起動要求を取消しない。結果は閲覧状態を変更せず、Appのpollで通知だけを行う。
  App終了ではpendingのDropが取消tokenを立て、既存外部ツール同様に未起動の処理を止める。
  起動済みの子は止めず、workerのjoinもしない。
- 設計簡素化: 別exeで閲覧／編集状態を共有しないD10の境界を維持する。起動をモーダルにすると
  閲覧と複数起動を制限するため採らず、IPC／世代／rollback／live rebuildを追加しない。
- `scripts/build-dev.ps1`: 通常featureの製品editorを `dev-runtime` へ追加build。
  他の3exeと同じexact-pathの停止／`-PreserveRuntime`、成果物存在確認、PE／CRT検査、出力案内へ含める。
  scriptはASCIIのみ。`scripts/test-build-dev-safety.ps1` も4exe／4Cargo呼び出しとeditorのprofile／binを検査する。
  release／launcher／portable／署名は変更しない（P4e）。manual／製品ページ／keymap-specは変更しない（P4f）。

**追加テストと変異検証**

追加Rustテストは8件。すべて `context_menu_musicvideo_` を名前に含め、通常実装で8 passed／0 failed。
意図的な変更は実装へ入れ、compile成功後のテスト失敗を検査し、元のバイト列へ復元した。
配置訂正後も変異Aは7 failed／1 passed（順序テストも対象判定の変異で失敗）、変異Bは2 failed／6 passed。
拡張した順序テストは、中間配置へ戻す変異Cで0 passed／1 failedになり、右回転が動的tool枠を
またぐ退行を検出した。全変異は元のバイト列へ復元済み。

| 追加テスト（共通prefixを省略） | 検査対象 | 検出した変異 |
| --- | --- | --- |
| `only_single_real_audio_on_both_surfaces` | 13kind × 2surface × 単一／checked／フォルダ文脈、label／enabled | AudioをVideoへ変更（A） |
| `saved_order_and_hidden_preserve_existing_items` | 新IDなしの既定／逆順／各既存IDの先頭・末尾移動、既知ID順と既定slot、Explorer／pin／新項目の非表示、2tool・2関連付けappと区切りを含む全既存node | commandの永続ID対応をCopyPathへ変更（B）、新IDと生メニューを中間挿入へ戻す（C） |
| `dispatch_uses_launcher_seam_and_input_gate` | 両surface、音声／動画、checked／背景、入力ゲート、fullscreen維持 | launcher呼び出しを削除（A） |
| `arguments_are_absolute_and_keep_os_paths` | exe兄弟、4引数、相対／絶対data dir、日本語・空白path | data dirの絶対化を削除（A） |
| `worker_is_nonblocking_and_allows_multiple_launches` | UIと別thread、使用中data dir、2起動保持、成功無通知、外部tool状態不変 | 新要求時に既存pendingをclear（B） |
| `missing_exe_and_spawn_failure_notify_with_reason` | exe不在と注入spawn失敗、path／理由付きtoast | pollの失敗通知を削除（A） |
| `disconnected_worker_notifies_and_finishes` | 切断の通知とpendingの終端 | pollの失敗通知を削除（A） |
| `drop_cancels_without_waiting_or_spawning` | worker待機中のpending Drop、取消後spawn抑止、joinなし | Dropの取消tokenをfalseへ変更（A） |

ビルドscriptの既存回帰も更新した。editorのexact-pathをcoreへ誤指定する変異と、editorのCargo呼び出しを
remoteへ誤指定する変異は、それぞれscript-only検査でexit 1になった。scriptも元のバイト列へ復元した。
プロセス操作はmockであり、製品の起動・停止を行わない。
ログは `target/p4d-added-tests.{log,stderr.log}`、`target/p4d-mutation-{A,B}.{log,stderr.log}`、
`target/p4d-build-mutation-{process-policy,build-command}.log`。

**非対話gateと確認build**

初回の中間配置を実装担当が追加点検したところ、「ゴミ箱を先頭へ移動」「Explorer／pinが
非表示」の既存保存順では、追加によって既存の右回転が外部tool枠の前へ移るケースがあった。
`resolved_order` 自体は既知のstatic ID順を保っていたが、生メニューのstatic slotが増え、
動的固定枠を挟む割当が変わるためである。briefは新項目の既定位置を指定していないので、
新IDと生メニューの両方を末尾追加へ訂正した。既存のlayout resolver／外部tool実装は変更しない。
上記の保存順テストを拡張し、区切りや動的groupの内部まで含め、新項目を除いた全nodeが
新項目を非表示にした旧メニューとexact一致することを検査する。
sourceに基づく中間配置の再現は `target/p4d-order-premise.log`、訂正後の変異Cのログは
`target/p4d-mutation-C.{log,stderr.log}`。訂正後の検証を以下に記録する。

配置訂正前の全体gate（`RUST_TEST_THREADS=1`）はexit 101。本体12736 passed／1 failed／59 ignoredで、
失敗は既存 `hud_focus_claim_uses_registered_editor_and_preserves_presenter_return` のHWND通知待ちだった。
実装担当が追加した直列設定により、libtestの表示が子の `MIV_FOCUS_WINDOWS` 通知の行頭へ付き、
既存readerの `strip_prefix` で読めないことをGUIなしの小さなlibtestプローブで検査した。
ログは `target/p4d-stdout-{serial,parallel}.log`。これはコマンド実行時間切れや製品の起動失敗ではない。
直列指定を外して当該テストと最終gateを再実行し、native実装／テストの期待値は変更しない。
初回全体gateのログは `target/p4d-full.{log,stderr.log}`。

配置訂正後の既定並列gateもexit 101（本体12736 passed／1 failed／59 ignored）。
失敗は既存 `import_uses_bounded_outer_transactions_instead_of_per_item_commits` の3 batch期待に対する
4 batchだった。実装の256件／64MiBに加え500msの時間上限でもcommitするため、並列負荷で
余分なbatchが生じ得る。取り込みコード／期待値を変更せず、親libtestだけCLI
`--test-threads=1` で直列化し、`RUST_TEST_THREADS` は未設定のままで全体gateを再実行する。
これにより子のHWND通知は既定の並列形式を保つ。全対象・assert・vendor gateは省略しない。
実行ラッパーは `target/p4d-full-serial-parent.ps1`（repositoryの実行scriptは変更しない）。
既定並列のログは `target/p4d-full-final.{log,stderr.log}`、親だけ直列のログは
`target/p4d-full-serial-parent.{log,stderr.log}`。初回ラッパーのPowerShell変数scope誤りはCargo開始前に
訂正した。コマンドrunnerの中断／timeoutによるproduct failureとしては数えない。

最終の親だけ直列gateはexit 0、**14560 passed／0 failed／66 ignored**。
workspace／integration／doc／vendorの全対象を含み、ネストしたhelper subprocessのfiltered結果2件は
合計へ二重計上しない（79 summary）。集計は `target/p4d-full-summary.json`。
metadataのbatch数テストとnative HWND通知テストは、この全体gate内で両方成功した。
error modeは終了時に元の値へ復帰した。native／metadataの製品コード・テストの期待値は未変更。

| 検証コマンド | 結果 | 記録 |
| --- | --- | --- |
| `cargo test -p mimageviewer --lib context_menu` | 最終138 passed／0 failed | `target/p4d-context-final.{log,stderr.log}` |
| `cargo test -p mimageviewer --lib external_tool` | 最終97 passed／0 failed | `target/p4d-external-final.{log,stderr.log}` |
| `cargo test -p mimageviewer --lib` | 初回12737 passed／0 failed／59 ignored。配置訂正後の最終全体gateでも同じ本体12737 passed／0 failed／59 ignored（1916.28s） | `target/p4d-main-lib.{log,stderr.log}`、`target/p4d-full-serial-parent.{log,stderr.log}` |
| `cargo test -p musicvideo-editor` | 最終177 passed／0 failed、bin／docは0件 | `target/p4d-editor-final.{log,stderr.log}` |
| `cargo test -p mimageviewer --test ui_snapshot` | 最終176 passed／0 failed | `target/p4d-ui-snapshot-final.{log,stderr.log}` |
| `cargo fmt`、`cargo fmt --check` | 適用済み、最終check exit 0 | `target/p4d-format-apply.log`、`target/p4d-fmt-final.log` |
| `python scripts/check_ui_glyphs.py` | 最終不足0、exit 0 | `target/p4d-glyph-final.log` |
| `scripts/test-build-dev-safety.ps1` | mockによる4exe検査PASS、exit 0 | `target/p4d-build-script-final.log` |
| `scripts/test-full.ps1 -SuppressCrashDialogs`（上記親CLI直列ラッパー） | 全対象14560 passed／0 failed／66 ignored、exit 0 | `target/p4d-full-serial-parent.{log,stderr.log}` |

最終の追加8件と変異A／B再検証、C、snapshot更新、各gateのコマンド・終了コードは
`target/p4d-final-gates.json`（最後の既定並列fullは上記の失敗記録）に残す。
最終source／script／PNGの9ファイルは `target/p4d-source-freeze.json` にSHA-256を保存し、
全体gate後に同一であることを照合した。文書更新は検証を無効にするinclude対象ではない。
確認buildは `scripts/build-dev.ps1 -PreserveRuntime` を `*>&1` なしで実行し、exit 0。
core／remote／EPUB worker／製品editorの4exeが `target/dev-runtime` に揃い、PE／CRT検査は
`runtime=4 pe=4` で成功した（`target/vcrt-pe-reports/dev-runtime.json`）。portable featureは付けない。
coreは2026-10-15 03:28:06 JST、SHA-256
`3F064E4683EB281CD1B798B4BE33177C93A2956609904C70E60125C2B98E5639`。
editorは同03:28:17 JST、SHA-256
`96CBBDF698D84D4E7C25517743710DC89CAD1F0A068C32B879A0D3B5F088B1E6`。
4成果物のsize／時刻／hashは `target/p4d-build-artifacts.json`。
build時もAPPDATAを作業ツリー内sandboxへ向けたが、これは親の一時的な環境設定であり、
binaryへ固定したdata dirではない。利用者が通常環境で起動すると通常 `%APPDATA%/mimageviewer` を使う。
生成物は起動していない。独立レビュー・実機検収は未実施のまま。

利用者の確認手順: installed／tray常駐mIVを終了してから、本worktreeで
`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`。
通常profileなので実設定／dataを更新し得る。実音声fileの一覧／音楽ビューの右クリック末尾から
「音楽ビデオを作成」でその音声のeditorが開くこと、画像／動画（動画→音声mode含む）／checkedでは
項目が出ないこと、非表示設定と既存外部toolの動作を利用者が確認する。
全build／Cargo検証に `MSBUILDDISABLENODEREUSE=1` を設定。
テストのAPPDATAは `target/p4d-sandbox/appdata`、App fixtureのdata dirは既存の一時ディレクトリ。
通常 `%APPDATA%/mimageviewer`、既存untracked `out.mp4` は変更しない。コミットと製品GUIの起動は行わない。
snapshot差分は設定画面2枚（新規なし）、更新前後を実装担当が目視した。
詳細は `docs/ui-snapshot-policy.md`。正本／briefとの仕様上の相違はない。

**独立レビュー 1 回目 REVISE と対応（2026-10-15）**

利用者から提示された独立実装レビュー（GPT-6.1 Sol / xhigh、対象コミット `261b10359`）は
REVISE。P2「エディタ起動時にコンソール窓が開く」1件で、その他のP1／P2／P3指摘はない。
以下は実装担当による対応・非対話検証であり、新たな独立レビューではない。

- `crates/musicvideo-editor/src/main.rs` に本体 `src/main.rs` と同じ
  `#![windows_subsystem = "windows"]` を追加。製品binだけを対象とし、CLI出力を使う
  `src/bin/musicvideo-dev.rs` とmIVのspawn／既存メニュー／外部ツールの経路は変更しない。
- `scripts/check-vcrt-pe-dependencies.ps1` にdumpbin headersのSubsystem解析と報告を追加。
  製品 `mimageviewer-musicvideo.exe` は2（GUI）、開発 `musicvideo-dev.exe` は3（Console）を必須とする。
  既存のbuild-dev最終PE gateから自動適用し、release成果物も同じgateで検査する。
  self-testは両profileの正しい値を受け入れ、逆の値とSubsystem欠落を拒否し、他のexeには新条件を適用しない。
- stderrの確認: `persistence::Diagnostics::write` はworkerからstderrへ書き、data-dir無しでは
  ファイルログと最近使ったプロジェクトを作らない。引数誤り／runtime保護の失敗／読込・保存・MP4・
  音声workerの失敗はstatus／notice／error snapshotとして画面へ出るので、通常操作の通知を失わない。
  既存のdata-dir無しの描画テストと診断配送テストでこの経路を確認した。
  GUI自体を起動できない場合とpanicの診断は、直接起動・data-dir無しでは見えなくなる制約がある。
  D10の開発用直接起動はconsoleの `musicvideo-dev`、mIV経由では必ず渡されるdata-dirの
  `logs/musicvideo.log`／`musicvideo-panic.log` を使う。推測した通常profileへの書込みや
  コンソール再生成は追加しない。D10へこの制約を明記した。
- 失敗確認: 修正前release成果物（Subsystem=3）を新gateが `expected=2 actual=3` で拒否した。
  拒否条件を無効化した一時scriptのself-testも「wrong subsystem」で失敗した。製品sourceには変異を残していない。
  記録は `target/p4d-review1-console-rejection.log`、`target/p4d-review1-pe-mutation.log`。

対応後の検証（すべてexit 0。Cargo／buildは `MSBUILDDISABLENODEREUSE=1`、
APPDATAは本worktreeの `target/p4d-review1-sandbox/appdata` に限定）:

| コマンド／対象 | 結果 | 証跡 |
| --- | --- | --- |
| `cargo test -p musicvideo-editor` | 177 passed／0 failed、bin／docは0件。data-dir無しのエラー描画と診断配送の既存回帰を含む | `target/p4d-review1-editor.{log,stderr.log}` |
| `cargo test -p mimageviewer --lib context_menu_musicvideo_` | 8 passed／0 failed | `target/p4d-review1-launch-tests.{log,stderr.log}` |
| `scripts/check-vcrt-pe-dependencies.ps1 -SelfTest` | parser／両profileのSubsystem条件と既存PE／CRT self-test成功。拒否条件無効化の変異は失敗 | 実行出力、`target/p4d-review1-pe-mutation.log` |
| `scripts/test-build-dev-safety.ps1` | PASS。process操作はmockのみ | 実行出力 |
| `cargo fmt`、`cargo fmt --check`、`python scripts/check_ui_glyphs.py` | fmt適用／check clean、glyph 0 | 実行出力 |
| `cargo build --release -p musicvideo-editor --bin mimageviewer-musicvideo` | 成功（2分47秒）。製品PE Subsystem=2 | `target/p4d-review1-release.{log,stderr.log}`、`target/p4d-review1-release-pe.json` |
| `scripts/build-dev.ps1 -PreserveRuntime`（`*>&1`なし） | 成功。通常featureの4exe、runtime=4／PE=4。製品PE Subsystem=2 | `target/p4d-review1-build-dev.log`、`target/vcrt-pe-reports/dev-runtime.json` |
| `cargo build --profile dev-runtime -p musicvideo-editor --bin musicvideo-dev --features dev-tools` | 成功（7.48秒）。CLI PE Subsystem=3 | `target/p4d-review1-cli.{log,stderr.log}` |
| 両製品exe＋開発CLIへ同じPE gateを適用 | runtime=4／PE=3。release製品=2、dev-runtime製品=2、開発CLI=3、SHA-256を記録 | `target/p4d-review1-pe.json` |

変更は製品bin、PE script、本設計書の3ファイル。Rust新規テストとsnapshot変更はない。
実装担当による再実行はRust計185件／0 failed。元の全体gate（14560 passed／0 failed／66 ignored）は独立レビューで
再集計・source freeze／成果物hash一致を確認済み。今回の変更は製品bin属性・PE検査・文書に限定されるため、
本体の共有実装・snapshotに変更はなく、その全体gateを再利用し、変更箇所の検証と両profileのbuildを行う。
GUI起動・実機検収と対応後の独立再レビューは未実施。コミットは行わない。

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
| **T1** | 文字装飾（D15）: comic-core既存装飾をText / TimeText / Lyricsに共通接続 | 初版fce65bb15、巨大確保・昇格・clip修正f54b6e5b6、準備共有・旧注釈22hash固定338a38899は条件付きACCEPT。保持64MiB = 静的RGBA24 / 補間RGBA8 / 準備32、1焼成RGBA8MiB以下。straight RGBA修正は検収採用済み。今回P3: eviction前後・再準備のglyph RGBA比較、旧非空判定への復帰変異も検出。計測器をscripts/musicvideo/t1-measure.pyへ保存し、前/中のCPU・新規PID・終了後CPU・短命PID通知を監視、取得不可も棄却。16ケース各3回＋5分全補間/混在、3枠保持とOSメモリ時系列・合否目安・所要時間をJSON/表へ。core104 / UI121 / lab75 / Python15、計315件成功、計測feature付きeviction1件も成功。既存4hash・fmt維持、release headless例build成功。**P2正式性能計測は利用者指定で今回は未実施、後日利用者が python scripts/musicvideo/t1-measure.py を実行**。smokeは競合/WMI拒否で不採用。初回4.274ms等を現行の採用値にしない。今回comic-core変更・commit・アプリ起動・build-devなし。手順・旧経路未測定理由はD15参照 |
| **I1 段 2a / 2b 実装・自動検証済み／実機確認待ち** | IME 補正の共有（D10「設定・入力」）: 本体 `ime_focus` の egui 依存部分（入力補正 plugin・single-line helper）を共有クレートへ挙動不変で切り出し、本体とエディタのホストが policy を登録する | **段 2a（2026-10-09、`367f9c9d3`・独立実装レビュー ACCEPT）**: `egui-ime` と本体 facade、既存呼び出し元を維持。既存 23 件は共有 16 / 本体 7 に配置、raw 禁止・adapter を追加（共有 17 / 本体 12 passed）。full gate 12,681 passed / 58 ignored、fmt / glyph、通常 profile の確認用 build 成功。**段 2b（同日、未コミット）**: 全域 Tab policy を挙動不変で共有化、本体 facade・既存 5 テストを維持。エディタの登録入口・5 欄 helper・Undo gate・raw 禁止を実装、歌詞 buffer / widget ID / native Undo と Hz DragValue を維持。利用者承認でモジュレータ見出しを不変 ID に固定し、同型箇所も調査。追加 13 テスト、UI 117 / ラボ 75 / core 80 / 共有 17 passed、既存 L1〜L3 全件維持、変異 6 件を検出。本体 Tab / IME / 関連・snapshot も通過。full gate **12,694 passed / 58 ignored / 0 failed**、fmt / glyph / 依存検査、ラボ release build 成功。独立実装レビュー **ACCEPT、指摘なし**。build-dev 再実行・コミット・アプリ起動なし。詳細は D10「I1 設計」§7・§8。**実装・自動検証は完了、実機確認・最終受け入れは未完了**。本体の確認用 exe は更新せず、ラボの確認用 exe は `target/release/musicvideo_lab.exe` |
| **H3a** ✅ | 解析契約の確定と実装: 共通格子／PCM view／解析タイムライン（チェックポイント・型付き key）／較正入力を、プレビュー・書き出し・PNG・ベンチマークで共有 | **完了**（2026-10-14、`b473603cc` + `b913d9408`）。設計は実装前レビュー 3 回目で ACCEPT、実装は独立レビュー 1 回目 REVISE（P2 2・P3 1）を修正し 2 回目で ACCEPT。A6 は設計担当判断で 4 分位置 約 0.7 秒以内（並列）を採用、生の値のキャッシュなし。解析を使う書き出しの RGBA ハッシュは予定どおり変化、無変調ハッシュ維持。詳細は D3 の H3a 実装記録。画素の一致の確認は H3b（P4b 後） |
| **H3b** ✅ | 一致の検証（P4b 接続後） | **完了**（2026-10-14、`6c1784aa2`、設計担当が依頼した独立レビュー ACCEPT。H3 完了）: 実 preview worker／採用 texture と共通 snapshot export の RGBA が一致。毎 frame、欠落／境界、前後／直接 seek、checkpoint／反復、設定変更／復元、短尺／長尺／同尺。新規 10 件すべて変異失敗確認、指定 gate 409 passed。不一致なし・製品動作変更なし。詳細は D3 の H3b 検証記録 |
| **H3（H3a + H3b）** 非対話検証完了 | 解析の契約と製品画素の一致 | H3a の共有格子／view／timeline／較正に、H3b の実 preview と snapshot export の画素検証を接続。実機 GUI／音声 transport の検収とは区別する |
| **P4a** ✅ | 方式の設計確定: 本書 D10 / D14〜D16 の独立レビュー | GPT-6.1 Sol / xhigh の設計レビューで blocking 指摘なし。**完了**（2026-10-09）: 3 回目で条件付き ACCEPT（残った P3 の文言は反映済み）。runtime 競合の割り切りはユーザー判断で確定 |
| **P4b** 実装完了・実機確認待ち（P4b-1〜4 すべて独立レビュー ACCEPT、2026-10-14。上書きの競合は利用者が許容。5 分 1080p60 は完走済み・速度は参考値で静かな環境での再計測が残る。音・GUI の実機確認は利用者へ依頼） | ホストの製品化（D16。段の分け方は D16 の「P4b の設計」）: ラボを `crates/musicvideo-editor` へ移し、FFmpeg デコードへ統一、スナップショット入力の MP4 書き出し（モーダル・進捗・キャンセル）、未保存の編集の確認、音声スレッドが stream を所有する再生と f32 以外の出力形式対応 | **P4b-3（2026-10-14、未コミット）**: 指定ゲート 365 passed、追加 18 件すべての変異失敗確認、既存 editor 101 件維持。共通 snapshot／解析／較正／PCM view AAC／原子的公開／モーダル終端を接続し、製品 editor・通常 CLI・dev CLI の release build 成功。編集済み 5 分 1080p60 を dev CLI で 18,000 frames 完走、184.441 秒／1.6265x（背景 Codex 4 件あり、参考値）。詳細は D16 の P4b-3 記録。P4b-2 の実機の音・GUI・H3b は未確認。**P4b-4（2026-10-14、未コミット）**: session 所属の単一意図、歌詞の先行解決、保存後のフレーム末尾の再判定、先行読込の receiver 退役、新規、CancelClose／承認済み Close と書き出し中止の終端待ちを実装。指定ゲート **396 passed**、追加 **31 件**すべて変異失敗確認。初回レビュー REVISE の P2／P3 に対応し、LRC モーダルの実クリック回帰、Project／保存完了の両順序、歌詞取消と同フレームの旧完了を検査。今回 3 件追加・既存 1 件補強、各テストの変異失敗確認、指定テスト **399 passed**。詳細は D16 の P4b-4 記録。修正後の独立レビュー・実機確認・H3b は未完了 |
| **P4c** 実装完了・実機確認待ち（独立レビュー ACCEPT、2026-10-15） | 起動引数と診断（D10）: `--audio` / `--project` / `--data-dir`、最近使ったプロジェクト、ログ・panic ログ、引数エラーの画面表示、runtime shared lease と取得失敗の表示 | **初回2026-10-14**: 追加21件の変異検証記録あり。**2026-10-15、未コミット修正**: P2のUI出力全経路9か所をworker配送、追加6件すべてで同期出力の変異失敗、P3の実機未完了表記を訂正。指定gate **516 passed / 0 failed / 1 ignored**、fmt／glyph0／製品release build成功。再レビュー・実機検収未実施。詳細は「P4c 実装記録」の独立レビュー1回目対応 |
| **P4d** 実装・非対話検証済み（独立レビュー1回目REVISE対応・再レビュー／実機確認待ち） | mIV 側の入口: 音楽ビュー・音声ファイルの右クリックから「音楽ビデオを作成」でエディタを起動 | Grid／Fullscreenの単一実Audioだけ。static ID／非表示設定、worker起動、絶対path引数、失敗通知、build-dev同梱を実装。既存保存順のstatic／動的枠を保つ末尾追加。追加8件の成功と各変異失敗、全対象14560 passed／0 failed／66 ignored、fmt／glyph0、通常featureの4exe確認build成功を記録済み。`261b10359`の独立レビューP2に対応し製品binのGUI属性／Subsystem gateを追加。製品GUI起動／実機検収／対応のcommitなし。詳細は「P4d実装記録」 |
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

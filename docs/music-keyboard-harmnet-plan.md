# 音楽ビュー鍵盤の基音推定 (HarmNet 移植) 設計

状態: 設計 (2026-10-12、利用者が方針 A と下記の推奨 2 点を承認)。実装前の独立レビュー (GPT-6.1 Sol /
xhigh) 1 回目 = ACCEPT WITH CHANGES、指摘は §12 のとおり反映。
作業ブランチ: `pitch-lab` (worktree `G:\home\mimageviewer-pitchlab`)。

## 1. 目的と背景

音楽ビュー下段のピアノ鍵盤 (`src/ui_music_spectrum.rs` の `draw_pitch_keyboard`) は、各半音帯域の
**スペクトルエネルギー**を表示している。利用者の報告では EffeTune v2.13.0 の Note Spectrogram と比べて
「半音上などが間違えて選ばれるケースが多い」。2026-10-12 の調査 (pitch-lab ブランチの
`crates/music-core/examples/`、手順は `examples/pitch_lab/README.md`) で次を確認した。

- 正解付き合成音で、mIV は倍音・オクターブ上の鍵と**半音上の鍵**を誤点灯する
  (ベース+和音+歌の混合で半音上 187 件 / 半音下 0 件)。EffeTune はほぼ 0。
- 倍音のない純音でも 140Hz 以下では半音上の鍵が正解の 0.5〜0.8 倍の明るさになる。原因は
  (1) 16384 点窓で 100Hz 付近の半音が約 2 ビンしかない分解能、(2) 半音幅の箱の平均/最大×0.35 による
  割り当て、(3) 表示用に dB+ガンマ圧縮した値で比較していること、(4) Layer ② の倍音加点が ±1 半音の
  max で倍音を探すため隣の鍵にも加点されること、(5) 基音と倍音を区別しないこと、(6) 再生位置を中心に
  した長い窓が次の音を最大 0.17〜0.34 秒早く光らせること。
- 実曲 2 曲 (正解なし) で「半音隣同士が同時に点灯している割合」は mIV 14.7% / 8.6%、
  EffeTune 3.4% / 1.9%。

EffeTune は学習済みの小型ネット **HarmNet L** で「その音が基音である確率」を 88 鍵ぶん推定している
(EffeTune DSP 0.13.0、MIT、`dsp/plugins/analyzer/note_spectrogram/`)。pitch-lab の Rust 移植試作
(`examples/harmnet_dump.rs`) は Python 版 `effetune==0.13.0` の即時推定と最大差 1e-4 で一致し、
scalar 実装で 1 フレーム (20ms) あたり約 2ms だった。

## 2. 決定事項 (利用者承認済み)

| ID | 決定 |
| --- | --- |
| K1 | 鍵盤の**中央の明るさ**は HarmNet の基音確率から求める。鍵盤の意味は「帯域エネルギー」から「推定した基音」に変わる |
| K2 | 中央の明るさ = **HarmNet 確率 × 既存の音量ゲート (presence)**。presence は今と同じく Norm (ラウドネス正規化) の適用ゲインを反映する |
| K3 | 鍵の**上下のグラデーション** (上=奇数倍音、下=偶数倍音) は残す。上端 = 中央 × 奇数倍音比、下端 = 中央 × 偶数倍音比。比率は従来どおりスペクトル (`notes`) から `harmonic_energies` で求める。HarmNet が基音と判定しない鍵は中央が暗いので上下も光らない |
| K4 | 計算は**再生中に順に進める**。シーク等で連続しないときは、表示位置の約 1 秒前から先に流して状態を作る (助走)。曲全体の事前計算・保存はしない |
| K5 | mIV は曲をデコード済みなので、**160ms 先読みの推定 (head 3、age 8)** を遅延なしで表示に使う |
| K6 | 従来の Layer ② (倍音加点) と Layer ③ (局所正規化による間引き) は外す。Layer ① (知覚補正) は上の棒グラフと倍音比率の計算に引き続き使う |
| K7 | Bass ▼ マーカー、上の棒グラフ、`tools/music_lab` は今回変えない |
| K8 | 作者へは実装時に利用者が一報する (MIT 表記は下の §7 で全配布形態に出す) |

## 3. 状態の組み合わせを減らす検討 (CLAUDE.md 2026-09-28)

- **事前計算をしない (K4)**: 曲全体を背景で計算して保存する案は、進捗・部分完了・曲切り替え中の
  キャンセル・保存先の寿命という状態を増やす。順に計算 + 助走なら、状態は「HarmNet の内部状態と、
  次に計算するフレーム番号」だけで、どこへ飛んでも同じ 1 つの判定 (続けるか作り直すか) で済む。
  助走の費用は 1 秒分 ≈ 58 フレーム ≈ 0.1 秒 (scalar 実装) で、シーク直後に鍵盤が 0.1 秒ほど
  古いままになるだけ。
- **所有者は既存の spectrum worker 1 つ**: HarmNet の状態は `run_music_spectrum_worker` のスレッドが
  `SpectrumAnalyzer` と並べて所有する。UI スレッドは状態を持たない。曲の切り替え・音楽ビューを閉じる
  操作は既存の `MusicSpectrumState::clear()` がワーカーごと捨てるので、新しい終了経路を足さない。
- **結果は既存の 1 本の結果チャネルに載せる**: `SpectrumAnalysis` に確率の配列を 1 つ足すだけで、
  別チャネル・別 pending を作らない (棒グラフと鍵盤の時刻が常に同じ要求から来る)。
- **所有の前提**: 音楽の状態 (`music_pcm` / `music_spectrum`) は ViewerContextBundle ではなく App 全体で
  1 つ。新しいメディアを開く前に parked-live を閉じる契約と、音源の所有者に限った teardown により、
  同時に 2 つの音源を描くことはない。tracker はさらに **PCM の同一性 (`Arc::ptr_eq`) と絶対フレーム番号**
  で継続を判断するので、teardown の経路に漏れがあっても別の音源の状態を使うことはない (音声トラック
  変更・別ファイルは別の `MusicPcm` になる)。park / 復帰は同じ PCM なので状態がそのまま続く。
- まれな失敗 (計算が想定より遅い等) は割り切り: 追いつけない分は助走扱いで作り直す (§5.3)。
  再試行や状態保持は足さない。

## 4. モジュール構成

### 4.1 music-core に `pitch` モジュールを追加

`crates/music-core/src/pitch.rs` (必要なら `pitch/` 配下に分割) に HarmNet L の前処理と推論を移植する。
pitch-lab の `examples/harmnet_dump.rs` を出発点にしてよいが、次を満たすこと。

- 公開型の例: `HarmNetTracker` (内部状態を持つ。`new()`, `reset()`, `push_frame(samples_ending_at)`,
  `probabilities(head) -> [f32; 88]`) と、定数 `HARMNET_HOP_SECS` 相当。
  名前・形は実装者が決めてよい。UI 依存は持たない。
- サンプルレートは `MusicPcm` が常に 48kHz なので 48kHz を前提にしてよいが、窓長・hop は
  EffeTune と同じ式 (窓 = round(rate×4096/48000)、round(rate×16384/48000)、hop = round(rate×0.02))
  で求め、48kHz 以外を渡されたら空の結果を返す (assert で落とさない)。
- 前処理は EffeTune `harmnet_frontend.h` と同じ: 非対称窓 (短窓のピークは 10ms 前、長窓は 20ms 前)、
  300 本の 1/3 半音三角フィルタ (幅がビン以下なら線形補間)、`ln(1 + 1e4 × v)`、入力の scale/shift。
- 推論は `harmnet_inference.h` と同じ: 3 層の周波数畳み込み (直近 3 フレーム)、0.5〜8 倍の倍音位置の
  集約、±2 音の畳み込み (残差)、音ごとの GRU、4 つの出力 (age 0/2/4/8)、校正値 [0.25,0.20,0.20,0.20]。
- 無音判定: 短窓の分散が 2.511886431509582e-10 未満のフレームは確率 0。**判定は推定対象のフレーム
  自身のもの**を使う (EffeTune は age ごとに過去フレームの判定を保持している。head 3 を使うなら
  8 フレーム前の判定)。
- 重みは `crates/music-core/assets/harmnet_l_effetune_dsp_0.13.0.bin` (float32 LE、47,012 個、
  188,048 バイト) を `include_bytes!` で埋め込む。SHA-256 は
  `0164ecb8b4eb9143a20255669612aed306c9db9b670d892de1aee7492051b6a3` (EffeTune の
  `harmnet_weights.provenance.json` の `weights_sha256` と一致することを pitch-lab で確認済み)。
  テストでこの値を検査する。変換スクリプトは `examples/pitch_lab/convert_weights.py`。
- 1 フレームの計算で heap 確保をしない (配列は構造体に持つ)。性能目標は §8。

### 4.2 `MusicPcm` に範囲コピーを追加

`copy_window` (再生位置 ±1 秒) では助走 (1 秒 + 長窓 0.34 秒 + 先読み 0.16 秒) が足りない。
`MusicPcm::copy_range(start_frame, end_frame)` 相当を足す。範囲の扱い:

- ファイル先頭より前 (負の位置) は 0 で埋める。
- **デコード完了済み (`is_complete()`) なら、ファイル末尾より後も 0 で埋める** (末尾 180ms や
  180ms 未満の短いファイルでも計算できるように。Sol P1)。
- デコード中の先端より後だけを「まだ無い」として扱い、取れた所までを返す (呼び出し側が
  どこまで計算できるかを知れる形にする)。

read lock の保持は今の `copy_window` と同じく短くする。

### 4.3 spectrum worker の変更 (`src/ui_music_spectrum.rs`)

`run_music_spectrum_worker` が `HarmNetTracker` と、それが**どの PCM のどのフレームまで計算したか**を
持つ (`PitchTrackerCursor` 相当: `Arc<MusicPcm>` の同一性 (`Arc::ptr_eq`) + 次に計算するフレーム番号)。
要求ごとに次を行う。

1. 既存どおり ±1 秒窓で `SpectrumAnalyzer::analyze_moving_window` を回す (棒グラフ・`notes`)。
2. 表示したいフレーム番号 `k` を再生位置から求め (§5.1)、先読みに必要な最終フレーム
   `k_end = k + 8` までを計算する (§5.2、§5.3)。
3. `SpectrumAnalysis` に `pitch_probabilities: Option<Vec<f32>>` (MIDI 21〜108 の 88 個、`notes` と
   同じ並び) を入れて返す。**`None` = 未取得** (デコード先端が `k_end` に届いていない)、
   `Some` = 有効な 88 個 (全部 0 もあり得る) を区別する (Sol P3)。
   `SpectrumAnalysis` は `Serialize/Deserialize` を derive しているので `#[serde(default)]` を付ける
   (永続化はされていない: music-integration-plan D8)。

### 4.4 鍵盤の描画 (`compute_keyboard_visuals` / `draw_pitch_keyboard`)

- `center[idx] = pitch_probabilities[idx] × presence`。presence は今の式
  (`update_keyboard_sustain` 後の `notes` の最大値に Norm ゲインを反映し、`KEY_HIGHLIGHT_MIN_PEAK` と
  0.28 で正規化した値の 0.7 乗) をそのまま使う。確率への追加のガンマは定数 1 本 (`KEY_PITCH_GAMMA`、
  既定 1.0) にして実機で調整できるようにする。
- `pitch_probabilities` が `None` のときは中央の入力を 0 として扱い、既存の残光
  (`note_trail`) で自然に消えるに任せる。特別なベースライン描画は足さない (未取得はデコード先端で
  一時的にしか起きないため)。`Some` の全 0 も同じ見た目になる。
- **再要求**: 今の `MusicSpectrumState::update` は `bands.is_empty() || playing || center_changed` の
  ときだけ要求する。一時停止中に棒グラフは取れたが確率が `None` だった場合、デコードが進んでも
  再要求されない (Sol P2)。条件に **「直近の結果の確率が `None` かつ `source_complete == false`」**
  を 1 つ足す (16ms 間隔・in-flight 1 件の既存の間引きに乗る)。デコード完了後は P1 により
  `None` が出ないので、完了後に要求し続けることはない。進捗通知チャネルは足さない。
- 偶数・奇数倍音比は今の `harmonic_energies` と `even/odd = energy / (raw + 1e-4)` の 0..1 クランプ、
  EMA (`KEY_HARMONIC_SMOOTH`) をそのまま使う。上下端の色は今と同じく `key_fill(midi, center × factor)`。
- 中央の残光 (`note_trail` の `KEY_HIGHLIGHT_DECAY`) は残す。
- Layer ② (`KEY_HARMONIC_BOOST`) と Layer ③ (`KEY_LOCAL_*`、`kth_largest`、`local_declutter_strength`)
  は削除する。関連テスト (`keyboard_peak_gate_attenuates_adjacent_key` 等) は新しい意味に合わせて
  置き換える。
- 色相 (`key_color`) は変えない。

## 5. 時刻の対応と連続性

### 5.1 表示フレームの決め方

HarmNet のフレーム `L` は、サンプル `L × hop` で終わる窓から計算する (最初のフレームは `L = 1`)。
長窓のピークが 20ms 前にあるので、フレーム `L` は時刻 `L × hop − 0.02 秒` 付近の音を表す。
再生位置 `c` 秒に対して表示フレーム `k = round((c + 0.02) / hop)` とし、head 3 (age 8) を使うため
`k_end = k + 8` まで計算した時点の head 3 の出力を表示する。

2026-10-12 に pitch-lab で確認した対応 (合成音 3 種、無音境界以外で最大差 5e-5):

- Rust のフレーム `L` (サンプル `L × hop` で終わる窓) = Python の `frame_index = L − 1`
  (`time_seconds = L × hop`)。
- Python の `revised_levels` (age 8) の行 `L` = **推論 `L` の head 3 の出力 × フレーム `L − 8` の
  無音判定**。age 2 (`revisions`) も同じく推論 `L` の head 1 × フレーム `L − 2` の無音判定。
- オンセット 0.2 秒の音で、head 3 の値はフレーム `L − 8` の時刻 ≈ 0.22 秒で立ち上がった。
  したがって `k = round((c + 0.02) / hop)`、`k_end = k + 8` の head 3 がフレーム `k` (時刻
  `k × hop − 0.02`) の推定になる。
- EffeTune のリアルタイム処理は計算を 1 hop に分散するので公開がさらに 1 hop 遅れるが、mIV は
  オフラインで計算するので **この遅れを `k_end` に足さない**。
- Python の 440 セル (1 鍵 5 セル) は各鍵の最大値で 88 鍵へ集約する (細かい位置の形は最大が 1)。

golden テスト (§6) はこれらを固定する。

### 5.2 続けるか作り直すかの判定 (純関数)

```
decide(cursor, pcm, k_end) ->
  Reuse         : 同じ PCM で、計算済みの最終フレーム == k_end
  Advance(n)    : 同じ PCM で、0 < k_end − 最終フレーム ≤ MAX_ADVANCE_FRAMES
  Restart(from) : それ以外 (別の PCM / 後ろへ戻った / 先へ飛びすぎ)。
                  from = max(1, k_end − WARMUP_FRAMES)
```

- `WARMUP_FRAMES` = 58 (1 秒 + 先読み 8 フレーム)、`MAX_ADVANCE_FRAMES` = `WARMUP_FRAMES`
  (それ以上先へ進むなら作り直したほうが安い)。
- 一時停止中は再生位置が変わらない限り要求が来ない (既存の `update` の条件)。小刻みに戻すシークは
  毎回 Restart になるが、要求は 16ms 間隔に間引かれ in-flight は 1 件なので許容する。
- **Restart と Advance の結果が同じになるとは限らない** (GRU の状態が違う)。助走 1 秒後の出力が
  連続計算とほぼ一致することをテストで確かめ (§6)、差の許容値を記録する。

### 5.3 デコードが追いついていないとき

`copy_range` で取れたサンプルまでしか計算しない。`k_end` まで届かなければ、その要求の
`pitch_probabilities` は空 (鍵盤の中央は暗い)。カーソルは計算できたところまで進める。

## 6. テスト

- **重み**: 埋め込みバイト列の長さ 188,048 と SHA-256 一致。
- **golden**: Python `effetune==0.13.0` で作った短い fixture (合成音、2〜3 秒、オンセット・無音・
  和音を含み、**有音→無音と無音→有音の両方の境界を age 8 の範囲にまたがって含む**) の age 0 と
  age 8 の出力を `crates/music-core/tests/fixtures/` に小さなバイナリで保存し、
  Rust の出力が 1e-3 以内で一致すること。fixture の生成スクリプトも同じ場所に置く (Python 依存は
  テスト実行には不要)。
- **判定関数**: `decide` の Reuse / Advance / Restart (別 PCM・後退・飛びすぎ) の状態遷移テスト。
- **助走**: 持続音・和音の切り替え・オンセット・無音後の再発音それぞれの途中へ Restart したとき、
  助走後の出力が連続計算と一致すること。許容差は実装時に測って数値で固定し、本書に記録する
  (目安: 確率の差 0.05 以下)。
- **範囲**: 180ms 未満の短いファイル、hop で割り切れない長さ、末尾の持続音で、デコード完了後は
  末尾まで確率が `Some` になること (P1)。
- **再要求**: 「一時停止・位置固定・bands あり・確率 `None`」→ PCM 追記 / 完了 → 次の要求で確率が
  `Some` になること (P2、`MusicSpectrumState::update` の handler-level テスト)。
- **PCM の同一性**: 同じ位置でも別の `MusicPcm` なら Restart になること。
- **未デコード**: デコード済み範囲の外を要求したとき空の結果になり、カーソルが計算できたところに
  あること。
- **鍵盤**: 倍音のある単音で、中央が最大の鍵が基音であり、オクターブ上・半音上の鍵の中央が
  基音の 0.3 倍未満であること。`compute_keyboard_visuals` の**瞬時値**で、確率 0 の鍵は中央・上下端とも
  0 であること。残光を含む描画状態は別のテストで、確率が 0 になった後に減衰することを確かめる。
- **ライセンス表記**: 通常構成と `portable` 構成の両方で、バージョン情報の notice 一覧に HarmNet
  (EffeTune MIT) が含まれること。
- 既存の `keyboard_visuals_*` テストは新しい意味へ置き換える。
- 実行: `cargo test -p music-core`、`cargo test -p mimageviewer --lib ui_music_spectrum::`。

## 7. ライセンス表記

- EffeTune は MIT (Copyright (c) 2025-2026, Yoshiyuki Kobayashi)。単体 exe 版・インストーラ版は
  バージョン情報の EffeTune notices に MIT 本文が既に出ている。
- **ポータブル版は EffeTune の notices を出していない** (`src/ui_dialogs/about.rs` の
  `#[cfg(not(feature = "portable"))]`)。HarmNet は core に入るので全配布形態に含まれる。
  `third_party/effetune-harmnet/` に EffeTune の LICENSE 本文 (MIT) と
  `harmnet_weights.provenance.json` (学習データの出所を含む) を置き、バージョン情報に
  「EffeTune Note Spectrogram (HarmNet L) — MIT」として **cfg なしで**表示する。
  installer / portable の同梱物の扱いは既存の LibRaw 等の notice と同じ形に揃える。

## 8. 性能

- spectrum worker 上で動き、UI スレッドには何も足さない。
- 目標: 1 フレーム 1ms 以下 (再生中は 50 フレーム/秒なので 1 コアの 5% 以下)。pitch-lab の scalar
  試作は約 2ms で、層ごとの Vec 確保と境界検査を除けば届く見込み。届かなくても動作はする
  (再生中 10% 程度) ので、目標未達は記録して実機判断とする。
- 計測用に release の小さな bench (example か `#[ignore]` テスト) を残す。1 フレームの推論だけでなく、
  初期化 (重みの読み込み・FFT 計画)、助走 1 回 (58 フレーム + PCM コピー)、通常の要求 1 回
  (棒グラフの FFT + 1 フレーム) の時間を出す。

## 9. 文書の更新

- `docs/music-integration-plan.md` §5.6 / §7.2: 鍵盤の中央が HarmNet の推定になったこと、worker が
  状態を持つこと、助走。
- `docs/architecture-overview.md`: music-core に pitch モジュールが加わったこと。
- `docs/async-architecture.md`: spectrum worker が HarmNet の状態を持ち、PCM の同一性で継続を
  判断すること。
- EffeTune 関連の文書 (`docs/effetune-integration-plan.md` 等) で、ポータブル版に同梱しないのは
  Mixwright bundle であり、HarmNet の表記は全形態に出ることを区別して書く。
- `htdocs/mimageviewer/manual/music.html` (145 行目付近) と `tut-music.html` (99〜105 行目付近):
  「鳴っている音の基音を推定して光る」「上下のグラデーションは倍音の多さ」。内部用語
  (ニューラルネット、GRU、HarmNet) は書かない。
- CHANGELOG はリリース時に公開担当が書く。

## 10. 対象外

- Bass ▼ マーカーの推定方式の変更 (HarmNet の最低音を使う案は実機で見てから判断)。
- 上の棒グラフ、タイムラインの解析、mIV Remote、`tools/music_lab`。
- EffeTune の 1/60 オクターブ表示 (セント単位の位置) と音量バー。
- pitch-lab の調査用 example (`keyboard_dump.rs`、`examples/pitch_lab/`) は master へ持ち込まない。
  必要なのは重み変換スクリプトと fixture 生成スクリプトだけ。

- **既存の不一致 (今回は直さない)**: parked-live の音楽ウィンドウ
  (`src/ui_fullscreen.rs` の `draw_parked_live_music_window`) は `draw(..., 0.0, ...)` で Norm の
  ゲインを渡しておらず、通常表示と鍵盤の明るさが違う (Sol P4)。今の presence も同じ式なので、
  この設計で新しく生じる差ではない。detached 経路に触れる修正は凍結ルールの手続きが要るため、
  バックログへ別件として起票する。

## 11. 永続データ

なし。`SpectrumAnalysis` は永続化されていない (D8)。設定項目も足さない。

## 12. 独立レビューの記録

1 回目 (GPT-6.1 Sol / xhigh、2026-10-12、session `01a12f97-de07-7290-b8e6-a4095800d4bd`):
ACCEPT WITH CHANGES。

| 指摘 | 対応 |
| --- | --- |
| P1 確定 EOF と未デコードを同じ扱いにすると末尾が空になる | §4.2: 完了後は末尾より後を 0 埋め。§6 に短いファイル・端数長・末尾の持続音 |
| P2 一時停止中に確率だけ未取得だと再要求されない | §4.4: 「確率 None かつデコード未完了」を再要求条件に追加。進捗通知は足さない |
| P3 未取得と有効なゼロ確率の区別、残光との関係 | §4.3: `Option`。§4.4: 未取得は入力 0 として残光で減衰 (ベースライン専用描画は足さない)。§6 で瞬時値と残光を分けて検査 |
| P4 parked-live が Norm ゲインを渡していない | 既存の不一致でこの設計が生むものではない。detached 凍結ルールの対象なので §10 に記録し別件で起票 |
| P5 golden の時刻対応と助走の合格条件 | §5.1 に確認済みの対応を記載、§6 に境界・助走ケースと数値許容差の記録 |
| P6 所有の前提・文書・検査 | §3 に App 全体 1 つの前提と PCM 同一性による継続判断。App レベルの park/復帰テストは、継続を PCM 同一性で判定するため tracker 単体の同一性テストで代える。§9 に async-architecture と EffeTune 文書。§6 にライセンス表記の検査、§8 に要求全体の計測 |

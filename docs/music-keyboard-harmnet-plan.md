# 音楽ビュー鍵盤の基音推定 (HarmNet 移植) 設計

状態: 実装済み・独立コードレビュー ACCEPT。portable check / 本体 release 計測 / 全体ゲート / 検証用 build は環境・成果物の前提待ち (2026-10-12、助走の合格条件は設計担当の追加判断で確定)。実装前の独立レビュー (GPT-6.1 Sol /
xhigh) 1 回目 = ACCEPT WITH CHANGES、指摘は §12 のとおり反映。
作業ブランチ: `pitch-lab` (worktree `G:\home\mimageviewer-pitchlab`)。

## 1. 目的と背景

音楽ビュー下段のピアノ鍵盤 (`src/ui_music_spectrum.rs` の `draw_pitch_keyboard`) は、各半音帯域の
**スペクトルエネルギー**を表示している。利用者の報告では EffeTune v2.13.0 の Note Spectrogram と比べて
「半音上などが間違えて選ばれるケースが多い」。2026-10-12 の調査 (pitch-lab ブランチの
`crates/music-core/examples/`、当時の手順書は `examples/pitch_lab/README.md`、調査用ファイルは
§10 に従って整理済み) で次を確認した。

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
  で求め、48kHz 以外を渡されたら**有効な全 0** (`Some`) を返す (assert で落とさない。`None` を
  返すと §4.4 の再要求が止まらなくなるため)。
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
  テストでこの値を検査する。変換スクリプトは `third_party/effetune-harmnet/convert_weights.py`。
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
持つ (`PitchTrackerCursor` 相当: `Arc<MusicPcm>` の同一性 (`Arc::ptr_eq`) + 実計算範囲
`[first_frame, next_frame)`)。カーソルは実際に 1 フレーム計算した後にのみ公開する。
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
  再要求されない (Sol P2)。条件に **「直近の結果の確率が `None`」** を 1 つ足す (16ms 間隔・
  in-flight 1 件の既存の間引きに乗る)。デコード完了の有無は条件に入れない (完了を `update` が先に
  観測しても、保存済みの `None` は次の要求を処理するまで更新されないため。Sol 2 回目 P2)。
  完了後の要求は P1 により `Some` を返すので自然に止まる。間引きで送信を見送ったときも、
  確率が `None` の間は `request_repaint_after(SPECTRUM_REFRESH_INTERVAL)` で次の機会を予約する。
  進捗通知チャネルや完了検出用のフィールドは足さない。
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
  Reuse         : 同じ PCM で、必要な助走範囲を含み、計算済みの最終フレーム == k_end
  Advance(n)    : 同じ PCM で、必要な助走範囲を含み、
                  0 < k_end − 最終フレーム ≤ MAX_ADVANCE_FRAMES
  Restart(from) : それ以外 (別の PCM / 後ろへ戻った / 先へ飛びすぎ)。
                  from = max(1, k_end − WARMUP_FRAMES)
```

- `WARMUP_FRAMES` = 58 (1 秒 + 先読み 8 フレーム)、`MAX_ADVANCE_FRAMES` = `WARMUP_FRAMES`
  (それ以上先へ進むなら作り直したほうが安い)。
- 「必要な助走範囲を含む」は `first_frame <= max(1, k_end - WARMUP_FRAMES)`。
  デコード先端までしか処理できなかった要求の後で位置が戻ると、最終計算フレームが新要求の
  `k_end` に届いていても、新要求に必要な前史を持たないことがある。この場合も Restart する。
  元の未取得要求への再要求はこの条件を満たし、計算できた先端から Advance する。
- 一時停止中は再生位置が変わらない限り要求が来ない (既存の `update` の条件)。小刻みに戻すシークは
  毎回 Restart になるが、要求は 16ms 間隔に間引かれ in-flight は 1 件なので許容する。
- **Restart と Advance の結果が同じになるとは限らない** (GRU の状態が違う)。連続計算もそれまでの
  曲の流れに依存する推定値であり、正解ではない。合格条件は正解の音との一致 (§6) とする。
  連続計算との最大確率差は参考値として記録し、合否に使わない。
- **助走は 58 のまま** (設計担当の追加判断、2026-10-12)。200 区間へ延長するとシーク、特に
  一時停止中の繰り返しシークごとに約 0.17 秒の推論費用を生み、利用者が承認した
  「表示位置の約 1 秒前から助走」と食い違うため採らない。

#### 実装時の助走測定と合格条件の確定 (2026-10-12)

`tests/harmnet.rs` は絶対フレームの同じ PCM 窓を連続計算と reset 後の計算に渡し、head 3 の
88 鍵の最大絶対差を測る。`from = k_end - 58` を両端含めて処理するため、最初の投入は
59 フレーム (58 hop 区間) となる。以下は初回調査における末尾 8 要求の最大値。
連続計算と Restart の差を 0.05 以下にするという当初の目安は、設計担当の追加判断で撤回した。
比較するのは信号の定義から分かる正解の音で、各ケースの末尾要求の確率 0.5 以上の鍵集合が
正解と一致することを連続計算と Restart の両方で検査する。和音切替直後など連続計算でも
一致しない瞬間は、切替から十分後の位置で検査する。最大差は引き続き参考出力として残す。

| 信号 | 4 秒信号、58 区間 | 前史を 4 秒延長、58 区間 | 延長信号、100 区間 | 延長信号、150 区間 | 延長信号、200 区間 | 延長信号、300 区間 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 持続音 A3、8 倍音 | 0.062645771 | 0.074530497 | 0.056932539 | 0.044586480 | 0.034883343 | 0.021358168 |
| C4/E4/G4 → D4/F4/A4、6 倍音 | 0.127416074 | 0.213090032 | 0.113369733 | 0.067401230 | 0.033543557 | 0.004748879 |
| A3 オンセット | 0.006171760 | 0.011413950 | 0.007891470 | 0.005196421 | 0.003242029 | 0.000773360 |
| E2 消音後の再発音、10 倍音 | 0.012870982 | 0.007909354 | 0.004629370 | 0.002814675 | 0.000749758 | 0.004027961 |

信号式は fixture と同じ絶対サンプル位相・振幅 0.1・両端 240 サンプル線形 fade。
4 秒版では和音切替/再発音 2.6 秒、オンセット 2.5 秒、消音 1.8 秒、`k_end` は
持続音/オンセット 150、和音/再発音 155。延長版はそれぞれ +4 秒、`k_end` +200。
持続音/和音の先行区間は 0 秒から延長する。200 区間は今回の 4 ケースで差 0.05 以下だが、
GRU の忘却に有限時間の保証はなく、再発音の差も単調ではない。普遍的保証とは扱わない。
独立レビューでも時刻・age の取り違えや移植ミスは見つからず、GRU の履歴の違いによる差と判断した。
この差だけを基音推定の正誤や助走延長の理由とは扱わない。

最終助走テストは 4 秒信号と前史を 4 秒延長した 8 秒信号の計 8 ケースを、表示対象時刻
3.42 / 7.42 秒 (`k_end = 180 / 380`、`from = 122 / 322`) で検査する。
和音切替・再発音から 0.82 秒後、オンセットから 0.92 秒後となる。
和音には 6 倍音・振幅 `0.1 × h^-2` を使う。初回の 4 秒版 `h^-1.3` 和音は連続推論自身が
正解の `[62,65,69]` に倍音鍵 `[77,81]` を加えるため、連続推論も正解となるケースを選ぶという
合格条件に従って助走テストの信号を選定した。上の初回測定表と Python golden の元の
`h^-1.3` fixture は維持する。推論コードの制限や確率のしきい値変更で誤鍵を消してはいない。

| 最終テストの信号 | 正解の MIDI 鍵集合 | 4 秒版の参考最大差 | 8 秒版の参考最大差 |
| --- | --- | ---: | ---: |
| 持続音 A3、8 倍音 | `[57]` | 0.064637609 | 0.074526176 |
| C4/E4/G4 → D4/F4/A4、6 倍音 (`h^-2`) | `[62,65,69]` | 0.048666891 | 0.015140098 |
| A3 オンセット | `[57]` | 0.023712359 | 0.029815186 |
| E2 消音後の再発音、10 倍音 | `[40]` | 0.118740574 | 0.199600309 |

全 8 ケースで連続計算と Restart の確率 0.5 以上の鍵集合がそれぞれ正解と一致した。
最大差の許容値は設定しない。上表は各ケースの最終要求の 88 鍵における最大絶対差で、
初回の末尾 8 要求の最大値とは測定位置・集約範囲・和音の信号定義が異なる。

### 5.3 デコードが追いついていないとき

`copy_range` で取れたサンプルまでしか計算しない。`k_end` まで届かなければ、その要求の
`pitch_probabilities` は空 (鍵盤の中央は暗い)。カーソルは計算できたところまで進める。
Restart は旧カーソルを消し、0 フレーム投入ならカーソル無しを維持する。開始予定位置を
計算済みとして公開しない。Advance で追加投入できない場合は既存の実計算範囲を維持する。

## 6. テスト

- **重み**: 埋め込みバイト列の長さ 188,048 と SHA-256 一致。
- **golden**: Python `effetune==0.13.0` で作った短い fixture (合成音、2〜3 秒、オンセット・無音・
  和音を含み、**有音→無音と無音→有音の両方の境界を age 8 の範囲にまたがって含む**) の age 0 と
  age 8 の出力を `crates/music-core/tests/fixtures/` に小さなバイナリで保存し、
  Rust の出力が 1e-3 以内で一致すること。fixture の生成スクリプトも同じ場所に置く (Python 依存は
  テスト実行には不要)。
- **判定関数**: `decide` の Reuse / Advance / Restart (別 PCM・後退・飛びすぎ) の状態遷移テスト。
- **助走**: 持続音・和音の切り替え・オンセット・無音後の再発音それぞれの途中へ Restart したとき、
  各ケースの末尾要求で確率 **0.5 以上の鍵の集合が正解の音と一致すること**を、連続計算と
  Restart の両方で検査する。正解は信号の定義から求める。連続計算でも正解と一致しない
  切替直後などは避け、対象時刻を切替から十分後 (0.3 秒以上) に置く。連続計算との最大差は
  テスト出力と本書へ参考値として記録し、合否に使わない。
- **範囲**: 180ms 未満の短いファイル、hop で割り切れない長さ、末尾の持続音で、デコード完了後は
  末尾まで確率が `Some` になること (P1)。
- **再要求**: 「一時停止・位置固定・bands あり・確率 `None`」→ PCM 追記 / 完了 → 次の要求で確率が
  `Some` になること (P2、`MusicSpectrumState::update` の handler-level テスト)。**`None` を受け取った後、
  次の `update` より前に `mark_complete()` する順序**も固定して検査する (2 回目 P2)。
- **PCM の同一性**: 同じ位置でも別の `MusicPcm` なら Restart になること。
- **未デコード**: デコード済み範囲の外を要求したとき空の結果になり、カーソルが計算できたところに
  あること。
- **部分助走後のシーク**: Restart が 0 / 1 / 9 フレームしか投入できなかった後で先端へ戻った場合、
  新しい表示位置の助走範囲を持たない状態を Reuse/Advance しないこと。未取得なら `None` を維持し、
  追記後は `Some`。取得済みなら新位置での fresh Restart と一致すること。旧カーソルがあった場合も
  0 投入 Restart で消えること。
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

最終 music-core release 計測 (2026-10-12、`cargo run --release -p music-core --example harmnet_dump`、
exit 0): 初期化 cold 11.549ms、同プロセスでの初期化平均 9.407ms、前処理＋1フレーム推論平均
0.783ms (500 回)、58フレーム推論のみ 47.017ms (10 回平均)。1ms 目標を満たす。
PCM コピーを含む助走と通常要求全体は `ui_music_spectrum::tests::harmnet_worker_benchmark`
(`--release --ignored --nocapture`) で測る。本体 Cargo ビルドの native 依存障害を切り分ける間に、
同じ `src/ui_music_spectrum.rs` を直接 `rustc --test -C opt-level=3 -C linker=rust-lld.exe`
でコンパイルし、release の music-core とリンクして測定した (egui は既存 debug rlib、
計測区間に描画は含まない)。59 フレーム助走＋PCM コピー 62.907ms、通常要求
(両 PCM コピー＋棒グラフ FFT＋1 pitch フレーム) 2.036ms、通常要求 100 回平均 2.291ms。
上の直接 rustc の値は部分助走カーソル修正前の初回測定で、最終コードの本体 Cargo release 計測とは
区別する。最終コードで `cargo test --release -p mimageviewer --lib
ui_music_spectrum::tests::harmnet_worker_benchmark -- --ignored --nocapture` を試みたが、
`turbojpeg-sys` の CMake/MSBuild 段で失敗 (exit 101)、bench 自体は未実行。環境を変える回避策や
再試行は行っていない。200 区間への延長費用は初回の 201 × 0.859 ≈ 173ms という推論のみの
概算であり、実測ではない。§5.2 の判断により 200 区間は採らない。

golden 最大差: age 0 = 0.000000782、age 8 = 0.000000358 (合格基準 0.001)。
初回 push を含む 12 フレーム・probabilities・reset・無音 push の allocation 実測は 0。

直接 `rustc --test` による spectrum モジュールの headless 検査は 24 成功・0 失敗・
bench 1 ignored (exit 0)。本体の Cargo テスト成功を代替しない。

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
ACCEPT WITH CHANGES。2 回目 (同セッション): 残る P1 なし、P2 1 件を反映すれば実装へ進めてよい。
P4 の別件化、P6 の単体テストでの代替 (既存の park／復帰・音声トラック変更テストは維持) も了承。

| 指摘 | 対応 |
| --- | --- |
| P1 確定 EOF と未デコードを同じ扱いにすると末尾が空になる | §4.2: 完了後は末尾より後を 0 埋め。§6 に短いファイル・端数長・末尾の持続音 |
| P2 一時停止中に確率だけ未取得だと再要求されない | §4.4: 「確率 None」を再要求条件に追加 (2 回目 P2 の追補を含む)。進捗通知は足さない |
| P3 未取得と有効なゼロ確率の区別、残光との関係 | §4.3: `Option`。§4.4: 未取得は入力 0 として残光で減衰 (ベースライン専用描画は足さない)。§6 で瞬時値と残光を分けて検査 |
| P4 parked-live が Norm ゲインを渡していない | 既存の不一致でこの設計が生むものではない。detached 凍結ルールの対象なので §10 に記録し別件で起票 |
| P5 golden の時刻対応と助走の合格条件 | §5.1 に確認済みの対応を記載、§6 に境界・助走ケースと数値許容差の記録 |
| 2 回目 P2 完了を先に観測すると None のまま再要求されない | §4.4: 再要求条件を「確率 None」だけに。48kHz 以外は有効な全 0 を返す (§4.1)。§6 に順序固定テスト |
| P6 所有の前提・文書・検査 | §3 に App 全体 1 つの前提と PCM 同一性による継続判断。App レベルの park/復帰テストは、継続を PCM 同一性で判定するため tracker 単体の同一性テストで代える。§9 に async-architecture と EffeTune 文書。§6 にライセンス表記の検査、§8 に要求全体の計測 |

### 最終検証記録 (2026-10-12、pitch-lab の未コミット差分)

助走の合格条件は設計担当の追加判断 (§5.2) で確定。独立実装レビュー (GPT-6.1 Sol / xhigh) は
最終差分を ACCEPT、新 P1/P2 なし。連続計算との差 0.05 という旧条件に関する初回指摘は撤回済み。

最終確認で部分助走カーソルの所有境界を追加修正した。Restart の開始予定位置を計算済みとして
公開すると 0 投入時に誤った Reuse となり、実際に少数投入した場合も後退した新要求の前史を
持たない状態を再利用し得た。カーソルを実計算範囲 `[first_frame,next_frame)` として push 後にのみ
公開し、要求の助走範囲を含むときだけ Reuse/Advance する。独立レビューはこれを既承認の
助走・None 契約を成立させる範囲内の構造修正と合意した。所有者・助走長・結果チャネルは変更なし。
0 / 1 / 9 投入後の取得済み／末尾 1 サンプル未取得、追記、fresh Restart との一致、旧カーソル破棄の
回帰検査を追加。レビュー対象 `src/ui_music_spectrum.rs` の hash は
`b686311df56f69ea58950bccbec51491db507999`。

| コマンド | exit / 結果 |
| --- | --- |
| `cargo fmt` | 0 |
| `cargo fmt --check` | 0 |
| `cargo test -p music-core` | 0: lib 21、HarmNet 6、allocation 1 の計 28 成功、失敗 0。doc test 0 件 |
| `cargo test -p mimageviewer --lib ui_music_spectrum::` | 0: 26 成功、失敗 0、bench 1 ignored |
| `cargo test -p mimageviewer --lib ui_dialogs::about::tests::` | 0: 4 成功 (HarmNet MIT 表記を含む) |
| `cargo check -p mimageviewer --features portable` | 101: `turbojpeg-sys` の CMake/MSBuild ビルドが exit 1。native 前提の失敗で check 未完了。回避策・再試行なし |
| `cargo test -p mimageviewer --features portable --lib ui_dialogs::about::tests::harmnet_notice` | 0: 1 成功 (portable の HarmNet MIT 表記) |
| `cargo clippy -p music-core --all-targets` | 0、警告 0。追加の `-- -D warnings` も 0 |
| `python scripts/check_ui_glyphs.py` | 0、問題 0 件 |
| `cargo run --release -p music-core --example harmnet_dump` | 0、§8 の最終計測値 |
| 本体 release の `harmnet_worker_benchmark` | 101: `turbojpeg-sys` の CMake/MSBuild ビルド失敗、bench 0 件・未測定 |
| `scripts/test-full.ps1 -SuppressCrashDialogs` | 101: launcher に取り込む release core / remote / EPUB worker の 3 本が未用意でビルド停止。テスト 0 件、全体ゲート未完了 |
| `scripts/build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | 1: `turbojpeg-sys` の CMake/MSBuild ビルド失敗。検証用 core 未作成 |

Cargo は全て `MSBUILDDISABLENODEREUSE=1`。この最終作業では TMP/TEMP を変更せず、target 成果物を
削除していない。release test 実行に備え、存在しなかった FFmpeg DLL 6 本だけを同 worktree の
`target/release/deps/` に実体コピーした。製品実行ファイルの起動・コミット・master 操作はなし。
music-core の成功結果は同一ソースのまま、後続の worker 修正・文書更新では再利用した。

記録は `target/pitch-validation/final/` (`results.json` と各コマンドの raw log)。
検証用 build の `build-dev-summary.txt` はツール出力から転記した要約であり raw log ではない。
§8 の初回直接 rustc headless 検査と `pitch-measurement-summary.txt` は以前の別枠の記録として維持する。
未成立の portable check / release bench / build は設計担当側の環境での実行が必要。
全体ゲートは release 入力 3 本を用意してから再実行する。検収・実アプリの手動確認は未完了。

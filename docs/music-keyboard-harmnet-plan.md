# 音楽ビュー鍵盤の基音推定 (HarmNet 移植) 設計

状態: 実装済み。コミット `555c41be9` の独立コードレビューは ACCEPT WITH CHANGES、回帰検査と記録の追補を §12 に記載。portable check / 本体 release 計測 / 全体ゲート / 検証用 build は環境・成果物の前提待ち (2026-10-12、助走の合格条件は設計担当の追加判断で確定)。実装前の独立レビュー (GPT-6.1 Sol /
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
  テスト実行には不要)。比較前に実測値・期待値の全要素が有限であることを要求し、どちらに
  NaN / ±Inf を混ぜても比較検査が失敗することを回帰検査する。
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
- **worker 結果**: 本番 `run_music_spectrum_worker` の要求・結果チャネルを使う headless 検査で、
  確率の結果への組み込み、同位置での有音／無音 PCM 切替、未取得／完了後の取得、取消後の終了と
  結果非公開を検査する。テスト内で結果へ確率を代入する検査だけで代替しない。
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
  (EffeTune MIT) が含まれること。実際の `show_about_dialog_window` を headless 描画し、一覧名と
  全文への入口、入口をクリックした後の埋め込み LICENSE 全文を検査する。
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

### 初回実装の検証記録 (2026-10-12、コミット前の pitch-lab 差分)

助走の合格条件は設計担当の追加判断 (§5.2) で確定。独立実装レビュー (GPT-6.1 Sol / xhigh) は
最終差分を ACCEPT、新 P1/P2 なし。連続計算との差 0.05 という旧条件に関する初回指摘は撤回済み。

最終確認で部分助走カーソルの所有境界を追加修正した。Restart の開始予定位置を計算済みとして
公開すると 0 投入時に誤った Reuse となり、実際に少数投入した場合も後退した新要求の前史を
持たない状態を再利用し得た。カーソルを実計算範囲 `[first_frame,next_frame)` として push 後にのみ
公開し、要求の助走範囲を含むときだけ Reuse/Advance する。独立レビューはこれを既承認の
助走・None 契約を成立させる範囲内の構造修正と合意した。所有者・助走長・結果チャネルは変更なし。
0 / 1 / 9 投入後の取得済み／末尾 1 サンプル未取得、追記、fresh Restart との一致、旧カーソル破棄の
回帰検査を追加。当時のレビュー記録にあった `b686311df56f69ea58950bccbec51491db507999` は
コミット前の版を指す記録であり、方式・対象版が明記されていなかったため、コミットの検証根拠には
使用しない。今回の別セッション独立レビューの対象版はコミット
`555c41be9d2883c1af10d402cbebff429cff4862`。その `src/ui_music_spectrum.rs` の **Git blob SHA-1** は
`a588ea21a58719549c3de2d0f6184edddfa54fb1` (`git rev-parse 555c41be9:src/ui_music_spectrum.rs`)。
コミット時に設計担当が追加した `PITCH_REFERENCE_DELAY_SECS` 定数もこの対象版に含む。

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

### コミット後の独立レビューへの追補 (2026-10-12、基点 `555c41be9`)

別セッションの GPT-6.1 Sol / xhigh は上記コミットを ACCEPT WITH CHANGES と判定。
ソース照合では DSP 移植、age の無音履歴、部分助走カーソル、PCM 同一性、EOF 補完の
具体的な実装不具合は見つからず、以下の回帰検査・記録修正を要求した。今回の変更は
テストと本書だけで、製品の実行経路・動作は変更していない。追補は未コミット。

| 指摘 | 追補対応 |
| --- | --- |
| P2 golden の `f32::max` が NaN 差分を捨てる | 共通比較関数で両配列の全要素に `is_finite()` を要求。実測側／期待側の先頭・中央・末尾に NaN / ±Inf を混ぜる 18 通りが必ず失敗することを検査 |
| P2 本番 worker の確率代入が検査対象外 | 実際の `run_music_spectrum_worker` にチャネルで要求を送り、受信した確率・同位置の有音／無音／有音 PCM 切替・未取得から完了後の取得・取消終了と結果非公開を検査。結果への確率代入をテスト内で代替しない |
| P3 notice の実際の表示登録が検査対象外 | 隔離された App fixture で `show_about_dialog_window` を実際に描画し、一覧名・全文入口・AccessKit のクリック操作後の埋め込み LICENSE 本文を検査。通常／portable とも cfg なしの同じテストを実行 |
| P3 レビュー対象 hash の不一致 | 初回記録をコミット前の版として区別し、別セッションの対象コミットと Git blob SHA-1 を明記 |

追補の検証対象 `src/ui_music_spectrum.rs` の Git blob SHA-1 は
`bb9ccd7d8f7428a872b10ed798329b011b4e495e` (`git hash-object src/ui_music_spectrum.rs`)。
これは上記コミットに回帰テストだけを足した作業ツリーの版で、別セッションレビューの対象版とは区別する。

| コマンド | exit / 結果 |
| --- | --- |
| `cargo fmt --check` | 0 |
| `cargo test -p music-core` | 0: lib 21、HarmNet 7、allocation 1 の計 29 成功、失敗 0。doc test 0 件 |
| `cargo test -p mimageviewer --lib ui_music_spectrum::` | 0: 27 成功、失敗 0、bench 1 ignored |
| `cargo test -p mimageviewer --lib ui_dialogs::about::tests::` | 0: 5 成功、失敗 0。実際の HarmNet notice 描画／全文展開を含む |
| `cargo test -p mimageviewer --features portable --lib ui_dialogs::about::tests::` | 0: 4 成功、失敗 0。実際の HarmNet notice 描画／全文展開を含む |

Cargo は全て `MSBUILDDISABLENODEREUSE=1`。golden 検査を今回ビルドした music-core の
テスト実行ファイルで `--nocapture` 再実行し、age 0 最大差 0.000000782、age 8 最大差
0.000000358 (exit 0、1 成功) を再確認した。助走長・合格条件・性能の測定対象コードは
変更していないため、§5.2 / §8 の測定値を維持し、今回の性能再計測とは扱わない。
通常版 About の最初の座標クリックによる検査は本文を取得できず失敗したため、同じ実描画の
入口を AccessKit のクリック操作で開く検査へ修正し、通常／portable とも成功した。
全ての指摘へ対応済みで、今回の指定確認は全て exit 0。native ビルド障害は発生していない。
`src/ui_music_spectrum.rs` と `src/ui_dialogs/about.rs` のテストモジュールより前の本番コードが
HEAD と一致することもソース比較で確認した。追補差分の再独立レビュー・検収は未実施。

今回の作業では TMP/TEMP の変更、target 成果物の削除、`target/dev-runtime` の操作、
製品実行ファイルの起動、コミット、master／別 worktree の操作を行っていない。
本体テストの raw log は `target/pitch-validation/{spectrum,about,portable-about}-review.log`、
golden 再実行は `golden-review.log` に保存。music-core 全体テストの結果はツール出力に記録。
初回の portable check / release bench / 全体ゲート / 検証用 build と実アプリ idle 測定の
未成立・未実施は、今回のテスト追補によって成立した扱いにはしない。

## 13. 第 2 段階: タイムラインのベース表示と鍵盤の ▼ (2026-10-12 追加)

状態: 第2段階を実装。実装前レビューは§13.7、実装・検証・計測の記録は§13.8。

### 13.1 背景

利用者の実機確認 (2026-10-12) で、鍵盤は EffeTune に近い表示になり、上下のグラデーション・シーク・
曲末尾は問題なしとの報告。一方、鍵盤上の Bass ▼ がずれて見えるとの報告があった。▼と上の
タイムラインの「Loudness+Bass」レーンの色は、どちらも曲全体の解析 (`apply_chroma_metrics`) が
0.1 秒ごとに作る `WaveformBin.bass_pitch_class` / `bass_pitch_confidence` を使っている。これは
35〜260Hz のエネルギーを 12 音名へ折りたたむ方式で、8192 点窓 (約 5.9Hz/ビン) では低音域の半音を
分けられず、3 倍音 (5 度上) も同じ票に入る。鍵盤だけが HarmNet になったので食い違いが目立つ。
利用者の判断: リリース前に 3 つ (鍵盤・▼・タイムライン) をそろえる。

### 13.2 決定事項

| ID | 決定 |
| --- | --- |
| B1 | タイムラインのベース色は HarmNet の推定から作る。**タイムライン解析とは別枠**で計算し、波形・タイムラインの表示までの時間は変えない |
| B2 | ベース音 = 各 HarmNet フレームで、MIDI 21〜55 (A0〜G3) のうち確率 0.5 以上の**最も低い鍵**。信頼度 = その確率。該当なしは信頼度 0 |
| B3 | 並列計算のスレッド数は **min(4, available_parallelism)** の固定上限。空きコア数などの実行時状態では変えない (CLAUDE メモの「決定性優先」)。スレッドの優先度は Windows で BELOW_NORMAL に下げる (`src/similar_index.rs` に前例あり) |
| B4 | 計算は PCM のデコード完了後に始める。曲を区間 (目安 20 秒 = 1000 フレーム) に分け、各区間の頭で 58 フレームの助走をして並列に処理する。区間の境目で GRU の状態が連続しないことは、鍵盤のシーク後と同じ扱いで許容する |
| B5 | 計算済みの部分から順にタイムラインへ反映する。未計算の部分は「ベースなし」と同じ既存の色 (`timeline_metric_color` の `LoudnessBassRoot` 既定色) で描く。古い方式の色は出さない |
| B6 | 結果は LRU にキャッシュしない (曲を開くたびに計算。3 分の曲で約 2 秒の見込み)。状態を増やさないため。LRU ヒットでも PCM は毎回デコードされる既存動作 (`run_music_analysis` の should_analyze=false 経路) に乗る |
| B7 | 鍵盤の ▼ は**鍵盤と同じ再生中の確率** (`pitch_probabilities`) から B2 の規則で選ぶ。タイムラインの結果の到着を待たない。色は鍵盤と同じ音名の色、残光 (`bass_marker_trail`) は残す。タイムラインの結果とは助走の履歴が違うので完全一致は保証しないが、同じ推定方式なので食い違いはまれ |
| B8 | 旧方式の `bass_pitch_class` / `bass_pitch_confidence` は `TimelineAnalysis` に残す (`tools/music_lab` が使う、計算は軽い) が、本体の表示からは参照しない。調 (Key) の推定は今回変えない |

### 13.3 構成

- **music-core**: `TimelineBassTrack` (HarmNet フレームごとの `{ midi: u8 (0 = なし), confidence: f32 }` と、
  計算済み区間の印) と、PCM の区間 1 つを計算する関数を追加。フレーム `k` (時刻 `k × hop − 0.02 秒`)
  の値は、§5.1 と同じく推論 `k + 8` の head 3 から取る。曲末尾は §4.2 と同じく 0 埋めで先読みする。
  ベース用には推論の後段 (gather / ±2 音の畳み込み / GRU / head) を MIDI 21〜58 付近に限って計算
  してよい (上流も音域指定を持つ。±2 音の畳み込みの端の扱いは上流の `Gather` 範囲に合わせる)。
  削れる時間は実測して記録する。
- **計算の担当**: `run_music_analysis` のワーカーが、デコード完了後にベース計算を始める。区間の
  キュー (atomic の次番号) を最大 4 本のスコープ付きスレッドが取り合い、区間が終わるたびに担当
  ワーカーが **計算済み全体のスナップショット** (`Arc<TimelineBassTrack>`) を作って
  `MusicAnalysisMsg::Bass(Arc<TimelineBassTrack>)` で送る。clone は worker 側で行い、UI スレッドで
  大きな配列を複製しない。キャンセルは既存の解析キャンセルトークンを各区間の各フレームで確認する。
- **UI 側**: `poll_music_analysis` が最新のスナップショットだけを取り込み、`App` の
  `music_bass: Option<Arc<TimelineBassTrack>>` に置く。差し替え時は `music_analysis_version` を
  進め、既存の行の再ラスタ判定にそのまま乗せる (別の版数を作らない)。`clear_music_view_state` と
  解析の作り直しで `music_bass = None`。
- **タイムラインの描画**: ベースを使う 3 か所 (`vote_bass_root_hint`、`timeline_metric_value` の
  `LoudnessBassRoot`、ツールチップの音名) を、bin の時刻から `TimelineBassTrack` を引く形に置き換える。
  ラスタ要求に `Option<Arc<TimelineBassTrack>>` を渡す。投票の重み付け (音量・低域エネルギー) と
  リズム区間ごとの投票は今のまま。

### 13.4 状態の組み合わせを減らす検討

- 計算開始をデコード完了後の 1 点にし、デコード中の追い掛け計算はしない (デコードは解析より速い。
  追い掛けると「デコード先端・区間・キャンセル」の組み合わせが増える)。
- キャッシュしない (B6)。版数は既存の `music_analysis_version` 1 本に集約する。
- 送るのはいつも「計算済み全体」のスナップショットで、UI は最新を置き換えるだけ (部分の結合を
  UI に持たせない)。
- まれな失敗 (スレッド生成の失敗など) は割り切り: 生成できた本数で続け、1 本も無理ならベース計算を
  諦めてログを残す (タイムラインは「ベースなし」の色のまま)。

### 13.5 テスト

- 区間の結果: 区間分割 + 助走で計算したベースが、合成音 (ベース単音・ベース + 和音・無音を挟む) で
  正解の音に一致すること。区間の境目をまたぐ持続音でも一致すること。
- スナップショット: 区間の完了順にかかわらず、計算済みの印と値が正しいこと。キャンセルで途中停止し、
  それ以上送らないこと。
- タイムライン: bass track が `None` / 一部計算済み / 全部計算済みで、ベースの色・投票・ツールチップが
  期待どおりになること (未計算は「ベースなし」の色)。既存の UI スナップショットに影響があれば
  docs/ui-snapshot-policy.md に従って更新し、差分を目視確認する。
- ▼: 確率から最低音を選ぶ規則 (範囲外・しきい値未満・複数候補) のテスト。
- 性能: 2〜3 分の曲と長尺の合成 PCM で、4 スレッド時の所要時間を release で測って記録する。

### 13.6 文書

- `docs/music-integration-plan.md` (タイムラインのベースの出どころ、`MusicAnalysisMsg::Bass`)、
  `docs/async-architecture.md` (解析ワーカー内の並列計算・キャンセル・優先度)。
- マニュアルでタイムラインのベース表示を説明している箇所があれば、「曲を開いた直後は少し遅れて色が
  付く」ことを内部用語なしで書く。

### 13.7 独立レビュー (第 2 段階の設計) と反映

GPT-6.1 Sol / xhigh (session `01a12f97-…`、3 回目): ACCEPT WITH CHANGES。以下を §13.2〜13.5 に優先して適用する
(本節が §13.3〜13.5 の該当記述を上書きする)。

| 指摘 | 反映 |
| --- | --- |
| P1 並列 worker が各自スナップショットを送ると送信順が逆転し、表示が後退し得る | **公開者は 1 つ**: 区間を計算するスレッドは「区間番号 + その区間の値」だけを mpsc で `run_music_analysis` のスレッド (スコープの持ち主) へ送る。持ち主が自分の配列へ書き込み、計算済みの印を立て、スナップショットを作って `MusicAnalysisMsg::Bass` で送る。公開される計算済み範囲は単調に増える。テストで区間の完了順の逆転を固定し、公開ごとの計算済み範囲の単調増加を検査する |
| P2 開始位置と既存メッセージとの順序 | 順序を **デコード成功・取消なしを確認 → (should_analyze なら) `TimelineComplete` を送信 → ベース計算** に固定する。`mark_complete()` はデコード失敗・取消でも呼ばれるので開始条件に使わない。ベース計算は `should_analyze` 分岐の外に置き、LRU ヒットでも実行する。`Bass` の採用は既存の source 一致判定の内側で行う。テスト: 旧曲の Bass の棄却、音声トラック変更、park / 復帰、同じ poll 内の `TimelineComplete` + 複数 `Bass` |
| P3 未計算部分に、リズム区間の投票で計算済み側の色が漏れる | 計算済みの印を描画段階まで渡す。リズム区間の投票は計算済み bin だけで行い、未計算 bin の bass hint は既定値 (「ベースなし」の色) にする。計算済みと未計算の境界がリズム区間の途中に入るテスト |
| P4 ▼ が音名として解釈され、スペクトルでオクターブを選び直している | `draw_bass_marker` の入力を **B2 の共通選択関数が返す MIDI 番号 + 確率** に変え、その鍵へ直接描く。旧解析 (`bass_pitch_class`) からの受け渡し (`src/ui_fullscreen.rs` の通常表示と parked-live の 2 か所) と、`notes` によるオクターブ再選択・sticky を外し、残光だけ残す。表示範囲は MIDI 21〜55。テスト: 端点 21 / 55、同じ音名の複数オクターブ、強い倍音を持つ低音 |
| P5 性能検証に再ラスタ負荷が無い | `poll_music_analysis` は 1 回の drain で Bass を最新 1 つだけ採用し、版数更新を 1 回にまとめる。公開者はスナップショットの送信を最短 250ms 間隔にまとめる (最後の区間の完了時は必ず送る)。4 区間の連続到着と長尺 (60 分級) の合成データで、ラスタ要求の滞留・最新の色が出るまでの時間・保持メモリを測って記録する |
| P6 「食い違いはまれ」の裏付けが無い | 合成音で、音の切り替え・計算区間の境界・しきい値付近について、▼とタイムラインの選択音の不一致率と継続時間を測って記録する。ツールチップの音名はタイムラインの色と同じリズム区間の投票結果を出す (瞬時値は出さない)。bin の時刻から HarmNet フレームを引く丸めは §5.1 と同じ式に固定する。音域を限った推論を採用する場合は、MIDI 21〜55 の確率が全音域版と一致する検査を加える |

**detached 凍結ルールとの関係**: P4 で parked-live の音楽ウィンドウ (`draw_parked_live_music_window`) の
`draw` 呼び出しから旧解析由来の ▼ 引数を外す。これは ▼ のデータの出どころを鍵盤と同じ確率へ移す
機能側の変更で、detached の述語・viewport 経路・所有は変えない (症状パッチではない)。CLAUDE.md の
手続きに従い、実装の独立レビューでこの主張への合意を取り、`docs/detached-rework-plan.md` に記録する。

### 13.8 第2段階の実装・検証記録 (2026-10-12、pitch-lab、未コミット)

全音域推論を採用し、音域を限定する任意の最適化は行っていない。`TimelineBassTrack` は
20秒単位の計算済み印と絶対MIDI／確率を保持し、時刻は鍵盤と同じ共通丸め関数で引く。
末尾の丸めを覆う1フレームを加え、head 3の先読みは0埋めする。
デコード成功→必要な `TimelineComplete`→区間計算の順序、単一公開者・250ms公開間隔・
完了公開・各フレーム取消を実装した。UIの1 drainで最新BassとTimelineをまとめて採用し、
共有の版数を1回進める。ベース結果のLRU保存、UIでの配列cloneや推論は追加していない。

ラスタ結果が計算済み印付きのリズム投票を保持し、textureと一緒に採用される。
tooltipはその表示中のtextureと同じ票を参照するので、旧textureを表示している間も色と
音名が一致する。未計算binには計算済み側の色を広げない。▼は鍵盤と同じ確率から最低音を
共通選択して直接描き、倍音エネルギーによるオクターブ再選択とstickyは除去、残光は保持した。

独立実装レビュー: GPT-6.1 Sol / xhigh、ACCEPT、material findingなし。
parked-liveの変更は旧▼引数の除去に限られ、detached述語／viewport／ownerを変えない
構造的なデータ出どころの変更という§13.7末尾の主張に明示的に合意。
設計側の判断と触れた範囲を `detached-rework-plan.md` §11へ記録した。

**不一致計測の意味**: §13.7 P6は不一致の合格上限を定めていない。B4/B7で許容した
58フレーム助走とGRU履歴の差を、正解付き合成音のraw MIDI選択で測る。
これはタイムラインのリズム投票・▼の残光を含む可視表示の一致率ではない。
B7の「食い違いはまれ」は普遍的保証ではなく、以下の実験条件付き観測に限定する。
特に確率0.5付近へシークした場合にはこの主張を適用しない。
独立レビューもこの測定だけから推論／助走／閾値を変更する必要はないと判断した。

指定コマンドのraw logと集約結果は `target/pitch-stage2-validation/` に保存する。
release core計測は `bass_track_benchmark`、ラスタはheadlessの
`ui_music_timeline::tests::timeline_bass_raster_benchmark`。製品実行ファイルは起動しない。
ラスタ計測は本番cache／workerと16msごとのCPU texture採用を通すが、native GPU presentを含まない。
計算時間・メモリはそれぞれの計測範囲を区別し、core単体値を本体release成功へ読み替えない。

#### 指定確認結果

Cargoは全て `MSBUILDDISABLENODEREUSE=1`。最新ソースのfmt再確認とclippy再確認もexit 0、
clippyの最終ログ (`clippy-final.log`) は警告0。初回clippyのexample警告は修正済み。
`results.json`／`fmt-check.log` のfmtは取消回帰追加前の集約実行で、追加後の最終
`cargo fmt --check` はツールから直接実行してexit 0を確認した（この再確認のraw logは保存していない）。
この出どころと最終clippyの結果は `extra-results.json` に分けて記録する。
本体には既存の警告があり、本体全体を警告0へ変更する作業は行っていない。

| コマンド | exit | 件数／結果 |
| --- | ---: | --- |
| `cargo fmt` | 0 | workspace全体 |
| `cargo fmt --check` | 0 | 追加取消回帰後の最終ソースでも成功 |
| `cargo test -p music-core` | 0 | lib 21、Bass 9、HarmNet 7、allocation 1、計38成功、失敗0、doc test 0 |
| `cargo clippy -p music-core --all-targets` | 0 | 最終警告0 |
| `cargo test -p mimageviewer --lib ui_music_spectrum::` | 0 | 28成功、失敗0、bench 1 ignored |
| `cargo test -p mimageviewer --lib ui_music_timeline::` | 0 | 19成功、失敗0、bench 1 ignored |
| `cargo test -p mimageviewer --lib music` | 0 | 151成功、失敗0、bench 2 ignored |
| `cargo test --test ui_snapshot` | 0 | 162成功、失敗0、既存PNG変更なし |
| `cargo check -p mimageviewer --features portable` | 0 | check完了 |
| `python scripts/check_ui_glyphs.py` | 0 | 危険glyph 0 |
| 新partial snapshotの限定生成 | 0 | 1成功。続くtimeline／musicで通常比較も成功 |
| `cargo run --release -p music-core --example bass_track_benchmark` | 0 | 下記計測 |
| 本体releaseの `timeline_bass_raster_benchmark` | 101 | `turbojpeg-sys v1.2.0` のCMake/MSBuild段がexit 1、bench 0件・未測定 |

新規PNGは `tests/snapshots/timeline_bass_partial_raster.png` (240×80、1,553 bytes) の1枚だけ。
計算済み／未計算の境界を同じリズム区間の途中に置き、ベース色が未計算側へ漏れないことを描く。
目視で紫系の計算済み側と既定色の未計算側を確認した。既存162件のPNGには差分なし。
Appを起動するsnapshotや製品実行ファイルの起動は行っていない。

#### release計測（最終ソース、4 worker、Windows BELOW_NORMAL）

入力は48kHz stereo f32。180秒／3600秒は、20秒の合成音楽信号を繰り返したPCM。
計測は区間キュー・単一結合owner・250ms／完了snapshotを含むcore exampleで、
本体のデコード・`MusicPcm` read lock・UI poll・ラスタ・GPUは含まない。

| PCM尺 | 全区間計算 | 最初の区間 | フレーム／区間 | 公開数 | snapshot clone累計 |
| --- | ---: | ---: | ---: | ---: | ---: |
| 180秒 | 2,756.503ms | 923.196ms | 9,001／10 | 3 | 0.139ms |
| 3600秒 | 41,716.022ms | 854.681ms | 180,001／181 | 47 | 24.498ms |

| PCM尺 | PCM配列payload | Bass配列payload（1個） | mutable＋最新snapshot（2個） |
| --- | ---: | ---: | ---: |
| 180秒 | 69,120,000 bytes | 72,018 bytes | 144,036 bytes |
| 3600秒 | 1,382,400,000 bytes | 1,440,189 bytes | 2,880,378 bytes |

これらのメモリ値は配列payloadの計数であり、プロセス全体やUI／チャネルの全保持量ではない。
音域限定は不採用なので、全音域版との一致検査と削れた時間の計測は該当なし。
§13.2 B6の「3分で約2秒の見込み」はこの合成条件では2.757秒だった。アルゴリズムや閾値は変更していない。

不一致fixtureは42秒。0〜10秒E2（10倍音）、11〜20.05秒G2＋C4/E4、20.05〜30秒D2、
30〜40秒はE2の振幅を0.5秒ごとに変えるsweep、区間間と末尾に無音を置く。
連続推論と、各採取位置でresetして58フレーム助走する推論を、分割trackの選択MIDIと比較する。
切替／区間境界は±0.5秒、seek全体はそれらの窓と30〜40秒sweepだけ（全42秒ではない）。
閾値近傍は各modeのpeak、連続推論のpeak、track信頼度のいずれかが0.35〜0.65に入る採取点。
このためmodeごとの閾値近傍の分母は異なる。

| 比較 | 範囲 | 不一致／採取数 | 率 | 採取不一致時間合計 | 最長連続時間 |
| --- | --- | ---: | ---: | ---: | ---: |
| 連続 | 全42秒 | 1／2100 | 0.0476% | 20ms | 20ms |
| 連続 | 音切替±0.5秒 | 1／253 | 0.3953% | 20ms | 20ms |
| 連続 | 区間境界±0.5秒 | 1／102 | 0.9804% | 20ms | 20ms |
| 連続 | sweep閾値近傍 | 0／4 | 0% | 0 | 0 |
| seek助走 | 指定窓全体 | 23／705 | 3.2624% | 460ms | 420ms |
| seek助走 | 音切替±0.5秒 | 2／253 | 0.7905% | 40ms | 20ms |
| seek助走 | 区間境界±0.5秒 | 0／102 | 0% | 0 | 0 |
| seek助走 | sweep閾値近傍 | 10／17 | 58.8235% | 200ms | 160ms |

**未成立の計測**: 本体releaseラスタbenchはnative依存のビルド失敗で実行できなかった。
4区間連続到着／250ms到着、180秒／60分の4条件を実cache／workerへ流すbenchと、
queue滞留・最新色採用時間・生存snapshot配列量・ラスタpayload上界・16msプロセスメモリ標本の
計装は追加したが、これらの数値は未測定。payload上界を実保持量、16ms標本を瞬間最大の
保証として扱わない。debug計測や直接rustcによる回避は行わない。
使用可能な2〜3分の実曲も未指定のため実曲計測は未実施（180秒合成値で代替成立とは扱わない）。

利用者側で実行する未成立コマンド:

```powershell
$env:MSBUILDDISABLENODEREUSE='1'
cargo test --release -p mimageviewer --lib ui_music_timeline::tests::timeline_bass_raster_benchmark -- --ignored --nocapture
```

実曲を48kHz stereo f32 LEへデコードしたPCMに対しては、同じcore exampleの
`--pcm <PCMパス> --no-compare` で計算時間と配列payloadを取得できる。
TMP/TEMP変更、target削除、nativeビルドの回避／再試行、`target/dev-runtime`操作は行っていない。
作業はpitch-labのみで、master／他worktree／通常設定データへ触れず、コミットしていない。
検証用buildも利用者の明示的禁止に従って行わない。実装の設計逸脱はなし。

変更ファイル: `crates/music-core/{src/bass.rs,src/lib.rs,tests/bass.rs,examples/bass_track_benchmark.rs}`、
`src/{app.rs,app/music_bass.rs,app/tests.rs,ui_music_timeline.rs,ui_music_spectrum.rs,ui_fullscreen.rs,video/seek_strip_wave.rs}`、
`docs/{music-keyboard-harmnet-plan.md,music-integration-plan.md,async-architecture.md,detached-rework-plan.md}`、
`htdocs/mimageviewer/manual/{music.html,tut-music.html}`、上記PNG。seek stripは共通ラスタ関数へ
`None`を追加しただけで、動画側の表示動作は変更しない。CHANGELOG／next-release-backlogは未変更。

### 13.9 再ラスタ負荷の release 計測 (2026-10-12、ClaudeCode が実行、commit 167bd5008)

`cargo test --release -p mimageviewer --lib ui_music_timeline::tests::timeline_bass_raster_benchmark -- --ignored --nocapture`
(exit 0)。4 区間のスナップショットが到着する間に可視 4 行のラスタを要求し、最新 (第 4 区間) の色が
信頼度付きで表示されるまでを測る。判定はコードレビュー P3 に従い「C かつ confidence > 0.06」で、
第 3 区間までのスナップショットでは成立しないことを通常テストで固定済み。

| 曲長 | 到着間隔 | 要求 / 処理 | 最大滞留 | 最大待ち (ms) | 最新色の反映 (ms、最後の到着から) | Bass スナップショット最大 (bytes) | ラスタ payload 上限 (bytes) |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: |
| 180 秒 | 0ms (連続) | 16 / 16 | 16 | 465 | 514 | 288,072 | 15,790,592 |
| 180 秒 | 250ms (設計値) | 16 / 16 | 4 | 104 | 133 | 72,018 | 7,895,296 |
| 60 分 | 0ms (連続) | 16 / 16 | 16 | 492 | 526 | 5,760,756 | 15,790,592 |
| 60 分 | 250ms (設計値) | 16 / 16 | 4 | 105 | 137 | 1,440,189 | 7,895,296 |

設計値の 250ms 間隔では滞留が可視行数 (4) に収まり、最新の色は最後の公開から約 0.14 秒で反映される。
連続到着でも 0.53 秒以内に全要求が処理された。プロセスのメモリ増分 (16ms 標本) は 180 秒で約 1MB
(250ms)、60 分の連続到着ケースの 80MB はテスト用の 60 分 timeline (72MB) の生成を含む。GPU なしの
headless 計測で、実アプリの描画時間は含まない。§13.7 P5 の受入条件を満たす。

## 14. 第 3 段階: タイムラインの調 (Key) 表示 (2026-10-13 追加)

状態: 実装済み。合成音の精度条件の未達 2 例は §14.7 で受け入れ済み (ClaudeCode 判断、利用者方針による)。利用者が方針を承認 (同じリリースに含める、精度の事前検証は簡単な合成音の確認でよい)。
実装前の独立レビューの反映は§14.6、実装・検証記録は§14.7。

### 14.1 背景と利用者の判断

第 2 段階は 2026-10-13 に利用者の実機確認を通過した (未計算・ベースなしの灰色を含む)。
タイムラインの「Key」レーンは今も旧方式で、`analyze_chroma_frame` が 55〜2800Hz の**全 FFT ビンを
その周波数の音名へ足し込んだ** chroma を 0.1 秒ごとに作り、`vote_key_hint` が前後 6 秒を音量・
打楽器性で重み付けして集計し、Krumhansl / Temperley の長調・短調プロファイルとの相関で 24 調から選ぶ。
倍音が混ざり (3 倍音 = 5 度上、5 倍音 = 長 3 度上で短調が同主長調へ寄る)、低音域では隣の音名へも漏れる。
利用者の判断 (2026-10-13): 同じリリースに含める。おまけ的な可視化なので、特定のケースで精度が
落ちることは大きな問題ではない。鍵盤で見えている構成音と調が合っていることを優先する。

### 14.2 決定事項

| ID | 決定 |
| --- | --- |
| C1 | 調の chroma は第 2 段階の HarmNet 計算から作る。各フレームで 88 鍵の基音確率 (head 3) を 12 音名へ足し合わせた値。第 2 段階は既に全音域を推論しているので推論は増えない (§13.8: 音域限定は不採用) |
| C2 | 第 2 段階の結果の型を、ベースと chroma を持つ 1 つのトラックに広げる (名前は実装者が決めてよい。例 `TimelinePitchTrack`)。単一公開者・250ms 間隔・計算済みの印・source 一致判定・版数 1 本は第 2 段階のまま。別のメッセージや別の公開者を作らない |
| C3 | `vote_key_hint` の入力を新しい chroma に置き換える。集計窓 (前後 6 秒)、音量の重み、24 調のプロファイル当てはめ、信頼度の式は変えない。旧 chroma 由来の `bin.key_confidence` による重みと、打楽器性による割り引き (`transient` / `transient_density`) は外してよいかを実装時に合成音で確かめ、外す場合は理由を記録する (HarmNet は打楽器を基音にしないため) |
| C4 | 集計窓の中に未計算のフレームが 1 つでもあれば、調は「未計算」とし、ベースと同じ灰色で描く。ツールチップも出さない。窓の一部だけで推定した値は出さない (計算が進むと値が変わって見えるのを避ける) |
| C5 | Key レーンの「調なし」の既定色 (`timeline_metric_color` の `Key` 既定 = 青緑 104,214,186) もベースと同じ灰色にする (利用者が第 2 段階で黄色に感じた違和感と同じ理由: 音名の色と紛らわしい) |
| C6 | `timeline_metric_value` の `Key` (高さ) が使う `bin.key_confidence` は旧 chroma 由来なので、新しい調の信頼度か音量だけに置き換える。どちらにするかは実装時に見た目の差が小さい方を選び、理由を記録する |
| C7 | `TimelineAnalysis` の旧 chroma 系フィールド (`chroma`、`key_pitch_class`、`key_confidence`) は残す (novelty の計算と `tools/music_lab` が使う)。本体の調の表示からは参照しない |
| C8 | キャッシュしない・LRU に入れない (B6 と同じ)。永続データの変更なし |

### 14.3 テスト

- 合成の和音進行 (倍音の多い音色、各 2 秒程度): 長調 (I-IV-V-I、I-vi-IV-V)、短調 (i-iv-v-i、和声的短音階の V を含むもの)、それぞれ別の主音で。新方式が正しい調を返すこと。旧方式の結果も参考として記録する (合否には使わない)。
- 未計算を含む窓では調が「未計算」になり灰色で描かれること、計算済みになれば調の色になること。
- 単一公開者のスナップショットに chroma が含まれ、公開範囲の単調増加が保たれること (第 2 段階のテストを広げる)。
- 既存の UI スナップショットが変わる場合は docs/ui-snapshot-policy.md に従って更新し、目視確認する。

### 14.4 状態の組み合わせを減らす検討

第 2 段階の計算・公開・採用の経路をそのまま使い、結果の型に chroma を足すだけにする。部分的な窓での
推定 (C4) を出さないことで、「計算途中で調が変わる」状態を作らない。

### 14.5 文書

`docs/music-integration-plan.md` の調の出どころ、マニュアルで Key レーンを説明している箇所があれば
「曲を開いた直後は少し遅れて表示される」ことを内部用語なしで。

### 14.6 追加決定と独立レビュー (第 3 段階の設計) の反映

**C9 (利用者決定 2026-10-14)**: ベースのレーンの**高さ**を、全帯域の音量から**低域 (low_cut_hz = 250Hz 以下) の
音量**に変える。今の高さは `timeline_metric_display_value` の `LoudnessBassRoot` で全帯域の音量
× (0.72 + 0.28 × 信頼度) になっており、ベースがほとんど鳴っていない部分でも大きな灰色の棒になる
(利用者の実機観察)。低域の音量は既存の `loudness_db` と `band_energy[0]` (低域が全体に占めるパワーの割合)
から `loudness_db + 10·log10(band_energy[0])` として求め、既存の `timeline_loudness_value` と同じ正規化に
かける。解析・保存データは変えない。キックのような音程のない低音は灰色の棒として残る (「低域は鳴って
いるがベースの音程は取れない」の意味)。ツールチップの説明 (`Height RMS, color bass root`) も低域の音量に
合わせて直す。既存の UI スナップショットが変わる場合は目視確認して更新する。

GPT-6.1 Sol / xhigh (4 回目): ACCEPT WITH CHANGES。本節が §14.2〜14.3 の該当記述を上書きする。

| 指摘 | 反映 |
| --- | --- |
| P2 C4 の判定範囲 | 集計窓 (全幅 6 秒、左右 3 秒) を PCM の実尺 [0, 曲長] で切り詰め、**実在するフレームだけ**の計算済みを確認する。範囲外 (負の時刻・曲末尾以降) を未計算と数えない。完成確認は音量ゼロ等による重みの省略より前に行う。テスト: 極短ファイル、先頭・末尾、窓内の無音部分だけ未計算、20 秒区間の境界の両側 |
| P2 ツールチップの不一致 | Bass と同じく、Key の推定値と「未計算」の状態をラスタ結果 / 行テクスチャに保持し、hover はその値だけを読む。UI 上での再集計はしない。未計算と計算済みの「調なし」を区別する。旧テクスチャ表示中のテストを足す |
| P2 C3 と C7 の矛盾 | 旧 `bin.key_confidence` の重みは**必ず外す**。`transient` / `transient_density` の割り引きを残すかは実装者に任せる (合成音で比較し、理由を記録) |
| P3 C6 の前提違い | 本番の Key 描画は既に `key_hint.confidence` で高さを決めている (`timeline_metric_value(Key)` は本番で未使用)。今の表示式を新しい hint にそのまま使い、未使用の旧信頼度の計算を整理する。C6 の「音量へ置き換え」は行わない |
| P3 計測と chroma の検査 | 第 2 段階の bench を広げ、実サイズで保持量を数える (60 分で約 1.44MB → 約 10MB の見込み)。全窓が計算済みで Key の集計が実際に走るケースでもラスタの滞留を測る。テスト: MIDI→音名、オクターブの加算、高音域を含む 88 鍵、計算済みの無音、スナップショット内の bass と chroma の計算済み範囲の一致 |

### 14.7 第3段階の実装・検証記録 (2026-10-14、pitch-lab、未コミット)

`TimelineBassTrack` / `TimelineBassSegment` の名前は維持して、各フレームのbassと
12音名chromaを保持する型へ広げた。head 3の同じ88鍵の出力から両方を作り、両配列の長さを
検査してコピーした後で共通の計算済み印を立てる。単一公開者、250ms／完了公開、既存source
判定、1 drainの最新結果採用、共通版数1本、LRU非保存はそのまま。

KeyはPCMの実尺へ切り詰めた全幅6秒・左右3秒の窓の全フレーム完成を、音量等の重み省略より
前に確認する。未計算は`None`、計算済みの調なしは`Some(default)`としてラスタ結果から
row textureまで保持し、hoverは表示中の推定と未計算状態だけを読む。旧binのchromaと
`key_confidence`は本体のKey表示に使わない。Key高さは§14.6どおり既存のhint信頼度式を維持し、
未使用の旧信頼度計算を除去した。調なしの色はベースと同じ灰色。

C9のベース高さは`loudness_db + 10·log10(band_energy[0])`を既存音量正規化へ渡し、
低域エネルギー0は高さ0。推定の信頼度は高さへ掛けない。ベースの色／リズム投票の重みは維持し、
音程を推定できない低音は灰色で残る。説明文とマニュアルも低域音量へ変更した。
detached／parked-live、旧解析の保存形、音名の`key_color`は変更していない。

UI応答性§4の点検: 新しいI/O・解析・配列cloneはUIへ追加していない。推論は既存の区間worker、
調の集計と窓完成確認は既存ラスタworker。UIの追加処理は表示中の行のhint参照だけ。
texture uploadの既存1frame予算、取消・source判定も維持した。
状態の簡素化は§14.4のとおり既存経路と1つの完成判定を使い、別公開者・版数・回復機構を足さない。

独立静的レビュー (GPT-6.1 Sol / xhigh): 実装のP1/P2なし。合成音の本番6秒窓と全8秒の
切り分け結果を区別し、全8秒の合格を本番窓の合格へ読み替えないことを確認した。
検証記録の保存先は`target/pitch-stage3-validation/`。検収は未完了。

#### C3と合成和音の切り分け

各和音2秒、10倍音・振幅`0.07·h^-1.3`・両端5ms線形fade、MIDI48からの和音を
主音C／Dへ移調した8ケースを使用する。I-IV-V-I、I-vi-IV-V、i-iv-v-i、i-iv-V-i。
本番と同じ全幅6秒の中央窓（1〜7秒）と、診断用の進行全体8秒を分けて記録する。

旧`bin.key_confidence`の重みは必ず除去した。`transient` / `transient_density`の割り引きは
**維持**する。割り引きあり／なしで8秒全体は8/8正解、本番6秒は6/8正解で同じ判定になり、
除去による精度改善は確認できない。既存の過渡音の重みを維持し、変更をchromaの出どころと
旧信頼度除去へ限定する。信頼度値の差はraw logに残す。

| 進行／期待する調 | 本番6秒・新方式 | 本番6秒・旧方式（参考） | 全8秒・新方式（診断） | 全8秒・旧方式（参考） |
| --- | --- | --- | --- | --- |
| I-IV-V-I / C長調 | C長調 | G長調 | C長調 | G長調 |
| I-IV-V-I / D長調 | D長調 | D長調 | D長調 | D長調 |
| I-vi-IV-V / C長調 | A短調 | C長調 | C長調 | C長調 |
| I-vi-IV-V / D長調 | B短調 | D長調 | D長調 | D長調 |
| i-iv-v-i / C短調 | C短調 | C短調 | C短調 | C短調 |
| i-iv-v-i / D短調 | D短調 | D短調 | D短調 | D短調 |
| i-iv-V-i / C短調 | C短調 | C短調 | C短調 | C短調 |
| i-iv-V-i / D短調 | D短調 | D短調 | D短調 | D短調 |

**設計判断が必要な未達**: §14.3の本番窓で全ケース正しい調という条件は未達。
中央6秒はIとVが各1秒、viとIVが各2秒になり、C長調の理想0/1 chromaでも
現プロファイルのscoreはC長調0.849110／A短調0.837088と僅差になる。
HarmNet確率・音量重みで順位が逆転するが、静的レビューでは入力の取り違え等の実装不具合は
見つからなかった。窓・プロファイル・信頼度式は承認済み仕様のまま変更していない。
fixtureの調整や全8秒だけの検査で合格に置き換えず、本番窓の不一致をテスト失敗として保持する。
→ §14.7 で、§14.1 の精度低下許容方針によりこの 2 例を受け入れた。

#### 指定確認結果

Cargoは全て`MSBUILDDISABLENODEREUSE=1`。raw logと各exit codeは
`target/pitch-stage3-validation/final/`（`results.json`、`base-commit.txt`、
`source-blobs.txt`はbass/core・app publisher・timelineの順）に保存。
対象はこのworktreeの未コミット差分。最終フォント対応の前後は下記の限定確認として区別する。

| コマンド | exit | 件数／結果 |
| --- | ---: | --- |
| `cargo fmt` | 0 | workspace全体、対象外ファイルの差分なし |
| `cargo fmt --check` | 0 | 成功 |
| `cargo test -p music-core` | 0 | lib21、bass12、HarmNet7、allocation1、計41成功。失敗0、doc test0 |
| `cargo clippy -p music-core --all-targets` | 0 | 警告0 |
| `cargo test -p mimageviewer --lib ui_music_spectrum::` | 0 | 28成功、失敗0、bench1 ignored |
| `cargo test -p mimageviewer --lib ui_music_timeline:: -- --nocapture` | 101 | 25成功、合成和音の精度1失敗、bench1 ignored |
| `cargo test -p mimageviewer --lib music` | 101 | 157成功、同じ精度1失敗、bench2 ignored |
| `cargo test --test ui_snapshot` | 0 | 162成功、失敗0。integrationの既存PNG差分なし |
| `cargo check -p mimageviewer --features portable` | 0 | check完了 |
| `python scripts/check_ui_glyphs.py` | 0 | 危険glyph0 |
| 限定snapshot生成 (`UPDATE_SNAPSHOTS=1`、lib `raster_snapshot`) | 0 | 2成功。続くtimeline/musicで通常比較も成功 |

本体には既存警告がある（lib test 102件）。今回の作業で無関係な警告整理は行っていない。
全て成功したとは扱わず、同一の精度失敗を両フィルタが検出した結果として記録する。

PNGは`timeline_bass_partial_raster.png`を更新（240×80、1544 bytes）、
`timeline_key_states_raster.png`を新規追加（660×80、7792 bytes）。更新前後と新規PNGを目視確認し、
変更理由を`docs/ui-snapshot-policy.md`へ記録した。上段波形・未計算側へのベース色漏れ防止を維持し、
Keyの未計算／調なしの灰色とC長調の色を確認。その他の既存PNGは変更しない。

master／他worktree、通常設定、`target/dev-runtime`、製品起動、コミットには触れていない。
検証用buildは利用者の明示的禁止に従って行わない。`CHANGELOG.md`と
`docs/next-release-backlog.md`も未変更。実装の設計逸脱はなし、精度条件の検収は未完了。

#### release計測

coreは`cargo run --release -p music-core --example bass_track_benchmark -- --no-compare`
（exit0）。48kHz stereo f32、20秒の合成音楽信号の繰り返し、4 worker、Windows
BELOW_NORMAL、20秒区間と単一結合owner、250ms／完了公開を含む。
本体のデコード・PCM read lock・UI poll・ラスタ・GPUは含まない。

| PCM尺 | 全区間計算 | 最初の区間 | フレーム／区間 | 公開数 | snapshot clone累計 |
| --- | ---: | ---: | ---: | ---: | ---: |
| 180秒 | 1941.340ms | 625.464ms | 9001／10 | 3 | 0.444ms |
| 60分 | 37077.963ms | 627.665ms | 180001／181 | 46 | 150.836ms |

| PCM尺 | PCM配列payload | track配列割当量1個 | mutable＋最新snapshot | 置換時のmutable＋旧／新snapshot最大 |
| --- | ---: | ---: | ---: | ---: |
| 180秒 | 69120000 bytes | 504066 bytes | 1008132 bytes | 1512198 bytes |
| 60分 | 1382400000 bytes | 10080237 bytes | 20160474 bytes | 30240711 bytes |

配列割当量は実capacity×`size_of`（bass1フレーム8 bytes、chroma48 bytes、計算済み印1 bytes）。
理論上の未padding値や旧bass-onlyサイズではない。小さいtrack構造体、allocator管理領域、
worker／キュー内の区間配列、HarmNet内部、プロセス全体はこの表の範囲外。
本体ラスタbenchの保持量は生存snapshotの同じ`storage_bytes()`を使い、
ラスタ／textureのpayload上界・16msプロセスメモリ標本とは区別する。
全窓計算済みでKey集計を実行する`full_key=true`を、180秒／60分・0ms／250ms到着の
各条件へ追加した。最新版のC長調・非ゼロ信頼度も確認する。
raw logは`target/pitch-stage3-validation/release/`。

本体releaseラスタbenchもexit0、1成功。releaseビルドは32分57秒、benchは5.15秒。
4可視行、4版のsnapshot、実cache／FIFO raster worker、16msごとに1行のCPU texture採用を通す。
`full_key=true`は集計負荷を確認する人工的な完全snapshot4版で、実際の区間公開の再現とは分ける。
推論・PCM・デコード・native GPU upload/present・実アプリ描画はこの計測に含まない。

| PCM尺 | 到着間隔 | 全窓計算済み | 要求／処理 | 最大滞留 | 最大queue待ち | 最新色反映（最後の到着から） | 生存snapshot配列最大 |
| --- | ---: | --- | --- | ---: | ---: | ---: | ---: |
| 180秒 | 0ms | いいえ | 16／16 | 16 | 275.013ms | 296.197ms | 2016264 bytes |
| 180秒 | 0ms | はい | 16／16 | 16 | 400.232ms | 430.485ms | 2016264 bytes |
| 180秒 | 250ms | いいえ | 16／16 | 4 | 71.041ms | 93.333ms | 504066 bytes |
| 180秒 | 250ms | はい | 16／16 | 4 | 81.508ms | 118.525ms | 504066 bytes |
| 60分 | 0ms | いいえ | 16／16 | 16 | 294.272ms | 319.334ms | 40320948 bytes |
| 60分 | 0ms | はい | 16／16 | 16 | 506.605ms | 543.254ms | 40320948 bytes |
| 60分 | 250ms | いいえ | 16／16 | 4 | 79.074ms | 100.757ms | 10080237 bytes |
| 60分 | 250ms | はい | 16／16 | 4 | 108.049ms | 129.703ms | 10080237 bytes |

ラスタ／texture payload上界は0ms到着の各条件16174720 bytes、250msの各条件8087360 bytes。
この上界は未処理の全要求／結果に完全imageとhintがあると仮定したもので、実保持量ではない。
16msプロセスメモリ標本は瞬間最大の保証ではなく、allocator再利用とケース実行順の影響を受ける。
privateメモリ増分は180秒の各条件0〜19910656 bytes、60分の各条件14315520〜128385024 bytes。
後者の最大には試験用の72MB timeline生成を含み、Keyの追加配列だけの増分と読み替えない。
本体releaseのnative依存停止は発生しなかった。実曲・native GPUの計測は未実施。

#### 最終snapshotフォント対応と証拠の再利用

新規比較PNGのラベルにも本体フォントを適用するため、`timeline_key_states_raster_snapshot`の
fixtureだけへ標準の初回`configure_fonts`／repaint／returnを追加した。
製品コード全体（test module前）と`timeline_bass_raster_benchmark`のソースが
release計測前と完全一致することを検査し、`final/font-source-equality.log`へ記録。
独立レビューも当該fixtureの方針適合と製品／benchの証拠再利用を確認した（P1/P2なし）。
上の指定gateの他テスト・portable check・release計測の証拠は有効で、無関係な再実行はしない。

| 最終の限定確認 | exit／結果 |
| --- | --- |
| `cargo fmt`／`cargo fmt --check` | 各0 |
| `UPDATE_SNAPSHOTS=1`、lib `raster_snapshot` | 0、2成功 |
| 通常比較、lib `raster_snapshot` | 0、2成功 |
| `python scripts/check_ui_glyphs.py` | 0、危険glyph0 |
| `git diff --check` | 0 |

raw log、`font-results.json`、最終ソース3本のGit blobは
`target/pitch-stage3-validation/final/`（`source-blobs-final.txt`）に保存。
最終PNGを再び目視確認し、サイズ合計9336 bytesをsnapshot方針へ記録した。
未解決は本番6秒窓の2例の精度条件だけ。失敗テストを無効化したり期待値を緩めたりしていない。

変更ファイル: `crates/music-core/{src/bass.rs,tests/bass.rs,examples/bass_track_benchmark.rs}`、
`src/{app/music_bass.rs,ui_music_timeline.rs}`、
`docs/{music-keyboard-harmnet-plan.md,music-integration-plan.md,ui-snapshot-policy.md}`、
`htdocs/mimageviewer/manual/{music.html,tut-music.html}`、
`tests/snapshots/{timeline_bass_partial_raster.png,timeline_key_states_raster.png}`。

### 14.7 精度の受け入れ判断 (2026-10-14、ClaudeCode)

合成 8 ケースのうち、I-vi-IV-V (C / D) の本番 6 秒窓だけが平行短調 (A 短調 / B 短調) になった。
窓の中は I と V が各 1 秒、vi と IV が各 2 秒で、理想の 0/1 chroma でも C 長調 0.849 / A 短調 0.837 と
ほぼ同点になる (構成音が同じ平行調を区別できる情報が窓に無い)。旧方式は 8 ケース中 7 ケース正解 (新方式は 6 ケース) で、
外したのは I-IV-V-I (C) を属調の G 長調にしたもの (倍音の混入による)。この小さな合成音の集合では
新方式のほうが 1 件少ない (2026-10-14 コードレビュー P3 で訂正。当初「旧方式も 6/8」と誤記)。新方式の外れは平行調の曖昧さ、
旧方式の外れは倍音の混入で、性質が違う。利用者の方針 (おまけ的な可視化なので特定のケースで精度が落ちても
大きな問題ではない、鍵盤の構成音と調が合うことを優先) に従い、窓・プロファイル・信頼度式は変えずに
受け入れる。テストはこの 2 例の本番窓に限り「正解または平行短調」を合格とし、他は完全一致を求める。

### 14.8 独立コードレビューP2の修正 (2026-10-14、pitch-lab)

本番の`frame_start / sample_rate`によるbin時刻では、40秒trackの区間1だけが完成していても、
中心22.995秒の窓に重なる先頭bin（19.99秒）が未完成のフレーム999を参照する。
秒窓だけの完成判定（1000〜1300）と集計入力がずれ、欠落を省略した部分推定を公開していた。

既存のPCM実尺へ切り詰めた6秒窓の完成条件を維持し、先頭binの実参照時刻まで判定範囲を広げる。
集計自体も`Option`を返し、重みゼロの省略より前に各入力のchroma取得を確認する。
欠落があれば推定全体を`None`とし、部分集計を公開しない。新しい状態・公開経路・回復機構は足さない。
精度の受け入れ例外、プロファイル、重み、音名の色、ベース高さの仕様は変更しない。

回帰は本番と同じサンプル番号の除算でbinを生成し、区間1→0の順に完成すると
未計算→推定済みになることを先頭binの有音／無音の両方で検査する。
無関係な区間2の到着では確定済み推定が変わらないこと、集計途中に重みゼロの未完成入力があっても
全体が`None`になることも検査する。修正前に前者の失敗を確認済み（exit101、1失敗）。
ログは`target/pitch-stage3-p2-validation/`。製品起動、`target/dev-runtime`操作、コミットは行わない。

修正後の追加回帰2件は各exit0。独立追レビュー（GPT-6.1 Sol / xhigh）はACCEPT、新P1/P2なし。
指定確認は全て`MSBUILDDISABLENODEREUSE=1`で実行した。

| 確認 | exit／結果 |
| --- | --- |
| `cargo fmt --check` | 0 |
| `cargo test -p music-core` | 0、41成功、失敗0、doc test0 |
| `cargo test -p mimageviewer --lib ui_music_timeline::` | 0、28成功、失敗0、bench1 ignored |
| `cargo test -p mimageviewer --lib music` | 0、160成功、失敗0、bench2 ignored |

既存PNGの通常比較も成功、PNG差分なし。native依存停止は発生しなかった。
変更は`src/ui_music_timeline.rs`と本節の記録のみ。

## 15. 第 4 段階: 音名の配色と、音楽ビューの「…」メニュー (2026-10-14 追加)

状態: §15.5 の独立設計レビューと利用者の実装指示に基づき、pitch-lab で実装・自動検証完了。未コミット、利用者検収待ち。

### 15.1 背景と利用者の判断

- EffeTune の Note Spectrogram の音名の色は、OKLCH で色相を半音順に 30° ずつ (C=30°)、明度・彩度を 12 色で
  そろえた低彩度の配色 (`MULTI_F0_NOTE_COLORS`)。mIV は HSV で 5 度圏順、C が青 (色相 約 205°)、強さで
  明るさが変わり、音名によって見かけの明るさに倍近い差がある (同じ強さ 1.0 で G 0.47 / C# 0.90、OKLCH L)。
- 利用者の判断 (2026-10-14):
  1. 見かけの明るさを**暗くせず明るい側へそろえる**。比較画像の「案 A」(同じ強さの 12 色の中央値まで、
     暗い音名だけ持ち上げる) を採用。白鍵・黒鍵の上で白っぽくなりすぎないため。
  2. **C を赤**にする (調べた範囲では C=赤が多い: Chrome Music Lab の配色、Boomwhackers、EffeTune)。
  3. 色の付け方 (5 度圏順 / 半音順) を**画面上で切り替えて楽しめる**ようにする。場所は静止画フルスクリーンと
     同じ「…」(その他の機能) のパネル形式で、音楽ビュー上部の HUD に置く。
- 調査で分かったこと: 上の棒グラフは音名ではなく周波数の位置で色が決まる (`spectrum_color`) ので対象外。
  音名の色は鍵盤 (`ui_music_spectrum.rs` の `key_color`、▼ を含む) と、タイムラインのベース・調
  (`ui_music_timeline.rs` の `key_color`) の 2 か所に**同じ定義が重複**している。

### 15.2 決定事項

| ID | 決定 |
| --- | --- |
| D1 | 音名の色を 1 か所にまとめる (新モジュール、名前は実装者が決める。例 `src/music_note_color.rs`)。2 つの `key_color` は削除してそこを呼ぶ。`spectrum_color` (周波数の色) は変えない |
| D2 | 方式 `MusicNoteColorScheme { Fifths (既定), Chromatic }`。色相 = 並び順の位置 / 12 (5 度圏順は `pc*7 % 12`、半音順は `pc`)。**C = 赤 (色相 0)** から始める。彩度・明るさの元の式 (HSV の s = 0.62 + 0.28v、v = 0.38 + 0.58v) は維持 |
| D3 | 明るさの補正 (案 A): 各強さ v で 12 音名の元の色を OKLab に変換し、OKLCH の明度 L が 12 色の中央値より低い音名だけ、L を中央値まで上げる (色相維持、出せない彩度は sRGB の範囲へ詰める)。**元の色より暗くなる音名は無い**ことをテストで保証する。方式 2 種 × 12 音名 × 強さ 256 段の表を初回に 1 回だけ作り (`OnceLock`)、以後は表を引く (タイムラインのラスタで大量に呼ばれるため) |
| D4 | 設定 `Settings.music_note_color_scheme` を追加 (`#[serde(default)]`、既定 Fifths)。未リリースの新項目なので移行不要。変更時は既存の他の設定と同じく `settings.save()` |
| D5 | 方式を変えたら、鍵盤は次の描画から新しい色。タイムラインは `music_analysis_version` を進めて既存の再ラスタに乗せる (別の版数を作らない)。方式はラスタ要求に明示の値として渡す (グローバルな可変状態にしない) |
| D6 | 音楽ビュー上部の HUD (`draw_music_top_chrome`) の右側に「…」ボタンを置き、静止画と同じパネル (`draw_fs_overflow_panel_surface` / `draw_fs_overflow_panel_row`) を開く。中身は「音名の色」の 2 択 (5 度圏順 / 半音順、選択中の印は既存の行の表現に合わせる)。パネルの開閉状態を静止画の `FsOverflowPanelState` と共有するか (例: 新しい variant)、音楽ビュー専用の状態にするかは、既存の閉じる経路 (フルスクリーンを閉じる・別の項目へ移る・音楽ビューの終了) との組み合わせが少ない方を実装前レビューで決める。設計担当の第一案は**共有** (1 つのフルスクリーンで同時に開けるパネルは 1 つで、既存の閉じる経路にそのまま乗るため) |
| D7 | 「…」ボタンは**通常のフルスクリーンの音楽ビューだけ**に出し、parked-live の音楽ウィンドウには出さない (detached 凍結ルール。parked-live も設定の変更は反映される)。`draw_music_top_chrome` は 2 か所から呼ばれているので、表示の有無は呼び出し側から明示で渡す |
| D8 | キー操作は追加しない (マウス・タッチのメニュー操作のみ)。`docs/keymap-spec.md` に固定入力として理由を残す (低頻度の見た目の好み設定で、専用キーを割り当てる必要がない) |

### 15.3 テスト

- 色: C が赤 (色相 0 付近)、5 度圏順では G が C の隣の色相、半音順では C# が C の隣。補正後の L が元の L 以上
  (全方式・全音名・代表的な強さ)、補正後の L が同じ強さの中央値以上。表と直接計算が一致。
- 設定: 既定値、serde の往復、旧設定 (項目なし) の読み込みで既定値になる。
- メニュー: 開く・選ぶ・閉じる (× / 外側クリック / Esc / フルスクリーンを閉じる / 別の項目へ移る / 音楽ビューの
  終了) の状態遷移。選ぶと設定が変わり、版数が進むこと。parked-live の HUD にボタンが出ないこと。
- UI スナップショット: 色が変わる PNG は docs/ui-snapshot-policy.md に従って更新し、目視確認する。
  「…」パネルの音楽ビュー版を 1 枚追加する。

### 15.4 文書

`htdocs/mimageviewer/manual/music.html` (と `tut-music.html` の鍵盤の色の説明) に「…」メニューと色の付け方の
切り替えを内部用語なしで書く。`docs/music-integration-plan.md`、`docs/keymap-spec.md` (D8)。

### 15.5 独立レビュー (第 4 段階の設計) と反映

GPT-6.1 Sol / xhigh (5 回目): ACCEPT WITH CHANGES (P1 なし)。本節が §15.2〜15.4 の該当記述を上書きする。

| 指摘 | 反映 |
| --- | --- |
| P2 D6 は共有を採用、ただし表示判定の分岐が要る | `FsOverflowPanelState` に音楽用の variant を足して共有する (静止画と音楽のパネルが同時に開く状態を作らない)。今の描画分岐 (`src/ui_fullscreen.rs` 25447 行付近) は音楽ビュー中に開いたパネルを無条件に閉じるので、**静止画用 variant と音楽用 variant で表示条件と描画を分ける**。音楽ビューの終了時は音楽用 variant を閉じ、静止画の表示中は音楽用 variant を閉じる |
| P2 音楽側の入力処理 | 音楽パネルの矩形を、フレーム開始時の入力所有判定に組み込む (静止画トップバーの判定 32104 行付近は音楽ビューを除外しているので、音楽側に同等の判定を足す)。矩形内の wheel / Ctrl+wheel / タッチと、外側クリックで閉じる操作が、背面のタイムラインのスクロール・Row 変更・シーク (50058 行付近のシーク抑止) へ漏れない。閉じたフレームも、その入力の所有を保つ。キーは静止画の現行仕様 (29365 行付近) どおり「メニューを閉じて本来の操作を続ける」。`common_modal_dialog_open` には足さない。状態遷移テストに加え、実際の入力経路を通す回帰テストを書く |
| P2 D4 設定の保持 | HUD から変える項目なので、`Settings::overwrite_non_preferences_from` に `music_note_color_scheme` を足す。古い環境設定スナップショットを適用しても最新の配色を保つテストを書く |
| P3 D3 の計算と量子化 | 手順を固定する: sRGB を線形化 → OKLab → OKLCH。各強さ v で元の 12 色の L の中央値 (偶数個なので中央 2 つの平均) を目標にし、目標より低い音名だけ L を目標へ上げる。L と色相を固定したまま chroma を二分探索で縮めて sRGB の範囲に収める (RGB 成分の単純クリップはしない)。強さ v から表の添字は `round(v.clamp(0,1) × 255)`。「暗くならない」は**最終の `Color32` で**検証し、許容誤差は L で 0.004 (8bit 量子化分)。全エントリと段の境目を対象にする。表は初回 UI 呼び出しで作ると待ちが出るので、所要時間を測り、1ms を超えるならビルド時の定数表にする |
| P3 D7 と detached 手続き | parked-live にボタンを出さない方針は維持。呼び出し側から表示の有無を渡す変更も viewport 経路に「触れる」変更なので、凍結ルールに従い、独立レビュー (本レビューで「症状パッチではなく構造的に妥当」と同意済み) と設計担当の合意、変更範囲を `docs/detached-rework-plan.md` §11 に記録する |
| P3 D5 の非同期切り替えと HUD 配置 | テストを足す: 旧方式の要求の処理中に切り替える、短時間に往復で切り替える、一時停止中に切り替える。最終的に最新の方式へ収束し、既存のテクスチャを保持して表示が欠けないこと。HUD のスナップショットは通常幅に加えて狭い幅で、「…」・Row・タイトル・閉じるボタンが重ならないこと |

設計担当 (ClaudeCode) も D7 の変更が detached の述語・所有を変えない構造的な追加であることに同意する。

### 15.6 実装記録（2026-10-14、pitch-lab、未コミット）

配色は `music_note_color` に集約し、鍵盤・▼とタイムラインのベース／調へ方式を値で渡す。
純計算の生成元は `src/music_note_color/compute.rs`、再生成用は
`scripts/generate_music_note_colors.rs`。sRGB線形化→OKLab→OKLCH、中央2色のLの平均、
低いLだけ補正、L／色相固定のchroma二分探索、最後に8bit量子化の順序を共有する。
表生成だけを `Instant` で計測（ファイル出力を含まない）し、standalone Rust の
`rustc --edition 2024 -O` は0.687ms、最適化なしは3.147msだった（初回は各0.580／2.891ms）。
1msを超える条件があるため、初回UI生成を行わず事前生成したコンパイル時の定数表を採用する。
配色表の参照ではUIにOKLab計算やI/Oを追加しない。全エントリと量子化境界で最終Color32のL誤差0.004を検査する。

状態の簡素化は§15.5の共有パネルを採用し、静止画用と音楽用を排他的variantで分ける。
配色変更のためのmodal／専用再構築・別版数は追加しない。表示の好み変更を既存の
解析版数による再ラスタへ乗せれば、再生・一時停止・処理中の変更を既存cache ownerで扱えるため。
再ラスタ中は表示済みtextureを保持する。まれな失敗は既定の割り切り方針とし、回復機構を足さない。

`settings_transfer` の全Settingsフィールド分類にも新項目の登録が必要なため、既存のHUD設定と同じ
非環境設定の除外項目へ登録した。転送元と転送先の配色を異なる値にし、転送先の配色保持も検査する。
転送の対象を拡張する設計変更は行っていない。

背面の音楽描画だけを常設の子UIで無効化し、親UIで描くメニューの操作性を保つ。
子UIのIDは`UiBuilder::id_salt`と同じ規則の共通helperを使い、音声トラックpopupの
producer／共通入力handler／既存HUDテストの参照を一致させる。
通常wheelは既存の音量操作、Ctrl+wheelはRow、middle-dragはタイムラインという実経路で
所有中・閉じるframe・閉鎖後のfresh入力を検査する。製品の通常wheel仕様は変更しない。

検証のownerは実装担当。指定gateのログと結果を `target/pitch-stage4-validation/` に保存する。
実アプリ起動・通常設定操作・`target/dev-runtime`への操作・コミットは、利用者の指定に従って行わない。
`CHANGELOG.md` と `docs/next-release-backlog.md` は未変更。独立実装レビューと最終結果は本節へ追記する。

#### UI画像の確認

| PNG | 寸法／bytes | 変更理由・確認 |
| --- | --- | --- |
| `timeline_bass_partial_raster.png` | 240×80／1544 | ベース音の配色を共通表へ変更。1820px、x=8..147/y=23..35のみ変更。波形と未計算の灰色は保持 |
| `timeline_key_states_raster.png` | 660×80／7799 | C長調を青から赤へ変更。3400px、x=424..623/y=47..63のみ変更。未計算／調なしは灰色のまま |
| `music_overflow_hud_normal.png` | 820×300／15910 | 新規。全操作・Row・タイトル・「…」を通常幅で確認 |
| `music_overflow_hud_narrow.png` | 320×300／8363 | 新規。既存操作を残し、タイトルのclipと操作の非重複を確認 |
| `music_overflow_panel.png` | 820×300／22604 | 新規。「音名の色」、2択、選択中、閉じる操作の表示を確認 |

5枚合計56220 bytes（新規3枚46877 bytes）。更新前後と新規PNGを実装担当・独立レビュー担当が目視確認した。
狭幅compactは通常のメニューボタン付きHUDだけへ限定し、parked-liveの従来配置は維持する。
実入力回帰はdisposable App fixture、画像は共通純描画関数を使うlib harnessで検査し、実アプリ確認とは区別する。

#### 最終の自動確認

全Cargo実行に`MSBUILDDISABLENODEREUSE=1`を付けた。ログと全実行履歴は
`target/pitch-stage4-validation/`（`results.json`）。最終の成功結果は以下。

| コマンド | exit／成功件数／ignored |
| --- | --- |
| `cargo fmt` | 0 |
| `cargo fmt --check` | 0 |
| `cargo test -p music-core` | 0／41／0（doc test 0） |
| `cargo test -p mimageviewer --lib ui_music_spectrum::` | 0／28／1 |
| `cargo test -p mimageviewer --lib ui_music_timeline::` | 0／31／1 |
| `cargo test -p mimageviewer --lib music` | 0／183／2 |
| `cargo test -p mimageviewer --lib ui_fullscreen` | 0／708／1 |
| `cargo test -p mimageviewer --lib settings` | 0／570／13 |
| `cargo test --test ui_snapshot` | 0／162／0 |
| `cargo check -p mimageviewer --features portable` | 0 |
| `python scripts/check_ui_glyphs.py` | 0／危険glyph 0 |

追加の焦点確認は共通配色5件、既存の実音楽HUD12件、新メニュー14件が各exit0。
新メニュー14件には純描画PNG3枚と、実音声トラックpopupの通常wheel（音量／ファイル移動）抑止を含む。
新しいテストはignoredにしていない。PNGは更新時の2件／12件成功後、通常比較でも確認した。

途中で見つかった分類登録・fixture型／viewport・子UI ID参照・分類総数の不備を修正し、
影響範囲を再実行した。設計の期待値や製品の機能を緩めて合格させていない。
最後の修正は分類件数のtest-only更新（全458／除外314、転送144は維持）のみで、
無変更の製品コードと既に成功した関連テストの証拠を再利用する。native依存のビルド停止は発生していない。
検証用実行ファイルのbuild／実機起動は`target/dev-runtime`禁止の利用者指定に従って行わない。
設計逸脱なし。独立実装レビュー（GPT-6.1 Sol／xhigh）は **ACCEPT**、未解決のP1/P2なし。
配色・設定保持・非同期再ラスタ・入力所有と終了経路・HUD・parked-liveの既存配置保持、
音声トラックpopupの実入力回帰、最終gateログと5枚のPNGを確認した。
最後の分類件数テストだけの変更に対する、他の成功証拠の再利用も確認済み。

### 15.7 独立レビューの追加指摘修正（2026-10-14、pitch-lab）

利用者が第4段階を`db1f0139c`でコミット後、別セッションの独立レビューは
ACCEPT WITH CHANGESとなった。以下のP2/P3を追加修正する。

- P2: パネル上で中ボタンを押したまま`L`で閉じ、次frameでタイムラインへ移動すると、
  表示状態だけに依存していた入力所有が消え、背面のパンが始まった。
  パネルの表示と開始済みのポインタ操作の寿命を分け、解放／接点の終了まで所有する。
  キー本来の操作は既存の経路で続ける。別のApp boolやmodalは追加しない。
  所有状態はviewport内の入力routerに置き、表示contextで分離する。ボタン／接点の順序付き
  開始・終了を処理し、最後の解放後の新しい押下を閉じたパネルへ取り込まない。
  解放frameの所有は維持し、focus喪失・音楽表示終了・context変更では終了する。
  fullscreenを閉じて未描画の間に解放された操作は、再表示時に入力router内の描画leaseで失効する。
  viewportごとのframe内再利用は維持し、親ROOTのframeも記録する。対象の通常音楽surfaceは
  ROOT更新内から同期immediate描画されるため、ROOTの未描画gapを判別できる。
  parked-liveにはボタンもrouter呼出もなく、この所有を生成しない。deferred描画経路を
  今後変更する場合は描画頻度の前提も再検討する。既存keepalive／detachedの終了経路には変更を加えない。
- P3: 共通パネルの高さが行間を含まず、2択でも不要なスクロールバーが出ていた。
  高さ算定と描画の行間を共有し、十分な高さと小画面の両方を回帰検査する。
  見開き調整のラベル／チェック欄は自然な高さを保持し、文字の折り返しを含むgalley寸法を
  高さ算定・描画・ヒット判定で共有する。全欄の48px固定化や別frameでの再計測は採用しない。
  行間は従来の3pxを維持する。見開き調整の入力geometry計算前に既存の基点解決を通し、
  同frameの描画までにラベルが変わることでhit矩形と描画矩形が食い違わないようにする。
  bar無し幅の自然高が収まる場合は、barの表示と幅予約をともに無くす。小画面では従来の
  solid barを使い、同Contextの通常高→小画面→通常高でも長いラベルの余分な折り返しが残らない。
  f32座標の加減算による丸めは、自然高のfit比較だけで0.01ptまで許容する。
  小画面のbody高がegui既定の最小scroll高64px未満でも末尾が切れないよう、
  ScrollAreaの最小高を0にし、実clip内で最後の行へ到達することを検査する。
  静止画パネルも共有部分の修正対象とし、変更PNGは基準画像の更新前に目視確認する。

実装前の入力所有設計は、独立レビュー担当（GPT-6.1 Sol／xhigh）と実装担当で確認した。
detachedの述語やviewport構造は変更しない。通常設定・製品起動・`target/dev-runtime`への操作・
コミットは行わない。追加検証のownerは実装担当、ログは`target/review-fixes-validation/`に保存する。

#### 回帰と画像確認

実入力は使い捨てAppの`render_fullscreen_viewport`を通し、中ボタン押下→`L`→押したまま移動→
解放→新しいパン、touch終了／cancel、同frameの解放→新しい押下、focus／context変更、
fullscreen終了・非描画中の解放・再表示を検査する。押下前のカーソル移動を別のunheld入力にし、
実timelineのclip／delta（押下0、drag−70）／解析保持／manual-panと正のoffsetを確認する。
テスト用の観測は`cfg(test)`だけで、IDやgeometryをContextのロック取得前に採取する。

パネルは4variant×2幅×2文字サイズ×DPI倍率1／1.25／1.5の48条件で、自然高への収まり、
実clipへのinner包含、小画面での末尾到達、同Contextの通常高→小画面→通常高を検査する。
包含の丸め許容は0.01pt、末尾到達だけはeguiのpixel-grid丸めを含む1物理pixel＋0.01pt。
画像はAppをsnapshot対象にせず、従来の共通surfaceと本体フォントで生成する。

全11枚は基準画像更新前にbefore／candidateを並べて実装担当が目視し、独立レビュー担当も確認した。

| PNG | 変化と確認 |
| --- | --- |
| `fullscreen_overflow_panel_root.png` | 高さ＋15px、不要bar除去、最後の48px行のclip除去。9883px変更 |
| `fullscreen_overflow_panel_navigator_submenu.png` | 高さ＋12px、不要bar除去、選択行全体が表示。10231px変更 |
| `music_overflow_panel.png` | 高さ＋3px、不要bar除去、2行が収まる。HUDは保持。2883px変更 |
| `spread_shift_hud_resolved_normal_light.png` | 下部空白だけ削減（高さ348→282px）。26832px変更 |
| `spread_shift_hud_resolved_normal_dark.png` | 同上 |
| `spread_shift_hud_unresolved_normal_light.png` | 同上。jump無効色と解除・記憶の表示は保持 |
| `spread_shift_hud_unresolved_normal_dark.png` | 同上 |
| `spread_shift_hud_resolved_narrow_light.png` | 同じ下部空白削減。18792px変更 |
| `spread_shift_hud_resolved_narrow_dark.png` | 同上 |
| `spread_shift_hud_unresolved_narrow_light.png` | 同上。無効表示を保持 |
| `spread_shift_hud_unresolved_narrow_dark.png` | 同上 |

Spread8枚の差分はy=338..411の下端だけで、本文・marker・操作・チェック欄は一致する。
音楽HUD通常幅／320px幅の2枚と、timeline配色の2枚は未変更。
全11枚の寸法・bytes・pixel範囲は`target/review-fixes-validation/png-results.json`に保存した。

#### 最終確認

全Cargoへ`MSBUILDDISABLENODEREUSE=1`を設定。基準画像の更新後は`UPDATE_SNAPSHOTS`を使わず比較した。
以下は最終source／PNGでの成功結果。ignoredは従来の計測テストだけで、新しい回帰はignoredにしていない。

| コマンド | exit／成功／ignored |
| --- | --- |
| `cargo fmt` | 0 |
| `cargo fmt --check` | 0 |
| `cargo test -p mimageviewer --lib ui_fullscreen` | 0／714／1 |
| `cargo test -p mimageviewer --lib music` | 0／189／2 |
| `cargo test --test ui_snapshot` | 0／162／0 |
| `python scripts/check_ui_glyphs.py` | 0／危険glyph 0 |

焦点確認の音楽メニュー20件も成功（最小scroll高の追加修正前）。最後の2つのfull lib gateは
その追加修正とpixel丸め許容も含めて実行した。native依存のbuild停止は発生していない。
途中のfixtureのimport／観測時点・入力順序／ロック再入の不備を修正し、全指定gateを再実行した。
旧nested AppTestEnv fixtureは実行前に自分の検証processを停止して同じ1環境へ修正し、
中断を製品テストやnative依存の失敗として扱っていない。全実行履歴は同ディレクトリの`results.json`に保持する。
設計逸脱なし。製品起動・`target/dev-runtime`への操作・コミットは行っていない。
独立最終レビュー（GPT-6.1 Sol／xhigh）は **ACCEPT**。P2/P3の未解決指摘なし。
実入力の所有と再表示境界、48条件の実clip検査、11枚のPNG、全指定gateの最終成功ログを確認した。

### 15.8 pitch-labとmasterのマージ解消（2026-10-14）

利用者の実機受入後、HEAD `456820e05` へmaster `c7c76d239`を取り込む進行中マージのファイル内容を解消。
共通高さは利用者指定どおりmasterの `d73d62841` / `113cf9711` を正とする。
§15.7のP3独自実装は統合前の履歴であり、本文の行間0・固定48pt行高の共通計算へ統一する。
新しい状態・非同期処理・回復経路は作らない。両側の入力意図に矛盾がないことを独立確認した。

| 元の競合 | 解消方針 |
| --- | --- |
| `FsOverflowPanelView` | 音楽配色と構成編集の可否・管理状態を両方保持 |
| `FsOverflowPanelAction` / `FsRootOverflowFrame` | 配色変更と構成編集アクション、Rootのフレーム入力境界を保持 |
| `draw_fs_overflow_panel_surface` | masterの行間0・共通高さを採用。独自fit/bar制御を撤去 |
| 調整HUD snapshot fixtureのView | 両側のフィールドを保持し、共通矩形計算を使用 |
| 描画末尾のoverflow分岐 | RootフレームのAbsent判定と音楽／静止画別表示条件を保持 |
| `fullscreen_overflow_panel_view` | 配色と構成可否・管理状態の投影を両方保持 |
| パネル描画メソッド | 音楽の配色変更／描画と静止画foreground描画を両方保持 |
| 高さ・配置検査 | masterのRoot396pt／Navigator300ptの期待値を採用 |
| 共通高さ検査・snapshot | masterのScrollLayout／サイズ指定snapshotを採用し、MusicNoteColorを補充 |

独自の `fs_overflow_panel_natural_body_height`、`fs_overflow_panel_rect_for_view`、
`fs_overflow_shift_memory_galley`、`fs_overflow_text_galley`、Appの`fullscreen_overflow_panel_rect`を撤去。
P3専用の自然高検査2本は共通`calculated_overflow_panel_height_matches_all_states_and_scrolls_when_constrained`
へ統合し、Root／Navigator／調整／音楽の4variantを同じ`ScrollLayout` helperで検査する。
長い保存基点名の固定高さに関するmaster側の既知残件は[監査記録](calculated-panel-height-audit.md)のまま。
今回のマージでその既知残件を修正済みとは扱わない。

音楽の入力所有、キー閉鎖後の開始済みpointer/touchの寿命、メニュー内の端hover抑止は保持する。
Rootのキー前受付・EditorAdopted・構成編集・foreground入力境界も保持する。
async-architecture／detached-rework-plan／keymap-specの3競合は、両側の独立した節を残した。
detachedの記録は既存変更の記述を保全するもので、新たな述語・viewport・park経路の変更はない。

静止画Root／Navigatorの2枚、見開き調整HUDの8枚はmaster側PNGを採用した。
全10枚のmaster blobとの一致と比較画像を目視確認し、master方式の既存見た目を保持する。
`music_overflow_panel.png`だけを通常比較の候補生成→更新前の目視→採用の順で更新した。
第2行と下端が3pt上がり、行間0で全高159→156ptになる。選択表示・×・HUDは維持する。
正確な画素差は2049px（x=408..807、y=169..214）。kittestの許容差込み検出は1768px。
PNGの寸法820×300とbytes22472は同じ。通常幅／320pt幅HUDの2枚は通常比較に一致し未変更。

自動mergeの追加確認で、設定分類の両追加が共存する一方、固定期待値は両parent共通の458/314のまま
残っていた。実テストで全459対期待458の失敗を確認し、期待値だけを全459／除外315へ修正。
対象144／wire142、両設定の除外登録・取り込み先値保持は変えず、説明の現行件数も合わせた。
検証ログ・元画像・比較画像は `target/merge-validation/` に保存する。

#### 合算後のAppサイズ

最初のfullscreen全検査では、既存の切り取り入力のevent順序検査が標準テストthread上で
`STATUS_STACK_OVERFLOW`（exit `0xc00000fd`）となった。両parentと同じテスト／fixtureであることを確認し、
既存の`history_transition_storage_keeps_app_stack_footprint_bounded`を実行するとApp110344bytesで
既存の110000未満の上限に失敗した。上限やthread stackを引き上げず、既存のheap収納方針へ合わせる。

独立の構造確認を経て、`filename_stack_ui.rs`の`StackReturnState::Ready.prepared`だけを
`Box<StackPreparedItems>`へ収納し、唯一の生成と復帰で同じ値を一度だけ移譲する。
外側のstate型、App／bundleのフィールド型、所有・stamp・取消・復帰の判定は維持し、
registry／viewport／park経路の変更や新しい状態・回復機構は追加しない。
大きな値の収納と破棄は元のstateの寿命に従う。型・構造の変更はこの3点だけ。
サイズ検査、失敗した切り取り入力、スタック往復・既存多窓回帰、fullscreen全検査で再確認する。

収納後のAppは109336bytes（1008bytes減）で上限検査に成功。ただし切り取り入力の単独実行は
exit0でもfirst-chance stack例外を印字し、fullscreen全検査では再び異常終了したため解決とは扱わなかった。
既存のテスト実行体をCDBで捕捉し、通常heap（`-hd`）でも最初のfixture生成／Settings DB初期化中に
約2MiBの標準stackを使い切ることを確認した。入力handlerにはまだ到達していない。
4回のAppTestEnv返り値を持つテストcallerだけで約1.2MiBを予約していた。

独立確認の上、capture専用`setup_capture_region_input_test`の返り値とtuple生成の2点だけを
`Box<AppTestEnvForTest>`へ変更した。4ケース・event順序・assert・明示dropのテスト本体は両parentと同一。
global fixtureや実製品入力を変えず、同じfixtureの所有・安全なdrop順を保った収納境界の修正である。
Appサイズ上限と上記本体の収納修正は維持し、thread stackの引き上げや検査の弱化は行っていない。
再実行の単独captureは例外の印字なしで成功した。CDB診断ログも同じ検証ディレクトリに保存する。

#### 統合後の確認と引き継ぎ

全Cargoに`MSBUILDDISABLENODEREUSE=1`を設定。snapshot更新後は環境変数を外して通常比較した。
最終変更と関係しない検証は既存の検証再利用方針に従って再利用し、収納変更の影響箇所は再実行した。

| コマンド | exit／成功／ignored |
| --- | --- |
| `cargo fmt --check` | 0 |
| `cargo check -p mimageviewer --lib` | 0 |
| `cargo test -p music-core` | 0／41／0（unit 21、bass 12、harmnet 7、allocation 1、doc 0） |
| `cargo test -p mimageviewer --lib ui_fullscreen` | 0／746／1 |
| `cargo test -p mimageviewer --lib music` | 0／189／2 |
| `cargo test -p mimageviewer --lib settings` | 0／579／13 |
| `cargo test -p mimageviewer --lib overflow` | 0／54／1 |
| `cargo test --test ui_snapshot` | 0／176／0 |
| `cargo check -p mimageviewer --features portable` | 0 |
| `python scripts/check_ui_glyphs.py` | 0／危険glyph 0 |
| `cargo test -p mimageviewer --lib calculated_` | 0／10／0 |

追加のAppサイズ上限1件、capture入力順序1件、filename_stack_ui 25件、多窓35件、
stack contextの往復・stale return 8件もexit0。CDBの通常heap検査もexit0でcapture 1件成功、
first-chance stack overflowは0件（`capture-first-chance-clean-final.log`）。
中間失敗を含む履歴は`results.json`に残し、修正後の成功と混同しない。

共有state収納変更に伴う追加の`test-full.ps1 -SuppressCrashDialogs`はexit101、テスト開始0件。
launcherのビルド入力であるrelease core／remote／EPUB workerの3実行ファイルが存在せず停止した。
process内のcrash dialog設定はscriptが復元済み。指定gateのnative依存ビルド停止はない。
不足入力の追加ビルド・代替経路は実施せず、全体gateは未完了として引き継ぐ。

ファイル内の競合markerと`git diff --check`は解消済みだが、`git add`は
`C:/home/mimageviewer/.git/worktrees/mimageviewer-pitchlab/index.lock`のPermission deniedで失敗した。
worktreeのGit indexが今回の書き込み許可範囲外にあるため、index上の14ファイルは未解消扱いのまま。
indexの迂回・許可変更・別worktreeへの操作はせず、利用者側のステージングを残す。
マージコミット、製品起動、`target/dev-runtime`への操作は行っていない。

独立最終レビュー（GPT-6.1 Sol／xhigh）はファイル内容の解消を受理し、新規findingなし。
指定gateの成功、PNG目視、最終CDBログ、masterの入力境界と音楽側の保持を確認した。
ステージング不可と追加全体gate未完了を含むため、マージ全体の完了・全検証成功とは扱わない。

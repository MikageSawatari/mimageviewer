# mIV Sound catalog 値の照合記録

対象: `docs/mcp-guide-settings-plan.md` §21.4.1.1。2026-11-27、実装分担としての source audit。
独立レビューの記録ではない。製品の実行時観測、Rust テストの成功、未受領のレビューを含めない。

## 入力と数え方

- 同梱版 `0.19.0-miv.1`、作業基点 `931349add`。旧一覧は作業開始時の
  `src/sound_effects.generated.json` を一時ファイルへ写し、以降の生成結果から独立させた。
- 旧一覧 SHA-256: `b7a0d907a9475c36f0beecbee6b6d50e87eec5fa9611eda5f28675ddc446f03d`。
- 以下の `P/` は `vendor/miv-sound/mIV Sound.vst3/Contents/Resources/webview/plugins/`、
  `J/` は同 `webview/js/`。`D` は `J/audio/dsp-params.generated.js`。
- `plugins.txt` から採用された全 **114 effect** が対象。旧一覧の top-level `kind:number` は
  **753 鍵** (数値選択肢 30 鍵を含む)。既定値の配列・object 内の数値 leaf は **1,117 個**、
  合計 **1,870 leaf**。同じ member の全 index に同じ式が適用される場合も、既定配列の全要素を照合した。
- nested metadata は数値 control 65 family (条件付きの Headphone 曲線 2 family を含む)、
  Bass Management の数値選択肢 `ro[*]` / `sl[*]` も加えると 67 family。
  省略される Headphone 曲線は 1,117 に含まない。
- Multi Channel Panel の既定値は 8 ch だが実装は 16 ch。同じ bounds を後半 8 ch にも適用するコードと、
  DSP の全 16 ch slot を照合した。従って既定値 leaf の件数と pack slot の件数は同じではない。
  nested の既定値1,117 leafから、packを使わないRoomEq.bsの15 leafを除き、
  Multi Channel Panelの後半8 chのv/d、計16 slotを加えると、数値packは1,118 slotになる。

## 方法と境界

1. 旧一覧の全 effect・全 number 鍵・nested 数値 leaf を列挙し、`plugins.txt` の class とファイルを結び付けた。
2. 各 class の `getParameters` / `getSerializableParameters`、`setParameters` とそこから呼ぶ setter、
   `createUI` と数値表示を照合。共通 setter を継承する FIR 系は `P/fir-asset-eq.js` と
   subclass の `_setDesignParameters`、`_validateParameters` を併せて調べた。
3. 全 plugin に対し `toDisplay`、`Math.exp/log/log10/log2/pow`、`**`、bit shift、百分率・時間の乗除、
   number input と slider の代入を抽出。数値表示の単位換算と、slider の位置・グラフの座標・
   DSP 内での dB→gain 等を区別した。後者だけでは保存の形を変えない。
4. `D` の全 `pack*Params` の slot と descriptor の `packedOffset` を結び付け、
   `containerKey` / `memberKey` / `element` で nested と flat indexed 鍵を照合。
   初回調査では旧一覧にminが既にあるtop-levelだけを照らしたため611鍵、残り142鍵だった。
   今回はdescriptorのないpublic鍵もpack右辺のaccess pathから直接抽出し、
   旧一覧にminのなかった17鍵を含む **scalar 628鍵** を照合した。scalarの未packは
   **125鍵** (753−628)。数値enum、表示だけの設定、FIR設計の入力、定数等を含み、
   すべてが不明なわけではない。nestedは **1,118 slot**、合計 **1,746 numeric pack slot / 106 effect**。
   このmapped slotで `range_verified:false` は0。既定値の全1,870 leafとは母集団が違い、
   packを使わないJS設定と、既定8 chから16 chへ展開するslotを上記の通り区別した。
5. literal `parseFiniteNumber` の全検出と複雑な family の source inspection を別に行い、
   一時的な比較表 346 行を作成。同名 scalar pack と結び付いた 266 行の bounds は全件一致した。
6. 全753 scalarについて、生成器と同じ inert VM の新しいインスタンスへ各鍵を単独で
   `-1e12` / `1e12` として渡し、`getSerializableParameters` の値を読んだ。
   enum の拒否、派生定数、validation による拒否、兄弟値による補正を分類してsourceへ戻った。
   独立した固定clampで pack と新しく食い違う scalar は見つからなかった。
   この probe は純粋な JS model の検査であり、製品を起動した試験ではない。
7. `PluginBase.getSerializableParameters` (`P/plugin-base.js:1187`) は原則 JSON deep copy。
   `setSerializedParameters` (`:1213`) は保存鍵を setter へ渡す。
   `getWorkletPluginData` (`:1134`) は `getParameters` の値を packer へ渡す。
   mIV 側で単位を換算する前提は置かない。

範囲が相互依存する場合、固定の min/max は外側の目安である。scalar の min/max で許容集合の
穴・離散値・前後関係までは表さない。例: weightDecay は `0` または `0.5..60`、
クロスオーバー周波数の昇順、Q の shelf 制限、Phase Select EQ の core/outer の順序。
source で境界を読めたことと、生成 script がその式を機械抽出できることは別に記録する。

## 保存と表示の形が違う鍵、旧一覧との差異

`v` は保存値。以下の式は丸め前の数値を示す。Hz の整数表示などの書式丸めは式の後に行われる。

| effect / 鍵 | 旧一覧 | 保存する範囲・単位 / 表示 | 根拠 |
| --- | --- | --- | --- |
| TiltEQPlugin.f0 | 20.0855369..19930.3704 Hz、step 1、default 6.91 | 3..9.9、ln(Hz)、raw step .01。表示 Hz = `exp(v)` | `P/eq/tilt_eq.js:186`, `:255`, `:266`, `:294`; `D:2531` |
| SimpleJitterPlugin.rj | .001..10000000 ns、step .001、default 100 | 0..200、.001 ns 基準の dB、raw step 1。表示 ns = `.001*10^(v/20)` (UI は大きさで ps/ns/μs/ms を選ぶ) | `P/lofi/simple_jitter.js:169`, `:202-214`, `:227`; `D:1616` |
| StereoBalancePlugin.bl | -100..100 %、step 1 | -1..1、無次元、raw step .01。表示 % = `100*v` | `P/basics/stereo_balance.js:49`, `:79-81`; `D:2452` |
| AdaptivePredictionEffectPlugin.weightDecay | .5..60 s、default 0 | 0..60 s。`0` は無限、0より大きく.5未満は.5へ。つまみは0時に以前の有限値を表示する | `P/resonator/adaptive_prediction_effect.js:122-125`, `:162-166`; `D:7` |
| CompressorPlugin.rl | 1..1000 ms | 10..1000 ms | `P/dynamics/compressor.js:618`; `D:354` |
| ExpanderPlugin.rl | 1..1000 ms | 10..1000 ms | `P/dynamics/expander.js:574`; `D:520` |
| BrickwallLimiterPlugin.sm | -1..0 dB | -3..0 dB。UI の編集範囲 -1..0 を保存の制限としない | `P/dynamics/brickwall_limiter.js:741`, `:795`; `D:266` |
| DigitalErrorEmulatorPlugin.be | -12..-2、unit `10^x`、step .2 | -12..-2、log10(error rate)、raw step .2。UI は保存した指数そのものを表示するため `display` は付けない | `P/lofi/digital_error_emulator.js:940-960`, `:1010-1012`; `D:420` |
| G726ADPCMSimulatorPlugin.re | -6..-2、unit `10^x`、step .1 | -6..-2、log10(error rate)、raw step .1。UI は保存した指数そのものを表示するため `display` は付けない | `P/lofi/g726_adpcm_simulator.js:61-62`, `:112-114`; `D:801` |
| DigitalErrorEmulatorPlugin.rf | 数値選択肢のみ、min/maxなし | 保存kHzの選択肢 {44.1,48,88.2,96,176.4,192}。外枠44.1..192の付記で、連続値を受け付ける意味ではない | `P/lofi/digital_error_emulator.js:42`, `:967-969` |
| ExciterPlugin.hs | 数値選択肢のみ、min/maxなし | 保存は0..2の選択番号。範囲の付記で単位換算はない | `P/saturation/exciter.js:180-185` |
| GSMFullRateSimulatorPlugin.tc | 数値選択肢のみ、min/maxなし | 保存は1..3の選択番号。範囲の付記で単位換算はない | `P/lofi/gsm_full_rate_simulator.js:51-53` |
| MSMatrixPlugin.md | 数値選択肢のみ、min/maxなし | 保存は {0,1}。0..1の付記で単位換算はない | `P/spatial/ms_matrix.js:111-113` |
| MSMatrixPlugin.sw | 数値選択肢のみ、min/maxなし | 保存は {0,1}。0..1の付記で単位換算はない | `P/spatial/ms_matrix.js:123-125` |
| ModalResonatorPlugin.rs[*].fr/lp/hp | 3..9.9、unit空、step .01、Frequency/Low Pass/High Pass Log | 保存は ln(Hz)、範囲と raw step は旧一覧で正しい。number input の表示 Hz = `exp(v)` | `P/resonator/modal_resonator.js:355-376`, `:546-549`, `:576-584` |
| OscilloscopePlugin.dt | .001..10 s、step .001 | 保存は旧一覧と一致。表示 ms = `1000*v` | `P/analyzer/oscilloscope.js:331-340`, `:535`, `:601` |
| OscilloscopePlugin.ho | .0001...01 s、step .0001 | 保存は旧一覧と一致。表示 ms = `1000*v` | `P/analyzer/oscilloscope.js:443-451`, `:570`, `:605` |
| StereoMeterPlugin.wt | .01..1 s、step .001 | 保存は旧一覧と一致。表示 ms = `1000*v` | `P/analyzer/stereo_meter.js:228-233`, `:276`, `:293` |
| SpectrumAnalyzerPlugin.pt | 8..14、step 1、unit `log2 FFT size` | 保存は log2(FFT points)、表示 points = `2^v` (256..16384)。範囲とstepは旧一覧で正しい | `P/analyzer/spectrum_analyzer.js:581-598` |
| SpectrogramPlugin.pt | 8..14、step 1、unit `log2 FFT size` | 保存は log2(FFT points)、表示 points = `2^v` (256..16384)。範囲とstepは旧一覧で正しい | `P/analyzer/spectrogram.js:751-766` |
| PhaseSelectEqPlugin.regions[*] の12境界member | pack外枠の周波数20..40000、位相0..180、balance -100..100。dynamic false | JS の最小core幅から厳しくなる固定外枠と、兄弟値に依存する注記が必要。下表に全12memberを記す | `P/spatial/phase_select_eq.js:105-143`, `:370-376` |

既知の pack 不一致 7 鍵だけでは、Modal Resonator の nested 対数、analyzer の秒→ms、FFT exponent、
Phase Select EQ の兄弟値による補正を検出できない。特に analyzer の helper `modelKey` は
`displayTime` / `holdoff` / `windowTime` だが保存鍵は `dt` / `ho` / `wt` であり、
旧一覧の `scaled:true` の列挙だけでは落ちる。Digital Error と G726 の旧数値範囲は保存の形で
正しく、画面にも指数をそのまま出す。自動化用の物理量 `10^v` を画面用の `display` に混ぜない。

`scaled:true` 5 件のうち、FifteenBandPEQ.mg と LoudnessEqualizer.rv は小数桁の丸めだけ
(`P/eq/fifteen_band_peq.js:831`, `P/eq/loudness_equalizer.js:360`)。
Adaptive.autonomy は hold 時だけ 1 を表示し、weightDecay は0時に別の保持値を表示するので、
`v` だけから求められる換算式とは扱わない (`P/resonator/adaptive_prediction_effect.js:162-170`)。

### descriptor がない pack 鍵から追加した範囲

以下も旧一覧には数値の min/max がなく、保存の範囲の追加に該当する。
数値enumは既存の options を保ち、その外枠を付記する。連続値を受け入れる意味にはしない。

| 鍵 | 追加した min/max | 根拠 |
| --- | --- | --- |
| BassManagement.ls / sl[*] | 24..96、options {24,48,96} | `D:211`; `P/basics/bass_management.js:154-156`, `:174-175` |
| BassManagement.ro[*] | 0..3、options {0,1,2,3} | `P/basics/bass_management.js:148-150` と同packの全16slot |
| BrickwallLimiter.os / DynamicSaturation.os / Exciter.os / HarmonicDistortion.os / MultibandSaturation.os / Saturation.os | 1..8、options {1,2,4,8} | 各 `D:264`, `:467`, `:510`, `:860`, `:1184`, `:1582` |
| HardClipping.os | 1..16、options {1,2,4,8,16} | `D:848` |
| FIRCrossover.bc | 2..4、options {2,3,4} | `D:653`; `P/basics/fir_crossover.js:133-138` |
| CrosstalkCancellation.fd / FiveBandFIRPEQ.fd / HeadphoneEQ.fd | 0..65536 samples。保存/getの派生値で、独立したsetterではない | 各 `D:373`, `:727`, `:868`; `P/fir-asset-eq.js:44-54` |

数値の min/max/step/unit/default が変わる全43 familyは、この2表で網羅している。
内訳は前表の14 top-level (旧minなしから追加した DigitalErrorEmulator.rf、Exciter.hs、
GSMFullRateSimulator.tc、MSMatrix.md/sw の5鍵を含む) と、この追加表の12 top-level、
前表のModal Resonator 3 member・Phase Select EQ 12 member、この追加表のBass Management 2 items。
従って **26 top-level + 17 nested = 43 family**。秒→msやFFT exponentのような
displayだけを補足する行はこの43に数えない。

## nested 全 family の照合

同じ行に複数の member を記す場合も全 member・全 index を照合した。
以下に示す範囲は保存の形。周波数 slider の log は移動量の定義だけで、Hz を保存する。

| effect / 鍵 | 数値 leaf (既定値) | 保存の bounds / 照合結果 | JS 根拠 |
| --- | ---: | --- | --- |
| BassManagement.fc/ri/ro/rt/sl | 80 | fc 20..300 Hz (整数)、ri/rt 0..65535 masks、ro {0,1,2,3}、sl {24,48,96}。JSとpackと旧一覧が一致。riはrtのbit内に限定 | `P/basics/bass_management.js:148-167` |
| FiveBandDynamicEQ.bs | 50 | f/scf 20..20000 Hz、q/scq .1..10、mg 0..24 dB、th -60..0 dB、r .1..100、kn 0..10 dB、a .1..100 ms、rl 1..1000 ms。一致 | `P/eq/five_band_dynamic_eq.js:470-577` |
| ModalResonator.rs | 25 | fr/lp/hp 3..9.9 ln(Hz)、dc 1..500 ms、gn -18..18 dB。範囲一致、表示だけ上表の補足が必要 | `P/resonator/modal_resonator.js:349-382` |
| MultiChannelPanel.d/v | 16 | d 0..30 ms、v -20..10 dB。一致。16 ch 全て同じ関数。リンクしたchは値を共有 | `P/basics/multi_channel_panel.js:285-308`, `:514-524`, `:322` |
| MultibandBalance.bands | 5 | balance -100..100 %。保存自体が百分率で、StereoBalanceのように100倍しない | `P/spatial/multiband_balance.js:553-562` |
| MultibandCompressor.bands | 30 | t -60..0、r .5..20、a .1..100 ms、rl 10..1000 ms、k 0..12、g -12..12 dB。一致 | `P/dynamics/multiband_compressor.js:1073-1078` |
| MultibandExpander.bands | 30 | t -60..0、r .05..20、a .1..100 ms、rl 10..1000 ms、k 0..12、g -12..12 dB。一致 | `P/dynamics/multiband_expander.js:938-943` |
| MultibandSaturation.bands | 12 | dr 0..10、bs -.3...3、mx 0..100 %、gn -18..18 dB。一致 | `P/saturation/multiband_saturation.js:436-509` |
| MultibandTransient.bands | 21 | fa .1..10、fr 1..200、sa 1..100、sr 50..1000、sm .1..20 ms、gt/gs -24..24 dB。一致 | `P/dynamics/multiband_transient.js:677-683` |
| PhaseSelectEq.regions | 65 | pack外枠は旧一覧と一致。ただしJSは周波数/位相/balanceに最小core幅を置く。全5regionの12境界member、計60leafの外枠が下表の通り狭まる。gn 0..200 %は不変 | `P/spatial/phase_select_eq.js:81-143` |
| RoomEq.bs | 15 | frequency 20..20000 Hz、gain -20..20 dB、q .1..10。一致。UI shelvesのQ上限2は保存値の上限10を変えない | `P/eq/room_eq.js:1374-1381` |
| SpatialMapper.dm/fm/rm | 768 | 16×16 の linear gain、全要素 -1..1。一致。百分率として100倍しない | `P/spatial/spatial_mapper.js:150-158` |
| HeadphoneEQ.mr/mt | 条件付き | 読込後の曲線は dB の有限数配列、長さはstorage grid固定。数値上下限はない。既定の空配列は保存で省略 | `P/eq/headphone_eq.js:205-210`, `:233-238` |

### Phase Select EQ の固定外枠と相互依存

| member family (各5要素) | JS が保つ固定外枠 | 追加の前後関係 |
| --- | --- | --- |
| ofl / fl | 20..39999 Hz | ofl ≤ fl |
| fh / ofh | 21..40000 Hz | fl + 1 ≤ fh ≤ ofh |
| opl / pl | 0..179 degree | opl ≤ pl |
| ph / oph | 1..180 degree | pl + 1 ≤ ph ≤ oph |
| obl / bl | -100..99 % | obl ≤ bl |
| bh / obh | -99..100 % | bl + 1 ≤ bh ≤ obh |

coreだけでなくouterにも同じ外枠が波及する。例えば `ofl≤fl≤39999`、`oph≥ph≥1`。
これは `setParameters` が使う既定constraintsの最小幅1からの外枠。グラフ操作は画面の幅や
sampleRate/FFTからさらに大きいcore幅を使う (`P/spatial/phase_select_eq.js:48-78`)。
固定boundsを狭めても相互依存は残るので、12memberにその旨を付記する。

### 兄弟値による補正の再照合

| 対象 | 保存時の依存 | 固定外枠への影響 |
| --- | --- | --- |
| BassManagement.ri[*] | `ri &= rt` | maskの0..65535は不変 |
| MultiChannelPanel.v[*]/d[*] | linkのあるch同士で共有 | bounds不変、自由に別々の値を保持できるとは限らない |
| FiveBandDynamicEQ.bs、ModalResonator.rs、Multiband*.bands、RoomEq.bs、SpatialMapper行列 | 各数値memberのsetterに兄弟memberからの追加clampなし | 前表のboundsのまま。ModalのUI選択番号は別の保存値 |
| Multiband系のcrossovers | 後段の下限が前段以上 (`_normalizeCrossoverFrequencies`) | 個別の固定外枠は変わらないがdynamic注記が必要 |
| Chorus.dp | `dp≤dl` | 外枠0..20 msは不変。既定dl=12ではdp上限12 |
| FrequencyShifter.mn/mx | mn≤mx | 外枠0..5000 Hzは不変 |
| SFZNotePlayer.mn/mx、vf/vc | mx≥mn、vc≥vf+1 | 外枠は不変。既定vf=-60ではvc下限-59、vfを下げればvc=-60も保持 |
| Headphone/TonalBalance の qa0..4 | shelfなら最大2、pkなら最大10 | 外枠 .1..10、filter type依存 |

根拠: `P/modulation/chorus.js:245`、`P/modulation/frequency_shifter.js` の setter、
`P/others/sfz_note_player.js:51-67`、`P/spatial/multiband_balance.js:470-489` および
後の全effect索引のsetter。nestedのうち追加の固定外枠変更が必要だったのはPhase Select EQ。

## JS に境界があるが単純な literal 抽出では拾えないもの

| effect / 鍵 | source で読める境界と理由 | 根拠 |
| --- | --- | --- |
| FiveBandFIRPEQ.f0..4/g0..4/q0..4/s0..4 | 20..20000 Hz / -20..20 dB / .1..100 / .1..384 dB/oct。template index + 複数行parse。DSPへはFIR assetの設計値として渡す | `P/eq/five_band_fir_peq.js:92-125`; `P/fir-asset-eq.js:78-89` |
| HeadphoneEQ.ca/uf/mb/sm/fa0..4/ga0..4/qa0..4 | tableは -100..100 % / 1000..20000 Hz / 0..18 dB / .02..1 oct / 20..20000 Hz / -20..20 dB。Qは .1..(pk?10:2) | `P/eq/headphone_eq.js:4-9`, `:218-230` |
| TonalBalanceEQ の数値鍵 | RANGES table + `Object.fromEntries`。qa0..4は .1..(pk?10:2)。保存は周波数Hz・gain dB・Qのまま | `P/eq/tonal_balance_eq.js:17-23`, `:319-328` |
| GroupDelayEq.d0..14 / GroupDelayPEQ.d0..4 | ms、±floor((tp/2-tp/16)*1000/sampleRate*10)/10。tpとsampleRateに依存し、変更時も再clamp。旧一覧±149.3は既定32768 taps/48kHzに一致 | `P/eq/group_delay_eq.js:82-90`, `:119-128`; `P/eq/group_delay_peq.js:126-138`, `:174-199` |
| GroupDelayPEQ.f0..4 / q0..4 | 20..20000 Hz / .1..100、Q_RANGE参照とindexed key | `P/eq/group_delay_peq.js:34`, `:182-198` |
| RoomEq.pl | 20..20000 Hzが外側の境界。manual時に測定等に由来する下限も適用 | `P/eq/room_eq.js:1196-1205` |
| scalar数値enum | UIの選択肢は保存数値。`includes` 等による離散集合の検査であり連続clampではない。選択肢のmin/maxを必須としない | 後の全effect索引のsetter |

RoomEqは保存でDSPのサンプル数 `dy` を削除し `dl=delayMs` を書く
(`P/eq/room_eq.js:1150-1154`)。従って保存 `dl` の 0..20 ms をサンプルへ換算しない。
同様に派生 `fd` はtapsから計算される。DSPにだけある鍵を通常のユーザー設定と混同しない。

## 上下限を照合できない鍵・単一の固定範囲にできない鍵

| effect / 鍵 | 残る制約 / 分類 | 一覧での扱い |
| --- | --- | --- |
| HeadphoneEQ.mr[*]/mt[*] | sourceは有限性と長さだけを検査。数値上下限が定義されていない | unit dB、境界なし、`range_verified:false`。既定省略を新しいdefault値で埋めない |
| RoomEq.rp | `Number.isSafeInteger` かつ0以上。上限のliteral clampはなく、測定のreference pointに依存 | 0以上の目安と条件注記。source上の整数性と数値上下限の検証を区別 |
| FiveBandFIRPEQ.dy/gn | `PACKED_EXTRAS={dy:0,gn:0}`。get側の定数で、setterが入力を採用する経路はない。ユーザーが選べる範囲を照合する相手がない | 定数の由来を記録し、自由に設定できる範囲とは断定しない (`P/eq/five_band_fir_peq.js:50`, `P/fir-asset-eq.js:48-54`) |
| CrosstalkCancellation.fd / FiveBandFIRPEQ.fd / HeadphoneEQ.fd | DSP packは0..65536の整数を扱うが、setterで独立に保持する設定ではなくtp/phaseからget側で求める | packに範囲がある事実と、AIが任意値を指定してそのまま保持できるかを区別。`P/spatial/crosstalk_cancellation.js:112`, `:165`; `P/fir-asset-eq.js:44-54` |
| GroupDelayEq.d0..14 / GroupDelayPEQ.d0..4 | sourceの動的clampは読めるが、tap/sampleRate全組合せを1つの固定min/maxにはできない | 既定値の目安と依存式を保つ。source不明ではない |
| UIだけのaliasやDSPだけの鍵、既定値なしの `kind:unknown` | 保存値としての型・既定値がない。例 `displayTime` と `dt` を別の数値設定に数えない | 本文の753/1117とは別。`keys_incomplete` を消す理由にはしない |

その他の固定範囲のsourceは下の索引およびnested表で追える。生成側の抽出未対応を理由に
`range_verified:false` が残る場合、上表の「sourceに上下限がない」と同じ意味にしない。
この監査はsourceの値の所有と形を扱い、任意の入力に対する全ランタイム経路の受理を保証しない。

## 生成結果の実数 (2026-11-27 の最終候補)

作業開始時の旧一覧と、r1 指摘への修正後に再生成した 769,784 bytes の候補を比較した。
範囲・step・unit・default のうちいずれかが変わったのは **43 family**
(top-level 26、nested 17)。既定値の全indexへ展開すると **133 numeric leaf**。
`default` 自体の変更は0。`display` は **11 family** (top-level 8、nested 3)、
既定値へ展開して23 leaf。表示だけを追加して保存の範囲を変えない鍵も含むため、43と11は足さない。
選択肢の型の変更はこの43には数えず、次節に別記する。

`range_verified:false` は **22 family** (top-level 20、nested 2)。
sourceに固定の数値上下限を発見できなかったものと、離散値しか受け取らないものを以下で区別する。
既定値のあるnestedの全65 family (1,117 leaf) では、生成の抽出不足を理由にしたfalseは残っていない。

| 区分 | 残る鍵 | family数 | sourceで分かること |
| --- | --- | ---: | --- |
| 数値enum | AnalogMeter.dc/ln/ls/sc、ChromaSpiral.dm、CrosstalkCancellation.tp、FIRCrossover.s1/s2/s3/tp、FiveBandFIRPEQ.tp、GroupDelayEq.tp、GroupDelayPEQ.tp、HeadphoneEQ.tp、ModalResonator.sr、RhythmAnalyzer.sp、RoomEq.tp | 17 | 保存する数値集合はoptionsとsetterで照合済。対応する連続のpack clampがなく、固定clamp検証とは扱わない |
| get側の定数 | FiveBandFIRPEQ.dy/gn | 2 | 0を返し、setterは入力を採用しない |
| 片側だけの境界 | RoomEq.rp | 1 | 非負のsafe integer。測定の選択に依存、上限literal clampなし |
| 条件付き曲線 | HeadphoneEQ.mr[*]/mt[*] | 2 | 曲線は有限dB、storage gridの長さ。数値上下限の規定なし |

Phase Select EQの全12境界memberと、Group Delayの動的clamp、Headphone/TonalのQ条件は
sourceで制約が分かるため、この22の「未検証」に含めない。固定の目安だけで全入力の受理を保証する
意味ではなく、それぞれのdynamic注記とこの文書の式を併せて使う。

## r1 指摘への修正: 画面・選択肢・上書きの根拠 (§21.4.1.1 の8〜10)

### 既存の display 全25要素の再照合

descriptor の `transform` は保存範囲・保存単位の導出だけに使う。画面の変換は、
根拠を持った `valueOverrides` の各行から明示的に付ける。下表の全25要素をUIコードへ戻って照合し、
指数をそのまま見せる2要素を外し、23要素を残した。式は従来と同じく書式丸め前の値を表す。

| 鍵 | 要素数 | 結果 / 実際の画面 | 画面コードの根拠 |
| --- | ---: | --- | --- |
| TiltEQ.f0 | 1 | 残す。Hz入力は `round(exp(v))`、対数sliderは保存値 | `P/eq/tilt_eq.js:252-266`, `:283-299` |
| SimpleJitter.rj | 1 | 残す。`.001*10^(v/20)` ns を大きさに応じ ps/ns/μs/ms で表示。一覧の基準単位はns | `P/lofi/simple_jitter.js:169`, `:174-179`, `:202-214` |
| StereoBalance.bl | 1 | 残す。保存比率を100倍した%を表示し、入力を100で割る | `P/basics/stereo_balance.js:78-81` |
| Oscilloscope.dt / ho | 2 | 残す。保存秒を1000倍したmsを表示 | `P/analyzer/oscilloscope.js:331-340`, `:443-451` |
| StereoMeter.wt | 1 | 残す。保存秒を1000倍したmsを表示 | `P/analyzer/stereo_meter.js:228-233` |
| SpectrumAnalyzer.pt / Spectrogram.pt | 2 | 残す。`1 << pt` の点数を表示し、入力からlog2で指数へ戻す | `P/analyzer/spectrum_analyzer.js:581-598`; `P/analyzer/spectrogram.js:751-766` |
| ModalResonator.rs[*].fr / lp / hp | 15 | 全5要素の3memberを残す。各Hz入力は `round(exp(v))`、入力からlogで戻す | `P/resonator/modal_resonator.js:546-583`, `:658-668` |
| DigitalErrorEmulator.be | 1 | 外す。画面は指数 `v` と単位ラベル `10^x`。`10^v` は自動化の物理量で画面の数ではない | `P/lofi/digital_error_emulator.js:1010-1012` |
| G726ADPCMSimulator.re | 1 | 外す。同じく指数 `v` と単位ラベル `10^x` | `P/lofi/g726_adpcm_simulator.js:112-114` |

### 選択肢の保存型

既定値と `storage_shape` を根拠に、scalar・配列items/elements・object membersの説明をたどる。
数値保存に対するUI文字列を数値へ戻し、labelと既定値とstorage_shapeを保つ。
以下の15 scalar鍵、計58個の `options[].value` が文字列からnumberへ変わった。

| 鍵 | 保存型 / 選択肢の例 | 根拠 |
| --- | --- | --- |
| AnalogMeter.dc / ln / ls / sc | number、`"0"`→`0` 等 | `P/analyzer/analog_meter.js:350-369`, `:580-599` |
| BassManagement.ls | number、`"24"`→`24` 等 | `P/basics/bass_management.js:120`, `:174-175`, `:724-726` |
| CrosstalkCancellation.tp | number、`"4096"`→`4096` 等 | `P/spatial/crosstalk_cancellation.js:165`, `:738-740` |
| FIRCrossover.tp / FiveBandFIRPEQ.tp / HeadphoneEQ.tp / RoomEq.tp | number、`"32768"`→`32768` 等 | `P/basics/fir_crossover.js:639-645`; `P/eq/five_band_fir_peq.js:191-198`; `P/fir-asset-eq.js:465-472` (Headphone呼出は `P/eq/headphone_eq.js:518`); `P/eq/room_eq.js:3197-3199` |
| GroupDelayEq.tp / GroupDelayPEQ.tp | number、`"16384"`→`16384` 等 | `P/eq/group_delay_eq.js:264-268`; `P/eq/group_delay_peq.js:341-346` |
| GSMFullRateSimulator.tc | number、`"1"`→`1` 等 | `P/lofi/gsm_full_rate_simulator.js:51-53`, `:102-104` |
| MSMatrix.md | number、`"0"`→`0` 等 | `P/spatial/ms_matrix.js:111-113`, `:142-147` |
| RhythmAnalyzer.sp | number、`"8"`→`8` 等 | `P/analyzer/rhythm_analyzer.js:294-299`, `:1188-1189` |

Bass Management の `ro[*]` / `sl[*]` は元からnumberで変更なし。object内のfilter typeなど
string保存の選択肢はstringのまま。選択肢metadata全169 familyのうち、number保存は32 family
(scalar 30、Bass配列2)。全既定値のJSON型と保存形、各選択肢の型を同じrecursive walkで検査する。
`--check` とRust回帰に両方の型を比較する検査を置き、Rust回帰にはnumberとlabelを保つ例も入れた。

### 上書きの内容指紋

`valueOverrides` 全11行に `evidence.file`、変換/丸め/範囲の式文字列 `code`、各式の
期待出現数 `occurrences` を固定で持たせた。行番号には依存しない。空白と改行を除いた式の内容と
出現数を現在の同梱物と照合し、不一致なら通常生成・`--check` の双方が書き出し前に停止して
上書きの見直しを求める。これらの根拠情報は配布するパラメータmetadataには出さない。

同じ式の別コピーが残る場合も見逃さない。TiltのHz表示式は3箇所、Jitterの基準定数は
DSP側とUI側の2箇所にあるため、存在だけでなく出現数も照らす。
`--check` では全行の各式・各出現を一つずつメモリ上で変えて拒否を検査し、
行移動を模した改行追加は許容する。vendorファイルを試験のために書き換えない。

今回のRustテスト・ビルドは指示により未実施。修正前のrunner結果を本修正後の結果として扱わない。
追加したRust回帰は `sound_catalog_display_only_describes_actual_screen_values` と
`sound_catalog_option_values_match_saved_shapes`。前者は2つの指数鍵への `display` 再追加と、
残した11 familyの欠落・余分な表示を検出する。後者は既定値・保存形・scalar/nested選択肢の型を比較する。
生成側の通常実行と `--check` では根拠の内容と出現数を照合し、`--check` 内で
指紋の各出現の改変拒否・行移動許容・全保存形と選択肢の型・画面根拠のない `display` の拒否も検査する。

## 全114 effect の調査索引

`数/中` はtop-level number鍵数 / 既定値のnested数値leaf数。`保存/受理/UI` は
該当ファイル内の getSerializable/get、set/validate/design、createUI の開始行。
`inherited` なら `P/plugin-base.js:1187,1213`、FIR受理ならさらに `P/fir-asset-eq.js:78`。
列挙した全数値鍵を対象にした。nestedの全memberは前表に記す。

| effect | 数/中 | 数値鍵 | source (P/以下; 保存/受理/UI) |
| --- | ---: | --- | --- |
| AMRadioSimulatorPlugin | 16/0 | bw, cp, dt, fd, hm, in, io, md, mx, og, pe, sg, sk, st, tb, tn | `lofi/am_radio_simulator.js` 1897/1909/2119 |
| AdaptivePredictionEffectPlugin | 7/0 | autonomy, gap, learn, original, prediction, residual, weightDecay | `resonator/adaptive_prediction_effect.js` 104/110/141 |
| AnalogMeterPlugin | 13/0 | at, dc, it, ln, ls, nr, os, ph, rg, rl, rt, sc, tg | `analyzer/analog_meter.js` 350/373/554 |
| AttackTonalBalancePlugin | 2/0 | at, tn | `dynamics/attack_tonal_balance.js` 32/21/43 |
| AutoFilterPlugin | 9/0 | at, hf, lf, mx, rl, rs, rt, sn, sp | `modulation/auto_filter.js` 268/276/348 |
| AutoLevelerPlugin | 7/0 | at, gt, mg, ng, rt, tg, tw | `dynamics/auto_leveler.js` 437/452/682 |
| AutoPanPlugin | 5/0 | ct, dp, ph, rt, wd | `modulation/auto_pan.js` 131/143/201 |
| BandPassFilterPlugin | 4/0 | hf, hs, lf, ls | `eq/band_pass_filter.js` 264/275/298 |
| BandwidthExtenderPlugin | 3/0 | cf, ha, na | `saturation/bandwidth_extender.js` 40/51/103 |
| BassExtenderPlugin | 2/0 | am, og | `saturation/bass_extender.js` 36/45/86 |
| BassManagementPlugin | 6/80 | bg, hg, lf, lg, ls, su | `basics/bass_management.js` 120/142/690 |
| BitCrusherPlugin | 4/0 | bd, be, sd, zf | `lofi/bit_crusher.js` 209/154/221 |
| BluetoothSBCSimulatorPlugin | 4/0 | bp, mx, og, pl | `lofi/bluetooth_sbc_simulator.js` 87/114/192 |
| BrickwallLimiterPlugin | 6/0 | ig, la, os, rl, sm, th | `dynamics/brickwall_limiter.js` 755/744/777 |
| CassetteArtifactsPlugin | 9/0 | az, bs, dl, dp, hs, mx, og, rl, wf | `lofi/cassette_artifacts.js` 3085/3105/3377 |
| ChannelDividerPlugin | 7/0 | bc, f1, f2, f3, s1, s2, s3 | `basics/channel_divider.js` 487/496/595 |
| ChorusPlugin | 7/0 | dl, dp, fb, mx, rt, ss, vc | `modulation/chorus.js` 228/233/301 |
| ChromaSpiralPlugin | 6/0 | df, dm, ft, hi, lo, lr | `analyzer/chroma_spiral.js` 93/99/223 |
| ClickRemoverPlugin | 2/0 | ml, sn | `restoration/click_remover.js` 112/122/217 |
| ClipRestorerPlugin | 2/0 | og, th | `restoration/clip_restorer.js` 112/122/217 |
| CombFilterPlugin | 3/0 | dw, ff, fg | `eq/comb_filter.js` 137/148/168 |
| CompressorPlugin | 6/0 | at, gn, kn, rl, rt, th | `dynamics/compressor.js` 645/604/822 |
| CrossfeedFilterPlugin | 3/0 | dl, lf, lv | `spatial/crossfeed_filter.js` 138/149/171 |
| CrosstalkCancellationPlugin | 9/0 | fd, fh, fl, mg, og, rg, st, tp, wl | `spatial/crosstalk_cancellation.js` 118/142/732 |
| DCOffsetPlugin | 1/0 | of | `basics/dc_offset.js` 39/24/48 |
| DSD64IMDSimulatorPlugin | 12/0 | am, an, cs, dw, eb, hf, nc, nt, ot, sc, st, ul | `lofi/dsd64_imd_simulator.js` 463/473/721 |
| DattorroPlateReverbPlugin | 11/0 | bw, dc, dd1, dm, dp, id1, id2, md, mr, pd, wm | `reverb/dattorro_plate_reverb.js` 519/537/552 |
| DelayPlugin | 8/0 | dp, ds, fb, hd, ld, mx, pd, pp | `delay/delay.js` 268/283/299 |
| DigitalErrorEmulatorPlugin | 3/0 | be, rf, wt | `lofi/digital_error_emulator.js` 995/940/1005 |
| DopplerDistortionPlugin | 4/0 | cf, df, sc, sm | `modulation/doppler_distortion.js` 136/146/122 |
| DynamicSaturationPlugin | 10/0 | cm, db, dd, dm, og, os, sd, sm, sp, ss | `saturation/dynamic_saturation.js` 227/163/333 |
| EarphoneCableSimPlugin | 20/0 | lc, lv, rc, rf0, rf1, rf2, rf3, rf4, rq0, rq1, rq2, rq3, rq4, rz0, rz1, rz2, rz3, rz4, zb, zo | `eq/earphone_cable_sim.js` 487/492/546 |
| ExciterPlugin | 6/0 | bs, dr, hf, hs, mx, os | `saturation/exciter.js` 213/170/457 |
| ExpanderPlugin | 6/0 | at, gn, kn, rl, rt, th | `dynamics/expander.js` 601/560/774 |
| FDNReverbPlugin | 13/0 | bd, df, dm, ds, dt, hd, lc, md, mr, pd, rt, sw, wm | `reverb/fdn_reverb.js` 379/400/417 |
| FIRCrossoverPlugin | 8/0 | bc, f1, f2, f3, s1, s2, s3, tp | `basics/fir_crossover.js` 123/129/624 |
| FMRadioSimulatorPlugin | 9/0 | bw, dl, fd, mp, mx, og, pr, st, tn | `lofi/fm_radio_simulator.js` 1164/1176/1220 |
| FifteenBandGEQPlugin | 15/0 | b0, b1, b10, b11, b12, b13, b14, b2, b3, b4, b5, b6, b7, b8, b9 | `eq/fifteen_band_geq.js` 254/268/283 |
| FifteenBandPEQPlugin | 46/0 | f0, f1, f10, f11, f12, f13, f14, f2, f3, f4, f5, f6, f7, f8, f9, g0, g1, g10, g11, g12, g13, g14, g2, g3, g4, g5, g6, g7, g8, g9, mg, q0, q1, q10, q11, q12, q13, q14, q2, q3, q4, q5, q6, q7, q8, q9 | `eq/fifteen_band_peq.js` 408/421/502 |
| FiveBandDynamicEQ | 0/50 | - | `eq/five_band_dynamic_eq.js` 367/378/585 |
| FiveBandFIRPEQPlugin | 24/0 | dy, f0, f1, f2, f3, f4, fd, g0, g1, g2, g3, g4, gn, q0, q1, q2, q3, q4, s0, s1, s2, s3, s4, tp | `eq/five_band_fir_peq.js` 80/93/179 |
| FiveBandPEQPlugin | 15/0 | f0, f1, f2, f3, f4, g0, g1, g2, g3, g4, q0, q1, q2, q3, q4 | `eq/five_band_peq.js` 321/331/364 |
| FrequencyShifterPlugin | 7/0 | cf, mix, mn, mx, rt, sh, sp | `modulation/frequency_shifter.js` 425/430/338 |
| G726ADPCMSimulatorPlugin | 3/0 | mx, og, re | `lofi/g726_adpcm_simulator.js` 39/50/100 |
| GSMFullRateSimulatorPlugin | 4/0 | ci, mx, og, tc | `lofi/gsm_full_rate_simulator.js` 39/50/99 |
| GatePlugin | 6/0 | at, gn, kn, rl, rt, th | `dynamics/gate.js` 674/634/841 |
| GroupDelayEqPlugin | 16/0 | d0, d1, d10, d11, d12, d13, d14, d2, d3, d4, d5, d6, d7, d8, d9, tp | `eq/group_delay_eq.js` 113/119/260 |
| GroupDelayPEQPlugin | 16/0 | d0, d1, d2, d3, d4, f0, f1, f2, f3, f4, q0, q1, q2, q3, q4, tp | `eq/group_delay_peq.js` 167/173/333 |
| HardClippingPlugin | 2/0 | os, th | `saturation/hard_clipping.js` 199/165/328 |
| HarmonicDistortionPlugin | 6/0 | h2, h3, h4, h5, os, sn | `saturation/harmonic_distortion.js` 116/130/298 |
| HeadphoneEQPlugin | 21/0 | ca, fa0, fa1, fa2, fa3, fa4, fd, ga0, ga1, ga2, ga3, ga4, mb, qa0, qa1, qa2, qa3, qa4, sm, tp, uf | `eq/headphone_eq.js` 205/218/464 |
| HiPassFilterPlugin | 2/0 | fr, sl | `eq/hi_pass_filter.js` 328/337/351 |
| HornResonatorPlugin | 8/0 | co, cv, dp, ln, mo, th, tr, wg | `resonator/horn_resonator.js` 414/430/460 |
| HornResonatorPlusPlugin | 8/0 | co, cv, dp, ln, mo, th, tr, wg | `resonator/horn_resonator_plus.js` 445/461/491 |
| HumGeneratorPlugin | 5/0 | fr, hm, in, lv, tn | `lofi/hum_generator.js` 291/304/343 |
| HumRemoverPlugin | 2/0 | hc, sp | `restoration/hum_remover.js` 119/130/249 |
| IRReverbPlugin | 6/0 | co, dl, dt, dw, pd, tr | `reverb/ir_reverb.js` 135/230/2052 |
| LevelMeterPlugin | 0/0 | - | `analyzer/level_meter.js` 118/129/332 |
| LoPassFilterPlugin | 2/0 | fr, sl | `eq/lo_pass_filter.js` 328/337/351 |
| LoudnessEqualizerPlugin | 8/0 | hf, hg, hq, lf, lg, lq, rv, sp | `eq/loudness_equalizer.js` 282/297/332 |
| MDSimulatorPlugin | 2/0 | mx, og | `lofi/md_simulator.js` 40/50/96 |
| MP3CodecSimulatorPlugin | 2/0 | mx, og | `lofi/mp3_codec_simulator.js` 51/64/161 |
| MSMatrixPlugin | 4/0 | md, mg, sg, sw | `spatial/ms_matrix.js` 97/108/138 |
| MatrixPlugin | 0/0 | - | `basics/matrix.js` 196/181/264 |
| ModalResonatorPlugin | 2/25 | mx, sr | `resonator/modal_resonator.js` 314/325/503 |
| MultiChannelPanelPlugin | 0/16 | - | `basics/multi_channel_panel.js` 259/274/800 |
| MultibandBalancePlugin | 4/5 | f1, f2, f3, f4 | `spatial/multiband_balance.js` 458/527/570 |
| MultibandCompressorPlugin | 4/30 | f1, f2, f3, f4 | `dynamics/multiband_compressor.js` 1121/1041/1326 |
| MultibandExpanderPlugin | 4/30 | f1, f2, f3, f4 | `dynamics/multiband_expander.js` 982/908/1174 |
| MultibandSaturationPlugin | 3/12 | f1, f2, os | `saturation/multiband_saturation.js` 523/436/680 |
| MultibandTransientPlugin | 2/21 | f1, f2 | `dynamics/multiband_transient.js` 714/657/941 |
| MutePlugin | 0/0 | - | `basics/mute.js` 28/21/36 |
| NarrowRangePlugin | 4/0 | hf, hs, lf, ls | `eq/narrow_range.js` 392/403/426 |
| NoiseBlenderPlugin | 1/0 | lv | `lofi/noise_blender.js` 218/229/246 |
| NoiseReductionPlugin | 5/0 | hf, mix, rd, sm, sn | `restoration/noise_reduction.js` 34/17/46 |
| NoteSpectrogramPlugin | 3/0 | mn, mx, ts | `analyzer/note_spectrogram.js` 319/337/1471 |
| OscillatorPlugin | 5/0 | fr, it, pn, vl, wd | `others/oscillator.js` 403/418/430 |
| OscilloscopePlugin | 5/0 | dl, dt, ho, tl, vo | `analyzer/oscilloscope.js` 596/611/310 |
| PhaseSelectEqPlugin | 0/65 | - | `spatial/phase_select_eq.js` 358/367/813 |
| PhaserPlugin | 7/0 | cf, fb, mx, rg, rt, sp, st | `modulation/phaser.js` 244/249/317 |
| PitchMeterPlugin | 3/0 | mn, mx, rf | `analyzer/pitch_meter.js` 94/107/409 |
| PitchShifterHQPlugin | 2/0 | ft, ps | `modulation/pitch_shifter_hq.js` 35/44/85 |
| PitchShifterPlugin | 4/0 | ft, ps, ws, xf | `modulation/pitch_shifter.js` 299/310/353 |
| PolarityInversionPlugin | 0/0 | - | `basics/polarity_inversion.js` 28/21/36 |
| PowerAmpSagPlugin | 3/0 | ps, rs, ss | `dynamics/power_amp_sag.js` 362/374/646 |
| RSReverbPlugin | 9/0 | df, dp, ds, hd, ld, mx, pd, rs, rt | `reverb/rs_reverb.js` 367/384/397 |
| RhythmAnalyzerPlugin | 3/0 | mn, mx, sp | `analyzer/rhythm_analyzer.js` 260/277/1157 |
| RoomEqPlugin | 17/15 | cr, dl, dw, fh, fl, gn, mb, pl, pr, ps, rf, rp, rs, rv, rw, sm, tp | `eq/room_eq.js` 1150/1175/204 |
| RotarySpeakerPlugin | 8/0 | ac, ad, dd, mx, rb, sp, sw, xo | `modulation/rotary_speaker.js` 325/358/305 |
| SFZNotePlayerPlugin | 15/0 | dm, mn, mx, nh, og, os, pl, rd, sp0, ss, th, tm, vc, vf, wm | `others/sfz_note_player.js` 36/48/332 |
| SWRadioSimulatorPlugin | 18/0 | bf, bw, cp, ds, dt, fd, hm, in, io, md, mx, og, pe, sg, sk, st, tb, tn | `lofi/sw_radio_simulator.js` 1605/1617/1864 |
| SaturationPlugin | 5/0 | bs, dr, gn, mx, os | `saturation/saturation.js` 152/110/254 |
| SectionPlugin | 0/0 | - | `control/section.js` 16/24/32 |
| SimpleJitterPlugin | 1/0 | rj | `lofi/simple_jitter.js` 218/225/150 |
| SpatialMapperPlugin | 6/768 | de, dr, ic, ph, sp, ts | `spatial/spatial_mapper.js` 186/161/333 |
| SpectrogramPlugin | 2/0 | dr, pt | `analyzer/spectrogram.js` 375/389/724 |
| SpectrumAnalyzerPlugin | 2/0 | dr, pt | `analyzer/spectrum_analyzer.js` 302/317/552 |
| StereoBalancePlugin | 1/0 | bl | `basics/stereo_balance.js` 64/49/73 |
| StereoBlendPlugin | 1/0 | stereo | `spatial/stereo_blend.js` 71/80/96 |
| StereoMeterPlugin | 2/0 | gn, wt | `analyzer/stereo_meter.js` 288/298/213 |
| SubSynthPlugin | 8/0 | dhf, dhs, dl, shf, shs, sl, slf, sls | `saturation/sub_synth.js` 256/271/335 |
| TVAudioSimulatorPlugin | 10/0 | bw, bz, dl, fd, mp, mx, og, pr, st, tn | `lofi/tv_audio_simulator.js` 1665/1677/1738 |
| TapeArtifactsPlugin | 6/0 | bs, hs, mx, og, rl, wf | `lofi/tape_artifacts.js` 1228/1243/1366 |
| TiltEQPlugin | 2/0 | f0, sl | `eq/tilt_eq.js` 205/214/230 |
| TimeAlignmentPlugin | 1/0 | dl | `delay/time_alignment.js` 106/84/114 |
| TonalBalanceEQPlugin | 24/0 | am, at, fa0, fa1, fa2, fa3, fa4, ga0, ga1, ga2, ga3, ga4, hi, lo, qa0, qa1, qa2, qa3, qa4, rg, sm, sp, tc, ts | `eq/tonal_balance_eq.js` 303/312/541 |
| ToneControlPlugin | 3/0 | bs, md, tr | `eq/tone_control.js` 285/295/303 |
| TransientShaperPlugin | 7/0 | fa, fr, gs, gt, sa, sm, sr | `dynamics/transient_shaper.js` 240/220/415 |
| TremoloPlugin | 7/0 | cp, cs, dp, rc, rn, rs, rt | `modulation/tremolo.js` 262/275/245 |
| TubeSimulatorPlugin | 15/0 | bi, dr, iv, kr, mx, nf, og, pb, pv, rl, sb, sg, sr, su, sz | `saturation/tube_simulator.js` 6400/6414/7487 |
| VinylArtifactsPlugin | 11/0 | cl, cm, hs, mx, pl, pp, rb, rt, tn, wr, xt | `lofi/vinyl_artifacts.js` 433/435/445 |
| VinylSimulatorPlugin | 17/0 | cm, dr, dz, hf, lv, mb, mx, og, rc, rd, rg, rs, sc, sm, st, tf, tm | `lofi/vinyl_simulator.js` 1203/1215/1493 |
| VolumePlugin | 1/0 | vl | `basics/volume.js` 62/47/71 |
| WowFlutterPlugin | 7/0 | cp, cs, dp, rc, rn, rs, rt | `modulation/wow_flutter.js` 337/350/321 |

### 再利用する検証の範囲

この文書の作成時に行ったのは Python による JSON/JS の読取・列挙・bounds照合、source inspection、
inert VMでのscalar setter probe。cargo / fmt / cmake / build / Rust test / 製品起動は
この監査担当では実行していない。生成器の決定性・catalog freshness・Rust側の応答と予算・
実装の独立レビューは担当者の検証記録を参照する。

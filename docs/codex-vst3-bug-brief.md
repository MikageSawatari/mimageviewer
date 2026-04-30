# VST3 機能の課題 (第 2 弾) — Codex への調査依頼

このファイルを Codex GUI でそのまま投入してください。
**回答は Markdown** で `docs/codex-vst3-bug-answer.md` に上書きしてください
(= 既存の前回回答は置き換え可)。

UTF-8 BOM 付きで保存しています (= Codex GUI / メモ帳で文字化け防止、
CLAUDE.md「Markdown / テキストファイルのエンコーディング」セクション参照)。

---

## 依頼内容

mIV (= Rust + egui アプリ) の VST3 機能で、前回の調査+反映後に残った課題と
新たに浮上した設計上の懸念について、コード読解ベースで以下 5 点の調査と
修正方針を求めます。

リポジトリ: `C:/home/mimageviewer/`

各課題で「**バグ箇所** / **原因** / **修正案** (具体ファイル + 行番号 + コード片)」
を P1/P2/P3 で出力してください。

---

## 設計の要点 (前回 brief から抜粋)

- **host window**: mIV プロセスの `src/video/dsp/gui.rs::run_gui_thread` で
  `CreateWindowExW`。HWND は cross-process で bridge に渡る。
- **plugin child**: bridge プロセスで `IPlugView::attached(host_hwnd)` で host
  の child として作成される。
- **永続 GuiHost**: 初回 show のみ create + attach、以降は ShowWindow(SW_HIDE
  /SW_SHOWNA) でトグル。
- **動的 TOPMOST**: 通常時は OFF、フルスクリーン中だけ
  `SetWindowPos(HWND_TOPMOST)`。
- **z-order snapshot/restore**: `set_all_guis_visible(false)` 直前に z-order を
  `GetTopWindow + GetWindow(GW_HWNDNEXT)` で snapshot、`set_all_guis_visible(true)`
  で全 SW_SHOWNA → bottom-to-top 順に SetWindowPos で復元。
- **音声経路**:
  ```
  ffmpeg decoder → audio_rx (channel) → audio-pump スレッド
    → bridge.process_block (push_audio + pull_audio = synchronous IPC)
    → AudioBuffer (リング、現在 cap=300ms@48kHz) → cpal callback → OS → 出力
  ```
  ※ `cap` は最近 1.5s → 300ms に縮小したばかり。

---

## 課題 1 (P1): プラグイン GUI 一括表示時の "パラパラ表示" + z-order 復元時のチラつき

### 現状の挙動

VST ボタン OFF → ON で複数 (3-4 個) のプラグイン GUI を一括表示するとき:

1. `set_all_guis_visible(true)` が slot 順に `show_slot_gui(idx)` を呼ぶ
2. 各 `show_slot_gui` は SW_SHOWNA で window を可視化 (= 即座に DWM 合成)
3. 全部 SW_SHOWNA した**後**、snapshot した z-order を bottom-to-top で
   `SetWindowPos` で復元
4. 結果: 各 GUI が 1 つずつ「パラパラ」と現れ、最後に z-order が並び変わる
   ため **明確なチラつき** がユーザーに見える

ユーザー要望: 「最初から正しい z-order で同時に表示したい」。

### 関連コード

- `src/video/dsp/mod.rs::DspBridge::set_all_guis_visible`
  (前後 ~530-600 行目あたり、`target_visible=true` 経路)
- `src/video/dsp/mod.rs::DspBridge::show_slot_gui` (= 各 slot 個別の SW_SHOWNA)
- `src/video/dsp/gui.rs::set_window_visible` (= ShowWindow ラッパー)
- `src/video/dsp/gui.rs::set_window_topmost` (= SetWindowPos ラッパー)

### 質問

1. **`BeginDeferWindowPos / DeferWindowPos / EndDeferWindowPos`** で全
   plugin GUI の visibility + z-order を **一発で確定**できないか?
   `DeferWindowPos` に `SWP_SHOWWINDOW` を渡せば show + z-order を同時に
   反映できる。問題は `ShowWindow` 経由ではなく `SetWindowPos` で `SW_SHOWNA`
   相当の挙動になるかどうか (= z-order 引数を明示しないと SHOWNA にならない)。
2. もし DeferWindowPos が使えない場合の代替案は? (例: SW_SHOWNA はそのままで、
   一斉に DWM compositing されるよう順序を工夫する等)
3. snapshot z-order 自体を「SW_SHOWNA 前に setwindowpos で並べ直す」順番にする
   と、SW_SHOWNA 後の追加調整が要らなくなるか?

具体的なコード片で示してほしい。

---

## 課題 2 (P1): 音声 latency をさらに縮小したい / cpal callback 内処理

### 現状

`cap_samples = sample_rate * 2 * 0.3` (= 300ms) に設定済。
ユーザーの EQ 操作 → 音への反映 = audio_buffer fill 量 = ~300ms。
ユーザーは「動画停止/再生は瞬時なのに、VST 反映は数百 ms 遅れる」と
不満を表明。「**もっと短くできないか**」と質問。

### ユーザーの提案

「再生しようとしてオーディオデバイスに送る直前 (= cpal callback) で処理したら
遅延も減るのでは?」

### 当方の懸念

- cpal callback は real-time audio thread (= WASAPI Shared なら 10-20ms 周期)。
  デッドラインに間に合わないと underrun でブツブツ音切れ。
- VST3 bridge IPC roundtrip は 1-5ms 程度 (実測)。callback 内で同期 IPC は
  危険 (= ジッタで超過し得る)。
- ただし「callback 内で読んでから加工して書く」が DAW での標準実装と聞く。

### 関連コード

- `src/video/audio.rs::run_pump` (= 現在 audio-pump スレッドで bridge.process_block)
- `src/video/audio.rs::fill_output` (= cpal callback、現在は加工済み audio を
  ring から pop するだけ)
- `src/video/dsp/mod.rs::DspBridge::process_block` (= synchronous IPC)
- `src/video/dsp/bridge.rs::Bridge::push_audio` / `Bridge::pull_audio`
  (= shared memory ring + named events)

### 質問

1. **DAW (Reaper / Cubase / Bitwig 等) は実際どう実装している?** プラグイン
   process を audio callback 内 ? それとも別スレッド ? bridge プロセス使う場合は?
2. mIV のように **bridge プロセスで plugin を host** している場合、最低どこまで
   latency を詰められる? 100ms 以下は現実的か?
3. `cap_samples` をさらに縮小 (= 100-150ms) するときに気をつけるべき点は?
   (= cpal の WASAPI Shared 内部 buffer + audio-pump のジッタ + plugin 処理時間)
4. もし cpal callback 内で IPC を回すなら、どう設計するのが安全か?
   (= タイムアウト fallback + lookahead pre-fetch 等)
5. **alternative**: Rust 側で audio をブロック単位で先読みして bridge に flow
   ぎりぎりまで send、bridge が即返さない場合は callback で「最後に到達した
   ブロックの末尾を再利用」するのは妥当?

---

## 課題 3 (P1): Plugin Delay Compensation (PDC) 未実装

### 現状

VST3 プラグインは `IAudioProcessor::getLatencySamples()` で latency を申告する。
mIV 本体の `latency_samples: u32` フィールドは bridge から `Loaded` event で
受け取り保存しているが、**実際の補正処理は未実装**。

### 影響

ユーザーが「mIV Test Latency」プラグイン (= 自作テスト用、特定 sample 数の
遅延を入れる) で確認したところ、**音声と動画が ズレる** 現象が出る。
本来は plugin が latency=N samples を申告したら、host は:
- 動画フレームを N samples 分 (= N/sr 秒) **遅らせる** (= 表示を後ろに引く)
- もしくは音声を N サンプル **先読み**して plugin に流し、出力で N samples
  分 trim する (= 完全 PDC)

DAW では PDC は標準機能。mIV は動画 + 音声同期もあるので両者を絡める必要あり。

### 関連コード

- `src/video/dsp/mod.rs::DspBridge::add_plugin` (= `latency_samples` を保存)
- `src/video/dsp/mod.rs::PluginSlot::latency_samples`
- `src/video/dsp/bridge.rs::Event::Loaded` / `LatencyChanged`
- `src/video/audio.rs::run_pump` (= 音声処理経路、現在 latency 無視)
- `src/video/clock.rs` / `src/video/engine/clock.rs` (= AvClock = A/V 同期の
  master、`set_audio_pts` で音声 PTS を anchor)

### 質問

1. mIV の audio anchor 設計を踏まえた PDC 実装案を提示してほしい。
   - VST3 plugin が `latency_samples=N` を返したとき、`AudioFrame.pts_secs` の
     扱いをどう調整する? (= "出力された sample が指す入力 PTS = wall - N/sr"
     になる、master clock 側で吸収?)
   - 動画は input PTS 基準で表示しているので、自動的にずれる? それとも
     明示的に shift するべき?
2. プラグインが途中で latency を変えた場合 (= `LatencyChanged` event) の扱い:
   - audio_buffer flush が必要?
   - 動画側の clock anchor のリセットが必要?
3. 複数 VST3 プラグインを直列にチェーンした場合の latency 合算:
   - plugin チェーンの先頭で「N1+N2+N3 sample 分先読み」して各 plugin を
     順番に通す? それとも各 plugin 個別に補償?
4. テスト用「mIV Test Latency」プラグインで段階的に検証する手順案

---

## 課題 4 (P2): VST3 プラグインで音量がピークを超えた場合の挙動

### 現状

ユーザーから質問: 「プラグインが元音量より増幅して peak を超えた場合の挙動は?」

mIV の音声経路を辿ると:
- bridge plugin が出力する f32 サンプルを **そのまま** ring に書く (= 制限なし)
- `audio.rs::fill_output` が `out[written] = s * vol` で volume 適用するが
  **clip しない**
- cpal callback → WASAPI Shared → OS mixer → DAC

f32 で `> 1.0` のサンプルが来た場合:
- WASAPI Shared は OS mixer にそのまま渡す (= float のまま)
- OS mixer は他アプリと混合して 16-bit / 24-bit 整数に変換するときに **hard
  clip** する (= 上限値で頭打ち)
- 結果: 耳に届く音は **ハードクリップ歪み** (= harsh / 割れた音)

DAW では出力段に soft limiter / brickwall limiter / clip indicator 等を入れる
のが普通。

### 質問

1. mIV としてどこまで「音量超過」をケアすべきか?
2. soft clip / brickwall limiter を出力段 (= cpal callback 直前 or 内) に
   挟む案の妥当性
3. それとも「VST3 プラグインの責任。host は clip しない」が正解?
4. 視覚的な clip indicator (= 管理パネルや動画 HUD に "OVER" 表示) を出すなら
   どこで peak detection する?

---

## 課題 5 (P2): リサイズイベントのスロットリング (= "バッファされたリサイズを再生"挙動)

### ユーザー報告

Insight2 を **Bitwig (DAW)** で使うと「window 内の表示は多少ちらつくが
反映は素早い」。一方 **mIV** では「ドラッグした後、バッファに詰まった
サイズ変更を時間をかけてトレースしているような動き」が出る。
**ユーザーの仮説**: 「プラグイン側での再描画が終わるまで、次のリサイズ
情報を送らない、みたいな処理が必要かも」。

### 当方の理解

- mIV `pump_gui_signals` は毎フレーム resize_signal を drain して latest を
  bridge に send (= 60fps なら 16ms 周期)
- bridge は受け取った notify_host_resize ごとに `view->onSize` 同期 call
- plugin (= Insight2) の onSize 処理が 16ms より遅いと、stdin pipe に
  notify_host_resize が **積み重なる**
- 結果: ドラッグ後も bridge / plugin が backlog を消化するまで内部リサイズが続く

### 関連コード

- `src/video/dsp/mod.rs::DspBridge::pump_gui_signals`
  (= resize_signal drain + notify_host_resize send)
- `crates/vst3-host/src/main.cpp` の `notify_host_resize` ハンドラ
- `crates/vst3-host/src/plugin_loader.cpp::PluginLoader::notify_host_resize`

### 質問

1. notify_host_resize にスロットリング (= 直近送信から N ms 経過しないと
   skip) を入れるべき? 適切な N は?
2. bridge 側で `onSize` 完了 ack を返して、mIV が ack 受信するまで次の
   notify を送らない方式にすべき? (= back-pressure)
3. WM_ENTERSIZEMOVE 中は notify を送らず、WM_EXITSIZEMOVE で **最終サイズ
   1 回だけ** 送る方式は妥当? (= ドラッグ中は plugin 内容更新なし、
   ドラッグ後に最終形に飛ぶ)
4. それとも throttle ではなく drain 強化 (= bridge が "古い" notify_host_resize
   を skip して **最新だけ処理**) で解決?

---

## 全体の整合性に関する質問

これら 4 課題は相互依存があります (= latency 縮小と PDC、parapara 表示と
バッチ更新)。どの順序で着手するのが安全か (= 依存グラフ + リスク評価) も
コメントしてもらえると助かります。

---

## 回答テンプレート

```markdown
# Codex 回答 (第 2 弾): VST3 残課題

## 課題 1 (P1): parapara 表示 + チラつき
### 場所
- file:line ...

### 原因
...

### 修正案
具体的なコード片:
\`\`\`rust
...
\`\`\`

## 課題 2 (P1): latency 縮小 + cpal callback 内処理
...

## 課題 3 (P1): PDC 実装
...

## 課題 4 (P2): peak 超過時の挙動
...

## 着手順序の提案
1. ... (理由)
2. ...
```

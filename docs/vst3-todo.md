# VST3 機能の TODO 管理

mIV v0.9.0 の VST3 プラグイン処理機能について、**完了 / 進行中 / 未着手 /
保留** のタスクを追跡する文書。ユーザーと Claude の両方が参照する。

更新履歴:
- 2026-04 初版作成
- ユーザー報告 → 修正 → 検証 を繰り返すサイクルで更新

凡例:
- ✅ 完了 (= ユーザー確認済)
- 🟢 修正済 (= ユーザー検証待ち)
- 🟡 進行中
- ⏳ Codex 回答待ち
- 📋 未着手
- 🤔 設計議論中
- 🚫 deferred (= 別リリース or 別優先度)

---

## ✅ 完了

### 基本機能
- [x] VST3 SDK 3.8.0+ (MIT) を vendor に配置
- [x] C++ bridge プロセス (`mimageviewer-vst3-host.exe`) 実装
- [x] stdin/stdout JSON IPC + 共有メモリ + Windows named events で音声 IPC
- [x] 単一 VST3 プラグインのロード / 音声処理
- [x] プラグインチェーン (= 最大 10 個直列処理)
- [x] チェーン内のプラグイン bypass / 並べ替え / 追加 / 削除
- [x] プラグイン GUI のホストウィンドウ作成 + 表示

### v0.9.0 開発中の修正
- [x] cmd プロンプトウィンドウ表示問題 (= `CREATE_NO_WINDOW`)
- [x] 黒い再生中パネル UI (= 動画背景と馴染む配色)
- [x] 動画コンパクト表示 (右上 1/4) でプラグイン GUI スペース確保
- [x] BS キー誤発動の防止 (= `any_dialog_open()` に追加)
- [x] 文字化け解消 (× / VST テキスト置換) + lint スクリプト
- [x] CLAUDE.md「Markdown / テキストファイルのエンコーディング」追記
- [x] フルスクリーン解除時の自動 cleanup
- [x] VST ボタン OFF 時の動画コンパクト自動解除
- [x] 永続 GuiHost 設計 (= show/hide で window 再作成しない、DAW 並み高速トグル)
- [x] 動的 TOPMOST 切替 (= フルスクリーン中のみ)
- [x] Insight2 リサイズ振動: WM_ENTERSIZEMOVE/EXITSIZEMOVE セッション式抑止
- [x] 環境設定での VST3 チェーン編集 UI
- [x] 環境設定で追加した plugin が OK 押下後に保存されないバグ修正
- [x] 重複メニュー (設定 > VST3 プラグイン管理) 削除
- [x] ツールバー VST ボタン削除
- [x] z-order 保持 (= snapshot/restore for `set_all_guis_visible`)
- [x] per-slot `user_hidden` 状態 (= GUI × したものは VST トグルで復活させない)
- [x] プラグインウィンドウ × も `user_hidden` として記憶 (= パネル GUI × と同じ扱い)
- [x] 音声 buffer 縮小 (= 1.5s → 300ms、EQ 反映遅延短縮)
- [x] 音声 buffer 追加縮小 (= 300ms → 150ms → 100ms、refill tick 2ms、ユーザー確認済)

---

## 🟢 修正済 / 検証待ち

### Step 1: PDC 最小実装 [課題 3] (2026-04, 検証待ち)
- `DspBridge::total_latency_samples()` accessor 追加
  (= `!bypass && Loaded` スロットの latency_samples 合算)
- `AudioBuffer.pdc_latency_secs: f64` フィールド追加
- pump push 時に `bridge.total_latency_samples() / sample_rate` を計算して更新
  (= 値変化時はログ出力)
- `fill_output` で `pts_for_video = (pts_now - pdc_latency_secs).max(0.0)` を
  `clock.set_audio_pts` に渡す
- VST 無効 / 全 bypass のときは pdc_latency_secs=0 なので影響ゼロ (= 既存動作)
- 検証手順 (Codex 提案):
  1. mIV Test Latency = 0 samples で現状一致 → ✅ 自動 (pdc=0 で既存と同じ pts)
  2. 4800 samples (= 100ms) で動画 100ms 先行を確認 (PDC OFF) → ⏳ ユーザー検証
  3. PDC ON で同期復元 → ⏳ ユーザー検証
  4. 直列 2400 + 4800 = 150ms 合算 → ⏳ ユーザー検証
  5. 再生中 latency 変更 → 短時間のジャンプ発生 (= flush/anchor reset は **未実装**)
     最大 300ms (= バッファ内の旧 latency 処理済 samples 分) のずれが出る可能性
- LatencyChanged event ハンドリング (flush + anchor reset) は未実装。
  通常は固定 latency なので問題にならない。プラグインモード切替で気になれば後続対応。

### Step 2: 音声 buffer 縮小 (300ms → 100ms) [課題 2] (2026-05-01, 完了)
- `TARGET_PROCESSED_SECS` を `0.10` に変更。
  (= post-VST processed queue が EQ / plugin 操作反映遅延の主因)
- `READY_THRESHOLD_SECS` も `0.10` に変更。
  (= processed cap を下げた後も Buffering → Playing 条件が満たされるようにする)
- pump の自律 refill tick を `5ms` → `2ms` に短縮して、低水位追従を速くした。
- cpal callback 内 IPC は未導入 (= deadline miss リスクを避けるため従来通り P3)。
- ユーザー確認 (2026-05-01):
  1. 150ms 版は動作問題なし。ワンテンポ遅れるほどではなくなった。
  2. 100ms 版でも動作問題なし。反応が少し良くなった。
  3. Windows / 非 Windows の秒数分岐はノイズになるため削除し、100ms に一本化。

---

## 📋 Codex 第 2 弾 回答 → 着手順 (= 次セッションで実装)

`docs/codex-vst3-bug-answer.md` に Codex 回答受領済 (= コード片 + 場所 + 理由)。
**Codex 推奨着手順** (= 依存関係と検証容易性で並べた):

### Step 1 (P1): PDC 最小実装 [課題 3]
- **mIV Test Latency** プラグインで検証可能、機能正当性の問題
- DspBridge に `total_latency_samples()` accessor 追加
- AudioBuffer に `pdc_latency_secs: f64` フィールド追加
- run_pump push 前に latency_secs を計算して buffer に書く
- fill_output で `pts_for_video = pts_now - pdc_latency_secs` を `clock.set_audio_pts`
- **方式**: 「動画クロックを plugin latency 分だけ遅らせる」(= audio 先読み版より低リスク)
- LatencyChanged event 受信時に flush + anchor reset
- 複数 plugin 直列は `N1+N2+N3` 合算して video clock shift
- 検証手順 (Codex 提案):
  1. mIV Test Latency = 0 samples で現状一致確認
  2. 4800 samples (= 100ms) で動画 100ms 先行を確認 (PDC OFF)
  3. PDC ON で同期復元確認
  4. 直列 2400 + 4800 = 150ms 合算確認
  5. 再生中 latency 変更 → flush/anchor reset 後にジャンプなし

### ~~Step 2 (P1): 音声 buffer 縮小 (300ms → 120-150ms) [課題 2]~~ → ✅ 完了
- `TARGET_BUFFER_SECS: 0.3` → `0.10` として反映
- 100ms 未満は実機計測必須、現時点では攻めすぎ
- pump の refill tick 5ms → 2ms に短縮 (= "低水位追従")
- cpal callback 内 IPC は **P3** (= まだ入れない、deadline miss リスク高)
- どうしても入れるなら `try_process_block(deadline=2ms)` + bypass fallback

### Step 3 (P2): リサイズ latest-only coalescing [課題 5] → ✅ 完了 (2026-05-02)
- **bridge 側**: `notify_host_resize` を pending に入れて control loop で 1 tick 1 回 onSize
  (= 古い notify を消化しない、Bitwig 並みのレスポンス)
- **mIV 側**: `last_resize_notify` で 33ms throttle (= 30fps)
- WM_ENTERSIZEMOVE 中の no-notify は drag 中追従止まるので非推奨
- ack 方式は実装コスト大、まずは latest-only + throttle で十分

### Step 4 (P1): GUI 一括表示の `DeferWindowPos` 化 [課題 1] → ✅ 完了 (2026-05-02)
- `gui.rs` に `show_windows_in_z_order` helper 新設
- `BeginDeferWindowPos` + `DeferWindowPos(SWP_SHOWWINDOW | SWP_NOACTIVATE | ...)`
  + `EndDeferWindowPos` で **show + z-order を 1 batch にアトミック化**
- snapshot HWND を bottom-to-top で積む → 最後の HWND が最前面
- `set_all_guis_visible(true)` 経路で snapshot HWND を優先して使う
- 既存 HWND の再表示では個別 `ShowWindow` を呼ばず、最終 batch のみで表示する
- VST ホストウィンドウに `DWMWA_TRANSITIONS_FORCEDISABLED` を適用し、OS フェードを best-effort で抑制
- fallback は bottom-to-top の個別 SetWindowPos

### Step 5 (P2): VST3 後段の安全 limiter [課題 4]
- [x] VST3 チェーンが active のときだけ、audio-pump 側で後段 safety limiter を適用する。
  - 目的: ユーザーがチェーン末尾に limiter を入れなかった場合の音割れ保険。
  - `cpal` callback ではなく `raw → VST process → processed` の直後で処理し、RT callback に処理を増やさない。
  - lookahead 5ms、ceiling -1dBFS、release 100ms の固定 sample-peak limiter。
  - limiter 遅延は PDC latency に加算し、映像同期に反映する。
- [ ] peak / gain reduction / OVER 表示は後続。Limiter 自体の保護動作とは独立して追加可能。

### 依存関係 (Codex 指摘)
- Step 1 (PDC) と Step 2 (latency 縮小) は同じ audio clock/buffer に触るので
  近い順で実施するのが安全
- Step 3 (resize) と Step 4 (DeferWindowPos) は GUI 領域だが独立、並行可能

---

## 📋 Codex 回答外 / Future Work

### Buffering 中の audio 先行 decode 制限 [P2, Codex 2026-05-01]
- 現状: raw_pending cap=30 秒の safety margin で凌ぐ (`34b877e`)
- 構造的問題: engine_state gate active 中 (= Buffering) は fill_output 非 drain
  → pump は raw_pending に積み続ける → audio decoder の生成速度 ~23x real-time で
  Buffering 1 秒あたり 23 秒分の raw が積まれる
- 30 秒 cap でも Buffering > 1.3 秒 wall で overflow 可能 (= AV1 long GOP / 高負荷時)
- overflow_for_serial が発動するとその seek 世代では音声が復帰しない設計のため、
  本来は overflow を「稀な非常時」に抑える必要あり
- **根本対策の選択肢** (Codex 助言):
  1. **back-pressure**: pump's `recv from audio_rx` に raw_pending soft cap
     (= 5 秒等) を入れる。raw 超過時 pump 待機 → audio_tx (0.7秒) → audio_pkt_tx
     (5.9秒) → demux で頭打ち。Buffering 11.6 秒 wall まで対応可能だが、それを
     超えると demux 詰まりで video 飢餓のリスク
  2. **同一 serial 内 re-arm**: overflow 後も fill_output が drain 始めたら
     overflow_for_serial を解除して pump 再開
  3. **audio decoder 速度制限**: audio decode thread を audio decoder 単位で
     throttle (= ProcessSetup mode 切替 / packet read pacing)
- 関連: `src/video/audio.rs::run_pump`, `RAW_OVERFLOW_SECS`, `RAW_WARNING_SECS`
- 検証: HW D3D11VA H264 動画で `raw_pending high water` ログが頻発するか観察
- 2026-05-03: AVI/DivX 互換の最小対策として、overflow 時に同一 seek 世代を恒久
  drop せず、raw queue を clear して現在 audio frame に re-anchor する処理を実装。
  re-anchor 時は active VST chain も reset して旧 delay-line tail を抑止する。
  `audio_rx` soft-cap back-pressure は引き続き follow-up。

### シーク後の post-seek pre-roll discard [P3, Codex P2-3, 2026-05-01]
- 現状: シーク時に bridge audio thread が `flush_with_silence(latency)` で plugin
  delay-line を silence で埋める → pre-seek 残留は解消
- 副作用: 純粋 latency plugin (例: mIV Test Latency) では post-seek の最初 N samples
  が silence (= delay-line 内 silence の出力)、その後 N samples 経って実 audio が出る
  → **シーク後 ~latency 秒の silence ギャップ**
- ユーザー報告: 「治った」(= silence 許容、pre-seek 漏れより遥かに良い)
- 完全な即時再生:
  1. reset 後、post-seek 実 audio を N samples 先まで pre-load
  2. plugin output (= silence 埋め部分) を discard
  3. その後の output (= delayed 実 audio) を AudioBuffer に流す
- 実装には mIV pump 側の協力 (= pre-roll 供給 + discard モード) が必要
- 関連: `crates/vst3-host/src/plugin_loader.cpp::flush_with_silence`

### reset_sync timeout 時の fence-fail policy [P3, Codex P2-2, 2026-05-01]
- 現状: timeout (= 2 秒) 時は CRITICAL log を出して continue
- 副作用: 後続 process_block が走るので pre-seek tail が一瞬漏れる可能性
- 完全な fail-closed:
  - 該当 bridge を一時 mute (= 出力を silence で埋める) until ack arrives
  - もしくはユーザーに通知 (= 「plugin が応答しない」warning UI)
- 現在は 2 秒 timeout で実用的にはほぼ起きない設計

### ~~VST Instrument / 音声入力なし plugin を一覧から除外する~~ [P2, 2026-04 ユーザー報告] → 🟢 修正済 (2026-05-02)
- 症状: 検出済プラグイン一覧に Instrument 系 (= MIDI 入力で音を生成するシンセ) も
  混在している。mIV は MIDI 入力経路を持たず Effect (音声入力→音声出力) のみ
  使えるので、Instrument を出しても無駄に選択肢が増えるだけ
- 実装:
  - bridge subprocess の `probe` コマンドで `IComponent::getBusCount`
    (`kAudio` input/output, `kEvent` input/output) と `getBusInfo` の channelCount を取得
  - mIV で通常候補に出す条件は **audio input bus > 0 && audio output bus > 0**
  - 同一 bundle 内に複数 class がある場合は、audio input/output を持つ class を優先採用
  - Instrument / MIDI FX / 音声入力なし plugin はデフォルト非表示。必要なら
    「音声入力なしのプラグインも表示」で opt-in
  - probe error / timeout / crash は一覧に `(error)` として表示するが追加ボタンは disabled。
    認証が必要な VST3 は他 DAW で一度認証してから再スキャンしてもらう方針
  - scan/probe は環境設定 UI thread ではなく worker thread で実行し、probe は最大4並列
    (= plugin crash / hang は個別 bridge subprocess drop で隔離)
- 関連ファイル:
  - `crates/vst3-host/src/main.cpp` (= scan コマンド or load 時の応答)
  - `src/video/dsp/scanner.rs` (= DiscoveredPlugin に bus probe 結果を追加)
  - `src/ui_dialogs/preferences.rs` の `page_vst3()` (= フィルタ追加)

### 環境設定 VST3 プラグインページ: 検出済プラグイン一覧のスクロールエリアが正しく拡張されない [P2, 2026-04 ユーザー報告]
- 症状: 環境設定→VST3 プラグインページで、「(556 個検出)」と表示されているにも
  関わらず、ダイアログを広げても下部の検索結果一覧 (= AA_VEQ-MG4+, Acid V, …) が
  10 個程度しか表示されない。スクロールエリアの高さがダイアログ拡張に追従していない
- 推測: `egui::ScrollArea` の `max_height` が固定値で指定されている、または
  `auto_shrink` が逆方向に効いていて、利用可能な縦スペースが活用されていない
- 修正案:
  - `ScrollArea::vertical().auto_shrink([false; 2])` で縦方向の縮小を抑止
  - `.max_height(ui.available_height())` で残り高さいっぱいまで使う
  - もしくは `ui.allocate_ui_with_layout` で残スペースを確保した後に ScrollArea
- 関連ファイル: `src/ui_dialogs/preferences.rs` の `page_vst3()`

### ~~プラグイン GUI 非表示状態の永続化~~ [P1, 2026-04 ユーザー報告] → 🟢 修正済 (2026-05-01)
- 実装: `Vst3PluginEntry.user_hidden: bool` (settings.json) を追加し、
  `add_plugin` 時に slot にコピー。パネル「GUI / GUI ×」ボタンとプラグイン
  ウィンドウ × 押下 (= `pump_gui_signals`) 経路すべてで settings 側を同期。
  `set_all_guis_visible(true)` は元から user_hidden=true スロットを skip する
  ため、起動時自動表示で × したスロットは再表示されない
- 副次変更: `pump_gui_signals` の戻り値を `Vec<usize>` (= user_hidden 化された
  idx 一覧) に変更し、App 側 wrapper (`vst3_pump_gui_signals`) で settings.save()
- 関連: `vst3_gui_visible` (= VST ボタン全体の ON/OFF) は既に永続化されている

### ~~プラグイン内部状態の永続化 (= EQ カーブ等の保存)~~ [P1, 2026-04 ユーザー報告] → 🟢 修正済 (2026-05-01, Codex P2-1/2/3 反映済)
- 実装: bridge protocol を拡張 (`Cmd::QueryState` / `Cmd::RestoreState` /
  `Event::PluginState`) して `IComponent::getState` / `setState` を base64 で
  IPC する。`MAX_CONTROL_MSG_SIZE` を 64 KB → 4 MB に拡張 (= ML 系 / preset 内蔵
  plugin の大きい state にも対応)
- 保存タイミング: `on_exit` (= アプリ終了直前)、VST3 OFF へのトグル直前、
  チェーン構成変更による rebuild 直前。いずれも snapshot → save の順
- **復元タイミング (Codex P2-3 反映)**: 初回 auto-restore は **`Cmd::Open` の
  `state` field に bake** して bridge 側で `audio_thread` 起動前に適用
  (= 完全シングルスレッド、race-free)。`Cmd::RestoreState` は runtime restore
  用に残し、audio thread fence 経由で実行
- **audio thread fence (Codex P2-2 反映)**: `query_state` / `restore_state` は
  bridge audio thread の loop 境界 (= read 後・process 前) で実行する。
  control thread はフラグを立てるだけ。これで `process()` と
  `setState`/`getState` の並走が排除され、VST3 plugin の thread safety を担保
- **OFF トグル時の guard 修正 (Codex P2-1 反映)**: snapshot helper の guard を
  `settings.vst3_enabled` から `dsp_bridge.is_enabled()` に変更
  (= preferences OK で settings 切替 **後** に呼ばれるパスでも teardown 前に
  state を取得できる)
- C++ 側: `PluginLoader::query_state` / `restore_state` を `MemoryStream` 経由で
  実装。`restore_state` は RAII `ProcessingPauseGuard` で `setProcessing(false)
  → setState → setProcessing(true)` を必ず対称化

### ~~プラグイン GUI ウィンドウ位置の永続化 + 非 resizable プラグインのダブルクリック最大化抑止~~ [2026-05 ユーザー要望] → 🟢 修正済 (2026-05-01)
- ウィンドウ位置: `Vst3PluginEntry` に `gui_pos: Option<(i32, i32)>` /
  `gui_size: Option<(u32, u32)>` を追加。`add_plugin` に `initial_window_pos`
  引数を追加して `PluginSlot.desired_window_pos` に格納、`show_slot_gui` の
  新規ウィンドウ作成時に `gui::create_window` の `initial_pos` 引数として渡す。
  保存は state snapshot と同じトリガ (on_exit / OFF / chain rebuild) で
  `GetWindowRect` を呼んで settings に書き戻す
- ダブルクリック最大化: `WS_OVERLAPPEDWINDOW` から `WS_MAXIMIZEBOX` を
  非 resizable プラグインで抹く。これでタイトルバーのダブルクリックも無効化
  され、SSL Meter Pro 等の固定サイズプラグインで「外枠だけ最大化されて中身が
  追従しない」紛らわしい挙動が解消

### 右クリックメニュー即閉じ問題 (SSL Meter Pro) [P1, 既知]
- bridge 側で plugin child window を `EnumChildWindows` + subclass する案を
  Codex が前回提示
- 実装規模が大きい (= async enum + WinEvent hook)
- 現在は WM_PARENTNOTIFY 経由の `SetForegroundWindow` のみ (= 効果なし)

### Insight2 リサイズ中の中身遅延 [P3, 既知]
- WM_ENTERSIZEMOVE 経由のセッション抑止で大幅改善
- 残るのはプラグイン側のレンダリングラグ (= host で完全に抑えるのは難しい)

---

## 🤔 設計議論中

### CLAP 対応
- v0.10.0 以降で検討 (= ユーザーの手持ちは VST3 中心なので優先度低)

### マルチプラグインチェーンのレイテンシ合算
- 現状: 各 plugin が独立に latency 申告するが、合算した PDC 未実装
- PDC 実装と合わせて設計

### exclusive WASAPI モード
- 現状: WASAPI Shared (~10-20ms latency)
- exclusive にすれば <5ms 可能だが他アプリと共存できない
- v0.10.0 以降の検討事項

---

## 🚫 Deferred / 別リリース

- マルチプラグインチェーン (実装済、上限 10 個に制約)
- VST3 SDK 法務確認 (= MIT 化済、解決済)
- bench / perf-log への VST3 IPC 計測組み込み
- VST3 GUI の DPI スケーリング詳細追従 (Per-Monitor v2)

---

## ユーザーフィードバックの履歴

- 2026-04: 起動時音声グリッチ → audio buffer 縮小で解消
- 2026-04: ON/OFF 連打で固まる → detach thread + persistent GuiHost で解消
- 2026-04: 文字化け (□1, など) → CLAUDE.md ポリシー化 + lint
- 2026-04: パネル白背景 → custom Frame で解消
- 2026-04: ドラッグ位置リセット → fixed Id で解消
- 2026-04: 表示が遅い → persistent GuiHost で DAW 並みに改善
- 2026-04: SSL Meter Pro 右クリック → ⏳ 課題 (Codex 提案 child subclass 待ち)
- 2026-04: フルスクリーン解除後も GUI 残る → 自動 cleanup で解消
- 2026-04: 環境設定で追加した plugin が保存されない → overwrite 撤去で解消
- 2026-04: ツールバー VST ボタン不要 → 削除
- 2026-04: z-order 登録順に戻る → snapshot/restore で解消
- 2026-04: GUI × したものが VST トグルで復活 → user_hidden で解消
- 2026-04: VST EQ 反映遅延 → buffer 1.5s → 300ms で改善
- 2026-05: VST EQ 反映遅延がまだワンテンポ遅い → 150ms に追加縮小、動作問題なし
- 2026-05: 150ms は動作問題なし → 100ms に追加縮小、反応改善・動作問題なし
- 2026-04: パラパラ表示 + チラつき → ✅ DeferWindowPos 化 + 個別 ShowWindow 回避 + DWM transition 抑制で改善
- 2026-04: プラグイン × も user_hidden で記憶 → 解消
- 2026-04: ピーク超過時の挙動 → ⏳ 課題 (Codex に質問中)
- 2026-04: プラグイン内部状態未保存 → 📋 未着手 (= bridge protocol 拡張要)
- 2026-04: Insight2 リサイズで「バッファに詰まった更新を再生」挙動 → ✅ latest-only + throttle で改善
  (Bitwig 比較で発覚、throttle / back-pressure 検討)

---

## メモ: 検証用テストプラグイン

ユーザーが自作した「mIV Test Latency」プラグイン:
- 特定 sample 数の固定遅延を返すだけ
- PDC 実装の動作確認に使う
- 動画と音声の同期が取れるかを目視 + 計測で確認可能

## 2026-05 background startup load

- Done: startup VST3 chain load now runs on a dedicated `vst3-startup-load`
  worker instead of blocking the general startup-init path.
- Image browsing can start before VST3 loading finishes.
- If the user opens a video while the VST3 startup load is still running,
  fullscreen shows a black loading state with VST3 progress text and starts the
  video automatically once the worker completes.
- Remaining follow-up: add a bridge batch-load command if startup VST3 loading
  should become parallel. The current Rust-side command path still loads the
  configured chain sequentially.

## 2026-05 editor chrome

- Phase 1 done: bridge-owned VST3 editor windows request DWM dark caption,
  text, and border colors after `CreateWindowExW`. This removes the bright
  native white title bar without changing the editor HWND ownership model.
- Phase 2 started: the bridge editor surface is split into an outer mIV frame
  HWND and an inner child HWND used for `IPlugView::attached()`. The outer frame
  draws a black title bar with plugin name, latency, and a close button, while
  drag/resize/z-order still target the outer HWND.
- Follow-up: a fully custom title bar with inline power/bypass and latency
  control still needs a Rust-side bypass IPC event and settings sync.

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

### Step 2 (P1): 音声 buffer 縮小 (300ms → 120-150ms) [課題 2]
- `TARGET_BUFFER_SECS: 0.3` → `0.12 (Windows) / 0.20 (other)` を提案
- 100ms 未満は実機計測必須、現時点では攻めすぎ
- pump の sleep 10ms → 2ms に短縮 (= "低水位追従")
- cpal callback 内 IPC は **P3** (= まだ入れない、deadline miss リスク高)
- どうしても入れるなら `try_process_block(deadline=2ms)` + bypass fallback

### Step 3 (P2): リサイズ latest-only coalescing [課題 5]
- **bridge 側**: `notify_host_resize` を pending に入れて control loop で 1 tick 1 回 onSize
  (= 古い notify を消化しない、Bitwig 並みのレスポンス)
- **mIV 側**: `last_resize_notify` で 33ms throttle (= 30fps)
- WM_ENTERSIZEMOVE 中の no-notify は drag 中追従止まるので非推奨
- ack 方式は実装コスト大、まずは latest-only + throttle で十分

### Step 4 (P1): GUI 一括表示の `DeferWindowPos` 化 [課題 1]
- `gui.rs` に `show_windows_in_z_order` helper 新設
- `BeginDeferWindowPos` + `DeferWindowPos(SWP_SHOWWINDOW | SWP_NOACTIVATE | ...)`
  + `EndDeferWindowPos` で **show + z-order を 1 batch にアトミック化**
- snapshot HWND を bottom-to-top で積む → 最後の HWND が最前面
- `set_all_guis_visible(true)` 経路で snapshot HWND を優先して使う
- fallback は bottom-to-top の個別 SetWindowPos

### Step 5 (P2): peak indicator [課題 4]
- まず **表示だけ** (= "OVER" 表示)、soft limiter は設定付きで後続
- fill_output 内で `peak = peak.max(y.abs())` を AtomicU32 に publish
- UI 側 (パネル / HUD) で 200-500ms ホールド表示
- soft limiter (`x / (1 + x.abs() * 0.25)`) は設定で OFF 可能に

### 依存関係 (Codex 指摘)
- Step 1 (PDC) と Step 2 (latency 縮小) は同じ audio clock/buffer に触るので
  近い順で実施するのが安全
- Step 3 (resize) と Step 4 (DeferWindowPos) は GUI 領域だが独立、並行可能

---

## 📋 Codex 回答外 / Future Work

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

### VST Instrument (MIDI 入力) を一覧から除外する [P2, 2026-04 ユーザー報告]
- 症状: 検出済プラグイン一覧に Instrument 系 (= MIDI 入力で音を生成するシンセ) も
  混在している。mIV は MIDI 入力経路を持たず Effect (音声入力→音声出力) のみ
  使えるので、Instrument を出しても無駄に選択肢が増えるだけ
- VST3 SDK では `IPluginFactory2::PClassInfo2.category` または `PClassInfoW.category`
  を見れば判別可能:
  - Effect: `kVstAudioEffectClass` (= "Audio Module Class")。`subCategories` に
    "Fx" 系のキーワードが入る (例: "Fx|EQ", "Fx|Dynamics", "Fx|Spatial" 等)
  - Instrument: 同じ `kVstAudioEffectClass` だが `subCategories` に "Instrument"
    系 (例: "Instrument|Synth", "Instrument|Sampler", "Instrument|Drum") が入る
- 修正案:
  - `crates/vst3-host/src/plugin_loader.cpp` (もしくは scanner) で classInfo の
    subCategories を読み、"Instrument" を含むものに `is_instrument: bool` フラグ
  - mIV 側 `DiscoveredPlugin` 構造体に `is_instrument: bool` を追加して
    scanner result に渡す
  - 環境設定 VST3 プラグインページで Instrument 系をデフォルト非表示
    (= 「Instrument も表示」チェックボックスを設けて opt-in にしてもよい)
- 関連ファイル:
  - `crates/vst3-host/src/main.cpp` (= scan コマンド or load 時の応答)
  - `src/video/dsp/scanner.rs` (= DiscoveredPlugin に is_instrument 追加)
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

### プラグイン内部状態の永続化 (= EQ カーブ等の保存) [P1, 2026-04 ユーザー報告]
- VST3 `IComponent::getState` / `setState` chunk のシリアライズ
- 現状: `Vst3PluginEntry::state: Option<String>` フィールドは settings に存在
  するが **bridge protocol 未実装** (= chunk の query / restore コマンドが無い)
- 追加が必要なコマンド:
  - `Cmd::QueryState` → bridge が plugin の getState chunk を base64 で返す
  - `Cmd::RestoreState { state: String }` → bridge が setState で復元
- mIV 終了時 / ダイアログ閉じる時に query → settings に保存
- 起動時 / プラグインロード時に settings から restore

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
- 2026-04: VST EQ 反映遅延 → buffer 1.5s → 300ms で改善 (さらなる縮小は ⏳)
- 2026-04: パラパラ表示 + チラつき → ⏳ DeferWindowPos 化 Codex 検討中
- 2026-04: プラグイン × も user_hidden で記憶 → 解消
- 2026-04: ピーク超過時の挙動 → ⏳ 課題 (Codex に質問中)
- 2026-04: プラグイン内部状態未保存 → 📋 未着手 (= bridge protocol 拡張要)
- 2026-04: Insight2 リサイズで「バッファに詰まった更新を再生」挙動 → ⏳ 課題 5
  (Bitwig 比較で発覚、throttle / back-pressure 検討)

---

## メモ: 検証用テストプラグイン

ユーザーが自作した「mIV Test Latency」プラグイン:
- 特定 sample 数の固定遅延を返すだけ
- PDC 実装の動作確認に使う
- 動画と音声の同期が取れるかを目視 + 計測で確認可能

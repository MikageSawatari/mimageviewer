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

(現在なし)

---

## ⏳ Codex 回答待ち

`docs/codex-vst3-bug-brief.md` で第 2 弾調査依頼済。
回答は `docs/codex-vst3-bug-answer.md` に上書き予定。

### 課題 1: 一括表示時の "パラパラ" + z-order 復元時のチラつき (P1)
- 各 GUI が SW_SHOWNA で 1 つずつ現れ、最後に z-order が並び変わる
- 期待: 最初から正しい z-order で同時表示 (= no flicker)
- 検討: `BeginDeferWindowPos` / `DeferWindowPos` でアトミック化

### 課題 2: 音声 latency をさらに縮小 (P1)
- 現状 300ms。さらに短くしたい (DAW は ~10-50ms)
- 検討: cpal callback 内で plugin 処理 vs 別スレッド (= 現行)
- bridge プロセス境界が前提でどこまで詰められるか
- ユーザー提案: 「OS に渡す直前で処理」の実現性

### 課題 3: PDC (Plugin Delay Compensation) 実装 (P1)
- VST3 plugin の `getLatencySamples()` を host 側で **未補正**
- 「mIV Test Latency」プラグインで A/V ズレを検証済
- audio anchor 設計を踏まえた補正案が必要
- LatencyChanged event の処理 (= 動的変化)

### 課題 4: ピーク超過時の clip 挙動 (P2)
- 現状: f32 を OS にそのまま渡す → OS mixer / DAC で hard clip
- DAW では soft limiter / brickwall limiter を出力段に挟む
- 視覚的 clip indicator (= "OVER" 表示) も検討

### 課題 5: リサイズイベントのスロットリング (P2)
- ユーザー報告: Bitwig だと Insight2 のリサイズが速い、mIV だと「バッファに
  詰まったリサイズを時間をかけてトレース」する挙動
- 仮説: notify_host_resize が plugin 処理速度より速く積み重なる
- 検討: throttle (~50ms) / back-pressure (= ack 待ち) / drain (= 古い notify
  skip) / WM_EXITSIZEMOVE のみ送信

---

## 📋 未着手 / Future Work

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

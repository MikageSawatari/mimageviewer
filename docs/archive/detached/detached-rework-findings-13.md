# 検収所見 #13: F12 切り離し瞬間のメイン窓側フラッシュ (既知 P3 の本修正)

正本プラン: [../../detached-rework-plan.md](../../detached-rework-plan.md)
前提: findings-12 (D1/D2/D3) 適用済み。実機 (2026-07-07 12:5x) で複数窓の切替/close は
解消確認済み。残 = **複数ウィンドウ OFF で F12 を繰り返すと、切り離した瞬間に
「ウィンドウの場所以外」がちらつく**。

## E1: 症状の位置づけ (既知 P3 の再浮上)

- 2026-07-06 の動画フレーム解析 (bug-20260706-2254-f12-flicker.log + 22-54-24.mkv
  frame 247/317) で確定済み: **F12 遷移でメインウィンドウが 1 フレーム「クリア色のみ」
  (ライトテーマで白) になる**。当時 P3 (polish、非ブロッキング) として保留し、
  修正案 = holdover パターン流用とだけ記録した。
- 今回 (2026-07-07 12:5x セッション、F12 トグル ×58) のログを Fable が解析:
  detach 遷移の lifecycle 機構は完全にクリーン (font resync 発火なし・host_lost なし・
  detached 窓側は A2 の可視化ゲートが content ready まで非表示を維持)。
  = **ライフサイクル/HWND 系の残バグではなく、メイン viewport の描画・present レベルの
  1 フレーム穴**。旧来は黒フラッシュとして存在していた可能性が高く (テーマ連動)、
  リワーク退行ではない polish 項目。
- ユーザーが smoke で継続的に気にしている症状なので、保留を解いて本修正する。

## 進め方 (憲法 6: 証拠を先に)

### Phase 1: クリア色フレームの発生源を特定する (調査、修正しない)

「メイン viewport が UI を描かずに present される瞬間」がどこで生じるかを絞る。
候補 (Fable の仮説、優先順):

1. **メイン pass の paint スキップ + present**: `App::update` がメイン UI を描かず return
   する経路 (font resync の defer 系・presentation 切替フレームの early-return 等) でも、
   eframe はそのフレームの surface を present する場合がある。描画ゼロ + present =
   クリア色のみのフレーム。F12 detach フレームで main viewport の
   「描いた shape 数 / early-return 経路」をログして相関を取る。
2. **fs viewport hide → メイン再表示の初回フレーム**: fullscreen viewport の
   `Visible(false)` でメイン窓が露出する瞬間、メイン側の swapchain が
   再構成 (resize / stale 破棄) されて最初の present がクリア色になる可能性。
   eframe/wgpu の surface (re)configure のタイミングを計装で確認する。
3. DWM 側の合成タイミング (mIV から制御不能な要素)。1/2 が白でなければ消去法で報告。

計装は `MIV_DETACHED_WINDOW_DEBUG=1` 時のみ有効でよい。F12 detach の前後 5 フレームで
(a) メイン viewport の update が主要 panel を描いたか (b) present が起きたか
(c) fs viewport の Visible コマンド発行タイミング、を 1 行ずつ出す。

**ユーザー再現 1 回** (F12 連打 + 時計入り録画) → 白フレームのタイムスタンプと
計装ログを突き合わせて発生源を確定する。

### Phase 2: 修正 (Fable 承認後)

発生源に応じて:

- 候補 1 なら: 該当 early-return 経路でメイン UI を「最後に描いた内容の holdover」で
  埋めるか、present をスキップする (空フレームを画面に出さない)。既存の
  holdover パターン (fs 側で実績あり) を流用。
- 候補 2 なら: fs viewport の hide をメイン側の最初の有効フレーム present 後に遅らせる
  (A2 の「初回描画まで非可視」と対称の順序保証)。時間窓は使わない。

## Phase 1 結果 (Fable 解析 2026-07-07): 候補 1/2 を棄却、候補 3 (scanout レベル) と確定

計装 (3bc9ce77) + 実機再現 (録画 = 13-22-56.mkv、ログ 0〜220s) の解析:

1. **アプリの paint 層は無実**: `main_flash_probe` の全 2681 フレームで、メイン viewport は
   毎フレーム「main_ui_drawn」または「early_return reason=embedded_fullscreen_or_pending
   (= embedded holdover を描画)」のどちらかで終わっている。描画なしで終わったのは
   frame 2105 の 1 つだけで、これは Escape でのフルスクリーン終了 + load_folder の遷移
   フレーム (F12 と無関係)。= 候補 1 (paint スキップ + present) は棄却。
2. **合成 (composition) 層も無実**: ユーザーの画面キャプチャ録画に異常フレームが
   写っていない。キャプチャは合成後のフレームを取得するので、合成結果はクリーン。
   = 候補 2 (swapchain 再構成でクリア色 present) も棄却 (present されていれば録画に写る)。
3. **消去法で候補 3**: 知覚される「黒い線 (左端〜中央、中央より少し下、毎回ではない)」は
   **合成より下の層 = スキャンアウト/ドライバ/DWM flip 遷移レベル**のアーティファクト。
   F12 detach の瞬間 = 新しい OS 窓の出現で DWM の合成プラン (MPO / independent flip) が
   切り替わるタイミングに、物理ディスプレイ上でのみ一瞬のティア状の線が出るもの。
   部分幅の水平線・間欠性・キャプチャ不可視、のすべてがこの層の特徴と一致する。

**含意**: アプリの描画コードから観測も直接修正もできない層であり、2026-07-06 に
1 フレーム白 (こちらはキャプチャに写った = 合成層) として見えていた事象とは別物。
白フラッシュ自体は今回の録画に写っていない = 既に消えている可能性が高い
(A2 の可視化ゲート等の副次効果)。

### 切り分けテスト (ユーザー、任意)

detached 固有かを判定する: 同じモニターで **通常のフルスクリーン (Enter/Escape) を連打**
して同じ黒い線が知覚されるか。出るなら「OS 窓の出現一般」で起きる環境レベルの事象で、
detached リワークとは無関係と確定する。

### 処置 (Fable 提案)

- Phase 2 (アプリ側修正) は**実施しない** (修正対象がアプリの外)。
- 本件は **P3 polish / 環境依存としてクローズ**し、出荷ゲートのブロッカーにしない。
- 計装 (main_flash_probe) は MIV_DETACHED_WINDOW_DEBUG ゲート内なので残置してよい。

## 完了条件 (改訂)

- [x] Phase 1: 計装追加 (3bc9ce77) + 再現ログ/録画で発生源確定 → **候補 3 (scanout
      レベル・アプリ外) と確定、Phase 2 は実施せずクローズ**
- [ ] (任意) 切り分けテスト: Enter/Escape 連打で同症状が出るかの確認

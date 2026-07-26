# 検収所見 #6: CUT-1 (フォント消失) は調査フェーズへ切替 — 推測修正の禁止

正本プラン: [../../detached-rework-plan.md](../../detached-rework-plan.md) /
経緯: [findings-5](detached-rework-findings-5.md) CUT-1。fix1 (f966a68c) で CUT-2
(host ループ) は解消したが、CUT-1 は実機で再発 (「操作したり、しばらくすると
表示されることもある」)。同一症状への修正 2 連続不決着のため、**コードを変更する
前に調査で根因を確定する** (R2d Phase 0 方式)。

## 背景: この問題は 2 層構造 (Fable 整理)

1. **破壊源** (未特定): OFF モード F12 の前後で main viewport の font atlas が
   desync する。何のイベントがどのパスで壊すのかが分かっていない。
2. **修復ポリシーのジレンマ** (構造既知):
   - detached 生存中に `set_fonts` を撃つと別 viewport 描画と競合して
     Y-32 panic (2026-06-29 実害) → R1c で「detached 生存中は resync 遅延」
   - しかし OFF モード F12 運用では detached 窓がほぼ常時生存 → **壊れた文字の
     修復が窓を閉じるまで走らない** (ユーザーの「しばらくすると直る」= close 時の
     cleanup resync)
   - fix1 の「deferred queue へ移す」は方針に忠実だが、症状の持続を保証してしまう

→ 破壊源を潰せば修復頻度の問題は消え、修復を安全化すれば壊れても即直る。
**どちらを直すべきかは調査結果で決まる。**

## Phase 0: 調査 (コード変更はログ追加のみ。修正は Fable 承認後)

### 0-1. 机上調査: egui/eframe 0.33 の multi-viewport font texture 配送

~/.cargo/registry の egui 0.33.3 / egui-wgpu / eframe wgpu_integration を読み、
以下を**ソース引用付き**で確定する:

- viewport ごとの Renderer (texture コピー) は共有か個別か
- font atlas の `TexturesDelta` (full / partial) は**どの viewport のパス出力に
  載り、他 viewport の renderer へどう届くのか** (egui は viewport ごとに
  texture 世代を追跡しているか)
- `request_discard` で破棄されたパスの `TexturesDelta` は失われるのか、
  次パスに引き継がれるのか
- `set_fonts` 後の full upload が「あるパスで消費されたが、そのパスの描画先
  viewport が main ではない / 描画されない」場合に何が起きるか

これで「main だけ文字が消える」が egui 仕様上どう起こり得るかの候補が絞れる。

### 0-2. 計装: パス単位のフォントテクスチャ追跡ログ

`MIV_DETACHED_WINDOW_DEBUG=1` 配下に追加 (使い捨て、修正時に整理):

- 各 root パス: frame/pass 番号、`will_discard`、resync pending/repeats、
  detached 窓の生存数
- `configure_fonts_for_texture_resync` (set_fonts) の呼び出し (既存 generation
  ログの拡充で可)
- 可能なら TexturesDelta の観測 (font texture id の set/free が egui 側 API で
  観測できるか 0-1 で確認。できなければ eframe ログに頼らず、resync 発火と
  F12 イベントの正確な相対順序だけでも取る)
- F12 toggle / detached open/close / fullscreen viewport close の各イベント
  (既存ログで足りるか確認)

### 0-3. 実機再現 1 回 (ユーザー)

計装ビルドで OFF モード F12 を数往復して文字消失を再現し、**発生した瞬間の時刻を
メモ** + 即ログ退避。直後に「detached 窓を閉じると直るか」も確認 (修復ポリシー
仮説の裏取り)。

### 0-4. 報告 → Fable 承認

仮説を「破壊源」と「修復ポリシー」に分けて報告する。修正案は承認後に実装。
検討済みの修正候補 (参考、先取り実装禁止):

- 破壊源側: F12 経路の該当パスの是正 (調査結果次第)
- 修復側: detached 生存中は discard なしの resync (`without paint defer` 分岐の
  流用) を許可する — ただし **Y-32 panic を再発させない根拠** (0-1 の配送仕様で
  安全性を説明できること) が必須条件

## 制約

- 憲法 6: 症状を見てのその場パッチ禁止。本所見の Phase 0 完遂が先。
- `global_search::tests::cancel_stops_early` の PermissionDenied は既知の環境
  フレーク疑い (過去にも並列実行時のみ赤)。今回「単独でも赤」とのことなので、
  Phase 0 の合間に 1 度だけ `cargo clean` なしで再実行して再現性を記録する
  (本件と混同しない。直すのは別タスク)。

---

## Phase 1: 修正指示 (Fable 承認 2026-07-06 深夜、Phase 0 結果 + 実機ログ解析に基づく)

### 確定した機構 (証拠付き)

1. Phase 0-1 (Codex 机上調査): `TexturesDelta` はグローバルで egui 内では discard で
   失われないが、**eframe の per-viewport `paint_and_update_textures` が surface 不在等で
   early-return すると texture 更新前に抜け、delta はそこで失われる**。renderer は共有
   なので、失われた full-upload は他の誰も再適用しない。
2. 実機ログ (bug-20260706-2222-cut1-diag.log): `native_video_backdrop_hide` 理由の
   resync (set_fonts full-upload ×5 世代) が、**F12 OFF (動画 detached→main) の
   placement 切替チャーンの最中** (窓 Removed の 60ms 後、native presenter 再構築・
   cloak/backdrop 遷移中) に毎回発火している (5.2s / 30.5s / 32.3s ほか計 5 回)。
3. 帰結: 修復用 full-upload が「描画がバックエンドに届かないフレーム」に消費されて
   消える → 共有 renderer の atlas は旧のまま、egui は新 atlas 前提の UV → 全文字
   消失。次に成功する full-upload (窓 close 時の cleanup resync 等) まで直らない —
   「操作したり、しばらくすると表示される」と一致。

### 追加確定 (Codex 動画×ログ突き合わせ、2026-07-06 深夜)

- 録画 22:22:55 = **静止画 detached の F12 OFF** 直後に main の文字が灰色矩形化
  (ブラウザ正常 = mIV main viewport の font texture 破損)。
- **静止画の F12 OFF cleanup でも `hide_native_video_black_backdrop_if_shown()`
  が呼ばれ、その中の `request_main_font_atlas_resync("native_video_backdrop_hide")`
  が発火している** ([ui_fullscreen.rs:6922](../src/ui_fullscreen.rs) / [:7337](../src/ui_fullscreen.rs))。
  動画用の resync が静止画経路に相乗りしていた。
- 破壊タイミングは「detached 生存中に遅延できていない」ではなく、**detached が
  閉じた直後に許可された即時 resync** が churn フレームに落ちて食われている。

### 修正内容 (2 本立て、どちらも必須)

**修正 A: 発火元の分離 (Codex 提案採用)**

- `hide_native_video_black_backdrop_if_shown()` を「本物の native video backdrop
  hide」と「静止画/PDF detached cleanup」に分離し、**静止画/PDF の F12 OFF では
  `native_video_backdrop_hide` の font resync を発火させない**。
- 静止画 cleanup 側に resync が本当に必要か (= 静止画 F12 OFF で delta が食われる
  実害があるか) を実機で確認し、必要なら別 reason で safe-frame ゲート (修正 B)
  経由にする。不要なら発火なし。判断根拠を完了報告に書く。

**修正 B: set_fonts 発火の「安全フレーム」ゲート (状態ベース、時間窓禁止 = 憲法 5)**

`maybe_defer_for_main_font_atlas_resync` の発火条件に safe-frame 述語を追加する。
修正 A 後も残る正当な発火 (本物の動画 backdrop hide、cleanup 等) も切替チャーン中に
撃たれる構図は同じため、こちらも必須:

- 動画の placement 切替が pending でない (既存状態)
- main window が cloak / backdrop 遷移中でない (既存フラグ)
- detached runtime に **Opening の窓がない**
- detached runtime に **Closing の窓がない** (close コマンド送信直後のフレーム含む。
  死につつある viewport の最終パスが delta を持ち逃げする経路を塞ぐ。
  Codex の「閉じた直後」観測に対応する条件)
- 既存の「detached 完全 idle まで待つ」deferred 機構は、この settled 判定に
  **置き換える** (OFF モードで窓が開いたままでも修復可能に)
- 条件を満たさない間は pending を保持し (`repeats` を消費しない)、満たした
  フレームで従来どおり 5 世代発火する
- Y-32 安全性の根拠: renderer が共有 (Phase 0-1) なので、settled フレームで
  full-upload が 1 回成功すれば全 viewport が同じ texture を見る。危険なのは
  「full-upload が食われた後に partial が来る」ケースで、本ゲートは full-upload を
  食われないフレームに置くことでそれを断つ。

### テスト・検証

- safe-frame 述語の単体テスト (切替 pending / cloak / Opening あり → 発火しない、
  settled → 発火し repeats が減る)
- 診断計装 (pass_probe 等) は実機合格まで残す。合格後に間引いて恒久化するものを
  選別 (set_fonts 発火ログは恒久で残す)
- 実機 (ユーザー): OFF モードで F12 を動画・PDF 各 4 往復 → **文字が消えない**。
  消えた場合は即ログ退避 (診断で発火フレームの状態が特定できる)
- コミットに `(detached-rework CUT fix2)` を含める

---

## Phase 1 実装メモ (CUT fix2)

### A. 発火元分離

- `hide_native_video_black_backdrop_if_shown()` は動画 startup/backdrop cleanup 専用に残し、
  `native_video_backdrop_hide` の font-atlas resync を発火する。
- 静止画/PDF の in-window 復帰で古い fullscreen/detached viewport を隠す処理は
  `hide_embedded_still_viewport_if_shown()` へ分離し、**font-atlas resync を発火しない**。
- 判断根拠: 静止画/PDF の cleanup はフォント定義・font texture を変更しておらず、
  実機ログではこの経路から動画用 resync に相乗りしたこと自体が main atlas 破損の
  発火源だった。したがって静止画側に別 reason の resync は現時点では不要。

### B. safe-frame gate

- `maybe_defer_for_main_font_atlas_resync()` は、次の settled 条件を満たすまで
  `set_fonts` を発火しない:
  - `native_video_mode_switch` / `pending_detached_video_host_switch` /
    `pending_detached_video_host_resync` / `native_video_source_swap_pending` が無い
  - main cloak または native video backdrop 遷移中ではない
  - detached runtime に `Opening` が無い
  - detached runtime に `Closing` が無い
- unsafe frame では pending を保持し、`repeats` を消費しない。settled frame に入ったら
  従来どおり repeat full-upload を開始する。
- 旧「detached renderer が完全 idle になるまで待つ」判定は、この safe-frame 判定へ置換。
  これにより detached 窓が安定して存在していても main atlas を修復できる。

### 回帰テスト

- 静止画/PDF cleanup が video backdrop resync を要求しないこと。
- stable passive detached window は resync をブロックしないこと。
- placement switch pending / cloak / Opening / Closing で safe-frame が false になること。
- unsafe frame では repeat budget を消費せず、settled frame で初めて消費すること。

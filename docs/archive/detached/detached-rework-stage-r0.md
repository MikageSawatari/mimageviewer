# Stage R0 指示書: child viewport の HWND を geometry 非依存で取得できるかのスパイク

正本プラン: [../../detached-rework-plan.md](../../detached-rework-plan.md)
**着手前に必ずプラン §2 (憲法) を読むこと。**

- 種別: 調査スパイク (本体の動作コードを恒久変更しない)
- 実装: Codex / 検収: ClaudeCode (Fable) / 実機実行: ユーザー
- 前提: 振動バグの応急修正 (rect 捕捉への passive HWND 除外) が commit 済みで
  作業ツリーが clean であること。

## 1. 目的

detached viewer の最重要欠陥 BA-1 (提案書
[../../detached-viewer-lifecycle-redesign-proposal.md](../../detached-viewer-lifecycle-redesign-proposal.md) §2)
の根治には、egui の child viewport (immediate viewport) に対応する **Win32 HWND を、
ウィンドウの見た目の矩形に依存せずに 1 回で確定する**手段が要る。本ステージでその
手段を確定し、R1 (rect 捕捉の全廃) の実装方式を決める。

## 2. 背景 (現状の仕組み)

- 現在は `find_visible_thread_window_matching_rect` (src/dwm_transitions.rs) が
  `EnumThreadWindows` で同スレッド可視窓を列挙し、「期待矩形の中心を含む + 面積 2/3 +
  四隅距離最小」で HWND を推定している。生成直後 (既定サイズ 822x656)・リサイズ中・
  DPI 変化・多窓同居で誤同定する (提案書 §2 BA-1)。
- 使っているスタック: eframe 0.33 (wgpu backend) + egui 0.33。detached viewer は
  `show_viewport_immediate` で出している (呼び出し箇所は src/ui_fullscreen.rs)。

## 3. 調査項目

優先度順。上から順に検証し、成立した時点で残りは「不成立の理由の確認」程度でよい。

### 3.1 (本命) eframe / egui-winit が viewport の HWND を公開しているか

- `~/.cargo/registry` の eframe 0.33 / egui-winit 0.33 ソースを読む。immediate viewport
  の `winit::window::Window` がどこに保持され、アプリ側から到達できる public API が
  あるかを確認する。候補:
  - `egui::ViewportInfo` / `ViewportOutput` に native handle 相当のフィールドはあるか
  - `eframe::Frame` の `window_handle()` (raw-window-handle) は main viewport 以外にも
    使えるか
  - immediate viewport のクロージャ内から `raw_window_handle` を引く経路はあるか
- **egui/eframe の fork・パッチは選択肢に入れない** (保守コスト過大)。public API
  限定で判断する。

### 3.2 (代替の本命) 生成直前後の EnumThreadWindows 差分法

提案書 §5.3 の方式。3.1 が不成立の場合の本命。

- `show_viewport_immediate` を初めて呼ぶ**直前**に同スレッドの HWND 集合 S0 を採取、
  呼んだ**直後** (同フレーム内、クロージャ実行後) に S1 を採取し、`S1 - S0` の新規窓を
  その viewport の HWND として確定する。
- 検証すべき点:
  - egui は同フレーム内で OS 窓を同期的に生成するか (直後の S1 に必ず現れるか)。
    現れない場合、何フレーム後に現れるか / 「生成完了」をどのシグナルで知るか。
  - 差分が 2 窓以上になるケースはあるか (IME 窓・ツールチップ窓・DWM の一時窓など
    winit が同スレッドに作る副次窓の有無)。クラス名 (`GetClassNameW`) で winit の
    トップレベル窓だけにフィルタできるか。
  - 既存の `disable_transitions_for_thread_windows` (src/dwm_transitions.rs) が同種の
    列挙をしているので実装の参考にする。

### 3.3 (参考調査) その他の代替

3.1 / 3.2 の比較材料として簡潔に当たりだけ付ける (深掘り不要):

- winit ウィンドウのクラス名 + 生成順序による同定
- `SetWindowsHookExW(WH_CBT)` による生成フック (侵襲的なので原則不採用、比較用)

## 4. プロトタイプ要件

確定した方式 (3.1 または 3.2) について、実験コードで以下を実証する:

1. detached viewer を F12 で開いたとき、新方式が HWND を 1 回で確定できる。
2. **旧方式 (rect 捕捉) と並走ログ**: 同じフレームで旧 `capture_detached_viewer_host_
   hwnd_from_logical_rect` の結果と新方式の結果を両方ログに出し、一致/不一致を記録
   する形にする (`MIV_DETACHED_WINDOW_DEBUG=1` 配下)。判定はログのみで、**動作には
   新方式を一切使わない** (挙動不変)。
3. 次の状況で新方式の HWND が安定していることをログで確認する:
   - detached 窓のリサイズ・移動・最大化
   - passive 窓 2 枚 + active 窓 1 枚の同居 (誤同定しやすい構成)
   - F12 での main↔detached 往復 (動画でも 1 回)
   - Ctrl+↑↓ の folder-nav reopen

実機実行はユーザーが行う。プロトタイプが載ったビルドを
`.\scripts\build-release.ps1` で用意し (CLAUDE.md「実機検証用バイナリの準備」参照)、
上記 4 状況の操作手順を完了報告に書いてユーザーへ依頼すること。

## 5. 成果物

1. **調査レポート `docs/archive/detached/detached-rework-stage-r0-report.md`**:
   - 3.1 / 3.2 / 3.3 それぞれの結論 (成立 / 不成立と根拠。ソースの該当箇所を
     crate 名 + パスで引用)
   - 推奨方式と、その方式で R1 を実装する際の注意点・リスク
   - 実機ログの要約 (並走ログで新旧の一致率、旧方式が滑って新方式が正しかった
     ケースがあれば特記)
2. **プロトタイプコード**: `detached-rework` ブランチに `(detached-rework R0)` として
   コミット。並走ログのみで挙動不変なので master へ merge してよいが、R1 開始時に
   ログごと置き換えられる前提の使い捨てとして書く (抽象化・一般化しない)。

## 6. やらないこと (禁止)

- 本体の動作変更 (HWND の利用箇所を新方式に切り替えるのは R1 の仕事)
- rect 捕捉ロジックへの変更 (憲法 1)
- egui / eframe / winit の fork・vendoring・パッチ
- App への新規状態フィールド追加 (並走ログに必要な一時変数は関数ローカル /
  既存構造体内に収め、やむを得ない場合は 1 個まで + レポートに明記)

## 7. 完了条件

- [ ] `docs/archive/detached/detached-rework-stage-r0-report.md` が §5-1 の内容を満たす
- [ ] 並走ログのプロトタイプがコミットされ、`cargo test --bin mimageviewer-core` が緑
- [ ] ユーザー実機で §4-3 の 4 状況のログが採取され、レポートに反映済み
- [ ] 推奨方式が「R1 で rect 捕捉を全廃できる」と言える根拠を持つ
      (並走ログで新方式が全ケース安定)

完了したら、完了報告 (レポートへのリンク + 機械チェック結果) を書いて
Fable の検収に回す。

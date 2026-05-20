# ウィンドウ内動画再生 — 実装方針案 (Claude Code 案)

## 0. このドキュメントの位置づけ

- **提案ドキュメント (未採用)**。「動画フルスクリーンウィンドウを、メインウィンドウの
  中で再生できるようにする」機能の実装方針を Claude Code がまとめたもの。
- 同じ機能について **Codex でも別途検討中**であり、後で両案を比較するための叩き台。
- 採用が決まるまで `docs/README.md` の索引には**登録しない**(競合提案のため)。
  採用後に索引へ移し、必要なら `video-architecture.md` に統合する。
- 調査時点のコード参照は関数名を主アンカーにしている (行番号は変動しうる)。
- **2026-05-20 改訂**: Codex 案 (`docs/codex-main-window-native-video-plan.md`) と
  突き合わせ、初版 §2 の「WS_CHILD は DComp 制約で不可」という記述が**誤り**だったため
  訂正。§9 に両案の比較と収束した推奨、§10 に実装・レビュー分担の見解を追加した。

## 1. 目的とスコープ

### やりたいこと

現状、動画のインライン再生は **モニタ全面のボーダレス native presenter ウィンドウ**
でしか行えない (= 実質フルスクリーン専用)。これに加えて、

> 例えば `Shift+Enter` で再生したとき、**メインウィンドウのクライアント領域に
> ちょうど重ねる形**で動画を再生する「ウィンドウ内再生モード」を追加する。

ユーザーの言葉では「native ウィンドウを同じ位置にちょうど重ねて再生するイメージ」。
操作は**事実上モーダル**(動画がグリッドを覆うので、その間グリッドは触れない)になる。

### スコープに含めるもの

- ウィンドウ内再生モード (以下「in-window モード」) の追加。
- 既存のフルスクリーンモードはそのまま残し、in-window はモード分岐として並置する。
- 再生系の機能 (シーク / ループ / 速度 / フレームステップ / ブックマーク / ピン /
  タイルモード / メタデータパネル / perf overlay 等) は in-window でもそのまま使う。

### スコープから外すもの (意図的)

- **VST3 プラグイン GUI**。in-window モードでは VST GUI を出さない。
  - 設定済み VST チェーンの**音声処理は有効のまま** (DSP チェーンは presenter
    ウィンドウと独立に動くため)。
  - HUD の VST ボタンを in-window モードでは非表示にする。
  - 理由: VST GUI は別プロセスの絶対座標トップレベル HWND で、移動・リサイズ可能な
    ウィンドウへの相対追従を作り込むコストが高く、リスクも大きい。ここを外すことで
    HUD の z-order 機構の大半 (後述) を不要化できる。

## 2. 採用方式: メインウィンドウ重ね合わせ (overlap popup)

「メインの中で再生」を実現する方法は3つあり、本案は **(C) を採用**する。

| 方式 | 概要 | 判定 |
| --- | --- | --- |
| (A) 独立ウィンドウ | 動画を `WS_OVERLAPPEDWINDOW` の別ウィンドウで再生 | 却下 |
| (B) 真の子ウィンドウ | presenter HWND を `WS_CHILD` でメインに埋め込む | **有力候補 (要スパイク)** |
| (C) 重ね合わせ popup | presenter HWND をメインのクライアント矩形に重ねる | **採用 (低リスク本命)** |

(B) と (C) は最終的にどちらも成立しうる。両者の優劣はスパイクで決める (§9)。
本書 §4 以降は (C) の詳細設計を記す。(B) の評価は次節と §9 を参照。

### (B) WS_CHILD の評価 — 可能だが要スパイク (初版の判断を訂正)

**初版は「DComp の `CreateTargetForHwnd` はトップレベル限定なので WS_CHILD は不可」と
書いたが、これは誤り。** Chromium 等のブラウザは GPU 合成出力を `WS_CHILD` の子
ウィンドウに対する DirectComposition で実運用しており (`gl::ChildWindowWin` が
`WS_CHILD` を作り、`DCLayerTree` が `CreateTargetForHwnd(child_hwnd, ...)` を呼ぶ)、
**子 HWND を DComp target にする方式は実証されている**。よって WS_CHILD は技術的に可能。

ただし本コードベース固有の検証ポイントがあり、Phase 0 の実機スパイクで潰すべき:

- 現 presenter の `Borderless` 経路は `WS_EX_NOREDIRECTIONBITMAP` を使う
  (`native_window.rs`)。子ウィンドウ向けの ex-style 調整が要る。
- **`WM_DPICHANGED` はトップレベル HWND にしか来ない**。子 presenter は親
  (メイン HWND) の DPI 変化を受けて自前で pixels-per-point / 矩形を伝播する必要が
  ある。現 DPI 処理は HUD HWND の `WM_DPICHANGED` 起点 (`src/video/mod.rs` の
  `DpiChanged` アーム) なので、ここは作り直しになる。
- `WM_WINDOWPOSCHANGED` の座標が親相対になる等、既存ジオメトリ前提が変わる。
- 入力・フォーカスモデルの差 (子 HWND は親と z-order を共有、focus も子で取れる)。

WS_CHILD の利点 (popup 案にない):

- 子は親相対座標なので **親の移動に自動追従する**。→ §4.3 で popup 案の弱点として
  挙げる「モーダルムーブループ中の追従ラグ / スナップ」が **WS_CHILD では構造的に
  発生しない**。
- 親クライアント領域に**自動クリップ**される。
- タスクバー / Alt+Tab エントリが完全にメイン 1 つにまとまる。

Codex 案はこの WS_CHILD を第一候補に据え、child+DComp の実機スパイクを Phase 0 に
置く。この判断は妥当 (§9)。

### (A) より (C) が優れる理由

(A) は別途 [video-windowed-mode の事前調査] で見積もったが、

- `WS_OVERLAPPEDWINDOW` モードの追加が必要。
- 動画再生を「モーダルなフルスクリーン状態」として扱う **App 側の状態モデル**
  (`fullscreen_idx` 前提) を崩す懸念がある。
- フォアグラウンド奪還・cloak まわりの分岐が増える。

これに対し (C) は:

- presenter は今のまま **ボーダレス `WS_POPUP` (= トップレベル、DComp 動作可)** を
  維持。矩形をモニタ矩形からメインのクライアント矩形に変えるだけ。
- 動画再生は引き続き**モーダル** (`fullscreen_idx.is_some()` がモーダルを意味する
  前提を一切壊さない)。App 状態モデルの改修が不要。
- presenter のレンダリングパイプラインは無改変。

つまり (C) は **「既存フルスクリーン presenter を、モニタ矩形ではなくメインの
クライアント矩形に置く」** だけが本質で、改修が presenter の外側 (ウィンドウ配置と
モード分岐) に閉じる。

## 3. 現状アーキテクチャ — 再利用できる土台

本案が「軽い」と判断する根拠。以下はすでに実装済みで、そのまま使える。

1. **presenter は egui から独立した単独ウィンドウ**。専用スレッド
   (`run_native_video_output`, `src/video/mod.rs`)・専用 D3D11 swap chain・専用
   入力処理 (`native_window.rs` の wndproc) を持つ。「egui から動画を引き剥がす」
   作業は v0.9 で完了済み。

2. **リサイズが完全配線済み**。presenter HWND の `WM_WINDOWPOSCHANGED` →
   `NativeVideoWindowEvent::GeometryChanged` → `run_native_video_output` の
   イベントループ (`src/video/mod.rs` の `GeometryChanged` アーム) が
   `presenter.set_hud_geometry()` + `presenter.resize()` を呼ぶ。
   `NativeVideoPresenter::resize()` は swap chain 背景・動画 transform・各
   overlay を再構築する。`WM_DPICHANGED` → `DpiChanged` も処理済み。

3. **presenter HWND はメイン HWND を owner に持つ `WS_POPUP`**。
   `native_video_presenter_config` (`src/app.rs`) が `owner_hwnd = main_hwnd` を
   `NativeVideoOutputConfig` に渡し、`run_native_video_output` が
   `NativeVideoWindowMode::Borderless { rect }` でウィンドウを作る。
   owner 付き popup は **owner より常に手前** に出て、**owner の最小化/復元に
   追従**する (位置は追従しない → 後述のジオメトリ追従が必要)。

4. **HUD overlay HWND の owner は presenter HWND**
   (`hud_window.rs` の `HudOverlayWindow::create`、`owner_hwnd: config.hwnd`)。
   つまり z-order は構造上 `main < presenter < HUD` が owner 関係だけで保証される。
   現状の `WS_EX_TOPMOST` は VST GUI より前に出すためだけのもの。

5. **コマンドチャネルが存在**。`NativeVideoOutputCommand` enum (`src/video/mod.rs`)
   を UI スレッド → presenter スレッドへ送れる。`RaiseHudToTop` /
   `RaisePresenterToFront` / `SwitchSource` などが既にある。ここに 1 variant
   足せばよい。

6. **モード列挙の片割れが既にある**。`NativeVideoWindowMode` には
   `Borderless { rect }` と `Windowed { width, height }` の両方が定義済み
   (`native_window.rs`)。in-window モードは `Borderless { rect }` をそのまま使う
   (矩形がメインのクライアント矩形になるだけ) ので、`Windowed` バリアントは
   今回**使わない**。

## 4. 実装方針 — 変更箇所

### 4.1 再生モードの定義と引き回し

- presenter の配置を表す列挙を新設する。例:

  ```rust
  // src/video/mod.rs (NativeVideoOutputConfig の隣)
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum PresenterPlacement {
      /// 従来。モニタ全面ボーダレス。main HWND は cloak する。
      Fullscreen,
      /// 新規。メインのクライアント矩形に重ねる。main は cloak しない。
      InMainWindow,
  }
  ```

- `NativeVideoOutputConfig` に `placement: PresenterPlacement` を追加。
- App 側はセッション中の再生モードを保持する必要がある (cloak / フォアグラウンド /
  HUD topmost / VST ボタン表示の分岐に使う)。`App` に
  `native_video_placement: PresenterPlacement` 相当のフィールドを足す。
  値は「動画フルスクリーン open のときに決定」する。

### 4.2 presenter の初期矩形の決定

`native_video_presenter_config` (`src/app.rs`) を分岐させる:

- `Fullscreen`: 現状どおり `MonitorFromWindow` + `GetMonitorInfoW` で
  `info.rcMonitor` を `rect` にする。
- `InMainWindow`: メイン HWND の **クライアント矩形をスクリーン座標に変換**して
  `rect` にする。`GetClientRect(main_hwnd)` + `ClientToScreen` 2 点、または
  `GetWindowRect` から非クライアント分を差し引く。
- どちらも presenter は `Borderless { rect }` で生成する (変更なし)。

矩形の取り方の設計判断は §8 を参照 (クライアント全体を覆うか、グリッド部分のみか)。
本案の既定は**クライアント領域全体を覆う**。

### 4.3 ジオメトリ追従 (in-window モードの唯一の本質的新規作業)

メインウィンドウが移動・リサイズされたら、重ねている presenter popup を追従させる。

- **新コマンド追加**: `NativeVideoOutputCommand::SetPresenterRect { rect }`。
  presenter スレッドが pop して、自分の HWND に `SetWindowPos` する。
  これが presenter HWND の `WM_WINDOWPOSCHANGED` を発火させ、**既存の
  `GeometryChanged` 経路** (`set_hud_geometry` + `resize`) が HUD 追従と swap chain
  リサイズを処理する。→ 新規の描画系コードはほぼ不要。
- **追従トリガ**: `App::update` 毎フレームで `main_hwnd` のクライアント矩形
  (スクリーン座標) を取得し、前フレームと差があれば `SetPresenterRect` を送る。
  既存の cursor polling と同型のパターンで素直に書ける。
- **最小化/復元**: owner 付き popup なので **自動追従**。追加処理不要。
- **モーダルムーブループ問題 (既知の弱点)**:
  メインのタイトルバーをドラッグ移動している最中は Windows のモーダルループに入り
  `App::update` が回らない → ドラッグ中 popup が追従せず、**離した時にスナップ**する。
  - v1 はスナップ許容で十分。
  - 完全追従が要るなら、`main_hwnd` をサブクラス化 (`SetWindowSubclass`) して
    `WM_MOVE` / `WM_WINDOWPOSCHANGED` を直接拾い、その場で `SetWindowPos(presenter)`
    する。winit 所有 HWND のサブクラス化は可能だが侵襲的なので v1 では見送る。
  - リサイズドラッグは eframe が再描画するため `App::update` がおおむね回り、
    ライブ追従に近い挙動になる。
  - **この弱点は WS_CHILD 方式 (§2 (B)) なら構造的に発生しない** — 子は親相対座標で
    親の移動に自動追従するため。ジオメトリ追従が popup 案で最も手間のかかる箇所で
    あり、ここが両案の実装コスト差の主因。詳細は §9。

### 4.4 cloak / フォアグラウンド / z-order のモード分岐

`InMainWindow` のときに**無効化**する処理:

- **main HWND の cloak**: `sync_native_video_main_cloak` (`src/app/native_video.rs`)。
  in-window ではメインを見せたまま popup を重ねるので cloak しない。
  起動の 150-300ms の穴ではグリッドが見えるだけで、自然な遷移に見える (むしろ可)。
- **フォアグラウンド奪還**: `close_fullscreen` 時の
  `pending_main_foreground_reclaim*` / `process_pending_main_foreground_reclaim`
  (Alt+Tab 割り込み対策)。in-window ではメインは元々見えていて popup は main owner
  なので、popup destroy → owner (main) が自然に activate する。奪還機構は不要。
- **`RaisePresenterToFront` の自動発火**: 外部フォアグラウンド復帰エッジでの再アサート。
  in-window では通常ウィンドウとして振る舞わせたいので抑制する
  (= 通常ウィンドウがフォーカスを強奪するのは異常動作)。

`InMainWindow` でも**維持**する処理:

- **`dwm_iconic_thumbnail`** (`src/dwm_iconic_thumbnail.rs`): in-window モードでも
  動画は別 HWND なので、DWM がメインを capture するとグリッドが写る (popup は写らない)。
  タスクバーのサムネに動画を出すには iconic thumbnail の偽装を**維持**する。
  予測子を「動画フルスクリーン中」から「動画再生中 (両モード)」に広げる。

### 4.5 HUD overlay HWND の非 topmost 化

- 現状 HUD HWND は `WS_EX_TOPMOST` (`hud_window.rs`)。これは VST GUI より前に
  出すため。VST を in-window でスコープ外にするので、**HUD から `WS_EX_TOPMOST` を
  外す** (in-window モードのとき)。
- HUD の owner は presenter HWND なので、topmost を外しても **owner 関係だけで
  HUD は presenter (動画) の上に残る**。`main < presenter < HUD` は保たれる。
- topmost を外すと、ユーザーが別アプリに Alt+Tab した時に
  `main + presenter + HUD` のグループごと背面に下がる (= 通常ウィンドウとして正しい)。
- これに伴い VST z-order 機構 (`schedule_hud_raise_burst` / `RaiseHudToTop` /
  `foreground_allows_hud_raise` の editor_hwnds allowlist / cursor polling の
  activation zone 検知) は in-window モードでは**動かさない**。
  Fullscreen モードのコードパスはそのまま残す。

### 4.6 VST の扱い

- in-window モードでは `NativeVideoOutputConfig.vst3_available` 相当を実質 false
  扱いにし、HUD の VST ボタンを描画しない (`overlay_draw.rs` の該当分岐に
  placement 条件を足す)。
- VST3 パネル (`SetVst3Panel`) も in-window では送らない / 描かない。
- DSP チェーン自体 (音声処理) は presenter ウィンドウと無関係に動くので、
  設定済み VST は音には効く。コード変更不要。

### 4.7 入力とモーダル性 / フォーカス

- popup が client 矩形を覆い、自前 wndproc が mouse/keyboard/IME を受ける。
  覆われた範囲では egui メイン (グリッド) に入力が行かない → **事実上モーダル**。
- メインの**非クライアント領域 (タイトルバー・枠・最小化/最大化/閉じるボタン) は
  生きたまま**にする。`EnableWindow(main, false)` は**しない** (タイトルバーまで
  無効化されるため)。これによりユーザーは動画再生中もウィンドウを移動・リサイズ
  できる。
- **キーボードフォーカス**: open 時に popup にフォーカスを当てる。同一プロセスの
  owned ウィンドウなので `SetForegroundWindow` / `SetFocus` の cooperative 制限に
  当たらず素直に効く。Space / 矢印 / J/K/L 等の既存ショートカットはそのまま動く。
- egui メインの menu bar はクライアント矩形を全面に覆うと隠れる。これは現状の
  フルスクリーン動画と同じ (再生中はメニュー非表示) なので整合的。

### 4.8 ライフサイクル (open / close / モード選択)

- **モード選択**: `Enter` = 従来フルスクリーン、`Shift+Enter` = in-window。
  どのキーに割り当てるかは確定でよいが、既定モードを設定で選べるようにしてもよい
  (§4.9)。
- **open**: 既存の動画フルスクリーン open 経路 (`start_fs_load` / `open_fullscreen`)
  に placement を引き回す。`fullscreen_idx` は両モードで `Some` (= モーダル)。
- **close**: Escape / 動画以外へのナビゲーションで閉じる経路は両モード共通
  (`close_fullscreen`)。in-window では cloak 解除・フォアグラウンド奪還が無いぶん
  むしろ単純。popup destroy で owner (main) が自然に前面に戻る。
- **動画→動画 fast-swap**: 既存の `SwitchSource` / `take_native_output` 経路は
  presenter HWND を維持したままソースだけ差し替える。in-window でも presenter HWND
  の矩形は変わらないのでそのまま流用できる。

### 4.9 設定とドキュメント

- 設定項目 (案): 既定再生モード (Fullscreen / InMainWindow)、または
  「Shift+Enter で in-window」固定でも可。in-window の初期サイズはメインの
  現在のクライアント矩形に追従するので、専用の永続化は不要。
- ドキュメント更新 (機能採用時):
  - `docs/video-architecture.md` — presenter の配置モードの節を追加。
  - `docs/spec.md` — 設定項目・操作の追記。
  - `htdocs/mimageviewer/manual/` / `index.html` — マニュアル・製品ページ
    (内部用語を出さない方針で「ウィンドウ内で再生」と記述)。

## 5. バグリスク評価

| 領域 | リスク | 補足 |
| --- | --- | --- |
| App 状態モデルの回帰 | **極小** | `fullscreen_idx` = モーダルの前提を壊さない。in-window はモード分岐のみ |
| 描画パイプラインの回帰 | **なし** | presenter / swap chain / DComp は無改変 |
| 既存フルスクリーン再生 | **小** | placement 分岐で隔離。`Fullscreen` パスは無改変のまま残す |
| ジオメトリ追従のグリッチ | **中** | ドラッグ移動中のスナップ等。**正しさのバグではなく cosmetic** |
| リサイズドラッグの中間状態 | **中** | swap chain 差し替えは元々「ソース切替」用。連続リサイズ時の黒/ちらつきは要検証 |
| z-order / 活性化 | **中** | 別アプリへ Alt+Tab、再活性化、最小化/復元。topmost を外す変更の検証が要る |
| マルチモニタ / DPI またぎ | **中** | メインを別 DPI モニタへ移動 → presenter の `WM_DPICHANGED` 追従を要検証 |

総評: **正しさを壊す系のリスクは低い**。リスクはジオメトリ追従とウィンドウ流儀
(z-order・活性化・リサイズ中間状態) の見た目に集中する。in-window が placement
分岐で隔離されているため、既存フルスクリーン挙動への波及は構造的に小さい。

## 6. テスト・検証項目

- in-window で open → 動画がメインのクライアント矩形にちょうど重なる。
- メインをリサイズ → 動画 + HUD が追従。リサイズドラッグ中の黒/ちらつきが無い。
- メインをタイトルバーでドラッグ移動 → 離した時に追従 (スナップ許容)。
- メインを最小化 → 復元 → 動画が正しく復帰。
- 別アプリへ Alt+Tab → `main + presenter + HUD` グループごと背面に下がる
  (HUD が単独で前面に残らない)。再び mIV に戻ると正しく前面に戻る。
- メインを別 DPI のモニタへ移動 → presenter の解像度・transform が追従。
- in-window 中の Ctrl+矢印 / ホイールナビ / シーク / ループ / 速度 /
  フレームステップ / ブックマーク / タイルモード / メタデータパネル / perf overlay。
- in-window で VST ボタンが出ない。設定済み VST チェーンの音は効いている。
- in-window ↔ 動画→動画 fast-swap が正常。
- Escape / 動画以外へのナビで close → メインのグリッドに正しく戻る。
- フルスクリーンモード (Enter) が従来どおり動く (回帰なし)。
- `scripts/perf_smoke.sh` でヒッチ無し。毎フレームの矩形ポーリングが
  UI スレッドを止めていないこと。

## 7. 開発ボリューム見積もり

合計 **おおよそ 1〜1.5 週間** (集中作業):

| 作業 | 目安 |
| --- | --- |
| placement 列挙の定義と引き回し / presenter 初期矩形の分岐 | 1 日 |
| `SetPresenterRect` コマンド + App 側の矩形ポーリング (ジオメトリ追従) | 2〜3 日 |
| cloak / フォアグラウンド奪還 / `RaisePresenterToFront` のモード分岐 | 1 日 |
| HUD の非 topmost 化 + VST z-order 機構の in-window 無効化 | 1〜2 日 |
| VST ボタン非表示 + overlay レイアウト微調整 | 1 日 |
| `Shift+Enter` バインド + open/close ライフサイクル配線 + 設定 | 1 日 |
| (任意) モーダルムーブループのサブクラス化対応 | 1〜2 日 |
| 実機テスト (DPI / マルチモニタ / 最小化 / 移動・リサイズ / 回帰) | 2〜3 日 |
| ドキュメント更新 | 0.5 日 |

(A) 独立ウィンドウ案より軽く、リスクも低い。理由は §2 のとおり、改修が presenter の
外側に閉じ、App のモーダルモデルと描画パイプラインを触らないため。

## 8. 未決事項 / 設計判断ポイント (Codex 案との比較用)

両案を突き合わせる際に論点になりそうな箇所:

1. **覆う範囲**: クライアント領域**全体**を覆うか、グリッド部分の**サブ矩形のみ**か。
   - 本案の既定は全体。実装が単純 (`rect = client rect`) で、再生中メニュー非表示は
     現状フルスクリーンと整合。
   - サブ矩形案はメニュー/ツールバー/アドレスバーを見せられるが、サブ矩形の追跡が
     増え、egui レイアウト変動 (アドレスバー高の変化等) への追従が要る。
2. **モーダルムーブループ**: スナップ許容 (v1) か、`main_hwnd` サブクラス化で
   完全追従か。本案は v1 スナップ許容を推奨。
3. **モード選択 UI**: `Shift+Enter` 固定か、設定で既定モードを選べるようにするか。
4. **`dwm_iconic_thumbnail` の扱い**: in-window でタスクバーサムネに動画を出すか
   (本案は維持して出す)。出さない判断なら predicate を狭める。
5. **`WS_CHILD` か owned popup か**: 最大の分岐点。WS_CHILD は技術的に可能
   (§2 (B) で初版の誤りを訂正)。child + DComp の実機スパイクで決める。詳細は §9。
6. **フォーカスモデル**: popup にフォーカスを与える方式 (本案) か、メインに
   フォーカスを残して入力を転送する方式か。本案は popup フォーカスが既存の
   入力経路 (wndproc) をそのまま使えて単純と判断。

## 9. Codex 案との比較と収束した推奨

参照: `docs/codex-main-window-native-video-plan.md`。

### 9.1 骨子の対比

| 観点 | Claude Code 案 (本書 §4) | Codex 案 |
| --- | --- | --- |
| presenter ウィンドウ | owner 付き `WS_POPUP` をメイン client 矩形に重ねる | `WS_CHILD` 子 HWND をメインに埋め込む |
| もう一方の方式 | WS_CHILD を対等な代替候補と位置づけ | owned popup を fallback 候補 |
| メイン移動への追従 | 明示的なジオメトリ追従コマンドが必要 (ムーブループ中ラグ) | 親相対座標で**自動追従** (ラグなし) |
| クリッピング | 明示的に矩形更新 | 親 client へ**自動クリップ** |
| DComp target | 現状コードのまま (トップレベル、実証済み) | 子 HWND への `CreateTargetForHwnd` (要スパイク) |
| DPI 変化 | presenter 自身の `WM_DPICHANGED` (現状経路を流用) | 親の DPI 変化を受けて伝播 (DPI 処理を作り直し) |
| App 状態 | `fullscreen_idx` = モーダルを流用 + placement フィールド | `VideoPresentationMode` enum を新設 |
| VST3 GUI | スコープ外 (音声処理のみ継続) | スコープ外 (音声処理のみ継続) |
| レンダリング改修 | **ゼロ** (presenter 無改変) | 子 HWND 対応のため要検証 |
| 段階分け | 1 フェーズ、約 1〜1.5 週 | Phase 0 スパイク → MVP → 安定化 |

### 9.2 一致点

- 「メイン client 全面を覆い、事実上モーダル」という UX 像。
- VST3 GUI は MVP から外し、音声処理 chain のみ継続。
- fullscreen 専用の main cloak / black backdrop / topmost 回復処理は本モードで動かさない。
- 再生モードを `fullscreen_idx` 一本に詰め込まず明示的に区別する
  (Codex は enum 新設、本案も §4.1 で placement を App に持たせる — 実質同じ思想)。
- 動画→画像/ZIP/PDF への移動時の挙動を仕様として決める必要がある、という認識。

### 9.3 相違点の評価

- **最大の分岐は presenter HWND を child にするか popup にするか。** 本書初版は
  「DComp が子 HWND を target にできない」と誤って WS_CHILD を却下していたが、これは
  誤り (§2 (B) で訂正)。WS_CHILD は技術的に成立する。
- WS_CHILD の利点 (自動追従・自動クリップ・単一ウィンドウ性) は実利があり、本書 §4.3 で
  popup 案の弱点として挙げた「ムーブループ中の追従ラグ」を**構造的に解消**する。
  ジオメトリ追従は popup 案で最も手のかかる部分なので、ここが消えるのは大きい。
- 一方 popup 案は **presenter のレンダリング経路を一切変えない** (現状が既に
  トップレベル DComp)。DComp・DPI・present が現行コードで実証済みで、レンダリング側の
  リスクが構造的にゼロ。「確実に動く」点では popup が上。
- Codex が Phase 0 に child + DComp の実機スパイクを置く判断は妥当。**この 1 点
  (子 HWND で DComp presenter が綺麗に通るか) が両案の優劣を実質決める。**

### 9.4 収束した推奨

両案は「child か popup か」を除けば設計が大きく重なる。よって次の順で進めるのが最善:

1. **まず Codex 案 Phase 0 のスパイク**を実施する (child HWND への DComp presenter、
   2〜3 日)。両案唯一の本質的分岐点を直接潰す。判定基準は Codex 案 §4 Phase 0 の
   とおり (GPU/CPU 経路で非黒画面・resize 追従・親の移動/最小化/復元への追従・
   DPI 伝播が許容範囲か)。
2. スパイクが綺麗に通る → **WS_CHILD を採用** (Codex 案)。UX 最終形が良く、
   ジオメトリ追従の作り込みが要らない。
3. スパイクで問題が出る (DPI 伝播・redirection bitmap・airspace・入力/フォーカス) →
   **owned popup を採用** (本書 §4 の設計)。レンダリング無改変で確実に動く低リスク経路。
4. **どちらに転んでも、§4.4〜§4.9 (cloak/フォアグラウンド分岐・HUD 非 topmost 化・
   VST 扱い・入力・ライフサイクル・設定) と §6 のテスト項目はほぼ共通で再利用できる。**
   スパイク結果に依存するのは「ウィンドウの作り方」と「ジオメトリ追従の有無」だけ。

つまり結論として、Claude Code 案 (popup) と Codex 案 (child) は対立ではなく
**「スパイクで最終形を 1 つに決める前段が同じ」**であり、本書 §4 の設計は popup を
選んだ場合の詳細設計 + 両案共通部分の設計、として有効。

## 10. 実装・レビュー分担の提案

**実装 = Claude Code / レビュー = Codex** を推奨する。

- 本リポジトリの Codex 運用は CLAUDE.md「Codex CLI レビュー」節のとおり
  `--sandbox read-only` の**レビュー専用**として確立している。実装の反復
  (`cargo build` / `cargo fmt` / 実行・修正ループ、Rust の borrow checker 解決) は、
  ビルド・実行ツールを持つ Claude Code 側が回す方が摩擦が少ない。
- 設計はスパイク後に 1 案へ収束させる前提なので、実装者と独立した目で Codex が
  レビューする構図を保てる。CLAUDE.md の「作業の一塊が終わったら `codex exec` で
  レビュー」フローにそのまま乗る。
- 仮に最終形が WS_CHILD (Codex 案由来) になっても、設計意図は
  `codex-main-window-native-video-plan.md` に文書化済みなので、Claude Code が
  それを実装し、Codex が**自分の設計意図と突き合わせて**レビューできる
  (= 設計者がレビューに回る形になり、実装の取りこぼしを拾いやすい)。
- Phase 0 スパイクも Claude Code が dev-only 経路を実装 → 実機 GPU 確認はユーザー →
  結果を見て Codex に方式判定の第二意見を求める、という回し方が無駄がない。

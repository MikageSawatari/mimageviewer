# 別ウィンドウの配置を物理座標で 1 か所が持つ (backlog §1.384)

状態: 設計第 2 版 (2026-10-20、設計担当 ClaudeCode Opus 5.5)。第 1 版 (241dadb75) は独立設計レビュー
(GPT-6.1 Sol / xhigh) で REJECT。方針 (物理外枠矩形を正本にし、単一の Win32 適用境界へ集約する) が
症状パッチではなく構造的修正であることには同意を得た。差し戻し理由 (初期化・描画・可視化の順序、
対象モニターの選び方、DPI 境界をまたぐ適用、main 全画面の HWND) を §3.4〜§3.7 で解いた。指摘との対応は §7。
detached 経路に触れるため、[detached-rework-plan.md](detached-rework-plan.md) §2 (憲法) と
CLAUDE.md「Detached viewer リワーク中のルール」に従い、構造的修正であることを双方で合意してから実装する。
合意と触れた範囲は同書 §11 に記録する。

## 1. 観測と原因

- 観測 (利用者の実機、2026-10-15。窓情報は ClaudeCode が読み取り専用で取得): DISPLAY1/2 = 150%、DISPLAY3 = 100%。
  別ウィンドウの動画を DISPLAY3 で F11 → メインのフォルダ変更で promote → メインで別の動画を開くと、
  新しい動画の窓が物理 9000,2270〜11880,4070 (DISPLAY3 の矩形の 1.5 倍、画面外) に作られた。
  Win+Shift+← と F11 では戻せなかった。
- 原因 (コードの参照。実機での同一手順の再現は未実施):
  - mIV は detached 窓の位置・大きさを「その窓の DPI で割った論理値」(`DetachedViewerWindowPlacement`
    `{x,y,w,h,maximized}`、src/settings.rs) で持ち、どの DPI で割ったかを持たない。
  - 論理値を物理へ戻すのは egui-winit / winit で、**そのときの HWND の DPI** を掛ける。
    - builder の `with_position` / `with_inner_size` は winit の `LogicalPosition` / `LogicalSize` になり、
      winit 0.30.13 は `CW_USEDEFAULT` で窓を作った直後の HWND の DPI で物理化する
      (winit `platform_impl/windows/window.rs` の `InitData::create_window`)。作成直後の DPI は対象モニターと無関係。
    - `ViewportCommand::OuterPosition` / `InnerSize` はそのときの窓の ppp を掛ける。
  - F11 (仮想全画面) の対象矩形は `get_monitor_logical_rect_at` が **対象モニターの DPI** で論理化し
    (`rcMonitor / GetDpiForMonitor`)、それを上の経路で **窓の DPI** で物理化する。両者が違うと倍率がずれる。
    作成直後 (F12 往復・新規作成・promote 後の新しい窓) は窓の DPI が既定位置のモニター (150%) なので、
    100% の DISPLAY3 の矩形が 1.5 倍になる。観測の矩形と一致する。
  - F11 解除の restore、保存値からの新規作成、passive 窓 (always-new の静止画窓) の作成も同じ境界を通る。
  - `detached_viewer_monitor_rect_logical` の代替経路は、保存値の中心へ **メイン窓の** ppp を掛けてモニターを
    引く (別の窓の DPI を混ぜている)。
  - 同型: メインから開く全画面 viewport (`fullscreen_viewport_geometry`) も「対象モニターの DPI で論理化 →
    作成直後の HWND の DPI で物理化」。全画面の背景 (`fullscreen_backdrop_physical_rect`) は
    「対象モニターの DPI で論理化 → メイン窓の ppp で物理化」。
  - v4.6.0 から同じ経路があり、v4.7.0 の退行ではない (2026-10-15 の調査)。

BA との対応: BA-6 / R2c (placement の正本は runtime。今回はその**単位**が DPI 依存で、正本が 1 つでも
変換境界が複数ある)、BA-2 (最初の可視化の前に最終 placement を適用する)、BA-7 (状態を増やさない)。

## 2. 不変条件

1. **デスクトップ上の位置・大きさは物理ピクセル (仮想スクリーン座標) で持つ**。detached 窓の runtime placement、
   F11 の restore、保存値、全画面の対象矩形はすべて物理値。論理値 (egui points) は窓の中の描画にだけ使う。
2. **detached 窓の top-level の位置・大きさを窓へ適用するのは 1 つの境界だけ** (`detached_window_geometry`、§3.3)。
   egui の `with_position` / `with_inner_size` / `OuterPosition` / `InnerSize` では送らない
   (egui-winit が窓のそのときの DPI で物理化するため)。
3. **全画面の対象は対象モニターの物理 `rcMonitor` をそのまま使う**。DPI で割らない・掛けない。
4. **対象モニターは、配置の意図を表す物理矩形から決める**。新しく作った (まだ配置していない) HWND の位置は
   使わない (§3.5)。
5. **OS 窓は、その HWND に最終の物理配置を適用し終えるまで見せない** (BA-2 のあるべき前提を、builder ではなく
   「最初の Visible commit の前に物理配置を適用する」として満たす)。判定は「この HWND に適用した」という
   事実 (runtime が持つ HWND 値と registry の現 HWND の一致) で行い、時間で判定しない。
6. 時間窓・リトライ・画面外を後から戻す補正を主修正にしない (憲法 §2-5、backlog の指示)。
7. Win32 の `SetWindowPos` は App / eframe の borrow や egui の lock を保持したまま呼ばない
   (`WM_DPICHANGED` が同期で winit の wndproc に入るため)。

## 3. 設計

### 3.1 型

```rust
/// 物理デスクトップ座標の外枠矩形 (GetWindowRect と SetWindowPos が使う矩形)。
pub struct PhysicalRect { left: i32, top: i32, right: i32, bottom: i32 }

/// 通常表示の配置。外枠 (装飾込み) の物理矩形と最大化の有無。
pub struct PhysicalWindowPlacement { outer: PhysicalRect, maximized: bool }
```

- 外枠 (`GetWindowRect` の値、DWM の見えない枠を含む) で持つ。読む API と書く API が同じ矩形なので往復でずれない。
  winit の `set_decorations` は `SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED` で外枠を保つ (独立レビューで確認)。
- `DetachedViewerWindowPlacement` (論理) は **保存値の読み込み (移行) にだけ** 残す。runtime・restore・
  builder・保存先の書き込みは `PhysicalWindowPlacement` に置き換える。

### 3.2 所有者

- **runtime placement** (`DetachedWindowRuntime.placement`): `PhysicalWindowPlacement` にする。
  window_id ごとの正本であることは R2c のまま (新しい保存先・同期経路は作らない。憲法 §2-4)。
- **runtime の適用記録** (`DetachedWindowRuntime.placed_hwnd: Option<isize>`、新設): §3.3 の適用が成功し、
  最終矩形を確かめた HWND。registry の現 HWND と一致するときだけ「配置済み」。HWND が作り直されれば
  自動的に不一致になる (別に失効させる処理は要らない)。同じ HWND の再登録では再配置しない。
- **F11 の意図と restore** (`App.detached_viewer_borderless_fullscreen` / `detached_viewer_restore_placement`、
  唯一の書き手 `write_detached_viewer_borderless_state`): 場所はそのまま、restore の型を物理にする
  (独立レビュー同意)。runtime へ移さない理由: F12 往復では旧 runtime を終端退役させてから新しい window_id を作り、
  その間も意図と restore を引き継ぐ (既存テスト `f12_round_trip_recreates_detached_host_with_borderless_mode_and_restore_placement`)。
  runtime へ移すと退役時の受け渡し経路が新たに要る。
- **F11 遷移の対象** (`DetachedViewerBorderlessTransition`): F11 を押した時点 (状態 command を送る前) の窓から決めた
  物理の対象矩形 (入るときは `rc_monitor`、出るときは restore の外枠) を遷移の値として持つ。遷移中に読み直さない。
- **builder placement latch** (`DetachedWindowRuntime.builder_placement_latch`): 全 builder が位置・大きさを持たなく
  なるので撤去する (独立レビューで他用途なしを確認。`ViewportBuilder::patch` は新値が `None` なら位置・大きさの
  command を出さない。ただし過去の `Some` を消さないので、位置を持つ builder の入口を 1 つも残さないことが条件)。

### 3.3 変換・適用の境界 `src/detached_window_geometry.rs` (新設)

純粋関数部 (テスト可能) と Win32 部に分け、Win32 部は trait 越しに呼んで fake でも検証できるようにする。

- `monitor_for_rect(PhysicalRect) -> MonitorFacts { rc_monitor, rc_work, dpi }` (`MonitorFromRect` NEAREST)。
  Windows が窓の DPI を決める規則 (交差面積が最大のモニター) と同じ規則。
- `borderless_target_for(intent: PhysicalRect) -> PhysicalRect` = `monitor_for_rect(intent).rc_monitor`。
- `apply_outer_rect(hwnd, target: PhysicalRect) -> Result<(), PlacementMismatch>`:
  1. `m = monitor_for_rect(target)`。`GetDpiForWindow(hwnd) != m.dpi` のときだけ 2 を行う。
  2. **staging**: target を `m.rc_monitor` に収めた矩形 (target と `rc_monitor` の交差。target が対象モニターより大きい
     辺は `rc_monitor` の辺に合わせる) へ `SetWindowPos(SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER)`。
     窓全体が m の中にあるので窓は m に属し、Windows が `WM_DPICHANGED` を同期で送る。winit は論理サイズを保って
     大きさを作り直し、m からはみ出せば m へ寄せる (winit `event_loop.rs` の `WM_DPICHANGED`)。
     左上だけを動かす案 (第 1 版) は、現在の大きさによっては元のモニターに残るので採らない (レビュー P2-5)。
  3. `SetWindowPos(target)`。target は m との交差が最大なので、この段では DPI が変わらない。
  4. `GetWindowRect == target` かつ `GetDpiForWindow == m.dpi` を確かめる。満たさなければ
     `PlacementMismatch` (target・実際の矩形・両方の DPI) をエラーとしてログに出す。これは設計の前提が崩れたことを
     示す不具合の検出で、まれな API 失敗の割り切りには分類しない (テストで前提を固定する)。
     ログを出したうえで、その HWND は配置済みとして記録する: 2 の後なので窓は対象モニターに居り、
     見えない窓を作るより、位置の誤差が見える窓のほうが利用者が回収できるため。再試行はしない。
  - winit の `WM_DPICHANGED` 処理は同期で、後から届く `ScaleFactorChanged` は egui-winit が `InnerSizeWriter` を
    変えないので大きさを再変更しない (独立レビューで確認)。待機・再試行は足さない。
  - `WM_GETDPISCALEDSIZE`・対象モニター上での生成・`SetWindowPlacement` は代替にならない (レビューの検討結果、§7)。
- `read_outer_rect(hwnd)` (`GetWindowRect`)、`read_zoomed(hwnd)` (`IsZoomed`)。
- `default_placement(main_outer: PhysicalRect, ...)`、`offset_placement(base, steps)`、
  `migrate_legacy_placement(...)` (純粋、§3.6・§3.7)。

### 3.4 配置して見せる順序 (作成・作り直し・adopt)

1 つの HWND について、順序は次の 1 本に揃える。

1. **作成**: builder は位置・大きさを持たず `Hidden` (`with_visible(false)`) で作る。
2. **登録と適用**: HWND を registry に登録する所で、その HWND が未配置なら §3.5 の意図から target を決めて
   `apply_outer_rect` し、成功したら `placed_hwnd` を記録する。続けてその viewport へ
   `ViewportCommand::RequestPaintWhileHidden` を送る (eframe は非表示の viewport の UI を通常走らせないため。
   `vendor/eframe/src/native/frame_output.rs` の `PassDecision`)。
3. **非表示描画と可視化**: 次の pass で callback が走り、最終配置の大きさで描く。callback の中の既存の
   Visible commit の位置で、**その HWND が配置済みなら** commit する (最大化は従来どおり commit の中)。
4. **shown の記録**: `fs_viewport_shown` などの「初回表示済み」は、実際に Visible commit を送ったときだけ立てる。
   未配置で commit を見送った pass では立てない (レビュー P1-3)。

登録の入口 (全部 2 を通す): created (`register_detached_window_hwnd_after_show`)、unclaimed adopted、
skipped-snapshot recovered、deferred adopted、watcher repair、ParkedLive の immediate 作り直し。
入口の一覧は独立レビューの監査表 (§7) を正とし、実装担当はコードで漏れを確かめて報告する。

- **active の通常描画** (Visible commit が子 callback 内、登録が `show_viewport_immediate` から戻った後):
  作成の pass では未登録なので commit しない。戻った後に登録・適用・`RequestPaintWhileHidden`。次の pass で commit。
  初回表示が 1 pass 遅れる (約 1 frame)。
- **loading shell / holdover / keepalive backstop** (登録の後に commit): 登録で適用してから同じ順で commit する。
  非表示描画が要るなら 2 の `RequestPaintWhileHidden` で満たす。
- **passive (deferred) 窓**: 第 1 版の「Hidden で作る」だけでは、eframe が Hidden deferred の callback を走らせず、
  HWND の採用 (`push_first_callback_once` → root 側の採用) と初回描画が循環待ちになる (レビュー P1-2)。
  作成と同時に `RequestPaintWhileHidden` を送り、最初の callback を走らせて採用の入口を開く。採用で 2 を行い、
  次の callback で 3 (passive の Visible commit を新設。今は builder の `with_visible(true)` が可視化を担う)。
  トレイ常駐中に作る passive 窓 (今 `visible=false`) は 3 を行わず、既存のトレイ復帰の commit に任せる。
- **既存窓の再表示** (トレイ復帰・native 遷移・`app.rs` の再表示・`ui_fullscreen.rs` の再表示など、helper を
  通らないものを含む): 全部を 1 つの commit helper に通し、helper が「現 HWND が配置済み」を確かめる。
  生きている同じ窓 (folder-nav の再利用、park / promote、active 再入場) は再配置しない。
- **非表示の keepalive・cleanup**: 作り直した HWND には 2 を行う。可視化は要求しない。

### 3.5 target の決め方 (意図から決める)

| 場面 | target |
| --- | --- |
| 通常表示の窓を作る・作り直す | runtime placement の外枠 (seed は §3.6) |
| 仮想全画面の意図のまま作る・作り直す (F12 往復・promote 後の新しい窓・host 消失後) | `borderless_target_for(restore の外枠)`。restore が無ければ runtime placement の外枠 |
| F11 に入る (生きている窓) | 押した時点の `GetWindowRect` (最大化中もその矩形) から `borderless_target_for`。遷移が保持 |
| F11 を出る | restore の外枠 (遷移が保持)。最大化は適用後の commit で戻す |

- 新しく作った HWND の位置 (`CW_USEDEFAULT`) は、どの場面でも target の決定に使わない (レビュー P2-4)。
- 仮想全画面の窓は、通常表示時の位置 (restore) が載っていたモニターに出る。F11 に入ったときの対象モニターと
  restore のモニターは、最大化中に別モニターの通常矩形を持つ窓を除いて一致する (除外の場合は restore 側に出る。割り切り)。
- F11 の遷移は、状態 command (`Maximized(false)` / `Decorations(...)`) を送る段と、次の pass で `apply_outer_rect`
  する段に分ける (immediate viewport の command は子 callback の後、`show_viewport_immediate` から戻る前に処理される。
  独立レビューで確認)。後段の前提は `IsZoomed == false` で確かめる。**装飾の有無は Win32 style では確かめない**
  (winit は装飾の有無にかかわらず `WS_CAPTION` を付け、`WM_NCCALCSIZE` で装飾を消すため。レビュー P1-1)。
  winit の装飾状態を読む境界 (例: `Window::is_decorated()` を eframe 経由で読む、または egui の `ViewportInfo` に
  反映された値) を実装担当がコードで選び、報告する。読めない場合は「状態 command を送った pass の次の pass」という
  command 処理順の事実で後段へ進む (時間待ちはしない)。
- 既存の `Settle(96ms)` (遷移の最後の時間待ち) は今回の範囲では触らない (既存の仕組み。撤去は R4 の範囲)。

### 3.6 seed・既定配置・複数窓のずらし

- 既定配置 (`default_detached_viewer_window_placement`): メイン HWND の外枠 (物理) を基準に、
  左上 + 48 × メインのモニターの倍率、大きさ (960x720 × 同倍率) + そのモニターの DPI の装飾
  (`AdjustWindowRectExForDpi`)。
- always-new の +40 (`offset_detached_image_window_placement`): 基準 placement のモニターの倍率を掛ける
  (100% で 40px、150% で 60px。従来の見た目を保つ。レビュー P3-7)。
- 保存値の妥当性: 物理外枠の上端帯 (タイトルバー相当) が `MonitorFromRect(DEFAULTTONULL)` で
  どこかのモニターに載るか。載らなければ既定配置 (モニターを外した場合の seed の選び直し)。
- 既定サイズ拒否の防波堤 (`detached_passive_placement_update_looks_like_default_viewport`) は撤去しない
  (rework-plan R2c の決定)。物理値で判定するよう単位だけ合わせる。

### 3.7 保存値と移行 (リリース済み: v1.5.0 から)

- `Settings.detached_viewer_window_placement` (論理) は v1.5.0 からリリース済み。移行と後方互換テストを用意する。
- 新しい field `detached_viewer_window_physical_placement: Option<PhysicalWindowPlacement>` (serde default `None`) を足し、
  以後の書き込みはこちらだけ。旧 field は読むだけで書き換えない (古い版へ戻した場合は古い値が使われる。割り切り)。
- `settings_transfer.rs`: 旧 field と同じく **PC 固有値として書き出し・取り込みの対象から除外** する
  (レビュー指摘)。settings.rs の live 値の copy (設定画面の draft で古い値に巻き戻さない仕組み) にも新 field を足す。
- 読み込み順: 新 field があればそれ。無く旧 field があれば、seed を作る所 (App、モニター情報が取れる所) で 1 回だけ移行:
  - 旧値は「外枠の左上 = 物理 ÷ s」「client の大きさ = 物理 ÷ s」(s はその窓が居たモニターの倍率)。
  - 各モニター M (倍率 s_M) について、旧値の中心 × s_M が M の `rcMonitor` に入るものを候補にする。
  - 候補が複数なら、旧矩形 × s_M と `rcMonitor` の重なりが最大のもの。候補が無ければ既定配置。
  - 外枠 = 左上 × s_M、大きさ = client × s_M + M の DPI の装飾 (`AdjustWindowRectExForDpi`)。
  - 結果は runtime placement になり、窓を閉じたときに新 field へ保存される。
  - 推定が外れる配置 (モニター配置の変更後など) は、利用者が窓を動かせば次から正しい値が保存される (割り切り)。
- 通常表示中の位置の記録: HWND の `GetWindowRect` (物理) を記録する。最大化中は外枠を更新せず `maximized=true` だけ
  記録する (active の現行規則に passive も揃える)。borderless 中・遷移中は記録しない (現行どおり)。

### 3.8 main 側の全画面

- 全画面の背景 (`fullscreen_backdrop_physical_rect`) は `monitor_for_rect(メイン HWND の外枠).rc_monitor` にする
  (メイン HWND は既知。HWND の取得の設計は要らない)。
- メインから開く全画面 viewport (`fullscreen_viewport_geometry`) は同型だが、**今回は直さない**。
  その HWND は detached の registry ではなく command binding で扱っており (`app.rs` の capture)、Hidden で作ってから
  配置するには、viewport の生存世代と対応付けて HWND を得る境界の設計が別に要る (レビュー P2-6。rect 一致探索の
  拡張は不可)。backlog に別項目として起票し、利用者の報告が無いこと (実機での発生は未確認) も書く。

## 4. 状態の組み合わせを減らす検討 (CLAUDE.md の設計方針)

- 採用: builder が位置を持たないことで、builder latch と「egui の patch が送る位置 command」と
  「mIV が送る位置 command」の競合が無くなる。
- 採用: 位置の単位を 1 つ (物理) にし、変換・適用を 1 か所に集める。DPI の組み合わせ (窓の DPI × 対象の DPI ×
  UI 倍率) を各入口で扱わない。
- 採用: 「配置済み」は `placed_hwnd` と registry の現 HWND の一致という 1 つの事実で表す。HWND の作り直し・
  終了で別に失効させる処理を持たない。
- 不採用: F11 の意図・restore を runtime へ移す案 (§3.2 の理由)。
- 不採用: 画面外の窓を見つけて戻す補正 (backlog の指示)。なお F11 の対象は窓の外枠から NEAREST でモニターを
  引くので、画面外に出てしまった既存の窓も F11 で最寄りのモニターに出る (副次的な効果)。
- まれな失敗 (`SetWindowPos` 自体の失敗) は既定の割り切り: ログを出し、再試行しない。

## 5. テスト

- 純粋関数: 移行 (150%/100% の 2 モニターを右に離して並べた fake、旧値が各モニターにある場合・どこにも無い場合・
  境界をまたぐ場合)、target の決定 (§3.5 の表の各行。新 HWND の位置を使わないこと)、既定配置・+40 の倍率、
  物理矩形の保存往復 (serde、旧 field だけの JSON・新旧両方の JSON)、settings_transfer の除外、live copy。
- 適用: `apply_outer_rect` を Win32 trait の fake で検証する。fake は「窓の DPI = 交差最大のモニターの DPI」
  「DPI が変わると winit と同じく論理サイズを保って大きさを作り直し、はみ出せば寄せる」を模す。
  境界をまたぐ target、現在の大きさと target の大きさが違う場合、モニターより大きい target、上下に並んだモニター、
  同じ DPI の場合 (staging しない) を含め、適用後の矩形 = target を確かめる。
- 実 HWND: winit (eframe と同じ backend) で非表示の窓を作り `apply_outer_rect` → `GetWindowRect` が target と一致すること
  (テスト機の DPI の範囲。テスト機が単一 DPI なら DPI 跨ぎは fake 側の検証になることを報告に明記)。
- App: 初回表示の順序 (作成 pass で commit しない・登録で適用と `RequestPaintWhileHidden`・次 pass で commit・
  shown は commit 時だけ)、Hidden deferred の進行 (passive が採用まで進み可視化される)、
  新 HWND の位置と seed のモニターが違う場合、同じ HWND の再登録で再配置しないこと、HWND の作り直しで配置済みが
  外れること、F12 往復・promote 後の新しい窓・always-new・複数 passive 窓・F11 入る/解除 (最大化 → F11 → 解除を含む)・
  トレイ中の作り直し → 復帰・always-on-top の保持。
- 既存テストのうち、builder に位置があることを確かめるもの (`f12_round_trip_recreates_detached_host_with_borderless_mode_and_restore_placement`
  ほか) や論理値を前提にした placement テストは、意図 (作り直した窓が対象モニターの配置を受け取る) を保ったまま
  物理値・適用記録の検証へ書き換える。書き換えた一覧と理由を実装報告に書く (憲法 §2-8)。

## 6. 範囲外

- メイン窓 (ROOT) の起動時配置・トレイ復帰 (既に物理の `SetWindowPos` / `WINDOWPLACEMENT`)。
- メインから開く全画面 viewport (§3.8、別項目)。
- `Win+Shift+矢印` の位置ずれ (docs/dpi-multimonitor-issue.md)。
- キー入力 subclass の rect 探索 (R1 の対象外のまま)。
- detached の状態遷移・HWND registry・park / promote の所有境界そのもの、F11 遷移の `Settle` 時間待ち。

## 7. 第 1 版のレビュー指摘との対応

| 指摘 | 対応 |
| --- | --- |
| P1-1 装飾を Win32 style で判定できない | §3.5: style では判定しない。winit の装飾状態を読む境界を選ぶか、command 処理順の事実で進む |
| P1-2 passive の Hidden 作成で採用と描画が循環待ち | §3.4: 作成時に `RequestPaintWhileHidden`、採用で適用、次の callback で新設の commit |
| P1-3 active の Visible が適用より先、shown が消費される | §3.4: 作成 pass では commit しない、登録で適用と描画依頼、次 pass で commit、shown は commit 時だけ |
| P2-4 新規 borderless の対象を未配置 HWND から決める | §3.5: 意図 (restore / runtime placement) から決める。新 HWND の位置は使わない |
| P2-5 左上だけの移動では対象 DPI に入らない | §3.3: 対象モニターに収めた staging 矩形へ移してから正確な矩形、最終矩形と DPI を確かめる |
| P2-6 main 全画面の HWND 取得が未設計 | §3.8: 背景だけ直し、viewport は別項目に起票 |
| P3-7 +40 の単位 | §3.6: 基準のモニター倍率を掛ける |
| settings_transfer / live copy | §3.7 |
| 入口監査表 (deferred adopted・watcher repair・ParkedLive・helper を通らない再表示) | §3.4 |
| 代替手段 (`WM_GETDPISCALEDSIZE` / 対象モニター上での生成 / `SetWindowPlacement`) | 不採用 (レビューの検討どおり) |

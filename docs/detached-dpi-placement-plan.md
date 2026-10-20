# 別ウィンドウの配置を物理座標で 1 か所が持つ (backlog §1.384)

状態: 設計第 3 版 (2026-10-20、設計担当 ClaudeCode Opus 5.5)。第 1 版 (241dadb75)・第 2 版 (331f717f7) は
独立設計レビュー (GPT-6.1 Sol / xhigh) で REJECT。方針 (物理外枠矩形を正本にし、単一の Win32 適用境界へ
集約する) が症状パッチではなく構造的修正であることには、両版とも同意を得ている。差し戻し理由
(初期化・描画・可視化の順序、対象モニターの選び方、DPI 境界をまたぐ適用、host の同一性、main 全画面の HWND)
を §3 で解いた。指摘との対応は §7。
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
7. Win32 の `SetWindowPos` は、同期の wndproc (winit・egui-winit・vendor の Painter subclass) が取り直す
   lock / `RefCell` borrow / egui の lock を保持したまま呼ばない (`WM_DPICHANGED` などが同期で wndproc に入るため)。
   通常の `&mut App` は対象外 (winit は App のハンドラ実行中のイベントを buffer する)。

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
- **runtime の適用記録** (`DetachedWindowRuntime.host_placement`、新設の型付き状態):
  `Unplaced` / `Placed { incarnation, pass }` / `Mismatch { incarnation, pass }` (§3.3 の 4)。`incarnation` は既存の
  `host_incarnation` (`DetachedHostClaim` と同じ同一性。数値の同じ HWND を clear → 再登録しても新しい値になる)、
  `pass` は適用した時点のその viewport の pass 番号 (`cumulative_pass_nr_for(viewport_id)` など、実装担当が選ぶ)。
  現 host の incarnation と一致するときだけ配置済み。host が作り直されれば一致しなくなる (別に失効させる処理は
  要らない)。同じ incarnation の再登録では再配置しない。生の HWND 値では比べない (レビュー第 2 版 P2-4)。
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
  1. `m = monitor_for_rect(target)`、`r = m.dpi / GetDpiForWindow(hwnd)`。`r == 1` なら 3 へ。
  2. **staging**: 窓を m の DPI に入れるための仮の矩形へ `SetWindowPos(SWP_NOZORDER | SWP_NOACTIVATE |
     SWP_NOOWNERZORDER)`。Windows は窓の DPI を交差最大のモニターで決め、`WM_DPICHANGED` を同期で送る。
     winit はそこで論理サイズを保って大きさを `× r` で作り直し、Windows の推奨矩形の左上に置き、
     はみ出した所属を回数制限付きの nudge で寄せる (winit `event_loop.rs` の `WM_DPICHANGED`。nudge は clamp
     ではなく、離れたモニターでは動かないこともある。レビュー第 2 版 P2-1)。
     そのため staging は **winit の作り直しの後も m の中に収まる大きさ** にする:
     m の `rc_work` の中心に、大きさ `rc_work の幅・高さ / (2 × max(r, 1))` の矩形。`× r` 後の大きさは
     `rc_work` の半分以下で、左上が staging の左上の近くにある限り m の中に収まり、nudge が要らない。
     左上だけを動かす案 (第 1 版) と target を m に収めた矩形 (第 2 版) は、作り直し後に m を外れうるので採らない。
  2b. staging の後、`GetDpiForWindow(hwnd) == m.dpi` かつ `MonitorFromWindow(hwnd) == m` を確かめる。
     満たさなければ 4 の失敗として扱う (3 へ進まない)。
  3. `SetWindowPos(target)`。target は m との交差が最大なので、この段では DPI が変わらない。
  4. `GetWindowRect == target` かつ `GetDpiForWindow == m.dpi` を確かめる。満たせば `Placed`。
     満たさなければ `PlacementMismatch` (段・target・実際の矩形・両方の DPI) をエラーとしてログに出し、
     **成功として扱わない** (`Mismatch` として記録し、テストの成功条件にも数えない。レビュー第 2 版 P2-1)。
     ただし窓は見せる: §3.4 の可視化は `Placed` と `Mismatch` の両方で進める。理由: この不具合は
     「見えない窓から製品内で回収できない」ことが P2 の根拠であり、適用に失敗した窓を見せないと同じ状態を作る。
     見えていれば利用者は動かす・F11 で回収できる。利用者には「別ウィンドウを指定の位置に置けませんでした」と
     通知する (まれな失敗の割り切り: ログ + 通知)。
  - winit の `WM_DPICHANGED` 処理は同期で、後から届く `ScaleFactorChanged` は egui-winit が `InnerSizeWriter` を
    変えないので大きさを再変更しない (独立レビューで確認)。待機・再試行は足さない。
  - `WM_GETDPISCALEDSIZE`・対象モニター上での生成・`SetWindowPlacement` は代替にならない (レビューの検討結果、§7)。
- `read_outer_rect(hwnd)` (`GetWindowRect`)、`read_zoomed(hwnd)` (`IsZoomed`)。
- `default_placement(main_outer: PhysicalRect, ...)`、`offset_placement(base, steps)`、
  `migrate_legacy_placement(...)` (純粋、§3.6・§3.7)。

### 3.4 配置して見せる順序 (作成・作り直し・adopt)

1 つの host incarnation について、順序は次の 1 本に揃える。**Visible commit は、配置を適用した pass より後の
pass の callback の中でだけ送る**。immediate viewport は callback の後に描画し、その後に native command を処理する
(`vendor/eframe/src/native/frame_output.rs`。独立レビューで確認)。したがって後の pass の callback で送った Visible は、
同じ pass の最終配置での描画の後に処理される。deferred viewport も callback → 描画 → command の順であることを
実装担当がコードで確かめて報告する。

1. **作成**: builder は位置・大きさを持たず `Hidden` (`with_visible(false)`) で作る。Windows では Hidden の窓でも
   callback と描画は走る (egui-winit は OS の可視状態を `ViewportInfo` に反映せず、`visible()` は minimized /
   occluded から求める。レビュー第 2 版で第 1 版 P1-2 の前提を訂正)。
2. **登録と適用**: HWND を registry に登録する所で、その incarnation が未配置なら §3.5 の意図から target を決めて
   `apply_outer_rect` し、`host_placement` を記録する。続けて、HWND が既にある viewport へ
   `ViewportCommand::RequestPaintWhileHidden` を送り、次の pass を確実に起こす (HWND 生成前の command は eframe が
   保留し、生成処理だけでは flush しない。`vendor/eframe/src/native/wgpu_integration.rs`。要求は描画完了の確認では
   ないので、順序の判定には使わない)。
3. **可視化**: `host_placement.pass` より後の pass の callback で、既存の Visible commit の位置から commit する
   (最大化は従来どおり commit の中)。
4. **一回限りの表示指定の消費**: 前面化しない指定 (`take_detached_viewer_activate_on_show` の `no_activate_once`) や
   `fs_viewport_shown` などの「初回表示済み」は、実際に Visible commit を送ったときだけ消費・記録する。
   commit を見送った pass では消費しない (レビュー第 2 版 P2-3、第 1 版 P1-3)。新しい bool は足さない
   (既存の値の消費位置を commit へ移す)。
5. **位置の記録**: 配置済みになるまで、その host の位置の観測を runtime placement へ書かない。配置済みの host の
   位置の記録は、egui の観測 (callback の `outer_rect` / 送られてくる Frame) を「変わったかもしれない」という通知として
   扱い、root が現 incarnation の HWND から `GetWindowRect` で読む (レビュー第 2 版 P2-5。初回 callback の観測は
   配置前の窓の値なので使わない)。

経路ごとの当てはめ (入口の一覧は独立レビューの監査表 §7 を正とし、実装担当はコードで漏れを確かめて報告する):

- **active の通常描画** (Visible commit が子 callback 内、登録が `show_viewport_immediate` から戻った後):
  作成の pass では未登録なので commit しない。戻った後に登録・適用・描画要求。後の pass の callback で commit。
  初回表示が 1 pass 遅れる。
- **loading shell / holdover / keepalive backstop** (callback で描画 → 戻って登録 → 同じ関数で commit): 適用した pass では
  commit しない。commit を後の pass の callback へ移す (レビュー第 2 版 P2-2)。
- **ParkedLive の immediate 作り直し** (今は commit が無く builder の可視化に頼る): 登録 (`ui_fullscreen.rs` の
  ParkedLive 登録) で適用し、後の pass の callback に commit を新設する。
- **passive (deferred) 窓**: Hidden で作る。最初の callback (`push_first_callback_once`) で root が採用し、採用で 2 を行う。
  後の pass の callback に passive の Visible commit を新設する (今は builder の `with_visible(true)` が可視化を担う)。
  トレイ常駐中に作る passive 窓 (今 `visible=false`) は 3 を行わず、既存のトレイ復帰の commit に任せる。
- **created / unclaimed adopted / skipped-snapshot recovered / deferred adopted / watcher repair**: すべて 2 を通す。
- **既存窓の再表示** (トレイ復帰・native 遷移・`app.rs` の再表示・`ui_fullscreen.rs` の再表示など、helper を
  通らないものを含む): 全部を 1 つの commit helper に通し、helper が「現 incarnation が配置済み (`Placed` または
  `Mismatch`)」を確かめる。生きている同じ窓 (folder-nav の再利用、park / promote、active 再入場) は再配置しない。
- **非表示の keepalive・cleanup**: 作り直した host には 2 を行う。可視化は要求しない。

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
- F11 の遷移で順序が要るのは **最大化の解除だけ**:
  - 装飾の切り替え (`Decorations`) は winit が `SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED` で外枠を保つので、
    幾何の適用と前後どちらでもよい。装飾の状態を確かめる必要はない (Win32 style では確かめられない。
    winit は装飾の有無にかかわらず `WS_CAPTION` を付け、`WM_NCCALCSIZE` で装飾を消すため。第 1 版 P1-1)。
  - 最大化の解除 (`Maximized(false)`) の後に幾何を適用しないと、後から処理された `SW_RESTORE` が通常矩形へ戻して
    幾何を上書きする。そこで幾何の適用の前提を **`IsZoomed(hwnd) == false` という OS の事実** にする。
    最大化していない窓は状態 command と同じ pass で適用してよい。最大化中の窓は、状態 command を送った後、
    `IsZoomed == false` を確かめられた所で適用する (確かめるのは F11 遷移を進める既存の root 側の処理。
    pass の数や時間では進めない。layout の追加 pass (`request_discard`) でも判定は同じ事実なので影響しない。
    レビュー第 2 版 P2-6)。
  - F11 を出る: `Decorations(true)` を送り、幾何 (restore の外枠) を適用し、restore が最大化なら `Maximized(true)`。
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
- 通常表示中の位置の記録: §3.4 の 5 のとおり、配置済みの host だけ、HWND の `GetWindowRect` (物理) を記録する。
  最大化中は外枠を更新せず `maximized=true` だけ記録する (active の現行規則に passive も揃える)。borderless 中・
  遷移中は記録しない (現行どおり)。

### 3.8 main 側の全画面

- 全画面の背景 (`fullscreen_backdrop_physical_rect`) は `monitor_for_rect(メイン HWND の外枠).rc_monitor` にする
  (メイン HWND は既知。HWND の取得の設計は要らない)。
- メインから開く全画面 viewport (`fullscreen_viewport_geometry`) は同型だが、**今回は直さない**。
  その HWND は detached の registry ではなく command binding で扱っており (`app.rs` の capture)、Hidden で作ってから
  配置するには、viewport の生存世代と対応付けて HWND を得る境界の設計が別に要る (レビュー P2-6。rect 一致探索の
  拡張は不可)。[backlog §1.407](next-release-backlog.md) に別項目として起票した (利用者の報告は無く、実機での発生は未確認)。
  背景の修正だけで main 全画面 viewport の配置問題が解消したとは扱わない。

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
  「DPI が変わると winit と同じく論理サイズを保って大きさを作り直し、推奨矩形の左上に置き、所属が違えば winit と
  同じ条件・上限で nudge する (常に収める clamp にしない)」を模す。staging 後の確認 (2b) と、失敗時に `Mismatch` に
  なり成功に数えないことも確かめる。
  境界をまたぐ target、現在の大きさと target の大きさが違う場合、モニターより大きい target、上下に並んだモニター、
  同じ DPI の場合 (staging しない) を含め、適用後の矩形 = target を確かめる。
- 実 HWND: winit (eframe と同じ backend) で非表示の窓を作り `apply_outer_rect` → `GetWindowRect` が target と一致すること
  (テスト機の DPI の範囲。テスト機が単一 DPI なら DPI 跨ぎは fake 側の検証になることを報告に明記)。
- App: 初回表示の順序 (適用した pass では commit しない・後の pass の callback で commit・shown と
  `no_activate_once` は commit 時だけ消費)、loading shell / holdover / backstop / ParkedLive / passive の各経路の順序、
  数値の同じ HWND を clear → 再登録したら新しい host として再配置されること、配置前の観測が runtime placement を
  上書きしないこと、最大化中の窓の F11 が `IsZoomed == false` まで幾何を適用しないこと、Hidden deferred の進行 (passive が採用まで進み可視化される)、
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

### 第 2 版のレビュー指摘との対応

| 指摘 | 対応 |
| --- | --- |
| P2-1 staging 後の所属保証・mismatch を配置済みにしない | §3.3: winit の作り直し後も m に収まる staging (rc_work 中心、`/ (2 × max(r,1))`)、2b で所属と DPI を確認、失敗は `Mismatch` で成功に数えない。可視化だけは進め、通知する (理由は §3.3) |
| P2-2 loading shell / holdover / backstop が最終配置の描画前に Visible | §3.4: 適用した pass では commit せず、後の pass の callback で commit。ParkedLive の commit を新設 |
| P2-3 一回限りの非アクティブ表示指定の消費 | §3.4 の 4: commit 時だけ消費 |
| P2-4 生の HWND では再生成を見分けられない | §3.2: `host_incarnation` で記録 |
| P2-5 初回 callback の古い観測で seed を上書き | §3.4 の 5: 配置前の観測は書かない。配置後は通知として扱い HWND から読む |
| P2-6 F11 の「次の pass」は証拠にならない | §3.5: 順序が要るのは最大化の解除だけ。`IsZoomed == false` の事実で進む |
| P3-7 borrow の禁止が広すぎる | §2 の 7 を wndproc が取り直す lock / borrow に限定 |
| main 全画面の起票 | §3.8: backlog §1.407 |

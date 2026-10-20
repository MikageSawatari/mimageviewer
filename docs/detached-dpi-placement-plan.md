# 別ウィンドウの配置を物理座標で 1 か所が持つ (backlog §1.384)

状態: 設計第 6 版 (2026-10-20、設計担当 ClaudeCode Opus 5.5)。第 5 版 (3f2180bfc) は REJECT (passive の Hidden 採用と ParkedLive の表示経路)。第 6 版は passive の可視化を今のまま残し、最大化の扱いを 1 つの規則にまとめた (§3.3a・§3.4)。第 4 版 (72eed920c) は ACCEPT WITH CHANGES。第 5 版は、第 4 版で足した「初回表示を後の pass へ移す」仕組みを、扱う組み合わせを減らす方向で置き換えた (§4)。第 1 版 (241dadb75)・第 2 版 (331f717f7)・第 3 版 (733624656) は
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
2. **detached 窓の top-level の位置・大きさを確定させるのは 1 つの境界だけ** (`detached_window_geometry`、§3.3)。
   egui の `OuterPosition` / `InnerSize` は送らない (egui-winit が窓のそのときの DPI で物理化するため)。
   builder の `with_position` / `with_inner_size` は、OS 窓を作る前の **仮の配置** (§3.4) としてだけ渡し、
   正しさは境界の適用で決まる。
3. **全画面の対象は対象モニターの物理 `rcMonitor` をそのまま使う**。DPI で割らない・掛けない。
4. **対象モニターは、配置の意図を表す物理矩形から決める**。新しく作った (まだ配置していない) HWND の位置は
   使わない (§3.5)。
5. **active の窓 (immediate) は、その host に最終の物理配置を適用した後で見せる**。作成と HWND の登録が同じ root frame で
   済み、明示の Visible command は root frame の最後に処理される (§3.4) ので、登録時に適用すれば順序が成り立つ。
   root 側からの表示要求は runtime の「配置済み」の事実で確かめる。時間では判定しない。
   passive (deferred) 窓は可視化の仕方を変えず、採用時に適用する (§3.4 の 5。倍率をまたぐ作成でだけ、仮の配置で
   見えてから正しい位置へ動く)。
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
  - `Unplaced { show_request: Option<ShowRequest>, awaiting_unzoom: bool }` — 現 host にまだ配置を適用していない。
    `show_request` は root 側から来た表示要求 (§3.4 の 4) を捨てずに保持する。`awaiting_unzoom` は §3.3 の
    最大化の規則で最大化の解除を待っていること。
  - `Placed { incarnation, outcome: Exact | Mismatch }` — 現 host に配置を適用した。
  `incarnation` は既存の `host_incarnation` (`DetachedHostClaim` と同じ同一性。数値の同じ HWND を clear → 再登録しても
  新しい値になる)。`Placed` の incarnation が現 host と違えば `Unplaced` と同じに扱う (host が作り直されれば一致しなく
  なり、別に失効させる処理は要らない。ただし保持中の `show_request` は host の交代で捨てる。§3.4 の 4)。
  同じ incarnation の再登録では再配置しない。生の HWND 値では比べない (レビュー第 2 版 P2-4)。
- **F11 の意図と restore** (`App.detached_viewer_borderless_fullscreen` / `detached_viewer_restore_placement`、
  唯一の書き手 `write_detached_viewer_borderless_state`): 場所はそのまま、restore の型を物理にする
  (独立レビュー同意)。runtime へ移さない理由: F12 往復では旧 runtime を終端退役させてから新しい window_id を作り、
  その間も意図と restore を引き継ぐ (既存テスト `f12_round_trip_recreates_detached_host_with_borderless_mode_and_restore_placement`)。
  runtime へ移すと退役時の受け渡し経路が新たに要る。
- **F11 遷移の対象** (`DetachedViewerBorderlessTransition`): F11 を押した時点 (状態 command を送る前) の窓から決めた
  物理の対象矩形 (入るときは `rc_monitor`、出るときは restore の外枠) を遷移の値として持つ。遷移中に読み直さない。
- **builder placement latch** (`DetachedWindowRuntime.builder_placement_latch`): 残す。builder は今までどおり、
  host が生きていない間 (`detached_viewer_should_seed_placement`) だけ仮の配置を持ち、latch が frame 間で値を固定する。
  latch に入れる値は物理 placement で、builder を作る所で §3.4 の 1 の換算をする。

### 3.3 変換・適用の境界 `src/detached_window_geometry.rs` (新設)

純粋関数部 (テスト可能) と Win32 部に分け、Win32 部は trait 越しに呼んで fake でも検証できるようにする。

- `monitor_for_rect(PhysicalRect) -> MonitorFacts { rc_monitor, rc_work, dpi }` (`MonitorFromRect` NEAREST)。
  Windows が窓の DPI を決める規則 (交差面積が最大のモニター) と同じ規則。
- `borderless_target_for(intent: PhysicalRect) -> PhysicalRect` = `monitor_for_rect(intent).rc_monitor`。
- `apply_outer_rect(hwnd, target: PhysicalRect) -> Result<(), PlacementMismatch>`:
  1. `m = monitor_for_rect(target)`、`k = monitor_for_rect(GetWindowRect(hwnd))` (窓の今のモニター)、
     `r = m.dpi / GetDpiForWindow(hwnd)`。`r == 1` なら 3 へ。
  2. **staging** (DPI を m に移す。winit の `WM_DPICHANGED` 処理が読むのは要求した矩形ではなく **その時点の client 寸法**
     であり (winit `event_loop.rs`)、作り直し後の大きさは `client × r` になる。レビュー第 3 版 P2-1):
     2a. **今の DPI のまま小さくする**: k の `rc_work` の中心に、client が小さい既定寸法 (`S0`、例: 320×240 物理) に
        なる外枠を `SetWindowPos`。外枠と client の差は **今の窓で実測した値** (`GetWindowRect` − `GetClientRect`) を使う
        (Win32 style からは求めない。winit は装飾なしでも `WS_CAPTION` を残し `WM_NCCALCSIZE` で装飾を消すので、
        `AdjustWindowRectExForDpi(style)` は装飾なしの窓で合わない。レビュー第 4 版 P2-2)。
        窓全体が k の中なので DPI は変わらない。`GetDpiForWindow` が変わっていないこと・`GetClientRect == S0` を確かめる
        (以後 winit が読む client 寸法が確定する)。
     2b. **m へ移す**: 外枠の大きさを保ったまま (`SWP_NOSIZE`)、m の `rc_work` の中心へ移す。窓全体が m の中なので
        窓は m に属し、Windows が `WM_DPICHANGED` を同期で送る。winit は client を `S0 × r` に作り直し、推奨矩形の
        左上に新しい DPI の装飾込みで置く。`S0` は、移動前の外枠 (`S0` + 今の装飾) と作り直し後の外枠 (`S0 × r` +
        新しい装飾。新しい装飾は今の装飾 × r で見積もる) の **両方** が m の `rc_work` の半分以下になるよう選ぶ
        (r < 1 のときは移動前の外枠のほうが大きい。レビュー第 4 版 P2-2)。推奨矩形の左上は、移動先の左上
        (m の中心付近) から大きく動かない前提で、**この前提は §5 の実モニター試験が通るまで未検証として扱う**。
     2c. `GetDpiForWindow(hwnd) == m.dpi` かつ `MonitorFromWindow(hwnd) == m` を確かめる。満たさなければ 4 の失敗
        として扱う (3 へ進まない)。
  3. `SetWindowPos(target)`。target は m との交差が最大なので、この段では DPI が変わらない。
  4. `GetWindowRect == target` かつ `GetDpiForWindow == m.dpi` を確かめる。満たせば `Placed`。
     満たさなければ `PlacementMismatch` (段・target・実際の矩形・両方の DPI) をエラーとしてログに出し、
     **成功として扱わない** (`Mismatch` として記録し、テストの成功条件にも数えない。レビュー第 2 版 P2-1)。
     ただし窓は見せる: §3.4 の可視化は `Exact` と `Mismatch` の両方で進める。理由: この不具合は
     「見えない窓から製品内で回収できない」ことが P2 の根拠であり、適用に失敗した窓を見せないと見えない窓を
     自分で作る。`Visible(true)` だけで画面内に現れる保証は無いので「必ず回収できる」とは扱わない。
     通知はメイン窓のトーストで出す (「別ウィンドウを指定の位置に置けませんでした」)。
     `Mismatch` は通常の操作 (倍率の違うモニター間の配置) で起きてはならない不具合の検出であり、
     まれな失敗として受容するものではない。発生したら不具合として扱う。
  - winit の `WM_DPICHANGED` 処理は同期で、後から届く `ScaleFactorChanged` は egui-winit が `InnerSizeWriter` を
    変えないので大きさを再変更しない (独立レビューで確認)。待機・再試行は足さない。
  - `WM_GETDPISCALEDSIZE`・対象モニター上での生成・`SetWindowPlacement` は代替にならない (レビューの検討結果、§7)。
- **最大化中の窓への適用の規則 (1 つだけ)**: `apply_outer_rect` は `IsZoomed(hwnd) == false` のときだけ行う。
  最大化中の窓に target を適用する必要があるとき (F11 に入る、passive の採用で最大化した窓が target のモニターに
  居ない) は、`Maximized(false)` を送って `host_placement` を `Unplaced { awaiting_unzoom: true, .. }` にし、root 側で
  `IsZoomed == false` を確かめられた所で適用し、target が最大化なら `Maximized(true)` を送る。時間や pass の数では
  進めない。F11 (§3.5) と passive の採用 (§3.4 の 5) はこの同じ規則を使う。
  最大化中の窓が target のモニターに居るなら適用は要らない (最大化の大きさは OS がそのモニターに合わせる)。
- `read_outer_rect(hwnd)` (`GetWindowRect`)、`read_zoomed(hwnd)` (`IsZoomed`)。
- `default_placement(main_outer: PhysicalRect, ...)`、`offset_placement(base, steps)`、
  `migrate_legacy_placement(...)` (純粋、§3.6・§3.7)。

### 3.4 配置して見せる順序 (作成・作り直し・adopt)

1. **作成 (今のまま + 仮の配置)**: active の窓は今も `Hidden` で作り、その frame の描画の後に callback 内で
   Visible commit を送っている (`ui_fullscreen.rs` の初回可視化。白い client を見せないための既存の仕組み)。
   これは変えない。builder に渡す仮の配置は、target (§3.5) の物理矩形を **target のモニターの DPI** で論理化した値:
   `位置 = target 外枠の左上 / s_m`、`client の大きさ = (target 外枠 − m の DPI の装飾) / s_m` (UI 倍率の換算は従来どおり)。
   winit は作成直後の窓の DPI で物理化するので、作成直後の窓の DPI が m の DPI と同じとき (単一 DPI の環境はすべて)
   は **今と同じく最初から正しい位置・大きさで作られ、その大きさで描かれる**。違うときだけ位置・大きさがずれた
   非表示の窓になり、2 で直る。
2. **登録と適用**: HWND を registry に登録する所 (created / unclaimed adopted / skipped-snapshot recovered /
   deferred adopted / watcher repair / ParkedLive の登録。入口の一覧は独立レビューの監査表 §7 を正とし、実装担当は
   コードで漏れを確かめて報告する) で、現 host が未配置なら §3.5 の意図から target を決めて `apply_outer_rect` し、
   `Placed` を記録する。作成直後の窓の DPI と m の DPI が同じなら、1 で既に target にあるので
   `apply_outer_rect` は確認だけで終わる (`SetWindowPos` は要らない)。
3. **可視化の順序**: egui は nested immediate viewport が送った command を root (primary) の pass の出力まで
   保留し、まとめて処理する (`vendor/egui/src/context.rs` の viewport 出力。「子の command も primary が扱う」)。
   active の通常経路では、作成 → callback (描画・Visible commit の送信) → `show_viewport_immediate` から戻る →
   登録・適用 → root frame の最後に Visible を処理、の順になる。**Visible commit の位置・一回限りの表示指定の
   消費・`fs_viewport_shown` は今のまま変えない**。loading shell / holdover / keepalive backstop も、登録が commit の
   前にある (独立レビューの監査) ので同じ。
   - この順序は **明示の command** の話で、builder の差分 (`ViewportBuilder::patch` が生む位置・大きさの command) は
     nested 側の出力でも処理される。そのため builder が仮の配置を持つのは host が生きていない間 (作る前) だけにする。
     今の active 動画経路は host が生きている間も latch の値を渡しているので、実装で「作る前だけ」に整理し、
     `Placed` の後に patch が配置を上書きしないことをテストで確かめる (第 5 版レビューの確認事項)。
   - **ParkedLive の immediate 作り直し** は、今は builder の `with_visible(true)` (作成属性) で作成時に見えるので、
     この順序が成り立たない (第 5 版レビュー P2-2)。新しく作る host は `Hidden` で作り、`show_viewport_immediate` から
     戻って登録・適用した後に、4 の共通 helper で表示する (明示 command なので root frame の最後に処理される)。
     追加の pass や描画の機会の判定は使わない。
   - 倍率の違う作成 (1 で仮の配置がずれた場合) だけは、最初に見える frame の中身が適用前の大きさで描いたものになる
     (次の frame で描き直される)。画面外に出る今の不具合に比べて許容する。単一 DPI では起きない。
   - 登録が作成の frame で済まない回復経路 (HWND の差分が取れず後の frame で採用する場合) では、Visible が先に処理され、
     仮の配置で見えてから登録時に正しい位置へ動く。単一 DPI では仮の配置が正しいので動かない。まれな内部経路として
     許容し、ログで分かるようにする (`apply_outer_rect` が実際に動かしたかを記録する)。
4. **root 側からの表示要求** (トレイ復帰 `tray_integration.rs`、native 遷移の `SetHostVisible` と付随する `FocusHost`
   `native_video.rs`、既存窓の再アクティブ化 `app.rs`、その他 callback の外から Visible を送る所。レビュー第 3 版 P2-2):
   1 つの helper に通す。helper は現 host が `Placed` なら今までどおり送る。`Unplaced` なら送らず、要求 (表示・最大化・
   前面化をまとめた 1 つの `ShowRequest`) を `show_request` に保持し、2 の適用の直後に送る。要求を捨てない。
   保持中に非表示の要求が来たら `show_request` を消す (古い要求で再表示しない)。host が交代したら保持中の要求は捨てる
   (新しい host へ漏らさない)。runtime が無い窓は従来の経路のまま (新しい保留を作らない)。
5. **passive (deferred) 窓**: **可視化の仕方は今のまま変えない** (builder の `with_visible(visible)` と
   `with_maximized`。トレイ常駐中は `visible=false`)。変えるのは、builder の位置・大きさを §3.4 の 1 の仮の配置
   (target のモニターの DPI で論理化) にすることと、採用 (deferred adopted) の時に 2 の適用を行うことだけ。
   - Hidden で作ると、既存の採用 (`select_detached_unclaimed_hwnd`) が `visible=true` を条件にしている
     (非表示の残骸を誤って採用しないため。既存テストあり) ので採用できず、初回表示が循環待ちになる
     (第 5 版レビュー P1-1)。Hidden の HWND を安全に採用する仕組みは R1 の領域で今回の範囲を超えるので、作らない。
   - 単一 DPI の環境・作成直後の DPI が target と同じ場合は、仮の配置が正しいので **今とまったく同じ動き** になる
     (採用時の適用は確認だけ)。
   - 倍率をまたぐ作成では、仮の配置で見えてから (位置によっては画面外)、採用の時に正しい位置へ動く。
     画面外に作られたままになる今の不具合に比べて許容する。
   - 最大化して作った窓が target のモニターに居ないとき (倍率をまたぐ作成のみ) は、§3.3 の最大化の規則で
     解除 → 適用 → 最大化し直す。
   - passive の一回限りの表示 commit は新設しない (第 5 版レビュー P2-3 の論点自体が無くなる)。
6. **位置の記録**: `Unplaced` の間、その host の位置の観測を runtime placement へ書かない。`Placed` の host の
   位置の記録は、egui の観測 (callback の `outer_rect` / 送られてくる Frame) を「変わったかもしれない」という通知として
   扱い、root が現 host の HWND から `GetWindowRect` で読む (レビュー第 2 版 P2-5)。
7. 生きている同じ窓 (folder-nav の再利用、park / promote、active 再入場) は再配置しない。非表示の keepalive・cleanup で
   作り直した host には 2 を行い、可視化は要求しない。

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
    幾何を上書きする。そこで幾何の適用の前提を **`IsZoomed(hwnd) == false` という OS の事実** にする。 (§3.3 の最大化の規則と同じ 1 つの規則)
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

第 1〜4 版は「OS 窓を作ってから、最終配置で描いた後に見せる」を全経路で新たに保証しようとし、初回表示を後の pass へ
移す・描画の機会を判定する・一回限りの表示指定の消費位置を動かす・`Placed` と `Shown` を分ける、と経路ごとに
扱う組み合わせが増え続けた (第 4 版でも、入力は新しい寸法なのに描画面が古い寸法の瞬間が残ると指摘された)。
親セッションの提案 (2026-10-20) を受け、組み合わせそのものを減らせないかを検討した。

- **採用 (第 5〜6 版): 仮の配置 + 登録時の適用 + 既存の可視化の順序に乗る**。passive (deferred) は可視化の仕方を
  変えず採用時に適用する (Hidden の採用という新しい仕組みを作らない)。最大化中の窓への適用は F11 と passive で
  同じ 1 つの規則 (§3.3)。builder に target のモニターの DPI で
  論理化した仮の配置を渡すと、単一 DPI の環境・作成直後の DPI が同じ場合は今とまったく同じ動きになる
  (退行が無い)。違う場合だけ登録時の適用で直し、active の可視化は「子の command は root frame の最後に処理される」
  既存の順序に乗るので、可視化・表示指定・初回表示済みの扱いを変えない。新しい判定 (描画の機会) と経路ごとの
  commit の移動がすべて不要になる。代償は、倍率の違う作成で最初の 1 frame の中身が適用前の大きさになりうること
  (§3.4 の 3) と、倍率をまたぐ passive の作成で仮の配置で見えてから動くこと (§3.4 の 5)。今の不具合 (画面外で見えない)
  に比べ許容する。単一 DPI での同一性は **仮の配置の換算と passive の可視化** について言え、active の表示の順序も
  今と同じ。
- **不採用: モニターをまたぐ配置の変更では窓を閉じて作り直す**。原因は作り直しそのもの (作成直後の DPI が対象と
  無関係) にあるので、作り直しに寄せても直らない。F11 の出入りで窓を作り直すと動画の host 移動 (presenter の付け替え)
  まで巻き込み、今より重くなる。
- **不採用: 第 4 版の「初回表示を後の pass へ移す」**。上のとおり、経路ごとの順序の組み合わせが増え、描画面の寸法の
  同期という新しい前提が要る。

- 採用: mIV は生きている窓へ位置の command (`OuterPosition` / `InnerSize`) を送らず、builder の仮の配置は
  窓を作る前だけ (既存の latch のまま)。egui の patch が送る位置 command と mIV の適用が競合しない。
- 採用: 位置の単位を 1 つ (物理) にし、変換・適用を 1 か所に集める。DPI の組み合わせ (窓の DPI × 対象の DPI ×
  UI 倍率) を各入口で扱わない。
- 採用: 配置の進み具合は、runtime の `host_placement` (`Unplaced { show_request }` / incarnation 付きの `Placed`)
  1 つで表す。host の作り直し・終了で別に失効させる処理を持たない。root 側からの表示要求も同じ状態に保持し、
  別の pending を作らない。
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
  なり成功に数えないことも確かめる。fake の winit 模倣は「`WM_DPICHANGED` の時点の client 寸法 × r」で作り直す
  (要求した寸法をそのまま現在の寸法にしない。レビュー第 3 版 P2-1)。
- 実モニター: 倍率の違うモニターが 2 台以上ある機械でだけ走る試験 (それ以外では skip し、skip したことを出力する)。
  非表示の winit 窓を作り、倍率の違う各モニターの中央・境界をまたぐ位置・モニターより大きい矩形へ `apply_outer_rect`
  して、`Exact` になることと実際の矩形・DPI を確かめる。開発機 (150% と 100% の構成) で実行する。
  2b の推奨矩形の左上の前提はこの試験で固定する。装飾あり・なし (仮想全画面)、150%→100% と 100%→150% の両方向を含め、
  すべて `Exact` になることを実装報告に含める。
  境界をまたぐ target、現在の大きさと target の大きさが違う場合、モニターより大きい target、上下に並んだモニター、
  同じ DPI の場合 (staging しない) を含め、適用後の矩形 = target を確かめる。
- 実 HWND (単一 DPI の範囲): winit (eframe と同じ backend) で非表示の窓を作り `apply_outer_rect` → `GetWindowRect` が
  target と一致すること。
- App: builder の仮の配置が target のモニターの DPI で論理化されること (単一 DPI では従来と同じ値になること。
  旧論理値 `x,y,w,h`、倍率 `s`、UI 倍率 `u`、実効の装飾 `F` について、物理外枠 `(sx, sy, sw+Fw, sh+Fh)` から作った
  builder の値が従来の `(x/u, y/u, w/u, h/u)` に戻ること。装飾なしは `F=0`、丸めは物理値で比べる)、
  登録時に適用されること (作成直後の窓の DPI が同じなら `SetWindowPos` しないこと)、ParkedLive / loading shell / holdover / backstop の登録で適用されること、ParkedLive の新しい host が
  Hidden で作られ登録・適用の後に helper で表示されること、passive が今と同じ builder の可視化のまま採用時に適用されること
  (単一 DPI では動かさないこと)、最大化して作った passive が別モニターに居るとき解除 → 適用 → 最大化し直すこと、
  builder が host の生きている間は位置・大きさを持たないこと、
  数値の同じ HWND を clear → 再登録したら新しい host として再配置されること、配置前の観測が runtime placement を
  上書きしないこと、最大化中の窓の F11 が `IsZoomed == false` まで幾何を適用しないこと、root 側の表示要求
  (トレイ復帰・native 遷移の Visible と FocusHost・再アクティブ化) が `Unplaced` では 1 つの `ShowRequest` に保持されて
  適用の直後に送られ、`Placed` では即送られること、保持中の非表示要求で古い要求が消えること、host の交代で
  保持中の要求が新しい host へ漏れないこと、Hidden deferred の進行 (passive が採用まで進み可視化される)、
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

## 7. レビュー指摘との対応 (履歴)

第 1〜3 版の表の「後の pass で commit」「`Shown`」などは当時の版の対応で、第 5 版の §3.4・§4 で置き換えた。
各指摘が第 5 版でどう扱われるかは、第 4 版の表と §3.4 を正とする。

### 第 1 版のレビュー指摘との対応

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

### 第 3 版のレビュー指摘との対応

| 指摘 | 対応 |
| --- | --- |
| P2-1 staging の式は winit が読む現在の client 寸法を確定させていない | §3.3: 2a で今の DPI のまま小さくして client 寸法を確かめてから、2b で m へ移す。推奨矩形の左上の前提は 2c の確認と実モニター試験 (§5) で固定。fake は client 寸法 × r で作り直す |
| P2-2 callback の外からの表示要求が初回描画の条件を迂回 | §3.4 の 6: 初回表示 (`Placed`) と再表示 (`Shown`) を分け、`Placed` では要求を保持して初回 commit で送る。捨てない |
| P3-3 pass 番号の境界がずれる | §3.4 の 3: pass 番号ではなく、egui の入力が適用後の client 寸法と DPI に一致した callback で commit |
| P3-4 §4 に旧 HWND 比較が残っている | §4 を `host_placement` に更新 |
| Mismatch の扱い | §3.3 の 4: 成功に数えず、メイン窓のトーストで通知、「必ず回収できる」とは扱わない、通常操作での発生は不具合 |

### 第 4 版のレビュー指摘との対応 (判定 ACCEPT WITH CHANGES)

| 指摘 | 対応 |
| --- | --- |
| P2-1 入力の寸法の一致は描画面の更新の十分条件でない | 第 5 版で描画の機会の判定そのものを廃止 (§3.4、§4)。既存の可視化の順序に乗る |
| P2-2 staging の外枠を style から求めると装飾なしで合わない、`S0` の制約が片側だけ | §3.3 2a: 実測の外枠と client の差、2b: 移動前と作り直し後の両方を制約。推奨矩形の左上の前提は実モニター試験まで未検証 |
| P3-3 `Unplaced` の要求の保持先が未定義、FocusHost が先に処理される | §3.2 `Unplaced { show_request }`、§3.4 の 4: 表示・最大化・前面化を 1 つの要求にまとめる。非表示で消す、host 交代で捨てる |

### 第 5 版のレビュー指摘との対応

| 指摘 | 対応 |
| --- | --- |
| P1-1 Hidden の passive は採用できず循環待ち | §3.4 の 5: passive は可視化を今のまま (Hidden にしない)。採用時に適用。Hidden の採用の仕組みは作らない |
| P2-2 ParkedLive は作成属性で見えるので順序が成り立たない | §3.4 の 3: 新しい host は Hidden で作り、登録・適用後に共通 helper で表示 |
| P2-3 passive の commit に一回限りの条件が無い | passive の commit を新設しないので論点が無くなる |
| P3-4 単一 DPI で「まったく同じ」の範囲 | §3.4 の 5・§4: 同一性は仮の配置の換算と passive の可視化について。倍率をまたぐときの過渡を明記 |
| 確認事項: builder の適用条件 (host 生存中も latch を渡す経路) | §3.4 の 3: 作る前だけに整理し、patch が上書きしないことをテスト |
| 確認事項: 仮の配置の同値性の式 | §5 の App テスト |

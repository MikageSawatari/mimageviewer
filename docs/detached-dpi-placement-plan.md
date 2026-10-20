# 別ウィンドウの配置を物理座標で 1 か所が持つ (backlog §1.384)

状態: 設計 (2026-10-20、設計担当 ClaudeCode Opus 5.5)。独立設計レビュー待ち。
detached 経路に触れるため、[detached-rework-plan.md](detached-rework-plan.md) §2 (憲法) と
CLAUDE.md「Detached viewer リワーク中のルール」に従い、構造的修正であることを設計担当と独立レビュー担当の
双方で合意してから実装する。合意と触れた範囲は同書 §11 に記録する。

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
      winit 0.30.13 は `CW_USEDEFAULT` で窓を作った直後の HWND の DPI (`hwnd_dpi`) で物理化する
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
2. **物理 ↔ 窓への適用は 1 つの境界だけが行う** (`detached_window_geometry` モジュール、§3)。
   detached 窓と全画面 viewport の top-level の位置・大きさを、egui の `with_position` / `with_inner_size` /
   `OuterPosition` / `InnerSize` で送らない (egui-winit が窓のそのときの DPI で物理化するため、
   対象モニターの DPI と一致する保証がない)。
3. **全画面の対象は対象モニターの物理 `rcMonitor` をそのまま使う**。DPI で割らない・掛けない。
   対象モニターは物理矩形から `MonitorFromRect(..., MONITOR_DEFAULTTONEAREST)` で引く。
4. **OS 窓は、最終の物理配置を適用し終えるまで見せない** (BA-2 のあるべき前提を、builder ではなく
   「最初の Visible commit の前に物理配置を適用する」として満たす)。適用済みかどうかは
   「この HWND に適用した」という事実 (runtime が持つ HWND 値) で判定し、時間で判定しない。
5. 時間窓・リトライ・画面外を後から戻す補正を主修正にしない (憲法 §2-5、backlog の指示)。

## 3. 設計

### 3.1 型

```rust
/// 物理デスクトップ座標の外枠矩形 (GetWindowRect と SetWindowPos が使う矩形)。
pub struct PhysicalRect { left: i32, top: i32, right: i32, bottom: i32 }

/// 通常表示の配置。外枠 (装飾込み) の物理矩形と最大化の有無。
pub struct PhysicalWindowPlacement { outer: PhysicalRect, maximized: bool }
```

- 外枠 (`GetWindowRect` の値、DWM の見えない枠を含む) で持つ。読む API と書く API が同じ矩形なので往復でずれない。
  旧形式は「外枠の左上 + client の大きさ」で、装飾の大きさを足し引きする所が DPI に依存していた。
- `DetachedViewerWindowPlacement` (論理) は **保存値の読み込み (移行) にだけ** 残す。runtime・restore・
  builder・保存先の書き込みは `PhysicalWindowPlacement` に置き換える。

### 3.2 所有者

- **runtime placement** (`DetachedWindowRuntime.placement`): `PhysicalWindowPlacement` にする。
  window_id ごとの正本であることは R2c のまま (新しい保存先・同期経路は作らない。憲法 §2-4)。
- **F11 の意図と restore** (`App.detached_viewer_borderless_fullscreen` / `detached_viewer_restore_placement`、
  唯一の書き手 `write_detached_viewer_borderless_state`): 場所はそのまま、restore の型を物理にする。
  - 場所を runtime へ移さない理由: F12 往復では旧 runtime を終端退役させてから新しい window_id を作り、
    その間も「仮想全画面だった」意図と restore を引き継ぐ (既存テスト
    `f12_round_trip_recreates_detached_host_with_borderless_mode_and_restore_placement`)。runtime へ移すと
    退役時の受け渡しという新しい経路が要る。今の単一の書き手で十分で、今回の不具合は単位の問題。
- **builder placement latch** (`DetachedWindowRuntime.builder_placement_latch`): builder が位置を持たなくなる
  ので不要になる。他の用途が無いことを実装担当が確かめたうえで撤去する (状態を減らす)。
  用途が残る場合は報告して止まる。

### 3.3 変換境界 `src/detached_window_geometry.rs` (新設)

純粋関数部 (テスト可能) と Win32 適用部に分ける。

- `monitor_for_rect(PhysicalRect) -> MonitorFacts { rc_monitor, rc_work, dpi }` (Win32、`MonitorFromRect` NEAREST)。
- `borderless_target(reference: PhysicalRect) -> PhysicalRect` = `monitor_for_rect(reference).rc_monitor`。
  reference は「生きている HWND の `GetWindowRect`」、無ければ restore / runtime placement の外枠。
  **メイン窓の ppp を使う代替経路は撤去する**。
- `apply_outer_rect(hwnd, target: PhysicalRect)` (Win32): 窓の DPI と対象モニターの DPI が違うとき、
  1. `SetWindowPos(SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE)` で対象の左上へ動かして対象モニターの
     DPI に入る (winit の `WM_DPICHANGED` 処理が論理サイズを保って大きさを変える。同期)。
  2. `GetDpiForWindow(hwnd) == 対象の DPI` を確かめ (事実の確認。待たない)、
  3. `SetWindowPos(target)` で正確な外枠を適用する。
  DPI が同じなら 3 だけ。2 が成り立たない場合 (まれ) はログを出して 3 を 1 回だけ行う (再試行しない)。
  - 1 を省いて 3 だけにすると、移動先で winit が `WM_DPICHANGED` を受けて大きさを論理サイズ基準に
    作り直し、3 の大きさが残らない (winit `event_loop.rs` の `WM_DPICHANGED`)。
- `read_outer_rect(hwnd) -> PhysicalRect` (`GetWindowRect`)、`is_zoomed(hwnd)`。
- 移行 `migrate_legacy_placement(legacy: DetachedViewerWindowPlacement, monitors: &[MonitorFacts],
  frame_for_dpi: impl Fn(u32) -> (i32,i32,i32,i32)) -> Option<PhysicalWindowPlacement>` (純粋、§3.6)。

### 3.4 適用の入口 (すべてこの境界を通る)

| 入口 | 今 | 変更後 |
| --- | --- | --- |
| active detached 窓の作成 (新規・F12 ON・always-new・promote 後の新しいメディア窓・host 消失後の作り直し) | builder に論理の位置・client 大きさ (borderless なら論理のモニター矩形) | builder は位置・大きさを持たず `Hidden` で作る。HWND を registry に登録する所 (created / adopted / recovered の全経路) で、runtime の物理 placement (borderless なら `borderless_target`) を `apply_outer_rect` で適用し、runtime に「適用した HWND」を記録する |
| passive 窓 (always-new の静止画窓) の作成 | `build_detached_image_window_builder` が論理の位置・大きさ、可視なら `with_maximized` | 同上。可視で作る経路も `Hidden` で作り、配置の適用と最初の描画の後に既存の Visible commit (最大化はそこで) で見せる |
| Visible commit (`send_detached_viewport_visible_commit` の全呼び出し元) | maximized → Visible | 前提として「この HWND に配置を適用済み」を runtime で確かめる。未適用なら見せずにログ (起きない想定の検査) |
| F11 入る | `Maximized(false)`・`Decorations(false)` の後、論理のモニター矩形を `OuterPosition` / `InnerSize` | 状態 command (`Maximized(false)` / `Decorations(false)`) を送る段と、物理の `borderless_target` を `apply_outer_rect` する段を分ける。後段は「最大化が解けている (`IsZoomed`=false)・装飾が外れている (style)」という事実を確かめてから適用する |
| F11 解除 | 論理の restore を `OuterPosition` / `InnerSize`、必要なら `Maximized(true)` | `Decorations(true)` の段 → 事実確認後に restore の外枠を `apply_outer_rect` → `maximized` なら `Maximized(true)` |
| 通常表示中の位置・大きさの記録 (active / passive) | egui の `outer_rect` / `inner_rect` を論理で記録 | HWND の `GetWindowRect` を記録。最大化中は外枠を更新せず `maximized=true` だけ記録する (active の現行規則に passive も揃える)。borderless 中・遷移中は記録しない (現行どおり) |
| 既定配置 (`default_detached_viewer_window_placement`) | メイン窓の論理位置 + 48、960x720 論理 | メイン HWND の外枠 (物理) の左上 + 48 × メイン窓の DPI 倍率、大きさ 960x720 × 同倍率 + 装飾。メイン窓のモニターに置く |
| 保存値の妥当性 (`title_bar_on_some_monitor`) | 論理座標を物理として `MonitorFromPoint` | 物理外枠の上端帯で `MonitorFromRect(DEFAULTTONULL)`。どのモニターにも無ければ既定配置 (モニターを外した場合の seed の選び直しで、窓を後から戻す補正ではない) |
| メインから開く全画面 viewport (同型) | 論理のモニター矩形を builder の位置・大きさ | 対象モニター = メイン HWND の外枠から `monitor_for_rect`。作成は `Hidden` → `rc_monitor` を `apply_outer_rect` → 既存の可視化。既に可視で作っている場合の可視化の順序は実装担当がコードで確かめて報告する |
| 全画面の背景 (`fullscreen_backdrop_physical_rect`) | 論理のモニター矩形 × メイン窓の ppp | `monitor_for_rect(メイン HWND の外枠).rc_monitor` |
| `detached_viewer_rect_physical` | 到達しない (調査で確認) | 撤去 |

- promote はメディアの context を付け替えるだけで窓の配置を変えない (現行どおり)。promote 後に別のメディアを開いて
  新しい窓を作る経路は、上の「active detached 窓の作成」に入る。
- detached の動画 presenter は host の client 矩形 (`GetClientRect`) に追従する子窓なので変更しない。
- 既定サイズ拒否の防波堤 (`detached_passive_placement_update_looks_like_default_viewport`) は撤去しない
  (rework-plan R2c の決定)。物理値で判定するよう単位だけ合わせる。

### 3.5 egui の viewport 情報との関係

- egui-winit は `WM_MOVE` / `WM_SIZE` / `ScaleFactorChanged` で `ViewportInfo` を更新するので、
  `SetWindowPos` で動かした結果は次の frame に egui へ届く。mIV 側で egui に位置を書き戻さない。
- builder が位置・大きさを持たないので `ViewportBuilder::patch` が `OuterPosition` / `InnerSize` を出さない。
  これにより latch が要らなくなる。

### 3.6 保存値と移行 (リリース済み: v1.5.0 から)

- `Settings.detached_viewer_window_placement` (論理) は v1.5.0 からリリース済み。移行と後方互換テストを用意する。
- 新しい field `detached_viewer_window_physical_placement: Option<PhysicalWindowPlacement>` (serde default `None`)
  を足し、以後の書き込みはこちらだけ。旧 field は読むだけで書き換えない (古い版へ戻した場合は古い値が使われる。
  割り切り)。settings の書き出し・取り込み (`settings_transfer.rs`) と複製 (settings.rs の copy) にも新 field を足す。
- 読み込み順: 新 field があればそれ。無く旧 field があれば、seed を作る所 (App、モニター情報が取れる所) で 1 回だけ移行:
  - 旧値は「外枠の左上 = 物理 ÷ s」「client の大きさ = 物理 ÷ s」(s はその窓が居たモニターの倍率)。
  - 各モニター M (倍率 s_M) について、旧値の中心 × s_M が M の `rcMonitor` に入るものを候補にする。
  - 候補が複数なら、旧矩形 × s_M と `rcMonitor` の重なりが最大のもの。候補が無ければ既定配置。
  - 外枠 = 左上 × s_M、大きさ = client × s_M + `AdjustWindowRectExForDpi(装飾ありの style, dpi_M)` の装飾。
  - 結果は runtime placement になり、窓を閉じたときに新 field へ保存される。
  - 推定が外れる配置 (モニター配置の変更後など) は、利用者が窓を動かせば次から正しい値が保存される (割り切り)。
- passive 窓の作成・F12 ON・新規作成はすべてこの seed を通る。

## 4. 状態の組み合わせを減らす検討 (CLAUDE.md の設計方針)

- 採用: builder が位置を持たないことで、builder latch と「egui の patch が送る位置 command」と
  「mIV が送る位置 command」の競合が無くなる。
- 採用: 位置の単位を 1 つ (物理) にし、変換を 1 か所に集める。DPI の組み合わせ (窓の DPI × 対象の DPI ×
  UI 倍率) を各入口で扱わない。
- 不採用: F11 の意図・restore を runtime へ移す案 (§3.2 の理由)。
- 不採用: 画面外の窓を見つけて戻す補正 (backlog の指示。今回の構造で画面外に作らない)。
  なお F11 の対象は窓の外枠から NEAREST でモニターを引くので、画面外に出てしまった既存の窓も F11 で
  最寄りのモニターに出る (副次的な効果で、主修正ではない)。
- まれな失敗 (`SetWindowPos` の失敗、§3.3 の 2 が成り立たない) は既定の割り切り: ログを出し、再試行しない。

## 5. テスト

- 純粋関数: 移行 (150%/100% の 2 モニターを右に離して並べた fake、旧値が各モニターにある場合・どこにも無い場合・
  境界をまたぐ場合)、`borderless_target` の選択、物理矩形の保存往復 (serde、旧 field だけの JSON・新旧両方の JSON)。
- 適用: `apply_outer_rect` を Win32 呼び出しの trait 越しに書き、fake で「移動で DPI が変わると窓の大きさが
  論理サイズ基準に作り直される」(winit の `WM_DPICHANGED` の挙動) を模して、適用後の物理矩形が target と
  一致することを検証する (backlog の「異なる初期 DPI の HWND に適用した後の物理矩形」)。
- 実 HWND: 非表示の top-level を作って `apply_outer_rect` → `GetWindowRect` が target と一致すること
  (テスト機の単一 DPI の範囲)。
- App: F12 往復・promote 後の新しい窓・always-new・F11 入る/解除で、builder が位置・大きさを持たず、
  runtime / restore が物理値で引き継がれること。Visible commit が未適用の HWND を見せないこと。
- 既存テストのうち、builder に位置があることを確かめるもの (`f12_round_trip_recreates_detached_host_with_borderless_mode_and_restore_placement`
  ほか、論理値を前提にした placement テスト) は、意図 (作り直した窓が対象モニターの配置を受け取る) を保ったまま
  物理値・適用記録の検証へ書き換える。書き換えた一覧と理由を実装報告に書く (憲法 §2-8)。

## 6. 範囲外

- メイン窓 (ROOT) の起動時配置・トレイ復帰 (既に物理の `SetWindowPos` / `WINDOWPLACEMENT`)。
- `Win+Shift+矢印` の位置ずれ (docs/dpi-multimonitor-issue.md)。
- キー入力 subclass の rect 探索 (R1 の対象外のまま)。
- detached の状態遷移・HWND registry・park / promote の所有境界そのもの。

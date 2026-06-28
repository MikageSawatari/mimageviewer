# Detached viewer lifecycle: 壊れている前提の一覧 + 作り直し設計案

作成: 2026-06-28 / ClaudeCode 横断レビュー

対象: F12 別ウィンドウ表示 (detached image window) と fullscreen viewport の
ライフサイクル。`src/app.rs` / `src/ui_fullscreen.rs`。

経緯: 「別アーカイブの画像を複数ウィンドウで同時表示」+「アクティブ窓だけ
編集/AI/先読み/スライドショー、非アクティブ窓は frozen 画像のみ」の実装過程で、
detached viewport の生成・配置・再生成・host 捕捉・active↔passive 往復まわりに
バグが連鎖。約 15 ラウンドの個別パッチでも収束していない。本書は **個別症状ではなく
「前提そのものが壊れている箇所」を洗い出し**、作り直し方針を提示する。

Phase A1/A2/B の関連ドキュメント:
- [detached-window-phase-a1-transient-audit.md](detached-window-phase-a1-transient-audit.md)
- [detached-window-phase-a2-runtime-separation.md](detached-window-phase-a2-runtime-separation.md)
- [detached-window-phase-b-placement-stabilization.md](detached-window-phase-b-placement-stabilization.md)

---

## 実装状況 (2026-06-28 更新)

§4 の **最小手術 S0 を適用済み**:

- **手術-1 (BA-3)**: host_lost からの自動 recreate を撤去。`render_fullscreen_viewport` は
  host_lost 検出時に `handle_detached_viewer_host_lost_before_render()` を呼び、診断ログ +
  stale hwnd 破棄のみ行う (generation を bump しない)。
- 旧 recreate 機械 — `should_recreate_detached_viewer_after_host_lost` /
  `mark_detached_viewer_host_stable_after_recreate` /
  `reset_detached_viewer_viewport_for_recreate` と
  `detached_viewer_host_lost_recreate_armed` / `detached_viewer_last_host_lost_recreate_at`
  フィールド — は **撤去済み**。
- **手術-2 (BA-2)**: detached の inactive builder
  (`build_inactive_fullscreen_viewport_builder`) は **host 未捕捉 (= 新規生成相当) のときだけ**
  placement を seed する。既存 window へは geometry を触らない。
- **手術-2b (BA-2 + 内容ブリッジ)**: `fs_nav_holdover_tex_for_draw` は新 idx の表示物
  (full / サムネ) が用意できる / デコード失敗が確定するまで holdover (前フレーム) を維持し、
  folder-nav の数フレームが黒にならないようにする。`poll_fs_nav_lock` は Failed でも lock 解放。
- **手術-3 (BA-4 の folder-nav 部分、実機動画 2026-06-28 で確定)**: detached + 「常に別ウィンドウ」
  モードで folder-nav (Ctrl+↑↓) すると、`load_folder → start_loading_items` が new items 導入前に
  `preserve_active_detached_image_window_for_main_context_change()` (= active 窓を passive snapshot へ
  park) + `close_fullscreen()` → `prepare_viewer_presentation_close()` で
  `detached_viewer_window_id = None` をしていた。detached の `fullscreen_viewport_id()` は
  **window_id 由来**なので、reopen 側の `ensure_detached_viewer_window_id` が新しい window_id を
  allocate → ViewportId 変化 → egui が OS 窓を破棄→再生成 (DWM フェードでデスクトップが透ける +
  既定サイズ 822x656 の小窓がカスケード)。修正: **fs_nav ロック中 (= folder-nav reopen 中) は
  park せず、`prepare_viewer_presentation_close` で window_id / presentation / live placement を
  維持**して、同じウィンドウの中で内容だけ差し替える。

そのため、以下 **§1.4 と BA-3 の本文は「撤去前 (= 修正前) の状態の記録」** として読むこと
(これらが言及する `host_lost_recreate_armed` 等の機械は現行コードには存在しない)。
§5 の構造リワーク (identity 一本化・hwnd 生成確定・rect 捕捉廃止・deferred 化) は未着手。

---

## 0. TL;DR

現行設計は **6 個の独立した「壊れた前提」** の上に立っており、それぞれを後追いの
ヒューリスティック（rect 一致・default 誤採用フィルタ・1500ms debounce・armed ガード）で
塞いでいる。ヒューリスティックは互いに干渉し、1 つが滑ると別のループを誘発する。

特に現行の Ctrl+↓ 小窓フラッシュ + host_lost ループは、次の 3 つの前提破綻が
連鎖した複合症状:

1. **BA-1**: OS ウィンドウを「論理矩形の一致」で同定できる、という前提（rect-based host capture）
2. **BA-2**: ウィンドウは「既定サイズで生成してから resize すれば正サイズになる」という前提（create-at-default-then-resize）
3. **BA-3**: `IsWindow()==false`（host_lost）を「自動 recreate すべき異常」とみなす前提（自己駆動 recreate ループ）

→ **最小手術 3 点**（§4）で出血を止め、その後 **構造リワーク**（§5）で根治する、の 2 段階を推奨。

---

## 1. 現状アーキテクチャの要約

### 1.1 ウィンドウの 3 形態

| 形態 | identity | content | 描画関数 |
| --- | --- | --- | --- |
| **active detached** (今操作中の 1 本) | `fullscreen_viewport_id()` = `from_hash_of(("fullscreen_viewer", fs_viewport_generation))` | live (編集/AI/先読み可) | `render_fullscreen_viewport` |
| **passive detached** (frozen な複数本) | `detached_image_window_viewport_id(id)` = `from_hash_of(("detached_image_window", window_id))` | frozen texture のみ | `render_detached_image_windows` |
| **fullscreen** (従来の全画面) | active と同じ `fullscreen_viewport_id()` | live | `render_fullscreen_viewport` |

ここで既に **identity の軸が 2 系統に割れている**:
- active は `fs_viewport_generation`（u64、recreate のたび +1）由来
- passive は `detached_viewer_window_id`（u64、本ごとに安定）由来

A2 で「detached の identity は window_id に寄せる」と決めたのに、**active 経路はまだ
generation を identity に使っている**（§BA-4）。

### 1.2 主要な状態フィールド（約 55 個）

- host 同定: `detached_viewer_host_hwnd`, `detached_viewer_last_outer_rect`,
  `detached_viewer_last_pixels_per_point`, passive 側 `passive_host_hwnd`
- viewport 表示: `fs_viewport_shown`, `fs_viewport_presentation`,
  `fs_viewport_generation`, `fs_viewport_recreate_after_hide`
- recreate/focus transient: `detached_viewer_recreate_on_next_render`,
  `detached_viewer_focus_requested`, `detached_viewer_no_activate_once`,
  `detached_viewer_host_lost_recreate_armed`, `detached_viewer_last_host_lost_recreate_at`,
  `fs_opened_at`, `fs_focus_*`, `fs_suppress_primary_until_release` ほか
- placement: `settings.detached_viewer_window_placement`(seed+永続),
  `active_detached_viewer_live_placement`(runtime 実測), `snapshot.placement`(passive)
- nav reopen: `detached_viewer_folder_nav_reuse_window_once`,
  `fs_nav_after_pdf_enumerate`(deferred), `fs_nav_locked_gen`,
  `detached_viewer_main_history_suppression_depth`
- context 本体: `active_detached_viewer_context: Option<ActiveDetachedViewerContext>`
  (= `ViewerContextBundle` を `swap_viewer_context_bundle` で出し入れ)

### 1.3 host 捕捉の仕組み（rect 一致）

`find_detached_viewer_host_hwnd_from_logical_rect`
→ `dwm_transitions::find_visible_thread_window_matching_rect(main_hwnd, expected_rect)`
→ `EnumThreadWindows` で同スレッドの可視窓を列挙し、各窓を:

1. main 窓・不可視を除外
2. `GetWindowRect` で物理矩形取得
3. 期待矩形の**中心点を含む** AND **期待面積の 2/3 以上を覆う** 窓だけを候補に
4. 四隅の Manhattan 距離が最小の窓を採用

期待矩形は `detached_viewer_last_outer_rect`(egui 論理) × `last_pixels_per_point` を
round して作る。

### 1.4 host_lost → recreate の自己駆動ループ

`render_fullscreen_viewport` 冒頭:

```text
detached_host_lost_before_render =
    detached && fs_viewport_shown
    && fs_viewport_presentation == DetachedWindow
    && detached_viewer_host_lost()           // IsWindow(host_hwnd)==false

if detached_host_lost_before_render:
    log host_lost_diag
    if should_recreate_detached_viewer_after_host_lost():   // 1500ms debounce + armed
        reset_detached_viewer_viewport_for_recreate()        // generation++, host=0
else (host 健在):
    mark_detached_viewer_host_stable_after_recreate()         // armed 解除
```

`reset_..._for_recreate` は `fs_viewport_generation += 1` する。
→ ViewportId 変化 → egui が OS 窓を破棄・再生成。

### 1.5 placement の適用タイミング（生成 vs resize）

`build_detached_viewer_viewport_builder(fs_idx, active, apply_placement)`:
- `apply_placement == true` のときだけ `with_inner_size` / `with_position` を渡す
- それ以外は title/decorations/transparent/taskbar だけ

呼び出し側:
- `render_fullscreen_viewport` の本描画: `apply_placement = need_show`（= 新規表示時 true）→ **正サイズで生成**
- **`keep_fullscreen_viewport_alive` の holdover**（nav 待機中の描画）:
  `build_inactive_fullscreen_viewport_builder(0)` → DetachedWindow 分岐は
  `build_detached_viewer_viewport_builder(fs_idx, None, false)` → **apply_placement=false**

passive 側 `build_detached_image_window_builder(window, apply_initial_placement)` も
`apply_initial_placement` が true の初回だけ placement を渡す。

---

## 2. 壊れている前提の一覧

### BA-1: OS ウィンドウを「論理矩形の一致」で同定できる ⚠️ 最重要

**前提**: detached 窓の HWND は、egui の outer_rect を物理化した期待矩形に
「中心含有 + 面積 2/3 + 隅距離最小」で一致する窓として一意に見つけられる。

**現実に壊れる条件**（いずれも実機ログで `candidate=none` を生む）:
- 新規 ViewportId の窓が**既定サイズ(822x656)で生成された直後**は、期待矩形
  (`last_outer_rect` = 旧正サイズ 2644x1260) と一致しない → 面積 2/3 条件を満たさず候補ゼロ。
- resize 途中フレーム（822→2644 へ遷移中）も一致しない。
- DPI(ppp)が捕捉時と生成時でずれると期待矩形がずれる。round 誤差も乗る。
- detached 窓が複数 + still fullscreen holdover 窓が同一スレッドに同居すると、
  別窓に**誤一致**しうる（中心含有 + 面積条件を別窓が満たす）。

**評価**: これは「egui が ViewportId に対応する実 HWND を返してくれない」という API 制約への
回避策。回避策が**ウィンドウの可視 geometry に依存**している時点で、生成直後・リサイズ中・
DPI 変化・多窓のすべてで滑る。`candidate=none` が 4 本のログで一貫＝**この方式は機能していない**。

**あるべき前提**: HWND は窓の geometry とは独立に、**生成イベントで 1 回だけ確定**して保持する
（§5.3）。以後は `IsWindow` 生存確認のみ。geometry で「探し直す」ことをしない。

---

### BA-2: 既定サイズで生成 → resize すれば正サイズになる ⚠️

**前提**: ViewportBuilder に placement を渡さず生成しても、直後に
`with_inner_size`/`ViewportCommand::InnerSize` で正サイズに直せばユーザーには見えない。

**現実**: 新 ViewportId の**最初の show が `keep_fullscreen_viewport_alive` の holdover を
通ると `apply_placement=false`** で OS 窓が egui 既定(822x656)で生成され、その 1〜数フレームは
既定サイズで可視。`render_fullscreen_viewport` が `need_show=true` で placement を適用するのは
その後 → **「一瞬小さいウィンドウ」フラッシュ**。ログの
`captured host ... 822x656` → `preserve ... 2644x1260` がこの順序を示す。

**あるべき前提**: ある ViewportId の **OS 窓が初めて生成される builder では、必ず最終 placement を
渡す**。生成と配置を別フレームに分けない。「未確定だから placement を渡さない」holdover/inactive
builder が DetachedWindow に存在すること自体が誤り。

---

### BA-3: host_lost(IsWindow==false) は自動 recreate すべき異常である ⚠️

**前提**: host HWND が消えていたら、viewport を作り直して復旧すべき。

**現実**: `reset_..._for_recreate` 自身が generation を bump して**自分で旧 HWND を捨てる**。
その直後〜capture が新窓に追いつくまでは、host_hwnd が 0 でない限り（=旧 hwnd を保持していると）
正常な過渡でも host_lost=true になる。recreate は generation を更に bump → 新窓 → capture 遅延 →
host_lost → … の**自己駆動フィードバックループ**。

`should_recreate_detached_viewer_after_host_lost` の 1500ms debounce + armed ガードは
**頻度を落とすだけでループを消さない**。armed は `mark_detached_viewer_host_stable_after_recreate`
（= 安定 host を rect 一致で捕捉できたとき）でしか解除されない。BA-1 により rect 一致が
失敗し続けると **armed が永久に立ったまま suppress ログを吐き続ける** か、debounce 明けに
recreate を再発する。

**あるべき前提**: viewport recreate は **ユーザー/明示イベント起点のみ**（presentation 切替、
borderless 切替、ウィンドウ ID 切替）。OS 側の真の close は egui の
`ViewportInfo::close_requested` / `Close` イベントで受ける。**geometry 由来の host_lost を
recreate トリガにしない**。

---

### BA-4: viewport identity を generation で頻繁に作り替えてよい

**前提**: active detached の identity は `fs_viewport_generation` でよく、recreate ごとに
bump して別 ViewportId にしても問題ない。

**現実**: ViewportId を変えるたびに egui は **OS 窓を破棄→再生成**（重い: フラッシュ・focus 移動・
host 再捕捉・font atlas resync・DWM transition）。detached は「本ごとに安定 ID」を持つべきなのに、
active 経路だけ generation を identity にしているため、**A2 で寄せた window_id 基準と二重化**。
nav/ recreate のたびに別人格の窓が生まれ、passive snapshot との往復が 1 フレームずれるだけで
破棄/再生成が起きる。

**あるべき前提**: detached 窓の ViewportId は **`window_id` のみから導出**し、active/passive で
同一。generation は「content の世代」(swap 判定)用途に留め、**OS 窓 identity に流用しない**。

---

### BA-5: immediate viewport を「親が毎フレーム描く」前提で多フラグ管理できる

**前提**: `show_viewport_immediate` の「親が描かないと即破棄」性質を、
`fs_viewport_shown` / `fs_viewport_recreate_after_hide` / `fs_nav_locked_gen` /
`fs_nav_after_pdf_enumerate` / deferred_reopen_wait などのフラグ組合せで正しく出し分けられる。

**現実**: 「このフレームで detached を描くか/隠すか/recreate するか/holdover を出すか」が
多数のフラグの AND/OR で決まり、**組合せ爆発**。nav 待機・close finalize・active↔passive 往復で
1 フレームの判定ミスが即「破棄→再生成」になる。15 ラウンドの修正が毎回別経路を壊したのは、
この状態空間が人間にもツールにも追えていないため。

**あるべき前提**: detached 窓は **deferred viewport (`show_viewport_deferred`)** にして lifetime を
egui に委ね、「毎フレーム描かないと死ぬ」制約から解放する。active/passive は **同一 deferred
viewport を 1 本のレンダラで描き**、live か frozen かだけを切り替える。

---

### BA-6: placement の所有者が settings / live / snapshot に三重化していてよい

**前提**: 「次に開く窓の seed (settings)」「今の窓の実測 (live)」「passive の保存値 (snapshot)」を
別々に持ち、必要時に同期すればよい。

**現実**: 三者がずれると既定サイズ(533x400 / 800x600 / 822x656)が「正」として採用される。
Phase B の `detached_passive_placement_update_looks_like_default_viewport`（800x600 近傍 + 急縮 +
大移動 + 直前が十分大なら拒否）は、**ズレを後追いで弾く保険**であって根治ではない。閾値外の
既定値（例: 822x656 ≠ 800x600）はすり抜ける。

**あるべき前提**: placement の **single source of truth を window_id ごとに 1 つ**持つ
(`WindowRuntime.placement`)。settings は「最後にユーザーが置いた位置」を永続するだけ、
snapshot/active はその runtime を参照するだけ。ヒューリスティック拒否は不要にする。

---

### BA-7: ~55 個の transient フラグで状態機械を表現できる

**前提**: one-shot bool 群（focus_requested / recreate_on_next_render / no_activate_once /
suppress_primary / host_lost_recreate_armed / grace 群…）の組合せで window の状態を表せる。

**現実**: 暗黙の状態機械を bool の積で表現しているため、1 つの取り違えで別経路が壊れる。
「どの組合せが正規状態か」がコードのどこにも明示されていない。

**あるべき前提**: 明示 enum:
`DetachedWindowState { Opening, Active, Parked, Resuming, Closing }` を window_id ごとに持ち、
遷移を 1 箇所（reducer）で管理。transient は遷移の副作用として閉じ込める。

---

## 3. 現行 Ctrl+↓ 小窓フラッシュ + host_lost ループの因果連鎖

BA-1〜BA-3 が連鎖した複合症状。detached な PDF/ZIP を開いた状態で Ctrl+↓:

1. **Ctrl+↓** → `close_fullscreen_for_folder_nav_reopen`。detached なので viewport を温存
   （generation / host / window_id 据え置き）、`folder_nav_reuse_window_once=true`、`fs_nav_locked`。
2. 次フォルダの PDF/ZIP を `load_folder` → **非同期 enumerate pending**。
   `reopen_fullscreen_after_folder_nav_load` が `fs_nav_after_pdf_enumerate`(deferred reopen) をセット。
3. 待機中の各フレーム: `keep_fullscreen_viewport_alive` が holdover を
   `build_inactive_fullscreen_viewport_builder`（DetachedWindow → **apply_placement=false**）で描画。
4. このフレームで `render_fullscreen_viewport` が走ると、capture は **`last_outer_rect`(旧正サイズ
   2644x1260) で holdover 窓を rect 照合できず candidate=none → host_lost 誤判定**（BA-1）→
   `should_recreate` → `reset_..._for_recreate` で **generation++**（BA-3）。
5. generation++ で ViewportId 変化 → 次フレームの keep_alive holdover が **placement 無し builder で
   新 OS 窓を生成 → 822x656 既定サイズで一瞬可視 = フラッシュ**（BA-2）。
6. enumerate 完了 → deferred reopen → `render_fullscreen_viewport` が `need_show=true` で
   placement 適用 → 正サイズ(2644x1260)へ resize。
7. しかし capture は resize 前/旧値の `last_outer_rect` 依存で新窓 hwnd を正しく取れず、再び
   host_lost → **4 に戻り ~300ms 周期でループ**。「次のファイルに行くたび小窓」、
   「何度かやると小窓のまま固着」（resize が間に合う前に次の recreate が来ると 822x656 が残る）。

→ 治すべきは個別フレームではなく、**(BA-3) 自動 recreate を断つ**・**(BA-2) 生成時に placement を
渡す**・**(BA-1) rect 捕捉をやめる** の 3 点。

---

## 4. 最小手術案（出血を止める / 低リスク / 即着手可）

構造リワーク前に、ループとフラッシュを止める 3 点。いずれも局所的で回帰テストを足せる。

### 手術-1: host_lost_before_render の自動 recreate を封印（BA-3）

`render_fullscreen_viewport` の `detached_host_lost_before_render` ブロックから
`reset_detached_viewer_viewport_for_recreate("host_lost_before_render")` 呼び出しを**外す**
（診断ログは残してよい）。

- genuine な OS close は egui の `close_requested` / passive 側の `Close` で既に拾えるはず。
  拾えていなければそちらを正とする。
- これで §3 の手順 4→5 の連鎖が断たれ、ループが止まるかを**まず実機で確認**する。
- 期待ログ: `recreate viewport: reason=host_lost_before_render` が 0 件、
  `suppress host_lost recreate` も 0 件に近づく。
- 回帰テスト: 「host_hwnd=0（未捕捉）で host_lost() が false を返す」既存性質に加え、
  「host_lost 検出時に generation が bump しない」ことを assert。

### 手術-2: detached の新規 OS 窓生成は必ず placement 付き builder にする（BA-2）

`build_inactive_fullscreen_viewport_builder` の **DetachedWindow 分岐を
`build_detached_viewer_viewport_builder(fs_idx, None, true)`**（apply_placement=true）に変える。
holdover でも初回生成時に `with_inner_size`/`with_position` を渡す。

- 注意: 既存窓が既に正サイズで生きているフレームに毎回 placement を再指定すると、
  ユーザーのドラッグ位置を引き戻す副作用がある（Fullscreen 分岐が geometry を触らない理由）。
  そのため **「その ViewportId の OS 窓がまだ生成されていない初回だけ apply_placement=true」** に
  限定する条件が要る。判定材料: `detached_viewer_host_hwnd == 0`（未捕捉=未生成相当）または
  「この generation で初めて show するフレーム」フラグ。
- これで §3 手順 5 の 822x656 フラッシュが消える。
- 回帰テスト: 「初回 show の builder が inner_size を持つ」「2 回目以降は持たない」を
  builder 単体テストで固定。

### 手術-3: capture の期待矩形を「リサイズ後の確定値」に限定 or 廃止（BA-1）

最小版: `last_outer_rect` を **placement 適用後（正サイズで描けたフレーム）にのみ更新**し、
holdover/resize 途中フレームでは capture を試みない。

- 本命は §5.3 の「生成イベントで hwnd 確定」だが、最小版としては
  「`fs_viewport_shown=true` かつ正サイズ描画済みのフレームでのみ capture / host_lost 判定」に
  ガードを足す。生成直後・holdover 中は host_lost を**評価しない**。
- 手術-1 を入れていれば host_lost が recreate を呼ばないので、最悪 candidate=none でも実害は減る。

**手術 1→2→3 の順で 1 つずつ入れて実機確認**するのが安全（同時に入れると切り分け不能）。
まず手術-1 だけで「ループが止まるか」を見るのが最短の切り分け。

---

## 5. 本命リワーク案（根治 / 中規模）

detached viewport を「本ごとに安定 ID・生成時に配置確定・hwnd は生成で確定・自動 recreate なし・
明示 state machine」の 1 モデルに統一する。fullscreen（従来全画面）はこのモデルの 1 ケースとして扱う。

### 5.1 identity を window_id に一本化（BA-4）

- active / passive / fullscreen すべて **`from_hash_of(("detached_window", window_id))`** を使う。
- `fs_viewport_generation` は **OS 窓 identity から切り離し**、content 世代（swap/再列挙判定）専用にする。
- presentation 切替（detached ↔ borderless fullscreen）でウィンドウ属性が変わって egui が
  OS 窓を作り直す必要がある場合のみ、window_id を**新規採番**して明示的に作り替える
  （= recreate は「新しい本/新しいモードを開く」ときだけ）。

### 5.2 deferred viewport 化（BA-5）

- detached 窓は `show_viewport_deferred(viewport_id, builder, |ctx, class| { ... })` で出す。
  「親が毎フレーム描かないと死ぬ」制約から解放され、holdover/nav 待機/close finalize の
  特殊フレーム分岐が大幅に減る。
- active と passive は **同一 deferred viewport を 1 本のレンダラ** `render_detached_window(window_id)`
  で描く。`state == Active` なら live content（編集/AI/先読み/スライドショー）、それ以外なら
  frozen texture。「どちらが描くか」の 1 フレームずれ問題が消える。
- egui 0.33 の deferred viewport の再描画は `ctx.request_repaint_of(viewport_id)` で必要時のみ。

### 5.3 hwnd は生成イベントで 1 回だけ確定（BA-1 根治）

- ViewportBuilder で OS 窓を出す **直前に `EnumThreadWindows` で既知 hwnd 集合 S0 を採取**し、
  出した**直後（最初のクロージャ実行後）に S1 を採取、`S1 - S0` の新規可視窓を hwnd とする**。
  geometry に依存しないので生成サイズに関係なく確定できる。
- 取れた hwnd を `WindowRuntime.hwnd` に保存。以後は `IsWindow` 生存確認のみ。
  **rect 一致 (`find_visible_thread_window_matching_rect`) は detached からは使わない**。
- それでも winit/egui の native handle が将来取れるようになれば（raw-window-handle 経由）、
  diff 法すら不要になる。まず diff 法で geometry 依存を断つ。
- ※ diff 法は「同フレームに複数窓を同時生成」しないことが前提。detached は逐次生成なので可。

### 5.4 placement の single source of truth（BA-6）

```rust
struct WindowRuntime {
    window_id: u64,
    state: DetachedWindowState,      // §5.5
    placement: DetachedViewerWindowPlacement,  // ← 唯一の真実
    hwnd: u64,                       // §5.3 で確定
    initial_placement_applied: bool, // 初回生成で placement を渡したか
    // content 側は別途 active のとき ViewerContextBundle を持つ
}
// App: detached_windows: IndexMap<u64, WindowRuntime>
```

- 生成 builder: `initial_placement_applied==false` のときだけ `with_inner_size`/`with_position`/
  `with_maximized` を渡す（§手術-2 と同じ規律を恒久化）。
- 実測 placement は描画クロージャ内で `outer_rect`/`inner_rect` から `runtime.placement` に書く。
- settings へは「ユーザーが最後に閉じた窓の placement」だけ保存（次回 seed 用）。
- → `detached_passive_placement_update_looks_like_default_viewport` ヒューリスティックは**削除**。
  生成時に正 placement を渡し、以後は実測を信じるので「既定値が紛れ込む」経路が無くなる。

### 5.5 明示 state machine（BA-7）

```rust
enum DetachedWindowState {
    Opening,    // OS 窓生成 → hwnd 確定待ち
    Active,     // live content。編集/AI/先読み/スライドショー可
    Parked,     // frozen texture のみ。passive
    Resuming,   // Parked → Active 復帰中（bundle swap-in）
    Closing,    // Close 送信 → 破棄確認待ち
}
```

- 遷移は 1 つの reducer 関数で行い、副作用（focus 要求・bundle swap・content cancel）を
  遷移に紐付ける。現行の散らばった one-shot bool 群はこの遷移の内部に閉じ込める。
- active は常に 0..1 本。新しい本を開く/別窓を active 化する = 旧 active を Parked へ、
  対象を Active へ、という 2 遷移として表現。

### 5.6 fullscreen（従来全画面）の扱い

- fullscreen は「decorations=false, taskbar 制御, モニタ全面 placement の detached 窓」と見なし、
  同じ `WindowRuntime` + レンダラに載せる。presentation を `Fullscreen | DetachedWindow | Borderless`
  の属性として持つ。
- これにより「fullscreen と detached で別の identity 軸（generation vs window_id）」という
  BA-4 の二重化が消える。

---

## 6. 段階移行プラン

| 段階 | 内容 | リスク | 検証 |
| --- | --- | --- | --- |
| **S0** | §4 手術 1→2→3 を 1 つずつ。ループ/フラッシュ停止を実機確認 | 低 | 既存テスト + 新規 builder/host_lost テスト |
| **S1** | placement を `WindowRuntime.placement` に一本化、Phase B ヒューリスティック削除（§5.4） | 中 | 多窓 resize/drag、active close 後の passive 維持 |
| **S2** | hwnd を生成 diff 法へ（§5.3）、rect 一致を detached から撤去（§5.1 host 部分） | 中 | F12 多窓、Ctrl+↑↓ 連打、PDF cold open |
| **S3** | identity を window_id 一本化（§5.1）、generation を content 世代専用化（§5.6） | 高 | 全 detached テスト + fullscreen 回帰 |
| **S4** | deferred viewport 化 + state machine（§5.2, §5.5） | 高 | 全面回帰、font atlas resync / focus |

- **S0 だけで現行バグが止まる可能性が高い**。S0 で止まれば S1〜S4 は腰を据えて進められる。
- S3/S4 は影響範囲が広い（fullscreen 全画面・動画 detached host switch・borderless 遷移）。
  これらは別 worktree で進め、`cargo test --bin mimageviewer-core` の detached 系を緑に保つ。

---

## 7. 未確認/要確認事項（実装前に潰す）

1. **genuine OS close の受け口**: 手術-1 で自動 recreate を外したとき、ユーザーが OS の×で
   detached 窓を閉じた場合に `close_requested` で正しく後始末できるか（現行コードの
   close_requested ハンドリング箇所を確認）。
2. **egui 0.33 で hwnd を native handle 経由で取れるか**: 取れるなら §5.3 の diff 法すら不要。
   `eframe` の `CreationContext`/`raw_window_handle` が child viewport にも使えるか要調査。
3. **deferred viewport の immediate-only 機能依存**: 現行 detached は IME / 入力 / 動画 presenter
   host switch で immediate viewport 前提の処理があるか（`pending_detached_video_host_switch` 等）。
   deferred 化で壊れないか S4 前に棚卸し。
4. **生成 diff 法の同時生成排他**: 複数 detached を 1 フレームで同時生成しない保証
   （逐次 open になっているか）。

---

## 8. 結論

- 現行の detached viewport は **rect 一致 host 捕捉 (BA-1)**・**生成時 placement 未適用 (BA-2)**・
  **自動 recreate ループ (BA-3)** の 3 つが噛み合って、Ctrl+↓ のたびに小窓フラッシュ + host_lost
  ループを生む。これらは個別フレームの不具合ではなく**前提の破綻**なので、パッチの逐次当てでは
  収束しない（実績: 約 15 ラウンド）。
- まず **§4 の最小手術 3 点**（特に手術-1 単独）で出血を止め、切り分ける。
- 根治は **§5 の構造リワーク**（identity 一本化・生成時配置確定・hwnd 生成確定・deferred 化・
  明示 state machine）。§6 の段階移行で S0→S4 と進める。

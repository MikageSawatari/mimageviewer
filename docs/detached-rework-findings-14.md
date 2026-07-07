# 検収所見 #14: 動画 detached 窓がドラッグ終了後に振動し続ける (2 値交互ループ)

正本プラン: [detached-rework-plan.md](detached-rework-plan.md)
実機 (2026-07-07 20:1x セッション、動画 detached = id=13)。「毎回ではない」再現。
ログは Fable が凍結・解析済み (scratchpad/vib_mimageviewer.log)。

## 症状と実測 (確定事実)

- 動画を detached 窓 (ON/OFF どちらのモードかはログ上 linked=true = F12 系) で再生中、
  **窓をドラッグして位置を変えると、ドラッグ終了直後から窓が振動し続ける**。
  今回は 284.971s〜303.06s の **約 18 秒間・939 回**、ユーザーが窓を close するまで継続。
- ログ署名 (`runtime_placement window_id=13 reason=active_placement_update`):
  - ドラッグ中は正常に単調追従 (1144→1146→…→1472→1564)。
  - **ドラッグ終了の瞬間から (x=1564.0, y=240.66667) ⇄ (x=1578.6666, y=244.0) の
    2 値を毎フレーム交互に保存** (7〜16ms 間隔)。Δ = 論理 (14.67, 3.33) = 物理 (22, 5) px。
  - 保存は 1 フレーム 1 回のみ。**交互の間に mIV 側の移動コマンド
    (OuterPosition / InnerSize / borderless 遷移 / rejected_default) のログは一切ない**。
  - 後半 (302s 台) は別の 2 値 (1064.67⇄1068.67、Δ=4 論理 px) に移っている =
    途中の再ドラッグで対が変わった。
- 既定サイズ拒否 (`detached_active_placement_update_looks_like_default_viewport`) は
  800x600 近傍限定で、今回の 1167x765 では発火し得ない (ログにも
  rejected_default なし) = **5722 の書き戻し経路は無罪**。
- 片方の値が**丸い論理値 (1564.0 = 物理 2346 の /1.5)**、もう片方が**端数
  (1578.6666)** — 「物理座標から計算した値で位置を SET する何か」と「winit の生の
  outer_rect 報告」の 2 系統が交互になっている署名。

## 含意

- 保存経路 (`save_detached_viewer_placement_from_logical_rect`, 呼び出しは
  [ui_fullscreen.rs:5711](../src/ui_fullscreen.rs) の 1 箇所のみ) は「報告された
  outer_rect を記録しているだけ」で、**OS 窓の位置自体が毎フレーム実際に往復している**
  (ユーザーに振動が見えている)。動かしている側はログに現れていない = 計装の穴。
- 候補 (Fable の推定、優先順):
  1. **active 描画の 2 call site** ([ui_fullscreen.rs:5214](../src/ui_fullscreen.rs)
     keepalive 系 / [5603](../src/ui_fullscreen.rs) 本描画) が交互に走り、builder の
     `apply_placement` (= `detached_viewer_should_seed_placement()`) の評価が
     フレームごとに異なる → egui が builder 差分で position を再適用。
  2. native 側の host 追従 (presenter / subclass / VST clamp 系のどれかが
     物理 rect 由来の SetWindowPos を毎フレーム発行)。丸い値 1564.0 の出所として有力。
  3. winit の set_outer_position / outer_position 非対称 (DWM invisible border) との
     複合。ただし (22,5) px は標準の border オフセットと一致しない。
- 発生条件が「毎回ではない」のは、ドラッグ終了位置の座標値 (丸め一致で無害化するか)
  に依存している可能性が高い。

## 指示 (Phase 1: 計装で発火源を確定 → Phase 2: 修正は Fable 承認後)

憲法 5/6 どおり、**ダンピング (座標差の閾値で無視する等) による症状抑制は禁止**。
実際に窓を動かしている writer を特定して 1 本化する。

### Phase 1 (計装、`MIV_DETACHED_WINDOW_DEBUG=1` ゲート内)

1. **位置系コマンドの全発行点にログ**: detached viewport 向けの
   `OuterPosition` / `InnerSize` / builder の `with_position` 適用 (= egui への位置指示)
   を、送信側の関数名 + 値付きで 1 行ずつ記録するヘルパーに集約する。
2. **save 地点の強化**: `active_placement_update` に (a) 呼び出し元 (keepalive/本描画の
   どちらか) (b) `detached_viewer_should_seed_placement()` の評価値 (c) 同時点の
   `GetWindowRect(host)` (物理の真実) を併記。
3. **native 側の host 移動候補にログ**: `sync_detached_video_child_presenter_rect` /
   subclass / VST clamp 系で host HWND に SetWindowPos し得る箇所。
4. ビルド → ユーザーが動画 detached でドラッグを数回 (再現するまで) → ログ解析で
   writer 確定。C3 保持があるので退避不要。

### Phase 2 (修正、Fable 承認後)

- writer が特定できたら、「placement の SSoT は runtime、position の適用はユーザー
  操作・明示遷移 (F11/F12/復元) のみ」の原則に合う形で該当経路を修正する。

## Phase 1 結果 (Fable 解析 2026-07-07 夜): writer = active_render の毎フレーム builder position

計装ログ (placement_trace) で確定:

- `source=active_render event=builder_with_position` が**毎フレーム** (1080 回)、
  `apply_placement=true` かつ **`seed_now=false`** で記録された。
- 発生源 = [ui_fullscreen.rs:5590-5592](../src/ui_fullscreen.rs) の
  `detached_seed_placement = need_show || should_seed_placement() ||`
  **`detached_video_presentation_active_or_targeted()`** — 3 つ目の条件で
  **detached 動画 active 中は毎フレーム builder に position/size が入る**。
  これは「旧 host 生存のまま新既定サイズ窓が生える」フラッシュ対策 (Codex 実機
  レビュー P1、BA-5 時代の防波堤) として意図的に入ったもの。
- **振動の力学 (period-2 遅延フィードバック)**: builder の position 値 = 「前フレームに
  保存した報告値」(1 フレーム遅れ)。egui は builder の値が変わると OuterPosition を
  patch する。窓が静止していれば builder 値 = 現在位置で no-op だが、**ドラッグ終了
  時点で「最後の 2 フレームの報告値 A, B」が異なると**: builder(N)=A を適用 →
  報告(N+1)=A → 保存 → builder(N+2)=A…ではなく適用がコマンドキュー経由で 1 フレーム
  遅れるため、報告が A,B,A,B… の**周期 2 軌道として固定**され、最後の 2 値の間を
  永久に往復する。ドラッグを離す瞬間の 2 フレームがたまたま同値なら発火しない =
  「毎回再現しない」と一致。コメントの「ドラッグ中は modal loop で再 diff しない」は
  正しいが、**ドラッグ後**の echo が考慮漏れ。

## Phase 2 指示 (Fable 承認済みの修正方針): builder に渡す placement 値をラッチする

毎フレーム seed 自体 (= フラッシュ防波堤) は残してよいが、**builder に渡す
position/size の「値」を live placement 追従にしない**:

1. active 描画用の **builder placement ラッチ**を導入する (置き場所は
   DetachedWindowRuntime か active viewport runtime の値フィールド。App 直下の
   新規 bool ではない = 憲法 3 準拠)。
2. ラッチの更新は **seed が本当に必要な契機のみ**: `need_show == true` または
   `detached_viewer_should_seed_placement() == true` の評価時に、その時点の
   live placement をラッチへコピー。
3. `detached_video_presentation_active_or_targeted()` による毎フレーム seed では
   **ラッチ値をそのまま builder に渡す** (live placement を読まない)。
   → 窓の生存中は builder の position/size が定数になり、egui の patch が発火しない
   = ドラッグ後の echo が構造的に消える。再生成契機 (need_show / seed) では
   ラッチが更新されるので、フラッシュ防波堤の効果は維持される。
4. **`with_inner_size` も同じラッチを使う** (リサイズでも同型の echo が起き得るため)。
5. 禁止: 座標差の閾値・「静止したら追従」等のダンピング (憲法 5)。
6. テスト:
   - ドラッグ相当のシーケンス (保存 placement が毎フレーム変わる) で builder への
     渡し値が不変であること (ラッチが live 追従しない)。
   - need_show / should_seed 契機でラッチが最新 live placement に更新されること。
   - 回帰: 保存 placement がドラッグ終了後 1 値に収束する (A/B 交互にならない)
     シーケンステスト。
7. コミット `(detached-rework findings-14)`。計装 (placement_trace) は残置してよい。

## 完了条件

- [ ] Phase 1 計装 + 再現ログで writer 確定の報告。
      コミット `(detached-rework findings-14 diag)`
- [ ] Phase 2 修正 + 回帰テスト (ドラッグ終了後に placement 保存が 1 値に収束する
      シーケンステスト)。コミット `(detached-rework findings-14)`
- [ ] full test 緑 / fmt / glyphs / build-release

## 備考

- stage-audio fix2 (F11 2 バグ) と作業が重なる場合は fix2 を先に完了させてから着手
  (同一ファイル域)。
- 本件は見た目が派手な P1 級 (窓が永続的に振動) なので、リリース前に必ず解消する。


## Phase 2b (実機 2026-07-08 未明): ラッチが復帰境界で更新されず、フォーカスで窓がジャンプする

### 実測 (jump_cur.log)

- active 中に窓を移動 → live placement は追従、ラッチは (87.3, 71.3) のまま (設計どおり)。
- park 中の passive/parked-live 描画の builder は **live 値**を使う → egui の builder
  キャッシュが live 値になる。
- 復帰すると active 描画の builder が**古いラッチ値**を渡す → egui が差分適用 →
  **窓がラッチ位置 (87.3, 71.3) へジャンプ** (t=881.3 / 976.1 / 980.1 の 3 回実測、
  いずれも実位置 → ちょうどラッチ値へ)。

### 修正要件 (Phase 2b)

- ラッチの refresh 契機に「**アクティブ描画への遷移境界**」を追加する:
  park からの復帰 commit (Resuming→Active) / adopt_passive / active 描画が非 active
  状態から最初のフレームを描くとき、に live placement でラッチを更新する。
  イベント (状態遷移) ベースで判定し、時間窓は使わない。
- 連続 active 中はラッチ据え置き (Phase 2 の echo 防止特性を維持)。
- テスト: active 中に live placement を動かす → park → 復帰、のシーケンスで
  復帰後の builder 渡し値が**復帰時点の live 値**であること (ジャンプしない)。
  既存の echo 防止テスト (drag A/B 非追従) は不変。
- コミット `(detached-rework findings-14 fix2)`。

### Phase 2b 実装メモ (Codex 2026-07-08)

- `transition_detached_window_state()` の非 Active → Active 境界で
  `refresh_detached_builder_placement_latch_for_active_entry()` を呼び、runtime の現在
  placement を builder latch に再同期する。
- 連続 Active 中の `active_detached_builder_placement_latch(false, ..)` は従来どおり
  latch を更新しないため、Phase 2 のドラッグ後 echo 防止は維持される。
- 回帰テスト `detached_builder_placement_latch_refreshes_on_active_reentry` を追加。

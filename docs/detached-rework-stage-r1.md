# Stage R1 指示書: HWND は生成イベントで 1 回だけ確定 — rect 捕捉を detached から全廃

正本プラン: [detached-rework-plan.md](detached-rework-plan.md)
**着手前に必ずプラン §2 (憲法) を読むこと。**

- 対象: 提案書 BA-1 の根治 (提案書 §5.3 / 段階 S2 相当)
- 方式は R0 で確定済み: [detached-rework-stage-r0-report.md](detached-rework-stage-r0-report.md)
  — `EnumThreadWindows` の before/after 差分。実機ログで 8/8 生成が
  `created_count=1`・同期生成・副次窓混入ゼロを確認済み。
- 実装: Codex / 検収: Fable / 実機 smoke: ユーザー (ゲート B)

## 1. このステージで成立させる不変条件

R1 完了後、detached viewer の HWND 管理は次の 3 行で説明できる状態にする:

1. detached 窓の HWND は、**その viewport の OS 窓が生成されるフレームで
   before/after 差分により 1 回だけ確定**し、`window_id` に紐付けて保存する。
2. 以後その HWND は **`IsWindow` による生存確認のみ**。フレーム中の再探索・
   「clear して捕捉し直す」操作は存在しない。
3. 窓の見た目の矩形 (rect) から HWND を推定するコードは detached 経路に存在しない。

## 2. 実装方針

### 2.1 hwnd registry (SSoT)

- `window_id (u64) → hwnd (u64)` の対応を持つ**単一の**構造
  (例: `DetachedWindowHwndRegistry`) を App に 1 つ追加する。
  - 憲法 3 (App への新フィールド禁止) の**明示的な例外として許可**する。ただし
    散在 bool ではなく registry 1 個に限る。R2 で `DetachedWindowRuntime` に
    吸収される前提の設計にする (コメントで明記)。
  - `ViewerContextBundle` 内の `detached_viewer_host_hwnd` など bundle に同居する
    HWND コピーは、可能なら registry 参照へ置き換えて撤去する。1 セッションで
    撤去しきれない場合は「読み取り専用の cache」と明記し、**書き込み経路を
    registry 1 本に絞る**ことを優先する (どちらにしたかを完了報告に書く)。
- active / passive を問わず**同じ registry** を使う (active↔passive 遷移で HWND の
  持ち主が変わらない = 提案書 §5.1 の先取り)。

### 2.2 生成時差分による確定

- 各 detached viewport の `show_viewport_immediate` 呼び出し点 (R0 で計装した
  active_render / passive / keep_alive_holdover / keepalive_backstop の 4 経路) で:
  - registry に当該 window_id の HWND があり `IsWindow` が真 → **何もしない**
    (snapshot も取らない。通常フレームのコストはこの分岐 1 つ)。
  - 無い / 死んでいる → before snapshot → show → after snapshot → 差分で確定。
- 差分の解釈 (R0 実測に基づく):
  - **1 件**: その HWND を registry に確定。class が `"Window Class"` であることを
    確認 (違ったらログして不採用)。生成直後は `visible=false` で正常。
  - **0 件**: OS 窓が生成されなかった (egui が既存窓を再利用)。registry は触らず
    次フレーム再試行。ログを出す。
  - **2 件以上**: ambiguous。**推測で選ばない** (rect / 面積での選別に戻さない =
    憲法 1)。未確定のまま次フレーム再試行し、警告ログを出す。title での絞り込みは
    補助ログとしてのみ許可 (採用判定に使わない。title は内容で変わるため)。
- 未確定の間の挙動: 従来 `detached_viewer_host_hwnd == 0` だった状態と同じ扱い
  (host 依存の処理は skip)。**時間窓によるタイムアウトを設けない** (憲法 5)。

### 2.3 撤去対象 (完了条件の grep リスト)

detached 経路の rect 推定と、その誤同定を前提とした防波堤・stopgap を全て消す:

| 撤去対象 | 場所 |
| --- | --- |
| `capture_detached_viewer_host_hwnd_from_logical_rect` と呼び出し | src/app.rs |
| `find_active_detached_viewer_host_hwnd_from_logical_rect` / `find_detached_viewer_host_hwnd_from_logical_rect` | src/app.rs |
| passive 窓の HWND を rect で推定している経路 (`passive_host_hwnd` への書き込み元を追って同様に diff 法へ置換) | src/app.rs / src/ui_fullscreen.rs |
| stopgap 一式: `clear_detached_viewer_host_if_matches_passive_window` / `clear_detached_viewer_host_if_closing_passive_window` / `passive_detached_host_hwnds` / `passive_detached_window_id_for_host` | src/app.rs |
| `find_visible_thread_window_matching_rect_excluding` (excluding 変種と除外リスト機構。無印 `find_visible_thread_window_matching_rect` は非 detached 用途が残るため**残す**) | src/dwm_transitions.rs |
| R0 の並走ログ (`[detached-r0]` 系) — 実装に置き換え。`debug_thread_window_snapshot` は diff 法の部品として昇格・改名してよい | src/app.rs / src/dwm_transitions.rs / src/ui_fullscreen.rs |

**触らない** (スコープ外、憲法 7):

- rect 捕捉の非 detached 用途: `raise_visible_thread_window_matching_rect`、
  仮想デスクトップ同期 (`fs_viewport_virtual_desktop_synced_hwnd` 経路)、
  キー入力 subclass 導入 (ui_fullscreen.rs)
- placement の保存・seed ロジックと default-rejection 防波堤
  (`detached_capture_rect_looks_like_default_viewport` の**捕捉側**は上の表で消えるが、
  placement 保存側の default-rejection は R2 で扱う)
- host_lost 後の挙動設計変更 (recreate は既に S0 で封印済み。R1 は判定元が
  registry になるだけ)

### 2.4 ログ (検証可能性)

- `captured host` 相当のログは残す: 「window_id / hwnd / どの経路の生成で確定したか」。
  R1 後の健全性指標 = **host の確定は窓生成時のみ、クリアは窓 close 時のみ**。
  ゲート B でこれをログで確認する。
- 差分 0 件 / 複数件の再試行は `MIV_DETACHED_WINDOW_DEBUG=1` 時のみ詳細ログ。

## 3. テスト要件

- `EnumThreadWindows` は headless テストで実窓を列挙できないため、snapshot 取得を
  注入可能にする (例: `#[cfg(test)]` で synthetic snapshot を差し込む。前例 =
  `native_video_key_physically_down` の cfg(test) 分岐)。
- 新規テスト (最低限):
  - 差分 1 件 → registry 確定、以後 snapshot を取らない
  - 差分 0 件 / 2 件以上 → 未確定のまま、registry 不変、再試行される
  - 死んだ HWND (IsWindow=false 相当) → 再確定経路に入る
  - active↔passive 遷移で registry の対応が維持される (window_id 基準)
  - stopgap 撤去後の回帰: 既存の stopgap 用テストは削除してよい (指示書による
    明示なので憲法 8 の例外。削除したテスト名を完了報告に列挙)
- 既存 detached テスト (104 本±) は緑を維持。仕様変更で赤くなるものは事前に
  Fable へ列挙して確認 (憲法 8)。

## 4. 完了条件

- [ ] §2.3 の撤去対象が grep で 0 件 (完了報告に grep コマンドと結果を貼る)
- [ ] `cargo fmt --check` / `cargo test --bin mimageviewer-core` / `cargo test` (フル) 緑
- [ ] §3 の新規テストが存在して緑
- [ ] `.\scripts\build-release.ps1` で実機検証バイナリを用意
- [ ] 完了報告に: 設計判断 (bundle 内 HWND コピーの扱い)、削除テスト一覧、
      既知の残リスク

## 5. ゲート B (実機 smoke、ユーザー)

`MIV_DETACHED_WINDOW_DEBUG=1` で R0 と同じ 5 操作 (F12 detach / リサイズ・移動・
最大化 / passive 2 + active 1 の切替 / 動画 F12 往復 / Ctrl+↑↓ reopen)。

合格基準:

- **動画ウィンドウの左右振動が消えている** (今回の直接目標)
- host 確定ログが「窓生成時のみ」出る (フレームごとの capture / clear チャーンが
  ログから消える)
- 小窓 (822x656) フラッシュ・位置リセットの新規発生なし
- `panic.log` に新規 panic なし

不合格の場合: 症状を BA 番号に対応付けて報告し、Fable の判断を待つ (憲法 6)。

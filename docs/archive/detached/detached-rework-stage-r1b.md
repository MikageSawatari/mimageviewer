# Stage R1b 指示書: passive アクティブ化をクリック限定にする + show 中再生成の取り逃がし回復

正本プラン: [../../detached-rework-plan.md](../../detached-rework-plan.md)
**着手前に必ずプラン §2 (憲法) を読むこと。**

- 位置付け: ゲート B (R1 実機 smoke) で見つかった 2 件の修正。R1 の registry 本体は
  合格済み (振動・rect 誤同定は解消をログで確認済み)。
- 根拠ログ解析: 2026-07-05 実機ログ 84.77〜85.6s。フォーカス到達だけで passive を
  アクティブ化 → 現 active を退避 → 窓の出入りでフォーカスがまた別の passive に落ちる
  → 連鎖、のピンポンを確認 (`passive_activate_queued via=focus`、全て
  `pointer_activation=false`)。
- 実装: Codex / 検収: Fable / 実機 smoke: ユーザー (ゲート B 再実施)

## 1. 修正 1: passive 窓のアクティブ化はポインタクリックのみ (ユーザー決定 2026-07-05)

**決定**: passive → active の復帰トリガは「passive 窓内でのポインタクリック」のみ。
**フォーカス到達 (Alt+Tab 含む) だけでは絶対にアクティブ化しない**。Alt+Tab で
passive 窓を前面にした場合は「表示されるだけ」で、クリックするまで passive のまま。

- [ui_fullscreen.rs:3859](../src/ui_fullscreen.rs) 付近の
  `if can_activate && window.activation_armed && (focus_activation || user_activation)`
  から `focus_activation` を外す (= `user_activation` のみ)。
- `via=focus` のアクティブ化経路を削除する。`passive_event` ログの focus 系フィールド
  (focused / focus_edge) は**診断用に残す**。
- `focus_activation_suppress_until` (close 直後のフォーカス抑止時間窓) は、focus
  アクティブ化が消えることで存在意義を失うはず。**pointer 経路で本当に不要かを確認の
  うえ、不要なら時間窓ごと削除**する (憲法 5 の負債返済)。pointer 経路で必要な理由が
  あれば残してよいが、理由を完了報告に書く。
- **仕様書の同時更新** (プラン外のドキュメントだが本修正の正本):
  [../../detached-viewer-implementation-plan.md](../../detached-viewer-implementation-plan.md)
  §3 表⑦ の「Passive 窓をクリック / フォーカスして Active 化」を「クリックで Active 化
  (フォーカス到達だけでは Active 化しない)」に改訂し、理由 (2026-07-05 の
  フォーカスピンポン実害、ログ解析は
  [detached-rework-stage-r0-report.md](detached-rework-stage-r0-report.md) 系列) を
  1 行残す。[detached-viewer-smoke-matrix-20260630.md](detached-viewer-smoke-matrix-20260630.md)
  に該当ケースがあれば表現を揃える。
- 回帰テスト:
  - focus_edge=true / pointer_activation=false では activation が queue されない
  - pointer クリックでは従来どおり queue される
  - (既存の focus 前提テストがあれば書き換え。書き換えたテスト名を完了報告に列挙)

スコープ外 (触らない): アクティブ化に伴う現 active の退避処理
(`park_current_active_detached` の `close_legacy_detached` 分岐等) の再設計は R2 で扱う。

## 2. 修正 2: show 中の窓再生成を registry が取り逃がす穴の回復

実機ログで確認した事象: 登録済み HWND が生きている場合 before-snapshot をスキップ
するが、egui がその show 呼び出しの**内部で** OS 窓を破棄→再生成すると、次フレーム
以降 `detached_hwnd_dead` → before-snapshot 再開時には新窓が既に存在
→ `no_new_window` リトライが継続する (実測 37 フレーム。次の再生成でたまたま回復)。

**修正: show 後の生存チェック + 消去法による未請求窓の採用**

- snapshot をスキップしたフレームでも、show 後に登録済み HWND の `IsWindow` を確認
  する (安価)。死んでいたら registry を clear し、**未請求窓スキャン**を 1 回行う:
  1. UI スレッドの top-level 窓を列挙し、class == `"Window Class"` (egui/winit) で
     フィルタ (native presenter の窓は別 class なので自然に除外される。念のため
     完了報告で class 名の実測値を確認)
  2. main 窓と、registry に登録済みの全 HWND を除外
  3. 残りが **ちょうど 1 件**ならその HWND を当該 window_id に採用 (ログ
     `hwnd_adopted_unclaimed` 等で記録)
  4. 0 件または 2 件以上なら採用せず、従来どおり次フレーム再試行 (警告ログ)
- これは集合の消去法であり、矩形・面積・距離などの geometry 推定ではない (憲法 1 に
  適合)。**2 件以上のときに「近い方」等で選ぶことは絶対にしない**。
- 回帰テスト (synthetic snapshot 注入で):
  - スキップフレームで登録済み HWND が死亡 → 未請求 1 件を採用
  - 未請求 2 件 → 採用しない
  - 未請求 0 件 → 従来どおり retry

## 3. 完了条件

- [ ] `via=focus` アクティブ化経路が存在しない (grep: `via=focus` が activation
      queue 経路に 0 件)
- [ ] §1 / §2 の回帰テストが存在して緑
- [ ] `cargo fmt --check` / `cargo test --bin mimageviewer-core` / `cargo test` (フル) 緑
- [ ] implementation-plan §3 表⑦ の改訂を含む
- [ ] `.\scripts\build-release.ps1` で実機検証バイナリを用意
- [ ] 完了報告に: `focus_activation_suppress_until` の扱い (削除したか、残した理由)、
      書き換えたテスト一覧

## 4. ゲート B 再実施 (実機 smoke、ユーザー)

前回と同じ 5 操作 + 追加観点:

- **動画 F12 往復でウィンドウのアクティブが暴れない** (今回の直接目標)
- Alt+Tab で passive 窓に切り替えたとき: 表示はされるがアクティブ化しない。
  その後クリックでアクティブ化する
- passive 窓のクリックによるアクティブ化が従来どおり動く
- `no_new_window` リトライが長時間続かない (`hwnd_adopted_unclaimed` で即回復するか、
  そもそも発生しない)
- 左右振動・小窓・panic の再発なし

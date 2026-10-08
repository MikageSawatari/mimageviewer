# T2 最終再レビュー: 待機・再駆動・所有境界 (v4.0.0 出荷直前)

読み取り専用のコード照合のみ。アプリは起動しておらず、実行時の観測は無い。以下はすべて
作業ツリー (未コミット差分) の source を読んだ限りの推定で、根拠に `file:line` を添える。

## 結論 (出荷可否)

- **P1: 0 件。** 永久モーダル・永久「読込中」・入力が戻らない・別窓の状態破壊・クラッシュに
  相当する経路は、今回の待機 owner / read lease / tray 保留の差分では見つからなかった。
- **P2: 1 件 (T2-1)。** 待機が意図的に「保留」される区間でも 10 分の安全予算が消費されるため、
  長いフルスクリーン閲覧のあとに偽のタイムアウト表示が出る経路がある。出荷を止める性質ではない。
- 元指摘 RC-2 / RC-3 / S1-2 / S1-3 / S1-6 はいずれも解消と判断する。

## 元指摘の判定

| 指摘 | 判定 | 根拠 (作業ツリー) |
| --- | --- | --- |
| RC-2 (Busy/Starting の無限 20Hz 再駆動) | **解消** | `src/collection_store/read_lease.rs:10-14,316-339` で 50ms→200ms→1s backoff + active 10 分の絶対期限。Grid は `src/app/collection_grid.rs:1371-1387,1694-1732` で `Failed{message:"コレクションの読み込みがタイムアウトしました", installed}` へ終端昇格。manager は `src/ui_dialogs/collections.rs:897-911,176-205`。navigation は `src/app/collection_navigation.rs:1848-1868` で終端し、`:3370-3374` の `finish_collection_navigation_without_target_no_context` が Slideshow/OuterFullscreen で `release_fs_nav_lock()` を呼ぶので **fs_nav lock は解放される**。定常状態は 1Hz (`SLOW_POLL`)。 |
| RC-3 (`AwaitingPdfPassword` が 16ms poll) | **解消** | `src/app/collection_navigation.rs:358-372` で `AwaitingPdfPassword => None`。lease は `:2167` で `pause`、`:1550` / `:2103` で `resume`。通知用に `CollectionRevisionWake` (`:217-249,2173`) を起こすスレッドを持つ。worker 待ちは一律 50ms (`completion_poll_delay`)。継続待ち 2 種も 16ms→50ms。 |
| S1-2 (待機モーダル中のトレイ格納で全窓のキーが止まる) | **解消** | `src/tray_integration.rs:243-248,626-646`。`root_tray_hide_modal_owner` が sidecar / smart_folder 遷移 / smart_folder 確認 / saved_group / rename 復旧の 5 owner を列挙し、いずれかが可視なら `CancelClose` を送ったうえで **hide せず**、理由を通常ログへ残す。`maybe_intercept_close` の戻り値は `src/app.rs:73581` で捨てられるため終了もしない。`:253-255` のコメントどおり、同一フレーム後半の terminal でも遡って隠さない。 |
| S1-3 (`Starting` の待ちに上限が無い) | **解消** | `src/ui_dialogs/collections.rs:912-927` は `Starting` を `lease.defer(now,"starting")` で backoff、それ以外の非 Ready は即 `Failed{"コレクションを利用できません。"}`。期限切れは `:903-911` で `Failed{"コレクション一覧の読み込みがタイムアウトしました。"}`。番号キー側は `src/app/saved_group_actions.rs:490-497` で `take()` 済みの request を戻さずトーストするので**モーダルは閉じる**。runtime 自体は `read_lease.rs:404-422` の `note_deadline_elapsed` で境界を 1 回だけ記録するだけで、actor を殺したり再起動したりしない (設計どおり)。 |
| S1-6 (待機モーダルが Esc で閉じない) | **解消** | `src/app/saved_group_actions.rs:292,302-304` が `dialog_escape_pressed(ctx)` (= `src/app.rs:18668-18670`、IME 変換中は false) を closure の**前**で取り、`cancel_saved_group_open_request(id)` に繋ぐ。取消は `saved_group_open` を `take` して `lease.finish("cancelled")` するので、遅着結果で開き直る経路は無い (`:218-231`)。 |
| 新たな問題 | **T2-1 (P2)** | 下記。 |

## 期限値の妥当性 (「毎回期限切れ」にならないか)

`COLLECTION_READ_ACTIVE_BUDGET = 10 分` (active-time、`read_lease.rs:10`)。台帳の合成 10k 計測
(actor 書込 93.6ms / snapshot 読込 4.9ms / prepare 253.6ms) に対して 3 桁の余裕がある。
PDF パスワード入力は `pause`/`resume` で予算から除外される (`read_lease.rs:341-392`)。
HDD / ネットワーク data-dir / AV スキャン / 初回 backup (VACUUM INTO) が数十秒かかっても、
active-time 10 分には届かない。**正常だが遅い環境で毎回期限切れになる設計にはなっていない。**
唯一の例外が T2-1 (期限が「処理時間」ではなく「利用者の閲覧時間」で消費される経路)。

---

## T2-1 [P2] 保留中 (fullscreen leaf / 非 current context) の read lease も 10 分予算を消費し、復帰時に偽のタイムアウトになる

**重要度**: P2 (永久固着・クラッシュではない。誤ったエラー表示と、その変更ぶんの再読込が落ちる)

**根拠**

- `src/app/top_level_grid_view.rs:849-875` — `CollectionGridSession::cancel_pending()` は
  `Ready` / `Empty` / `Failed` / `Deleted` からの再要求で `CollectionReadLease::new(..., "revision")` /
  `"retry"` を作る。`new` は `read_lease.rs:176-192` で**その場で Active 化し、`active_deadline = now + 10分`**
  を焼く (`dormant` と違い遅延開始ではない)。`:892-899` の `invalidate_thumbnail_presentation` も同じ。
- `src/app/collection_grid.rs:1635-1651` — 改訂通知の処理は `collection_grid_root_materialize_active()`
  の早期 return (`:1670-1672`) **より前**にあり、`position == Root` なら
  **フルスクリーン表示中でも `cancel_pending()` が走る**。
- しかし `:1351-1353` (`schedule_collection_grid_snapshot`) と `:1670` により、
  `fullscreen_idx.is_some()` の間は要求を駆動しない (`collection_grid_refresh_waits_for_viewer`
  (`:1546-1567`) が「フルスクリーン葉の解放待ち」と説明する意図的な保留)。
- lease 側にこの保留に対応する `pause` は無い (`pause` の呼び出しは
  `src/app/collection_navigation.rs:2167,2886,5076` の PDF パスワード待ちのみ)。
  `expired()` は `read_lease.rs:394-400` のとおり poll の有無に関係なく経過時間だけで真になる。
- 復帰時、`:1371-1387` (RequestNeeded) または `:1694-1732` (Snapshot/Preparing) が
  即座に `Failed{"コレクションの読み込みがタイムアウトしました"}` を立て、
  `:1596-1608` の `collection_grid_stale_error_message` 経由で
  `render_collection_grid_error_overlay` (`src/app.rs:74745`) にエラーが出る。

**失敗シナリオ (推定)**

1. コレクション root を開き、そこから画像をフルスクリーンで開く (`position` は Root のまま)。
2. 閲覧中にそのコレクションの改訂が 1 上がる。実例: フルスクリーンから「コレクションに追加」、
   Del で削除 (`collection_grid.rs:631-651` の `invalidate_current_collection_grid_sources` →
   `cancel_pending()`)、別窓 / Remote / メタデータ取り込みによる更新。
3. その時点で 10 分の予算を持つ lease が作られ、**フルスクリーンのあいだ一切駆動されないまま**時計だけ進む。
4. 10 分以上読んでから Esc で一覧へ戻ると、最初のフレームで
   「コレクションの読み込みがタイムアウトしました」が出る。DB もワーカーも正常で、
   単に利用者が長く読んでいただけ。
5. 一覧は古い内容のまま残り、次にさらに新しい改訂通知が来る (または開き直す) まで自動復帰しない
   (`cancel_pending` は `revision > wanted_revision` のときしか呼ばれない)。開き直せば
   `top_level_grid_view.rs:1277-1282` の `begin` が新 session (dormant lease) を作るので復帰する。

同型の経路として、複数ウィンドウで**前面にいない viewer context** の session がある
(`invalidate_collection_grid_sources` (`collection_grid.rs:653-671`) が全 context に対して
`cancel_pending()` を呼ぶが、その context の `poll_collection_grid` は current になるまで走らない)。
10 分以上そのウィンドウへ切り替えなければ、切り替えた瞬間に同じ表示になる。

**最小の修正方向** (出荷前に入れるなら、どちらか一方で足りる)

- (推奨・2 行相当) `top_level_grid_view.rs:852-873` / `:894-898` の `CollectionReadLease::new` を
  `CollectionReadLease::dormant(...)` に変える。`schedule_collection_grid_snapshot` は
  `collection_grid.rs:1370` で既に `lease.activate(now, "admission")` を呼ぶので、
  **実際に駆動され始めた時点から予算が始まる**。保留中は時計が動かない。
  期限・backoff・終端昇格の契約は一切変えない。
- (構造的) 保留区間を PDF パスワード待ちと同じ `pause`/`resume` で扱う
  (`poll_collection_grid` で `!collection_grid_root_materialize_active()` のとき `pause`、
  materialize 再開時に `resume`)。`Snapshot` / `Preparing` のままフルスクリーンへ入った場合も塞がる。

**判断材料**: 利用者は「重要な問題だけ直して出荷」と明言している。これは誤表示であって
固着・破壊ではないため、**見送って v4.0.x へ回す判断も妥当**。見送る場合は
「既知の問題」に載せる必要も無い程度 (再現に 10 分の連続閲覧 + 改訂を要する)。

---

## その他 (指摘としては起票しない観測)

- `collection_grid_root_materialize_active()` (`collection_grid.rs:1518-1524`) は
  再レビュー時点と同じく `window_visible` を見ない。ただし `RequestNeeded` は backoff で
  1Hz (`SLOW_POLL`) まで落ちるため、背面・トレイ格納中でも 10/s を超えない。
- 一方 `Snapshot` / `Preparing` と manager の `InFlight`、および Starting 中の
  `runtime_observation` は `completion_poll_delay()` = **固定 50ms (20Hz)** で、
  可視状態も backoff も掛からない (`read_lease.rs:286-298`、`collection_grid.rs:1538-1539`、
  `ui_dialogs/collections.rs:2733-2737`)。実測では in-flight が 1 秒未満なので実害は見込まれないが、
  actor / prepare worker が数十秒ブロックする環境 (切断されたネットワーク上の参照を
  prepare が分類する等) ではトレイ格納中も 20Hz が続く。設計上は 10 分で終端する。
- `read_lease.rs:123-139` の `Drop` は `Arc::strong_count == 1` のときだけ `retired` を出す。
  2 つの clone が別スレッドで同時に drop すると両方とも出さない可能性があるが、
  lease は UI スレッド専有なので実害は無く、計装だけの話。
- `try_lock + sleep` 型の再試行は差分中に無い。lease は actor と一切やり取りしないため、
  read が actor の変更をブロックすることも、変更が read を飢餓させることも無い
  (`read_lease.rs:1-4` の module doc の主張とコードが一致している)。
- 通常経路の 1 フレーム遅延は入っていない。新規 lease は `next_poll_at = now`
  (`read_lease.rs:504-514`) なので `is_due` が即真になり、同一フレームで admission へ進む
  (`collection_grid.rs:1370-1371`、`collection_navigation.rs:1030-1045`、
  `ui_dialogs/collections.rs:881,897`)。stale 結果の棄却 (`collection_grid.rs:1814-1839,1902-1913`) と
  fullscreen leaf 表示中の root 再 install 保留 (`:1546-1567,1670`) は維持されている。

## 出荷前に 1 回測るべき idle health

1. `check-idle-health.ps1 -Scenario static-background` と `-Scenario tray-residency` を、
   **通常フォルダではなくコレクション root を開いた状態**で 1 回ずつ測る。
2. 失敗したら `target/idle-health/*-perf.json` の `ui.tail_repaint.action` を見る。
   `request_repaint_after_collection_grid` / `_collection_ui` が 10/s を超えて続いていれば本件。
3. 期待値: 読み込み完了後は `none` / `idle_upgrade`、混雑中でも `RequestNeeded` は 1Hz まで落ちる。

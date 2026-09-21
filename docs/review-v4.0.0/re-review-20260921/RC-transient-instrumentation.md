# v4.0.0 コレクション 再レビュー — 一過性状態の typed 化 (C-1/C-2/C-3/B-3) と計装 (B-6)

対象: `e10669da2` (計装) / `3d8f5d279` (transient)、基準は **HEAD = `a5a1fc43b`**。
作業ツリーの未コミット差分 (KeyAction 追加) には触れていない。行番号はすべて
`git show HEAD:<path>` 時点。**読み取り専用でコードを読んだだけであり、ビルド・テスト・
アプリ起動・実機観測は行っていない。以下はすべてコードを読んだ限りの推定である。**

---

## 1. 元指摘ごとの判定

| ID | 判定 | 根拠 (要点) |
| --- | --- | --- |
| **C-1** (read が `can_edit` に巻き込まれる) | **部分的** | read 入口が `collection_store_client_for_read` (`src/ui_dialogs/collections.rs:1911-1929`) に分離され、phase だけを見る。`can_edit()` (`:673-679`) は `reorder.phase.is_busy()` を含むが、Grid snapshot (`src/app/collection_grid.rs:1167`)・watch 再購読 (`:1381`)・navigation (`src/app/collection_navigation.rs:966`) はすべて read 入口を通る。`CollectionUiAction::Open` は `requires_ready()==false` (`collections.rs:4356`) なので並べ替え保存中でも開ける。**残存**: 読み取りしかしない書き出し 2 経路が `can_edit()` 依存のまま (RC-4)。 |
| **C-2** (一過性を終端 `Failed` に格上げ) | **解消 (別問題あり)** | `is_read_retryable()` = `Busy \| Starting` (`src/collection_store/model.rs:336-338`)。Grid は admission・reply とも retryable なら `RequestNeeded { not_before: now+50ms }` (`collection_grid.rs:1170-1177, 1200-1209, 1556-1568`)、終端 (`Unavailable`/`IncompatibleSchema`/`Persistence`) は `Failed`、`NotFound` は `Deleted` (`:1569-1583`)。`Failed`/`Deleted`/`Ready`/`Empty` は `collection_grid_poll_delay` の `_ => None` (`:1302`) に落ちるので**終端は再駆動も repaint もしない**。→ **上限・backoff・終端昇格が無い** のが新規の問題 (RC-2)。 |
| **C-3** (管理 UI の要求を無言 drop) | **解消** | `CollectionReadSlot<R,D>` = `Idle / RequestNeeded{demand,not_before} / InFlight{request,next}` (`collections.rs:97-101`) の単一 owner。`request_catalog`/`request_snapshot` は `InFlight` 中の需要を `next` に畳み (`:726-742, 812-848`)、`drive_*` が Busy を食っても `RequestNeeded` を維持 (`:780-786, 898-904`)。同 revision の明示 refresh も `next` に残る。選択切替は `select_collection` が旧 slot を `Idle` にして旧 `SnapshotRequest` を drop し、その `Drop` が `outcome="cancelled"` を出す (`:138-166`)。runtime 交換/終了は `shutdown_for_exit` が両 slot を `Idle` (`:710-711`)。**window を閉じても需要は消えないが、要求生成は notice / event / 明示操作駆動のみで毎フレームではない**。 |
| **B-3** (`Snapshot`/`Preparing` 待ちに repaint 駆動が無い) | **解消** | `App::update` 末尾が `collection_grid_poll_delay` / `collection_ui_poll_delay` を **毎 pass 再予約** (`src/app.rs:75404-75424`)、即時 `reasons[]` には入れない。perf の `action` に `request_repaint_after_collection_grid` / `_collection_ui` を追加 (`:75436-75441`)。navigation は自前の tail (`collection_navigation.rs:2186-2190`)。 |
| **B-6** (計装ゼロ) | **概ね解消 (集計のみ未対応)** | §2 参照。`cat="collection"` を 15 種。全 call site が `is_enabled()` の外ガード付き、毎フレーム経路なし、source path / コレクション名なし。**`scripts/analyze_perf.py` に collection サブコマンドが無い** (RC-6)。 |
| **B-13** (nav pending の 16ms repaint) | **未解消** | `CollectionNavigationPending::poll_delay()` (`collection_navigation.rs:322-329`) は `RequestNeeded` 以外を一律 `16ms`。worker 待ちに加えて**利用者入力待ちの `AwaitingPdfPassword` も 60Hz** (RC-3)。 |
| **B-20** (`Starting` の 20Hz に上限が無い) | **未解消 (対象が増えた)** | `collection_ui_poll_delay` の `runtime_delay` (`collections.rs:2170-2174`) に加え、Grid (`drive` 相当の `:1170-1177`) と navigation (`:976-979`) も `Starting` を 50ms で回し続ける。上限なし。 |
| **T2** (一過性と終端の混同) | **主要 3 経路は解消 / 周辺に同型が残る** | RC-4 / RC-5 参照。 |
| **T4** (計装ゼロ) | **計装は入った / 測る道具は無い** | RC-6。 |

---

## 2. B-6 計装の精査結果

- **`--perf-log` 無効時のコスト**: 全 call site が外ガード付き。
  `crate::perf::is_enabled().then(Instant::now)` で時計を取り、`if let Some(start)` の内側でのみ
  `serde_json::Value` と `format!` を組む
  (`collection_grid.rs:151-152,171,204,213,240,268,311,1073,1112,1189,1502,1630,1750,1930`,
  `collection_navigation.rs:560,695,948,1024,1546,2795,3266`,
  `collections.rs:768,885,2951,3419,3517` + `collection_ui_actor_rtt` は非 Option の `Instant` を
  取るため呼び出し元で必ず判定済み、`prepare.rs:323,389`, `runtime.rs:1020,1030,1062`,
  `persistent_collections.rs:911,981,1027,1165`, `top_level_grid_view.rs:812`)。
  ヘルパー 3 本 (`collection_grid_prepare_result_event` `collection_grid.rs:2144-2149`,
  `remote_exact_perf_event` `persistent_collections.rs:1157-1165`,
  `log_collection_grid_pending_cancel` `top_level_grid_view.rs:812-813`) は自前で早期 return。
  **未ガードの call site は見つからなかった。**
- **毎フレーム出るイベント**: 無い。すべて「actor 応答到着」「prepare 完了」「install」「取消」
  「open」「import/export/migrate」の単発契機。
- **プライバシー**: 記録は `collection_id` (UUID)、revision、`entries`/`candidates`/`parents`/
  `mappings`/`bytes`/`lines`、`ms`、`outcome`、`surface_generation`/`request_sequence`、
  `context` (`ViewerContextId` の Debug)。**source path もコレクション名も含まれない**
  (import/export/preflight/navigation すべて確認)。
- **挙動への影響**: 追加されたのは `queued_at: Option<Instant>` / `perf_started_at: Option<Instant>`
  フィールドと、その `Drop`/取得だけ。制御分岐は増えていない。ただし
  `CatalogRequest`/`SnapshotRequest` の `Drop` (`collections.rs:138-166`) は計装のためだけに
  `Drop` 実装を足しているので、将来この型を move する変更を入れると偽の `cancelled` が出る。
- **未対応**: `scripts/analyze_perf.py` に `collection` が 1 か所も無い (`from collections import`
  のみ)。`docs/ui-responsiveness.md §4.4` は追加済み。

---

## 3. 新規指摘

### RC-1 [P2] navigation の terminal read error が `CollectionStoreError` の**英語 Display** をそのまま利用者トーストに出す

**根拠**
- `src/app/collection_navigation.rs:1069-1082` `finish_collection_navigation_read_error` は
  `show_feedback_toast(format!("コレクションを読み込めませんでした: {reason}"))`。
- その `reason` に `&error.to_string()` を渡す箇所が 4 つ:
  `:984` (read client が終端)、`:1003` (`subscribe` 終端)、`:1019` (`load_collection` admission 終端)、
  `:1775` (`Snapshot` reply の終端 error)。
- `CollectionStoreError` の `Display` は英語 (`src/collection_store/model.rs:341-356`):
  `"collection store is unavailable"` / `"collection or entry was not found"` /
  `"collection revision conflict: expected {} actual {}"`。
- 日本語化ヘルパーは存在するが**別モジュールの private**:
  `collection_error_message` (`src/ui_dialogs/collections.rs:970-994`)、
  `collection_grid_error` (`src/app/collection_grid.rs:2175-2183`)。
- 同じ関数の他の呼び出しは日本語リテラル (`:976` `"保存先が利用できません"`、`:1813`
  `"応答が途切れました"`) なので、意図的な英語ではなく**揃え漏れ**と推定する。

**失敗シナリオ**: コレクションのスライドショー中にストアが終了/失効した状態で次コマへ進むと、
`コレクションを読み込めませんでした: collection store is unavailable` が表示される。
削除済みコレクションを開いたままの手動送りでは
`コレクションを読み込めませんでした: collection or entry was not found`。
この 4 経路は 3d8f5d279 で新設されたもの (従来は `finish_collection_navigation_without_target`
でメッセージ無し) なので、**v4.0.0 が初出の利用者可視文字列**になる。

**修正方向**: `collection_error_message` を `pub(crate)` にして 4 箇所で使う
(`collection_grid_error` と重複しているので、どちらか 1 つを正本にして両方から呼ぶのが
CLAUDE.md「1つの意味を2か所に書かない」に沿う)。
handler-level テストでトースト文言に ASCII の内部語が出ないことを固定する。

---

### RC-2 [P2] `Busy` / `Starting` の再駆動に上限・backoff・終端昇格が無い (idle health と fs nav lock)

**根拠**
- Grid: `COLLECTION_GRID_READ_RETRY_INTERVAL = 50ms` (`collection_grid.rs:10`) 固定。
  `RequestNeeded { not_before: now+50ms }` を繰り返すだけで attempt 数も deadline も持たない
  (`:1170-1177, 1200-1209, 1556-1568`)。`collection_grid_poll_delay` は
  `RequestNeeded{Some(deadline)}` に対して残り時間を返す (`:1291-1294`) → **20Hz で無期限**。
- 管理 UI: `COLLECTION_READ_RETRY_INTERVAL = 50ms` (`collections.rs:40`)。
  `drive_catalog_request` / `drive_snapshot_request` が `Starting` と Busy で
  `not_before = now+50ms` を再設定し続ける (`:752-757, 780-786, 868-874, 898-904`)。
  `collection_ui_poll_delay` は `InFlight` に対しても一律 50ms (`:126-129`) → **20Hz で無期限**。
- navigation: `COLLECTION_NAV_READ_RETRY_INTERVAL = 50ms` (`collection_navigation.rs:11`)。
  `defer_collection_navigation_read` (`:1051-1068`) に attempt/deadline が無い。
  対照的に **Remote 側は `MAX_EXACT_RESTARTS` と `deadline` を持ち、超過で `busy_error()` を返す**
  (`src/remote_ipc/persistent_collections.rs:906-908, 979-981`)。PC 側だけ無制限。
- `Busy` は `COMMAND_CAPACITY = 128` の `TrySendError::Full` でのみ発生
  (`src/collection_store/runtime.rs:690, 713`)。actor が長い 1 コマンド
  (10,000 件の `LoadCollection` / `export_all_snapshot` / `migrate_source_batch`) を処理中で、
  Remote が 1 request あたり最大 16 回 subscribe+load を投げる構成
  (B-21 / D の指摘) なら飽和は起こり得る。`Starting` は `open_at` が長引く間 (ネットワーク DB、
  AV スキャン中) 続く。

**失敗シナリオ (推定)**
1. **idle health**: コレクション root を表示したまま (または読込途中で) ウィンドウを背面へ回す /
   トレイへ格納する。`collection_grid_root_materialize_active()` (`:1274-1280`) は
   `fullscreen_idx.is_none() && position == Root` だけを見て **`window_visible` を見ない**ので、
   隠れていても 20Hz の `request_repaint_after(50ms)` が続く。
   `check-idle-health.ps1` の既定 `max_update_rate` は 10/s
   (`scripts/analyze_perf.py` の `analyze_idle_health` 既定 `max_update_rate=10.0`) なので、
   `static-background` / `tray-residency` の測定窓にこの状態が入ると **FAIL する**。
   リリース手順 9.7 は 4 シナリオ必須なので出荷ゲートに直撃する。
   (現行の 4 シナリオは対象がフォルダなので、コレクションで測らない限り顕在化しない可能性が高い。)
2. **navigation**: `Slideshow { outer: true }` / `OuterFullscreen` の pending は
   `owns_fs_navigation_lock()` (`collection_navigation.rs:376-392`) が真なので、
   `RequestNeeded` で待つ間ずっと `fs_nav_lock` を握る。admission が回復しない限り
   **無期限に「読込中」のまま 20Hz**。利用者は Esc / フルスクリーン終了で抜けられる
   (`collection_navigation_request_is_current` が false になり `release_fs_nav_lock`) ので
   永久固着ではないが、待ちの終わりを知らせる表示もタイムアウトも無い。

**修正方向**: read slot / nav pending に `attempts` か `first_deferred_at` を持たせ、
(a) 50ms → 200ms → 1s の backoff、(b) 上限超過で終端へ昇格
(Grid は `Failed { message: "コレクション処理が混み合っています" , installed }`、
navigation は `finish_collection_navigation_read_error`) にする。Remote が既に
`deadline` + `MAX_EXACT_RESTARTS` で同じ判断をしているので、その形を PC 側へ揃えるのが
「同じ判断を 2 箇所で別実装にしない」(C-2 の元指摘) の継続になる。
併せて `collection_grid_root_materialize_active` に `window_visible` を足すかどうかは
別判断 (背面でも読み込みを進める仕様なら、backoff だけで update rate を落とす方が素直)。

---

### RC-3 [P3] `AwaitingPdfPassword` が「利用者の入力待ち」なのに 16ms poll を出し続ける

**根拠**
- `CollectionNavigationPending::poll_delay()` (`collection_navigation.rs:322-329`) は
  `RequestNeeded` 以外を一律 `Duration::from_millis(16)`。
- `AwaitingPdfPassword` (`:1956-1986`) は利用者がパスワードを入力して送信するまで保持される。
  解決条件は「watch notice」「target が current でなくなる」「利用者入力」だけで、
  16ms の再訪では何も進まない。
- `AwaitingOuterContinuation` / `AwaitingManualContinuation` も `landing_ready` が false の間
  同じ 16ms で回る (`:2084-2098`)。

**失敗シナリオ**: コレクション内のパスワード付き PDF へ送ると、パスワードダイアログが出ている
数十秒〜数分のあいだ `App::update` が 60Hz で回る。IME で日本語入力する場合はさらに長い。
`static-foreground` の idle health は「測定前に準備完了」前提なので通常は引っかからないが、
ノート PC の電力と `perf_smoke` のフレーム分布には効く (B-13 の想定コストと同じ)。

**修正方向**: `poll_delay()` を状態ごとに分け、`AwaitingPdfPassword` は `None`
(watch の `wake_receiver()` と利用者入力の repaint に任せる)、worker 待ち
(`Snapshot`/`Preparing`/`Preflighting`/`PdfPasswordPreflighting`) は 50ms へ揃える (B-13 の修正方向)。

---

### RC-4 [P3] `can_edit()` に残る読み取り専用経路 (書き出し 2 本)

**根拠**
- `start_collection_export_snapshot_request` (`collections.rs:3017-3025`) は
  `can_edit()` で弾いたあと `client.load_collection()` しかしない。
- `collection_export_all_available` (`:3040-3042`) = `can_edit() && operation.is_idle()`、
  `start_collection_export_all` (`:3044-3066`) は `client.export_all_snapshot()` のみ。
- どちらも C-1 の元指摘と同じ「読み取りが並べ替え保存中に巻き込まれる」形。

**失敗シナリオ**: 並べ替え window を dirty のまま閉じて `Saving` に入っている 1 フレームの間に
「書き出し」「全コレクションを書き出す」を押すと
「コレクションを現在編集できません。」「別のコレクション処理が進行中です。」で弾かれる。
実害は小さい (利用者が押し直せる) が、C-1 の構造修正が届いていない箇所。

**修正方向**: 両方を `collection_store_client_for_read()` 経由にし、
`Err(retryable)` はメッセージ「コレクションを準備しています / 混み合っています」+ 再試行案内、
終端のみ失敗にする。`operation.is_idle()` のガードは worker 一本化のために残す。

---

### RC-5 [P3] 同型が残っている箇所の一覧 (`Busy`/`Starting` を終端扱い or 再駆動 owner なし)

| 箇所 | 現状 | 影響 |
| --- | --- | --- |
| ツールバー追加 `add_grid_selection_to_collection` (`collections.rs:1882-1906`) | `load_collection` の `Err` を種別問わず失敗トースト | 利用者操作なので押し直せる。P3 |
| 並べ替えの再読込 `start_collection_reorder_refresh` (`:1376-1399`) と `Refreshing` reply (`:1479-1485`) | `Busy`/`Starting` を `CollectionReorderPhase::Error` (終端) に落とす | 「最新内容を読み込む」ボタン起点なので押し直せる。P3 |
| 書き出し / 全書き出し (`:3026-3037`, `:3064`) | 同上 | RC-4 と同じ |
| import 確定 `start_collection_grid_content_action` (`:2496`) / `request_collection_grid_remove` (`:4320`) | mutation なので `can_edit()` ガードは妥当 | 問題なし |
| Remote producer (`persistent_collections.rs:2062-2063, 2102`) | `Busy`/`Starting` を専用 error code にして端末へ返す。`deadline` + `MAX_EXACT_RESTARTS` あり | 上限を持つのはこちらだけ (RC-2 の対比) |
| rename migration (`src/app.rs:32658-32695`) | `Busy`/`Starting` で `return` して次フレーム再試行。**再駆動用の repaint 予約が無い** (`App::update` の `reasons[]` にも `delayed_poll_delay` にも入っていない) | 次の入力まで migration が止まり得る。P3 |
| auto-aspect cache actor (`src/auto_aspect_cache.rs:266-268`) | `mpsc::channel()` (無制限) なので `Busy` 自体が発生しない | 同型ではない |

---

### RC-6 [P3] `scripts/analyze_perf.py` に collection の集計が無い

**根拠**: `git show HEAD:scripts/analyze_perf.py | grep -n collection` は
`from collections import Counter, defaultdict` の 1 行のみ。サブコマンド一覧
(`:2957-2975` 付近) にも `collection` が無い。B-6 の修正方向が
「`scripts/analyze_perf.py` にサブコマンドを足せる形にしておく」だったので、
イベントは出るが**リリース手順 Phase 2 で読む手段が無い**。

**修正方向**: `cmd_collection` を足し、`open`→`actor_rtt`→`prepare(stage 別)`→`install` を
`collection_id` + `surface_generation` で、navigation を `request_sequence` で相関させて
p50/p95/最大と `outcome` 内訳を出す。plan §23.1 が書いている
「完了の無い `navigation_begin` を成功や滞留と断定しない」をそのまま実装の注記にできる。

---

### RC-7 [P3] テストの穴

- `terminal_read_error_is_not_reported_as_a_collection_boundary`
  (`collection_navigation.rs:3964-4000`) は `fs_boundary_hint.is_none()` と
  `fs_feedback_toast.is_some()` しか見ない。`OuterGrid` の**真の末尾**も
  `show_feedback_toast("コレクション内の次のコンテナはありません")` でトーストを出す
  (`:3187-3193`) ので、**この assert は真の末尾でも通る**。文言を区別していない。
  → RC-1 の英語文字列もこのテストでは検出できない。
- `Failed` (終端) のとき `collection_grid_poll_delay() == None` を固定するテストが無い
  (`:4518` は Busy 解消後の `None` のみ)。C-2 の「終端は再駆動しない」の要は
  ここなので、`Failed` / `Deleted` / fullscreen parked (`root_materialize_active == false`) の
  3 ケースで `None` を assert する状態遷移テストが欲しい。
- 時間依存: 新規 4 テストはいずれも `test_barrier` で actor を止めて `Busy` を決定的に作り、
  `not_before` を手で `Instant::now()` に巻き戻してから poll する形なので、
  **実時間 sleep に合否を預けていない**。`wait_for` / `poll_until`
  (`collections.rs:4465-4474`, `collection_grid.rs:2248-2257`) は 5 秒 deadline +
  2〜5ms sleep のポーリングで、既存パターンと同じ。不安定化の懸念は小さい。

### RC-8 [P3] stale / prepare 却下からの再要求に backoff が無い (実効 20Hz で律速されている)

**根拠**: `Snapshot` reply が `minimum_revision` / `wanted_revision` に届かないとき
(`collection_grid.rs:1552-1556`)、`prepare` 結果が `accepts` しないとき (`:1660-1666`)、
`Cancelled` のとき (`:1670-1677`) は `RequestNeeded { not_before: None }` = 即時。
`collection_grid_poll_delay` はこれを `Some(Duration::ZERO)` (`:1295-1297`) にする。
ただし `schedule_collection_grid_snapshot()` が `poll_collection_grid` の**同じ pass の末尾**
(`:1726`) で呼ばれるため、tail が見るころには `Snapshot` (= 50ms) に移っている。
**実効ループ速度は 50ms/回**で、フレームレートでは回らない。
残るのは「mutation が連続する間、全件 SELECT を 20Hz で投げ続ける」点で、
PDF プールのような epoch prune も無い (`docs/pdf-pool-context-epoch-plan.md` の対照)。

**修正方向**: 出荷ブロッカーではない。将来、stale 連続回数に応じた backoff か、
actor 側で「同じ collection の未処理 `LoadCollection` は最新 1 件に畳む」を検討する。

### RC-9 [P3, 観測] navigation commit が `--perf-log` 無効でもフルパスを通常ログに書く

`collection_navigation.rs:2792-2797` の `crate::logger::log(format!("[collection-nav] commit
collection={} revision={} target={} payload={}", .., ready.target.source_path.display(), ..))` は
`perf::is_enabled()` に関係なく毎回 `mimageviewer.log` へ出る。
`git log -S` で確認した限り `b48c46303` (計装コミットより前) の追加であり、今回の退行ではない。
計装側が「個々の source path を記録しない」方針で作られているのに、同じ行の隣で
無条件にパスを書いているので、方針の正本を 1 つに決めるなら見直し対象。
(通常ログにパスが出るのは他機能でも同じなので、単独では P3 未満。)

---

## 4. 確認して問題なし

- **actor の異常終了は終端へ昇格する**: `try_send` の `Disconnected` は
  `CollectionStoreError::Unavailable` (`runtime.rs:692, 714`)、
  in-flight は `TryRecvError::Disconnected` で Grid `Failed` (`collection_grid.rs:1592-1600`) /
  navigation `finish_collection_navigation_read_error` (`collection_navigation.rs:1777-1782`)。
  無限 retry には落ちない。
- **同一 slideshow / EOF 通知が期限を伸ばさない**:
  `enqueue_or_reuse_collection_automatic_navigation` (`collection_navigation.rs:1193-1213`) は
  `matches_automatic_intent` (`:313-320`、origin と action の完全一致) が真なら
  pending をそのまま戻して `return true` する。`not_before` にも `intent_sequence` にも触れない。
- **別 action / 別 serial は別意図**: `origin` は `intent_sequence` を含み、EOF 系 action は
  `seek_serial` を持つ (`:2216-2240`)。異なれば `advance_collection_navigation_sequence()` →
  `enqueue_collection_navigation` → `set_collection_navigation_pending(None)` で旧 pending を退役。
- **手動送りの連打は期限を伸ばさない**: `accumulate_manual` (`:394-446`) は `RequestNeeded` の
  `request.action` の `queued_steps` を積むだけ。`not_before` は保持される。
  逆方向キーは `signum` 不一致で false → 新規 enqueue になる。
- **待機中のフルスクリーン終了 / 別ファイル / コレクション削除**:
  `collection_navigation_request_is_current` (`:2192-2250`) が `fullscreen_idx` /
  `items_generation` / `surface_generation` / `context_id` / `intent_sequence` を見て false になり、
  `RequestNeeded` 分岐 (`:1731-1734`) が `finish_collection_navigation_without_target_no_context`
  を呼ぶ。`Slideshow` / `OuterFullscreen` はそこで `release_fs_nav_lock()` (`:3227-3231`)。
  pending を差し替える経路は `set_collection_navigation_pending`
  (`top_level_grid_view.rs:1284-1294`) が `owns_fs_navigation_lock()` を見て
  `collection_navigation_retired_fs_lock` を立て、次の `poll_collection_navigation` 冒頭
  (`collection_navigation.rs:1712-1720`) が `finish_fs_navigation_sequence(RequestFailed)` する。
  **lock の解放漏れ経路は見つからなかった。**
- **terminal read error を「真の末尾」として表示しない**:
  `finish_collection_navigation_read_error` (`:1069-1082`) は
  `..._no_context` (= boundary hint も末尾トーストも出さない) を通ってから
  読み込み失敗トーストを出し、`Slideshow` は `stop_slideshow_playback()`。
  `Manual` は `fs_boundary_hint` を立てない。**表示上の区別はできている** (文言は RC-1)。
- **native video 再生中でも読み込み失敗トーストは見える**: `show_feedback_toast` →
  `show_feedback_toast_with_duration` (`src/app.rs:66533-66540`) が Windows で
  `show_native_video_overlay_toast` へも送る。
- **`RequestNeeded { not_before: None }` は spin ではない**: RC-8 のとおり同 pass で消費される。
- **fullscreen leaf / 物理子フォルダで表示中は poll も schedule も止まる**:
  `collection_grid_root_materialize_active()` (`collection_grid.rs:1274-1280`) を
  `collection_grid_poll_delay` (`:1285`) と `schedule_collection_grid_snapshot` (`:1151`) と
  `poll_collection_grid` の早期 return (`:1432`) が**同じ述語で**見ている。
  片方だけ進む / 片方だけ起きる非対称は無い。
- **`install_collection_navigation_root` が `wanted_revision` を代入で下げる**
  (`collection_navigation.rs:2539`、再利用枝 `:2526` は `max`) が、
  `minimum_revision` は下限としてしか使われず (`collection_grid.rs:1166`)、
  `load_collection` は常に現在 revision を返すので、staleness にはならないと判断した。
- **管理 window を閉じた後に毎フレーム要求は出ない**: `request_catalog` / `request_snapshot` の
  production 呼び出しは Ready イベント・watch notice・`install_catalog` の stale 再要求・
  reorder 保存完了・`NotFound` の 5 契機のみ。`poll_collection_ui_tail` から毎フレーム走るのは
  `reconcile_collection_toolbar_target()` (`collections.rs:2154`) だけで、これは B-7 の既知指摘
  (今回の修正範囲外、未着手のまま)。
- **計装は request ownership / worker 配置 / deadline / 取消 / 表示順を変えていない**
  (§2 参照)。plan §23.1 の記述と実装は一致している。

---

## 5. 出荷判断の目安 (この担当範囲のみ)

- **出荷前に直したい**: RC-1 (英語文字列が利用者に出る、1 か所の可視化 + 4 行の置換で済む)。
- **出荷前に判断が要る**: RC-2。少なくとも
  ①リリース手順 9.7 の 4 シナリオを **コレクション root を開いた状態でも 1 回測る**か、
  ②Busy/Starting の再駆動に backoff + 上限を入れるか、どちらかを決めてから出す。
  「20Hz が無期限に続き得る」ことは idle health のゲート定義 (既定 `max_update_rate=10/s`) と
  直接ぶつかる。
- **v4.0.x 以降で良い**: RC-3〜RC-9。

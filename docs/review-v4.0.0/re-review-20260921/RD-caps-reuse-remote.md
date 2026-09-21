# 再レビュー RD: 登録上限 / import 資源上限 (B-1)・prepare 再利用 (B-4 / D-2)・Remote 媒体別位置 (D-4)

対象: コミット `2814fb139` / `1c0b470de` / `8dbad53e5`。行番号はすべて **HEAD (`a5a1fc43b`) 時点**。
作業ツリーの未コミット差分 (KeyAction 追加) は対象外で、触れていない。

**本レビューはソース読解のみ。** アプリは起動していないし、ビルド・テストも実行していない。
以下はすべて「コードを読んだ限りの推定」であり、実行時の観測はこちらに存在しない。

---

## 1. 元指摘ごとの判定

| ID | 判定 | 根拠 |
| --- | --- | --- |
| **B-1** (import モーダルの全行描画 / 上限なし) | **部分的** | 仮想化 (`ui_dialogs/collections.rs:3733` `show_rows`)、worker 実読込 32 MiB+1 (`:3262-3281`)、pure parser の 32 MiB / 非空 50,000 行拒否 (`collection_store/text.rs:7-8,64-66,81-84`)、長 path の `truncate()`+hover (`:3746-3750`) は実装済み。**修正方向 ② 「accepted / invalid を遷移時に 1 回数えて保持」だけ未実施** (`:3754-3759` が毎フレーム O(行数) を 2 本回す) → RD-1 |
| **仕様判断 3** (1 コレクション 10,000 件) | **解消** | 定数 1 箇所 `collection_store/model.rs:12`。actor transaction 内の `COUNT(*)` を正本に (`db.rs:316-321`)、重複判定の後に `remaining == 0` で typed 拒否 (`db.rs:334-337`)、`capacity_rejected` を outcome へ (`model.rs:301`)。`added` が空なら collection revision も catalog revision も進めず (`db.rs:357-364`)、actor の publish も抑止 (`runtime.rs:974-976`)。production の追加入口は `ui_dialogs/collections.rs:2731` の 1 本だけで、他の `add_batch` 呼び出しはすべて `#[cfg(test)]` 内 |
| **B-4 / T3** (PC のページ送りごと全件 prepare) | **解消** (承認済み仕様変更を含む) | `collection_navigation.rs:1508-1533` が `CollectionGridPrepareReuseKey` 一致時に prepare worker を起動せず preflight へ直行。キーは collection id / revision / `GridDisplayOrder` / sidecar 認識 fingerprint / `skip_image_if_video_exists` / `video_thumb_use_sidecar_image` / 動画 pin DB instance+成功書込世代 (`top_level_grid_view.rs:564-597`)。**残る O(N)**: actor `load_collection` は毎ステップ発行され全 entry 行を読む (`collection_navigation.rs:1009` → `db.rs:148-153`) → RD-5 |
| **D-2** (Remote のページ送りごと全件 prepare) | **解消** | `persistent_collections.rs:51-79` の 1 件 cache を `RemoteSessionIdentity` (client_id+session_id) を owner として保持。owner 不一致で epoch bump + entry 破棄 (`:238-242`)、候補は `entry.owner == *owner` 必須 (`:245-258`)。`exact_prepared` は **actor load で revision を確認してから** prepared を再利用 (`:1003-1020`)、facts は `Arc::ptr_eq(prepared)` のときだけ再利用 (`:390-405`)。`snapshot()` (明示更新) は常に `view_facts` を作り直し (`:531-537`)、`begin_snapshot_refresh` が epoch を進めて古い navigate の publish を棄却 (`:218-227`, `:262-303`) |
| **D-4** (deep link 着地と seek の射影ずれ) | **解消** | `position: {kind, ordinal, count}` を着地した実媒体の exact 公開列から算出 (`persistent_collections.rs:1819-1847`)。`remote_eligible` は prefix 打ち切りと無関係に全 entry を観測して作る (`:342-362`)。protocol は `crates/remote-ipc/src/lib.rs:31` の単一定数 58 で core / remote-web とも同じものを参照。Web は `persistentCollectionLandedPosition` で kind と範囲を検証し (`app.js:7369-7384`)、画像 seek は `still_image` のときだけ `sparsePosition` を使う (`app.js:5411-5421`)。HTTP の partner 昇格は checked 演算 + `< count` フィルタ (`crates/remote-web/src/http.rs:2909-2924`)。core 側は `image_group_retains_target` (`:1439-1445`) で対象消失時の partner 誤着地を拒否 |
| **D-5** (`image_count` / `target_count` が remote-web 二重検証前の値) | **未解消 (延期済み)・悪化なし** | `validate_persistent_snapshot_addresses` は今も `image_count` / `entry_limit` / `truncated` を書き換えない (`http.rs:2833-2866`)。`position.count` も core の `remote_eligible` 由来 (`persistent_collections.rs:1840-1846`) なので同じ性質。射影が媒体別になった分だけ「どの列の総数か」は明確になった |
| **T7** (大規模コレクションの前提が未定義) | **解消** | 10,000 件を定数化し、Remote root prefix も同じ定数から導出 (`persistent_collections.rs:144`)、catalog は `MAX_REMOTE_COLLECTION_CATALOG = 100_000` で別上限 (`:43`, `:265`)。`manual/collections.html:144,146,167` と `manual/known-issues.html:109` に記載済み |

---

## 2. 新規指摘

### RD-1 [P3] import 確認モーダルが accepted / invalid を毎フレーム数え直す (B-1 修正方向 ② の積み残し)

- 根拠: `src/ui_dialogs/collections.rs:3754-3759`。`preview.accepted_paths().count()` と
  `lines.iter().filter(...).count()` が `egui::Modal` の closure 内にあり、モーダルが開いている
  **すべてのフレーム**で `preview.lines` 全体を 2 周する。`show_rows` 化されたのは描画だけ。
- 失敗シナリオ (推定): 上限いっぱいの 50,000 行を読み込ませると、1 フレームあたり 10 万回の
  `CollectionImportLineStatus` 比較が UI スレッドに載る。比較自体は判別子の一致だけなので
  致命的ではないが、B-1 の 修正方向 ② が明示的に挙げた項目がそのまま残っている。
- 修正方向: `CollectionDialogOperation::PreviewImport` へ遷移する時点 (`:2198`) で
  `accepted` / `invalid` を数えて構造体に持たせ、closure では読むだけにする。

### RD-2 [P3] 全件が容量拒否でも「成功」として通知される

- 根拠: `src/ui_dialogs/collections.rs:2848-2851`。`report_collection_add_result(&origin, !errors.is_empty(), message)`
  の error フラグが **path 準備エラー `errors` だけ**から導かれ、`outcome.capacity_rejected` を見ていない。
- 失敗シナリオ (推定): 上限到達済みコレクションへ 5 件追加すると
  「追加 0 件、重複 0 件、上限による拒否 5 件。1コレクションの登録上限は10,000件です。」が
  **成功扱いの通知**として出る。1 件も入っていないのに成功の見た目になる。
- 修正方向: `added.is_empty() && !capacity_rejected.is_empty()` を error (または警告) 扱いにする。

### RD-3 [P3] 上限値 10,000 が UI 文言にリテラルで重複している

- 根拠: `src/ui_dialogs/collections.rs:2846` の `" 1コレクションの登録上限は10,000件です。"` が
  `MAX_COLLECTION_ENTRIES` (`collection_store/model.rs:12`) から導出されていない。
  `manual/collections.html` / `known-issues.html` にも数値が直書きされている (文書側は妥当)。
- 失敗シナリオ: 将来定数を変えたときに UI 文言だけ古い数字が残り、利用者に矛盾した説明を出す。
  テストも文言を検査していないので静かにずれる。
- 修正方向: `format!("… {MAX_COLLECTION_ENTRIES} 件 …")` 相当にする (桁区切りが要るなら helper)。

### RD-4 [P3] 容量判定が actor まで来ないので、分類 worker が拒否される分まで全件 stat する

- 根拠: import は preview → 分類 worker (対象 path の存在・種別判定) → actor `add_batch` の順。
  容量判定は `db.rs:334` の actor 側だけにある。UI は snapshot (`collection_ui.snapshot`) から
  現在件数を知れるのに、確認ボタンの文言は `accepted`件そのまま (`:3763`)。
- 失敗シナリオ (推定): 空でないコレクションへ 50,000 行を import すると、分類 worker が
  50,000 パスを stat し終えてから「上限による拒否 40,000 件」と出る。待ち時間の大半が捨てられる。
  (資源上限と登録上限が別なのは仕様どおりで、文書にも書かれている。問題は順序。)
- 修正方向: 確認画面で残容量を出し、「このうち N 件まで追加されます」を先に見せる。
  actor の `COUNT(*)` を正本に保つ設計は変えなくてよい (UI 側は表示のみ)。

### RD-5 [P2] ページ送りごとの actor 全件 SELECT が残り、単一 actor を Remote と共有し続ける

- 根拠: `src/app/collection_navigation.rs:1009` が next/prev/EOF ごとに `client.load_collection()` を
  発行し、actor は `db.rs:148-153` → `load_snapshot` で **全 entry 行**を読んで `CollectionSnapshot` を
  組み立てる。`spawn_collection_navigation_prepare` (`:1508`) はその後の filesystem 分類だけを省く。
  Remote も同じ actor を使う (`remote_ipc/pipe.rs:763`, `collection_store/runtime.rs:231-233`)。
- 位置づけ: B-4 の修正方向 ① は「actor load すら不要のファストパス」を挙げていたが、
  実装は revision 確認のために actor load を残した。**この判断自体は正しい** (revision を確認せずに
  再利用すると mIV 内編集の追従が壊れる)。ただし T3 の「1 回の矢印キーが O(entry 数)」は
  filesystem から DB へ場所を変えて残っている。
- 失敗シナリオ (推定): 10,000 件のコレクションでスライドショー / 連続再生を回すと、1 ステップごとに
  10,000 行の SELECT + 行→構造体変換が単一 actor を占有する。その間 Remote の
  `list_catalog` / `load_collection` と PC の `schedule_collection_grid_snapshot` が後ろに並ぶ
  (`COMMAND_CAPACITY` 超過時は `Busy` → B-21 の経路)。
- 修正方向: actor に「revision だけ返す」軽量コマンド (`LoadRevision`) を足し、再利用条件が成立する
  ときはそれで確認する。全 entry が要るのは再利用不能なときだけ。
  少なくとも perf イベント (`collection` / `actor_rtt`) で entries と ms が出るので、規模別の実測は取れる。

### RD-6 [P3] pin stamp が process 全域なので、無関係な動画のピンでコレクション全件 prepare が走る

- 根拠: `src/video_pins.rs:41-45` の `VideoPinMutationStamp { instance, revision }` は DB ハンドル単位。
  `set_pin` / `remove` の成功でグローバルに 1 つ進む (`:333`, `:344`)。
  `collection_grid_prepare_reuse_key` (`collection_grid.rs:1732-1743`) はこれをそのまま鍵に入れる。
- 失敗シナリオ (推定): コレクションに 1 本も含まれない動画へ P でピンを打つと、次のページ送りで
  10,000 件の完全 prepare (stat + sidecar 走査 + pin DB open) が再実行される。正しさは保たれるが、
  再利用が効かない時間帯が生まれる。
- 補足 (悪い方ではない): 逆向きの取りこぼしもある。`metadata_import_refresh.rs:177` のように
  **別ハンドル**経由で `video_pins.db` が書き換わる経路では `self.video_pin_db` の revision が
  進まないので、pin 変更を再利用キーが観測しない。メタデータ取り込みとコレクション root の
  同時利用という狭い条件。
- 修正方向: stamp を「対象 path 集合に触れたか」まで絞るか、少なくとも別ハンドル書込の扱いを
  実装計画へ明記する。

### RD-7 [P3] 再利用キー不一致時の再駆動に回数上限・backoff が無い

- 根拠: `src/app/collection_grid.rs:1659-1665` — `accepts` が false のとき
  `CollectionGridLoadState::RequestNeeded { installed, not_before: None }` に落とすので、次フレームに
  即再要求 + prepare worker 再 spawn。`src/app/collection_navigation.rs:1839` の
  `Ok(Ok(_)) => self.restart_collection_navigation(...)` も同様に無条件再送。
- 失敗シナリオ (推定): 鍵の構成要素が prepare 実行中に毎回変わる状況 (例: pin 書込が連続する、
  `image_page_recognition_fingerprint` が参照する susie pool が揺れる) では、prepare → 不一致 →
  即再要求のループになり、worker spawn が frame 周期で続く。実際の駆動源は利用者操作なので
  通常は 1〜2 回で収束すると読めるが、**収束しない構造ではない**。2026-07-10 の動画ピン
  自己ループ (`app/native_video.rs:8014-8026` のコメント) と同型の形をしている。
- 修正方向: 不一致由来の再要求に短い `not_before` (または再試行回数) を入れ、
  perf イベントに `reason="reuse_key_mismatch"` を出して観測可能にする。

### RD-8 [P3] 64 MiB の保持予算は presentation 単位で、viewer context ごとに合算され得る

- 根拠: `src/app/top_level_grid_view.rs:553` `MAX_RETAINED_COLLECTION_PIN_BLOB_BYTES` と
  `:555-562` の判定は 1 つの `CollectionGridInstalledPresentation` の pin blob 合計だけを見る。
  `top_level_grid_view` は viewer context ごとに swap される field
  (`src/app/viewer_context_registry.rs:2107`) なので、別窓 / 複数ウィンドウで別コレクションを
  開くと最悪 N × 64 MiB になる。
- 補足: 同一 collection を共有する context 間は `Arc<CollectionGridPreparedThumbnailSources>` を
  共有するので二重には持たない (`collection_grid.rs:1756-1767`, `:950-957`)。
  pin blob は WebP サムネなので 64 MiB に届くには 1,000 本規模のピンが要る。実害の確率は低い。
- 修正方向: process 全体の予算にするか、少なくとも「per presentation」であることを
  実装計画 §23.7 の記述 (「長期保持する pin BLOB の合計が 64 MiB」) に合わせて明記する。
  現在の文面は process 全体の予算と読める。

### RD-9 [P3] 再利用キーの要素別テストと Oversized 経路の統合テストが無い

- 根拠:
  - PC: `collection_navigation.rs:3391` `navigation_reuses_exact_presentation_without_full_prepare_and_pin_mutation_invalidates_it`
    は **pin stamp の不一致だけ**を固定する。`grid_display_order` / `skip_image_if_video_exists` /
    `video_thumb_use_sidecar_image` / `sidecar_scan_fingerprint` の各要素で再利用が落ちることを
    確かめるテストは見当たらない。
  - `collection_navigation.rs:3671` `pin_blob_retention_budget_is_bounded_without_truncating_installed_payload`
    は純関数 `collection_pin_blob_sizes_fit_retention_budget` だけを検査する。
    「予算超過 → `Oversized` → 次の navigation が完全 prepare に戻る」という**経路**は固定されていない。
  - 上限境界は `collection_store/tests.rs:152` が 9,999 既存 +1 追加 +1 容量拒否、
    および 10,001 件 (legacy) からの拒否・削除・手動並替を押さえており十分。
    ただし「ちょうど 10,000 件の状態で 1 件追加 → `added` 0 かつ revision 不変」は
    `:220-233` の `unchanged` ブロックが重複経由で見ているだけで、容量経由では `:253-265` が該当。
- 修正方向: 鍵の要素ごとに 1 行ずつ negative テストを足す (`app.settings.* = ...` して
  `spawn_collection_navigation_prepare` が `Preparing` になることを見る形で、既存テストの型に乗る)。

### RD-10 [P3] 保持する側で `CollectionGridPreparedThumbnailSources` を UI スレッドで clone する

- 根拠: `src/app/collection_grid.rs:1756-1767`。予算内なら
  `CollectionGridPresentationSources::Retained(Arc::new(thumbnail_sources.clone()))`。
  `video_pin_blobs` は `Arc` なので安いが、`video_sidecars: HashMap<String, PathBuf>` は実体 clone。
- 失敗シナリオ (推定): 動画を多く含む 10,000 件コレクションの install フレームで、
  sidecar 件数ぶんの `String` + `PathBuf` 確保が 1 回増える。B-2 (revision 前進ごとの再 install) が
  未修正なので、編集のたびに発生する。絶対値は小さい。
- 修正方向: `prepare` worker 側で最初から `Arc<CollectionGridPreparedThumbnailSources>` を作り、
  install は `Arc::clone` だけにする。

---

## 3. 確認して問題なしと判断した点

- **上限の正本**: `MAX_COLLECTION_ENTRIES` は `model.rs:12` の 1 箇所。Remote の root prefix
  (`persistent_collections.rs:144`) も同じ定数。catalog 100,000 は別名 `MAX_REMOTE_COLLECTION_CATALOG`
  (`:43`) に分離済みで、混線していない。
- **追加 0 件で revision / 通知が進まない**: `db.rs:357-364` (catalog / collection revision とも据え置き)、
  `runtime.rs:974-976` (`mutated` が false なので `publish_revision` しない)。
- **transaction の原子性**: `add_batch_sql_failure_rolls_back_earlier_insert_and_revision`
  (`collection_store/tests.rs:267`) が trigger で途中 INSERT を失敗させ、entries / revision /
  catalog_revision がすべて巻き戻ることを固定している。
- **既存の上限超過データ**: 読取・`remove_entries`・`reorder_manual` はいずれも容量を見ないので
  そのまま動く (`db.rs:375`, `:406`)。テスト `:239-266` が 10,001 件状態で確認している。
- **Remote の full eligible / seek / total の一貫性**: `image_count` は `facts.remote_eligible` 全件から
  数え (`persistent_collections.rs:1512-1516`)、`entry_limit` / `truncated` だけが prefix を表す。
  `BoundedWirePrefix` は先頭から順に push し、予算超過で `accepting=false` にするので**真の prefix**
  であり、`wire_snapshot` の `entries.get(index)` による prepared index の読み替えが破綻しない
  (`:142-153`, `:1455-1470`)。`complete_display_unit_prefix` (`:1243-1264`) が見開き単位を途中で切らない。
- **Remote cache の owner 分離**: `navigation_cache_candidate` が owner 不一致で epoch bump + entry 破棄、
  候補 filter も `entry.owner == *owner` 必須 (`:238-258`)。`publish_view` は epoch・owner・
  「同一 collection のより新しい revision を上書きしない」の 3 条件 (`:65-79`, `:275-286`)。
  テスト `view_cache_is_one_session_exact_key_and_explicit_refresh_retires_old_work` (`:2670`) が
  別 session_id・設定差・spread 差・明示更新での棄却を固定している。
- **セッション / PIN / 公開範囲の変更**: engine と cache は `ServerGuard::start`
  (`remote_ipc/pipe.rs:713`) 内で作られるため、PIN 変更・serve 設定変更・全端末ログアウトで
  child / server が作り直されると cache も消える。加えて **remote-web が毎応答で再検証する**
  (`http.rs:2775`, `:2817-2822` → `validate_remote_file_kind`) ので、core の cache が古くても
  公開範囲外の住所は端末へ出ない。cache 再利用は認可の境界を動かしていない。
- **実 target の再検証は省いていない**: 画像は `image_display_unit_for_target` が slot ごとに
  `inspect_collection_source` + `path_guard::resolve_existing` をやり直し (`:1371-1421`)、
  動画・音声も候補ごとに同じ 2 つを通す (`:745-770`)。落ちた候補は `continue` で次へ進み、
  候補列は `resolve_prepared_collection_navigation` が作る有限 Vec なので無限ループにならない。
  全滅時は `Boundary { reason }` (`:850-875`)。
- **PC 側 preflight の終端**: `preflight_candidates` (`collection_navigation.rs:550-692`) は候補を
  順に実 I/O で確認し、`required_matches` 到達で即 return、尽きたら `Ok(None)` →
  `finish_collection_navigation_without_target` (`:1637`)。固着経路は見つからなかった。
- **mIV 内の rename / move / 本のページ転送**: `migrate_source_batch` が実際に置換した collection の
  revision を進める (`db.rs:572-578`) ので、再利用キーが変わり完全 prepare に戻る。
- **mIV 内の削除 (ごみ箱)**: `invalidate_collection_grid_sources` → `invalidate_current_collection_grid_sources`
  (`collection_grid.rs:500-540`) が該当 entry を含む場合に `session.cancel_pending()` を呼び、
  `load` を `RequestNeeded` へ落とす (`top_level_grid_view.rs:756-785`)。
  `installed_presentation()` は Ready / Empty でしか返らない (`:711-718`) ので、
  **削除後は navigation の再利用が確実に無効化される**。全 viewer context に配る形になっている (`:529-539`)。
- **明示更新 (F5 / 「最新の情報に更新」)**: `KeyAction::GridReload` → `reload_top_level_grid`
  (`top_level_grid_view.rs:1025-1027`) → `open_collection_grid` → `begin()` で session を作り直し、
  `poll_collection_grid` の `Preparing` 経路には再利用のファストパスが無い
  (`collection_grid.rs:1227-1250`) ので、**全件 prepare をやり直す**。
- **Remote の明示更新**: `snapshot()` は cache を読まず、`begin_snapshot_refresh` で epoch を進めてから
  常に `view_facts` を再構築する (`persistent_collections.rs:526-537`)。
- **D-4 の版数整合**: `crates/remote-ipc/src/lib.rs:31` の 1 定数を core (`remote_ipc`) と
  remote-web (`ipc_client.rs:1825`) が共有。Web は HTTP なので版定数を持たない。
  `target_ordinal` / `target_count` は payload から除去され、テストが不在を assert している
  (`lib.rs:4430-4470`)。
- **D-4 の ordinal 補正**: HTTP の partner 昇格は `checked_add` / `checked_sub` + `< count` で
  範囲外を `BadRequest` にする (`http.rs:2915-2922`)。RTL では anchor が group 内の後ろ側で
  射影 ordinal が必ず 1 以上になるため、`checked_sub(1)` が None になる組み合わせは作れないと読める。
- **Web 側**: `persistentCollectionLandedPosition` が target.kind と position.kind の一致・整数・
  範囲をすべて検査し、不一致なら navigation 全体を失敗させる (`app.js:7369-7384`, `:7495-7499`)。
  画像 seek は `still_image` のときだけ (`:5411-5421`)。動画・音声は seek バーではなく
  タイトルの `n / total` に使われる (`:5089`, `:7544`) ので、B-4 以前の「射影が切り替わって数字が飛ぶ」
  は解消している。`viewerSeekSnapshot` は画像ビューアからしか呼ばれない (`:5469`, `:7783`, `:8155`)。
- **`VideoPinDb` の instance**: `App` 構築時に 1 回だけ開く (`app.rs:15923`, `:16671`) ので
  instance が走行中に変わらず、RD-7 の暴走条件にはなりにくい。
  `mutation_stamp_is_db_instance_scoped_and_advances_only_after_success` (`video_pins.rs:369`) が
  失敗時に進まないことも固定している。
- **`without_thumbnail_sources` が `Settings::default()` から鍵を作る点** (`top_level_grid_view.rs:677-694):
  `#[cfg(test)]` 付きで production 経路には無い。
- **計装**: `collection` カテゴリの perf イベントが open / prepare / preflight / remote_prepare /
  actor_rtt に入っている (`collection_grid.rs:1111`, `collection_navigation.rs:694`,
  `persistent_collections.rs:1039-1063`)。B-6 / T4 の指摘は本差分の範囲では解消側に動いている。
- **文書**: `manual/collections.html:144,146,167` が 32 MiB / 50,000 行 / 10,000 件と
  「資源上限と登録上限は別」を説明し、`:130` と `known-issues.html:107-113` が
  「外部変更の自動反映は保証しない。明示更新か開き直しで再評価」を明記している。
  内部用語 (reuse key / prepare / actor / stat) は利用者向け文書にも UI 文言にも出ていない。

---

## 4. 出荷可否の見立て

データ破壊・公開範囲逸脱・固着に当たる新規 P1 は見つからなかった。
**P2 は RD-5 の 1 件**で、これは B-4 / T3 の残余 (ページ送りごとの actor 全件 SELECT が
Remote と同じ単一 actor を占有する) であり、承認済みの仕様変更で filesystem 側は解消済み。
出荷を止める性質ではないが、**10,000 件前提の実測が無いまま「解消」と記録しない**ほうがよい。
残りはすべて P3。

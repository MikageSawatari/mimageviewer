# 長さ以外の並べ替えも worker で走査する (§1.849)

- 状態: master 取り込み後の修正中。§8.4 の部分1を実装、部分2は追加経路の扱いを設計担当へ確認中。v5.6.0 で直す (利用者決定、統括経由)。
- 利用者の言葉: 「長さの件、v5.6.0 で直すことにしたいと思います。修正お願いします。」
  - その前に、統括が変わる動きを説明している。小さいフォルダでも数フレームは前の並びが残り、そのあいだのキーは止まる。
- 枝: `sort-worker-849` (master d190b67cc から)。
- 関連: §1.838 (`docs/sort-change-input-hold-838-plan.md`、並べ替えの待ちの間の入力の止め方)、§1.762 (長さ順)。

## 1. 観測と原因

- 観測 (サブ PC 担当、master 0e1b617b0、`C:\home\mimageviewer-subpc\range835\results\notes.md`):
  - **NG-1**: `\\tsclient` 越しの 3004 件のフォルダで「長さ↑」から「名前↑」に戻すと、画面が約 7 秒止まる。「並べ替え中…」は出ない。ログは `[SLOW FRAME] 7048.5ms ... pre_grid=7043.4ms`、panic.log は `UI THREAD HANG suspected`。
  - **気-1**: 「長さ」から「名前」へ戻すと、選択が同じ番号の別の項目へ移る。「名前」から「長さ」では同じ項目のまま。
- 原因 (コードから):
  - `apply_sort_change_reload` は長さ順だけ `PhysicalFolderSortReload::WorkerScan` を選ぶ。それ以外は `Immediate` を選び、UI スレッドで `scan_directory_with_settings` を実行する。`load_folder_or_convert_archive_with_auto_fullscreen_owned_at_stage` が `DirectoryScanRoute::UiThread` で、pre_scan を持たずに呼ぶ。待ちを作らないので、§1.838 のゲートも「並べ替え中…」も効かない。
  - worker の経路は、採用時の `apply_current_view_order_refresh` で `preserve_cursor_hint_for_reload()` を呼び、選択中の項目名で選び直す。Immediate の経路はこれを呼ばない。
  - 番号が残る仕組み (設計レビュー r1 で確認): `folder_history` の控えを外した後、採用処理が旧一覧の選択番号を履歴へ保存し直す。名前のヒントが無い Immediate は、その番号を復元する。worker は差し替え前に名前を控えており、名前の照合が番号の復元より優先される。
  - どちらも v5.4.0 から同じ (v5.5.0 も同じ)。§1.835・§1.838 の変更では変わっていない。

## 2. 変更

1. `apply_sort_change_reload` の物理フォルダの分岐を、常に worker の走査 (`CurrentViewOrderRefresh`) にする。`PhysicalFolderSortReload` (Immediate / WorkerScan) の区別をなくし、`apply_sort_change_reload_without_ui_io` は `apply_sort_change_reload` と同じになるので一本にまとめる。
2. 呼び出し側の長さ順だけの分岐をなくす。
   - `src/ui_main.rs` の `apply_sort_control_change`。
   - `src/app/gamepad_input.rs` のピッカーの並べ替え。
3. 同じ関数を呼ぶほかの入口も、同じ経路になる。
   - 環境設定の「表示順」「評価順での未評価の位置」の変更 (`src/ui_dialogs/preferences.rs`)。
   - 一括変換の後の読み直し (`src/ui_dialogs/batch_convert.rs`)。
   - Immediate を残すと経路が 2 本のままになるため、こちらも寄せる。どちらも、待ちの間は §1.838 のゲートが掛かる。
4. **物理ディレクトリの一覧でないときは走査しない** (設計レビュー r1 の P2)。前の分岐で返らなかった一覧が、物理ディレクトリとは限らない。振り分けは拡張子ではなく、既存の一覧の種別と列挙の持ち主で行う (`.zip` などの名前を持つ実ディレクトリは、通常どおり worker で走査する)。
   - **ZIP などの列挙待ち** (`load_zip_as_folder` が `zip_nav` を外して `current_folder` に ZIP ファイルを置いた後、列挙の完了前): 走査を始めず、何もしない。列挙の完了が一覧を作る。完了時の一覧が、今の設定 (表示順など) で作られることを実装で確かめる。作られないなら、実装を止めて報告する。
   - **PDF / EPUB のページ一覧**: 既存の早期 return を保つ。
   - **合成の一覧** (物理ディレクトリを読み直せない一覧: お気に入り検索の結果・閲覧履歴・Snapshot・ドライブ一覧・タグの結果など): 何もしない。今は Immediate が合成パスを読もうとする (ドライブ一覧の `drive_list_synthetic_path()` など)。これは以前からの問題で、何もしない形にすると直る。変わる動きとして §3 に書く。
   - **合成の一覧から入った物理の子フォルダ** (コレクション内の `PhysicalSource`、Everything の結果から入った物理フォルダ、タグ・レーティング・スマートフォルダから入った子フォルダなど): 親の surface を保ったまま物理フォルダを表示している。これは物理ディレクトリの一覧として worker で走査する (設計レビュー r2 の P2)。タグから入った子フォルダは並べ替えの操作が固定 (`FixedOrder`) でも、一括変換の後の内容の読み直しは要る。「並べ替えの操作が無効」と「内容を読み直せない」を同じ条件にしない。
   - 判定は 1 か所の述語にまとめる。土台は既存の `current_grid_reload_is_physical()` (`src/app/top_level_grid_view.rs`。合成の surface が物理の子を持つかを、位置と一覧の作り手から決める)。これだけではディレクトリか書庫かは決まらないので、列挙の待ちが無いこと・ページ一覧でないことを足す。どれも既存の状態から導出し、新しい状態は持たない。`grid_sort_lock_reason()` は使わない。
5. 気-1 は 1 で直る。採用時の `preserve_cursor_hint_for_reload()` が、名前で選び直す。新しい処理は足さない。
6. 変えないもの:
   - zip_nav・検索・タグ・レーティング・ブックマーク・スマートフォルダ・サブフォルダ展開の分岐 (物理フォルダの分岐より前で返る)。
   - §1.838 のゲートと待機表示。
   - ほかの 11 か所の `DirectoryScanRoute::UiThread` (§5 で別項目として報告する)。
   - detached の述語・viewport の経路。

## 3. 変わる動き (利用者が了承済み)

- 物理フォルダで長さ以外へ並べ替えると、worker の走査が終わるまで前の並びが出たままになる。手元の小さいフォルダでは数フレームの見込み。
- そのあいだ、一覧の項目へのキー・クリックは §1.838 のゲートで止まる。項目に依らない操作 (フォルダツリー・戻る/進む・環境設定など) は止まらない。
- 500ms を超えると「並べ替え中…」が出る。ネットワーク越しや大量のファイルのフォルダでは、画面が止まらずにこの表示になる。
- 並べ替えの後も、選んでいた項目が選ばれたままになる (名前で選び直す)。これまでは長さ以外で番号の位置が残っていた。
- 環境設定の表示順の変更と、一括変換の後の読み直しも、同じく worker 経由になる。
- 合成の一覧 (お気に入り検索の結果・閲覧履歴・Snapshot・ドライブ一覧など) を表示中に環境設定の表示順を変えても、読み直さない。これまでは Immediate が合成パスなどを読み込みに行っていた (コードから。その結果の見え方は未確認)。合成の一覧から入った物理の子フォルダは、普通のフォルダと同じく worker で読み直す。

## 4. 状態の組み合わせを減らす検討

- **採用**: 経路を 1 本にする。Immediate をなくし、並べ替えの読み直しは `CurrentViewOrderRefresh` の 1 つの待ちだけにする。待ちの状態・入力のゲート・待機表示・選択の選び直しは、どれも既存のものを使い、新しい bool・pending・時刻を持たない。
- **採らない**: フォルダの種類 (ネットワークかどうか・件数) で Immediate と worker を分ける案。実行時の状態で挙動が変わり、試験の組み合わせが増える。決定性を優先する方針にも反する。
- **採らない**: 並べ替えの間をモーダルにする案。§1.838 のゲートが項目への入力だけを止め、ツリーでの移動などは通している。モーダルにすると、遅いフォルダで別の場所へ移る手段がなくなる。
- **採らない**: 環境設定・一括変換の入口だけ Immediate を残す案。経路が 2 本のままになる。どちらの入口にも、同期で終わることを前提にした後続の処理は見つかっていない (実装時にコードで確かめる、§6)。
- まれな失敗 (走査の失敗・フォルダが消えた等) は、既定の割り切り方針による。既存の worker 経路の失敗処理 (警告を出して待ちを終える) をそのまま使い、新しい回復処理は足さない。

## 5. 範囲の外 (別項目として報告する)

- `DirectoryScanRoute::UiThread` は、製品コードにほかに 11 か所ある。ネットワーク越しや大量のファイルのフォルダで、同じように止まり得る。
  - 例: 起動時に開くフォルダ。サブ PC のログでは、many で約 7.6 秒止まっている。
  - 今回は触らない。一覧 (入口ごとの関数名と、どの操作から来るか) は実装の報告で作り、統括へ渡す。

## 6. 確かめたこと (設計レビュー r1 の回答、コードから) と、実装で確かめること

- `current_folder` の種類:
  - 表示済みの ZIP・直接読む RAR / CBR・変換キャッシュの ZIP: 通常は `zip_nav` があり、前の分岐で返る。今のまま。
  - PDF / EPUB のページ一覧: `current_folder` はファイルになり得る。既存の早期 return を保つ。
  - ZIP などの列挙待ち: §2 の 4 のとおり、走査を始めない。
  - 合成の一覧 (お気に入り検索の結果・閲覧履歴・Snapshot・ドライブ一覧など): 環境設定から届き得る。§2 の 4 のとおり何もしない。合成の一覧から入った物理の子フォルダは worker で走査する (r2)。
  - `.epub`・`.rar` などの名前を持つ実ディレクトリ: worker で通常どおり走査する。拡張子で一律に外さない。
- 4 つの入口の後に、新しい一覧が同期でできていることを前提にした処理は見当たらない。ゲームパッドの reveal は、採用時の名前での選び直しが再び要求する。
- 「評価順での未評価の位置」は、採用時の今の設定から並べ替えを作る。`CurrentViewOrderSnapshot` に足す必要はない。
- 続けての並べ替えは、既存の pending の取り消しと置き換えで扱われる。採用の前に、navigation・path・設定・reload owner を照合する。
- pending は context とともに移り、park / retire で取り消される。新しい context の状態や detached の述語は要らない。
- `folder_history` の控えを、worker の失敗時に戻さないのは、長さ順と同じ扱い (まれな失敗の割り切り)。
- ZIP などの列挙の完了時の一覧は、今の `grid_display_order` で作られる (r2 で確認、通常・本の指定階層の両方)。列挙待ちで走査しない方針でよい。回帰のテストは残す。

## 7. テスト

- 足す:
  - 長さ以外の並べ替えで、`CurrentViewOrderRefresh` の待ちができ、その間は項目への入力が止まること (§1.838 の既存テストの形に合わせる)。
  - 長さ以外の並べ替えでも、採用の後に選んでいた項目が名前で選ばれていること (気-1)。長さ→名前・名前→日付の両方。
  - 環境設定の表示順の変更と、ゲームパッドのピッカーでも、同じ待ちになること。
  - 一括変換の後: 待ちができるだけでなく、完了後の一覧に生成した ZIP が入り、同名の元の項目が除かれていること。
  - ZIP などの列挙待ちと、合成の一覧 (少なくともドライブ一覧) で、環境設定の表示順を変えても走査を始めないこと。
  - 合成の一覧から入った物理の子フォルダ (少なくともコレクションの `PhysicalSource` と Everything の物理の子) で、長さ以外の並べ替えが worker で効き、選択が名前で保たれること。タグから入った子フォルダで、一括変換の後の読み直しが走ること。
  - 既存の `src/app/tests/sort_hold_838/lifecycle.rs` の形を使い、長さ以外の順でも、名前→日付→名前の連続・移動による置き換え・兄弟 context の不変を確かめる。新しい待ちの仕組みは作らない。
- 既存テストの書き換え: 並べ替えの直後に、新しい並びが同期でできていることを前提にしたテストがある。これは挙動の変化 (§3) なので、worker の完了を待ってから確かめる形にする。assert は弱めない。書き換えたテストと理由は §8 に記録する。

## 8. 記録

### 8.1 設計レビュー (Sol / xhigh、2026-11-22)

- r1 (93f276a61): REVISE / STRUCTURAL: AGREE。P2: ZIP の列挙中や合成の一覧でも物理フォルダの分岐に届く。P3: 一括変換の内容と、長さ以外の lifecycle の検証が足りない。→ c5d719926 で反映。
- r2 (c5d719926): REVISE / STRUCTURAL: AGREE。P2: `surface == Folder` だけでは合成の一覧から入った物理の子を外してしまう。P2: `FixedOrder` で一律に何もしないと、タグから入った子フォルダの一括変換後の読み直しも止まる。→ dde3cd92c で反映 (判定の土台を `current_grid_reload_is_physical()` にした)。
- r3 (dde3cd92c): ACCEPT / STRUCTURAL: AGREE。指摘なし。レビュー担当の言葉: 「既存 worker への統一は構造的修正です。」

### 8.2 実装 (2026-11-22、sort-worker-849 / 2a38c942f 上の未コミット差分)

#### 変更ファイルと設計の対応

| ファイル | 変更 | 設計 |
| --- | --- | --- |
| `src/app.rs` | `PhysicalFolderSortReload` と Immediate を撤去し、`apply_sort_change_reload` へ統一。物理ディレクトリは常に `CurrentViewOrderRefresh`。可視性変更と favorite overlay の旧 API 呼び出しも改名 | §2.1、§2.4、§2.5 |
| `src/app/top_level_grid_view.rs` | `current_sort_reload_is_physical_directory()` を追加。既存 `current_grid_reload_is_physical()` に列挙待ち・ZIP owner・文書ページの除外を集約 | §2.4 |
| `src/ui_main.rs` | `apply_sort_control_change` の長さ分岐を撤去。`reload_from_sort_control` の呼び先を改名 (分岐を残す理由は後述) | §2.2 |
| `src/app/gamepad_input.rs` | `apply_grid_picker_sort_order` の長さ分岐を撤去 | §2.2 |
| `src/ui_dialogs/epub_batch_convert.rs` | 旧 `without_ui_io` API の呼び出しを統一 API に改名 | §2.1 |
| `src/ui_dialogs/batch_convert.rs` | 読み直さない合成一覧についての既存コメントを現実装に合わせる。完了処理自体は既存の共通呼び出しを維持 | §2.3、§2.4 |
| `src/app/tests.rs` | 旧 mode 引数を外し、評価一覧の物理子・リング所有窓の同期完了前提を worker 採用後の検査へ変更 | §3、§7 |
| `src/app/tests/sort_hold_838.rs` | 既存 `request_sort` helper を全ソートで使える形にし、`worker_849` module を登録 | §7 |
| `src/app/tests/sort_hold_838/worker_849.rs` | 共通 worker、入力停止、選択、環境設定、変換、合成一覧、物理子の回帰を追加 | §7 |
| `src/app/tests/sort_hold_838/gamepad.rs` | 実際のパッド dispatch / ピッカーによる長さ以外の並べ替えを追加 | §7 |
| `src/app/tests/sort_hold_838/lifecycle.rs` | 連続置換・移動置換・兄弟 context の回帰を追加。既存の移動・兄弟入力テストの本体を order 引数付き helper に共用 | §7 |
| `src/app/folder_pane_reclick_tests.rs` | 旧 API 呼び出しの改名だけ | §2.1 |
| `src/app/tag_view_open_supersession_tests.rs` | 旧 API 呼び出しの改名だけ | §2.1 |
| `src/app/tests/search_request.rs` | 旧 API 呼び出しの改名だけ | §2.1 |
| `src/app/tests/section1795.rs` | 旧 API 呼び出しの改名だけ。既存の worker 完了待ちと assert を維持 | §2.1 |
| `docs/sort-worker-849-plan.md` | 本記録 | §8 |

- 新しい待機状態・入力ゲート・選択復元処理は追加していない。既存 `preserve_cursor_hint_for_reload()` を採用時に使う。
- `grid_sort_lock_reason()` は走査判定に使っていない。タグの物理子の FixedOrder は、変換後の内容更新を妨げない。
- 初回実装では `.epub` / `.pdf` 名の実ディレクトリを誤分類する既存 recipe を `folder_observation` の有無で補っていた。この方法は実装レビュー r1 の P2 で不十分と判明し、下記の r1 対応で撤去した。非空の実ページ列の `PdfPage` 行による除外は維持。
- `zip_nav`・検索・タグ・評価・ブックマーク・Smart・サブフォルダ展開の先行分岐、§1.838 の製品ゲート/表示、detached の述語/viewport は変更していない。

#### `reload_from_sort_control` の分岐を残す理由

この関数はソートの選択変更だけでなく、同じ順の再選択や「最新の情報に更新」も扱う。
長さ順で capabilities が許す場合は、取得済みの長さを使って再ソートする共通 API へ送る。
それ以外の `reload_top_level_grid(ctx)` は、検索の再検索・履歴やコレクション等の更新を各 owner へ送り、
物理一覧でも `current_list_refresh_intent()` の RefreshList と既存 staged transition を使う。
これを `apply_sort_change_reload` の PreservePresentation に置き換えると、合成一覧の更新が無操作になり、
物理一覧でも一般更新の意図を変えてしまう。今回はこの分岐を残し、旧 API の名前だけを統一した。
独立レビューもこの判断に同意。

#### ZIP 列挙完了時の設定の確認 (ソースから)

- `poll_zip_enumerate` → `finalize_zip_enumerate` → `prepare_zip_grid_with_nav` は、完了採用時の `self.settings` を参照する。
- 通常の一覧は `materialize_zip_grid(..., &self.settings.grid_display_order, ...)` へ渡す。
- 本の指定階層 (`book_nav`) と rating の指定 ZIP 内階層も `materialize_archive_level(..., Some(&self.settings.grid_display_order))` を使う。
- したがって列挙待ちの間の設定変更は、待ちを置き換えずに完了後の一覧へ反映される。通常 ZIP の実列挙応答を受信境界で保留し、環境設定の適用後に同じ owner で採用する回帰を追加した。実行結果は未取得。
- 環境設定の表示順/未評価位置変更後と、一括変換の完了後に、同期で新しい一覧ができることを必要とする製品の後続処理は見つからなかった。パッドの reveal は既存の採用時の選択復元で再要求される。

#### 追加テスト (15 件、実行未実施)

以下はそのまま nextest の `test(/名前/)` の filter に使える。

| テスト名 | 検査する内容 |
| --- | --- |
| `sort_worker_849_non_duration_wait_blocks_keys_and_clicks` | 名前・日付・サイズで共通 pending ができ、旧 rows へのキー/クリック/Enter を止める |
| `sort_worker_849_duration_to_name_preserves_selected_name` | 長さ→名前の採用後も名前で同じ項目を選ぶ |
| `sort_worker_849_name_to_date_preserves_selected_name` | 名前→日付で番号が変わっても同じ項目を選ぶ |
| `sort_worker_849_preferences_display_order_uses_worker` | 実際の環境設定の適用から待ちを作り、採用後のカテゴリ順と選択を検査 |
| `sort_worker_849_picker_name_change_uses_same_worker_wait` | 実際のパッド入力による非長さ順ピッカーの変更、待ち、採用後の順と選択 |
| `sort_worker_849_batch_conversion_adopts_zip_and_removes_same_name_source` | バッチ完了通知から worker を開始し、生成 ZIP の追加と同名の元アーカイブの除外を検査 |
| `sort_worker_849_zip_enumeration_preferences_leave_owner_and_use_latest_order` | ZIP 列挙待ちで環境設定を変更しても directory scan を始めず、同じ列挙 owner が最新設定で採用する |
| `sort_worker_849_drive_list_preferences_do_not_scan_or_replace_rows` | ドライブ一覧で設定を変更しても rows/世代/場所を差し替えず、走査しない |
| `sort_worker_849_collection_physical_source_sorts_and_preserves_selected_name` | Collection PhysicalSource を worker で並べ替え、親 anchor と選択名を維持 |
| `sort_worker_849_everything_physical_child_sorts_and_preserves_selected_name` | Everything 物理子を worker で並べ替え、検索 position と選択名を維持 |
| `sort_worker_849_tag_physical_child_conversion_refreshes_despite_fixed_sort` | FixedOrder のタグ物理子でも変換後に ZIP を採用し、同名の元を除外 |
| `sort_worker_849_container_named_directories_still_scan` | `.zip` / `.rar` / `.epub` / `.pdf` という空の実ディレクトリも worker で走査する |
| `sort_worker_849_name_date_name_replaces_wait_without_adopting_old_results` | 名前→日付→名前の置換で入力停止を維持し、古い応答を採用しない |
| `sort_worker_849_navigation_replaces_non_duration_wait` | 非長さ順の待ちを実際の直接移動で置換し、入力ゲートを引き継ぐ |
| `sort_worker_849_main_wait_leaves_sibling_context_unchanged` | 兄弟の入力を妨げず、main の採用後も兄弟の rows/世代/場所/選択/scroll/cancel token を変えない |

#### 同期完了前提を書き換えた既存テスト

assert は削除・弱体化していない。

| テスト名 | 書き換えと理由 |
| --- | --- |
| `mutation_refresh_rating_child_sort_immediate_keeps_parent_and_back_forward` | 共通 helper で worker 完了を待ち、既存の順・親/帰路・履歴・兄弟不変の assert を行う。§3 の「長さ以外も完了まで旧一覧が残る」に対応。名称の immediate は旧経路の回帰名として維持 |
| `a_ring_sort_order_reloads_the_window_the_ring_was_opened_in` | 所有 context へ要求が配送された後、worker 完了と既存の identity 準備を待ってから、同じ一覧件数・他窓不変の assert を行う。§3 のパッド並べ替えの非同期化に対応 |

`mutation_refresh_rating_child_sort_worker_keeps_parent_and_back_forward` は同じ helper の mode 引数を外しただけで、従来どおり完了後の同じ assert を行う。
`sort_hold_838_direct_navigation_replacement_inherits_gate` と `sort_hold_838_main_sort_wait_does_not_block_independent_context` は、既存の DurationAsc ケースをそのまま helper へ移し、新規の非長さケースと共用した。
そのほかの既存テストの変更は、撤去した `apply_sort_change_reload_without_ui_io` の呼び出し名の置換だけである。

設計担当向けの filter (この担当はコマンドを実行していない):

```text
test(/sort_worker_849/)
test(/sort_hold_838/)
test(/duration_762/)
test(/mutation_refresh_rating_child_sort_immediate_keeps_parent_and_back_forward/)
test(/mutation_refresh_rating_child_sort_worker_keeps_parent_and_back_forward/)
test(/mutation_refresh_rating_child_stale_sort_owner_keeps_selection_hint/)
test(/a_ring_sort_order_reloads_the_window_the_ring_was_opened_in/)
test(/folder_pane_reclick_same_pass_ready_order_refresh_is_replaced/)
test(/tag_view_open_supersession_tag_refuses_current_view_order_refresh_and_keeps_new_order/)
test(/tag_view_open_supersession_sibling_edge_keeps_real_current_view_order_refresh/)
test(/search_name_snapshot_retires_completed_search_before_prefill_sort_and_enter/)
test(/section1795_off_refresh_and_sort_preserve_current_position/)
```

#### 範囲外の `DirectoryScanRoute::UiThread` 棚卸し (読み取りのみ)

行番号は本実装差分のもの。製品から使用される 11 か所に加え、テストからだけ使う wrapper の定義にも literal が 1 個あり、単純な rg は 12 個となる。
途中でこの 12 個を設計との相違と判断して一時停止したが、caller の監査で製品使用は設計どおり 11 か所と判明し、報告を訂正して再開した。
また、`UiThread` の指定があっても `pre_scan=Some` やファイル専用の継続なら物理ディレクトリの UI 走査は起きない。

| ファイル:行 | 関数 | どの操作から来るか | UI スレッドで物理走査するか |
| --- | --- | --- | --- |
| `src/app.rs:30190` | `poll_open_path_classification` (DirectNavigation 継続) | お気に入り・アドレス・元の場所への移動等のパス分類完了 | しない。directory は分類 worker の `folder_scan` を `pre_scan` に渡す |
| `src/app.rs:30266` | `poll_open_path_classification` (Direct 継続) | 共通 loader から開く EPUB/変換書庫等のパス分類完了 | しない。directory は同じく worker の走査結果を渡す |
| `src/app.rs:30512` | `open_direct_navigation_target` | 右クリックの「元の場所へ」の ArchiveContainer 移動 | `pre_scan` があれば回避。EPUB/RAR 等は分類 worker。未走査の通常 directory へ届けば同期走査可能 |
| `src/app.rs:30853` | `load_folder_or_convert_archive_with_auto_fullscreen_owned_at_stage` | 通常の読み直し、全画面終了後の Direct/CollectionSource、前回一覧の復元、コンテナを開く操作等 | する。`pre_scan=None` の通常 directory は UI で走査。分類対象のパスは先に worker へ |
| `src/app.rs:30875` | `load_folder_or_convert_archive_with_auto_fullscreen_classified` | 別窓で変換アーカイブを開く操作 | しない。製品 caller は分類済み File。directory は別の FolderCandidate 経路 |
| `src/app/collection_navigation.rs:4406` | `commit_collection_navigation` | コレクションの archive entry への移動、外側/DFS/全画面の継続 | しない。`Some(File)` 固定の archive 継続 |
| `src/app/location_jump.rs:71` | `dispatch_address_input` | AddressInput・特殊なお気に入りの場所などの移動 | 検索外 Physical は同期走査候補。検索内は worker、合成一覧は別入口。通常の AddressEnterPhysical は別の Worker 指定 |
| `src/app/location_jump.rs:106` | `dispatch_location_jump` | メニュー・キー・全画面終了後等の LocationJump | 検索外 Physical は同期走査候補。検索内の物理移動は worker。BooksRoot 等は専用入口 |
| `src/app/startup_ops.rs:1637` | `apply_startup_open_path_resolve_result` (RAR fallback) | 起動・activation・agent・bookmark の RAR 解決で volume proof がない場合 | しない。File/RAR が条件で、分類 worker へ渡す |
| `src/app/startup_ops.rs:1805` | `apply_startup_open_path_resolve_result` (通常完了) | 起動・activation・agent・bookmark の解決済み target を開く操作 | する。`pre_scan=None` で resolution が Directory の場合。ファイルは文書/書庫/変換へ分岐 |
| `src/app/update_phases.rs:1329` | `post_render_main_navigation` | navigation 調停の既定 route。Direct/CollectionSource/一部 Favorite/フォルダを開く操作等 | 未走査 Physical は同期走査候補。folder-pane 完了は `pre_scan` で回避。お気に入り/recent/address-enter 等には Worker 上書きの枝もある |

別記: `src/app.rs:30914` の `load_folder_or_convert_archive_with_auto_fullscreen_classified_with_navigation` は、定義以外の caller が `src/app/tests.rs:3958` だけの wrapper。製品から使う 11 入口には含めない。
共通の実同期走査点は `src/app.rs:32307` 付近の `pre_scan=None` → `scan_directory_with_settings`。上記の経路・実装は変更していない。

#### 検証と独立レビュー

- 実施: `cargo fmt` (引数なし、成功)、`git diff --check` (問題なし)、撤去した enum/API 名の `src/` 全体検索 (残存なし)。
- ビルド・テスト・check は利用者指示により一切実行していない。確認用バイナリも同じ指示により作っていない。共有実行口での自動検証と実機確認は未実施。
- 独立レビュー: 実装と別 context の `gpt-6.1-sol` / `xhigh`、read-only のソース検査。初回の追加テストにあった非 Copy 値の move を `.clone()` に修正し、main 採用後の兄弟不変検査を追加した最終差分に **ACCEPT / STRUCTURAL: AGREE**。未解決指摘なし。reviewer も cargo/build/test/check を実行していない。
- git の commit/reset/stash/checkout は実行していない。まれな失敗は従来の worker の通知/終了処理をそのまま使い、回復機構を追加していない。

#### 実装レビュー r1 対応 (2026-11-22、76aaa4617 上の差分)

統括からの実装レビュー結果は **REVISE / STRUCTURAL: AGREE**。76aaa4617 の関連 782 本成功は統括の報告であり、以下の修正後の実行結果ではない。

**P2: 案(a)、一覧採用時の既存 recipe の分類を正す。**

- `src/app.rs` の `install_loading_items_inner` で、受理済みディレクトリ走査から渡された既存の `folder_signature` を根拠に `ProjectionRecipe::for_scanned_directory` を使う。明示された抽出結果等の `VisibleInstallAuthority::Projection` / Rating は従来どおり優先する。
- `src/app/book_materialization.rs` の新しい directory 用入口は、既存と同じ Book/Ordinary 判定と動画・音声の除外を保ち、Book の source scope だけを `ImageFolder` / `CompiledFolder` として確定する。実ディレクトリを拡張子で Document/Archive に分類しない。通常の文書/書庫の `for_physical` による分類は従来どおり。
- Book/Ordinary の判定条件を複製しないため、`src/app.rs` の `physical_page_order_locked` の条件式を純粋な `physical_page_order_locked_with_books_root` へ抽出し、既存の入口は wrapper とした。recipe の両入口は共通 helper で books root を一回だけ解決し、同じパスを policy と scope の決定に使う。既定 root が Windows Shell の既知フォルダ照会を使うことも確認し、追加の照会を行わない形にした。新たな stat / is_dir / read_dir 等の UI I/O はない。
- `src/app/top_level_grid_view.rs` の述語は、正しく生成された既存 Document recipe とページ行で文書を除外する。`folder_observation` の有無は一切使わない。新しい bool・sentinel・App/bundle/snapshot field は追加していない。
- 案(b)の観測の複製は採らない。受動観測の破棄と一覧分類の寿命は異なり、帰路ごとに観測を保存する形では同じ依存が残る。分類を作る時点で既存 recipe を正せば、既存の保存・復元がそのまま正しい分類を運ぶ。

**ほかの保存・復元経路のソース確認。**

| 経路 | 分類の扱いと確認結果 |
| --- | --- |
| Snapshot 固定→子→一覧復帰→解除 | `activate_snapshot` が `saved_projection_recipe` を保存し、`snapshot_return_to_list_view` は固定一覧用 recipe、`deactivate_snapshot` は元の recipe を items と対で復元する。観測が子 ZIP で消えても、正しい元 recipe は残る。指定の経路を回帰に追加 |
| Snapshot の DirectMedia 基底復元・viewer context への保存列移譲 | 既存の `saved_projection_recipe` と items を一緒に運ぶ。recipe の分類を作り直さず、受動観測の有無にも依存しないため同じ修正が効く。これらの経路・detached 述語/viewport は変更していない |
| Global 検索から戻る | `close_global_search` → `restore_view_return_origin` → `dispatch_main_list_restore_destination`。保存 rows の直接復元ではなく、物理の戻り先は既存 navigation で再走査・再採用するため修正した directory 分類境界を通る |
| お気に入り検索から戻る | `close_favsearch` → 同じ `restore_view_return_origin`。物理の戻り先は同じ再走査・採用境界を通る |
| タグ検索から戻る | `close_tag_view` → 同じ `restore_view_return_origin`。物理子・戻り先とも同じ directory 分類が使われる |
| 閲覧履歴から戻る | `exit_reading_history_root` → `dispatch_main_list_restore_destination` → `dispatch_owned_folder_history_destination`。物理の戻り先は `start_physical_navigation_owned` で再走査・再採用される |
| Stack の再採用 | 初回の受理済み directory 列の recipe を既存の Stack 要求・`VisibleInstallAuthority::Projection` が運ぶ。追加の分類状態や観測の復元は不要 |

検索・お気に入り検索・閲覧履歴では、Snapshot と同じ「保存 rows を戻すが観測は戻さない」処理は見つからなかった。再採用後の分類も観測の寿命から独立している。

**P3: 走査失敗通知も検出する。**

- `src/app/tests/sort_hold_838/worker_849.rs` のドライブ一覧と ZIP 列挙待ちのテストで、環境設定の適用前後の `feedback_toasts_for_test` 全体が同じであることを追加した。既存の pending/rows/世代/場所、ZIP owner/cancel の assert も維持。
- これにより、旧 Immediate が合成パスの同期走査に失敗し、rows を保持したまま「フォルダを読み取れませんでした」等の通知だけを出す場合も失敗する。`FeedbackKind` の既存 Eq/Debug を確認し、製品型への derive は追加していない。

**追加・補強したテスト名。**

| 種別 | テスト名 | 内容 |
| --- | --- | --- |
| 追加 | `sort_worker_849_snapshot_child_zip_return_unpin_keeps_directory_reload` | `.epub` 名の実ディレクトリを読み込み、固定→実際の子 ZIP 列挙→一覧復帰→解除で観測が無いまま rows/recipe が復元されること、その後の表示順変更が worker を開始し完了採用することを検査 |
| 追加 | `sort_worker_849_scanned_directory_recipe_keeps_book_policy_without_container_scope` | 空/ZIP行/画像/音声の directory 列を純粋関数で比較し、既存の Book/Ordinary 方針を保った directory scope、通常の文書 scope、CompiledFolder scope を検査 |
| 補強 | `sort_worker_849_drive_list_preferences_do_not_scan_or_replace_rows` | 環境設定適用による通知全体の無変更を追加 |
| 補強 | `sort_worker_849_zip_enumeration_preferences_leave_owner_and_use_latest_order` | 同じ通知全体の無変更を追加 |

- 独立した `gpt-6.1-sol` / `xhigh` に案(a)の設計と最終の affected paths を read-only で再レビューしてもらい、**ACCEPT / STRUCTURAL: AGREE**、未解決指摘なし。reviewer は cargo・fmt・編集・git mutation を実行していない。
- 修正担当は `cargo fmt` のみ実施。ビルド・テスト・check・確認用ビルドは利用者指示により未実施で、共有実行口へ引き継ぐ。git の commit/reset/stash/checkout は実行していない。

### 8.3 テストの実行と独立レビュー (設計担当がまとめた、2026-11-22)

- テスト (設計担当が共有の実行口 run-heavy で nextest を実行。本体 lib の全件は流していない。テストの段階分けによる):
  - `0cb1f5f7b`: 新しいテストのコンパイルエラー (`EverythingPosition` に `PartialEq`・`Debug` が無い) で、1 本も走らなかった。`76aaa4617` でフィールドごとの比較に直した。
  - `76aaa4617`: 関連 filter で 782 本すべて成功。`sort_worker_849_` 15 本と `sort_hold_838_` 33 本が含まれることを `cargo nextest list` で確かめた。
  - `77773f47a`: filter を本の recipe・snapshot・page_order 系まで広げて 1399 本すべて成功。
- 8.2 の「独立レビュー」は、実装担当が自分で起こした確認。ここに記す独立レビューは、設計レビューと同じ Sol / xhigh の会話 (実装担当とは別) で行った。
  - r1 (`76aaa4617`): REVISE / STRUCTURAL: AGREE。P2: `folder_observation` の不在では、文書と復元済みディレクトリを区別できない (★固定 → 子 ZIP → 戻って解除で、`.epub` 名の実ディレクトリが読み直されない)。P3: ドライブ一覧のテストが、同期の走査をしないことを確かめていない。→ `77773f47a` で反映。
  - r2 (`77773f47a`): ACCEPT / STRUCTURAL: AGREE。指摘なし。レビュー担当の言葉: 「これらは実ディレクトリの分類修正に伴う、通常フォルダの扱いへの是正と判断します。」
- r2 で確認された、並べ替え以外の変化 (`.epub`・`.pdf`・`.zip`・`.rar` という名前の実ディレクトリだけ。旧分岐はいずれも v5.5.1 にある):
  - フォルダの自動更新: これまで Document / Archive として除外されていたが、通常のフォルダ監視の対象になる。
  - 本優先の評価: これまで評価できなかったものが、フォルダとして評価できる。保存キーは同じ。
  - 本の構成のメニュー: ImageFolder は画像フォルダ → ZIP の既存の経路、CompiledFolder は既存の対象外の方針になる。旧分類で使えていた編集機能を失う変更ではない。
  - 読書位置・見開き・トリム・ページ編集のキーは変わらない。通常の操作で作ったデータが読めなくなる経路は見つからなかった (レビュー担当、コードから)。
- 実機の確認は未実施 (サブ PC の依頼文を用意、送るのは統括)。

### 8.4 master 取り込み後の修正

2026-11-22、`sort-worker-849` を master `219edde21` へ fast-forward した作業ツリーで実施。§1.849 は `2fde2724d` で master 入り済み。以下は §8.2〜8.3 の分類変更を見直す後追い記録であり、先行記録は当時の判断として残す。

#### 部分1: §1.418 の先送りを維持して、再走査の資格だけを判定する

- 統括の報告: master の本体 lib 全件で `book_preferred_loaded_extension_named_folders_do_not_write_or_record_undo` が失敗。本優先評価のテスト・仕様は名前による分類を §1.418 へ先送りしており、§1.849 で recipe を directory に分類し直す変更はこの決定に踏み込んでいた。
- `src/app/book_materialization.rs` の `for_scanned_directory` / `for_physical_scope` と専用テストを撤去。`src/app.rs` の recipe 採用時の `folder_signature.is_some()` 分岐と `physical_page_order_locked_with_books_root` の分割も撤去した。`for_physical` と `physical_page_order_locked` は分類変更前の `76aaa4617` とテキスト一致を確認。既存の本優先評価テストは編集していない。
- `src/app/top_level_grid_view.rs` の `current_sort_reload_is_physical_directory()` は、非空かつ全行 `PdfPage` を除外し、Document recipe の除外は空の一覧だけに限定する。Document recipe でも非 `PdfPage` 行があれば物理の一覧として走査する。観測レコード・拡張子・UI I/O・新しい状態は使わない。
- 通常の文書行とキャッシュからの仮行は、ともに `document_page_item` が `PdfPage` を作る。通常・prepared・Smart の文書経路を確認し、本の構成は既存行を並べ替え/部分列にするので、実文書の一覧に非 `PdfPage` を混ぜる producer は見つからなかった。
- **空の `.epub` / `.pdf` 名の実ディレクトリは Document 分類のため再走査しない。並べ替える行がなく表示は同じであり、§1.418 の先送り範囲として扱う。**

| テスト | 変更と理由 |
| --- | --- |
| `sort_worker_849_scanned_directory_recipe_keeps_book_policy_without_container_scope` | 撤去した directory 専用分類関数の検査なので削除。分類変更自体を取り消すためであり、本優先評価の期待を弱める変更ではない |
| `sort_worker_849_container_named_directories_still_scan` | 空一覧から、画像1枚を持つ `.zip` / `.rar` / `.epub` / `.pdf` 名の実ディレクトリへ変更。走査の待ち・完了後の場所の確認は維持し、空の assert を画像行の完全一致へ置き換えた |
| `sort_worker_849_empty_container_named_directories_keep_deferred_document_policy` | 追加。空の `.epub` / `.pdf` は pending・世代・通知を変えず除外されることを検査し、空の `.zip` / `.rar` は従来どおり worker 採用まで検査する。全ケースで場所と空行の assert を維持 |
| `sort_worker_849_snapshot_child_zip_return_unpin_keeps_directory_reload` | 変更なしで維持。非空の `.epub` 名の実ディレクトリ→固定→子 ZIP→戻る→解除→表示順変更の回帰 |
| `book_preferred_loaded_extension_named_folders_do_not_write_or_record_undo` | 変更なし。名前による従来の ArchiveRootMismatch / DocumentMismatch、DB/Undo 無変更の期待を維持 |

上記の残存する4テスト名は nextest の `test(/名前/)` filter にそのまま使える。独立 `gpt-6.1-sol` / `xhigh` の read-only レビューは部分1 **ACCEPT / STRUCTURAL: AGREE**。実行検証は未実施。

#### 部分2: 監視の再構築と外部走査の経路調査 (実装前の確認)

- `folder_projection_key_with_presentation` は sort/order を含み、毎フレームの `service_folder_watch` → `ensure_current_folder_watch` は鍵が違えば監視を作り直し、直後に `start_external_rescan(..., UNIX_EPOCH)` を呼ぶ。案Aが対象にする重複経路はソースと一致する。サブ PC 担当による `77773f47a` の約14〜15秒という観測の内訳は、この担当では実測していない。
- 古い監視の `collect` → debounce → `check_external_folder_changes(Notified)` から外部の再走査が始まる経路もある。実際の外部変更との重なりは、利用者指定の既定の割り切り対象である。
- 追加の経路を発見: 監視登録用の初回走査が並べ替え開始時にまだ進行中だと、旧結果は `poll_external_rescan` で projection key の不一致として破棄され、既存 `folder_refresh_pending` へ保持される。`consume_folder_refresh_pending` は並べ替え待ちを見ないため、次の走査を並べ替え待ち中にも開始できる。外部変更通知なしでも成立し、単に「まれな外部変更」とは扱えない。
- この追加経路を受け、利用者の停止条件に従って部分2の編集を開始せず報告した。「同じフォルダの並べ替え待ち中は、既存の先送り要求の消費も待たせる」まで範囲を広げるか、案Aだけに留めて残る経路を記録するかを確認中。新しい状態を追加する案ではない。

部分1の `cargo fmt` (引数なし) と `git diff --check` は成功。ビルド・テスト・check は利用者指示により未実行。git の commit/reset/stash/checkout/merge も未実行。部分2の製品コード・テストは追加範囲の回答待ちで未変更。

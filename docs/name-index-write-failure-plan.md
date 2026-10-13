# §1.369 名前／コンテナ索引の書込失敗

2026-10-12、基点 `ea060b66e`、branch `next-fix`。
状態: **設計レビューのP2 2件を反映・実装レビュー追補済み、全体gate・確認用build成功**。
正本の項目は [next-release-backlog.md §1.369](next-release-backlog.md)、先行棚卸しは
[fts-integrity-plan.md §6.2](fts-integrity-plan.md#62-名前コンテナ索引類似索引の対応範囲)。
本書はその既定方針を、名前索引専用の通知所有と結果の配送まで具体化する。
`f182037c2` の独立レビュー（P1なし／P2 2件）を反映後、利用者が再レビュー前の実装を許可した。
反映内容は実装レビューで併せて確認する。実機再現の報告ではなく、以下はコード調査である。

## 1. 問題と不変条件

名前索引は `search_index.db` 単独。Full は失敗結果を表示するが、実際の DB 書込失敗の
出口が log のみで、名前索引の作り直し印と UI 通知に揃っていない。
watcher は削除・親更新・subtree 更新／prune が失敗しても `updates_applied` を増やす。

- 実際の DB 更新／削除の Err は log + 通知 + 既存の名前索引 rebuild marker へ一度配送する。
- FS の不完全観測、明示取消、正常 no-op は索引破損扱いにしない。不完全な観測で親置換・prune
  を行わない既存契約を維持する。SQL Err の後に cancel が立っても実際の Err を隠さない。
- marker は `SearchIndexDb::request_rebuild_on_next_start` が正本。現在の session で索引を
  消さず、通常監視・検索・[今すぐ確認] を維持する。後の成功で marker を消さない。
- 次の writable open の `rebuild_names_if_requested` を再利用する。readonly open は消費しない。
  名前索引以外の DB・利用者の設定／タグ／本などを変更しない。schema／移行も追加しない。
- **まれな失敗は割り切り (ログ + 通知 [+ 作り直し])**。retry、回復 worker、途中状態保持、
  回復失敗の再回復、shutdown の通知待ちを作らない。

## 2. 入口／出口と扱い

行番号は基点の source。実装で行が移動したら関数名を優先する。

| 入口／出口 | 報告境界と結果 |
| --- | --- |
| initial / manual / overflow Full | `name_index_supervisor::run_full_scan` (443)。全入口が同じ関数へ来る。`prepare_full_scan` の Err (468) は既存の Failed を保ち、共通失敗出口へ |
| Full の直下置換 | `name_bulk_indexer::run_bulk_name_index_with_reader` の `upsert_children_if_changed` (224) の Err だけを報告。`Ok(false)` は正常な未変更。`had_error` は既存の prune 抑止／Full Failed にだけ使い、全 had_error を rebuild に変換しない |
| Full の最終 prune | `prune_stale_for_favorite` の Err (251) を報告。cancel / read_dir / entry / depth 不完全時の prune 抑止を維持 |
| Full 完走印の保存 | `record_full_scan_outcome` の `record_complete_scan` Err (553) は **log + 専用通知のみ**。索引行の走査成功を Failed に変えず、全再構築を予約しない。旧印が残る場合もあるので「次回起動で必ず再走査」とは通知しない |
| watcher の missing root / ancestor / subtree | `handle_missing_path` の `delete_subtree` Err (665, 689, 725) を報告。成功結果だけを apply caller へ返す。`Ok(0)` は正しい空状態の正常成功 |
| watcher の parent refresh | `refresh_parent_listing` の `upsert_children` Err (874) を報告。read_dir / entry error / cancel は報告対象外、false の既存 prune 抑止を維持 |
| watcher の subtree upsert | `run_subtree_scan` の `upsert_children` Err (973) を報告。既存 Completed / Cancelled / Errored を維持し、不完全時の prune を抑止 |
| watcher の post-scan prune | `handle_existing_path` の `prune_stale_under_subtree` Err (790) を報告し、イベント全体を成功件数へ加えない |
| OFF / root 変更 / 削除の clear | `name_index_manager::DatabaseBackend::clear` (143) の既存 marker 設定を共通失敗出口へ置換する。二重に予約しない。stop→join→clear と cleared 管理、次構成の開始順は変更しない |
| read-only query / folder count / can_skip_initial_scan | 読取失敗の既存 log / Full への fallback を維持。書込失敗出口へ入れない |

旧 `spawn_bulk` は製品内の呼出しがないが、共通 bulk 本体を呼ぶので同じ出口を通る。
根拠のない FS 観測の成功化や、エラー文字列での除外は行わない。

`bb6ca0c66` と [FTS 計画 §12.4](fts-integrity-plan.md#124-公開直前の無害なmerge拒否の誤通知修正-2026-10-12)
の教訓は、失敗を作った境界で正常な no-op と真の Err を区別すること。
名前索引には Tantivy merge はないため、その型や汎用 InvalidArgument 除外を移植しない。
未変更集合、削除対象0行、cancel、FS 読取不完全、完走印だけの保存失敗を一律 rebuild にしない。
更新を行う API の比較 SELECT 失敗も、その更新処理の Err として扱うが、一般検索の読取失敗は含めない。

## 3. 通知の所有とロック

採用案: App が既に所有する `Arc<SearchIndexDb>` を通知の単一所有者にもする。
DB に短い **通知専用 mailbox** と worker 専用の失敗報告 helper を置く。
更新失敗の予約試行と通知は、同じDBインスタンスの起動中に1回だけ。
完走印だけの失敗通知も、同じDBインスタンスの起動中に1回だけ。各失敗のログは毎回残す。
`std::sync::Once` の受付をdrain・Full開始・root切替・後の成功でリセットしない。
予約に失敗しても次のfailureで試行し直さない。最大2件の通知だけをmailboxに保持する。
これは表示待ちの通知だけであり、回復状態／retry 管理／per-root pending ではない。

1. bulk / supervisor / manager の DB API が Err を返して SQLite lock を解放した後、
   origin・root/path・error を共通 helper に渡す。helper はまず log を出す。
2. 更新／削除 failure の helper はDB lifetimeで一度だけ既存 marker を設定し、
   その成否に応じた文面を mailbox へ置く。複数root／長いFull／drain後も再予約・再通知しない。
   完走印 failure は専用通知だけを置く。marker 設定に失敗しても別印・再試行をしない。
3. App の既存 `poll_indexer_notifications` (30492) が名前 DB の mailbox も drain し、
   既存 feedback toast へ渡す。FTS manager が Unavailable でも名前通知を消費できる。
   名前通知のdrainは起動診断通知と同じ「overlay非表示・root可視・OS非最小化」だけ。
   overlay中・tray非表示・最小化中はDB内で保持し、通常UIが描画可能なpollで
   5秒のtoast時計を開始する。native動画overlayにも同じlingerを渡す。FTS の既存配送は変えない。
4. poll は mailbox lock だけを短く取る。SQLite lock、marker 読取、I/O、join を UI に追加しない。
   DB lock と mailbox lock は同時に保持しない。callback、App/manager/supervisor を DB から参照せず、
   Arc cycle を作らない。終了時は表示のために App を延命しない。

実装文面（glyph lint成功）:

- 予約成功: 「名前索引の更新に失敗しました。mIV を再起動すると名前索引を作り直します。」
- 予約失敗: 「名前索引の更新に失敗しました。再構築を予約できませんでした。ログを確認してください。」
- 完走印のみ: 「名前索引の完走記録を保存できませんでした。ログを確認してください。」

root supervisor の入替え／停止後も同じ DB の通知は App の poll まで残る。
他の DB インスタンス（別 test fixture / readonly handle）に通知を移さない。
main/detached の viewer state、viewport、context 世代は変更しない。これは共有索引の通知であり、
閲覧中のページへ書き込みを配送する処理ではない。FTS reporter / `fts_meta.db` の pending は流用しない。

## 4. 成功件数と設計の簡素化

`apply_single_change` はイベントの完全な成功結果だけで `updates_applied` を増やす。
missing delete の正常結果、parent refresh、必要な subtree scan と post-scan prune を確認してから
数える。部分更新の既存行は保持するが、イベント全体の成功とは数えない。
既存の `SubtreeScanOutcome` を helper の完了配送に再利用し、FS 不完全／cancel の
通常遷移を保つ。DB failure の種類別回復状態、追加 owner/pending/bool 群は作らない。

最初に組合せを減らす案を検討した。索引更新を modal にして閲覧を止める案は通常操作を不便にし、
必要がないため採用しない。失敗時に supervisor を再作成する案も追加の停止／開始状態を作る。
既存の DB、worker、次回 open の marker 消費へ揃えることで、回復の組合せを増やさない。
per-supervisor に通知を置く案は OFF/clear 時に owner が消え、通知の引継ぎが必要になるため不採用。
DB 所有の mailbox に集約する案は、その引継ぎ自体をなくす。

## 5. 実装範囲と受入条件

実装範囲: `search_index_db.rs`（通知と共通出口）、`name_bulk_indexer.rs`、
`name_index_supervisor.rs`、`name_index_manager.rs`、`app.rs`（配送）。
同時更新: 本書、search-architecture の名前索引節、fts-integrity-plan §6.2 の引継ぎ状態。
事前レビューのP2（drain後の反復、overlay／最小化中の消費）を上記の契約に反映した。
利用者の実装許可に従い再レビュー待ちを挟まない。実装レビューで反映と回帰を確認する。

使い捨て SQLite fixture の実 trigger 拒否で、prepare / Full upsert / Full prune、
watcher root/ancestor/subtree delete、parent/subtree upsert、post-scan prune を検証する。
各ケースで通知、名前 marker、既存行保護、成功件数、FTS marker の非変更を assert する。
marker 設定自体の拒否は予約成功を表示しないことだけを検証し、再回復は作らない。
完走印のみの拒否は専用通知・Full の行更新成功・rebuild 非予約を検証する。

正常 Full/watch、未変更 Full、delete 0行、不完全 FS、cancel は誤通知／rebuild がないことを検証する。
不完全 FS / cancel の既存 prune 抑止、stop→join→clear、clear-failure 回帰も維持する。
DB 2個と root 2個、supervisor 停止前の failure と停止後の drain、非表示／overlay／最小化→表示復帰、
FTS Unavailable 時の App 配送を非対話 handler-level test で検証する。
drainを挟む複数失敗・複数root・長いFullでも予約試行と通知がDB lifetimeで1回に留まることを、
実際の予約transactionの監査triggerで検証する。予約失敗後の次failureでも再予約しない。
UI mailbox drain が SQLite lock を取らないことも、worker 側の DB lock を保持した fixture で検証する。

targeted: `--lib name_bulk_indexer` / `name_index_supervisor` / `name_index_manager` /
`search_index_db` と配送 handler、`--test search_name_e2e`。
fmt、UI glyph lint、通常／portable core check、項目末に一度の test-full を実行する。
実アプリは起動せず、コミットは coordinator が行う。実行結果は次節に記録する。

## 6. 実装レビューと検証記録

独立実装レビュー（GPT-6.1 Sol / xhigh）はP2 1件（native動画toastへ表示時間を渡していない）を指摘。
既存の時間指定APIへ5秒を渡し、Appの名前通知試験でdisconnected VideoPlayerと実command busを使って
ShowToastのlingerを検証した。追補レビューは修正必須指摘なし。アプリ／rendererは起動していない。
DB lifetimeのOnceは手動Defaultで初期化する。途中のコンパイル不備は修正し、成功結果を以下に記録した。
中断した3回は試験を実行しておらず成功件数に含めない。うち1回は後続compilerの
プロセスを見落とした確認誤りによる中断で、製品テスト失敗ではない。

検証担当は実装root。対象は本書の未コミット§1.369差分、通常Windows feature。
`CARGO_BUILD_JOBS=1`、`RUST_TEST_THREADS=4`、VS18のlibclang、実コピーのFFmpeg DLLを使用。

| command（`cargo test -p mimageviewer`） | 成功／失敗／ignore | ログ（`target/`） |
| --- | --- | --- |
| `--lib name_index_` | 32 / 0 / 0 | `nextfix-1369-name-tests-retry.log` |
| `--lib name_bulk_indexer` | 16 / 0 / 0 | `nextfix-1369-name_bulk_indexer.log` |
| `--lib search_index_db` | 48 / 0 / 0 | `nextfix-1369-search_index_db.log` |
| `--lib index_full_check_tests` | 13 / 0 / 0 | `nextfix-1369-index_full_check_tests.log` |
| `--test search_name_e2e` | 17 / 0 / 0 | `nextfix-1369-search-name-e2e.log` |

5実行126件成功（名前通知handlerの重複1件を除くと125件）。各exit 0。
fmt checkとUI glyph lint（危険字形0件）もexit 0。
通常 core checkはexit 0（1分26秒）、portable core checkもexit 0（1分12秒）。
ログは `nextfix-1369-core-check.log` / `nextfix-1369-portable-check.log`。
release前提build、test-full、確認用buildは以下のとおり成功。

`build-release.ps1 -SkipVst3Bridge -PreserveRuntime` は稼働中mIVを検出し保護規則で拒否した。
アプリを止めず、CLAUDE.mdのtest-full前提にある直接Cargo手順で当worktreeの3本を作る。
`CARGO_INCREMENTAL=0`、core→remote（embedded-web-assets）→EPUB workerの順。
通常APPDATAのruntime cache／署名／配布stageを変更しない。
wrapperの拒否ログは `nextfix-1369-release-build.err.log`。外側Start-ProcessのExitCodeがnullだったため、
親commandのexit 0をbuild成功の証拠には使わない。直接Cargoの実exit codeと成果物で判定する。

API側の中断（利用者報告: model at capacity / stream disconnected）の後、同じHEADと差分、
成功済みログ、own buildプロセスなし、release入力3本未生成を確認して直接Cargoを再開した。
成功済みのtargeted／fmt／glyph／通常・portable checkは再実行しない。
中断したcore buildログは `nextfix-1369-release-core.log`、再開ログは `nextfix-1369-release-core-resume.log`。
再開後のrelease coreはexit 0（39分37秒）。`target/release/mimageviewer-core.exe`を生成した。
remote（embedded-web-assets）もexit 0（4分59秒）、EPUB workerもexit 0（2分36秒）。
release入力3本が揃った後、`test-full.ps1 -SuppressCrashDialogs`を項目末のgateとして開始した。
ログは `nextfix-1369-full.log` / `nextfix-1369-full.err.log`。実アプリは起動しない。
第1 fullはexit 101。primary結果（filtered out=0）は13,317成功／41失敗／63 ignore。
失敗targetは `libraw-sys --lib`（4件）と本体lib（37件）だけで、全て `vendor/raw-samples` の欠落。
本体libは12,105成功／37失敗／56 ignore。成功済みtargetをfull成功へ読み替えない。
RAWサンプルの前提を見落としたため、`docs/raw-libraw-plan.md` §15と
`tests/raw-samples.json`に従い、`C:/home/mimageviewer/vendor/raw-samples`からCC0の27ファイルを
当worktreeへ実コピーした。sourceとdestinationのSHA-256／サイズを検査し、
`setup-raw-samples.ps1`の既存ファイル検証もexit 0（27件）。ダウンロード・junction作成はしていない。

狭い `cargo test -p libraw-sys --lib all_cc0_sample_dimensions_match` は別依存構成を作り、
turbojpeg native buildでexit 101、試験実行0件。製品／試験sourceは変更しない。
第1 fullで生成した `libraw_sys-703273c684882100.exe --exact tests::all_cc0_sample_dimensions_match`
を使った同構成のfixture検証は1成功／0失敗／0 ignore（114.32秒、exit 0）。
各ログは `nextfix-1369-raw-fixture-{verify,test,exe-test}.log`。
この前提修正を理由として第2 fullを実行した。ログは `nextfix-1369-full-retry{,.err}.log`。
第2 fullは **PASS／exit 0**。workspace＋vendor egui／egui-wgpu／eframeの全gateを通過した。
primary結果（filtered out=0）は **13,412成功／0失敗／63既存ignore**。
本体libは12,142成功／0失敗／56 ignore。childのfiltered実行は主集計へ加算しない。
fixture修正後はソースを変更せず、失敗のskip／期待値緩和／製品への症状パッチは行っていない。

`build-dev.ps1 -PreserveRuntime` を通常feature（portable／test-scriptなし）で実行した。
新規 `target/dev-runtime` はworkspace内、parentのtargetはreparse pointでないことを確認した。
ログは `nextfix-1369-dev-build{,.err}.log`。アプリは起動せず、利用者へ確認用コマンドを渡す。
初回確認用buildはturbojpeg native buildのexit 1で停止（外側exit 101）。生成済み同projectの
ZERO_CHECKを詳細ログで直接実行するとexit 0、installを1並列で実行してもexit 0
（4分5秒、警告0／エラー0）だった。元の失敗ログにはMSBの具体的なerrorがなく、
原因を製品コードや並列性へ断定しない。全体gateの成功結果は失効していない。
native依存を生成後、`CARGO_BUILD_JOBS=1`で同じ`build-dev.ps1 -PreserveRuntime`を再開した。
source／feature／profileは変えず、再実行ログは `nextfix-1369-dev-build-retry{,.err}.log`。
native診断ログは `nextfix-1369-native-{diagnostic,install}.log`。
再開scriptのnative待機は、当worktreeの上記out/buildをcwdにしたMSBuild worker10個が
残っていたため。全て `/nodemode:1`、親終了済み、子はconsole hostのみだった。
この条件を確認した自分の残存workerだけを終了し、待機scriptの通常再開を確認した。
他worktreeのbuild、稼働中mIV、ユーザーデータは操作していない。
fullは `CARGO_BUILD_JOBS=2`、`RUST_TEST_THREADS=4` で実行した。
確認用buildの初回は2並列、native前提の診断後の再実行は上記のとおり1並列。
ソース／feature／profileは変更していない。

再実行は **exit 0**。core（55分7秒）／remote（6分50秒）／EPUB worker（3分43秒）が
全て生成され、FFmpeg DLL6本・VCRT4本・EffeTune bundleをdev-runtimeへ配置した。
VCRT／PE gateも成功（runtime=4／pe=3、`target/vcrt-pe-reports/dev-runtime.json`）。
成功済み検証後の変更は文書のみ。確認用成果物も起動せず、利用者へ
`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe` を渡す。
既定は通常 `%APPDATA%\mimageviewer` profileで実データを更新し得るため、
共有single-instance mutexを使うインストール版／tray常駐mIVを利用者が閉じてから起動する。
手動確認は使い捨てfavoriteのCtrl+S検索、子folder／ZIP／PDFの追加・削除と[今すぐ確認]。
真の書込失敗は使い捨てDB triggerで自動検証済みであり、実DBを破損させる手順は求めない。
コミットは行わず、§1.372は設計書で止める。

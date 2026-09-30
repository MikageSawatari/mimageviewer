# §1.313 現在ビューの mutation refresh 監査 (2026-10-01)

観測者は利用者 (v430-integration、2026-10-01)。Collection C → 登録実フォルダ B で代表サムネを固定すると BS が B の実親を指し、← に B の一覧が二重に出る。以下はコード調査と headless 状態遷移テストの記録であり、エージェントによる実アプリ観測ではない。

## 不変条件と修正境界

同じ可視場所の再表示は採用済み location owner が所有する。CollectionPhysical の root anchor／親 provenance、RatingPhysical の chain、navigation history の back／forward を変えない。`folder_history` は navigation history とは別のスクロール／カーソル保存領域なので、再表示時に消さない。

`current_folder_reload_owner` が現在の物理地点の typed owner を選び、F5／mutation reload と pre-scanned external reload で共有する。Collection／Rating の owner に `Refresh` intent を持たせ、変換書庫の cache path が論理 source と異なる場合にも履歴・親 chain を追加しない。通常 Path の採用境界も、現在の backing と一致する場合は既存の archive alias 解決で論理 source を履歴 identity に使い、既存の等値比較で履歴を増やさない。追加の suppress flag／collection 専用 reload 禁止分岐は設けない。

ZIP pin は既存の階層再 materialize を使い、選択・スクロール・チェック・ローカルフィルタを保持し、未完了のローカル検索を再発行する。明示 F5 の ZIP 再列挙は維持する。Smart Folder は既存の resident snapshot 契約、subfolder expansion は既存 snapshot restore を維持する。Search／Tag／Rating 等の合成行は通常 `load_folder` に渡さず、pin intent なら既存 metadata-pin worker と generation 照合付き適用で資産だけを更新する。Full intent (設定変更・ファイル操作) は既存 surface owner の再入場を使う。

folder dirty は変更コンテナ集合へ置換した。pin 更新対象はフォルダと動画の集合を一つの `Pins` intent が所有し、同一 context／generation の先行要求を統合する。フォルダ変更は自動代表・cascade のため変更コンテナの祖先タイルも失効させ、動画依存の cascade 解決は worker で行う。未変更タイルの UI 失効は避ける。要求は viewer context ごとの単一 typed resource が所有し、置換・retired context・App drop で取消、完了時には既存 egui context を起こす。共有 token を交換する適用経路は未完了動画と変更動画の producer を再開し、既に Loaded の無関係な動画は保つ。ZIP の設定対象が現在階層に見えない場合はそのコンテナの dirty だけを除去する。

合成 ZIP 本タイルの依存は同じ書庫内の兄弟参照を含むため、変更された本と同じ書庫のタイルを更新し、別書庫は保持する。Ctrl+G の動画更新は既存 streaming 候補選択（検索中は pin blob のある動画だけ）と空 sidecar map を使う。専用 search producer の token は main context だけが取消・交換し、兄弟 context の更新はその context の token／channel だけを交換する。retired 要求の取消は fullscreen 等の早期 return より前の共通 housekeeping で行う。

簡素化として既存 reload／prepared scan／metadata-pin 更新経路を再利用した。モーダル待機は不要で、短い資産 refresh のために通常操作を遮断しない。detached 固有述語・viewport 経路は変更していない。fix1 の sort worker consumer 内の owner 転送だけは、利用者の明示判断と ClaudeCode／独立 Codex の構造合意の下で変更した ([detached-rework-plan.md §11](detached-rework-plan.md#11-リワーク外からの変更記録))。

## 同型経路の列挙

| 経路 | 判定 | 理由 |
| --- | --- | --- |
| `consume_folder_thumb_pin_dirty` (pin／unpin) | fix | bare Navigation が Collection 子を plain Folder として採用。共通現在ビュー reload へ集約 |
| `consume_video_thumb_overrides_dirty` | fix | 同じ bare load。既存可視性判定・pending frame／cooldown・Smart frozen 方針を維持して共通 reload |
| `consume_folder_refresh_pending` (export／外部変更の遅延反映) | fix | 同じ bare load。対象一致と検索中の延期を維持して共通 reload |
| `apply_external_rescan` | fix | `load_folder_with_scan(folder, Some(scan))` が Navigation。取得済み scan に共通 reload owner を付与し再走査を避ける |
| `toggle_stack_mode` | fix | 状態変更後の `load_folder(folder)` が Navigation。選択 hint／stack target を維持して共通 reload |
| `reload_current_folder_preserving_override` (F5／`pending_reload`) | fix/shared | 物理 owner 選択を共通化し、Refresh intent で cache alias を含む履歴／Rating chain を保持。synthetic は Full と pin intent を同じ router で surface owner へ渡す。通常 Path F5 は既存同一地点比較で history 不変 |
| rename／new-folder／delete 後の `pending_reload` | keep/shared | rename／new-folder は共通 reload consumer。delete は既存 batch remove／selection remap が主処理で、必要な reload だけ同じ consumer を使う |
| Preferences の列挙・Susie 設定変更 | keep/shared | `reload_current_folder_preserving_override` または `apply_sort_change_reload` を使用 |
| sort (Collection 子／ZIP／合成 root)／RAR batch completion／EPUB batch completion | keep/shared | 合成ビュー別 router、Collection physical owner、worker scan の owner が既にある。ZIP は同階層の再 materialize を先に選ぶため cache alias の navigation reload に入らない |
| sort (RatingPhysical 子、Immediate／WorkerScan) | fix (fix1) | 両経路の通常 Navigation fallback が親 chain と back／forward を変えていた。F5 の共通 owner 選択を再利用し、`CurrentViewOrderRefresh` が単一の `OpenRequestOwner` を全 consumer へ転送する。共通採用境界は選択 hint の変更前に owner を検証。detached consumer 内は機械的転送だけで、freeze 手続きの構造合意を §11 に記録 |
| pin worker の共有 DB schema open (`FolderThumbPinDb::init_schema`) | fix (fix1 検証で発見) | 兄弟 context の同時 reopen が DEFERRED schema 読取から書込へ upgrade して deadlock。開始時に schema writer を取得する IMMEDIATE transaction へ変更し、原子的な revision／trigger 導入と既存 timeout を保持 |
| search close／search-child sibling／parent／smart/subfolder exit | keep | `load_folder(saved/next_path/top/target/path)` は保存された地点への復帰や明示 navigation。typed restore／history suppression はそれぞれの router が所有 |
| book root／slideshow NextFolder／folder pane／JumpToPhysicalFolder | keep | `load_folder(root/next_path/ready.path/path)` は別地点を採用する意図的 navigation |
| snapshot required-fullscreen／internal navigation | keep | snapshot 範囲内の別コンテナを開く navigation。mutation refresh ではない |
| detached descriptor／prepared image-book／detached pane | keep | detached 固有の既存 navigation。依頼の変更禁止範囲 |
| `load_folder(cur/folder/current)` が残る test fixtures | keep | テストの初期配置／navigation の作成であり mutation consumer ではない |

## 検証

Collection root → physical child を実 owner／scan で採用した fixture で pin／unpin、video pin、deferred export、external rescan、stack toggle、F5 をそれぞれ通す。current target／PhysicalSource／BS collection root、back／forward、選択・スクロールが保持され、parked sibling の rows／generation／token／location／position が不変であることを検査する。合成 surface は worker 完了による repaint wake と rows／history／position の不変、ZIP は同一階層と位置・検索・check を検査する。

以下は初回修正 `86e051181` の検証記録。実行環境は Windows、default features、dev/test profile、`MSBUILDDISABLENODEREUSE=1`。対象 source 5 ファイルの SHA-256 は `target/pin-reload-source-hash.txt` に記録した。fix1 差分の再検証は末尾に別途記録する。

| コマンド | 結果 | ログ (`target/` 内) |
| --- | --- | --- |
| `cargo fmt --all -- --check` | exit 0 | `pin-reload-fmt.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0 | `pin-reload-core-check.log` |
| `cargo test -p mimageviewer --lib mutation_refresh_` | exit 0、18 pass／0 fail | `pin-reload-focused.log` |
| `cargo test -p mimageviewer --lib` | exit 0、9903 pass／0 fail／51 ignored (616.20 秒) | `pin-reload-lib.log` |
| `cargo test -p mimageviewer --test ui_snapshot` | exit 0、55 pass／0 fail | `pin-reload-ui-snapshot.log` |
| `python scripts/check_ui_glyphs.py` | exit 0、危険字形 0 | `pin-reload-glyphs.log` |
| `cargo run --locked -p viewer_context_audit --quiet` | exit 1、既知 6 violation／追加 0 | `pin-reload-audit.log` |

監査の6件は A4 (`viewer_context_ids`／`viewer_context_residence` の非 Windows API) 2件と A6 (`video/decoder.rs` の test-only 呼出し) 4件。指摘対象2ファイルと audit tool が起点 `b98dce051` から未変更であることを `git diff --quiet` (exit 0) でも確認した。allowlist を追加して隠していない。実装担当の補助 Sol / xhigh completion review では当時残存指摘を検出しなかった（利用者指定の独立レビューとは別）。追加のソート経路の確認では上表の RatingPhysical 不具合を両者が確認し、変更禁止境界で停止した。

回帰テストには通常・Collection の変換キャッシュ ZIP の実 preflight、Rating の parent chain、同一書庫の兄弟本依存と別書庫の保持、合成 request の統合・古い generation の拒否・retired request の取消、main search producer を兄弟 context の terminal 適用が取消しない検査を含む。

確認用 `build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` は exit 0 (`target/pin-reload-build-dev-handoff.log`)。`portable` は付けず、normal profile の core／remote／EPUB converter と必要 DLL を `target/dev-runtime/` に配置した。環境は `MSBUILDDISABLENODEREUSE=1`、最終 Cargo jobs は4。初回の turbojpeg-sys native build は CMake/MSBuild exit 1 となり、同じ worktree の依存だけを `cmake --build ... --parallel 1` で完了 (exit 0、`pin-reload-turbojpeg-build.log`)。残存 MSBuild の CPU 時間不変と compiler 非稼働を確認し、自分の待機実行だけを中断して既存 wait override を使った。プロセスを停止する運用変更や build script の変更はしていない。

上記は初回18-test差分のビルド記録。RatingPhysical sort 修正を含む fix1 の確認用成果物は以下の最終差分で再ビルドした。製品バイナリの起動・UI smoke は行っていない。

## fix1: RatingPhysical sort (起点 `86e051181`)

利用者の 2026-10-01 判断を受け、`target/pin-reload-sort-proposal.patch` と同等の変更を適用した。Immediate と WorkerScan の producer が共通 reload owner を使い、worker payload の Collection 専用 Option を単一の `OpenRequestOwner` へ置換する。main／detached／非 Windows の既存 consumer は同じ owner を転送し、共通採用境界で owner を検証してから cursor hint を保存する。追加の状態 flag や detached 専用 guard／遅延／retry はない。

owner の直置きは App を110,664 bytesに増やし、既存 footprint テストと fullscreen 入力テストの stack overflow (0xc00000fd) で検出された。worker payload だけを `Box<OpenRequestOwner>` に保持して型と転送契約を維持し、App は109,712 bytesへ戻った。既存の110,000 bytes上限は変更していない。

ソート変更時の先頭移動と `folder_history` (位置保存領域) の破棄は既存仕様として維持する。fix1 が保持するのは可視地点の owner／親 chain と navigation history の back／forward であり、F5／pin refresh の位置保持仕様をソートへ新設する変更ではない。

3件の状態遷移回帰を追加した。評価 root から実 owner で親フォルダ→子フォルダを開いた fixture で、Immediate／実 scan worker 完了をそれぞれ通し、変更後の行順、RatingPhysical location、二段の親 chain、保存地点、BS の評価親ルート、back／forward stack と履歴 target を確認する。両経路で parked sibling の行・generation・token・選択・スクロールも保持される。同じ path／order のまま source generation を更新した旧 worker 完了は、行・履歴・選択・selection hint を変更しない。`select_after_load` は既存の App-global 操作 hint なので、兄弟固有の hint があるという fixture は作らない。

実装担当の補助 Codex (Sol / xhigh) は実装前・完了レビューで detached の機械的転送が構造修正であることを確認した。この補助レビューは利用者指定の独立レビューではない。検証の実行は実装担当が所有し、レビュアーによる重複 build／test／起動は行っていない。

検証中に既存の parked synthetic pin 回帰が間欠失敗した。worker の結果をそのまま consumer へ渡す前に errors を検査する診断を追加し、単独再実行の3回目で `フォルダピンDBを再読込できませんでした: database is locked` を確認した (`target/pin-reload-fix1-pin-repeat-3.log`、0.15秒)。2つの context worker の schema open が DEFERRED transaction 内の schema 読取後に `INSERT OR IGNORE` へ upgrade して競合する。pin map／reset indices が作られず、owner や generation の誤りではない。`FolderThumbPinDb::init_schema` を既存の単一 schema transaction の開始時に writer を取得する `BEGIN IMMEDIATE` へ変更した。2秒の既存 timeout、原子的な revision row／triggers、pin rows は保持する。新しい retry／delay／guard は加えず、detached 固有コードには及ばない。独立レビューもこの所有境界の修正に合意した。

DB 回帰は新規・初期化済み store を各8 workerで同時に open し、全取得成功、初回 migration が一度だけ、同じ instance／revision と既存 pin の保持を検査する。parked 回帰の失効・context 隔離の assertions は弱めていない。

### fix1 最終検証

Windows、default features、`MSBUILDDISABLENODEREUSE=1`、起点 `86e051181` の未コミット差分。検証後から確認用 build 完了まで source 3ファイルが不変であることを `target/pin-reload-fix1-source-hash.txt` の SHA-256 と照合した。当時の実装担当の補助 Sol / xhigh レビューは Box 転送・DB schema writer 修正を含め指摘を検出しなかった。その後の独立レビューの P2 と fix2 は以下に記録する。

| コマンド／検査 | 結果 | ログ (`target/` 内) |
| --- | --- | --- |
| `cargo fmt --all -- --check` | exit 0 | `pin-reload-fix1-fmt.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0 | `pin-reload-fix1-core-check.log` |
| `cargo test -p mimageviewer --lib mutation_refresh_` | exit 0、21 pass／0 fail | `pin-reload-fix1-focused.log` |
| `cargo test -p mimageviewer --lib folder_thumb_pins::tests::` | exit 0、55 pass／0 fail | `pin-reload-fix1-pin-db.log` |
| parked pin 状態遷移の単独20回 (同じ lib test artifact) | 全20回 exit 0 | `pin-reload-fix1-pin-repeat-fixed.log` |
| 既存 App footprint／fullscreen capture-region 回帰 | exit 0、full lib にも含む | `pin-reload-fix1-footprint.log`、`pin-reload-fix1-stack-regression.log` |
| `cargo test -p mimageviewer --lib` | exit 0、9907 pass／0 fail／51 ignored (663.69秒) | `pin-reload-fix1-lib.log` |
| `cargo test -p mimageviewer --test ui_snapshot` | exit 0、55 pass／0 fail | `pin-reload-fix1-ui-snapshot.log` |
| `python scripts/check_ui_glyphs.py` | exit 0、危険字形 0 | `pin-reload-fix1-glyphs.log` |
| `cargo run --locked -p viewer_context_audit --quiet` | exit 1、既知6 violation／追加0 | `pin-reload-fix1-audit.log` |
| `build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | exit 0、Cargo jobs=4 | `pin-reload-fix1-build-dev.log` |
| `git diff --check` | exit 0 | 最終差分で確認 |

監査6件の内容は初回と同じ A4 2件／A6 4件。該当2ファイルと audit tool は `86e051181` から未変更 (`git diff --quiet` exit 0)。allowlist は変更していない。ビルド開始時に他の native compiler／MSBuild がないことを確認して既存 wait override を使用した。`portable` は有効にせず、normal profile の core／remote／EPUB converter と DLL を `target/dev-runtime/` へ配置した。製品の起動・UI smoke・通常プロファイルの操作は行っていない。利用者による実機確認は未実施。

## fix2: pin materialization の worker 境界 (起点 `0d11e5325`)

2026-10-01 の独立 Codex レビュー（gpt-6.1-sol / xhigh、session `01a0f38f-1566-75e0-bfa7-4f1fdf06d28b`）は、合成ビュー pin 完了が UI thread で FS metadata、cascade pin DB lookup、catalog DELETE、動画 pin 読取と seed 書込を行う P2 を指摘し、changes needed と判定した。他の owner／history／context routing の追加指摘はなく、pin DB の IMMEDIATE schema transaction と detached typed owner 転送は構造修正として合意された。ClaudeCode と独立レビューの detached 合意日時・session は §11 に正確に記録した。

同型 consumer は合成 pin の `poll_current_view_pin_refresh` と明示 metadata import の終端適用（main／active／parked／非 Windows）で、すべて共通 `apply_current_metadata_import_terminal_result` に入る。両 producer が既存 `metadata_import_refresh::run` に container row、metadata、依存 root、sort／depth／cache-key policy、cache／catalog ownership のメモリ snapshot を渡す。worker が sparse dirty 範囲を絞り、FS／cascade／video DB の参照と catalog materialization の更新を完了してから結果を送る。ZIP alias の論理 root は要求時の既存 resolver で保持する。

worker は表示中の共有 cache を直接変更せず、worker 上の private map に削除／seed を適用する。影響 folder がない結果は map を交換しない。先行要求は後続 worker を起動する前に cancel し、catalog の writer lock 内でも cancel を検査する。さらに、一覧世代が変わる共通 `App::set_items_generation` で、その projected context の旧 pin 要求を退役させる。合成 worker が frame A を読んだ後に実フォルダへ移動して動画を frame B に再固定し、新 view が同じ key に B を seed してから旧 A が永続化される順序を、後続 view の catalog seed より前の generation ownership 境界で断つ。mount／deposit／restore は bundle の raw swap であり、この取消を通らない。 metadata import も同じ世代の旧 worker と同一 key を競合できる（実フォルダ由来の Snapshot など）。`take_metadata_import_refresh_requests(changed)` が pin 変更のある対象要求を確定した時点で、その context の旧 pin owner を退役させてから後続 worker を起動する。取消した unpin が既に mounted pin map から消えていても、可視 container keys を既存 `old_folder_pin_keys` に引き継ぎ Full snapshot の無効化範囲を保持する。export／開始前の import 取消／rating のみ／対象外 context は旧要求を保持する。削除と seed を単一 transaction にまとめ、cancel／error は未 commit の batch を rollback する。seed 書込失敗時は、同じ key の旧動画 bytes を再表示させない既存仕様に従って、失敗した replacement seed の stale 行を worker で削除する。この cleanup も catalog writer 境界で cancel を検査し、取消済み旧 owner が後続 seed を消せないようにする。UI 側の `Prepared` は reset index、準備済み cache map、scalar identity だけを持ち、DB handle／FS path／I/O callback を含まない。UI は items generation と resource／policy identity を検証し、map と pin map を採用して thumbnail のメモリ状態・既存 worker lifecycle を更新する。旧 thumbnail producer が保持する map は新たな採用 map と分離する。modal／新 pending phase／待機を追加せず既存 worker と typed terminal を再利用する。

回帰は実際の pin worker terminal を UI 適用前に取得し、cascade による動画 seed と stale catalog DELETE が既に完了し、元の live map と thumbnail は不変であることを確認する。準備後に source を削除し UI DB reader を外しても準備済み bytes と位置／history が採用される。terminal DTO は全 field を型付きで exhaustively destructure し、DB handle の追加を検出する。同世代でも cache owner が変わった完了は拒否する。無関係な folder dirty は map／pool token を保持する。実際の ordinary visible adoption と prepared aggregate adoption を通し、旧 context の pin token が取消される一方、parked sibling／単なる mount／restore ではその要求と token が保持されることも検査する。既存の metadata import／兄弟 context fixture も同じ worker 準備済み結果を使う。catalog batch は seed 失敗時の削除 rollback と cancel 済み旧 owner の書込拒否を検査する。実 synthetic worker で、動画 frame bytes だけを変えて key が不変であることを確認し、INSERT のみ失敗する SQLite trigger を使って旧 seed の worker-side purge と UI 採用後の旧 bytes 非再表示を検査する。

`target/release/` は存在しない。launcher build script が必要とする `mimageviewer-core.exe`、`mimageviewer-remote.exe`、`mimageviewer-epub-pdf.exe` の3入力がすべてないため、利用者の条件に従い `scripts/test-full.ps1` は未実行。これらを作るための release build は行わない。下記 full lib 等は別途実行する。

### fix2 最終検証

最終 source 6ファイルを `target/pin-reload-fix2-source-hash.txt` の SHA-256 で固定して検証する。途中 checkpoint の full lib は 9912／9913件通過したが、seed 失敗時 cleanup と世代切替時 cancel を追加したため、最終結果としては再検証分だけを採用する。

最終世代取消差分の最初の full lib で、今回未変更の `content_identity::tests::an_origin_whose_edits_were_all_removed_stops_being_a_restore_source` が台帳 flag の検査 (`src/content_identity.rs:3707`) で1件失敗した（9913 pass／1 fail／51 ignored、603.03秒、`target/pin-reload-fix2-lib-content-identity-failure.log`）。同 test の `--exact` 単独再実行は exit 0（`pin-reload-fix2-content-identity-rerun.log`）。`content_identity.rs` は起点から未変更 (`git diff --quiet` exit 0)。source を変えない2回目の通常並列 full lib でも同じ検査が失敗した（9913 pass／1 fail／51 ignored、546.66秒、`pin-reload-fix2-lib-parallel-second-failure.log`）。

切り分けでは、既存の process-global `RECORD_SEQUENCE` が原因のテスト間干渉を確認した。`drop_sources_with_nothing_to_restore_with_probe` は探索中に別の記録があれば source flag を下ろさないが、復元内容のない candidate は取り除く。失敗 test はこの global sequence を直列化せず、別 test の明示 increment／recorder submit と重なると上記の結果になる。今回追加した navigation fixture は physical scan marker を持たず、identity backfill を開始しない。製品コードと assertion は変更せず、全 lib を `-- --test-threads=1` で検証して harness 間の干渉を除く。各回帰内で明示的に作られる worker／並列 producer の検査は維持する。個々の失敗実行の競合 producer を trace で特定したという主張はしない。

同型 producer の最終確認で metadata import の同世代 succession を追加修正するため、その途中の直列 full lib は自分の lib test harness だけを停止した（`pin-reload-fix2-lib-before-import-succession-interrupted.log`）。これは成功結果には数えない。対象要求の owner 引継ぎ・unpin coverage・rating のみ／abort／兄弟保持と実 worker の reset index の回帰を追加し、最終 source で gates を再実行する。最終検証は Windows、default features、`MSBUILDDISABLENODEREUSE=1`、起点 `0d11e5325` の未コミット差分。source は確認用 build まで固定し、以下に最終差分の結果だけを記録する。

| コマンド／検査 | 結果 | ログ (`target/` 内) |
| --- | --- | --- |
| `cargo fmt --all -- --check` | exit 0 | `pin-reload-fix2-fmt.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0 | `pin-reload-fix2-core-check.log` |
| `cargo test -p mimageviewer --lib mutation_refresh_` | exit 0、27 pass／0 fail | `pin-reload-fix2-focused.log` |
| `cargo test -p mimageviewer --lib pin_materialization` | exit 0、3 pass／0 fail | `pin-reload-fix2-materialization.log` |
| `cargo test -p mimageviewer --lib metadata_import_refresh` | exit 0、6 pass／0 fail | `pin-reload-fix2-metadata-refresh.log` |
| `cargo test -p mimageviewer --lib metadata_folder_pin_refresh` | exit 0、1 pass／0 fail | `pin-reload-fix2-metadata-pin.log` |
| `cargo test -p mimageviewer --lib folder_thumb_pins::tests::` | exit 0、55 pass／0 fail | `pin-reload-fix2-pin-db.log` |
| `cargo test -p mimageviewer --lib -- --test-threads=1` | exit 0、9915 pass／0 fail／51 ignored (728.73秒) | `pin-reload-fix2-lib.log` |
| `cargo test -p mimageviewer --test ui_snapshot` | exit 0、55 pass／0 fail | `pin-reload-fix2-ui-snapshot.log` |
| `python scripts/check_ui_glyphs.py` | exit 0、危険字形0 | `pin-reload-fix2-glyphs.log` |
| `cargo run --locked -p viewer_context_audit --quiet` | exit 1、既知6 violation／追加0 | `pin-reload-fix2-audit.log` |
| `scripts/test-full.ps1` | 必要な release 3入力不在で未実行 | `pin-reload-fix2-test-full-prerequisites.txt` |
| `build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | exit 0、Cargo jobs=4 | `pin-reload-fix2-build-dev.log` |
| SHA-256／`git diff --check` | source 6ファイル一致／exit 0 | `pin-reload-fix2-source-hash.txt`、`pin-reload-fix2-handoff-checks.txt` |

監査6件の内容は同じ A4 2件／A6 4件。該当2ファイルと audit tool は起点から未変更 (`git diff --quiet` exit 0) で、allowlist は変更していない。fix2 の実装側補助レビューは最終の import succession と unpin coverage を含め残存指摘を検出しなかったが、利用者指定の独立レビューとは別であり、その再レビューを代替しない。確認用 build は全 lib 成功後に実行し、開始時に native compiler／MSBuild がないことを確認して既存 wait override を使用した。`portable` を有効にせず、normal profile の core／remote／EPUB converter と必要 DLL を `target/dev-runtime/` に配置した。検証後から build 完了まで source 6ファイルの SHA-256 が一致する。製品の起動・UI smoke・通常プロファイルの操作は行っていない。

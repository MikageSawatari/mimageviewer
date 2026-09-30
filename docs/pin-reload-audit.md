# §1.313 現在ビューの mutation refresh 監査 (2026-10-01)

観測者は利用者 (v430-integration、2026-10-01)。Collection C → 登録実フォルダ B で代表サムネを固定すると BS が B の実親を指し、← に B の一覧が二重に出る。以下はコード調査と headless 状態遷移テストの記録であり、エージェントによる実アプリ観測ではない。

## 不変条件と修正境界

同じ可視場所の再表示は採用済み location owner が所有する。CollectionPhysical の root anchor／親 provenance、RatingPhysical の chain、navigation history の back／forward を変えない。`folder_history` は navigation history とは別のスクロール／カーソル保存領域なので、再表示時に消さない。

`current_folder_reload_owner` が現在の物理地点の typed owner を選び、F5／mutation reload と pre-scanned external reload で共有する。Collection／Rating の owner に `Refresh` intent を持たせ、変換書庫の cache path が論理 source と異なる場合にも履歴・親 chain を追加しない。通常 Path の採用境界も、現在の backing と一致する場合は既存の archive alias 解決で論理 source を履歴 identity に使い、既存の等値比較で履歴を増やさない。追加の suppress flag／collection 専用 reload 禁止分岐は設けない。

ZIP pin は既存の階層再 materialize を使い、選択・スクロール・チェック・ローカルフィルタを保持し、未完了のローカル検索を再発行する。明示 F5 の ZIP 再列挙は維持する。Smart Folder は既存の resident snapshot 契約、subfolder expansion は既存 snapshot restore を維持する。Search／Tag／Rating 等の合成行は通常 `load_folder` に渡さず、pin intent なら既存 metadata-pin worker と generation 照合付き適用で資産だけを更新する。Full intent (設定変更・ファイル操作) は既存 surface owner の再入場を使う。

folder dirty は変更コンテナ集合へ置換した。pin 更新対象はフォルダと動画の集合を一つの `Pins` intent が所有し、同一 context／generation の先行要求を統合する。フォルダ変更は自動代表・cascade のため変更コンテナの祖先タイルも失効させ、動画依存の cascade 解決は worker で行う。未変更タイルの UI 失効は避ける。要求は viewer context ごとの単一 typed resource が所有し、置換・retired context・App drop で取消、完了時には既存 egui context を起こす。共有 token を交換する適用経路は未完了動画と変更動画の producer を再開し、既に Loaded の無関係な動画は保つ。ZIP の設定対象が現在階層に見えない場合はそのコンテナの dirty だけを除去する。

合成 ZIP 本タイルの依存は同じ書庫内の兄弟参照を含むため、変更された本と同じ書庫のタイルを更新し、別書庫は保持する。Ctrl+G の動画更新は既存 streaming 候補選択（検索中は pin blob のある動画だけ）と空 sidecar map を使う。専用 search producer の token は main context だけが取消・交換し、兄弟 context の更新はその context の token／channel だけを交換する。retired 要求の取消は fullscreen 等の早期 return より前の共通 housekeeping で行う。

簡素化として既存 reload／prepared scan／metadata-pin 更新経路を再利用した。モーダル待機は不要で、短い資産 refresh のために通常操作を遮断しない。detached 固有述語・viewport 経路は変更していない。

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
| sort (RatingPhysical 子、Immediate／WorkerScan) | fix 対象・未編集 | 両経路とも通常 Navigation に落ち、RatingPhysical の親 chain と back／forward を変える。worker の `CurrentViewOrderRefresh` は現在 Collection owner しか運べない。共通 `OpenRequestOwner` への置換は detached 専用 consumer 内にも機械的転送修正を要するため、依頼の「detached-specific paths に触れるなら停止して報告」に従い未編集。利用者へ scope の確認を提示中 |
| search close／search-child sibling／parent／smart/subfolder exit | keep | `load_folder(saved/next_path/top/target/path)` は保存された地点への復帰や明示 navigation。typed restore／history suppression はそれぞれの router が所有 |
| book root／slideshow NextFolder／folder pane／JumpToPhysicalFolder | keep | `load_folder(root/next_path/ready.path/path)` は別地点を採用する意図的 navigation |
| snapshot required-fullscreen／internal navigation | keep | snapshot 範囲内の別コンテナを開く navigation。mutation refresh ではない |
| detached descriptor／prepared image-book／detached pane | keep | detached 固有の既存 navigation。依頼の変更禁止範囲 |
| `load_folder(cur/folder/current)` が残る test fixtures | keep | テストの初期配置／navigation の作成であり mutation consumer ではない |

## 検証

Collection root → physical child を実 owner／scan で採用した fixture で pin／unpin、video pin、deferred export、external rescan、stack toggle、F5 をそれぞれ通す。current target／PhysicalSource／BS collection root、back／forward、選択・スクロールが保持され、parked sibling の rows／generation／token／location／position が不変であることを検査する。合成 surface は worker 完了による repaint wake と rows／history／position の不変、ZIP は同一階層と位置・検索・check を検査する。

対象は `v430-pin-reload` の未コミット差分。実行環境は Windows、default features、dev/test profile、`MSBUILDDISABLENODEREUSE=1`。対象 source 5 ファイルの SHA-256 は `target/pin-reload-source-hash.txt` に記録した。

| コマンド | 結果 | ログ (`target/` 内) |
| --- | --- | --- |
| `cargo fmt --all -- --check` | exit 0 | `pin-reload-fmt.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0 | `pin-reload-core-check.log` |
| `cargo test -p mimageviewer --lib mutation_refresh_` | exit 0、18 pass／0 fail | `pin-reload-focused.log` |
| `cargo test -p mimageviewer --lib` | exit 0、9903 pass／0 fail／51 ignored (616.20 秒) | `pin-reload-lib.log` |
| `cargo test -p mimageviewer --test ui_snapshot` | exit 0、55 pass／0 fail | `pin-reload-ui-snapshot.log` |
| `python scripts/check_ui_glyphs.py` | exit 0、危険字形 0 | `pin-reload-glyphs.log` |
| `cargo run --locked -p viewer_context_audit --quiet` | exit 1、既知 6 violation／追加 0 | `pin-reload-audit.log` |

監査の6件は A4 (`viewer_context_ids`／`viewer_context_residence` の非 Windows API) 2件と A6 (`video/decoder.rs` の test-only 呼出し) 4件。指摘対象2ファイルと audit tool が起点 `b98dce051` から未変更であることを `git diff --quiet` (exit 0) でも確認した。allowlist を追加して隠していない。実装したピン更新差分の独立 Sol / xhigh completion review では残存指摘なし。追加のソート経路の確認では上表の RatingPhysical 不具合を両者が確認し、変更禁止境界で停止した。

回帰テストには通常・Collection の変換キャッシュ ZIP の実 preflight、Rating の parent chain、同一書庫の兄弟本依存と別書庫の保持、合成 request の統合・古い generation の拒否・retired request の取消、main search producer を兄弟 context の terminal 適用が取消しない検査を含む。

確認用 `build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` は exit 0 (`target/pin-reload-build-dev-handoff.log`)。`portable` は付けず、normal profile の core／remote／EPUB converter と必要 DLL を `target/dev-runtime/` に配置した。環境は `MSBUILDDISABLENODEREUSE=1`、最終 Cargo jobs は4。初回の turbojpeg-sys native build は CMake/MSBuild exit 1 となり、同じ worktree の依存だけを `cmake --build ... --parallel 1` で完了 (exit 0、`pin-reload-turbojpeg-build.log`)。残存 MSBuild の CPU 時間不変と compiler 非稼働を確認し、自分の待機実行だけを中断して既存 wait override を使った。プロセスを停止する運用変更や build script の変更はしていない。

上記ビルドは検証済み18-test差分の成果物であり、未編集の RatingPhysical sort 修正案は含まない。機械的転送を含む未適用の案は `target/pin-reload-sort-proposal.patch` に置いた。実施する場合は Immediate／WorkerScan の RatingPhysical 親・履歴回帰と、同じ path／order でも古い owner の完了が selection hint を変えない回帰を追加し、最終 gate と確認用ビルドを取り直す。製品バイナリの起動・UI smoke は行っていない。

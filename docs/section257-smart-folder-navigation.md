# §1.257 スマートフォルダの物理子・仮想本ナビゲーション

状態（2026-09-20）: **§1.257 本体の利用者実機確認から判明した二件を修正し、独立レビュー・全体自動テスト・確認ビルドが完了。追加分も利用者の実機確認済み**。利用者はスマート一覧の PDF open、Folder→Backspace の位置復帰、画像 Folder 内の Ctrl 上下を確認した。root 直下 PDF からの Ctrl 上下と root scan 中の中央モーダル・入力遮断が追加修正の対象である。下記の本体検証結果と旧 core hash は追加修正前のソースに対するものとして区別する。この開発確認版を配布版とは扱わない。エージェントは GUI と実データを操作していない。

## 観測と根因

利用者設定は共通 DateAsc、元実フォルダのお気に入り FileName、表示設定の記憶 ON。スマート一覧の PDF open 中に元一覧が再表示され、子 Folder から Backspace で戻ると元の選択・scroll が失われた。`target/section257-log-triage-20260920.txt` では、PDF 開始直後に同じ root 321 件が再 prepare/install され、PDF 列挙の完了前に要求が消える。Folder 復帰後も新しい root install が起きた。

旧入口は物理子を可視採用する前に Smart scope と favorite を変更した。cache-cold PDF では `current_folder` が synthetic root のまま物理 PDF の favorite sort が適用され、frame-end reconcile が共通 sort に戻して root を再 prepare した。戻りでは逆に実 Folder の favorite が残り、復元した root grid を再 prepare が上書きした。scan Err、PDF password 取消、履歴先行 pop、変換 alias と旧 PDF/ZIP receiver も同じ「新要求と旧可視 owner の混同」を持っていた。修正前 HEAD に対して PDF、Folder 戻り、root/child scan Err、password 取消の 5 handler 回帰が失敗することを確認した。

## 実装された所有境界

`SmartFolderTransition` は新しいナビゲーション要求を一つだけ所有する。source lease は viewer context、top-level surface generation、A/B slot と意味上の source identity を照合する。可視 PDF placeholder の通常検証で進む `items_generation` は lease の失効条件にしない。新しい独立ナビゲーションは入力開始時に transition を退役させるが、可視表示の refresh と detached sibling の操作は main の transition を奪わない。

root は scan→件数/確認→prepare→Ready、物理子は Folder scan、PDF warm/cold enumerate、ZIP enumerate、変換 alias の preflight→Ready を **旧可視表示の外側** で進める。Collection、検索、Folder、Smart root/Child の items・session・選択・scroll・AutoAspect・worker・既存 PDF/ZIP receiver・favorite・履歴は採用まで残る。no-resident 履歴復帰でも root を中間表示せず、offscreen `PreparedSmartFolder` を子 Ready まで保持する。PDF warm placeholder は既存の page-row 構築と列挙 handle を一度だけ使い、採用後もその handle が検証を続ける。ZIP key migration は従来どおり worker の Ready 送信前、pin batch lookup は可視採用直前に論理元 source で従来の一回を行う（既存 cascade 読取は別）。

可視採用直前に source lease、要求 ID、履歴 peek、rule/source、worker 由来の sort/display/grouping・metadata stamp を再照合する。設定の表示差は保存 snapshot から非同期再 prepare、rule/source 差は新 scan に戻す。`VisibleInstallAuthority::Smart` を共通 installer へ明示的に渡し、既存 `OpenRequestOwner` や path/pending presence から採用権限を推測しない。Folder は成功済み `ScannedDir` から直接 install し、fallible な共通 loader 前半へ再入しない。PDF/ZIP は列挙結果を一度だけ materialize する。採用時に旧 receiver と旧 fullscreen defer を同一 path でも先に退役し、その後だけ新 handle/defer を公開する。大きい旧 payload の破棄は retire worker へ渡す。通常/Remote/detached の Ordinary loader 経路は維持する。

採用済み `SmartFolderSession` は Root または Child の排他 phase、Child は元 root を `Visible` move payload か `Offscreen` prepared payload として一つだけ所有する。Root へ戻る際は synthetic 側 favorite を先に確定し、stamp 同値なら選択・scroll・AutoAspect を exact 復元、真の設定差だけ anchor 付き再 prepare する。削除 tombstone も同じ root snapshot accessor を通り、offscreen root で削除した行を復活させない。

`SmartFolderViewState` の Root、Container、Scoped は履歴位置の正本である。Container は root 直下 PDF/ZIP/変換書庫である。Scoped は現在地の `SmartChildKind` を同じ位置に保持する。別 Folder への `move_to` は Folder へ戻し、PDF/ZIP/変換書庫の採用時だけ exact kind を記録する。no-resident 履歴はこの kind を使うので、表示元 row の消失後も子フォルダ内の仮想本を Folder scan と誤認しない。変換元 `.7z` 等が履歴・favorite・address の論理 source、cache ZIP は実ロード alias のみである。

履歴 Back/Forward は pop 前に target を peek し、成功採用で一辺だけ commit、失敗・取消・別操作では stack を変えない。連打は transition 内の仮想 cursor に集約する。root-only は root 採用、Container/Scoped は子採用で確定する。検索 Return は query と検索結果の旧 owner を採用まで保持する。Remote fullscreen 復帰は exact Smart request ID の Pending/Adopted/Retired terminal を待ち、古い grid に復帰しない。

Fullscreen Ctrl の DFS と後続 Folder/PDF/ZIP/変換 preflight は typed `FolderNav` continuation が再開意図と追加 step を持つ。旧可視 PDF/ZIP の成功・失敗・password 取消はこの要求の fullscreen sequence を完了させない。採用済み Folder に ZIP/PDF しかない場合、reopen fallback の二段目 Smart request に continuation を移し、二段目採用で一度だけ reopen/chain する。Esc、別 mode、stale、取消は自身の request ID の lock/holdover だけを終了する。変換ダイアログ A と同じ source の新要求 B が競合した場合も、A の遅い取消は B を退役できない。

| terminal | 旧可視表示と履歴 |
| --- | --- |
| root/子 Ready の exact 採用 | ここで初めて旧 owner を retire し、Smart state・論理 favorite/history/source 効果を一回公開する。PDF/ZIP 明示 error grid は採用済 Child として root への戻り先を維持する。 |
| scan Err、cold PDF password/convert 取消、worker切断、stale、別nav/close、履歴逆操作 | 新要求だけを retire。旧表示・旧 receiver・履歴 peek は保つ。採用済 warm placeholder の後続検証は既存 Child の規則へ渡す。 |

## 入口と非干渉の監査

grid Enter/ゲームパッド、Folder pane/親、Backspace、Ctrl DFS、履歴、検索 Return、editor・rename refresh、grouping/sort、Remote reload、変換 cache hit/完了を新 transition または採用済可視 refresh の適切な owner に接続した。製品コードの旧 `SmartFolderOpenPhase::Opening` / Resume producer は 0。採用済 Smart root の sort 用 `smart_folder_prepare_pending` は **新ナビゲーションとは別の可視更新 owner** として残す。新要求の進行表示は transition phase/progress から導出し、旧可視 PDF/ZIP pending の表示と区別する。`src/global_search_ui.rs` は検索 Return 時に元 query を保持するため変更した。detached の述語/viewport は変更していない。

## 検証と引き渡し

### 実機確認後の追加修正（完了）

旧 root の scan/count/prepare は中央 `egui::Modal` と中止ボタンで背面入力を遮断した。offscreen 化後の `SmartFolderTransition` は同区間を左下の非モーダル badge にしか投影せず、別画像を開ける間に古い scan が継続した。新 root 要求の `RootScan` / `RootCount` / `RootConfirm` / `RootPrepare` / `RootReady` を共通 modal gate に接続し、元一覧は背面で保持する。中止は表示時の exact request ID だけを退役させ、遅い worker 完了が別画像や後続要求を上書きしない。子 Folder/PDF/ZIP/変換書庫の `ChildPreflight` は従来の非モーダル扱いを維持する。

利用者は root 直下 PDF からも Ctrl 上下で次のファイルへ移動することを求め、**Folder/PDF/ZIP/変換書庫を root 一覧の表示順に横断**する仕様を明示承認した。旧 Folder-only 横断契約はここで変更する。`PreparedSmartFolder.items` の最終表示順から `{logical_path, SmartChildKind}` の単一 `Arc` 列を作り、単体画像・動画と表示専用行は含めない。Folder 内は既存の DFS を続け、端で隣の root entry へ移る。Container は自身の exact root entry の隣へ移る。worker が捕捉した列と可視列が異なる、または削除 tombstone に該当する結果は stage 前と採用前に拒否し、現在の表示で再操作を案内する。旧 PDF placeholder の正常な page 検証による `items_generation` 進展だけでは列を無効化しない。候補 kind は worker の既存 scan facts と捕捉した root entry から渡し、UI に新しいファイル判定 I/O を加えない。sort/grouping 再準備と offscreen 復帰では列を最終 items から再構築し、位置 anchor を path と kind で照合する。

独立レビューで、Scoped DFS が空の子フォルダを見つけた後に scope を尽くすと旧 `navigate_folder_with_skip` の `Some(hit=false)` が root 横断を止める点も確認した。Grid の Ctrl は従来どおり、直下画像のない最初の root Folder に入り、次の操作でその Folder 内を DFS する。Fullscreen は一つの skip 予算で Scoped DFS と root 列を続け、root Folder 自体に再生項目がなければ前進時はその子孫を先頭から、後退時は末尾から探す。scope exhausted は次 entry へ、予算到達は従来の最寄り空 Folder fallback へ分ける。通常フォルダ用 helper は変更しない。root に Folder とその子 PDF が別行で存在するときは別の表示 entry/context として二度訪問し、root index は有限に前進する。

追加修正前のソースで中央 modal 理由と root PDF→次本の handler 回帰は失敗した。現在の差分では両方 green、実際の egui 画像セル click は root scan 中に拒否、exact 取消後に受理、遅い完了でも modal 復活なしを確認した。さらに Scoped 空子→隣 PDF は修正前 red→修正後 green、root PDF→空 Folder→隣 PDF の双方向、Grid の root-first と Fullscreen の root Folder 子探索の前後を回帰に加えた。完成差分の `cargo test -p mimageviewer --lib smart_folder -- --quiet` は **155 pass / 0 fail / 1 ignored**（`target/section257-followup-smart-focused.txt`）。modal/input 局所 filter は **2/2** と **3/3**（`target/section257-followup-modal-click.txt`、`target/section257-followup-root-modal.txt`）。独立レビューは今回の型付き列・空 Folder 境界・modal exact 取消を受理し、追加 blocker なし。

追加修正後の `scripts/test-full.ps1` は **初回 exit 0 / PASS**。workspace lib は **8742 pass / 0 fail / 45 ignored**、UI snapshot は **53/53**、vendor egui/egui-wgpu/eframe は **25/25・9/9・15/15**（`target/section257-followup-full-gate.txt`）。`cargo check -p mimageviewer --bin mimageviewer-core`、`cargo fmt --all -- --check`、`git diff --check`、UI 字形検査、`cargo run --locked -p viewer_context_audit` はすべて exit 0（`target/section257-followup-bin-check.txt`、`target/section257-followup-viewer-audit.txt`）。変更 source 13 ファイルの snapshot SHA-256 は `cdb6add6367f15f14356b013a08edad8d808b27e609abfcb3fd0e8936b1c2bc7`（従来と同じ相対 path 昇順、UTF-8 path + NUL + 8-byte little-endian 長 + raw bytes。ファイル別一覧は `target/section257-followup-source-hash.txt`）。`src/app.rs` の保護 detached block は HEAD と byte 一致、SHA-256 `46155cc6d1160b36a80396fd5c88373e49b10df8e51a8b36532dbb7aacd64331`。

ビルド前に mImageViewer の常駐プロセスがないことを read-only で確認した。`scripts/build-dev.ps1 -PreserveRuntime` は **exit 0**、core/remote と VCRT/PE check が完了（`target/section257-followup-build-dev.txt`）。`target/dev-runtime/mimageviewer-core.exe` の SHA-256 は `8c80603c4246ed2e4c0c991dc7657bb9179fd9f99b7dda6ac7bb7fc221ce8e7b`、remote は `60f4efd348c5ad5a1963bd61b7d8dd79855ca4c707b275c901324c9247793aa9`。ビルドしたアプリは起動していない。

- §1.257 本体の Smart-focused filter は **148 pass / 0 fail / 1 ignored**（`target/section257-smart-focused-final.txt`）。修正前に失敗した 5 handler 回帰のほか、旧可視 PDF/ZIP error が新 fullscreen sequence を閉じないこと、Folder→ZIP 二段目の取消・再採用、DFS 中 Esc、同 source archive A→B の exact 取消、no-resident nested ZIP/PDF/変換 alias の履歴を確認した。元選択・scroll・auto-aspect、検索・Collection・A/B・detached の局所回帰も保持した。独立レビューはこれら所有境界と履歴種別の当時の差分を受理した。追加修正で Ctrl 横断対象は変更する。
- `cargo check -p mimageviewer --bin mimageviewer-core`、`cargo fmt --all -- --check`、`git diff --check` は exit 0。`python scripts/check_ui_glyphs.py` は危険字形 0（`target/section257-bin-check-final.txt`）。`viewer_context_audit` は旧 helper の失効した A2b 例外を削除し、同じ main context の resident root を `Child.parked_root::Visible` へ move する新 owner 関数だけに理由を限定した。独立レビューでこの例外を確認し、`cargo run --locked -p viewer_context_audit` exit 0、同 crate test **35/35**（`target/section257-viewer-context-audit-final.txt`）。
- `scripts/test-full.ps1` の初回は workspace の他 target が成功した一方、`ui_snapshot` executable が 53 件中 15 件成功後に Windows `STATUS_ACCESS_VIOLATION (0xc0000005)` で異常終了し、script は exit 101 となった。画像差・assert 失敗ではない。同一ソースで `ui_snapshot --test-threads=1` **53/53**、通常並列再試行 **53/53**。script が未到達だった vendor lib test も egui **25/25**、egui-wgpu **9/9**、eframe **15/15** 成功した（`target/section257-full-gate-final.txt`、`target/section257-ui-snapshot-{serial,parallel-retry}.txt`、`target/section257-vendor-*-final.txt`）。構成 target はすべて通過したが、初回 script 自体を成功とは記録しない。異常終了の環境上の原因は未特定。
- `src/app.rs` の無関係 detached block が途中の多 hunk 編集で二度欠落したため、HEAD の対象 block だけを復元し、独立レビューで byte 一致を確認した。以後この巨大ファイルは hash 照合付き一意局所置換でのみ編集し、`target/section257-edit-audit-20260920.txt` に監査記録を残した。上の検証は復元後の全ソースを対象にした。
- resident mImageViewer プロセスがないことを確認後、`scripts/build-dev.ps1 -PreserveRuntime` exit 0。core と remote の `dev-runtime` ビルドおよび VCRT/PE check を通過した（`target/section257-build-dev-final.txt`）。変更した source 12 ファイルの snapshot SHA-256 は `74fcb4de78e82ae2f67e7bc0fa43d7062927a9fca8a15885658643092127b615`（相対 path 昇順、UTF-8 path + NUL + 8-byte little-endian 長 + raw bytes を連結して hash）。core binary `target/dev-runtime/mimageviewer-core.exe` の SHA-256 は `876a6756856ed060c2c8469f7a18c2caabf8bc0a5754006e812b088b23a1b763`、remote は `60f4efd348c5ad5a1963bd61b7d8dd79855ca4c707b275c901324c9247793aa9`。
- 利用者はスマート一覧の PDF open、Folder→Backspace の位置復帰、画像 Folder 内の Ctrl 上下を確認した。追加分の root PDF→隣の本/Folder と、root scan 中の中央 modal・操作遮断/中止は新ビルドでの実機確認待ち。PDF password 取消も未実施。エージェントは GUI を起動していない。

初期の「favorite guard のみ」、resident のみ分割、一般 ViewerContextBundle の擬似 rollback は、scan/取消/検索・Collection prior の owner を閉じないため採用しなかった。設計・失敗分析の途中資料はこの文書の前版と `target/section257-log-triage-20260920.txt` に対応する。本確認ビルドは未コミットの §1.257 ソースから作成しており、公開版ではない。

## 利用者確認（2026-09-20）

上記追加修正の確認ビルドについて、利用者から「動作大丈夫そうです」と確認を受けた。これにより今回のスマートフォルダ修正を開発完了とする。個々の取消・競合・遅着結果の網羅は記載済み自動検証の証跡であり、利用者が全ケースを個別確認したという意味ではない。過去節の実機確認待ちは、この確認前の記録である。

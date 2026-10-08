# §2.25 フォルダツリーのソート設定分離

2026-09-14。設計確認後、§1.234の完了に続けて製品実装した。§2.24の一覧サイズ順はこの分離の後に実装する。

## 範囲と不変条件

一覧の並びを変更しても左ツリーの並びが変わらないよう、`Settings`に独立した`FolderTreeSortOrder`を保存する。候補は名前・番号・日付の昇順/降順6種、欠落時の既定は名前昇順。ツリー上部の省スペースなプルダウンで選択する。サイズ順は型として表現できない。

一覧用`SortOrder`は、お気に入り表示状態、Remote listing、詳細表示、代表サムネイル選定、ZIP/PDFのページ構成等にも使われるため、ツリー専用の降順をそこへ追加しない。`FavoriteViewState`の保存・適用はツリー設定を変更しない。設定全体の保存・復元は新しい値を含む。

`FolderTreeOptions::from_settings`と左paneは同じツリー比較器を使う。Ctrl+上下のDFS、Ctrl+PgUp/PgDownの隣接移動、smart-folder内のDFSにも適用する。左paneは実ディレクトリのみ、DFSは本も扱うため、対象項目が同じという意味ではなく、同じ比較規則を使う。

UIスレッドに列挙・metadata照会・待機を追加しない。ソート変更自体はフォルダを開く操作を発生させず、フォーカスも奪わない。

## 再列挙の所有

現実装はsort/hidden変更時にnodesと展開状態を全消去する。展開キーのclearだけを除くと、子が未ロードのまま展開済みになるため、変更受付から結果反映までを同じpane ownerで扱う。

- `FolderPaneListingOptions`（tree sort、hidden）をpaneの正本とする。drive/reload/expand/key各経路はそのoptionを使う。
- sort/hidden変更では全旧pendingをcancelし、private Receiverごとdropしてから新scanを作る。channel identityをrequestの所有境界とし、重複する世代counterは追加しない。
- 保持するnodesではcancel対象keyの`loading`を解除する。`loaded=true`でも再列挙できるforce-refresh入口から、選択rootと既にmaterializeされた展開parentを新optionsの既存workerへ載せる。
- 各parentの現在のchildren、3種の展開集合、cursor pathを、新しい結果が来るまで保持する。全treeを一括でstagingする必要はない。
- 現世代の結果だけをpath identityで差し替える。sort変更だけでcursorをactiveへ戻さない。再列挙失敗時は旧childrenとerrorを保持する。
- 現世代の成功結果でcursorの消失が確定した場合だけactive/rootへ戻す。これに伴うフォルダopenは発生させない。
- 明示reloadの従来のresetと、option変更時の状態保持refreshは理由を型で区別する。

## 検収

6候補と同値tie-break、設定欠落の既定/DB往復、Favorite/list sortからの独立、DFS/sibling、paneの途中rows/展開/cursor維持、A→B→Cのstale完了拒否、hidden/drive/reload/arrowの全経路を回帰する。スナップショットで最小pane幅のプルダウンを確認する。

実装者が前提をコードで再確認し、矛盾・範囲拡大は実装前に戻す。通常判断は範囲内で進める。独立レビューは所有・世代・設定のconsumer境界を重点とする。Cargo・ビルド・検証は実装担当に集約し、実機は別途承認されたsuiteに限る。

## 設計レビュー記録

Sol/xhigh独立担当がread-onlyで既存consumerとpaneのcancel/refresh経路を照合。上記方針に重大な矛盾なし。追加照合で、既存のscanごとのprivate Receiverをdropすれば旧完了が混入せず、numeric generationは不要と確認した。旧nodesを保持するためのloading解除とforce-refreshは必要。この時点では実装・自動テスト・実機確認は未実施だった。

## 実装記録

- `Settings`へ`FolderTreeSortOrder`を追加し、名前・番号・日付の昇順/降順をツリー専用の比較器へ集約した。`FolderTreeOptions::from_settings`、左pane、通常DFS、兄弟移動、smart-folder内DFSはこの値を使い、一覧用`SortOrder`と`FavoriteViewState`は変更しない。
- paneは`FolderPaneListingOptions`を所有する。option変更では古いprivate Receiverをcancelしてdropし、cancel対象nodeのloadingを解除する。既存node、children、手動/自動展開、明示collapse、cursorを保ったまま、選択rootとmaterialize済みの展開parentを新optionで再列挙する。
- option変更の結果は`RefreshPreservingChildren`として扱い、成功時だけ同pathのchildrenを置換する。失敗・channel切断・worker生成失敗では旧childrenを残してerrorを表示する。成功がcursorの祖先直下からcursorを含む枝が消えたことを証明した場合だけ、可視activeまたはrootへcursorを戻す。明示reloadは`Populate`の前に従来どおり全node/展開をresetする。
- 各nodeは最後に成功または失敗まで解決した`FolderPaneListingOptions`を保持する。option変更時に折りたたまれていたloaded nodeは旧childrenを保持し、再展開時に条件不一致を検出して`RefreshPreservingChildren`へ載せる。失敗もそのoptionの終端結果として記録するため、旧childrenとerrorを表示したまま自動retry loopには入らない。
- ツリー上部に短縮表示の6択プルダウンを置いた。変更時は現在フォルダを開き直さず、pane同期と設定保存だけを行う。180ptの最小pane幅でdrive、sort、reloadが同じ行に収まることをactual egui snapshotで確認した。
- sortのauthoritative変更点では、旧`FolderTreeOptions`をcapture済みのDFS/兄弟/smart-folder navigationと累積stepだけをcancelする。遅着結果のReceiverは失われるため新しい順序の後に旧targetを開かない。path exactな`folder_pane_open_pending`は順序非依存なので維持する。

## 検証記録

製品freezeに対して次を実行した。

- `cargo test -p mimageviewer --lib folder_tree::tests`：44 passed。
- `cargo test -p mimageviewer --lib folder_pane -- --nocapture`：22 passed。collapsed loaded branchの再展開、refresh失敗後の非retry、旧order result拒否とexact pane open維持を含む。
- `folder_tree_sort_defaults_roundtrips...`、`smart_folder_ctrl_nav_stays...`：各1 passed。
- `cargo check -p mimageviewer --bin mimageviewer-core`：exit 0。
- `cargo fmt --all -- --check`、`python scripts/check_ui_glyphs.py`、通常viewer-context audit、`git diff --check`：すべてexit 0。auditは0 violation。
- 最初の`./scripts/test-full.ps1`はmain 8415 passed / 6 failed / 45 ignored、exit 101だった。失敗6件は§1.234でAI Completed公開がsettings-family read leaseを取得するようになった後、AI test fixtureが`DataDirOverrideGuard`のprocess-global直列ownerへ参加しておらず、並列のrestore testが保持するQuiesced phaseを観測したtest isolation不備だった。製品経路は変更せず、fixture 13件と手組み1件を既存guardへ参加させ、terminal観測後にsession operation完了まで待ってからguardを解放するよう補正した。補正後の`remote_ipc::ai_job::tests`は14 passed。
- 補正後の`./scripts/test-full.ps1`：main 8421 passed / 0 failed / 45 ignored、UI snapshot 50 passed、workspace integration・doc test・vendor egui 25・egui-wgpu 9・eframe 15を含め全PASS、exit 0。
- residentの`mimageviewer` / `mimageviewer-core` / `mimageviewer-remote`が0件であることを確認後、`./scripts/build-dev.ps1 -PreserveRuntime`：exit 0。agentはアプリを起動・停止していない。

完全なstdout/stderr/exit、source freeze、binary hashは`target/section225-folder-tree-sort-20260914/`に保存した。初回失敗は`test-full-initial.*`、採用した最終結果は`test-full.*`として分離している。

Sol/xhigh独立reviewは、typed 6種設定、pane/DFS/sibling/smart-folderの共有比較器、list/Favorite/Remoteの独立、private Receiverによるstale拒否、collapsed loaded branchのoption ownership、旧order navigation cancel、path-exact pane open維持を照合した。2件のblocking指摘は上記node ownershipとauthoritative cancelで解消し、最終gateへ進行可能との判定を得た。UIはheadless snapshotまで確認し、GUI実機確認は実施していない。

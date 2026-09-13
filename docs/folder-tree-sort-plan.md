# §2.25 フォルダツリーのソート設定分離

2026-09-14。設計確認済み、製品実装は§1.234の検収後。§2.24の一覧サイズ順はこの分離の後に実装する。

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

Sol/xhigh独立担当がread-onlyで既存consumerとpaneのcancel/refresh経路を照合。上記方針に重大な矛盾なし。追加照合で、既存のscanごとのprivate Receiverをdropすれば旧完了が混入せず、numeric generationは不要と確認した。旧nodesを保持するためのloading解除とforce-refreshは必要。実装・自動テスト・実機確認はまだ未実施。

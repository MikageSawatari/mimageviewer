# コンテナ索引の起動処理改善

2026-09-11。利用者がログ調査後に修正を依頼。実装前の設計・検証記録。

## 観測と範囲

同じ確認buildの再起動で、類似索引は42.157秒、indexed=0、containers_completed=0、画像本最大4.192秒（前回39.492秒）。1,280ページの再登録は消えた。コンテナ索引はE:/share/18で107.865秒（29,177folders/33,692entries）、D:/home/18で55.828秒。アイテム索引は最長24.197秒、同jobのingest=0でwalkerが全時間。対象・仕事量が異なるため単純な索引間倍率とはしない。

証跡は `target/similar-index-startup-delta-optimization-20260911/restart-and-shared-index/`。調査時点で通常app/DBへの変更なし。

利用者の追加指示「小さな修正でできそうな範囲まで」を優先し、今回はコンテナ索引のDB検索範囲の修正を中心とする。全索引共通walker・常駐全件cache・並列度増加は含めない。二重列挙の解消は走査/進捗/失敗処理の組み替えを伴うなら今回保留。アイテム索引も調査に基づく安全で小さな変更がない限り触らない。

## 実装前提の確認

1. `name_bulk_indexer::run_bulk_name_index` は再帰で全folderを列挙し、その後各folderを再度read_dirする。単一観測への変更は後続候補として記録し、今回のSQL修正と混ぜない。共通folder_treeの他consumerを変更しない。
2. `SearchIndexDb::upsert_children` のDELETEはfavorite_rootと `path LIKE ? || '%'` で絞る。prefixをSQLで連結するとpath範囲indexを使わず、同favoriteの行をfolder数だけ走査する可能性がある。productionと同じSQL・schemaを使ったEXPLAIN/固定scope増NのVMで先に確認する。
3. path内の `%` / `_` をパターン扱いせず、正規化済みliteral prefixの範囲を既存複合PKで引き、直下判定を維持する案を優先する。移行や新indexが必要かは計画確認後に決める。
4. 無変更書込みを単純にskipすると、updated_atによる未観測prune契約が壊れる。今回はDB検索範囲だけを直し、書込みskipは根拠なく追加しない。

独立Sol/xhigh設計レビューはSQL-onlyの範囲を承認。正規化parent末尾の`/`をlowerとし、その最後のASCII `/`を`0`へ進めたupper未満をbindする。DELETE/INSERTのtransaction・空親の置換・stamp位置は変えない。共通upsert_childrenを使うbulkとwatch親/subtreeにも同じ修正が届く。範囲内の深い子は直下判定のため走査が残り、常に直下件数だけの処理になるとはしない。

## 維持する不変条件

- 完全観測した親の直下だけを置換する。空folderの旧行削除、親ごと消えたsubtreeの掃除、深い子の保持、入れ子favoriteの独立性を維持する。
- read_dir/entry/file_typeエラーや取消で不完全な親は置換しない。不完全scanではfinal pruneしない。既知の旧不完全観測欠陥を高速化で継承しない。
- ActivityGate・取消・除外root・internal entry・AppleDouble・reparse/循環・depth上限を確認し、UI応答性を維持する。進捗は未確定の総数を確定値として表示しない。
- scan stampとwatch更新の新旧判定を維持する。変更したDB共通APIはwatchのsubtree/親更新、clear/取消の同等経路も検証する。
- 新しいSQLはドライブ直下、区切り、Unicode、似たprefix、`%`/`_`を回帰対象とし、全favorite再走査がないことを測る。scope内subtreeの走査が残る場合は明記する。

## 担当・検証

source実装とテストはSol/xhighのimplement_resume、独立レビューは別Sol/xhighのreview_resume、rootは設計・文書・親タスク調整。source同時編集なし。実装前に根拠を確認し、矛盾時は設計へ戻す。Cargoは親タスクの枠返却後に実行。

焦点回帰・production SQL性能証拠・独立レビューの後に全体gate、確認buildを行う。起動中appは停止せずbuild-dev -PreserveRuntimeを用い、通常appの手動確認は利用者が行う。現時点で所要時間の短縮幅は未測定。

## SQL前提確認

Python SQLite3.49.1、productionと同じschema/DELETEの合成fixture。対象直下K8、同favorite範囲外N4,096→32,768で旧VM steps24,839→196,871、新238→238。旧planはfavorite_rootのみ、新planは既存複合PKのfavorite_root+path両端range。両方の対象8件削除・深い子保持・rollbackを確認。`100%_set`の旧LIKEは別parent `100xxset`を誤削除し、新範囲は保持した。

証跡 `target/container-index-startup-optimization/sql-preflight.{py,json}` をrootも照合。これは外部SQLiteによる前提確認で、アプリbundled SQLiteのRust回帰・最終gateはこれから。実機108秒のうちこのSQLが占めた時間は未計測で、短縮倍率を外挿しない。実装は共有upsert_childrenのDELETE一点と関連回帰に限定する。

## 実装レビュー

r1 precompileでrootが上限生成の `debug_assert_eq!(upper.pop(), ...)` を指摘。debug assertion無効時にはpop自体が消え、製品版でupperが誤るためP1としてfreezeを解除した。独立reviewerも確認し、その他のSQL/transaction/stamp契約にblockingなし。副作用を通常文へ分離し、debug assertion無効のhelper検証を追加してr2へ固定する。r1は未compile・未承認で、利用者用buildへは載せていない。

r2は `strip_suffix('/').expect(...).to_owned()` で通常文として上限を生成。production helperと同じbodyを用いたstandalone rustc `-C debug-assertions=no` の1件が成功し、通常parent/drive root境界を確認。独立レビューは限定差分とこの証拠を承認、P1/P2なし。workspace Cargoは親の統合gate/buildが使用中のため未実行で、焦点DB・search/name watcher回帰の成功を最終条件とする。

`target/container-index-startup-optimization/freeze-r2/MANIFEST.txt` SHA256 `CA518FADC8FB1E589BC32ADE8157F925A4F93F1F629A17A167F5A3A9E5D1BDB1`、source `37564320802EF05BCFC773CB621D5ABF42E0537A3AF816F4145DB1A2AACDA253`。新schema、singlepass、item索引、他の削除SQLは変更しない。

## 焦点検証

17:16 JST、r2 source不変でsearch_index_db 30 passed、name_index_supervisor 7 passed、search_name_e2e 15 passed、全失敗0。core check14.14秒、fmt/diff/glyphもexit0。アプリbundled SQLiteのexact production SQL plan/VM回帰もDB集合に含む。rootが原本とhash一致を照合した。

証跡 `target/container-index-startup-optimization/validation-r2/RESULTS.txt` SHA256 `184C68B44AC45B55E65348570A18C22F23EBF9107B791B5D3441A8D2DBBD0936`。全体gateと確認buildはまだ。親master統合gateの§209経路による失敗は本ブランチに含まない別source条件であり、このSQL修正の失敗として扱わない。最終gate/buildの統合可否を親担当と調整する。

独立reviewerは実行証拠も確認しP1/P2なしで承認。`freeze-r2/review-approval.md` SHA256 `DD4406CA2553787EFF5337F71E4D29B85C2339FD7D2B4E5462EAD12CD3B35517`。親担当と合意し、§209修正を混ぜず、本ブランチの検証済み基盤で今回SQL修正の全体gate/buildを独立実行する。

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

## 全体gate

実装commit `2b03c73b6`。17:18–17:29 JSTの `test-full.ps1 -SuppressCrashDialogs` はexit0/PASS、timeoutなし。本体8,198 passed / 45 ignored（360.38秒）、UI48（5.66秒）、vendor egui25 / egui-wgpu9 / eframe15を含め全段階成功。開始時12e9b8008＋差分とcommit後のsource bytesは同じ。rootも原本を照合した。

証跡 `target/container-index-startup-optimization/fullgate-r2/`。stdout SHA256 `200A053E1AB903D1A14B2985C285864A10155D7D8DED6CDAA40F49110A0FEECE`、stderr `DF97C616B43C6EA18F7584C8C15F77411B0EDFFCA59D96F71BFDDC3F6B5E4696`。利用者がmIV終了を連絡し、担当が対象dev-runtime core/remote0を確認後にbuild-dev -PreserveRuntimeへ進む。実機のコンテナ索引時間は引き続き未測定。

## 確認build

`build-dev.ps1 -PreserveRuntime` normal featureは17:32:44 JSTにexit0で完了。前後の対象core/remote0、終了時Cargo/rustc/link0。通常app起動停止・実DB変更は行っていない。sourceは2b03c73b6/freezeと一致する。

`target/dev-runtime/mimageviewer-core.exe` は310,145,536 bytes、17:32:42 JST、SHA256 `1C6BB8F01CCECF8B391F71241A72F15B847B60791B1EE66953D2447A2DD802ED`。Remoteは変更なし、SHA256 `4456A614C8D40E9DF08C3BEACF208ACA86743EAB812D2CEE3B02CE0C33A2C3AA`。rootが実file hashを照合。証跡 `target/container-index-startup-optimization/build-r2/RESULTS.txt` SHA256 `9718D05D832647B64A5BB7D4C34FE61A5DCFD0F5A2FE35187671FB03E9208D0E`。

利用者には常駐版を終了して同coreを手動起動し、通常の `%APPDATA%/mimageviewer` を使用・更新することを伝える。コンテナ索引の起動完了時間を前回E:107.865秒/D:55.828秒と比較し、watch変更後の検索反映も確認する。二重列挙・アイテム索引・並列度・DB形式は変更していない。CPU/IOの削減根拠は合成SQLで確認済みだが、実機秒数の改善は利用者の次回起動ログ待ち。

## 実機起動確認

17:37起動sessionで利用者が「かなり速くなっている」と報告。rootがログを読み取り、E:/share/18は107.865→15.415秒、D:/home/18は55.828→9.941秒を確認。folder/entry数はそれぞれ29,177/33,692、12,219/13,244で前回と同じ。全name scanはcancelled=false、対象ログにname_bulkのerrorなし。

アイテム索引の最長jobは24.197→23.077秒、類似は42.157→43.052秒（indexed=0、removed=0、containers_completed=0）。類似のdecode failures142は前回同数、同一対象とは未照合。通常索引のinitial_scan_settledは起動t24.071、類似完了はt44.047。並列のため各索引時間を足さない。

キャッシュを統制したA/Bではないが、対象件数の一致と合成SQLのN比例排除の証拠があり、今回小修正の実機改善を支持する。watch変更後の検索反映はこのsessionでは未検証。証跡 `target/container-index-startup-optimization/live-verification/events.txt` SHA256 `9B5C31C6C0CAD37F8D798312F2E178EA226B0A2256F03B2048A4742F81FB0E71`。アプリ起動停止、DB操作、Cargoなし。

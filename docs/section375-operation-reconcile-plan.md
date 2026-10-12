# §1.375 自分のファイル操作の反映とフォルダ監視の照合

2026-10-12、`next-list` / `ea060b66e` 基点。**設計レビュー待ち・未実装**。
利用者の実機報告をソースで調査した。アプリは起動していない。
正本項目: [next-release-backlog.md §1.375](next-release-backlog.md#1375-フォルダで-delete-キーで削除すると正しく消えた一覧がもう一度丸ごと読み直されてちらつく--利用者の実機報告-2026-10-12)。

## 1. 原因の確定 (コード上)

`handle_delete_key` (ui_dialogs/context_menu.rs:2246) → `request_delete_confirm` →
`start_delete_files` (app.rs:46555) → delete worker → `DeleteMsg::Batch` / `Done` →
`poll_delete_pending` (app.rs:46610) → 成功 path だけ `remove_items_batch`。
ここまでは正しい削除後一覧を投影する。

しかし末尾 (app.rs:46738–46754) は UI で directory metadata を取り、
`current_folder_last_mtime` を進め、**`current_folder_signature = None`** にする。
mtime を進めるのは `Resumed` の安いふるいにしか効かない。

監視経路は `start_current_folder_watch` (app.rs:24990 付近、non-recursive notify)
→ `poll_current_folder_watch` (700 ms の既存 debounce)
→ `check_external_folder_changes(Notified)` → `start_external_rescan` の worker
→ `poll_external_rescan` → `apply_external_rescan` (app.rs:25244)。
`Notified` はファイル上書きも拾うため directory mtime の一致で止めない。
削除が完了し pending が消えたあと、この経路は signature の `same_listing` 比較へ入る。
旧 signature が None なので一致分岐を通れず、`load_folder_with_scan_owned` が全再読込する。
一覧の更新と「その更新を既に反映した」という比較基点が分離したのが根因。
利用者の報告と一致する実行可能経路であり、当該実機のログによる時系列証明は未取得。

削除進行中の guard と、rescan 中に通知が来たら `restart_requested` で再走査する契約は
必要である。guard を消す、時間窓で通知を捨てる、mtime をさらに上書きする案は採らない。

## 2. 削除／同型経路の棚卸し

| 場所／操作 | 現行 producer → consumer、扱い |
| --- | --- |
| 通常フォルダ Delete／mIV メニュー削除 | 同じ確認 → delete worker → 成功 path 除去 → signature 無効化。今回の直接対象 |
| ごみ箱／完全削除 | 独立した完全削除 Action はない。worker は `IFileOperation` の recycle / nuke warning を使い、最終判断は Shell。成功報告後の UI 経路は共通 |
| ZIP / PDF 本体 | 実ファイルとして上記削除。中身の cache／password／metadata purge も既存 worker が担当 |
| ZipImage / ZipDir / PdfPage | `drag_source_path` がなく理由通知して拒否。書庫内部を書き換える削除ではない |
| 検索結果 | 実 source は同じ物理削除・path 再探索で除去。検索 active 中は現在フォルダ watch 再読込が抑止される。検索索引側の watch は別 ownerで、通知を消してはいけない |
| Collection root の Delete | `collection_root_delete_resolution` → `request_collection_grid_remove`。参照解除でありファイル削除ではない |
| Collection 子／物理削除メニュー | 実 source を削除し `invalidate_collection_grid_sources`。Collection actor の revision／再投影は別の正当な更新 |
| 改名 | rename_item.rs:158 `poll_rename_pending` が移行・選択ヒント・`pending_reload` を設定。watch rescan との二重開始候補。UI 除去だけで済む経路ではない |
| 固定整理先へ move / copy | file_organize.rs の submit → shell_file_ops worker → poll 終端 → `refresh_file_organize_affected_folder`。現在の元／先に external check を提出する |
| 外から D&D copy | app.rs:46832 付近 → `copy_paths_into_folder` (PowerShell worker) → `poll_file_drop_pending` が `pending_reload`。watch との二重開始候補。CopyOutcome は件数中心で全出力 identity を持たない |
| Ctrl+C / X → Ctrl+V、Shell 背景 Paste | clipboard / Shell verb が owner。衝突後の出力名を mIV は所有しない。watch で初めて反映する。既に反映済み扱いは禁止 |
| native Shell メニュー、外向き D&D | OS／外部 owner。mIV worker 成功 receipt がない。外部変更として扱う |
| 製本フォルダ | `current_favorite_target` が本棚配下を監視対象から除外。本追加の `pending_reload` 等の明示更新が担当。同じ watch ちらつき原因ではない |

ZIP/PDF 内、合成検索、Collection root を通常物理一覧の receipt 比較へ入れない。
ピン、sidecar 音声画像、非表示候補の影響も scan の projection に含める。

## 3. 操作 receipt と適用済み一覧の単一 owner

単純化を先に検討: 削除は既存進捗 modal を維持し、その終端一箇所で一覧と基点を確定する。
全操作を新たに modal 化したり、watch を止めて再開したりする案は、外部 writer を排除できず
通常操作を不便にするため採らない。read-only reload は既存 worker を使う。

`current_folder_signature` だけの scalar では成功 path を差し引けないため、
既存 physical-list stamp を **`AppliedFolderListing`** に置き換える。owner は表示する
viewer context。folder、scan options、一覧世代、scan 由来の観測 rows、適用済み operation ID
を一つに保持する。観測 rows は画面の `items` から逆算しない (stack、同名除去、facetで欠ける)。
表示メタ用秒精度とは別に、worker 列挙の完全精度の変更 stamp を照合用に保持する。
新たな DB、永続 journal、時間 TTL は不要。

同じ owner に `observed_watch_sequence` / `reconciled_watch_sequence` を持たせる。
イベントを channel から drain した時点で前者を進め、削除 modal 中に check を見送っても
dirty を消さない。現行の `should_check` 一回だけの呼出しでは、削除中の return 後に次の通知が
来る保証はない。終端後は残った dirty から照合を開始する。古い世代の scan 拒否でも未照合
sequence は残し、同じ owner が再び表示された時に再照合する。新しい場所へ dirty を転送しない。

1. 操作受理時にプロセス内単調 `FileOperationId` を一回発行。要求と終端結果へ同じ ID を渡す。
   receipt は `{id, owner context, physical folder, baseline revision, successful exact effects}`。
   path／時刻だけを操作の印にしない。
2. delete worker の terminal は成功／失敗／取消を確定する。削除 receipt に入るのは成功分だけ。
   UI は受理した同じ baseline に成功差分を一回適用し、items と AppliedFolderListing の rows を
   同じ呼出し中に更新する。operation ID の二度目の採用は no-op。
   完了時の directory metadata を「適用済み contents」の証拠にしない。
3. watch event は一件も receipt ID で捨てない。notify のイベントには mIV 操作 ID がないため、
   **OS 通知へ勝手に ID を付けて自己由来と断定しない**。全イベントを既存 worker 照合へ渡す。
4. worker は現在の scan 全体を返す。`baseline - accepted successful deletes` と全体が一致
   した時だけ「この operation ID の効果は既に反映済み」と判定し、stamp 更新だけで終わる。
   items 世代、サムネイル、選択、checked、scroll、AI/PDF保持画像は変えない。
5. 一件でも説明できない差分があれば、その scan を既存 full load の pre_scan として採用する。
   receipt が説明した差分だけを消して残りを捨てる処理は作らない。
6. 不明／Modify／watch error は dirty として保持する。とくに秒精度・同サイズ上書きを
   従来 signature の一致だけで「自己削除反映済み」へ落とさない。削除 receipt が説明できるのは
   成功 source の消失だけ。残存ファイルへの内容変更通知は通常更新へ渡す。
   reconciliation の採用時にだけ、その要求が取り込んだ sequence まで既読にする。
   worker 実行後の通知は未照合のまま残り、次の入力／新しい通知がなくても次の照合を起動する。

外部変更を混ぜた worker の事後 snapshot を無条件で新 baseline にする案も採らない。
外部で追加された C を scan に含めて基点だけ更新すると、UI に C がないまま見逃すため。

### 外部変更と順序の証明例

| 並行変更 | 比較と結果 |
| --- | --- |
| mIV が A を削除、外部が C を追加 | expected は B、actual は B+C → full 更新 |
| A を削除、外部が B を削除／改名 | expected に B、actual は欠落／別名 → full 更新 |
| A を削除、外部が同名 A を作成 | expected に A はない。actual に A → full 更新。旧成功 path を新 A の削除命令に再使用しない |
| A を削除、外部が B を上書き | 完全精度 stamp／dirty 内容通知 → full 更新。旧秒精度 hash のみでは合格にしない |
| 外部変更が scan 読取後に発生 | 通知の watch sequence が走査開始時より進む → 既存 restart 契約を維持、再照合 |
| 通知が Done より先／あと／重複 | Done 前は既存削除 modal gate、終端後に最新 baseline と照合。receipt は一回採用、通知は回数に関係なく捨てない |
| 一部失敗／取消 | 成功分だけ baseline から除去、残りは維持。外部差分は通常採用 |
| 走査中 A/B切替・同 path 再入場 | context + baseline revision + scan options + generation が違えば採用しない。旧 receipt を新一覧へ転用しない |

索引 watcher、Collection actor、兄弟 viewer はそれぞれ自分の owner で変更を受ける。
一つの一覧で「反映済み」となっても、プロセス全体の mutation invalidation を握りつぶさない。

## 4. 改名・move・copy の適用方針と段階

delete は exact 成功 source を既に持つので第一段で上記を実装できる。
改名／コピー／移動は事前 scan だけで新 row・実際の衝突名・sidecar／代表画像を正しく作れない。
第一段では既存 prepare worker の一回の full 更新を保持し、更新要求を同じ ID にまとめる。
通知が早ければ watch prepare に操作終端を引き継ぎ、終端が早ければその prepare に watch dirty を
引き継ぐ。どちらも結果採用直前の全 scan と watch sequence で確認し、通知そのものは捨てない。
採用済み snapshot に一致した二回目の候補は再 install しない。

成功 destination を得られない Shell paste／native 操作は receipt 未所有の通常外部更新を維持。
PowerShell copy と ShellTransferOutcome の成功出力を拡張して incremental 投影まで行うのは
別のレビュー対象とし、今回その出力を「既に反映済み」と偽らない。

設計変更範囲予定: `app.rs`、`app/folder_scan.rs`、`delete_worker.rs`、
`app/viewer_context_registry.rs`、`app/tests.rs`。操作要求の共通 ID 配管は
`ui_dialogs/rename_item.rs` / `file_organize.rs` / `shell_file_ops.rs` と D&D終端を別段に分ける。
各共有ファイルは一人の実装者が所有する。設計レビューで第一段と後続段の受入範囲を固定する。
まれな失敗は割り切り (ログ + 通知 [+ 作り直し])。新しい retry／復旧 journal は作らない。

## 5. 受入試験と記録

予定: 上表の順序ごとの reducer tests、`poll_delete_pending`→実 `apply_external_rescan` の
handler 回帰 (full load／世代／thumbnail の変更回数を検査)、完全削除と recycle の fake sink、
partial cancel、stack・facet・同名 sidecar、全件削除、複数 context、検索／Collection を追加する。
改名／D&D はイベント先着／終端先着、外部変更混在、Shell 衝突名を別に検証する。
単に最終一覧が同じという試験だけではちらつきを検知できない。

検証所有者は各段の実装担当。targeted、fmt、通常／portable core check、項目末尾 full gate 一回。
今回は設計のみ: テスト追加 0 件、実行 0 件、build 0 件。アプリ起動・コミットなし。

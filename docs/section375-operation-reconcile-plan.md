# §1.375 自分のファイル操作の反映とフォルダ監視の照合

2026-10-12、`next-list` / `ea060b66e` 基点。**設計レビュー待ち・未実装**。
利用者の実機報告をソースで調査した。アプリは起動していない。
正本項目: [next-release-backlog.md §1.375](next-release-backlog.md#1375-フォルダで-delete-キーで削除すると正しく消えた一覧がもう一度丸ごと読み直されてちらつく--利用者の実機報告-2026-10-12)。

`001a0dabf` の独立設計レビューは **REVISE**。本改訂は表示投影、context ごとの購読、
通知 kind／path、段階の受入範囲を確定する再レビュー用。指摘は通常操作として構造で扱う。

R2 (`1c313a2ec`) は **REVISE**。実 Folder の mtime だけの差分で親一覧を再 install しない
§1.314 の契約を以下へ反映。実装は再レビュー後。

## 1. 原因の確定 (コード上)

`handle_delete_key` (ui_dialogs/context_menu.rs:2246) → `request_delete_confirm` →
`start_delete_files` (app.rs:46555) → delete worker → `DeleteMsg::Batch` / `Done` →
`poll_delete_pending` (app.rs:46610) → 成功 path だけ `remove_items_batch`。
ここでは既存表示行から削除成功項目を除く。ただし非表示候補の再露出等の再投影は行わない
(今回のレビューで判明)。削除後の全表示が常に正しいと仮定しない。

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
と、その rows の採用済み表示投影 proof を一つに保持する。
観測 rows は画面の `items` から逆算しない (stack、同名除去、facetで欠ける)。
表示メタ用秒精度とは別に、worker 列挙の完全精度の変更 stamp を照合用に保持する。
新たな DB、永続 journal、時間 TTL は不要。

### 3.1 rows と表示投影の不変条件 (P2)

「反映済み」と呼べるのは、**採用済み rows と、それを現行設定／metadata revision で投影した
表示・代表画像依存が対応している**場合だけ。raw rows の一致は必要条件であり十分条件ではない。
`ScannedDir` の同名 filter 前の folders／all_media／auxiliary candidates を immutable に保持し、
`materialize_local_folder_listing_with_order` の同名優先、並び順、画像本判定、Stack、
動画／音声 sidecar、folder pin／代表画像と依存 stamp、facet／rating を含む採用投影を記録する。
表示用 index は proof の identity に使わず、stable row key／asset dependency を使う。
設定・rating／pin等の revision が変わった候補を旧 projection と比較して合格にしない。

ただし **実 Folder の mtime だけ**は既存 `FolderScanSignature::same_listing` (§1.314) と
同じ比較除外を維持する。完全精度の観測 stamp は進めるが、表示 metadata・sort 用metadata・
代表要求 stamp は前回採用値を使って投影比較する。この例外は実 Folder に限り、ZIP/PDF等の
コンテナファイル、通常ファイル、追加・削除・改名・サイズ／種別／依存変更へ広げない。
新しい代表固定 state は作らず、既存一覧 owner の観測値と採用表示値を使う。

削除 receipt は raw rows の成功差分と「現在行から success を取り除いた暫定投影」を
同時に作る。この段階は **要照合** とし、完了した projection proof を持つ状態と区別する。
`remove_items_batch` を行っただけで Applied と扱わない。新 bool は足さず、既存一覧 owner の
typed 採用状態で `NeedsProjection` / `Applied` を表す。
watch 通知が届かなくても delete terminal 自体が再投影準備を提出する。

worker は保持 raw rows から成功差分を除いた expected と現在 actual scan を照合し、
actual の表示投影を既存 prepare 経路で作る。UI は次の二択だけを行う。

- actual rows が expected に一致し、prepared projection が暫定表示と依存まで一致:
  rows／projection proof／watch cutoff を一緒に Applied へ進める。再 install はしない。
  世代、texture、選択、checked、scroll、AI/PDF保持画像は維持。
- raw rows が異なる、非表示候補が露出する、代表／sidecar／本判定等が変わる、または後述の
  強制 dirty が残る: 同じ actual scan の prepared listing を既存採用境界へ一回渡す。
  本当に変わった表示の更新であり、削除直後の全再読込を常に省略する仕様にはしない。
  カーソル等の復元は既存 reload owner に任せる。新しい独自 partial install は作らない。

`a.jpg + a.png` の JPG 優先、Folder + ZIP、PDF + EPUB、動画 + 同名画像／音声 sidecar は
この後者になる。外部 C が混ざった actual を baseline だけに採用することも禁止。
UI で列挙・DB再照会・代表画像 decode はしない。prepare の I/O は worker に移す。

### 3.2 操作採用

1. 操作受理時にプロセス内単調 `FileOperationId` を一回発行。要求と終端結果へ同じ ID を渡す。
   receipt は `{id, owner context, physical folder, baseline revision, successful exact effects}`。
   path／時刻だけを操作の印にしない。
2. delete worker の terminal は成功／失敗／取消を確定する。削除 receipt に入るのは成功分だけ。
   UI は同じ baseline に成功差分を一回反映し、§3.1 の NeedsProjection として照合を提出する。
   operation ID の二度目の採用は no-op。表示候補の欠落を「反映済み」として残さない。
   完了時の directory metadata を「適用済み contents」の証拠にしない。
3. watch event は一件も receipt ID で捨てない。notify のイベントには mIV 操作 ID がないため、
   **OS 通知へ勝手に ID を付けて自己由来と断定しない**。全イベントを既存 worker 照合へ渡す。
4. worker は現在の scan 全体とその prepared projection を返す。§3.1 の両方の一致と
   §3.4 の通知分類を満たす時だけ「この operation ID の効果は既に反映済み」と判定する。
5. 一件でも説明できない差分があれば、その scan を既存 full load の pre_scan として採用する。
   receipt が説明した差分だけを消して残りを捨てる処理は作らない。
6. 内容変更／不明／watch error は強制 dirty として保持する。名前変更は内容変更と区別する。
   とくに秒精度・同サイズ上書きを
   従来 signature の一致だけで「自己削除反映済み」へ落とさない。削除 receipt が説明できるのは
   成功 source の消失だけ。残存ファイルへの内容変更通知は通常更新へ渡す。
   reconciliation の採用時にだけ、その要求が取り込んだ sequence まで既読にする。
   worker 実行後の通知は未照合のまま残り、次の入力／新しい通知がなくても次の照合を起動する。

### 3.3 購読 identity と context lifecycle (P2)

単純化: App-global watcher を folder 一致で A/B が共有する形はやめ、
**一つの physical-list context に一つの購読**を既存 ViewerContextBundle へ置く。
同 folder の A/B にも別購読を持たせ、通知の fanout／共有 ack／休止 context の追跡台帳を作らない。
signature、projection、watch、dirty、rescan 要求の所有先を同じ一覧 context へ揃える。
同じ directory の OS watcher が複数になる費用は、購読を共有する新 service より小さい設計を優先。
本棚／ZIP／PDF／合成 root の既存非監視仕様は変更しない。

購読 identity は `{ViewerContextId, subscription_epoch, logical_folder, listing_options}`。
同 path でも別 context／再入場／再作成した watcher では epoch を変える。
callback はこの immutable identity とその購読の dirty summary だけを更新し、App の
「今投影中の context」から配送先を再計算しない。sequence は callback が受理した順に進める。
OS watcher 登録と初期 scan は worker で、購読を開始してから scan する。
候補一覧の受理拒否／取消では候補購読を drop し、元の購読を変更しない。

| lifecycle | 契約 |
| --- | --- |
| 同じ採用一覧の更新 | 同じ subscription epoch。採用した rows／projection と、その scan の通知 cutoff を同時確定 |
| A を park、B を mount | A の watcher／dirty は A bundle に残る。B 自身の watcher を使う。同 folder という理由で A の購読を B へ付け替えない |
| park 中の通知 | callback は元購読の dirty に記録。UIへ install はせず、A の再 mount 時に未照合通知から worker を起動。directory mtime が同じでも開始 |
| scan 中の park／generation変更 | 結果は元 identity を照合。新世代へ適用せず、dirty を既読にしない。再 mount／新基点の採用後に未照合分を再検査 |
| source 切替／context破棄 | その購読を drop。古い callback／結果は identity で拒否。兄弟購読の dirty／ack／取消を変更しない |
| watcher 不成立／coverage切断 | 元一覧を保ち、次の復帰で worker の全照合を提出。mtime の安いふるいだけで無変更としない。新しい retry loop は作らない |

現行の `external_rescan_pending` も context-owned physical-list owner に移し、cancel／drop が
一つの context にだけ作用するようにする。main の現在 folder 判定だけで旧結果を採用しない。
初期 scan 中・削除 modal 中の通知は未照合で残す。mtime は購読の連続した coverage を
代替しない。callback の集約は短いメモリ操作のみ、UI に OS登録や重い待ちを追加しない。

### 3.4 通知 kind／path の保持と消費 (P2)

`saw_change` へ潰さず、購読ごとに `WatchDirtySummary` を保持する。
各対象には event sequence と分類を付け、名前変更の old/new paths (分割イベントでは片側)、
内容変更 paths、Unknown を記録する。集約上限を越えたら同じ summary を Unknown に昇格して
全照合する。時刻 TTL／通知の無視は使わない。

| notify kind | 保持と採用条件 |
| --- | --- |
| Create／Remove／`Modify(Name)` | Namespace。raw rows と投影を全照合。操作結果の後と一致すれば再 install 不要。改名を一律 Content にしない |
| `Modify(Data)`／ファイル属性・時刻等の `Modify(Metadata)` | Content/Metadata と対象 path を保持。残存 row または代表／sidecar依存への通知は stamp一致でも通常更新が必要。成功削除で消えた exact source だけなら、消失と投影一致を確認して既読にできる |
| 対象が確定した実 directory の Metadata／Modify(Any) | worker が前後 scan の typed directory と通知pathを照合。mtimeだけの変化は観測値だけを更新し、§1.314の表示値を維持。再installを強制しない。追加・削除・改名／種別変化は通常照合 |
| ファイルへの Modify(Any) | 内容変更を否定できないのでContent。stamp一致だけで更新を抑止しない |
| Other／path不明／directoryと確定できないModify(Any)／watch error／coverage不明／集約上限超過 | Unknown。raw hash一致を免除理由にせず、一度 full prepare／採用する |
| Access | 既存どおり一覧 mutation 対象外 |

`observed_watch_sequence` と `reconciled_watch_sequence` はこの同じ購読の記録。
scan 開始時に dirty と cutoff を snapshot。採用成功時だけ **cutoff 以下**の evidence を消費する。
同 path に後から内容通知が来ても先の Name とまとめて消さない (集約 record の最大 sequence と
最も強い分類を保持し、cutoffを越える record は未照合のまま)。
Nameだけの batch は照合／投影一致で既読、残存 B の Content または Unknown が混ざれば
同じ batch でも full 更新へ進む。イベントの kind/path は自己由来の証明には使わない。
更新が pending の間の新通知は未照合として残し、次のscanへ引き継ぐ。

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
**第一段の完了対象は Delete の二重 install 解消だけ**。
表示投影、購読／通知分類の基盤は第一段に含めるが、rename／move／copy の共通 operation ID
配管と二重 install の解消は第二段の完了条件とする。第一段では既存操作を維持し、第二段の
受入を済ませたと報告しない。項目全体の完了は両段の受入後。

第二段では既存 prepare worker の一回の full 更新を保持し、更新要求を同じ ID にまとめる。
通知が早ければ watch prepare に操作終端を引き継ぎ、終端が早ければその prepare に watch dirty を
引き継ぐ。どちらも結果採用直前の全 scan と watch sequence で確認し、通知そのものは捨てない。
採用済み snapshot に一致した二回目の候補は再 install しない。

成功 destination を得られない Shell paste／native 操作は receipt 未所有の通常外部更新を維持。
PowerShell copy と ShellTransferOutcome の成功出力を拡張して incremental 投影まで行うのは
別のレビュー対象とし、今回その出力を「既に反映済み」と偽らない。

設計変更範囲予定: `app.rs`、`app/folder_scan.rs`、`delete_worker.rs`、
`app/viewer_context_registry.rs`、`app/tests.rs`。操作要求の共通 ID 配管は
`ui_dialogs/rename_item.rs` / `file_organize.rs` / `shell_file_ops.rs` と D&D終端を別段に分ける。
各共有ファイルは一人の実装者が所有する。第一段では `CurrentFolderWatch` と
`ExternalRescanPending` の bundle 移設も含める。第二段は上記操作 producer の ID／終端／
採用の接続を行い、成功 destination の incremental 投影は含めない。
まれな失敗は割り切り (ログ + 通知 [+ 作り直し])。新しい retry／復旧 journal は作らない。

## 5. 受入試験と記録

予定: 上表の順序ごとの reducer tests、`poll_delete_pending`→実 `apply_external_rescan` の
handler 回帰 (full load／世代／thumbnail の変更回数を検査)、完全削除と recycle の fake sink、
partial cancel、stack・facet・同名 sidecar、全件削除、複数 context、検索／Collection を追加する。
第一段の追加回帰:

- JPG優先の `a.jpg + a.png` から JPG Delete → PNG再露出。Folder／ZIP、PDF／EPUB、
  動画／画像、音声 sidecar の依存変更。raw一致でも必要な新 projection を採用する。
- 名前／path が不変の代表差し替え、画像本判定・Stack再編、facetで隠れた代替候補。
- 同 folder の二 context: A が自己削除を反映済みにしても B の購読／dirty は残り、
  B再 mount で更新。A→B→A、park中の同サイズ上書き (directory mtime不変)、context破棄。
- Remove／Name と残存 B の Data／Metadata が同じ batch に混在、分割 rename、Unknown、
  cutoff後に同pathの内容通知、modal中の通知のみ (次の通知なし)、subscription再作成。
- §1.314既存handler回帰を維持。子フォルダ内の.part追加・削除による実Folder mtimeだけの
  `Modify(Any)`、および自己Delete通知との同batch混在で、親一覧の追加install 0回。
  観測stampのみ進み、表示metadata・sort順・代表要求stamp・textureを維持する。
  ファイルModify(Any)／対象不明／watch errorは更新される対照例も検査する。

第二段は改名／D&D／整理先move・copy のイベント先着／終端先着、外部変更混在、
Shell 衝突名を別に検証する。`Modify(Name)` だけの重複通知で二重 install しないことと、
同じbatchの B 上書きを取り込むことを同時に検査する。
単に最終一覧が同じという試験だけではちらつきを検知できない。

検証所有者は各段の実装担当。targeted、fmt、通常／portable core check、項目末尾 full gate 一回。
今回は設計のみ: テスト追加 0 件、実行 0 件、build 0 件。アプリ起動・コミットなし。

# sidecar 初回取り込みの非同期化設計

最終更新: 2026-09-11

## 1. 対象と現象

`next-release-backlog.md` §1.209 を対象にする。v3.8.0 の空の中央 DB で、既存
`mimageviewer.dat` を持つフォルダを初めて開くと UI が長時間停止する。

実データは読み取りだけで確認した。`D:\mImageViewer_portable_v3.8.0\data\logs\mimageviewer.log`
では `load_folder` 開始が 19.638 秒、sidecar import 完了が 66.435 秒で、430 field
（adjust 379、mask 40、local-adjust 7、crop 1、comic 3）の取り込みに約 46.797 秒、
同じ区間を含む slow frame は 47.1699 秒だった。別の 14 field は約 1.994 秒だった。
これは 1 field あたり約 109--142 ms で、SQLite の durable commit を field ごとに
繰り返す挙動と整合する。

根因は `App::start_loading_items_inner` が UI thread で
`App::import_sidecar_to_dbs` を同期呼び出しし、その先の `sidecar::import_to_dbs` が
各 field を既存 DB setter へ個別に渡すことにある。呼び出し位置は items と cache を
新しい folder へ切り替えた後、補正 DB の hydration と thumbnail worker 起動より前である。

守る仕様は「初めて見える画像・サムネイルにも sidecar から復旧した補正が反映済み」である。
raw を先に表示して後から補正へ切り替える変更は採らない。

## 2. 不変条件

- sidecar の read、JSON parse、mask decode、SQLite lock/transaction を UI thread で行わない。
- 中央 DB に既にある編集・タグは authoritative とし、sidecar で上書きしない。
- commit 直前の source revalidation までに sidecar source が変わったら、古い snapshot の row と
  sync marker を一切書かない。revalidation 後の外部変更は次の revision として扱う。
- edit family の main DB と 5 個の ATTACH DB は全部成功するか、全部 rollback する。
- tags は独立した family/transaction とし、edit 成功・tag 失敗などを明示して返す。
- cancel、decode error、schema error、marker error を成功扱いせず、marker を進めない。
- completion は要求を発行した viewer context と items generation だけへ適用する。
- viewer の切替・close・cancel が別 viewer の items、cache、worker を破棄しない。
- ZIP/PDF の virtual key、通常 file key、合成/remote view を同じ path 検証規則で扱う。
- pending sidecar writer の snapshot/Remove と disk snapshot を混同しない。

## 3. Stage 1: UI 未配線の取り込み engine

Stage 1 は `src/sidecar.rs`、`src/sidecar_import.rs`、`src/tags_db.rs` と focused test に
閉じる。この段階だけでは UI 停止は解消しない。

### 3.1 source snapshot

`SidecarFile::load_for_import` は forgiving な既存 `load` と分離し、次を区別する。

- `Loaded(LoadedSidecarImport)`: immutable sidecar と source identity を分離不能な owner に束縛
- `Missing`
- `Unreadable`
- `Corrupt`
- `UnsupportedVersion`
- `WriterFailed`
- `ChangedDuringRead`

Disk identity は normalized folder、byte length、mtime の nanosecond 値、SHA-256 digest を
持つ。metadata-before、bytes、metadata-after が一致した snapshot だけを返し、commit 直前に
同じ検証をもう一度行う。sync marker は length、mtime、digest を混ぜた v2 identity であり、
mtime が同じ別 content を synchronized と誤判定しない。

Pending writer は folder、process-local sequence、Write/Remove kind を持つ。Write は同じ
immutable `Arc`、Remove は同じ sequence の current Remove と空 snapshot であることを
commit 前に検証する。writer failure や writer mutex poison は strict load で disk fallback
せず terminal state にする。pending snapshot は disk marker の対象にしない。

`prepare` は `LoadedSidecarImport` を consume する。token と別の snapshot を再結合する
public API は置かない。completion は次の outer state とする。

- `Current { sidecar, result }`: UI/cache へ install できる唯一の状態
- `SourceChanged { error, result }`: stale sidecar は返さない
- `Cancelled { result }`: stale sidecar は返さない

pre-cancel は source 再読込より先に判定し、DB open と tag backup rotation も行わない。
`NotRequested` と prepare 時点の `Failed` は result に維持し、commit 可能だった `Ready` を
`Cancelled` または `SourceChanged` に写す。

### 3.2 prepare と path 検証

全 key の containment、mask base64/deflate と寸法上限、crop の有限値と source 範囲、
JSON serialization、tag normalization を DB transaction 前に済ませる。通常 file key は
folder 直下の単一 filename だけ、virtual key は単一 container filename と `::` 以下の
rooted/empty/`.`/`..` を含まない entry だけを受け付ける。一つの edit field が不正なら
valid row を含む edit family 全体を失敗させる。tag family は独立に判定する。
sidecar 自体の key/decode/domain 検証は import 設定で未要求の family も含む全 content に行う。
一つでも失敗した snapshot は、別 family の更新で原本を部分的に再構成した内容へ上書き
しないよう session 中 write-disabled の owner として返す。DB outcome は未要求なら
`NotRequested` のまま保ち、別の `source_validation_error` warning で保存抑止を明示する。
DB open/query/transaction の一時障害とは区別する。

ATTACH 対象は data directory 直下の固定された canonical 6 store だけである。sidecar key
から DB path、schema 名、table 名を作らない。

### 3.3 edit transaction

`adjustment.db` を main とし、`mask.db`、`conceal.db`、`local_adjust.db`、
`export_crop.db`、`comic.db` を固定 alias で ATTACH する。6 file/table と
`main.sidecar_sync` が揃わなければ開始しない。SQLite の multi-file atomic commit の前提を
守るため、各 store の journal mode が rollback-journal 対応であることを確認し、WAL、
MEMORY、OFF は拒否する。

一回の `BEGIN IMMEDIATE` 内で既存 row を再照合し、各 INSERT を
`ON CONFLICT DO NOTHING` とする。prepare 前または後に作られた中央 row は常に勝つ。
全 row 後に marker を同じ transaction へ書き、一回だけ COMMIT する。途中 cancel、
attached DB trigger fault、marker fault は全 store を rollback する。

### 3.4 tag transaction

tags.db は WAL のため edit multi-file transaction へ含めず、別 outcome と別 transaction を
持つ。既存の `rotate_backups_once` を経由した後に `BEGIN IMMEDIATE` を開始する。
transaction 内で `tag_item_state` と decision row のない legacy `item_tags` の両方を再照合し、
どちらかがあれば sidecar tag を入れない。tag rows、decision row、tag marker は同じ
transaction で commit する。

### 3.5 Stage 1 focused verification

実 `%APPDATA%`、既存 portable data、実 sidecar は使わず `TempDir` fixture だけを使う。
受入 test は次を含む。

- 430 mixed fields の prepare/transaction/commit/total 時間と field count
- edit transaction が構造上 1 BEGIN/1 COMMIT であること
- main marker fault と late attached store fault の全 rollback
- unsafe attached journal mode の transaction 前拒否
- mid-transaction cancel の全 rollback、pre-cancel の source I/O/DB/backup なし
- prepare 後に別 connection が作った row と legacy tag row の維持
- edit failure/tag success の family 別 partial outcome
- escaping key と invalid mask が valid row も含め family 全体を止め、marker を書かないこと
- sidecar validation failure 後の owner は保存不能を明示し、後続 edit/flush が不正な原本を
  書き換えないこと。未要求 edit + requested tag とその逆でも全fileを保護し、DB 一時障害
  だけでは source owner を write-disabled にしないこと
- Disk content change、Pending Write/Remove supersede、writer failure、mutex poison の fail closed

2026-09-11 の review 修正後 debug fixture 実測は load 12.595 ms、prepare 6.116 ms、
edit transaction 12.208 ms、commit API 17.906 ms、engine（prepare+commit）24.022 ms、
end-to-end（load+prepare+commit）36.617 ms だった。この 430 mixed fixture は edit family だけを
要求するため transaction/commit は一回で、tags を同時に要求する通常形では family ごとに最大
二回である。時間の assertion は置かず、件数と transaction/marker の不変条件だけを固定する。
この数値は UI 未配線 engine の TempDir/debug 実測であり、旧 47.1699 秒の UI frame と同条件の
比較でも、UI 停止解消の実測でもない。

## 4. Stage 2: worker と初回表示 continuation

Stage 2 で旧 `App::import_sidecar_to_dbs` の同期経路を置き換える。既存の seconds mtime
fast path と v2 marker を混在させない。App consumer を typed load/engine へ同時に切り替え、
旧 seconds 比較と無条件 marker update を削除する。

### 4.1 最小 owner

worker の owner は viewer ごとには作らず、App-global な `SidecarImportCoordinator` とする。
coordinator は normalized folder を key に in-flight job を一つだけ持ち、同じ folder を同時に
開いた context はその job を共有する。job は request id、source owner、DB write permit、cancel、
worker receiver、terminal result を所有する。別 folder の job とは区別するが、中央 DB writer
との排他は folder を越える process-global な permit で行う。

各 `ViewerContextBundle` が持つのは worker ではなく、typed subscription と load continuation
である。subscription は context id、items generation、normalized folder、request id を一体で
持つ。context swap は subscription/continuation も一緒に swap し、context drop または次の
load は自分の subscription だけを解除する。同じ job を待つ subscriber が残っていれば job を
cancel せず、最後の subscriber が外れた時だけ coordinator がその folder job を cancel する。
terminal result は一致する subscriber ごとに通知し、別 context/generation/folder の state、
cache、worker へは適用しない。

worker request は sidecar folder、data directory、edit/tag family flags を値として受け取り、
`load_for_import -> prepare -> commit` を実行する。App の DB handle や cache を worker へ渡さない。

continuation は import の直後に現在同期実行されている処理を所有する。少なくとも
`source_path`、prepared aggregate/subfolder metadata、catalog existing keys、video items、
thumbnail channel/cancel、SLI perf span を保持する。completion が同じ context、generation、
folder に一致した時だけ、次の順で UI thread に短く適用する。

1. `Current` sidecar を、同 folder の in-memory sidecar がまだ存在しない場合だけ install
2. family outcome を log/metadata refresh に反映
3. adjustment/local/crop/mask/conceal/comic を中央 DB から hydrate
4. catalog を開き、thumbnail/video worker を開始
5. selection/history/first-display の既存 tail を再開

worker 中に同 folder の sidecar が編集・作成されていれば、completion の sidecar で
置き換えない。中央 DB は transaction 内 insert-if-absent により before/during/after の
ユーザー編集を保護する。

### 4.2 表示と lifecycle

items install 後から hydration 完了までは新 generation の「初回表示準備中」である。
thumbnail producer をまだ開始せず、grid は既存の pending cell 表示を使う。ZIP/PDF finalize
など caller が `start_loading_items` 直後に fullscreen を要求しても、fullscreen renderer は
この typed pending state を見て loading surface だけを描き、raw/final texture request を
開始しない。完了して DB hydration が終わった同じ frame 以降だけ補正済み表示を許す。

次の navigation は古い pending を cancel する。transaction が既に commit 済みでも、その
completion を新 generation へ適用しない。insert-if-absent の復旧 row と正しい source marker は
中央 DB に残ってよい。cancel/error/disconnect/close は成功として continuation を適用せず、
必要なら同じ generation を DB-only で hydrate して閲覧を継続する。terminal load の
Missing は worker で marker を clear し、Unreadable/Corrupt/Unsupported/WriterFailed は
marker を進めず disabled state を維持する。

prepared aggregate、synthetic search/rating/tag view、remote-only source は sidecar import を
開始しない。通常 folder はその folder、ZIP/PDF は実 container の parent を使う。folder の
導出に UI thread の追加 `is_dir` I/Oを使わず、列挙済み item/source kind から決める。

### 4.3 Stage 2 実装前 gate

次を独立設計 review と test で確定してから App 配線を編集する。

- `ViewerContextBundle` へ追加する typed pending/continuation と全 swap/drop 箇所
- grid、embedded fullscreen、detached fullscreen の初回表示 gate が同じ ownerを見ること
- import transaction と別 viewer の adjustment/mask/tag writer が競合しても user write が
  busy 失敗せず最終的に勝つこと。Stage 1 の約 13.5 ms は debug/SSD の実測であり、slow
  disk の上限ではない。UI writer に busy timeout で待たせる設計は採らない
- import が lock を先に取る場合と UI edit が先に取る場合の両順序を deterministic test すること
- commit 直前の source revalidation 完了を disk snapshot の線形化点とし、それより前の変更は
  `SourceChanged` で row/marker とも書かず、それより後の外部変更は次の sidecar revision として
  marker mismatch にすること
- new navigation、rapid reopen、cancel、worker panic/disconnect、close、detached sibling、
  ZIP/PDF、remote/synthetic の state transition test

### 4.4 DB writer 競合の実装分割

mutation producer の棚卸しでは、次の複数 owner が同じ store を書くことを確認した。

- adjustment の単体 set/remove と selection bulk set/remove は UI thread の App wrapper
- mask、conceal、crop、comic の単体 set/remove も UI thread の App wrapper
- local-adjust と通常 tag 操作は既存の専用 worker
- page edit bundle の貼り付けと bulk 操作は別 worker で 6 store を atomic 更新
- 製本ページの copy/move は UI thread から 6 edit store と tags を順に更新
- rename migration worker は 6 edit store と tags を直接更新
- tag maintenance/metadata import は通常 tag worker の DB release/ACK state machine を既に持つ

低層の public DB mutation API は 7 module に 55 入口ある。内訳は adjustment 15
（page 6、sync marker 2、favorite params 3、favorite view state 4）、mask 6、conceal 7、
local-adjust 4、crop 4、comic 5、tags 14 である。55 個が別々の利用者操作という意味ではないが、
SQLite の write lock は table でなく DB file 単位なので、sidecar field 以外の favorite state や
tag maintenance も競合監査から除外できない。また `PreparedPageEditBundle`、metadata transfer、
rename/content-identity は public setter を通らず transaction/direct SQL を実行する。

permit を接続ごとに後付けするのではなく、値を最後まで所有する production producer 境界で数えると
最低 15 系統ある。内訳は page adjustment、favorite adjustment params、mask、conceal、crop、
comic、book page copy/move、local-adjust worker、tag write worker、tag editor maintenance、
favorite-view worker、single/bulk edit bundle、rename migration、content-identity/delete、
metadata transfer/legacy-tag seed である。旧 sidecar import はこれらと競合する 16 番目の writer
として coordinator 側へ置換する。Stage 2 の実装前に call-site audit test/list を固定し、新しい
direct writer が permit を迂回した時に review だけへ依存しない形を選ぶ。

`indexer_manager` の legacy Tantivy tag import も tags.db writer だが、viewer と worker の起動前に
完了する startup-exclusive producer である。permit 対象から外す場合も lifecycle invariant を
test/comment で固定する。`app/metadata_import_refresh` は production では DB read only であり、
同 file の tag setter は test fixture だけなので producer 数には入れない。

adjustment、mask、comic は通常 connection に busy timeout がなく、import が先に
`BEGIN IMMEDIATE` を取れば user write は即失敗し得る。conceal/crop/local-adjust/tags に
timeout があっても UI thread で待つ解決には使わない。

既存 SidecarEntry だけから競合した操作を常に lossless に復元することもできない。DB failure
時は意図的に sidecar mirror を書かない現在の契約があり、全 field 削除には entry の不在とは
別に dirty key が必要である。sidecar backup 対象外の copy/move と migration もある。

したがって Stage 2 は次の三つの coherent change に分ける。

1. **DB commit coordinator と worker producer**: process 内で一つの typed write permit を置く。sidecar import は
   transaction 前に exclusive permit を取る。既存 worker producer は permit を worker 上で
   待てる。`TagWriteJob` と release/ACK/paused FIFO、local-adjust の key 単位 coalescing queue と
   fence、`FavoriteViewStoreCommand`、`PreparedPageEditBundle` と single/bulk pending、rename/
   delete/content-identity の worker request は値を所有済みなので再利用できる。これらへ permit
   acquisition/pause を足し、import-first/user-first の両順序を deterministic test する。
2. **UI producer の lossless intent**: UI producer は try-acquire だけを行う。失敗時は import
   cancel を立て、値を所有する typed deferred intent を FIFO/coalescing owner へ渡す。page
   adjustment、favorite params、mask、conceal、crop、comic、book copy/move は現在この intent
   owner を持たないため新設が必要である。import terminal 後に通常の authoritative setter/atomic
   bundle として再適用し、user write を最後にする。全 producer が permit を通る call-site audit
   と競合 regression が green になるまでは coordinator から import を起動しない。
3. **folder job と viewer continuation**: §4.1--4.2 の folder-keyed coordinator、context subscriber、
   worker、hydration、初回表示 gate
   を配線する。coordinator が green になるまで import transaction を App から起動しない。

permit flag だけを追加する案、Busy 後に clone できない closure を再実行する案、busy timeout
だけを増やす案は採らない。いずれも user intent の所有または UI responsiveness を保証しない。

Disk source は「read した snapshot を中央 DB が空の field へ復旧する」契約であり、commit 時点の
外部 file の最新版を強制する契約ではない。commit 直前の digest revalidation 完了を線形化点に
する。その時点までの変更は `SourceChanged` として取り込まず、その直後に外部 tool が保存した
変更は次回 open で marker mismatch になる。ただし既存中央 row を sidecar で上書きしないため、
その後の sidecar revision が既取り込み row を置換しないのは従来からの中央 DB authoritative 仕様
である。

Windows の share-write/delete を拒否する handle を load から commit terminal まで保持すれば
「commit 完了まで disk path が最新版」というさらに強い freshness は得られる。しかし遅い disk、
DB permit 待ち、rollback journal の時間だけ外部 tool の保存/atomic replace を sharing violation
で拒否し、retry しない tool では利用者の保存を失敗させる。commit 直後の外部変更を中央 DB へ
再適用する問題も解かない。これは snapshot 復旧と UI responsiveness に不要な新仕様なので
Stage 2 の必須条件にしない。process 内の編集は外部変更と別であり、DB permit、lossless deferred
authoritative replay、Pending source の commit 前 revalidation、stale completion install 拒否で
必ず user edit を勝たせる。

現時点の production 対象は少なくとも 22 source file である。coordinator/source は新規 2 module
と `src/lib.rs`、`src/sidecar.rs`、`src/sidecar_import.rs`、worker/direct producer は
`src/tag_write_worker.rs`、`src/local_adjust_write_worker.rs`、`src/favorite_view_state.rs`、
`src/edit_bundle.rs`、`src/edit_bundle_app.rs`、`src/edit_bundle_bulk.rs`、
`src/rename_key_migration.rs`、`src/content_identity/restore.rs`、`src/delete_worker.rs`、
`src/metadata_transfer.rs`、`src/tag_legacy_seed_worker.rs`、`src/ui_dialogs/tag_editor.rs`、
App/continuation は `src/app.rs`、`src/app/viewer_context_registry.rs`、
`src/app/snapshot_ops.rs`、`src/ui_fullscreen.rs`、`src/app/tests.rs` が候補になる。低層 API で permit
を型として強制する設計を選ぶ場合は 7 DB module も増える。実装量は focused test と設計 review を
含め 20--35 時間（3--5 開発日）程度、最後の disposable portable 実機確認を別に 60--90 分と
見積もる。Stage 1 の小規模性能修正と同じ chunk には収めない。

Stage 2 完了後に disposable portable と合成 fixture で実機確認する。確認対象は 430 mixed
folder の初回 open が UI input/repaint を止めず、最初の thumbnail/fullscreen が補正済みで、
navigation cancel と別 viewer が破壊されないこと。通常 `%APPDATA%` と既存 portable data は
使わない。

## 5. 2026-09-11 承認済みモーダル仕様による Stage 2 の縮小

利用者は、sidecar 復旧が必要な限定的な open に限り「サイドカーから設定を復元中」と表示し、
復元対象画面の操作を完了まで止める仕様を承認した。別 viewer の独立 native video は再生・seek
などの通常操作を継続する。既存の App-global load/edit/save gate は保存競合を増やさないため維持し、
全 sibling 操作の自由化や厳密な競合順序保証までは行わない。この仕様では、§4.4 の全 mutation producer に
write permit と lossless deferred intent を実装する必要はない。§4.1 の folder-keyed job と
複数 subscriber も使わず、一つの App-global state が一つの target context を最後まで所有する。
§4 は待機中も利用者操作を許す仕様へ将来戻す場合の調査記録として残し、次版の実装範囲は本節で
置き換える。

操作を止めても、sidecar/DB I/O、writer の待機、preview cache の ACK 待ちは UI thread へ
移さない。モーダルは競合する操作を新しく発生させないための仕様であり、UI thread を block する
仕組みではない。worker の進捗待ちは毎 frame の poll と repaint deadline で進める。

### 5.1 state owner と pause 位置

`SidecarRestoreState` を App-global な一つの tagged state とし、少なくとも次の phase を持たせる。

- `Quiescing`: 既に受理済みの DB/sidecar writer を完了させ、その結果を反映する
- `Checking`: quiescence 後の sidecar flush、strict load、read-only marker probe の worker を待つ
- `InvalidatingPreview`: edit import 前の preview DB clear completion を待つ
- `Running`: strict reload、prepare、Stage 1 commit または Missing marker clear を待つ
- `CacheRefreshing`: import を再試行せず、folder-global sidecar cache owner を worker で再取得する。
  Running worker の outcome が channel disconnect で不明な場合だけ、中央 DB の edit rollup と target tag
  snapshot も同じ worker で回復する
- `Resuming`: DB hydrate と保留した load tail を一度だけ再開する

別 bool、request field、modal field を並立させず、この state を coordinator と既存の App-global
load/edit/save gate の根拠にする。modal と直接 semantic-input sanitize は projected context が target と
一致するときだけ有効にする。state は target の `ViewerContextId`、`items_generation`、normalized folder、request id、
cancel、worker receiver、load continuation、必要なら deferred fullscreen intent を一体で所有する。
completion は三つすべてが一致した target だけへ適用し、別 F12 viewer や次 generation へ移送しない。
registry 上でも同じ target が coordinator に所有された mounted load context のままであることを確認し、
retired/reused context id や sibling の at-rest state へ continuation を設置しない。

pause 位置は `start_loading_items_inner` の旧同期 `import_sidecar_to_dbs` 呼出点である。ここまでに
新 items と generation は target へ設置済みだが、6 DB の hydrate、catalog、thumbnail/video worker、
selection/history と初回表示はまだ始まっていない。この位置で state を立ててから caller へ戻り、
旧呼出より後ろの処理を typed continuation へ移す。関数入口で folder load 全体を分割して多数の
caller に別の ownership を作らない。

現 `SidecarWriter::enqueue` は channel send/spawn failure 時に呼出 thread で `write_one` へ同期 fallback
するため、復旧候補の open では UI thread から `flush_all_sidecars()` を呼ばない。state を立てた直後は
`SidecarFile` owner を App cache に残したまま `Quiescing` へ入り、先行する DB completion と sidecar mirror
をすべて通常 owner へ適用する。開始時に cache を `mem::take` して pre-probe すると、late mirror が作った
新 owner を古い retained owner で上書きして保存値を失うため禁止する。

quiescence 完了後に一度だけ、全 cache owner を App に残したまま items の immutable `Arc` を予約し、dirty
writable snapshot だけを `Checking` worker batch へ渡す。worker は queue、send-failure fallback、
process-global strict idle wait、strict load/probe を順に行う。予約が生存中の sidecar mutation は
`Arc::make_mut` で別 identity になるため、worker 成功時に dirty を解除できるのは folder、items `Arc`、
writable state が capture 時点と exact に一致する owner だけである。spawn/write/batch identity failure は
App cache を一切変更せず DB import を始めない。これにより sibling の sidecar-backed completion/edit が
owner 不在による UI 同期 read や二重 owner を作らず、capture 後の未保存変更も失わない。

通常の main-context folder 遷移で旧 clean owner を降ろす場合も、Checking で capture した identity が
terminal 時点まで exact な entry だけを削除する。途中で sibling が作成・置換した clean cache は削除しない。
detached context の open は従来どおり clean owner を保持する。既存 `SidecarWriter` の通常保存契約や
coalescing queue は変更しない。

sidecar backup と tag sidecar backup が両方 OFF、prepared aggregate、synthetic search/rating/tag、
remote-only source では復旧 state を開始せず、従来の tail を直ちに続ける。通常 folder はその folder、
ZIP/PDF/RAR は実 container の parent を使う。拡張子は source kind に使わず、folder-watch mtime 用の
既存 `metadata()` read から同時に capture した `is_dir` を continuation が持つため、`photos.zip` のような
実 directory は自身の sidecar を使い、追加の UI thread I/O は生じない。metadata transfer など既存の
App-global exclusive operation が進行中なら同時開始せず、同じ modal ownership の下でその terminal を
先に待つ。

### 5.2 worker probe と source cache

quiescence と sidecar flush の完了後、`Checking` worker は Stage 1 に追加した read-only probe を呼ぶ。
probe は strict `load_for_import` と、要求された edit/tag family の v2 marker
照合を行い、write transaction、tag backup rotation、marker 更新を一切しない。結果は次を型で
区別する。

- 全 family が同期済み: source-current sidecar を保持し `Resuming` へ進む
- Missing かつ marker も無い: empty sidecar を保持し `Resuming` へ進む
- Missing で古い marker がある: worker で marker clear を行う
- marker mismatch: probe snapshot は捨てて import worker へ進む
- Unreadable/Corrupt/Unsupported/WriterFailed: disabled sidecar を保持し、marker を
  変えず警告して `Resuming` へ進む
- ChangedDuringRead または commit 前 revalidation mismatch: install 可能な sidecar を持たない
  `SourceChanged` とし、後述の one-shot strict cache reload へ進む

readable だが field の decode/domain/key が不正な場合も writer を止める前に検出できるよう、probe
では transaction を作らない Stage 1 prepare/validation まで実行してよい。復旧実行時は probe の
prepared value/source token を再利用しない。

最初の probe より前に必ず `Quiescing` を通る。既に受理された local-adjust/edit-bundle などが
commit 前なら、その時点の marker が一致していても初回 hydrate は stale になり得るためである。
`LocalAdjustWriteHandle` が存在する場合は queue snapshot が空でも後述の fence ACK を待つ。
全 completion と sidecar mirror の反映後に in-place reservation と worker batch を作り、strict idle、flush、probe を
同じ `Checking` worker でこの順に一度だけ行う。通常の再 probe は行わず、source identity が実際に
変化した場合だけ one-shot strict re-probe を許す。

worker から受け取った `Current` の `SidecarFile` だけを matching folder の App-global cache owner として
install する。これは後日の `sidecar_mut` が巨大な `mimageviewer.dat` を UI thread で再読込することを
防ぐと同時に、import 前の古い owner が新しい disk state を書き戻すことを防ぐ。commit 前
revalidation の `SourceChanged` は stale completion を install せず一度だけ strict re-probe する。
ただし同じ folder に write-disabled かつ dirty な owner が既にある場合、その未保存変更を
`Current` / disabled placeholder / cache refresh のいずれでも置換しない。新snapshotはcacheへinstallせず、
原本保護中の未保存変更を維持した旨をwarningとして残す。clean ownerだけを新snapshotへ置換できる。

import worker の cancel/disconnect/prepare error、preview clear の enqueue/ACK/DB error、target 消失後の
non-Current terminal では DB import を再試行しない。typed `CacheRefreshing` worker が strict
`load_for_import`、全content validation、source revalidation を一度行い、stable Loaded/Missing を
App-global owner へ戻す。stable だが semantic-invalid な Loaded は exact snapshot を disabled owner
として戻す。cleanup は旧 viewer continuation の cancel token から独立しており、target 消失後も
有限な検証を完了する。Unreadable/Corrupt/Unsupported/WriterFailed/ChangedDuringRead、refresh worker の
spawn/disconnect だけは disabled placeholder にして、UI thread の forgiving load や古い dirty-capable
ownerへ fallback しない。Checking の flush/spawn failure は例外で、DB import がまだ始まっておらず
live owner は App cache に残っているため、その cache を変更せず dirty のまま維持する。

### 5.3 既存 writer の quiescence

state を設置した直後に常に `Quiescing` へ進む。target 操作と App-global load/edit/save producer は
既存 gate が止める。独立 native video の再生・seek event は sibling owning context で通常処理するが、
sidecar owner は下記 in-place reservation が守る。ここでは既存
`metadata_transfer_writers_busy` と
`quiesce_metadata_transfer_context_writers` の release/ACK/poll 機構を共通 helper に切り出して使う。
少なくとも次を drain 対象にする。

- tag writer の paused FIFO、legacy tag seed、tag maintenance と App の `tags_db` release/ACK
- rating、local-adjust、edit preview、book/bookmark、book page copy/move
- metadata transfer、rename migration、delete/purge、drop copy、新規 folder、batch convert、capture
- single/bulk edit bundle と content-identity restore
- `FavoriteViewWriteDebounce` と `FavoriteViewStoreWriter`

`LocalAdjustWriteHandle::has_unfinished_work()` は開始判定の hint にだけ使う。worker は queue item と
document を取り出してから `process_write` を実行し、completion を送るまでの間、queue、documents、
result channel がすべて空に見えるためである。state 開始後の新 producer を止めた上で
`enqueue_fence()` を一度だけ queue 末尾へ積み、UI は非blocking に ACK を poll する。ACK より前に
publish 済みの全 completion を通常 `apply_local_adjust_write_completion` へ通して sidecar mirror を
反映し、`local_adjust_write_pending` も空になった後だけ flush/re-probe へ進む。fence の enqueue failure、
disconnect、worker stop は quiescence failure として import を開始しない。

worker は cancel して値を捨てず、既に受理した job と queue を finite terminal まで進め、その result と
sidecar mirror を通常 owner へ適用する。queue が空で worker が idle になっただけでは完了にせず、
completion が UI owner に消費されたことを fence/ACK に含める。writer failure は sidecar import を
開始せず typed error で modal を終端し、元の operation が保持する retry/通知契約を維持する。復旧の
ために同じ失敗を無限再投入しない。次段落の favorite-view だけは、失敗 command 自体をこの state が
保持できるため、intent を失わずに quiescent と判定して import を続けられる明示的な例外である。

metadata transfer の dialog は `ImportConfirm` / `Result` のように実 worker を持たない stage でも
毎 frame 描画される。restore が tag DB reader の release を所有している間は dialog 末尾の通常
`resume_metadata_transfer_context_readers` を抑止し、restore terminal の中央 resume だけへ戻す。
そうしないと、release ACK を待つ `Quiescing` と Result dialog の resume が同じ state を毎 frame
往復させる。

favorite-view は `adjustment.db` writer だが既存 helper から漏れており、500 ms debounce もある。
restore state 設置から modal 解除まで通常の favorite reconcile/retry submit を止める。
`Quiescing` では一度だけ `take_all` した command を
`retry=false` で submit し、全 result の消費と `is_busy == false` を待つ。submit/result failure の
command は `SidecarRestoreState` が lossless に保持する。DB import の終了後、modal を解く直前に Set は
通常 debounce、Remove/Clear は順序を保つ FIFO へ戻す。失敗を捨てたり、復旧中に再試行 loop を回したり
しない。この限定された intent 保持だけを追加し、§4.4 の全 producer 用 deferred-intent 層は作らない。

全 DB completion と sidecar mirror の反映後、state は App cache 内 owner から immutable flush batch と
identity reservation を作る。
recovery worker が既存 SidecarWriter への queue と send-failure fallback を実行し、process-global writer の
bounded strict idle fence を待つ。UI thread は serialize/write/idle wait を行わない。spawn、writer、
mutex poison、timeout の失敗では cache owner を変更せず、import を開始しない。idle 後に
strict probe/load を行い、synchronized なら Resume、mismatch ならその新しい owner だけを
`prepare -> commit` へ渡す。
これにより、quiescence より前に完了した in-process write を古い snapshot で取り込まない。

Missing marker clear も UI thread の既存 DB handle では行わない。canonical DB path と要求 family を
worker へ値渡しし、edit marker と tag marker をそれぞれの transaction/outcome で clear する。
cancel、busy、fault では当該 marker を clear 済みと報告しない。

### 5.4 target modal と入力境界

`modal_dialog_block_reason()` は `SidecarRestoreState` を一つの理由として参照する。projected context が
restore target と一致する main grid / embedded fullscreen / egui viewport だけが
「サイドカーから設定を復元中」表示を使う。target では state 開始時と各 frame
で keyboard、text/IME、pointer、wheel、touch、gamepad、shortcut、right-drag など semantic input を
consume/discard し、button/key release edge だけは hold state の解消へ反映する。入力を保存して modal
解除後に replay しない。state の解除は `Resuming` が完了した frame の全入力処理より後の App outer
tail で行い、同じ frame に残った event が通常操作として発火しないようにする。
IMEは Enabled / Preedit / Commit をsemantic inputとして捨てるが、composition ownerを残さないため
`ImeEvent::Disabled` はkey/pointer/touch releaseと同じcleanup edgeとして通す。

gamepad は dispatch 可否より先に device-disconnect/session-end を通常 commit していたため、modal
開始時と dispatch の最内境界で picker/hold を明示的に終端する。modal 前に成功済みの live rating
preview は App-global publication ledger から Undo record だけを作り、新しい DB/sidecar/XMP write や
commit-only picker row は発火させない。物理 hold/repeat は neutral に戻し、解除後に action を replay
しない。

エクスプローラからの raw dropped file と single-instance activation open path も開始時と各 frame に
drain/discard する。activation queue を未消費で保持すると解除後に navigation が発火するため不可とする。
metadata transfer、別 viewport の load/edit/save producer は既存 App-global gate を維持する。これは全 sibling
操作を新たに自由化する scope ではなく、保存競合を増やさず UI freeze を除くための境界である。

target の main / egui viewport の利用者 close は active 中 `ViewportCommand::CancelClose` で抑止する。root の
close request は同 frame の tray hide 判定も止め、CancelClose command を送っただけで抑止済みとしない。
modal に Cancel button や Escape close は置かない。通常 frame の poll では worker completion 受信だけで
join せず、result を state に保持し、`JoinHandle::is_finished()` が true の frame だけ join/owner resolve
する。process exit が不可避に始まった時だけ、`Checking` worker を cancel→join→result resolve し、
in-place reservation を live cache へ照合してから既存 exit flush/wait へ渡す。同じ fixed temporary path へ
Checking fallback と exit fallback が並行 write しないために必要な process-exit 限定境界である。

native video output event は sidecar-restore classifier へ入れず、従来の source epoch / item / owning-context
照合を通して通常処理する。target の native presentation は state 設置前の既存 fullscreen close で退避し、
その後に遅着した event は従来の stale 判定へ任せる。sibling context の再生、seek、pointer hold、HUD操作を
restore が捨てたり neutralize したりしない。target の egui input sanitize からも App-global native pointer/
seek hold のclearを外すため、main target frameを挟んだ sibling の down→move→up seekを維持する。

App受信前の presenter/WndProc まで止める `NativeSemanticInputGate`（revision、pump/render ACK、入力state
neutralizeを含む）も設計レビューしたが、2026-09-11 の仕様縮小で実装対象外とした。今回の価値は target で
進捗を表示して UI thread の47秒停止をなくすことであり、別native windowを全面停止する厳密排他ではない。
半端な App受信側 native event discard は残さず、sibling native videoは通常操作を継続する。

この close 抑止には残余リスクがある。`std::fs` の network filesystem read/write は hard cancel や
移植可能な I/O timeout を持たず、OS/driver が永久に返さなければ worker completion も来ず、承認済みの
target modal 仕様では対象画面を通常 close できない。repaint watchdog は stall を記録できるが、
処理を安全に完了したことにはできない。実装時はこの制約を log/運用記録へ残し、timeout を success や
marker 進行に変換しない。network data-dir/source の hard-cancel 対応は別 scope とする。

native presenter の中へ新しい dialog renderer は追加しない。通常の folder 切替ではtarget presentationを
state設置前にcloseしてowning egui viewportのmodalを露出する。main変更で保持するsibling native surfaceは
通常操作を継続し、targetの復元modalは表示し続ける。opaque target surfaceがmodalを隠す
経路が見つかった場合は、既存の非同期 presenter close terminal を`Quiescing` に含めてmodalを露出し、reopen
intentを保持する。同期Win32待機で隠さない。この方法で表示できない場合だけnative-painted modalを別scopeとする。

`start_loading_items_inner` は state 設置より前に、active detached media を AtRest owner へ promote して
root の `fullscreen_idx` を外すか、通常 `close_fullscreen` を行う。presentation transition中の close は
`TerminalClose` を積むが、次 frame は native/embedded early return より前の transition poll で terminal
effect を実行する。modal はtargetのroot/egui embedded/passive viewportに描き、保持するnative siblingは
通常のproducer/event経路を継続する。開始frameのtarget入力は `begin_sidecar_restore` 自身がsanitizeする。このため frame tail外に置いたrestore pollとwriter
fenceがfullscreen描画で止まらず、通常completion pollを飛ばすのは開始frameの一度に限られる。

remote session の acquire/release、disconnect、transport drain など ownership lifecycle は継続する。
mutation、navigation、user Stop/Close request は protocol の typed `Busy`（HTTP は既存 503）を返し、
黙って捨てたり解除後に replay したりしない。`BookResumeRead` のような read-only request は現在の
authoritative state を返してよい。reply 型に Busy が無い mutation は配線前に型を追加する。

### 5.5 初回表示と continuation の完了順

`start_loading_items_inner` から戻った直後に ZIP/PDF finalize が fullscreen を要求する経路がある。
最深の `open_fullscreen_with_materialization_and_contract` で restore state を確認し、matching target の
presentation intent だけを state に保持する。intent は context/generation、stable item identity、
`FsOpenMaterialization`、`FsPageLoadContract` を持つ。raw/final texture request、metadata load、
video/native presenter、fullscreen worker は起動せず、terminal 後に同一 item を再照合できた場合だけ
exact payload で通常入口を一度呼ぶ。利用者入力による二つ目の intent は input gate で破棄する。

modal 開始前に受理済みの別 context の非同期 load completion は、state が active の間に payload を
`take` すると caller 固有の ZIP/PDF password、navigation、fullscreen tail まで失う。次の poll/consumer
は receiver/pending owner を `take` する前に共通 active state を確認し、terminal までそのまま保持する。

- startup/activation path resolver、folder-pane scan、folder navigation
- tag/favorite/rating view、bookmark browser/media/book open
- ZIP/PDF enumerate/finalize
- external rescan、`pending_reload`、folder refresh/thumb pin/video override dirty consumer
- subfolder-expansion scan/install、smart-folder scan/count/prepare、filename-stack result
- fullscreen/media-navigation resolver、VST startup の deferred media open、automation play-test owner

terminal の次 frame に既存 poll が同じ owner と caller tail を通常どおり一度だけ適用する。これらの
scan/read/preparation worker は中央 DB/sidecar writer barrier ではなく、結果を receiver に保持したまま
`Quiescing` を塞がない。modal 中に新しい navigation producer は入力 gate が止める。

`Current` completion だけを cache/install 対象にし、family outcome は独立に扱う。edit success/tag
failure、またはその逆でも、成功 family の transaction/marker は維持し、失敗 family の marker は
進めない。channel disconnect のように outcome が不明なら DB marker と中央 row を再読込し、推測で
success にしない。通常成功では Stage 1 が返す insert 済み edit key/tag の delta を既存 in-memory cacheへ
overlay し、全 DB key scan を UI thread へ戻さない。Running worker が commit 後に切断して delta を失った
可能性がある場合だけ、worker が readonly で全 edit rollup と target tag key を最大 3 回読み直す。tag query
は prepare/query/row error をすべて伝播し、空/部分 map を authoritative success にしない。有限回で失敗した
場合は警告し、comic cache を保守的に無効化するが、既存 tag/rollup cache を空値で置換しない。

初回の thumbnail/fullscreen を補正済みにするため、次の順を固定する。

1. final probe が edit family の `ImportRequired` を返した場合だけ、Stage 1 commit より前に
   `edit_preview_cache.clear_with_completion()` を発行する。DB `DELETE` の typed success ACK と、それより
   先に publish された `Cleared` event の UI 適用を `InvalidatingPreview` で待ってから import を開始する
2. `Cleared` は current と全 AtRest viewer context の既存 materialization/request を evict する。DB owner の
   process-local epoch を DELETE 成功時に進め、各 thumbnail request ownerにもenqueue時epochを保持する。
   旧 row を既に読んだ result は UI install 時の exact-epoch gate で破棄し、同じ旧epoch ownerだけを
   `requested`/finalize待ちから外して再enqueue可能にする。clear後の新ownerへ旧resultが遅着してもその
   ownerは消さない。Building/Retiring context は lifecycle owner に任せる
3. Stage 1 の family transaction/marker update を worker で実行する
4. comic 変化時に `comic_docs` を無効化し、Stage 1 の確定 edit/tag delta を既存 cache/index へ overlay
   する。tag suggestion cache を無効化し、target の facet `visible_indices` を selection tail より前に一度
   再構築する。outcome 不明時の readonly recovery snapshot 取得も worker 上で行う
5. adjustment/local/crop/mask/conceal/comic を中央 DB から target generation へ hydrate し、probe 前に
   prewarm 済みだった tag cache も Applied tag outcome 後に再 hydrate する
6. catalog、thumbnail/video worker、selection/history を保留した既存 tail の順序で再開する
7. deferred fullscreen intent を exact materialization/load contract で通常入口へ戻す
8. favorite の保留失敗 command を通常 owner へ戻し、その frame の入力処理後に modal を解除する

preview DB clear の enqueue/DELETE/ACK failure は import を開始せず、中央 DB と旧 preview を整合した
まま cache refresh へ進む。DB row DELETE 成功後の cache file 削除失敗は stale read を生まないため、
orphan cleanup warning として扱う。

error、Missing、partial success でも live target は同じ中央 DB hydrate を行い、閲覧を続ける。target
context が不可避に消滅した場合は continuation と presentation intent を破棄する。`Quiescing` なら
App-global owner は移動しないので即 terminal にできる。`Checking` は snapshot reservation/flush receiver、
`InvalidatingPreview` は clear completion、`Running` は commit/cancel terminal、`CacheRefreshing` は
fresh cache owner の解決まで App-global state に保持する。`Current` sidecar は target 消失後も matching
folder の global owner として install し、non-Current は cache refresh 後だけ終端する。context-local
hydrate/effects/tail は sibling へ移送しない。target 以外の items、worker、一般 cache も変更しない。
tag transaction の確定結果はtarget消失時もApp-globalな候補catalog、facet件数、tag apply候補、
native overlay変換cacheを無効化する。target固有のtag cache hydrateやvisible rebuildはsiblingへ適用しない。
ただし process-global edit-preview DB の clear だけは例外で、その DB に由来する materialization を全
live/AtRest context から evict し、旧 epoch の遅着 result を全 context で拒否する。

### 5.6 別 process と線形化点

同じ build flavor で同じ data directory を使う二重起動は、既存 `SingleInstanceGuard` の
data-dir hash 付き named mutex が排除する。一方、portable と non-portable は base mutex 名が異なるため、
意図的に同じ `--data-dir` を渡した cross-flavor process、旧版、mIV 以外の SQLite client までは App の
modal で止められない。そこでは Stage 1 の transaction 契約を維持する。別 writer が先に row を作れば
transaction 内の
`INSERT ... ON CONFLICT DO NOTHING` でその row が勝つ。import が先なら、その後の通常 setter/update が
勝つ。lock を取れなければ family は rollback し marker を進めず、UI は worker の typed failure を
受ける。

ただし、import が先に write lock を取った短い区間に、busy timeout が無い旧版/cross-flavor viewer の
UI writer が来た場合、その相手の書込み失敗まで本 process から防ぐことはできない。cross-flavor で一つの
data directory を同時利用する構成まで正式対応するには、flavor に依存しない per-data-dir interprocess
mutex を全 writer が取得するか、全 producer が Busy intent を保持して再適用する必要があり、§5 の
縮小範囲を再び超える。次版の §1.209 では同一 build flavor の既存 single-instance 構成を対応範囲とし、
cross-flavor shared data-dir について「相手を破壊しない/書込みを失敗させない」とは報告しない。

Disk source の commit 直前 digest revalidation 完了を復旧 snapshot の線形化点とする。それ以前の
sidecar change は `SourceChanged` で row/marker を書かず、それ以後の external change は次の marker
mismatch とする。load から commit まで external tool の write/delete を拒否する file lease は、
外部 tool の保存を失敗させる新仕様なので追加しない。中央 DB の既存 row を sidecar が上書きしない
authoritative 契約も維持する。

### 5.7 実装範囲と見積り

production 差分は 17 file である。

- state/continuation/modal: `src/app.rs`、新規 `src/app/sidecar_restore.rs`、新規
  `src/ui_dialogs/sidecar_restore.rs`、`src/ui_dialogs/mod.rs`
- probe/marker clear/writer fence: `src/sidecar_import.rs`、`src/sidecar.rs`
- drain と favorite intent 保持: `src/ui_dialogs/metadata_transfer.rs`、
  `src/favorite_view_state.rs`
- preview clear/late result gate: `src/edit_preview_cache.rs`、`src/thumb_loader.rs`、thumbnail request ownerを
  context移送する `src/app/viewer_context_registry.rs`
- bypass/input/load-completion gate: `src/ui_fullscreen.rs`、
  `src/app/startup_ops.rs`、`src/app/subfolder_expansion.rs`、`src/app/smart_folder.rs`、
  `src/remote_ipc/ui.rs`
- main/fullscreen 共通 gamepad dispatch gate: `src/app/gamepad_input.rs`

test は主に新 module の pure transition test、既存 `src/app/tests.rs`、Stage 1 integration fixture へ
置く。当初のApp側実装、focused test、独立 review と修正の14--22時間見積りから、native producer
gateの5--8時間とnative-painted modal候補4--6時間を除外した。最後の disposable portable smoke は
別に60--90分を見込む。

実装時の横断監査表は次のとおりである。個別 caller の guard ではなく、owner を `take` する最初の
consumer と、結果を install する最後の terminal の両方で照合する。

| 境界 | restore 中の owner / 処理 | terminal 条件 |
| --- | --- | --- |
| target root / fullscreen / passive detached / gamepad 入力 | targetのsemantic inputをdiscard、release/不可避 lifecycleだけ処理 | replay queueを作らず、target holdとpickerをneutral化 |
| native video 入力 | source epoch/item/owning-contextの従来判定後、再生・seek・pointer/HUD eventを通常処理 | restore classifierを設けず、target egui sanitizeからもnative holdを消さない |
| activation / drop / remote | 新規 activation/drop は discard、reply を持つ remote mutation は typed Busy | restore 後の遅延実行なし |
| startup、folder-pane/nav、tag/favorite/rating/bookmark、ZIP/PDF、subfolder/smart/stack、media navigation | 受理済み receiver/pending と caller tail を `take` 前で保持 | state 解除後に同じ context owner が通常 poll で一度だけ適用 |
| external rescan / pending reload / folder thumb pin / video override | dirty/pending owner を保持し、mtime/signature を進めない | state 解除後の fresh reload/rematerialize |
| tag/rating/local-adjust/edit-bundle/content-identity/favorite/metadata/file operation writer | completion と sidecar mirror を通常 poll で適用、fence/release ACK を待つ | pending/result/FIFO と active worker がすべて settled |
| metadata transfer dialog | active worker は terminal まで進め、restore 中は reader resume を抑止 | restore terminal の中央 resume だけが reader ownership を戻す |
| periodic/edit-end sidecar flush | dirty ownerを App cache に残し、UI から `queue_flush` しない | `Checking` の in-place reservation付きworker batchが一度だけflush/strict idle/probe |
| thumbnail priority/keep/idle と late result | enqueue を止め、preview result は request owner の exact epoch で照合 | clear 前 ownerだけ解放し、clear 後 ownerを旧resultで消さない |
| edit-preview clear | DB DELETE、epoch bump、success event/ACK は cache worker が所有 | ACK と全 retained context の event 適用後だけ import worker開始 |
| Running success/partial | Stage 1 の Current sidecar と確定 delta を folder-global cacheへ適用 | live targetだけ facet/tag/context tail、discarded targetはglobal invalidationのみ |
| Running disconnect/error/cancel/source change | workerの有限 strict cache/DB recoveryへ移送し、推測で成功にしない | stable source ownerまたはdisabled owner、既知global effectを解決後に resume |
| target destruction | continuation/presentation intentを discard、App-global worker/ownerは保持 | phase固有terminalまでpollし、siblingへcontext stateを移送しない |
| process exit | `Checking` だけcancel→join→reservation resolve、他phaseはSQLite rollback/既存owner契約 | capture後に変化したdirty ownerを既存exit flushへ残してから終了 |
| UI thread I/O | restoreのflush/probe/prepare/commit/marker clear/preview DB clear/recovery scanを行わない | UIはchannel pollとin-memory delta/cache installだけ |

### 5.8 受入 test

- `Quiescing -> Checking -> Synced -> Resuming` と `Quiescing -> Checking ->
  InvalidatingPreview -> Running -> Resuming`、non-Current terminal から `CacheRefreshing` の transition
- local fence/pending、favorite busy、競合writer のどれか一つでも残る間は `Checking` を開始せず、
  全 owner が settled の時だけ進むこと。preview clear は success ACK 前に `Running` を開始しないこと
- state が旧 import 点で caller return より先に立ち、6 DB hydrate/catalog/thumbnail/video が未開始で
  あること
- tag release/ACK/paused FIFO、local-adjust、book/copy、rename/delete、content-identity/edit-bundle の
  completion と sidecar mirror を consume 後にだけ flush/reload/import すること
- pre-flush と最終 flush の channel/spawn failure で UI thread の serialize/write が 0 回、worker が
  既存 queue/fallback を所有し、App cache の dirty owner が変更されず import/marker が始まらないこと
- synchronized/Missing/disabled probe より前に local-adjust/edit-bundle/favorite completion が残る race で
  全 completion/mirror 反映と flush 後の strict probe を通ること
- local-adjust worker を queue/document の pop 後、`process_write` 内で停止して
  `has_unfinished_work == false` となる窓でも、fence ACK、completion 適用、pending map の解消前には
  Resume/flush/import しないこと
- favorite debounce/in-flight/submit failure/result failure を有限に drain し、失敗 command が modal 後の
  通常 owner へ順序どおり戻ること
- target main/embedded/detached/passive/gamepad/activation/drop/remote の gate table。target user close と
  semantic input は抑止し、release edge と不可避 lifecycle/result は進むこと。root close は tray hideを
  起こさず、gamepad session-endは新しいwriteを作らず成功済みlive previewだけをUndo可能なterminalへ
  畳むこと。process exit 中の Checking reservation は worker join/result resolve 後に既存 exit flush と
  同じ live cache ownerへ反映されること
- modal/semantic input sanitizeはtarget projected contextとtarget passive windowだけに適用され、sibling
  passive windowとnative videoのTogglePlay/seek/HUD eventが通常処理されること。main target sanitize frameを
  またぐsibling native pointer down→move→up/hold stateもneutralizeされないこと
- Checking result が届いても thread が `is_finished` になる前は通常frameでjoin/owner resolveせず、
  完了後のframeだけnonblocking joinすること
- ZIP/PDF の caller が folder load 直後に fullscreen を要求しても exact materialization/load contract
  付き intent だけを保持し、terminal 前に raw/final/metadata/video worker を一つも起動しないこと
- startup/folder-pane/folder-nav/tag/favorite/rating/bookmark/ZIP/PDF/external-rescan/pending-reload/
  dirty consumer/subfolder/smart-folder の ready completion が modal 中は owner/tail ごと保持され、
  terminal 後に context ごと一度だけ通常適用されること。保留した read/scan job が writer drain を
  塞がないこと
- preview DB DELETE success ACK が Stage 1 commit と最初の thumbnail spawn より先で、失敗時は import
  未開始であること。current と全 AtRest context の旧 materialization を clear し、clear 前に DB row を
  読んだ同世代 thumbnail が遅着しても epoch mismatch で install しないこと。旧epoch resultは一致する
  request ownerだけを解放して再enqueue可能にし、clear後の新epoch ownerを消さないこと
- comic/rollup、6 DB hydrate、Applied tag cache refresh、tag suggestion invalidation、facet visible-index rebuildを
  終えた初回表示だけが許可されること。通常成功 delta は既存 cache/index を保持して追加され、Running
  disconnect の strict DB recovery は query faultを空成功にせず有限 terminal になること
- target id/generation/folder mismatch、不可避 target destruction、複数 F12 sibling の非破壊
- Missing marker clear、source change、edit/tag partial、busy、fault、panic/channel disconnect の
  no-false-marker と DB-only recovery
- ImportRequired 後の cancel/disconnect/preview-clear failure で古い sidecar ownerを残さず、stable
  Loaded/Missing は `CacheRefreshing` で復帰し、viewer continuation のcancel/target消失から独立して
  cleanupを完了すること。semantic-invalid Loadedはexact disabled owner、不安定/読取不能 sourceだけは
  disabled placeholderにすること。
  target 消失後も Current owner は global cache へ入れ、context-local effects/continuation は適用しないこと
- aged dirty owner と edit-end edge が Quiescing 中の通常 UI flushへ入らず、Checking batchが一度だけ所有する
  こと。restore中は priority/keep-range/idle-upgrade thumbnail producer が旧cancel queue/requestedを変更せず、
  Resume後のfresh queueへ一度だけ投入されること
- Checking の batch identity/件数 mismatch は import を開始せず cache ownerを変更しないこと。capture後の
  mutation/write-disable/owner置換はdirtyを解除せず、main folder遷移でもcapture identityがexactな旧clean
  ownerだけを降ろす。detached openはsibling ownerを保持して従来のcache寿命を守ること
- metadata transfer が `ImportConfirm` / `Result` / dialog close直後でも、restore active中はtag DB reader releaseを
  resumeで巻き戻さず、Quiescingが有限にACKを観測できること
- 別 connection が import の prepare 前、transaction 待機中、commit 後に row を書く三つの順序

自動 test と build gate 後、使い捨て portable と 430 mixed 合成 fixture で、target modal が repaint し、
targetのkeyboard/mouse/closeとApp-global load/edit/save mutationを抑止しながら UI thread が heartbeat を
続けること、sibling native videoの再生・seekが継続すること、最初の thumbnail/fullscreen が補正済みで
あることを確認する。通常 `%APPDATA%`、既存 portable data、
実 sidecar は使わない。

### 5.9 実装 A: worker engine 境界

UI 配線に先行する実装 A は `src/sidecar.rs` と `src/sidecar_import.rs` に限定した。
`SidecarFlushOwners` は UI cache から移した全 owner を保持し、dirty snapshot だけを
`SidecarFlushBatch` として worker へ移す（write-disabled dirty owner は下記のとおり除外）。
通常の `queue_flush` 契約は変更せず、worker 上で既存 queue と channel failure fallback を使い、
strict bounded idle fence 後の成功だけが retained owner の dirty を解除する。write、spawn 相当の
channel disconnect、timeout、poison、結果件数不一致は元の dirty owner を返し、復旧を開始しない。
owner と worker result は batch 固有の型内部 identity で再結合し、別 flush の同件数 result では dirty を
解除できない。write-disabled owner は原本を絶対に書かないため batch へ積まず、成功 resolve 後も
dirty を保つ。これにより invalid folder の未保存 mirror が別の valid folder の復旧を停止させない。

read-only `probe` は strict source load、全 field validation、edit/tag marker 照合を行い、SQLite write と
tag backup rotationを行わない。Missing marker clear は canonical `adjustment.db` / `tags.db` だけを開き、
family ごとの `BEGIN IMMEDIATE` transaction と outcome を持つ。source が新しく現れた場合、cancel、fault、
busy ではその family を成功扱いせず、別 family の確定済み outcome は明示して維持する。

この段階では App、modal、writer drain、load continuation、preview ACK を配線していないため、単独では
UI 停止を解決しない。TempDir の焦点検証では lib sidecar-import 27 件、worker flush 5 件、strict
queue/idle 2 件、owner resolution 2 件、Missing revalidation 1 件、既存 integration 14 件が成功した。
430 mixed integration の再計測は load 13.282 ms、prepare 6.104 ms、transaction 13.858 ms、commit
19.391 ms、engine 25.495 ms、end-to-end 38.777 ms であり、UI 配線後の応答性を保証する値ではない。

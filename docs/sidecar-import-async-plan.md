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

# §1.234 / §1.231 設定復旧の所有境界

更新: 2026-09-14

## 確認した原因

`settings.db-shm` の共有違反は、復元一覧の一時接続ではなく Remote IPC の常設接続が
保持している。`RemoteIpcServer` は Web Remote の有効・無効にかかわらず起動し、
`LiveFavorites` が `SettingsFavoritesReader` の read-only SQLite 接続を server の終了まで
所有する。ダウングレードで設定 DB が非互換になった場合、起動用 `SettingsDb` と
`GLOBAL_DB` は空になるが、この独立接続は `run_native` の内側で復元・完全リセットを
実行している間も残る。

Windows の使い捨て WAL DB で、read-only 接続の生存中は `-shm` と `-wal` の削除が
どちらも `ERROR_SHARING_VIOLATION` になり、接続を閉じた後は両方削除できることを確認した。
したがって `-shm` の失敗だけを無視する変更は行わない。

## §1.234 の修正境界

設定 family を置換する直前だけ、`settings_db` が所有する process-wide の型付き
settings-family lease domain を quiesce する。`with_db` / `with_db_result` は全callerで
closure 全体を同じ RAII read lease に入れるため、Remote 以外の短期 access も同じ境界で
新規開始を止められる。production で常設する `LiveFavorites` は `ServerGuard::start` が
1 個だけ生成し、`current` を明示的に同じ read leaseへ入れる。test-only snapshot source は
DB lease を持たない。既知consumerは次のとおり。

| consumer | DB 読み取り | lease の範囲 |
| --- | --- | --- |
| 全processの通常設定 access | `with_db` / `with_db_result` 全caller | Arc clone前から closure 完了まで |
| `LiveFavorites::current` | persistent `SettingsFavoritesReader` | source mutex を取る前から `current` 完了まで |
| Remote collection settings | `load_into_settings` | 上記 `with_db_result` lease |
| Remote sort settings | `load_sort_order` | 同上 |
| Remote container reading / listing | `load_remote_reading_settings` / `load_remote_listing_settings` | 同上 |
| Remote adjustment settings | 通常 / timed `load_adjustment_render_settings` | 同上 |

要求には単調増加する世代を持たせる。domain は `Active` から `Quiescing { generation }` へ
移る短い mutex 区間で新規 lease を止める。以後の通常 access は typed
`SettingsFamilyQuiescing` を返し、Remote request は settings consumer だけ既存の error
responseで「設定復旧中・再試行可」を返す。pipe/session、decode、stream、画像 job は止めない。
既に lease を取った caller は DB 呼び出しを完了し、RAII Drop で active count を減らす。
active count が 0 になった後、action worker が唯一の production `LiveFavorites` と同一objectを
指す opaque control handle を使って `SettingsFavoritesReader` を source から take / Drop
した時点を線形化点とする。新しい reader はこの control handle の resume 以外から作らない。
その後に `Quiesced { generation }` ACK を App へ返す。この ACK の前には settings family を
変更しない。

同じ thread で `with_db` closure から別の `with_db` へ再入する場合は、thread-local な depth
で外側の同一 lease へ束ねる。外側 lease が Active 中に取得済みなら nested access を完了でき、
process-wide の active count は 1 のままなので自己 deadlock しない。外側 lease がない thread
から Quiescing 中に来た新規 access だけを待たずに拒否する。

lock 順は固定する。request は domain mutex から lease を得て mutex を解放してから
`LiveFavorites` mutex または `GLOBAL_DB` / SQLite へ入る。lease Drop 時には SQLite と
source mutex を先に解放してから domain active count を減らす。control worker は domain
mutex を解放して active=0 を待ち、最後に source mutex を取って persistent reader を落とす。
settings mutation worker は ACK 後に発行された非Cloneの `SettingsFamilyMutationPermit` を
要求され、permit無しでは production の restore/reset family mutationを呼べない。mutation
側の global DB checkpoint と Arc take/close も同じ generation の permit 専用 API を通す。
通常 boot/test 用の global install と区別し、quiesce を迂回する family 操作を作らない。
mutation中はdomain/source mutexを一切取らない。このため
UI、Remote request、SQLite の間に循環待ちは作らない。

reader を落とした後は、既存の checkpoint、WAL の strict 除去、main の atomic replace
または family reset へ進む。`-shm` も接続解放後に通常どおり除去する。ACK 後にも
sharing violation が出た場合は、別 process や短期の別 owner による通常の I/O failure として
扱い、「内部 Remote reader を閉じられなかった」とは断定しない。

quiesce は UI thread で mutex、condvar、worker join を待たない。復元ダイアログの操作を
`Idle -> Quiescing -> Mutating -> Resuming -> Result` の一つの状態所有へまとめ、control と
mutation worker の event を poll する。世代が違う ACK/result は破棄する。server 不在は
`NoRemoteOwner` として直ちに mutation へ進み、server startup failureも同じく DB owner が
存在しないことを表す。quiesce command の送信失敗や control worker failure は family 未変更の
`Recoverable` として表示する。

mutation worker は既存 `RestoreFailure` の「family を触る前なら Recoverable、触った後なら
Terminal」を保つ。operation owner は `ReaderRelease::{NotStarted, ConfirmedDropped}` と
`Mutation::{NotEntered, RecoverableProven, MayHaveTouched}` を直積で持つ。Cancel / worker起動失敗
などmutation前と証明できる場合はNotEntered、既存`RestoreFailure::Recoverable`だけを
RecoverableProven、Success / Terminal / mutation内panicやchannel切断はMayHaveTouchedへ倒す。

NotStartedではmutation permitを発行しない。persistent reader dropのACKが不明な場合もfamilyは
未変更だがmutationへ進まず、Remote ownerの結果が確定するまでdomainを閉じたまま再試行または
終了を表示する。ConfirmedDropped + RecoverableProvenでは同generationのresumeを必ず要求する。
resume成功ならRemote sourceとlocal domainをActiveへ戻す。resume失敗でもlocal family domainは
必ずActiveへ戻し、persistent readerを必要とするFavorites要求だけを`Resuming`/typed
`Unavailable`へ残して明示再試行できるようにする。family未変更が証明済みなので、6短期readは
process-wide domain再開後に通常の`with_db`で現DBを読み、個別I/O失敗は従来どおり返す。
復元成功や黙ったRemote恒久停止として扱わない。MayHaveTouchedではreader/domainを
再開せず、従来どおりsave抑止のままアプリ終了へ進む。Remote session/requestはquiesce中の
settings responseを再試行でき、resume後は同じsessionの次requestから通常応答へ戻る。

process-wide domain は `settings_db`、persistent reader controlは `ServerGuard` が所有し、Appは
opaque control handleだけをoperation workerへ渡す。Appはsettings restore UIのconfirm後にだけ
世代を払い出す。閉じる/cancelはaction開始前だけ従来どおりで、Quiescing以後はstate ownerが
最終 Result までactionを保持する。server shutdownはreader ownerを通常dropし、stale世代の
pause/resumeは拒否する。server不在時もprocess-wide domainは必ずdrainし、local短期accessを
残したままmutation permitを発行しない。

`RemoteIpcServer::start` が Err を返しただけでは `NoRemoteOwner` とみなさない。reader生成後に
home/write/heavy/stream/listener spawnが失敗しても、それ以前に起動済みのqueueとthreadを
startup rollback ownerが停止・joinし、全`CollectionEngine` cloneと`LiveFavorites` readerの
Drop完了を確認してからErrを返す。rollback完了後だけAppは`NoRemoteOwner`として扱える。
reader生成前の失敗とrollback済みの失敗を回帰し、detached JoinHandleや待機中queueがreaderを
保持しないことを固定する。

Remote の短期readは `SettingsFamilyQuiescing` を途中でString/Internalへ潰さない。collection /
home / favorite search / tag browse とsortは`CollectionErrorCode::Busy`、reading / adjustmentは
`MediaErrorCode::Busy`、listingは`RemoteWriteErrorCode::Busy`へ写し、その他のDB errorだけを
従来のInternal/PersistenceFailedへ写す。`LiveFavorites`も同じtyped Busyを返す。各protocolの
直接回帰で、quiesce中にstartup snapshotへfallbackしないことを固定する。

UI threadへ届くRemote writeでは、settings familyを変更する`SetSortOrder`だけをproductionの
exhaustive classifierで識別する。Quiescing以後は`apply_remote_write`を呼ぶ前に
`RemoteWriteErrorCode::Busy`を返し、in-memory `self.settings.sort_order`もdiskも変えない。
Spread、Bookmark、Rating、ViewTrimなど別DB ownerのwriteとVideoStream requestは従来どおり
処理する。全`RemoteWriteRequest` variantのclassifier回帰で、将来settings-family variantが
無分類で追加されることを検出する。

quiesce を操作側で cancel する場合は family 未変更の間だけ許し、同 generation の persistent
reader を resume してから domain を Active へ戻す。古い ACK、timeout 扱い、channel 切断では
mutation へ進まない。元の save-suppression 値も family 未変更の Recoverable 経路で復元する。
worker 待機は UI thread 外で condvar/event を使い、UI 側へ固定 delay や polling I/O を
追加しない。

回帰は TempDir の WAL DB と fake lease backend で行う。少なくとも、上表の全 consumer が
同domainを通ること、通常の read、in-flight read、quiesce 中の新規 settings readだけが
Busy、世代違い ACK、recoverable 後の再接続、resume failure、success/terminal 後の非再接続、
WAL 削除失敗時の原本保持、復元と完全リセットの双方を固定する。Windows では実 read-only
connection が生存中だけ sharing violation になり、ACK 後に family 操作が成功する統合試験を
加える。外部 process lock を模した ACK 後の sharing violation は通常の restore I/O failure と
して分類し、内部 lease failure の診断にしない。

resume open failureではlocal domainと6短期readが通常再開し、`LiveFavorites::current`を使う
favorite-searchだけがtyped Busyを返すこと、明示retry成功後にfavorite-searchも復帰することを
直接固定する。

test の `DataDirOverrideGuard` / `reset_global_for_test` は global DB だけでなく domain phase と
active count も隔離する。guard 取得下で active=0 を確認して Active へ戻し、Quiesced 世代や
thread-local depth を後続 test へ漏らさない。nested read、Quiescing 中の別 thread 拒否、外側
取得済み nested 完了、stale permit 拒否を pure / TempDir 回帰に含める。

## §1.231 の修正境界

`BackupSource::PreUpgrade` の version はファイル名由来の来歴であり、
`schema_meta.app_version` は DB 内容の互換性である。両者を同じ値へ上書きせず、一覧の
「保存した版」と `BackupCompatibility` は DB 内容だけから決める。ファイル名の version は
従来どおり世代ラベルにだけ使う。これにより既存 `preupgrade-vunknown` の中身が新版なら、
新版として操作を無効にできる。

初回起動は `last_seen_version == None` を「版変更」と扱わない。DB には現版を記録するが
preupgrade snapshot は作らない。`Some(old) != current` の実在する版跨ぎだけが従来の
snapshot を作る。TempDir 回帰で clean install、既存 unknown ファイル、ファイル名と内容の
version 不一致、通常の版跨ぎ、同版再起動を固定する。

2026-09-14 実装 checkpoint: `BackupSummary` は `content_app_version` を明示し、来歴は既存
`BackupSource::PreUpgrade(version)` だけに保持する。clean install は bootstrap save で現版を
記録するが snapshot を作らない。focused 回帰は backup summary 18 件、clean install 1 件、
実版跨ぎ 2 件が pass。§1.234 統合前のため full gate / verification build はまだ行わない。

## 検証区分

§1.231 と §1.234 のデータ・状態遷移は通常プロファイルを使わず headless で検証できる。
最終の手動確認が必要な場合も `scripts/prepare-portable-smoke.ps1` で作る
`target/portable-smoke/data` だけを使用し、通常 `%APPDATA%\mimageviewer` と既存 portable は
使わない。portable の実行は別途承認された時間帯・操作範囲に限る。

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
から Quiescing 中に来た新規 access だけを待たずに拒否する。read lease は取得 thread から
移動できない型とし、同一 thread 内で guard が取得順と逆でない順序で Drop されても、TLS depth が
0 になった最後の guard だけが process-wide count を減らす。`SAVE_SUPPRESSED` と quiesce が同時に
有効な mutation 中は family Busy を優先して返し、Remote の再試行可能性を失わない。

lock 順は固定する。request は domain mutex から lease を得て mutex を解放してから
`LiveFavorites` mutex または `GLOBAL_DB` / SQLite へ入る。lease Drop 時には SQLite と
source mutex を先に解放してから domain active count を減らす。action worker は domain
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

quiesce は UI thread で mutex、condvar、worker join を待たない。復元ダイアログは
`Idle / Running / RemoteUnavailable` の一つの型付きstateを持ち、`Running` が所有するworker内で
`Quiescing -> reader drop -> Mutating -> Resuming -> Result` を直列化する。worker完了はchannelと
repaint requestでUIへ返し、UIは結果だけをpollする。server不在はpersistent ownerなしとして
process-wide leaseだけをdrainしてmutationへ進む。server startup failureも、下記rollback完了後だけ
同じ扱いにできる。worker起動失敗はfamily未変更の`Recoverable`として表示する。

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
opaque control handleだけをoperation workerへ渡す。settings restore UIのconfirm後にworkerが
domain世代を払い出す。閉じる/cancelはaction開始前だけ従来どおりで、Quiescing以後はstate ownerが
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

短期readを長いjobの準備段階から使う2経路も、一時的Busyを通常の実行失敗へ潰さない。
Remote archiveはlisting設定を読む前に変換・列挙を始めず、Remote AIは補正設定を読む
composite準備で止める。job自体を待機・自動再試行したり、長い処理中にfamily leaseを保持したり
せず、IPC protocol v55の`settings_recovery_in_progress` terminal detailで終了し、Web側へ
「復元またはリセット完了後にもう一度実行」のmessageをそのまま表示する。通常の
`MediaErrorCode::Busy`（取消・supersede等）とはcore内部のtyped composite errorで区別し、
archive/AIの各job registryまで専用terminal codeが届くことを直接回帰する。

Remote AI は最終 composite 再検証だけでなく、`Completed` を registry の Ready として公開する
直前にも短い settings-family read lease を取得し、`registry.complete` が終わるまで保持する。
再検証後に quiesce が先行した場合は `settings_recovery_in_progress` で終了し、公開leaseが先行した
場合はmutation側が Ready 公開完了まで待つ。AI計算全体やarchive job全体へleaseを広げない。

UI threadへ届くRemote writeでは、settings familyを変更する`SetSortOrder`だけをproductionの
exhaustive classifierで識別する。Quiescing以後は`apply_remote_write`を呼ぶ前に
`RemoteWriteErrorCode::Busy`を返し、in-memory `self.settings.sort_order`もdiskも変えない。
Spread、Bookmark、Rating、ViewTrimなど別DB ownerのwriteとVideoStream requestは従来どおり
処理する。全`RemoteWriteRequest` variantのclassifier回帰で、将来settings-family variantが
無分類で追加されることを検出する。

action開始後の取消は追加せず、既存どおり完了結果までmodal ownerが保持する。reader dropを
確認できない場合、worker panic、結果channel切断ではmutationへ進めず、または
`MayHaveTouched`として終了を要求する。元のsave-suppression値はfamily未変更のRecoverable経路で
復元する。worker待機はUI thread外でcondvar/eventを使い、UI側へ固定delayやpolling I/Oを
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

2026-09-14 実装 checkpoint: `settings_db` に process-wide lease domain と非 `Send` の
read guard、世代付き non-Clone mutation permit を実装した。`with_db*` は save 抑止判定や
`GLOBAL_DB` clone より前に lease を取得し、nested guard は Drop 順に依存せず同threadで最後に
残った guard が process active count を 1 回だけ減らす。Remote の短期 read 6 系統は既存の
protocol別 Busyへ写し、UI write は `SetSortOrder` だけを apply 前に拒否する。

`LiveFavorites` は production の常設 readerを `Live / Paused / ResumeFailed` で所有し、
同じ世代の mutation permitだけがtake/dropと再openを実行できる。再open失敗時はlocal domainと
短期readを戻し、Favoritesだけを明示再試行までBusyにする。`RemoteIpcServer::start` はreader生成後の
全workerをstartup rollback ownerへ載せ、途中のspawn失敗でも停止・join・clone解放を終えてから
Errを返す。復元ダイアログはworkerを起動して即returnし、UIはchannelをpollするだけである。

mutation worker とFavorites再開retryは、結果receiverだけでなく`JoinHandle`も同じtyped App stateが
所有する。通常frameは`is_finished`確認後だけjoinするため待機しない。worker実行中にprocess終了へ
至るroot closeを受けた場合は、通常の「閉じてtrayへ格納」は従来どおり処理し、実際にprocessを
終了するwindow close / tray Exitだけをtyped intentとして保持して`CancelClose`する。tray-hidden時は
このdeferred closeを既存のhidden-root wake projectionへ同じframeで反映し、terminal観測後に元の
close intentを再発行する。`on_exit`先頭にも最終fenceを置き、通常close frameを迂回した終了でも
worker/retryをjoinしてからsettings save/flushへ進む。結果channelの受信だけをthread完了とは扱わず、
panic・切断時はsave抑止を解除しない。

production caller scanでは、`restore_from_with_permit` / `full_reset_with_permit` は
`run_settings_family_operation`だけから呼ばれ、`checkpoint_global_db_for_mutation` /
`take_global_db_for_mutation`もこの2関数内だけにある。`settings_restore::read_summary`のlocal
`Connection`は同期`list_backups`のprivate helperであり、各反復の末尾でDropされてから
`open_settings_restore_dialog`が戻る。action workerが始まるのは後続のconfirm後なので、この一覧用
接続はmutationと重ならない。settings familyを直接開く残りのproduction経路は起動時boot、
permit内の復元候補scratch検証、またはpermit内のfamily mutationに限定される。

TempDir回帰では、実 `SettingsFavoritesReader` のread-only接続を保持したまま開始した復元と
完全リセットが、lease drainとreader drop後にWAL/SHMを含むfamily操作を完了することを確認した。
recoverable結果、resume失敗と明示retry、成功後の閉鎖、nested/out-of-order Drop、Busy変換、
startup rollbackもheadlessで固定した。AI Ready公開とquiesceの両順序、process終了closeの保留と
再発行、tray-hidden wake、`on_exit`のjoin-before-persistenceもbarrier / synthetic workerで固定した。
通常プロファイルと既存portableの設定は使用しない。

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

2026-09-14 最終自動検証では、settings-family 9件、終了lifecycle 6件、Remote jobの
settings-recovery境界 5件のfocused回帰がpassした。`cargo check -p mimageviewer --bin
mimageviewer-core`、fmt、UI glyph、viewer-context audit、diff-checkもpassした。最終
`scripts/test-full.ps1`はmain 8410 pass / 0 fail / 45 ignored、snapshot 50、workspace・
integration・doc・vendorを含めてexit 0だった。完全ログは
`target/section234-settings-recovery-20260914/test-full-final.{stdout.log,stderr.log,exit.txt}`へ
保存した。

resident不在を確認して`scripts/build-dev.ps1 -PreserveRuntime`を実行し、同じprotocol v55
sourceからcoreとRemote serviceをともに生成した。exit 0、core SHA-256は
`51C2694DA2DC24AE4670E03354DD9206BB8916BAC96934EF5FE8CEE69C8B257C`、Remoteは
`192E2F800704A833C46831C2A42C47F24558B17A19C80F3DC23BEC21CE7D057D`である。agentは
通常profileのアプリを起動・停止していない。承認済みの実機確認には別途current sourceの
`portable,test-script`成果物を`target/portable-smoke`へ作り、通常profileと既存portableを
使わないfixtureを配置した。

2026-09-14朝の実機検証: 利用者の18:30までのPC操作了承のもと、本人が起動した上記portableを
skyで操作した。新版内容999.0.0の`preupgrade-vunknown`候補は利用不可表示で、無効ボタンを押しても
確認画面へ進まないことを確認した。起動時の世代回転後、読み取り専用で期待markerを照合したbak2を
復元し、成功表示と終了を確認。`portable-verify-restore.*`は1/1 passで、期待favorite、旧favorite不在、
操作前の退避、WAL/SHM不在を検査した。続く完全リセットも成功表示・終了し、
`portable-verify-reset.*`は1/1 passでactive db/WAL/SHM/bak1..10不在と退避を確認した。
リセットの退避名も製品の既存規則どおり`before-restore-*`であり、fixture側の誤った
`before-reset`期待だけを修正した。製品変更・全体テストの再実行はしていない。

初回markerのliveは安全なDrives+last_seen=Noneをseedした状態で保留。利用者が外出済みのため
最後の起動は後にすると回答した。追加起動は行わず、復元・完全リセットの成功とは分けて残す。
起動前に検証担当が誤って実行した`VerifyClean`のNone判定失敗は、未起動のseed状態を見たもので
製品不具合・実機結果として採用しない。clean DB不在からの初期化はheadless証拠と区別する。

ツール境界の記録: 最初のsky裸の絶対pathによるlaunch要求は、インストール済み通常版へ誤解決し、
意図せずAPPDATA runtimeのプロセスが起動した。直ちにUI操作を停止して利用者へ報告し、本人が通常版を
終了して正しいportableを手動起動した。通常版での復元・リセット操作はしていないが、起動による
通常設定の自動保存は否定しない。以後の操作対象はprocess pathとfixture markerを照合したportableのみ。
Web Remote serviceの別プロセス起動は観測しておらず、core内常設IPC readerの検証と区別する。

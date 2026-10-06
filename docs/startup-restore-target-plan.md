# §1.335 起動時に復元する明示一覧の所有設計

2026-10-06。Phase 1 改訂は `23d885e11` で独立設計レビュー承認済み
（ready to implement、指摘なし）。main 所有境界は detached の症状パッチではなく構造的修正と合意済み。
Phase 2 は red 再現から着手し、遅延 close 完了経路へ達する案について一度停止した。
その後、利用者・設計担当・独立 reviewer が、適格な既存一覧への表示要求を受理した時点で
記録する簡素化案に合意し、実装を再開した（§10）。terminal event/effect は変更しない。
初回実装は `83d2ac3c1`。独立実装レビュー `target/r1335i-review.txt` の P2 3件
（Collection の採用通知、動画リングの一覧復帰受理、全体 gate の完了証拠）への対応は完了。
初回の結果は §10.4、今回の修正・再監査と最終 gate は §10.5 に記録する。
追加監査で見つかった close 呼出し3箇所も設計担当・独立 reviewer の合意後に適用し、
今回の追補検証は §10.6 に記録する。
製品は起動しておらず、利用者の実機確認と設計担当の検収は未実施。

## 1. 対象と観測の区別

- 外部報告: mIV スレ >>508、v4.3.0 portable。ZIP の画像を表示して終了すると次回は
  ZIP のページ一覧になる。報告者のモード・開き方・起動設定は未確認。
- 本作業の指定: 起動場所が「前回終了した場所」の場合、最後に明示的に開いた一覧を復元する。
  本の一覧を開いてから読むなら本一覧、一覧を経ず読むなら直前の親一覧。
  読書後に Backspace 等で一覧を明示したら本一覧へ更新する。
- コード調査・headless 再現はこの指定条件を対象にする。外部報告者の環境そのものを
  再現したとは主張しない。
- 作業開始時: branch `next-startup-restore`、HEAD `1f2f6d5c0`
  （§1.335 仕様追記）。base `330f1c3e3`、開始時の working tree は clean。
  以下の製品参照行はこの HEAD 基準。

不変条件は「**main が採用した明示一覧の identity が、読書用コンテナのロード・
表示窓の移動・終了処理によって置き換わらない**」。ページ読書位置とは別の状態である。

## 2. `last_folder` の全 writer と経路

`rg --no-ignore` でリポジトリの参照を調査した。値を通常操作で生成する直接 writer は
次の 2 箇所。終了時に現在フォルダをこのフィールドへ再代入する処理はない。

| writer / state transfer | 現在の意味・条件 |
| --- | --- |
| `src/app.rs:35565`、`start_loading_items_inner`（入口 `:34278`、内側 `:34394`） | 読み込んだ main の非合成物理場所を `effective_folder().unwrap_or(source_path)` で保存。直前 `:35556` は合成 path と detached history suppression を除外するだけで、一覧／ページ表示の意図を区別しない。`:35566` で A/B を同期し、`:35567` で settings 保存 |
| `src/app.rs:23679`、`enter_drive_list`（`:23616`） | `Some(PathBuf::new())` をドライブ一覧 sentinel として保存。SLI を通らない専用 main 一覧入口 |
| `src/settings.rs:9950` | 環境設定 OK の差し替えで live 側 `src.last_folder.take()` を引き継ぐ。新しい場所を生成しない。カーソルの 2 値も直後に一緒に移す |
| `src/settings.rs:7271` / `:4286` | default / serde 欠落時は `None`。通常操作の writer ではない |
| `src/settings_db.rs:2966`, `:2024`, `:900`, `:949` | generic settings_kv 読み込み／Settings 全体保存。JSON 移行も Settings に復元する。last_folder 固有の変換・意味の migration はない |

既存の経路を上記 writer まで照合した:

| 入口・経路 | writer への到達と現在の問題 |
| --- | --- |
| フォルダ navigation（ツリー、住所欄、親、通常 load） | `src/app.rs:23602` → `:24691` / `:25252` → `install_scanned_folder_listing :25655` → `:25930` SLI。実フォルダを保存。画像のみフォルダの自動ページ表示判定は `:25813` で、同じ一覧 install を再利用する |
| ZIP / 直読み RAR enter | `:31274` / `:31331` → worker → `poll_zip_enumerate :31565` → `finalize_zip_enumerate :31769` → `finalize_prepared_zip_grid :31814` → `:31830` SLI。自動 fullscreen は `:31395` の予約から後段で開くため、先に ZIP 自身が保存される |
| PDF / EPUB enter | `:32424` / `:32624` / `:32736`。warm placeholder `:32936` と実列挙結果 `:33389` 等が SLI を使う。後段の `:33425` 付近で deferred fullscreen を開く。cold candidate／staged owner は成功採用後に到達するが、採用後の一覧意図の区別はない |
| 変換アーカイブ | `src/ui_dialogs/archive_convert.rs:312` の `open_archive_via_cache_owned` → `:362` load cached ZIP。元 source を `archive_source_override` で保持し、SLI writer は元パスを保存。staged 変換採用は `src/app.rs:30427`、Smart は `supply_smart_archive_load_alias` を通る。キャッシュパスを復元先へ出さないことは既存の正しい契約 |
| ZIP 内部 enter/back/DFS | `src/app.rs:32241`, `:32256`, `:32295`, `:32327` → `zip_nav_show_current_level :31974`。軽量 items install で SLI を通らず **last_folder は更新されない**。外側 ZIP へ丸められ、現在一覧の prefix は保存されない |
| 本／実フォルダから外への back | `grid_parent_nav_target :20895`、`resolve_return_to_parent_nav :20991` 周辺、`take_pending_return_to_parent_nav :21033` → `apply_fullscreen_close_nav_immediate :21077` または update の共通 navigation。物理親なら通常 load→SLI。合成親なら各 surface の復帰 path |
| Smart root / scoped child | `src/app/smart_folder.rs:8602`, `:8755` は合成 path の SLI（保存除外）。物理子採用 `:3246` / `:3405`, `:3472`, `:3518` は SLI inner を通り、非合成の本／実フォルダを保存。prepared root 復帰 `:7883` 等は軽量 path で直接 writer なし |
| Collection root / physical child | `src/app/collection_grid.rs:3030`, `:3314`, `:3356` 周辺は `current_folder=None` の aggregate install、SLI を通らず writer なし。子は `OpenRequestOwner::CollectionGridPhysical`、`src/app.rs:25269` / `:29832`、`src/app/collection_navigation.rs:3495`, `:3593` 等から通常 load→SLI で物理 source を保存 |
| Global / Favorite / Tag search | `src/global_search_ui.rs:1693`, `:1701`, `:1805` の Global 結果置換は軽量 install、writer なし。Tag `src/app.rs:27796`、Favorite `:28114` は合成 SLI なので除外。結果から物理本に drill すると通常／staged load→SLI で本自身を保存。検索 close の `:27433` や `src/global_search_ui.rs:2195` 周辺は既存 restore を再利用する |
| サブフォルダ展開 | `src/app/subfolder_expansion.rs:2293` の prepared install → `:2348` `start_loading_subfolder_items` → 合成 SLI。`:2376` 周辺で `SubfolderExpansion` surface にする。root/roots/saved_folder は復帰用で、表示中の物理一覧の証明ではない。reinstall `:2147` / restore `:2242`, `:2269` も合成一覧なので新 target/cursor を変更しない |
| Rating root / physical child | `src/app.rs:28261`, `:31166` は合成 SLI なので除外。物理子は `:28651`, `:28764`, `:29832` 等の owner を通り物理 SLI へ到達。戻る `:30487`, `:30548` は Rating root install または物理 chain の load |
| 本棚／製本フォルダ | `src/app.rs:42236` は settings の books root を読む。`open_books_root :43873` → `:43882` 通常 load。名前だけが「本棚」の **実フォルダ** (`book_address_label_for_path :43850`)。本棚／個別本は物理 SLI writer。管理ダイアログを開いただけでは writer なし |
| 閲覧履歴／しおり | `src/app.rs:28136` → `:28233`、`:43020` → `:43050` は合成 SLI で除外。履歴／しおりから main で本を開くと物理 SLI で本を保存。return-to は既存 view state が所有し、last_folder の別 writer ではない |
| Back/Forward、A/B | `src/app.rs:22171`, `:22506`, `:29832`, `:30331`, `:21490` 等の既存 staged 採用を使い、物理着地は SLI、Drive は専用 writer、合成着地は保存除外。`:21320` の `sync_quick_folder_settings` 自体は last_folder を書かない。同じ path の A/B switch など load のない切替は last_folder を再生成しない |
| F12 linked / independent | linked は同じ main book の表示先切替。独立窓は `src/app.rs:55800`, `:55810`、変換 `:56105` から別 context。`with_active_viewer_context :20485` → `with_detached_viewer_main_history_suppressed :20470`、`:20460` の深さによる既存保存抑止。main の target writer を独立 viewer の book load に参加させない |
| startup | `src/app/startup_ops.rs:1311` → mode routing → 通常 load (`auto_fullscreen=false`)→SLI。初回 update `src/app.rs:85067`、解決失敗等 `startup_ops.rs:381`, `:508`, `:523`、Remote IPC の fallback `src/remote_ipc/ui.rs:4002` もこの共通入口 |
| 終了、トレイ退避／復帰 | `src/app/runtime_ops.rs:370`, `:423` と `src/tray_integration.rs:342` → `persist_window_state_and_flush :70948`。last_folder 自体の追加 writer はなく、カーソル `:70957` と全 settings を保存する。退避は終了ではなく、復帰で default startup を再実行して本を開き直さない |

事故の生成境界は SLI の物理コンテナ採用である。shutdown や startup にモード別 guard を
置くと、それ以前の settings save、明示一覧への復帰、内部 ZIP 一覧、reload の区別が残る。

## 3. `last_folder` の全 reader

| reader | 用途 |
| --- | --- |
| `src/app/startup_ops.rs:1318` | `open_default_startup_target` が起動候補に渡す |
| `src/known_folders.rs:118`–`:134` | Previous は旧値を resolve、開けなければ祖先→Desktop。**Desktop / Specific もその場所と Desktop が使えない場合に last_folder へ fallback**。Drives / ReadingHistory はここでは path を返さない |
| `src/known_folders.rs:145`, `:166`, `:183` | 解決先が旧値そのものか照合し、対になった名前／行位置だけを `startup_cursor_hint` に返す。削除済み子から祖先へ遡った時には旧カーソルを適用しない |
| `src/app.rs:70919` | `cursor_to_persist` が `effective_folder` と照合。合成／別場所なら None とし、exit/tray で legacy カーソルを明示破棄する |
| `src/app.rs:87760` | Previous + 空 path の Drive sentinel を判定。Drives 明示設定も扱う |
| `src/settings.rs:9950`–`:9953` | Preferences OK が live 値とカーソルを一緒に引き継ぐ |
| `src/settings_db.rs:949`, `:2024`, `:2966` | 全 Settings の serde／DB roundtrip、旧 JSON import、バックアップ／復元の generic 読取 |
| `src/settings_transfer.rs:454` | 環境設定 transfer で「利用データ・登録先・履歴・検索対象」に分類して除外。復元先を他 PC の設定へ移さない |

**「現在の本」を last_folder から取得する製品 reader は見つからなかった。** 現在の本／住所／
BS の親は `effective_folder` (`src/app.rs:20501`) と `current_folder` / source override、
本棚は `book_root_path` を使う。SLI に現在の場所を保存する副作用が混在しているのであり、
current-folder accessor を新設する必要はない。

テスト側の参照も棚卸しした: `src/app/tests.rs:16766`, `:16811` (Drive)、`:18403`,
`:18467` (startup password / A/B exit-restart)、`:20642` (元 archive 保存)、`:31523`–`:31643`
(cursor と sentinel)、`:71709` (detached sibling 不変)、`src/app/sidecar_restore.rs:2675`,
`:2681` (隔離 sentinel)、`src/known_folders.rs:489`, `:534`, `:560` (cursor/fallback)、
`src/settings.rs:13669` (default)、`src/settings_db.rs:4640` (roundtrip)、
`src/settings_transfer.rs:1604` (除外 sentinel)。コメントだけの参照は
`src/folder_tree.rs:207`, `:1270`、`src/global_search_ui.rs:1681`、App の契約コメント。
これら以外に direct field reader/writer は検索で見つからなかった。

## 4. 現在の startup と red 再現

`open_default_startup_target` は ReadingHistory → Drive → known_folders の順に分岐する。
path が決まったら legacy cursor hint を `select_after_load` と行位置へ載せ、
`load_folder_or_convert_archive` (`src/app.rs:23808`) を呼ぶ。
この wrapper は `with_auto_fullscreen(path, false)` (`:23809`)。
従って通常操作で直前の book path が保存されていると、**本は再開するがページ表示は要求せず、
見ていないページ一覧を開く**。Ignore / Refused は selection hint を以前の値へ戻す。

再現プローブは実 ZIP（2×2 PNG）と `phase_c_support::AppTestEnv` の temporary data_dir を使う。
parent load → `with_auto_fullscreen(book, true)` → staged 通常採用 → fullscreen を確認 →
`on_exit_inner` → `Settings::load` → `App::new_from_settings` → default startup → staged 採用を
通して確認する。最後に「復元 path は parent」という新仕様 assert を置く。
正常な現状確認（book が保存され、再起動後 book の一覧、fullscreen=None）を先に通すため、
単なる hand-written state model の失敗とは区別できる。

実行結果・command・ログ・プローブ保存先は §9 に記録する。プローブは製品ファイルから
取り除き、設計段階で red テストを通常 suite に残さない。実アプリは起動しない。

## 5. 推奨: versioned 復元レコードを一つ、main の明示一覧採用でだけ確定

### 5.1 永続 owner

既存 `last_folder` / `last_cursor_*` は legacy の意味で維持し、
**Previous 専用の `startup_list_restore` という一つの Settings record** を追加する案を推奨する。
record は版・target・target に属する cursor hint を同じ型で保持する。runtime の正本も
この record とし、App / bundle / A/B にもう一つの復元先を複製しない。

最小の型の概形（最終命名は実装段階で決める）:

```rust
enum StartupListRestore {
    V1 { target: StartupListTarget, cursor: Option<ListCursorHint> },
}
enum StartupListTarget {
    Unavailable,
    PhysicalList { logical_path: PathBuf, zip_prefix: Option<String> },
    DriveList,
}
// Settings のフィールド欠落 (None) は legacy migration の入口だけ。
// 移行後の「場所なし」は明示 Unavailable。別 bool / version sentinel は足さない。
```

上の target set は §7A の決定どおり、物理フォルダ／本／Drive 一覧だけを対象とする。
合成一覧とそこからの直接読書は、直前の復元可能な明示一覧とその cursor を保持する。
ZIP prefix は filesystem path に連結しない。元 archive path と内部一覧 prefix を別々に扱い、
RAR/7z/LZH/foreign-archive ZIP の変換 backing、EPUB 世代 PDF を保存先にしない。

**既存フィールドを狭義化する案も検討した。** 現在本の reader はないので一見可能だが、
Desktop/Specific の既存 fallback と legacy cursor pairing が変わり、入れ子一覧は PathBuf
だけで表せない。全 reader を一緒に狭義化して「他の startup mode は不変」を破るより、
Previous のみ新 record を読む方が所有・互換契約を明示できる。
legacy 側は current/main-load の既存 compatibility carrier であり、新 record の正本へ
戻し同期しない。恒久的に毎回 legacy→new を読み替える実装は禁止する。

### 5.2 一つの commit point、既存 request が表示意図を運ぶ

新しい独立 bool や `pending_restore_path` を App に置かない。
既存 typed request とその continuation に **最終表示意図**を持たせ、同じ request owner が
scan／ZIP/PDF／変換／パスワード／後続ページ open まで運ぶ。
`auto_fullscreen` は最終表示意図の代用にしない。入口で確定できない画像フォルダ判定だけは
同じ要求の completed scan で解決する。reload は既存 presentation の維持であり、一覧要求と分ける。
具体的な確定点・全 consumer への運搬は §5.2.1 と §5.2.2 の契約とする。

すべての採用元は `commit_main_list_restore(event)`（仮称）へ渡す。
この reducer だけが新 record を変更する。event の作成は、**main の論理 navigation の
最終採用境界**、または main の「現在の一覧を見せる」明示要求の受理境界で行う（§10.3）。
SLI、低レベル `close_fullscreen`、viewport paint、folder path の変更そのものでは確定しない。
ZIP の `zip_nav` を SLI の後に設定する現行順序を保ち、prefix が確定する tail で採用を通知する。
PDF warm placeholder はその request の既存可視採用を使い、実列挙検証で二度確定しない。

| semantic event | 復元 record の扱い |
| --- | --- |
| 一覧 open が成功して main に採用 | target をその一覧へ置換。別 target の cursor を持ち込まない |
| 一覧からページを直接 open / ページ中の本移動 / 読書 resume | target は保持。出発元一覧の cursor は同じ owner に捕捉する |
| Backspace／「一覧へ」等で main の本一覧を明示表示 | 適格な採用済み一覧への要求を受理した時点で、現在本＋確定 prefix を通知。遅延 close の完了を待たず、完了時の再通知もしない。検索結果等へ戻る場合は target/cursor とも保持 |
| Escape 等で親一覧へ戻る | 実際に採用された親一覧の既存 navigation 完了が通知する。途中の本一覧は通知しない |
| load/reload 内部 close、sort/filter/rebuild、thumbnail completion | target を変更しない |
| 採用前の error/cancel/Ignore/Refused/stale result | record を変更しない。rollback 予約を作らない |
| 直接ページ要求が空／失敗で表示できない | 「一覧を明示した」操作へ勝手に読み替えず旧 target を保持。後で利用者が一覧を明示した時だけ更新 |
| F12 linked の移動／独立 viewer 内の open・移動・close・park | record を変更しない。main ownership のイベントを発行しない |

low-level close は navigation、shutdown、F12、reload でも再利用されるので、そこに無条件の
record 更新を置いてはならない。ユーザーの semantic close／一覧復帰 caller に型を渡す。
追加した所有チェックは既存 context identity / `ViewerNavigationScope` を用いる。
現在 active HWND、detached settings bool、fullscreen idx の有無から main owner を推測しない。

F12 linked によって main 側に一覧が描画される場合も、表示先変更の side effect であり
一覧を明示した操作として event を生成しない。この区別があるため、毎フレームの
「一覧が画面にある」判定を commit point にする案は採用しない。

#### 5.2.1 最終表示意図の型と採用単位

概念上の意図は `ExplicitList`（明示一覧）、`PageContinuation`（一覧はページを開くための
内部採用）、`ClassifyFolder`（scan 結果待ち）、`PreservePresentation`（既存表示の再構築）の
相互排他的な型とする。最終命名・保持場所は実装時に既存 request の所有型へ合わせる。
外部から渡す `auto_fullscreen` と新しい bool を二つ並べて正本にしない。既存の typed mode /
exact target / fullscreen continuation を同じ意図へ投影し、legacy bool が必要な consumer にだけ
既存実行用の値を渡す。未分類は ExplicitList とみなさず、結果が確定するまで通知しない。

**一つの操作が物理フォルダ→内部 ZIP→ページの複数 install を行っても、その操作は最後まで
PageContinuation である。** 後から fullscreen が開いたので target を巻き戻す設計にしない。
同期 helper は引数で、非同期 worker／prompt はその既存 request／continuation で意図を運ぶ。
既存 owner の検証を通過して final list の generation / source / prefix が整ってから、
§5.3.2 の current-list projection と ExplicitList を照合して一度だけ commit する。
PreservePresentation、PageContinuation は SLI が何回走っても target の commit を発行しない。
request が消える前に子 continuation へ所有を渡し、App-global の一時フラグや別 pending を足さない。

変換書庫の source／surface 採用前の hydration は、既存 request 内の
`InternalHydration(Box<StartupListIntent>)` が元の意図を所有する。列挙済み ZIP の同期採用でも
その内側の RestoreList を materialization へ投影し、最初から保存 prefix の行だけを作る。
hydration 自体は reducer へ通知せず、logical source と surface が確定してから既存の
列挙 request に元の意図を渡すか、同期採用の semantic tail を一度実行する。
別 pending bool／復元予約や root を表示してから階層を作り直す経路は追加しない。

#### 5.2.2 表示意図の入口・所有者・採用先の棚卸し（P1 対応）

下表は新 record のイベント作成についての契約である。現在の画面を変更する提案ではない。

| 経路とコード根拠 | 意図の確定点と、運ぶ既存 owner / consumer |
| --- | --- |
| 明示 PageList / PageFullscreen、通常 GridVirtual / MainGridArchive | `GridContainerOpenMode` / `GridVirtualOpenEffects` の解決時点で ExplicitList / PageContinuation を決める。`OpenRequestOwner` → classification → physical history / ZIP/PDF / archive convert owner → final adoption が同じ意図を運ぶ。各 loader の false を一覧要求と解釈しない |
| 単体画像等の startup argument / SendTo / activation / bookmark | `src/app/startup_ops.rs:903` の `select_requested_file` と `:1011` の owned load、`:1033` の指定 file open、`:1347`–`:1355` の fullscreen seam は**同じ操作**。`StartupOpenPathOwner` と resolved requested-file 情報から load 前に PageContinuation とする。親 directory の load / Classifying の採用にもその意図を渡し、後段 exact-file open の有無で parent を先に commit しない。directory 明示 open と default startup は別の操作。独立 bookmark viewer (`:1259`) は main イベントを出さない |
| 通常／画像フォルダの auto open | `src/app.rs:25813` と `:50079` の completed scan 分類が必要。画像以外を含む mixed folder は ExplicitList、画像本を直接開く要求は PageContinuation。auto 値だけで決めず、`ClassifyFolder` が同じ request の scan と既存実効設定方針で解決する。既存コードが scan 完了時の設定変更を反映する箇所はその方針も保つ |
| grid の実 Folder／本棚の製本 Folder | `AddressBarNav::Direct` と既存 `CollectionSource`／`RatingSource` が、受理済みの `container_open(auto_fullscreen)` を運ぶ。通常住所・親・履歴からの Direct は ExplicitList。既存の実行 owner／優先順位／auto-open 予約は変更せず、winning consumer が意図を分類 request へ渡す。Rating の実 Folder は既存の採用分岐が自動ページ表示を予約しないため、設定 ON でも実一覧として分類する。設定だけでページ続行と推測しない |
| 読書中 DFS / sibling / SlideshowNext | `FolderNavMode :3623` と `PhysicalHistoryDfsContinuation :450` の fullscreen / resume_slideshow を起点に決める。`:50531` 付近は `Navigation { auto_fullscreen:false }` でも continuation.fullscreen=true なので PageContinuation。`FolderNavPending` / result → staged request.dfs_continuation → `commit_physical_history_transition :29832` → `reopen_fullscreen_after_folder_nav_load :50121` / `DeferredFsReopen` が運ぶ。Grid / SiblingGrid の着地は ExplicitList、Favsearch / Smart の fullscreen=false は適格一覧の採用だけを通知 |
| 読書 DFS が通常 folder に着地し、内部の本へ続く | `src/app.rs:50606` の folder load → `:50638` reopen → `find_fullscreen_nav_target_filtered :50765` → `:50812` 付近の inner book load。`load_folder_nav_target` と inner selection helper に同じ PageContinuation を渡す。intermediate folder、inner ZIP/PDF/converted の全採用で commit しない。`reopen_fullscreen_after_folder_nav_load :50151` の enumerate 再継続も同じ意図。inner open が失敗／空でも明示一覧へ自動昇格させない |
| slideshow / Collection playback / EOF の続行 | SlideshowNext は上記 PageContinuation に resume_slideshow を持つ。Smart は `start_smart_folder_scope_nav` (`src/app/smart_folder.rs:8013`) → typed FolderNavMode の fullscreen を保持。Collection は `CollectionNavigationAction` (`src/app/collection_navigation.rs:40`) の Manual.landing／OuterGrid 対 OuterFullscreen／Slideshow／EOF から request (`:180`) 内で意図を決め、preflight 結果へ渡す。OuterGrid／Manual の一覧着地だけ ExplicitList、読書・再生の継続は PageContinuation。変換 owner と `CollectionArchiveNavigationContinuation` (`:3482`)、auto_fullscreen=false の load (`:3495`)、prepared ZIP/PDF の採用 (`:3601` 周辺)、`DeferredFsReopen` (`:3669`) と reopen tail に同じ意図を運ぶ。outer folder / inner book の全 adoption に適用し、再生再開／動画除外の動作を保つ |
| required-page 移動（Snapshot / similar / bookmark） | `src/app/snapshot_ops.rs:1801` の location load、`:1861` の completed-scan load の**前**に exact target＋FsNavigationPurpose から PageContinuation を決める。archive は `PhysicalHistoryIntent::RequiredFullscreen` (`src/app.rs:29859`)、folder は `FolderOpenScanPurpose::RequiredFullscreenTarget` → `open_required_fullscreen_from_completed_scan`、legacy path は load→`finish_required_fullscreen_load` まで同じ意図。Page open が成功する前の物理 items を一覧として通知しない |
| `FolderOpenScanPurpose::PaneNavigation` | `src/app.rs:5339` / `:49591`。pane の明示一覧 navigation を ExplicitList として `FolderOpenReady` → `ResolvedMainFolderOpen` →通常 load に渡す。他の purpose の default に使わない |
| `FolderOpenScanPurpose::GridFolderCandidate` | `:49597`–`:49688`。main が持つ scan / detached lease は分類まで owner を保つ。scan→image_book / should_detach が確定してから mixed main は ExplicitList、main direct は PageContinuation、独立 image book は main 通知なし。Smart / Collection physical child に再joinする際も解決した意図を渡す |
| `FolderOpenScanPurpose::DetachedFolder / DetachedImage` | `:49696`, detached consumer `:49799` / `:49836`。独立 owner 内の materialize/open であり main イベントなし。画像 filename があっても main folder の commit をしない |
| `FolderOpenScanPurpose::JumpToPhysicalFolder` | `:49703` → `apply_jump_to_physical_folder_ready :49136`。元の場所へ一覧移動する ExplicitList。`JumpToFolderSelection::{None,ExactPath}` (`src/ui_dialogs/context_menu.rs:62`) は**一覧の選択**であり required fullscreen ではない。selection適用後の適格 main 一覧を通知 |
| `FolderOpenScanPurpose::RequiredFullscreenTarget` | `:49707` → exact-page の上記 owner。ExplicitList に落とさない |
| `FolderOpenScanPurpose::CurrentViewOrderRefresh` | `:49722` → `apply_current_view_order_refresh :49732`。order snapshot / typed reload owner に PreservePresentation を運ばせる。通常 Navigation owner を再利用していても新一覧要求ではない |
| 現在一覧の reload / sort / pin / preferences / placeholder verification / sidecar continuation | `current_folder_reload_owner :22886`、`reload_current_view_in_place :22907`、`reload_top_level_grid` (`src/app/top_level_grid_view.rs:1246`) は PreservePresentation。未完了の同一 open の PDF verification／sidecar deferred fullscreen (`src/app/sidecar_restore.rs:145`) は元 request の意図を引き継ぐ。再buildだから ExplicitList を生成することも、deferred-page が残る間に commit することもしない。F5 は評価操作であり reload shortcut とは扱わない |
| explicit 一覧復帰 / search close / A/B / history / default restore | ユーザーの一覧復帰操作と既存 navigation owner が ExplicitList を所有する。最終的に合成一覧なら §5.3.2 で通知しない。Previous の ZIP prefix 復元は §5.3.1 の最終 cursor 適用後が commit。内部 close・F12・tray・quit は一覧復帰 intent を生成しない |

この inventory の各 loader / adoption 呼出しは、Phase 2 で intent の投影と受け渡しをテストする。
「将来 caller が true/false を正しく渡すはず」という前提では閉じない。後段の page continuation を
欠落したら compilation または handler regression で検出できる型／APIにし、製品 entry の既存 behavior を保つ。

### 5.3 cursor、A/B、トレイと startup

target を parent に保つだけでは、現行 `cursor_to_persist` が book の選択を parent の
cursor として捨てる問題が残る。新 record は親一覧の名前／行 hint を**その一覧を離れる前**
に捕捉し、book 読書中の終了では保持する。これは `folder_history`、main source snapshot、
既存選択 hint を利用する。ページ位置を親一覧 cursor として保存しない。
非同期要求で出発時 hint を運ぶ場合も既存 request が所有し、受理しただけでは target を
変えない。§5.3.2 の projection が同じ main target（logical path **と prefix**）を返し、
その一覧を表示中の時だけ、quit/tray は最新 cursor を reducer へ渡す。
synthetic list、ページ表示、内部 loader の途中状態では target と保存済み cursor をともに保持する。
independent context を一時 mount 中でも、cursor と target をその viewer に差し替えない。

`PreservePresentation` は新 target の採用を発行せず、reload の途中は保存 cursor も保持する。
既存の released reload が完了して、既に明示・保存した同じ target の一覧を実際に表示している
場合は、通常の exit／tray／departure がその一覧の最新 cursor を捕捉できる。
直接読書で一度も明示していない別本の reload は新 target に昇格しない。
この区別は同じ target と採用済み一覧表示の照合で行い、新しい状態や固定の fullscreen reopen、
既存の reload 後の見え方の変更は加えない。
legacy cursor 保存判定は他 mode 用に維持する。

A/B それぞれの navigation/history は現状どおり。復元 record は application main に一つ。
slot 切替で新しい一覧が採用された時だけそこを記録し、同じ path の no-load switch も
「一覧を明示した」結果があれば同じ commit event を使う。slot ごとに復元 field を追加しない。
新 record と legacy 値は Preferences OK の live 引継ぎ対象、settings transfer の除外対象にする。

Previous の default startup だけが新 record を解決する。物理 target は既存
`load_folder_or_convert_archive(..., false)`、Drive は `enter_drive_list` を使う。
prefix は §5.3.1 の手順で、その同じ復元 request の最終階層を採用する。
存在しない場所の ancestor / Desktop fallback と cursor の同一 target 照合を保つ。
消えた内部 prefix は外側本の既存一覧へ着地し、内部 cursor は適用しない。
新しい worker、resume/retry/rollback owner を作らない。

Desktop / Specific / Drives / ReadingHistory は今の mode routing、fallback と cursor 条件を維持。
command-line / SendTo / activation / Remote IPC の明示 open をこの default restore で
上書きしない。明示 open の再生・auto fullscreen・password 等の既存仕様を変更しない。
トレイ hide は新 record を保存するだけで読書表示を閉じたり target を再生成しない。
トレイ restore は生きている session を戻し、startup restore を呼ばない。

#### 5.3.1 ZIP prefix 復元の順序と Backspace の契約（P2 対応）

`ZipNavState` の stack は描画した **collapse 済みの実効 prefix** であり、root も collapse
する（`src/zip_tree.rs:275`, `:293`）。`enter("P/Q/")` は一段だけ push する (`:334`)。
これを startup にそのまま使うと Q→root になり、P の親一覧を飛ばす。保存するのは元本の
logical path と現在の実効 prefix だけとし、stack を永続化しない。

Previous の ZIP／ZIP に変換された archive 復元は、既存 request が次の順序を所有する。

1. **prefix の存在検証**: 既存列挙結果の `ZipTree::node_at` (`src/zip_tree.rs:87`) で
   保存 prefix の node を確認する。内部 prefix は tree と同じ segment 表現で扱う。
  filesystem join／Windows の大文字小文字同一視を適用しない。不正な prefix は受理しない。
   不正／消失した prefix は既存の collapsed root 一覧へ着地し、内部 cursor を破棄する。
2. **親階層を tree から再構成**: root の `collapse_redundant([])` (`:146`) を stack の底とし、
   保存 node の canonical な実効 prefix（同じ collapse 規則で解決）まで、現在 node から
   target に向かう**直下の子**を選んで collapse 済み prefix を積む。通常の ZipDir 降下と
   同じ stack にする。各 push は真の子孫であり、重複／wrapper の見かけの階層を積まない。
   root 側の wrapper collapse が旧 prefix を通り越す場合も、canonical target が底と同じなら
   一段の root とする。tree 内で root→target の関係が成立することを確認する。
   これは ZipNavState 内の tree-based constructor／復元 helper として既存 enter/back の
   不変条件を共有し、App から private stack を書き換えない。
3. **最終階層だけを materialize／採用**: 再構成した nav の `materialize_current` (`:389`)
   を既存の items install 経路に渡す。中間の root/P 一覧を順番に表示・commit しない。
   SLI 後に nav を設置する既存 ownership を保ちながら、final prefix と items generation を揃える。
4. **その階層へ cursor を適用**: sort／visible rows が整ってから保存 hint を適用する。
   root の `try_select_after_load` (`src/app.rs:35505`) に内部 hint を先に消費させない。
   `zip_nav_show_current_level` の selection／scroll reset (`:32110`) より後に最終階層の
   hint を適用し、既存のスクロール経路を使う。logical path＋実効 prefix が一致しない
   cursor は捨てる。tree 更新で wrapper collapse が変わり canonical prefix が変わった場合も
   別一覧として内部 cursor を適用しない。legacy migration の prefix 不明 (`None`) は
   §6 の旧 root 復元として扱い、旧 cursor の既存適用条件を保つ。
5. **最後に一度 commit**: final nav／items／cursor が揃い、§5.3.2 の適格性を満たした
   main 一覧だけを reducer に通知する。既存 request の cancel／stale 判定を共有する。

復元直後の Backspace は Q→P→collapsed root→既存の外側親 navigation となる。
途中が単一 wrapper なら通常操作と同じくその見かけの階層を飛ばす。
`current_parent_zipdir_prefix` (`:311`) で親から見た literal な子セルを選び直せることも
維持する。通常の nested ZIP ファイル／foreign archive の logical-source 解決と外側への
戻りは既存経路を再利用する。新 worker、root の余分な表示、永続 stack、復元 journal は不要。

#### 5.3.2 「復元可能な現在 main 一覧」の適格性（P2 対応）

明示一覧 event と cursor 捕捉は、共通の read-only projection
`restorable_current_main_list`（仮称）を使う。返すのは target とその installed items generation
に属する選択 hint であり、新たな runtime owner／sentinel を置かない。
**main ownership、実際に採用された行集合、surface の現在の physical-child position、
logical source、ZIP の実効 prefix** を照合する。items の先頭が物理 path というだけでは
判定しない（空の物理一覧も適格である）。I/O や存在確認はこの projection に追加しない。

`current_folder` だけを使わない。Ctrl+G はそれを残して検索 items を install する
(`src/global_search_ui.rs:1691`, `:1808`)。`current_top_level_restore_snapshot`
(`src/app.rs:21774`, 特に `:21785`) は表示中の一覧より先に `return_to` を返し得るので、
projection の代用にしない。この API は既存の戻り navigation 専用の意味を保つ。

| 実際の一覧／position | projection の結果 |
| --- | --- |
| `TopLevelGridSurface::Folder` | main の installed context と一致する物理 source があり、合成行集合でなければ PhysicalList。local filter は同じ物理一覧の絞り込みなので適格。旧 synthetic の return_to、rating stars、saved_folder を target にしない |
| `DriveList` | 実際の Drive surface と main install が一致すれば DriveList。現行の Drive cursor 保存方針を保ち、残った folder cursor を結び付けない |
| Smart root / Collection root | 合成一覧なので結果なし。real path の item が並んでも物理一覧とは扱わない |
| Smart `Container` / `Scoped`、Collection `PhysicalSource` | typed position の現在 path／source key と installed physical context が一致する実一覧だけ PhysicalList。Collection の `PhysicalSource.path` は親 source のまま内側の本へ移動し得るので、現在採用済みの `effective_folder()` と position の所有関係を照合する。`SmartFolderPosition` (`src/app/top_level_grid_view.rs:21`)／`CollectionGridPosition` (`:395`) と既存採用 authority を使い、root session の snapshot／return_to を使わない |
| Global / Favorite / Tag の検索結果、合成 drill rows | 結果なし。`items_are_global_search_view` 等の installed aggregate-row 情報を含めて判定する。drill.current_path が実 path でも、検索条件で再構築した合成 items は物理一覧ではない |
| Search から実 folder／book に入った physical drill | 現在の drill／nav position と loaded physical source が一致し、合成-row install ではなくその source の一覧を採用した時だけ PhysicalList。`global_search.active` 単独では付与も除外もしない。`advance_drilled_current_path` (`src/global_search_ui.rs:2990`) と物理 install の対応を確認する |
| Rating root / ReadingHistory / Bookmarks / Snapshot | 合成 rows なので結果なし。そこから採用した physical child は、その実際の Folder surface／physical position で上記判定を行う。合成親へ戻るための provenance は保存しない |
| SubfolderExpansion | root／saved_folder が物理 path でも集約 rows は結果なし (`src/app/subfolder_expansion.rs:2348`、`src/app.rs:34806`)。実 source の SLI は `clear_subfolder_expansion_view_state` (`src/app.rs:34537` → `src/app/subfolder_expansion.rs:1829`, `:1847`) で Folder surface に戻る。別途物理子一覧を採用した場合だけその子を判定する |
| ZIP／変換 ZIP の内部一覧 | 上記 physical position に加え、current logical source とその context の zip_nav.tree／current prefix、materialize 済み items が一致する PhysicalList。住所文字列や別 context の nav から prefix を取らない。PDF は内部 ZIP prefix を持たない |
| loading 中の source 混在／別 owner を mount 中 | 未採用の destination を返さない。既存 source が安定して表示中ならその main source の projection だけを利用し、independent items／generation を main に転用しない |

合成行集合の既存 install 情報 (`src/app.rs:35753`–`:35759`) と typed surface
(`src/app/top_level_grid_view.rs:1231`) を一箇所で投影する。判定を個別の close／shutdown
caller の mode guard に散らさない。実装時に不足する source 対応が判明したら既存 install
authority／position の境界を直し、別の「復元してよい」bool を追加して隠さない。

projection が Some であることだけでは target を更新しない。§5.2 の ExplicitList 成功と
組み合わせて初めて commit する。出発元 cursor 捕捉／quit／tray はさらに現在 record と
target 全体（logical path＋prefix）の一致、安定した一覧表示、projection と選択 hint の
items generation の一致を要求する。runtime generation は永続 record に保存しない。
ここでいう一覧表示は main の論理 presentation であり、F12 linked の副作用で main grid が
描画された事実ではない。読書中の表示先変更から cursor 捕捉イベントを生成しない。
従って Ctrl+G 中の終了、search-result ページから Backspace で合成結果へ戻る操作、
サブフォルダ展開、合成 root からの direct page は、直前の physical target **と cursor**を保持する。
Backspace が実際の physical book page list を明示した場合はその一覧を記録する。

### 5.4 簡素化と detached 憲法

- **終了時に既存状態だけで決める案**も比較した。「親→本一覧→ページ」と「親→直接ページ」は
  同じ current book／ページ／履歴状態へ収束するため、終了時に過去の明示一覧を識別できない。
  `last_folder`、現在の auto-open 設定、book_resume、return_to からも区別を復元できない。
  合成 surface と読書中の本移動を含めると、現在本の親を選ぶ方法も仕様に合わない。
  quit の判定を増やすより、成功した明示一覧をその時点で一つの record に記録する設計を採る。
- 保存処理から一覧意図の推測を取り除き、成功した semantic adoption だけを通知する。
  cancel/stale completion は既存 request 判定で到達しないので新しい rollback machinery が不要。
- 変換・パスワード等は既存の modal/admission 制約を再利用する。一般の folder scan を新たに
  modal 化する案は、応答性と既存の置換 navigation を削るため採用しない。
- ウィンドウを閉じて読み直す案も、読書／linked の表示先だけの切替を変えるため採用しない。
  一覧復帰は既存 semantic path を使う。target 更新のために余分な再読込を発生させない。
- 新 record は main startup domain の owner。detached bool / Option、host/placement 保存先、
  viewport recreation、delay/retry/repaint は追加しない。context 境界を誤って共有する BA-7 型の
  混同を commit の入力 ownership で排除する設計であり、detached の症状 guard ではない。
- 実装で detached predicate / viewport 経路へ触れる場合、着手前に設計担当＋独立 reviewer の
  構造合意を取り、触れた範囲・理由を `detached-rework-plan.md` §11 に記録する。
  Phase 1 はその記録・合意を既成事実として追加しない。
- 保存失敗への独自の retry、journal、段階回復は追加しない。既存 Settings 保存／通知の契約を使い、
  新しいまれな failure handling が必要なら設計担当へ戻す。

## 6. リリース済みデータの migration

`last_folder` と旧 cursor はリリース済み（依頼者による明示前提）。新 record が欠落する
設定を初回 load する時に限り、旧値を **そのまま** V1 へ投影する（§7B、2026-10-06 決定）。

1. None → Unavailable、空 path → DriveList、通常／book path → PhysicalList、prefix 不明。
   legacy cursor は同じ旧 path の cursor として一緒に引き継ぐ。
2. 旧 ZIP/PDF path が「見たページ一覧」か「内部ロードしただけ」かを判定する情報は無い。
   auto-open の**現在の設定**や `book_resume`、履歴の存在は過去の一覧閲覧の証拠にならない。
   一律親へ移す／現在 mode で推定する migration はしない。
3. 初回起動の旧復元先は維持する。従って旧版で直接表示して終了した利用者は、更新直後の
   **一度目だけ旧 ZIP 一覧が開き得る**。新しい明示一覧採用以降は新契約で保存する。
4. V1 の存在が移行済みの事実。次回 load、Preferences OK、別設定保存で legacy が新 target を
   上書きしない。DB は Settings の既存一括保存へ V1 record 全体を載せる。
   新規 clean install は V1/Unavailable を初期値とし、legacy JSON の欠落は旧データ入口と区別する。
5. 旧値を削除せず、Desktop/Specific fallback と旧版へ戻した場合の carrier を維持する。
   旧版へ downgrade 後の操作は新 record を更新できない。再 upgrade 時に旧版の最新状態まで
   同期する保証を加えるなら別仕様判断が必要（process version 推定を勝手に増やさない）。
6. settings export に新 record を混ぜない。export/import、Preferences OK、DB roundtrip、
   old JSON bootstrap、backup load の各テストで対応を確認する。

field を狭義化する代案でも 2 の情報不足は解消しない。新 setting を足すだけで migration が
不要になる訳ではなく、欠落時の旧意味を上記のように明示する必要がある。

## 7. 決定済みのユーザー判断（2026-10-06）

### A. 現行の復元対象範囲を維持する（決定済み）

対象は **物理フォルダ／本／Drive 一覧のみ**。Search / Rating / Collection / Smart root /
閲覧履歴／しおり／Snapshot／サブフォルダ展開等の合成 surface の起動復元は追加しない。
合成一覧を表示中、およびそこから本を直接ページ表示している間は、直前の復元可能な
明示一覧 **とその cursor** を保持する。

合成 surface から実際の physical child の一覧を明示した場合は、その物理一覧を記録する。
検索 drill 等は real path の有無ではなく §5.3.2 の実際の行集合／physical position で判定する。
検索結果へ戻るための session、条件、return_to を新 record に含めず、従来の戻り navigation
は維持する。合成 descriptor の追加は今回の範囲外であり、ユーザー判断の再確認は不要。

### B. 初回 migration は legacy の復元先を維持する（決定済み）

新 record が欠落した既存設定は、legacy `last_folder` と対応 cursor を一度だけ投影する。
旧データには一覧閲覧の情報がないため、更新直後の一度目は旧 book のページ一覧に戻る
挙動が残り得ることを了承済み。旧 book path を一律親へ変更せず、過去の明示一覧を推測しない。
以後は新 record を正本にし、新しい明示一覧の採用を §5 の契約で記録する。

A/B の製品判断は解決済み。改訂設計の独立承認は `23d885e11`、受理 caller の追加合意は
`target/r1335d3-review.txt` に記録済み。構造合意と検証結果は §10 と detached plan §11 を参照。

## 8. 回帰設計と影響ファイル

| 対象・scenario | 新仕様の検証と既存機能の不変 |
| --- | --- |
| full feature: ZIP/PDF の page list→page→quit | book 一覧を復元。読書 resume とは独立、startup は auto fullscreen=false |
| full feature: parent→direct page→quit | parent 一覧と選択 book の hint を復元。本のページ名を parent cursor にしない |
| explicit PageList / PageFullscreen の一時 override | 設定 ON/OFF に関係なく入口の実際の意図で決める。表示設定を変えて過去 target を推測し直さない |
| direct page→Backspace／一覧ボタン→quit | 適格な採用済み book 一覧への明示要求を受理した時点で更新。Esc の親戻りは途中 book 一覧を記録しない。close が deferred の場合も受理時に一度だけ更新し、effect 完了では再更新しない。受理直後の quit でも book 一覧を復元 |
| folder book／本棚の製本フォルダ | 直接読書では親、一覧を明示すれば当該実フォルダ。本棚は仮想 sentinel としない |
| F12 linked root↔child、繰り返し／close | page list/direct のどちらも元 target 不変。表示先だけが変わり、F12 移動で一覧を見たことにしない |
| independent main list + 2 viewers | A/B viewer の ZIP/PDF/Folder/converted open、Ctrl traverse、activation、park、close、main quit 中も main record/cursor 不変。sibling context の items/receiver/cache/history を変えない |
| converted archive cache hit / Convert / Ask / Ignore | 元 source＋prefix のみ保存。cache ZIP、EPUB generation PDF は保存しない。password／cancel／error／古い完了は変更なし。既存 warm PDF 即時表示を遅らせない |
| nested ZIP / ZipDir | 外側本の prefix P 一覧→子 Q を direct→quit は P。本 Q の一覧を明示した後は Q。§5.3.1 の順序、nested ZIP／foreign archive cache 両方。復元直後の Backspace は Q→P→root→外側親で、親の子セル選択も維持 |
| ZIP wrapper / 同名 cursor / 欠落 prefix | root と各親に単一 wrapper を持つ fixture で stack は通常の降下と一致し、BS は無限ループ／phantom wrapper なし。root/P/Q に同名ページを置き、Q でのみ hint 適用・scroll。消失 prefix は root へ着地して内部 cursor を破棄、collapse 変更による別 prefix にも旧 cursor を適用しない |
| Collection/Search/Rating/Smart/History/Bookmarks/Snapshot | 合成 root 中と root から direct page 中は直前の物理 target＋cursor を保持。physical descendant の ExplicitList / direct を区別。各親の戻り chain、filter、履歴、root session に変更を加えない |
| Ctrl+G / Favorite / Tag の結果ページ→Backspace、合成 drill | current_folder／return_to に旧物理 path が残る fixture を用意。結果ページから合成結果へ戻っても target/cursor 不変。実 path を持つ filtered drill も除外し、full physical drill の明示一覧だけ採用。quit／tray の両方で確認 |
| サブフォルダ展開 enter／reinstall／restore／direct page→戻り | root／saved_folder が実 path、選択 hint が別名の状態で集約 items を表示しても旧 target＋cursor 不変。展開結果から明示した実 folder／本一覧のみ更新。再構築／終了／tray で合成 cursor を混入しない |
| startup の単体画像 argument / activation | auto_fullscreen=false の親 folder load→指定 file fullscreen でも、途中 folder を commit しない。Classifying／即時 scan 経路、指定 file 消失／失敗も旧 target を保持。明示 folder list の入口は別に commit。引数優先・ページ表示は従来どおり |
| reading DFS / sibling / slideshow continuation | auto_fullscreen=false＋fullscreen continuation の typed request を実 handler に通す。直近の明示一覧を保持し、folder/ZIP/PDF/converted の採用・DeferredFsReopen・再生再開で一度も intermediate commit しない |
| reading DFS の normal folder→inner book | images のない normal folder に ZIP/PDF/converted を置き、folder install→inner book load→page の各 seam で target/cursor を assert。非同期 inner enumerate、空本／cancel／stale にも保持し、従来の継続再生を確認 |
| required-page / Snapshot / similar / bookmark 移動 | location load と completed-scan load、folder と virtual-book を各 handler で通す。exact page を開く前後とも中間一覧 commit なし。required target 不在／失敗を一覧要求に変えない |
| 全 FolderOpenScanPurpose と画像フォルダ分類 | PaneNavigation／JumpToPhysicalFolder は selection 完了後の適格一覧を commit。GridFolderCandidate は mixed／image-book と auto 設定の組合せ、scan 中設定変更、main／independent lease を確認。DetachedFolder／DetachedImage は main 不変、RequiredFullscreenTarget は direct、CurrentViewOrderRefresh は preserve。purpose を default false で同一扱いしない |
| folder Back/Forward / A/B / same-path / no active slot | 採用した一覧だけを記録。失敗／置換／取消で target 不変。A/B histories と slot ownership は既存契約を維持 |
| 現在一覧の reload / sort / filter / pin / preferences / PDF placeholder verification / sidecar | PreservePresentation と新 ExplicitList を同じ物理 source で対比。reload は一覧未表示の book を新 target にしない。reload 中は cursor を保持し、完了して同じ保存済み明示一覧を表示した場合は通常の exit／tray／departure で最新 cursor を捕捉する。別の direct book との対比も置く。cursor と target の世代を混ぜず、元 request の後段 page／verification で2回 commit しない |
| tray hide/restore→quit | hide による target 変更なし。読書／再生表示を保持して restore。session を restart restore で再構築しない |
| Previous 以外 | Desktop/Specific の成功・失敗→Desktop→legacy fallback、Drive、ReadingHistory すべて現状の target/cursor/auto fullscreen を確認 |
| command-line / SendTo / activation / Remote fallback | 明示 file/book/folder open 優先を維持。default restore が結果を上書きしない。同名 path、Ignore/Refused、解決失敗も既存意味 |
| migration・settings | legacy None/path/Drive/book/cursor、欠落新 key、new record 優先、repeated load、Preferences OK、roundtrip/JSON/backup/transfer exclusion。削除・移動済み path の祖先 fallback で cursor を捨てる |

テストは pure target reducer / intent projection を先に、handler-level adoption と temporary
settings DB の exit-restart を次に追加する。単に field を手で代入する test だけにしない。
ZIP は実 fixture、PDF は既存の completed enumerate handle／prepared page payload を使う
（lib test exe は PDF worker entry を持たない）。変換・stale/取消は既存 request injection を利用する。
F12 linked と独立窓は既存 headless multiwindow context tests に sibling 不変 assertion を足す。
操作自体の順序と owner を検証し、単なる duplicate assertion／UI snapshot だけにしない。
ZIP stack の導出／適格性 projection は pure tests に加え、deep-prefix の Previous startup→
Backspace と、上表の実 request→adoption→page の handler tests で検証する。
「親→本一覧→ページ」と「親→direct page」の現在本／ページが同じでも、新 record は
それぞれ book／parent になる対比を置く。各中間 seam の record を確認して、最終結果だけ
同じにする巻き戻し実装が通らないようにする。

2026-10-06 現在、`src/app/tests/startup_restore.rs` には上記の handler／状態／exit-restart
回帰を初回 52 test case として実装し、P2 対応で Collection sidecar ON/OFF の実 OuterGrid 経路と
動画リングの実 handler → exit/restart の2件を追加した。追加 caller 合意後は native／egui の3入力、
parked 除外、通常親戻りの5件を追加して、同ファイルは59 test case となった。
元の direct-ZIP red は `src/app/tests.rs` に保持し、
settings／DB／transfer、ZIP tree、変換／EPUB、headless multiwindow にも対応する回帰を置く。
初回結果は §10.4、P2 対応の結果は §10.5、追加 caller は §10.6 に集約する。
初回の bounded completion review は、Folder carrier／Rating の実行分岐による
分類／password owner のサイズ修正まで確認し、未解消 blocker なし（`target/1335-implementation-review.txt`）。
自動 gate の実行所有は implementer で、reviewer は実アプリを起動していない。

想定 ownership / file scope:

Phase 2 は repository の bounded Sol handoff 方針に従い、root implementer が共有 App と
全体検証を所有し、変換、専用回帰、対応 docs を範囲の決まった Sol agent へ分担する。
**各ファイルの writer は一人**とし、担当を替える時は safe checkpoint と差分を引き継ぐ。
独立 reviewer は別 context で read-only の設計・completion review を行う。
実装は `gpt-6.1-sol / high`、独立 review は `gpt-6.1-sol / xhigh` の指定を使い、
追加の Astra coordinator は置かない。分担は必要な coherent chunk に限る。

- `src/settings.rs`, `src/settings_db.rs`, `src/settings_transfer.rs`: record・legacy projection・保存／除外・live 引継ぎ。
- startup domain の新 module（候補 `src/app/startup_list_restore.rs`）: reducer と唯一の new record writer。
- `src/app.rs`, `src/app/startup_ops.rs`: 既存 request 意図の運搬、物理／ZIP／PDF の採用、明示一覧復帰、Previous の消費、legacy cursor との分離。
- `src/app/snapshot_ops.rs`, `src/app/top_level_grid_view.rs`, `src/app/subfolder_expansion.rs`:
  required-page／reload の intent と実 surface／position／installed rows の projection seams。
- `src/zip_tree.rs`: 列挙済み tree に基づく復元 stack の導出と通常 enter/back 不変条件の共有。
- `src/app/smart_folder.rs`, `src/app/collection_grid.rs`, `src/app/collection_navigation.rs`,
  `src/global_search_ui.rs`, `src/ui_dialogs/archive_convert.rs`: owner を運ぶ必要のある adoption seams。
  §7A に従い synthetic descriptor と起動 dispatch の拡張は行わない。
- `src/ui_fullscreen.rs`, main/key/gamepad dispatch: 必要なら existing semantic 一覧復帰 caller の
  意図伝達だけ。新 key / KeyAction は追加しない。実装前に keymap docs を読む。
- `src/app/tests.rs`, `src/app/multiwindow_scenario_tests.rs`, module tests:
  上記の状態／handler／settings 回帰。
- `docs/virtual-folders.md`, `docs/folder-history-location-plan.md`, `docs/architecture-overview.md`,
  本 plan、必要時 detached §11、manual の startup 説明: 実装時に意味と所有境界を追記。

Phase 2 の検証担当は root implementer。narrow filtered lib tests → owner 横断回帰 →
利用者指定の full lib と `scripts/test-full.ps1`（workspace／統合／doc／補助 bin／vendor）、
normal／portable／portable+test-script check、fmt、必要な glyph check を
集約し、重い Cargo command を並行実行しない。成功後 `scripts/build-dev.ps1` で未起動の
確認 binary を用意する。現在の依頼は check／build までで、実アプリを起動・操作しない。
将来の実アプリ suite はシナリオ・時間・desktop/input・disposable data を提示した明示了承後のみ。
normal profile binary をエージェントが launch しない。Phase 1 は docs/test probe のみなので
確認 binary は不要。Phase 1 の証跡だけを修正済み／全体 gate 成功として扱わない。

## 9. Phase 1 証跡

- 対象: HEAD `1f2f6d5c0` の製品コード＋一時的な設計プローブのみ。
- command: `cargo test -p mimageviewer --lib section1335_design_probe_direct_zip_exit_restart_keeps_explicit_parent_list -- --nocapture`
  （通常 feature set / test profile、temporary data_dir、実アプリ未起動）。
- 結果: command runner exit **1**、**0 passed / 1 failed / 0 ignored / 10,700 filtered out**。
  初回 compile 4分14秒、test 0.50秒。timeout／worker エラーではなく、最後の期待値での red。
- 通過した確認: parent が旧 last_folder に保存 → 実 ZIP direct page が fullscreen に到達 →
  exit / reload で book が保存 → default startup が book を開く → fullscreen=None。
  その後の新仕様 assert は実際 `books/direct.zip`、期待 `books` で失敗した。
- ログ: `target/1335-red-test.log`。再利用可能なプローブ: `target/1335-red-probe.rs`
  （`src/app/tests.rs` の test module の末尾へ追加するコード）。
- `src/app/tests.rs` は検査付きで元のバイト列へ復元した。
  復元 SHA-256: `0dbb5a7f9a7dfb2954f9d43a9fa507c064f7bce6d8bd5443f449a7a070bd3730`。
  通常 suite に red テストを残していない。
- 全体 gate／確認 binary は未実施。今回は設計・文書のみで製品コード差分がないため不要。
  設計の検証済みを製品修正の検証済みと取り違えない。

初稿の独立レビューは `target/r1335d-review.txt`（HEAD `02b301b8d`、read-only）で
**revise**。本改訂は P1 を §5.2.1／§5.2.2、ZIP の P2 を §5.3.1、適格性の P2 を
§5.3.2 と各 §8 handler 回帰へ反映した。終了時推測案との比較は §5.4、ユーザー決定 A/B は
§7 に記録した。改訂の独立再レビュー・実装・detached §11 の構造合意記録・ユーザー実機確認は
未実施だった。以上は Phase 1 改訂時点の履歴であり、後の承認・Phase 2 の状況は冒頭と §10 を参照。

## 10. Phase 2 の安全な区切りと viewport 完了経路への追加合意事項

### 10.1 変更前に停止した理由と具体的な経路（当時の記録）

今回のユーザー指示は「detached predicates / viewport paths に触れる必要がある場合は、
変更前に停止・報告し、design owner + reviewer の合意後に §11 へ記録する」。
main ownership の設計承認は有効だが、以下の terminal close の変更範囲まで承認済みとは扱わない。
`docs/detached-rework-plan.md` §2 の構造合意条件も併せて適用する。

`23d885e11` の実コード:

1. `src/ui_fullscreen.rs:35565` の Backspace（ページ一覧へ）は `:35581` の
   `close_fullscreen()` を呼ぶ。呼び出し直後の commit は閉じ終えたことを証明しない。
2. `src/app.rs:68481` の `close_fullscreen()` は、Windows の表示遷移中には
   `:68489` の `PresentationTransitionEvent::TerminalClose` を配送して return する。
3. `src/app/presentation_transition.rs:191`, `:234`, `:1078`, `:1149` の既存 owner が
   `TerminalSessionClose` effect を生成し、`src/app/native_video.rs:3035` の effect consumer が
   実際の通常 close を後段で実行する。ここは main／detached の動画表示遷移・viewport 完了経路。
4. `src/app.rs:68287` の一般 close は auto-open 設定により親への navigation を予約する。
   途中の本一覧を記録せず、親採用の semantic completion に任せる契約を維持する。

当時は §5.2 の「main の明示操作の完了境界で確定」を実際の close 完了と解釈し、明示一覧へ戻る意図を
既存の close request → terminal effect → 実際の close 完了まで運ぶ必要があると判断した。
この解釈は §10.3 の受理境界への変更により不要となった。polling、新しい App pending bool、低レベル close での無条件 commit、
`close_fullscreen_to_completion` を Backspace に流用して既存の遅延 timing を変える案は採用しない。

### 10.2 terminal close purpose 案（不採用）

停止時には、明示一覧復帰の purpose と main owner／世代を既存 terminal event/effect に載せ、
実際の teardown 完了後だけ record を更新する案を提示した。これは「実際の close 完了まで
記録しない」という旧解釈を守るための案であり、実装していない。

利用者が §10.3 の受理時記録を了承し、独立 reviewer も
`target/r1335d3-review.txt` で承認したため、この運搬は不要と判断した。
terminal event/effect、`TerminalSessionClose` consumer、presentation transition reducer に
新しい purpose／完了 tail を加えない。既存の close timing、lease、native teardown、focus、
promotion は従来の owner が処理する。

### 10.3 受理境界での簡素化案（2026-10-06 合意）

利用者・設計担当（ClaudeCode）・独立 reviewer の合意は、**main が既に採用した適格な
一覧を明示表示する要求を受理した時点で、起動復元先を記録する**こと。
`target/r1335d3-review.txt` は、この所有・確定境界の変更を detached の症状パッチではないと
確認した。§7 A/B の対象範囲と migration 判断は変えない。

- semantic caller は、modal による拒否、別 action への配送、親へ戻る navigation の分岐を
  済ませてから共通 helper を呼ぶ。即時 close と遅延 close は同じ受理境界を使い、
  terminal effect の完了時には record を更新しない。
- main identity と現在採用済みの surface／physical-child position、logical source、ZIP の
  実効 prefix を §5.3.2 の projection で照合する。合成 root、independent context、
  未採用の列挙先、holdover の表示を採用済み main 一覧の証拠にしない。
  変換書庫は元 source、Collection 内の本は現在の `effective_folder()` を使う。
- `src/app/native_video.rs` の **`FsBackToList` semantic caller** も同じ helper へ接続する。
  「native_video を一切変更しない」とは扱わない。terminal effect consumer／表示遷移処理は
  変更しない。`src/ui_fullscreen.rs` の既存 Backspace caller も同じ契約を使う。
- gamepad B の通常 close は auto-open 設定により親へ戻る場合がある。
  その場合は途中の本一覧を記録せず、親 navigation の実際の採用完了に任せる。
- cursor は受理前の `selected` をそのまま保存せず、通常 close が同期する閲覧中ページ／
  編集 anchor と同じ選択先から導出する。ファイル名スタックの flat 行は戻る集約行と
  一致しないため、不確かな hint は空にする。ページ名を親一覧 cursor にしない。

**了承した境界の差**: 要求を受理してから遅延 close が終わるまでに終了した場合でも、
次回起動は受理済みの本一覧を復元する。実際に一覧が描画されるまで記録を遅らせる保証は
設けない。新しい rollback／supersession／完了待ち状態を加えず、既存の modal 排他と
採用済み一覧の証拠を使う簡素化として採用する。

必須回帰は、即時／遅延 close の受理直後に更新され effect 完了で再更新しないこと、
親戻り・入力拒否・合成一覧・independent context で保持すること、受理直後の quit、
閲覧中ページ／編集 anchor の cursor と flat stack の hint を含む。
触れた範囲と構造合意は `docs/detached-rework-plan.md` §11 に記録する。

### 10.4 現在の差分と検証の扱い

- permanent red: `section1335_direct_zip_exit_restart_keeps_explicit_parent_list`。
  Phase 2 の最初に既存実装で失敗を確認した（0 passed / 1 failed、実際は ZIP、期待は親）。
  legacy `last_folder` の book 保存は引き続き期待し、Previous の復元先だけを親とする。
- 保持した準備: versioned Settings record、released `last_folder` と cursor の一度だけの移行、
  通常 load／JSON 移行への接続、Preferences OK の live 引継ぎ、設定 transfer 除外、
  列挙済み tree から wrapper collapse を考慮した ZIP prefix の親 stack 再構築。
  保存失敗の retry／journal／recovery は追加していない。
- 停止時の navigation 途中配線は `target/1335-navigation-wip.patch` と
  `target/1335-startup-list-restore-wip.rs` へ退避していた。再開時に各入口・continuation・採用 tail を
  再確認して製品差分へ接続しており、退避ファイルを検証済み完成パッチとして扱っていない。
- 現在の作業範囲: Previous の record 消費、明示一覧／ページ続行／reload の typed intent、
  scan／ZIP／PDF／変換／password／sidecar／history／Collection／Smart の既存 owner への運搬、
  source／prefix／cursor の最終採用、§10.3 の受理 caller、main／independent の隔離回帰。
  関連 spec／virtual-folders／manual は新しい利用契約に更新する。
- 実装・自動回帰確認は完了。準備部分の成功と、下表の最終差分の自動 gate／構成別 check／
  build-dev 完了を区別する。利用者の実機確認は未実施で、実アプリは起動していない。
- 停止時点の準備差分の確認: 新規 filtered 6 件、settings DB 129 件、transfer 16 件、ZIP tree 50 件が
  すべて成功。transfer の全 field 分類数は追加前 440／追加後 441（出す 131／除く 310、wire 129）。
  fmt／diff check／glyph lint も成功。当時の red 再実行は最後の ZIP 対 parent assert で
  0 passed / 1 failed、10706 filtered out（0.36 秒）。全体成功や修正完了を意味しない。
  当時は normal／portable／portable+test-script check も全経路実装後の gate として残っていた。

#### 最終差分の自動検証（2026-10-06）

初回 full lib は 10706 passed / 1 failed / 52 ignored。唯一の失敗は既存
`history_transition_storage_keeps_app_stack_footprint_bounded` で、追加した Legacy password intent が
owner を 88 bytes に増やしたことだった。Legacy variant 内で意図を Box に置き、既存条件を変えず
16 bytes へ戻した。失敗ログは `target/1335-full-lib-before-box.log` に保持する。

実操作による本棚 Folder direct 読書と、Rating Folder auto 要求の実一覧着地もそれぞれ red を確認し、
既存 navigation carrier と最終 scan 分類を修正した。ログは `target/1335-bookshelf-red.log`、
`target/1335-rating-folder-red.log`。既存の実行 owner、表示 timing、modal／優先順位を変えず、
Rating Folder は従来の一覧着地を維持する。設定値だけから page continuation と推測しない。

| 確認（最終 source freeze 差分） | 結果・ログ |
| --- | --- |
| `cargo test -p mimageviewer --lib section1335 -- --test-threads=1` | 59 passed / 0 failed。`target/1335-final-narrow.log` |
| 既存 App / password owner サイズ回帰 | 1 passed。`target/1335-final-stack.log` |
| archive conversion tests | 8 passed。`target/1335-final-archive.log` |
| settings tests | 277 passed / 12 ignored。`target/1335-final-settings.log` |
| settings DB tests | 129 passed。`target/1335-final-settings-db.log` |
| settings transfer tests | 16 passed。`target/1335-final-transfer.log` |
| ZIP tree tests | 50 passed。`target/1335-final-zip.log` |
| normal / portable / portable+test-script core check | いずれも exit 0。`target/1335-final-{normal,portable,portable-test-script}.log` |
| `cargo fmt --all -- --check` | exit 0。`target/1335-final-fmt.log` |
| `git -c core.safecrlf=false diff --check` | exit 0。`target/1335-final-diff.log`。当該コマンドだけ改行警告を抑制し、repo 設定は変更していない |
| `python scripts/check_ui_glyphs.py` | 危険な UI glyph 0。`target/1335-final-glyph.log` |
| `cargo test -p mimageviewer --lib` | 10710 passed / 0 failed / 52 ignored、764.79 秒。`target/1335-full-lib.log` |
| `.\scripts\build-dev.ps1 -PreserveRuntime` | exit 0。normal feature set の core／remote service／EPUB PDF worker を配置し、runtime=4 / pe=3 の検査成功。`target/1335-build-dev.log`。製品・確認 binary は起動していない |

集計と実行 command は `target/1335-final-gates.json` に保持する。
full lib の PowerShell runner 終了値は 1 だったが、先頭の stderr warning が
`NativeCommandError` として扱われたためで、末尾は全件 `test result: ok`、Cargo の後続エラーもない。
同じ warning／リダイレクトを狭いサイズ回帰で再現し、明示的に捕捉した `$LASTEXITCODE` は 0
（`target/1335-full-runner-probe.log`）。テスト失敗と混同せず、完走した full lib の結果を再利用する。
利用者の実機確認と設計担当の検収は未実施。通常 profile と実データを使う確認は利用者へ引き渡す。

#### 利用者への確認手順

インストール版／トレイ常駐中の mImageViewer を閉じてから、worktree で実行する。
single-instance mutex を共有する。確認 binary は通常の `%APPDATA%\mimageviewer` を使い、
起動すると実際の設定／利用データを移行・更新し得る。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

1. 起動場所を「前回終了した場所」、フル機能の本表示を直接ページ表示にする。
   親の本一覧を開き、ZIP を直接読んで終了・再起動する。親一覧と選択した本へ戻る。
2. 同じ本を直接読み、ページを移動して Backspace でページ一覧を明示し、終了・再起動する。
   本のページ一覧と対応する選択へ戻る。PDF／変換書庫でも同じ違いを確認する。
3. ネスト ZIP の内部一覧を明示して終了・再起動する。同じ階層へ戻り、Backspace が親階層へ進む。
   F12 の表示先切替や独立窓内の読書で main の復元先が変わらないことも確認する。

更新後の最初の起動だけは、決定 B に従い旧設定の復元先を引き継ぐ。上記は親一覧を新たに
明示するところから確認する。起動引数なしで再起動し、トレイ退避を実終了と取り違えない。

### 10.5 独立実装レビュー P2 対応（2026-10-06）

基準は `83d2ac3c1`、指摘は `target/r1335i-review.txt`。初回の full lib 成功は有効な
証拠として保持するが、workspace／統合／doc／補助 bin／vendor を含む `test-full.ps1` の
完了を代替しない。今回の差分は未コミット、製品・確認 binary は起動しない。

- Collection `OuterGrid` の同期 ZIP load はまだ `Root` の間に採用を通知していた。
  sidecar OFF ではその通知が適格性 projection に拒否され、後の `PhysicalSource` 採用後に
  通知がなかった。Folder／ZIP／prepared PDF の内部 loader には既存の
  `InternalHydration(original)` を渡し、物理子採用後の一箇所で元の intent を通知する。
  sidecar ON は既存 Live continuation にその intent を渡し、後段の完了が確定する。
  conversion／PDF failure の非同期 owner は元の intent を保持し、別の完了状態は追加しない。
- 動画リング `CloseFullscreen` は明示的な現在一覧への復帰なので、既存
  `close_fullscreen_to_page_list` に接続する。gamepad B と Image ring の通常 close は
  親 navigation へ進む分岐を保ち、途中の本一覧を記録しない。
- 再監査は `target/1335-close-caller-audit-2.txt`。基準差分の製品 raw close 49箇所を
  semantic／navigation／内部処理／failure／independent に分類した。動画キー入力の
  `VideoCloseFullscreen` と source-swap 待ちの native close 入力2箇所にも迂回があった。
  具体的な helper 接続案は `target/1335-extra-close-callers-proposal.patch`、独立構造レビューは
  `target/1335-p2-extra-caller-review.txt`。native 2箇所は既存 §11 の FsBackToList 限定合意を
  越えるので、設計担当との追加合意前には変更しなかった。その後の合意・適用は §10.6。
  terminal effect consumer、世代 gate、
  parked 除外、presentation／viewport の処理は変更対象にしない。

追加回帰は実際の Collection Ctrl+↓ → `OuterGrid` → ZIP の sidecar ON/OFF 対比と、
動画 direct → 実 ring handler の close → exit/restart。Collection は通常フレームと同じ
ナビゲーションロック解除を通してから cursor を変更し、原設定を一時 profile に保存して
再起動まで検査する。Grid ring の無操作、親戻りの既存回帰も保持する。

#### P2 対応の検証

元の実装に対し、Collection の OFF ケースは親一覧対 ZIP の target mismatch で red
（0 passed / 1 failed、Cargo exit 101、`target/1335-2-collection-red-target.log`）。
動画リングも以前の親一覧対現在の動画フォルダで red
（0 passed / 1 failed、Cargo exit 101、`target/1335-2-ring-red.log`）。
Collection の最初の ON 試行は対象指摘へ届く前に cursor で失敗したため、通常フレームの
lock 解除と startup sidecar 完了待ちを fixture に補い、保存直後のメモリ／DB cursor も検査した。
その fixture 不備のログを製品側の red と混同しない。

| 確認（`83d2ac3c1` + P2 対応の未コミット差分） | 結果・ログ |
| --- | --- |
| §1.335 filtered lib | 61 passed / 0 failed。`target/1335-2-narrow.log` |
| Collection navigation / gamepad input tests | 37 / 38 passed。`target/1335-2-{collection,gamepad}.log` |
| settings / settings DB / transfer tests | 277 passed + 12 ignored / 129 passed / 16 passed。`target/1335-2-{settings,settings-db,transfer}.log` |
| normal / portable / portable+test-script core check | いずれも exit 0。`target/1335-2-{normal,portable,portable-test-script}.log` |
| fmt / diff check / glyph lint | exit 0、危険な UI glyph 0。`target/1335-2-{fmt,diff,glyph}.log` |
| 全体 gate 用 release core / remote / EPUB worker | 3本とも exit 0。core は同一 TurboJPEG ソースの worktree 内コピーと `--jobs 1` を使用。`target/1335-2-release-core-isolated-serial.log`、`target/1335-2-release-{remote,epub}.log` |
| `test-full.ps1 -SuppressCrashDialogs` | exit 0、1177.38秒。workspace 11794 passed / 0 failed / 58 ignored（full lib 10712 passed / 52 ignored を含む）、vendor egui / egui-wgpu / eframe は 25 / 9 / 18 passed。`target/1335-2-full-final.log` |
| `build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | exit 0、91.45秒。normal feature set の core／remote／EPUB worker を配置、runtime=4 / pe=3 の検査成功。製品は起動していない。`target/1335-2-build-dev.log` |

今回の Cargo exit は stderr を PowerShell 内でリダイレクトせず、外側の subprocess が直接捕捉する。
実行 command／feature／profile／所要時間／終了値は `target/1335-2-gates.json` に集約する。
全体 gate の前提である release core／remote／EPUB worker は欠落していたので、CLAUDE.md の
3 command で先に build した。最初の core は TurboJPEG の CMake/MSBuild 24ジョブ build で
exit 101 となり、worktree 内の同一ソースコピーも24ジョブでは同じ失敗だった。
コピー先を単一ジョブで native build すると成功し、同じ Cargo path override と `--jobs 1` で
release core が成功した。コピーの関連ソース325ファイルと Cargo.lock の一致を
`target/1335-2-dependency-copy-check.json` に記録した。設定ファイルや lock を書き換えず、
通常データを借りず、製品を起動していない。これは build 環境への対応で、製品の保存失敗に
retry／journal／recovery を追加するものではない。

全体 gate の初回はテスト実行前の workspace compile で Windows os error 1455
（ページング容量不足による rlib mmap 失敗、後続の compiler 診断）になり、exit 101。
`target/1335-2-full.log` を保持する。テスト失敗や未実行 vendor の成功として扱わない。
同じ `test-full.ps1 -SuppressCrashDialogs` を子プロセス限定の `CARGO_BUILD_JOBS=2` で
再実行し、コンパイルを通過した。対象／feature／test profile／テスト thread 設定を省略・変更せず、
ビルド並列度だけを抑えた。条件は `target/1335-2-full-conditions.json`。

この再実行の full lib は 10712 passed / 52 ignored（826.13秒）、workspace の他ターゲットも
Susie 以外は成功した。Susie 統合だけは3件が前提条件で失敗した。worktree の
`testdata/susie-plugins/extracted` には README しかなく、retro-images の実サンプルもなかった。
テストが文書化している `MIV_TESTDATA` に既存の `C:\home\mimageviewer\testdata` を指定し、
実プラグイン9個をロードした Susie 統合8件が成功（`target/1335-2-susie-fixtures.log`）。
通常のアプリ profile は使わず、サンプル／プラグインを読み取るだけで、期待値や skip 条件は変更していない。
同じ全体 script をこの条件で再実行し、workspace／統合／doc／補助 bin／vendor を含む
全体 gate が exit 0 で完了した。実行条件は `target/1335-2-full-conditions.json`、
Cargo suite ごとの集計は `target/1335-2-full-suites.json`。子テスト harness の結果を二重計上せず、
workspace と vendor の合計は 11846 passed / 0 failed / 58 ignored。
最終 gate 後、normal profile の確認 binary を build した。常駐製品を停止せず、
既に完了した native build の待機を省略しただけで、portable／test-script feature は付けていない。

指摘2件の差分に対する独立レビューは `target/1335-p2-implementation-review.txt`、未解決の指摘なし。
この時点では追加 close 呼出し3箇所の案は独立構造レビュー済みだが、設計担当の追加合意は未取得。
当時の detached §11 は FsBackToList caller に限定されるため、この区切りでは native の変更や
§11 の承認記録を追加しなかった。後続の設計担当合意と適用は §10.6 に記録する。
利用者の実機確認と設計担当の検収は未実施。

#### 今回の手動確認

§10.4 の起動 command と通常 profile の注意事項に従い、起動場所を「前回終了した場所」にする。
更新後初回の旧値移行を経た後、物理親一覧を明示してから確認する。終了はトレイ退避と区別し、
再起動時にファイル引数を付けない。

1. Collection に登録したフォルダから Ctrl+↓ で ZIP のページ一覧を開き、ページを選択して
   実終了・再起動する。同じ ZIP 一覧と選択ページへ戻る。sidecar 読込 OFF／ON の両方で確認する。
2. 別フォルダの動画を直接表示し、動画リングの閉じる操作で一覧へ戻して実終了・再起動する。
   その動画フォルダの一覧へ戻る。gamepad B の親へ戻る動作も従来どおりか確認する。

### 10.6 追加動画 close caller の合意・適用（2026-10-06）

設計担当（ClaudeCode）は `target/1335-extra-close-callers-proposal.patch` に合意した。
独立 reviewer の構造同意は `target/1335-p2-extra-caller-review.txt` に保持する。
native source-swap 待ちの `Window(CloseRequested { generation })` と
`CloseFullscreen { generation }`、egui `handle_video_input` の受理済み `close_video` は、
動画リングと同じ「動画をその一覧へ戻す」semantic action である。
この3 caller の raw close だけを `close_fullscreen_to_page_list` へ置き換えた。

native の既存 committed generation gate と parked 除外、egui keymap consume、
terminal event/effect consumer、presentation transition、viewport／predicate は変更しない。
当初の FsBackToList 限定記録を detached §11 で今回の native／egui caller 名へ更新し、
受理要求の所有境界へ接続する構造的修正である理由と双方合意を明記した。
新たな保存失敗 retry／journal／recovery は追加しない。決定 A/B と released `last_folder` の
初回移行、他の起動設定および legacy consumer は今回も変更しない。

headless 回帰は `section1335_extra_video` の5件。直前の明示一覧 A から別動画フォルダ B を
PageContinuation として採用して直接表示し、実 native source-swap poll の2イベントまたは
実 egui keymap入力で close し、保存・再起動後の B 一覧と動画名 cursor を検査する。
native の2種類は pending output の committed generation を5、fs_cache 側を0とし、
generation 4の拒否後に5の受理を対比する。parked の両イベントでは owner 不一致と一致後の
既存除外を確認し、main record と実表示は変えず pending の退役だけを維持する。
通常 close の親 navigation も保存・再起動まで追加確認する。

イベント注入は既存 `NativeUiProbeForTest` の test-only `send_event` 一箇所を使い、
production と同じ event bus を通す。実 decoder や native window は作らず、拒否後の poll は
既存 navigation debounce の fixture を保持する。製品の時間窓・状態・入力条件は追加しない。
修正前の red は2 passed / 3 failed、3件とも A 対 B の target mismatch
（`target/1335-2-extra-close-red.log`）。parked と通常親戻りは修正前から成功している。

追補の独立実装レビューは `target/1335-p2-extra-implementation-review.txt`、修正要求なし。
最新 source freeze は `target/1335-2-extra-source-snapshot.json`。full lib・確認 build の最終結果は
親担当の gate とし、source review を実機検収と取り違えない。

| 確認（§10.5 + 合意済み3 caller と headless 回帰） | 結果・ログ |
| --- | --- |
| `cargo test -p mimageviewer --lib section1335 -- --test-threads=1` | 66 passed / 0 failed。`target/1335-2-extra-narrow.log` |
| normal / portable / portable+test-script core check | いずれも exit 0。`target/1335-2-extra-{normal,portable,portable-test-script}.log` |
| fmt / diff check / glyph lint | exit 0、危険な UI glyph 0。`target/1335-2-extra-{fmt,diff,glyph}.log` |
| `cargo test -p mimageviewer --lib -- --test-threads=1` | exit 0、10717 passed / 0 failed / 52 ignored。テスト1683.97秒、command 全体1686.49秒。`target/1335-2-extra-full-lib.log` |
| `build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | exit 0、122.43秒。normal feature set の core／remote／EPUB worker を更新し、runtime=4 / pe=3 の検査成功。`target/1335-2-extra-build-dev.log`。製品・確認 binary は未起動 |
| 最終文書を含む diff check／source 一致 | exit 0。`target/1335-2-extra-final-diff.log`、`target/1335-2-extra-source-validation.json`。Rust source、HEAD、Cargo.lock は検証対象と一致 |

今回の gate は利用者指定の full lib を再実行して完了した。先行する `test-full.ps1` は1177.38秒かかり、
安価な確認ではないため、今回の3 caller 差分で workspace／vendor を再実行したとは扱わない。
§10.5 の全体 gate は同節の差分の証拠として保持し、今回の full lib の結果を別に記録する。
ビルド並列度は子プロセス限定 `CARGO_BUILD_JOBS=2`、full lib の fixture は文書化済みの
`MIV_TESTDATA=C:/home/mimageviewer/testdata`、test thread は1で、normal feature set を使う。
最新の全体実行・終了値・所要時間は同じ `target/1335-2-gates.json` へ追記する。
指定された追補 gate と normal 確認 build は完了。コミットせず、製品・確認 binary は起動していない。
利用者の実機確認と設計担当の検収は未実施。

利用者の追補確認では、起動場所を「前回終了した場所」にして以前の物理親一覧を明示した後、
別フォルダの動画を直接表示する。動画窓の ×、動画を閉じて一覧へ戻る操作、同操作へ割り当てた
キーのそれぞれで一覧へ戻し、実終了・引数なし再起動でその動画フォルダと選択動画へ戻ることを
確認する。動画の前後切替直後にも同じ操作を確認し、通常の親戻りは親一覧へ戻ることを確認する。
確認 binary と通常 profile の注意事項・起動 command は §10.4 に示す。実機確認は未実施。

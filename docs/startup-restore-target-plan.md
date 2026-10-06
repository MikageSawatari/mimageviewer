# §1.335 起動時に復元する明示一覧の所有設計

2026-10-06、Phase 1（設計のみ）。製品実装・コミット・実アプリ検証は行わない。
設計担当への提案であり、独立レビュー済み／構造合意済みとは扱わない。

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

上の最小 target set は既存 startup の復元可能対象を保つ案であり、合成親一覧を再開する
仕様まで無条件に満たすものではない。§7 の判断を先に確定する。
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
既存 `GridContainerOpenMode`、`GridVirtualOpenEffects`、`PhysicalHistoryIntent`、Smart /
Collection の typed request に、必要な **最終表示意図**（一覧を明示／ページを表示）を持たせ、
同じ request owner が ZIP/PDF/変換/パスワード継続から採用まで運ぶ。
settings mode を毎回再判定するのでなく、入口で解決したユーザー操作の意図を用いる。
reload は既存 presentation の維持であり、新しい明示一覧 open ではない。

すべての採用元は `commit_main_list_restore(event)`（仮称）へ渡す。
この reducer だけが新 record を変更する。event の作成は、**main の論理 navigation の
最終採用境界**、または main の「現在の一覧を見せる」明示操作の完了境界で行う。
SLI、低レベル `close_fullscreen`、viewport paint、folder path の変更そのものでは確定しない。
ZIP の `zip_nav` を SLI の後に設定する現行順序を保ち、prefix が確定する tail で採用を通知する。
PDF warm placeholder はその request の既存可視採用を使い、実列挙検証で二度確定しない。

| semantic event | 復元 record の扱い |
| --- | --- |
| 一覧 open が成功して main に採用 | target をその一覧へ置換。別 target の cursor を持ち込まない |
| 一覧からページを直接 open / ページ中の本移動 / 読書 resume | target は保持。出発元一覧の cursor は同じ owner に捕捉する |
| Backspace／「一覧へ」等で main の本一覧を明示表示 | 現在本＋確定 prefix を同じ commit point へ通知 |
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

### 5.3 cursor、A/B、トレイと startup

target を parent に保つだけでは、現行 `cursor_to_persist` が book の選択を parent の
cursor として捨てる問題が残る。新 record は親一覧の名前／行 hint を**その一覧を離れる前**
に捕捉し、book 読書中の終了では保持する。これは `folder_history`、main source snapshot、
既存選択 hint を利用する。ページ位置を親一覧 cursor として保存しない。
非同期要求で出発時 hint を運ぶ場合も既存 request が所有し、受理しただけでは target を
変えない。source がまだ表示中なら quit/tray はその main 一覧の最新 hint を保存する。
見えている同じ main 一覧で quit/tray した時だけその一覧の最新 cursor を reducer へ渡す。
independent context を一時 mount 中でも、cursor と target をその viewer に差し替えない。
legacy cursor 保存判定は他 mode 用に維持する。

A/B それぞれの navigation/history は現状どおり。復元 record は application main に一つ。
slot 切替で新しい一覧が採用された時だけそこを記録し、同じ path の no-load switch も
「一覧を明示した」結果があれば同じ commit event を使う。slot ごとに復元 field を追加しない。
新 record と legacy 値は Preferences OK の live 引継ぎ対象、settings transfer の除外対象にする。

Previous の default startup だけが新 record を解決する。物理 target は既存
`load_folder_or_convert_archive(..., false)`、Drive は `enter_drive_list` を使う。
prefix はその同じ復元 request の採用 tail で既存 ZIP navigation に渡す。
存在しない場所の ancestor / Desktop fallback と cursor の同一 target 照合を保つ。
消えた内部 prefix は外側本の既存一覧へ着地し、内部 cursor は適用しない。
新しい worker、resume/retry/rollback owner を作らない。

Desktop / Specific / Drives / ReadingHistory は今の mode routing、fallback と cursor 条件を維持。
command-line / SendTo / activation / Remote IPC の明示 open をこの default restore で
上書きしない。明示 open の再生・auto fullscreen・password 等の既存仕様を変更しない。
トレイ hide は新 record を保存するだけで読書表示を閉じたり target を再生成しない。
トレイ restore は生きている session を戻し、startup restore を呼ばない。

### 5.4 簡素化と detached 憲法

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
設定を初回 load する時に限り、旧値を **そのまま** V1 へ投影する案を推奨する。

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

## 7. 実装前のユーザー／設計担当判断

### A. 合成親一覧の「最後の一覧」をどこまで永続復元するか（要決定）

現状は Search / Rating / Collection / Smart root / 閲覧履歴／しおり／サブフォルダ展開の
一覧を Previous の復元先に保存しない。本をそこから直接読むと物理 book path だけが保存される。
「最後に明示的に開いた一覧」を全 surface に文字どおり適用すると、最小 target set だけでは
不十分で、既存にない **合成一覧の起動復元** が必要になる。

- **最小範囲の案（今回の推奨、ただし承認が必要）**: 既存の復元可能な物理一覧＋Drive を対象。
  合成 root 中とそこからの直接読書中は直前の復元可能な明示一覧を保持する。
  合成から明示的に物理子の一覧を開いた場合はそこを記録する。これを「検索結果の親一覧を
  復元できた」とは扱わない。
- **全一覧を対象にする案**: stable ID を持つ Collection / Smart、Rating stars、ReadingHistory /
  Bookmarks と、検索 query / scope / filter・サブ展開の入力条件の永続 descriptor が必要。
  `FolderNavHistoryTarget` (`src/app.rs:574`) / `TopLevelGridRestore`
  (`src/app/top_level_grid_view.rs:343`) の**意味と既存 navigation**を再利用するが、
  session-local revision／lease／prepared snapshot を serde して保存しない。
  root 再構築・削除された ID・検索実行条件・cursor identity を追加設計する。
  起動は既存の synthetic dispatch / worker を使い、物理 path に sentinel を偽装しない。

この選択は、現在復元できない surface へ機能を広げるか、今回の「一覧」の範囲を限定するかという
製品判断である。ユーザー指示の限定をこちらで確定せず、**決まるまで Phase 2 を始めない**。
同じ判断は Search 中の物理 drill 一覧を明示一覧と数えることにも適用する。

### B. 初回 migration の旧復元先維持（推奨、了承を得る）

旧データには一覧閲覧の履歴が無いため、更新直後に一度だけ旧挙動が残る案を推奨する。
すべての旧 book path を親へ移す案は、明示 page list 利用者の復元を変える。
その代案を採用するなら変更対象と失われる意味を明示して承認を得る。
履歴を推測する追加 machinery では情報不足を埋められない。

## 8. 回帰設計と影響ファイル

| 対象・scenario | 新仕様の検証と既存機能の不変 |
| --- | --- |
| full feature: ZIP/PDF の page list→page→quit | book 一覧を復元。読書 resume とは独立、startup は auto fullscreen=false |
| full feature: parent→direct page→quit | parent 一覧と選択 book の hint を復元。本のページ名を parent cursor にしない |
| explicit PageList / PageFullscreen の一時 override | 設定 ON/OFF に関係なく入口の実際の意図で決める。表示設定を変えて過去 target を推測し直さない |
| direct page→Backspace／一覧ボタン→quit | 明示した book 一覧へ更新。Esc の親戻りは途中 book 一覧を記録しない。close が deferred の場合も完了で一度だけ更新 |
| folder book／本棚の製本フォルダ | 直接読書では親、一覧を明示すれば当該実フォルダ。本棚は仮想 sentinel としない |
| F12 linked root↔child、繰り返し／close | page list/direct のどちらも元 target 不変。表示先だけが変わり、F12 移動で一覧を見たことにしない |
| independent main list + 2 viewers | A/B viewer の ZIP/PDF/Folder/converted open、Ctrl traverse、activation、park、close、main quit 中も main record/cursor 不変。sibling context の items/receiver/cache/history を変えない |
| converted archive cache hit / Convert / Ask / Ignore | 元 source＋prefix のみ保存。cache ZIP、EPUB generation PDF は保存しない。password／cancel／error／古い完了は変更なし。既存 warm PDF 即時表示を遅らせない |
| nested ZIP / ZipDir | 外側本の prefix P 一覧→子 Q を direct→quit は P。本 Q の一覧を明示した後は Q。enter/back、wrapper collapse、nested ZIP／foreign archive cache 両方。再起動時は保存 prefix を通常 navigation で適用 |
| Collection/Search/Rating/Smart/History/Bookmarks | §7A の決定に従う root の復元先を assert。physical descendant の list/direct を区別。各親の戻り chain、filter、履歴、root session に変更を加えない |
| folder Back/Forward / A/B / same-path / no active slot | 採用した一覧だけを記録。失敗／置換／取消で target 不変。A/B histories と slot ownership は既存契約を維持 |
| F5 / sort / filter / PDF placeholder verification | 一覧未表示の book を新しい restore target にしない。cursorとtargetの世代を混ぜず、2回 commit しない |
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

想定 ownership / file scope:

Phase 2 の製品・テスト・対応 docs の writer は、この coherent chunk の implementer 一人。
設計担当／独立 reviewer は read-only で判断し、別 agent の同時編集を前提にしない。

- `src/settings.rs`, `src/settings_db.rs`, `src/settings_transfer.rs`: record・legacy projection・保存／除外・live 引継ぎ。
- startup domain の新 module（候補 `src/app/startup_list_restore.rs`）: reducer と唯一の new record writer。
- `src/app.rs`, `src/app/startup_ops.rs`: 既存 request 意図の運搬、物理／ZIP／PDF の採用、明示一覧復帰、Previous の消費、legacy cursor との分離。
- `src/app/smart_folder.rs`, `src/app/collection_grid.rs`, `src/app/collection_navigation.rs`,
  `src/global_search_ui.rs`, `src/ui_dialogs/archive_convert.rs`: owner を運ぶ必要のある adoption seams。
  §7A の拡張を採る場合だけ synthetic descriptor と起動 dispatch を広げる。
- `src/ui_fullscreen.rs`, main/key/gamepad dispatch: 必要なら existing semantic 一覧復帰 caller の
  意図伝達だけ。新 key / KeyAction は追加しない。実装前に keymap docs を読む。
- `src/app/tests.rs`, `src/app/multiwindow_scenario_tests.rs`, module tests:
  上記の状態／handler／settings 回帰。
- `docs/virtual-folders.md`, `docs/folder-history-location-plan.md`, `docs/architecture-overview.md`,
  本 plan、必要時 detached §11、manual の startup 説明: 実装時に意味と所有境界を追記。

Phase 2 の検証担当は implementer。narrow filtered lib tests → owner 横断回帰 →
`scripts/test-full.ps1`、fmt、UI string を変える場合 glyph check を実行する。
成功後 `scripts/build-dev.ps1` で未起動の確認 binary を用意する。
実アプリ suite はシナリオ・時間・desktop/input・disposable data を提示した明示了承後のみ。
normal profile binary をエージェントが launch しない。Phase 1 は docs/test probe のみなので
確認 binary は不要。今回の red を修正済み／全体 gate 成功として扱わない。

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

実装・独立レビュー・detached §11 の構造合意記録・ユーザー実機確認は
Phase 2 以降の未実施事項。コミットしない。

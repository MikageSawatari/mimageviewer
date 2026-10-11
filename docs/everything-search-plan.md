# Everything ファイル名検索 (§1.374) — Stage 1 設計案

作成: 2026-10-11。対象: v4.6.0 より後。`next-effetune`、基点 `31ac80b9c`。
**設計のみ。採用・実装開始・master への統合は未承認。** 本文の「採用案」は設計担当の推奨であり、実装済みの仕様ではない。

## 1. 要求と調査の出どころ

- 利用者の要求: Ctrl+S / Ctrl+G と別の割り当て可能な操作から Everything を検索し、mIV のサムネイル一覧で結果を開く／戻る。
- バックログ正本: `C:/home/mimageviewer/docs/next-release-backlog.md` の未コミット §1.374 を読み取り。変更していない。単発検索、実ファイル、対応形式への絞り込み、既存仮想検索結果一覧の再利用が試験案。常時追従・USN の直接取り込み・既存索引の置換は対象外。
- 利用者からの環境情報: この PC に Everything があり、service と client が動いている。今回は実プロセス／IPC／索引へアクセスしていない。製品の実機動作や所要時間を観測したとは主張しない。
- 調査対象: AGENTS.md、CLAUDE.md の担当・簡素化・まれな失敗・UI／dialog／IME／検証／文書規則、docs/README.md、architecture-overview、search-architecture、ui-responsiveness、async-architecture、virtual-folders、keymap-spec、key-customization-impl-plan、top-level-grid-view、display-pipeline の検索採用境界、detached-rework-plan §2、Remote の collection 契約。
- 初期 `git status --short` は空、branch は `next-effetune`、HEAD は `31ac80b9c`。git は読取確認のみ。SDK・DLL・exe・crate・zip をダウンロードしていない。
- 親セッションの GUI モデル／reasoning 設定を、この作業から変更・検証していない。利用者指定の実装役は GPT-6.1 Sol / high。重要な所有／非同期設計は別コンテキストの Sol / xhigh レビューを通し、実装者の自己確認で代替しない。

## 2. 結論と受入条件

**Rust の直接 IPC を採用し、既存 `GlobalSearchState` を検索ソースの型付きモードへ拡張する。一覧・戻り先・thumbnail worker を二重所有しない。** UI 上では「Everything ファイル名検索」として Ctrl+G から独立させる。

初版の推奨仕様:

1. main の一覧から専用検索バーを開く。文字入力だけでは問い合わせず、IME 確定後の Enter または「検索」で送信する。
2. 送信中は main の対象 context に限定した取消可能な進捗ダイアログを表示する。問い合わせと準備がすべて終わってから、完成した結果一覧を一度採用する。未完成行を閲覧・編集できる状態を作らない。
3. Everything の生の検索文字列を渡し、返却後に mIV の対応形式・表示するファイル種類・EPUB／変換書庫設定を適用する。お気に入り外・別ドライブも許可する。
4. raw 結果は先頭 10,000 件まで。初版は IPC offset paging／「もっと表示」を実装しない。絞り込み後の件数と raw 上限を区別して表示する。
5. 結果は flat のサムネイル一覧。既存の詳細表示・ソート・★／tag facet・通常の閲覧を共有する。Ctrl+G の自動集約・hit-count sort・全文検索フィルタは Everything では出さない。
6. 初版に Remote の検索 API／結果共有を追加しない。F12 の既存の open／close／fork 経路は利用する。兄弟 viewer の items／queue／世代／編集状態を変更しない。

受入条件は「索引機能が無効でも検索できる」「UI スレッドに IPC／FS／DB を追加しない」「古い完了結果が新しい一覧へ入らない」「取消やエラーが元一覧・ユーザーデータを壊さない」「結果から画像・本・動画・音声を開き、同じ結果へ戻れる」。

## 3. 公式仕様と依存・ライセンス

一次資料は 2026-10-11 に Web 本文として確認した。リンク先の SDK download は利用していない。

| 根拠 | 確認事項 |
| --- | --- |
| [IPC](https://www.voidtools.com/support/everything/sdk/ipc/) / [SDK](https://www.voidtools.com/support/everything/sdk/) | ローカル IPC。SDK は IPC wrapper で、起動中の Everything が必要 |
| [公式 IPC C example](https://www.voidtools.com/support/everything/sdk/ipc_c_example/) | HWND へ WM_COPYDATA で問い合わせ、reply window で結果を受ける |
| [作者公開 QUERY2 定義](https://www.voidtools.com/forum/viewtopic.php?t=5724) | 1.4.1 の QUERY2W、固定幅 DWORD の request／response と full-path データ |
| [IsDBLoaded](https://www.voidtools.com/support/everything/sdk/everything_isdbloaded/) | 1.4.1 以降。DB 未ロード時の空結果を成功の 0 件と取り違えない |
| [version IPC の作者説明](https://www.voidtools.com/forum/viewtopic.php?p=12529) | WM_USER による version 問い合わせ |
| [取消の作者説明](https://www.voidtools.com/forum/viewtopic.php?t=14938) | 同じ reply HWND へ新しい `nothing:` 検索を送ると旧検索を取消す。1.4 でも利用可能 |
| [service と client の作者説明](https://www.voidtools.com/forum/viewtopic.php?f=5&t=5611) | IPC の相手は service ではなく client |
| [複数 instance の作者説明](https://www.voidtools.com/forum/viewtopic.php?t=8338) | 既定の窓 class と named instance は異なる |
| [search syntax](https://www.voidtools.com/support/everything/search_syntax/) | Everything の検索構文を使う |
| [SDK licence の作者説明](https://www.voidtools.com/forum/viewtopic.php?p=20943) / [Everything licence](https://www.voidtools.com/License.txt) | SDK は MIT。コピー／同梱する場合の notice 義務 |
| [WM_COPYDATA](https://learn.microsoft.com/en-us/windows/win32/dataxchg/wm-copydata) / [SendMessageTimeoutW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendmessagetimeoutw) | payload は callback 内だけ有効。送信 API の成功と相手の戻り値は別 |

採用案では Everything 本体・SDK DLL・SDK wrapper source を配布しない。既存 Windows bindings の user32 を使うので、新しい実行時 DLL 配置・launcher extraction・SDK DLL の bitness／DLL 検索パス対策は不要。Everything のインストールと索引設定は利用者が管理する。

独立した Rust 実装でも、公開 header／example から定義・実装を移植する場合は出典と MIT notice を保守的に残す。配布時の既存第三者 notice へ必要分を載せる。MIT は mIV のソース公開を強制するものではない。Everything 本体に含まれる PCRE 等を、IPC 接続だけで mIV が同梱したことにはしない。

SDK DLL 案は MIT notice を原文で同梱できるが、DLL／import lib の取得・版固定・配置・署名／配布管理と global SDK state の直列化が増えるため却下。SDK の notice と Everything.exe の licence を同一物として扱わない。取得を選ぶ場合は承認済みの別工程にし、取得した SDK 内の licence を確認する。

**今回の設計には download は不要。** 公開資料で確認できない wire constant／packing が実装段階に残った場合は推測で補わない。既存ローカル資料または作者の公開 Web 定義で確認し、それでも SDK header が必要なら `Everything-SDK.zip` の header／licence が何のために必要かを報告して停止する。無断取得へ進まない。

## 4. IPC transport と安全境界

### 4.1 接続と wire

新規 `everything_ipc` module は UI／egui に依存しない safe interface を公開する。pure codec、Windows transport、fake endpoint を分ける。検索ごとの専用 worker が受信用の **不可視 Win32 window と message pump** を所有する。egui/native presenter の HWND を IPC reply HWND に流用しない。

- 対象は current user session の既定 instance、class `EVERYTHING_TASKBAR_NOTIFICATION`。タイトル文字列・exe の決め打ちパス・registry 走査では探さない。named instance 選択と 1.5 alpha instance は初版の必須対応にしない。
- worker で version と DB loaded を確認する。baseline は IPC QUERY2W／IsDBLoaded を備える 1.4.1 以降。1.5 の既定 instance は互換 wire を受理できる場合に限り同じ経路を使う。将来の major 版を番号比較だけで互換と判定しない。
- `WM_COPYDATA` / `EVERYTHING_IPC_COPYDATA_QUERY2W`。`offset=0`、`max_results=10_000`。request は full path and name のみ。highlight、content、icon、全件 metadata を要求しない。sort は Everything の名前昇順を要求する。response の実 sort を記録する。
- search_flags は大文字小文字無視・部分一致・filename match を既定とし、Everything UI の現在のトグルに依存させない。利用者が構文中に指定する path／regex 等の指定は Everything 側の規則に従う。
- query header は 7 個の DWORD と NUL 終端 UTF-16LE、reply header は 5 個の DWORD、各 item は flags と data offset。Rust `usize`／通常の HWND struct layout を wire へそのまま書かない。HWND の wire 表現も公開規約の 32-bit 幅と native handle 変換を確認する。
- request 内の `reply_copydata_message` は active request の識別子。reply HWND・識別子・query run の対応を送信前に登録し、即時／reentrant reply も取り落とさない。

### 4.2 timeout、cancel、drop

worker 内の送信は `SendMessageTimeoutW`、送信ごとに上限 1 秒、応答全体は送信開始から 30 秒を推奨。`SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT` を使い、`SMTO_NOTIMEOUTIFNOTHUNG` は使わない。callback の再入を妨げる `SMTO_BLOCK` は使わない。API return と receiver acceptance を別に検査する。

待機は OS event／message pump と deadline で行い、`try_lock + sleep` や UI の `recv`／join を使わない。UI の cancel は token と wake event を立て、receiver を手放して即座に main 入力を戻す。worker は同じ reply HWND に `nothing:` の取消 query を一度だけ bounded send し、返却を待たず window を破棄して自然終了する。取消 query の成功は UI の取消完了条件にしない。旧 request の処理が Everything 側で即時停止する保証はしない。

close、再送信、別検索への移行、main の別一覧への移行、context retire、App 終了はいずれも同じ request drop／cancel 境界を通る。新 query は別の run として開始し、旧 HWND 再利用でも旧 reply ID を受理しない。reply ID の使い回しによる alias を許さない。UI は worker 終了を待たない。送信 buffer と callback state の寿命は worker が RAII で持ち、timeout／callback 再入中にも dangling pointer を作らない。

### 4.3 parser と結果の信頼範囲

WndProc はサイズ／request ID を確認し、上限内の payload を owned bytes へコピーしてから戻る。解釈・分類・DB 準備は callback 外の worker。callback が借りたポインタを channel へ送らない。

提案上限は reply 16 MiB、query 32,768 UTF-16 code units（終端を含む）。割当前の byte／count 検査、checked add/mul、header／item table／offset／UTF-16 length と NUL の境界検査を行う。count が request 上限を越える、必須 full-path flag がない、offset が 0 でない等は malformed として log＋通知し、部分結果を公開しない。query の埋込み NUL は送信前に拒否する。

実ファイル結果だけを採る。folder／drive／root flag は除外。relative path、URL、device namespace、制御文字／仮想 entry separator を含む値は physical-file identity にしない。drive absolute／UNC／通常の verbatim file path は既存 `path_key` の規則に沿って I/O なしで正規化・重複除去する。UNC を勝手に除外しない。存在・権限を IPC の主張だけで保証せず、読出し／open は既存 worker の検証を使う。欠落観測から metadata DB の purge を呼ばない。

## 5. query、形式、件数、paging

### 5.1 検索文字列は Everything へ渡す

`search_query::parse`、mIV の AND／OR toggle、文字数 minimum、NOT-only 制限、FTS post-filter を通さない。入力文字列を UTF-16 化して渡す。全空白だけは未検索として案内するが、非空 query の内部の quote、空白、大小文字、演算子は書き換えない。

Everything の `ext:`、wildcard、引用符、path、regex 等を利用できる。mIV の tag／rating／EXIF／AI prompt 検索ではないことをバーの説明・manual に明記する。高度な構文は相手の版に依存し、IPC の空結果から「構文が無効」とは断定しない。content 等の遅い検索も UI thread へ移らず、利用者は取消できる。

採用案は **query の自動 `ext:` 追記をしない**。OR／group／macro の意味を変えず、plugin が対応する拡張子も後段で扱える。一方、非対応ファイル・folder も raw 上限を消費する。この限界は件数表示で隠さず、必要なら利用者が `ext:jpg;png` 等で絞る。

### 5.2 mIV の候補分類

worker にファイル種類設定、EPUB／変換書庫設定、thumbnail source discovery と対応形式の snapshot を渡す。通常 scan の分類規則 (`app/folder_scan.rs:880` 付近) と同じ採否・優先順を共通 helper にするか、既存 helper の組合せで利用する。固定の小さい IMAGE_EXTS リストを新設しない。

| 候補 | 採用行 |
| --- | --- |
| 内蔵／WIC／RAW／利用可能な Susie 対応画像 | `Image` |
| 対応動画／音声 | `Video` / `Audio` |
| ZIP／CBZ | `ZipFile` |
| PDF／許可された EPUB | `PdfFile`（元 EPUB identity を保持） |
| 許可された RAR／CBR／7z／CB7／LZH／LHA 等 | `ConvertibleArchive`（probe／変換は通常 open へ） |
| folder、未対応形式、非表示種類 | 除外 |

Everything は archive 内 entry／PDF page を列挙しない。検索結果の本タイルを開いた後は、その本の通常の全ページを閲覧する。選択した archive の内容だけ query で再フィルタしない。

名前ヒットをそのまま示すため、親フォルダの代表への集約、別ファイル名の同 stem collapse、動画 sidecar 画像の自動省略は Everything 結果では行わない。query にヒットした画像は独立した結果である。動画／音声の補助画像選定は既存 source discovery worker を使い、画像の表示可否を補助画像の採否へ誤流用しない。

**初版は対応形式へ絞った候補だけを、共通 Search prepare worker で一度 stat する。** IPC の全 raw 行に stat するのではなく、folder／非対応／非表示形式を先に除外する。これは thumbnail の既存契約を満たすための準備であり、新しい存在確認 worker ではない。metadata 成功から file 種別、mtime、size を確定し、`image_metas=Some((mtime,size))` と `ListingSortMetadata::new(mtime,Some(size))` を同じ prepared row に保持する。実 0-byte と取得失敗を同一視しない。分類・sort・source-discovery・stamp は採用前に worker で揃える。再 sort は採用済み stamp を再利用する。

根拠: 現行 thumbnail enqueue は画像／ZIP／PDF の `image_metas=None` を skip し (`app.rs:52088` 付近)、convertible の None は Unavailable になる (`:40649` 付近)。`Some((0,0))` を捏造すると `thumb_loader.rs` の cache freshness 比較へそのまま入る。DetailsLazyMeta は日時／size を後から埋める一般 owner ではない。このため「full-path-only をそのまま採用し、既存 lazy thumbnail が stat してくれる」前提を却下した。unknown stamp を許す共通 thumbnail request の構造変更は初版に追加しない。

stat に失敗した、または現在は file でない候補は今回の新しい結果から除外し、件数を集計して「読めなかった項目 N 件」を案内する。これを mIV DB の不在証明にせず、user data／metadata row を削除しない。採用後に消えた path は thumbnail／open の既存 failed 表示と通知で扱う。再検索は利用者の明示送信だけ。

decode・archive inspection／展開・全件 EXIF 読取は検索完了条件にしない。共有 I/O permit と各候補の cancel checkpoint を使い、UI は stat 終了を待たない。UNC 等の OS I/O 自体は強制中断できないため、cancel の UI 解除と旧完了の棄却を別に保証する。検索結果は stat 時点の snapshot であり、採用までの外部変更を完全同期するものではない。

**速度上の費用:** Everything の名前照会が速くても、最大 10,000 の対応候補への stat と source-discovery が完了までの時間を延ばし得る。cold HDD／UNC を含む採用時間を計測する。これで要件を満たせなければ、IPC indexed stamp の利用または unknown-source-stamp を既存 thumbnail request owner が解決する構造を別の coherent design としてレビューする。見かけだけの zero stamp／同期 UI I/O／形式の無断削減で埋め合わせない。

### 5.3 上限と paging の判断

1 query／1 reply、raw 上限 10,000、後段の表示行も最大 10,000。raw limit は Everything の並び順で切り、mIV のソートは **取得した部分集合内** で行う。Everything の `totitems` は全構文ヒット数であり、mIV 対応形式の総数ではない。

表示例: `Everything: 52,340 件中、先頭 10,000 件を取得 / mIV 表示対象 1,246 件`。種類・★／tag facet でさらに隠れる件数と stat で読めなかった件数も取得件数とは別。上限で打ち切り、後段で 0 行になっても「対象がありません。取得上限に達したため検索条件を絞ってください」と案内し、「全体で 0 件」と表示しない。16 MiB を超える reply は一覧を採用せず条件を絞る案内とする。

offset paging は wire 上できるが、QUERY2 に同一結果 snapshot を保持する token の保証を今回の資料で確認できない。別 query で offset を進めると Everything の更新で行が飛ぶ／重なる可能性があり、local dedup は欠落を直せない。後続取得中の行再配置・編集・open/back の組合せも増える。初版は cap＋明示絞り込みで取り除く。将来 paging を入れるなら snapshot 契約と上限の意味を別設計で決める。

サムネイルは完成した一覧採用後、通常の可視範囲優先・async request lifecycle・GPU upload pacing で順次表示する。この段階表示と IPC paging／検索結果の追加採用は区別する。

## 6. 既存一覧への統合と唯一の owner

### 6.1 現行コードから判明した境界

- `global_search_ui.rs:418` の `GlobalSearchState` が query、run、pending、prepare、hits、drill を持つ。`SearchHandle` (`indexer_manager.rs:56`) は drop で cancel、UI join なし。
- `open_global_search`／`dismiss_global_search_with_origin` (`:2374`／`:2438`) と `App::take_origin_for_search_entry` (`app.rs:30614` 付近) が `TopLevelGridView` の origin を移譲する。結果 path を本当の directory として保存しない。
- `SearchPageEditPrepare`／`SearchViewWish`／`poll_search_page_edit_prepare` が worker の rating／page edit／page identity と context・run・items generation・設定を結び付ける。`Ready` の採用 closure で rows と sidecar maps をまとめて公開する。
- `replace_search_view_items`／`install_new_items`／`invalidate_idx_state_and_queues` が一覧と async thumbnail 世代の境界。直接 `items.push/clear` を追加しない。
- `build_flat_items_with_recipe` (`:1021`) は ZIP 本体を明示除外し、`grid_item_from_fs_hit_path` (`:1404`) は未分類 path を Image にする。**Everything 結果を単に GlobalHit に入れるだけでは ZIP が消え、CBZ／変換書庫が誤分類される。** この二つを全文検索のまま転用しない。
- backlog §1.372 と display-pipeline の 2026-10-11 記録には、旧 Search streaming の idx-only edit/Undo 誤配送の残件がある。この設計をその一般修正の完了と主張しない。

### 6.2 型と共通経路の提案

`GlobalSearchState` に独立した `everything_active`／`everything_pending`／saved-folder 群を追加しない。共通検索 session 内に `SearchSource::{Metadata, Everything}` を持つ。source ごとの query options／run を enum の payload とし、Everything の phase は `Editing | Running(request) | Ready(stats) | Failed(message)` から導出する。pending の有無と別 bool 群で排他 phase を表さない。

共通 owner に残すものは採用済み rows／prepare、選択復帰、origin、thumbnail lifecycle。Metadata の streaming events と Everything の単一完了は typed producer として分ける。既存の `pending: Option<SearchHandle>` を source-aware handle へ置き換え、共通 cancel/drop を持たせる。FTS manager の初期化・readiness・favorite ownership を Everything spawn の前提にしない。

入力中の source/draft、未採用の request、採用済み view は意味が異なる。共通 session の `accepted` が `{source, submitted spec, dataset, stats, position}` を一つの型付き payload として所有し、request はその置換候補だけを所有する。source 切替や再送信の開始で accepted payload を書き換えない。失敗／取消では旧 accepted view とその source に従う open/back を保ち、バーに旧結果の source と「新しい条件は未適用」を示す。採用成功の同じ closure で rows と accepted payload を置き換える。現在のバーの source だけで表示済み rows を解釈しない。旧 dataset の第二コピーや App-global result map は追加しない。

候補から共通 prepared row へ、元 physical identity、正しい `GridItem`、unknown を区別する sort metadata を渡す。`SearchPageEditPrepare` の FIFO／PageEditAvailability／rating write provenance／stable page identity／source discovery を再利用し、Everything 用の第二 prepare owner は作らない。分類／sort／rating bulk read／巨大な discarded payload の解放は worker 側。source を `SearchViewWish` と判定へ含め、Metadata の filters と Everything options を比較し違える状態を作らない。

Everything は completed candidates を一回渡し、prepare の final Ready まで progress phase を維持する。既存 Batch 処理が途中で `rebuild_items_from_global_search` を呼んで空 shell や部分結果を公開しないよう、Everything producer の完了境界を型で示す。prepared rows／thumb source／rating／identity maps は同じ accepted closure で install する。

`TopLevelSearchView::Global` は共通検索 surface のまま利用し、Metadata／Everything の区別を session が所有する。UI 説明を明確にするため名称を共通 Search へ変える場合も新 surface owner は追加しない。F/S/T と source 切替は現在の origin を restore せず移譲する。Metadata→Everything の際に `open_global_search` の active early return や `SearchMode::Global` の keep 処理で旧 source が残らないよう、**source 切替を共通 entry transaction** にする。

### 6.3 簡素化と UI 入力

全 interleaving の rollback／部分採用／resume を作る案を比較し、明示送信＋準備中の target-context modal を推奨した。変更するのは新機能の待機中だけで、Ctrl+G の streaming／debounce／集約を遅らせない。

バーは existing global-search top panel を source 別に描画する。TextEdit は `ime_focus::show_singleline` helper、Enter／Esc は `dialog_enter_pressed`／`dialog_escape_pressed`、context menu は既存 singleline helper。未確定文字の確定 Enter で検索を送信しない。Esc は実行中なら取消、Editing／Ready なら検索を閉じる。query draft の変更は表示済み結果を再実行せず、「検索」を押すまで保持する。

進捗 Window は `default_pos`、`.open` の ×＝取消、テーマ色と制限された本文高さを使う。描画 phase と入力ブロック predicate を同じ helper から導出し、`common_modal_dialog_open` の中央規則へ接続する。ただし request context と projected context を照合する **対象限定の predicate** とし、兄弟 viewer へ停止を広げない。新しい main／detached 用の別 bool・別 modal list は持たない。既存 sidecar restore の対象 context gate を設計例にするが、その復元 state へ検索を混ぜない。

主一覧の open／navigation／history／F12 switch／Remote acquire はこの modal admission を共有する。取消・Window ×・shutdown は常に受理。既存 detached の独立閲覧は停止しない。UI frame は bounded `try_recv` と小さい状態遷移だけを行い、worker wake は ROOT を明示。idle で毎 frame repaint しない。待機 backstop が必要なら既存の再武装規約を使い、競合解消の grace／sleep を足さない。

古い Ready は `{source, run_sequence, context, source/items generation, adopted surface authority, policy snapshot}` を既存採用証明へ照合して捨てる。取消・切替後は query text が偶然同じでも受理しない。Everything の submitted query/options は Running／Ready の immutable run payload が所有する。バーの `query` は draft として別の意味を持ち、draft を変えただけで採用済み rows の sort／facet prepare を失効させない。現行 Metadata の `wish.query == self.global_search.query` 検査を Everything にそのまま適用せず、submitted spec 同士で比較する。公開前に失効した map は現行一覧へ先行書込みしない。

送信／source 切替／結果ソートなどで index 空間を替える前に、既存の編集終了・page-boundary 処理で保存先を旧 identity のまま確定し、idx-only Undo と editor draft を新一覧へ持ち越さない。Everything の一覧再投影では閲覧を閉じる必要がある場合もこの既存境界へまとめる。共通 editor を新規 owner 化したり、Ctrl+G の released 動作を変更して §1.372 を同時修正しない。既存境界だけで Everything の通常操作を安全にできないことが実装前検証で判明したら、追加設計の範囲・費用を報告して止める。

## 7. open、back、history、F12、Remote

### 7.1 開く／戻る

media は一覧の exact physical identity から通常の fullscreen／native player open を使う。Esc で同じ検索一覧へ戻り、通常の cursor／Shift anchor owner を使って開いた結果へ選択を戻す。本は通常の ZIP/PDF/EPUB/変換書庫 open と `FacetRoute`／親一覧復帰を使い、BS／親操作で同じ結果・query・取得集合へ戻す。変換／password cancel は元結果を保持する。検索を再送信しない。

現在の Ctrl+G は `SearchContainer` の hit-only drill と physical 本 open の双方がある。Flat から本を開く帰路も `commit_staged_search_navigation_effects` → `advance_drilled_current_path` (`global_search_ui.rs:3503` 付近) が一段の `DrillState` を作ることで成立している。`app.rs:53067` の BS は active search かつ drill=None で no-op になる。したがって Everything の本 open は、型付きの位置を持つ構造的変更が必要であり、現行 return を無変更で使えるとは判断しない。

共通 session の source payload を `Metadata(existing drill/view)` と `Everything { submitted/accepted result, position }` に分け、後者の position は `Root | PhysicalChild { logical_source, return_anchor, route }` とする。logical_source は検索行の元 ZIP/PDF/EPUB/変換書庫であり、cache ZIP/PDF ではない。archive 内の現在 prefix は既存 `zip_nav`、本の現在ページは既存 reader が所有し、position に複製しない。origin の「検索前の場所」と、この position の「採用済み結果内の子」は異なる役割である。同じ戻り先の第二 owner は作らない。

本 open の typed request は現在の search run/source と選択した logical identity を捕捉する。**実際の physical open の accepted closure**（ZIP/PDF の install、または変換後 alias を採用する境界）だけが Root→PhysicalChild を公開する。open の受付・変換開始・password 表示だけでは position を替えず、cancel/error/stale は Root のまま。common result dataset/query/stats は session に保持し、child の `App.items` へ検索 rows を同時に表示・再採用しない。

BS／親移動は Everything position を経由し、book 内の既存 prefix parent がある間は通常の内部移動へ渡す。book root からは保持済み result dataset を共通 Search prepare→accepted install で Root へ戻す。再 IPC は行わない。復帰成功時だけ position を Root にし、return_anchor の full path で選択・ensure-visible を行う。cancel/failed return は子の accepted view を保持する。Esc は fullscreen の通常終了を先に行い、grid の本階層では既存の検索終了 semantics を維持する（元結果への階層復帰は BS／親操作）。

`commit_staged_search_navigation_effects`、`advance_drilled_current_path`、grid／fullscreen の parent・Ctrl+上下、converted alias、F12 direct-media／container open を source-aware routing の対象として列挙する。Everything Root では filesystem の sibling navigation をしない。PhysicalChild 内では本の通常 navigation を使い、外側 container を越える操作は検索結果集合の契約へ戻す。Metadata の hit-only drill はそのまま維持し、Everything 用に container hit count や偽の DrillState を作らない。

検索は一時 overlay。既存と同じく folder back/forward stack に登録せず、検索中の履歴ボタンを無効にする。検索の close は origin の typed restore を一回消費して元の folder／smart child／sub-expansion／Collection／rating／history／Drive に戻る。「実フォルダに移動」は origin を復元せず共通 `JumpToPhysicalFolder` の typed open へ移り、成功後に exact file を選択する。synthetic result path に `load_folder`／FS stat を呼ばない。再起動復元に query／検索一覧を保存しない。

### 7.2 F12

新しい検索操作の発火面は **main grid のみ**。fullscreen／native video／detached viewer から検索バーを出す shortcut は初版に追加しない。結果を開くときの F12 モード、既存 viewer の park／fork／promote／close は維持する。

`ViewerContextBundle` は global_search、top-level surface、thumbnail／items generation の既存所有単位。`viewer_context_registry.rs:3299` 付近の parked split は検索 UI state を main に保持する一方、表示束をメディア窓へ移す場合がある。検索 source が context に移る／main に残る契約を各 registry transaction と同じ境界で扱い、App-global Everything result cache から active window へ配送しない。検索 worker を fork しない。

検索中は main F12 switch を target-context admission で止め、Ready 後の通常 F12 open を可能にする。既存 detached が同じ画像を読んでいる間の再検索／取消／close は main の state だけを退役させ、兄弟の unchanged items、thumbnail cancellation、PDF render epoch、full-screen load、edit state を失効させない。

**未解消の構造境界（独立レビュー P1）:** 現行 `replace_search_view_items_inner` (`global_search_ui.rs:2065`) は `bump_render_epoch_only` (`app.rs:50360`) を呼び、crate-global `CURRENT_CONTEXT_EPOCH` (`pdf_loader.rs:1180`) を更新する。pool は全 context の Normal／HighNormal jobs をこの epoch で prune／pop 時に棄却する (`:3876`／`:4694`)。各 context の thumbnail enqueue も同じ global epoch を焼き付ける (`app.rs:51643`／`:52153`)。fullscreen 本体の render は epoch=0 でも、detached PDF の静止画シークは `ui_fullscreen.rs:22367` → `ensure_still_seek_thumbnail_requests` → `enqueue_priority_thumbnail` を使い、この失効の対象になり得る。既存 helper の単純再利用だけで上記の兄弟不変を達成できる、とはしない。

**chunk B 開始前の必須条件:** PDF thumbnail render の enqueue・派生 request への継承・pool prune／pop 判定・cancel／drop を、所有 viewer context に限定する bounded subdesign を作り、設計担当と独立 reviewer が合意する。通常 folder load、Search replace、priority／upgrade／seek、background／fullscreen の epoch=0、registry の park／fork／promote／retire を producer／consumer 一覧に含める。同じ path を main と detached が同時に読む試験で、main の旧 jobs は失効し sibling の jobs／requested は残ることを示す。owner の世代は既存 bundle とともに移譲し、fork が他 context の失効権限を複製しない設計が必要である。

これは Everything request の cancel/generation と異なる既存 PDF pool の所有境界で、現在の設計書では具体的な改修方式を最終承認していない。統合実装前に止める条件であり、まれな失敗として割り切らない。global bump を Everything だけ省く guard、全 render の epoch=0 化、F12／seek の無効化では代替しない。F12 の機能制限は利用者の明示了承なしに選ばない。分離した先行設計・改修の範囲と費用を提示してから統合へ進む。

実装で predicates／viewport／registry transfer を触る場合は `detached-rework-plan.md` §2 に沿う構造変更として独立レビューと設計担当の合意を得て、§11 に範囲・理由・試験を記録する。今回 §11 は編集していない。症状 guard・追加時間窓・HWND geometry capture・placement 保存先を作らない。

### 7.3 Remote

初版では Web UI、protocol enum、Everything query endpoint、collection type を増やさない。Remote collection は settings／独自 worker から一覧を構築しており、PC `GlobalSearchState` を公開する経路ではない (`remote_ipc/collections.rs`)。main の結果を「現在地」として自動配信しない。Remote session の既存 local-control gate を尊重し、取得中の main modal と Remote acquire を同じ受付判定で排他にする。

通常の物理 file address と利用者が作った永続 Collection を Remote で閲覧できる現行仕様は維持する。Everything 結果から既存 Collection に明示追加した file が後で Remote に見えるのは既存の Collection 機能である。新 feature の query／結果 owner を Remote に渡すこととは区別する。IPC 版更新は初版では不要の見込みだが、実装で contract を変えるなら scope を返す。

## 8. KeyAction とメニュー

提案: `GlobalEverythingSearch`、日本語説明「Everything でファイル名を検索する」、`Press`、main grid からだけ実行。**既定キーの推奨は Ctrl+Shift+G**。`docs/keymap.ini.default`、`docs/keymap-spec.md`、`src/keymap.rs` の読取確認で、この chord の既定割り当ては見つからなかった。Ctrl+G は item search、Ctrl+S は container search／fullscreen capture、Ctrl+E は export なので流用しない。利用者の個別割り当てと OS／他アプリの global hotkey は未検査。

`KeyContext::Grid` とし、caller 側でも main grid・IME／text focus・modal／Remote lock の適格性を保証する（helper の chord 判定だけで context enforcement されたと思わない）。既存 Global 検索 action と同じ context にすると native video の全 Global-action forwarding に入り得るため、名前の Global は検索範囲を示すものとし発火 context を Global にしない。fullscreen に新しい検索転送を追加しない。操作カスタマイズの候補にも実行 scope を示す。

`ini_name`／description／context／trigger／default_chords／ALL_ACTIONS と同梱 default を一緒に更新する。menu の「Everything ファイル名検索…」と key は同じ entry transaction に接続し、caption は現在の keymap から表示する。初版にリング／gesture 専用 action は増やさない。キーは利用者決定後に確定する。

## 9. 利用不能・失敗の案内

| 状態 | 動作と案内 |
| --- | --- |
| IPC window 不在（未導入／client 未起動／service だけ／named instance のみ） | 「Everything に接続できません。インストール済みの場合は検索クライアントを起動してください」。これらを HWND 不在だけで区別したと主張しない。手動の再検索と公式案内リンク |
| Lite／IPC 無効／QUERY2 拒否／古い版 | 「この Everything では IPC 検索を利用できません。通常版の 1.4.1 以降と IPC 設定を確認してください」。lite 検出を version だけで断定しない |
| DB 未ロード／再構築中 | 成功の 0 件を採用しない。「Everything の索引を準備中です。準備後にもう一度検索してください」。自動 wait/retry はしない |
| DB loaded だが folder index が更新途中 | 得られた時点の snapshot を表示。全ドライブ／全 folder の完走や最新性を保証しない。loaded は全索引更新完了の証明ではない |
| 応答 deadline／hung／終了／UIPI 拒否 | log＋「Everything から結果を取得できませんでした。起動状態・権限を確認して再検索してください」。mIV を勝手に elevate しない |
| malformed／byte limit／互換 wire 不一致 | その reply を全体で拒否。log＋通知／条件を絞る案内。QUERY1／SDK／FS 全走査への silent fallback なし |
| worker spawn／prepare／DB read のまれな失敗 | log＋通知。未採用一覧を公開せず、元の accepted view を保持。新 recovery state／retry loop／DB 再作成を設計しない |
| 取消 | 未採用 request を破棄し、旧 accepted view を保持。エラー toast／部分結果は出さない |

service/client の起動・終了、Everything の再索引・設定変更・run history 更新は行わない。mIV の FTS/name DB／favorite 設定を変更しない。query と結果全 path を通常ログに残さず、source・run・context・duration・count・limit・失敗種別を診断に使う。

## 10. 却下・後送り

- SDK DLL 同梱、ES 子プロセス、HTTP／ETP: 配布依存・encoding／shell／ネットワーク設定を増やす。直接ローカル IPC で要求を満たす。
- USN を mIV が読む／Everything の index をコピー: 対象外であり、索引の第二所有者になる。
- Ctrl+S／G の置換、favorite 自動登録、tag／rating を Everything query に翻訳: 現行機能と検索意味を壊す。
- typing ごとの live query、IPC paging、後着 rows の追加採用、結果の常時追従: snapshot と編集の組合せを増やす。初版は単一完成採用。
- 全 raw 行の stat、全件 decode／archive 展開で内容を事前計算: 不要な I/O。対応候補への stamp stat は §5.2 の既存 thumbnail 契約のために行い、decode／展開は可視範囲の既存 worker に任せる。
- Everything 未導入時の勝手な download／launch／検索 fallback: 利用者環境を変更し、失敗理由を隠す。
- 全 window を止める新 modal、App-global result map、second saved-folder／generation sentinel: viewer-context ownership を損なう。
- Remote API、新 persistent query history、multi-instance picker、集約ビュー: 初版で必要な操作を増やさず、別工程へ。

## 11. 試験・性能・文書更新・実装の区切り

### 11.1 自動試験（実 Everything 不要）

pure codec と `FakeEverythingEndpoint` を Windows transport と同じ safe interface に差し込み、実 DB／プロセス／service の有無に依存させない。fake endpoint は request capture と任意の delayed／malformed reply を返せる。fake clock／event を使い、実時間 sleep で deadline を検査しない。

| 試験境界 | 必須ケース |
| --- | --- |
| wire codec | DWORD packing／UTF-16 roundtrip、日本語・emoji・長い UNC、NUL 拒否、length overflow、欠落 header/table、data offset 不正、odd bytes、NUL 不一致、unknown flags、count／byte cap、異なる reply ID、actual sort |
| transport fake | absent endpoint、version unsupported、loaded=false、query 拒否、accepted 後の timeout／disconnect、即時 reply、cancel 前後の late reply、cancel query の失敗、正常 0 件。失敗時に実 client fallback しない |
| query pass-through | quote／OR／group／NOT-only／1文字／ext／regex を送信文字列のまま捕捉。空白のみと embedded NUL の局所拒否 |
| format projection | 内蔵／RAW／fake Susie、video／audio、ZIP/CBZ、PDF/EPUB、RAR/CBR/7z/CB7/LZH、ignore settings、file visibility、folder／unknown／relative／device path、大小文字・slash dedup、favorite 外／複数 drive |
| cap／paging／stamp | 0／9,999／10,000 件、totitems が cap 超、raw cap が unsupported／folder で埋まる、後段 0 件、要求 offset 常に 0・検索が一回。fake stat 成功の実 0-byte／失敗／directory を区別し、成功stampだけで thumbnail enqueue／cache／convertible registration が成立 |
| session／adoption | source A→B→A、同じ query の再送信、close／navigation／settings change／context retire 後の Ready、failed prepare。rows と sidecar maps は採用成功時だけ更新。部分 Batch で閲覧可能な一覧を出さない |
| open/back | image/video/audio、ZIP/PDF/EPUB/convertible、本内 BS と変換/password cancel、exact selection、元 synthetic／smart child／Collection 復帰、JumpToPhysicalFolder、履歴不変更 |
| editing | Ready で開いて補正→戻る→sort／再検索／source 切替。旧 idx の Undo/draft が新行へ書かれない。Metadata の既存テストを弱めない。§1.372 の一般修正と混同しない |
| context／F12／Remote | worker を fork しない。main 検索 cancel/close/requery で sibling の rows/cache/queues/generation 不変。main の Search採用／通常loadで旧main PDF jobsだけがpruneされ、同じpathのdetached seek jobs/requestedは不変。modal は main だけ。Ready 後 F12 と park/promote/return。Remote に source/query/collection enum を増やさず既存 lock を維持 |
| keys／UI | inventory／generated default、変更した key の発火、無効化、Ctrl+G/S/E 非競合、IME preedit/commit/Esc/Tab、日本語 pasted query、Window ×取消、長い通知、small screen／100%・200% snapshot、progress 消失後の入力復帰 |

fake Win32 server が必要な adapter integration は、専用 thread のテスト用 class を **注入した HWND** として使う。default Everything class を検索しない、実 client を終了・操作しない、foreground/input を使わない。実 transport の wire／callback lifetime／SendMessageTimeout と Win32 parser をここで検査する。real Everything の compatibility 確認は別の承認済み実機工程であり、自動 suite の必須依存にしない。

### 11.2 性能と後続の手動確認

計装区間: discover/version/readiness、send、reply-copy、parse/classify、prepare、adopt、first-visible-thumb。source／run／context と件数・byte・elapsed を記録する。UI install に同期 FS／DB／IPC がないことを call graph で確認する。10,000 件 fake の UI drain／adopt、旧 payload 解放、back／cancel、idle repaint の既存予算を測る。計測前の「瞬時」「Small」で受入としない。

実装者が narrow lib tests／keymap／App search/context regressions／snapshot／glyph lint／fmt／core check を担当し、共通 Search に触るため最終 `scripts/test-full.ps1` を行う。既に有効な結果は独立 reviewer が再利用し、修正・未解決懸念がないのに全 gate を重複実行しない。通常確認 build は tests green 後の `scripts/build-dev.ps1`。今回は build なし。

実機工程では事前に具体的な scenario・時間・desktop/input 使用・使い捨て data 範囲を示し、明示了承後だけ disposable portable で行う。通常 profile executable は agent が起動しない。Everything 未起動の検査のために利用者の client／service を勝手に止めない。隔離 fake 検証と、利用者による実 Everything 検索（日本語／複数 drive／favorite 外／大量 hit／cancel／本open-back／F12）は別証拠として記録する。

### 11.3 実装時に更新する文書

`docs/README.md` の入口、search-architecture、async-architecture、architecture-overview、top-level-grid-view、virtual-folders（検索由来本openの契約）、spec、keymap-spec／key-customization-impl-plan／keymap.ini.default、manual の検索・shortcut、製品ページの短い紹介をコードと一緒に更新する。IPC 接続と query／path の保存方針について privacy と index.html の安心セクションを突き合わせる。必要な MIT notice を既存配布 notice に記録する。detached paths を触れば §11。現行仕様の文書に設計案を先行して実装済みと書かない。

実装を始める前に独立 design review と下記利用者回答を反映する。実装の coherent chunk は (A) pure codec／transport＋fake、(B) common source／single adoption＋classification＋open/back/context、(C) UI/keymap/manual＋gates。B は §7.2 の PDF owner 境界の先行設計合意・必要な改修と回帰確認が揃うまで開始しない。A も今回の Stage 1 では実装しない。共有ファイルは各 chunk で実装者一人が所有し、独立 reviewer は読取だけ。Stage 1 の今回の writer は設計書と handoff message のみ。

## 12. 利用者への質問（この二点だけ）

1. **既定キーは Ctrl+Shift+G でよいか。** 代案は Ctrl+Alt+G、または既定なし（メニュー＋任意割り当て）。いずれも確認した既定 map には割り当てがなかった。利用者が最終決定する。
2. **初版を明示送信・raw 先頭 10,000 件・paging なし・完了後一括採用・main から検索・Remote 追加なしで進めてよいか。** 非対応 file/folder も raw 上限を消費するため、すべての対応形式ヒットを保証しない。対応候補への worker stat が必要で、cold HDD／UNC では一覧採用まで待つ費用がある（取消可能）。全対応ヒット取得や stat 前の段階表示が必須なら、実装前に prefilter／snapshot paging／thumbnail stamp owner の別設計へ広げる。

## 13. Stage 1 の結果と未確認事項

公式公開 wire と現行 owner を基に直接 IPC の実現性はあると判断した。コードは未変更で、IPC の actual version／reply HWND／timeout／10,000件の実機性能は未検証。ユーザー設定の hotkey conflicts、advanced query の版差、候補 stat／source-discovery の採用時間、physical 本 return と F12 registry transaction の全 caller は実装前の premise checks と独立レビュー対象。必要な wire constant が公開本文だけで足りない場合の停止条件は §3。

独立レビュー: 別コンテキストの GPT-6.1 Sol / xhigh。初稿で見つかった (1) metadata 未取得行では thumbnail enqueue が成立しない前提、(2) physical 本の既存帰路が DrillState に依存する前提は、対応候補の worker stat と typed Root/PhysicalChild の accepted boundary に改訂した。reviewer は両修正を設計上解消と判断した。

共有 PDF epoch の owner 境界 P1 は **未解消**。§7.2 にコード根拠と統合前の停止条件を記録した。追記確認後の reviewer 最終判定は「Stage 1 調査案に限定して ACCEPT」。統合実装 ready／full design ACCEPT ではない。PDF owner の具体設計合意・必要改修・回帰確認が揃うまで chunk B を開始しない。Stage 1 完了は実装開始や master 統合を意味しない。

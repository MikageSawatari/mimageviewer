# Everything ファイル名検索 (§1.374) — Stage 1 設計と実装台帳

作成: 2026-10-11。対象: v4.6.0 より後。`next-effetune`、基点 `31ac80b9c`。
**最新状態（2026-10-12）: 利用者の実装指示とPDF prerequisiteの独立ACCEPT/実装に基づく未コミット製品実装済み。
§14を現行の実装台帳とし、以下の「設計のみ」「製品コード未実装」「PDF未解消」はStage1時点の履歴として保持する。
最新の検証結果は§14.4。全libは完走して既知のEOF timing failure一件（単独再確認成功）。
確認build作成済み・未起動。Everythingの独立実装レビュー・利用者の実機確認は未完了。**
**設計のみ。実装開始・master への統合は未指示。** 初版 scope と既定キーは §12 の利用者決定を反映した。製品コードは未実装。2026-10-12: PDF owner の独立 subdesign と Ctrl+R の context 調査を追記。

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

Stage1の送信1秒/reply30秒は、実機timeoutの調査で§17の方針へ更新した。
statusは1秒（`SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT`）、queryのsend＋replyは合計60秒。
query sendは`SMTO_ERRORONEXIT`でbusy handlerを待ち、`SMTO_NOTIMEOUTIFNOTHUNG`と
callback再入を妨げる`SMTO_BLOCK`は使わない。API returnとreceiver acceptanceを別に検査する。
専用IPC runがHWND/send/pump/packetを一括所有し、検索prepare workerは完了/取消eventを待つ。
取消はnative sendの終了を待たず返し、旧runはdeadline付きのsendから戻って自然退役する。

待機は OS event／message pump と deadline で行い、`try_lock + sleep` や UI の `recv`／join を使わない。UI の cancel は token と wake event を立て、receiver を手放して即座に main 入力を戻す。IPC workerはnative send終了後に同じ reply HWND へ `nothing:` を一度だけ最大100msで送信し、返信を待たずwindowを破棄する。取消 query の成功は UI の取消完了条件にしない。旧 request の処理が Everything 側で即時停止する保証はしない。

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

2026-10-12: [PDF render epoch ownership の独立設計案](pdf-render-epoch-ownership-plan.md)を作成した。既存 Ctrl+G／通常一覧にもある不具合を `PDF-EPOCH-OWNERSHIP` として単独で適用できる scope に分離する。bundle の非 Clone writer、immutable request ticket、typed Unscoped、pool prune/pop、registry 全移譲、同 path main+detached 回帰を同案に定義した。独立レビューは利用者が依頼するため、今回内部レビュー／別 agent review を行っていない。設計を書いただけでは P1 解消とせず、同 stage の合意・改修・gate・必要な利用者確認まで B の停止条件を維持する。

これは Everything request の cancel/generation と異なる既存 PDF pool の所有境界で、現在の設計書では具体的な改修方式を最終承認していない。統合実装前に止める条件であり、まれな失敗として割り切らない。global bump を Everything だけ省く guard、全 render の epoch=0 化、F12／seek の無効化では代替しない。F12 の機能制限は利用者の明示了承なしに選ばない。分離した先行設計・改修の範囲と費用を提示してから統合へ進む。

実装で predicates／viewport／registry transfer を触る場合は `detached-rework-plan.md` §2 に沿う構造変更として独立レビューと設計担当の合意を得て、§11 に範囲・理由・試験を記録する。今回 §11 は編集していない。症状 guard・追加時間窓・HWND geometry capture・placement 保存先を作らない。

### 7.3 Remote

初版では Web UI、protocol enum、Everything query endpoint、collection type を増やさない。Remote collection は settings／独自 worker から一覧を構築しており、PC `GlobalSearchState` を公開する経路ではない (`remote_ipc/collections.rs`)。main の結果を「現在地」として自動配信しない。Remote session の既存 local-control gate を尊重し、取得中の main modal と Remote acquire を同じ受付判定で排他にする。

通常の物理 file address と利用者が作った永続 Collection を Remote で閲覧できる現行仕様は維持する。Everything 結果から既存 Collection に明示追加した file が後で Remote に見えるのは既存の Collection 機能である。新 feature の query／結果 owner を Remote に渡すこととは区別する。IPC 版更新は初版では不要の見込みだが、実装で contract を変えるなら scope を返す。

## 8. KeyAction とメニュー

提案 action: `GlobalEverythingSearch`、日本語説明「Everything でファイル名を検索する」、`Press`、main grid からだけ実行。**既定キーは Ctrl+R（利用者決定 2026-10-11/12）**。一修飾・左手で操作する意図を反映する。初稿の Ctrl+Shift+G 推奨は置き換えた。Ctrl+G/S/E の現行操作は維持する。

2026-10-12 の context／fixed-key 調査（`78e5f09ad`）:

| 発火面／リスト | 調査結果と実装契約 |
| --- | --- |
| main grid | `docs/keymap.ini.default` と `src/keymap.rs` の default chords に Ctrl+R はない。`GRID_ACTIVE_SCOPES` (`keymap.rs:6988`) は Global/Grid/Rating。Grid action として追加し、caller は main の実一覧を確認する |
| fixed/reserved keys | `RESERVED_BINDINGS` (`keymap.rs:7143`) に Ctrl+R はない。`src` の R key／VK 0x52 の直接参照に固定 Ctrl+R handler は見つからなかった |
| 画像 fullscreen | `FS_IMAGE_ACTIVE_SCOPES` に Grid はない。`ui_fullscreen.rs:7967–7988` の R は `is_fullscreen_shortcut_probe_key` の検出用集合。handler 起動の可否を決めるだけで consume／action 割当ではなく、Ctrl+R 競合とはしない |
| text／IME／focus recovery | `keyboard_input.rs:223` の `shortcut_permit` は TextInput 全 phase と Modal を拒否する。新 shortcut は text 入力を奪わず、IME preedit／commit／検索バー focus 中に起動しない。`preferences/pages.rs` の R は key chooser の候補で固定操作ではない |
| egui 動画／音声 fullscreen | `FS_VIDEO_ACTIVE_SCOPES` (`keymap.rs:6998`) に Grid はない。`handle_video_input`／fullscreen handler に固定 Ctrl+R は見つからなかった。新 action は送らない |
| native 動画／音声 | `app/native_video.rs:12296` の key routing／fixed VK branches に Ctrl+R 操作は見つからなかった。`video/native_presenter/render_core.rs:16152` の 0x52→R は key 変換であり割当ではない。native dispatch に新 Grid action を追加しない |

確認した repository の default／固定経路では **競合なし**。利用者の独自 keymap と OS／他アプリの hotkey は未検査であり、現在の customization conflict warning を維持する。実装時の再棚卸しで新たな競合が見つかったら利用者へ報告し、別キーへ勝手に変更しない。

`KeyContext::Grid` とし、caller 側でも main grid・IME／text focus・modal／Remote lock の適格性を保証する（helper の chord 判定だけで context enforcement されたと思わない）。名前の Global は検索範囲を示し、発火 context を Global にしない。fullscreen の active scopes は Global を含むが Grid を含まない。fullscreen／native video／detached から検索バーを出す転送を追加しない。操作カスタマイズの候補にも実行 scope を示す。

実装時に `ini_name`／description／context／trigger／default_chords／ALL_ACTIONS と同梱 default を一緒に更新する。menu の「Everything ファイル名検索…」と key は同じ entry transaction に接続し、caption は現在の keymap から表示する。初版にリング／gesture 専用 action は増やさない。今回 keymap の product/default file は未編集。

## 9. 利用不能・失敗の案内

| 状態 | 動作と案内 |
| --- | --- |
| IPC window 不在（未導入／client 未起動／service だけ／named instance のみ） | 「Everything に接続できません。インストール済みの場合は検索クライアントを起動してください」。これらを HWND 不在だけで区別したと主張しない。手動の再検索と公式案内リンク |
| Lite／IPC 無効／QUERY2 拒否／古い版 | 「この Everything では IPC 検索を利用できません。通常版の 1.4.1 以降と IPC 設定を確認してください」。lite 検出を version だけで断定しない |
| DB 未ロード／再構築中 | 成功の 0 件を採用しない。「Everything の索引を準備中です。準備後にもう一度検索してください」。自動 wait/retry はしない |
| DB loaded だが folder index が更新途中 | 得られた時点の snapshot を表示。全ドライブ／全 folder の完走や最新性を保証しない。loaded は全索引更新完了の証明ではない |
| 送信失敗時に Windows が対象 HWND を応答なしと判定 | `NotResponding`。log＋「Everything が応答していません。起動直後の索引の読み込みや更新などの処理が終わるまで待ってから、もう一度検索してください」。検索条件の絞り込みを勧めない |
| 応答 deadline／送信timeout（応答なしと判定されない） | `Busy`／`Timeout`。log＋「Everything の処理が遅いか、混雑しています。しばらく待つか、検索条件を絞って再検索してください」。未起動案内にまとめない |
| 終了／UIPI 拒否等 | log＋「Everything から結果を取得できませんでした。起動状態・権限を確認して再検索してください」。mIV を勝手に elevate しない |
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
| keys／UI | inventory／generated default、Ctrl+R の main Grid 発火・無効化・任意再割当、Ctrl+G/S/E 非競合、画像fullscreen／egui動画／native動画／detached／text focusでの非発火、R検出probeがあるだけでは新actionをdispatchしない、IME preedit/commit/Esc/Tab、日本語 pasted query、Window ×取消、長い通知、small screen／100%・200% snapshot、progress 消失後の入力復帰 |

fake Win32 server が必要な adapter integration は、専用 thread のテスト用 class を **注入した HWND** として使う。default Everything class を検索しない、実 client を終了・操作しない、foreground/input を使わない。実 transport の wire／callback lifetime／SendMessageTimeout と Win32 parser をここで検査する。real Everything の compatibility 確認は別の承認済み実機工程であり、自動 suite の必須依存にしない。

### 11.2 性能と後続の手動確認

計装区間: discover/version/readiness、send、reply-copy、parse/classify、prepare、adopt、first-visible-thumb。source／run／context と件数・byte・elapsed を記録する。UI install に同期 FS／DB／IPC がないことを call graph で確認する。10,000 件 fake の UI drain／adopt、旧 payload 解放、back／cancel、idle repaint の既存予算を測る。計測前の「瞬時」「Small」で受入としない。

実装者が narrow lib tests／keymap／App search/context regressions／snapshot／glyph lint／fmt／core check を担当し、共通 Search に触るため最終 `scripts/test-full.ps1` を行う。既に有効な結果は独立 reviewer が再利用し、修正・未解決懸念がないのに全 gate を重複実行しない。通常確認 build は tests green 後の `scripts/build-dev.ps1`。今回は build なし。

実機工程では事前に具体的な scenario・時間・desktop/input 使用・使い捨て data 範囲を示し、明示了承後だけ disposable portable で行う。通常 profile executable は agent が起動しない。Everything 未起動の検査のために利用者の client／service を勝手に止めない。隔離 fake 検証と、利用者による実 Everything 検索（日本語／複数 drive／favorite 外／大量 hit／cancel／本open-back／F12）は別証拠として記録する。

### 11.3 実装時に更新する文書

`docs/README.md` の入口、search-architecture、async-architecture、architecture-overview、top-level-grid-view、virtual-folders（検索由来本openの契約）、spec、keymap-spec／key-customization-impl-plan／keymap.ini.default、manual の検索・shortcut、製品ページの短い紹介をコードと一緒に更新する。IPC 接続と query／path の保存方針について privacy と index.html の安心セクションを突き合わせる。必要な MIT notice を既存配布 notice に記録する。detached paths を触れば §11。現行仕様の文書に設計案を先行して実装済みと書かない。

実装を始める前に独立 design review と下記利用者回答を反映する。実装の coherent chunk は (A) pure codec／transport＋fake、(B) common source／single adoption＋classification＋open/back/context、(C) UI/keymap/manual＋gates。B は §7.2 の PDF owner 境界の先行設計合意・必要な改修と回帰確認が揃うまで開始しない。A も今回の Stage 1 では実装しない。共有ファイルは各 chunk で実装者一人が所有し、独立 reviewer は読取だけ。Stage 1 の今回の writer は設計書と handoff message のみ。

## 12. 利用者決定（2026-10-11/12、回答済み）

1. **既定キーは Ctrl+R。** context と固定リストの調査を §8 に記録した。repository default／fixed-key の競合は見つからず、この決定を設計へ採用する。
2. **初版 scope は提案どおり承認。** 明示送信・raw 先頭 10,000 件・paging なし・完了後一括採用・main から検索・Remote 追加なし。非対応 file/folder も raw 上限を消費し、対応候補への worker stat の待ち費用がある前提を維持する。

同じ質問を再要求しない。PDF ownership の独立設計は利用者の今回の指示で作成し、独立レビューは利用者の依頼待ち。追加の利用者質問はない。scope 決定は実装開始や独立レビューの代わりにはしない。

## 13. Stage 1 の結果と未確認事項

公式公開 wire と現行 owner を基に直接 IPC の実現性はあると判断した。コードは未変更で、IPC の actual version／reply HWND／timeout／10,000件の実機性能は未検証。ユーザー設定の hotkey conflicts、advanced query の版差、候補 stat／source-discovery の採用時間、physical 本 return と F12 registry transaction の全 caller は実装前の premise checks と独立レビュー対象。必要な wire constant が公開本文だけで足りない場合の停止条件は §3。

独立レビュー: 別コンテキストの GPT-6.1 Sol / xhigh。初稿で見つかった (1) metadata 未取得行では thumbnail enqueue が成立しない前提、(2) physical 本の既存帰路が DrillState に依存する前提は、対応候補の worker stat と typed Root/PhysicalChild の accepted boundary に改訂した。reviewer は両修正を設計上解消と判断した。

共有 PDF epoch の owner 境界 P1 は **未解消**。§7.2 にコード根拠と統合前の停止条件を記録した。追記確認後の reviewer 最終判定は「Stage 1 調査案に限定して ACCEPT」。統合実装 ready／full design ACCEPT ではない。PDF owner の具体設計合意・必要改修・回帰確認が揃うまで chunk B を開始しない。Stage 1 完了は実装開始や master 統合を意味しない。

2026-10-12: Stage 1 計画は利用者が `78e5f09ad` として commit。§12 の回答を反映し、[PDF ownership の独立設計案](pdf-render-epoch-ownership-plan.md)を追加。**今回の新設計・追記に対する内部／独立レビューは行っていない。** 先の限定 ACCEPT は Stage 1 の履歴であり、今回の subdesign の ACCEPT ではない。製品コード・.git は未変更、tests/build/launch/download なし。

## 14. 実装記録（2026-10-12）

### 14.1 開始条件と一次資料

利用者から製品実装の開始指示を取得した。PDF ownership prerequisiteは独立レビューACCEPT済みの
実装 `4510346f5`、master統合 `c1e66f589` として提供され、§7.2のchunk B停止条件を満たす。
Stage1の限定ACCEPTをEverything製品実装の独立受入へ読み替えない。今回の実装は完了後に別の独立レビューを受ける。

IPCの未確認定数で一度安全checkpointを取り停止した後、公式[CLI案内](https://www.voidtools.com/support/everything/command_line_interface/)が
リンクする作者の[ES公開header](https://github.com/voidtools/ES/blob/main/src/everything_ipc.h)により、
`IS_DB_LOADED=401`、`QUERY2W=18`、FULL_PATH request flag、folder/drive/root flag、query/replyのDWORD幅を確認した。
DRIVEとROOTは同値2で、folderは1。SDK ZIP/DLL/headerをdownloadしていない。
定義の出典と[ESのMIT licence](https://github.com/voidtools/ES/blob/main/LICENSE)のnoticeを配布noticeへ載せる。
Everything本体を同梱しない。実client/process/索引への検索は自動gateの前提にしない。

### 14.2 実装の所有境界

- `everything_ipc.rs` / `everything_ipc/native.rs`: pure codecとworker-only QUERY2 transport。
  不可視reply HWND、callback copy、bounded send/deadline、request ID相関、drop/cancelを一つのownerへ閉じる。
  fake endpoint/注入HWNDの検証は実Everythingをdiscoverせず、foreground/inputも使わない。
- `everything_results.rs`: 通常の対応拡張子/利用可能Susie形式と送信時visibility/EPUB/変換設定を使い、
  pathを正規化/重複除去して対応候補だけstatする。mtime/size/sort metadataとraw/除外/読取不能/重複/上限件数を
  immutable `EverythingDataset` に持つ。画像decode、内部archive probe、DB書込み、同名collapse/sidecar探索は行わない。
- `GlobalSearchState`: draftの `SearchSource`、排他的pending `SearchProducer`、accepted `SearchAccepted` を持つ。
  MetadataとEverythingは共通 `SearchPageEditPrepare` / `SearchViewWish` / adoption closureを使う。
  request run/query/context/items generation/policyを照合し、完成後だけitems/metas/source map/件数/★の副作用を一括採用する。
  大きい送信時Settings snapshotは同producer内のBoxで所有し、App/bundleのstackへインライン展開しない。
  query/sourceのdraft変更・取消・失敗は既存accepted datasetの出所を上書きしない。
- mainの取得/prepareを取消可能modalにして、open/navigation/query切替の組合せを制限する。
  producerはRemote sessionの非Clone local-modal leaseを保持し、acquireと同じmutex/typed ownerで排他にする。
  公開Remote phase/schema/APIは変更しない。終了後の通常Remote操作を妨げない。
  UIには同期IPC/FS/DB/worker joinを増やさない。希少失敗はlog＋通知で扱い、retry/recovery state/DB再作成を追加しない。
  pin/タグ照会と変換書庫のPending登録は共通prepareで完成させ、UIはprepared aggregate installとmap交換を使う。
  初期化済みDB handle/pathだけをworkerへ渡し、TAG_WRITES/pin mutation stampの通常変更は既存freshnessへ戻す。
  同datasetのsort/facetだけがrating membershipを移譲する。新query候補は新sourceとして採用し、
  ★のneighbor判定もHashSetの集合交差にして大量disjoint結果のUI二乗探索を避ける。
- accepted Everythingは `EverythingPosition::Root` / `PhysicalChild { logical_source, return_anchor, route }`。
  childへは通常物理openの成功採用境界だけで遷移する。ZIP内部親移動は既存navを維持し、外側の戻りは保持datasetを
  共通prepareで再投影する。IPCを再送しない。sort/★/tag変更も保持datasetと取得時stampから準備する。
- 既存thumbnail/context所有とPDF prerequisiteを再利用し、F12のopen/fork/park/promoteを維持する。
  mainのdataset/return originをdetachedへ複製しない。兄弟items/queue/cache/PDF ticket/編集状態を変更しない。
  変更caller familyは物理本open採用・検索帰路・main modal受付であり、detached predicateの症状パッチではない。
  [detached計画 §11](detached-rework-plan.md#11-リワーク外からの変更記録)にも変更範囲を記録する。

### 14.3 UI、保存、文書

`GlobalEverythingSearch` はGrid/Press/Ctrl+R。`MenuCommandId::GlobalEverythingSearch` をファイルmenuに追加し、
captionは実効キーから生成する。main一覧/text/IME/focus recovery/modal/Remote gateを共通受付へ接続する。
fullscreen/native/detachedからの検索起動を増やさず、Rの検出probeをdispatchへ読み替えない。
IME確定後Enter/検索ボタンで明示送信し、raw先頭10,000件の後で形式/表示条件を適用する。
paging/追加取得、自動ext追記、Ctrl+Gの自動集約/hit-count sort/全文対象filterは追加しない。

新しいSettings field/schema/DB/索引/検索履歴/Remote APIは追加せず、settings-transfer項目数を変更しない。
query/result一覧はmIVの永続データや診断ログに残さない。Everything自身のquery history設定は変更しない。
結果からの通常open/tag/★/cacheは既存の保存契約に従う。
keymap spec/default/実装計画、検索/shortcut manual、privacy、製品ページ、関連architecture/virtual/top-level/spec/READMEを同期する。

### 14.4 検証の状態

parentが未コミット差分の検証を集約する。全件数は各filterの実行件数で、重複を含む。
新規テストはfake IPC endpoint/注入HWND・immutable dataset・一時DBを使い、実Everythingへ問い合わせない。
自動検証の成功を最新実装の独立ACCEPTや実Everything互換性の受入へ読み替えない。

| 検証 | 結果とログ（`target/`） |
| --- | --- |
| `cargo fmt --all -- --check` | exit 0、`EV-1374-fmt.log` |
| normal / portable core check | policy heap化後も両方exit 0、`EV-1374-check-normal-final.log` / `EV-1374-check-portable-final.log` |
| `--lib everything` | 59 passed、`EV-1374-focused-everything.log` |
| `--lib global_search_ui` / `keymap` | 80 / 165 passed、`EV-1374-focused-global_search_ui.log` / `EV-1374-focused-keymap.log` |
| `--lib remote_ipc::session` / `viewer_context_registry` | 52 / 84 passed、`EV-1374-focused-remote_ipc-session.log` / `EV-1374-focused-viewer_context_registry.log` |
| `--lib thumbnail_request` / `pdf_loader` | 48 / 105 passed、同名の`EV-1374-focused-*.log` |
| `--lib settings_transfer` / `keyboard_input` / `ime_focus` / `rating_membership` | 24 / 16 / 23 / 80 passed、同名の`EV-1374-focused-*.log` |
| new `ui_snapshot everything_search_` | 5 passed、`EV-1374-ui-snapshot.log`。light/dark/narrow、100%/200%進捗。新PNG5枚だけを追加 |
| full `ui_snapshot` | 162 passed、`EV-1374-full-gate.log`。描画を変えないpolicy heap化後も有効 |
| `python scripts/check_ui_glyphs.py` | exit 0、`EV-1374-glyphs.log` |
| 10,000行の単独synthetic回帰 | heap化後も1 passed、drain最大6.853ms、初回採用5.863ms、置換採用18.549ms、`EV-1374-performance-final.log` |
| full workspace gate初回 | exit 101。Appサイズ113,256が既存110,000上限を超え、capture-region testのstack overflowでlibが途中終了。`EV-1374-full-gate.log`。UI snapshotは162 passed、他workspace targetは成功。完走済みfull libとして扱わない |
| policy heap化の再確認 | Appサイズ105,400で既存上限内、capture-regionも1 passed。`EV-1374-stack-final.log` / `EV-1374-capture-stack-final.log`。上記全焦点filterも修正後に成功 |
| final full lib | 修正後に完走、12,206 passed / 1 failed / 56 ignored、exit 101、1106.75秒、`EV-1374-full-lib-final.log`。`cargo test -p mimageviewer --lib --features pack-build-tools`。process内のcrash dialogだけを抑える |
| EOF単独再確認 | 同featureで完全名`--exact --nocapture`、1 passed、exit 0、74.87秒、`EV-1374-eof-recheck.log` |
| workspaceでスキップされたvendor gate | egui 25 / egui-wgpu 11 / eframe 18 passed、全てexit 0、`EV-1374-vendor-summary.txt`と各`EV-1374-vendor-*.log` |
| `scripts/build-dev.ps1` | exit 0、core / Remote companion / EPUB workerとruntime DLLを準備、`EV-1374-build-dev.log`。normal feature / dev-runtime profile、未起動 |

初回のfresh dataset回帰でShift anchorをNoneとした期待値は、既存のfirst-visible選択が
新世代のanchorも先頭へ設定する仕様に合わせた。keymap既存menu順テストも新commandを追加した。
製品の選択規則やassertを弱めて通していない。両方の再検証は成功。
10,000行の置換で約296msだった旧/新集合の二乗探索を、source境界とHashSet交差へ修正した。
測定はdebug/fake endpointの同一工程であり、実Everything/PDFium/実アプリの性能受入ではない。
pin/タグを書き換えてからheld Readyを解放する回帰は、古いmapを公開せず共通workerで再準備し、
新mapだけ採用することとUIの同期タグprewarmを通らないことを確認した。

final full libの失敗は`video::tests::real_decoder_eof_full_loop_preserves_single_tail_burst_per_epoch`、
`src/video/mod.rs:13261`の`try_send_native_first_frame_ready(...)` assert。対象ソースは今回無変更で、
利用者がPDF段階の同じ失敗を既存load-timing flakeと判定したものに一致する。単独再確認は成功した。
その成功を全lib gate成功へ読み替えず、対象外の修正・assert緩和・全libの再実行は行わない。
初回のAppサイズ／capture-region overflowは今回の変更による不具合として修正し、
影響する焦点回帰とfinal full libで両方成功した。UI snapshotや他workspace targetの成功は、
描画・protocolを変えないpolicy heap化の後も有効なので再利用する。
OS error 1455なし。実装者による追加の独立レビューは行っていない。
最終`git --no-optional-locks diff --check`もexit 0（`EV-1374-diff-check.log`）。
利用者向け手順と通常APPDATAの注意はUTF-8 BOM付き`target/EV-1374-impl-msg.txt`へ記録した。

実Everything compatibility/性能/日本語query/本open-back/F12は、利用者が通常profile確認binaryを
起動する別証拠とする。agentは通常profile binaryを起動しない。確認build後に独立reviewへ渡す。

## 15. 実Everything SendFailed調査（2026-10-12）

利用者はHEAD `3ce0040d3` のdev buildを隔離`--data-dir`で起動し、
Everything 1.4.1.1032（利用者申告）の`genshin_random yaoyao`でSendFailedを観測した。
利用者ログ`target/ev-try/data/logs/mimageviewer.log`は読取のみとし、agentはmIVを起動していない。

`everything_ipc::native::adapter_tests::real_everything_diagnostic_probe`は明示実行専用の
ignored test executableであり、製品のdiscovery/encode/send/pump/parserを呼ぶ。
自動gateは常にskipし、実Everythingをテストの依存先にしない。queryは明示環境変数からだけ読む。
Codex実行環境の専用desktopではFindWindowがUnavailableだったため、このprobeのthreadだけを
入力desktopへ割り当てて測定した。表示desktopの切替・mIV起動・Everythingの再起動や設定変更はしない。
desktop操作は`cfg(test)`内のこのignored probeに限定し、製品のtransportに追加していない。

現在の観測（`target/EV-1374-real-probe-final.log`）:

- FindWindow class=`EVERYTHING_TASKBAR_NOTIFICATION`、title=NULL、target HWND=`0x47e21d8`、
  PID=91556、thread=79152。`IsHungAppWindow=true`。
- 返信用message-only HWNDは同じprobe workerで生成・破棄。最初のWM_USER/major照会で
  `SendMessageTimeoutW`が0、response=0、直前にクリアしたGetLastErrorも0。約0.01秒で失敗する。
  minor/revision/build/DB-loaded/admin照会も失敗し、版・索引準備状態をIPCでは取得できなかった。
- SMTO_NORMALまたはERRORONEXITだけの補助送信でも1秒timeout、error=1460。
  ABORTIFHUNG入りでは即座にgeneric failure、error=0。延長やflag変更を製品へ採用していない。
- QUERY2Wはid=18、packed header=28、今回のpacket=72 bytes。
  QUERY1Wはid=2、header=20、packet=64 bytes。両方とも同じ送信失敗でcallbackに到達しない。
  x64 native COPYDATASTRUCTは24 bytes、dwData/cbData/lpDataのoffset=0/8/16。
- process tokenの補助読取ではprobe/Everythingとも非昇格、integrity RID=8192（medium）。
  返信HWNDだけのChangeWindowMessageFilterEx(WM_COPYDATA, ALLOW)は成功したが、送信失敗は変わらない。
  これを権限差や返信filter不足の修正根拠にせず、製品のmessage filterは変更しない。

要求形式は[作者の公開header](https://github.com/voidtools/ES/blob/main/src/everything_ipc.h)と一致する。
error=0も失敗として扱い、事前クリアする方法は
[Microsoft SendMessageTimeoutW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendmessagetimeoutw)に従う。
[ChangeWindowMessageFilterEx](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-changewindowmessagefilterex)
は受信windowのfilterだけに作用し、相手threadの応答停止を解決しない。
公開textを閲覧しただけで、SDK/header/DLL等のdownloadは行っていない。

この時点で確認した失敗地点はEverything通知windowの応答停止であり、検索packetを送る前である。
Everything内部の停止理由と、応答するclientでの互換性は未確定。利用者へ直接検索の応答確認を依頼した。
wire/layout/filterの欠陥と断定して修正を作らない。製品側の変更は失敗したmessage/status番号と
Win32 errorの非blocking診断を加えること。query/result pathは記録せず、既定のlog＋通知と明示再検索を保つ。
fake HWNDのversion handlerを停止させる回帰で、bounded failure・query/cancel/retry未送信・
reply HWND破棄を検証する。既存fake codec/再入callback/cancel回帰は維持する。
検索・adoption・UI・keymap・Remote・viewer-context ownershipは変更しない。

この差分の検証はIPC 14 passed / 1 ignored、関連`--lib everything` 60 passed / 1 ignored。
normal/portable core check、fmt、UI glyph（危険文字0）、diff --checkは成功。
source/UI/ownerの無変更部分の先行検証は§14.4を再利用し、full lib/UI snapshotを再実行していない。
`scripts/build-dev.ps1 -PreserveRuntime`はexit 0。実行直前にこのworktreeのcoreなしを確認した
（`target/EV-1374-ipc-fix-process-check.txt`、global core数も0）。
新coreは2026-10-12 04:29:59、325,150,208 bytes。agentによるmIV起動なし。
詳細ログは`target/EV-1374-ipc-fix-{tests,everything,check-normal,check-portable,build-dev}.log`。
manual real probeのexit 101は上記の外部client応答停止の観測であり、fake自動gateの失敗に数えない。
実検索成功・Everything内部の停止理由・必要な製品修正の特定は残る。結果を「IPC修正済み」と扱わない。

利用者の追加確認: PID91556はResponding=False、22:45からCPU約19,000秒でbusy/hung。
これがSendFailedを説明する。初期indexingは利用者の推定であり、IPCで取得した状態ではない。
利用者判断に従って応答回復後に再測定する。Everything設定は変更しない。

## 16. P2: 動画pinのworker準備（2026-10-12）

独立reviewはP1/P3なし、P2としてEverything完成採用の動画pin SQL/WebP BLOB読取を指摘した。
`replace_search_view_items_with_ratings`が`respawn_search_video_thread`を呼び、
従来はUI上で`lookup_webps_many`（500件ずつSQL）を実行していた。完了採用に同期DB I/Oを
含めない§11.2の契約に反するため、pin取得を完成metadataのproducerへ移す。

`SearchMetadataResources`は初期化済みVideoPinDbから、cached connection filenameと
読取専用revision publicationを捕捉する。path取得はSQL/FS/既定profile探索を行わない。
SQLite connectionと変更revisionのwriterはVideoPinDbに保持し、非CloneのDBをworkerへ共有しない。
prepare workerは実pathをreadonlyで開き、表示動画のpin BLOBを一括取得して
`EverythingMetadata.video_pin_blobs`のArcマップと、読取前のinstance/revision stampを返す。
UIのfreshnessチェックはpublicationだけを読み、変更・同pathのDB交換でstaleなら既存再準備へ戻る。
pin変更がworker作成後に起きても、publicationは最新revisionを読めるので古いspawn時stampへ固定されない。

採用は`SearchVideoPins::Prepared`を動画thumbnail workerへ渡す。空mapも取得済みと明示し、
UIでDB fallback・BLOB copy/decodeを行わない。別items/cache/worker ownerやAppのpending状態は追加しない。
Rootの初回・置換・再sort・物理本からの帰路はいずれも既存の同じprepare/adoption経路を通す。
旧payload/cancel/stale退役は既存のworker retirementとitems generationを使い、兄弟contextを失効しない。
まれな失敗は既定のlog＋通知で扱い、DBを作成し直したり削除したりせず、回復手順を増やさない。

Ctrl+Gにも同じ同期lookupがあることを実装前に利用者へ報告した。
`SearchMetadataInstall::Existing`→`SearchVideoPins::Existing`に従来のlookupとstreaming時のpin-only規則を残す。
Ctrl+Gを移行するには既存streaming/rebuildの準備契約を別件で扱う必要があり、今回の範囲を広げない。
これは今回追加したEverythingの同期I/O漏れを既存producer境界で直す修正であり、一般的なDB actor導入ではない。

回帰は既存10,000行fixtureへ5,000動画＋pin DBの実WebP BLOBを加える。初回・置換のReadyに
全pin BLOBが含まれること、UI側DBのbulk lookup件数が0のままで採用・本からの帰路を行えることを確認する。
兄弟の同PDF ticket/generation/cancel/itemsは従来通り不変。readonly worker接続とDB更新/交換/削除の
freshness回帰も追加する。実Everythingを自動テストの依存先にしない。

### 16.1 検証記録

- `cargo fmt --all -- --check`、normal/portable core check、UI glyph（危険文字0）は成功。
- 焦点test: `global_search_ui` 81、`everything` 61（実probeは1 ignored）、
  `video_pins` 17、`viewer_context_registry` 84、`thumbnail_request` 48 passed。filter間の重複あり。
  ログは`target/EV-1374-P2-test-{filter}.log`。
- 動画/BLOB入り10,000行fixtureの初回観測はmax UI drain 3.282 ms、初回adoption 15.761 ms、
  replacement 28.007 ms、帰路adoption 23.108 ms。実DBの5,000件のpinがReadyに含まれ、
  初回/置換/取消/完成帰路のUI側bulk lookupは0。tiny WebPを使うsynthetic fixtureであり、
  実ファイル/実画面全体の性能保証へ一般化しない。`target/EV-1374-P2-everything-tests.log`。
- 最終full libは1回だけ実行: `cargo test -p mimageviewer --lib --features pack-build-tools`。
  **12,209 passed / 0 failed / 57 ignored**、exit 0、1057.06秒。
  `target/EV-1374-P2-full-lib.log`。新pin回帰も成功し、前回のEOF flakeも今回は成功した。
  test processだけのcrash-dialog抑止は既存helperを再利用し、desktop/製品設定を変更しない。
- UI layoutは変更せず、先行のfull UI snapshot 162 passedを再利用。実Everything probeは実行しない。
- `scripts/build-dev.ps1 -PreserveRuntime`はexit 0。直前にglobal/worktree core数0を確認
  (`target/EV-1374-P2-process-check.txt`)。normal feature / dev-runtimeのcore・Remote companion・
  EPUB workerをbuild/stageした。coreは2026-10-12 05:17:48、325,190,144 bytes。
  `target/EV-1374-P2-build-dev.log`。起動/Everything設定変更/downloadなし。
- `git diff --check`成功 (`target/EV-1374-P2-diff-check.log`)。OS error 1455なし。差分は未コミット。

## 17. WM_COPYDATAの遅い送信と取消（2026-10-12）

§16のP2修正は利用者/独立reviewがACCEPTし、`5a0d92e21`へcommitされた。
今回、利用者の実機再試行はversion照会を通過したが、query送信が1秒でERROR_TIMEOUTになった。
ログのWM_COPYDATA失敗2回はqueryとbest-effort `nothing:`の取消であり、自動query retryではない。

### 17.1 実機観測と公開根拠

- readonlyの`target/ev-try/data/logs/mimageviewer.log`で、約1秒ずつのWM_COPYDATA/error1460を確認。
- standalone ignored probeのみ実行。対象は通知window `0x47e21d8`、PID91556/thread79152。
  Process.Responding=Trueでも`IsHungAppWindow`はtrue。version/DB-loadedもprobe時点では送信失敗した。
  ABORTIFHUNG付き60秒budgetではqueryが約0.205msで早期失敗（Win32 error0）した。
- 診断用に一回だけABORTIFHUNGを外した同じQUERY2W送信は**59,999.845msでerror1460、返信なし**。
  これは「sendが60秒以内に戻らなかった」という下限であり、実検索の完了時間は測れていない。
  messageのqueue待ちとhandler内のどの処理で止まったかを、この観測だけでは区別できない。
  ログ: `target/EV-1374-slow-real-probe.log` / `EV-1374-slow-real-probe-no-abort.log`。
- 05:39:29→05:42:08のreadonly thread観測で、通知threadのCPUは21.90625秒のまま/Wait状態。
  Process CPUは22,965.328125→23,119.359375秒。別threadの処理を待つ状況と整合するが、
  内部の関数/lock/停止理由の特定ではない。`target/EV-1374-slow-thread-sample-{1,2}.json`。
- [公式SDK Query](https://www.voidtools.com/support/everything/sdk/everything_query/)はFALSE時の
  非同期結果受信を提供する。一方、[作者の説明](https://www.voidtools.com/forum/viewtopic.php?t=8282)では
  別IPCがDB完了を待つこと、replyの組立/送信が取消できないことを明示している。
  非公開のEverything 1.4内部で「全検索がWM_COPYDATA handler上で実行される」と断定しない。
  transport契約として**query send自体がDB処理/応答準備を待ち、長く戻らない可能性**を扱う。
- [作者公開header](https://github.com/voidtools/ES/blob/main/src/everything_ipc.h)のQUERY2順序
  （request_flags→sort_type）、raw cap、full-path flag、名前昇順を再確認。wire変更なし。
  [Microsoft SendMessageTimeoutW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendmessagetimeoutw)
  のbounded timeout/ABORTIFHUNG/再入契約に従う。SDK/DLL/header等のdownloadは行わない。

### 17.2 待機と所有境界

1秒のSendMessageTimeout上限では普通の遅いhandlerも失敗するため、statusの1秒とqueryの
send＋reply合計60秒を分ける。60秒以内の検索完了を保証する値ではなく、長い処理を許容する
有限の待機上限である。今回の実clientの検索成功は未確認。deadline延長retry/無期限waitは行わない。
queryはERRORONEXITだけで待ち、5秒以上pumpしない計算/DB待ちをABORTIFHUNGで早期打切りしない。
statusと100msのbest-effort取消はABORTIFHUNG付き。NOTIMEOUTIFNOTHUNG/BLOCKは使わない。
native送信引数は呼出し側budgetを尊重し、旧1秒への再clampを取り除いた。

長いSendMessageTimeoutだけでは検索workerも取消を観測するまで止まるため、その案だけでは済ませない。
検索prepare workerが開始する`everything-ipc` runは、query/取消token/一件用結果channelを所有する。
IPC runのthread一つが不可視reply HWND、callback Box、packet、sendとpumpを一括所有し、
handler完了前の返信をコピーし、parse/終端処理まで同threadを生かす。Everythingのcaller-thread監視も考慮する。
thread-affine WindowsEndpointや借用bufferを別threadへ渡さない。

検索workerは`WaitForMultipleObjects`で取消eventと完了eventを待つ。取消はnative sendを待たず戻り、
receiverを手放す。IPC runはOSのbounded sendから戻って取消を観測し、active IDを失効して
`nothing:`を最大100msで一回送信し、返信を待たずRAIIでHWNDを破棄する。旧callback/packet/送信bufferは
send終了まで保持し、遅い完了は廃棄する。UI/検索workerでjoinせず、Everything側の処理停止は保証しない。
callerにもstatus4回＋query60秒＋cleanupを含む65秒の上限を置く。caller側timeout/errorは
利用者の取消flagを立てず、既存失敗経路へ通知する（取消との混同で失敗通知を消さない）。
run/channel/eventは作成した検索だけが持ち、一覧、App/bundle field、Remote API、PDF epoch ownerは増やさない。
元のrun/context/generationによる旧完了棄却とcontext退役の契約を保持する。

当時はtimeout/ERROR_TIMEOUTと実hungを`Busy`/`Timeout`として同じ「処理が遅いか、混雑しています」案内へ接続した。2026-10-14の分類分離は§19参照。
HWND不在のclient起動案内、索引未ロード、拒否/権限等の案内とは区別する。
失敗logはmessage/status/errorにbudget/elapsedだけを追加し、queryやpathを出さない。
まれなspawn/event/native失敗はlog＋既存通知。partial結果/回復owner/retry loopを追加しない。
Everythingの設定、service/clientの起動・終了、mIV起動、downloadは行っていない。

### 17.3 回帰と確認範囲

fake HWNDがquery handler内で1.2秒停止してから再入返信する回帰で、旧1秒capを超えて成功することを確認する。
別のfake handlerをeventで停止し、cancel後に検索workerの結果が先にCancelledとなり、
旧HWNDはsend終了までIPC workerに保持され、handler解放後に破棄される順序を確認する。
fake elapsed endpointではsend/replyが一つの60秒deadlineを共有し、timeout後の取消は一回/100msであること、
status Busyではquery/取消/retryを送らないこと、Busy/Timeout文言が未起動と異なることも確認する。
fake codec/reentrant/cancel/correlation回帰を維持し、実Everything probeは自動gateから除外する。
Busy文言を検索barのsnapshot fixtureへ反映し、明暗/狭い幅の3 PNGを更新する。

### 17.4 検証記録（2026-10-12）

fmt check、normal/portable core check、UI glyph（危険文字0）が成功。
Everything関連libは65 passed / 1 ignored（実client probe）、global_search_uiは81 passed。
Everything UI snapshotは更新5 passed、更新flagなしの確認も5 passed。変更した3 PNGを目視確認した。
今回のprivate transport/案内の変更ではfull libを再実行せず、共有一覧/pin/PDF/decoder等は
先行ACCEPTed `5a0d92e21` の12,209 passed / 0 failed / 57 ignoredを再利用した。
今回差分全体のfull-lib成功を主張するものではない。

worktree内coreプロセス0を確認し、`scripts/build-dev.ps1 -PreserveRuntime` が成功。
normal featureのdev-runtime core、Remote service、EPUB PDF workerとVCRT確認を完了した。
core更新日時は2026-10-12 05:50:41、サイズ325,166,592 bytes。mIVの起動は行わず、未コミットで引き渡す。
ログ、実probeの失敗観測、隔離data-dirの利用者確認手順は`target/EV-1374-impl-msg.txt`へ記録した。

## 18. 実機確認と検索欄の再表示（2026-10-12）

coordinatorが隔離データで実機確認を実施。未起動・索引準備中・60秒応答timeoutの案内、
取消、キー入力の解放、元一覧の保持は設計どおり。応答するEverythingで `mist` は
184,914件中10,000件取得、9,825件表示、取得から表示まで約3秒（classify 2.16秒）。
結果からfullscreenを開き、Escで一覧へ戻る動作も確認済み。
これは利用者から引き継いだ確認結果であり、実装agentがアプリを起動した記録ではない。

再表示時は既存focus requestだけでcaret位置が残り、入力が前の検索語に追加されていた。
Everything controlsはIME helper経由のTextEditに対し、再表示のfocus request時に文字数で
全選択し、TextEdit stateへ保存する。IME active中の同latchによるfocus recoveryでは
compositionの選択を変更しない。別のpending fieldや選択ownerを追加せず、通常passで再選択しない。
Ctrl+R handler→実controls→次passの文字入力の回帰で `mist` の置換、Unicode文字範囲、
IME確定、通常の続き入力、IPCを送信しないことを確認する。

master `71e6b4c83` のincoming 17ファイルを共通ancestorから衝突なく作業ツリーへ展開した。
共通git管理領域への書込み禁止によりmerge metadataは未作成。coordinator向けの通常mergeへの
引継ぎ手順と検証logは `target/EV-1374-impl-msg.txt` に記録する。
統合後のfmt・通常/portable check・Everything/検索/IME焦点テスト・検索バーsnapshotに成功。
`test-full.ps1` を1回実行し、workspaceとvendor 3 crateがPASS（本体lib 12,214 passed / 0 failed / 57 ignored）。
最後に `build-dev.ps1 -PreserveRuntime` を完了し、normal featureのcore/Remote/EPUB workerを構築。
未コミットで引き渡す。agentによるmIVのUI起動は行っていない。

## 19. Everything側の応答なしと遅い検索の案内分離（2026-10-14）

利用者の操作と設計担当の読み取り専用process観測による追加確認:

- Everything 1.4.1.1032の約2,800万件・約1 GBの索引環境で、起動・スキャン後に
  約6分間Responding=Falseとなり、CPU 1コアを使用し続けた。
- この間のmIV検索はWM_USER（0x400）のABORTIFHUNG付き版照会が約0.03 msで失敗するか、
  queryのWM_COPYDATAが60秒でerror1460となり、従来はいずれもBusyの案内だった。
  読取ログの出どころは`G:\miv-iso\view\data\logs\mimageviewer.log`のeverything行。
  今回の実装agentは実Everythingへの追加probeや当該process/logの追加観測を行っていない。
- Everythingが落ち着いた後は`mist`・`妹`とも検索成功した（利用者の観測）。
  内部の処理内容をIPCで特定したという意味ではない。

利用者決定に従い、送信失敗直後の対象HWNDに対して既存Windows API
[IsHungAppWindow](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-ishungappwindow)
を一度呼び、trueなら型付き`EverythingError::NotResponding`を返す。
Win32 errorはAPI呼出し前に保存し、error0の状態照会即時失敗とerror1460のquery timeoutを
同じ失敗境界で分類する。falseのerror1460は従来どおりBusy、返信待ちdeadlineはTimeoutのまま。
Windowsの応答なし判定を通知するもので、Everything内部の停止理由を断定しない。

送信失敗logへ分類名を加え、query/pathは記録しない。新たなretry・待機・pollingは追加しない。
status失敗時はquery/取消を送らず、query失敗時は既存の一回・100 msのbest-effort取消と
correlation失効・RAII破棄を維持する。検索owner・一覧・旧結果保持・main modalは変更しない。
既存のmodalと有限deadlineで組合せを抑える設計を維持し、新たな回復状態は不要。

fake endpointはerror0/1460とWindows判定の組合せを注入し、分類から案内への伝播、
応答なし・遅いだけ・未起動の文言分離、status失敗の未送信、query失敗の取消一回を検査する。
応答なし案内の狭幅dark検索bar PNGを1枚追加し、生成後に目視確認する。
マニュアルのEverything節へ、多数のファイルがある環境の起動直後／索引更新中の応答停止、
画面の「応答なし」の確認と処理完了後の再検索、頻繁に更新されるフォルダーの除外が
処理負荷を軽くする場合があることを追記する。

### 19.1 検証記録

master `f5bdab499` 上の今回の未コミット差分で、以下はすべてexit 0。

- `cargo fmt --all -- --check`、通常／`--features portable`のcore check、
  `python scripts/check_ui_glyphs.py`（危険文字0）、`git diff --check`が成功。
- `cargo test -p mimageviewer --lib everything`: **68 passed / 0 failed / 1 ignored**
  （実Everything probeは実行しない）。新しい分類／案内のfake回帰を含む。
- `cargo test -p mimageviewer --lib global_search_ui`: **85 passed / 0 failed**。
  新PNGは更新指定で1 passed、その後の上記2 filterでは更新指定なしで比較成功。
  狭幅darkの案内全体と検索／閉じるボタンのviewport内配置を検査し、PNGを目視確認済み。
  追加1枚は38,209 bytes。既存`--test ui_snapshot everything`も**5 passed / 0 failed**。
- `scripts/build-dev.ps1 -PreserveRuntime`が成功。通常featureのdev-runtime core／Remote service／
  EPUB PDF workerをbuild/stageし、VCRT gateはruntime=4 / pe=3で成功。
  アプリは起動していない。コミットしていない。

各検証の記録は`target/EV-1374-hung-{fmt,glyph,check-normal,check-portable,everything-tests,search-tests,ui-tests,snapshot-update}.log`。
build完了と成果物の記録は`target/EV-1374-hung-build-dev.log`（JSON）。
変更はprivate transportの失敗分類と案内に限定し、今回はfull gateを実行していない。
実Everythingが応答しない間の新案内と、応答回復後の再検索は利用者確認待ち。

## 20. §1.386 利用できない理由の表示（2026-10-19、段2）

正本は `next-release-backlog.md` §1.386 と2026-10-19の利用者決定。
段1の条件拡大案は独立設計レビューの条件付き合意を受け、**今回は理由表示だけ**に縮小した。
Ctrl+R の可否条件、Ctrl+G の入口／メニュー／結果採用、detached の所有移管・表示切替は変更しない。
以下のコード参照は HEAD `be318299f` の調査時点（実装で行番号が動く）。

### 20.1 原因と現行条件の保持

利用者観測は「連動動画窓の表示中、メニューが灰色で hover/キーとも理由がない」。
`everything_search_entry_allowed` (`src/global_search_ui/everything.rs:178`) は bool だけを返し、
`ui_main.rs:7577,7658` の両メニューは `add_enabled` だけ、
`everything/shortcut.rs:7` はキー判定より前に拒否していたため、理由が生成されなかった。

都度算出する `EverythingSearchEntryAvailability` を導入し、`reason_text()` が None のときだけ
`is_allowed()` を true にする。App の field や別の理由キャッシュは追加しない。
既存 bool helper はこの enum から導く。元の次の conjunction と可否を一致させる。

修正箇所は可否を作る境界とその表示consumerである。別の理由bool、通知用の保持状態、detachedの
追加guardを設けず、メニュー・起動／送信の可否・拒否通知を同じ判定から導くため、理由と可否の
不一致を構造で防ぐ。BA-7の状態重複とBA-5の入口差を増やさず、凍結中の所有移管経路には触れない。

| 旧成分／境界 | enum と理由 |
| --- | --- |
| `!main_folder_history_available()` | `ViewerContext`: メインウィンドウの一覧から検索してください |
| `fullscreen_idx.is_some()`、linked detached/switching | `LinkedViewer`: 連動した別ウィンドウで閲覧している間は使えません。表示を閉じてから検索してください |
| 同じ idx 判定、main の fullscreen/ウィンドウ内表示 | `MainViewer`: メインウィンドウで画像・動画などを表示している間は使えません。表示を閉じてから検索してください |
| Windows の native main backdrop | `MainViewer`。旧 native 判定も保持する（現状この述語自身が fullscreen idx を要するため、先の idx 判定に包含される） |
| `remote_session_blocks_local_control()` | `RemoteControl`: mIV Remote で操作中のため検索できません |
| その他の `common_modal_dialog_open()` | `OtherOperation`: 別の操作や準備が終わってから検索してください |
| いずれにも該当しない | `Available`、理由なし |

判定は main context → idx → native → Remote → common modal の順。Remote は common modal にも
含まれるため、その前で理由だけを区別する。VST3 manager は `any_dialog_open` にだけ含まれ、
元の Everything 可否には含まれなかったので、今回もメニューの可否を変更しない。
catch-all は準備待ちも含むため、必ず「ダイアログを閉じて」とは案内しない。

`main_folder_history_available` (`app.rs:23682`) は projected registry main と suppression depth=0 を要求する。
main の Back/Forward/facet は bundle 外の App 所有なので、viewer 投影処理から触らせないための条件である。
`with_detached_viewer_main_history_suppressed` (`app.rs:22947`) は実行スコープの depth で、
**連動窓が存在する間ずっと立つフラグではない**。linked 中の拒否は raw `fullscreen_idx` にも由来する。
この履歴述語・全 caller と `viewer_session_blocks_main_window` は変更しない。

Everything の未導入／未起動は入口では判明しない。バー表示では IPC を送らず、送信 worker が
接続／版／DB loaded を確認して既存 `EverythingError::user_message()` を表示する（§9/§19）。
UI 同期 probe、最終エラーを使った入口の無効化、未確認の NotRunning variant は追加しない。

### 20.2 メニューと ROOT の拒否通知

両メニューは同じ `everything_search_menu_button` に enum を渡し、既存
`HoverTipExt::hover_tip_disabled` (`ui_helpers.rs:1076`) で disabled の理由を描く。
既存例は sort lock の `reason.tooltip()` (`ui_main.rs:8351`) と
`OpenAdmissionRefusal::message` → `show_open_admission_refusal` (`app.rs:1490,28423`)。

拒否通知専用の `notify_everything_search_shortcut_refusal` を ROOT の `input_begin_key_frame` に接続する。
この phase は既存 keyboard owner の確定とWin32 native frameの確立後、embedded/native fullscreen の描画 early return より前に通る。
通知入口は検索を開かず、IPC／context 移管も行わない。

- ROOT、検索の main context 資格、ROOT/Main に一致する application shortcut permit を要求する。
  `Modal` / TextInput / IME / pending focus / focus recovery / FocusedUi / 他 viewport では通知も消費もしない。
  新しい dialog が同 pass で開いた場合も `any_dialog_open()` でそのキーを任せる。
- 拒否の非 repeat press だけを既存 `keymap.consume_action_no_repeat(GlobalEverythingSearch)` で処理する。
  実効割当／none／exact modifiers／Win32 viewport ごとの edge 所有をそのまま使う。
  通知はnative frame確立直後の1か所だけで処理し、後段dispatchでは通知しない。
  同じ押下のWin32/egui両キューを既存helperで同時に消費し、repeat は通知を重ねない。
  固定 Ctrl+R、OS current level、Modal 越えの diagnostic peek は使わない。
- 本文に「Everything ファイル名検索: 」を付け、既存の本文長に応じた時間を使って
  `show_feedback_toast_on_with_duration(..., ActionSurface::MainWindow, ...)` へ出す。
  面未指定の helper は native sibling にも送るため使わない。通知 queue/新 latch は追加しない。
- 許可時の dispatch は従来の ROOT/Main permit → consume → `open_everything_search`。
  source 切替、前 query の全選択、IME、明示検索、Ctrl+G toggle は変更しない。

**入力／表示の到達限界:** fullscreen child/native presenter がキーを所有する場合、ROOT への新しい転送は
追加しないので通知しない。embedded fullscreen のように ROOT で受けても `ShortcutSurface::Fullscreen` が
所有するキーは消費せず通知もしない。Grid の Everything 再割当を Ctrl+G 等にすると、拒否通知の先行consumeが
既存fullscreen編集Actionを奪うためである。通知の入力所有は通常の検索起動と同じ Main permit に限定する。
linked viewer 中の main ROOT は Main owner なので、今回の利用者報告の操作面では理由を通知できる。
MainWindow 面の通知は viewer/native overlay へ複製しない。
Remote は `modal_dialog_block_reason` (`app.rs:22728`) に含まれるため、**実際の owner は Modal**となり、
現行の実経路ではキー通知を出さずメニューホバーで Remote 理由を示す。
これは「Modal のキーはその所有者へ任せる」という今回の優先方針による。
fullscreen/Remote の非到達・非視認については、ホバーと manual の利用条件で足りるかを検収時の判断材料にする。

### 20.3 今回やらないこと／後続 §1.406 への申し送り

条件拡大と Ctrl+G の欠陥は [backlog §1.406](next-release-backlog.md) へ分離する。
以下はコード上の欠陥経路・設計上の論点であり、今回の実機再現／修正済みを意味しない。

- **Everything の入口だけを広げられない:** source 切替・send・child return・採用前が
  `finish_everything_edit_boundary` (`everything.rs:199,265,499`, `global_search_ui.rs:3190`) を通り、
  同 helper (`app.rs:22708`) は fullscreen idx があれば viewer を閉じる。
- **Ctrl+G の既存欠陥:** 連動 detached 中も `update_phases.rs:1010` の Ctrl+G gate は通る。
  初回の結果は `adopt_prepared_search_view` → `replace_search_view_items_with_ratings`
  (`global_search_ui.rs:3697,2176`) に入り、`invalidate_idx_state_and_queues`
  (`app.rs:44289`) が `fs_cache.clear()` を行って、main の旧列を読む linked VideoPlayer を Drop し得る。
  main adoption は履歴/facet の採用であり、通常の `start_loading_items` の viewer 移管を代替しない。
- **source 交換と同一結果の再投影を分ける:** source 交換は main を保つ既存 live-park fork
  (`app.rs:65705` → `viewer_context_registry.rs:5577`) を候補にする。
  `split_current_context_preserving_main_grid` の `keep_in_main!(global_search, ...)` (`:4344`) を使えば
  検索要求を複製せず main に残せる。一方 streaming/sort/rating 等の同一結果集合では既存の
  `capture_rating_membership` / `prepare_rating_membership_items_swap` / `finish_rating_membership_viewer_swap`
  (`global_search_ui.rs:3240–3310`) の identity remap を保ち、一律 fork しない。
- **表示切替と採用を直列化する:** live-park snapshot は `viewer_session_is_detached()` を要求するが
  main 操作 gate は detached への切替途中も許可する。detached へ／からの未完了 effect と採用を
  同じ presentation/session owner で順序付ける。raw idx guard、検索の機能制限、時間待ちで回避しない。
- **rating publication の再入を防ぐ:** `fork_mounted_context` と registry の rating publication 境界
  (`viewer_context_registry.rs:5477` 付近、fork_outgoing/fork_incoming) が採用中の状態へ再入し得る。
  prepare/stamp/currentness の確定と fork/publication/install の順序を独立設計で定める。
- **promote をそのまま検索採用へ足さない:** `stash_mounted_and_start_fresh` (`:5697`) は bundle 全体を
  退避し fresh main を作るため、Search producer/prepare/return origin と採用 context がずれる。
  通常 folder/book open と検索 close の既存 promote は維持し、C1 後の reader に二度目の promote を作らない。
- C1 は独立化／promote **後**の受動 watch/rescan/restart を停止する（`detached-snapshot-reader-plan.md` §4/§9）。
  linked 中と最初の移管は残る。既存 `section1375_c1_search_close_promotes_once_and_remount_never_observes`
  (`src/app/section375_c1_tests.rs:117`) は Ctrl+G/R × 動画/音声の close/promote/remount を検査するが、
  新入口・初回採用の安全性までは証明しない。

後続は BA-7（状態／所有）を主に、BA-5（複数入力／描画入口）の構造設計として双方レビューする。
今回の理由表示は detached の所有移管・viewport 経路を変えないため、リワーク計画 §11 の記録対象を増やさない。
まれな失敗は既定の割り切り（ログ + 通知 [+ 既存の単純な復帰入口]）。回復 owner は追加しない。

### 20.4 回帰・文書・検証台帳

- enum: 旧 conjunction との一致、許可、main fullscreen/ウィンドウ内表示、linked、別 context、
  通常 modal、Remote、VST3 manager の従来可否を検査する。
- 入力: 非 Modal 拒否一回／MainWindow 面、同 pass 再 dispatch、repeat、再割当/none、Modal/key-capture、
  TextInput/IME/focus recovery/pending focus、foreign scope、許可時の既存 open を検査する。
  Fullscreen owner の ROOT 入力も保持し、Grid の Ctrl+G 再割当が fullscreen の同キー編集を奪わないことを検査する。
- 両 menu は同じ pure 描画 helper。disabled button を実際に hover して理由本文を検査し、
  Light/Dark の代表 PNG を追加する。実 App の起動はしない。
- manual/search.html の既存の fullscreen/linked 中の利用不可説明を残し、hover による理由確認を追記する。
  新 action／設定／DB schema／Remote API は追加しない。
- 独立完成レビューは別担当。main lib 全件は ClaudeCode が最後に実施する。
  対象テスト、glyph lint、fmt、check、確認用 build の実施結果は以下へ記録する。

以下は初回実装（`3306bf124` としてcommit済み）の検証記録であり、レビュー修正後の結果は§20.5に記録する。
当時の検証対象は HEAD `be318299f` 上の未コミット差分。Rust 差分
（`git diff --binary -- src tests/ui_snapshot.rs`）の SHA-256 は
`3185f5ebca6da9c24be05198e1a471e4c9e9cce62535a50aaf4ecd8341252a9f`。

| 実施 | 結果 |
| --- | --- |
| 通常 core `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0（先行ソース、5m07s）。最終ソースは通常featureの下記buildで確認 |
| 同 `--features portable` | exit 0（最終ソース、13.98s、`target/search6-check-portable-final.log`） |
| `cargo nextest run -p mimageviewer --lib -E 'test(/everything/) \| test(/phase_c_key_tests/) \| test(/global_search_ui/)' --test-threads 4` | **174 passed / 0 failed**（最終ソース、18.461s、`target/search6-tests-owner-final.log`）。先行72件／173件の成功と件数を合算しない |
| `cargo test -p mimageviewer --test ui_snapshot everything`（更新指定なし） | **7 passed / 0 failed**（最終ソース、1.43s、`target/search6-ui-owner-final.log`）。既存5枚と新規2枚の比較成功 |
| 新規 hover PNG | `tests/snapshots/everything_search_disabled_menu_hover_{light,dark}.png`、640×180、19,847 / 19,833 bytes、計39,680 bytes。本文・折返し・Light/Darkを目視確認 |
| `cargo fmt`（引数なし）／`cargo fmt --check`／glyph lint／`git diff --check` | exit 0、危険グリフ0 |
| `scripts/vst3-host-identity.ps1 -ValidateRepo` | 現ソース一致を確認、hostの起動・変更なし |
| `scripts/build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | **exit 0**（2026-10-20）。通常featureのcore／Remote service／EPUB PDF worker、DLL配置、EffeTune bundle、VCRT検査（runtime=4 / pe=3）成功。`target/search6-build-dev-final.log` |

UI検査の最初の480pt fixtureは本文が右へ16ptはみ出して失敗した。その際「640ptが実際の最小幅」と誤認して
fixtureを広げたが、これははみ出しを隠しただけだった。`MIN_INNER_SIZE` はUI倍率で割ってviewportへ渡し、
小さい作業領域では最小寸法自体も縮むため、論理幅640ptは保証されない。§20.5で共通tooltipの幅を修正し、
480pt・200%で320pt・小さい作業領域の200%で240ptを追加検査する。640×180の2枚は代表画像としてのみ保持する。

前提復元: このworktreeで不足していた `vendor/libraw` を既存treeから実体コピーした。
TurboJPEGの並列native buildは終了コード1となり、`MSBUILDDISABLENODEREUSE=1` とworkspace内TEMP、
`CARGO_BUILD_JOBS=1`（必要なCMake段もparallel 1）で成功した。最初のnextestはnative buildで止まり、
テスト本体未実行である。上表はその後の成功結果。まれな製品エラーやテストの失敗とは分類しない。
既存の非推奨／dead code等の警告は残るが、新規テストの旧ID API警告は現行APIへ揃えて解消した。

通常profileの確認用buildは上表のコマンドで完了した。core 19m05s、Remote service 4m48s、EPUB PDF worker 3m24s。
`target/dev-runtime/mimageviewer-core.exe` は334,415,360 bytes、更新日時2026-10-20 00:17:11。
先行buildはFullscreen所有キーの非消費を補うため、自分のbuildプロセスだけを中断した。
これは製品／コンパイル失敗ではなく、上記の最終ソース検証後に同コマンドで作り直した。
portable featureは付けず、製品アプリ・実Everythingへの照会は実行していない。git commit は行わない。
本体lib全件と独立完成レビュー、実機でのlinked窓／native／実IMEの最終確認は未実施。

利用者による確認は、インストール版／トレイ常駐版を終了してから
`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`。
通常の `%APPDATA%\mimageviewer` を使用し、実設定／データを更新し得る（single-instance mutexも共有）。
連動した別ウィンドウを開いてmainへ戻り、両メニューのhoverとCtrl+R一回の理由通知、
操作カスタマイズの「押して入力」等でキーが奪われないこと、通常一覧では従来どおり開けることを確認する。

### 20.5 独立コードレビューのP2対応（2026-10-20）

独立レビューの2件をコードで確認して修正する。可否enum・文言・入力所有gateは維持する。

- **入力frameの境界:** `input_update_begin` はWin32の `begin_frame_for_context_with_tracking` より前だった。
  未取込pendingはkeymapの両キュー消費から漏れるため、eguiで通知した後に同じ押下を再取込できた。
  前frameの未消費edgeを次frameのMain所有と読み替える経路もあった。
  既存のnative begin位置を `input_begin_key_frame` として抽出し、その直後だけで通知する。
  後段の実行dispatchから通知を外す。frame確立そのものの位置、入力所有、検索の受付条件は動かさず、
  新しいlatch／guard／queueを増やさない。実phase順の回帰でegui+Win32 pendingの同時押下と
  TextInputからMainへ移った時の旧edge失効を確認する。
- **tooltipの表示可能幅:** 共通 `show_offset_tooltip_with_observer` がviewportを無視して
  `tooltip_width` を再設定していた。popup枠の寸法を差し引いた現在の `content_rect().width()` と
  tooltip幅の小さい方で折り返す。UI倍率・小さい作業領域・表示後resizeにも同じ計算を使う。
  enabled／disabled／暗色固定のhoverは同じhelperを通るため、既存のtooltip回帰とUI snapshot全件を確認する。
  文言を省略せず、既存の遅延変換・AccessKit名・配色・pointer回避を維持する。
- main grid限定、Modal非消費、Fullscreen所有入力の非消費、Ctrl+Gおよびdetached経路は変更しない。

検証対象は `3306bf124` 上の未コミット差分。初回実装の結果と今回の件数は合算しない。
Rust差分（`git diff --binary -- src tests/ui_snapshot.rs`）のSHA-256:
`2681f842f477ac1cad15cd3f06624832a3675e819ff1129251d4771de18bc2ab`。

| 実施 | 結果 |
| --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(/everything/) \| test(/phase_c_key_tests/) \| test(/global_search_ui/) \| test(/ui_helpers::accessibility_names_tests/)' --test-threads 4` | **182 passed / 0 failed**、21.275s。`target/search6-review-tests.log` |
| `cargo test -p mimageviewer --test ui_snapshot -- --test-threads 4` | **189 passed / 0 failed**、94.95s。`target/search6-review-ui.log` |
| UI差分 | 既存PNG全件の比較成功。代表のhover PNG2枚も変更不要。新規PNGなし |
| 通常 core `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0、34.72s。`target/search6-review-check.log` |
| 同 `--features portable` | exit 0、53.48s。`target/search6-review-check-portable.log` |
| `cargo fmt`（引数なし）／`cargo fmt --check`／glyph lint／`git diff --check` | exit 0、危険グリフ0 |

最初のlib test compileは追加fixtureの `RawInput.modifiers` がegui 0.37に存在せず失敗した。
既存と同じ `Event::ModifiersChanged` に直して再実行し、上表の成功を確認した。
実行環境はworkspace内TEMP/TMP/MSBUILDDEBUGPATHと `MSBUILDDISABLENODEREUSE=1` を使用。
tests/checkは `CARGO_BUILD_JOBS=1`、native依存が構築済みの確認用buildは4。
`scripts/build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` もexit 0。
core 5m47s、Remote service 0.54s、EPUB PDF worker 0.47s、VCRT検査runtime=4 / pe=3で成功した。
ログは `target/search6-review-build-dev.log`。通常featureの `target/dev-runtime/mimageviewer-core.exe` は
334,356,480 bytes、更新日時2026-10-20 00:59:04。製品の起動はしていない。
利用者確認は前節の通常profile起動手順に従い、連動窓中のCtrl+R一回の通知、ダイアログの非消費、
UI倍率200%／狭い窓でのホバー理由全文を確認する。
製品アプリは起動せず、commitは行わない。本体lib全件・独立再レビュー・実機での確認は別途。

### 20.6 ホバーsnapshotのOSカーソル依存を除去（2026-10-20）

`389371880` 時点の代表2枚がsearch6単独／master統合後とも縦1～2px揺れる利用者報告。
コード確認でも、fixtureが製品の `anchor_for` → `cursor_anchor_rect` → `GetCursorInfo` / `GetIconInfo` を
通り、headlessの擬似ポインタとは別にデスクトップの現在のカーソル画像を計測していた。

公開fixtureへ上下の物理px寸法を引数で渡し、2 PNGと倍率geometryは `(0, 34)` で固定する。
メニューbutton／可否／理由と、tooltipの遅延変換・AccessKit名・幅制限・配色・gapは製品と共有する。
寸法からアンカーへの変換をpure helperに分け、製品は従来のOS計測値（取得不可時は既存の論理34pt fallback）、
fixtureは明示寸法を渡す。製品のOS計測gateや実カーソル回避を変更せず、テスト判定用の環境変数・状態を追加しない。

他のhover撮影も確認した。Ctrl+G拒否バッジ、EffeTune、動画タグはegui標準 `on_hover_text` /
`on_disabled_hover_text` でOS計測へ入らない。詳細アイコンはhover色だけ、整理先／見開き調整／toolbar等は
撮影前にpointerを外している。libのstill seek hover撮影は独自previewで、lock button外を指す。
確認範囲で同じ実カーソル依存の修正対象はEverythingの2枚のみ。

物理寸法の100%／200%変換と、OS計測不可時の既存fallbackをlib回帰で検査する。
検証対象は `389371880` 上の未コミット差分。Rust差分
（`git diff --binary -- src tests/ui_snapshot.rs`）のSHA-256は
`40b5ab7c5b01731d8ad9da7443bba9d6ba13ccab9b2edb431e8ea72b87bc93d7`。

| 実施 | 結果 |
| --- | --- |
| `UPDATE_SNAPSHOTS=1` でEverything hover明暗2件をnextest実行 | 2 passed。`target/search6-cursor-update.log` |
| PNG目視／比較 | 明暗とも640×180、理由全文・disabled状態・枠・位置を目視確認。固定寸法で再生成した2枚はHEADとbyte単位で一致（19,847 / 19,833 bytes）、PNG差分なし |
| `cargo nextest run -p mimageviewer --test ui_snapshot --test-threads 4`（更新指定なし） | **191 passed / 0 failed / 0 skipped**、64.816s。`target/search6-cursor-ui.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/ui_helpers::accessibility_names_tests/) \| test(/everything/)' --test-threads 4` | **82 passed / 0 failed**、7.440s。`target/search6-cursor-lib.log` |
| 全体 `cargo fmt`／`cargo fmt --check`／glyph lint／`git diff --check` | 成功、危険グリフ0 |

workspace内TEMP/TMP/MSBUILDDEBUGPATH、`MSBUILDDISABLENODEREUSE=1`、`CARGO_BUILD_JOBS=4`を使用。
fixtureの再現性だけの変更なので確認用製品buildは行わず、製品起動・commitも行わない。

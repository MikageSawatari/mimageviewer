# §1.220 切り取り中の実項目表示 — 設計・検証記録

2026-09-13利用者確認: 18:36の追補buildを案内した後、「切り取り表示は良さそうです」との確認を受けた。開発側の受入・commit待ちを解消する。利用者による個別シナリオごとの結果・hash照合は未取得であり、下記の実機未確認記述は確認前の履歴として保持する。製品sourceと自動検証は追補freezeから不変。本確認追記により本書自身のhashのみfreeze時点から変わる。

## 状態

- 2026-09-13: 現行の Shell clipboard 経路、一覧描画、Windows の通知・転送完了契約を調査し、ownership 案を独立レビューして固定した。
- 2026-09-13: App-global observer、message-only listener / OLE reader、private cut identity、typed reducer、サムネイル / 詳細一覧の painter 分離を実装した。同期 startup handshake は置かず、UI を待たない generation 付き `Starting` owner へ統合した。初回verification buildの利用者確認ではmIV Cutは反映したがExplorer Cutは反映しなかった。通常logでExplorer `IDataObject::QueryGetData` の未対応formatが `DV_E_CLIPFORMAT (0x8004006A)` を返し、readerが一時失敗と誤分類してPreferred MOVE / HDROPへ進めず再試行していた根因を確定した。`S_FALSE`を返す実Shell objectも含めて未対応formatへ分類し、production readerのE2Eを追加した。中央の暗い円＋白いvectorハサミも追加し、動画では中央play表示をハサミへ置き換えた。追補後の実 Windows clipboard / Explorer再確認は未実施。

## 利用者向け仕様

Windows clipboard の現在内容が `CF_HDROP` と `Preferred DropEffect = MOVE` を持つ間、そこに含まれる実ファイル・実フォルダと同じ一覧項目の内容を半透明にし、中央へ暗い円背景と白いハサミを通常alphaで表示する。動画の中央play表示は切り取り中だけハサミへ置き換える。対象はサムネイル一覧と詳細一覧だけで、選択背景、選択枠、チェック、見開きカーソル、hover と pointer 入力は通常の濃さ・挙動を保つ。フルスクリーン画像、ファイル内容、DB、ZIP/PDF 内ページやその他の仮想項目は変えない。

mIV からの切り取りは成功直後に反映し、コピー、別の切り取り、外部アプリによる clipboard 上書き、確定した貼り付け完了で現在 snapshot を置き換える。貼り付けの開始、失敗、中止だけでは解除しない。

## 現行入口と確認した境界

- `GridCopyFiles` / `GridCutFiles` と右クリックの mIV 項目は `App::invoke_shell_clipboard_verb_for_paths` に合流し、`GridItem::drag_source_path` が実ファイル・実フォルダの集合を決める。ZIP/PDF 内ページ、`ZipDir`、`Stack`、`SearchContainer` はこの入口で実パスを返さない。
- native / egui の右クリック renderer は同じ `MenuCommand` dispatcher を使う。Windows Shell の動的メニューが直接行う copy/cut と、他アプリが行う変更は mIV の dispatcher を通らないため、system clipboard 通知が正本になる。
- mIV の paste はフォルダ背景 Shell verb の `InvokeCommand` を呼ぶ。`Ok` はコマンド受付であり、ファイル移動の完了ではないため、この戻り値で半透明表示を解除できない。
- `ClipboardDataObject::SetData` は inner Shell data object へ転送している。この HRESULT と `STGMEDIUM` の ownership は既存の実ファイル cut/paste 契約なので変更しない。
- `draw_cell` はセル背景の後に内容を描き、最後に枠、見開きカーソル、チェック、badge を描く。`draw_details_row` は行背景・separator・カーソルの後に preview icon と文字を描く。後者は下部選択情報表示にも共有されるため、一覧 caller だけが半透明値を渡す。

Windows の根拠は [Using the Clipboard](https://learn.microsoft.com/en-us/windows/win32/dataxchg/using-the-clipboard)、[GetClipboardSequenceNumber](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getclipboardsequencenumber)、[OpenClipboard](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-openclipboard)、[Shell Clipboard Formats](https://learn.microsoft.com/en-us/windows/win32/shell/clipboard)、[Handling Shell Data Transfer Scenarios](https://learn.microsoft.com/en-us/windows/win32/shell/datascenarios) とする。`WM_CLIPBOARDUPDATE` が変更通知、sequence number が同一内容を確認する補助証拠であり、sequence number の定期 polling は通知の代用にしない。sequence は wrap する `u32` なので大小比較せず、read 前後と既知の local publish との等値比較だけに使う。遅延 rendering 中は read の前後で sequence が変わり得る。

Shell の完了通知は段階を区別する。`PerformedDropEffect = MOVE` は未最適化 move の source delete 前にも届くため、それだけでは解除しない。`PasteSucceeded = MOVE` または `LogicalPerformedDropEffect = MOVE` は完了、`PerformedDropEffect = NONE` は optimized move で original data が既に削除された完了として解除する。COPY、無効値、通知なし、paste の受付だけでは解除しない。

## 所有構造

### process 共通 snapshot

新しい `cut_clipboard` module に `CutClipboardObserver` を置き、App が process 単位で 1 つ所有する。viewer bundle へは入れない。snapshot は正規化済み path の不変集合で、可視セルは `drag_source_path` 1 件を HashSet 照合するだけにする。folder 全体の走査、`canonicalize`、`stat`、同期 clipboard I/O は行わない。items の再読込後も新しい item から同じ snapshot を照合するため、item ごとの cut flag は持たない。

path key は Windows の大文字・小文字、`/` と `\\`、`\\?\\C:\\...` と通常 drive path、`\\?\\UNC\\...` と通常 UNC、root 以外の末尾 separator を字句的に統一する。filesystem の実在や symlink 解決はこの表示判断に含めない。

### 通知と読取 worker

production の明示 startup だけが 2 つの専用 thread を開始する。通常の `App::default` と headless test は inert backend のままで HWND、listener、実 clipboard を作らない。

- listener thread は message-only window を所有し、`AddClipboardFormatListener` を登録する。main / detached HWND の再生成や lifetime へ listener を結び付けない。WndProc は `WM_CLIPBOARDUPDATE` ごとに process 内の単調な request serial と `GetClipboardSequenceNumber` を stamp し、mutex 内の latest-request slot を必ず置換してから bounded wake channel を `try_send` する。wake が既に満杯でも最新request自体は失われない。clipboard の open、COM object の取得、`CF_HDROP` 展開は WndProc/UI thread で行わない。
- reader thread は自分の apartment で OLE を初期化し、`OleGetClipboard` から得た `IDataObject` をその thread 内だけで読み捨てる。COM pointer は apartment を越えて渡さない。path の字句正規化と `HashSet` 構築もreader内で完了し、UIへは不変`Arc`を渡す。read 前後の sequence が同じ場合だけ `Cut(owner, paths)` / `NotCut` / `Terminal(owner)` の typed observation を返す。別 sequence なら途中結果を破棄し、新しい request serial で再読取する。`QueryGetData` はexact `S_OK`だけを利用可能、`S_FALSE`、`DV_E_CLIPFORMAT`、既知の `DV_E_FORMATETC` / `DV_E_TYMED` / `DV_E_DVASPECT` / `DV_E_LINDEX`を形式なしと扱う。call rejected / retry laterを含むその他の失敗はstableな`NotCut`へ落とさずPending retryを続ける。Explorer型の実Shell `IDataObject`を使うheadless E2Eで、private/completion formatなしからPreferred MOVE + CF_HDROPの2pathまで進むことを固定する。

異なる sequence の通知を受けた時点で旧 snapshot は無効なので、UI owner は paths を空にした `Pending` へ移る。同じ local publish sequence の重複通知だけは既知の local snapshot を保持できる。clipboard 競合中は旧snapshotへ戻さず、reader が 25/50/100/250 ms などの上限付き backoff で同じ request を、成功、新通知、shutdown のいずれかまで再試行する。これは Pending 中だけの worker-owned retry で、成功後の定期 polling、UI frame の OS polling、`try_lock + sleep` は行わない。新通知は latest-request slot へ coalesce し、sequence の大小ではなく request serial で旧結果を拒否する。wakeが連続通知数より少なくても、readerは起床ごとにslotを取り直すため最後の変更へ必ず収束する。

production install は thread を生成した時点で UI を待たず `Starting` を返す。reader の OLE / format 初期化と listener の message-only HWND / `AddClipboardFormatListener` 登録は、それぞれ backend generation 付きの `Ready` / `Failed` event で報告する。両方の同世代 `Ready` を App が受けた時だけ `Running` へ移り、listener / reader が起動中に先行して発行した変更・読取 event は順序を保って保留し、その後に適用する。両 component が ready になった境界で initial request を 1 回発行し、起動前から存在する cut も非同期で復元する。起動中の local Copy / Cut は従来の Shell 操作を行うが、private wrapper と即時暗転はまだ付けない。いずれかの初期化失敗時は typed `Disabled` へ移って表示snapshotを空にし、local Cut を追跡済みと見せない。旧世代の遅い `Ready` / `Failed` は現在の backend を変更しない。listener、reader、SetData completionはいずれもtyped eventを公開するとroot repaintを明示要求する。Appはupdateの先頭、fullscreen/nativeのearly returnより前にeventをdrainする。将来のnatural frameを待たない。detached を含む複数 viewer は同じ App snapshot を読む。

App の既存 exit 境界と observer owner の `Drop` は独立した shutdown signal を listener / reader の双方へ送る。`Starting` の終了は初期化中の thread を待たず、stop / wake / shutdown message を送って、終了済みの handle だけ join する。message-only HWND threadはlistener登録解除・window破棄まで制御できるので、`Running` の終了では shutdown message を正常に投入できた時または既に終了した時だけjoinする。message投入に失敗したthreadや、初期化または外部の`IDataObject` / `OleGetClipboard`呼出し中のreaderは無条件joinしない。stopでretry開始を止め、handleをdetachしてprocess終了時までの限定所有としてlogする。外部へ渡った clipboard wrapper の sender はこの shutdown channel を保持しないため、data object が App より長寿命でも終了を妨げない。通常 frame は join しない。

### mIV の書込みと paste 完了

mIV の Copy/Cut は呼出前に単調な local token と現在の notification request floor を reserve する。`OleSetClipboard` 成功時だけ同じ token を commit し、Cut は既知 path snapshot、Copy は空 snapshot をその UI frame で反映する。失敗時は現在の system clipboard と表示を変えない。成功後に取得した OS sequence は local publish との等値確認にだけ用い、publish 中または直後に届いた通知を一律に捨てない。

Cut の外側 `ClipboardDataObject` には App 参照を持たせず、token と非ブロッキング completion sender だけを渡す。さらに process 起動時 nonce と token を持つ private clipboard format を read-only で公開し、`QueryGetData` / `GetData` / `EnumFormatEtc` から同じ data object identity を reader が取得できるようにする。この private format は callback の `SetData` ownerや外部pathへならず、raw COM pointer比較の代わりに同一process・同一cutを証明する。

任意 thread から来る `SetData` は `FORMATETC` と medium の DWORD を read-only に検証・観測した後、従来どおり inner object へ 1 回だけ転送して同じ HRESULT と release ownership を返す。inner Shell object が従来どおり `E_NOTIMPL` を返しても、正当な completion callback 自体は観測eventとしてobserver sinkへpublishし、root repaintを要求する。matching token について `PerformedDropEffect = MOVE` は `AwaitingPasteSucceeded` として暗転を保持し、`PerformedDropEffect = NONE`、`PasteSucceeded = MOVE`、`LogicalPerformedDropEffect = MOVE` は `Terminal` として解除する。COPY、無効値、callbackなし、folder paste の `InvokeCommand = Ok` は解除根拠にしない。clipboard readerでも完了formatを Preferred MOVE + HDROP より優先する。private owner format の登録は観測専用であり、登録不能時も既存の Shell Cut / `OleSetClipboard` を失敗させない。

UI owner は commit 済み local Cut のうち完了した exact identity だけを tombstone として保持し、その同じobjectを伴う遅いcallbackと再読取結果を拒否する。未発行token、nonceだけ一致する偽token、別の古いclipboard履歴objectは終端済みにしない。これによりTerminal後に同じdata objectを再読取しても Preferred MOVE + HDROPから暗転が復活せず、別tokenの履歴を範囲判定で誤って抑止しない。外部clipboardはprivate identityを持たないため、別sequence通知で即Pendingになり、古いlocal callbackから保護される。

notification observation は notification request floor と stable OS sequence の等値の双方で対象を確認する。local publish より前に発生した結果を後から適用せず、publish 後に発生した system 通知は同じ mIV cut の遅延renderingでも外部上書きでも読取り、private identityで区別する。read 中に別内容へ変わった場合は前述の再読取へ回す。

## 描画境界

- サムネイル: 通常 alpha のセル背景を描いた後、clone した content painter だけへ opacity を掛け、画像・placeholder・実項目の名前・rating・編集/tag/format/filter/play/count等の内容badgeを描く。その後の選択枠、見開きカーソル、チェックとpointer feedbackだけは通常 alpha で描く。Response、hover、cursor、hit rect は変更しない。
- 詳細: 通常 alpha の行背景、checked accent、separator、見開きカーソルを先に描き、preview icon と全列文字だけを content painter で半透明にする。`DetailsColumnSet::Details` の一覧 caller だけが snapshot 由来 opacity を渡し、下部選択情報の `display_only` caller は 1.0 のままにする。
- opacity は 0.5 に固定し、ライト/ダーク双方の production painter snapshot で「切り取り中」と判別できる中央ハサミ、動画playとの置換、操作状態の通常alphaを確認する。

## 実装範囲

- `src/cut_clipboard.rs`（新規）
- `src/lib.rs`
- `src/app.rs`
- `src/native_context_menu.rs`
- `src/ui_main.rs`
- `src/app/grid_paint.rs`
- `tests/ui_snapshot.rs` と必要な golden
- 本書、`docs/architecture-overview.md`、`docs/async-architecture.md`、`docs/display-pipeline.md`、`docs/spec.md`、該当 manual

## 回帰条件

1. path 正規化が case、separator、drive/UNC の verbatim prefix、末尾 separatorを吸収し、異なる drive/share/path は混同しない。
2. `drag_source_path` を持つ単一/複数/フォルダ混在だけが一致し、ZIP/PDF 内ページ、`ZipDir`、`Stack`、`SearchContainer` は一致しない。
3. local Cut 成功で即時表示、Copy 成功と新Cutで全置換、書込失敗は既存snapshot維持。古い通知、旧read、旧SetData callbackは新tokenを解除しない。外部上書きはprivate local identityと混同しない。未発行future tokenは終端にできず、完了したtokenは別の古いclipboard履歴objectを抑止しない。
4. stable readだけを採用し、read中sequence変更、clipboard競合、連続通知は最新Pendingへ収束する。別sequence通知では旧表示を即解除し、同local sequenceの重複だけは保持できる。sequenceは大小比較しない。wake channel満杯時もlatest slotが最後の通知を失わない。形式なしを示す既知HRESULTと、一時的または未知のQuery失敗を区別する。
5. matching local tokenの `Performed=MOVE` は暗転を保持し、`Performed=NONE`、`PasteSucceeded=MOVE`、`Logical=MOVE` だけで解除する。COPY、InvokeCommand受付、paste失敗・中止では解除しない。Terminal後の同object再読取でも再暗転しない。
6. inert / injected typed eventで外部cut/copy/overwrite、複数windowで共有するApp owner、folder再読込後のpath照合を確認する。startupはreader/listener両方の成功を要求し、失敗したobserverはDisabledでlocal commitも表示しない。local publish前のqueued通知、publish直後の外部overwrite、Terminal後の同object通知、wake満杯中のlatest通知を競合回帰に含める。headless testは実clipboardを開かずmessage-only HWNDも作らない。
7. production painterのshapeでcontentだけalphaが下がり、中央vectorハサミ、背景、選択枠、チェック、hover/cursor ownerが通常alphaであることを確認する。切り取り動画ではplay iconが重ならない。サムネイル/詳細のlight/dark snapshotを目視する。
8. 既存Copy/Cutキー、native/egui右クリック、Windows Shell動的menu、folder paste、実ファイルデータ、DB、fullscreen本体の挙動を変えない。

## 検証結果（2026-09-13）

- 初回freezeではreducer / Windows format / startup barrier 19 件、Shell data object 2 件、Shell clipboard、thumbnail content painter、details content painter、light / dark snapshotが成功した。その後のExplorer再現修正では `DV_E_CLIPFORMAT` / `S_FALSE`分類と実Shell `IDataObject` E2E、中央vectorハサミ、動画play置換を含むcut focused 45/45とlight / dark snapshot 2/2が成功した。headless testは実clipboard / listener HWNDを作っていない。
- 右クリック構成§1.221追補と同じ最終freezeで、context-menu focused 100/100、操作カスタマイズ共有 8/8も成功した。
- `cargo check -p mimageviewer --bin mimageviewer-core`、`cargo fmt --all -- --check`、`python scripts/check_ui_glyphs.py`、`cargo run --locked -p viewer_context_audit --quiet`、`git diff --check` は成功した。
- `RUST_TEST_THREADS=1 .\\scripts\\test-full.ps1 -SuppressCrashDialogs` は main 8378 passed / 0 failed / 45 ignored、UI snapshot 50/50、workspace / integration / doc、vendor egui 25、egui-wgpu 9、eframe 15を含めexit 0だった。完全ログは `target/section220-221-followup-20260913/test-full.{stdout,stderr}.log` と `test-full.exit.txt`。SHA-256は順に `FEB4DE1C1572BA6145B6D314E27728AFC2D78D693492E84F1196F961B409141F`、`B8921BA3AAE71331FFA319A04F60CF51D940A053EA879FFC8C233104D0618AD2`、`13BF7B3039C63BF5A50491FA3CFD8EB4E699D1BA1436315AEF9CBE5711530354`。
- 同じsource freezeの `scripts/build-dev.ps1 -PreserveRuntime` はexit 0。coreは2026-09-13 18:36:01 JST、SHA-256 `6910EAE6ACB511D9C1BF4C77ABA673C087B9624F27CBEC99E9F978375E7A5002`。remoteは既存runtimeを保持し、SHA-256 `A03CE402A7137613E063867C9F8338BEDC5C338FE8897325007BB35110935C64`。エージェントは通常アプリを起動・停止していない。
- 独立レビューは Shell completion / exact identity、latest notification / retry、非同期 startup / shutdown、App-global snapshot、content / interaction painter 分離、Explorer形式のE2Eを検収し blocking 0。残る確認は実 Windows clipboard / Explorer での Cut、paste 成功・中止・失敗、外部上書きの手動操作だけである。

### source freeze SHA-256

| path | SHA-256 |
|---|---|
| `src/cut_clipboard.rs` | `90DB89A43463B2913C8459A7B191FC8ECFD98A7FEFDBAF6250AEDDB7B1885839` |
| `src/lib.rs` | `E1281D9D2305A4C2DEA1564D700DD922C79F823B29A3FD79D9D75A7EE6A6BA89` |
| `src/app.rs` | `97849A566ED852EAF82C5778DE89CBB14220D6D584F89BEA0760A9FB440A3417` |
| `src/app/runtime_ops.rs` | `F4215D26E30384FA9083A2D3EF34614401BBD994F4C92DFCA39C4378B0F84F38` |
| `src/native_context_menu.rs` | `9299C82A665D7DB7261D942E6D2213D6AAF0E2983C12A567A51F6FB5F5FA7C9F` |
| `src/ui_main.rs` | `A65F3EA432C9436D62036DD112F9B03C0FD8F742C41F81F6A4B096167A238198` |
| `src/app/grid_paint.rs` | `57CFB2BD2C12094EDF7F2AE58081CBC875CB1BD0434F8DFE9B61A0FAEDC3FBB4` |
| `tests/ui_snapshot.rs` | `D794157BAD0799745E92016FEBBC41869261081C470DF6201588B7A3611A94A5` |
| `tests/snapshots/cut_item_appearance_dark.png` | `05F5D4328B18FB4D49C0CC18C3910D1C9B0425C93DCABFB185045066FD34BCA2` |
| `tests/snapshots/cut_item_appearance_light.png` | `08FE41F717648234A088E1476B79A12C9D5A49A946DD07C7E94C1C59B6C2D0E9` |
| `docs/architecture-overview.md` | `6A50101CD55702CB5EEA1409B79D41ABADDAE13720987C2D3FB5A1A10B6FBE86` |
| `docs/async-architecture.md` | `02AC19D8EDBC1B04F74FDDDA6A8B40F0A5D228408AC45B189D992EB62796E81F` |
| `docs/display-pipeline.md` | `CFABD18CA75E29545EC4AADE06DA934E2393EF45AD6ACFA6CB7F45F0DF1C9993` |
| `docs/spec.md` | `06F6BBDF68061B8C970A8067F5AFC7B9B5C9C84B4196ACC5EE15469B7F95E2D1` |
| `htdocs/mimageviewer/manual/grid.html` | `C9478F079B5152F60ADC0906CE41BB2FF5CD00C918A8767C64A3CDB974D6D88B` |

本書自身の最終hashを含む所有file一覧は `target/section220-221-followup-20260913/owned-files.sha256.txt` に保存する。

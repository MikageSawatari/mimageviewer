# 第2回レビューの残件・追加指摘

最終対象: `cf4a3ca502000bb65e5d0af5c85cd0ba305e01cb`。実コードの最終変更は `8632af6a9`。
前回の F 番号は対応関係を示す。今回の R 番号は修正担当が個別に追跡するための番号。
P1 の誤 context 配送は修正されているが、「15件すべて解消」にはなっていない。
以下は P2 が14件、P3 が1件。静的確認は実機で再現したという意味ではない。

## R01 [P2 / F12 残件・修正で別条件が悪化] 奇数・偶数の長さを混ぜると連結の隙間と重なりが交互に出る

- 場所: `src/ui_fullscreen.rs:4370` の unit 中心の丸めと `4385` の相対原点の丸め、`8229` の `vertical_reading_offsets`。
- offsets は前後の可視長の平均を加えるため、1000px と1001pxの間は1000.5pxになる。コメントにある「段ごとの間隔は物理ピクセルの整数」は成立しない。中心と相対原点を独立に丸めると、この半画素が二度処理され、共有する辺が分離する。
- 再現: 幅501px、高さ1000/1001pxを交互に並べ、等倍・縦連結・gap0。間隔は `[1,-1,1,-1]` px。gap1なら `[2,0,2,0]` px、gap20なら `[21,19,21,19]` px。負値は1pxの重なり。DPI100/125/150/200%、trim有無で確認。
- 証拠: `geometry_probe.py` / `geometry-probe.log`。現ソースの page/unit型、drawn_band、span、rect配置、offset、丸め関数を抽出して実行。2,880境界中464に0.01px超の差。同寸法controlは改善している。ネイティブUI/GPUの実行ではない。
- 同じfixtureを基点`512b49d4d`から抽出した関数でも実行 (`geometry-probe-baseline.log`)。DPI100%・等倍・原点0・1000/1001px混在のgapは修正前 `[0,0,0,0]` → 修正後 `[1,-1,1,-1]`。同寸法1001pxの旧sign tieは修正前に1pxの隙間、修正後0px。改善した条件と悪化した条件を分離して確認した。
- 修正境界: unit間の共有辺を一度決め、そこから整数の描画長とgapを積む。共通中心を丸めるだけでは奇偶混在を扱えない。元の sign tie 対策は保ち、縦/横、単枚/見開き、異寸法、trim、回転を含む配置テストへ広げる。

## R02 [P2 / F14 修正による退行] 再走査中の次の変更通知を消費して捨てる

- 場所: `src/app.rs:17948–17955` (`start_external_rescan`)。
- 同じ folder の pending があれば何も記録せずreturnする。一方 `poll_current_folder_watch` (`17706`) は通知をdrainし、debounce期限も消費済み。完了時にdirty/rerunを確認する処理がない。
- 条件: workerが画像Aのstampを読んだ後、走査終了前にAをもう一度上書きし、次のdebounceが発火する。2回目は捨てられ、1回目の古い走査結果で完了する。最後の上書きについて追加通知が来る保証はない。同名上書きでdirectory mtimeが変わらなければ、復帰時のmtime gateでも回復しない。
- 結果: 外部編集後の画像が古いままになる。UI外へ走査を移す方向は適切だが、通知を「現在処理中だから不要」と扱うのは元の上書き検出問題を残す。
- 回帰条件: controlled workerをstamp取得後に止め、2回目の通知を届けてから解放。最後の変更を含む再走査/適用まで進むこと。所有contextが一時的に非投影になった場合も、通知を失わない要求の所有者が必要。

## R03 [P2 / F14 修正による退行] 古い再走査の完了が、新しい一覧や削除処理へ適用される

- 場所: `src/app.rs:17987` の `ExternalRescanPending` 作成、`18019–18056` の完了検査。
- requestが保持するのはowner/folder/mtime/cancel/rxだけで、`items_generation` や走査設定の版を持たない。適用時もprojected ownerとfolder pathしか照合しない。報告にあった「世代」はこの走査要求に入っていない。
- 条件A: Aの再走査中にA→B→Aと移動する、または同じAを新しい「隠しファイル」設定で再読込する。その後、古いscanが同じowner/folderへ返ると新しい一覧を上書きする。
- 条件B: 走査を開始してから削除を開始する。削除中を除外する判定は要求開始側 (`17910`) にしかなく、完了側は`delete_pending`を見ずに`load_folder_with_scan`へ進む。削除結果の世代を壊すことを防いでいた既存の制約が、非同期化で外れる。
- 修正境界: context/一覧世代/走査条件を固定した要求と、現在のmutation状態を照合する完了処理。古い結果を捨てる場合もR02の最新変更要求を失わないこと。単にfolder一致条件を増やすだけではABAを解決できない。

## R04 [P2 / F04 修正による退行] 一括書き出しの対象が準備中に別ファイルへ変わる

- 場所: `src/ui_dialogs/export_batch.rs:14–17` (`ExportBatchTarget::Item(usize)`)、`145–146`、`src/ui_fullscreen.rs:35089` / `35102`。
- 複数フレームに分けたpendingに一覧indexだけを保存し、各フレームで現在の`self.items[idx]`とeffective paramsを読み直す。page key/source/context/一覧世代が固定されていない。
- 条件: A/Bを選んで一括書き出しを開き、準備が複数フレームにまたがる間に外部監視の再読込で先頭へ別ファイルが入る。残りのindexは移動後の別ファイルを指す。監視の再読込はexport dialog表示中も除外されない。
- 結果: 選んだBが書き出されずAが二度含まれる、または未選択ファイルが出力される。以前の同一イベント内の全件snapshotには無かった時間境界。
- 修正境界: 選択確定時に安定したsource/page keyと所有context、未保存編集overrideを固定し、その要求から準備する。現在のindexから対象を再解釈しない。pendingを残して一覧を並べ替えるhandler-levelテストが必要。

## R05 [P2 / F03 接続後の新規問題] AIの実行条件が実体化cacheの識別子に入っていない

- 場所: `src/materializer.rs:729–741` / `1364` (`edit_fingerprint`)、`543–547` (hit時の早期return)。
- 新しい`BookAiMaterials`はfeature mode、upscale/denoise寸法制限、透過背景色を持ち、実際のAI出力に使う。しかしfingerprintはparams/編集/stage等だけで、これらの材料を含まない。
- 条件: 透過画像へAI拡大を指定し、補正済み一時ファイルで外部起動を成功させる。元画像/編集/outputファイルを変えずにAI用背景を黒→白にして再実行しても、同じkey/stampがhitし前の黒背景のファイルを渡す。AIの有効範囲を変える場合も同型。
- `begin_generation`はworkerの旧要求を失効させるだけでcacheを消さない。cacheへの登録は起動成功後の`transfer_to_process_directory` (`1314`)、再利用検査はsource/output stamp (`1151`)。したがってRenderedの再利用不可とは別問題で、実ファイル/ZIP/PDFの編集済み出力に該当する。
- 修正境界: 実際に描くAI policy/materialsのsnapshotを出力identityの一部にする。設定値を変えた後に二度materializeし、cache hit条件と出力画素の両方を検査する。

## R06 [P2 / F03 接続後の新規問題] AIで拡大したスタック内ページの注釈が元の位置・大きさに残る

- 場所: `src/external_tool.rs:2083`、`src/materializer.rs:819`、`src/ui_fullscreen.rs:34818` / `35160`、`src/books.rs:1375`。
- indexなしのstack memberは`comic_source_dims=None`。AIを接続したため画像の大きさは変わるが、`bake_comic_annotations` (`1445`) は元寸法がSomeのときだけsceneを拡縮する。外部ツールと一括exportの両経路に存在。
- 証拠: `annotation_probe.rs` / `annotation_probe.log`。公開productionの`books::write_composited_page`を実行。16×16画像、中心(8,8)の赤い注釈、決定的な2倍AI runnerで32×32にした。Noneでは赤領域が(6,6)–(10,9)、期待する(16,16)は黒。Some([16,16])では赤領域(12,12)–(20,19)、(16,16)が赤になる。
- このprobeはAI推論の品質検証ではなく、AI結果を受け取った後の実compositorの座標検証。model/GPU/通常profileは使っていない。
- 修正境界: decode直後・AI前のsource寸法を合成所有者が保持する。indexの有無で注釈座標の意味を変えない。保存済みauthoring寸法がある場合はその値を優先し、stack/通常/PDFで検証する。

## R07 [P2 / F03 残件] 単枚Ctrl+EとMergedは今も焼き込み段の設定を使用しない

- 場所: `src/ui_fullscreen.rs:35410–35427` / `35429`、`src/external_tool.rs:2162–2180`。
- 単枚は`export_page_pixels_for_idx`から表示済み最終画素を取り、`settings.bake_stage_export`を読まない。Mergedも同じ左右の表示画素を合成し`page_edits: None`にするので`bake_stage_external_tool`を使わない。
- 条件: セピア/LUT等が表示中に、単枚を「編集」または「AI処理」までへ変更して書き出す。表示用効果が残る。Mergedも外部ツールを「編集」にしても表示用効果を含む。
- `docs/bake-stage-unification-plan.md`は段取り5–7未着手と明記。これは以前からの指摘の残件で、今回runnerを接続した製本/一括/外部個別の修正とは分けて評価する。
- レビュー中の`2eac62930`でREADME/マニュアルにも「1枚を含む4出力で選べる」と追記されたため、未実装設定を利用者へ約束する状態が強まっている (`htdocs/mimageviewer/manual/export.html:260` 付近)。
- 修正境界: 全producerをstage付きのsource合成へ接続。各出力×3段を、同一素材の寸法/画素で確認する。UI選択を消すだけの対処は仕様変更になる。

## R08 [P2 / F04 残件] 6msのフレーム予算は個々の同期I/O・展開・AI初期化を止めない

- 場所: `src/ui_dialogs/export_batch.rs:145–157`、`src/ui_fullscreen.rs:34700`付近の`book_baked_edit_snapshot`、`34874`。
- `batch_export_item_for_target`を同期で完了した後にelapsedを検査する。maskのDB読出し/展開、local-adjust load、font準備、AI runtime初期化はまだUI上。1件に100ms以上かかればその全時間UIが止まり、6msでは打ち切れない。AI段の初期化は今回さらにこの経路へ加わった。
- 多数件を一度に処理しなくなった効果はあるが、UI responsivenessの根本境界は変わっていない。`draw_export_batch_dialog`はWindowを描く前にこの準備を呼ぶので、初回表示やcancelも待たされる。
- 修正境界: UIが軽量identity/未保存overrideを固定し、重いsnapshot作成をworkerへ。遅いDB/大きなmask/未初期化runtimeを注入し、準備中のUI tickとcancel応答を確認する。

## R09 [P2 / F05 残件] Mergedの画素合成が引き続きUI入力handlerで実行される

- 場所: `src/external_tool.rs:2162–2173`。
- SHA-256を消した判断は正しい。Renderedはsource stampが無く、lookup/insertの両gateを通らず、ファイル名にもfingerprintを使っていなかった。
- ただし`render_export_pixels`のcrop/回転/左右画像コピー/合成は残る。巨大見開きで画素数に比例する処理を終えてからworkerへ渡し、cancelは明示的に常時false。元指摘はhashだけではない。
- 修正境界: 左右sourceと編集snapshotをworkerに渡して合成し、入力handlerでは全画素を読まない。hash削除でcache再利用を失った、という指摘ではない。

## R10 [P2 / F13 残件] 関連付けのcache missで同期Shell列挙へ戻る

- 場所: `src/ui_dialogs/context_menu.rs:1303–1309`。
- prewarm中や対象外拡張子のcache missで`enumerate_handlers`をUIから実行する。先行workerが同じ拡張子を列挙中でも待機状態を区別せず重複列挙する。prewarmは最大8拡張子かつ結果を最後に一括送信するため、最初の右クリックでは特に残りやすい。
- 結果: Shell拡張/関連付け先の遅延がメニュー表示とUI入力を止める。cache hitの改善は確認したが、同期列挙の経路は解消していない。
- 修正境界: 拡張子ごとの準備状態をworkerが所有し、メニューへ非同期に候補を渡す。機能を落とさず、in-flight/missでもUIからCOM列挙しない構成を検証する。

## R11 [P2 / F08 残件] 同じページを保持する別viewerは一括編集の変更を受け取らない

- 場所: `src/edit_bundle_bulk.rs:1034–1046`、`src/edit_bundle_app.rs:237`。
- request ownerを固定しそこで完了させる修正は構造的に適切。回転のみ/終了時drain/保持overrideもownerを通っている。
- 残る条件: A/Bの独立viewerで同じpage keyを開き、Aで一括貼付/リセット。DBとAは更新されるが、Bの`adjustment_page_params`、mask/local-adjust/comic等の保持状態と派生cacheへmutationが配送されない。Bは旧表示を使い続ける。
- `docs/detached-rework-plan.md:1481`にも未実装と明記。以前から単一貼付にもある問題なので、新しい修正が作ったP1としては数えない。ただし前回F08の解消条件に含めた境界であり、今回の一括機能でも再現条件が成立するためF08全体をcloseできない。
- 修正境界: page-key単位のmutation通知を該当contextへ配送。別page/contextは失効させない。2窓同一ページと2窓別ページの対になる回帰テストが必要。

## R12 [P2 / F06 修正による退行] ゲームパッド切断時に保存済みの評価変更のUndoが失われる

- 場所: `src/app/gamepad_input.rs:3504–3518` (`end_gamepad_input_session`)。
- `self.ring_picker = None`でpickerを捨てる。評価行のpreviewは既にDBへ書く (`3889` → `apply_picker_item_rating_targets`) 一方、開いた時のbefore記録からUndoを作る処理は通常の`commit_ring_picker` (`4516–4531`) だけにある。
- 条件: ゲームパッドのリングで画像/フォルダの★を変更し、閉じる前にパッドが切断される。新しい自動終了でoverlayは閉じるが、保存した評価だけ残り、before snapshotが失われてこの操作をUndoできない。
- OFF後のアナログ入力残りは直っている。問題は新しい終了経路が、live変更を持つpickerの確定/取消ライフサイクルを通っていない点。
- 修正境界: 入力セッション終端とpickerのlive mutation終端を一緒に確定する。変更を保持するならdurableな差分をUndoへ、取り消すならbeforeへ戻してから閉じる。通常閉鎖/切断/OFFを同じ保存済みrating fixtureで検査する。
- 前回の関連指摘: `src/gamepad.rs:130–140`のUIからのthread joinも残る。テストcfgではjoinが除外されるため通常の単体テストでは停止待ちを保証できない。

## R13 [P3 / F13 修正による退行] 関連付けcacheを更新する方法がなくなる

- 場所: `src/app.rs:53031`、`src/ui_dialogs/context_menu.rs:1303`。
- メニュー終了時のclearを廃止した後、既存拡張子を失効/再列挙する経路がない。folderの世代が変わっても`contains_key`ならprewarm対象から除外する。
- 条件: 一度拡張子を表示した後にアプリのインストール/アンインストール/関連付け変更を行う。一覧を再読込しても、追加アプリは現れず削除済み候補は残る。一時失敗で空リストをcacheした場合もプロセス終了まで固定される。
- 修正境界: 明示更新、関連付け変更、適切なcache lifetime等でworkerへ再要求する。毎回UI列挙へ戻して直さない。folder再読込と空結果からの回復を検証する。

## R14 [P2 / F03 接続後の新規問題] AI runtime初期化失敗を「AIなしの出力成功」に変換する

- 場所: `src/materializer.rs:748–760`、`src/ui_fullscreen.rs:34872–34882`。
- model選択がない場合とruntimeを作れなかった場合を同じ`ai=None`にする。`compose_book_page`はNoneならAI段を飛ばし、その後のencodeを成功させる。
- 条件: AI拡大/denoiseの設定とAI以上の段を選び、ORT初期化が失敗する。worker側はログへ書くだけで通常サイズの画像を出力/外部起動し、成功通知になる。新runner自身が定義した「AI失敗は黙ってAI抜きへ落とさない」契約にも反する。
- UI側の`ensure_ai_runtime`はRemote側で初期化中なら未準備のままreturnする経路も持つ (`src/app.rs:53163`)。準備中と非要求を同じNoneで表現する問題は共通。
- 修正境界: AIの非要求/準備中/初期化失敗/実行可能を区別し、必要なAIが準備できなければ結果を成功にしない。モデル非選択や機能OFFは正常な無処理として維持し、初期化失敗を注入した出力テストを追加する。

## R15 [P2 / F10 の同型経路の取りこぼし] 製本・一括exportは入れ子お気に入りの外側標準を無視する

- 場所: `src/ui_fullscreen.rs:34697–34717` (`stack_member_effective_params`)。呼び出しは製本 (`34650`) と一括export (`35160`)。
- 外部ツールの`stack_member_default_params`は共通`active_favorite_default_id_for_path`へ修正された。一方こちらは最も近いお気に入りを1件だけ選び、その1件に標準補正がなければglobalへ落ちるまま。
- 条件: 外側 `C:/pictures` に標準補正A、内側 `C:/pictures/set` はお気に入り登録だけで標準補正なし。内側のスタックメンバーにページ個別補正がない。通常表示と修正後の外部ツールはAを使うが、製本/一括exportはglobalを使う。
- 結果: 同じページ・同じ段でも色調、LUT、AIモデル選択等が出力経路で変わる。外部ツールに限定したF10の主症状は直っているが、根本的なparams解決の共有は完了していない。
- 修正境界: ページ個別の優先を保ち、製本/一括も同じ祖先標準resolverへ通す。外側のみ標準あり/内側にもあり/ページ個別ありの3条件を各producerで固定する。

## テスト報告の評価

- `scripts/test-full.ps1`は8,159 passed / 0 failed / 36 ignored。mimageviewer libは7,235 passed / 30 ignored。型/既存テストの整合は確認できた。
- 「productionを元に戻すと落ちる」検査は実装者の報告として扱い、この再レビューでは変更を巻き戻してのmutation testを再実施していない。
- 追加のAI配線テスト (`src/books.rs:2389`) はソース中の`book_ai_snapshot(`存在等を調べる。単枚設定を消費すること、cache identity、stack注釈寸法、runtime失敗時の出力は保証しない。
- Renderedテストはsource path/stampの性質を確認するが、実lookupと成功後insertの両経路を呼んで固定するテストではない。両gateは今回コードを読んで確認した。
- Remoteの追加テストはleaseがbarrierへ反映されることを固定する。両workerが最後までleaseを保持することは型とclosureのdrop順を別途読んだ。GPU/実端末の停止・再接続は未実施。
- 数値probeと注釈probeは既存テストが緑でも残る問題を示している。全体gateの成功を全件解消の根拠にはできない。

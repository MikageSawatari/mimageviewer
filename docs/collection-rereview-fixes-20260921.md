# v4.0.0 再レビュー対応台帳

2026-09-21: 利用者が、再レビューの妥当な指摘と前回からの残件を順次修正することを承認。
元報告は `review-v4.0.0/re-review-20260921/README.md`。元報告は当時の監査記録として保持する。

## 所有・不変条件

- 製品コードの実装は直列。親は設計・台帳、Sol xhigh の実装担当と別の Sol xhigh が独立レビューを担当する。
- 既存の未コミットのキー操作統一・サイドカー改善・文書差分を保持する。実装担当は割り当てたファイルだけを編集する。
- UI thread に I/O・待機を追加しない。モーダルで開くまで待つ承認済み仕様、キャンセル、要求の世代・所有境界を維持する。
- 通常 APPDATA を検証目的で変更せず、アプリの自動起動・操作は今回行わない。
- チャンクごとの焦点テストは実装担当が所有する。最終統合の全体 gate と確認 build は一度集約する。
- 既存の成功証跡は変更していない範囲だけ再利用する。新しいソースでの実機確認は未実施として区別する。

## 直列の作業順

| 段 | 指摘 | 範囲・受入条件 | 状態 |
| --- | --- | --- | --- |
| 1 | S2-3 / S2-2 / S2-4 | proof の再利用と sidecar owner のパス一致、hit/miss 計装、writer・失敗・dirty owner・予算の直接回帰。復元省略条件を緩めない | 実装・焦点テスト・独立レビュー完了、統合gate/build待ち |
| 2 | RE-1 / RE-2 / RE-3 / RE-4、RB-1 / RB-8 | バックアップ結果と時間の永続ログ、編集なし起動で世代を押し出さない設計、復旧手順。壊れた移行記録は自動破棄せず利用者が明示選択した場合だけ退避する | RE/RB実装・焦点テスト・独立レビュー完了、REは`6fdd912a7`、RBは統合gate/build待ち |
| 3 | RA-1 / RA-2 / RA-3 / RA-4 / RC-1 | パッドのソートを対象コレクションへ、無効理由、一覧列順と再生順の説明、音楽前後移動、日本語エラー | 実装・焦点10件・独立レビュー完了、統合gate/build待ち |
| 4 | S1-1〜7 / RC-2 | 操作番号の表示、本とコレクションの入口統一、固定本 cache、待機 UI が隠れたまま入力を止めない所有設計、共通 runtime の待機終端と計装 | 未着手 |
| 5 | 残る P3 / RD-5 / RE-6 / S2-1・12 | 個別根拠を照合して修正、既に延期合意済みの設計拡張とは分離。サイドカー保持量削減案は測定して判断し、未検証の割合を製品効果と断定しない | 未着手 |

## 再レビューとの判断差

- S2-3 はコード上の所有不整合であり、改善を同梱するなら出荷前に修正する。実機で編集消失を観測したわけではない。
- S2 のメモリは有界で LRU・終了時の解放がある。hit ログも既存である。「解放点なし」「効果を観測不能」はそのまま採用しない。
- RB-1 は復旧の出口を追加する。読み取れない記録を黙って空にする旧動作には戻さない。退避失敗や確認後の内容変更時は保護を維持する。
- S1-4 はキー操作からモーダルを外す方向ではなく、同じ意味のメニュー・ツールバーの入口を承認済みの所有経路へ統一する。
- S1-6 の背景クリックでの取消は必須ではない。明示取消を維持し、Esc は IME と同一要求の取消を守る。
- S1-1 は位置指定という仕様自体の変更ではなく、番号と変動条件を利用者が読めるようにする。
- RA-4 は P2 不具合として不採用、P3 の引数/説明整合と handler 回帰補強へ変更。音楽HUDが渡すgrid順は、正常Collection rootでは `start_manual_media_navigation` 冒頭のCollection専用ナビが先に処理するため使われない。wanted revision先行時も同じ経路。PhysicalSource/noncollection/generation不一致ではreader順もgrid順へ戻る。親と独立reviewerで全経路を確認し、実不具合と断定しない。
- RA-13も不具合として不採用。親と独立reviewerが単件SiblingZip/複数batchの到達と完了を照合した。変換は元archiveを残して未登録のZIPを作るため、Collection rootの明示参照は変えるべきでない。通常folderやCollectionのphysical childは従来どおり再読込する。rootへ通常folderの同名ZIP優先dedupを適用すると無断の登録変更になる。後続でbatchの説明コメントを明確化し、元レビューは歴史として保持する。

## 証跡

着手前 HEAD は `8e483f38e`。以前の全体 gate と build は
`target/sidecar-confirmation-reuse-20260921/` にあるが、本対応で変更した箇所の成功証跡としては扱わない。
各段の実装・独立レビュー・検証結果をここへ追記する。

段1: raw folder/data-dir/family の完全一致はChecking workerで判定し、cacheの候補取得では
LRUを更新しない。採用publishだけ昇格する。worker checkとApp採用/publishは別event。
独立レビューで判明したKeyMismatchのproduction経路未到達も修正し、cache→workerの回帰を追加。
`app::sidecar_restore::tests` 38 pass、`sidecar_import::tests` 28 pass/1 ignored、
`sidecar_import::probe_reuse::tests` 3 pass、`sidecar::tests` 51 pass。
fmt/check差分検査成功、独立sourceレビューblockingなし。全体gate/buildと実機は未実施。

段2 RE: `collection_store::tests` 40 pass、fmtと所有差分検査成功。
全変更入口のPrepare→必要時backup→transaction内再検証を独立reviewerが受理。
新規/未編集空DBの初成功後arm、WAL、no-op/reject、外部競合、通常backup失敗継続、
schema backup失敗停止・移行失敗rollbackを含む。通常logとperfへ成否/時間を記録。
本体・テスト・仕様/マニュアルは `6fdd912a7` に保存。関連実装計画/async文書は既存差分と共に保持。
文書小修正の `53ed94c34` と合わせ、local masterへコミット済み。pushはしていない。

文書の小項目: RE-6 は英語の保存データ一覧を日本語と合わせた（コレクションと世代バックアップ、
類似照合用の索引、外部ツール用一時画像、動画・波形cache）。RE-7 は一般語の「コレクション」を
「画像や本」へ変更。RE-12 は掲載合意を維持し「仕様上の制限」と明示、詳細への既存リンクを保持。
docs索引にはコレクションを触る際の案内と本台帳を追加した。
追加の文書照合: E-15はfull-path cache keyの対象一覧と判定正本、E-16は`pinned_collections`の設定表を補足。
RE-8/RA-17はテキストに現順は残るが別の手動順・shuffle設定は復元しない旨を明記。
RE-13の未リリース機能に対する「古い上限超過一覧」の説明を利用者マニュアルから削除し、内部の保護動作は維持。
RD-8は保持予算がpresentation単位であることを実装計画へ明記した。

## 小項目の棚卸し（未実装）

- RA-5/8/9/10/11/14 は段3の共通ソート入口・表示理由・同値再選択の扱いと合わせて検証する。RA-11では同値選択でも列ヘッダ順解除の意図を失わないこと。
- RA-1の親側照合: `gamepad_input.rs` の picker構築、preview、確定の3経路すべてがglobal sortを参照している。末端setterだけでなく、選択済みguardと表示値も対象collectionへ統一する。pickerが既に保持するowner/anchorの失効契約を保つ。
- RA-12は本のページ順固定という既存判断に沿うため制限を撤回せず説明を補う。RA-13はバッチ変換の到達性と参照先を確認してから、実一覧の更新要求をソート変更から分離する。
- RA-6/7、RD-1/2/3/4/9/10 は cache・import・prepared install の所有境界を保持する小改善として確認する。RD-4はDB側の容量判定を正本に保ち、重複や同時編集を無視した事前切り捨てはしない。
- RB-2/3は明示された元ファイル操作と利用者のメニュー設定の仕様でもある。解除失敗を削除へ転送していない限り、確認設定を勝手に無効化しない。確認文言の区別は改善する。RB-4/5は欠損参照・親なしの理由と到達先を確認する。RB-6は意図したGrid限定を説明する。
- RB-7はファイル種別の取得失敗でTreeへ広げない。RB-9は終了中に復旧画面を新規表示せず、未完了記録を保持する。無期限終了待ちの変更はworker寿命・再実行の安全性を設計してから行う。
- RC-3/7/8、RD-7は段4の待機状態・終端・再駆動の共通設計で扱う。RC-4/5は読取と変更の所有を混同しない。RC-6は既存計装の集計、RC-9はログ方針を確認する。
- RE-6/7/8/11/13/14、RA-16、RD-8は文書と外部表記の整合対象。RE-9は既存のテキスト出力方針と日本語パス往復を確認する。RE-10のOS予約名の断定は根拠を確認する。
- RA-17/RE-8: パスを有効順で出力する承認済み仕様は維持。DBの完全バックアップと同等とは説明しない。手動順・再シャッフルの状態を復元する新フォーマットは今回のバグ修正に紛れ込ませない。
- RE-12: 仕様上の制限の掲載自体は以前の利用者合意がある。不具合と区別した見出し・正本へのリンクで維持する。
- RA-15は現protocolの厳密一致で未知modeが通常到達しないため、現在の製品不具合とは分ける。RD-5/6とS2-12は計測や所有設計を伴う性能改善で、未評価の効果を保証しない。
- S1-12「スピナーが10Hzに制限される」は不採用。`vendor/egui/src/widgets/spinner.rs:40` の可視spinner自体が `request_repaint()` を発行するため、workerの100ms予約だけでは表示周期は決まらない。速度を上げる追加repaintは入れない。
- RE-10「Microsoftの予約名一覧にCOM0/LPT0がある」は、2026-09-21に確認した[公式の命名規則](https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file)の一覧では裏付けられない（COM1〜9/LPT1〜9と上付き数字）。同ページにはCOM0の名前空間リンクの例はあるが、予約名一覧と同一ではない。現OSでの失敗は未観測のため、確定不具合としては扱わない。

## 段2の設計条件（独立レビュー合意済み）

1. Backupはcollection DB actorが所有する。新規DBで空の世代を押し出さず、既存v1は移行前backup必須、通常v2は変更前backupを一セッション一回試行する。読取のみ、同値変更、競合拒否、影響なしのsource migrationで正常世代を消費しない。`VACUUM INTO`を既存transactionの内部で呼ばない。既存shared `db_backup`の他ストアの動作は変えない。
2. Parse失敗とRead失敗は型で保持する。Readは明示再読込、Parseは再読込に加えて退避の選択肢を用意する。退避の説明に「未完了の名前変更に伴う設定・参照の引き継ぎを自動再開できなくなる可能性」を含める。閉じる/取消では元bytesと保護状態を維持する。
3. 退避はworkerで行い、確認したファイルと現在の内容が同じで、依然として両対応formatとも解析不能であることを検証する。別ファイルへ衝突上書きせず、退避成功を確認できるまで保護を解除しない。保持しているlocal job・完了結果・deferred removalは消さず既存の統合とdurable ACKを通す。元の削除/名前変更を勝手に再実行しない。
4. 終了時には新しいblocking警告画面を開かず、未完了記録を保持してログに残す。既存in-flightの寿命と永続化を無視した単純な待機打切りは行わない。

段2はバックアップと移行記録の復旧を別の実装・検収単位に分ける。
バックアップは read-only の変更計画を `NoOp / Reject / Write` として一度判定し、
`Write` の場合だけ transaction 外で試行してから適用する。適用時には revision を再確認する。
同値変更・対象なしの移行で世代を消費しない。既存の未版管理 DB も schema 書込前の退避対象とする。
完成レビューでNewの扱いを補正: 空DBの最初の書込み前にはbackupしないが、その成功後はRequiredへ進める。
同じ初回sessionの次の実書込み前に一度backupし、作成直後の削除でも復旧可能にする。
最初のNoOp/Reject/失敗では進めない。
移行記録の退避は確認時の bytes と同一のファイルを扱い、検証後の差し替え競合も防ぐ。
Windowsの実装案は既存 `book_fs_journal.rs` の同一handle検証を踏襲する。
shareなし・read/delete accessでsourceを開き、同handleのbytesを確認して
`SetFileInformationByHandle(FileRenameInfo)`、`ReplaceIfExists=false` で同parentのunique名へ移す。
WRITE_THROUGHでnamespace変更のdurabilityを要求し、成功をlinearization pointとする。
成功後のcancelを失敗扱いに戻さない。path検証後にhandleを閉じてrenameする方法には戻さない。
完成レビュー補正: 確認後にsourceがmissing/validへ変わってもquarantine要求から採用しない。
Changedで元の保護を維持し、明示retryだけが新内容を採用する。取消とcommit開始は
Running/Cancelled/RenamingのCASで裁定し、Renaming取得前の取消は必ず退避を止める。
Renaming取得後は取消不可のcommit段階となり、APIの成否をそのまま回収する。
復旧画面は背面pointerも遮断するegui::Modalとし、headlessのclick/Esc経路で固定する。
取消要求だけでは画面を閉じず、workerのterminal結果を回収するまでmodalと背面入力遮断を保持する。
独立レビューはこの補正後のRB差分を受理。最新のaffected test結果を回収してから完成記録に移す。
新quarantine workerの終了はcancel/terminal回収で扱い、既存initial同期loadや一般のexit drainを
この変更に紛れ込ませない。

段2 RB最終証跡: `rename_key_migration::tests` 39 pass、`recovery_modal_` 2 pass、
changed-valid/missingの明示再読込とexit中worker回収の直接回帰各1 pass。
viewer-context audit、fmt、glyph、diff check成功。App-global journal ownerのためaudit免除追加なし。
モーダルテストは複数Appの同時生存によるfixture競合を分離して再実行した。
source独立受理済み。全体gate/buildと実機は未実施。
文書小修正は`13ba37a46`へ保存した。

段3証跡: `target/collection-rereview-20260921/stage3-verification.txt` と同所stage3-*.log。
bin check、焦点10件、fmt、glyph 0、viewer-context audit、所有diff check成功。
UI実handler、tooltip、root/header/local解除/Shuffle、stale reply、reader/spread、
Collection/Global ringのpreview/確定/取消/対象失効、manual navigation、terminal errorを含む。
source独立受理済み。既存ctx.run must_use警告は変更前snapshotにもあり無関係修正を避けた。

## 段4の設計条件（独立レビュー合意済み）

実装・検収は二つの区切りにする。先に番号表示・入口統一・固定本cache・IME対応取消を扱い、
次に共通read要求の寿命・backoff・tray可視性・待機計装を扱う。前者の成功テストは後者で影響した範囲だけ再実行する。
親が先にtut-books/tut-collectionsへ現在の番号規則（本は名前順、Collectionは作成順）と変動条件を追記した。
UIの番号列自体はまだ未実装。次担当はこの説明を保持しUIに合わせて必要な補足だけ行う。

共通のread要求ownerに有限の待機とbackoffを持たせ、混雑・起動待ち・revision不一致の再要求で寿命をリセットしない。明示的な新操作は新しい要求とする。タイムアウトはUI要求の終端であり、生存中のDB actorを失敗扱いにして二重起動してはならない。遅い結果の到着は要求identityで棄却し、既に表示中の内容を保持する。

PDF password等の利用者入力待ちをDB待機と同じdeadlineに含めない。入力と通知によるwakeupを確保してから不要な周期pollをなくす。待機モーダル中にmainをtrayへ隠す入口は、既存sidecarと同じ表示所有境界で保留する。別窓の入力遮断だけを外す修正にはしない。

独立レビュー追記: read owner は開始時刻・絶対期限・backoff・取消を一つの型で所有する。
50ms→200ms→1s の backoff を共通化し、再試行や再準備で期限を作り直さない。
shared catalog の結果採用と、期限切れになった「開く」意図の採用は分離する。
runtime の Starting 自体にも低頻度の観測を持たせるが、個別要求の timeout で actor を再起動しない。
PDF password 入力待ちは同じ要求の期限を pause/resume する。
tray の終了・installer shutdown は妨げず、通常の「閉じて格納」のみ描画中モーダルの所有境界で保留する。
RBで追加した復旧ダイアログも、同じモーダル/tray可視性の棚卸し対象に含める。
期限の値は実装時に既存の大規模コレクション計測と処理phaseを確認して決める。
正常な重い準備を短い固定値で打ち切る変更にしない。利用者入力待ちの除外と、
timeout後も元の表示を保持し明示的に再試行できることを検収する。
timeoutは要求の取消・採用権の退役であり、OSのファイルI/Oや既存DB actorを強制終了する仕組みではない。
遅いworker結果は要求identityで棄却し、workerの既存cancel/drop契約を維持する。

## 段3のソート入口設計（独立レビュー合意済み）

`CollectionGridSetOrderIntent` 相当の型にcontent stampと要求mode/sortを持たせ、UI各入口と
リングから同じ要求生成を使う。同値Manual/Standardは抑制するが、Shuffle再選択と、
列ヘッダ所有中の同値Standard（列順解除）は許す。上部Collectionメニューだけがこの解除導線を持ち、
Toolbar/表示メニュー/リングは既存のDetailsHeaderSort lockを維持する。
同値Standardかつheader所有中は永続値変更ゼロのLocalHeaderResetとして、Ready/exact stamp/wanted<=acceptedを
同じ同期境界で確認してその場で解除し、actor commandを出さない。NoOp/LocalHeaderReset/Mutationをtypedに分ける。
通常の変更は既存prepare installで解除し、error/stale/別rootへの遅着は触らない。
Loading/Deferred/Stale/Failed/DeletedとManual/Shuffle列固定の理由はtyped reasonから統一表示する。

リングは開いた時にGlobal/Collection/Lockedの対象をcaptureし、Collectionはinstalledのsortを初期値にする。
Collectionの途中previewはoverlay値だけ変え、確定時に一件だけ要求する（従来も一覧previewは実行されずglobalだけ変わっていた）。
Globalは既存live previewを維持。owner mount後とstart/reply時にcontent stampを検証する。
普通のフォルダ・bookmark/rating・Collectionの物理子の既存経路を変えない。

完成レビューで設計補正: 当初のno-op actor replyでheader解除する案は、要求後のheader操作を上書きする。
既存details_order_revisionのcaptureも、lazy metadata/filter/列表示設定で同root/revisionのまま進むため
正当な解除を落とし得る。親・独立reviewerでこれらを撤回し、上記LocalHeaderResetへ分離する。
永続値を変えない表示操作を非同期化しないことで、後発header操作・ABAとの競合自体を除く。
隣接の実際のmode/sort変更では、確定したcollection順のprepared adoption時にheaderを解除する既存契約を維持。
こちらは保存順そのものの変更なので、同値local解除とは区別する。外部refreshとlocal mutationを分けて
後発headerを優先させる仕様変更にはadoption markerの別設計が必要で、今回へ無断で混ぜない。

## 小項目の追加照合

- RC-4: 指摘されたexport helperはproduction callerがない。実際のGrid action入口と全件exportのread admissionを修正対象にする。mutation可否とread可否を分離し、既存operationの所有を保持する。
- RC-5: rename migrationの100ms repaint予約は既に `2a71679341` 由来で存在するため、この部分は解消済み。toolbar addのmutating gateは外さず、reorder再読込は未保存順を保持する。
- RD-1: import previewの不変なaccepted/invalid件数はparse時に求め、毎frame全件走査しない。
- RD-2/3: 全件容量拒否は成功と表示せず、上限表示を定数から生成する。
- RD-4: 残容量は説明値として表示可能だが、実登録判定はactorが正本。分類前の候補切捨ては重複/無効項目との関係で不正なので行わない。拒否予定分のstat省略は別のadmission設計を要する性能拡張として区別する。
- RD-10: cold prepareだけでなくwarm reuseのpreflight workerでも、保持用Arcとlive install用mapを分離してUIへmoveする必要がある。新worker/ownerを増やさず別構造chunkで扱う。既存のvideo worker配送用cloneまで解消したとは記録しない。
- RE-9/11: 外部へ持ち出すテキストはCLAUDE.mdのエンコーディング規約に合わせBOMを付ける方針。単体のパス一覧と全件indexを揃え、日本語パスの往復を固定する。indexのsortはDebugから安定した明示の綴りへ移す。既存の設定/通信向け綴りがあれば再利用し、別の名前表を増やさない。
  `collection_store/db.rs`の`sort_order_as_str`/`sort_order_from_str`が既にsnake_caseの永続名を所有している。RE-11はこの対応表を共通位置へ移してDB/exportで共有する候補で、settings全体のserde表現を変える必要はない。
- RA-6/7の親側照合: collection Auto比率cache actorにはUUID単位のdeleteがなく、Getだけがmaintenance epochを持つ。単にDeleteコマンドを足すと、別contextの遅いrecordで削除済みUUIDを再挿入できるため、catalogの削除採用とlookup/recordの寿命を合わせる設計が必要。無関係のcollectionを全消去しない。eligible_totalは現状collection rootでitemsを毎回走査するため、準備済みpresentationの不変な集計として保持する候補（contextのinstall/replaceと一致させる）。

### RA-6/7 後続chunkの設計合意

RA-6は既存cache submission ownerのmutex下で、session内の退役UUID集合とDelete enqueueを直列化する。
record/get/adoptが同じ退役判定を使い、先行Upsertはactor FIFOでDelete前に処理、遅いlookup/recordは拒否する。
当該UUIDのApp map/DB行だけを除き、無関係UUIDやglobal maintenance epochを変えない。
非stale catalogの採用時の旧definitions−新definitionsと、Delete成功replyのexact UUIDが退役の入口。
UUIDは再使用しないため退役情報はsession中保持する。DB復元は終了中のみの既存仕様を維持。
RA-7はprepare workerでthumbnail対象数を一回集計し、immutable PreparedSnapshotのscalarにする。
install途中は旧sessionへ問い合わせず新prepared値をseedへ渡し、公開後はid/revision/items generationと一致する
root preparedから取得する。遷移中不一致は0、physical child/通常folderは従来items.lenを維持する。
親と独立reviewerで既存構造内の変更として合意。実装は段3/4後に直列で行う。

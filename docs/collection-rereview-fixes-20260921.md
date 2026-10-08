# v4.0.0 再レビュー対応台帳

2026-09-21: 利用者が、再レビューの妥当な指摘と前回からの残件を順次修正することを承認。
元報告は `review-v4.0.0/re-review-20260921/README.md`。元報告は当時の監査記録として保持する。

**追補の現在地（2026-09-22）**: U-1（本からEscでCollectionへ戻る）とRA-3（列順の画面説明）は
独立検収・自動検証・確認用build完了。T2-1も利用者承認により10分の時間制限を廃止し、
独立検収・全体gate・更新build完了。T2-1を含む最新buildの利用者実機確認は未実施。
末尾の追補を現在の判断とし、以下の段4Bにある期限採用の記録は当時の判断として保持する。

**元対応の最終状態（2026-09-21 23:43 JST）**: 今回採用した指摘の実装・独立検収・自動検証・確認用buildを完了。
下の各段にある「統合gate/build待ち」は当該段完了時の記録で、最終統合結果は末尾を参照する。
通常profileのアプリは起動しておらず、今回の差分に対する利用者の実機確認は未実施。
動画pinの過剰再準備を減らす性能拡張など、明示した後続版項目は完了扱いにしない。

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
| 4 | S1-1〜7 / RC-2 | 操作番号の表示、本とコレクションの入口統一、固定本 cache、待機 UI が隠れたまま入力を止めない所有設計、共通 runtime の待機終端と計装 | 4A完了。4B待機owner/trayは完成レビュー指摘修正・焦点38件・10k計測1件・静的gate成功、独立source再検収blockingなし。最終統合gate/build待ち |
| 5 | 残る P3 / RD-5 / RE-6 / S2-1・12 | 個別根拠を照合して修正、既に延期合意済みの設計拡張とは分離。サイドカー保持量削減案は測定して判断し、未検証の割合を製品効果と断定しない | 5.1〜5.5の焦点・静的gate・独立検収完了。source freeze、最終全体gate/buildへ移行 |

段5の実装単位は、4B完了後に次の順で直列とする（調査・設計のみ先行可能）。

1. Auto比率cacheの削除寿命・thumbnail対象数集計・prepared payloadのworker側構築（RA-6/7、RD-9/10）。
2. 取り込み件数・容量拒否の表示、書き出しのread admissionとBOM/安定sort名（RD-1〜4、RC-4/5、RE-9/11）。
3. 欠損参照の「元の場所へ移動」と親なし理由、既存操作の回帰（RB-4/5/10/11）。
4. 別接続import後の動画pin表示の失効（RD-6正しさ部分）。性能拡張は利用者判断と分離。
5. S1/S2の残る小項目とcollection計装の集計（RC-6）、文書の完了・延期状態を確定。

各単位は実装前の前提確認、焦点検証、独立検収を行い、最後に全体gateと確認buildを集約する。

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

段4B: 完成レビュー後の焦点検証は16 command / 38 tests pass（`RUST_TEST_THREADS=1`）。
`target/collection-rereview-20260921/stage4b-focused.log`にfilter・exit・件数を記録した。
待機lease、managerの後発要求保持、runtime観測、Grid/navの期限切れと遅着拒否、PDF入力待ち、
trayの回帰を含む。独立source再検収は追加blockingなし。
release optimizedの合成10k件試験は別に1件pass。actor書込93.557ms、catalog読込0.196ms、
snapshot読込4.869ms、存在するローカル参照のprepare253.560ms、全missingのprepare84.088ms。
`stage4b-10k-release.log`へ記録。これは合成データの一回の測定であり、実媒体のdecodeや
ネットワーク/AVを含む利用者環境の上限保証ではない。最適化build約13分44秒は操作時間と区別する。
`stage4b-static.log`のcore check / fmt / glyph / diff-check / viewer context auditはすべてexit 0。

段5.1: Auto比率cache 8、catalog退役 1、eligible stale 1、全reuse-key要素とOversized 1、
physical child 1、保持予算超過時のlive全payload保持 1、最新root配送 1、source identity 1の計15件pass。
core check / fmt / glyph / diff / viewer context auditも成功。独立reviewerはsource/docsと証跡を照合して受理し、Cargoは再実行していない。
`app.rs`は段5.1直前snapshot比でauto eligibleとseed引数の2 hunkだけ。統合gate/buildは未実施。
証跡は`target/collection-rereview-20260921/stage5-1-focused.log`と`stage5-1-static.log`。

段5.2: 12 commands / 15 tests pass、core check / fmt / glyph / viewer context audit / diff-check成功。
プレビューの行をprivate・不変accessorとしparse時集計との乖離を防止。容量全拒否と部分追加の表示、
書き出しのread admission、未保存並び順の保持、BOM・日本語パス往復・sortの安定名を検証した。
import previewのsnapshotを更新し実装担当が画像を確認。独立source再検収blockingなし。
証跡は同ディレクトリの`stage5-2-focused.log` / `stage5-2-static.log`。GUI起動はしていない。

段5.3: `stage5-3-focused.log`は10 commands / 13 tests pass、`stage5-3-static.log`は
core check / fmt / glyph / viewer context audit / diff-checkの5項目成功。placeholderのJumpだけ登録パスを使い、
削除/drag/shell/外部toolの能力は追加しない。NoParent、exact不在、復元との競合、通常rescanのmenu ownerを検証。
`app.rs`は直前snapshot比3 hunkだけ。独立検収blockingなし。レビュー中のモデル混雑による中断は同じ担当で再開し、
完了済み照合と成功証跡を再利用した。GUI起動はしていない。

段5.4: `stage5-4-focused.log`は9 commands / 10 tests pass、`stage5-4-static.log`は
core check / fmt / glyph / viewer context audit / diff-check成功。動画pinのcommit済み件数と部分成功、
適用0・rollback、Import受理一回だけのepoch前進、旧payload拒否、全contextのpresentation失効を検証。
独立レビューで指摘された過剰なload state取消を修正し、未開始/読込中のleaseと終端状態を保持する回帰も追加した。
`app.rs`は直前snapshot比5行追加のみ。独立source再検収blockingなし。統合gate/build・実機確認は未実施。

段5.5: `stage5-5-manifest.txt`のRust 6 commands / 6 testsとPython集計46 tests、計52件pass。
core check / fmt / glyph / viewer context audit / py_compile / diff-check成功。
全20枠の番号対応、SavedGroupのGrid/Press/空既定/INI対応、誤ったfullscreen経路の理由付き拒否を検証。
sidecarはwriter優先、metadata不一致短絡、一致時full hash、warm未採用proof保持とLRU非昇格を確認した。
集計はsession/request IDと属性整合を照合し、部分ログや未知の終端から成功を推測しない。
独立レビューでowner属性の取りこぼしを修正し、追加blockingなしで最終受理。
途中のモデル混雑による中断は同じSol xhigh担当で再開し、既存証跡を再利用した。

文書の小項目: RE-6 は英語の保存データ一覧を日本語と合わせた（コレクションと世代バックアップ、
類似照合用の索引、外部ツール用一時画像、動画・波形cache）。RE-7 は一般語の「コレクション」を
「画像や本」へ変更。RE-12 は掲載合意を維持し「仕様上の制限」と明示、詳細への既存リンクを保持。
docs索引にはコレクションを触る際の案内と本台帳を追加した。
追加の文書照合: E-15はfull-path cache keyの対象一覧と判定正本、E-16は`pinned_collections`の設定表を補足。
RE-8/RA-17はテキストに現順は残るが別の手動順・shuffle設定は復元しない旨を明記。
RE-13の未リリース機能に対する「古い上限超過一覧」の説明を利用者マニュアルから削除し、内部の保護動作は維持。
RD-8は保持予算がpresentation単位であることを実装計画へ明記した。

## 小項目の棚卸し（完了分を含む判断記録）

- RA-5/8/9/10/11/14 は段3の共通ソート入口・表示理由・同値再選択の扱いと合わせて検証済み。RA-11では同値選択でも列ヘッダ順解除の意図を失わないことを固定した。
- RA-1の親側照合: `gamepad_input.rs` の picker構築、preview、確定の3経路すべてがglobal sortを参照している。末端setterだけでなく、選択済みguardと表示値も対象collectionへ統一する。pickerが既に保持するowner/anchorの失効契約を保つ。
- RA-12は本のページ順固定という既存判断に沿うため制限を撤回せず、manual/fullscreen.htmlへ画像だけの通常フォルダを本扱いした場合も詳細列の並べ替えが固定になる旨を追記した。RA-13は到達性と参照先を確認し不具合ではないと判断済み。根拠は上の判断差と`893b25c8e`の説明コメントに残した。
- RA-6/7、RD-1/2/3/4/9/10 は cache・import・prepared install の所有境界を保持する小改善として確認する。RD-4はDB側の容量判定を正本に保ち、重複や同時編集を無視した事前切り捨てはしない。
- RB-2/3は明示された元ファイル操作と利用者のメニュー設定の仕様でもある。解除失敗を削除へ転送していない限り、確認設定を勝手に無効化しない。確認文言の区別は改善する。RB-4/5は欠損参照・親なしの理由と到達先を確認する。RB-6は意図したGrid限定を説明する。
- RB-7はファイル種別の取得失敗でTreeへ広げない。RB-9は終了中に復旧画面を新規表示せず、未完了記録を保持する。無期限終了待ちの変更はworker寿命・再実行の安全性を設計してから行う。
- RC-3/7/8、RD-7は段4の待機状態・終端・再駆動の共通設計で扱う。RC-4/5は読取と変更の所有を混同しない。RC-6は既存計装の集計。RC-9は不具合修正から除外する。§23.1の「個々のsource pathを記録しない」は追加したperfイベントの契約であり、既存の通常診断ログ全体の匿名化ではない。既存navigation commitの通常ログには対象パスがあるという観測は正しいが、新しいread_lease計装にパスを加えず、既存診断の情報を今回無断で削らない。
- RE-6/7/8/11/13/14、RA-16、RD-8は文書と外部表記の整合対象。RE-9は既存のテキスト出力方針と日本語パス往復を確認する。RE-10のOS予約名の断定は根拠を確認する。
- RA-17/RE-8: パスを有効順で出力する承認済み仕様は維持。DBの完全バックアップと同等とは説明しない。手動順・再シャッフルの状態を復元する新フォーマットは今回のバグ修正に紛れ込ませない。
- RE-12: 仕様上の制限の掲載自体は以前の利用者合意がある。不具合と区別した見出し・正本へのリンクで維持する。
- RE-14: 実装計画§23の先頭は§23.10の文書整備完了と、native入力/過去項目の実機確認待ちを分けて記載済み。今回の再レビュー修正と最終gateの状態は本台帳で追加追跡し、旧gateを今回の成功証跡に流用しない。
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

段4A証跡: `target/collection-rereview-20260921/stage4a-verification.txt`。
saved_group_actions 15件、番号UI 2件、Collection番号/実削除後の繰上り、Failed表示、
モーダル背面入力遮断、更新後の本/Collection管理snapshot成功。
fmt、glyph、viewer-context audit、diff check成功。独立レビューblockingなし。
初回の別target cold buildはturbojpeg-sysのCMake installで失敗したが、既存default targetで
対象の製品テストは成功。実機やAPPDATA操作は実施していない。

## 段4の設計条件（独立レビュー合意済み）

実装・検収は二つの区切りにする。先に番号表示・入口統一・固定本cache・IME対応取消を扱い、
次に共通read要求の寿命・backoff・tray可視性・待機計装を扱う。前者の成功テストは後者で影響した範囲だけ再実行する。
親が先にtut-books/tut-collectionsへ現在の番号規則（本は名前順、Collectionは作成順）と変動条件を追記した。
4AでUIの番号列も実装し、説明と対応づけた。4Bはこの番号規則を変更しない。

共通のread要求ownerに有限の待機とbackoffを持たせ、混雑・起動待ち・revision不一致の再要求で寿命をリセットしない。明示的な新操作は新しい要求とする。タイムアウトはUI要求の終端であり、生存中のDB actorを失敗扱いにして二重起動してはならない。遅い結果の到着は要求identityで棄却し、既に表示中の内容を保持する。

実装中の独立照合で新要求の境界を明確化した。pending中のrevision前進は同じleaseを使い、
旧replyが最新wantedを満たさなければfinishせず次readへ引き継ぐ。終端後に最後のobserved/wantedより
strictly newerなrevision、または別のtyped source invalidationが届いた場合は、新しい自動refreshとして
新leaseを開始する。同revisionの通知/pollだけでtimeout済み要求を復活させない。
利用者の明示retry/open/refreshは同revisionでも新操作。これにより無限延長と自動追従の劣化を両方防ぐ。
managerのcoalesced nextもwatchと明示操作を区別する。shared leaseの旧reply側をfinishして
継続側までterminalにしてしまわないことを直接回帰に含める。

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

段4Bの完成レビューで、再試行のbackoffと投入済みworkerの完了観測を分離する必要を確認した。
50→200→1000msは再投入のadmissionだけに適用し、in-flightの完了観測は従来のbounded cadenceを維持する。
期限を過ぎたnext_poll_atから0-delay repaintを反復しない。入力待ちはpauseし、pollを行わない。
またmanagerの古いerror/disconnectは古い要求だけを終端化し、後発の明示要求を消さず、
まだ有効なnextを昇格させる。NotFoundは同一UUIDの削除確定なので、そのUUIDへのfollow-upを破棄し
catalogを再照合する。別targetへ切替済みのslotは保持する。これらは修正・直接回帰・独立再確認を終えてから段4B完了とする。
PDFのパスワード待ち中に新revisionで再準備へ進む場合も、同じ要求IDのleaseをresumeしてから
RequestNeededへ移す。Pausedのままdeferして永久待機になる経路を、password送信とは別に回帰で固定する。

段4Bの期限は、親・独立reviewerでactive-time 10分を最終保険として採用する。
通常の待ち時間の目標値ではなく、短い固定値で正常な重い処理を切らないための余裕とする。
phase前進でpoll stepだけ50msへ戻せるが、絶対期限は継承。利用者入力待ちだけpause/resumeする。
初回計測はoptimized条件の合成10k件でactor/catalog/snapshotとroot prepare（成功/全missing）を対象にし、
phase遷移・再試行・入力待ち・遅着の安全性はpure clock/handler回帰で固定する。
ネットワーク/AV等すべてが期限内に終わる保証とはしない。phase・active/wall・terminalの計装を残し、
期限短縮は別途実測が揃ってから判断する。

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
- RC-6の相関条件: 旧openはcontextのみ、installはrequest_generationのみ、navigationのbeginとroot_installでもsurface情報が揃わない。collection UUIDや時刻の近さだけで複数contextの操作を結びつけない。段4Bのrequest identity付きphase/terminalが揃う範囲で待機を相関し、旧ログは段別時間・outcome集計と未相関件数に留める。request IDはプロセス起動で再利用されるためsession境界を越えて結合しない。途中からのログや欠けた終端は成功/固着の証明としない。
  現sourceの`read_lease`はbegin後に`bind_viewer`でcontext/surfaceを初めて付与する場合がある。相関キーはsession内で一意なrequest_idとし、owner/context/surfaceは整合検査する属性として扱う。未付与→付与を別要求へ分断せず、付与済み属性の矛盾は正常な完了集計から外す。
- RB-4/5: 欠損placeholderの場所移動はread-only入口だけで登録元pathを使い、汎用drag_source_pathを有効化して削除/外部操作へ漏らさない。既存workerの親走査とexact不在通知を再利用し、root/UNC共有の親なしは更新中と区別する。RB-10はmodal/input gateで本当に到達するかを先に確認し、到達しない呼出へ不要なtoastを追加しない。
  独立照合済み: Jump専用ExactPathを維持すれば既存physical load ownerへ載る（元reportのselection=Noneでは不在通知が消えるため不採用）。Ready/NoParentをstaleと型で分け、workerを起動しない。RB-10は他contextの復元中とscan開始後の復元開始で実際に到達する。start/readyで要求を拒否する場合だけ元contextへ通知し、root/historyを保持する。通常folderの内容変更rescanでmenu owner失効、内容不変なら維持する回帰を加える。
  実装前提確認: `context_menu_model.rs`のJump専用kind許可へplaceholderを追加しないとmenu自体が生成されないため、同modelの限定条件と回帰も段5.3の所有範囲に含める。汎用の削除・外部操作・dragの許可を広げない。
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

### RD-6の追加照合と分割

親と独立reviewerで、別接続のmetadata importがcommitした動画ピンを、保持済みCollectionの
thumbnail sourcesが観測できない経路を確認した。元報告のmetadata_import_refresh.rsはreaderであり、
writerはmetadata_transfer.rsのATTACH transaction。データ破壊ではなく古い表示の再利用である。

正しさ修正は独立chunkとする。ImportSummaryにcommit済みvideo pin変更の型付き情報を載せ、
batch commit成功後だけ集計し、WorkerMessage::Importの最初の受理で一度だけCollection用の
thumbnail source世代を進める。AppのVideoPinDb handleが無い場合も観測できる所有にする。
terminal refreshの再試行ごとに世代を増やさない。partial importの過去commitは反映し、
rollback/書込失敗/適用0からは変更を発行しない。

全viewer contextのthumbnail presentationだけを保守的に再準備へ移す。
削除用invalidate_current_collection_grid_sourcesはinstalled_items_generationまで消すので流用しない。
完成レビューで、汎用のcancel_pendingもSnapshot/read ownerやFailed/Deleted終端を巻き込むため不適切と判明した。
状態別にReady/Emptyのpresentationを退役しPreparingだけ旧準備を取り消す。
RequestNeeded/Snapshotはprepare開始時に新epochを取れるのでlease/receiverを維持し、Failed/Deletedも自動再試行しない。
このthumbnail側だけを失効し、項目のbinding/generation/position、
physical child表示、再生中player、navigation pending/intent/fs-nav lockは保持する。
進行中navはreuse keyの世代不一致で既存の同一intent再準備へ進む。新epochもreuse keyへ入れる。
import時はO(context数)で、UIで全登録項目を走査しない。
隣接するRemoteは`remote_ipc/thumbnail.rs`で要求時にpin DBを直接lookupし、PCのretained pin mapを使わない。
既存のHTTP cache（60秒）は別契約として維持し、今回Remote protocolやcache policyは変更しない。

無関係な通常pinによる過剰prepareは別のP3性能課題。毎pinで全context×1万件をUI走査する
rebase案は採用しない。bounded path journalと次回利用時の交差判定が必要になるため、
2026-09-21に利用者が「表示の正しさを先に直し、性能改善は後続版へ分ける」を選択した。
この性能拡張は後続版へ送り、今回のRD-6は取り込み後の古い表示を残さない正しさ修正までとする。

### S1/S2小項目の現行再照合

- S1-10: 全action共通テストは存在。追加するのは本/Collection全20枠のslot対応と保存groupのGrid/Press/空既定/INI往復の表テストだけ。
- S1-8: Grid＋fullscreenのsilent拒否は存在するが、通常のmount済みcontextからの到達は未確認。防御の理由表示を追加する場合も誤登録を起こさず、拒否を維持する。
- S1-9: 不具合として不採用。List要求はWaitingBookListでterminal回収済みになってからScanning/Readyへ進むため、ready採用の退役時にdetachすべきList ownerは残らない。不要な追加cleanupは入れない。
- S1-13: 件数進捗は共通scannerのterminal-only channelを拡張する新UX。現在のphase表示/取消と、段4Bの有限待機・計装とは別。今回の不具合修正へ暗黙に足さない。
- S1-14: 残件は本台帳で追跡中。番号UI/入口は4Aで対応し、番号順自体は承認済み仕様。未採用の番号並替UIまで不具合として起票しない。
- S2-5: writer pending/failedの優先確認後、len/mtimeが違う場合にhash前でmissとできる。同size/mtimeは必ず全bytes hashし、strict fallbackを省かない。この短絡と直接回帰を段5で追加する。
  実装前提照合で、この順序を所有するのは`sidecar.rs`の`revalidate_disk_import_source_for_reuse_from`と確認した。同worker helperと直接回帰を段5.5の編集範囲へ追加する。metadata一致だけで復元を省略する経路は作らない。
- S2-9: warm hitの未採用/cancelだけは既存proofを残せる。LRUを昇格せず候補Arcをworker退役し、次回も全fence/token/marker検証。cold未採用はpublishせず、dirty/warning/failedは従来どおりevictする。
- S2-7/10/11: const予算assert、pub(crate)化、async/sidecar計画の所有・退役説明で解消済み。
- S2-6: 大きなproofが実質1件になる上限tradeoffは詳細planに明記済み。S2-1/12の軽量proofは測定を伴う後続提案。
- S2-8: worker完了後の取消でcloneが無駄になる場合はあるが、UIへ重いcloneを戻せない。第2worker/handoffを増やす最適化は実測後の別設計とし、正しさの不具合扱いにしない。

上記は独立reviewerのread-onlyコード照合による判断で、未観測の経路を実機で再現したとは扱わない。

## 最終引き渡しと実機確認の範囲

初回全体gate（`final-test-full.log`）は本体8,892成功・1失敗・47除外、他targetは通過した。
失敗は`commit_barrier_restarts_when_revision_changes_after_preflight`で、段4Bの同要求leaseによる
再試行待ちを導入した後も、直後の状態をSnapshotと期待していた旧fixtureだった。
親・独立reviewerが経路を照合し、製品のbackoffを外さず、RequestNeededの同一要求保持と
due後のSnapshot到達を検証する方針に合意した。修正・再検証が済むまで全体成功とは扱わない。
再実行の`final-lib-rerun.log`は本体8,893成功・0失敗・47除外（581.99秒）。製品コードは変更していない。
その後、当該testだけに有効なfs-nav lockを持たせるpositive fixtureを追加し、
`final-gate-fix-focused.log`で焦点1件成功、fmt/diff検査も成功した。
変更のないworkspace/統合/vendor対象は初回全体gateの成功を再利用し、本体の初回失敗をこの証跡で置き換える。
独立reviewerがpositive lock回帰まで再検収し、追加blockingなしで受理した。

`final-build-dev.log`の`build-dev.ps1 -PreserveRuntime`はexit 0、VCRT PE検査はruntime=4 / pe=2で成功。
core SHA-256: `39005F8C06DF442FE38CA89CEF548592E4AC44310FCCF7B9031A21F64D75CBD3`。
remote SHA-256: `9FBF80F94403CDF78E8698A0BD31271B1B874DAEB8D5C7063EA6F780BEEC45E5`。
証跡のまとめは`target/collection-rereview-20260921/final-integration-manifest.txt`。
ビルド前にresident不在を確認し、プロセス停止・アプリ起動・通常APPDATAの操作は行っていない。
既存の未コミット差分を保持したまま、native操作を含む統合差分のコミットは利用者確認後とする。

最終source freeze後、実装担当が全体gateと`build-dev.ps1 -PreserveRuntime`を一度集約する。
通常APPDATAを使う確認用coreはエージェントから起動せず、利用者へ引き渡す。
以下は確認項目であり、この文書への記載は実機操作の実行許可ではない。

- 本とコレクションの管理画面で1〜20番と説明が見え、番号アクションが同じ対象を開く。
- コレクションのソートをメニュー・ツールバー・ゲームパッドから変更し、通常フォルダのソートを変えない。詳細列ソート時の無効理由も確認する。
- 初回読み込みのモーダルから取消/Escが効き、待機中にトレイへ隠れて操作不能にならない。
- スマートフォルダ・コレクションでCtrl＋上下の往復、PDF/ZIPと別ウィンドウを含む閲覧を維持する。
- コレクションの取り込み件数・容量拒否、書き出した日本語パスの再取り込み、欠損参照から元の場所への移動を確認する。
- 動画ピンを取り込んだ後、保持済みコレクションでも新しいサムネイルへ更新され、再生中の動画や別窓の閲覧位置が変わらない。

壊れた移行記録・書込失敗などの破壊的fixtureは自動回帰の隔離データで扱う。
利用者の通常データを壊して実機再確認するような手順は要求しない。

## 追加の利用者報告: コレクションの本をEscで閉じた際の戻り先（2026-09-22）

利用者がコレクションからPDF/ZIPを開き、Escで閉じると実親フォルダへ戻ることを報告。
スマートフォルダでは期待どおり戻るとの報告。エージェントによる実機再現は未実施。
source上、`grid_parent_nav_target`と`resolve_grid_parent_nav`はCollectionのtyped returnを
優先するが、`resolve_return_to_parent_nav`にはその分岐がなく物理parentへ進む。
後者はEsc/Enter/右クリック等の共通close予約を消化する入口であり、この欠落を修正対象とする。
既存`collection_grid_parent_nav`のidentity/revision/entry anchorを再利用し、別の復帰stateは増やさない。
即時・通常navigation consumerは既にCollection restoreに対応している。
detachedの明示closeはmainの一覧を保持する既存契約を維持し、通常folder/SmartFolder/Backspaceや
別contextを変更しない。実装担当が前提・同型経路・回帰を確認し、別担当が設計と完成差分を検収する。
この追加修正は上記23:43のビルドには含まれない。検証・更新buildの結果は別途追記する。

回帰作成時に上記の前提を補正した。consumerはCollection restoreを受け取れるが、
`open_collection_grid`は旧fullscreenを閉じないため、resolverだけの修正ではroot再採用に進めない。
一般のCollection open全体へcloseを追加せず、pending close専用の共通適用境界で
旧fullscreenを閉じてからrootを開く。通常pending消化と即時消化で同じ処理を使い、
既存のnavigation優先順位、Backspace、detached closeを維持する設計を独立レビューする。
U-1実装後の焦点検証は8 filters / 13 tests成功。frame-start経由のPDFとpost-render経由のZIP、
root/order/exact entry anchor/scroll/fullscreen終了、一般Collection navigationのno-close、
descendantと別contextを検証し、独立source検収で追加blockingなし。
frame-startはpending close由来をローカル変数で入力裁定まで保持し、勝ったCollection closeだけに適用する。
新しいApp stateやUI I/Oを追加していない。証跡は`target/collection-return-fix-20260921/`。
確認用アプリが稼働中のためbuild更新は未実施。続いてRA-3へ進み、静的検査をまとめる。

## 2026-09-22 最終再レビューの追加対応

`07f2d63cc`の`review-v4.0.0/re-review-20260922/README.md`を照合。
前回P1の解消評価は受領し、元報告は監査時点の記録として保持する。
利用者の作業継続依頼に基づき、上記U-1の後、以下を直列に確認・修正する。

- T2-1: 意図的に閲覧中のため未駆動の再読込要求が、利用者の閲覧時間で期限切れにならないこと。
  新規未開始要求のdormant化を候補とし、既に開始した要求の期限を延長し続ける修正にはしない。
  進行中要求と閲覧・context切替の境界も前提確認する。
  独立照合でnew→dormantだけではSnapshot/Preparing中のfullscreen遷移やactive leaseの持越しを
  塞げないことを確認。viewer退避・再駆動境界の同一lease pause/resumeまで必要になるため、
  単純な2行修正は実施せず、今回の構造修正か後続版かを利用者に確認中。回答前に延期確定とはしない。
- RA-3: コレクションの列ヘッダ順は一覧表示のみ、送りは保存されたコレクション順であることを画面にも示す。
  StandardのCollection rootかつ列ヘッダ順が有効なときだけ、sortable headerのhoverに説明を追加した。
  状態表と既存reader orderの回帰が各1件成功、独立reviewer受理。Manual/Shuffle/通常folder/読込中には追加しない。
- 決定的Read失敗（権限拒否・journal pathがdirectory等）は今回自動退避へ広げない。
  内容を読めないままの破壊的操作を避け、既存保護を維持する後続課題とする。

静止時ヘルスチェックなど実機操作は別途承認が必要な検証枠として残す。
合成10k件の一回の計測値は、あらゆる低速媒体や環境での期限保証とは解釈しない。

U-1/RA-3の最終静的検査はcore check / fmt / glyph / viewer context audit / diff-checkの5項目成功。
`target/collection-return-fix-20260921/static-*.log`に保存。今回のfreeze版で全体gateを実行中。
T2-1の製品コードは未変更、確認用buildは利用者のアプリ終了連絡待ち。

### U-1 / RA-3 の最終検証と確認ビルド（2026-09-22）

上記の実行中・終了待ちの記録を更新する。全体gateは全targetを完走し、主libは
8,896成功 / 1失敗 / 47除外、他targetは成功した。唯一の失敗はテストhelperが作成応答だけを待ち、
実UIが選択肢として使うcatalogへの採用を待っていなかった競合である。製品コードは変更せず、
同じID/revisionのselected・snapshot・catalogが揃うまで待つfixtureへ修正した。
該当テスト1件とcollections module49件が成功し、独立レビューが受理した。
同一製品sourceの全体gate成功分を再利用し、fixture修正後の影響範囲を再検証した構成で合格とする。
全体gateコマンド自体の初回exit 101を、単独でexit 0だったとは扱わない。

アプリのプロセスがないことを確認後、`build-dev.ps1 -PreserveRuntime`はexit 0。
core SHA-256は`E04B3023EDAC8CC9CFAB7F761F8136BC8D50D10B1F3AC825885239656D2C7C7B`。
証跡は`target/collection-return-fix-20260921/final-evidence.txt`。アプリは起動していない。
このビルドにはU-1 / RA-3を含み、T2-1のタイムアウト廃止は含まない。

### T2-1：経過時間による打切り廃止の検討（2026-09-22）

利用者は長時間閲覧での誤失敗の不利益を重視し、10分のタイムアウトを廃止する方向での
検討を依頼した。上記の「pause/resumeを拡張するか延期するか」の選択待ちは、この依頼へ置き換える。
調査対象はCollectionReadLeaseが担うUI読取要求の時間制限全体であり、一覧更新だけに限定しない。
DBのbusy timeout、IPCや通信など別用途の制限は対象外とする。

守る条件は、明示取消・別要求への置換・世代/所有者検査・古い応答の不採用・実エラーの終了・
再試行の間隔制御を維持すること。共有actorの再起動やDB操作の取消を10分制限が担っていたとは
解釈しない。再openはUI要求を作り直すが、DB自体が停止している場合の復旧まで保証しない。
実装担当と独立reviewerが全consumerと管理画面の次要求の扱いを確認する。この節追記時点では
調査・設計検討のみで、タイムアウトの製品コードは未変更。

実装担当と独立reviewerの調査で、期限廃止が成立すると判断した。対象consumerは管理画面の
catalog/snapshot、Grid、Collection navigation、SavedGroupの番号・前後操作で、runtime Startingの
期限超過観測も整理対象。lease自体は要求識別・phase・backoff・pause/resume・計測の所有者として残す。
budget/active_deadline/expiredとconsumerの時間切れ終端を一貫して除去し、DB busy_timeoutやIPCは変更しない。

管理画面の各slotは実行中1件と集約された後続1件を保持し、通常応答・実エラー・切断でも後続へ進む。
期限切れは唯一の昇格契機ではない。管理画面の閉じ直しは表示操作であり、共有actorが停止した場合の
復旧を保証しない。受理済み処理が永久に応答しない場合、ownerと既存50ms完了観測は取消/終了まで
継続する点を仕様上の代償として記録する。観測頻度の最適化は今回の期限廃止と分ける。

実装時の回帰は、長時間経過しても失敗せず正常応答を採用すること、managerの後続集約、Gridの
既存一覧保持、Navigationのlock/history、SavedGroupのモーダル取消、実error/cancel/stale/shutdownを
対象とする。特に長時間待機後の明示取消でworker・移動lock・モーダルが退役し、遅延応答が採用されない
ことを直接検証する。今回の検討終了時点では製品編集・この案のテスト実行はまだ行っていない。

### T2-1 実装着手（2026-09-22）

利用者から「実装まで進めて下さい」と承認を受け、上記の期限廃止設計を実装へ移した。
既存の実装・検証担当と、それとは別の独立reviewer（ともにSol xhigh）が継続して担当する。
実装と回帰の検証結果、独立レビュー、確認用buildの記録をこの節に集約する。
前節のU-1 / RA-3確認用buildはこの変更前のものであり、期限廃止の検証証跡へ流用しない。

製品・関連文書の期限廃止を実装し、焦点16件が成功。core check / fmt / glyph / viewer context audit /
diff-check / 旧期限API残存監査はすべて成功した。独立reviewerは、明示取消・置換・古い応答の不採用、
manager後続要求の有界保持、Grid既存表示、Navigation lock/history、PDF入力待ち、SavedGroupの
モーダル取消と遅延Book応答不採用を照合し、追加blockingなしで受理した。
旧perfログの`deadline_elapsed` / `timeout`解析は過去ログ互換のため残し、新しい製品動作は出力しない。
証跡は`target/collection-read-unbounded-20260922/`。この時点では全体gateと更新buildへ進行中。

最終検証: `RUST_TEST_THREADS=1`で`test-full.ps1 -SuppressCrashDialogs`を実行しexit 0、
主lib 8,897成功 / 0失敗 / 47除外、全体`[test-full] PASS`。
`build-dev.ps1 -PreserveRuntime`もexit 0でcore/remoteを更新した。
core SHA-256: `A6DD7F98F4618313113BD04F5EB9554F645624F3579976D91351488624BEB7CE`。
`verification-manifest.txt`に焦点・静的・独立レビュー・全体gate・buildの結果を集約した。
通常profile/portableともアプリの起動操作は行わず、実データを検証目的で操作していない。
長時間相当の状態を作る自動回帰であり、実アプリを24時間起動した実測ではない。
製品差分は未コミットで保持し、確認用buildを利用者へ引き渡す。

### 公開担当へのコミット引き渡し（2026-09-22）

利用者がコミットを依頼。上記の検証済み製品・回帰・関連文書をlocal masterのコミット対象に
まとめ、バックログの追記は別の文書コミットとする。別件の未追跡
`rating-snapshot-order-plan.md`は含めない。製品sourceは最終検証・buildから変更しない。
`fbdd7c42c`のClaudeCode再照合も出荷blockingなし。別窓EscとCollection背面/トレイidleの
実機確認は公開時の確認項目として引き継ぎ、実行済みとは扱わない。push・公開は本作業の対象外。

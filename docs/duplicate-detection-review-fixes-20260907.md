# 別バージョン検索のレビュー修正計画 (2026-09-07)

開始点: `duplicate-detection` / `52723f7ce` (v3.6.0 統合済み)。
指摘の根拠は [引き継ぎレビュー](duplicate-detection-handoff-review-20260907.md) の R1〜R9。
本書の設計候補は実装完了を意味しない。実装前検証、独立レビュー、実行結果を区別して記録する。

## 利用者指示と作業境界

- 設計・進行管理は親 Astra / high、通常実装・テストは Sol / xhigh、独立レビューは Astra / high。
- 実装担当は主要前提をコード・反例・測定で検証し、矛盾時は実装せず設計担当へ戻す。
- 編集担当は一人ずつ割り当てる。親は本書と引き継ぎレビューを所有し、実装担当は合意したコード・回帰テストを所有する。
- 音声途切れは利用者が解消を確認済み。高負荷走査の単純復活、ページ間引き、比較ボタン無効化による回避は採らない。
- `similar.db` と既存の派生配列を保持する。索引再生成や通常設定の初期化を検証手段にしない。
- 検証成果物は利用者指定の `target/portable-dev/mimageviewer.exe`。`data` と `data-remote` を保持し、`-Seed` は使わない。既存アプリは利用者が終了してから更新する。
- エージェントによるアプリ起動は使い捨て portable-smoke のみ。利用者の portable-dev は起動しない。
- 他worktree・master・他系統の変更を保持する。今回の修正をmasterへ逆統合・pushしない。
- ClaudeCodeを指名する旧文書の役割は、このタスクに限り上記体制へ移行する。detachedの構造修正条件、§11記録、既存テストとデータ保護は維持する。

## 修正単位

進捗 (2026-09-08):

| 指摘 | 実装・独立レビュー | 検証 |
| --- | --- | --- |
| R3 サムネイルとviewer所有 | `f0f5287e4`。独立Astra承認済み | 関連21件、check、fmt成功。全体gate・実機は後続 |
| R7 完成キャッシュ / R9 prefillとscope | `3d42f4a98`。実装・独立Astra承認済み | 索引39成功/4ignored、DB16成功/1ignored、check/fmt成功 |
| R2 類似移動でのパネルロック | 製品コード・テスト設計の独立Astra承認済み | 実handler/pollを含む16件・既存legacy lock1件・resolver1件成功。全体gate・実機は後続 |
| R4/R8 長押しと見開き | 独立レビューで実装境界を確認済み、実装前 | 未実施 |
| R1/R5/R6 本照会 | 厳密検索・一貫したDB読取の設計候補。試作前 | 正確性と実データ性能による採否判断が必要 |

### R2: 類似候補への移動と閲覧終了を区別する

類似画像と本のページ帯の両入口を、viewer内の移動として共有ナビゲーションへ接続する。
パネルのロックをUIで保存・復元する修正はしない。
通常フォルダ、ZIP/PDF非同期列挙、対象消失、取消、真の終了、snapshot、別viewerへの影響を確認する。
新たな同期フォルダ走査をUIへ追加しない。

実装前レビューで、`fs_nav_locked_gen` を開始するだけの案には次の不足を確認した。
PDFのpassword待ちは現在nav lockを解除するため、後続のitems差替えが真の終了へ誤分類される。
また入れ子ZIPは対象階層をmaterializeする前にRequired targetを検索すると、現存ページを欠落と判定する。
明示移動要求がpassword待ちを含めて所有する閲覧寿命と、画像ロード待ちのnav lockは区別する必要がある。
R2はこの設計を独立レビューで詰め、R3/R7/R9の独立修正を先行できるよう分ける。

設計候補は既存 `FsNavigationSequenceTarget` にpassword待ちphaseを持たせるもの。
画像待ちのholdover表示・入力blockは解除しても、viewer内移動の意図は同ownerに残す。
close/poll/drawはこのphaseを同じ意味で扱い、明示終了・取消・別要求は先にownerを終端する。
入れ子ZIPは既存bookmarkのtree target解決・prefix materializeを共通化してからRequired lookupする。
変換ZIPのaliasは、既存の `archive_source_override` と `current_folder == nav.tree.zip_path` が
確定している要求にだけ適用し、無関係なZIP内の同名entryへは着地させない。
静止画の類似移動を直すために、動画presentation transition全体へ新close理由を伝搬する案は採らない。

独立レビューで合意した実装境界:

- `FsNavigationSequenceTarget` の `FolderItems / AwaitingPassword / Display` を正本とし、
  viewer内の継続、入力block、旧画像holdover表示をownerのメソッドから別々に導く。
  password待ちは継続するが、入力blockと旧画像表示はしない。
- 明示候補移動は既存 `DeferredFsTarget::Required`、従来snapshotは `Preferred` として区別する。
  Requiredの消失時に無関係な先頭ページへfallbackしない。deferredをtakeしても表示完了前にownerを消さない。
- 通常フォルダは既存workerで走査し、成功して一覧を適用するときにsequenceを開始する。
  走査前の失敗/取消で旧viewerを閉じない。同items内の直接openは維持する。
- ZIP/PDFの `Opened / Deferred / NoPlayableItem / RequiredTargetMissing` を終端処理まで通す。
  パスワードretryは同ownerを再開し、cancelは終端する。
- Esc/Enter/右click/閉じる、Backspaceのページ一覧復帰、列挙待ち取消、別移動、context退役、
  エラー/切断を棚卸しし、真の終了はearly returnより前にsequenceを終端する。
- 入れ子ZIPはbookmarkの既存tree解決を共通化し、実entryと実階層をmaterializeしてから照合する。
  元要求の論理targetを書き換えて残さず、その要求内だけで実targetを使う。

これらは実装担当がコードと再照合する前提であり、まだ実装完了の記録ではない。

実装前照合で、通常フォルダは既存bundle-owned `FolderPaneOpenPending` の
`FolderOpenScanPurpose` にRequired targetを持たせて共有できると確認した。
既存 `snapshot_load_and_open` はそのままPreferredの挙動を保持し、明示候補の入口を分ける。

再照合時に `release_fs_nav_lock` へ「sequenceあり・fullscreen_idxなしならpanel終了」を
追加する案が出たが、親と独立Astraが不承認とした。password待ちから別候補へsupersedeする際も
その条件が成立し、新要求開始前にロックが失われるためである。
実装担当もこの反例を確認し、raw releaseは資源解放だけに保つ方針へ戻した。
再openしない `ViewerExited` と同viewerの要求置換 `Superseded` をtypedな終端処理で区別する。
password待ち→別候補のhandler経路でロック維持を必須回帰とする。

さらに独立レビューで、従来Ctrl+↑↓のlegacy `FolderNavigation` は画像取得に失敗しても
nav lockを取得する一方、owner自体がNoneになることを確認した。
legacy ownerに任意のprevious画像を持たせ、captureと非page bind fallbackの両producerが
画像なしでもownerを生成する形を採用する。legacy poll・動画bindの既存挙動は維持する。
描画がTargetReadyになったらpreviousだけを解放し、要求ownerは既存terminalまで保持する。
このOptionは移動状態のsentinelではなく任意の描画資源であり、移動中はenum variantで表す。
通常Ctrl+↑↓とsnapshotのロック維持、画像資源なしの移動も回帰に含める。

全srcを再検索すると `app/native_video.rs::toggle_still_window_mode` にも同variantのproducerがあり、
これはnav lockを持たない表示切替用captureだった。単なるOption包みでは新しい継続判定へ
誤分類されるため、親/独立Astraが `PresentationSwitch` としての分離を承認した。
texture/traceとviewport-enter描画は専用資源を参照し、timeout/中断は当該variantだけ解放する。
toggleで既存NavigationSequence/FolderNavigation (Display段階を含む) を上書きしない。
表示切替直後の真の退出、資源の描画/解放、移動中toggle/timeoutの非干渉を回帰へ追加する。
native presentation APIやregistry fieldの追加はしないが、静止画のウィンドウ/全画面切替は
portable実機確認にも含める。

### R3: 類似サムネイルの要求・完了・退避

受付からキャッシュ上限を維持し、退避済み・差替え済み要求の遅延完了を復活させない。
同じpathでも内容stampが変われば別要求として扱う。
実際の要求→完了→上限超過、旧成功/失敗、同一path更新を回帰で検証する。
GPU uploadとworker数も有界にし、他viewerのキャッシュや要求は変更しない。

実装担当の前提検証により、現行 `SimilarPanelState` はAppだけが保持し、
`ViewerContextBundle` のcapture/swap対象にないことを確認した。
親・独立Astra双方で、tab/origin/last_ready/texture/要求/完了channelを同じviewer bundleへ
移す構造修正を承認した。tabは永続設定でも全窓共通preferenceでもなくruntime選択であり、
新viewerはInfo、parkからの復帰は元tabを保持する。
編集範囲は `ui_metadata_panel.rs`、必要な専用module、`app.rs`、
`app/viewer_context_registry.rs` と回帰テスト。
main Aのpark中完了をA復帰時だけ消費すること、BのdropがAの要求/textureを無効化しないことを検査する。

先行レビューで、全候補を毎frame要求すると512件超で循環失効し、可視行だけの受付にしても
過去の待機/完了FIFOが新可視行を待たせることを確認した。
frame-localの需要集合を描画時に集め、描画後に現需要の完了を優先して1upload、
現需要の待機jobからworker枠へ投入する。画面外のReady・待機・完了画像は上限内で保持する。
loadingとcancel tokenは一つのPending状態へ集約し、描画用の状態はそこから投影する。
PDFは保存認証のrevisionを内容stampへ含め、App-globalのgrid render epochは含めない。
既存のbackground契約であるepoch=0と要求ごとのcancel tokenで寿命を所有する。

### R4/R8: 長押し表示の寿命と見開き

一時候補の表示権限と、通常比較のmode/sourceを分ける。
候補を単体で画像領域へ表示し、元のページ位置・見開き・左右順は変更しない。
release/focus喪失はパネルの可視性から独立した所有viewportの入力段で処理する。
未準備なら元表示を続け、release後の完了で候補表示を復活させない。
準備済み候補は次の長押しに再利用できるよう保持する。
通常比較Wipe/Diffの見開き制約を一律解除しない。

独立レビューで既存通常比較slot/pending/preparation/modeもApp-globalと確認したが、
今回これらの所有全体を移行する必要はない。一時表示だけをviewer-owned `SimilarPanelState` に置く。
純asset準備APIを共有し、peekは通常pin jobや現在ページとのpair準備を起動しない。
一時assetは実行1件＋最新待機1件に有界化し、取消不能な処理はDrainingで終端まで回収する。
新しいprimary press edgeだけがgestureを開始し、held levelから再生成しない。
release/focus喪失後、focusが戻っても同じ押下で再開しない。

描画はSingle/Spread分岐の前で一時assetを選び、本文とナビゲータが同じ画像・変換を参照する。
候補を元ページのidxへ偽装せず、編集座標や元ページの提示完了にも公開しない。
raw mount/swapには取消副作用を加えず、明示park/close/pagechangeとowner viewportでの
source/session照合により終端する。明示「比較に設定」だけが通常slotへassetをcommitする。
通常pin A→peek B→A、左右/LTR/RTL見開き、未準備release、hidden panel、focus復帰、
遅延完了、2viewer、画像/ZIP/PDF/認証更新、ナビゲータと編集座標の非干渉を回帰対象とする。

### R7/R9: 索引ジョブの終端とprefillの所有境界

R7の完成キャッシュは `NotIndexed` という照会結果を保持し、現在のジョブ進行状態に応じた
`Preparing` 表示は返値の投影とする。過去のRunning状態を結果に焼き付けない。

R9の実装前検証で、既存workerがrunごとに `prefill_db` を別Arcへ置換することを確認した。
scheduler存続中のDB ownerを固定してprefillとpruneの直列化を保証する。
prefillはDB mutex取得後に最新scopeを短時間参照し、scope guardを解放してから書き込む。
UIのscope変更はDB待ちや署名生成を待たない。runを跨ぐ旧prefillとON/OFF連打で、prune後の復活がないことを検証する。
DBは初回workerだけが開き、正常終了/取消/失敗後も同じArcを次runへ渡す。
query/array用の別接続はこの共有化へ含めない。global登録はDB/scopeのweak pairを維持する。
既存DB回帰の親root OFF・有効子root保持・prefix兄弟保持は引き続き実行対象とする。

### R1/R5/R6: 本照会の正確性と負荷 (採用前に試作・測定)

単体画像の線形検索は維持する。本照会だけに次の派生機構を検討する。

1. 専用接続の一つのread transactionでstore_id、変更連番、起点、候補、common、ページ帯を読む。
   捕捉配列から同transactionの連番まで変更履歴を追随し、新たな第三本の追加をcommonから漏らさない。
   有効favorite scopeも照会identityへ含め、変更後に旧結果を採用しない。
2. 不変base/deltaに厳密半径検索用の派生MIHを設ける候補。
   256bitを16bit×16に分け、1blockは距離2以内、他15blockは距離1以内のbucketを列挙する。
   全bucketに入らなければ距離は最低33なので、半径32の取りこぼしはない。
   最後に全256bitの距離を検証する。superseded/deletedも反映する。
3. commonは有効な異なる冊数で数える。raw256ページの打切りを根拠にしない。
   起点と候補の全ページを調べ、9冊確認した場合だけcommon確定として打ち切れる。
   確定済みcommonを分類器へ渡す入口を設け、全corpusの既存分類器を参照実装として同値性を検証する。
4. 同一要求を重複投入せず、実行中と最新待機を有界に所有する。取消と終了回収を区別し、
   旧worker終了前に際限なく次workerを投入しない。既存の低優先度実行を保持する。

MIHは462万件でposting約296MBとoffset約4MBを追加する概算であり、採用は未決定。
初回構築、warm照会、delta、compaction時ピーク、非一様なPDQ、反復署名で測定する。
通常/既存portableのDBへschema更新するbenchを直接実行せず、read-onlyまたは一貫したbackupで検証用コピーを作る。
R1の非推移性反例、R5の2冊300ページ一致、R6のtransaction中更新、array遅延中の第三本追加、
OFF後prune待ちの本を必須回帰とする。

独立レビューで、旧sweep helperの `zip` 比較は候補件数・キー集合の違いを見逃すため、
そのまま正確性oracleにできないと確認した。起点/候補keyで対応づけ、relation、matched、
distinctive、coverage、alignment、ページ帯まで比較する。
試作は合成反例→実PDQのbucket分布/重複排除/256bit検証件数→同transactionの全照会の順とする。
postingは連続配列、scratchは実行中の有界workerが所有し、多数threadへの複製を避ける。
初回構築込み・warm中央値/p95・総CPU時間・常駐/compactionピーク・delta追随を別に計測する。
既存400ページの約1.69秒は目標であり、新仕様の許容値ではない。
元DBへのread-only接続からSQLite online backupでworkspace内に整合複製を作る方法を検証する。
稼働中の `.db` 単体コピーは使わない。

## 完了の確認

各単位の前提検証→実装→独立設計/コードレビュー→狭い回帰を順に行う。
共有経路の変更を揃えた後は `scripts/test-full.ps1`、fmt、UI文字変更時のglyphチェックを通し、
portable成果物を作成してから手渡す。実機確認前の項目を自動検証済みと混同しない。

実装・検証の記録は後続の節に追記する。

## 利用者のportable確認手順 (修正済み成果物の作成後)

1. 旧 `target/portable-dev` のmImageViewerを終了してから成果物を更新する。
   `data` / `data-remote` は保持する。起動後、お気に入りと既存索引が引き継がれていることを確認する。
2. 右パネルをロックし、類似画像の「移動」と本のページ帯から別の通常フォルダ・ZIP・PDFへ移動する。
   同じviewerで閲覧を続けている間はロックが維持され、真の終了後の次回openには残らないことを確認する。
   入れ子ZIP、password入力が必要なPDFも対象とする。
3. 通常表示と見開きの左右候補で長押し表示を試す。パネル外で離す、別アプリへfocusを移す、
   読み込み前に離す場合も元の表示へ戻り、後から候補へ切り替わらないことを確認する。
   比較画像Aを表示中に候補Bを長押しした場合はAへ戻ることも確認する。
4. 候補が多数ある結果をscrollし、見えるカードが順次表示されることを確認する。
   2窓で異なる本を開き、片方の操作・終了が他方のタブ・候補画像へ混ざらないことを確認する。
5. 索引走査中と完了後の未収録画像の案内を確認する。
   索引の対象を変更する際にUIがDB待ちで止まらないことを確認する。
6. 従来と同じ本・候補で本照会を行い、YouTube等の音声が途切れないことを再確認する。
   正確性の反例・SQLite世代整合は自動回帰と照合し、見た目の確認だけで保証しない。
7. 静止画のウィンドウ表示/全画面を切り替え、表示とロック状態が維持されることを確認する。
   直後に閲覧を終了した場合はロックが解除され、移動待ち中の切替では移動先を失わないことを確認する。

この節は確認予定であり、実機検証の完了記録ではない。

## 実装・検証経過

### R3 初回実装 (2026-09-08)

`cargo check -p mimageviewer --bin mimageviewer-core --features pack-build-tools` は成功。
`cargo test -p mimageviewer --features pack-build-tools --lib similar_panel -- --nocapture`
は18件成功 (compile 2分09秒、test 0.15秒)。ログ: `target/r3-similar-panel-tests.log`。
ただし独立レビューで、要求→完了→Ready→退避を一続きに通す回帰と、
cancel済みworkerのterminal回収までを検査する回帰が不足していると確認した。
追加テストと最終レビューが済むまでR3を完了扱いにしない。

### R3 回帰補強 (2026-09-08)

`Pending { cancel } / Ready / Failed` の一つの状態ownerに要求を集約した。
描画前に完了を回収し、描画中に可視カードとhoverページ帯の需要を集め、描画後に
現在需要の完了を優先して1フレーム1件upload、現在需要の待機要求から最大4workerへ投入する。
非可視のReady・待機・完了は512件の上限内で再利用する。
PDFは内容・資格情報revisionで識別し、他viewerのglobal render epochでは失効させない。

実際の要求→完了→Ready→退避、取消workerのterminal回収、600候補の2フレームclip、
旧100件より新可視需要を優先する遷移、同一フレームupload制限、PDF資格情報、
viewerのpark/mount/retire非干渉を含め、同じ絞込コマンドで21件成功した。
ログ: `target/r3-similar-panel-tests-final.log` (test 0.19秒)。
上記binの `cargo check` と `cargo fmt --check` も成功。
独立Astra/highが最終コードと21件の成功ログを確認し、阻害するP1/P2なしで承認した。
全体gateとportableでの実機確認は後続であり、未実施。

### 本照会試作用の入力準備 (2026-09-08)

親の調査作業として、元portable-devのDBを `mode=ro` / `query_only` で開き、
SQLite online backupで `target/review-fixes-bench-20260908/similar.db` へ整合複製した (8.325秒)。
元アプリの停止、元DBへのschema操作、索引再生成はしていない。
対応する `similar.base` も複製し、複製DBの `quick_check=ok`、store_id一致、base checksum一致を確認した。
baseは4,628,611件・applied_seq=0、DBはlatest_seq=9852で、必要な変更履歴1～9852が連続して残る。
prototypeではこの履歴を適用してから同世代の比較に使う。入力準備は性能試作・採用の完了ではない。
手順と結果は同directoryの `input-provenance.json` / `input-validation.json` に保存した。

### R7/R9 実装・回帰補強 (2026-09-08)

R7はworkerが完成 `NotIndexed` を保持し、現在のRunning状態だけを返値へ投影する。
manager-localな `cfg(test)` の通知でworker完了前後を固定し、Running、Complete、Cancelled、Failedを検証した。
R9はscheduler存続中のDB Arcを固定し、scope確認からownedな準備値を返す処理と、
DB mutex内で最新scopeを確認して書き込む処理を分けた。scope guardは準備値に含めない。
DB保持中にOFF完了→書込み拒否と、旧ON読取→OFF完了→旧書込みcommit→purge削除の順序を検証した。

初回の索引回帰は38成功/1失敗/4ignored。新テストが100回yieldでSQLite worker完了を待つ方式で、
待機不足だったため完了通知へ修正した。次のcompileでテスト用unwrapに必要なDebugの不足を修正し、
最終は39成功/0失敗/4ignoredとなった。これらを製品機能の失敗とは混同しない。
ログ: `target/r7-r9-similar-index-tests.log`、`target/r7-r9-similar-index-tests-final.log`、
`target/r7-r9-similar-index-tests-final-v2.log`。最後の実行はcompile込み1分23秒。
独立Astra/highが最終差分と成功ログを確認し、P1/P2なしで承認した。
DB側の既存回帰は16成功/0失敗/1ignored (`target/r7-r9-similar-db-tests.log`)。
core checkも成功 (`target/r7-r9-core-check.log`)、fmtとdiff checkも成功した。
全体gate・portable実機は後続確認。

### R2 編集監査での復元 (2026-09-08)

実装途中の親のdiff監査で、`app.rs` に予定外の約3,800行削除を検出した。
当初は複数置換のoffsetずれと判断していたが、後述の隔離再現により編集ツールとCRLFの
組合せも原因候補として確認した。初回事象の原因は断定しない。誤った3hunkだけを逆適用し、
正しいR2変更を保持した。親も巨大削除の消失とdiff checkを確認した。
復元前差分は `target/r2-app-diff-audit-before-recovery.patch` に保持している。
以後は一意contextのpatchを一変更ずつ適用し、直後にdiffを監査する。
この段階のコードから検証成果物の作成・利用者アプリの更新はしていない。

追加の独立監査で、別編集による約130行の予定外削除と裸のコメント断片も検出した。
親は実装担当を停止し、そのhunkだけを復元して全コード差分を再監査した。
既存fileは1回につき一意contextの1hunkだけを編集し、直後に差分を確認する手順へ限定した。
構文確認を再開条件とし、実装途中の未定義参照とは区別して検査する。
構文確認は成功した (fmtは未整形差分だけを報告)。第2事象の直前の2つのpatch原文を
親が隔離したHEADコピーへ同順で再適用したところ、意図した6hunk (+46/-1) だけとなり、
破損は再現しなかった。第2事象の原因は未特定で、patch toolの欠陥とは断定しない。
原文は `target/r2-edit-incident-call.txt`、隔離試行は `target/r2-edit-probe/` に保持する。

同じ先行レビューで、`fs_nav_is_locked` をDisplay全phaseへ拡張すると、RenditionFailed後も
snapshotのCtrl+↑↓を拒否し続けるP2を検出した。AwaitingPasswordだけ入力を解除し、
その他の既存lockgenによる入力制約は保持する方針へ修正する。
viewer継続のtyped ownerと、従来の入力制約の意味を混同しない。

### R2 編集ツールの隔離再現と手順変更 (2026-09-08)

一意contextの1hunkだけでも約440行の予定外削除が再発したため、同じ手順を停止した。
親がHEADの隔離コピーをCRLFにして、その1hunkを `tools.apply_patch` へ渡すと、
意図は1行置換なのにsemantic diffが +123/-3489となる破損を再現できた。
LF版の先行試行では正常だった。大きなUTF-8/CRLFファイルとツールの組合せが再現条件であり、
ツール内部の原因までは特定していない。第1・第2事象を担当者のoffset処理だけに帰属させない。
証拠: `target/r2-edit-incident-third-call.txt`、`target/r2-app-third-corruption.patch`、
`target/r2-edit-probe/after-crlf.rs`。隔離コピーを製品のソースや検証成果物には使わない。

実装担当は破損した181 bytesだけをHEAD由来の正しい18,613 bytesへ復元した。
直前SHA-256、context一意性、変更外prefix/suffixのbytes一致、UTF-8妥当性とNULなしを確認し、
app.rsのsemantic diffは +138/-16、core checkも成功した。正しいR2変更は保持した。
以後、既存CRLF/混在sourceにはこのpatch toolを使わず、現ファイルのbytesを直前に読み、
一意contextの1回置換・書込直前hash一致・範囲外bytes同一・直後diff監査を行う。
全体の改行変換やファイル全体のcheckoutで復旧しない。

### R1/R5/R6 API境界の独立再照合 (2026-09-08)

既存SimilarDbのload_base_search_rows/load_item_changes_after/load_book_pages/
resolve_pages_by_item_idは各自mutex・transactionを取得する。新read transaction内から呼ばず、
SQL本体をprivateなConnection reader helperへ抽出し、既存wrapperとBookReadSnapshotで共有する。
本照会専用接続はschema操作をしない。read_seqは履歴prune後も残るsqlite_sequenceを使う。
履歴欠落・store不一致・array先行時の復旧は、同TX公開行からmemory-only snapshotを構築する。
永続base書込・history pruneを伴うload_or_rebuild/rebuild_from_sqliteをqueryから呼ばない。
ページ順修復はitem_changeを増やさないため、page orderは同TXから読む。

起点要求/scope/store変更とshutdownはhard cancel、同storeのarray進行・compactionはsoft staleとする。
後者では同TXの整合した結果を一度公開し、最新refreshを1件へまとめる。memory_epoch一致だけで
完成結果を捨てると、索引更新中の数秒queryが永久に表示されないためである。
『変更後に旧結果を採用しない』はfavorite scope等のhard identity変更を指す。
workerはmanager全体で1件、desired/requested/completedと最新待機1件はlive viewerごとのBookQueryClientが所有する。
同一完了要求を毎frame再投入せず、owner間はFIFO/round-robinで公平にdispatchする。既存last_readyはItemQuery専用であり、本の完成結果保持は新owner内の同一要求に限定して設計する。

新classifier入口はcommon状態だけでなく、同TX検証済みの対応辺(index_a,index_b,distance)を受け、
既存weighted_monotonic_alignmentと判定計算を共有する。全corpus入口は参照実装として残す。
少数2冊の同一頁が各10,000回反復する場合は真の辺が1億になるため、MIHだけで性能を保証できない。
試作で反復数を段階的に増やし、辺保持/alignmentが限界なら間引きせず設計へ戻す。
全頁commonとquality=0の既存表示区別を維持し、未依頼のUI分類を追加しない。

### MIH bucket分布の予備調査 (2026-09-08)

親が検証用baseコピーをread-only memmapし、quality正値4,619,313件の16bit bucketを集計した。
blockごとの最大bucketは784～1,843件。等間隔に選んだ2,048署名について392bucketを読む場合、
重複排除前posting参照数は中央値31,682、p95 39,884、最大68,844だった。
`target/review-fixes-bench-20260908/base-bucket-research.json` に詳細を保持した。
これはseq=0の分布調査で、実本のquery計測や正確性検証ではない。delta追随・全照会性能・
重複排除・alignment・音声影響について採用判断はまだ行わない。

### R4/R8 描画境界の独立再照合 (2026-09-08)

DisplayedImageTransform/Inputはpage_idxを必須とし、draw_fs_imageは元pageのcontent_bboxを読む。
候補を元idxやsentinelで渡さず、identity非依存の幾何計算・paintを共有helperへ抽出する。
従来page APIはwrapperとして残し、candidate identity付きの一時transformは別に所有する。
render_fullscreen_viewportのpage layout clear後、continuous/Single/Spreadより前で
Original/TemporaryPreviewを選び、本文・navigator・提示完了判定へ同じ選択を渡す。
候補表示では通常比較・旧holdover重畳を通さず、元のnormal pin/mode/pairは変更しない。

候補はcanonical orientationと候補自身のsource/texture寸法・全体領域を使う。
viewerのfit mode/scale limits/zoom/panは読取専用で適用するが、元ページのtrim、split、
rotation/free rotation、postfilter、調整は転用しない。必要なpan補正は一時transform内だけに留める。
release時は元の表示状態へ戻す。continuousでも候補一枚を同じ画像領域へ一時表示する。
panoramaでは現行UIに新規press入口がないため、新APIを加えずmode変更で取消する。

navigatorは同じcandidate asset/geometryから縮図を作り、FsNavigatorTextureSourcesへ
候補idxを偽装登録しない。通常page layoutを取得する部分と縮図の幾何計算を共有化する。
候補geometryはsingle_transform/fullscreen_page_layoutへ保存せず、候補frameで元pageの
emit_fs_page_turn_ready_for_display_unitやfs_painted_lastを更新しない。
編集・範囲copy・ルーペ・navigator等のcanvas操作開始ではpreviewを取消し、元pageのgeometryを
解決して通常操作へ渡す。候補の座標を元画像への操作に流用しない。

### R2 終端ownerの先行レビュー補強 (2026-09-08)

内部closeではNavigationSequenceとlegacy FolderNavigationの両方を閲覧継続として扱うため、
ViewerExitedの終端でも両variantへpanel終了を投影する必要があると独立Astraが指摘した。
NavigationSequenceだけに限定すると、従来Ctrl+↑↓のPDF取消/Interrupted/対象なし/非継続password待ちで
ownerだけ消え、ロック状態が残る。PresentationSwitchは移動ownerに含めない。
また通常folder scanは受付時に旧要求を取消し、成功時にだけ新viewer teardownを行う境界を監査中。
同itemsの別候補操作・真の終了・password待ちからの別scan後に、旧完了を適用しない回帰を加える。

### R1 対応辺が密な場合の設計境界 (2026-09-08)

独立Astraが現Fenwick alignmentの完全同点規則を確認した。A=X/B=XXXでは(A0,B2)、
A=X/B=XXXXでは(A0,B0)が選ばれる。候補辺のあるB座標だけを圧縮するtree形状にも依存し、
scoreだけ同じ別LCSへ置き換えるとページ帯の位置・クリック先が変わる。
同長かつ全eligibleページが同一署名なら、最適な単調全単射はk番目同士だけなので、
common判定後のO(n+m) shortcutを厳密に検証できる。異長/混在/非連続反復へ一般化しない。
一般の密な対応ではstreaming Fenwickとcheckpoint再実行が保持量を下げる候補だが、
CPU費用は残り、正当性・速度は未検証。性能試作で必要性が判明した場合に設計へ戻す境界とする。

### R2 最終コードレビューでの適用境界の訂正 (2026-09-08)

独立Astraのcall graph照合で、load_folder_with_scan_claimedに共通closeはなく、通常folderは
start_loading_items_innerでclose、ZIP/PDFは早期returnと条件付きcloseに分かれると確認した。
ZIPのsame-viewer移動はnav lockにより条件付きcloseを通らず、PDF cache missも列挙完了まで
closeしない。converted ZIPも同じZIP loaderへ到達する。
実装中Required入口のclose省略はこの前提を満たさず、旧fullscreen_idxを残すため訂正する。
共有適用境界は『owner取得→旧要求取消→snapshot解除→content close→load→strict open/defer』。
通常folderはscan成功後にこの境界へ入り、受付時は旧request取消だけを行う。
SLIでの重複closeは同じFolderItems ownerの内部teardownとして扱い、個別loader/video APIへ
症状ごとの分岐を増やさない。

併せて、実装中差分の次の取りこぼしを修正対象とした。
- Fs scan前のsnapshot解除は失敗/取消でもsnapshot/★固定/戻り先payloadを失うため成功applyへ遅延する。
- 新scan受付では旧folder-nav worker/累積stepも取消し、古いDFS完了で新要求が負けないようにする。
- RequiredFullscreenTargetは既存bundle-owned purposeであり、main/detached双方のapply consumerで扱う。
- deferred/embedded holdoverのEsc/閉じるもraw releaseだけにせず、ViewerExitedと旧列挙取消を通す。
- HEAD既存のZIP exact→item_key fallbackは共有resolverに保持する。今回新規のFs/PDF path_eqは
  Required resolverだけに置く。既存Preferredの挙動を縮小しない。
これらは検証成果物を渡す前のレビュー指摘であり、修正完了・回帰成功の記録ではない。

### R1 同長alignmentの厳密shortcut候補 (2026-09-08)

親が前記shortcutを一般化し、独立Astraが数学的成立条件を再確認した。
同じTX/scopeでidentity・common・qualityを検証したeligible列が両側N件で、実page_index順の
k番目同士がすべてradius以内なら、matched=Nを達成する厳密単調対応はk→kの全単射だけである。
署名が混在し距離が0でなくても、距離合計やFenwick同点処理に選択余地はなく、O(N)で確定できる。
元のpage_indexと実距離を返し、N=0、最低一致数、coverage、Strong/Weak等の既存分類は共有する。
params・同一本・page_index重複・署名幅など参照APIの入力検査も飛ばさない。

適用は候補発見と両側common確定後、pair辺の展開/ソート/Fenwick前とする。
候補発見段階で同署名の近傍結果を共有し、container集合と検証済みページidentityを保持して、
各Aページとの直積展開を遅らせる。先に全辺を作るとshortcut前に1億辺となり意味がない。
対角に一つでも半径外がある場合、異長の場合は通常の厳密経路へ戻る。
これは正当性の設計レビューであり、実装・oracle一致・速度の検証は後続で行う。

### R6 ページ順修復の通知境界 (2026-09-08)

独立再照合で、memory loadは索引workerのpage-order修復と並行し、修復前に本queryが始まれると確認した。
修復成功は現在logだけで、通常run終端のIdle化まで再照会されない。旧query→修復→Idle→同key新query→
旧query完成で新結果を阻むABAもあり、read_seqだけでは修復前結果を失効できない。
修復transaction成功直後に本query ownerへ通知し、既存の要求ID境界で旧要求を失効させる設計候補とする。
同read_seqでも再照会し、同TXのpage_order_versionをbook検証/stampへ含める。
originだけでなくcandidate/common対象の修復も通知の対象とし、UIにSQLや全book集計を追加しない。
reader側の修復・base再生成・人工的な履歴追加はしない。実装前に通知とownerの具体APIを再検証する。

### R2 中間compileと既存resolver回帰 (2026-09-08)

中間core checkは成功 (17秒、実装担当の実行結果)。最初のlib test compileは、新しい
FolderNavigationの任意資源型とAwaitingPasswordに既存test helperが未追随で、9 compile errorとなった。
helperを追随させた後、`cargo test -p mimageviewer --features pack-build-tools --lib
resolve_snapshot_target_idx_matches_each_leaf_kind` は1成功/0失敗 (compile 1分55秒、test 0.16秒)。
ログ: `target/r2-snapshot-target-narrow.log`、`target/r2-snapshot-target-narrow-compile2.log`。
これは既存resolverの狭域確認であり、新Required handler・全終端・複数viewerの回帰完了ではない。
レビュー指摘への最新修正、handler回帰、最終独立レビューは継続中。

### R2 製品コードの独立承認 (2026-09-08)

Required scan失敗をmain/detached共通化し、旧pageありはSuperseded（入力制限/資源を解放しpanel保持）、
pageなしはViewerExitedへ終端した。still-owned receiverのDisconnectedもRequiredだけ同じ終端へ進む。
新候補受付では旧DFS/累積stepを取消し、snapshotはscan成功まで保持する。
同contextの実open境界を共通化して、別viewerへの転送後、validかつ別display unit時に旧Required scanを取消す。
continuous seek/reanchorはidxまたはPageSliceの実変更を扱い、同項目内部再適用/無効idx/別contextは巻き込まない。
scan成功ではpendingは既にpoll.take済みで、新しく作る移動ownerを自分で取り消さない。

独立Astra/highが全体の製品差分と最後の2修正の直接影響を確認し、残存P1/P2なしでコードを承認した。
予定外の巨大削除はなく、HEAD既存のsnapshot ZIP fallbackも保持されている。
実handler回帰の追加・成功ログ照合、全体gate、portable実機確認はまだ未完了である。

### 最初のportable検証マイルストーン

利用者が右パネル修正を先に実機確認できるよう、R2のhandler回帰と独立承認後、R2/R3/R7/R9を
含む最初のfull gate・portable buildを行う。その後もR4/R8、本照会R1/R5/R6の試作・修正を継続する。
最初の成果物では未修正の指摘を完了と扱わない。ビルド元、成功ログ、実際の更新可否は作成後に記録する。
初回の実機対象は右パネルの場所間移動/取消/真の終了、別窓の状態独立、候補scroll、索引状態と対象変更、
静止画のウィンドウ/全画面切替である。長押しと本の判定修正の確認は、それぞれを含む後続成果物で行う。
`build-portable.ps1 -KeepRunning`で先にpackageを用意し、利用者が旧portableを終了した後だけ
`update-portable-dev.ps1 -SkipBuild`で更新する。data/data-remoteは保持し、Seed/初期化/agent起動はしない。

### R2 状態回帰14件の中間結果 (2026-09-08)

新規 `app::similar_navigation_tests` の初回compileはfixture内の借用競合6件で失敗した。
生成番号を先に読むhelperへ修正後、同filterは14成功/0失敗 (compile 1分08秒、test 2.18秒)。
ログ: `target/r2-similar-navigation-tests-compile1.log`、`target/r2-similar-navigation-tests-compile2.log`。

独立レビューでは実際の `open_similar_hit` / `open_similar_book_page`、ZIP/PDFのpoll完了、
Aの完了時にBの表示/待機が不変であること、PresentationSwitch自身の資源解放、legacy ownerの
実producer経由の確認が不足していると判断した。14件の成功だけでR2の検証完了とは扱わず補強中。
PDFのテスト用handle生成は既存coordinator/leaseを使うcfg(test) factoryに限定する。
### portable更新手順の独立確認 (2026-09-08)

独立Astraがscriptsの読取確認を完了。test-fullにはfmt/glyphが含まれないので別途実行する。
`build-portable.ps1 -KeepRunning`は当worktree直下から実行し、終了コード0とpackage完成を確認する。
updaterはdataだけを明示除外するため、更新直前の未起動packageにdata/data-remoteがないことを確認する。
対象アプリが終了している状態で`update-portable-dev.ps1 -SkipBuild`を実行し、更新後にpackageと
実行物のSHA-256を照合する。コピーは非原子的なので、途中失敗時は未更新と扱い、閉じたまま再実行する。
事前walkでは既存dist v3.6.0は24files/439399502bytesでdata/data-remote・reparse pointなし。
これはbuild前の確認であり、package再生成と更新直前には再確認する。
### R2 実機確認とcommit境界

PresentationSwitch分離はnative APIを追加しなくても、専用fullscreen viewportへの切替時の
表示保持・移動ownerへ影響する。親と独立AstraはAGENTS.mdの
「For these native features, commit after the user confirms on real hardware」に該当すると判断した。
R2は未commitで初回portableを作り、基点HEAD、最終fmt後の正確な差分、検証ログ、成果物SHA-256を保存する。
利用者の実機確認後にR2単位でcommitする。後続R4が同ファイルを編集する前にR2差分を確定保存し、
R2としてcommitする差分が検証版と一致することを照合する。R3/R7/R9等の既存commitは変更しない。
### 初回portable版の利用者確認手順

成果物の更新成功とハッシュ一致を記録した後、利用者が次で起動する。
`Start-Process -FilePath C:\home\mimageviewer-dupe\target\portable-dev\mimageviewer.exe`
既存portableのdata/data-remoteを使用する。エージェントは起動しない。

1. 右パネルをロックし、類似画像または本のページ帯から、別フォルダ・ZIP・PDFへ移動する。
   対象ページへ移り、パネルと類似タブが維持されることを確認する。
2. 読み込み中に別ページへ移動する／閲覧を閉じる。古い完了で元の移動が復活しないこと、
   真の終了後に画像を開き直したときはパネルのロックが解除されていることを確認する。
3. パスワード付きPDFを利用する場合、待機中の再入力と取消を確認する。
   別の無関係な先頭ページへ移動しないことも確認する。
4. 別窓を二つ開き、一方の類似移動・閉じる操作が他方の画像、パネル、類似タブを変えないことを確認する。
   静止画のウィンドウ／全画面切替も行い、画像の保持と操作の復帰を確認する。
5. 類似一覧をスクロールして候補サムネイルを確認し、お気に入りの索引対象を変更して操作が止まらないこと、
   索引終了後に未収録画像が「準備中」のまま残らないことを確認する。

音声途切れは前版で解消を利用者確認済み。今回のUI変更でも同時再生を確認し、再発があれば記録する。
R4/R8の長押しとR1/R5/R6の本判定は初回成果物の修正完了項目に含めない。
### R2 handler回帰の最終狭域結果 (2026-09-08)

初回final compileは新fixtureのVec<GridItem>直接比較2箇所でPartialEq/Debug不足の6診断となった。
product型のtraitを増やさず、既存perf_keyの順序付きidentity列の比較に修正した。
`target/r2-similar-navigation-tests-final-v2.log` は16成功/0失敗/0ignored、compile1分06秒/test2.83秒。
旧log `target/r2-similar-navigation-tests-final.log` も失敗証跡として保持する。
既存legacy lock回帰は `target/r2-legacy-lock-test.log` で1成功/0失敗。

独立Astraの確定差分レビューは製品コード・追加16tests・cfg(test) helperに残存P1/P2なし。
実hit/book-page handler、ZIP/PDF channelから実poll、main/detachedの完了・終了、閉じたBへのA完了の
非干渉、PresentationSwitch資源と期限解放、previous無しlegacy captureからの内部closeを確認した。
PDF factoryはtest用local coordinatorの正規leaseを使い、UI helperは実handlerへの委譲だけとした。
全体gate・portable build・実機確認はこの狭域成功時点では未完了である。
### R1 本照会の公平な所有境界への訂正

親がmanager-globalの「1実行＋1最新待機」案にviewer間の飢餓を懸念し、独立Astraが現経路を監査した。
passive detachedはsnapshot描画なので、二窓を放置しただけで毎frame相互要求する経路は確認していない。
一方、照会完了より速いA→B→Aのactivateでは要求が交錯し、共有latestで他viewerを取消す案は成立しない。

workerはglobalで1件に保ち、BookQueryClientを既存bundle-owned SimilarPanelStateに保持する。
要求ID・desired/requested/completed・取消・完成結果はclient単位。各live clientの最新待機を1件まで保持し、
FIFO/round-robinで処理する。同一要求のpollで順番を動かさず、soft refreshは列末尾へ戻す。
origin変更・要求撤回・retireは当該clientだけを取消し、park/swapは失効させない。
scope/store/shutdownは共有失効境界。完了・失敗・取消をdrainしたらUI pollingを待たず次ownerをdispatchする。
APIはquery_book(client, container_key)相当が必要で、container_keyだけでは同一本を要求する窓も分離できない。
待機量はlive viewer数に比例して有界となる。全体の待機1件を優先して他窓の機能を落とさない。

回帰はA/B交互poll、A park中完了、B dropからAへの非干渉、連続refresh下で両方完了、取消/失敗後の次要求開始。
既存last_readyはItemQuery専用で、本の直前結果保持が既にあるという記述を訂正した。
検索prototypeの採否評価と分け、product schedulerはこの境界を実装前に具体APIと再照合する。
### R2 初回full gateの失敗と追随方針

`target/r2-test-full.log` はworkspace終了コード1。本体libは7627成功/3失敗/38ignored、384.64秒。
workspaceの他suiteは完走したが、scriptは失敗を返しvendor2suiteには進んでいない。
失敗は既存の次の3件。親と独立Astraがfixture・描画/終端consumerを照合した。

- fs_nav_holdover_for_draw_bridges_until_new_target_content_ready:
  新画像選択後はprevious.takeでtextureを不可逆に破棄し、FolderNavigation(None)がterminalまで残る。
  owner全体Noneという旧assertを修正し、再Pendingで旧画像が復活しない検証を保つ。
  新thumbをLoadedに戻し実poll_fs_nav_lockを通して、lock/owner双方の解放まで確認する。
- fullscreen_folder_nav_close_preserves_still_viewport_for_reopen /
  start_loading_items_during_fullscreen_nav_keeps_viewport_reuse:
  raw fs_nav_locked_genだけのfixtureを実capture_fs_nav_holdover(idx)producerへ追随させる。
  viewportの表示・姿勢・世代・再生成防止の既存assertは保持する。

legacyの終端はmain/detached updateからpoll_fs_nav_lockのdisplay_ready/has_failedへ到達し、
lock/ownerをともに解放する。製品回帰を隠す期待値変更ではないことを独立確認した。
tests.rsだけ編集を再開し、狭域・fmt・full gateを再実行する。実機・portableはまだ未実施。
初回source checkpointはtarget/r2-portable-milestone-20260908に失敗証跡として保持し、
修正後は別checkpointを採る。R2のindexとcommit境界は親が管理する。
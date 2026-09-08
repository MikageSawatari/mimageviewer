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
| R3 サムネイルとviewer所有 | `f0f5287e4`。独立Astra承認済み | 関連21件、check、fmt、共通full gate成功。実機は後続 |
| R7 完成キャッシュ / R9 prefillとscope | `3d42f4a98`。実装・独立Astra承認済み | 索引39成功/4ignored、DB16成功/1ignored、check/fmt・共通full gate成功。portable更新済み |
| R2 類似移動でのパネルロック | 製品コード・テスト設計の独立Astra承認済み | 実handler/poll16件・legacy lock1件・resolver1件・追随3件成功。共通full gate成功。portable-dev更新済み、実機確認・R2 commit待ち |
| R4/R8 長押しと見開き | 候補owner・geometry/描画identity・終端/paint寿命・navigator所有/ordered入力・capture・依存API・実Ready描画dispatcher・context非干渉/受付は段階レビュー済み。純draw snapshotと依存統合も独立承認済み。最終gateで再現した初回DB open競合も修正・独立承認済み。全体gate成功、portable更新・24files照合済み | owner・geometry・GPU寿命・入力/capture・実描画dispatcher・context終端/非干渉の狭域回帰成功（内訳は経過記録）。vendor egui 25件成功。最終gate成功（main7693/0/38ignored、vendor25/9/15）。新portable更新済み、実機確認待ち |
| R1/R5/R6 本照会 | 検索kernel試作v2の独立レビュー・限定計測完了。独立要求ownerは3109b60e6、需要/通知はbe0075dc1で独立レビュー済み。generic gateはad2130581、readonly reader第1区切りは5e0d2bd1b。狭域/既存DB・array回帰/製品check/独立レビュー成功。製品caller未接続 | 単署名集合oracle・owner mock回帰成功。本照会全体・世代整合・負荷/peak/実caller公平性は未検証 |

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
### R2/R3/R7/R9 共通full gate成功とportable build開始

3追随fixtureは `target/r2-full-failure-fixtures-{1,2,3}.log` 各1成功。
独立Astraが実poll終端・既存viewport assert保持と成功ログを確認し、追加指摘なし。
`target/r2-test-full-v2.log` は終了コード0 / [test-full] PASS。
本体7630成功/38ignored (328.39秒)、snapshot54成功、vendor egui-wgpu9成功/eframe15成功。
他workspace suiteを含め、ログの51結果合計は8562成功/44ignored。失敗0。
初回本体384.64秒との時間差は測定条件を切り分けておらず、pool影響を特定したとは扱わない。

検証基点は `8c83ecdc81c781870573bbd81e8bce3fb719ff95`。
R2の8ファイルは未commitでstageし、source checkpointを
`target/r2-portable-milestone-20260908-v2/` に保存した。
patch SHA-256: `3adba6c9c6768c1540f3d8f0791d1dfefdc322acb270ec90966d5bca2131f42a`。
full gate成功時に8ファイルのdisk SHAがcheckpointと一致することも確認した。

`target/r2-portable-preflight.json` でpackageとコピー先対象/親のreparseなし、
packageのdata/data-remoteなし、対象アプリの稼働なしを確認。
`build-portable.ps1 -KeepRunning` を開始し、ログは `target/r2-build-portable.log`。
この記録時点はbuild中であり、portable-devの更新・実機確認は未完了。

### R1 dense alignment checkpoint候補の独立監査

checkpoint＋block再実行は、現Fenwickの完全同点手順まで再現できる設計候補として成立する。
A行グループ開始直前の全Fenwick cellをscoreと安定したtail(row,col)で保存し、
同じA行の全query後にB昇順updateする。tieでは既存値を保持し、queryのcell訪問順も変えない。
全passで辺を(index_a,index_b,distance)順にし、同座標は最小distanceへdedupする。
B座標圧縮は「辺のあるBだけ」でglobalに固定し、A/Bを転置しない。
Aが1行・辺B={0,2}の同点では現圧縮がB0、辺のないB1を足すとB2になる反例がある。

後方復元はtailが属するblock開始前checkpointから再実行し、そのblockの全辺parentを一時保持する。
親がblock外へ出たらその親のblockへ進み、A行が厳密に減るため各blockの再実行は最大1回。
保持量はO(ceil(N/K)M + KM + N + M + 出力長)、ただし上流のnear_pairs/全辺Vecを残すと削減にならない。
同一TXの入力から決定的に辺を再列挙できる行producerが採用境界となる。
B圧縮を別に取得できなければ列挙passも必要。1億辺の処理時間/TX寿命は未解決で、
この監査は実装・oracle一致・性能確認や採用判断ではない。
### R1 検索prototype sourceと実行前レビュー

Solが `target/review-fixes-bench-20260908/mih_radius32.rs`、`export_delta.py`、`PROTOTYPE.md` を作成。
この時点はPython syntax/rustfmtだけで、DB export・rustc・self-test・実計測は未実行。
exportはコピーDBをmode=ro/query_only・同read TXで読み、出力はxbで新規作成する。
親の確認でbase identity取得を全222MB読込から96byteだけのreadへ縮小した。

独立Astraはkernel/delta last-write-winsの読取に誤りを認めなかったが、実行前の検証補強を要求した。
現合成差分は先頭64bit内だけなので、残りの完全一致blockがprobe漏れを隠す。
全16block各2bit、およびj=1..15でblock0=3bit/j=1bit/他14block=2bitの32境界、
block0=3bit/他15block=2bitの33除外をbruteと比較する。
実queryでもitem_id全集合をbrute比較し、delta更新/削除/追加/同ID連続変更は既知期待値で別検査する。

現prototypeはdelta適用後に全体を再構築したsingle MIHで、base/delta別索引やcompaction性能ではない。
メモリ集計はposting/offset/marks以外のsnapshot/index records・構築一時領域も区別して記録し、
採用には実peakを測る。radius<=32の制約を明記する。product変更や採用を意味せず、portableには含めない。
### 初回portable更新完了・実機確認依頼

`build-portable.ps1 -KeepRunning` はexit0。core release build9分15秒、remote0.40秒。
更新直前の `target/r2-portable-update-preflight.json` で、fresh package24files/440920142bytes、
userdata/reparseなし、コピー先対象/親reparseなし、対象processなしを再確認した。
`update-portable-dev.ps1 -SkipBuild` はexit0 (`target/r2-update-portable-dev.log`)。
`target/r2-portable-hash-verify.json` は全24filesのsize/SHA-256一致、mismatch0。
既存data/data-remoteは保持、agentによるアプリ起動はしていない。

起動先: `C:\home\mimageviewer-dupe\target\portable-dev\mimageviewer.exe`。
coreは94249472bytes、SHA-256 `ea4cf7a17d64609bf83056db44bb52ebc9e1e264065508c81d57973a67554804`。
zipは264631167bytes、SHA-256 `fea8d788b9247e2b4d8f7dff0c039ea58d3e96baa183e36cd8e54f44e0ce1474`。
後続buildから独立して保存するため、同zipをcheckpoint v2内の `r2-portable-verification.zip` に複製し一致確認した。
source・成功log・binary hashはcheckpoint v2のmanifest.jsonで照合できる。

利用者へ起動コマンドと、右パネルの場所間移動/別窓/ZIP/PDF、静止画ウィンドウ/全画面切替の確認を依頼した。
R2の実機確認回答とcommitは未完了。初回版にはR4/R8・R1/R5/R6の修正を含めておらず、作業は継続する。
### R1 検索prototype v2の限定検証完了

`target/review-fixes-bench-20260908/prototype-self-test-v2.log`、`mih-metrics-v2.json` と
sourceを独立Astraが照合し、新規P1/P2なし。前回のoracle不足2点も解消した。
既知binary fixtureは実load_deltaと同じdecoderを通り、seq11..15の更新/削除/追加と
同IDの連続変更をsignature/quality/revisionまで検査する。
実oracleは全体に分散した8件とdelta由来の分散8件でbruteの全item_id集合と一致した。

9852件のdelta適用後の有効4631165件を単一MIHへ再構築し、build606.306ms。
2048署名のp50 2.5465ms、p95 3.2077ms、最大6.9884ms。これは単署名の検索時間であり、
本照会全体の時間や音声への影響を示すものではない。品質正値は4621867件。
snapshot records約222MBとindex records約222MBを二重保持し、posting約296MB等を別途使用する。
allocator・構築scratch・process peakは未測定。split base/delta、compaction、同TX common、
全book/alignment、取消とviewer間公平性は製品採用前の後続gateとして残る。
この試作はignored target内のみで、初回portable成果物にも製品コードにも含めていない。

### R4/R8 実装前再照合と描画consumerの補足

Solは現在のApp-global SimilarPeekが通常比較slot/modeを書き換え、panel末尾だけでheld終了を
判定する根因を確認した。viewer-ownedの一時gesture/asset/requestへ分離する合意設計で進める。
独立Astraの棚卸しで、single_transform以外にもcapture-region、loupe、continuous調整outline、
original-preview indicatorが元pageを読むと確認。本文と同じ一時表示の選択に従わせる。
既にactiveのloupeも元pageを候補上へ重ねない。操作開始時は一時表示を解除し元geometryへ合流する。
navigatorのPendingCenter/PendingPanTransitionも新pressなしにfs_panを書けるため、
候補のpaintと通常interaction consumerを分離する。

提示完了の契約は「候補を元pageとしてpublishしない」とする。候補からemit-ready、
fs_painted_lastのSome、page layoutを発行しない。一方prepare_fullscreen_state等で元画像の
実texture消失に伴うNone無効化や準備処理は継続し、元ownerの状態機械を凍結しない。
この明確化に親と独立Astraが合意した。
明示parkのmain/detached両経路はreset_detached_pause_foreground_modesの後にsnapshotする。
この既存境界で一時表示を解除し、raw mount/swapへの取消副作用は追加しない。

### R1 候補集合の維持

現query_book_readyの候補条件は、commonでない起点ページから得た近傍辺数の合計>=3であり、
最終alignmentのmatched>=3やdistinct起点ページ数とは異なる。UIはUnrelatedも部分一致として表示する。
修正時に候補集合を不用意に縮小しない。各起点の適格な近傍をcandidate bookごとに3で飽和集計し、
その起点が非commonの場合だけ本別合計へ加える。commonは起点の本も含む9冊を確認して確定する。
候補側commonや最終alignmentでdiscoveryをfilterしない。1起点×3候補ページと3起点×1候補ページの
双方が残ることを回帰対象にする。同署名の起点反復は、非common確定後に出現数を掛けて集約可能。
飽和はdiscoveryだけに使用し、alignment/strip用の全辺は固定TXから厳密に再列挙できるよう保持する。

### R1 不変base/deltaと派生MIHの所有・世代選択

独立Astraがsrc/similar_search_array.rsの実型と照合した。book worker内の派生cacheだけが
SearchSnapshotとbase/delta postingsを保持し、postingsは元配列の行位置を参照する。
BaseArrayへのMIH埋込みやrecords複製はしない。単体画像照会とそのcache/workerは変更しない。
baseは品質正値を登録しても、品質0への変更・削除を含めsuperseded maskを常に適用する。
deltaは最終Live品質正値だけ。dedupe marks・候補buffer等はworker private scratchとする。
待機要求/完成UI結果にsnapshotを保持させず、workerの派生cacheは現在1組に有界化する。
scopeはpostingへ焼き込まず、同TXの行適格性検証で適用する。

ItemChangeBatch自体にはstore_idがない。適用前に必ず同TX storeとの一致を検査する。
shared/private候補はstore一致とbase_seq<=snapshot_seq<=TX read_seqを先に確認し、
(base_seq,snapshot_seq)の降順で選ぶ。同順位なら既存MIHを再利用できる候補を優先する。
postings再利用は数値の一致でなく保持中Arc<BaseArray>同一性で判定する。
private=(base0,snapshot200)、shared=(base100,snapshot180)、TX201ならsharedを選んで180→201を追随する。
snapshot seqだけを優先するとprivate旧baseが毎回最新へ進み、compactionへ移れなくなる。
未来snapshotは最終deltaから巻き戻せない。履歴不足等は同TX公開行からmemory-only baseを作り、
そのprivate snapshotを後続の追随元として再利用する。queryはbase永続化/履歴pruneを行わない。

新baseへの切替が確定したら旧MIHをworkerで解放してから新MIHを構築する。
旧base recordsはitem queryやcompactionが保持し得るため、アプリ全体のpeakが1世代分とは保証しない。
構築途中のMIHは公開せず取消時に破棄する。完成base MIHはorigin/scope変更後も再利用可能だが、
結果/common/TX文脈は失効させる。store変更/shutdownではworker上でcacheを退役させる。
新旧postingの二重保持回避、初回/compaction/復旧peakは実測が必要で、実装・採用の完了ではない。

### R1 全corpus意味論のoracleと規模制限

旧256件打切り/候補側skipのqueryはoracleにしない。独立bruteで同TXの適格全体を走査し、
候補本別の3飽和集計からcandidate key全集合を照合する。最終matched数で代用しない。
分類はorigin/candidate全pagesに、common=trueの各対象ページの実近傍9冊分のwitnessだけを
加えたcertificate corpusを旧classify_pairへ渡す。falseの代表追加は不要と独立Astraが証明した。
certificateはglobalのsubsetなのでfalse commonを新たに作らず、trueは9冊の証拠で維持する。
witness自身のcommonは証拠能力へ影響せず、再帰closureは不要。実container→book IDは一意に割り当てる。
対象の品質0行・実page_index・A/B向きを保持すれば、対象間の全辺、分母、alignment同点、
matched/coverage/relationが一致する。ページ帯も同じ実行の独立対応値と照合する。

対象Lページに追加witness<=9L、certificate<=10L。ただし旧分類器は全近傍辺を保持するので、
2冊各10000同署名ではcertificateが20000行でも約2億辺を作る。巨大旧oracleは実行しない。
通常/短い実本はcertificateによる全フィールド比較、denseは小規模の網羅/反復/同点oracleと
10k級の証明済み構造の期待値および負荷計測を分ける。同一TX内の同署名brute結果は再利用可能だが、
非common確定には全走査が必要。これはoracleの設計合意であり、実行完了ではない。

### R1 実本ケースの形状調査

親はコピーDBをmode=ro/query_only・同read TXで照合し、
`target/review-fixes-bench-20260908/book-case-shape.json`へ50ケースを記録した。
Completeの400ページ全24冊、10000ページ全8冊、kind別の短い本18冊を選択した。
read_seq9852/page_order_version1。最大8冊は品質正値10000行、異なる署名9197〜9788、
同一署名の最多反復3〜10。dense合成だけでなく高いunique署名数の負荷を確認する必要がある。
これは署名/ページ数の形状調査だけで、照会・分類・性能テストではない。
旧1.69秒計測の400ページ/7候補の具体keyは現記録から未特定。条件の同一性を推測しない。

### R4 第1段owner APIの独立レビューと修正指示

新規similar_preview.rs作成後、UI接続/compile前に独立AstraがAPIを先行レビューした。
gesture（表示権限）/cached（有効資源）/worker（未完了処理）は独立寿命として分離可能だが、
releaseをworker取消と同一にしていた点を修正する。release/focusは表示だけ終了し、有効な準備完了は
非表示cacheへ保持する。page/park/close等のsource/session失効はcancel→Drainingで旧結果を拒否する。
失敗済み要求はIdleと区別し、新pressの明示retryを定義する。cache hitでも古worker/nextを調停する。
File/ZIPのcanonical source_dimsをGPU縮小後pixels.sizeと区別して保持する。
新preview uploadはArc<ColorImage>をload_textureへ渡し、UI上の不要な全画素cloneを避ける。

freshnessは親/独立Astraで次の境界に合意した。
indexed hit stampとworkerが実file/outer ZIP/PDFから得るobserved stamp（modified SystemTime+len）を分ける。
decode前後のobserved一致を検査し、再pressは同じ有界workerのValidateOrPrepareで確認する。
同一ならtextureを再利用し、変化ならcurrent sourceを再decodeする。cacheの存在だけで表示を許可せず、
当該press/requestの確認成功後に表示する。metadata失敗/前後不一致はFailedで、旧画像へ戻さない。
release後completionはcacheへ採用可能だが、source/session失効後の旧completionは採用しない。
PDF認証の現revisionをpredrawと完了採用時に照合する。workerへ渡すPdfPasswordStoreは値cloneである。

既知候補更新はmetadata panelがfresh Readyを受領した時、同target/keyのindexed stamp更新をownerへ渡す。
Preparing中のlast_ready表示や、候補が結果から消えたことだけを更新/削除通知にしない。
hidden中のDB照会追加、global memory_epochによる全viewer失効、UI同期stat、watcher新設はしない。
保証境界は既知更新/現認証revision/新press/decode前後。metadata同値を内容hash同値や常時外部変更検出とは謳わない。
この節は修正指示・設計合意であり、実装/自動検証完了ではない。

### R4 owner改訂の再レビューと初回compile

改訂APIと7回帰を作成後の先行再レビューで、独立Astraが3件P2を確認した。
Ready中も現credentialを確認すること、Drainingの旧pendingをactive freshness対象から外すこと、
cache A更新で無関係Bのrequest/gestureを消さないこと。いずれも修正対象とする。
要求正本はRunning.request/Draining.next/Failed.request、cacheは独立resource。
既知更新は一致するcache/gesture/要求だけを失効し、viewer全体generationはsession/park/close等に限る。

独立coreはZIP/PDF各pageのindexed mtime/sizeがouter FileCandidate由来と確認した。
同candidateの変更に加え、正規化resource_pathが同じでouter versionが違う別entry/pageも対象にする。
別physical sourceと、既に新versionに対応する資源は保持。認証revisionは同PDFの全pageに関係し、
viewport等の描画条件差とは分ける。新watcher/同期stat/全体epochは追加しない。

`target/r4-similar-preview-state-tests1.log` はexit101、テスト実行前のcompile失敗。
新fixtureのitem_idへi64のmtimeを代入した型不一致と、存在しないMatchBand::Sameの2診断。
製品型を拡げずfixtureを実型へ合わせ、上記P2の回帰とともに別ログで再実行する。
この時点ではR4テスト成功・renderer接続・portable再buildは未完了。

### R4 owner state test run2成功

`target/r4-similar-preview-state-tests2.log` はexit0、10成功/0失敗、compile1分57秒。
Ready後credential変更、旧drain中のnew Ready反復、cache A更新時の別source B維持を含む。
release hidden-cache、新press validation、focus復帰非再開、失敗retry、session ABAも成功した。
実metadata前後検証と同outer ZIP/PDF別pageの回帰を補強後、第1段の独立再レビューを行う。
現時点はowner/pure assetの検証であり、renderer/lifecycle接続の完了ではない。

### R4 owner追加レビューとrun3 compile

driveを落とすpath_key::normalizeをsource identityに使う新P2を独立Astraが検出。
索引と同様にdrive/UNCを保持する正規化へ直し、C:/D:の同階層同名の非干渉を回帰へ追加する。
2viewer回帰はBが空ownerだったため、Bも実pendingとcancel tokenを持たせ、
B dropでBだけ取消・A非取消とA完了採用を検査するよう補強する。
PDFはclamp前render寸法保持まで確認。page box aspect/native raster寸法/Vectorのpixel基準の
具体契約はrenderer接続前に確定する必要があり、PDF描画全体の完了とは扱わない。

`target/r4-similar-preview-state-tests3.log` はexit101、追加fixtureのcompile失敗3診断。
存在しないItemKind::ZipImageと、source_version test helperが本体関数を隠した型不一致/unwrap不存在。
テスト実行は未到達。fixture修正と上記補強後のrun4を別ログで行う。

### 設計概要の旧索引記述の訂正

architecture-overviewのrowid/key hash・44byte similar.compact・1件cache・終了時のみ再読込の記述を、
現行の明示item ID/revision・48byte similar.base/base-delta・6件cache・array worker追随へ訂正した。
独立coreが既存コードと照合。header applied_seqは保持されるがread_baseでDB最新seqと照合しないため、
検証対象の説明から切り離した。本文SHA-256がheader seqを保護する保証も記述しない。
R1の未採用MIH等は現行architectureへ混ぜず、修正計画への参照に留めた。


### R4 owner第1段の検証・独立承認

`target/r4-similar-preview-state-tests4.log` はexit0、15成功/0失敗、compile1分53秒。
実metadata同値のcache再利用、変更後の再decode、decode前後変更の拒否、同outer archiveの別page、
C:/D:の非干渉、双方が実workerを持つA/BでB dropがAを取り消さないことを確認した。
独立Astraはowner/pure asset段階を承認し、追加P1/P2なしと報告した。
`target/r4-owner-stage1-20260908/manifest.json` とpatch/new moduleを保存した。
基準は79d30b23eとR2 staged index。R4 tracked patch SHA-256は
`e6b1b6455b738280009acd3d073497ad39e96ee7316d179b7c55205719852a82`。
これはrenderer、入力・park等のlifecycle、PDF geometry、全体gateの完了を意味しない。

### R4 共通geometry抽出の実装前合意

親・実装Sol・独立Astraは、identityを持たないDisplayedImageGeometryへ数学/paint geometryを集約し、
既存DisplayedImageTransformはpage_idxとgeometryを保持するwrapperにする境界で合意した。
既存のflat input APIと読み取りを維持し、immutable Derefだけを実装する。DerefMutは追加しない。
唯一の外部source_size書込はcapture用座標変更なので、明示的with_coordinate_source_sizeへ移し、
矩形・UV・倍率を再計算しない。wrapperのtranslated_byはpage_idxを保持したSelfを返す。
候補画像はGeometryを直接使い、偽page_idxや元ページのlayout/cache identityを借用しない。

base resolveのfit/Original/no-upscaleは従来のtexture_size基準を維持する。
canonical 100%とPDF aspectは既存resolve_fs_image_transformのidentityなし抽出に集約する。
layout長辺をsource長辺へ正規化し、Proportional中間枠、texture contain、最終pixel snapの順を保つ。
PDF Rasterのsourceはcanonical native寸法、Vectorはrender寸法、layoutはpage box aspect、
textureはGPU実寸法として分離する。RasterのGPU clampもcanonical layout経路を使う。
通常・連続・別窓・captureを含む共有機構のため、wrapper移動/座標変更と既存geometry回帰を検証し、
最終full gateで統合する。API合意時点ではこの段階の実装・テストは未完了。


### R4 共通GPU paint/cacheの実装前合意

FullscreenPaintSourceIdをPage(usize)/SimilarPreview(u64)に分け、既存page constructorはwrapperで維持する。
resourceのResampleable/Lanczos変換、cache key、fallback key、同source判定、旧source掃除の全経路で
variantを保持する。page_idx()は既にOptionなので候補はNone、postfilter None/trace falseとする。
page由来のinput_generationを借りず、preview assetのidentity/generation/TextureIdを使う。
同値のValidate完了では資源IDを保持し、新Preparedだけ新IDとする。pressごとにcacheを捨てない。
fs_lanczos_cacheはViewerContextBundleでSimilarPanelStateと一緒に移動するため、viewer-local IDでよい。

retain_page_indices/remove_pageはPageだけ、preview専用retainは候補だけを掃除する。
releaseでcached assetを保持する間は派生cacheも保持でき、失効/置換で旧候補IDのentriesと
limit_fallback_sourcesの双方を退役させる。後者の無上限HashSetへ旧IDを累積させない。
outputsはtyped sourceを返し、VRAM全体集計(None)には両variant、ページ別予算(Some(indices))には
Pageだけを含める。候補の元textureもowner cached assetから全体集計へ含める。
候補を架空pageへ帰属させず、at-rest viewerの資源も集計対象とする。
同数値ID/同TextureId/同世代でもPageとPreviewが異keyになること、双方の退役非干渉を回帰対象にする。
親・独立Astraが既存consumerを照合した設計合意であり、この段階の実装検証は後続。


### R1/R5/R6 実装区切りと依存境界の先行棚卸し

独立coreが製品callerを再確認した。ui_metadata_panel→App::query_similar_book→managerの1系統。
実装は(1)viewer client/fair worker owner、(2)同read TX helpers、(3)worker-private MIH、
(4)common/discovery/classifier・stripの4区切りで進める。単体画像照会/cacheには混ぜない。
clientはSimilarPanelStateのdefault可能なfieldとし、park/swapで退役させない。
query_bookがpublicを維持する場合、引数clientも公開可能なopaque型にする。
DBはcommit結果の値を返しschedulerが通知する。DBからquery ownerへ依存させない。
workerからmanager/schedulerへの強参照循環や、ConnectionとTransactionの自己参照構造を作らない。
readerはworker localで所有し、BookReadSnapshotはread TX期間だけ借用する。

ページ順修復はworker_loop→repair_page_order_if_stale→renumber_container_pages。
DB側のtransaction.commit成功後、helperは変更なし/commit済みを返し、schedulerが直ちに通知する。
containers==0でもorder-version更新がcommitされ得るため、件数を通知省略条件にしない。
現在のBookQueryState::Idle代入はscope変更・run終端・array公開に分けてhard/soft通知へ移行する。
この棚卸しはread-onlyであり、新query ownerやTX実装の完了ではない。


### R4 handler回帰の入口

独立UIは既存harnessを確認し、実装担当へ次の4境界を共有した（テスト実行は後続）。
setup_fullscreen_fixed_key_testとviewport focus入力でhidden release/focus lossを実predrawへ通す。
spread/navigator既存texture fixtureで実pair解決と共通paint selectionを通し、LTR/RTL本文・overviewを検証する。
実open_fullscreen(second)のpark経路で候補のsnapshot混入と遅延完了の復活を防ぐ。
navigatorの実input handler、およびcapture_region_target_at/cropで操作開始後の元geometryを検証する。
metadata_panel_similar_results_dark等はパネル単体fixtureなので、このlifecycle回帰の代用にはしない。


### R4 geometry/GPU製品差分の先行実装レビュー

独立Astraが79d30b23eとR2 stagedの上のgeometry/GPU/accounting差分を確認し、追加P1/P2なしとした。
数学式、immutable wrapper/page identity、cache/fallback/familyのtyped identity、双方向退役、
VRAMの全体/ページ予算投影は合意した境界を維持している。
`target/r4-geometry-check1.log` はFinished dev 36.00s、
`target/r4-geometry-gpu-check1.log` はFinished dev 31.75sを確認した。
追加回帰の結果は後続。preview retainの製品caller、元textureのmounted/at-rest計上、
candidate generation/paint/input取消は未接続であり、このレビューはR4全体の完成承認ではない。


### R4 geometry/GPU狭域回帰の成功

`cargo test -p mimageviewer --features pack-build-tools --lib` のfilterとして
page_wrapper_and_identity_free_geometry_resolve_to_the_same_result、続いて
page_retention_and_preview_retirement_do_not_cross_source_ownersを実行した。
`target/r4-geometry-test1.log` は1成功/0失敗（compile1分55秒）、
`target/r4-gpu-retire-test1.log` は1成功/0失敗（warm compile0.67秒）。順次実行sessionはexit0。
既存geometry/GPU module回帰とfmt、renderer接続は次の区切りで確認する。


### R4 geometry/GPU共有回帰とcheckpoint

`target/r4-displayed-transform-tests-stage1.log` は22成功/0失敗（compile1分29秒）、
`target/r4-gpu-lanczos-tests-stage1.log` は36成功/0失敗（warm compile0.63秒）。
初回fmt checkは整形差分でexit1。cargo fmt適用後のfmt checkとgit diff --checkはexit0と実装担当が報告。
成功fmtはstdout/stderr 0bytesでTeeがlogを作らなかった。command/exit0の記録は
`target/r4-fmt-check-stage1-v2.meta.json` に保存され、親がmanifestへ取り込んだ。
整形は新R4 hunksのみで、R2 staged8files +1632/-79を保持した。
親はsource編集停止中に `target/r4-geometry-stage1-20260908/` へpatch/new module/manifestを保存し、
R2 staged patch SHA-256が最初のportable成果物と一致することを再照合した。
共有部の独立レビュー・回帰が済んだ区切りであり、renderer/lifecycleと新portableはまだ未完了。


### R4 候補表示の一時pan補正

元画像用のpanを候補へそのまま適用すると候補がviewport外へ出るため、親・独立UIは
候補をresolveした後、既存pan_correction_for_minimum_overlapの補正を候補専用local panへ加え、
元GeometryInputから再resolveする境界で合意した。元fs_panへ書き戻さない。
候補はrotation0/free0/bboxNoneなのでfull_image_rectを補正の入力に使える。
各軸min(48 logical points,画像幅,viewport幅)の既存最小重なりを維持し、最終snapの約0.5 physical px差を許容する。
snap済rectをfrom_resolved_rectへ再入力せず、補正0なら元geometryを返す。
translated_byはviewportを移さないが、placement.panを更新せず物理整数px移動の契約なので、
任意補正には使わない。この節は設計合意で、実際の候補描画回帰は後続。


### R4 編集操作との逆順とcaptureの入力所有

独立UIのsource照合で、capture-region/loupe/通常adjustment中もmetadata候補へ到達する。
erase/conceal/text/localadjust/exportcrop/SNS/viewtrim/analysis中は既存UIがmetadataを表示しない。
長押し開始を共通entryへまとめ、競合する一時captureだけを終了してpreviewを開始する。
loupeの固定設定やadjustment mode/paramsは保持し、候補表示中の元page overlayを外す。
調整・canvas操作開始時はpreview終了後に元pageへ合流する。park用resetの流用でpin等を消さない。

capture overlayはpanel描画より先にprimary pressed/released/downと最終hover位置を読む。
Release(canvas)→Press(panel)が同frameに入ると、正当な先行releaseのcropを後続panel位置で上書きし得る。
frame全体をpanel pressで取消す案も、先行の正当なコピーを失うため親の確認で棄却した。
親・独立UIは、既存selectionをPointerButton(pos,pressed)/PointerMovedの順に更新し、
releaseの座標で一度だけframe-local Copy/Cancelを出して終端するreducerへ移すことに合意した。
canvas所有のreleaseはpanel上でもclampして確定する。panel所属pressはその時点で残るselectionだけ取消す。
wait_for_releaseも最初の該当releaseで解除し、同frameの後続pressを処理できるようにする。
terminal後はcaptureだけ処理を終了し、metadata側のeventをconsume/removeしない。
集約primary_down/final hoverによる後書きはしない。新しいApp bool/Optionは追加しない。
panel所有の判定は当該frameのhover latch/実効visible/seek-height込みrectをdrawと共有し、
古いframeのhover/表示boolや、clipboard worker開始後のresetでは代用しない。
これは通常captureにも届く共有入力境界の修正であり、両順序、wait解除同frame、canvas→panel releaseを回帰対象にする。
実装前の構造合意で、実行結果は後続。renderer/lifecycle初回checkは
`target/r4-renderer-check1.log`、cargo check -p mimageviewer --bin mimageviewer-core、exit0/30.21秒。


### R4 renderer/lifecycle先行レビューの3指摘

基準d0784377ffとstaged R2の上のR4差分を独立UIが確認し、次のP2を共有した。
調整panelのopenだけでsession=Noneにすると表示中の長押しbuttonが無反応になる。mode/paramsを保持する契約へ戻す。
候補frameで元page layoutをclearする一方、navigator入口はそのlayout無しでreturnするため、
候補navigatorへの最初の操作が捕捉されずcanvasへ漏れる。既存interaction activeの検査だけでは足りない。
候補navigatorの操作edgeを入口で所有し、gesture終了→元geometryを同frameに解決→同eventを既存handlerへ渡す必要がある。
元geometryの最小共有resolver境界は実装担当が確認し、page偽装/1frame delay/layout温存sentinelを追加しない。

parkはowner invalidateだけで、preview outputの退役がpredraw内にしかないためAtRest bundleに旧GPU資源が残る。
session=None分岐はDrainingをpollせず毎frame再invalidateし、遅延completion/pixelsを回収しない。
共通終端でpreview-only GPU退役を揃え、表示可否と独立した非blocking terminal drainを所有contextへ接続する。
Page/通常GPU cacheをclearせず、有界workerの契約をreceiverの無断detachで崩さない。

肯定確認: 本文/専用navigatorは同assetとcandidate geometryを使い、元page layoutや提示完了/traceへ混ぜない。
元prepare_fullscreen_state/PDF準備、通常比較pin/mode、temporary local-pan clampは維持する。
mounted/AtRestのsource texture計上と全体/ページ別output予算も確認済み。
新3P2、capture ordered修正、handler回帰が残るため、R4全体の承認は保留。


### R4 終端・capture接続のcheckとowner再回帰

`target/r4-terminal-capture-check2.log` はcargo check -p mimageviewer --bin mimageviewer-core、exit0/16.31秒。
atomic preview失効、AtRest bundle直接poll、capture ordered処理を接続した段階のcompile確認。
`target/r4-similar-preview-state-tests5.log` はowner15成功/0失敗、compile1分48秒。新handler回帰は後続。
独立UIは、ROOT更新がactive detachedのmountとshow_viewport_immediate callbackまで同期実行すると確認した。
完了wakeはROOTへ一度だけ送り、古session viewportへの追加通知はしない。
全context pollはeframe::App::updateのupdate_frame前へ置き、early returnや描画可否から独立させる。
新timer/loop、pollのためのraw mount/swapは追加しない。

### R4 navigatorの同frame入力所有（設計合意）

元画像を裏で通常paintし、全面黒と候補で上描きする案は棄却した。提示通知やContinuousの状態更新、
背景と描画コストを候補中へ混ぜるため。試行2hunkは実装担当がexact reverseで戻し、親承認前の新設計編集を停止した。
Single/Spread/Continuousのpure frame plan抽出も調査したが、既存input/実paint layout契約を維持できる次案で合意した。

現image_rectとgeometry非依存fs_navigator_panel_rectから入力所有をframe-local typed予約で確定し、
preview gestureを終了する。予約済み入力をcanvas/touchへ渡さず、元rendererを一度だけ実行する。
本文・holdover・通常navigatorの描画と既存pending消費が完了した後、予約nav操作を同frame内でmodelへ適用する。
pan等の表示更新は通常repaintで行う。要求を次frameへ保存せず、新App pending/sentinel/偽page_idx/二重paintを使わない。
元geometryが未準備等で得られなくても、そのframeで入力所有を終端しcanvasへ漏らさない。
nav外枠は画像aspect非依存で、aspect差による外枠不一致の懸念は独立UIが実コード照合後に撤回した。

既存handlerはframe冒頭interactionをsnapshotしており、Press→Release同frameで新Pan/Selectが残る。
そのため単に後段で同handlerを再呼出しせず、normal入口と予約入口が同じordered reducerを使う。
peekのrelease→navのnew pressと、navのpress→releaseを所有順で区別する。既存Pan/Select/PendingCenter/
PendingPanTransitionの意味を保持し、primary/secondary、header操作一度、capture/editorへの二重配信を回帰する。
これは親・独立Astraの設計合意であり、実装担当はAPIの前提を検証してから進める。


### R4 終端・capture・adjustment接続の再レビュー

独立UIはnav未完成部分を除いて再確認し、追加P1/P2なしとした。
captureはrelease.event.posで確定して即終端し、後続panel eventを残す。逆順panel pressはcopy前に取消し、
canvasから始めたdragはpanel上releaseでも確定する。wait_for_release解除後も同frame後続pressへ進む。
共通invalidateはownerとpreview-only GPU退役を揃え、App::update冒頭の全context pollはAtRestへ直接作用する。
ROOT一回の完了wake、Draining成功/切断の資源破棄とIdle/最新1件遷移を確認した。
sessionからadjustment/captureのblanket拒否が除去され、begin entryはcaptureだけ退役しpin/params/loupeを保持する。
前節の3P2のうちこれらの接続はコード再確認済みだが、nav修正と新実handler回帰の検証は残る。

### R4 navigator回帰の中間結果と追加レビュー

`target/r4-nav-check2.log` は cargo check -p mimageviewer --bin mimageviewer-core、exit0/29.54秒。
check1のmatch arm型不整合2件は修正した。ordered primary/secondary、旧peek release→新nav press、
元layout不在の予約終端、header/wheelの狭域4テストは各1成功。header初回失敗は証跡を残した。
ただし独立UI再レビューでは、最終pointer位置による先行guardがcanvas内press→panel外releaseを捨てるP2と、
header action先処理が先行する範囲選択releaseの確定を捨てるP2が残る。既存Responseとraw action併用は
複数corner clickで余分なsettings保存を起こし得るため、最終値だけのテストでは「一度」を証明できない。
共通ordered ownerと単一副作用出口へ修正し、操作順と副作用の回帰を追加する。中間PASSを最終承認としない。

### R4 native textureのpaint batch寿命（追加欠陥・設計調査中）

候補をpaint commandへ追加した後、同frameでpark/page/closeによりcacheを退役すると、
UI closure終了で最後のLanczosOutput Arcがdropし、実egui render前にnative TextureIdが消える経路を確認した。
NativeTextureIdLease::drop→RendererTextureIdReleaser→renderer.free_textureは即時登録削除であり、
既存textures_delta.freeのrender後解放とは別経路。meshはTextureIdだけを保持するため、frame-local Arcでは足りない。
独立UIも既存の自動遅延解放がないことを確認し、このままでは承認不可とした。
previewだけでなく共有typed paint resourceの通常/holdover/frozen各paint入口を棚卸しし、
実paint outputがnative登録を所有する方式を調査する。任意の1frame遅延、App pending field、
viewportを無視した一括解放queueにはしない。現時点では修正済み・実機検証済みとは扱わない。
### R4 native texture寿命の構造合意

親・独立coreはmain/immediate paint順を照合し、FullscreenPaintResourceのpaint境界で同painter/同clipへ
有効なno-op egui_wgpu::Callbackを添え、不変Arc<LanczosOutput>を実paint outputに保持させる案で合意した。
rendererはprimitivesを借用し、update_buffersのwrite guardとrenderのread guardは局所で解除される。
primitivesは呼出元でpaint完了まで生存し、surface不在/取得失敗は未描画outputの破棄で資源を終端できる。
callback内でArcをtake/dropするとrenderer再writeのdeadlock、callback_resources保存は循環所有になるため禁止。
通常/holdover/候補のpaint_texture_id群に加え、id()を使うfrozen window/continuous pageも共有境界へ統合する。
診断用ID取得とpaint時の寿命所有を区別する。vendorやviewportの生成/選択は変更しない。
private callbackのArc<dyn Send + Sync>にproduction outputとtest CountingReleaser leaseを載せ、
実Context::run→tessellate→primitives破棄、未描画output/空clip/2batch独立をGPU不要で回帰する方針。
これはpaint outputのRAII所有を確認するテストであり、実GPU paint/renderer lockの実行検証とは区別する。
### R4 nav追加レビューでの設計訂正と再lock欠陥

nav-p2-check2は33.36秒でcompile成功したが、nav-p2-tests1でContext RwLockの10秒deadlock失敗を検出。
ctx.data/data_mut closure内のctx.viewport_id呼出しが再lockする製品コードの欠陥であり、テスト揺れではない。
viewport/frame/keyをclosure外で確定し、新R4の同種呼出しを棚卸しする。失敗ログは保存する。
前回の内press→外releaseとheader先処理の2P2、およびResponse/raw actionの重複は独立UIが解消を確認した。
ただしwheelのprevious pointer更新がlayout成功時だけで、候補中のMove-only frameを記録せず、次frame wheelを
誤配信する問題を追加検出した。PointerGoneの失効も含め、描画可否に依存しないviewport入力境界へ移す。

親・独立UIが以前合意した「reserved handlerを通常nav paint前」はPendingCenter consumerを取り落としていた。
Select確定→新zoom/panとPendingCenter→直後の古geometry navがpending消費、という順でpanを二重加算する。
最小接続を本文/holdover/通常navigator描画後、capture/editor入力前へ訂正した。予約は同frameで一度適用し、
既存PendingCenterは次の本物の新geometryで消費する。新timer/pending/遅延frame sentinelは追加しない。
既述設計段落も訂正し、handler-onlyではなく実frameのproducer/consumer順を通す回帰を必須とした。
### R4 pointer snapshotの入力入口合意

handle_fs_wheel_and_clickの冒頭だけでは、ROOT grid、ZIP/PDF deferred、native backdrop、park中の
同detached viewportのframeを覆えないと独立UI・親が確認した。snapshot作成をegui on_begin_passへ置き、
各viewportのbegin-passごとに新入力を記録し、同pass内のhandlerは読取りにする。既存libの入力plugin群へ登録。
eguiは内部Context write lockを解放してからhookを呼ぶ。viewport/frame/keyは各ctx closureの外で確定する。
同frame再利用、PointerGone→None、初回未知Noneを未来のfinal positionで補完しない契約を維持する。
必要なPointerMoved/Button/Gone/MouseWheelのみを記録し、Paste/IME/ファイルdrop payloadは複製しない。
raw input/key/IME自体は消費・改変しない。hookはtest ctxにも設置し、grid/passive gap→Wheel→Move、
Move-only→次frameWheel、初回/Gone後の未知位置を実複数frameで回帰する。
GPU callbackはrectだけRect::ZERO、clipはmeshと同painterとする。現epaintはZERO callbackも保持し、
rendererの不要viewport切替は省略する。mesh batching分割は残るため無コストとは扱わない。
nav-p2-tests1の最終結果は30成功/11失敗、112.22秒。Context再lockの失敗ログを保持した。
今回のtimeoutはコマンドrunnerによる中断ではなく、egui DEBUG PANICによる再lock検出である。
修正後の再実行と実frame回帰が終わるまで、この段階をgreenとは扱わない。
### R4 nav tests2とviewer所有の追加欠陥

nav-p2-check4は29.12秒で成功。限定UI再レビューはContext再lock、全viewport begin hook、未知位置/Gone、
nav描画後reserved適用のコード修正を承認した。nav-p2-tests2は41成功/2失敗、3.12秒。
失敗はHeader導入後の旧global state assertionsであり、所有設計の確定前に機械的追随はしない。

親・独立coreは、既存flat/panorama interactionの固定egui temp IDが全Context共通であることを確認した。
AのPendingCenter→B activate/mount→B geometryの同page_idxでconsume/pan変更しglobal entry削除、
またBが対象外modeのhandlerでAのentry削除、という経路がある。bundle swap/明示終端に退避保証がない。
viewport suffixだけでは同viewport別viewer mountを覆えず、flat/panoramaの互いに排他的な操作を一つのtyped ownerにし、
Appのmounted field/ViewerContextBundleに移す方針で合意。egui global tempに二重の正本を残さない。
入力観測はviewport所有、操作意図はviewer所有と区別し、raw mount/swapはpayload交換だけにする。
page/source/真のcloseで対象ownerを失効し、parkのpointer gestureと確定済Pending*の扱いは別途コード照合する。
A→B→A、同viewportのcontext交替、flat/panorama別viewer、B変更でAが不変という回帰を要求する。

### R4 multipassと入力cache回収の設計訂正

frame_nrはegui runの全pass終了後だけ増えるため、第1passの入力をframe単位で再利用する以前の合意は誤り。
第2passのRawInput.eventsはtake済みで空だが、旧snapshotを再生するとwheel/headerを重複適用し得る。
begin-pass hookは毎回そのpassのsnapshotを作り、同passのconsumerだけが共有する契約へ訂正した。
pass番号も同ID viewport再生成時には0へ戻るため、hookのrefreshを番号等価で省略しない。

新viewport-keyed tempの自動GCはなく、closed IDごとの最後のVecが残る。single typed snapshot mapで所有し、
on_begin_passのcurrent_pass_index()==0時だけbackend RawInput.viewports+currentのlive集合で回収する。
rootの第1passでchild生成→discard→第2passの古いviewport集合でchildを消さないため、第2passでGCしない。
本番runとheadless begin/endの双方でpass indexが成立することを独立coreと照合した。
前passのfold済pointer位置は引き継ぎ、Unknown/Goneを未来位置で補完しない。live siblingを含む正しいRawInputで、
実multipassの副作用一度、新child保持、次fresh入力でのclosed回収を回帰する。

実装担当は利用上限により一度中断した。利用者の「続きをお願いします」を受け同Sol担当を再開し、
中断時の差分・tests2の失敗記録を保持したまま、合意済み所有修正へ継続する。GPU paint leaseはこの時点で未実装。
### R4 Flat navigatorの復元意図とpointer gestureを分離

親と独立UIの追加監査で、単一unionにPendingCenterとHeaderを置く設計では確定済みの位置復元が失われると確認した。
Zの中心外表示→Primary pressでmanual zoomへ採用（pan ZERO）→releaseでPendingCenter→同passのHeader press/release、
という経路で復元意図がHeader/Idleに上書きされる。viewport幅800・描画幅1600・中心UV .75では約400pxの視点差になる。
これはコードとgeometryによる根因確認であり、実機での再現結果とは区別する。旧テストは次の実geometryの中心を見ていなかった。

実装前にSolが前提を再確認し、親・独立UIが以下の構造訂正に合意した。App/ViewerContextBundleの唯一のownerは維持し、
Flat payload内をgeometry intent（None/Center）とpointer gesture（Idle/Pan/Select/Header/AwaitingPan）の独立した型へ分ける。
これらは同時に存在する別の責務であり、同じ状態のbool/sentinel追加ではない。両軸が空ならouter Idleへ正規化する。
旧PendingPanTransitionはCenter+AwaitingPanに対応する。release/park/focus lossはgestureだけ終了し、次の実geometryで
Centerだけを一度消費する。Header/Selectはその消費で消さず、AwaitingPanは復元後panをbaselineとしてPanへ移す。
Center待ち中の新Pan pressはzoomを再採用せずAwaitingPanを設定する。page/source/真のcloseは両軸を失効する。
Headerだけに保存用pendingを足す案は、続くPan/Selectで同じ欠陥を残すため採用しない。新App fieldやディスク保存は不要。
normal/reserved両経路の次実geometry、Headerのframe跨ぎ/park、続くPan、およびA→B→Aの回帰を要求した。

GPU paint batch leaseの製品接続は実装済みで、r4-gpu-lifetime-check1は28.82秒で成功。
独立coreは6描画経路の接続とimmutable Arc保持を限定承認した。r4-gpu-lifetime-tests1はFnMut closure内のmoveで
E0507となり、テスト成功には至っていない。Arc cloneの修正に加え、未tessellate outputと空clipでの解放回帰を要求した。
### R4 capture focus終端の独立監査

独立UI・親が、canvas drag中のWindowFocused(false)で旧targetが残り、他アプリでrelease後の復帰clickが
旧startからCopyを確定する残存経路を確認した。R4導入回帰とは断定せず、変更中の入力所有境界の穴として修正する。
PointerGoneは正当なviewport外dragにも起きるため取消条件にしない。focus lossはevent順のCancel terminalとし、
それより前のreleaseで確定したCopyを後続focus loss/panel pressで覆さない。実Clipboardへ書く前のtyped effectへ分離する。
唯一のarm callerはcapture overlayより後のhoverbar結果処理なので、開始前の同passイベントを新selectionへreplayしない
現在の順序を維持すれば新frame/session sentinelは不要。次passはeguiがvolatile eventsを再送しない。
回帰は実handler/target resolverで、canvas→panel releaseのclamp、Copyとpanel pressの両順序、wait解除後の同pass再press、
focus lossとCopyの両順序、Esc取消後、overlay後armの旧event非replayを確認する。OS Clipboard操作は行わない。

独立coreはR1/R5/R6の既合意設計と現APIを追加照合し、着手を阻む新たな重大矛盾なしと報告した。
R4後の最初のスライスはviewer client/fair ownerの型と独立executor状態回帰、既存SQLを共有するsame-TX reader。
NotBook早期returnも自client撤回、page-order修復0件のcommitも通知、item query不変を接続条件とする。
製品の照会切替は取消可能な実workerとの接続後。MIH採用と本全体の正確性・peak・公平性の合格を意味しない。
### R4 navigator owner checkと共有layout所有の追加修正

r4-nav-owner-check1は30.77秒で成功。独立UIはFlat二軸のHeader/Center共存、AwaitingPanの復元後baseline、
App/bundleの操作owner移管とglobal temp正本除去を確認したが、lifecycleとgeometry共有境界は承認保留とした。
独立coreは入力snapshotの毎pass更新、pass0 live GC、Unknown/Gone、全button、Context非再入をコード上で限定承認。
同IDの物理窓再生成で旧pointerが誤用される実経路は確定せず、counter resetの推測的失効は追加しない。

fullscreen_page_layoutはAppだけのfieldでBundle交換対象に無いと親・独立UI/coreが確認した。
update_active_viewer_contextはwith_active_viewer_context内でBを描画し、closure終了時にAへswap復元するが、
BのlayoutだけAppに残る。次Aのhandle_fs_wheel_and_click→fs_navigator_layoutは新rendererのlayout.clearより前なので、
A/Bのpage_idxが同じならB transformからA ownerのgesture/panが決まる。これは実mount/描画順のコード確認である。
geometryも操作ownerと対でViewerContextBundleへ移す最小修正を親・独立Astraが構造的根因修正として承認した。
raw mountの既存swap/default/capture/restoreへ接続し、ディスクSnapshot、新viewport predicate、blanketclearを追加しない。
影響先はnavigator、ルーペ、範囲コピー、holdoverの表示geometry参照。A/B同page_idx・異geometryで実swap後の描画前handlerを回帰する。

独立UIが、same-idx Splitの直接slice変更で操作ownerが失効しない経路と、focusfalseでgesture終了後に
同pass raw Pressがgestureを再生成する経路も検出した。slice producer全体とordered focus終端を設計照合中。
Continuousではflat layoutがNoneとなる既存仕様があるため、旧Panが直ちに新continuous geometryへ適用されるとは主張しない。
### R4 ordered focusとdouble-clickの依存API境界

Flat/Panoramaのfocusfalse終端後にraw Pressがgestureを復活させる問題は、共通filtered event列へWindowFocusedを加え、
focused区間の操作とFocusLost終端を順序処理する根因修正とする。focusfalseはgestureだけ終了しCenterを保持、
unfocused中のMoved/Button/Wheelは作用させず、focustrueだけで旧gestureを再開しない。先行releaseの確定は保つ。
表示許可（最終focus/key permit）と、既存ownerの終端受付・構造的禁止を区別する。
Panoramaも既存UV/yaw/pitch/FOV計算を各event armへ移す。表示・投影・native APIの再設計はしない。

ただしegui0.33.3のResponse.double_clickedはwidget CLICKED bitとpass全体のdouble-click有無を合成する。
公開位置・時刻にevent序数が無いため、同passのfocus往復を挟んだreleaseへ正確に対応付けられない。
独立UIは以前の「Responseを残して承認区間だけdispatch」案を撤回し、親・独立coreもAPIの情報不足を確認した。
独自時間窓、曖昧時drop、機能制限、unsafe/privateアクセスは採らず、同versionのeguiをvendorへ置き、
PointerStateの判定済み全release列をread-only iteratorで公開する小さい依存変更を独立した境界として承認した。
APIはbuttonと任意のclick位置/回数の値射影のみ。内部mutable型、クリック判定、時刻、入力挙動は変更しない。
raw releaseごとに必ず内部Releasedが1件あり、非click=Noneを含めるとordinalが1:1になる。Focus/Goneはreleaseを合成しない。
全button/全focus区間へ注釈後にappのhit/gesture/focusを判定する。API自体はwidgetのclick所有を保証しない。
root patchとworkspace除外に加え、standalone vendor/eframe・egui-wgpuにも同local egui patchを接続する。
直接依存だけpathに変えてtransitive registry eguiと型を分裂させない。各cargo tree/lockの一実体化、
vendor/eguiの無filter lib testsをtest-fullへ追加することを統合条件とする。Solも実装前に前提整合を確認した。

親の原本検証: cached egui-0.33.3.crateのSHA256はCargo.lockの
6a9b567d356674e9a5121ed3fedfb0a7c31e059fe71f6972b691bcd0bfc284e3と一致。
archive内106fileと展開sourceの全bytesが一致（mismatch0）。追加.cargo-okはregistry markerなのでvendorへ不要。
upstream git sha1は44cdd653e2317d300fb8a6c9c36b03f23991e803、path_in_vcs=crates/egui。
これはread-only検証であり、依存変更の実装・テスト完了を意味しない。

initial focusを!eventで逆算する案も訂正した。winit adapterは重複Focused通知を除かない。
既存viewport snapshotの前pass final factを使い、focus event無しpassは現在raw.focusedをその区間のfactとする。
native egui-winitはfocused=falseで作られ通知を待つため、初回にfocus eventsがある場合のseedはその初期条件と照合する。
初回/重複通知/通常複数passの回帰を実装前確認に含め、新Appfieldやcounter reset heuristicは追加しない。
### R4 paint batch寿命の補強回帰成功

r4-gpu-lifetime-tests2.logはcompile 1m04s、exit0、5件成功（既存lease単体1件＋新4件）。
filterから外れたempty_clip_discards_mesh_and_callback_and_releases_leaseは
r4-gpu-empty-clip-test1.logで別途実行し、compile0.67秒、exit0、1件成功した。
これにより新5群（shapes→tessellate→batch、未描画FullOutput破棄、空clip、同lease2batch、異lease独立）はすべて成功。
独立coreは余分なArcが結果を隠さないことを含めコード承認済み。実renderer lock/GPU実機の実行検証とは区別する。

navigatorはr4-nav-owner-tests-compile1が旧state fixture型で33errorsとなり、その追随後の
r4-nav-owner-tests-compile2は2m08sでno-run成功。これはsuite実行成功ではない。
その後fullscreen_page_layoutをbundleのfield/default/destructure/swap/parked moveへ移管し、状態回帰へ進む。
親はR2 staged binary diffを再計算し、100651 bytes、SHA256
3adba6c9c6768c1540f3d8f0791d1dfefdc322acb270ec90966d5bca2131f42aが引き続き一致することを確認した。
### R4 navigator所有と依存APIの成功checkpoint

r4-nav-owner-tests3はPageSlice import不足、tests4は実enumに無いFirstHalf指定でcompile失敗。
tests5は46成功/2失敗（hook未導入fixture、通常zoomの即時panをCenter待ちと誤認した期待値）。
fixtureを訂正し、r4-nav-owner-tests6はcompile 1m00s、3.06秒、exit0、49件すべて成功した。
Zの実adopt→短押し→Header→次の実geometry consumerで中心外UVを復元する回帰と、
復元後baselineからの実Moved移動量を追加し、独立UIが検証内容を承認した。
A/B同page_idx・異geometryの実bundle swap後handler、slice変更失効、multipass更新/GCもこの49件に含む。

新vendor eguiのAPI/2回帰/manifest/test-full接続は独立coreがコード承認済み。
r4-vendor-egui-tests1はofflineでcolor-hexが未取得のためexit101（製品テスト失敗ではない）。
通常取得後のtests2はcompile 7.40秒、exit0、無filter lib 25件すべて成功した。
appのordered focus/release接続、各standalone graph/lock、provenance記録はこの時点では未完了。

復元可能な中間記録をtarget/r4-nav-owner-stage1-20260908へ保存した。
HEAD=d0784377ff9259de21d5db38559eb0326328c5bf、staged R2 patchは従来のSHA256と一致。
unstaged-source.patch SHA256=9c3aaa458aab0efa4b7446cd959032dd32852f24a521e5620ae4c63fd9a3ec78、
vendor-egui.zip SHA256=ba0364691db9af7924f5e5e9893993ad98ef37796981f56d5802600f971b4963。
親も保存artifactの4 hashをmanifestと再照合して一致を確認した。vendorは106 source files、生成targetを含まない。
このcheckpointはR4全体合格や新portable完成を意味しない。capture effect/実handler・lifecycle回帰、
残るfocus接続、fmt/glyph/full gate、portable更新と実機確認を継続する。R1/R5/R6の製品実装も未完了。
### R4 ordered Flat接続の先行レビュー（未合格）

通常画像のordered focus/release接続後、compile前の独立UIレビューで2件のP2を確認した。
1. Zの先行Primary pressでmanual adopt済み・Center未消費なのに、double-click release時の
   再adopt=falseを理由にgeometry=Noneへ上書きしていた。既存Centerを踏まえて新clicked UVの
   Centerへ置換し、次の実geometryで解決する。通常zoomの即時panを保持し、新sentinelは不要。
   実count2、中心外Z表示、handler→consumer→UV中心を回帰する。親もコードで確認済み。
2. 固定表示OFFのhold表示中、同passのpress/release→focus lossで最終hold permitが消えると、
   ordered reducer前のfs_navigator_allowedが先行操作まで捨てる。入力受付と最終表示許可の
   分離が入口では未完了。過去focusを使った偽permitで現在OS levelを読むことや、gate全撤去で
   不可視navigatorを操作可能にすることはしない。event-time修飾キーと既存Keymap解決の境界を調査中。
SolはPanorama接続も進めており、この2件の修正・狭域回帰までは接続全体を承認しない。
### R4 hold-onlyの既存別件を分離（範囲訂正）

親/Solが変更前HEAD=d0784377fを照合し、hold-only未開始操作→focus loss/hold releaseの消失は
元から同じ入口gateで発生する既存欠陥で、今回のordered接続が導入した回帰ではないと確認した。
「あらゆる先行操作を保持」をこの別件まで広げた親の要求を訂正し、今回R4の阻害指摘から外す。
独立UIも、これを残して今回の既存owner終端/fixed-visible順序/preview handoffを正しく実装できない
追加経路は確認していないと回答した。既存機能の削除・劣化による回避は行わない。

event ModifiersにはWindowsの左右情報が無く、既存matches_modifiersも非Windows用exact matchのため、
カスタムを保つ単純helper追加では直せない。実描画済み領域・Fixed/Hold根拠をnavigator ownerへ統合する
案は今後の候補としてのみ残す。今回その新構造を実装しない。
別件はnext-release-backlog.md §1.200へ記録（branch最大198、ローカルmaster最大199をread-only確認し採番）。
Z double-clickで未消費Centerが消える今回の接続回帰は引き続き必須修正・必須回帰とする。
R4 ordered接続の製品check: target/r4-ordered-focus-check1.logは47.04秒で成功。
独立UIはPanorama handlerを変更前HEADと対比し、今回変更による新P1/P2なしと限定承認した。
Selectのrelease位置→従来UV/yaw/pitch/FOV式、Panのstart/TAU/PI/clamp/sanitize、headerの同owner処理、
所有Panと対応release count2での再中心、focus終端と先行確定保持を確認。追加回帰の実行成功は未確認。
### R4 ordered focus/releaseの段階回帰成功

r4-ordered-focus-tests1.logはexit0、54成功/0失敗、4.36秒。Z実count2→次geometry復元、
Flat/Panoramaのheader releaseとfocus lossの両順、focus復帰後の新pressを含む。
独立UIはhad_pending_centerを使ったZ double-clickの製品修正と回帰を承認し、追加製品P1/P2なし。
テスト検出力の指摘に対し、Panoramaは2回目release後の別位置Movedを追加して補強した。
r4-panorama-release-position-test1.logはcompile38.92秒、exit0、1成功、0.16秒。
初回focus通知/重複false通知/次pass no-event raw trueの実ctx.run snapshot回帰は
r4-focus-snapshot-test1.logでcompile55.54秒、exit0、1成功、0.00秒。
54件suite後の補強2件は各々実行したもので、55件suite一括再実行とは記録しない。
Solはcapture typed Copy/Cancel effectと既合意handler5群へ進む。R4最終gate/portableは後続。
### R4 capture effect接続のcheckと初期focus終端

r4-capture-check1.logは35.06秒で製品check成功。Copy{idx,crop}/Cancelのtyped effectをoverlayが返し、
唯一callerがClipboard dispatchする境界へ分離した。OS Clipboard非操作のhandler回帰は追加中。
親が、既存selectionでinitial_focused=falseのままloopへ進むと、同pass後続Focused(true)→releaseで
旧targetをCopyできる枝を確認。Solも所有契約の矛盾に同意し、loop前にCancel終端するよう修正する。
focusfalseをoverlay非実行passで記録→次passFocus(true)+releaseでもCopy0の回帰を含める。
新armは既存どおりoverlayより後なので、旧focus往復の後に始めた選択を次passで誤取消しない。
### R4 capture回帰初回のfixture自己待ち

r4-capture-tests1.logはcompile 1m48s後、11件中7件成功、4件がover60sの実行中表示となった。
親/独立UIはrelease→panel逆順テストとfocus/Escテストで、旧AppTestEnvを同scopeに保持したまま
次のsetupを呼ぶ自己待ちを確認した。let shadowingでは旧値がscope末までdropされず、
AppTestEnv::_lockが保持する非再入data_dir::test_override_lockを同threadが再取得する。
他2件は共有lock待ちに巻き込まれ得るため、4件を製品hang/製品テスト失敗とは分類しない。
実装担当が該当実行を管理し、各caseを独立scope/closureとして次setup前にAppTestEnv全体をdropする。
設定override/保護mutexや既存drop順を弱めず、十分な実行時間で再試験する。
r4-capture-tests1はfixture自己待ちのため実装担当が中断し、次setup前にAppTestEnv全体をdropする4箇所を訂正。
r4-capture-tests2.logはcompile38.82秒、exit0、11成功/0失敗、1.35秒。保護mutex/OverrideGuardは不変。
独立UIは初期focusのloop前Cancel、typed terminal、実overlay/target/crop、fixture寿命を限定承認した。
検証表記の訂正: Esc caseはこの時点でselection=None直代入のため、実Esc handlerは未検証。
成功は「取消済みownerへの後続releaseがCopyしない」まで。実Esc event→handle_fs_key_inputへ
同caseを置換する最小補強を要求した。未変更のEsc製品経路に新P1/P2を発見したという意味ではない。
### R4 実Esc補強とテストlock順の追加訂正

r4-capture-esc-handler-test1.logはcompile1m04s、exit0、1成功、0.65秒。
Windowsでnative Escape KeyEdge＋egui Escape event→実handle_fs_key_input→次pass releaseを確認。
非Windowsは取消済state回帰のままなので、実Escの合格範囲はこのWindows実行に限る。
ただし親/独立UIは、当初の補強がAppTestEnv(data_dir lock)の後でTEST_INPUT_LOCKを取り、
既存app/tests.rs::same_frame_second_ctrl_down_edge_is_dropped_until_target_is_presented等の
input→data順とAB/BAを作ることを確認。狭域単独PASSは並列安全性を意味しない。
TEST_INPUT_LOCKをtest冒頭・最初のAppTestEnvより先に取得し、App全体のdrop後まで保持する。
入力cleanupと保護overrideも維持する。修正確認までこの補強の最終承認を保留する。
capture Esc lock順v2: r4-capture-esc-handler-test2.logはcompile39.46秒、exit0、1成功、0.63秒。
TEST_INPUT_LOCK→ClearTestKeyFrame→最初のAppTestEnvを取得し、各caseのAppTestEnvを次setup前にdrop。
終了時もAppTestEnv→入力cleanup→input lockとなることを独立UIが確認し、capture段階を最終承認した。
先の11件成功＋実Esc補強後1件成功であり、11件全体を最終版で再実行したとの表記はしない。

残るR4既合意の実入口回帰は、(1) compare pin Aを保持してReady B描画→release後A保持、
(2) Single/Spread/ContinuousでReady assetをbody/nav双方の実描画へ通すこと、
(3) 実park/close/source通知とmounted/AtRestのpreview遅延完了pollの接続。
独立coreが棚卸しし、製品接続はあるが現在のbegin_test_pressは未完了workerを保持してB描画へ到達しないと確認。
新規機能範囲ではなく、既合意の残検証として進める。snapshot、依存provenance/graph/lock、fmt/full gate、
portable更新、利用者実機確認も後続。R1/R5/R6の製品修正は引き続き未完了。
### R4 入力/captureの整形区切り

cargo fmt -p mimageviewer成功後、cargo fmt --all -- --checkもexit0。
記録はtarget/r4-cargo-fmt-stage2.metaとr4-cargo-fmt-check-stage2.meta（正常終了時のstdout logは未生成）。
vendor/egui/src/input_state/mod.rsの前後SHA256は
866485bd3cb04f63639a1e6c142d85dab0689e28a471942ee1f9fa03fd942c0cで一致し、原本の一括整形はしていない。
親がR2 staged patchを再確認し、100651 bytes/既定SHA256一致。source editを一時freezeして
文書8件だけをpathspec commitし、入力/capture段階のsource checkpointを保存してから次回帰へ進む。
### R4 入力/capture checkpoint確定

文書8件を通常hookでcommit: 32fefa17fd21a6d251793b6439e046c6afed6ddf。
正本artifactはtarget/r4-input-capture-stage2-20260908-v3/manifest.json。
19 source filesの生bytesをsource-files.zipへ保存し、改行/BOMも含めて復元可能にした。
source-files.zip SHA256=fd79bb2dfbcb3e9fdfe8b90ce41898886427a13675a2ac2b5e80cccf89c9ecee、2350069 bytes。
unstaged-source.patch SHA256=a26a84584888652d4bde6bc698fd0f62045acc2d45a2006430f59b76b961e329。
vendor106filesを生成targetなしで保存し、archive全entryを凍結中sourceとbytes照合、全20artifactsのhashも検証した。
R2 staged patchは既定SHA256/100651 bytesのまま。native sourceはcommitしていない。
初回/v2 backupは原本の1980年より古いtimestampと不存在の空fmt stdout logで保存scriptが失敗した不完全記録。
source変更は無く、ZIP timestampの範囲設定とmeta参照に訂正したv3だけを利用する。
このcheckpointは最終gate/portable/実機合格ではない。freezeを解除しReady描画/実lifecycle回帰へ再開した。

### R4 Ready assetのhelper/owner統合回帰

target/r4-similar-preview-render-tests1.logはcompile 1m55s、exit0、19成功/0失敗、0.67秒。
新2件は実completion channel→asset_for_frame→body/nav描画helperを通し、3 layout modeの
session resolverと、通常pin Aを保持したReady B描画→生release入力後のA slot identity/mode保持を確認した。
独立coreはcompletion handleと回帰を承認したが、製品render_fullscreen_viewportのdispatcherを
直接通らないため、Continuousの誤分岐やnavigator呼出欠落、release後のA再描画までは検出しない。
helper/owner統合成功と製品dispatcher接続の残検証を区別し、既存snapshot harnessの適用を調査する。
実park/close/source通知とMounted/AtRestの遅延完了回帰も継続する。

### R4 context終端の狭域回帰

target/r4-similar-preview-lifecycle-tests1.logは新fixtureのborrow競合E0502でcompile停止。
password store cloneをmut borrow前に取るよう訂正後、tests2はcompile58.48秒、exit0、3成功/0失敗、0.47秒。
実close_fullscreen→Mounted pollと、実pause_current_active_viewer_context→AtRest復元→背景pollは
独立coreも根因を検出する回帰と確認した。poll前後のAtRestを検査し、試験中の確認mountとは区別する。
source caseはfresh Ready hitのobserve_ready_hit以降であり、filesystem通知やmetadata panelの
Ready受付loop全体を実行した証拠ではない。Ready/Preparing受付境界と、park時に無関係Bの
pending completionが生き残ることを最小補強する。

Ready実dispatcherは既存embedded fullscreen経路を通常unitで通す。App丸ごとsnapshotは
ui-snapshot-policy.mdの対象外なので作らず、純navigator paintを共有する固定sceneのみに限定する。

### R4 実embedded dispatcher回帰の成功

製品コードを変えず、Windows unitのctx.run→render_fullscreen_viewport→embedded bodyを通す2件を追加。
最初のdispatcher-tests1はfilterがPinnedNormalだけに一致し、1件成功だった。
modes-tests1は元FsCache IDへの固定期待で失敗。実通常描画はprocessed textureを選べるため、
release後のread-only resolve_fs_display_texから製品が選択したIDを取得して検証するよう訂正した。
font/backgroundでも通るany mesh!=candidateの途中案は親が不承認とし、採用結果に含めない。
modes-tests2/3はSpread LTRのsession失効。fixtureの寸法未確定時はDouble、横長cache投入後は
既存ペアリング規則によりSingleとなるためで、縦長canonical寸法へ訂正し投入後sessionも照合した。
modes-tests4はSingle/LTR/RTL通過後、Continuous通常復帰のnav期待で失敗。既存の
flat_navigator_main_pagesはContinuousを対象外とするため、通常復帰は本文だけを要求する。
候補中は固有navigatorを持ち、body/nav双方を引き続き要求する。製品機能の変更はない。

最終狭域ログ:
- target/r4-similar-preview-dispatcher-modes-tests5.log: compile42.77秒、1件内4mode全成功、0.61秒、exit0。
- target/r4-similar-preview-dispatcher-pinned-tests2.log: compile0.65秒、1成功、0.16秒、exit0。

候補exact TextureIdをbody/navで確認し、release後は製品resolverのexact IDとviewer-owned page
geometryを確認する。Single/SpreadとPinnedNormalの復帰はbody/nav、Continuous復帰はbodyを検査。
PinnedNormalはpin Aのslot identity/modeも保つ。実native GPU/windowの検証とは区別する。
独立coreはこの判定境界と成功ログを照合し、dispatcher段階を最終承認した。

### R4 実parkの別context非干渉補強

既存park caseへ無関係なmounted root Bの実pending completionを追加した。
Aを実pause→AtRestへ戻してから両completionを送り、同all-context pollでAの破棄/drainと
Bのcache採用・gesture保持・pending終了を確認する。Aはpoll前後ともAtRestで、検査時だけmountする。
target/r4-similar-preview-park-sibling-test1.logはcompile52.61秒、1成功/0失敗、0.24秒、exit0。
独立coreがcode/logを照合し、別contextを空にした旧fixtureの検出不足が解消したと最終承認した。

### R4 Ready受付の所有境界と最終狭域成功

SimilarPreviewState::observe_query_resultへfresh ItemQueryの受付を集約し、個別hit入口はprivate化。
UIはlast_ready fallbackの適用前にこの受付を呼ぶ。Readyの全hitだけが既存stamp照合へ進み、
Preparing/空Ready/NotIndexed/NoIndex/Featureless/Failedは更新や削除の証拠にしない。

- target/r4-similar-preview-query-boundary-test1.log: compile1m01s、1成功、0.00秒、exit0。
- target/r4-similar-preview-source-query-lifecycle-test1.log: compile0.65秒、1成功、0.17秒、exit0。

Ready更新時の旧要求Drainingと非Ready/空結果時のRunning保持、実Mounted pollでの旧completion
破棄/drainを検査。独立coreは製品接続・テスト・両ログを照合し、この境界を最終承認した。
これはindexed queryの更新受付であり、filesystem watcherを実行した検証とは表記しない。

### R4 file-change入口を含むcontext suite成功

実reset_fs_side_panel_runtime_for_file_change→旧completion送信→all-context pollのcaseを追加。
親が製品入口の取消とDraining保持、pollによるcache非採用/回収を確認した。
target/r4-similar-preview-lifecycle-tests3.logはcompile55.42秒、4成功/0失敗、0.63秒、exit0。
Ready受付・true close・別context pendingを含むpark・file-changeを同最終suiteで実行済み。
これで合意したReady描画とcontext実入口の回帰は揃った。純draw snapshot、依存graph/license/lock、
最終fmt/glyph/full gate、portable build/updateと実機確認は引き続き後続である。

### R4 純描画snapshotの追加・目視承認

候補navigatorの既存paint部分をpaint_similar_preview_navigator_surfaceへ抽出。
製品wrapperのgate/settings/geometry/resource寿命は不変で、色・枠・順序・clip保持を独立UIが承認した。
Appを作らないlib unitの固定sceneは、4象限と白十字の生成texture、通常zoom/pan、実geometry解決、
minimum-overlap補正を使い、補正後geometryを本文/navで共有する。
初回fixtureのhost clipとnav配置域は製品と異なったため、本文body_rect clip・layoutのbody/body入力へ訂正した。

正本: tests/snapshots/similar_preview_navigator_dark.png、640×360。
SHA256=2c9830fb7116944ef5b79151e9aa65a03e1d078a1f1d7bd02329bf4b8968f9b4。
- target/r4-similar-preview-navigator-snapshot-update3.log: compile39.55秒、1成功、0.90秒、exit0。
- target/r4-similar-preview-navigator-snapshot-compare1.log: compile0.66秒、1成功、0.78秒、exit0。

親・独立UIがそれぞれPNGを開き、本文右上の48px緑領域とnav左下黄枠、4象限方向、白十字、headerを確認。
独立UIはcode/PNG/hash/logを照合し最終承認。実dispatcher・GPU/nativeの検証とは区別する。
ui-snapshot-policy.mdの旧bin/stub説明を現lib構成へ訂正し、固定texture純描画の実例を記載した。

### R4 依存統合と最終gate前のfreeze

root/eframe/egui-wgpu/egui standaloneのcargo treeはすべて同local egui 0.33.3へ統一。
target/r4-cargo-tree-{root-egui,eframe-egui,egui-wgpu-egui,egui-standalone}1.logを親・独立coreが確認した。
既存3 Cargo.lockはegui registry source/checksumの2行除去だけでversion変更なし。
原本106ファイルを再比較して変更はinput_state/mod.rsのみ。LICENSE-MIT/APACHEを既存vendorから
byte copyし予定SHA256と一致。PATCHES.mdを含む追加3ファイル、計109ファイルで出典と本文を保持する。
独立coreはmanifest/gate接続、graph/lock/licenseを最終承認。provenance詳細はvendor/egui/PATCHES.md。

Solがcargo fmt、cargo fmt --check、git diff --checkをexit0で確認（fmt stdoutは空）。
整形前後numstatはtarget/r4-final-fmt-{before,after}.numstat。
target/r4-check-ui-glyphs-final1.logはexit0、no dangerous glyphs。
親もdiff-checkとR2 staged100651 bytes/既定SHA256一致を再確認した。
source編集をfreezeし、文書3件だけをpathspec commit、全source/artifactをcheckpoint化してから
同sourceの最終full gateとportable buildへ進む。R2/native sourceのcommitと実機確認は後続。

### R4 最終検証用source checkpoint確定

文書3件を通常hookでcommit: f67b0b61734dd415de8f7b3928bf68dc78bd0fa2。
正本: target/r4-verification-source-20260908/manifest.json。
23 source filesを改行/BOM/PNGの生bytesで保存し、vendorは生成targetを除く109filesを保存。
87 artifactsを再hash照合し、凍結中sourceとのbytes一致も検証した。
- source-files.zip: 2403218 bytes、SHA256=19db5db916510191895745bdd826c79aa86cc585a9d201f9d95297561e875cc8。
- vendor-egui.zip: 455410 bytes、SHA256=dbfb4cc41d95a511efba5da93d7583dee85d54245392a989240c3066e03068e6。
- unstaged.patch: 371632 bytes、SHA256=ba14dcd67d17aa79cd113bb0cee8719da0b862ff9edd084a39b24c5b499e9760。
- staged R2: 従来100651 bytes/SHA256=3adba6c9c6768c1540f3d8f0791d1dfefdc322acb270ec90966d5bca2131f42aのまま。

source freezeを維持し、同sourceへ最終test-fullを実行する。これは実行開始指示の記録であり、
full gate成功やportable完成の記録ではない。R1/R5/R6の製品修正は次段階に残る。

### R4 full gate初回の環境失敗とfixture追随漏れ

最初のtarget/r4-test-full-final1.logはworkspace compile中、複数rustcが数MiBのメモリ確保に失敗。
OOMに伴う0xc0000409やmetadata形式エラーを記録した。test assertionは開始前で、製品テスト失敗とは分類しない。
プロセス終了を確認し、ソース不変でCARGO_BUILD_JOBS=1を指定して同scripts/test-full.ps1を再実行した。
ビルド並列だけを変更し、テストの選択・並列実行や要求を弱めない。

再実行target/r4-test-full-final2-j1.logはcompile通過後、libが7680成功/10失敗/38ignored、411.95秒。
10件すべてがui_fullscreen.rsの「fullscreen navigator input tracking must run at begin-pass」panicだった。
app/testsの別窓bookmark/フォルダ移動7件とui_fullscreenのtouch/right-drag3件で、独自Contextに製品の
begin-pass hookを登録していない。親・独立coreは製品lib.rsの登録と実update→描画の経路を照合し、
R4で追加した入力初期化契約へのfixture追随漏れと確定した。最初に疑った5秒scan timeoutではない。

workspaceの残suiteは完走し、失敗targetはlibのみ。scriptは失敗伝播によりvendor後段へ進まなかった。
対象fixtureへ製品同hookを最初のpass前に登録する。productionのexpect/fallback、既存assertion、
待機期限、テスト選択は変えない。修正後は狭域10件→fmt→再freeze/checkpoint→全体gateを再実行する。
最初のsource checkpointと両失敗logは保持し、合格記録へ上書きしない。
### R4 fixture追随と最終検証checkpoint v2

10件は各Context生成直後へ製品のinstall_fs_navigator_input_trackingを1行ずつ追加した。
狭域10実行はそれぞれ1成功/0失敗（target/r4-fullgate-fixture-*.log）。fmt/fmt-check/diff-checkもexit0。
親と独立coreが前checkpointの生bytesと比較し、app/tests.rsの7行・ui_fullscreen.rsの3行以外の
source/vendorが不変、assertion/timeout不変、初pass前の同Context登録を確認して承認した。

再freeze正本: target/r4-verification-source-20260908-v2/manifest.json。
HEADはf67b0b61734dd415de8f7b3928bf68dc78bd0fa2、23 source/109 vendor/99 artifacts。
source-files.zipは2403400 bytes、SHA256=07de0729cd07a81f4334510c00f14bf00edd3724d66276b19f578f8e14b3fbe6。
親が99 artifactsを再hash確認した。R2 staged100651 bytes/既定SHA256も不変。
初回checkpointと失敗ログを保持し、CARGO_BUILD_JOBS=1で同full gateのfinal3へ進む。
この時点ではfull gateと新portableの成功はまだ記録していない。

### R4 final3で再現した初回DB openの競合調査

final3-j1はPowerShell内部pipelineが通常cargo stderrをNativeCommandErrorへ変換し2.5秒で停止した。
製品assertion未実行のrunner失敗として保存し、source不変で外側redirectのfinal3b-j1へ切り替えた。

target/r4-test-full-final3b-j1.logはlib7689成功/1失敗/38ignored、429.39秒。
前回の10fixtureは通過。失敗は既存indexer_manager::tests::similar_only_favorite_uses_existing_supervisor_watcherで、
初回reconciliationの最終状態がFailed("similar.db open failed: database is locked")だった。
workspace後続は完走、vendor後段へは失敗伝播で進まなかった。

configureはschedulerのdb_for_workerとstart_memory_loadを並行起動し、両者が初回同じSimilarDb::open_atへ入る。
R9の同Arc所有はscan/purge/prefillまでで、この独立open競合は対象外だった。
既存の並行open回帰は事前にWAL化済みで初回競合を覆わない。親・Sol・独立coreがこの経路を確認した。
ただし現ログはopen全体の失敗でSQL段階は未特定。診断用のpath限定Barrier/stage記録で旧経路を観測してから
修正する。SQLiteのWAL切替はbusy handlerを呼ばないREAD→WRITE昇格競合があり、timeout延長だけを修正にしない。
CatalogDbに存在する変換時だけの直列化・再確認を参考に、SimilarDb初期化の所有境界を検討する。

### 初回open診断の観測と修正境界の承認

一時cfg(test) seamでunique fresh pathだけを対象に、Connection::open直後で8 openerをBarrierへ揃えた。
target/r4-similar-db-first-open-probe1.logはcompile2m14s、0.03秒、1成功/0失敗。
観測値は `concurrent fresh open failures=1 stages=[JournalMode]`。この成功は旧経路の失敗段階を特定したもので、
製品修正が成功したという意味ではない。元の全体失敗ログ自体にはSQL段階が無いことも区別する。
診断seam/testを完全撤去し、src/similar_db.rsの事前SHA256=
4f2fb28e3aa2625387f78594914e0e54b0c4e0e4804c9e7eb043ddb12fd635c5へ戻ったことを親も確認した。

親・独立coreはSimilarDb private helperで初回WAL変換を所有する方針を承認した。
既WALなら変換せず、読取statementを解放してから変換用mutexを取り、mutex内で再確認して必要な変換だけを行う。
schema初期化は既存IMMEDIATE・180秒待機・lock取得後version再確認を維持する。
helperからscheduler/roots/DB ownerを取得せず、全open_at入口を覆う。Catalog側への変更やworker全体の再設計は不要。
別プロセス等の競合後に再確認してもWALでなければエラーを保持する。UI待機・retry/sleep・テスト期限延長を導入しない。
製品helperと新規同時open、既WAL＋writer、schema/移行回帰の実装・検証へ進む。

### WAL修正の狭域結果と最終fixture

製品helperを変更せず、次の各狭域で実対象1件成功/0失敗を確認した。

| ログ（target/） | 対象 | 実行時間 |
| --- | --- | --- |
| r4-similar-db-wal-fresh1.log | 新規8 stores ×8 concurrent open | 0.17秒 |
| r4-similar-db-wal-existing-writer1.log | 既WAL・IMMEDIATE writer保持中のopen | 0.01秒 |
| r4-similar-db-wal-existing-many1.log | 既WALの多数open/write | 0.12秒 |
| r4-similar-db-wal-v1-migration1.log | 既存v1署名・行・store保持 | 0.02秒 |
| r4-similar-db-wal-timeout1.log | busy_timeout設定値のみ | 0.01秒 |
| r4-similar-db-wal-v1-wait4.log | v1 WAL writer解放後の実移行と行・署名・store・seq保持 | 0.14秒 |
| r4-similar-index-integration-after-wal1.log | 元のsupervisor watcher結合回帰 | 1.30秒 |

fresh回帰は初回openを実行するが、SQLite内の特定interleavingを必ず強制するテストではない。
旧コードのstage診断でJournalMode失敗を観測した証跡とは区別する。
v1待機fixtureの開始通知はopen_at直前なので、100ms未完了だけでSQLite内部の待機到達を断定しない。
writerを保持した状態で開始し、解放後に実移行とデータ保持を確認する。最終fixtureは早期観測を保存し、
rollback→terminalのResult保存→join→assertionの順で、失敗時もworkerを回収する。

v1-wait1はfixtureの型比較によるcompile error、wait2はfilter不一致で0件、wait3は1成功だがtimeout側cleanup前。
これらを最終成功の代わりに使わず、修正後wait4の実1成功を正本とする。
cargo check coreはr4-similar-db-wal-check1.log、14.30秒、exit0。
package fmt・fmt-check・全diff-checkはexit0。最終similar_db.rsのSHA256=
3719842f08992d7ae4d6049c266040722cccbccc93429ee4e73ef5adb1ae6653。
R2 stagedのbytes/hashは不変。WALだけのfocused commitとsource checkpoint後に、全体gateを再実行する。

### WAL focused commitと最終source checkpoint v3

独立coreが最終helper/fixture/hash/7狭域/core checkを照合し、追加指摘なしで最終承認した。
WALだけをcommit: 48610d4d476dc4a3029ac58e548cfd9bdc808ef2
(`fix(similar): serialize initial WAL conversion`)。src/similar_db.rsのみ147追加/1削除、通常fmt hook成功。
master逆統合はせず、R2 stagedは既定100651 bytes/SHA256不変。

正本: target/r4-verification-source-20260908-v3/manifest.json。
24 source /109 vendor /112 artifactsを保存し、親が全artifact hashを再検証した。
前v2の23 sourceは生bytes不変、focused commit済みsimilar_db.rsも生bytesを追加保存した。
source-files.zip: 2424106 bytes、SHA256=4d07663c777281755664dd6f4305a8be179ac36cb799335594dbdf4ac042c71c。
CARGO_BUILD_JOBS=1・PowerShell内部pipeline無しで同test-fullをfinal4-j1へ再実行する。
このfreezeからfull gate・portable完了まで製品sourceとHEADを変更しない。
後続R1の草稿はCargoから参照されないtarget内に限り準備できるが、製品接続/別cargo/負荷計測は後続とする。

### R4 最終full gate成功

target/r4-test-full-final4-j1.logはexit0、末尾[test-full] PASS。
main lib 7693成功/0失敗/38ignored、352.25秒。workspace/既存snapshot群も成功し、後段vendor egui25、
egui-wgpu9、eframe15もすべて成功した。ログは52 test-result行であり、重複を含み得る合計をunique件数と呼ばない。
元のsupervisor watcherと前回の10fixtureも全体実行で通過した。

親が24 sourceと109 vendorの生bytes/hash、HEAD、R2 stagedの不変を再確認した。
full gate log: 812000 bytes、SHA256=406448c28196bf45f2b983aa22d4c4ca0467b8232bba56c96ea5090de1ee01a7。
以前の320/375秒とは構成・環境が異なるため、この352.25秒だけからテスト時間の変動原因を断定しない。

同source/HEADを固定してCARGO_BUILD_JOBS=1・build-portable.ps1 -KeepRunningを開始。
親もdist package/zipの絶対pathがrepo配下、package25entriesにreparse無し、data/data-remote無し、
前R2 immutable zip（264631167 bytes）保持を確認。利用者processの停止やportable-devの起動は行わない。
portable build/update/hash照合と実機はまだ後続である。

### R4 ポータブル成果物と利用者への引き渡し

2026-09-08、build-portable.ps1 -KeepRunning がexit0。release core 25分03秒、remote 0.39秒。
HEAD 48610d4d476dc4a3029ac58e548cfd9bdc808ef2 とsource checkpoint v3を固定したビルドである。
正本はtarget/r4-portable-milestone-20260908/manifest.json。配布zip・build log・source manifest・
full gate参照と更新前後の証跡を保存した。

- r4-portable-verification.zip: 264207780 bytes、SHA256=71b037d893e2f788a673a3b6352587a856e3d94298ee7e0647297d7bb6c176cc。
- mimageviewer.exe: 92667904 bytes、SHA256=7351fe1e4e7a5867c27aac23bd35ec2b48336a2257b5444e229e5c4d30e8e341。
- build-portable.log: 11934 bytes、SHA256=2b83f32c120c74dcb2d3d328e0546043a55e279219c0dbd57ecc97d6fb5e3b88。

packageにdata/data-remote無し、対象と親にreparse無し、更新先の正確なexe pathを使う稼働process無しを確認。
update-portable-dev.ps1 -SkipBuildはexit0、runtime全24filesのSHAがpackageと一致した。
data/data-remoteは存在・非reparse・creation/lastwrite/attributesが前後不変。内部データ走査は行っていない。
親も配布24files・artifact6files・全体gate logのhashを独立照合して不一致0。アプリの起動・利用者process停止は行っていない。
前R2 immutable zipとstaged patchも保持した。

利用者へtarget/portable-dev/mimageviewer.exeの正確な起動コマンドと長押し解除・見開き/連続・pin・focusの
実機シナリオを渡した。R2とR4の実機確認は返答待ちであり、成功とは扱わない。
R4成果物を固定したためsource freezeを解除し、R1 slice1の独立owner/executorとmock回帰へ進む。
既存query callerの切替、R1/R5/R6の製品計算・oracle・性能検証は未完了。

### R1 要求管理の第1区切り（製品caller未接続）

src/similar_book_query.rsとlib登録を独立moduleとして実装した。要求key/世代・実行権・FIFO・取消・completion・
worker lifecycleを型で所有し、Weak client再bind、owner Dropの非join、job panic後のruntime破棄/再作成を扱う。
初回/再作成は共通create_runtime_if_liveでlock下にLiveを確認して承認し、guard解放後にfactoryを実行する。
承認前の停止は初期化を始めず、承認後の停止はworker内で構築終了後executeせずdropする。

初回tests1はtrait bound/型注釈のcompile error、tests2は8成功だが100yieldの待機を含むため最終証跡にしない。
tests3は8成功。独立coreの差戻しでgetter製品公開、初期化race、同本2client、withdraw/Drop・global hard・
init/restart Err/panicの回帰を補強し、tests4は17成功。tests5も17成功だがDrop返却200msという不要な
scheduling条件を残していた。論理的な非join証明はbarrier未解放での返却なので、最終は3秒bounded待機へ揃えた。

停止fixtureはDropを専用test threadへ渡し、観測結果保存→必ずbarrier解放/worker回収→最後にassertする。
delayed spawnerの保留slotはWeakで失敗時のArc循環を防ぎ、client Dropはclients/fifoから対象IDが消えたことを確認する。
100yield/try_recv Emptyだけを完了・誤dispatch不存在の証明には使わない。独立coreは以上の解消を確認し、追加指摘なし。

最終: target/r1-book-query-owner-tests6.log、17成功/0失敗/0ignored、compile35.04秒、test0.00秒、exit0。
log SHA256=ebcb0064fa4d8614ee60da2f232ec443a797515151f6bd57f826e35954fa615b。
製品cfg checkはtarget/r1-book-query-owner-check1.log、32.26秒/exit0（その後はtest fixtureだけ変更）。
package fmt-check/diff-check成功。module未接続由来のdead_code warningは抑制せず、後続接続で解消する。

focused commit: 3109b60e6a7c569d4c91ff474ddf645fced723d9（2files、1544追加）。
module生bytes SHA256=94d9e86150dff5b7a71696d23a618729e7301b804324a812db13aebc6b6e9805。
lib.rsにR4登録が共存するため、一時GIT_INDEX_FILEでHEAD＋R1登録1行と新moduleだけを構成し、通常hook付きでcommit。
real indexの当該2entryだけを同期し、R2 staged patchの100651bytes/SHA不変とworking source bytes不変を確認した。
正本: target/r1-owner-slice1-commit-20260908/manifest.json。R4 portableは以前固定したartifactを維持している。

これはDB計算やUI callerを切り替えた修正ではない。次の独立区切りは需要Active/Retained/Withdrawnとlock外notifier。
その後の同TX reader・MIH・正確な分類/対応付け・性能/peak・最終gate/portableは未完了である。

### R1 需要状態と結果通知の区切り（製品caller未接続）

focused commit: be0075dc1818b6ac45e20ab4cc34e092c2feaf13。src/similar_book_query.rsだけを通常hook付きで保存した。
Active / Retained / Withdrawnを単一の需要状態とし、非表示時は既に受理したjob・完成結果を保持する。
Retained中のsoft更新はdesiredだけ更新し、新規refreshを投入しない。実queryで同一本を再受理すると旧Readyを保ってrefreshを投入し、
別本ならhard取消する。scope/storeのhard失効はRetainedの完成結果も退役し、再投入はActiveだけに限る。
結果完成・致命失敗・hardによる完成結果退役はowner lock解放後の注入notifierで通知する。egui依存はmoduleへ入れない。

回帰はdemand-tests1で19成功/2失敗（旧Readyを即取得するfixture誤り）、tests2で21成功。
別本再受付の回帰を追加しhard-switch1で1成功、製品hard通知と陰性owner状態検査を補強したtests3で22成功/0失敗。
独立coreは製品差分を承認したが、hard通知testのtry_lockとidle workerの待機復帰が競合し得る点を指摘した。
最終fixtureは別clientをMockRuntime::execute内で停止させてlock外callbackを検証し、取消後のterminal回収まで待つ。
notifier2は回収前のstate assertが早すぎて1失敗。修正版target/r1-book-query-demand-hard-notifier3.logは対象1成功/exit0、compile1分24秒。
最終test-only修正後の22件再実行はしていない。製品変更後のtarget/r1-book-query-demand-check1.logは28.85秒/exit0。
package fmt-checkとdiff-check成功。source生bytes SHA256=7abace84c3d9d28f1dc455dde55fb0e2ae056dc536439a7f1f2b5b24e14d3f35。

- tests3 log SHA256=724b35aadb8ffb6102829dc98dc79903b8b07c44163ac93710e1f493c5e2c319。
- notifier3 log SHA256=07e9a7058a0e10251cf0d68d08bc1b221d5f1d4e77b8cf421d2539d4b533ef9e。
- check1 log SHA256=57abe2661e04fe51a38f227056f5ba564a8ce55b3badaf9a5090fa18425dd5b3。

commit前後でR2 staged patchは100651bytes、SHA256=3adba6c9c6768c1540f3d8f0791d1dfefdc322acb270ec90966d5bca2131f42aのまま。
R4 portableは固定成果物を維持する。UIの需要遷移・ROOT repaint接続、global readiness gate、同TX reader、MIHと実分類は後続。
次の区切りは実装前にMemory/schedulerの状態射影・通知ticket・guard解放順を具体化して独立レビューする。

### R1 global dispatch gateの区切り（製品caller未接続）

focused commit: ad21305819f80662c28b6d6051ed97b0490f402c。src/similar_book_query.rsだけ、582追加/47削除。
Run/Wait/Completeのprobeをowner lock外で実行し、ticket・先頭client/key/hard世代・lifecycle再照合後に採用する。
Wait中はFIFO未消費、runtime未生成。soft/hard通知はfreshnessとticketを同じlock下で更新し、bare signalは待機解除だけを担う。
worker local資源はWorkerRuntime Initial/Ready/Restartで所有し、取消/停止、init/restart失敗、job panic後の破棄を維持する。

初回gate-tests1は28成功。独立レビューでOption＋restart boolを単一enumへ整理し、gate-tests2も28成功。
probeを停止したままsignal/soft/ABAを操作する3fixtureは、製品がlock外probeを破った場合にもtestを停止させないよう補強した。
operatorを別scoped threadで実行→3秒bounded返却結果保存→必ずprobe解放→operatorとworker回収→最後にlock非保持をassertする。
このtest-only補強の初回tests3はHarness全体のcaptureでReceiver非SyncとなりE0277が2箇所発生した。
実装担当が同名で再実行し、失敗log原本は上書き消失した。親toolのtail観測と担当報告はあるが、原本保存済みとは扱わない。
修正はexecutor/client参照だけをcaptureするもの。以降は試行ごと別log名とする。

最終target/r1-book-query-gate-tests3.logは28成功/0失敗/0ignored、compile1分25秒、test0.02秒、exit0。
log SHA256=9796ac92bd55fb5c5e5231eebfad76d851c05b169323083abc05c00943306ad5。
製品cfg target/r1-book-query-gate-check1.logは27.89秒/exit0、SHA256=237900c3ff1569ca4a4de70c9654a30636876b7687f70a2ef18f005341dc06f5。
fmt-check/diff-check成功。source生bytes SHA256=0b4f52b9d73c8534812a6992c11c8c818ab4eba8d27f223b586ba0d8a096bb22。
独立coreはこの最終SHAのenumと3fixtureを再確認し、追加指摘なしでgate単独差分を承認した。
通常hook付きcommit後もsource SHAとR2 staged patchの100651bytes/3adba6c9...f42aは不変。

後続reader設計は独立レビュー済み。Missing/Failedは派生loaderの過去状態なので本照会の固定terminalにはせず、readonly TXで実態を読む。
実NotIndexedを保存し有効key＋scheduler稼働中の返却だけPreparingへ投影する。全終端通知は配列publishに依存せずsoftへ直結する。
詳細とSQL取消/busy限界はbook-query-review-fixesを正本とする。共有loaderと単体画像検索の再設計には範囲を広げない。
architecture-overviewの永続化表に旧similar.compact/44byteの説明が残っていたため、コードに一致するsimilar.base/48byteへ訂正した。
R4 portableと実機確認待ちは維持。reader/MIH/classifier/実caller/本照会性能とpeak/最終gate/portableの完了を意味しない。

### R1/R6 readonly readerの第1区切り（配列追随・製品caller未接続）

focused commit: 5e0d2bd1b1f456e54cef87d7e8e1a257f4904cfb、2files/686追加/83削除。
src/similar_db.rsにworker専用SimilarBookReader、要求借用BookReadSnapshot、取消可能なreadTX guardを実装した。
READ_ONLY/NO_MUTEX接続は不在だけNone、権限・破損・schema/read errorをFailed用のエラーへ残す。
Deferred TX最初のmetadata SELECTでstore_id/read_seq/page-order versionを固定し、同TXのbase/delta/pages/targetを読める。
SQL本体をprivate &Connection helperへ共有し、既存public wrapperのlock/TX・条件/列順/並び・ID解決の入力順と重複を維持した。
HRTB closureでsnapshot借用を要求外へ逃がさず、SQL/row loopの取消と、成功/error/panic時のhook解除→TX終了を所有する。
Cargo.tomlはrusqlite hooks featureだけ追加した。専用readerのbusy timeoutは5秒で、busy即取消の保証とはしない。

初回target/r1-book-reader-tests1.logは4成功、compile3分25秒。tests2も4成功、compile1分17秒。
独立レビューで、次TXがhandlerを上書きしてしまうと解除漏れを隠すfixtureを補強した。
最終はcancel後join→旧token破棄→Weak消滅確認→別tokenの同接続TX、error/panic後も次TX前にWeakを確認する。
同TX固定の回帰ではmetadata確定後に別WAL writerが本のgenerationを更新し、それ以後の全readが旧snapshotのままなのを確認した。
別の取消回帰内writer更新は取消通知後・join前の正常性を確認するものであり、SQLとの実行重複を固定した証明ではない。

最終fixtureをcompileしたcancel-tests3はfilter未完全修飾のため0件実行であり、成功根拠に含めない。logは保存した。
完全修飾したcancel-tests4は対象1成功、最終tests5は4成功/0失敗/0ignored、test0.04秒。
既存target/r1-book-reader-db-tests1.logは23成功/1ignored/0失敗、0.44秒。reader4件もこの23件に含まれる。
既存target/r1-book-reader-array-tests1.logは8成功、0.05秒。製品check1は37.24秒/exit0（以後test-only変更）。
fmt-check/diff-check成功。独立coreは最終source SHAを照合し追加指摘なしでreader第1区切りを承認した。

- source生bytes SHA256=d01f24299a991e05d3ec2676250d928fd118f057dd973d4d9dbe9bcab3107c4c。
- tests5 log SHA256=f803f447c16aa330277d2c2a3e136706176197bb0f3156e24775ff88c5efbf01。
- DB suite log SHA256=38fbe415e8e73ae01fa9fcb7c2229b374ed715aeb92c584fec543ba1165e099f。
- array suite log SHA256=587fe29c73c533540796f55f35f7f1905bdd8f5a6c9786b2d882b3ef94bc1a1d。
- product check1 log SHA256=fcf3370bf26c6a262d8e39393545a5e231c886fe4262e8acd5adecddcac826ec。

R4 Cargo差分が共存するため、一時GIT_INDEX_FILEへreaderとHEAD Cargoのhooks1行だけを構成し、通常hook付きで保存した。
当該2entryのみreal indexへ同期し、source/Cargo生bytes不変、R2 staged patch100651bytes/3adba6c9...f42a不変を前後検証した。
正本: target/r1-reader-slice1-commit-20260908/manifest.json。Cargo.lockに今回の新package差分はなく、既存R4 path patch差分だけを保持した。

arrayの同TX追随・非永続fallback、ZIPのeffective order、MIH/classifier、manager/実caller通知接続はこのcommitの完了へ含めない。
前者2点を次のcoherentな区切りとして設計確認中。R4 portableと実機返答待ちは維持する。

### R1/R6 配列追随・ZIP effective orderの区切り（製品caller未接続）

focused commit: 69f33e6a957b6b9b27c241020173ecbf05cf6f16、src/similar_db.rs / src/similar_search_array.rs、783追加/30削除。
同TX metadataと整合する最高rank snapshotを選び、連続差分を取消可能に適用する。完全tieは既存MIH baseのArc identityを優先する。
最高rankの履歴不足、未来/store不一致/no適格候補は同TX全行からmemory-only baseを再構築し、取消/DB errorはそのまま伝播する。
ZIPはwriter/private readerで全key SQLと取消可能なstable merge sortを共有し、保存済みpage_index,item_id順を同点時に保持する。
stored orderがcurrentと異なるComplete ZIPに必要な本ごとのmapを作り、book行とtarget行へ同じordinalを適用する。
旧hash行は穴を予約し、current hashのindex Noneは全key ordinal付与後にwriter修復後と同じ適格行になる。quality0と実target keyも保持する。
同TX全keyに存在するはずのpage/targetのordinal欠落は黙ってNoneにせずerrorへ伝播する。readerは永続page_index/version/item_change/sidecarを書かない。

初期narrowのDB1/array tie・追随2/fallback2/取消各1はtool出力のみでlog原本未保存。重複実行せず初期成立確認に限定した。
最終suiteは試行ごとのtarget/r1-reader-follow-order-*へ保存した。

- db-tests1: 25成功/1ignored/0失敗、compile1分31秒、test0.18秒。
- array-tests1: 13成功/0失敗、test0.06秒。
- existing-page-order-tests1: 17成功/0失敗、test1.68秒。名前filterなので他featureも含み、上記との重複を除いた総数とはしない。
- zip-oracle-tests2: future versionケースを同fixtureへ追加後、対象1成功/0失敗、test0.02秒。
- check1: 製品cfg check成功、31.98秒。fmt-check1/diff-checkも成功。

独立coreは製品とテストを再レビューし、下記最終source SHAを承認した。sortは32比較で取消停止する。
delta fixtureは現処理順でvalidationと既存delta/mask複製を通過し、適用途中の146回目checkで取消を検証する。
cloneの各取消箇所を独立に実証するmutation testではなく、その範囲はcode review根拠と区別する。

- DB source SHA256=8466dc8025e1239e18e591d5215dcf817836dd802e1e55395a4c321dbc746bc2。
- array source SHA256=321f6533a4902c6fca9fe535d860d134a3f6b3fc67614929a6a65c5e773a2028。
- DB log SHA256=6c15e4e5ea64a17c187cef38d4cebc1a8dcf6b85a1d1311a918a49b94ac17ad5。
- array log SHA256=194a5fa1d0f6dfb39d5d86f1a1273a51df95fb37cbfdec48d565aeb52f23e1ab。
- page-order log SHA256=1c85f95f8e6e5af5a8883a271923489027df087d74aee494551af9a15e56e631。
- ZIP追加 log SHA256=acc0c67aa7955457fdfff7e657eb4a9bfb91dbff38ba95eb88646d2c2290eba6。
- check log SHA256=82951a913e53e053b8de26a293a910164f19b98d91d00fc85da7aff67bb264b0。

通常hook付きgit commit --onlyで当該2filesだけ保存し、前後のsource生bytesとR2 staged patch100651bytes/3adba6c9...f42aを照合して不変を確認した。
製品caller切替、MIH、common/分類/alignment/帯、最終全体gateとportable更新は未完了。既存R4 portableと実機返答待ちを維持する。

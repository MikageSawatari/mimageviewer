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

### R7/R9: 索引ジョブの終端とprefillの所有境界

R7の完成キャッシュは `NotIndexed` という照会結果を保持し、現在のジョブ進行状態に応じた
`Preparing` 表示は返値の投影とする。過去のRunning状態を結果に焼き付けない。

R9はscheduler存続中のDB ownerを固定してprefillとpruneの直列化を保証する案を検証する。
prefillはDB mutex取得後に最新scopeを短時間参照し、scope guardを解放してから書き込む。
UIのscope変更はDB待ちや署名生成を待たない。runを跨ぐ旧prefillとON/OFF連打で、prune後の復活がないことを検証する。

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

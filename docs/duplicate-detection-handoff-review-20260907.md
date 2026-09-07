# duplicate-detection 引き継ぎ・現状レビュー (2026-09-07)

対象: `C:\home\mimageviewer-dupe` / `duplicate-detection`。
レビュー基準: `65d13e58f0dcb343f035b314d5d3fcd3a23881af`。

現状の実装を引き継ぎ、まずレビューするという利用者依頼への記録。
**本記録時点ではアプリコードを修正していない。レビュー完了は統合可能の意味ではない。**
音声途切れは利用者が解消を実機確認済み。右パネルのロック解除は未修正。

## 1. 作業境界と体制

- 開始時の working tree は clean。未コミット変更なし。
- merge-base は `186e24b4d98bb7fc4e14c93b3d10293f77e0e0d9`。
- local master は `843fa4cc8`。`master...duplicate-detection` は master 側20、こちら側41コミット。
  相互に未包含で、`65d13e58f` は master に未統合。別 worktree の master や他系統は変更しない。
- 分岐点からの変更は48ファイル、約18,160行追加・244行削除。重要な経路を優先してレビューした。
- 設計・進行管理は親 Astra。独立 Astra / high がコアとUIを分担レビューし、Sol / xhigh が検証証跡を監査。
  通常実装とテストも今後は Sol / xhigh、まとまった変更は独立 Astra / high が設計の妥当性を含めてレビューする。
- 今回、レビュー担当はファイルを編集していない。親だけがこの記録と docs 索引を編集する。
  検証担当の再現用ファイルは `target/handoff-review-20260907/` に限定する。

## 2. 実装状況をコードと照合した結果

| 領域 | 状態 | 根拠・留意点 |
| --- | --- | --- |
| 正準画像入力・PDQ-256 | 実装済み | `src/similar_image.rs`, `src/dupe/proxy.rs`, `pdq.rs`。長辺2048、quality=0除外。保存済みサムネイルを署名の正本にしない |
| 独立した `similar.db` | 実装済み | `src/similar_db.rs`。現行schema=3、hash algorithm=1。v1移行とZIPページ順修復がある |
| お気に入りごとの索引設定 | 実装済み | `auto_index_similar`、既存supervisorのwatcher共有、設定画面の進捗・状態表示 |
| 索引の並列走査・取消 | 実装済み | `similar_index`。本単位staging→Complete公開、in-flight終了後の取消確定。詳細全分岐の再実行は未実施 |
| 単体画像照会 | 実装済み | 帯は距離≤8 / 9～48、originと候補をSQLiteで再検証。未作成・未収録・特徴不足等を区別。終端状態にR7の問題あり |
| 不変base + delta + snapshot | 実装済み | `similar_search_array.rs`。明示AUTOINCREMENT ID、revision、変更履歴、削除反映、非同期統合 |
| 本の関係・ページ帯 | 実装済み、要修正 | `dupe/book.rs`, `similar_index.rs`, `ui_metadata_panel.rs`。半径32、coverage=0.5、最低3ページ、common冊数上限8。R1/R5/R6参照 |
| 類似タブ・見開き両ページの照会 | 実装済み、要修正 | 移動・比較設定・長押し表示、状態案内と設定ボタン、表示スナップショット。R2～R4/R8参照 |
| 音声途切れ対策 | 実装済み、利用者実機確認済み | 専用低優先度pool、候補側走査省略。後者の正しさの説明はR1により撤回が必要 |
| §9.2 横断一覧 | 未実装 | 計画書§21.10末尾、`TopLevelGridSurface::Similar` はない |
| renameへの索引追随 | 意図して未実装 | 計画書§21.8。公開・stagingの複数path列を持ち、既存の1列descriptorでは不十分。移動後の再ハッシュを許容する判断 |
| master統合・統合後full gate | 未実施 | このレビューではmerge・commit・アプリ起動を行わない |

不変配列、変更履歴のtransaction、Complete公開、単体画像の再検証は設計と実装が対応している。
ただし、その契約を本照会にも適用できているという記述はR6のとおり正しくない。

## 3. レビュー指摘

行番号は上記レビュー基準コミットのもの。R2は利用者報告とsource inspectionが一致。
他は主としてsource inspectionによる指摘であり、実機で全件再現したという意味ではない。

### R1 [P1] 候補側の走査省略で、判定不能な本を「同じ」と過大評価できる

- `src/similar_index.rs:1540`～1570、計画書§21.11 (`docs/duplicate-detection-plan.md:1541`)。
- 「起点に近い候補ページなら、その周辺の文脈も起点走査で拾える」は距離の非推移性を無視している。
  距離 A–B=32、B–C=32、A–C=64 では、Aの半径32走査はBのcommon判定に必要なCを拾えない。
- 反例: A本の3署名を `[0x0f;32]`, `[0xf0;32]`, `[0xff;16]+[0x00;16]` とする。
  Bは各Aのbyte位置0,1,16,17をXOR ff、Cは各Bの位置2,3,18,19をXOR ff。
  Cの3ページを別々の9冊に置く。全署名は128個の1を持ち、qualityは正値、ページ順0～2。
- A起点の省略corpusではA/Bだけとなり、distinctive=3/3、matched=3、coverage=1/1で `Same`。
  全corpusではBの各ページが11冊に近く、8冊超のcommonとなるため `Undecidable`。
  `dupe/book.rs:333` は片方のdistinctiveが0なら判定不能とする。
- Solが `target/handoff-review-20260907/book_common_counterexample.rs` をrustcで実行し、
  **実際の `src/dupe/book.rs` を `#[path]` で読み込んだ分類でも Same / Undecidable を確認した。**
  これは判定関数への合成署名による再現であり、実画像からのPDQ生成やDB/UIを通す結合テストではない。
- **「誤差は必ず被覆率を低く見せる」という主張は成立しない。** 実店39冊で分類変更0という計測は、一般保証にならない。
- 修正境界: 本のcommon判定の情報を、索引内容世代と整合した形で取得する設計が必要。
  単に高負荷走査を復活させて音途切れを再発させる変更や、機能制限で回避する変更は採らない。
  DBへcommon情報を持つ案も、蔵書追加・削除・更新時の近傍変化を扱う必要があり、保存するだけでは解決しない。

### R2 [P2] 類似候補へ場所をまたいで移動すると右パネルのロックが解除される

- `src/ui_metadata_panel.rs:810` (`open_similar_hit`)、834 (`open_similar_book_page`)。
- 同じitems内なら `open_fullscreen` だけで維持する。一覧外では
  `snapshot_load_and_open` → `src/app/snapshot_ops.rs:1467` の `close_fullscreen` を通る。
  類似移動は `fs_nav_locked_gen` を持たず、`src/app.rs:54093`～54095 で本当の終了と判定される。
- `FullscreenInfoPanelState` は表示対象変更時にロックを保ち、真の終了時だけ解除する契約。
  UIにロックの保存・復元を足すのでなく、移動と退出の所有境界を修正する必要がある。
- 対象は画像候補と本のページ帯、通常フォルダと非同期ZIP/PDFの両方。
  真のEsc終了・取消・失敗・別viewerへの影響も確認する。
- 既存 `src/app/tests.rs:27829` 付近のテストは低レベルの状態と手動nav lockを検証するが、類似handlerを通らない。

### R3 [P2] サムネイル512件上限の退避順キューが埋まらない

- `src/ui_metadata_panel.rs:115`～121。完了時の `insert(...).is_none()` の場合だけ順序に登録する。
  通常要求は173～174行で既に `Loading` を登録するため、完了時は `Some` となる。
- `thumb_order` が空のままなので94～100行の削除が働かず、ページ送りでGPU textureが累積する。
- 回帰は要求→完了→上限超過→破棄の実際の遷移を通すこと。配列長だけの人工テストでは不十分。
  遅延完了・失敗・同一キーの内容更新もキャッシュ所有の一部として確認する。

### R4 [P2] パネル外で長押しを離すと元画像へ戻らない

- `src/ui_metadata_panel.rs:1557` の停止処理はパネル描画末尾だけ。
- ホバー表示で候補が映った後、押したままパネル外へ移動して離すと、1063～1068行の非表示returnで停止処理に到達しない。
  `PinnedNormal` が残り、パネルを再表示して初めて戻る。
- release/cancelを可視性から独立したownerに置き、非表示・focus喪失・終了・非同期ロード中のreleaseを検証する。

### R5 [P2] 一致256ページ超を「8冊超のcommon」と同一視する

- `src/similar_index.rs:1966`, 1996。`BOOK_PAGE_MATCH_LIMIT=256` 超過はDB検証前のページ件数で決まる。
- 互いに遠い3署名を各150回持つA/Bの2冊では、各ページ300件一致で全ページがcommonとなり候補なし。
  異なる冊数は2なので、正本 `dupe/book` の定義では除外されない。
- raw件数制限を冊数の証拠にしないこと。stale候補・ルーズ画像が上限を埋める場合も同じ境界で扱う。

### R6 [P2] 本照会が複数時点のSQLiteデータを混ぜる

- `src/similar_index.rs:1484` の起点、1497からのID解決、1527の候補読込は別snapshot。
  `src/similar_db.rs:999` と1022のAPIを通じて同一read transactionを所有していない。
- 索引公開が途中に入ると、起点の旧世代と候補の新世代を組み合わせられる。
  起点旧版だけが候補新版に似ている構成では、同一DB時点に存在しない一致を表示できる。
- 配列側revision/署名もID解決へ持ち越さず、署名不一致の厳密検証というコメントとも異なる。
- 単体画像の `verify_item_candidates` と同じ保証を本照会全体へ適用する設計が必要。
  走査中に公開世代を切り替える決定的テストを追加する。

### R7 [P2] 索引に入らなかった画像の「準備中」がキャッシュに残る

- `src/similar_index.rs:384`～385 はRunning中のNotIndexedをPreparingへ変換し、完成結果として保存する。
- 走査終了955行は本照会だけをclear。配列変更0件なら1034行以降でepochが変わらず、単体キャッシュが残る。
- 既存索引がある環境で、新規のデコード不能画像を索引中に開き、他の変更なしで完走した場合など。
- 内容変更とジョブ終端を区別し、終端後に未収録/失敗の実状態へ収束することをテストする。

### R8 [P2] 見開きの「長押し表示」と比較エンジンの制約が接続されていない

- `src/ui_metadata_panel.rs:950`～955 は見開きでもPinnedNormalへ入り準備する。
  一方 `src/ui_fullscreen.rs:32438` は見開き比較を拒否する。
- 見開きの左右候補には長押しボタンが出るが、候補が映らず、押している間に準備と拒否が繰り返される。
- 見開きから候補を一時表示する仕様を先に確定する。ボタン無効化による機能制限を無断で修正としない。

### R9 [P2] サムネイル署名のDB待ちがお気に入り操作を止め得る

- `src/similar_index.rs:3417` はenabled_rootsのread lockを署名生成とDBのput完了まで保持する。
  `src/similar_db.rs:345` のDB mutex/SQLite書込待ち中も保持する。
- UIからのconfigureは `src/similar_index.rs:748` で同じlockのwriteを同期取得するため、待ちがUIへ伝播する。
- OFF後の遅延prefillで削除済み範囲を復活させない契約は維持しつつ、scope変更と書込確定をworker側で整合させる。
  共有watcher/FTS設定変更にも到達するため、別バージョン索引だけの局所修正として扱わない。

### 継続調査: 本照会の未完了worker

`query_book` (`similar_index.rs:491`～510) は単一の本キーを保存し、別キーへ変わると新workerをspawnする。
本を照会完了前に切り替え続けると未完了照会が残る。受付・重複排除・取消・結果再利用の上限を検討する。
ただし **複数窓を放置しただけで毎フレーム交互にspawnする経路は確認されなかった**。
passive窓は保存された表示を描画しており、activeパネルとは異なる。負荷の実測は未実施。

## 4. 検証証跡

- 利用者確認: 音声途切れは解消。その他は一旦問題なしとの申告、ロック解除は再現あり。
- 今回実行: R1の反例を実判定関数で確認 (上記driver)。アプリは起動せず、trackedソースの変更もなし。
- 引き継ぎ会話: lib 7,559件 + snapshot 48件成功、commit後build成功。
  今回の監査ではその成功件数を裏付ける保存された実行ログを発見していない。今回再実行した結果ではない。
- `65d13e58f` のcommit本文には、reconciliation待ちが120秒で失敗し、今回の索引変更前にも発生していたとの記録がある。
  診断メッセージ追加は原因解決を意味しない。
- 320→375秒の全体テスト時間増加は会話記録上の未調査事項。常駐poolが原因とは確定していない。
- `target/dev-runtime/mimageviewer-core.exe` は存在するが、mtimeは2026-09-07 02:58:37で、
  HEADのcommit日時21:24:35より早い。この情報だけでは現HEADのビルド成功を証明できない。
- 今回は実機UI・full gate・通常verification buildを実施していない。アプリ挙動を変更していないため新ビルドは不要。
  実プロファイルや既存portable-devは起動していない。

## 5. 旧指示の移行対象

| 旧文書 | 衝突または陳腐化 | このタスクでの扱い |
| --- | --- | --- |
| Step 5 brief:3 | 実装=Codex Sol、レビュー・検収=ClaudeCode | 現在の利用者指示により設計=親Astra、実装テスト=Sol/xhigh、独立レビュー=Astra/high |
| Step 5 brief:9 | 「コミットしないこと」 | 当時の委任終了条件。今回の現状レビューではcommitしない。将来の実装区切りは現行AGENTSと利用者指示を適用 |
| Step 5 brief:15 | 「移行コード不要」 | 実装はv1移行を採用。計画書§21.8に11.6時間の再生成を避ける根拠あり。古いbriefを再実行して索引を捨てない |
| 計画書:5、§2～6、§12、§19.5 | Step 1のみ完了、旧ハッシュ構造、削除/推奨テスト、本UI未実装等が残存 | 現行は§16以降とコードに照合する。履歴を無言で現行仕様としない |
| 計画書§21.11 | 候補走査省略の誤差方向を保証 | R1により保証を撤回して再設計。音途切れ解消の事実は維持 |
| CLAUDE.md:1954以降 | Claude側からcodex CLIへレビューを出す運用 | このタスクは明示された独立Astra subagentを利用。ユーザーに中継させない原則は維持 |
| AGENTS.md detached規則、CLAUDE.md:42、detached-rework-plan §2/§3 | ClaudeCode/Codex双方合意、ClaudeCode検収、検収を指示書照合に限定 | このタスク内で触る場合は親Astraと独立Astraが構造修正性を判断。設計自体もレビュー。§2、症状パッチ禁止、§11記録、他系統との排他は維持 |

この役割移行を他の進行中ブランチへ無断適用しない。共有運用文書の一括書換えは今回行わない。

## 6. 次の実装順序と統合条件

1. R1/R5/R6を一つの本照会設計の問題として整理し、正確性・速度・更新世代の契約を確定する。
   Solが主要前提を反例と実コードで検証し、矛盾があれば実装せず親へ戻す。
2. R2を類似の二入口から共有ナビゲーションまで追って修正する。
   `snapshot_load_and_open`、真のclose、通常/ZIP/PDF再open、snapshot、detachedの影響を明示する。
   共通機構に触るなら既存folder-navのテストとviewer間の独立性も検証する。
3. R3/R4/R8のUI状態・比較寿命、R7の索引終端、R9のworker境界を、それぞれ独立した変更単位で直す。
   `similar_index.rs`等の同一ファイルに複数担当を同時投入しない。R2と比較修正がapp/UI共有ファイルを触る順序も排他する。
4. 各単位で独立Astraレビュー→狭い回帰→必要な共有テスト。大きな共有変更とmaster統合では `scripts/test-full.ps1`。
   既存の不安定テストを無効化せず、発生時の状態を記録する。
5. 実装変更後はfmt、関連test、UI文字変更時glyphチェックを通して `scripts/build-dev.ps1` を実行する。
   native/release依存の変更ならAGENTSの該当build規則を適用。利用者に渡すまでのbuild成功を実ログで確認する。
6. 以下はレビュー時点の既定手順。**2026-09-07の利用者指示により、このdupeブランチの
   検証用成果物は従来の `target/portable-dev` に変更する。**
   `scripts/update-portable-dev.ps1` のdata保持経路を使い、`-Seed`は使わない。
   既存の `data` / `data-remote` を初期化せず、実行中の検証アプリは更新時に利用者が終了する。
   エージェント用の使い捨てsmokeと、利用者が蓄積したportable索引を混同しない。
   他ブランチの通常profileビルド規則は変更しない。
   通常profileを別途必要とする場合の起動は利用者のみ。installed/tray版を閉じてから
   `Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`。
   `%APPDATA%\mimageviewer` の実設定・データを更新し得る旨と、修正内容ごとの具体的確認手順を必ず添える。
   エージェント実機UIは `prepare-portable-smoke.ps1` による使い捨て環境のみ。
7. 修正・検証後に§9.2の追加とmaster統合を別の完了条件で管理する。他系統の差分を巻き戻さない。

今回のレビューは重要経路の監査であり、全18,000行の網羅検証、全実データの検索精度保証、
障害注入・多窓・IME・GPU・音声の実機検証を完了したものではない。

## 7. v3.6.0 統合記録 (2026-09-07)

§§1〜6 は `65d13e58f` を基準にした統合前レビューとして保持する。その後、引き継ぎ記録だけを
`b8d46a71f` に独立して保存し、リリースタグ `v3.6.0` の commit
`56d97632ce3eb20bb3cec0b61016a2b18b2b1dc4` を第2親として `--no-ff --no-commit` で統合した。
リリース後の master の commit は取り込んでいない。自動mergeは競合0で、手動の競合解消もない。

独立Astraレビューでは、release差分とstaged差分、およびfeature差分とrelease→統合結果の
patch-idが一致し、交差したsource 5ファイルでも双方の変更が保持されていることを確認した。
merge起因のP1/P2指摘はなかった。

初回 `scripts/test-full.ps1` は次の2理由で終了コード101だった。

- `app::metadata_ops::tests::every_pass2_read_is_preceded_by_a_cancel_check` は、作業treeの
  `src/app/metadata_ops.rs` がCRLFである一方、Rustの複数行literalがLFとして解釈され、
  `include_str!` のCRLF本文から関数終端を見つけられなかった。productコードは変えず、
  テストhelperの入口でCRLFをLFへ正規化し、実sourceから作ったLF版とCRLF版の両方に、既存の
  5 read存在検査とread間cancel検査を同じまま適用した。独立Astraレビューでテスト弱体化が
  ないことを確認し、狭域テストは1件成功した。
- `susie_integration` の3件は、このworktreeのignored `testdata` に実Susie pluginがなく
  `loaded 0 plugins` となった実行前提不足だった。別worktreeの既存testdataから `ifpi.spi`、
  `ifmag.spi` と対応するPI/MAG/BMP fixture 6ファイルを通常ファイルとしてローカルコピーし、
  狭域8件がすべて成功した。このignored fixtureはcommitへ含めない。

前提補完とtest-only修正後のfull gateは終了コード0で `PASS`。本体libは7,601件成功・失敗0・
ignored 38件、UI snapshotは54件、Susie統合は8件、vendor egui-wgpuは9件、vendor eframeは
15件すべて成功した。`cargo fmt --check`、UI glyph検査 (危険文字0) も成功した。
`scripts/build-dev.ps1` は通常feature set (portableなし) で成功し、
`target/dev-runtime/mimageviewer-core.exe` を生成した。エージェントはアプリを起動していない。
検証ログは `target/merge-v3.6.0-20260907/` に保存した。

この統合では、§3のR1〜R9および継続調査項目を修正していない。各指摘の優先順位、根拠、
修正境界は統合前レビューの記録どおりであり、今後の実装単位として残る。

# §1.329 外部ツールの {file_list} 設計案

2026-10-09、ライン C。**1.329初回実装は独立レビュー承認済み・mpv実機確認済み。対象順を利用者決定に合わせて表示順へ変更 (下記)**。
正本: [バックログ §1.329](next-release-backlog.md#1329-外部ツールへ渡すファイルの一覧を書いたリストファイルを渡す-file_list--利用者要望-2026-10-05)、
[外部ツール起動](external-tool-launch-plan.md) §4.4〜4.7。

## 決定済みとコード上の前提

以下の「現在」は実装前のコード照合記録 (R1〜R3)。要求構築の全走査と DirectOriginal の
早期 return を確認したうえで、決定済みの境界に沿って実装する。

- 利用者決定: **1 行 1 パス、UTF-8 BOM なし、CRLF 固定**。文字コード・改行の設定は作らない。
- `{file_list}` は「この起動に渡すファイルの一覧」。対象・順序・実体化ポリシーは `{files}` と同じ。
- `src/external_tool.rs` の `split_argument_template_with_default` / `build_request_for_files` は
  現在 `{files}` のみを認識。通常の登録ツールは `build_materialize_operation` →
  `start_materialize_launch_worker` → `run_materialize_launch_operation` を通る。
  `start_launch_worker` の ready 経路は関連付け等の既存起動用で、引数はすでに確定している。
- R2コード照合: `build_materialize_operation` (src/external_tool.rs:2327) は worker 開始前に
  全対象へ `materialize_target` を呼ぶ。同関数は `launch_target_item_index` (:2019) の
  `items.iter().position` を毎回呼び、RealFile は一致する arm がないので一覧末尾まで走査する。
  **現状のUI要求構築は軽量ではなく O(N×M)** (対象N、一覧M)。大量選択対応には下記の準備分割も必要。
- `src/materializer.rs` は `create_new` の予約 lease、起動前 drop 時の削除、
  成功後の process directory への移管、`keep_paths`、孤児回収を持つ。
  ただし現在の `PendingTempLease::finish` は画像再利用用の stamp / cache key を要求する。
  リスト用には**再利用しない一時成果物**の所有形を同じ管理内に設ける必要がある。
  既存画像 cache に架空の source key を登録しない。実行時の外部プレイヤー互換性は未確認。
  `materialize` の DirectOriginal 分岐 (src/materializer.rs:575) は directory 初期化前に戻り、
  `reserve_collision_path` (:1160) 自体は directory を作成しない。

## 受け渡し契約

| モード | 起動とリスト |
| --- | --- |
| Single | 対象 1 件、1 起動、1 行のリスト。2 件以上は既存どおり拒否 |
| Each | 成功して準備できた各対象を順に起動。**起動ごとに別ファイル**、それぞれ 1 行 |
| Batch | 成功して準備できた全対象を 1 起動へ渡す。1 リストに順序どおり全行 |

- **対象順は `current_grid_order()` の表示順** (利用者決定 2026-10-09)。現在項目を先頭へ移さない。
  `{files}` とリスト行、Each の起動要求は同じ順序。スタック内はページ順、見開きの両ページは読み順。
  実体化失敗は現在の件別通知を維持し、リストにはその起動へ実際に渡す準備済みパスだけを書く。
  0 件なら起動しない。リストの作成失敗で `{files}` へ黙って代替しない。
- 実ファイル・フォルダ・本そのものは既存 resolver が渡す元パス
  (RAR / EPUB 等の変換キャッシュでは元コンテナ)。ZIP / PDF 内ページ、
  一時コピー・編集焼き込み・合成見開き・動画の現在フレームは **materializer が返した実体のパス**。
  動画ファイルを渡す設定では元動画。仮想パス、コンテナ内 entry 名だけ、表示名は書かない。
  `OriginalFile` で渡せない仮想ページ等の既存拒否を変えない。
- リストの仕様案: 絶対パスを引用符・escape・コメント・ヘッダーなしで書き、**最終行にも CRLF**。
  Unicode を UTF-8 へ厳密変換する。非 Unicode の `OsStr`、CR / LF / NUL を含むパスは
  当該起動を理由付きで拒否し、`to_string_lossy()` で別のパスに変えない。
  空白・日本語・絵文字・UNC パスはそのまま 1 行にする。canonicalize による別 identity 化はしない。
- 引数は先に分割し、リストパスを 1 個の `OsString` 値として置換する。
  `{file_list}` の複数出現は同じリストを指す。`{files}` と同じトークンにあれば
  そのトークンは対象数ぶん展開し、各引数内のリストパスは共通 (独立トークンなら 1 引数)。
  未知記法は従来どおり literal、`{file}` は既存互換として `{files}` へ正規化する。
- 既知記法がない場合の `{files}` 自動追加は維持。`{file_list}` だけなら自動追加しない。
  Batch の検査と UI 説明は `{files}` **または** `{file_list}` を受け入れる。
  引数テンプレートがある `Executable` が対象。`Association` / `OsDefault` にリストを渡す新仕様は作らない。

## 所有・処理位置・制限

- リストの確定・検証・作成・書込み・flush / close は、登録ツールの**既存実体化／起動 worker**で行う。
  UI に残す対象列挙・編集snapshot構築も下記の準備段階で分割する。元ファイルだけの場合も UI でリストを作らない。
  ready 経路に将来テンプレート起動を載せる場合も、生成は launch worker 内に限定する。
- **リスト成果物API自身**が worker 内で `ensure_process_directory` (:1117) を通し、
  startup cleanup の完了待ち・親／process directory の作成と安全検証を済ませてから予約する。
  呼び手の画像実体化による偶然の初期化に依存しない。待ちは既存Condvarでworker内のみ。
  DirectOriginalだけでも、起動後最初の動画だけの要求でもこの契約を適用する。
- 実体化後、各起動の確定パス集合から `file-list-<一意番号>.txt` を `create_new` で予約する。
  メディア成果物と衝突しない名前を使い、全行を書いて handle を閉じてから既存の起動境界へ進む。
  同じ対象でもリストは再利用・上書きしない。既存 generation / cancel / UI launch ACK を共有する。
- spawn 成功までは起動要求所有。cancel・supersede・書込み／起動失敗は lease の drop で削除。
  Each の途中失敗は既に渡したリストを削除しない。成功したリストと参照する一時メディアを
  同時に process 所有へ移し、`keep_temp` は**双方**へ適用する。
- 保存先は通常 `%TEMP%\mimageviewer\ext-<pid>\`、portable は `<data_dir>\temp\ext-<pid>\`。
  終了時削除と次回起動の死んだ PID の孤児回収を流用し、reparse point を辿らない。
  外部プロセスの終了監視・独自削除タイマーは追加しない。
- 上限は展開後の対象件数で従来どおり (確認既定 5、上限既定 10、ツール別に変更可)。
  リスト独自の件数上限は提案しない。書込みは行単位で行い、全体連結バッファを増やさない。
  Windows コマンドラインの 32,767 UTF-16 単位上限 (終端 NUL 等込み) の検査は維持。
  リストだけなら対象パス列の長さを除けるが、`{files}` も併用すればその分の制限は残る。
  一時ファイル容量・通常のファイル／パス制約、受け手独自の形式・件数制約は回避できない。

## 大量選択のUI要求構築 (R2追加、既存入口共通)

1. resolver の一覧indexを捨てず、対象を `Listed{context_id, items_generation, index, source_key}` /
   `OutsideList{target}` の型付きlocatorで引き渡す。右クリック、固定スロット、ピッカー、
   viewer／再生中、コンテナ背景を同じ準備ownerへ揃える。ListedはO(1)でitemを参照し、
   indexの世代とtyped source identityを照合する。古いindexを別itemへ読み替えない。
2. RealFileの動画／音声／本／フォルダはページ編集用index探索を行わない。現在フレームは既存の
   再生source identityから解決する。Stack展開で一覧外になるメンバーはOutsideListを保持し、
   `stack_member_default_params` とworker側ページDB読取を使い、一覧を探し直さない。
   見開きの左右は既存の確定indexを保持する。indexのない旧内部入口でページ照合が必要なら
   1要求につき一度だけ分割してsource→index表を作る (O(M+N))。対象ごとの `position` は廃止する。
3. **準備モーダルは全対象snapshotを作る前に開始**する。外部起動要求の単一ownerに
   Preparing → Confirmation → Materializing → Launchingのphaseを持たせ、準備用bool／並行jobを足さない。
   Preparingは表示順の列挙、選択解決、Stack展開、必要なsource照合表、編集snapshotをcursorで進める。
   1フレーム最大128対象／走査entryかつ経過2msで区切り、次frameへrepaintする
   (設計上の予算案。単一対象の超過はperfで検出)。全件clone／collectをモーダル開始前に置かない。
   ツール・共通設定・AI/LUT等の不変snapshotは要求内で共有し、UIからI/Oや画素生成は行わない。
4. Preparing中の選択・ナビ変更は既存modal入力抑止へ登録する。所有contextのitems mutation／
   context close／明示cancel／supersedeで準備を破棄し、snapshotの取り直しretryはしない。
   別contextの世代変化では取消さない。全件の具体的sourceが確定した後は既存起動契約を維持する。
   件数確認はStack／見開き展開後の正確なNで従来閾値を適用し、確認前に実体化／起動は開始しない。
   上限を下げる・選択を切り捨てることでUI負荷を解決しない。

## 単純化と利用者決定

準備snapshotが終わるまでの割り込みはモーダルで減らす。既存の準備モーダル・単一 generation・
要求 leaseを拡張し、別worker／削除owner／再利用cacheを作らない。準備をworkerへ丸ごと移す案は、
viewerのlive編集状態をworkerに読ませるか巨大な同期cloneが必要になるため不採用。
ディスク書込み失敗には既存の失敗通知と log を使い、再試行・途中ファイル復旧は持たない。

**C329-1 決定 (利用者 2026-10-08):** 推奨案を採用。ヘッダーなし・引用符なし・最終行も CRLF の
`.txt` とし、表現できないパスは当該起動を拒否する。固定フォーマットと正確なパスを守る。
件数上限・寿命・モード別意味は決定済みで再質問しない。
R2で新規の利用者質問はない。要求構築とdirectory初期化は既存仕様を成立させる実装上の修正。
2026-10-08時点で未回答の利用者質問はない。

## 対象順の変更 (利用者決定 2026-10-09、今夜のリリース対象)

- 利用者のmpv実機確認で `{file_list}` (`--playlist=`) と `{files}` のBatchを確認できたが、
  最後の項目から発火すると「3 → 1 → 2」になった。複数チェック対象は **表示順「1 → 2 → 3」**
  に統一する。右クリック・固定スロット・ピッカーで同じ規則を使い、選択なしの単一現在項目は維持する。
- Batchの引数／リスト行とEachのspawn呼出し順を共通の準備順から作る。外部プロセス同士は並行し、
  相手アプリの読み取り／表示完了順まで保証するものではない。Stackのmember順、両ページのreading順は変更しない。
- 簡素化: 「現在項目を先に処理した」という段階を廃止し、列挙cursorだけで準備を進める。
  唯一のチェック対象が現在項目なら、順序が一意なので既存のO(1)解決を保ち、大量一覧の走査を増やさない。
  新しい選択順の状態・設定・workerは作らない。選択／クリックした順で渡したい要望は、利用者からの要望が来た時点で検討する（バックログには載せない、利用者判断 2026-10-09）。
- 回帰: 非index順の表示一覧と末尾の現在項目から、右クリック／固定スロットの準備→既存worker→fake launcherを通す。
  Each／Batchと `{files}`／`{file_list}`／併用で実際の引数・CRLFリスト内容・起動要求順を検査する。
  既存の大量準備／Stackページ順／見開き読み順も検証する。1.329は未出荷で、移行・互換設定は追加しない。
  1.356に依存するコードには触れず、この順序変更だけをmasterへ取り込める差分にする。

## 実装後の受け入れ・文書

バイト列 (BOM 不在／全改行 CRLF／末尾 CRLF)、Unicode・UNC・空白、厳密変換拒否、
Single / Each / Batch、混在プレースホルダ、Batch 上限／長さ、連続起動での内容不変、
失敗／取消／成功移管、keep_temp と通常／portable 保存先を非起動の unit／fake launcher で検証する。
追加回帰: 起動後最初の動画のみ／全件DirectOriginalでリスト作成成功、初期化失敗で非起動、
大規模M・NでRealFileのitem探索0回／Listedの直接参照／全体O(M+N)の処理件数、
準備が複数frameに分割されframe予算を守ること、各入口の順序・Stack／見開きの件数、
snapshot途中の同context取消しと別context不変をfake入力・計数・perf検査で確認する。
準備phaseの所有と既存確認／workerへの受け渡しも実装前の独立レビュー対象。
互換性確認は利用者が受け手プレイヤーを操作する (mIV 終了後に読むなら keep_temp が必要)。
実装時は外部起動正本、spec、環境設定の記法説明、manual/external-tools.html、製品ページを更新し、
保存先の記述は privacy.html と突き合わせる。

R1レビューのP2「UI要求構築の同期全走査」とP3「元ファイルのみのdirectory初期化」を
コード照合して採用し、次の独立レビューで対応確認済み。R3レビュー結果はACCEPT (利用者回答待ち)、
利用者は2026-10-08にC329-1を採用した。初回の設計作業は文書のみ、続く実装指示で §1.329 を実装する。


## 実装記録 (2026-10-08、自動検証完了)

- `ExternalOperation` が Idle / Preparing / Picker / Confirmation / Materializing / Launching を
  所有する。旧 worker の Vec は完了通知の drain 用で、並行した準備状態にはしない。既存の
  `edit_request_owner_context` / `with_owner_viewer_context` を使い、detached の predicate を追加しない。
- 起動 / ピッカーの開始要求は context / generation / source descriptor のみ。右クリックメニューの
  可否判定は別の参照 summary、準備は `ExternalPreparationBudget` の 128 entry / 2ms と
  同 frame の再入防止で区切る。Listed の直接参照、RealFile の探索省略、必要時のみ一度の
  page / Stack index を使い、共通設定・AI 材料・LUT map の不変 snapshot を要求内で共有する。
- worker の `PreparedInvocation` は引数・リスト lease・参照メディア index を持つ局所状態。
  **全リストの書込み / close と引数検査を UI launch ACK より前**に済ませる。Each は起動ごとの
  1 行リスト、Batch は準備成功分の全行。ACK 取消 / 世代置換では双方の要求 lease を drop し、
  spawn / Invoke 成功時だけ参照メディアとリストを process 所有へ移す。
- `MaterializeSession::create_file_list` は自分で process directory を初期化し、一意名を予約する。
  `PreparedFileList` は stamp / cache key を持たない成果物で、既存の request lease / keep_paths /
  孤児回収を使う。keep_temp は終了時だけ残し、次回起動時の死んだ PID の回収は維持する。
- 有効な red: HEAD `3a0409e86` の実際の `launch_target_item_index` に観測 counter だけを付け、
  M=4,096 / N=128 の RealFile で **524,288 probes (期待0)** を確認した。焦点回帰1件が assertion
  failure / exit101。診断後は保存した実装ファイルを自動復元した。記録: `target/C-1329-red-proof.txt`。
  修正後は大規模準備の entry 数・frame 数・空のページ照合表・Listed 編集の直接参照で検証する。
- 本体・manual・spec・architecture・UI responsiveness・privacy を同時更新する。設定の項目や
  DB schema は追加しない。確認用の受け手と手順は `target/C-1329-capture-list.ps1` /
  `target/C-1329-manual-check.md`。製品の起動と受け手プレイヤー互換性の確認は利用者が行う。


### 自動検証結果 (2026-10-08)

- 焦点: external_tool 73 / materializer 45 / creative_lut 14 / context_menu 61、
  計193件成功。全libは `cargo test -p mimageviewer --lib` をpipeなしで実行し、
  **10,986件成功、52件ignore、失敗0、exit0** (1,134.98秒)。
- `cargo fmt` / `cargo fmt --check`、通常／portableのcore check、glyph lint
  (危険glyph 0)、`git diff --check` が成功。CRLFを維持し、numstatに全体改行変換はない。
- 記法説明と準備bodyの明暗4枚を追加して目視確認し、更新モードを外した
  ui_snapshot全体115件が成功。回り続けるspinnerは既存の固定frame撮影を使う。
- 初回全libで、menu fixtureが既存Appの共有test lockを持ちながら別Appを作る
  再入待ちを検出して停止した。fixtureは既存Appと純粋なtest用descriptor構築を再利用する形に
  修正し、上記の焦点／全libを再実行した。製品の状態遷移やテスト期待値の回避変更はない。
- `CARGO_BUILD_JOBS=1 .\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0`
  はexit0。通常featureのdev-runtime core・Remote・EPUB workerを配置し、VCRT/PE検査も成功。
  core buildは11分24秒。製品を起動していない。
- 詳細記録: `target/C-1329-verification.md`。独立実装レビューと受け手互換性／実動作の
  利用者確認は未実施。利用者仕様の未回答質問はない。コミットは行っていない。

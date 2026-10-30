# コレクションの本から戻ると「違う場所が表示される」調査

調査日: 2026-10-30。作業ブランチ `collection-return`。

## 報告と対象

第三者の X 投稿「mImageViewer、コレクションの本から戻ると上にいっちゃう不具合は修正されてるけど違う場所表示されてるなぁ」が対象。依頼者から提示された引用が根拠。使用版・設定・件数・素材・どの「場所」かは不明で、投稿者の環境の原因を確定した診断とはしない。問い合わせ・返信は行わない。

「上にいっちゃう」は §1.454 と考えられるが推定。§1.454 は v4.9.0 で登録フォルダの厳密な子孫から物理親へ戻す修正が入っている。§1.475 の2026-10-29の別投稿は「本を閉じたときの保存が遅い」という調査であり、その実アプリ計測を今回の位置復元の証拠として流用しない。

- 基点 `b10dc7406`。製品コードは公開 `v4.10.0` と同じ。比較対象 `v4.9.0`。
- 追加は test-only module と調査記録。製品コード変更・製品起動・デスクトップ操作・commitなし。

## 試験条件

`src/app/tests/collection_return.rs` は `scroll-661` の調査テスト（`01af22d76`）の Full-frame Driver を基に作成。同ブランチは読み取り参照のみ。

- `phase_c_support::setup_app` の TempDir/data_dir/DB。素材と collection.db も同 TempDir。App/workerを先に破棄する。
- JPEG入り ZIP 200冊（通常1ページ、選択して開く本だけ32ページ）。画像フォルダ対照は PNG入りフォルダ200冊（通常1枚、対象32枚）。PDFは有効な2ページ PDF 200冊（対象だけ32ページ）。ZIPの1ページ本を詳細・サムネイルの対照として別に残す。
- 実 collection actor で作成・登録し、実走査・hydration・詳細取得・復帰・egui描画を通す。PDFは既存 `RemotePdfTestBackend` の同一プロセス PDFium 呼び出しで、製品 PDF workerを起動しない。guardはApp終了まで保持。
- `eframe::HeadlessEpiIntegration`、1280×800 logical points、tree表示、focus=false、viewport command破棄。起動先自動選択・更新確認・tray・名前索引/LUT初期処理を既存headless fixtureと同様に停止。
- RawInputには1/60秒刻みを指定するが、HeadlessEpiIntegrationのprepare_raw_inputが実経過時間で上書きする。worker待機フレーム間は16msで、固定60Hzではない。実窓/GPU提示/OS入力の測定ではない。
- Esc/BSは別のegui passへ合成キーを渡して実fullscreen handler→navigationを実行。閉じる相当は共通close request、親は実parent resolverを呼ぶ。実マウス/native focusは未検証。
- sidecar/tagのバックアップはOFF（既存setup_appのNoSidecar条件）。
- 自然完了順と、実replyのhydrationを戻り先の最初の描画前に完了させる `early` を分ける。偽の列挙結果・復元値は注入しない。CollectionSourceはframe前に同期採用される経路もあるため、frame前/後の両方で実replyをpollする。通常もpost_render_main_navigationの採用から同frame末尾pollまでに実replyが間に合えばこの順序になる。NoSidecarではindex受理からResumeDisplay/terminalまで同poll内で進む。
- `fresh` は診断対照のみ。世代切替時に新しいegui contextへ入れ替える。製品の修正案ではない。
- `settled` は詳細Ready/Disabled、保留metadataなし、App offsetが4フレーム不変という観測点。egui速度の完全停止を意味しない。
- current/effective folder、address、tree active path、collection position、selected、folder_history、App offset、egui ScrollArea State（offset/max/velocity）、items generation、件数・高さ・hydration・詳細状態を記録。

## 結果と証拠

**開発側の headless 試験で (c) を再現した。(a)(b)(d) は全20件で正しい。投稿者の実機の原因と確定した意味ではない。**

| 候補 | 結果 |
|---|---|
| (a) 戻り先のフォルダ | 登録元の `zip-200` へ正しく戻る。直下本は同じ collection ID/root へ戻る |
| (b) 選択項目 | 下部の `0199.zip` / `0199-folder` / `0199.pdf`（index 199）を保持。先頭対照は index 0 |
| (c) スクロール位置 | 保存・復元は正しく、その後の入力なし描画で上へずれる。詳細・サムネイル双方で再現 |
| (d) アドレス・ツリー | 戻り先の実パスとtree active pathが一致。collection rootはcollection名とactive pathなし。状態assertとheadless描画による確認 |

### 再現条件と対照

戻り元の32ページ一覧がスクロール可能で、戻り先の長い一覧より小さい上限を持つこと、保存位置の復元が最初の戻り先描画に間に合うことが今回の再現条件。NoSidecarの実replyをその順序で完了させた試験は、形式・通常フォルダ/collection root/登録フォルダ内を問わず (c) に失敗した。Esc→BS、全画面BS→一覧BS、閉じる相当、親ボタンのresolverを通る経路で確認した。

- 自然完了順ではZIP詳細・サムネイルとも保持。最初に長い一覧をoffset 0で描画し、その後に復元すると旧上限が既に更新されている。
- 1ページ本は両表示とも保持。旧 `content_is_too_large.y=false` のためばねが無効になる（`scroll_area.rs:923`）。「短い一覧」だけでは十分な条件ではなく、短い側もスクロール可能かを区別する必要がある。
- 先頭の本からは0を保持。旧上限の外へ復元しないため、このばねを発火させない。
- Appの復元・実workerを維持し、旧egui状態だけを引き継がないfresh対照は両表示で保持。
- 直接全画面からの閉じる相当も、今回の実経路では (c) に失敗。入場途中の短いページ一覧のScrollArea状態が残っていることをログで確認した。直接開く設定なら常に短い一覧が描かれない、とは扱わない。

### 全件結果

最終ソースに固定して **7 PASS / 13 FAIL、実行46.08秒、exit 101**。13件すべて最後の `(c) restored viewport moved` assertで失敗し、フォルダ・選択・address/tree・保存historyのassertは通過。compile/harness failure、timeoutはない。各位置はlogical points。フレーム経過時間に依存し、ずれる行数は固定ではない。

| テスト名（`collection_return::` 以下） | 保存→観測位置 | 結果 |
|---|---:|---|
| `collection_image_folder_details` | 5040 → 1876 | FAIL (c) |
| `collection_image_folder_thumbnail` | 11760 → 2450 | FAIL (c) |
| `collection_one_page_details_control` | 5040 → 5040 | PASS |
| `collection_one_page_thumbnail_control` | 11760 → 11760 | PASS |
| `collection_pdf_details` | 5040 → 1820 | FAIL (c) |
| `collection_pdf_thumbnail` | 11760 → 2205 | FAIL (c) |
| `collection_zip_details_auto_close` | 5040 → 280 | FAIL (c) |
| `collection_zip_details_bs` | 5040 → 1820 | FAIL (c) |
| `collection_zip_details_fast` | 5040 → 1792 | FAIL (c) |
| `collection_zip_details_fresh_control` | 5040 → 5040 | PASS |
| `collection_zip_details_natural` | 5040 → 5040 | PASS |
| `collection_zip_details_parent` | 5040 → 1792 | FAIL (c) |
| `collection_zip_details_top` | 0 → 0 | PASS |
| `collection_zip_thumbnail_fast` | 11760 → 1715 | FAIL (c) |
| `collection_zip_thumbnail_fresh_control` | 11760 → 11760 | PASS |
| `collection_zip_thumbnail_natural` | 11760 → 11760 | PASS |
| `ordinary_zip_details` | 5040 → 1792 | FAIL (c) |
| `ordinary_zip_thumbnail` | 11760 → 3430 | FAIL (c) |
| `root_zip_details` | 5040 → 1204 | FAIL (c) |
| `root_zip_thumbnail` | 11760 → 4900 | FAIL (c) |

### 保存・復元と描画の記録例

`collection_zip_details_fast`。選択は復帰後ずっと `0199.zip`（199）、保存値は `(5040, Some(199))` のまま。元一覧generation 3 → 本generation 4 → 戻り先generation 5。戻り先の描画・詳細取得中のitems再生成はない。

| 時点 | App offset | egui実offset / max | egui Y速度 | generation |
|---|---:|---:|---:|---:|
| 元一覧で末尾選択・退出前 | 5040 | 5040 / 5040 | 0 | 3 |
| 本の32ページ一覧 | 0 | 0 / 336 | 0 | 4 |
| frame 40、実reply受理・復元後、最初の描画前 | **5040** | **0 / 336（旧状態）** | 0 | 5 |
| 同frame、最初の描画後 | **4984** | **4981.5 / 5040** | **-6636.4** | 5 |
| frame 85、移動後の可視範囲の詳細取得が再びReadyへ | 1792 | 1779.2 / 5040 | -517.2 | 5 |
| frame 88、詳細Ready＋App offset 4frame不変 | **1792** | **1780.5 / 5040** | -446.6 | 5 |

current/effective folder、address、tree active path は復帰後すべて同じ一時 `zip-200`。collection positionは登録した同じ `PhysicalSource`（登録folderのentry ID/source key/path）を維持。pending edgeはNone、ensure-visibleは復元後true→最初の描画でfalse。frame 40は詳細Ready、移動した可視範囲の取得がframe 41にLoadingとなり、frame 85に再びReadyへ戻る。最初の位置変化はviewport 587の最初の描画で発生し、下部進捗バーによる563への変化（frame 42）より前である。

### コマンドと証跡

```powershell
$env:MSBUILDDISABLENODEREUSE='1'
cargo test -j 1 -p mimageviewer --lib collection_return:: --locked --offline -- --test-threads=1 --nocapture
cargo fmt --all -- --check
git diff --check
```

- 生ログ: `target/collection-return-final.log`。全結果抽出: `target/collection-return-results.json`。
- 実行記録: `target/collection-return-final-evidence.json`。source開始/終了SHAが一致。全体248.76秒（コンパイル約3m23s＋実行46.08s）。
- テストsource SHA-256: `8e09dc44afaca9454ebb5957c5ea1fb9e831e6a0e8e383e7f7543d5dfc69bd58`。
- test binary SHA-256: `641182e58a1101386d7524fa607fd8a7eb7c31399476b439dd26258d55fef5fe`。
- `cargo fmt --all -- --check`: PASS（`target/collection-return-fmt-final.log`）。`git diff --check`: PASS。
- 独立 Sol/xhigh reviewerがsource・fixture・実行証跡を読み取り検収しACCEPT。詳細Readyの観測順序の表現をログに合わせて訂正済み。reviewerはCargoを再実行せず、同じ実行結果を再利用した。
- 初回cold buildはturbojpeg-sys並列ビルドで失敗。`-j 1` 再実行で通過。全Cargo実行はMSBUILDDISABLENODEREUSE=1。
- 先行1ページfixtureの17PASS/1timeout、およびコンパイル中にsourceを変えた実行は最終結果に混ぜない。timeoutはAutoClose直前に画像folder自動表示設定を変えたための再走査競合候補で、最終fixtureは入場前から設定を固定し解消。最初のearly待機もframe後だけだったため同期採用を捉えず、frame前pollを追加した。これらを製品の失敗とは数えない。

## 原因・所有境界・修正方針

守るべき条件は「現在一覧に正しく復元した位置を、入力のない描画が旧一覧の制約で変更しない」こと。違う場所を作る所有者はCollectionの帰り先やDBではなく、**Appのcanonical offsetとeguiが保持する旧bounds/速度の適用境界**。

1. `src/app/collection_grid.rs:1213` の親判定と `src/app/close_return.rs` のclose resolverは正しい登録folder/子の選択を返す。
2. `src/app.rs:42307` の退出保存と `:43111` の復元は正しい。history読取 `:43578`、offset復元 `:43612`。ここで5040/199へ戻っている。
3. 両rendererは現在の件数とviewportで正しいoffsetを計算し、詳細 `src/ui_main.rs:17608`、サムネイル `:20052` からeguiへ渡す。
4. `vendor/egui/src/containers/scroll_area.rs:823` はoffsetだけを受け取る。`:919` は旧max、`:923` は旧content_is_too_largeでphysicsを構築し、`:994` のstepが新しい復元位置を旧範囲のoverscrollとして処理する。ばねは `scroll_physics.rs:124`。**現在一覧の上限が確定する `scroll_area.rs:1671` より前に位置と速度が変わる。**
5. mIVが実offsetを読み戻し（詳細 `src/ui_main.rs:17733`、サムネイル `:20432` 付近）、上へ変わった値をcanonical offsetへ採用する。選択と履歴は正しいまま。ensure-visibleは消費済みなので選択行は画面外へ出る。

**>>661と同じ機構**。今回特定した追加条件は「短い側が実際にスクロール可能」であること。collection固有のparent/navigation修正を追加する必要は今回の証拠からは見当たらない。

修正方針・規模は>>661で進行中の共通修正へ集約する。現在geometry、位置更新の意味（同期/明示復元/App gesture）、一覧と入力の寿命を同じ境界で扱い、旧bounds/速度を新しい復元へ適用しない。詳細/サムネイルの共通scroll ownerとvendored eguiのopt-in境界に及ぶ中規模修正。DB/schema/Collection保存形式の変更は不要。遅延・毎frameの強制復元・復元無効化・ばね全体の無効化は提案しない。

`scroll-661` の進行中修正へは今回のtest moduleを移植して確認できる。旧field参照（offset/reveal/pending edge）だけ同branchの `GridScrollState` 呼出しへ合わせ、fixture、実worker、合成キー、20件の期待値を維持する。現在13件の(c)失敗がgreenへ変わり、7件の対照が維持されることを受入条件にする。同branchは変更しておらず、**今回のCollection経路で修正適用後に通ることは未実行**。旧egui状態を除いたfresh対照が通ることと、修正branch自体で通った既存>>661試験の証拠は区別する。


## 版の比較

比較記録: `target/collection-return-version-comparison.json`。v4.9.0とv4.10.0で次の関数本文は一致: `save_leaving_folder_grid_position`、`resume_loading_items_after_sidecar`、`apply_scroll_to_selected`、`poll_details_meta_load`、`handle_fullscreen_close_request`、`resolve_grid_parent_nav`、`open_grid_container_with_mode`、`collection_grid_parent_navigation`、`resolve_close_return_navigation`。eguiの `scroll_area.rs` と `scroll_physics.rs` も全文一致。v4.9.0への今回の追加テスト実行とソース比較は区別する。

v4.9.0自体で今回の追加テストは実行していない。上記のソース同一性を、同版のruntime結果とは扱わない。

v4.10.0の `accept_grid_container_open` は入力の共通化。フォルダ対照の入口に使用。v4.9.0へ移植する場合は従来のフォルダ入場handlerへの接続が必要。

## >>661共通修正での受入（2026-10-30）

設計担当が `scroll-661` へ `da7bd5aaa` で20試験を取り込んだ。
旧offset／edge／reveal fieldと最後のgeometry参照だけを `GridScrollState` のaccessor／要求入口へ適合した。
fixture、実worker、キー／close経路、全期待値とcase tableは維持し、独立reviewerも機械照合した。

共通修正の最終focusedは **45 PASS**（27.192s）で、Collection **20件すべてPASS**。
元の13件の(c)失敗はgreenとなり、7件の対照も維持した。
さらに最終本体fullでも20件全件を含む **14,993 PASS / 0 FAIL**（536.504s）を確認した。
ログは `target/scroll661-stage-c-final-focused-2.log`、case名／件数抽出は
`target/scroll661-stage-c-focused-results.json`、fullは `target/scroll661-stage-c-test-lib-full-2.log`。
共通原因の修正以外にCollectionの帰還先／DB／キー処理は変更していない。
他gate・既存の特殊混在入力§1.487の切出しは [>>661最終台帳](scroll-661-investigation.md#stage-c-final-verification) を参照。
実窓／投稿者環境では未確認で、上記の修正適用前の失敗記録と区別する。

## 未確認の範囲

投稿者の使用版・具体的操作・素材・設定、実窓/実画面、OSのfocus、DPI/複数画面、Remote/F12、NAS、バックアップON、改名・削除・並べ替えを伴う復帰、filtered/grouped一覧は未確認。件数・ページ数の境界探索は行っていない。test-onlyの調査なので全体gate・verification binaryは作成しない。

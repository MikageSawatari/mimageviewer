## 1. サマリ

- **不一致: 14 件**
  - 主因が文書の陳腐化: 12 件
  - コードが設計上の不変条件に違反: 2 件
- **リファクタ候補: 7 件**
  - P1: 2
  - P2: 4
  - P3: 1
- **バグ: 2 件**
- ファイル変更、ビルド、テスト実行は行っていない。コードと既存テストを静的に監査した。最終確認時の `git status --short` は空だった。

重点項目の結論:

- 通常の 2D 表示順序は、現行コードでは  
  `raw → erase → local adjustment → conceal → overall color → final AI → smart sharpen → colorize → Creative LUT → post-filter → annotation`。crop は edit-result に含まれず、出力段階で扱われる。
- 通常 2D の resolver では、補正変更後に旧 `ai_upscale_cache` が完成済み final composite より直接優先される分岐は確認できなかった。edit/final のキーと世代無効化も概ね整合している。
- 一方、**360° panorama には、完成済み final composite がまだない間、旧 adjustment/legacy AI キャッシュを暫定採用する経路がある**。色付け・Creative LUT などの変更直後に旧映像が一時表示される構造は存在する。ただし現行文書にも明記されたフォールバックなので、確認済みバグではなく P2 の設計見直し候補とした。
- 通常の edit/final/conceal キャッシュは viewer context に格納されているが、ローカル調整の bypass/prefix preview キャッシュだけは context 境界から漏れている。

## 2. 不一致リスト

### 1. `local-adjustment-layer-v1.1.0-plan.md` が実装済みなのに Draft のまま

- 文書: [local-adjustment-layer-v1.1.0-plan.md:3](<../local-adjustment-layer-v1.1.0-plan.md:3>) は `Status: Draft`。一方、同文書後半と [docs/README.md:56](<../README.md:56>) は統合済みとしている。
- コード: [local-adjust-core/src/lib.rs:116](<../../crates/local-adjust-core/src/lib.rs:116>)、[app.rs:49313](<../../src/app.rs:49313>)。
- 判定: **文書が古い**。
- 修正案: Status を Implemented に変更し、当初案と現行仕様を分離する。歴史的提案部分を archive 化するのが安全。

### 2. 同計画書の合成順序・キャッシュ優先順位が現実装より一世代古い

- 文書: [local-adjustment-layer-v1.1.0-plan.md:52](<../local-adjustment-layer-v1.1.0-plan.md:52>)、[同:630](<../local-adjustment-layer-v1.1.0-plan.md:630>) は AI、全体補正、post-filter、erase、local、conceal を古い単一パイプラインとして記述。
- コード:
  - edit-result: [app.rs:49310](<../../src/app.rs:49310>)
  - final composite worker: [app.rs:4778](<../../src/app.rs:4778>)
  - crop 除外: [app.rs:49341](<../../src/app.rs:49341>)
  - edit key: [app.rs:4411](<../../src/app.rs:4411>)
- 判定: **文書が古い**。コードは現在の edit/final 分離設計と整合。
- 修正案: §§3/8 を `EditResultKey` と `FinalCompositeKey` の二段構成へ全面更新する。

### 3. ローカルレイヤー／マスク／effect のデータモデルが古い

- 文書: [local-adjustment-layer-v1.1.0-plan.md:98](<../local-adjustment-layer-v1.1.0-plan.md:98>) の `id: Uuid`、限定的な effect/mask enum。
- コード:
  - 現行 layer: [local-adjust-core/src/lib.rs:116](<../../crates/local-adjust-core/src/lib.rs:116>)
  - 現行 mask: [同:196](<../../crates/local-adjust-core/src/lib.rs:196>)
  - effect enum: [同:821](<../../crates/local-adjust-core/src/lib.rs:821>)
- 判定: **文書が古い**。現行 enum は `None` を含む 108 variant、実 effect は 107 種。
- 修正案: 擬似コードを「初期案」と明記するか、現行型から生成した一覧に置き換える。

### 4. ローカル調整 DB スキーマが実装と違う

- 文書: [local-adjustment-layer-v1.1.0-plan.md:589](<../local-adjustment-layer-v1.1.0-plan.md:589>) は layer ごとの行と `layer_id` を提案。
- コード: [local_adjust_db.rs:31](<../../src/local_adjust_db.rs:31>) はページ単位の `page_path / layers_json / updated_at`。取得も Vec 全体を JSON 復元する [同:57](<../../src/local_adjust_db.rs:57>)。
- 判定: **文書が古い**。
- 修正案: 現行のページ単位 JSON スキーマ、sidecar との優先関係、移行責務を記載する。

### 5. 「サムネイルには反映せず badge のみ」が古い

- 文書:
  - [local-adjustment-layer-v1.1.0-plan.md:569](<../local-adjustment-layer-v1.1.0-plan.md:569>)
  - [conceal-feature-plan.md:1110](<../conceal-feature-plan.md:1110>)
- コード:
  - edit preview snapshot 作成: [app.rs:47061](<../../src/app.rs:47061>)
  - 保存 worker 投入: [app.rs:47118](<../../src/app.rs:47118>)
  - thumbnail loader の preview 利用: [thumb_loader.rs:945](<../../src/thumb_loader.rs:945>)
- 判定: **文書が古い**。
- 修正案: 通常の raw thumbnail と、local/mask/conceal/crop/annotation を含め得る永続 edit preview の二経路を区別する。

### 6. conceal の保存先・DB schema が古い

- 文書: [conceal-feature-plan.md:127](<../conceal-feature-plan.md:127>) などは `settings.json`、[同:641](<../conceal-feature-plan.md:641>) は `vectors_json NOT NULL`。
- コード:
  - 正本は `settings.db`: [settings_db.rs:3](<../../src/settings_db.rs:3>)
  - conceal schema は `shapes TEXT`: [conceal_db.rs:43](<../../src/conceal_db.rs:43>)
- 判定: **文書が古い**。
- 修正案: `settings.db` と実際の nullable column、bitmap/vector の保存形式を正確に記載する。

### 7. conceal tool が「8 種」で固定されている

- 文書: [conceal-feature-plan.md:78](<../conceal-feature-plan.md:78>)、[同:1198](<../conceal-feature-plan.md:1198>)。
- コード: [conceal.rs:202](<../../src/conceal.rs:202>) は `Polygon` を含む 9 種。
- 判定: **文書が古い**。なお [conceal.rs:12](<../../src/conceal.rs:12>) のソースコメントも「8 種」のまま。
- 修正案: Polygon とショートカットを追加し、Shape enum と raster bitmap の違いを明示。ソースコメントも別途修正対象。

### 8. conceal の worker 実行約束に対し、UI thread で同期合成している

- 文書:
  - worker・cancel・progress を要求: [conceal-feature-plan.md:1129](<../conceal-feature-plan.md:1129>)
  - 同じ文書後半は worker を将来課題扱い: [同:1232](<../conceal-feature-plan.md:1232>)
  - UI thread の I/O・重処理禁止: [ui-responsiveness.md:13](<../ui-responsiveness.md:13>)
- コード:
  - 同期実装である旨のコメント: [conceal_compose.rs:288](<../../src/conceal_compose.rs:288>)
  - DB load、raster、compose、texture upload: [app.rs:49153](<../../src/app.rs:49153>)
  - local layer の同期 SQLite/JSON load: [app.rs:22259](<../../src/app.rs:22259>)、[local_adjust_db.rs:57](<../../src/local_adjust_db.rs:57>)
- 判定: conceal は **コードが responsiveness 不変条件に違反**。文書内にも矛盾がある。実際の停止時間は未計測。
- 修正案: バグとして報告し、generation/cancel を持つ worker 所有へ移す。遅延や閾値ガードだけの症状パッチにはしない。

### 9. effect 候補文書の「106 種」が古い

- 文書: [local-adjust-filter-candidates.md:71](<../local-adjust-filter-candidates.md:71>)。
- コード: [local-adjust-core/src/lib.rs:821](<../../crates/local-adjust-core/src/lib.rs:821>) は実 effect 107 種。`Repair` は [同:881](<../../crates/local-adjust-core/src/lib.rs:881>) にあるが文書にない。
- UI catalog: [local_adjust_catalog.rs:89](<../../src/local_adjust_catalog.rs:89>)。
- 判定: **文書が古い**。
- 修正案: 107 種へ更新し `Repair` を追加。可能なら enum/catalog から検査可能な一覧を生成する。

### 10. 恒久正本の normal final-stage 記述が節によって異なる

- 文書:
  - 正しい順序: [preset-and-adjustment.md:203](<../preset-and-adjustment.md:203>)
  - colorize/LUT が抜ける節: [同:629](<../preset-and-adjustment.md:629>)
  - bypass 説明も Creative LUT が抜ける: [同:484](<../preset-and-adjustment.md:484>)
- コード:
  - final hash/bypass: [app.rs:3865](<../../src/app.rs:3865>)
  - final-only 差分判定: [adjustment.rs:496](<../../src/adjustment.rs:496>)
  - 実際の順序: [app.rs:4778](<../../src/app.rs:4778>)
- 判定: **文書が古く、同一文書内で不整合**。コード順序は文書冒頭および display-pipeline の新しい記述と一致。
- 修正案: final-stage の全節を smart sharpen、colorize、Creative LUT、post-filter の共通一覧へ統一する。

### 11. Book/headless 合成に smart sharpen/post-filter が入るという記述が誤り

- 文書:
  - [preset-and-adjustment.md:851](<../preset-and-adjustment.md:851>)
  - [display-pipeline.md:1070](<../display-pipeline.md:1070>)
- コード: [books.rs:1054](<../../src/books.rs:1054>) は erase → local → conceal → `apply_adjustments_fast` → comic。smart sharpen、colorize、Creative LUT、post-filter は適用しない。
- テスト: [books.rs:1811](<../../src/books.rs:1811>)、[同:1903](<../../src/books.rs:1903>) は display-only effect の除外を明示的に検証。
- 判定: **文書が古い**。コードとテストが意図された仕様。
- 修正案: Book/headless はすべての display-only enhancement を除外すると明記する。

### 12. `display-pipeline.md` が crop を edit-result に含めている

- 文書: [display-pipeline.md:731](<../display-pipeline.md:731>)、[同:1032](<../display-pipeline.md:1032>)、crop 変更時の edit/final clear は [同:1163](<../display-pipeline.md:1163>)。
- 正しい文書: [preset-and-adjustment.md:622](<../preset-and-adjustment.md:622>)。
- コード: [app.rs:49341](<../../src/app.rs:49341>) は crop を edit-result から明示的に除外。
- 判定: **`display-pipeline.md` が古い**。
- 修正案: crop を表示 overlay／最終出力の座標処理へ移し、edit/final cache 無効化表から外す。

### 13. `local-adjust-testing.md` の進捗表と参照行が古い

- 文書:
  - 冒頭では M6 が TBD: [local-adjust-testing.md:34](<../local-adjust-testing.md:34>)
  - 後半では Phase 3 実装済み: [同:332](<../local-adjust-testing.md:332>)
  - snapshot が将来予定: [同:126](<../local-adjust-testing.md:126>)
  - 後半では Phase 5 実装済み: [同:252](<../local-adjust-testing.md:252>)
- コード／テスト: [ui_adjustment_panel.rs:1160](<../../src/ui_adjustment_panel.rs:1160>) 以下に snapshot coverage がある。
- 判定: **文書が古く、履歴追記型になっている**。記載された `app.rs` の絶対行番号も現位置と大幅にずれている。
- 修正案: 冒頭 matrix を現況へ更新し、行番号ではなく関数名・テスト名を参照する。

### 14. ローカル preview cache が viewer context の所有境界に入っていない

- 設計不変条件: [CLAUDE.md:66](<../../CLAUDE.md>) は items/cache/texture/queue を owning context ごとに管理するよう要求。
- コード:
  - `ViewerContextBundle`: [app.rs:1921](<../../src/app.rs:1921>)
  - 通常の local cache/pending は bundle 内: [同:2140](<../../src/app.rs:2140>)
  - bundle swap: [同:12384](<../../src/app.rs:12384>)
  - bypass/prefix preview key は source/context identity を持たない: [同:4174](<../../src/app.rs:4174>)
  - preview lookup/producer: [同:48226](<../../src/app.rs:48226>)、[同:48377](<../../src/app.rs:48377>)
- 判定: **コードが context 所有不変条件に違反**。Bug 1 と同一。
- 修正案: detached 凍結ルールに従い、swap 時の blanket clear のような症状パッチは提案しない。preview lane 自体を context-owned な typed owner に含める構造課題として扱う。

## 3. リファクタ候補リスト

### P1-1. bypass/prefix preview lane を viewer context 所有へ移す

**なぜ問題か:** 現在のキーは item index と generation を中心に構成され、画像ソースや viewer context の安定した識別子を持たない。別 context が同じ index/generation を持つと、他方の preview を再利用できる。

- **影響範囲:** 3～5 ファイル、約 150～350 行。`ViewerContextBundle`、bundle swap、preview producer/consumer、テスト。
- **回帰リスク:** layer bypass、prefix preview、detached viewer の open/switch/park/close。誤ると別画像表示につながる。
- **テストで担保できるか:** 同一 index/generation を持つ二 context の回帰テストを追加可能。detached viewer の Windows 実機確認も推奨。
- **規模:** Medium
- **優先度:** P1

### P1-2. edit materialization を generation/cancel 付き worker へ移す

**なぜ問題か:** SQLite、JSON、deflate、mask raster、blur compose、texture upload が render 経路から同期実行され得る。大きな保存マスクや 4K blur では UI frame stall が起こり得る。

- **影響範囲:** 5～7 ファイル、約 400～900 行。`app.rs`、local/conceal DB、compose、pending state。
- **回帰リスク:** 初回表示、mask rescale、編集 preview、prefetch、表示切替、キャンセル。
- **テストで担保できるか:** generation/cancel/state transition は単体テスト可能。実時間と GPU upload pacing は 4K 素材での実機確認が必要。
- **規模:** Medium
- **優先度:** P1

### P2-1. 360° source fallback を effective parameter version で管理する

**なぜ問題か:** [app.rs:54873](<../../src/app.rs:54873>) では完成済み final がないと、[同:54883](<../../src/app.rs:54883>) から旧 adjustment/legacy AI cache へ落ちる。色付け・Creative LUT 変更後に旧 pixel を現在世代の panorama source として扱う余地がある。

- **影響範囲:** 2～3 ファイル、約 100～250 行。panorama source resolver、cache key、文書・テスト。
- **回帰リスク:** 360° の開始直後、AI 完了遷移、色変更直後の暫定表示。通常 2D への波及は限定的。
- **テストで担保できるか:** source selection/key の単体テストは可能。実 WGPU と 360° 表示の確認も必要。
- **規模:** Medium
- **優先度:** P2

これはコード経路として確認済みだが、現行仕様にも [display-pipeline.md:754](<../display-pipeline.md:754>) で記載されているため、現時点ではバグ扱いしていない。

### P2-2. キャッシュ無効化を typed cause/state graph に集約する

**なぜ問題か:** `bump_local_adjust_generation`、erase/conceal generation、conceal clear、edit clear、final clear が cascade し、同一操作で重複 clear/cancel が発生する構造になっている。[app.rs:48155](<../../src/app.rs:48155>)、[同:48647](<../../src/app.rs:48647>)、[同:48871](<../../src/app.rs:48871>)。

- **影響範囲:** 2～4 ファイル、約 250～500 行。
- **回帰リスク:** edit/final/retained-final-AI、prefetch、conceal cache 全般。過剰無効化と無効化漏れの両方。
- **テストで担保できるか:** 既存の cache matrix テストに、原因別の state transition テストを追加可能。通常は実機不要。
- **規模:** Medium
- **優先度:** P2

### P2-3. overlay/edit mode を相互排他的な typed state にする

**なぜ問題か:** local、erase、conceal、analysis、bypass などが複数 bool と手動の enter/exit assignment で表現されている。[app.rs:8468](<../../src/app.rs:8468>)、[ui_conceal.rs:127](<../../src/ui_conceal.rs:127>)、[ui_erase.rs:297](<../../src/ui_erase.rs:297>)。不正な同時 active 状態を型で禁止できず、終了順序によって final effect の bypass 復元を誤る余地がある。

- **影響範囲:** 6～9 ファイル、約 500～1,000 行。
- **回帰リスク:** 各モードの shortcut、undo、spread、preview、mode exit。
- **テストで担保できるか:** typed transition と handler-level テストを追加可能。spread、erase、conceal、analysis の手動確認も必要。
- **規模:** Medium
- **優先度:** P2

### P2-4. mask/conceal の永続化 codec を共通化する

**なぜ問題か:** [conceal_db.rs:329](<../../src/conceal_db.rs:329>) に `mask_db` と重複している旨のコメントがあり、実際に [mask_db.rs:1090](<../../src/mask_db.rs:1090>) と圧縮・展開・bounds 処理が並行実装されている。破損対策や互換修正が片側だけになる保守リスクがある。

- **影響範囲:** 3 ファイル、約 150～300 行。
- **回帰リスク:** 既存 mask/conceal DB、sidecar、dimension rescale、破損データ処理。
- **テストで担保できるか:** round-trip、corrupt input、dimension mismatch、旧形式 fixture でほぼ担保可能。実機依存なし。
- **規模:** Small
- **優先度:** P2

### P3-1. effect family 単位に core/UI/catalog を分割し、inventory を検査可能にする

**なぜ問題か:** `local-adjust-core/src/lib.rs` は約 24,000 行、`local_adjust_effect_ui.rs` は約 11,000 行、`ui_adjustment_panel.rs` は約 13,000 行。effect 追加時に enum、dispatch、catalog、UI、label、docs を別々に更新する必要があり、今回の `Repair` 文書漏れはその具体例。

- **影響範囲:** 8～15 ファイル、移動を含め約 3,000～8,000 行。
- **回帰リスク:** 全 effect の serde/default/render/UI category。変更面が広い。
- **テストで担保できるか:** enum/catalog/UI の網羅性テストを生成可能。effect ごとの visual spot check は残る。
- **規模:** Large
- **優先度:** P3

## 4. バグリスト

### Bug 1. viewer context をまたいでローカル preview cache が衝突し得る

- **観測できる症状／条件:** detached viewer と main viewer など二つの context が、同じ item index と同じ generation 値を持つ状態で layer bypass/prefix preview を使うと、一方が他方の preview texture/pixels を再利用し得る。実機再現は未実施だが、キー衝突と global cache lookup の経路はコード上確認できる。
- **壊れている不変条件:** context 固有の cache/texture/pending は、その context の bundle が所有し、別 context から参照されない。
- **原因経路:**
  - context bundle: [app.rs:1921](<../../src/app.rs:1921>)
  - swap 対象一覧: [app.rs:12384](<../../src/app.rs:12384>)
  - source identity のない preview key: [app.rs:4174](<../../src/app.rs:4174>)
  - lookup/producer: [app.rs:48226](<../../src/app.rs:48226>)、[同:48377](<../../src/app.rs:48377>)
  - fullscreen consumer: [ui_fullscreen.rs:2309](<../../src/ui_fullscreen.rs:2309>)
- **同型入口・終了経路:** detached pause は pending/mode を止めるが cache ownership 自体は移さない。mode entry でも source identity は更新されない。通常の erase/local/conceal/edit/final cache は bundle 内にあり、同じ欠陥は preview bypass/prefix lane に限定して確認された。
- **扱い:** detached 凍結ルール上、clear-on-swap などの局所修正は提案しない。BA-7 系の context state ownership 問題として構造的に報告する。

### Bug 2. local/conceal materialization が UI thread で同期実行される

- **観測できる症状／条件:** 大きな `layers_json` を初回表示する場合、または高解像度 conceal blur/mask を初めて合成する場合、UI frame が停止し得る。同期実行は確認済みだが、停止時間の実測は未確認。
- **壊れている不変条件:** UI thread では SQLite I/O、大量 JSON 復元、重い raster/blur、複数の GPU upload を同期実行しない。
- **原因経路:**
  - local lazy DB load: [app.rs:22259](<../../src/app.rs:22259>) → [local_adjust_db.rs:57](<../../src/local_adjust_db.rs:57>)
  - conceal DB/raster/compose/upload: [app.rs:49153](<../../src/app.rs:49153>)
  - 同期 compose の注記: [conceal_compose.rs:288](<../../src/conceal_compose.rs:288>)
  - normal edit assembly: [app.rs:49310](<../../src/app.rs:49310>)
- **同型入口・終了経路:** conceal edit-mode entry、通常表示の edit assembly、final prefetch から到達する。Book/headless 経路は別 worker 内で処理されるため、同じ UI-thread 違反には該当しない。
- **扱い:** generation/cancel/backlog を持つ worker 化が根本対応。単なる閾値分岐や repaint 追加では不変条件を回復できない。

## 5. 総評

実装そのものの合成順序は比較的一貫している。通常表示では edit-result と final composite の境界、Creative LUT・色付け・smart sharpen・post-filter の順序、annotation が完成済み final composite を下地にする点を確認できた。キャッシュキーにも Creative LUT と色付けが含まれており、通常 2D の「補正変更後も旧 AI cache が優先される」疑いは、現行 resolver では支持されなかった。

ただし文書の信頼度には大きな差がある。

- **最も信頼できない:** `local-adjustment-layer-v1.1.0-plan.md`  
  Draft の初期案、実装完了記録、古い schema/pipeline が一つの文書に混在している。現行設計の参照元として使うと誤誘導される。

- **次に信頼できない:** `conceal-feature-plan.md`  
  settings.json、8 tools、旧 DB schema、thumbnail 非反映、worker 実装状況が古く、同一文書内にも矛盾がある。

- **履歴資料としては有用だが現況表として弱い:** `local-adjust-testing.md`  
  後半の完了記録は有用だが、冒頭 matrix と行番号参照が追いついていない。

- **比較的信頼できる:** `preset-and-adjustment.md`  
  edit/final 分離と主要な適用順序は正しい。ただし後半の final-stage 一覧、bypass、Book/headless の説明は更新が必要。

- **概ね現行だが crop 記述に注意:** `display-pipeline.md`  
  panorama fallback や新しい final effect はよく反映されている一方、crop を edit-result に含める古い節が残っている。

v2.8.1 前に優先すべきなのは、context 外に残った preview cache 所有問題と、UI thread 上の同期 materialization である。文書整理では、`local-adjustment-layer-v1.1.0-plan.md` と `conceal-feature-plan.md` を現行仕様書として延命するより、歴史的計画と現行正本を明確に分離する方が誤誘導を減らせる。
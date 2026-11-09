# §1.750 ページめくりの表示単位 binding

2026-11-10、設計基点 `fix-1750-page-turn-layout` / `2b51ea3a5`。r4 ACCEPT後の実装基点は `8ef140539`。
現状は同ブランチでr5判断を適用済み。設計担当によるsandbox外の本体lib全件は17,006/17,006成功（§8.4）。
master未統合、利用者実機確認待ち。
実装・検証の記録は§8。初回の差戻しは§8.1、再開後の結果は§8.3。
参照: [表示パイプライン](display-pipeline.md)、[backlog §1.750](next-release-backlog.md)、
[detached 憲法 §2](detached-rework-plan.md#2-憲法-全ステージ共通の不変条件禁止事項--最重要)。

## 1. 観測と不変条件

段階1の開発側 headless 検証は、遠方の未知ページへの横長寸法到着 →
`landscape_epoch` 更新 → 通常 builder の本全体 `Arc<Vec<SpreadDisplayUnit>>` 再構築で、
対象 occurrence・formation・placement・素材・content・geometry が等しいまま取消になることを示した。
Managed × LTR/RTL は `turn_compare`、合成 Native × LTR/RTL は実 prepare/bind の
`prepare_reuse` で失敗し、差は `layout_arc` のみ。4失敗・2成功の既存結果を再利用する。
これはコード上の欠陥の再現であり、利用者ログの各取消の直接原因や実 GPU の確認ではない。

**本全体の allocation 更新は、そのまま提示単位の失効を意味しない。**
同じ context・items・対象単位・内容・view なら受理した素材と時計を維持し、canonical flat の
実 output 採用まで進む。相方、補助表紙、形成理由、配置、対象ページの回転・編集、view が変われば失効する。
先読み・thumbnail・通常 producer は継続し、品質公開だけでは受理済み素材を差し替えない。

## 2. Arc 同一性の全照合点

行番号は基点のもの。`PT` = `src/ui_fullscreen/page_turn.rs`、`FS` = `src/ui_fullscreen.rs`、
`B` = `src/ui_fullscreen/page_turn/barrier.rs`。単なる `Arc`/`Weak` の資源所有・会計は別の用途。

| 箇所 | 現在保護しているもの | 本全体再構築時の意味／置換条件 |
| --- | --- | --- |
| PT:929 `same_material_and_geometry` | Flat の再利用と Turning:2950 の incoming 継続 | 同じ局所 binding なら継続。素材 ID/世代・coverage・geometry の比較も維持する。単に Arc 比較を削らない |
| PT:1663 `page_turn_unit_matches_current` | 提示済み判定・次 unit 準備・navigation badge の対象 | 現 anchor の意味/content bindingで既提示の資格を照合。現在viewはprepare/Turningと実receiptで確認し、採用後のfit変更だけで提示済みという事実を失効させない。別 context/対象への提示証拠流用は禁止 |
| PT:1687 `page_turn_receipt_matches_held` | 通常 final 公開前の held receipt 採用 | 保存 binding と現在 binding・実素材が等しければ採用。遠方更新で捨てない |
| PT:2016 `page_turn_legacy_crop` | 受理済み legacy crop の寸法基準 | 局所構成・raw 編集入力を比較し、等値なら既知/未知の受理済み基準を保持。content resolver の再帰に注意（§3） |
| PT:2225 `page_turn_held_resource` | incoming の保持素材取得。transform と matching/bind も間接依存 | 局所 binding を使う。同じページ番号だけで返さず、相方・role・配置・方向・content も見る。Offered の ready sources も同じ入口へ統一 |
| PT:2828 Offered → Turning | outgoing と新 incoming が同じ閲覧条件で接続可能か | **outgoing と incoming は異なる単位なので互いの binding 等値では代替不可**。それぞれの保存 binding を現在の自身の anchor へ照合し、共通 context/items/mode/方向/slice と incoming の受理素材を照合 |
| PT:3260 `refresh_page_turn_completion_pages` | 必須 flat の対象・source/configuration の維持 | 同じ局所構成/content なら、view 変更時も通常 prepare の新 geometry/material へ再束縛可。古い flat 証拠で完了させない（§4） |
| B:297 / 329 `page_turn_take_continuation` | Completing の失効判定／採用済み flat の退役資格 | incoming の局所 binding と receipt の完全一致へ統一。遠方更新だけなら同じ continuation を保持 |
| B:361、保持元 B:253 `ResolvedPageTurnNavigation.layout` | 受理済み from/target 操作の commit 資格 | incoming だけの比較では不足。from と target の局所構成を受理時に保存して各々再照合。操作 ID・destination・items・from_idx・intent/contract は維持。**SpreadShift だけは全体 Arc 照合も維持**（§4、r1 P2） |
| FS:31684、保持元 FS:31840 `PendingFsPageTurnReady.units` | 遅延 output receipt が現表示のものか | pending の局所 binding に置換。既存 composition・layout kind・各頁 geometry・anchor・採用証拠・source policy の照合も維持 |
| FS:17658–17666 `FinalDemandTopologies::same_visible` | 本全体の Current/Next/Previous topology キャッシュ | **維持**。遠方の変更も next demand を変え得るので、unit 比較は同等でない。nav Arc と全体 token/units を識別する用途 |
| FS:17735–17736 `ContinuousAdmissionBinding::same_request` | 連結読みの探索範囲・不可視区間の request | **維持**。全体順序/探索範囲の識別であり paged presentation の有効性ではない |

PT:2787 の `PaintedUnit.layout`、FS:2742 の pending、B:21 の command が全体 Arc の保持元。
presentation の資格には不要となる。`cancel_diagnostics.rs`:17/106/146/214 の Arc 比較は
**診断専用**で資格判定ではないが、実装時は同じ binding の差分を報告する形へ更新する。
PT:1049 の Weak 同一性（会計重複排除）、PT:2096 の comic/base pixels 同一性は変更しない。
`matches_flat_unit`、`same_submitted_pages`、output hook、`barrier::record_page_turn_completion_flat`
は直接 layout Arc を読まない兄弟 consumer。新 binding の保存・比較をここにも接続する。

## 3. 単一の意味的 owner

提案名 `PageTurnUnitBinding`。既存 context 所有 `PageTurnPresentation` の各単位に一つ持たせ、
ばらばらな `PaintedUnit` の destination/items/mode/direction/slice/content と全体 layout Arc による
資格判定を置き換える。snapshot の資源 lease と実際の prepared geometry は引き続き `PaintedUnit` が所有する。
receipt は同じ immutable binding の値証拠を保持し、cache 再参照や素材再取得の権限にしない。

binding の値は次の三部分。比較 helper はこの owner の読み取り投影とし、呼出元ごとに条件を複製しない。

1. **単位の意味**: `PresentationDestination` 全体（context/window/viewport）、items generation、
   mode、reading flow、実効 reading direction、slice、要求 anchor、canonical navigation pages と
   `SpreadDisplayUnitFormation`、描画順の全 occurrence（idx/role/navigation_anchor_idx）、
   supplemented navigation side、白 companion の有無。**singleton placement (Left/Right/Center) は
   意味に含めず、3 の表示条件に置く** (r3 改訂。下の「r3 改訂」参照)。
2. **内容**: occurrence ごとの item identity と既存 `PageTurnContent` 相当
   （crop、`EditResultKey` の source_gen を除く全項目、実効 params、comic generation、gap）、
   実効の直角/自由回転。source の品質 publication 世代や全体 landscape epoch は含めない。
   source_gen を無視するのは内容の照合だけで、受理した素材の ID/世代の proof は残す。
3. **view 条件**: singleton placement (Left/Right/Center)、viewport rect、placement、pixels_per_point、回転、content_bbox、fit mode/limits、
   `same_fit_policy` 相当の指定値、clip、underlay と、Native prepare の post-filter/bypass・smoothing・
   upscale source limit。prepared 後は各頁の paint quad/UV/fit input/spine、白 companion rect/clip と
   source/paint ID・世代・coverage/ROI を同じ snapshot の出力証拠として照合する。
   新品質の raster 寸法・fit basis をそのまま新しい view 条件とせず、現行 matching helper と同じく
   view policy が一致する間は受理済み transform/basis を使う。Native の `reuse_presentation` の
   exact key/coverage 判定は省略せず、失敗した Native を bind で復活させない。

### SpreadDisplayUnit からの導出

通常 resolver が採用した nav/units と同じ政策入力を使う。`unit_position(anchor)` の逆引きで単位を得て、
`(unit.pages の順序付き値, unit.formation)` を採る。`nav_start`、unit ordinal、units/nav の
allocation、book 全体 token/epoch は binding に入れない。遠方の組数変更による ordinal 移動は無関係だからである。
次に同じ units の first/last と現行 policy を通常 `resolve_spread_display_composition` へ渡し、
要求 anchor へ `with_navigation_anchor`、`pages_in_screen_order` を適用した上記の値を保存する。
**生の unit.pages だけでは不十分**。端の扱い・末尾表紙・白紙・画面順まで解決した結果が必要で、
遠方の first/last 変更でもそれが当該単位の補助表紙/配置を変えるなら binding は変わる。
Single や facet 外表示も既存の通常 resolver が作る singleton の形成理由と composition を同じ導出へ渡し、
cache 不在を「同じ単位」と見なす fallback や本全体探索は追加しない。

したがって別 Arc でもこの導出値が同じなら binding は等しい。相方/形成理由/役割/配置の変更、
同じ singleton のままの回転、resize/clip、編集は上記のいずれかで必ず異なる。
内容・view は unit 単独からは導けないので、同じ通常 resolver/prepare の入力と出力を併せて束縛する。

### r3 改訂 (2026-11-10、初回実装の停止を受けて)

初回実装 (`fix-1750-wip-impl1` / `4c9416ab2`、未 merge) は、既存契約との矛盾で停止した。読み取り専用の切り分け
(Sol / xhigh) では、page_turn 関連 47 件の失敗が次のとおりだった。設計矛盾 2、WIP の実装誤り 1、
テストの準備の問題 44 (うち 42 件は WIP が新しく足した owning viewport / frame の照合に、
ROOT frame で子 viewport 宛を処理する fixture が合わないもの)。

1. **singleton placement は表示条件**。表紙演出 ON→OFF で側配置から保存済み Center へ戻っても、既存契約
   (`cover_tests` の `page_turn_cover_cancel_to_off_repaints_saved_center_before_completing` と
   `page_turn_cover_setting_change_retires_old_flat_evidence_and_commits_continuation_once`) どおり、
   Completing・保持素材・予約・受理済み continuation を保ち、新しい通常 flat の採用後に一度だけ完了する。
   使い分け: Offered/Turning は意味・内容・表示条件のどれが変わっても skip/cancel。Completing は意味・内容・
   受理素材が同じなら表示条件の変更を通常 prepare で再束縛して `NeedFlat` へ戻す。continuation は from/target の
   意味と **occurrence ごとの item identity** を照合し、配置の変更では退役させない (SpreadShift の本全体 topology
   照合は維持)。item identity は generation だけでは防げない差し替えを識別するので (`app.rs` 10700 付近)、
   受理後に同じ idx・型・generation のまま Image A→Image B へ差し替わった場合は拒否する (r4、レビュー r3 P2)。
   対照テスト: 配置だけの変更では継続し、Image A→Image B の差し替えでは退役する。receipt 採用は
   完全な binding と実出力証拠を照合し、旧側配置の receipt を Center 出力の証明に流用しない。
   新しい可変 owner は作らず、同じ immutable binding から「意味」「表示条件」「完全出力」の 3 投影を定義する。
2. **既存に無い owning viewport / frame の照合を足さない**。destination の照合は base (`fd0d80bc3`) の
   `same_material_and_geometry` 等にある範囲のまま binding へ移す。continuation 消費・pending ready 配送に
   新しい「所有 viewport の frame でだけ処理する」条件を加えない。§1.750 の修正に不要で、42 件の fixture の
   書き直しと、ROOT/子 viewport の配送規則の変更 (detached 凍結の範囲) を伴うため。必要なら別項目で扱う。
3. **WIP の誤り**: `page_turn_unit_matches_current` は、保存済みの単位 A の資格を、最後に描いた別単位 B の
   `fullscreen_page_layout` と比べてはならない。A の現在の意味・表示条件は A の anchor から通常 resolver で導き、
   「今回の出力が A だった」証明とは分けて照合する (`wait_indicator_tests` の
   `page_wait_indicator_appears_for_holdover_after_returning_to_a_stale_flat_unit`)。
4. **テストの準備の修正は 2 件だけ**: 動画の拒否を確かめる 2 件 (`page_turn_actual_f12_continuation_keeps_existing_video_refusal`、
   `page_turn_actual_completion_video_hands_off_both_still_routes`) は、continuation 捕捉後に item を Image→Video へ
   直接書き換えている。動画拒否の契約は保ち、Video を捕捉前から置く形にする。捕捉後の item 変更の拒否は別の
   binding 失効テストにする。§1.750 の回帰のうち child viewport の frame 順を前提にした 2 件
   (`regression_1750_root_service_preserves_child_ready_until_its_own_frame` ほか) は、2 の方針に合わせて
   既存の配送規則の範囲で書き直すか削る。
5. 進め方: 製品修正は一つのまとまりのまま。base で成功していた 324 件 (うち既知の更新対象は
   `crop_legacy_accepted_basis_rejects_changed_document_and_owner` の等値 Arc 再生成の期待だけ) をすべて維持し、
   §1.750 の 4 件を成功させる。

### 導出の順序と共用

通常の adoption → landscape/rotation reconcile → canonical unit 解決を先に済ませ、その解決結果の
小さな値を held lookup → prepare/bind → Turning 比較 → receipt 採用へ通す。
`&self` の照合 helper 内で builder/producer/I/O を起動せず、古い token の units から現在性を推定しない。
入力前/描画前/遅延 receipt 前の既存 prefix で canonical 解決の順序を保証する。遠方 flip を
「token が古いのでとりあえず cancel」と扱わず、通常再構築後の局所値を比較する。
Current/Next 観測等で余分な本全体走査・builder 再実行を増やさない。

Offered では ready sources の**受理済み単位条件/content**を一括所有し、通常 prepare で
**prepared 出力証拠付き binding**へ移す。二段階は既存 Offered/Turning に対応させ、追加 pending flag や
第二の可変 owner は作らない。旧 `AcceptedSource.content` のページ別判定もこの単位条件から読む。
lookup は単位全体の意味/content を照合してから各頁を返し、view/Native key は prepare 境界で照合する。
`page_turn_legacy_crop` は先に局所構成と raw 編集入力を比較して受理済み crop basis を解決し、
その後 content を組み立てる。`page_turn_content` → crop → binding → content の再帰を作らない。

receipt の `Presentation(Vec<PageTurnContent>)` は binding の値証拠へ置き換える。
Ordinary/OFF の無 snapshot 経路は維持し、pending が必要な場合の局所構成/view 証拠にも同じ導出・比較を使う。
`capture_binding` は実 occurrence/source/paint が対応した場合だけ保存し、`rebind` で anchor を変える経路は
通常 resolver の新 binding と一緒に更新する。occurrence だけを書き換えて旧 binding を残さない。
flat は全 occurrence/source/paint と binding、画面順、非 curl、output 採用を照合する。
curl の可視性 ledger・frame/pass・operation tracker は別の証拠として維持し、flat の面積0頁を拒否しない。

## 4. 簡素化と lifecycle

| 検討案 | 判断 |
| --- | --- |
| visible/turning 範囲が等しいとき全体 Arc を維持 | 不採用。遠方が実際に変わっている同じ Vec を残すと navigation/Next/連結の正本が古くなる。現在用と演出用の列を分けると二重正本・合流条件が増える。全件が等しい場合の allocation 再利用だけでは本件を直せない |
| layout rebuild を着地まで保留し、一度に commit | 不採用。寸法の近傍/遠方判定、保留列、cancel/resize/次操作時の適用、held receipt と最新通常配置の切替を新たに所有する必要がある。全体 layout と入力 navigation の追従も遅れる |
| turn 中の thumbnail/prefetch 寸法採用を停止 | 不採用。既存の先読み・表示構成更新を止める退行。利用者承認なしには採らない |
| 単位 binding の値照合、通常 layout は即時更新 | **採用案**。既存の presentation state と canonical flat commit 点を維持する。遠方更新の pending/rollback/resume 状態を作らず、同じ対象なら状態遷移自体が不要 |
| turn 全体を新たに modal 化／窓を閉じて再表示 | 不採用。既存の一件 continuation/flat barrier で入力の直列化は足りる。通常の送り・先読み・窓操作を制限/中断する変更は不要 |

| lifecycle | held material / receipt / 操作の扱い |
| --- | --- |
| open / items 列置換 | 新 context/items の通常 flat から開始。旧 binding/receipt/continuation は使用しない。兄弟 context の資源や世代を失効させない |
| turn start | outgoing を自身の現 unit と照合し、target の受理 binding/source を捕捉。両者は別 binding。通常 prepare 一回で incoming を束縛し、既存 pin budget・motion clock へ移る |
| 遠方 flip | 通常 units/cache を更新。outgoing/incoming の意味と内容が等しければ held lease・projection・時計・canonical 証拠を維持。receipt の旧 Arc を新 Arc へ付け替える必要はない |
| 近傍 flip / 回転 / 編集 | 当該頁または相方の formation/occurrence/content/view の変化を検出。Offered は skip、Turning は既存取消→通常の最新 flat。無効な held を返さず、新 composition へ通常 navigation owner が再束縛する。outgoing 側の実変更も照合対象 |
| resize / fit / clip | 同じ内容でも curl は取消。採用前の Completing は同一単位/content/source を保つ範囲で通常 prepare の新出力へ再束縛し `NeedFlat`。full binding が変わるので古い geometry の receipt を新出力の証明に使わない。canonical flat採用後のresizeはbaseどおりpre-draw確定を妨げない（r5） |
| cancel / 設定OFF | 既存 Completing が必要な canonical flat を所有。内容/構成は同じまま view のみ変わる場合は上記再束縛。それ以外は古い completion を退役し通常の新 flat に任せる。OFF由来の通常経路は増やさない |
| 新操作 | 既存一件 continuation と flat barrier を使う。command は fact/from/target/intent/contract と from/target の局所構成証拠を持つ。両単位が同じなら遠方更新後も同じ target を一回 commit。target が実際に変わった/許可を失った場合は既存 skip とし、別の next へ勝手に解き直さない。**SpreadShift の continuation だけは、既存の本全体 topology 照合 (layout Arc) を残す**。ずらしの基点は対象より前の直近の横長境界で決まり (`ui_fullscreen.rs` 18848 付近)、遠方の横長化で from/target の局所構成が同じまま基点だけが変わり得るため (独立レビュー r1 P2)。不一致なら既存どおり退役し、古い基点を適用しない |
| close / context/window 切替 | 既存 owner の退役/値移譲に従う。close は自身の held/pending を解放し、遅い hook が旧 operation を復活させない。mount/park は兄弟を reset しない。destination が変わる証拠は再利用しない |

Completing の再束縛では「単位/content/source が同じか」と「prepared 出力が同じか」を同じ binding の
投影で区別する。変更後の flat の採用は full binding 一致を必須とする。**採用前**の Completing では、現在 view と
異なる receipt を早期 return で残さず再束縛する。**採用後**は base どおり、描画先の resize で確定を止めない
(r5、§8.1 の判断)。output hook は App を読まず提出時の証拠だけを受理し、
owning frame が現在 binding と照合して退役/continuation commit を決める。

## 5. 実装・検証の受入条件

実装は共通 binding 導出と上記全 consumer の接続を一つの coherent chunk とする。
`FinalDemandTopologies`、連結 admission、Remote IPC、素材品質政策、入力の受付仕様は変更しない。

- 既存 `regression_1750_far_thumbnail_layout_rebuild_{managed,native_prepare}` と各 RTL の4件を
  同じ素材・時計で curl 継続 → canonical flat 採用まで成功させる。ignore/should_panic/期待弱体化は禁止。
- 既存 partner-change と Native resize/clip の2 controls（両方向の計6ケース）は成功を維持する。
  近傍 thumbnail 寸法の実到着で formation が変わる対照も追加し、resident 寸法に隠れる fixture にしない。
- Offered と Turning と Completing のそれぞれで遠方 flip。特に flat 提出後/採用前および採用後/owning frame 前に
  flip を入れ、pending ready・navigation observer が退役し、一件 continuation が一回だけ commit すること。
  command target 自体の構成変更、旧 binding/別 destination/別 generation の receipt 拒否も検証する。
- SpreadShift の対照 (r1 P2): LTR・表紙なし・shift なし・全頁縦長の `[10,11]` から `+1` ずらしを受理した後、
  遠方の頁 3 に横長寸法が届く → 古い基点の shift を適用せず、barrier が退役する。Page/Slideshow の continuation は
  同じ遠方更新で継続する。
- 同一 pages でも formation/placement/補助表紙/白紙/方向/slice が変わる対照、対象回転・crop/erase/params/comic 編集、
  outgoing 側変更を追加/再利用。同じ内容の Arc 再生成、ordinal/nav_start のみの変化は継続する。
- `crop_binding_tests.rs`:361 の「等値 Arc 再生成なら legacy crop を拒否」は本欠陥を固定する旧期待。
  等値なら basis 維持へ更新し、本当に unit/content が異なる対照を加える。未知 basis と再帰防止も維持する。
- source 品質更新/cache eviction、Native key/coverage 拒否、discard/replay、面積0頁の flat、OFF canonical、
  close/late hook を既存回帰で確認。main と detached の同一本/同頁でも別 context の binding/receipt を混用せず、
  片方の遠方更新/open/close が兄弟の資源・世代・queue・提示状態を変更しない headless 対照を含める。

実装担当が focused §1.750 → page_turn/関連 consumer → 必要な full gate を所有し、独立 reviewer は同じ source の
有効な結果を再利用する。実装後は [build/test 手順](development-build-and-test.md) に従い fmt/check と
確認用 build を行う。段階2では文書だけを変更して意図的redを残した。段階4の実装・実行結果は§8へ記録する。
live 検証は別途具体的 suite と明示承認が必要で、利用者実機/実 GPU の成功とは扱わない。

## 6. detached と実装前合意

binding は main/detached 共用 presentation と destination を消費する。
`page_turn_destination` の context/window/viewport 生成、detached 述語、viewport routing、
HWND/focus/z-order は変更しない設計であり、新たな detached フラグや geometry 捕捉は不要。
ただし共用 lookup/receipt とその導出順序が viewport の描画/入力 prefix に及ぶ実装範囲はある。
**着手前に ClaudeCode と Codex の双方で「症状パッチではなく、資格の所有単位を正す構造修正」
との合意を取り、実際の対象と理由を detached rework 計画 §11 に記録する。** 着手前合意は同計画§11へ記録済み
（r2 ACCEPT、改訂r4 ACCEPTで維持）。
実装中に述語/viewport 分岐そのものの変更が必要となれば、その範囲を明示して同じ合意へ戻す。
detached で利用者症状が再現したとは主張しない。

## 7. まれな失敗

まれな失敗は割り切り (ログ + 通知)。retry・多段 recovery・途中状態保存は設計しない。
遠方寸法更新・連続操作・receipt の遅延は通常到達する遷移なので、本書の所有境界で正しく扱う。

## 8. 改訂設計の実装・検証（2026-11-10）

初回WIP `fix-1750-wip-impl1` / `4c9416ab2` は未mergeのまま保持し、実装を再利用してr3/r4を適用した。
その停止理由と47件の切り分けは§3「r3改訂」に残す。今回の製品差分は一つのcoherent chunkであり、
Offered/Turning/Completingとimmutable bindingだけで解決し、新しいpending/rollback/resume ownerは追加しない。

- `binding.rs`が局所意味・内容・view政策・prepared出力証拠を所有する。意味比較ではsingletonの側/Centerを除き、
  白companionの有無と補助表紙・formation・role・方向を維持する。完全receiptでは配置も含め厳密一致する。
- continuationはfrom/targetの意味と各occurrenceの`EditResultKey.item_id`を比較する。
  同idx・同Image型・同generationの別画像への置換も拒否し、SpreadShiftの全体topology照合を保持する。
- Aの提示資格はAのanchorのsingleton/trim/fit/prepare政策と受理済みgeometry/basisで照合する。
  最後のBの描画layoutを読む比較は、receiptの実出力照合に限定した。
- baseに無かったowning viewportのgateは追加しない。既存destination/frame判定を維持する。
  WIPのchild frame寿命に依存したROOT保持試験は、異寸法ROOTから既存observerへ配送する試験へ置換した。
  policy/resizeのprepare境界回帰と、実childの製品pre-draw順のresize回帰を分離した。
- 既存テストの期待変更は等値Arc再生成時のlegacy crop basis保持だけ。実回転による失効対照も追加した。
  動画拒否fixture2件はVideoをcontinuation捕捉前に置き、捕捉後の画像置換拒否は別回帰で検査する。

検証担当は実装者。同一sourceの成功台帳を独立reviewerが再利用する。
アプリ起動・実GPU検証は未実施。通常APPDATAには触れない。コミット/index変更はbriefの指定で行わず、
`target/1750/impl2/`へbinding+consumers、diagnostics、tests、docsの順に適用するパッチと英語メッセージを渡す。

### 8.1 既採用後resizeの設計境界（当初の差戻し記録）

以下はr5判断前の記録。二つの差戻し事項は、後掲の設計担当判断r5と独立reviewerのAGREE（P3適用）で解決した。

独立Sol/xhighレビューで、WIP由来のpending/barrierの `ctx.content_rect()` / ppp照合が、
異寸法ROOTから子viewportの採用済み出力を既存observerへ届ける経路を妨げると確認した。
ROOTのgeometryを子の現viewと扱う比較を撤去し、保存済みpending.layout・実描画layout・receiptの照合へ戻した。
Aの提示資格にも保存Painterの共有Contextからambient ROOTのgeometryを読む条件は残していない。
異寸法/異DPI ROOT配送→observer退役→子continuation一回commitの回帰を追加した。

一方、`begin_page_turn_viewer_frame` は通常draw/prepareより前に `commit_page_turn_continuation` を呼ぶ。
そのため、既採用のcanonical flatの後に**実際の描画先自身のresizeを観測したpre-draw**で、
保存済みの旧geometryを持つfullscreen_page_layoutとreceiptだけからcontinuationを確定できる問題が残る。
§4の「view変更時はNeedFlatへ再束縛」「古いgeometryのreceiptに完了資格なし」と、
r3の「baseに無いowning viewport/frame照合を足さない」を両立する判断境界を、設計担当へ返す。
ROOT配送を止めるgate・別geometry保存owner・時間待ちでの回避は追加しない。

候補は既存destinationのper-viewport入力から**観測済み**geometryを読み、配送可否とは別に完了資格を照合すること。
ただしegui `input_for` は未知viewportにdefaultを生成するため、それを実geometryの証拠と扱わない設計が要る。
あるいは、既採用後は従来どおりpre-draw確定可能、通常prepareが先行したときだけ再束縛する仕様へ明記する判断がある。
いずれも実装者だけでは選ばず、設計担当と独立reviewerの限定合意・§3/§4およびdetached計画§11への記録後に進める。
briefの「設計前提がコードで違う場合は止めて報告」に従い、製品コードの追加変更を停止した。

もう一つ、fixture修正を動画2件だけとした前提にも漏れがある。
既存 `page_turn_actual_navigation_waits_for_adopted_flat_then_bookmark_uses_new_page` は、
`seed_interrupted_leaf` / `adopt_interrupted_target` の**後**にtargetの `items[2]` を
ImageからPdfPageへ差し替え、次のpre-drawでcontinuationが成功してPDFへ移ることを期待している。
r4のoccurrence item identity照合はこの差し替えを拒否し、単独再実行でも同じassertが失敗した。
動画2件と同じくPDFを捕捉前に置くfixture修正が候補だが、briefの許可対象にないため変更していない。
「基点で成功した契約は保持、fixture修正は動画2件だけ」との前提を設計担当へ返す。

### 8.1 への設計担当の判断 (r5、2026-11-10)

1. **既採用後の resize は base どおり**。base (`fd0d80bc3`) の `page_turn_take_continuation` は、受理済み receipt の
   frame・destination・items・mode・slice・`matches_flat_unit` だけで continuation を確定し、描画先の現在の
   viewport 寸法とは照合しない (`barrier.rs` 297〜349 付近)。canonical flat の採用後に描画先自身が resize しても、
   従来どおり pre-draw で確定してよい。次の target は通常 prepare で新しい寸法から描き直される。
   §4 の「view 変更時は NeedFlat へ再束縛」「古い geometry の receipt に完了資格なし」は、**採用前の Completing**
   (flat の提出前・採用前) に限る。観測済み geometry を読む新しい照合は、base に無い所有 frame / viewport の照合に当たるため
   足さない (r3 の 2)。実 child pre-draw の resize 回帰は、この仕様 (採用後は確定、採用前は再束縛) に合わせて書き直す。
2. **PDF bookmark の fixture も動画 2 件と同じ扱い**。`page_turn_actual_navigation_waits_for_adopted_flat_then_bookmark_uses_new_page`
   は捕捉後に `items[2]` を Image→PdfPage へ直接差し替えている。PdfPage を捕捉前から置く形に直し、捕捉後の差し替えの
   拒否は item identity の失効テストで別に確かめる。r3 の「fixture 修正は動画 2 件だけ」は「捕捉後に item を直接
   差し替える既存 fixture (動画 2 件と PDF 1 件)」と読み替える。

### 8.2 r5判断前の検証記録（履歴）

- 初回compileの `ResolvedDisplayPlacement::Z` のmatch漏れは修正済み。
- 最初の焦点回帰18件は成功（`regression-1750.log`）。ROOT/子geometry比較撤去後は、WIP追加resize試験の
  ambient ROOTサイズだけでobserver退役を拒む旧期待が1件失敗（18成功/1失敗、`regression-1750-reviewed.log`）。
  この試験を実prepareの再束縛へ限定した後の焦点回帰19件は成功（`regression-1750-final.log`）。
- 上の19成功は実child pre-drawの完了境界を証明しない。その製品順を検査する
  `regression_1750_owning_resize_does_not_commit_before_reprepared_flat` を別途追加した。
  viewport一致・既存frame順のassertを通過後、製品pre-drawが期待idx10からidx12/Flatへ確定して失敗した。
- 最終sourceの `cargo nextest run -p mimageviewer --lib -E 'test(/page_turn/)' --no-fail-fast` は
  **342実行、339成功/3失敗**（85.561秒、`page-turn.log`）。失敗は上の新規resize、既存PDF差し替えfixture、
  `page_turn_mainwindow_submits_visible_vacant_leaf_both_directions`（trackerのFinished対Started）だった。
- 既存2件はそれぞれ `--test-threads 1` で単独再実行した。PDF fixtureは同じassertで失敗を再確認
  （`navigation-bookmark-isolated.log`）。draw_submitは単独1成功（`draw-submit-isolated.log`）。
  併走時失敗と単独成功の両方を記録し、時計の失敗原因が確定したとは扱わない。
- 最終sourceの焦点回帰も `--test-threads 1 --no-fail-fast` で再実行し、**20実行、19成功/新規resize1失敗**
  （15.193秒、`regression-1750-isolated.log`）。元の遠方更新4件、ROOT配送、全ての他の§1.750対照は成功した。
- 独立Sol/xhighレビューは、ROOT geometry誤照合とパッチ分類漏れの修正を確認した上で、
  **実child pre-drawの完了境界をREVISE**と判断。PDF fixtureの前提漏れも別件として独立確認した。
  最終ACCEPTではなく、設計担当への限定差戻しである。reviewerはbuild/testを重複実行していない。
- `cargo fmt`（引数なし）・`cargo fmt --check` は成功。`git diff --check` はexit 0（CRLF変換の警告のみ）。
- この時点のlib全件・確認用buildは設計差戻しのため未実施。分割パッチは基点から使い捨てコピーへ順次適用し、
  全変更ファイルの一致を確認して保存した。r5再開後の結果と再生成は§8.3。

### 8.3 r5適用後の実装・検証

設計担当判断r5と独立reviewerのAGREE（P3適用、利用者の再開指示）に従って再開した。
新しい観測済みgeometry gate、owning viewport/frame制限、可変ownerは追加しない。

- `refresh_page_turn_completion_pages` は、canonicalの `FlatCompletion::Submitted` が採用済みなら
  base同様に再束縛を行わない。curl採用も表す `incoming.evidence` をこの判断の代用にしない。
  page-turn設定setterは既存のfresh evidence/NeedFlatへ戻すので、設定変更時の新しいflat義務を維持する。
- barrierは採用済みreceiptと保存incomingの完全binding/実素材を照合し、意味・内容の現在性を確認する。
  その後の別drawのlayoutやresize・fit政策だけを、既採用出力の失効根拠にしない。
  fit setterはbaseでも採用済みflatをNeedFlatへ戻さない。page-turn設定setterによる明示退役は維持する。
- 既提示unitの資格もanchor/occurrence/意味/contentを照合する。現在viewをこの事実の失効条件に重ねない。
  次unitのcanonical demandは現在のfit/limits/clip/pppを自身のprepare入力から解決し、
  Turningのview取消・badgeの実出力geometry照合はそれぞれ既存のprepare/receiptで維持する。
  canonical collectorは通常view gateと、同じ既存state/evidenceから得るraw output evidenceの有無を先に読み、
  pure topology resolverの外側OptionをTopologyへ確定し、
  既提示unitの資格、内側OptionのBoundary判定の順に読む。証明欠落をIneligibleへ隠さず、
  cache不在の同値fallbackやbuilder再実行を追加しない。topologyのArc/token資格は変更しない。
- 新しいreceiptには、その呼出しで実際にpaintしたprepared pagesのbindingを捕捉する。
  採用済みCompletingが旧geometryを保持していても、新しいdrawへ旧geometryの証拠を付与しない。
  OFF由来とineligibleのcanonical出力も、同じ実prepared証拠を使う。ただし捕捉資格はbase同様、
  retained incomingと実receiptのdestination・画面順occurrence/source/paint ID一致を必要とする。
  preview/nonfinal/faithfulなど通常selectorが選んだ別素材をpresentationへ昇格させない。
- 実child回帰を、採用前NeedFlat→resize prepare→新flat採用後commit、採用後resize→pre-draw commit、
  採用後draw先行→新receiptへ旧geometryを流用せずcommit、の3対照へ書き直した。
  3番目は実fit setterで現在政策が変わる場合も検査し、採用済みflatをview政策だけで足止めしない。
  managed/OFFのprepare政策変更は、採用前の再束縛と完全receiptを検査する。
- PDF bookmark fixtureはPdfPageを捕捉前に置き、bookmarkが確定先PDFを使う既存契約を維持した。
  捕捉後Image→PdfPageの置換拒否は、generation不変でもitem identityが変わる別回帰で検査する。

焦点回帰、page_turn、関連consumer、本体lib全件、fmt、build-devを順に実施し、結果を以下へ記録した。
独立コードreviewerは実装者の同一sourceの検証証拠を再利用する。

- 素材資格の是正後、焦点回帰は **20/20成功**（`regression-1750-r5-material.log`、3.384秒）、
  page_turn全件は **342/342成功**（`page-turn-r5-material.log`、48.289秒）。
  遠方更新4件、採用前/採用後/fit変更を含むdraw先行、ROOT配送、PDF bookmarkと捕捉後置換拒否、
  original/nonfinal/faithfulの既存3対照を含む。
- 独立Sol/xhighはr5追加製品コードを限定レビューACCEPTとし、素材資格の是正も確認した。
  未解決findingはなく、後続の最終検証証拠とdocs/patchを追補確認する。

r5反復中の記録: 焦点20件は成功したが、最初のpage_turn全件は342中341成功/1失敗
（`page-turn-r5.log`）。既存 `original_preview_nonfinal_and_faithful_spread_never_reuse_frozen_native` が
original_previewでNormalResourceを維持できず、単独再実行でも同じ失敗だった
（`prepare-boundary-r5-isolated.log`）。これは負荷とは扱わず、今回のactual receipt捕捉が別素材まで
presentationへ昇格した実装誤りと確認した。上記のretained material資格を復元して修正し、
独立reviewerも限定是正にAGREEした。既存3対照の期待は変更していない。

本体lib第1回は17,006実行、16,997成功/9失敗、既存ignored75（817.285秒、
`lib-full-r5.log`、run `20261109T195746675Z-b348ab42`）。内訳はcanonical demand7件と
agent IPC2件のnamed pipe作成時 `PermissionDenied`。canonicalのdraw2件とIPC stdio fixtureは
各単独でも同じ失敗を確認した。canonicalは現在viewの照合を既提示資格へ重ねた実装誤りとして切り分け、
独立reviewerのAGREEに従って上記の資格投影を修正する。IPCは別の未変更moduleであり、
アクセス拒否を負荷や本件と無関係だと断定せず、再確認結果と分けて残す。
getterのview照合撤去後、canonical全15件は14成功/Topology対Ineligibleの1失敗
（`canonical-r5.log`、23.171秒）。証明を一時的に取り除く既存対照の分類順序を、上記pure resolver順へ修正した。
この追加修正も独立reviewerがACCEPTし、既存テストの期待は変更していない。
次の反復では初回未描画AppのTopology対Ineligibleが1件残った（`canonical-r5-complete.log`、14/15成功）。
既存state/evidenceの抽出を共用し、raw evidenceなしを先にIneligibleへ分類するread-only投影を分けた。
新しい可変flagはなく、raw evidenceだけで需要を許可せず、後段の完全な意味/content資格を維持する。
その後はcanonical **15/15**（22.722秒）、焦点 **20/20**（3.417秒）、page_turn **342/342**（47.930秒）が成功
（各 `*-r5-qualified.log`）。独立reviewerは追加コードを最終追補ACCEPTとし、未解決findingなし。
未変更IPC2件は各単独でもnamed pipe作成時のWindows Error 5を再現した
（`ipc-r5-isolated.log`、`ipc-shutdown-r5-isolated.log`）。この制約をテスト除外や製品側変更で隠さない。
最終関連consumerはcanonicalを含め **779/779成功**（`affected-final-r5.log`、76.985秒、exit 0）。
修正後の本体lib全件を同じ通常feature/test profile・既存default並列数で再実行した。

#### 最終検証結果

| gate | 結果 | 証拠 (`target/1750/impl2/`) |
| --- | --- | --- |
| §1.750焦点回帰 | 20/20成功、exit 0 | `regression-1750-r5-qualified.log` |
| page_turn全件 | 342/342成功、exit 0 | `page-turn-r5-qualified.log` |
| canonical demand全件 | 15/15成功、exit 0 | `canonical-r5-qualified.log` |
| 関連consumer | 779/779成功、exit 0 | `affected-final-r5.log` |
| 本体lib全件第2回 | 17,006実行、17,004成功/2失敗、既存ignored75、820.302秒、exit 100 | `lib-full-final-r5.log` |
| formatter/差分 | `cargo fmt`（引数なし）、`cargo fmt --check`、`git diff --check` 成功 | `fmt-r5.log`、`fmt-check-r5.log`、`diff-check-r5.log` |
| 確認用build | 初回MSBuild並列失敗（exit 101）後、`CARGO_BUILD_JOBS=1`で `build-dev.ps1` 成功（exit 0）。起動なし | `build-dev-r5.log`、`build-dev-serial-r5.log` |

lib第2回のrunは `target/test-lib-full/runs/20261109T202700899Z-1e2bdb2c`、
source IDは `c908501bd6bb20a6faef8b228d839d265df5ec35fbeb38bb6d6b0ee7de081776`。
失敗は次の未変更IPC試験だけで、いずれもnamed pipe作成時のWindows Error 5（アクセス拒否）だった。

- `agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry`
- `agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers`

各単独再実行でも同じ失敗を再現した。負荷で片付けず、製品コードの変更・テスト除外・権限変更は行っていない。
本体lib全件をgreenとは扱わず、この実行環境でのIPC検証制約として残す。
本件コードのコンパイルと全関連回帰は成功しているため、依頼の通常profile確認用binaryをbuildだけ行った。
配布・実機受入・全件gate成功の代用にはしない。

確認用build初回はTurboJPEGのMSBuild `--parallel 24` がexit 1となり、Cargoはexit 101だった。
briefの指示どおり `CARGO_BUILD_JOBS=1` で同じ `build-dev.ps1` を再実行し、
core・remote・EPUB PDF worker・MCP adapter・music-video editorの5本を生成した。
FFmpeg/VCRT配置とPE依存検査（runtime=4、pe=5）も成功し、スクリプト全体exit 0。
通常feature構成でportableは有効にしていない。製品は起動せず、通常APPDATAの設定・データも操作していない。
利用者向け起動手順・実profileの注意・具体的確認項目は `target/1750/impl2/handoff.md` に記録する。

最終formatter後も `test-source-r5.json` のRust14ファイルSHA-256は全て一致し、検証したsourceを保持した。
検証結果追記だけの文書更新はその後に行う。分割パッチは再生成して使い捨てコピーで順次適用照合し、
最終成果物の照合と独立reviewerの追補確認記録は同じ `handoff.md` に保存する。

### 8.4 outgoing単独失効の取消診断（P3追補）

r5差分は設計担当が `0f78444f6`・`7258a6600`・`bfc2079ef`・`8ca75da6f` へコミットした。
設計担当から、同一内容（CRLF差のみ）のsandbox外lib全件 **17,006/17,006成功**、
§8.3のIPC2失敗はsandbox制約だったとの報告を受けた。これは追補前のr5に対する結果である。

独立実装レビューACCEPTのP3に従い、Turning取消の診断へ `outgoing.current_binding` と
`outgoing.unit_view` を追加した。取消条件と同じ `matches_current` / `matches_unit_view` の投影を使い、
incomingの既存差分を保持する。取消動作は変更せず、パス・画像内容もログへ追加しない。
outgoingだけの消しゴム世代／180度回転変更で、incomingは有効のまま取消となり、
診断の差分が空にならずoutgoingへ帰属するheadless回帰を追加した。
追補の焦点回帰は **21/21成功**（3.510秒）、page_turn全件は **343/343成功**（49.016秒）、いずれもexit 0。
証拠は `target/1750/impl3/regression-1750.log` と `page-turn.log`。
`cargo fmt`（引数なし）・`cargo fmt --check` も成功（同ディレクトリの `fmt.log`・`fmt-check.log`）。
利用者指示によりlib全件・build-devは再実行せず、設計担当のsandbox外検証へ引き継ぐ。

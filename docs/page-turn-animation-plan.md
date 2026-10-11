# ページめくりアニメーション設計案 — backlog §1.341

2026-10-11 / r3 + A0実装追補 / **利用者からR3 ACCEPTと伝達され、`0f6ad9f19`でmasterへ統合済み。既定OFF・slideshow適用を採択し、時間は実機評価用の仮設定。A0の未コミット製品差分と自動検証を記録。実装の独立完了レビュー・実機確認・性能/実用性受入れは未完了。**

R3設計時の参照treeは `next-prefs` / `1ee3e178288f2eff119da16b672d546cc3c9992c`（R2文書commit、開始時clean）。当時の製品コードはmaster `cb00930011323cacd7088ae70c37835141aad58a` と同じだった。
既存の `file:line` は設計時のsource inspection位置。A0は§1.364統合後の`0f6ad9f19`を基点とする未コミット差分で具体化した（§12）。新しい実機性能測定はまだ行っていない。
実装開始の前提は**§1.364のmaster統合後**。そのowner/unit/token契約を消費し、同機能を再実装しない。
後続ラインGの本構成は相互作用だけを扱い、本項でbuilder・保存仕様を先取りしない。

## 1. 要求・読む文書・比較調査

正本は `docs/next-release-backlog.md:227`。まず簡単な方式から始め、方式・時間・ON/OFFを設計時に決める。
連続ページ送り・見開き・縦横連結・多窓の既存速度と入力応答を保つ。**readyなページを効果のために待たせない**。

AGENTS.md、CLAUDE.mdの設計簡素化・まれな失敗・退行相談・UI/IME・detached・keymap規則、
docs/README.md、architecture-overview.md、display-pipeline.md、async-architecture.md、ui-responsiveness.md、
detached-rework-plan.md §2、fullscreen-specified-scale-plan.md、keymap-spec.md、key-customization-impl-plan.mdを参照した。
動画はnative presenterの境界（video-architecture.md §native_window_host / native_presenter）、
Remoteはweb-remote-plan.mdの独立所有、snapshotはui-snapshot-policy.mdを参照。

以下は**一般知識による短い比較で、Web調査・最新版の仕様確認ではない**。固有アプリの方式は未確認として扱う。

| 系統・例 | 一般的に見られる選択肢 | 本項への示唆・不確かな部分 |
| --- | --- | --- |
| 電子書籍 reader（Apple Books / Kindle系） | 紙を曲げるcurl、横スライド、連続スクロールなど | 読書の雰囲気重視。どの方式をどの端末・版で選べるか、現行ON/OFFは未確認 |
| 漫画 reader（NeeView / CDisplayEx / Perfect Viewer等） | 見開き・綴じ方向・瞬時切替・連結読みが重要 | 個々のアプリのアニメーション有無・既定・方式は記憶が不確か。mIVのheld browsingの根拠には使わない |
| 写真 viewer / slideshow（IrfanView / XnView系） | 手動閲覧の瞬時切替と、slideshowのfade等を区別する構成 | 各版の具体的効果・手動ページ送りへの適用可否は未確認。手動入力と自動送りを別に考える参考 |

共通の参考点は「演出を選べる」「高速に探す操作を演出の完了待ちにしない」。他製品との速度比較はしていない。

## 2. 現行の表示・入力を先に確認した結果

| 境界 | コードで確認した事実・維持する契約 |
| --- | --- |
| navigation | `src/app.rs:11281` の `FsNavigationSequence` が受理したtargetを所有。`src/ui_fullscreen.rs:11411` が開始、`src/ui_fullscreen.rs:11967` がready→実描画のラッチ。効果の終了をsequenceの完了条件にしない。実際に見えるtargetのpaint証拠を維持（§4） |
| 速いページ送り | `src/ui_fullscreen.rs:15492` のpaint source / work admissionは、色忠実renditionとtarget materializationを区別する。`src/ui_fullscreen.rs:15815` のburstは最初の押下をArmed、同じholdの次の移動をActiveにする。physical heldだけで単発をrepeatと扱わない |
| 入力基数 | `docs/keymap-spec.md:254`、`src/ui_fullscreen.rs:30010`。ページ送りはSinglePerFrame、未提示target中の追加repeatは既存どおりdrop、queueしない。first down/upが同一frameでも単発を消さない。効果の完了を待つ入力ロックを足さない |
| 表示単位 | `src/ui_fullscreen.rs:14373` の `SpreadDisplayComposition`、`src/ui_fullscreen.rs:17146` のdisplay demandが正本。単ページ・見開き・補助表紙・端配置・白紙相方のoccurrence/roleをそのまま使う。idx±1で相方や進行方向を推定しない |
| 選ぶ画素 | `src/ui_fullscreen.rs:9787` のdisplay texture解決、`src/ui_fullscreen.rs:11052` のprocessed texture解決と `docs/display-pipeline.md:1169` の優先順位を維持。AI/LUT/カラー化待ちを避けるため未処理rawを出すことは禁止。見開きは既存all-or-none readinessを維持 |
| shared dispatch | `src/ui_fullscreen.rs:36090` のhandlerがpage / jump / folder / closeを扱い、`src/ui_fullscreen.rs:21368` が着地。seekのLatestSeekと順送りSequentialをアニメーション側で統合しない |
| gamepad | `src/app/gamepad_input.rs:6776` はrepeatを受けるが、静止画方向経路へ渡す時点でrepeatの区別が落ち、`src/app/gamepad_input.rs:6945` 付近から通常handlerへ合流する。新効果の適格性に使うなら、この既存factを明示的に渡す必要がある（holdタイマーの新設は不要） |
| GPU・幾何 | `src/gpu_lanczos.rs:167` の `FullscreenPaintResource` を共用。`src/gpu_lanczos.rs:129` のpaint batch lifetimeを維持。単ページ `src/ui_fullscreen.rs:36845`、見開き `src/ui_fullscreen.rs:42352` はprepareしたresourceを描き、`src/ui_fullscreen.rs:10511` のcaptureは実際のunit/fit入力を保持する。`src/displayed_image_transform.rs:732` はquadの各vertexへtintを載せる |
| §1.342 | `src/fullscreen_specified_scale.rs:52` / `src/fullscreen_specified_scale.rs:114` のfit入力・resolver、`src/app.rs:11267` / `src/app.rs:11268` のcaptured fit入力とlimitsを維持。効果の投影・陰影からscale・PDF demand・連結基準を逆算しない |

**現在の速さについての証拠の限界**: docs/display-pipeline.md §2.5には過去の測定記録がある。
`docs/display-pipeline.md:2138` の2026-08-11記録は24MP JPEGのUIコスト28.2msがほぼupload、`docs/display-pipeline.md:2446` 以降は
2026-08-12のFHD ZIP / 4K 165Hz環境で約100ページ/秒という旧測定。ただし`docs/display-pipeline.md:2096` 以降の
2026-08-14規則が優先し、受理unitを必ず一frame提示する。旧100ページ/秒を現HEADの保証・新効果の目標にはしない。
この節はR3設計時のコードと既存記録の確認であり、現HEADのbaseline測定は§9の未実施gateである。

### 2.1 既存transitionは何を再利用できるか

名前が似た二つを区別する。

- `continuous_page_transitions`（`src/app.rs:11033`, `src/app.rs:78475`, `src/app.rs:78510`、描画 `src/ui_fullscreen.rs:39340`）は
  **同じページの新final compositeまで旧処理済みtextureを保つbridge**。時間・alpha・移動量を持たず、
  items世代/keep-set/カラー化gateで所有を絞る。ページ間の演出エンジンではないので、名前だけで兼用しない。
- `ContinuousReadingScrollTransition`（`src/app.rs:4173`、`src/ui_fullscreen.rs:37225` 付近）は
  **連結slideshowのscrollとreanchor/history**を所有。smoothstepは`src/ui_fullscreen.rs:37295`、終了後AwaitingReanchorへ移る。
  本項でこのenumを汎用アニメーションownerへ広げるとscroll・履歴・navigationの状態を混ぜる。

再利用は既存の時計/repaint方式、必要なら `t*t*(3-2*t)` の**純粋な時間補間helper**、既存paint resource、
表示単位の解決・snapshot基盤まで。既存scrollの時間設定・終了条件・入力取消・履歴更新は変えない。
parallelなdecode/prefetch/cacheは作らない。R3でも必要な旧unitの短期leaseだけを追加する（R1の旧texture保持ゼロは撤回）。同一ページのbridgeを別ページ間の演出ownerへ転用しない。

## 3. 採択・方式と品質基準

**利用者決定（Q1、2026-10-10）: C方向。旧ページをめくっている面と、下から現れる次ページの両方が見え、紙をめくると読める効果にする。細い端帯だけでは不可。**
長くかかる場合は次々回releaseへ延期可能。R1のA推奨・80ms・旧resource保持なしは採用しない。
Q5は **PC viewer限定（F12を含む）、Remote不要** と確定。2026-10-11にQ3の**既定OFF**、Q4の**slideshowにも同じ効果**を採択した。Q2の最終時間は動きを見てから決めるため、評価用に100〜600ms・既定200msの仮設定を使う（§7・§10）。

| 方式 | R3での扱い・費用 |
| --- | --- |
| 短fade / slide | R1の比較候補。既存quadなら安いが、今回は紙をめくる要求を満たさない。代替として黙って実装しない |
| **折り返しflip（A1の最初の品質候補）** | 大きい表面と折り返した裏面を持つ2.5D sheet mesh。単なる一枚板の縮小、左右反転、細い帯ではない。旧・新resource、visible face判定、陰影、gutter、取消・会計が必要。初期stageでも中程度以上の費用 |
| **連続曲面curl（A2の完成候補）** | 同じ所有・入力契約で、foldを円筒状の曲面へ細分化し、紙の厚み・自己陰影を改善。meshで品質が出れば新shaderなし。custom shaderが必要なら別stageの技術検証・reviewを先に行う。高めの費用、GPU/曲面の品質riskあり |

時間の中盤（目安15〜75%）で旧ページの表と次ページの内容をそれぞれ広い面として識別できることをsnapshotで判定する。
単に旧画面を消して新画面の端を動かす、裏面に旧文字の鏡像を貼る、白い板だけが飛ぶ演出は不合格。
始点は前frameの旧unit、終点は通常描画の新unitと画素・配置が一致する。実際のready frameは少し折り返しが始まったposeから描き、
次ページを全面遮蔽する「準備frame」を入れない。fake clockのelapsed=0でも採用する初期fold量は非ゼロで、そこから終端へ補間する。初期fold量は§3.2の可視性gateで決める。終端付近は旧面が自然に消えてよい。
有限の演出中は新ページの一部が紙に隠れる。**新内容の初出と全面表示は別の時刻**であり、全面表示まで最大採択時間かかる費用を隠さない。

### 3.1 単ページ・横長単ページ

新ページを通常の完成位置に置き、その上の旧ページsheetを端からめくる。LTRの進む操作は右の自由端から左のhingeへ、戻る操作は逆。
RTLは画面の左右だけを鏡映する。次ページが全体で現れるまでのsheetの投影を変えるが、画像UVや印字は鏡映しない。
旧画像の実際の裏面は提供されていないため、裏は弱い紙色・陰影だけ。次の画像を「旧画像の真の裏」と偽って貼らない。
横長でも単ページunitは一枚の横長sheet。中央で勝手に二分・再pairingしない。縦長画像との切替ではそれぞれの採用済みrectと背景を使う。

### 3.2 安定した見開きのleaf turnと綴じ目

以下のA/B/C/Dは**screen slotの名前**で、idxやファイル順ではない。旧左A・右Bから新左C・右Dに着地する例:

| reading / 操作 | めくる旧面（表） | めくった紙の裏に載る新面 | 下から現れる新面 |
| --- | --- | --- | --- |
| LTR / 進む | 旧右B | 新左C | 新右D |
| LTR / 戻る | 旧左A | 新右D | 新左C |
| RTL / 進む | 旧左A | 新右D | 新左C |
| RTL / 戻る | 旧右B | 新左C | 新右D |

通常の安定Pairでは、旧の反対slotも紙の下に残り、裏面の新slotがその上へ着地する。
新unitの両面は最初のeffect frameから実際に見える必要がある。自由端が巻き返る初期poseで新裏面と下の新面を露出させる。
一枚板flipを90度まで回す間、新裏面を完全に隠す設計は採らない。最初のpose・面積は品質検証し、単に1pixel出して提示済みとする抜け道も不可。
A0の仮定量基準は、最初のframeで各target実pageのcanonical可視面積の5%以上が、他面に遮蔽されず画面に投影され、
可視faceの短辺が8 logical points以上あること。透明画像はink量ではなく通常下地を含むpage surfaceで判定する。
t=50%ではめくっている旧表面と新contentが各基準面積の20%以上を占め、snapshotでも大きい二面と読めること。
数値はA0で品質を確認して採択し、geometryだけの微小な露出で提示を偽らない。
これを正しいgeometryで満たせないtemplateは効果を省略し、既存の新unitをそのframeに通常表示する。

hingeは採用済みspreadの綴じ目から取る。左右のinner edgeとgapを保持し、通常のgutter背景は固定。
紙が浮く間だけgapを覆うことはできるが、二重の綴じ目、隣の画像の引伸ばし、gapから画像UVの漏れを作らない。
左右の画像サイズが違う場合もcanonical rectが正本。旧面から着地面へ紙の投影を補間しても、fitや実画像のscaleは再計算しない。
正しく対応できるgeometryの範囲をA0で定義し、超える場合はinstant switch。向きはacceptedの進む/戻る原因と実効bindingから決める。

### 3.3 Blank・Vacant・補助表紙・unit変更

将来Gの `Pair` は二つの実occurrence、`Single+Blank` は一つの実occurrenceと白いsurface、`Vacant` はsurfaceなし。
`C:/home/mimageviewer-nextbook/docs/book-composition-plan.md:340` / `:358` / `:360` / `:366` の契約を消費し、Blankにid・readiness・selectionを作らない。
白紙slotは無textureの紙面として同じmesh/時計でめくれ、裏も実際に採用されたBlankなら白紙。二slot templateはPairだけでなくSingle+Blankにも適用するが、navigation上の型はSingleのまま。Vacantを白紙に変えない。
補助表紙はaccepted roleの素材をそのまま使う。論理上の別ページ、隠し候補や存在しない裏面を足さない。
Gではlandscapeを含む明示Pairを勝手にSingleへ直さない（同文書 `:427`）。旧/新のroleとplacementからtemplateを選ぶ。

隣接navigationでも **Single↔Pair、slot/companion種別変更、split slice変更、layout token変更** は初版ではinstant switch。
実行中なら即取消し、全targetがreadyなら同frameの通常描画へ。横長寸法の後着による組直し、spread shift、方向・sort/filter・G構成adoptionも同じ。
これらを「物理的に同じ紙の表裏」と推測してmorphしない。A2でもunit変更はこの仕様で正しく扱い、全ケースのmorph実装は完成条件にしない。
安定Single/Pair/Blankの対応を早期stageで実装し、G未統合ならsynthetic compositionで契約検査し、実builderとの接続検証はG統合後へ残す。

## 4. 状態の組み合わせを減らす・readyと入力の契約

**effectは純粋なpresentationで、navigation、読書位置、history、decode、slideshow deadlineを所有しない。**
modalなページ送りはheld閲覧を遅くするため不採用。代わりに「受理時からreadyの通常静止unitだけ」「新入力で終了」「不適格なら即通常表示」で組み合わせを削る。
待機・rollback・resume・animation queue・遅れて再開する候補・独自prefetch/cache・別navigation lockは作らない。

context-localな一つの型を提案する（別々のbool/Optionへ分散しない）:
`Inactive | Flat { last_painted } | Offered { outgoing, accepted_ready_fact } | Turning { outgoing, incoming, signature, start_at, deadline, direction, recipe }`。
FlatはON・適格viewの最後の実paintのprepared resource/geometryを保持する。OFF・ineligible・未paintはInactive。
Offeredは**受理時にreadyだった操作から最初の通常target paintへ渡す一回限りのpresentation handoff**で、decode待ちではない。
既存navigationはpaint後にも呼ばれる（`src/ui_fullscreen.rs:25872` 付近）ため、targetのprepared resourceが受理時にあると仮定できない。
Offeredを最初の通常prepare後に必ず消費し、source/geometry/qualityが合えばTurning、そうでなければ通常Flatへ。待機・再評価・再開しない。
accepted_ready_factは§4.2のViewerOperationFact・既存sequence identity/opened_at・readyだったsource identityだけを持ち、target所有・navigation phaseを複製しない。
snapshotはoccurrence/role/slice、実際のprepared resourceとUV、rotation、canonical rect、fit入力・limitsの小さいimmutable集合。
配送先context/window binding、受理操作identityとitems/layout/source identityをsignatureに含める。共有input_seqは診断相関値で、identity・失効条件には使わない（§4.2）。既存tokenを利用し、nav Vecを持つ `SpreadDisplayUnitsToken`（`src/ui_fullscreen.rs:14619`）を毎framedeep cloneしない。
別のlayout generation、待機用Option、開始済みbool、抑制timerを足さない。現在1〜2実ページなので毎frameの処理は上限を持つ。

1. 既存handlerが隣接unitへの単発navigationを受理したときだけ、通常のsource selection結果と旧unitの実paint snapshotを評価する。
   `resolve_*_from_caches`相当でもproducerを呼ぶ経路があるので、名称から非生成と推測しない（`src/ui_fullscreen.rs:9787`, `:11052`）。
   既存の通常prepareが選んだ処理済みresourceを使い、演出目的のensure/decode/uploadを追加しない。
2. 受理時に全targetの正しいrenditionが未readyなら、Offeredへ入らず通常表示状態を保つ。既存sequenceのholdover/thumbnail/loading・work admissionだけが動く。
   受理時の未readyはOfferedへ入れず、後着readyはすぐ通常表示し、curlを後から開始しない。見開きは既存all-or-none。rawや低品質画像へ変更して演出を可能にしない。
   最初の実paintまでにresource/roleが変わった場合もskip。失敗のerror表示にcurlを付けない。
3. Offeredは最初の通常paint passで、**表示unit全体を一度prepareした結果**を受け取る。全incomingのcurl mesh/visibilityを提出前にpreflightし、一つのflat/curl決定の後、一つのunit submission ownerから提出する（§4.1）。受理時の時計を使い、paint時に巻き戻さない。
   このframeで**targetの全実occurrenceが、正しいtextureから可視の面へ描かれる**ことをgeometryのvisibility receiptで証明する。
   新裏面も新ページの提示として追跡するが、裏面culled・全面遮蔽・画面外・shadowだけは提示に数えない。
   receiptを作れなければ同じpassでcurlを捨て、通常targetを描く。effectのためsequenceを未提示のまま延命しない。
4. `fs_nav_holdover_for_draw` のready→Presentingラッチ（`src/ui_fullscreen.rs:11967`）と、実paint後の
   `emit_fs_page_turn_ready_for_display_unit`（`src/ui_fullscreen.rs:26977`、内部 `:26987`）を維持する。
   一度提出したunitの可視receiptからincomingの提示を通知し、outgoingをtargetに混ぜない。sourceの存在だけを可視性の証明にせず、現行通知の接続を§4.1で拡張する。効果終了までsequenceを保持したり、paint前に強制releaseしない。
   `fs/paint`はOS presentの証明ではない。visible receipt、描画batch、実present検証を区別し、受理unitを一frame提示する既存契約を緩めない。
5. 新操作はそのframeの**page描画より前**に非消費のoperation factでOffered/Turningを解除し、元のActionは通常handlerへ渡す。
   seek/navigator等は描画より後にもhandlerがある（`src/ui_fullscreen.rs:24626`, `:24919`）ので、完成Actionを待って取消してはいけない。
   配送先readerを確定したkey-down/repeat、pointer-down/drag/wheel、gamepad操作、設定変更をpre-drawで観測する。source viewportと配送先を分け、root転送も同じfactを運ぶ（§4.2）。UIの入力所有は変えない。
   元操作のstampを再pass/次frameで新操作と誤認せず、passive hover/key-upだけは取消原因にしない。
   repeat/heldの次のedge、方向反転、wheel、gamepad、seek/jump、click/drag、zoom/pan、表示設定変更は取消。
   キャンセルframeのtargetは効果なし。次のtarget受理・drop・提示順はOFFと同じで、同じ入力から別のcurlを再開しない。
   判定は一回のtyped reducer結果と既存burst/Press/Repeat factで行い、時間差からrepeatを推測しない。
6. keyboardの最初のdownをphysical heldだけで除外しない。同frame down/upも単発として残す。
   passive hover/pollは取消原因にしない。keyupだけで演出を再開しない。accepted inputがない境界no-opにも取消は効く。
   同frameのegui replay passで二重開始/終了/通知を作らない。
7. source/rendition/fit/viewport/DPI/構成の変更、hide/minimize/closeは当該effectを終了し、通常rendererへ戻る。
   AI/PDF finalや色補正の差替えを古いsnapshotのため待たせない。期限を過ぎたら即通常frame、見えなかった途中frameを追いかけない。
   動作中のviewportだけrepaintを要求し、取消/finish/hidden後のheartbeatはゼロ。

### 4.1 unit全体のprepare → 決定 → 一度提出 → 提示通知

現行の `src/ui_fullscreen.rs:42009` / `:42046` は左右を順に `draw_fs_spread_page` へ渡し、そこでprepareとpaintを行う。
prepared resourceは `:42389` 以降の局所変数。サイズ未確定のfallbackも `:42129` で同helperを呼ぶ。
また `src/ui_fullscreen.rs:26609` の `fs_display_unit_trace_pages` はsourceの存在とoccurrence集合を照合するだけで、遮蔽・可視面積は証明しない。
従って既存loopに片側ずつcurlを足す方法では本書の契約を満たせない。**通常paged single/spreadとそのholdoverの共通提出境界を分離する**。
連結・native presenter・編集等へ汎用engineを広げず、正常時のsource優先順位・work admissionは既存のまま。

| 段階 | 提案する接続と責務 |
| --- | --- |
| 1: draw unit選択 | 既存のtarget readiness/holdover決定から、このpassで実際に描くcanonical/holdover unitを一つ選ぶ。通常targetのproducer admissionは従来どおり実施し、holdoverを選んでもmaterializationを止めない |
| 2: `prepare_paged_display_unit` | canonical geometry、全実occurrenceの通常prepared resource、UV/ROI、clip、underlay、Blank surface、loading/error表示を一つのframe-local `PreparedPagedDisplayUnit` に集める。各実pageの通常prepareは一回。ここではimage/underlay/影/提示通知をpainterへ出さない。FullscreenPageLayoutの更新もまだしない |
| 3: `choose_unit_presentation` | 同じprepared unitとimmutable outgoingから全faceを生成し、全incomingの遮蔽後面積、ROI coverage、role/token、宛先binding、pin予算を一度検査。`Flat(prepared)` か `Curl(prepared, meshes, visibility)` の排他的なplanを選ぶ。Offeredはここで消費し、curl不可なら同じprepared resourceを使うflatへ決定 |
| 4: `submit_paged_display_unit` | 選んだplanを一つのsubmission ownerからpainterへ提出。unitの全page/underlay/shadowとbatch leaseをまとめて管理する。複数mesh drawはあっても途中に取消判定・再resolve・再prepareを挟まない。canonical geometryからFullscreenPageLayoutとnavigator用データを一度更新し、Flat記録もこの実提出素材から作る |
| 5: submitted receipt | 提出後に `SubmittedUnitReceipt` を一つ返す。context/window binding、render transaction、accepted unit/source identity、flat/curl、全incoming occurrenceと遮蔽後可視面積を結び付ける。提出したplanとreceiptが同一であることを照合してから既存navigationの提示通知へ渡す |

loading/error/thumbnailは既存規則で作るflat planに含め、curlのために新しいall-or-none待ちや空白frameを作らない。
全targetの正しいsourceが揃わない場合は既存のholdover/readinessを維持し、target提示receiptを偽造しない。
curl preflight失敗は **同pass、同prepared unitでflat提出**。片面curl提出後の取消、flatを先に描いてからcurl重ね、二重prepare、次frameへfallbackを予約する経路は禁止。
単ページも同じ境界を使い、spreadの各slotが独立に提示通知しない。source selectionとGPU resample準備の順序・回数はOFFでも回帰検査する。

`src/ui_fullscreen.rs:26977` のready通知とその内部 `:26987` のsequence提示観測は、source集合の照合に加えてsubmitted receiptを受け取る形へ接続する。
`FsNavigatorTextureSources` はcanonicalの素材/hit情報として維持できるが、それだけをcurl可視性の証拠にはしない。
flatでも通常clip後の実提出を示すreceiptを作り、既存の正常提示判定は変えない。curlだけ§3.2の追加面積条件を検査する。
Blank・shadow・outgoingはincoming実occurrence集合に数えない。source idとnative prepared texture idは両方記録し、素材選択と使用したGPU面を混同しない。

提出途中の通常入力割込みはUI threadの一つのpass内では処理しない。既に取り込んだ操作factは段階3より前に反映し、後着入力は次の通常passで即取消する。
egui再passは同じ操作factを再発火させず、各採用passで同じ境界を通る。再pass自体をfallback frameにせず、discardされたpassのreceiptを追加提示数に数えない。
このreceiptの最終採用・dedupとpaint記録接続をA0で具体化して再レビューする。OS present時刻の検証は別gateであり、receiptは採用passのpainter提出の証拠であり、GPU提出・実presentは別途検証する。

### 4.2 取消factの宛先・操作identity

`src/app.rs:21654` の `App.input_seq` は共有診断カウンタで、ViewerContextBundleのswap対象ではない。
**input_seqが変わっただけでは兄弟readerのOffered/Turningを取消さない。** 生存source/rendition世代の正当な照合とは分ける。
`src/ui_fullscreen.rs:28531` のrootキー経路はROOTからfullscreen readerを操作し、`:28619` の通常handlerへ合流する。
source viewportをそのまま取消先にする設計では、この転送と複数窓の所有を扱えない。

提案する非消費factは `ViewerOperationFact { destination, operation_id, source, phase, cause, diagnostic_input_seq }`。
これはnavigation request/queueではなく、既存入力routerが確定した操作のpresentation側read-only情報。

| 項目 | 契約 |
| --- | --- |
| destination | `ViewerContextId` と受理時のwindow binding（window idまたはmain binding、表示viewport）をセットで保持。source HWND/viewportとは別。既存registryの `src/app/viewer_context_registry.rs:200` / `:208` / `:215` のwindow↔context対応で配送先を照合。内部mount/depositは同じ宛先、実rebind/retireは古いbindingのeffectを終了。古いfactを新contextへ付け替えない |
| operation_id | 入力の一回のdeliveryを表す型。提案例は `KeyEdge(batch_id, source_hwnd, ingress_ordinal)`、`PointerEdge(batch_id, source_viewport, event_ordinal)`、`PadAction(poll_batch_id, controller, action_ordinal)`、`UiCommand(batch_id, widget/action, activation_ordinal)`。batchはraw入力を一度取り込むtransaction、ordinalはconsumerが削除する前に固定。pass番号・処理後Vec index・キー値・共有input_seq・時刻差だけからidentityを作らない |
| source | 転送前のviewport/HWND/device。root→fullscreen転送はoperation_idを保持し、宛先だけをrouterの確定結果にする。二重probeから別idを発行しない |
| phase/cause | `Press / Repeat / Continuous` と、page step・wheel・seek/jump・zoom/pan・表示変更・gamepad等の原因。keyboard release/passive hoverは新操作にしない。`src/app/gamepad_input.rs:297` のPress/Repeatを静止画経路（`:6776`）でも失わず渡す |
| diagnostic_input_seq | perf/worker相関だけに使用。effectの配送、同操作判定、取消・候補失効のownerにはしない |

現在の `src/key_input.rs:1175` のKeyEdgeにはsource stampとrepeatがあるが、安定したoperation_idはない。
`:3035` のconsumeはframe Vecをremoveするため、処理後のindexは使えない。A0でraw取り込み時のimmutable stampを付与し、
既存typed consume resultとroot転送へ同じidを渡す最小拡張を定義する。新しい物理入力判定やrepeatタイマーを追加しない。
non-Windows/egui直接入力も同じbatch+ordinal契約、UI button/ringはその操作を生んだpointer/key idを引き継ぐ。

配送の規則:

- viewport直接入力はそのwindowの現在のreaderへ。ROOT転送は `handle_fullscreen_root_key_input` と同じ所有・modal/IME/embedded/detached条件で宛先を確定し、描画前factにも同じ結果を供給する。rootだからmain readerと決め打ちしない。
- pointer/seek/navigatorの完成Actionが画像描画後でも、pointer-down/drag/wheelのfactと宛先は先に判明する。前面UIの同一reader操作ならeffectは取消せるが、キーやwheelを背面navigationへ再注入しない。無関係な兄弟窓・root UIの操作は当該readerへ配送しない。
- gamepadは既存routeでreaderを確定してからfactを渡す。新たなPress/Repeatには別delivery id、同じpollのreplayには同じid。analog等のContinuous activeは毎passでeffectを不適格にし、level pollを新Pressへ変えない。digital最初のPressをheldだけでRepeat扱いしない。
- 一つのraw batch内の複数edgeは別idを維持する。最初のedgeからOfferedを作っても、後続の別edgeがあればその候補も取消し、元のSinglePerFrame/drop基数はそのまま。受理通知がpaint後でも、そのbatchで観測済みの後続edgeがあればOfferedを作らない。余ったedgeを次frameのnavigation queueにしない。
- Offered/Turningには開始操作idを保持。同じidのroot転送/再passは再取消・再開始しない。別idはnavigationが境界no-op/dropでも即取消。既存入力transactionの観測結果を使い、演出用の全履歴setやfreshness時間窓は作らない。
- 処理時のAppにどのcontextがmountされているかで宛先を推定せず、destinationのownerだけを更新する。配送前後のbindingを確認し、退役窓のfactを兄弟へ流さない。

pre-drawの非消費fact供給点、typed consumeへのid拡張、root/viewport/gamepad/late UIの配送順序、同frame多edgeとreplayのdedupはA0の必須成果物。
A1へ進む前にunit提出境界と併せて再レビューし、現R3を接続実装済み・構造合意済みとは扱わない。
## 5. 描画・素材寿命・VRAM

### 5.1 既存egui meshを第一候補とする

`src/displayed_image_transform.rs:732` のquad+UV+vertex colorを、純粋なsheet projectionが生成する分割meshへ拡張する。
同じ `FullscreenPaintResource::paint_texture_id`（`src/gpu_lanczos.rs:264`）をeguiに渡すので、eguiのwgpu sampler/alpha blendとnative Lanczos出力を使える。
CPUは解析的なfold/円筒曲面の位置・法線・UV・face visibility・有限個の描画順だけを計算し、GPUが既存pipelineでrasterizeする。
frontとbackは別material/UV basisを使い、紙の裏から見たときに旧文字が反転する実装にしない。裏に新ページを載せる場合は着地時の正しいrotation/trimからUVを導く。
左右反転はgeometry・hingeに適用する。texture UVを単純に `1-u` としてRTL対応しない。

A1は大きな表面＋折り返した裏面、A2は連続曲面。目安上限64横segment×8縦segment、全meshで4096vertices/8192triangles/context/frame以内。
mesh bufferは再利用し、全items走査やページ全画素のCPU合成をしない。eguiにはdepth bufferを前提にできないので、
投影の隠面を解析的にclipし、必要なstripを奥から描く。自己交差、左右の綴じ目、front/backの接合をsnapshotで検査する。
純粋projectionで隠面を安定して解けない場合、深度つきcustom callbackを別stageで検証する。場当たり的なepsilon重ね描きで済ませない。

曲面の法線から控えめな明暗をvertex colorへ掛け、下の紙へのsoft shadowは少数のgradient polygon、綴じ目shadowは専用の細いgeometry。
これは演出の局所陰影であり、LUT/AI/色管理を再適用しない。平らな終端では陰影ゼロ・通常画素に戻る。
透明PNGは通常の下地とpremultiplied alphaを維持し、「紙の不透明下地を追加するか」は既存page underlayに合わせる。画像の透明性を勝手に消さない。
HUD/seek/カーソル・navigator枠はsheetに巻き込まず通常の前面に置く。

custom WGSL shaderが必要な場合は曲面のperspective UV、法線・自己陰影の改善だけを狙い、所有契約は同じにする。
`vendor/egui-wgpu/src/renderer.rs:821` のtexture参照はnative登録でも必ずwgpu Textureを得られるAPIではない。
`src/compare_wgpu.rs:81` / `:539` のcallbackはCPU RGBAと専用pair cacheを持つため、そのまま流用すると追加upload・窓間の所有衝突になる。
任意TextureIdから安全に同じbind group/viewを借りるbridge、paint-batch lease、複数context/device lifetimeを先に検証する。
取得不能時のreadback/再decode/CPU copyは不可。vendor変更・offscreen surface・shader compilationはA2技術gateで費用を再評価し、必要なら延期する。
初版はframeごとのoffscreen captureも新GPU texture allocationもしない。

### 5.2 旧frameは画像resourceとgeometryをretainする

「旧frame」はOS screenshotや全画面bitmapではなく、**最後に実際に描いたunitのprepared resources + geometry**。
既存capture（`src/ui_fullscreen.rs:10511`）とholdoverのoccurrence/fit/role（`src/app.rs:11207`, `:11247`）は再利用できるが、
**prepared素材がそのまま残っているとは仮定しない**。`src/ui_fullscreen.rs:10704` はsourceを再resolveし、`:9872` のwrapperはDirect/Resampleable。
実際のLanczos prepare結果はsingleの `:37000`、spreadの `:42389` 以降の局所変数で、現FullscreenPageLayoutはresourceを保存しない。
従ってON時だけ、これらの実draw境界から一つのFlat last-paint recordへprepared resource+geometry+clip+underlayを保存する。
受理時にそのrecordをOfferedへmoveし、navigation previousはidentity照合に使う。旧sourceを再prepareしてeffect素材を作る経路は追加しない。
通常paint/finish/cancel後は現在の実paintでFlatを置き換え、OFF/ineligible/closeで解放する。Flatのcurrent resourceも会計対象で、idle中に旧unitを積み重ねない。
未使用ページ・兄弟window・UI chromeは保持しない。Turningには旧・新のimmutable snapshotを一つずつ持たせ、renderer/cacheへのraw pointerやTextureIdだけを残さない。

TextureHandleのcloneはtexelコピーではないが、cache eviction後もGPUメモリをpinする。
Lanczosではsource handleと `Arc<LanczosOutput>`、native TextureIdLeaseが必要（`src/gpu_lanczos.rs:167`, `:282`, `:1054`）。
stateをcancel/dropしても、既に提出したbatchの描画終了までは `retain_native_output_for_paint` と同じleaseを**全使用face**へ付ける。
managed sourceもbatchにleaseを付け、先にfree deltaを送らない。resourceのdropとGPU queueの完了待ちをUI threadで同期しない。
finish/cancelで旧effect側の参照は即解放、batch側の最後の参照は既存paint lifecycleで自然に解放する。
会計/admissionの予約は共通のresource lease ownerに付け、最後のbatch参照が消えるまで解除しない。
weakなlease inventoryを既存会計へ接続してcache外素材を列挙する（inventoryはtextureを所有せず、navigation queueではない）。

Lanczosの `visible_source_uv_rect`（`src/gpu_lanczos.rs:289`）は画面に見えていたROIだけの場合がある。
partial outputを全ページtextureとして引き伸ばさない。通常paintで採用されたUV領域をsheet上へ正しく写し、その外は従来clipのまま。
曲げた結果、未準備ROIや画面外の画素が必要になるtemplateはskip。演出poseのたびにROI/resample demandを再発行しない。
source generationや処理済みoutputが変わったら古い画像を守るより即取消し、新ready素材を通常表示する。

### 5.3 会計とadmission

既存 `src/app/vram_accounting.rs:33` はLanczos outputを、`:102` はfullscreen cache等を数える。
cacheから外れたeffect所有のsourceも漏らさず、mounted/at-rest contextと生存paint batchを含めて列挙する。
`src/vram_budget.rs:123` / `:181` のsubsystem分類とTextureId dedupを拡張し、source mip分・native output分をそれぞれ一度だけ計上する。
単なるhandle cloneを追加textureとして二重計上しない。cacheから離れてeffectだけが保つbytesと、共有resident bytesを区別してlogする。

目安: RGBA8なら3840×2160=31.6MiB/面、24MP=91.6MiB/面。二つの旧24MPだけで183MiB、別のLanczos output/mipも増える。
したがって「新allocationゼロ」でも追加保持のcostはゼロではない。二窓以上ではさらに積み上がる。
A0の仮admission案は全context合計の旧素材pin予算 **min(128MiB, fullscreen high-waterの10%)**。
現cacheにある分も将来evictionを妨げる最大pin量として保守的に予約し、cache/outputと重なる分の実使用量はdedupする。
通常target/cacheが先に予算を使う。予算不足・通常pipelineのmemory pressureならcurlを省略/即取消し、ready画像を通常表示する。
演出を維持するための本体cache縮小、page decode抑制、待ちqueue、retryは作らない。この数値は測定後の設計採択事項で、現実装済みではない。
blank/shadow meshはtextureを作らない。mesh/batchのCPU/GPU bufferも別途上限を計測する。

通常の不足・source後着・多窓競合は構造で扱う。まれなGPU resource失敗は既定の割り切り（log + 通知、effectを終了して既存描画へ）、
専用retry/中途復元/回復の回復を追加しない。device lostの全体復旧は既存rendererに従い、ユーザー保存データを削除・再生成しない。

## 6. 操作・表示機能・多窓

| 操作/状況 | R3と2026-10-11の利用者決定による規則 |
| --- | --- |
| 単発キー・click/tap/ring | 受理済み隣接・即readyの通常静止unitだけ開始。Action/入力基数を変えず、effectの入力ロックなし |
| 連打・held / 方向反転 | 次入力で即取消、その移動は通常表示。既存burst Active・repeatは常に効果なし。最初のPressはheld levelだけで排除しない |
| gamepad | Pressは単発と同じ。Repeat/analog連続は取消+通常表示。`src/app/gamepad_input.rs:6776` のrepeat factを静止画経路にも明示的に渡す。独自hold timerなし。nudgeは組変更で取消 |
| wheel | pageへrouteされた単発1notchだけ候補。複数notch/burst/演出中は通常表示。accumulator・popup/panel/seekの所有は維持 |
| seek / jump | seek/strip、Home/End、割合、しおり/履歴/検索、類似本・フォルダ移動は取消+通常表示。たまたま隣でもjump原因で除外。LatestSeekとSequentialを維持 |
| slideshow | **同じrecipe/即ready条件を適用する採択済み対象**。追加toggleは作らない。自動入力も既存effectを取消し、interval/deadline/readinessをeffect終了へ連動しない。評価中は選択した仮時間をDとしてinterval < Dなら省略する。開始時の既存next deadlineまでの残り時間がD未満なら同様に省略し、deadlineを延長しない |
| 縦横の連結読み | 新curlなし。既存scroll transition・reanchor・wheel/gamepad・slideshow scrollは維持。連結へのmode switchは取消 |
| §1.342指定サイズ | canonical fit/limits/physical pxをsnapshotへ保持。指定％/pxを演出poseから逆算しない。end/cancel frameは同じresolver結果へ戻す。指定scaleでpageがcanvas外へ大きくはみ出す場合、必要ROIが揃わなければcurlだけskip |
| zoom/pan / free rotation・trim / original hold / 編集・比較・分析 | 操作開始で取消、これらがactiveなviewには新curlを重ねない。機能は従来どおり。固定rotation済み素材のUVは通常と同じ。新操作中に古いsheetをhit-testに使わない |
| loupe | 開く/動かす操作で取消。active中はeffectなし。拡大するのはcurrent canonical pageの正しい画素で、foldや旧snapshotを拡大しない |
| navigator | static navigatorは新current canonical unitとpan rectを通常表示。紙のposeをnav rect/scroll positionへ逆算しない。navigatorでdrag/seekしたら取消。old pageを別の選択targetへしない |
| capture / copy / export | 操作時は取消し、既存captureがcurrent canonical unitを取得。影や中間foldを保存画像に焼き込まない。スクリーンOS captureはその瞬間の見た目になる。captureのready待ち仕様は増やさない |
| GIF/APNG/Animated WebP / 動画・音声 | 新effectなし。静止との媒体境界も通常切替。native presenterは別D3D11/DComp（`docs/video-architecture.md:2172`）、wgpuへ取り込まない |
| F11/F12・window switch/open/close | presentation切替自体は取消+通常表示。次の独立readerでの単発turnは同じ条件。focus/host生成/placementの待ちにeffectを使わない |
| Remote | **Q5決定: PC viewer限定、F12含む、Remoteの新effectは不要**。通常window内embedded still/F11も対象。Web PageGroup、fetch/prefetch/readiness/RTLと既存閲覧を維持。state・texture leaseをIPCへ流さずprotocol変更なし。Remote curlは本項の工程に含めない |

このpresentation enumは `ViewerContextBundle`（`src/app/viewer_context_registry.rs:804`）の一つのtyped field。
default/destructure/swap（`:2203`）/fork/park/mount/retireを揃える。通常mount/depositで取消しない（Aの描画がBのeffectを終わらせてはいけない）。
明示hide/minimize/close/retireは当該contextだけ取消、forkはInactive。focusの実移動なら前windowのeffectを終了するが、内部mountと混同しない。
時計は共有できるがrepaint先・resource・signatureはwindow/context binding別。兄弟のworker/cache/generationをreset/cancel/drainしない。
二窓で同時Turningが成立し、共通VRAM予算だけがadmissionを制約する。CPU texture pair cacheをglobal一個にしない。

detached述語/viewport経路に触れる実装前には `docs/detached-rework-plan.md:29` の§2に従い、設計担当と独立reviewerで
「presentation所有の構造的拡張で、症状パッチではない」と明示合意し、範囲を同plan §11（`:1456`）へ記録する。
A0の接続範囲は同plan §11へ記録する。Appへdetached用bool/rect捕捉/grace/debounce/追加hidden viewportを足さない。
採択時間は演出の長さで、event freshness・競合解決の時間窓ではない。

## 7. §1.364・Gの接点、設定・keymap

**実装開始はE（§1.364）master統合後。** R2で参照した別treeはその時点で未統合・dirtyであり、未統合設計を現HEADの実装事実としない。
`C:/home/mimageviewer-nextview/docs/spread-shift-memory-plan.md:380`（unit/token）、`:453`（local context）、`:617`（G契約）、`:767`（D2記録）を再照合する。
共有接点は `src/ui_fullscreen.rs:14619` token、`:17859` unit owner失効、`:37440` 組変更reconcile、app/registry swap、HUD/input。
E採用済みshift/role/unit/tokenを読み、shift操作・復元・解除で取消。保存replyだけをturn原因にしない。
SpreadShiftService/DB/RestoreReceipt/context非同期owner/復元順を再実装せず、休眠shiftや兄弟sessionへwriteしない。
E統合でsignature/paint/cancel境界が変わったら本書を先に改訂する。共有ファイルは実装者一人が所有し、他treeへ書かない。

**後続Gは相互作用だけ。** `C:/home/mimageviewer-nextbook/docs/book-composition-plan.md:427` / `:950` / `:1072` のbuilder/shift/local-Remote境界を消費する。
Gのdirectionはscreen slotのみを反転しlogical orderを変えない（`:358`）。Blankはtexture/addressを要求しない。
構成save/reload/解除・adoption・membership/role/slice変更で取消。LayoutSource/明示Pair/Single/companion/hidden許可を受け取り、idx±1や自動pairingを前提にしない。
Gのmode-onlyとbindingの分離、modal入力所有、hidden提示許可を迂回しない。本構成store/projection builderの変更は範囲外。

有効設定は `page_turn_animation_enabled: bool`、表示名「ページめくりアニメーション」、**既定OFF採択済み**。
方式は採択されたfold/curl一つ。Q2の最終時間を実機評価後に決めるため、`page_turn_animation_duration_ms: u32`を仮に追加し、100〜600ms・既定200ms、「時間（仮）」スライダーで調整する。最終の時間仕様は評価後に再決定し、この仮範囲を出荷仕様の採択と扱わない。方式picker・本別値・新DBは作らない。
説明は「単発のページ送りとスライドショーで紙をめくる効果。連打・読込待ち・連結読みには適用しません。」。読込待ちの既存loading/holdoverまで瞬時化する約束ではない。
既存preferences draft/save worker/settings transfer round-tripを使い、欠落fieldはOFF/200msへ補完する。通常設定の時間は100〜600msへ補正し、設定転送は型違い・範囲外を拒否して現在のdraftを保つ。
ON→OFFは即取消、OFF→ONは次の適格な操作から。設定確定に本reloadを足さない。
slideshowは同じ有効設定と時間を使い、追加toggleは作らない。Q5のPC限定は決定済み。

新KeyAction/shortcutは不要。取消は非消費のpresentation処理で、既存page Action・SinglePerFrameを維持。
既存typed consume結果/PadActionKindに操作原因を接続し、キーコード直書き・OS level再分類をしない。
将来toggleをkeyboard/ringへ公開するならKeyAction/helper/ALL_ACTIONS/ini_name/context/default_chords/操作カスタマイズ・default keymapを一緒に追加。
IME/TextInput/前面popupの所有は維持し、そこで消費されたキーを背面viewerへ再注入しない（`docs/key-customization-impl-plan.md:443`）。

## 8. 段階分け・費用・出荷判断

日程の確約ではなく、一人の実装+独立reviewの概算。既存texture/paintを使っても、R1のtint変更規模には収まらない。

| stage | 完成物・主な所有ファイル | 仮の費用と判断 |
| --- | --- | --- |
| **A0: 契約・baseline・geometry spike** | E統合後再照合、純粋projection fixture、unit全体のprepare/一度提出/receipt接続、配送先とoperation_idのpre-draw取消、first paint契約、last-paint record/Offered handoff、native lease/ROI/VRAM proof、開始/完走/skip率。app/ui_fullscreen/gpu_lanczos/registryの変更境界を独立review | 2〜4作業日。製品に未完成effectを有効化しない。ここで具体的接続を再レビューし、REVISE事項とA0品質/実用性を閉じてからA1へ。期間を再見積し次々回へ延期できる |
| **A1: 出荷可能なfold/flip** | single（landscape含む）・安定Pair・LTR/RTL・gutter/Blankのmesh、全入力取消、F12、OFF fast path、VRAM会計、設定・snapshot・perf。新小module例page_turn_animation、app/registry/gamepad/key_input/既存keymap helper/ui_fullscreen/settings/preferences/settings_transfer/vram_accounting/vram_budget | 5〜10作業日+review修正・実機枠。中央で旧/新の大きい面が見えること、全gates必須。品質が紙めくりとして成立すれば初版出荷候補。端帯・一枚板縮小で代用せず、不合格なら延期 |
| **A2: 曲面curlの品質完成** | A1と同じownerで曲面/陰影/厚み改善、幅違い・高DPI・4K品質。必要時のみshader bridge/depth技術検証、shader/lifetime review | 5〜10作業日、shader/vendor変更が必要なら追加3〜7日以上。要求品質がA1で不足ならA2まで一括して延期。G実統合のfixtureは統合後再確認 |
| **A3: 出荷検証・文書** | focused tests→snapshot→共有gate、perf分析、独立完了review、別承認interactive、display/architecture/keymap/manual/docs索引・detached §11 | 2〜4作業日+実機待ち。性能・正常入力が退行するなら修正/延期、既存機能を削って出荷しない |

全体は目安3〜6週間、review修正・shaderでさらに伸び得る。A0は単独で確認可能だが利用者向け機能ではない。
A1は品質gateを満たした場合に早期出荷できる**同じfeatureの第一段階**。別のfadeを先に出す案ではない。
その場合もunit変更時は瞬時切替、ズーム・連結・Remoteは既存表示というscopeを明記する。A2は同じ設定を引き継ぐ。
Gを先取りしてbuilderを書き換える工程、全unit morph、Remote curlはこの見積り外。

## 9. テスト・性能gate（実機/性能受入れ未実施）

A0で **E統合後・effect未導入のbaselineを先に取得**。同commit系・fixture・settings・窓配置・refresh rateでOFF/ONを交互に3回以上。
warm/coldを分け、FHD/24MP、ZIP、uncached PDF、AI/LUT/カラー化、single/spread/Blank/landscape、LTR/RTL、指定％/px、二F12窓を含める。
30/60Hzのdown/up連打、held repeat、gamepad、wheel burst、seek、連結scroll、短interval slideshowを別々に測る。
既存記録（§2）を現HEAD測定の代わりにしない。製品起動は別の明示承認interactive枠で行い、実装中の非対話検査と区別する。

| 層 | 必須の検査 |
| --- | --- |
| pure state-transition | fake clockで0/25/50/75/100%、start/finish/cancel、同入力再開始なし、即ready/未ready→後着、Press/Repeat、no-op/jump、source/role/token/ROI不一致、memory pressure。queue/resumeがないこと |
| handler/ordering | ROOT→fullscreen転送、兄弟窓で共有input_seqのみ変化、sameframe down/upと複数edge/ordinal、egui再pass、released repeat、popup wheel、gamepad Press/Repeat/Continuousの継続とpoll replay、slideshow/jump原因。OFFと受理target数・順序・drop基数・load/work admissionが一致。新inputでold meshを残さず、cancelがActionを消費しない |
| paint/visibility | 最初のready frameで全incoming実occurrenceが実際のvisible faceへ寄与し提示receipt発行。occluded/cull/画面外/blank/shadowを実page proofにしない。右側のpreflight失敗/片側ROI不足/左右role変更でも全unitは同pass flat、prepare各page一回・submission一回・提示通知一回。片側curl残り/二重prepare/次frame fallbackなし。source存在だけのreceiptは拒否。既存all-or-noneとnative leaseを維持 |
| lifetime/VRAM/context | cache eviction・sequence release・cancel後もin-flight batch lease生存、最後のbatch後解放、Flatのcurrent素材に旧unitが残らない。source+output/mip dedup、at-rest/parked/batch会計、予算skip。A cancel/closeがBを変えず、fork Inactive、mount/deposit維持、同時二窓に独立resource |
| snapshot frames | synthetic番号付き頁・裏面・白紙、inject clockで5時点+cancel。single/spread・LTR/RTLの4route、gutter/gap/違うsize・Blank/Vacant/cover・landscape・透明PNG・rotation/trim・Light/Dark・小さいF12。中盤の旧/新面積、文字鏡像・接合・UV漏れ・影・endの通常画素一致を目視 |
| 表示機能 | 指定％/pxのcanonical size不変、pan/loupe/navigator hit規則、capture/copy/exportに影なし。AI/PDF final差替えを待たせずcancel。unit変更/G adoption/shift restore、動画境界、連結にcurlなし |
| perf/idle | OFFの処理経路とupload数を維持。ONの追加decode/upload/resample jobゼロ、retained bytes・mesh生成CPU・GPU draw/overdraw、二窓で計測。代表warm機会数/開始/完走件数とskip理由割合を必ず報告（下記）。cancel/finish/hidden後repaintゼロ。repeat/seek/連結がbaselineのまま |

### 9.1 速度維持とeffectの実用性を別gateにする

**全件skipして速いだけでは機能gateに合格しない。** 性能と実用性を別々に判定し、両方を満たしたstageだけを出荷候補とする。
A0で代表profile・VRAM設定・duration・品質条件を先に固定し、測定後にskipしやすいcaseを分母から除外しない。

- 必須warm profile: 通常fitのsingle、安定spread（各LTR/RTL）、二つのF12 readerが存在する状態で各readerのsingle/spread。FHD画像を4K表示する条件等、対象hardware/settingsを明記。各profile/runは30件以上の受理済み隣接操作、3run以上。手動操作間隔はD+2表示frame以上、同じ窓で完走してから次操作し、意図的取消を混ぜない。
- 分母 `opportunities` はON・静止paged・通常fit・supported layoutのwarm隣接受理で、target final sourceが受理時readyだった件数。pin budget不足、ROI不足、geometry preflight失敗、offered/source照合失敗は**skip理由として分母に含める**。unsupported modeやrepeat/jump等は対象外件数と理由を別に報告。理由なし除外を禁止。
- profile/contextごとに `opportunities / offered / started / finished / cancelled / skipped` の件数と、`started/opportunities`、`finished/started`、skip理由別の件数/分母比率を出す。startは実際のcurl提出、finishはcanonical終端の提出で証明し、候補生成だけを開始に数えない。frame replayで重複計上しない。run末尾でstartedはfinishかcancelへ収束し、未収束は失敗。
- 仮の出荷gateは各必須profileで **開始率90%以上、完走率95%以上**。少なくとも各profileに実start/finishがあることが必須で、全件skip/分母ゼロ/coverage不足はFailまたはCannotJudge。閾値・fixtureはA0で採択し、ONのperf比較には実際に開始したrunも含める。
- 24MP single/spread、指定サイズ、異なる寸法/landscape、同時に複数contextがpinする条件は追加の実用性profileとして同じ集計を必須にする。旧24MP二面だけで仮128MiB上限を超えるため、現案ではspreadが全件budget skipになる可能性を明記する。平均率へ混ぜて隠さない。

追加profileが全件skipなら、その条件でeffectは利用できないと結論する。A0で画素をコピーしないlease/予算の改善、stageの適用範囲を具体的に利用者へ提示して採択、または延期を判断する。
仮pin上限を未検証のまま「一般の高画素spreadも対応」と称してA1へ進めない。通常cache/target/入力を遅くして開始率を達成する案は採らない。
速さ維持の合格だけをもって品質・実用性・構造レビューの合格に置き換えない。
paint記録は `src/app/paint_record_test_support.rs:106` / `:188`、snapshotはtests/ui_snapshot.rs・tests/support/ui_harness.rs
（`docs/ui-snapshot-policy.md:97`）を拡張する。meshの数学だけでGPU/実presentの成立を主張しない。

`--perf-log`では既存 `fs/page_turn_decision` / `fs/page_turn_ready` / `fs/paint`、frame.begin、ui.fs_render_breakdown、ui.tail_repaint、
upload/schedulerと `gpu/vram_accounting`（`src/app/vram_accounting.rs:270`）を使う。
追加イベントは `page_turn_animation/start|skip|cancel|finish` と、最初のvisible/fully_visible receipt。
配送先context/window binding・source viewport、operation_id・phase/cause、診断input_seq、accepted unit・source/native texture identity、recipe/direction、reason、retain bytes、mesh CPU timeを相関する。開始/完走/理由別skip集計に必要な機会も一度記録し、共有input_seqだけでcontextを相関しない。
pose frameはpage提示数・新input_seqに数えない。既存paintのgeometry署名（`src/ui_fullscreen.rs:26667`）にposeを混ぜて新ページのように増殖させない。
OFF時はeffect診断用の時計・extrasを作らず即return。ログ欠落・coverage不足はCannotJudge。

```powershell
python scripts/analyze_perf.py <log> page-turn --check
python scripts/analyze_perf.py <log> page-turn-animation --check --profile single --run-id run-1
python scripts/analyze_perf.py <log> latency
python scripts/analyze_perf.py <log> hitches --ms 16
python scripts/analyze_perf.py <log> idle-health
```

`scripts/analyze_perf.py:930` のI1〜I5を維持し、animation visibility/cancel/leaseのcontext別相関を追加する。
特に **input→resource ready / ready→first visible / ready→fully visible / repeat input→通常target** を分離。
fully visibleが採択duration分遅くなるのは演出の費用として報告する。first visibleは通常ready frame、cancelは次の通常paintへ即反映し、追加wait frameゼロ。
ON単発の仮描画予算はCPU p95追加0.5ms/context、GPU追加2ms/4K frame以内（実測後採択）。offscreen uploadで予算達成したと偽らない。
repeat/seek/連結のp95/p99がOFF比5%以上または1ms以上悪化し続けたら原因調査、追加drop/順序違反・利用者に分かる悪化は閾値内でも不合格。
max VRAM/pin量、warm/cold、GPU/refresh rate、runばらつきを添える。複数窓で満たせなければ通常targetを優先しeffectをadmission skipする。

狭いstate/handler/paint→snapshot→fmt/glyph→共有変更のscripts/test-full.ps1、独立完了reviewが実装時のgate。
interactiveはscenario・時間・desktop使用・disposable dataを提示した別の明示承認suite。normal-profile起動は利用者だけ。
必要なbuild/portable-smoke/build-dev handoffは実装時に行う。**R3設計作業は文書のみだった。A0の製品差分と未完了gateは§12で別に記録する。**

## 10. 利用者決定と残る時間評価（2026-10-11）

Q1は2026-10-10にC方向・旧/新が見える実際のpage-turnとして採択し、2026-10-11に再確認した。Q5も2026-10-11にPC viewer限定・F12含む・Remote不要と再確認した。両項は再質問しない。

1. **Q2: 最終時間は動きを見てから決める。** 評価用の仮スライダーは100〜600ms・既定200ms。新内容は最初のready frameから見え、全面表示は選択した時間内。新入力/held/seekで即終了する。
2. **Q3: 既定OFF採択済み。** 2026-10-11の利用者判断は「速度が売り」。従来の瞬時切替を初期値に保ち、希望する方がONにできる。ONでも読込待ち・連打には演出を付けない。
3. **Q4: slideshowにも同じ効果を適用する。** 即ready条件と§6のduration/deadline条件を使い、intervalや次ページ提示を遅くしない。個別toggleは追加しない。

次々回への延期許容は品質と正しさのための工程判断であり、通常の入力を遅くする、既存機能を外す、ready提示を停止する許可ではない。
そのような変更が必要と判明したら実装前に費用と選択肢を提示して相談する。

## 11. 記録の訂正・設計担当自己点検・初回独立レビュー

**訂正・撤回**: R2冒頭と旧§11の「別contextで独立レビュー/再レビュー済み」「追加重大指摘なし」「構造合意済み」という完了記録を撤回する。
旧target/A-1341-design-r2-msg.txtの同じ主張も本R3 handoffで訂正する。今回、利用者が提示したレビューだけを初回独立レビューとして記録し、未実施のレビューを実績に数えない。
R2の三項目は以下の**設計担当の自己点検**であり、独立レビューや受入れの証拠ではない。

| 設計担当の自己点検 | 設計への反映（未検証） |
| --- | --- |
| 最後のprepared Lanczosは既存captureに保存されない | §5.2の実draw境界Flat記録、旧source再prepareなし、idle/batch会計 |
| 受理時にincoming prepared snapshotは存在しない | §4のOffered初回通常paint handoff、非ready候補・後着再開なし |
| seek/navigatorはpage描画後にも入力処理する | §4のpre-draw非消費fact。R3で配送先/操作identityを具体化 |

**初回独立レビュー（利用者提示、対象next-prefs / 1ee3e1782、読み取りのみ）: REVISE、P1なし。**
製品code編集・build・起動・性能測定なしというレビュー範囲もそのまま記録する。

| 初回レビューの指摘 | R3の設計修正と未完了gate |
| --- | --- |
| P2: 見開き全体のprepare/提出境界 | §4.1のunit全体を一度prepare→flat/curl決定→一度提出→submitted visibility receipt。A0で既存loop/提示通知への具体的接続を再レビュー |
| P2: 取消factの宛先/操作identity | §4.2のdestination binding・stable delivery id・Press/Repeat/Continuous/cause。共有input_seqで取消しない。root/兄弟/複数edge/replay/gamepadをtestsへ |
| P2: 未実施の独立レビューを完了扱い | 本節と冒頭で撤回、旧三項目を設計担当自己点検へ。R3再レビュー・合意の結果はまだない |
| P3: 全件skipでもperf gate成立 | §9.1にwarm single/spread/F12の機会/開始/完走と理由別skip率、速度と実用性の別gate。24MP予算skipの制約と工程判断 |
| P3: 決定事項と時間条件 | Q5をPC/F12のみ・Remote不要へ確定、質問はQ2〜Q4だけ。slideshow省略は採択duration D/既存deadline残時間へ連動 |

初回reviewerはimmutable lease/会計/入力取消/canonical geometry/連結分離/A0と延期方針を妥当とし、bundle presentation owner方向をdetached症状パッチではないと判断した。
**この方向への所見を、接続境界の設計承認・構造合意完了とは記録しない。** 初回結果はREVISEのまま。
上の初回REVISE記録は履歴として保つ。その後、利用者からR3 ACCEPTと伝達され、masterの`0f6ad9f19`（merge題名: `1.341 page-turn animation design R3, ACCEPTed`）へ統合された。ここで新しい独立レビューを実施したとの主張や、そのレビュー本文・検証証拠を補作しない。
Q3/Q4の採択とQ2の仮評価設定は§10のとおり。R3の設計採択はA0実装の完了レビューや実機受入れを代替しない。E統合後のA0接続、detachedの具体的範囲への構造確認、geometry/visibility・native lifetime・GPU/実present・性能/実用性gatesは別に閉じる。

## 12. A0実装差分・検証状況（2026-10-11）

基点は`0f6ad9f19`、対象は未コミットのA0差分。以下は追加した実装の境界であり、完成・出荷の証明ではない。

- `src/page_turn_animation.rs`はcanonical unitを受け取る純粋projection、可視面のreceipt、ROI/clip/layoutのpreflightを持つ。texture/addressの存在だけを可視提示の証拠にしない。実GPU/実presentの品質証明は別gate。
- `src/ui_fullscreen/page_turn.rs`は`PresentationState::{Inactive, Flat, Offered, Turning}`を所有し、最後に提出したunitからimmutable paint resourceを保持する。`ViewerOperationFact`は配送先context/window/viewport、stable operation ID、source viewport、Press/Repeat/Continuous/causeを持ち、診断input_seqだけで兄弟contextを相関しない。
- `PageTurnPresentation`を既存`ViewerContextBundle`でswap/deposit/mountし、forkはInactive、通常のpause/parkは状態を保持する。実際のclose/非表示/配送先変更/取消は当該ownerで処理する。Appの共有`PageTurnLeaseInventory`はweak参照の会計用一覧であり、contextのpresentation状態やnavigationを所有しない。source/outputの既存VRAM会計へ接続し、parked/in-flightも含めた保持量とadmissionを扱う。native lifetime・複数窓での実解放は未検証。
- 単ページと見開きの通常prepareから同じpass内で全unitのflat/curlを排他的に提出する。holdoverを採用する場合はそのunitだけをeffectの判断へ渡し、表示しなかったcurrent候補を提示証拠にしない。最終のegui output hookで破棄passを除き、実occurrence/source/nativeと正のvisible areaを持つreceiptだけを採用する。
- 受理時のready判定は、現在のedit/final keyに一致する完成済みnormal textureと、読込済み注釈のresident合成結果だけを参照する。raw cache/preview/thumbnailをnormal finalの代用にしない。未準備時は`not_final_ready_at_accept`で機会外を記録し、従来の通常producerと瞬時表示へ進む。後着から演出を開始せず、accept側で処理・metadata読込・uploadを増やさない。単ページ/見開きの正例fixtureは通常producerで素材を準備し、cold rawの見開きは別回帰で確認する。
- UVは通常描画と同じinverse rotation・trim変換から作る。cursor座標の逆変換を描画素材のUVとして流用しない。終端PNGは製品の通常`paint_texture` helperで別描画したcanonical画素と完全一致させる。
- ONの既存ready observerへの通知は`PendingFsPageTurnReady`で当該contextへ保持し、次のpre-drawで新しい入力/target操作より前に採用済みreceiptを検査して行う。これは既存observerへの出力証拠の配送であり、新たなnavigation要求・待機・load admissionを作らない。元画像/通常final/passthroughの既存source選択、items/input generation、unit/layout/配送先を照合する。OFFはstack上の即時flat receiptで従来の同pass observer経路を使う。pendingのactual paint resourceもVRAM会計へ含める。
- §7のOFF/仮時間を既存Settings、preferences draft/OK、SQLite KV、settings transferへ追加した。UIは既存「閲覧表示」ページと検索索引を使う。新DB・本別設定・slideshow専用toggle・KeyActionは追加しない。
- OFFではeffect用のraw-input観測、native edgeのeagerコピー、pad batch/replay観測、隣接unit判定、projection・pin・effect診断を行わない。generic egui consumerが実際に一致した入力のprovenanceだけはlazy保持して安定IDを守る。OFFの割当があらゆる入力でゼロ、または実測で速度差ゼロという主張ではない。ON時の新操作がactive turnを取り消した場合も、受理済みrepeat/jump等の機会外理由をtyped dispositionで一度だけ記録する。

**解析器**: `python scripts/analyze_perf.py <log> page-turn-animation --check`を追加し、従来の`page-turn --check`のI1〜I5とは別に集計する。`context/window/operation_id`の文字列でreceiptを相関し、replayを重複計上しない。profile/context/windowごとに機会・offer・start・finish・cancel・skip、開始率/完走率、理由別skip率、mesh CPU時間・保持bytes、ready→first/fully visibleを出す。入力時刻は実際のWin32 enqueue / egui raw delivery / pad sample / pointer delivery / slideshow activationに付けた`input/key_edge_ingress`と同じoperation IDで結ぶ。受理時刻や診断input_seqから入力時刻を補作しない。これはアプリ内への配送時刻であり、物理入力・OS queue到着時刻ではない。可視receiptも採用されたegui出力の提出証拠であり、OSの実presentを証明しない。後者は別承認の実機枠で検証する。

ログに測定ラベルがなければ`--profile single --run-id run-1`で補える。既定の必須warm profileは`single/spread_ltr/spread_rtl/f12_single/f12_spread_ltr/f12_spread_rtl`、F12は二readerを必要とする。各contextに3run×30機会を要求し、一つのcaptureを3runとみなさない。`--required-profile`で評価範囲を明示できるが、範囲外profileの受入れを意味しない。全件skip・未収束start・矛盾した終端・終端後のanimation frameはFail、機会ゼロ・coverage/receipt/metric不足はCannotJudge。既存stored delivery checkpoint/ACK/manifestによるログ配送検証も使い、証拠不完全ならCannotJudge。`--json-out`で集計を保存できる。

**実施済みの限定証拠**: 基点`0f6ad9f19`からの本A0差分で以下を実行した。Cargoは`MSBUILDDISABLENODEREUSE=1`。合成ログ・fake input/stateとheadless GPUによる回帰であり、製品の描画速度・実present・実VRAM計測ではない。

| コマンド/対象 | 結果 | 記録 |
| --- | --- | --- |
| `cargo fmt --all`、`cargo fmt --all --check` | exit 0 | agent command結果 |
| `python scripts/check_ui_glyphs.py` | exit 0、危険glyph 0 | agent command結果 |
| `python -m unittest discover -s scripts -p test_analyze_perf.py` | exit 0、101件成功 | `target/A-1341-a0-python.log` |
| 新preferences PNG生成: `UPDATE_SNAPSHOTS=1 cargo test -p mimageviewer --lib page_turn_animation_settings` | exit 0、2件成功。最終文言は非適用条件のみ説明し、既存loading/holdoverの変化を約束しない | `target/A-1341-a0-pref-snapshots-3.log` |
| `cargo test -p mimageviewer --lib page_turn`（最終文言/素材復旧後、snapshot更新なし） | exit 0、75件成功 | `target/A-1341-a0-focused-final-3.log` |
| `cargo test -p mimageviewer --test ui_snapshot` | exit 0、157件成功 | `target/A-1341-a0-ui-snapshot.log` |
| `cargo test -p mimageviewer --lib`（指定どおり無filterは1回） | exit 101、12,035成功・55 ignored・6失敗。下記のfixture/golden/vendor環境を整備して対象を再検証 | `target/A-1341-a0-lib.log` |
| `cargo test -p mimageviewer --lib app::tests::still_window_mode_key_tests` | exit 0、548件成功。root Key fixtureを使う入力/held/方向/F12を含む群 | `target/A-1341-a0-root-key-regression.log` |
| `cargo test -p mimageviewer --lib effetune_host_handler_tests` | exit 0、3件成功。復旧したsource-compatible hostでheadless IPCを検査 | `target/A-1341-a0-host-regression.log` |
| `cargo test -p mimageviewer --lib effetune_notice_texts` | exit 0、1件成功。復旧したv0.13.0とtracked notice一致 | `target/A-1341-a0-notice-regression.log` |
| `cargo test -p mimageviewer --lib preferences_viewer_notice_visibility_snapshot`（更新後、UPDATEなし） | exit 0、1件成功 | `target/A-1341-a0-viewer-golden-check.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0 | `target/A-1341-a0-normal-check.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core --features portable` | exit 0 | `target/A-1341-a0-portable-check.log` |
| `.\scripts\build-dev.ps1 -PreserveRuntime` | exit 0。通常profileのcore/remote/EPUB worker、VCRT/PE検査（runtime 4、PE 3）成功。起動なし | `target/A-1341-a0-build-dev.log`、`target/vcrt-pe-reports/dev-runtime.json` |

新goldenは19枚（設定2枚、五時点×8routeのstrip 8枚、実mesh提出helperの単ページ/LTR・RTL見開きのpre/mid/end 9枚）。全19枚を目視し、向き・文字の非鏡像・透明部/underlay・gutter・設定数値欄を確認した。既存`preferences_viewer_notice_visibility.png`は、新設定でscroll contentが増えたscrollbar thumb終端の10 pixel差分だけを目視して更新した。8routeの終端は通常paint helperで別描画したcanonical PNGと完全一致する。実アプリを起動した検査や動きの受入れではない。初期の単ページreferenceのtriangle対角差、raw sourceをnormal finalと誤認した正例fixture/受理判定、同frameで複数操作を作った取消fixtureの失敗は修正後の焦点結果で解消した。raw sourceは受理側でnormal素材を作らず、通常producerを使うcold回帰を別に通した。

無filterで出た6失敗の内訳は、backendと異なりpass開始後にKeyを足す既存root-input fixture 1件、上記scrollbar golden 1件、vendor環境4件（EffeTune v0.12.0と要求v0.13.0の不一致1件、古いhostのIPC Ready version不一致3件）。root-input helperはRawInputへの投入とrepeat用の事前keys_down設定へ直し、製品consumerのidentity条件を緩めていない。productionのnative/synthetic/IME/native overlay ingressはすべてRawInputまたはnative delivery batchに入ることを読み取り確認した。

このworktreeのignored vendorは旧素材だった。master worktreeのv0.13.0を本基点の承認済みmanifestと全SHA256で検証し、host PEも本基点のCMake/include/src/testsから求めたsource identity `2899e71293f96a8ffe5852b1a991b0a6d6aabe67a9c7f61e5e6ca3cd13f52541`との一致を確認して実ファイルで復旧した。元のplugin/hostは`target/A1341-E12`、`target/A1341-VH-old`へ保存した。APPDATAやhost cacheから取り込まず、bridge source/protocolを変更していない。復旧後の全hash/identity照合はexit 0（`target/A-1341-a0-assets.log`）。6件は上表の対象群の再実行で解消し、未変更の経路の全体実行結果を再利用する。無filter全体がgreenだったと扱わず、指定に従い無filterを繰り返していない。

**確認build handoff**: ビルド直前にこのworktreeのcoreが動いていないことを確認し、`-PreserveRuntime`で正常終了した。`target/dev-runtime/mimageviewer-core.exe`はportableではなく、既定で通常の`%APPDATA%\mimageviewer`を使う。実settings/dataを更新し得るため、利用者はインストール済み・tray常駐のmImageViewerを先に閉じてから、`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`で起動する。実装者は起動していない。`target/A-1341-a0-msg.txt`に単ページ/見開きLTR・RTL、F12、取消、slideshowと仮時間の確認手順を記録した。コミットしていない。

**未完了**: 出荷向け`test-full.ps1`、A0実装の独立完了レビュー、detached接続の具体的構造確認、別承認interactive、OFF/ON baseline・§9.1のwarm/追加profile実測、利用者による時間/動きの評価と最終時間採択は別gate。製品起動禁止の本handoffではbaseline/実機計測を行っていない。実装側の今後の結果はその時点の対象差分・command・exit/count・logとともに追記し、予定を実績にしない。

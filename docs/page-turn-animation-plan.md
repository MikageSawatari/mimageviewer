# ページめくりアニメーション設計案 — backlog §1.341

2026-10-11 / **R5設計案（§16、未実装・再レビュー/採択待ち）**。R3はACCEPT後に統合され、A0と追補もコミット済みだが、利用者の実機評価で動きは不合格となった。R4（§15、`afc474d8d`）の独立設計レビューはREVISE。R5では背を軸に一枚のleafを返す方針を維持し、取消targetのflat提示を次navigationより先に保証し、異幅Pair・中央単頁の逆送り・分割読みの境界を具体化する。既定OFF・仮時間・final-only・F12所有・VRAM会計は維持する。§1〜15は履歴を含み、衝突箇所は§16の置換表を優先する。R5の独立レビュー・実装/実機/性能受入れは未了。利用者の2026-10-11決定で、リリースは次の次の版へ延期済み。

R3設計時の参照treeは `next-prefs` / `1ee3e178288f2eff119da16b672d546cc3c9992c`（R2文書commit、開始時clean）。当時の製品コードはmaster `cb00930011323cacd7088ae70c37835141aad58a` と同じだった。
既存の `file:line` は各設計/追補時のsource inspection位置。A0は§1.364統合後の`0f6ad9f19`を基点に具体化した（§12）。A0は`532635fdd`、golden補完は`e0f323d24`、HUDは`bf3b2fae0`、PDF追補は`ea5f6624a`、保存判断の来歴訂正は`e3a328ff6`でコミットされた。§15はこのHEADと利用者提示の実機結果を基点とする。既存の自動検証成功は動きの品質合格を意味しない。
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
既存preferences draft/save API/settings transfer round-tripを使い、欠落fieldはOFF/200msへ補完する。通常設定の時間は100〜600msへ補正し、設定転送は型違い・範囲外を拒否して現在のdraftを保つ。2026-10-11 HUD調査でsave workerという当初の記述を訂正した。既存の保存は同期APIであり、HUD接続の方式確認は§13に記録する。
ON→OFFは即取消、OFF→ONは次の適格な操作から。設定確定に本reloadを足さない。
slideshowは同じ有効設定と時間を使い、追加toggleは作らない。Q5のPC限定は決定済み。

当初のPreferences-only A0では新KeyAction/shortcutは不要。取消は非消費のpresentation処理で、既存page Action・SinglePerFrameを維持。
2026-10-11追加利用者要求（`target/A-1341-user-answers.txt`）で「その他の機能」にON/OFF行も追加する。
既存overflowのON/OFF行はKeyActionと実割り当て表示を持つため、`FsPageTurnAnimationToggle`（FsImage/Press、既定なし）を同じ方式で追加する。
固定並びのため表示項目カスタマイズは追加しない。Settingsの同じboolと時間を使用し、PreferencesとHUDは共通のadoption処理で取消とinput observation policyを更新する。別のbool、窓別値、reloadは作らない。
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
4. **2026-10-11の実機再評価: A0の動きは不合格。** 利用者は「本のページをめくるように見えること」を要求し、RTL forwardでは旧左頁だけを持ち上げて右へ返すこと、普通のscanの寸法/配置差で省略しないことを明示した。新しいmotionのR4案を§15へ記録する。§15の通知契約・数値・instant範囲まで採択されたとは扱わない。

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

## 13. HUD追加要求（2026-10-11）

A0は利用者が`532635fdd`、projection golden補完を`e0f323d24`でコミットした。HUD追補は`e0f323d24`を基点に実装され、利用者が`bf3b2fae0`でコミットした。
`target/A-1341-user-answers.txt`の追加要求に従い、PC静止画上部の「その他の機能」に同名のON/OFF行を追加する。固定順のナビゲータ位置の後へ置き、既定OFFと仮時間は維持する。
既存toggleのKeyAction/実割り当て表示の方式を使い、既定キーなしの`FsPageTurnAnimationToggle`を追加する。固定順はカスタマイズ一覧ではないため、toolbar/HUD用の別設定やring Actionは作らない。

PreferencesとHUDは`adopt_page_turn_animation_settings`に設定採用を集約する。全体Settingsの同じbool/時間、既存の全presentation退役とingress policy更新だけを使用し、window/contextの別設定、reload、pending stateや保存の待機は追加しない。既存ownerの採用経路へ合流する簡素化を選び、専用のlive rebuildや非同期保存の後着を扱う機構は導入しない。

**保存方式の決定（2026-10-11、設計担当 ClaudeCode。既存経路の踏襲）**: fullscreen overflowの既存Settings toggleと同じ経路を使う。source照合ではナビゲータ表示・位置・サイズと本別調整保存toggleが同期`Settings::save()`を呼ぶ。ページめくりも共通toggle出口で設定採用直後にこの呼出しを再利用する。Preferencesは既存の`save_checked()`のまま。save workerという旧前提は撤回し、専用async writer、dirty state、待機、全Settings snapshotの後着保存は追加しない。保存失敗は既存save経路の扱いを継承する。通常のページ送り/paintから保存を呼ばない。

HUDコミットまでの検証時は利用者がA0 dev buildを確認中だったため、`build-dev.ps1`や`target/dev-runtime`への書込み、製品起動は行わなかった。検証を`target/debug`のcheckとheadless testsに限定し、結果と残件を`target/A-1341-hud-msg.txt`へ記録した。§14の追加指示では利用者がtest buildを終了し、worktreeのcore不在確認後の確認用buildを許可した。独立レビュー・実機受入れの実績を追加しない。

以下はHUDコミット`bf3b2fae0`までの検証履歴。保存接続後のDB再読込とprojection修正の検証は§14へ記録する。Cargoは`MSBUILDDISABLENODEREUSE=1`。

| コマンド/対象 | 結果 | 記録 |
| --- | --- | --- |
| `cargo fmt --all --check`、glyph lint、`git diff --check` | exit 0、危険glyph 0 | agent command結果 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0 | `target/A-1341-hud-normal-check-final.log` |
| `cargo check --features portable` | exit 0 | `target/A-1341-hud-portable-check-final.log` |
| `cargo test -p mimageviewer --lib page_turn` | exit 0、80成功 | `target/A-1341-hud-focused-2.log` |
| `cargo test -p mimageviewer --lib keymap::tests` | exit 0、153成功 | `target/A-1341-hud-keymap.log` |
| `cargo test -p mimageviewer --lib fullscreen_overflow` | exit 0、6成功 | `target/A-1341-hud-overflow.log` |
| `cargo test -p mimageviewer --test ui_snapshot` | exit 0、157成功 | `target/A-1341-hud-ui-snapshot.log` |

HUD rootの既存PNGを更新し、ON代表PNGを1枚追加して両方を目視した。Navigator submenu PNGは未変更。UPDATEなしの通常検査も成功した。初回lib compileのprivate context API参照は検査を既存context moduleへ移して解消し、APIの公開範囲を広げていない。新keymap fixtureの失敗は既存のkeyboard-owner cache/input guardへ揃えて解消し、製品側の入力条件を緩めていない。当時のcoreのsize/mtimeはA0の325133312 bytes / LastWriteTimeUtc 2026-10-11 02:18:42のままだった。

## 14. 実PDFのprojection拒否・HUD保存追補（2026-10-11）

基点は利用者がコミットした`bf3b2fae0`。利用者はnormal profileの実PDFで演出が始まらないと報告した。`%APPDATA%/mimageviewer/logs/perf_events.jsonl`を読み取りだけで調査し、47件の`not_final_ready_at_accept`、18件の`rendition_or_burst`、11件のready opportunity/offeredと、その11件すべての`geometry_visibility_or_roi`を確認した。製品やPDFを起動・変更していない。

### 14.1 失敗条件と修正境界

generic reasonはpure `project_turn`の失敗をまとめたものだった。callerの`canonical_unit`のfree rotation/partial ROI拒否は元から別reasonであり、この11件のgeneric reasonとは区別できる。ready区間の既存`fs/paint`は全件が二つのPDF occurrenceを示す。利用者が述べたsingleの実geometryは旧ログから確定できないため、single fixtureは同じportrait aspectを16:9へ中央fitした再構成とする。
旧ログには実viewport/clip/prepared ROIの値もなく、fixtureのmedia clipは再構成値である。拒否の最初の成立条件であるpaint rect不一致は実記録から確認できる。その他の値まで実測済みと扱わず、新しいskip geometry診断で次の実機確認時に残りを記録できるようにする。

実座標例（t330.412→330.823、point単位）:

| unit | 左 paint rect (x,y,w,h) | 右 paint rect (x,y,w,h) |
| --- | --- | --- |
| PDF 18/17 | 512.66656,44,766.66669,1182.66663 | 1280.66663,44,766.66663,1183.33337 |
| PDF 20/19 | 512.00006,44,767.33319,1182.66663 | 1280.66675,44,766.66663,1182.66663 |

旧preflightは最初のold/new paint rectの`!=`で拒否する。さらに左右width/top/bottomにもfloat完全一致を要求し、通常のportrait fitの小差も拒否した。PDFも読み取り専用で確認し、全26頁、index 17〜22のMediaBox/CropBoxは約499×725 PDF pointで、499.32×726.36、498.6×724.56、499.44×725.76、499.32×724.92、499.44×725.4、499.56×725.04と元寸法にも小差がある（回転なし）。したがってすべてを同一page boxの丸めだけと断定しない。2/3ptのpaint格子はeffective ppp=1.5相当（旧イベント自体にはpppなし）。source rasterは1343〜1346×1954、scaleは約0.605。paint幅はtrimを反映するためraster幅×scaleとは同一でなく、UVは既存transformをそのまま使う。

修正はstable paint geometryの比較単位を描画先transformのeffective pppへ揃える。独立のround-to-nearest edgeに各0.5物理pxの誤差があるため、fitの小差を許容する上限を1物理pxとし、辺・左右幅/上下位置をこの範囲で比較する。元寸法の小差を一般の異寸法morphへ拡張しない。f32演算誤差は辺の座標から計算し、幅の引き算にも元の座標の大きさを使う（右へ移した小型goldenがこの負のpremiseを検出した）。paint rect、clip、UV、source/native resource、prepared ROIは書き換えず、page sheetや別layout ownerを作らない。終端はincomingの実canonical rect/UVをそのまま提出する。clip/DPI/layout/role変更、overlap、1pxを超える寸法/配置変更、ROI不足、可視面積/8pt短辺/中盤20%条件は維持する。outgoingとincomingの両spreadを検査する。

実ログのPDF 22は幅814.66669pt（通常約767pt）で、これは丸め差ではない。22への切替と戻りは従来A0の異寸法/配置変更除外のままで、全ready機会を開始できると主張しない。このページを含む一般の不等寸法morphは本追補の範囲外。通常flat提示を遅らせない。

geometry skip/cancelは既存reasonを保ち、`sub_check`に`page_placement_changed`、`spread_slot_width/top/bottom`、`destination_clip_changed`、UV/可視面積/短辺などの具体的条件を付ける。両unitのpaint/clip/viewport rect、effective ppp、source/native size/id、UV/prepared ROIも添える。JSON構築はperf有効時だけ。OFF経路・prepare一回/全unit同pass提出・destination/operation identity・lease/VRAM契約は変更しない。

### 14.2 readinessの調査（final-only規則は維持）

ready lookupは現edit/params keyのresidentかつcompleteなfinal composite、読込済みannotation metadata、必要なら一致するcomic composite、texture identityを要求する。PDF raw prefetchやCPU AI prefetchだけではfinal textureの存在を意味しない。AI OFFでも初回final compositeは通常の可視処理経路で作られる。

47件中45件はt326.5より前で、ログにはt319までAI workと多数の`complete=false`がある。AI OFF切替とみられる後、19/20のfinalはt326.456/.463でcomplete=trueに再構築された。残る2件は22/21（t326.903、同世代で先行paintなし）と18/17（t327.837、以前のpaintはAI期間）で、どちらも受理後にcomplete finalを作る。その後の暖まった往復は11件ともreadyで、失敗はgeometryへ移る。AI設定変更は既存generation更新でfinal cacheを消す。旧ログに受理時key/entry状態/metadata residencyがないため、47件すべての内部失敗predicateやAI設定値を確定したとは言わない。

この証拠で通常warmのreadiness bugとは判断しない。final-onlyを緩める、受理時にproducerを起動する、animation用prefetch、後着で開始する、通常cache保持を増やす変更は行わない。最終品質を待たず演出する仕様への変更は利用者判断として残す。

### 14.3 保存・簡素化・検証

§13の設計担当ClaudeCodeの決定（既存overflow経路の踏襲）に従い、HUDと任意KeyActionの共通toggleは`Settings::save()`を呼ぶ。これは利用者による保存方式の決定ではない。Preferencesと同じSettings owner/adoptionを使い、F12の変更でitems/idx/input sequenceを変えない。headless実行でON→OFF各clickとF12 OFFの後、隔離DBを再読込してboolと仮時間を確認する。

非同期保存や追加pendingの組合せを作らず、既存同期保存の出口へ揃える簡素化を採用する。geometryは一つのpure比較境界で補正し、viewport reloadやreader別の丸め状態は作らない。まれな保存失敗は既存経路を継承し、新たなretry/recoveryは追加しない。

実ログのspread矩形とportrait singleの16:9 letterbox fixtureを加え、LTR/RTL・前後・5時点以上・DPI 100/125/150/200%・非zero originを検査する。2px pan、大きい実寸法変更、clip変更、ROI不足は拒否する負の回帰を含む。projection goldenはsingle/spread各LTR/RTLの4枚を追加し、終端の通常paintとのheadless画素一致も検証する。実GPU/OS present、実機の開始率/完走率、OFF/ON速度、最終durationの採択と独立レビューは引き続き未了。今回のコマンド結果は`target/A-1341-fix-msg.txt`へ記録する。

| 今回の検証 | 結果 | 記録 |
| --- | --- | --- |
| fmt check / glyph lint / diff check | exit 0、危険glyph 0 | agent command結果 |
| normal core check | exit 0 | `target/A-1341-fix-normal-check-final.log` |
| `cargo check --features portable` | exit 0 | `target/A-1341-fix-portable-check-final.log` |
| `cargo test -p mimageviewer --lib page_turn`（UPDATEなし） | 86成功、0失敗 | `target/A-1341-fix-focused-final.log` |
| PDF golden生成、終端画素比較 | 5成功、4枚目視済み | `target/A-1341-fix-pdf-snapshots-verified.log` |
| `cargo test -p mimageviewer --test ui_snapshot` | 157成功、0失敗 | `target/A-1341-fix-ui-snapshot.log` |
| `build-dev.ps1 -PreserveRuntime`（通常profile） | exit 0、core/remote/EPUB成功、VCRT/PE runtime=4・pe=3。起動なし | `target/A-1341-fix-build-dev.log` |

初回の小型spread goldenは幅の引き算に必要な座標丸め誤差を検出し、計算境界を修正してから上の最終gateを通した。閾値を大きくして通したものではない。unfiltered lib/workspace検査を成功したと主張しない。確認用buildはcore process不在を直前に確認して`build-dev.ps1 -PreserveRuntime`で行い、結果をhandoffとbuild logへ記録する。製品起動は利用者のみ。

## 15. R4 — 背を軸に一枚のleafを返す再設計（2026-10-11）

**状態: 設計案のみ。製品code・golden・設定・実行binaryは変更しない。build/起動/独立レビューも行っていない。**
設計担当から伝達された利用者の実機評価は「本のページをめくるように見えること」であり、A0の帯状の動きは不合格。
R4はこの要求を具体化する案で、以下の数値・shape対応・可視提示契約の改訂まで利用者が採択したと扱わない。

### 15.1 実機で反証された前提と黒い帯の原因

| 証拠 | 確認できる原因・限界 |
| --- | --- |
| 利用者報告: scanned PDF / spread / RtlCover、8/8が`page_placement_changed` | 読み取り専用の現perfログでも8件を確認。最初の旧左rectは`[2.0,44,699.33331,1056]`、新左は`[0.66667,44,699.33337,1056]`（min/max、point単位）、ppp=1.5。sourceは旧1151×1670、新1153×1671。左端差1.33333pt=2物理pxは実際のfit差であり、1pxの丸め誤差ではない。`preflight`のold/new配置一致条件が、普通のscanを非対応にしている。閾値を2px、5pxへ増やす修正では解決しない |
| 利用者報告: 20 curls / 1455 framesだが黒い帯にしか見えない | `canonical_unit`は`FsBgStyle::Solid(BLACK)`をpage underlayへ変換し、`project_turn`のsingle reverseは`old.underlay.clone()`を**裏面materialそのもの**に使う。通常初期値`fs_transparent_bg_mode=0`はBLACKなので、背面の幅広いflat stripと細いcurve stripが黒くなる。画像textureの裏面を参照し忘れたのではなく、「画面背景を紙の裏とみなす」明示的な素材選択の誤り。通常のimage underlayは維持するが、紙のreverse materialと別概念にする |
| 現投影の`screen_rect` / `fold` / `radius` | 全faceは旧頁のtop/bottomを持つ軸平行矩形。foldは幅方向を移動し、radiusは頁幅の2.5%だけ。depth、perspective、広いleafの回転、revealed pageへのcast shadowがない。既存shadeは細帯ごとの一様tintで、黒いmaterialはtintを変えても黒い。この形と素材の組合せが「帯が横へ移動する」見え方を直接作る |
| 利用者報告: spread全体が一枚に見える | コードは`turning_slot`を選び、旧反対頁を静止描画している。したがって「二頁全体を一つのmeshで回した」とは断定しない。しかし裏面の新反対頁を旧頁側の`screen_rect`へ貼り、折り線を動かして露出させるため、背を支点として反対slotへ回る一枚のleafになっていない。既存RTL forward goldenも途中の旧左頁側にD4が現れる帯状投影を示す |
| 実提出・UV/clipの切り分け | 現ログではstartにsource/native texture IDと正のareaがあり、finishまで続く。spread backは`incoming.pages[1-turning_slot]`のtextureを明示使用する。mesh UVの欠落、全裏面のtexture未登録、黒いunderlayだけの描画漏れを示す証拠はない。現`Builder::face`は参照pageのaxis-aligned clipを移動面にも使うため、対応範囲を広げる際はdestination clipとの分離も必要だが、今回の黒色はsingleのmaterial選択で説明できる |

参照ownerは`src/page_turn_animation.rs::{project_turn,preflight,Builder::face}`、
`src/ui_fullscreen/page_turn.rs::canonical_unit`、`src/ui_fullscreen.rs::{transparent_bg_style,fs_bg_style}`、
`src/app.rs`の初期background mode。既存single-mid goldenも紙ではなく高さ一定の中央帯であることを目視した。
ログにbackground mode自体はないので、各captureのBキー選択状態まで復元したとは言わない。
黒背景経路が実装上必ず黒いreverseを作ることと、実機の「黒い帯」という観測が整合する、という原因の特定である。

今回読み取ったログ全体は24 start/24 finish/1731 frame（single 20 start、Rtl spread 4 start）を含む。
これは利用者が提示した20/1455の集計範囲と区別し、後の追加試行を同じ実験とみなさない。
8件のgeometry拒否と、成立した提出でも品質不合格という二つの問題を別に閉じる。start/finish数だけでは合格にならない。

### 15.2 book model — 紙一枚、表裏二面、固定された下の頁

既存compositionが返すscreen-left/right、occurrence/role、singletonの形成理由・配置を正本とする。
`idx±1`、paint rectの左端だけ、`pages.len()==2`だけから物理slotやcoverを推測しない。
綴じ方向は既存の`mode.reading_direction().unwrap_or(context.reading_direction)`から一度捕捉する。
現callerの`mode.advances_right_to_left()`はSingleで常にfalseになり、保持しているRTL方向を表さない。
R4ではpresentationの背/回転に既存contextの方向を使い、forward/backwardは受理済みnavigation causeをそのまま使う。
矢印キー、tap、seekの既存方向や入力基数を変更せず、animation用の第二の綴じ設定を足さない。
**一回のturnで動くleafは一枚だけ**。静止する相方まで同じsheetに入れず、leafのfront/backは同じ頂点位置に別materialを持つ。
front/backをcross-fadeして一面に貼る方式は使わない。viewerに向いた面を法線で選び、隠れた面は描かない。

旧左=A、旧右=B、新左=C、新右=D。Sは持ち上げる側、Tは着地側。

| direction | S→T | leaf front | leaf back | leafの下に置く新頁 | 閉じるまで残す旧相方 |
| --- | --- | --- | --- | --- | --- |
| LTR forward | 右→左 | B | C | D（右） | A（左） |
| RTL forward | **左→右** | **A** | **D** | **C（左）** | **B（右）** |
| LTR backward | 左→右 | A | D | C（左） | B（右） |
| RTL backward | 右→左 | B | C | D（右） | A（左） |

frontはSで平らな旧頁から持ち上がる。角度が90度未満では表が見え、下の新S頁が自由端側から現れる。
90度付近では紙は薄いedgeになり、旧T頁がそのまま見える。90度を越えると新T頁を印刷したbackが見え、Tへ倒れて旧T頁を覆う。
最後は新S/Tの通常描画になる。RTLは位置・回転の向きだけを反転し、文字のUVを鏡映しない。
reverseでも同じS/T対応を使う。slideshowはforwardの同じrecipeで、別motionや専用状態を作らない。

描画順は、通常canvas背景 → 新S頁 → 旧T頁 → 受ける面にclipしたsoft shadow → 浮くleaf。
Tの新頁を初めから平面にも二重描画しない。backがTへ入るまでは新T頁は不可視であり、存在だけで提示済みにしない。
gutterは通常の空間として固定し、leafが浮いた投影だけが背を跨ぐ。HUD/navigator/seekは通常の前面に置く。

**single-page表示は本の片側だけを見る窓として定義する。** LTRの背は頁左端、RTLは頁右端（reading-forwardの自由端の反対）。
forwardは現在頁のfrontをこの背から起こし、次頁を通常の自身のrectで下に置く。背の反対側へ回った部分はsingleのbook apertureでclipし、
90度を越えて隠れた旧leafを画面端へslideしたり、200ms後に突然消したりしない。横長頁も一枚で、中央を分割しない。
backwardはその時間反転で、**前の頁（incoming）一枚が同じ背から戻って、現在頁（outgoing）の上へ着地する**。
LTR backwardは左側から、RTL backwardは右側から入る。逆操作だけ背を反対端へ移す「旧頁をもう一度捨てる」方式は採らない。
incoming frontは自身のrectへ着地し、outgoingはその下で静止する。incomingの見える部分が初回poseからあるよう、窓外の不可視の半回転は演出時間に含めない。

singleのreverse印刷はデータにない。必要なedge/backの演出素材は控えめなneutral paper（例: RGB 238/236/230）と陰影であり、
黒背景・checkerを紙の裏へ流用しない。知らない別頁、鏡像の旧頁、仮のoccurrenceを作らない。
frontの透明部分は従来のunderlay/checker/alphaをそのまま使い、frontの透明性を紙色で塗り潰さない。
このneutral paperは一時的なreverse surfaceだけで、通常表示・capture/export・保存画素を変更しない。

### 15.3 leafのgeometry・perspective・陰影

**R5注記: Pairでも通常の背は移動する。静止面とgutterの契約・admissionは§16.3で置換する。**

素材と通常geometryは分離する。各面は自身のoccurrence、prepared resource、rotation/trim UV、canonical paint/layout rectをimmutableに持つ。
新Sと旧Tの平面は各々の通常rectのまま。leafのfront endpointは旧S、back endpointは新Tの実rectへ正確に一致させる。
**old/new rect同士の一致・左右頁の等幅/同じ高さをadmission条件にしない。** 普通のscan差、portrait/landscapeの非対称Pairも対象にする。
同じ紙の二面が異なるscan boxを持つことはpresentationの問題として扱い、sourceのfit/scale・PDF demandを作り直さない。

背は通常layoutのslot/gutterを解決した位置から捕捉する。trim後paint端の微差を「新しい背」と誤認しない。
通常Pairでは同じvirtual spreadの背が支点となる。coverの中央表示から開いたspread等でcanonicalな背の位置が変わる場合は、
旧背→新背を一回のrecipeの中で連続補間する。これは**同じleafが付く一本の背の移動**で、独立して頁幅を横断するfold線ではない。
window移動/resize/DPI/zoom/pan操作による背の変更は補間対象にせず、従来どおりcancel+通常表示。
normal layout ownerの既存slot/placement情報をprepared unitへ添えるだけとし、projectionに第二のfit resolverや端判定を作らない。

純粋projectionの座標は次を基準にする（製品codeではなくgeometry契約）:

- u=0は背側、u=1は自由端。vは上下。Sの左右符号をσとし、leafの距離d(u)、高さy(v)、背Hを持つ。
- leafは`x=H+σ*d(u)*cos(θ)`、`z=d(u)*sin(θ)`で背の周りに回る。zはviewer側が正。
  initial/finalの隙間を含むdとyは各面の通常rectから作り、端点のgap・top/bottomも一致させる。
- 異寸法のfront/backは、θ=60〜120度の狭く見える区間でd/y/必要ならHをsmoothstep補間する。
  0〜60度のfrontは旧寸法、120〜180度のbackは新寸法。UVは各面の自身の通常UV basisで、隣頁への引伸ばしや再fitをしない。
  これはscanの違いを表示上吸収する一枚のvirtual leafであり、scan寸法から実物の紙寸法を推測したとは言わない。
- perspectiveは背の中央を基準に`F/(F-z)`でx/yを投影する。Fはleaf envelopeの最大寸法の8倍以上とし、near planeを跨がせない。
  浮く自由端で上下の広がりが変わる台形、幅のforeshortening、背に留まる辺が見える。x幅だけ縮める軸平行quadは不可。
- 基本は自己交差しない一枚の3D leaf。必要な紙らしい小さい反りは同じu/θ関数に限定し、背で変位ゼロ、平らな端点で反りゼロ。
  折返し帯や自己交差する円筒、別のspring/物理simulation/途中resume状態を復活させない。

egui meshを第一候補にする。perspectiveでprojectしたvertexへcanonical UVを付け、screen上のaffine UV近似誤差をsubdivisionで抑える。
recipe作成時に最大poseの誤差を見積もってgridを選び、目安16×4、上限32×8、境界で0.5物理px以内を検査する。
texture/underlay/shadowを合わせて既存§5の4096 vertices / 8192 triangles上限内。buffer再利用、page画素のCPU合成・新upload・毎frame全items走査なし。
back UVは「θ=180度の各投影cornerが新Tの同じcanonical cornerになる」対応から作る。uのmin/maxをsortして向きを失わず、
非対称の文字・角番号で前後/LTR/RTLを検査する。固定回転・trimも通常UVを消費し、印字を逆転させない。

leafは床面z=0より前にあり、基本形は自分自身と交差しないので、front/back選択とstatic surfaceとの遮蔽を解析できる。
leafのsilhouetteとdestination clipをpolygonで処理し、visibilityは投影polygonの面積から遮蔽を差し引く。
現Builderの「軸平行rectの面積」やunion bounding boxをperspectiveのproofとして転用しない。underlayとimageを二面として面積に加算しない。
単なる頂点色shadeやshadowはincoming内容の可視面積に数えない。

**陰影は必須で、あとで足す飾りではない。** 固定の柔らかい光に対する法線の明暗に、背側→自由端の弱いgradientをvertex colorで掛ける。
読める表面を残すため暗さの下限を設ける（初期案: content tint 0.72以上、紙edgeは別）。LUT/AI/色管理を再適用しない。
投影leafから床面へ落とすshadowは、受ける新S/旧Tの実面にclipした数段のalpha gradient polygon。
浮き始めは近く細く、持ち上がると柔らかく広がり、着地で縮んで消える。不透明な黒い長方形を影と呼ばない。
shadowはpageを覆い隠す主役にせず、平らな端点ではゼロにする。

異寸法の新Tが旧Tより小さい場合、backで覆えない旧T外周を最後に突然消す問題も扱う。
旧Tの座標は動かさず、閉じる最後の10%で **旧T rectのうち新Tの通常surface外に残る部分だけ**を通常背景へ連続的に退役させる。
single arrivalでも同じ規則で、incomingの通常surface外に残るoutgoing下地だけを退役させる。
新Tの印刷面・新S・全spreadをfadeさせず、時間から求める差分polygonのalphaとclipで処理する。別のsettle owner/待機stateは作らない。
通常の微小scan差では数pxの外周整合であり、大きいサイズ差もfixtureで確認する。これは異なるcanonical footprintをつなぐ明示的なpresentation処理で、
文字を隣頁へ伸ばす案や、二頁全体をslide/fadeする代替案ではない。終端/cancelは通常helperでflat提出し、通常画素と完全一致することを検査する。

### 15.4 unit shapeごとのrecipeとinstantの境界

**R5注記: CenteredSingle→Pairの方向別対応は§16.4、通常PageSlice navigationは§16.5が優先する。**

frame-localなshape入力は既存composition/formation/placementから一回作る。
`Pair(left,right)`、`SideSingle(side,real,companion=Blank|Vacant)`、`CenteredSingle(real)`を区別する。
SideSingleは強制単頁、端配置の単頁、coverの論理sideを持つ。coverが設定により中央へ置かれていても、既存unit位置/cover位相でsideが確定するなら
pageのcanonical rectはその中央配置のままSideSingleのendpointとして使う。根拠のないsingletonはCenteredSingle。
Blankは既存の明示白いsurface、Vacantはsurfaceなし。どちらにも新occurrence/address/readinessを作らない。
末尾の表紙補助は既存の`FrontCoverSupplement`を持つ実materialで、同じidxのNavigation occurrenceと混同しない。
recipe選択は、まず以下のSingleWindow対象を解決し、残りをBookLeafのS/T対応へ渡す。
SideSingle同士の一頁窓では、背はそのphysical sideのinner edge（Leftなら右辺、Rightなら左辺）を使う。
中央の一頁窓の背は§15.2の綴じ方向から取る。これは同じnormal placement入力の消費で、画像を別sideへ置き直す処理ではない。

| 旧→新（隣接、同じ本・表示設定） | R4の動き |
| --- | --- |
| Pair→Pair | §15.2の一枚のleaf。頁の寸法・位置差だけではskipしない。左右どちらの頁もportrait/landscapeでよく、固定gutter・fitを消費する |
| Single mode→Single mode（横長含む） | forwardは旧頁のdeparture、backwardは前頁のarrival。両頁のrectは独立。viewport内で見えるcanonical domainとROIが満たされればaspect差も対応 |
| CenteredSingle→CenteredSingle（同じspread設定で中央横長頁が続く等） | SingleWindowのdeparture/arrival。同じ設定内の一頁unitとして各rectを消費し、仮の左/右実slotを作らない |
| SideSingle→Pair、cover→最初のspread | 旧Sが実頁/明示Blankならその一枚がfront、新Tがback、新Sが下の頁。旧TがVacantなら通常背景のまま。中央配置coverは旧rectから開いた新back rectへ接続し、白紙相方や隠しcover textureを作らない |
| Pair→SideSingle、Pair→強制単頁+Blank | 新Sに実頁があり、新Tが明示Blankならその白紙をback、新Sを下に露出させる。旧Tは閉じるまで静止し、終端の白紙は通常のBlankと一致。新TがVacantなら`vacant_landing_surface`でinstant（紙を着地させてから突然消したり、空きslotを白紙に変えたりしない） |
| SideSingle→SideSingle / 強制単頁を含む隣接境界 | 同じslot対応で一枚のleafを組めるものはBookLeaf。旧/新とも一実頁だけの同じ側の窓、相方VacantならSingleWindowを使う。単独表示を二実頁に数えず、明示Blankのleafはtexture不要で同じclockを使う。空き側へ着地するBookLeafを無理に構成せず、上/下のinstant条件を適用 |
| 最終spread+表紙補助 | 既存screen orderとroleに従うPairとして同じBookLeaf。補助のdecode、readiness、native leaseも通常の採用素材だけ。補助を別navigation targetにしない |
| BookLeafで新unitの唯一の実頁がT側（閉じるcover、末尾の片側単頁等） | 前半に実incomingが一枚も見えず、first-visibleを待たせる。R4初版は`landing_only_target`でinstant。最初から100度以上回したposeへ飛ぶ案、Vacantへ架空の新頁を置く案は採らない。端に限定された既知の省略で、普通のPair間turnを省略しない |
| BookLeafで旧SがVacant、動かす実leaf/Blankがない | `no_turning_leaf`でinstant。旧Tの頁をSへ移してめくる、存在しない白紙を作ることは禁止。通常のcover→Pairは旧coverがSにあるのでこの理由にはならない |
| CenteredSingle→Pair（単頁表示設定のswitchではなく横長等のunit境界） | 一枚の旧頁をSingleWindowのdepartureで返し、新Pair全体を自身の配置で下に露出させる。旧画像を左右に割らず、新Pairの片頁を偽のreverseにしない。BookLeafの表裏対応と区別したrecipe名を記録 |
| Pair→CenteredSingle（中央横長頁、相方を持たない中央単頁等） | 一枚の新画像が両slotを跨ぐため、二枚の旧頁から一leafの表裏を対応できない。`centered_target_spans_spine`でinstant。新画像を二分、duplicate、勝手にsideへ縮小しない |
| 中央coverで論理sideを証明できない／その他の不整合 | 既存sideの根拠がないcoverはCenteredSingle扱い。compositionのrole/occurrence対応が不正なら具体的sub-checkを付けてinstant。frame geometryだけからcoverを推測しない |

unitの一頁/二頁への**通常の隣接遷移**は、cache内の同じunit列/tokenに属する別unit間のshape変更であり、上表でrecipeを選べる。
寸法の後着によるunit列の再構成、shift/設定/本構成adoption、slice変更などの**列/token自体の変更**は従来どおりcancel/instant。
これを混同して全Single↔Pairを一括除外する旧§3.3の条件は撤回する。Gのbuilder/storeは改変せず、将来の明示Pair/Blank/Vacant契約を消費する。

通常fitの連続Pair comicは、scan寸法差を含め原則対応。instantは端のlanding-only、中央横長への遷移、存在しないleafなど構造上の境界へ絞る。
cover/landscape/forced-singleが多い本では省略率を別集計し、稀だと推測だけで済ませない。これらの省略も利用者評価の対象として開示する。

### 15.5 final readinessと実際の可視提示を分ける（R3の矛盾を解消する提案）

**R5注記: 以下の部分提示によるsequence完了案はレビューP1で棄却。unit完了と取消順序は§16.2で置換する。**

**final-onlyは一切変更しない。** 受理時に全incomingの現在key/generationに一致するcomplete finalと必要annotation/comic合成がresidentである条件、
all-or-none、通常source選択、後着から開始しない条件は§4/§14.2のまま。
raw prefetch/CPU AI prefetchをfinal-readyと数えない。今回は`not_final_ready_at_accept`の条件・cache保持・producerを変更しない。

一方、R3の「最初のframeで新S/T両頁が5%以上/短辺8pt」「50%で旧frontと各incomingが20%以上」は、
opaque leafを背から回すと満たせない。新Tはθ>90度まで紙の裏に隠れ、θ=90度ではfront/backの面積がほぼゼロになる。
このgateを維持したまま本のように見せるために、新TをSの帯へ先に貼ったり、同じ頁を平面へ重複表示したりしてはならない。
**§3.2・§4(3〜4)・§4.1・§9の「全incoming初回可視/中点20%」はR4で置換する提案**。
素材のready条件を緩める話ではなく、紙に隠れている素材を不可視として正しく扱う表示契約の改訂である。

first frameには「実incomingの少なくとも一つ」の意味ある露出を要求する（仮基準: そのcanonical可視面積の5%以上、connected短辺8pt以上）。
BookLeaf/Single forwardは初期liftの仮角度30度を第一候補とし、通常の終端180度（single窓は90度まで）へ補間する。
F≥8×最大寸法なら同寸法の通常頁で30度の自由端は幅の約92.4%以内となり、perspectiveの拡大を含めても下の頁を露出できる。
不等寸法ではrecipe作成時だけ30/45/60度を順に検査し、閾値を満たす最小角を固定する。60度を越える初期poseで旧frontを細帯にすることはしない。
single backwardはincomingが初回から見える仮角度80度から0度へ戻す。これらの角度はdurationと独立した品質定数で、追加preferenceにはしない。
角度は受理時刻からの`elapsed/D`をclampしてsmoothstep補間し、paint時に時計をリセットしない。
純粋なθ=0/180のendpointは通常frameとの照合に使うが、初回ready passに「まだ新頁が何も見えない準備frame」は挟まない。
上の有限候補で有効なincoming露出がない場合は具体的`initial_target_visibility`で同pass flat。
初期角の選択と通常のθ補間もrecipeに含め、毎frame姿勢を探して待つ、geometryのためにphaseを先送りする、後から開始する処理はない。

atomic submission/visible receiptは以下へ改訂する。

1. 全incomingを通常どおり各一回prepareし、全resource/ROI・recipe・pin予算・初期の実incoming露出を先に検査する。
   同passの一ownerが`Flat`か`BookLeaf/SingleWindow`を排他的に一度提出する。途中で片頁だけ提出しない。
2. receiptは、**unitの全final素材とidentityが採用された証拠**と、**各occurrenceの投影/遮蔽後可視area（ゼロも含む）**を別に保持する。
   source存在を面積に変換しない。新Tのarea=0は正しい結果であり、source照合失敗とは区別する。
3. navigationには、同じaccepted target unitの全素材がfinal-readyで、一つ以上の実incomingが上の閾値で見えた採用passを
   unit presentationとして通知する。sequenceをleafの裏やfinishまで保持しない。これがR4の**ON時unit提示の意味の改訂**であり、
   `SubmittedUnitReceipt::matches_sources`の現「全page area>0」と`observe_fs_navigation_sequence_presented`へのall-pages trace依存をそのまま流用できない。
   typed unit submission証拠をobserverへ渡し、まだ見えない新Tをper-page提示済みと偽らない。
4. per-occurrenceの`fs/page_turn_ready`/first-visibleは実際に見えたpageだけへ通知し、新Tはbackが露出した採用passで初めて通知する。
   unitのidentity/source照合と可視集合を分け、既存ready signatureのunit丸ごとdedupで新Tの後の初出を捨てない。
   観測済み集合は同じaccepted-operationのsubmission ledgerで持ち、第二のnavigation queue/ready ownerや待機boolを作らない。
5. 破棄egui passのreceiptは採用せず、再passは同じoperation/source identityでdedupする。通常OFFの通知は従来の全unit平面の即時経路のまま。
   新入力で取消したpassは通常canonical targetを描き、既存受理/drop/orderを変えない。取消を用いて未提示targetを飛ばすことも禁止。

この改訂では`input→全素材ready`、`ready→unit first target visible`、`ready→各page first visible`、`ready→canonical fully visible`を別計測する。
新Tの初出が回転の後半になることはreal book modelの演出費用として記録し、初回から両頁visibleと報告しない。
採択duration内の全面表示は維持する。held/repeat/seekは次の通常paintへ即取消で、finishまで操作を止めない。
既存page-turn perf解析のI1〜I5はOFFで維持し、ONではこのunit/per-page区別を明示した検査にする。ログの意味だけを黙って変えない。
**表示通知・入力との接点が変わるため、R4実装前に設計担当と独立reviewerがこの契約を確認する。未確認のまま実装へ進まない。**

### 15.6 維持するowner・ROI・VRAM・OFFの契約

**R5注記: context所有は維持し、取消の完了barrierを同じtyped owner内へ加える範囲は§16.2。**

`PresentationState::{Inactive,Flat,Offered,Turning}`、context/window/viewport binding、stable operation identityを再利用する。
immutable recipeは`Turning`に一つだけ入り、front/back/under/restは既存unit素材への面参照。
BookLeaf/SingleWindow departure/arrivalというrecipeの選択を時間stateや別boolで重複表現しない。
受理時の時計/deadline、expiry、interruption、F12 fork/park/mount/deposit/retire、shared-input-seqを取消identityにしない規則は§4/§6のまま。
同じincoming snapshotが演出中に変わったら取消す安定性検査は維持する。
綴じ方向もrecipe/signatureへ同じcontextから捕捉し、方向変更のadoptionで当該effectを終了する。共有/兄弟contextの方向から再導出しない。
**開始前のold/new頁同士が異寸法であること**と、**開始後の同じincomingがviewport/処理/source変更で変わること**を区別する。
前者はrecipeの入力、後者は取消factであり、両方を「placement_changed」で扱わない。

geometryのclipは三つを分ける: (a)正本canonical page domain、(b)prepared resourceが実際に覆うUV/ROI、(c)描画先のviewport/HUDを除くclip。
浮くleafを参照pageの旧slot矩形でclipして隣側へ渡れなくしない。destination clipで投影を切り、UVは各materialのavailable domain内だけ。
必要な全poseのUV coverageを開始前に保守的に証明する。通常fitの全頁素材なら成立し、指定scale/pan等でpartial ROIをめくると
未prepared画素を露出する場合は`leaf_roi_not_covered`で同pass instant。全pose保証を数枚のsnapshot検査だけで代用しない。
OFF/通常描画のROI policy、prepare/resample/upload回数を変更せず、旧素材再prepare・effect目的の拡大ROI・readback・offscreen captureは禁止。

outgoing retentionは§5の実paintしたprepared source/output/native leaseのまま、全使用faceとaccepted/in-flight batchへ寿命を渡す。
新Tをbackと終端で使ってもtextureは一つ、Blank/paper/shadowは新textureなし。
dedup VRAM会計、mounted/at-rest/parked/batch、effect-only retained bytes、global旧素材pin予算`min(128MiB,fullscreen high-waterの10%)`を維持する。
予算不足時は通常target優先でskip/cancelし、cache縮小、target decode抑制、admission待ちやretryを足さない。
material参照追加による会計漏れと、同じsource/outputをfront/back/平面で二重計上しないことを回帰検査する。

default OFF、「ページめくりアニメーション」のPreferences/HUD/既存KeyAction、単一Settings owner、即時adoption・既存保存経路は維持。
時間100〜600ms・既定200msは引き続き**評価用の仮設定**。Q2は利用者の実際の動きの評価後に決める。
HUDの同期`Settings::save()`は設計担当ClaudeCodeが既存overflow経路に合わせた判断（§13、`e3a328ff6`）で、利用者決定と再記録しない。
R4のpaint/turnに同期保存・I/O・decode・GPU upload・blocking waitを加えない。
slideshowは同じDを使い、intervalまたは次deadline残時間がD未満なら省略、期限延長なし。Remoteへeffect/state/leaseを送らない。
OFFでは新geometry/recipe/receipt診断/時計/pinの作成をしないfast pathを維持する。

F12は同じprojectionとcontext-owned presentationで実行する。viewportの捕捉/生成/focus条件や新しいgraceを追加しない。
具体的receipt/context ownerへ触れる実装時にdetached-rework-plan §11へR4の範囲を追記し、§2に従う構造確認を受ける。
本設計作業では同planに未実施の実装/確認を追記しない。

### 15.7 perf・品質・回帰gate

**R5注記: 部分提示の完了通知、ON時の入力順序、分母・追加fixtureは§16.2/§16.6で置換・補足する。**

既存`opportunity/offered/start/first_visible/frame/finish/fully_visible/skip/cancel`とbinding/operation/source/native相関を維持する。
`recipe`にBookLeaf/SingleDeparture/SingleArrival、S/T、shape、θ、背、face（front/back/edge）を付ける。
visibilityは各occurrenceのarea/fraction/connected spanと、hidden理由（back-facing/occluded/outside-clip）を付け、0を失敗扱いしない。
初回unit-visibleと各pageの初出・fully-visibleを別に集計する。degreeにかかわらず毎frameを新page提示に数えない。

skip/cancelはgeneric reasonと`sub_check`を維持し、少なくとも`landing_only_target`、`vacant_landing_surface`、`no_turning_leaf`、
`centered_target_spans_spine`、`shape_role_mismatch`、`invalid_page_rect`、`leaf_roi_not_covered`、
`initial_target_visibility`、`destination_clip_changed`、`destination_pixel_scale`、`projection_budget`を区別する。
old/newのsource寸法・canonical rect・layout/slot/背・clip/viewport・ppp・UV/ROI、選んだS/T、material種別をperf有効時だけ添える。
演出中のsource/geometry変更には変わった対象と項目を付け、ordinary scan差を示す旧`page_placement_changed`をadmission理由から除く。
mesh/visibility CPU、vertices/triangles、draw数、pin/resident/effect-only bytesとGPU追加時間を報告する。

| 検査 | R4で満たすもの |
| --- | --- |
| real-scan regression | §15.1の実rect・1153×1671対1151×1670・ppp=1.5を読み取り結果からfixture化。旧1px負例を「異寸法対応」の正例に変更し、2px/大きい差をthresholdで通すテストにしない。16:9 letterbox singleとspread、非zero origin、100/125/150/200% DPI、LTR/RTL前後を含む |
| slot/material | A/B/C/Dの非対称文字と角番号で4方向。RTL forwardでは旧左だけが持ち上がり、旧右は静止し、新左は下、新右はback。θ<90で新右visible=0、θ>90で新右初出、終端の文字鏡像なし。単頁backwardのincoming arrivalも専用確認 |
| shape matrix | Pair/SideSingle/CenteredSingle、forced after-cover/last/Boundary、cover中央/片側、明示BlankとVacant、FrontCoverSupplementの各正例と明示instant理由。隣接shape変更と列/token変更を別検査。全実idx一意、Blankにreadiness/selectionなし |
| motion snapshots | fake clockの0/15/30/45/50/55/70/85/100%と90度近傍。静止goldenだけでなく同じfixtureの連続poseを順に目視。広い画像面が回り、自由端のperspective、spine固定、柔らかいshadow、leaf gradientが分かる。黒背景のopaque imageを必須にし、checker/white fixtureだけで裏面素材の誤りを隠さない |
| 見える面の品質 | 90度付近の一瞬の薄いedgeは自然な頁の挙動として許容。前半には旧front+revealed新S、後半には新back+新Sが識別できる広い面。初期案としてθ=45/135度で、それぞれ可視になる実面がcanonicalの20%以上（clipされた小窓は別profile）。中点で全face20%を強制しない。shadowをpage areaに数えない |
| canonical/integration | 開始前・終端/cancel画素を通常helperと比較。不等寸法で旧Tの外周残像/終端jumpなし。UV/ROI、opaque/transparent alpha、黒/白/checker、指定％/px、small F12、trim/固定rotation。geometry不正時は全unit同pass flat、各page prepare一回、submission/採用通知一回 |
| observer/input/context | 各source final-readyだが新T不可視のreceiptを正しく区別。first-visible unit通知がfinish待ちにならず、per-page traceは隠れた新Tを含まない。pre-draw cancel、sameframe down/up/複数edge/replay、repeat/wheel/gamepad/seek/no-op、F12 Aの操作がBを変えない、OFFとtarget数/順序/drop/work admission一致 |
| lifetime/VRAM/perf | eviction/cancel/close後batchまでlease生存、最後のbatch後pin解放、parked accounting、二窓独立。OFF追加仕事ゼロ、ON追加decode/upload/resampleゼロ、idle heartbeatゼロ。CPU p95追加0.5ms/context、GPU追加2ms/4Kの仮gateは実測で検証 |

§9.1のwarm 3run×30機会、開始率90%以上/完走率95%以上の仮gate、機会外理由とskip分母、速度/実用性/品質の別判定は維持する。
real-scan spreadを必須warm profileに加え、cover/forced/landscapeの対応率も別報告。8/8拒否や帯状の動きを「性能合格」で閉じない。
R4の回転角・visible面のproofを解析器へ反映してから測定し、R3ログとの比較では通知の意味の違いを添える。
実機確認ではこのPDFと通常画像のsingle/spread、LTR/RTL前後、F12、仮200/440/600msを利用者が判断する。
判断点は「一枚の紙を背から返している」「新しいbackが正しい側へ着地」「旧相方が動かない」「黒い帯にならない」「通常送りの速さが保たれる」。
最終durationとR4の品質は未採択。headless snapshotだけで実GPU/OS presentの動きや利用者受入れを主張しない。

### 15.8 組合せの削減・不採用案・実装見積り

**R5注記: 一件だけ保持する取消後navigationは待機queueを拒否した本節の例外として§16.2で定義。現見積りは§16.7。**

採用する簡素化は「final-readyの隣接turnのみ」「一枚のleaf」「一つのimmutable recipe」「新操作で通常表示へ終了」。
形状をframe-localに解決し、開始後にunit構成を追い掛けない。front/back/under/restを一つのrecipeにまとめ、両頁別のclock/phase/ownerを作らない。
single arrival/departureはrecipe内の変換で、navigation/history/readiness ownerは既存一つ。設定変更にreader reloadや専用live rebuildを足さず、既存adoptionでcancelする。
front/backを同一の固定寸法へ矯正する案も退ける。実scanのcanonical boxを変えるか、未知の紙余白/透明域を新設する必要があるため、
狭く見える角度でvirtual leafを補間し、各面の通常端点を守る案を選ぶ。実物の紙寸法を再現することと、既存表示のgeometryを維持することを混同しない。

不採用: geometry tolerance増量、全頁同じrectへ矯正、spread全体の一枚化、slide/fadeへの置換、黒背景をreverse paperに流用、
新Tの初期帯/二重描画、mirror文字、架空Blank/裏頁、scan差を理由に全件skip、modal入力ロック、finish待ちnavigation、
遅延開始/queue/retry/resume、独自prefetch/cache、新しい保存writer、物理simulation/depth専用engineの先行導入。
modal化は既存高速閲覧を遅くするため使えない。source/geometry変更は通常表示へ戻ることで組合せを削る。
まれなGPU/保存失敗は既存のlog/通知と通常表示への終了を継承し、多段回復を新設しない。

これは旧projectionの数行修正ではなく、**projection/shape対応/visibility契約の置換**。準備・取消・lease/会計・設定は再利用する。
R4は設計revision名で、detached R4と別。実装chunkは以下の順を提案する。

| chunk | 完成境界・主なfile owner | 一人の実装工数の概算 |
| --- | --- | --- |
| R4-a | 純粋BookLeaf/SingleWindow projection、spine/不等寸法/UV/clip/visibility、soft shadow/gradient、real-scan/4方向/黒背景golden。`src/page_turn_animation.rs`とそのsnapshot testsが中心 | 3〜5作業日 |
| R4-b | 既存normal compositionからshape/slot/背をprepared unitへ接続、recipe選択、atomic unitとper-page visibility receipt、accepted-pass observer/trace/解析器、F12/context回帰。`src/ui_fullscreen/page_turn.rs`、`src/ui_fullscreen.rs`、`scripts/analyze_perf.py`。新所有fieldが必要なら既存bundle/ledger内で確認 | 3〜5作業日 |
| R4-c | shape境界/partial ROI/不等外周settle/二窓leaseの回帰、既存入力/設定/VRAMの退行検証、required gate、確認build、利用者の動き評価、docs/spec/manual/detached §11の実装追補 | 2〜4作業日 + 実機待ち |

製品Rust置換/追加は約800〜1,400行、tests/fixtures約600〜1,000行、解析器/そのtests約150〜300行、goldenは約20〜35枚を目安とする。
現在のstrip/矩形可視性コードを削除して置換するため、これはnet増分の保証ではない。Settings/DB migration、新KeyAction、Remote変更は不要。
合計 **8〜14作業日（約2〜3週）+ 独立設計/完了レビューと修正・実機枠**。
egui subdivisionでperspective UV誤差/CPU予算を満たせない場合だけ、同native texture/leaseを借りるshader bridgeを別判断する（追加3〜7日以上）。
その場合もCPU copy/readbackやOFF経路の遅延で帳尻を合わせず、範囲と費用を設計担当へ返す。

実装前の確認点は、(1) §15.5のunit提示/per-page可視性改訂、(2) §15.4の端/中央横長のinstant範囲、
(3) 異canonical寸法のleaf補間と最後の外周整合をこの方式で扱うこと。
利用者の本のmotion要求をすでに満たしたと記録せず、設計担当の確認と独立レビュー後に実装する。
今回は本planと`target/A-1341-r4-design-msg.txt`だけを書き、製品code/.git/dev-runtime/normal profileを変更しない。

## 16. R5 — 取消後の提示順序とbook geometryの契約（2026-10-11）

**状態: 設計改訂のみ、未実装・独立再レビュー待ち。** 対象は`next-viewer` / R4 `afc474d8d`。
設計担当から伝達された独立レビューの判定はREVISE。P1〜P3（五件）を以下で解消する案とする。
「navigationを続ける限り、受理済みtargetの各実頁を少なくとも一度提示する」保証を撤回しない。
利用者がreaderを閉じる・本を切り替える場合まで旧本を強制提示する保証ではない。
accepted receiptはeguiの採用出力の証拠であり、OS scanoutを観測した証拠とは区別する。

### 16.1 現行の置換範囲・固定条件

| レビュー指摘 | R5で優先する契約 | 置換するR4記述 |
| --- | --- | --- |
| P1: 隠れた新Tを飛ばせる | §16.2: 部分curlでsequenceを解放せず、取消targetのcanonical flatを一採用frame提出してから次navigation | §15.5(3/5)、§15.7の部分unit通知・ON/OFF同一drop/orderの主張 |
| P2: Pairの背移動と静止面 | §16.3: 一つの移動する背、一定幅gutter、半平面で排他的にclipする静止面、内容欠損の品質admission | §15.3の「通常Pairの背は同じ」「staticの通常rectだけで境界も一致する」という前提 |
| P2: 中央単頁から逆送り | §16.4: forward departure、backwardは理由付きinstant | §15.4の方向を問わないCenteredSingle→Pair departure |
| P2: split navigation | §16.5: 通常表示stepと構成失効を分離、halfを含む遷移は明示instant、Full→Fullは候補 | §15.4のslice変更一括失効、およびsplit経路もofferされるという暗黙の前提 |
| P3: 台帳/判断者 | §16.8とbacklog/detached記録を同期 | A0受入れ待ち・次版予定・HUD同期保存を利用者決定とする記録 |

§15.1の黒帯の根因（背景をsingle reverseに流用、背回転/perspectiveを持たないstrip）と、
§15.2のA/B/C/D四方向対応は維持する。default OFF、PC/F12のみ、仮100〜600ms/既定200ms、
最終時間は実際のmotionを見た利用者が決定、slideshowは同じD/deadline省略規則、final-onlyは変更しない。
一回prepare→排他的flat/leaf選択→同pass提出→採用receipt、追加decode/upload/resample禁止、
stable operation identity/destination binding、context所有、通常mount/deposit非取消、fork初期化、
batch leaseとVRAM dedup/旧素材予算は維持する。Remoteに新state/leaseを送らない。

### 16.2 P1 — flatを一frame採用してから次navigationを解放する

#### 確認した違反と完了条件

現`page_turn_pre_draw`は前回receiptをflushした後にinput factで演出を取消し、rootは描画前に
`handle_fs_navigation`を呼ぶ。`PageTurnPresentation::cancel`はTurningを`Flat(incoming)`へ変えるだけで、
そのunitのflat提出を保証しない。次targetがresidentなら旧targetの隠れた新Tを描かず次へ進める。
viewportの描画後navigation、gamepadの共通handler経路も同じadmission境界で扱う必要がある。

**curl中の一部可視receiptはper-page first-visibleの証拠だけとし、navigation sequenceを完了させない。**
自然終了でも取消でも、全target occurrence/role/source/final generationが一致し、通常helperが全unitを
canonical flatとして提出した採用frameだけを既存`observe_fs_navigation_sequence_presented`の完了証拠にする。
未提示新Tをarea=0のまま「全unit提示済み」と通知しない。flatが証明するのは通常clip内の可視domainであり、
通常ROI外まで強制表示する新保証は作らない。部分curlのfirst-visible/per-page ledgerはこれとは独立した観測値。

#### ownerと一件の継続操作

取消を`Turning→Flat`だけで終えず、同じcontextの`PageTurnPresentation`に排他的な
`Completing`状態を持たせる。内部phaseは`NeedFlat`か`FlatAccepted { frame_id, receipt }`のenum。
対象unit、元turn identity/binding、continuation（`None`または**一件のtyped ResolvedNavigation**）を
この状態だけが所有する。別のpending bool/queue/ready ownerをAppやviewportへ散らさない。
ResolvedNavigationは既存のsemantic優先順位を解決済みの操作と、元のoperation ID/入力時刻、配送先context/window/viewport、
取消targetから解決したnavigation先を持つ。key/gamepadのraw eventを保存して再読込・再押下扱いしない。
source/unit列bindingが変われば継続操作を破棄し、兄弟contextや新しい本へ付け替えない。
frame IDはowning viewportの採用FullOutput frameとpassを区別する既存ledgerの識別子へ結び付ける。
同じframeのpass番号が増えただけでは「次frame」としない。output hookはreceiptをpublishし、
presentation状態の変更とcontinuation消費はowning contextのpre-drawだけで行う。

| 順序 | owner / 行うこと | 禁止すること |
| --- | --- | --- |
| 1. 入力受理 | root/viewport/gamepadの既存semantic解決後、共通navigation入口で、idx/slice/history/load/次sequenceの変更**前**にbarrierを判定。Turningに対する最初のnavigationをCompletingへ一件移す。raw factによるpre-draw取消もCompleting/NeedFlatへ合流 | 既存`blocks_new_target`で最初の割込みを捨てる、またはtarget変更後にbarrierを置く |
| 2. 取消target提出 | current idx/stepは取消対象のincomingに留める。presentation ownerが時計/leafを終了し、normal prepareを各unit一回使って全targetをflat提出。次targetへのload/work admissionはまだ行わない | 隠れた新Tをsource存在だけで提示済みにする、leafのfinishまで待つ、旧頁を再prepareする |
| 3. 採用 | 既存submission output hookは、破棄されなかったflat receiptをそのcontext ledgerへ記録するだけ。次のowning pre-drawがmatching source/role、canonical flat、binding、frame IDを照合してFlatAcceptedへ進む | hookからpresentationをmutate/navigationをdispatchする、破棄passやpartial curlで完了する |
| 4. 次のframe | **採用frame Nとは異なるframe N+1以降**のowning context pre-drawがreceiptをflushし、旧targetの全頁提示/既存sequence完了を通知。その後continuationを一度取り出し、同じ共通handlerへ渡す | 同じegui再pass中に次targetへ進める、通知より先に次sequenceを開始する |
| 5. 継続 | 元operation IDを保持して通常navigationを一回実行。この継続は新しい演出を開始せずinstant。次frameの新入力より優先し、既存SinglePerFrame等の一操作予算を消費する | synthetic input ID、二重history、二重offer、held/repeatを遅延queueへ積む |

Completing中の追加navigationは既存の単一操作/drop方針に従い、一件を増やさない。
既存LatestSeek等のownerはそのままにし、同じ操作をcontinuationとseek pendingの二箇所へ所有させない。
seekを含む対象移動も、旧target flat完了前にcommitしない共通入口へ接続する。
最初の割込みを一件保存する規則はanimation ONの取消に限定したR5の明示変更であり、OFFの入力規則へ拡張しない。
まだturnを開始していないloading/Offeredは既存の未提示targetのadmission規則を使い、新たな「readyになるまでの操作queue」は作らない。

viewport入力が描画後に届く場合も次navigationは未実行のまま保持する。
既存egui discard/replayを使用できるpassなら、再passをNeedFlatとして通常flatへ置換する。
再pass上限等で当該frameが既にpartial出力なら、それをflatと数えず次frameでflat、その翌frameで継続する。
したがって費用は**通常一採用flat frame、遅い入力では次frameへの繰越を含む一〜二frameのdispatch遅延**。
duration終了待ちやUI blocking waitはなく、例えば600ms設定でも残時間を待たない。この費用をOFFと同じ遅延だとは報告しない。
これは今回の依頼で優先された「取消targetを一flat frame先に提示する」規則の具体化である。

zoom/設定OFF/geometry変更などnavigation以外の取消も同じtargetをNeedFlatで通常表示へ戻す。
そのflat採用前にnavigationが来たら同じ一件のbarrierへ合流する。設定OFFで既に保存したcontinuationを消さない。
ON由来の完了状態だけをnormal flat receiptで片付け、OFFから新たな完了state/時計/recipe/pinを作らない。
close/別本open/context retireは既存の明示終了を優先し、continuationとeffectを当該ownerだけで終了する。
native batch leaseは既存採用出力の寿命まで保つ。通常mount/deposit/parkはbind先を勝手に替えず、
未flush receipt/Completingをcontext bundleとともに移す。pause中に自動dispatchせず、同じcontextの次の有効frameで処理する。
epoch/構成失効では一致しないflat receiptを採用しない。通常の再描画/最終ready経路へ戻し、retry workerや復旧waitを足さない。

必須回帰はθ=30/60/89度で新Tが一度も見えていない状況からのroot key、F12 viewport key、gamepad edge/repeat、
wheel/seek、同frame down/up、描画後の入力、discard/replay/上限、双方resident、設定OFFへの切替。
どの経路も`target1 partial → target1 adopted flat(N) → target1 observer complete → target2 navigation(N+1以上)`を検査する。
target1の新S/T双方のnormal receipt、operation IDの不変、一度だけのwork admission、兄弟context不変を検査し、
target1 flatなしでtarget2が提出される反例を失敗にする。自然終了にもflat完了frameを必須とする。

### 16.3 P2 — 異幅Pairの背と静止面

#### 一つの背、排他的なslot、静止した画素

`layout_spread_page_rects`の通常Pairは中央X、左幅L、右幅Rから概ね`H=X+(L-R)/2`を得る。
Pair同士でもHは変わる。trim後paint edgeから再導出せず、normal layoutが解決した旧/新のHとgutter幅gを捕捉する。
**固定gutterは幅gが一定という意味に限定し、画面上のx位置はHとともに動く。** 同じ設定内のgは同じ値であり、
g自体の設定変更は通常cancel。一本のleafとそのslot境界は同じH(t)を使用する。
H(t)はleaf寸法と同じ60〜120度区間でsmoothstep移行し、端点は旧/新Hに一致する。
旧/new canonical rect、UV、各頁のscaleは一切変更しない。
このBookLeafのslot契約はSideSingle/coverを含む一leaf recipeにも適用する。
Vacantには画素/面積を捏造せず、実在するstatic surfaceと既存Blankだけを検査する。
中央配置coverで背が大きく変わる場合も品質admissionを免除せず、endpoint profileの省略へ計上する。

新S under / 旧T restは各々の通常rectへ固定したまま、S半平面/T半平面へclipする。
left slotは`x<=H(t)-g/2`、right slotは`x>=H(t)+g/2`。gutter内に平面の印字を描かず、
floating leafだけが跨げる。双方のrectが重なっても、排他的なslot maskにより同じ床面pixelを二頁が奪い合わない。
shadowも実際に残る静止面へclipする。静止頁の画素をHへtranslate/stretchせず、slotに残る内容は同じ位置に留まる。
「旧相方が静止する」は**rect/印字座標が不動**という契約であり、背の移動に伴う内側clipまで不動とは主張しない。
leafのhingeと平面slotは一本の背で一致するが、scanのrect端が常にその背へ密着するとは主張しない。
leaf endpointのinner gapは各canonical rectから捕捉したd(0)に含める。

異なるrectのためslot maskで覆えない床面の細い領域は、既存canvas背景をそのまま露出する。
これはgutterの幅拡大でも架空の紙余白でもなく、**page support不足**として別計測する。
未知の頁画素を生成したり、端の一列を引き伸ばして穴を埋めたりしない。
旧Tの外側残像を最後10%で退役させる§15.3の規則は維持するが、内側のslot競合の解決には流用しない。
開始/終了/cancelは通常helperによるflatで、通常rectの印字が欠けない端点に必ず一致する。

#### 品質admissionと幅入替えの反例

大きく背が移動するPairまで同じ演出を強行すると、静止頁を半分切る・幅広い背景の穴が走る問題になる。
全trajectoryについて、leafによる自然な遮蔽とは別に、slot maskがcanonicalな新S/旧Tから切る面積と、
背側のsupport不足で余分に背景が露出する面積を解析する。
support gapは各静止面の背側paint端と現在slot境界の距離から、当該面の**自身の通常Hでのinner gap**を引いた正の部分。
各面の通常clip後の縦domainと交差させて面積にする。元からあるtrim/gutter余白・外周letterboxは欠損に数えない。
一方、normal Hからずれたため増えた内側の背景帯はshadowで隠せても欠損に数える。
分母は各対象のcanonical可視面積（通常destination clip適用後）とし、欠けは該当slotの小さい方のcanonical面積を使う。
**初期案の上限は各2%**。scene全体の面積やletterbox込みで欠損を薄めない。
この上限はpx配置一致判定ではなく、静止した内容/床面を何割損なうかというmotion品質判定であり、R5-aで連続poseを評価して再レビューする。
H(t)の単調性とrect/half-planeの交差境界から最大欠損を求め、数枚のsampleだけを全pose証明としない。
満たせなければ**`spine_static_surface_loss`でinstant**、失敗したsurface/clipかsupport gapか、最大fraction/θ/Hをperfに残す。
寸法一致guard・「H差1px以内」・aspect一致条件を戻さない。異寸法そのものは許し、可視品質で判定する。

必須fixture: X=500,g=0、旧L/R=600/300（H=650）、新L/R=300/600（H=350）。
LTR forwardでは旧左rect=[50,650]、新右rect=[350,950]がそのままだと300px重なる。
R5 maskは各poseで境界をHへ揃えるが、旧左を最終側で300/600=50%欠かせるためadmissionはinstantとなる。
mirror RTL/backwardおよび幅の逆入替えも同じ検査をする。
**連続pose fixture**はθ=0〜180度のH/slot mask/印字座標/遮蔽/欠損曲線と60/90/120度境界を含み、
diagnostic projectionで重なりが消えること・大きい欠損の理由を示す。製品で不合格poseを演出開始してからfallbackしない。
2%境界の上下、背不動だが高さ/外側幅が違う例、実scan微差も同じfixture familyに入れる。

通常scan差の正例には§15.1の実paint rect/ppp/sourceを使用し、normal layout ownerからのH/slotもfixtureへ明示する。
paint内側がほぼ共通でも、実際のHが同じだったとpaintだけから断定しない。
読み取り専用のPDF metadata確認では対象PDFは26頁、rotation反映後landscapeは0頁。
MediaBoxからtrimなし・共通高さ1000・gap0・RtlCoverで(2,1)…(24,23)をPairとした**参考モデル**では、
11 Pair間の最大`|ΔH|/最小頁幅`は0.8102%、2%超は0/11。
cover/末尾はこの11件の分母に入っておらず、SideSingle/Blank/Vacantの省略をゼロと見積もった結果ではない。
これはsource raster/実trim/viewport/quality面積を再現した実app開始率ではない。参考モデルで通常scanの少差を大量除外する必要は見えないが、
実H/全pose supportの再取得と3run測定なしに「fallbackが稀」と結論しない。強い非対称Pairの頻度は本ごとに報告する。

### 16.4 P2 — 中央単頁からPairへの方向別対応

CenteredSingleは物理sideが確定しない一頁窓であり、Pairの半頁と同一視しない。
SideSingle/cover/Blank/Vacant、A/B/C/D四方向は§15.4を維持し、次の行だけ明確化する。

| 通常の隣接unit遷移 | front / back / revealed / rest | R5の選択 |
| --- | --- | --- |
| CenteredSingle→Pair forward | front=旧中央一頁、back=single用neutral reverse、revealed=新Pairの両頁の各通常rect、rest=なし | SingleDeparture。背は綴じ方向の旧単頁spine edge、旧画像を一枚で窓外へ返し、新Pair全体を下に露出。新Pairの頁を偽のbackにしない |
| CenteredSingle→Pair backward | 一leafのincoming arrivalでは新Pair二頁を一枚化するか、片頁を着地させ残りを別の時点で出す必要がある | **instant `centered_to_pair_reverse_topology`**。方向に合わない旧中央頁departureへ置換しない |
| Pair→CenteredSingle forward/backward | 新中央一画像が両slotを跨ぎ、旧Pairから一leafのfront/back/restを対応できない | instant `centered_target_spans_spine`（R4維持） |
| CenteredSingle→CenteredSingle forward/backward | departure/arrivalのsingle素材対応 | SingleWindow（R4維持）、backwardはincoming arrival |

Pair P0→中央横長L→Pair P1のfixtureは、前進P0→L instant / L→P1 departure、
後退P1→L instant / L→P0 instantを四綴じ/操作方向で検査する。
中央横長が孤立して現れる境界では、その二遷移だけを見るとforwardは50%、backwardは100%省略となる。
中央単頁が続く区間は通常SingleWindowであり、すべてのlandscapeを一括除外する意味ではない。
形成理由・cover sideが分かるSideSingleにこのfallbackを適用しない。
孤立横長が多い漫画では省略が稀でない可能性を明記し、全unit遷移の分母とshape別率を報告する。

### 16.5 P2 — landscape splitは通常表示stepとして数える

`PageSlice::{Full,Left,Right}` / `PresentationStep { source_idx, slice }`は既存`page_split.rs`の正本。
同一source内の半頁移動は`StepMove::WithinPage`でnavigation sequenceを通らず、別sourceの`FsPageNav::Split`も
現コードではnormal landing後にsliceを変更するがpage-turn offerへ入らない。
これらをcomposition token失効と呼ばない。分割方向/設定adoptionや後着寸法によるstep列の再構成は構成失効、
既に解決した通常stepから次stepへ進むことは通常navigationである。

| slice / sourceの遷移（前後対称） | R5の対応と理由 | 明示理由 |
| --- | --- | --- |
| Half→Half、同一source | instant。左右は同じ一枚の紙の部分表示を移る操作で、二つのhalfを別leafの表裏としてめくらない。既存slice移動をそのまま実行し、新sequence/prepareを作らない | `split_same_source_step` |
| Half→Half、別source | instant。通常prepared halfから完全なleaf/revealed/back素材を保証できず、未知の半面や新full-page uploadを追加しない | `split_partial_leaf` |
| Half→Full / Full→Half、別source | instant。partial physical leafと全頁を一枚のsurfaceとして接続せず、通常stepの表示を維持する | `split_partial_leaf` |
| Full↔Half、同一source | 普通の固定split列ではこの隣接stepは生成されない。設定/寸法adoptionによる切替なら構成失効で通常表示。別経路から通常stepとして来てもcurlしない | `split_configuration_changed` / 通常stepなら`split_partial_leaf` |
| Full→Full、別source、split mode | SingleDeparture/SingleArrivalの候補。既存Splitの受理成功→normal landing→slice確定の**後**に、Targetと共通のoffer admissionへ接続する。既存sequence/source/readiness/operationを一度だけ使う | final-ready/通常geometryの既存理由。slice Fullだけを確認して独自loadを始めない |
| Half→別頁がPair / SideSingleとなる経路 | partial素材を含むためinstant。Half側をFullだったと扱わず、unitとsliceの両identityを報告 | `split_partial_leaf` |

halfのinstantは表示/送り/履歴を省くものではなく、**演出のみの対応制限**。
full textureが偶然residentでも、half用の通常placementは紙全体のspine/相方のlayout契約を供給しない。
全頁素材が常に欠けていると断定するのではなく、full leafのplacement/全pose coverageを通常half提出から保証する設計が未成立であるため、
R5ではhalfを切り抜いた紙として回す実装を採らない。追加素材が不要な場合を含む将来対応は別途その契約から設計する。
within-sourceの受理stepにもoperation/source/sliceのperf記録を付け、nav sequenceがないことを理由に分母から落とさない。
P1のTurning割込みに相当する場合は、次sliceを変更する前に同じCompleting barrierへ通す。
普通のHalf表示からHalfへの操作にはturnがないためbarrierを作らず、現在の即時slice移動を維持する。
half間にslot対応を捏造する案、上下/横slideへ変える案、未prepared fullを追加decode/uploadする案は不採用。
Full→Fullのoffer接続はpure presentationの不足を補う範囲とし、source producer/history/sequenceを二重に作らない。

#### 通常comicでの頻度・必須分母

N sourceのうちLがsplit landscapeならstep数M=N+L、全列を一方向に読む操作数はM−1。
同一sourceのhalf移動はL回。Fullの連続run長をrとするとFull→Full候補数は`E_FF=Σmax(r−1,0)`。
half関連の演出省略は`M−1−E_FF`、省略率はその数をM−1で割る。逆送りも同じ分母で報告する。
**すべてlandscapeなら2N−1操作の全件がinstant、開始率0%・演出省略率100%**。
Portrait→Landscape→PortraitもFull/Half/Half/Fullの3操作すべてがinstant。
したがってR5はlandscape-split comicでanimation省略が稀とは言えず、対応範囲が狭いという既知の制限をdesign acceptanceの対象にする。
この制限を「対象外なので機会なし、合格」と総合報告から隠さない。

対象実PDFのmetadataは26 portrait / 0 landscapeなので、metadata通りの未trim・未回転split判定なら25操作中half関連0/25。
実appのsplit判定は処理後寸法等にも依存するため実profileの証明ではなく、またlandscape漫画の代表にはならない。
手元でlandscape comic corpusの実測は未取得。synthetic全landscape/mixed計数をfixture化し、
実装後に利用者提供またはdisposable fixtureのsplit profileを必須測定する。データ不在は**判定不能**と報告する。
実comic別のaccepted steps / within-source / across-source / Full/Half組合せ / ready-at-accept / eligible / start / finishを
ON/OFF、LTR/RTL、forward/backward別に出し、全通常step開始率と対応候補内開始率を両方提示する。
分割主体の漫画まで本項の品質を満たすと採択される場合は、full leaf素材と同一紙のview移動を別の設計chunkで扱う必要があり、
R5の一leaf/通常prepared素材だけで対応済みとしない。

### 16.6 perfと回帰・成立性gate

§15.7の実scan/16:9 letterbox、単頁/Pair、LTR/RTL、DPI/非zero origin、UV/trim/alpha/ROI、
黒背景・四方向A/B/C/D、soft shadow/gradient、canonical endpoint、F12/batch/VRAM回帰は維持する。
次を追加・置換する。

| gate | 必須証拠 |
| --- | --- |
| flat-first input | §16.2の全入力経路。turn ID、interrupt operation ID、completion flat receipt/frame ID、observer完了frame、next navigation/work admission/frame、dispatch遅延ms/frameを相関。hidden Tのper-page初出とsequence完了を別に確認。OFFは旧結果と一致、ON割込みは一flat frameが追加されることを明記 |
| moving-spine geometry | §16.3の600/300→300/600と逆、LR/方向mirror、微小scan差、H不動の異高さ。連続θ/H、gutter幅、静止印字の不動、排他的slot、support不足/clip loss、端点通常画素。品質不合格は開始前に全unit flat、`sub_check=spine_static_surface_loss`と失敗surface/metricを検査 |
| shape/direction | §16.4のP0/L/P1を前後に辿り、reverse departureを出さない。SideSingle/coverの正常四方向・Blank/Vacantとの区別も維持 |
| split | Half→Half同/別source、Half↔Full、Full→Full、構成adoption、未知寸法→split確定、split navigationからのP1割込み。source/step/operationは一回。全landscape100%省略、mixed列の計数、Full連続runの候補数を検査 |
| 純粋投影の成立性 | R5-aで**silhouette/occlusion/visibility CPUをtessellation CPUから分離**して測る。θ/Hと面polygonからvisibilityを計算し、mesh分割密度を変えてもproofが変わらないことを確認。clip/shadow/image面積の二重計上なし |

skipは既存generic reason + `sub_check`を維持し、上の新理由と`centered_to_pair_reverse_topology`、
`split_same_source_step`、`split_partial_leaf`、`split_configuration_changed`を加える。
geometryには旧/新slot/paint rect、H/g、source/role/slice、clip/ROI/ppp、最大lossとそのsurface/θを付ける。
partial-first-visible、per-page-first-visible、canonical-flat、navigation-releaseは意味を分け、履歴ログの通知意味との差を明示する。
`silhouette_visibility_cpu_ms`、`tessellation_cpu_ms`、submission CPU、vertices/triangles、draw数、GPU追加時間、
pin/resident/effect-only bytesを報告。既存perf writerで記録し、UI threadへ同期ログI/Oを追加しない。

CPU追加p95 0.5ms/context、GPU追加2ms/4Kは**未検証の仮予算**。
R5-aでpure projectionが成立してからreceipt/observer改修へ進み、成立しない方式を後段で帳尻合わせしない。
warm 3run×30機会、開始90%/完走95%の仮gateは対応候補の評価として維持するが、
**全通常navigation数を別分母に必須追加**し、shape/背品質/splitのintentional fallbackもそこへ含める。
real scan portrait Pair、single、cover/forced/中央横長、landscape-split/mixedを別profileにする。
対応候補がゼロのsplit profileは候補内性能が判定不能、全step開始率0%という制限は確定、という両結果を出す。
普通のscanで稀という採択には、実layout/trimでの`spine_static_surface_loss`等の率が必要で、metadataモデルだけで代用しない。
実機/OS presentと「本をめくる見え方」、最終durationは利用者による今後の評価。設計文書やheadless fixtureだけで受入れ済みとしない。

### 16.7 状態削減・不採用案・更新見積り

最も単純に保証を守る規則として、部分curlからnavigationを解放する案を撤回し、unit完了を一種類のcanonical flatへ集約した。
「各頁が見えたbit集合によって早期解放する」「90度到達まで入力lock」「turn完走待ち」「取消をresumeする」案は採らない。
観測用per-page可視集合を新たなadmission ownerにせず、Completing一ownerと一件だけの継続に絞る。
これはboundedなframe境界handoffで、未ready操作や複数のturnを待つnavigation queueではない。
modal化/本全体slide/静止頁移動/架空余白/二leaf/halfを偽の紙とする案も退ける。
slice通常操作は既存stepへ戻し、half対応のために素材寿命/ROI/producer組合せを増やさない。
source/設定adoptionは既存canonical表示へ終了し、新たなlive-rebuild/第二の保存writerは不要。
まれなGPU/保存失敗は既存のlog/通知/通常表示への終了を継承し、retryや多段回復を新設しない。

| chunk | review可能な完成境界 | 一人の実装工数概算 |
| --- | --- | --- |
| R5-a | §15.2の一leaf + §16.3のmoving spine/mask/品質admission。silhouetteをtessellationから分離した純粋投影、soft shadow/gradient、real scan/幅swap/単頁/四方向の連続poseとgolden。主owner=`page_turn_animation.rs`/tests | 3〜5作業日 |
| R5-b | context-owned Completing/一件handoff、採用flat→sequence完了→次frame navigation、root/viewport/gamepad/split共通admission、shape方向recipe、receipt/per-page/解析器、F12 lifecycle。主owner=`ui_fullscreen/page_turn.rs`、`ui_fullscreen.rs`、既存ledger/`analyze_perf.py` | 4〜7作業日 |
| R5-c | 全profileの分母/率、splitの明示制限、partial ROI、state/input/context/leaseの回帰、required gates、確認build、利用者のmotion評価、spec/manual/detached §11の実装追補 | 3〜5作業日 + 実機待ち |

合計**10〜17作業日（約2〜4週）+ 独立設計/完了レビュー・修正・実機枠**。
製品Rust置換/追加約900〜1,600行、tests/fixtures約900〜1,400行、解析器/tests約200〜400行、golden約25〜45枚を目安とする。
旧strip/receipt処理の置換を含みnet増分ではない。Settings/DB migration、新KeyAction、Remote変更は不要。
observer/input改修はP1の保証に必要なstructural範囲であり、数行の取消guardとして実装しない。
egui UV近似/CPU/GPU予算が不成立ならR4同様shader bridgeを別判断し、追加費用を設計担当へ戻す。
将来halfを本のmotionに含める拡張工数はこの見積りに含めない。

### 16.8 台帳・判断の来歴・今回の範囲

2026-10-11利用者決定: **次の次の版へ延期し、v4.5.0の次版には入れない**。
A0（`532635fdd`〜`e3a328ff6`）はnormal profile実機で利用者が不合格と評価し、
PDFの微差skip・黒帯・spread一枚扱いを指摘した。本のような一leafの背回転が必要。
backlog §1.341のA0「受入れ待ち」をこの結果へ更新する。
HUD保存経路を既存overflowの同期`Settings::save()`へ合わせた判断者は**設計担当ClaudeCode**（`e3a328ff6`）。
HUDにON/OFFを追加する利用者要求と区別し、detached-rework-plan §11の誤った「利用者の追加決定」を訂正する。

本revisionでは製品code・tests/golden・設定・正常profile・dev-runtime・.gitを変更せず、build/launchを行わない。
本plan、backlog、detachedの来歴訂正、`target/A-1341-r5-design-msg.txt`だけを更新する。
R5-bはdetached §2のcontext所有境界を触るため、実装前に設計担当と独立reviewerの構造確認が必要。
その実装時に§11へCompleting/receipt/handoffの実変更を記録し、本revisionで未実施の合意/実装確認を追記しない。
R5のfallback範囲、特にsplit漫画の100%省略、品質2%仮上限とON割込みの一flat frame費用をレビューへ明示して再提出する。

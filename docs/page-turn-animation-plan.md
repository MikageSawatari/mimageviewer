# ページめくりアニメーション設計案 — backlog §1.341

2026-10-13改訂：単頁サイズ差は共通の黒い紙、local pagedの先頭・末尾配置はアニメーションON時の実効ON（§26）を優先する。両端の配置UIは実効値で無効化し、末尾の補助表紙Pairも対象にする。§25の暫定単頁spine比較は撤去する。

2026-10-11 / **R7設計ACCEPT（`fa5119c19`、Stage 1自動検証・確認用build済、実機評価待ち・§19）**。採択済みR7に従い、まず通常単頁と二実頁Pairのleaf・陰影・per-page geometryを既存A0へ接続し、利用者のmotion評価用buildを作った。§19時点ではcover/中央単頁/split等の対応を後段に残した。§24で通常の中央/片側singletonと横長への対応を追加する。R3統合後のA0は利用者の実機評価で不合格。R4 `afc474d8d`・R5 `c73f0d857`・R6 `29291d496`はREVISE、R7の独立設計レビューACCEPTは利用者から伝達された。§1〜17は履歴で、衝突箇所は§18、段階実装の範囲は§19を優先する。既定OFF・仮時間・F12所有・VRAM会計を維持。readinessは2026-10-11利用者決定の§22でresident displayableへ置換し、Pairの移動背とpin上限は§23、横長・通常singletonは§24を正本とする。実装の独立完了レビュー・実機motion/性能受入れ・最終時間は未了。2026-10-11の次の次の版への延期を、2026-10-12利用者決定で次の版を目指して継続する方針へ改訂した（採用は進捗を見て判断、§24）。

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

2026-10-11利用者決定: 背は読書方向ごとに固定する（LTRは頁左端、RTLは頁右端）。進む操作では現在頁がこの背から起きて次頁を現し、戻る操作では前の頁が同じ背から戻って現在頁の上へ着地する。操作方向で背を反対側へ替えない（§15.2・§21）。
RTLは頁の配置と回転を左右対応させるが、画像UVや印字は鏡映しない。
2026-10-12利用者決定（案A）: 単頁の縦長↔横長も動くのは一枚だけ。進むときは旧頁が
自身の背で去り、新頁は最初から最終rect全体に静止。戻るときは前頁が自身の背から降り、
現在頁を静止した下頁とする。下頁の補間・外周拡張clipは使わない（§25）。
追加提案の比較用に、案Aを既定とする暫定切り替えと移動する綴じ目を§25.3で規定する。
旧画像の実際の裏面は提供されていないため、裏は弱い紙色・陰影だけ。次の画像を「旧画像の真の裏」と偽って貼らない。
単ページ表示の横長画像は従来どおり一枚のsheet。2026-10-12利用者決定により、見開き表示の横長画像だけは中央を背にして左右の頁面としてめくる（§24）。navigationの再pairingは行わない。

**2026-10-12利用者決定（このworktreeで継続実装、次の版への採用は進捗を見て判断）**: 見開き表示中の横長画像は「見開き2頁を1枚にした画像」として扱い、画像の中央を綴じ目にして片側を折り返すleaf turnにする (単ページ表示の横長は従来どおり一枚)。利用者の実機報告: `C:/tmp/miv-spread-test/03_landscape_mid_10` (縦長 900×1300 の中に5枚目だけ横長 1600×900、表紙なし見開き [1,2][3,4][5][6,7][8,9][10]) で、めくりアニメーションが一部しか出ない (各遷移の切り分け・回転検証は§24)。Pair↔横長unit・末尾単独unitへの遷移のtemplate選択とadmissionを最初に調べる。

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

readiness追補（2026-10-11利用者決定）: final-onlyを撤回し、受理時のresident displayable素材で開始する。新しい正本は§22。過去のfinal-only記録は当時の判断の履歴として残す。

Q1は2026-10-10にC方向・旧/新が見える実際のpage-turnとして採択し、2026-10-11に再確認した。Q5も2026-10-11にPC viewer限定・F12含む・Remote不要と再確認した。両項は再質問しない。

1. **Q2: 最終時間は動きを見てから決める。** 評価用の仮スライダーは100〜600ms・既定200ms。新内容は最初のready frameから見え、全面表示は選択した時間内。新入力/held/seekで即終了する。
2. **Q3: 既定OFF採択済み。** 2026-10-11の利用者判断は「速度が売り」。従来の瞬時切替を初期値に保ち、希望する方がONにできる。ONでも読込待ち・連打には演出を付けない。
3. **Q4: slideshowにも同じ効果を適用する。** 即ready条件と§6のduration/deadline条件を使い、intervalや次ページ提示を遅くしない。個別toggleは追加しない。この旧期限方針は2026-10-14の§26.12で改訂し、めくり完了後の全面表示時間を保存間隔どおり確保する。
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
**R6注記: 新under-pageの旧footprint外は§17.3の連続clipで露出させ、最初から全rectを描かない。**

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

**R6注記: canonical flat採用の完了条件は維持。pre-drawでのcontinuation実行は撤回し、§17.2のclose解決後・ページ情報捕捉前の共通境界へ移す。**

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

**R6注記: rect/UVを動かさないslot契約は維持。新Sの外周露出には§17.3の追加clipを適用する。**

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

**R6注記: 以下のhalf全除外と100%省略案は棄却。別sourceのHalf→Half/Half↔Fullは§17.4のSingleWindow、同一sourceの左右移動だけinstantとする。**

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

**R6注記: continuationの実行位置・異高さ・split fixture/率は§17.2〜17.5で置換・追加する。**

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

**R6注記: 別sourceのhalfを一頁窓として回す案を採用。旧「halfを偽の紙とする」一括拒否とhalf対応見積り外の記述は§17.4/§17.6で置換する。**

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

## 17. R6 — frame入口・外周露出・split一頁窓（2026-10-11）

**状態: 設計改訂のみ、未実装・独立再レビュー待ち。** 対象は`next-viewer` / R5 `c73f0d857`。
設計担当から伝達された再レビューはREVISE。前回P1のcanonical flat完了条件、幅入替えの反例、中央単頁の逆送り省略、
split分類/分母、P3の台帳と判断者訂正は確認済みとの指摘を記録する。R6自体の独立確認を実施したとは扱わない。

### 17.1 現行契約と置換範囲

| 残るP2 | R6の契約 | 置換する記述 |
| --- | --- | --- |
| continuationで捕捉済みidx/stateが古くなる、closeに先行する | §17.2: owning viewportの終了入力解決→採用flat通知/continuation commit→描画・通常入力用snapshot捕捉 | §16.2のpre-draw内dispatch。flat frame Nより後に実行する条件は維持 |
| 高い新Sの上下が開始直後に全量現れる | §17.3: 旧footprint外を同じclockからの連続拡張clipで露出 | §15.3/§16.3の新Sを最初から全rectで静止描画する規則 |
| 半頁の全省略に技術的必然性がない | §17.4: 同source左右はinstant、別sourceはHalf/FullのSingleDeparture/SingleArrival | §16.5のhalf全除外・開始0%・将来別chunk扱い、§16.6の対応候補ゼロのsplit fixture |

§16.2の**取消targetを一canonical flat採用frame提示してから次navigation**という保証、Completingの単一context所有、
一件だけのcontinuation、stable operation/destination identity、破棄pass非採用は維持する。
default OFF、仮100〜600ms/既定200ms（最終時間は利用者評価後）、PC/F12/Remote除外、slideshow deadline、
final-only、通常prepare各一回・同pass排他的flat/leaf、通常mount/deposit非取消、fork初期化、batch lease/VRAM dedup、
追加decode/upload/resample禁止は変更しない。黒帯の根因とA/B/C/D四方向対応も§15.1/15.2のまま。

### 17.2 frame共通境界 — closeを先に解決してからcommit/captureする

**R7注記: commit前のembedded/viewport選択は入口の選択だけ。commit後は§18.2で描画owner/経路を再判定し、同じ経路の場合だけC/Dへ進む。変更時は旧callbackと通常footerを終了し、既存media dispatchへ渡す。**

#### コードで確認した境界違反

root `handle_fullscreen_root_key_input`は入口で`fs_idx`を捕捉し、その後に`page_turn_pre_draw`を呼び、
さらにその古いidxで`handle_fs_key_input`/`handle_fs_navigation`を実行する（`ui_fullscreen.rs:28969`以降）。
`render_fullscreen_viewport`は冒頭のidxからcomposition/load decisionを作り、`prepare_fullscreen_state`を呼んでから、
`render_fs_body`の中でpre-drawに到達する（同:23302/23453/23783）。
本のBは`handle_fs_key_input`内で`add_current_book_bookmark(fs_idx)`へ直結する（同:30728/31411）。
そのためR5の位置へcommitを追加すると、現idx・描画material・HUD・Bの書込先が別targetになる。
viewport closeは同:24005、Esc/FsCloseは同:30313/31325、HUD ×は`draw_fs_hover_bar`内の同:43624以降で解決しており、
pre-drawからcommitするとこれらのterminal優先にも先行する。

#### 一つのframe入口と順序

共通境界を仮称`begin_viewer_frame`とし、**当該contextをmountしたowning viewportのinputが読める位置**へ置く。
page-turn helper単独のpre-drawはnavigationを実行しない。receiptの採用証拠をledgerへpublishする既存output hookもdispatchしない。
以下のA〜Dは**continuationを消費し得るCompleting frame**の順序である。
それ以外は入口で通常経路へ戻り、従来のshortcut/closeの処理順を保つ。OFFへ切り替えた直後のON由来Completingだけは完了処理を残す。
OFFのB+Esc等の既存副作用順までこのfeatureで一括変更しない。
必要な構造変更は`render_fullscreen_viewport`のpage依存precomputeをviewport body内の入口後へ移すこと。
show/reuseするviewport identity/placement、embedded判定、IME/input owner、現在のモードに対する終了権限は入口前に解決できるが、
その段階のold idxを描画/通常shortcutの正本として保持しない。

| phase | ownerと処理 | その時点で禁止すること |
| --- | --- | --- |
| A. 終了入力 | 同じviewport/contextの既存input owner/IME/modal/popups/submode規則でEsc/FsClose/BS、native `close_requested`、HUD ×を解決。結果はframe-localな`ExitViewer` / `OwnedElsewhere` / `Continue`。closeは既存close/親一覧復帰経路へ一回渡し、continuationを退役 | pageを替える、page-bound B/rotation等を先に実行する、既存ownerを無視してEscを奪う |
| B. 継続commit | Continueの場合だけ前frame以前のmatching canonical flat receiptを採用通知し、旧sequenceを完了。Completingの一件を一度取り出し、元operation IDで既存navigation handlerへ渡す。frame Nでflat採用ならcommitはN+1以降、instant・一操作予算を消費 | 同frame replayで再commit、close解決前の次load/history/sequence、fresh keyより先にcloseまで消費する |
| C. 捕捉 | commit後のcurrent source idx、PresentationStep/slice、composition/unit token、spread pair/occurrence、page-dependent load decisionを捕捉し、通常prepareを一回行って`FsFrameState`、HUD page_infos/selection、描画materialを作る | A以前に作った`fs_idx`/pair/stateを再使用、古いstateへ一部fieldだけpatchする |
| D. 通常入力/描画 | root/viewportの残りのpage-bound shortcutはCのsnapshotを使う。Bはこの新sourceに一回追加。描画/receiptも同じsnapshotから作る。追加navigationはR5の既存一操作予算/dropに従う | Bと画面番号が違う、snapshot作成後にcontinuationだけ割り込む、rootとviewportで同じキーを二回処理する |

Aの終了判定用に必要なcurrent item kind/モード/ownershipの参照は短命のguard入力とする。
それをCの描画snapshotの代わりに使わず、commitによって媒体/idxが変わればCは正本から全部作る。
ExitViewerではcontinuationをcommitせず、既存close処理へ戻ってpage prepare/drawを行わない。
Escで比較overlay・click panel・編集・popupだけを閉じる場合は既存の局所処理であり、viewer closeへ格上げしない。
OwnedElsewhereのframeではcontinuationを実行せず、同じCompletingの一件を保持する。ownerが通常readerへ戻った有効frameで同じ入口を通す。
modalの背面dispatch/自動resume worker/新操作queue/周期repaintを作らない。close/別本/retire時は一件を破棄する。

#### rootとF12の接続、HUD ×の実interaction

- embeddedはroot wrapperの既存委譲を維持し、そのmain ctxのviewport body入口でA〜Dを一回実行する。
- 専用fullscreenへ配送されるroot入力は、継続可能なCompletingがあるframeでは**page-bound handlerをまだ呼ばず**、
  そのframeのroot入力をowning viewportのframe入口へ渡す。root wrapperは従来どおりgrid shortcutを抑止する。
  frame-localなinput参照/解決済みactionは同じframe内だけ使い、Bへold idxを付けず、raw keyを次frameへ再配送しない。
  owning viewportのAがroot由来の終了入力とviewport自身の終了入力を集めてからBを行い、Dで許可されたroot handlerの残りを新snapshotで処理する。
  したがってrootでB、viewportで×が同frameでも×が先で、次targetのload/ブックマークを作らない。
- F12はmountされたreaderのviewport入口で同じA〜Dを実行する。現在root→detachedを拒否するroutingは変えず、
  main/root入力を兄弟F12へ渡さない。root/viewport/gamepadのcontinuationはその配送先contextのCompletingだけから消費する。

このframe-local ingressはCompletingと別のnavigation待機ownerではなく、通常入力を**処理する場所を同frameの入口へ合わせる**ための引数。
rootの全shortcutに新しい永続queueを作らない。continuationがない/OFFの入力admission・順序・仕事量は既存どおりで、
描画precomputeの一回を移すだけとする。処理を二回呼んで古い副作用を取り消す案は不採用。
具体的には`app.rs:91666`のroot handler結果をframe-localなtyped result（handledと必要なroot input参照/配送binding）として受け、
`update_frame`のstackから同frameの`render_fullscreen_viewport`へ渡す。handledは既存の
`reconcile_fullscreen_after_main_focus`/grid抑止に渡す同じ意味を保ち、委譲を「未処理root」と誤って扱わない。
frame終了時に使われなかったingressは捨て、Appに永続pending root fieldを足さない。
F12の`with_window_viewer_context`描画にも入口を置き、ingressの配送bindingに一致しないmountへ渡さない。
このcall-chain変更とroot handlerの既存回帰はR6-bの所有範囲に含める。

HUD ×は前frame rectへの生pointer hit-testで代用しない。
既存hover/lock/suppressionとtop-barのgeometry/IDを共用し、**現在frameの実widget interactionをAで一回解決**する。
page_infosを要しないclose controlのinteractionをpage依存表示から分離し、後のHUD描画はその同じResponseを使用する。
×を二度instantiate/consumeせず、絵は従来のHUD前面layerへ描く。hover途中のclick/releaseやreplayもeguiの一つのwidgetが所有する。
panorama時の×は従来の360解除でありviewer closeに変えない。embedded mainのnative ×はapp終了要求のままで、fullscreen解除へ変換しない。
close resolverも既存owner/Enter suppress/IME/Esc局所処理を抽出・共有し、独自のraw Esc判定や第二のKeyActionを追加しない。

採用frame/入口のcommit済みidentityは既存context ledgerに置き、同frameのroot委譲・viewport再passで二重commitしない。
viewport callback外から捕捉済み`FsFrameState`を渡す旧順序を残してidxだけ再読込する修正は禁止。
draw/input snapshotのsource/step/token一致をhandler-levelで検査し、batch/native receiptはその同じsnapshotへ結び付ける。
viewport body後の`handle_fs_navigation_with_render_perf`（現`ui_fullscreen.rs:26111`）やhint/statusのfooterも対象にする。
callbackからframe-localな結果（終了、またはCのsnapshotと入力結果）を返し、footerはそのsnapshotのsource/stepを使用する。
closure内だけfresh idxへ変えて外側tailに旧`fs_idx`を残す接続は禁止。終了結果では通常navigation footerを再実行しない。
replayで更新された最終passのsnapshotを使い、旧passの入力結果を二重commitしない。
遅い入力で取消target flatを次frameへ送るR5の規則は維持するが、継続frameでもAが常に先になる。

#### 実handler回帰

rootの実wrapperとF12の実viewport bodyを通し、accepted flat(N)とcontinuationを準備してN+1に以下を入力する。
pure state helperだけのテストを本gateの代用にしない。headless/fake inputで可能な範囲を検査し、native OS入力は将来の承認済み実機枠へ残す。
描画一致の基本fixtureはtarget1/target2双方resident finalとし、stale snapshotをholdoverと誤認させない。
target2未readyの別fixtureでは通常holdoverを維持し、request snapshot/Bの書込先とholdover provenanceを別に検査する。
Bをholdover元のidxへ戻す補正や、continuationのためのready待ちは足さない。

| 継続frameの入力 | 必須結果 |
| --- | --- |
| Bのみ、または継続とBが同frame | target1完了後target2を一回commit。描画source、HUD番号、FsFrameState、selection、Bのsource keyがtarget2で一致。target1へBなし |
| Esc / FsClose / BSによる終了 | close/一覧復帰が先。continuation/target2 load/offer/history/Bなし、二重closeなし |
| native viewport × / fullscreen HUD × | 同じくclose先。現在frame widget Responseを使うHUD hit-testも実行し、root Bとviewport ×の同frame組合せも閉じるだけ |
| popup/IME/編集がEscを所有 | 現ownerの局所処理のみ。readerを閉じず、背面continuation/Bを実行しない。owner解除後の有効frameで入口から再検証 |
| replay / root+viewport / gamepad edge | 同frame二重commit/二重prepare/Bなし。元operation ID、採用flat frame、commit frame、fresh snapshot identityを相関。F12 A操作がB readerのidx/state/receiptを変えない |

### 17.3 異高さunder-page — 新外周を連続clipで開く

新Sを通常rectのまま最初から全描画すると、旧leafが小さい場合、その外周の新内容が回転より先に現れる。
slot lossがゼロでもこのjumpは起きる。R6ではrect/UV/寸法を動かさず、**旧S footprint外への露出apertureだけ**を連続拡張する。
BookLeafの新S underとSingleDepartureのincoming underに同じ規則を適用する。
SingleArrivalは既存outgoingがunderで、新incomingは回転するleaf自体なので、この「新under外周」のgateは不要。
同じclockからの旧下地外周の終端退役（§15.3）は維持する。

旧Sのcanonical surface boundsをO、新under自身のcanonical boundsをNとする。通常destination clip前の同一座標系で定義し、
実描画ではdestination clip/slot/準備済みUV域と交差する。accept時刻からの同じ`p=clamp(elapsed/D)`だけを用い、
`r(p)=smoothstep(clamp(p/0.5))`で最初の半durationに拡張を完了する。別clock/待機phase/settle boolを作らない。

- `K.min = O.min + r * (min(O.min,N.min) - O.min)`（各軸）。
- `K.max = O.max + r * (max(O.max,N.max) - O.max)`（各軸）。
- underの実surfaceは`N ∩ K(p) ∩ destination clip ∩ S slot`。leafに遮蔽された領域をvisibilityから差し引く。
  underlayとimageの双方、shadowのreceiverもこの同じsurfaceを使う。切り抜きによるUVを自身のcanonical transformから求め、再fitしない。

p=0ではK=Oなので、Nのうち旧leafのfootprint外にある部分は全く現れない。
p→0+では増える外周帯の面積もゼロへ収束し、拡張辺の速度もsmoothstepでゼロから始まる。
p≥0.5ではKがNを包含するため、その後/終端のnew underは通常rect全体になり、finishで残りを突然出さない。
newが小さい場合はNの全rectが初めからO内にあり、通常のleaf silhouetteによる露出を使う。
slot spine移動によるclip/support lossは§16.3で別検査し、この拡張clipで品質lossを隠してadmissionしない。
surfaceの透明画素は通常underlay/alphaを使用し、leafのsurface遮蔽domainとmaterialの透明性を混同して裏の頁を提示済みにしない。

このclipは新Sの全頁alpha fadeではなく、**新旧footprintの差だけを徐々に提示する幾何**。
旧S/新Sをscale/translateして高さを合わせず、余剰領域を別imageに焼いたりtextureを追加したりしない。
first-visibleの面積はgate/slot/leaf遮蔽後の実polygonから計算し、まだ閉じた外周は数えない。
初回poseのincoming露出判定（§15.5）はこのclipを含めて行い、旧footprint内でも意味ある露出がない場合は`initial_target_visibility`で同pass flat。
面積が後で増えるのを待ってから開始する規則にはしない。

§15.5の初期lift候補30/45/60度は維持するが、**p=0の新外周は初期lift角に関係なくゼロ**。
θを30度から始めることを口実に外周gateも30度相当まで飛ばさない。初回で見えるincomingは旧footprint内の露出だけ。
analyticなθ=0/p=0 endpointに加え、実際の初期lift/p=0と次clock poseの外周連続性も検査する。
cancelによるinstant canonical flatは利用者の割込み規則であり、連続motionの自然終了とは区別する。

必須fixtureは幅/背が同じ、中心Y=500で旧高さ600（y=200…800）、新高さ900（y=50…950）。
p=0は外周0、p=0.25ではr=0.5でK.y=125…875、p=0.5以後K.y=50…950となる。
θ=0とθ→0+（εとε/2、対応するp→0+）のmask/面積/pixel差がゼロへ収束することを検査し、
「0度だけflat、次poseで上下計300pxが全量出る」実装を負例にする。
fake clockのp=0/微小値/0.1/0.25/0.5/1、実初期lift→次pose、逆の高さ900→600、上下offset、横幅差、
black/white/checker、透明image、DPI/viewport origin、single/spread/LTR/RTLを含む連続pose fixtureにする。
featherを入れるなら既存上限内のごく狭いedgeに限定し、continuityのproofをalphaの見た目だけで代用しない。

### 17.4 split最小対応 — 表示halfをSingleWindowとして扱う

**再レビューの最小案を採用する。具体的な技術blockerは確認できていない。**
未知の半面が不要なSingleWindow/neutral reverseを既に設計している以上、完全な紙の素材不足を理由に
別sourceのhalfまで全除外したR5の推論は不適切だった。表示halfを一頁窓のmaterial domainとして回すことと、
同sourceの左右を別の物理leafに捏造することを区別する。

| 通常step遷移 | forward / backward | identity・通常動作 |
| --- | --- | --- |
| 同source Left↔Right | instant `split_same_source_step` | 同じ紙のview移動。既存WithinPageを実行し、新navigation sequence/load/historyを作らない |
| 別source Half→Half | SingleDeparture / SingleArrival | 各half自身の通常rect/UVを一頁窓として使用。departureは旧half front、arrivalは新half front、未知reverseはneutral paper |
| 別source Half→Full / Full→Half | 同じSingleDeparture / SingleArrival | half側は通常half、Full側は通常全頁。それぞれのrectが独立、under外周は§17.3。元source単位の履歴/保存を維持 |
| 別source Full→Full（split mode） | 同じSingleDeparture / SingleArrival | R5の対応を維持、splitと別のmotion/stateを作らない |
| 同source Full↔Half | 設定/寸法adoptionならcancel/通常表示 | 普通の固定step列では隣接しない。通常stepとして来る別経路も同sourceのview変更としてinstant。理由は構成変更なら`split_configuration_changed`、通常stepなら`split_same_source_step` |

spineはSplitLtr/SplitRtlの既存`reading_direction()`と受理された前後方向からSingleWindowの規則を使う。
source原画像の中央を物理spineと推測せず、**表示窓の内側edge**を支点にする。
neutral paperは未知reverse専用で、black/checker underlayを裏面へ使わない。印字のUVを鏡映せず、残りhalfを参照しない。
これは本の片側を見るSingleWindow表現の適用であり、原画像に本の二頁が印刷されているかを推測する処理ではない。

#### 受理・素材・receiptの接続

既存`FsPageNav::Split`のsource変更経路は、sequence受理成功→normal landing→target slice確定の後に共通offerへ接続する。
WithinPageはofferしない。sequence拒否/着地失敗時はsliceを変えず、offerもしない既存契約を維持する。
通常Final source readinessは受理時点の同じkey/generationで検査し、着地後に素材が来たから遅延開始する経路を作らない。
準備済みsource/outputがhalf/Fullの全poseに必要なROIを満たすかは通常unit prepare後に検査する。
partial ROIが覆えなければ既存`leaf_roi_not_covered`で同pass flat。演出目的でfull-page decode/upload/ROI拡張を要求しない。

direction/adjacencyは受理済みPresentationStep列とStepMoveから捕捉する。
現`adjacent_page_turn_direction`はTarget/Deltaしか認めないため、Splitをそのhelperへ渡してnullをskipする接続は不可。
受理済みSplit factも同じoffer入口で消費し、slideshow等の既存source送りはその実際のtarget slice/方向を捕捉する。
本項のためにtimerが同source halfの新navigationを生成するようには変えない。

prepared unit signatureには既にsliceがある一方、現`SubmittedUnitReceipt`と`same_submitted_pages`は
occurrence/source/native texture中心で、sliceを照合しない（`ui_fullscreen/page_turn.rs:205/477`）。
R6では既存unit/ledger内の**PresentationStep identity（source occurrence/role + PageSlice + canonical UV/geometry signature）**を
offer、immutable recipe、canonical/curl receipt、matching/dedup、Completing、observer/per-page計測へ一貫して渡す。
Left/Rightは同source/native IDでも同じ提示証拠ではない。異なるstepのreceiptをcopyしてflat採用/完了済みにしない。
navigation source idxの永続identityと一時表示sliceを分離し、sliceをDB/履歴/しおり/タグの新保存keyにしない。

old sliceとnew sliceが異なるのはrecipe入力として許す。現prepared unitのcross-unit `outgoing.slice == incoming.slice`による
除外は撤回し、**演出開始後の同じtarget slice変更**の検査は維持する。
PageSliceのUVを通常transformから一回消費し、rotation/trimとの順序を通常描画に揃える。
`uv_rect()`をtextureとtransformへ二重適用して四分の一へcropする実装は不可。
native texture/leaseは共有でき、same-source Left/Rightの観測identityが異なることをVRAM二重計上へ変換しない。

新inputがhalfのturn中に来た場合も、次source/次sliceを変更する前に§16.2/17.2のflat-firstを通す。
ターゲットhalfを一flat frame提示してから同sourceの反対halfへinstantで進むので、未提示halfを飛ばさない。
単発が続けば別sourceの操作は毎回候補となる。held/burst/renditionやreadinessで省略する条件は従来どおりで、候補率と開始率を混同しない。

#### 計数と追加費用

静止画N source、L split landscapeならM=N+L step、全通常操作はM−1=N+L−1。
same-source instantはL、別sourceの**構造上の候補はN−1**で、候補率は`(N−1)/(N+L−1)`。
すべてlandscape（N>1）では`(N−1)/(2N−1)`、大きい本で約50%（N=3なら2/5=40%、N=100なら99/199≈49.75%）。
残るN回の左右view移動を紙めくりにしない理由は同一紙だからであり、素材不足ではない。
Portrait→Landscape→Portraitは3操作中、別sourceの2操作が候補、同sourcehalf移動1操作がinstant。
すべてportraitなら25/25等の別source操作が候補。対象PDFのmetadata26 portraitの参考値は25/25だが、実app final-ready/ROI/開始率の証明ではない。
directionを反転してもこの列計数は同じ。slideshowのsource単位送りは実際に受理した操作だけを別分母で数える。

実開始数は候補数からfinal-only未ready・burst/held・ROI/visibility/予算等のskipを引いた値であり、
全landscapeで約50%「開始する」とは未測定のまま言わない。全通常step分母と候補内分母、skip sub_checkの全報告はR5から維持する。
R5の100%省略/開始0%の仕様制限は撤回し、実landscape/mixed comicのwarm率は実装後の必須測定・未取得と記録する。

費用は一頁窓projectionの再利用、Split offer/direction接続、sliceを含むreceipt/observer照合、ROI/回帰が中心。
R5見積りへの追加は**1〜2作業日**、製品Rust約100〜200行・tests/解析fixture約150〜300行の目安。
新decode/upload、texture、永続設定/KeyAction/DB migration、二葉motion、split専用clockは不要。

### 17.5 R6の回帰・計測gate

**R7注記: 画像turnへの割込み先がnative動画になるfullscreen/in-window回帰とF12拒否を§18.3で追加する。**

R5のflat-first保証・幅swap/2%仮品質admission、中央単頁reverseのinstant、F12/VRAM/ROI/endpointを維持し、
以下をR6-a/bの完成条件へ加える。CPU p95追加0.5ms/context、GPU追加2ms/4Kは未検証の仮予算のまま。

| 対象 | 必須fixture/計測 |
| --- | --- |
| common frame boundary | §17.2のroot実handler/F12 body。継続frame B/Esc/native ×/HUD ×、root B + viewport ×、IME/popup/submode、replay。terminal outcome→commit→captureのstamp順、snapshot idx/slice/token、bookmark source、load/history/sequence回数、兄弟context不変 |
| under outer continuity | 600→900高さ、逆/offset/幅差、θ=0/ε/ε÷2、actual initial lift + p=0/次clock、0.25/0.5/終端。外周面積/境界座標・pixel差の収束、平面UV不動、gate後first-visible、shadow receiver。0度専用flat分岐では通らない負例 |
| split material/identity | Half→Half別source/Half↔FullのLTR/RTL前後、corner番号/非対称文字/90度rotation/alpha/neutral back。全pose half UV内、extra prepare/decode/uploadなし。同native ID/異sliceのwrong receipt拒否、source単位の履歴/保存、flat-first後WithinPageが一回だけ進む |
| split率 | all-landscape/mixed/portrait列のforward/backward、same/across-source分母、候補N−1、実ready/start/finish/sub_check、slideshowは実source操作を別集計。N=3,L=3で5操作/2候補/3 same-source instantの計数fixture |

silhouette/visibilityはtessellationから分離し、§17.3のclipを含む実surfaceのpolygonで求める。
外周露出maskの作成CPUも別に計測し、mesh細分化と独立したproof/予算をR6-aで先に確認する。
perfの新fieldは`outer_reveal_progress`/gate後visible area、`step_before/step_after`（slice含む）、
`terminal_resolution`、`continuation_commit_frame`、`snapshot_step/token`を既存operation/context/receipt相関へ付ける。
wrong receiptは`presentation_step_mismatch`として該当source/slice/geometryを示し、ROI欠けは既存理由を使う。
実機profileの通常scan省略率・motion品質・最終durationは未実証。headless snapshotをOS presentや利用者合格の代用にしない。

### 17.6 簡素化・更新見積り・今回の記録

**R7注記: 媒体経路引継ぎの追加範囲と現見積りは§18.4。外周clip/splitの設計は維持。**

採用する簡素化は「一つのframe入口」「終了判定を既存ownerから共有」「commit後にsnapshotを一回作る」
「同じclockからの外周clip」「halfも同じSingleWindow」。old/new idxを両方正本として持つ案、
古いFsFrameStateの一部patch、旧頁Bを後から取り消す案、前frame ×rectへの推測hit-test、
終了より先のcommit、全頁fade/scale、half専用shader/producer/state、half全除外を不採用とする。
modal化/turn完走wait/多操作queueを足さず、まれなGPU/保存失敗は既存log/通知/通常表示への終了に留める。

| chunk | R6で更新する完成境界 | 概算 |
| --- | --- | --- |
| R6-a | R5-aのpure leaf/spine/visibility/tessellationに外周拡張clip・0度直後の連続pose・half UV単頁fixtureを追加 | 4〜6作業日 |
| R6-b | R5-bのCompleting/flat receiptに共通frame入口・close実interaction・root委譲・precompute移動、`app.rs` update/render call-chain、slice identityとSplit offerを接続 | 6〜9作業日 |
| R6-c | R5-cのcontext/lease/ROI/input/perf/profile gatesとR6実handler回帰・split候補率。実装時のdocs/spec/manual/detached §11追補、確認build、利用者motion評価 | 3〜5作業日 + 実機待ち |

合計**13〜20作業日（約3〜4週）+ 独立設計/完了レビュー・修正・実機枠**。
R5比で外周clip/fixture約1日、frame入口の捕捉順整理約1日、split約1〜2日を見込む（chunk間で共通作業を含む概算）。
製品Rust置換/追加約1,100〜1,900行、tests/fixtures約1,100〜1,700行、解析器/tests約200〜400行、golden約30〜50枚の目安。
shader bridgeが必要な場合の別判断/追加費用はR5同様。halfの最小対応を本見積りに含め、完全な紙/未表示halfを扱う別の大拡張は先取りしない。

利用者の2026-10-11次々回延期・A0実機不合格・book-like要求と、HUD保存経路は設計担当ClaudeCodeの判断という来歴は維持する。
backlogはR6の現状に合わせ、R5のhalf全省略を現行仕様として残さない。
今回の変更は本plan、backlogの現行要約、`target/A-1341-r6-design-msg.txt`だけ。
製品code/tests/golden/.git/dev-runtime/normal profileへの書込、build/test/launchは行わない。
共通frame入口はviewport/context所有境界に触れる構造変更として再レビューへ出す。
実装前の構造確認と実装時detached §11の範囲記録が必要であり、今回未実施の合意や実装確認を追記しない。

## 18. R7 — continuationの媒体境界と描画経路引継ぎ（2026-10-11）

**状態: 設計改訂のみ、未実装・独立再レビュー待ち。** 対象は`next-viewer` / R6 `29291d496`。
伝達された再レビューはREVISE。§17.2のclose→commit→fresh snapshot、§17.3の外周clip、
§17.4のsplit最小対応は設計上解消との確認を記録する。残るP2は画像からnative動画へのcontinuationで描画経路が変わる場合。
R7の独立確認、実GPU/OS present、motion/性能の受入れは未実施。

### 18.1 反証された前提と維持する境界

`fullscreen_embedded_still_active`は`native_video_in_window_active`だけでなく現在媒体も見て、
Image/ZipImage/PdfPageまたは音楽ビューをembedded対象とする（`ui_fullscreen.rs:28024`）。
通常Videoはnative presenterで、`render_fullscreen_viewport`の通常canvasより前の分岐を使う（同:23381）。
`show_native_video_black_backdrop`と`hide_native_video_black_backdrop_if_shown`はともに
`show_viewport_immediate(fullscreen_viewport_id(),...)`を呼ぶ（同:28186/28078）。
R6でその分岐を旧viewport callback内へ単に移すと同じIDへ再入する危険があり、fresh `FsFrameState`だけでは解決しない。

「A→Bのturn中に、通常入力で次の動画Cへ進む」操作は効果の対象媒体が静止画でも発生する。
continuationを静止画だけへ制限したり、動画Cを飛ばしたりしない。現在のnormal navigation/open/media policyを維持する。
§16.2のBを一canonical flat採用frame提示してから次navigationする保証、§17.2のclose優先、
元operation ID、一回だけのcommit、同じcontext所有、default OFF/仮duration/final-only/VRAM/leaseは変更しない。
R6のsplit/外周clip/四方向leafも変更せず、Remoteやnative動画自体にpage-turn effectを追加しない。

### 18.2 commit後の経路再判定とframe-local return

§17.2のA/BとCの間へ**描画owner/経路の再判定**を入れる。
commit前に選んだembedded/専用viewportは、現在Bの入力を受けてA/Bを行うための入口でしかなく、Cの描画ownerを固定しない。
実装は既存のmedia/session/presentation predicateを共用し、page-turn専用の第二のmedia分類/保存stateを作らない。

比較するframe-localな描画bindingはcontext/window/viewportと、egui canvas（main embedded /専用viewport）、
native video（既存fullscreen popup / in-window child）、音楽等の既存egui経路の区別。
presenterのReady/PendingやHWND生成は既存native ownerが管理し、R7のbindingへready bool/pending targetを複製しない。
requested target Cの種類だけで判定せず、通常handlerのcommit結果から**実際に採用されたcurrent session/媒体・既存媒体切替state**を読む。
F12拒否や別の既存noopでBに留まれば旧経路のまま。音声モードのVideoは既存`fs_music_view_active`を通し、
一律にnative Videoと扱わない。current itemのkindが同じでも描画window/viewport/host経路が変われば旧callbackは継続不可。

| commit後の結果 | owning callback | callback外の既存dispatcher |
| --- | --- | --- |
| 描画bindingが同じ | §17.2 Cでcurrent source/step/composition/FsFrameStateを捕捉し、Dの入力/通常prepare/drawへ進む | Cのsnapshotから通常footerを一回実行 |
| 描画bindingが変わった | **page prepare・CentralPanel/canvas・画像HUD・still shortcut・通常navigation/slideshow footerへ進まず**、frame-localな`DrawingPathChanged`結果を返して旧callbackを終了 | callbackを完全にunwindしてから、採用済みsessionを正本とする既存media-switch/update経路へ制御を渡す。continuationを再実行しない |
| close/既存終了 | R6のExitViewer結果、continuationなし | 既存close/親一覧復帰、一回だけ |

`DrawingPathChanged`はR6のframe-local callback結果のenum variantで、old/new binding、current source、
元operation IDとcommit済みの相関を添えるだけ。App/ViewerContextへpending flag/target/queueを保存せず、そのframeで消費する。
通常footerの代わりに実行できるのはcallback終了・perf/test witness・既存出力のlease寿命の処理と、
経路引継ぎのouter bookkeepingだけ。古い`embedded`/`fs_idx`で`fs_viewport_shown`/presentationを上書きしたり、
旧C/D snapshotを作ってから廃棄したりしない。存在するviewportの通常output寿命を処理することと、new mediaの所有を旧経路で主張することを区別する。

通常handlerは`land_still_page_navigation_target`→`open_fullscreen_from_fs_navigation_with_contract`を通り、
動画Cの通常open/既存fast swap可否・source/履歴・native起動を一回だけ決定する（同:21623/35898）。
R7のhandoffで`open_fullscreen`をもう一度呼ばず、追加load/二重history/別のnative presenterを作らない。
`app.rs`の通常native poll/open/presentation処理とmain native backdrop/chrome分岐（同:90433以降/91940以降）へ戻す。
main UI/grid抑止もcommit後の正本とframe-local結果を使い、古いembedded判定だけで旧canvasやgridを再描画しない。
未処理の同frame root ingress等は有効な既存media input owner/handlerへ渡し、旧静止画Bのshortcutを実行しない。
再配送native keyや次frame用の入力queueは作らず、既存のmedia loading/focus/modalによるdropはそのまま。
R6のB回帰はstill→stillのまま維持し、native動画のB等はその既存media規則で扱う。

#### 同viewport ID再入を避ける具体的なdispatch順

経路変更を受けたframeでは、旧callback内からbackdropのshow/hide helperや`render_fullscreen_viewport`を呼ばない。
outerでも同じfullscreen viewportをそのframeに再showして二つ目のbodyを走らせる接続は採らない。
動画open/source switch自体はBのcommitで開始済みで、native presenterは既存runtimeに従って進む。
outerは現在の媒体に対する既存main chrome/grid抑止等を処理し、その通常updateを終える。
backdrop show/hideは**次の通常top-level render dispatch**でcommit後のsessionから再判定する。
fullscreen動画は既存の「presenter未確定時だけ起動cover、ready時はhide」の条件をそのまま使い、
in-window動画はmainのchild presenter経路なので専用fullscreen起動coverを作らない。

これはviewport再入を避けるframe-local returnであり、Cのcommit/native起動を次frameまで延期する待機ではない。
新しいrepaint timer/待機state/finish wait/holdover resourceを作らず、進捗通知・通常repaintは既存media-switchに任せる。
新ready待ちやBの再prepare/延命はなく、Bの最後の採用outputのbatch leaseは既存の寿命で解放する。
old callbackを終えた際の画面露出・startup cover/child attach/旧viewport退役は既存媒体切替と同じ責務で検査し、
grid露出や二重canvasを隠すための新guard/遅延を足さない。実装時に既存切替ではこの順序を満たせない前提が判明した場合は、
通常媒体切替の所有境界として設計担当へ返し、R7専用のpending/waitを足して通さない。

#### F12の既存動画拒否

`should_block_detached_independent_still_navigation_to_video`はindependent still sessionからmediaへ進む操作を拒否し、
`open_fullscreen_from_fs_navigation_with_contract`は`DetachedVideoUnsupported`のnoopへ戻す（`app.rs:63734` / `ui_fullscreen.rs:35915`）。
continuationもこの同じ通常handler/policyを通し、Cのkindを見てnative側へ直接openしない。
拒否時はcurrent Bと既存F12 drawing bindingを維持し、Bの通常flatを描く。新native owner・別窓・兄弟contextの変更はない。
これは既存independent still sessionの拒否を維持する規則で、既存detached native動画の機能を新たに禁止するものではない。

### 18.3 回帰・perf・成立条件

headlessでは実handlerとframe dispatchを通して経路/呼出し順/一回性を検査し、fake native adapter等の既存境界を使用する。
純粋なroute enumの比較だけを統合回帰の代用にしない。HWND/実present/起動時露出は将来の承認済み実機枠の検査と区別する。

| fixture | 必須結果 |
| --- | --- |
| fullscreen A→B turn、θ<90で次動画Cへ割込み | Bの全実頁canonical flat採用N→observer完了→N+1以降Cを一回commit。post-commit native fullscreen routeを検出し、旧callback canvas/prepare/still HUD/通常footerなし。動画open/history/source admission一回、callback内show/hideなし、同viewport IDの二重bodyなし |
| in-window embedded A→B turn→C | 同じflat-first順序。post-commit native child route、旧embedded CentralPanel/footerなし。main grid二重描画/専用fullscreen coverなし、in-window設定/placement維持。outerのnative main chrome分岐はcurrent Cを使用 |
| 動画Cの既存起動がpending / ready | readyを待ってcommitしない。既存native進捗を使用。次の通常dispatchでfullscreen起動cover/hideの通常条件を検査。stale embedded/shown/presentation上書きなし。old outputのbatch lease後解放、effect-only pinの二重計上なし |
| F12 independent still A→B turn→動画C | 既存拒否handler/feedbackを一回通す。current B、items/reading位置、F12 drawing ownerを維持。native open/handoffなし、兄弟reader不変、B flat提示保証維持 |
| Esc/× + 動画C continuation、replay/root+viewport | R6 close優先でC commit/native起動なし。closeなしのreplayはCを二回openせず、post-commit routeはそのpassの正本から判定。古いpassのfooter/ingressを再実行しない |

媒体境界を新たなpage-turn候補へ数えず、全navigation分母には含める既存方針を維持する。
perfは同じoperation/context/flat receiptに`drawing_path_before/after`、`drawing_path_changed`、
`continuation_commit_frame`、既存media open/presentationの相関を付け、canvas snapshotとnative targetを混同しない。
追加decode/upload禁止は効果の仕事についての規則で、通常動画navigationに必要な既存video open/decodeを抑制しない。
CPU/GPU仮予算・scan省略率・実機motionの受入れ未了はR6のまま。

### 18.4 簡素化・費用・今回の範囲

採用: **commit後に経路を読み直す、変わったらcallbackから返す、既存media-switchへ戻す**。
不採用: Cをstill限定、動画をskip、旧canvasへnative動画を描く、callback内で同IDをshow、再帰render、
新pending target/ready bool/wait/入力queue、二回open、Bの追加holdover/再prepare。
R6-aのprojection/split/外周clipに変更はなく、R6-b/cへroute再判定・frame result/outer dispatchの接続と媒体境界回帰を追加する。
追加概算**0.5〜1作業日**、製品Rust約50〜120行・tests約100〜200行。
全体の目安を**14〜21作業日（約3〜5週）+ 独立設計/完了レビュー・修正・実機枠**へ更新する。
永続設定/KeyAction/DB/native worker/protocolの追加は不要。

今回の変更は本plan、backlogの現行revision/見積り要約、`target/A-1341-r7-design-msg.txt`のみ。
製品code/tests/golden/.git/dev-runtime/normal profileへ書き込まず、build/test/launchは行わない。
既存媒体切替とF12/context境界を保つ構造変更として再レビューへ提出し、実装前の確認と実装時detached §11記録を継承する。
今回未実施の独立合意・実装・実機・性能確認を記録しない。

## 19. Stage 1 — 通常頁の実装と早期motion評価（2026-10-11）

R7 `fa5119c19`は独立設計レビューACCEPT（利用者からの引継ぎ）。利用者指定により段階実装する。
この節は実装範囲と検証証拠の台帳であり、製品差分の独立レビュー合格・実機motion品質の受入れを表さない。
Stage 1の自動検証とnormal確認用build後、利用者の実機確認で演出が開始・継続しない入力接続の不具合が判明した（§20）。motion品質の受入れは未了。
次の次の版へ延期、既定OFF、PC/F12限定、仮時間100〜600ms/既定200ms、slideshow条件を維持する。readinessは§22の2026-10-11決定でresident displayableへ置換する。

### 19.1 今回の投影・素材・geometry

**2026-10-11追補: 固定静止面のloss admissionは§23の利用者option Aで置換。以下の2%省略は当時のStage 1履歴。**

通常Single→SingleはforwardのSingleDeparture、backwardのSingleArrival、通常Pair→PairはBookLeafを使う。
reading directionは現在のnormal layoutの正本から捕捉し、RTL forwardは旧左頁だけ、LTR forwardは旧右頁だけを返す。
Pairの裏面は新しい反対側頁、下頁は新しい同側頁、静止面は旧反対側頁。各素材は自身のnormal prepared rect/UV/clipを保つ。
単頁の未知の紙面にはneutral paperを使い、黒い背景を頁裏として使わない。

normal layoutが確定した背Hとgutterをprepared unitへ載せ、paintの丸め後rectから別の背を推測しない。
60〜120度のsmoothstepでleafのH/寸法を移し、全trajectoryのslot clip/support lossが2%を越すgeometryは
`spine_static_surface_loss_clip` / `spine_static_surface_loss_support`としてinstantにする。数pxのscan差を一致guardで退けない。
新下頁の外周clipは旧rectから両rectの和へ同じ時計で連続拡張し、異高さの開始直後の跳ねを避ける。
旧静止面の新canonical footprint外は最後10%で退役する。寸法入替えで大きく背が移動する反例は明示的quality fallback。

leafはrational perspectiveの背回転で、表/裏UVのcanonical corner順を保持する。淡い紙面・紙端、leafの陰影gradient、
receiver面にclipしたsoft shadowを同じmesh passへ提出する。16×4または32×8 gridを最初に選び、
全trajectoryで0.5 physical pixel以下のaffine近似誤差を保証できなければ`perspective_mesh_error`で省略する。
prepareはnormal pipelineの一回のみ、projection recipe/canonical unitsはoffer採用時に一回確定して再利用する。
追加decode/upload/素材再解決はしない。元のsource generation/ROI確認、batch lease、VRAM dedup/accountingを維持する。

### 19.2 この段階のinstant範囲と完了境界

| 範囲 | Stage 1の扱い・perf理由 |
| --- | --- |
| 通常単頁、同寸法/異寸法Pair間、LTR/RTL・forward/backward | quality admissionとfinal-readyを満たす時にleaf |
| cover境界・side single・supplement/白頁 | `stage1_cover_or_side_single` / `stage1_cover_or_single_companion` |
| spreadのCenteredSingle | `stage1_centered_single` |
| landscape split（同source/別source、Half/Fullとも） | `stage1_split_step`。このstageでは開始率0%、R7の最小対応は後続stage |
| native動画/音声target | `stage1_media_target`、通常媒体navigationを維持 |
| turn中の次navigation | 取消targetのcanonical flat採用後、`stage1_continuation_instant`で通常handlerへ一回commit |

部分leaf receiptは頁ごとの初出だけを証明し、fs ready/navigation完了の証拠に使わない。
incomingの各実頁は面積5%以上・connected short edge 8pt以上で`first_visible`を一回記録する。
全実頁のcanonical flatのaccepted receiptが後のowning frameで確認されて初めて旧sequenceを完了する。
割込みはcontext所有の`Completing { incoming, flat receipt, one resolved continuation }`へ移り、outgoing effect pinを退役する。
終了入力→continuation commit→fresh page/composition/frame snapshotの共通境界をroot/F12で通す。
媒体/描画経路が変われば旧callbackのcanvas/footerを終了し、既存media-switchへ戻す。新pending/wait/再帰renderは作らない。
F12の既存動画拒否、normal context mount/deposit/fork、設定ownerと既存保存pathは変更しない。
設定OFFはflat描画へ直行し、clock/geometry/lease inventory/入力診断を実行しない（既に開始済みturnのflat完了とその入力identityだけ保持）。
未採用のflatは同じlogical target/sourceの最新normal prepared geometryへbindingを更新し、resize/fitで変わるnative出力IDを旧poseに固定しない。
既に採用されたreceiptは保存して後のframeで通知する。source/configuration失効は通常navigation observerへ戻し、別targetのcontinuationを持ち越さない。
key/wheel/touch/pad/slideshowの継続は実producerが解決したoperation IDを渡す。touch EndはStartのcontact/bindingを継承し、
padは同期dispatch内だけのdestination/frame/ID witnessを既存egui temp ownerへ置き、return/unwindで元に戻す。
一つのtyped intentで通常頁・spread shift・slideshowの着地を区別し、shift-anchorやAutoAdvance履歴を通常handlerの規則で採用する。

### 19.3 R5-aの測定と検証

可視性は四隅の凸silhouetteとpolygon clip/subtractionで求め、tessellationのtriangle列・bounding box面積を提示証拠にしない。
同一poseの16×4/32×8 gridでreceiptが不変となる回帰を置く。perf有効時だけ時計を読む。
`recipe_prepare_cpu_ms`、`silhouette_visibility_cpu_ms`、その内数`outer_mask_cpu_ms`、
`tessellation_cpu_ms`、既存`mesh_cpu_ms`を分ける。skipには`sub_check`と旧/新のnormal geometry・H/gを載せ、
loss guardには最大損失率・面・Hも記録する。frameの`unit.mesh`にはvertices/triangles/drawsと
vertex/index payload bytesを載せる（renderer/driverの実GPU allocation量ではない）。
cached recipeの300 pose測定を単頁・equal Pair・portrait PDF/scan Pairで行う。
CPU 0.5ms/GPU 2msは仮予算のまま。headless/debug CPU測定や静止snapshotは実機GPU時間・motion品質の受入れを代用しない。

2026-10-11のnormal Cargo test profile、単独test/thread（warmup 20、各300 pose、cached recipe）の限定測定:

| fixture | silhouette/visibility p50 / p95 ms | outer mask p95 ms（内数） | tessellation p50 / p95 ms | 合計p95 ms |
| --- | --- | --- | --- | --- |
| SingleDeparture | 0.0074 / 0.0082 | 0.0007 | 0.0304 / 0.0335 | 0.0423 |
| SingleArrival | 0.0075 / 0.0082 | 0.0010 | 0.0304 / 0.0320 | 0.0400 |
| equal Pair | 0.0150 / 0.0165 | 0.0017 | 0.0355 / 0.0419 | 0.0579 |
| portrait PDF/scan Pair | 0.0140 / 0.0200 | 0.0024 | 0.0688 / 0.0816 | 0.1078 |

計算分離の成立性の証拠であり、初回recipe・normal prepare・perf JSON・renderer CPU/GPUを含む
製品frameの予算合格ではない。ログは`target/A-1341-stage1-feasibility.log`。
実報告の1153×1671→1151×1670と異寸法の相方を、非zero origin・16:9 letterbox・150% DPIへfitし、
通常centeringで移動する背と頁rectを四方向の連続poseで検査する回帰も追加した。
continuationのperfは元operation/contextにflat receipt frame、commit frame、commit前後のdrawing pathと変更有無を相関させる。

自動検証結果・新goldenの目視確認・verification buildの証拠は本stageのhandoff
`target/A-1341-stage1-msg.txt`へ記録する。実機での開始率（全navigation分母、split別集計）とmotion評価は利用者確認待ち。

## 20. Stage 1入力接続の実機不具合（2026-10-11）

利用者が`0d1b1f99f`のnormal確認用buildで演出を見られなかった。pid 82140のログは読取り時点で
`%APPDATA%\mimageviewer\logs\perf_events.1.jsonl`へrotate済みだった。利用者報告に対応する冒頭約61秒には
key trigger 21件、keyのanimation event 0件、pointerのstart 2件とその次frameの`cancel/new_operation` 2件がある。
その後の同sessionにも入力が続くため、この区間の数をsession全体の数として扱わない。元ログ・normal profileは変更しない。

### 20.1 キー候補だけが消えていた所有境界

初回turnの`begin_page_turn_viewer_frame`は、通常key handlerがpassのkeyboard ownerを選択する前に
`page_turn_pre_draw`を呼んでいた。後者が使うcached-only `keymap_owner_blocks_shortcuts`はowner不在を
fail-closedとし、immutable key factsを消した。後段の実key handlerはownerを選択して移動を受理するが、
offerの`take_candidate`と旧excluded helperは空factsで黙ってreturnする。renderer/projection/readinessの拒否ではない。

修正は`page_turn_pre_draw`自身が同じApp/pass/viewportの`keyboard_owner_for_pass`を確定してから
key admissionを行うこと。generic keymapのfail-closed契約、modal/TextInput/IME抑止、native source identityを緩めない。
root、専用fullscreen、embedded/F12の同じ観測入口へ適用する。設定OFFの早期returnより後でのみ実行する。

### 20.2 クリックを取り消したのは入力ゼロのgamepad poll

raw mouse releaseはpress deliveryのID/destinationを継承していた。取消直前のfresh ingressも無い。
違反は`dispatch_gamepad_batch`のanalog handler前に毎回作る`gamepad_navigation_dispatch_scope(Continuous)`だった。
sampleの`analog_operation=false`、actions空でもこのscopeがfactを発行し、開始後の次root pollで
presentationの`observe`が別operationとして取り消す。Offered時はbarrierがないためscopeを作らず、
最初のcurlが採用されてTurningになった次frameだけ取消になる、という実ログの順序と一致する。

既存batchが捕捉した`analog_operation`がtrueの場合だけscopeを作る。通常analog handlerは毎回呼び、
neutral/切断時の既存終了・reset処理は維持する。実analog操作（poll内に動きがあり最終levelだけneutralの場合も含む）と
別key/clickは従来どおり取消・flat-first完了を行う。時間窓、release無視、取消禁止、別bool/pendingは加えない。

### 20.3 受理済み移動の省略を一回記録する

設定ONのanimation診断では、resolved navigation handlerが渡す実operation IDをoffer/exclusionへ渡す。
空facts・複数facts・repeat/continuousで演出候補を作れなくても、受理済み移動には一回の`skip`を残す。
同batchの余剰入力はnavigationとして受理していないので、それぞれを別のskipへ数えない。
既存transactionのClaimed/Excludedがoffer・skip・replayの一回性を所有し、新しいqueue/履歴setは作らない。
`sub_check=missing_operation_fact` / `multiple_operation_edges` / `accepted_navigation`を付ける。
実ID自体が欠落した場合は診断専用`UnattributedNavigation`をExcluded状態で保持し、physical inputを捏造して演出しない。
通常jump、collection受付、stack jump、非隣接slideshow、split/境界とcontext切替のinstantも同じ診断へ接続する。
key/padの実ID取得も「既にturn中」だけでなく設定ONの初回送りから行う。gamepad Releaseはsampleとdispatchで同じく
新Pressへ変換せず、通常release handlerだけを実行する。
gamepadから通常navigationへ渡すwrapperも初回ON時の実IDを保持する。二つのpad factが同batchにある
初回button handlerでも、受理した一つのIDへskipを結び、scope退出時には通常どおりIDを外す。
final-only readiness、既定OFF、OFF時の入力診断なし・flat直行、geometry/perf/VRAM契約は維持する。

### 20.4 回帰と検証の範囲

初回の実egui/native key、ROOT/専用viewer、Single/Pair、および実canvas clickのpress→小move→releaseから
通常navigation、curl receiptのoutput採用、Started、通常canonical flat採用、Finishedまで通す。
直接offerやslideshow seedを初回開始の代用にしない。ROOT/子の異なるframe counterを同一Contextで交互に進め、
毎frameの実neutral gamepad batch配送がturnを維持することを確認する。
headlessではOS foreground gateだけをadmission seamで供給し、通常batch consumerの処理順を変えない。
mixed key/wheel・ingress欠落は即時の通常移動とskip一回を確認し、実analog factは取消になる負の回帰を置く。
既存interruption・F12・媒体・projection/golden回帰も維持する。実窓/実GPU・motion品質の受入れをheadlessで代用しない。

今回の検証結果、build、利用者向け手順は`target/A-1341-stage1-fix-msg.txt`へ記録する。
製品差分の独立完了レビューと、修正後の実機motion品質/性能受入れは未了。

## 21. Stage 1の実機評価追補（2026-10-11）

利用者は`52cd5a15f`の見開き・単頁のmotionを良いと評価した。ただし単頁の背が往復で替わって見えることと、開始が不定に見えることを報告した。背の要求は§3.1・§15.2の固定背として再確認する。単頁departureはoutgoing、arrivalはincomingを動かし、どちらもLTR左／RTL右の背を使う。異寸法の頁は自身の通常rectを保つので背の絶対座標まで異頁間で固定する意味ではない。

読取り専用で確認したnormal logのsession `95876-1791711361018`（pid 95876）では、Singleの635 curl frameは全てLTRで、forward/backwardの両方が`hinge_x=792.666687`、頁rect `[792.666687,44,1768,1298]`だった。既存投影と既存goldenも同じ背を使っていた。今回、方向依存のないsingle bindingを名前付きhelperで明示し、RTLのleaf退役後に左端を誤報告していた終端の`hinge_x`診断を修正した。これを実際の背交換の根因と扱わない。旧goldenの画素は変わらず、利用者が感じた実機の見え方はまだ再現できていない。

前後・LTR/RTLの4枚の新goldenは固定端をLEFT/RIGHT文字で示し、departure/arrivalの頁所有、影、文字の向きと終端を比較する。同寸法／異寸法を各200 poseで検査し、同じ読書側の背、角度の連続性、UV非鏡映、通常paintとの終端画素一致を保証する。追加decode/upload、通常layoutの移動、影の推測調整によって未再現の症状を隠さない。

### 21.1 `missing_operation_fact`の実際の発生経路

同sessionの18件はnative held-key Repeatの16件（batch 97,99,…,127）と、native Pressのnavigation boundary 2件（batch 67,71）だった。wheel raw ingressはこのsessionには無い。リピートは既存規則どおりinstantで、境界では頁が変わらない。ここでfactが欠落した原因はinput producerではなく、child `render_fs_body`で観測した入力をcallback外のROOT navigation handlerで受理する際の所有境界だった。

child（viewport 2615）の`observed_frame`をROOTの`cumulative_frame_nr`と裸の整数で比較し、Available/Cancelled transactionを消していた。明示的なoperation IDは渡っているのに元factが消え、skipがROOT/Pressの診断専用factで再構成された。readyなoffer経路にはこのresetが無いので28件の開始は可能だった。修正では現在destinationと一致する明示的operation IDを最初に照合し、そのresolved handoffをviewport/frame境界の後でも保持する。無関係・ID無しのambient観測は既存の失効規則を保ち、既存frame stampを`(ViewportId, frame_nr)`へ変更して別counterの偶然の一致も除く。Claimed/Excludedは同じIDの再配送を一回に抑える。

固定矢印と`FsPageNext/Prev`は、typed key consumerが実際に選んだIDをnavigationへ直接渡す。immutable edge列を再検索しないので、repeat→release→fresh pressが一batchに来た際にもconsumerのinitial-press優先と一致する。wheel、click、touch、seek、spread shift、jump、pad、slideshowの既存ID producerも同じ受理境界を共有する。mouse side buttons／browser pendingは頁のsingle-stepではなく既存のcontext移動であり、その動作は変更しない。ID無しのcontext受付は従来の診断専用skipを使い、物理入力を捏造しない。新KeyAction、queue、pending、保存経路は追加しない。設定OFFの診断なし・flat直行とfinal-only判定を維持する。

### 21.2 3件の取消と検証

`new_operation`の3件はneutral pollや同clickのreleaseではなく、演出終了直前の新しいnative矢印Pressだった。旧batch 47→51 Left（199.426ms）、55→59 Left（184.189ms）、75→79 Right（193.172ms）で、既定仮時間200msのturnへ割り込んでいる。対応するegui duplicateはbatch 52/60/80だが、継続操作のIDは元native 51/59/79。全件`target_reached=true`で、childのcanonical flat採用frame 830/868/950の次frame 831/869/951で継続をcommitした。既存flat-first保証と取消規則は変更しない。

childの実key／wheel handler→callback外ROOT受付を異なるcounterで再現し、通常single-stepがStarted→Finishedになること、Repeat／boundaryが元ID・source・phaseで一回のskipになることを検査する。既存handler、interruption、F12、projection、ui_snapshotを維持する。自動検証とbuildの証拠・短い利用者手順は`target/A-1341-stage1-fb-msg.txt`へ記録する。独立レビュー合格、実機での見え方修正、final-readyの開始率やGPU性能の受入れを先取りしない。


## 22. Residentの表示可能素材で即開始する（2026-10-11利用者決定）

`eefb9b370`後の利用者判断でfinal-onlyを撤回する。普通のページ送りが非AI画像からAI結果へ更新するのと同じく、受理時に表示可能な通常画像・preview・thumbnailが各incoming実頁にあれば、それを使う。§1〜21のfinal-only／complete必須記述より本節を優先する。設定OFF、仮時間、単頁LTR左／RTL右の固定背、Stage 1のshape/media/split/continuous/repeat省略、F12 ownership、flat-first保証は変更しない。表示素材が一切ない場合は待たずinstant。

### 22.1 素材選択と各面

acceptのresident-only lookupは通常表示の品質順序・identity・既存のRAW表示許可を守る。現在EditResultKeyとFinalCompositeKeyに一致するresident final compositeは`complete=false`でも表示可能。現在final pixelsをbaseにするcomic compositeがあればその合成を選び、未residentのcomic metadataを読まず通常finalへ戻る。finalがない場合、既存のカラー化／Creative LUTのfaithful-display gateを守り、residentの小さいfaithful renditionがあれば使う。それ以外はRAW preview／Static元画像、最後に通常表示で許されたthumbnailへ進む。旧document・旧edit keyのbest-effort finalを採用しない。lookupはhandle cloneと最大二頁の設定比較だけで、producer、decode、upload、metadata I/Oを呼ばない。通常navigationのprepare／producer／pollは従来どおり一回実行し、演出の仕事は追加しない。

| 面 | 素材の正本 |
| --- | --- |
| forward leaf front / outgoing rest | 最後に提出・採用されたoutgoing各頁のimmutable prepared resource。品質を引き戻さない |
| spread leaf back / incoming revealed | 対応するincoming各頁がaccept時に選んだresource。二頁の品質は異なってよい。backとrevealedは独立したoccurrenceとUVを持つ |
| single departure back | 既存neutral paper。未知の裏を生成・decodeしない |
| single backward arrival front / current rest | frontはaccept時のprevious incoming、restは提出済みcurrent outgoing。固定背とfront/back/reveal対応は既存recipeを保つ |
| 着地flat / interruption flat | 同じaccepted incomingを全頁canonical flatとして一度提示し、採用を完了条件にする |

TextureIdだけでなくtyped `FullscreenPaintResource`を保持する。thumbnailはDirectのままで、cache eviction／replacement後もResampleableへ再分類しない。final/provisional/normal/preview/thumbnail/faithful_preview/comicの各品質とsource sizeをoffered perfへ記録する。無素材は`skip=no_displayable_source_at_accept`、`sub_check=no_resident_texture`または`faithful_effect_not_resident`とidxを一回記録する。既存ready_at_acceptイベント名は解析互換のため残し、意味をfinal-readyからdisplayable-readyへ更新する。

### 22.2 品質固定と通常upgradeへの復帰

最初の通常prepareでaccepted素材のtransform／per-page placement／ROI／spineを決定し、turnとcanonical flat採用までは同じresourceとgeometryを使う。通常処理は続け、新しいAI結果・PDF raster・RAW現像結果は普通のcacheへ届く。途中でleafの画素や寸法を替えると別のpopと投影不連続が生じるので、そのframeではpresentationだけを保持素材へ束縛する。fresh painterを使い、actual layout・navigator・receiptを実際の保持transformとsource/native IDで組み立てる。通常navigation sequenceのreadinessは、ON-originのbound held resourceがある各頁を表示可能として採用し、thumbnail/previewを旧stop-page holdoverで隠さない。これは既存sequenceの素材判定への接続で、新しいready stateや後着startは作らない。別のquality queue、fade、upgrade timer、producer pauseは作らない。

canonical提出時に採用を進める一回のrepaintを要求し、採用の次frameに既存close→commit→capture境界が保持を解き、通常selector／geometryへ復帰する。高品質がresidentならそのframeから使い、未着なら既存経路で後に更新する。演出専用の二重swapと途中のgeometry popは加えない。ただし通常表示自体の低品質→高品質更新に伴う画質・丸め・metadata差は残る。「全品質で画素が同じ」とは保証しない。ON時だけupgradeの可視化が最大仮duration＋flat採用まで後ろへ移る。OFFの選択・処理・速度を変えない。

source publication epochはimmutable resourceの旧generationとして保持し、新epochに偽装しない。PDF／RAWの高品質publicationによるinput generation更新だけではcancelしない。代わりにitem identity/items、source_genを除いたedit revision、effective AdjustParams、comic generation、gap、context/window/viewport、layout/mode/sliceがcontent bindingを所有する。実編集・設定変更・resize/DPI・rotation/trim/fit-policy/clip変更は既存取消またはcanonical再prepareへ戻す。元画像previewの明示要求は通常selectorを優先し、ineligibleな編集・分析・ズーム表示やlive=falseのprepared pageへ保持素材を押し付けない。通常publicationのsource_size・canonical basis・Pair spine変化だけは最初のplacementを維持する。close／binding無効化は当該contextだけを退役させる。

### 22.3 Pin／VRAM／割込み

accept時からincoming sourceもoutgoing prepared source/native/checkerと共にpin予算へ入れる。共有weak inventoryはaccepted sourcesとprepared unitsを列挙し、VramAccountantがTextureId／native IDでdedupする。incomingの通常prepareによるnative ROI outputも開始前に同じbudgetへ加え、超過は`pin_budget`＋`prepared_incoming_roi`でinstant。予算拡大・animation用prefetch・新uploadは追加しない。park/mount/depositはcontext ownerを移すだけで、forkはInactive。batch leaseは実提出unitを保持し、effect退役後もin-flight textureを会計する。

new inputの取消は保持incomingでflat-first完了し、そのreceipt採用後に一件の既存continuationをcommitする。品質更新を待たず、古い品質でも全頁の提示を保証する。設定OFFも同じ対象のflat完了を経て通常表示へ戻す。新targetのqualityを前targetへ混ぜない。close/context失効は既存退役。採用済みflatのeffect reservationを解除し、batchが消えるまで実resident会計を続ける。

### 22.4 回帰と確認

resident Static／thumbnail／incomplete finalのaccept→通常draw→curl開始、thumb replacementのDirect保持、品質publicationとinput epoch更新中のcurl維持、canonical flatまで同素材でその後通常upgrade、取消flat-first、実編集取消、無素材とfaithful gateの理由付きskipを検査する。accept前後のpending/upload queueを照合し新仕事なしを保証する。Single四方向とPairの低解像度素材goldenを追加し、full-sizeと同じmesh/spine、紙・陰影・影、文字の向きを比較する。golden目視はmotion品質・実GPU性能の受入れを代替しない。

fmt、normal/portable check、focused、ui_snapshot、未起動確認後のnormal buildと手順は`target/A-1341-nonfinal-msg.txt`へ記録する。独立完了レビューと実機開始率／GPU性能の受入れは未了。

## 23. Pairの移動背とresident pin予算（2026-10-11利用者option A）

`49e2b0911`の実機評価ではresident素材により開始率が改善し、単頁の固定背も問題なしとの評価。
残る通常scanのPair省略を、利用者が選んだ**option A: 背と静止頁を連続移動し、incoming canonicalへ正確に着地**で置換する。
§16.3/§19の「静止rectのまま移動slotで切り、2%超なら省略」は本節より古い契約。
単頁、resident品質固定、default OFF/仮duration、入力identity、canonical-flat採用、F12所有・mount/deposit/forkは維持する。

### 23.1 一本の時計、各頁の自身の寸法

正規化progressをp、leafが既に使うeasingを`q=p*p*(3-2*p)`とする。
`H(p)=H0+(H1-H0)*q`。leaf角も同じqでinitial liftから180度へ進む。背だけを60〜120度で移す旧式は撤回する。
leafのfront/back寸法のmorphは従来の60〜120度のsmoothstepを使い、端点は旧S/frontと新T/backのcanonical位置へ一致させる。

- 旧T静止頁: 自身のcanonical rectを`(H(p)-H0,0)`平行移動。
- 新S下頁: 自身のcanonical rectを`(H(p)-H1,0)`平行移動。
- leafの旧front、新back、および各静止面は自身のimmutable resource/UV/寸法を保つ。静止頁の文字を他頁の縦横比へ伸縮しない。
- 一定gutterとinner gapは同じ背に付いて移る。静止したrectを別位置のhalf-planeで切らないため、旧式のspine clip/support lossは構造上発生しない。
- shadowは移動後のreceiver polygonにclipし、image/underlay/影/visibilityは同じposed rectを使う。

異高さ・大きい新下頁の外周は、**移動後の旧S footprintから、移動後の旧Sと新Sのunionへ**既存` smooth(2*p) `のrect gateを連続拡張する。
p=0で旧leaf外の新下頁余剰はゼロ、直後の差もゼロへ収束する。静止面のUVはgateで切られても自身の全rectをbasisにし、gateへ画像全体を詰めない。
旧Tの新T footprint外は最後10%だけ連続退役する。その差分は新Tを`H(p)-H1`移動したfootprintから求める。
p→1で新Sは新canonical、leaf backは新T canonical、旧T余剰alphaは0へ収束し、p=1の通常incoming flatとの位置/画素のsnapを作らない。
終端で旧spread全体を移したまま消す方式や、二頁をまとめてslide/fadeする方式ではない。

極端な600/300→300/600の幅交換では、初期60度以内で新下頁が旧leafに全部隠れる方向がある。
通常の30/45/60度の初期lift選択は維持し、その範囲でincoming可視性が得られないvalid Pairだけは30度で開始する。
**開始とfirst_visibleを分離**し、不可視のincoming面積は0として記録し、実際に現れたposeで初出を通知する。
角度を85度等へ跳ね上げて初出を偽らず、不可視頁の存在だけでnavigationを解放しない。
R5以降のsole completionは全incoming実頁のcanonical flat採用であり、この保証は撤回しない。単頁の初期可視性規則は変更しない。

### 23.2 destination clipの正本とinstant境界

実ログsession `143364-1791718409783`の6件はviewport `[0,44,2560,1298]`、ppp=1.5が双方で同じ。
一方painter clipはcontent rectとfull viewport `[0,0,2560,1440]`が交互だった。
通常spreadのpainterはcontent trimがあるとimage_rectでclipし、trimがないPage fitでは親painterを使う。
これはscan/layoutに伴うpaint policyの差で、描画destinationの変更ではない。
canonical captureで`effective clip = painter.clip_rect ∩ transform.viewport_rect`とし、同じcanvasを比較・使用する。
raw painter clipも診断に残す。実際のcanvas縮小/resize/DPI/clip変更は補間せず既存失効・通常表示へ戻す。

普通のportrait scanの幅・高さ・背の差、同一Pairの幅交換はshapeの省略理由にしない。
geometry不整合は明示理由で維持する: overlap、背が通常左右gap外（paint rounding一physical pxを超す）、invalid/empty rect、異なるlayout/slot数、混合clip/pixel scale、素材ROI未充足等。
Stage 1のcover/side/CenteredSingle/split/media/continuation等の既存未対応unit recipeは理由付きinstantのまま。
resident無し・効果faithful gate・repeat/burst・実メモリ圧力・pin予算はgeometryとは別の省略理由であり、待機や追加decode/uploadに置換しない。

### 23.3 pin_budget 11件の測定と上限変更

同sessionの11件すべてで、`pin_bytes = 旧source mip + 旧native RGBA8 + 新source mip`がbyte単位で一致し、余剰は0。
このサンプルのskipは残留pinや二重計上でなく、固定128 MiBの小さすぎるcapが原因。
8192px finalの5628×8192はmip込み245,885,644 B（約234.5 MiB）で、一頁だけでも旧capを超す。
実測accept最大773,292,152 B（737.47 MiB）。そのincoming頁の過去の通常native出力寸法から19,454,240 Bを加えた準備後推計は792,746,392 B（756.02 MiB）。
skip時はnativeを新prepareしていないため、後者をその操作の実測とは表記しない。
実resident最大1,254,059,108 B、poolは12,655,263,744 B、fullscreen枠10,124,210,995 B。

有限設定は従来の**fullscreen枠の10%**を維持し、128 MiBの固定capだけ撤去する。
この実機で上限1,012,421,099 B（965.52 MiB）となり、測定4頁+nativeを収める。pool/fullscreen設定、source品質、会計方法を拡張しない。
VRAM無制限設定だけは独立bounded capを1 GiBとし、この測定を収める余裕を持たせる。低VRAM有限枠は10%のままで、予算不足は引き続きinstant。
acceptとnative ROI準備後の両gateは同じhelperを使う。全context reservation、source/native/checker、batch leaseのTextureId dedupを保持し、普通にresidentだったからpinが無料とは扱わない。
perfへpin_limit_policy/fullscreen_budget_bytes/pool_budget_bytesを追加し、次回はどの枠が抑制したか判別できるようにする。
詳細算術と原寸はread-only logからの`target/A-1341-spine-pin-audit.txt`。この11件の整合を全lifecycleにleakが無い証明とはしない。

### 23.4 回帰・簡素化・検証

四方向のmoving-spine Pair golden（幅交換/異高さ、canonical旧参照、初回直後/中間/着地直前/終端）を追加し目視する。
200poseのH/translation/各面UV/静止面非overlap、0と直後の外周連続、p→1の収束、canonical終端のGPU一致、incoming初回0→後着初出→全flat、実canvasとoptional trim clipの区別を検査する。
予算はログの実寸をfake ID会計へ入れ、cache/context/batch重複参照のdedup、旧128 MiB超から10%枠内、低VRAM拒否、無制限1 GiBを検査する。
既存cross-context/cancel/flat-first/品質upgradeの回帰は継続する。R5-aのsilhouette/visibilityとtessellationの分離計測を維持する。

新pending、settle、spine owner、surfaceごとの時計、source縮小/upload、待機は作らない。単一recipeのframe-local poseだけを変更する。
固定静止面の閾値緩和や大きいscanを一括除外する案を不採用とし、資源とgeometryのownershipは既存contextに残す。
今回の自動結果・normal build・利用者確認手順は`target/A-1341-spine-msg.txt`へ記録する。
実装独立レビュー、実機motion/開始率/CPU・GPU受入れは未了で、自動greenだけを受入れとは扱わない。


2026-10-11自動検証の参考値: page-turn focused 144件、projection golden限定19件、normal/portable check、ui_snapshot157件が成功。
R5-aのcached debug CPU各300poseではportrait PDF scan Pairのsilhouette/visibility p95=0.0153ms、outer mask p95=0.0013ms（前者の内数）、tessellation p95=0.0706ms、合計p95=0.0844ms。
これは単独headless fixtureの参考値で、UI全frame/実GPU/実機motion受入れではない。詳細は`target/A-1341-spine-feasibility.log`。


## 24. 見開きの横長一画像と通常単独unit（2026-10-12）

本節は§3.1・§19のCenteredSingle一律省略、および§23の形状由来gutter差の拒否を改める。
利用者決定に従い次の版を目指して作業を続け、出荷判断は検証・実機motionの進捗から行う。

### 24.1 表示素材を二つの頁面へ正規化する

見開きの横長singletonは通常描画済みのpaint_rectの中央を背Hとし、左右half rectを持つ
二slotのBookLeafとして扱う。判定は通常compositionと回転後の表示形状から取り、sourceの
生寸法だけやtextureのpreview寸法で推測しない。単頁モードの横長はSingleWindowのまま。
source/通常native ROI/textureは一枚を共有する。通常回転を含む四隅UVの左右辺を50%で
補間して各halfへ配り、90/180/270度でも表示の左・右を切り出す。underlayも同じ表示UVを切る。
追加decode/upload、半面専用texture、history/seek/navigationの半頁は作らない。

| 旧→新 | LTR進む / RTL戻る | RTL進む / LTR戻る | 着地 |
| --- | --- | --- | --- |
| Pair→横長 | 旧右が表、新画像左halfが裏、右halfが下 | 旧左が表、新画像右halfが裏、左halfが下 | 一枚の新画像の通常rect |
| 横長→Pair | 旧画像右halfが表、新左頁が裏、新右頁が下 | 旧画像左halfが表、新右頁が裏、新左頁が下 | 新Pairの通常rect |
| 横長→横長 | 旧右halfが表、新左halfが裏、新右halfが下 | 旧左halfが表、新右halfが裏、新左halfが下 | 一枚の新画像の通常rect |

反対側の旧面は裏が覆うまで保持する。旧/新背はq=smoothstep(progress)で補間し、
旧静止面をH-H0、新下頁をH-H1だけ移動するoption Aをそのまま使う。横長の中央gapは0、
Pairは通常gapを持つが、この形状差は設定変更ではない。各inner距離をleafの同じmorphで
補間し、静止面は自身の寸法・UVを維持する。設定gap変更はcontent/binding失効のまま。
2026-10-12独立レビューP2への修正: 受理境界で、保存済みoutgoingの全実頁contentを
各頁の現content（設定gapを含む）と照合する。旧flat提示後に設定gapが変わり、まだ再描画
していなければpin予約・offer前に`skip=outgoing_content_changed`、
`sub_check=gap_setting_changed`を一回記録し、通常incoming flatへ進む。他のcontent差は
`sub_check=page_content_changed`。新しい設定でoutgoingを通常再描画した後は再び候補にできる。
横長のresolved gutter=0とPairのresolved gutter差は設定content差ではなく、この検査を通る。
投影でgutter一致を要求する方式は再導入せず、新state・待機・decode/uploadも加えない。
露出gate・終端外周退役・shadow/visibilityは§23の移動後footprintを共用し、終端でsnapしない。

### 24.2 末尾・境界の縦長singletonとVacant

通常の縦長singletonは通常paint_rect/sideを保持する。Left/Right配置はその実頁の内辺と
通常gapからHを取り、Center配置は実頁を読書先頭slot（LTR左、RTL右）として扱い、その
内辺をHとする。表示中の実頁を中央から別の場所へ移さない。空いたslotは明示Vacantで
surfaceもoccurrenceもreadinessも持たない。白紙へ置き換えず、canvas背景が見える。
Pairとの遷移も同じ二slot時計で扱う。2026-10-15の利用者決定により、片面だけVacantなら
§26.24の黒paperを用いた通常の一枚めくりとする。旧仕様の90度からのarrival／90度で終わる
departureと旧静止頁のfadeは撤去する。表がVacantなら0度の黒紙から開始し、裏がVacantなら
180度の黒紙までめくる。Vacantのstaticは描かず、occurrence／readiness／頁番号は足さない。
特殊な片側配置で表/裏が共にVacantとなる方向は、従来どおり`vacant_leaf_on_both_sides`でinstant。
全actual incomingの通常flat採用だけがnavigation完了条件。
実際の白紙companionは§26.21、補助表紙等は各採択節の既存挙動を維持する。
このVacant改訂ではsplit・媒体継続の適格性を変更しない。

### 24.3 受付・receipt・会計と切り分け

旧stage_one_shape_reasonはreal_pages!=2を一律省略し、offer matchingも実頁数一致を要求し、
canonical_unitは一枚ならSingleとした。三境界を通常compositionの同じ正規化規則へ揃える。
旧・新のactual頁数が違っても同じlayout token/destination/mode/roleを検査する。
投影だけのface identityは左右0/1、横長のreceiptは二面の可視面積を実occurrence一件に集約する。
短辺は各面の保守的spanの最大値を使い、bounding unionから架空の可視spanを作らない。
perfはactual頁とfaceの両方を記録し、halfを別navigation頁として数えない。
素材lease・TextureId dedupはactual resourcesに対して行い、横長二面でpin量を倍にしない。

`03_landscape_mid_10`の旧結果はPair→Pairだけ候補、Pair→[5] / [5]→Pair /
Pair→末尾[10]と逆方向がshape省略（その先にも実頁数/slot数の拒否がある）。回帰では
六unit・五境界の前後、LTR/RTL、横長連続、回転後横長/縦長、空slot、正規終端を検査する。
新goldenはPair↔横長・横長↔横長・Pair↔縦長singletonと四方向のUV/紙面/背移動を目視確認する。
簡素化は既存BookLeafへの素材正規化を採用し、別state・pending・worker・navigation topologyの
増加を退けた。OFFの通常描画・レイアウトには変更を加えない。

実ログ補足: read-only `perf_events.4.jsonl` session `47452-1791765013300`では対象フォルダを
開いた後に`stage1_centered_single`が8回。7枚目のsourceは900×1300だが通常paintは1300×900で、
保存済み回転を含むunit列だった。READMEの未回転六unit列の結果と混同しない。
対象59画像のEXIF orientationは全て無し。画像ヘッダだけ読み、素材・profileは変更していない。


### 24.4 実装回帰と検証記録

通常composition/offer/prepare/receipt/提示observerを通る未回転40遷移（5境界×LTR/RTL×前後×
末尾配置OFF/ON）と、元横長5/元縦長6・7の90/180/270回転180遷移で、開始・canonical着地と
actual occurrence数を検査した。fixtureは実資料と同じaspectの1/10素材で、実PNGのlive操作ではない。
153 focused tests成功、5新golden（2,911,571B）を目視。終端は製品と同じ通常描画を独立quadへ
画素比較し、終端直前はprojectionのまま検査する。初回の二half quad対一whole quadの比較に
12画素/最大RGB1のGPU丸め差があり、許容差を緩めず製品経路へfixtureを揃えた。
normal/portable check・fmt/glyph lint成功。test-fullは13,374 passed/62既存ignored/0 failed
（本体lib12,119、UI snapshot157、vendor54含む）で成功。確認用buildの結果は
`target/A-1341-landscape-msg.txt`へ記録した。normal確認用core/remote/EPUB workerの
`build-dev -PreserveRuntime -WaitForOtherBuildsMinutes 0`も成功（待機省略は残存MSBuild nodeへの対応、script変更無し）。アプリは起動せず、独立レビュー・実機motion/性能の
受入れ・出荷時期の確定は利用者/設計担当の判断へ残す。

同日、`787b4bf74`の独立レビューP2（gap設定変更後の古いoutgoing）へ対応。
形状由来gutter差の8遷移は開始・完走、設定変更後の24遷移は受理時skip一回・pin予約無し・
通常incoming flatを検査した。focused 155件、normal/portable check、fmtが成功。
test-fullは一回実施したが、無変更のclipboard HTTP deadline fixtureが接続待ちで終了せず、
このworktreeの本体libテストプロセスだけを診断後に停止したため未完了（exit 101）。
後続を含む完了済み58スイートは1,201 passed/7 ignored/0 failed、うちui_snapshot157件成功。
本体libの最終summaryは無く、vendor 3 crateはscriptの非ゼロgateにより未実行。
全体greenとは扱わず、再実行もしていない。詳細は`target/A-1341-gap-fix-msg.txt`。
同HTTP fixtureの単独確認は1件成功（1.43秒）。全体実行で接続が未到達となった理由は
未確定であり、単独成功で全体gateの未完了を解消したとは扱わない。
今回の確認用buildは利用者指示で行わず、稼働中の通常core・dev-runtimeへは触れていない。

## 25. ROI端の丸めと単頁の静止下頁（2026-10-12）

### 25.1 利用者決定: 単頁は一枚だけ動かす（案A）

縦長↔横長を含む単ページ表示では、forwardは旧頁だけが自身の通常rectの綴じ目
（LTR左／RTL右）から去り、新頁は最初のposeから最終canonical rect全体を静止して下に置く。
backwardは前頁（incoming）だけが自身の同じ読書側の綴じ目から降り、現在頁（outgoing）は
通常rect全体を静止して下に置く。下頁のrect補間・拡大・外周拡張clip・退役fadeは行わない。
旧縦長の綴じ目が画面1/3付近、新横長の綴じ目が画面端にあることは各頁の通常placementであり許す。
頁間で背の絶対座標を揃えたり、下頁をその背へ移したりしない。

終端と取消は既存のincoming canonical flat提出・採用を共用する。backwardで旧下頁の外周が
incomingの外に残る場合、終端で通常incoming flatに置き換える。その外周を途中で縮める別演出は
加えない。透明部分の下頁/underlayはturn中の通常重ね順、終端は通常incomingの背景に従う。
§17.3/§21の異寸法単頁に対する外周gate/退役は本決定で置換し、Pairの移動背・外周連続規則は維持。
新recipe/state/時計/textureを増やさず、既存SingleDeparture/SingleArrivalの静止surfaceだけを修正する。
矩形morphや二頁slide、下頁を順次開く方式は利用者の見え方と異なるため採用しない。

LTR/RTL・前後・縦長→横長/横長→縦長の8routeをdense poseとGPU goldenで検査する。
動くleafの所有・自身の背/寸法/UV、静止下頁のrect/全clip/alpha不変、p=0/直後の全下頁、
canonical着地と取消は別々に確認する。追加decode/uploadやreadiness変更は行わない。

### 25.2 Native ROI端の契約

実機のLTR skipはnative prepared ROIの右端`0.9999999403953552`とcanonical source UV`1.0`の
厳密contains比較で`partial_roi`となった。source UVからnative UVへの写像には丸めの許容を
source texelの0.001（`TRANSFORM_EPSILON / source_texture_size`を軸別に適用）だけ設け、
許容した端点はROI境界へclampしてからnative UVへ正規化する。本当に足りないROIを
許容したり、不足部分の追加uploadを要求したりしない。LTRB・quarter rotationでも同じ契約を使う。
実値のLTR/RTL回帰と、許容を超す部分ROIの拒否を対にする。fixtureとproducerの差、
自動結果・golden目視・確認用buildは今回の引き継ぎへ記録する。

実ログ6件の右頁幅は`2278.666748046875 - 1282 = 996.666748046875`、ppp=1.5で
`1495.0001220703125`physical px。通常paint bandは1495pxへ丸め、そのUVへの逆写像が
`1495 / 1495.0001220703125 = 0.9999999403953552`（f32）。`LanczosSourceRegionKey`は
そのbitsを保持する。欠損に見える量は900px sourceで0.00005364418 texel、8192pxでも
0.00048828125 texelであり、実pixel不足ではない。producerの通常clip/ROIを拡張せずcaptureで扱う。
旧handler fixtureは`wgpu_render_state=None`でnative ROI作成前に元textureを返したため、
LTRが通っても本件を検出できなかった。今回の回帰はheadless RenderStateと普通のnative準備を
使い、実際に同じROI端が作られたことをassertしてから開始・完走を検査する。

### 25.3 単頁の綴じ目移動を比較する暫定設定

2026-10-12利用者追加提案により、§25.1の案Aと移動する綴じ目を実機で比較する。
最終採択は利用者が実際の動きを見て決める。環境設定「閲覧表示」のページめくり設定に
「単頁の綴じ目（比較用・暫定）」を置き、`page_turn_single_spine=OwnPage`（案A）を既定、
`Moving`を比較案とする。評価時間のスライダーと同様の暫定設定で、比較後は一方と切り替えを削除する。

| 案 | 動くleaf | 下頁 |
| --- | --- | --- |
| 案A・自身の綴じ目（既定） | forwardは旧頁自身のH0、backwardは新頁自身のH1を固定軸にする | forwardは新頁、backwardは旧頁を自身のcanonical rect全体で静止 |
| 比較案・綴じ目を移動 | 同じ一枚のleafをH(p)=H0+(H1-H0)×smooth(p)で移動させながらめくる。内辺は常にHに付き、自身の幅・高さ・UVを保つ | 案Aと同じcanonical rect全体で静止。背の移動に追随させず、rect補間・外周gate・fadeもしない |

H0/H1は通常singleのLTR左端／RTL右端。同じsmoothstepとH補間helperを見開きoption Aと共有する。
見開きはその既存recipeの静止面translation・outer gateを維持し、単頁はそれを共用しない。
共通化は背の軌道までとし、単頁の一枚だけ動く要求と下頁の静止を優先する。
forwardは旧leafが90度で消え、backwardはH1・角度0へ収束する。着地・取消は既存canonical flatへ戻る。
設定はprepare時のTurnRecipeに固定し、初回visibilityと全poseが同じ選択を使う。
OKで設定を変えたときは既存の全context設定取消を使い、進行中のleafをlive rebuildしない。
新pending・state owner・clock・worker・texture・upload・navigation操作は追加しない。

独立設計レビュー（GPT-6.1 Sol / xhigh）はこの投影境界の構成に合意。素材/geometryの所有と
detachedのviewport述語・生成・mount/deposit・continuationの境界は変更しない。
両案の縦長↔横長×LTR/RTL×前後をdense pose、通常handler、GPU goldenで検査する。
自動検証後は、利用者指定に従ってmaster（v4.6.0以降）の取り込みをcoordinatorへ引き継ぎ、
build-devの直前で停止する。コミット・アプリ起動は行わない。

### 25.4 引き継ぎ差分の自動検証と残るgate

同日の継続セッションで、前任のgap/ROI/案A差分を確認し、比較案を追加した。
`page_turn` focusedは163 passed、設定転送は25 passed、normal/portable check、fmt/glyph lint成功。
両案16route×203poseでleafの所有・自身の幅高さ・内辺、静止下頁の全rect/clip/alphaを検査し、
通常handlerでも16遷移の開始/完走と、設定変更取消→canonical flat採用→次操作を確認した。
実測と同じnative ROI端が通常prepareで作られるLTR/RTL前後4routeと、実partialの理由付きinstantも成功。
従来partial試験の観測不備は、perf OFF時にprojection失敗のTEST_EVENTSまで省かれたことが原因。
軽いevent観測を共通emitへ通し、重いgeometry診断はperf ON限定を維持した。

案A8枚/Moving8枚の新projection golden（計2,327,152 B）、高さ差の既存2枚更新、設定3枚を目視。
正規終端は独立の通常canonical quadとGPU画素一致を検査した。
独立GPT-6.1 Sol / xhighレビューは設計・全source・新Moving8枚/設定3枚・転送互換試験へACCEPT、
要修正指摘なし。これは実機motion/性能の採択や全体gate成功を意味しない。

`test-full.ps1 -SuppressCrashDialogs`は一回実施、exit 101で未完了。
テスト実行並列を`RUST_TEST_THREADS=8`へ制限したが、コンパイル並列は未指定だった。
コンパイル中にrustcの`memory allocation of 2096992 bytes failed`で本体lib testが終了し、
後続にE0463、compiler ICE、rlib不足エラーが出た。test result summaryは0件、vendor 3 crateも未実行。
timeoutや製品assertion failureとして扱わず、全体再実行・cache削除・無関係なsource修正はしていない。
今回の実行は前回のHTTP fixture待ちまで到達していない。
UI snapshot単独は`CARGO_BUILD_JOBS=1`/`RUST_TEST_THREADS=8`で157 passed（39.09秒）。
この成功で全体gate未了を解消したとは扱わない。

利用者指定のmaster取り込み境界で停止し、build-dev・master merge・コミット・アプリ起動は未実施。
coordinatorは未コミット差分を保護してmaster（確認時v4.6.0）を取り込み、全体gate未了を解消して
確認用buildを用意する。取込で変わった共有経路の検証結果を、このpre-merge結果から先取りしない。
根拠・各command/feature/exit code・ログ・build後の隔離比較手順は
`target/A-1341-roi-spine-compare-msg.txt`。実機motion・最終案/時間・出荷受入れは利用者/設計担当へ残す。

### 25.5 master取り込みの入力・描画契約

2026-10-12、coordinatorが§25.4の差分を`a7155f674`へ保護し、master `d357fd5ae`
（v4.6.0以降のEverything・PDF世代・spread shift・rating/grid cursor・監査復旧等）を取り込んだ。
競合6ファイルは双方の契約を合成し、backlogは§1.341以外をmasterと一致させる。

rating publicationによるClosed/Rebasedは通常root・委譲root・viewer内の全入力経路で扱う。
Rebased後は同じ描画経路なら既存prepareを呼び、state/compositionとfooterのpage snapshotを
新しいページへ更新する。描画経路が変わればそのframeを終了し、既存dispatcherへ戻す。
ClosedはCentralPanel後のvisibility/tailも終了する。Manual/RatingMembership共通closeで
page-turnと未配送readyを退役し、rating Undo/toastはmasterのmetadata boundaryを維持する。
manual navigationによるRestoreOrigin取消はflat-first defer前に行う。

gamepadのoperation witness/replay判定とrating picker終端型を共存させ、masterの
GridCursorIntent・spread shift・PDF ticket・Everything所有境界を保持する。
新しいstate ownerやnative host述語は加えず、既存frame/context境界を接続する構造に
独立GPT-6.1 Sol / xhighレビューが合意した。検証結果・確認用buildは今回の引き継ぎに記録する。
git add/commitはcoordinatorへ残し、アプリ起動と実機motion採択は行わない。

masterの監査復旧との合成では、既存page-turn registry API 5件の正確なfingerprintを登録した。
配送bindingとMounted/AtRest readyの読み取り、追跡要否、tray hide/全体設定の明示取消だけであり、
通常mountによる兄弟contextの変更はない。Everythingとpage-turnのtest-only module計3ファイルは
parentのcfg(test)に加えてinner cfg(test)を明記した。監査規則や製品公開範囲は変更しない。
限定監査追従も独立レビューACCEPT、`cargo run --locked -p viewer_context_audit`はexit 0。

追加merge回帰は、rating→nextのroot/委譲root/viewer、空集合close、Turning/Completing終了、
flat defer前のRestoreOrigin取消を検査する。ratingとnextの複数Pressが同居し、初回drawも未採用な
fixtureではアニメーションofferを必須にせず、実submissionの頁1とfooterによる移動先2を検査する。
一操作・採用済outgoingの開始/完走gateは既存focused回帰が検査する。

取り込み後の最終sourceでfocusedはpage_turn 167件、rating_membership 80件、grid_cursor 31件、
spread_shift 128件、settings_transfer 25件、viewer_context_audit unit 36件が成功。
通常/portableのcore check、fmt/glyph lintもexit 0。全体gateとbuildの結果・各ログは
`target/A-1341-merge-msg.txt`へ記録する。実機での採択を自動結果から先取りしない。

全体初回はlib 12,366件成功/1件失敗/57件ignored、UI snapshot integration 162件成功。
唯一の失敗は`preferences_viewer_notice_visibility`のscroll thumb下端で、単頁spine比較UIの
追加に伴う総contentheight増による約1px短縮だった。本文・checkbox・色・配置は画素一致。
差分画像/sourceを独立レビューして1goldenだけ通常UPDATE_SNAPSHOTS経路で更新し、対象試験は成功。
判定の許容値を広げず、利用者指示に従ってbuild jobsを2から1へ下げた全体再実行を1回行う。

全体再実行はexit 0 / PASS。本体12,367件成功・0件失敗・57件ignored、UI snapshot162件成功。
workspaceの残りとdoc-tests、vendor egui 25件・egui-wgpu 11件・eframe 18件も成功した。
初回の失敗/未実行をこの結果で解消し、通常の確認用buildへ進む。コマンド・環境・時間・ログは
今回の引き継ぎ記録を参照。実機motion・性能・最終案/時間・出荷受入れは利用者/設計担当へ残す。

`build-dev.ps1 -PreserveRuntime`はbuild jobs=1、normal feature / dev-runtime profileで成功。
core・remote service・EPUB workerの生成とruntime 4 DLL / PE 3 binary検査を完了した。
アプリ起動・実設定操作・git add/commitは行わず、mergeの登録とコミットはcoordinatorへ引き継ぐ。
利用者の隔離`--data-dir .\target\dev-runtime\data --perf-log`比較手順は今回の記録に用意した。

## 26. 2026-10-13 採用改訂：共通の黒い紙と実効両端配置

本節を単頁サイズ差と両端配置の現行仕様とする。§25の案A / Movingは比較の履歴であり、
両方とも撤去する。利用者の実機確認で、広い頁から狭い頁へMovingで移る変形に違和感があった。
異なる背・紙サイズ・floor移動の組み合わせを増やすより、同じ大きさの紙に揃えて状態を減らす。

### 26.1 単頁は一つの紙サイズへ揃える

同一viewportにおける旧・新のcanonical画像rectのunionを共通paper rectとする。
通常の中央fitでは大きい方の幅・高さを持つ紙となる。丸めを含め両rectを覆う最小rectを使う。
軸はLTRならその左端、RTLなら右端に固定し、紙の幅・高さ・背を補間しない。
画像自身のrect、UV、元texture、underlayは保持し、紙内の印刷領域として投影する。
余白は既定fullscreen canvasと同じBLACK。めくる面も下の面も同じ紙全体を黒で埋める。
新texture、decode、uploadは作らない。陰影・厚み・角度・終端採用は既存同サイズ単頁と共通にする。

前進は旧画像を印刷した紙だけがめくれて去り、新画像の紙は最初から最終位置で静止する。
後退は前頁の紙だけが降りて重なり、旧画像の紙は静止する。下の画像のrectは補間しない。
投影の遮蔽・影は紙の全領域、first_visible/visibility/receiptは実画像の印刷領域で計算する。
黒余白を頁提示の面積に数えない。終端で通常のcanonical画像へ戻る余白は既定の黒canvasと一致する。
カスタム余白色の保存値は変更しない。効果中の紙は黒、終端はその保存済背景色の通常描画へ戻る。
非黒canvasでは黒paperの消失が見えるが、終端に黒paperを残留させない。
共通paperの距離・高さをfocal/error boundにも使い、従来のmesh誤差と予算の制限を保持する。

暫定`page_turn_single_spine`、`SinglePageTurnSpine`、比較UI/検索・説明・比較snapshotは撤去する。
未リリース設定なのでmigrationは作らず、古い保存keyは通常の未知keyとして読み捨てる。

### 26.2 local pagedの両端配置を実効ONにする

local paged見開きのcomposition構築だけで、アニメーションON時にendpoint.first/lastをtrueにする。
全体保存値・本別FollowGlobal/Place/Centerを変更しない。OFFでは保存値による実効値へ戻る。
既存complete-book / UnpairedSlotの適格条件は維持する。shared resolver、連結読み、Remote、
本別設定getterは保存値を扱うままにする。2026-10-13の利用者決定で末尾も先頭と揃える。
環境設定の両端checkboxはON中に実効trueを表示して無効化する。表示用の値を保存値へ戻さない。
OFFでは保存値の表示・操作へ戻る。本別HUDの両端指定もlocal paged/ON中は実効Placeを表示して
無効化し、FollowGlobal/Place/Centerの保存値を変更しない。他の見開き調整は維持する。
理由を近くに表示し、検索・マニュアル・snapshotを揃える。連結読み/Remoteの本別指定は
強制の対象外なので、その保存値を扱うUIも変更しない。

ON/OFFはcomposition geometry変更として既存の全context設定取消へ集約する。
mounted/parkedの未配送readyと旧Flatを退役し、Completingの採用済receiptもNeedFlatへ戻す。
既存ownerにある一件のresolved continuationは保持し、新設定の通常canonical flatの採用まで
解放しない。新pending、bool、queue、context ownerは増やさない。通常mountの兄弟変更はしない。
時間だけの変更も同じ安全な設定採用境界を通す。表示経路・texture sourceは通常描画で再採用する。
Turning/Completingの品質変更だけによるgeometry freezeは維持する。
通常prepareのplacementが変わった場合はreal geometry変更としてfreezeを外し、
Completingを現在のCenter/Sideへ再bindする。未採用evidenceは旧hookと共有しない新しい証明へ置換する。

### 26.3 表紙と末尾の実builder診断・回帰

変更前は実builderの表紙→Pair / Pair→表紙をLTR/RTL×中央/sideで組み、
accept / outgoing canonical / incoming canonical / recipeのどこで拒否するかを型付きで区別する。
変更前の表紙guardは中央/sideともcanonical化で`stage1_portrait_cover`を返していた。
強制sideだけではこのguardは消えないため、実builderのfirst unit、UnpairedSlot、cover mode、
Navigation occurrenceの一致と通常compositionが解いたRight/Leftに基づいて表紙を許可する。
中央の未証明表紙へbody用fallbackの左右を当てはめない。white、分割、未適格構成の
既存拒否は保持する。補助表紙の受理は§26.6で改訂する。軽い通常ログは一操作のskip時だけ
理由/sub_checkを記録し毎frame出さない。

実builder回帰は表紙LTR/RTL×前後、ON/OFF保存値・本別設定保持、連結/Remote非変更、
縦長⇔横長×LTR/RTL×前後、黒余白/画像UV/可視性/終端を検査する。画像は利用者の本を縮小した
12頁fixture（4/7/8横長）を使い、末尾singleton確認には11頁派生を使う。
末尾は中央/side×補助ON/OFFで同じ診断を行う。前回の診断結果は§26.4に履歴として残し、
今回の実効末尾配置・補助表紙付きPairの回帰は§26.6へ記録する。
独立設計レビュー後に実装し、関連回帰・snapshot・fmt/check・glyph・全体gate・通常build-devを行う。
コミットと実機採択はcoordinator/利用者へ残し、エージェントはアプリを起動しない。

### 26.4 実装・回帰で確認した所有とpolygonの補正

変更前の表紙8経路は実builder/通常描画/実handlerでcanonical拒否を確認した。
先頭side強制だけでは解消しないため、§26.3のproofを使ってcover guardを置換した。
12頁fixtureの末尾はPair [11,12]。11頁派生の末尾singletonは補助OFFなら中央/sideとも
前後にめくれ、補助ONでは`stage1_cover_or_single_companion`でaccept拒否される。
末尾は強制しない。補助/whiteの対応を先取りしない。

独立実装レビューで、設定取消時のevidence置換とOFF通常prepareのgeometry再bindが生成する
fresh PaintedUnitのWeakをglobal lease inventoryへ登録する必要を確認した。
両生成境界で既存inventoryへ登録し、旧output batch退役後もmounted/parked Completingの
retained/pin bytesを会計する。旧Arcを強参照で残さない回帰で両境界と最終予約解除を検査する。

新しいdense pose回帰では、紙と印刷域を重ねて二度subtractする実装の不備を発見した。
一つのvisibility faceに実画像のvisible polygonと全紙のocclusion polygonを持たせて一度だけ引く。
また既存clip_halfはscissor角で同一点を連続出力し得る。subtract_polygonがそのzero-length edgeを
inside/outside両方として扱うと残りpolygonが重複計上されるため、正確な空edgeを除外する。
退化coverは遮蔽しない。面積の許容を緩めず、重複corner/退化coverと全poseを回帰にする。
この数学的修正は通常のclipped Spreadにも共通に適用し、関連回帰と全体gateで確認する。

### 26.5 自動検証と確認用buildの引き継ぎ

独立GPT-6.1 Sol / xhighの完了レビューはACCEPT、全指摘を解消した。生成16goldenを目視し、
共通paper12枚・設定2枚・先頭notice1枚と、本文が一致するpreferences scrollthumb1枚を更新した。
非UPDATEのfocused179件、設定転送26件、normal/portable core check、fmt/glyph、context監査が成功。
撤去設定1件分の分類/許可/wireの期待件数だけを修正し、除外/unique/export集合の検査は維持した。

`test-full.ps1 -SuppressCrashDialogs`はbuild jobs1 / test threads8で一回PASS。
本体12,379件成功・57件ignored・失敗0、UI snapshot162件、vendor54件、残りworkspace /
integration / doc-testsも成功した。normalの`build-dev.ps1 -PreserveRuntime`もexit0で完了し、
core/remote/EPUBとruntime4 DLL / PE3 binary検査を通した。coreは2026-10-13 21:58:30の
325,658,112 bytes / FileVersion4.6.0.0。portable featureを有効にせず、起動もしない。
command/feature/profile/結果と隔離確認手順は`target/A-1341-black-paper-msg.txt`に記録する。
コミット・アプリ起動・実データ変更は行わない。motion/性能の採択はcoordinator/利用者へ残す。

### 26.6 2026-10-13 末尾配置・UI・補助表紙の追加採用

前回の共通黒paperと表紙の実効配置について、利用者は実機で「アニメーションよさそう」と
報告した。今回も状態の組み合わせを減らす方針を継続し、末尾も本来の側へ揃える。
旧§26.4の「末尾は強制しない」「補助は拒否」は前回時点の診断であり、現行仕様ではない。

共通builderの補助条件は、表紙ありmode・補助設定ON・完全な単本のcanonical列・現在が末尾unit・
先頭unitと末尾unitがともに一枚・現在のnavigation頁が末尾unitと一致・表紙idxが末尾anchorと
異なること。単に最終画像が一枚という条件ではない。横長等のsingletonもこの判定に含まれ、
ForcedLastの白い相手枠には表紙が入り、通常のsingletonでは読書方向どおりの二枚を構成する。
この既存条件を今回変更しない。navigation/seek列は一枚の末尾anchorを保持する。

アニメーション受理はNavigation一枚＋FrontCoverSupplement一枚の実画像Pairを許可する。
通常描画のrole/occurrence・resident source・texture leaseをそのまま使い、recipe/投影/receiptも
通常Pairと共通にする。補助idxをnavigationへ変換しない。同じ表紙textureを再利用しても役割と
末尾anchorを保持する。受理時に未residentなら既存readiness規則でinstantとし、UI同期decodeや
専用upload/pending/queue/ownerを追加しない。whiteや他のcompanionの拒否は維持する。

実builderから前のunit→補助付き末尾Pairと逆方向をLTR/RTLで通常描画・実handler・完走まで
検査する。補助表紙のtexture/role、末尾anchor・visibility・終端通常flat、白companion拒否を
回帰に含める。両端ON/OFF・全体/本別保存値保持・連結読み/Remote非変更、環境設定とHUDの
無効化・実効表示・OFF復帰を検査する。独立レビュー、関連回帰/snapshot、fmt/check/glyph、
通常build-devの後、最後にtest-fullを一回行う。アプリ起動とコミットはしない。

### 26.7 2026-10-14 末尾追補の実装・検証記録

local paged compositionでfirst/lastをともに実効ONにし、全体と本別の保存getterは変更しない。
環境設定は一時的なtrueをdisabled checkboxへ渡し、paged本別HUDは保存choiceを保持したまま
実効Placeをdisabledで示す。OFFで保存値の表示・操作を復帰する。連結HUD/Remoteは保存値を扱う。
検索索引の実ラベルとの一致と両端/無効化/保存値/Remoteの検索到達性も回帰で確認した。

補助PairはNavigation一枚＋FrontCoverSupplement一枚・共通anchor・Navigation idx=anchor・
異なるsource idx・cover modeを、受理/retained outgoing/Offered matchingの同じpredicateで証明する。
builderの補助適格条件は§26.6のままで、既存通常描画が生成したprocessed/fallbackのresident素材を
使う。raw decode cacheのtexture IDと同一とは仮定しない。追加decode/uploadやownerを作らない。
実builderの11頁末尾16経路、横長/ForcedLast、2頁の同じ表紙textureの役割別再利用、canonical
receipt/visibility/終端を確認した。12頁本の末尾は通常Pair、whiteは引き続き拒否する。
両端global独立boolと本別独立3choiceの72fixtureでOFF→ON→OFF、実layoutと連結/Remoteを検査した。

関連page_turn182件、singleton11件、preferences181件が成功。設定4枚とHUD4枚のgoldenを生成し、
独立レビューでも目視確認した。標準disabled opacityを二重に薄くせず、HUDの最終行まで収める。
非UPDATEのUI snapshot全163件、normal/portable core check、fmt/glyph、context監査が成功した。
独立GPT-6.1 Sol / xhighの設計・最終レビューはACCEPT、P1/P2無し。

normal `build-dev.ps1 -PreserveRuntime`は通信中断後の再実行でexit0。core/Remote/EPUBとruntime
検査まで成功し、coreは2026-10-13 23:23:51、325,668,864 bytes、FileVersion4.6.0.0。
アプリ起動・コミットは行わず、隔離data-dirと11頁コピーの場所を検証台帳へ記録した。

最後の`test-full.ps1 -SuppressCrashDialogs`はjobs1/threads8で一回実行し、exit101。
本体12,386成功・1失敗・57ignored、UI snapshot163成功、他のworkspace/integration/docは成功した。
変更していない音声の`zero_start_pump_trims_latency_and_preserves_samples_through_eos_loop_and_seek`
がrate=48000/phase=0でaudio_tail_completeの3秒待機期限に達した。同テストの単独一回は0.34秒で
成功したが、全体失敗の原因確定や全体PASSとは扱わない。音声sourceや待機期限は変更していない。
workspace非ゼロによりscript後半のvendor gateは今回は未実行。全体gateの再実行はしない。
詳細・コマンド・ログ・binary SHAと隔離確認手順は`target/A-1341-terminal-msg.txt`を参照する。

### 26.8 2026-10-14 隣の表示単位を先読みする

利用者の22頁PDFでは末尾の一つ前から22頁＋表紙へ進むとき、表紙idx0のGPU textureが
未residentで受理を省いていた。従来のsource/AI先読みと保持は画像indexの近傍を選び、
遠い補助表紙を加えるのは現在表示中の組だけだった。CPU Retained pageのhitはGPU素材の
準備完了を意味しない。短い本で成立した理由は設定・組み方・cache履歴を未確認で断定しない。

local pagedのアニメーションONでは、通常navigationの実unitと実compositionを共通の需要にする。
各方向の既存予算はNavigation画像頁数で消費し、選ばれたunitの実画像は補助表紙も含めて全て選ぶ。
予算境界でunitを分割しない。現在unitの相方は隣unitの予算を消費しない。ゼロ方向には
新規投機要求を出さず、既存の保持幅＋1はresident素材のpassive保持として残す。
sourceとAIの独立した予算を尊重し、双方の需要をlive cacheのkeepに合流する。
indicatorも実unitの位置で前後を表し、遠い表紙を先頭側の処理と誤表示しない。
動画・音声singletonは投機画像処理も画像枚数予算の消費もしない。実navigation位置を保ちつつ
その先の画像unitまで走査し、従来の画像枚数の先読みを縮小しない。OFF・連結読み・Split・Remoteは従来どおり。

既存current_loading抑止とpage-turnの非target作業admissionを維持する。ready後や組み合わせ
変更後は導出した需要との差分だけを既存source schedulerへ渡す。root/active detachedの
同じ非target作業admission内からmounted contextだけをreconcileする。受理時に同期decode/uploadを
しない。PDFのRetained FinalAiだけが残る場合も、source先読み予算から既存final-effect workerで
最終GPU素材を準備する。colorize/LUT無しやAI先読みゼロでも既存CPU結果を再利用できるが、
新規AI推論はAI予算に従う。新しいpending/bool/queue/ownerは追加しない。

Retained pageのentries/bytes上限と共有LRUは変更せず、隣unit全体を永久pinしない。
CPU表紙が押し出されてもresident GPU素材があればめくれる。両方が失われた場合は既存loaderで
再準備し、受理までに間に合わなければ既存readiness規則でinstantにする。待機・予算超過・
専用再試行は追加しない。contextをまたぐcancel/eviction/pollのownerも変更しない。
PDFのexact current edit/params/bypass keyとtexture identityが一致する完成live compositeは、
display需要ではresidentとして扱う。raw sourceの状態は変えず、RAW比較や現像要求を妨げない。
表示前の各ensureと通常texture選択でも同じ派生判定を使い、CPU LRU退去後の不要な再ロードで
完成GPU素材を失効させない。RAWのdisplay需要の変化は既存需要計算との差分で検出する。

先頭表紙→最初の組、補助付き末尾への前進/後退、本別補助指定を同じbuilder経由で扱う。
単頁の縦長⇔横長の黒paperは実画像二枚の隣unit需要だけで、黒紙に別textureは不要。
22頁実builder・source/AI独立予算・ゼロ・保持・Retained CPU→GPU・二contextを回帰で確認する。
独立設計レビューは構造的修正として承認。関連テスト/fmt/checkと通常build-devを行い、
今回test-full・コミット・アプリ起動は行わない。

### 26.9 隣unit先読みの検証記録

実builderの22頁でpenultimate19/20から末尾21＋表紙0をatomicに選び、既存Retained rasterの
source queue/paced pollとRetained FinalAiのfinal-effect workerからGPUを準備した。
CPU cap1退去後もexact GPU素材で実handlerの前進/後退が開始・完走する。
LTR/RTL、current partner非消費、source/AI独立予算、ゼロ、media、OFF、RAW需要変更、
LatestSeek、二contextのcache/cancel/channelを回帰した。既存Retained entries/MiB上限は維持する。
極小PDF fixtureはnative Raster4×6の要求長辺6pxを証明して保持素材と合わせる。
Vectorの最小256pxに6px rasterを当てる不一致を、製品側の許容拡大で隠さない。

独立GPT-6.1 Sol / xhighの最終静的レビューはACCEPT、未解消P1/P2なし。
focused16、page_turn186、retained53、pipeline163、RAW27、UI snapshot163が成功した。
normal/portable core check、fmt/glyph、viewer_context_auditも成功。snapshot golden変更なし。
新test moduleへfile-level cfg(test)も明示し、auditのtest-only境界と実compile条件を揃える。
test-fullは利用者指定で未実行。新field/pending/queue/owner、共有LRU変更、RAW policy拡大はない。

通常build-devはPreserveRuntime指定で開始したが、既存確認core PID54220が起動中のため
scriptの保護でcompile前に停止した。利用者の終了待ちであり、アプリはagentから起動/停止しない。
build完了後に結果を追記する。詳細・command/log・二冊の隔離確認手順は
`target/A-1341-neighbor-msg.txt`へ記録する。コミットは行わない。

### 26.10 2026-10-14 独立レビューの二件を修正する

利用者はc8c343767の006末尾と表紙のめくりを実機でOKと報告した。独立レビューの
REVISE二件は素材の所有境界で修正する。完成compositeはCPU pixelsとsource_sizeも所有するので、
現在のedit/params/bypass/identityに一致する完成entryから画素を直接取得する。コピー・capture・
注釈合成のために元画像を再ロードしない。古い/未完成/別contextのentryやanimationは拒否する。
注釈の座標基準は既存source_dims、同generationのPageDimsCache pixel dims、完成素材の
source_sizeの順で得る。8K clamp前の元頁寸法と編集/AI出力寸法を取り違えない。
capture/exportは既存のNavigation頁のみの出力を維持し、補助表紙を出力へ追加しない。
末尾頁と表紙の個別capture・注釈画素を退去前後で検査する。

先読みは取得済みunitsとunit positionを共通pure composition resolverへ直接渡す。
既存SpreadDisplayUnitsCacheにnav Arcの同一性、item→unit位置と画像unit位置の派生索引を持たせ、
同じtokenで生成/失効/交換する。slice callerは内容比較fallbackを維持する。cold構築は一覧長に
比例してよいが、warm需要は画像予算に比例し、長い動画区間を一頁ずつ再探索しない。
単頁も同じ索引へ通し、current partner・Navigation画像予算・supplement・距離順を維持する。
単頁の通常compositionもON対象で同じunit cacheを準備し、outgoingのpaint後に先読みだけで
layout Arcを置き換えない。単頁unitの形はlandscape/rotationに依存しないため、寸法harvestや
rotation再照合でそのtokenを失効させず、Spreadへ切り替えた時に最新metadataから再構築する。
大きな動画区間でnav内容比較/位置探索/unit訪問を数える回帰と、CPU退去後のcapture/注釈を追加する。
新owner/pending/queueを作らず、OFF・連結・Remoteの表示仕様や保存設定を変えない。

§1.242はこのworktreeへ未統合。ここでは統合検証を行わず、**§1.242統合後に明示指定
（Pair/Blank）・構成優先配置との検証が要る**ことを残件にする。fmt、normal/portable check、
関連回帰、test-full一回と通常build-devを行う。アプリ起動・コミットは行わない。

### 26.11 REVISE修正の検証記録

完成CPU画素/個別capture/注釈/別contextの5件、nav索引/大きな動画区間/通常Single paintと
harvestの3件が成功。既存paged_prefetch16、page_turn186、capture257、comic42件も成功した
（comicの既存2件ignored）。normal/portable core check、fmt/glyph、context監査も成功。
独立GPT-6.1 Sol/xhighの最終レビューはACCEPT、未解消P1/P2なし。

通常build-dev -PreserveRuntimeはcore/Remote/EPUBとruntime4 DLL・PE3 binary検査までexit0。
coreは2026-10-14 02:52:25、325,590,016 bytes、FileVersion4.6.0.0。portable featureを有効にせず、
アプリ起動・停止・コミットは行っていない。test-full -SuppressCrashDialogsはjobs1/threads8で
一回実行してexit0（PASS）。本体12,409成功/既存57ignored、UI snapshot163、vendorのegui25・
egui-wgpu11・eframe18も成功した。command/log・binary情報・隔離確認手順は
`target/A-1341-revise-msg.txt`を参照する。

### 26.12 2026-10-14 スライドショー期限と非フォーカスの受信入力

f33a93bb8で§26.10の修正はコミット済み。利用者がアニメーションONでもスライドショー・
非フォーカスのホイールでめくれないと報告した。隔離DBの現在の保存間隔は3.0秒、めくりは
200ms（現在の有効設定保存値はOFFであり、診断ログ時点のONとは区別する）。
通常スライドショーは旧頁期限に達してから次頁を受理するが、projection準備がその旧期限と
受理時刻＋めくり時間を比較していた。新頁のanchorを設定するのは描画後なので、次頁の
表示時間を判定したつもりで、既に過ぎた旧頁期限により毎回skipしていた。

§6/§10の「deadlineを延長しない」方針をここで置き換える。間隔がめくり時間未満なら従来の
瞬時切替とし、めくり時間を短縮せず、短い間隔の自動送りをめくり待ちにしない。
間隔以上なら通常のめくりを開始する。通常描画がOfferedをTurning/Flatへ解決してからtimerを
同期し、Turning/Completing中は既存slideshow_anchor_idxを未設定とする。採用済みcanonical flatになった後、既存schedulerが
保存間隔の全時間を数え始める。手動移動中のスライドショーにも同じ全面表示時間を適用する。
これにより新頁の表示時間から演出時間を差し引かず、ON時の周期は演出＋保存表示時間になる。
OFF/読込待ち/短間隔/連結読みは既存のready-anchor/continuous waitを使う。旧deadlineの
二重admissionを撤去し、新timer/queue/pending/boolを追加しない。

非フォーカスwheelは受信HWNDのWM_MOUSEWHEEL/HWHEELからwinitのWindowEventを経て、
対応viewport固有のegui raw eventとして届く。通常wheel handlerはfocusを要求しないのに、
page_turn_pre_drawはframe-final focus=falseで配送情報のrouting前にreturnしていた。
同levelによる毎frame取消も、非フォーカスで開始しためくりを次frameで止めていた。
minimizedは従来の取消を維持し、focusは実WindowFocused(false)のloss edgeで旧presentationを
一度だけ取消す。既存PointerTransaction/PointerDeliveryがraw ordinalと宛先を保持して
loss edgeもrouting済みとして記録し、replayが新しいwheelを再取消しない。focus edgeは
navigation候補・press/contact継承・navigation診断へ含めない。ROOTのlossでchildを取消さず、
受信元とpresentation viewportが同じ時だけ適用する。受信済みwheel/keyが同batchにある場合は
loss前後の順序によらず新しい離散配送として通常通り受理する。

native stamped keyの離散受理とcurrent-levelのfocus permitは従来のkeymap契約を維持する。
受理済みtyped continuationのcommitに新しいkeyboard permitを要求していた外側gateも撤去する。
close/modal/IME/keyboard ownerと、宛先・世代・layout・origin・canonical receipt・次frameの
既存commit検査は維持し、兄弟からraw eventを収集しない。左右クリック・middleの既存受付と
focus復帰クリック抑止、touch Start→Endのcontact/宛先継承は変更しない。横wheelはfactのみで
通常の頁送りをしない。Extra1/2・browser履歴移動は従来のfocus条件とinstantのまま。

利用者ログ36.197秒のUnattributedNavigation(5638)は、直前のfolder_history_backに対応する
戻る側ボタンのcontext移動であり、wheel頁送りの証拠ではない。19.889/20.155/20.247/20.616秒の
no_outgoing_paintはPointer IDが既に付いており、incoming resident素材確認後に現在unitの
採用済みFlat snapshotが無かった別の拒否理由である。PDFのretained miss/LoadPendingと
相方のthumbnail_not_loadedが直前にあり、表示合成のpaint未成立と整合する。ログにはfocusと
receiptの証拠がないため、四件とfocus不具合の因果は断定しない。素材/receipt待ちを増やさず、
adopt済みflatの無いケースを瞬時切替とする契約は維持する。

変更は既存表示時計・raw配送・context presentationの所有境界を揃える構造修正であり、
detached症状パッチではない。独立GPT-6.1 Sol/xhighの事前レビューで構造合意を得た。
期限経過からの実slideshow開始/完走・全面表示間隔、手動送り中の間隔、短間隔/OFF/連結と、
非フォーカスの実wheel・無入力frame・focusloss順序/replay・横wheel・兄弟宛先・continuationを回帰する。
§1.242統合後の残件は§26.10のままとし、今回も統合検証しない。

### 26.13 スライドショー・非フォーカス入力の検証記録

新規13件を含むpage_turn199、実input handler20、slideshow16、keyboard_input16が成功。
スライドショーはSingle/LTR/RTLの自動/手動送り、全面表示からの3秒、0.5秒<600msの瞬時切替、
OFF/連結の既存時計を検査した。非フォーカスはROOT/child×Single/LTR/RTLの開始/完走、
focusloss順序/replay、横wheelと兄弟source、実mountした兄弟の取消分離、受理済みPointer/
Slideshow/native stamped-Keyの採用flat後・permit消滅後の一回commitを検査した。
continuationの初回fixtureが通常のready通知を省略して失敗したため、productionと同じemitを
追加した。同frame待機・observer退役・採用receipt・次frame commitのgateは緩めていない。
normal/portable core check、fmt/glyph、viewer_context_auditも成功。独立GPT-6.1 Sol/xhighの
最終レビューは承認、未解消P1/P2なし。通常build-devはcore/Remote/EPUBとruntime4 DLL・
PE3 binary検査までexit0。coreは2026-10-14 04:18:14、325,590,016 bytes、FileVersion4.6.0.0。
対象プロセスの停止・アプリ起動・コミットは行っていない。前回f33a93bb8の修正を含む現ソースで
test-full -SuppressCrashDialogsをjobs1/threads8で一回実行しexit0（PASS）。本体12,422成功/
既存57ignored、UI snapshot163、vendor egui25・egui-wgpu11・eframe18も成功した。
コマンド/ログ/隔離確認手順は`target/A-1341-automatic-input-msg.txt`を参照する。

### 26.14 完成素材だけで表示するページの操作契約（2026-10-14）

f33a93bb8の再レビューは、PDFサムネイル寸法を注釈座標に使う経路と、Ctrl+Eが元画像
キャッシュ必須のまま残る経路をP2として指摘した。Retainedから先読みした完成素材は
元画像キャッシュなしでも正規の表示素材であり、Retained CPU LRU退去後もこの契約を保つ。
入口を個別に回避せず、表示素材・正準寸法・元画素をそれぞれ必要とする操作として揃える。

`PagePixelDimensions`でThumbnailとCanonicalを区別する。PDFの小さいサムネイルrasterは
形状判定だけに使い、正準座標へ昇格させない。PDF page boxも独立のlayout単位のままとする。
正準寸法は元画像取得時点の回転前寸法（GPU縮小前）から保持し、完成素材まで渡す。
AI出力寸法や縮小されたedit canvasの寸法から推測しない。正準寸法resolverを注釈・書き出し・
各種座標参照で共有する。Thumbnailの後着でCanonicalを上書きせず、items世代が違う値は使わない。

| 表示ページへの入口 | 必要素材・既存経路 |
| --- | --- |
| コピー、単頁／見開き／領域キャプチャ | 完成合成CPU画素を直接取得。元画素の再ロード不要。 |
| 注釈の表示・焼込み・テキスト／吹き出し編集 | 完成素材と正準寸法。サムネイル寸法を座標に使わない。 |
| Ctrl+E／SNS書き出し | UIは正準寸法と編集snapshot。元ファイルのdecodeは既存書き出しworker。 |
| 保存消しゴム／局所補正を伴う書き出しの準備 | 従来のcommit／layer準備gateを維持。不足する元画素は通常ロード。§1.177の準備worker化は別件。 |
| 外部ツールへの加工画像転送 | 正準寸法とsnapshot、既存materializer workerがdecode。 |
| メタ情報表示 | 既存metadata worker。画像キャッシュを要求しない。 |
| AI設定変更・再合成 | 既存最終合成pipeline。再生成に元画素が必要なら既存source loaderへ要求。完成素材を元画像として代用しない。 |
| 消しゴム入場／保存マスク再適用、FsApplyErase1/2（F7/F8） | 元画素が必要。入場・slot適用要求をロードpurposeへ保持し、取得後に同じ入口を一度実行。 |
| FsApplyConceal1/2（F9/F10）、マスク削除 | 保存済みslot／設定の変更は元画素不要。変更後の再合成に必要な原素材だけを既存pipelineから要求。 |
| 隠蔽加工入場 | 現在keyの局所補正／消しゴム結果があればその編集段を使用。完成表示を編集入力に代用せず、不足時だけ元画素を取得して従来の編集段とmaterializerへ接続。 |
| 局所補正入場・再計算 | 現在keyの消しゴム結果、または原素材が入力。段の素材が不足する場合だけ元画素を取得。単回入場のintentと既存layer materializationを維持。 |
| 消しゴム／隠蔽／局所補正の左右頁切替 | 切替先の原素材・必要な編集段、隠蔽はマスク取得も完了するまで旧mode／target／pair／maskを保持。既存source／inpaint／materializerのpurposeへtyped switch intentを渡し、準備完了時だけ旧頁を確定して一度切替。 |
| 回転 | 既存rotation設定とtransform。完成素材で操作可能。 |
| 手動クロップ、SNS分割 | 正準寸法と完成表示。元画素の再ロード不要。 |
| 自動クロップ、余白fit／自動表示trim | 検出は元画素。常時Autoは取得後に通常描画で再評価。明示「自動検出」／Auto→Manualの頁seedは同purposeに操作を保持し、見開き両頁が揃ってから一度確定。未取得を「余白なし」やゼロseedとしない。手動bboxは保存値から解決し、元画素不要。 |
| 原画像preview、分析・色取得 | 元画素。既存mode／キー状態のまま取得を要求し、完成画像を原画像として提示しない。 |
| 比較wipe／差分、pan／zoom／navigator、ルーペ | 完成素材と正準寸法。原画像比較を選んだ場合だけ元画素が必要。 |
| ドラッグ書き出し | 現実装はgridの既存file pathのnative drag。表示合成の画素をドラッグ書き出す別入口はない。仮想頁の既存拒否契約を維持。 |
| PDF指定倍率・最大倍率 | page box／content typeと正準寸法を使う既存倍率policy。元画像キャッシュの生存を寸法条件にしない。 |

元画素が必要な操作は既存`FsLoadPurpose`へOriginalと開始操作を加え、同じfs pending／
upload backlog／schedulerで所有する。別pending、queue、boolは増やさない。単にロードして
returnすると再クリックが必要になるため、入場intentは要求時のnavigation anchorとともに保持する。
末尾＋補助表紙ではanchorと実source targetが異なる。完了時は同context／世代、現在anchorと
再解決した編集targetを確認してから元の入口を実行する。navigation／close／cancel／別入口選択で
古いintentを適用しない。同じidxの不足要求は既存要求へ合流し、workerを重複発行しない。

左右切替のintentはfrom／target／元pairを一つの型で保持し、先にtargetを切り替えてから
準備待ちにしない。消しゴムで旧マスクの保存先だけが新頁になる経路や、隠蔽のmodeを先に
落として通常reconcileが旧pivotへ戻る経路を排除する。必要なmaterializerまで同intentを渡し、
全準備完了時に同じpassで旧頁の確定／新頁へのpivot／編集素材の採用を行う。失敗・取消は
旧編集状態を保つ。同keyの通常Display要求が専用Switchを置換せず、mode終了・同頁再入でも
古いSwitchを復活させない。Text／手動Crop／SNSの左右切替は正準寸法で進む既存経路を維持する。

materializerへの通常Display合流では、worker返信の素材identityと操作継続を既存のtyped
request内で区別する。items世代・投入時input_seq・source key・output contractを照合し、
待機中の操作で変わるglobal input_seqそのものは採用条件にしない。取消は通常Displayの
採用契約へ戻す。隠蔽のDisplay合成と編集mask取得は別output contractなので、同素材でも
一つの返信へ読み替えず、通常合成の採用後に専用mask準備を要求する。通常採用の失効契約を
維持し、cold Switchの素材準備だけ完成表示を保つ。

保存消しゴムがあり中間結果も退去している場合、原素材の取得だけでは局所補正／隠蔽の
編集入力は揃わない。既存inpaint jobのpurposeをPreview／Commit／素材準備と単回入口の
継続へ型付けし、同じCommit laneで保存マスクから中間段を再準備する。保存マスクの取得も
既存workerへ渡し、入口へ同期DB読取を追加しない。現在keyの中間段があれば再生成しない。
自発の素材再準備は完成表示・注釈素材を保持し、既に通常Commitが進行している場合はその
元の世代更新／invalidation契約を保持して操作だけ合流する。採用後は同じcontext／anchor／
編集targetと操作の許可を確認して一度入場する。別操作・navigation・close・取消で継続を
退役させる。これは新pendingや再クリック待ちを加える案より状態の組合せを少なくする。
独立レビューは既存source／inpaint owner内の型付けとして構造合意した。

Original取得は同sourceのmaterializationであり、reloadや編集変更ではない。成功／失敗で
完成合成・注釈素材・表示keyを破棄せず、実編集の既存invalidationだけを後から行う。
既に通常Display／Prefetchのreloadが進行している場合は、その既存要求へ操作intentだけを
合流する。開始済みworkerのrender条件は変えられないため、通常reloadの採用・世代更新契約を
Original materializationへ読み替えない。自分で発行した正準寸法でのOriginal取得だけを
同sourceのmaterializationとして扱う。各操作の完了受付は元の入口に合わせ、表示トリムや
閲覧中のslot適用へpixel編集mode専用の連結／独立window制限を拡張しない。
表示トリムの正規化bboxは従来どおりRAW preview画素でも検出できる。これを消しゴムなどの
正準画素編集許可へ読み替えない。見開きの取得完了callbackは同frameの全source採用後に行い、
既にupload batchにある相方を未要求として重複ロードしない。
PDFの通常display rerender demandでOriginal要求を取消・置換しない。RAW previewを編集許可へ
昇格せず、RAWの既存develop gate、動画／animation、Remoteの既存契約は維持する。
独立GPT-6.1 Sol/xhighの事前レビューはこの既存owner拡張を構造修正として合意した。
失敗／取消では既存sourceロードの終端を利用する。未取消の受信切断も取得失敗として一度通知し、
原画像preview／Auto表示から毎frameの再取得を繰り返さない。明示取消は通知せず、完成素材を保持する。
Original取得中に変わったPDF absolute targetは実要求が可能になるまで発行済みとして記録しない。
§1.242統合後に明示指定・構成優先配置との検証が要る（今回も統合検証しない）。

### 26.15 完成素材の入口整合・検証記録

通常／portable core check、fmt、glyph、viewer_context_auditが成功。元素材20、実操作入口25、
完成CPU画素5、寸法13、先読み16、page_turn199、表示trim55の関連回帰が成功した。
PDF thumbnail 160×240／正準1200×1800／edit canvas 4×6／完成8×12を分離し、
実source capture→Retained→完成worker→CPU退去→注釈表示／export／captureまで検査した。
Ctrl+Eは実dialog入口、外部ツールは実snapshot準備、native dragは既存path選択入口まで確認。
原素材の単回操作、通常ロードへの合流、失敗／切断／取消、左右の完了順・同時完了、
設定変更、既存RAW preview／表示trimとslot適用、実context mount往復の所有分離も回帰した。

検証中に、AppTestEnvのprofile排他lockを二重取得するfixtureを修正した。二contextは
一つのAppから実bundleをbuild／mountし、独立ケースはfixtureを順に解放する。
表示readyの旧テストも、完成合成が残る状態をNeedsLoadとする旧前提を更新し、
元画素が無くてもResidentFinalCompositeであることと、めくり開始／完走を維持して検査した。
独立GPT-6.1 Sol/xhighの最終静的レビューはACCEPT、未解消P1/P2なし。
test-fullはjobs1／threads8で一回実行した。本体12,462成功／2失敗／57ignoredでexit101。
UI snapshot163とその他workspace targetは成功。vendor追加gateは本体失敗のため未実行。
局所補正のpivot fixtureが原素材も編集段の素材も持たない点と、隠蔽入場が現在の消しゴム結果を
持つ場合まで元画像取得を強制する点が見つかった。前者は実sourceをseedしてpivotのassertionを
維持し、後者は適切な編集段があればそれを使う契約へ修正した。完成表示のCPU素材は編集段と
異なるため、その代用は引き続き拒否する。Retained退去fixtureで局所補正／隠蔽への実入場と
不要な再ロードがないことの回帰を追加した。全体gateの再実行は今回行わない。

追加の独立レビューで左右切替の準備前pivotも見つけ、既存ownerのtyped Switchへ揃えた。
原素材／保存消しゴムstage／隠蔽maskの取得中、旧mode・idx・pair・編集gestureを保持し、
ready後の一回commit、失敗／取消／同頁再入、normal Display workerのready既着／input_seq進行後の
合流と取消を実handlerの5回帰で確認した。stage素材完成後にrawを再退去しても切替が成立し、
自発の再準備で完成表示・comicが保持されることも検査した。修正後の通常／portable checkと
glyph／fmt／viewer_context_audit、上記関連回帰に加え旧失敗2件、inpaint10、erase52、
local_adjust146、conceal80（既存1ignored）がすべて成功。最終の独立静的レビューもACCEPT、
未解消P1/P2なし。test-only fixtureのtuple／ItemsGenerationMap参照の型誤りも修正済み。
通常build-dev -PreserveRuntimeもexit0。core／Remote／EPUB workerを更新し、runtime4 DLL・
PE3 binary検査が成功。coreは2026-10-14 07:44:07、325,638,656 bytes、FileVersion4.6.0.0。
確認用coreの起動は検出されず、対象プロセスの停止・実アプリ起動・コミットは行っていない。
コマンド／ログ／隔離確認手順は`target/A-1341-material-msg.txt`を参照する。

### 26.16 素材要求前の作業判定と初回入場の取消（d10b1501f 再レビュー）

AI先読みがeffective AI・完成cacheの確認前にCPU入力を要求し、初回隠蔽入場の継続が
materializer段で統一取消から漏れていた。入口の素材要求は実行可能な作業を判定した後だけにし、
単回操作の取消はその操作を所有する全段へ接続する。別pending・queue・boolは増やさない。

| §26.14 の入口群 | 素材要求前の確認／取消の再点検 |
| --- | --- |
| AI先読み | ページ固有のeffective AI、既存target枚数／同時job予算、Retained AI、必要なPDF native入力、現在keyの完成composite、resident AI／pending／failed、保持済みの正確なedit canvas寸法でのサイズ上限を先に判定。fit rasterが上限超過でもnativeが対象なら従来のnative再描画を要求する。不明な寸法・分類を必要なAI作業で解決するときだけ通常sourceを要求。正準寸法やthumbnail／AI拡大後の寸法を入力サイズとして代用しない。 |
| final-effect先読み | 既存prepare帯・VRAM予算、colorize／LUT有効性、完成composite／pending、必要なAI結果を確認してからCPU入力を要求。完成AI画素があればその画素と保持済みsource_sizeだけで既存effect workerへ渡し、raw／editを再取得しない。AI入力が不明でAI結果待ちならAI producerへ準備を任せる。 |
| Retained PDFの通常描画 | Retained AIを入力としてeffect workerへ渡した後のNoneは既存非同期処理の待機。完成素材を作る同workerが入力を所有しているため、原画像要求へfallthroughしない。 |
| 局所補正の表示／Ctrl+E準備 | 現在の完成local stageや既知のinactive layerなら原画像を要求しない。原素材がある場合のmaskサイズ変更の再検査は従来どおり。未読layer／未完成stageだけ既存producerへ接続。 |
| 初回隠蔽入場・左右切替 | FS pending／upload、inpaint、Local materializer、Conceal mask materializerを統一取消へ接続。専用EnterConceal／EnterMode jobは取消・除去し、mask返信をDisplay合成へ読み替えない。既存通常Display合流のResumeだけはDisplayへ戻す。 |
| コピー／キャプチャ／注釈、Ctrl+E／SNS／外部転送、メタ情報、回転／手動crop／drag | 完成素材・正準寸法・既存workerで足りる入口の原画像要求を増やさない。明示別入口の選択は統一取消で旧初回入場を退役させる。 |
| 明示AI再生成、消しゴム／slot／局所補正／隠蔽、自動crop／trim、原画像preview／分析 | 作業と許可を確認して不足素材を要求する既存契約を維持。FS／inpaintの単回intentとmaterializerへ渡ったintentを同じ取消へ接続。失敗／cancel／navigation／closeの所有境界を保つ。 |

初回入場を別modeにする手で割込みの組合せを減らす案は、通常の別ツール選択を制限するため
採用しない。既存ownerの専用intentを取消するだけで組合せを減らす。AI予算ゼロでも対象になる
現在の見開き相方は既存仕様として保持し、全体の枚数ゼロだけで先読み処理を止めない。
Remoteとdetachedのviewport／host条件は変更しない。§1.242の統合検証は前節の残件のまま。

通常／portable core check、fmt、glyph、viewer_context_auditが成功した。作業判定14、
実操作入口27、Original20、final AI26、final effect9、local adjustment146、conceal82
（既存1ignored）、paged prefetch16、page turn199の関連テストが成功した。
取消回帰はLocal／Concealの両段×待機／返信到着済み×Text／Crop／分析／Ctrl+Eの16通りと、
表示anchorが実sourceと異なる場合、実context bundle往復を検査する。分析が必要とする
MaterialOnlyのOriginal要求は残し、旧隠蔽入場だけを退役させる。AI回帰はeffective無効、
完成Retained、入力サイズと正準／完成寸法の相違、予算ゼロ、native PDFのfit超過、
完成AIを使うcolorize／LUT、表示中のeffect待機で不要なsource要求がないことを検査した。
テスト中の正準寸法fixtureと分析用Original要求のassertionを修正し、製品契約を弱めず再実行した。
独立GPT-6.1 Sol/xhighの最終静的レビューはACCEPT、未解消P1/P2なし。

通常build-dev -PreserveRuntimeはexit0。core／Remote／EPUB workerとruntime4 DLL・PE3 binaryの
検査が成功した。coreは2026-10-14 08:42:25、325,643,264 bytes、FileVersion4.6.0.0。
確認用プロセスを停止せず、実アプリの起動・コミットも行っていない。

最後にtest-fullをjobs1／threads8、SuppressCrashDialogsで一回通し実行し、exit0／PASS。
workspace 60 targetは13,710成功／0失敗／64ignored、本体は12,489成功／0失敗／57ignored、
UI snapshot163件も成功した。vendor追加gateもegui25、egui-wgpu11、eframe18件がすべて成功。
合計63 target、13,764成功／0失敗／64ignoredで、前節の旧失敗2件も今回の全体実行で通過した。
全63 targetの結果は`target/A-1341-entry-r2-full-results.txt`、全ログは
`target/A-1341-entry-r2-full.log`、修正・コマンド・隔離確認手順は
`target/A-1341-entry-r2-msg.txt`に記録した。

### 26.17 master旧形式の本の構成との統合（c1539a922へのmerge）

§1.341の独立ACCEPT後、master `0d79f5077`を取り込む。ここで扱う§1.242はmasterにある旧形式であり、
別ブランチで進行する組の作り直し・新形式の統合検証とは区別する。
masterの構成提示許可を、既存のcontext内pagedフレーム準備と組み合わせる。Hidden／Checkingを
通常表示・めくりの素材選択で迂回せず、即時に全occurrenceが許可されたunitだけをcurl候補にする。
部分許可・待機・明示Blankは通常の許可済み描画へinstantで戻し、許可の後着で再offerしない。
これは既存の即ready・全unit受理契約の接続であり、新しいpermission ownerは作らない。

構成unitは通常描画と近傍需要で同じrole／slot resolverへ渡す。構成tokenの採用時に
既存cacheのprimary頁→明示unit位置と画像unit位置を作り、補助表紙はprimary位置を上書きしない。
各近傍で全layout／navを再探索しない。Single／Splitの通常一頁unitと、Spreadの明示Pair／Blank／
補助表紙を区別し、Automaticの両端配置実効ONや補助表紙設定で保存構成を作り直さない。

スライドショーは初回受付だけがoperation／offerを扱い、permission後着とbarrier完了はmasterの
同じ通常completionへ戻す。既存typed continuationへsliceとNext／FirstInBookを保持し、
grid cursor／範囲起点／scroll／deadlineを落とさない。Offered／Turning／Completing中の
deadline更新は既存presentation状態で抑え、canonical flat採用から表示間隔を数える。
別pending／queue／bool、別timerや再試行経路は増やさない。

masterの局所補正pivotは内容identityを保持する新型のまま、元素材入場／左右切替の共通受付へ
現在のpairを渡す。旧pivot型への変換でidentityを捨てない。HUDは構成本の編集制限と
通常本のアニメーション中の両端設定無効化を保持し、overflow本文の縦spacing=0と行高式を合わせる。
PNGは統合後の本体描画fixtureで再生成し、両側の画像の一方を採用しない。
設計担当の統合指示・実装premise確認と、独立GPT-6.1 Sol/xhighの構造レビューACCEPTに基づく。
自動検証とPNG目視の結果は本節へ追記する。新形式の§1.242統合後の明示指定・構成優先配置は、§26.20で共通builderへ接続した。

監査でmaster由来のpage-edit API 11件のA4明示登録漏れも検出した。全signature／本体が
MERGE_HEADと同一で、masterのallowlistには登録がないことを確認した。§1.372／§1.376の
History／Exitでのowner commit収集、context／target／documentを照合するproducer採用、
同keyのdraftと別keyを保護するpublication／queue失効を独立レビューで確認し、exact fingerprint
だけを理由付きで登録した。A4規則・除外範囲を広げず、既存ownerの実装も変更しない。

通常／portable core check、fmt、glyphとviewer_context_audit（単体36件＋実行）が成功した。
統合3、page_turn202、素材要求14、入口27、Original20、page_edit55、book_composition201、
overflow38（既存1ignored）、設定転送28、keymap169、registry88、UI snapshot177、
構成editor snapshot10件の関連検証が成功した。masterの追加fixtureではegui Context／KeyEdge
operation IDと準備済みsource／identity pivotの前提を現入口へ揃え、保存対象のassertionは維持した。
統合fixtureはON/OFF不変検査の後にONへ戻して近傍需要を検査する。

overflow描画fixtureの4件をUPDATE_SNAPSHOTSで再生成し、Root／高さ十分のRoot／Navigator／
アニメーションONをすべて目視した。製本行と紙めくり行、縦spacing=0、狭い画面でのscroll、
高さ十分の末尾到達・scrollbarなしを確認した。競合Rootの再生成SHA256は
`f81db21a31a95a6b50c724be84456352b7d5f00ad66bb088458143ae31b5744e`で、
stage2／stage3の元画像と異なる。baseline比較以外にも共通scroll_layout helperの高さ／到達検査を通過した。
VST3 SDKはmainのsource実体をこのworktreeへcopyし、masterのhostをCMake Releaseで再buildした。
旧host cacheをimportせず、現ソースidentity
`04e94b0d62c8d4eb0128a708aec1a70c7ecb2c7639d9899fc54f621098a869ad`の検査が成功した。

統合後のtest-fullをCARGO_BUILD_JOBS=1／RUST_TEST_THREADS=8で一回実行したが、exit101で未完了。
本体12974件はAppの既存stackサイズ上限違反の後、capture_regionの回帰で
STATUS_STACK_OVERFLOWとなり、全件の結果は得られなかった。別のmetadata purge journalと
大きいsubtract previewにも失敗行があり、同じbinaryでの単独再実行はいずれも成功した。
本体以外のworkspace60 targetは1247成功／0失敗／7ignored、vendor追加gateは未実行。
本体の子harnessの成功を親の完了と数えず、結果一覧ではINCOMPLETEと記録する。

master統合によるAppの肥大化に対し、既存analysis_hist_cacheの固定長配列tupleだけを
Option<Box<同じtuple>>へ移した。App／context bundleの所有・swap／無効化／dropとcache keyを
変えず、新しいowner／pending／boolは加えない。上限110000を緩めず、サイズは
111232から107720 bytesとなり、既存サイズ回帰が成功した。実context mount往復による
値・heap所有先保持の回帰を追加し、独立GPT-6.1 Sol/xhighレビューはACCEPT。
修正後は通常／portable check、fmt、capture1、heap cache1、分析22、registry89、page_turn202、
page_edit55、metadata journal1、subtract preview1とviewer_context_audit実行が成功した。
本体全体の修正後再実行はこの個別成功で代用せず、test-fullの未完了として残す。
初回全ログ・全target結果はtarget/A-1341-merge-r3-full-first.logと
 target/A-1341-merge-r3-full-first-results.txtに記録する。

通常build-dev -PreserveRuntimeはexit0。core／Remote／EPUB workerとruntime4／PE3検査が成功。core生成日時=2026-10-14 11:20:04、328720384 bytes、FileVersion=4.6.0.0。アプリの起動・停止、git add／commitは行っていない。test-fullの追加実行は行っておらず、初回未完了の記録を維持する。

### 26.18 merge再レビュー：通常許可受付と明示表紙の投影

0f65c08b3の独立レビューでP2二件を確認した。§26.17の直接offer回帰は通常handlerの
非同期許可受付を通らず、構成配置の回帰はcanonical投影まで検査していなかった。
通常handlerは実permission serviceを使い、必要unitの全occurrenceが現在の採用済み構成／
source identityと一致し、serviceが初期化済み・現在token・先行mutationなしの場合に限り、
同じ許可owner内で即許可する。Admissionの既存Running状態で先行mutationを追跡し、
未初期化・maintenance・旧token・不明source・Hiddenは従来のworker照合へ渡す。
即許可は新しい操作のcontext／世代／source／role／slot／sliceに対する証明であり、
textureや以前の表示episodeだけから復活させない。UIでI/O・全archive走査・待機を行わず、
別pending／queue／bool／ownerや、許可後着による再offerは追加しない。

明示構成の単ページをcacheへ写す境界で、既存SingletonCauseにExplicitCompositionを保持する。
Automaticの「先頭unpaired Navigationが表紙側にある」証明はAutomaticに限定し、明示単ページは
共通構成resolverのCenter／Leading／Trailing配置をcanonical正本にする。指定表紙が後方でも
先頭を自動表紙と推定しない。ROI／source／許可・通常canonical geometryの拒否は変更しない。
旧保存unitsに対する修正は由来の保持と証明分岐に限定する。next-bookの『組を保存しない』
構成がmasterへ入った後は、共通builderの結果から同じ由来・配置を判定する形へ統合し、
明示指定／構成優先配置／先頭と後方指定表紙／各handlerを再検証する。この接続と再検証は§26.20で行う。

構成のない本や既存許可済みunitを毎回Checkingへ落とさない実service＋通常navigation／
slideshow handler回帰、LR／RL・中央／側配置・先頭／後方指定表紙の実builder→通常描画→
canonical投影回帰を追加する。別mode制限やめくり後着再試行で組合せを減らす案は、
既存の通常ページ送りを制限するため採用せず、同じ許可ownerとlayout型で成立条件を表す。
設計担当の修正指示、実装premise確認と独立Sol/xhighの構造合意に基づく。
通常／portable core check、fmt、glyph0、diff checkとviewer_context_audit実行が成功した。
新しい実service handler3件、明示singleton投影1件、service境界1件が成功。
handlerはPDF／ZIP×Automatic／Composed×Single／Spreadで通常前進・後退・slideshowのcurlを確認し、
先行write待ちの通常着地・後着curlなし4caseと、無効な証明16caseを検査する。
明示投影はLTR／RTL×Center／Leading／Trailing×先頭／後方指定表紙×前進／後退の
実builder・通常描画から96 phase投影を検査した。page_turn206、book_composition202、
registry89、UI snapshot177、構成editor snapshot10件も成功。独立Sol/xhighの最終静的レビューはACCEPT。
通常build-dev -PreserveRuntimeはexit0。core／Remote／EPUB workerとruntime4／PE3検査が成功。core生成日時=2026-10-14 12:17:34、328766464 bytes、FileVersion=4.6.0.0。アプリ起動・停止、git add／commitなし。

最後のtest-fullはjobs1／threads1、SuppressCrashDialogsで一回通し実行し、exit101で完了。
workspace61 targetは14169成功／1失敗／64ignored、本体は12922成功／1失敗／57ignored。
失敗は今回未変更のvideo::native_window_pump::tests::hud_focus_claim_uses_registered_editor_and_preserves_presenter_return。
src/video/native_window_pump.rs:2012で使い捨てeditor子processのHWND通知が10秒以内に来ずTimeoutとなった。
同じpack-build-tools libtest binaryのexact単独確認でも同じ10.02秒の失敗を再現し、原因は未確定。
今回の二件の修正とnative/video経路を混ぜず、この失敗を設計担当へ引き継ぐ。timeoutやassertionは緩めない。
スクリプトが失敗後に省略したvendor3 gateを別commandで補足し、egui25／egui-wgpu11／eframe18成功。
合計64 targetは14223成功／1失敗／64ignored、全targetの完了結果を取得した。全体gateのPASSとはしない。
前回のApp size／capture stack overflow／metadata journal／subtract previewの各回帰は今回の全体内で成功。
今回追加した通常handler・投影・service境界の回帰も全体内で成功した。
全64 targetの一覧はtarget/A-1341-permission-r4-full-results.txt、通しログはtarget/A-1341-permission-r4-full.log、
単独失敗ログはtarget/A-1341-permission-r4-focus-isolated.log、vendorログは同prefixのvendor-*.log。
修正・検証・隔離確認手順はtarget/A-1341-permission-r4-msg.txtに記録した。
アプリ起動・停止、git add／commitは行っていない。

### 26.19 pitch-lab/masterの結合（2026-10-14）

§1.341は利用者の実機確認がすべてOKとの報告を受けた（利用者の観測）。
PDF／ZIP、先頭表紙・末尾、slideshow、非focus wheel、末尾の組でのコピー／Ctrl+E／
注釈／capture、「…」パネルを確認した。自動gateの結果とは区別する。
masterの音楽HarmNet・配色メニューと計算高パネル修正を取り込む。音楽パネルのpointer所有と
§1.341のoperation factは同じ入力受付に接続し、背面への入力抑止と出どころの記録を両立する。
履歴・keymapの両機能の記述、音楽配色の転送除外とページめくりの転送対象を保持する。
overflowは実本文の行間と高さ式を照合し、共通scroll_layout helperで十分な高さ／狭い高さ・
末尾到達を検査する。bodyの行間が0の場合に環境の行間を高さへ加算しない。
masterのmin_scrolled_height(0)を保持し、既定最小高64ptが狭い本文をはみ出す退行を防ぐ。
next-book統合後の構成優先配置等の接続（§26.18）は§26.20へ引き継いだ。
独立Sol/xhighの静的レビューはACCEPT。両stageの機能・文書と、ページめくりsubmissionの
17テスト／音楽overflowの20テストを保持し、全callerの引数整合を確認した。
通常／portable check、fmt、glyph0、diff checkとviewer_context_audit（単体36件＋実行）は成功。
App size1、page_turn206、overflow58（1ignored）、設定転送28、keymap169、構成202、
registry89、音楽UI82（2ignored）、配色6、低域6、music-core41、共通scroll helper3件が成功。
高さ回帰は4状態×3画面高×ambient spacing0／13の24条件で成功した。
UI snapshot177／構成editor snapshot10件も成功し、PNG更新は不要。
Root／高さ十分のRoot／音楽メニュー／狭い音楽HUD／timeline調表示のPNGを目視した。
test-fullはjobs1／threads4／SuppressCrashDialogsで一回実行し、exit101で終了した。
本体は12990成功／0失敗／59ignored。前回失敗したnative HWND focus回帰も今回は成功。
後続のbook_composition_editor_snapshots targetはfooter snapshot一件が進まず、
CPU時間もほぼ増えないまま15分の検査予算を超えた。identity／開始時刻を確認した
この検査processだけを16分7秒で終了し、--no-fail-fastで残りのworkspace targetを続行した。
利用者のcoreは停止していない。原因は未確定で、assertionや製品コードは変更していない。
同じpack-build-tools binaryの該当caseはexact単独で6.29秒成功したが、全体成功の代用にはしない。
workspace64 targetは完了サマリー分のみ14247成功／0 assertion失敗／66ignored、
構成editor一targetはINCOMPLETE（その完了件数は不明）。省略されたvendor3 gateを補足し、
egui25／egui-wgpu11／eframe18成功。合計67 target中66完了、14301成功／0 assertion失敗／66ignored。
全体gateは未完了とし、PASSとは記録しない。全target一覧はtarget/A-1341-pitch-merge-r5-full-results.txt、
通しログは同prefixのfull.log、単独確認はeditor-isolated.log。test-fullの二回目は実行していない。
通常build-dev -PreserveRuntimeはexit0。core／Remote／EPUB worker、runtime4／PE3の検査が成功。
coreは2026-10-14 15:37:21生成、329141248 bytes、FileVersion=4.6.0.0。
アプリ起動・停止、git add／commitは行っていない。競合マーカーは解消済みで、
indexのUUは設計担当のgit addまで残す。今回の解消方針・確認手順はtarget/A-1341-pitch-merge-r5-msg.txt。

### 26.20 next-book新形式とmasterの統合（2026-10-14）

§26.18／§26.19に残した接続を、B2 ACCEPT `4eb172a15` とmasterの統合で解消する。
組は保存せず、構成の方向・表紙on/off・並べ替え／hidden適用済みの実列・ページ指定を
通常の共通builderへ渡す。旧保存units用の組立て・unit位置map・ExplicitCompositionは撤去し、
通常描画・提示許可の需要・canonical投影・近傍先読みを同じ派生unit／role／slot resolverへ接続する。
navのimmutable Arc、unit位置索引、近傍画像unitだけを訪れるmasterの先読みを保持する。
Single／Splitはページ指定でpairへ変えず、一頁ごとの通常topologyを使う。

共通builderのExplicitAloneは通常resolverの配置をcanonical正本にする。
中央固定は§26.21の利用者決定で置換し、表紙と同じ先頭配置規則を使う。
先頭のAloneをAutomaticの表紙と推定せず、AutomaticのUnpairedSlotには従来の
Navigation／anchor／cover側配置の証明を要求する。任意の後方表紙PageRefは新形式にはなく、
表紙on時の表紙は並べ替え・非表示適用後の先頭実ページである。
ExplicitBlankOppositeは読む方向から解く指定側と白紙を優先し、末尾の表紙添付を抑止する。
末尾Aloneは通常本と同じ末尾表紙の実効設定に従う。補助表紙はpresentation／先読みの
occurrenceに入り、Navigation実列・seek・位置／しおりのページ数を増やさない。
寸法・回転の後着による組替えも共通builderのcache条件で行い、旧明示Pairの固定前提を持たない。

構成投影はworkerで準備した回転をidentity別にitemsと同時採用し、新facet列を揃えてから
同じ許可ownerで需要を生成する。cold memoでもUIから回転DBを読まないB2の境界を保持する。
通常navigation／slideshowの実service即許可と、Checking／Hidden・先行mutation後着で
curlを再offerしないmasterの契約を維持する。構成のmode／flow保存・直接draft変更は
§1.373の共通後続mutation受付を通り、no-opや拒否ではreceiptを失効させない。
回転全リセットも同じ受付から、current／parked構成のidentity別入力を明示Noneへ更新する。
§1.375の物理行signature・Stack対応表・context別購読／dirty／cutoffは組の派生結果から独立して保持する。

新形式の回帰は、LTR／RTL・表紙on/off・順序／hidden後の先頭Aloneの実builder→通常描画→
canonical投影、末尾Alone／白紙両側と表紙添付on/offのpresentation／許可需要／先読み、
寸法・回転後着による再ペア、実serviceの通常前進／後退／slideshowを検査する。
旧形式の96 phase等の検証記録は履歴として保持し、新形式の証拠と混同しない。
検証結果は以下。実アプリは起動せず、git add／commitは設計担当が行う。

追加のcontext監査では、master既承認のFolderObservation／ALL22と、Append観測を指定AtRest
contextへ届ける二関数のexact API登録漏れを検出した。registryの各signature／本体が
MERGE_HEADと同一であることとowner／folder限定を独立確認し、A4の四対象だけを登録した。
A6は親でcfg(test)となる二つのテストファイルを単独走査するため、out-of-lineファイルと
include先の該当関数に同じtest限定を明示した。監査規則・除外範囲は変更しない。
統合接点と追加登録の独立静的レビューはいずれもACCEPT。自動検証は実装担当が行う。

#### 統合差分の自動検証（2026-10-14）

基点はHEAD `4eb172a15`、取り込み対象はmaster `d3f2e913c`、検証ownerは実装担当。
ログは`target/nextbook-master-*`へ保存し、過去の§26.17〜§26.19の結果と区別する。

| 検査 | 結果／証跡 |
| --- | --- |
| fmt／normal／portable check | すべてexit 0。fmt.log、check-final.log、check-portable.log |
| 構成／投影採用 | book_composition 209、b2_ 6成功。related-book_composition.log、related-b2_.log |
| ページめくり／先読み | page_turn 207、paged_prefetch 16成功。初回焦点のmerge_master 5件も成功。related-page_turn.log、related-paged_prefetch.log、page-turn-merge.log |
| Undo／Delete／ZIP更新 | section1373 76、section1375 36、section1353 22、facet_navigation 21成功。各relatedログ |
| App格納 | 既存サイズ上限の回帰1成功。related-history_transition_storage_keeps_app_stack_footprint_bounded.log |
| 機械監査 | viewer_context_audit実行成功・単体36成功。後続mutation受付監査PASS、UI glyph 0、diff check成功。context-audit.log、context-audit-tests.log、mutation-audit.log、glyph.log |
| UI snapshot | 通常UI177／構成editor10成功、PNG更新なし。editorの画像下のラベル帯とoverflowを目視。ui-snapshots.log、editor-snapshots.log |
| test-full | jobs1／threads4／SuppressCrashDialogsで一回だけ実行、PASS、exit 0。workspace・補助bin・vendor4まで完了。本体13,165成功／0失敗／59既存ignored、vendor egui25／egui-winit29／egui-wgpu11／eframe18成功。full.log、full-summary.log、full.exit |
| 独立静的レビュー | 共通builder／canonical／先読み／回転・facet採用／Undo・Delete接点とA4登録を限定確認し、ACCEPT。実行検証は上記実装担当の結果 |

通常build-dev `-PreserveRuntime`はexit 0。core／Remote／EPUB workerとruntime4／PE3検査が成功。
core生成日時は2026-10-14 17:47:52、329,547,776 bytes、FileVersion 4.6.0.0。
証跡はbuild-dev.log／build-dev.exit。アプリの起動・停止は行っていない。
起動中だったこのworktreeのcore 6プロセスは停止していない。
ビルド直前の再確認では起動中core／Remoteなし。ビルドは`-PreserveRuntime`で停止を禁止する。
競合マーカーは除去済み。indexのUUは設計担当がgit addするまで残る。

### 26.21 Aloneの表紙配置と白い相方のページめくり（2026-10-14）

利用者の実機観測: 本の途中のAloneはONでも中央、BlankOppositeでは瞬時切替。
以下は利用者の明示決定であり、§26.3／§26.4／§26.6／§26.7に記録したwhiteの拒否と
§26.20のAlone中央固定を置換する。これらの過去の検証結果は履歴として保持する。

**Alone**: 組み直しの起点を表紙（先頭の単ページ）と同じ性質として配置する。
本来の側はLTRなら画面右、RTLなら画面左。cover on/offや途中の位置から別の側を推測しない。
表紙なしでも読む方向だけから表紙ありと同じ側を決める。連続Aloneでは毎ページ同じ側を使い、
その直後の無指定ページから通常pairを再開する。末尾Aloneも同じ規則を使う。
local paged見開きのアニメONでは既存§26.2の実効first=trueを共通resolverへ渡す。
OFFではsingleton_spread_first_enabledと本別先頭preferenceの実効値に従い、保存値は変更しない。
先頭・途中・末尾の縦長Aloneすべて同じ。末尾endpointでは上書きしない。横長単独は中央。
末尾表紙が適格で添付されれば既存Navigation-firstの実2頁presentationを優先し、単独配置では扱わない。
明示Aloneの由来をAutomaticのUnpairedSlot／表紙証明へ混ぜず、共通builderの形成理由を使う。
横長AloneはLandscapeBoundaryの中央経路へ通す。通常本にはAlone指定がなく配置の挙動を変えない。
連結／Remoteは保存済みの先頭実効設定を使い、アニメONのlocal強制は従来どおり適用しない。

**白い相方**: BlankOpposite／自動after-cover・last・forced-boundaryの白面は
普通の見開きと同じ物理paperとして前後にめくる。末尾の明示白紙も含める。
通常paintと同じColor32::WHITEをPageMaterial::Blankで塗り、既存default texture／WHITE_UVを使う。
白紙用のtexture／decode／upload／source request／navigation indexを生成しない。
現行canonical_unitはすでに白い面をoccurrence=NoneのBlankとして投影しているので、この一経路を採用する。
stage_one_shape_reasonの白紙一律拒否を撤去し、白紙つきspreadが実Navigation一枚／補助表紙なしで
通常の白rectを持つことを同じ入出両方の検証で確認する。白紙は紙として遮蔽するがvisibility／receiptを作らない。
Flat・curlの着地・終端通常表示は同じ白rect／clip／colorを使い、黒いVacantとの区別を維持する。
末尾明示白紙のfinal cover抑止は変更しない。split_same_source_step／split_partial_leaf等の
他のinstant理由、素材が未完成のときの通常提示契約、許可・完成素材・兄弟contextの所有も変更しない。

新しいworker／queue／stateは不要。通常resolverの配置と既存のBlank materialへ集約し、
別の「白紙アニメ」分岐や専用素材を作らない。まれな失敗は既定方針で扱い、復旧の状態機械を足さない。

回帰: Aloneの中央／片側×LTR／RTL×ON／OFF（全体／本別、先頭／途中／末尾、横長、添付表紙）、
明示白紙のLeading／Trailing×LTR／RTL×前進／後退と末尾、構成なし本の白い相方、
白いmaterial／UV・遮蔽・実頁だけのreceipt・終端paint一致、trim後のclipを検査する。
実handlerと共通builderの回帰、白い紙の中間pose snapshot、通常UI snapshot、fmt／通常・portable check、
関連テスト／test-full一回／build-devで検証し、エージェントはアプリ起動・停止・commitを行わない。

#### 追加決定の実装と焦点検証（86b06c232からの未コミット差分）

共通builderの五つのoverride受付で横長AloneをLandscapeBoundaryへ通し、
共通resolverでは末尾表紙の二枚化を先に解決してから、残るExplicitAloneへ先頭配置を適用した。
local pagedの実効endpoint入力を一つのhelperへ集約し、facet外の表示中Aloneも同じ入力を使う。
そのページの横長判定はcache／worker採用済みのidentity別回転と既知寸法だけを使い、DBを読まない。
whiteの入出・保持画像・matching検証を同じ形資格へ通し、通常paintとcanonicalの白clipを一致させた。
productionのmesh／shade／visibility rendererは変更せず、既存Blank materialを採用する。
限定独立静的レビューは指摘なし。自動検証ownerは実装担当。

| 焦点gate | 結果／ログ（target/alone-white-） |
| --- | --- |
| fmt／通常・portable core check／glyph | exit 0、危険glyph 0件。fmt.log、check.log、check-portable.log、glyph.log |
| Alone全体／本別・facet外 | 設定matrix 1、採用済み回転／facet外1成功。related-alone.log、related-facet.log |
| 実builder→paint→canonical／白紙前後進 | merge_master 6成功。明示白紙はLTR／RTL・表紙on/off・両側・先頭／末尾・両方向、素材とreceiptは実頁だけ。related-merge.log |
| 白紙の投影／自動白紙／trim clip | 3成功。投影は8shape×4route×97phase、WHITE_UV・実occurrenceのみ・終端面積の一致を検査。related-white.log |
| 関連広域 | page_turn 211、本の構成212成功。related-page-turn.log、related-composition.log |
| 白い紙のsnapshot | 4routeの新規PNG4枚成功、目視済み。終端を通常primitiveへ置換した画像との完全一致も確認。snapshot-update.log、通常比較はrelated-page-turn.log |
| 通常UI／editor snapshot | 177／11成功、既存PNG変更なし。ui-snapshots.log、editor-snapshots.log |

新規dense testで当初置いた「可視面積はcanonical面積以下」は、遠近投影で拡大する到着leafには
成立しないと確認した。後退pose0.8125では194814.3125対194400となり、既存逆方向leaf回帰と同じ
投影面積を使う必要があった。検査を投影surface／leaf印刷域以下へ修正し、重複計上・白紙のreceipt混入・
終端一致の検査を維持した。renderer本体を緩めていない。証跡はvisibility-diagnostic.log。

#### 全体gateと確認用build（今回差分、2026-10-14）

`test-full.ps1 -SuppressCrashDialogs`をjobs1／threads4で一回実行し、exit 101。
本体は13,174成功／0失敗／59既存ignored、その他workspace targetも成功した。
唯一の失敗targetはeditor snapshot。最後の200%確認／Blockedケース中にプロセスが
0xc0000005（STATUS_ACCESS_VIOLATION）で異常終了し、Rust assertion失敗は記録されなかった。
初回の異常終了の原因は未確定であり、全体scriptをPASSとは扱わない。full.log／full.exit。

同じfeatureのtargetをthreads1で補完し11件成功（full-editor-supplement.log）。
feature解決を含め初回と同一であることを確かめるため、失敗した同一バイナリ
`book_composition_editor_snapshots-16a4793e4faef824.exe`もthreads1／process-local WER抑止で
直接実行し11件成功、exit 0（full-editor-exact.log／full-editor-exact.exit）。
PNG更新・製品コード変更で異常終了を回避したものではない。全体scriptの再実行はしていない。
workspace失敗で未実行だったvendor4をscriptと同じ引数で補完し、
egui25／egui-winit29／egui-wgpu11／eframe18が成功（vendor-*.log）。
初回と補完を合わせ、要求された全gate targetの成功結果を揃えた。並列実行時の異常終了記録は保持する。

確認用buildは`build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0`を実行し、exit 1。
このworktreeのdev-runtime coreが6プロセス起動中で、PID3932のguardで停止防止のため中断した。
アプリ起動・停止、ランタイムexeの更新、git add／commitは行っていない。build-dev.log／build-dev.exit。
前のコメントの未起動報告はプロセス出力の確認不足だったため訂正し、JSONで稼働を確認した。
現在のdev-runtimeはこの差分の検証用binaryではない。

利用者が確認用coreを閉じた後、`scripts/build-dev.ps1 -PreserveRuntime`でビルドし、
隔離`--data-dir G:\miv-iso\book\data`で起動して`G:\miv-iso\media`のbook_zip.zip／book_pdf.pdfを確認する。
先頭設定を中央にしたAloneをONでLTR右／RTL左、OFFで中央へ戻す。先頭設定を片側へ変えたOFF、
横長の中央も確認する。明示白紙の前後進／末尾白紙、構成なしの表紙次／最終単独の白面も同じくめくる。
自動末尾白面を見る場合は末尾表紙をOFFにする。明示末尾白面は末尾表紙ONでも表紙を添えない。
引数なしのnormal coreは実APPDATAを使うので、通常起動時はinstalled／常駐版を閉じ、実設定／データの更新に注意する。

### 26.22 表紙なしAloneの実機報告の追跡（2026-10-14）

327d67b8b／f04fbfbc1後、利用者からLTR・アニメONでもAloneが中央に見えるとの報告を受けた。
指定された一時JSONは取得時点で存在しなかったため、隔離book_composition.dbをmode=roで読み、
schema 2の構成だけを`tests/fixtures/book_composition_coverless.json`へ記録した。利用者DBは変更していない。
40ページ、表紙なし、hidden raw 4／2、Alone raw 10／11／17、白紙raw 15 leading／16 trailing／18 trailing。
rawは0始まりで、画面のPage 11／12／18に対応し、投影後indexは8／9／15。
隔離設定はPaged・アニメON・先頭／末尾の片側設定OFF。PDFの対象ページは576×816の縦長で、回転の保存値はない。

新しい回帰は保存recordを実project_compositionへ通し、hidden適用後の実builderをwarmにしてから、
許可済みの通常render_fullscreen_viewport・画像primitiveを観測する。
縦長PDFのcached rasterをfixtureから供給し、存在しないfixture PDFへの再decode要求は既存probeで抑止する。
descriptorの側だけではなく、表示中のsource identity・transform・実mesh頂点の矩形とviewport中心からの位置を検査する。
連続Aloneの直後raw 12／13が通常Pairへ戻ることも確認する。
追加matrixは表紙on/off×LTR/RTL×アニメON/OFF×全体先頭on/off×本別FollowGlobal/Center/Place、
各raw 10／11／17と追加した末尾raw 39の192配置を検査する。末尾設定は片側へ置き、先頭規則との混同を検出する。
末尾表紙はmatrixだけOFFにし、元の保存構成ケースでは全体の既定ONを維持する。

現行コードのままで元の保存構成と192配置は成功した。表紙なしを理由に中央にする条件は確認できていない。
したがってこの追跡では製品コードを変更せず、「修正済み」とは扱わない。
実機で観測したページ名・表示モード・継続的な中央表示か一瞬かの確認を依頼した。
headlessのcached raster描画は実機観測を否定する証拠ではなく、native提示を含む再現条件の差は未確定。
設計上の配置規則は§26.21を維持する。推測による表紙条件の追加や別配置経路は作らない。

検証（target/coverless-*.log）: fmt／通常・portable core check／glyph成功、危険glyph 0件。
保存構成・実描画matrix 2件、ページめくり関連213件、本の構成関連212件、UI snapshot 177件成功。PNG変更なし。
test-fullは今回の指示どおり実行していない。build-devはPreserveRuntime付きで成功し、アプリ起動・停止・commitなし。
当初はcore 6プロセスの稼働を確認したが、ビルド直前の再確認では0件だった。
初回buildはPowerShell内のstderrリダイレクトがCargoの警告を終了エラー化して中断。
同じscriptを別PowerShellプロセスで直接呼び、外側でログを捕捉して正常完了した（build-dev.exit=0）。

### 26.23 指定直前の余りの末尾配置と見開きへの復帰（2026-10-14）

§26.22の実機問題は、利用者スクリーンショットと保存構成から設計担当がPage 15（raw14）と特定した。
このページはAloneではなく、次のPage 16（raw15）のBlankOpposite leading直前で余った単ページ。
現builderは相方のoverrideを見てUnpairedSlotを生成し、resolverが本の途中なので中央としていた。
前回のAlone raw10/11/17の回帰が成功したことと矛盾しない。

利用者決定Q30（案A）: 指定直前の余りを単独形成理由OverrideBoundaryRemainderで区別する。
本来の側は末尾単ページと同じLTR左／RTL右。local pagedアニメONは実効last=trueを渡し、
OFFはsingleton_spread_last_enabledと本別last preference。連結／Remoteは既存保存実効値を使う。
横長のcurrentは先行するLandscapeBoundaryのまま中央。Alone本体のLTR右／RTL左・先頭設定は維持。
segment末尾を本全体の末尾に数えず、末尾表紙添付やAutomaticの表紙証明には使わない。
通常本の空overrideで出る横長前LandscapeBoundary・非pairable前NonPairableBoundary・shift終端UnpairedSlotは
似た形の余りだが明示指定によるものではない。今回の決定を拡張せず、その配置・原因・cache条件を維持する。
builderの同じ境界走査（相方override検出と強制末尾segmentの終端）と共通resolverに集約し、
描画側にページ番号による判定を足さない。
回帰は保存構成raw14の実投影→builder→viewport→画像primitive、LTR/RTL×ON/OFF×全体/本別last、
直後の白紙・Alone本体・末尾・連続指定、横長と通常本の上記境界を含める。

利用者決定Q31は本構成計画§9.2に記録。一致する既存Spread要求をmode-onlyとして共通受付し、
復帰後の通常builder・許可・preparation・Flat/turn ownerをそのまま使用する。
旧形式のunits・移行・第二binding writerは追加しない。まれな失敗は既定方針で扱う。

#### Q30/Q31の実装・検証（2026-10-15、未コミット差分）

修正前の保存構成回帰はraw14のbuilder配置でCenterとLeftの不一致を検出した
（`target/boundary-baseline.log`）。修正後は実投影・共通builder・実viewportの画像primitiveまで検査。
raw14は方向×表紙×アニメ×全体last×本別lastの48ケースと横長4ケース、
Aloneは既存192ケースを維持する。通常本の類似境界は原因・中央配置の据置きを検査した。

| gate | 結果 / 証跡 |
| --- | --- |
| fmt / normal・portable core check | exit 0。`target/boundary-fmt.log`、`boundary-check.log`、`boundary-check-portable.log` |
| 本の構成 / ページめくり | 219 / 214成功、0失敗。`target/boundary-composition.log`、`boundary-page-turn.log` |
| 一致キー・Remote UI・Remote scope | 各target 1成功、0失敗。キーtarget内で4 binding×4キー、Remote UIはSingle/Split/同値からのmode-only・最新構成再検証・休眠shift保持を検査 |
| UI glyph | 危険glyph 0。`target/boundary-glyph.log` |
| UI snapshot | 181成功、0失敗。追加4 PNGの2/3/4/5、拒否される8/9の非表示、説明全文を目視。`target/boundary-snapshot.log` |

test-fullはjobs1／threads1／SuppressCrashDialogsで一回実行し、exit 101。
本体は13,185成功／1失敗／59既存ignored。唯一の失敗は
`video::native_window_pump::tests::hud_focus_claim_uses_registered_editor_and_preserves_presenter_return`で、
`native_window_pump.rs:2012`の子プロセスHWND通知待ち10秒がTimeoutになった。
同じ実行バイナリを単独実行しても0成功／1失敗、exit 101で同じTimeoutだった。
該当ソースはHEADから変更しておらず、原因は未確定。待機延長・テスト除外・native実装変更では回避しない。
証跡は`target/boundary-full.log`／`.exit`、`boundary-native-focus-exact.log`／`.exit`。

他のworkspace target・integration・doc-testsは成功。編集画面snapshot 11、UI snapshot 181も成功。
scriptが未実行にしたvendor4を同じ引数で補完し、egui25／egui-winit29／egui-wgpu11／eframe18が成功
（`target/boundary-vendor-*.log`／`.exit`）。全体scriptをPASSとは扱わず、native失敗の切り分けを残す。

`build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0`はexit 1。
runtime DLL検査4は成功したが、このworktreeのcoreが6プロセス
（PID62480／65936／77056／80208／92772／93344）起動中で、PID62480のguardによりCargo build前に中断した。
`target/boundary-build-dev.log`／`.exit`。新しい確認用binaryは作成していない。
実アプリの起動・停止、git add／commitは行っていない。

**利用者の実機確認**: nativeテストの失敗を切り分け、確認用coreとインストール済み／tray常駐mIVを閉じ、
`scripts/build-dev.ps1 -PreserveRuntime`の成功後、利用者が次を実行する。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe -ArgumentList '--data-dir G:\miv-iso\book\data'
```

引数なしの通常coreは実APPDATAのsettings/dataを使うため、今回は指定済み隔離data-dirで確認する。
本は`G:\miv-iso\media\book_pdf.pdf`、保存構成はLTR・表紙なしのまま使う。
アニメONのpaged見開きでPage 15を左、AloneのPage 11／12／18を右に置く。
OFFでは末尾の全体／本別設定でPage 15だけ中央／左を切り替え、Alone本体は先頭設定で別に決まることを確認する。
`1`の後に`2`で見開きへ復帰し、`3`〜`5`は`2`と構成編集の案内を出す。HUDは`[2]`、
Pad Selectは単ページ／一致見開き、Remoteの`2`も同じ復帰。方向／表紙を編集した場合は一致キーも3／4／5へ変わる。

### 26.24 Vacant片面を共通の黒paperでめくる（2026-10-15）

利用者は本の構成のあるbook_pdf.pdf、LTR・表紙なし・ON/600msで、9|10→Vacant|11の
10が90度で消え、後ろの9が突然消失／出現すると報告した（利用者の観測）。原因は§24.2の
半回転の仕様と、projectionのVacant leaf省略・旧static終盤fadeがその仕様を実装していたこと。
利用者決定により、片面Vacantを§26.1と同じ無textureの黒paperで普通にめくる。

裏がVacantなら旧10を180度までめくり、後半は黒い裏面とする。旧static9はalpha1で保持し、
黒いleafが覆うまで見せる。表がVacantなら0度の黒い表から開始し、下の新static9をleafの
離陸によって露出させ、裏の新10を着地させる。LTR／RTL・前進／後退は同じslot対応を使う。
表紙境界、補助表紙、余り単ページ、明示Aloneもbuilderの由来で分岐せず同じVacant規則を消費する。
両面Vacantは既存instantを維持する。白いBlankOpposite／自動白紙の材質・構成・receiptは変更しない。
Blankと実頁／Blankの既存めくりを保ち、Blank×Vacant境界もVacant側だけ黒くする共通規則に従う。

黒い面の仮紙rectは、その面が覆う対向staticのrectを対応するspine座標へ平行移動したものを使う。
サイズ差でも下のstaticが最初から露出したり、着地直前に外周をfadeしたりしない。
実画像のrect／UVを変更せず、既存の角度による形状補間・moving spine・shadow・厚みを共用する。
Vacantのcanonical材質は空のまま保持する。効果中の黒paperは遮蔽だけを持ち、印刷領域の
可視面積／実頁receiptは空。新texture・decode・upload・素材要求・実occurrenceは作らない。
p=1は既存の通常incoming flatだけを返し、黒paper・旧static・影を残さない。
新しい状態／待機／ownerは足さず、既存leafとblack_paperを共用して組合せを減らす。

回帰は実際の本の構成projection→共通spread builder→通常flat採取→leaf投影を通す。
11頁・表紙なしの自然余り（LTRでは11|Vacant）と末頁Alone（LTRではVacant|11）を別に組み、
LTR／RTL×往復で検査する。報告の保存構成そのものを取得したとの扱いにはしない。
紙サイズが異なる純粋投影fixtureでも、静止9のrect／alpha1、離陸時の完全遮蔽、
着地直前の完全被覆、黒紙の実頁面積0、終端の通常canonical描画を検査する。
4方向のspread windows snapshotを再生成・目視し、終端と独立した通常描画との一致も検査する。

検証（2026-10-15）: 通常coreの`cargo check --locked -p mimageviewer --bin mimageviewer-core`、
page_turn関連216件、book_composition関連219件が成功。追加2回帰と既存正規化行列の単独実行、
snapshot更新後の再照合、fmt／diff検査も成功。独立静的レビューはACCEPT。
利用者指定によりtest-full／build-dev／アプリの起動・停止／git add・commitは行わない。

### 26.25 2018fb445実機報告のhandler経路調査（2026-10-15）

ClaudeCodeは隔離dataで構成ありPDFの9|10→Vacant|11と逆方向がinstantに見えると報告した。
先のbuilder回帰は直接prepare_leafを呼ぶため、通常navigationのoffer／出力採用の証明ではなかった。
実機での原因はこの観測だけから受付拒否と断定せず、受理後の取消・描画も分けて調査する。

隔離DBを読み取り専用で確認した実構成は40頁・LTR・表紙なし、raw2/4非表示、raw10/11/17がAlone、
raw15がLeading白紙、raw16/18がTrailing白紙。対象は末尾でなくitems[6,7]（9|10）とitems[8]（11）の中間境界。
保存値はON/600ms/gap4で、対象頁に保存回転・trimはない。提供先の通常ログ現行／直前世代には
page-turnのskip行がなく、perf記録も見つからない。従来の通常ログはskipだけなのでstart／cancelは証明できない。

回帰は実permission serviceへ保存／prepareしたこの構成を採用し、素材residentの通常flatから
handle_fs_navigation_with_operationへページ送り要求を渡す。LTR／RTL・往復、通常窓／縦長領域・native DPI1.5、
元画像なしでResidentFinalCompositeだけの条件も含め、Offered、curl提出のStarted、中盤25/75%の継続、
通常flat提出のFinished、skip／cancelなしを検査する。旧11頁末尾Aloneの小fixtureも対照として残す。
試験のDPIはRawInputのnative_pixels_per_pointで渡す。set_pixels_per_pointは次passのzoom変更なので、
初回rectが6666×6666、次passが指定rectとなるfixture由来のdestination_clip_changedを起こした。
この失敗に合わせて本番のclip／DPI gateを緩和しない。

実機で次の観測を判別できるよう、既存emitの通常ログをskipに加えてstart／cancel／finishにも各1行出す。
frame／first_visible等は従来どおりperfのみ。既存のoperation/context相関と理由を使い、新しい状態／素材／receiptは足さない。
投影・受理・offer matchingを変更する原因は、現段階では未確定。実機の開始・取消・完了の記録と照合して判断する。

追加handler回帰は全条件でStarted→中盤→Finishedに成功し、skip／cancelなし。
通常core checkとpage_turn関連217件も成功した。これは実機instantの原因を再現したとの証拠ではなく、
機能修正済みとは扱わない。2018fb445のperf記録、または追加lifecycleログを含む実機記録が残件。
book_composition関連219件、fmt／diff検査も成功。独立静的レビューはACCEPT（実機の原因確定を意味しない）。
指定どおりtest-full／build-dev／アプリ起動・停止／git add・commitは未実施。


### 26.26 実機観測の訂正と最終描画の照合（2026-10-15）

ClaudeCodeは73e4415cbの隔離実機確認について、往復とも約600msでstart→finishしており、
「instantだった」とした§26.25の観測を訂正した。新たな観測は、前進では旧9もめくる10も
見えず、Vacant|11の完成形に見える一方、後退では10の着地が見えるというもの。
受付拒否や90度での打切りと同一の原因だとは扱わない。

提供perfの前進3回はfrontの30→約53度、backの約106→157→180度を記録し、
incoming11の可視率も初期約9%→約36%→100%となる。canonicalと実効clipは全画面で、
旧9/10のnative User(3)/(4)は後退でも共用され、途中の再生成はない。
ただし従来のmesh統計・可視面積はprojectionの計算結果であり、最終描画やGPU提示の証明ではない。
特にfrontの期間は開始後約260msまでなので、撮影コマは開始時刻・frameと対応させる必要がある。
撮影の遅れが原因だったとは、この記録だけから断定しない。

追加回帰は保存構成を反映した40頁fixtureと実permission/navigation handlerから、MainWindowの
render_fullscreen_viewport、FullOutput、実tessellationまで通す。LTR/RTLの往復の10/25/60/75%で、
旧9/新11の静止rect・alpha、10のtextureと曲がったmesh、床→静止頁→leafの描画順、
最終三角形での被覆（後段のflatや黒面による上書きも含む）を検査する。
元画像cacheを退去させ、両表示単位を通常rendererで事前表示してから要求する。
画像は単色fixtureとし、寸法は実機のdecoded PDFと同じ932×1320を使う。
MainWindow fixtureはvideo_in_window_modeもONにする。presentation/native flagだけを直接設定すると、
通常openが設定に従ってFullscreenへ移し、destinationだけが不一致になるため、実機の条件と違っていた。
初期のbuilder回帰は投影DTO、前回のhandler回帰はcurl receiptと開始/完了までだったため、
この最終描画内容の検査を持っていなかった。

既存perf有効時に限り、projectionのmesh.paint_orderと、discard判定後のoutput_paintを同じframe/passに添える。
texture ID・rect・clip・UV範囲・alphaと順序を比較し、後段のrectやmeshによる上書きを確認できる。
二度目のtessellationや素材取得は行わず、新しい状態・texture・decode・要求・実頁receiptは足さない。
Vacant/白紙の投影規則、面積、素材保持、描画の挙動はこの調査では変更していない。
実機の根因は未確定であり、機能修正済みとは扱わない。

投影・描画を変更せず、追加回帰はLTR/RTLの往復で成功した。自動試験では前半の旧9/10、
後半の黒paperによる被覆、終端の通常flatが提出され、報告された前進の欠落は再現していない。
検査点には外縁1/2%も含める。75%時点の旧9の残りが約4.6%だったため、5%刻みだけでは
この細い帯を見落とした。これは試験の検査点の不足であり、実機不具合の原因とは扱わない。
このheadless回帰はnative GPUの画素内容・実App::update全体・画面キャプチャの時刻を証明しない。
残件は撮影コマと開始後のframe（例: 約150ms）の対応、および実機の最終output_paintの照合。
その証拠を得るまで、角度・素材・描画順を推測で変更しない。

検証（2026-10-15）: 追加回帰1件（内部でLTR/RTL×往復）、fmt、通常core check、
page_turn関連218件（既存snapshot比較を含む）、book_composition関連219件、diff検査が成功。
PNG変更はない。test-full/build-dev、アプリの起動・停止、git add/commitは指定どおり行っていない。

## 27. D1 — 高解像度の素材保持とフレーム停止の分離（2026-10-16）

**D2・D4実装済み・実機未確認（2026-10-16）。** D1文書は `81b6d8d38`、D1rは `cc4e775c1`。
利用者からD1rの独立設計再レビュー「指摘なし・ACCEPT」とD2実装の承認を受けた。
独立レビュー（GPT-6.1 Sol / xhigh、利用者から全文伝達）はREVISE、P1相当なし・P2が3件。
D2のsource/paint所有分離と、それがdetachedの症状パッチではなく構造修正である方向には同意を得た。
P2-1は本改訂でD2のManaged方針・単独受入条件へ反映し、P2-2/3はD3の未解決要件として残す。
当初調査の基点は `pageturn-material` /
`911d33427`（開始時clean）。v4.7.0で出荷済みの機能に対する改修案であり、未出荷機能として
既存の操作・表示品質を削らない。本節は素材所有・処理配置・時計の改訂案だけを扱う。
§1〜26のmotion、既定OFF、PC/F12限定、flat-first、最新の§26.12のslideshow契約は維持する。
とくに§15〜18のfinal-onlyや古いslideshow期限条件を現行仕様へ戻さない。

**利用者決定（2026-10-16）**: v4.7.1にはD2だけを入れる。02のカクつき対策D3a/D3bと時計変更D4は次の版へ送り、
D3bの実改修はegui 0.37更新後の再計測・再設計を経て行う。計装の仕様・更新前baseline採取は先行可だが、
D2出荷のためにD3の完了を要求しない。時計の原点は「最初に採用されたcurl提出」を採択済み（§27.6）。

### 27.1 調査範囲と証拠の限界

利用者の2026-10-16の報告は「03/04で省略、4800×7200の02でカクつく」。今回の実行時の根拠は
利用者の既存perfログの読取りであり、エージェントによるアプリ起動・画面観測は行っていない。
指定文書のほか、coding-conventions、git-workflow、development-build-and-test、detached-rework-plan §2、
現行の§22〜26と以下の所有・描画コードを照合した。

ログのディレクトリは `C:\Users\mikag\AppData\Roaming\mimageviewer\logs\`。原本を移動・削除・更新しない。
表のbytes/行数/hashは読取り時点のsnapshot範囲。現ログには調査中も追記があり、末尾再読取り時には
49行増えていたが、集計済み先頭15,393,771 bytesのhashは一致した。追加分にanimationイベントはない。
以下の行番号はJSONLの1始まり、`t`はsession内秒。別ファイルの時刻やoperation IDを混ぜない。

| 原本 | bytes / 行数 | session / SHA-256 |
| --- | --- | --- |
| `perf_events.jsonl` | 15,393,771 / 26,492 | `33064-1792109295164`、UI tid=99348 / `84b3709a34f333e7e3c3d202c0557073756de0686bdbb4bb3671af2a627bc7e6` |
| `perf_events.1.jsonl` | 24,827,426 / 55,250 | `69140-1792109089896` / `40e3e5d446f22b0b39652bc9163d7f672d1f03e33435fe471d3ae54cfacbf29d` |

集計用Pythonと全204 curl frameの相関CSVはリポジトリ外の
`%TEMP%\miv-pageturn-d1\`（`aggregate.py`、`budget.py`、`compact.py`、`finalstats.py`、`frames.csv`等）へ置いた。
各frameを `(session, context, window, viewport, operation_id)` でまとめ、同tidの
`frame.begin.prev_update_start_t/end_t` に重なるouter updateへ対応させた。
outer frame Nの時間は次の `frame.begin`（n=N+1）が報告する値を用いる。
`fs_viewport_breakdown` はnを持たないため、同tid・包含区間で対応させる。丸めたtの境界は約1msの誤差を持つ。
UIの親子区間は加算しない。別tidのthumb decodeやfs worker時間もUI時間へ加算しない。

既存 `analyze_perf.py ... page-turn-animation --check` と `... hitches --ms 150` も実行したが、
checkpoint/coverage証明、profile/run_id等がなく **CannotJudge**。
解析器には `flat_barrier` / `continuation_commit` 未対応もある。以下は保存済みイベントの集計であり、
ログ完全性、出荷性能gate、OS presentの合格を主張しない。
ログの実入力は `target\repro\page-turn-budget-571-20261016\` 内のZIP
（`02_portrait_4800x7200.zip`、`03_portrait_6144x8192.zip`、`04_extreme_8000x8192.zip`等）。
利用者の元の画像フォルダでの観測と、これらのZIPのログ上の結果は区別する。

### 27.2 観測された失敗とコード上の違反

#### A. pin_budgetは画面用出力ではなくsource込みの保持量で超える

`perf_events.jsonl` はopportunity=34、offered=25、start=23、finish=22、cancel=1、
curl frame=204。機会内skipはpin_budget=9とoffer_mismatch_or_expired=2。
別の14 skipはcontext移動6・境界7・continuation instant 1である。
`.1.jsonl` はanimation skip=23（`book_presentation_not_fully_allowed` 22、`rendition_or_burst` 1）、start/frame/pin_budgetは0。
したがって9件のpin_budgetは全て現ログの同一sessionに属する。

現上限はfullscreen予算10,124,210,995 bytesの1/10 = **1,012,421,099 bytes（965.520 MiB）**。
`page_turn_pin_limit`（`src/ui_fullscreen/page_turn.rs:764`）の値に一致する。全9件はaccept時の拒否で、
`prepared_incoming_roi`での拒否ではない。

| 対象 / operationのseq（診断値） | pin bytes / MiB | outgoing retain bytes | 出どころ |
| --- | --- | --- | --- |
| 03 / 68〜74、7件 | 1,101,735,408 / 1,050.697 | 564,864,504 | L21485〜22158、t=1403.513〜1407.098 |
| 04 / 79〜80、2件 | 1,428,267,016 / 1,362.102 | 729,216,848 | L23205/23326、t=1412.780/1413.864 |

`PaintedUnit::account`（同:716）は各pageのsource（完全mip込み）とnative outputの両方を数え、
`AcceptedSources::account`（同:667）はincoming sourceを数える。accept時点の総量は次の式で
**ログとバイト単位で一致**する。RGBA8、mipは各辺を整数半分・最小1まで足した実式
（`src/vram_budget.rs`の`texture_size_texels`）を用い、4/3近似は使っていない。

| 所有対象 | 03の1枚bytes × 枚数 | 04の1枚bytes × 枚数 |
| --- | --- | --- |
| outgoing source、mip込み | 268,435,452（6144×8192）×2 | 349,525,084（8000×8192）×2 |
| outgoing native output、1 mip | 13,996,800（1620×2160）×2 | 15,083,340（1919×1965）×2 |
| incoming accepted source、mip込み | 268,435,452×2 | 349,525,084×2 |
| 合計（この時点ではincoming native outputを含まない） | **1,101,735,408** | **1,428,267,016** |

代表の03はoutgoingが003/002頁、incomingが005/004頁（L21484 target=3、0始まり）。
直前のpaintにはoutgoing source `Managed(953)/(952)` がある（L21424/21425）。
04も同じ頁対応で、outgoing sourceは `Managed(1029)/(1028)`（L23176/23177）。
native寸法は `gpu/lanczos_regenerate`（03: L21410/21411、04: L23158/23159）から取った。
ただしskipイベント自身は全leaseのID一覧を持たないため、incomingの正確なManaged ID、native ID、
各参照元までをこのログから復元したとは言わない。上表はcodeと隣接イベントによる会計の再構成であり、
全9件の実lease inventoryの直接dumpではない。

同じ4面をnative出力だけで保持する参考値は03=55,987,200 bytes（53.394 MiB）、
04=60,333,360 bytes（57.538 MiB）。checker等があればその実allocationを加える。
これは改修後の実測ではない。通常cacheにあるsource分のVRAMまで消えるという意味でもない。
**上限を緩める問題ではなく、presentationが描かない原寸sourceまで所有する境界の問題**である。

#### B. 200msの演出に高解像度の通常準備が重なる

23 operation / 204 frame全体のmesh CPUは中央値0.0663ms、p95=0.1880ms、最大0.4362ms。
4000×6000群（seq44〜51）はframe数 `2,1,11,3,11,14,7,18`、最大間隔131.7448ms
（seq44のelapsed差、L17345から次curl frame）。
02（seq55〜62）は計15 frameで、以下の通り。間隔はイベントのtの丸め差でなくelapsed差から求める。

| seq / 初回frameの行 | curl数 | 初回elapsed ms | 最大curl間隔 ms | 初回を含むouter N / update ms | viewport closure / closure外 ms |
| --- | --- | --- | --- | --- | --- |
| 55 / L19447 | 1 | 79.513 | 次curlなし | 2831 / 239.376 | 57.282 / 162.989 |
| 56 / L19592 | 1 | 111.810 | 次curlなし | 2839 / 158.939 | 86.317 / 70.107 |
| 57 / L19729 | 1 | 124.911 | 次curlなし | 2850 / 190.246 | 119.696 / 68.050 |
| 58 / L19875 | 1 | 147.229 | 次curlなし | 2863 / 274.453 | 141.270 / 130.159 |
| 59 / L20008 | 1 | 105.209 | 次curlなし | 2874 / 186.416 | 83.288 / 100.364 |
| 60 / L20144 | 1 | 61.529 | 次curlなし | 2885 / 145.890 | 38.735 / 95.764 |
| 61 / L20265 | 1 | 111.416 | 次curlなし | 2894 / 192.765 | 93.928 / 96.018 |
| 62 / L20382 | 8 | 70.479 | 85.943 | 2903 / 130.387 | 53.415 / 65.877 |

02の通常原寸面は4800×7200、native面は1440×2160。
例外として最初のseq55の片面はaccept時thumbnail 526×789をDirectで使用している
（L19446のunit.incoming.pages）。「02は全素材が1440×2160」とはしない。
02だけのmesh中央値は0.0689ms。高解像度mesh化やsegment削減は今回の主対策にならない。

代表区間の内訳は次の通り。

- N2831（1393.775940〜1394.015316、次begin L19462）: update=239.376ms。
  viewport body=220.272msはclosure=57.282ms + `eframe_show_ms`=162.989ms。
  closure内setup=35.453ms、media=21.413ms（L19455）。同区間の通常final合成2件は
  23.359/15.079ms、内訳edit=7.780/0.015、post=7.472/7.557、load_texture側=8.083/7.496ms
  （L19443/19444）。poll_prefetchも2回、13.649/11.140ms、そのupload区間13.546/11.074ms
  （L19429/19441）。合成はclosureの子、pollは呼出位置によってbodyの子なので全部を再加算しない。
- N2863: update=274.453ms、body=271.430ms = closure141.270 + closure外130.159ms
  （L19878、次begin L19885）。通常final合成50.245 + 87.658ms（L19870/19871）がclosureの
  大半を説明する。post計76.931ms、load_texture側計52.589ms、edit計8.352ms。
- 初回入場付近のN2829は別に560.253ms（次begin L19365）。poll_prefetch=164.004msのうち
  同期補正92.227、auto_apply_saved_mask47.181、upload24.151ms（L19324）、
  viewport render=373.882ms（L19358）。これは演出開始前の入場費用として別集計する。
- N2832にもpoll 41.223/36.521ms、うちauto_apply_saved_mask31.542/30.226msがある
  （L19467/19471）。この段名だけで「DBのマスク読取が全時間の原因」とは断定しない。
  `auto_apply_saved_mask`はmask有無を調べる前にerase入力の解決・black flattenも行う
  （`src/ui_erase.rs:3194`、`:2977`）。分割計装が必要。
- 02の完了後には `prev_outside_ms` が約200〜264msのframeもある
  （例N2913、L20479:264.250ms）。これはupdate外で、curl計算へ足してはならない。
  同時期の別tidのthumbnail decodeがUIを直接実行している証拠はない。

コード上、`resolve_fs_processed_texture`（`src/ui_fullscreen.rs:12167`付近）は
**通常の副作用つきcache/ensureを先に実行し、その後でheld resourceへ差し替える**。
通常final build（`src/app.rs:85631`以降）はedit準備・post処理・全画素clone・load_textureを同期実行できる。
`page_turn_bind_prepared`も通常prepareの後で旧pageを戻すため、最終的に保持面を描くframeでも
その前の仕事は消えない。これが確認できた「表示の固定とproducer実行が分かれていない」境界である。
poll_prefetchのslotも呼出単位でcurrent + otherを取り出し（同:92239以降）、
rootとviewportの複数pollでouter frame全体の1枚上限にはなっていない。

**計測上の重要な区別**: `ctx.load_texture`は `vendor/egui/src/context.rs:2263` ではTextureManagerへの
allocation/delta登録である。実際の `queue.write_texture`、mipmap、buffers、surface acquire、submit、presentは
`vendor/egui-wgpu/src/winit.rs:590`以降、`renderer.rs:602`以降で実行される。
資料中の「load_texture = 同期GPU upload」という要約だけでは、今回のclosure外163msを説明し切れない。
`fs_viewport_breakdown.eframe_show_ms`はbodyからclosureを引いた値（`ui_fullscreen.rs:27893`）で、
GPU実行時間そのものではない。ここまでを確認済みとし、texture delta配送/mip/GPU待ち/present/
renderer lock等の個別寄与は **未確定**。低cyclesや巨大なt空白だけでdevice wait、pagingと診断しない。

#### C. 準備が時計を消費し、開始すら省かれる

`AcceptedOffer.accepted_at`は受理処理で作られ（`page_turn.rs:1928`）、
`:2050`で `now - accepted_at < duration` を開始条件にし、`:2164`の進行率も同じ原点を使う。
02の初回elapsedは61.529〜147.229msなので、最初から200msの31〜74%を使っている。
03のseq67はoffered=1402.330→skip=1402.590（L21349/21392、約260ms）、
04のseq78は1411.348→1411.723（L23097/23140、約375ms）で
`offer_mismatch_or_expired`。duration超過は事実だが、現ログはmismatchとexpiryを分離しないため、
「照合は全項目成功、expiryだけが原因」とまでは証明できない。pin_budget以外にもこの失敗がある。

### 27.3 先に検討した簡素化と不変条件

まずモーダル化、演出中の全画像処理停止、設定切替でreaderを閉じて再openする案を検討した。
モーダル化は連打・seek・close・F12を不便にし、全停止は既に進む通常高画質化や次unit先読みを
演出後まで遅らせる。reader再openは表示位置・窓・cacheを作り直す。
いずれもリリース済み挙動の退行なので **不採用**。採るなら利用者判断が必要であり、本案の前提にしない。

代わりに組合せを削る方法として、**通常pipelineだけが画像を準備し、presentationは完成した
paint materialと値のgeometryだけを消費する**。D2はこの所有分離を行い、通常producerの実行位置・
同期/非同期・表示選択時刻を変えない。prepare境界では、旧版の既存出力直接再利用を維持するため、
同じviewと既存cache identityのNativeだけを再利用する（§27.4、§27.11.1）。
新sourceのprepareを先に実行してから旧素材で上書きする形にはしない。重いprepareの移管はD3の未確定範囲である。
演出専用のdecode/prefetch/cache、resample queue、復旧state、cancel後のresumeを作らない。
既存 `Inactive/Flat/Offered/Turning/Completing` と一件continuationの所有は保つ。

守る不変条件:

1. OFFでは従来のselector、入力基数、producer/admission、prepare/paint、時計を含むfast pathを維持する。
   共通pipelineの非同期化を適用する場合もOFFのfirst-visible/最終画質到達が遅くならないことを別gateにする。
2. source identityとpaint allocationを別の型にする。sourceのidx/role/items/input/content/論理寸法は値の証拠であり、
   そのメタデータのために原寸TextureHandleやCPU pixelsをpresentationへ残さない。
3. Lanczos native出力は実allocationと `Arc<LanczosOutput>` をleaseする。source自体を描く経路は大判も含め
   明示的なManaged素材として実managed handleを保持・会計する。画像サイズによる新しい適用制限は足さない。
   TextureIdだけを所有証明にしない。native出力から元textureを再構築・readbackしない。
4. 通常の色忠実性、許可token、編集・AI・comic順、各頁自身のcanonical rect/UV/ROIを維持。
   不足ROIを引伸ばさず、演出poseから通常fit・PDF需要を再導出しない。
5. 通常producerは進み、演出後は最新の通常最終画質へ戻る。演出に固定した品質を最終cacheへ書き戻さない。
6. 自然終了・取消ともincoming全unitのmatching canonical flatを一採用frame提出し、その後だけnavigationを解放。
   close/別本/retireは既存の明示終了を優先する。破棄pass・部分curlはflat完了の証拠にしない。
7. 各contextの生成・更新・drain・cancel・invalidation・dropをそのownerに閉じる。共有するのは会計inventoryと
   GPU実行資源まで。兄弟を止めるanimation-active global boolやglobal source epochを作らない。

### 27.4 採用する所有境界 — sourceと画面素材を分ける

**D2の完成境界（P2-1への回答）**: Native出力を描く経路から余分なsource参照を除く。
等倍、Nearest等の拡大、出力上限fallback、大判Directのようにsource自体がpaint allocationである経路は、
そのsourceを明示的なManaged素材として従来どおり保持・会計する。新しい素材供給・縮小/ROI生成・skipを作らない。
「原寸sourceを全経路で保持しない」というD1の一律条件を、この実allocationに基づく区別へ改訂する。
名称は仮称。単なる `account()` の減算ではなく、参照グラフ自体を変更する。

| 所有者 | 保持するもの / handoff |
| --- | --- |
| 通常source/派生cache・通常prepare | 従来の原寸source、CPU pixels、編集/AI結果、`FullscreenPaintResource`、resampler需要。通常のkeep/evictionを継続 |
| `PresentationMaterial` | 排他的な `Native(Arc<LanczosOutput>)` / `Managed { handle, actual_mip_policy }`。paint ID、実寸、source UV domain、sampler等の描画契約。Native variantにsource handleはない |
| `PresentationPage` / unit | occurrence/role、source provenanceとgeneration/論理寸法の値、prepared material、transform/clip/spine/underlay、content binding、採用証拠。Painterは長期の正本にせず現passから供給 |
| Flat / Offered / Turning / Completing | 同じpresentation unitをmove/共有。acceptのincomingはresident選択の証拠と再利用可能なmaterialを捕捉し、通常prepareから最初の一回だけmaterialを受ける。独自の準備待ちは作らない |
| accepted output / in-flight batch | 実際に使うmaterial/underlayの同じArc lease。最後のbatch破棄までnative登録を保ち、UIでqueue完了を待たない |

通常prepareが出したLanczos結果は `LanczosOutput` 自体にsource handleを含まない
（`gpu_lanczos.rs:1001`）。この所有を利用し、`FullscreenPaintResource::Lanczos` 全体のcloneをやめる。
sourceを使う通常prepareにpaint-onlyを混ぜると、source再prepareやルーペで低解像度を原寸と誤認するので、
`FullscreenPaintResource`へ「sizeだけ偽装したDirect」を足す案は採らない。
元寸法・source IDは比較証拠として値で残すが、そのIDでsourceを描くことはできない型にする。

incomingのacceptでは現在のresident-only選択・faithful gateを維持し、I/O/新uploadを呼ばない。
使える通常prepared出力があればそのmaterialを共有する。なければ通常描画の一回のprepareから切り離す。
その間sourceを必要とする参照は通常pipelineのprepare jobが所有し、`AcceptedSources`の永続handle pinは廃止する。
通常prepare ownerは選択済み入力を準備完了まで保持して実resident会計し、単なるcache退去や品質publicationで
受理済み素材を失わない。準備が終わればその入力leaseを解放し、演出durationまで通常jobにsourceを残してはならない。
本/編集/許可等のbindingが無効になった場合は同pass通常表示・理由付きskipで終え、遅着から再開しない。
未完成materialを待つ新Offered queueやアニメーション専用のsource keepを作らない。
通常の隣unit需要（§26.8）とresample cacheは引き続き通常ownerのものとする。

保持済みNativeがあるframeの通常prepare境界は、既存bindと共有するview比較を先に行う。
一致時だけ保持page自身のtransform・論理寸法から需要を導出し、通常resolveと共有する
`LanczosCacheKey`の計算でtarget寸法・有効smoothing・scale branch・ROIを照合する。
source ID/世代は保持素材の値を使い、品質publicationの新sourceへの置換を再利用の条件にしない。
これらが一致する間は既存Nativeを再利用し、新sourceのresample/submitはthaw後に始める。
resize/DPI/fit/clip/画質設定等が一致しなければ従来の通常prepareへ進み、その後に既存bind/取消判定を行う。
したがって「毎frameの通常prepare後にbind」は、常に新sourceを生成するという意味ではなく、
この通常のprepare/既存出力再利用の境界を通った後にbindするという契約へ明確化する。
設定同期とfallback記録の解除は従来の直接再利用と同じで、cache-use時計は進めない。
別のbool/pending・worker/queue・source再pinを足さない。

Flat記録もNativeの余分なsourceをpinしない。Managedでは描画に必要なsourceそのものを保持する。
通常flat paintの準備済みmaterialを借り、次の操作へ使うのは採用された一unitだけ。
同じ解像度で通常flat、curl、mandatory canonical flatを描くので、終端で画質やgeometryが変わらない。
そのflat採用後、既存の次frame入口でfreezeを解き、通常selectorが新finalを選ぶ。
これはcopy/export/ルーペ用の原寸所有を移す設計ではない。明示的な操作は既存の通常素材要求へ戻す。

間接的なsource保持も同じ変更範囲に含める。`FsNavigatorTextureSource`と
`PendingFsPageTurnReady`（`ui_fullscreen.rs:2150/2204`付近）は現在resourceをcloneし、後者もsource/outputを会計する。
これらのON-origin提出情報を `NormalResource | PresentationMaterial` の明示的な提出payloadへ分け、
navigator・receipt・ready observerは描いたmaterialと保存したsource provenanceを消費する。
rawのcache存在をpresentationの可視証明として再要求せず、source IDを現在の新finalへ偽装もしない。
capture/holdover/frozen snapshotの通常原寸resourceを全域置換する必要はないが、presentation由来の
ready handoff/batch経由でNativeの余分なsourceが再pinされないことを必須監査にする。

`gpu_lanczos.rs:602/625/634`付近の通常prepareは、非resampler分岐、同寸法、出力上限fallback等で
sourceのまま返る。`Direct`というvariant名や寸法だけで判定せず、prepareが実際に返したpaint allocationで
Native/Managedを確定する。partial ROIのNativeは従来のcoverage/UVを維持し、不足時の既存判定を変えない。
Managedはその実寸・mipを数えるため、大判Managedの既存pin拒否が残っても会計を減らして通さない。
Native化するための大判Direct新素材供給はD2の範囲外であり、今回の未解決事項にも残さない。

D2では通常prepareが生成・submitまで入力を所有する現在の境界を保つ。acceptからprepareへの一時入力leaseは
実residentとして数え、prepare結果の確定後はNativeなら出力leaseだけ、Managedならsource自体を渡す。
`AcceptedOffer`の診断情報やready/navigator/batchに一時入力leaseを残して、Nativeのsourceをduration中保持し続けない。
この分離のためにworker/prepare待ち/新queueを追加しない。03/04の通常fitでは既存Lanczos出力だけへ
分離できることを最初のfixtureで証明する。通常prepare/producerと現行accepted_at時計の処理はD2ではそのままである。

### 27.5 D3の未解決要件 — 次版で再設計してから着手する

**P2-2/3は本D1rでは解決していない。D3a/D3bはv4.7.1の範囲外。** 表示consumerと通常準備producerを
分ける方向は候補として残すが、全共通pipelineへのworker移管を採用済み実装方針とはしない。
`resolve_fs_processed_texture`の先頭へheld判定を足して通常仕事を止めるだけでは根本修正にしない。

#### P2-2: CPU worker移管後の通常表示時刻（D3a）

現行の `ensure_final_composite_texture` はraw/edit resident・final missでも、その呼出しで
補正/post/texture登録まで実行して完成素材を返せる。一方、既存final-effect workerは未完了なら
既存textureまたはNoneを返し、後のpollで採用する（`app.rs:85303/85643/85689/85763/85806`付近）。
一般finalをカラー化/LUTなしでも先読みで事前完成させる契約はない（同:79524付近）。
単純なworker移管は軽い画像やOFFでも完成表示を一pass遅らせ、未加工表示/holdoverへ変え得る。
これは「遅くならない」という性能gateだけでは閉じない。次の点をD3aの着手前に定義し、独立レビューへ出す。

- **要求開始位置**: open/navigation/既存prefetch/描画のどの境界で、どのkey/content/contextの通常要求を
  開始するか。現在同passで完成するcaseをどう保つか。表示要求と先読みの優先・重複所有も明示する。
- **描画前の採用位置**: 完了のpoll/adoptionを最初の表示選択のどこへ置くか。同passの利用可能な結果を
  次passへ無条件に送らず、root/viewport/parkedの各入口で一度だけ採用する契約が必要。
- **cache miss時の表示**: 初回open/通常送り/OFF/ONそれぞれで、未加工・既存texture・holdover・loadingの
  どれをどの条件で描くか。faithful gateを守り、完成素材の初出・最終画質への到達を従来と比較する。
  通常利用の追加待ちが必要なら実装前に利用者判断へ返す。
- **既存の無変換texture再利用**は残す。CPU処理が不要なpathまで一律workerへ送り、別uploadや一pass待ちを
  足さない。全共通pipelineへ適用する範囲も上記契約を確定するまでは未決とする。

同期edit/補正/sharpen/post/clamp/大きいclone、`apply_sync_adjustment`、auto mask入力/black flatten等の
inventoryと、既存worker/key/generation/cancelを使う候補は残す。`Ordinary | FrozenPresentation(unit)`で
paint選択を一度決める案も候補であり、D2でensure/通常prepareの実行順を先に変更しない。

#### P2-3: GPU prepare完了と最初の表示選択（D3b）

resident sourceがあっても、初表示頁のnative出力は未residentになり得る。現在はcache miss時に
通常prepare内で生成・submitしてから返し、見開きも描画時にprepareする
（`gpu_lanczos.rs:685/703/818`、`ui_fullscreen.rs:47581`付近）。非同期化後の最初の呼出しで
material未完成を即skipとすれば、source先読み済みの通常fitで開始率が下がる。
同期生成へ戻すだけでは停止区間が残るため、次をD3bの着手前に確定する。

- 通常prepare要求を**いつ開始**するか、入力/需要/geometry/ROIの所有者と取消境界。
- **何を完了**とするか（生成・submit・登録・paint可能性）と、完了を描画前に採用する位置。
- **最初の表示選択**とoffer/material handoffをどの境界で一度だけ行うか。
  原寸sourceはreadyだがoutput未完成という通常caseで、新しいskip・品質退行・待機を足さない具体的な受け渡し。
  現行の同期prepare向け即skip規則を未完成の非同期jobへそのまま適用しない。

これらは通常利用の設計要件であり、まれな失敗の回復・retry機構を要求する指摘ではない。
**D3bはegui 0.37更新後に再計測・再設計する**。更新側もmipmap、delta消費、無描画submit、free順序、
presentを変えるため、ファイル競合だけでなく配送契約と性能原因を更新後のtreeで再照合する。
§27.8の計装仕様・旧baseline採取は先行可能だが、旧backendの測定を更新後の根因の確定に代用しない。

closure外の計測後、byte/job予算、重いallocation/staging/encodeのworker移管、短い登録/adoption、
必要ならupload/mip/resampleの分割・paint優先提出を比較する。**threadへ移すだけでGPU競合が消えるとはしない**。
managed deltaをworkerから渡しても実GPU仕事がUIへ戻り得ること、native化しても既存resamplerが
`renderer.texture(id).texture`を要求すること（`gpu_lanczos.rs:715`）も更新後に照合する。
tile数・転送上限・新GPU scheduler、native source所有の具体策は原因確定前に実装方針へ格上げしない。

全処理の演出後送り、先読み削減、低画質固定、最終画質化の意図的延期は不採用のまま。
必要と判明すれば利用者判断へ返し、D2だけで02のカクつき解消や全体性能gate達成と報告しない。

### 27.6 時計の採択（2026-10-16利用者決定、次版D4実装済み・実機未確認）

**最初に採用されたcurl提出を演出時計の原点にする案を利用者が採択した。**
`accepted_at`は操作から準備/初出までのレイテンシの観測値として残し、motion clockと混ぜない。
この仕様では、例えば準備120ms + 設定200msなら全面表示まで約320ms＋採用境界となる。
受理から約200msで打切る現行より全面表示が遅くなり得る費用も了承済み。
再質問は不要。**v4.7.1/D2ではaccepted_at原点・expiry判定を維持した。次版D4で本契約を実装した（§27.12、実機未確認）**。

採択済み案の実装条件:

- 最初の通常prepareで照合・ROI・pin admissionが成立したpassにp=0の初期poseを提出。
  同じoperationの最初の非discard outputだけが既存submission ledgerに開始時刻を一度publishする。
  次のowning frameがそれを使う。owner/hookの責務を逆転せず、毎frame/replay/mountで原点を更新しない。
  `Turning`内のclockを `AwaitingAcceptedOutput | Running { start_at }` の排他的な型にし、別pending boolを足さない。
  p=0は既存recipeの初期lift姿勢であり、角度0へ戻す変更ではない。遅いoutput hookもoperation/bindingと
  既存の終端状態を照合し、取消/close後に開始を復活させない。
- 以後のpはmonotonic wall clock / durationで決める。フレーム数で時間を伸ばす、hitchごとに時計を止める、
  最低frame数に達するまでfinishを待つ案は採らない。開始後に遅いframeがあれば飛ぶ事実を計測する。
- prepare待ちを新設しない。accept時にdisplayable sourceがなければ従来どおりinstant。
  終端にしたofferを非同期materialの後着でstartし直さない。一方、通常GPU prepareを非同期化した際の
  開始・完了・最初の表示選択は§27.5のP2-3として未解決であり、最初の呼出しの未完成を即skipと決めない。
  `offer_mismatch_or_expired`はbinding/source不一致と旧clock expiryを分離し、durationを準備timeoutとして流用しない。
- output採用はOS presentではない。backend提出前の長い163ms等が残ればこの新clockでも体感時間を消費する。
  §27.8でaccepted output→backend submit/presentを併記し、D3とD4の順序は通常prepareの未解決契約を踏まえて再レビューする。
  実present原点が必要と判明した場合は既存output tagからbackend提出証拠を同じledgerへ戻す範囲を別reviewし、
  wall clockを都合よくresetして合格にしない。
- close/取消・flat-first・新入力の既存順序は維持。slideshowは最新§26.12の
  「interval < Dならinstant、そうでなければcanonical flat採用後から保存間隔全体」を維持する。
  新clockに合わせて旧期限比較を復活させない。OFF/短間隔/連結読みの時計は変えない。

### 27.7 VRAM会計・lifecycle・多context

pin上限式は変更しない。会計は二種類を区別する。

- **実resident**: 通常cacheのsource/mip/出力と、cache外でも生存する通常prepare job入力・presentation/batchを
  `VramAccountant`のTextureIdで一回だけ数える。原寸sourceが通常cacheにあれば当然そのbytesを数える。
  subsystemの先着分類は総量と区別し、presentation-only/共有residentを診断で分ける。
  **at-rest contextの通常sourceもD2の必須監査対象**。現在の兄弟列挙はLanczos出力と類似previewが中心で、
  通常source cache全体を既に会計しているとは扱わない（`app/vram_accounting.rs:87/215`付近）。
  mounted/at-rest/parkedにある通常source・派生cacheとその保持経路を列挙し、必要な列挙を通常会計ownerへ接続する。
  presentationの参照を除いたため実residentまで消える実装は不可。通常cacheに残るsourceをeffect pinへ戻して
  帳尻を合わせず、同一TextureIdはcontext/cache/material間でdedupする。
- **演出pin admission**: 全contextで実際に保持するpresentation material allocationの和をdedupして予約する。
  Nativeは出力のみ・1mip、Managedは実際のmip/sampler属性を使う（thumbnailまで一律mip=trueにしない）。
  underlay checkerも保持するなら加算。logical source sizeの値をtexture allocationとして加算しない。
  Flatの実residentも会計し、Offered→Turningのincoming material確定で同じreservationを再評価する。
  共有outputを両面/二窓/batchで使っても一allocation。予算の確認とreservation確定はUI ownerの一境界で行う。

現在のweak inventoryはtextureを所有しない仕組みを保ち、source一覧ではなくmaterial leaseを登録する。
予約の有効性はtyped reservation ownerに結び、数値0を将来の状態判別sentinelとして増殖させない。
finish/cancelでpresentationがoutgoingを手放しても、生存batchのpin/実residentは解放しない。
canonical採用後のincomingは通常Flatへ移り、batchも含め最後のreservation ownerが退役した時だけpinを外す。
必要な全contextの通常会計はsnapshot境界で行い、演出frameごとの専用source全件走査を別に増やさない。
Managedは実paint allocationなので原寸のまま保持され得ることを、不要なNative source参照と区別する。

| 入口・終了 | 維持/変更するhandoffと確認対象 |
| --- | --- |
| 初回open、通常の別本open、folder/history/seek switch | 初回に採用Flatがなければ演出なし。別本/列/許可bindingの変更は旧ownerを退役。新本へ素材・continuation・worker完了を流さない |
| 単発key/click/wheel/touch/ring/gamepad | 既存semantic navigationと同じoffer入口。real source/role/slotから通常unitを作り、incoming全頁の一回prepareをhandoff。入力eventの再注入なし |
| 連打/held/no-op/方向反転 | 新operationでCompleting。incoming全unitを保持materialでcanonical flat採用→次frameに一件continuation。追加入力queue、finish待ち、drop規則変更なし |
| 自然終了、設定OFF、編集/fit/trim/rotation/resize/DPI | 自然終了と表示操作取消は既存flat completion。内容/geometryが無効なら通常再prepareへ戻し旧receiptを拒否。OFF由来で新material/clockを作らない |
| close/cancel/error/retire | closeはflat強制より先、当該ownerの要求/lease/receiptを退役。in-flight batchは自然drop。device loss/VRAM不足はログ＋演出省略、回復state/retryなし |
| F12 fork / promote / mount / deposit / park | forkは新presentation Inactive、通常の値移譲は同じcontext identityとmaterial/receipt/clockを保つ。新prepare要求もcontext所有。pause中にcontinuationを自動dispatchしない |
| 複数context / 同じ本・同じ頁 | Aのaccept/adoption/close/失効はAだけ。Bのmaterial/resample cache/queue/source世代を変更しない。共有allocationは会計dedupし、各窓のoperation/時間/receiptは別 |
| slideshow / 媒体境界 / 連結 / Split | §26.12の時間と通常のsource送りを維持。native動画への切替は§18のclose→commit→描画binding再判定。未実装Split演出を今回拡張せず、通常Split/連結読み/Remoteを維持 |

detached-rework-plan §2を読んだ。今回想定する変更は既存ViewerContextBundleのpresentation/通常準備資源を
sourceとpaintに分けるもの。geometryでHWNDを推測、host_lostで再生成、grace/retry、
新detached bool/placement保存を足さず、window/viewport述語を性能逃がしに変更しない。
したがって**症状パッチではなく素材・要求の所有境界の構造的修正**として提案する。
独立reviewerは素材/receiptを既存context owner内で分離する方向を構造修正として同意した（利用者からの伝達）。
描画入口・frozen/ready payloadへ触る実装はClaudeCodeと独立reviewerの双方がその具体的範囲に合意した後に限る。
実変更時に同plan §11へ範囲と根拠を追記する。D1rではそのファイルを変更せず、具体的実装範囲の合意完了とも記録しない。

### 27.8 追加perf計装と判定方法

D2の必須追加はmaterial所有/lease・会計・pin拒否の内訳とその解析へ限定する。
以下の通常producer/backend/新clock計装はD3/D4向けの仕様であり、全ての実装をD2出荷条件にしない。
旧baseline採取は先行可。D3bの性能原因・配送契約はegui 0.37更新後に再計測・再照合する。

perf無効/OFF fast pathでextrasや新しい時計を作らない。既存非blocking writerへ、次を相関して送る。
新profileはsource寸法・viewport物理寸法・ppp・duration・GPU/driver・warm/cold・通常/ZIP/PDF・窓配置を固定する。

| 境界 | 必須の測定/識別 |
| --- | --- |
| material capture / admission | source provenance（ID/世代/品質/寸法、非所有）、paint allocation ID/実寸/ROI/mip、owner=context/op、参照元=cache/Flat/offer/turn/completing/batch、per-allocation bytes・共有先・global pin合計・limit。skipでも内訳を一回出す |
| paint選択 / 通常producer | read-only選択、held選択、通常ensure/edit/final/post/clone/clamp、mask lookup/erase input/black flatten/rasterize、upload登録を排他的spanに分離。worker側のqueue/runとUI adoptionを区別 |
| root/viewport pollとupload | session/context/viewport、outer frame stampとpass、origin、request/key/items/content世代、current/companion/prefetch、job数/bytes/mips、CPU clone・delta登録時間。複数pollの総量を同outer frameへ合算 |
| backend（vendor経路） | output/view/context tag、deltaごとのID・bytes・mip数、renderer lock wait/hold、allocation/write、mip encode、resample encode/submit、buffers、surface acquire、queue.submit、present、free。wallとUI cyclesを対にし、親子/同時workerを二重加算しない |
| 演出の時計 | ingress→accept、accept→material prepared、prepared→first accepted curl、accepted curl→backend submit/present、motion elapsed、canonical flat採用、通常final-ready→初paint。旧accepted originと新motion originをschemaで明示 |

GPU timestamp等で実GPU時間を追加する場合は対応環境だけに限定し、CPU encode-submit時間をGPU時間と呼ばない。
backend計装は全viewport共通の既存配送点に置き、page-turn専用の別rendererを作らない。
renderer lock/driver待ちが支配的で通常spanで説明不能なら、次に非対話traceの待機stackを取る判断へ進む。
最初から大量の仮説別テストを利用者へ要求しない。

`analyze_perf.py`は新event/schemaと既存flat_barrier/continuationを扱い、session/context/window/
viewport/operationごとのcurl frame間隔・初回progress・完走・全skip理由を出す。
frame gap全体（idle/sleep含む）を演出中のhitchへ混ぜず、accepted outputと実presentを別列にする。
checkpoint/profile不備をFailでなくCannotJudgeとして残す現在の証拠規則は弱めない。

### 27.9 テスト計画・受入条件（D2単独と全体を分ける）

実装者がfocusedと会計/状態回帰を所有し、同じsource/条件の成功台帳を独立reviewerが再利用する。
D1/D1rではRustを変更していないためcargo fmt/test/buildを実施しない。文書差分・式・参照の照合だけを行う。

#### 27.9.1 v4.7.1 / D2単独の受入条件

- **Nativeの余分なsource保持だけを除く**。Flat/offer/Turning/Completing/ready/navigator/batchまで追い、
  通常prepare完了後にNativeのsource handle/CPU pixelsへの不要なstrong pathがないことを検査する。
  source cache退去後も出力leaseだけでpaintでき、最後のbatchまで登録が生存することをheadlessでも検査する。
- **Managedの正規経路は維持**。等倍、Nearest等の拡大、出力上限fallback、大判Direct、thumbnailを実prepare経由で
  fixture化し、従来と同じallocation/実寸/mip・画素・UVを保持/会計する。新しい素材生成・upload/resample・skip理由や
  サイズ制限を作らない。大判Managedの既存予算超過は残してよいが、会計だけを削って通してはならない。
- **03/04と同型のsource pin拒否を解消**。上限式は据え置き、通常fitの4 Native出力を保持するfixtureで
  旧1,101,735,408/1,428,267,016 bytesの余分なsource参照を外し、出力53.394/57.538MiB＋実underlay等を
  正しくpin判定する。acceptから通常prepareへの一時入力と、prepare後の長期materialの両境界を検査する。
  limit未満/等値/超過、incoming確定時の再判定、兄弟予約・重複ID、cancel/close後のbatch残留を含む。
- **実resident会計を失わない**。mounted/at-rest/parkedの通常source/派生cache・prepare入力・material・batchを
  監査し、presentation由来の参照を外した前後で、本当に解放されたallocation以外が総VRAMから消えないことを検査。
  とくに同じ本/頁のA・BでAだけclose/退去させ、Bの通常sourceが会計に残り、共有IDは二重計上されないことを確認する。
- **OFFの通常表示と既存演出契約を維持**。selector、prepare/producerの開始・完了/同期処理、表示選択時刻、
  無変換texture再利用、入力基数、品質upgrade、最終画質、flat-first、slideshow、F12の移譲/独立性を変えない。
  Native/Managed mixed、single/Pair、ROI/回転/trim/補助表紙等で通常flatとcanonical flatを照合する。
- focusedの所有/会計/handler回帰→既存snapshot→共有変更の `scripts/test-full.ps1`、独立完了review、
  関連文書とdetached §11更新・検証buildをD2の実装gateにする。実GPU/多窓の確認は別の明示承認枠で、
  headless結果を実presentの証明にしない。perfで同型のpin拒否と全skip理由を区別して報告する。

**D2で直らないもの**: seq67/78の約260/375msの開始前省略（現行expiry/照合）、02の同期final処理と
backend停止、準備による初回progressの消費。これらは残り得る。D2を「03/04の全省略解消」や
「02の滑らかさ改善完了」と報告しない。時計と通常prepareは変えないので、D2の出荷判定に次の全体性能gateや
開始率90%/完走率95%をそのまま課さない。同型source pinが消えた証拠と、残るskip/hitchを別に示す。

#### 27.9.2 次版D3/D4を含む全体の検証計画・性能gate

以下はD3の未解決契約を設計レビューで確定した後の全体計画。D2に共通する寿命・会計・flat/context回帰は
前項で先に実施するが、worker移管・新clock・backend性能の成功をv4.7.1の完成条件へ混ぜない。

| 検査 | 必須の反例と成功条件 |
| --- | --- |
| 素材寿命/会計 | Native materialから原寸handleへのstrong pathがない。source cacheをevictしてもoutputを描ける。通常cacheに残るsourceは総VRAMから消さない。managed/native/underlay/mipをIDでdedup。複数context+batchで共有し最後のdropだけ解放 |
| pin budget | 上記03/04式を旧構造の負例、4出力53.394/57.538MiBを新構造の正例にする。limit未満/等値/超過、incoming prepare確定時の増分、兄弟予約、自然終了/取消/close後のbatch残留を検査。数字を引くだけでsourceが生き残る実装を失敗にする |
| 状態遷移 | fake clockでFlat→Offered→Turning→Completing→Flat、accept後prepareがD以上、discard/replay、no source/no outgoing、bind/content/token変更、late結果、OFF切替。選んだ時計なら初回p=0、開始一度、再passで原点更新なし、開始後hitchはwallで進みduration無限延長なし |
| producer/入力統合 | root/viewport/gamepad/click/wheel/slideshowを実handler経由。Frozen frameに同期ensure/画素処理/upload/resampleがないことをspy、同時に通常workerの完了と最終cache採用・解除直後のupgradeが進むことを検査。単に全producerを停止して緑になる試験にしない |
| flat/capture/quality | LTR/RTL前後、single/Pair、横長/黒paper、Blank/Vacant/補助表紙、mixed thumbnail/native、trim/回転/ROI/DPI。始点とmandatory canonical flatは同material/geometry、終端/cancelの画素は通常flatと一致。navigator/receipt/ready/copy/exportがpaint IDとsource identityを取り違えない |
| multi-context/lifecycle | A/Bで同じ本・頁、Aだけclose/別本/失効、fork/mount/deposit/park/promote、Aの遅着をBへ採用しない。F12のready/batchにNativeの余分なsource参照が残らず、Managedとat-rest通常sourceは実会計に残る。媒体切替・close優先・flat-first N→N+1・OFF基数の既存回帰を維持 |
| GPU/性能 | headless rendererでoutput登録・sample・batch lifetime、delta配送と原寸free後の画素を検査。通常fit02/03/04、4000×6000、原寸/指定サイズ/DirectとOFF対照、二F12同時を別profile。実GPU/present/滑らかさは承認済み隔離実機枠の残gate |

自動gateは狭い状態/handler/会計→snapshot→共通変更の `scripts/test-full.ps1`。
vendor配送/計装を変更した場合は同gate内のvendor graphも含める。検証用buildは実装段階で
`build-dev.ps1`、exact release性能はrelease構成、ライブ確認は別の明示承認枠とする。
製品バイナリのエージェント起動・実データのテスト利用はしない。

全体の受入条件（D2単独出荷には適用しない）:

- 予算式を据え置き、03/04の通常fit・resident warm条件でsource pinによる9件と同型のskipを解消する。
  全件skipや分母除外で性能合格にしない。§9.1の3run×30機会、開始率90%/完走率95%を各profile別に評価する。
- Nativeでは出力materialと値の証拠だけを保持し、描かない原寸sourceやCPU pixelsを間接保持しない。
  source自体を描くManagedの実保持・会計、通常cacheの原寸品質と最終表示は維持する。
- 代表warm 60Hz/200ms profileでは初回progressを準備時間で飛ばさず、curl提出間隔p95≤20ms・最大≤33.4msを
  **提案gate**とする（利用者/設計reviewで固定してから測定）。CPU追加p95≤0.5ms/contextと
  GPU追加≤2ms/4Kの既存仮gateも維持。cold/open費用は別に報告し、大きいhitchを平均へ埋めない。
- OFF/held/seek/連結と、ONの通常高画質到達・次unit先読みの時間/件数をbaselineと比較し、
  意図的な待ち・機能削減なし。同期処理を消してもbackend境界の50〜163ms等が残るなら性能未達とする。
- 最終画質の着地・flat-first・兄弟独立・native lifetimeの自動回帰、独立完了review、利用者のmotion評価を全て満たす。
  D1の数値再構成やheadless成功をそれらの代用にしない。

### 27.10 採否・実装順・見積り・未確認事項

| 案 | D1rでの判断・実装時期 |
| --- | --- |
| Nativeの余分なsource参照を除き既存output/ROIを共有、source描画はManagedで保持 | **D2採用案、v4.7.1**。新素材生成/skipなし。実residentとat-rest通常sourceの会計を保つ |
| 表示consumerと通常準備producerの分離、重いCPUの既存worker移管 | **次版D3aの候補**。要求開始・描画前採用・cache miss表示の未解決契約を先に確定する |
| GPU配送/提出の計装→原因別の非同期・有界準備 | **次版D3bの候補、egui 0.37更新後**。旧baselineは先行可、更新後に再計測・配送契約を再照合して再設計する |
| 初回採用curlをclock原点にする | **利用者採択済み、次版D4実装済み・実機未確認**。準備+Dの全面表示費用を了承。D2単独では時計を変更しなかった |
| pin上限増量、会計だけsourceを除外、source寸法偽装 | 不採用。実所有と乖離、通常VRAM圧迫・原寸誤認を残す |
| 演出専用decode/縮小cache、frameごとのcapture/readback、mesh削減だけ | 不採用。通常準備の重複・画質/寿命問題を増やし、観測された主区間を直さない |
| 演出中全producer停止、final uploadをfinish後へ一括延期、先読み削減 | 不採用。通常機能の退行。採るなら別途利用者判断が必要 |
| modal入力lock、delay/retry/追加repaint、全case原寸保持のsilent fallback | 不採用。所有違反と停止を隠す。通常不足は理由付きskip、まれなVRAM不足/device lossはログ＋演出省略、回復機構を足さない |

実装は独立設計レビュー後にcoherent chunkで分ける。共有fileのwriterは各chunkで一人。
v4.7.1はD2だけ。D3a/D3b/D4は次の版の範囲で、D3bはegui更新後の依存gateを持つ。

| chunk | 完成物・主なfile所有範囲 | 概算（一人の作業日） |
| --- | --- | --- |
| D2 / v4.7.1 | Native source参照の除去、Managed維持、Flat/accept/batch/ready/navigatorの一貫接続、at-restを含む通常source会計の監査・必要な接続、素材/pin計装・解析と単独受入回帰。`page_turn.rs`、`gpu_lanczos.rs`、`ui_fullscreen.rs`、`app/vram_accounting.rs`、context登録。新素材供給・producer/clock/backend変更は含めない | **3〜5日**＋独立レビュー・修正・実機枠 |
| D3a / 次版 | §27.5の要求開始/採用/cache miss表示を再設計してから、通常selector/producer・edit/final/mask前処理とcontext/cancelを改修 | 旧3〜5日は既存ownerで収まる場合の条件付き修正目安。再設計後に更新 |
| egui 0.37移行後の再照合 / D3b前提 | 更新後treeでmip/delta/無描画submit/free/present契約を再照合し、更新前後baselineを比較、通常prepare開始/完了/最初の表示選択と原因を確定 | **別計上、未見積り**。egui更新本体の工数とも別 |
| D3b / 上記確定後 | upload/mip/resample/lock/submitの原因に応じた具体策を再レビューし、必要な通常GPU準備ownerとvendor経路を改修 | **3〜6日は原因確定後に割り当てる修正予算**。現時点の完了見積りではなく、範囲次第で再見積り |
| D4 / 次版 | 採択済みclock、遅いhook/取消/fake clock/handler回帰、関連文書と検証build。D3との順序・全体性能比較は再レビュー | 旧2〜3日は条件付き目安＋レビュー・実機待ち |

旧合計11〜19作業日は既存ownerの再利用で収まる場合の参考値に留め、**全体完了見積りとしては使わない**。
D3の未解決設計、egui移行後の再照合、原因確定後のD3b予算を一つの確定日数へ足さない。
D2は単独出荷可能な境界としてレビューするが、目的2の完了とはしない。想定外の所有境界変更が必要なら
guardを足して通さず範囲を設計担当へ返す。大判Directの新素材供給はD2へ追加しない。

未確認の前提・次に解決するもの:

- closure外の支配要因。既存ログが確定したのは境界までで、GPU待ち/配送/mip/present/lockの割合ではない。
  計装仕様/旧baselineは先行可、D3bはegui 0.37更新後に再計測・再設計する。D1rでは新しい実機採取を求めない。
- P2-2の通常要求開始/描画前採用/cache miss表示、P2-3の通常prepare開始/完了/最初の表示選択。
  今回は解決せず、次版D3の着手前要件として残す。
- skipped offerの全texture ID/参照元、実GPUでsource解放後のnative出力独立性、at-restの通常source会計。
  D2の実装・検証で閉じる。Direct/指定サイズ等は実paint allocationを維持し、新素材供給の検討でD2を止めない。
- 元の画像フォルダとログのZIPで同じ割合になるか、二F12同時のupload競合、GPU/driver/refresh条件別の改善幅。
  ログからそれらの性能を一般化しない。
- clock原点と準備+Dの費用は利用者採択済み、実装は次版D4。D1rの独立再レビュー、D2の具体的実装範囲の
  構造合意・実装/テスト/実機受入れ、D3bの具体策の合意は未了。
  D1r時点では設計文書だけで停止した。D2の今回記録は次項に分ける。

### 27.11 D2実装記録（2026-10-16、実機未確認）

`FullscreenPaintResource`は通常source/prepare用の型として残し、描画境界の
`FullscreenPaintPayload`を`NormalResource | PresentationMaterial`へ分離した。
`FullscreenPresentationResource`の素材は`Native(Arc<LanczosOutput>) | Managed`の排他的な所有で、
Nativeはsource ID・世代・論理寸法・proofを値で保持するだけである。
Managedは実ハンドルとcapture時のTextureOptionsを持ち、実mip policyで会計する。
通常display sourceはallocationのsamplerを後から変更しない。egui 0.33のmetadataは初期optionsなので、
この既存契約をcaptureの前提とする。`.set`/partial更新の実使用はmask overlayだけで、display sourceではない。
大判Direct、等倍、Nearest拡大、出力上限fallbackをNativeへ変える処理は足していない。

accept直後は既存のresident選択を`AcceptedSources`の短命なNormal入力leaseとして保持する。
通常描画が一度prepareした後、offeredの照合・旧expiry・projectionが成立する境界で、
診断情報を含む受理素材をprepared presentationへ置き換え、入力leaseを解放してから再予算判定する。
Flat/unit/batch、navigator、pending readyも同じpresentationを保持し、Nativeのraw handleを復活させない。
追加worker/job queue/待機状態を作らず、通常producerとaccepted_at原点は維持した。
通常prepareの仕事開始境界は§27.11.1の採択に従い、既存出力の条件付き再利用を維持する。
テストの時計注入は既存の時刻読取り位置だけに置き、OFFでは時計を読まない。

実装上、従来のheld resourceは通常TextureHandle選択とgeometryの入力にも使われていた。
通常selectorの実行を残したまま、最終paint選択をcache residencyから分離し、
sourceが退去してもheld outputと保存した寸法/fitの値から描く。
通常sourceが在住するpassも通常のprepare境界を通すが、同じview/keyのNativeは生成前に再利用する。
viewport/clip/placement等は現passから再計算し、既存bindと共有するview比較で変更を検出する。
source不在で新しい要求UVが保持ROIを超える場合だけ、同passの既存許可thumbnail/loading選択へ戻し、
既存のbinding取消理由を使う。source texelの既存許容内の丸め差は不足としない。
readyは採用済みreceipt・destination/layout/items/input generationと素材のprovenanceを照合し、
Nativeの可視証明としてraw cache在住を再要求しない。
実提出unitの`PageTurnContent`をreceiptの排他的な`Ordinary | Presentation`値bindingとして捕捉し、
flush時にmask/params/comic等も照合する。emit時の現値を採り直さず、maskだけが変わってinput epochが
同じ場合も旧Native readyを採用しない。通常OFFはOrdinaryでsnapshot allocationを行わない。
ON由来のOFF-Completing再描画は実receiptのoriginから素材化し、canonicalのgeometry更新後も
unit/navigator/ready経由でNative sourceを再pinしない。
演出対象外のON表示・SNS previewとOFFのnavigatorは従来の通常sourceを使う。
演出由来でもnavigator全景を覆わない本物のpartial出力は、該当頁の通常cacheに残る同じsource IDを
read-onlyで一時参照し、なければ既存の許可thumbnailへ戻す。新finalのpublicationで旧rawが候補の
一位から外れても、在住する旧rawを見つける。新しい素材生成/keep/workerは足さない。

通常会計ownerにread-onlyの`ViewerTextureCaches`列挙を置き、mounted/at-rest/parkedを同じwalkerへ接続した。
通常source・final/edit/comic/erase/local-adjust/conceal等の派生cacheとthumbnailを、
presentation inventoryの有無やON/OFFとは独立に数え、全contextで同じTextureId集合を使う。
従来の兄弟列挙がLanczos/similar中心だった欠落と、通常passthrough/holdover/snapshotの保持も監査・接続した。
通常cache側の既存mip規約（静止画等true、thumbnail/passthrough false）は維持する。
erase preview、analysis overlay/SV、退避snapshot thumbnail、類似候補Ready thumbnail、
composition editorの採用済みtexture mapも列挙する。
editor worker送信前/receiver内の未採用textureはread-only列挙不能で、既存producer/transferの別境界である。
今回の保証は通常cacheと採用済みmapの全context会計であり、全GPU所有の完全列挙を主張しない。
この既存の未採用境界はpresentation参照削除によって生じた会計消失ではなく、D2でproducerを変更しない。
通常snapshot/holdoverだけに残ったthumbnailは通常resourceにmip metadataがなく、既存true規約の保守的な会計となる。
presentationのManagedはこの制約とは別に実optionsを記録する。

pinの上限式は据え置き。03/04 fixtureのoffer入口では、一時入力2枚とoutgoing Native2枚の
564,864,504 / 729,216,848 bytesを判定し、prepare後は4出力の55,987,200 / 60,333,360 bytesだけを予約する。
前者の一時入力は実residentとしても数える。固定時計で旧expiryとpin拒否を分離し、
pinを通過しただけでfinishしたとは判定しない。寸法fixtureはGPU/pixel確保をしないheadless会計検査であり、
実GPUでの画質・開始率・滑らかさ改善の証拠ではない。
`offered`/`skip(pin_budget)`にinput/materialのID・寸法・保持内訳を、
`material / prepared`に入力解放後の再判定bytesと両unitの素材内訳を追加する。
解析器は同kindを任意診断として読み、operation/頁idxごとのNative/Managed内訳とpin値を出す。
旧ログに新fieldを必須化せず、D3/D4の性能gateは追加しない。perf無効時は新しい素材JSONを構築しない。

今回の利用者指定の実行gateはfmt、core check、focused、本体lib全件と必要なsnapshotである。
build-dev、製品起動、コミットは行わず、実機確認とgit操作は設計担当/利用者へ渡す。
seq67/78の開始前省略、02の停止、準備時間がdurationを消費する挙動は今回の未解決範囲として残す。

#### 27.11.1 prepare境界の補足判断（解決済み、2026-10-16）

独立実装レビューで、source-free化後の通常prepareの仕事開始時刻にD1rとの不一致を確認した。
旧版は通常producer/pollを続けながら、安定viewの描画にはheld source付きLanczos resourceを返していた。
`GpuLanczosCache::resolve`はその既存出力を再利用するため、新finalのpublicationがあっても、
新sourceのresampleはthaw後に始まる。checkpoint `a021f23f8`では通常selectorの新sourceをprepareし、その後に旧素材へbindしていた。
新sourceの出力がcache missならturn中に生成・submitが前倒しされる。旧素材を描き続けるだけでは
§27.9.1のprepare開始・完了維持を満たさない。性能悪化の実測証拠はなく、仕事の開始境界の差をコードで確認した段階である。

設計担当は推奨案を採択した。安定したviewで保持素材とgeometry/ROI/filterが一致するときは既存Native出力を再利用し、
resize/DPI/fit等で一致しないときは従来の通常prepareへ進む。新sourceの生成・submitを演出中に前倒しする案は採らない。
旧版の仕事開始時刻を維持するための選択なので、追加の利用者判断は不要との決定である。

実装は既存bindのview比較を`page_turn_matching_held_page`へ共通化し、single/Pair双方からprepare前にも使う。
品質publicationでsource寸法が変わっても、同じviewなら保持transformの倍率と保持source寸法で需要を作る。
新sourceの倍率を旧寸法へ掛ける判定はしない。Native出力自身が生成時の`LanczosCacheKey`を値で所有し、
通常resolveと共有する需要計算・既存のtarget/smoothing/scale branch/ROIの一致を使う。
描画coverage用の許容誤差をcache keyへ混ぜず、ManagedはこのNative再利用の対象にしない。
一致時には設定同期とfallback記録解除を保ち、cache-use時計や新source key lookup/生成は進めない。
不一致時の通常prepare、後段bind/取消、最初のOffered入力prepare、thaw後の品質upgradeを維持する。
無条件のheld選択、新bool/pending、新worker/queue、source再pinは追加しない。
§27.4の契約をこの境界に合わせて明確化した。先行の素材所有・receipt指摘も修正済みである。
source退去後も同じ契約を使う。通常の表示modeがheldを許可しない場合は候補にせず、
Nativeのview/key/coverage不一致はprepare済み素材なしとして通常thumbnail/loadingへ戻す。
後段bindは、shared prepareが選んだNativeのpaint IDを確認し、拒否した旧Nativeを復活させない。

#### 27.11.2 checkpoint `a021f23f8`以前の検証記録

- `cargo fmt`（引数なし）、core `cargo check`は終了コード0。ただしcore check後の小さなreceipt/選択修正は
  focusedでコンパイル・検査しており、判断後の最終core checkは再実行する。
- `test-focused.ps1`はpage-turn 105件、gpu_lanczos 46件、通常会計4件、ui_fullscreen 925件が成功、終了コード0。
  ui_fullscreen内にpage-turn 105件を含み、延べ1,080件・重複を除くと975件。
- `cargo test -p mimageviewer --test ui_snapshot`は181件成功、終了コード0。
  `--features test-script`の`drawn_paint_records_native_presentation_after_source_release`は1件成功、終了コード0。
- `python -m unittest test_analyze_perf`は103件成功、`cargo run -p viewer_context_audit`も終了コード0。
  `git diff --check`は終了コード0。
- 03/04は実offer入口とprepare後再判定の回帰で検査済み。旧時計のexpiryは固定時計で分離している。
  このcheckpointでは変異検査と`test-lib-full.ps1`は未実施だった。続きの検証は次項に分ける。
- 実GPU、実本03/04、F12実機は未確認。製品バイナリ起動、build-dev、コミットは行っていない。

#### 27.11.3 prepare境界の追加回帰と最終検証

`prepare_boundary_tests`は実offer/startを通し、実需要と同じkeyのNative metadata fixtureで
single/Pairの通常描画と通常resolve入口spyを接続する。固定時計で開始を成立させ、その後の
prepare検査中はexpiryから十分離す。時計の原点・製品のduration処理は変えない。

- 安定viewで新sourceのID/寸法/input epochがpublishされても通常resolveを呼ばず、保持出力/geometryで描く。
  通常selector自身は新sourceを返すため、producerを停止して成功させる検査ではない。
- resize/DPI/fit/clip/underlay/filterの変更では新sourceの通常prepareへ進む。
- ON/OFF双方のCompletingはcanonical flat採用まで保持し、thaw後の通常描画は新sourceをprepareする。
- 見開きの原画preview・faithful表示・非最終提出ではNativeを早期再利用しない。
  実際の提出資格をprepare前へ渡し、後段でliveを下げるだけで加工済み素材が残る回帰を防ぐ。

GPU側の3件は共通keyの各入力・Managed拒否・通常の設定同期/fallback解除・cache-use時計維持を検査する。
これらはmetadata・処理経路の自動検証であり、実GPU生成/presentや利用者の滑らかさ評価を証明しない。
独立レビューは見開きの資格伝達の指摘修正後、製品差分と新規4件をACCEPT（テスト実行は親担当）。
一時変異はscratch `target/d2-mutations.py`で実行し、次の9種類を一つずつ有効にした。
対照9件は全成功・終了コード0。各変異は対応回帰1件が失敗し、すべて終了コード100だった。

| 一時的に壊した処理 | 失敗した回帰 |
| --- | --- |
| 大判Nativeをsource保持のManagedへ戻す | 03/04の実offer入口・prepare後pin再判定 |
| prepare後のpin超過を通す | 入力解放後の出力超過拒否 |
| at-rest通常source会計を外す | A退去後もparked Bのresidentが残る検査 |
| receiptの内容照合を省く | mask変更/missing binding/品質publicationの区別 |
| 成功Native再利用を無効にする | 安定single/Pairで新sourceのprepare入口を呼ばない検査 |
| 共通view比較の拒否を無効にする | 変更viewで通常prepareへ進む検査 |
| Native cache keyの一致を無視する | ROI/target/filter/Anime limitの入力別検査 |
| 再利用時の設定同期/fallback解除を省く | housekeepingとcache時計維持の検査 |
| 見開きの提出/原画等の資格を無視する | 原画preview/nonfinal/faithfulの実描画検査 |

全変異を除去し、4ファイルを開始前のbytes/SHA-256と照合して復元一致を確認した。
scratchとログはGit管理外の`target/`だけに置く。復元後の最終gateは引数なしfmt、core check、
focused（page-turn/GPU/通常会計/UI fullscreen）、UI snapshot、本体lib全件の順で行う。
復元後のfmt/core checkは終了コード0。focusedは109/49/4/929件が成功（延べ1,091件、
page-turnの重複を除くと982件）、UI snapshotは181件が成功し、各終了コード0だった。
Python解析器103件とviewer context auditも終了コード0である。
全件の初回実行はRAW用fixture不足で37件失敗（終了コード100）。製品コードを変更せず、既存checkoutの
CC0サンプル27件（360,971,087 bytes）を`tests/raw-samples.json`のSHA-256/サイズと照合して実体コピーし、
失敗37件の限定再検査は全成功・終了コード0になった。fixtureはGit管理外の`vendor/raw-samples/`に置く。
ignored入力の追加なので、全件は`test-lib-full.ps1 -Force`で改めて実行する。
本体lib全件は`test-lib-full.ps1`の台帳/JUnitと最終報告を実行結果の正本とする。
実機・製品バイナリ起動・build-devは今回の指示どおり実施しない。D3a/D3b/D4は未実装である。

#### 27.11.4 source退去後の完成レビュー指摘への対応（2026-10-16）

`1e3639d86` / `dde4dd345`の完成独立レビューはREVISE、P2が2件だった。
元画像selectorがNoneを返してもheldの加工済みNativeへ戻る経路と、source不在でview/keyが不一致でも
presentationの無条件cloneが旧Nativeを描く経路が残っていた。設計担当も退行と判断し修正を指示した。

修正は既存の通常選択・prepare境界にまとめる。`fullscreen_paint_payload_for_page`へ呼出し元の
既存original-preview sampleを渡し、通常resolverと同じ`fs_display_bypasses_final_pipeline`を使う。
bypass時は通常sourceだけを選び、NoneならNoneとする。OSキーの再sampleや新しいmode状態は足さない。
単頁・ズーム・見開きで同じ入口を使い、OfferedのNormal入力overrideにも同じ資格を適用する。

`prepare_fs_paged_paint_payload`はview/key/coverageが一致するNativeだけを返す。
sourceを持たないNativeが不一致なら、その候補ではpreparedなしを返し、single/Pairの既存candidate loopが
通常thumbnailを一度選び直すかloadingへ進む。通常sourceがある候補は従来のprepareへ進み、
Managedは実allocation保持を維持する。source-freeの無条件cloneを行う下位wrapperは削除した。
coverageだけを再利用の許可とせず、同じprepare判定の一入力として扱う。

keyだけの不一致（例: downscale smoothing変更）からthumbnailを選んだ場合も、late bindが旧Nativeを
戻さないよう、旧素材がNativeなら準備後のpaint ID一致を要求する。IDは選択結果の確認であり、
寿命は既存のArc leaseが保証する。後段に別のkey計算やsource再pinは足さない。
単頁の候補切れも見開きと同じempty submit / `no_page`退役へ接続し、loadingだけでCompletingを残さない。
単頁のRAW提示通知もprepareで採用した候補だけに行い、拒否候補を表示済みとして記録しない。
新しいbool/pending/state/worker/queue/repaint/待機は追加せず、producer・時計・予算式は変更しない。

回帰はsource cacheを退去させ、通常raw selectorがNoneであることを確認してから描画する。
元画像表示、安定view、resize/DPI/fit、key-only変更、通常thumbnail/loading、Turning/Completingを検査し、
最終mesh・navigator/readyと状態の終了まで追う。03/04のbytes/offer/prepare後予算/expiry/lease会計は維持し、
実viewと異なる数値用Native fixtureを安定view描画の証拠とする検査は、実需要一致の小fixtureへ分離する。
一時変異はGit管理外の`target/d2-source-absent-mutations.py`で次の4種類を個別に有効化した。
変異なしの対照2件は成功・終了コード0、各変異は対応回帰が失敗し、すべて終了コード100だった。

| 一時的に壊した処理 | 失敗したsource不在の回帰 |
| --- | --- |
| 通常表示modeによるheld選択の拒否を除去 | 元画像表示で加工済みNativeを描かない検査 |
| 不一致Nativeの無条件cloneを復活 | resize/DPI/fit/key変更から通常候補へ戻す検査 |
| late bindのNative paint ID照合を除去 | key-only変更後のOFF/thumbnail選択を維持する検査 |
| 単頁の候補切れempty submitを除去 | loadingへ進んだCompletingを退役させる検査 |

変異後は2つの製品ファイルを開始前のbytes/SHA-256と照合して復元し、注入コードが残っていないことを確認した。
結果は`target/d2-source-absent-mutations-result.json`と対応ログに保存する。
初回focusedではCompleting/OFFのthumbnailが即acceptされた際もpendingを要求するテスト側の誤りが2件あった。
実際のreceiptがaccept済みならpending不在を検査するよう修正し、上記対照と変異を実行した。
製品側の挙動をテストに合わせて変更したものではない。
fullscreen全体の初回検査は933件中932成功・1失敗（終了コード100）だった。既存ROIテストはheld ownerも
一致keyもないNativeを直接渡し、coverage内なら描けると期待していたため、今回の不変条件と矛盾していた。
独立レビューでもテスト前提の修正を妥当と確認し、covered/expandedの両需要で既存thumbnailへ戻り
旧Nativeを描かない検査に変更した。正当な再利用は実offer/startと実需要key一致の回帰で検査する。
復元後の引数なし`cargo fmt`とcore checkは終了コード0。修正後の`test-focused.ps1 -Path src/ui_fullscreen.rs`は
933件全成功・終了コード0（70.209秒）で、追加回帰と03/04の実offer/prepare後予算検査も含む。
UI snapshotも181件全成功・終了コード0（25.29秒）。独立レビューは製品差分とテスト前提修正をACCEPTした。
本体lib全件の初回は13,263成功・59 skip・3失敗（終了コード100）だった。変更外の
`detached_native_video_navigation_syncs_main_selection`（worker未完了）、
`streaming_drain_and_reacquire_between_frames_do_not_restore_local_view`（AcquiringRemoteのまま）、
`startup_configuration_away_back_then_normal_stop_cannot_reuse_old_marker`（scan marker不一致）が失敗した。
同じソースの失敗3件を明示filterで限定再実行すると全成功・終了コード0（0.348秒）。
原因は未確定であり、製品/テスト/並列条件を変えず全件を再実行する。初回ログは
`target/d2-source-absent-final-lib-full.log`、限定再検査は`target/d2-source-absent-full-failures-focused.log`に残す。
同条件の全件再実行は13,264成功・59 skip・2失敗（終了コード100）。初回の3件は成功し、今回は変更外の
`abort_on_cancel_returns_interrupted_immediately`（218msで時間上限超過）と
`finite_source_reaches_end_after_filling_the_live_window_without_a_next_fetch`（producer完了待ちtimeout）が失敗した。
この2件の限定再検査も全成功・終了コード0（2.080秒）。ログは`target/d2-source-absent-final-lib-full-rerun.log`と
`target/d2-source-absent-full-rerun-failures-focused.log`に保存する。最終全件はホストの同時実行数を
プロセス内の`NEXTEST_TEST_THREADS=4`へ下げて実行する。製品/テスト/skip/retry条件は変更しない。
この実行条件も台帳へ含まれる。並列度変更による因果の確定や、初回失敗を製品修正で解決したとは扱わない。
並列度4の初回全件は13,265成功・59 skip・1失敗（終了コード100、1,222.377秒）。先の5件は成功し、
変更外の`incremental_reconcile_uses_existing_supervisor_watcher_without_new_fulls`だけが失敗した。
similar arrayのbase publish（既存`MoveFileExW`）がアクセス拒否を返してDegradedとなり、完了待ちが120秒で切れた。
同じソースの限定再検査は1件成功・終了コード0（2.831秒）。ログは`target/d2-source-absent-final-lib-full-threads4.log`と
`target/d2-source-absent-threads4-failure-focused.log`に保存する。原因は未確定であり、対象外のI/O処理へ
retryや回復処理を追加せず、同じ並列度4で全件を再確認する。
最後の本体lib全件は`test-lib-full.ps1`で実行し、対象差分のsource hash・件数・終了コードは
`target/test-lib-full/`の成功台帳/JUnitと完了報告を正本とする（文書への結果追記で台帳のsourceを失効させない）。
製品バイナリ起動・build-dev・コミットは行わず、実機・03/04実本・F12は未確認である。

### 27.12 D4実装記録（2026-10-16、実機未確認）

D4は採択済み§27.6の時計だけを変更する。`accepted_at`は受理レイテンシの診断値とし、
Offeredの照合はbinding/source/geometry/許可・pinだけで判定する。durationによる準備中のexpiryは撤去し、
不一致は`offer_binding_or_source_mismatch`へ分離した。受理時にdisplayable sourceがない場合は従来のinstantで、
新しい素材待ち・timeout・後着からの再開を作らない。通常prepare、producer、Managed/Native所有はD2のままである。

Turningは`MotionClock::AwaitingAcceptedOutput | Running { start_at }`を持つ。
Awaitingでは既存recipeのp=0（初期lift）を提出する。既存output hookが非discard curlを初採用したときだけ、
operation固有のsubmission ledgerへ時刻を公開する。hookはtrackerのimmutable operation/destinationとreceiptの
宛先を照合し、Offered/Started以外の終端から開始を復活させない。UI ownerは次の描画で一度取り込み、
Runningではmonotonic wall time / durationだけを使う。replay、追加output、mount/depositで原点を更新せず、
hitch・少ないframe数を理由に延長しない。取消・close優先、canonical flat採用→次frame continuationの順序は変更しない。
slideshowは§26.12のまま、interval < Dならinstant、それ以外はflat採用後から保存間隔全体を数える。
OFF・短間隔・連結読みのtimerには変更を加えない。

簡素化は既存operation ledgerへの一回publishと既存Turning内の排他時計で実現した。
モーダル化・入力停止・別pending bool・新しいtick/worker/queueは不要で、追加しない。
まれな失敗は既存のログ＋演出省略を維持し、復旧処理を追加しない。

perfは既存`page_turn_animation`のstart/frame/finish/visibilityに以下を追加する。
`clock_origin=first_accepted_curl`はmotionの原点で、旧`elapsed_ms`の意味は受理→projectionのまま保つ。
`accept_to_first_curl_ms`は受理→最初の非discard curl output採用、`motion_elapsed_ms`はその原点→projection、
`output_motion_elapsed_ms`は原点→当該output hook、`progress`は実提出poseである。
`motion_frames`は採用curl数、`motion_max_gap_ms`は採用output同士の最大間隔（最初のcanonical終端までを含む）。
初回の準備時間は含めず、1 curl→duration超hitch→canonicalでも最大間隔を記録する。
解析器はoperationごとのレイテンシ・frame数・最大間隔・初回progress・motion/output elapsedを任意診断として集約する。
旧ログへの必須fieldや新性能gateは追加せず、backend submit/OS presentの証拠とは扱わない。
開始後のbackend停止やD3の同期処理は今回の未解決範囲である。

回帰はfake clockで2秒の準備後に実handler/通常描画からp=0を提出し、Single/LTR/RTLのmeshをrecipeと照合する。
projection後からhookまでさらに80ms進め、開始がprepare時刻ではなく初回output採用時刻であることも区別する。
Singleは1 curl後の1秒hitchで着地し、見開きは50/150/199msの中間提出から着地まで確認する。
原点一回、discard/replay、取消/宛先/operation不一致の遅いhook、wallのhitch完了、無素材instantと後着非再開を検査する。
既存の実mount回帰は兄弟ごとの異なる開始時刻と、兄弟取消・deposit後に元contextが原点を維持することも確認する。
既存のpose/finishテストはaccepted_atの書換えからmotion時計の操作へ変更し、D2の03/04会計fixtureも
準備duration超過で失効しない契約へ更新した。slideshow・OFF・短間隔・連結とsnapshotも検査対象に含む。
実行結果・変異確認は本節追補と`target/`内ログ、全libは`target/test-lib-full/`の台帳/JUnitを正本とする。
利用者指示によりbuild-dev・製品起動・コミットは行わない。実機03/04と滑らかさ・F12/presentは未確認。

公開文書は設計担当が変更する。修正案:

- `manual/settings.html#page-turn-animation`の「時間」は「めくる動きが始まってからの時間。
  画像の準備時間は含まれず、操作してから次のページが全面表示されるまでは準備時間もかかる」と補足する。
- `manual/fullscreen.html#page-turn-animation`の時間にも同じ意味を添える。
- 両ページの「残り時間が短ければ省略」は§26.12と不一致なので削除し、
  「スライドショー間隔がめくる時間未満なら省略。それ以外は全面表示後から設定間隔を数える」へ変更する。
- `manual/changelog.html`の既存リリース記事は履歴として維持し、次版記事で時計の変更を説明する。

#### 27.12.1 自動検証の条件と記録

新規worktreeではLibRawソースが不足していたため、既存checkoutのLibRaw 0.22.2/zlib 1.3.1のtarballを
setup scriptの固定SHA-256と照合して展開した。RAWのCC0 fixture 27件（360,971,087 bytes）も
manifestのサイズ・SHA-256と照合して実体コピーし、FFmpegの6 DLLを`target/debug/deps/`へ配置した。
TurboJPEGのMSBuild経由の構築は終了1・診断0で失敗したため、同じturbojpeg-sys 1.2.0 / Debug /
SIMD有効 / 静的CRT条件の既存install出力を実体コピーしてexplicit static指定で検証する。
MSVCのincludeには通常形式パスもCXXFLAGSへ渡す。製品/依存ソースはこの環境問題のために変更しない。
環境指定・native入力hashは`target/d4-native-env.ps1`と`target/d4-verification-environment.json`へ保存する。

初回focusedは追加fixtureのnavigation引数型でコンパイル失敗（101、テスト0件）。
修正後は116件中115成功・追加回帰1失敗（100）。draw helperの通常ready通知がなくobserverが退役しない
fixture不備だったため、実経路と同じ通知を追加した。限定再検査は3件中2成功・1失敗（100）で、
見開きfixtureが単頁用Deltaを渡していた。見開き入力の解決済みTargetを渡す契約へ直し、
限定3件は全成功（0）となった。製品のnavigation/observer/採用条件を弱める修正はしていない。
独立Sol / xhighの静的完成レビューは指摘修正と回帰補強後にACCEPT、未解消P1/P2なし。
レビュアーはテストを実行せず、親担当の実行証拠を再利用する。
最終の全件件数・終了コード・source/条件は`target/test-lib-full/`の台帳/JUnitと完了報告を正本とする。

一時変異はGit管理外の`target/d4-mutations.py`で6種類を個別に実行した。
対照の新規3回帰は成功（0）で、各変異は対象回帰1件がassertionで失敗し、全て終了100だった。

| 壊した処理 | 検出した回帰 |
| --- | --- |
| Offeredへ旧duration expiryを復活 | 2秒準備後もp=0から開始 |
| 進行率をaccepted_at原点へ戻す | 初回の実提出meshとp=0 recipeの照合 |
| hookではなくprojection時刻を公開 | projection→hookの80msの区別 |
| 後続curlでも原点を再公開 | 初回後のhook/replayで開始原点が一回だけ |
| 取消後のhookにも原点の公開を許可 | 終端ledgerがAwaitingのままであること |
| 終端canonicalを最大間隔から除外 | 1 curl→1秒hitch→canonicalの間隔 |

全変異を除去し、`page_turn.rs`を開始前のbytes/SHA-256と照合して復元一致を確認した。
結果は`target/d4-mutations-results.json`と対応ログに保存する。復元後の最終gateは
引数なしfmt、core check、fullscreen/geometry focused、UI snapshot、本体lib全件の順とする。
全件はホスト負荷による既知の不安定さを考慮し、初回から`NEXTEST_TEST_THREADS=4`で実行する。
製品/テストのretry・skip・回復処理は追加しない。全件の結果追記だけのためにソースを変更せず、
成功台帳/JUnitと最終報告で件数・終了コード・同条件再実行の有無を示す。

### 27.13 D2+D4 の実アプリ計測（2026-10-17 未明、ClaudeCode が隔離 portable で実行）

利用者が夜間の実アプリ検証を許可した時間枠で、`build-dev.ps1 -TestScript` の診断ビルドを使い捨て data-dir で起動し、
`target/repro/page-turn-budget-571-20261016/` の 03 / 04 を 1 回ずつ 8 回めくった (間隔 2.5 秒、設定 200ms、演出 ON)。
これは開発機での perf ログの計測で、利用者の実機での見え方の確認ではない。証跡は `target/page-turn-smoke/<run>/logs/perf_events.jsonl`。

| 条件 | 1 回の演出の curl frame 数 | 受理から初回 curl | frame 間隔の最大 |
| --- | --- | --- | --- |
| 04 単ページ、D4 前 (master `417e8331a`) | 6〜9 (早く始まった 2 回は 19) | 約 90〜100ms | 51〜64ms |
| 04 単ページ、D4 後 (`3c6ae74cb`) | 22〜26 | 約 70〜78ms (早い回 17〜19ms) | 38〜58ms |
| 03 単ページ、D4 後 | 18〜28 | 約 54〜60ms | 31〜59ms |
| 04 見開き (RtlCover)、D4 後 | 2〜26 (8 回中 2 回が 2〜3) | 約 62〜158ms | 42〜111ms |
| 03 見開き (RtlCover)、D4 後 | 3〜28 (8 回中 2 回が 3) | 約 35〜133ms | 33〜93ms |

- D4 で、準備が遅れた回も演出が最後まで進む (最終 frame が初回 curl の約 200ms 後)。単ページはほぼ毎 frame 描ける。
- 見開きでは演出中にも約 90〜110ms の停止が残り、最初の 2 回は 2〜3 frame しか出ない。2 頁分の通常準備が演出中に走るためと見ており、
  D3a / D3b (§27.5) の対象。D4 で解消したとは扱わない。
- 計測中に `scripts/page-turn-smoke.ps1` が現行の `--test-evidence-dir` 必須条件を渡しておらず起動直後に exit 2 になることが分かった
  (バックログ §1.390)。今回は一時的な写しで証跡先 (`target/ui-smoke/runs/` の 2 階層下、事前作成) を足して実行した。

### 27.14 無補正 source→edit→final の handle 共有（2026-10-17）

利用者は `a460aded7` の静的調査で提案した最小案を採択した。設計担当は edit 登録の
1 frame 1枚制限と既存の edit→final Arc 一致共有をコードで確認済み。調査対象04の無補正画像では
CPU pixels は source と同じ Arc のまま、edit の texture だけが再登録されていた。
同 frame の見開き2頁目は予算超過で final が別登録され、次 frame に edit が追加登録される。

修正は `ensure_edit_result_pixels` の新規組立て・CPU-only entry 表示昇格の2経路だけで、
upload 枚数制限前に現在 context の静止画 source handle を借りる。現在 key/item identity、
pixels の Arc 一致、texture の寸法/options、既存 display identity gate を条件にし、
不一致なら従来 upload へ進む。共有は upload budget を消費しない。final の entry/完成判定と
resolver は変更せず、既存の edit→final 共有へ接続する。契約は [display-pipeline §3.1](display-pipeline.md#31-詳細)。

新しい worker/queue/state field は不要で、表示操作や通常 producer を止める簡素化は採らない。
通常表示・一覧からの初回表示・演出OFFにも適用する。raw preview/animation、編集や透過 flatten の別 Arc、
Nearest 等の final sampler は従来経路を維持し、共有を理由に AI 未完了を complete にしない。
D2 の Native 素材には source handle を残さず、通常 cache の共有allocationは既存会計で一度だけ数える。
detached predicate/viewport と Remote の CPU adapter は変更しない。

回帰は小画像による ID・delta・世代・寿命の検査とし、同 frame の2頁、CPU先読み、upload予算消費済み、
Arc/寸法/options/key/identity の拒否、補正/AI/sharpen/post/Nearest、AI pending、編集待ち・透過、
source退去/通常再読込/兄弟context、VRAMと最後のowner解放、実drawのON/OFF・Single/LTR/RTL・resize、
Native ROI素材のsource解放を確認する。既存のROI/resize/canonical/ready回帰も関連focusedで検証する。
共有を壊す一時変異は復元後に最終gateを行い、結果・件数・終了コードは完了報告と
`target/texshare-*`、`target/test-lib-full/`の台帳/JUnitを正本とする。

新worktreeの検証準備では、固定SHA-256のLibRaw 0.22.2/zlib 1.3.1アーカイブを既存checkoutから
実体コピー・検証して展開した。RAW CC0 fixture 27件もmanifestのサイズ/SHA-256を照合して配置し、
FFmpeg 6 DLLを`target/debug/deps/`へコピーした。TurboJPEGのtest graph初回構築はMSBuildの
exit 1・診断なしで失敗したため、先行D4と同じturbojpeg-sys 1.2.0 / Debug / SIMD / static CRTの
静的出力を記録済みSHA-256と照合して実体コピーし、explicit static指定を用いる。
環境指定と準備scriptはGit管理外の`target/texshare-env.ps1`等に保存する。
これらの失敗はテスト0件のnative準備失敗として、製品テストの失敗と区別する。
全件は既知の同時負荷問題を避けるため初回から`NEXTEST_TEST_THREADS=4`で実行し、retryは追加しない。

独立Sol / xhighの静的完成レビューはACCEPT、未解消P1/P2なし（reviewerはテストを実行しない）。
追加fixtureの初回compileは`FsCacheEntry`（Dropあり）からのmoveでE0509となり、借用とhandle cloneへ
修正した。修正後の新規12回帰は全成功・終了コード0。共有を無効化する一時変異では同frame2頁と
CPU-only昇格の2件が失敗、Arc条件を外す変異では別Arc拒否の1件が失敗し、どちらも終了100だった。
`target/texshare-mutations.py`のfinallyで`app.rs`を変異前bytes/SHA-256と照合して復元した。
復元後はコメント整備と引数なしfmt、core check、関連pipeline/workerの限定検査、明示Pathのfocused、
UI snapshot、diff check、本体lib全件の順に実行する。`test-focused.ps1`の自動選択は広域入力を
検出して規定の終了2（実行0件）となるため、関連moduleを明示し、共有変更の保証はlib全件で行う。

利用者指示により build-dev・製品起動・コミットは実施しない。実GPU/present、04の改善幅、
滑らかさと実機F12は今回の自動検証では未確認。調査の登録時間・500/250MiB等は削減対象の見積りであり、
改修後のframe時間や改善率の実測値として扱わない。D3a/D3bの未解決契約は本変更で解消したとはしない。

### 27.14 source テクスチャ共有後の実アプリ計測（2026-10-17、ClaudeCode が隔離 portable で実行）

`94c6cdab2` (D2 + D4 + 無補正画像の source テクスチャ共有) の `build-dev.ps1 -TestScript` 診断ビルドで、§27.13 と同じ条件
(03 / 04、演出 ON 200ms、1 回ずつ 8 回、2.5 秒間隔) を測った。利用者の実機での見え方の確認ではない。証跡は `target/page-turn-smoke/ts-*/logs/`。

| 条件 | 1 回の演出の curl frame 数 | 受理から初回 curl | frame 間隔の最大 |
| --- | --- | --- | --- |
| 04 単ページ | 32〜33 | 3〜4ms | 10〜13ms |
| 04 見開き (RtlCover) | 31〜34 (初回だけ 18) | 4〜14ms | 7〜26ms |
| 03 単ページ | 32〜33 | 3〜8ms | 8〜13ms |
| 03 見開き (RtlCover) | 31〜33 | 3〜11ms | 10〜20ms |

§27.13 (共有前) の見開きで残っていた 90〜110ms の停止と、めくり始めの 200〜240ms の frame は出なくなった。原因は、無補正でも
edit-result / final が source と同じ画素の原寸テクスチャを登録し直していたこと (調査は本書の記録と `5ceb32dc0` のコミットを参照)。
補正・AI・カラー化などで画素が変わるページは従来どおり別テクスチャを作るので、その場合の滑らかさは別に測る必要がある (D3 の対象)。

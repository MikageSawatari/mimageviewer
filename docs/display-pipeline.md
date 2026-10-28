# 表示パイプライン (サムネイル / フルスクリーン)

### GPU の初期化（§1.448）

root GUI の描画 API は `--gpu-backend auto|dx12|vulkan` で選ぶ。既定の `auto` は surface 対応の
最初の DX12 adapter を試し、`request_device` が失敗した場合だけ最初の Vulkan adapter を一度試す。
明示指定はその API だけを使う。AppCreator は成功した device で一回だけ呼ばれ、以後の surface / pipeline /
描画の失敗、実行中の device lost、panic では開き直さない。成功した API の永続化もしない。
DX12 の shader compiler は root GUI・独立した動画 HUD ともに FXC 固定で、root の Vulkan 指定は HUD の
DX12 Instance や動画 native presenter（D3D11）を変更しない。
試行別の結果・GPU / driver・初期化中だけ収集する Error はログと起動記録へ残す。
この API 選択は報告環境の原因を確定した修正ではない。条件・診断 owner・テスト範囲の正本は
[GPU 初期化計画](gpu-init-fallback-plan.md)、[アーキテクチャ概観](architecture-overview.md#コア) を参照。

### 本の構成と提示許可（§1.242 B2、2026-10-13）

accepted構成は方向・表紙・可視実列・ページ指定を渡し、通常の共通builderでSpread/連結/editor/Remoteの組を生成する。
回転入力はitemsの投影workerで準備し、identityでlive memoを移して新facet列と同時に採用する。cold memoでも採用済みのページキー別入力を使い、UIからRotationDbを読まない。寸法後着と実効自動規則を反映し、休眠shiftは適用しない。Single/Split/portraitは同じ実列を投影する。
明示Aloneは表紙 (先頭単ページ) と同じ配置規則に従う (ページめくりアニメーション ON の間は本来の側、OFF は先頭単ページの全体・本別設定、横長は中央)。指定の直前で余って単独になったページは、本の末尾の単ページと同じ配置規則に従う (アニメーション ON の間は本来の側 = LTR 左／RTL 右、OFF は末尾単ページの全体・本別設定、横長は中央)。明示BlankOppositeは指定側+白紙で、白紙の側は白い紙としてめくる。形成理由をresolverまで保持して末尾表紙に置換しない。
自動強制単独の白紙置換は通常本と同じ。facet外の現在ページは単独の同builder入力として保持する。
Blankはsource/readinessを持たず、補助はNavigation/resume/exportの実列へ追加しない。
提示候補はcontext/世代/source/role/slot/sliceをworkerで確認し、相方はunit内でatomicに採用する。
通常open／navigation／slideshowは、採用済みsource投影と現在のservice token／identity indexを
証明でき、初期化済み・保守外・先行mutationなしなら、同じ提示許可owner内で新しいunitの
全occurrenceを即許可する。不明source／旧token／Checking／Hiddenは従来のworker照合へ渡す。
表示済みepisodeの許可を新規要求へ流用せず、UIのI/Oや別許可ownerは増やさない（紙めくり計画§26.18）。
Checking/Hiddenはcached processed/raw/thumbnail/holdoverからもtexture選択・paint・transform/hit・読了traceへ渡さない。
連結の測定extentと画像許可を分離し、bounded cacheの既知寸法だけで未許可画像を復活させない。
scroll/reanchor/resizeで探索を更新しても、画面に残るoccurrenceの許可とChecking要求は保持する。
測定された可視要求との差分だけで退出を破棄・新入場を照合し、cached画像も継続描画する。
類似候補の長押しプレビューは、Single/Spread/連結それぞれの許可済み表示を一時的に置き換える。
連結では通常の可視unit受付を共有し、Checking/Hiddenの元occurrenceをプレビューで迂回しない。
候補自身のidentityも既存preview workerで共通resolverへ渡し、完了採用時に最新scope tokenを照合する。
確認は共有serviceのFIFOへ受付し、初期化・maintenance・先行保存の後で返す。preview workerだけが
その私有channelを待ち、押下時の未初期化snapshotから拒否を決めたりUIで待機したりしない。
候補のcache/source更新証明だけを構成の許可証にせず、次の長押しでは新しく確認する。
類似queryや候補thumbnailの生成には構成lookupを追加しない。
非表示範囲は現在の要求内の最大20範囲と連続cursorへ限定し、退出・再入場では新しい許可を取得する。
Single→SpreadのChecking中は、同じdisplay cacheのtyped transitionへ捕捉した旧Allowed descriptorだけを保持する。
旧source/世代/tokenを照合し、新Pairの確定でatomicに置換する。新しい相方のcacheを先に表示しない。
指定サイズはEの既存geometryを使い、新fit参照の選択は許可後だけ。scroll/texture退去/resizeの既存参照は保持し、
構成更新/削除のidentity・role・slot・sliceが変わる場合は既存ownerで失効する。
PDFのtarget・bbox・表示解像度の準備も、pagedでは許可済み投影、連結では許可済みの各unitだけを入力にする。
Single→SpreadのChecking中は捕捉した旧表示を使い、新しい相方のPDF再描画要求を先に送らない。
trim左右別は実slotへ適用し、Single modeはsingle margin。投影前の未保存補正と通常保存・resetは
同じservice FIFOで既存view_trim.dbへ書き、受付から完了まで既存Edit ledgerのguardを保持する。
UIでDBを開かず、古い投影処理による上書きを防ぐ。詳細は[本の構成](book-composition-plan.md#5-表示projection)。

**これが一番事故が多い領域**。画像補正・AI アップスケール・回転・アニメーション・消しゴムマスクのすべてが、
「どのテクスチャを画面に出すか」を巡って絡み合う。修正前にこのドキュメントを読むこと。

---

### 紙めくりpresentationのA0実装 (§1.341、2026-10-11、受入れ待ち)

ページ間の紙めくりは`ui_fullscreen/page_turn.rs`のcontext単位presentationと、
`page_turn_animation.rs`のcanonical unit projectionへ分離する。既存navigation target、
ready判定、captureのfit/UV/occurrenceを正本にし、unit全体の通常prepare後にflat/curlを選ぶ。
保持する旧画素は提出済みimmutable paint resourceで、別のdecode/upload/resample経路を作らない。
共有weak lease inventoryはpresentation素材とprepare前の一時入力leaseを会計し、兄弟readerの状態を所有しない。
ONはeguiの最終output hookで破棄passを除外して実occurrence/source/nativeの提示receiptを採用し、
当該contextの次のpre-drawで既存ready observerへ届ける。配送時には元画像/通常final/passthroughの
既存選択、unit/layout、items/input generationを照合する。OFFは従来の同pass通知を保つ。
OFFは従来の瞬時切替、ONは単発手動/slideshowの即ready時を対象にする。連打・読込待ち・
連結・媒体境界は通常切替を保つ。時間は評価用100〜600ms・既定200msで、最終採択は未了。

projectionのstable geometryは描画先transformのeffective pppで比較する。独立に物理pxへ
丸めたpaint矩形の辺/見開き幅は1物理px以内の差を許容し、float完全一致を要求しない。
実際のpaint矩形/UVと終端の通常画素は保持し、clip変更、1pxを超える配置/寸法変更、
不完全なprepared ROI、可視面積不足は引き続きunit全体を同pass flatへ戻す。
skipは`sub_check`と両unitのpaint/clip/viewport/ppp/ROIをperf有効時だけ記録する。

この記録はA0差分の接続方針であり、実GPU提示・画質・速度・native lease・多窓受入れの証明ではない。
2026-10-12追補: native ROI端の丸めは元source texelの0.001以内だけを許容し、actual ROIへ
clampしてnative UVへ写像する。単頁は一leafだけが動き、下頁は自身のcanonical rect全体で静止。
自身の背（既定の案A）と見開きと同じ補間で移動する背を、環境設定の暫定項目で比較できる。
正本と検証記録はpage-turn plan §25。比較後は一方を削除し、実機motionの採択は利用者へ残す。
実装の独立完了レビュー、実機とOFF/ON性能比較は未完了。
正本は[page-turn-animation-plan.md](page-turn-animation-plan.md)、detached範囲は同rework plan §11。
既存`analyze_perf.py page-turn --check`のI1〜I5は維持し、専用`page-turn-animation --check`で
機会・開始/完走・skip率と可視提示/保持量を別集計する。合成ログの成功を製品性能受入れと扱わない。

2026-10-11 Stage 1実機追補: 単頁は読書方向で背を固定し（LTR左／RTL右）、forwardのoutgoing departureとbackwardのincoming arrivalを同じ側で描く。
実機ログと既存goldenは既にこの向きで、新goldenは固定端を明示する。終端RTLの背座標診断だけを訂正し、未再現の見え方を修正済みとは扱わない。
子viewportで捕捉しcallback外ROOTで受理するnavigationは、明示operation IDと現在destinationを照合して元factを保持する。
ambient frame stampは既存fieldの(ViewportId, frame_nr)で、viewportの違うcounterを比較して消さない。
固定矢印／FsPageNext/Prevはtyped consumerのIDを直接渡す。Repeat・境界のinstant規則、設定OFFは維持する。final-onlyは2026-10-11利用者決定で撤回（page-turn plan §22）。
根因・実機取消3件・回帰の詳細は[page-turn-animation-plan.md §21](page-turn-animation-plan.md)を正本とする。

2026-10-11 resident readiness追補: acceptは現在keyのresident complete/incomplete final、対応comic、既存faithful rendition、通常元画像/RAW preview、許可thumbnailを品質順に選ぶ。acceptでproducer/I/O/uploadは呼ばない。incomingもtyped resourceとしてpin会計へ入り、thumbnailのDirect属性をcache失効後も保持する。通常producer/poll/prepareは継続し、turnとcanonical flatは最初の素材・geometryに固定する。source品質publicationのinput epoch更新はcancelしないが、item/edit/params/comic/view/context bindingの失効は維持する。実素材のIDでnavigator/receiptを作り、flat採用後は通常selectorへ戻る。§22を過去のfinal-only記録より優先する。

2026-10-16 D2追補（実機未確認）: 通常`FullscreenPaintResource`とONの提出payloadを型で分離した。
prepare済み`FullscreenPresentationResource`はNative出力leaseだけ、または実際に描くManagedハンドルと
TextureOptionsを保持し、Nativeの原寸sourceをpinしない。受理時のNormal入力leaseは通常prepare後に
presentationへ置換してから再予算判定する。Flat/curl/canonical/batch/ready/navigatorがこの素材を共有する。
source ID・世代・元寸法は値のproofであり、raw cacheから再取得して描く権限にはしない。
実提出unitの内容bindingもreceiptへ値で保存し、遅延readyでmask/params/comicの変更を照合する。
OFFへ切り替えたON由来canonical再描画もreceiptのoriginで素材化し、原寸sourceを再保持しない。
source退去後もview/key/coverage一致時だけ保持出力で描き、不一致は通常thumbnail/loadingへ戻す。
元画像表示/分析のbypassでは通常selectorの結果だけを使い、held fallbackで加工済み素材を戻さない。
navigatorは全景を覆う出力を使い、本物のpartialでは同頁の通常cacheの
同じsource IDを一時参照し、なければ既存の許可thumbnailを使う。対象外のON表示/SNS previewは通常sourceを維持する。
通常source/派生cacheはmounted/at-rest/parked共通のread-only会計walkerが独立に数える。
等倍/Nearest拡大/出力上限fallback/大判Directは従来のManaged allocationを保持する。
OFFのsource描画、通常producer、accepted_at原点の時計は維持する。
prepare前には既存bindと同じview比較を行い、一致時だけ保持transform・寸法から通常cache keyを導出する。
同じtarget/smoothing/scale branch/ROIのNativeは既存出力を再利用し、新sourceのresampleをthawまで前倒ししない。
view/key変更時は通常prepareへ進む。詳細・検証はplan §27.11。

2026-10-16 D4追補（次版向け実装済み・実機未確認）: ONのmotion時計は最初の採用curl outputを原点とする。
受理時刻はレイテンシ専用で、Offered照合にduration expiryを使わない。Turningの排他時計は
AwaitingAcceptedOutputではp=0を提出し、operation固有ledgerが非discard outputで一回公開した時刻を
次のowning描画がRunningへ取り込む。追加frame/replay/mountでresetせず、以後はwall/durationで進める。
取消/closeの終端とoperation/destination照合が遅いhookの復活を防ぐ。素材待ちを増やさず、accept無素材はinstant。
flat-firstと§26.12のslideshow（短間隔instant、それ以外はcanonical flat採用後から保存間隔全体）、
OFF・連結読みの時計は維持する。perfの受理レイテンシとmotion統計の区別は[plan §27.12](page-turn-animation-plan.md)を参照。

PDF thumbnailの一覧失効・優先昇格はviewer owner/要求世代/keyに限定する
（未リリースPDF-EPOCH-OWNERSHIP）。seek/hoverもViewer ticketをUI admissionで捕捉し、
派生workerで更新しない。fullscreen本体はUnscoped＋既存fs_pending cancel/generation、
そのpromotionはUnscoped jobsだけをfoundと数える。兄弟同PDF/pageの表示要求は失効しない。
GPU upload、ThumbnailRequests、Canceled/SourcePixels/Finalizedの既存採用契約は維持する。
[詳細](pdf-render-epoch-ownership-plan.md)。

2026-10-18 D3-2: ON/paged の現在 unit の受理後だけ、通常 fullscreen work から次 unit の
一般 final を CPU 先回り準備する。入力は既存 CPU pixels のみで、新しい編集 materialization や I/O は行わない。
共有 final executor と同じ Triangle 表示 clamp を worker 上で実行し、正本/表示用 Arc を同じ pending の CpuReady に保持する。
登録は共通投機枠と実 options の VRAM admission を使う。表示要求は CpuReady を通常登録へ引き取り、
同期可能な未完成仕事は公開権を外して同期 takeover する。カラー化/LUT 待ちは同 worker を表示へ昇格する。
無補正 source 共有、初回/OFF の同 pass 完成、保持 presentation と canonical receipt は維持する。

2026-10-18 D3-1: final-effect の受信と採用を表示 decision から分離した。
各入力・描画・permission continuation・実表示再開の入口で、確定済み失効の後に
保持 presentation の accepted receipt、final 結果の exact 再照合・採用、通常 selector の順で処理する。
通常 source の selector 照合が必要な receipt は採用後へ残す。受信は非 blocking・再入可能で、
公開時は当該 viewport の decision cache だけを失効する。一般画像の exact complete final も
edit texture 確保前に再利用する。通常同期 miss、無補正 handle 共有、D2 素材、D4 時計は維持する。
この D3-1 段階では次 unit を通常 composition の実 occurrence/role から観測するだけだった。
現在は上記 D3-2 と下記 D3-4 の条件を満たす場合だけ先回り準備へ接続する。
slideshow の still-only unit 計算は表示 cache を置換しない。詳細・検証は page-turn 計画 §27.17.12。

### 次unitの編集素材の一括準備（§28 段階B、2026-10-28〜29、実装済み・検証結果は別記）

段階BはON／pagedの現unit提示後、Nextだけで局所補正→隠蔽→final→注釈を先回りして作る。
完成erase画素を入力にする場合はMI-GANや黒背景合成を再適用しない。after-AIは`used_upscale`に従い、
注釈はclamp前のfinal正本へ合成する。finalと注釈のupload画素はそれぞれ通常と同じTriangle clampで作る。
必要なfontが不足する場合もdoc解決は正常終端し、作れるlocal／conceal／finalは採用できる。
入力・key・世代・params・doc／fontのpreflight後、必要なtextureが揃ってから段・edit・final・対応comicを一括公開する。
final正本とcomicの下地は同じArcを持ち、段採用だけで上段を失効させない。

local／concealの段cacheはCPU-only entryを持てる。実体化済みはexact画素＋寸法正規化、描画可能はそれにtextureがある状態である。
resident lookupは副作用なし。表示ensureだけが既存画素を1 frame 1枚uploadし、texture不足を再レンダーやkey変更の理由にしない。
新規の投機textureはROOT共通枠で1枚ずつ登録し、途中の非公開textureも全contextのVRAM会計へ入れる。
登録残は次のROOT frameへ進め、OverBudgetでは結果を保持して再描画ループを起こさない。

表示要求は先に非blocking受信する。BのComputingは取消して通常経路へ戻し、CpuReady／Registeringは表示へ引き取って作成済みhandleを再利用する。
既存legacyカラー化／LUTの表示昇格契約は維持し、BをDisplayPreparedへ昇格させない。
Ctrl+Eを含む編集入口はBを取り消して既存ownerへ渡す。Bの不足を理由にめくりの受付を待たせたり演出を省いたりしない。
演出は受理済みのresident素材に固定し、品質公開でheld resource／canonical receiptを差し替えない。
設計と実装記録は[紙めくり計画§28.5](page-turn-animation-plan.md)、検証結果・未計測の表示引取りGPUコストは
`target/pageturn-1461/implementation-report-b.md`を参照する。

### 音声セルの代表画像 (§1.347、2026-10-08)

同名補助画像の採用は表示行の種類投影から独立している。画像種類を隠しても既存の候補順で
選んだ表紙／動画sidecarを維持する。通常／Remoteは一覧sort、Smart／集約は既存走査順を使い、
候補mapを表示画像で上書きしない（[候補採用規則](audio-album-art-plan.md)）。


Audio は既存 worker / texture backlog に乗り、同名 sidecar → MP3 先頭 ID3v2 の前面表紙優先 → 音楽アイコンの順で表示する。`ThumbnailState::NoArt` は正常終端で、Failed とともに一覧世代内では再投入・hover repaint を続けない。Loaded の画像寸法を Auto 比率へ使い、画像のない音声を分母から除く。全音声が終端なら Auto 未確定も現在の比率で終わる。

`ThumbMsgPayload` が Pixels / SourcePixels / ProvisionalPixels / NoArt / Failed / Canceled / Finalized を一つに所有する。Audio は保存完了後に終端 Pixels または画像なし終端を一通知で送り、source生成の二通知契約とは分ける。音声マークは `AudioThumbnailIndicator` の音楽アイコン (既定) / 文字バッジ / なし。画像なしではどの設定でも音楽アイコンを描く。音声を補正対象の画像ページにせず、再生画面・入力操作は保持する。[詳細設計](audio-album-art-plan.md)。


## 1. サムネイル表示パイプライン

### 1.1 状態機械

表示状態と要求の寿命は別である。context-local な `ThumbnailRequests` が request ID / items
generation・要求種別・未採用CPU画像を一体で所有し、sourceは `SourcePixels` の採用/破棄と
workerの保存/保存省略終端 `Finalized` の両方まで再要求を拒否する。cache / Audio / video /
直接画像の `Pixels` は採用/破棄だけで終了する。ドライブの `ProvisionalPixels` は検証終端を待つ。
Pending / Evicted / Loaded高画質化の表示状態から終端を推測しない。
uploadは既存の8枚/frameを維持し、範囲外動画はGridが採用、Rendition consumerが破棄する。
hideのCPU画像解放も同ownerのdiscardを通す。[寿命計画と検証](thumbnail-request-lifecycle-plan.md)。

`GridItem` 1 個につき 1 つの `ThumbnailState` (grid_item.rs) を持つ:

```
Pending ──────────(ワーカーがデコード)──────────▶ Loaded { tex, origin, rendered_at_px,
                                                           source_dims, layout_dims, source_coverage }
                                                       │
                                           (keep_range 外に出ると)
                                                       ▼
                                                    Evicted ─(再び可視範囲へ)─▶ Pending (再要求)
```

Failed は単発の終端ステート。デコードエラー時のみ。

`origin` はworkerの`ThumbMsg`とLoadedが共用する`ThumbLoadOrigin`で、
`SourceGenerated { evaluated_display_px }` / `SourceIntrinsic` /
`UpgradeableCache` / `FinalCache`を区別する。`rendered_at_px`は実際の画像長辺、
`evaluated_display_px`はその画像の生成に使った要求サイズであり、単位は同じでも意味が違う。
cache由来かどうかはoriginから導出し、独立した`from_cache`状態は保持しない。
画像なしのfinalized/cancel/errorはLoaded画像のoriginを更新しない。

`Loaded` へ遷移した時点で、全体のピクセル寸法と PDF の `layout_dims` を
per-context の `PageDimsCache` の別 map に記録する。部分編集 preview の全体寸法は
`source_coverage` の生成由来から取り、crop 後 `source_dims` は一覧の比率に残す。
通常素材は従来の `source_dims`（無ければロード済み画像寸法）を使う。
`layout_dims` は PDF page box の 1/1000 point であり、ピクセル寸法と単位を混ぜない。`Evicted` は GPU texture
だけを破棄し、同じ `items_generation` で判明済みの両寸法は残す。idx 空間が変わるときだけ
`invalidate_idx_state_and_queues` で clear し、cache 自身の generation 不一致も `None` へ
fail-closed する。

### 1.1.1 Windows の切り取り中表示

App-global な `CutClipboardObserver` が、現在の Windows file clipboard に
`CF_HDROP + Preferred DropEffect=MOVE` がある場合だけ、正規化済み実パス集合を公開する。
描画時は `GridItem::drag_source_path` が返す 1 パスをこの集合へ照合する。ファイル走査や
`canonicalize`、同期 clipboard I/O は描画経路へ入れず、ZIP / PDF 内ページ、Stack、
SearchContainer など実パスを持たない仮想項目は対象外とする。

サムネイルではセル背景を通常 alpha で描いた後、画像 / placeholder / 名前 / 評価 / 編集・タグ・
形式・再生等の内容 badge だけを opacity 0.5 の painter へ描く。選択枠、チェック、見開きカーソル、
hover と pointer owner は通常 alpha のまま後段で描く。詳細一覧も行背景、選択 accent、separator、
カーソルを通常 alpha にし、preview icon と列文字だけを 0.5 にする。詳細行 helper を共有する
下部情報バーは表示専用 caller なので opacity 1.0 を維持する。fullscreen 本体はこの projection を
参照しない。

### 1.1.2 サムネイル余白の一次クリック（§1.298）

既定OFFの「サムネイルの余白部分のダブルクリックで親フォルダへ移動」がONの場合だけ、
`grid_paint::draw_cell_with_hit_areas`が描画した画像・plate・label・overlayの矩形を同frameの
一次入力に返す。補正texture・回転・DPI・clipを別計算せず、letterboxと外周余白を背景にする。
bookmark title plateも実描画の矩形を返す。右クリック／右ドラッグとOFF時は従来のセル全体の判定。
押下のCell／Background所属は既存pairing ownerの型で持ち、永続hit cacheや別の背景ownerを作らない。
native D&Dはその押下時の所属を使い、読込み完了後の矩形で押下点を再判定しない。
小さいセルでplateからはみ出すcaption・理由・アイコンも、描いた領域のセル／一覧clip内を項目として返す。
代表画像のないフォルダ・ドライブ・ZIP内directoryは、アイコン周囲も描画ownerの`inner`全体を項目にする。
代表画像ありのletterbox、セル外周4pt、実際の空き領域の背景判定は維持する（§1.399）。
媒体別の境界・実描画順の検証は[決定仕様](grid-background-double-click-plan.md#描画領域とセル種類)を参照。

### 1.2 2 フェーズ優先ロード

`App::update()` 毎フレーム:

1. **keep_range の再計算** (`update_keep_range_and_requests` in `app.rs`)
   - 可視範囲 + `thumb_prev_pages` + `thumb_next_pages` を含む範囲を算出
   - `keep_start_shared` / `keep_end_shared` (`Arc<AtomicUsize>`) に書き込み → ワーカーが参照
2. **エビクション**: 同一 items 世代では前回 keep_set から外れた Loaded だけを
   `Evicted` に遷移 (GPU テクスチャを drop)。items 差し替え後の初回だけ全件照合する
3. **要求投入**: keep_range 内の Pending / Evicted に対して `LoadRequest` を作り
   - 通常キュー: `reload_queue` (Image/ZipImage/PdfPage)
   - 重 I/O キュー: `heavy_io_queue` (Folder/ZipFile/ConvertibleArchive/ZipDir — 全体走査、
     ZIP セントラルディレクトリ読み、または ZIP 内 prefix の代表解決が必要。PdfFile は PDF ワーカー IPC なので通常キュー)
   - 1 件以上キューへ投入したフレームは `update_keep_range_and_requests` 自身も repaint を要求する。
     通常は `App::update` 末尾の `requested_nonempty` と同じ役割だが、フルスクリーン中の
     early return で末尾まで到達しないフレームでも worker 結果を入力待ちにしないため。
4. **アイドル時品質アップグレード**: 入力・スクロールが500ms静止し、既存要求が完了したら、`UpgradeableCache`のLoadedを`LoadSourcePolicy::SourceOnly`で再要求する。`SourceGenerated`は実寸が`min(source_long_edge, current_display_px)`の80%未満、かつ現在の要求サイズがその画像の`evaluated_display_px`より大きい場合だけ再要求する。編集プレビュー、`FinalCache`、動画Shell等の`SourceIntrinsic`は対象外。ただし、`make_load_request`が親コンテナや手動ピンを最終targetへ解決した後も`SourceOnly`を保つ要求だけをupgrade queueへ入れる。動画ピン要求は`apply_folder_thumb_pin`が意図的に`CacheOrSource`へ変換し、フォルダ自動代表画像による上書きと無限再投入を防ぐ。対象外と確定したidxは現在のLoadedサムネイル/viewer contextに紐づけて記憶し、repaintごとのpin解決も行わない。この記憶は新しいitems世代、サムネイルの退去・再ロード、編集プレビュー更新で破棄する。`--perf-log`時は最終判定を`thumb.idle_upgrade_enqueue` / `thumb.idle_upgrade_ineligible`として記録し、同一key/idx/items世代の反復をリリース前`idle-health`検査で拒否する。
   - **同一要求の収束 (§1.198)**: 整数寸法選択は比率精度を優先するため、884×444への374px要求が247×124になる場合がある。実寸だけで不足判定すると同じ生成を永久に繰り返す。生成に使った要求サイズを画像と共に保持し、同一/縮小要求では再投入しない。374→400pxのような拡大要求では不足を再評価する。生成画素・0.05%の比率精度契約は変更せず、「十分な実解像度」ではなく「同一生成要求の完了」を保証する。
   - 要求サイズはworkerが実際に生成へ使った値を載せ、送信時やUI受信時の最新値に置き換えない。texture backlogもoriginをそのまま保持し、過去要求の最大値と合成しない。世代不一致・keep外で捨てた画像から完了coverageを公開しない。
   - 一覧ロード直後は `start_loading_items` が履歴スクロール復元後の位置で idle 判定をリセットし、
     親一覧へ戻った瞬間に古い idle 時刻で高品質再生成が走らないようにする。
   - **フレーム内境界レース対策 (2026-06-19)**: アップグレードの起動条件 (input/scroll が
     500ms idle) は `enqueue_idle_upgrades` がフレーム先頭で、`thumb_idle_upgrade_recheck_delay`
     がフレーム末尾で評価する。同一フレーム内で経過時間が 500ms 境界をまたぐと、enqueue は
     skip・recheck は `None` を返して egui が就寝し、**次のユーザー入力までアップグレードが
     走らない** (ESC で親一覧へ直帰した直後にサムネが低画質/灰色のまま固まる。perf-log で
     約 7.9 秒のフレーム停止 = `tail_repaint action=none reasons=[]` を確認)。`recheck_delay`
     は閾値に `MARGIN` (50ms) を足して wake フレームを閾値より後ろへずらし、起床フレーム先頭の
     enqueue が確実に enqueue できるようにする。BS で 1 段ずつ戻ると境界をまたがず発生しにくいが、
     ESC 直帰では再現性が高かった。

### 1.2.1 黒サムネ回帰 (v1.8.0) と texture delta 配送境界

**症状 (v1.8.0 で発生 / v1.7.0 では起きない)**: フルスクリーン (静止画 / PDF ページ) を
Esc / BS で閉じてグリッドへ戻った直後、一部のサムネが**真っ黒**になり ~0.5 秒固着する
(その後アイドル高画質化の再レンダで自然回復)。

**旧原因**: v1.8.0 のフルスクリーン viewport cleanup (`show_viewport_immediate` +
`Visible(false)` + recreate) では、`ctx.load_texture` が生成した texture delta を eframe / painter の
早期 return が捨てる境界があった。CPU 側だけが `ThumbnailState::Loaded` へ進み、GPU に中身が無い
ため `draw_thumb_texture` の黒 backdrop だけが描かれた。2026-06-19 には暫定的に font atlas resync
中の upload を `texture_backlog` へ先送りしていた。

**根本修正 (2026-08-14) と暫定策の撤去 (2026-08-27)**: `Painter` は presentation surface を
調べる前に delta を共有 renderer へ適用し、eframe も `egui_ctx.run()` 後に viewport / window が
消えていた 4 つの早期 return で `Painter::apply_textures_delta` を呼ぶ。したがって、生成済み delta は
「描画されない frame でも renderer へ一度だけ配送する」という所有境界で守られる。2026-08-27 の
実機計測でも font-atlas delta は全件 `site=paint outcome=Submitted` で、`SurfaceAbsent`、
`no_paint` site、`RenderStateAbsent` は 0 件だった。

この配送契約に caller 側の時間窓を重ねる理由は無くなったため、`poll_thumbnails` は resync 状態を
見ず、ready になった `ColorImage` を通常の frame upload 上限内で直ちに texture 化する。
`texture_backlog` は frame ごとの upload 上限だけを表す。回帰は caller の 4 early-return site、
painter の no-surface delivery、viewport cleanup 中にも thumbnail upload を遅らせないテストで固定する。

2026-09-27: 同じ shared painter の `set_window(viewport_id, None)` は、その viewport の
surface だけを破棄する。fullscreen host の再生成時に全 surface を消すと、visible な root は
egui pass を続けても `SurfaceAbsent` となり screenshot と表示更新が失われる。texture delta の
共有 renderer への配送契約は維持する。

### 1.3 ワーカー側の流れ

`thumb_loader.rs::process_load_request`:

```
1. 編集済みページでは `edit_preview_cache.db` の対応表を先に確認
   ├─ ヒット (`source_policy != SourceOnly`): 最大辺 2048px の edit-result 下地 WebP (q=90) と
   │   注釈レイヤー WebP (lossless) を同一寸法で display_px へ縮小 →
   │   下地＋注釈の ColorImage と表示時補正用の分離 payload
   └─ ミス: 通常の catalog へ続行
2. キャッシュ DB (catalog.db) に該当エントリがあるか確認
   ├─ ヒット (`source_policy != SourceOnly`): WebP バイト → ColorImage
   ├─ ミス + `CacheOnly`: 元ソースを開かず終了
   └─ ミス or `SourceOnly`:
        ├─ ソースデコード (RAW=LibRaw preview、必要なら executor で half 現像,
        │                   JPEG=turbojpeg, PNG/GIF/WebP/BMP=image crate,
        │                   AVIF=libavif→WIC、HEIC/JXL=WIC, PDF=PDFium ワーカー)
        ├─ EXIF Orientation 適用 (通常画像=path / ZIP=bytes。RAW は LibRaw の
        │                   flip 適用済み、PDF は対象外)
        ├─ Lanczos3 で display_px までリサイズ
        ├─ CacheDecision::should_cache でキャッシュ可否判定
        └─ 必要なら WebP エンコードして catalog.db に保存
3. RAW half は現像完了時に画像を持つ後続 LoadRequest を既存キューへ戻し、
   thumbnail worker が縮小・cache 保存を続ける。画像と finalized の 2 通を送信し gen_done を進める
4. mpsc で (idx, ColorImage, from_cache, from_edit_preview,
   edit_preview_adjustment, source_dims, layout_dims) を送信
```

### 1.3.1 非破壊編集プレビューキャッシュ

フルスクリーン編集を閉じる直前または別ページへ移動する直前に、source 解像度の
`edit_result_cache`（消しゴム／補正レイヤー／隠蔽加工まで）を snapshot する。専用 worker 上で
テキスト／スタンプを z 順の注釈ラスターレイヤーへベイクし、下地と各注釈レイヤーへ同じ保存済み
crop を実切り出しする。最大辺 2048px の下地は WebP q=90、透明縁と Multiply 係数を持つ注釈は
lossless WebP として分離保存する。保存先は `<data_dir>/edit_preview_cache/`、対応表と LRU は
`<data_dir>/edit_preview_cache.db` に保持し、crop 後の縦横寸法を一覧のアスペクト比として使う。
元画像と各編集 DB は変更しない。

§1.165 B1（2026-10-21、実装・自動検証済み・独立レビュー受入可・ClaudeCode の隔離環境で実アプリ確認済み、利用者の実機確認は未）では、生成時の収録範囲を
`source_coverage::EditPreviewGeometry` として preview 自身へ持たせる。座標空間は EXIF 適用後、
保存回転・自由回転前。`coordinate_dims` は正規ページ座標の元寸法、`raster_dims` は実際に
切り出す直前の raster 寸法、`pixel_bounds` はその raster に対する `CropRect::pixel_bounds`
の整数結果（min=floor、max=ceil）である。8192 clamp／PDF 再レンダ後の raster、crop 設定の
authoring 寸法、PDF page-box のレイアウト単位を正規ページ寸法へ読み替えない。
下地と Normal／Multiply 注釈には同一境界を適用し、crop 後の一覧寸法と生成由来を別に保持する。

既存の released v6 DB は `CACHE_FORMAT_VERSION=6` を維持し、nullable な
`source_coverage_json` と独立の `layout_dims_json`、生成物への束縛を示す
`metadata_content_hash` を加法 migration で追加する。
この追加で既存行や WebP を削除しない。
新生成は crop の有無にかかわらず `SourceCoverage::Known(geometry)` とし、WebP の参照と
同じ upsert で geometry と既知の layout 寸法を保存する。content hash にも由来を含め、同じ画素内容でも
元画像内の位置が異なる素材を区別する。旧行の NULL は `Unknown` のまま一覧で利用し、
次の通常生成で metadata が埋まる。
metadata は下地／注釈と geometry／layout を含む生成物の content hash に束縛し、
`cached_path` の WebP basename に含まれる hash と照合する。旧 writer の実 UPSERT は新列を
更新しないため、束縛が NULL／不一致なら coverage を `Unknown`、layout を `None` として
画像自体は利用する。同寸法の上書きでも古い由来を新しい素材へ適用しない。
hash の basename で照合するので、同一生成物の content-copy は由来を維持する。
束縛が一致した非 NULL の coverage／layout JSON が壊れている場合や寸法・境界が不正な場合は
ログを残してその preview の load を失敗させ、旧行扱いへの読み替えや行・WebP の削除はしない。

PDF の既知 page-box 寸法は `EditPreviewData.layout_dims` として coverage と別に保存・復元する。
保存要求は既存の page-box cache 値を渡し、通常画像と旧行は `None` のまま扱う。
preview hit の早期 return でも worker はこの値を thumbnail payload へ継承するため、layout を
整数 raster 比から再計算せず、preview 利用のための追加 PDF IPC／待機も発生させない。

coverage は「その素材が収録する範囲」であり、現在の crop DB／メモリ値から再構成しない。
crop A→B の保存後・非同期 Delete 前や、保存済み crop と未保存の生成入力が違う場合にも、
読み込んだ preview は自身の生成時点を示す。`Unknown` の表示可否は
`App::resolve_preview_coverage_for_display` 一か所で決める（現方針の純粋な判定は
`SourceCoverage::resolve_for_display`）。入口は owning App／idx／素材を受け取るため、将来推測方針を
採る場合もその入口の実装だけで切り替え、各 consumer の変更や同期 I/O を必要としない。現在 crop ありなら typed reason
`UnknownWithCurrentCrop` で fullscreen／通過表示／animation の代役から除外し、現在 crop
なしなら互換方針として `FullSource` を返す。これは一覧の旧 preview 利用を止める判定ではない。
worker受理では、この現在cropによる利用許可を寸法descriptorへ固定しない。crop中にUnknownを
受理しても、後でcropを解除すれば同じ素材を既存の互換方針で利用できる。

読み込み worker は保存サイズを品質原本として残したまま、現在の `display_px` へ下地と
全注釈レイヤーを Lanczos3 で同一寸法へ縮小してから合成する。UI / GPU には表示寸法の
画像だけを渡し、ページ数ベースの eviction と共有 VRAM 会計を実際のテクスチャ寸法に一致させる。
透過境界は `Color32` の premultiplied alpha を保ったまま縮小し、WebP 保存の直前だけ
straight alpha へ戻す。これにより Normal 注釈や透過画像の縁に透明黒が混じることを防ぐ。

注釈済みの `final_composite_cache` は流用しない。そこには色調補正・final AI・スマートシャープ・
ポストフィルタまで含まれるため、scene / font / stamp cache を worker へ snapshot し、
`edit_result_cache` から下地と注釈だけを作る。グリッドでは下地へだけ色調補正を 1 回適用した後、
Normal / Multiply の注釈レイヤーを z 順に合成する。これにより fullscreen と同じ
`edit -> color -> comic` を保ち、文字・スタンプの色がページ補正で変化しない。

色調補正をキャッシュに含めないため、グリッドの既存 `thumb_adjust_tex` を一度だけ適用でき、ページ
補正を後から変えても重い編集プレビューを再生成せず追従する。編集データを更新した時点で旧 preview
を非同期削除し、終了時に最新の完成済み結果だけを保存する。source の mtime / size が変わった場合も
load 時に失効する。削除／全消去通知を受けた UI も、メモリ上の古い編集 preview を即時破棄する。
保存完了通知を受けた UI は該当セルを `Evicted` に戻して読み直し、同時進行して
いた raw decode が後着しても `from_edit_preview` が立つまで再試行する。

閲覧だけで viewer を閉じた場合は、永続 preview 行が実在して削除されたときだけ UI へ失効を通知する。
行が存在しない通常サムネイルまで `Evicted` に戻すと、開閉したセルだけが灰色へ戻って再 decode される
ためである。一方、編集データの明示的な更新では、preview 行が未生成でもメモリ上の派生表示を破棄する
必要があるため、従来どおり必ず失効を通知する。この二つの削除契約は service command で区別する。

親コンテナの代表サムネイルを手動 pin した場合も、cascade 解決後の leaf が固定の
Image / ZipEntry / PdfPage なら同じ preview を通常 catalog より優先する。直接ページは
page mtime + size、ZIP/PDF の親代表は保存 worker が記録した container size と thumbnail
worker が読んだ現在の container mtime + size で stale 判定する。変換アーカイブでは元
アーカイブではなく、実際にページを読んだ cache ZIP の key / identity を使う。編集 preview
通知は直接ページだけでなく、その leaf を固定している親セルにも伝播する。固定 leaf の個別
色調補正 key は編集 preview の有無とは独立して load request へ渡し、thumbnail worker が下地へ
補正を適用してから注釈を合成する。一括補正の変更 / 解除も該当 leaf を固定する親セルを失効させる。
自動選定代表への反映は別 backlog とする。

既定は有効・上限 1GB。上限超過時は最終アクセスが古い WebP から削除する。encode、ファイル I/O、
SQLite 更新、LRU prune はすべて専用 worker 上で行い、UI スレッドをブロックしない。

`display_px` の算出には main egui Context の実効 `pixels_per_point` (= OS DPI × UI 表示倍率) を
使う。そのため「設定 → スケーリング」を上げるとサムネイル / 表示用デコード解像度も上がり、
高倍率ではメモリ使用量が増える。要求サイズは 256〜2048px に clamp されるため、UI 表示倍率を
200% にしても 2048px を超えて増え続けることはない。

### 1.3.2 親コンテナの代表サムネ — 優先順位

親コンテナ (Folder/ZipFile/PdfFile/ConvertibleArchive) の代表サムネは次の順で決まる:

1. **手動ピン (`folder_thumb_pins.db`)** — ユーザーがアドレスバー 📌 ボタンや
   右クリックメニュー「📌 代表サムネに固定」で指定した子アイテム。`make_load_request`
   の `apply_folder_thumb_pin` が `LoadRequest` を target アイテム用に書き換え、cache
   key は `{base_key}#pin:{source_id}` に変わる。直接ページなら source_id は
   kind/rel/entry/page/mtime/size の compact 表現、子のフォルダ / ZIP / PDF / ZipDir に
   さらに代表 pin があれば、最終ページまでの経路 hash + leaf identity になる。pin の
   付け替え、途中コンテナ、target ファイルの変更で source_id が自動的に変わるので
   古い WebP を catch しない。固定 leaf が Image / ZipEntry / PdfPage なら、そのページの
   編集 preview と個別色調補正も親コンテナまで引き継ぐ。詳細:
   [virtual-folders.md §3.1.1](virtual-folders.md#311-親コンテナの代表サムネピン-folder-thumb-pinv09x)。
2. **自動代表選定 (`resolve_folder_thumb_image`)** — Settings の `folder_thumb_sort`
   (Numeric / Modified / etc.) + `folder_thumb_depth` (再帰深さ) で先頭画像を選び、
   通常のサムネ生成パイプラインに乗せる。pin が無い場合の既定動作。キャッシュ
   ヒット時は表示速度を優先して毎回の再スキャンは行わないが、cache key には
   自動選定アルゴリズム版・sort・depth を含めるため、番号順ロジックや設定が
   変わったときは自然にミスして再スキャンされる。キャッシュミス時の Folder
   自動選定は、グリッドのブロック順に揃えて「サブフォルダ (folder_thumb_sort 順) →
   直接画像 (sort 順)」で候補を辿る。途中の子フォルダに非 Image の代表 pin がある場合は、
   子が直上一覧で既に生成した完全一致 WebP だけを読み取り専用で再利用する。WebP が無い・
   stale・破損の場合は PDF/ZIP/動画を生成せず、そのまま従来の画像候補探索を続ける。
   再利用した WebP は `ThumbLoadOrigin::FinalCache` として UI へ渡し、idle
   quality-upgrade が元 PDF 等を再生成しないようにする。既に有効な上位キャッシュはこの
   再探索より先に通常の cache hit となるため、従来どおりそのまま使う。
   ConvertibleArchive は有効な変換キャッシュ ZIP がある場合だけ、その ZIP の先頭画像を
   `archivethumb:{format}:{identity}` キーで読む。キャッシュ未作成/失効時は要求を出さず
   アイコンに戻す。
3. **フォルダ / ZIP / PDF / アーカイブアイコン fallback** — 中身が空 / 全部エラーで上 2 段が失敗
   したときの最終フォールバック。`grid_item.rs` の draw_cell でアイコン表示。ZIP / PDF /
   RAR 等の形式は中央アイコンと独立した左下バッジでも示す。形式バッジは従来サイズの約
   70%（小さいセルでは可読性のため 7pt を下限）とする。フォルダ名バッジは可変長 CJK の
   可読性を残す専用スタイルで、旧サイズの 85%、8.5pt 下限・13.5pt 上限、横 padding
   0.30em・縦 padding 0.12em とする。形式バッジと機械的に同じ 70% にはしない。

サムネイル四隅 overlay の計測・padding・配置は `src/thumb_overlay_layout.rs` の
`ThumbnailOverlayLayout` が一括で所有する。チェック印の領域とスタック枚数を先に確保してから左上を配置する。左上は
ブックマーク時刻 → 動画 `UP` → 編集状態 / pin → タグの順に実測幅を予約する。幅不足時は
時刻・`UP` を省略表示し、編集状態は順序を保って入る分を置き、残りを ASCII の `+N` にまとめる。
タグは残る幅に応じて省略または非表示にする。描画とタグ hover / click は同じ `BadgePlacement.rect` を
使う。左下はフォルダ名または形式バッジとファイル名プレートを同じ下段へ配置し、その実測行の
上へ評価を積む。評価の星が収まらない幅では数値へ短縮し、セル外や優先順位が高い左上の表示に
重なる場合は左下の低優先要素を表示しない。右下の絞り込み件数は左上・左下と重ならない位置まで上げ、余地がなければ省略する。色・角丸・フォントなど各要素の見た目は
`ui_helpers.rs` の個別描画関数が持つ。
スタック枚数と絞り込み件数も狭幅では文字を短縮し、`draw_cell` の内容はセル内に clip する。
共有設定 `thumb_show_resume_meter` がONなら、`ThumbnailOverlayLayout` は本・動画・音声・画像・その他の
全セルに固定下端帯を予約する。位置記録なし・長さ未取得でも同じ予約を行い、設定boolを
`fraction.is_some()` へ置き換えない。帯は `inner = cell.shrink(4)` の全幅、高さ9 logical pt、
下端 `inner.max.y` (セル下端から4pt)。描画時は左端ceil・右端/下端floorでphysical pixelへ揃え、
上端を丸めた下端から `floor(9 * pixels_per_point)` pixels戻す。上端まで元の矩形の内側へ丸める規則ではない。
100/125/150/200%の厚さは9/11/13/18px (9/8.8/8.667/9 logical pt)。保存済み位置の有効な比率がある本のFolder/ZipFile/PdfFileと
動画/音声だけ、不透明な角丸バーを描く。位置のないセルはtrackも描かず予約した空き領域を残す。
ONでは左右の下端ラベルを一律13 logical pt上へ移す。右下の従来基準 `cell.max.y - 3` に対し、
9ptの帯と最低3ptのgapを確保する量は `9 + 3 + (4 - 3) = 13`。右下のgapは3pt、
左下の従来基準 `inner.max.y - 3` からのgapは7ptとなる。ファイル名・形式・フォルダ名・評価・
長さ・絞り込み件数に同じ予約量を使う。純layout外のSearchContainerの件数とCollectionPlaceholderの
単行captionも13pt移動する。SearchContainerの代表画像なし・未ロードの多行階層パスだけ、上端を固定アイコン下の元位置に保ち、
下端だけ13pt縮めた矩形で既存階層layoutを再計算し、親階層から省略して末端の本/フォルダ名を優先する。
代表画像ありでは、もともと小さい固定種別アイコンより下にある背景/パス全体の13pt移動を維持し、高さを縮めない。
通常サイズで末端名の全消去を許さない。
単行captionはON時だけ描画と同じpixel原点のgalley inkを確認し、cell・帯・固定主アイコンに
収まらなければ省略する。代表画像なし・未ロードの階層パス全体を上へ移して交差時に丸ごと消す方式は使用しない。
代替位置を探さず、収まりはviewport clipでなくcell境界で判定する。
SearchContainerの代表画像ありのラベル背景はONなら元の上部アイコンより先に描き、OFFは元の順序を維持する。
階層パスは `ui_helpers::layout_path_hierarchy` の既存計算を共有するため `pub(crate)` 可視性だけを変更する。
OFFでは予約せず従来位置へ戻す。同じ設定状態では、媒体種別や
バーの有無で帯・ラベルのgeometryを変えない。極小セルでは既存の低優先badge省略を使い、
帯の位置・厚さは固定のまま既存のcell clipを適用する。
画像fit・中央の再生/音楽アイコンの位置/大きさは維持し、画像幅との対応・バーの空き領域の上移動探索・実glyph/アイコンのobstacle追跡を行わない。
通常セルの描画順はサムネイル/内容→バー→媒体アイコン/切り取りマーク→通常ラベル→選択/見開きcursorの枠→check。
固定予約帯とラベルは交差しないため、バーをラベルより先に描く。実際にバーがあるセルだけ主マークを
バー直後・通常ラベル前まで遅らせ、極小セルでバーが主マークを隠さず、主マークがファイル名を覆わない従来の優先関係を保つ。
SearchContainer / CollectionPlaceholderの特殊captionは内容描画内の上記規則に従う。OFF/位置なしでは元の順序を維持し、
枠とcheckのopacity、cutのcontent opacityも従来どおり。
セル高・画像fit・並び順・hit-testは変更せず、内容bitmapへ焼き込まない。色は
`os_theme::book_resume_meter_palette`。採用fillはLightがRGB(38, 67, 122) / `#26437A`、
DarkがRGB(142, 176, 234) / `#8EB0EA`。選択strokeのRGB(60, 120, 220)と明度/彩度を変え、
緑のFolder badgeとも区別する。track/boundaryは従来の不透明灰色を維持する。fill/trackのコントラスト比は
Light 7.91 / Dark 6.26、fill/選択strokeはLight 2.27 / Dark 1.94。
メーターは常に左から右へ伸び、`fullscreen_seek_direction` と本の読み方向には連動しない。
本の比率はHUDの読み順で記録したanchor ordinal/totalで、見開きの相手ページは加算しない。
動画/音声はliveのwatched集合にあれば長さ不要で満タンとする。それ以外はliveの再生位置表と現在source stampに一致した取得済み長さから求める。
watchedなし・位置なしならバーを出さない。途中位置は長さ不明・失敗/不正比率ではバーを出さないが、ONなら帯予約とラベル位置は維持する。
長さバッジOFFでも共通の前回位置設定がONなら既存の可視+近傍workerで長さを取得する。
本は起動時writer全行readと稀なDB変更後read以外はAppのpath memo/mapだけを参照する。
動画/音声用の位置mapは複製せず、描画からDB/ファイル/FFmpegへは到達しない。
未読・NULL・不正値はtrackも出さず、設定OFFでも位置の記録は続く。動画/音声の長さ取得は、長さバッジまたは共通の前回位置設定がONのときに要求する。
新しい状態・設定・worker・DB I/Oは追加しない。詳細と今回の検証記録は [book-resume-meter-plan.md §14](book-resume-meter-plan.md#14-全セルに固定下端帯を予約する方式へ再改訂) を参照する。
補正済みサムネイルの生成は `thumb.adjustment_build` perf event で色調処理と
`ctx.load_texture` を分けて計測できる。`origin=visible` は一覧描画中、
`origin=prefetch` は可視外の先読み、`n` は `frame.begin` と同じ更新フレーム番号。
元サムネイルが未ロード（トレイ格納による退去を含む）の間は生成しない。

4K・Windows 175% で 10 列と 20 列の実機負荷を比較する場合は、`--perf-log` 付きで起動し、
同じ画像フォルダと補正設定を使う。各列数で画面を静止させ、補正パネルの**グローバル標準**の
明るさを変更して確定する。この経路は `copy_params_to_global` → `clear_all_color_caches` を通るので、
全補正キャッシュ消去直後の可視セル再生成を記録できる。10 列と 20 列はそれぞれ起動し直して個別のログで測り、
`%APPDATA%\mimageviewer\logs\perf_events.jsonl` を各測定後に別名で保存し、
`python scripts/analyze_perf.py <perf_events.jsonl> thumbs` で可視/先読み・画像寸法別の
`thumb.adjustment_build` 件数と、可視セルだけの 1 フレーム合計 (`session` / `n`、
`apply_ms`・`texture_ms`・`total_ms`) を見る。比較は最大値と上位フレームの `cells` を使い、
先読みの合計を可視フレームへ混ぜない。
`python scripts/analyze_perf.py <perf_events.jsonl> hitches --ms 33` でフレーム間隔も確認する。
イベントの `texture_ms` は egui のテクスチャ作成までで、GPU 転送時間を直接表さない。
headless の ignored 計測テストも、可視セル補正生成の連続処理だけを測り、フレーム全体の描画時間と GPU 転送は測れない。

**Video ピンの特殊経路**: pin source が動画の場合は `seed_folder_video_pin_thumbs`
が起動時に `video_pins` DB の抽出済み WebP を pinned cache key として catalog にミラー
seed する。worker は通常の cache_hit で取り出すので、Shell API を再呼び出ししない。
`CacheOrSource` 固定で idle quality-upgrade の対象外。**動画 folder pin は
`video_pins.db` に WebP がある (= フルスクリーンで `P` キー / HUD でフレーム保存済み)
場合のみ set 可能** — sidecar / Shell 抽出を UI スレッドで同期実行しないための仕様
(Codex post-merge P2)。詳細:
[virtual-folders.md §3.1.1](virtual-folders.md#311-親コンテナの代表サムネピン-folder-thumb-pinv09x)。

**ドライブ一覧用の保存例外**: ドライブルート (`C:\` 等) を通常フォルダとして
表示している間、そのドライブの手動ピン代表そのもののセルだけは
`LoadRequest::force_cache` を立て、`CachePolicy` が Auto/Off でも親 catalog に
サムネを残す。ドライブルート catalog はドライブ文字を保持して分離し、直下同名項目の
取り違えを防ぐ。ドライブ一覧ビュー自体はこの catalog / `video_pins.db` を cache-only で
読むだけで、表示時にドライブルートの `read_dir`、代表探索、metadata 確認、ピン先
デコードは行わない。root catalog に正しいフォルダタイルが無い場合は、子 catalog へ
深掘りせずアイコン fallback にする。

### 1.4 表示時の変換

サムネイルには以下が適用される:

| 変換 | 適用場所 | 備考 |
| --- | --- | --- |
| 回転 (DB) | 描画時の GPU 行列 | `get_rotation(idx)` で毎フレーム参照、結果は `rotation_cache` にキャッシュ |
| EXIF Orientation | **デコード時**に適用 | 通常画像はファイル path、ZIP 内画像はエントリ bytes から読む。PDF は対象外 |
| 非破壊編集結果 | **編集プレビューキャッシュ** | 消しゴム・補正レイヤー・隠蔽加工・テキスト／スタンプ・切り取り。編集終了時に非同期生成し、次回一覧から利用 |
| プリセット補正 (色調のみ) | **UI スレッド同期適用** | `thumb_adjust_tex[idx]` に保持、§1.5 参照 |
| カラー化 | **適用されない** | 最終表示専用。トーン濃淡変換を含むため worker で処理 |
| ポストフィルタ | **適用されない** | コスト/実装維持のためサムネは色調のみ |
| AI アップスケール | **適用されない** | 1 枚 10 秒級のためサムネでは非現実的 |

### 1.5 サムネイル補正パイプライン (色調)

「黄ばんだ紙のスキャンをモノクロ漫画補正して見る」等、サムネ一覧とフルスクリーンの
見え方を揃えるため、グリッド描画時に色調補正を適用する。LUT 路ならサムネサイズ
(600 px 級) で ~3ms/枚 で済むが、70 枚同時に UI スレッドで掛けると 200ms 級の
フリーズになるため、以下の構造にしている。

**対象**: 画像系グリッドアイテム (`Image` / `ZipImage` / `PdfPage`)。自動選定されたフォルダ /
ZipFile / PdfFile / ConvertibleArchive の代表サムネには適用しない (`adjusted_tex` は `None` で
素通し)。手動固定した親代表だけは、解決済み leaf の個別色調補正を thumbnail worker 内で
下地へ適用する。

**データ構造** (`App` 内):

- `thumb_pixels: HashMap<usize, Arc<ColorImage>>` — 補正のソースとなる生ピクセル。
  `poll_thumbnails` でテクスチャアップロードと同時に `Arc::new` し格納する。
  `keep_range` 外に出たら drop (§1.2 のエビクション時)。`is_video` な idx は対象外。
- `thumb_adjust_tex: HashMap<usize, TextureHandle>` — 補正済みテクスチャ。
  `effective_params(idx).is_color_identity()` が `true` の idx は**エントリを持たない**
  (= 生サムネがそのまま描画される)。ポストフィルタは判定に含めない (サムネ非適用のため)。

**適用タイミング**:

1. **可視セル**: `ui_main::render_grid` がセル描画直前に `maybe_apply_thumb_adjustment(ctx, idx, "visible")`
   を同期呼び出し。`thumb_adjust_tex[idx]` がすでにあるか identity ならスキップ。
2. **先読み分 (keep_range 内, 非可視)**: `update()` 終盤で `process_thumb_adjust_budget(ctx, 8)`
   が最大 8 枚/フレーム処理する (600px で ~3ms/枚 × 8 = 24ms 予算)。
3. **スライダードラッグ中** (`App::adjustment_dragging == true`): 両経路ともスキップし、
   `draw_cell` も `adjusted_tex = None` を渡して生サムネを表示。
4. **ドラッグ解放**: ページ個別の色調変更なら対象キーの補正 tex だけ失効させ、
   次フレームに必要な可視セルだけ再生成する。標準の変更は一括失効する。

**キャッシュ無効化** ([preset-and-adjustment.md §4](preset-and-adjustment.md) の早見表に追補):

- `clear_adjustment_caches(idx)` 経路で `thumb_adjust_tex[idx]` も落とす。
- `clear_all_adjustment_and_ai_caches(idx)` 経路も `thumb_adjust_tex[idx]` を落とす。
- バルク系 (`apply_params_to_all_pages` / `clear_all_page_params` / `copy_params_to_global`) は
  `thumb_adjust_tex.clear()` で全部落とす (対象 idx を絞る判定より単純かつ安全)。
- `start_loading_items` (フォルダ切替) で `thumb_pixels` と `thumb_adjust_tex` を全クリア。
- **ピクセル (`thumb_pixels`) は keep_range を出るまで保持**。補正テクスチャだけ
  捨てて差し替えるのが基本、ピクセルを捨てるのは範囲外 evict とフォルダ切替のみ。

---

### 1.6 静止画シーク列・プレビューの保存回転

列と hover preview は `RotationCache` の確定値だけを読み、未読値は
`start_still_seek_rotations` → `still-seek-rotations` worker → `poll_still_seek_rotations`
で取得する。通常画像・ZIP・PDF・変換アーカイブとも本体と同じ `page_path_key` を使う。
DB の read-only open / SELECT は worker 内に閉じ、UI は要求のキー作成・`try_recv`・
メモリへの merge だけを行う。回転キャッシュが全件ヒットした一括取得も DB ハンドルへ進まない。

回転が未取得の Loaded サムネイルは列では `Unavailable` とする。現在ページから外へ育つ
既存レイアウトがその側で止まり、0°で一度置いてから回転後のセル幅へ動くことを防ぐ。
サムネイルの `Failed` は従来の正方形セルのまま。DB open / SELECT 失敗は worker で診断し、
従来の回転取得と同じく `Rotation::None` を確定値にする。回転は付加情報であり、読み出し失敗で
列を永久に止めず、そのセッションでの閲覧を継続するためである。再取得は通常の失効境界で行う。

見開きの組み合わせとページ番号も回転に依存する。overlay はこの間接経路も同期 SELECT へ
進めず、nav の未読回転を 1 worker 最大 256 キーで順次準備する。初回で表示単位がまだ無い場合は
読み込み表示とし、見開きの組み合わせを推測しない。既に同じ nav / items 世代 / 見開きモード /
ずらし位置の表示単位がある場合は、その snapshot を再取得中も使って入力とラベルを維持する。
これにより回転失効を挟んだドラッグの release を欠落させない。セルとプレビューの向きは引き続き
確定値だけを使い、未取得セルの側で止める。確定済み nav の確認は同じ immutable `Arc` のメモで省略し、
idle frame で本全体を再走査しない。本体側の見開き解決規則・入力・着地の仕様は変えない。

プレビューの 1 枚は `StillSeekPreviewPage { texture, rotation }` として渡す。吹き出し・画像枠は
`rotated_display_size` で 90° / 270° の縦横を入れ替えてから決め、描画は本体・列と共通の
`draw_rotated_image` を使う。見開きも画面上の左右順を保ち、2 枚それぞれの回転と寸法を使う。
テクスチャまたは回転が未取得のプレビュー枠は「読み込み中…」とする。
`still_seek_preview_size` と `still_seek_preview_size_values` は吹き出しの表示寸法だけを変え、
既定の最小 90 / 小 117 / 中 144 / 大 180 / 最大 360pt は従来の画像枠高 180pt の
0.5 / 0.65 / 0.8 / 1 / 2 倍に当たる。画像枠の最大幅は高さ × 240/180 とする。
既存のサムネイル読み込みと見開きの構成を変えない。上部バー・下部バー・列と画面端の
間に収まるよう、描画時に縦横同率で縮小する。

回帰は `src/ui_fullscreen/tests/still_seek_rotation.rs`。実 overlay を実行したスレッドの
RotationDb read-only open / SELECT 回数が増えないこと、未確定セルの停止と worker 完了後の
向き、失効中の release 入力、終了・context 交換、実テクスチャの UV と矩形を検査する。
`still_seek_real_overlay_rotation_*` の PNG 6 枚で本体・列・吹き出しを同時に比較する。

### 1.6.1 閲覧中のページ一覧 (§1.340、2026-10-27 独立レビューP2追補・指定自動検証済み)

通常 fullscreen／メイン内閲覧／F12連動窓／独立窓は既存surface内で
`ui_page_overview::draw_page_overview` を使う。現在の可視実ページ列を仮想行として描き、
未生成／回転未確定／失敗セルも番号とidentityを持つ選択可能なplaceholderにする。
列数・scroll・cursorはviewer内のセッション値で、入場／取消は読書位置とmain Gridを変更しない。
新規入場とSelectは既存`page_overview_available()`へ集約したnavigation-sequence／page-turnの
両admissionを共用し、animationのON／OFFにかかわらず未提示の受理済み移動があれば拒否する。
通常出力の採用と受理済みcontinuationを保持する。O／HUD／ring／padは共通受付を使い、
完了後にもう一度操作する。入場要求の保留・replay・新しい取消処理は作らない。
Oの既存operation観測が通常flat完了を始める場合も、入場受付自体はpage-turn ownerを変更しない
（2026-10-27の新coordinator手順2、一覧設計書§25.4）。旧選択肢2の即退役は採用しない。
独立レビューP2-1の通常表示待ちへの拡張とP2-2の前面キー／共通chrome終了の指定自動検証は同§25.7で成功。
新しいpredicate・pending・replay・取消は作らず、HUDのenabledはavailability、理由は既存optional reasonを使う。
受理した共通入場だけが`dismiss_fs_transient_menu_state()`でspread／fit／panoramaのApp boolと
overflowを終了し、通常`close_fullscreen`も同helperを使う。keyboard次キー／HUD Close／HUD Overviewの
overflow-only終了は`dismiss_fs_overflow_panel()`へ揃える。rotation／seek等のegui Popupは
表示／keepされなくなった所有viewportの`Memory::end_pass`で自然に退役するため、明示popup closeや
兄弟viewportも消す`Popup::close_all`は加えない。keyboard seekの既存優先、context／modal／edit／pickerの
入場拒否、physical stream、info lock、拒否されたring入場の旧chromeは維持する。
front pointer captureは`fs_front_menu_presented`とoverflow rectだけを読む。通常adjustment／metadata／
infoはその集合外で、非focused adjustmentは入場可能な場合もある。focused TextEdit／IMEは既存ownerで止める。
tag pickerは既存common modal、overlay編集は既存availabilityの拒否を使う。hover tooltipは
PopupKind::Tooltip／Sense::hoverと既定inactiveのArea／描画寿命であり、durable panelとともに一括resetしない。
Selectはstamp／binding／items・projection revision／incarnation／stable identity／active資格の
既存照合を全通過したら一覧をClosedにし、`land_still_page_navigation_target`の`LatestSeek`を
ちょうど一度呼ぶ。即時着地・Checking後許可・後着拒否・同期拒否は通常navigation ownerが扱い、
一覧の再open／待機／追加pending／受付証明を作らない。通常／連動のGrid追従、独立窓の
main非干渉も通常ページ移動と同じ時点・規則で扱う（2026-10-27 coordinator判断、一覧設計書§25.1追補）。
`LatestSeek`は既存seek／jumpと同じく、隣ページでも新curlを開始しない。
`offer_page_turn_with_operation`の対象は`Sequential`だけ。選択先の通常flatが実表示受理された後、
D3の次unit先読みは`prefetch_next_final_unit`の既存ON／paged／non-split／入力素材資格へ合流する。
一覧専用policyや、同期停止・Checking後の結果から一覧を復元する経路は追加しない。

一覧を描くCentralPanelのearly returnより前に、既存共通入力・案内へ到達する経路を持つ。
`ui_fullscreen/viewer_chrome.rs`は通常viewerとactive一覧が共用するprimary suppression終端、
右ドラッグ／gesture認識、中ボタン短押し、F11 borderless step、overlay／dialogの境界である。
physical primaryはviewerのCentralPanel入口でkey handler前に一度advanceし、終端frameの
抑止decisionを通常pointer／一覧へ渡す。Esc／Oで一覧が閉じてもPointerStreamのreleaseを
取り逃さず、そのframeの背面clickを抑止したまま次の通常clickへ戻す（一覧設計書§24）。
2026-10-27のmaster合流では、CentralPanelで一度captureした同frameのfront pointer入力値を
primary終端／一覧／通常canvasへ渡し、一覧のright recognizerはmasterのrecognize／apply分離APIを使う。
old-owned releaseの実renderer3surface回帰は追加したが未実行（一覧設計書§25.3）。
通常callerのguard・タイミング・描画順を維持し、一覧は保存済み割当を一覧設計書§4.1の実行可能集合へ配送する。
mouse ring／gesture／X長押しring、feedback、既存job進捗・picker／modalは既存guardで描く。
passive guideは描画元のimmutable `overview.executable_actions`を使い、activeな別窓の集合を流用しない。
F11のPrimeBlackFrame→ApplyWindowMode→Settleも同じconsumerを使い、prime black frame中は
一覧body／guideを描かない。通常の画像ページ送り、wheel pan／zoom、中ドラッグzoom、
短右click／long-press画像menu、編集canvas／navigator／stripは一覧へ開放しない。
接続漏れの根因・限定レビュー・実renderer検証は
[一覧設計書§23](detached-book-page-overview-plan.md#23-一覧で省略された共通入力案内の接続tile112026-10-13)を参照する。

画像は既存 `ThumbnailState::Loaded` と在メモリの `thumb_adjust_tex` を投影する。
調整drag中は元のLoaded texture、それ以外は評価済みtextureを優先し、保存回転の確定値から
寸法とUVを解く。描画でDB読取、補正texture生成、GPU upload、元画像decodeを始めない。
回転は§1.6の既存workerを使い、未確定値を0°と推測しない。
一覧のactive需要dispatcherはconsumer適格性を確認した後、同contextの既存回転workerの
完了を`poll_still_seek_rotations`で回収してから要求・投影する。通常stripを描画しない一覧中も
完了consumerを省略しない。Loaded済み／回転未確定は新規decodeではなく完了採用で解消する。
通常ログ`[page-overview-demand]`はSurface経路、Admissionの型付き終了理由、
requested／keep／表示可否と局所Pruneを記録し、同じAdmission観測の反復を抑える。
passive投影は全実ページのimmutable metadataと可視近傍のtexture handleだけを保持し、
resize／DPI変更後の新セルもmetadataからtargetを解決する。full-image textureを入場条件にしない。

前面需要は同一viewerのstrip／hoverと一覧のいずれか一つをdispatcherで選び、
可視セルを先に、上下overscanを後に要求する。非表示stripが一覧のexact setを解除しない。
一覧新規要求は通常画像／ZIP画像／PDFページ（変換済みEPUBを含む）を共通dispatcherへ渡す。
PDFは§1.377の既存viewer ticket付き要求・owner-scoped昇格へ接続済み。
2026-10-27のmaster統合では構成採用後の可視実items順とhiddenを反映する。
白紙補助annotationは`None`のまま未接続で、fixtureの補助表示を製品接続済みとは扱わない。
2026-10-12〜13のfull補完・独立ACCEPTと§25.6の統合commit `1c134703b`までの指定自動検証は履歴。
独立レビューP2追補後の検証は§25.7で別に扱い、再レビュー・残工程はcoordinatorへ引き継ぐ。
[所有・入力・統合の正本§25](detached-book-page-overview-plan.md#25-2026-10-27-master統合記録)を参照する。

### 1.7 サムネイル画質ダイアログのプレビュー寸法

`src/ui_dialogs/thumb_quality.rs` は viewport と実際の利用可能幅から A/B プレビューを配置する。
詳細表示の `last_cell_size` / `last_cell_h` は行幅と行高であり、サムネイルの寸法として使わない。
詳細表示ではサムネイル比率を反映した基準寸法を用い、サムネイル表示でも巨大なセル寸法を
そのまま Window の要求寸法へ流さず、プレビューを利用可能幅と高さの上限へ収める。

狭幅では A/B を縦に配置し、内容を ScrollArea に入れる。下部の説明・操作はその外へ置く。
Window の初期サイズ・最小サイズ・最大サイズと constrain rect を viewport から定め、
詳細表示から開いた場合やウィンドウを縮めた場合も、実外枠と閉じる操作を viewport 内に保つ。
egui の content サイズには title / frame が後から加わるため、内側の余白と実外枠を同一視しない。
プレビューの拡大クリック、スライダー、A/B 適用、全画面プレビュー、Esc とタイトルの閉じる処理を維持する。

§1.225 (旧 §1.200) の回帰は同 module の headless egui tests 8 件。日本語フォントを用いた詳細/サムネイル表示、
fresh 480×420 と大画面からの縮小、実 ScrollArea 後の B 適用ボタン到達、プレビュークリック、
タイトル閉じるクリックと後始末を確認する。適用ボタンの到達性は実 Response で観測し、
ユーザーの設定保存は実行しない。2026-09-08 の焦点 gate は 8 件 PASS、独立 Astra レビューに blocking なし。
実 OS のリサイズドラッグ、IME、実アプリ操作の完了を示すものではない。

### 1.8 詳細表示と複数のサムネイル利用元の保持

詳細表示は通常グリッドのサムネイル読み込みを抑制するが、静止画シーク列・ホバー・しおりや
ページ移動先の要求まで消去しない。`thumbnail_keep_projection` が現在の利用元を合成し、
`install_thumbnail_keep_projection` が同じ viewer context の保持・退去・キューを調整する。

| 利用元 | 投影の正本 | worker 側の範囲 |
| --- | --- | --- |
| 通常グリッド | 既存の可視範囲・タッチ追加行・VRAM上限を反映した keep slice | bbox |
| ページ移動先 | items 世代が一致する navigation sequence の target pages | bbox・優先 |
| 詳細ホバー | 現在の hover idx | bbox・優先 |
| 表示中のしおりパネル | 現 container と loaded bookmark の typed page identity に一致する既存 keep 候補 | bbox・優先 |
| 静止画シーク | 既存の `still_seek_thumbnail_pages` | sparse exact set・優先 |

合成済みの keep / bbox からグリッドの元範囲を逆算しない。しおりの新しいページは既存 ensure が
keep へ入れてから要求し、次の投影は現在の membership と実パネルの表示条件で保持・失効を決める。
navigation / bookmark の ensure は既存 bbox と union してから worker へ公開する。
離れた still seek ページは bbox を広げず、従来の exact set で読み込みを許可する。

最後の利用元が離れたページだけを退去する。lock下で実行前queueから除いた一致するrequest IDは
局所取消で終了する。未採用source画像のkeep外破棄は画像側終端だけであり、保存終端まで予約を残す。
workerが既にpopした要求は相関済みの通知と画像採用/破棄で終える。VideoBatchの画像はkeep投影で
捨てず、既存Grid / Rendition消費policyに渡す。世代・source失効だけが旧request namespaceを退役させる。

しおり候補照合は loaded bookmark の typed HashSet と keep 候補を使い O(B+K) に留める。
フォルダ本の適格性に必要な全画像分類は既存 `ViewerNavigationCaches` の context / source 世代で
memo 化する。製本・synthetic 等の既存短絡条件を維持し、毎フレームの全件走査を増やさない。
独立レビューと回帰の結果は [v3.7.0 作業台帳](v3.7.0-priority-work.md) に記録する。

## 2. フルスクリーン表示パイプライン

### 2.0 RAW の表示段 (S3)

RAW は worker の info → 埋め込み preview → LibRaw フル現像の順で表示する。`RawPageStore` が context / items generation / 物理 source 指紋 / 要求 ID / 明るさを検証し、`RawPreview` を最終的に既存の Static へ置き換える。現像完了後は preview の追加保持をしない。preview より現像が先に完了した場合も、古い preview が表示段を戻さない。

fullscreen に catalog サムネイルや色忠実 rendition を出せるのは、埋め込み preview の実デコードと向き検査を通った後だけ (`raw_fullscreen_fallback_allowed`)。preview 無し・壊れた JPEG・向き不一致では、half 現像由来のサムネイルが存在していても読み込み表示のまま待つ。現像も不可なら Terminal としてナビゲーションを終える。

RAW の配置・fit・Original・Z・pan・回転・見開き・連結読み・座標変換の基準は全表示段で `developed_dims`。8192 clamp 後の Static や派生 final / AI texture の実寸を配置基準に戻さない。actual raster は canonical 枠へ contain し、ナビゲータ・ルーペ・capture の写像も共有 transform を使う。

preview は生デコード段の表示専用で、編集・同期補正・AI・final composite の入力にしない。カラー化 / LUT の既存 gate は維持し、必要なページは検証済み preview の後だけ色忠実 rendition を表示する。ただし現像が Blocked (非対応・失敗) の RAW は処理入力が得られないため gate の対象外とし、rendition の有無にかかわらず埋め込み preview をそのまま表示して通知する。判定は owner の現像軸から導く `raw_development_blocked` に集約する。設定変更で Failed が Idle に戻れば通常の gate が再評価される (計画 7.10 K)。PreviewShown はその提示でページ送り・フォルダ lock を終えられる。PreviewAbsent は developed / complete final の提示か Terminal まで待つ。両ナビゲーション経路は RAW の分類と同じ readiness 述語を使う。編集の全入口・見開き左右・ツール内ページ切替・ボタン状態は、実際の編集対象に対する `raw_edit_target_gate` を mode / page mutation の前に確認する。

明るさ変更と worker の Stale はページ単位の source transaction。既存の `capture_final_effect_source_reload_holdover` だけで提示中のユニット / 連結読み transition を退避し、入力と派生 cache を失効させる。明るさ変更では preview / 寸法の要求を残す。retained AI の source 指紋 + 明るさ key を更新し、他ページの retained epoch は変更しない。

埋め込み preview texture 自体は現像軸の状態にかかわらず画面フィルターへ渡さない。paint resource の source texture と RawPreview の texture identity を照合するため、同じページの表示専用 rendition や前の通常画像の holdover は既存のフィルター処理を維持する。Full 完了時の同期補正は通常画像と同じく現在ページに限り、先読みや画面外の完了は既存の bounded final pipeline に処理を委ねる。

### 2.1 エントリポイント

```
ユーザーが Enter / ダブルクリック
    │
    ▼
App::fullscreen_idx = Some(idx)
    │
    ▼  (次フレーム)
ui_fullscreen.rs::render_fullscreen_viewport
    │
    ├─ fs_cache[idx] がなければ → start_fs_load(idx) を発火
    ├─ テクスチャ選択 (後述の優先順位)
    ├─ spread_mode に応じて 1 枚 or 2 枚並べる
    ├─ rotation + zoom + pan + free_rotation を合成して描画
    └─ update_prefetch_window(idx)     # フィルタ後の前後数枚を先読み / 範囲外を解放
```

フルスクリーン余白色の正本は global な `Settings::fullscreen_image_margin_color` である。
静止画キャンバスでは画像外の余白と seek strip のセル外空欄へ使い、通常画像、ZIP / PDF、
単ページ、見開き、連結読み、比較、360 度パノラマ、画像分析で共有する。native 動画では DComp root
最下層の canvas と、拡大・パン / display-resolved scaling の source 外画素へ同じ値を投影する。
native 動画の seek strip は場面サムネイル / 波形の両表示とも全域で従来の暗色下地と配色を保ち、
`NativeEguiOverlay` は canvas RGB の snapshot を持たない。動画画素、素材に含まれる黒帯、字幕、
音声 / 動画の音声モード、HUD / 左右パネル、ナビゲータやルーペなど部品内部の黒も余白設定へ接続しない。

透明下地は `fs_transparent_bg_mode` から不透明な黒 / 白 / 市松へ解決し、余白色には fallback
しない。下地は `DisplayedImageTransform::paint_quad` が返す画像 texture と同じ 4 頂点へ描く。
表示トリム後の `paint_rect` と任意角度回転中心を共有するため、回転 quad の AABB へ余分な色を
塗らない。市松 texture の UV は page-local の幅 / 高さで反復し、quad と一緒に回転する。
可視領域だけを再サンプルした texture でも、同じ clip 内で先に画像全体の paint quad 下地を描き、
その上へ source-region texture を置く。この paint layer は画面表示専用で、画像画素、AI、copy、
capture、export の合成には入らない。

passive frozen single の direct texture も source-region mesh を使う。保存済み回転と表示 trim が
同時に snapshot に残る場合、元の `image_rect` を geometry の基準にして trim bbox を位置と UV の
両方へ渡すため、画像と透明下地は同じ trim 後 quad / clip になる。detached keepalive backstop は、
両側を capture できた単ページ / 見開きを typed display unit で描き、片側だけ準備できた見開きで
navigation holdover が無ければ canonical spread painter を呼んで両 slot を保つ。連結読みも canonical
visible-pages painter を使う。general keepalive / embedded deferred の canvas は typed static unit が
ある場合だけ現在の余白色にし、unit の無い media transition は egui 側の黒を保つ。native 動画の
letterbox canvas は別の DComp owner が現在の余白色を描き、音声系だけ黒を保つ。

#### 2.1.1 crop 済み代役の全体寸法と収録範囲（§1.165 B1、実装・自動検証済み・独立レビュー受入可・ClaudeCode の隔離環境で実アプリ確認済み、利用者の実機確認は未）

一覧のアスペクト比は従来どおり crop 後の `source_dims` を使う。部分編集 preview のページ構成
（見開き・横長分割）と注釈座標には crop 前の正準座標寸法を供給する。
100%表示の基準は独立した `DisplayOriginalDimensions` で表す。通常完成素材は選択中の実
texture 寸法（GPU clamp 後／AI 適用後）、部分代役は生成時の crop 前 `raster_dims` を使う。
正準20000×15000／GPU raster8192×6144の場合も、代役と本画像の100%は8192×6144で一致する。
`SourceCoverage::is_partial()` は実画素境界から派生させ、収録範囲が
全体の素材（新生成 no-crop Known を含む）の単ページの従来 Original／thumbnail sizing は維持する。
`poll_thumbnails` の寸法記録と Loaded を直接参照する fallback の双方でこの区別を保ち、
texture 退去後も `PageDimsCache` に crop 後寸法を再登録しない。PDF page-box、RAW の許可 gate、
processed の縦横比変更、指定方式の既存 A/O 基準は、それぞれの既存の意味を維持する。

`SourceCoverage` は `ThumbnailState::Loaded`／CPU 下地から通常の
`FullscreenPaintResource`、prepare 後の `FullscreenPresentationResource`、paint payload へ
素材と一緒に継承する。coverage を idx 別の追加 map や `content_bbox` へ置かない。
`content_bbox`／左右分割は表示要求であり、coverage は自由回転時も失われない素材の由来である。
単ページ・見開き左右・連結読みは全画像 transform を先に解き、その全体枠へ部分素材だけを配置する。
見開き gap や連結読みの extent を、crop 後 quad の外接矩形で縮めない。
見開き全体の左右寸法と100%基準は `fs_spread_page_layout_size`／`fs_spread_fit_input` の
供給境界で選ぶ。通常完成素材は7474e5cf1の実texture基準、部分代役は切り出し前raster、
通常thumbnailはページ全体を代表するGPU raster寸法で構成する。後者の8192 clamp計算は
実producerの `gpu_raster_dimensions` と共有し、UIで元画像を読み直さない。
これはcropなしの読込中代役にも適用する、設計担当が受け入れた仕様差である。
7474e5cf1ではOriginal見開きの代役をthumbnail実寸（例:150×200）で表示し、本画像
（例:600×800）の到着時に拡大していた。現在は代役もページ全体のraster寸法の枠へ置き、
到着時に大きさが跳ばないようにする。7474e5cf1と同じ倍率契約へ戻した範囲は完成素材であり、
読込中の代役まで完全に同じ表示へ戻したものではない。
指定方式は通常rasterのAI前A/Oを単ページ・見開き・連結読みで共通に解決する。
RAWのdeveloped寸法、PDF page-box、他4方式の連結読みのsource基準は維持する。
通過renditionも部分素材なら同じ切り出し前rasterを使い、全面renditionの従来source基準は維持する。
各ページ内の通常素材の contain／物理 pixel snapping は既存の規則を維持する。
正準寸法とGPU clamp後寸法が異なる場合も、注釈・source座標を100%表示寸法で上書きしない。

`MaterialDisplayBasis` が素材の不変な寸法契約を所有する。thumbnailはworker受理時のsource寸法と
生成coverage、sourceは自身のmetadata、派生素材は実際に選択した入力ownerから値を受け取る。
Single／Spread／Continuousの旧4方式と指定A/Oは、この一つのdescriptorからの名前付き射影とする。
producer selectorは `FullscreenPaintResource` を返し、handle getterはその射影に限定する。
frame stateの通常素材・thumbnail／通過代役、Native不採用時の描画fallbackもResourceとして受け渡す。
描画入口でbare handleをLoadedと照合し直す処理は置かない。
保持presentationの選択は別に `FullscreenPaintPayload` を返し、NormalResourceもPresentationMaterialも
そのまま運ぶ。fit・見開き構成・指定倍率・保持・退去後の再描画でidxやtexture IDから100%を再探索しない。
現在のfit設定・倍率制限は描画時に適用し、frozen captureだけが設定を含むfit descriptorも捕捉する。
通常thumbnailの旧catalog行でsource寸法が無い場合は、指定倍率を含め従来の選択済みsampler契約を
受理時に明示して保持する。描画側でsamplerや正準座標から100%を補う処理は置かない。
不正寸法や、従来から表示に必要なPDF／RAW metadataの未確定は、互換行の寸法省略と区別する。
RAW／PDF／連結のlegacy source-layoutは `LegacySourceLayoutBasis` にlayout単位と
受理rasterの参照寸法を別々に持つ。単ページのaspect正規化もこの参照寸法を使い、
cache退去後の座標寸法やsamplerへ戻らない。PDFの有効なKnown素材は全域収録でも、従来のPDF契約に従って
coverageの全体座標寸法をlegacy参照寸法として受理し、page-box layout単位とは分離する。
FullSource thumbnailの参照寸法は従来どおりown samplerとする。PDF thumbnailのmetadata再受理では
page-boxと指定A/Oを更新し、同じ素材のlegacy参照寸法は保持する。

受理条件の棚卸し（`76eeab77b`との比較）:

| 素材 | legacy参照寸法・受理条件 |
| --- | --- |
| PDF Known部分収録 | coverageの全体coordinate寸法。page-box layout単位とは別に保持 |
| PDF Known全域収録（cropなしの注釈・局所編集preview） | 部分判定をせず、同じ全体coordinate寸法を保持。Original／Page／指定倍率で本画像採用前後を比較 |
| PDF FullSource（座標なし） | 従来のown sampler参照を維持 |
| RAW Known | 全域／部分ともcoordinate寸法を受理。worker寸法・RawInfoも既存の受理境界で扱う |
| 通常画像／ZIP | 全域／部分とも座標・crop前rasterを受理。残る部分判定はsampler表示／部分素材aspect等の既存の方式別契約に限る |

RAWの旧catalog行に寸法が無い場合は、受理済みRawInfoのdeveloped寸法をthumbnail受理時に採用する。
thumbnailが先に届いた場合も、RawInfo／previewの実受理境界でlive descriptorを再受理する。
PDF／RAWでdescriptorが変わったページの依存通過素材だけを退役し、保持済みcaptureと兄弟ページは維持する。
見開き・連結の既存layout単位はそのまま使い、物理100%の型へ統一しない。
素材のない連結ページのlegacy placeholderは、退去後も正準layoutを保持する。これは未描画ページの
空間だけを確保する値であり、指定倍率の物理100%基準には使わない。ページ送り受付も共通の
fit resolverから型付きの基準欠落を受け取り、描画不能な素材をNormal入力としてpinしない。

部分素材の `full_source_size_vec2` はsampling canvasを表す。fitが読む型付き100%基準は
素材descriptorの値であり、`coordinate_dims` と混ぜない。
`FullscreenFitInput::Raster` がOriginal／Page／Width／Heightと倍率制限の100%基準を
capture／replay／Zへ保持する。AI完成後は従来どおり実出力textureの100%へ再配置する。
丸め後の texture 寸法を coverage の幅・高さで個別に割って全体寸法を逆算しない。
native resampler は別に保持する実 texel 寸法と coverage から軸別の sampling 密度を求め、
共有 geometry が返す交差範囲と物理出力寸法を使う。細い crop の縮小丸めも全画像の aspect や
fit／contain へ戻さず、native ROI の素材内座標への変換は準備境界だけで行う。

純粋 helper `source_region_texture_quad` が描画頂点と素材 UV を一元計算する。
coverage を C、表示要求を V とすると描画範囲は C∩V、素材 UV は `(source-C.min)/C.size`。
native ROI がある場合は素材内 ROI を C の空間へ合成し、さらに交差させてその ROI 内 UV へ写す。
画面位置は全画像の transform、保存回転、自由回転から求める。通常 mesh、保持素材、canonical
animation はこの helper の結果を共有し、native の不完全 ROI を許すための条件緩和はしない。

透明下地とページ枠は全画像側へ残す。ページ送りは `PageMaterial::PartialTexture` により
紙面全体と部分素材の domain を分けて保持・投影する。coverage と表示範囲が交差しない場合も
背景だけのページ occurrence として提示／receipt を進め、crop を理由に animation を無効にしない。
passive detached の snapshot capture／保持／再描画、navigator、ルーペ、パノラマ navigator、
保持素材復元も同じ resource の coverage を使い、capture 後の thumbnail 差し替えで再推定しない。
通過 rendition は注釈を含まない CPU 下地と coverage を継承し、注釈を省略する既存契約を保つ。

設計簡素化として既存 resource／presentation の所有へ由来を載せ、現在 crop と preview の
非同期失効を同期させる追加状態・全 context reset・待機は導入しない。保存や DB のまれな失敗は
既定の割り切り（ログ＋既存通知・単純な作り直し）とし、多段の復旧処理を足さない。
SNS 分割パネルのプレビューも、各分割枠を全画像座標へ戻して同じ resource の mapping を使う。
シーク用の一覧素材プレビューは従来どおり crop 後の素材として扱う。
この節は実装契約の記録であり、完了した自動検証／実 GPU 提示／実機の成功証拠ではない。

検証と未決（2026-10-22、master 取り込み `124dc4828`。経緯の全文は取り込み前の backlog §1.165 = コミット `e5c9fc951`）:
- 独立レビュー（Codex xhigh）最終判定は受入可（`629a7d85b..a43750fb8`）。`test-lib-full.ps1` 14,070 件成功、`ui_snapshot` 195 件成功。
- ClaudeCode の隔離環境での操作（画面操作の許可なし。PostMessage のキー + 全画面の連続切り出し、8000×11000 の格子 PNG と
  合成 8 ページ PDF、crop は `export_crop.db` へ直接記入）: 単ページ（Page / Original / 指定倍率 + 縮小しない）・見開き・縦連結・
  crop 済み PDF の Original で、代役は全体枠の中の切り取り位置に出て、本画像到着の前後で切り取り範囲の画素位置が変わらず
  外側が補われた。crop なしの巨大画像の代役も本画像と同じ位置・大きさ（ぼけのみ差）。
- 未確認: AI アップスケール併用、回転・表示トリム・分割との組み合わせ、crop なしの編集済み PDF（自動テストのみ）。利用者の実機確認は未。
- **利用者決定（2026-10-22）**: 旧行（Unknown）＋現在 crop ありは全画面の代役に使わない現行の扱いを維持する。
  そのページを新版で編集し直せば coverage 付きの行に置き換わって解消するので、「現在の crop から推測して使う」案は採らない。
  判断は `App::resolve_preview_coverage_for_display` の 1 か所。
- crop ドラッグ中の Escape の別件は、2026-10-22 の追補で開始前の `Option<CropSettings>` を復元する。
  修正前の handler-level テストで、DB は元の値のまま、メモリだけ新しい crop が残る失敗を確認した。
  ドラッグだけ取り消してモードを維持し、次の Escape で終了する。取り消しは保存や preview 失効を発行せず、
  close / ページ移動 / ツール切替の preview snapshot より前に完了する。詳細は [crop の所有と終了](preset-and-adjustment.md#crop-ドラッグの所有と終了2026-10-22)。

frame 冒頭では、先読み窓ぶんの `fs_cache` を 1 回だけ走査し、`Static` / `Animated` / `Video` が
現在の縦横判定に使う寸法を `PageDimsCache` へ回収する。見開きの判定順は `fs_cache` →
ロード済みサムネイル → `PageDimsCache` → 未知なら縦長扱いであり、一度判明した寸法は live cache
退去後も同じ viewer context / items 世代に残る。全 thumbnails の毎 frame 走査は行わない。
各 cache が保持する寸法は回転前なので、見開きの横長判定は nav 分の保存済み回転を
`rotation_db.get_many` で一括取得し、共通の `landscape_after_rotation` へ寸法とともに渡す。
90° / 270° は幅と高さを入れ替え、0° / 180° は元の寸法比を使う。`rotation_cache` は item
差し替え時に失効し、個別回転時は更新値を直接 memoize する。`SpreadDisplayUnitsCache` は
items 世代・読書順・見開き mode・ずらし位置・横長判定 epoch に対応する unit 列を保持する。
回転変更による横長判定の更新を次の解決へ反映し、単独境界と後続ペアを再計算する。
Remote の通常コンテナとコレクションも catalog の回転前寸法と同じ回転キーを一括で読み、同じ純関数を
通す。寸法未確定を縦長扱いにする契約は維持する。

末尾に表紙を添える場合も `SpreadDisplayUnit` の navigation 列は変えない。
`SpreadDisplayComposition` の slot は source idx・Navigation / FrontCoverSupplement role・読書 anchor
を持ち、phase 所有の `FsNavigationDisplayDemand` が navigation と全 presentation の投影を一体で扱う。
読書位置・シーク・履歴・ページ番号は navigation、読み込み・全 slot の ready/failure・描画・保持は
presentation を参照する。末尾だけを先に確定して後から表紙を足す表示にはしない。
連結読みは既存 still-image unit owner を使い、同じ source が先頭と末尾に現れる occurrence を
unit anchor と role で区別する。保持対象は残存 unit の source の和集合から求める。
全画像からなる単本の完全読書順の検証は items / nav identity ごとに保持し、回転だけで再走査しない。
検索・stack・合成一覧などの一時条件は別に評価する。
Remote は navigation の `PageGroup.anchor/pages/slice` を維持し、補助のある group だけに
role 付き `presentation` を付加する。仕様・検証状況は[末尾表紙の設計書](final-cover-spread-plan.md)を参照。

端の単ページを本来の側へ置く設定では、canonical `SpreadDisplayUnit` がpage列と
`Paired / Singleton(UnpairedSlot | LandscapeBoundary | NonPairableBoundary)`の形成理由を一体で持ち、
同じ `SpreadDisplayComposition` が `Center / Left / Right` の layout projection を所有する。
末尾表紙補助を解決した後も実 1 page で、全ページを含む本であることを証明でき、かつ本当に相方slotが
無い `UnpairedSlot` の先頭・末尾 unit だけが対象になる。現在または相方候補が横長で単独になったunitは、
回転後判定を含め端でも `Center`。先頭側は cover 位相を含む見開き mode、末尾側は綴じ方向から決まり、
1 page のportrait本は先頭規則を優先する。空き側は同じ page 寸法を
使う仮想 slot であり、`SpreadPair`、navigation、occurrence、page count、source request は増やさない。
paged / Z / 連結読み / Remote は composition の placement と物理 pixel 量子化済み gap を共用する。
`FullscreenPageLayout` は実際に描いた側を transform に残し、navigator・範囲選択・edit hit はその値を読む。
holdover は capture 時の transform から placement を取り込み、後から設定や canonical unit を再解決しない。
詳細は[端の単ページ配置の設計・検証記録](section218-singleton-spread-placement.md)を参照。

paged 表示でキーリピート由来の未消費ページ送り edge が同じ input frame に残る場合は、現在の
表示 unit をカタログサムネイルで 1 frame 描き、processed texture と完成済み worker result の
GPU upload を次の frame へ保留する。単ページは現在ページ、見開きは通常描画と同じ
共通 composition が返す補助を含む全 presentation ページについて `ThumbnailState::Loaded` を要求し、1 ページでも
欠ける場合や unit を解決できない場合は通常の実体化へ fail-closed する。ただし、その display unit の
全ページについて `current_final_composite_texture` が既に返せる場合は、未消費 edge があっても
`Materialize` を維持し、完成表示からサムネイルへ降格しない。この在住確認は cache lookup だけで、
`resolve_fs_processed_texture` や新しい worker / GPU upload を起動しない。時間閾値や前 frame の
pending state は使わず、連結読みは対象外とする。

**`keep_fullscreen_viewport_alive`** はフルスクリーン非アクティブ時 (`fullscreen_idx == None`)
に呼ばれ、`fs_viewport_shown == true` の 1 フレームだけ `Visible(false)` cmd を送って hidden 化
する責務を持つ。それ以外のアイドルでは何もしない (2026-05-10、hidden viewport 維持コスト削減)。
再入場時は `render_fullscreen_viewport` が新規 viewport を hidden で作成し、DWM transition
抑止属性を適用してから `Visible(true)` にする。アイドル時の hidden viewport 維持コストを
戻さずに、初期 white client / サイズ遷移フラッシュを抑える。
詳細は [docs/ui-responsiveness.md §9](ui-responsiveness.md) を参照。

Ctrl+↑↓ のフォルダ横断と、同じ表示ユニットの final-effect source reload は、単一 field
`fs_holdover_tex: Option<FsHoldover>` を共有する。値は typed state で相互排他になっている:

- `FolderNavigation(previous)`: `previous.pages` に画面順の 1 ページ、または `[left, right]` の
  見開き 2 ページをまとめて保持する。ページごとの fallback ではなく、直前に見ていた
  **表示ユニット単位**の hold であり、カラー化の有無とは無関係に
  `capture_fs_nav_holdover` が作る。各 page は texture に加えて capture 時点の rotation、
  canonical coordinate source size、PDF page box（通常画像は source size）由来の独立した
  canonical layout size、実表示に使った単ページ / 見開き側別 content bbox、paint-time
  post-filter と診断 identity を所有する。片側配置した endpoint singleton では、実paint transformが
  確定した `Left / Right` も unit に保持し、待機中に中央へ戻さない。
- `FinalEffectSourceReload { target_idx, previous, started_at }`: PDF の Z ズーム再レンダなど、同じ
  表示ユニットの source が差し替わる直前に、`previous.pages` へ単ページまたは見開き全体を保持する。
  `capture_final_effect_source_reload_holdover` だけが作り、左右別 slot は持たない。`started_at` は
  作業中表示の 400ms UX 閾値を variant 自身に持たせる。通常のページ移動はこの state を作らず、
  移動先の色忠実な低解像度 rendition を final composite 到着まで表示する。

`fs_nav_holdover_for_draw` が有効な間、`fullscreen_idx == None` の PDF/ZIP enumerate
gap と、`fullscreen_idx == Some` の nav ロック継続中のどちらも、カラー化側と共通の
`draw_fs_display_unit_holdover` で旧単ページまたは旧見開き全体を `image_rect` に重ねる。
画面順を payload に保存するため、LTR は `[小 idx, 大 idx]`、RTL は `[大 idx, 小 idx]` の
物理的な左右順を items 差し替え後も維持する。texture handle と geometry snapshot を unit
自身が所有するため、旧 `fs_cache` が drop されても描画寿命は変わらない。holdover の単ページ・
見開き描画は capture 済み canonical layout size を明示的に渡し、同じ数値 idx が新フォルダの
別 item を指し始めても `items` / `thumbnails` / page-layout metadata から rotation / layout size /
source size / trim / post-filter を再解決しない。idx は `fullscreen_page_layout` の identity と
診断イベントの数値欄にだけ使い、診断 key / load sequence も capture 済みの値を使う。

フォルダ横断後、新しい `items_generation` のページで full / final / edit / thumbnail の
いずれかを一度でも表示候補に選べたら、`fs_nav_holdover_for_draw` は
`FolderNavigation` の unit と全 handle を破棄して一方向にラッチする。表示確定待ち世代を持つ
`fs_nav_locked_gen` は `poll_fs_nav_lock` が別途解除するため、両者の寿命は同一でなくてよい。
AI final の invalidation / install の過渡フレームで表示解決が再び `None` になっても、
旧フォルダの unit は既に存在せず、nav holdover が復活することはない。

`fs_nav_locked_gen` は入力を拒否する lock ではなく、どの `items_generation` の表示確定まで
旧ビューの holdover を維持するかを表す presentation generation である。Ctrl+↑↓ と
Ctrl+PageUp/PageDown は lock 中も、snapshot / Ctrl+G / ZIP tree など副作用を持つ context
routing へ再入せず、読み取り専用 resolver が DFS 系 request だけを `start_folder_nav` へ渡す。
表示待ち中に次の DFS を新規開始した場合は handle を再キャプチャせず、locked generation だけを
現在世代へ進める。これにより中間ページの表示完了が先に届いても、次の folder load 前に
holdover を解放しない。スライドショー停止、detached session routing、検索分岐、ZIP 内移動は
最初の入力でだけ実行し、lock 中のリピートでは二重実行しない。

DFS 実行中の同種入力は `start_folder_nav` の単一受付口で
`pending_folder_nav_steps` へ符号付きで累積し、前後入力は相殺する。追加ステップの上限は
`MAX_PENDING_NAV = 5`。worker 完了時に `poll_folder_nav` が残数を `FolderNavResult` へ移し、
その request が起こした folder load の適用成功時だけ次の DFS へ引き渡す。
`start_loading_items` 自体は従来どおり App 上の accumulator を clear するため、アドレスバー、
お気に入り、ツリークリックなど無関係なフォルダ切替をまたいで burst が残ることはない。

フルスクリーン画像領域の合成順は、ページ画像 → ページ単位の編集オーバーレイ
(消しゴム / クロップ / キャプチャ領域 / ルーペ) → ビュー単位の holdover →
インジケータ / HUD / パネルとする。holdover は退避時に見えていたビュー全体なので、
移動先ページの編集矩形を旧ビューの上に重ねないよう、編集オーバーレイより後に描く。

`--perf-log` 診断では `fs.paint` に `source` / `idx` / `items_generation` /
`texture_id` に加え、実描画 transform の `x` / `y` / `w` / `h`、texture 寸法、
`scale_x` / `scale_y` を記録する。重複抑止署名は texture identity だけでなくページごとの
描画矩形と軸別 scale も含み、同じ texture のまま幾何だけが変わる frame も記録する。
浮動小数点の揺れでログを増やさないため、矩形は 0.25 point、scale は 0.001 を超える差だけを
変化とする。表示済みの同じ idx が解決不能へ落ちた遷移は
`fs.texture_choice` (`source=none_after_paint`) として 1 回だけ記録し、nav holdover /
continuous transition が実際に選ばれた場合も同イベントへ `branch` と source を残す。

フルスクリーンの先読み対象は、`items` 全体ではなく `visible_indices` 由来の display list から
作る。★フィルタや Ctrl+F で一覧が疎になっているときも、スライドショー / 前後移動と同じ
次候補を先読みし、フィルタで隠れた画像は対象にしない。

評価変更による所属更新 (§1.371) は、各 viewer context の表示集合を採用する境界で読書位置まで
調停する。現在ページが残れば同じ identity を保ち、外れれば旧読書順の次の残存ページ、なければ
前へ一度だけ着地する。RTL でも読書順の ordinal が増える方向を「次」とし、raw index や左右キーで
反転しない。読書対象がゼロなら既存 teardown で現在の絞り込み済みページ一覧へ戻り、親退出や
filter 解除を行わない。strip / seek / ページ番号、見開き相方、連結 unit、先読みは同じ採用済み集合を使う。
評価による landing / close は評価通知と評価 Undo を保持し、有効な Undo origin の復帰を通常近傍より
優先して一度適用する。独立した `DetachedPhysical` は main の global filter に従わず、main の所属更新で
移動・閉鎖しない。ContextTable の plan / execute / finish 中に publication を再入させず、設定投影済みの
安定境界でその owner だけに公開する。

リングの★は保存済み live preview のまま、その preview だけで離脱する既存行を picker 終端まで
context の共有表示集合へ残す。全画面専用の例外ページ列は作らない。新しい検索 hit・件数・明示 sort は
即時採用し、保留行を含む集合から可視件数と読書ページ数を毎回計算する。source 削除・非評価条件の離脱を
復活させず、通常終端で保留だけを外して最終成功値から一度調停する。集合決定・非同期結果の採用前検証・
Undo 復帰の取消は [評価変更と絞り込みの計画](rating-filter-fullscreen-plan.md) を参照。
grid cursor の移動・解除は §1.370 の `App::move_grid_cursor` に適用し、scroll intent は集合採用callerが保持する。
Grid起点の評価Undo/Redoは、元項目が現在集合へ戻れば同じownerからcursorとShift起点を戻す。
明示選択は履歴内のGrid originをValueOnlyへ失効させ、live requestの復帰も取消す。Viewer originからの復帰と分け、Grid復帰ではreaderを開かない。
同identityの再配置は移動とせず、selectedとShift+クリック起点をそれぞれpaired remapする。
Searchの同一覧items交換では、readerのexact identity transferを完了してからvisible集合を再計算し、
集合ownerが行先を一度適用する。交換前のreader idxを交換後のslotへ解釈してpickerのsource離脱を判定しない。
ページidentity/sidecar準備でopenを待つ場合は、既存の遅延open要求にrating/Manualのmetadata境界を保持し、
IndexOnlyと通常hydrationの両再開経路へ渡す。遅延を挟んだrating Undo復帰でもrating Redoを消さない。
所有中のrating遅延targetは集合ownerの論理的な着地点とし、集合更新時のKeep/Move/Emptyを一度適用する。
Keepは要求を継続し、Moveは置換、Emptyや手動操作はその要求だけを取消す。再開時に移動先を再決定しない。

2026-10-11の利用者決定により、Rating/Searchのitems再採用に伴う編集の扱いは出荷済みboundaryの
再利用へ限定する。Rating再採用は既存境界で編集を終了し、非rating Undoを落としてrating Undoだけを
保持する。Searchは既存のclear/cancel方針を維持する。reader同identityとselected/Shift起点のpaired remapを
保つ一方、loaded/global editorやidx-only Undoの新しい保持/remap、一般editor ownerの拡張は行わない。
v4.5.0で既存のSearch streaming idx-only誤配送は別件・本修正の対象外であり、利用者がバックログへ
記載する。根拠と受入範囲は [編集範囲の利用者決定](rating-filter-fullscreen-plan.md#stage-2-の編集範囲限定--利用者決定-2026-10-11) を参照。

### 2.1.2 通常フォルダの初期ページ構成（§1.425）

初期の物理一覧採用は `SpreadRestoreDefaults::for_physical_folder` に本扱い・実ディレクトリの
採用済み事実・実効設定の値を渡す。本扱いは従来の `for_book` と同じ mode/flow/direction、
それ以外は原則 `NON_BOOK`（Single/Paged/Ltr）。`apply_default_spread_to_plain_folders`（既定 OFF）が
ON の実ディレクトリだけ、mode を実効 `default_spread_mode` へ置き換える。
この分岐だけ旧 Vertical を Single に正規化し、flow は Paged、方向の fallback は Ltr を保つ。
`StoredSpreadState::resolve_reading` が保存値を優先し、未保存では mode の埋め込み方向を使う。
保存済み flow と旧 Vertical の復元は従来どおり。既定の適用から `spread.db` へは書かない。

実ディレクトリの証拠は、directory scan が生成して既存 loader/Stack が運ぶ
`FolderScanSignature`。Physical 分類や拡張子、UI での stat から推定しない。
メタ情報取り込み後の既存復元で新 resolver を使うのは、設定 ON・非本の実フォルダだけとする。
同 context の source/世代が一致する `FolderObservation` を使って対象を確認し、既定を再解決する。
OFF・本扱い・その他の対象は §1.425 導入前の計算を維持する（通常 open との既存不整合も今回の修正対象外）。
main/detached は共通 loader を使い、兄弟の読書状態を変更しない。
抽出結果の early-return と保存 flow の優先順、書庫・文書の専用 `for_book` 経路は保つ。

簡素化として、新しい pending/worker/live rebuild は設けず、既存の一覧採用と取り込み後復元へ
純粋 resolver を接続する。設定変更を理由に表示中の全窓を組み直す経路は追加しない。

### 2.2 ロードスレッド

`App::start_fs_load` (app.rs) が 1 枚ごとに worker を spawn し、worker は
App-global な `FsPageLoadScheduler` の permit を取得してから以下へ進む。総枠は 6、
先読み (Normal) は最大 4、現在ページと表示中の見開き相方は High で残り 2 枠も使える。
要求と cancel / result の所有は viewer context ごとの `fs_pending` に残る。

```
             FsPageLoadScheduler (Mutex + Condvar, RAII permit)
                                  │
          ┌─ GridItem::Image      ┐
          ├─ GridItem::ZipImage   ┴→ canonical_image_loader::decode_canonical_image
          │                          → image crate → WIC → Susie の順で static fallback
          │                          → EXIF Orientation 適用済み native image
          └─ GridItem::PdfPage    → pdf_loader::render_page_for_display
                                     (実 viewport×ppp、PDF ワーカープロセス)
                                     ※zoom 時は倍率に応じて再レンダリング

アニメーション:
  ├─ .gif      → fs_animation::decode_gif_frames (通常画像のみ)
  ├─ .png/APNG → fs_animation::decode_apng_frames (通常画像のみ)
  └─ .webp    → fs_animation::decode_webp_frames / decode_webp_frames_from_bytes
                (通常画像 / ZIP 内画像)
```

待機中の要求は cancel されると source を読まずに終了する。実行中の cancel は permit を
即時返却せず、worker が実際に終了するまで `Cancelling` として数える。ZIP は位置指定 reader の
`read` / `seek` 境界、書庫読み出し直後・decode 前、animation frame 境界で cancel を確認する。
PDFium / Susie IPC は開始後の応答まで permit 内で待つ。先読みから表示対象への遷移は同じ
ticket の High 昇格であり、cancel + 再投入は行わない。

Animated (`FsCacheEntry::Animated`) は playback-only として扱う。表示時は常に
`current_frame` の raw テクスチャを直接選び、`edit_result_cache` /
`final_composite_cache` / comic composite などフレーム非対応の派生キャッシュには
乗せない。編集モード・AI アップスケール・ポストフィルタは Animated では無効。
これは edit/final cache key が idx + generation ベースで、フレーム番号を含まないため、
1 フレームを派生キャッシュに入れると以後の `current_frame` 更新が画面へ出ず
アニメーションが停止して見えるため。

`decode_canonical_image` は fullscreen の現行分類も共有する。通常ファイルの GIF/APNG と、
通常ファイル・ZIP 内の WebP は Animated を返す。ZIP 内 GIF/APNG は animated 拒否ではなく、
従来どおり static fallback で先頭フレームを読む。Static は EXIF 適用後・8192 clamp 前の
native `DynamicImage` と `source_dims` を返す。panorama tee はこの native image を受け取り、
非 panorama の GPU raster 化だけが `CanonicalStaticImage::into_gpu_raster` で従来と同じ
Bilinear 8192 clamp を行う。

PDF 初回レンダは、描画先の `fullscreen_media_rect` の論理 point 寸法へ、その
viewport context の effective `pixels_per_point` (OS DPI × UI scale) を掛けて物理 viewport
`(Vw, Vh)` を作る。ページ寸法 `(Pw, Ph)` は PDF worker が content type と同じ PDFium
open 内で取得し、ページ全体表示なら
`S = min(Vw / Pw, Vh / Ph)`、`target_long = ceil(max(Pw, Ph) × S × 1.10)` とする。
横幅 / 縦幅 fit はそれぞれ `Vw / Pw` / `Vh / Ph`、90°/270° 回転時はページ軸を交換する。
1.10 は丸め・filter・1-frame の layout 差で等倍表示を眠くしないための品質余裕である。

- raster-only PDF は同じ worker 内で判明した埋め込み画像の native 長辺を絶対上限にする。
- 全経路の上限は 8192。100% 原寸 / no-downscale は従来の表示密度を下げないよう
  vector の最低長辺 4096 を維持し、raster native 上限は常に優先する。
- `start_fs_load` は実 viewport が確定する前の PDF を enqueue しない。fullscreen、detached、
  in-window の各 context は同じ typed display target を所有し、先読みもそれを再利用する。
- 自動 / 手動の表示 trim bbox は初回 raster 到着後に確定する。bbox 部分が viewport へ fit した
  ときの必要 texel 密度を同じ式で再計算し、不足する場合だけ priority 再レンダする。
- zoom 再レンダは display-fit 長辺を基準に倍率を掛け、8192 と raster native で clamp する。
  AI が必要な raster は display-fit が native より小さい場合も大きい場合も native へ収束させ、
  その間の final AI / AI 先読みを保留する。

表示解像度合わせ・zoom/AI native・指定倍率の再レンダはすべて
`request_pdf_rerender_at_target` で進行中要求に合流する。`FsPendingValue` 自身が
source (`metadata_cache_key`)・要求長辺・PDF pool の priority lane を所有し、同じ source/長辺の
要求を毎フレーム取り消さない。Critical は Normal の需要も満たす。Normal → Critical だけは
一度取り消して置き換え、異なる source/長辺も置き換える。取消済み ticket には合流しない。
完了・取消・close・世代破棄で pending が消えれば identity も消える。別の要求表は持たない。
`fs_pdf_rerender_demand` は完了後も zoom/指定倍率の需要を記憶するため維持する。

2026-10-22 追補: 再レンダの失敗は、pending を回収する境界で、そのページの既存
`FsCacheEntry::Static.pdf_rerender_failures` へ要求 identity（source / target / lane）を移す。
画素・texture・派生 cache・input 世代を維持し、失敗結果を upload backlog の `Failed` へ流さない。
全 producer が同じ入口で source / target の失敗事実を照合するため、fit と AI 等で失敗 target が
交互に出ても同じ組を再要求しない。priority の昇格も失敗した組の再要求理由にはしない。
異なる source / target は要求でき、新しい raster の採用・cache 退去でその画に属する失敗記録も終わる。
Static が無い初回の失敗は従来どおり `Failed`。未取消の再レンダ worker の切断も既存画を保持する。
取消済み ticket や source が更新された旧要求の失敗は、現在の画の失敗事実へ採用せず破棄する。
native 再レンダが失敗した場合は AI の再レンダ待ちも終え、保持した raster で既存の AI 経路を続ける。
まれな失敗は割り切り（ログ + `show_feedback_warning` による通知）。同じ組への通知は一度だけ。
画が残るだけでは品質更新の失敗に気付けないため通知し、再試行時間窓・回数・新しい要求表は作らない。

cache の長辺は通常どおり要求の 90〜110% で照合する。ただし極端に細長いページでは、
既存の比率精度 0.05% を満たす整数寸法が要求長辺の 90% 未満になることがある
（page box 比 1:160、要求 256px → 1×160px）。不足判定時だけ、worker と共有する
`render_dimensions_for_layout` を page box の固定小数点寸法へ適用し、実際に生成される長辺とも
照合する。live / retained raster の両方で同じ規則を使い、完了→再要求の反復を防ぐ。
renderer の寸法選択・画質は変更しない。metadata がない間は従来の要求長辺で比較する。
再レンダの共通入口でも既存の source-keyed retained metadata 採用を通すため、
snapshot 等で layout 投影だけを失った細長いページも、保持 pixel を繰り返し enqueue しない。

設計の簡素化として pending に合流し、別の「前フレームの需要」や時刻・再試行状態を足さない。
PDF 表示準備を modal にすると閲覧・ページ移動を妨げるため採用しない。

この合流は backlog §1.114 の修正（2026-10-21、`5392c03ac` + `5a7d3523e`、master 取り込み `124dc4828`）。
修正前はページ全体 / 幅 / 高さフィットの `ensure_pdf_display_resolution` が毎フレーム同じ要求を出し直して
進行中を取り消し、PDFium のレンダが一度も完了しなかった（利用者の 2026-08 のログ）。
ClaudeCode の隔離環境での操作では、合成 8 ページ PDF を全画面で幅フィット・先頭 / 末尾ジャンプ・ページ送りして、
再レンダ要求がそれぞれ約 0.2 秒で 1 回ずつ完了し、取消は 0 回だった（§1.165 の構造変更後のビルドでも同じ）。
再レンダ失敗で Static が Failed に置き換わる別件は、2026-10-22 の追補で上記の保持と失敗記録へ変更した。
source inspection の所見に加え、修正前の状態遷移テストで Static を失う失敗を確認した。
実アプリで失敗を起こした観測はない。再試行や多段の recovery 機構は追加しない。

WebP は `ANIM` chunk の背景色を `WebPDecoder::set_background_color` に渡してから
展開する。`image-webp` は dispose-to-background の処理を持つが、背景色未設定のままだと
dispose が実質 no-op になり、透明差分フレームで前フレームの軌跡が残る。

→ EXIF 適用済の DynamicImage に `clamp_dynamic_for_gpu` を掛けて長辺 8192 以内に縮小
   (wgpu デフォルト上限)。7K-9K クラスの画像で過去に UI スレッドで 5s 級の
   Triangle リサイズが走ってしまい応答なしになった実害から、worker で先に縮小する。

→ 結果: FsCacheEntry (Static / Animated / Failed) を fs_cache に格納
```

### 2.2.1 ViewerPresentation / detached viewer

v1.4.0 後の別ウィンドウ対応では、同じ viewer session をどこへ表示するかを
`ViewerPresentation::{MainWindow, Fullscreen, DetachedWindow}` で扱う。

現時点の実装は、`open_fullscreen` 入場時に `requested_viewer_presentation_for_open`
で F12 の `detached_viewer_enabled` を反映した要求表示先を導出し、
`effective_viewer_presentation_for_open` で実表示先を決めて `App.viewer_presentation`
へ保持する。静止画 / ZIP画像 / PDFページ / 動画は `DetachedWindow` が有効で、
`render_fullscreen_viewport` の描画本体を装飾付き・taskbar 表示ありの通常 viewport へ
出す。動画もこの egui detached viewport を安定した host として使い、
`NativeVideoPlacement::DetachedViewerChild` と `NativeVideoWindowMode::Child` で
host HWND のクライアント領域へ native presenter を重ねる。DComp presenter /
decoder / audio clock は保持したまま `SwitchPlacement` で MainWindow / Fullscreen /
Detached を切り替える。

`prepare_viewer_presentation_open` / `prepare_viewer_presentation_close` は、main HWND
cloak、native 動画の DWM chrome、foreground reclaim など fullscreen takeover 前提の
処理をまとめた境界。detached viewer では main HWND を cloak せず、fullscreen foreground
reclaim も予約しない。detached session はメイン一覧をブロックしないため、root viewport 側の
通常ショートカットやダイアログは継続して扱える。detached 表示の `×` / Esc / Enter は
`close_fullscreen()` に寄せ、`detached_viewer_enabled` は維持する。

ボーダーレス fullscreen session 中に main viewport がフォーカスを得た場合、既定では
「一覧へ戻りたい」意図とみなして `close_fullscreen()` する。環境設定
`fullscreen_keep_on_app_switch` (表示名: 「メインに戻ったらフルスクリーンへ復帰」) が ON
のときは、Alt+Tab などで mIV の main viewport へ戻っても閉じず、fullscreen viewport /
native presenter 側へ focus / raise を戻す。main/root 側へ届いた fullscreen 操作用キーは
`handle_fullscreen_root_key_input` で先に処理する。メイン一覧も並行操作したい場合は
detached viewer (F12) を使う。

F11 の MainWindow / Fullscreen 選択は、F12 の detached ON/OFF とは独立した
non-detached 側の表示設定として保持する。動画の native presenter から
`DetachedWindow` への `PlacementSwitched` が返っても `settings.video_in_window_mode`
は更新せず、F12 を OFF にしたときは直前の F11 状態へ戻る。detached 中の
F11 は non-detached 設定を更新せず、通常配置を保持したまま装飾なし・モニター全体の
仮想フルスクリーンをトグルする。動画 detached では native presenter を fullscreen
presenter に作り直さず、detached viewport host を広げて child presenter を親 client rect
へ追従させる。
静止画の fullscreen viewport と detached viewport は装飾・サイズ・taskbar 表示が異なるため、表示形態が
変わるときは既存 viewport を隠して ViewportId 世代を進め、新しい表示先として作り直す。
静止画 / PDF / ZIP 画像の fullscreen viewport と detached viewport はどちらも hidden
状態で作成し、`DWMWA_TRANSITIONS_FORCEDISABLED` を適用してから `Visible(true)` を送る。
これにより OBS などの window capture には映らない DWM の表示フェード / 出現アニメーションを
抑止する。

静止画 fullscreen viewport は true fullscreen API へフォールバックせず、対象モニターの
論理矩形へ装飾なし viewport を配置する。Windows 11 仮想デスクトップで静止画 fullscreen
viewport が現在デスクトップへ付いてくる症状を避けるため、捕捉できた fullscreen viewport
HWND は main HWND と同じ仮想デスクトップへ best-effort で同期する。動画 fullscreen は
native presenter 経路であり、この egui viewport 同期の対象外。

複数 viewport の close / recreate では、過去に CPU 側 font atlas と renderer texture の
サイズがずれ、日本語フォルダ名の部分 glyph upload (`Y 29..44`) が古い高さ 32 の texture を
越えて validation panic した。この原因は viewport teardown そのものではなく、生成済みの
full atlas delta を配送前の早期 return が捨てていたことだった。

**2026-08-14 の根本修正**は、所有境界を次の 2 層で閉じた。

1. `Painter::paint_and_update_textures` は surface の有無を調べる前に
   `textures_delta.set` / `free` を共有 renderer へ適用する。
2. `vendor/eframe/src/native/wgpu_integration.rs` は `egui_ctx.run()` 後に
   viewport / window / egui-winit state が消えていた 4 つの早期 return でも
   `Painter::apply_textures_delta` を呼ぶ。

egui は delta を `Context::end_pass` で一度だけ取り出すため、不変条件は
**「生成された frame は、描画されなくても texture delta を renderer へ必ず一度渡す」**である。
painter 内だけを直した build では caller が painter を呼ばない境界で再発したため、両層が必要。
この契約は font atlas と thumbnail の双方を同じ場所で保護する。

**2026-08-27 の所有境界再監査**では、main の egui viewport 群は同じ `Context` /
`Painter` / `render_state` と renderer texture namespace を共有し、detached viewport の
破棄は font atlas texture を所有も破棄もしないことを確認した。native video presenter は
独自の `egui::Context` と `egui_wgpu::Renderer` を持ち、main atlas の再発行先ではない。
presenter は生成時に現在の `Settings.ui_font` で自分の font definitions を構成する。

実機計測では全 atlas delta が `site=paint outcome=Submitted` で、surface-less frame へ
当たった発行は 0 件だった。一方、旧 `native_video_backdrop_hide` が F12 toggle ごとに
5 frame の discard を作り、`detached_viewer_cleanup` は先行 pending を re-arm しただけの
場合にも同じ時間窓を延長していた。したがって `d48982e5` で採用した「cleanup は stale atlas
を避けるため保守経路へ乗せる」という判断の前提は、delta を落とす配送バグの修正と実測によって
失効した。

なお、`hide_embedded_still_viewport_if_shown` のコメントには 2026-06-18 時点の実機観測として
「動画 backdrop 用 resync の相乗りが main atlas 破損の発火源だった」と書かれていた。
この観測自体は今の理解と矛盾しない。resync は full atlas の realloc を強制するので、
**落とされると致命的になる大きな delta を作っていたのは resync そのもの**だった。
つまり当時の帰属 (「相乗りが発火源」) は機構としては正しく、欠けていたのは
「なぜその delta が届かないのか」という配送側の問いだった。2026-08-14 の修正で配送が保証され、
2026-08-27 に resync 自体を撤去したことで、同じ破損の登場経路が両端から閉じている。

現在は `detached_viewer_cleanup`、`fullscreen_viewport_cleanup`、
`native_video_backdrop_hide`、`fullscreen_viewport_recreate`、
`still_window_mode` のいずれも font work を発行しない。固定 repeat、marker を加えた full
reload、`request_discard` / repass、安全待ち、thumbnail upload 先送りも持たない。
viewport lifecycle の font firing 数は **0** が契約であり、1 回または acknowledgement 待ちではない。

**緩和策: クリア色をテーマ連動にする (`App::clear_color` / `main_window_clear_color`)**。
detached 窓 close 直後の DWM / top-level HWND 再合成中にメインウィンドウ surface の地として
見える eframe 既定の `clear_color` 値は
near-black `(12,12,12, a=180)` 固定。ダークテーマでは panel_fill と馴染むが、ライトテーマでは
「一瞬黒」が目立つ (再現性が高いのは `auto_fullscreen_zip_pdf` ON で PDF を直接フルスクリーンに
開き、Esc → `pending_return_to_parent` で親フォルダを `load_folder_or_convert_archive` 再読込する
経路。重い再読込が描画スキップフレームを安定して踏む)。`eframe::App::clear_color` を
オーバーライドして `visuals.panel_fill` (= メニューバー / パネル背景) を不透明で返すことで、
ライト/ダークどちらでも地と馴染ませる。これは font atlas の正しさとは独立した表示緩和で、
top-level HWND teardown の再合成自体を消すものではない。回帰ガードは
`main_window_clear_color_follows_theme_panel_fill`。

実際の UI フォント設定変更だけは `request_main_font_update` が pending を 1 つ所有する。
同じ update 前の複数変更は coalesce し、次の main `update` で現在の `Settings.ui_font` を
`configure_fonts_with_settings` に一度渡す。`set_fonts` は次 pass の `begin_pass` で適用されるが、
それまでの atlas は有効なままなので pass を discard しない。新しい delta の配送は上記 backend
契約に任せ、caller 側の repeat / acknowledgement / frame count は追加しない。

なお、専用ボーダーレス fullscreen viewport を閉じると、別トップレベル窓の `Visible(false)` が
DWM で反映されるまで約 1 フレームかかるため、メインウィンドウが手前に合成されてから fullscreen
窓 (黒) が消えるまでの間、メイン周囲に一瞬黒地が残る。これは上記の panic / 黒コンテンツ問題とは
別の DWM 表示タイミング由来で、単一 viewport 化以外では完全には消せないため現状は許容している。

detached session が開いている間は、`App::update` 終端でそのフレームの最終
`selected` を読み、静止画 / ZIP画像 / PDFページ / 動画であれば viewer 側を同じ項目へ追従させる。
同期済み判定は `idx` だけでなく `metadata_cache_key` 相当の item key と
`items_generation` を含む stamp で行う。これによりフォルダ切替や ZIP/PDF 仮想フォルダの
items rebuild 後に、同じ idx が別項目を指しても再同期できる。detached window が閉じている
場合は、メイン一覧のカーソル移動だけでは再表示しない。
同じ raw idx で stamp が変わった場合は、動画 fast-swap の same-idx no-op を通さず
`open_fullscreen` の通常初期化へ戻して再同期する。

`settings.detached_viewer_open_images_in_window` または detached 画像ビューア上バーのピン留めが
有効な画像 / ZIP画像 / PDFページ session は、メイン一覧との自動同期を行わない。次の画像を
開くとき、現在の active detached viewer は `TextureHandle` と表示位置だけを持つ passive
detached image window として退避される。passive window は最後の画像を表示するだけで、active
viewer cache / AI / 先読み / スライドショーは単一 active session のまま扱う。編集状態は
detached bundle 間で保持・確定しない方針とし、always-new / ピン留めの連動なし窓では
消しゴム・補正レイヤー・隠蔽加工・テキスト注釈・切り取り等の編集機能を起動できない。
通常 F12 の linked detached viewer では従来どおり編集できるが、編集中はピン留めできない。
passive snapshot の paged 経路は、pause 時点で最後に実際にpaintされたpresentationをtypedに選ぶ。
navigation / source-reload holdoverがlive pageの後にoverlayされていれば、その旧resource unitをcloneし、
index再利用後のcurrent itemを取り直さない。holdoverが無ければcurrent live unitをcaptureする。選択は
read-onlyで、navigationの`Ready` phaseを進めずholdoverもclearしない。resource unitへ、painted
`fullscreen_page_layout`（無ければcanonical composition）のexact `ResolvedDisplayPlacement`を合成した
`CapturedPagedDisplayUnit`が唯一のbake入力である。

unitはtexture、保存回転、canonical layout size、source size、trim/content bbox、post filter、page /
occurrenceとtyped placementを一体で保持するため、passive側でcurrent item、設定、book preference、縦横を
読み直さない。2 page見開きは2件、`Left / Right`のendpoint singleton、Z / ZipPla single、holdover singleは
1件のfrozen pageとして、liveと同じ`DisplayedImageTransform`、fit / scale limit、物理pixel量子化済みgap、
zoom / pan、自由回転、visible-source UV / clip規則で焼き込む。Zはactive / aimingとfactor / zoom-pan /
singleton side/gapをcapture-time placementから取り、active doubleだけgapをzoom倍、fit scaleはzoom前のgap
から求める。page / occurrence / requestはsingletonでも1件のままである。通常liveの`Center`（設定OFF、
本別Center、横長 / 非pairable境界を含む）は従来のdirect-single snapshotを維持し、既定値のgeometryと
丸めを変えない。paged bakeは既に確定したplacementと`pixels_per_point`だけを受ける純境界なので、parkの
snapshot captureがegui `Context`を渡さない場合も同じ結果になる。縦連結読みはpause時点で可視だった
各ページの`TextureHandle`と正規化済み表示矩形をfrozen page listに保持し、layout計算にegui `Context`が
必要なため`Some(ctx)`のcapture時だけ従来どおり作る。必要なpage textureを取得できない場合はmulti-page
frozenを作らず、従来の現在ページ1枚snapshotへフォールバックする。
CUT 後、linked window は passive 化しない。設定 OFF の F12 linked window は最大 1 枚で
メイン一覧に追従し、別の independent / ParkedLive window を Active 化する場合は閉じる。
設定 `detached_viewer_open_images_in_window` が ON のときだけ、画像 / ZIP画像 / PDFページを
independent passive window として残す。動画は複数窓化せず、専用の detached 動画 window を
再利用する。

メイン一覧で `Enter` / 明示 open した項目が、開いている detached session の stamp と
同一なら、`open_fullscreen` を再実行せず前面化要求だけを出す。静止画 detached viewport
では `Minimized(false)` / `Visible(true)` / `Focus` を送り、動画 child presenter では
host 内で presenter raise 要求へ寄せる。同じ raw idx でも item key / generation が変わっている場合は、
通常どおり再オープンして表示状態を更新する。

F12 linked detached session で見開き 2 ページ表示中は、メイン一覧の通常カーソルを現在ページに置き、
相方ページがグリッド / 詳細一覧の可視範囲内に描画される場合だけ破線のサブカーソルを重ねる。
相方がスクロール外なら追加描画は行わない。複数ウィンドウモード / independent detached session は
メイン一覧との見開き連動表示ではないため、破線のサブカーソルを描画しない。detached 動画はメインウィンドウを占有しないため、
fullscreen / in-window 動画用の main backdrop や black chrome 判定から除外する。

detached window placement は `settings.detached_viewer_window_placement` に保存する。
保存値の意味は outer position + inner/client size + maximized flag。最大化中は restore
placement を上書きせず、`maximized` だけを更新する。

静止画フルスクリーンの左右パネル召喚は Settings の `FsSidePanelMode` を正本にする。
`Hover` は左端だけが補正パネル、右端だけがメタデータパネルを召喚し、上端は上バーの
独立経路だけを動かす。`ClickToShow` は端ホバーでパネルを開かず、通常ホバー帯より狭い
最端帯で呼び出しバーを表示する。ClickToShow の左右開状態は per-file transient で、
ファイル移動とフルスクリーン退出で閉じる。右は `App.fs_click_info_open`、左は面ごとの runtime
flag を使い、Settings には保存しない。端判定とモード別召喚判定は `ui_helpers` の純関数へ置く。
動画では左をジャンプ / ブックマーク、右をメタデータ / タグとして独立させ、`Hover` は従来の
二段ラッチ、`ClickToShow` は presenter-local な左状態と App から同期した右状態で可視性を決める。
egui 音楽ビューも同じ mode と App runtime を使う。3 面とも ClickToShow では通常の端ホバーを
無効化し、最端の painter 描画 callout から明示的に開閉する。
左右いずれかの実パネルが表示中なら上 + 下のクロームも同時表示する。動画は既存の
`panel_chrome_visible`、音楽は上下常時表示、静止画は render 経路で集約した
`side_panel_visible` を上バーと `FS_SEEK_BAR` の force 条件に使う。辺ごとの召喚分離は維持し、
右端ホバーだけで左編集パネルを開くことはない。
静止画では callout / 右パネルの描画可否と wheel・click の当たり判定を同じ述語で決める。
補正編集、表示トリム、分析、比較 wipe、360、ズーム、アニメ中など描画抑止中は、非表示の
右パネル矩形や ClickToShow の最端帯も入力を奪わない。

### 2.2.2 動画フルスクリーンとタイルモード

`GridItem::Video` は画像ロードワーカーを使わず、`App::start_fs_load` の動画分岐で
`FsCacheEntry::Video { player }` を直接 `fs_cache` に挿入する。Windows の native
presenter 有効時は `VideoPlayer` が `NativeVideoOutput` を持ち、動画フレームと
egui overlay は専用 HWND 側で表示される。フルスクリーン viewport は黒い backdrop と
focus/chrome 管理だけを担当する。

native presenter の動画 scaling は `Settings::video_scale_filter` を正本にする。既定の
`OS に任せる` は source 解像度 swap chain と DComp scaling を維持する。`標準` は拡大・縮小を
Lanczos3、`シャープ` は拡大を NIS、`ニアレスト` は拡大を nearest で解決し、後二者も縮小時は
Lanczos3 を使う。縮小の支持幅は静止画と別の `video_downscale_smoothing_percent` で調整する。
SAR / orientation 適用後の映像表示矩形を物理 pixel 単位で fit し、GPU / CPU frame の双方を
source 解像度の grade pass (補正時だけ中間 RT) から表示寸法へ渡す。物理 1:1 は resample を
通さず既存 copy path のままにする。

切替は prepare / commit に分け、prepare が現在寸法用の中間 resource と未接続の次 video surface
を作った後、同じ request / source / viewport / resize state であることを照合して commit する。
commit と一時停止中の held frame 再提示では compile / allocation を行わない。window resize stream
中は現在の surface を DComp が stretch し、寸法 sample が 150ms 静止した edge で 1 回だけ交換する。
表示 surface 作成不能は typed reason を perf へ出して OS path へ戻し、同じ要求を frame ごとに
再試行しない。長辺 8192px または総画素数 16,777,216px を超える表示矩形はサイズ制限として同じ
fallback に入り、左パネルの設定位置へ理由を表示する。

Windows のタスクバー hover preview は main HWND を対象にするため、動画 fullscreen 中は
`dwm_iconic_thumbnail.rs` が DWM iconic thumbnail/live preview を main HWND に供給する。
フレーム抽出は worker thread 上の `video::screenshot::capture_frame` で粗く更新し、
DWM の WndProc 要求内では cached bitmap だけを返す。

動画から動画へのフルスクリーンナビゲーションでは、Windows native presenter 経路で
`NativeVideoOutput::SwitchSource` を使い、HWND / D3D11 presenter / overlay を保持したまま
新しい `VideoPlayer` の source binding に差し替える。通常ナビゲーションでは 120ms の
quiet period を置き、その間は decoder を作らず `NativeOverlayNavigationPreview` が
移動先ファイル名と保存済み resume サムネ (無ければ黒背景) を表示する。quiet period 後に
最新 target だけを open し、`native_video_fast_swap_pending` が最初の native frame 表示まで
連続入力を抑制する。navigation preview も最初の native frame が実際に表示されるまで
残し、decoder startup 中に旧動画の frame が露出しないようにする。navigation preview は
source swap 中の受動表示なのでカーソル idle をリセットせず、キーボードでの動画送りだけでは
非表示カーソルを復活させない。画像⇄動画の遷移はこの fast path の対象外で、従来どおり
`open_fullscreen` / `start_fs_load` 経路で扱う。

mIV Remote の動画ストリーミングは fullscreen cache / native presenter を使わず、remote session
state が headless `VideoPlayer` を所有して UI frame ごとに tick する。decoder と video/audio tap
は継続するが、folder load、`open_fullscreen`、detached presentation は要求しない。そのため本体の
既存表示を変更せず、「リモート接続中」modal を表示したまま配信できる。音声モードの `Music`
surface は従来の music-only predicate のままで、remote streaming 専用 surface は持たない。

動画タイルモード (`video_tile_state`) は再生中動画の `VideoInfo` からタイムスタンプ列を
作り、`TileThumbnailWorker` が別 FFmpeg input でサムネイルを抽出する。タイルモードが
active の動画→動画ホイール移動でも同じ `SwitchSource` を使うが、
`video_tile_swap_pending` 中は preparing overlay を出し、新動画の `player.info()` 到着後に
新しい `VideoTileState` を構築してタイルを progressive に埋める。
タイルモード中の P は、キーボード操作で選択中のタイルカーソル位置の timestamp と
抽出済み RGBA を `video_pins.db` へ書き込み、
その動画の代表フレームとして使う。タイル画像がまだ未抽出なら、通常の動画 P と同じ
seek thumbnail 待ちの保険経路にフォールバックする。

タイルモードの fast path では保存済み resume 位置を新 `VideoPlayer` に渡すが、autoplay は false
に固定する。タイル解除後は resume 位置の静止状態を表示し、特定位置から見たい場合は
タイルクリックによる seek を使う。タイルクリックは autoplay 設定に関係なく、その位置へ
seek して再生開始する。切替元の `VideoPlayer` は cpal stream を同期 pause し、audio
buffer を明示クリアしてから native presenter を渡し、前動画の処理済み音声が短く漏れる
のを抑える。

静止画フルスクリーンでも、動画再生中と同じ `fullscreen_cursor_hide_delay_secs` 設定を使う。
UI / パネル / ダイアログが出ていない状態でマウス操作が止まると `CursorIcon::None`
に加えて viewport の `CursorVisible(false)` を送るため、スライドショー再生中も OS カーソルが
画面上に残らない。スライドショーのタイマー送りやキーによるページ送りではこの idle 状態を
次画像へ引き継ぎ、画像切替だけでカーソルを再表示しない。`open_fullscreen` の cursor
リセットは fullscreen 新規入場向けであり、fullscreen 内ナビでは呼び出し元が状態を引き継ぐ。
非表示状態は `cursor_hide_reason: Option<FsCursorHide>` が単独で所有し、`Idle` は従来どおり
ラッチ、`ZoomAiming { restore_idle }` は `fs_zoom_aiming` から毎フレーム導出する。照準中の
pointer activity は `ZoomAiming` を解除せず、照準前から `Idle` だった場合は payload で保持して
照準終了後に戻す。active fullscreen の `CursorVisible` と `CursorIcon::None` は同じ apply 境界だけが
送り、理由が変わった frame だけ visibility command を送りつつ、非表示中は icon を毎フレーム適用する。
カーソル非表示中は最後の hover 座標を stale とみなし、上バー / 右パネル /
補正パネル edge hover / panel 内判定など、passive hover 由来の状態遷移では
`!cursor_hidden` で gate する。マウス操作または固定 UI 表示が戻ったら
`CursorVisible(true)` で復帰する。

#### 2.2.2 右パネルへの動的状態反映

`VideoInfo.dynamic: Arc<VideoDynamicState>` は decoder thread / present thread / UI で
共有する atomic 群。`VideoPlayer::open` で 1 度生成し、以下の経路で同じ Arc が伝搬する:

- `decoder::spawn` → `run_video_decode` (video-decode スレッド) が `deinterlace_status` /
  `interlace_detected` を更新
- `NativeVideoOutput::spawn` → `run_native_video_output` → `PresenterSourceState::new`
  → `NativeFullscreenPresentStats::new(dynamic)` で保持し、`record_present` が
  `present_path` を per-frame 更新
- 動画→動画 fast-swap 時は `SwitchSourcePayload.dynamic` に新 source の Arc が乗り、
  旧 presenter が新 PresenterSourceState (新 Arc を握る present_stats) に切り替わる

UI 側 (`app/native_video.rs::sync_native_video_metadata`) は overlay 同期時に Acquire load
して `NativeOverlayMetadata` の動的フィールド (`last_present_path` /
`deinterlace_status` / `interlace_detected`) に snapshot 化、右パネル
`overlay_draw.rs::draw_native_metadata_panel` が「フレーム表示」「デインターレース」行
として描画する。

per-frame 経路 (`d3d11_shared` / `cpu_upload`) はプレゼン側の判定で、デコード自体が HW
かどうかとは独立。デインターレース (FFmpeg `bwdif`) を使う場合は HW デコード後に D3D11
テクスチャを CPU メモリへ転送するため、`record_present` は `cpu_upload` を観測する
(= 右パネル「フレーム表示」が CPU、perf overlay の CPU カウンタが増える)。
既存の静的フラグ `gpu_path_active` (= GPU video device の有無) は能力フラグとして
右パネル「GPU経路」(改名前は「経路」) にそのまま残し、per-frame の動的状態は別行で
表示する。

**リサイズ実装 (`src/fast_resize.rs`)**:

リサイズは `image::imageops::resize` (スカラー) ではなく、`fast_image_resize`
(AVX2 / SSE4.1 SIMD) ラッパの `crate::fast_resize` 経由で呼ぶ。実測で 3-10 倍速く、
7K-9K クラスの画像でも数百 ms で完了する。フィルタは `Quality::Bilinear` (≈ Triangle) と
`Quality::Lanczos3` の 2 択。使用箇所:

- `clamp_dynamic_for_gpu` — GPU 上限クランプ (Bilinear、縮小前提で速度優先)
- `thumb_loader::resize_to_display_color_image` — 表示用サムネ (Lanczos3、品質重視)
- `catalog::encode_thumb_webp` — キャッシュ用サムネ (Lanczos3、品質重視)

表示用 / catalog 用サムネイルの整数寸法は `fast_resize::aspect_accurate_fit_dimensions` を
共用する。長辺を上限へ固定して短辺を 1 回だけ丸めると、PDF のように raster ごとの丸めが
異なるページで 0.1% 程度の縦横比誤差が残る。そこで width / height の両軸から整数候補を
列挙し、元の縦横比に対する相対誤差 0.05% 以下を満たす最大解像度の候補を選ぶ。PDF page box
(`layout_dims`) や JPEG DCT 縮小の元ピクセル寸法 (`source_dims`) のように decoded raster より
正確な比がある場合は、その比率を選択器へ渡す。PDFium の `fit_to_target` も page box を同じ選択器へ渡し、低解像度 raster を
最初から選択済み整数寸法で描画する。上限を超える拡大はせず、既存 catalog 行も強制再生成
しないため、この改善は新規生成分から適用する。

新規リサイズ経路を増やすときは `image::DynamicImage::resize(_exact)` を直接呼ばず、
`fast_resize::resize_dynamic_fit` / `resize_dynamic_exact` を使うこと。`image` crate の
`resize` は UI スレッドに乗ったとき秒単位の応答なしを招きやすい。

**GPU テクスチャ上限の規約 (MAX_TEXTURE_DIM = 8192)**:

- `fs_cache` / `edit_result_cache` / `final_ai_cache` / `retained_final_ai_cache` /
  `final_composite_cache`
  (および 360 モード等で残る `ai_upscale_cache` / `adjustment_cache`) に入る pixels は
  **常に 8192px 以内**。worker 側 `clamp_dynamic_for_gpu` で担保される。
- UI スレッドの `clamp_for_gpu(&ColorImage)` は異常経路の安全網。通常パスでは
  `Cow::Borrowed` で返り、Triangle リサイズは走らない。発動したらログに
  `clamp_for_gpu (UI-thread fallback)` が出る。
- AI アップスケールは `ai_upscale_size_limit` (長辺 x 短辺、既定 2048 x 2048。
  旧 `ai_upscale_skip_px` からの読み替えは `Settings::ai_upscale_limit()`) で
  対象を制限する。`4096 x 2048` 等の大きい上限では ×4 出力が 8192 を超えるため、
  final pipeline は upscaler に `MAX_TEXTURE_DIM` を渡し、タイル出力を最終クランプ後
  サイズへ直接合成する。`clamp_color_image_for_gpu` は安全網として残すが、通常は
  no-op になる。final AI / 旧 AI 経路とも判定は `ai::upscale::should_process_rect`。
- `apply_adjustments_fast` は pointwise 変換なので入力サイズを保つ → 入力が 8192 以内
  ならば出力も 8192 以内。`edit_result_cache` / final AI 結果を入力に取るので成立する。
- 消しゴム (MI-GAN) / PDF 再レンダも同じ 8192 上限を尊重する。PDF vector は長辺
  256 以上、raster は native 上限を優先するため極小原稿では 256 未満になり得る。
- GIF / APNG / WebP アニメーションは `fs_animation::clamp_rgba_frame_for_gpu` で各フレームを
  `MAX_TEXTURE_DIM` 以下に縮めてから `ColorImage` 化する (巨大 animated 画像で
  `ctx.load_texture` が panic するのを防ぐ安全網)。

新しい経路で `FsCacheEntry::Static` を作るときは、`pixels` が 8192 以内であることを
自分で保証するか、`clamp_dynamic_for_gpu` を掛けてから格納する。UI スレッド側の
同期 Triangle リサイズを増やさないこと。

**mip chain を保持する経路**:

- `DISPLAY_IMAGE_TEXTURE_OPTIONS` を指定した managed texture は、ローカル差し替えした
  `vendor/egui-wgpu` が level 0 upload 後に完全な mip chain を GPU render pass で生成する。
- 対象は raw static、PDF/ZIP page、編集・補正・AI・注釈・比較用の表示 texture。Windowsの
  wipe/diff比較と360度パノラマの独自`Rgba8Unorm` textureも同じGPU生成器を使う。パノラマは
  水平フル/垂直cropではU=Repeat、水平cropではU=ClampToEdgeを選び、低LODで部分画像の
  反対端が混ざらないようにする。経度シームではU微分を周期補正して`textureSampleGrad`へ渡し、
  シームだけ過度に粗いmipが選ばれることを防ぐ。比較callbackは現在のpinned/current textureと
  mip chainを1組だけ保持し、入力失効・別サイズの再準備時に旧組を解放する。Cの通常表示へ戻る操作では
  準備済みpinned buffer / managed textureを保持し、同じcurrentへ再表示するときはworkerを起動しない。本文 / ナビゲータのuniform bufferと
  bind groupはtyped slotごとに分ける。右下のピン表示は72x54以下の専用textureを使う。1つの
  `TextureHandle` 内に全 level を保持するので、表示 texture の優先順位、論理サイズ、zoom、
  見開き、連結読み、ルーペ、pixel grid の座標系は変更しない。
- animated frame、動画、サムネイル、mask、checker、UI texture は対象外。明示的な
  `PostFilter::Nearest` も level 0 + nearest sampler のまま。
- 通常静止画の縮小は level 0 を入力に §2.4.1 の Lanczos3 出力を生成する。通常静止画向けの
  LOD bias / level 0 切替 uniform と renderer setter は持たない。wipe/diff 比較は固定の
  implicit mipmap sampling、360度パノラマは周期補正した勾配による `textureSampleGrad` を使う。
  「縮小時のなめらかさ」は通常静止画の Lanczos3 支持幅だけを変更し、比較・パノラマには影響しない。
- mip texture の partial update では全下位 level を再生成する。完全な chain の VRAM は level 0
  の約 1/3 増えるため、フルスクリーンの既存 prefetch / eviction 境界を越えて保持しない。

旧 LOD 設計の履歴は [downscale-moire-lod-plan.md](downscale-moire-lod-plan.md)、現在の決定は
[dot-by-dot-and-downscale-plan.md](dot-by-dot-and-downscale-plan.md) を参照。

**原寸表示とダウンスケール警告 (`source_dims`)**:

`FsCacheEntry::Static.source_dims: Option<[usize; 2]>` は **clamp 前** の原寸。
fs_load ワーカーが `clamp_dynamic_for_gpu` を掛ける直前に記録して送る。ホバーバーの
画像サイズ表示はこれを優先して使い、`pixels.size` と不一致なら「⚠ GPU上限により解像度
制限中」マーカーを出す (利用者が縮小表示に気づけるように)。文言の定義元は
`ui_fullscreen/draw_icons.rs` の `DOWNSCALE_MARKER` 一箇所で、描画側の色分け切り出しと
テストも同じ定数を読む。

派生キャッシュ (`edit_result_cache` / `final_ai_cache` / `final_composite_cache` など) や
消しゴム再挿入の entry は `source_dims: None` で良い。ホバー UI は `fs_cache` 側のエントリから原寸を読むため、
派生側は参照されない。ただし消しゴム inpaint / マスク解除で `fs_cache` を上書きする
ケースは既存 entry の `source_dims` を必ず引き継ぐこと (上書きで原寸情報が消えて
警告が出なくなる事故を防ぐ)。

**先行 dims ヒント (`fs_early_dims` / `FsLoadResult::DimsOnly`)**:

フルデコードには数百 ms-数秒かかるが、画像の**寸法だけ**ならヘッダ数バイトで
取れる (PNG の IHDR、JPEG の SOF マーカー等、通常 2-10 ms)。この時間差を活かして
「ロード中はホバーバーに何も出ない」状態を短縮する経路を設けている:

1. `start_fs_load` ワーカーは perf `decode_begin` 直後、ローカル画像パス限定で
   `fast_resize::probe_dims(&path)` を呼び、成功したら `FsLoadResult::DimsOnly {
   source_dims }` を先行送信する (PDF / ZIP は probe がそもそも遅いので対象外)。
2. `poll_prefetch` は各 `fs_pending` に対して **drain ループ** で try_recv を回し、
   `DimsOnly` を受信したら `App::fs_early_dims: HashMap<usize, [usize; 2]>` に格納し、
   fs_pending はそのまま残す (本デコードが続く)。
3. ホバーバー (`build_fs_frame_state`) は `fs_cache` にエントリがないケースで
   `fs_early_dims` を見に行き、あれば原寸をそのまま表示。原寸が MAX_TEXTURE_DIM を
   超えていればダウンスケール警告もこの時点で出せる (本デコード完了まで待たない)。
4. 本体 (`Static` / `Animated` / `Failed`) 受信で `fs_early_dims[idx]` は削除される。
   `load_folder` / キャンセル時も一緒に drop して HashMap が肥大化しないようにする。

この設計は `DimsOnly` 省略時 (probe 失敗 / PDF / ZIP) でも問題なく動く。drain ループが
終端メッセージ 1 個で `completed` に積んで抜けるだけなので、従来挙動と互換。

### 2.3 表示テクスチャの優先順位 (決定版)

`ui_fullscreen.rs::resolve_fs_processed_texture` はフレームごとに以下の順で「今表示する
テクスチャ」を選ぶ。通常表示は `edit_result_cache` に `AdjustParams` / AI /
`post_filter` を最終段で重ねた `final_composite_cache` が正系で、edit 系の cache は
常に source 解像度・補正前の空間を保つ:

```
0. 静止中に元画像表示の割り当てをホールド中
   (既定は右 Ctrl。fs_cache の raw decode。ナビゲーションシーケンス進行中、または
   ページ送り入力の物理ホールド中は適用しない)
1. erase / local_adjust / conceal の編集中プレビュー (各 UI の in-memory state)
2. final_composite_cache[edit_key, params_hash, bg]
   (= edit_result_cache + 色調補正 + final AI + スマートシャープ + カラー化
    + Creative LUT + post_filter。
    スマートシャープは AI アップスケール出力には掛からない —
    preset-and-adjustment.md §2.7)
3. edit_result_cache[edit_key]
   (= raw -> erase -> local_adjust -> conceal。crop を含まない最終段待ちの fallback)
4. fs_cache[idx] (生デコード結果、raw 専用)
5. フォールバック: サムネイル (低解像度)
```

AI 待ちの `complete=false` final composite も表示専用の候補である。final AI が到着しても
同一 `FinalCompositeKey` の entry は消さず、AI 後の final-effect worker が完成するまで
カラー化済みの暫定 texture を表示し続ける。完成結果の `insert` が同じ key を上書きするため、
表示はキャッシュ欠落を挟まず原子的に差し替わる。コピー / 書き出しと nav lock 解除は従来どおり
`complete=true` だけを受け付ける。

#### ページ単位表示のキーリピート

v2.13.0 で一度削除した通過表示は、2026-08-12 に単一 / 見開き共通で再実装した。physical
page-turn key level が held で、かつ実際の display-unit 遷移が成立したバースト中は、色忠実な
低解像度 rendition を全ページ一様に選び、final-effect worker の完了回収、`fs_upload_backlog`、
full decode / AI producer を保留する。境界で遷移しなかった入力では通過表示に入らず、release 後
(または held 中に境界へ達した後) は current display unit を `resolve_fs_processed_texture` の
通常経路へ戻す。詳細な要件と失敗から得た知見は §2.5 を正本とする。

`colorize_display_requires_final_effect(idx)` は materialized 経路で、設定が有効かだけでなく、
そのページで待たずに出せる画素と final-effect worker の出力が視覚的に変わるかを表す gate である。
`AllImages` は常に gate する。`MonochromeOnly` の高速パスは色調補正が identity の場合だけ使い、
`edit_result_cache`（raw → erase → local_adjust → conceal）の画素から作った近モノクロ要約
（主成分軸からの p95 直交残差）が現在の `mono_tolerance` を超えるページでは gate を外す。
これにより編集レイヤーが raw の chroma を落とした場合も、worker が見る入力と同じ編集結果を
基準にできる。色調補正が非 identity なら、その後の adjusted 画素が別物になるため安全側に gate
する。final AI は復元・拡大処理で入力の近モノクロ性を反転させないものとして扱う。
この条件を満たすカラー画像でカラー化が no-op になるページだけ、raw / edit / thumbnail fallback
を許可する。
通過 rendition ではこの保留 gate を「カラー化を適用する」という判定に流用しない。常駐する
完成 composite の実判定、既存 full-size summary、調整後の低解像度画素の順に 1 つの適用可否を
解決して rendition に保持し、着地点の final-effect worker も同じ判定を使う (§2.5.3)。
Creative LUT も**単独で** gate 条件に入れる。LUT もカラー化と同じ「色が変わる最終段」であり、
待たないと LUT 未適用の絵が 1 フレーム見えてから色が変わる (ユーザー報告 2026-07-29)。
gate している間の Ctrl+↑↓ は捨てず、folder-nav の accumulator が受け取る
(`handle_fullscreen_ctrl_nav_context` の lock 分岐と `KeyAction::press_multiplicity`)。
一度この gate を LUT から外した経緯があるが、当時の症状 (押下が消える) の原因は accumulator 側で、
別途修正済みである。post_filter 単独は同期合成なので gate 対象にしない。

近モノクロ要約は `idx → { EditResultKey, p95_residual }` で memo 化し、`poll_prefetch` の末尾から
1 frame 最大 4 件、現在の `fullscreen_idx` 最優先で計算する。producer は既存の
`edit_result_cache` entry だけを走査し、重い編集合成を新規生成しない。`EditResultKey` は source
generation と erase / local-adjust / conceal の各編集世代を含むため、再デコード、PDF 再レンダ、
編集操作のいずれでも自然に stale になる。memo miss、現在 key の edit result 不在、または
`EditResultKey` 不一致は安全側の `true` とし、要約が完成するまで従来どおり生画像へ
フォールバックしない。`edit_result_cache` から消えた entry は次の reconcile で memo からも落とす。

ページ枠での最終的な fallback 順は、`final/processed` → ページ単位表示で final-effect 待ち中の
色忠実 rendition → 連結読みの `continuous_page_transition_texture` → 未処理サムネイル →
`読込中...` である。見開きの final-effect 待ちはページ別に fallback せず、片側でも final が
未完了なら左右とも rendition、全ページが揃った frame で左右とも final へ切り替える。

`fs_holdover_tex` はこの優先順位には入らない。`FolderNavigation` /
`FinalEffectSourceReload` ともページ画像と編集 overlay の後に、共通描画で黒背景 +
退避した単ページ / 見開き全体を重ねる。ただし解放条件は統合しない。フォルダ移動は
`fs_nav_locked_gen` と新 anchor の表示 readiness、source reload は**同じ表示ユニット**の
全ページで final-effect 適用済み表示（または終端の読込失敗）が揃った描画 frame を使う。

Ctrl+↑↓ の nav lock 解放判定も、描画側と同じ
`fs_display_bypasses_final_pipeline` を使う。通常表示でカラー化が有効なページは
`complete=true` の final composite まで待つ一方、元画像表示と分析モードは
raw `fs_cache` を直接描くため、Static / Animated またはサムネイルの到着で解放する。
この表示モード判定を描画側と lock 側へ別々に実装してはならない。表示だけ raw に
切り替わって composite が生成されない場合、lock 側だけが完成を待ち続けるためである。

**この優先順位は動かさないこと**。変更すると「補正を掛けた瞬間に一瞬生画像が見える」
「AI 処理中にプリセットを変えると古い final composite が残る」等の不整合が出る。

実装上は `ui_fullscreen.rs::resolve_fs_processed_texture` を通常表示の共通入口にする。
単ページ、見開き、連結読み、ルーペが `edit_result_cache → fs_cache` のような独自チェーンを
再実装すると、新しい派生レイヤ (消しゴム / 隠蔽加工 / AI など) の横展開漏れが起きる。
見開きと連結読みも、画面全体で 1 枚を解決するのではなく、描画対象の **各ページ idx**
についてこの入口を呼ぶ。ページ単位の見開きで final-effect が未完了なら、各 idx の結果を
個別公開せず左右とも色忠実 rendition に揃える。同一 unit の source reload 中だけは
`FinalEffectSourceReload` が両方の final 完成まで旧 unit overlay を全体へ重ねる。
ルーペも hit-test 後に選ばれたページ idx で同じ入口を呼ぶ。

local-adjust / conceal の materialization は worker で進む。必要な local 結果が未完成なら
conceal は erase / raw を入力にして先へ合成せず `None` を返し、edit / final assembly も
未完成の上位結果を作らない。表示側はその間だけ現在有効な下位レイヤーへフォールバックし、
古い local / conceal cache を再表示しない。保存・比較・クリップボードの pixel job はこの
表示用フォールバックを完成結果として扱わず、必要な edit materialization の完了を待つ。
360 度パノラマ表示も同じ考え方で、完了済みの `final_composite_cache` を 8K base
アップロード元として画素編集の有無にかかわらず優先する。final AI が未完了の間だけ
`adjustment_cache` / `ai_upscale_cache` / `fs_cache` の順へフォールバックし、AI 完了後は
`FinalCompositeKey` の hash を含む cache_key へ変えて再アップロードする。
高解像度化は原本を再デコードするため、AI / 自動補正 / ポストフィルタ / スマートシャープが
有効なら無効にする。消しゴム・隠蔽加工・補正レイヤーだけがある場合は高解像度化を許可するため、
切替後には編集が見えなくなる。この差は仕様として受け入れ、画素編集があり高解像度化が実際に
動く状態だけ、画質ステータスの下へ注意書きを表示する。
保存・比較・クリップボードのようなピクセル出力経路も、`prepare_capture_pixel_job` で
同じ最終 composite pixels を取得する。EXIF Orientation は decode 済みなのでここでは
再適用しないが、`rotation_db` の非破壊 90 度回転は GPU 描画専用のため、crop 後の
ピクセル出力段で焼き込む。補正レイヤーが有効だが `local_adjust_cache` がまだ無い場合、
古い結果や下位画像は保存せず、完了後の再実行を促す。

`FsOriginalPreviewHold` の元画像プレビューは例外的な一時表示で、派生キャッシュは作り直さない。
通常の画像 / ZIP 内画像 / PDF ページだけを対象にし、動画には適用しない。表示元は常に
`fs_cache` の生デコード結果で、補正 / AI / 消しゴム / 隠蔽の派生キャッシュは参照しない。
ナビゲーションシーケンスが `blocks_new_target()` の間は画面を所有するのが前の表示単位の
holdover で、現ページ自体が出ていないため、元画像ホールドを適用しない。また、ページ送り入力が
物理的にホールドされている間も、シーケンス間の隙間を含めて移動中として元画像ホールドを
適用しない。判定は元画像表示の割り当て chord との一致ではなく、typed sequence と blocker から
独立した typed page-turn physical level を使い、時間窓は使わない。
補正レイヤーの派生キャッシュも同様に参照しない。ただし補正レイヤーモード中の
`Ctrl+Shift` は「選択レイヤーをバイパスし他レイヤーを全て適用したプレビュー」に割り当てるため、
元画像プレビューはこの組み合わせを捕まえず、`resolve_fs_processed_texture` の
local_adjust 分岐に処理を譲る。元画像表示の割り当て単体は従来どおり元画像プレビューになる。
元画像プレビューの譲渡判定と layer bypass preview の modifier gate は、fullscreen viewport
外側の main `ctx.input` では modifier が取れないため、元画像表示と同じく OS キー状態を
参照する。

### 2.3.1 デバッグ出力

フルスクリーン表示中に `Ctrl+Alt+Shift+D` を押すと、現在表示中ページ (見開き時は左右
ページ) のパイプライン段階を `%APPDATA%\mimageviewer\debug-pipeline\...` へ出力する。
PNG エンコードとファイル I/O は `pipeline-debug-export` worker で行い、`manifest.json`
に `input_generation` / `erase_mask_generation` / `local_adjust_generation` /
`conceal_mask_generation`、各 stage の
有無、欠落理由、出力ファイル名を記録する。消しゴム・隠蔽加工モード中でも、通常
ショートカットより先にこのキーだけを処理する。

### 2.4 変換の合成順序

各ページの実表示幾何の正本は
[`DisplayedImageTransform`](../src/displayed_image_transform.rs) である。単ページでは
`draw_fs_image` が、Z ズームでは `ResolvedZTransform` が `DisplayedImageTransform::resolve`
を 1 回だけ呼び、次の入力を同じ transform に確定する:

```
1. 上記の優先順位で解決済みのページテクスチャと source / texture size
2. 表示トリムの実効 content bbox
3. 回転 (rotation_db, 0/90/180/270) と一時的なフリー回転
4. viewport 固有の `pixels_per_point`、型付き fit descriptor、自動フィット倍率制限
5. Zoom / Pan、または Z ズームの確定済み placement
6. 物理ピクセル境界へ整列済みの paint rect / hit rect / UV / source↔screen 写像 / total scale
```

### 部分矩形 (content bbox) の座標系 — 2026-08-25 に分離

`content_bbox` は**元画像空間**の正規化矩形である。作っているのは
`margin_fit::detect_content_bbox` で、**復号した画素をそのまま走査する**ので回転を知らない。
一方 `full_image_rect` と `display_size` は回転後の寸法なので、**用途で座標系が分かれる**:

| 用途 | 座標系 |
| --- | --- |
| UV (テクスチャのどこを読むか) | **元画像空間** — `content_bbox` をそのまま |
| paint rect の切り出し、fit 倍率、`content_center` | **表示空間** — `rotate_bbox_to_display` で写す |

写像は screen ↔ source と同じ `forward_uv` / `inverse_uv` を使う (別の式を書かない)。

2026-08-25 まで `effective_bbox` は**回転していれば矩形を丸ごと捨てていた**。同じ矩形を
fit では表示空間、UV では元画像空間として二重に扱っており、回転すると両立しなくなるための
辻褄合わせだった。結果として表示トリムは回転したページで無効だった。空間を分けたので、
保存回転 (0/90/180/270) では矩形が残る。

**自由回転 (`fs_free_rotation`) 中は今も降ろす。** 傾いた矩形の外接が広がる分の拡大量を
解けないため。自由回転は保存しない一時値なので、やめれば次の解決で戻る。

`Original` (100% 原寸) と「拡大しない」「縮小しない」の 100% 基準は論理 1.0 倍ではなく
`1.0 / pixels_per_point` とする。`pixels_per_point` は描画対象 viewport の `egui::Context`
から取得し、main と DPI が異なり得る detached / frozen viewport では main の値を流用しない。
実効物理倍率 `total_scale * pixels_per_point` が整数に十分近い場合だけ、最終描画矩形の
**位置**を `round(position * pixels_per_point) / pixels_per_point` へ整列する。矩形サイズは
倍率とアスペクト比を保つため丸めない。見開きは物理 px 単位で配置し、ページ間隔も
`round(gap * pixels_per_point) / pixels_per_point` へ量子化する。連結読みは各ページ位置の
累積ごとに同じ量子化を行い、長いページ列でも端数誤差を持ち越さない。
見開きは `Original` (100% 原寸) のときだけ各ページの固有寸法を保ち、それ以外の
フィット方式では倍率にかかわらず高いページへ高さを合わせる。高さ合わせ中の原点整列は、
ページごとの実効倍率 `total_scale * height_match_scale` をこの判定に使う。

fit / trim / rotation / zoom-pan を overlay や入力処理が再計算してはならない。
`draw_fs_image` は解決した transform 自身で描画し、その同じ値を次の consumer へ渡す:

- 消しゴム、隠蔽加工、補正レイヤー、テキスト注釈、crop の overlay と pointer 座標変換
- ルーペのカーソル位置から source pixel への逆変換
- 範囲キャプチャの対象ページ判定と source rect 変換
- フレーム内のページ hit-test 用 `FullscreenPageLayout`

Z ズームも別の screen/source 計算を持たず、`ResolvedZTransform` が cover / contain と
pan を決めた後の `DisplayedImageTransform` を描画と上記 consumer が共有する。このため
ルーペはズーム前の矩形ではなく、実際に描いた Z transform を逆変換に使う。

`FullscreenPageLayout` は毎フレーム `Single` / `Spread` / `Continuous` の種別で初期化し、
**実際に描けたページ**を paint 順に `DisplayedImageTransform` ごと登録する。
`hit_test(pos)` は 3 モード共通でページと transform を返す。ページ間 gap はどのページにも
属さず、gap が 0 の共有境界は先に描いたページへ決定論的に割り当てる。ルーペはここで得た
page idx の processed texture をページ単位で解決し、範囲キャプチャも同じ hit-test を使う。
見開き / 連結読みのページ配置計算は引き続き `ui_fullscreen.rs` が担当する。ただし配置側が
確定した page rect と、最終的に選ばれた texture の比率は progressive load / PDF 再 raster /
加工済み texture の差し替え中に異なり得るため、live paint は
`resolve_fs_transform_in_layout_rect` で両者を同時に束縛してから共通レイアウトへ登録する。
page rect は包含枠として保持し、実 texture はその内側へ contain する。これにより source/layout
比と texture 比が異なる frame でも `scale_x == scale_y` を保ち、矩形をそのまま別比率の raster
へ渡して横または縦だけを伸縮させない。

フルスクリーンのナビゲータも、このフレームで描画済みの `FullscreenPageLayout` を正本にする。
単ページ / 見開きの各 `DisplayedImageTransform` を同じ比率で縮小した座標空間へ写し、
現在範囲は `visible_source_uv_rect(clip_rect)` の UV をその縮小 transform へ戻して描く。
画像全体が見えている場合もナビゲータは隠さない。表示範囲枠は全体枠に置き、実表示がフィット基準
より小さいときは縮小画像だけを内側へ連続的に縮める（下限 40%）。フィット丁度では画像 = 枠 =
全体となり、ズームアウトをまたいで大きさが飛ばない。
ナビゲータ上の位置は `screen_to_source_normalized` で UV へ変換し、
`pan_to_center_source_normalized` で倍率を変えずに画面中央へ置く `fs_pan` を求める。
このためフィット、表示トリム、90度回転、見開きの配置をナビゲータ側で再計算しない。
通常の縮小画像は、そのフレームの単ページ / 見開き描画が解決済みの
`FullscreenPaintResource::source_texture()` を使う。これは加工済みの full-image texture であり、
ズーム中に可視 source 領域だけを持つ Lanczos 拡大出力 (`paint_texture_id()`) は使わない。
加工済み composite が未準備のページだけは既存のサムネイルカタログへフォールバックし、黒や
点滅を挟まない。nav / final-effect source-reload holdover を重ねたフレームは、ナビゲータへ渡す idx → resource も
その display unit 全体へ置き換え、本文とナビゲータが新旧ページで食い違わないようにする。
ナビゲータ自身は texture producer を呼ばず、UI スレッドで画像読み込みやデコードも行わない。
D2の演出由来payloadだけはsourceを所有しない。全景coverageを既存texel許容込みで証明できるNativeは
出力を直接使い、本物のpartialでは同頁の通常cacheのsaved source IDを一時借用して全画像を描く。
旧rawより新finalが優先されても、同じIDが在住すれば借用できる。source不在時は既存の許可thumbnailへ戻る。
演出対象外のON/SNS splitやOFFは通常sourceを使う。本文のpartial ROIだけを全景として引き伸ばさない。
比較表示では本文と同じ単一比較キャンバスをレイアウト正本とし、Wipe / Diff はナビゲータの
画像矩形へ `zoom_pan=None` で同じ `CompareShaderCallback` を再描画する。PinnedNormal は
準備済み pinned texture を直接描画する。同一フレームの2 callback は同じ `pair.key` と寸法を
渡すため GPU texture / mip chainを再利用する。一方、`egui-wgpu`は全callbackのprepare後にpaint
するため、callbackごとに異なるuniformとbind groupは`CompareShaderSlot::{Main, Navigator}`ごとに
保持し、後のprepareが先の描画状態を上書きしない。
本文の primary 描画は `CompareFramePrimaryDraw` が排他的に所有する。現在ページに一致する
`ComparePreparationState::Ready` があるフレームは準備済み比較だけを描き、`Unprepared` /
`WaitingForSource` / `Preparing` / `Draining` と別ページの `Ready` は通常ページだけを描く。準備中に raw pinned
texture や Wipe の切れ端を通常ページへ重ねず、準備済み比較を描いた後も nav / source-reload holdover を
重ねない。
比較を準備・描画できるのは `SpreadMode::Single` の単ページ表示だけとする。X / C / Shift+C / Alt+C
の全入口は見開き表示モードで案内toastを表示して状態を変更しない。表紙が1枚だけ見える瞬間も
layout結果ではなく表示モードで拒否する。ページ移動や表示モード変更で比較中に見開き表示モードへ
入った場合は、フレーム先頭で比較を終了してから通常の見開きを描く。
見開き側の primary 分岐には比較準備・比較描画を置かない。
比較キャンバスは current 側の縦横比を正本とし、その比率を維持したまま pinned 側の入力寸法も
不必要な縮小なしで収まる解像度まで引き上げる。縦横比が違う pinned 側は等比で中央配置する。
このとき pinned の範囲外はフルスクリーン viewport の既定背景と同じ不透明な黒で
埋め、WGPU の alpha blend と CPU fallback のどちらでも下層の current を透過させない。Diff は
この黒背景と current の画素差を表示するため、current が黒でない余白は「差あり」として見える。
単ページ画像は decode 時点で `MAX_TEXTURE_DIM = 8192` 以下だが、比較workerも完成キャンバスが上限を
超えた場合は全体を Lanczos で等比縮小する backstop を持つ。上限内は寸法も画素も変更しない。
独自 WGPU upload も同じ判定を texture 作成の直前に通し、別入口が増えても上限超過の texture を
作らない。
比較準備workerは同時に1本だけ所有し、失効した途中cancel不能workerは`Draining`で
完了を回収してから次要求を開始する。workerは結果送信直後にeguiへrepaintを要求し、100ms tickを
待たずに完了を回収する。PinnedNormalはcurrentの完成寸法だけをworker開始前に使い、不要なcurrent RGBAを
生成しない。描画方式ごとのtyped CPU payloadにより、WindowsのWipe / Diffは
pinned/current RGBA 2枚だけを保持し、Diff用CPU画像 / managed textureはCPU fallbackのDiff時だけ作る。
本文の比較 callback は、ズーム / パン後の実画像矩形と viewport の交差だけを callback rect にし、
切り落とした範囲を uniform の UV 窓で元の合成画像座標へ戻す。テクスチャ採取と Wipe 境界判定は
復元後の座標を共有するため、白線の基準である実画像矩形と一致する。ナビゲータは実画像矩形が
パネル内に収まり UV 窓が常に全域となるので、このクリップによる表示変更はない。
Wipe は `CompareViewMode::Wipe` の中に fraction と `CompareWipeInteraction` を一体で所有する。
interaction は `Ready / Dragging` のどちらかで、セッション開始時は `Ready` とする。準備済み比較を
描画できる main / navigator の CPU / GPU 4経路は、pointer hoverやdrag状態にかかわらず明暗二重
stroke の境界線と中央handleを常時表示する。focused viewportで既存のCtrl非表示入力が押されている
間だけ、`Ready / Dragging`の双方で境界とhandleを隠し、解除したframeで再表示する。drag reducerは
実合成境界と同じrectのpress / releaseだけを所有し、描画はinteractionを変更しない。grab band、
境界fractionの`0.0..=1.0`、CPUのclipとGPU uniform、画像合成は変更しない。比較準備の失効は
`Dragging`だけを`Ready`へ戻す。

比較は現行どおり App 全体で同時に一つの foreground セッションである。detached viewer を park
すると比較を終了し、次に foreground で Shift+C を押したときは新しい案内セッションを開始する。
passive detached snapshot や入力を持たない一時 context mount へ比較状態を複製しない。
通常の単ページ / 見開きで利用者入力から `fs_pan` を更新するときは、直前の
`FullscreenPageLayout` に記録された実表示矩形を使い、少なくとも 1 ページが viewport と
各軸 48 logical point（ページまたは viewport がそれより小さい軸では、その小さい方の全幅）
以上重なる範囲へ制約する。48 point はポインタ / タッチで再びつかめる標準的な操作領域を
残すための値であり、制約範囲内の pan は変更しない。これにより Space ドラッグ、ゲームパッド、
ホイールの pivot 補正、ナビゲータの移動 / 範囲拡大のどの入口でも、画像が完全に画面外へ出ない。
ナビゲータの `visible_source_uv_rect` が得られない場合の自動非表示は安全機構として扱わない。
部分拡大ズーム（Z）は既存の専用制約を使い、連結読みのスクロールもこの制約の対象外とする。

ナビゲータが実際に表示されている間は、そのパネル矩形を配置された側の左右端と上下端まで
それぞれ広げた領域を、同じ側のサイドパネルと上下ホバーバーの受動的な表示判定から除外する。
反対側の画面端は除外せず、ナビゲータが実効非表示なら除外領域を作らない。
360度パノラマでは `FullscreenPageLayout` を使わず、equirect の元画像をレターボックス表示する。
現在範囲は viewport の NDC 外周を `panorama::ndc_to_equirect_uv` へ通した折れ線とし、
`aspect = viewport_w / viewport_h` と `tan_half = tan(fov_y / 2)` は `panorama_wgpu` と同じ導出を使う。
yaw の継ぎ目では隣接点の U 差が 0.5 を超える線分を分割する。クリック / ドラッグは
`PanoramaState` の yaw / pitch / fov_y だけを更新し、GPU 描画経路には介入しない。
平面・パノラマとも、利用者が固定表示またはホールド表示を要求している間は全体可視でも表示する。
`FullscreenPageLayoutKind::Continuous` は全体図が極端に細長くなるため引き続き対象外とする。

### 2.4.1 フルスクリーン静止画の GPU Lanczos3 縮小・標準拡大と選択式拡大

通常静止画は、上記の表示優先順位と
`edit -> color -> final AI -> smart sharpen -> post_filter` の合成をすべて解決した**後**に、
最終 paint 専用の `FullscreenPaintResource` へ包む。この段階は source cache や合成結果を
置き換えず、ピクセル内容の優先順位・合成順序にも介入しない。

`FullscreenPaintResource` は `Direct / Resampleable / Lanczos` の typed state で、全 variant が
元の `TextureHandle` を保持する。レイアウト、trim UV、回転、hit-test、pixel grid は元ハンドルの
`size_vec2()` から従来どおり `DisplayedImageTransform` が解決する。縮小時、標準拡大時、
シャープ拡大時、アニメ塗り拡大時、ドット絵拡大時だけ GPU resampler の native texture を
別所有する (C-1)。`Lanczos` という resource variant 名はこの共有出力の既存所有境界を示し、
実際の shader は `scale_branch` で Lanczos3 / NIS / Anime4K / pixel-AA を区別する。縮小出力は従来どおり `TextureId` だけを
差し替え、拡大出力はその texture が表す元画像 UV も typed resource に保持して同じ
`DisplayedImageTransform` 上の対応位置へ描く。level 0 の `Rgba8Unorm` source を vertical
`Rgba16Float`、horizontal `Rgba8Unorm` の 2 pass で直接リサンプルする。box mip 前縮小、
倍率閾値、ヒステリシスは持たない。Lanczos4 は採用しない。
画像補正パネルの「縮小時のなめらかさ」は 0〜100%・10% 刻みで、
`blur = 1.0 + percent × 0.003`（1.00〜1.30）へ変換する。各軸の
`filter_stretch = max(1, 1 / scale) × blur` とし、支持幅・重み・CPU 側の推定 fetch 数を
同じ値から導出する。既定の 0% は blur 1.00 である。

分岐は `total_scale * pixels_per_point` と
`physical_scale_is_near_integer` を共通基準にする。

- 1.0 未満: CPU 参照と同じ `floor(source × physical_scale)` の目標寸法へ Lanczos3。
- 1.0: 元 `TextureId` を直接 paint し、段階1・2のドットバイドットを維持。
- 1.0 超かつ `PostFilter::None`: 表示 trim と画面に映っている範囲の積だけを、表示解像度の
  Lanczos3 texture へ生成する。整数倍も含む。拡大では blur 1.00 固定とし、カーネルを
  1.0 より広げない。パン先読み領域は持たず、可視範囲が変われば再生成する。
- 1.0 超かつ `PostFilter::Nearest`: 元 `TextureId` を直接 paint し、NEAREST を維持。
- 1.0 超かつ `PostFilter::UpscaleSharp`: `PostFilter::None` と同じ可視 source 領域と目標寸法を
  NVIDIA Image Scaling で生成する。公式 SDK の 64 位相係数と 6×6 support を 1 pass fragment
  shader へ移植し、倍率上限は持たない。alpha は bilinear のまま USM の対象外とし、
  premultiplied RGB を alpha 以下に保つ。
- 1.0 超かつ `PostFilter::UpscaleAnime`: 同じ可視 source 領域と目標寸法を、17 枚の
  `RGBA16Float` 中間 texture を使う x2 VL 多段処理で生成する。alpha は bilinear のまま保ち、
  premultiplied RGB を alpha 以下に保つ。可視 source 領域の長辺が設定上限（2048px /
  4096px / 制限なし、既定 4096px）を超える場合は、中間 texture を確保する前に同じ領域の
  `UpscaleLanczos` へ切り替える。上限ちょうどは処理対象とする。
- 1.0 超かつ `PostFilter::UpscalePixelArt`: 同じ可視 source 領域と目標寸法を、軸ごとの
  面積被覆重みを使う分離可能 2 タップ補間で生成する。テクセル内は平坦なまま、境界だけを
  常に出力 1px 幅で混合する。この経路だけは premultiplied alpha を外して sRGB からリニアへ
  変換して混合し、sRGB へ戻してから再度 premultiply する。高コントラストなドット絵の白線と
  黒線の太さを保つためであり、新規経路だけに限定することで既存経路の見え方は変えない。
- 1.0 超かつ CRT / レトロ / セピア等の効果 post-filter: 元 `TextureId` を直接 paint し、
  従来の LINEAR を維持。拡大方式と効果は独立選択にしない。

貼り先とリサンプラの出力テクセル数は**同じ所有者が対で決める**。`full_image_rect` は倍率で
分岐せず必ず物理ピクセルへ寄せ (原点・辺の両方)、表示 trim と可視領域はその格子へ量子化する。
可視領域の出力テクセル数は `DisplayedImageTransform::visible_region_request` が貼り先と同時に
返し、`gpu_lanczos` 側では倍率から計算し直さない。倍率はスカラなので、全体を W 物理 px へ
寄せた端数を部分矩形へ配分できず、再計算すると最大 1px 食い違って二度目の Linear が掛かる
(backlog §1.0e / §1.161)。transform を通らない keep-alive backstop も、自分が組んだ矩形から
テクセル数を出して同じ対を渡す。

拡大出力は一辺 8192px、総画素 4096×4096 相当までとし、いずれかを超える場合は
resampler texture を生成せず従来の LINEAR 表示へ戻す。フォールバックは同じ source generation
と branch につき一度 `gpu/lanczos_upscale_limit_fallback` へ記録し、4 種の拡大は
`scale_branch` フィールドで区別する。

単ページ、見開きの各ページ、縦 / 横連結読み、page transition、nav / source-reload holdover、
detached single / spread / continuous frozen snapshot と keep-alive backstop は同じ typed
resource を通る。見開きは高さ合わせ係数を含むページ別実効倍率を使う。ルーペは鮮明な元画素が
必要なので `resolve_fs_processed_texture -> TextureHandle` の元経路、pixel grid は元論理寸法の
まま。比較表示 (wipe/diff) と 360 度パノラマは既存 callback ownership、thumbnail、animated、
動画、mask、checker、UI preview は direct 経路のままである。

GPU resampler cache は viewer context ごとに所有し、key は Page / SimilarPreview の source identity、元 `TextureId`、
`items_generation`、ページ別 `input_generation`、目標寸法、正規化済み smoothing percent、
拡大 / 縮小 branch から成る。拡大 entry だけは可視 source UV も key に含め、縮小 entry は
従来どおり full source 固定である。設定値が変わると context 内の Lanczos 出力 cache を消去し、
typed resource に保持した旧 percent と一致しない出力も再利用しない。source ごとの直近 2 寸法
(拡大 / 縮小 branch 別)、context 全体 64 entry の LRU とし、
fullscreen close / invalidation / context park・swap / 連結読み keep-set に追従する。native
`TextureId` は cache、holdover、snapshot、実 paint batch が共有する `Arc` lease の最終 drop で free する。
UI closureの終了は実renderより前なので、frame-local変数だけではnative登録の寿命を保証できない。
Lanczos resourceをpaintした同painter/clipへ、出力Arcを不変fieldに持つ有効なno-op egui_wgpu callbackを添える。
callbackのrectはZEROで、画像のmeshとともにtessellate/paint outputへ保持される。通常/候補/holdover/
backstop/frozenの全typed paint入口が共通helperを使う。callback内ではArcを破棄せず、callback_resourcesへ
保存しない（renderer lockの再取得と循環所有を避ける）。surface不在等で描画をしないoutputも、その破棄で終端する。
新しい任意frame遅延やvendorの解放queueは追加しない。ZEROでもmesh batchingの分割コストは残る。
出力は `LanczosOutputs` として実寸・mip なしで VRAM 会計する。拡大 cache だけは context ごとに
総画素 4096×4096 相当の 2 枚分を追加上限として古い entry を落とし、縮小 cache の保持規則は変えない。
perf log の
`gpu/lanczos_regenerate` に source / target、smoothing percent / blur、推定 fetch 数、
encode + submit CPU 時間、累積再生成回数、拡大 / 縮小の別を記録する。Lanczos3 / NIS /
Anime4K / pixel-AA は event 名を共用し、`scale_branch`
(`upscale` / `upscale_nis` / `upscale_anime` / `upscale_pixel_art`)
で区別する。拡大時はさらに、
生成元となった source pixel 領域の x / y / width / height を記録する。

静止画の最終フィット矩形は `fullscreen_media_rect_with_geometry` が所有する。
active CentralPanelの `ui.max_rect()` から一度解決した `StillSeekGeometry` の
下部予約量・有効gap・上部予約量を使い、上下を同時に除外した同一矩形を、
単ページ・見開き・連結読み・入力座標へ渡す。単独入口の `fullscreen_media_rect` は同じ解決を行うwrapperである。
極小の外枠では `min(1, viewport_height)` の画像領域を保ち、controlsを先にfitしてから任意gapへ
残りを配分する。上部固定バー自身の描画を高さ44pt未満へ再設計する変更は含めない。
固定領域の予約は各バーの描画可否と同じ述語を使う。特に編集／注釈・範囲キャプチャ・音楽ビューで
上部バーを抑止するときは `TOP_BAR_HEIGHT` も予約せず、非表示バー由来の黒帯を残さない。
上端の原画プレビュー・スライドショー進捗インジケータと、下端の比較ピンインジケータも
この同じ矩形を配置基準にし、固定バーの高さを個別の定数補正として重ねない。
透過背景 (Shift+B) の変更通知は共通の feedback toast だけが担い、専用インジケータを持たない。
同じ文言・同じ表示時間 (`FEEDBACK_TOAST_DURATION`) の表示を 2 系統に分けると、上部バー固定時に
右上で重なるため。持続表示が要る原画プレビューと違い、モード変更の一時通知はトーストの役目である。
上部情報バーの見開き2ページ情報は表示済み `fs_cache` / `ThumbnailState` / `image_metas` だけから
構築し、HUD 描画のために同期ファイル I/O やアーカイブ読み込みを追加しない。AI 処理名と
処理後解像度も current page の一時状態ではなくページ別の実効補正値・cache から解決し、
単ページと見開き左右の各行へ同じ規則で表示する。バーが固定・ホバー・popup のいずれでも表示されない
フレームでは、この文字列と AI ラベル自体を構築しない。固定暗色面に載る右下ページ番号と
シークバーのページラベルはテーマや文字コントラスト設定に左右されない専用の明色を使う。
混在メディア要約など通常 HUD の文字は暗色 UI のセマンティック文字色へ追従する。

静止画の上部バーは、通常閲覧中のページ解決 (`SpreadPair::Single` / `Double`) や
`ReadingFlow` の違いだけでは `draw_bar_button` の個数と x 座標を変えない。分析入口は右上の
「その他の機能」パネルへ移し、`…` は `×` の直後となる右から 2 番目に置く。360 度入口は
常に同じ slot を確保して、見開き 2 ページ表示、
連結読み、非 360 画像では disabled 描画と理由 tooltip にする。実描画を 4 組
（見開き単ページ / 見開き 2 ページ × ページ単位 / 連結読み）で走らせ、登録済み button rect の
個数と全 x 座標が一致することを回帰テストで固定する。

「その他の機能」パネルは `FsOverflowPanelState` が閉 / root / ナビゲータ位置 / 見開きの調整 / スライドショー / 音名の色を排他的に所有し、
上部バーの下へ 12pt、画面右端から 12pt 離して配置する。項目は
`FullscreenOverflowItemId::fixed_order()` と同じく、見開きの調整 → ナビゲータ ON / OFF →
ナビゲータ位置 → ページめくりアニメーション → スライドショー → 分析ツール →
ピクセルグリッド → ルーペ固定の固定順にする。行高は 48pt、
ヘッダーと閉じる hit target は 44pt とし、短い viewport では本文だけをスクロールする。同じ純粋な
panel rect resolver を描画と入力が共有する。入力は現在の矩形を egui の layer hit-test に
通し、押下位置で最前面が overflow の Area となる領域だけを対象にする。
同じ pass の領域を左・右ボタンの開始判定と `touch_excluded` が共有する。
シーク設定・回転等の popup が手前なら、そのメニューが入力列を所有する。
メニュー自体の描画・Response による項目選択は省略しない。
初回 / sizing で Area がまだ hit-test に現れない部分は、現在の矩形と既存の hit 情報で補う。
既存の最前面が Foreground 以上なら補わず、Area の登録・順序変更はしない。
閉じた状態では矩形を返さないので、前 pass の Area が残っても新しい押下を所有しない。
パネル内の wheel はパネルが受け、パネル外の wheel は背面へ流れる。スライドショー設定中も
外側の wheel でページ送り・ズームでき、パネルは維持する。外側の左クリック / タップは
パネルを閉じるだけで消費し、その入力でページ送りやバー表示切替をしない。
スワイプ・ドラッグ等の連続操作は既存の扱いを維持する。最前面が overflow パネルの位置で始まった左・右・touch 入力は
離すまでパネルが所有し、矩形外へ出てもナビゲータを含む背面へ渡さない。
右クリック終了・リング・ジェスチャにも渡さない。overflow の外側で始まった入力は従来どおり扱う。
overflow 単独では中ボタンを所有せず、従来の短クリック割り当てと drag zoom を維持する。
中ボタンの前面所有は、後述するメニューが提示されているときだけ開始する。
所有は、root の早期描画が閉じる / 行選択で状態を変える前に `capture_fs_front_pointer_input` で一度確定する。
`FsFrontPointerOwnership` に物理列を保持し、`FsFrontPointerFrameInput` を全背面 consumer に配る。
これはパネルと同じく App 全体の前面 viewer の一時状態であり、context bundle には移さない。
同じ viewer 内のページ移動 (`open_fullscreen`) では終わらず、物理 release、最後の touch End / Cancel、
所有 viewport の focus loss、実際の表示先移譲 / close で終わる。
F11 / F12、native presentation の確定などで表示先が変わる境界と、active viewer を passive へ
移譲する境界で所有を終了する。fullscreen の close、新規 detached viewport / passive viewport 採用、
`reset_detached_pause_foreground_modes` でも既存の前面リセットと同時に解放する。
他 viewport の描画や focus loss は所有を変更せず、同じ表示先の再確認でも終了させない。
focus loss で物理所有を終了しても、その pass の確定イベント列は破棄しない。
release が focus loss より前なら範囲コピーを確定し、逆順なら取消する。最終 focus level とイベント順序を区別する。
同じ pass-local 所有結果を、ナビゲータの予約 / 直接処理に渡す raw 入力の projection と
secondary action が共有する。所有結果には pass 最後の down / press と旧列の release mask に加え、
元イベント順に対応する所有情報を保持する。ナビゲータ / 範囲キャプチャの raw event projection は
この情報で所有したボタンedgeを除き、位置・focus・wheel・非所有ボタンは残す。範囲キャプチャは所有moveも除く。
背面ナビゲータと重なる場所でも所有は変わらず、
外側開始のパンや範囲選択、外側 wheel を止めない。終端フレームの primary 抑止は
後続 touch 判定の Allow で上書きしない。
最前面が overflow パネルの押下は、背面のすべての raw 入力処理が確定済みの所有結果を読む。
対象はナビゲータ / 範囲ズーム、比較ワイプ、通常パン / 回転 / 連結読み、リング / ジェスチャ、
分析のパン / ガイド / 色固定、ブックマーク行、metadata の長押しプレビュー、
シークバー / サムネイル列、中ボタン短押し / drag zoom、touch コマンド、範囲キャプチャ、
編集 canvas（消しゴム・隠蔽・注釈・補正レイヤー・切り取り・SNS 分割）。キーでパネルを閉じて
編集へ入る同一 pass でも所有は維持し、ボタンごとに入力を除くので非所有ボタンの操作は残す。
旧所有列の release と新しい背面 press が同じ pass にある場合は、release mask を新しい down / press と
分けて渡す。古い終端を背面 click / drag-stop として再利用せず、新しい列の開始を維持する。
同じ pass 内で owned press → move → release → 新 press が完結しても、最後の down 状態から
前半の所有を再推定しない。メニューの提示条件は pass 入口で一度確定するため、その pass 内の
後続 press もメニューに属する。overflow 単独では後続 press の位置により新しい所有を判定する。
画像パイプラインより前の隠蔽プレビュー hold だけは、前 pass で確定した同じ前面所有を
継続参照する。新しい矩形判定や viewport 経路は作らない。範囲コピーの排他条件、Response が
所有する入力、hover 描画・保存終了判定等の区別は [raw 入力監査表](section396-overflow-input-audit.md) を参照。
パネル表示中はカーソル自動非表示の clean 判定を false にする。モーダルダイアログ扱いにはせず、
フォーカス中の設定部品へはキーを渡し、それ以外のキーはパネルを閉じて通常の操作へ進む。
touch recognizer 自体は terminal frame まで駆動して ownership を残さない。状態表示や
実効 shortcut label は既存 state / keymap の読み取りだけで組み立て、UI 描画のための同期 I/O は行わない。

§1.403 の前面メニュー F / S / R / P / Q / fullscreen context menu と、その viewport の
egui memory popup は、提示中に始まった押下を位置によらず終端まで所有する。
提示条件は上部バー / メニュー内容の描画条件と共有し、非提示の開 bool だけでは背面を止めない。
mIV のメニューは各描画境界で外側 press により閉じ、起動ボタンと開いた子メニューを内側に含める。
パネル内 ComboBox・色 picker・context menu は egui の click 基準を維持する。個別 ID の close は
対象 viewport のみで行い、`Popup::close_all` は使わない。native context menu は OS menu loop が所有する。
背面の double-click は前面所有の境界以降に背面が受理した click の対と egui の click count の両方を要求する。
通常 canvas の変形リセットと flat / panorama ナビゲータの中央移動は同じ境界を使う。
メニューを閉じた click と背面の最初の click を対にせず、続く背面の二つの click で成立させる。
独自タイマーは持たない。共通 modal Window、VST manager、gamepad picker、上部バー / 360 品質ボタンの余白、
AI status / toast の入力関係はこの契約に含めない。詳細は [§1.403 設計・実装記録](section403-front-popup-input-investigation.md)。

スライドショーの root 行には現在のページ送り間隔を表示する。
サブパネルは環境設定と同じページ送り間隔、連結読みの待機時間・スクロール時間・スクロール量、
末尾動作を編集し、開始 / 停止ボタンを置かない。旧 `slideshow_popup_open` と設定 popup は廃止する。
root の先頭には既存の「本の構成を編集」行を置く。スライダーは操作中に設定値を更新し、
ドラッグ終了またはキーボード等による変更時に一度だけ既存の `settings.save()` で保存する。
末尾動作は選択時に保存する。ヘッダーと「戻る」は固定し、本文だけを `DragScroll::Always` で
スクロールする。フォーカス中の設定部品へキーを渡し、背面のページ送りには流さない。
HUD の再生 / 停止ボタンは 1 クリックで即時切り替え、`KeyAction::FsSlideshow`、マウス割当、
リング、ジェスチャと同じ `App::toggle_fullscreen_slideshow` へ合流する。開始対象は `Image` / `ZipImage` / `PdfPage`
だけとし、停止時の状態解放も共通化する。既存の表示完了待ち、ページめくり、連結読みの進行は維持する。

再生中に設定を変えても現在の `slideshow_next_at` は変更せず、次回の切替・待機の予定から新しい値を使う。
進捗リングは現在の設定間隔を分母にするため、変更直後から次の予定を作るまで一時的に進捗がずれる。
これは既知の制約であり、設定変更で現在の表示時間を再計算しない。
ファイル切替で閉じるのは、内容が表示中のファイル / 本に依存する `SpreadAdjustment` だけとする
（調整の基点・解除がその本に依存する）。全体設定を扱う root / ナビゲータ位置 / 音名の色 /
スライドショーは、自動送り・手動送りとも開いたままにする。判定は
`FsOverflowPanelState::closes_on_file_change` に集約する。root の行・見開き調整の status・本の構成の可否は
毎フレーム `fullscreen_overflow_panel_view` から作り直し、切替先の状態を表示する。
`close_fullscreen` と viewer の前面所有権を手放す境界では、すべての状態を閉じる。
音楽ビュー終了時の音名の色パネルの close、キー・外側クリック / タップによる close は従来どおりとする。
通常の物理フォルダへ自動で進む経路は、`try_start_slideshow_next_folder` →
`start_folder_nav(...SlideshowNext)` → `FolderNavMode::SlideshowNext` →
`close_fullscreen_for_folder_nav_reopen()` で一度 fullscreen を閉じてから開き直し・再生再開するため、
その境界で設定パネルも閉じる。ZIP 内 DFS / snapshot / collection 等は各既存経路の
close 有無に従い、フォルダをまたぐすべての経路でパネルを維持する契約にはしない。

ページシークバーの実効左右方向は `fullscreen_seek_direction` と `reading_direction` から一度だけ
決定し、ラベル配置、pointer fraction、つまみ位置、進捗塗りへ共有する。見た目だけを反転して
クリック先が逆になるような独立判定を置かない。
列を描画できるナビゲーションかどうかは、キャッシュ済み `FsSeekInfo` の
`has_page_strip_content()` が一元判定する。geometry と内容描画は同じ述語を読み、
画像・動画などの混在時は列高を予約せず、通常バーの件数サマリーだけを表示する。
この判定は初回の画像フィット計算でもキャッシュを解決して使うため、描画後の情報に依存しない。
通常バー非表示時の右下ページ番号は固定状態でも表示し、固定された通常バーが実際に表示される
フレームだけ重複を抑止する。

サムネイル列は現在ページを帯の中央へ置き、左右へ交互に 1 ページずつ広げる。各セル幅は
そのページ自身のロード済み `ThumbnailState::Loaded` texture の縦横比 × セル高で決め、
セル高の 35% を下限とする（横長側の上限は設けない）。`Pending` / `Evicted` へ当たった側は
セルを置かずに成長を止め、`Failed` は結果確定済みとしてセル高と同じ正方形を置いて先へ進む。
配置済みセルの source 範囲から両端へ `STILL_SEEK_STRIP_OVERSCAN_CELLS` だけ広げた範囲を
priority 要求へ載せるため、停止位置とその先がロードされれば次フレームに外側へ成長する。
セル矩形、画面順、要求範囲は `StillSeekStripLayout` が一度だけ解決し、描画と pointer hit test は
同じ `cells` を読む。これにより、後から外側へセルが増えても既に置いたセルの位置は変わらない。
`strip_content` は左右対称 inset の全幅を layout / request / paint の共通座標にする。通常バーを隠したときの
表示切替と列固定ボタン、および通常バー表示時の列固定ボタンはセルの上へ後登録する overlay で、専用の
空欄幅を取らない。body の press / hover / click / preview / source mapping は両 control の矩形を除外するが、
body から始まった drag は control 上を横切っても同じ gesture owner が release まで継続する。
静止画の高さは専用の `StillSeekStripHeight` と5段階の独立値で保存する。既定は
最小36 / 小48 / 中72 / 大104 / 最大144、各値は36～320のlogical point（100%表示時のpx相当）。
閲覧中は下部バー右端のフィルムボタンから、列の表示 / 非表示と5段階を同じ menu popup で
選べる。各行は環境設定に保存済みの現在値を表示し、選択は既存の Settings setter / save と
`StillSeekGeometry` へ合流するため、main / F12 と本・通常画像で同じ設定を使う。popup の開閉は
viewport ごとの egui memory が所有し、新しい App state は持たない。
高さとシーク位置プレビューの大きさの 5 段階は、動画 HUD と同じ見出し・段階表記・行寸法の 2 列に並べる。
狭い画面では共通の短縮表記と行高を使い、非常に低い画面だけスクロールする。egui の floating scrollbar
には幅を予約し、右列の文字や操作面へ重ねない。
popup 表示中は下部バーを維持し、Escape・矢印・Enter、pointer・touch は popup を優先して
背面のページ移動や fullscreen close に流さない。
動画・音声のシークストリップも専用の `SeekStripHeight` と独立した5段階値を持ち、同じ既定値と
36～320の解決時clampを使う。旧4段階の保存名と既定の大は保ち、静止画の値とは共有しない。
動画側は `NativeBarLockState` が段階と値をsource切替・main/F12間へ運ぶ。実効下部バー、strip、
top lock、固定gapは `VideoSeekGeometry` がviewport内で一度解決し、paint / hit / 映像予約 / hover /
波形要求が同じ矩形を読む。操作面を優先して残余だけをgapへ使い、上部HUDとstripを重ねない。
正の極小stripではcell・鍵・notice・範囲文字を同じbody clipへ収め、文字は実galleyに合わせて
連続縮小・省略する。0領域だけ描画と新規material要求を止め、Primary dragの所有はreleaseまで保つ。
周辺表示のdragは押下時のセル幅または波形の帯幅・時間幅を保持するため、途中の高さ変更で着地が
飛ばない。0寸法中は新規material要求だけを止め、進行中gestureのreleaseは押下時尺度で完了する。
現在のCentralPanel外枠から一度解決する `StillSeekGeometry` が、画像予約量・表示・入力・
左右パネル・zoom・holdoverの実効寸法を共有する。小さい外枠ではbar、strip、gap、ボタンを
同じ予算内へ収め、正の操作領域を寸法の下限だけで隠さない。holdoverも現在のchrome設定を使う。

列の横ドラッグ中は `StillSeekGesture::Strip` が押した時点の中央 source position、pointer の X/Y と
`drag_step_width` を保持する。各フレームの中央は、動画と共有する `center_index_after_drag` へ
押下時点からの総 x 移動量と保持した換算幅を渡し、整数 source position へ丸めて先頭 / 末尾で
clamp する。右へ引くと中央 source position は小さくなる。判定順序も動画と同じく、まず
`strip_drag_closes_downward` で下ドラッグ close を決め、close でない場合だけ中央を動かす。
静止画側の呼び出し境界は `min(strip_rect.bottom(), full_rect.bottom() - 1 / pixels_per_point)`
とし、通常バーを隠して列下端が画面下端になる場合も、最後の物理ピクセルで close できる。
共有関数の既存条件（押下原点から 24pt 超、下方向の移動量が横方向を上回る）は維持する。
release フレームも最終 pointer を使って close / 中央確定を判定し、通常バーの横シークも
`Track(Scrub)` の release で最終位置へ着地する。egui の `total_drag_delta()` は release 時に
取得できないため、close の方向はジェスチャが保持した押下原点から計算する。
横ドラッグ中はページを移動せず、release は `StripCommitted` としてその中央を保持する。セルの
click だけが従来のページ着地を行う。実際の現在ページが変わった次の描画では committed center を
破棄して新しい `current_pos` へ戻す一方、現在ページの強調は帯の中央とは独立して常に実際の
`fs_idx` を指すため、帯の表示範囲外なら強調セルは出ない。ページシーク行の上ドラッグは動画と
共有する `SeekRowGesture` を引き続き使い、通常バー本体の横ドラッグによるページシークは変えない。

`StillSeekGesture` 全体（進行中の操作と `StripCommitted` の確定中心）は
`ViewerContextBundle` に属する。App 上の field は mount 中の context の投影で、
context の交換・復帰は同じ値を退避・復元する。main grid と viewer の分割では viewer 側へ移す。
context の pause は進行中の Track / Strip だけを中断し、確定中心を残す。
fullscreen の close はその context の状態を Idle にし、retire は bundle ごと破棄する。
別窓の現在ページや close によって他の context の中心を消してはならない。

通常の左右カーソルキーによるページ移動は
`fullscreen_horizontal_cursor_direction` で、従来のページ表示方向か、このシークバー実効方向かを
選ぶ。対象はページ移動になる左右キーだけで、横連結中の左右スクロール、Shift / Ctrl+左右、
`FsPagePrev/Next`、PageUp / PageDown、画面端クリック、ホイールには適用しない。

**ZipPla 風 全画面ズームモード (<kbd>Z</kbd>、v2.0.0)** は、上記の通常ズーム/パンとは別系統だが
**解決後の `DisplayedImageTransform` と描画は共用**する (`draw_fs_zoom_mode`)。`KeyAction::FsZoomMode` (KeyHold、既定 Z、
keymap カスタマイズ可) のホールドで動き、
ズーム中は「cover 倍率 (画像が画面を覆う最小倍率) × `fs_zoom_factor`」を `zoom_pan` に変換して
ページ全体フィット指定で描く (`zip_cover_zoom_pan`)。**既定 `fs_zoom_factor = 1.0` = cover** で、
縦長画像では横幅目一杯 (ZipPla 単ページの既定) になる。ホイールは照準中 (Z 押下中) のみ倍率変更で、
ズーム確定後は前後ページ移動 (倍率変更は Ctrl+ホイール、ZipPla 準拠)。
**ズームアウト下限 = contain** (`fs_zoom_factor < 1.0` を許可): 片方の軸が画面に収まり、もう片方に
余白が出始める点まで縮小でき、それ以上 (上下左右どちらにも余白が出る状態) には縮まない
(`total = (cover × factor).max(contain)`)。`factor` の dead zone を避けるため、描画側 (`draw_fs_zoom_mode` /
`draw_fs_spread`) で画像ごとに `fs_zoom_factor` を `[contain/cover, 16]` (見開きは合成ページ基準) へ
clamp する。`adjust_fs_zoom_factor` は広めに `[0.05, 16]` で受け、描画側 clamp が最終的な下限を決める。
パンはカーソル位置を **`ZAimBasis` の `cursor_mapping_rect`** 基準で元画像範囲へ写す
(`z_cursor_image_px`)。この矩形は **実際に描画している内容の矩形 ∩ 操作帯 (`pan_band`)**。操作帯は
画面から上下のホバーバー領域 (上部バー `TOP_BAR_HOVER_Y` / 下部 `FS_SEEK_BAR_HEIGHT`) を内側へ
詰めた矩形で、**カーソルがホバー領域へ入る前に画像の上端・下端へ到達**する (実機 FB 2026-06-21)。
交差を取るのは、余白が出る向き (縦長なら左右、パノラマなら上下) で**画面端ではなく画像の端**が
写像の端になるようにするため (2026-08-21、backlog §1.107)。交差が空か 1 論理ポイント未満に
なる縮退時だけ `pan_band` へ戻す。
ズーム中は左右の補正/メタデータパネルを抑止して (`adjustment_active` / メタデータ描画を
`!fs_zoom_active` ゲート、左端ホバーの `adjustment_mode` も抑止) パン操作を邪魔しない。
照準中 (Z 押下中) は **OS カーソルを隠し**、ズーム範囲の枠 (`z_aim_frame_rect`) を重ねる。
**枠と写像は同じ `ZAimBasis` を共有する**ので、両者の基準が食い違うことはない。basis が持つのは
**実際に描画しているスケール**であり、単ページは `ResolvedZTransform::resolve` が Z 中に
`fit_mode = Page` とスケール制限リセットを強制するので **contain** (表示トリムがあればトリム後
コンテンツ)、見開きは上書きが無いので**表示モード由来の `fit_scale`** になる。カーソルを
隠すだけで写像は変えないため、枠と離した瞬間の表示範囲は一致する。
倍率・状態は settings に
保存せずセッション内のみ保持し、ページ送りをまたいで維持・グリッドへ戻ると解除。
単ページ・見開きの通常閲覧で動作 (連結 / パノラマ / 動画 / 分析モードでは無効)。**見開き** は
`draw_fs_spread` 側で合成ページを対象に `zip_spread_zoom_pan` で同様にズーム/パン
(既定 = 単ページ幅の約 1.2 倍)。**表示トリム中はトリム後合成ページ (`fit_w × fit_h`) を対象**にし、
`zip_spread_zoom_pan` の pan/clamp をトリム後座標で行うことで `content_center_offset` +
`layout_spread_page_rects` のトリム配置と座標系が一致する (= トリム後コンテンツだけを拡大、余白を
見せない)。`content_center_offset` (トリム中央寄せ) は確定ズーム中も維持する。トリムが無ければ
`fit_w = combined_w` で従来と一致。**PDF** (単ページ) は
`maybe_rerender_pdf(zoom)` でズーム倍率に応じ高解像度へ再レンダ ((idx, zoom) が変わったときだけ要求し、
in-flight キャンセルの無限ループを避ける)。**既知の制限**: PDF を見開きでズームしても再レンダせず
fit 解像度のまま (画像見開きは問題なし。PDF 見開きズームは niche のため後続)。**表示トリム** 中は
ズーム対象をトリム後 bbox に絞り (上記コンテンツ領域 `content_min`/`content_size`)、確定ズームは余白を
クリップ、照準オーバービューも `draw_fs_image` へ `content_bbox` を渡してトリム後コンテンツを表示する
(= ズームのどの段でも切り落とした余白を見せない)。`zip_aim_frame_rect` はそのコンテンツ contain 表示
矩形を基準に枠を置く。操作仕様は [keymap-spec.md](keymap-spec.md) を参照。
見開きの geometry は Z ズームの解決前にフィット方式だけから確定するため、
`zip_spread_zoom_pan` の倍率・パン解決は 1 回だけ行う。`Original` 以外は従来の高さ合わせ
composite をそのまま使う。

指定サイズfitの PDF は通常 / 手動zoom / 単ページZ / 見開きZの解決済みページごとの
full-page fit 枠から絶対需要を作る。上記の旧 `(idx, zoom)` 要求と PDF 見開きZの再レンダ制限は
既存 4 方式の契約として維持する。新方式の一方向依存と要求比較は下の指定サイズfit節を参照。

`settings.fullscreen_fit_mode` は <kbd>0</kbd> と割り当てたリング／ジェスチャ／マウスボタン／
パッドリングで、同じ `cycle_fullscreen_fit_mode` を経由して循環する。
`FullscreenFitMode::next_for_flow` は `fullscreen_fit_cycle_excluded` を除外して通常順に進む。
現在モードが対象外なら最初の有効モードへ、1つだけならその方式を設定／維持する。ホバーバーのフィットボタンは
クリックで選択メニュー (`fit_popup_open`) を開き、flow で選べるモードを一覧表示して現在モードを
青でハイライトする (見開きボタンのポップアップと同型)。メニュー項目選択は
`set_fullscreen_fit_mode_for_current` を直接呼び、<kbd>0</kbd> 循環 (`cycle_fullscreen_fit_mode`)
とは別系統。メニュー候補は `FullscreenFitMode::selectable_for_flow` が返す
(余白カットフィットを除く5モード、Page → Width → Height → Original → SpecifiedScale)。ページ単位へ切り替えたときはページ全体、
縦連結へ切り替えたときは横幅フィット、横連結へ切り替えたときは縦幅フィットに戻す。連結モード中は
保存値が旧余白カットフィットでもページ全体フィットとして扱う。

フィットメニュー下部の「拡大しない」「縮小しない」は `FullscreenFitScaleLimits` として
自動フィット倍率にだけ適用する。つまりページ全体/横幅/縦幅が求めた `fit_scale`
を物理 100% (`1.0 / pixels_per_point`) で clamp してから、ユーザー操作の `fs_zoom` を掛ける。明示的な Ctrl+ホイール、
中ボタンドラッグ、ゲームパッド等の手動ズームは制限しない。`縮小しない` が ON の場合は
ページ全体フィットでも 100% 表示で画面外へはみ出すことがあるため、`fullscreen_fit_allows_drag_pan`
も true 扱いにしてパンできるようにする。
既存 4 方式の100% 判定の基準サイズは、単ページ / 見開きでは「今実際に描くテクスチャ」のサイズであり、
AI アップスケール完了後は `final_composite_cache` の寸法で再レイアウトする。
連結読みではスクロール中の配置ジャンプを避けるため、processed テクスチャが raw と同じ
アスペクト比なら raw/source サイズをレイアウト基準として保つ。別の処理で processed 側の
アスペクト比が変わった場合だけ processed 側のサイズを使う。crop は通常表示では暗転 overlay
だけなので、レイアウト基準も描画 UV も変えない。元画像プレビューや分析モードでは
raw 表示に合わせるため、raw サイズを優先する。

旧 **余白カットフィット** (`FullscreenFitMode::MarginFit`) は設定互換入口としてだけ残している。
新規 UI / <kbd>0</kbd> 循環 / フィットメニューには出さず、本を開いたタイミングで
`fullscreen_fit_mode = Page` と表示トリム `Auto` へ移行する。表示トリムパネルの
「自動余白カット」が同じ検出結果を使う。
`cached_margin_bbox`(idx) が `margin_fit::detect_content_bbox` で
中身の bounding box (正規化座標) を検出してキャッシュ (`fs_margin_bbox_cache`、
`fs_cache` と同じタイミングでクリア) し、`draw_fs_image` が `fit_scale` を bbox サイズで
求めて中心を bbox 中心へ寄せる (= 余白分ズームイン)。描画時は bbox 外を描かず背景に落とすが、
**ソースピクセルは一切変えない**ので補正/
AI/エクスポートには無影響。

検出は「全部映る」優先の頑健化版 ([margin_fit.rs](../src/margin_fit.rs)):
(1) 検出用に長辺 ~1000px へ面積平均で縮小 (サブピクセルのゴミを潰し線は残す)、
(2) 余白色を縁ピクセルの median で推定 (四隅 1 点ではないので糊汚れ・勾配に強い)、
(3) 広めの luma 許容で色味差を無視、
(4) 中身マスクを 8 連結ラベリングし面積が小さい孤立塊 (点/ゴミ) を捨てて線・文字は残す
(枠外へ伸びる線は本文と連結するので残る)、
(5) 残った成分の union bbox にセーフティパッドを足し、各辺のトリムを `MAX_TRIM_FRAC` (20%)
までに制限 (隅の小さな中身だけに巨大ズームする "やりすぎ" を防ぐ)。
縁の過半 (`BORDER_MARGIN_FRAC`=0.50 未満) が余白でない真のフルブリード / トリム量が極小 のときは
`None` で通常フィットへフォールバック (= 迷ったら切らない)。**漫画の裁ち落とし (絵が一部の辺
だけ端まで届く。例: 左上下はブリード・右だけ余白) は、union bbox が自然に各辺へ追従するので、
余白のある辺だけ詰まる** (縁ゲートを 0.80→0.50 に緩めて、3 辺ブリードのページを丸ごと諦め
ない。縁の余白色は median 推定なので過半が余白なら緩めても誤検出しない)。
診断: 余白カットモードへ切り替えた時に `log_margin_fit_diag` が各成分の面積・位置・各辺の決定要因を
logger へ出力 (`[margin-fit diag]`)。

**見開き (`draw_fs_spread`)** でも有効: 左右各ページの content bbox をページ別に保持し、
見える幅だけを `spread_page_gap_px` に合わせて詰める (bbox 無しのページは全域扱い=切らない)。
自動余白カットでは、左右ページで上下の検出量が違うと見開き全体の基準が崩れるため、
上 / 下は左右の少ないカット幅を共通採用し、横方向 (左 / 右、中央側 / 外側) はページ別の検出を残す。
中央側をトリムした場合も、切った領域を見開き中央に残さず、左右の見える端が設定 gap で並ぶ。
`fit_scale` は左右の見える幅の合計と上下 bbox の union 高さを基準にし、描画時はページ全体
rect のうち bbox 部分だけを UV 指定して描く。トリム量によってページ全体 rect が重なり得ても、
各ページの `DisplayedImageTransform` が実際の UV / paint / hit rect を保持し、
`FullscreenPageLayout::hit_test` はその見える hit rect だけを判定する。どちらかが回転していれば
通常フィットにフォールバック。

**表示トリム** (`ui_view_trim.rs` / `view_trim.rs` / `view_trim_db.rs`) は、
漫画ビューア用途で読みながら使う表示専用トリム。左端 / 上端 / 右端ホバーで開く左パネルの
`画像補正 / 表示トリム / ブックマーク` タブから操作し、選択中タブは `Settings::fullscreen_left_panel_tab`
へ保存する。
本側の基本モードは `ViewTrimApplyMode::{None, Auto, Book}` のラジオで選ぶ。
`None` は bbox を使わず、`Auto` は表示中ページごとに `cached_margin_bbox` を使う。
ただし見開きの `Auto` は左右をペアで調停し、上下だけ少ないカット幅に揃える。
`Book` は手動設定の基底モードで、各 `idx` に enabled なページ個別行があれば `Page`、
無ければ本全体設定を適用する。したがってページを移動して戻っても、保存済みの個別行は
選択操作なしで自動適用される。連結読みと見開きは左右を含む各 `idx` を独立して解決し、
個別行のあるページと無いページが同時に描画されても Page / Book を混在できる。
`None` / `Auto` の間は保存済みページ個別行を参照しない。
UI の `適用範囲` は enabled な個別行の有無から毎フレーム導出する。
`このページ` は個別行を作成し、`本全体` は対象ページの個別行を DB から削除する。
`Auto` から `手動設定` → `このページ` へ入った場合は、現在表示中の自動検出 bbox を
ページ個別値へ流し込み、見た目を保ったまま手動調整へ移る
(`手動設定` を経由する 2 クリックになったため、`Auto` から入った事実を UI 一時状態で持ち越す)。
`Book` / `Page` では、単ページ / 見開き連動 (上・下・中央側・外側) / 見開き左右別を
0〜20% で調整する UI が展開される。スライダーを動かした対象は自動的にその手動モードで
適用される。手動指定された bbox も、自動検出 bbox も、ページ全体 / 横幅 / 縦幅 /
100% 原寸で fit 基準として使う。
bbox 外は描画せず背景色を見せるだけで、
`export_crop.rs` の切り取り、保存、Ctrl+E エクスポート、補正 / AI キャッシュには影響しない。
保存済み回転が 0° 以外のページでは `effective_bbox` / `fs_image_fit_bbox` が表示トリムの
bbox を採用せず、ページ全体を回転して表示する。表示トリムと保存済み回転は同時に成立しないため、
回転ページ用に bbox の座標系を変換しない。
見開きの「左右別」切替は値を移行し、連動→左右別では中央側/外側を左右ページへ展開、
左右別→連動では中央側/外側/上下を平均値へ畳む。`Page` での「左右別」切替は
本全体設定を書き換えず、各ページ個別行の値と表示側 semantics に反映する。ページ個別値のうち、見開き連動 UI から
作られたものは作成時の表示側を記録し、表紙設定や 1 ページずらしで左右が入れ替わっても
中央側 / 外側の意味を保って適用する。左右別 UI や Auto から固定した値は画像座標のまま保持する。
自動検出ボタンは `Book` / `Page` の手動設定へ現在ページの検出結果を流し込むための補助であり、
`Auto` とは別物。ただし検出元は `Auto` と同じ `cached_margin_bbox` に揃え、表示中の自動余白カットと
手動値へ流し込む自動検出で左右の結果がずれないようにする。見開き連動で左右を別々に調整しない場合は、
手動値へ流し込む自動検出も上下 / 中央側 / 外側の共通値に左右の少ないカット幅を採用する。
`Auto` は保存済みの手動値を作らず、ページごとに検出する。
旧 `FullscreenFitMode::MarginFit` が保存されている場合は、表示トリムの適用モード `Auto` と
ページ全体フィットへ移行して保存する。
基本適用モードと本全体設定は `view_trim.db::view_trim_books` に本キー
(`spread_container_key` と同じ粒度。ネスト ZIP は zip_path + 実効 prefix) で保存する。
ページ個別設定は `view_trim.db::view_trim_pages` に `page_path_key` で保存する。
`Page` は本側の適用モードとしては保存しない。代わりに enabled なページ個別行そのものが
そのページの永続 Page 選択を表す。`effective_view_trim_apply_mode_for_idx(idx)` が基本モードと
当該行を一箇所で解決し、`view_trim_single_bbox` / `view_trim_spread_bbox` と各 content bbox
解決が同じ規則に従う。見開きの片側だけに個別行がある状態も保持し、反対側は Book へ戻す。
この DB はリリース済みデータなのでマイグレーションや既存行の削除は行わない。従来残っていた
enabled 行も、新仕様では該当ページを表示した時点で自動適用される。
`Auto` は「モード」だけ保存し、検出 bbox は保存しない。スライダードラッグ中はメモリだけ更新し、
マウスを離したフレームで本設定と表示中ページの個別値をまとめて SQLite へ書き込む。

mIV Remote のページ描画は protocol v29 で現在の container address、画面上の
`single / spread_left / spread_right`、見開き相手 address を Page 要求へ添え、本体と同じ本キー・ページキー・
左右 semantics で `None` / `Book` / enabled な `Page` を解決する。remote 側の
`ViewTrimDb` は既存 `view_trim.db` を `SQLITE_OPEN_READ_ONLY` だけで開き、DB が無い場合も
作成・DDL・migration は行わない。表示 crop は committed な最終合成（加工用 crop を含む）の
後、端末用 resize / JPEG encode の直前だけに適用するため、補正 / AI の materialized pixels と
その cache は表示トリム前のまま保持する。`Auto` は本体と同じく補正前の raw raster を
`detect_content_bbox` へ通す。見開き request は自ページの bbox を先に小さな LRU へ公開し、
相手 bbox が未検出なら相手 request を待たず、同じ worker / cancel token で相手 raw raster だけを
復号する。これにより heavy worker が 1 本でも deadlock せず、2 本なら並行 request が互いの結果を
再利用できる。LRU key は `page_key + source mtime/size + target_px` で、補正・AI・表示トリム設定を
含めない。本体の `(load_seq, pixels_ptr)` と owner / 無効化条件は同一ではなく、remote は既存の
decode / composite cache と同じ source stamp または decode 上限が変わった時に再検出する。同一 size / mtime
を保った source 差し替えは既存 remote cache と同じ制約を持つ。端末からのモード変更は別段とし、remote は
設定を書き込まない。

保存済み回転も通常画像 / ZIP entry / PDF page の共通ページキーで解決し、保存済み加工 crop を
含む最終合成の後、端末用 resize / JPEG encode の前に 0° / 90° / 180° / 270° を適用する。
ただし回転が 0° 以外なら、上記の本体規則と同じく表示トリム plan を最初から bbox なしにし、
`Auto` の検出や見開き相手の復号も開始しない。応答寸法とブラウザのレイアウト寸法は回転後を返す。
回転は合成 cache key に含める一方、raw / Auto 検出 cache は回転前の正準 raster のまま共有する。

本体側だけで表示トリム設定を変更した直後は、端末の `PageResourceCache` と HTTP の
`private, max-age=60` が既存 JPEG を返し得る。この既存制約は Auto でも同じで、response cache hit
では IPC と bbox 検出自体が走らない。IPC まで到達した要求は最新の read-only DB 状態を読み、raw bbox
LRU は設定値を保持しないため stale 期間を延長しない。即時反映には、本体の設定 revision を remote / SPA
へ通知して page render revision を進める別の無効化経路が必要になる。

Spread モード (見開き) の場合は、`draw_fs_spread` が `resolve_spread_pair` で左右の idx と配置
(LTR/RTL/Cover) を決め、両ページを「1 枚の合成画像」とみなしてレイアウトする。

フォルダ / 仮想ビューの読み込み時は、`page_order_locked_for_items` が install 前の
`source_path` と確定済み `items`、対象ビュー種別から「この読み込みは本か」を一度だけ判定する。
`spread.db` に明示値があれば本 / 非本のどちらでもその値を優先する。未保存の場合だけ、製本フォルダ、
閲覧履歴、ZIP / CBZ / PDF / 直接閲覧・変換アーカイブ、設定 ON の画像のみフォルダには Settings の
見開き・連結方式・読み方向の既定値を使い、それ以外の通常フォルダと合成ビューには
単ページ + ページ単位 + 左→右を使う。ネスト ZIP の本キーから root ZIP キーへの fallback と、
旧 `SpreadMode::Vertical` の読み替えは従来どおりである。

`resolve_spread_pair` は先に表示ユニット列を組み、表紙、横長ページ、末尾端数を単独ユニットにして、
横長ページの次の縦長ページから通常の見開きペアリングを再開する。
2 ページのペア化対象は `GridItem::Image` / `ZipImage` / `PdfPage` だけである。
`Video` / `Audio` はナビゲーション列に含まれる場合も必ず単独表示ユニットにし、静止画との混在でもペア化しない。

組んだ `Vec<SpreadDisplayUnit>` は viewer context ごとの `SpreadDisplayUnitsCache` で再利用する。
token は `items_generation`、`spread_mode`、`spread_shift_anchor_idx()`で得るlive解決済みanchor、`landscape_epoch` と、
**nav index 列そのもの**である。nav の完全一致を使うため、同じ items 世代でも絞り込み・並べ替え・
ページ読みと連結読みの列切替を混同しない。

nav index 列自体は `ViewerNavigationCaches` が `Arc<Vec<usize>>` で保持する。動画を含む
ページ単位列、動画を除く連結読み・先読み列、seek 情報の 3 つを 1 owner にまとめ、
表示集合 / 詳細 sort / snapshot の index 空間変更では `invalidate` 1 回で同時に破棄する。
items 世代と連結方式は owner の source key でも照合する。owner は
`ViewerContextBundle` と一緒に交換し、main / detached 間で共有しない。

未知寸法は従来どおり縦長と同じ `false` に分類する。`fs_cache`、thumbnail の source / layout 寸法、
`page_dims_cache` への記録で cached nav 上の実効的な横長性が反転した場合だけ
`landscape_epoch` を進める。したがって未知→縦長では再構築せず、未知 / 縦長→横長では
同じ frame の後続参照から再構築する。訂正寸法や保存回転により横長→縦長となる場合も反転として
失効する。回転 cache の一括破棄 / 部分破棄は、次の参照で対象 nav の回転を一括再取得してから
同じ分類を照合する。cache と epoch は main / detached 間で共有しない。

前後移動も同じ表示ユニット列を使い、`FsPageNav::Target(index)` または
`FsPageNav::Boundary` として解決する。見開き末尾から次へ進む入力を raw page delta に戻さないため、
内部 index が同じ見開きの片側へ移ることはなく、最初の入力で末尾ヒントを表示する。キー、ホイール、
画面クリック、ゲームパッド、スライドショーはこの契約を共有する。
Ctrl+←/→ の「1 ページずらし」は `spread_mode` を保存し直さず、
`ViewerSpreadShift`が現在のペアリング開始位置を所有する。§1.364 D2の接続差分では、
通常一覧は`ViewerShift::Ordinary(index)`のsession調整、本は
`ViewerShift::Book(SpreadShiftState)`のPageRef・解決状態・保存状態に分ける。
描画用indexは`spread_shift_anchor_idx()`の投影であり、独立に代入するraw fieldではない。
記憶ONの適格なBookで明示ずらしを受けると、その実anchorのPageRefを専用serviceへ送る。
保存replyは元操作の状態表示を更新するだけで、その後の現在ページや別窓の組を変えない。
OFFはliveをsession扱いで保持し、新規入場で保存済み調整を使わない。明示解除はOFFでも保存値を消す。
アンカーは単純な移動先ページではなく、
移動先から前方向へ見て直近の横長ページ直後、または先頭まで戻した同じ偶奇の位置にする。
これにより、表紙ありの本で一度ずらした後に前ページへ戻っても、アンカー直前のページだけが
単独表示へ吸われず、先頭または横長ページ境界から一貫した見開き列として戻れる。
横長ページ自体は単独ユニットのまま境界として扱い、自動的な表紙扱いにはしない。

Bookのitems/nav投影は`PreparedPageIdentityIndex`とnavのArcを対で保持し、変更時にPageRefを
実ページへ解き直す。基点が欠落・filteredなら`NotInView`として保存refを保持し、描画へはNoneを渡す。
slot削除やsnapshotのindex変換はowner内のlive resolutionだけをremapする。別の同indexページへ
保存基点を置き換えない。解決済みanchorの変化は既存unit/cache失効点へ接続する。
snapshot化はdurable scopeを切り離し、その時点のnumeric pairingをOrdinary sessionとして保持する。
成功済みページ改名のmappingは同じ本のlive PageRefへpureに反映し、追加saveやmountを行わない。
root改名の永続移行はD1が所有し、開いているcontextの参照元・items・prepared identity・pending load・
addressとshift scopeは旧rootのまま保持する。mainの旧path再読込・F12の旧items保持を維持し、
live追従や自動reopenを追加しない。新rootの保存値はそこから改めて開くときに使う（設計§11.6）。
case/slash衝突のshift identityはD1と同じく受入列の先頭slotを決定的に選ぶ。
snapshotのanchor remapはitems交換で行い、一覧から固定してもfullscreen時と同じ対応付けを使う。
新規Restoreはsidecar continuation内で先行shift/resumeの完了証明を満たしてから同じ最終列へ
解決し、位置復元・unit正規化・first-displayへ進む。retained ONの再採用は受入済みidentity indexを使い、
待機をbundle内の`RetainedReadoption::Idle | Reading`に持たせる。global Commonの別context依存と
併存させ、既存target-context semantic gate/dialogで対象窓だけを止める。兄弟窓を一括modalにしない。
`ContextAsyncOwner::RetainedSpreadShift`でpoll/cancel-on-parkを登録し、parkではreceiverだけを
dropしてaccepted sessionを保つ。再採用はfresh cutoffを捕捉し、forkはreceiverと元contextのwrite受付を複製しない。
forkの保存待ち/失敗の調整はsession扱いとし、元contextだけがwrite完了を採用する。
初回位置の`FencedResume`はopen continuationの初回採用完了時に終了し、後のresume参照へ持ち越さない。
maintenanceで失効したshift readはrow不在の証明ではない。retainedのaccepted調整・projection・
commandを保ち、unit/cache失効と現在ページの再正規化を起こさない。初回Commonはcapture時の
調整なしと独立resumeを採用する。別本改名はscope stampを変えず、元のreadをそのまま採用できる。
明示ON/OFFではretained待機を取り消してaccepted sessionを保ち、表示済みsiblingsへ旧保存値を
適用しない。fresh-openは元のresume cutoffと捕捉済みopen scopeを保ったまま、新settings世代・
現在のshift受付cutoffで依存を再受付する。世代不一致を理由にresume fenceまで破棄しない。
OFFではlive調整を変更しない。retained/import採用は
`adopt_shift_layout_without_navigation`で現在の実ページを含むunitへ正規化するだけで、
resume/historyを書かない。metadata importはContextRequest workerの捕捉FIFO読取結果を
context/items世代の採用後、current scope + ON + Automaticが一致するときだけ使う。
HUDの`… → 見開きの調整`と見開きcontrol近くの印はこのownerの状態から作り、jumpは実re-pairing anchor、
Clearと全体記憶設定は同じ操作ownerへ戻す。構成等の資格は`PairingEditAvailability`を共通受付へ渡す。
RemoteのD3接続も、App serviceから登録されたread handleを通して保存PageRefを解決し、
この共通Automatic resolverへoptional shiftを渡す。PCのlive indexを流用しない。
scope/identityはresponse budgetで切り詰める前のfull本を使い、その後の応答で保存基点が
見えなくてもMissingや本の末尾とみなさない。完全応答のgroupは同じrules・列のPC unitから
anchor/画面順/slot/roleを派生する。bounded応答は既存のgroup生成・cap・ページアクセスを維持する。
切詰め境界差はshiftなしのmasterにもある既存制限であり、省略ページの全寸法走査や境界group削除で
変更しない。master証明とD3の検証結果は設計§11.10および`target/D-1364-d3-msg.txt`を参照する。
記憶OFFのopenはshiftなし、force_single_pageとorientationは表示投影だけで保存値を変えない。
Q3 SetSpreadの成功公開・group cache失効はshift Clearのcommit結果を同requestで受け取ってから行う。
新しいWeb nudge/記憶UIは追加しない。D3時点ではG3構成consumerは未統合だった。
2026-10-11のG3は共有C1–C12のadmission/restore規則を消費し、editor固有のC7/C9はG4で接続する。
本記述は接続の実装箇所を記録し、完了検証・レビュー合格を主張しない。

実際の配置は従来の次の手順を使う。

1. フィット方式を先に解決し、`Original` なら各ページの固有寸法、それ以外なら高いページへ
   揃えた geometry を選ぶ
2. 選んだ geometry と各ページの content bbox から見える合成サイズを計算する
3. `spread_page_gap_px` を左右ページの画面上の間隔として差し込み、フィットモードに応じた
   `fit_scale` とズーム後の `total_scale` を求める
4. ズーム/パンを `(fit_scale * fs_zoom, image_rect.center() + fs_pan)` として合成し、選択した
   geometry の content center から左右ページ矩形を配置する
5. ズーム/パンが有効なフレームでは `image_rect` にクリップして他の UI 領域へのはみ出しを防ぐ

ページ単位表示のカラー化遷移も、この `SpreadDisplayUnit` / `resolve_spread_pair` と同じ境界を
使う。表紙、末尾端数、横長ページは 1 ページ unit、通常見開きは画面順 2 ページ unit として
旧表示を保持する。1 ページずらしのlive解決済みanchorと LTR/RTL の画面順も同じ resolver
から得るため、遷移状態だけが別のペアを作ることはない。

見開きのページ間隔は環境設定から変更でき、既定 4px、0px で左右ページを隙間なく接続する。
実配置では対象 viewport の物理 px へ gap を丸め、物理整数倍率では DPI 125% / 150% でも
各ページ原点を物理ピクセル境界へ着地させる。
`Original` では手動ズーム後も各ページの固有寸法比を保つため、元画像の高さが異なればノドに
段差が生じる。ページ全体 / 横幅 / 縦幅フィットでは倍率にかかわらず高さ合わせを維持するため、
ズーム中に片側だけが不連続に拡大・縮小しない。
見開き中も `rotation_db` の単独ページ回転 (R/L) は左右ページそれぞれに反映する。
`fs_free_rotation` (Ctrl+ドラッグのフリー回転) は見開きに反映しないため、Ctrl+ドラッグは
`handle_fs_wheel_and_click` 側で no-op にしている。
ズーム中または横幅/縦幅/原寸フィット中のパン (非修飾ドラッグ) と Ctrl+ホイールズーム、ダブルクリックリセットのみが見開きで有効。

連結読み (`draw_fs_continuous_reading`) は、`SpreadMode` のページ構成 (単ページ / 見開き)
を表示ユニットとして縦または横へ仮想配置する。巨大キャンバスは作らず、各ユニットの表示矩形を
スクロール位置から毎フレーム計算する。描画対象はマージン 0 の厳密可視ユニットだけに保ち、
final pipeline の処理対象には前後 1 ユニットの準備帯も加える。準備帯は描画にも可視ページ数上限の
判定にも含めない。GPU に保持するのは可視範囲と前後少数ページだけにする。
同時可視ページ数は最大 16 ページ程度、`fs_cache` の連結読み用 keep set は最大 20 ページ程度に
抑え、推定総テクセル数でも追加の安全弁をかける。推定にはrawだけでなく、erase、local-adjust、
conceal、edit、final composite、comic、補正の完全mip chainをTextureId重複排除つきで含め、
同じkeep setでevictionする。連結読みのページ/見開きユニット間隔は
`continuous_reading_gap_px` (既定 20px) を使い、見開きユニット内部の左右ページ間隔は
通常見開きと同じ `spread_page_gap_px` を使う。見開き構成の縦連結 + 横幅フィットでは、
表紙・横長ページ・端数の単独ページも仮想的な 2 ページ幅で fit scale を求め、実ページを中央寄せする。
これにより表紙だけ横幅いっぱいに拡大されず、後続の見開きページと同じ倍率で読める。
unit と unit の**見える端の距離**は、倍率にかかわらず `continuous_reading_gap_px` ちょうどに
なる。そのために、場所を空ける長さは理想値ではなく**描画が実際に置く帯**
(`visible_paint_band_px`。transform の trim 量子化と同じ入口) から取り、累積位置は物理 px の
f64 で足す。1 段ごとに丸めると中心間の半画素が段ごとに違う向きへ倒れて端の距離が 1px ずれ、
points の f32 で足すと誤差が段数に比例して積もる (backlog §1.159)。各 unit の貼り先を物理 px へ
寄せる段階 (§1.161) は整数画素の平行移動と可換なので、寄せても距離は保たれる。

**等間隔をレイアウトの不動性より優先する** (2026-09-02 利用者判断)。段の長さは表示中の
テクスチャの縦横比から決まるので、サムネイル代替から本解像度・加工済みテクスチャへ差し替わる
瞬間に縦横比がわずかに変わり、以降の段が最大 1 物理 px まとめて動く。これは差し替え前から
あった端数の移動が、整数化によって「0 か 1px」へ倒れて揃って見えるようになったもので、
移動量そのものは増えていない。段の長さをテクスチャに依らない正規寸法から取れば差し替えで
一切動かなくなるが、そのぶん縦横比の違うページで見える間隔が揃わなくなる。**0px と 1px、
1px と 2px の違いは知覚しやすく、解像度が変わったときの再配置は理解されやすい**ため、
間隔の正確さを取る。この優先順位を逆にしたくなったら、まずこの段落を読むこと。

連結読みの見開きユニットも通常見開きと同じ geometry 選択を使い、`Original` では
倍率にかかわらずページ固有の高さを保って縦中央へ配置し、それ以外では常に高さ合わせを維持する。
表示トリムが有効なページは、各ページの見える bbox だけをユニット幅/高さとページ間 gap の基準にし、
描画時も同じ bbox を UV に使う。見開きユニットでは左右の見える端が `spread_page_gap_px` で並ぶ。
final composite / comic composite は厳密可視ページと準備帯を処理対象にし、キャッシュ済みの
最終テクスチャは厳密可視ページだけへ即描画する。未生成ページだけを 1 フレーム 1 枚ずつ final
pipeline に流し、現在ページがキャッシュ済みの場合は viewport 中心に近い未生成ページ、続いて
画面外の準備帯へ処理枠を回す。これによりスクロール中の大量 GPU アップロードを避けつつ、
次ユニットが可視になる前に final composite の生成を始める。連結表示自身が 16ms 後の再描画を
所有するのは、live な raw decode / upload backlog、解決済みページの次候補、または保存済み
編集マーカーを同期的に整理して状態が進んだ場合だけとする。`resolve_fs_processed_texture` の
`None` には raw 読込失敗や worker 起動失敗などの終端状態も含まれるため、`None` だけを根拠に
再描画を続けない。AI / final-effect / 各編集 worker は完了時の repaint をそれぞれの owner が
要求し、結果到着後に処理枠を再評価する。AI アップスケールのように
processed テクスチャは配置サイズへ反映せず、raw/source のアスペクト比を連結読みの
レイアウト基準として保つ。crop は通常表示では暗転 overlay だけなので、連結読みの
配置サイズや UV を変えない。
通常見開き・ページ送り・連結読みは同じ画像表示ユニット列を使うため、横長ページの前後で
単ページ扱いと一時的なずらしアンカーの扱いがずれない。
ただし source reload の遷移 texture は所有粒度を意図的に分ける。
`capture_final_effect_source_reload_holdover` は `ReadingFlow::Paged` の現在 unit 全体を保持し、
連結読みは従来どおり keep-set 内の `continuous_page_transitions[idx]` をページ別に使う。
通常の paged ページ移動は holdover ではなく移動先 rendition が所有する。スクロールで複数 unit が
同時可視になる連結読みへ単一の paged holdover を重ねないため、縦 / 横連結の描画挙動は変わらない。
横連結では `ReadingDirection` により
左→右 / 右→左の座標符号を反転する。UI で横方向を変更した場合は `SpreadMode` の
表紙あり/なしを保ったまま LTR/RTL も同期し、ページ単位の見開き方向と横連結方向が
食い違わないように保存する。ホイール 1 ノッチ、矢印キー/D-pad 1 回、左スティック最大入力の
スクロール量は、それぞれ画面幅/高さに対する割合として `continuous_reading_*_percent`
設定に保存する。PageUp/PageDown は従来どおり画面長の 85% で移動する。

連結読みの左ドラッグは、通常時は従来どおり連結方向の成分だけを
`fs_vertical_scroll` へ流す。Ctrl を押しているフレームでは軸固定を解除し、主軸成分は同じ
スクロールへ、直交成分だけは `fs_pan` (`Horizontal` は y、`Vertical` は x) へ加算する。
主軸側の `fs_pan` は変更せず、パン量の clamp も行わない。修飾状態はフレームごとの pointer
delta に適用するため、ドラッグ途中の Ctrl 押下 / 解放でも別の drag state へ移行しない。
ダブルクリックは既存の共通経路で `fs_zoom` / `fs_pan` / `fs_free_rotation` をリセットするが、
読書位置である `fs_vertical_scroll` は維持する。可視ページ数の安全弁が必要な場合はリセット後も
`fs_zoom` を安全な下限まで再び引き上げるが、`fs_pan` はゼロのまま中央へ戻る。連結方式から
ページ単位へ切り替える場合も既存の連結 transform リセットで `fs_pan` を消去する。

連結読み中も、フルスクリーン左パネルの「画像補正」「表示トリム」「ブックマーク」を
利用できる。編集対象はシークバーやページ番号表示が示す `fullscreen_idx` を基準に
`current_adjust_target_idx()` で解決するため、見開きでは選択中の左ページまたは右ページだけが
対象になる。左パネルを開いている間は、`FullscreenPageLayoutKind::Continuous` に記録された
対象ページの `DisplayedImageTransform::paint_rect` を枠線で示す。同じ transform は直交パン後の
配置から作るため、枠線もページと一緒に移動する。枠線は画像領域でクリップし、
ズームによってページの一部が画面外にある場合も追加の補正は行わない。枠線は暗い外縁と
高輝度の青い内線を重ね、白い紙面と暗い画像のどちらでも対象ページを判別できるようにする。

スクロールによる現在ページの再アンカーでは、対象変更前に未確定の表示トリムを保存するが、
左右パネルの開閉状態は維持する。表示トリムでユニット寸法が変わった場合は、古いスクロール
アニメーションを破棄して現在ユニットを表示基準位置へ再アンカーし、編集対象を画面内に保つ。
画像補正値と表示トリム、ブックマークは連結読みでも変更できる。一方、消しゴム、補正レイヤー、
隠蔽加工、切り取り、テキスト注釈、エクスポートはページ単位表示専用であり、連結表示中は
ヘッダーアイコンを無効化して「ページ単位表示でのみ使用できます」と案内する。対応する
ショートカットを押した場合も、同じ文言に機能名を付けた no-op 表示で理由を知らせる。

---

### 2.4.2 指定サイズfit（§1.342、実装・検証中）

決定仕様と検証の正本は [fullscreen-specified-scale-plan.md](fullscreen-specified-scale-plan.md)。
以下は接続する所有境界を記録する。実装差分の独立レビュー・自動gate・実機確認は完了扱いにしない。

`SpecifiedScale` は既存4方式に続く第5方式。各軸は `SpecifiedScaleAxisSetting { enabled, value }` で、
valueは `PercentTenths` / `LogicalPixels`。OFF中も数値・単位を保持し、再ONで同じ値へ戻す。
初期値は両軸ON・幅80％・高さ80％。％は描画先の現在の media rect、pxは100％時のpoint相当へ解く。
片軸ONはその軸だけ、両軸ONは内接、両軸OFFはPage相当のfit式になる。pointを `/ppp` しない。
最大倍率は共通Settingsの `fullscreen_specified_scale_max_tenths`（既定50＝5.0倍、10〜200＝1.0〜20.0倍、
0.1倍刻み）。既存no_up / no_downの後に新方式だけ `(max_tenths / 10.0) / ppp` 上限を適用する。
両軸OFFのPage相当にも適用し、no_upの1倍上限が勝つ。no_downの1倍下限は維持する。
手動zoomはその後に掛けるため共通上限を超えられる。最大倍率はfavorite専用値に含めない。
単ページZはPage・制限なしを維持し、見開きZの照準は選択fitと実効gapを使う。

`FullscreenFitInput` は旧方式と指定値・共通最大倍率snapshot＋`FullscreenFitBasis` を型で分ける。
Aは部分素材のcoverage座標aspect／RAWのdeveloped aspect／PDF page box、通常の全面rasterでは
解決済みAI前rasterのaspectとする。OはAI前の物理原寸で、Aの長辺をOの画素長辺へ正規化する。
通常全面素材はAも予測rasterと到着rasterで揃え、pre-AI cache登録だけで正準aspectから
整数丸め済みaspectへ切り替えない。正準source座標とページ構成の寸法は独立して維持する。
AI前寸法は既存edit / final AI / compositeのsource metadataへ対応付け、派生textureや
AI出力寸法から逆算しない。source寸法が判明している通常thumbnailで本画像未到着なら、生成時のcrop前raster寸法を使い、
それが無ければ正準寸法をproducerと共通の `gpu_raster_dimensions` へ渡してGPU raster寸法を予測する。
正準座標寸法を物理100%へそのまま流用しない。PDFは検証済みpage boxとcontent typeから、Vectorは長辺4096px、Rasterは
`min(4096, native_long_edge)` の固定Oを解く。既存source座標用のraster寸法は別用途のまま残す。
通常の旧catalog行でsource寸法が無い場合は、従来の選択済みsampler基準を受理時に保持して表示する。
これは既知寸法のGPU raster予測とは別の互換契約であり、描画時に不足寸法を探すfallbackではない。

`layout_full_image_rect` はこのA/Oから得るfull-page fit枠。実textureの整数丸めで縦横比が微妙に
違ってもこの枠を変更せず、`from_layout_rect` がsampling用の `full_image_rect` を枠内へcontainする。
paint / hit / UV / source写像は実際のcontained textureを共有し、PDF需要・合成layout・captureの
fit枠は安定したcanonical extentを使う。descriptorと枠はcapture時の値を保存し、frozen / holdover
replayやnavigatorから現在の指定値・共通最大倍率や新しいraster寸法を後読みしない。
単ページ・見開き・連結読みは選択した素材の `MaterialDisplayBasis::specified` からA/Oを読む。
部分素材はcoverageの正準座標寸法とcrop前raster、保持presentationは同時に保持したdescriptorを
使い、thumbnail退去後にidx探索の正準寸法へ100%基準を戻さない。見開き全体と各ページも
同じ `fs_spread_fit_input` の結果を使う。連結読みのreference／measurementも同じ素材selectorを使い、
元画像／解析表示ではsource素材、通常表示ではprocessed／保持素材を選ぶ。
§1.165最終P2追補（2026-10-22）の仕様差として、source寸法が判明している通常thumbnailの指定倍率は7474e5cf1の
thumbnail実寸（例:60×80）へ戻さず、到着予定GPU raster（正準15000×20000なら6144×8192）を
100%基準にする。指定倍率＋「縮小しない」でも到着時に枠・倍率が跳ばない。
通常rasterの指定方式では連結読みのsource原寸例外も外し、到着後・AI前metadata・退去後まで
同じraster基準を維持する。正準座標とページ構成、RAW／PDF、他4方式の連結規則は変更しない。
NormalResource保持後のthumbnail退去でも、素材自身の6144×8192などの受理済み基準を維持する。
同じallocationを共有するthumbnail／通過素材／元画像／finalも、実際に選んだproducerのdescriptorを
運び、texture IDから属性を逆引きしない。消しゴムの独立下地・worker preview・preview昇格、
注釈preview、連結transition、captureも入力ownerのdescriptorを継承する。
[素材所有の設計判断](material-display-basis-ownership-plan.md)に境界と維持する方式差を記録する。
捕捉済みの `SpecifiedScale.max_tenths` をgeometryの再解決にも使い、live設定変更を混ぜない。

見開きは既存高さ合わせと仮想slotを維持し、2枚＋gap全体をfitする。幅指定が極小なら表示だけ
`gap = min(saved_gap, max(0, specified_width - 1/ppp))` とし、保存gapを書き換えない。
連結の `ContinuousSpecifiedFitReference` は選択 / 明示値変更時の中央unitを保持し、scrollで
基準を替えない。共通最大倍率だけの編集でも基準unitを選び直さず、保持sourceから再解決した
同じunitのgeometryへ現在の共通上限を適用する。同じunitの確定済みAI前・非回転の元寸法は
元texture / 派生cache退去後も保持する。
現在のtrim / rotationからgeometryを各layoutで再解決し、fitと参照unit自身の配置に同じ値を使う。
解決済みbboxを退去時の値に固定しない。resize / DPIでは同じunitへ再計算し、手動zoomは保持する。可視枚数上限では
既存の遠いunit描画除外を使い、新方式のfit / fs_zoomを変更しない。旧4方式のper-unit fitと
可視上限時のauto zoomは維持する。

PDFの依存はmetadata → 固定A/O → fit枠 → 絶対需要 → raster結果の一方向とする。
初回 `StaticPdf` と再レンダcompletionは同じcontext / source / generation検証の後でmetadataを採用し、
既存retained PDF keyと一緒にsnapshotを保持する。同一metadataの再採用でOを更新しない。
`PdfRerenderDemand` のLegacyは旧idx / zoom閾値、Absoluteはsource identity付き各ページのtargetを
比較する。同じ需要で再送・in-flight cancelを増やさず、通常 / partner / manual / Zは新方式では
この一つの絶対需要へ集約する。headroom・最小 / 最大・native capは既存helperで一度だけ適用する。
凍結描画はlive siblingへ需要を送らない。

回転＋trimの入力は今版も経路別例外を保つ。通常単ページは保存回転でもtrimを渡すが、見開き・
連結unit・display-unit capture・暫定PDF bboxは回転ページのtrimを除く。分割優先と自由回転時の
trim解除も維持し、全経路統一は別件§1.358へ残す。

共通の指定値はSettings、favoriteの専用値は既存 `FavoriteViewState` / overlay / workerが所有する。
favorite外側Optionの欠落は共通値継承、明示両軸OFFは保持値を含む専用指定である。
共通・favorite、HUD・prefs、保存 / 再起動・転送は同じenabled＋valueを使う。未リリースの旧軸Option
形式からのmigrationは作らない。最大倍率は共通設定として保存・転送し、live編集はメモリ更新だけで
既存共通設定保存境界を使う。capture / idle reconcile / 離脱 / 終了は
保存stateの欠落を保持し、継承解決だけで書込みを予約しない。専用値を作るのはlive popupの
明示変更だけ。prefs / import / exportは共通値のrouteを使い、favorite専用行を上書きしない。
新しいDB・worker・viewport生成・host lifecycleは設けない。

### 2.5 ページ送り中の表示規則 (実装の正本)

> **v2.13.0 では通過表示をいったん削除し、2026-08-12 にページ単位表示へ再実装した。**
> `page_turn_decision_for_inputs` / `FsPageTurnDecision` と `fs/page_turn_ready` /
> `fs/page_turn_decision` は現行コードに存在する。単一ページ / 見開きとも physical key level が
> held で実際の表示単位遷移が成立したバースト中は、全ページ一様に低解像度の色忠実 rendition を
> 使い、release 後または境界 no-op 後に完成画像へ戻す。
> `colorize_display_requires_final_effect` / `waiting_for_colorize`、またはページ送り判定を変更する前に、
> この節を読むこと。
> **この領域は繰り返し壊れており、壊れ方が毎回違う。**
> 2026-08-11 の 1 日だけで 3 回退行した (すべて「支配している規則を読まずに判定へ条件を
> 足した / 反転させた」もの)。

> **2026-08-14 Part B 追補 (現行の最優先規則)**: held browsing は入力レートへ
> 追従して idx だけを先行させない。`FsHoldover::NavigationSequence` が受理済みの
> 1 display unit を所有し、その unit 自身の rendition / materialized image が実際の
> 最終描画 source として 1 frame 提示されるまで、次のページ送りまたは手動フォルダ移動を
> 受理しない。待機中に届く key repeat / 同一 frame の追加 edge は **drop し、queue しない**。
> ページ送りの入力端は、viewport-stamped `KeyEdge` の順序を保った typed consume result から
> first keydown / auto-repeat、matched chord、送信元 viewport、処理時点の physical held / 同一 frame
> の後続 key-up を判定する。first keydown は同一 frame に key-up があっても必ず 1 回受理する。
> navigation へ昇格させず consume-only にするのは、処理時点ですでに release 済みの auto-repeat
> だけである。これは受理済み `FsNavigationSequence` target の取消しではない。受理済み target は
> release 後も少なくとも 1 frame 提示し、提示前の `fullscreen_idx` rollback は行わない。
> 未キャッシュ PDF は対象ページを既存 thumbnail worker / PDFium pool へ interactive priority
> で要求し、約 300ms かかっても rendition を提示してから次へ進む。これは以下の過去の
> 「入力に追いつく」「約 100 ページ/秒」という性能記録より優先する現行仕様である。

#### 2.5.1 満たすべき要件

| ID | 要件 | 出どころ |
| --- | --- | --- |
| **R1** | **ページ表示自体を飛ばさない。** キーを押しっぱなしで通り過ぎるページも、一瞬でも必ず 1 回は画面に出す | 利用者要件 (§1.58、2026-08-07) |
| **R2** | **白黒 → カラーの切り替わりを見せない。** カラー化や LUT が乗る絵で、色が付く前の状態を一瞬でも出さない | 利用者報告 2026-07-29 (「LUT 未適用の絵が 1 フレーム見えてから色が変わる」)。確定要件 |
| **R3** | **押しっぱなしでも内容を見せる。** 受理済み unit が未提示の間は次の repeat を drop し、idx だけを先行させない。ready な rendition を描いている間は target を含む full-size producer / final 合成 / upload を後回しにする。rendition が無く `Materialized` を描画元にしている間は、sequence を settle する target 自身の producer と consumer を通し、target 外の先読みだけを保留する | 利用者判断 2026-08-14: seek bar / Ctrl+G は高速移動、held key は内容を見ながら読む操作。paint source に従う target materialization 例外は 2026-08-20 の二層の循環待ち修正 |
| **R4** | **キーを離したら完成画像で終わる。** 押し終わったページは通常の画質・加工で表示する | 同上 |

R1〜R4 は同時に満たす。**どれか 1 つのために他を崩す修正を入れない。**

##### 2.5.1.1 何のための機能か — 解像度は変わってよい、色は変わってはいけない

**利用者の意図 (2026-08-11 に明示)**: ページ送りは**音声のスクラブと同じ**もの。今どのあたりに
いるかを何となく見ながら高速に切り替えて、目的のページを探すためにある。

そこから 2 つの軸の扱いが決まる。**この区別が §2.5 全体の前提になる。**

| 軸 | 通過中の扱い | 理由 |
| --- | --- | --- |
| **解像度** | **変わってよい。サムネイル画質で構わない。** 止まったページでフルサイズに差し替われば良い。サムネイルキャッシュが無いページを、一旦サムネイル画質で出してから差し替えるのも可 | 通り過ぎる絵に精細さは要らない。探すのに必要なのは「何が写っているか」だけ |
| **色** | **変わってはいけない。** 通過表示にも完成画像と同じ色処理 (色調補正 / カラー化 / LUT) を乗せる | 見た目の差が大きく、ページごとに色が入れ替わると破綻して見える (R2) |

したがって **通過表示に必要なのは「フルサイズを速くする」ことではなく「色を合わせた低解像度を出す」こと**。
2026-08-11 の 5 回の失敗は、通過表示という発想が誤りだったのではなく、
**描画元をページごとに変わる条件で選び、しかも色を合わせていなかった**ことによる (§2.5.2.1)。

> 実測 (2026-08-11、`scripts/page-turn/measure.rhai` + 24MP JPEG fixture): 1 ページの
> UI スレッドコスト 28.2ms は**ほぼ全量がフルサイズのテクスチャアップロード**で、元画像の
> 画素数にほぼ比例する (11.8MP で 13.9ms)。表示は精々 1000×1500 程度なので、通過中に
> フルサイズを上げていること自体が費用の源。

#### 2.5.1.2 連打は押しっぱなしと別経路 — 失われるのは到達点であってページの並びではない

§2.5 の本文はほぼ全部が**押しっぱなし (held browsing)** を前提に書いてある。
**連打 (down/up のペアが繰り返し届く形)** は通る経路が違うので、ここに分けて置く。
2026-08-16 に外部の計測ツールでの挙動を問われ、コードから再構成するのに 3 往復かかった。

**通過表示 (stand-in) には入らない。** `fs_page_turn_input_held` は、移動を受理した瞬間に
記録した `still_held` (`page_turn_edge_hold_this_frame`)、それが無ければ
`key_held_chord_via_os` の OS 直読みで決まる。連打は毎回 down と up が揃って届くので
`still_held` は常に false で、通過表示の条件を満たさない。したがって `accept_rendition` も
立たず、移動先は `materialized_ready` を待つ。

> 実測 (2026-08-16、FHD ダミー 555 枚、`SendInput` で down/up ペアを 60Hz):
> `fs/page_turn_decision` は全件 `pending_zero` / `no_page_transition` で、`pass_through` は
> 1 件も出ない。

ただし `materialized_ready` の判定は `resolve_fs_display_tex(idx, include_thumb = true)` なので、
**フルサイズが未完でもサムネイルがあれば「提示済み」として次の移動を受理する**。
連打だからといって毎ページがフルサイズで確定するとは限らない。

**押下回数と到達点の関係。** ページ送りは 1 フレーム 1 ページが上限
(`consume_key_press_edges_with_result` はそのフレームの一致エッジを全部消費するが、
受け側の `page_next` は `bool`)。さらに未提示の受理済み target がある間は移動要求自体を
受け付けない (Part B 追補)。**このどちらで失われるのも「押下」であって「ページ」ではない。**

| | 起きること |
| --- | --- |
| ページの並び | **抜けない。** 到達した範囲は必ず連番で 1 枚ずつ表示される (R1) |
| 到達点 | 失われた押下のぶん手前で止まる。N 回押しても N ページ進むとは限らない |
| 画質 | フルサイズを優先するが、間に合わなければサムネイル提示で先へ進み得る |

> 実測 (同上): 5 / 30 / 40 / 50Hz は 554 押下 → 554 移動。60Hz は 554 押下 → **538 移動**で、
> `input/page_turn_edge` も 538 件 (= 16 押下が同一フレームで畳まれた)。idx の遷移は
> 538 回すべて +1 で、**飛んだページ番号はゼロ**。到達は 539 ページ目。

この上限は「ページ送りをキューに溜めない」という設計方針そのもので、仕様として維持する
(利用者確認 2026-08-16)。入力側の基数規約 —— ページ送りは one-shot single-dispatch、
フォルダ移動 (Ctrl+↑↓) だけが押下回数を保つ —— は
[docs/keymap-spec.md](keymap-spec.md) を正本とする。

#### 2.5.2 描画元と work admission は target materialization で接続する

判定は 2 つの問いを持つが、**独立ではない**。描画元を待っている navigation target が、
その待ちを終わらせる唯一の upload まで保留できてはならない。2026-08-20 の実ログでは、
見開きの片側だけ thumbnail が無く、もう片側だけ full-size ready、残り片側の decode 結果が
`fs_upload_backlog` にある状態で、旧 `defer_ui_uploads=true` が backlog 全体を止めた。
target は materialize せず sequence も退役できないため、同じ `still_awaiting` が無期限に続いた。

| 軸 | 問い | 誰が読むか |
| --- | --- | --- |
| **A. 描画元** (`paint_source`) | このフレームで**何を描くか** — カタログサムネイルか、完成画像か | `prepare_fullscreen_state` |
| **B. 重い処理の admission** (`FsPageTurnWorkAdmission`) | このフレームで**どの full-resolution work を許すか** — 全許可か、現在の navigation target の materialization だけか、全保留か | `ensure_fs_page_turn_target_load` / `poll_final_effects` / `fs_upload_backlog` の消化 |

- **B は A に従う**。非 active は `All`。active でも rendition 未着で A が
  `Materialized` なら `NavigationTargetMaterializationOnly` とし、target page set の
  `ensure_fs_page_load` / PDF display-resolution producer、final-effect 回収、完成済み
  `fs_upload_backlog` を許可する。ready rendition があり A が `PassThrough` なら
  `Deferred` とし、target 自身を含む full-resolution work を保留する。どちらの active
  状態も target 外の先読みは保留する。見開きの target consumer は現在 idx と相方を既存の
  1 frame pacing (現在 + もう 1 枚) で処理する。
- viewer 内ページ送りの open も同じ 3 状態を使い、`NavigationTargetMaterializationOnly` は
  target の `ensure_fs_page_load` だけを開始して `update_prefetch_window` は開かない。grid からの
  新規 open は page-turn sequence ではないため従来どおり `Eager` である。
- probe の `defer_ui_uploads=true` は既存ログ解析との互換フィールドとして残るが、意味は
  「少なくとも非 target の full-resolution work を保留する」である。target も保留するかは
  `ui_work_admission` の typed 値を見る。
- rendition 未着の間は `Materialized` を選び、typed sequence が
  直前の complete unit を overlay する。rendition が atomic unit 全体で ready になった frame
  だけ `PassThrough` に切り替える。従来の「rendition が無くても PassThrough」は廃止した。
  ready は「提示済み」ではなく、描画末尾で target page set と最終 source を照合して初めて
  sequence を完了する。

#### 2.5.2.1 「ページ送り中か」は**フレーム間で安定した信号**でなければならない

**この節は 2026-08-11 に 4 回連続で退行させた原因そのもの。判定を触る前に必ず読むこと。**

通過表示に入るかどうかは「いまページ送り中か」で決まる。この信号が**フレームごとに変わると、
描画元も一緒に変わってちらつく**。

§1.58 の初期実装は、これを **「このフレームに未消費のページ送りキー edge が残っているか」**
で判定していた。キーリピートは約 30 回/秒、描画は 60fps なので、**おおむね 1 フレームおきに
true と false が入れ替わる**。その結果、同じページのまま描画元が毎フレーム往復する。

実測 (2026-08-11、`analyze_perf.py page-turn --check`):

```
I1 violation: burst t=45.222..45.987 idx=123
  final_composite -> thumbnail -> final_composite -> thumbnail -> ... (28 往復)
```

ページは動いていない。**入力信号が 30Hz で振動しているだけ**。

したがって判定には、**そのフレームに edge が来たか**ではなく、
**ページ送りキーが今押されているか** (`keymap::key_held_chord` /
`key_held_via_os` の OS 直読み) を使う。押下状態は物理状態そのものなので、
リピート周期に関係なくバーストの間 true のまま安定する。

ただし physical held は**必要条件であって十分条件ではない**。描画より後に解決される navigation
結果が実際に display unit を変えた時点で `FsPageTurnBurstState::Active` にし、その target idx と
items generation を所有させる。先頭で戻る、末尾で進むなど target が無い / current idx と同じ
入力はバーストを作らず、すでに active な hold が境界 no-op に達した場合はその場で
`Idle` に戻して着地点を materialize する。これは page / cache readiness ごとの描画条件ではなく、
**navigation が遷移を作ったかというコマンド結果**である。edge、時間閾値、ヒステリシスによる
平滑化は使わない。

2026-08-14 以降、`FsPageTurnBurstState` は「同じ physical hold の 2 回目以降なら低解像度
target にできる」という画質選択だけを所有する。受理可否と表示待ちは
`FsNavigationSequenceTarget::{FolderItems, Display}` と
`FsNavigationTargetPhase::{Awaiting, Ready, Presenting, RenditionFailed}` が所有する。
`Awaiting` / `Ready` / `Presenting` 中の repeat は edge multiplicity を含めて捨てる。
`RenditionFailed` は damaged/password page 等の terminal outcome であり、次 target を受理できる。
ただし release 後に landing target の full materialization が成功または失敗確定するまでは、
captured previous unit を原子的に保持する。

§1.58 が edge 方式を選んだのは「時間閾値・押下開始時刻・前フレームからの pending 状態を
判定に使わない」ためだが、**押下状態の直読みはそのいずれでもない** (履歴を持たない現在値)。
制約の意図を保ったまま、信号だけを安定させられる。

**禁止**: この不安定さを、時間閾値・ヒステリシス・「前フレームの決定を覚えておく」で
埋めないこと。原因は信号の選び方であって、平滑化で隠す対象ではない。

#### 2.5.3 サムネイルを代役に使ってよい条件

通過表示は「カタログサムネイル、またはその場で PDFium 等から作る低解像度 rendition を
そのページの代役にする」表示契約である。未着なら直前の表示単位を原子的に保持し、後続 repeat を
drop する。rendition ready 後の target を実際に 1 frame 提示してから次を受理する。
加工の扱いは**見た目の差**と**処理時間**の 2 つで決める。

§1.165 B1 では、編集 preview を代役に採る前に §1.3.1 の共通 Unknown policy を通す。
Known coverage は現在 crop から変更せず、CPU 下地から生成する rendition と保持 resource へ
同じ由来を継承する。全体寸法の fit と部分素材の mapping は §2.1.1 を共有し、crop 前の全体を
crop 後 texture で引き伸ばさない。これにより本画像到着時に既存内容の倍率・位置とページ枠を
維持し、未収録だった外側が補われることを受入条件とする。代役が持たない crop 外画素の復元を
到着前の要件にはしない。旧 Unknown＋現在 crop ありはこの代役を使えないため、本画像または
既存の利用可能な代役を待つ互換上の制約が残る（実装・自動検証済み・独立レビュー受入可・ClaudeCode の隔離環境で実アプリ確認済み、利用者の実機確認は未）。
renditionは下地の受理済み `MaterialDisplayBasis` も継承する。無加工時にcatalogと同じtextureを
共有しても、通常thumbnailと通過素材の方式別契約はそれぞれの素材が保持する値を使う。
生成後のselector・連結transition・ページ送り受付ではResourceのまま運び、保持済み素材から
handleだけを取り出してthumbnail寸法や現在のidxから基準を作り直さない。PDF metadataの採用で
現在のthumbnailを再受理した場合は、そのthumbnailに依存するrenditionを再生成対象とする。
既に保持・captureした素材の不変descriptorは書き換えない。

| 加工 | サムネイルとの見た目の差 | フルサイズの実測 | 通過表示の扱い |
| --- | --- | --- | --- |
| 色調補正 (明るさ / コントラスト / 彩度など) | 色が変わる | `adjust_ms` **0.00ms** | **サムネイルに適用して使う** |
| Creative LUT | 色が変わる | `creative_lut_ms` **0.00ms** | **サムネイルに適用して使う** |
| カラー化 | **白黒 → カラー**。差が最も大きい | 下表のとおり寸法依存。347×506 で **1.67ms** | **サムネイルに適用して使う** (実測済み、下記) |
| 消しゴム / 隠蔽 / テキスト注釈 | 内容が変わる | 最悪 **1 秒以上** かかりうる | **サムネイルを使う (加工なしで通過してよい)**。待つと R1 / R3 を壊すため、意図的にスキップする |

**消しゴム・隠蔽・注釈をスキップする理由は「色ではないから」ではなく「時間がかかりすぎるから」。**
将来この 3 つが十分速くなったら、扱いを再判断する余地がある。

**サムネイルキャッシュが無いページの扱い**: target page set を既存 `keep_set` の一時 anchor
として保持し、thumbnail worker へ priority request を発行する。PDF page は PDFium pool の
HighNormal lane へ昇格する。catalog hit と `thumb_pixels` の両方が無い場合も request を省略せず、
source render 完了後に色忠実 rendition を同期生成する。生成中は previous unit を保持する。
worker が `ThumbnailState::Failed` を返した場合は `RenditionFailed` へ遷移し、sequence を永久停止
させない。

##### 2.5.3.1 寸法別の色処理コスト (2026-08-11 実測)

`cargo test --release -p mimageviewer --lib colorize::tests::thumbnail_effect_cost_measurement
-- --ignored --nocapture` (24 スレッド、`legacy4color` / `luminance100` / `gaussian` /
`radius1.0` / `strength100` + `contrast:+20`)。

| 寸法 | 画素数 | 色調補正 中央値 | カラー化 中央値 | 最大 | うちトーンぼかし |
| --- | --- | --- | --- | --- | --- |
| **347×506** | 0.18MP | **0.25ms** | **1.67ms** | 4.75ms | 1.25ms |
| 800×1200 | 0.96MP | 1.22ms | 5.76ms | 7.12ms | 4.67ms |
| 1123×1648 | 1.85MP | 2.40ms | 9.97ms | 10.34ms | 8.29ms |
| 2480×3508 | 8.70MP | 11.74ms | 41.24ms | 47.62ms | 32.49ms |

**§2.5.6 が警告していた固定費は実在した。** 面積比だけなら 347×506 は 1123×1648 の 0.095 倍
なので 0.95ms のはずだが、実測は 1.67ms (0.167 倍)。差はトーン階調化のぼかしで、
サムネイル寸法でもコストの 4 分の 3 を占める。

それでも **サムネイル 1 枚あたり 色調補正 + カラー化 ≒ 2ms** で、キーリピート間隔 34ms に対して
十分小さい。**したがって通過表示の色処理は UI スレッドで同期的に行ってよい。** 非同期にすると
「色が後から付く」状態が生まれ、それは R2 の失敗そのものである。同期実行なら**同じフレームで
色が揃うことが構造的に保証される**。

`colorize_display_requires_final_effect(idx)` ([ui_fullscreen.rs](../src/ui_fullscreen.rs)) は、
materialized 経路で raw / edit fallback を見せず final-effect を待つための保守的な gate である。
**この gate の true を、通過 rendition にカラー化を適用する意味へ読み替えてはいけない。**
`MonochromeOnly` で full-size summary が未到着、または色調補正が classifier 入力を変える場合、
gate は「未確定なので待つ」という意味で true を返すためである。

通過 rendition の `MonochromeOnly` 適用可否は、(1) 同じページ / 編集世代 / params の常駐する
完成 composite が保持する実 worker 判定、(2) 色調補正が identity で同じ `EditResultKey` の
full-size summary、(3) 色調補正後の低解像度画素に対する既存の `colorize::should_apply`、の順に
1 回だけ解決し、`colorize_applied` を rendition cache entry に保持する。着地点の final-effect
worker は同じ entry の判定を再利用する。既存 final があるページでは rendition が final に合わせ、
final が無いページでは final が rendition に合わせるため、thumbnail と full-size の classifier が
食い違っても settle で色が変わらない。raw decode 到着だけで進む source generation はまたいでよいが、
編集世代 / params が違う entry は再利用しない。この判定は pass-through / materialized の mode
選択には使わない。

#### 2.5.3.2 実測: 軽い本では見開きだけに露出した (2026-08-11〜12)

**利用者が引っかかりを確認したフォルダ** (`aimodel1`、306 枚、**1.0〜1.6MP**) を、同じフォルダ・
同じ設定で単一ページと見開き (RTL) の両方で計測した。この軽い本では解像度は支配項でなかった。

| モード | ナビ命令 | 実際に描かれた枚数 | 画面滞留時間 | 逆行 | 再訪 |
| --- | --- | --- | --- | --- | --- |
| **単一ページ** | 144 | **144** (1:1) | 中央値 34.7ms | 0 | 0 |
| **見開き (RTL)** | 142 | **23** (6:1) | 中央値 35ms / **最大 2267ms** | 0 | 0 |

見開きでは 142 回のページ送り命令に対して **23 枚しか描かれない**。描かれた idx は
`14 → 57 → 70,71,72,73 → 80,…,94 → 229 → 234,235,238` と飛び、**1 枚が 746ms / 2267ms
画面に留まる**。これが利用者報告の「たまに一瞬同じ画像が止まる」の実体。

**ナビゲーション自体は正しい。** `input/fs_key` は `Target(2), Target(4), … Target(14),
Target(15), Target(17), …` と単調増加で、逆行は 0 件。つまり**入力も遷移も壊れておらず、
描画が単位ごと落ちている**。画面には前の表示単位が残り続けるので、止まって見えたあと飛ぶ。

> 「ページ数が巻き戻るようにちらつく」については、**逆行そのものは観測できていない**。
> 止まって飛ぶ挙動が巻き戻りとして知覚されている可能性が高いが、未確認。

単一ページはこの条件では 1 命令 1 描画を維持し、すべて `final_composite` で追いついていた。
一方、見開きの主症状はスループットではなく、表示単位が final 完成まで旧 unit に覆われることによる
描画欠落だった (§2.5.4)。どちらも R1 は同じであり、単一が常に安全という意味ではない。

その後、4.1MP の実 ZIP (`原神.zip`) を単一ページで測ると、materialized 経路は **174 命令に
対して 29 表示 (約 1:6)** しか出なかった。`aimodel1` の 1:1 は完成処理が 34ms の repeat に
たまたま追いついていただけで、高解像度では単一ページにも同じ R1 違反が露出する。この実測を受け、
利用者判断で単一ページも見開きと同じ pass-through 規則へ統一した。

計測メモ: **RTL 見開きでは Right が「戻る」方向**。前進は Left。RTL で Right を押し続けると
`page_nav=Boundary { at_end: false }` が 144 回並ぶだけになる (実際にそうなった)。

##### 方向による差 (2026-08-11、利用者の「戻りの方が引っかかる気がする」を検証)

同一 run で 3 回のホールドを取った。**集計値は方向でほぼ差が無い。**

| ホールド | ナビ命令 | 描画枚数 | 滞留 中央値 | 最大 |
| --- | --- | --- | --- | --- |
| 前進 | 142 | 23 | 35ms | 2266ms |
| 戻り (直前に通ったページ) | 143 | 23 | 36ms | 2284ms |
| 戻り (末尾から) | 143 | 22 | 36ms | 2280ms |

ただし**最初の無反応時間は戻りの方が長い**。末尾からの戻りは idx 303 から始まり、
**最初の 1.29 秒は 1 枚も描かれず**、238 に達して初めて描き始めた (前進の初回停止は 746ms)。

**そして 2 周目以降に大量の再デコードが起きる。**

| ホールド | decode 開始 | 対象ページ | **既に composite 済みなのに再デコード** | 同一ホールド内で 2 回以上 |
| --- | --- | --- | --- | --- |
| 前進 (初回) | 261 | 261 | 0 | 0 |
| 戻り (末尾から) | 353 | 264 | **228** | 44 |

初回前進は 261 ページをデコードして 23 枚しか出さない。末尾からの戻りは 264 ページ中 **228 ページが
「以前 final composite を作ったのに再デコードされている」**。これが backlog の対策 2
(再デコードの抑止) の実データ。

**方向そのものの影響か、単に 2 周目だからかは切り分けていない** (前進の 2 周目を測っていない)。
利用者が戻りで引っかかりを強く感じるのは、実使用では前進で読み進めた後に戻るため、
この再デコード経路に常に入ることで説明が付く可能性がある。

**フレーム自体は詰まっていない。** 5 秒のホールド中に 588〜610 フレーム描かれており
(間隔中央値 6〜7ms)、その中で表示画像が変わったのは 22〜23 回だけ。
つまり**描画が遅いのではなく、表示単位が更新されない**。

##### 根本原因と 2026-08-12 の修正

当時の source inspection で、見開きの表示欠落は `open_fullscreen` が前の
`ColorizeDisplayUnitHoldover` を capture し、`colorize_display_unit_holdover_for_draw` が対象見開きの
全ページで final texture が解決するまでその旧 unit を通常描画の上へ重ね続ける経路と確定した。
ナビゲーション先の frame は描かれていたが、旧 unit の overlay に隠されていたため、final が同時に
揃った表示単位しか画面上で観測されなかった。

再デコードは key mismatch ではない。ページ送りのたびに `open_fullscreen` が
`ensure_fs_page_load` と `update_prefetch_window` を起動し、通過ページまで full decode / final pipeline
へ投入していた一方、`fs_cache` / `final_composite_cache` の小さい keep window が先に通ったページを
退避した。2 周目は正しい同じ key であっても resident entry が既に無く、もう一度 source decode へ
入っていた。

修正後は、単一ページ / 見開きで physical key level が held かつ実遷移バーストが active の間、
次を一体で行う。

- 表示単位の全ページを、色調補正 → カラー化 → Creative LUT 済みの低解像度 rendition として
  同じ frame で原子的に解決する。ready 状態は通過モードへ入る条件に使わない。
- `open_fullscreen` は通過先の full decode / prefetch を開始せず、進行中の context-owned
  decode / final effect / AI producer を cancel する。resident cache と完成済み upload backlog は保持する。
- release frame は current display unit だけを通常の eager materialization へ戻し、その後の通常
  prefetch を再開する。
- 通過終了後は着地点の色忠実 rendition を final 到着まで維持する。通常ページ移動は旧
  `ColorizeDisplayUnitHoldover` を作らず、同一 unit の source reload だけが typed holdover を使う。

最初の見開き修正 (96faeee6) は利用者実測で、**267 / 267 ページを順番どおり表示** (間隔中央値
30ms)、hold 中の `decode_begin` **0 / 0 / 0**、release 後 **29〜56ms** で左右同時に完成画像へ
settle、`page-turn --check` は **checked bursts=3 / violations=0** を確認した。

単一ページ側の source inspection では、`fs_page_turn_ordinary_context_blocker` の
`single_page_materialized` が pass-through を明示的に除外し、`open_fullscreen` の eager
materialization へ流していた。4.1MP ZIP では次の final texture が間に合わず旧ページが残るため、
174 命令のうち完成した 29 ページしか `fs/paint` に現れなかった。この blocker を除き、active
burst 中は resident final の有無を見ず rendition だけを選ぶようにした。4800f2cd の利用者実測では、
単一 `aimodel1` が 144 命令 / 145 表示、単一 4.1MP ZIP が 174 命令で全 30 ページ表示、見開きが
144 命令 / 267 ページ表示で、hold 中 decode は全ケース 0〜1、I1〜I5 は violations=0 だった。

2026-08-12 の追加 source inspection で、残る 3 件の経路を次のように確定した。

- **境界での画質低下**: `render_fullscreen_viewport` は navigation 解決より前に physical held だけで
  pass-through を選び、その frame の後段で `adjacent_navigable_idx` が `None` /
  `FsPageNav::Boundary` を返していた。idx が変わらなくても rendition を描く順序だった。navigation
  結果を burst owner へ publish し、実遷移だけを active、boundary no-op を idle とするよう直した。
- **縦横比の差**: 単一の `draw_fs_image` は rendition texture の `size_vec2()`、見開きの layout も
  左右 texture の整数丸め後寸法を fit に使っていた。`ThumbnailState::Loaded::source_dims` は既に
  保持していたが、この経路が参照していなかった。通過中の layout は source/header 寸法で解決し、
  texture 寸法は sampling と upload 判定だけに使う。
- **MonochromeOnly の誤カラー化**: `ensure_passthrough_rendition` が
  `colorize_display_requires_final_effect` の保守的な「待つ」結果を
  `final_color_effect_required` として builder へ渡し、builder が true を「カラー化を適用する」と
  解釈していた。full summary 未到着のカラーページほど強制カラー化される意味の反転だった。
  常駐 final の実判定、既存 full-size summary、rendition 自身の adjusted pixels の順に 1 つの判定を
  解決し、landing final と共有するよう直した。

#### 2.5.3.3 実測: 通過表示の上限は約 100 ページ / 秒 (2026-08-12)

外部からの「mIV は 125Hz リピートで 50〜52fps しか出ない」という指摘を受け、4K 165Hz の
foreground で 500 ページの FHD ZIP (1200×1700 = 2.04MP、STORED、335MB) を各リピートレート
1 ホールドずつ、毎回新しいプロファイルで測った。合成入力は `scripts/page-turn/measure.rhai`。

| リピート | 入力 / 秒 | **表示ページ / 秒** | **fps** | フレーム間隔 中央値 | p99 | 最大 |
| --- | --- | --- | --- | --- | --- | --- |
| 30Hz | 29 | **29** (1:1) | **165.2** | 6.0ms | 8.0ms | 9ms |
| 125Hz | 119 | **100** | **164.1** | 6.0ms | 8.0ms | 18ms |
| 165Hz | 156 | **100** | **164.4** | 6.0ms | 8.0ms | 15ms |

**フレームレートは 3 条件とも表示リフレッシュに張り付いている。** 「50fps 止まり」は
この構成では再現せず、修正前の描画欠落 (§2.5.3.2 の 142 命令 / 23 表示) を fps として
観測したものと考えるのが自然。

**表示ページレートは約 100 / 秒で頭打ちになる。** 律速は通過表示の素材づくりで、165Hz
ホールド中の内訳は次のとおり:

| イベント | 件数 | 中央値 | 合計 |
| --- | --- | --- | --- |
| `thumb/decode_end` | 460 | **32.8ms** | 14356ms |
| `details_meta/load_done` | 487 | 1.69ms | 796ms |
| `fs/decode_end` | 5 | 26.97ms | 129ms |

5 秒のホールド中に 460 枚のサムネイルをワーカー並列で生成しており、460 / 5 秒 ≒ 92/秒が
表示ページレートとほぼ一致する。**カタログキャッシュが無い本でも通過表示は成立する**
(その場で生成される) が、生成スループットが上限を決める。

`fs/paint` の実測 geometry は `w=1016.83, h=1440.0, texture_w=334, texture_h=473,
scale_x=3.0443971, scale_y=3.0443973` で、論理 1440 × ppp1.5 = 実 2160px の 4K 描画、かつ
`scale_x` と `scale_y` が一致する (e7191ad2 の縦横比保持が効いている)。

**出荷時の言い方**: 「連続ページ送り中はサムネイル画質へ落として高速に切り替える。
その状態でおよそ 100 ページ / 秒」。フル画質のまま同じレートを出せるかは未測定
(ホールド中は full decode / upload を意図的に抑止しているため、別構成での測定が要る)。

#### 2.5.4 表示単位 (単一 / 見開き / 連結)

単一ページは 1 ページ、見開きは左右 2 ページを表示単位として、**表示単位全体を 1 回で解決する**。
受理時に current unit の texture / rotation / Part A の captured layout geometry を
`FsNavigationSequence::previous` へ 1 unit として取る。`FsNavigationSequenceTarget::Display` は
受理した `anchor_idx` と通過 rendition の許可方針を target の生存期間を通じて固定し、現在の
canonical page set は `Awaiting / Ready / Presenting / RenditionFailed` phase が所有する。
寸法または 90 度回転が後着して anchor の canonical unit が単ページ↔見開きへ変わった場合は、
どの phase からでも新しい page set の `Awaiting` へ戻り、同じ許可方針で全ページを再検証する。
`previous` と待機開始時刻はそのまま保持する。target page set は、page renderer が描く
`GridItem::has_page_data` の項目だけで構成する。動画 / 音声など native presenter が描く項目を
1 ページでも含む unit では `Display` sequence を作らず、そのまま移動先へ着地する。
sequence を解放する presentation trace は page renderer だけが emit するため、native presenter 所有の
項目を target にすると閉じられない sequence になり、後続 target を永久に block するためである。
この条件は単一ページだけでなく、片側だけが native presenter 所有になり得る混在 unit にも適用する。
見開きは左右 rendition が揃った frame だけ `Ready(Rendition)` になり、片側だけ完成画像、もう片側
だけ rendition という組み合わせを描かない。rendition がまだ無ければ `Awaiting` のまま previous
unit 全体を描く。target と同じ idx に旧 texture が残っていても、texture identity が captured
previous に属するなら提示とは数えない。

描画直前に `Ready` を `Presenting` へ一方向遷移させて previous overlay を外す。描画末尾で
`fs_display_unit_trace_pages` が target generation、完全な page set、各ページを実際に選んだ
`Live` / `Holdover` provenance を返し、target の全ページが `Live` として描かれたことを確認して
初めて「提示済み」と判定し sequence を解放する。旧 unit と target unit が同じページを共有して
同じ texture id を使っても、選択元が live なら `Live` であり、texture id の包含から ownership を
推測しない。previous overlay を実際に描いたページは `Holdover` なので解放しない。
したがって次の同 frame repeat が受理される場合でも、その前に target の draw command は確定済みで
あり、見開き片側だけを提示済みと数えることはない。

fullscreen frame は async 完了を `poll_prefetch` で取り込んだ後に canonical `SpreadPair` を 1 回
解決し、その同じ pair で target の再 binding / readiness、holdover disposition、source 選択、描画、
presentation trace を行う。poll 前に作った page-turn decision が同じ egui frame に cache 済みでも、
page set が変わったときは rebound phase から decision を再計算して同じ cache slot を更新する。
通常の sequence 非存在 frame は事前の target identity check で余分な spread 解決を行わない。
target の generation / anchor が現在の mounted viewer context と一致しない場合は再 binding せず、
兄弟 context の page set や holdover を変更しない。
同一 context 内で common open、ページ編集の単ページ pivot、または連結読みの seek / scroll reanchor が
別 anchor へ移る場合は、旧 `Display` intent とその lock / holdover を代入前に明示的に破棄する。
同じ mounted bundle の items generation が実際に変わる場合も、旧 index identity を指す `Display` intent を
generation owner で解放する。`FolderItems` は folder install 前の世代から新 items の bind までを所有するため
この解放対象に含めず、同じ generation の再設定でも有効な `Display` intent を維持する。
viewer-context の activation は `fullscreen_idx`、generation、navigation sequence、lock / holdover を
bundle 単位で一緒に swap する ownership 移動なので、この in-context retarget cancellation は行わない。

target の `materialized_ready` は、その frame の描画 resolver が実際に選ぶ source に対して評価する。
`fs_display_bypasses_final_pipeline` が true の分析モード、またはナビゲーションシーケンスが
`blocks_new_target()` でなく、かつページ送り入力も物理ホールドされていない状態で元画像表示が
成立する frame では、target 全ページの raw `fs_cache`
(`resolve_original_preview_tex`) が解決したときだけ ready とし、AI・カラー化済み final の完成は
要求しない。false の通常表示は従来どおり加工済み source を要求し、raw があるだけでは ready にしない。
raw source を選ぶ frame で元画像も無ければ `Awaiting` を維持する。見開きはどちらの source でも
全ページの `all` 判定を保ち、片側だけで ready にしない。元画像表示の OS キー状態は frame ごとに
一度だけ sample し、producer gate / readiness / draw が同じ値を共有する。ページ送りの物理レベルも
`(frame_nr, viewport)` 単位で一度だけ sample し、元画像判定と page-turn burst 判定が共有する。

§1.341のbook leaf（[page-turn-animation-plan.md §19](page-turn-animation-plan.md)）も、このnormal prepare/source選択を一回だけ使用する。
leafのvisible receiptは一部のincoming頁がまだ裏にあるため、通常`page_turn_ready`/navigation sequenceの完了証拠にはしない。
各頁の初出はpolygon可視面積で記録し、全実頁のcanonical flatがoutput hookで採用された後のowning frameで完了する。
割込みでも同じflat採用を先に行い、close→継続commit→fresh drawing/input snapshotを共有する。
normal layoutのH/gと各頁のprepared rect/UVをprojectionへ渡し、異寸法のscanを同じrectへ揃えない。
効果専用decode/upload・UI thread I/Oを追加せず、設定OFFの通常flat経路は維持する。
演出の入力観測も通常key handlerと同じpassのkeyboard ownerを先に選択する。
gamepadのneutral pollは操作factを発行せず、sampled analog factだけが演出を取り消す。
設定ONの受理済み移動は、候補欠落/複数入力やinstant経路でも既存transactionで一回の理由付きskipを記録する
（[page-turn-animation-plan.md §20](page-turn-animation-plan.md)）。

通過 rendition の外側の**ページ矩形**は、低解像度 texture の整数寸法ではなく source/header 寸法
から求める。PDF は thumbnail / fullscreen の各 raster 寸法が別々に整数丸めされるため、PDFium が
読む page box を `layout_width/layout_height` に 1/1000 point の固定小数点レイアウト寸法として
保存し、通過 rendition と完成 texture のページ矩形を同じ位置・大きさに固定する。一方、
`source_width/source_height` は PDF を含め常に raster のピクセル寸法であり、注釈・クリック判定・
編集座標等に使う。両者は別の軸で、相互の fallback として単位を混ぜない。

縦連続表示も同じ PDF レイアウト寸法を `vertical_reading_base_size` の最優先 source とする。
2026-08-12 の実ログでは page 0 の final raster が `4540×6920` へ切り替わった frame に、直前の
cached thumbnail `226×422` の比 (`0.5355`) が連続表示だけに残り、ページ矩形が
`561.1×1048` (`0.5354`) になっていた。page box は `468.600×714.360 pt` (`0.6560`) であり、
同じ final raster の比 (`0.6561`) と一致する。原因は paged path の canonical layout owner を
連続表示が迂回し、かつ `Evicted` で PDF layout metadata を失えたことだった。現在は同一の
`layout_dims` を paged / continuous の両方へ供給し、generation-scoped cache が texture eviction
後も保持する。

ただし低解像度 raster 自体をそのページ矩形へ非等方に引き伸ばしてはならない。rendition の実描画
矩形は texture 自身の縦横比を保つ contain fit とし、ページ矩形の中央へ置く。完成 texture だけは
従来どおりページ矩形全体へ描く。整数丸め差に相当する最大数 pixel の余白は許容し、低解像度の
中身を伸縮して settle 時に位置が動くことを避ける。単一 / 見開きとも同じ規則を使い、見開きの
source 選択と差し替えは引き続き表示単位で原子的に行う。

縦・横の連結読みは通常のスクロール操作では通過表示の対象外 (スクロールは実体化済みのページを
見せるだけなので、キーごとの実体化コストが元から出ない)。ただしフォルダ横断 navigation sequence
が `Ready(Rendition)` の target unit を連結 renderer へ渡す場合は例外である。renderer は target
全ページの rendition を all-or-none で描画 source にし、実際に draw command を作れたページだけを
提示 source set として返す。複数 unit が同時に可視でも、target page set が完全一致した場合だけ
sequence を retire する。ページ矩形の canonical layout 契約は paged / continuous で共通である。

#### 2.5.5 トレース不変条件 (機械判定)

現行実装は上の規則を `fs/page_turn_ready` / `fs/page_turn_decision` の並びとして計装し、
`python scripts/analyze_perf.py <jsonl> page-turn --check` で検査する。event が無いログを成功扱い
しないため、本番計測では `checked bursts>0` も完了条件に含める。
**規則を変えたらこの不変条件も同時に更新する。**

`fs/page_turn_decision` は、元画像ホールドの入力経路を信号自体で gate せず調査できるよう、
`original_preview_active`、`context_blocker`、`right_ctrl_held`、`left_ctrl_held` を全 event に持つ。
`context_blocker` は blocker が無ければ null、左右 Ctrl は focused current-level permit が無ければ
どちらも null とする。Ctrl は egui modifiers ではなく非消費の OS 物理レベルを記録する。

| ID | 不変条件 | 対応する要件 |
| --- | --- | --- |
| I1 | 1 バースト内で、同じ idx の `source` が `final_composite` → `thumbnail` へ戻らない | R2 |
| I2 | 1 バースト内で `source` がページをまたいで混ざらない | R2 |
| I3 | 受理した target unit の全 idx が、次 target の受理前に同じ final display-unit source として 1 回以上出る | R1 |
| I4 | key-up より後に処理された auto-repeat は入力端で consume-only にし追加 target を発火させない。一方、受理済み landing target は取消さず、その rendition を提示した後に同 idx の `materialized` が現れる | R4 |
| I5 | unresolved rendition sequence で `paint_source=materialized` なら `ui_work_admission=navigation_target_materialization_only` とし、target の producer / consumer を許可する。`paint_source=pass_through` なら `ui_work_admission=deferred` とし、target を含む full-resolution work を保留する。どちらも target 外の先読みは保留する | R3 |

`fs/paint` は単一ページでは既存の `(idx, texture)` producer が rendition も記録し、見開きでは
holdover を含む最終表示単位の解決後に同じ page list から記録する。`source=thumbnail` が通過
rendition、`source=final_composite` が完成画像である。前表示単位と同じ `(idx, texture)` は重複して
出さない。event の `x` / `y` / `w` / `h` は実際に texture を描いた矩形、`texture_w` /
`texture_h` は回転後 texture 寸法、`scale_x` / `scale_y` はその描画倍率である。これにより
`fs/paint` の表示ページ数と `fs/page_turn_ready` を直接照合でき、同じ idx の rendition → final で
非等方伸縮が混入していないかもログから確認できる。

開始時から境界で、1 回も display unit が変わらない hold は ready signature も変えないため、
その押下に対応する `fs/page_turn_ready` を 1 件も出さない。

endpoint 例外は設けない。active burst が端へ達して次の repeat が no-op になった時点で、physical
key がまだ held でも burst を閉じて着地点を materialize する。したがって I4 は端でも必ず要求する。
通常の移動中 burst の settle 窓は従来どおり key-up にアンカーし、burst 内の最後には要求しない。

2026-08-12 の実機報告にあった「戻り hold の端で黒背景 + 中央の白文字」は、
`ColorizeDisplayUnit` の「カラー化中…」ではなかった。active burst の最後は rendition を描いた後、
navigation 解決で `Idle` になる。次 frame の `prepare_fullscreen_state` は materialized を選ぶが、
着地点の final composite はまだ無いため `resolve_fs_processed_texture=None` となり、R2 のため未処理
thumbnail も抑止する。残る `draw_fs_image` の終端が黒背景 + 中央「読込中...」だった。
pass-through open は旧 page-transition holdover を消しているので、この frame に
`ColorizeDisplayUnit` と「カラー化中…」は存在しない。

修正後は materialized settle も同じ着地点 rendition を final 到着まで使う。見開きは片側だけ final に
せず、片側でも final-effect 待ちなら両ページとも rendition、両 final が揃った frame で原子的に
差し替える。開始時から端の no-op は既存 final が解決済みなので表示 source を一切変えない。
通常ページ移動は旧ページ holdover を作らず、「カラー化中…」は同一ページの PDF 再レンダ等で
`FinalEffectSourceReload` が 400ms を超えた場合だけ表示する。その表示開始は
`fs/colorize_wait_indicator` (`reason=source_reload`) に一度だけ記録する。

#### 2.5.6 やり直しの作業順 (2026-08-11 に更新)

§2.5.1.1 で意図が確定したので、次版は「色を合わせた低解像度を出す」ことに集約する。

1. ~~サムネイル寸法での色処理コストを実測する~~ → **実測済み (§2.5.3.1)。約 2ms / 枚で、
   UI スレッド同期実行が可能。**
2. ~~通過表示に色処理を適用する~~ → 色調補正 → カラー化 → LUT を UI スレッドで同期実行する
   context-local rendition cache を実装済み。
3. ~~通過中はフルサイズのアップロードとデコードを始めない~~ → 実遷移バーストが active の間は
   producer 起動と UI upload 回収を保留し、既存 producer を cancel する。
4. ~~止まったページだけ実体化する~~ → key-up 後も受理済み landing rendition の提示までは
   sequence を維持し、提示後の次 frame で current display unit を eager path へ戻す。入力 repeat は
   queue せず、処理時点ですでに release 済みなら入力端で consume-only にするため、release 後に
   別 target が自動で進むことはない。first keydown は同一 frame の key-up でも 1 回発火する。
5. ~~未キャッシュ target を実際に生成する~~ → sequence page set を keep-set に固定し、priority
   thumbnail request と PDF HighNormal promotion を発行する。terminal failure は typed outcome として
   次 target または landing full materialization へ進める。

単一ページの `aimodel1` / 4.1MP 実 ZIP と見開きの実ログ gate は利用者実測で合格済み。境界 no-op、
rendition の表示矩形、PDF + `MonochromeOnly` は上記の回帰テストと再実測で維持する。

##### ボトルネックは 1 つではない (2026-08-11 実測)

計測対象によって支配項が違う。**どちらか一方だけ直しても足りない。**

| 対象 | ページの画素数 | 合成コスト | ページ間隔 | 追いつくか |
| --- | --- | --- | --- | --- |
| 実物 ZIP (1792×2304 PNG、664 枚) | 4.1MP | 4.7ms | **33.5ms** | materialized 単一は ❌ **29 / 174** |
| 合成 fixture JPEG | 11.8MP | 13.9ms | **35ms** | ✅ 追いつく |
| 合成 fixture JPEG | 24.0MP | 28.2ms | **67ms** | ❌ 追いつかない |
| 実物の本 (`-Setup`、カラー化 + AI 有効) | 小さい | — | — | デコード 72.9ms + AI タイル 57.5ms×44 が支配。upload は **1.38ms** |

合成 fixture 単体のコストは画素数にほぼ線形だったが、実アプリでは decode / upload / final-effect
回収を含むため、4.1MP ZIP でも materialized 経路は追いつかなかった。したがって合成 fixture の
11.8〜24MP だけから実用上の閾値を決めてはいけない。

実物の本では upload は問題ですらなかった (ページが小さいため)。合成 fixture では upload が
支配的だった。**共通して効くのは「通過するページでフル解像度の作業を始めない」こと**であって、
upload だけを速くすることではない。

**AI アップスケールは通過中に走らせない** (利用者判断 2026-08-11)。止まったページで推論して
遅れて差し替えてよい。57.5ms/タイルは通過表示の予算に対して桁違いで、通り過ぎる絵に拡大結果は
要らない。

**カラー化 Disabled + 色調補正のみ**のページは、`colorize_display_requires_final_effect`
が `false` を返す (`is_color_identity` を見ているのが `MonochromeOnly` の枝の中だけのため)。
上の 2 を入れれば自然に解消する。

なお **デコードも律速**である。24MP JPEG の実測でデコードは 172ms/ページで、ページ間隔 67ms は
ワーカー並列でようやく成立している。UI スレッドの 28ms を消しても 34ms まで落ちるとは限らない。
通過するページのデコードを始めないこと (上の 3) が効くはず。

---

## 3. 補正・AI・カラー化・ポストフィルタキャッシュと再描画

### 3.0 処理順序 (= 最終表示への適用順)

ユーザーが表示で見る最終画像は、v1.1.0 以降は以下の順序で各レイヤが重ねがけされる:

```
1. fs_cache (= 生デコード結果)
   ↓
2. 消しゴム (MI-GAN inpaint)
   → ESC / E / × ボタンで確定したとき MI-GAN がマスク領域を補完
   → erase_result_cache[idx,input_gen,mask_gen]
   fs_cache は raw decode 専用で、消しゴム確定結果を書き戻さない。
   ↓
3. 補正レイヤー (local-adjust-core)
   → local_adjust_cache[idx,input_gen,erase_mask_gen,local_gen]
   消しゴム結果があればそれを、なければ raw を入力にして非同期 worker で合成する。
   未生成または stale の間は古い補正レイヤー結果を表示せず、下位レイヤの画像を表示する。
   raw thumbnail 自体は書き換えないが、永続 edit preview 経路では補正レイヤーを含む
   編集結果を一覧へ表示できる。
   ↓
4. 隠蔽加工 (モザイク / 白塗り / 黒塗り / ぼかし)
   → conceal_cache[idx, generation] (= local_adjust_cache / erase_result_cache / raw をベースに合成)
   ↓
5. edit_result_cache[EditResultKey]
   ここまでが source 解像度の edit pipeline。crop / AdjustParams / AI / post_filter は含めない。
   ↓
6. 色補正 (色温度・彩度・コントラスト・露出など)
   → apply_adjustments_fast(edit_result)
   ↓
7. AI アップスケール / デノイズ (Real-ESRGAN / Real-CUGAN / NMKD-Siax / 1x denoise)
   → final_ai_cache[FinalAiKey] (pixels + used_upscale)。未完了中は色補正後の画像を暫定表示する。
   ↓
8. スマートシャープ (シャープ化スライダー 0..=100、輪郭中心の最終段シャープ)
   → AI アップスケール出力には掛からない (固定動作)。デノイズのみ / AI なしには掛かる。
     詳細は preset-and-adjustment.md §2.7
   ↓
9. カラー化
   → 近モノクロ判定、対象画像だけの濃度・コントラスト自動補正、必要なら
     スクリーントーン濃淡変換、カスタム階調 LUT
   → `final_effect_pending` worker。カラー化前の provisional texture は公開しない。
     前後ページは final composite まで先読みし、完成済みならページ移動直後からカラー表示する。
     先読みが未完了なら、ページ単位表示では移動先の色忠実な低解像度 rendition を表示し、
     対象 unit の全ページが揃った frame で完成画像へ一括で切り替える。
     PDF の Z ズーム再描画など、同じ表示ページの source 解像度が更新される場合は、旧世代の
     完成済み texture を display-only holdover に退避してから live cache を無効化し、
     新世代の final composite 完成時に置き換える。AI 待ちの incomplete composite は
     final AI 到着後も同一 key の cache entry として維持し、AI 後の完成結果で直接上書きする。
     `complete=true` の final だけが source-reload holdover / nav lock を解放する。
     `FinalEffectSourceReload` が 400ms 続いた場合だけ、中央の暗いラウンド矩形に
     「カラー化中…」を表示する。通常ページ送りとフォルダ移動の `FolderNavigation` では表示しない。
   ↓
10. Creative 3D LUT
   → 環境設定で登録した `.cube` を適用量付きで最終表示へ適用する。
     入力色空間変換ではなく表示用ルック専用。カラー化と同じ `final_effect_pending`
     worker で処理し、サムネイルには反映しない。
   ↓
11. ポストフィルタ (CRT エミュレート / 減色 / モノクロ / 複合エフェクト)
   → final_composite_cache[FinalCompositeKey]
   ↓
12. capture / export 時だけ crop で実切り出し
```

**ユーザー向けの言い換え**: 消しゴム / 補正レイヤー / モザイク加工は元画像の
解像度で先に確定し、その後に明るさ・色・AI 拡大・効果フィルタが乗る。crop は
通常表示では範囲外を暗転するだけで、capture / export の最終段で初めて切り出す。
そのためアップスケール ON/OFF や補正スライダー変更で編集マスクの解像度は変わらない。

製本、単枚 / 一括 Ctrl+E、外部ツールは表示 cache を完成画素の入力にせず、
`CompositeSource` と `BakedEditSnapshot` から worker の headless compositor が復元する。
順序は raw → erase → local_adjust → conceal → adjustment → AI → smart sharpen → colorize →
Creative LUT → post-filter → comic → rotation → export crop。各機能の `BakeStage` はこの順序を
変えず、どこまで進むかだけを決める。製本で編集が 1 つも無い File / ZIP entry は従来どおり
この経路へ入れず byte copy を維持する。

Ctrl+E の隠蔽プリセット出力 (`_1`〜`_4`) も、`_0` と同じ source・編集 snapshot・選択段を
`books::compose_export_entry` へ渡す。`_0` は override を付けず snapshot の現在の隠蔽 preset を
1 回だけ適用し、`_1`〜`_4` はその preset だけを差し替える。隠蔽以外の入力と段は同一なので、
AI を含む段では各エントリの合成中に AI も実行する。焼き込み済み画素へマスクを重ねないため、
隠蔽が二重に掛からない。

### 3.1 詳細

詳細は [preset-and-adjustment.md](preset-and-adjustment.md) に譲る。ここでは要点のみ:

- **補正 (adjustment)**: `ensure_final_composite_texture` が `edit_result_cache` のピクセルへ
  `apply_adjustments_fast` を適用する。色系パラメータ変更では `edit_result_cache` を保持し、
  `final_ai_cache` / `final_composite_cache` だけを落とす。
- **スマートシャープ (シャープ化)**: final AI の後・カラー化の前に
  `apply_final_smart_sharpen` を適用する。AI アップスケール出力には掛からない
  (固定動作)。サムネイルには反映しない。詳細は
  [preset-and-adjustment.md §2.7](preset-and-adjustment.md)。
- **カラー化**: スマートシャープ後・ポストフィルタ前に適用する。カスタム色、近モノクロ判定、
  対象画像だけの着色前自動レベル補正、スクリーントーン濃淡変換を含む。重い画素処理は
  viewer context 所有の worker へ送り、
  stale 結果を `FinalCompositeKey` と items 世代で拒否する。AI 先読みと同じ前後枚数の
  `final_composite_cache` を背景で 1 枚ずつ作る。先読み中は provisional texture を upload
  せず、同ページが表示対象になったときは進行中 job を昇格して再利用する。通常ページ送りでは
  完成まで移動先の色忠実 rendition を保持し、生画像・未処理サムネイルへフォールバックしない。
  同一ページの source reload holdover が 400ms を超えた場合だけ「カラー化中…」を表示する。
  `open_fullscreen` はページ入場だけでは final composite / worker を無効化しない。カタログの
  サムネイル自体は書き換えない。連結読みは `FinalEffectSourceReload` を通らないため、
  このインジケータの対象外とする。
- **Creative LUT**: カラー化後・ポストフィルタ前に適用する。登録済み `.cube` の parse 済み
  table を worker へ `Arc` で渡し、ファイル I/O は UI thread と final-effect worker の外で行う。
  LUT の選択・適用量は final composite key に含め、変更時も edit / final AI cache は保持する。
- **ポストフィルタ**: AI の後段で CPU 処理 (CRT/減色/複合)。rayon 並列化済み。
  `PostFilter::Nearest` のみ NEAREST サンプラー、それ以外は LINEAR でアップロードする。
- **消しゴム/隠蔽加工/分析モード中の一時バイパス**: `App::post_filter_bypassed = true` の間は
  final composite の key からカラー化・Creative LUT・post-filter を外し、表示用最終段だけを切り替える。
  edit 系 generation は進めない。
- **補正レイヤー**: `local-adjust-render` worker で `local-adjust-core` を適用し、
  `local_adjust_cache` に載せる。生成中は古い補正レイヤー結果を使わず、
  `erase_result_cache > fs_cache` の下位画像を表示する。
  Ctrl+E / キャプチャ保存では、補正レイヤーが有効なページは `local_adjust_cache` 完了後だけ
  出力対象にする。
  ブラシ stroke 中は 150ms の idle まで重い再合成を遅延し、release 時に確定世代を進める。
  `Repair` の周囲パッチ探索 / クローン / 色とテクスチャのなじみ処理もこの worker で行い、
  決定した結果ピクセルは専用レイヤーに保持せず、パラメータとマスクから再生成する。Repair の
  マスク境界なじませは、ぼかす前のマスクで修復元探索 / テクスチャ生成を行い、ぼかしたマスクは
  最終合成 alpha にだけ使うため、なじませ幅を変えても参照パッチ自体は変化しない。
- **AI アップスケール/デノイズ**: final pipeline の別スレッドで推論。完了時に
  `final_ai_cache` に pixels と `used_upscale` を格納し、未完了の
  `final_composite_cache` を表示用に残したまま同一 key で再合成する。final-effect worker
  完了時の `insert` が未完了 entry を上書きするため、再合成中も直前のカラー化済み表示が
  継続する。
- **共有 final composite 層**: tone / smart sharpen / colorize / Creative LUT / post_filter の
  CPU 実行順は `final_composite::FinalCompositePlan` と `execute_final_composite` が App / remote
  で共有する。final AI は tone 後・smart sharpen 前の独立した非同期 cache 層なので plan には
  含めず、adapter が AI 前後の source と `used_upscale` 解決を所有する。raw / edit result の
  source 選択も materialize と世代管理を持つ adapter の責務で、共有 executor は選択済み pixels
  だけを受け取る。
- **AI 結果の保持 LRU**: 完了済みの final AI pixels は `retained_final_ai_cache` にも
  `metadata_cache_key(idx) + edit_size + color_ai_hash + bg` で保持する。entry には
  smart sharpen 判定用の `used_upscale` も保持する。通常の
  `final_ai_cache` は fullscreen keep-set や `close_fullscreen()` で消えるが、保持 LRU は
  `retained_final_ai_cache_max_entries` / `retained_final_ai_cache_max_mib` の範囲で残る。
  再表示時は `final_ai_cache` miss の後に保持 LRU を参照し、ヒットした entry を live cache
  へ戻してから final composite を再生成する。AI 入力が変わる編集では該当ページの保持分も
  破棄し、post_filter / smart_sharpen だけの変更では保持する。fullscreen close / reopen
  に伴う同じ item の `fs_cache` 再ロードでは live cache だけを作り直し、保持 LRU は残す。
  PDF ページの display final AI は、巨大ページで完了前に session close / keep-set eviction
  されると LRU へ store できないため、保持 LRU が有効な場合だけ最大 1 件を
  `retained_final_ai_orphans` として live pending から外し、cancel せず完走させる。PDF ページは
  汎用 `retained_final_ai_cache` へ重複保存せず、PDF ページ専用の統合スロット
  (`Raster` / `FinalAi` のどちらか 1 つ) へ store する。PDF レンダリングが完了した段階では
  `Raster` として保持し、その後 final AI が完了したら同じページスロットを `FinalAi` に昇格して
  raster pixels を解放する。この PDF ページスロットと汎用保持 LRU は同じ枚数 / MiB 予算で
  合算 LRU 退去される。再表示時に `FinalAi` が条件一致すれば PDF worker を起こさず、AI pixels
  から `final_composite_cache` を直接復元する。`Raster` lookup は固定 4096 ではなく、現在の
  display target と保持 raster の縦横比から必要長辺を再計算し、±10% 内だけを再利用する
  （細長いページの整数寸法は上記の共有 renderer 規則でも照合する）。毎フレームの
  `fs_page_load_state` が行う `FinalAi` 照合は、非 PDF を先に除外し、保持スロットに
  `FinalAi` がない場合も source key の normalize / String 生成より先に終了する。
  既存 LRU の設定上限は合算 20 件なので、種別だけの走査を選び、件数カウンタや
  別索引を追加しない。`FinalAi` がある場合のページ・現在パラメータ照合は維持する。
  外部変更や AI 設定変更で retained epoch が進んでいた場合、その orphan 結果は store 時に捨てる。
  汎用保持 LRU の store / hit /
  miss / skip / evict / clear は `mimageviewer.log` に `[AI] Retained final AI ...`
  として記録する。PDF orphan の開始 / 回収は `[AI] Final AI retained orphan ...` /
  `[AI] Final AI orphan result stored for retained cache only ...` に出る。epoch 不一致や設定無効で
  store しなかった orphan は `skipped for retained cache only` になる。PDF ページ統合スロットの
  `Raster` / `FinalAi` の store / hit / miss / evict / clear は `[PDF] Retained page ...` として
  記録する。
  ヒット時も元画像ロードと final composite の再生成は必要なので、再入場直後に 1 フレーム程度
  AI 前の暫定表示が出ることはある。ただし PDF の `FinalAi` hit は raw PDF レンダリングを待たずに
  final composite を復元するため、ラスタ化待ちの低解像度表示を避けられる。外部アプリによる現在フォルダの実ディスク変更を
  signature 差分で検出した場合は、同じ path / 同じ寸法の差し替えに備えて保持 LRU を全クリアする。
- **AI 先読み (新パイプライン)**: `App::prefetch_final_ai` がフルスクリーン更新ループ
  終盤で呼ばれ、現在ページの `final_ai_pending` (cancel フラグ除く) が空のときだけ
  隣接ページの `final_ai` 推論を 1 件 spawn する。`ai_prefetch_targets` で前後の
  対象 idx を `ai_upscale_prefetch_forward / back` 件まで取得する。同じ更新入口から
  カラー化の final composite 先読みも行い、AI 使用ページは `final_ai_cache` 完成後、
  AI 不使用ページは edit pixels 準備後に 1 件ずつ final-effect worker へ送る。
  対象列はアップスケール / ノイズ除去の final AI と、カラー化 / Creative LUT の
  final effect で共通である。ページ単位の見開きでは表示単位の読み順先頭が現在 idx になり、
  同じ見開きの相方が前方 1 枚目を使う。既定の後方 2 / 前方 3 は、表紙なしの 2 ページ見開きで
  直前・直後の表示単位を両ページとも範囲へ含める。v3.7.0 では旧既定の前方 2 だけを版境界で
  一度 3 へ移行し、保存済みの後方値は変更しない。表紙ありモードの末尾から先頭への循環は
  この範囲計算へ含めない。
  未リリース§1.341のlocal pagedアニメーションONでは、source/AI先読みとも通常navigationの
  表示unitから対象を選び、予算境界の組を分割しない。現在の相方は予算を消費せず、末尾unitの
  実画像である補助表紙も同unitの需要へ含める。source/AI双方の需要をlive keepに合流し、
  indicatorの方向もunit位置で示す。Retained PDF FinalAiのみ残る隣頁は、既存final-effect workerと
  paced pollでGPU素材を準備する（AI先読みゼロでも既存結果の再利用は可能）。Retained pageの
  entries/bytes共有LRU上限は維持し、受理までに素材が間に合わなければinstantとする。
  OFF・連結・Split・Remoteの既存範囲計算は変更しない。詳細はページめくり設計§26.8。
  ⚠️ **退行注意**: Pipeline P1 リファクタ (be05cfef) で旧 `prefetch_ai_upscale` が
  dead code 化され、新版が未実装のまま 1 リリース過ごした。`App::update` の
  「フルスクリーン work セクション」(= `// AI 先読み (新パイプライン)` コメント) を
  消すと再発するため、リファクタ時は呼び出し元の存在を要確認。
- **元画像プレビュー**: 割り当てた ModifierHold (既定は右 Ctrl) の間だけ描画時のテクスチャ選択を
  raw 専用の `fs_cache` に切り替える。DB・補正設定・AI queue は変更しない。
- **何かを変えたら正しいキャッシュをクリア**:
  - 色調パラメータ / AI 変更 → `final_ai_cache` / `final_composite_cache` をクリア
    (`edit_result_cache` は保持)。post_filter / シャープ化**のみ**の変更は
    `final_composite_cache` だけクリアして final AI を保持する
    (preset-and-adjustment.md §4)
  - 消しゴム / 補正レイヤー / 隠蔽加工変更 → source 解像度の edit cache と final cache をクリア。
    特に全ページ共通の隠蔽パラメータ変更は、edit 世代を安定キーに含めない
    `retained_final_ai_cache` も全失効し、変更前の AI 結果の復元を防ぐ
  - crop 変更 → `edit_result_cache` / `final_ai_cache` / `final_composite_cache` は保持。
    crop 済み下地と注釈を保存する永続 edit preview WebP だけを非同期失効し、
    capture / export の切り出し範囲を更新する
  - 消しゴム/隠蔽加工/分析モード入出 → 該当 idx の final cache のみクリア (bypass 切替のため)
  - フォルダ切替 → edit / final / thumb 系 cache をグローバルクリア
  - 回転変更 → **キャッシュはクリアしない** (GPU 行列で回すため)

#### 無変換の静止画テクスチャ共有（2026-10-17）

2026-10-28（紙めくり計画 §28、段階 A）: exact な隠蔽・局所補正・消しゴムの累積段も
共有候補に含める。選択は通常表示とめくり受付が同じ resident-only helper を使い、
受付の edit_result だけは exact key と texture を必要とする（通常表示の holdover 一致は維持）。
共有には現在 key・画素 Arc・item identity・寸法・実 LINEAR options の一致が必要で、
NEAREST 段は共有しない。共有元の accepted display_basis を継承し、各段の実 options の
mip 有無を VRAM 会計へ渡す。Native 先回りも実際に共有可能な段の material を返す。

閲覧用 crop の暗転・枠は、基準寸法付きの crop 値を持つ prepared page が描く。
通常 flat・curl 表裏・canonical は同じ leaf 投影と可視 polygon を使う。実二頁では抑止し、
見開き設定中の片頁は描く。crop overlay は編集ハンドルと入力だけを所有する。
受付はメモリだけを読み、legacy 基準寸法の永続採用は従来の通常表示 owner に残す。
保持表示は capture-time crop を保持し、別フォルダで同じ idx を使っても新頁の crop を参照しない。

Z照準／拡大は画像・pixel grid・照準暗転／照準枠を先に描き、実際のZ transformを使って
同じflat crop ownerを呼ぶ。mainでは従来のcrop合成段（canvas overlayの後、crop handlesの前）、
単独Z描画ではそのcompositeの最後に呼ぶ。crop暗転と枠を照準の下へ移さず、二度描かない。
通常flatも実際にbindしたprepared pageのcrop／transform／clipをフレーム内の描画値として返し、
pixel gridとcanvas overlayの後、handlesの前で同じownerが描く。受付・snapshotにはcrop bindingを残す。
curlはこの後描画値を生成せず、各faceに投影する。canonicalの最初のflatから同じ後描画段を使い、
演出OFF／非eligible／取消のflatも同じ境界を通す。捕捉表示は従来どおり捕捉cropを面と一緒に描く。

準備済み比較（固定画像・ワイプ・差分）も、合成画像の実 transform で同じ flat crop owner を
一度だけ呼ぶ。WipeはCPU／shaderともcomposite→操作表示→canvas overlay→crop暗転／枠の旧順。
比較準備中は通常ページのownerを使い、編集overlayは引き続きハンドルだけを描く。
legacy crop のメモリ内基準解決は受付と通常表示で共有し、FullSource サムネイルの原寸情報、
情報がない場合の resident raster も含む。受付後の品質更新では既存の受付内容が crop を保持する。
矩形が収まらず基準を未確定のまま受け付けた場合もその値を保持し、paint の clamp は transform に
委ねる。基準が解決できた場合の永続採用は通常表示でだけ行う。

`ensure_edit_result_pixels` は新規組立て時と CPU-only entry の表示昇格時の両方で、
upload の枚数制限より先に現在 context の `fs_cache` の静止画 source を調べる。
現在の `EditResultKey`（item identity と各世代を含む）が一致し、source/edit pixels が
`Arc::ptr_eq`、texture 寸法が pixels 寸法と一致、options が
`DISPLAY_IMAGE_TEXTURE_OPTIONS`（LINEAR＋完全 mip）と一致し、既存 display identity gate を
通る場合だけ `source.tex.clone()` を edit entry に保存する。画素比較・I/O・新しい待機状態はない。
handle 共有は upload ではないので `last_edit_materialize_upload_frame` を消費せず、見開きの
両頁も同 frame に共有できる。不一致時は従来の paced upload を行う。CPU-only の先読み入口は
texture を確保せず、表示時まで `None` を維持する。

final の key・pixels・source_size・canonical_dims・complete と resolver の優先順位は維持する。
既存の `Arc::ptr_eq(post_filtered, edit_pixels)` による edit→final の共有につなぐため、
無補正では source/edit/final が同じ TextureId になる。共有自体は完成の証拠ではなく、
AI 待ち等の `complete=false` を昇格させない。raw preview、animated frame、別 Arc の編集結果、
消しゴム用の透過 black flatten、寸法/options 不一致は共有しない。Nearest 等の final sampler は
従来の final 処理が解決する。注釈や Native Lanczos 出力の生成・所有境界も変更しない。

共有 allocation は immutable とし、各 stage の entry と失効規則は省略しない。source 退去後も
edit/final の clone は生存するが、raw cache がある証明には使わない。通常再読込は既存の input 世代更新で
派生 cache を失効させ、兄弟 context には作用しない。VRAM は既存の TextureId dedup で一回だけ数え、
最後の handle owner の drop で解放する。演出 ON/OFF・初回表示とも共通の全画面表示経路へ適用する。


---

## 4. 分岐チェックリスト

修正時、以下の観点で漏れが出やすい。どれかを触るなら全部を確認する:

### 4.1 画像種別分岐

| 処理 | Image | ZipImage | PdfPage | Video |
| --- | --- | --- | --- | --- |
| サムネイルデコード | image/turbojpeg/libavif/WIC | 共通 bytes decode（AVIF=libavif） | PDFium ワーカー | Shell API (別スレッド) |
| フルスクリーンデコード | 同上 + EXIF + 動画判定 | bytes から decode + EXIF | PDFium で実 viewport×ppp に fit (raster は native 上限) | なし (サムネのみ) |
| EXIF Orientation | ✅ path から読む | ✅ bytes から読む | ❌ | — |
| アニメーション | GIF/APNG/WebP ✅ | GIF/APNG/WebP ✅ | ❌ | — |
| 回転 (rotation_db) | ✅ | ✅ (path+entry キー) | ✅ (path+page キー) | — |
| プリセット補正 | ✅ | ✅ | ✅ | — |
| AI アップスケール | ✅ | ✅ | ✅ | — |
| 消しゴム (inpaint) | ✅ | ✅ | ✅ | — |

#### 4.1.1 アニメーションの先読みと現ページ昇格

fullscreen の canonical decode は `AnimationPolicy` を正本にする。現ページと remote AI の
形式判定は `FullFrames`、前後の先読みは GIF / APNG / WebP と通常ファイル / archive entry の
組み合わせに関係なく `FirstFrameOnly` を使う。サムネイル生成経路はこの方針の対象外。

`FirstFrameOnly` で複数フレーム形式と確認した static entry は、その事実を
`StaticAnimationState::FirstFrameOnly(format)` として `fs_cache` に保持する。ページが現表示の
`fullscreen_idx` になったら、第1フレームを描いたまま `AnimationPromotion` として全フレームを
非同期 decode し、完了時に同じ idx / items generation の `Animated` entry へ原子的に差し替える。
第1フレームは FullFrames decoder の先頭フレームと同じ向き・画素を保ち、差し替えで描画元を
サムネイル等へ切り替えない。

昇格 worker / upload backlog は load purpose、items generation、target idx を保持する。別ページへ
移動した昇格は cancel し、完了が遅れても target idx が現在ページでなければ適用しない。昇格失敗は
`Failed` entry にせず第1フレームを `PromotionFailed` の static として残す。進行表示は、現ページの
全フレーム展開を所有する状態から導く。一覧から直接開いた初回 `Display` はそれだけでは表示条件に
せず、worker が GIF の2枚目、APNG の `is_apng()`、Animated WebP の `has_animation()` で複数
フレーム形式を確認した非終端信号が届いた後だけ対象にする。先読み第1フレームからの
`AnimationPromotion` は既に形式確認済みなので、従来どおり typed purpose の開始時刻を使う。
どちらも確認 / 開始から150ms以上続いたときだけ右上3段目へ表示する。この150msは競合を吸収する
時間窓ではなく、短時間処理を提示するかどうかだけの UI ゲートである。

### 4.2 サムネイル / フルスクリーンの整合性

- サムネイルに適用する変換を増やすなら、フルスクリーン側も同じ処理が走っているか確認
- 逆も同様。サムネは**色調のみ**を適用する (§1.5)。ポストフィルタ / AI は意図的に非対象。
  フルスクリーンでポストフィルタ / AI を掛けても、サムネは色調止まり — ユーザの
  「ざっくり一覧で雰囲気をつかむ」用途に揃えた割り切り。

### 4.3 キャンセル安全性

ワーカー内部のループは以下を頻繁にチェック:

- `cancel_token` (フォルダ切替時に true になる)
- `keep_range` (自分の idx が範囲外なら結果を捨てる)

新しいワーカーを追加するときは同じパターンに従う。詳細は [async-architecture.md](async-architecture.md)。

### F11での固定表示の実効値 (§1.344)

保存値からの予約とHUD描画は `ResolvedViewerChrome` を共通に使用する。`ViewerChromeSurface` は
projected contextのwindow bindingとexact active hostのapplied borderlessから導出し、mainやsiblingへ転用しない。
通常content・holdover・navigation gapは同じresolverで上・下・右の予約を解放する。
strip表示選択とresource ownerは変更せず、一時表示のrectだけをhit / sinkへ渡す。
[設計](fullscreen-locked-chrome-suppression-plan.md) §4・5を参照。


### §1.341 Pairの移動背（2026-10-11利用者option A）

[page-turn plan §23](page-turn-animation-plan.md)が固定静止面の2%loss省略を置換する。
leafと同じsmooth progressで背をold→newへ移し、旧Tは旧背から、新Sは新背からのdeltaを自身のrectへ平行移動する。
UV/寸法は保持し、reveal gate・終端の差分退役・shadow/visibilityは移動後のgeometryを消費する。
projectionのdestination clipはpainter scissorとtransform viewportの交差で、任意のtrim有無をcanvas変更と誤認しない。
初回incomingが不可視でもvalid Pairを開始し、first_visibleは実面積、完了は全canonical flat採用で保証する。
有限pin枠はfullscreenの10%（固定128 MiB cap撤去）、VRAM無制限時は1 GiB。実測737.47 MiB/準備後推計756.02 MiBが根拠で、会計/dedup/leaseと低VRAM・memory_pressure gateを維持する。


### §1.341 見開きの横長一画像とVacant（2026-10-12）

[page-turn plan §24](page-turn-animation-plan.md)により、見開きで単独表示する横長は通常paint_rect
の中央を背にして二つの投影faceへ正規化する。sourceの回転後サイズで判定し、quarter rotationを
適用済みのquad UV/underlay UVを表示左右へ分ける。追加decode/upload・texture・navigation stepは無い。
Pair↔横長・横長↔横長は同じBookLeaf/移動背時計で着地し、形状由来のgap0↔Pair gapを扱う。
単頁表示の横長は従来の一枚。通常の縦長singletonは既存のCenter/Left/Right rectを保持し、
空slotは明示Vacant（surface/occurrence無し）。Vacant側のleafは描かず、90度arrival/departureを使う。
提示receiptは横長二面の実面積を一つのsource occurrenceへ合算し、面ごとの保守的short spanを使う。
pin/lease/dedupはactual resourceのまま。OFFの通常layout・page count/seek/historyは変更しない。

### §1.341 共通の黒い紙と両端の実効配置（2026-10-13）

[page-turn plan §26](page-turn-animation-plan.md)が単頁の暫定spine比較を置換する。
old/newの画像rectを覆う共通paperに黒余白を加え、紙端の固定背で一leafをめくる。
画像rect/UV/underlayは保持し、下頁は静止する。紙全体の遮蔽と画像自身のvisibilityを区別する。
黒paddingは新textureやsource occurrenceを作らず、終端は保存背景色の通常canonical描画へ戻る。
local paged見開きのcompositionは演出ON時だけ先頭・末尾配置を実効ONにする。保存値・本別指定、
連結読み・Remoteは保持する。環境設定とlocal pagedの本別HUDは実効値を表示して両端指定を
無効化し、OFFで保存値の表示・操作へ戻る。末尾Navigation＋FrontCoverSupplementの実画像Pairは
通常Pairと同じtexture/role付きreceiptでめくり、白companionの拒否は維持する。
ON/OFFによるgeometry変更では同じpresentation ownerの
旧flat/ready/receiptを失効し、一件のcontinuationを新canonical採用まで保持する。


## 紙めくりD3-4(a)の投機登録境界（2026-10-18）

非共有finalの新規投機登録はnative wgpu ROOTの全retry後のoutput hookで行う。
通常の表示昇格・同期takeover・共有handle採用はlookup前のserviceを維持する。
最終passの通常live paged dispatchと現在runtime/session/binding、当該outer frameのoffer/curl/mandatory canonical活動で排他する。
追加delta/freeは返却済みoutputに `append_texture_delta` で結合して既存配送へ渡す。
ON/pagedのLegacyPrefetchも同じ非表示登録枠に含め、OFF/連結/Split/通常表示は維持する。
詳細は[紙めくり計画](page-turn-animation-plan.md) §27.17.14.10/.11、§27.17.15を参照。

D3-4(b) は通常 Single／Pair と canonical geometry を共有し、採用済み resident 入力だけから
次 composition の Native 需要を frame 限りの値で求める。未配送 backing・cold 回転／auto bbox・
未確定 fit basis は見送り、通常表示は従来の prepare を行う。同じ ROOT 最終 hook で全 live context の
Next final登録→Next Native→全Next成立を再照合し、その後だけPrevious登録→Previous Native→
resident-only Previous CPU開始へ進む（§1.405の限定拡張）。通常 Native key／plan と出力＋一時 allocation 会計を使い、
新規投機GPU仕事はouter frame当たり最大一件とする。Next CPUの通常producer位置は維持する。
出力は通常 `fs_lanczos_cache` に置き、最初の通常 prepare の hit と既存 D2 handoff を使う。
held／canonical receipt／D4 時計へ後付けせず、parked snapshot や単なる mount から開始しない。

2026-10-19 D3-5整理: Native canonical 候補収集・最終再照合は、既存 installed unit token の完全性証拠と
頁→unit 索引を読む。未採用の証拠を全頁走査で補わず、その機会を見送る。slideshow も既存 still-only
topology の採用を必要とする。最新 callback の viewport／clip／ppp と一致する frame 内候補だけを使う。
未実体化の erase／local／conceal・mask と comic の CPU-only 接続は D3-3 として次の版の後へ延期した
（[backlog §1.401](backlog-on-hold.md#1401-ページめくり-d3-3-編集済み注釈付きの頁でも-final-を先回り準備する--d3-の残り-2026-10-19)）。
これらの通常表示・同期 miss は維持する。完了範囲・計測の限界は [計画 §27.17.17](page-turn-animation-plan.md) を参照。
CPU 需要用 resolver に残っていた全頁走査・mixed slideshow 列再構築は、D3-5 で同owner内の派生 record に移す。
visible と同じ枝は既存索引を読み、異なる枝だけcoldに導出する。CPU需要は縮小せず、adoption／producer／keepの
共有prefixで同pass採用する。通常表示token／units／receiptを準備用に置換しない。詳細は同節F1を参照。

§1.405のPreviousは通常unit ordinal−1、既存後方keep内の全occurrenceだけを補完する。
Next CPUの実予約拒否時だけ予約表最大2 unitからparkedを含むownerを訪問し、未昇格Previousの完成結果
（未受信channelも含む）をdropして一度だけ再試行する。Current/Next共有素材・表示昇格・legacy・Unknownは保護する。
Previous Nativeはsmoothing clear/旧source除去/容量pruningによる既存entry除去ゼロを、事前と実行入口で検査する。
上限・D2/D4・通常draw/Next/source/AIの範囲は維持する。詳細と検証状況は[計画 §27.17.21](page-turn-animation-plan.md)。

## 一時通知の表示時間（§1.402）

通知の種類、文字数・指定秒数・動画の既定時間、設定の倍率と例外は
[通知時間ポリシーと棚卸し](notification-duration-policy.md) を正本とする。
App で寿命とnative fadeを確定して渡し、描画側は倍率を再適用しない。
標準ではnativeの既存linger/fadeの不一致、保存完了のegui1.2秒/native1.8秒も維持する。
動画内部で発行する通知だけは、既存 chrome snapshot の選択値から同じ関数で一度確定する。
処理の寿命に連動する進捗表示、ナビゲーション境界の専用ヒントは独立した表示を維持する。

### AVIF の共通デコード境界（§1.407）

`image_decode` は path / bytes の AVIF を内蔵 libavif → WIC → Susie の順に接続する。
他形式の image / TurboJPEG の順序を保ち、呼出元の worker で実行する。
内蔵は RGBA8 straight alpha、最初のフレームのみ。`libavif-sys` のsafe Rust APIがRGB上で
clap → irot → imir を一度だけ適用し、不正 clap は無視する。
`Decoded::orientation = Applied` を canonical / thumbnail / 類似検索 / clipboard へ運び、
後段 EXIF / WIC orientation を重ねない。サムネイルの source_dims はこの変換後の寸法。
WIC / Susie 成功時は既存の orientation 規則を使う（bounded WIC は内部で適用済み）。
背景処理・先読みは dav1d 1 thread、需要のフルサイズは 4 threads。

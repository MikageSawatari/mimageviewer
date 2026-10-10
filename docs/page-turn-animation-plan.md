# ページめくりアニメーション設計案 — backlog §1.341

2026-10-10 / r1 / **利用者回答・独立設計レビュー待ち。製品コードの変更なし。**

参照treeは `next-prefs` / `cb00930011323cacd7088ae70c37835141aad58a`（開始時masterと同じ、作業treeはclean）。
本書の `file:line` はこのtreeでsource inspectionした位置。実行時の確認・新しい性能測定は行っていない。
§1.364は別worktreeで実装中であり、**そのmaster統合後に本項を実装する**。行番号と契約を統合後に再照合する。
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
| navigation | `src/app.rs:11281` の `FsNavigationSequence` が受理したtargetを所有。`src/ui_fullscreen.rs:11411` が開始、`src/ui_fullscreen.rs:11967` がready→実描画のラッチ。本項はsequenceの完了条件を延ばさない |
| 速いページ送り | `src/ui_fullscreen.rs:15492` のpaint source / work admissionは、色忠実renditionとtarget materializationを区別する。`src/ui_fullscreen.rs:15815` のburstは最初の押下をArmed、同じholdの次の移動をActiveにする。physical heldだけで単発をrepeatと扱わない |
| 入力基数 | `docs/keymap-spec.md:254`、`src/ui_fullscreen.rs:30010`。ページ送りはSinglePerFrame、未提示target中の追加repeatは既存どおりdrop、queueしない。first down/upが同一frameでも単発を消さない。効果を80ms待つ入力ロックを足さない |
| 表示単位 | `src/ui_fullscreen.rs:14373` の `SpreadDisplayComposition`、`src/ui_fullscreen.rs:17146` のdisplay demandが正本。単ページ・見開き・補助表紙・端配置・白紙相方のoccurrence/roleをそのまま使う。idx±1で相方や進行方向を推定しない |
| 選ぶ画素 | `src/ui_fullscreen.rs:9787` のdisplay texture解決、`src/ui_fullscreen.rs:11052` のprocessed texture解決と `docs/display-pipeline.md:1169` の優先順位を維持。AI/LUT/カラー化待ちを避けるため未処理rawを出すことは禁止。見開きは既存all-or-none readinessを維持 |
| shared dispatch | `src/ui_fullscreen.rs:36090` のhandlerがpage / jump / folder / closeを扱い、`src/ui_fullscreen.rs:21368` が着地。seekのLatestSeekと順送りSequentialをアニメーション側で統合しない |
| gamepad | `src/app/gamepad_input.rs:6776` はrepeatを受けるが、静止画方向経路へ渡す時点でrepeatの区別が落ち、`src/app/gamepad_input.rs:6945` 付近から通常handlerへ合流する。新効果の適格性に使うなら、この既存factを明示的に渡す必要がある（holdタイマーの新設は不要） |
| GPU・幾何 | `src/gpu_lanczos.rs:167` の `FullscreenPaintResource` を共用。`src/gpu_lanczos.rs:129` のpaint batch lifetimeを維持。単ページ `src/ui_fullscreen.rs:36845`、見開き `src/ui_fullscreen.rs:42352` はprepareしたresourceを描き、`src/ui_fullscreen.rs:10511` のcaptureは実際のunit/fit入力を保持する。`src/displayed_image_transform.rs:732` はquadの各vertexへtintを載せる |
| §1.342 | `src/fullscreen_specified_scale.rs:52` / `src/fullscreen_specified_scale.rs:114` のfit入力・resolver、`src/app.rs:11267` / `src/app.rs:11268` のcaptured fit入力とlimitsを維持。効果のalphaからscale・PDF demand・連結基準を逆算しない |

**現在の速さについての証拠の限界**: docs/display-pipeline.md §2.5には過去の測定記録がある。
`docs/display-pipeline.md:2138` の2026-08-11記録は24MP JPEGのUIコスト28.2msがほぼupload、`docs/display-pipeline.md:2446` 以降は
2026-08-12のFHD ZIP / 4K 165Hz環境で約100ページ/秒という旧測定。ただし`docs/display-pipeline.md:2096` 以降の
2026-08-14規則が優先し、受理unitを必ず一frame提示する。旧100ページ/秒を現HEADの保証・新効果の目標にはしない。
本turnはコードと既存記録の確認だけであり、実装前の現HEAD baseline測定は§8の未実施gateである。

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
parallelなdecode/prefetch/texture保持機構を作らない。bridgeのためのtextureは本項の旧ページ素材に流用しない。

## 3. 簡単な三案と推奨

値は見た目の提案で、実測・利用者採択済みではない。

| 案 | 見た目・仮の時間 | wgpu/egui上の費用とrisk | 次ページが未decode/未uploadの時 |
| --- | --- | --- | --- |
| **A: 短フェードイン（推奨）** | 新unitを初めからalpha **0.85**で提示し、**80ms**で1.0へ。旧ページとのcrossfadeなし | 既存image meshのtintを変えるだけ。旧texture追加保持・offscreen合成・新shaderなし。文字が一瞬少し薄くなるため強さはQ1で判断。元alphaは乗算し、透明PNGを不透明化しない | 演出を付けず既存表示へ。遅れてreadyになったら直ちに通常alpha1.0で提示。decode待ち後の「遅いfade開始」をしない |
| B: 短距離スライド | 新unitだけ水平に最大12 logical points（canvas幅の1%以下）ずらし、80msで着地 | 二重描画は不要だが、実paint pose・hit・loupe・navigator・captureの整合が必要。GPU resampleの可視regionが毎frame変わらない設計も必要。画面端では短時間のclipが生じ得る | Aと同じ。全画面幅のslideはreadyページの大部分を隠すため本案に含めない |
| C: page-curl-lite | 新unitを先に表示し、端の細い帯だけをめくる、120ms | 端の分割mesh/UV/陰影とclip、RTL/白紙/見開き綴じ目を扱う。旧帯を出すなら旧resourceの短期保持とVRAM会計も必要。立体curlより軽いが今回には大きい | 新unitは即表示、帯の素材がなければcurlなし。演出用decode/readbackは禁止 |

通常の「alpha0から新ページを出すfade」や「旧ページを全面表示してから新ページへcrossfade」は、
readyページを不可視のままにする時間を作るため推薦しない。Aは**ready frameから内容を見せる控えめな効果**である。
処理済みtextureの色忠実性は維持し、alphaだけを最後の表示段に掛ける。slideshowにも別の重い効果engineを作らない。
利用者がより強い演出を希望する場合は、内容が見えにくい時間と費用を明記して再設計する。

## 4. 推奨Aの状態削減とready契約

**アニメーションはpresentationだけを所有する。navigation state、読書位置、history、decode、入力受付を所有しない。**
navigationをmodalにする簡素化は、連打やheld browsingを遅くするため不採用。
代わりに「即readyの順送りだけ」「入力で取消」「未readyは演出なし」で待機/rollback/resumeの組み合わせを消す。
設定変更時も窓やreaderをreloadする必要はなく、次の描画で効果を終えるだけで足りる。

提案する最小stateはcontext-localな **`Idle | Animating { accepted_unit_signature, start_at, duration, input_seq }`**。
signatureは受理済みlayoutの小さい識別情報（context/items世代、occurrence/role/slice、既存layoutの識別、描画sourceのID）を使う。
現在の `SpreadDisplayUnitsToken`（`src/ui_fullscreen.rs:14619`）はnav Vecを持つので毎入力deep cloneしない。
既存ownerが供給する識別を再利用し、別layout generation/独自unit cacheは作らない。
`WaitingForDecode`、animation queue、旧texture cache、pending target、resume/catch-up状態は不要。

1. **既存navigationが受理・着地した時だけ**開始候補を作る。入力イベントの到着だけで開始しない。
   同一本・同じreader orderで隣のcanonical表示unitへ進む/戻る、静止ページ、通常のpage単位表示が条件。
   既存非生成のcache参照で必要な全ページの処理済み描画resource・geometryが既に揃う場合だけAnimatingへ。
   準備を判定するために `resolve/ensure` のproducerを追加起動しない。
2. **非readyならIdleのまま**。既存sequence/holdover/loading表示・schedulerが通常どおり進む。
   後からdecode、final、uploadが完成しても新たに開始しない。新しく表示可能になったページを待たせない。
   rendition・thumbnail fallback・補正最終段待ちの暫定sourceでは新効果を付けない（既存表示はそのまま）。
3. 最初の実paintで候補と実際のsource/unitが一致する時だけalpha0.85以上で通常描画。
   受理時刻から進んだ `t` を使い、paint時に時計を巻き戻さない。ready/presentedの通知とsequence解放は
   **この最初のpaintで従来どおり行う**。animation終了を新たなreadinessにしない。
4. alphaは `0.85 + 0.15 * smoothstep(clamp(elapsed/80ms))`。unitの実ページと白紙slotへ同じ係数、
   ページ下地・canvas背景・HUD/seek/navigator/カーソルは従来どおり。size/UV/回転/trim/gap/placementは変えない。
   透明画素・下地とのblendはsnapshotで仕様化する。crossfade用の旧ページを裏に描かない。
5. 新しいviewer操作（キー押下、wheel、クリック、drag/touch、gamepad操作、設定/表示変更）で即Idle。
   操作が既存nav lockで受理されなくても効果は終了可能。取消自体は入力をconsumeせず、元のhandlerへ渡す。
   受動的なhover移動やpollは新操作扱いにしない。down/upが同一frameの単発はtyped consume resultから判定し、
   key-upを新しいページ送りやhold終了後の演出再開に変えない。
6. **演出中に次の操作が来たら、同じ操作では次の演出を始めず**通常表示へ。この判定はreducerの一回の結果で、
   抑制用timer/boolを追加しない。auto-repeat / burst Active / gamepad Repeat / 複数wheel notchは常に新効果なし。
7. source差替え、membership/role/slice/fit/viewport/DPI変更、context終了・不可視化はIdle。
   通常のAI高画質化・PDF再render・補正変更を再フェードさせない。egui同frameの再passで開始/終了を重複させない。
   Animatingの間だけ既存viewportへrepaintを要求し、deadlineで終了。hidden/minimized窓には演出heartbeatを残さない。

UI処理は定数個（現在unitは1〜2実ページ）のsource照合・tint計算のみ。
新しいI/O/DB/readback/CPU合成/GPU upload、全items走査、worker、ロック待ち、sleepを追加しない。
まれな失敗は既定で割り切り（ログ + 通知、保存データ保持）。専用retry・途中演出復元・回復中の回復は作らない。
通常の入力順序・後着source・兄弟窓は「まれ」扱いせず、上の所有/取消規則とtestsで扱う。

## 5. 操作・媒体・方向ごとの挙動（推奨案、回答待ち）

| 操作/状況 | 新効果の規則と保存する既存挙動 |
| --- | --- |
| 単発キー/クリック/タップ/リングの順送り | 即readyの隣接unitだけA。既存KeyAction/固定矢印・tap領域・リングrouterを使う。押下を遅延/再送しない |
| 連打・held key・方向反転 | 演出中の次入力は取消し、その移動は効果なし。repeatと既存burst Activeは常に効果なし。最初の押下はheldだけで除外しない。release後の着地sourceを再演出しない。境界no-opも新効果なし |
| gamepad | 単発digital pressは同じ条件、Repeat/analog連続入力は取消 + 効果なし。既存PadActionKind/dispatchのrepeat factを渡す。frame間の時間から推定しない。spread nudgeは次行の組変更扱い |
| wheel | ページ送りへ確定した単発1notchは同じ条件、複数notch/演出中のwheelは効果なし。既存notch基数/accumulatorを変えない。popup/seek/panelが所有したwheelは背面へ再注入しない |
| seek/ジャンプ | seek bar/strip、Home/End、割合移動、しおり/履歴/検索/類似本jump、Ctrl+フォルダ移動は取消 + 効果なし。到着先がたまたま隣でもjump原因で判定する。LatestSeek/Sequentialの契約は維持 |
| slideshow | 初版は**新効果を手動送りだけに限定**する案。既存の間隔・readiness・自動ページ送り、連結scrollアニメーションは維持。新効果の終端でslideshow_next_atを変えない（Q4） |
| 縦/横連結読み | 新効果なし。wheel/drag/キー/gamepadによるscroll/reanchorと既存slideshow scrollは維持。設定ONで連結のすべてのページをフェードさせない |
| single / spread / 白紙 / 補助表紙 | canonical表示unit全体に一つのalphaと時計。左右独立開始はしない。white slot/補助roleも既存compositionで決まる。片側未readyで新効果のための待ちを足さない |
| split | 同じsourceの左右slice切替は新効果なし、通常のsplit navigationを維持。次sourceへの移動も初版は効果なし。sliceをページ番号と同一視しない |
| RTL / LTR | Aは方向非依存。キー/画面左右から独自deltaを生成しない。Bを選ぶ場合はcanonical進む/戻ると実効bindingから向きを決め、RTLでは進む頁が左側から入る案。白紙/補助roleから向きを推定しない |
| 見開き組/設定変更 | shift、mode/flow/direction、横長寸法後着、page-alone、末尾/端配置、sort/filter/items更新で取消。単ページ↔見開きへの組直しはアニメーションの対象にせず、既存ready/reconcile経路へ戻す |
| zoom/pan/編集/分析/比較/ルーペ/元画像hold/パノラマ | 操作開始で取消し、これらの表示中は新効果なし。既存機能自体を制限しない。capture/export/copyは通常の完成画素を使い、表示tintを焼き込まない |
| GIF/APNG/Animated WebP / native動画・音声 | source自身のanimation/playbackに新効果を重ねない。静止画との境界移動は効果なし。native presenterは別D3D11/DComp所有（`docs/video-architecture.md:2172`）、本項でwgpuへ取り込まない |
| F11/F12・窓切替/close/open | presentation切替自体は新効果なし・実行中の効果は取消。次の独立窓での単発page送りは通常と同じ条件。ホスト生成/placement/focusの待ちに演出を使わない |
| Remote | 初版の新効果はPC側だけ（Q5）。Web独立ownerのPageGroup・fetch/prefetch・latency・RTLを維持。PCの演出stateをIPCへ送らずprotocol変更なし。Remote取得でlocal viewerが閉じる既存経路に取消が従う |

## 6. 多窓・§1.364・後続Gとの境界とファイル所有

presentation stateは `ViewerContextBundle`（`src/app/viewer_context_registry.rs:1059` 付近）へ入れる一つのtyped field。
default/destructure/swap/fork/park/mount/retireを揃える。**通常のmount/depositという実装上の切替では取消しない**。
画面Aを描くためのmountがBの演出を取消すと多窓が成立しない。明示hide/close/parkは当該contextだけ取消、
forkはIdleから開始し、効果や時計を新readerへ複製しない。兄弟のsource/worker/cacheをclear/cancel/drainしない。
UI時刻は共有しても期限・source照合・repaint先はcontext/window binding別。追加VRAM保持はAでゼロ。

detached述語/viewport経路に触れる実装前には、設計担当と独立reviewerで
「contextのpresentationを拡張する構造的変更で、症状パッチではない」と明示合意し、
範囲と理由を `docs/detached-rework-plan.md` §11へ記録する（§2は`docs/detached-rework-plan.md:29`）。本turnでは同プランを編集しない。
Appにdetached用bool、rect捕捉、placement保存、競合吸収のgrace/debounce、追加hidden viewportを作らない。
80msは演出の長さであり、イベントfreshness/所有/競合の判断時間窓ではない。

**§1.364（ラインE、別worktree）**: `C:/home/mimageviewer-nextview/docs/spread-shift-memory-plan.md:380`
のunit/token/既存失効経路、`C:/home/mimageviewer-nextview/docs/spread-shift-memory-plan.md:453` のcontext-local session、`C:/home/mimageviewer-nextview/docs/spread-shift-memory-plan.md:617` のG消費契約を読んだ。
同文書は進行中で本turn時点の参照に過ぎず、未統合コードを現HEADの事実とはしない。
現在共有する接点は `src/ui_fullscreen.rs:14619`（unit token）、`src/ui_fullscreen.rs:17859`（display-unit owner失効）、
`src/ui_fullscreen.rs:37440`（組変更の失効/reconcile）、`src/app/viewer_context_registry.rs:2698` 付近（bundle swap）、
HUD/設定とhandler。統合後の**採用済みshift/組/role/tokenを読み取るだけ**とし、anchor復元、
SpreadShiftService、DB、RestoreReceipt、ON/OFFや保存順、兄弟sessionの決定を再実装しない。
shift操作・復元・解除に伴う組直しで効果を取消し、保存replyの到着だけでページturnを合成しない。
実装者一人が共有ui_fullscreen/app/registryを所有し、E差分へ同時に書き込まない。

**G（本構成、後続）**: `C:/home/mimageviewer-nextbook/docs/book-composition-plan.md:427`
（builder/cache）、`C:/home/mimageviewer-nextbook/docs/book-composition-plan.md:950`（shift境界）、`C:/home/mimageviewer-nextbook/docs/book-composition-plan.md:1072`（多窓/Remote）を参照。
将来のLayoutSource/明示Pair/Single/Blank/補助role/hidden許可を、accepted compositionの消費側として扱う。
自動pairing・idx連続性・shiftの存在を必須条件にしない。構成保存/reload/解除・新unit adoptionは取消 + 演出なし。
Gのmode-onlyとbinding変更の分離、hidden候補の提示許可、保存モーダルの入力ownerを迂回しない。
構成のrevisionだけが変わって休眠shiftが残る場合も、Gが供給する実効layout tokenで照合する。
Gの仕様/APIの追加、共同store、projection生成の変更は本項の範囲外。

実装候補のファイル所有範囲:

| chunk | 主な所有ファイル・すること |
| --- | --- |
| A0: E統合後再照合・baseline・設計採択 | 本書、display-pipelineの契約、E/G接点。利用者回答と独立review合意を記録 |
| A1: pure presentationと通常page paint | 新規小module（例 `src/page_turn_animation.rs`）、lib登録、ui_fullscreen.rsのtint/input原因。既存draw_fs_spread_pageとdraw_fs_imageを同じ係数へ接続。FullscreenPaintResourceのprepare/UV/leaseと実paint frameの提示通知は変更しない |
| A2: contextと全入力・設定 | app.rs、viewer_context_registry.rs、gamepad_input.rs、settings.rs、preferences.rsとpages側、settings_transfer.rs。全経路の取消、OFF fast path、既存確定/保存worker |
| A3: 完了review・検証・説明 | unit/handler/paint/snapshot、perf解析、display/architecture/keymap/spec/manualとdocs索引、detached §11。共有ファイルのwriterはA1/A2を通して一人 |

## 7. 設定とkeymap

初版は共通の `page_turn_animation_enabled: bool` **一つ**を提案。環境設定の静止画表示ページに
「ページめくりアニメーション」、説明「単発のページ送りに短いフェード。連打・読込待ち・連結読みでは瞬時切替」。
方式A・80ms・初期alpha0.85は固定、方式picker/時間slider/本別favorite値/Remote同期は初版に作らない。
**既定OFFを推奨するが、Q3回答待ちであり確定ではない。** 欠落fieldの初期値も採択値へ揃える。
settings.dbの既存save、preferences draft確定、settings transferの分類/round tripを使い、新DBを追加しない。
OFF→ONは次の適格な操作から、ON→OFFは次paintで終了。settings変更のために本をreloadしない。

初版の新しいショートカット/KeyActionは提案しない。既存ページActionを変更せず、取消は非消費のpresentation処理。
操作原因・repeat factをexisting typed consume/PadActionから渡し、キーコード直書き・OS levelによるedge再分類をしない。
後にtoggleをkeyboard/リングへ公開するならKeyAction + ini_name/context/trigger/default_chords/ALL_ACTIONS/helper/
docs/keymap.ini.defaultと操作カスタマイズ対応を一緒に追加する。IME/TextInput/前面popupの所有規則を守り、
入力取消のためにそのキーをviewer Actionへ昇格させない（`docs/key-customization-impl-plan.md:443`）。

## 8. テスト・性能gate（実装時、今回は未実施）

**A0で先にbaseline**: E統合後の同じcommit/fixture/設定/窓配置/refresh rateで、effect OFFを測る。
実装後はOFFとONを交互に3回以上比較し、warm/coldを分ける。通常画像（FHD/24MP）、ZIP、未cached PDF、
カラー化/LUT/AI、単ページ/見開き、LTR/RTL、縦横連結、二つのF12窓を含める。
単発、down/up連打30/60Hz、held repeat、gamepad repeat、wheel burst、seekを別々に採る。
実機入力はconcreteなシナリオ・時間・desktop使用・disposable dataを提示した**別の明示承認suite**でだけ行う。
今回はbaselineを取得するための起動/ビルドも行わない。

| 層 | 必須の検査 |
| --- | --- |
| pure state-transition | 偽clockで0/40/80ms、開始/finish/次入力取消/同入力再開始なし。未ready→後着readyはIdleのまま。repeat/held初回と二回目、境界no-op、jump/split、generation/token/source不一致を検査 |
| handler / ordering | sameframe down/up、frame内複数edge、released repeat、前面popup wheel、gamepad Press/Repeat、jump原因を通す。追加入力drop基数・既存seqの受理/ready/retire時刻とtarget順がOFFと同じ。animationの有無でload contract/producer admissionが変わらない |
| paint / context | 最初のready frameで全対象ページがalpha>=0.85でpaintされ、sequenceが通常どおり提示済みになる。見開きall-or-none、白紙/補助role、透明PNG、capture/exportにtint混入なし、AI/PDF高画質差替えで即1.0。A cancel/close/parkがBを変えない、fork Idle、mount/depositで時計保持、replay pass重複なし |
| snapshot frames | synthetic source + injected clock、t=0/40/80msとcancel直後を固定して採る。single/spread・LTR/RTL・white slot・透明画素・通常/指定サイズ・Light/Dark、小さいF12。設定OFF/ON説明行も採る。sleep実時間を使わず全PNG差分を目視 |
| perf / idle | baseline/ONのaccepted target数・提示順・drop基数、input→ready/最初paint、release→完成、frame update/outside、cycles、GPU resample/upload数・VRAMを比較。終了・cancel・hiddenで演出repaintが残らず連結の既存scroll周期も変わらない |

paint検査は `src/app/paint_record_test_support.rs:106` のresource記録と通常handler fixtureを拡張し、
snapshotは `tests/ui_snapshot.rs` / `tests/support/ui_harness.rs` の共通builderを使う
（`docs/ui-snapshot-policy.md:97`）。GPU比較の成立をunit testだけで主張しない。

`--perf-log` は既存 `fs/page_turn_decision`, `fs/page_turn_ready`, `fs/paint`、
`frame.begin`、`ui.fs_render_breakdown`、`ui.tail_repaint`、scheduler/upload/VRAMを再利用。
必要な新イベントは `page_turn_animation/start|skip|cancel|finish` にreason/context/viewport/items世代/
unit source/元input_seqを載せる。最初のpaintは別途1回相関し、frame replayで重複しない。
alphaだけのframeはページ提示数に数えない（既存geometry/sourceのdedupを維持）。新seqをframeごとに発行しない。
perf OFFではextras/時計の診断読取を行わず、効果OFF時の分岐は直ちに返る。

```powershell
python scripts/analyze_perf.py <log> page-turn --check
python scripts/analyze_perf.py <log> latency
python scripts/analyze_perf.py <log> hitches --ms 16
python scripts/analyze_perf.py <log> idle-health
```

`scripts/analyze_perf.py:930` のI1〜I5を維持し、必要なら新イベントのcontext別相関を追加する。
`fs/paint`は描画commandの証拠でOSの実present時刻ではないため、presentation latencyの実機確認は別に残す。
ログのdrop/coverage不足はCannotJudgeとし合格にしない。
目標: repeat/seek/連結の提示順・入力基数は同一、ready→first paintに意図的待機ゼロ、
OFFの処理経路/追加uploadゼロ、ONでも追加decode/upload/resample job/texture保持ゼロ。
性能の仮判定は3回のばらつきを併記し、repeat/seek/連結のp95/p99がOFF比5%以上または1ms以上悪化し続ける場合に調査。
その閾値以下でも操作欠落・ready提示遅延・idle継続・利用者が気付く悪化は不合格。閾値を悪化の許可にしない。

狭いpure/handler/paintテスト→UI snapshot→fmt/glyph→共有変更の `scripts/test-full.ps1` が実装時の順。
実装後の独立reviewは設計自体・全consumer・OFF経路・E/Gの接点を確認する。
実装時はbuild-devで利用者用binaryを準備し、interactiveは別承認のportable-smoke、normal profileは利用者だけが起動する。
**本turnは文書だけなのでtest/build/launch/commitは行っていない。**

## 9. 利用者への質問（各項一問、すべて回答待ち）

1. **方式はどれにしますか？** 推奨: **Aの短フェードイン（0.85→1.0）**。Bの短距離slideは座標/capture対応、Cのcurl-liteはmesh/素材管理が増えます。Aは普通の全面crossfadeより控えめです。
2. **効果の長さは何msにしますか？** 推奨: **80ms固定**。代案は60ms（さらに短い）/120ms（目に留まりやすい）。新入力で即取消し、完了待ちはどの値でも足しません。
3. **初期設定はOFFとONのどちらにしますか？** 推奨: **OFF**。既存の瞬時切替を初期値として維持し、使いたい方だけ環境設定でONにできます。
4. **初版は手動の単発page送りだけに適用してよいですか？** 推奨: **手動だけ**。slideshowは既存動作を維持します。自動送りにも必要なら即ready時だけ同じ効果を付け、間隔・readinessを変えない案へ広げます。
5. **初版の新効果はPC viewerに限定してよいですか？** 推奨: **PCだけ（F12の各窓を含む）**。Remoteの既存閲覧は維持します。Webにも必要なら端末側の描画/取消/画像readyを別chunkで設計します。

この質問は演出の新仕様だけを対象とする。通常入力を遅くする、既存連結/slideshow/Remoteの機能を外す、
未readyページの既存早期表示を止める変更を承認済みとは扱わない。必要になれば実装前に費用と選択肢を提示して相談する。

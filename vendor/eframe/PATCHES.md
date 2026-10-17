# eframe 0.37.0 のローカル変更（段2c-1）

原本コミット`83836dd16`を基準に、旧vendor対eframe 0.33.3原本の実差分から既存パッチを付け直した。
原本のchecksum・VCS・file別hashは[upstream-provenance.json](../upstream-provenance.json)。
以下は**実装担当の内部確認**であり、正式な独立レビュー・受入れではない。

## 範囲と分類

| 項目 | 分類 | 変更・維持する契約 |
| --- | --- | --- |
| #7905 hidden repaint基本修正 | (a) | 本家0.37のhidden due基本経路・sleepを利用し、旧backportを重ねない |
| dirty／painting・outer phase | (b) | native/run.rs。旧typed reducerを再適用し、通常damageはmessage dispatchの外で処理。windowごとの再入・coalesce・次epoch、hidden再予約のmaxによる100ms下限と遅い要求を早めない保証を保持 |
| surface generation・render target | (b) | native/winit_integration.rsとwgpu_integration.rs。viewport／window／generation／sizeをownerに結び付け、古いclaimを別surfaceへ適用しない |
| 非ゼロresize inline provenance | (b) | Bootstrap／AccessKit／InteractiveResizeを明示。元のResizedが非ゼロのときだけinline資格を付ける。新版のbuffered resizeに対する最新実寸利用も保持 |
| 250ms deadline・PassMode | (a)+(b) | 本家の期限を既存schedulerへ移管。期限超過はouter LogicOnlyへ渡し、同じ対象のFull damageがあればFullを優先。OS redraw待ちはWaitUntil、idle heartbeatを作らない |
| 初回visible commit／Frame getter | (b) | 旧is_first_frameをFrameの既存receiptへ置換し、rootのshow返却時だけ確定。子paintはrootのreceiptを消費しない。新版本家のbefore／after_first_showも保持 |
| startup window observer | (b) | src/startup_window_observer.rs。旧非描画observerを同梱し、root show／window create／GPU startupの既存観測を保持 |
| root／child max_texture_side | (b) | 実rendererのlimitを両方のegui-winit Stateへ同じ規則で渡す。比較回帰後の撤去判断は後続 |
| 非描画／消滅viewportのdelta | (b) | 新版Running／Viewportへ分散していたpending deltaを既存SharedStateへ集約。lookup／GC前にappendし、消滅・State欠損・非描画は同じPainter::apply_textures_deltaへ渡す |
| delta consumer契約 | (b) | 段2bのmutable consumerへ接続。renderer未準備なら共有ownerに残す。通常paintでdrain済みdeltaをclear／再適用しない。明示終端は共有renderer／Context終了時だけ |
| immediate子screenshot | (b) | 既存保留actionだけからその子のcallbackを取り出す。wgpuは子のpaintへ渡し、glowは子surfaceの画像でone-shot完了。親や兄弟のcallbackへ配送しない |
| callback screenshot／RawInput等の新版基本処理 | (a) | 上流のUi callback、ScreenshotCallback、SurfaceConfig、App::logic/ui、250ms契約等を利用。新規2c-2〜4契約とは区別する |
| test-script window witness／capture・focus診断 | (b) | callback-local Context／viewport／native allocation identityとTLS入れ子復帰を保持。基底wgpu_no_default_featuresへcfgを適応 |
| 通常版immediate計測 | (b) | 新規window・input・UI・tessellation・paint・outputの時間とdelta件数／RGBA bytes、既存の遅延ログを保持。常時I/Oを増やさない |
| liveテスト隔離 | (b) | synchronous_damage_returns_before_outer_paint_gateはignoreを保持。子再実行の--ignoredも保持し、App::uiへAPI適応。今回はcompileだけ |
| manifest／feature／Win32／standalone | (b) | 新manifestを基準にmiv feature、Gdi／Performance、Windows testのwgpu30 DX12を追加。patchは準備workspaceへ統一し、原本Cargo.toml.origを保持 |
| license同梱 | (b) | 旧vendorのMIT／Apache本文をbyte保持で同梱。normalized manifestのincludeを同directoryの本文へ合わせる |
| 旧Event::Screenshot・capture channel | (c) | 新版callbackへ置換。旧Eventを戻さず、画像配送のための追加repaintも戻さない |
| 旧manifest／lock／#7905だけを前提にしたREADME撤去条件 | (c) | 旧ファイルをコピーしない。#7905が上流にあっても残る(b)があるためvendor全体の撤去条件にはしない |

README.md、Cargo.toml.orig、同梱Cargo.lockは0.37原本のまま残す。
実行時は`vendor-next/Cargo.lock`と同workspaceのpath patchを使う。
製品rootのworkspace／patch／依存版／Cargo.lock／codeは変更しない。

## deltaの所有境界

各viewportのUI出力は、対象が存続するかを調べる前に共有pending ownerへ一度だけappendする。
通常paintは同じmut deltaを段2b consumerへ渡し、非描画／消滅時も同consumerへ渡す。
同ID内のwhole→partial列を維持し、set drain→upload／submit→free drainは段2b側が完結する。
rendererがまだ無ければ、同じownerに未適用分が残る。viewport終了で共有deltaを捨てない。
診断はscalar／文字列だけで、適用権を持つTexturesDelta cloneを作らない。

eframe回帰はrenderer未準備時の共有owner保持・子消滅と兄弟不変を検査する。
窓なしでPainterへ実RenderStateを注入する既存APIは無いため、新setterや初期化時点変更は追加しない。
実GPUでのdrain／submit／freeは同じ準備graphのegui-wgpu回帰で別に検証し、eframeからの
接続は実sourceを照合する。eframe実入口からGPU完結までを一体実行したとは主張しない。

## 後続へ残す境界

- 2c-2: アプリ共通pending_raw_inputのviewport移管、childのtake前skip判断。
- 2c-3: 案BのWindows最小化UI継続、UI／surface描画分離、描画なしaction完結、immediate子自身の描画資格。
- 2c-4: 子Close受付・消費・非描画退役、root logic-only Close確定の追加回帰。

この回は本家のUI省略・描画資格・Close処理を維持する。immediateの当該UI出力からの
Screenshot前倒し取り込みも2c-3へ残し、既存保留Screenshotの移植と混同しない。
新しいApp flag・queue・retry・時間窓・追加repaint・強制UIは追加しない。

実行command・件数・exit code・未実施・handoff境界は[段2検証台帳](../../docs/egui-0.37-stage2-verification.md)。

## 段2c-2: 保留RawInputのviewport所有（2026-10-17）

`d9a28e0f3`上で上記2c-1の未実装項目のうち入力所有・skip保持を実装した。
以下は**実装担当の内部確認**。正式な独立レビュー・ACCEPTではない。

- (b・新規) EpiIntegration共通pending_raw_inputを除き、wgpu／glowの各Viewportへ移管。
  共通take helperはそのownerの保留→新規State入力だけをappendし、最新metadataを採る。
  hookは連結後の従来位置で呼ぶ。root LogicOnlyはpost-hookの未消費RawInputを返し、
  backendはユーザーコード中のborrowを解放した上で同じrootへ戻す。
- (b・新規) child skipはState take前の共通selectorで決定し、StateとViewportの保留を触らない。
  root／deferred／immediateが同じ取得helperを使う。immediateのApp hook呼出位置は増やさない。
- (b・新規) native window再生成はlogical Viewportの終了ではない。旧Stateを置換する前に
  その未消費入力を同じ既存保留へ移譲する。初回take前のStateのraw viewport_idはROOTなので、
  所有Viewport.ids.thisで正規化し、次の通常取得で最新live-setを付けてhookへ渡す。
  mapからlogical ownerを除去した場合だけ保留も終了し、同IDの新ownerは空から始まる。
- (b・新規・準備egui-ime) 通常Context GC直後の既存output_hookでpost-GCの全live-setを用い、
  終了対象だけのcomposition／focus contract／既存retryを整理する。入力hookだけを待たない。
  別欄がfocus contractを置換するときは旧欄のretryを終了し、対象を既存contractで特定可能にする。
  新しいqueue／registry／Plugin APIや全widget data resetは追加しない。

実EpiIntegration＋fake App／State取得fixture、実egui-winit State、実TextEdit、両backendの
実initialize_or_update_viewportと、wgpuの通常GC helperを通す回帰を追加した。FullOutputは検査後に明示破棄する。
準備eframeのegui-ime依存追加はnative dev-dependencyだけで、root製品の共有policyは不変。

明示Closeの受付・承認・backendとContextのpass外退役、およびその時のpolicy整理接続は2c-4へ残す。
現在backendからContext::retire_viewportを呼ぶ経路はない。案B・immediate描画資格は2c-3のまま。
実window／live試験は実行しない。検証件数・変異試験は段2台帳を参照。

## 段2c-3a: 案Bのroot／deferred Fullとsurface分離（2026-10-17）

`1034d4ea4`上へ、合意済み案Bの主経路だけを実装した（b・新規）。
以下は**実装担当の内部確認**であり、正式な独立レビュー・ACCEPTではない。

- 新規`native/frame_output.rs`のPassDecisionを両backendの実主経路で共用する。
  Windows最小化中も要求済みFullのUIを実行し、paintはmetadataのvisibleまたは明示描画要求で決める。
  metadataは変更せず、LogicOnly、他OS occlusion、visible descendantの既存方針は保つ。
- 同じ主経路のfinish_main_viewport_outputでplatform／IME出力を対象Stateへ渡し、
  Cut／Copy／Pasteをsurface描画の外で対象の次回入力へ一度だけ配送する。
  従来のclipboard API、CRLF正規化、空textとtext無しのimage fallbackの区別は保持する。
  native／通常のclipboard producer条件やegui-ime policyは変更しない。
- 当該root／deferred UIのRequestCut／Copy／Paste、Screenshot／RequestPaintWhileHiddenを
  既存actions_requestedへ先に移し、そのcommandだけを元の列から除く。
  取り込み後にpaintを再計算し、後段の通常command handlerで重複登録しない。
  非描画action処理はcapture／PaintWhileHiddenを保持し、paint ownerだけが完了する。
- wgpuは非描画時にも共有deltaを既存Painter::apply_textures_deltaへ渡す。
  主経路のsurface／resource分岐をfinish_wgpu_main_renderへ抽出し、surface処理には
  tessellation／capture／post_renderingを集める。非描画はpresent／visible commitを呼ばない。
- glowは有効な生存GL contextを選び、非描画でもset_texture→free_textureを実行する。
  clear／tessellate／paint／swapとは区別し、contextが無い場合だけ既存renderer ownerへ残す。
  post-UIの明示描画に備え、preclearを実行した事実から後段clearの要否を決める。
- 出力元がFull中GCで消えても、共有deltaの処理と他viewport宛commandsの通常配送を継続する。
  rootのvisible receipt／実表示ACKはsurface経路のみ。追加repaintや待機／retryは足さない。

回帰名minimized_fullの10件は、実EpiIntegration＋fake App、実State／TextEdit、
実backendが使う判断・出力・resource helperとfake window／painterを検査する。
native run_ui_and_paint全体、実HWND／GL surface、eframeから実GPUまでの一体実行は行っていない。
GPU consumerは同graphのegui-wgpu回帰で別に確認する。詳細・変異試験は段2台帳。

native/run.rsと両render_immediate_viewport本体は不変。100ms下限、遅い要求の非加速、sleep、
outer phase・bootstrap／resize例外、250ms deadlineを維持する。
immediate子の描画資格と非描画出力・当該子UI action完結は2c-3b、Close契約は2c-4へ残す。

## 段2c-3b: immediate子自身の描画資格と出力完結（2026-10-17）

`fb43c743f`上へ合意済み案Bのimmediate部分を実装した（b・新規）。
以下は**実装担当の内部確認**であり、正式な独立レビュー・ACCEPTではない。

- 両backendの登録callbackとrender_immediate_viewportを共通driverへ接続する。
  子UIの実行後、子自身の最新ViewportInfoと既存Screenshot／PaintWhileHiddenでpaintを決める。
  親のpaint資格を流用せず、metadataも偽装しない。UI callback中はnative borrowを保持しない。
- 共有deltaの受領を出力元の再検索より前に置く。platform／IME・clipboard actionは
  2c-3aのfinish_main_viewport_outputで対象Stateへ完結し、native commandsは通常handlerへ渡す。
  子のpaintが不要でも資源とcommandsを処理し、present／visible commitを発行しない。
- glowの既存renderer所有pending_deltasをRcで共有し、旧viewport別pending_deltaを除いた。
  root／deferred／immediateが同じownerを使用し、有効GL contextでのset／freeと
  clear／paint／swapを分離する。context無しでは既存ownerへ残す。別queueは追加しない。
  通常commandsで初めてcontextが利用可能になった場合も同ownerを適用し、空なら何もしない。
- 子出力に当該UIの明示描画commandがあればpaint前に既存action ownerへ移す。
  実入れ子passではeguiがcommandsを親FullOutputまで保持するため、通常の親出力配送から
  子の既存ownerへ渡し、必要な既存Fullの列で一度だけcaptureする。Contextの配送規則は不変。
  既存保留と当該UIのScreenshotはcallback identity／順序を保ち、後続Fullで重複しない。

回帰2件は各backendの実immediate入口・実Context／入れ子run_uiを通し、fake window／painterで
各27回の子UIを検査する。親／子のpaint・callbackを別に数える。描画先不在時の資源配送も検査するが、
Close／退役・再作成防止の証拠とはしない。実native HWND／GL実動作は未実施。
3種の部分復元で両回帰の失敗を確認し、byte復元後の全検証は段2台帳へ記録する。
native/run.rs・scheduler・root製品graph・双方のlockは不変。追加repaint・強制UI・App flag・
新pending bool・retryは追加せず、Closeの受付・消費・退役とroot Close回帰は2c-4へ残す。

## 段2c-4a: 子Closeの非描画受付・消費・退役（2026-10-17）

`41ec02184`上へ、合意済みの子Close lifecycleを実装した（b・新規）。
以下は**実装担当の内部確認**であり、正式な独立レビュー・ACCEPTではない。

- `App::on_viewport_close_requested(ctx, viewport_id)`と`ViewportCloseResponse`を追加する。
  Close／Cancelはその一回を消費し、DelegateToUi（既定）は従来UIへ委譲する。rootには呼ばない。
  非描画の`native/viewport_lifecycle.rs`を両backendのOS event完了・outer Full／logic前後へ接続し、
  backend borrow／lockやUI passの外でAppの既存終了ownerへ渡す。immediate callbackからは呼ばない。
- OSとprogrammatic Closeは既存のViewportInfoのCloseへ合流する。window未生成のCloseも受付可能。
  承認・拒否とも対象info.eventsと全ownerの保留RawInput／State snapshotからCloseだけを取り除く。
  Contextの既存cached inputも限定消費し、通常のkey／pointer／focus／modifier／IME列を保つ。
  拒否ラッチは作らず、新しいCloseは再び受け付ける。
- 承認後は対象と子孫のnative window・WindowId対応・State・surface・保留入力を、通常GCと共用の
  resource cleanupで終了する。次の子callback／親Fullを待たない。兄弟と共有deltaは保持する。
  既存rendererへのdelta移譲を先に完了し、終了を理由にdeltaをclearしない。
- native公開停止後、borrow外で`Context::retire_viewport`と
  `egui_ime::retire_viewport_ime_policy`を呼ぶ。保留中のcomposition／focus contract／retryも
  そのownerだけ終了し、同IDの明示再openへ引き継がない。準備eframeのegui-imeをnative実依存へ移した。

hide／minimizeだけでは退役しない。新queue・pending bool・退役済みIDリスト・追加repaint・
強制UI・RequestPaintWhileHidden・retry・全memory resetは追加しない。
rootの終了／tray契約と製品graphは変更せず、root logic-only Close確定回帰は2c-4bへ残す。
実windowを作らない回帰・変異確認・feature別結果は[段2台帳](../../docs/egui-0.37-stage2-verification.md)。

## 段2c-4b: root logic-only Close確定回帰（2026-10-17）

`19fc1e55a`上へ案B15の回帰を追加した（b・新規）。**実装担当の内部確認**であり、独立ACCEPTではない。
wgpu／glowのfeature別2件が、共通の実EpiIntegration::update_logic_onlyをfake Appと実行する。
App::uiはpanic、commandsはlogicだけから返す。root Close入りRawInputで、logic直後・native効果前に
CancelCloseでshould_close=false、CancelClose無しでtrueを検査する。本家の確定経路は変更しない。
完了を消費したlogicのCloseは通常出力→fake nativeイベント→次回入力へ一度だけ渡し、再受付・再発行を検査する。
両backendの既存「未消費入力を同rootへ戻す／今tickのinfo.eventsを消費する」2行だけを
finish_root_logic_inputへ共通抽出し、回帰でも同じhelperと既存take_viewport_inputを使う。
通常入力保持・metadataの更新順序・visible commit無しも確認する。
backend全入口／OS配送はsource接続の確認までであり、実window・GPU・製品終了ownerを動かした試験ではない。
tray・保存・settings-family・ZIPの製品policy、新pending bool・queue・retry・repaint・強制UIは追加しない。
3種類の部分復元で両回帰の失敗を確認し、復元後の全graph検証と段2全体の引継ぎ一覧は段2台帳へ記録する。


## Stage 3: approved viewport retirement (design revision 4.5, 2026-10-17)

Implementation record only; independent acceptance and execution evidence belong
in the stage-3 verification ledger.

- Both native adapters retain the exact Context/output registration and parent
  registration. viewport_lifecycle consumes approved root commands for Full,
  LogicOnly, pass-outside hooks, and shared-renderer shutdown before ordinary
  command lookup or native-resource absence can discard them.
- Close hooks capture native registration before entering App code. Legacy Close
  is normalized to approval of that captured lifetime; RetirementTransferred
  drains authority already emitted by App. Cancel consumes only this request;
  DelegateToUi retains the traditional unmanaged-viewport path. App approvals
  execute before an old OS Close can be redispatched to an ended binding.
- Exact native parent registrations extend the owned subtree if Context GC ran
  first. Native cleanup is shared with normal GC and removes children first.
  Removed callback owners drop outside backend borrows; Context and then IME
  finish outside those borrows. Same-ID new registrations and sibling resources
  remain untouched, including IME when the old Context entry is already gone.
- Full first transfers texture output to the shared pending owner, consumes
  authority, removes stale output, runs normal GC, and collects callback-drop
  deltas. The ordinary paint/no-paint consumer submits that batch once. LogicOnly,
  hooks, and shutdown complete delivery at their own non-drawing boundary.
  Retirement-generated frees merge without erasing already accepted set lists.
- Immediate prepare checks native-vs-Context registration before update. Explicit
  same-ID reopen removes the obsolete native subtree through the common cleanup,
  transfers callback-drop deltas, and creates fresh State/input/commands/resources.
  It never retags the old owner or retires the new Context/IME registration.
  Nested ordinary output cannot retag a different native lifetime either.
- HeadlessEpiIntegration is exposed only with miv-test-script-window-witness on a
  native renderer build. It drives the real EpiIntegration Full/LogicOnly and root
  Close decision without windows, renderer, user profile, or filesystem access.
  Callers own output delivery. Existing vendor fixtures share its constructor.

Ten common viewport_retirement scenarios run through each adapter: real Full
logic and GC/discard; LogicOnly; Close cancel/delegate/transfer/legacy behavior;
Context-first/native-only cleanup and IME; old output/reopen/duplicates; overlap
and root Close/CancelClose; texture ordering within the shared batch and callback
free; immediate nesting; hook-before-reopen identity; and actual discard plus
immediate native replacement with stale State/pending/deferred Close seeded.
Wgpu uses real SharedState/Painter with no initialized renderer. Glow uses real
Viewport and production common cleanup with fake WindowKeys. GPU submission/free
still requires the existing egui-wgpu offscreen evidence; fake adapters do not
prove GPU completion.

A confirmed design gap remains: pass-1 texture deltas can stay in run_dyn's local
FullOutput while pass-2 immediate cleanup submits free, after which the outer
old set can arrive late. The replacement regression currently proves native input
isolation and shared-owner free retention only, not strict submission ordering.
A discard-retry delivery callback was proposed but is NOT implemented pending
an explicit design decision covering installation/removal, Context matching, and
exactly-once transfer.

## Stage 3: formal vendor graph

Earlier descriptions of vendor-next/preparation-only graphs are historical.
Stage 3 uses the formal vendor/ crates from the root Cargo.toml and Cargo.lock.
Each normalized vendor manifest has its own [workspace] and [patch.crates-io]
pointing at the same formal egui, egui-winit, egui-wgpu, and eframe crates.
Cargo.toml.orig and the upstream README are retained. Original upstream lockfile
hashes remain in the provenance inventory. Active standalone Cargo.lock files were
initially copied from the root lock during WIP setup and are resolved/recorded by
ordinary offline Cargo; they are not original upstream lockfiles.
The shared IME implementation lives in crates/egui-ime; egui-winit and
eframe use ../../crates/egui-ime. Archive/VCS/checksum and original file hashes
were moved to vendor/upstream-provenance.json. Graph migration is not evidence
that runtime checks or release gates have passed.


## Stage 3: discard texture delivery (design revision 4.6, 2026-10-17)

Implementation record and implementer internal inspection only. Independent
acceptance and command results belong in the stage-3 verification ledger.

- Native Full operations in wgpu and glow install the Context-bound discard
  scope through their output-processing lifetime. Consumers upgrade Weak native
  resource owners, receive into the existing shared pending delta, and use the
  existing no-paint resource consumer. Native immediate entry asserts the scope
  invariant before native lifetime replacement. No cleanup is delayed.
- append_retirement_delta is generalized to append_texture_delta. Full,
  immediate, discard and retirement acceptance preserve old set/partial lists
  when a later free arrives, including renderer / GL-context unavailability.
  The generic epaint append and TextureManager metadata API remain unchanged.
- discard_texture_delivery tests check actual registration GC, native callback
  capture drop (last TextureHandle), State/input separation, same-ID reopen,
  sibling IME ownership, and the production wgpu scope's Weak/uninitialized
  pending behavior. Glow's real resource-consumer function is exercised with
  fake GL calls, explicitly not real-GL evidence. egui-wgpu tests separately
  prove actual upload/submission/free and offscreen pixel content.

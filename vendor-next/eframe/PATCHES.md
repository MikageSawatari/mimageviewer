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

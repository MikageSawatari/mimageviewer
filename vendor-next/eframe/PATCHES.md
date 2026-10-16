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

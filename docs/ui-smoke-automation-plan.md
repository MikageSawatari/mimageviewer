# 実アプリ smoke の自動化 (§1.197)

2026-09-07。担当・検証台帳は [v3.7.0-priority-work.md](v3.7.0-priority-work.md)。
既存のテスト・実機確認・通常データ保護を維持し、検証できる入力経路を段階ごとに増やす。
各段階は独立レビューを受け、未実装の段階を自動化済みとして数えない。

**実行運用（2026-09-08利用者指定）:** 実アプリ起動・操作は原則リリース前の検証枠に集約し、
具体的な内容・所要時間・PC使用範囲への明示了承を得てから実行する。会話中の予告だけでは開始しない。
通常の実装・非対話テストは継続し、live検証は待ちとして記録する。
`ui-smoke.ps1`の`-InteractiveApproved`は了承後だけ指定する。
共通手順は[実アプリ検証の実行確認](interactive-release-verification.md)。

## 範囲

| 段階 | 実アプリで確認すること | 必要な境界 |
| --- | --- | --- |
| S0 | 診断ビルドの隔離と使い捨てデータでの起動 | `portable,test-script` の別target/staging、exact exe/data、成果物の証跡 |
| S1 | 複数窓でPDFを開き、bindingとページの独立性を確認 | registry由来の窓snapshot、固定target、viewportを指定したKeyAction |
| S2 | 静止画の列の押下・ドラッグ・release | egui pointer timeline、描画ownerの名前付き矩形、frame acknowledgment |
| S3 | 動画canvasのzoomとstrip/panel/modalとの入力優先順位 | 実OSマウス入力、exact native target/矩形、実配送と処理の観測 |

## S0: 配布物と診断成果物の分離

通常portableは既存の`target-portable`・`dist`・zipの流れを維持する。診断モードは
`target-portable-test-script`で`portable,test-script`をビルドし、
`target/portable-smoke-package`へ組み立てる。署名・zip・通常配布先への書込は行わない。
依存copyの定義は通常portableと共用し、別のDLL/model一覧を作らない。

`prepare-portable-smoke.ps1 -TestScript`から、既存の安全な最終パス
`target/portable-smoke/mimageviewer.exe`と同`data`を準備する。診断runnerに任意exe/dataの
引数を持たせず、起動対象・データ・使い捨てmarker・reparse pointを検査する。
生成・置換・削除前にもworkspace内の固定パスであることを確かめる。
`-SkipBuild`はfeature等の生成証跡とexeの一致を要求し、通常portableや古い別成果物を
診断版と誤認しない。診断準備から既存の通常profileプロセスを停止しない。

独立レビューで、leafだけでなくrepoからの全祖先のreparse検査、Cargo workspaceの
固定、build前後のsource fingerprint照合、gitから除外された埋込SVG等の明示列挙を
追加した。準備不成立はexit 2、実行済みRhai assertionは1、timeoutは124として区別する。

prepareは毎回smokeのdataを作り直すため、runnerは次回準備の前に消えないrun固有の
証跡ディレクトリへ、使い捨てdataの実行ログ・build manifest・シナリオ識別・終了結果を保存する。
成功だけでなくassertion失敗・環境不成立・timeoutも保持する。初回liveでは担当が同じ退避を
行い、runnerへの保存処理の統合前に次シナリオのprepareでログを消さない。

実UI試験では対話desktopへアクセスできる環境を使う。今回のdefault sandboxは
GetForegroundWindowが0で、承認されたsandbox外の同じread-only呼出しでは非0だった。
build/単体検証の成功と対話入力環境の成立を区別し、foreground不成立を通常handlerの
guard弱化で迂回しない。sandbox外でも起動可能なアプリはexact disposable portableだけである。

既存の`page-turn-smoke.ps1`はnormal coreを起動する経路があるため、その起動処理は流用しない。
通常profileの設定・画像・DBをsmokeデータへコピーしない。
現`lib.rs`は`--test-script`実行時にsingle-instance取得・既存instanceへのactivate/open-path
配送・listener起動を省略する。runnerはこの既存経路と明示的な使い捨てdata-dirを維持し、
利用者の別portableへfixtureを転送しない。新しいinstance名前空間は追加しない。

runnerの証跡保存は実装・独立レビュー済み。初期のlive証跡は担当が実行ごとに
`target/v370-work/`へ保存した。2026-09-08のnative再liveではAppの失敗（exit 2）を
prepare成功・archive成功と分け、12ファイルの保存とSHA一致を確認した。
自動保存では、prepare前に一意のrun directoryを
`target/ui-smoke-runs/`へ作成し、準備段階・検証済みartifact・起動PID・終了値を区別する。
fixture/script/runner/settings overrideの実ファイルとSHA、build manifest、marker、
使い捨てdataのlog/perfを保存し、例外・期限切れ・準備失敗でもmetadataを残す。
準備失敗時の既存dataを今回のrun証跡へ混ぜない。未検証pathやreparse pointはコピーせず、
収集失敗は隠さず記録する。環境変数全体や通常profileのDBは収集しない。
同時runは共通portable/dataを共有できないためrunner間で排他し、別runを停止しない。
準備子processのstdout/stderrはrawで別々に保存し、PowerShell 5.1の
`ErrorActionPreference=Stop`とnative stderrのstream変換を避ける。
scenarioの期限はアプリ起動時のmonotonic clockから一度決め、focus待ちも同予算に含める。
prepare・App・runnerの終了値は分ける。cleanupとarchiveに例外があっても外側finallyで
排他を解放する。fixtureも新しく作成できた領域だけを今回の証跡の対象にする。

## S1: 窓snapshotとtargetの所有

App投影だけでなくregistryのread-only参照から、window ID・context serial・residence・
viewport ID・現在項目/ページ・items generationを公開する。snapshot取得のためにmountを
入れ替えたり、他contextのworkerをdrainしたりしない。Buildingの未公開bindingは公開しない。
描画済みの証拠は、対応するviewport/owner/frameから得る。窓が存在することだけで
PDFが表示されたと判定しない。

ROOTとdetachedはtyped ownerで区別する。ROOTは`viewer_context_main()`のcontext serial・
`ViewportId::ROOT`・既存`main_hwnd`、detachedはregistry bindingとmanager発行host claimを使う。
Rhai表示にもroleを付け、ROOT用のwindow IDやhost incarnationを捏造しない。
ROOT snapshotは一時的にAppへmountされているdetached内容を読まない。
main context自身にdetached bindingが付く既存F12等の経路もあるため、binding列挙から
mainを除外しない。同じcontextでもROOTとDetachedの表示ownerは別に保持する。
非detachedの専用fullscreen viewportはROOTとは別hostであり、window IDがないことだけを
理由にROOTへpaintを帰属させない。S1では一覧ROOTとsingle-page detached PDFを対象とする。
一覧ROOTはread-onlyなtarget一覧の提供に限定し、一覧描画の完了証跡は追加しない。
一覧自身は新しいviewport/paint証跡を発行しない。値は初回の未観測（false/0）または
同ownerの過去fullscreen callbackの履歴であり、一覧の描画失敗・完了を示す値ではない。
一覧の開始条件は既存のtarget登録・focus・items状態で確認する。ROOTの画像paint証拠は
実際にROOT内でfullscreen画像を描く経路に限る。

scriptが選ぶtargetはwindow/context/viewport identityを固定する。closeやswitch後に
別の最新窓へ付け替えない。stale targetは明示的な失敗として返す。
既存`test_script::PendingAction`はviewport指定がないため、そのままでは最初のconsumerが
別窓の操作を消費し得る。test-script専用のconsume/peekにviewport照合を追加する必要がある。
通常キー操作のKeyAction/keymapを迂回する新しい操作実装は作らない。

独立レビューで加えた条件:

- 既存の`Fs*` scriptを壊すため、未選択targetを一律ROOTにはしない。後方互換の
  `LegacyImplicit`と明示`Targeted`を型で区別し、明示操作だけexact ownerを固定する。
- 同じviewportに別contextが移る場合がある。明示操作はwindow/context/viewport incarnationを
  pendingへ保持し、入力consumerの直前にも現在ownerと照合する。
- `snapshot().windows`はread-only。passive窓を操作する選択にactivationが必要なら、既存の
  activation handlerを通す。テスト専用のmount/session書換えは加えない。
- 現`target_rendered`は入力callback到達でありpaint証明ではない。current context・
  items generation・pageに結び付くtexture readyとpaintを観測し、旧ページの描画済み値を
  新ページへ引き継がない。ROOT/childのrevisionを区別する。
- `draw_fs_image`には旧ページの`Captured` holdoverも入る。mounted currentのpage/genを
  そのまま付けず、S1では`CurrentItem`の実texture描画だけを現ページの証拠にする。
- deferred callbackは登録時の固定viewではなく、実行時の`shared.view()`を描く。
  証拠は元snapshotのtexture選択時に作り、画像と同じview payloadとしてcopy/updateする。
  登録時ownerと実行時画像を別々に取得して結合しない。
- passive snapshotはthumbnail fallbackを許す。後からfs_cacheがfull readyになっても、
  既存snapshotのtextureをfullと扱わない。画質の由来もtexture取得時に保持する。
- content keyはread-onlyな`ContextRef.items()`の`GridItem::perf_key()`から取得する。
  snapshot取得時に`App::item_id`のinternerを変更しない。textureの由来を
  `FullscreenPaintResource`と一緒に運び、変換・cache hitでも現在のownerとの対応を保つ。

S1は、S1a（read-only window snapshot・paint-commandの証拠）と、S1b（対象固定・
既存activation・PendingAction配送・複数窓PDFシナリオ）へ分割する。
S1aはsingle/paged PDFを対象にし、S1bのfixture設定もsingle-pageを明示する。
paint-command発行はGPU scanout成功の証明ではない。
`full_texture_painted`は現在のowner/context/items generation/pageについてfull/processedを
一度描画した証拠であり、最新frameの画質保証ではない。`paint_revision`もその証拠のrevisionで、
入力処理完了のackやGPU present番号には流用しない。owner/page/genが変われば旧証拠を破棄する。
同じpage/genの調整変更後に、最新の調整まで描画済みであるとは主張しない。
履歴はowner/current contentごとの最良証拠に集約し、同じページのTextureIdを無制限に保持しない。

最低限の回帰は、兄弟/ROOTのconsume/peekで対象操作が失われないこと、対象だけが一度消費、
close/binding移譲後は拒否、snapshot前後でmounted owner・queue・generation不変を確認する。

PDF fixtureは既存`page-turn/generate_pdf_fixture.py`で使い捨て領域に2文書を作る。
設定overrideで複数窓モードを選び、ROOTからそれぞれ開く。異なる2つのwindow/contextと
各PDFページの描画、片方への操作で兄弟状態が変わらないことを確認する。
close後のlive確認はwindow ID自体のregistry不在と、保持した選択identityの
`current == false`を両方要求する。identityの不在だけでは同じ窓のHWND再生成も成功になる。
固定fixtureの初期page 0、対象page 0→1、対象と兄弟のitems generation不変も照合する。
stale action受付の拒否・LegacyへfallbackしないことはUiRuntime/consumer回帰で確認し、
意図的なlive失敗や任意のexit 2を成功へ読み替える経路を追加しない。

### S1b: 実装前提調査で確定した配送境界

既存`run_action`はOS foregroundの登録・focusを要求しないdirect actionであり、
`tap_key`/`hold_key`の物理foreground由来routingとは契約が異なる。
未選択の`LegacyImplicit`は、従来の最初の該当consumerへの配送・ROOT frame基準のexpiryを
維持する。窓固定の保証は持たず、新しいforeground/host登録条件を加えない。
明示`Targeted`はS1aのRoot/Detached exact identityを保持し、受付時にも再検証する。
Targetedがstale・未登録・不適合の場合にLegacyへfallbackしてはならない。

- 暗黙操作のownerをpresentationやKeyAction scopeだけで固定する初期案は撤回した。
  Global/Ratingの複数caller、専用fullscreenのFs操作をROOT handlerへ転送する経路があり、
  単純なmappingは従来動作を変える。既存handler/guardを再実装せずLegacyの互換性を保つ。
  新S1/S2シナリオは必ず明示selectし、Targetedであることを記録・assertする。
  Legacyの成功を複数窓への固定配送の証明には使わない。
- bootstrapはRootのexact hostと対象contentの準備を待ち、select_root後のTargeted actionが
  通常Focusと実consumer ackを担当する。初期待機へLegacy foreground由来の
  target_registered/focusedを要求しない。初回liveでRoot/items準備済みでもこの2値だけfalseと
  なって停止し、Targeted Focusへ進めない順序の誤りを確認したため修正する。
  通常handlerのfocus/owner guardとLegacy key APIの前提は維持する。
- `Keymap`の5つのconsume/peek入口は既に`&egui::Context`を受け取るので、そこから
  test-script consumerへctxを伝える。`keyboard_owner_for_pass`の既存pass境界を使って
  実行中の論理ownerを公開し、Targetedのownerと一致するhandlerだけに渡す。
  兄弟のconsume/peekはTargeted要求・ackを変更しない。Legacyへこの新規条件を重ねない。
  eguiのdataはviewport間で共有されるため観測keyへviewport IDを含め、pass番号だけで
  区別しない。inventoryではなく実mounted contextとidentityの一致を観測する。
- Targeted Rootは既存`ViewportCommand::Focus`を要求し、既存gridのfocus/permit/text/modal等の
  guardを通った実handlerのeligible passを待つ。専用のguard迂回は加えない。
- passive detachedのactivationは、既存`queue_deferred_detached_window_activation`と
  `commit_pending_deferred_detached_window_activation`を使う。既存intentはwindow IDだけを
  保持し、commitは全pendingの最小IDを選ぶため、直前queueだけでは対象commitを保証しない。
  test requestはtyped待機状態で保持し、Closingを含め既存intentが一件もないdispatch機会で
  exact claimを再検証→queue→通常commitする。同じUI処理内で行い、actual ownerも照合する。
  既存intentは消去・並替え・追越しをしない。commit失敗で再queueせず明示失敗にする。
  close/transfer後に最新bindingへ付け替えず、独自mount/session変更を加えない。
  active contextもROOT処理中はAtRestになり得るため、residenceをpassive判定に使わない。
  既存active session/bindingを先に確認し、真のpassiveだけactivationへ渡す。
- Targetedには従来の新ROOT frameごとのexpiryを適用せず、passiveのactivation/child処理を待つ。
  受付後の対象ownerの最初のeligible passを基準に未消費失敗を判定する。pass開始を
  完了と誤認せず、対象handlerが処理する機会を持った後に判定する。対象が消えた場合は
  staleで終了する。待機状態は一つのtyped request ownerへ集約する。
  ROOTは既存`update_frame`の外側、childはcallback scope終端を使い、対象handlerに機会の
  あったpassだけ完了を観測する。cached `KeyboardOwner`の早期returnでも論理owner観測を省かず、
  その戻り値（focus等の許可区分）を新しいdirect action許可条件として使わない。
  実peekでack済みの要求は、一般handlerのeligible値にかかわらずそのowner pass終端で
  解放し、次passで再実行させない。同pass内のrepeatable peekは維持する。
- cancel/environment failure/finishではdirect actionのackも一度だけErrで解放する。
  script由来の長寿命activation intentをmanagerへ残さない。配送ackと操作結果は区別し、
  PDFシナリオはその後のpage/full-paintと兄弟不変を待って判定する。

PDF fixtureのoverrideは`detached_viewer_open_images_in_window: true`、
`default_spread_mode: "Single"`、`default_reading_flow: "Paged"`を明示する。

回帰にはLegacyの従来consume/peek/expiryの維持、Targetedへのfallback禁止、
兄弟の非消費、exact対象だけの消費、close/transfer、activation前stale、
対象pass基準のexpiry、取消時のack一回解放を含める。既存Grid等のfocus/permit guardを
迂回する意味ではない。上記のLegacy/Targeted分離とdispatch境界は独立Astraレビューで合意済み。

### S1b追加: backendの実hostとの照合

manager claimだけをactual callbackのHWNDとみなす当初前提は独立レビューで反証された。
旧hostが生存中でもeguiが新hostを作る実機記録があるため、claimの`IsWindow`では足りない。
診断featureをvendor eframeへ連動させ、wgpuのROOT/deferred `integration.update`直前と
immediate `ctx.run`直前で、RawInput取得と同じ実`Arc<Window>`を観測する。
既存`surface_generation`は通常resizeでも進み、viewport再挿入で再利用されるためhost identityに
使わない。Windowsのwinit `WindowId`もHWNDそのものであり、HWND再利用を区別できない。

実WindowへのWeak identityを診断recordで保持し、同じallocationには同じprocess内非再利用tokenを
対応させる。WeakはOS窓の寿命を延ばさない。dead recordを回収し、窓の履歴を無制限に持たない。
callback中のactual witnessはthread-local RAII scopeに置き、nested immediate終了時に親へ戻す。
取得不成立も明示的なNoneにし、親や過去callbackのwitnessを借用しない。
App snapshotは観測したviewport/HWNDとmanager claimをjoinし、target identityへtokenを含める。
入力consumerでは最新recordでなく現在scopeのactual witnessを照合する。
selected tokenを更新して対象変更を救済しない。初回未観測は未準備として待つ。
paint証跡も実callbackと同じhostであることを照合する。

影響先はvendor eframeのwgpu backend、test-scriptのsnapshot/選択/consumer/paint観測。
通常ビルドへ観測を有効化せず、host登録・生成・focus・mountの正常処理を変更しない。
S2 input_hookもこのactual witnessを参照可能だが、App contextとの配達直前joinは別途必要。
親Astraと独立Astraが観測境界に合意し、Solが実装前にContext分離・初回・テスト構築を検証する。

通常lib testも`cfg(test)`でtest_script moduleをコンパイルするため、rootのdev-dependencyには
同じeframeの診断featureを指定する。root packageはedition 2024であり、通常buildとdev用の
featureを分離できる（[Cargoのfeature resolver仕様](https://doc.rust-lang.org/cargo/reference/resolver.html#feature-resolver-version-2)）。
`--all-targets`等はdev featureも統合するため配布の根拠にせず、通常core指定buildの依存graphと
featureありgraphを別々に確認する。通常版のsource cfg除外と実build graphの両方を検証する。

## S2: egui pointerと名前付き矩形

既存の合成キーと同じviewport指定・timelineへ、今回の初期APIではmove/buttonを載せる。
egui面のwheelはこの初期APIにはまだ含めない。
矩形は実際の描画ownerがcontext/viewport・frame/revision・pixels-per-pointとともに公開し、
scriptは領域名と正規化位置を指定する。テスト用に別レイアウトを再構築しない。
press、drag閾値を超すmove、継続moveを別frameにする。最終stepは同一batch内で
`PointerMoved(final)`→`PointerButton(up)`を送り、release frameで変化した最終座標も検証する。
各stepは対象frameの処理を待つ。待つのはscript workerであり、UI threadにblocking waitを追加しない。
LTR/RTL、列中心とページ着地の区別、release時だけ動いた最終位置を検証する。
このfixtureは既存の使い捨て`--settings-override`でSingle/Pagedと
`fullscreen_seek_bar_locked=true`・`still_seek_strip_locked=true`を指定する。
実callbackのspread/flow/読み方向/表示/lockを公開して条件を確認し、LTR/RTL等の変更は
既存KeyActionを使う。lock切替操作そのものを試験したとは扱わない。

`StillStripDrag`は使い捨てrootに`images/`の40枚（5種類の縦横比を反復）と
`zzz-sibling.pdf`の2ページを生成する。PDFを先に開き、既存設定
`auto_fullscreen_image_folders=true`で画像folderを通常Openして独立book contextを作る。
同folderの画像leafを2回開くlegacy passive経路の窓は、S1のbinding由来DTOと同一ではないため
このsetupには使わない。ROOT初期2件のidentity/items generation、PDFと画像のcontext分離、
両identityとgenerationを照合し、画像targetをactiveに保って列を操作する。
fixture命名は既定のFolder/Archive共通行・FileName順に対応し、実media kindでも確認する。
中央pageへの移動は各FsPageNextの着地とfull paintを待つ。Single/Paged・未調整PNGの
初訪問では、その証跡公開前に通常navigation sequenceが終了する順序を確認済み。
見開き・effect holdover・同page再訪について、一般にfull paintがnavigation idleと同義とはしない。
trackのUpは、直前Moveと異なる実pageへ着地することも要求する。
列dragは使い捨て設定のheightを既定と同じ`large`へ固定し、実press rectの幅から
152ptずつ1/2/3枚分の移動点を計算する。固定の正規化座標では、幅と整数丸めによって
複数stepが同じ中心へ着地し得るためである。Down前に全点が領域内へ収まる幅と
方向別3枚の余地を検査し、操作後は実handlerが返す中心と実paintを照合する。
このfixtureは全heightや任意の小さい窓を網羅するものではない。
KeyActionの成功ackはconsume/peek時点であり、handler完了やmode公開を保証しない。
方向・flowの切替後は、Rhaiの単調timestampと残り予算を用いて、新鮮なregionの実mode一致を待つ。
queryが期限後に返した一致も成功にせず、ownerエラーはそのまま伝播する。
固定の描画回数を増やす方法は使わない。

実装前提調査で確定した境界:

- stripは外枠ではなく実セルunionに対する`fullscreen_still_seek_strip_row`の`Response.rect`。
  page seekは`fullscreen_seek_track`の実hit rectを別regionとして公開する。
- press時のrectとexact targetをgestureへ保持する。動く列の最新rectへ毎frame再正規化して
  pointer deltaを変えない。owner/hostが変われば最新窓へ付け替えない。
- 注入口は既存`SyntheticInputPlugin::input_hook`のviewport batch。配達済みviewportは
  widget処理済みではない。実handler・同frame後段のnavigationが完了したowner tailで
  input tokenとpost stateをackし、列中心の移動とpage変更を区別する。
- ROOTでbatchを作ってからchild input_hookまでにownerが移る可能性があるため、raw eventを
  eguiへ渡す直前にもexact context/hostを検証する。S1bのKeymap consumer検証だけでは不十分。
  Appのmount/show直前のowner共有とinput_hookの照合をS1b確定後に設計する。
  新ownerへDownを配送した後でackだけstaleにして済ませない。
- 現stripはlayout計算→gesture更新→先に計算したcellsのpaintという順序である。
  handler後のgesture中心を同frameの描画中心として公開しない。step ackは処理/navigation完了、
  見た目の確認は実layout中心・セル位置の次のpaint revisionを待つことで区別する。
- key専用timelineのidle/cancel/finishへpointerのheld button・queued/in-flight stepも統合する。
  cancel時は生存するlatched targetへのButtonUp、または閉鎖済みtargetの明示的なterminal処理を
  行い、egui側を押下中のまま放置して成功としない。
- releaseをROOTでmaterializeしてheld集合が空になっても、prepared batch・配送中・
  対象handlerのrelease receiptが残っていればidle/成功ではない。logical context退去と
  egui viewport破棄は別であり、同じviewportの新ownerへcleanup Upを送らない。
  元surfaceへの安全な解放を証明できない場合は環境不成立としてrunを終了し、
  使い捨てアプリの終了まで確認する。後続操作やPASSへ進めない。
- 配送済みDownのhandler成功receiptが欠けても取消を停止させない。元showの処理終了と
  操作成功を別の証拠にし、終了後は同じ生存ownerへのcleanupへ進める。
  CleanupUpはdrag閾値到達を前提にせず、exact Up配送・primary level解除・同ownerの
  処理終了で確認する。取消中も元stepとcleanup stepのobserverを有効にする。
  Move/Upのheld_beforeはreducerが所有する最後の配送座標から生成し、callerへ委ねない。
- seek track自身によるpage変更はgestureのhost変更ではない。同じwindow/context内の
  page着地を許し、actual landingをnavigation後に検証する。入力中の物理pointer混入は
  合成gestureの成功証明から除外し、通常入力を抑止する代わりに明示的な環境不成立とする。

追加の実装境界:

- 初期pointer APIはdetached静止画のstrip/trackを対象とする。既存Legacy keyは維持する。
  SyntheticTimelineが単一のtyped pointer transactionを所有し、pluginのprepared batchは
  step ID付きの輸送用cacheとする。受信boolとreceipt Option等の二重状態を作らない。
- child input_hook前にAppの実mounted context scopeを開始し、backend active witnessとjoinする。
  callback開始では入力注入より遅い。ROOT pointerやpassive deferredを未確認のまま受理しない。
- activeのnavigation処理はshow_viewport_immediate復帰後にある。widgetが捕捉したstep ID・
  child witness・handler結果をframe-local値で運び、navigation後の実App ownerとjoinしてackする。
  その時点のactive witnessはROOTへ復元済みなので、child証拠の代用にしない。
- 入力前の初回region geometryにも実callbackのviewport/backend witnessを保持する。
  show前のownerを描画証拠へ付け直さず、host登録後にexact joinしてから公開する。
- eguiの同じrunは複数passを回せる。初期案の「各passでreceiptをclearし最終passだけ採用」は
  Solの実egui probeで反証された。request_discardで2passを強制すると、releaseの
  Moved(final)→Upはpass1だけdrag_stopped=true/interact_posあり、pass2はfalse/Noneとなる。
  既存key再注入を変更せず、owner/viewport/prepared frame/time/step IDが同じrunに限った
  typed accumulatorで有効なhandler proofを保持する設計へ修正し、独立レビューで妥当と確認した。
  show復帰/navigation後に一度だけfinalizeし、別runへ持ち越さない。最終描画のlayout/paintは
  handler proofとは別に記録する。pass番号の増加だけでpressとmoveを別frameと扱わない。
  実測ログ: `target/v370-work/s2-multipass-probe-run.log`。これはeguiの応答特性の確認であり、
  S2のアプリhandler連携や実portable操作を実行した結果ではない。
- accumulatorは1回のshow invocationのlexical scopeに限定する。同じraw timeでも別showへ
  合流せず、同stepの証拠を回数として加算しない。別owner/payload/widgetによる実消費は矛盾として
  失敗を維持する。outer ROOTのdiscardでchild showを再度呼ぶ場合も、完了済みstepをprepared
  cacheから再注入しない。gesture/navigationのRust側状態はegui discardで巻き戻らない。
- Rhai pointerとnative wrapperは別child moduleへ置ける。親test_script.rsのmod/登録接点は
  一人ずつ統合し、同じファイルの同時編集を避ける。

実hook草案の独立レビューで、さらに次を実装条件へ加えた。

- pointer未使用の通常Close/F12や兄弟showで観測が欠けても、run全体を失敗にしない。
  catalogの失効と、対象pending/held/cleanup取引の未完了義務を区別し、後者だけを
  そのtyped ownerへ帰属させる。最終show tailの欠落を前passのtailで補わない。
- APIのtimeout/interrupt/channel failureは、owner・transaction ID・step IDを持つ
  cancel handleで該当取引だけを取消し、wakeする。Rhaiが例外をcatchしても、遅れて
  配送されたDownを残さない。古いtimeoutで新しい同owner取引を取消さない。
- 次paint待ちのrevisionと、pressに使うgeometry tokenを分ける。後者はexact owner・
  items generation・press前page/item・widget ID・rect・pppが同じ間は維持し、
  自然な再描画だけで失効させない。内容/幾何の変更やcatalog失効後は古いtokenを復活させない。
  Downの実Responseと準備矩形/pppを再照合し、Move/Upはpress時の座標系を維持する。
  stripのResponse rectは、描いた先頭/末尾cellのunionをstrip_contentで切った範囲であり、
  centerや画像比率により変わり得る。Move/Upで現在のrow rectをpress rectと同一要求しない。
  実layoutの安定したcoordinate frame（strip_content、trackの実座標frame）も別に保持し、
  DownではResponse rectと共に照合する。以後はwidget/mode/ppp/coordinate frameを照合し、
  normalized座標の変換は最初のpress rectを使う。通常dragによるrow境界変化と
  resize/layout変更を混同しない。実layout・handlerを使う混在比率の回帰で前提を確認する。
  Heldの領域検索もpage/item一致を要求するDown用検索から分離する。trackの同showで
  pageが移動しても、owner/genが同じ最新frameの座標系を参照できる。
  navigationがmode/layoutを変える可能性は残るため、次の実handlerでもmode/frame/pppを照合する。
  同showのgeometryをpublishして実revisionを確定し、catalog lock解放後にtimelineの
  completionへ`after_revision`を渡す。これはそのshowの最終passの描画revisionであり、
  handler後の効果が描かれた証明ではない。次paintはこれより大きいrevisionを要求する。
  公開失敗時にrevisionを捏造せず、環境失敗を確定してからexact terminal ackへ進む。
- ROOTのprepared frame/timeは輸送の証拠であり、childの時刻と一致すると仮定しない。
  eframeはROOTとimmediate childでそれぞれelapsedを採る。child input_hookの実RawInput.timeを
  配送証拠へ保持し、callbackの実InputState.timeとはそのchild時刻のbitsを照合する。
  時刻を上書きして一致させない。show-local配送証拠のない後続showへstepを付け直さない。missing-tailなどの
  終端失敗はUiRuntimeに確定してからtimelineをidleへ解放し、同frameのSuccessを優先させない。

## S3: native入力の継ぎ目

実装・レビューは次のまとまりで進める。部品の成功を後段の実OS成功へ読み替えない。

1. 通常wheel command分類の回帰（VideoZoomWheelのraw二重配送候補）と、
   actual wheelからrender・同output bus・App handlerへつながるreceiptを分けて実装する。
   source/placement/owner gate、latest-slot、overflowは維持する。
2. 外部button helperの入力解放とIPCを検証し、top hoverとpanorama buttonの通常clickを
   接続する。その操作でflat videoのzoom modeへ入り、実1notchの倍率を確認する。
   setupのためにApp状態を書き換えたり診断専用KeyActionを作ったりしない。
3. 同helperのcanvas dragでpanとreleaseを確認し、パネル・modal・stripの負の入力確認へ
   名前付き対象を広げる。既存操作の各効果ownerでreceiptを照合する。

wheel部分が単体成功しても、通常clickによるzoom開始と実wheelが通るまでは
「動画zoomの自動確認済み」とは扱わない。混在batchの§1.199は別の未完了項目として維持する。

**利用者選択 (2026-09-07): 実マウス入力を採用。テスト中に前面ウィンドウとマウスを使用し、
既存のWindows入力経路を通す。** 対象はS0の使い捨てportableに限定する。
通常のpointer polling・auto-hide・captureを止めたり、診断pointer providerへ差し替えたりしない。
実装前にexact PID/HWND・window/context・host claim・source/placementと実描画矩形の結合、
DPI/仮想desktopの座標変換、各stepの実配送/処理完了の観測をレビューする。
無変化を期待するstrip/panel/modal上の操作も、無配送を成功と誤認しないreceiptが必要。
入力配送・対象喪失・環境不成立・Appへの結果適用を区別する。

native矩形は描画ownerが公開する。payloadの座標はclient physical pxであり、pointsとの
変換はownerのpixels-per-pointで一度だけ行う。OS入力へ渡すscreen座標は対象の実HWNDから得る。
MouseMoveのcoalesceを考慮し、一括投入の成功をドラッグ成立とみなさない。
既存routeのsequenceはpump/renderで別採番なので共通ack IDに流用しない。
待つのはscript workerであり、pump/render/UI threadはackをblocking waitしない。

独立調査で具体化した実装境界:

- driverは既存Rhai workerで動かす。`SendInput`はHWNDを受け取らないglobal入力であり、
  戻り値は挿入件数にすぎない。事前のexact target検査に加え、実WndProcの受信HWNDと
  source/placementを各stepで照合し、対象喪失・前面変更・無配送を成功にしない。
- detached親HWNDとpresenter/HUDは別物。HUDは独立top-levelなので、親子関係から推測せず
  `NativeWindowHost::contract_windows()`由来の対応を観測する。
- canvasは`NativeVideoInputRegion`、panelは実描画・判定共用rect、stripは
  `seek_strip_layout().rect`、modalは実描画結果を使う。概算の`compute_hud_regions()`から
  名前付き領域を推測しない。driver threadのDPI contextを明示して復元し、
  `ClientToScreen`と仮想desktopの原点・寸法を使う。
- `SendInput.dwExtraInfo`のstep tokenを実WM mouse分岐の`GetMessageExtraInfo`で採取し、
  test feature限定の観測metadataとして既存envelopeの両routeへ運ぶ。
  polling生成move/leaveやcapture cleanupへtokenを継承しない。tokenが対象環境で
  保持されることは、最初の実入力試験で検証する主要前提とする。
- panel上wheel等の負の確認は、指定tokenの実領域内処理、zoom command/raw forwardingの
  非発行、その後のApp状態を揃えて判定する。MouseMoveの上書き・未配送・stale・render errorは
  成功にしない。route固有sequenceをstep tokenとして扱わない。

API根拠: [SendInput](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput)、
[MOUSEINPUT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-mouseinput)、
[GetMessageExtraInfo](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getmessageextrainfo)。

### S3a: 実MouseMoveの最小経路試験

最初はproductionで使われる`DetachedViewerChild + PresenterOnly`を対象に、canvas内の
異なる2点へ識別子付きMouseMoveを一段ずつ送る。HUD設定・topologyを無効化する必要はない。
Button/capture/zoom/panel/modalを一度に実装せず、実WM→両route→render後段まで
`dwExtraInfo`が保持される前提をsafe portableで実証してからS3bへ進む。

- 実sourceの正本はrenderの`PresenterSourceState.source_epoch`。UI側のAtomicは
  `SwitchSource`送信前に進むため、実sourceの代わりにしない。ただし両者が不一致な
  切替途中はprepareを成立させない。一致条件としてのみUI値を利用する。
- 初回source epochは0が正当である。publisherとtyped PresenterSourceStateが存在し、
  requestedとactualが一致することを確認し、数値0を未準備sentinelにしない。
  初回対話liveでこの診断側の設計誤りを発見した。世代値とseek serialを混同しない。
- `NativeVideoInputRegion`・同layoutのeffective ppp・render width/heightを同時に保持する。
  同placement内のresize/DPI/予約panel変更もあるため、SendInput直前に実client extentと照合し、
  render消費時にもsource/placement/geometryを再検証する。geometry versionは内容の変更を
  表し、動画の通常render回数だけでは増やさない。
- brokerは実HostWindows/leaseのpublisher寿命と退去を管理する。parent HWNDだけの
  last-writer-winsにせず、同parentに複数live候補があれば曖昧として拒否する。
  old rendererの遅いpublishで新ownerを上書きしない。
- tagged moveも通常latest-slotのcoalesceを通す。診断入力だけlosslessにする案は採用しない。
  metadataはeventと不可分に保持し、上書き・片route欠落は失敗/timeoutにする。
- `handle_window_events`のErrは通常コードがdefault routingへfallbackする場合も診断失敗。
  正常Resultと最終routing/raw-forwarding判定を揃えてrender receiptを出す。
  pump receiptはactive epoch判定とcursor reducer適用後とする。
- runtime guardはworkerでcompile manifest workspaceからのexact
  `target/portable-smoke/mimageviewer.exe`・同`data`のcanonical一致とexact markerを確認する。
  path末尾だけの一致や、利用者の別portableでは成立させない。

実装レビューで、token/HWND一致だけでは要求したcanvas座標への配送を証明できないと判明。
pump/renderの実MouseMove座標とPresenter sourceもreceiptへ保持し、両route一致、実canvas内、
要求との距離を検査する。許容は仮想desktopの16bit絶対座標への量子化から導出し、
要求座標を実績として返さない。最初の2点は実測座標も異なることを確認する。
broker-local回帰は両receipt順序、片側受領後のsource/geometry変更、publisher退去・曖昧化、
不一致後に正しいreceiptが来ても失敗維持、lock外callbackと取消後始末、local poisonを含む。
共有global brokerをテストで故障させず、故障状態もbroker自身に所有させる。

Rhai workerはcallback-localなbackend `latest(viewport)`を呼べない。選択済みの
viewport/HWND/process-unique tokenについて、registryを変更せずWeakの生存と完全一致だけを
確認するAPIを用いる。workerでdead recordを掃除したりWindowをupgradeして保持したりしない。
同じbackend allocationでAppの論理ownerが変わる場合はcached snapshotにも遅延があるため、
準備前とreceipt後は既存UiCommand上のUI検証barrierを通す。Appが直前に作ったread-only窓一覧を
ui_update入口でpublishし、その直後のcommand drainでexpected identityを照合する。
その間にApp ownerを変更できない順序を維持し、finish/cancel/timeoutはErrを返す。
UI側では待たず、workerが既存wake・interrupt・期限でreplyを待つ。barrier後にも
backend tokenとnative prepared targetを再検証し、snapshotだけの成功にしない。

親Astra・調査Sol・独立Astraが上記を実装条件として確認した。S3a成功は入力経路だけの
証明で、zoom/panのApp適用、他surface、GPU scanoutの成功とは扱わない。

**採用しなかった注入案と反証**: `NativeVideoOutputEvent`直接注入はpresenterの当たり判定を、
render routeだけの注入はpumpのcursor ownership/activityを飛ばす。当初はper-HWND sinkから
両routeへ注入する案を選んだが、`NativeWindowHost::observe`が実OSのcursor/button/captureを
読み、`cursor_polling_tick`がMouseLeave/MouseMoveを再生成するため、時間を跨ぐ合成dragを
上書きすることが判明した。成立させるには診断入力providerの追加設計が必要になるため、
利用者へ範囲と選択肢を説明し、実OS入力が選ばれた。

また、既存envelopeのepoch/generationはplacementでありsource epochではない。
将来注入案を再開する場合は、sourceの固定をrender消費直前まで保持する必要がある。
HUD wndprocにはpresenterと異なるMouseLeave/capture/held-buttons/focus claimがある。
実OS入力で到達・観測した範囲だけを検証済みとし、touch・未実施のfocus/z-orderシナリオ・
GPU scanoutを一括して合格とは扱わない。

### S3b: ズーム確認の精度と追加調査

既存のcanvas入力矩形は`video_visual_layout`から`compute_video_visual_target_rect`で求める。
同layoutが使うのはclient extent/DPI・compact・固定bar/strip・固定info panelの予約領域で、
video zoomの倍率やpan中心は含まない。固定領域を変えない今回のzoom/panで厳密なcanvas
geometry照合を維持でき、成功したzoomを受け入れるために照合を緩める必要はない。

S3aのmoveにwheelと左button down/upを加え、既存のズーム開始操作を通す。
nativeのVは`matches_vk_action`経由で、S1bのdirect action consumerとは別経路である。
`run_action("FsPanorama")`だけで開始できると仮定せず、実描画した`native_top_panorama`
ボタンのResponse.rectを公開し、実クリックする。panelは`native_top_side_panel_mode`から
ClickToShowへ切り替え、端moveで描画されるcalloutをクリックする。左jump panelの
`native_jump_bulk_bookmark`で空のbulk dialogを開き、実closeボタンで閉じれば、
既存bookmarkやOSキー入力の追加を前提とせずmodal経路へ到達できる。
これらのfixture操作では登録実行・全削除・clipboard取込を押さない。
非表示からの開始には、通常hit-testと同じ条件・矩形を持つHoverActivationの観測を使う。
上部barの初回表示は上端36ptであり、76ptは表示維持の範囲である。calloutも通常の
ClickToShow等の条件を満たす24pt端帯へのhover後に描かれる。Canvas・HoverActivation・
NamedControlを別のtyped対象とし、実hover移動の処理証拠→enabledな実Response出現→
ボタン操作の順に進める。Canvasの内側判定を広げたり、初期表示の時間内に押せることに
依存したりしない。strip/modal/dimmed等による抑止も実状態から判定する。
非360動画でzoom stateが存在することを確認し、+120の1回がscale 1.0→1.2の1段だけに
なることを検証する。単にscaleが増えた条件では二重処理も成功になる。
panは動かせる倍率へ上げてdown→move→upを通し、center変化、scale不変、再生toggleなし、
drag state解放を確認する。Appへの送信成功とsource epoch検査後の実適用receiptを区別する。

実装前の独立調査により、setupボタンの完了境界を区別する。
panoramaと右calloutは実ResponseからApp commandを生成する一方、左calloutと
bulk bookmark dialogの開閉はoverlay内の状態だけを更新する。全ボタンにApp commandの
receiptを要求する前提は採用しない。実Responseとtagged Upの対応を確認し、正常な
render_once終了後のoverlay状態を観測する。App commandがある操作だけ、下記のApp完了も
照合する。診断の都合で通常commandや状態変更経路を新設しない。
今回のpanel確認は、実状態がClickToShowかつinfo_panel_locked=falseであることを
入力前後に確認する。明示openは映像に重ねるだけで、左panel/dialogも映像の予約幅を
変えないため、S3aのcanvas region・ppp・client extentの厳密一致は維持できる。
ボタンやdialogが現れる/消える通常変化を含むwidget一覧のrevisionは、このcanvasの
geometry versionとは別物であり、操作後の一覧全体一致を追加条件にしない。

Appまでの証拠は、既存SequencedNativeOutputEventに診断feature限定の不変な
token・実入力payload・output/source/placement/host identity・dispatch IDを保持して運ぶ。
通常のlatest-slot分類・置換・sequence順序は維持し、eventを別variantで包んで分類を変えない。
実入力から生成したcommand/rawの対応を確定した後、同じbusへ診断tailを送り、Appが
そのtailへ到達した時点で期待dispatch全件の実処理を照合する。fresh UI barrierだけでは
途中drainの完了を証明できない。latest-slotによる上書き、source拒否、batch途中のclose、
tail未達を成功に補完しない。空のdispatch集合を許すのは、実renderの処理結果から
Appへ配送しないことが確定した負例だけである。

Appの観測scopeはAppを借用せず、通常のidx/parked/source gateの拒否理由と、通常handler
復帰後のexact mounted owner/source・zoom・pointer latchのbefore/afterを記録する。
zoom handlerのboolはgeometry不在でもtrueになるため、適用成功とは扱わない。
未分類の早期returnは診断失敗にする。zoom stateはContextRefからMounted/AtRestを
読み分けて観測できるが、App全体のpointer latchを任意の窓の状態として公開しない。
同じsource epoch 0でも別outputは別物であり、prepared output/host identityも照合する。
これらは設計合意で、実装・App統合試験は未完了である。

buttonを使う段階は、runner側の入力driverがDown/UpのSendInputとtyped transactionを
単独所有する。アプリ側はprepared target・fresh owner検査・実配送receiptを提供する。
SendInputがDownを1件挿入した時点でdriverがUpの後始末を所有し、Down receiptを待たない。
アプリ内送信後に外部へ通知する案では、Down挿入直後・通知前にrunnerがアプリをKillする
隙間が残るため採用しない。アプリ内RAIIと短いstep期限だけでは、既存runnerの強制終了経路を
覆えないという独立レビューにより、当初のアプリ内所有案を修正した。
runnerのtimeout/finallyはdriverのcleanupを先に行い、アプリの異常終了やIPC切断後も
driverが自分の挿入済みDownを解放する。外部所有はbuttonに限定し、move/wheelは既存の
アプリ側SendInputを維持する。専用の一時named pipeとrunner内C#処理threadを使い、
current SIDのDACL・remote拒否・first-instance・双方のexact PID検査を行う。
PS5.1/PS7互換のCreateNamedPipeW→SafePipeHandle→NamedPipeServerStreamを候補とする。
要求は長さ制限付きのversion/session/gesture/step/固定target/point/tag/期限だけとし、
同時1件・再送なし。アプリは実WMが返信より先着する場合に備えてreceiptを先に登録する。
外部driverは直前のPID/HWND/foreground/cursorを確認するが、logical context/source世代は
Windowsから認証できないため、アプリ側のfresh barrierとreceipt後照合を維持する。
Down後・返信前のアプリ終了、EOF、runner timeoutとの競合、挿入0件、Up失敗、
期限切れ・重複要求・異PID・解放後の重複cleanupを回帰対象とする。
この構成は親と独立Astraの実装前レビュー済みで、実OS入力への統合・検証はまだ行っていない。
外部helperのignored草案は、bounded wire/current-SID local pipe/固定process handle/
単独owner/非blocking返信とpure reducerを接続し、PS5.1/7各16群のfake・自process pipe試験を
独立レビューした。queue飢餓、app death後のcleanup期限停止、最後の肯定証拠で終了検査を
飛ばす3経路を修正済み。実SendInput・OS observer・Rust/runner接続はこの承認範囲に含まない。

2026-09-08の後続checkpointではtyped OS observerとWin32 backendをignored草案へ追加し、
独立レビュー・PS5.1/7各36群のfake/自process pipe試験を完了した。
通常送信はfacts取得後にも実ownerの生存・期限を確認し、Refused/Calledを区別する。
挿入countを既存reducerへ記録してからDPI等の復元失敗を扱い、cleanup義務を失わない。
残留last_errorだけでcount 1を失敗にしない。拒否/挿入0の理由は既存の最初の失敗へ保持する。
これは同一点clickの草案承認で、実SendInput・Rust/App・runner host接続は未実装である。
期限/owner変更/元HWND退去でも
この義務を捨てず、通常操作とは別の短いcleanup期限で移動なしのglobal LeftUpを試みる。
cleanupで元の失敗を成功へ変換せず、AppのZoomPanを診断コードで直接resetしない。
正常完了は実Up配送・pump側のcapture解放・UI handler後のZoomPan解放を揃える。
workerのGetCapture/ReleaseCaptureは別threadのcapture確認/解放の代用にならない。

2026-09-10に、上記ignored草案から外部button owner/pipe/hostを
`scripts/ui-smoke/button-helper/`へ追跡可能なcheckpointとして移した。host草案はこの時点まで
実行・独立検収されていなかったため、current-SIDの自process pipeで正常完了、App death、EOF、
reader/protocol fault、second Begin、blocked/faulted reply、固定join期限、未解決releaseを追加確認した。
terminal replyのtransport失敗をSucceededに残さず、runnerが先に観測したApp timeout/非0終了も
helper成功で上書きせず、後続runner接続では先行失敗の理由文字列も正本として保持する。
cleanup Upは古いtarget/foreground/cursorを要求せず、helper threadと
開いたinput desktopが現在の入力desktopであることだけを送信区間中に再検証する。PS5.1/7で
reducer 16件とhelper 54件が成功した。manifestは
`target/next-version-work/logs/button-helper-host-checkpoint-20260910/manifest.json`。
このcheckpointはnative inserterを構築せず、実SendInput、Rust/App receipt、
`scripts/ui-smoke.ps1`へのscenario接続を完了したとは扱わない。

2026-09-10の次checkpointでは、実`native_top_panorama` Responseから既存
`NativeOverlayCommand::TogglePanorama`が生成されたcommand indexだけへ、`test-script`
限定のbutton dispatch metadataを付けるRust側producer→App経路を実装した。frame内の
command数や後続の状態値から操作を推定しない。実WndProcで確認したtoken・HWND・process・
thread、prepared時のnamed token、output/source/placement/generation、host/presenter、
overlay ownerを同じlossless `SequencedNativeOutputEvent`で運ぶ。通常のTogglePanorama variant、
分類、source/current/ParkedLive/VST gate、handlerは変更せず、そのhandlerが完了した直後の
非360状態`panorama=false, zoom=None`から`panorama=false, zoom=Some(1.0)`への遷移だけを
当該gestureのApp receiptとする。gate拒否、別gesture、同frameの無関係Toggle、既にzoom中の
逆遷移、Responseなし/disabled、present前のlogical pass、次frameへ残ったtagは成功にしない。
このcheckpointは通常/`test-script` core check、feature限定のbroker・実egui Response・
logical batch・output bus・通常App handler回帰を非対話で確認する範囲である。
このproducer checkpointではRhai API、`scripts/ui-smoke.ps1`、外部helper hostとの起動/終了接続を
まだ公開せず、実SendInputとportable liveも未実施だった。従って、このcheckpoint単体を
「実click確認済み」や「zoom確認済み」には読み替えない。

2026-09-10のrunner接続checkpointでは、`NativeTopPanoramaClick`のRhai APIとscenarioを追加し、
既検収のhelperとproducerを`ui-smoke.ps1`から接続した。runnerは一実行だけのpipe名・session nonce・
自processのserver PIDをportable Appへ継承し、起動後に得たexact App PIDをhelperへ渡す。
App clientはserver PID、helperはcurrent SID/local-only pipe、session、App PIDとprocess creation identityを
相互に照合する。Rhaiはhidden top barの実MouseMoveからenabledな実Responseを取得し、その同じ
named token/control centerへ一回だけLeft Down/Upを要求する。成功には両stepのWndProc/process/thread・
pump/render、Response command index、通常TogglePanorama handlerの`None -> Some(1.0)`、pointer/drag解除、
helperの`ConfirmedReleased`がすべて必要である。

runnerのfinallyはhelperの`CancelAndJoin`を先に実行する。`StillRunning`、release `Unknown`、
`ConfirmedOutstanding`ではexact portable Appのkillも許可しない。helper成功はApp timeout・非0終了・
prepare/script失敗のexit/phase/reasonを上書きしない。helperがDown前に止まった状態と、Down後に
解放を確認した状態も`ReleaseState`で分けてrun metadataへ残す。PowerShellのpure policy、loader、
approval guardとRhai compileを非対話で検証する。実SendInputを使うportable liveは別途具体的な
了承を得るまで未実施であり、この接続完了だけで実click/zoom PASSとは扱わない。

通常操作の現在target検証と、既に挿入したDownのcleanup解放証明は型で分ける。
cleanupの権限は保存済みのtagged/validated Downとown Up挿入から取り、元HWND消失後も
別windowへtargetを付け替えない。現在のhelper-thread input desktop/accessとphysical Upを
改めて検証できる場合だけ解放証拠を作り、現在foregroundはアクセス確認の文脈に限る。
旧target生存・現在captureNoneをcleanup証明の必須条件にはせず、未知のaccessやdesktop変化は
Unconfirmedとする。historical validated Downなしの0値やSendInput成功だけでは証明しない。
初期observerはswap=trueをMappingUnsupportedとして送信前に止める。SendInputのswap時の
物理対応は未実証であり、これは診断fixtureの範囲制約で、通常アプリの操作仕様は変更しない。

開始時は既存mouse buttonとmodifierの非押下を確認し、実modifierを補正するkey-upは送らない。
観測した介入や前提不一致は環境不成立とする。ただしOSのglobal button状態に「テスト分だけ」を
差し引くAPIはなく、短時間の他アプリ向け実入力までポーリングで完全監視できるとは表明しない。
今回の実入力確認は前面windowとmouseを使用する対話的テストとして扱う。
根拠: [SendInput](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput)、
[GetCapture](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getcapture)、
[ReleaseCapture](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-releasecapture)、
[GetAsyncKeyState](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getasynckeystate)。

独立Astraのコード調査で、canvas wheelから`VideoZoomWheel`を生成する一方、
`render_if_dirty`の`consumed_wheel`判定に同variantがなく、raw wheelもAppへ送られる
二重適用の可能性を発見した。親も両App handlerへの経路を確認したが、まだ実行による
再現結果ではない。S3a観測実装へ無検証で混ぜず、実handler回帰とliveの1段確認で根因を
検証してから独立した修正単位にする。frame単位のwheel消費が複数eventに与える影響も調べる。
当初はzoom/audio modeが同batch内で変わらないため混在時も領域操作を維持できると推定したが、
追加の独立調査で反証された。strip wheelをeguiへ積んだ後にcanvas wheelがpointer_posを
更新すると、draw_native_seek_stripは最後のpointerだけを使うため、先のstrip wheelを
消費できない可能性がある。panelにも最終hoverへの集約が及ぶ。親もコード経路を照合した。
これは既存360/通常navigationにも及ぶ別の根因であり、Zoom variant追加で解消するものではない。
最小classifier修正の再レビューでは、同batch内mode一定の下で、全wheelにcanvas semanticか
egui/local ownerがあり、App rawだけが正当な処理先になる兄弟がないことを確認した。
今回の追加判定はpending egui入力を削除しないため、新たな正当入力の消失は見つからなかった。
これは既存のegui最終pointer問題が解消したという判断ではない。
S3の各stepは一入力ごとにreceiptを待つ。混在batchの領域操作を保証したとは表明せず、
追加調査を [next-release-backlog.md](next-release-backlog.md) §1.199へ分離する。
frame全体のany-commandは個々のtokenの処理理由ではなく、
receiptに流用しない。
また既存seek_strip_wheel_is_consumed_and_becomes_one_range_stepには#[test]がなく、
直前の別テストに#[test]が重複していた。既存テスト名の存在を実行済み証拠にしない。

二重zoomの回帰はproductionのwheel dispatchとcommand→routing組立を通して
should_forward_to_ui(original wheel)を判定し、手作業でconsumed_wheel=trueを与えない。
App commandは既存disconnected player/OverlayInputRouting fixtureでsource gate後の
1.0→1.2とstale epoch不変を確認できる。raw wheel側の座標変換は実HWNDを要求するため、
その全経路はportableの実1notch→1.2で補完する。GPU必須のoverlayを不正初期化しない。

strip/panel/modalの負例は、実領域内で指定tokenが処理された証拠、hit-testの理由、
zoom/raw-wheel/navigationの非発行、Appの同context/source/zoom不変を揃える。
strip rangeがWholeの仕様上no-opをスクロール機能成功とみなさず、Window spanの段変更を
確認する場合はその前提を明示する。

### 上部ボタンの観測契約（hover-only実装済み・live確認済み）

親Astraと独立Astraのsource照合により、最初の対象を上部hover入口と
`native_top_panorama`に限定する。既存catalogへ実renderの観測を加え、別のglobal widget表は作らない。
契約は「同じsource/host ownerの下で描画された実Responseと、現在も一致する入力条件」。
metadataや保持中のzoom値が新sourceから生成されたこと、クリック処理、GPU出力は保証しない。

実owner stampを初期・候補ctorの描画前、source切替後の描画可能setterより前に渡す。
overlay自身がArc markerを所有し、catalog/準備済みtargetはWeakで同一性を照合する。
既存の数値IDはchecked非再利用が保証されていないため、その性質を仮定しない。
候補の観測はcommit後のexact joinで初めて公開する。ctor後に対象状態が変われば旧frameは
利用不可、同値なら元stamp/frameを保ったまま公開できる。旧sourceのframeを新sourceへ付け替えない。
bind後の再描画待ちだけではpaused/cleanが固着するため、bootstrap/resizeを含む全render入口を覆う。

位置は実Responseから採取し、明示enabled引数・actual click sense・rect/interact rect/layer/clipを保持する。
既存button helperは無効時にSense::hoverへ変えるので、Response.enabled()だけでは不十分である。
対象状態の比較は寸法/ppp、Unknown/Panorama/NonPanorama、audio、pose/zoomの存在、
実chrome・dim・modal・重なりの入力条件に絞る。右固定配置に無関係な時刻・filename・
metadata全体は比較せず、通常の再生tickでtargetを失効させない。
Unknownからmetadataが届くと通常setterがdirtyを立てるため、診断repaintを足さずenabledを待つ。

hover入口は通常の非表示時36ptの領域を共用し、表示後の76ptへの変化で自分自身を失効させない。
named targetはfinal passの実inventoryを使い、非表示・再出現時には同じ矩形でも新tokenを発行する。
frame revision、named geometry token、既存canvas geometry versionは別の意味を保つ。
App/current-source receiptと実OS point/layer確認は後続層であり、Responseの存在だけでは入力成功にしない。

2026-09-10にhover-onlyの最初の閉じた単位を実装した。`test-script` feature内で
Canvas・TopHoverActivationを別のpoint/containment型としてprepareからpump/render receiptまで
維持し、上端36ptのMouseMove完了後に、同じowner/source/hostでenabledになった実
`native_top_panorama` Responseを待つ。Responseは`ui.interact`直後の明示enabled、sense、
rect/interact rect/layer/clipを保持する。final logical passだけをsurface present成功後にcommitし、
ctor bootstrap・通常/overlay-only resize・event batch・tickは同じcommitted inventoryを更新する。
source切替は旧inventoryを新epochへ結合する前に失効させ、overlay ctorの各試行は別Arc markerを持つ。
通常再生tickではnamed tokenを維持し、非表示からの再出現では更新する。診断用repaintは追加しない。
hover準備は`native_top_panorama` Response観測自体がないhidden基線だけを受け入れ、visible-disabledを
hiddenとはみなさない。MouseMove後はowner/source/host/activation areaを厳密に保ったまま、実presentで
Responseが現れるinventory版進行だけをreceipt完了時に許す。送信前はhidden基線の版完全一致、receipt時は
tagged final passでの版進行とResponse出現を必須とし、後続tickの出現で成功を代用しない。
Canvasはchrome inventory版から独立し、
従来どおりsource/hostとcanvas geometry/versionを厳密に照合する。touch初回helpとnormalize scanningも
前面blockerとしてnamed targetを公開しない。
`NativeTopPanoramaHover` scenarioが送る実OS入力はこのMouseMove 1件だけで、click/wheel/key/panは
後続単位に残す。非対話検証と独立review後も、明示了承を得たportable liveが終わるまで実機PASSとは
扱わない。

## 検証記録

S3aのraw識別子診断は、WndProc入口と既存metadata照合位置でGetMessageExtraInfoを
別々に読み、元の照合位置・完全一致条件を維持する。固定8件の記録は標準Mutexの
try_lock一回で所有し、UI/pumpで待たない。reset・記録・snapshotのowner tokenを照合し、
snapshotをコピーしてlockを解放してから整形する。競合やpoisonによる欠測も明示し、
記録なしを無配送と断定しない。独自unsafe排他や別のpending状態は追加しない。
Microsoftの[MOUSEINPUT仕様](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-mouseinput)
はdwExtraInfoをULONG_PTRとしている。2026-09-08の隔離liveでは、送信値
`0x4d49565300000001`に対し要求座標のWndProc入口・既存照合位置がともに`0x1`だった。
受信値が送信値の下位32bitと一致したことは実測済みで、OS内部の変換箇所は未特定。
2026-09-08の修正後liveでは`0x4d490001`/`0x4d490002`の2点が、
送信値との完全一致と要求/実座標一致でWM・pump・renderを通過した。
送信token自体を32bit内の共通prefixとchecked単調serialへ収める。
0・周回・失敗時の番号返却を禁止し、枯渇時は送信前に失敗させる。
受信値のmask比較へ変更せず、実際の送信値との完全一致とowner/sourceの照合を維持する。
このprocess内での非再利用は、process再起動をまたぐOS packetの非再来まで保証するものではない。

S0はscript実装・静的検証・独立レビューと実artifactのbuild/prepareを完了。
S1aは実装・焦点23テスト・feature有無のcore check・独立レビューを完了。
S1bは対象配送とbackend host観測を実装し、独立source reviewを完了。
最終検証はfeatureあり対象37件、featureなしlib test対象29件、manager関連7件、
backend witness 3件が成功。通常/診断core checkと通常dependencyへのfeature非混入も確認。
対話desktopでのMultiWindowPdf run4はexit 0。複数窓PDFの自動確認は成功した。
S2は実egui probeでmultipass前提を修正し、`fd0f94b89`で本体を実装した。
独立最終レビューとpointer_input 12件、key_input 20件、test_script 36件、
still_seek 61件（別に1件ignored）、feature/default core check、root cargo fmtが成功。
filter間には重複があるため件数を合算しない。static fixture/Rhaiとportable liveは別工程である。
2026-09-08の最終StillStripDragはexit 0、複数窓PDF回帰もexit 0。
量子化とRhai関数scope、KeyAction ackの誤った前提を修正した経緯・証跡は優先作業台帳へ記録した。
S3aはnative基盤に続きRhai接続・fresh UI owner validationを実装し、独立source review、
通常/feature core check、owner/classification/期限/worker token回帰を完了。
S3aの初回対話liveは別窓video表示まで進み、診断側の初期epoch0誤判定で入力前に停止した。
その後のtoken幅修正とPowerShell間fingerprint修正を経て、2点moveの最終liveはexit 0。
静止画列dragは上記S2の最終liveで自動確認済み。動画zoomは未完了であり、
S3aのmove配送確認だけでzoom・button・panの完了を主張しない。
S3bの上部hover入口と実Response観測は2026-09-10に実装した。`NativeTopPanoramaHover`の
Rhai/runner接続とfeature限定回帰を非対話で検証し、test-script付きportable artifactも準備した。
同日の非対話検証ではmain 8029件（43件ignored）、UI snapshot 48件、vendor 25/9/15件、
PowerShell 5.1/7 parserとapproval guardを通過した。
2026-09-10 10:16〜10:18の承認済みDefault desktop実行で、MultiWindowPdf、StillStripDrag、
NativeMouseMove、NativeTopPanoramaHoverの4項目がすべてexit 0となった。静止画列は実アプリへの
synthetic egui pointer、動画は実Windows MouseMove（2点配送と上HUD表示用1点）で確認した。
上HUDは同じowner/sourceの実enabled Response、位置・clip・DPIまでを検証した。
ボタンclick、zoom wheel、pan、thumbnail pixelの成功へは読み替えない。
先行7runはCodexの隔離desktopと実入力desktopの不一致で入力前に停止しており、製品不合格とは
分けて証拠を保持した。実行方式を改める前に追加了承を取得し、同じsource/artifactを再利用した。
最終証拠は`target/next-version-work/logs/ui-smoke-default-desktop-suite-20260910.json`
（SHA256 `387A045F05C538A8E4BC2D8A3E2ECA374DDC41C074CC384373408F5D6964CCE6`）。
Idle198Convergenceは通常版の索引更新負荷が継続しているため、このsuiteでは未実施。
実行結果と到達した経路は段階ごとに作業台帳へ記録する。

# 実アプリ smoke の自動化 (§1.197)

2026-09-07。担当・検証台帳は [v3.7.0-priority-work.md](v3.7.0-priority-work.md)。
既存のテスト・実機確認・通常データ保護を維持し、検証できる入力経路を段階ごとに増やす。
各段階は独立レビューを受け、未実装の段階を自動化済みとして数えない。

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

既存の`page-turn-smoke.ps1`はnormal coreを起動する経路があるため、その起動処理は流用しない。
通常profileの設定・画像・DBをsmokeデータへコピーしない。
現`lib.rs`は`--test-script`実行時にsingle-instance取得・既存instanceへのactivate/open-path
配送・listener起動を省略する。runnerはこの既存経路と明示的な使い捨てdata-dirを維持し、
利用者の別portableへfixtureを転送しない。新しいinstance名前空間は追加しない。

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
- `Keymap`の5つのconsume/peek入口は既に`&egui::Context`を受け取るので、そこから
  test-script consumerへctxを伝える。`keyboard_owner_for_pass`の既存pass境界を使って
  実行中の論理ownerを公開し、Targetedのownerと一致するhandlerだけに渡す。
  兄弟のconsume/peekはTargeted要求・ackを変更しない。Legacyへこの新規条件を重ねない。
- passive detachedのactivationは、既存`queue_deferred_detached_window_activation`と
  `commit_pending_deferred_detached_window_activation`を使う。既存intentはwindow IDだけを
  保持し、commitは全pendingの最小IDを選ぶため、直前queueだけでは対象commitを保証しない。
  test requestはtyped待機状態で保持し、Closingを含め既存intentが一件もないdispatch機会で
  exact claimを再検証→queue→通常commitする。同じUI処理内で行い、actual ownerも照合する。
  既存intentは消去・並替え・追越しをしない。commit失敗で再queueせず明示失敗にする。
  close/transfer後に最新bindingへ付け替えず、独自mount/session変更を加えない。
- Targetedには従来の新ROOT frameごとのexpiryを適用せず、passiveのactivation/child処理を待つ。
  受付後の対象ownerの最初のeligible passを基準に未消費失敗を判定する。pass開始を
  完了と誤認せず、対象handlerが処理する機会を持った後に判定する。対象が消えた場合は
  staleで終了する。待機状態は一つのtyped request ownerへ集約する。
  ROOTは既存`update_frame`の外側、childはcallback scope終端を使い、対象handlerに機会の
  あったpassだけ完了を観測する。cached `KeyboardOwner`の早期returnでも論理owner観測を省かず、
  その戻り値（focus等の許可区分）を新しいdirect action許可条件として使わない。
- cancel/environment failure/finishではdirect actionのackも一度だけErrで解放する。
  script由来の長寿命activation intentをmanagerへ残さない。配送ackと操作結果は区別し、
  PDFシナリオはその後のpage/full-paintと兄弟不変を待って判定する。

回帰にはLegacyの従来consume/peek/expiryの維持、Targetedへのfallback禁止、
兄弟の非消費、exact対象だけの消費、close/transfer、activation前stale、
対象pass基準のexpiry、取消時のack一回解放を含める。既存Grid等のfocus/permit guardを
迂回する意味ではない。上記のLegacy/Targeted分離とdispatch境界は独立Astraレビューで合意済み。

## S2: egui pointerと名前付き矩形

既存の合成キーと同じviewport指定・timelineへ、move/button/wheelを載せる。
矩形は実際の描画ownerがcontext/viewport・frame/revision・pixels-per-pointとともに公開し、
scriptは領域名と正規化位置を指定する。テスト用に別レイアウトを再構築しない。
press、drag閾値を超すmove、継続moveを別frameにする。最終stepは同一batch内で
`PointerMoved(final)`→`PointerButton(up)`を送り、release frameで変化した最終座標も検証する。
各stepは対象frameの処理を待つ。待つのはscript workerであり、UI threadにblocking waitを追加しない。
LTR/RTL、列中心とページ着地の区別、release時だけ動いた最終位置を検証する。

実装前提調査で確定した境界:

- stripは外枠ではなく実セルunionに対する`fullscreen_still_seek_strip_row`の`Response.rect`。
  page seekは`fullscreen_seek_track`の実hit rectを別regionとして公開する。
- press時のrectとexact targetをgestureへ保持する。動く列の最新rectへ毎frame再正規化して
  pointer deltaを変えない。owner/hostが変われば最新窓へ付け替えない。
- 注入口は既存`SyntheticInputPlugin::input_hook`のviewport batch。配達済みviewportは
  widget処理済みではない。実handler・paint・同frame後段のnavigationが完了したowner tailで
  input tokenとpost stateをackし、列中心の移動とpage変更を区別する。
- key専用timelineのidle/cancel/finishへpointerのheld button・queued/in-flight stepも統合する。
  cancel時は生存するlatched targetへのButtonUp、または閉鎖済みtargetの明示的なterminal処理を
  行い、egui側を押下中のまま放置して成功としない。

## S3: native入力の継ぎ目

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

## 検証記録

S0はscript実装・静的検証・独立レビューを完了。実artifactのbuild・起動は未実施。
S1aは実装・焦点23テスト・feature有無のcore check・独立レビューを完了。
S1b～S3は設計段階であり、複数窓PDF・列drag・動画zoomを
自動化済みとは扱わない。
実行結果と到達した経路は段階ごとに作業台帳へ記録する。

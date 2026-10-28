# キー / マウス操作仕様 (静止画 vs 動画)

## 左右ドラッグによる連続ズーム

既定は Ctrl+Space を押しながら primary pointer（ペン／マウス左ボタン）で左右ドラッグ。
右で拡大、左で縮小し、押下位置を固定したまま開始時の倍率と pan から毎回計算する。
100 px で2倍／半分、開始しきい値は中ボタンドラッグと同じ4 px。
しきい値内へ戻すと開始時の倍率と pan に戻る。通常・分析・編集は0.1～50倍、
Z ズームは既存の0.05～16倍を維持する。360では入力だけを所有し視点は変更しない。

| Action | Context | Trigger | 既定 |
| --- | --- | --- | --- |
| `FsScrubZoomHold` | FsImage | KeyHold | Ctrl+Space |
| `EraseScrubZoomHold` | Erase | KeyHold | Ctrl+Space |
| `ConcealScrubZoomHold` | Conceal | KeyHold | Ctrl+Space |
| `CropScrubZoomHold` | Crop | KeyHold | Ctrl+Space |
| `TextScrubZoomHold` | Text | KeyHold | Ctrl+Space |
| `LaScrubZoomHold` | LocalAdjust | KeyHold | Ctrl+Space |

この6操作だけ `FullChord` policy を使い、通常キーと修飾キーの組合せを許可する。
従来の KeyHold は `SinglePlainKey` のまま。修飾は完全一致で、Ctrl+Space は
`FsSpaceCheck` を発火させない。Crop の Ctrl 併用パンも含め、共通 helper でズームを優先する。
描画・選択等のドラッグ中は開始しない。パネル上で開始せず、開始後はパネルを横切っても続く。
chord または primary を離すと倍率変更を終了し、通常・分析・編集の PDF は最終倍率で再描画する。
chord を先に離した場合も primary を離すまで同じ押下を所有し、描画・クリックへ流さない。
通常閲覧は静止画のキャンバスが対象で、動画・音声・音楽・ページ一覧は対象外。
TextEdit／IME／modal／非アクティブ窓の既存入力所有を優先し、detached の配送経路は変更しない。

右 Ctrl+Space では従来の `FsOriginalPreviewHold` も成立し、元画像表示を併用する。
入力方式によっては Ctrl+Space が入力の切り替えに使われるため、その場合は操作カスタマイズで別のキーへ変更する。
中ボタンと scrubby は種類・開始時の持ち主を持つ一つのドラッグ状態を共有し、同時開始しない。
組合せを減らすため、chord 解放後の再開・パンへの切替は設けず、同じ primary の終端まで所有する。

ページ一覧は `FsPageOverviewToggle`（FsCommon / Press、既定 O）で開閉する。
動画／音声／編集モードでは入場しない。S／Shift+S／Ctrl+S、Ctrl+O、編集の O は従来どおり。
受理済み移動が未提示の間はanimationのON／OFFにかかわらずOによる新規入場を受け付けない。
既存`fs_navigation_sequence_blocks_new_target()`と`page_turn_barrier_active()`を
既存`page_overview_available()`へ集約し、OpenとSelectで共用する。受理済み移動を保持し、
完了後にもう一度Oを押す。新しいpredicate・pending・replay・取消や一般lockの新案内は加えない。
HUD／リング／パッドも`open_page_overview_for_viewer`→`open_page_overview`の同じ受付へ通す。
入場要求の保留・自動再実行・新しい取消処理は作らない。Oの既存operation観測が通常flat完了へ
進める場合と、一覧入場の受付がownerを変更することは区別する（2026-10-27の新coordinator手順2、
一覧設計書§25.4）。旧選択肢2の即退役を採択した意味ではない。独立レビューP2追補の指定自動検証は同§25.7で成功し、
同§25.6の統合時の成功と区別する。

Oは既存のfocused Slideshow／overflowの「次のキーで閉じる」処理より後に判定する。
受理した共通一覧入場ではkeyboard／HUD／ring／gesture／padとも`dismiss_fs_transient_menu_state()`で
spread／fit／panoramaのboolとoverflowを終了し、physical streamを維持する。
keyboard次キーdismiss／HUD Close／HUD Overviewが共用するのはoverflowだけの`dismiss_fs_overflow_panel()`。
拒否されたring入場はchromeを残し、ring全体の一括resetと通常キーのdismiss規則の変更は行わない。
rotation／seek等のegui Popupは表示／keepされなければ所有viewportの`Memory::end_pass`で自然に退役する。
keyboardのseek ownerは従来どおりOを拒否し、非keyのring入場後はowner pass終端で自然に閉じる。
兄弟viewportへ影響する`Popup::close_all`やroot ctxへの明示popup closeは加えない。
context menu／modal／overlay編集／pickerの既存入場拒否と、info panelの永続lockは維持する。
fullscreen tag pickerも既存common modalに含む。通常adjustment panelは非focusedなら入場できる場合があるが、
front-menu captureには含まれず、focused TextEdit／IMEは既存keyboard ownerがOを止める。
metadata／infoのlock・明示表示・hoverと、非永続のhover tooltipは旧menu captureとして一括resetしない。
一覧中の scope は Global + FsCommon + FsPageOverview とし、FsImage／Rating は同時 active にしない。
一覧確定は `FsPageOverviewConfirm`（Enter）、画面送りは `FsPageOverviewPagePrev`／
`FsPageOverviewPageNext`（PageUp／PageDown）、先頭末尾は `FsPageOverviewFirst`／
`FsPageOverviewLast`（Home／End）、列数は `FsPageOverviewColumnsLess`／
`FsPageOverviewColumnsMore`（既定なし）。新規操作はすべて Press／no-repeat。

一覧の実行可能集合は `FS_PAGE_OVERVIEW_ACTION_PRIORITY` を正本とし、新規一覧操作、
`HelpShowContextShortcuts`、`FsToggleWindowMode`、`ToggleDetachedViewerMode`、
`ToggleAlwaysOnTop`、`FsCtrlNavPrev`／`FsCtrlNavNext`／`FsSiblingPrev`／`FsSiblingNext`
に限る。表示先と navigation の既存 gate でさらに絞り、独立静止画窓の F12 は含めない。
ヘルプ・ボタン・リング／ジェスチャ・パッドも同じ集合を使い、許可外の保存済み割当は実行しない。
`FsClose`／`FsBackToList`、背面の画像操作／評価／slideshow へ fallthrough しない。
一覧を取消した後は通常の割当へ戻る。

一覧のactive rendererも通常viewerと共通のpointer recognition／guide描画を通る。
右ドラッグring／gestureと中ボタン短押しは既存の認識・取消・primary suppressionを使い、
保存済み割当を上の有限集合へdispatchする。X長押しringは一覧中も案内を描き、
許可slot／「無効」slotのlabelとrelease時の実行gateを一致させる。
activeのlabelは現在ownerの集合、passiveのlabelはそのimmutable `overview.executable_actions`を使う。
集合・scope・新規Actionは増やさず、短右click／画像long-press menu、中ドラッグzoom、
通常ページ送り・wheel pan／zoomを一覧へ流さない。
TILE11の接続修正・検証記録は[一覧設計書§23](detached-book-page-overview-plan.md#23-一覧で省略された共通入力案内の接続tile112026-10-13)を参照する。

修飾なし矢印は一覧 cursor の固定ナビ、Esc は一覧取消の固定操作で keymap 対象外。
どちらも通常 viewer と共通の固定入力／IME-safe 契約を保ち、TextInput／IME／modal を優先する。
背景クリックでは取消しない。OS／タイトルバーの窓 close は従来の terminal close。
入力優先は modal・text・popup → 固定 Esc → ヘルプ → toggle → F11／F12／最前面 →
Ctrl／sibling navigation → Confirm → その他の一覧操作／固定 cursor とし、一つの edge を一度消費する。
toggle を Enter に割り当てると Confirm との競合を表示し、実行時は toggle だけで一回取消す。
潜在的な競合は一時的な modal／nav lock によって消さず、一覧の全許可集合で検査する。

## 手動の前後移動ループ（§1.400）

`fullscreen_manual_nav_wrap`（既定OFF）がONなら、既存の手動前後移動Actionは現在の閲覧順・
表示対象の端で折り返す。新しいKeyAction・既定キー・トグル操作は追加しない。
`FsNextImage` / `FsPrevImage` / 縦方向版 / `FsPageNext` / `FsPagePrev` と、動画・音声の
`VideoNextFile` / `VideoPrevFile`、同じ入口を通るホイール・クリック・HUD・マウス割り当て・
ゲームパッドに適用する。連結読みでは項目をまたぐ静止画単位の明示移動だけが対象。
単ページ / 分割の `FsSpreadShift*` と `ImageSpreadShift*` は既存のsource単位Delta移動として折り返す。
見開きでの同操作は組み合わせ調整なので対象外。

`FsJumpFirst` / `FsJumpLast`、`FsFixedJump*`、スタックジャンプ、シーク、Ctrl+↑↓ /
Ctrl+PageUp/PageDown、連結方向のスクロール、自動送りは変更しない。再割り当てしたキーでも
Actionの意味に従う。対象が1表示単位で動けなければ境界案内を保ち、折り返した場合は案内を出さない。
Remoteの固定操作は対象外。設定と着地規則は [手動前後移動ループ設計](manual-nav-wrap-plan.md) を参照。

## 保存済み位置の全件クリア（§1.394）

`ClearVideoMediaMemory` / `ClearBookResumePositions` は独立した Global / Press 操作で、
どちらも既定キーなし。操作カスタマイズの「全体共通」に表示する。確認ダイアログは付けない。
一覧、画像フルスクリーン、egui 動画・音声、native 動画の既存 `ToggleAlwaysOnTop` と同じ
Global の keymap helper を使う。文字入力・IME・モーダル・編集モードの入力所有を優先し、
native 転送対象にも登録する。detached の述語・viewport 配送は変更しない。

動画側は `Settings::clear_video_media_memory` に対象を集約し、環境設定の編集値とコマンドの
live 値を同じ関数で消す。対象は再生位置、視聴済み記録、音声トラック選択だけ。
コマンドは件数（各記録別）または空を発火面へ通知し、その場では同期保存しない。
記録時と同じく終了時等の既存保存機会に永続化する。環境設定は従来どおり OK で反映、
キャンセルでは破棄する。古い draft の OK も最新 live を merge し、消した記録を復活させない。

App 共通の `media_memory_clear_generation` をキー消去と環境設定 OK の消去で進める。
空の消去も世代境界とし、draft のボタン・変更比較・キャンセルでは進めない。
`MediaResumeUpdate` は採取時の世代を保持し、通常適用と teardown 適用の双方で照合する。
旧世代から再生位置・視聴済み・音声選択や resume thumbnail の要求を復活させない。
対象外の閲覧履歴 progress 更新は世代によらず従来どおり行う。

| 採取元 | 世代と適用時点 |
| --- | --- |
| `poll_video` 定期採取 | native キー処理より前の世代を保持し、処理後の適用で検査 |
| `save_all_video_resume_positions`（表示中・終了時の保存） | プレイヤー採取時の現在世代、同じ UI 呼出し内で適用 |
| `viewer_context_media_teardown_plan`（別窓の破棄・終了時保存） | App の現在世代を採取関数へ渡し、保持した plan の適用時にも検査 |
| `apply_remote_media_resume_update` | session 検証後の受信処理で現在世代を付け、直ちに適用 |
| 音声トラック確定 | poll の tick 直後、明示選択の Unchanged、Remote の選択記憶はいずれも採取した UI 呼出し内で即適用。旧更新を保持しない |

音楽ビューの早期受付も既存 `music_normalize_modal_active` を使い、その内側の
`normalize_scan_is_modal_for_current_player` を native と共有する。音楽のしおりモーダルも
受付を止める。文字入力・IME は各入力所有者に任せ、native に root の文字入力状態を混ぜない。

本側は既存 `clear_book_resume_async` の writer FIFO、追加後続変更の受付、`Delta::Clear`、
`last_book_resume` の解除、失敗時のメーター再読込を共有する。完了の件数・空・失敗は
環境設定の結果欄、コマンドは発火コンテキストと面を完了票に保持し、既存
`with_viewer_context` 内から発火元の既存トーストへ通知する。root poll時のmainや兄弟の
native overlayへ送らない。発火コンテキスト退役後の通知はログに残して破棄する。
egui の完了通知は既存の App 共通トーストを使い、同じ面の別窓へ切り替えた場合は
表示中の窓に出す。egui の窓を追跡する状態は追加しない。
先行 clear の完了票が未処理なら、キーと環境設定ボタンのどちらも「クリア中です」と通知して
再要求を拒否する。先行の件数・通知先・変更受付状態を保ち、待ち行列は作らない。
閲覧履歴・しおり・構成・見開き調整・タグ・評価・編集・最近のフォルダ・A/B の場所は消さない。
消去後も通常の閲覧・再生による新規記録を続ける。

設計の簡素化: 新しい保存 worker・待機状態・モーダルは追加せず既存の所有境界を再利用する。
動画は live の変更だけ、本は既存 FIFO の clear だけなので閲覧を追加で止める必要はない。
まれな失敗は割り切り（ログ + 通知 + 既存の本メーター再読込）。保存前の強制終了は
通常の再生位置記録と同じ扱いにする。

Remote は本体の同じ live media 記録と BookResumeWriter を読み書きする。
本の再開読取は writer の受付済み操作後を読む。操作権によるローカル入力の既存遮断と
旧 session の write 拒否を維持し、Remote 専用クリア・IPC・キャッシュ無効化は追加しない。

`FsPageTurnAnimationToggle` は `FsImage` / `Press`、既定キーなしの操作。
静止画フルスクリーンの「その他の機能」の他のON/OFF行と同様にKeyActionを持ち、
操作カスタマイズでキーを割り当てられる。HUD行には割り当てた先頭chordを表示する。
通常窓/F12から環境設定と同じ全体設定 `page_turn_animation_enabled` を変更する。
メニューの並びは固定で、toolbar/HUDの表示項目カスタマイズ対象ではない。
動画・音楽には適用せず、新しい既定キーやリング操作は追加しない。

音楽ビュー上部の「…」→「音名の色」はマウス・タッチの固定入力で、keymap対象外。
低頻度の見た目の好み設定なので専用キーを追加しない。開いたメニューは次のキー押下で閉じ、
そのキー本来の操作を続ける。パネル内のホイール・Ctrl+ホイール・タッチ、および外側クリックで
閉じる入力は背面のタイムラインへ渡さず、閉じたフレームもその入力の所有を保つ。
パネル表示中に開始したドラッグやタッチは、キーで閉じた後もボタンの解放／接点の終了まで
パネル側が所有する。キー本来の操作は続け、新しく開始した操作は通常どおり受け付ける。

## 構成のある本の表示キー（§1.242 B2）

`FsViewSpread` / `FsViewLandscapeSplit`は閲覧方法だけを切り替える。既定キーなしでカスタム割当の対象。
`FsSpreadSingle`も構成本ではmode-only。2026-10-14利用者決定Q31により、既存の見開きActionも
構成の方向・表紙と完全一致する場合はmode-onlyとして受け付ける。既定は
`FsSpreadLtr` = 2（左→右・表紙なし）、`FsSpreadLtrCover` = 3（左→右・表紙あり）、
`FsSpreadRtl` = 4（右→左・表紙なし）、`FsSpreadRtlCover` = 5（右→左・表紙あり）。NumPadも同じ。
Action名と既定割当は変更せず、再割当したキーにも同じ判定を適用する。
不一致は構成を変更せず、現在のkeymapで一致Actionに割り当てたキーとPCの構成編集を案内する。
一致Actionが未割当なら`FsViewSpread`の割当、それも未割当ならHUDの「見開き」を案内する。方向付き横長分割Action（既定8/9）、
読む方向切替、見開きずらしは引き続き共通受付で拒否する。Single/Splitの通常ページ送りfallbackは維持する。
本別mode欄だけを保存し、構成/方向/表紙/休眠shiftを書き換えない。構成なしの既存割当は維持する。
HUDの「見開き」は構成の方向と表紙に一致するActionの割当・選択状態を表示する。
「横長分割」のshortcutは`FsViewLandscapeSplit`のカスタム割当だけを表示し、旧8/9を案内しない。
hoverも有効なActionだけを案内する。固定Pad Selectは構成本では単ページと構成に一致する見開きを循環し、
pickerも同じ共通受付を使う。通常本の五種類の循環と抽出一覧の単ページ/見開き循環は維持する。
Remoteの既存`SetSpread`も最新構成の方向・表紙と一致する見開き要求だけをmode-onlyへ通す。
不一致はWebの固定キー2/3/4/5のうち一致するキーとPCの構成編集を案内する。
WebはPCのkeymapを参照せず、IPC形状・既定キーは変更しない。
`FsTogglePageAfterCoverAlone` / `FsToggleLastPageAlone`は通常本と同じ自動規則の本別値を変更できる。
端配置・末尾表紙も既存HUD/preferenceで変更し、構成record/revisionへ保存しない。

## 抽出一覧の表示キー（§1.242 G1）

検索・評価・スマートフォルダ・コレクション・★固定・サブ展開の結果を直接見るときは、
新規openで単ページに戻る。既存`FsSpreadLtr/Rtl`とcover付きは一時見開き、
`FsSpreadSplitLtr/Rtl`は一時横長分割として受け、読む方向は全体既定からのsession snapshotを使う。
`FsReadingDirectionToggle`と見開きずらしは無効。Single/Split時のずらしキーは従来のページ送りfallbackを保つ。
キー/リング/パッドは既存KeyActionと共通表示setterを通し、結果の表示設定を記憶しない。
新Action・既定割当は追加しない。通常の本・通常一覧の既存キーは変更しない。

`GridOrganizeFiles` はサムネイル一覧のカーソル項目／チェック済み項目の「ファイル整理先」を開く。
既定キーはなし。操作カスタマイズからキー、リング（マウス／パッド）、マウスジェスチャ、
戻る／進む／ホイールクリックへ割り当てられ、右クリックの同じ入口と対象解決を共有する。
`RingActionId::GridOrganizeFiles` は一覧（サムネイル／詳細）だけで有効。整理先選択画面を開くまでで、
直接の移動・コピーはしない。新しい既定割り当ては追加せず、既存の割り当ても保持する。
整理先画面内の矢印、Enter（明示選択済み操作のみ）、Escape はダイアログ専用で、IME 変換中は操作しない。

`RingActionId::ImageFitModeCycle` は `FsFitModeCycle` に対応し、画像フルスクリーンの
リング（マウス／パッド）、ジェスチャ、戻る／進む／ホイールクリックへ割り当てられる。
ページ全体 → 横幅フィット → 縦幅フィット → 100%原寸 → 指定サイズ → ページ全体の循環をキーと共有し、
単ページ／見開き／縦・横連結で使える。動画／音声、一覧、編集モード中は実行しない。
新しい既定割り当てはなし。既存の `0` / `Numpad0` と保存済み設定は変更しない。

2026-10-08 利用者決定 (§1.343): 環境設定のズーム/フィット付近にある
「フィット循環に含めるモード」で、上記の順序から循環対象を選ぶ。既定は全モード有効、
最低1つは有効。現在の方式が対象外なら通常順の最初の有効モードへ、1つだけならその方式へ
設定／維持する。キーとリング／ジェスチャ／マウス3ボタン／パッドリングは
`cycle_fullscreen_fit_mode` → `FullscreenFitMode::next_for_flow` の一つの循環処理を共有する。
ツールバーのフィットボタンは循環ボタンではなく直接選択メニューで、全モードを表示し続ける。
各モードを直接選ぶ新しい割り当て操作は追加しない。

保存は `fullscreen_fit_cycle_excluded: Vec<String>`（除外するstable ID、serde既定は空）。
新モードは、対象集合を変更済みの利用者にも、IDが明示的に除外されていなければ有効になる。
未知IDは保持し、現在の候補には影響させない。読込時に全候補が除外されていたら、通常順の
先頭（現在はページ全体）を有効に戻す。環境設定の転送に含め、全候補除外の転送値は拒否する。
包含一覧や「利用者が変更した」フラグを併設せず、既存の環境設定draftと循環ownerを再利用する。
§1.342 の「指定サイズ」は `SpecifiedScale` のstable IDで通常順のOriginalの次へ追加した。
既存4方式だけを除外した設定では指定サイズが有効に残る。`FutureScale`など未知IDを改名しない。
幅・高さが両方未指定でも選択方式は指定サイズのままで、循環候補から自動的に外さない。
追加のキーActionや既定割り当ては設けず、指定値の編集は環境設定または既存fitメニューで行う。

初回設定の Enter はダイアログ専用の固定操作で keymap 対象外。選択肢/リンクにフォーカスが
ないときは現在の設定で「開始」を確定する。フォーカス中はその widget の操作を優先し、
IME 変換中はダイアログを確定しない。Esc では初回設定を省略しない。

mimageviewer のフルスクリーン操作におけるキー / マウス アサインの整理。
画像 (静止画 / アニメーション GIF / PDF / ZIP 内画像) と動画でアサインが
異なる項目を一覧化し、整合性 / 不整合を明らかにする。

フルスクリーン境界ヒント、Ctrl+S/F/G 検索スコープ、動画タイルモードまで含めた
横断仕様と現状差分は [fullscreen-navigation-consistency.md](fullscreen-navigation-consistency.md)
を参照すること。

一部のキーボード操作は設定メニュー「操作カスタマイズ…」で上書きできる。設定の正本は
`Settings.keymap` (`settings.db`) で、旧 `%APPDATA%\mimageviewer\keymap.ini`
が残っている場合は初回起動時に 1 回だけ取り込んで
`keymap.ini.imported*.bak` へ退避する。Action 名・書式・固定扱いの入力は
[keymap.ini.default](keymap.ini.default) と、コマンド設定画面の表示を正とする。マウス、ゲームパッド、
OS/egui clipboard、D&D、IME 確定、右クリックメニューは keymap 対象外。

`ToggleAlwaysOnTop` は Global / Press の操作で、既定キーはない。操作カスタマイズでキーを割り当てると、main・egui フルスクリーン・native 動画の発火面から同じ設定を切り替える。設定メニュー先頭のチェック項目からも操作でき、ON/OFF の通知は発火した面だけに表示する。

`UndoLastAction` は Global / Press、既定 `Ctrl+Z`。グリッド・画像ビューアでは、本への追加直後の
一回取消を先に扱い、有効な追加がなければ従来の評価・タグ・補正Undoへ進む。取消は今回作った
コピーだけをごみ箱へ移し、元ファイル・既存ページ・本フォルダを残す。追加のRedoはない。
後続の編集・ファイル操作・場所切替で失効する。選択・チェック・スクロールだけでは失効しない。
処理中または本の変更検出で取消を拒否した入力では、古いメタデータUndoを続けて実行しない。
消しゴム・隠蔽・テキスト等の編集専用Undoと、TextEdit / IME / ダイアログの入力所有を優先する。
メタデータRedoの `Ctrl+Y` / `Ctrl+Shift+Z` は固定入力のまま維持する。

ゲームパッド全体は `Settings::gamepad_enabled` で無効にできる (操作カスタマイズ →
設定 → ゲームパッド)。これは割り当ての変更ではなく**デバイスを読むかどうか**の設定で、
無効の間は `GamepadRuntime` が読み取りスレッドごと止まる。読み捨てるだけにすると、
スティックのずれで `request_repaint` が飛び続けて UI が起きたままになる。
マウスジェスチャ / リングフリックは対象外。

マウスとゲームパッドも原則 keymap 対象外だが、例外として、マウス右ドラッグの
フリック、マウス戻る / 進む / ホイールクリック、ゲームパッド <kbd>X</kbd> リングは
`Settings.ring_shortcuts` で厳選アクションだけを差し替えられる。
これは `KeyAction` / コマンド設定の完全カスタマイズではなく、マウス・ゲームパッド用の
固定入力レイヤーで扱う。
レーティングは専用の `[Rating]` グループ (`RatingItem1..5/Clear/StepUp/StepDown`、
`RatingContainer1..5/Clear/StepUp/StepDown`、`RatingBookPreferred1..5/Clear/StepUp/StepDown`) で、グリッド / 画像フルスクリーン / 動画フルスクリーンが
同じ割り当てを共有する。v2.2.0 からは物理キー寄りの `KeySlot` を正本にし、通常の数字キーとテンキー数字を別キーとして扱う。互換性のため、従来の数字キー既定操作は `1` と `Numpad1` のように両方を既定割り当てにする。
`StepUp/StepDown` は標準キーなし。未評価は操作時だけ ★3 を基準にし、上げると ★4、下げると ★2 を保存する。★5 から上げる / ★1 から下げる操作は書き込み・Undo を作らない。チェック済み項目は各自の現在値から一括で変化し、1 回の Undo で戻せる。コンテナ用は直接指定と同じく現在開いているフォルダ / ZIP / PDF 本体が対象。
「本を優先」の8操作はすべて標準キーなし。一覧ではアイテム用と同じく選択 / チェック項目を評価する。
画像閲覧・動画・音楽では、採用済み `ProjectionRecipe::Book` のときだけ開いている本全体を評価し、
通常一覧・抽出結果の閲覧では表示中のアイテムを評価する。本の★は親一覧の本のセルと同じ保存先を使う。
本の recipe と物理状態が一致しないときは書き込まず「この本は評価できません」と通知する。
既存アイテム用 / コンテナ用の対象規則と標準キーは変えない。詳細は [§1.417 の設計](book-preferred-rating-plan.md) を参照。
評価変更で表示条件から外れた場合、グリッド選択は現在の表示順、フルスクリーンは現在の読書順で次の残存対象、なければ前へ移る。読む順の「次」は RTL でも左右キーの向きで反転しない。ページがゼロなら現在の絞り込み済みページ一覧へ戻り、フィルタを解除しない。自動移動・一覧復帰で評価通知と評価 Undo を消さず、直後の `Ctrl+Z` は評価を戻して、同じ一覧・条件で対象となる元画像へ復帰する。通常の手動移動・close の履歴終了方針は維持する。
マウス / パッドのピッカーの★は保存済み live preview を維持し、その preview だけで離脱する既存行を終了まで残す。現在ページと各行の preview は維持するが、新しい検索 hit・件数・明示 sort は通常どおり更新する。終端で最終成功値の一件 Undo を記録し、次 / 前 / 現在一覧の移動を一度決める。新しい `KeyAction` は追加しない。所有・復帰不可・取消の詳細は [§1.371 の計画](rating-filter-fullscreen-plan.md) を参照。
本体 <kbd>Enter</kbd> と `NumpadEnter` も別スロットであり、押下操作、KeyHold、native 動画、
操作カスタマイズの「押して入力」の全経路で相互に発火しない。Windows では両者が
`VK_RETURN` を共有するため、Win32 `WM_KEYDOWN/WM_KEYUP` の extended bit を物理種別の
正本にする。egui 0.33 の `physical_key` は両者を同じ `egui::Key::Enter` へ畳むため、
この 2 スロットの照合には使わない。旧 `keymap.ini` と現行設定の表記は従来どおり
`Enter` / `NumpadEnter` で、migration や破壊的な表記変更は行わない。
frame-active な viewport (subclass 登録済み = Win32 キューを持つ) では、そのキューに該当 edge が
無いことが「押されていない」の答えになる。egui が畳んだイベントで代用しない。同じ物理押下の
egui イベントは main と fullscreen の両 viewport へ届く一方、Win32 edge は届いた側にしか無いため、
フォールバックすると edge の無い viewport が別スロットの押下を拾う (NumpadEnter でフルスクリーンが
閉じた)。対象は畳まれ先のスロット (`Enter` / `Num0`-`Num9`) に限る。
KeyHold が Win32 edge を消費したときは、同じ物理押下から egui が生成したイベントも同時に
claim する。2 つのキューは同じ押下を表すので、片方だけ消費すると残ったほうを次の読み手が拾う
(`NumpadEnter` を KeyHold へ割り当てると、egui が畳んだ `Key::Enter` を `FsClose` が拾って
表示が閉じていた)。照合に使う egui Key (`to_egui`) と、claim に使う「畳まれた先」は別物として扱う。
KeyHold の押下中状態も viewport に対応付いた Win32 ラッチだけを正本にする。新規 viewport の
subclass 登録前など、対象 viewport の送信元付きラッチを確認できないフレームでは、
`Enter` / `NumpadEnter` を未押下として扱い、物理種別と送信元 HWND を失う
`GetAsyncKeyState(VK_RETURN)` へフォールバックしない。固有 VK をそのまま識別に使える
その他の KeyHold は従来の OS 状態参照を維持する。
OS 予約ショートカット (例: Alt+F4 / Alt+Tab / Win キー系) は keymap では上書きできない。
`Esc` と修飾なし矢印ナビゲーションは、モード脱出と閲覧の最低限の固定入力として keymap 対象外にする。
Enter / Backspace / Home / End / PageUp / PageDown などの閲覧操作は文脈ごとの `KeyAction`
として扱い、コマンド設定・競合検出・ヘルプ表示に載せる。マウス戻る / 進む / ホイールクリック、OS クリップボードの
貼り付けや Shell 連携 (パスコピー / フォルダを開く) は入力経路が異なるため固定入力レイヤーに残す。
標準キーを持たないが割り当て可能な操作は `keymap.ini.default` に
`# Action = none` として列挙される。コマンド設定または旧 `keymap.ini` 移行でキー名を指定すると割り当てられ、
`Action = none` を明示した場合は無効化として扱う。
コレクションへの追加は `GridAddToCollectionTarget` / `FsAddToCollectionTarget` /
`VideoAddToCollectionTarget` の3つの文脈別Actionで、既定キーは空。Gridはチェック済み優先、
静止画FSは現在の実画像、動画・音声は現在の実ファイルを追加先コレクションへ渡す。
音声VSTプラグイン画面でのキー入力は既存のVST入力隔離が優先し、このActionを発火しない。
ZIP/PDF内ページは実ファイルに置き換えず拒否し、入力時に対象パスと追加先を捕捉する。
同じ追加操作は `RingActionId::AddToCollection` として、グリッド・画像フルスクリーン・
動画/音声フルスクリーンのリング、ジェスチャ、マウスボタンにも割り当てられる。
お気に入りの前後移動 / 1〜20 番を開く、現在位置のルートディレクトリを開く、
「ドライブ `C:\` のルートを開く」〜「ドライブ `Z:\` のルートを開く」、
「ドライブ `C:` の最後の場所へ切り替える」〜「ドライブ `Z:` の最後の場所へ切り替える」、場所▼の固定項目
(ドライブ一覧 / 閲覧履歴 / ★1〜★5 / 本棚フォルダ / デスクトップ / ピクチャ / ダウンロード)
、ピン留めタグ 1〜20 の付与/解除も標準キーなしのサムネイル一覧 Action として割り当てられる。設定名は
`GridFavorite...` / `GridOpenFavorite...` / `GridOpenCurrentDriveRoot` / `GridOpenDrive...` /
`GridSwitchDrive...` /
`GridOpenLocation...` / `GridTogglePinnedTag...` とし、競合判定とヘルプ表示でも `Grid` 文脈として扱う。
本とコレクションにも、1〜20 番を開く、前/次へ移動する、追加先を開く、管理画面を開く
`GridOpenBook1..20` / `GridBookPrev・Next` / `GridOpenActiveBook` / `GridManageBooks`、
`GridOpenCollection1..20` / `GridCollectionPrev・Next` / `GridOpenCollectionTarget` /
`GridManageCollections` を設ける。お気に入りの管理は `GridManageFavorites`。
いずれもグリッド文脈で既定キーは空。番号と前/次はツールバーの固定項目に限定せず、
管理一覧の全件（本は名前順、コレクションは管理順）を対象にする。前/次は端で循環し、
現在位置が一覧の項目でないときは次=先頭、前=末尾から始める。
本棚一覧またはコレクション一覧が未読込・更新中なら、1 回の入力で専用の待機ダイアログを開き、
一覧確定後に同じ要求を実行する。待機中は背面操作を止め、「中止」で要求だけを取り消す。
本は対象フォルダの事前走査が済んでから移動し、コレクションは新しい一覧の読み込み画面を
採用した時点で待機を終える。番号と前/次は読み込み後の管理順で決めるが、追加先を開く操作は
押下時点の追加先を維持し、待機中に設定が変わっても別の追加先へ飛ばない。
フォルダバーの「最近開いたフォルダ履歴をクリア」と
「A/B の記憶した場所と一覧位置をクリア」も、
`GridClearRecentFolders` / `GridClearQuickFolderSlots` として標準キーなしで割り当てられる。
外部ツール起動は `ExternalToolPicker`、固定スロットの `ExternalTool1..10`、
コンテナー用ピッカーの `ExternalToolForContainer` を、いずれも標準キーなしの `Grid` Action として扱う。
固定スロットは登録順の 1〜10 番に対応し、ツールを並べ替えると同じ Action が指すツールも変わる。
`ExternalToolPicker`／固定スロットはchecked優先、なければselectedの実フォルダを含む項目を渡す。
実フォルダは選択したフォルダ自身のパスで、ZIP内directoryは対象外。`ExternalToolForContainer`の現在地1件という意味は変えない（§1.397）。
動的な Action は作らず、設定 UI では対応するスロット番号を明示する。
同時に有効になり得る Action へ同じキーを割り当てた場合や、予約扱いの
Escape / 修飾なし矢印キー、サムネイル一覧の <kbd>Shift</kbd>+矢印範囲選択へ割り当てた場合は
起動時に警告ログを出すが、設定自体は読み込み、現行 dispatch の優先順を変えない。
予約項目の表示名は `ReservedBindingKind::label()` が単独で所有し、UI の競合相手欄・警告ログの
どちらも同じ文字列を使う。判定はこの表示文字列ではなく `BindingConflict::reserved_kind` で行う。
名前自体に「解除不可」を含め、競合警告では固定側を解除できないこと、競合をなくすには利用者が
割り当てた側を変更 / 解除すること、競合したまま使う場合の優先関係は処理順で決まり画面ごとに
異なることを示す。`Enter` は予約扱いではないので固定キーの説明に含めない
(`enter_is_assignable_and_not_reserved` で固定)。
サムネイル一覧、通常の画像フルスクリーン、動画フルスクリーン、消しゴム / 隠蔽加工 / 切り取り / SNS 分割 / テキスト注釈 / 補正レイヤーモードでは、既定 <kbd>?</kbd> の `HelpShowContextShortcuts` で現在の文脈で使える
ショートカット一覧を表示する。keymap 化済み操作は現在のコマンド設定の実割り当てから
表示し、Esc / 修飾なし矢印など固定扱いの操作は固定キーとして補助表示する。
通常の画像フルスクリーンではさらに「タッチ操作」欄を表示し、中央タップのクローム切替、
左右タップのページ送り、2 本指のズーム / パンを再確認できる。動画では中央タップの HUD 切替と
左 / 右タップの中シークで戻る / 進むを表示する（既定 5 秒、設定で変更可能）。音楽にはこの欄を出さない。
キー未設定または明示無効化中の割り当て可能操作は、操作カスタマイズ画面で確認する。
`HelpShowContextShortcuts` もコマンド設定で変更でき、ヘルプ内の固定キー欄や native 動画 overlay の表示は変更後のキーを表示する。動画フルスクリーンは egui 経路と Windows native
動画 overlay の両方で対応する。既定の `?` は設定ファイル上も `?` と書けるが、内部的には `Shift+/` と同等に扱う。
消しゴム / 隠蔽加工パネルの描画・消去ボタンとツールボタンは、先頭 chord が
修飾なし単独キーの場合だけ `筆 [B]` のような compact 表示を実割り当てから作る。
修飾付き chord や未割り当ての場合、パネル上の compact キー表記は省略する。
メニューバーのフォルダを開く / 検索 / タグビュー項目など一部のメニュー表示と hover text は、
先頭 chord を `現在地フィルタ (Ctrl+F)` のように表示する。
★フィルタボタン / スマートフィルタ内の★項目 / フォルダバー右側のコンテナ★ tooltip も、
`RatingItem*` / `RatingContainer*` の実割り当てから `F1〜F6` / `Shift+F1〜F6` などの
表示を作る。これら既存の表示用 helper には `RatingBookPreferred*` の割り当てを混ぜない。
静止画フルスクリーンの上部ホバーバー tooltip と、表示モード / ズーム・フィット
popup の shortcut 表記は、`分析ツール [Shift+Z]` や `パネル表示 [I / Tab]`
のように実割り当てから作る。native 動画 overlay の top bar / bottom HUD / jump panel /
seek hover thumbnail も KeyAction 由来の shortcut 表記を実割り当てから作る。
グリッドの <kbd>Backspace</kbd> 親フォルダ移動、Enter / Home / End / PageUp / PageDown
ナビゲーション、グリッド / フルスクリーンの <kbd>F11</kbd> 系ウィンドウ切り替えも
keymap 対象にする。`Esc` / 修飾なし矢印ナビゲーション、Ctrl+ホイールなど固定扱いの入力は従来どおり。

**動画→音声モード / 音楽ビュー**: 音声ファイル (`GridItem::Audio`) の再生と、動画再生中に映像を
消して音声だけ聴くモードは、どちらも `[FsVideo]` のキーをそのまま流用する。映像 ↔ 音声の切り替えは
`VideoToggleAudioMode` (既定 <kbd>Z</kbd>)。`VideoLoop` / `VideoBookmark` / `VideoMarkerPrev` /
`VideoMarkerNext` は音楽ビューでは音楽のループ / ブックマーク / ブックマーク移動へ翻訳される。
動画のシークストリップは `VideoSeekStripCycle` (既定 <kbd>Shift</kbd>+<kbd>S</kbd>) で
「なし → 場面 (周辺) → 場面 (全体) → 波形 (周辺) → 波形 (全体) → なし」の順に切り替える。
環境設定 (`Settings.video_seek_strip_cycle`) で外した表示は飛ばす。**外した表示も右下の
メニューからは選べる**ので、設定で到達不能な表示は作らない (全解除は読み込み時に
「場面 (周辺)」だけ有効へ正規化する)。`VideoSeekStripToggle` は
表示中なら閉じ、非表示なら上ドラッグと同じく最後に選んだサムネイル / 音声波形を復元する。
`VideoSeekStripNone` / `VideoSeekStripThumbnails` / `VideoSeekStripWaveform` は表示範囲
(周辺 / 全体) を保ったまま指定内容へ直接移る。
この追加 4 action に既定 chord はなく、ユーザーが `[FsVideo]` で任意に割り当てる。
長さ情報が無い、または場面 index が疎すぎて strip material unavailable と確定した動画では、
これら 5 action は film button / 上ドラッグと同じく設定状態を変更しない no-op とする。
動画専用の `VideoTileMode` / `VideoCapture` / `VideoFrameStep` / `VideoPin` は音楽ビューでは無効
(キャプチャパレット / コマ送り UI を出さない)。`VideoAdjustSlot1..10` も映像が隠れている通常の
音声モードでは無効だが、VST GUI 表示で presenter が再表示され映像が見えている間は有効。
`[Rating]` グループは静止画・動画と同じく
音楽ビューでも使える。左右パネル表示モードの <kbd>I</kbd> / <kbd>Tab</kbd> は 3 面で同じ
`FsToggleMetadata` を使い、native 動画も effective chord を App 側へ転送する。

`KeyContext` が異なる Action には同じ物理キーを割り当ててよい。ただし `KeyContext` は
割り当て時の競合検査とヘルプ表示の契約であり、実行時の active context を自動判定しない。
静止画フルスクリーンの `FsSeekStripToggle` も同じ <kbd>Shift</kbd>+<kbd>S</kbd> を既定にし、
`still_seek_strip_visible` を切り替える。`FsImage` と `FsVideo` は同時に有効にならないため、
動画側の `VideoSeekStripCycle` とは競合しない。
したがって key edge を消費する call site は、**消費する前に現在の item / mode が自分の
context かを確認し、自分の所有でない edge を残す**責務を持つ。既定 <kbd>Z</kbd> を共有する
`FsZoomMode` (FsImage) と `VideoToggleAudioMode` (FsVideo) では、前者が Video / Audio を
先に除外してから `take_key_hold_edges` を呼ぶ。同じ FsImage context 内で現在だけ使えない状態
(連結表示、パノラマ、分析、編集、比較) は別であり、従来どおり FsImage 側が edge を消費して
no-op 理由または状態リセットを行う。

音声モードから戻る `VideoToggleAudioMode` の診断計装は、調査対象の
`action_peek` / Win32 viewport Z / 全 viewport Z / egui Z の成否だけをログ候補の
抑制条件にしてはならない。`[fs-key]` と同じ `fullscreen_shortcut_event_summary` が
`Z:down` を報告した frame も候補にし、さらに音声モード中は理由や Z 観測の有無に関係なく
最低 1 秒に 1 行を出す。停止中に通常 repaint が無くても heartbeat 時刻の repaint を予約する。
調査中の信号が全て false でも診断自体が黙らないことを不変条件とする。

動画フルスクリーンの `FsVideo` action は、native presenter がキーを所有する経路と、
focus handoff や detached 窓へ他アプリから戻った直後などに egui がキーを受ける経路の両方へ
届き得る。したがって `VideoToggleAudioMode` と `VideoAdjustSlot1..10` のように両面で成立する
操作は native / egui の双方に mapping を持たせる。slot action の順序付き一覧は
`VIDEO_ADJUST_SLOT_ACTIONS` を唯一の正本とし、片方の経路だけへ追加しない。

Windows の egui 経路では、KeySlot でショートカットを所有した押下を egui event queue からも
同時に除去する。ただし egui 0.33 は UI 実行前の `begin_pass` で <kbd>Tab</kbd> から focus
方向を確定するため、event 除去だけでは traversal を止められない。各 egui Context に登録した
`on_begin_pass` ポリシーが、TextEdit や IME の状態にかかわらず、すべての <kbd>Tab</kbd> の
focus 方向を最初の focusable widget 登録前に `FocusDirection::None` へ戻す。これは
「<kbd>Tab</kbd> はアプリ内の keyboard focus を一切移動しない」という固定ルールであり、
操作カスタマイズの対象外。event 自体はこの段階では残すため、<kbd>Tab</kbd> chord は引き続き
`FsToggleMetadata` などの `KeyAction` へ割り当て可能で、後段の Keymap が通常どおり consume
する。no-repeat の <kbd>Tab</kbd> は repeat event も発火させず除去する。
`wants_keyboard_input()` の gate はこの消費より先に維持し、TextEdit / IME やフォーカス中 UI が
キーボードを所有している間は KeySlot と egui event の双方を残す。
Windows の `allow_ime` が true のとき、IME が処理した `NamedKey::Process` の press / repeat は egui-winit が Key / Text /
clipboard event にしない。native presenter は `VK_PROCESSKEY` を egui key にしない。
本体 Win32 queue と native 生キー公開も 0xE5 の press / repeat を捨てるため、JIS 記号の
scan-code shortcut に IME 処理済みキーが一致することはない。
egui-winit は物理キーの押下を Hidden（press 未配送）/ Forwarded（配送済み）として所有し、Process repeat でも Forwarded を保持する。
本家 0.34.0 #7967 の HashSet との差分はこの型付き状態、allow_ime にかかわらない release の状態除去・Hidden のみ遮断、focus 喪失時の全消去。
Win32 queue / native 生キー経路の release と Ime event は残す。composition 中という理由で通常の Esc press は除去しない。
取消の `Disabled` / 空 `Commit` に非空 `Commit` が無い場合だけ、input plugin が
空の `Preedit("")` を補い、未確定文字を取り消す。アプリ操作の抑止は今 pass の Ime 列を
反映した後の composing だけで決まり、Commit 後の通常 Enter / Escape / shortcut は即座に通る。
composition state は viewport ごとに独立し、main / fullscreen / detached を共有する App Context と
native presenter Context の両方へ同じ plugin を登録する。viewport 消滅・focus 喪失は当該状態を終了する。
IME 中は Windows の Alt+文字を含むキー処理で TextEdit の focus が一時的に外れる場合があるため、
`ime_focus` が直前 pass の helper-managed field を 1 pass の `FocusRecovery` owner として保持する。
この owner は TextEdit の再描画前から keymap を抑止する。focus 復帰は別判定で、`lost_focus()`、
pointer 無しの key press、他 widget の focus 無し、かつ composing または Enter / Escape press 無しの場合だけ行う。
helper の retry は同条件（`lost_focus()` 以外）で次 pass 限り有効で、初回要求用の呼び出し側 latch には書かない。
pointer 入力や他 widget への focus 移動を優先する。アプリ全体の Tab traversal 無効化、IME field の
focus 保持、keymap の操作発火抑止は別責務であり、同じ event をそれぞれの ownership 境界で扱う。
Hover で出す side panel 内の helper-managed field が実際に focus を持つ間は、その field を次 pass
にも登録できるよう panel lifetime を keyboard focus でも維持する。これは focus を取り戻す処理ではなく、
pointer 等で focus が正当に移れば次 pass の通常 hover 判定へ戻る。1-pass
`FocusRecovery` の条件と上限は変更しない。

診断は通常 logger の `[text-input-key]` として恒久的に残す。helper field が focused または直前 pass で
focused だった各 key press について、前後の widget id / egui focus、id 変化、resolved keyboard owner / phase、
同 pass で左 side panel を閉じた call site を記録する。通常 record は process あたり 1 MiB で
抑止し、id / focus / owner / close の異常 record は上限後も記録する。文字 key と physical key は修飾キーの有無に
かかわらず `Char` にマスクする。パスワード等の機密 helper field は focus contract 自体が
diagnostic policy を所有し、`key` / `physical_key` / `modifiers` をすべて `Redacted` として、
field id / focus / owner / phase / close site と押下件数だけを残す。helper focus contract がない
pass は key event を走査しない。

開発者向けメモ: 新しいキーボード操作を追加・変更するときは、ユーザーから明示されて
いなくても keymap 対応要否を確認する。通常ショートカットは `KeyAction` に追加し、
`docs/key-customization-impl-plan.md` の「新しいキー操作を追加するとき」に従って
`docs/keymap.ini.default` まで更新する。固定扱いにする入力は、この文書の該当節に理由を残す。

## 同一フレームの物理押下数

Windows の `KeySlot` 経路は Win32 key-down edge をフレーム単位で取り込む。累積可能と
明示した離散操作では、同一フレームに同じ chord の non-repeat edge が複数届いても
**1 回の物理押下を 1 回の Action 発火として数える**。選択は
`KeyAction::press_multiplicity()` の `EachPhysicalPress` に集約し、呼び出し側は
`Keymap::consume_action_press_count()` が返す回数を既存の typed request / accumulator へ渡す。

`EachPhysicalPress` にできるのは、同じ方向へ 1 step ずつ進むように反復の意味が明確で、
順番どおりの複数実行が安全であり、下流に明示的な累積上限または準備中の受付拒否がある操作だけとする。累積する対象は
`FsCtrlNavPrev` / `FsCtrlNavNext` / `FsSiblingPrev` / `FsSiblingNext`。これらは既存の
folder navigation accumulator (`MAX_PENDING_NAV = 5`) を使う。
`FsListOrderPrev` / `FsListOrderNext` も物理押下を数えるが、準備中は既存の sequence / 表示待ち lock で
拒否し、累積しない。egui のキーリピートは Ctrl+↑↓ と同じく、物理押下が無いフレームでは最大 1 回にまとめる。
トグル、削除などの破壊的操作、
ダイアログ表示、設定変更、単発の保存処理は `SinglePerFrame` のままにし、同一フレームの
複数 edge を回数分発火させない。ページ送り系は、固定矢印入力、見開き単位、連結読みスクロール、
割合ジャンプで 1 step の意味が異なるため、共通の回数適用規則を定めるまでは対象外とする。

**ページ送りが `SinglePerFrame` であることの帰結** (2026-08-16 に外部の計測ツールでの挙動を
問われて確定): 高速な連打では **N 回押しても N ページ進むとは限らない**。同一フレームに届いた
複数 edge は全部消費されるが移動は 1 回で、未処理ぶんを次フレームへ持ち越さない (上記の
edge 寿命 1 フレーム) ため、超過分の押下は失われる。**失われるのは押下であってページではない**
—— 到達点が手前になるだけで、通り過ぎたページ番号が飛ぶことはない。実測は
[docs/display-pipeline.md](display-pipeline.md) §2.5.1.2 (60Hz で 554 押下 → 538 移動、
idx 遷移はすべて +1)。これは「ページ送りをキューに溜めない」という設計方針そのもので、
仕様として維持する (利用者確認 2026-08-16)。

キーリピート (`edge.repeat`) の意味は従来どおり。repeat 許可 Action でも、長い 1 フレームに
溜まった repeat edge は最大 1 回へまとめ、同じフレームに non-repeat edge があれば物理押下数を
優先する。no-repeat Action は repeat edge を消費するが発火させない。これにより、キーを離した後に
溜まった repeat 分だけナビゲーションが続く余韻を増やさない。

fullscreen のページ送り Action と固定矢印は page-turn 専用の typed consume result を使う。
result は first keydown / auto-repeat、matched chord、送信元 viewport、処理時点で chord がまだ
physical held か、または同一 frame の後続 key-up ですでに release 済みかを保持する。first keydown は
keydown + key-up が同一 frame に届く短い tap でも必ず 1 回発火する。発火させず consume-only にするのは
release 済み auto-repeat だけである。`fs_page_turn_input_held` は複数 level の OR で、blocker や
viewport routing でも false になるため、physical key-up の証拠としてこの判定には使わない。

edge の寿命は 1 フレームのままにする。Action が消費しなかった edge は次の `begin_frame()` で
破棄し、後のフレームへ持ち越さない。無制限の backlog を作って「押していないのに後から動く」
挙動を避けるため、`begin_frame()` の `frame.clear()` は維持する。

Windows の edge queue は HWND / viewport 単位で配送する。メイン HWND は `ViewportId::ROOT`、
fullscreen / detached の HWND はその `fullscreen_viewport_id()` と対応付け、subclass proc が edge を
投入した時点で送信元 HWND と viewport を `KeyEdge` へ焼き付ける。consume / pressed / frame-state /
Enter-held の各 API は対象 `ViewportId` を必須引数にし、別 viewport の edge は成立判定も消費も
しない。HWND の `WM_NCDESTROY` では対応とその HWND 由来の未処理 edge を同じ owner が除去するため、
再生成後の viewport へ stale edge を配送しない。未登録 HWND は全 viewport へ公開せず `ROOT` として
配送して診断ログへ記録する。メイン HWND は subclass が edge を publish できる前に `ROOT` 対応を
登録済みにすることを不変条件とする。

診断だけは `diagnostic_pressed_key_down_any_viewport` で current frame の全 edge を横断して
送信元 viewport を観測できる。この API は read-only であり、edge を consume せず、Action の
成立・dispatch・fallback 判定には使用しない。production の入力所有権は引き続き viewport 必須の
API だけが担う。非消費であることは queue-level test で固定する。

### 配送済み edge と現在レベルの ownership 境界

同一 viewport に stamped された key-down edge は、OS が配送した時点の input ownership を表す
離散 fact である。egui pass が処理されるまでに foreground が別 window へ移り、その pass の最終
`viewport().focused` が false になっていても、この edge は対象 viewport で消費・dispatch してよい。
一方、unfocused pass に generic egui key event だけが存在しても送信元 viewport の証拠にはならず、
stamped edge の代用にはしない。modal、TextInput、IME、および別 viewport owner の block はこの契約
より優先し、edge を背面の application shortcut へ漏らさない。

`GetAsyncKeyState`、egui の frame-final key/modifier level、held 判定などの現在レベル観測は、対象
viewport がその pass でも focused であることを示す `FocusedKeyStatePermit` がある場合だけ許可する。
page-turn の initial press は配送済み edge だけで成立するが、auto-repeat の navigation 昇格や
`still_held` はこの permit がなければ成立しない。focus 喪失時の KeyHold transient 取消は離散 edge の
dispatch 可否とは別の状態遷移であり、取消後の unfocused pass に届いた hold edge から state を再生成
しない。WM_APPCOMMAND、Browser Back/Forward pending、pointer Extra1/Extra2 など keyboard edge 以外の
入力はこの ownership 証拠を共有しないため、従来どおり処理時点の focus を要求する。

## 固定入力 / KeyAction 対象外の整理

操作カスタマイズの一覧に出すものは、原則として `KeyAction` 化された離散ショートカットだけにする。
次の入力は OS / フォーカス中 UI / モード内ドラッグ状態に強く依存するため、現時点では固定入力として扱う。
固定入力へ同じキーを割り当てた場合は競合警告やヘルプで補助表示することはあるが、固定入力そのものは
コマンド設定で差し替えない。

| 分類 | 固定扱いの入力 | 理由 / 備考 |
|---|---|---|
| OS 予約 | <kbd>Alt</kbd>+<kbd>F4</kbd>、<kbd>Alt</kbd>+<kbd>Tab</kbd>、<kbd>Win</kbd> キー系など | Windows 側が先に処理する。mIV の keymap では上書きしない |
| keyboard focus 移動 | <kbd>Tab</kbd> traversal | 非テキスト widget へ focus が移って `wants_keyboard_input()` が true のまま残り、全 shortcut が停止した履歴と、TextEdit から 1 hop だけ移動する不統一を避けるため、アプリ全体で常に無効。Tab chord 自体は `KeyAction` へ割り当て可能 |
| フォーカスローカル UI | テキスト入力、IME 変換、コンボボックス、リスト、フォルダツリー、製本並べ替えダイアログ内の矢印 / Enter / Esc / PageUp / PageDown / Home / End など | その UI 部品の中だけで意味を持ち、グローバルショートカットとして外へ漏らさない |
| 本の構成editor (G4、next-book) | 実ページのクリック、左のページ順ドラッグ・非表示、右のページ指定、各編集button、IME-safe な Enter = 保存 / Escape = キャンセル | 本HUD・本一覧右クリック・「本」menuから開く局所UI。新しい KeyAction は設けない。Enter はフォーカスwidgetがなく確認中でないときだけ保存し、Escape は確認を先に閉じる。保存 / 再読込中は取消を受け付けず、IME変換中は Enter / Escape の保存・取消を行わない。modal owner が背面の閲覧 / メタ情報 / ナビゲータ入力を止める。解除確認は明示buttonで行う。組の変更確認はなく、結果を直接表示し元に戻す/やり直すで修正する |
| 画像folder→ZIP変換 (G) | 「ZIP に変換」「取消」「閉じる」、IME-safe な Escape = 取消 / 閉じる | folder右クリックと構成対象外案内から開く局所UI。新しい KeyAction / global shortcut は設けない。変換は明示buttonで確定し、取消可能なphaseだけEscapeを受け付ける。保存確定以降は取消しない。process modal ownerがmain / fullscreen / detachedの背面操作と新しいopenを止める |
| 環境設定の持ち運び | 設定の復元の「環境設定を書き出し…」「環境設定を取り込み…」、説明 Modal の「書き出す」「ファイルを選ぶ」、取り込み後の環境設定の Enter / Escape | ダイアログ内だけのファイル操作で、新しい KeyAction は設けない。Enter / Escape は IME-safe な dialog helper を通す。環境設定表示中は新しい転送入口二つだけを無効化し、既存の復元操作は変更しない。取り込み完了時にも環境設定が開いていれば draft を変更せず通知する。転送 Modal の表示中は背後の環境設定へ Enter / Escape を渡さず、操作カスタマイズの「押して入力」も捕捉を停止する。待機状態と入力欄は保持し、Modal を閉じれば捕捉を再開する。通常の設定復元の表示だけでは捕捉を停止しない。転送 job 中は説明 Modal と設定復元の実行 / キャンセル / × / Enter / Escape を無効にし、メインの背面設定メニュー・ツールバーは既存の設定復元/common modal と説明 Modal で止める。別窓・fullscreen・native 動画への転送専用入力遮断は加えない。検証成功後の環境設定は既存 OK / キャンセルを使う。操作カスタマイズ自体は転送対象外で既存共有を使う |
| モーダル削除確認 | <kbd>Y</kbd> = 削除、<kbd>N</kbd> / <kbd>Esc</kbd> = キャンセル、<kbd>←</kbd> / <kbd>↑</kbd> = 削除を選択、<kbd>→</kbd> / <kbd>↓</kbd> = キャンセルを選択、<kbd>Enter</kbd> = 選択中を実行 | 破壊的操作の確認ダイアログだけで意味を持つフォーカスローカル入力のため keymap 対象外。ごみ箱へ移す確認は削除、完全削除の可能性がある確認はキャンセルを初期選択にする。表示中に各キーを消費して背面の KeyAction へ漏らさず、Enter は IME-safe helper で判定する。IME 変換中は Y / N / Esc / Enter の確認操作を行わない |
| 最低限の脱出 / 閲覧ナビ | <kbd>Esc</kbd>、修飾なし矢印キー | モード脱出とページ / 一覧移動の最後の手段として残す。静止画 FS では raw-key permit を要求し、TextInput の全 phase では editor に残す一方、非テキスト `FocusedUi` はスライダー等にページ矢印を奪われないよう通す。Enter / Backspace / Home / End / PageUp / PageDown などは文脈ごとの `KeyAction` 化対象 |
| 画像連結読みのドラッグ | 左ドラッグ = 連結方向スクロール、<kbd>Ctrl</kbd>+左ドラッグ = 軸固定を解除して直交方向にもパン | 連結レイアウト中だけの連続ポインター操作。離散コマンドではなく、ドラッグ中の各フレームの修飾状態で拘束を切り替えるため keymap 対象外。ダブルクリックの表示変形リセットも固定入力として扱う |
| 右ドラッグ中の取消 | リングショートカット / マウスジェスチャ中に左ボタンを押す | 進行中の右ボタン drag state に従属する取消入力であり、独立した離散コマンドではない。active grid / viewer では左ボタンの press と release を両方ともその取消に消費し、選択・項目を開く・動画の再生切替へ流さない。passive detached window では Windows の通常動作として release による窓の activate だけを残すが、viewer action には再利用しない。このため固定入力として `KeyAction` 対象外とする |
| UI 表示倍率 | egui 既定の <kbd>Ctrl</kbd>+<kbd>+</kbd> / <kbd>-</kbd> / <kbd>0</kbd> | `Settings.ui_scale_factor` と main Context の `zoom_factor` を単一の正本にするため、main / native presenter の両 egui Context で `zoom_with_keyboard=false`。表示倍率は設定メニュー「スケーリング」だけから変更し、KeyAction 対象外とする |
| サムネイル一覧の範囲選択 | <kbd>Shift</kbd>+矢印キー | グリッド選択カーソルの移動とチェック追加が一体になった固定操作。Grid 文脈で同じキーを割り当てた場合は予約キー警告を出す |
| Shell / クリップボード / D&D | Shell 貼り付け、Shell 右クリックメニュー、外部アプリへのドラッグ送出 | Windows Shell やクリップボードイベントの経路を優先するため、キーボードコマンドとは分ける。グリッドのファイルコピー / 切り取りだけは `GridCopyFiles` / `GridCutFiles` として keymap 化し、Shell 実行経路を共有する |
| 編集ツール内のドラッグ状態 | 消しゴム / 隠蔽加工 / 補正レイヤーの選択後の矢印 / <kbd>Ctrl</kbd>+矢印、<kbd>[</kbd> / <kbd>]</kbd>、<kbd>Ctrl</kbd>+<kbd>[</kbd> / <kbd>]</kbd>、ハンドル操作中の <kbd>Shift</kbd> / <kbd>Alt</kbd>、テキスト注釈の四隅ハンドルドラッグ中の <kbd>Ctrl</kbd> / <kbd>Shift</kbd>、切り取り / テキスト注釈のドラッグやホイールなど | 選択中オブジェクト、ドラッグ中の形状、パネルフォーカスに依存するモード内操作。テキスト注釈の <kbd>Ctrl</kbd>（中心対称）と <kbd>Shift</kbd>（縦横比固定）は離散ショートカットではなく、マウスドラッグ中だけ幾何制約を切り替える修飾なので keymap 対象外。切り取りで既存枠より新規作成を優先する修飾だけは `CropForceCreateHold` として keymap 化する。フルスクリーンキャンバスでは egui の修飾状態が stale になり得るため、ドラッグ中に使う固定修飾は各フレームで OS から直接読む。操作カスタマイズ画面では消しゴム / 隠蔽 / 切り取り / テキスト / 補正レイヤーの通常コマンドを「編集モード」としてまとめる |
| マスク筆の半径 | 消しゴム / 隠蔽加工 / 補正レイヤーで、筆系ツール選択中のキャンバス上 <kbd>Shift</kbd>+ホイール | 1 ノッチごとに半径を約 1.1 倍 / 1.1 分の 1 とし、小さい側でも最低 1px 動かす。筆系ツールかつキャンバス上のときだけ消費し、筆以外やパネル上では従来のホイール経路へ残す。バックログ §4.1 の一般的な Shift / Alt+ホイールのペアバインド再設計はグリッド / 画像 / 動画を横断する別課題であり、今回は動画へ到達しない編集モード限定の固定入力なので `KeyAction` や `Settings.ring_shortcuts` の対象にしない |
| 360 度パノラマ / 通常動画の拡大・縮小表示中 | 360 は左ドラッグの yaw/pitch、ホイールの FOV、上バーの解除・投影方式・視点リセット。通常動画は左ドラッグの表示位置移動、ホイールの拡大縮小、上バーの解除・位置と倍率のリセット | 表示中だけの連続操作 / ボタンなので keymap 対象外。通常動画のホイールはシークストリップ、端パネル、モーダルの領域から奪わず、タッチのピンチは使わない。<kbd>V</kbd> の表示モード切替 (`FsPanorama`) は `FsCommon` の 1 Action で、360 候補ならパノラマ、それ以外の通常動画なら拡大・縮小表示を切り替える。<kbd>Shift</kbd>+<kbd>V</kbd> の投影方式順送り (`FsPanoramaProjection`) は 360 専用。どちらも画像 / 動画のコマンド設定から変更できる |
| 動画の修飾なし左右 | <kbd>←</kbd> / <kbd>→</kbd> の中シーク（既定 5 秒）、およびタイル中の左右カーソル移動 | 修飾なし矢印は固定ナビゲーションとして残す。<kbd>Alt</kbd> 付き左右は固定シーク扱いにせず、割り当てた `KeyAction` を優先する。<kbd>Shift</kbd> / <kbd>Ctrl</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd> 付きの動画シーク / フレーム送りは `KeyAction` 化済み。段階別の秒数は Settings と共通 |
| 動画のタッチシーク | 物理的な左 / 右の単発タップ = 中シークで戻る / 進む（既定 5 秒） | 静止画と同じ位置分類と presenter / HUD の source ownership に依存する固定タッチ操作なので `TouchAction` を `KeyAction` に合流させず、実行時だけキーボードと同じ相対シーク helper を共有する。読み方向では反転せず、シーク時に HUD 表示状態を変えない |

補足: 画像分析モード起動の既定 <kbd>Shift</kbd>+<kbd>Z</kbd> は `FsImageAnalysis`、ルーペの
<kbd>Shift</kbd> 押しっぱなしは `FsLoupeHold`、元画像表示の右 <kbd>Ctrl</kbd> 押しっぱなしは
`FsOriginalPreviewHold`、360 度パノラマ / 通常動画拡大の切替に使う <kbd>V</kbd> は
`FsPanorama`、360 の投影方式の
順送り <kbd>Shift</kbd>+<kbd>V</kbd> は `FsPanoramaProjection` として `KeyAction` 化済み。
これらは固定入力ではなく、操作カスタマイズで変更または解除できる。

## グリッドビュー (フルスクリーン外) 共通

Everything ファイル名検索は `GlobalEverythingSearch` / `Grid` / `Press`、既定
`Ctrl+R`。名前の Global は検索範囲を表し、fullscreen の Global scope へは登録しない。
main の一覧からだけ開き、text focus / IME（focus recovery を含む）/ modal / Remote lock
中には発火しない。画像・egui動画/音声・native動画/音声・detached viewer の検索起動には
使わず、native 転送 snapshot に含めない。画像 fullscreen の R 検出 probe は action の
dispatch ではない。`Ctrl+G/S/E` の既存割当は維持する。メニュー
`MenuCommandId::GlobalEverythingSearch`（ファイル → Everything ファイル名検索…）も
同じ受付へ接続し、caption は実効 keymap の先頭キーを表示する。操作カスタマイズでは
サムネイル一覧の操作として変更・解除できる。Everything の入力は明示送信とし、IME確定の
Enter では送信せず、確定後の Enter または「検索」で送信する。

§1.386: 可否条件は維持し、両メニューの無効時は理由を hover 表示する。拒否された実効Actionの
非repeat押下がmain ROOTのMain surfaceのapplication shortcut ownerに届いた場合だけ、MainWindow面に一時通知する。
Modal／文字入力／IME／focus recovery／他viewportのキーは通知も消費もしない。Remoteは現状Modal所有なので
hoverで理由を確認する。ROOT上でもFullscreen surfaceが所有するキーは既存viewer操作へ残す。
fullscreen/native側からROOTへの新しい転送は加えない。
通知はWin32 pendingを当該native frameへ取り込んだ直後の1か所で処理する。後段の実行dispatchでは
再通知せず、同じ押下のegui/native両キューを既存keymap helperで同時に消費する。

`GridColumnCount11..20` のリング・マウス割り当てを旧版から守るには、公開時にアプリ版番号を v4.1.0 より上げることが前提。`settings.db` は保存時の版番号が実行版より新しければ、旧版の起動時に読み込み前の `Incompatible` 判定で保存を止める。同じ v4.1.0 の開発ビルド間ではこの判定が働かず、旧版の `sanitize()` が未知のリング・マウス操作を `None` に置き換えて保存し得る。版番号の更新はリリース作業で行う。

| キー | 動作 |
|---|---|
| <kbd>Ctrl</kbd>+<kbd>R</kbd> | main の一覧から Everything ファイル名検索バーを開く。Action: `GlobalEverythingSearch`。閉じる操作は×/Esc。取得・準備中は取消可能なmainのモーダルを使い、完成した一覧だけを採用する |
| <kbd>?</kbd> (既定) | 現在のサムネイル一覧コンテキストで使えるショートカット一覧を表示する。Action: `HelpShowContextShortcuts`。keymap 化済み操作は現在読み込まれている割り当て済みのものを表示し、固定扱いのナビゲーションキーは別枠で表示する |
| <kbd>Backspace</kbd> | 親フォルダへ。Action: `GridParentFolder`。ドライブルート (`C:\` など) ではドライブ一覧へ戻り、元ドライブを選択状態にする。検索 (Ctrl+S / Ctrl+G) 中は検索仮想階層を 1 段ドリルアップ。Everything (Ctrl+R) 中は開いたフォルダ・本の内部を 1 段戻り、起点からは結果一覧へ戻る。検索の最上位 (集約ビュー / 結果一覧) では no-op。タグビュー (Ctrl+T) 中は、検索結果から開いたフォルダ / ZIP / PDF / 変換アーカイブを 1 段戻り、検索結果一覧では no-op (検索を閉じるには <kbd>Esc</kbd> / 検索バーの <kbd>×</kbd> / <kbd>Ctrl</kbd>+<kbd>G</kbd>・<kbd>Ctrl</kbd>+<kbd>S</kbd>・<kbd>Ctrl</kbd>+<kbd>T</kbd> 再押下)。Ctrl+F フィルタ中は、フィルタを実行したフォルダだけ親移動を no-op にする。検索結果から子フォルダへ入った後は通常どおり親へ戻れる |
| <kbd>Enter</kbd> | 選択アイテムを開く。Action: `GridOpenSelected`。別ウィンドウセッションが同じ項目を既に表示中の場合は再オープンせず、必要に応じて別ウィンドウを前面化する |
| 既定キーなし (`GridRename`) | 選択中の実ファイル / 実フォルダ 1 項目の名前を変更する。複数チェック中は実行せず通知する。Grid 文脈だけで、画像 / 動画フルスクリーンには入れない |
| 既定キーなし (`GridReload`) | 現在の一覧を、その一覧種別の再読み込み / 再スキャン / 再クエリ経路で更新する。スマートフォルダ / サブ展開の走査中は二重起動せず、★固定中は何もしない。表示中のフォルダツリーペインも同時に再読み込みする |
| 既定キーなし (`GridOpenSelectedAsPage` / `GridOpenSelectedAsList`) | 選択中の ZIP/PDF/対応アーカイブを、全体設定に関係なく「ページを開く」または「一覧を開く」で明示的に開く。右クリックメニュー、リングショートカット、マウスジェスチャ候補にも同じ操作を出す。メイン一覧をページ一覧へ切り替えるフル機能ウィンドウ用コマンドのため、複数ウィンドウモードでは共通実行入口でトーストを表示し、履歴・変換・一覧・viewer の状態を一切変更しない。現在コンテキストのショートカット一覧にも使用可能なときだけ表示する |
| 既定キーなし (`GridOpenPreferences` / `GridOpenOperationCustomize`) | 「環境設定」または「操作カスタマイズ」を開く。Grid 文脈だけで、キーボード、リング、マウスジェスチャ、マウスボタン、ゲームパッド X+方向へ割り当てられる。メインウィンドウ側のダイアログが見えない画像 / 動画フルスクリーンでは候補に出さず、発火もしない |
| 既定キーなし (`GridClearRecentFolders` / `GridClearQuickFolderSlots`) | フォルダバー設定メニューと同じく、最近開いたフォルダ履歴、または A/B の記憶した場所と起動中に記憶した全フォルダの一覧位置を確認なしでクリアし、設定へ保存して完了をトースト表示する。後者は現在の一覧も先頭へ戻す。Grid 文脈だけで、キーボード、リング、マウスジェスチャ、マウスボタン、ゲームパッド X+方向へ割り当てられる |
| 右ドラッグ（設定時） | 操作カスタマイズで選んだリングショートカットまたはマウスジェスチャを実行する。「ファイル上で右ドラッグを始めたとき、そのファイルを選択」が ON の場合は、押下時点で開始セルを選択してから操作を開始する。チェック済みの複数選択は解除せず、空き位置から開始した場合は選択を変えない。短い右クリックは従来どおり release 時に対象セルを選び、メニューを開く |
| <kbd>Shift</kbd>+<kbd>Enter</kbd> | 選択中の動画を外部プレイヤーで開く。Action: `GridOpenExternalPlayer` |
| <kbd>Alt</kbd>+<kbd>↑</kbd> | 親フォルダへ。Action: `GridParentFolder` の別既定割り当て (<kbd>Backspace</kbd> と同じ。Explorer 慣習に合わせた代替ショートカット。ドライブルートではドライブ一覧へ戻る。Ctrl+F フィルタ元フォルダでは no-op) |
| <kbd>Alt</kbd>+<kbd>←</kbd> / <kbd>→</kbd> | フォルダ履歴を戻る / 進む。Action: `GridHistoryBack` / `GridHistoryForward` (フォルダバーの ←/→ と同じ。検索中は無効。ドライブ一覧でも履歴 target があれば有効) |
| 既定キーなし (`GridOpenCurrentDriveRoot`) | 現在位置のルートディレクトリへ移動する。通常フォルダ、ZIP/PDF/変換アーカイブ内では `effective_folder()` のドライブ root / UNC share root を対象にする。検索中・★固定中は無効 |
| 既定キーなし (`GridOpenDriveC..Z`) | 対応する `C:\`〜`Z:\` のドライブルートを直接開く |
| 既定キーなし (`GridSwitchDriveC..Z`) | アクティブな A/B クイックフォルダスロットごとに覚えている、そのドライブの最後の場所へ切り替える。未訪問または最後の場所が存在しない場合は対象ドライブ root へフォールバックする |
| <kbd>Ctrl</kbd>+<kbd>↑</kbd> | ツリー順で前のフォルダへ (DFS pre-order、画像なしフォルダは skip_limit までスキップ)。Action: `GridTreeFolderPrev`。検索中は前のヒットフォルダへ移動 (`global_search_ctrl_nav` / `favsearch_ctrl_nav`)。★固定 中は snapshot 内の前 entry へ。スマートフォルダでは Folder 内の DFS を維持し、端または root 直下の PDF/ZIP/変換書庫からは一覧表示順の前の Folder/本 entry へ（単体画像・動画は除外） |
| <kbd>Ctrl</kbd>+<kbd>↓</kbd> | ツリー順で次のフォルダへ (DFS pre-order)。Action: `GridTreeFolderNext`。検索中は次のヒットフォルダへ移動。★固定 中は snapshot 内の次 entry へ。スマートフォルダでは Folder 内の DFS を維持し、端または root 直下の PDF/ZIP/変換書庫からは一覧表示順の次の Folder/本 entry へ（単体画像・動画は除外） |
| <kbd>Ctrl</kbd>+<kbd>PageUp</kbd> / <kbd>PageDown</kbd> | 前 / 次の兄弟フォルダへ。Action: `GridSiblingFolderPrev` / `GridSiblingFolderNext`。同じ親の直下だけを対象にし、空フォルダも skip せず、子や祖先の兄弟には入らない。検索中は無効。★固定 中は snapshot 内の前/次 image-like entry へ (Folder/Zip/Pdf entry は skip) |
| <kbd>Home</kbd> / <kbd>End</kbd> | サムネイル一覧の先頭 / 末尾へ移動する。Action: `GridMoveFirst` / `GridMoveLast` |
| <kbd>PageUp</kbd> / <kbd>PageDown</kbd> | サムネイル一覧を 1 ページ分前 / 次へ移動する。Action: `GridPagePrev` / `GridPageNext` |
| <kbd>F1</kbd>〜<kbd>F5</kbd> | レーティング 1〜5。ドライブ一覧中は無効 |
| <kbd>F6</kbd> | レーティング解除。ドライブ一覧中は無効 |
| <kbd>F7</kbd> / <kbd>F8</kbd> | 消しゴムマスクスロット 1 / 2 をチェック済み画像へ一括適用 (チェックがなければ選択中の 1 枚)。Action: `GridApplyErase1/2` |
| <kbd>F9</kbd> / <kbd>F10</kbd> | 隠蔽マスクスロット 1 / 2 をチェック済み画像へ一括適用 (チェックがなければ選択中の 1 枚)。Action: `GridApplyConceal1/2` |
| <kbd>Shift</kbd>+<kbd>F7</kbd> / <kbd>Shift</kbd>+<kbd>F8</kbd> | チェック済み画像 / 選択中画像から消しゴムマスクを削除。Action: `GridDeleteEraseMask` |
| <kbd>Shift</kbd>+<kbd>F9</kbd> / <kbd>Shift</kbd>+<kbd>F10</kbd> | チェック済み画像 / 選択中画像から隠蔽マスクを削除。Action: `GridDeleteConcealMask` |
| <kbd>P</kbd> | 選択中アイテムを現在のコンテナの代表サムネに固定 / 解除 (toggle、フォルダバー 📌 の左クリックと同等)。ZIP 内の ZipDir も通常フォルダと同じ cascade で子の pin に追従する。pin 不能アイテム / 検索アグリゲート / zip_nav のない変換キャッシュ状態では silent no-op。**動画フルスクリーンの P と合わせて P = Pin に統一** |
| <kbd>F</kbd> | 左側のフォルダツリーペインの表示 / 非表示を切り替える。表示時は現在フォルダへツリーカーソルを移す。非表示にする時、ツリーカーソルが別フォルダへ動いていれば <kbd>Enter</kbd> 相当でそのフォルダへ移動してグリッドへ戻る (動いていなければ単に閉じる) |
| 既定キーなし (`GridToggleStackMode`) | コマンド設定でキーを割り当てると、フォルダバーの「スタック」と同じスタック表示トグルを実行する。実フォルダまたはサブ展開ビューで有効 |
| 既定キーなし (`GridConvertEpubToPdf`) | コマンド設定でキーを割り当てると、選択中の EPUB を同じフォルダの PDF に変換する。複数選択にも対応 |
| <kbd>T</kbd> | 選択中アイテムへタグを付ける/外すダイアログを開く |
| <kbd>Ctrl</kbd>+<kbd>T</kbd> | タグビューを開く / 閉じる。`tags.db` のタグから候補を表示し、選んだタグを持つフォルダ・画像・動画・音声・ZIP/PDF/対応アーカイブを検索結果グリッドに表示する。「すべての種類」プルダウンで画像・動画・音声など結果の種類を絞れる。フルスクリーン中はテキスト注釈モードの <kbd>Ctrl</kbd>+<kbd>T</kbd> を優先する |
| 既定キーなし (`GridTogglePinnedTag1..20`) | コマンド設定でキーを割り当てると、ピン留めタグ 1〜20 番を選択中またはチェック済みアイテムへ付与/解除する。ピン留めタグの番号は「ピン留めタグの管理」画面の番号列と同じ |
| 既定キーなし (`ExternalToolPicker` / `ExternalTool1..10` / `ExternalToolForContainer`) | `ExternalToolPicker` は登録済みツールの選択メニュー、固定スロット 1〜10 は登録順の N 番目のツールの直接起動、`ExternalToolForContainer` は現在のフォルダー / 本を渡すツール選択メニューを開く。すべて Grid 文脈の操作で、並べ替えると固定スロットとツールの対応も変わる |
| <kbd>X</kbd> | 選択中の画像 / ZIP 内画像 / PDF ページを比較スロットへピン留め / 同じ画像なら解除 |
| <kbd>Ctrl</kbd>+<kbd>B</kbd> | 選択中またはチェック済みの画像 / ZIP 内画像 / PDF ページを追加先の本へ追加する。画像・ページ以外が選択に混在している場合は一部追加せず全体を拒否する |
| <kbd>Space</kbd> | 選択中アイテムをチェック ON/OFF。画像 / 動画 / ZIP・PDF 本体 / 変換前アーカイブ / ZIP 内画像 / PDF ページが対象。サブフォルダ展開を使える通常のローカルフォルダ一覧では、展開起点となる実フォルダもチェックできる。ドライブ一覧は対象外 |
| <kbd>Ctrl</kbd>+<kbd>A</kbd> | 表示中の `is_checkable` 項目を全選択。現状は Space の追加条件を使わないため、サブフォルダ展開可能な実フォルダは含めない |
| <kbd>Alt</kbd>+<kbd>1</kbd>〜<kbd>9</kbd> / <kbd>Alt</kbd>+<kbd>0</kbd> | サムネイル列数を 1〜9 / 10 列に切り替え。詳細表示中はサムネイル表示へ戻してから列数を適用 |
| 既定キーなし (`GridColumnCount11..20`) | サムネイル列数を 11〜20 列に切り替え。操作カスタマイズで個別にキーを割り当てられる。詳細表示中はサムネイル表示へ戻してから列数を適用 |
| <kbd>Alt</kbd>+<kbd>-</kbd> | サムネイル表示 / 詳細表示を切り替え |
| <kbd>Ctrl</kbd>+<kbd>C</kbd> / <kbd>X</kbd> | `GridCopyFiles` / `GridCutFiles`。チェック済み、または選択中の実ファイル / 実フォルダを Windows Shell のコピー / カット verb へ渡す。ZIP/PDF 内ページなど仮想項目が含まれる場合は実ファイルだけを部分コピーせず、トーストで通知して中止する。右クリック最上位の「コピー」「切り取り」も同じ対象解決と Shell 実行経路を使う |
| <kbd>Ctrl</kbd>+<kbd>V</kbd> | ファイル類 (仮想ファイルを含む) は従来の Shell 貼り付け、画像データは現在の実フォルダへ保存、HTML は含まれる画像の選択ダイアログを開く。それ以外は Shell 貼り付け。ZIP/PDF/検索結果グリッドなど、従来無効な一覧では無効。固定入力のまま、キー割り当ての対象外 |
| <kbd>Delete</kbd> | 通常一覧とコレクション内の物理 child では、チェック済みまたは選択中の実ファイル / 実フォルダを削除する (通常はゴミ箱。ZIP/PDF 内ページなど仮想項目は対象外)。コレクション直下では checked 優先、なければ selected の参照登録を「コレクションから外す」確認へ渡し、元ファイル / 元フォルダは変更しない |
| <kbd>F11</kbd> | メインウィンドウを最大化 ⇔ 元のサイズに復元する (`toggle_main_window_maximized` → `ViewportCommand::Maximized`)。Action: `GridToggleMaximize`。フルスクリーン中は F11 が window/全画面切替 (フルスクリーン共通表参照) なので、この最大化トグルは通常 (グリッド) 表示時のみ |
| 標準キーなし（リング / ジェスチャ / マウス / X リング専用） | `CloseMainWindow` はグリッド文脈だけで、メインウィンドウの [×] と同じ close request を送り、タスクトレイ常駐設定が有効ならトレイへ格納する。`QuitApplication` はグリッド / 画像フルスクリーン / 動画フルスクリーンで割り当て可能で、明示終了フラグを立て、常駐設定にかかわらず通常の保存・終了処理を通ってアプリを終了する。次回起動時は一覧へ戻る。画像 / 動画の `CloseFullscreen` とは別操作。通常キーへは割り当てず、操作カスタマイズのリング系候補として提供する |
| 標準キーなし（リング / ジェスチャ / マウス / X リング専用） | `GridScrollTop` / `GridScrollBottom` は、選択・チェック・最後に選択した画像を変えず、現在の一覧だけを先頭 / 末尾へスクロールする。`GridMoveFirst` / `GridMoveLast`（既定 Home / End）の選択移動とは別操作。実行時は pending intent を立て、サムネイル / 詳細表示の同フレームの行高と viewport から最大 offset が確定した後に適用する |
| <kbd>F12</kbd> | 画像・動画ビューアの別ウィンドウモード ON/OFF を切り替える。静止画 / ZIP画像 / PDFページは detached viewport、動画は同じ detached viewport の child native presenter で表示する |
| マウス左ドラッグ | グリッドのセルを掴んでエクスプローラ等へファイル D&D 送出 (コピー)。複数チェック選択時はその実パス群をまとめてドラッグ。フォルダ / ZIP・PDF 本体 / 変換前アーカイブも対象。ZIP/PDF 内画像 (仮想フォルダ) とドライブ一覧は対象外 |
| マウス戻る / 進む / ホイールクリック | `Settings.ring_shortcuts.mouse_buttons_grid` に従い、物理戻る / 進むボタンとホイールクリックを個別に割り当てる。新規環境と既定リセットは戻る / 進むがフォルダ履歴、ホイールクリックは未割り当て。既存環境は初回ダイアログで標準 / 従来どおりを選ぶまで、戻る / 進むのみ従来互換の Ctrl+↑ / Ctrl+↓ 相当 |
| エクスプローラ等からのドロップ | mIV ウィンドウへファイルをドロップすると現在表示中のフォルダへコピー (**フォルダは v1.1.0 で一旦無効化・skip**)。ZIP/PDF / 検索結果グリッドなど実フォルダ以外を表示中は拒否 |
| グリッド空白の右クリック | 現在の実フォルダの Windows Shell 背景メニューを表示する。mIV 先頭項目の「貼り付け」および Shell 側の Paste はどちらも Shell `paste` verb を使い、新しいフォルダ作成やペースト後の一覧更新は表示中フォルダ watcher が拾う |

Ctrl+V はクリップボードを開かずに形式の有無で分岐し、生データの読み取り・解析・保存は worker で行う。
画像データが HTML より優先される。監視の OFF、Office の除外、自身の印、監視の重複抑止は手動操作に適用しない。
HTML の SourceURL が無い場合や画像候補が 0 件の場合は「取り込める画像がありません」と通知して終了し、Shell へ戻さない。
選択ダイアログは他のモーダル・移動準備・Remote 操作中には開かず、保存が終わるまでモーダルを保つ。

Windows では、Shell のコピー / 切り取り / 貼り付けや native context menu の
モーダル処理中に Ctrl/Shift/Alt の KeyUp が egui へ届かないことがある。そのため
mIV は各フレームの入力処理前と Shell 復帰直後に Win32 の実キー状態を読み、
`i.modifiers` / `raw.modifiers` を同期する。個別の `Event::Key` に記録された
modifiers はイベント発生時点の情報として残し、離散ショートカットの event 判定は
書き換えない。

一覧の矢印 / PageUp・Down / Home・End による通常移動は、移動先を次の Shift 選択の起点にする。
Shift を伴う一覧カーソル移動と Shift+クリックは起点を保持する（キーの範囲チェック加算は従来どおり）。
既定の Shift 派生は矢印キーだけで、PageUp・Down / Home・End の受付は既存の完全一致 keymap に従う。
全画面のページ送り・ジャンプと Esc で戻るカーソルも通常移動として起点を更新する。
これらは既存の固定 / KeyAction 派生動作の意味を揃える修正で、新 Action・割り当ては追加しない。
サムネイル / 詳細一覧と context 所有の詳細は [範囲選択の起点](spec.md#範囲選択の起点-13702026-10-11) を参照。

## フォルダツリーペイン

フォルダツリーペインはツールバーの `ツリー` ボタン、またはグリッド側の <kbd>F</kbd> で表示する左ペイン。ここで扱うツリーは実ファイルシステムのディレクトリのみで、ZIP / PDF / 変換アーカイブなどの仮想フォルダは表示しない。ZIP / PDF 表示中は、その親実フォルダを現在位置として同期する。

ツリー内の矢印 / Enter / Esc は、テキスト入力やリストボックス操作と同じフォーカスローカルな固定入力として扱う。グローバルショートカットではないため `KeyAction` には追加しない。ツリー側にフォーカスがある間は、同じキーがグリッド選択移動やレーティング操作へ漏れないようグリッド側のキー処理を止める。

| キー | 動作 |
|---|---|
| <kbd>↑</kbd> / <kbd>↓</kbd> | 展開されているツリー行の中でカーソルを前 / 次へ移動 |
| <kbd>←</kbd> | カーソル行が展開中なら閉じる。閉じている場合は親フォルダへ移動 |
| <kbd>→</kbd> | カーソル行が閉じていれば展開。展開済みで子フォルダが読み込み済みなら先頭の子フォルダへ移動 |
| <kbd>Enter</kbd> | カーソル位置の実フォルダを右側ペインで開き、フォーカスをグリッド / 詳細一覧へ戻す |
| グリッド側の <kbd>Esc</kbd> | フォルダツリー表示中、かつ検索バー・ダイアログ・フルスクリーンなどの優先処理がない場合、ツリー側へフォーカスを移す |

★固定 (Snapshot Lock) 中はフォルダ間移動を禁止するため、フォルダツリーペイン全体を disabled とし、クリック / キー操作を受け付けない。

## 製本 並べ替えダイアログ

製本の並べ替えダイアログ内の PageUp/PageDown/Home/End は、テキスト入力やリストボックス操作と同じフォーカスローカルな固定入力として扱う。グローバルショートカットではないため `KeyAction` には追加しない。カーソル表示と Space 選択は v1.7.0 初期版では持たず、キー操作はサムネイル一覧のスクロールだけを行う。

| キー | 動作 |
|---|---|
| <kbd>PageUp</kbd> / <kbd>PageDown</kbd> | サムネイル一覧を 1 画面弱、上 / 下へスクロール |
| <kbd>Home</kbd> / <kbd>End</kbd> | サムネイル一覧の先頭 / 末尾へスクロール。予約ナビゲーションとして固定扱い |

## フルスクリーン共通 (画像 / 動画とも)

| 入力 | 動作 |
|---|---|
| <kbd>Esc</kbd> | フルスクリーン解除。**環境設定 `auto_fullscreen_zip_pdf` が ON で ZIP/PDF/変換アーカイブ内のページを表示している場合は、ページ一覧 (L2) を経由せず親フォルダの一覧 (L1) へ直帰**。追加設定 `auto_fullscreen_image_folders` も ON で、画像のみ通常フォルダを直接ページ表示している場合も同じ (`handle_fullscreen_close_request` → `pending_return_to_parent` → 入力ナビ合流点が親へナビ) |
| <kbd>Enter</kbd> | (画像) フルスクリーン解除。Action: `FsClose`。グリッドで Enter / ダブルクリックで開く動作とトグル成立。`auto_fullscreen_zip_pdf` ON のコンテナページ、または追加設定 ON の画像のみ通常フォルダでは親直帰も Esc と同じ / (動画) 再生・一時停止トグル。Action: `VideoPlayPause` |
| <kbd>Space</kbd> | (画像) 選択 (チェック) トグル — スライドショー再生中なら停止 / (動画) 再生・一時停止トグル |
| <kbd>Backspace</kbd> | フルスクリーンを 1 段閉じてグリッドビューへ戻る。Action: `FsBackToList`。ZIP/PDF/変換アーカイブ内のページ、または追加設定 ON の画像のみ通常フォルダでは、そのコンテナ / フォルダのページ一覧 (L2) を表示 (= Esc/Enter の「L1 へ直帰」と対をなす) |
| <kbd>Home</kbd> / <kbd>End</kbd> | フルスクリーン中の先頭 / 末尾の項目へ移動する。Action: `FsJumpFirst` / `FsJumpLast`。動画 native presenter 経路でも App 側へ転送する |
| <kbd>Ctrl</kbd>+<kbd>PageUp</kbd> / <kbd>PageDown</kbd> | 前 / 次の兄弟フォルダへ。同じ親の直下だけを対象にし、移動先に image-like があればフルスクリーンを維持して先頭 image-like を開く。なければ一覧へ戻る。切り離した detached 窓 / always-new 窓では、メイン bundle との境界をまたがないため無効化し案内だけ出す |
| マウスホイール | 前 / 次のファイル。縦/横連結モードでは連結方向へスクロール |
| マウス戻る / 進む / ホイールクリック | `Settings.ring_shortcuts.mouse_buttons_image` / `mouse_buttons_video` に従い、物理戻る / 進むボタンとホイールクリックを個別に割り当てる。画像フルスクリーンでは Home/End 相当の先頭 / 末尾移動、全画面ズームモードも候補に含む。ウィンドウ最小化は通常フルスクリーン / detached viewer / native video で利用でき、動画は最小化後も再生を継続する。画像 / 動画フルスクリーンのマウスボタン候補では `C:\`〜`Z:\`、お気に入り、閲覧履歴、★一覧などの場所移動系は表示しない。動画の小 / 中 / 大シーク 6 操作は v3.7.0 で一旦候補・実行を抑止したが、次版の検証 build では利用者の明示指示により再公開する。保存済みの割り当ても同じ共有 resolver から実行する。機器によって 1 クリックが複数 producer から届く根因と hold/release 契約は未解決で、時間 debounce や scan 0 の一律破棄は行わない。fresh provenance trace 後に恒久修正する。保存値、キーボード、リング、マウスジェスチャ、ゲームパッド、左右タップのシークと通常ホイールの音量調整は従来どおり。新規環境と既定リセットは戻る / 進むがフォルダ履歴、ホイールクリックは未割り当て。従来どおりを選んだ既存環境は戻る / 進むのみ Ctrl+↑ / Ctrl+↓ 相当。ホイールクリックは 500ms 以内かつドラッグしきい値以下の短クリックだけ発火し、中ボタンドラッグズームとは分離する。全画面ズームモードへ割り当てた場合、Z キー長押し時の照準表示はスキップして現在のカーソル位置でズーム状態へ入る |
| マウス左クリック | (画像) ページめくり。LTR では右半分クリックで次 / 左半分クリックで前、RTL では左半分クリックで次 / 右半分クリックで前 / (動画) 再生・一時停止トグル |
| マウス右クリック短押し | `Settings.ring_shortcuts.short_right_click_image` / `short_right_click_video` に従い、画像 / 動画別にフルスクリーンを閉じる（既定）/ 何もしない / 右クリックメニュー表示を実行する。右ドラッグのリング方向・登録ジェスチャが発火した場合は実行しない。グリッドは従来どおりメニュー、編集モードは編集操作を優先する固定入力で、この設定の対象外 |
| <kbd>F1</kbd>〜<kbd>F5</kbd> / <kbd>F6</kbd> | 表示中アイテムへレーティング 1〜5 / 解除 |
| <kbd>Shift</kbd>+<kbd>F1</kbd>〜<kbd>F5</kbd> / <kbd>Shift</kbd>+<kbd>F6</kbd> | 現在のコンテナへレーティング 1〜5 / 解除 |
| <kbd>F11</kbd> | ウィンドウ内表示 ⇔ 全画面表示 を切り替え (右上 × の左のトグルボタンと同等)。Action: `FsToggleWindowMode`。静止画は egui 経路 (`toggle_still_window_mode` = 設定 flip のみ)、動画は native presenter 経路 (`toggle_video_window_mode` = presenter rebuild)。Windows native 動画ウィンドウにフォーカスがある場合も現在の keymap 割り当てで App 側へ転送し、S キーの動画タイル表示中もタイル状態を維持したまま切り替える。別ウィンドウ表示中は、真の fullscreen API ではなく装飾なしでモニター全体を覆う仮想フルスクリーンをトグルする。消しゴムモード中は無効化 |
| <kbd>F12</kbd> | 画像・動画・音声ビューアの別ウィンドウモード ON/OFF を切り替える。Global action として keymap 対象。native 動画ウィンドウにフォーカスがある場合も App 側へ転送する。静止画のフルスクリーン編集モード中、IME 変換中、ダイアログ操作中は発火させない。設定「画像/動画を別ウィンドウで開く」が ON の間は、静止画 / ZIP画像 / PDFページでの F12 切替は無効化し、動画 / 音声表示中だけ現在のメディアをメイン / detached へ一時 migration する。F11 のウィンドウ内表示 / 全画面表示の選択は F12 とは独立して保持し、F12 OFF 時は直前の F11 状態へ戻る |
| <kbd>Ctrl</kbd>+<kbd>B</kbd> | (画像) 現在ページを追加先の本へ追加 / (動画) 現在の再生フレームを画像として追加先の本へ追加 |

「画像/動画を別ウィンドウで開く」設定で開いた always-new 窓では、消しゴム・隠蔽加工・テキスト注釈・切り取り・補正レイヤー・マスクスロット適用/削除などの編集系ショートカットを無効化する。通常 F12 の linked 別ウィンドウでは従来どおり有効。全体の色調補正、ポストフィルタ、パノラマ、分析などの表示系操作は連動なし窓でも有効。

## フルスクリーン編集モード共通 (静止画)

消しゴム / 隠蔽加工 / 切り取り / テキスト注釈 / 補正レイヤーでは、ツール固有の描画・選択・ハンドル操作中でも、以下の閲覧操作を共通で使えるようにする。パネル上で開始した操作はパネル UI を優先し、画像上で開始した Space パンや中ボタンズームは途中でパネル上を横切っても継続する。

| 入力 | 動作 |
|---|---|
| <kbd>Space</kbd>+左ドラッグ | 一時パン。進行中の描画 / 図形 / crop / 注釈ドラッグは途中でパンへ切り替えず、現在の操作を完結させる |
| <kbd>Ctrl</kbd>+<kbd>Space</kbd>+左右ドラッグ | 押した位置を中心に右で拡大・左で縮小。描画・選択等のドラッグ中は開始しない |
| マウスホイール | 画像上ではズーム。スクロール可能なツールパネル上ではパネルスクロールを優先 |
| <kbd>Ctrl</kbd>+マウスホイール | ズーム。ツールパネル上でも同じ |
| ホイール押し込み+上下ドラッグ | 中ボタンドラッグズーム。パネル上で開始した場合は無視 |
| 元画像表示の割り当て (既定: 右 <kbd>Ctrl</kbd>) を押しっぱなし | 静止中は元画像表示。補正 / AI / 消しゴム / 補正レイヤー / 隠蔽 / 注釈を一時的に外す。ナビゲーションシーケンス進行中は前の表示単位の holdover が画面を所有して現ページが出ていないため、元画像ホールドを適用しない。Action: `FsOriginalPreviewHold`。補正レイヤーの <kbd>Ctrl</kbd>+<kbd>Shift</kbd> は選択レイヤーバイパス表示を優先 |

## ゲームパッド操作

ゲームパッドは閲覧専用の固定割り当て。編集、削除、レーティング、チェック切り替え、
エクスポートは対象外。マウス戻る / 進む / ホイールクリックは環境設定の
「マウスボタン」、<kbd>X</kbd> リングは「リングショートカット」で差し替えられるが、
`keymap.ini` では扱わない。画像・動画フルスクリーンのリングには、キーボード側の
`FsToggleWindowMode` と同じウィンドウ / 全画面切替、<kbd>F12</kbd> 相当の別ウィンドウ ON/OFF、
ウィンドウ最小化も割り当てられる。

detached viewer とメイングリッドが同時に表示される場合、ゲームパッド全体と
<kbd>X</kbd> リングの現在コンテキストは OS の前面ウィンドウを優先して決める。
前面 HWND を一時的に確定できない場合だけ、最後にゲームパッドまたは割り当て済み
マウスボタンを操作した面へ追従する。マウス戻る / 進む / ホイールクリックはイベントを
受け取った面を発火面として固定し、メイングリッドからの操作が detached viewer の項目へ
誤って適用されないようにする。<kbd>X</kbd> の値ピッカーも発火面だけに表示する。中ボタンの
短クリック判定は開始面を保持し、同時表示中の別面に入力がないフレームで取り消さない。
これらは `KeyAction` ではなく固定入力レイヤーの責務である。

gilrs のゲームパッド入力は HWND / viewport へ配送されないグローバル入力なので、mIV 内の
文字入力面を排他的に選ばない。root viewport、操作対象 viewer viewport、独自 egui Context を持つ
native video / music overlay の `wants_keyboard_input` のいずれかが active なら、固定ゲームパッド
操作全体を止める。native overlay の状態は presenter から既存 output event bus の latest-value
snapshot として一方向 publish し、App から presenter Context を直接参照しない。

| 入力 | 動作 |
|---|---|
| 方向パッド / 左スティック | グリッドでは選択移動。詳細一覧では上下が 1 行移動、左右が表示行数ぶん前後にスキップ。画像ではページ送り。縦連結では上下がスクロール、左右がページ送り。横連結では左右がスクロール、上下がページ送り。左スティックは連結方向を連続スクロール。動画では左右がシーク / タイルカーソル移動、上下が前後ファイル移動 |
| <kbd>A</kbd> / <kbd>B</kbd> | 決定・開く・再生 / 戻る・閉じる。グリッドでの <kbd>B</kbd> は <kbd>Backspace</kbd> / <kbd>Alt</kbd>+<kbd>↑</kbd> と同じ親移動で、ドライブルートではドライブ一覧へ戻る |
| <kbd>LB</kbd> / <kbd>RB</kbd> | Ctrl+↑ / Ctrl+↓ と同じ前 / 次フォルダ移動 |
| <kbd>LT</kbd> / <kbd>RT</kbd> | グリッドでは連続スクロール、画像ではズームアウト / ズームイン、動画では連続シーク |
| 右スティック上下 | 画像フルスクリーンのズーム。LT/RT より速め |
| <kbd>Select</kbd> | グリッドでは場所リストを開き、ドライブ一覧 / 閲覧履歴 / 本棚フォルダ / 既知フォルダ / ドライブを上下で選んで <kbd>A</kbd> で移動する。画像フルスクリーンの通常本では見開きモードを巡回切り替え (単ページ / 見開き 左→右 / 左→右(表紙あり) / 右→左 / 右→左(表紙あり))。構成本では単ページと構成の方向・表紙に一致する見開き、抽出一覧では単ページと一時見開きを切り替える。動画フルスクリーンではブックマーク / チャプター / ピンの一覧を開き、上下で選んで <kbd>A</kbd> で移動する |
| <kbd>X</kbd>+方向パッド / 左スティック | リングショートカット。<kbd>X</kbd> を押しながら方向を選び、<kbd>X</kbd> を離すと現在コンテキスト (グリッド / 画像フルスクリーン / 動画フルスクリーン) のスロットに設定された一発アクションを実行する。左スティックは軽く触れた程度では方向確定せず無方向扱い。方向なしで離すと専用ピッカーパネルを開き、方向で行選択/値変更、<kbd>A</kbd> で一覧を開くか一覧項目を選択、<kbd>X</kbd> / <kbd>B</kbd> で確定して閉じる。<kbd>B</kbd> でリング選択もキャンセルできる。<kbd>X</kbd> リング中とピッカー表示中の方向入力は通常ナビゲーションへ流さない |
| <kbd>Y</kbd> | グリッドではフォルダツリーペインを開閉する。表示時は現在フォルダへツリーカーソルを移す。非表示にする時、ツリーカーソルが別フォルダへ動いていれば <kbd>A</kbd>/<kbd>Enter</kbd> 相当でそのフォルダへ移動してグリッドへ戻る (動いていなければ単に閉じる)。動画では S キー相当。画像では Y+左右で Ctrl+左右相当の見開き 1 ページずらし、Y+上下で Home/End 相当の先頭 / 末尾移動 |
| フォルダツリー表示中の方向パッド / 左スティック / <kbd>A</kbd> | ツリーカーソルを上下左右に移動し、<kbd>A</kbd> で選択フォルダを開く。決定後はツリーを自動で閉じ、グリッドへ戻る |
| <kbd>Y</kbd>+方向パッド左右 (動画) | J / K キーと同じ前 / 次のチャプター・ブックマーク・ピン移動 |
| <kbd>Start</kbd> | お気に入り／スマートフォルダ一覧を開く。左右でタブを切り替え、上下で選んで <kbd>A</kbd> で移動する。現在スマートフォルダを表示中ならそのタブから開き、件数が多い場合はスクロールバーで続きが分かる |

## キーによる段階ズーム（§1.470）

`FsZoomIn`（画像を拡大する）/ `FsZoomOut`（画像を縮小する）は `FsImage` / `Press`。
既定の拡大は `Shift+Semicolon`、`Semicolon`、`NumpadAdd`、縮小は `Minus`、`NumpadSubtract`。
物理 KeySlot / Win32 VK が正本で、JIS / US とも VK_OEM_PLUS（0xBB、`Semicolon`）の
Shift 押しを `+` とする。修飾なしは JIS の `;` / US の `=`。下段テンキーの +/- も使える。
VK_OEM_MINUS（0xBD、`Minus`）は修飾なし。表示専用の `display_name` は
`Shift+Semicolon` → `+`、`Ctrl+Shift+Semicolon` → `Ctrl++` とする。
settings.db / JSON の `settings_name` は既存の `Shift+;`（Ctrl 付きは `Ctrl+Shift+;`）を維持する。
chord 解析は既存の `Shift+;` / `Shift+Semicolon` に加え、表示形の `+` を受理する。
`Ctrl++` / `Alt++` / `Ctrl+Alt++` 等の末尾 `++` は、その修飾 + Shift + Semicolon として読む。
通常の `Ctrl+A` 等の区切りは維持し、編集欄の初期表示・「押して入力」の表示形も保存できる。
既定 chord 全件の表示往復に合わせ、右修飾キーの表示名（`右Ctrl` / `右Shift` / `右Alt`）も解析で受理する。
ini テンプレート / 参照ファイルの生成だけは、`;` / `#` を含む表記をコメント記号と衝突しない
KeySlot の別名へ置き換える。拡大は `Shift+Semicolon` / `Semicolon` と書き、旧 ini 取り込みでも
その別名を読む。現行キーでこの変換が必要なのは `Semicolon` だけで、`#` を出すキーはない。
修飾なしの JSON 保存表記も既存の `;` / `-` を維持する。表示形を入力しても保存・ini 出力表記は変えない。
修飾なし `Semicolon` の表示は従来の `;` を維持する。

1 回につき ×1.25 / ÷1.25。OS リピートを受け付け、同一フレームは各操作1回
（`SinglePerFrame`）へまとめる。マウス位置を使わず、画面の中央を基準にする。
書き先は既存 `fs_zoom_owner()` に従う。

| owner | 適用 |
| --- | --- |
| Image | `apply_touch_zoom_factor_about_pivot(factor, center, center)` で `fs_zoom` / `fs_pan`。`ZOOM_MIN`〜`ZOOM_MAX` と `set_fs_pan_from_input` の制約を共有。単ページ / 見開き / 縦横連結に適用 |
| Analysis | 同じ関数で `analysis_zoom` / `analysis_pan`。分析画像の表示中央を固定 |
| Panorama | `apply_panorama_touch_zoom` で FOV を変更 |
| ZoomMode | 照準中 / 確定中とも `adjust_fs_zoom_factor(±1.0)` で専用倍率だけを変更 |

Image / Analysis で変化したらそれぞれの倍率を `maybe_rerender_pdf` へ渡す。
上下限で変化しなければ何もしない。変化時だけ repaint を要求する。
Image / Analysis とも中心・pivot に同じ点（原点）を渡すと pan は倍率比で拡縮される。
ページ単位表示ではこれで表示中央が固定され、矩形の位置 / 寸法は計算に不要となる。
キー段では表示矩形・seek geometry を計算せず、root から配送された入力でも同じ結果となる。
設計の簡素化: キー段で完結させ、フレーム間 pending や専用の viewport 配送を増やさない。

連結読みの主軸位置は `fs_vertical_scroll` が所有するため、`fs_pan` だけを拡縮する現在の
キー / Ctrl+ホイール / ピンチでは表示中央がずれ得る。全ズーム入力に共通する既存課題として
今回の修正対象から外し、主軸位置・スクロール所有は変更しない。

動画 / 音声 / 音楽ビューでは consume しない。消しゴム / 隠蔽 / 切り取り / SNS 分割 /
テキスト注釈 / 補正レイヤーでは既存 `FsFitModeCycle` と同じ early return に従い実行しない。
比較表示は `FsFitModeCycle` と同様に通常キー段へ到達するので適用する。
ページ一覧中は実行可能集合に含めない。TextEdit / IME / modal / popup / 非アクティブ窓は
既存 helper の入力所有に従い、F12 / 複数窓も既存 viewer context の状態だけを変更する。
修飾キー長押しのルーペは Shift のレベルを別に読むため、`+` 中も既存の `Shift+S` 等と同様に反応する。

今回はキーボード専用。ヘルプと操作カスタマイズは全 Action の一覧から自動表示し、リング /
ジェスチャ / マウスボタン / ゲームパッド、HUD / フィット popup ボタンは追加しない。
保存済み設定で新 Action の override がなければ既定が有効になる（`FsPageOverviewToggle` と同じ）。
他操作の保存済み割当を移動・削除せず、重複は既存の競合表示と handler の消費順に従う。
移行処理は追加しない。明示的な再割当 / `none` による解除は通常の置換規則を使う。

## 画像 フルスクリーン

| キー | 動作 |
|---|---|
| <kbd>?</kbd> (既定) | 画像フルスクリーンで使えるショートカット一覧を表示する。Action: `HelpShowContextShortcuts`。keymap 化済み操作は現在読み込まれている割り当て済みのものを表示し、固定扱いのナビゲーションキーと、中央 / 左右タップ・2 本指ズーム / パンの「タッチ操作」は別枠で表示する |
| <kbd>I</kbd> / <kbd>Tab</kbd> | 左右パネルの表示モード (通常ホバー / クリック表示) を切り替える。Action: `FsToggleMetadata`。画像フルスクリーン、native 動画、egui 音楽ビューで共通。native 動画では effective chord を presenter から App へ転送する |
| <kbd>←</kbd> / <kbd>→</kbd> | 前 / 次のファイル (見開き中は前 / 次の見開き = 2 ページ送り)。左右の方向は環境設定で「ページ表示 / 読み方向に合わせる」（既定）または「シークバー方向に合わせる」を選べる。最後 / 最初の見開きでは、手動の前後移動ループがOFF、または折り返せる別の表示単位がない場合、1 回目の入力で境界案内を表示する。ループONで折り返せる場合は反対側の端へ移る |
| <kbd>↑</kbd> / <kbd>↓</kbd> | 前 / 次のファイル (= 一般慣例で左右と同義)。見開きの境界判定も左右と共通。縦連結では縦スクロール、横連結では前 / 次ファイル。スライドショー中もフォルダ内移動は再生を止めない |
| <kbd>Shift</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | 通常はプレーン <kbd>↑</kbd>/<kbd>↓</kbd> と同義 (前 / 次ファイル)。**ファイル名スタックのフラット読書中 (v2.0.0)** は「前 / 次のスタックの先頭画像へジャンプ」(= 今の投稿の残りをスキップ。`Shift+↑` はスタック途中なら現スタック先頭、先頭なら前スタック先頭)。端では no-op (次フォルダは <kbd>Ctrl</kbd>+<kbd>↓</kbd>)。スタックジャンプ部分の Action: `FsStackJumpPrev` / `FsStackJumpNext`。動画再生中は音量 (下表参照) |
| 既定キーなし (`FsPagePrev` / `FsPageNext`) | コマンド設定でキーを割り当てると、前 / 次のページへ移動する。矢印ナビゲーションは固定扱いのまま残しつつ、明示的にページ送りへ割り当てたい場合の Action |
| <kbd>Shift</kbd>+<kbd>←</kbd> / <kbd>→</kbd> | 現在の表示順で、環境設定の「ページジャンプ量」ぶん前 / 次へジャンプ。Action: `FsFixedJumpPrev` / `FsFixedJumpNext`。既定は全ページの 10%。固定ページ数にも切替可。見開き中は最低 2 ページ進む。左右の意味はページ表示の RTL で反転し、「カーソルキー左右の方向」の影響は受けない。動画は対象外 |
| <kbd>PageUp</kbd> / <kbd>PageDown</kbd> | 縦/横連結モードでは画面単位で連結方向へスクロール。通常のページ単位表示では、環境設定の「ページジャンプ量」ぶん前 / 次へジャンプする。Action: `FsFixedJumpPrevNoRtl` / `FsFixedJumpNextNoRtl`。こちらは PageUp/PageDown へ割り当てる用途のため RTL でも前 / 次の意味を反転しない。修飾なし矢印ナビゲーションは文脈依存の固定入力として残す一方、メタデータパネル・ページジャンプ・Home/End 先頭末尾移動など副作用が明確な操作は KeyAction 化して競合検出対象にする |
| <kbd>Ctrl</kbd>+<kbd>←</kbd> / <kbd>→</kbd> | 見開きの「1 ページずらし」(現在の表示ユニット先頭を軸に見開きを 1 ページぶんずらす。空白/欠落ページでの綴じずれ補正。端の no-op は保存しない)。適格な本では既定ONの全体記憶設定に従い、実際の内部アンカーのPageRefを`spread_shift.db`へ保存する。通常一覧と記憶OFFではセッション調整。`spread_db`のmodeは保存し直さない。Single / Split モードでは従来の前 / 次ページ移動。ページ表示が RTL なら左右の意味を反転し、「カーソルキー左右の方向」の影響は受けない。リング / マウス / パッド用の操作候補には、この左右版に加えて PageUp/PageDown と同じく RTL でも前 / 次を反転しない「前 / 次」版を用意する |
| <kbd>Ctrl</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | ツリー順で前 / 次のフォルダへ移動する。Action: `FsCtrlNavPrev` / `FsCtrlNavNext`。スマートフォルダでは Folder 内 DFS の端と root 直下の本から、一覧表示順の Folder/PDF/ZIP/変換書庫へ移る。独立 detached 静止画窓では main の検索・絞り込みを継承せず、窓自身の物理順で通常画像フォルダ / ZIP / CBZ / PDF へ移動する |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | 一覧の表示順で前 / 次の本を開く。Action: `FsListOrderPrev` / `FsListOrderNext` (`FsCommon` / Press / `EachPhysicalPress`)。最後に通常フォルダ一覧から本を開く操作を受け付けた時点の絞り込み・並べ替え順を使い、その一覧の中だけを移動する。Ctrl+↑↓ の DFS と異なりサブフォルダへは降りない。使える範囲・停止条件は [一覧順ナビゲーション設計](list-order-book-navigation-plan.md) 参照 |
| <kbd>Ctrl</kbd>+<kbd>PageUp</kbd> / <kbd>PageDown</kbd> | 同じ親直下の前 / 次の兄弟フォルダへ移動する。Action: `FsSiblingPrev` / `FsSiblingNext`。独立 detached 静止画窓でも窓自身の物理兄弟順を使う |
| <kbd>0</kbd> 〜 <kbd>7</kbd> | <kbd>1</kbd>〜<kbd>5</kbd>: ページ構成切替 (<kbd>1</kbd>: 単ページ / <kbd>2</kbd>: 見開き 左開き / <kbd>3</kbd>: 見開き 左開き+表紙単独 / <kbd>4</kbd>: 見開き 右開き / <kbd>5</kbd>: 見開き 右開き+表紙単独)。<kbd>6</kbd>: 連結方式をページ単位 → 縦連結 → 横連結で循環。<kbd>7</kbd>: 横方向 左→右 / 右→左を切替。<kbd>0</kbd>: ズーム/フィットをページ全体 → 横幅フィット → 縦幅フィット → 100%原寸 → 指定サイズで循環。余白カットは左パネルの表示トリムで設定する。見開き中は表紙あり/なしを保ったまま左開き / 右開きも連動して切り替える。ZIP の作品区切り表示上でも有効。ホバーバーの表示モード/フィットボタンからも切替可 |
| 既定キーなし (`FsTogglePageAfterCoverAlone` / `FsToggleLastPageAlone`) | 表紙あり見開きで、表紙の次 / 最終ページを白い相方付きで単独表示する本別設定を切り替える。全体設定に従う状態からは現在の実効値の反対を明示保存する。表紙なし見開き・単ページ・横長分割では値だけ変更し、表示には適用しない |

連結モード (縦連結 / 横連結) は画像 / ZIP 内画像 / PDF ページの通常閲覧用。360度パノラマ、分析、全画面ズーム、比較表示、消しゴム、隠蔽加工、テキスト注釈、補正レイヤーなどの編集・解析系モードはページ単位表示でのみ起動する。連結表示中にこれらのショートカットを押すと、`[機能名] ページ単位表示でのみ使用できます` と理由を画面表示する。
360 度パノラマ表示中は <kbd>E</kbd>、<kbd>Ctrl</kbd>+<kbd>G</kbd>、
<kbd>Ctrl</kbd>+<kbd>M</kbd>、<kbd>Ctrl</kbd>+<kbd>T</kbd> による画素編集モード入場を
共通の availability gate で抑止し、「360° ビュー中は画像編集モードを利用できません」と
理由を表示する。個々のキー分岐へ 360 判定を重複実装しない。
連結読みでは、連結方向と直交する矢印キー (縦連結の左右、横連結の上下) で前後へ移るとき、
途中の動画と音声を飛ばして次の静止画へ着地する。連結表示が静止画ページだけをつなぐため、
連結方向へスクロールして読むときの対象と揃える。ページ単位表示と、動画・音声を表示中の前後ファイル移動は変えない。
| <kbd>R</kbd> / <kbd>L</kbd> | 右 / 左 90° 回転 |
| <kbd>Z</kbd> (ホールド/短押し) | ZipPla 風 全画面ズームモード (v2.0.0)。**押している間**は OS カーソルを隠して画像全体を表示し、ズーム範囲の枠をマウスで移動・ホイールで枠サイズ (= 倍率) を変える。**離す**とカーソルを戻して枠の範囲を画面いっぱいに拡大し、以後マウス移動で表示範囲をパン (元画像範囲内のみ、余白なし)。**ズーム確定後のホイールは通常どおり前後ページ移動** (ZipPla 準拠)、倍率を変えたいときは <kbd>Ctrl</kbd>+ホイール。**もう一度 <kbd>Z</kbd> で元の表示へ戻る**。短押しでもその場でズーム。既定倍率は単ページ=cover (縦長で横幅目一杯) / 見開き=単ページ幅の約 1.2 倍。ズーム中は左右の補正/メタデータパネルを抑止し、パンは上下のホバー領域へ入る前に画像の上端・下端まで到達する (操作帯方式)。照準中もこの写像は変えない。PDF はズーム倍率で高解像度へ再レンダ、表示トリム中はトリム後範囲のみパン。前後ページへ移動してもズーム状態・倍率はセッション内で維持する。単ページ・見開きの通常閲覧で動作 (連結 / パノラマ / 動画 / 分析モードでは無効)。現行ルーペ (<kbd>M</kbd> / <kbd>Shift</kbd>) とは別物 |
| <kbd>Alt</kbd>+<kbd>N</kbd> | `FsNavigatorToggle`。ナビゲータの固定表示を切り替える。単ページ / 見開き / 比較表示 / 360 度パノラマで、縮小画像と現在の表示範囲を示す。左ドラッグは範囲指定ズーム（パノラマでは選択の高さを視野角にして中心を移動）、右ドラッグはパン、ダブルクリックはその位置を中央へ移動、ホイールはナビゲータ自体のサイズ変更。ヘッダーの四隅ボタンで配置を選ぶ。固定表示の状態・配置・サイズは保存する。全体可視時も表示し、画面が画像より広いときは枠を全体に保って画像を内側へ縮める。比較中は現在の Wipe / Diff / ピン表示と同じ全体合成を示す。連結読み・動画・編集 / 分析モードでは表示しない。ルーペとは用途が異なるため同時表示できる。既定 <kbd>Alt</kbd>+<kbd>N</kbd> は `FsImage` 文脈の既定 chord と衝突しない |
| ホイールクリックに全画面ズームモードを割り当て | 短クリックでズーム状態を ON/OFF する。Z キーの押下中照準表示は出さず、現在のカーソル位置でズーム状態へ入る。ズーム状態中は中ボタン押し込み + 上下ドラッグで倍率を変えられる |
| <kbd>Shift</kbd>+<kbd>Z</kbd> | 画像分析モード (旧 <kbd>Z</kbd>。Z を全画面ズームへ明け渡したため移動) |
| <kbd>S</kbd> | スライドショー 再生 / 停止。ホバーバーの ▶ / 一時停止ボタンも即時に開始 / 停止する。ページ送り間隔・連結読みスクロール設定・末尾動作は上部の「…」→「スライドショー」または環境設定で変更する。動画はスキップして継続。縦/横連結読み中は待機時間ごとに設定割合だけ短時間スクロールし、終端で通常の末尾動作へ合流する。フォルダ内移動 (矢印 / ホイール / クリック / Home / End) では止まらず、Ctrl+↑↓ のフォルダ移動・S・Space・Esc で止まる |
| <kbd>Shift</kbd>+<kbd>S</kbd> | `FsSeekStripToggle`。下部のサムネイル列を表示 / 非表示にする。フィルム状ボタンと上下ドラッグも同じ `still_seek_strip_visible` を切り替える |
| <kbd>M</kbd> | ルーペ トグル。360 度パノラマモード中とテキスト注釈モード中は表示しない。テキスト注釈モードに入ってもトグル状態は保持する |
| <kbd>G</kbd> | ピクセルグリッド表示 ON/OFF (ユーザーズームが等倍より拡大中、かつ高倍率時のみ画像ピクセル境界を表示) |
| <kbd>B</kbd> | `FsBookBookmark`。本として開いている画像フォルダ / 製本 / ZIP / PDF / 対応アーカイブの現在基準ページへブックマークを追加する。同じページは重複登録しない。本ではない単独画像では無効。編集サブモード中は筆ツール等の既存 `B` 操作を優先する |
| <kbd>Shift</kbd>+<kbd>B</kbd> | `FsBgCycle`。透過背景色サイクル (黒 → 白 → 市松)。AI アップスケール時は黒 ↔ 白の 2 段 (市松は出力に焼き込まれるため不可) + 背景変更時に `clear_adjustment_caches` を呼び背景別 `(idx,bg)` 結果を表示し直す (idx キーの派生キャッシュ取り違えによる固着防止)。透過 (alpha) の無い画像では無効化してトースト案内 |
| <kbd>E</kbd> | 消しゴムモード開始 / 確定 |
| <kbd>Ctrl</kbd>+<kbd>G</kbd> | 補正レイヤーモードを開始する。Action: `FsLocalAdjustMode`。画像フルスクリーンの操作カスタマイズ対象。モード終了は <kbd>Esc</kbd> |
| <kbd>Ctrl</kbd>+<kbd>1</kbd>〜<kbd>9</kbd> / <kbd>Ctrl</kbd>+<kbd>0</kbd> | 画像補正保存スロット 1〜10 を現在ページの個別設定として読み込む。Action: `FsAdjustSlot1..10`。通常数字とテンキー数字の両方を既定割り当てにする |
| <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>1</kbd>〜<kbd>9</kbd> / <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>0</kbd> | 画像補正保存スロット 1〜10 を**その場所の標準**へ読み込み、ページ個別設定を解除する。「このお気に入り用に標準を分ける」が ON のお気に入り配下ならそのお気に入り標準、それ以外は共通の標準を更新する。Action: `FsAdjustSlotDefault1..10`。通常数字とテンキー数字の両方を既定割り当てにする |
| <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>-</kbd> | 共通の標準の内容を、この場所のお気に入り標準へ複製する。書き込み先が共通の標準のときは no-op + トースト。Action: `FsAdjustCopyGlobalDefaultToFavorite`。上段の `-` とテンキーの `-` の両方を既定割り当てにする (JIS 配列の上段 `-` は実機確認が必要) |
| <kbd>Ctrl</kbd>+<kbd>M</kbd> | 隠蔽加工モード開始 / 終了 |
| <kbd>F7</kbd> / <kbd>F8</kbd> | 消しゴムマスクスロット 1 / 2 を現在ページに即適用 |
| <kbd>F9</kbd> / <kbd>F10</kbd> | 隠蔽マスクスロット 1 / 2 を現在ページに即適用 |
| <kbd>Shift</kbd>+<kbd>F7</kbd> / <kbd>Shift</kbd>+<kbd>F8</kbd> | 現在ページの消しゴムマスクを削除 |
| <kbd>Shift</kbd>+<kbd>F9</kbd> / <kbd>Shift</kbd>+<kbd>F10</kbd> | 現在ページの隠蔽マスクを削除 |
| <kbd>Ctrl</kbd>+<kbd>E</kbd> | 現在の表示結果を別ファイルへエクスポート |
| <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd> | 画像パイプラインのデバッグ出力 (`%APPDATA%\mimageviewer\debug-pipeline\...` に段階別 PNG と manifest を保存) |
| <kbd>P</kbd> | 現在表示中アイテムを現在のコンテナの代表サムネに固定 / 解除 |
| <kbd>X</kbd> | 現在表示中を比較スロットへピン留め / 同じ画像なら解除 |
| <kbd>C</kbd> | 比較スロットのピン画像と現在画像をトグル表示 |
| <kbd>Shift</kbd>+<kbd>C</kbd> | Wipe 比較を ON/OFF (左=ピン、右=現在) |
| <kbd>Ctrl</kbd> (Wipe 境界を左ドラッグ中) | 押している間だけ境界線を隠す。ドラッグを継続したまま見え方を一時的に変える修飾であり、単独のキー操作ではないため keymap 対象外の固定入力とする。フルスクリーン viewport の stale な modifier event を避け、ドラッグ中の各フレームで OS の押下状態を読む |
| <kbd>Alt</kbd>+<kbd>C</kbd> | 差分比較を ON/OFF (RGB チャンネルごとの差分を色付きで強調表示) |
| <kbd>U</kbd> / <kbd>Shift</kbd>+<kbd>U</kbd> / <kbd>Alt</kbd>+<kbd>U</kbd> | AI モデル 次 / 前 / リセット。標準キーなしの `FsAiModelAuto` / `FsAiModelRealEsrganX4Plus` / `FsAiModelRealEsrganAnime6B` / `FsAiModelRealCugan4x` / `FsAiModelNmkdSiax4x` / `FsAiModelRealEsrGeneralV3` にキーを割り当てると、任意の AI モデルへ直接切り替えられる |
| <kbd>T</kbd> / <kbd>Shift</kbd>+<kbd>T</kbd> / <kbd>Alt</kbd>+<kbd>T</kbd> | ポストフィルタ 次 / 前 / 標準 (リセット)。標準キーなしの `FsPostFilterUpscaleSharp` / `FsPostFilterUpscaleAnime` / `FsPostFilterUpscalePixelArt` など `FsPostFilter...` 系 Action にキーを割り当てると、任意のポストフィルタへ直接切り替えられる |
| <kbd>V</kbd> | 360 度パノラマモード トグル (360 候補画像のみ) |
| <kbd>Ctrl</kbd>+<kbd>S</kbd> | 現在画像 / アニメーション現在フレーム / ZIP 内画像 / PDF ページをキャプチャ保存フォルダへ保存 |

### 押している間だけの操作

| 既定キー | Action | 動作 |
|---|---|---|
| <kbd>Shift</kbd> | `FsLoupeHold` | 押している間だけルーペを表示する。360 度パノラマモード中とテキスト注釈モード中は表示しない |
| <kbd>Alt</kbd> | `FsNavigatorHold` | 固定表示が OFF でも、押している間だけナビゲータを表示する。ナビゲータ上でドラッグを始めた後は、修飾キーを離してもドラッグ終了まで表示を保つ |
| 右 <kbd>Ctrl</kbd> | `FsOriginalPreviewHold` | 静止中に押している間だけ、補正 / AI / 消しゴム / 補正レイヤー / 隠蔽 / 注釈を外した元画像を表示する。ナビゲーションシーケンス進行中は適用しない |

3 操作とも同じ `ModifierHold` の仕組みを使い、操作カスタマイズで修飾キーを変更または割り当て解除できる。
`Ctrl` / `Shift` / `Alt` は左右どちら側でも成立し、`RightCtrl` / `RightShift` /
`RightAlt` は Windows では右側だけで成立する。左右を区別できない環境では、右側限定の指定も
対応する左右不問の修飾キー判定へ縮退する。左側限定は用意しない。

U / N / T、<kbd>Ctrl</kbd>+数字、<kbd>Ctrl</kbd>+<kbd>Alt</kbd>+数字、
<kbd>Ctrl</kbd>+<kbd>Backspace</kbd> の補正系操作は、ページ単位表示だけでなく縦・横の連結読みでも
利用できる。対象ページはシークバーが示す現在ページを基準に、各操作の既存の対象解決規則で
決まる。消しゴム、補正レイヤー、隠蔽加工、切り取り、テキスト注釈、エクスポートなどの
加工機能は引き続きページ単位表示に限定する。
連結表示中に加工・解析・全画面ズーム・比較表示・エクスポートのショートカットを押した場合は、無言で抑止せず、機能名とページ単位表示への切替が必要な理由を画面表示する。通知は単一の no-op 表示を更新するため、キーリピートでもキューには蓄積しない。

## 消しゴムモード

結果プレビューは `ErasePreviewHold`（Erase / KeyHold、既定 Tab）と目のボタンを OR で合成し、
既存の false → true 遷移で保存なし補完を 1 回だけ投入する。両方押しても再投入せず、両方離すと終了する。
キーの保持判定はパネル描画の外、キャンバスのドラッグ解放処理より前に行い、
`focused_key_state_permit` / keymap の TextEdit・IME・modal・非アクティブ窓の入力所有に従う。
legacy keymap helper は Modal を呼出元で止める契約なので、このキャンバス側のキー受付も
cached `KeyboardOwner::Modal` では保持・解放を観測しない。
筆・囲み・多角形・線・図形作成・ハンドル編集の進行中に押したキーは保持受付を拒否し、
描画終了後も押したままなら開始しない。Shift 等の修飾変更や入力所有の喪失を解放とみなさず、
permit のあるフレームで割当キー自体の解放を確認してから押し直す。既存のボタン中のキャンバス操作は変更しない。
保持受付は Released / Held / BlockedUntilRelease の一つの状態とし、入場・退出で初期化する。
設計の簡素化: キーとボタンに別々の補完要求や worker を設けず、既存のプレビュー遷移を共用する。
目のボタンの hover、`?` ヘルプ、操作カスタマイズは実割り当てを表示する。隠蔽加工には追加しない。

| 入力 | 動作 |
|---|---|
| <kbd>?</kbd> (既定) | 消しゴムモードで使えるショートカット一覧を表示する。Action: `HelpShowContextShortcuts`。keymap 化済み操作は現在読み込まれている割り当て済みのものを表示し、固定扱いの操作は別枠で表示する |
| <kbd>E</kbd> / <kbd>Esc</kbd> | 補完を実行して終了。選択中オブジェクトがあるときの <kbd>Esc</kbd> はまず選択解除 |
| <kbd>S</kbd> / <kbd>B</kbd> / <kbd>K</kbd> / <kbd>L</kbd> / <kbd>P</kbd> / <kbd>I</kbd> / <kbd>V</kbd> / <kbd>H</kbd> / <kbd>R</kbd> / <kbd>O</kbd> | 選択 / 筆 / バケツ / 囲み / 多角形 / 直線 / 縦線 / 横線 / 矩形 / 楕円ツール |
| <kbd>D</kbd> / <kbd>F</kbd> | 描画 / 消去モード切替 |
| <kbd>Space</kbd>+左ドラッグ | 一時パン |
| <kbd>Tab</kbd>（押している間） | 消しゴム適用後の結果プレビュー（モザイクは除外）。Action: `ErasePreviewHold`。操作カスタマイズで変更・解除できる。描画ドラッグ中の押下は無効なので、終了後に押し直す |
| <kbd>Ctrl</kbd>+マウスホイール | ズーム。ツールパネル上でも同じ |
| マウスホイール | 画像上ではズーム。ツールパネル上ではパネルスクロール |
| 矢印 / <kbd>Ctrl</kbd>+矢印 | マスクまたは選択オブジェクトを 1px / 10px 移動 |
| <kbd>[</kbd> / <kbd>]</kbd>, <kbd>Ctrl</kbd>+<kbd>[</kbd> / <kbd>]</kbd> | マスクまたは選択オブジェクトを ±0.1° / ±1° 回転 |
| <kbd>Enter</kbd> | 多角形マスクの頂点列を確定。Action: `EraseConfirmPolygon` |
| <kbd>Shift</kbd>+ハンドル | 端点角度・回転角をスナップ、矩形/楕円の角リサイズを等比化 |
| <kbd>Alt</kbd>+ハンドル | 矩形/楕円を中心固定でリサイズ |
| <kbd>Ctrl</kbd>+<kbd>Z</kbd> | マスク編集 Undo |
| <kbd>Ctrl</kbd>+<kbd>Y</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Z</kbd> | マスク編集 Redo |
| <kbd>Del</kbd> | 選択中オブジェクトを削除 |

## 隠蔽加工モード

| 入力 | 動作 |
|---|---|
| <kbd>?</kbd> (既定) | 隠蔽加工モードで使えるショートカット一覧を表示する。Action: `HelpShowContextShortcuts`。keymap 化済み操作は現在読み込まれている割り当て済みのものを表示し、固定扱いの操作は別枠で表示する |
| <kbd>Ctrl</kbd>+<kbd>M</kbd> | 隠蔽加工モード終了 |
| <kbd>Esc</kbd> | 選択中オブジェクトがあるときは選択解除、なければ隠蔽加工モード終了 |
| <kbd>T</kbd> | 隠蔽タイプを順に切替 |
| <kbd>G</kbd> | ピクセルグリッド表示 ON/OFF (ユーザーズームが等倍より拡大中、かつ高倍率時のみ画像ピクセル境界を表示) |
| <kbd>1</kbd>〜<kbd>4</kbd> | プリセット 1〜4 を呼び出し |
| <kbd>D</kbd> / <kbd>F</kbd> | 描画 / 消去モード切替 |
| <kbd>S</kbd> | 選択ツール |
| <kbd>B</kbd> | 筆ツール |
| <kbd>K</kbd> | バケツツール |
| <kbd>L</kbd> | 囲みツール |
| <kbd>P</kbd> | 多角形ツール |
| <kbd>I</kbd> | 直線ツール |
| <kbd>V</kbd> | 縦線ツール |
| <kbd>H</kbd> | 横線ツール |
| <kbd>R</kbd> | 矩形ツール |
| <kbd>O</kbd> | 楕円ツール |
| <kbd>Space</kbd>+左ドラッグ | 一時パン |
| <kbd>Ctrl</kbd>+マウスホイール | ズーム。ツールパネル上でも同じ |
| マウスホイール | 画像上ではズーム。ツールパネル上ではパネルスクロール |
| 矢印 / <kbd>Ctrl</kbd>+矢印 | 選択オブジェクト、またはオブジェクト全体を 1px / 10px 移動 |
| <kbd>Shift</kbd>+ハンドル | 端点角度・回転角をスナップ、矩形/楕円の角リサイズを等比化 |
| <kbd>Alt</kbd>+ハンドル | 矩形/楕円を中心固定でリサイズ |
| <kbd>Enter</kbd> | 多角形マスクの頂点列を確定。Action: `ConcealConfirmPolygon` |
| <kbd>Ctrl</kbd>+<kbd>Z</kbd> | マスク編集 Undo |
| <kbd>Ctrl</kbd>+<kbd>Y</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Z</kbd> | マスク編集 Redo |
| <kbd>Del</kbd> | 選択中オブジェクトを削除 |

## 補正レイヤーモード

| 入力 | 動作 |
|---|---|
| <kbd>?</kbd> (既定) | 補正レイヤーモードで使えるショートカット一覧を表示する。Action: `HelpShowContextShortcuts`。keymap 化済み操作は現在読み込まれている割り当て済みのものを表示し、固定扱いの操作は別枠で表示する |
| <kbd>Q</kbd> | 補正レイヤー直前の元画像表示 ON/OFF |
| <kbd>W</kbd> | 選択中レイヤーのマスク表示 ON/OFF |
| <kbd>D</kbd> / <kbd>F</kbd> | 手動マスクの描画 / 消去モード切替 |
| <kbd>B</kbd> / <kbd>K</kbd> / <kbd>A</kbd> / <kbd>G</kbd> | 筆 / バケツ / 境界筆 / すき間塗りツール |
| <kbd>L</kbd> / <kbd>P</kbd> | 囲み / 多角形ツール |
| <kbd>S</kbd> | 選択ツール |
| <kbd>I</kbd> / <kbd>V</kbd> / <kbd>H</kbd> | 直線 / 縦線 / 横線ツール |
| <kbd>R</kbd> / <kbd>O</kbd> | 矩形 / 楕円ツール |
| <kbd>Space</kbd>+左ドラッグ | 一時パン |
| <kbd>Esc</kbd> | 編集中の図形操作を解除。解除対象がなければ補正レイヤーモード終了 |
| <kbd>Enter</kbd> | 多角形マスクの頂点列を確定。Action: `LaConfirmPolygon` |
| <kbd>Del</kbd> | 選択中の図形マスクを削除。Action: `LaDeleteShape` |
| <kbd>Ctrl</kbd>+<kbd>Z</kbd> | 多角形入力中は頂点を戻す。それ以外は補正レイヤー操作を Undo |
| <kbd>Ctrl</kbd>+<kbd>Y</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Z</kbd> | 補正レイヤー操作を Redo |
| 矢印 / <kbd>Ctrl</kbd>+矢印 | 選択中の図形マスクを 1px / 10px 移動 |
| <kbd>[</kbd> / <kbd>]</kbd>, <kbd>Ctrl</kbd>+<kbd>[</kbd> / <kbd>]</kbd> | 選択中の図形マスクを ±0.1° / ±1° 回転 |
| <kbd>Shift</kbd>+ハンドル | 端点角度・回転角をスナップ、矩形/楕円の角リサイズを等比化 |
| <kbd>Alt</kbd>+ハンドル | 矩形/楕円を中心固定でリサイズ |
| <kbd>Ctrl</kbd>+マウスホイール | ズーム。ツールパネル上でも同じ |
| マウスホイール | 画像上ではズーム。ツールパネル上ではパネルスクロール |

## 切り取りモード

| 入力 | 動作 |
|---|---|
| <kbd>?</kbd> (既定) | 切り取りモードで使えるショートカット一覧を表示する。Action: `HelpShowContextShortcuts`。keymap 化済み操作は現在読み込まれている割り当て済みのものを表示し、固定扱いの操作は別枠で表示する |
| <kbd>Ctrl</kbd>+<kbd>E</kbd> | ドラッグ中も現在の切り取り範囲を先に確定・保存し、現在の表示結果を別ファイルへエクスポートする。Action: `CropExecute` |
| <kbd>Esc</kbd> | ドラッグ中は開始前の切り取り設定（crop なしも含む）へ戻し、保存せずドラッグだけを取り消す。モードは維持する。ドラッグしていなければ切り取りモードを終了する固定操作 |
| <kbd>Ctrl</kbd>+左ドラッグ | 既存の切り取り枠やハンドル上からでも、新しい切り取り範囲を作成する。Action: `CropForceCreateHold`。既定は <kbd>Ctrl</kbd> で、操作カスタマイズから変更・無効化できる |
| <kbd>Space</kbd>+左ドラッグ | 一時パン。`CropSpacePan` と `CropForceCreateHold` を同時に押した場合もパンを優先するが、`CropScrubZoomHold` が成立すると左右ドラッグズームを優先する。開始後に修飾キーを押す、または離しても、同じ左ドラッグを切り取り操作へ切り替えない |
| マウスドラッグ | 切り取り範囲を作成、移動、またはリサイズする。アスペクト固定時は、辺なら反対辺、角なら反対角を固定して拡大縮小する |
| <kbd>Ctrl</kbd>+マウスホイール | ズーム。ツールパネル上でも同じ |
| マウスホイール | 画像上ではズーム。ツールパネル上ではパネルスクロール |

## SNS 分割モード

| 入力 | 動作 |
|---|---|
| <kbd>?</kbd> (既定) | SNS 分割モードで使えるショートカット一覧を表示する。Action: `HelpShowContextShortcuts` |
| パネルの「分割して書き出す」 | ドラッグ中も現在の枠配置を先に確定して、エクスポートダイアログへ進む。画像が小さく選択枚数を配置できない間は無効 |
| `SnsSplitExecute` の利用者割り当て | パネルボタンと同じ書き出しを実行する。標準キーはなく、操作カスタマイズで割り当てられる |
| <kbd>Ctrl</kbd>+<kbd>E</kbd> | 通常の `FsExport`。SNS 分割中は書き出さず、パネルの「分割して書き出す」を案内する |
| <kbd>Esc</kbd> | ドラッグ中は開始前の枠配置へ戻し、ドラッグだけを取り消す。モードは維持する。ドラッグしていなければ SNS 分割モードを終了する固定操作 |
| マウスドラッグ | 枠全体を移動するか、四隅・辺のハンドルで比率を保って拡大縮小する。辺なら反対辺、角なら反対角を固定する |
| マウスホイール / <kbd>Ctrl</kbd>+マウスホイール | 画像上ではズーム。パネル上ではスクロールまたはズーム |

### crop / SNS 枠ドラッグの終了操作

終了理由は `ui_crop::OverlayDragEnd`、確定／取消の対応はその `finish()` の match を正本にする。
終端は引き続き `OverlayDragFinish::{Commit, Cancel}` の二つだけ。

| 操作・境界 | 終了理由 | 終端 |
|---|---|---|
| primary release、focus を保持した窓外 release の物理ボタン up、欠落 release 後の次の press | `Released` | 確定 |
| `CropExecute`、`SnsSplitExecute`、SNS パネルの「分割して書き出す」、出力準備成功後のモード完了 | `Execute` | 現在値を先に確定して使う |
| ドラッグ中の Escape | `Escape` | 取り消してモードを維持 |
| パネルの終了、ページ移動、close、別ツール／補正モード、再入場、preview snapshot、恒久的な park | `ModeExit` | 取り消して既存の終了・切替を続ける |
| items／編集投影の入替、削除、世代・対象ページの変更 | `OwnerChanged` | 取り消し。別 owner へ復元しない |
| 本の構造操作の開始（構造 owner が存在する最初の poll、FS 開始・結果採用より前） | `BookStructure` | 全 context の所有ドラッグを取り消し。DB は保存せず、完了後も古いドラッグを再開しない |
| フォーカス喪失 | `FocusLost` | 取り消し |
| 右ボタン押下 | `SecondaryPress` | 取り消し |
| 前面 UI への primary 入力所有移譲 | `FrontUiClaim` | 取り消し |
| パネルの数値・比率・自動 crop、SNS の枚数・比率等の明示設定変更 | `SettingsChanged` | 旧ドラッグを取り消して明示設定を採用 |

crop の確定は現在の範囲を既存 DB／sidecar 保存経路へ一度だけ渡す。SNS の枠配置は従来どおり
セッション内の値で、確定した現在の枠列を export snapshot へ渡す。出力準備が失敗しても確定を取り消さない。
実行入口と取消を伴うモード終了入口を分け、書き出しを取消経路へ通さない。
別 context の一時 mount は終了操作ではなく、所有者を維持する。F12 は既存の overlay edit policy で受け付けない。
SNS 中の通常 `FsExport` は案内だけを表示し、枠の確定・取消やモード終了を行わない。
本の構造操作中は crop／SNS ともキー・パネル・raw pointer の新規入力を受け付けない。
取消しは `poll_book_structural_op` が単独で所有し、入力 handler が呼ばれなくても実行する。

## テキスト注釈モード

| 入力 | 動作 |
|---|---|
| <kbd>?</kbd> (既定) | テキスト注釈モードで使えるショートカット一覧を表示する。Action: `HelpShowContextShortcuts`。keymap 化済み操作は現在読み込まれている割り当て済みのものを表示し、固定扱いの操作は別枠で表示する。本文入力や検索欄などが keyboard focus を持つときは入力を優先する |
| <kbd>Ctrl</kbd>+<kbd>T</kbd> | テキスト注釈モードを確定または終了する |
| <kbd>Ctrl</kbd>+<kbd>Z</kbd> | テキスト注釈編集を Undo。本文入力欄が keyboard focus を持つときは TextEdit の Undo を優先する |
| <kbd>Ctrl</kbd>+<kbd>Y</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Z</kbd> | テキスト注釈編集を Redo。本文入力欄が keyboard focus を持つときは TextEdit の Redo を優先する |
| <kbd>Space</kbd>+左ドラッグ | 一時パン。注釈の移動中は現在のドラッグを継続する |
| <kbd>Esc</kbd> | 選択を解除する。未選択ならテキスト注釈モードを終了する。本文入力中は TextEdit / IME を優先する |
| <kbd>Del</kbd> / <kbd>Backspace</kbd> | 選択中の注釈を削除する。本文入力中はテキスト入力を優先する |
| ドラッグ | 画像上で注釈を選択、移動、またはハンドル編集する |
| <kbd>Alt</kbd>+ドラッグ | Alt を押している間だけ、注釈移動の吸着とスマートガイドを一時的に無効化する。マウスドラッグ中の一時 modifier なので keymap 対象外の固定操作。フルスクリーン viewport の modifier event が stale になる場合を避けるため OS の押下状態を使う |
| <kbd>Shift</kbd>+四隅ハンドル | ドラッグ開始時の縦横比を維持して拡大・縮小する。<kbd>Ctrl</kbd> と併用すると中心固定の等比リサイズ。番号バッジとスタンプは <kbd>Shift</kbd> なしでも常に縦横比を維持する |
| <kbd>Ctrl</kbd>+四隅ハンドル | ドラッグ開始時の中心を固定した対称リサイズ。<kbd>Shift</kbd> と併用可能 |
| マウスホイール / <kbd>Ctrl</kbd>+マウスホイール | 画像上ではズーム。パネル上ではスクロールまたはズーム |
| 中ボタン+上下ドラッグ | ドラッグ開始位置を中心にズームする |
| 元画像表示の割り当て (既定: 右 <kbd>Ctrl</kbd>) | 押している間だけテキスト注釈を外した元画像を表示する。Action: `FsOriginalPreviewHold`。操作カスタマイズで変更または解除できる |

## 動画 フルスクリーン (Phase 7.H 適用後)

| キー / 入力 | 動作 | 備考 |
|---|---|---|
| <kbd>?</kbd> (既定) | 現在の動画フルスクリーンコンテキストで使えるショートカット一覧を表示 | Action: `HelpShowContextShortcuts`。egui 経路と Windows native 動画 overlay の両方で対応 |
| <kbd>Space</kbd> / <kbd>Enter</kbd> | 再生 / 一時停止トグル | Action: `VideoPlayPause`。動画 HUD 2 段化リデザイン (Phase 1) で Space を再生/停止に変更 (旧: 選択トグル)。チェックしたい場合は Esc で一覧へ戻る |
| 既定キーなし | 動画フルスクリーンを閉じて一覧へ戻る | Action: `VideoCloseFullscreen`。<kbd>Esc</kbd> は固定入力として残しつつ、操作カスタマイズで追加の閉じるキーを割り当てられる |
| <kbd>Backspace</kbd> | 一覧へ戻る | Action: `FsBackToList`。画像フルスクリーンと同じ。native presenter 経路でも App 側へ転送する |
| <kbd>Home</kbd> / <kbd>End</kbd> | 先頭 / 末尾の項目へ移動 | Action: `FsJumpFirst` / `FsJumpLast`。native presenter 経路でも App 側へ転送する |
| <kbd>Shift</kbd>+<kbd>Enter</kbd> | 外部プレイヤー起動 | Action: `VideoExternalPlayer` |
| <kbd>←</kbd> / <kbd>→</kbd> | 中シーク (既定 5 秒) | 修飾なし矢印は固定ナビゲーション。タイル中はカーソル移動を維持 |
| <kbd>Shift</kbd>+<kbd>←</kbd> / <kbd>→</kbd> | 小シーク (既定 1 秒) | Action: `VideoSeekBackSmall` / `VideoSeekForwardSmall` |
| 既定キーなし | 中シーク (既定 5 秒) | Action: `VideoSeekBackMedium` / `VideoSeekForwardMedium`。固定矢印以外にも割り当て可能 |
| <kbd>Ctrl</kbd>+<kbd>←</kbd> / <kbd>→</kbd> | 大シーク (既定 30 秒) | Action: `VideoSeekBackLarge` / `VideoSeekForwardLarge` |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>←</kbd> / <kbd>→</kbd> | 1 フレーム戻る / 進む | Action: `VideoFrameStepBack` / `VideoFrameStepForward` |
| <kbd>V</kbd> | 表示モードを切り替える | Action: `FsPanorama`。360 候補の動画では 360 度表示、それ以外の通常動画では拡大・縮小表示を ON / OFF にする。表示中はホイールでfit比10%〜1600%の範囲を拡大縮小し、左ドラッグで移動する。上バーには倍率とリセットボタンを表示し、項目を切り替えると表示モードを終了する |
| <kbd>T</kbd> | 動画の拡大方法を OS に任せる → 標準（補間あり）→ ニアレスト（補間なし）→ シャープ拡大 → アニメ塗り拡大の順に切り替える | Action: `VideoScaleFilterNext`。一時停止中も現在フレームへ即時反映し、設定へ保存する。反映完了後に右上へ選択を表示し、表示サイズの上限で適用できない場合は「OS に任せる」で表示することも併記する |
| 割り当てなし | 動画のアニメ塗り拡大の性能をもう一度測定する | Action: `VideoAnime4kRemeasure`。再生位置と現在表示を維持し、完了後だけモデルを再選択する |
| <kbd>Ctrl</kbd>+<kbd>1</kbd>〜<kbd>9</kbd> / <kbd>Ctrl</kbd>+<kbd>0</kbd> | 動画補正保存スロット 1〜10 を読み込む | Action: `VideoAdjustSlot1..10`。通常数字とテンキー数字の両方を既定割り当てにする。映像が隠れた通常の音声モードでは無効、VST GUI で映像が見えている間は有効 |
| <kbd>←</kbd> / <kbd>→</kbd> (タイル中) | タイルカーソルを前 / 次へ移動 | seek しない。現在位置より後の最初のタイルを時刻ラベル込みで強調表示 |
| <kbd>Ctrl</kbd>+<kbd>←</kbd> / <kbd>→</kbd> (タイル中) | タイルカーソルを 1 行分移動 | 列数分だけ前 / 次へ移動 |
| <kbd>Space</kbd> / <kbd>Enter</kbd> (タイル中) | タイルカーソル位置から再生 | S / Esc で閉じた場合は再生位置を変更しない |
| <kbd>P</kbd> (タイル中) | タイルカーソル位置のサムネイルを代表フレームとしてピン留め | マウス hover ではカーソルを動かさない。マウス操作はタイルクリックだけが seek として反応する |
| <kbd>↑</kbd> / <kbd>↓</kbd> | **前 / 次のファイル** (画像と同じ) | Phase 7.H |
| 修飾なしマウスホイール | 前 / 次のファイル（既定）、または音量調整 | `Settings.ring_shortcuts.video_normal_wheel_action`。動画・音声共通で、操作カスタマイズ「マウスボタン」から選択。パネル・モーダル・ストリップ、360度・拡大表示など既存の入力所有を優先し、KeyAction にはしない |
| <kbd>Shift</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | 音量を dB フェーダー目盛りの 1/4 幅で上下 | |
| <kbd>Ctrl</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | 現在コンテキストの前 / 次フォルダまたは検索結果へ移動 | native presenter 経路でも有効。切り離した detached 窓 / always-new 窓ではメイン一覧を動かさず、案内だけ出す |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | 最後に通常フォルダ一覧から本を開く操作を受け付けた時点の、その一覧の表示順で前 / 次の本を開く | `FsListOrderPrev` / `FsListOrderNext`。native presenter でも同じ handler を使う。別窓・★固定中・保持した一覧に現在の本が無いときは案内だけ出し、準備中の追加入力は受け付けない。native の repeat 受付は既存 Ctrl+↑↓ と同じく無視 |
| <kbd>Ctrl</kbd>+<kbd>PageUp</kbd> / <kbd>PageDown</kbd> | 前 / 次の兄弟フォルダへ | 同じ親の直下だけを対象にし、空フォルダも skip しない。検索中は無効。切り離した detached 窓 / always-new 窓では無効 |
| <kbd>M</kbd> | ミュート トグル | |
| 既定キーなし | 次の再生可能な音声トラックへ循環 | Action: `VideoNextAudioTrack`。`[FsVideo]` で割り当て可能。動画 HUD と動画→音声モード HUD の「音声 N」からも直接選べる。1 本以下では何もしない |
| <kbd>L</kbd> | ループ再生 トグル | 連続再生 ON 中は無効化し、「連続再生中はループ無効」を表示 |
| <kbd>B</kbd> | ブックマーク追加 (現在位置 🔖) | |
| <kbd>S</kbd> | タイルモード ON/OFF | |
| HUD 連続再生ボタン | オフ → 連続再生 → 連続再生 + ループを循環 | ループ再生とは排他。アプリ再起動時は OFF |
| <kbd>Ctrl</kbd>+<kbd>S</kbd> | 現在フレームをキャプチャ保存フォルダへ保存 | v0.10 MVP。egui / native presenter 両経路で有効 |
| <kbd>X</kbd> / <kbd>C</kbd> / <kbd>Shift</kbd>+<kbd>C</kbd> / <kbd>Alt</kbd>+<kbd>C</kbd> | 比較ビュー対象外のため silent no-op | native presenter 経路でも passthrough しない |
| <kbd>P</kbd> | 現在再生位置をピン留め (= HUD 📌 ボタンと同等)。タイル中はタイルカーソル位置をピン留め | v0.9.x、グリッドの P (folder_thumb_pin toggle) と統一した「P = Pin」 |
| <kbd>F</kbd> | Perf / フレームレート オーバーレイ トグル | v0.9.x、以前は P。P を Pin に再割り当てしたため F (Frames) へ移動 |
| <kbd>Esc</kbd> (タイル中) | タイルモード解除 | |
| マウス左クリック / 左ドラッグ | 通常時は再生 / 一時停止トグル。通常動画の拡大・縮小表示中は表示位置を移動 | HUD / パネルを除く。100%未満は動画全体を表示領域内に保った範囲で移動できる |
| マウスホイール | 通常時は前 / 次のファイル。通常動画の拡大・縮小表示中はfit比10%〜1600%で倍率を変更し、全倍率でポインタ位置を保つ | シークストリップ、端パネル、モーダルが所有する領域では拡大・縮小表示が奪わない。表示範囲の端では画像を失わない制約を優先する |
| <kbd>Ctrl</kbd>+ホイール (タイル中) | 列数切替 (4/6/10/16/20/26/30) | 上部バーの 3x3 / 5x5 アイコンボタンでも同じ操作可 |

## 不整合の解消 (Phase 7.H 適用後)

| 入力 | 画像モード | 動画モード | 状態 |
|---|---|---|---|
| <kbd>↑</kbd> / <kbd>↓</kbd> | 前 / 次ファイル | 前 / 次ファイル | ✅ 揃った |
| マウスホイール | 前 / 次ファイル | 前 / 次ファイル | ✅ 揃った |
| <kbd>Ctrl</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | 前 / 次フォルダまたは検索結果 | 前 / 次フォルダまたは検索結果 | ✅ 揃った。切り離した detached / always-new では両方 no-op |
| <kbd>Shift</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | 前 / 次ファイル (= ↑↓ と同義)。スタックのフラット読書中は前 / 次スタックへジャンプ (v2.0.0) | 音量を dB フェーダー目盛りの 1/4 幅で上下 | ⚠ 残った差異 (許容、動画プレイヤー慣例) |
| <kbd>←</kbd> / <kbd>→</kbd> | 前 / 次ファイル | 中シーク（既定 5 秒） | ⚠ 動画プレイヤー慣例 (mpv/VLC/YouTube) で許容 |
| マウス左クリック | ページめくり | 再生 / 一時停止 | ⚠ 動画プレイヤー慣例で許容 |

## ★固定 (Snapshot Lock) 中の挙動

★ filter ツールバー右端の `[★固定]` ボタンで現在の絞り込み結果を一時 snapshot 化して
凍結する機能 (v1.1.0+、設計: [star-lock-snapshot-design.md](star-lock-snapshot-design.md))。
snapshot 中のキー操作は以下のように再定義される:

| 入力 | snapshot 中の挙動 |
|---|---|
| <kbd>Ctrl</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | snapshot 内の前/次 entry へ (= 混合 nav: Folder/Image/Video 全部対象、Folder entry は中の最初の image を fullscreen で開く) |
| <kbd>Ctrl</kbd>+<kbd>PageUp</kbd> / <kbd>PageDown</kbd> | snapshot 内の前/次 **image-like** entry のみへ (Folder/Zip/Pdf entry は skip) |
| スライドショー末尾 | snapshot 内の次 playable entry へ自動遷移 (= ★5 folder 巡回の主用途) |
| ★ filter ボタン | 操作可能、ただし top-level grid 表示は snapshot のまま凍結 (= captured folder の中身には作用する) |
| <kbd>Backspace</kbd> / <kbd>Alt</kbd>+<kbd>↑</kbd> / <kbd>Alt</kbd>+<kbd>←</kbd>/<kbd>→</kbd> / フォルダツリー / お気に入りクリック / フォルダパス入力 | 無効、toast「スナップショット中は他のフォルダに移動できません」 |
| <kbd>Ctrl</kbd>+<kbd>F</kbd> / <kbd>Ctrl</kbd>+<kbd>S</kbd> / <kbd>Ctrl</kbd>+<kbd>G</kbd> / <kbd>Ctrl</kbd>+<kbd>T</kbd> | snapshot 自動解除 + 検索 mode 起動 (= scope mutual exclusion) |
| `[★固定]` ボタン再クリック | snapshot 解除 (= 元のフォルダ表示に戻る) |

snapshot 末尾到達時は `FsBoundaryHint::NoImageFolder` で boundary hint を表示。

## 設計メモ

一覧の余白ダブルクリック (§1.298) は環境設定 → 表示 → サムネイルの
「サムネイルの余白部分のダブルクリックで親フォルダへ移動」で有効にする（既定OFF）。
ON時のサムネイルでは、描いた画像矩形・ファイル名・アイコン・バッジ以外のセル内余白
（縦長／横長画像のletterbox、4ptの外周余白）とセル間・最終行より下の背景が対象。
画像・ラベルは選択／openを維持する。OFF時はセル全体が従来どおり項目領域。
詳細一覧は最終行より下の空白だけが対象で、行内の空列は項目領域のまま。
操作バー、dialog／popupは除外する。touch／ペンのdouble tapにも同じ設定を使う。
描画ownerの矩形をそのframeの入力判定へ渡し、押下と解放の所属を一つのpairing ownerで照合する。
同じセルの画像1打→letterbox1打も背景の対にはならない。
背景2打は同じ一覧世代・表示面・入力種別、OSのdouble-click時間と既存`max_click_dist`内でのみ対になる。
押下／解放の所属不一致、scroll／pinch／cancel、一覧変更や表示面切替で対を切る。
発火時は対を消費し、既存の選択解除を行ってからキー`GridParentFolder`と共有する親操作へ渡す。
Explorer方式では選択を解除し、Check方式では維持する。
ドライブルートはドライブ一覧へ戻り、検索・仮想一覧は既存キーの戻り方／no-opを保つ。
これは固定ポインター操作の専用設定であり、キーのchord割り当ては変更しない。
[決定と入力所有・セル種類・検証の詳細](grid-background-double-click-plan.md)を参照。

- 動画モードで ↑↓ をファイル移動に再アサインする方針は、旧 egui 経路では
  `handle_video_input` がプレーン ArrowUp/ArrowDown を consume せず後段へ流すことで
  実現していた。現行 Windows native presenter 経路では
  `app/native_video.rs::handle_native_video_key_event` が plain ↑↓ を直接
  `navigate_native_video_fullscreen` に流している。
- Ctrl+↑↓ も同じ思想で native key handler から
  `handle_fullscreen_ctrl_nav_context` へ流し、フォルダ / Ctrl+S / Ctrl+G の
  スコープ解決を画像系と共有する。
- **Shift+↑↓ のスタックジャンプ (v2.0.0) は 2026-06 の Phase 4 初期実装で
  `KeyAction::FsStackJumpPrev` / `FsStackJumpNext` 化した。** 同じ Shift+↑↓ は
  ① 非スタック時のプレーン ↑↓ エイリアス (ページ送り)、② 動画再生中の音量、
  ③ スタックのフラット読書中のスタックジャンプ、の 3 用途をコンテキストで出し分ける。
  `ui_fullscreen.rs` では動画の `VideoVolume*` / `VideoNextFile` / `VideoPrevFile`
  を先に扱い、非動画かつスタックフラット時だけ `FsStackJump*` を見る。これにより
  native 動画経路と App 側の縦方向 keymap 解決を近づけつつ、通常の Esc /
  plain 矢印ナビゲーションは固定扱いのまま残す。Enter / Backspace / Home / End は
  文脈ごとの `KeyAction` として扱う。
- **ZipPla 風 全画面ズームは `KeyAction::FsZoomMode` (KeyHold トリガ、既定 <kbd>Z</kbd>) として扱う**。
  「押している間=照準 (枠表示) / 離す=ズーム確定 / ズーム中の押下=解除」というホールド + トグルの
  ハイブリッドだが、`KeyHold` アクション基盤 (Shift ルーペ `FsLoupeHold` / 編集モードの Space パンと
  同じ枠組み) に乗せてカスタマイズ可能にした。押下状態は `keymap.key_held_action` (OS 直読み =
  フルスクリーンビューポートで stale な egui key_down を回避) で取り、高速タップ (idle からの同
  フレーム押下+離し) は `keymap.take_key_hold_edges` で補完する。Windows では held と edge の
  両方を Win32 物理入力へ統一し、`Enter` / `NumpadEnter` は extended bit 別の押下ラッチを
  参照する。その他の固有 VK は従来どおり `GetAsyncKeyState` を使う (Codex P2)。
  状態 (`fs_zoom_active` / `fs_zoom_aiming` / `fs_zoom_factor`) は settings に保存せずアプリ
  セッション内のみ保持し、グリッドへ戻ると解除 (倍率は維持)。
- **画像分析モードは `FsAnalysis` → `FsImageAnalysis` へ改名し、既定 chord を <kbd>Shift</kbd>+<kbd>Z</kbd>
  へ移動**した (v2.0.0)。改名により、旧バージョンの `keymap.ini` に残る `FsAnalysis = …` の割当ては
  **未知アクションとして無視され**、全員が新既定 (Z=ズーム / Shift+Z=分析) へ移行する (旧 Z=分析の
  カスタムと固定ズームが衝突するのを避けるための clean break)。分析を別キーへ割り当てていた場合も
  新名 `FsImageAnalysis` で設定し直す。`FsZoomMode` と `FsImageAnalysis` はどちらも `keymap.ini` で
  カスタマイズ可能。
- 5/1/30 秒シークの粒度は動画プレイヤー一般の慣例 (mpv: ←→=5s, Shift+←→=1s,
  ←/→ alone in YouTube=5s, J/L=10s) を踏襲しつつ、modifier で粒度切替できる
  ようにした。v3.7.0 ではこの値を小 / 中 / 大の初期値として維持し、各段階の秒数を
  環境設定で変更できるようにする。
- 既に先頭 / 末尾に居て ←→ シークが動かない場合は、シークを発行せず
  「動画先頭です」「動画末尾です」のトーストを出す (詳細は
  [video-architecture.md](video-architecture.md) の seek HUD 節)。
- タイルモード中の ←→ は修飾キーの有無に関係なくタイルカーソル移動を優先する。
  Shift は無視し、Ctrl が含まれる場合だけ 1 行分移動にする。
- 音量は HUD 下部の dB フェーダーをマウスでドラッグして調整可能。
  キーボードでは Shift+↑↓ で -∞/-60/-40/-20/-10/-5/0/+6/+12/+18dB の
  目盛り間を 1/4 幅ずつ移動し、plain ↑↓ アサインは廃止 (= プレーン ↑↓ を
  ファイル移動に譲るため)。

### F11と固定表示の一時抑制 (§1.344)

環境設定の対象がONなら、既存 `FsToggleWindowMode` によるF11全画面の間だけ固定表示を自動表示へ投影する。
HUD・native VK・カスタム割り当て・初回fullscreenも同じ表示先契約。新しい固定キーやKeyActionは追加しない。
F12通常窓は抑制せず、その窓のF11拡大だけが対象。ナビゲータは既存 `FsNavigatorHold`（既定Alt）を使う。

## ツールバーに固定した検索欄 (§1.409)

既存 `GlobalFavSearch / GlobalLocalSearch / GlobalMetadataSearch / GlobalEverythingSearch` の
受付条件を維持し、対象の固定がONなら同じ欄へpending text-input focusで移す。
固定Ctrl+Gは終了toggleにせずfocusする。固定OFFの一時バーとCtrl+G toggleは従来どおり。
新しいKeyActionは追加しない。Enter/EscはIME helperを通す固定の入力操作である。

Escは全検索バー描画後の単一routerが一度だけ消費する。modal/IME→描画前の検索入力owner→
適用結果または有効sessionの順。非active欄ではfocus解除だけ。検索終了時に固定欄のdraftは
保持し、Everything実行中は取消だけを行う。egui begin_passでfocusが消えたときは既存recoveryの
viewport/pass/pointer優先条件に従って入力Idを捕捉し、描画後のhas_focusで選び直さない。

固定OFFの一時バーのEscは、そのバーの×と同じく共通検索sessionを終了して元一覧へ戻る。
別sourceの結果を残したまま未送信入力を閉じて入力先だけ戻す(b)は、固定欄に限る。
全OFFとG/Rの片方だけ固定の場合も、この入力ownerの区別を保つ。

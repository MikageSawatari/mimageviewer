# §1.327 起動時の白いウィンドウの調査

2026-10-04 / `next-startup-flicker`、調査対象 `a0aea7fe2`。
実装担当によるソース調査と、別 context の GPT-6.1 Sol / xhigh による独立設計レビュー。
**修正未実装・製品未起動。白フレームと HWND の対応は未確定。**

利用者の「診断先行・修正は後」の判断を受け、opt-inのnative診断を実装した。
2026-10-04、調査文書commit `d82f94d76` の後続。表示／配置の挙動修正は含まない。
採取方法・観測限界は §6。利用者実行のログ・録画はまだない。

## 1. 利用者の録画から読めること

利用者は新規の隔離 `--data-dir`、保存 placement なし、VST3 / EffeTune なしでも
起動時のちらつきを報告した。資料は `target/flicker/f_01.png`〜`f_24.png`、
4K / 60 fps の録画を幅1280へ縮小した24枚（隣接フレーム約16.7 ms）。

実装担当が資料を見た結果:

- f_01〜05: 起動元の PowerShell と背景アプリ。mIV の表示は読み取れない。
- f_06〜07: 画面の大部分を覆う半透明の白い窓。f_06 には `mimageviewer` のタイトル。
- f_08: 画面全体に近いタイトルバー、client は背景が見える。
- f_09: 左上、約640×410（縮小画像上）のタイトルバー付き窓。
- f_10: 左上の白い窓と、約(498,195)〜(1138,608)の中央寄りの白い窓が重なる。
- f_11: ほぼ全画面の白い client と `mimageviewer` のタイトルバー。
- f_12〜19: 大きい白い窓が約170 msかけて薄くなる。
- f_20〜24: 背景アプリへ戻る。この24枚には、描画済みの mIV 本体は写っていない。

資料の全フレームについてPILの8bit grayscale平均を計算した値は、背景f_05が113.80、
f_06/07が145.90/172.66、f_08/09が114.88/114.13、f_10/11が200.76/248.95、
f_12が236.37、f_19が127.85、f_20が116.96、f_24が113.94。2イベントと後半のfadeは
この資料の画素にも表れている（製品の追加実行による測定ではない）。

重なる矩形だけから「複数の HWND」「中央の窓が本体」「別 viewport が破棄された」とは
証明できない。同じ HWND の DWM animation の残像でも説明できる。2つのイベントを
**main root の配置・最大化／復元と非表示操作によるものと推測する**が、以下の
STARTUPINFO と最初からの native HWND 記録が必要。

## 2. 起動時のウィンドウ所有者

| 所有者 / 経路 | 生成・可視性のソース上の契約 | 今回のフレームとの対応 |
| --- | --- | --- |
| eframe ROOT | `src/lib.rs` の NativeOptions。通常は1280×800 logical、最小640×580、保存サイズ／有効な保存位置を復元。`--window-size` はサイズを優先し位置(60,40)、最大化を抑止。位置なしは Windows 既定。`resolve_startup_maximized` が最大化を決める。eframe `wgpu_integration::create_window` は常に `visible=false` を上書きし、`EpiIntegration::post_rendering` が初回 paint 後に `set_visible(true)` | タイトルと矩形変化に整合する第一候補。実 HWND 対応は未確定 |
| winit の thread event target | event loop 作成時。0×0、タイトルなし、LAYERED / TOOLWINDOW / NOACTIVATE。WM_PAINT のため native WS_VISIBLE は立つが desktop に描画されない | 録画のタイトル付き大窓とは一致しない |
| clipboard listener | App creator から observer worker を開始。`cut_clipboard` は0×0、`HWND_MESSAGE`、WS_VISIBLEなし | タイトル付き大窓とは一致しない |
| tray-icon の receiver | tray設定が有効な場合だけ `sync_tray_with_settings` から生成。タイトルなし、LAYERED / TOOLWINDOW / NOACTIVATE、WS_VISIBLEなし | タイトル付き大窓とは一致しない |
| activation listener | main HWND 捕捉後、named event / IPC worker を開始。独自の表示窓は生成しない。既存 main を復帰させる経路はある | 追加 HWND の候補ではない |
| 起動中 overlay | `render_startup_overlay` は ROOT の CentralPanel / painter / spinner / label。独自 HWND / viewport は作らない | 白い別窓の所有者ではない |
| fullscreen / detached host | 空の通常起動では `fullscreen_idx`、detached opening request、shown-host cleanup のいずれもなく、`keep_fullscreen_viewport_alive` は viewport を作らない。明示のファイル引数・ユーザー open では既存の専用経路が生成する。新規 detached は hidden builder、描画後の visible commit | 保存placementなしの空起動で early secondary viewport を生成する根拠なし |
| native video presenter / HUD | media open 後に native output thread / window pump が生成する。GPU device の creator 初期化は HWND を生成しない | 空起動では生成しない |
| VST3 / EffeTune editor | configured plugin の background load / GUI復元から生成。hidden指定と保存位置を持つ | 利用者の隔離再現条件では除外される |
| launcher splash | launcher は extraction / process spawn を行うが、通常成功起動に splash HWND はない。失敗時の MessageBox は別。今回の録画のコマンドは core の直接起動 | 今回の候補ではない |

上表はソース調査の inventory であり、利用者 PC 上での全 HWND 捕捉記録ではない。
winit / egui-winit は現在 registry の `winit-0.30.13` / `egui-winit-0.33.3` を使用。
eframe persistence feature は有効ではなく、eframe の別保存 placement はこの経路にない。
起動時のSusie workerと、設定ON時のremote service子プロセスはCREATE_NO_WINDOWで起動する。
通常の空起動でPDF / EPUB / TRT workerの表示窓を作る経路もない。

## 3. ソースと API から分かる不変条件違反

期待する契約は「明示の visibility commit 前には、配置／最大化／復元の準備から
desktop への表示・アクティブ化を起こさない」。アプリに `with_visible(false)` を
追加するだけでは修正できない。eframe は既にその指定をしている。

winit Win32 の `window_state.rs::WindowFlags::apply_diff`:

1. flags が変わり、変更後に VISIBLE がなければ、VISIBLE の差分がなくても
   `ShowWindow(SW_HIDE)` を実行する。初回は `WM_NCCREATE` 内、サイズ・位置の適用前、
   GWL_USERDATA の登録前に実行される。
2. MAXIMIZED の差分、または変更後に MAXIMIZED がある場合、hidden でも
   `ShowWindow(SW_MAXIMIZE/SW_RESTORE)` を実行し、その後 `SW_HIDE` を実行する。
3. `window.rs::init` は CW_USEDEFAULT で生成し、WM_CREATE の通常矩形設定後に
   `set_maximized(true)` を行う。egui-winit の生成後補正は inner size / position を
   再適用してから maximized を再適用する。最大化指定では、通常→最大化→補正／復元→
   最大化の処理が hidden のつもりでも表示 API を通る。

[Microsoft STARTUPINFO](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/ns-processthreadsapi-startupinfoa)
と [ShowWindow](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-showwindow)
は、起動元が STARTF_USESHOWWINDOW を指定した GUI process の最初の ShowWindow が
起動元の wShowWindow に従うことを説明している。したがって上の1は「非表示初期化が
表示を起こす」成立条件を持つ。ただし今回の起動元の実 dwFlags / wShowWindow は未記録。
新規設定のアプリ要求は `RememberLast` + `window_maximized=false` なので、録画の
全画面矩形を「アプリの保存最大化要求」と断定してはいけない。

当初の通常ログ（2026-10-04 18:06の起動）では、0.88秒にmain HWND捕捉、0.884秒に
VST3起動load、0.940秒に`tray: detected external ShowWindow — running sync_after_restore`。
これはcreator以前のnative表示を記録しておらず、白フレームのHWND同定には使えない。
後から得た隔離再現の利用者報告により、VST3 / EffeTune必須という仮説は除外された。

別の表示順上の問題もある。`App::apply_deferred_initial_size` は startup overlay の
early return より後にあり、overlay 初回 paint → eframe show → 初期サイズ補正の順となる。
さらに root の output commands は paint / `post_rendering` の後に適用される。
これも最初の表示時点での DPI 補正完了という契約を満たしていない。ただし今回の
白フレームの原因かは未確定。最大化時には補正を保留する既存仕様があり、単純な移動では足りない。

## 4. 独立設計レビューと実装境界

実装担当と独立 reviewer は、hidden 初期化と native ShowWindow の所有境界に問題が
あることには一致した。**backend 修正案への設計合意は未成立**。
表示／配置の挙動修正は実装していない。後続の診断のみ §6 のとおり追加した。

退行を作る近道は採用しない:

- 冗長 SW_HIDE だけの撤去: STARTUPINFO の最初の呼出上書きが post-paint の SW_SHOW へ
  移り、保存最大化が通常表示へ変わる可能性を残す。
- 最大化を表示時へ延期: 通常サイズで描いた初回内容と表示寸法が食い違う。
- WM_WINDOWPOSCHANGING の gate: 最初の呼出は userdata 登録前。そこで追加した
  SWP_NOACTIVATE は [公式仕様](https://learn.microsoft.com/en-us/windows/win32/winmsg/wm-windowposchanging)
  上無視されるので、SWP_SHOWWINDOW を消すだけで activation も防げるとは言えない。
- cloak、透明化、delay、追加 repaint、別の dummy window に初回 ShowWindow を消費させる
  対応は、表示／配置 owner を直さないため採用しない。

[Window Features](https://learn.microsoft.com/en-us/windows/win32/winmsg/window-features) の
WS_MAXIMIZE + WS_VISIBLE なしの native 生成は候補だが、現 winit が生成後に最大化する
理由は復元矩形の保持。初期矩形、WM_CREATE、DPI確定、生成後のegui-winit補正と、初回
paint / visible commit を一体で設計する必要がある。winit vendor化だけへの了承を、
未解決の backend 設計への合意として扱わない。

未解決の Win32 問題に限定した GPT-6 Astra / medium の追加確認も、現段階では診断を
先行させる結論だった。候補は hidden + WS_MAXIMIZE 生成、復元矩形とDPIを維持した
初回描画、[SetWindowPos](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowpos)
の SWP_SHOWWINDOW による表示commit。ただし後続の最初の ShowWindow に起動元指定が
残る問題は未解決で、tray等の操作まで確認が必要。
[WINDOWPLACEMENT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-windowplacement)
の復元矩形は一般にworkspace座標なので、screen座標のAPIへ単純コピーできない。
これは採用済み設計ではなく、診断とnative feasibility検証で評価する候補。

組み合わせを減らす案は、既存の startup modal と root の初回 visibility commit を維持し、
その前の準備を native owner に集約すること。通常使用への追加待機や機能制限は必要ない。
detached の述語や placement owner を別に増やさない。main tray は winit VISIBLE を true に
保ったまま native hide / restore を行い、detached は Visible(false/true) を使う違いも維持する。
共有 viewport backend を変更する前に、設計担当・独立 reviewer の構造修正への合意と
`detached-rework-plan.md` §11 の記録が必要。

## 5. 次の観測と受入条件

次の bounded brief の診断を §6 の方法で実装した。起動直前の STARTUPINFO と、
最初の HWND 生成からの native通知、requested visibility、実visibility、foreground、
矩形と DPI を記録する。creator で main HWND を得てからの記録だけでは遅い。
利用者が隔離起動と録画を行い、2イベントと各 HWND / 操作を対応付ける。
その結果を根拠に hidden preparation と明示 visibility commit の backend 設計を確定する。

修正後に必要な自動検証:

- placement選択（新規／保存正常／画面外／CLI size／通常／最大化）と、初回準備から
  paint、明示showまでの順序を純粋な state-transition test で固定する。
- normal + portable core check、関連 lib test、fmt。共有 backend の変更では全体gateも実行。
- main tray の native復帰、detachedの明示Visibleとsibling ownerの維持を回帰対象に含める。

利用者の実機シナリオ（エージェントは起動しない）:

1. 通常profileの起動前にインストール版／常駐tray版を終了する。同じdata dirの
   single-instance mutexを共有する。明示の別data dirは別namespaceになる。
2. 毎回新しい空の `--data-dir` で起動し、初回の白い大窓／左上の残像がなく、通常の本体窓が
   配置済みの状態で一度だけ現れるか録画する。
3. 通常profileで、通常サイズで終了→再起動、最大化で終了→再起動、最大化解除後の
   元の通常矩形、起動状態設定3種を確認する。通常profileは実設定／データを更新し得る。
4. 2モニター（可能なら異なるDPI）で両方の保存位置、最大化、通常復元、切断した
   モニターの保存位置、`--window-size`、tray格納／復帰を確認する。

通常profileの手動起動コマンド:

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

新規隔離profileの例（その都度未使用の名前に変える）:

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe -ArgumentList '--data-dir', '.\target\flicker\fresh-user-check-01'
```

現時点の dev build は診断機能を持つ**ちらつき未修正版**。上の「白い窓がない」という
受入判定は修正後に行う。ビルド／テスト結果は `target/flicker-msg.txt` に記録する。

## 6. native診断の採取と読み方 (2026-10-04)

`--diag-startup-windows` を渡したcoreプロセスだけで有効。env varや設定DBは使用しない。
通常のdev-runtime build、portable、明示の `--data-dir` で使える。
無効時はフック・QPC/native照会・worker・ファイルI/Oを始めず、CLI scanとmilestoneの
未登録チェックだけ。診断は表示／配置APIの引数・呼出順・WndProc/CBTの戻り値を変更しない。

### 採取

worktreeのPowerShellで実行する。新規隔離profileの例:

```powershell
$startupDiagDir = Join-Path (Get-Location) ('target\flicker\fresh-diag-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe -ArgumentList @('--diag-startup-windows', '--data-dir', ('"{0}"' -f $startupDiagDir))
```

本体が現れてから2秒程度は終了せず、同時に起動直前からの画面録画を残す。
ログは `$startupDiagDir\logs\startup-windows.log`。
明示の別data dirは別single-instance namespaceなので、通常profileの既存プロセスへ転送されない。

通常profileは、インストール版・tray常駐版を先に終了してから:

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe -ArgumentList '--diag-startup-windows'
```

この通常版coreは `%APPDATA%\mimageviewer` の実設定／データを更新し得る。
同じdata dirのsingle-instance mutexを共有する。ログは
`%APPDATA%\mimageviewer\logs\startup-windows.log`。
診断起動ごとにそのログを上書きするので、次の起動前にログ・録画をコピーしておく。
無効時は既存診断ログを変更しない。古いログとの混同はheaderのpid／process creation FILETIMEで確認する。

収集期間はcore診断入口から最大10秒、またはeframeの最初の `set_visible(true)` が戻って
から1秒まで。診断workerがメモリの記録をUTF-8 BOMなしJSON Linesへ保存して終了する。
mainが表示されない場合も10秒上限で保存する。data dir決定前の即時終了やprocess強制終了は
保存を保証しない。フラグ付きの `--help`／worker内部モードは診断対象外。

### ログの実装境界

- core `run()` の入口でSTARTUPINFO.dwFlags/wShowWindow、creation FILETIMEとQPCを取得する。
  eframe/winit event loop、起動worker、ネイティブ窓を作る前に診断workerの
  WinEvent登録完了を待ち、UI threadへCBT／CALLWNDPROCフックを登録する。
  このhandshakeはGUI開始前だけ。App::updateはworker待機・join・ファイル保存をしない。
- workerの `SetWinEventHook` はOUTOFCONTEXT、process id限定、全thread対象。
  OBJECT CREATE/SHOW/HIDE/DESTROY/LOCATIONCHANGEとSYSTEM FOREGROUNDを選ぶ。
  OBJID_WINDOW/idChild=0のwindow通知を選ぶ。同期UIフックはmessage-only／childを除く。
  WinEventは配送前に破棄・再利用された短命窓のCREATE/SHOWまで失わないようbare通知を残す。
  top-levelを確定できないものはscope unknown、historical identityも未確定と表示する。
- UI threadのCBTはCREATEWND (WM_NCCREATE前)、MINMAX、ACTIVATE、MOVESIZE、DESTROYWND。
  CREATESTRUCTから生成要求のtitle/style/parent/xywhをコピーする。
  CALLWNDPROCはWM_NCCREATE/CREATE/SHOWWINDOW/WINDOWPOSCHANGING/CHANGED/SIZE/DESTROYの
  WndProc実行前を記録する。生成要求のrectは最終rectとは区別する。
- native snapshotはclass/rect/style/exstyle/visible/DPI/owner pid・thread/foreground HWND。
  UIではtitleの同期messageやDWM照会を行わない。workerが別のdelivery_snapshotに
  title (WM_GETTEXT timeout 20ms)／DWMWA_CLOAKEDを補足し、それぞれsample_qpcを保存する。
  title取得失敗・破棄済み窓などはnull。以前のtitleはcached_identityに分ける。
- milestoneはNativeOptions決定 (size/position/maximized/UI scale)、App creator開始、
  tray用途のmain HWND捕捉、初回paint呼出からpost_renderingへ戻った境界、
  set_visible(true)前／後、deferred初期sizeの判断とInnerSize command発行。
  `eframe.first_paint.call_returned` はsurface取得skipでも通るので、描画成功の証明ではない。
- 同期UIのHCBT_CREATEWNDだけがnative_generationを進める。遅配WinEvent CREATE/DESTROYは
  その世代を変更せず、WinEvent自身のevent世代はunknown。別世代のdelivery titleを
  以前のUI snapshotへ混ぜない。
- 非blockingの有界record queueと16,384件のbuffer。header/output pathとhook handleは
  lossy queueに載せない。終了時は受付を閉じ、進行中callbackとaccepted recordをworkerで
  回収してunhook／保存する。pump/drainにはbudgetとdeadlineを置く。欠落数はfooterのdropped。
  フック登録失敗はhooks.*.readyのerrorコードに記録する。

診断を単一owner・単一期限に閉じ、アプリ操作とのrollbackやresumeを設計しない形で
組み合わせを減らした。modal化・別窓の閉鎖／再openは挙動を変えるので診断には採用しない。
独立Sol/xhighレビューで観測限定の設計を確認し、生成世代・終了回収・paint名称の指摘を修正した。

### 読む

```powershell
python .\scripts\analyze_startup_windows.py "$startupDiagDir\logs\startup-windows.log"
python .\scripts\analyze_startup_windows.py "$env:APPDATA\mimageviewer\logs\startup-windows.log"
```

最初にhook error=0、footerのdropped=0、main_visible_commit=Trueを確認する。
`entry_ms`はQPCによるcore診断入口からの時刻。raw qpc/entry_usが順序の正本。
process生成からのt_usはcreation FILETIMEとQPCの一度の較正による推定。
WinEventのOS_event_process_msはdwmsEventTimeからのms精度の推定で、配送遅延も併記する。
UI行は同期callback前の状態、WinEvent行は非同期配送時点の状態。
title_at_delivery／cloaked_at_delivery／cached_title_last_observedを過去イベントの状態と混同しない。

録画の白矩形と同じrect/titleを持つHWNDを探し、main capture milestoneのHWNDと比較する。
同じUI native_generationで生成→位置変更→show/hideが続くなら同一窓の経路、異なるHWND／
世代なら別窓として読む。最初のeframe明示showより前のSHOW／SWP_SHOWWINDOW／activationも探す。

**API観測の限界:** WinEventはWM_NCCREATE途中のhistorical状態を保存しない。
CBT/CALLWNDPROCはその前後を補うが、すべてのShowWindow呼出をinterceptしない。
MINMAXのeffective_cmd_showはOS通知のlow-word SW_で、callerのrequested nCmdShowそのものとは限らない。
WM_SHOWWINDOWのwParamはboolで、[一部show操作では通知されない](https://learn.microsoft.com/en-us/windows/win32/winmsg/wm-showwindow)。
正確な最初のShowWindow引数がログから決められなければ未確定とし、その呼出サイト計装を
次の限定scopeとして検討する。process限定FOREGROUNDは別processへのforeground移動を通知しない。
この診断だけでDWMがどのsurfaceをどの録画frameに表示したかを断定しない。

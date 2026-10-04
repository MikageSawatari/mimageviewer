# §1.327 起動時の白いウィンドウの調査

2026-10-04 / `next-startup-flicker`、調査対象 `a0aea7fe2`。
実装担当によるソース調査と、別 context の GPT-6.1 Sol / xhigh による独立設計レビュー。
**表示修正未実装・エージェントは製品未起動。利用者ログによる原因確認と修正設計は §7。**

利用者の「診断先行・修正は後」の判断を受け、opt-inのnative診断を実装した。
2026-10-04、調査文書commit `d82f94d76` の後続。表示／配置の挙動修正は含まない。
採取方法・観測限界は §6。診断commit `0000df439` の利用者採取結果を §7 に追記した。

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

## 7. 最大化起動の原因確認と修正設計 (2026-10-04、設計のみ)

### 7.1 利用者ログで確定したこと

正本は `target/flicker/user-run-20261004/normal-startup-windows.log` と
`maximized-startup-windows.log`。両方を `scripts/analyze_startup_windows.py` とraw JSONで
再確認した。STARTUPINFOは両方とも `dwFlags=0x1` / `wShowWindow=1`。
利用者は通常状態での2回の起動ではちらつかず、最大化して終了した次の起動で再現した。
対応録画は `C:\Users\mikag\Videos\NVIDIA\Desktop\Desktop 2026.10.04 - 21.17.31.01.mp4`。
今回エージェントはこの動画を再生していない。

時刻はcore診断入口からのentry_ms。以下は同期UI通知を優先して読む。

| 最大化ログの時刻 | main HWND `0x8c0d36` の操作／状態 |
| --- | --- |
| 10.148 ms | NativeOptions: maximized=true、normal inner=1280×800 logical、position=(50.667,50.667) |
| 17.893〜24.249 ms | CREATE / NCCREATE / CREATE、hidden normal |
| 27.483 / 30.223 / 101.005 ms | HCBT_MINMAX(3) → ACTIVATE、visible maximized → WM_SHOWWINDOW(0) |
| 111.650 / 114.030 / 137.448 ms | HCBT_MINMAX(9) → ACTIVATE、visible normal → WM_SHOWWINDOW(0) |
| 168.514 / 172.048 / 194.220 ms | HCBT_MINMAX(3) → ACTIVATE、visible maximized → WM_SHOWWINDOW(0) |
| 752.363 ms | first_paint.call_returned、まだhidden maximized |
| 752.421 / 756.590 ms | eframe.set_visible.begin → ACTIVATE、本来の表示 |
| 792.082 ms | 表示処理中にさらにHCBT_MINMAX(3)。802.244 msにset_visible.complete |

UI physical outer rectは、通常表示時 `[76,76,2018,1332]`、最大化時
`[-11,-11,3851,2099]`。normalログのmain `0x10e1afc` はmaximized=falseで、
895.906 msのpaint呼出復帰後、895.914 msのset_visible.beginから初めて表示し、
899.426 msにACTIVATE。一時的なshow/hideはない。両ログともfooter dropped=0。

**証明できた原因:** 同一main HWNDの、paint前の3回のnative表示／非表示／activation。
ソースの `winit::init` のhidden `set_maximized(true)`、egui-winitの生成後inner size / position
補正によるrestore、最後の再maximize、および `WindowFlags::apply_diff` のShowWindow→hideと
一致する。callerの全ShowWindow引数をinterceptした証明ではないが、別viewportを原因とする
必要はなく、最大化起動経路の不変条件違反は確認できた。

§1の全画面→通常矩形→再び全画面とfadeは、この往復と整合する。どの録画frameがどの
DWM animationに属するかまでは未確定。当初の「fresh profileでも再現」という報告を
取り消したり、通常起動がすべての環境で安全と一般化したりはしない。
worker WinEventのrectはthread DPI virtualizationと配送遅延の影響を受けるため、
UI physical rectと直接比較しない。

### 7.2 修正の契約と候補比較

foregroundで最大化起動するrootは、**正しい通常復元矩形をnative OSに保持し、hiddenのまま
最終最大化client寸法で最初のframeをpresentし、その後一度だけvisible commitする**。
表示してからサイズを直す、cloak／透明化で隠す、時間待ち、固定回数repaintは採用しない。
起動overlayも最初からそのclient寸法に描く。modal化で減らせる別操作の問題ではなく、
native生成・geometry・paint・visibilityの順序を一つの初期化ownerへ集約する。

| 候補 | 評価と採否 |
| --- | --- |
| (a) normal-hidden生成 → SetWindowPlacement(showCmd=SW_HIDE)でnormal placement → WS_MAXIMIZE設定 + SetWindowPos(非show) → commitでShowWindow(SW_SHOWMAXIMIZED) | 比較対象として残すが主案にはしない。SW_HIDE placementは通常復元矩形を渡せても、それだけでhidden maximizedを表さない。後付けstyleと手動最大化rectは、OSのmaximize内部状態／復元矩形／WM_GETMINMAXINFO／nonclient処理を再実装することになる。commitのShowWindowにもSTARTUPINFO問題が残る。native検証なしにIsZoomedやstyleだけを成功条件にしない |
| (b) CreateWindowEx(WS_MAXIMIZE、WS_VISIBLEなし) | **主案**。nativeの初期最大化を利用し、生成入力をnormal restore outer rectにする。WM_CREATEと生成後にnormal geometryを再適用しない。既定位置と実DPIを解決する方法、paint成功確認、表示APIの所有範囲まで下記の一組として実装する |
| (c) normal-hiddenで描画 → visible commit時にmaximize | 不採用。最初のframeがnormal client寸法なので、最大化表示に古いsurface／未描画部分が出る。先にshow-maxして次frameを待つのも契約違反。hiddenのままmaximizeして最終寸法で描き直すなら(b)等の準備が必要で、「延期」だけでは解決しない |

(b)の根拠はWindowsの
[native初期最大化とvisibilityの仕様](https://learn.microsoft.com/en-us/windows/win32/winmsg/window-features)。
現winitの `window.rs::init` のコメントにも、WM_CREATEでsizeを変えると復元sizeが変わるため
生成後にmaximizeしている理由がある。この注意を無視してcreate styleだけ変えるパッチは不可。

### 7.3 normal restore矩形とDPIを決めてからnative最大化する

appは現在の選択規則を維持する: saneな保存normal inner size（なければ1280×800）、最小
640×580、有効な保存位置、startup state設定、CLI `--window-size` 優先（normal、位置60,40）。
保存geometryの単位はUI倍率を除いた既存のlogical geometry。ui_scaleを二重に掛けず、
normal size、normal outer position、target monitor、native DPIを型で区別する。

最大化rootでは、winitのprivate初期化内に**配置解決専用のnormal-hidden HWND**を置く。
同じdecorations／exstyle／min-max制約／DPI-awarenessで、現在の保存位置またはCW_USEDEFAULT
を使い、normal状態で既存の論理→物理size／position補正を完了させる。全過程でWS_VISIBLEを
付けず、ShowWindow、activate、fullscreenを呼ばない。このHWNDは公開WindowId、App creator、
tray、single-instance listener、owner HWND、GPU surface、viewport mapへ渡さない。
生成通知もprivate owner内で処理し、公開window eventとして配送しない。
resolverはraw private HWNDとし、winit WindowData／event-loop登録、IME、OLE／drag-and-drop、
clipboardやrendererを初期化しない。nativeの既定配置処理と必要なframe／min-max／DPI処理だけを
持つ。最終rootだけを通常のwinit windowとして登録する。

ここで `GetDpiForWindow`、`GetClientRect`、`GetWindowRect`、`GetWindowPlacement` を取得する。
Windows既定のcascade位置は一度だけ解決し、保存monitorが無効なら現在のWindows既定位置へ
戻す。推測したprimary DPIで固定してから表示後に補正しない。
per-monitor-aware UI threadからの `GetDpiForMonitor` を新たな根拠にはしない
（[APIのDPI-awareness上の注意](https://learn.microsoft.com/en-us/windows/win32/api/shellscalingapi/nf-shellscalingapi-getdpiformonitor)）。
既存 `title_bar_on_some_monitor` のlogical値をphysical APIへ渡す点はmixed-DPIで前提確認が
必要。保存値を変換せずそのままnative screen rectと見なさず、既存settings形式を読む境界で
既存のlogical position適用の意味を維持しながら候補monitorとtitlebar到達可能性を検証する。
現settingsにはmonitor identityがなく、mixed-DPIで曖昧なlogical座標から元monitorを一意に
復元できるとは保証しない。今回に新たなmonitor affinity保存仕様を混ぜない。
別の保存先やdetached同期経路は作らない。

配置解決HWNDを破棄し、その確定した**physical screen outer rect**をx/y/w/hとして、
最終rootをWS_MAXIMIZE・非WS_VISIBLEで生成する。単なるoverride消費用dummyとは異なり、
目的はWindows既定配置・実DPI・normal geometryの解決。HWNDは二つ生成され得るが、
desktopへ出る／Appが所有するmainは最終rootの一つだけ。
先にAppやrendererを作ってからrootを差し替える方式にはしない。

最終rootのWM_NCCREATEではnative creation styleとwinit flagsを最初から一致させ、
空flagsから通常の `apply_diff` を呼んでShowWindowを発生させない。WM_CREATEではicons等は
適用するが、normal `request_inner_size` / `set_outer_position` とpost-create maximizeを
行わない。min/max制約も生成前のattributesから利用し、sizeを直すsetterを後から呼ばない。
初期最大化のnonclient境界とwork areaはWindowsのnative maximize／WM_GETMINMAXINFOに任せる。
モニター全体のrectを手動でclient寸法に置き換えない。
userdata登録前にも来るWM_GETMINMAXINFOはnative creationの入力を参照できる境界で扱い、
初期min/max制約を失わない。登録後のWindowStateだけを見て初期制約が効いたと仮定しない。

`WINDOWPLACEMENT.rcNormalPosition` は通常のtop-level非TOOLWINDOWでは**workspace座標**。
`GetWindowRect`／CreateWindowEx／SetWindowPosのscreen座標と混ぜない。
配置解決前後と最終rootのplacementを同じDPI-awareness contextで照合し、taskbarが上／左、
負座標monitorでもnormal restoreが等しいことを確認する。workspace→screenが必要な場合は
対象monitorのwork areaとmonitor originの差を含める。単にrcNormalPositionをCreateWindowExへ
渡す実装は不可。toolwindowは別の座標契約として扱う。
詳細は [WINDOWPLACEMENT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-windowplacement)。
`WPF_RESTORETOMAXIMIZED` はshowCmd=SW_SHOWMINIMIZED用で、hidden最大化の代用にはしない。

**native実証が必要:** 最終hidden rootのIsZoomed、実client size、monitor/DPI、normal placementが
全て成立すること。CreateWindowExの初期最大化が全restore bookkeepingをどう保持するかを
styleだけから断定しない。実証が失敗したら(a)のstyle注入へ黙ってfallbackせず、設計を戻す。

### 7.4 STARTUPINFOと表示APIの所有境界

主案のcommitは `ShowWindow(SW_SHOWMAXIMIZED)` ではなく、既にnative最大化済みのrootへ
`SetWindowPos(..., SWP_SHOWWINDOW | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER)` を一度だけ
行う。active=falseの既存意図にはSWP_NOACTIVATEを付け、foreground許可のOS制限を保つ。
rectもmaximize状態もこのcommitでは変えない。ShowWindowの戻り値（以前visibleだったか）を
成功判定に使わず、native実状態を同期してwinit requested visibilityと区別する。

STARTF_USESHOWWINDOWの最初のShowWindowでは指定nCmdShowが上書きされ得る。
今回のshow=1はnormal restoreなので、hidden WS_MAX rootへShowWindow(3)を初回実行する
案は安全でない。[ShowWindow公式仕様](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-showwindow)。
SetWindowPos／SetWindowPlacementがこの初回指定を「消費した」とは扱わない。
表示済みrootへ後からShowWindowして消費させる案も、normalへrestoreし得るので不可。
STARTUPINFOのコピーやPEBを書き換えず、dummy HWNDで消費せず、hidden時のSW_HIDEが
常に上書き例外だという観測依存の仮定も置かない。

したがって**root初回だけの差し替えでは足りない**。mIVが所有するWin32の明示表示操作を
同じAPI契約へ移すことを主案に含める:

- vendored winitのWin32共通owner: visibilityの実差分はSetWindowPosのSHOW/HIDE、
  min/max/restoreの実遷移はGetWindowPlacement→SetWindowPlacement。
  flags変更だけの冗長show/maximize/hideを発生させない。normal placement／restore-to-max
  flags／activation意図を保持し、WM_SIZEでOS状態を同期する。hiddenのgeometry準備からは
  showCmdで可視化しない。これは各setterの機械的なAPI置換ではなく、下記のproducer分類と
  対にした契約。初期geometry準備はprivate startup plan、runtime visibility commitはその
  viewportの既存ownerが表す。公開runtime要求を捨てたりhidden期間に延期したりはしない。
- appのnative owner: `presentation_observer::show_window` とtray、single-instance、
  DSP GUI、native video／HUDのcallerを同じvisibility／show-state helperへ移す。
  単純show/hideはSetWindowPos、normal/max/minへの状態変更はplacement操作とし、
  SW_SHOWNOACTIVATEを単にSW_SHOWへ置き換えない。SW_SHOWNAは現在のsize/stateの非activate
  表示、SW_SHOWNOACTIVATEはnormal restoreを伴い得る表示なので、両方を一律に
  SHOW|NOACTIVATEへ写像しない。tray最小化復帰を含めnative show-stateとactivationを別々に
  指定する。trayの既存placement restoreは維持する。
- mIV-controlledな直接ShowWindow／ShowWindowAsync（productionとtestを区別）、dependency内の
  owner、外部pluginがcore内で行うshowを実装前にinventoryする。root以外の最初のShowWindowへ
  問題を移しただけなら不合格。外部ownerの呼出を制御できない場合、この案の保証範囲外として
  設計担当へ戻す。プロセス再起動でSTARTUPINFOを正規化する別案は今回のbackend変更へ混ぜない。

これはSTARTUPINFOそのものを消費／変更する設計ではなく、制御下のshow-state変更を
その暗黙の初回ShowWindow規則に依存させない設計。
[SetWindowPlacement](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowplacement)
のshowCmdにも可視化作用があるため、hidden初期化とruntimeのAPI列を別々にテストする。
SetWindowPlacement後のforeground／no-activate／Z-orderが従来と同じかはnative検証対象。
**hidden runtimeの設計gate:** `src/ui_fullscreen.rs::send_detached_viewport_visible_commit` は
paint後にMaximized(true)→Visible(true)を送り、maximize自体のshowに依存する既存のcommit
producer。これは初期hidden geometry補正ではない。post-paintというowner境界とcommand順を
維持し、その列を明示commitとして扱う。hidden HWNDへのnormal↔max変更を全部延期する、
またはSetWindowPlacement(MAX/RESTORE)→hideで準備する方式はいずれも不可。
その他のhidden Maximized／InnerSize／OuterPosition producerを在庫確認し、準備なのか
明示showなのかをowner側で決める。独立した「hiddenのままmax状態を変更する」runtime要求が
見つかった場合、(b)のcreate-time方式だけでは解けないので、native policyを確定するまで
共有setterの変更に着手しない。タイマーやIsWindowVisibleだけの推定で分類しない。
STARTUPINFO show=0/2/3/7と既存の起動可視性意図は一度だけ明示的に決め、通常復元APIに
暗黙再適用しない。現在coreに独立した「start in tray」CLI／設定は見当たらないため、新設しない。
tray有効設定・close時格納・ショートカットからのminimized起動の現行挙動を検証してから
effective startup visibilityの対応表を固定する。show=1で保存最大化をnormalに変更しない。

### 7.5 first paintとvisibility commitのstate owner

vendored eframeのroot初期化を `PreparingGeometry → AwaitingFirstPresent(geometry) → Committed`
の一つの状態へ置き換える。geometryには最終HWND identity、physical client size、native DPI／
scale、geometry revisionを持つ。Appへ別の「最大化準備中」boolは追加しない。

1. 7.3が完了してから最終rootのApp creator、tray HWND捕捉、surface作成を行う。
   egui-winitの生成後builder適用はrootのnormal size／position／maximizeを再適用せず、
   geometry以外の属性は維持する。通常起動も最終normal寸法をshow前に確定する。
2. rootの初回geometry commandsとUI scaleをpaintより先に反映する。
   `pending_initial_size` / `created_maximized` の起動補正責務をこの準備へ移し、
   最大化解除後に旧deferred InnerSizeを送ってnormal restoreを上書きしない。
   他viewportのcommand処理順を一括変更しない。
3. vendored egui-wgpuのpaint結果に、実際のsurface取得／render submit／presentを行ったかと
   physical surface sizeを返す。既存のcall_returnedやscreenshot完了を成功の代用にしない。
   GPU完了のblocking wait／DwmFlushは不要。ここでいうpresentはframeのsubmit／presentで、
   録画上のDWM表示完了を保証する通知ではない。
4. 成功したpresentと現在のGetClientRect／geometry revisionが一致した時だけ7.4でcommitする。
   surface skip、lost、outdated、size=0、DPI／monitor変更は未commitのまま既存のsurface再構成・
   native resize／redraw経路へ戻す。通常のsize invalidationに従うので、timerや固定追加repaintを
   初期化成功の代用にしない。show前のroot geometry outputをpaint後に適用してからshowする
   旧順序は残さない。
5. commit前のactivation／tray open／second-instance要求も最終root ownerへ渡し、未描画窓を
   workerからshowさせない。既存IPC/open要求は処理し、foreground意図だけcommitへ集約する。
   commit後は通常のtray復帰・minimize restoreを即時処理する。起動用gateをruntimeへ延長しない。

### 7.6 変更を置く場所と回帰境界

| 場所 | 責務 |
| --- | --- |
| `vendor/winit` を実ファイルで追加しCargo patch（現0.30.13基準） | private配置解決、hidden native-max生成、初期WindowState、Win32 show-state API owner。registryを直接編集しない |
| `vendor/eframe/src/native/{wgpu_integration,epi_integration}.rs` | root専用startup planの引渡し、geometry-before-paint、present後の一度だけcommit、activation要求の収束 |
| eframeのroot create呼出／egui-winit builder適用境界 | 7.3で確定したgeometryを生成後補正で壊さない。公開builderからattributesを作り、root geometry適用を除いた残りを適用する。必要ならegui-winitも限定vendor patchとし、全viewportの補正を無条件削除しない |
| `vendor/egui-wgpu` | 成功presentと実surface寸法の結果通知（既にvendor化済み） |
| `src/lib.rs` / app startup geometryとnative show-state helper | 現設定／CLI優先順位、deferred補正の移管、tray／single-instance等のnative show caller統一。normal placementの保存形式は維持 |

appがHWNDを後から捕捉して直すだけでは27 msのshowに間に合わず、eframeだけでもwinitの
WM_NCCREATE／WM_CREATEでのshowを止められない。従って**winit patch + eframe調整 + app caller移管**
が必要。共有Windows API ownerの変更はroot専用のhidden-max生成より広いscopeであり、
独立レビューと全体gateを省略しない。

- non-max／CLI size: Windows既定・保存・60,40の選択と論理client寸法を維持。配置解決HWNDの
  二重生成はmax rootだけ。通常起動にmaximizeを経由させない。
- tray: winit requested VISIBLE=trueのままnative hide／placement復帰する現在の契約を維持。
  IsWindowVisible=falseだけをstartupの判定に使わない。placement lockを持ったままnative APIを
  呼ばない。quitのno-activate／cloakによるupdate再開も維持する。
- single instance: 同じdata dirへのsecond processは従来どおり転送／終了。既存rootのactivation、
  minimized／tray hiddenからの復帰を維持し、新HWNDを生成しない。
- detached／fullscreen: 空起動では従来どおり生成しない。private配置解決とroot startup planは
  secondaryへ使わず、既存のcontext／placement owner／明示Visibleを維持する。
  shared winitのAPI差し替えが両経路にも届くため、既存テストを弱めず、実装前にCLAUDEと
  独立reviewerの構造修正への合意、`detached-rework-plan.md` §11への変更範囲記録を行う。
- minimize／restore: normal rectとrestore-to-max flagsを保ち、最小化報告で保存max状態を失わない。
- DPI change／monitor removal: 初期化中はnative DPI／resizeイベントでgeometryを更新してから
  matching frameをcommit。commit後は現行WM_DPICHANGED等のownerに戻す。前回monitorがない時の
  fallbackをhiddenのまま完了させ、起動時以外のwindow移動規則は変更しない。

### 7.7 実装前のnative実証と修正後の検証計画

**この節は具体案であり、修正実装の完了／native feasibilityの実証ではない。**
独立Sol/xhighと、初回ShowWindow契約の未解決点に限定したAstra/mediumは、native hidden-max
生成をstyle注入より優先すること、配置解決HWNDの非公開性、first ShowWindowを消費済みと
みなせないことを確認した。全native ownerのAPI移管、restore bookkeeping、launcher指定の
互換性は実装着手前の設計検収項目。rootだけのSetWindowPosパッチへの合意ではない。

まずdisposable native harnessで、配置解決→hidden WS_MAX生成→最終client描画→showと、
その後のtray／min/max／secondary表示のAPI列を検証する。desktopへ表示するharnessも
interactive testなので利用者の明示承認が必要。今回はharness作成・実行も製品起動も行わない。
native生成のnormal restore rectが合わない、制御不能なfirst ShowWindowが残る、起動minimize
互換性が定義できない場合は実装前に設計担当へ戻す。失敗をdelayやShowWindow消費で隠さない。

自動テストとbuildの担当は後続の修正実装:

- 純粋なstartup plan: fresh／保存normal／保存max／Normal・Maximized・RememberLast／
  CLI size、最小値、invalid保存値、切断monitor、UI scaleとDPIを区別した選択。
- 座標: physical screenとworkspaceの往復、上／左taskbar、負座標、toolwindow、
  100%と150%／200%の2monitor。設定logical→target monitor physicalのround-trip。
- API列を記録するfake native owner: hidden準備にSHOW／ACTIVATEなし、MAXの生成属性、
  geometry補正でrestoreしない、first present失敗ではshowなし、matching成功でcommit一回。
  DPI／size変更時の古いframe、commit前activation、commit後tray hideをstartupと誤認しない。
- production ShowWindow呼出inventoryを確認。単純show／hideとmin/max/restore／no-activateを
  区別したAPI列、normal placementとrestore-to-max維持をテストする。
- normal + portable core `cargo check`、関連lib tests、`cargo fmt --check`、共有backendの
  `scripts/test-full.ps1`、成功後 `scripts/build-dev.ps1`（portableを付けない）。
  本文だけを変更する今回にはRust check／binary rebuildは不要。

修正後のdiagにnative create role（private resolver／final root）、確定normal placement、
target DPI／max client size、paint.presentedとsurface寸法、commit API／flagsを追加する。
foreground max起動の期待timeline:

```text
resolver CREATE -> normal geometry resolved -> DESTROY       visible=false、activationなし
main CREATE (WS_MAXIMIZE, !WS_VISIBLE) -> final geometry ready
main first_paint.presented (surface_px == final_client_px)    まだvisible=false
main visible_commit.begin (SetWindowPos / SWP_SHOWWINDOW)
main HCBT_ACTIVATE + EVENT_OBJECT_SHOW                       各1回、paint後
main visible_commit.complete                                max=true、rect不変
```

最終mainのHWND／generationだけで数え、paint前SHOW/HIDE/ACTIVATE、commit中のrestore／
re-maximize、表示後の旧deferred InnerSizeがゼロであることをraw UI記録とWinEventで確認する。
WM_SHOWWINDOWは操作により通知されないため、その件数だけを条件にしない。
foregroundを利用者が他へ切り替えた場合やno-activate／hidden／minimized開始はactivation数の
例外として分け、通常のforeground採取で「ACTIVATE一回」を確認する。

利用者確認は §6の `--diag-startup-windows` commandを使い、毎回ログを別名で保存する:

1. 新規isolated `--data-dir`でnormal起動。そのprofileでnormal rectを設定→最大化→終了→
   再起動→最大化解除。ちらつきゼロ、表示一回、元のnormal rect、最初から適切なoverlay寸法。
2. 通常profileのnormal保存／max保存と起動状態設定3種、`--window-size`。
   real profileは利用者だけが起動し、先にinstalled／tray resident版を終了する。
3. 2monitor（異なるDPI、負座標配置、上／左taskbar）それぞれで保存max起動とunmaximize、
   minimize→restore、tray→open、second instance activation、detached／fullscreenを確認。
4. 保存先monitorを切断して起動し、到達可能なmonitorへ配置してから一度だけ現れること、
   unmaximizeも到達可能なnormal rectに戻ること。起動準備中と表示後のDPI変更も別に確認する。
5. STARTUPINFO指定なし／show=1/2/3/0/7のlauncher・shortcut起動と、表示後に初めてtray／
   secondary／native editorを開く場合を別採取。後続ownerへ初回ShowWindow問題が移っていないこと。

画面録画とログを一緒に残し、main最初の表示がmax frameで、三つの過渡表示が消えたことを
利用者が確認してから修正を検収する。diagnostics commit `0000df439` 自体は未修正版のまま。

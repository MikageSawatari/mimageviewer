# §1.175 GPU snapshot 実行体の AV / 停滞調査

2026-10-09、Line F、`next-qa` / `d1c95344e`。製品の起動・実データ操作・コミットなし。

## 初回調査の結論 (5d7b30619)

実際の `ui_snapshot` の4-thread実行で second-chance `0xc0000005` を2件捕捉した。
例外の場所は NVIDIA OpenGL driver `nvoglv64.dll`、呼出元は wgpu-hal の
**WGL Instance の初期化 / context 生成**だった。
これは今回捕捉した AV の発生箇所を特定する証拠であり、driver 内部の原因や
過去の全 AV / 停滞を同じ原因と断定する証拠ではない。§1.175 は未解決。

生成競合・初期化中の bootstrap context 破棄・driver / hook の共有状態・先行する
メモリ破壊のどれが引き金かは未確定。製品、vendor、既存の snapshot harness、
期待画像、test-full の実行方式を変更せず、根因修正は実装していない。
retry や全テストの直列化で gate を成功へ置き換えていない。

## 読んだ実経路

`tests/ui_snapshot.rs` と lib 内の preferences / resume-meter 等は `egui_kittest::Harness`
を作る。`Harness::run()` などの geometry 検査だけでは GPU を初期化しない。
`snapshot()` / `render()` の最初の呼出しで `LazyRenderer` が、その Harness 専用の
`WgpuTestRenderer` / Instance / Adapter / Device / Queue / Renderer を生成する。
cached registry の `egui_kittest-0.33.3/src/renderer.rs:36,66`、`src/wgpu.rs:60,88` が正本。
共有 GPU Instance / Device static はない。snapshot 結果収集用の thread-local は GPU owner ではない。

- `vendor/egui-wgpu/src/setup.rs` の既定 mask は `PRIMARY | GL`。
  `WGPU_BACKEND` はこの mask を変更する。wgpu-core 27.0.3 の `src/instance.rs:94` は
  選択前に要求された各 backend を初期化する。**描画に DX12 を選んでも WGL が初期化される**。
- Windows の GL は `wgpu-hal-27.0.4/src/gles/wgl.rs`。
  `wgpu Device Class <address>` は process-global `LazyLock` で一度登録する HWND class / title
  (`271–327`)、renderer の共有ではない。各 Instance は別の隠し HWND / DC owner thread を作る
  (`329–432`)。class 名に使う static mutex は生成全体の排他ではない。
- owner thread は setup 結果を渡してから channel を待つ。JoinHandle は保持しない。
  `Inner` の field drop は device の sender を閉じた後に WGL context を削除し得る
  (`135–172`)。DC / window 終了の完了待ちがないため、直列テストも native teardown 同士を
  完全には直列化しない。これは source で確認した重なりの可能性であり、今回の AV の原因とは未証明。
- `RenderState` は選択した adapter に加え `available_adapters` 全体を保持する
  (`vendor/egui-wgpu/src/lib.rs:75,190`)。使わない GL adapter も残り得る。
  wgpu の Adapter / Device は core Global を保持するため、kittest の local Instance handle を
  drop した境界だけでは実 backend の解放を示さない。
- kittest の render / readback poll は10秒制限だが、readback の `receiver.recv()`
  (`src/texture_to_image.rs:59`)、WGL setup channel の `recv()`、native driver 呼出し全体は
  この制限の外。今回、自然発生した hang の stack は取得していない。

## 捕捉した AV の証拠

`target/F-1175-fix-evidence/previous/snapshot-default-4-run1.debug.log` の 286行以降に
second-chance AV、699行以降に fault thread の stack がある。exe は
`target/F-qa-cargo/debug/deps/ui_snapshot-092aabb1c40d88a8.exe`、隣に PDB。
テストは進行中で、main は libtest の結果待ち、終了時の事故ではない。

```text
"wgpu-hal WGL Instance Thread" (thread 18, OS tid 0x26b14)
nvoglv64.dll (read AV)
nvoglv64!DrvDescribePixelFormat
opengl32!wglChoosePixelFormat
gdi32full!ChoosePixelFormat
wgpu_hal::gles::wgl::setup_pixel_format
wgpu_hal::gles::wgl::create_instance_device
```

export symbol の `DllMain+offset` 表記は「DllMain 中の事故」を意味しない。
別の owner threads 13 / 17 も pixel format 初期化、thread 14 は setup 後の channel 待ち。
test thread 4 は WGL Instance **初期化中**の `DrvDeleteContext` にいる (359行以降)。
bootstrap context の削除と整合する。最終 renderer drop の競合を証明した stack ではない。
faulting instance は adapter 選択にも到達していない。

同じ exe の GL-only / 4-thread 対照でも再現した (`snapshot-gl-4-run1.debug.log` の
137 / 200行以降)。今回の fault は test thread 上の `wglCreateContext → DescribePixelFormat`。
default の owner-thread 側 `ChoosePixelFormat` と **同じ exception address**
`nvoglv64.dll + 0xA48AD7` だった。context 削除 / DestroyWindow と他 Instance 初期化の
重なりもある。GL対照の thread 7 は **最終 Inner / Instance teardown**、owner thread 9 は
DestroyWindow にいる。上の default run1 の bootstrap 削除とは区別する。
thread 7 / 9 が同じ Instance / DC を解放しているかは照合できておらず、
重なりだけで drop順序違反や因果関係とは扱わない。
full dump `snapshot-gl-4-run1.dmp` (1,741,678,072 bytes) を保存し、別の offline CDB でも
exception context / stack を読み出せた (`.dump-analysis.log`, `.pointer-analysis.log`)。
`!analyze -v` は `INVALID_POINTER_READ_c0000005_nvoglv64.dll!Unknown`、driver は
32.0.16.1088 (610.88)。fault instruction は `rax=0x000001727f6c0000` の `+0xA4` を読む。
読み先は `MEM_FREE / PAGE_NOACCESS`。free領域への read を確認したが、
どの owner / driver 処理が pointer を作ったか、use-after-free かどうかまでは未証明。

OBS `graphics-hook64.dll` / ShadowPlay `nvspcap64.dll` の load も確認した。
load だけでは原因とは扱わず、これらのアプリ・サービス・設定は操作していない。

## 再現ハーネスと採取方法

調査用の小さな独立 crate / runner を `target/F-1175-harness/` に置いた
(`Cargo.toml`, `Cargo.lock`, `src/main.rs`, `run.py`)。依存版と local egui / egui-wgpu / eframe
patch は本 worktree に合わせ、root Cargo.lock を seed に offline build。
製品 crate を使わず、利用者設定に触れない。恒久的な dependency patch は追加していない。

JSONL に PID、thread、worker、cycle、経過時間と次の begin / end を flush する:
Instance、adapter 列挙、選択、Device / pipeline (既存 startup observer)、render、drop。
default の CPU adapter 優先 selector を使い、64×64 image の readback を行う。
固定 worker + barrier と、libtest に近い「case ごとの新規 thread / barrier なし」を切り替えられる。
`process_end` は caller 側の完了であり、detached WGL owner の終了証明ではない。

CDB を非表示で起動し、初期 breakpoint で例外 filter を設定してから実行する。
first-chance AV は記録して `gn`、second-chance は全 thread stack / module / full dump を保存。
制限時間超過では、CDB が出した PID の process handle を開き、exact image path を照合して
`DebugBreakProcess` で stack / dump を採取する。prefix 一致での process 操作はない。
AV は exit 1、timeout は 2 を保ち、CDB の exit 0 をテスト成功として扱わない。

初回 prototype は `-g` と `-cf` を併用して最初の AV まで filter 設定が遅れた。
それでも実 second-chance と全 thread stack は採れたが、dump command 中の backslash escape により
dump 保存は失敗した。このログを消さず、`-g` を外して初期 breakpoint で設定し、dump path を
forward slash へ変更した。AV / hang を意図的に起こす陽性対照で、second-chance / watchdog の
stack と dump 保存をそれぞれ確認した。陽性対照は実 AV / hang の再現回数に含めない。
hang 対照の初回は exact-path 比較の引数添字を誤り、比較拒否となった。
同じ owned process の image を照合して終了用 break を送り、修正後の対照は自動採取まで完了。

WER LocalDumps を使う案も検討したが、既に入っている CDB で対象 exe だけを包めるため
今回は registry を変更していない。`test-full.ps1 -SuppressCrashDialogs` の `0x0002`
(`SEM_NOGPFAULTERRORBOX`) は WER 自体を抑止する。
dump を WER で採る別セッションではこの flag を併用せず、**target の test exe 名だけ**を対象にする。
全 process の既定 LocalDumps や通常 core の設定は変更しない。
[SetErrorMode の仕様](https://learn.microsoft.com/en-us/windows/win32/api/errhandlingapi/nf-errhandlingapi-seterrormode)、
[CDB の first / second-chance command](https://learn.microsoft.com/en-us/windows-hardware/drivers/debuggercmds/sx--sxd--sxe--sxi--sxn--sxr--sx---set-exceptions-)。

## 実行結果

実行結果の一覧は `target/F-1175-fix-evidence/previous/*.result.json` と各 `.debug.log` / `.jsonl`。
実行を後の成功で置換せず、失敗を含む全 attempt を保存する。
本 worktree の probe / build / suite は順番に動かした。他 worktree や OS / hook の GPU 使用を
排除した実験ではない。GL probe は RTX 4090 / NVIDIA 610.88、default probe の描画選択は
DX12 / Microsoft Basic Render Driver / 10.0.26100.9278。

| 条件 | 試行 / 件数 | 結果 |
| --- | --- | --- |
| 小 probe: default、Instance + 列挙、4 threads | 20 cycles × 4 = 80 | 完走、AV観測なし (初期 prototype の CDB 条件) |
| 小 probe: GL、Instance + 列挙、4 threads | 100 × 4 = 400 | 同上 |
| 小 probe: GL、Device + render + drop、4 threads | 100 × 4 = 400 | 同上 |
| 小 probe: default、caseごとの新規 thread、barrierなし、4 workers | 40 × 4 = 160 | 完走、AVなし、DX12 CPU adapter 選択 |
| actual ui_snapshot、default、4 threads | 8 process 起動 | run1 = AV、run2–8 = 各140/140成功。run1のdump保存不備も保持 |
| actual ui_snapshot、default、1 thread | 1 process 起動 | 140/140成功 (160.02秒)、直列の安全性を証明する回数ではない |
| actual ui_snapshot、DX12-only、4 threads | 1 process 起動 | 140/140成功、GL driver module loadなし |
| actual ui_snapshot、GL-only、4 threads | 1 process 起動 | AV、full dump保存 |
| actual ui_snapshot、GL-only、1 thread | 1 process 起動 | native AVなし、132成功 / 8 snapshot画像不一致、exit101。**suite成功ではない** |
| lib preferences module、default / GL-only、4 threads | 各1 process 起動 | 各64/64成功、他11,521 testsは対象外。全lib検証ではない |

GL serial の8失敗は `Image did not match snapshot`、Diff 値1–7。
期待画像や許容値を更新せず、`.diff.png` / `.new.png` を保存した。
DX12とGLの画像差を今回の AV や製品回帰と同一視しない。
陽性対照の2種以外には自然発生 hang を捕捉していない。
この小標本・異なる debugger 初期停止条件から故障率や backend / 直列化の安全性を推定しない。

## 初回調査時の構造修正候補と検証条件 (後述の backend 整合を採用)

候補は **テスト process 内で一度作る GPU setup owner** (Instance / Adapter / Device / Queue) と、
各 Harness 専用の新しい Renderer / texture state。backend / CPU adapter 優先の選定は維持し、
`WgpuSetup::Existing` から Harness ごとに renderer を作る。使用済み `RenderState.renderer` は
共有しない。egui の texture ID は Harness ごとの所有物なので、単一 Renderer の共用は不適切。
integration と lib の全 snapshot producer を同じ fixture owner に通す必要がある。
static を選ぶなら Rust static の destructor による終了順序は作らず、native shutdown 検証は
別の短命 probe に残す。別 test process / worktree 間の並行はこの owner では排除しない。

この単純化で per-Harness の native GPU setup / teardown の組合せを減らせる。
全テストの直列化、全プロセス共通の mutex、retry、製品からの GL 除去は選んでいない。
ただし共有化は現段階では fixture 設計候補であり、**証明済みの根因修正としては未実装**。

次の検証は、同一失敗条件の bounded 対照を先に行う:

1. unchanged default を CDB 下で反復し、失敗した setup / thread / 同時削除を採取する。
   既知の AV が出たら通常 gate の再試行をせず、stack / dump を解析する。
2. 同じ actual snapshot exe を DX12-only / GL-only / 1-thread で比較する。
   adapter / mask を記録し、成功回数だけで「直列なら安全」「GL が根因」としない。
3. 共有 owner の disposable 対照で GPU setup が本当に1回になり、各 Renderer が別であることを
   計装する。元 variant で故障が再現し、共有 variant で同じ負荷・複数 process 起動回数を
   十分に比較できて初めて contained fix の実装判断へ進む。
4. 実装する場合は期待画像を更新せず並列 ui_snapshot / lib snapshot を検証し、
   test-full の通常 thread 数でも gate を通す。自然発生 hang は別途 watchdog dump で
   初期化・readback・終了処理を分類する。AV 修正だけで hang も解決扱いしない。

## 製品 backend の確認と contained fix (2026-10-09、5d7b30619 の後続)

Windows 製品の root GUI は `src/lib.rs` で `DX12 | VULKAN` を明示する。
`WgpuSetupCreateNew` の `PRIMARY | GL` / `WGPU_BACKEND` 既定値をその後に上書きし、
eframe → Painter → `new_instance()` はその descriptor を維持する。
root selector は DX12 adapter を優先し、なければ Vulkan を選ぶ。
これは列挙結果の優先順であり、DX12 の device 作成失敗後に Vulkan で retry する意味ではない。
`NativeOptions` の renderer は eframe の `wgpu` feature により Wgpu。
製品の root GUI が GL を要求する経路はない。

動画は root の backend 選択とは独立する。現在の native video presenter は自身の
D3D11 device / DirectComposition swapchain と、共有 D3D11 texture / fence を使う
(`render_core.rs` の `D3D11CreateDevice`、`OpenSharedResource1`、`OpenSharedFence`)。
HUD は `overlay_gpu.rs` の別 Instance で **DX12 のみ**を要求し、CompositionVisual surface を
D3D12 composition swapchain へ接続する。現在の frame 経路に root wgpu への D3D12 texture
import / GL interop はない。`src/lib.rs` の「root が Vulkan なら動画 CPU readback」という
旧コメントは現行の無条件 `GpuVideoDevice::new` と一致しないため訂正した。
独立開発ツール `gpu_lanczos_spike` (DX12)、`vst3-host-tester` (Glow) は製品実行経路ではない。

### 修正する所有境界と、masking でない理由

破れていた invariant は「Windows の UI 回帰テストも製品の backend 集合で初期化する」。
旧 test setup は製品が要求しない GL まで初期化し、描画 adapter の選択前に WGL の
hidden HWND / DC / context owner を並列生成していた。捕捉した2件の AV はまさにこの
追加された初期化経路にあり、画像描画を DX12 に選んでも実行される。
テスト setup の要求 mask を製品と一致させる修正は、この証明済みの経路の不一致を
作られる境界で除く。assertion や失敗の捕捉を弱めたり、製品で使う backend を除外したりしない。
NVIDIA driver 内部の invalid pointer の生成者まで特定したという主張ではない。
自然発生した hang と過去の未採取 AV 全部の根因も、これだけでは確定しない。

- `src/wgpu_backend_policy.rs` に元の root mask を切り出す。製品の値・selector・実行動作は同じ。
- `tests/support/ui_harness.rs` の共通 builder を integration と cfg(test) の lib で使う。
  Windows は同じ policy を要求し、`WGPU_BACKEND=gl` にも製品と同じ上書きを適用する。
  非 Windows は従来の kittest setup を維持する。
- 全163箇所の kittest builder をこの入口へ接続する。lib 内の別用途の同名 `Harness`
  (`similar_book_query` の純ロジック fixture) は対象外。
- kittest の CPU adapter 優先、predictable rendering、lazy GPU 初期化は維持する。
  **adapter 優先は製品 root と同一ではない**が、要求する backend 集合は同一。
  Instance / Device / Renderer / texture state は各 Harness が独立所有し、並列実行を維持する。
  共有 GPU static、process-global mutex、sleep、retry、`--test-threads=1` は追加しない。
- 初回の「GPU owner 共有」は、backend の不一致を直すだけで対象経路を除けるため採用しない。
  vendor、入力・UI機能、test-full の thread 数、期待 PNG、比較許容値も変更しない。

### GL-only の8画像失敗の確認

同じ旧 exe は DX12-only / 4 threads で既存期待画像 **140/140** と一致し、
GL-only / 1 thread では132成功・以下8件のみ画像不一致 (exit101) だった。
旧 default の probe は `DX12 / Microsoft Basic Render Driver` (CPU) を選択している。
従って、この PC の現在の default と期待画像は DX12 描画と整合する。
全 PNG の過去の生成環境を逆算できる証拠ではないが、GL に合わせて期待画像を更新すべき差ではない。

| 画像 | kittest Diff |
| --- | --- |
| audio_thumbnail_indicator_modes_dark | 1 |
| audio_thumbnail_indicator_modes_light | 7 |
| collection_placeholder_light | 1 |
| cut_item_appearance_dark | 7 |
| cut_item_appearance_light | 7 |
| details_icons_dark | 4 |
| details_name_colors_dark | 7 |
| metadata_panel_comfyui_provenance_dark | 1 |

既存期待画像を保ったまま、修正後にも実選択 adapter と全画像一致を検査する。
GL 専用の driver stress は製品の UI 回帰 gate とは別の調査であり、今回そこを安全にしたとは言わない。

修正後の CDB log にも `opengl32.dll` / `nvoglv64.dll` の load はある。
DLL のリンク・load と wgpu の GL backend 初期化は区別する。除いたのは要求 mask の GL と、
それが作る wgpu-hal WGL owner 経路であり、GL 関連 DLL の未ロードを合格条件にはしない。
実際の要求 mask、列挙 backend の回帰検査、選択 adapter の記録で確認する。

### 後続検証と証跡整理

結果は `target/F-1175-fix-evidence/` に保存する。初回の小さな log / JSON は `previous/` に移した。
旧 `target/F-1175-evidence` の1,891,088,877 bytes (約1.76 GiB、主に full dump) の削除に先立ち、
上記の exception address、全 thread の主要 stack、driver 版、MEM_FREE 読み先と限界を本記録へ残した。
絶対パスが worktree の target 配下で、祖先・全子要素に reparse point がないことを確認した
(`deletion-preflight.json`)。ただし自動承認レビューが削除を `blocked by policy` として拒否し、
フォルダーは未削除。削除可能な環境では次の利用者要求済み操作だけが残る:

```powershell
Remove-Item -LiteralPath 'C:\home\mimageviewer-nextqa\target\F-1175-evidence' -Recurse -Force
```

registry / WER の system-wide 設定は今回も変更しない。

### 修正後の検証 (既存期待画像のまま)

同じ `test-script` feature の lib / ui_snapshot 実行体を使った。stress は旧 AV を捕捉した
**default 8起動 + GL指定1起動、各4 threads** の9起動をそのまま比較する。再試行による
成功の選別ではなく、全9 attempt を保存した。既存140件に setup 回帰2件を加えて各142件。

| 条件 | 結果 | 証跡 (`target/F-1175-fix-evidence/`) |
| --- | --- | --- |
| GL環境変数下の setup 回帰 (lib、4 threads) | 2/2成功、実選択 DX12 / Microsoft Basic Render Driver / CPU / 10.0.26100.9278 | `setup-gl-override.result.json` / `.debug.log` |
| actual ui_snapshot、default、4 threads | 8起動とも142/142成功、AV・timeoutなし | `snapshot-fixed-4-run{1..8}.*` |
| actual ui_snapshot、GL環境変数、4 threads | 142/142成功、AV・timeoutなし。GL要求へ戻らない | `snapshot-fixed-gl-env-4-run1.*` |
| 全 ui_snapshot、libtest既定thread数 | 142/142成功、45.15秒、AV・timeoutなし | `snapshot-full-default.*` |
| 全 lib 初回、CDB / libtest既定thread数 | 必須RAW不足5件と下記assertion1件を記録。fixture前提不成立として606秒でexact imageを照合したowned test PIDだけ終了 (125)。**未完了 / 不合格** | `lib-full-default.*` / `lib-initial-failures.json` / `lib-full-default-aborted.json` |
| 初回で落ちた collection epoch ケースの単独分類 | 1/1成功、0.34秒。全体の失敗を置換しない | `collection-epoch-focused1.*` |
| RAW準備後の全 lib、test-script付き / debuggerなし / 既定thread数 | 11,534成功・1失敗・52 ignored、exit101、1265.82秒。AVによる異常終了ではない | `lib-full-seeded.log` / `.result.json` |
| 上記で落ちた診断double-clickの単独分類 | 0成功・1失敗、exit101、0.35秒。単独でも再現 | `widget-double-click-focused1.*` |
| 全 lib、通常feature / debuggerなし / 既定thread数 | **11,417成功・52 ignored、native exit0、1111.61秒**。全11,469件、filterなし | `lib-full-normal.log` / `.result.json` / `lib-normal-native-exit.json` |
| normal / portable core check | 両方exit0、24.258 / 25.134秒 | `checks.json` / `check-*.log` |
| cargo fmt --all -- --check | exit0 | `fmt-check.log` |

初回の全 lib で落ちた `prepare_result_from_a_previous_thumbnail_source_epoch_is_not_installed`
は、変更していない App fixture → collection polling を使い、kittest builder / backend policy を
呼ばない。stale結果の拒否後の `RequestNeeded` は同じ poll 内の再要求へ進み、
read lease の50msを超えると `Snapshot` に移るため、特定の一時状態だけを許す assertion は
時間依存。また synthetic install の `page_edits: None` は epoch比較より前に拒否されるので、
thumbnail epoch 専用の回帰にはなっていない。失敗時の実状態は未採取のため、
時間依存を実原因と確定せず、**既存テストの時間依存疑い**として引き継ぐ。
このcollection状態 / ownership を今回のbackend修正に混ぜない。

RAW前提は manifest 27ファイルのサイズ・SHA-256を確認して、既存 worktree の
`C:\home\mimageviewer\vendor\raw-samples` から当該worktreeへ実ファイルをコピーした。
初回終了後に準備し、進行中のテストのfixtureは差し替えていない。
全 lib の通常検証は、その後に debuggerなし / libtest既定thread数で別に実行する。
子test processだけに `SEM_NOGPFAULTERRORBOX` を継承させ、親のmodeを復元する。
これは既存 `test-full.ps1 -SuppressCrashDialogs` と同じ抑止範囲で、終了値 / assertion は維持する。
tempは当該worktreeの `target/F-1175-fix-evidence/temp`。通常プロファイルのアプリは起動しない。

追加featureの全libが落ちたのは、既存の
`test_script::tests::rating_folder_back_key_pointer_reveals_and_double_clicks_actual_large_grid_without_focus`
(`src/test_script.rs:6039`)。selected=310の確認を通過した後、期待した `AddressBarNav::Direct`
が返らず失敗した。単独でも同じ assertion が再現し、過負荷時限定とは言えない。
この未変更のテストは `Context::run` に明示timeのfake frameを渡して `app.render_grid` を呼び、
kittest / wgpu setup を呼ばない。今回のGPU設定修正と混ぜず、別件として失敗を保持する。
実動作 / multipass / synthetic click のどの層の不備かは未確定で、期待値の緩和やretryはしない。
要求された通常featureの全libは、別実行体で既定thread数のまま検証し成功した。
追加featureの失敗をこの成功で置換しない。

通常全libの使い捨てrunnerは、起動時に読み込んだregex Matchのslice誤りにより、
native完了後の集計で例外になった。読み取り専用の別observerがexact imageを照合して
保持したprocess handleでnative exit0を採取済みであり、runner例外を製品失敗とは扱わない。
集計は全 `--list` 件数11,469と最終libtest summaryを照合して修正し、テスト本体は再試行しない。
追加feature側もpassed数の推測を廃し、passed / failed / ignored / measuredの合計で
完了判定を修正した。exit101 / failure はそのまま保持している。

期待PNG251枚は開始前のSHA-256 manifestと最後にも全一致し、比較許容値の変更なし。
`git diff --check` はexit0。独立Sol/xhighレビューは生成経路・cfg・独立所有・製品mask維持を
静的に確認して ACCEPT、指摘されたlib用の文書例も共通builderへ直した。
新規UI文字列なし、glyph lintは対象外。テストtoolingと等価なpolicy切出しのため、
実機確認用アプリbuild / 起動は行わない。コミットなし。

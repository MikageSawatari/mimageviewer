# §1.241 ORT 初期化失敗・VC++ ランタイム同梱の修正計画

状態: **実装・focused / full / static gate・独立 completion review・unsigned release verification build / 三形態 Sandbox / Defender 有効host / TensorRT実GPU 完了。署名済み最終配布と§1.243限定再検証はrelease handoff待ち** (2026-09-15)。

調査の正本は
[msstore-startup-hang-vcrt-investigation-20260915.md](msstore-startup-hang-vcrt-investigation-20260915.md)、
作業項目は [next-release-backlog.md](next-release-backlog.md) §1.241。本文は、その観測を
現行 source・upstream・配布経路へ照合し、製品修正の所有境界と検収条件を定める。

## 1. 観測、期待する不変条件、破れている境界

観測済みの失敗は、VC++ ランタイムが無い Windows で
`onnxruntime.dll` のロードが win32error 126 になった後、`ort` 2.0.0-rc.12 の
`G_ORT_LIB` 初期化中に `Error::new_internal` が ORT API を要求し、同じ
`OnceLock` へ再入して停止することにある。初回 `App::update` が
`ensure_ai_runtime()` を同期実行するため、失敗が UI へ戻らず起動画面ごと応答不能になる。

修正後は次を同時に満たす。

1. 動的 ORT のロード、export 解決、C API 版検証のどこで失敗しても、ORT API を使わない
   型付きエラーとして一度だけ確定し、同じプロセスの全 consumer へ有限時間で返す。
2. 正常な配布物は VC++ ランタイムが未導入の Windows でも DirectML ORT を使える。
   AI 機能、TensorRT、編集用パックの被写体分離を削らない。
3. GUI プロセスでの埋め込み DLL 展開と ORT 初期化は App が開始する一つの worker が所有し、
   UI thread は完了を待たない。Ready / Failed は terminal であり、同じプロセス中に再試行しない。
4. TensorRT infer worker と builder は共通 ORT 初期化の Err をその場で親へ返す。
   infer worker は最初の JSON 応答を `ok=false` にして終了し、builder は stderr と非 0 exit を返す。
   明示された初期化失敗を 45 秒 timeout や一時的通信失敗に分類せず、silent retry しない。
5. 配布に入る全 PE を列挙して直接依存を検査し、必要な app-local DLL、版、由来、署名を
   配布 build の gate で確認する。本体 exe の一つだけを調べて合格にしない。

## 2. upstream と版の判断

### 2.1 採用する修正

pykeio/ort の issue [#560](https://github.com/pykeio/ort/issues/560) は rc.12 の
`load-dynamic` 失敗時に同じ `OnceLock` 再入を報告している。upstream commit
[`17ed727`](https://github.com/pykeio/ort/commit/17ed727) は、動的ロードの失敗を
通常の `ort::Error` から `LoadDynamicError` (`Dlopen` / `MissingApi` / `BadVersion`) へ分離し、
エラー生成時に ORT API を呼ばない。この変更は rc.13 の release note でも
「Don't deadlock when `load-dynamic` fails」と明記されている。

現行へは **ort 2.0.0-rc.12 の source をローカル crate として保持し、workspace の
`[patch.crates-io]` から全 consumer へ適用して、`17ed727` の 3 file 差分だけを backport** する。
これにより本体だけでなく `local_adjust_lab` も同じ修正版を使う。`PATCHES.md` に元 commit、変更 file、
除去条件を記録し、元 crate の MIT / Apache-2.0 license を同梱する。アプリ側の
事前 `LoadLibrary` だけでは `OrtGetApiBase` 欠落や C API 版不一致が rc.12 の再入経路へ残るため、
部分的な回避としては採らない。

### 2.2 rc.13 全体更新を分離する理由

[rc.13 release](https://github.com/pykeio/ort/releases/tag/v2.0.0-rc.13) は ONNX Runtime 1.28 へ
4 世代進み、配布 CUDA binary は CUDA 13 のみになった。mIV の DirectML DLL と TensorRT pack は
どちらも ORT 1.24.2、`ort` rc.12 の C API 24 に揃えてある。rc.13 への更新は DirectML NuGet、
TensorRT pack、CUDA/TensorRT 依存、全 model の画質・性能をまとめて再検収する別 dependency migration
であり、Store 起動 P0 の修正へ混ぜない。

## 3. VC++ ランタイムの由来と固定条件

Microsoft は Visual Studio の `VC\Redist` 以下を、正規ライセンスを持つ利用者がプログラムと共に
未変更で再配布することを認めている。app-local 配置も利用できるが Windows Update による自動更新を
受けないため、mIV の依存更新・公開 gate で更新責任を持つ。

今回の source は次の正規 redist directory とする。System32 の copy は採らない。

`C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Redist\MSVC\14.50.35710\x64\Microsoft.VC145.CRT\`

同 directory の servicing metadata は `14.50.35719`。必要な最小 closure は次の 4 file である。

| file | file version | SHA-256 | Authenticode |
| --- | --- | --- | --- |
| `msvcp140.dll` | 14.50.35719.0 | `DEF46AA6A8F72F27BAFAC0C43334419486A4D1DCDB6C479A8EF7034B3E1FA4CB` | Microsoft / Valid |
| `msvcp140_1.dll` | 14.50.35719.0 | `2DD670F874562FBDCA5B022DF1943D70A57BA91FDE559280E3A1DAEBE4DB2380` | Microsoft / Valid |
| `vcruntime140.dll` | 14.50.35719.0 | `184146852727A9DB4EEA06178716BEC3CDBB1015C911F6B0F915B184AD7775B2` | Microsoft / Valid |
| `vcruntime140_1.dll` | 14.50.35719.0 | `E6BFB3662AB4B1969A73441DBE35C96D51441B6BFF8CF1FE7430BD5B246CA605` | Microsoft / Valid |

DirectML / TensorRT の ORT PE は linker 14.44 で、上の runtime 14.50 は Microsoft の
v14 binary compatibility 条件「runtime は build tools と同じか新しい版」を満たす。
`dumpbin /dependents` で DirectML `onnxruntime.dll` は 4 file 全部、
`onnxruntime_providers_shared.dll` は `vcruntime140.dll`、TensorRT pack の ORT provider 4 file も
この集合の部分集合だけを要求する。4 file 自身の非 OS 依存もこの集合内で閉じる。

実装では未変更 binary と、machine-readable な provenance manifest
（source product / redist directory family / file version / SHA-256 / signer subject）を
`vendor/vcrt/` に置く。build は manifest と現物の exact hash、x64、Microsoft Authenticode Valid、
全 file の同一 version、最低 `14.44` を検査する。更新時は正規 `VC\Redist` source から取り直し、
manifest とこの表を同時更新する。

参考となる Microsoft の一次資料:

- [Visual Studio Redistribution](https://learn.microsoft.com/en-us/visualstudio/releases/2026/redistribution)
- [Latest supported Visual C++ Redistributable](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist)
- [Deployment in Microsoft C++](https://learn.microsoft.com/en-us/cpp/windows/deployment-in-visual-cpp)

## 4. entry point と lifecycle の棚卸し

### 4.1 同じプロセスの ORT consumer

| entry / consumer | 現状 | 修正後の owner |
| --- | --- | --- |
| GUI 初回 frame `App::ensure_ai_runtime` | UI thread で extract + `ort::init_from` | App の `AiRuntimeInitOwner` が一度だけ worker を開始。frame は poll のみ |
| fullscreen AI / final composite | `ensure_ai_runtime` 後に `Option` を読む | Ready だけ開始。毎 frame の producer は Initializing 中に再 poll し、Failed だけを unavailable とする |
| erase / book bake / external materializer | snapshot 時点の `Option` が `None` なら拡散 fallback または worker 内の別 init | 一回入力と保存済み snapshot を worker へ渡し、worker が共通 owner の terminal を待つ。Failed のときだけ既存 diffusion fallback / error 契約へ入る |
| subject segmentation | click 時点で `Option` が `None` なら操作を捨てる | source / layer generation と一回入力を pending owner に固定し、segmentation worker が init terminal 後に実行する。既存 generation gate で stale completion を捨てる |
| video upscale queue | `Option` が `None` なら queued task を IO failure にする | Initializing 中は Queued を維持して後続 poll で再試行し、Failed だけを terminal failure にする |
| Remote AI bridge | `Empty/Initializing/Ready` を独自所有し、remote worker が同期 init することがある | App と同じ owner clone。Ready / Failed を Condvar で受け、別 constructor を持たない |
| materializer worker | snapshot が無ければ worker 内で同期 init し、失敗後も要求ごとに再試行 | App / Remote と同じ `Arc<AiRuntimeInitOwner>` だけを受け取る。cancel-aware に terminal を待ち、Ready の同じ `Arc<AiRuntime>` または Failed の同じ `Arc<AiError>` を消費する。別 constructor は持たない |
| bench / probe bin | CLI thread で同期 init | 同期 API を維持。共通 patched boundary から finite Err |

`AiRuntimeInitOwner` の状態は `Dormant | Initializing | Ready(Arc<AiRuntime>) | Failed(Arc<AiError>)`
の一つだけとする。現行 `ai_runtime: Option<_>` と pending bool を併置して状態を推定せず、この owner を
App / Remote / background consumer の唯一の正本にする。App 構築完了時、
remote bridge を設置した後に `Dormant -> Initializing` を一度だけ行い、worker completion は同じ owner へ
Ready / Failed を publish して egui repaint と Condvar を wake する。

GUI process 内で `AiRuntime::new_with_backend` を呼べるのは、この owner が開始した一つの初期化 worker
だけとする。`PageEditContext` / `MaterializeSession` も runtime の任意 snapshot や独自
`worker_ai_runtime` cache を持たず、同じ owner clone を運ぶ。materializer は Condvar を短い bounded
wait で待つたびに request の cancel / generation を確認し、cancel なら ORT 初期化の完了を待たずに
その materialize request を終了する。Ready なら owner 内の同じ runtime を使い、Failed なら既存の
diffusion fallback または error 契約へ一度だけ入る。Remote の待機も session disconnect / request cancel
を同じ方式で観測し、process-global 初期化そのものは cancel しない。

通常 close、tray hide、Remote acquire/release は owner を cancel/restart しない。ORT は process-global で
中断不能だからである。App exit は worker を join して UI 終了を遅らせず、process termination に任せる。
worker が先に完了すれば保持した owner だけへ terminal result を publish し、破棄済み App を触らない。
Failed は process lifetime 中 terminal とし、別 consumer の後着で再初期化しない。thread spawn 失敗は
呼び出し thread が直ちに Failed を publish する。worker body は unwind guard / `catch_unwind` で囲み、
panic でも Failed を一度 publish して全 Condvar waiter を wake する。したがって Initializing のまま
producer が失われる状態を許さない。App drop / exit は owner を drop するだけで待たず、Remote / materializer
request は各自の disconnect / cancel で先に終了でき、後から起きる completion は生きている `Arc` owner
だけを terminal にする。

### 4.2 TensorRT の別プロセス

| entry | 成功 | 失敗 | parent 側 |
| --- | --- | --- | --- |
| `--tensorrt-infer-worker` | 最初に `WorkerResp::ok()` | `WorkerResp` の `RuntimeInit` discriminator と元の `LoadDynamicError` / fallback reason を flush して非 0 exit | `WorkerHandle::spawn` は `WorkerStartError::ChildRejected` を直ちに返す。文字列内容にかかわらず retry 対象外 |
| `--tensorrt-build <model>` | load / compile / warmup | stderr に model と ORT error、非 0 exit | pack builder は child status をその場で失敗として確定 |

両 entry は core exe の別 process なので独立した `ORT_INIT` を持つ。ローカル backport は同じ binary に
コンパイルされ、DirectML と TensorRT pack のどちらの `ort::init_from` にも効く。pack 未導入、破損、
provider 不足、C API 版不一致の既存 fallback / notice は維持する。

親子 protocol は `WorkerResp` の失敗へ `WorkerFailureKind` (`RuntimeInit` / `CommandRejected`) を追加し、
handshake の親側は `WorkerStartError` (`Spawn` / `Transport` / `Timeout` / `Protocol` /
`ChildRejected { kind, detail }`) へ変換する。その型を `TrtWorkerPool::start`、App の spawn task、
`WorkerNotice` まで失わず運ぶ。自動 retry は `Spawn` / `Transport` / `Timeout` のうち既存上限内で許す
分類だけに限定し、`RuntimeInit` / `CommandRejected` / `Protocol` は deterministic terminal とする。
`is_transient_spawn_failure(detail)` と HRESULT / 日本語部分文字列判定は削除する。TensorRT runtime が
DirectML へ fallback した結果を child が拒否するときは、要求 backend、active backend、
`BackendSelection::fallback_reason`（`LoadDynamicError` の Dlopen / MissingApi / BadVersion と詳細を含む）を
typed failure の detail に残し、親の log / notice まで同じ原因を届ける。

## 5. 配布 layout

### 5.1 launcher / installer / 単体 exe

`crates/launcher` が 4 CRT file を core、remote、FFmpeg と同じ `ASSETS` に含め、
`%APPDATA%\mimageviewer\runtime\<version>\` へ hash 検証付きで展開してから core を spawn する。
この directory には `mimageviewer-core.exe` と `mimageviewer-remote.exe` があり、TensorRT infer / builder
も core 自身の mode なので、全実行形態で exe directory から同じ CRT を解決できる。installer は
launcher 一つを配布する現行構造を変えない。

`build-release.ps1` は launcher build 前に 4 file の provenance gate を通し、
`target/release/` にも 4 file を stage する。Microsoft 署名は保持し、mIV の署名処理には渡さない。

### 5.2 portable / diagnostic portable / dev-runtime

`build-portable.ps1` は package root へ 4 file を copy する。そこには renamed core
`mimageviewer.exe`、remote、Susie worker が同居する。x86 Susie は静的 CRT であり x64 CRT を読まないが、
新しい依存は増えない。portable の Microsoft PE 除外規則へ 4 file を明記し、署名を上書きしない。

`build-dev.ps1` も `target/dev-runtime/` へ同じ 4 file を stage し、ユーザーへ渡す normal-profile
検証 binary の layout を配布時と揃える。`prepare-portable-smoke.ps1` は portable build 経由で自動的に含む。

TensorRT pack / editing pack 自体へ CRT を重複同梱しない。TensorRT は同じ core child process の
exe directory から解決し、editing pack は DLL / exe を含まない。

## 6. 全 PE 依存 gate

新しい read-only script を配布 build の package 完成後に実行し、少なくとも次を対象にする。

1. release: launcher、core、remote、埋め込み元となる PDFium、DirectML ORT、FFmpeg、Susie、VST host。
2. portable: package root 以下の全 `*.exe` / `*.dll`。既知名の固定列挙だけでなく filesystem 列挙を使う。
3. TensorRT: `dist/trt-pack-v3` が存在するときは全 `*.dll`。pack 作成 / upload gate からは必須 mode で呼ぶ。
4. 4 CRT file 自身。

各 PE で machine、linker version、direct imports を `dumpbin` から収集する。machine の期待値は artifact
別に定義し、main / remote / vendor x64 PE は x64、Susie worker / plugin と Inno Setup wrapper の既存 x86 PE は
x86 を許容する。
VC v14 import は
`msvcp140.dll` / `msvcp140_1.dll` / `vcruntime140.dll` / `vcruntime140_1.dll` の allowlist だけを認め、
要求集合が配布 app-local 集合の部分集合であることを検査する。未知の `msvcp*` / `vcruntime*` /
`concrt*` が現れたら fail し、runtime source を再評価する。CRT と Microsoft ORT は Authenticode Valid、
manifest hash exact を必須にする。結果は file ごとの imports と hash を含む JSON / text report として残す。

`build-dist.ps1` はこの gate を installer / portable 作成後かつ完了表示前に必須実行する。
TRT pack の `setup` / `build` / `upload` にも同じ script の pack mode を接続する。
これにより CLAUDE.md Phase 3 step 12 の「release exe 一つに CRT import が無いこと」だけを見る検査を置き換える。

## 7. 実装 file scope

想定する所有 file は次のとおり。独立レビュー合意までは本節の製品 file を編集しない。

- local dependency: `crates/ort-patched/**`, `Cargo.toml`, `Cargo.lock`
- runtime owner: `src/ai/runtime.rs`, `src/app.rs`, `src/materializer.rs`, `src/remote_ipc/session.rs`,
  `src/remote_ipc/ui.rs` と対応 test
- worker error: `src/ai/trt_worker_runtime.rs`, `src/ai/tensorrt_builder.rs`,
  `src/ai/trt_worker_pool.rs`, `src/ui_dialogs/trt_worker_notice.rs` の必要最小箇所と test
- CRT assets / build: `vendor/vcrt/**`, `crates/launcher/build.rs`,
  `crates/launcher/src/main.rs`, `scripts/build-release.ps1`, `scripts/build-portable.ps1`,
  `scripts/build-dev.ps1`, `scripts/build-dist.ps1`, TensorRT pack script、PE gate script / test
- docs: 本文、調査記録、`docs/README.md`、`CLAUDE.md` の ONNX / Distribution / release step 12、
  `docs/release-operations.md`、必要なら portable / TensorRT distribution runbook

変更履歴、版番号、version highlights は公開担当の scope なので触らない。

## 8. 自動検証

実装中は次の順で行う。GUI は起動しない。

1. patched ort の subprocess 回帰: 存在しない DLL と export を持たない file を渡し、
   5 秒以内に `LoadDynamicError` が返って process が終了する。hang 時は test が child を kill して fail する。
2. `AiRuntimeInitOwner` state test: UI start が待たない、constructor 1 回、Ready / Failed terminal、
   remote / materializer waiter が両 terminal で起きる、poll / 後着 consumer で再試行しない、spawn failure /
   worker panic が Failed を publish して全 waiter を wake する、App drop 後 completion が安全。Remote disconnect /
   materializer cancel は owner terminal 前でも request だけを終了し、owner の後続 completion を壊さない。
3. TensorRT test: infer initialization Err は最初の `ok=false` response、builder Err は非 0、
   明示 init error は detail に `0x8007045A` / `DLL 初期化` が含まれても `RuntimeInit` 型により retry 対象外。
   Spawn / Transport / Timeout だけが定義済み上限で retry され、fallback reason が child response から
   `WorkerNotice` まで保たれる。親が timeout deadline を待たず受理する。
4. launcher / build script test: 4 file が exact hash で runtime directory へ展開され、corrupt / old copy は置換、
   current copy は再書込しない。portable / dev-runtime の expected list に 4 file がある。
5. PE gate focused run: DirectML vendor、TensorRT pack、実在 package fixture の全 PE を数え、
   4 file closure、x64、版、署名、hash が通る。1 file 欠落 / hash 不一致 / unknown CRT import の parser fixture は fail。
6. `cargo check -p mimageviewer --bin mimageviewer-core`、対象 unit / integration、`cargo fmt --check`、
   `scripts/test-full.ps1`、static checks。
7. resident process が無い同一 source checkpoint で `build-dev.ps1 -PreserveRuntime` と release / portable build。
   resident があれば停止せず build を保留し、既存 process と保留理由を記録する。

## 9. ClaudeCode へ渡す隔離実機シナリオ

修正 source と署名・hash・build report が揃ってから、公開担当へ次を渡す。現段階では起動しない。ClaudeCode向けの短い依頼は
[section241-claudecode-verification-request.md](section241-claudecode-verification-request.md)、詳細コマンド・artifact ledger・合否と終了条件は
[section241-sandbox-handoff.md](section241-sandbox-handoff.md) を正本とする。

1. VC++ redist 未導入の Windows Sandbox で単体 launcher、silent installer、portable の 3 形態を
   それぞれ fresh data で初回 / 2 回目起動する。起動画面から進む、`Responding=True`、初回設定 UI、
   `[AI] Runtime initialized`、CRT 4 file の exe 隣配置を確認する。
2. disposable build で ORT DLL を欠落 / 破損させ、5 秒以内に `[AI] Runtime init failed` が出ても
   UI と通常画像 / 動画 / PDF / ZIP が使えることを確認する。正規 package の self-repair と混同しない
   fault-injection 手順を成果物に添える。
3. VC++ runtime のある NVIDIA 開発機で TensorRT pack install、infer worker、全配布 engine、builder を確認する。
   成功は従来どおり、意図的な ORT/provider 欠落は明示理由が直ちに返り 45 秒 timeout×2 にならないことを測る。
4. editing pack の BiRefNet 被写体分離、DirectML upscale / denoise / erase、Remote AI を一巡し、
   worker 化で要求が失われず画質・fallback・notice が変わらないことを確認する。
5. final installer / portable の全 PE gate report と Microsoft / mIV の署名を照合してから Store 再申請へ進む。

## 10. 実装開始 gate

- Codex design lead と独立 reviewer が、local backport、terminal init owner、Remote / materializer の
  lifecycle、app-local CRT の更新責任、全 PE gate を合意する。
- rc.13 全体更新、TensorRT pack への CRT 重複同梱、VC redist installer の global install は今回扱わない。
- 実装中に UI の一回入力を保持できない consumer、別 ORT constructor、配布 PE の未棚卸しが見つかった場合は、
  scope を狭める workaround を入れず本文へ戻して設計を更新する。

2026-09-15、独立 reviewer は上記の境界へ合意した。特に、GUI process の
App / Remote / materializer が一つの `Arc<AiRuntimeInitOwner>` だけを消費すること、spawn 失敗 / panic を
Failed terminal として全 waiter へ通知すること、TensorRT の retry 判断を文字列でなく typed failure で行うこと、
artifact ごとの machine 条件を含む全 PE gate を実装開始条件として確認した。

## 11. 実装 checkpoint (2026-09-15)

実装は本文の境界に沿って次まで完了した。

- workspace 全体の `ort` を `crates/ort-patched` へ向け、rc.12 に upstream `17ed727` を exact
  backport した。欠落 DLL と export 不足は隔離 subprocess で5秒以内に `LoadDynamicError` を返す。
- App / Remote / materializer と本・消しゴム・被写体分離・動画 upscale の consumer を一つの
  `Arc<AiRuntimeInitOwner>` へ統一した。constructor は worker 上で一度だけ、Ready / Failed は terminal、
  waiter は cancel-aware で、spawn失敗・panicもFailedとwakeへ収束する。final AI は Initializing 中だけ
  provisional cacheを保持し、terminal後にjob無しならReadyへのenqueueまたはFailedのcomplete昇格へ再進入する。
- TensorRT infer の child rejection を `WorkerFailureKind`、親起動失敗を `WorkerStartFailureKind` として
  protocolからnoticeまで保持した。自動再試行は ProcessSpawn / Transport / Timeout の型だけで判断し、
  RuntimeInit / CommandRejected / Protocol / ParentSetup は即時 terminal とする。DirectML fallbackの元原因も保持する。
- Microsoft VC/Redist servicing 14.50.35719.0 のx64 4本を `vendor/vcrt` に固定した。manifestは同一版・
  最低14.44・SHA-256・Microsoft署名を正本とし、launcherは4本の実体hashを毎launch検査する。
  same-length破損と正しい古sidecarの組合せもembedded bytesへ修復し、正しいcopyは再書込しない。
- launcher runtime、release、portable、dev-runtimeへ4本をstageし、`check-vcrt-pe-dependencies.ps1` を
  build-release / portable / dist と TensorRT setup / build / uploadへ接続した。gateは全input hash、artifact別
  machine、全PE direct import closure、CRT exact copy/version/signature、Microsoft ORT signatureを検査する。

focusedでは patched ORT のmissing DLL / missing export subprocess、共有init owner 6件、materializer 29件、
TensorRT worker 11件、launcher 9件、Remote owner/cancel、final AI terminal再評価を通した。追加の
`test-build-dev-safety.ps1`、`test-release-build-safety.ps1`、PE parser SelfTestもexit 0である。
`cargo check -p mimageviewer --bin mimageviewer-core` と、workspace外のconsumerを確認する
`cargo check -p local_adjust_lab` も成功し、後者がlocal patched `ort`をcompileすることを確認した。

最終 `RUST_TEST_THREADS=1 scripts/test-full.ps1 -SuppressCrashDialogs` は本体 **8539 passed / 0 failed /
45 ignored**、UI snapshot **52 / 52**、vendor egui / egui-wgpu / eframe **25 / 9 / 15**、
`[test-full] PASS`、exit 0でprocess error modeを`0x00008001`へ復元した。ログは
`target/section241-final-20260915/test-full-rerun.{stdout.log,stderr.log,exit.txt}`、SHA-256は順に
`D77AB9637752FD691B4BFB9F5E920A5A3A8414882A398282D6795ECE92384357`、
`D4D01B84AFD504064942605475E90F8CA73B04B9D3BABA39E668511408307F5D`、
`13BF7B3039C63BF5A50491FA3CFD8EB4E699D1BA1436315AEF9CBE5711530354`である。初回fullで旧回帰
`ensure_final_composite_never_leaves_stuck_incomplete_entry`がlive producerへ新しい`Initializing`を数えず
1件失敗したため、製品条件を緩めずtest invariantを`Initializing | exact pending | exact cache`へ更新した。
focusedで再現修正を確認後、上記final fullを一度通した。初回失敗ログも同directoryへ保持する。

同じ製品sourceで`cargo fmt --all -- --check`、UI glyph、viewer-context audit、`git diff --check`はexit 0。
PE gateはcanonical CRT 4本、DirectML ORT 2 PE、既存Inno installer x86 PEで成功し、JSON reportは
`target/section241-final-20260915/vcrt-{canonical,ort,installer}.json`、SHA-256は順に
`CF9D372C28097FED66361980E6EAEDFC66BD0464CAA045D465B5EC1869E0FF12`、
`23E02B5AC2305D01A7F3904D3128F670BB647926357CC9DFC6253BCB0DBCCF0A`、
`970E80E995BB3D04921C790FFC7BCC7511F1BEA1CCEF360276C1D911FD4DC4DD`である。

release-only launcher / embedded asset確認のため`build-release.ps1 -PreserveRuntime`を実行したが、
`target/dev-runtime`のcore 8 process、Remote 1 processと、その子Susie 3 process / VST host 1 processが
residentだった。安全preflightが停止せず拒否したためverification buildは保留した。記録は
`target/section241-final-20260915/build-release.{stdout.log,stderr.log,exit.txt}`、SHA-256は順に
`E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855`、
`D5227F9FDEB03F5CE5BA9C8264125D87B6DD2FDD608D453C2342F04FC92A78BB`、
`F1B2F662800122BED0FF255693DF89C4487FBDCF453D3524A42D4EC20C3D9C04`である。アプリ起動、GUI操作、
resident停止、通常profile / real dataへのアクセスは行っていない。

その後、利用者が全window / tray終了を明示した。read-only `Get-Process` とscript自身のpreflightで対象process不在を
確認し、同じsource commit `dd96be07394d5276db6a94ebd8a65db7bf32fd44`から
`build-release.ps1 -PreserveRuntime`を再実行した。core / remote / launcherの構築、release runtime
**runtime 4 / PE 3**、embedded **runtime 4 / PE 11** はexit 0で、extracted VST3 bridge cacheも保持した。
launcher / core / remoteのSHA-256は順に`3966900559C6F2EF70A3498ECF014FE2AB19D3C786A2000EB86A57EB16B9B4E9`、
`08E41A4B1D8CEE7865DFFB43986AD12EAD09B619397182EFA86AD9D3E8FC0578`、
`1B313FF1D89347EAE2F073AFA3C1AB9EC07331825568F96F820A226C3863237A`である。PE reportは
`target/vcrt-pe-reports/release-{runtime,embedded}.json`、SHA-256は順に
`549A38429F0207C2BECCAB3C68D0944FE1E403F341587E68E8ACC7CBA623DB65`、
`3A1D9FF87F82CB069D3EF021D8EBC55201D269DFDC62C835C8600FB0FCCB046F`である。全9 artifactのexact ledgerは
`target/section241-release-build-20260915/BUILD-MANIFEST.sha256`、そのSHA-256は
`E2721702AFF3B2C590B11D9DDCAD57AEBE2DB8D89484C808A1EB4481228BF395`である。この確認buildの3 exeは未署名である。
その後ClaudeCodeは同じsourceのunsigned single-exe / installer / portableを別々のfresh Sandboxで検証し、
三形態のfirst / second launch、launcher CRT self-repair、portable loose ORT failureを完了した。Defender有効hostの
portable first / second launch、実GPUのDirectML / subject / TensorRT / Remoteも完了している。詳細は
[`RESULTS.md`](../target/section241-release-verification-20260915/RESULTS.md)。TensorRTの壊れたpackで画像ごとに
lazy startを繰り返す残件は§1.243としてtyped lifecycle ownerへ修正し、自動gateとunsigned release buildまで完了した。
署名済み最終三形態、版番号・公開・Store再申請と、§1.243の壊れたpack / manual restart / 正常pack限定再検証を
release handoffとして残す。

static logは`target/section241-final-20260915/static.{stdout.log,stderr.log,exit.txt}`、SHA-256は順に
`6148BA18192E65F9E4930484CD138C545F149673DC3370518A90C9F574EABAE1`、
`9DC53AC11C947712C9E2FAB67EAD7E8C2C00A43C8DF7786D4437F30720B3D905`、
`13BF7B3039C63BF5A50491FA3CFD8EB4E699D1BA1436315AEF9CBE5711530354`である。

独立 Sol / xhigh reviewerは設計と実装を別担当として検収し、load failure再入、共有ownerのterminal / cancel、
final AI provisional再評価、TensorRT typed retry、launcher self-repair、CRT provenance / PE closure、
全consumer / package接続についてblocking / should-fixなしと判定した。reviewer所有のrating planと着手前からの
EOL-only 2 fileを除くowned 130 fileは、同じsource freezeのexact SHA-256を
`target/section241-final-20260915/OWNED-MANIFEST.sha256`へ保存し、completion handoffでartifact hashを固定する。

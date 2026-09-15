# §1.241 ORT / VC Runtime 修正 — ClaudeCode 検証・配布引継ぎ

更新日: 2026-09-15
対象 source commit: `dd96be07394d5276db6a94ebd8a65db7bf32fd44` (`Fix ORT startup initialization and bundle VC runtime`)

## 1. 引継ぎの目的と現在地

この文書は、§1.241 の source 修正を ClaudeCode Opus が独立に確認し、最終署名済み配布物を Windows Sandbox と実 NVIDIA 環境で検証するための実行手順である。公開版番号の決定、changelog / version highlights、署名、installer / portable / single-exe の最終生成、GitHub Release、Web、Store 再申請は release lead である ClaudeCode の担当とする。Sandbox 確認の依頼や成功は、版番号確定、公開、Store 再申請の許可を意味しない。

依頼内容の短い正本は [§1.241 ClaudeCodeへの検証依頼](section241-claudecode-verification-request.md) である。この文書は、その依頼を実行するための詳細コマンド、artifact ledger、合否・終了条件を補う。

source commit は実装・自動検証・独立 Sol / xhigh completion review まで完了している。review は ORT load failure の再入防止、process 共通 init owner、request cancel、final AI provisional 再評価、TensorRT の型付き retry、launcher の exact self-repair、VC Runtime provenance と全 PE closure を確認し、blocking / should-fix なしと判定した。既存の green gate を引継ぎだけの理由で再実行しない。

`scripts/build-release.ps1 -PreserveRuntime` による unsigned development verification build はexit 0で完了した。この build は source / launcher / embedded assets の確認用で、最終配布物ではない。署名済み installer / portable / single-exe は未生成。ClaudeCodeは同じsourceのunsigned single-exe / installer / portableを別々のfresh Sandboxで検証し、Defender有効hostのportable起動と実GPUのDirectML / subject / TensorRT / Remoteも完了した。

### 2026-09-15 ClaudeCodeからの検証結果受領

ClaudeCodeがサブPCのWindows SandboxをComputer Useで操作し、6シナリオをPASSと報告した。
対象は本書のsource commitとlauncher SHA `3966900559C6F2EF70A3498ECF014FE2AB19D3C786A2000EB86A57EB16B9B4E9`。
Codexは報告を受領し、ローカル成果物のSHA一致を確認した。画面操作の観測者はClaudeCodeである。

| 実施シナリオ | 報告結果 |
| --- | --- |
| VC++未導入で初回起動 | PASS。展開を含む10.8秒でAI初期化成功、画像閲覧・ページ移動可能 |
| 2回目起動 | PASS。1.6秒でAI初期化成功、場所復元・ZIP閲覧可能 |
| CRT同一長破損からlauncher自己修復 | PASS。正本hashへ復旧して起動 |
| 修復後の再起動 | PASS。正常CRTの更新時刻不変 |
| CRT4本なしでcore直接起動 | PASS。1.101秒で初期化失敗1回、UI・画像閲覧が応答 |
| ORT同一長破損からlauncher起動 | PASS。0.959秒で初期化失敗1回、UIが応答 |

詳細証跡: [RESULTS.md](../target/section241-release-verification-20260915/RESULTS.md)。
環境はx64 Windows build 26100、System32に対象CRT4本なし。SandboxのDefenderは無効だったと報告されており、有効環境の結果には一般化しない。
正常版の埋込ORTはhash sidecarを信頼するため、同一長破損が自己修復されない既存挙動も観測された。
今回の停止防止は機能しており、この自己修復の拡張は別課題として扱う。ここでは製品変更を追加しない。

unsigned installer / portableの初回・再起動、portable loose ORT欠落・破損、DirectML / 編集用pack / Remote AI /
TensorRT infer・builderの実GPU回帰、Defender有効hostのportable起動は完了した。残りは署名済み最終三形態、
Defender有効かつVC++ runtime無しの組合せ、§1.243修正後の壊れたTensorRT packでの複数画像・通知close・
manual restart・正常pack attachの限定再検証である。担当が変わっただけで成功済みscenarioをやり直す必要はない。
成果物・source・試験条件が変わった場合は影響範囲に応じて再確認する。

Phase 5 Remote と rating sort の製品実装には着手していない。

## 2. 固定済み source evidence

source freeze は次である。

- commit: `dd96be07394d5276db6a94ebd8a65db7bf32fd44`
- owned 130-path manifest: `target/section241-final-20260915/OWNED-MANIFEST.sha256`
- manifest SHA-256: `7E8168BD2C30E3088062CAFCB31CC0E67C0A078E8FC23732F68C324EFF090BB3`
- final full: main **8539 passed / 0 failed / 45 ignored**、snapshot **52 / 52**、vendor egui / egui-wgpu / eframe **25 / 9 / 15**、exit 0
- final full logs: `target/section241-final-20260915/test-full-rerun.{stdout.log,stderr.log,exit.txt}`
- static gate: fmt、UI glyph、viewer-context audit、`git diff --check` が exit 0
- canonical / ORT / installer PE gate reports: `target/section241-final-20260915/vcrt-{canonical,ort,installer}.json`

作業ツリーにはこの commit 外の rating plan と既知 EOL-only 2 file が残り得る。これらは §1.241 の source evidence に含めず、release lead は owned manifest と commit から対象差分を判定する。

VC Runtime の正本は `vendor/vcrt/provenance.json` である。Microsoft VC/Redist x64 14.50.35719.0 の次の4本を同一版で使用する。

| file | SHA-256 |
|---|---|
| `msvcp140.dll` | `DEF46AA6A8F72F27BAFAC0C43334419486A4D1DCDB6C479A8EF7034B3E1FA4CB` |
| `msvcp140_1.dll` | `2DD670F874562FBDCA5B022DF1943D70A57BA91FDE559280E3A1DAEBE4DB2380` |
| `vcruntime140.dll` | `184146852727A9DB4EEA06178716BEC3CDBB1015C911F6B0F915B184AD7775B2` |
| `vcruntime140_1.dll` | `E6BFB3662AB4B1969A73441DBE35C96D51441B6BFF8CF1FE7430BD5B246CA605` |

4本は Microsoft Authenticode `Valid`、signer に `Microsoft Corporation` を含む必要がある。Microsoft DLL を mImageViewer の証明書で再署名しない。

## 3. development release build

実行コマンドは次の一つである。`*>&1` は付けない。

```powershell
.\scripts\build-release.ps1 -PreserveRuntime
```

`-PreserveRuntime` は mImageViewer の process が1つでも残っていれば停止せず拒否し、既存 runtime cache も削除しない。実行直前の read-only `Get-Process` preflightでは対象processが返らず、build script自身も`no running mimageviewer process found`を確認して開始した。利用者の終了連絡だけをprocess不在の根拠にはしていない。

見込みは warm build で約 8～15分、VST3 bridge の再構成や再リンクが入る場合はそれ以上である。前提は Rust / MSVC toolchain、CMake または再利用可能な VST3 bridge、libclang、`vendor` runtime 一式である。署名はこの development build の前提に含めない。

完了時に次を固定する。

- `target/release/mimageviewer.exe`
- `target/release/mimageviewer-core.exe`
- `target/release/mimageviewer-remote.exe`
- `target/release/{msvcp140.dll,msvcp140_1.dll,vcruntime140.dll,vcruntime140_1.dll}`
- `target/vcrt-pe-reports/release-runtime.json`
- `target/vcrt-pe-reports/release-embedded.json`

### 3.1 build result

2026-09-15 17:37～17:49 JSTに実行し、exit 0で完了した。coreはrelease profileを11分03秒、remoteは26.93秒、launcherは17.19秒で構築した。VST3 bridge build、事前canonical runtime gate、release runtime gate **runtime 4 / PE 3**、embedded gate **runtime 4 / PE 11** が成功した。`-PreserveRuntime`によりextracted VST3 bridge cacheを変更していない。アプリやGUIは起動していない。

| artifact | size (bytes) | SHA-256 |
|---|---:|---|
| `target/release/mimageviewer.exe` | 445,797,888 | `3966900559C6F2EF70A3498ECF014FE2AB19D3C786A2000EB86A57EB16B9B4E9` |
| `target/release/mimageviewer-core.exe` | 315,661,312 | `08E41A4B1D8CEE7865DFFB43986AD12EAD09B619397182EFA86AD9D3E8FC0578` |
| `target/release/mimageviewer-remote.exe` | 11,148,800 | `1B313FF1D89347EAE2F073AFA3C1AB9EC07331825568F96F820A226C3863237A` |
| `target/release/msvcp140.dll` | 553,552 | `DEF46AA6A8F72F27BAFAC0C43334419486A4D1DCDB6C479A8EF7034B3E1FA4CB` |
| `target/release/msvcp140_1.dll` | 35,488 | `2DD670F874562FBDCA5B022DF1943D70A57BA91FDE559280E3A1DAEBE4DB2380` |
| `target/release/vcruntime140.dll` | 123,472 | `184146852727A9DB4EEA06178716BEC3CDBB1015C911F6B0F915B184AD7775B2` |
| `target/release/vcruntime140_1.dll` | 47,264 | `E6BFB3662AB4B1969A73441DBE35C96D51441B6BFF8CF1FE7430BD5B246CA605` |
| `target/vcrt-pe-reports/release-runtime.json` | 5,409 | `549A38429F0207C2BECCAB3C68D0944FE1E403F341587E68E8ACC7CBA623DB65` |
| `target/vcrt-pe-reports/release-embedded.json` | 12,772 | `3A1D9FF87F82CB069D3EF021D8EBC55201D269DFDC62C835C8600FB0FCCB046F` |

exact manifestは`target/section241-release-build-20260915/BUILD-MANIFEST.sha256`、そのSHA-256は`E2721702AFF3B2C590B11D9DDCAD57AEBE2DB8D89484C808A1EB4481228BF395`である。runtime reportは3 exeをx64として確認した。development buildの3 exeは未署名であり、PE reportのsignatureはnullである。CRT4本とDirectML ORT 2 PEはMicrosoft署名`Valid`を確認した。ClaudeCodeはこのbuildをsource確認には使えるが、Sandboxの最終三形態matrixには§5で新たに作る署名済みdistributionだけを使う。

## 4. ClaudeCode が先に行う source / release 判断

ClaudeCode は最終配布を始める前に次だけを確認する。全 test suite の再実行ではなく、source commit と release 境界が配布要件を満たすかの独立判断である。

1. workspace の `ort` が `crates/ort-patched` の rc.12 backportへ統一され、upstream `17ed727` と同じ load-dynamic failure separation になっていること。
2. GUI process 内の `AiRuntime` constructor が共有 `Arc<AiRuntimeInitOwner>` の worker に一つだけあり、App / Remote / materializer が Ready / Failed terminal を消費すること。
3. TensorRT failure が protocol から notice まで型を保持し、automatic retry が ProcessSpawn / Transport / Timeout だけで、RuntimeInit / CommandRejected / Protocol / ParentSetup は直ちに terminal になること。
4. launcher が versioned runtime の4 CRT実体を毎回 SHA-256 検査し、sidecarだけを真実にしないこと。
5. final distribution build が4 CRTを全配布 exe の隣へ置き、全 PE closure、file version、Microsoft署名、artifact別 machine を gateすること。

これらに製品修正が必要と判断した場合は、release metadata 作業と混ぜず development workflow へ戻す。変更後は commit と evidence を更新し、以下の matrix には更新後の最終署名物だけを使う。

## 5. 最終署名物の生成と三形態 Sandbox matrix

最終版番号・release notesを確定した後、ClaudeCode が repository root から次を実行する。コード署名は既定で有効なので、公開候補では `-NoSign` を付けない。source が `dd96be073` から変わった場合は既存 full evidence の流用条件を再評価する。

```powershell
.\scripts\build-dist.ps1 -PreserveRuntime
```

想定所要は署名・installer・portableを含め約20～40分以上である。SimplySign、Inno Setup、network、timestamp service、十分な disk 容量が必要になる。最終成果物は次である。

- single exe: `target/release/mimageviewer.exe`
- installer: `installer/Output/mImageViewer_setup.exe`
- portable: `dist/mImageViewer_portable_v<version>.zip`
- distribution PE reports: `target/vcrt-pe-reports/dist-runtime-portable.json`、`target/vcrt-pe-reports/dist-installer.json`

Sandbox 実行は desktop / foreground を使う interactive verification である。ClaudeCode は開始前に、三つの fresh Sandbox session、約25～40分、foregroundを使用すること、使用データは disposable sample のみであることを利用者へ示し、明示承認を得る。今回の build 許可や引継ぎ依頼は Sandbox GUI 実行の承認ではない。

三形態は必ず別々の新規 Sandbox session で行う。前の session の `%APPDATA%\mimageviewer`、versioned runtime、設定、VC Runtime が次の形態へ混ざるのを防ぐためである。各 session では host の通常 profile や既存 portable installを共有せず、hostからread-only shareした署名済み成果物と disposable sampleだけを Sandbox内の書込み可能directoryへコピーする。

### 5.1 共通観測

各 launch の前に artifact SHA-256 と Authenticode statusを記録する。起動後は Task Managerまたは `Get-Process` で `Responding=True`、UI、次のlogを観測する。

- 通常 / installer: `%APPDATA%\mimageviewer\logs\mimageviewer.log`
- portable: 展開directoryの `data\logs\mimageviewer.log`
- 前回 launch: 同じdirectoryの `mimageviewer.log.prev`

main logger は次回起動時に現logを `.prev` へコピーしてからtruncateする。各 launch を閉じた直後にlogをrun別evidence directoryへcopyし、次のlaunchで上書きされないようにする。

clean packaged launch の合格条件は、起動から30秒以内に main/settings UIが表示されて応答し、logに AI runtime の初期化成功が残り、画像閲覧ができることである。AIを使わない閲覧だけが成功し、clean packageで`[AI] Runtime init failed`となる場合は不合格にする。

30秒までにUIが表示されない、または `Responding=False` が継続する場合は、その時点のprocess一覧、`mimageviewer.log` / `.prev`、Windows Event Log、artifact/runtime SHA-256を保存してそのscenarioを終了する。同じ壊れた状態を無制限に再起動しない。アプリを閉じるときはSandbox内の通常UI終了を使い、終了しない場合だけ証拠取得後にSandbox sessionごと破棄する。

### 5.2 single exe

1. fresh Sandboxに署名済み `target/release/mimageviewer.exe` とsample画像をcopyする。
2. Sandbox内に `%APPDATA%\mimageviewer` がないことを記録する。
3. Explorerから非管理者としてsingle exeを起動し、first launchの共通合格条件を確認する。
4. UIから終了し、`%APPDATA%\mimageviewer\runtime\<version>` のcore / remote / runtime DLLと4 CRTのSHA-256、署名、mtimeを記録する。
5. 同じSandbox / profileでsecond launchを行い、応答、AI初期化、画像閲覧を再確認する。
6. 正常な4 CRTのSHA-256とmtimeが維持され、不必要な再抽出がないことを確認する。

### 5.3 installer

1. 別のfresh Sandboxに署名済みinstallerとsample画像をcopyする。
2. `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART` でinstallし、自動起動はさせない。
3. installed shortcutまたはinstalled exeをExplorerから非管理者で起動する。
4. first launchの共通合格条件とversioned runtimeの4 CRTを確認して終了する。
5. installed entryからsecond launchし、同じ条件を確認する。
6. uninstaller、installed files、installer本体の署名を記録する。テスト後はSandbox sessionを破棄する。

### 5.4 portable

1. 別のfresh Sandboxでportable zipを `C:\miv-test\portable` のような書込み可能場所へ展開する。
2. 展開直後に `data` がなく、`%APPDATA%\mimageviewer` もないことを記録する。
3. `mimageviewer.exe` を起動し、first launchの共通合格条件を確認する。
4. `data\logs` と設定が展開directory内だけに作成され、`%APPDATA%\mimageviewer` が作成されないことを確認する。
5. second launchを行い、同じ条件を確認する。
6. exe隣の4 CRTがcanonical SHA-256 / Microsoft署名と一致することを確認する。

portableのCRTを壊してlauncher self-repairを試してはいけない。portableはloose CRTを含む配布形態で、single-exe launcherのembedded self-repair対象ではない。

## 6. single-exe launcher self-repair

self-repairは§5.2のclean first / second launchが通った後、同じ disposable Sandbox内で行う。署名済み配布物、hostの`vendor/vcrt`、hostの`target/release`、System32、実利用者の `%APPDATA%` は変更しない。

1. single exeと、そのcore / remote / child processがすべて終了したことを確認する。
2. `%APPDATA%\mimageviewer\runtime\<version>\vcruntime140.dll` のSHA-256、length、mtimeと、隣の`.sha256`内容を保存する。
3. Sandbox内のextracted copyだけを同一lengthのまま破損させる。正しい古sidecarは意図的に残す。

```powershell
$runtime = Join-Path $env:APPDATA 'mimageviewer\runtime\<version>'
$crt = Join-Path $runtime 'vcruntime140.dll'
$before = Get-Item -LiteralPath $crt
$beforeHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $crt).Hash
$bytes = [IO.File]::ReadAllBytes($crt)
$bytes[0] = $bytes[0] -bxor 0xFF
[IO.File]::WriteAllBytes($crt, $bytes)
$corruptHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $crt).Hash
```

4. `Length`が同じ、`$corruptHash`がcanonicalと異なる、`.sha256`が古いcanonicalのままであることを記録する。
5. 同じ署名済みsingle exeを起動する。launcherがcoreをloadする前にextracted CRTをembedded canonical bytesへatomic repairすることを確認する。
6. UI / AI初期化が共通合格条件を満たし、`vcruntime140.dll`のSHA-256が `184146...775B2` に戻ることを確認する。
7. 終了後にmtimeを記録し、さらに正常状態で一度起動する。正常copyとcurrent sidecarの組合せではmtimeが変わらず、再書込みされないことを確認する。timestamp resolutionを避けるため観測間に2秒以上置く。

破損したままcoreが開始した、repair後hashがcanonicalでない、正しいcopyが毎launch書き換わる、UIがhangする場合は不合格として証拠を保存し、そのSandboxを破棄する。

## 7. ORT load failure fault injection

このscenarioはlauncher self-repairと目的を分ける。self-repairは「配布launcherがCRTをcanonicalへ戻す」検証であり、ORT fault injectionは「DLL load失敗が再入deadlockせず有限時間のFailed terminalになる」検証である。同じSandbox stateで混ぜない。

別のfresh Sandboxでportable zipを展開し、そのコピーから loose `onnxruntime.dll` だけを別名へ退避する。4 CRTは変更しない。公開候補artifactそのものではなくdisposable diagnostic copyを使う。製品は `src/ai/runtime.rs` から明示的に `ort::init_from` を呼ぶため、`ORT_DYLIB_PATH` の上書きだけではこのfailure pathへ入らない。通常single-exe launcherは欠落したembedded ORTを自己修復するため、この試験には使わない。

```powershell
$portable = 'C:\miv-test\ort-failure'
Rename-Item -LiteralPath (Join-Path $portable 'onnxruntime.dll') -NewName 'onnxruntime.dll.disabled'
```

起動後5秒以内にlogへ `[AI] Runtime init failed` と load-dynamic由来の原因が1回だけ現れ、UI threadが応答し、AI以外の画像 / ZIP / PDF / video機能を操作できることを合格条件とする。同じprocess内でconstructor retryやlog stormが起きず、終了できることも確認する。AI成功はこのintentional failure scenarioの条件ではない。

5秒を超えてterminal errorが出ない、UIがhangする、同じinitが繰り返される場合は不合格として、10秒でprocess/log evidenceを保存してscenarioを終了する。fileを戻して同じstateを再利用せず、Sandboxごと破棄する。

## 8. TensorRT実GPU検証

Windows SandboxにはNVIDIA GPU / CUDA / TensorRT packの実条件がないため、Sandbox matrixとTensorRT検証を分離する。TensorRTは対応NVIDIA machineのdisposable Windows accountまたは明示承認されたisolated `%APPDATA%` rootで行い、利用者の既存 `%APPDATA%\mimageviewer\tensorrt` をrename、delete、上書きしない。

packの取得・作成・署名・uploadは `docs/tensorrt-pack-distribution.md` に従い、release leadであるClaudeCodeが担当する。networkと約10 GiBのdisk、対応driver / CUDAが必要である。公開uploadは別の公開承認を得る。

実GPU success scenarioでは、最終署名buildと正しいpackをisolated profileへinstallし、TensorRTを選択して実画像のupscaleを行う。次を合格条件とする。

- infer workerが一度でreadyになり、処理結果を返す。
- builder commandが対象modelのengineを生成し、二回目にcacheを再利用する。
- UIはworker / builder待ち中も応答する。
- failureも含むlogにprovider、model、worker response、fallback reasonが残る。

typed failureの自動回帰はsource gateで完了している。実機fault injectionも行う場合は正常packを直接壊さず、disposable profileへcopyしたpackのprovider依存DLLを別名にし、UI inferとbuilderを別々に一回だけ実行する。child RuntimeInit / CommandRejectedは元のload errorを保ったまま直ちにterminalまたはDirectML fallbackへ進み、45秒timeoutや同じdeterministic failureのsilent retryへ入らないことを確認する。15秒以内に親へterminal noticeが来なければ不合格として証拠を保存し、test profileを破棄する。pack validationでworker開始前に拒否された場合は「pack preflight failure」であり、child protocol fault injection成功とは数えない。

## 9. evidence layout

ClaudeCodeは各artifact / runを混ぜず、例えば次の構成で保存する。

```text
target/section241-release-verification-<date>/
  source-commit.txt
  artifact-hashes.sha256
  signatures.txt
  pe-reports/
  single-exe/run-1/
  single-exe/run-2/
  single-exe/self-repair/
  installer/run-1/
  installer/run-2/
  portable/run-1/
  portable/run-2/
  ort-failure/
  tensorrt-gpu/
```

各runには開始・終了時刻、Windows build、artifact path / SHA-256、process path / PID / Responding、data directory、`mimageviewer.log` / `.prev`、runtime4本のSHA-256 / version / Authenticode、操作結果を残す。Windows Event Logはhang / crash / loader failure時に添付する。hostの通常dataを含めず、sampleの個人情報も含めない。

## 10. 完了と公開の境界

§1.241 verification完了は次のすべてが揃った時点である。

1. ClaudeCodeのsource / release境界確認で未解決findingがない。
2. 最終署名済み三形態のfirst / second launchがfresh Sandboxで通る。
3. single-exe self-repairと独立ORT failure injectionが定義した有限時間で通る。
4. 対応NVIDIA machineでTensorRT success scenarioが通り、必要と判断したfailure scenarioの結果が分類される。
5. artifact / runtime hashes、署名、PE reports、logsがsource commitと最終版番号へ結び付く。

ここまでの検証成功はrelease candidateの技術gateである。版番号確定、署名鍵使用、GitHub Release、Web更新、Store package提出 / 再申請はClaudeCode release checklistの別段階で行い、それぞれ既存の利用者承認を守る。

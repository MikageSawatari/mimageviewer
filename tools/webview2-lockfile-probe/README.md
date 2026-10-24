# WebView2 lockfile probe

`docs/effetune-fork-plan.md` 第6版 §4・§5・§15.5・§15.6 の前提を実測する独立した試作。
製品・VST3 host・Cargo workspace には接続しない。移行の本実装は含まない。

## ClaudeCode が実行するコマンド

この worktree のルートから、サンドボックスの外の Windows PowerShell 5.1 で実行する。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\tools\webview2-lockfile-probe\run.ps1 -GracefulAttempts 5 -BlockedAttempts 3
```

ビルド後、4種類のシナリオを順に実行する。通常は数分程度だが、起動待ち・残存プロセス・
所有者照会の時間切れが続けば十数分以上になる。各起動の待ちは既定20秒、外側の watchdog はさらに5秒。
終了後の観測は既定30秒。`-StartupSeconds`、`-ObserveSeconds` で変更できる。
`-GracefulAttempts` は正常終了の反復回数（既定5、1〜20）。
`-BlockedAttempts` は b の各モードの反復回数（既定3、1〜20）。すべて指定回数を最後まで
実行し、FAIL / ERROR で早期終了しない。各回で新しい UDF を使う。
既にビルド済みなら `-SkipBuild` も使える。exe・Loader・ページはビルド記録の hash と照合する。

試作用の小窓を表示するが、前面化・キー入力・マウス入力は行わない。
ホストは正常終了・起動失敗（b の `0x800700AA` を含む）・ページ失敗・時間切れ・例外を、
共通の終了経路に集める。controller があれば `Close()` し、WebView / controller / environment を
解放する。作成 callback が未完なら STA のメッセージを処理し、遅れて届いた controller も即座に
Close する。新しいページは開かない。SDK の WMI で `msedgewebview2.exe` の引数を完全一致で
調べ、保持した OS プロセスハンドルも終了済み、未完 callback なし、という状態を500ms以上
観測してからホストを終える。終了待ち上限は `-ObserveSeconds`（既定30秒）。超過は exit code 6。
WMI の読取不能は静止と見なさない。Close の HRESULT 失敗も成功にせず exit code 6。
runner は終了処理の完走・Close の失敗なし・許容 exit code（正常0、起動失敗2、内部起動待ち切れ3）を
確認する。例外の exit code 10 やホスト内部エラーは、後片付けができても ERROR。
全プロセスを待つ間も watcher は監視を続ける。
通常は runner もホスト・WebView2 を Kill しない。停止／release 信号後の有界待ちで終わらない
場合のみ、失敗を先に保存し、最後の手段として該当プロセスを Kill する。WebView2 の終了対象は
試験 UDF 引数の完全一致に限る。Kill が成功してもその試行を ERROR、全体を STOP にする。
`cleanup-actions.partial.json` / `results.json` / `results.md` に期限超過・対象 PID・Kill を残す。
試験データとログは削除せず残す。

出力はすべて、この worktree の次のディレクトリに置く。

```text
target/webview2-lockfile-probe/
  bin/                         probe.exe, WebView2Loader.dll, page.html, build.json
  run-<UTC時刻>-<PID>/          実測の使い捨て UDF、証拠、results.json、results.md
  run-offline-<UTC時刻>-<PID>/  非 WebView2 検査の証拠
```

プロセス名だけで一括終了する処理はない。API の `userDataFolder` に渡すディレクトリを
ここでは UDF と呼ぶ。WebView2 Runtime は、その下に `EBWebView` を加えたディレクトリを
Chromium の `--user-data-dir` に渡すため、**引数の値と `<UDF>\EBWebView` を完全一致で照合する**。
引用符を Windows の規則で解析し、大文字小文字・区切りの表記差を正規化する。
兄弟ディレクトリや部分一致は対象にしない。
実行ファイル名でブラウザだけに絞らず、コマンドラインの該当 switch を持つ全実行ファイルを
調べる。GPU・renderer・network service・utility・crashpad handler 等も同じ完全一致の対象。
ブラウザの識別だけは `msedgewebview2.exe` かつ `--type` 無しで行う。
診断一覧には `--database=<UDF>\EBWebView\Crashpad` が完全一致する crashpad handler も
含めるが、それだけ一致したプロセスは終了対象にしない。ファイル所有者として報告された PID も
診断だけに使い、終了対象を広げない。
この階層差の根拠は [Microsoft の WebView2 用 DevTools 手順](https://learn.microsoft.com/en-us/microsoft-edge/web-platform/devtools-mcp-server#step-2-find-the-webview2-user-data-directory)。

## ファイルとビルド

| ファイル | 役割 |
|---|---|
| `probe.cpp` | 最小ホスト、lock 保持・5ms watcher、ファイル読取・実コピー・Restart Manager 照会 |
| `snapshot.h` | lock 取得 worker 内の全ファイル BCrypt SHA256、読取区間・前後 metadata、参考コピー |
| `page.html` | 同じ origin の localStorage・IndexedDB・OPFS の書き込みと読み出し |
| `build.ps1` | CHOC から宣言・x64 Loader を読み取り抽出し、VS 2026 BuildTools でコンパイル |
| `common.ps1` | パス制限、プロセスの完全一致、ファイル一覧・SHA256、複製、判定、レポート |
| `run.ps1` | 実測シナリオの実行、時間切れ、後片付け、証拠の保存 |
| `test-offline.ps1` | WebView2 を呼ばずに native lockfile と道具の判定・出力を検証 |

ビルドだけ行う場合:

```powershell
powershell -NoProfile -File .\tools\webview2-lockfile-probe\build.ps1
```

既定の読み取り元:

```text
G:\home\effetune-mixwright-v015\third_party\choc\choc\gui\choc_WebView.h
C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Auxiliary\Build\vcvars64.bat
```

別の場所なら `build.ps1 -ChocHeader <path> -VcVars <path>` で指定できる。
`MSBUILDDISABLENODEREUSE=1`、x64、C++17、`/W4 /WX`。NuGet・ネットワーク取得はしない。
CHOC の宣言境界・埋め込み Loader の長さと x64 PE machine・ライセンス境界が想定と違えば停止する。
CHOC の ISC 表示と Loader のライセンスを `bin/THIRD-PARTY-NOTICES.txt` に保存する。
上流のファイルは変更しない。

WebView2 の `CreateCoreWebView2EnvironmentWithOptions` を明示 UDF で直接呼び出す。
CHOC の `WebView` / `getUserDataFolder()` はコンパイルも呼び出しもしない。
origin のページ配信は上流 Mixwright の `fetchResource` / `customSchemeURI` と同じく、
`AddWebResourceRequestedFilter` + `add_WebResourceRequested` で要求を受け、
`CreateWebResourceResponse` とメモリ上の `IStream` で返す。CHOC の既存 COM 宣言だけを使い、
手書きの拡張 ABI 宣言・IID・`QueryInterface` による機能取得は使わない。
固定の `page.html` を UI 作成前に読み込み、要求の URI をファイルパスとして使わない。
クエリーを除いた既知ページだけを200、それ以外を404で返す。
環境変数やレジストリによる UDF の上書きから試験を守るため、このホスト子プロセス内だけの
`WEBVIEW2_USER_DATA_FOLDER` も同じ使い捨て UDF に固定する。製品での環境変数の設計を
実装したものではない。ブラウザの追加引数もこの子プロセス内で固定する。

UDF と操作用ファイルの引数は `target/webview2-lockfile-probe/run-*` の中だけを許可する。
PowerShell 側も同じ target 配下を検査し、既存の親・子の reparse point を拒否する。
引数省略時は使用法を表示して終了し、APPDATA の既定 UDF を選ばない。
利用者の `%APPDATA%\mimageviewer-vst3-host.exe`、`Frieve`、`effetune`、`mimageviewer` は
試験素材として読まず、書き込み・バックアップ・復元もしない。

## 判定と証拠

### (a) 正常終了中の lockfile と静止

`a-graceful` は各回で新規 UDF を使う。ホストを強制終了するシナリオ・引数は撤去した。
書き込んだ3種類のデータをページで検証し、API の browser PID が完全一致で抽出した
ブラウザ PID と一致することを確かめてから観測を始める。

終了試験で `mode=churn` を使う。最初の fixture を検証した後も、別のキー・ファイルに
localStorage → IndexedDB transaction 完了 → OPFS writer close の順で書き続ける。
一つの cycle が終わってから次へ進み、cycle 間は3msの休止を要求する。実際の cycle 時間は
保存 API の処理時間に依存し、50msごとを目安に最終完了 seq・store 別 seq・実時間を記録する。
3store をまたぐ transaction ではないため、終了時の store 間 seq の違い自体を破損とは判定しない。

watcher 準備後に新しい完了 pulse（10 cycle 以上）を受けてから、直ちに正常終了を要求する。
終了前に JavaScript を drain / flush / stop させない。終了要求前に書込み失敗が報告されていれば
計測を始めず、そのイベントを保存して未完とする。要求後の close に伴う中断とは区別する。
`last_churn_progress` と終了要求の時刻・QPC を残す。

- 生存中のブラウザに対し別プロセスから `GENERIC_READ | GENERIC_WRITE`、共有なし、
  `OPEN_EXISTING` の `CreateFileW` を試す。lockfile が消えていたら
  `CREATE_NEW | FILE_FLAG_DELETE_ON_CLOSE` での取得も試し、元のエラーも残す。
- native helper は PID の OS ハンドルを保持し、open 前後の生存を確認する。
  生存が前後とも確認された状態で open に成功したら **FAIL**。
  有効な共有違反のサンプルがなく、アクセス拒否などしか得られなければ **INCONCLUSIVE**。
- 正常終了は停止ファイルで要求し、正常な終了イベントと exit code 0 も確認する。
  ホストは controller の Close と全該当 WebView2 の終了待ちを行う。
- 終了要求の前に native `lock-watch` を待機させる。watcher は親・ブラウザの OS ハンドルを
  保持して約5ms間隔（要求する間隔。実精度は OS と I/O に依存）で観測する。
  watcher の開始後、親スクリプトは lock を開かず、その記録を読むだけにする。
  二つの観測ツールが互いに共有違反を作ることを避ける。
- 初めて取得に成功したら、**同じハンドルを閉じずに**保持する。
  最初の成功を捨てて取り直さない。親がまだ生きている場合もそのまま捕捉し、取得イベントの
  親の前後状態と `acquisition_host_state` を `HOST_STILL_ALIVE` / `HOST_EXITED` で明記する。
  成功後は保持の観測であって、新たな取得試行ではない。以前の繰り返し open の成功回数とは
  比較できないが、ブラウザ生存中に一度でも取れれば従来どおり **FAIL**。
- 親の終了前後それぞれの生存、PID、コマンドライン、作成・終了 UTC、総生存時間、
  親終了後の生存時間、最後に開けなかった時刻、初めて開けた時刻を保存する。
  開けるようになる正確な瞬間ではなく、サンプル間の区間として読む。
- ブラウザ群の自然終了と lock の取得を観測できた場合にサブシナリオ **PASS**。
  ブラウザが残ったまま観測を打ち切った場合などは **INCONCLUSIVE**。
  watcher 解放後の lockfile 有無と、watcher が `CREATE_NEW` したかを記録する。
  これは observer の介入を含み、Runtime だけの終了時の残存判定ではない。
  Runtime の通常終了直後の有無は、b の seed 終了時にも別に記録する。

以前のホスト終了後の orphan 観測は再実行しない。今回のホストはブラウザより長く生存し、
その終了待ち中に watcher が排他と解放を測る。取得がホスト存命中でも、ブラウザ生存前後に
一度でも取得できれば従来どおり FAIL。前の回の FAIL / ERROR / 不完全な終了は消さず、
全回のログ・タイムライン・集約結果を保存する。

### 取得直後と全該当プロセスの観測後の比較

取得した native worker が、取得時刻・生存状態をメモリに保存し、**ログの書出しや CIM の
待ち合わせより先に**全 UDF のファイル列挙・size・SHA256 読取を始める。lockfile 一つだけを除く。
取得の UTC FILETIME・QPC・周波数、実際の snapshot 開始・終了、各ファイルの読取開始・終了、
前後 size と最終更新 FILETIME、読取エラー、metadata の変化の観測を保存する。
`acquisition_to_capture_start_ms` は取得から snapshot 手続き開始までの遅延。各 file の実読取は
その後なので、最初のファイルまでの遅延も per-file の QPC から確認できる。
`lock_acquired` のログ公開は読取完了後になるため、イベントの `elapsed_ms` ではなく、
内側の取得 FILETIME / QPC を取得時刻として読む。

全フォルダは瞬間には hash 化できない。これは**読取区間の観測**であり atomic snapshot ではない。
前後の metadata に変化が見えなくても、memory-mapped 書込みや pathname の置換が無かった
証明にはしない。読取不能・途中変化・列挙不能・時間切れは完全捕捉として扱わない。

スクリプトは終了前から、完全一致する**全種類のプロセス**の OS ハンドルを保持し、CIM の
各 poll で新しい PID を追加する。PID と実ハンドルの作成時刻を CIM の microsecond と照合し、
不一致や保持できない行は `tracking_missing` に残す。終了を確認したハンドルの OS 終了時刻を
使い、取得時刻との差 `exit_minus_acquisition_ms`、取得時点の生存、role / type / utility subtype
を記録する。CIM で「見えなくなった時刻」を OS の終了時刻の代わりにはしない。

`alive_at_acquisition` は取得後も生きていた PID・種類の一覧。
`arrivals_after_acquisition` は取得後に作成されたプロセス、`newly_observed_after_acquisition` は
取得後に初めて観測したプロセスの一覧で、両者を区別する。待つ対象が増えたかを推測で
固定せず、全 poll とこの一覧を残す。CIM の poll 間で生まれて消えた短命 PID は見逃し得る。
新規起動の明示的な抑止試験は b が引き続き担当する。

完全一致する CIM 行が空・保持済みプロセスも終了済みという状態を、
連続する poll で500ms以上観測したら worker に quiet 信号を渡す。これは観測上の静止であり、
全プロセスを網羅した厳密な消滅証明とは呼ばない。そこで lock を保持したまま2枚目の全件
snapshot を取り、取得直後との差分を PROFILE / CRASHPAD_METRICS / OTHER に分類する。

### 参考: 観測上の静止を待ってからコピー

quiet 信号を受けた同じ worker が、lockfile だけを除いて新しい参考 UDF にコピーし、
コピー後の元とコピー先の全件 snapshot を取る。この間も CIM poll を続け、quiet 信号後に
該当 PID が現れれば参考結果を `INVALIDATED_NEW_MATCHING_PROCESS_AFTER_QUIET` にする。
コピー後の空の poll も2回観測し、ホストの有界な終了待ちが終わってホスト自体も終了してから
release 信号を送り、保持ハンドルを閉じる。取得直後の捕捉・quiet 判定・コピー自体は
ホスト存命中にも進め、watcher はホストの終了待ちの全区間を監視する。

結果は別々に記録する:

- 取得直後 → 観測上の静止: `acquired_to_observed_quiet` とその変更分類。
- 静止時のコピー前 → コピー後の元: `reference_quiet_copy.source_comparison`。
- 静止時の元 → コピー先: `reference_quiet_copy.copy_comparison`。

**待ったことで、取得直後から静止までに出た差分が消えたとは扱わない。**
参考コピーが同一でも、元の生存中取得の FAIL はそのまま残す。
取得直後の完全性、quiet / copy の完全性、信号プロトコルの完走を分けるため、最初の読取が
不完全でも、後の参考コピーの独立した結果は保存できる。全体の未完・従来の FAIL は保持する。
全 snapshot / copy には native の同じ期限があり、親側も期限で打ち切る。
親の観測期限は実際の終了要求を基点に `ObserveSeconds + 15秒`。ホストに全30秒（既定）を
待たせた後も、静止判定・参考コピーのために15秒を確保する。native watcher は準備後の
継続書込み pulse 待ち（最大5秒）を含め `ObserveSeconds + 30秒` の上限で動く。
その予算で全件読取・コピーが完了しなければ未完とし、検査対象を省略しない。
保持自体が Runtime の終了を妨げる可能性もあり、消滅しないケースを成功扱いにしない。
最後は保持 worker の終了でも OS がハンドルを解放する。
計測途中の例外時は、先にホストへ正常終了を要求し、watcher へ専用 abort 信号を送る。
quiet やコピー前でも協調的に終了し、protocol / observation は未完として残す。
通常の release 信号による完走と、abort による未完を混同しない。

各回の証拠は `a-<mode>-rN-capture-{acquired,quiet,after-copy,copy}.json`、
`a-<mode>-rN.json`、`a-<mode>-rN-watch.jsonl`。参考結果は製品の移行方式の採用ではない。

### ログとハンドル継承の観測

ログ読取は `FileShare.ReadWrite | FileShare.Delete` を使う。
改行まで書かれた JSON だけを解析し、書込み途中の末尾は次回まで保留する。原本の断片は
ファイルに残る。改行まで書かれた不正 JSON はエラーとして扱う。

ログ書込みだけを Win32 `CreateFileW` の append・共有 R/W/Delete・security attributes=null に変更した。
各書込みの `GetHandleInformation` 結果を `log_handle_*` に残し、非継承であることを確認できる。
標準入力・出力・エラーのフラグは `standard_handle` に記録し、**変更しない**。
WebView2 内部のプロセス起動・`bInheritHandles` もフック・変更しない。上流 CHOC と同じく
Runtime に任せる。ホストを起動する .NET 側の redirect 設定は `.process.json` に記録する。

ホスト終了後に pipe の EOF がまだ来なくても `ReadToEndAsync().Result` を待たない。
`RanToCompletion` のときだけ内容を取り、pending / faulted / canceled と、その後の EOF の
観測履歴を残す。継承可能フラグや EOF の未到着だけで、特定のブラウザ PID が継承したとは
断定しない。終了後のログについても、後述の Restart Manager の使用者一覧を保存する。
ログの非継承化により、今回以降のログは継承を避ける。前回の継承フラグは遡って確定できない。

### (b) 起動の抑止・写す元の不変性

`b-existing` と `b-create-new-delete-on-close` も別々に、正常に使用・終了した UDF を作る。
後者では、その使い捨て素材に限って既存 lockfile を消し、`EBWebView` が存在するまま
lockfile が無い状態を作る。前者で Runtime が lockfile を残さなかった場合は、空の試験用
lockfile を置いて `OPEN_EXISTING` の経路を試す。どちらも元の正常終了時の有無を記録する。

- 別 helper が共有なしのハンドルを保持し、さらに別プロセスからの error 32 で排他を確認する。
- その間に、同じ UDF を渡したホストの環境・controller 作成・ページ表示を試す。
  各同期呼び出し・callback の HRESULT と経過時間、500ms ごとの UI heartbeat を保存する。
- 起動段の失敗 HRESULT を **EXPLICIT_STARTUP_FAILURE**、起動待ち20秒でページに
  到達しなかった場合を **BOUNDED_NO_PAGE** として分ける。後者は今回の依頼の
  「または表示に至らない」に対応し、永久に起動できない証明ではない。
- ページから一度でも通知が来た場合は抑止成功にしない。
  ページ配信など試験側の失敗は lock による起動拒否として数えない。
  heartbeat の最大空白が3秒を超えた場合や、外側の watchdog による強制終了は成功にしない。
- 起動試行前、試行後の lock 保持中、lock 解放直後、自然な静止を待ってさらに500ms後に、隠しファイルも含む全ファイルの
  相対パス・大きさ・SHA256 を比べる。除外するのは **`EBWebView/lockfile` 一つだけ**。
  **すべての段階が strict 判定の対象**。一つでも追加・削除・内容差・読取失敗があれば成功にしない。
  読めないファイルも一覧から省かず `read_error` を保存し、前後が同じ読取エラーでも不一致にする。
- `CREATE_NEW` の場合、ハンドルを閉じた後に lockfile が残っていないことも確認する。
  さらに排他を外した同じ UDF を開き、3種類のデータが読めることを正の対照とする。

変更ファイルの分類は次のとおり。分類や正規化によって合否を緩めない。
変更を確認した分類は true、十分に比較して差分なしは false、読取不能だけの場合や列挙不足は
null（Markdown では UNKNOWN）とする。未確認を変更確定とも変更なしとも扱わない。

| 分類 | 対象 |
|---|---|
| `PROFILE` | `EBWebView/Default/` 以下、および `Local State`・`Preferences`・`Secure Preferences`・`First Run`・`Last Version`・`Last Browser`・`Variations` |
| `CRASHPAD_METRICS` | `Crashpad/`、`CrashpadMetrics.pma`・`CrashpadMetrics-active.pma`、`DeferredBrowserMetrics/`・`EBWebViewMetrics/`・`BrowserMetrics/` |
| `OTHER` | 上記以外。未知のファイルをメトリクスとして除外しない |

各回・各段階の raw の差分／読取未確認パス集合と、その集合が毎回同じかを保存する。
別に、`BrowserMetrics-*.pma` / `EBWebViewMetrics-*.pma` の可変末尾だけを正規化した診断用の
集合も保存する。raw のファイル名・hash・差分はそのまま残す。後の回の成功で前の FAIL を消さない。

### ファイルの使用者とコピー可否

lock 保持中・解放直後・静止後に、変更ファイルと Crashpad / metrics ファイルを調べる。
`file-probe` はそのファイルを `GENERIC_READ`、共有 R/W/Delete で最後まで読み、
別の証拠ディレクトリへの **`CopyFileW` による実コピー** も試す。読取とコピーそれぞれの
Win32 エラーを保存する。コピー先は同じ run の `*-read-copies/` で、上書きしない。
元の UDF は書き換えない。

Restart Manager の `RmStartSession` / `RmRegisterResources` / `RmGetList` / `RmEndSession`
で使用者の手がかり（PID・プロセス作成 FILETIME・アプリ名）を得て、可能なら後続の
プロセス一覧のコマンドライン・作成時刻を添える。FILETIME と後続一覧の作成時刻を照合して読む。
プロセスが先に終わることもある。RM のエラーも保存し、終了・再起動の API は使わない。
**RM は全 handle / memory mapping の完全な列挙ではない**。空の結果を「誰も持っていない」
証明にはしない。stdout pipe の特定の継承先も、この方法だけでは確定しない。

診断のみ、1段階32ファイル・開始時間予算30秒、1ファイルの native worker は10秒で打ち切り、
owner の追加一覧照会は最大4件・1.5秒の開始予算を設ける。開始済みの有限な呼び出しは予算後に
終わる場合がある。未観測のパス・owner 件数・理由を保存する。**全件の manifest / hash 比較は
上限で省略しない**。診断 worker の終了に失敗した場合は PID とエラーを残し、全体を STOP にする。

### (c) lockfile を除くコピー

`c-copy-and-reopen` は新規 UDF X で、
`https://effetune-mixwright.localhost/` に対するリソース要求を上流と同じ経路で応答する。
上流 `src/bridge/webview_host.cpp` の Windows origin と同じ文字列である。

localStorage の文字列、IndexedDB の文字列とバイナリ、OPFS ファイルの UTF-8 文字列と
バイナリを保存し、その起動内でも読み出して照合する。IndexedDB の transaction 完了と
OPFS writer の close を待ってから終了する。

自然終了を確かめ、X の lock を別 helper で保持したまま、lockfile 一つだけを除いて
UDF 全体を Y にコピーする。X の前後と Y の全ファイルの size・SHA256 を比較する。
保持を解いて Y を起動し、3種類の期待値を読み出す。続いて X も再起動して同じ値を読む。
空のディレクトリもコピーする。コピー前から存在する Y への上書きはしない。

これはブラウザの保存領域のコピー試験であり、旧 Mixwright v0.12〜0.14 の
保存データを新しいプラグインが読めるかという §5.5 の別の受け入れ試験の代替にはならない。

### 全体の停止条件

`results.md` は人向けの時刻・HRESULT・PID 生存時間・hash 比較・3種類の読み出しの要約。
`results.json` は全シナリオとイベントの機械可読記録。途中失敗も `.jsonl` と
`.startup.json`、`.process.json` に残す。全ファイルの一覧と hash は各 manifest に残す。

各シナリオの PASS / FAIL / ERROR / INCONCLUSIVE と各回の厳しい判定は維持する。
全体の Migration prerequisite は、3回目の実測で §5.2 の文字どおりの前提が既に FAIL と
観測されたため **STOP**（終了コード2）を維持する。今回の正常終了や参考コピーが成功しても、
過去の FAIL を消して移行を許可しない。強制終了の既存証拠は
`run-20261024-140853-682-71368` の1回分として記録し、再実行しない。

### WER と Application ログ

シナリオの直前・全後片付けの直後（追加1秒の有限観測）に `WerFault.exe` の PID・作成時刻・
コマンドラインと、Application ログの id 1000/1001 のうち `msedgewebview2.exe` / `probe.exe` を
含むイベント（時刻・RecordId・本文・XML）を読む。直前のイベントは過去24時間の baseline、
直後は実行開始以降。過去イベントを今回のクラッシュと断定しない。
`crash-before.json` / `crash-after.json` と results に残す。WerFault やダイアログを閉じたり、
OS のエラー表示を抑止したりしない。観測不能は UNKNOWN、実行中イベントや前後の WerFault は
OBSERVED として成功扱いにしない。NONE_OBSERVED も有限の前後サンプルであり、途中の短命な
WerFault や観測後に遅延するイベント／ダイアログが絶対に無かった証明ではない。

**STOP のまま移行の本実装に進まない。** ClaudeCode が証拠と停止条件を
確認し、設計前提が成り立たないときは設計担当に戻す。成功しても、Chromium の全バージョン・
観測されなかった瞬間までの証明ではない。画面通知は2回の animation frame を経たものを
使うが、物理画面のピクセルを別経路で検査する試験ではない。

## Codex 側の非 WebView2 検証

```powershell
powershell -NoProfile -File .\tools\webview2-lockfile-probe\test-offline.ps1
```

WebView2 の環境・controller・ブラウザは起動しない。native の共有違反・既存ファイル非破壊・
delete-on-close・有限の保持時間、保持中のコピー、全ファイル hash、同じ大きさの内容変更、
プロセス引数の照合、runner の holder 起動と失敗記録、起動拒否判定、JSON / Markdown の
出力を検証する。途中例外があれば `offline-results.json` に成功を残さない。
並行 append 中の共有読取、改行途中の保留、ログの非継承フラグ、pipe の pending / fault / cancel、
全回を保持する集約、遅延した hash 差の FAIL、Crashpad の診断と cleanup の分離も検査する。
native lock helper を親・ブラウザの代役にしたテストで、親終了後の5ms watcher と
「watcher 開始後に親が lock を開かない」実際の runner 経路を検査する。代役の unlock〜終了の
短い区間を検出して FAIL になることは、WebView2 の不具合ではなく合成素材の観測である。
取得ハンドルを保持したまま合成 PROFILE / metrics を変え、取得直後との差分が残る一方、
quiet 後のコピーは独立に同一となるケースも検査する。native BCrypt と PowerShell の SHA256、
取得時の読取不能と後の参考完走の分離、PID 作成時刻の不一致、全反復の完走も検査する。
Node があればブラウザを起動せず churn ページの JavaScript 構文も確認する。
ページ配信の URI 判定・200/404・ヘッダー・メモリストリームの内容も、WebView2 を呼ばずに
`probe.exe resource-check` で検査する。この検査だけで実際のブラウザからの要求受付まで
成功したとは扱わない。
レポートの検査素材には `SYNTHETIC_NOT_A_RESULT` を表示する。
検査中の `offline-exception : ERROR` は、意図した例外を正しく失敗に分類する検査素材の表示。

2026-10-24 の最初の外部実測 (`run-20261024-132810-237-34940`、ClaudeCode 実行) は、
Runtime 155.0.4283.62 の環境・controller 作成後、手書きの誤った IID による
`query_mapping: 0x80004002` で全シナリオ ERROR / STOP。lockfile の振る舞いを判定した結果ではない。
その後、上記の上流と同じリソース応答方式に変更した。修正後のビルド・非 WebView2 検査を行い、
ブラウザを使うシナリオの再実測は ClaudeCode がサンドボックス外で行う。
修正後の VS 2026 x64 `/W4 /WX` ビルドと非 WebView2 検査65項目は成功。
検証記録は `run-offline-20261024-133143-810-73164/offline-results.json` と
`resource-checks.json` に保存した。

2回目の外部実測（ClaudeCode、`run-20261024-133615-639-63716`）:

- `a-graceful` と `c-copy-and-reopen` は PASS。
- `a-forced` は `Wait-Host` / `Require-Ready` のログ読取中に共有違反で ERROR。
  スタック上はシナリオによる強制終了前であり、このログだけで終了後のハンドル継承を示したとは言えない。
- b の両モードは起動抑止（`controller_callback: 0x800700AA`）と応答性を確認したが、
  Crashpad / metrics の追加・内容差により FAIL。既存の raw manifest と結果は保存したまま。

この結果を受けて上記の反復・分類・ハンドルの観測を追加した。修正後のブラウザ実測は未実施。
最終の VS 2026 x64 `/W4 /WX` ビルドと非 WebView2 検査119項目が成功。
記録: `run-offline-20261024-140645-342-41756/offline-results.json`。
独立静的レビューも ACCEPT。合成素材の成功を WebView2 の成功に置き換えず、外部実測を待つ。

3回目の外部実測（ClaudeCode、`run-20261024-140853-682-71368`）は、正常・強制終了の
どちらも生存中の open を fast watcher が4回記録して FAIL。ClaudeCode の遷移の読みでは、
親終了後も共有違反が続いた後、ブラウザがまだ生きている間に取得でき、約16〜30ms後に
ブラウザが終了した。b は引き続き profile 外の PMA 差分のみで FAIL、c は PASS。
当時の a-forced は orphan 観測後に1回で止まっていた。この結果を受け、上記の取得保持・
継続書込み・全種類の終了時刻・quiet 後コピー・5+5回完走を追加した。追加版の外部実測は未実施。
追加版は VS 2026 x64 `/W4 /WX` ビルド・非 WebView2 検査161項目が成功。
最終証拠は `run-offline-20261024-150032-436-70684/offline-results.json`。
最初の snapshot の読取不能と、その後の quiet-copy 完走を分ける検査、quiet 後の新 PID で
参考結果を無効にする検査も含む。製品の移行処理・設計書の受け入れ条件は変更していない。
このサンドボックスでは実プロセス一覧の `Get-CimInstance` も `0x80041003` (アクセス拒否)
となった。プロセス照合自体は合成したコマンドラインと一覧で検査済みだが、実一覧の取得と
WebView2 実測は上記のサンドボックス外コマンドで行う。

利用者の追加要件を受け、ホストの全終了経路の Close・自然終了待ち、callback 内の例外処理、
強制終了シナリオ撤去、Kill 前の失敗記録、前後 WER 観測を追加した。
強制終了1回の既存実測は上記3回目の記録として保存し、再実行しない。
前の5+5回用追加版も今回版も、WebView2 の再実測はまだ行っていない。
今回版の VS 2026 x64 `/W4 /WX` ビルドと非 WebView2 検査188項目は成功。
最終証拠: `run-offline-20261024-152644-296-76828/offline-results.json`。
native の callback 例外・ログ書込不能・UDF 完全一致、終了待ち／Close 失敗の ERROR 伝播、
quiet 前 abort、通常 cleanup の Kill 不使用、WER / Application 記録とその観測不能の報告を
含む。検査素材のプロセスも通常は協調的に解放し、最後の手段の Kill は必ず失敗として記録する。
独立静的レビューは ACCEPT。実際の Runtime での終了とダイアログ有無は外部実測待ち。

# アイドル健全性チェック

## 1. 目的

静止中・背面表示中に、同じ work の再投入や不要な repaint が高速で続く回帰をリリース前に
検出する。通常のヒッチ検査は「1 フレームが遅い」問題向けであり、今回の動画ピンのように
各処理は短いが 1 コアを使い続けるループは見逃す。そのため、次の 2 系統を同じ測定区間で
検査する。

- `--perf-log`: `frame.begin`、`ui.tail_repaint`、thumbnail work の反復を解析
- プロセス外 sampler: CPU time、`mimageviewer.log` / `perf_events.jsonl` の増加量を測定

アプリ内に周期 heartbeat を置くと、正常時にもアプリ自身を起こしてしまう。完全に sleep して
測定区間内の perf event が 0 件になる状態を正常として扱うため、wall time と CPU は
PowerShell 側で測る。ただし空の窓を、perf log を書かない別 instance の sleep と誤認しないよう、
外部 sampler は取得できた `Win32_Process.CommandLine` に `--perf-log` があることを、完全 sleep を
許可するための必須証拠にする。WMI が拒否されて command line を取得できない環境では、path だけを
記録して warning を出し、空の窓の sleep 証明を認めない。実行パスが取得できた場合は常に
`%APPDATA%\mimageviewer\runtime\` 配下のランチャー展開コピーではないことも確認する。
analyzer の `session.start.pid` 照合はその後のログ整合性確認であり、単独ではプロセス同一性の証拠に
しない。2026-10-09の§1.356以降はログの保存完了証明も必須とする。手動samplerは測定開始前と
終了後のcheckpointを照合する。ReleaseChecks suiteは停止済みのshutdown checkpointを照合してから
perf解析する（測定中にflushやrepaintを要求しない）。
session累計の通常／perf／診断ログ欠落が一件でもある場合、完全性を確認できない場合、旧buildの場合は
参考値を残して手動scriptは `CannotJudge`、suiteは `SKIPPED`（いずれも判定不能／環境不成立、exit 2）にする。
CPUが低い、保存済みframeが少ない、
command lineに`--perf-log`があるという事実だけではPASSにしない。

## 2. 実行方法

### 正規手順: ReleaseChecks suite（2026-10-11採用、毎リリース）

利用者がrepository rootのPowerShellで実行する:

```powershell
.\scripts\ui-smoke.ps1 -Scenario ReleaseChecks -InteractiveApproved
```

診断portableのbuildと生成fixture／隔離dataのseedを行い、static-foreground、static-background、
video-pin-background、tray-close-while-loading、tray-close-after-loadingの5 idleケースと、
cached／uncached perfを逐次実行する。通常終了後に証跡を保存して解析し、途中のEnterや手動setupは不要。
**CLAUDE Phase 2 9.6／9.7は同じ1回のrunで確認する**。FFmpeg CLIがPATHに必要。
seed込み約5〜8分、初回buildは別（採用確認日のbuildは約7.5分）。

開始前に他アプリの窓を最小化し、build待ちを含め終了までPCに触れない。suiteがexact rootを
前面にし、背面ケースではrunner所有の専用窓へ移行する。利用者がコンソール等へ切り替えない。
foreground条件が失われれば停止して証跡を残す。環境を整えた別runで再実行し、自動retryで隠さない。
同一ソースの診断artifactが検証できる場合だけ `-SkipBuild` を追加できる。
先に `prepare-portable-smoke.ps1 -TestScript` でbuildしておくこともできる（この準備は起動しない）。

結果は `target/ui-smoke/runs/<run-id>/` のcase別log／sampler／case JSON、`summary.json`、
貼り付け用 `release-record.md` に保存する。0=PASS／PASS_WITH_WARNINGS、1=FAIL、2=SKIPPED／環境不成立。
警告・環境注記を記録し、証拠不足をPASSにしない。閾値は引き続き `idle_health_thresholds.json` を正本とする。
suiteはexact PID／exe／data path、測定区間全体の窓状態、UTC／session anchor／worker sleep包含、
cached／source来歴とtray要求寿命も照合する。性能値だけが低くてもこれらが欠ければ成立しない。

採用はエージェントの実行了承ではない。[実行確認](interactive-release-verification.md)に従い、
具体的なsuite／所要時間／desktop入力／使い捨てdata／AI有無への明示了承後だけ起動する。
`target/portable-smoke/mimageviewer.exe` と `target/portable-smoke/data` を使い、通常profileを起動せず、
利用者設定をコピー・初期化しない。full gateやbuildからの自動起動も行わない。
採用証跡・perf分類・手動で残す範囲は [自動化計画](release-check-automation-plan.md) を参照。

### 手動fallback／原因診断

以下は従来の `check-idle-health.ps1` による手動手順。利用者が通常releaseを起動するため、
実設定を開くことに注意する。エージェントはこの通常profileを起動しない。
隔離dataによる正規suiteの代わりに通常設定を消したり、suiteの証拠不足を手動の低CPU値だけで補わない。
先にrelease verification binaryを作る。

```powershell
.\scripts\build-release.ps1
```

最初のシナリオはスクリプトが `mimageviewer-core.exe --perf-log` を起動する。画面を準備して
Enter を押すと、5 秒 warmup 後に 15 秒測定する。測定中はマウス・キーボードへ触れない。
前面シナリオは Enter 後の warmup 中に mImageViewer へフォーカスを戻す。背面シナリオは
コンソール等を前面のままにする。warmup 中の入力は測定対象外で、測定開始表示後の入力だけが
idle 測定を無効にする。シナリオ名に `foreground` / `background` が含まれる場合は、測定開始・
終了時の foreground process ID も検査し、実際の状態と名前が一致しなければ FAIL にする。

スクリプトは Enter の prompt を出す前に、FTS と名前索引の全 supervisor が初回スキャンを
終えたことを表す `index.initial_scan_settled` を待つ。待ち上限は
`idle_health_thresholds.json` の `index_settle_wait_seconds` (既定 180 秒)。上限に達した場合は
`Initial index scan completion could not be confirmed; measuring anyway.` と warning を出し、process report の
`index_scan_wait` を `timeout` にするが、それ自体は FAIL にしない。完了を確認した場合は
`settled`。これは時間窓で索引負荷を隠す待ちではなく、明示的な完了イベント待ちである。

```powershell
.\scripts\check-idle-health.ps1 -Scenario static-foreground
```

同じプロセスで続ける場合は `-NoLaunch` を使う。`-NoLaunch` はプロセス名の最新候補ではなく、
perf log の `session.start.pid` と一致するプロセスを選ぶ。背面シナリオでは、別ウィンドウを
前面へ移してから Enter を押す。選択後は `-ProcessId` / `-NoLaunch` / 自前 launch のどの経路も
同じ `Win32_Process` 検査を通る。command line を取得できて `--perf-log` のない instance、または
判明した path がランチャー展開コピーなら測定前に停止する。WMI が拒否された場合は
`identity_evidence=path_only` として続行するが、空の測定窓を完全 sleep としては許可しない。

```powershell
.\scripts\check-idle-health.ps1 -NoLaunch -Scenario static-background
.\scripts\check-idle-health.ps1 -NoLaunch -Scenario video-pin-background `
    -TargetKey 'D:\media\video-pin-folder'
.\scripts\check-idle-health.ps1 -NoLaunch -Scenario tray-residency
```

既存の対象プロセスを明示する場合は `-ProcessId <PID>` を使う。そのプロセスが
`--perf-log` なしで起動されていた場合、stale perf log を誤解析せずエラーにする。

結果は `target/idle-health/` に出る。

- `*-perf.json`: update / repaint / work 反復の解析結果
- `*-process.json`: CPU / ログ増加量、top-level / visible window 数、対象の
  `identity_evidence` / `command_line` / `path`、`index_scan_wait`、checkpoint／完全性失敗理由と、
  両検査を統合した最終 PASS / FAIL / CannotJudge
- `*-prefix.jsonl`: 終了checkpointのACKで確定したperfファイルのbyte prefix
- `*-checkpoint.json`: 保存済みcheckpointとの照合に使うACK

スクリプトは性能・シナリオの失敗時にexit 1、証拠不完全時にexit 2を返す。判定不能はPASSへ変換しない。
起動した mImageViewer は追加シナリオを続けられるよう
終了させないので、検査後に通常操作で完全終了する。

### 2.1 非同期ログの保存完了証明

local logging-controlは`\\.\pipe\mimageviewer-log-{pid}-{process_start_unix_ms}`のnamed pipeを使う。
u32 little-endianの長さに続くUTF-8 JSONで、schema、request ID（nonce）、PID、OS process作成時刻、
session IDを指定する。clientのconnect／write／read全体は一つの2秒deadlineで、再要求しない。
timeout、Busy、Closing、writer／sink failure、不一致は判定不能にする。計測用warmupや測定時間は
残すが、固定sleepをflush完了の証明に使わない。制御requestはUI入力やrepaintを発生させない。

開始前のACK取得後にCPU・wall time・ログサイズの測定を開始し、終了時の測定値を採取してから
終了checkpointを要求する。sessionの`process_start_unix_ms`を実process作成時刻と照合し、
`wall_unix_ms`／`wall_t`で測定境界をperfのmonotonic `t`へ換算する。終了ACKの`coverage_t`が測定末尾を
覆い、publication cutoff以前のsettled／written watermark、両sink状態、三種の累計drop counterが
明示されることを必須にする。drop summary未保存をzeroと推測しない。

clientはACKの`perf_path`をread共有で開き、Windowsのvolume serial／file indexを`perf_file_identity`と
照合する。同じhandleから`perf_end_offset`までだけ読み、後続append中のJSONやrotation後の別ファイルを
混ぜない。analyzerはこのprefixのbyte長と保存済み`log.drain_checkpoint`をACKと照合する。
ログ増加量は開始・終了の成功ACKにある`normal_written_bytes`／`perf_written_bytes`の差で計算する。
通常logのrotationでも累計値は減らず、終了barrierで保存したtailを含む。perfのcheckpoint自身は
自己参照を避けるためbyte数から除外する。counter不明・逆行・identity不一致はCannotJudge。
完全性確認に失敗してもprocess reportにCPUの参考値を残し、増加量不明はnullのままにする。

`idle-health`、`hitches`、`scroll`、`startup`は同じ完全性validatorを使う。手動で確定prefixを解析する場合は、
`--checkpoint-id <nonce>`と測定末尾（idleは`--end-t`、他は`--coverage-end-t`）を指定する。
live prefixには`--delivery-ack <ACK.json>`も渡す。終了済みsessionの末尾shutdown checkpoint、または
診断ZIPを展開した`delivery_manifest.json`を`--delivery-manifest <path>`で指定した場合は、その確定境界まで
解析できる。manifestのperf byte長・checkpoint・evidence_statusも検査する。
証明のない旧ログは既存の参考値を表示し、検証用の終了コードは2となる。startupのfirst_frame、scrollの
all_readyという終点自体が確認できない場合も判定不能とする。writer上のslow_io区間の重複だけで
UIがそのI/Oを待ったとは判断しない。

## 3. リリース前シナリオ

毎リリース、正規suiteで次を実行する。以下のEnter／TargetKey／手動[×]はfallbackで同じ条件を
準備する場合の手順であり、suiteでは生成fixtureと台本が設定・操作する。

1. 通常画像フォルダのロード完了後、前面で静止 (`static-foreground`)
2. 同じ状態を背面へ移して静止 (`static-background`)
3. 動画を代表画像に固定したフォルダを keep 範囲内へ置き、背面で静止
   (`video-pin-background`)
4. cache の無い Folder 代表を多数含む一覧を、**読込中に格納**と**読込完了後に格納**の
   両方で測る（suiteは `tray-close-while-loading`／`tray-close-after-loading`、手動は
   `tray-residency` を2回）。前者は未完了要求が残る間、後者は通常ロードと
   idle upgrade が完了してから `[×]` で close-to-tray し、格納したまま静止する。

通常画像の前面・背面測定には、884×444のような横長画像と、要求長辺374px付近の条件を
含める（§1.198）。既存の整数比率選択では374px要求が247×124になり、400px要求なら
400×201になるため、400pxのケースだけでは同一要求の反復を検出できない。
比率「自動(3:4)」・7列は発見時の設定であり、DPIや窓幅によって要求pxは変わる。
修正後も出力247×124は既存品質仕様として残るが、同一要求の再decodeは収束することが必要。
サイズを大きくしたときに高画質化が再開することも、静止測定とは別に確認する。

`thumb/ready`は初回Loaded化の計装であり、Loaded→Loadedの高画質化では出ない。
ready件数0や`load_phases.should_save=false`だけで結果破棄と判断せず、生成結果・
要求サイズ・UI消費・`idle_upgrade_enqueue`の反復を合わせて調べる。

`tray-residency` は Enter 前に手動で close-to-tray を行う。**トレイ常駐は既定 OFF** なので、
先に環境設定 →「タスクトレイ常駐」→「アプリを閉じる代わりに、タスクトレイに常駐する」を
ON にしてから `[×]` で閉じる。**最小化では成立しない** — Win32 では最小化したウィンドウも
`IsWindowVisible` が真を返すため。

読込中ケースは hide をまたぐ既存要求、読込完了後ケースは Loaded→Evicted の GPU 解放に
続く再取得の寿命を検査する。case 間で利用者の cache や設定を消さず、使い捨て fixture・
隔離データ、または利用者が用意した cache 未作成の対象をそれぞれ使う。2 回目が保存済み
cache hit だけになった場合は source の画像通知と保存完了通知の両方を検査したことには
ならないので、cache 状態を分けて記録する。保存省略 / 保存ありの source と cache hit の対照は
自動テストでも補う。

各ケースの名前、対象、cache 状態、close 時の requested / backlog、hide から収束までの
時系列と各 report をリリース記録に残す。close 操作は測定窓外に置くが、close 直後からの
work / backlog の経過も残す。warmup 5 秒・測定 15 秒は維持する。正当な初回 load が測定中に
残った場合は準備条件と収束時点を記録し、同じケースで収束後の静止窓も測る。
長時間の重複生成を warmup 延長で隠さない。読込中の時点を手動で合わせにくいことを理由に
完了後ケースを無効扱いしない。既存の完了後格納の利用者測定も GPU 退去経路の有効な証拠である。

warmup 後の測定開始時と 15 秒後の終了時に、対象 PID が top-level window を 1 個以上所有し、
その visible window 数がどちらも 0 であることを必須にする。これにより「単に別ウィンドウの
背面に置いた」測定をトレイ常駐として誤採用しない。

visible の数え方は**利用者から見えるウィンドウに限る**。framework が作る隠せない top-level
window を数えるとこのゲートは永久に通らない。除外するのは `Winit Thread Event Target`
(winit の 15x15 イベント受け窓。常に `WS_VISIBLE`)、`IME` / `MSCTFIME UI` (Windows が
スレッドごとに作る IME ヘルパ) と、64x64 未満のウィンドウ (名前を挙げていないヘルパ用の
保険)。2026-08-04 にトレイ常駐中の core を実際に列挙して確認した: メインウィンドウを隠すと
visible が 2 → 1 に減り、残った 1 個が winit のイベント受け窓だった。CPU one-core ratio と perf / log gate は他シナリオと共通で、
paused media / still は完全 sleep、active media はこの静止シナリオの対象外とする。

`video-pin-background` では、動画を代表画像に固定したフォルダのパスを `-TargetKey` で渡す。
この値は perf log の thumbnail event の `key` に対して、大文字小文字を無視した部分一致で
照合する (フォルダタイルの `key` は `dir::<path>` 形式)。analyzer は測定終了以前で最後の
`nav.load_folder_begin` まで evidence の下限を戻し、それが無い log では session 先頭 (`t=0`) を
下限にする。その下限から測定終了までに対象 `key` の thumbnail work が 1 件も無ければ、現在の
場所で keep 範囲へ入った証拠がないため FAIL になる。測定より前に work が完了していても、
最後のフォルダ load より後なら有効なので、連続シナリオのために対象場所を開き直す必要はない。
根拠は perf report の `evidence_floor_t` / `evidence_floor_basis` に残る。
対象タイルの work はあっても `idle_upgrade_enqueue` / `idle_upgrade_ineligible` が無い場合は、
手動analyzerはアイドル高画質化パスの評価未確認をwarningで知らせる。
**ineligibleだけを必須にしない**。正規suiteは最新folder-load以降の対象workに加えてcached読取と、
`idle_upgrade_enqueue` **または** `idle_upgrade_ineligible` の評価証跡を要求し、不足はSKIPPED／環境不成立。

**ピン作成直後はこのシナリオは成立しない。** 作りたてのサムネイルは `from_cache=false` の
ためアイドル高画質化の候補にならない。このシナリオが意味を持つのは、アプリ再起動後など、
対象フォルダのサムネイルがキャッシュから読み直される状態である。上記 warning が出た場合は
その状態を作ってから測り直す。

ZIP / PDF / スマートフォルダ、AI、動画・音楽などの非同期経路を変更したリリースでは、対象の
ロード・解析が完了した状態も追加する。動画再生中、スライドショー中、索引・解析中は継続
処理が正当なので、静止シナリオへ混ぜない。

タスクトレイ常駐中の active media は continuous EOF を進めるため最大 20 Hz の UI tick を明示的に
維持するが、これは再生中シナリオであり本チェックの静止上限とは分ける。paused media / still の
tray residency は wake gate 対象外で、完全 sleep が期待値。`video-pin-background` は動画サムネイルの
pin / keep-range を測るシナリオで `VideoPlayer` を生成しないため、この active-media 例外には入らない。

## 4. 判定値

正本は `scripts/idle_health_thresholds.json`。初期値は、今回観測した 1 論理コア相当の
ループを十分な余裕で検出しつつ、OS の一時的な repaint を許容する値にしている。

| 指標 | 目標 / 上限 | 意味 |
| --- | --- | --- |
| CPU one-core ratio | 目標 0.05 / 上限 0.10 | `CPU time delta / wall time`。1.0 が論理コア約 1 本 |
| update rate | 目標 2/s / 上限 10/s | 測定区間の `frame.begin` 件数 / wall time |
| repaint reason streak | 上限 2 秒 | 同じ `requested_nonempty` 等が 1秒以下の gap で継続した時間。1.5Hz 程度の低頻度ループも検出する |
| same thumbnail work | 上限 3 回 | kind + key + idx + items generation が同一の work 件数 |
| input event | 上限 0 | 測定中に操作が混ざった場合は無効な idle 測定として失敗 |
| 通常ログ増加 | 上限 16 KiB / 15 秒 | 非構造化ログの高速肥大を検出 |
| perf ログ増加 | 上限 256 KiB / 15 秒 | 未知のイベントループも量で検出 |

input event から除くのは `cat=input` / `kind=modifier_probe` / `trigger=heartbeat` /
`keys=[]` / `wheel=false` が全て揃う受動 snapshot だけで、focus や修飾キーの値は問わない。
key press / release、wheel、他の input、keys 非空 / wheel=true の矛盾 heartbeat、必須 field が
欠けたものや型の異なる未知形式は操作として数える。snapshot は総 event 数に残し、
frame / repaint / 同一 work の反復、CPU・ログ量、PID / session、空窓の判定は変えない。
この除外は入力件数だけに適用する。配送checkpointの不足・測定末尾までの未保存・欠落が
ある場合は、受動heartbeatを除いてinput=0でも `CannotJudge` とし、PASSにしない。

正常版で各シナリオを 3 回程度測定し、JSON report を残して環境内の分布を確認する。閾値を
調整するときは単に失敗を消すのではなく、`top_repaint_causes`、reason streak、repeated work
から正当な継続処理かを確認する。静止シナリオで継続理由が説明できなければコード側を直す。

## 5. 計装と自動テスト

アイドル高画質化は候補を最終解決した時点で次を emit する (`--perf-log` 時のみ)。

- `thumb.idle_upgrade_enqueue`: 最終 `skip_cache=true` で upgrade queue へ進む
- `thumb.idle_upgrade_ineligible`: 完成済み派生キャッシュのため対象外

Sourceの`thumb.decode_end`には、実際に生成へ使った`evaluated_display_px`と、
`display_output_width`/`display_output_height`を記録する。§1.198の再確認では、
374px要求から247×124が生成されたことと、その後同一要求が反復しないことを併せて見る。
`thumb.ready`は初回Loaded化だけなので、Loaded→Loadedの生成完了数の代用にはしない。

どちらも `key`、`idx`、`items_gen` を持つ。同じ identity が入力・items 世代変更なしに
繰り返されると `idle-health` が失敗する。ただし `idle_upgrade_ineligible` は memo により
`(idx, items 世代)` ごとに最大 1 回しか emit されず、ピン作成直後には発生しないこともある。
このため、単独の必須イベントとしてシナリオ成立判定へ使わない。解析ロジックの回帰テストは
次で実行する。

```powershell
python scripts\test_analyze_perf.py
.\scripts\test-idle-health-delivery.ps1
```

回帰テストは focus 有無の受動 heartbeat が input=0 になること、実操作・矛盾・未知形式が
FAIL になること、および heartbeat を除いても work / repaint 反復による FAIL が残ることを検査する。
保存完了・checkpoint不足・coverage不足・各sinkの欠落・未排出watermarkと、受動heartbeat／
実操作を組み合わせ、解析本体とidle-healthコマンドの判定・exit codeも検査する。

Pythonテストは `scripts/build-dist.ps1` の先頭でも必ず実行され、解析ゲート自体の退行がある
状態では配布ビルドへ進まない。hidden repaint scheduler の純ロジックは vendored eframe の
unit test で production の throttle 関数に対して「即時要求を最短 100 ms 後へ送る」
「既に先の予定を早めない」「既存要求の無い window に heartbeat を挿入しない」を検査する。
logging-controlテストはアプリを起動せず、fake pipe→実client→file identityとprefix snapshot→実analyzerを
通してdrop zeroの完全sleepがPASS、累計dropが非zeroならCannotJudge／exit 2となることを検査する。

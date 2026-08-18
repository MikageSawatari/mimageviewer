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
記録して warning を出し、空の窓を FAIL にする。実行パスが取得できた場合は常に
`%APPDATA%\mimageviewer\runtime\` 配下のランチャー展開コピーではないことも確認する。
analyzer の `session.start.pid` 照合はその後のログ整合性確認であり、単独ではプロセス同一性の証拠に
しない。

## 2. 実行方法

先に release verification binary を作る。

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

結果は `target/idle-health/` に 2 ファイル出る。

- `*-perf.json`: update / repaint / work 反復の解析結果
- `*-process.json`: CPU / ログ増加量、top-level / visible window 数、対象の
  `identity_evidence` / `command_line` / `path`、`index_scan_wait` と、両検査を統合した最終 PASS / FAIL

スクリプトは失敗時に exit 1 を返す。起動した mImageViewer は追加シナリオを続けられるよう
終了させないので、検査後に通常操作で完全終了する。

## 3. リリース前シナリオ

毎リリース、最低限次を実行する。

1. 通常画像フォルダのロード完了後、前面で静止 (`static-foreground`)
2. 同じ状態を背面へ移して静止 (`static-background`)
3. 動画を代表画像に固定したフォルダを keep 範囲内へ置き、背面で静止
   (`video-pin-background`)
4. サムネイルが多いフォルダを開き、読込完了前に閉じてトレイへ格納したまま静止
   (`tray-residency`)

`tray-residency` は Enter 前に手動で close-to-tray を行う。**トレイ常駐は既定 OFF** なので、
先に環境設定 →「タスクトレイ常駐」→「アプリを閉じる代わりに、タスクトレイに常駐する」を
ON にしてから `[×]` で閉じる。**最小化では成立しない** — Win32 では最小化したウィンドウも
`IsWindowVisible` が真を返すため。

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
アイドル高画質化パスがそのタイルを評価していないことを warning で知らせる。

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

正常版で各シナリオを 3 回程度測定し、JSON report を残して環境内の分布を確認する。閾値を
調整するときは単に失敗を消すのではなく、`top_repaint_causes`、reason streak、repeated work
から正当な継続処理かを確認する。静止シナリオで継続理由が説明できなければコード側を直す。

## 5. 計装と自動テスト

アイドル高画質化は候補を最終解決した時点で次を emit する (`--perf-log` 時のみ)。

- `thumb.idle_upgrade_enqueue`: 最終 `skip_cache=true` で upgrade queue へ進む
- `thumb.idle_upgrade_ineligible`: 完成済み派生キャッシュのため対象外

どちらも `key`、`idx`、`items_gen` を持つ。同じ identity が入力・items 世代変更なしに
繰り返されると `idle-health` が失敗する。ただし `idle_upgrade_ineligible` は memo により
`(idx, items 世代)` ごとに最大 1 回しか emit されず、ピン作成直後には発生しないこともある。
このため、単独の必須イベントとしてシナリオ成立判定へ使わない。解析ロジックの回帰テストは
次で実行する。

```powershell
python scripts\test_analyze_perf.py
```

このテストは `scripts/build-dist.ps1` の先頭でも必ず実行され、解析ゲート自体の退行がある
状態では配布ビルドへ進まない。hidden repaint scheduler の純ロジックは vendored eframe の
unit test で production の throttle 関数に対して「即時要求を最短 100 ms 後へ送る」
「既に先の予定を早めない」「既存要求の無い window に heartbeat を挿入しない」を検査する。

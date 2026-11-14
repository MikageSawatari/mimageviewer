# ASR T1 区切り5: サブPCで観測した入力と自動送りの調査

2026-11-14。基点 `182074ced`。コミット・製品バイナリの起動はしていない。

## A. nativeのCtrl+ホイール

Ctrlが届いている場合、既存の経路は正しい。製品コードの入力処理は変更していない。

- `src/video/native_window.rs:2967` の `native_mouse_wheel_event` は `mouse_ctrl(wparam)`（同`:3020`、MK_CONTROL = 0x0008）を `wheel.ctrl` に入れる。
- `src/video/native_presenter/render_core.rs:10286` の `push_native_wheel_event` → `plan_native_wheel`（同`:952`）は、パネルのwheelを映像ズームより先にeguiへ渡す。
  `egui_modifiers` は `wheel.ctrl` を `ctrl` と `command` の両方に保持し、実際の `MouseWheel` に載せる。
- `vendor/egui/src/input_state/mod.rs` のwheel処理はzoom修飾つきのdeltaを `smooth_scroll_delta` へ入れない。
  共通文字起こし一覧の `manual_scroll_input` は、ScrollAreaと同じ処理済みdeltaを見る。

追加テストは本物の `push_native_wheel_event` と `render_logical_once` を使うheadless CPU描画で、Ctrlあり／なしを対照にした。
上3目盛り（delta=360）で、Ctrlありはoffset不変・Following維持・映像ズームcommandなし、Ctrlなしは上へスクロールしてManualになる。
別にWPARAMのCtrlビット有無を検査する。OSの実メッセージ配送や操作ツールのCtrl押下を検証したものではない。

ジャンプタブ（`overlay_draw.rs:1248`）と画像補正タブ（同`:729`）も通常の縦ScrollAreaで、同じ処理済みdeltaを消費する。
**一覧本文の上では、どちらもCtrlが届けばスクロールせず、通常wheelならスクロールする**（端ではoffsetが動かない場合がある）。
パネル上のwheelは映像ズームへ渡さないので、「映像が拡大しなかった」ことだけではCtrlの有無を区別できない。
native contextのキーボードzoom設定 `zoom_with_keyboard=false` も回帰テストにそろえた。
これはキーボード用の設定であり、wheelをscrollから除く判断は前述のInputStateが行う。

サブPCの観測Aでは本物のCtrlが届いたか未確定なので、入力ツールの問題と断定しない。

## B. presenter再作成後の一覧

確認できた不具合は、**強調する行が無いと、自動送り先も無くなる**こと。
`src/transcript_panel.rs:143` の `active_row` は発話区間内だけSomeを返し、修正前はFollowingのoffsetもその値だけから決めていた（修正後のoffset決定箇所は同`:288`）。
発話の隙間で新しい一覧を開くと `fresh` の分岐でoffset=0となり、既存の一覧なら以前のoffsetに留まる。
Following自体は維持されており、次の発話区間へ入れば送られる。

先に追加した再作成テストでは、同じsnapshotと現在位置を新しいoverlayへ渡して実際のタブResponseをクリックした。
発話内の118.5秒は成功、隙間の119.5秒は失敗して先頭に残った（修正前4件中3成功・1失敗）。

修正は共通描画側に `follow_row` を置き、発話の隙間では直前の行、最初の発話前は先頭、最後の発話後は末尾へ送る。
強調は従来どおり実際の発話区間内だけ。未知の原点では送らず、到着後の現在位置から求める。
動画・音楽ビュー・別ファイルへの移動・非表示からの再表示が同じ規則を使う。
追加state、delay、retry、repaint、特定のF12経路のguardは足していない。

### F12と同型の入口を追った結果

- `App::sync_native_transcript_snapshot`（`src/app/transcript_display.rs:69`）→ `VideoPlayer::set_native_transcript_snapshot` → NativeVideoOutput → `SetTranscriptSnapshot` → presenterの順で配信する。
  時刻原点の変更も `same_presentation` の比較対象。
- F12の新presenter準備では `cur_transcript_snapshot` を復元し（`src/video/mod.rs:6781`）、`tick_overlay_video_state(source.clock.now_secs(), ...)`（同`:6818`）で現在位置も渡す。
  overlayの `update_video_state` は位置の変化量によるdirty判定とは独立に `video_position_secs` を更新する。
- 新presenterのタブは既存どおりJump。文字起こしタブを開く際に共通の一時stateを作る。
  本文へはそのフレームのsnapshot・positionを渡し、時刻変換は既存の `TranscriptTimeMapping::to_transcript` を使う。
- source切り替えは既存のsession reset、音楽ビューはsource/context別の `MusicTranscriptPanelSession` を使う。
  パネル非表示時も既存の境界で一時stateを破棄する。このresetとeguiの保持offsetの有無によらず、表示時の送り先を現在位置から求める。

detachedの述語、viewport、窓の生成・切り替え・復元経路の製品コードは変更していない。
`docs/detached-rework-plan.md` §2の凍結対象に変更が無いため、同文書への追記は無い。

### 実素材と観測の対応・残る確認

区切り1の最終workerの保存済み出力 `target/asr-t1-fixes/asr-final.json` のm6（`runs/t0/m6/audio16k.wav`）では、
直前の区間は109.34〜116.80秒、次は118.92〜122.22秒。**1:58は発話の隙間**で、今回の不具合に合う。
ただし、手順17で使われた素材がm6かはこの記録だけでは確定しない。

自動非表示後の再表示は、同じ非表示時のstate破棄・共通描画を通る。
headlessでは再表示最初の描画で現在位置へ送られることも検査する。
サブPCで見えた「一瞬古い位置」のGPU表示そのものは再現確認していない。別のsurface表示の問題とは断定せず、専用の回避処理は足していない。

## 変更ファイル

| ファイル | 変更 |
| --- | --- |
| `src/transcript_panel.rs` | 自動送り先と強調行を分離。隙間・端・原点後着・source変更の回帰、test専用の状態参照 |
| `src/video/native_presenter/render_core.rs` | nativeのwheel入口、再作成後のタブ操作、非表示後の最初の描画を通す回帰テストのみ |
| `src/video/native_window.rs` | WPARAMのCtrlビットの回帰テストのみ |
| `src/ui_music_panels.rs` | 実タブhandlerと共通本文を通す、再表示・source変更時の回帰テストのみ |
| `docs/local-asr-t1-chunk5-report.md` | 本報告 |

作業中に設計担当が変更した `docs/local-asr-transcript-plan.md` は編集していない。

## 検証

本体をコンパイルする下記の実行はすべて次のwrapper経由。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner asr-t1 -WorkDir G:\home\mimageviewer-asr-t1 -Command 'python -B -I target/asr-t1-chunk5/run.py related'
```

runner引数は修正前が `pre`、関連131件が `related`、native最終5件が `native-final`。
ロックディレクトリの書き込み不可警告が出たため、scriptの既定どおりBelowNormalで実行した。
補助runnerはignoredの `target/asr-t1-chunk5/run.py`。環境は `CARGO_BUILD_JOBS=1`、`MSBUILDDISABLENODEREUSE=1`、
`CMAKE_BUILD_PARALLEL_LEVEL=1`、テストの並列度 `-j 1`。

| 実行 | 結果 |
| --- | --- |
| 修正前: `cargo nextest run -j 1 -p mimageviewer --lib -E 'test(transcript_native_tests) \| test(native_mouse_wheel_control_flag)' --no-fail-fast --success-output final` | 4件中3成功・1失敗。再作成後の隙間でoffset=0を再現。log: `target/asr-t1-chunk5/pre.log` |
| 修正後: `cargo nextest run -j 1 -p mimageviewer --lib -E 'test(/transcript/) \| test(native_mouse_wheel_control_flag)' --no-fail-fast --success-output final` | **131/131成功**。39.387秒（compile 5分40秒）。log: `target/asr-t1-chunk5/related.log` |
| テストの追加精緻化後: 修正前と同じnative filter | **5/5成功**。1.606秒（compile 5分02秒）。m6の1:58の実時刻と、native MouseMoveによるHover自動非表示→再表示も成功。log: `target/asr-t1-chunk5/native-final.log` |
| `python scripts/check_ui_glyphs.py` | 成功。危険なglyph 0 |
| `cargo fmt` | 成功。最後に実行（引数なし） |

既存の文字起こし一覧・native左右パネル・音楽左右パネルのdark/light PNG比較も131件に含み成功。PNGの生成し直し・更新は無い。
製品の起動・実F12往復・GPU描画は未実施。

計画書と異なる保存・時刻・session境界は導入していない。隙間の送り先を直前の区間とする細部は今回の修正で明示した。
build-devは今回の「最後にcargo fmtだけ」の指示に従い実行していない。修正後の実機結果はこの報告には含めない。

## C. 中止後のworker I/O失敗をログに残さない（基点e934fd3f3）

### 原因と終了経路

Opusの見立ては正しい。修正前の `src/ai/asr_worker.rs:122` の `ProcessOwner::fail` は、
`KillHandle` の状態に関係なく `asr_worker: ...` を記録してからkillしていた。

中止では `ui_dialogs::transcript_job::TranscriptJob::stop` がStoppingへ移り、待ち行列を空にして
`TranscriptWorker::stop` → `Control::stop`（`src/ai/transcript_job_worker.rs`）を呼ぶ。
Controlは取消の印を立ててからKillHandleをkillする。`src/job_process.rs` の `KillHandle::kill` は、
atomicのkilledをtrueにしてから `TerminateJobObject` を呼ぶ（UIで終了待ちはしない）。

kill後に起こる2つの失敗の順序は固定されていない。

- 抽出threadがpipeへの `write_all` で待っていれば、killによりI/O失敗で解け、`AsrInput::send` → `ProcessOwner::fail` が記録する。
- `asr-stdout` threadの `read_frame` もI/O失敗で解ける。このthreadはkillしてからエラーを容量1のchannelへ渡す。
  結果側が `AsrOutput::recv`（hello/load中なら `exchange`）で受け、`ProcessOwner::fail` が再び記録する。

このため、報告されたreadの「failed to fill whole buffer」とwriteの「os error 109」が、正常な中止でも同時刻に記録される。

### 修正

判定を `should_log_failure(error, killed)`（`src/ai/asr_worker.rs:122`）に分け、`log_failure` が実際の `KillHandle::is_killed()` を渡す。
**kill済みの `AsrError::Transport(WorkerError::Io(_))` だけを記録しない**。
メッセージの内容・言語・OS error番号では判定しない。エラーの返却、kill、ジョブの中止/終了処理は従来どおり。

ただし、stdout threadは自発的なEOFやread失敗でも、受信側より先にkillする。
`ProcessOwner::fail` だけを変えると、この最初の本当のI/O失敗まで消える。
そこで**読取側のI/O失敗だけは、このthreadがkillする前に同じ判定で記録する**（同`:178`）。
その後に受信側へ届く同じI/O失敗と、killで解けた書込み側のI/O失敗は抑制される。
framing/protocol不正、GPU/modelのfatal応答、timeoutなどはkill済みでも従来どおり受信側で記録する。
先にqueueへ届いたfatal応答をkill済みだからと読み捨てる変更もしていない。

中止以外の終了経路も照合した。

| 経路 | この規則でよい理由 |
| --- | --- |
| App終了、TranscriptWorkerのDrop | `stop_for_exit` → `stop` が同じKillHandleを使う。後続のpipe失敗は終了に伴うもの |
| 作成完了後 | `Finish`で実行役が戻り、AsrOutput/InputのDropがkillする。読取threadの後続EOFを失敗扱いしない |
| ASRの実際の失敗 | 検出時のI/O失敗をkill前に記録し、それによる他方のI/O失敗は記録しない。protocol/fatalはkill後でも残す |
| WriterAbortによる終了 | endを送れない失敗やpanicでkillし、他方のpipe待ちを解く。原因は既存のASR/job失敗経路で記録・通知し、後続I/Oを重ねない |

終了理由の追加state、待ち、再試行は導入していない。共有のKillHandle、VLM client、worker protocolも変更していない。

### テストと結果

変更は `src/ai/asr_worker.rs`、`src/ai/asr_worker_tests.rs`、本報告のみ。
loggerは共有sinkのままなので、実際の記録分岐が使う純関数で状態を検査する。

- `asr_io_failures_are_logged_only_before_kill_regardless_of_error_text`: 観測された2種の文言と任意のI/O文言で、kill前は記録、kill後は非記録。
  読取threadが先に記録・killし、受信側で同じエラーを受ける場合にも同じ判定となる。
- `asr_protocol_and_worker_failures_still_log_after_kill`: framing/protocol、fatal、GPU、timeout等はkill前後とも記録。
- 既存のkillによる読取・PCM書込の解除、先にqueueへ届いたfatal/framingエラー、中止・完了・保存通知の状態遷移も対象filterへ含める。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner asr-t1 -WorkDir G:\home\mimageviewer-asr-t1 -Command 'python -B -I target/asr-t1-chunk5/run.py cancel-io'
```

runner内部のコマンド:

```text
cargo nextest run -j 1 -p mimageviewer --lib -E "test(ai::asr_worker::tests) | test(ai::transcript_job_worker::tests) | test(ui_dialogs::transcript_job::tests)" --no-fail-fast --success-output final
```

結果は **35/35成功**（8.520秒、compile 4分48秒）。追加2件の状態テスト、既存の読取・PCM書込のkill解除、kill後のfatal/framingエラー受信、
作成ジョブの中止・追加・保存通知の回帰を含む。log: `target/asr-t1-chunk5/cancel-io.log`。
ロックへの書込不可警告後、run-heavyの既定どおりBelowNormalで実行。環境は上のA/Bと同じ。
最後に引数なしの `cargo fmt` を実行し成功。`git diff --check` も成功。
製品の起動・コミット・build-devは実施していない。

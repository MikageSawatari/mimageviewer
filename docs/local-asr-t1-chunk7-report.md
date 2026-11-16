# ローカル文字起こし T1 区切り7: master取り込み・メディア時刻への統一

作業ツリー: `G:\home\mimageviewer-asr-t1`。7bの開始HEADは `1340503b7`。
製品exeは起動せず、コミットは作らない。

## 7a: master取り込み

master `8d6201dce` の `MediaTimeline` をそのまま採用した。`decoder.rs` の衝突は、
masterの `timeline_origin_secs` とT1の一時的な `container_start_secs` を両方残して解消した。
音声一覧はmasterの `enumerate_audio_tracks_with_timeline` を `pub(crate)` とし、
T1の抽出が自身のInputから作る同じMediaTimelineを渡す。旧名のwrapperはtest専用にした。
再生の時刻・トラック選択の挙動は変えていない。
現在の参照位置は `src/video/decoder.rs:2130`（timeline付き一覧）と
`src/ai/transcript_audio.rs:119`（Input自身のtimelineを渡す呼出側）。

run-heavy経由の `cargo check -p mimageviewer --lib --tests` は成功。
`cargo nextest run -j 1 -p mimageviewer --lib -E 'test(/transcript|timeline|audio_track|media_timeline/)'`
は257件成功、失敗なし。ログは `target/asr-t1-chunk7a/check.log` と `tests.log`。
当環境でのgit addはworktree外のindex.lockへのアクセス拒否で実行できなかった。
その後、設計担当が取り込みを `be80d5135` としてコミット済み。

## 7b: 時刻の契約

設計担当の決定（計画書§6.3・§8.3、`1340503b7`）どおり、保存時刻・再生位置・行のseek・MCPを
すべてメディア時刻に統一した。原点を知るのは `MediaTimeline` だけとする。

- 抽出は自身のInputのMediaTimelineを保持し、新しい `raw_stream_ticks_to_media` でframeの整数PTSを変換する。
  原点をstream tickへ `av_rescale_q` で丸めてから引き、f64秒への往復を避ける。
  原点0では整数をそのまま保持する。既存の `aresample=16000:async=1:first_pts=0` がメディア時刻0からの
  無音を埋め、負のsampleを捨てる。選択した音声trackの開始時刻は引かない。
- TS/PSは再生と同じ有効原点を使う。正の開始時刻を持つMKVは有効原点0なので、開始10秒の素材なら
  先頭10秒の無音を保持する。第2trackがさらに3秒遅れる素材では、音声開始を13秒に保つ。
- 表示は保存msを秒へ直してそのままseekへ渡し、再生位置をそのまま強調・自動送りに使う。
  再生metadata到着前も行を押せる。通常のplayerのclampは共用し、専用の補正や迂回は設けない。
- `get_transcript` も保存msを秒へ直す既存の処理をそのまま使う。descriptionとhandlerのコメントを
  「mIVに表示される再生時刻」へ合わせ、開始秒での絞込みと返す区間が同じ座標であることを検査する。

撤去したものは `TranscriptTimeMapping`、snapshotの `time_mapping`、
`VideoPlayer::transcript_time_mapping`、`VideoInfo.container_start_secs`、
`container_start_to_secs`、抽出側の `timestamp_origin` と独自のコンテナ原点計算、原点待ちの状態・検査。
`timeline_origin_secs` はmasterの診断・旧resume移行用として残す。
未出荷のため保存済み行の移行は行わず、`asr_worker::SETTINGS_VERSION = 1` も変更しない。
旧座標で作った開発・確認用の行は、自動skipを避けるため既存の削除操作後に作り直す。

## 変更ファイル

| 範囲 | ファイル |
| --- | --- |
| 抽出・整数PTS | `src/ai/transcript_audio.rs`, `src/ai/transcript_audio/tests.rs`, `src/video/media_timeline.rs` |
| snapshot | `src/transcript_display.rs`, `src/transcript_display/tests.rs`, `src/app/transcript_display.rs` |
| 共通一覧・音楽ビュー | `src/transcript_panel.rs`, `src/ui_music_panels.rs` |
| 再生metadata・回帰 | `src/video/mod.rs`, `src/video/decoder.rs`, `src/video/transcript_display_tests.rs` |
| nativeのテストfixtureのみ | `src/video/native_presenter/overlay_draw.rs`, `src/video/native_presenter/render_core.rs` |
| MCP | `crates/mcp-server/src/schema.rs`, `crates/mcp-server/src/tests.rs`, `src/agent_ipc/transcript.rs`, `src/agent_ipc/transcript_tests.rs` |
| 素材と説明 | `src/ai/transcript_audio/fixtures.md`, `testdata/timeline-origin/README.md`, `testdata/timeline-origin/transcript-origin-video.ts` |
| 現行の設計説明 | `docs/video-timeline-origin-plan.md` §5, `docs/video-architecture.md`, `docs/architecture-overview.md`, `docs/async-architecture.md`, `docs/mcp-integration-plan.md`, 本報告書 |
| 削除 | `src/video/transcript_time.rs`, `src/video/transcript_time/fixtures.md`, `src/video/transcript_time/fixtures/origin-video.ts`（共通testdataへ移動） |

旧TS fixtureはbytesを変えず共通testdataへ移し、生成手順・SHA-256・期待値をREADMEへ移した。
snapshotのテスト入力の再生位置も同じメディア時刻へ直し、同じ行を強調する。
detachedの述語・viewport・窓の遷移は変更していない。

## 回帰検査

人工pulseを発話の時刻の代わりに使い、TS（小原点・大原点・折返し・interlace・複数track）・PS・MKVで、
抽出した16kHz PCMと再生用 `AudioRangeDecoder` のpulseが同じメディア時刻に来ることを検査する。
Whisperの認識精度や実発話の認識結果を評価する試験ではない。
旧T1のTSは先頭の音がメディア時刻0付近にあり、保存3秒の行がseek3秒へ進むことを検査する。
正の開始時刻のMKVは先頭10秒の無音と、その後のPCMの一致、第2trackの追加3秒の遅延を検査する。
負のsampleはfilterに直接渡し、0より前の半秒が捨てられることを検査する。

原点待ちの旧テストは、metadata到着前後で区間Arcと保存時刻が変わらない検査へ置き換えた。
区切り5のnative presenter再生成・再入場・隙間での直前行への追随、Manual、source変更、
音楽ビューのsession境界、通常wheel/Ctrl+wheelの検査は維持する。

### コマンドと結果

ビルド・テストはすべて次のwrapper経由。Gドライブ内のworkspace専用TEMPとbuild jobs=1は
開発用runner `target/asr-t1-chunk7b/run.py` で指定した。
lock置き場への書込みとwrapperのTEMP作成にはアクセス拒否の警告が出たが、指示どおり低優先度で続行した。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner asr-t1 -WorkDir G:\home\mimageviewer-asr-t1 -Command 'python -B -I target/asr-t1-chunk7b/run.py tests-final'
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner asr-t1 -WorkDir G:\home\mimageviewer-asr-t1 -Command 'python -B -I target/asr-t1-chunk7b/run.py transcript-media-final'
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner asr-t1 -WorkDir G:\home\mimageviewer-asr-t1 -Command 'python -B -I target/asr-t1-chunk7b/run.py mcp'
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner asr-t1 -WorkDir G:\home\mimageviewer-asr-t1 -Command 'python -B -I target/asr-t1-chunk7b/run.py mcp-final'
```

| 内部で実行するコマンド | 結果 |
| --- | --- |
| `cargo nextest run -j 1 -p mimageviewer --lib -E 'test(/transcript\|timeline\|audio_track\|media_timeline/)' --no-fail-fast --success-output final` | 257件中256成功、1件失敗、253.618秒。失敗は追加MCPテストのfixture名が分類対象外の `.mka` だったため。`.mkv` へ訂正して下の1件を再検査。初回の別の3失敗（snapshotの旧座標・連続seekのcoalescingを無視した2テスト）は入力・testの訂正で解消 |
| `cargo nextest run -j 1 -p mimageviewer --lib -E 'test(/transcript_media_seconds_are_returned_and_filtered_without_origin_conversion/)' --success-output final` | 1件成功、0.361秒。変更は追加testのfixture名と整形のみ。成功済み256件を再利用し、対象257件を確認済み |
| `cargo nextest run -j 1 -p mimageviewer-mcp --lib --no-fail-fast --success-output final` | 初回77成功・3失敗。2件は既知のnamed pipe OS error 5、1件は旧時刻文言を要求するtestの残り。後者を新契約の検査へ訂正 |
| `cargo nextest run -j 1 -p mimageviewer-mcp --lib -E 'not test(/real_pipe_cancel_reaches_core_while_stdio_stdout_is_blocked\|real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld/)' --no-fail-fast --success-output final` | 78件成功、既知の2件を除外。15.494秒 |
| `python scripts/check_ui_glyphs.py` | 成功、危険なグリフ0 |
| `cargo fmt`, `cargo fmt --check`, `git diff --check` | 成功 |

表示のPNGは更新なし。`transcript_panel_dark/light`、`native_transcript_panels_dark/light`、
`music_transcript_panels_dark/light` の既存PNGとの比較が成功した。
source切替、Manual、native presenter再生成、非表示からの再表示、音楽ビューの隙間への追随、
nativeの通常wheel/Ctrl+wheel、metadata前のseek、実音声track切替の検査も成功。
本体のログは `target/asr-t1-chunk7b/tests-final.log` と `transcript-media-final.log`。

MCPの実測値: tools/list最大 **43,873文字**、512文字の余裕込み **44,385文字**（上限48,000）。
最長descriptionは `list_folder` の **667文字 / UTF-16 667 units**（目標2,048・上限4,096以内）、
instructionsは **852文字 / UTF-16 852 units**。全権限・プロトコル・IDの組合せ、frame上限、データ応答40,000文字の
境界とdiscovery予算が他応答へ漏れない検査も成功。ログは `target/asr-t1-chunk7b/mcp-final.log`。

### 確認用ビルドの試行

AGENTS.mdの実行挙動変更時の規則に従い、同じrun-heavy経由で
`powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-dev.ps1 -PreserveRuntime` を実行する。
`-PreserveRuntime` は、このworktreeの確認用exeが実行中でも終了させないための指定。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner asr-t1 -WorkDir G:\home\mimageviewer-asr-t1 -Command 'python -B -I target/asr-t1-chunk7b/run.py build-dev'
```

**全5exeのCargoビルドは成功、runtime配置の素材検査でscriptはexit 1**。
core 33分46秒、remote 2分42秒、EPUB worker 1.46秒、MCP adapter 46.37秒、music-video editor 1分08秒。
共有の全件テスト・heavy枠待ちを守って実行した。ログは `target/asr-t1-chunk7b/build-dev.log`。

原因は既存 `vendor/miv-sound/VERSION` が **`0.15.0-miv.7`** のままなのに対し、取り込んだmasterの
`config/miv-sound.rs` が **`0.17.0-miv.1`** を要求すること。
`scripts/effetune-distribution.ps1:147` の検査で停止した。文字起こしの時刻変更に由来するcompile失敗ではない。
このvendor素材は変更せず、再試行・検査の迂回は行わない。FFmpeg/VCRTのcopy後、mIV Soundの配置と最後のPE依存検査の前で停止しており、
runtime一式の準備完了とは扱わない。要求版の承認済み素材を揃えてからbuild-devを再実行する必要がある。

生成済みexeは `target/dev-runtime/` の `mimageviewer-core.exe`、`mimageviewer-remote.exe`、
`mimageviewer-epub-pdf.exe`、`mimageviewer-mcp.exe`、`mimageviewer-musicvideo.exe`。
EPUB workerは既存の一致するartifactを再利用した。製品exeの起動は行っていない。

## 設計書との関係

時刻の契約は更新済みの計画書に従う。区切り3の旧報告にあるraw PTS変換と末尾の制約は当時の記録として残し、
現在の契約は本報告と更新した設計文書で示す。保存済み行の移行・設定版変更・再生側の挙動変更は行わない。

計画書§8.3末尾の「時刻が変わるのはコンテナの開始が0でないMKV・MP4だけ」は、対象を狭く書いている。
旧抽出は形式によらずformatのstartを引いていたため、O_eff=0かつformatのstartが非0の他形式も変更対象になる。
例えば同梱 `encoder-delay.mp3` はffprobeでformat start・最初のdecode frame PTSとも **0.023021秒**。
新契約ではこのPTSを0へ付け替えず、メディア時刻0からの先頭無音として扱う（この差は契約からの帰結）。
これは「原点はMediaTimelineだけ、TS/PS以外はO_eff=0」という決定に従う変更で、追加の原点補正は行わない。
計画書本文は設計担当の更新対象として残す。確認コマンド:

```powershell
ffprobe -v error -select_streams a:0 -show_entries format=start_time:frame=pts_time -read_intervals '%+0.05' -of compact testdata/timeline-origin/encoder-delay.mp3
```

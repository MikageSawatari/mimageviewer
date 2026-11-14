# ローカル文字起こし T1 区切り3 実装・検証記録

2026-11-14。基準commit `649a677a2`。作業場所は `G:\home\mimageviewer-asr-t1` のみ。
コミットしていない。製品exeは起動していない。

設計担当の2026-11-14の決定を反映して再開した。時刻変換はT+container_startに固定し、
既存seekの制限は再生側の既存問題としてT1の範囲外に残す。
以下は実装内容と検証記録。確認用ビルドの結果は末尾に記載する。

## 時刻の対応と既存のseek制限（§Cの設計決定）

コード調査で、ローカルのnative動画・音声ファイル・動画の音声モードはすべてraw PTS秒を使うことを確認した。

- decoderは `input.duration()` をそのまま秒へ換算して `VideoInfo.duration_secs` に入れる。
- video frameとaudio resamplerはraw PTSを保持し、`VideoPlayer::position()` はclockの秒を返す。
- demuxのseekはtarget秒をAV_TIME_BASEへ換算して `av_seek_frame(-1)` に渡す。
- 既存bookmark/pin/chapter/jumpもこの再生秒へ無変換で入る。
- nativeの行clickもmusicの行clickも既存経路から `VideoPlayer::seek()` へ合流する。
- しかし `VideoPlayer::clamp_seek_target()` は、targetを **`[0, duration_secs - 0.1]`** に制限する。

文字起こしの時刻Tをraw PTSへ変換する式は `T + container_start` だが、
コンテナのdurationが原点を含まない長さの場合、正しいraw targetが既存の上限を超える。
この場合、正しい変換結果を渡してもseekの中で前の位置へ切り詰められる。

### 同梱FFmpegでのメタデータ確認

4秒の合成MPEG-TSを2種類作り、**同梱FFmpeg `n7.1.5-16-g9a4bb2c579-20260816`** の
`avformat_open_input` / `avformat_find_stream_info` でmetadataを読み取った。
FFmpeg 9 CLIでのprobe値だけに依存した判断ではない。

| 合成素材 | container start O | duration D | 文字起こしT | 正しいraw target O+T | 既存clamp式適用後 | 文字起こしへ逆換算 |
| --- | --- | --- | --- | --- | --- | --- |
| AAC音声TS | 1.400000 | 3.818667 | 3.000000 | 4.400000 | 3.718667 | 2.318667 |
| MPEG-2映像＋AACのTS | 1.578667 | 4.021333 | 3.000000 | 4.578667 | 3.921333 | 2.342666 |
| 既存 `offset_tracks.mka` | 10.000000 | 14.000000 | 3.000000 | 13.000000 | 13.000000 | 3.000000 |

単位は秒。TS動画では3秒の行が、計算上 **約0.657334秒手前** へ移る。
MKAの既存fixtureはduration=14でraw 13が範囲内なので、この問題が当初の回帰だけでは見えない。
負の原点に対する既存の0下限も、helperが負値を保持できることとは別の制約である。

ここでの表は **同梱ライブラリのmetadata実測＋現行ソースのclamp式による計算**。
再開後は同じTSをテストfixtureとして保存し、実decoderのVideoInfoから変換して
`VideoPlayer::seek()`がclamp済みtargetを発行する回帰も追加した。
製品exeでの実再生ではなく、ウィンドウ・音声deviceを開かないtest transportでの検査である。

probeは同梱 `libavformat/avformat.h` のAVFormatContext公開prefixを用いる開発用Pythonで、
start_time/durationのfield offsetはx64で96/104。major version 61を検査し、終了時は入力をcloseする。
UIにprobeを追加したわけではない。

証跡は `target/asr-t1-chunk3/probe-native-time.py` と `native-time-probe.log`。
入力は同directoryの `origin-range.ts` / `origin-video.ts` と既存fixture。開発用生成物はgit対象に入れていない。

```powershell
ffmpeg -hide_banner -loglevel error -n -f lavfi -i sine=duration=4:sample_rate=48000 -c:a aac -f mpegts target/asr-t1-chunk3/origin-range.ts
ffmpeg -hide_banner -loglevel error -n -f lavfi -i testsrc2=size=64x64:rate=5:duration=4 -f lavfi -i sine=duration=4:sample_rate=48000 -c:v mpeg2video -threads:v 1 -c:a aac -f mpegts target/asr-t1-chunk3/origin-video.ts
python -B -I target/asr-t1-chunk3/probe-native-time.py target/asr-t1-chunk3/origin-range.ts target/asr-t1-chunk3/origin-video.ts src/ai/transcript_audio/offset_tracks.mka
```

### 採用した範囲

設計担当は「変換はT+Oで正しい」「既存clampはシークバー・bookmark・chapterにも同じく効く既存問題」
と決定した。T1は文字起こしも既存seekと同じ制限を受ける。
clampの迂回、durationへの一律原点加算、ASR専用のclampは追加していない。
再生側の時刻・有効範囲の問題は別課題として扱う。原点未知のseek禁止とRemote対象外も維持した。
計画書§8.3の追記は設計担当の既存差分であり、この実装側では編集していない。

## 実装内容と説明文・ジャンプの型との差

- Appに共通の `TranscriptDisplayCache` を1つ追加。DB SELECTとファイル署名は単一の遅延起動workerが読む。
  UIはchannelとメモリだけを使い、epoch＋キーごとの要求世代で古い結果を捨てる。
  clear/invalidate後は旧Arcを外し、次の要求からLoadingにする。
  読み取りだけの再入場refreshは説明文と同じく旧値を保持し、同じキーの別panelを消さない。
  説明文より本文が大きいためentry上限は32、Appのstackを増やさないためcache自体はBoxで持つ。
- descriptor由来の `transcript_display_stale` を共通のmutation採用点へ接続。
  作成保存・削除・metadata取込・STORES汎用操作の確定通知からcacheを消す。
  索引・caption epoch・MCPには接続していない。
- snapshotは共通builderがkey、保存状態、job所属、Arc区間、保存track、applied track、作成日、時刻mappingから作る。
  別trackはappliedとの比較で毎回導き、独立した永続状態にしない。
  古い／別trackでも本文を出し案内を添える方式にした。既存の説明文が古い本文を残して警告を添えるため。
  queued/runningだけでは旧本文Arcを消さない。署名のほかconfig_versionの違いも古いとする。
- nativeには独立したlatest-value command slotを追加。markerの変更判定・音楽mode gateに従属しない軽量同期を置いた。
  source swapで空snapshotへ戻す。F12は同じoutputのままrender coreだけ再生成するため、
  source側の最新snapshotを再生成時の表示データprepareへ復元する。
  この復元漏れは独立レビューで発見して修正した。headlessのoutput再bind往復も検査対象とし、
  同じoutput内での実機F12往復は利用者の確認シナリオへ残す。
- 同じ共通rendererをnative左の3タブとmusic左の2タブから呼び、右には同snapshot由来の1行だけを出す。
  musicのtab/followは現在のviewer context＋sourceのpanel sessionが所有し、動画とは共有しない。
  nativeの既存tab保持動作は変えず、Transcriptのfollow/scrollだけを閉鎖時に破棄する。
- 長文は常時全文を折り返す。320pt前後で1行省略にすると大部分が隠れ、読むたびhoverが必要になるので採らなかった。
  既存jumpは全行描画であり、そのままは使わず、共通bodyにFenwick高さ索引＋show_viewportを置いた。
  可視行だけlayoutし、測定した高さはpass末尾にまとめて採用。手動pauseはoffset変化で推測せず、実入力で判定する。
  幅/font変更は高さとanchorだけを更新し、manual pauseを勝手に解除しない。
- `VideoInfo` には従来container原点が公開されていなかったので、decoderが既に開くInputからmetadataとして追加した。
  選択trackの開始時刻を代用せず、UI再probeはしない。共通mappingのDefaultは未知で、情報到着前のseekを禁止する。
  Remoteのsession相対timelineは今回のmappingに通していない。
- 区切り2のP3は、同じ入力内の表示processed_msを前回とのmaxにし、次の入力開始で0へ戻す形を入れた。
  作成待ちの表示照合が毎frame全パスを正規化しないよう、queueと検索用キーを同じprivate ownerで更新する。

detachedの述語・viewport/host/placementの判断経路は変更していない。
F12の既存データ復元batchにsnapshotを足しただけで、独立レビュアーもこの範囲に合意した。
MCPと検索は範囲外で未変更。

## 変更ファイル

| 区分 | ファイル |
| --- | --- |
| 共通表示値 | `src/transcript_display.rs`, `src/transcript_display/tests.rs` |
| async cache | `src/transcript_display_cache.rs`, `src/transcript_display_cache/tests.rs` |
| 共通描画・perf | `src/transcript_panel.rs` |
| App統合 | `src/app.rs`, `src/app/transcript_display.rs`, `src/app/transcript_display_tests.rs`, `src/app/content_identity_detection.rs`, `src/app/native_video.rs` |
| native/movie/music | `src/ui_music_panels.rs`, `src/video/mod.rs`, `src/video/native_presenter/overlay_draw.rs`, `src/video/native_presenter/render_core.rs` |
| 原点metadata/変換 | `src/video/decoder.rs`, `src/video/transcript_time.rs`, `src/video/transcript_display_tests.rs` |
| TS合成fixture | `src/video/transcript_time/fixtures/origin-video.ts`, `src/video/transcript_time/fixtures.md`。96,820bytes、生成手順とSHAを記載 |
| 進捗P3 | `src/ui_dialogs/transcript_job.rs` |
| 登録・文書 | `src/lib.rs`, `docs/README.md`, `docs/video-architecture.md`, `docs/architecture-overview.md`, `docs/async-architecture.md`, `docs/ui-responsiveness.md`, 本書 |

## テストと検査の結果

コンパイルし得る確認はすべて次の入口を使った。CARGO_BUILD_JOBS=1、MSBUILDDISABLENODEREUSE=1、CMAKE_BUILD_PARALLEL_LEVEL=1。
ロック領域に書けないため、runnerの警告後BelowNormalで継続した。
全libを含む一連の広い確認には `-FullTest` も指定した。ソースを固定し、retryなしで実行した。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner asr-t1-codex -WorkDir G:\home\mimageviewer-asr-t1 -Command '<コマンド>'
# 全libを含む確認:
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner asr-t1-codex -WorkDir G:\home\mimageviewer-asr-t1 -FullTest -Command '<コマンド>'
```

| wrapper内のコマンド | 最終結果 |
| --- | --- |
| `cargo check -j 1 -p mimageviewer --bin mimageviewer-core` | 成功、58.44秒。以後の本体通常featureのコンパイルは下のintegrationと確認用buildでも行う |
| `cargo check -j 1 -p mimageviewer --lib --tests` | 成功、4分39秒 |
| `cargo nextest run -j 1 -p mimageviewer --lib -E '<表示snapshot filter>' --no-fail-fast --success-output final`（`UPDATE_SNAPSHOTS=1`） | **8件成功**、8.797秒。新規6PNG＋既存3PNG更新。通常比較でも対象379件と全libに含めて成功 |
| `cargo nextest run -j 1 -p mimageviewer --lib -E '<対象filter>' --no-fail-fast --success-output final` | **379件すべて成功**、109.035秒 |
| `cargo nextest run -j 1 -p mimageviewer --test ui_snapshot -E '<既存UI filter>' --no-fail-fast --success-output final` | **17件すべて成功**、16.324秒。音楽panel/HUDとnative HUDを通常比較 |
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-lib-full.ps1`（外側 `-FullTest`） | **17,726件実行：17,724成功、既知の環境制約2件失敗、78件skip**。931.989秒、14件slow、timeoutなし。全件exit100を成功とは扱っていない |
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-full.ps1 -SuppressCrashDialogs` | **未完了（exit101）**。launcher build.rsが内包用release exe 5本の不足で停止。workspaceテスト・doctest・後続vendor gateは未実行 |
| `cargo test -j 1 -p mimageviewer --lib transcript_panel::tests::transcript_panel_long_list_perf -- --ignored --exact --nocapture` | **1件成功**、最終計測0.64秒。下表 |
| 同梱FFmpeg metadata probe（前述Python） | 成功。3素材の値を採取し、TSの実decoder＋player transport回帰も379件と全libで成功 |
| `cargo fmt`（引数なし） / `cargo fmt --check` / `git diff --check` | 成功 |
| `python scripts/check_ui_glyphs.py` | 成功、危険なグリフ0件 |

filterの実値は次のとおり。

```text
表示snapshot filter:
test(/transcript_panel_snapshots|native_transcript_panels_.*_snapshot|music_transcript_panels_.*_snapshot|native_video_adjustment_(tone|lut|anime4k)_panel_snapshot/)
対象filter:
test(/transcript|caption_display|ui_music_panels::|native_presenter::.*(jump|metadata|side_panel|transcript|adjustment)|native.*command|history_transition_storage_keeps_app_stack_footprint_bounded|metadata_transfer::|rename_key_migration::/)
既存UI filter:
test(/music_locked_panel|music_touch_panel|tracks_hud_(native|music)|normalize_hud_navigation_(native|music)|music_hud_layout/)
```

対象確認はcache世代/epoch/refresh、A→Bの古い読込み、削除・取込・保存・STORES失効、
再bind往復、applied track切替、job対象と進捗の単調性、共通rendererのclick/wheel/再開/resize、
非0原点の実素材metadataとseek制限、既存caption/metadata/改名、およびApp stack上限を含む。

全libで失敗した2件は次のとおり。どちらも `Os { code: 5, PermissionDenied }` で、
既に利用者がsandbox外で再確認済みの同じnamed pipeの環境制約として扱った。
この最終ツリーでsandbox外の再実行はしていない。新規の失敗はなかった。

- `agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry`
- `agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers`

全libのsource IDは `a3a0f79c1b4e1ef8e1225d4d34785eb89e74d02d9dad37e50e54b3d6491c06da`。
JUnitとstdout/stderrは `target/test-lib-full/runs/20261113T230805276Z-b4fb7abe/`。
失敗実行なので成功台帳への登録・再利用はない。実行後の追記は本報告書だけで、本体ソースは変更していない。

workspace gate不足の5本は `target/release/` のcore、remote、EPUB worker、MCP adapter、music-video editor。
別profileのexeで代用せず、release buildの追加は今回行っていない。

途中の検証では、chronoの直接依存誤認を既存日時helper利用へ、testのfs_cache添字をgetへ、
snapshot fixtureの旧egui APIを現行APIへ修正した。複数harnessのSnapshotResultsは1つへ集約した。
それぞれ修正後の実行結果を上表へ記載しており、compileのみの結果をテスト成功件数には含めていない。

詳細ログは `target/asr-t1-chunk3/` の
`check.log`, `check-tests.log`, `snapshots-update.log`, `focused-final.log`, `ui-integration.log`,
`workspace-gate.log`, `lib-full.log`, `render-perf-final.log`, `native-time-probe.log`。
初回checkは `target/asr-t1-chunk2/chunk3-check.log`。

## 5,000行のperf

共通rendererをheadless egui Harnessで実行した。320×460pt、折返し日本語の5,000区間。
準備8stepの後、再生位置を41行ずつ進める120stepを測った。
`transcript/display_render` のJSONLから集計し、準備完了markerで初期表示を分離した。
writerの終了待ちはignored測定test内だけで行い、通常UIへ待機を加えていない。

| 指標 | 値 |
| --- | --- |
| 準備中の描画サンプル | 11 |
| 準備後の描画サンプル | 120 |
| 準備後の中央値 | **3.8895ms** |
| 準備後の95パーセンタイル | **4.8561ms** |
| 準備後の最大 | **11.0993ms** |
| 準備を含む最大 | **72.1592ms** |
| 1回のlayout行数の最大（準備を含む） | **15行 / 全5,000行** |

最初の測定は初期を含む131sampleの中央値3.8442ms、p95 5.335ms、最大73.2624msだった。
この最大を隠さず、最終測定では初期と準備後を区別した。
全区間を毎frame描く処理にはなっていない一方、初期準備には上記のピークがある。
これはtest profile（unoptimized＋debuginfo、rustc 1.99.1）のCPU描画処理の実測で、
実アプリのnative GPU描画・動画再生中のframe時間やFPSを測った値ではない。
他セッションの負荷も排除していない。

## PNGの生成・目視

次の**6枚を追加、3枚を更新**した。親と独立レビュアーの双方が原寸で9枚すべてを目視した。
タブ、案内、時刻、本文の折返し、現在行強調、右summary、既存補正操作に欠け・重なり・tofuは見当たらなかった。
Lightのmain UIでも既存の暗いpanelの配色を維持し、main側へのテーマ漏れがないことを確認した。
下端の部分表示はscroll viewportによる通常のclip。

| `tests/snapshots/` のファイル | 扱い | サイズ |
| --- | --- | --- |
| `transcript_panel_dark.png` | 新規 | 320×460 |
| `transcript_panel_light.png` | 新規 | 320×460 |
| `native_transcript_panels_dark.png` | 新規 | 640×650 |
| `native_transcript_panels_light.png` | 新規 | 640×650 |
| `music_transcript_panels_dark.png` | 新規 | 640×460 |
| `music_transcript_panels_light.png` | 新規 | 640×460 |
| `native_video_adjustment_tone_panel.png` | 更新（3タブ化） | 340×650 |
| `native_video_adjustment_lut_panel.png` | 更新（3タブ化） | 340×650 |
| `native_video_adjustment_anime4k_panel.png` | 更新（3タブ化） | 340×650 |

「無い」の案内と右1行は、native/musicのdark/light双方のgeometry testでも確認した。
その他の状態はsnapshotの純粋テストと共通案内のテストで検査した。

## 設計との対応と残る確認範囲

- §Cは2026-11-14の設計決定どおり。変換はT+container_startとその逆を1か所に置き、
  既存の非0原点の末尾seek制限は**再生側の既存問題として別に扱う**。
  TS回帰は「正しい変換結果」と「既存の制限」を両方固定する。
- 既存captionと同じく古い／別trackの本文を残して案内を添える。job状態は本文とは別の値で表す。
- 固定高のshow_rowsでは全文折返しができないため、同じ共通bodyのshow_viewport＋高さ索引を採用した。
- detachedの凍結対象の判断経路は変更していない。F12は既存の再生成データbatchへsnapshotを追加しただけ。
  output再bindは自動確認し、同じoutput内のrender core再生成への保持値復元はコード確認した。
  **同じoutput内の実機F12往復・実再生・GPUの描画は未実施**。
  製品exeの起動禁止を守り、下の利用者確認に残した。
- 計画書の本文は設計担当が更新した§8.3の差分を保持し、実装担当からは変更していない。
  その他の未承認の設計変更はない。Remote、MCP、検索、索引の一般化は未変更。

## 利用者確認用ビルド

`run-heavy` 経由で次を実行し、**exit0で完了**した（08:29:02〜08:43:07 JST）。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-dev.ps1 -PreserveRuntime
```

| 段階 | 結果 / Cargoの所要時間 |
| --- | --- |
| core | 成功、13分17秒 |
| remote service | 成功、0.52秒 |
| EPUB PDF worker | 成功、0.45秒 |
| MCP adapter | 成功、0.30秒 |
| music-video editor | 成功、30.37秒 |
| VCRT/PE検査 | 成功、runtime=4、PE=5 |

- core: `target/dev-runtime/mimageviewer-core.exe`
- サイズ: **394,026,496 bytes**
- SHA-256: **`c7dcca34d243a7bacbd0dac51f14966a508e050ce0d8641ed6b43eae046fe41b`**
- build log: `target/asr-t1-chunk3/build-dev.log`
- VCRT証跡: `target/vcrt-pe-reports/dev-runtime.json`

worker、モデルpack、DB schema、protocolは今回変更していない。backupへの書込みもしていない。
コミットせず、このworktreeに変更と確認用バイナリを残した。

製品exeは起動していない。利用者がこのworktreeで起動するコマンドは次のとおり。

```powershell
Set-Location G:\home\mimageviewer-asr-t1
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

この通常featureのcoreは、既定で実際の `%APPDATA%\mimageviewer` を使い、設定やDBを更新し得る。
single-instance mutexが共通なので、installed版・tray常駐版を先に終了する。
確認対象は動画の左3タブと音楽の左2タブ、右1行、行clickのseekと強調、手動scroll停止/戻る、
track切替の案内、保存/削除/取込後の更新、F12往復後の本文再表示。
非0原点TSの末尾は上記の既存seek制限を受けることを前提とする。


## 独立レビュー後の修正

2026-11-14、基準commit `e34a28713`。P2「一覧以外の入力でも自動送りが止まる」を修正した。
今回の変更ファイルは `src/transcript_panel.rs` と本報告書のみ。コミット・製品exeの起動はしていない。

### 原因と対応

従来はポインタの現在位置とglobalなdrag/wheelの有無で判定していたため、
一覧外のwidgetが保持するdragが本文を横切った場合や、前面layer上のwheelもManualへ移していた。

判定をScrollArea描画後の1か所へ移し、実際の入力所属へ合わせた。

- dragは同梱eguiのScrollAreaが登録した本文Responseの `dragged_by(Primary)` と移動の有無で判定する。
  pressの所属を再実装せず、eguiが保持するwidgetの所有を使う。
- スクロールバーも実Responseの押下/clickで判定し、UIの有効状態・最上位layer・clip内の位置を確認する。
- wheelはUIの有効状態、最上位layer、ScrollAreaの本文/バーの矩形とUIのclip内であることを確認する。
- `ScrollAreaOutput` にはResponseが直接含まれないため、返されたidから `Context::read_response` で
  同梱ScrollAreaの本文（`id.with("area")`）/縦バー（`id.with(1_usize)`）のResponseを読む。
  この対応は共通helper内だけに閉じ、handler-level testで実際のegui入力を検査する。
- offsetの変化からの推測、新しい状態、UI I/Oは追加していない。動画と音楽は同じ修正を使う。

### 回帰テストと結果

6件のhandler-level testを追加し、次を確認した。

- 一覧外の実drag widgetから始めたdragが本文を横切ってもFollowingのまま（drag所有idも検査）。
- 本文と重なる前面Area上のwheelでFollowingのまま（最上位layerが異なることも検査）。
- 本文上のwheel、本文で始めたdragではManualへ移る。
- 縦スクロールバーの押下ではManualへ移り、前面Areaに覆われたバーの押下ではFollowingのまま。
- 無効なUI、clip外のwheelではFollowingのまま。

コンパイル・テストは従来どおりrun-heavy経由で実行した。
CARGO_BUILD_JOBS=1、MSBUILDDISABLENODEREUSE=1、CMAKE_BUILD_PARALLEL_LEVEL=1。
ロック領域への書込み不可の警告後はBelowNormalで継続した。retry・期待値変更はしていない。

| wrapper内のコマンド | 結果 |
| --- | --- |
| `cargo nextest run -j 1 -p mimageviewer --lib -E 'test(transcript_panel::tests::)' --no-fail-fast --success-output final` | **13件すべて成功**、5.346秒（compile 4分28秒）。新規6件＋既存7件。perf測定のignored testは除外 |
| `cargo nextest run -j 1 -p mimageviewer --lib -E '<対象filter>' --no-fail-fast --success-output final` | **385件すべて成功**、134.909秒。filterの実値は前述の対象filterと同じ。13件もこの385件に含む |
| `cargo fmt`（引数なし） / `cargo fmt --check` | 成功 |
| `python scripts/check_ui_glyphs.py` | 成功、危険なグリフ0件 |
| `git diff --check -- src/transcript_panel.rs docs/local-asr-t1-chunk3-report.md` | 成功 |

実行入口は `run-heavy.ps1 -Owner asr-t1-codex -WorkDir G:\home\mimageviewer-asr-t1 -Command 'python -B -I target/asr-t1-chunk3/p2.py'`。
ログは `target/asr-t1-chunk3/p2-panel.log` と `p2-focused.log`。
最初のwrapper呼出しはfilterの括弧のshell引用が不足し、テスト起動前に終了したため、
引数配列で渡す開発用Pythonへ直して上記2段を実行した。

既存の自動送り停止/再開・source切替・幅変更、およびnative/musicの表示snapshot比較も成功した。
PNGの更新はない。今回の修正で計画書との食い違いは増えていない。

利用者指示に従い、build-devは今回は実行していない。前節の確認用バイナリとSHAはこのP2修正を含まない。
確認用buildは区切り4の最後に作り直す。

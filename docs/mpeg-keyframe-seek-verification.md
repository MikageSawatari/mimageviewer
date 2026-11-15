# §1.785 実装の検証記録

実装元: `6204f6c2d`、ブランチ `fix-785-mpeg-seek`。設計の正本は
[mpeg-keyframe-seek-plan.md](mpeg-keyframe-seek-plan.md) 第4版。
2026-11-15〜16、製品アプリは起動せず、headlessのFFmpeg復号で検証した。
独立実装レビューと実機確認は未実施。コミットは設計担当が行う。

## 変更ファイル

| ファイル | 変更 |
| --- | --- |
| `src/video/keyframe_seek.rs`（新規） | 対象demuxer、整数換算、A or B、back-off、保持列、量の予算、取消、結果・計装を所有 |
| `src/video/decoder.rs` | 共通seekと読み口、Abortedの破棄と要求公開待ち、Flush後の保持映像preload、回帰試験の接続 |
| `src/video/clock.rs` | cfg(test)のseek取消・公開前競合の制御点 |
| `src/video/mod.rs` | 新規moduleの登録（試験支援はcfg(test)） |
| `src/video/thumbnail.rs`, `tile_thumbnails.rs` | 共通seek・読み口と取消、EOF drain、画像IDの回帰試験 |
| `src/video/screenshot.rs` | 共通seek・読み口、240上限撤去、目標到達・EOF・read errorの区別、画像IDの回帰試験 |
| `src/video/seek_strip.rs` | TS/PS用TimeGridReasonとdemuxer方針 |
| `src/video/seek_strip_thumbs.rs` | 索引・warmupより前のTimeGrid選択、共通seek・読み口、軸と画像IDの回帰試験 |
| `src/video/seek_strip_batch.rs` | dev-tools側の新しいTimeGridReasonの扱い |
| `src/video/clockless_transcode.rs` | Remoteの共通seek・読み口、MP2のInvalidDataを記録して継続、映像ID・音声出力の回帰試験 |
| `src/video/keyframe_seek_tests.rs`, `keyframe_seek_test_support.rs`, `mpeg_keyframe_seek_decoder_tests.rs`（新規） | 実素材・独立正解表、packet一致、予算・取消、再生・音声・step・supersede・backpressure試験 |
| `scripts/ui-smoke/generate_mpeg_keyframe_seek_fixture.py`（新規） | 素材生成、独立全復号・packet表・特殊目標の検査 |
| `testdata/mpeg-keyframe-seek/`（新規）, `.gitignore` | 10素材・各正解JSON・READMEと追跡許可 |
| `docs/video-architecture.md`, `docs/next-release-backlog.md` | TS/PS seekの契約、785の状態、upscale番号付けの795を起票 |
| `docs/mpeg-keyframe-seek-verification.md`（本書、新規） | コマンド・計装・設計前提との差・未検証範囲の引継ぎ |

## 素材と正解表

生成コマンドは `python scripts/ui-smoke/generate_mpeg_keyframe_seek_fixture.py`。
システムのFFmpeg / ffprobe 9.0.2で先頭から全復号し、画素に埋め込んだ表示順ID、
`best_effort_timestamp`、映像packetのPTS / DTS / key / hashをJSONに保存した。
seek後の時刻から正解を推定する方法や製品のRustコードは使っていない。
PSのB条件と反例の検査には同梱DLLも使用する。手順と検査結果の保存先は
[素材README](../testdata/mpeg-keyframe-seek/README.md)を参照。

96×64、通常12秒、長GOPのみ18秒。10素材の映像・音声合計2,218,201 bytes、
正解表JSON合計2,054,771 bytes。既存のtimeline-origin素材・試験は変更していない。

## 計装

新規回帰28件が通過した実行から採録。読取量は全backの`AVIOContext.bytes_read`増分で、
packet payload量とは異なる。目標は通常5.4秒、long-gopのみ17.4秒。
試行回数は実際に呼んだseekの回数（先頭へのseekを含む）。

| 素材 | 結果 | 試行 | 読取bytes | seek ms | 取消→Aborted ms |
| --- | --- | ---: | ---: | ---: | ---: |
| h264-small.ts | Verified / back 1 | 1 | 76,628 | 2.5932 | 0.0971 |
| mpeg2-large.ts | Verified / back 1 | 1 | 135,020 | 0.2038 | 0.0185 |
| hevc-open.ts | Verified / back 1 | 1 | 142,164 | 0.4622 | 0.0297 |
| mpeg2-missing-pts.mpg | Verified / back 1 | 1 | 65,536 | 0.2117 | 0.0163 |
| mid-gop.ts | Verified / back 1 | 1 | 76,628 | 1.7752 | 0.0916 |
| mid-gop.mpg | Verified / back 1 | 1 | 65,536 | 0.1613 | 0.0178 |
| long-gop.ts | Verified / back 8 | 4 | 648,304 | 3.3284 | 0.0236 |
| control.mp4 | Direct | 1 | 0 | 0.0290 | 0.0193 |
| pts-delayed.ts | StartOfStream | 3 | 404,308 | 0.9736 | 0.0134 |
| short-key-gap.ts | Verified / back 1 | 1 | 135,020 | 0.2074 | 0.0160 |

取消は取消確認直後に発生させ、次のFFmpeg呼出し後に検知する試験。
6並列・キャッシュの影響を含む単発の観測値で、実機での最大遅延の保証値ではない。
FFmpeg内部の1呼出し中のI/Oは中断できないという設計上の制限が残る。
元ログ・集計は`target/785-verification/focused-new-pass.log`、`metrics.json`。

実PSのB条件はraw target=579000 tick、back 1で検査した。
JはPTS不明のkey 3個（DTS=498000 / 534000 / 579000）で、
最初のI frameは表示順ID=150、表示時刻498000 tick。
独立した先頭からの全復号の画・時刻と一致した。
長GOPのスクリーンショットでは、直前keyのID=360から目標ID=690まで
330 frameのprerollを通し、旧240上限を超えた位置の画を照合した。

## seek呼出しの照合

`rg -n 'av_seek_frame|avformat_seek_file|Input::seek|\.seek\(' src crates`で確認。
設計§3.2の6経路（再生、hover、タイル、stripの枠、スクリーンショット、Remote）を
共通`PacketReader`に接続した。表の外の追加対象となる製品映像seekは見つからなかった。
さらにvendor / targetを除くrepository全体のRust / C / C++も同じ式で検索した。
追加の一致は`tools/seek_strip_probe/src/main.rs`の実装前提を測る診断用seekで、
製品workerではないため変更していない。検索結果は
`target/785-verification/seek-audit.txt` / `seek-audit-repository.txt`。

- `audio_decode.rs`: 音声のみで対象外。
- `upscale/job.rs`: 設計で明示的に対象外。コード・既存試験は変更していない。
- stripの索引warmup: TS / PSでは呼出し前にTimeGridを選び、索引の列挙とwarmupを省く。
- 残りはファイルI/Oのseek、上位の要求API、試験・probe。逐次読取だけのD3D probe等は変更不要。

## コード上で判明した設計前提との差

- stripの低水準索引列挙関数自体は、合成索引を注入する既存timeline-origin試験でも使われる。
  既存試験を変更せず、製品の軸解決関数で先にdemuxerを判定して列挙・warmupを省いた。
- Remoteの音声decoderはseek後最初の不完全なMP2 packetの`InvalidData`を即座に返していた。
  独立DLL probeではPSの旧Direct seekも同じ失敗になり、続くpacketは正常だった。
  通常再生側と同じく音声decoderの所有箇所で記録して読み進める（Remoteでは`InvalidData`だけ）。
  音声の採用時刻、trim、DSP、resamplerは変更していない。回帰試験は最初の映像IDと
  AAC stream・音声frame・復号後の非ゼロPCMまで検査する。
- FFmpegは不正な正のstream indexをseek前に検証せずアクセス違反になり得るため、
  共通のsafe Rust入口で範囲外をEINVALとして返す。正しい要求の挙動は変えない。

## 自動検証と確認用ビルド

すべてのCargoビルド・テストは次のrunner経由で実行する。本体lib全件は実行しない。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner fix-785 -WorkDir G:\home\mimageviewer-785 -Command "<下記コマンド>"
```

環境: `MSBUILDDISABLENODEREUSE=1`、`BuildInParallel=false`、
`TEMP` / `TMP`はworktree内の`target/785-verification/tmp`。
runnerがロックを書けない場合の警告と低優先度・6並列への移行は指示どおり受け入れた。
既存full-testロックの待機は迂回していない。

| コマンド | 結果 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 最終ソースで成功、2分02秒。ログ`check-final.log`（初回成功3分16秒） |
| `cargo nextest run -p mimageviewer --lib -E 'test(/keyframe_seek\|mpeg_strip\|mpeg_timestamp\|remote_mpeg/)' --test-threads 6 --ignore-default-filter --run-ignored default --retries 0 --no-tests fail --no-fail-fast` | 28件成功・失敗0、20.433秒 |
| 動画関連の既存試験と追加backpressure試験（下記） | 346件成功・失敗0、10.939秒。test profileのコンパイル6分22秒 |
| `cargo check -p mimageviewer --bin mimageviewer-core --features dev-tools` | 成功、2分41秒。ログ`check-dev-tools.log` |
| `python scripts/check_ui_glyphs.py` | 不足字形0 |
| `cargo fmt` / `cargo fmt --check` | 成功（引数なしfmtを実行後にcheck） |
| `.\scripts\build-dev.ps1 -PreserveRuntime` | 他worktreeのnative build終了後、通常構成で初回ビルド中 |

最初の新規試験実行は28件中26件成功、2件失敗（不正stream indexによるアクセス違反、
Remote PS音声のInvalidData）。上記の修正後に28件すべて成功した。
その後は試験1件とコメントだけを追加し、実際のFFmpeg復号が保持映像から目標に届いて
音声backpressureを解消する経路を既存動画試験とともに検証した。
新規29件は両実行を通じて成功しており、28件と346件には重複がある。
関連実行の正確なコマンドは次のとおり（runner経由、ログは
`target/785-verification/focused-related.log`、run ID
`9a92c53e-4c21-4be8-89cc-8b32b622cab9`）。検証対象のRust差分とCargo入力の
SHA-256は`target/785-verification/source-snapshot.json`に保存した。

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/video::decoder::|video::thumbnail::|video::tile_thumbnails::|video::seek_strip::|video::seek_strip_thumbs::|video::screenshot::|video::clockless_transcode::|video::upscale::|timeline_origin/)' --test-threads 6 --ignore-default-filter --run-ignored default --retries 0 --no-tests fail --no-fail-fast
```

初回のnative turbojpeg MSBuild失敗は、同じ出力先の単一nodeビルドと上記環境で解消し、
依存ソース・ビルド設定は変更していない。

確認用バイナリは通常profileのため、ユーザー自身が起動して確認する。
インストール版・トレイ常駐を終了してから次を実行する（single-instance mutexが共通）。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

`%APPDATA%\mimageviewer`の実設定・データを使用し、更新する可能性がある。
手動確認の中心は`mpeg2-large.ts` / `mpeg2-missing-pts.mpg`の末尾0.3秒前へのseekで、
画が以前の位置のまま残らず音声が続くこと、その位置で前後のframe stepが動くこと。
同じ末尾付近のhover / stripの枠が空にならないことも確認対象とする。
0.1秒刻み・key境界・各worker・Remoteの画像ID照合は上記の自動試験で実施済み。
今回の実装作業ではアプリの起動・実機UI入力は行っていない。

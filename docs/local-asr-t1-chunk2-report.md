# ローカル文字起こし T1 区切り2 実装・検証記録

2026-11-14。検収済み `59ed945dc` からの作業。実装・テストはCodex、設計・検収はClaudeCode Opus。
変更は `G:\home\mimageviewer-asr-t1` だけに残し、コミットしていない。
製品exeは起動していない。通常プロファイルの設定/DBとバックアップは操作していない。

## 実装範囲と判断

- `TranscriptJob` はAppに1つ。受付・待ち行列・次の1件の送信・Running/Stoppingを単独所有する。
  `Option<Box<TranscriptJob>>` とし、非稼働時のAppにjob本文を載せない。既存のAppサイズ上限は維持する。
  作成中の追加は説明文ジョブと同じく未処理queueへ追加し、同じqueue内の正規化キーを重複させない。
  説明文とは別owner、別channel、別プロセスで、互いのジョブ終了を待たない。
- 中止は説明文の「現在の推論後に止める」と異なり、指定どおり即時cancel+killする。
  抽出のinterrupt callbackとpipeの相手終了で待ちを解く。commit済みItemDoneとFinishedは中止後も採用し、
  FinishedまではStoppingのownerを残す。UIにJoinHandleやjoinを持ち込まない。
- 実行役はファイルの署名、保存済み行、音声probeと選択、後処理・保存を扱う。
  PCMの抽出/書込みだけをscoped threadへ置き、同時に結果を読む。
  変換だけが失敗した場合はendを送り、最後の窓と終端まで受け取り、暫定窓を保存せず次のbeginへ進む。
  wire・プロセス・モデル・GPU失敗はジョブの致命的失敗。再試行/再起動は追加しない。
- 音声一覧・decoder open/layout/discardは既存デコーダの部品を共用し、選択規則は
  `resolve_initial_audio_track` をそのまま呼ぶ。受付時に再生と共通の
  `saved_audio_track_choice_for_path` からメモリ上の選択を捕捉し、パスと一つの `TranscriptTarget` に持たせる。
  モデル案内・queue追加・worker送信までこの値を保持し、workerのprobe結果へ共通resolverを適用する。
  設定がDBへ保存される前に依頼しても、受付時点の次回再生と同じ選択になり、
  UI/workerでsettings DBを読む必要がない。受付後のトラック変更には追従しない。
- 抽出は評価と同じ `aresample=async=1:first_pts=0` と16kHz mono s16le。
  コンテナ原点を引いたPTSをfilterに渡す。1MiB上限の枠で流し、一時WAVは作らない。
  実際の長さは送ったPCM sample数が正本。container durationは途中の進捗のhintで、Endでは実長にそろえる。
- 復号と後処理の版は `CONFIG_VERSION = asr_worker::SETTINGS_VERSION`（現在1）で一元化。
  後処理の変更でもこの整数を上げる。後処理は評価の `clean(segs, None)` と同じ順・正規表現・定数。
  backreferenceを使わないRust regexに合わせ、loopだけUnicode scalar列で左端/短い単位優先/貪欲反復を実装した。
  stock/repeatの除去数を保存し、loopの短縮で別の本文の書換えはしない。
- `transcript.db` は設計どおり1本1行、segmentsはJSON tuple配列、mtimeは署名のms。
  `transcript_seq` とinsert/update/delete triggerがrev/indexed/削除印を管理する。
  未リリースのschema 1なので移行はない（検収側のコミット説明にも記載できる）。
  保存直前にBEGIN IMMEDIATEの中で署名を取り直し、相違はDiscarded、失敗で既存okは上書きしない。
  長さ0・重なる区間は区切り1のtransportと同じく正当として受け入れる。
- STORESにはuniqueなstoreとして登録した。表示失効の種類だけをdescriptorで宣言し、説明文の表示失効も移した。
  既存 `ai_captions_changed` の通知判定と `caption_epoch` / indexer_supervisorは維持した。
  文字起こし表示の印の受け手は区切り3。indexed/removed印を拾う役は今回存在せず、索引接続はT2。
- metadata転送はv14、従来版の読取りは維持。記載されたemptyは削除し、省略したsectionは触らない。
  import commitの変更も同じdescriptorから表示失効印を作り、既存の完了採用点へ渡す。
  unique storeの汎用copy/restore、rename/move、orphan cleanup、hard purgeへSTORES経由で入る。
  settings転送は設定値だけ、settingsバックアップ/復元はsettings.db familyとtags.dbが対象で、説明文本文も対象外。
  文字起こし本文もそれらへ追加せず、本文の持運びはmetadata転送を使う。
  metadataのattached DBは18件で、既存のSQLite上限20以内。
- 初回案内と削除の全phaseをmodalにし、長い作成・取得はmodelessとした。
  閲覧と並行する必要がある作成を全体modalにする案は採らず、Appがqueueと終了を単独所有して組合せを抑えた。
  取得status/進捗/cancel/再開の表示は既存 `model_addon` の部品をVLMと共用した。
  新しいKeyAction/既定shortcutは作らず、メニューcommand IDとkeymap-specの記録だけを追加した。

## 確認した設計上の差と範囲外

既存の再生側 `video/decoder.rs` は非0コンテナ原点を差し引かずraw PTSで扱う経路がある。
評価の抽出契約は「0 = container start」であり、今回の抽出はこの明示契約に合わせた。
再生側は今回変更していない。非0原点の動画での表示/seek接続は区切り3で照合が必要。
先頭音声だけを0へ詰めるのではなく、第2音声の遅延は維持する。

独立レビューで初回DB作成中の並行metadata exportが未公開の表を読む競合を発見した。
説明文にも同じproducer/consumer経路があるため、両storeの読取/exportが共有するschema公開境界を設けた。
version0だけ未公開として扱い、公開済み表の欠損や未知versionは失敗として返す。
DB破損の握り潰しや再作成・再試行は追加しない。

独立レビューでは、当初のsettings DBからの音声トラック選択が、再生のメモリ上の選択より古くなる点も修正した。
切替後・DB保存前でも同じ規則で選ぶよう、受付時のメモリ値を対象に持たせる構造へ変更した。
DBのcommit待ちやUIの同期読取りは追加していない。

最終監査では、1pass最大64件の受信でProgressが積み残されると、coalesceされたrepaint予約を
使い切り、後ろのcommit済みItemDone/Finishedまで到達しない可能性も確認した。
64件処理してownerが残る場合だけ次passを予約する。空queue/終端では予約せず、idleの連続起床を作らない。
Stopping中の200件のProgressと保存・終了を使う回帰を追加した。

全件libではAppが110048bytesとなって既存の110000bytes未満の上限を超えたため、
新jobの本文をBoxへ移した。上限の緩和や、ほかのApp状態の変更はしていない。
修正後はApp=109768bytesで、従来の上限を満たした。
全storeの実schemaを作るrestoreテストにもTranscriptDbのconstructorを追加した。
新storeのPKは既定BINARYで正しく、索引検査の条件は変更しない。
restoreのDB open回数はstore追加分だけ37→38（copy29 + origin1 + runtime8）とし、
候補1件/100件で同じ回数という性能条件を維持した。

左/右パネル、表示cache、MCP、検索は実装していない。UTF-8破棄数は入力ごとにログ/perfへ出し、DBには保存しない。

入力のmetadata自体が読めず作業開始署名を取れない場合は、説明文と同じくFailed件数とログだけを残し、
署名を捏造したfailed行は保存しない。署名が取れた後のprobe/抽出失敗は通常どおりfailed行の保存対象とする。
これは「失敗時はfailed行」の境界で、既存okはどちらの場合も保持する。

## 検証記録

ビルド・テストはすべて次の入口を使う。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner asr-t1-codex -WorkDir G:\home\mimageviewer-asr-t1 -Command '<コマンド>'
```

Rust/nativeはCARGO_BUILD_JOBS=1、MSBUILDDISABLENODEREUSE=1、CMAKE_BUILD_PARALLEL_LEVEL=1。
テストのPATHにvendor/ffmpeg/binとvendor/vcrtを加える。runnerが共有full-test lockを検出した場合は解除を待つ。
ロック領域に書けない場合は指定どおり警告後BelowNormalで継続する。

| コマンド（wrapper内） | 結果 |
| --- | --- |
| `cargo check -j 1 -p mimageviewer --bin mimageviewer-core` | 初回成功78秒、監査前成功68秒、poll修正後成功23.35秒、Box化後の最終確認も成功24.81秒 |
| `cargo check -j 1 -p mimageviewer --features dev-tools --bin transcript_extract_bench` | 成功85秒 |
| `cargo nextest run -j 1 -p mimageviewer --lib -E 'test(ui_dialogs::transcript_job::tests::)' --no-fail-fast` | 起床継続修正後、新規3回帰を含む11/11成功、4.772秒（compile 5分34秒） |
| `cargo nextest run -j 1 -p mimageviewer --lib -E 'test(/transcript\|asr_setup\|metadata_transfer::\|rename_key_migration::\|audio_decode::\|ai_caption_db::\|ui_tools_menu::\|context_menu_model::\|caption_job_worker::\|model_addon::\|vlm_addon::\|keymap::/) \| test(preferences_context_menu_layout_snapshot)' --no-fail-fast` | 更新指定なし、559/559成功、110.958秒。initial545件は539成功/6失敗で、新メニュー・転送版・store数の期待値更新漏れを直して再確認 |
| `UPDATE_SNAPSHOTS=1 cargo nextest run -j 1 -p mimageviewer --lib -E '<snapshot5件とprogress小画面1件>' --no-fail-fast` | 6/6成功、7.942秒（compile 5分48秒）。選択は `target/asr-t1-chunk2/snapshots.py` |
| `cargo test -j 1 -p mimageviewer --lib ai::transcript_job_worker::tests::transcript_extract_long_media_benchmark -- --ignored --exact --nocapture` | m6とGを個別に1/1成功。下記の環境変数で素材を明示 |
| `python scripts/check_ui_glyphs.py` | 危険なグリフ0件、exit0（コンパイルしない静的検査として直接実行） |
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-full.ps1 -SuppressCrashDialogs` | exit101、launcherの埋込入力となるrelease exe 5本がなく、workspaceテストの実行前に停止 |
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-lib-full.ps1` | 17676件実行、17669成功、6失敗、1timeout、77skip、1202.475秒。下記で原因と修正後の再確認を区別 |
| `cargo nextest run -j 1 -p mimageviewer --lib -E 'test(/transcript\|asr_setup\|content_identity::restore::\|history_transition_storage_keeps_app_stack_footprint_bounded/)' --no-fail-fast --success-output final` | Box化とrestore fixture修正後、75/75成功、16.319秒（compile 7分14秒） |
| `cargo test -j 1 -p mimageviewer --lib app::folder_scan::section1375_scan_perf_tests::section1375_large_physical_scan_10k_100k -- --exact --nocapture` | 単独再確認1/1成功、97.11秒。テスト/timeout条件のsource変更なし |
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-dev.ps1 -PreserveRuntime` | exit0、coreを含む5本とruntime DLLの配置成功。製品の起動なし |
| `cargo fmt` / `cargo fmt --check` / `git diff --check` | 最終ソースですべてexit0。改行のLF→CRLF予告は既存のgit設定によるwarning |

初回の対象テストcompileは11分50秒。タイムアウトせず完了を待った。
テスト名の部分一致で処理全体を弱めていない。転送版の回帰はv14に加えてv13読取りとv12の未知欄無視を維持し、
メニューは兄弟kindの期待列も更新した。新ASRの実装テストは初回から成功した。
ログは `target/asr-t1-chunk2/{check,focused,snapshots-update,focused-final}.log`。
workspace gateの不足入力は `target/release/mimageviewer-{core,remote,epub-pdf,mcp,musicvideo}.exe`。
別treeの古いexeで埋めたり、launcherの検査を回避したりしていない。
通常core/devbin checkとこのgateのログは `target/asr-t1-chunk2/{check-final,check-bench,full-gate}.log`。
起床継続のコード・3回帰も独立レビューで指摘なし。
559件の結果はこの局所修正前、11件の再確認は修正後の `drain-focused.log` に記録した。

本体lib全件では、既知のnamed pipeアクセス拒否2件のほか、上記Appサイズ1件とrestoreのfixture/期待値3件を検出して修正した。
named pipeは区切り1と同じ2テスト・同じOS5で、検収側がsandbox外で成功を確認済みの環境制約として扱う。
既存 `app::folder_scan::section1375_scan_perf_tests::section1375_large_physical_scan_10k_100k` は600秒でtimeoutした。
10k/100kの走査・projection出力は出ていたが、終了まで成功したとは扱わない。
同じテストを単独Cargoで再確認し、97.11秒で後片付けを含む終了まで成功した。
単独のscan/projectionは10kが74.758/68.001ms、100kが887.365/635.617ms。
全件時はそれぞれ391.699/229.320ms、2716.169/2545.787msだった。
生成/削除の各phase時刻は計測しておらず、他セッション負荷や後片付けがtimeoutの原因と断定はしない。
全件ログは `target/asr-t1-chunk2/lib-full.log`、詳細とJUnitは
`target/test-lib-full/runs/20261113T190312531Z-b83a4a16/`。
全体成功のreceiptは作っていない。
追補のBox化とtest-onlyのinventory修正も独立レビューで指摘なし。
修正後はASR/UI・asr_setup・restore module・Appサイズと通常coreを再確認し、
無影響の個別成功・snapshot・抽出測定は再利用する範囲でレビュアーと合意した。
全件の非ゼロ結果や600秒timeoutを、再確認の成功で上書きしない。
修正後のログは `target/asr-t1-chunk2/{post-full-focused,scan-isolated,check-final-core}.log`。

後処理の参照は、正本Pythonそのものから生成した6合成ケース・44個のloop置換と、保存済み公開評価出力3本
（m1 22 / m3 31 / m5 10、計63区間）。全kept区間の本文/時刻とstock/repeat数を比較した。
正規表現のしきい値だけでなく、秒floatへ変換してから差を取る演算順も一致させた。
1〜1001msのstockと、4001〜64001msのrepeatは丸めで境界の側が変わるため、独立したfixtureにした。
私的Gの本文はsource/fixtureへ転載していない。

### 抽出だけの長尺測定（§13）

上記の手動ignoredテストは `MIV_ASR_EXTRACT_BENCH_MEDIA` で素材を明示したときだけ実行する。
一般のlibテストでは実行しない。モデル・ASR worker・App・通常settingsは使わず、PCM sinkはno-op。
同じ抽出APIを使う `cargo run --features dev-tools --bin transcript_extract_bench -- <media>` も用意した。
変換は一時WAVを作らず、最大1MiBの枠で捨てる。今回の実測はwarmなlib test binaryを使った。

| 素材 | 入力 | 選択stream | 出力sample数 | PCM長 (秒) | open/probe (秒) | 変換 (秒) | 合計 (秒) | 合計/音声 | 倍速（合計） |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| m6 | 評価 `materials/m6_long/input.m4a`（AAC、48kHz） | 0 | 63959723 | 3997.482 | 0.02124 | 2.08166 | 2.10402 | 0.00052634 | 1899.93 |
| G | 評価対応表の元MP4、5,735,991,473bytes | 1 | 253505434 | 15844.089 | 0.07067 | 23.56292 | 23.63637 | 0.00149181 | 670.33 |

Gの元動画は `work/user-videos/mapping.json` のGから読み、音声の再生・私的本文の保存はしていない。
表のPCM長はsample数をmsへ切り捨てた終端値（duration_ms/1000）。
合計はopen/probeを含み、Rustテストの起動・DLL loaderの時間は含めない。変換はextract本体だけ。
ASR pipeの背圧は測定に含まない。実ジョブでは同じeventにpipe待ちも含まれる。
他セッションのCPU/IO負荷とOSキャッシュ状態は排除しておらず、参考値として扱う。
証跡は `target/asr-t1-chunk2/extract-bench-{m6,G}.log`、実行scriptは同 `bench.py`。

GのPCM sample数はT0の固定WAVと同じ。一方m6はT0固定WAV（63959536samples / 3997.471秒）より
187samples（約11.69ms）長い。入力AACの宣言長は3997.471秒、nativeのsample数は最終AACフレームの
paddingが残る場合と整合する量だが、今回の計測はPCM全byteの比較ではないので原因は断定しない。
native依存のheaderはFFmpeg `n7.1.5-16-g9a4bb2c579-20260816`、現在の評価用CLIは9.0.2。
無音埋め/原点の合成回帰は全byte一致で成功した。metadataの長さを根拠に末尾を独自に切る処理は追加していない。
区切り1のm1〜m6一致は固定評価WAVによるworker側の結果であり、このnative抽出で同じ本文になることを
追加確認した証拠とは扱わない。C++ workerとそのparamsは今回変更していない。

### UI snapshot

新規3枚・更新4枚を実装担当と独立reviewerが目視し、文字欠け・重なり・豆腐文字・操作の画面外配置がないことを確認した。
小画面1093×614と546.5×307では、純geometryと実クリックで取得/閉じる・削除/取消・中止を確認した。
Appや製品ウィンドウを起動した対話試験ではない。

| PNG | 大きさ | bytes | 区分 |
| --- | --- | --- | --- |
| `asr_setup_confirm_dark` | 1093×614 | 68254 | 新規 |
| `transcript_job_progress` | 480×290 | 25718 | 新規 |
| `transcript_delete_confirm_dark` | 560×260 | 24436 | 新規 |
| `tools_menu_dark` / `tools_menu_light` | 370×300 | 30424 / 30167 | ASRの2行を追加 |
| `preferences_context_menu_layout` / `preferences_context_menu_layout_open_with` | 620×700 | 166174 / 152841 | 新しいmenu commandの設定行を反映 |

## 変更ファイル

brace内は同じディレクトリ。新しいbinary fixtureも検収時の追加対象に含める。

| 区分 | ファイルと要点 |
| --- | --- |
| module / 開発用bin | `Cargo.toml`, `src/lib.rs`, `src/ai/mod.rs`。新moduleとdev-tools限定の `transcript_extract_bench` を登録 |
| 作成worker | `src/ai/transcript_job_worker{,_tests}.rs`。取消・起動・並列pipe・skip・失敗/保存・perf・手動抽出測定 |
| 音声抽出 | `src/ai/transcript_audio.rs`, `src/ai/transcript_audio/{tests.rs,delayed_tracks.mka,offset_tracks.mka,fixtures.md}`, `src/bin/transcript_extract_bench.rs` |
| 共有デコーダ | `src/audio_decode.rs`, `src/video/decoder.rs`。decoder open/layout/discard・音声一覧の部品を共用し、既存再生の時刻/選択挙動は変更しない |
| 後処理 | `src/ai/transcript_postprocess.rs`, `src/ai/transcript_postprocess/{tests.rs,fixtures.json,evaluation_fixtures.json,generate_fixtures.py,fixtures.md}`。Python参照と素材の由来を同梱 |
| DB | `src/transcript_db.rs`, `src/transcript_db/tests.rs`, `src/ai_caption_db.rs`。新schemaと保存規則、共通の取消対応commit・時刻変換・schema公開境界 |
| 汎用store / 転送 | `src/rename_key_migration.rs`, `src/metadata_transfer.rs`, `src/ui_dialogs/metadata_transfer.rs`。STORES、descriptor表示失効、v14のexport/importと変更集計 |
| App所有とpoll | `src/app.rs`, `src/app/update_phases.rs`。ASR owner、root poll、終了、中止、共通modal、metadata採用。再生と共通のメモリ上の音声選択helperをcrate内公開 |
| ジョブ / 取得 / 削除UI | `src/ui_dialogs/{transcript_job.rs,asr_addon.rs,transcript_delete.rs,mod.rs,model_addon.rs,vlm_addon.rs}`。新UIと取得部品の共有 |
| メニュー | `src/context_menu_model.rs`, `src/keymap.rs`, `src/ui_dialogs/context_menu.rs`, `src/ui_main.rs`, `src/ui_tools_menu.rs`。右click/Toolsと安定ID、既定キー追加なし |
| menu inventory回帰 | `src/app/search_nav_entry_inventory_tests.rs`。新Tools commandの一覧だけを更新、検索機能は変更していない |
| restoreのstore inventory回帰 | `src/content_identity/restore.rs`。test-onlyの実schema初期化一覧とDB open回数だけを更新 |
| UI画像 | 上記 `tests/snapshots/` の新規3枚・更新4枚 |
| 文書 | `docs/{README.md,architecture-overview.md,async-architecture.md,ui-responsiveness.md,keymap-spec.md,local-asr-t1-chunk2-report.md}` |

区切り1のC++、`asr_worker`、モデルpack/公開index、T2のindexerと検索、MCP、表示パネル/表示cacheは変更していない。

## 利用者確認用ビルド

`scripts/build-dev.ps1 -PreserveRuntime` は成功。native build待ちを経て、coreの初回ビルド45分18秒、
remote 3分09秒、EPUB PDF 1分55秒、MCP 23.77秒、music-video editor 43.21秒。
`target/dev-runtime/` の5本とFFmpeg/VCRT DLL等を配置した。VCRT PE監査はruntime=4、pe=5で成功。
ビルド/最終静的検査のログは `target/asr-t1-chunk2/{build-dev,fmt-final,fmt-check,diff-check}.log`。

確認用coreは393881088bytes、SHA-256:
`5b0ec8be2b8ec466299d6a7afd940b4713628331d1d367f9d5d7768058f4b6ef`。
内包元workerもSHA-256を再照合し、区切り1と同じ
`d7e38aee6e90cc96ce8125b684d9e875c37a06d6f17582043d8762cc7979811c` だった。
workerのsource identity/CTest/実モデル検証はC++とvendorに変更がないため区切り1の結果を再利用する。

エージェントは製品exeを起動していない。利用者がこのworktreeで起動するコマンド:

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

dev-runtimeは最適化profileでありportableではない。通常の `%APPDATA%\mimageviewer` を使い、
実際の設定/DBを更新し得る。single-instance mutexを共有するため、起動前にインストール版とトレイ常駐版を終了する。
確認シナリオは、短い動画/音声での初回案内・作成→再実行のskip、実行中の追加、進捗を閉じて再表示、中止、
終了後の削除、メタ情報のexport/import。文字起こし本文の表示は区切り3なので今回の確認対象に含めない。

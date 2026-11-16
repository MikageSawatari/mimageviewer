# ローカル文字起こし T1 区切り4 実装・検証記録

2026-11-14。基準commit `cee2b4ad3`（区切り3 `e34a28713` とP2修正）。
作業場所は `G:\home\mimageviewer-asr-t1` のみ。コミット・製品exeの起動はしていない。
公開文書（manual/privacy等）と計画書本体は設計担当の並行差分であり、実装担当は編集していない。

## 実装内容

- 第3権限のcommit `6e1a8ff79` と同じ保存・snapshot・受付・送信前検査・設定転送除外の経路へ、
  独立した第4権限 `agent_show_transcripts` / `PermissionSnapshot.transcripts` を足した。既定OFF。
  旧JSONと旧DBキー無しでもOFFになり、5値のどれが変わってもepochが進む。
  preferencesの適用/取消で即時設定済みの許可を巻き戻さない。
- agent IPCをstrict-equalityの3→4へ更新。coreとMCP adapterは同じ確認用buildで作る。
- `get_item_info.transcript` は要約だけ。モデル、UTC Unix msの作成日時、音声stream index、
  秒単位の音声長、区間数、Unicode文字数、古さを返し、本文を含めない。
  OFFはDB open前のNotPermitted。UIとMCPの古さは共通純粋関数
  `transcript_display_cache::transcript_is_stale`（署名＋Windowsのconfig_version）を使う。
- `get_transcript` は同じread-only workerの経路で取得する。tool一覧だけに依存せず、
  受付とhandlerの両方でadmissionの権限を確認し、送信直前は既存ReplyPolicy::Readのepoch照合を使う。
- UIに第4チェックと氏名/私的会話の短い注意を追加し、公開範囲表示・設定検索・snapshot fixtureへ反映。
  検索Transcript/索引（T2）、open_in_viewerの開始時刻（T3）は追加していない。

## 決めた扱いと説明文の型との差

| 項目 | 扱い / 理由 |
| --- | --- |
| 対象 | 動画・音声のFS上の実ファイル。get_item_infoの非対象（画像/本/部分指定）は、captionと同じONでNone、OFFでNotPermitted。DBは開かない。専用toolではNotApplicable |
| 無い・作成中 | 保存済み行が無ければNone。作成中も同じで、旧行があれば旧行を返す。Appのジョブ状態を別のsnapshotへ複製せず、保存データだけを読む |
| 失敗 | Failed。captionの失敗と同じく理由の文字列は送らない（DBには理由文字列も保存していない） |
| 開始秒 | `item:{path,subresource?}`、`start_secs?`（既定0）、`cursor?`。保存済みコンテナ開始=0秒。開始秒より後まで続く区間を返し、指定秒に重なる区間も含める。負/非有限値はBadRequest |
| 通常の区間 | 区間境界でページを切る。時刻はstart_secs/end_secs、本文は既存TextValueの `{text,truncated}` |
| 巨大な1区間 | 1区間がMAX_ITEM_CHARSを超える場合のみUTF-8の文字境界で本文を切り、text.truncated=true。元の区間時刻を保ちcursorは次の区間へ進む。最後の1区間ならcursor無しでもpage.truncated=true |
| 出力量 | 区間はMAX_ITEM_CHARS、状態と要約とcursorを含むページ全体はMAX_PAGE_CHARS。既存mcp_content_chars（structured JSON＋escaped text copy）を使う。極端に巨大なメタ情報はInternal＋ログで割り切る |
| 続き | 同じitemを指定。start_secsは省略または初回と同じ値。対象キーdigest・rev・次区間位置・初回開始秒・許可epochをcursorに持つ。rev更新/行削除/無効な位置は取り直しを要求 |
| 権限変更 | ONのまま他の許可が変わった場合を含め、cursor epoch違いはPermissionChanged。文字起こしOFFはcursorより前の受付でDisabled（縮小画像と同じ例外） |
| 改ざん | FolderCursorと同じhex JSON/8KiB上限/引数照合に加え、位置やrev自体の改ざんも拒むためHMAC-SHA256で署名。保持するのはプロセス共通ランダム鍵だけで、行snapshotやcursor台帳は持たない。再起動前のcursorも無効 |

HMACは既存lock内の `hmac 0.12.1` をcoreの直接依存へ追加した。鍵は既存uuid v4を2つ使った32 bytes。
対象キーは既存cursorの引数digestと同様にdigestで持ち、長いpathをcursorへ複製しない。
MACはconstant-time検証を使う。独立限定レビューでは、この認可・所有・cursor・予算の設計に重大な指摘はなかった。

## tools/listのサイズ

初回の実測は `stage2_complete_tool_discovery_fits_final_envelope_budget` が失敗。
最初の超過ケース（Organize、transcripts ON、他の3チェックOFF、legacy）は40,449文字で、
必要な512文字の余裕を確保できなかった。schemaや上限を削る/緩める変更はしていない。

15toolの従前の詳細説明を全文保持してinitializeのINSTRUCTIONSへ移し、共通のページ上限/cursor/too_long/bytes補足を集約した。
各toolには機能・引数要旨・許可・製品名・untrusted注意を残し、get_transcript自身にコンテナ開始=0秒の1文も残した。
INSTRUCTIONSは6,136文字（レビュー後のscope明確化を含む）。tools/list側の説明の重複を削減した。上記40,449文字は初回に止まった途中のcaseで、
以下の最終最大値と同一tool集合のbefore/after差を表すものではない。
最終の全組合せ/各MCP版のテストで、最大は**39,467文字**
（Organize、縮小画像ON、GPS/説明文OFF、文字起こしON、modern）。
512文字のreserveを加えて39,979文字で、40,000文字以内。schema・errorの検証契約も維持した。

## UIスレッド・perfの監査

`Connection::request` の許可受付後、`agent-read` → `work::respond` → `agent-read-worker` →
`Handlers::execute` のget_item_info/get_transcriptから、readonly open/get/JSON parse/stat/文字数集計を行う。
UI/presenterはDBへ触れず、既存の文字起こし表示cacheもreader workerのまま。
設定UIの変更は既存の即時保存経路に1フィールドを加えたもので、I/O経路を増やしていない。

区切り2の既存perfをsourceで確認した。`transcript/extract`（変換elapsedと音声長）、
`infer_window`、`infer_file`、`save`、`utf8_discarded`（bytes/fragments）が揃っている。
MCPの2経路は既存Connection::record → LaneLog/record_perfを通し、`agent_ipc/get_item_info` と
`agent_ipc/get_transcript` にduration_ms/count/errorを記録する。本文やcursorをログへ入れない回帰も追加した。
新しい性能計測は行っていない（利用者指示）。

## 変更ファイル

- wire: `crates/agent-ipc/src/lib.rs`, `read_types.rs`
- adapter: `crates/mcp-server/src/lib.rs`, `schema.rs`, `tests.rs`
- core: `src/agent_ipc/mod.rs`, `transcript.rs`, `transcript_tests.rs`, `item_info.rs`, `item_info_tests.rs`, `logging_tests.rs`, `read_tests.rs`
- snapshot literalの追随: `src/agent_ipc/folder_tests.rs`, `input_path_tests.rs`, `metadata_write_tests.rs`, `tests.rs`, `thumbnail_alias_tests.rs`, `thumbnail_identity_tests.rs`, `thumbnail_tests.rs`
- 設定/UI: `src/settings.rs`, `settings_db.rs`, `settings_transfer.rs`, `ui_dialogs/agent_integration.rs`, `ui_dialogs/agent_integration/tests.rs`, `ui_dialogs/preferences.rs`, `ui_dialogs/preferences/search_index.rs`
- stale共有: `src/transcript_display_cache.rs`, `transcript_display.rs`, `transcript_display/tests.rs`
- 依存/文書: `Cargo.toml`, `Cargo.lock`, `docs/mcp-integration-plan.md`, 本書
- UI PNG（10枚更新、1000×1150px、親が最終10枚を原寸で目視。独立reviewerも4チェック/注意文/公開範囲/既存操作を確認）:
  `tests/snapshots/agent_integration_window_disabled_light.png`, `agent_integration_window_disabled_dark.png`,
  `agent_integration_window_read_light.png`, `agent_integration_window_read_dark.png`,
  `agent_integration_window_open_light.png`, `agent_integration_window_open_dark.png`,
  `agent_integration_window_organize_light.png`, `agent_integration_window_organize_dark.png`,
  `agent_integration_window_records_light.png`, `agent_integration_window_records_dark.png`。
  checkbox/氏名・私的会話の注意/送出範囲の折返し/固定の閉じるに欠けや重なりは見当たらない。
  最終目視で見出しも4項目へそろえ、再生成と通常比較を行った。下部は既存のscroll viewportへ続く。
  狭い1093×614・200%の到達性はheadless handler/geometry testで検査し、実機の操作確認とは区別する。

## テスト・検査の結果

コンパイルし得るコマンドはすべて次の入口を使った。全libを含む最終確認はさらに `-FullTest`。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner asr-t1-codex -WorkDir G:\home\mimageviewer-asr-t1 -Command '<コマンド>'
```

run-heavyは先行sessionの全件lockを待った後、書込み不可の警告とともにBelowNormalで実行した。
CARGO_BUILD_JOBS=1、MSBUILDDISABLENODEREUSE=1、CMAKE_BUILD_PARALLEL_LEVEL=1。
以下の所要時間は、特記以外nextest Summaryの実行時間。

| wrapper内のコマンド | 結果 |
| --- | --- |
| `cargo check -j 1 -p mimageviewer --lib --tests` | 成功、4分32秒 |
| `cargo check -j 1 -p mimageviewer --bin mimageviewer-core` | 成功、1分25秒 |
| `cargo nextest run -j 1 -p mimageviewer-agent-ipc --lib --no-fail-fast --success-output final` | **18件すべて成功**、0.385秒 |
| `cargo nextest run -j 1 -p mimageviewer-mcp --lib --no-fail-fast --success-output final` | **77件実行：75成功、実named pipeの環境制約2件失敗**、64.916秒。失敗を成功とは扱っていない |
| `cargo nextest run -j 1 -p mimageviewer-mcp --lib -E '<adapter影響filter>' --no-fail-fast --success-output final` | instructionsの文言scope明確化後、**6件すべて成功**、1.886秒。最悪discovery 39,467文字 |
| `cargo nextest run -j 1 -p mimageviewer --lib -E 'test(agent_ipc::transcript::tests)' --no-fail-fast --success-output final` | **9件すべて成功**、2.613秒（test codegen 9分16秒） |
| `cargo nextest run -j 1 -p mimageviewer --lib -E '<関連filter>' --no-fail-fast --success-output final` | **312件すべて成功**、73.564秒。既知core named pipe2件は明示除外。9件もこの312件に含む |
| `cargo nextest run -j 1 -p mimageviewer --test ui_snapshot -E 'test(agent_integration_window_)' --no-fail-fast --success-output final` | 最終 `UPDATE_SNAPSHOTS=1` 生成は**10件成功**、21.257秒。その後UPDATE無しの通常比較も**10件成功**、13.096秒 |
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-full.ps1 -SuppressCrashDialogs` | **未完了、exit101**。launcher内包用release exe 5本がなく、workspaceテスト開始前に停止。doctestと後続vendor gateも未実行 |
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-lib-full.ps1`（外側 `-FullTest`） | **17,748件実行：17,744成功、3失敗、1timeout、78skip**、1095.741秒、16slow。全件exit100を成功とは扱っていない。下記の切分けを参照 |
| `cargo nextest run -j 1 -p mimageviewer --lib -E 'test(=app::tests::still_window_mode_key_tests::remote_acquire_retires_startup_resolves_and_rejects_new_activations)' --no-fail-fast --success-output final` | 全件終了後、同じソースを単独で**1件成功**、1.113秒。全件の失敗を上書きしない |
| `cargo fmt`（引数なし） / `cargo fmt --check` | 成功 |
| `python scripts/check_ui_glyphs.py` | 成功、危険なグリフ0件 |
| `git diff --check`（今回所有したsource/docs） | 成功 |

filterの実値:

```text
adapter影響filter:
test(/initialization_instructions|stage2_complete_tool_discovery|schema::tests/)
関連filter:
test(/agent_ipc|agent_permission_snapshot|failed_permission_commit|settings_transfer::|immediate_agent_permission_tests|agent_integration::tests|settings_window_hints_match|transcript_display|transcript_job_worker|transcript_audio/) - test(=agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry) - test(=agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers)
```

本文の9件は受付/handlerのOFF拒否、None/Failed/対象外、開始秒と重なる区間、cursorの続きとreadonly bytes不変、
rev更新・削除・epoch変更・MAC改ざん・別item・start不一致・巨大UTF-8/escape本文、送信直前の許可変更を検査した。
期限は制御clockで実handlerを1区間確定後にexpiredへ進め、prefixとnext=1のcursor、その後の残り2区間を検査する。
pipe経由で実時間20秒待った確認とは区別する。一般controllerのdeadline/prefix/permitは既存read_testsでも確認した。
関連312件の後の見出し1行の変更は、最終PNG比較と全libで再検査した。

### 環境制約と全件で残った失敗

adapterの失敗2件は次のとおり。`crates/mcp-server/src/pipe.rs:499` のfixture thread開始前のclient openが
`raw_os_error=Some(5)`、AccessDeniedで失敗した。該当pipeコードは今回変更していない。
sandbox外の再実行はしていない。

- `pipe::tests::real_pipe_cancel_reaches_core_while_stdio_stdout_is_blocked`
- `pipe::tests::real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld`

core全libの失敗3件とtimeout1件:

1. `agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry`
2. `agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers`
   上記2件は既知の同じOS5 named pipe制約。今回sandbox外では再確認していない。
3. `app::tests::still_window_mode_key_tests::remote_acquire_retires_startup_resolves_and_rejects_new_activations`
   全件ではlease解放直後の1回のpoll後のassertがAcquiringRemote/RemoteActive不一致（3.797秒）。
   同一ソースの単独確認では成功した。既存のRemote acquireはAI/book writer等の非同期barrierが揃うまで
   Acquiringに留まる経路だが、今回どのblockerだったかは確定していない。再現しないことだけで原因を断定せず、
   関係するRemote/表示/fixtureコードを変更していない。
4. `app::folder_scan::section1375_scan_perf_tests::section1375_large_physical_scan_10k_100k`
   600.067秒で打切り。ログには10,000件と100,000件のassert後の実測がともに残る。
   100,000件はscan=3090.116ms、projection=2017.328ms。
   このlog以後のsource上の処理はTempDirを含むローカル値の破棄とworker join。
   どこで停滞したかは未特定で、検査済みの走査処理と後処理を含むテスト全体のtimeoutを区別する。
   制限時間延長やこのテストの再実行はしていない。

全lib開始時点のsource IDは `ba517ebb1b64a73d29b264d7410e28745d4e546a66747a8bda9f52d07c8b5742`。
JUnit/stdout/stderrは `target/test-lib-full/runs/20261114T010938333Z-effd8367/`。
失敗実行なので成功台帳として再利用しない。全件の実行中は本実装のRust sourceとPNGを固定した（公開文書は設計担当の並行作業）。
実装担当が全件終了後に変更したのは本報告書だけ。

workspace gateで不足したのは `target/release/` のcore、remote、EPUB worker、MCP adapter、music-video editor。
別profileで代用せず、今回releaseの追加buildはしていない。

詳細ログは `target/asr-t1-chunk4/` の `check-tests`, `core-check`, `wire`, `mcp-budget`（初回超過）、
`mcp`, `mcp-final`, `transcript`, `focused`, `snapshots-update`（初回）、`snapshots-final-update`,
`snapshots-compare`, `workspace-gate`, `lib-full`, `remote-isolated` の各 `.log`。
wrapper呼出し1回は括弧付きfilterのshell引用不足でテスト起動前に終了し、Pythonの引数配列で渡す形へ直した。

## 設計書との対応

未承認の機能変更はない。今回決めた対象外/未保存/失敗/巨大区間/開始秒/続きの扱いは前述のとおり。
FolderCursorとの差は内容改ざんも拒む署名と再起動時の無効化で、snapshot/台帳は追加していない。
権限OFFのDisabledは既存の縮小画像受付と同型。その他のepoch変更はPermissionChanged。
MCP planの§5.6・§15.2・toolの一覧/引数/予算とserver instructionsを更新した。
計画書本体と公開manual/privacyの並行差分には触れていない。

## 確認用ビルド

`run-heavy` 経由で次を実行し、**exit0で完了**した（10:32:40〜10:47:45 JST）。
製品exeは起動していない。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-dev.ps1 -PreserveRuntime
```

| 段階 | 結果 / Cargo所要時間 |
| --- | --- |
| core | 成功、14分09秒 |
| remote service | 成功、0.54秒 |
| EPUB PDF worker | 成功、0.52秒 |
| MCP adapter | 成功、13.12秒 |
| music-video editor | 成功、30.03秒 |
| VCRT/PE検査 | 成功、runtime=4、PE=5 |

coreとadapterは共通crateのAGENT_PROTOCOL_VERSION=4でビルドした。
確認用バイナリ（`target/dev-runtime/`）:

| ファイル | bytes | SHA-256 |
| --- | ---: | --- |
| `mimageviewer-core.exe` | 393,913,856 | `89c3d87e78654f03e545dbe26c00f5ef60015c55751e86baade5c565253838b8` |
| `mimageviewer-mcp.exe` | 2,092,544 | `fc7eb03d77bdd117389633b1f73213edd891e35fb1a8f52e5dc0bee9a71158c4` |

build logは `target/asr-t1-chunk4/build-dev.log`、VCRT証跡は `target/vcrt-pe-reports/dev-runtime.json`。
worker exeとモデルpackの変更・backupへの書込みはない。コミットせずworktreeに残した。
設計担当による公開文書・installer readme・計画書の並行差分は保持している。

利用者がこのworktreeで起動するコマンド:

```powershell
Set-Location G:\home\mimageviewer-asr-t1
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

通常featureでportableは有効にしていない。既定で実際の `%APPDATA%\mimageviewer` を使い、設定やDBを更新し得る。
single-instance mutexが共通なので、installed版・tray常駐版を先に終了する。

確認対象はAI連携の第4チェックのON/OFF、get_item_infoの本文を含まない要約、get_transcriptの時刻と続き、
取得後に権限を変えたcursorの拒否。MCP接続・実機UIの操作は未実施で、利用者確認へ残す。
登録先adapterが今回の `G:\home\mimageviewer-asr-t1\target\dev-runtime\mimageviewer-mcp.exe` を指すことも確認する。
既存登録が別の古いadapterを指している場合は、そのまま今回のcoreと組み合わせない（IPCは厳密一致）。
実登録の変更は利用者が既存のAI連携画面から行い、この作業では登録先ファイルを変更していない。

## 追加作業: Aboutのソフトウェア情報（2026-11-14）

### 変更と判断

今回の変更は `src/ui_dialogs/about.rs`、下記PNG 2枚、本報告書の計4ファイル。

- `src/ui_dialogs/about.rs`: 見出しと全文の折りたたみを「AI 説明文・文字起こし」に揃え、使用ライブラリに whisper.cpp を追加した。Qwen の案内の次に Whisper large-v3-turbo の MIT・ggml 変換・Q5_0 量子化・再学習なしの案内を加えた。
- 全文は既存の `VLM-WORKER-LICENSE.txt` 1つをそのまま使う。同梱本文には whisper.cpp v1.9.5 と MIT の著作権表記が既にある。折りたたみの ID `about_vlm_worker_license` も維持した。
- `asr_addon::addon_root()` と `addon` の公開先を確認した。LICENSE / NOTICE の保存先は `<data_dir>/addons/asr/packs/<model_id>/` なので、About もこの具体的な階層を案内する。ASR の環境設定ページやフォルダを開く導線は追加しない。
- 製品とテストが同じ `draw_ai_worker_credits` を描く。UIで新しい I/O は行わない。installer readme・ライセンス全文・worker exe・モデルpackは変更していない。計画書との新たな食い違いはない。

### スナップショット

追加したPNGは以下の2枚。既存PNGの更新はない。

| ファイル | 寸法 | bytes |
| --- | --- | ---: |
| `tests/snapshots/ai_worker_credits_light.png` | 520×300 | 70,515 |
| `tests/snapshots/ai_worker_credits_dark.png` | 520×300 | 69,872 |

実装担当が両方を目視し、見出し・共通全文の折りたたみ・Qwen と Whisper の案内・保存先の全文が欠けずに読めることを確認した。純粋な描画fixtureであり、製品アプリは起動していない。
同じテストで折りたたみを実クリックし、whisper.cpp を含む共通ライセンス全文が表示されることも確かめた。

### 検証

以下の Cargo / ビルドコマンドはすべて、この入口を通した。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner asr-t1-codex -WorkDir G:\home\mimageviewer-asr-t1 -Command "<コマンド>"
```

ログを残すため `<コマンド>` 内で `python -B -I target/asr-t1-chunk4/run.py <label> ...` を使い、Cargo / CMake のジョブ数を1にした。ロック領域に書けない警告後は、指定どおり低優先度で続けた。

| コマンド | 結果 |
| --- | --- |
| `cargo nextest run -j 1 -p mimageviewer --lib ai_worker_credits_snapshot --no-fail-fast --success-output final`（`UPDATE_SNAPSHOTS=1`） | 1件成功、上記2枚を生成（6.072秒）。初回のテスト用import不足を修正した後の結果。 |
| `cargo nextest run -j 1 -p mimageviewer --lib ui_dialogs::about::tests --no-fail-fast --success-output final`（更新指定なし） | 9件すべて成功（16.630秒）。新規2枚の通常比較、既存ライセンス・全文表示・Aboutのスクロールも含む。 |
| `cargo fmt`（引数なし）、`cargo fmt --check` | 成功 |
| `python scripts/check_ui_glyphs.py` | 成功、危険なグリフ0件 |
| `git diff --check` | 成功 |
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-dev.ps1 -PreserveRuntime` | 成功。core・remote・EPUB worker・MCP adapter・music-video editor を配置し、VCRT/PE検査も成功（runtime=4 / pe=5）。 |

ログは `target/asr-t1-chunk4/about-credits-update-fixed.log` と `about-credits-compare.log`。
確認用ビルドのログは `target/asr-t1-chunk4/about-credits-build-dev.log`。
run-heavy は11:13:36開始、別worktreeのnative build終了を待って11:30:55にexit 0で完了した。
core のコンパイルは5分54秒。最終 core は `target/dev-runtime/mimageviewer-core.exe`、393,921,536 bytes、
SHA-256 は `5918dba6cfb7ca841b190f5b6e118ceb06a1ca6f1aad5c15115225440334f192`。

コミット・製品バイナリの起動は行っていない。並行して変更された `htdocs/mimageviewer/manual/tut-ai-transcript.html` は、この追加作業の対象外として保持した。
起動コマンドと通常profileの注意は前節と同じ。利用者が確認する箇所は「バージョン情報 → ソフトウェア情報」の
「AI 説明文・文字起こし」、モデルの案内、共通のライセンス全文。

## 独立レビュー後の修正（2026-11-14）

### 変更と判断

この節は、初期実装の「詳細説明のinstructionsへの移動」「cursorのHMAC」「Aboutの内部model_id表記」を置き換える。

- P2-1: `crates/mcp-server/src/schema.rs` の各tool descriptionへ、権限・入力の条件・返す内容の意味・untrusted dataの注意を戻した。`get_thumbnail` の補正非反映・画像blockの並び・上限、タグの正規化・64文字・空白不可・失敗/queuedの意味なども各tool単体で読める。`lib.rs` の INSTRUCTIONS は共通の補足に絞った。
- 予算調整として、条件を保って説明を短くし、schemaのdescriptionとの重複を除いた。outputSchemaのrootが保証するobject型を直下の分岐へ重ねる指定も除いた。rootのobject型、必須列、追加列の拒否、値の制約、各tool単独のschemaは保持する。40,000文字と512文字の余裕は変更していない。
- P2-2: `src/transcript_panel.rs` は、eguiがzoom修飾を取り除いた `smooth_scroll_delta()` をScrollAreaが消費する前に読む。raw MouseWheelの有無では止めない。一覧・scrollbarの所属、layer、clip、有効状態の既存の判定は維持した。Ctrl+wheelはzoomされてもFollowingを保ち、通常のwheelはManualへ移るhandler-level回帰を加えた。
- P3: `src/agent_ipc/transcript.rs` のHMAC・SignedCursor・プロセス鍵を削除した。hex JSONには対象キーと初回開始秒の引数digest・rev・epoch・位置を持ち、要求ごとに行と照合する。同じ本文内の有効な別位置は許可し、範囲外・引数不一致・行変更/削除・権限変更は既存の拒否を返す。認可の受付・handler・送信直前の照合は維持した。
- `Cargo.toml` と `Cargo.lock` からcoreのhmac直接依存を外した。Remoteが使う別のhmac依存とlock内のpackageは残す。
- `src/agent_ipc/transcript_tests.rs` は、署名の改ざん拒否から、引数・rev・epoch・範囲の照合と有効な別位置を読めることの検査へ変更した。cursorの再符号化も同じ値になり、再起動だけを理由にした鍵由来の失効条件はなくなった。
- `src/ui_dialogs/about.rs` のWhisperの保存先は、指定どおり「データフォルダ内の addons\asr\ に保存されます。」へ変更した。ライセンス全文と折りたたみIDは維持した。
- `docs/mcp-integration-plan.md` は、instructionsの任意性と操作条件の配置、mIVの応答予算、署名を使わないcursorの照合に合わせた。

変更ファイルは上記に加え、MCPのテスト2ファイル（`crates/mcp-server/src/tests.rs`・`schema.rs`内）、AboutのPNG 2枚、本報告書。
予算の最悪値と検証の最終結果は以下。build-devは指示どおり省略し、コミット・製品バイナリの起動は行っていない。

### P2-1の予算不足による停止

操作条件を戻した最初の計測は41,754文字のケースで失敗した。上記の重複削減後も予算内には収まらなかった。
全toolを公開し、modern形式・64文字のIDを使った最悪条件の実測は以下。

| 項目 | 文字数 |
| --- | ---: |
| 最終tools/list JSON | 40,636 |
| 既存の予約余裕 | 512 |
| 余裕を含めて必要な予算 | 41,148 |
| 現行の予算 | 40,000 |
| 不足（予約余裕を含む） | 1,148 |

応答そのものも現行予算を636文字超える。予算・予約余裕の値やテストの閾値は変更していない。
残す必要がある内容は、縮小画像の補正非反映・編集で隠した画像の扱い・blockの順序/サイズ、
タグの正規化/64文字/空白/queuedと完了の区別、各書込みの対象/結果/タイムスタンプ保持、
メタ情報の状態/古さ/生データの除外、一覧の並び/対象外/フィルタ/cursorの無効化、
文字起こしの時刻/続き/切詰め/保存状態、各toolの権限とuntrusted dataの注意。

指示どおり、P2-1は上限を変えずここで停止し、予算の見直しか現行予算での再設計を設計担当へ確認中。
このworktreeは全tool公開時にtools/listが予算で拒まれる状態なので、P2-1を完成品としては扱わない。

### 検証（独立レビュー後）

今回もCargoのビルド・テストはすべて前節の `run-heavy.ps1 -Owner asr-t1-codex -WorkDir G:\home\mimageviewer-asr-t1 -Command "<コマンド>"` 経由。
ログ用Pythonを内側に使い、Cargo/CMakeは1ジョブ。共用full testの実行中は入口で待機し、
ロック領域に書けない警告後は指定どおり低優先度で続行した。

| コマンド | 結果 |
| --- | --- |
| `cargo nextest run -j 1 -p mimageviewer-mcp --lib stage2_complete_tool_discovery_fits_final_envelope_budget --no-fail-fast --success-output final` | 予算超過で失敗。最終の最大条件40,636文字を記録。`review-budget-first.log` / `review-budget-compact.log` / `review-budget-worst.log`。 |
| `cargo nextest run -j 1 -p mimageviewer-mcp --lib --no-fail-fast --success-output final` | 77件中74成功・3失敗、63.884秒。descriptionの条件・schema制約・認可・cursorのadapter側検査は成功。失敗は上記予算1件と既知のnamed pipe 2件（fixture thread開始前のOS error 5）。`review-adapter.log`。 |
| `cargo nextest run -j 1 -p mimageviewer-mcp --lib -E 'test(tool_descriptions_preserve) \| test(schema::tests)' --no-fail-fast --success-output final` | 条件のassertを補った最終テストで5件すべて成功、0.261秒。補正非反映・権限・古さ・各toolのuntrusted注意もdescription単体で検査。`review-conditions.log`。 |
| `cargo nextest run -j 1 -p mimageviewer --lib ai_worker_credits_snapshot --no-fail-fast --success-output final`（`UPDATE_SNAPSHOTS=1`） | 1件成功、2.251秒。下記のAbout PNG 2枚だけを更新。`review-snapshot.log`。 |
| `cargo nextest run -j 1 -p mimageviewer --lib -E 'test(agent_ipc::transcript::tests) \| test(transcript_panel::tests) \| test(ui_dialogs::about::tests)' --no-fail-fast --success-output final`（更新指定なし） | 32件すべて成功、13.121秒。cursor・認可・Ctrl/通常wheel・外部drag/layer/clip・既存パネルとAboutの画像比較を含む。`review-core.log`。 |
| `cargo fmt`（引数なし）、`cargo fmt --check` | 成功 |
| `python scripts/check_ui_glyphs.py` | 成功、危険なグリフ0件 |
| `git diff --check` | 成功 |

ログのディレクトリは `target/asr-t1-chunk4/`。

更新したPNGは以下の2枚。ほかのPNGは変更していない。明暗それぞれを実装担当が目視し、
`<model_id>`を含まない `addons\asr\` の案内と本文全体の折り返し・末尾が読めることを確認した。

| ファイル | 寸法 | bytes |
| --- | --- | ---: |
| `tests/snapshots/ai_worker_credits_light.png` | 520×300 | 67,931 |
| `tests/snapshots/ai_worker_credits_dark.png` | 520×300 | 67,375 |

未完了はP2-1の予算不足に対する設計判断のみ。P2-2・P3・Aboutは対象の回帰検査と通常画像比較まで完了した。

### P2-1の予算分離（設計担当決定 2026-11-14、61a8cee41後）

設計担当の決定に従い、tools/listだけを別予算に分ける。上記の停止理由に対する追加修正であり、
各toolのdescriptionとINSTRUCTIONSは61a8cee41の内容から変更していない。

- `crates/mcp-server/src/lib.rs`: adapter内に `MAX_MCP_DISCOVERY_CHARS = 48_000` を追加した。
  `Envelope`に `ResponseKind::Data / ToolsList` を持たせ、tools/list成功結果の生成箇所だけがToolsListを指定する。
  writerへ種別を渡し、`serialize_response`は種別で予算を選ぶ。JSONのキーや本文の文字列では分類しない。
- 初期化・server/discover・通知・エラー・tools/callはDataのまま40,000文字を適用する。
  shared crateの `MAX_MCP_RESPONSE_CHARS = 40_000`、coreの `MAX_PAGE_CHARS = 38_000`、項目上限、画像上限は変更していない。
  `MAX_FRAME_BYTES`による共通のbyte検査も維持した。
- `crates/mcp-server/src/tests.rs`: 全権限・3プロトコル・3種類のID、計576組合せで、48,000文字以内かつ余裕512文字を検査する。
  tools/listの実際の生成関数を使い、従来の計測が含めていなかったmodernの `ttlMs / cacheScope` も数える。
  同一JSONをDataとToolsListで送るwriterテスト、初期化等への別予算の非適用、48,000文字境界、共通frame上限、
  3プロトコルの実受付→writerでの全tool応答を追加した。データの40,000文字境界テストも維持した。
- `docs/mcp-integration-plan.md`: §4.3・§15.4へ、データとdiscoveryの予算・理由・種別の受渡しを追記した。
  tools/listはtool数に比例するセッション開始時の一覧なので、操作条件を削るよりT2の拡張等を含む別枠を確保する。
- 本報告書を含め、今回の編集対象は上記の計4ファイル。並行して追加された `docs/local-asr-transcript-plan.md` と
  `src/agent_ipc/transcript.rs` の説明コメントは保持し、今回の変更には含めない。

UIの描画・文言・PNGは変更していない。build-devは指示どおり省略し、コミット・製品バイナリの起動は行っていない。
検証コマンドは前節と同じrun-heavy経由（内側のPythonでログ保存・1ジョブに制限）。

全576組合せの最大は **40,669文字**（Organize・縮小画像/文字起こし許可ON・modern・64文字ID）。
従来の40,636文字との差33文字は、実際の送信形に含まれる `ttlMs / cacheScope`。
description・schema・INSTRUCTIONSを61a8cee41と照合し、変更していないことを確認した。
最大値＋既存の余裕512文字は **41,181文字**、discovery予算48,000文字に対して **6,819文字**残る。
全組合せで実際のJSONのbyte数（改行込み）が `MAX_FRAME_BYTES = 4,194,304` 以下になることも検査した。

| コマンド | 結果 |
| --- | --- |
| `cargo nextest run -j 1 -p mimageviewer-mcp --lib -E 'test(tools_list_) \| test(stage2_complete_tool_discovery) \| test(stage1b_final_budget) \| test(stage1a_shared_content_budget)' --no-fail-fast --success-output final` | 6件すべて成功、3.139秒。全576組合せ、3プロトコルの受付→writer、別予算の非漏洩、40,000/48,000文字境界と共通byte上限を含む。`target/asr-t1-chunk4/discovery-focused.log`。 |
| `cargo nextest run -j 1 -p mimageviewer-mcp --lib --no-fail-fast --success-output final` | 80件中78成功・2失敗、68.614秒。失敗は既知のnamed pipe 2件のみ（fixture thread開始前のOS error 5）。文字数予算を含め残りはすべて成功。`target/asr-t1-chunk4/discovery-all.log`。 |
| `cargo fmt`（引数なし）、`cargo fmt --check` | 成功 |
| `python scripts/check_ui_glyphs.py` | 成功、危険なグリフ0件 |
| `git diff --check` | 成功 |

対象filterは共用full testの終了後12:52:20に開始し、12:52:30にexit 0で完了した。
MCP lib全件はheavy枠の空き待ち後12:57:08に開始し、12:58:20にexit 100で完了した。
環境制約2件の内容は前節と同じで、`pipe.rs`は変更していない。上記のP2-1の停止状態はこの予算分離で解消した。
設計担当の判断との食い違いはない。従来値との差33文字は計測の補正で、toolの説明文の追加や削減ではない。

### client側のdescription・instructions上限（2026-11-14）

[Claude Code公式MCP文書](https://code.claude.com/docs/en/mcp.md) のHTML版でも、
[tool結果の上限](https://code.claude.com/docs/en/mcp#mcp-output-limits-and-warnings) と
[description・instructionsの切詰め](https://code.claude.com/docs/en/mcp#tool-search-for-mcp-server-authors)
を確認した。25,000トークンと既定50,000文字超のファイル退避はtools/callの結果の扱いであり、
tools/list全体の文字数に同じ上限を適用する根拠にはしない。データ40,000・discovery 48,000の分離は維持した。

`crates/mcp-server/src/tests.rs` に `client_text_limits_tool_descriptions` と
`client_text_limits_server_instructions` を追加した。各toolとinitializeのINSTRUCTIONSは4,096文字超で失敗する。
各toolの2,048文字超過は互換性の目標として警告出力し、その目標超過だけでは失敗にしない。
非BMP文字を過小に数えないようUnicode文字数に加えUTF-16長でも上限を検査する。
`docs/mcp-integration-plan.md` §15.4へ出典・予算分離の根拠・4,096文字の検査を追記した。

| 対象 | 最長の文字数 | UTF-16長 |
| --- | ---: | ---: |
| tool description（`get_transcript`） | 569 | 569 |
| initialize INSTRUCTIONS | 852 | 852 |

2,048文字を超えるtoolは0件。全toolとinstructionsが4,096文字以内なので、重要条件の並べ替えや文言の削減は不要だった。
実際のdescription・INSTRUCTIONS、40,000/48,000の予算、coreのページ計算は前節から変更していない。
設計担当所有の `src/agent_ipc/transcript.rs` のコメントと文字起こし計画書§10には触れていない。

| コマンド | 結果 |
| --- | --- |
| `cargo nextest run -j 1 -p mimageviewer-mcp --lib client_text_limits --no-fail-fast --success-output final` | 前節と同じrun-heavy・Owner・WorkDir経由。2件すべて成功、0.038秒。上記の最長値と2,048文字超過0件を出力。`target/asr-t1-chunk4/client-text-limits.log`。 |
| `cargo fmt`（引数なし）、`cargo fmt --check` | 成功 |
| `python scripts/check_ui_glyphs.py` | 成功、危険なグリフ0件 |
| `git diff --check` | 成功 |

今回の追加はテストと文書のみのため、前節のMCP lib全件・対象filterの結果を再利用した。
変更対象はMCPのテスト、MCP計画書、本報告書の3ファイル（既存の未コミット差分に追加）。
UI・PNGは変更せず、build-dev・コミット・製品バイナリの起動は行っていない。設計判断との食い違いはない。

- 再レビューP3（2026-11-14）: `schema.rs`でcursorを返す9 toolのdescriptionへ変更・再起動の注意を復帰し、`tests.rs`で全対象を検査。`get_transcript`は既存のrev/epoch拒否の説明を維持した。
- run-heavy経由 `cargo nextest run -j 1 -p mimageviewer-mcp --lib --no-fail-fast --success-output final`: 83件中81成功・既知のnamed pipe制約2失敗（OS error 5）、68.949秒。最長description612文字・INSTRUCTIONS852文字・tools/list最大41,380文字で全上限内、2,048文字超のtoolは0件。
- MCP crate内で `cargo fmt`・`cargo fmt --check`、`git diff --check`成功。コード変更はMCP内2ファイルのみで、本体ソース変更・コミット・製品起動はなし。ログ: `target/asr-t1-chunk4/p3-cursor-descriptions.log`。

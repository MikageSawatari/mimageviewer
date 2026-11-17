# §1.801 全画面を閉じるときの待ちの調査・計装

2026-11-18、`perf-801-776b`。原因修正は行わない。観測源は利用者の v5.3.0 perf ログ
（session `44796-1794917202980`、0～40秒の抜粋）とソース、headless handler テストに限る。
製品アプリの実行・画面操作・commit は行わない。

## 1. 結論と測定の境界

**利用者の約337 msの待ち先は未確定**。AI処理との時間的な重なりだけを原因としない。
有力な原因と呼べるところまでは絞れていない。下記のRemote mutexはコード上の同期到達と
合成競合を示すもので、利用者ログでの保持者や待ち先を確定したものではない。
`load_folder_end.ms` の開始は `load_folder_with_scan_claimed_with_search_origin` 内の
`scan_started` であり、公開された loader 関数の入口ではない。
利用者ログの `lf_scan.ms=3.4279` は走査後の image-book 判定・署名計算まで、
`lf_scan` の発行自体は materialize/sort の後である。
`load_folder_end.ms=18.5698` から、主な空白は `scan_started` **以前**にあると読める。

`upscale_end` は worker の処理終端ではない。`ai/upscale.rs` はこのイベントの後に、
累積バッファから `ColorImage` を作る正規化・画素配列割当・バッファ解放を行う。
`run_final_ai_job` の返却・結果channel送信もその後である。
一方、通常DirectMLタイルの `with_session` guard は各タイル関数から戻ると解放され、
`upscale_end` では既にその区間を抜けている。
したがって約86 ms後のUIログ再開を、そのまま同じORT mutexの解放時刻とは扱えない。

## 2. 同期到達先の棚卸し

対象は main の通常PDFから実親フォルダへ戻る `Direct` 経路。
条件付きの他の帰路も、どこへ分岐したかを小区間名で区別する。

| 到達箇所 | 待ち得るもの・判定 |
| --- | --- |
| `take_pending_return_to_parent_nav` → `take_close_return_navigation` → `resolve_close_return_navigation` | 通常PDFは在メモリの帰路・選択名解決。ZIP/Collection/履歴/Smartの分岐は別区間。帰路なしのcontext退役も別区間。通常PDFでORT/GPU待ちなし |
| normalize / cursor capture / modal・snapshot admission | pathの字句処理と在メモリ状態。fullscreen中のcursor captureは早期return。新しいDB openやファイル内容読取りなし |
| `capture_physical_open_navigation` → history snapshot / current target | 在メモリの履歴cloneとscope生成。合成path生成で `data_dir::get` → `TEST_OVERRIDE` Mutexに到達する。製品にも存在するロックだが通常は短いpath cloneだけで、AI推論中に保持する設計ではない |
| `claim_open_request_owner_with_startup_intent` → `cancel_unresolved_open_for_navigation` | explicit admission時の `remote_session_blocks_local_control` → `SessionHandle::snapshot` が **`inner`、`remote_web_connections` の2 Mutex** を順に取得する。handleがある場合に到達。実ロック保持fakeによるUI handlerの待ちの候補。ローカルAI workerがこのロックを推論中ずっと持つ経路は見つからず、利用者事象の原因とは未確定 |
| 同claim内のarchive/startup/bookmark取消・book/classification/cold-document/history退役 | cancel token、receiverや要求payloadのDrop。`ArchiveConvertState` / `OpenPathClassification` はcancelを立てるだけでthread joinしない。普通のPDFの検証済み状態ではcold-document退役条件に入らない。未完了EPUB等の別要求が存在する場合はそのlease/payloadのDropが条件付き候補で、退役の種類ごとに計測 |
| `classified_kind.unwrap_or_else(|| path.is_file())` | **通常の実親フォルダでも同期Windows属性照会を行う**。OS/ストレージ/フィルタの待ちは可能。AIとの直接の共有ロックや「起動20秒台限定」の根拠はない。`PathIsFile`で単独測定 |
| `scan_directory_with_settings` | `read_dir`、FindFirst/Next、DirEntry属性・metadata。既存同期I/O。利用者ログでは3.4 msの区間に含まれ、約337 msの空白とは異なる |
| `prepare_scanned_folder_listing_with_cache_keys` | 本棚既定pathの `SHGetKnownFolderPath`、favorite設定投影、materialize/sort、rating順なら既存接続のSELECT。rating DB cold openはここにはない。`load_folder_end`の計測内なので主な空白の説明にはならない。従来の `lf_scan.ms` はmaterializeより前に算出するため、区別して計測 |
| `prepare_visible_install` | PDF/EPUB行を含む別の遷移ではEPUB read lease取得があり得る。今回の実親フォルダのfolder行は該当しない。走査後なので約337 msの対象外 |

通常PDFの上記経路に、`recv` / `Condvar::wait` / thread `join` / wgpu `poll(Wait)` /
buffer map完了待ち、ORT session・DirectML deviceの最終Dropは見つからない。
`book_materialization::read_composition_worker` の `recv` は準備worker専用で、
通常実親フォルダを開くこの同期経路からは呼ばれない。
`close_fullscreen` のcache破棄・cancelは `lf_scan` **後**の既存処理で、今回の空白とは位置が違う。

AIの共有資源は次のとおり。

| 資源 | UI帰路との関係 |
| --- | --- |
| `AiRuntime.sessions` | モデルcache/loadとタイル推論が同じMutex。通常PDF帰路の到達先ではない。空の実runtimeのmutexを保持するfakeで検査。既存の別AI入口の `is_loaded/load_model` がUI上で呼ばれる場合に備え、取得待ちを全callerで計測 |
| upscale `SCALE_CACHE` (`LazyLock<Mutex<...>>`) | scale detectのworkerだけ。既知scale参照時とinsert時の短い保持で、初回推論中ずっと保持しない。UI帰路から到達しない |
| `AiRuntimeInitOwner.state` / changed Condvar | ORT初期化自体はstate mutex外。UI向けready snapshotとworker向けterminal待ちを分離。この帰路はruntime初期化・terminal待ちを要求しない |
| `AiJobQueue` Mutex/Condvar | pop中だけqueue lock、推論前に解放。通常closeは個別token/receiverを退役し、queueのjoinや推論合流を行わない。保持fakeでhandlerを検査 |
| AI結果channel / cancel / retained orphan | unbounded送信、UIはtry_recv、cancelはatomic。PDF retained orphanは完走許可でありUIの完了待ちではない |
| ORT/DirectMLと描画wgpu資源 | runtime/sessionと描画rendererの所有者が異なり、この帰路はそれらを最終解放しない。driver内部やOS割当の相互影響はログ/CPU fakeだけでは検証できない |

初回遅延初期化（ORT DLL・モデルsession・scale detect・本棚既定pathのShell初回照会）には
「初めだけ」という性質があるが、worker専用か `scan_started` 以後である。
今回の空白に対応するものは未特定。Windows属性照会やメモリ割当のOS待ちも可能性に留める。
ログ出力は `log_writer` の有界queueへの `try_send` であり、UI上でファイルflushを待たない。

棚卸しの主なソース位置（本差分の行番号）:

- 帰路消費・即時dispatcher: `src/app.rs:24544`, `src/app.rs:24589`、
  帰路の分岐: `src/app/close_return.rs:358`。
- 履歴捕捉: `src/app.rs:25310`, `src/app.rs:26587`, `src/app.rs:31664`。
- claimと属性照会: `src/app.rs:31237`, `src/app.rs:30782`。
- Remote到達: `src/app/startup_ops.rs:1253` → `src/remote_ipc/ui.rs:1059`
  → `src/remote_ipc/session.rs:1895`。
- 計測開始とlf_scan: `src/app.rs:31864`, `src/app.rs:32136`。
- 走査とmaterialize: `src/app/folder_scan.rs:954`, `src/app/book_materialization.rs:455`。
- 本棚既定path: `src/books.rs:699` → `src/capture.rs:208`。
- AI worker/queue: `src/app.rs:10878`, `src/app.rs:10939`, `src/app.rs:104636`。
- session取得と推論: `src/ai/runtime.rs:620`, `src/ai/runtime.rs:822`、
  タイルの呼出し: `src/ai/upscale.rs:933`。
- worker終端との違い: `src/ai/upscale.rs:662`（upscale_end）、
  `src/ai/upscale.rs:681`（その後のfinalize）。
- worker専用recv: `src/app/book_materialization.rs:307`、非blockingログ配送: `src/log_writer.rs:358`。

## 3. 計装の契約

すべて `--perf-log` 有効時のみ。main closeの帰路消費と即時applyにthread-localの計測scopeを置く。
ナビゲーションのowner、保留field、キャンセル規則、入力規則は変えない。
pendingなしの毎フレームpollでは時計取得・イベント発行を行わない。
共有loader内のspanもclose scope外は時計を読まない。detachedの述語・viewport・registryは変更しない。

| イベント | フィールド | 条件 |
| --- | --- | --- |
| `nav/close_nav_stage` | `stage`, `outcome`, `ms`, `n`, `seq`, 既存`tid/t` | close scope内の各小区間が終了した時。早期returnもRAIIで記録。msは子区間を含むので合算しない |
| `sync/lock_wait` | `lock`, `operation`, `ms`, `close_navigation`, `holder`, `seq`, 既存`tid/t` | **取得まで5 ms以上**。lockを保持した処理時間は含まない。実holderの追跡は行わず `holder=unknown` と明記 |

`stage` はenumで固定する。

- 帰路: `TakeReturn`, `ResolveReturn`, `ResolveZipReturn`, `ResolveCollectionReturn`,
  `ResolveSpecialReturn`, `RetireNoDestination`, `ApplyReturn`, `Dispatch`, `DirectOpen`。
- 受付: `OpenAdmission`, `NormalizePath`, `CaptureCursor`, `ModalAdmission`, `SnapshotAdmission`,
  `ClassifyPath`, `OpenClassified`, `CaptureNavigation`, `SmartTarget`, `ClaimOwner`。
- 取消・退役: `CancelArchive`, `CancelStartup`, `CancelBookmark`, `RetireBookOpen`,
  `RetireClassification`, `RetireColdDocument`, `RetireHistory`, `RemoteSnapshot`。
- 読込・準備: `PathIsFile`, `LoadClaimed`, `ScanDirectory`, `OwnerCurrent`, `PrepareListing`,
  `BookRoot`, `FavoriteProjection`, `MaterializeListing`, `RatingRead`, `PrepareInstall`。

`outcome` もenum。`Complete`はその小区間の通常終了であり、ナビ採用成功の意味ではない。
`DirectOpen` / `LoadClaimed` の `Loaded` はloader結果。
`NoDestination`, `UnsupportedNavigation`, `ModalBlocked`, `SnapshotRefused`, `Classifying`,
`AdmissionRefused`, `OwnerStale`, `ScanFailed`, `Ignored`, `VisibilityRefused`, `SmartDispatched`
で拒否/分岐を識別する。

ロック名とoperation:

- `ai_runtime.sessions`: `is_loaded` / `load_model` / `with_session` / `unload_model`。
  close外のworkerや既存UI callerも5 ms以上だけ記録。
- `remote_session.inner`, `remote_session.remote_web_connections`: `snapshot`。
  close scope内だけ計測し、通常の毎フレームsnapshotには時計取得を足さない。
- `data_dir.TEST_OVERRIDE`: `get`。close scope内だけ計測。
- `pdf_loader.epub_pin_coord`: `read_lease_drop` / `try_enter_read`。別の未完了EPUB要求の
  退役・EPUB行の準備に備え、close scope内だけ計測。通常PDF→実親folderには非該当。

次のログでは同一seq/nの最長の子区間と、その中の `lock_wait` を読む。
例えば `CancelStartup` → `RemoteSnapshot` → `remote_session.inner` なら資源まで絞れる。
`PathIsFile`だけが長ければOS属性照会であり、ORT mutexの推測とは分ける。
spanの外側だけが長い場合は、その親区間内の残余（割当/Drop/未分割の呼出し）を次に追う。
OS/driver内部の待ちの根因を、この計装だけで断定できるという保証ではない。

## 4. テストと検証

`src/app/tests/close_nav_801.rs`。実PDFレンダラやGPUを生成せず、使い捨てprofileの本の行から
`request_live` → `take_pending_return_to_parent_nav` → `apply_fullscreen_close_nav_immediate`
→通常親フォルダのscan/採用/closeまで通す。キーroutingや実画面の再現を主張するものではない。
test-only観測channelは実際の取得直前/直後に通知し、perfのglobal有効化や本物のログprofileは使わない。

- `close_nav_801_inference_sessions_held_does_not_block_pdf_return_handler`:
  空の実 `AiRuntime.sessions` を別threadが保持したままhandlerが完了するかを検査。
  ORT・device・モデルは生成しない。
- `close_nav_801_ai_queue_held_does_not_block_pdf_return_handler`:
  実queue mutexを別threadが保持したままhandlerが完了するかを検査。AI workerは起動しない。
- `close_nav_801_remote_state_held_blocks_pdf_return_handler_at_named_lock`:
  `SessionHandle.inner` を保持し、handlerがその取得直前まで来た通知を待って、
  保持中には取得後通知が来ず、解放後に完了することを検査。**資源競合の合成テストであり、
  利用者事象の再現・原因確定ではない**。修正後はこのfixtureを「保持中にもhandler完了」へ変更できる。
- `close_nav_801_modal_refusal_and_unsupported_return_have_typed_reasons`:
  modal拒否・履歴の即時処理対象外の早期return理由を検査。

再現できない候補の扱い:

- ORT session解放・scale cache・wgpu poll/map・AI結果recv/joinは、通常PDFのpre-scan handlerに
  同期入口がない。未使用の待ちをテスト専用にUIへ注入して「再現」とはしない。
- Windows属性照会の長いOS待ち、driver内部、heap/page faultは実環境依存で、
  ただのMutex fakeでは再現できない。今回のheadless試験はその根因を検証しない。
- cold EPUB/startup/bookmark取消のpayloadが存在する別の状態は、通常PDF fixtureの前提とは異なる。
  条件付き候補として退役区間・EPUB lease mutexを計測し、利用者ログで到達の有無から分ける。

実行結果（2026-11-18、実装担当のheadless実行）:

| コマンド | 結果 |
| --- | --- |
| `cargo test -p mimageviewer --lib close_nav_801 -- --nocapture` | **4成功、0失敗、0ignored**、1.46秒。18,614件はfilter対象外で未実行。初回成功時のcompileは10分08秒 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 成功、16分22秒（通常構成の初回依存graphを含む） |
| `scripts/build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | 2026-11-18 01:07:29（ローカル時刻）にDONE。core / remote / EPUB worker / MCP / music-video editorの5本を作成。VCRT/PE検査は `runtime=4 pe=5` 成功。起動はしていない |
| `cargo fmt`（引数なし） / `cargo fmt --check` / `git diff --check` | 成功 |

上のtestは `C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1` 経由、WorkDirは本ツリー。
Ownerは実行時 `codex/perf801-focused`（以後のcheck/buildは指定どおり `perf801-codex`）。
`target/perf801/run-tests.ps1` で `MSBUILDDISABLENODEREUSE=1` を設定し、最終成功時の
`CARGO_BUILD_JOBS=6`。前提のTurboJPEG並列CMake buildはexit101で止まったため、同runner経由の
`cmake --build ... --target install --config Debug --parallel 1` で前提だけ作成し（0 errors）、続行した。
最初の本体test compileでfixtureの必須fieldとprivate field参照の2件を訂正した後の成功である。
新規ツリーのため `vendor/ffmpeg/bin/*.dll` を `target/debug/deps/` へ実ファイルで配置した。
製品test失敗やtimeout中断があった、という結果ではない。
ログは `target/perf801/focused-tests.log`, `native-build.log`, `msbuild.log`。
通常core checkは `target/perf801/run-check.ps1` でjobs=1、同runner経由。
ログは `target/perf801/core-check.log`。

独立 `gpt-6.1-sol / xhigh` は静的レビューACCEPT、P1/P2なし。
非対象の委譲分岐にも `Ignored` が出るP3注記は、`LoadClaimed` の未分割分岐を
小区間通常終了の `Complete` へ戻して対応した。
本体lib全件・実画面試験は実行していない。追加4件はdeviceを作らないのでgpu group追加は不要。

確認用buildも同じ共有runner経由（Owner `perf801-codex`）。既知のCMake失敗を避けるため、
同一 `dev-runtime` profileの `cargo build -p turbojpeg-sys --profile dev-runtime -j 1` を先に行い、
続けて `CARGO_BUILD_JOBS=6` で既存 `build-dev.ps1` を素の呼出しで実行した。
通常feature setであり `portable` は付けていない。
共有枠待ちの後に開始。手順は `target/perf801/run-build-dev.ps1`、DONEまでの記録は
`target/perf801/build-dev.log`、5本のsize・時刻・SHA256は `target/perf801/artifacts.json`、
PE検査は `target/vcrt-pe-reports/dev-runtime.json`。
通常checkのログは `target/perf801/core-check.log`。

計装ログを採取する場合は、本ツリー直下で利用者が実行する（エージェントは起動しない）:

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe -ArgumentList '--perf-log'
```

これは既定で本物の `%APPDATA%\mimageviewer` を使い、実設定・データを更新し得る。
single-instance mutexを共有するため、先にインストール版・tray常駐版を終了する。
対象シナリオは、PDFの全画面を「アニメ拡大」で表示し、AI高画質化の処理中にEsc/Enterで
親一覧へ戻る通常操作。既存設定のまま同じseq/nの `close_nav_stage` と5 ms以上の
`lock_wait` を読む。全画面の再現や改善はこのbuildでは未確認で、修正を検収するbuildではない。
release版との厳密な性能比較の証拠にも使わない。
最終レビューもACCEPTで、保存した4件成功・通常core check成功のログを再実行せずに照合した。
全件gateの省略を利用者が指示した、という意味ではない。今回は原因修正前の診断計装として
実装担当の検証を絞っており、リリース/統合の全件gateの成功証拠は本記録に含めない。

## 5. 修正案（未実装・計装ログで対象を確定してから判断）

1. **属性照会が対象なら**、既存folder preparation workerで親の分類・scanを行い、
   既存のnavigation requestとprepared scan採用へ通す。新しいbool/pendingを足さず、
   現在のrequest ownerを使う。利用者の閉じる操作は即時受付し、既存の読み込み表示を保つ。
   走査完了まで次の本が開けなくなるような新modalは、通常のcloseの自由を失うため採用しない。
   現在の同期adoptionから次フレーム以後のadoptionに変わるので、履歴・選択・取消の既存契約を
   先に設計担当と確認する。新しい状態の種類は増やさないが、既存要求の準備時間は長くなる。
2. **Remoteの状態lockが対象なら**、`blocks_local_control` のためだけにfull session snapshotを
   作らず、既存のLocal/Remote lifecycle ownerが公開する小さい不変projection/phaseを読む。
   operational判定とadmissionの意味を保持し、別の正本やclose専用pendingを作らない。
   表示snapshot・Remote操作の同期規則は維持する。必要な公開値は派生値1個であり、
   新しい状態の組合せや利用者の追加操作は作らない。単に `try_lock` 失敗をLocal扱いする案は不可。

ORT sessions/GPU Dropが名前付きログで示された場合は、この2案を流用せず、その資源ownerに
worker側の寿命を持たせる案を改めて設計する。今回の調査ではUIのORT待ちを根拠に修正しない。
まれな失敗は既定のログ＋通知で割り切り、retry/回復待機の設計は追加しない。

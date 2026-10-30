# 5ch >>655 AI 処理中の連続ページ送り調査

調査日: 2026-10-29。製品コードの修正・製品 exe の起動・デスクトップ操作・コミットは行っていない。
この文書は調査記録であり、原因が確認される前の候補を製品の確定不具合として扱わない。

**結果: 公開版 v4.9.0 と依存更新後の HEAD の各10試行、計20試行で終了症状は未再現。**
AI 未完了のまま10回進める条件、および取消要求後も推論が続く条件は再現した。
原因は未特定であり、製品修正・依存更新による解決済みとの判断はしない。実画面の描画・VRAM・window lifecycle は未検証。

## 対象と報告の区別

- 報告者の観測: 約30枚の ZIP、高速汎用、AI 完了前のホイール連続送りで約10枚先に進むと砂時計になり終了。
- >>641 の初期の複数回終了→後に安定、>>651 の読み込み中の移動・ダブルクリックも前提に含める。
- 「先読みバッファをオーバー」は報告者の仮説。版、画像寸法、GPU、先読み設定、表示モードは未確定。
- 原文 URL は閲覧ツールで取得できなかったため、依頼文に記載された経緯を根拠とする。未提供の返信内容は補わない。

| 区分 | 対象 |
| --- | --- |
| 公開版の基準 | `v4.9.0` = `7fea5839dfe3f3685080ab6956bf1b4a83b57db1` |
| 調査ブランチ | `ai-crash-655`、HEAD `fec29871bc85c19f9fbe64cc2dee3d494742bf21` + 調査用テストのみ |
| 確認時の master（23:27） | `45366aff56bcb811c1c57f0105fdf662f5c03047`。v4.10.0版情報・test-script専用キー修正等は進んだが、調査 HEAD との `src/app.rs` / `src/ai/` 差分なし |
| 現行 native AI | ORT DirectML 1.24.4 / ort rc.15、API 24 |
| 公開版側 native AI | ORT DirectML 1.24.2 / タグ内の patched ort rc.12 |

公開版ソースは Git archive を `target/ai-crash-655/v490/` へ展開した。Git LFS filter を無効化して取得したソースを使い、モデルは用意済みの実ファイルをコピーした。
公開版側の ORT は開発機の既存 DLL を読み取り専用でコピーし、`OrtGetApiBase()->GetVersionString()` が `1.24.2` であることを確認した。
LibRaw はタグの setup script と一致する SHA-256 のローカル保管アーカイブから 0.22.2 を展開、PDFium は保管されていた 8086 を使用する。
FFmpeg / モデル / その他のビルド入力は提供済み vendor から実ファイルをコピーする。ZIP 内 PNG の試験で FFmpeg・LibRaw・PDFium のデコードは通さない。

入力 SHA-256:

- 高速汎用モデル: `09B757ACCD747D7E423C1D352B3E8F23E77CC5742D04BAE958D4EB8082B76FA4`。タグの Git LFS pointer の oid・サイズ4871181と一致
- ORT 1.24.2 DLL: `A2323BC49544645B911743052F1EDCE594E17DF1E3423B71468C7386BC902F80`
- ORT 1.24.4 DLL: `E7EEDEC6A6F26DC39DC948276A75EF6D2BEE3FFF944D874CEED0BBD3B97BFF40`

## 自動試験

試験本体は `src/app/tests/ai_crash_655.rs` の `ai_crash_655_headless_zip_navigation`。
`src/app/tests.rs` の test-only module 宣言以外、既存ソースを変更していない。GPU 試験は明示実行の ignored test とし、通常の gate からは起動しない。

1回ごとに新しいテストプロセスを使う。既存の `phase_c_support::setup_app()` の TempDir、data_dir override と共有直列化ロックを保持する。
DLL・モデル・DB・ZIP・ログはその隔離先に置く。既存 `tests/test_ai_upscale.rs` は実行しない。
`MSBUILDDISABLENODEREUSE=1`。cold build の turbojpeg-sys 並列失敗後、`-j 1` で再実行した。
公開版側は古い lockfile の依存物も再ビルドした。TurboJPEG 成功後、依存コンパイル途中で自分が起動した build process tree だけを停止して `-j 4` へ変更し、完成済み依存を再利用した。
その中断（exit 15）はテスト実行前で、製品／テストの異常終了とは区別する。最終の両 `--no-run` build は exit 0。

30枚の RGB PNG を Stored ZIP にし、実際の ZIP 一覧採用・source worker・ページ移動の共通着地点 `land_still_page_navigation_target`・通常の final composite selector・先読み・eviction・結果回収を呼ぶ。
0→10 の11ページを訪問し、直前ページの AI 完了前に何回進めたかを記録する。初回のモデルロード・推論と、AI 結果保持済みの閲覧を区別する。
設定は隔離環境の新規既定値を用い、AI mode を Light・モデルを高速汎用にする。source先読みは後4／前12、AI先読みは後2／前3、処理上限は2048×2048未満。
ここでの cold は新しい process の ORT / Session / AI 結果 cache の状態を指す。OS のファイルキャッシュや driver の shader cache は消さない。

| ケース | 動作 |
| --- | --- |
| `rapid` | AI 完了を待たずに進む |
| `wait` | 各ページの AI 完了を待つ |
| `off` | AI 無効の対照 |
| `cached` | 0〜10を処理済みにしてから session close 用の live cache 破棄関数を呼び、保持 AI 結果を利用して進む。実ウィンドウの close/reopen は行わない。11枚を収容するため、この対照だけ保持枚数を20にする（既定10枚・512MiB） |

記録: job ID、対象 idx、items generation、取消フラグ、待機キューの画像バイト数、local activity、live/retained 件数、retained bytes、PrivateUsage / working set / peak working set。
`popped_or_result_waiting` は「キューから取り出したが pending に残る」要求であり、モデルロード中・推論中・結果未回収をそれだけでは区別できない。
`local_ai_activity` は待機を含む job lifetime の件数であり、同時推論数ではない。native 実行は perf の `upscale_begin` / tile / cancel / end と合わせて読む。
既存 logger / perf / native exception recorder は隔離先へ初期化し、通常終了・Rust unwind 時にログを artifacts へ保存する。native 強制終了時は標準エラーに表示した TempDir のログも確認する。

**限界**: テスト用 profile であり公開 release exe の試験ではない。OS ホイール入力の受付、実 HWND、swapchain、実描画、egui/wgpu の GPU texture upload、Lanczos 描画は実行しない。
egui texture delta は消費するだけなので、画像表示と DirectML が VRAM を共有する実画面の圧力を、この試験だけで否定できない。
新規テストの診断はすべて test-only で、製品には追加しない。

### 実行結果

公開版 `v4.9.0` / `7fea5839` + 調査テスト、ORT 1.24.2 の10試行はすべて1件実行・exit 0。
実際に隔離先へ展開された DLL の hash も、AI を使った全7試行で上記1.24.2の値と一致した。
非対話の `test` profile（unoptimized + debuginfo）、追加featureなし。公開 release exe の実行結果ではない。

| 公開版の入力 | rapid: 完了前に進めた回数 / Private MiB | wait: 同回数 / Private MiB | off: Private MiB |
| --- | ---: | ---: | ---: |
| 256×341 | 10 / 847.20 | 0 / 855.20 | 202.80 |
| 768×1024 | 10 / 1106.34 | 0 / 1614.75 | 198.64 |
| 1440×1920 | 10 / 2028.52 | 0 / 2763.25 | 270.30 |

公開版 cached は256×341で完了前の移動0回・Private 881.25MiB。Display の0〜10は warm-up 各1回だけで、再訪時に再推論しない。
公開版を含む全20試行で native exception log は0 bytes。AI失敗・panic・CPU fallback のログも検出しなかった。

現行 HEAD `fec29871` の10試行はすべて1件実行・exit 0。通常の `test` profile、feature追加なし。
表のメモリはナビゲーション時点でサンプルした最大 PrivateUsage（GPU dedicated bytes やプロセス全期間の最大値ではない）。

| 入力 | rapid: 完了前に進めた回数 / Private MiB | wait: 同回数 / Private MiB | off: Private MiB |
| --- | ---: | ---: | ---: |
| 256×341 | 10 / 789.86 | 0 / 850.71 | 186.60 |
| 768×1024 | 10 / 1107.64 | 0 / 1617.16 | 187.05 |
| 1440×1920 | 10 / 1837.79 | 0 / 2668.71 | 269.07 |

cached は256×341で実施し、完了前の移動0回・Private 898.77MiB。0〜10の Display 要求は各1回（warm-up）だけで、再訪時に再投入されず保持結果を利用した。2回目の閲覧中も idx11 の通常先読みは開始している。
全試行で native exception log は0 bytes、DirectML EP 登録失敗／CPU fallback のログなし。システムの物理RAMは127.27GiB（GlobalMemoryStatusEx）。低RAM・低VRAM機を試した結果ではない。
1440 rapid の最終ページ完了時は公開版でlive結果2件、現行側では1件だった。実行順と保持量が異なるため、版間のメモリ差をORT更新の改善／修正の証拠として扱わない。

公開版の代表証拠:

- `target/ai-crash-655/v490-analysis.json` と `v490-results.json`: 全10試行の集計、exit、実際の展開DLL hash。
- `target/ai-crash-655/v490-source-check.json`: タグと、展開先のapp / AI関連4ファイル・manifest・lockの一致確認。両版で使った追加テストファイルも同一hash。
- `target/ai-crash-655/v490-7fea5839-rapid-1440.log` と `v490-artifacts/rapid-1440/{mimageviewer.log,perf_events.jsonl}`:
  idx0/job1を22.768sで要求、23.454sから12タイルの処理開始。24.537s付近のidx6入場でidx0のcancelが立ち、24.649sで `upscale_cancel(after_tile=3)`。
  25.688sのidx10入場時には、job10（idx9）が既に処理中、最新job11（idx10）はキュー先頭に待機。世代は1のまま。
  idx9 の処理後、31.993sからidx10を処理し、37.394sに upscale end。取消フラグと実処理の終了が異なること、最新ページより前の近傍ページの実行が残り得ることを記録した。
- 同時点の取消済み queued idx1〜4 は各11,059,200 bytesのsourceを保持。合計約42.19MiBだが、他のsource cacheとのArc共有を考慮せず「追加RAM42.19MiB」とは数えない。

現行側と共通チェックの証拠:

- `target/ai-crash-655/head-analysis.json`: 10試行の exit・メモリ・取消タイル数・Display 要求 idx。
- `target/ai-crash-655/master-rapid-256.log`: 最初の cold 試行。モデルロードが終わる前に0→10へ進み、11 pending / 10 queued。idx0 は取消済みでも処理 owner に残り、最新 idx10 が次に処理された。
- `target/ai-crash-655/head-fec29871-rapid-1440.log` と `artifacts/rapid-1440/{mimageviewer.log,perf_events.jsonl}`:
  idx0/job1 を26.092sで開始要求、26.778sから12タイルの処理開始、27.977s付近の idx6 入場で idx0 の cancel が立つ。
  4タイル目の native 推論が返った後の28.355sで `upscale_cancel(after_tile=4)`。
  idx10/job11は29.248sで要求、29.625sに処理開始、36.739sに upscale end。初回ジョブが取消要求後も実行・回収待ちに残る場合を実際に通した。
- 1440 rapid の10枚先では、取消済み queued idx1〜4 の入力だけで約42.19MiBを保持（1枚10.55MiB）。11 activity は11並列推論を意味しない。
- `target/ai-crash-655/focused-results.json`: 既存の結果回収、PDF orphan、取消済み Ready の context/job 照合、先読み gate、古い retained epoch の拒否の5テストすべてexit 0。
- `target/ai-crash-655/fmt-check.log`: `cargo fmt --check` exit 0。
- `head-temp-cleanup.json` / `v490-temp-cleanup.json`: 実行終了後、今回のstderrに記録された使い捨てdata_dir各10件だけを検証・清掃。DLL／DBの開いたhandleによるTempDirの削除残りを、親側でprocess終了後に片付けた。

完了待ちの対照のほうがメモリ使用量が多くても正常に終わった。したがって、この開発機の結果から「連続送りだけでメモリ量が増えて終了した」とは結論できない。
低RAM機のメモリ不足、実描画中のVRAM圧力、driver固有障害を否定する試験でもない。画像形式は合成RGB PNG、ZIP方式はStoredに限定した。
失敗のまま残した回帰テストはない。全ライブラリ約1.5万件の実行や `test-full.ps1` は行っていない。

開発者向けの再実行例（新しいテストプロセスを1回だけ起動する）:

```powershell
$env:MSBUILDDISABLENODEREUSE = '1'
$env:MIV_655_CASE = 'rapid'
$env:MIV_655_SIZE = '1440'
cargo test -p mimageviewer --lib app::tests::ai_crash_655::ai_crash_655_headless_zip_navigation -j 1 -- --exact --ignored --nocapture
```

公開版側は `target/ai-crash-655/v490/` を作業ディレクトリにし、`CARGO_TARGET_DIR` をこの作業ツリーの絶対 `target` パスへ向けて同じテストを実行した。
実行した test exe は現行 `target/debug/deps/mimageviewer-528eefd81284cb36.exe`、公開版 `target/debug/deps/mimageviewer-877362fec51d7707.exe`。
ビルドログは現行 `target/ai-crash-655-build-final.log`、公開版 `target/ai-crash-655/v490-build-j4.log`。

## ソースで確認したこと

### 公開版と現行版の差

関数単位の抽出比較で、`prefetch_final_ai` / `maybe_start_final_ai` / `run_ai_worker` / `run_final_ai_job` / `has_uncancelled_final_ai_pending` / `cancel_or_orphan_final_ai_for_session` / `mark_cancel_or_orphan_final_ai_for_evict` / `cancel_final_ai_for_idx` は同一。
`src/ai/upscale.rs` と `src/ai/final_pipeline.rs` もファイル全体が同一（改行コードを正規化して比較）。
`poll_final_ai` の差は AI 完了後に取り消す後段 final-effect の reason を渡す部分。
eviction / session close にも同じ後段 final-effect の所有処理の差がある。開始・推論の仕組みそのものを依存更新後の挙動だけから公開版へ推定しない。
ORT 初期化は rc.15 の明示 Environment 所有と API 差への対応が入っている。

### 開始・取消・所有

- 現役は `maybe_start_final_ai` → `AiJobQueue` → 常駐 `final-ai-worker` → `run_final_ai_job` → `ai/final_pipeline` → `upscale_to_max_dim_with_progress`。
  main の `render_prepare_fullscreen_materials` と active viewer 更新から先読み・eviction を呼び、通常 selector から表示要求を作る。
- 旧 `prefetch_ai_upscale` は dead code。`maybe_start_ai_upscale` の製品側呼び出しはその旧先読み内のみ。旧 poll が残っていても、今回の主たる要求 producer と混同しない。
- final worker は1本。`AiRuntime::with_session` の Mutex は他の AI 利用者を含む各 `session.run()` を直列化する。ページを10枚送ったことと、GPU 推論が10本並列で動くことは異なる。
- Display は LIFO、Prefetch は FIFO。先読みは未取消 pending があれば新規要求を止める。表示要求は別ページの pending を全取消せず enqueue できる。
- ZIP は PDF retained orphan の例外対象ではない。keep-set 外では cancel を立てるが、live pending は結果回収まで残す。session close / page invalidation は pending を外して cancel を立てる。
- **取消要求は実終了ではない**。キューから出す前なら worker は `Cancelled` を返して推論を省略する。開始済みではモデルロード、倍率検出用の初回推論、実 `session.run()` はその途中で止まらない。upscale の取消確認は各タイルの前。
- job は source / runtime / manager の Arc と local activity lease を終端まで持つ。ページ・context の解放だけで推論入力や Session が先に解放される経路は見つかっていない。
- worker の Drop は join しないが、キューの共有 owner は worker 側にも残る。UI の取消を GPU の完了・資源返却と誤認してはいけない。

### 結果・世代・context

- FinalAiKey は item identity / items generation / source・編集世代 / 色・AI 設定 / 背景を含み、結果は key + job_id が一致する pending または追跡 orphan へだけ配送する。
- pending / live cache は viewer context とともに移譲され、queue / shared receiver / job sequence は App 側で持つ。退避 context 宛ては既存 backlog へ保留する。
- root の logic service に `service_cancelled_ai_terminals` があり、取消済み Ready を context/job に照合して回収する。表示 poll だけを見て「取り消した結果が常に復活する」と診断してはいけない。
- PDF orphan は最大1件で retained 層だけへ保存し、epoch が変われば破棄する。ZIP の終了原因の説明に PDF の完走例外を流用しない。
- open/switch/close/invalidate、Remote の drain と context Drop も確認対象とした。activity lease は pending の撤去後も実 worker の終端まで残る。

### メモリと上限

- キューは `VecDeque<AiJob>` で、件数・バイトのハード上限がない。取消済みの未着手 job も、worker が取り出すまで source Arc を保持する。LIFO の新要求が先行する間、古い取消済み入力を保持し続け得る。
  これは確認できる資源所有上の懸念であり、「30枚の ZIP で10枚先に行くと終了する原因」の確定とは異なる。
- source decode は既存 process-wide scheduler の6枠（Normal 最大4）を使用する。final AI の実行数1件とは別の上限。
- final AI は長辺8192以内へ直接累積する。出力 RGBA だけでなく、RGB + 重みの4本の f32 累積配列を確保する。モデル倍率検出は高速汎用でも512²の推論を1回行う。
- blender は1本、タイル channel は `sync_channel(2)`。ページ単位 job queue / shared result channel の上限とは別である。
- retained AI は既定10件・512MiB（PDF retained と合算）。live result と retained は同じ Arc を共有するため、その合計を単純加算すると二重計上する。
- live AI CPU cache は keep-set 管理。表示用の GPU texture 会計があっても、DirectML の Session・native GPU allocation や CPU の累積配列まで同じ予算へ収まることは意味しない。
- Session は ModelKind ごとに保持・再利用する。ページ移動ごとに他モデルの Session を排出する処理はない。この試験は高速汎用1種のみで、複数モデル／個別プリセットの利用履歴による累積は未確認。

理論量（4x、clamp 前に長辺8192以下の場合。モデルの native メモリ、タイル出力、入力コピー、表示 texture は別）:

| 入力 | source RGBA | f32累積4本 | 結果RGBA | 累積＋結果が同時にある量 |
| --- | ---: | ---: | ---: | ---: |
| 256×341 | 0.33MiB | 21.31MiB | 5.33MiB | 26.64MiB |
| 768×1024 | 3MiB | 192MiB | 48MiB | 240MiB |
| 1440×1920 | 10.55MiB | 675MiB | 168.75MiB | 843.75MiB |

### 終了の種類

通常の ORT error は `AiError` → `FinalAiResult::Failed` へ返る。通常の worker panic は channel 切断として UI 側に届く経路がある。
一方、native access violation / stack overflow / fast-fail / allocator abort は Rust の Result や通常の panic hook で捕捉できるとは限らない。
「エラーメッセージがなかった」という報告だけではどちらとも決められない。native exception log は first-chance 記録であり、存在自体は fatal の証明にならない。

DirectML の同一 Session の Run 直列化要件等は [Microsoft / ONNX Runtime の公式説明](https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html#configuration-options) も参照した。
この参照から、今回の機器固有の device loss や OOM の発生を推定したものではない。

## 開発機の既存ログ（報告者の証拠ではない）

- 読取り時の `native-exception.log`: 0 bytes。
- `panic.log`: 最終更新 2026-10-19。末尾には viewer context assertion や native video pump stall の記録があり、今回の報告との同一性を示す証拠はない。
- Application / WER: 過去14日・ID1000/1001は94件を読取り、mIV関連は Susie の意図的 crash plugin の AV、v4.8.1 / v4.7.0 の RADAR_PRE_LEAK 等。今回の core + AI 終了の証明にはならない。
- 既存起動ログの wgpu adapter は RTX 4090、driver 32.0.16.1088。これはその既存起動の記録であり、新しいテストの DirectML EP が選んだ adapter の直接照会とは区別する。CIM によるハード情報照会はアクセス拒否だった。
- 利用者の通常 settings / DB / キャッシュは変更しない。インストール済みアプリも停止しない。

## 原因・次の判断

終了の原因は特定できていない。守るべき不変条件は、取消やページ移動後も native 実行中の入力・Session は実終了まで所有し、終端結果は要求元の key / job / 世代へだけ配送すること。
今回その寿命違反や別ページへの結果誤採用は確認していない。取消済みの未着手入力をキューが保持する点はコード上の改善候補だが、今回の終了原因と認定する証拠はない。
製品コードの修正はせず、実画面での process liveness / fault module / CPU・GPU メモリを次の切り分けにする。依存更新で直った、先読みが原因、という結論も出さない。

根本修正を検討する場合も、まず「未着手要求の入力所有」と「native 実行中の所有・会計」を分ける。
不要になった未着手要求が source を保持する境界を直す案は、AI・先読み機能を維持したまま検討できる。
ただし、この所有上の懸念を直せば報告されたプロセス終了も直るとは、fatal の証拠なしには結論しない。
実行中の資源を取消フラグだけで返却したり、症状を消すキャッシュ一括リセットを入れたりしない。

## 実画面を確認する場合の引継ぎ

ClaudeCode が利用者の明示承認を取得してから行う。今回の調査では実行しない。

- 素材: artifacts に保存した30枚 ZIP。まず 768×1024、必要なら1440×1920を使用。
- 環境: 公開版 v4.9.0 の使い捨て portable を準備し、data_dir がその portable の `data` であることを確認。通常設定をコピーしない。
- 条件: 単ページで高速汎用、既定先読み。cold session で AI 完了前にホイールで約10枚進め、現在 idx と実 job の対応を perf/通常ログから確認。
- 対照: 同じ ZIP で AI 完了待ち、AI なし。キャッシュ利用の往復は初回と区別する。表示モードの総当たりはしない。
- 記録: ウィンドウが消えた後のprocess生存（非表示／tray移行と終了を区別）、終了コード、native exception、WER の fault module / exception code、プロセス private bytes、GPU dedicated/shared usage。可能なら fatal dump を開発側で採取する。
- 所要: ビルド・準備を除いて約5〜10分。試験中はテスト用ウィンドウの前面化とホイール入力でデスクトップを使用する。
- 公開版で起きたときだけ同じ限定条件を現行版へ比較する。再現のために実利用データ・設定を変更しない。

報告者への最初の追加照会案は「使用版」と「該当画像1枚の縦×横」。前者は追う実装の版、後者は既定のAI処理対象か・どの寸法で試験するかを変える。
既存ログがあれば先にそこから読み取る。GPU・設定・終了ログが必要になった場合も診断資料1件へまとめ、追加操作の総当たりや「念のため再試験」を頼まない。

## 追加調査: GPU / VRAM と終了理由の表示（2026-10-29）

ClaudeCode からの追加依頼に基づくコード調査・設計案。**実装は未着手、v4.10.0には含めない。**
この追記では製品・デスクトップ・GPU負荷試験を起動していない。前節の20件の headless 成功を再利用するが、それらは描画 device / texture upload / present を通らないため、以下の描画側の故障を否定する試験ではない。

### 結論と確度

VRAM不足・GPU起因で、理由を画面に出さず終了する経路は**ある**。特に描画用 texture の確保失敗が wgpu の未捕捉 `OutOfMemory` になると、既定 handler の panic へ進む経路はコードで確認できる。
一方、通常の ORT error はジョブ失敗として返り、それだけで process を終了させる実装ではない。
**>>655 がこの経路だったという証拠はない。** VRAM不足、TDR、native driver障害は候補であり、現状は原因未特定のまま。

| 条件 | コードで確認した挙動 | >>655への確度 |
| --- | --- | --- |
| ORTが通常の失敗Statusを返す | Rustの `Result::Err` → `AiError` → `FinalAiResult::Failed`。該当keyを失敗扱いにして通常ログへ記録。現役final経路では利用者向け通知なし | この戻り値だけではprocess終了を説明しない |
| 描画texture等の未捕捉OOM / validation / internal error | wgpu既定handlerでpanic。UI/main経路で未回収ならprocess終了。panic hookはログのみ | 無表示終了が可能であることは確認。今回の発生は未確認 |
| `DXGI_ERROR_DEVICE_REMOVED` / RESET | HALは `DeviceError::Lost`。wgpuはdeviceを無効化し、device-lost callbackへ通知する別経路 | **Lostそのものが必ず既定panicへ行くわけではない**。後続surface再設定のpanic等はあり得るが、実際の終端は未確認 |
| native AV / stack overflow / heap corruption / fast-fail / allocator abort | Rust Resultとは別。panic hookを通る保証なし。VEH / WERも記録できない場合あり | 無表示終了は可能。報告者の証拠なし |

### 調査した版と差分

- 公開版基準: `v4.9.0` = `7fea5839dfe3f3685080ab6956bf1b4a83b57db1`。
- 今回の読取り対象: `ai-crash-655` の `4a729bc091b7809f5cbc58fb627ca5a9543e5562`（前節の調査差分をコミット済み）。読取り時の master は `45366aff56bcb811c1c57f0105fdf662f5c03047`。
- `src/gpu_info.rs`、`native_exception_log.rs`、`run_native_failure.rs`、`gpu_startup_diagnostics.rs`、`feedback.rs`、`crates/startup-diagnostics/`、patched `vendor/eframe/` と `vendor/egui-wgpu/` は v4.9.0 と調査HEADで差分なし。`src/lib.rs` の差分は別機能の公開fixture/moduleだけで、panic hook / GPU設定 / 終了処理に差分なし。これらの対象は調査HEADと上記masterでも同じ。
- 両版とも `eframe / egui-wgpu 0.37.0`（vendor patch）、`wgpu / wgpu-core / wgpu-hal 30.0.1`、`winit 0.30.13`。registry内の該当版sourceまで追跡した。
- AI製品側の `upscale.rs` / `final_pipeline.rs` は前節どおり同じ。依存は ORT 1.24.2 + ort rc.12（patch）から ORT 1.24.4 + ort rc.15へ変更。今回追加で確認した上流ORT実装は **v1.24.2 source**。1.24.4のnative内部実装が完全に同じだとは主張しない。現行Rust wrapperの `session/run.rs` も失敗Statusを `?` で返す。

### 推論側: 通常エラーとnative終了を分ける

上流ORT v1.24.2の `ExecutionProvider.cpp` は HRESULT をthrow/checkし、COM入口で `ORT_CATCH_RETURN` を使用する。`InferenceSession::Run` は通常のC++例外を失敗Statusに変換し、C APIの `OrtApis::Run` は `ToOrtStatus(status)` を返す。
したがって、割当失敗がこの通常のエラー伝播を通る場合は、mIVが受け取れるエラーになる。あらゆるOOM、非同期GPU故障、driver内SEH、abortが必ずStatusへ変換される保証ではない。
根拠: [ORT DML provider v1.24.2](https://github.com/microsoft/onnxruntime/blob/v1.24.2/onnxruntime/core/providers/dml/DmlExecutionProvider/src/ExecutionProvider.cpp)、[InferenceSession::Run](https://github.com/microsoft/onnxruntime/blob/v1.24.2/onnxruntime/core/session/inference_session.cc#L2836)、[OrtApis::Run](https://github.com/microsoft/onnxruntime/blob/v1.24.2/onnxruntime/core/session/onnxruntime_c_api.cc#L785)。

mIV側の経路は `src/ai/upscale.rs::run_tile_inference` / `detect_scale_factor` の `session.run(...).map_err(...)` → `final_pipeline::execute_selected_final_ai` → `src/app.rs::run_final_ai_job` → `poll_final_ai` の Failed分岐。
Failed分岐はlive pendingなら失敗key登録とログ・repaint、orphanならログのみ。ここに終了要求、panic、MessageBox、feedback通知はない。EP登録失敗時の既存CPU fallbackは `runtime.rs::register_directml_ep` の別段階で、推論途中のOOMを一般に救うものではない。

DirectML公式仕様は、OOMもdevice-removalの要因に挙げる。DML deviceだけの喪失がD3D12 device喪失を伴うとは限らず、その逆向きは伝播する。このため「AIのDMLエラーが出たら必ず描画deviceも死ぬ」とは扱わない。
[Microsoft: DirectMLのエラーとdevice-removal](https://learn.microsoft.com/en-us/windows/ai/directml/dml-errors)。
同じGPUでも、mIVのORT default DirectML EPとeframeのwgpu deviceは別の所有者である。両者が同じadapterかは、GPU名の推定ではなくLUID等の一致を採らない限り未確認とする。

### 描画側: OOM panicとdevice lossは別経路

1. `vendor/egui-wgpu/src/renderer.rs::update_texture` は新規画像textureを `device.create_texture` で確保する。製品描画経路にはこの割当を囲むOOM error scopeがない。
2. `wgpu-core 30.0.1/src/device/resource.rs::create_texture_inner` のHAL割当失敗は `handle_hal_error_with_nonfatal_oom` で戻る。ここでの「nonfatal」はcore内のdevice無効化を直ちには行わない意味で、mIVが通知して継続するという意味ではない。
3. `wgpu 30.0.1/src/backend/wgpu_core.rs::ErrorSinkRaw` はscopeもcustom handlerもなければ `default_error_handler` を呼び、`panic!("wgpu error: ...")`。OOM以外のValidation / Internalも対象。
4. mIV / vendorの通常描画deviceについて `on_uncaptured_error` の差替えは見つからない。error scopeの検索結果はvendorテストのみ。動画native overlay用の別deviceにだけ `set_device_lost_callback` があり、generation付きlatchを立てるがreason/messageは保持していない (`src/video/native_presenter/overlay_gpu.rs`)。
5. DX12 HALの `auxil/dxgi/result.rs` は `E_OUTOFMEMORY` → OutOfMemory、DEVICE_REMOVED / RESET → Lost。`cargo tree -e features -i wgpu-hal --offline` で `device_lost_panic` / `internal_error_panic` は有効でないことも確認した（compile・製品起動なし）。
6. **LostはOOM panicと別**。wgpu backendの `handle_error_inner` は `ErrorType::DeviceLost` をuncaptured handlerへ渡さずreturnする。coreの `Device::lose` はvalid=falseにして登録済みclosureを呼ぶが、初期値はNone。mIVのroot描画deviceにはclosureがない。この入口だけから「TDR→必ずpanic」とは結論できない。
7. surface acquireのOutdatedは再設定、Lostはsurface再作成要求、Occluded等はskip。これは**surface**の扱いで、device再作成ではない。patched `winit.rs::configure_surface` / runtime installには `expect("runtime surface configuration is fatal")` が残る。device喪失後に再設定失敗・別エラーへ進めばpanic等になり得るが、単に描画が止まる経路も含め、今回の機器での結果は未観測。
8. 初回rootの非対応surfaceや `run_native` が返した `Err` は、既存 `run_native_failure::finish_run_native` によりログ保存・終了処理後にnative MessageBoxを出す（script時は抑止）。これはpanic / native例外を包む処理ではない。

wgpuには予算比率によるresource OOM / device lossの閾値指定もあるが、mIVのinstance設定は `MemoryBudgetThresholds::default()`（両方None）で、独自の固定閾値で喪失させてはいない。OS/native側からの割当失敗・喪失までなくなるわけではない。

Cargo profiles / `.cargo/config.toml` / release scriptsには `panic=abort` 指定がなく、通常Rust panicはunwind。
winit Windowsはcallbackのpanicをcatchして保留し、event loop境界で `resume_unwind` する。mainの `mimageviewer::run()` / `eframe::run_native` を包む最外のcatchはないため、UI側のpanicはmainまで抜けて終了し得る。**常にabort設定だから落ちる、ではない。** 二重panicやunwind不可境界、native異常は別にabortし得る。
AI worker等の背景threadだけの通常panicなら、hookが動いても直ちにprocess全体が死ぬとは限らない。従ってpanic.logの有無も終了の確定印にはできない。
なおCPU側も `upscale.rs` の累積配列4本は `vec![0.0f32; npixels]` で確保し、`try_reserve` 等による失敗Resultへの変換はない。Rust標準allocatorの割当失敗によるabortは通常panicと別で、panic.logが残らない可能性がある。VRAMとprocess private bytesの両方を記録する理由になる。

`src/lib.rs::install_panic_log_hook` はpayload・位置・強制backtraceを構築し、`panic.log` へ直接appendしてから通常loggerへ渡す。panic.logは4MiBで1世代rotation。hook内の画面通知はない。windows subsystemでstderrも通常見えない。
この既存hookには割当・filesystem処理があるため、極端なOOM時やstack破損時まで保存できる保証はない。新しいfatal証拠はこの重い処理**より前**に短く残す設計とする。

### 「砂時計→消失」とTDRの位置付け

**推測として症状に合う経路**は、同じadapterを使うAI負荷やメモリpagingにより、UI threadのsurface acquire / queue submit / present / texture割当などのnative callが待たされ、続いてOOM panicやdevice loss後の別panic、またはnative障害で終了する経路。
`egui-wgpu::Painter::paint_and_update_textures` はUI描画中にacquire / submit / presentを行い、DX12 HALにもswapchain待機、fence待機、`Present` があるため、AIがworker上でもUIがGPU待ちになる可能性はある。OSの「応答なし」表示に相当するか、実際にどのcallで待ったかは未確認。
CPUメモリ圧迫や別の同期停止も似た見え方になるので、砂時計だけをTDRの証拠にはしない。

TDR既定2秒は、schedulerがGPU taskを完了またはpreemptできるまでのtimeoutである。**`Session::run` の経過時間やタイル全体のCPU wall timeと同じではない。**
弱いGPU・driverのpreemption特性・長いdispatchによってAI由来のTDRが起こる可能性はあるが、タイルが2秒以上かかった事実だけでは判定できない。
[Microsoft: WDDM TDR](https://learn.microsoft.com/windows-hardware/drivers/display/timeout-detection-and-recovery)。
高速汎用は `model_tile_size` で512²、初回の倍率検出も512²の実推論。通常タイルはoverlap32、画像端も固定入力へpaddingする。取消はタイル境界であり実行中のRunを中断しない。
従って「取消要求済みなのにGPU処理継続」は正常にあり得る。画像寸法が大きいと主にタイル数・CPU累積配列が増えるので、画像寸法をタイルdispatchの巨大化と同一視しない。
TDRが起こった、OOMが起こった、同じGPUだった、の3点はいずれも報告者について未確認。TdrDelayの変更や機能削減は提案しない。

### 既存診断の記録条件と不足

| 仕組み | 現在取れるもの・時点 | 不足 / 注意 |
| --- | --- | --- |
| `gpu_info::emit_vram_trace[_with_options]` | **perf有効時だけ**、呼出点でDXGI local/nonlocal Usage / Budget / Reservation等。任意でPDH Dedicated / Sharedも取得、失敗は一度ログ | 現在の製品callsiteはvideoのdecode/drop/render経路。静止画final AIの連続ページ移動に常時sampleはない。PDHは比較的重い。DXGIも列挙・COMを含みfatal内では不可 |
| `query_dxgi_video_memory_snapshot` | 最初のsoftware以外・専用VRAM>0のadapter、node0を照会 | wgpu/ORTの選択adapterと照合しない。専用VRAM0のUMAを除外し得る。全GPU合計でもAI専用使用量でもない |
| `native-exception.log` | data_dir初期化・single-instance受付後に事前open、VEHでAV/stack overflow/breakpoint/illegal instruction等7種類。時刻、thread、code、address、AVアクセス種別、version、exe/ntdll base。1行2048bytes、1session256件、起動時1MiB超でrotation | first-chanceで `CONTINUE_SEARCH`、捕捉済み例外も含む。HRESULTのOOM / DEVICE_REMOVEDはnative例外とは限らず通常ここに出ない。fast-failはVEHを通らない場合あり。job/VRAM/device reasonなし |
| `panic.log` | hookが呼ばれたRust panicの要約・位置・backtraceを常時直接append | UI通知なし。threadの帰結、run idとの強い相関、GPU予算/対象jobなし。背景thread panicや捕捉されたpanicも記録 |
| 通常log / perf | 通常logは常時、AIの要求・実行・cancel境界やFailedの文字列。perfはopt-inで詳細時系列、writerは非同期・有界 | crash直前のqueue未排出分は失われ得る。通常logにはパスを含み得る。perfの全件提出は利用者負担が大きい |
| startup diagnostics / GPU startup capture | 常時の起動timeline、run/role/pid、adapter選択、初回root描画段階、process.terminal。GPU依存Errorの限定捕捉はInstance〜最初のRootPresentedまで | 起動完了後のGPU失敗は対象外。`ProductDiagnostics` は通常runtimeのwgpu HAL/core Errorを保存しない。device removalの詳細HRESULTを一般に保持しない |
| UI heartbeat watchdog | 5秒以上App updateがない疑いを通常ログへ記録 | 待っているGPU callを確定しない。killもしない。logger停止やprocess即死では残らない場合あり |

`CurrentUsage > Budget` は圧迫・pagingの手掛かりで、即OOMの証明ではない。予算は変動し、sampling間のpeakも見逃す。
[Microsoft: DXGI_QUERY_VIDEO_MEMORY_INFO](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_4/ns-dxgi1_4-dxgi_query_video_memory_info)。
逆に直前sampleが予算内でもnative一時割当失敗を否定しない。VRAM数値だけで「VRAM不足で終了しました」と表示しない。

### 設計案の比較と推奨範囲

**推奨は(a)+(c)、および(b)のうち既にResultとして返るORT失敗の通知。**
目的は理由を取り出せるようにする診断であり、device再作成、再推論、retry、キャッシュ一括破棄、AI無効化の回復処理は含めない。

| 案 | 利点 | 制約・規模 |
| --- | --- | --- |
| (a) 次回起動の前回終了通知＋コピー | 落ちたUIに依存せず、利用者は短文を貼るだけ。常時有効なら事前の再現設定が不要 | 中規模。終了境界、前回資料の読取り、privacy-safe要約、表示とコピー、単体/子process試験 |
| (b) 受け取れた失敗を即時通知 | ORT Failedは既存feedbackを使い小規模で説明可能 | wgpu OOM / device lossを一般に握りつぶして描画継続する案は不採用。失敗textureは利用不可で、device無効の場合もある。描画復旧は別の大きな設計 |
| (c) 軽い直前状態＋fatal時保存 | cancel済みRun継続、現在ページと実job、VRAM圧迫を区別できる | 中規模。(a)と共通基盤。fatalだけのdumpではfast-fail/kill/電断を捕捉できないため、通常時の非同期保存も必要 |

#### (a) 終了印と表示の責務

- launcher / worker / help / 二重起動activationを除く、**GUI coreの実行単位**で1つの診断ownerを持つ。既存startupのrun ID（launcher共有相関がある場合はcore role/pid/開始時刻も含む）へ結び付ける。正常設定DBへboolを追加しない。
- single-instanceのfirst admissionとdata_dir確定後、起動workerが前回の小資料を読み、上限・schema・runを検査してメモリへ渡す。UIはpollして表示するだけ。通常ログのrotationや今回の記録で前回資料を上書きする前に前回1件を確保する。同じprocessが生存するleaseは異常終了に数えず、前回版に資料がない初回は表示しない。
- 状態は例として `Running / ReturnedOk / ReturnedError` を1つの型で保持し、run id・version・開始時刻・最終更新時刻を付ける。`ReturnedError` は制御されたエラー終了で、即時通知済みなら次回同じ警告を重ねない。
- cleanの確定はmainの明示した最終終了境界でのみ行う。ウィンドウ非表示、tray移行、App Drop、unwind中のRAII Dropではcleanにしない。既存 `finish_run_native` / `main` / `exit_startup` の実際の早期return・終了コードを棚卸しする。正常な終了が実行中AIをdetachして終える既存仕様も異常と扱わず、AI完了待ちを新設しない。
- `--play-test` は `src/app.rs` から直接 `process::exit(0)` しmain境界を通らない。これを含む試験modeは診断対象外と明示するか、意図した試験終了として同じownerへ通知する。GUI roleを持つという理由だけで対象に含めない。
- `miv_startup::terminal` はfirst-outcomeを守るが、`shutdown_until` の既定Cancelledや別roleのterminalもあり、単なるterminal存在を正常終了印に流用しない。通常終了印の書込み失敗もあり得る。最終保存は既存の終了時deadline内でworkerへ依頼し、UI frame内やfatal内でbarrier待ちをしない。記録失敗を回復する待機・再試行は作らない。
- 前回Runningのままなら「**前回の正常終了を確認できませんでした。原因は記録されていません**」が基準。taskkill、OS終了、電源断、診断保存失敗とcrashを区別できない。対応するpanic / device loss / first-chance例外があれば「終了前に観測された情報」として添える。確実な終端証拠なしにfirst-chance例外やworker panicを終了原因へ昇格しない。
- UI表示は初回の通常root描画が可能になってから、消えない小さな通知・詳細欄に「報告用の文をコピー」「閉じる」を置く。起動中の操作overlayを塞がない。コピーは明示操作時のみ、送信は自動化しない。直前資料は前回1件だけを保持し、表示済み/閉じるの扱いも同じownerへ集約する。短時間のtoastだけでコピー機会を失わせない。

表示・コピーの例（数値・IDは架空、全項目必須ではない）:

```text
前回の正常終了を確認できませんでした。
終了前の記録: 描画用GPUメモリの確保に失敗 / Rust panic
mIV x.y.z (build ...), Windows ..., ORT ..., GPU ... / API DX12
直前GPU使用量 1700 MiB / 予算 1900 MiB（採取0.8秒前・描画adapter）
表示11/30, 実行job42: 6/30, 高速汎用, 入力1440x1920, tile512x512
job42: 取消要求済み・Run終了未確認。記録ID ...
※使用量の値だけではメモリ不足を確定できません。
```

既存 [startup diagnostics](startup-diagnostics-plan.md) のrun・時刻・role・非blocking publisher / writer・保存先解決・保持方針を再利用する。通常版の起動診断保存先はLOCALAPPDATA、portable/明示data-dirはそのdata/logs/startup配下なので、診断のためにUIへAPPDATAの同期I/Oを増やさない。
既存の [run_native Err通知](render-init-failure-report-plan.md) と二重通知を避け、次回起動も描画できない場合は既存nativeエラー表示までが限界。crash中のMessageBoxを保証する設計にはしない。

#### (c) 記録内容とfatal経路

- 通常時は固定容量の直近イベント（例32件、文字列を含め全体16KiB程度を上限）と最新snapshotを保持。現在表示のcontext/page/items世代と、queue要求・worker pick・実Run開始/終了・取消要求・実terminalを分ける。要求job ID、model/backend、source/output/tile寸法、tile番号、in-flight件数、queue件数・所有入力bytes、CPU private bytesも対象を絞って採る。未計測値は0で偽装せずunknown。
- UIからは既にある整数・enum等をメモリへpublishするだけ。画像を再decodeせず、queue/session lockを計測のために待たず、文字列のパス整形やCOM照会をしない。worker側のRun直前/直後で状態を刻むため、UI上のpending撤去をGPU完了と誤認しない。初回model load / scale detect / tile Runも区別する。
- GPU memory samplerは背景workerで、AI活動中など必要な期間に約1秒間隔から開始。PDHの毎回open/enumerationは常時経路に転用せず、まず選択adapter/nodeに結び付いたDXGI Usage/Budget（local/nonlocal）、採取時刻、照会成否を取る。LUID取得不能・DMLのadapter未確認・UMAでも欠落を明記し、別GPUの数値をAIのVRAMと呼ばない。sampleの年齢をコピー文に含める。待機はworker通知付きとし、新しいUI repaintやUI sleepを作らない。
- 小資料の保存もworker所有。dirty時の周期checkpointと主要境界をcoalesceして上限付きstateを置換し、前回1件＋今回1件だけ保持する。Disk停止でもproducerを止めず、記録失敗はdiagnostic-unavailableとする。通常ログqueueに入っただけのイベントを「保存済み」と表示しない。詳しいperfは従来の任意診断として残す。
- root device生成直後にdevice-lost callbackを登録し、reason/messageを有界・path非含有の情報へ変換してpublishする。明示destroyを異常に数えない。callbackはwgpu再入・resource解放・再作成・UI呼出しをしない。uncaptured handlerを診断用に置く場合も、既定のfatal契約を無条件の継続へ変えず、記録して同じpanicに進む範囲から始める。
- wgpu callbackの `DeviceLostReason::Unknown` と一般messageだけでは、nativeな `DXGI_ERROR_DEVICE_HUNG` 等は取れない場合がある。詳細が必要なら生きている各device所有境界で `GetDeviceRemovedReason` を採る設計を別途限定する。wgpuはHAL公開範囲/vendor境界の確認が必要、ORT内部DML deviceは現在mIVが直接所有しない。**別deviceへの照会を代用したり、常に詳細理由が取れると約束しない。** [D3D12のGetDeviceRemovedReason](https://learn.microsoft.com/en-us/windows/win32/api/d3d12/nf-d3d12-id3d12device-getdeviceremovedreason)。
- panic / native handlerは上記の完成済みsnapshotと読取り可能なring要素を、小さな事前open済み別sinkへbest effortで保存する。既存 `native_exception_log` の固定buffer、再入時skip、直接 `WriteFile`、件数上限を再利用する。一般loggerを呼ばず、alloc / Rust lock / TLS / COM / DXGI / backtrace / thread待機 / flush待機を足さない。既存のfirst-chance方針と `CONTINUE_SEARCH` を維持する。
- producer停止中でもfatal readerが待たずに読めるpublication契約が必要。非atomic payloadをseqlockの検査だけで並行readする方式はRustのdata raceを避けられないので不採用。固定primitive atomicと世代確認、読めないslotはskip等の上限付き方式を独立レビューする。ringの完全取得を優先してhandlerを待たせない。
- job ID・寸法・段階は同じpublication単位で照合し、異なるthreadの更新を継ぎ合わせない。単純な二重buffer切替も読取り中bufferの再利用問題があるため、それだけでは採用しない。一度の整合性検査が失敗した項目はunknownとし、fatal handlerで読めるまでspinしない。
- fatal追記と通常state保存は別sink/runで相関し、遅れたcheckpointがfatal証拠を消さないようにする。panic記録は「観測」であり自動的なcrash印ではない。fast-fail / TerminateProcess / 電源断ではhandlerが走らないので、前回checkpointまでしかない場合を正常に扱う。これを埋める監視process・dump強制採取・WER検索の自動実行は初期案に含めない。

#### (b) 捕まえられた失敗の通知範囲

ORT Failedの詳細を安全な短文へ変換し、現役jobの所有contextへ既存 `show_feedback_result_on(..., FeedbackKind::Error)` 等で通知する案は小規模。背景prefetchや古い取消jobの失敗を現在ページの失敗として出さず、同一失敗を連打しない既存key寿命へ結び付ける。再試行やAI設定変更は追加しない。短い通知から同じ診断詳細・コピーパネルを開ける形にすると資料を再要求せず済む。

wgpuにcustom handlerを付けるだけでpanicを消す案は採らない。scopeでOOMを受け取れても、eguiのtexture登録・delta適用・bind group・後続描画に不完全な資源を渡さない所有境界が必要になる。
device loss後に同じdeviceで通知UIを描ける保証もない。安全な制御終了へ変えるなら、eframe/vendorの描画中断と終了境界を先に設計し、次回通知へ渡す別作業とする。device再構築・自動API切替・CPU再推論を診断追加へ混ぜない。

#### 個人情報・規模・検証

コピー文と常時の小資料は、**ファイル名、ZIP内名、絶対パス、検索語を保存しない**。ページ番号・総数・寸法・model・短命context/job/run IDで相関する。ORT error / panic payload / driver messageはパス等を含み得るため、生文字列を無条件にコピーせず、分類・HRESULT・安全な要約・長さ上限を適用する。panic source位置もbasename程度に留め、識別できない場合は一般文とする。既存詳細ログはローカルに残るが、自動添付・送信しない。GPU型番・driver/version・メモリ値を含むコピー内容は押す前に見えるようにする。

規模の見積りは、(a)+(c)で**中規模・数日程度の実装と独立レビューを要する別作業**。startup診断の拡張、実行owner/終了印、AI publish、GPU sampler、主device callback/vendor接点、fatal-safe snapshot、UI詳細・コピーとテストが対象。native詳細HRESULTまで確実に取る部分は別段階で見積もる。(b)のORT通知だけは小規模だが、単独ではprocessが消える理由を取れない。

将来の自動検証はすべて一時data_dirで行い、正常return / 制御されたErr / second-instance / worker / tray非終了 / worker panic / main panic / 強制終了 / stale・破損資料 / fatalとcheckpointの競合 / adapter不一致 / sample欠落・古さ / privacyを重点化する。panic/nativeは同じtest exeの限定子processと注入したformatter/sinkで検証し、本物のGPU OOM・TDRを自動で起こさない。UI文・コピーはsnapshot/純粋テストで確認する。
今回の追記は文書のみなのでbuild-devは作成せず、既存headless試験も再実行しない。実装承認や実画面再現の承認をこの設計調査から推定しない。報告者へは現段階で設定変更や追加の総当たり試験を頼まず、診断を導入できた版で自然に再発した際に「画面の報告文をコピーして貼る」1操作で資料を得られる構成を目指す。

限定独立レビュー（`gpt-6.1-sol` / `xhigh`）でLostとpanicの区別、終了印のGUI process所有、fatal-safe記録順序を再確認した。指摘された `--play-test` の直接終了とsnapshotの複数thread整合性を上記へ反映。製品変更はなく、文書の `git diff --check` が成功。

## 確定設計 (2026-10-30)

対象は [backlog §1.482](next-release-backlog.md)。ClaudeCode の範囲決定と、独立設計レビュー REVISE の P2 4件を反映した設計の正本とする。**この節の確定は実装完了・独立レビューの再受入れを意味しない。製品コードはまだ変更しない。** >>655 の原因は引き続き未特定。

前節の比較・調査記録は履歴として残すが、競合する提案は本節で置き換える。特に (c) の fatal reader 用 atomic snapshot publication、ring、seqlock、二重 buffer は作らず、fatal handler は最新の表示・job・GPU 状態を読まない。「直前」「採取0.8秒前」という旧例も鮮度の保証には使わない。

### 1. 採用範囲と簡素化

- (a) GUI core の終了印と、次回起動の消えない通知・詳細・コピーを追加する。
- (c) 通常の背景 worker が必要な活動期間だけ、目標約1秒周期で小さな checkpoint を置換保存する。最新値へ coalesce し、イベント全履歴は持たない。panic/native の短い観測は別 sink、device loss は単一 atomic latch から通常 worker が保存する。
- (b) Result として捕まえられる ORT 失敗は、現役 job の所有 context に既存 feedback で即時通知する。
- device 再作成、再推論、再試行、AI 無効化、キャッシュ一括破棄、監視 process、dump/WER の自動収集を追加しない。既存の推論・先読み・取消・PDF orphan・表示・tray の挙動を維持する。

簡素化の検討: 診断のために閲覧をモーダル化すると、AI 完了前のページ移動という既存機能を制限するため採用しない。代わりに、正常時の保存を単一 owner/writer に集約し、fatal 時との snapshot 受渡しをなくす。参考状態が古い・欠落することを仕様として表示する。パネルは前回1件の不変な表示モデルを描き、現在の閲覧状態への live 追従は作らない。

まれな保存・置換・初期化の失敗は、今回の範囲決定に従い**通常経路でログのみ**とする。追加の通知・回復 UI、再試行、保存完了を待つ追加機構、設定や保存済みユーザーデータの削除・再作成は入れない。handler 内は失敗ログさえ出さない。保存失敗を別の原因分類に細分化しない。

### 2. GUI run の所有・終了印・保存 worker の寿命（P2-4）

#### admission と対象外

`main` が未 admission の診断 owner（仮称 `GuiRunDiagnostics`）を作り、`lib::run` に明示的に渡す。owner 自体の作成には I/O を伴わせない。既存 startup run ID に **core role + core PID + core 開始時刻**を加えた組を GUI run の identity とする。launcher と run ID を共有しても別 process を混同せず、PID だけの一致を採用しない。startup crate から必要な identity/保存先だけを受け取る小 API を追加し、startup の terminal を clean 印の代用にしない。

対象は data_dir が確定し、通常 single-instance の first admission に成功した GUI core のみ。二重起動 activation は admission 前に戻し、前回資料の読取り・消費・rotation・今回印の作成を一切しない。launcher、help/version、PDF worker、TensorRT build/infer worker、`--trt-smoke-test` も対象外とする。

`--play-test`、実際に `--test-script` を実行する mode、`--dcomp-presenter-test` は、single-instance 迂回や直接 exit の有無にかかわらず**通常の終了印・前回通知から除外**する。test-script feature 付きでも通常起動なら対象。除外 mode は通常の終了印/checkpoint/前回通知資料を読まず書き換えず、既存 test evidence と詳細ログは維持する。診断自体の検証には後述の一時 data_dir 専用 fixture を使い、この除外を製品フラグで迂回しない。

first admission の guard は run のローカル変数だけに置かず、main の診断 owner に移して最終化まで保持する。run が戻ってから clean 保存までに次の通常起動が割り込まないようにする。生存 lease がある run は未終了として扱い、異常終了通知を作らない。

#### 起動と保存先

admission 後、診断 worker を1本起動する。通常版は既存 startup と同じ保存先解決（LOCALAPPDATA）、portable/明示 data_dir/fixture はその data 配下を用い、namespace は解決済み data_dir ごとに分ける。設定 DB への bool 追加や既存詳細ログの全件読取りはしない。

worker の初期化順序は **前回小資料・対応 native 区間を上限付きで読む → schema/run/lease を検査して前回表示モデルをメモリへ確保 → 今回の記録先を準備 → Running を一度保存 → 必要な古い診断資料を整理**。native recorder の今回 open/rotation/登録も、この前回資料確保より後へ移す。通常 logger の本文や旧 panic.log は今回要約の採用元にしない。UI は初期化完了を非blocking poll し、worker を待たず起動を続ける。事前 open が完了する前の異常は新しい小証拠を残せない限界として扱う。既存の詳細 panic hook はその間も維持する。

保存は今回/前回の2 run 分だけ。run 固有の記録先と、今回を指す小さな参照を worker が管理する。checkpoint/終了印は同じ小ファイルを一時ファイル経由で置換する。run ごとの保存先にすることで、終了予算を超えて残った旧 worker の I/O が新 run のファイルや参照を上書きしない。終了開始後は参照の更新・世代整理を行わない。参照・各ファイルのサイズ、schema、件数、identity を検査し、不明・破損・欠落は不明として扱う。資料のない旧版からの初回起動では警告しない。

#### main の最終境界

終了状態は単一 enum `Running / ReturnedOk / ReturnedError` で所有する。最終化要求には結果と、既存の制御エラー通知の扱いを typed に添える。`ReturnedError` は異常消失とは区別し、既存 `run_native` Err 通知の対象は次回同じ警告を出さない。

現状 `finish_run_native` は cleanup・terminal・`finish_logs()`・必要な native 通知を済ませてから main に戻る。**今回の writer は startup writer と別の、main owner 所有の writer** とし、`finish_logs()` では停止させない。既存 Err の「ログ排出後に通知」の順序は保持する。main は run の結果を受け取り、通常ログの最終処理など panic し得る終了作業を済ませた最後に、診断 owner へ一度だけ `ReturnedOk` または `ReturnedError` を確定する。App Drop、unwind 中の RAII Drop、window close、非表示、tray 移行では確定しない。main を抜ける直前の小さな処理まで終了理由を必ず取れる保証はしない。

既存の合計2秒の終了 deadline を明示的に共有できる形へ接続する。前段の `finish_logs()` で既に費消した時間を取り戻す新しい2秒待ちは作らず、main 最終境界では残予算だけを使う。予算超過・writer 停止では join・flush・再試行を追加せず終了する。通常の状態でも UI frame 内では待たない。最終保存が失われると次回 Running のままになる可能性は明記する。

最終要求は通常の coalesce データに紛れ込ませず、owner の吸収的な終端遷移として受理する。単一 writer が進行中の Running 書込みを終え、残る最新値を一度だけ組み込み、終端を最後に保存して停止する。終端確定後の producer 更新・遅着 checkpoint・device-loss latch は Running を復活させず、終端ファイルを置換しない。AI worker の detach は維持し、正常終了に AI 完了待ちを加えない。

`exit_startup` / `exit_with_logs` と admission 後の直接 exit も実装時の接続対象。対象 GUI の意図した早期 exit は、終了コードに対応する同じ明示最終化へ接続してから exit し、未 admission/除外 mode では何もしない。現状の呼出元（GPU 引数エラー、起動前提エラー、試験終了等）を一覧化して取りこぼしを試験する。未知の直接 exit を RAII clean で隠さない。

### 3. checkpoint の時刻・活動期間・受渡し（P2-1）

**約1秒は保存の目標周期であり、「最大約1秒前」の保証ではない。** worker の遅延、採取元の更新停止、活動外の終了で資料はもっと古くなる。最後の checkpoint 以降のページ移動、job 交代、取消、メモリ変動は失われる。

各レコードは run identity に加え、checkpoint 採取時刻、各表示状態・job 状態・CPU sample・GPU sample の採取時刻を持つ。UTC と run 内単調経過を使い、checkpoint の保存時刻を他項目の採取時刻に代用しない。GPU の前回成功値を残す場合も、その時刻と最新照会失敗を残す。未取得は unknown とし、0 bytes や現在時刻で補わない。

表示の基準は「最終記録時刻: …／終了との間隔不明」。同じ run の panic 等の**対応する観測時刻**があり、同じ単調時計で前後関係を確認できる項目だけ「panic 観測の1.4秒前」などを出す。native の wall clock だけの値、時刻逆転、対応不明では間隔不明とする。device loss の callback では採時せず、worker が latch を確認した時刻を「保存 worker の観測時刻」として扱い、callback 発生時刻・終了時刻とは呼ばない。**次回起動時刻との差から終了何秒前かを計算しない。**

活動判定は UI の pending 件数ではなく、以下の実所有を含めた run 全体の観測とする。

| 期間 | 判定・終了の境界 |
| --- | --- |
| 受付・queue・実 worker | enqueue 済み未回収 job、既存 local activity lease、worker が保持する実 job。model load、scale detect Run、tile Run、blend を区別する |
| 取消・PDF orphan | cancel 要求や pending 撤去で活動終了にしない。実 Run の帰還、worker 終端、結果の回収/破棄まで採る。PDF retained orphan の完走・retained store も含む |
| AI 結果後 | 結果待ち/backlog、所有 context への採用、後段 final-effect 合成、表示用 GPU 登録要求から既存登録境界の復帰/破棄まで採る。texture 登録要求を GPU 実完了・present 成功と呼ばない |
| 活動終了後 | 最後の完了/破棄状態を1回 checkpoint に含め、周期起床・DXGI 採取を止める。AI 結果が cache に残るだけでは活動扱いにしない。後の再表示で GPU 登録する期間は再開する |

producer は既存所有境界で短命の診断活動 lease/件数を渡し、結果・後段要求へ必要な範囲だけ引き継ぐ。診断用 lease は画像や Session の寿命を延ばさず、処理を待たせない。UI の件数とは別に queue/worker/結果適用の活動を追跡し、main/退避 context/PDF orphan のどこでも実作業が残れば採取を維持する。Remote 等の別 producer が同じ runtime を使う場合も、実 worker 活動を含めるが表示ページは unknown/別 context とする。worker の異常終端は lease 解放と一般 panic 観測に落とし、未完了を成功に変えない。

通常時の受渡しも、`run + context ID + items世代 + job ID + model/backend + 段階 + source/output/tile寸法 + tile番号 + 採取時刻` を一つの typed record にする。要求時の不変 metadata を job に添え、同じ実行 owner が段階と組にして更新する。cancel は同じ job ID の別観測として扱う。表示ページと実行ページを混ぜず、異なる job の ID と寸法を項目別に継ぎ合わせない。全ページや cache 本文を毎秒走査せず、既存カウンタ・更新点から queue 件数/所有入力 bytes、実行件数、CPU private bytes を採る。Arc 共有分を追加 RAM として単純加算しない。

受渡しは通常 thread 用の有界 latest-value mailbox と通知で十分で、fatal reader 用の仕組みはない。短い保護区間で固定上限の record を複製し、**保護を解いてから serialize・DXGI・I/O**を行う。UI producer は一度の非待機更新を試みる方式とし、競合時は欠落を数えて諦める。活動の開始/終了は実所有のカウンタ/lease を正本として、metadata の欠落で誤って idle にしない。結果が多い場合は代表 job と合計・省略数を残し、保存上限に合わせて job を止めない。

初回 Running、活動開始/終了、device-loss latch 確認、main 終端で保存要求を出し、通常の高頻度更新は最新へ coalesce する。idle 時も表示状態は既存の変更機会にメモリへ渡せるが、毎秒保存や定期 repaint を起こさない。device loss のための新しい常時 timer は作らず、既存 device 保守の通常 thread/イベント経路から latch 確認を促し、活動中なら周期でも確認する。完全 idle からそのまま即死して worker が確認できない場合は記録なしを許容する。

### 4. GPU sample の意味と負荷

DXGI sampler は背景側だけで、既に選択された adapter の LUID/node に相関して local/nonlocal Usage/Budget を照会する。既存 `gpu_info::query_dxgi_video_memory_snapshot` は最初の専用 VRAM 付き adapter を選ぶので、そのまま「AI の VRAM」に転用しない。描画 adapter と ORT/DirectML adapter は別に相関の可否を持ち、ORT 内部 device への直接照会や新しい device 作成を足さない。

一致確認できた側だけ「描画 adapter」「AI と同一 adapter」と表記し、DML 側不明なら「AI adapter との一致未確認」。UMA を専用 VRAM 0 の理由で除外せず、local/nonlocal の区分をそのまま出す。LUID/照会不可は unknown。sample ごとに成功/未取得/古い値と採取時刻を持つ。PDH の毎回 open/列挙は常時経路に使わない。予算超過・古い sample だけで OOM/TDR/終了原因を断定せず、予算内でも失敗を否定しない。

checkpoint は通常2～4 KiBを目標、schema 全体16 KiBを上限とする。活動中毎秒保存なら約7～14 MiB/時、上限16 KiBなら約56 MiB/時の論理書込み量になる（計算値であり実測ではない。置換に伴うファイルシステム等の追加書込みは別）。idle は周期起床なし。初期化・保存失敗時に UI の同期 fallback を作らない。

### 5. panic/native/device-lost の安全契約（P2-2・P2-3）

#### panic と描画エラー

panic hook の最初で、借用した `&str` / `String::as_str()` を既知の固定分類へ変換して、小さな別 panic sink に直接記録する。既存 hook の payload 複製、location 整形、backtrace 構築、panic.log append、一般 logger より**前**に行う。新しい分類・書込みで任意の `Display` / `Debug` を呼ばない。長い payload は走査量にも上限を設け、未分類は「Rust panic（詳細は分類できませんでした）」だけとする。生 payload・source path は新しい小資料に残さない。

安全契約は **通常 worker で事前 open、固定 buffer、再入/競合時 skip、件数上限（例16件/run）、1件につき単発 WriteFile、一般 logger/flush/再試行なし**。run identity は初期化済み不変値だけを使い、UTC・同じ run 内単調時刻・thread ID・既知分類と数値だけを固定長で書く。alloc、Rust lock 待ち、TLS 初期化、COM/DXGI、backtrace、worker/barrier 待ちを追加しない。未準備 sink は skip。製品用 handle/buffer は process 終了まで存続し、停止時に hook と close/reopen を競合させない。

ただし単発の同期 [WriteFile](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-writefile) に**実行時間の上限はない**。best effort は保存成功や即時復帰の保証ではない。panic/既存 VEH の短い直接記録に限る割り切りで、通常 UI と device-lost callback の保存には使わない。

OOM の確かな分類は描画用 wgpu の typed `Error::OutOfMemory` を観測できる境界に置く。必要な uncaptured handler の接続は既定の fatal 契約を保つ最小変更とし、固定分類の panic に進める。Validation/Internal/一般 panic も分類を区別する。任意 payload に「memory」があるだけで OOM と断定しない。panic を握りつぶして描画継続する変更はしない。worker panic・捕捉 panic は process 終了と同義でなく、ReturnedOk なら次回異常終了通知を作らない。

#### native 例外の run 相関（P2-2）

既存 `native-exception.log` は追記型で、従来行には run/PID がない。その末尾を今回の根拠として採用しない。今回から recorder 初期化時に `schema/run/core PID/core開始時刻/role` の境界 header を加え、**各例外行にも同じ不変 identity** を固定 buffer で付ける。GUI 以外や除外 mode の既存 recorder も自分の identity を付け、GUI の区間へ混入させない。header 準備に失敗したら新しい相関資料としては利用しない。

前回 worker は今回の native rotation/open より前に、前回 GUI identity に一致する区間をメモリへ確保する。読取り対象は current/bak の固定上限内だけとし、別 run、identity のない旧行、header 不整合、途中で切れた行、相関できない資料は採用しない。該当なしを過去の末尾で補わない。コピーへ渡すのは例外 code・既知分類・観測時刻だけで、address/target/自由文を渡さない。

既存 VEH の固定2048-byte buffer、再入時 skip、256件/run 上限、単発 raw append、`CONTINUE_SEARCH` を維持し、最新 checkpoint は読まない。first-chance は捕捉されて続行した例外も含むので、表示は「終了前に観測された例外（first-chance）」に留める。AV 等の記録があっても終了原因の確定にはしない。fast-fail/allocator abort/kill/電断は記録できない場合がある。

#### device-lost callback は単一 atomic latch（P2-3）

root 描画 device の生成境界で callback を登録する。callback が行うのは、reason enum を固定値へ写して**単一 atomic に最初の非正常理由を latch する1回の CAS**だけ。message の複製・文字列加工・ファイル書込み・logger・採時・通知 lock・wgpu 再入・resource 解放・device 再作成・UI 呼出しをしない。明示 destroy は異常観測に含めない。wgpu-core の callback は内部 lock 保持中にも呼ばれるため、同期 WriteFile を置かない。

通常 worker が latch を読み、worker の観測時刻と固定文を保存する。callback の message は新資料に保存せず、`Unknown` は unknown のままで TDR/DEVICE_HUNG へ推定しない。詳細 HRESULT がその device の既存所有境界で取れないなら省略する。ORT 内部 device や別 adapter の removed reason を代用しない。既存 overlay device の latch/復旧経路は変更せず、root device と混同しない。

device ごとの不変 identity を callback 登録時に結び付け、旧 device の callback を新 device の理由へ付け替えない。latch は「handler が snapshot を読む publication」ではない。即死して worker が読む前なら理由は失われる。それを救う fatal reader、callback 内 I/O、常駐監視 thread は追加しない。

panic・native・通常 state/device-loss の sink は分離する。通常 writer が device-loss 観測を追記する小資料も件数/容量を制限し、checkpoint の置換対象に含めない。**後の観測・checkpoint が先の証拠を置換消去しない**。別 sink も同じ GUI identity でだけ結び付け、観測の存在を clean 状態より優先して「異常終了」に昇格しない。

### 6. 次回通知、許可項目、表示とコピーの例

前回 Running のままなら「前回の正常終了を確認できませんでした」を基準とする。情報なしでは「原因は記録されていません」。観測があれば「終了前に観測された情報」として添え、taskkill/OS終了/電断/記録失敗と crash の区別は保証しない。

UI は通常 root の描画が可能で、起動 overlay を塞がずに出せる段階に、消えない小さな通知と詳細欄を一度表示する。tray/最小化中には表示済みを消費せず、復帰後に表示する。「報告用の文をコピー」「閉じる」は常時到達できる位置に置き、本文だけ scroll する。前回1件の提示済み/閉じるは診断 owner が管理し、通知を毎 frame 再生成しない。今回も描画不能なら既存の native エラー表示が限界。自動送信・自動 clipboard 更新はしない。

**checkpoint とコピー文の両方を許可項目から組み立てる。** 許可するのは schema、製品/build/依存版、OS の数値版、短命 run/context/job/device ID と世代、時刻、既知 model/backend/API enum、ページ番号/総数、段階/取消/件数、寸法/bytes、照会結果/相関可否、既知の失敗分類と数値 code。adapter は vendor/device 数値 ID と役割で識別する。GPU 名・driver/ORT/panic の任意文字列は流し込まず、必要な版も数値/既知形式だけにする。

ファイル名、ZIP 内名、絶対/相対パス、検索語、コマンドライン、ユーザー名、画像内容、任意の model 名、panic/ORT/driver の生 message、backtrace/source path は許可しない。パスらしい部分を除去する文字列加工で安全にしようとせず、未分類は固定の一般文へ落とす。既存詳細ログの記録方針は維持するが、自動で添付しない。worker が一度作った同じ本文を画面とコピーに使用し、コピーだけに隠れた追加項目を入れない。

例1（架空値。typed OOM 分類と同一 run の panic 観測がある場合）:

```text
前回の正常終了を確認できませんでした。
終了前に観測された情報: 描画用GPUメモリの確保失敗 / Rust panic
panic観測時刻: 2026-10-30 12:34:56.400 UTC
最終記録時刻: 2026-10-30 12:34:55.000 UTC（panic観測の1.4秒前）
表示状態の採取時刻: 2026-10-30 12:34:54.800 UTC（panic観測の1.6秒前）
表示: context3・世代7・11/30
実行状態の採取時刻: 2026-10-30 12:34:54.900 UTC
AI job42: 6/30・高速汎用・DirectML・tile Run中
入力1440x1920 / 出力5760x7680 / tile512x512・3/20
job42: 取消要求済み・Run終了未確認
GPU採取時刻: 2026-10-30 12:34:54.600 UTC（panic観測の1.8秒前）
描画adapter: vendor 0x10DE / device 0x2684 / DX12 / node0
local使用量1700 MiB / 予算1900 MiB、nonlocal使用量80 MiB / 予算4096 MiB
AI adapterとの一致未確認
CPU採取時刻: 2026-10-30 12:34:54.700 UTC / private 2100 MiB
mIV x.y.z / ORT 1.24.4 / 記録ID core-1234-5678
各状態は最終記録以降に変わった可能性があります。
観測された情報だけでは終了原因を確定できません。
```

例2（終了観測がなく、古い checkpoint だけの場合）:

```text
前回の正常終了を確認できませんでした。原因は記録されていません。
最終記録時刻: 2026-10-30 10:00:00 UTC / 終了との間隔不明
表示状態の採取時刻: 2026-10-30 09:59:59 UTC / 表示11/30
GPU採取時刻: 2026-10-30 09:59:58 UTC / 描画adapter local使用量1700 MiB・予算1900 MiB
AIの実行状態: 不明 / AI adapterとの一致未確認
mIV x.y.z / 記録ID core-1234-5678
使用量の値だけではメモリ不足を確定できません。
```

native の証拠があれば、例2へ「終了前に観測された例外（first-chance）: access violation / code 0xC0000005 / 観測時刻 …。終了原因かは不明」を添える。欠落項目は省略または不明を明示し、架空の値で補完しない。

### 7. ORT の即時 feedback

`AiError` → `FinalAiResult::Failed` → 所有 context への既存配送・採用境界で、現在の要求 key/job ID/世代が有効な現役 Display job だけ `FeedbackKind::Error` へ接続する。例:「AI処理に失敗しました（推論エラー）。」typed に分類できるメモリ確保失敗ならその固定文を使い、未分類 ORT 文字列は一般文にする。背景 Prefetch、取消済み/古い job、PDF retained orphan、別 context の結果は現在ページのエラーとして出さない。同一 key の既存失敗寿命で重複通知を防ぎ、結果の配送・失敗 key・repaint の契約は維持する。

即時通知の詳細/コピーも許可項目から同じ本文を作り、既存 feedback から開ける診断詳細へ接続する。通知時間は既存 feedback ポリシーに従う。通常 Result の失敗を終了印にせず、device loss や native 例外の証明にも使わない。CPU fallback の追加や再試行は行わない。

### 8. 実装段階と検証所有

実装は後続の承認された依頼で開始する。各段は同じ設計 owner の範囲内で進め、担当実装者が focused 検証を所有し、独立 reviewer は同一 source/条件の成功証拠を再利用する。これは本追記で新たな実装・製品起動を許可するものではない。

| 段 | 実装範囲 | 自動検証・受入条件 |
| --- | --- | --- |
| A 終了印＋次回通知 | main owner/admission、専用 writer、前回読取り、単一状態/終端、許可項目 formatter、通知/コピー | 一時 data_dir の純粋状態テストと限定子 process。正常 return/制御 Err/早期 exit、二重起動で資料不変、除外 mode 全種、tray は Running、App Drop/unwind は非clean、AI detach 正常終了。mainまでwriter生存、終端後の遅着Running拒否、終了予算でjoinしない、旧writerが新run不変。初回/生存lease/stale/schema不明/破損/書込み失敗。headless UI snapshot とコピー本文一致 |
| B checkpoint＋GPU sampler | typed job/表示 record、実活動から後段登録までの lease、活動中の周期保存、時刻、adapter相関 | fake clock/sampler/sink で目標周期・coalesce・idle周期起床なし、遅延や古い採取元、間隔不明/時刻逆転、採取時刻の個別保持。取消後Run継続、pendingゼロのPDF orphan、結果backlog/退避context/後段登録、世代・job交代で寸法混在なし。adapter一致/不一致/UMA/unknown/照会失敗を注入。共有保護中にserialize/DXGI/I/Oしないことをfixtureで確認 |
| C panic/native/device-lost | 事前openの短いpanic証拠、native run相関、root deviceのatomic reason latch、worker保存 | 同じtest exeの限定子 processでworker/main panic注入、worker panic後に正常終了、強制終了でRunning残留。hookの重い既存処理より先に固定記録、再入skip/上限/未準備sink/単発短書込みを注入。nativeは既存子process/固定formatter試験を拡張し、複数run/旧行/別mode/rotation前確保/不完全行の誤相関なし。callbackはfakeでCASのみ、重複理由・明示destroy・保存前終了・旧deviceを確認。panic/native/device観測が互い/遅いcheckpointで消えない |
| D ORT 即時通知 | 現役DisplayのFailedをfeedback/詳細へ接続 | handler級試験で有効key/job/contextだけ1回通知、prefetch/取消/orphan/別context/staleは通知なし。正常Resultと終了印を混同しない。パス/ZIP名/検索語を埋めた未分類payloadを注入し、checkpointと表示/コピーの全出力に含まれないことを検査 |

全試験は TempDir/明示 data_dir とローカル sink を使い、通常の settings/DB/ログに触れない。panic/native handler の process-global 状態は子 process ごとに隔離する。実 GPU OOM/TDR、通常製品 exe の起動、デスクトップ操作は自動 gate に含めない。過去20件の headless 成功は調査証拠として再利用するが、今回の診断機能・実描画の検証済み証拠には転用しない。

将来の実装検証は focused → 必要な shared 回帰/full gate の順とし、[開発ビルドとテスト](development-build-and-test.md) に従う。UI snapshot は1093×614/1366×728、倍率100%/200%、長文/unknown/閉じる/コピー操作の到達性を含め、fmt/UI glyph検査も行う。native/device 接続は実機未確認を残し、実装後に検証バイナリを build して利用者へ渡す。agent の実画面検証は別途、具体的 suite の明示承認と使い捨て portable の準備が必要。本段階は文書のみなので build・テスト・製品起動は行わない。

### 9. 変更ファイルの見込みと UI 応答性確認

| ファイル/領域（実装時の見込み） | 責務 |
| --- | --- |
| 新規 `src/previous_run_diagnostics.rs` 等 | GUI owner/状態、専用 writer、checkpoint schema、許可項目 formatter、前回読取り・typed表示モデル、panic sink/device latch。必要なら小moduleに分割 |
| `src/main.rs` / `src/lib.rs` | owner引渡し、first admission/guard寿命、除外mode、hook前段、早期exit/最終境界、共有deadline接続 |
| `src/run_native_failure.rs` | 制御Err通知の扱いをownerへ渡す接続。既存ログ排出→native通知の順序を保持 |
| `crates/startup-diagnostics/src/` | 既存identity/保存先/lease/deadlineの必要最小の公開。startup writerを診断writerの代用にしない |
| `src/app.rs` / `src/app/` / `src/ai/{upscale,final_pipeline,runtime}.rs` 等 | job/実Run/取消/結果/後段登録の観測点、既存活動情報、現役Failedのfeedback。dead legacy producerへ主経路を誤接続しない |
| `src/gpu_info.rs` / `src/native_exception_log.rs` | adapter指定sampler、native header/各行identityと読取りformatter試験 |
| root device生成の `vendor/egui-wgpu` 接点等 | typed uncaptured errorのfatal契約とdevice-lost callback登録。最小のobserver接続でrenderer復旧を変更しない |
| `src/ui_dialogs/` とそのmodule宣言、関連headless test/snapshot | 消えない前回通知、詳細、本文共有、明示コピー/閉じる |
| 本書、backlog、startup/async/UI応答性/仕様の該当文書、manual/製品ページ/privacy | 実装段階で診断内容・保存範囲・限界を同期更新。本依頼で変更するのは本書だけ |

[UI応答性 §4](ui-responsiveness.md#4-新機能追加時のチェックリスト) に照合した設計上の確認: **UIへ同期I/Oを追加しない**。前回資料の read/open/metadata/rotation、事前open、serialize/置換保存、DXGI/COM、CPUメモリ照会はすべて背景側。UIは固定上限の整数/enum record の非待機受渡し、表示モデルのpoll、既存texture登録点の観測、描画と明示clipboard操作だけを行う。診断のためのdecode、DB open/検索、全フォルダ/cache走査、追加GPU upload、session/queue待ち、sleep、周期repaintはない。main最終境界以外でbarrier待ちをせず、fatalの限定直接書込みは通常UI経路へ流用しない。detached述語/viewport所有/表示切替は変更対象に含めず、接続上それらへ入る必要が判明した場合はrework規則に従って別途設計合意を取る。

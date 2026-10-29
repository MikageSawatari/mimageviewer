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

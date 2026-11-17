# GPU device / session テストの並列数制限

## 対象と境界

`.config/nextest.toml` の `gpu` group は `max-threads = 2` とする。
仮説は **GPU device / 実行 session の同時作成が GPU driver の停止に関係した可能性（未確認）**。
2026-11-16 の設計判断に従い、実 device / GPU session の作成、または driver の GPU 実行経路へ
到達するテストに限定する。情報照会・instance 作成だけのテストは含めない。
これらが停止と無関係だと断定したわけではなく、今回検証する仮説の範囲外として扱う。

全体の `test-threads = "num-cpus"`、retry、timeout、priority は維持する。
制限単位は nextest のテストプロセスで、1テスト内部の device / worker 数を制限しない。
別の nextest 起動とは枠を共有せず、`cargo test` はこの設定を読まない。
製品の挙動と ignore 指定は変更しない。生成入口にはテストビルド限定の所属検査だけを追加する。

## 含める経路

§1.776 B-A の `gpu_lanczos::gpu_tests::still_nis_dxbc_matches_wgsl_pixels` と
`still_anime4k_vl_dxbc_matches_wgsl_pixels` は `gpu_tests` prefix で自動所属する（Windows 限定、ignore なし）。
DX12 adapter / device / PASSTHROUGH_SHADERS の欠落は panic で gate を失敗させ、黙って skip しない。
同じ adapter から feature なし/あり device を作り、製品 WGSL/DXBC の prepare・encode・readback を比較する。
wgpu 更新時の register 配置と passthrough 契約の変化を検出する目的で常設する。
Linux の WGSL 検査とは分け、Windows の GPU gate で必ず実行する。

名前ではなく、生成点から helper と呼び出すテストを追った。モデル・adapter・環境変数などの
条件が成立すると生成へ到達するテストも含む。既存 ignored テストは通常の全件では実行しない。

| device / session の経路 | 対象テストと判断理由 |
| --- | --- |
| `gpu_lanczos` → `request_device` | `ordinary_prepare_output_limit_fallback_captures_actual_managed_allocation`、`back_native_execution_rechecks_no_removal_before_backing_claim_or_mutation`、prewarm の実 state を使う2件。plan / shader 検証だけのものは除外 |
| `OffscreenGpu::new` | compare の実 callback 4件、depth renderer 4件・focus sampling 2件、panorama の実 callback 1件。uniform / 幾何 / shader 文字列テストは除外 |
| `depth_view_tests::renderer` / `install_pages` | depth view 38件、`RootDepthViewer::{new,linked}` を使う main-window 9件、setup-entry 8件、export 12件、fullscreen depth / focus 11件、`DepthWaitViewerFixture` の呼び出し元。Windows の `install_pages` はモデル状態によらず device を作る |
| 深度ダイアログの macro → `RootDepthViewer` | `depth_dialog_owner_scenario!` の12件。helper は grid / linked / independent / parked / export の分岐より前に device を作る。前回期待一覧の macro 生成名の漏れを、今回は展開した完全名で補った |
| final-prepare / canonical / page-turn の実 renderer | `app::final_prepare_budget::native_tests` の13件、canonical draw-key 3件、backward native handoff 1件、実 native ROI 1件。予算・diagnostics の状態検査・CPU final 登録だけのものは除外 |
| compare-crop の shader 分岐 | `crop_ready_compare_shader_wipe_affordance_precedes_dim_frame_and_handles` だけ。残り3件は `shader=false` なので state を作らない |
| LazyRenderer → WgpuTestRenderer → RenderState → `request_device` | `Harness::render` / `snapshot` / `try_snapshot` と推移的な helper。lib は module / 完全名、UI統合テストは binary 単位で選ぶ。builder / run / input / query だけでは初期化されない。CPU adapter 優先でも device を作るため含む |
| shared `ui_harness` の実 renderer テスト | `context_root_fixture_panel_matches_raw_root_without_wrapper_margin`、`render_state_enumerates_only_product_backends` を各 importing binary で含む。後者は名前に enumeration とあるが実 device を作る |
| `D3D11CreateDevice(WARP)` | NIS draw、Anime4K draw、ignored compile benchmark の3件。実 D3D device を使い、件数も少ないため含む（通常2件・ignored 1件） |
| D3D11 hardware / DComp | ignored timestamp benchmark、native-window pump と thread-spike の ignored stall テスト。実 hardware / DComp device を作る |
| FFmpeg hardware encoder | clockless EOF 2件、ignored encoder probe、深度MP4出力4件、musicvideo-editorの実worker開始1件。`hw_decode=false` やCPU frameでも encoder がAutoならNVENC / QSV / AMF / MF 経路を試す |
| ORT DirectML EP session | AI upscale / tile-blend 統合テスト6件、ignored DA3 oracle、ignored `ai_crash_655_headless_zip_navigation`。後者は隔離先へ出荷モデルをコピーする。runtime / ORT environment の初期化だけとは区別する |
| VLM worker の実 Vulkan 推論 | `real_model_serve_matches_v1_0_on_all_26_images_when_configured`。環境変数で実モデルを指定すると GPU 実行へ進む |
| vendored egui-wgpu / eframe | egui-wgpu の capture / renderer / winit の実 state 14件。eframe は `retirement_texture_delivery_wgpu_startup_before_full_and_free_only` と ignored 実窓 `synchronous_damage_returns_before_outer_paint_gate` の2件 |

## 含めない経路

| 経路 / テスト | 含めない理由 |
| --- | --- |
| DXGI factory / adapter 列挙、`QueryVideoMemoryInfo`、VRAM budget / accounting | 情報照会だけでは device / GPU session を作らない。thumbnail、continuous、page-turn、final prepare、frame bookkeeping の CPU headless テストも、この理由だけでは所属させない。別に実 renderer を作るテストは上表の理由で含む |
| CUDA `cuInit` / SM・Compute Capability 照会 | context / session を作らない照会だけなので対象外 |
| wgpu Instance だけの作成 | eframe の17件と egui-wgpu の `renderer_absence_retains_both_sets_and_frees_for_existing_pending_owner`。実 `Painter::new` は呼ぶが `request_device` へ進まない |
| CPU headless frame / logical native presenter / fake slot・pool | Context や HeadlessEpiIntegration で UI を進めても実 renderer がない経路は device を作らない。fake reader の diagnostic sampler も対象外 |
| CPU shader / uniform / geometry / cache state | shader 文字列・数値計算・fake TextureId だけの検査では device を作らない |
| ORT environment・Session::builder・CPU tensor API 検査 | GPU EP 登録と model session 作成まで進まない。通常の隔離 App fixture は model_path が None、failed owner、fake worker 等を使う |
| TRT / CUDA worker protocol、pack、lifecycle の既存テスト | fake child / 状態検査で、実 TRT / CUDA EP session を作るテストは見つからなかった |
| software-only FFmpeg tests | 個別に `hw_decode=false` を確認した decoder / thumbnail / tile / strip fixture、OpenH264明示のmusic-video / segmenter。audio-only Auto はvideo encoderを作らない。Remoteの実H.264や外部HW probeは下記のとおり別に含む |
| WebView2 worker の既存テスト | 不正引数、URL / PDF / process-record helper 等で、有効な変換の `webview::Host::new` へ進まない。将来の実変換テストは対象にする |
| lab / editor main、開発 probe bin | main を呼ばない unit-test harness や `test=false` のツールは実 GUI / device 起動と区別する。現行 eframe glow tests は context 作成を注入した fake を使う |

repository の Rust に直接の `D3D12CreateDevice` / MF DXGI device manager 呼び出しはなく、
DX12 は wgpu、MF は FFmpeg `h264_mf` 経由。情報照会の除外は仮説の境界であり、
それらの API やテストが停止に無関係だという実験結果ではない。

## 製品側の生成点の検査（2026-11-16、再レビューP1）

`a4ab64a5c` のテスト専用入口だけの検査では、製品の関数の奥で作るdeviceを取りこぼした。
今回、既存のテスト専用37入口に加えて、製品側32か所へ **`#[cfg(test)]` 限定**で
`crate::test_gpu_group::assert_gpu_test_group()` を追加し、test-only module内の2か所も補強した。
新規34入口で、製品ビルドの挙動は変えない。

| 製品側のファイル / 入口 | 生成点と条件 |
| --- | --- |
| `src/video/decoder.rs` / `try_init_d3d11va_for_codec` | codecのD3D11VA対応確認後、shared HW context / FFmpeg-owned `av_hwdevice_ctx_create` の全分岐より前。非対応codecは検査前にreturn |
| `src/video/gpu_renderer/d3d11_device.rs` / `GpuVideoDevice::new` | hardware D3D11 device作成前 |
| `src/video/gpu_renderer/ffmpeg_d3d11.rs` | shared `av_hwdevice_ctx_init` と `av_hwframe_ctx_init` 前の2か所 |
| `src/video/native_presenter/{render_core,resample_pipeline,overlay_gpu}.rs` | presenter D3D11、DComp、Anime4Kの計測device、overlayのwgpu `request_device` 前の4か所 |
| `src/video/native_window_thread_spike.rs`（test-only moduleの補強） | hardware D3D11 / DComp の2か所 |
| `src/video/stream/encoder.rs` / `open_ffmpeg_encoder` | 実codec open前。OpenH264は除外し、NVENC / QSV / AMF / MF を検査。mock ladderは迂回 |
| `src/ai/runtime.rs` | DirectML / TensorRT・CUDA EP登録前の2か所。CPU EP・force_cpu・environment作成だけ・cached session取得は除外 |
| `src/ai/trt_worker_pool.rs` / `WorkerHandle::spawn` | concrete TRT子workerの起動前。fake factoryは迂回 |
| `src/ai/vlm_worker.rs` / `VlmWorkerClient::load` | 有効なload要求を実 / 未知workerへ送る前。spawn / hello / GPU情報照会だけは除外 |
| `src/ai/asr_worker.rs` / `AsrWorkerClient::load` | 有効なLUIDのload要求を実 / 未知workerへ送る前。`transcript_job_worker::run` の起動・hello後もこの入口を通るため、ジョブ側・共有transportには重ねて置かない |
| `src/gpu_lanczos.rs` / `Lanczos3Resampler::new` | 借りたdeviceで実GPU pipelineを作る前。pure plan / cache検査は除外 |
| `vendor/egui-wgpu/src/lib.rs` / `RenderState::create` | CreateNew分岐の実`adapter.request_device`直前。instance / adapter照会・Existing setup・mock候補closureは除外 |
| `vendor/eframe/src/native/{wgpu_integration,glow_integration}.rs` | wgpuの4つの実window登録境界と、実GL contextを作るclosure。set_window(None) cleanup / instance-only / mockは除外 |
| `vendor/eframe/src/web/{web_painter_wgpu,web_painter_glow}.rs` | RenderState / WebGL1 / WebGL2の3生成点。Windows構成ではweb targetは実行しない |
| `src/lib.rs`、`crates/musicvideo-editor/src/lib.rs`、`crates/vst3-host-tester/src/main.rs` | 各crateから実`eframe::run_native`へ入る4か所。dependencyのcfg(test)非伝播を補う |
| `crates/musicvideo-editor/src/mp4.rs` / `export_snapshot` | core libが通常依存なので、自crateでもAuto / hardware encoder要求を検査。OpenH264は除外 |
| `crates/epub-pdf-worker/src/webview.rs` / `Host::new` | WebView2 environment作成前。version probeだけには置かない |
| `src/epub_convert.rs` / 実native worker spawn | 通常exeの子にはcfg(test)が入らないため、親のConvert要求も検査。Inspectと既存の明示fake argsは除外 |
| `crates/epub-pdf-worker/src/main.rs` / `Host::new`呼出し | binの通常lib依存にはlib側cfg(test)が入らないため、自crateでも検査 |
| `src/musicvideo_launch.rs` / 実editor起動の入口 | 通常GUI子exeのGPU初期化を親で検査。generic fake launch / spawn seamは変更しない |

共通検査は [tests/support/gpu_test_group.rs](../tests/support/gpu_test_group.rs)。
`NEXTEST=1` のときだけ `NEXTEST_TEST_GROUP == gpu` を要求する。
欠落・`@global`・別groupの場合は、設定への追加を促すpanicの案内と入口の位置を表示し、
**テストプロセスをexit 101にする**。Remote workerはjoinのErrを捨て、AI workerはpanicを
通常errorに変える経路があるため、workerのpanicだけではlibtestの失敗を保証できない。
案内のstderr出力が失敗してもexitは実行する。製品のworker / join / error処理は変更しない。
nextest以外では検査しない。環境変数を変更しない純粋な判定単体テストで、nextestなし / gpuの許可と
group欠落 / global / 別名の拒否、設定案内を固定している。

根拠: [nextest 0.9.143の環境変数仕様](https://github.com/nextest-rs/nextest/blob/cargo-nextest-0.9.143/site/src/docs/configuration/env-vars.md)。
`NEXTEST` は常に `1`、`NEXTEST_TEST_GROUP` はgroup名または`@global`（0.9.90から）。
最低版0.9.143の仕様を確認し、実行版は0.9.148。

VLMのCPU fakeも製品load APIを使うため、`cfg(test)`だけのmarkerで区別する。
既存fake fixtureが初期化した共有`OnceLock<PathBuf>`の`get()`と完全path一致だけでfakeと認める。
検査がfakeをbuildすることはなく、未知・実workerは既定で検査する。caption側の重複fake準備は
同じ共有fixtureへ統一した。製品の型・動作とtestcaseのassertは変わらない。

### 文字起こしの入口（2026-11-16、ASR区切り9）

ASRも `cfg(test)` だけのmarkerを起動時に持ち、VLMと同じ `vlm_worker::tests::is_fake_worker`
で既知fixtureを判定する。判定の複製や別のfake登録はせず、未知・実workerへのload前に所属を検査する。
`transcript_job_worker::run` は `AsrWorkerClient::start_at` → hello → loadを使うので、
実workerのジョブもここで検査される。spawn / hello自体はGPU sessionを作らない。

VLM/ASR共通の `worker_gpu::{inventory,probe_inventory,system_vulkan_loader_available}` は
DLLの確認、DXGI列挙とworkerの `--list-devices` による情報照会である。
workerの `Inventory` とggml Vulkanの列挙・properties/memory照会まで追い、Vulkan instanceと
physical-device情報だけで、logical device / backend sessionを作らないことを確認した。
既存VLMと同じく列挙には検査を追加しない。T1/T2のASR transportと作成ジョブのテストは
既知の同じfake workerを使い、抽出・後処理・DB・検索はASRの実GPU推論を開始しない。

ただしT1/T2で加わった次のsnapshot 10件は、共通 `test_ui` のLazyRendererから実wgpu deviceを
作る。masterの一覧には無かったためgpu groupへ追加した。CPUだけの表示・入力・状態テストを
module単位で巻き込まず、次の完全名だけを選ぶ。max-threads=2、timeout・priority・retryは変更しない。

- `global_search_ui::transcript_target_ui_tests::transcript_search_target_combo_handler_and_dark_light_snapshots`
- `transcript_panel::tests::transcript_panel_snapshots`
- `ui_dialogs::about::tests::ai_worker_credits_snapshot_light_and_dark`
- `ui_dialogs::asr_addon::tests::asr_setup_confirm_snapshot`
- `ui_dialogs::transcript_delete::tests::transcript_delete_confirm_snapshot`
- `ui_dialogs::transcript_job::tests::transcript_progress_snapshot`
- `ui_music_panels::transcript_session_tests::music_transcript_panels_{dark,light}_snapshot`（2件）
- `video::native_presenter::overlay_draw::tests::native_transcript_panels_{dark,light}_snapshot`（2件）

区切り9の焦点検査は、共有runner経由の
`cargo nextest run -p mimageviewer --lib -E 'test(/asr|transcript|vlm_worker|caption_job|test_gpu_group/)'`
（成功stdout保存の指定を付加）で **245件成功、失敗0、10.571秒**。
ASR / VLM fake、作成ジョブ、所属の純粋な検査、文字起こしsnapshotを含む。
実行ログは `target/asr-chunk9/focused.log`。全件・製品起動・commitは行っていない。

焦点filter外のAboutは
`cargo nextest run -p mimageviewer --lib -E 'test(=ui_dialogs::about::tests::ai_worker_credits_snapshot_light_and_dark)'`
を同じ共有runnerから実行し、**1件成功、2.606秒**。PNGの更新はない。
`cargo nextest show-config test-groups --verbose -p mimageviewer --lib --groups gpu --no-pager` と
`cargo nextest list -p mimageviewer --lib --message-format json` を共有runnerから実行し、
同じ現在のテスト一覧へ現行設定・変更前設定を適用して照合した（default features）。
gpu所属は **294 → 304件**、追加は上記10件だけ、削除0。TOMLの構造比較でも所属filter以外の
設定値が不変で、max-threads=2を確認した。検査結果は `target/asr-chunk9/group-verification.json`、
一覧とログも同所。fakeのASR推論テストはgroupへ追加していない。

## 新しく所属させるテスト

条件が成立するとdevice / GPU sessionへ届くものを含める。開始直後の取消raceも対象とする。

| テスト（本体lib、末行だけ別crate lib） | 理由 |
| --- | --- |
| `app::tests::remote_video_stream_keeps_remote_dialog_without_entering_fullscreen` | 実H.264 fixture、既定の`video_hw_decode=true`からRemote generation worker → clockless補助decoder → D3D11VA。encoderをOpenH264にしてもdecodeはHW候補 |
| `depth_export::tests::depth_export_cancel_removes_each_partial_and_preserves_destination` | MP4分岐はCPU frameでもAuto encoderを実openする |
| `depth_export::tests::depth_export_job_drop_cancels_and_cleans_each_format_without_ui_wait` | 同上。最初のframe / cancelより前にencoderを初期化 |
| `depth_export::tests::depth_export_mp4_roundtrip_has_even_size_two_cycles_and_cfr_timestamps` | 実MP4書き出しがAutoのHW encoder ladderへ進む |
| `depth_export::tests::depth_export_process_exit_reaps_partial_files_after_on_exit_and_app_drop` | MP4の実workerがencoderを初期化してからframe barrier / exitへ進む |
| `video::seek_strip_thumbs::tests::probe_app_thumbnail_worker_window_from_env`（ignored） | 外部実動画probeのHW設定は既定true。codec / 環境次第でD3D11VAへ進む |
| `musicvideo-editor` の `app::mp4_tests::p4b3_start_pauses_audio_cancels_preview_and_suppresses_new_workers` | Auto設定の実workerを開始して即cancelするため、順序次第でHW encoderへ進む |

直接decoder / tile / hover / stripの実fixtureは、個別に`hw_decode=false`を確認したものを除外。
audio-only Remoteは映像deviceを作らない。
`app::tests::still_window_mode_key_tests::both_media_builders_open_saved_nondefault_tracks` はHW設定trueだが、
固定`multi.mkv`はMPEG-4 Part2。FFmpeg 7.1の当該decoderにD3D11VA configは無く、対応照会後にreturnする。
audio builderはHW無効、RemoteHeadlessはnative GPU presenterを作らず、別device作成もないため除外する。
根拠: [同梱7.1系のdecoder対応表](https://github.com/FFmpeg/FFmpeg/blob/n7.1.5/libavcodec/mpeg4videodec.c#L3575)、
`vendor/ffmpeg/VERSION`、fixture生成scriptと両builderの呼び出し経路。
fixture / codec / FFmpeg実装が変わったら再確認する。driverと無関係だという判断ではない。

前回P1のoverflow snapshot 5件とmacro生成page-turn snapshot 8件も引き続き含む。
`helper(...).snapshot(...)` やmacroの生成関数まで追い、GPU用helperの返り値 / 関数ポインタ経由も調べた。
group外の残存呼び出し元は、今回のgroup内実行では検証できない。

## filter・継承・今後の追加規則

正本は [.config/nextest.toml](../.config/nextest.toml)。`gpu_tests` と実device小moduleはprefix、
混在lib moduleは完全名に固定したregex / equalityで選ぶ。
4 integrationはbinary全体を選び、新規snapshot / GPU testを自動で含める。

```text
kind(test) & binary(/^(ui_snapshot|book_composition_editor_snapshots|test_ai_upscale|test_tile_blend|egui(_wgpu|_winit)?|epaint|eframe)$/)
```

同一regexのvendor lib名で各standalone graphのpredicate validationを成立させ、
`kind(test)`とのANDがvendor lib全体への割当を防ぐ。CPU integration 37件
（ui_snapshot 20、book-composition 16、AI heuristic 1、tile 0）も含める。
検査単体も全rootでgpuへ入れる。query / instance-onlyの全moduleを広く選ぶことはしない。

nextestは設定項目ごとに最初の一致overrideを使うため、先行slow overrideのpriority / timeoutと
gpu groupは併用できる。`New-MivTestRun`のTOML全体コピー、`miv-tool inherits = default`、
test-fullのルート設定指定を維持する。台本修正は不要。
根拠: [per-test settings](https://nexte.st/docs/configuration/per-test-overrides/)、
[profile inheritance](https://nexte.st/docs/configuration/)。

- 新しい製品GPU device / session生成点には、実GPU分岐で生成前に`#[cfg(test)]`の検査を置く。
  test-only helperにも検査を置き、CPU fake・情報照会・instance-onlyは区別する。
- テストは原則`gpu_tests`か既存4 integrationへ置く。混在libの新規caller / macroもfilterへ含める。
- **cfg(test)はdependencyへ伝播しない。** 新しいintegration binaryや別crate testが通常libを通じて
  GPUを使う場合は、binary単位の所属、または自crateの検査付き生成境界を用意する。
  通常exeで動く外部workerも、親の実session要求 / 起動境界で検査する。
- 各graphの`nextest list --run-ignored all --message-format json`、`-E 'group(gpu)'`、
  `show-config test-groups --groups gpu --no-pager`を**binary ID + 完全名**の集合で照合する。
  group内だけを実行して、期待漏れと既知CPU追加の両方を示す。
- 全件で検査付き生成点へgroup外から到達すると、background workerでもプロセスが失敗する。
  **group外の残存経路の確認は、取り込み後にClaudeCodeが行う全件実行で初めて成立する。**
  今回の静的期待一覧との「漏れ0」と区別する。未知の未検査APIや新規integrationまでの静的な網羅保証は無く、
  device追加時には生成点の検査・cfgの有効なcrate・callerの所属をレビューする。

## 検証記録（2026-11-16、製品生成点の再レビューP1）

`a4ab64a5c` + 今回の差分、cargo-nextest 0.9.148。共有runner経由で7構成をbuildし、
all list / GPU list / show-config / 生成miv-toolのlist・show-configが全てexit 0。
`MSBUILDDISABLENODEREUSE=1` / `CARGO_BUILD_JOBS=1`を指定した。
sandboxから共有lock / TEMPへ書けず、runner自身のBelowNormal fallbackで動いた。

| graph / features | 全一覧 | gpu所属 | ignored | 期待deviceの漏れ / CPU追加 |
| --- | ---: | ---: | ---: | ---: |
| workspace、launcher除外、`test-script,pack-build-tools` | 20,151 | 597 | 9 | 0 / 46 |
| うち本体lib | 18,119 | 299（1.65%） | 9 | 0 / 1 |
| egui-wgpu、`winit` | 51 | 15 | 0 | 0 / 1 |
| eframe、no-default-features、`default_fonts,wgpu` | 57 | 3 | 1 | 0 / 1 |
| eframe、no-default-features、`default_fonts,glow` | 58 | 1 | 0 | 0 / 1 |
| egui / epaint / egui-winit | 199 / 104 / 44 | 各0 | 各0 | 各0 / 0 |

コードから調べた期待unionは **567項目**（前回560＋新規7）、うち本体lib 298項目。
全項目を実一覧へ解決し、未解決0、期待一覧に対する漏れ0。
所属unionは615項目で、期待との差48はintegration CPU 37＋検査単体11。
単体のrootは前回7＋EPUB worker lib / bin・musicvideo-editor・VST testerの4。
eframe単体が2 feature graphに現れるので表の所属の単純和は616となる。
追加の未知項目0、明示除外147項目の所属0。
filter独立照合、default / miv-tool、list / show-configは全graphで項目が一致した。
portable graphの追加buildはしていない。

共有runnerから生成miv-tool profileで
`cargo nextest run … -E 'group(gpu)' --run-ignored default --retries 0 --no-fail-fast` を実行。
`--test-threads`は指定せず、groupの2枠で実行した。

| graph | 実行 / 成功 / 失敗 | group内ignored（未実行） | テストwall秒 |
| --- | ---: | ---: | ---: |
| workspace（補強後の再実行） | 588 / 588 / 0 | 9 | 430.750 |
| egui-wgpu（成功証拠再利用） | 15 / 15 / 0 | 0 | 7.381 |
| eframe wgpu（成功証拠再利用） | 2 / 2 / 0 | 1 | 0.776 |
| eframe glow（成功証拠再利用） | 1 / 1 / 0 | 0 | 0.014 |
| 合計 | **606 / 606 / 0** | 10 | 438.921 |

初回は605件成功後、EPUBの親 / binと音楽ビデオエディタの起動境界を追加補強したため、
workspaceを再build・再実行した。vendorのsource / dependency / 共通guard / 設定は変わらず、
現リストとの一致も確認できたため18件の成功を再利用した。
JUnit実行項目は各graphの「所属かつ非ignored」集合と一致し、group外の実行0。
検査での失敗0、検査単体も11 root / 12 graph実行ですべて成功。
実行引数のgroup filter・ignored設定・test-threads未指定と、保存設定のmax-threads=2 / profile継承も確認した。
ミリ秒丸めの誤差分を除いたJUnit区間の同時実行数も各graph最大2以下。
条件付きモデル・adapter不在や取消による既存早期returnは維持しており、全項目が必ずdeviceを作ったという意味ではない。
group外の残存呼び出し元は未実行で、取り込み後の全件で初めて確認する。

CPU integration 37件の時間総和は16.396秒、検査単体12実行は0.541秒。
今回のgroup内だけの固定時間 / 開始順モデルで、CPU37件をglobal 24枠へ戻した計算との差は
2枠で約8.06秒、1枠で約16.40秒。workspace全件の実測増分ではなく、下記lib予測とも分けて扱う。

`run-summary.json` / `run-verification.json` / `verify-run.py` と
`target/gpu-product-run-{workspace,egui-wgpu,eframe-wgpu,eframe-glow}/runs/` のJUnit・ログ・command.jsonが証跡。
初回結果は `*-pass1.json`、最終workspace実行の入力は `run-workspace-final.ps1` に保存した。
全件・cargo test・製品起動・commitは行っていない。fmt / diff検査も成功。

証跡は `target/gpu-test-group-product/`。
`expected-{ui,wgpu,native-ai}.tsv` は前回期待、`expected-video-additions.tsv` は新規7項目、
`expected-nonvideo-additions.tsv` は追加0、`expected-resolved.tsv` はunion。
`product-guard-entrances.tsv` は新しい34入口（製品側32＋test-only補強2）、`nonvideo-product-guards.tsv` は条件判断、
`excluded-video.tsv` は固定codecの除外。既存除外は `target/gpu-test-group-device/excluded-*.tsv`。
各graphのJSON / show-config / miv-tool所属、`verification.json` / `extra.tsv`、
再確認用script、command.jsonも同所に保存する。

## 本体lib全件の時間見積もり

前回と同じ成功JUnit 4件（`C:\home\mimageviewer\target\test-lib-full\runs\`）を使う。
testcase時間を固定しglobal 24枠 + GPU 2枠 / 1枠で離散イベント計算する。
priority 100を先行、同順位は過去の開始順とし、GPU満杯でも他テストでglobal枠を埋める。
見込み = 実測wall +（制限あり計算 − 24枠計算）。24枠計算と実測の差は各約0.2秒。
失敗のある11/13の0034・0334、11/12の1419・1525は成功記録に含めない。

| 成功run ID | 実測全件（分） | timed device件数 / 秒の総和 | 2枠 全件 / 増分（分） | 1枠 全件 / 増分（分） |
| --- | ---: | ---: | ---: | ---: |
| `20261112T113608616Z-6126c782` | 13.52 | 289 / 1,204.018 | 14.41 / +0.89 | 21.32 / +7.80 |
| `20261112T132150646Z-c5d748c1` | 15.94 | 289 / 1,441.764 | 17.56 / +1.62 | 25.52 / +9.58 |
| `20261113T030807051Z-9ef845ed` | 16.12 | 289 / 1,579.197 | 18.35 / +2.23 | 27.68 / +11.57 |
| `20261113T040852201Z-66e317ad` | 16.03 | 289 / 1,485.509 | 17.95 / +1.92 | 26.12 / +10.09 |

本体libの非ignored実device 289件は4記録にすべて時間があり、未計測0。
検査単体1件は過去記録に無い。integration / 別crateの追加時間をこのlib予測へ加えない。
名前順の感度確認もほぼ同じ。長い順では2枠の増分はほぼ0秒、1枠は+7.31〜11.31分。
2枠を基本とする。前回から増えた5件はこの固定順モデルの2枠のcritical pathを延ばさないが、
1枠の見込みは指定最新記録で約0.27分延びる。
build・integration・vendor・競合減少によるtestcase時間の変化を含まない予測であり、
全件の実所要・停止原因・2枠での停止予防効果は未確認。上限を増やす負荷実験はしない。

## §1.767 ブランチの推移的caller登録（2026-11-16）

`feat-767-stage2` のmaster取り込み `5975008e4`／`82b883c40` 後、設計担当の本体lib全件
`20261116T112427019Z-6a76bc41` で21件が所属検査により失敗した。
テスト本体・製品・検査入口を変えず、既存gpu inventoryへ完全名に固定したregex／equalityを追加する。
混在module全体や `realmachine50_` 全体は選ばない。`gpu.max-threads = 2`、global並列数、
retry／timeout／priority、既存overrideの継承とignoreは維持する。

| callerのまとまり | 追加件数 | device生成へ至るhelper／理由 |
| --- | ---: | --- |
| `book_composition_permission::tests::realmachine50_composition_*` の対象3件 | 3 | `realmachine50_composition_open_recheck` → `depth_view_tests::install_pages`。継続／hidden判定前に生成 |
| `depth_view_tests` の独立close／park・main handoff・readonly entry・snapshot activation | 6 | `install_pages`／`renderer`。状態検査だけに見える場合もWindowsでdeviceを作る |
| `multiwindow_depth_accepted_(folder\|pdf\|zip)_transition_*` | 3 | `depth_accepted_transition_case` → `install_pages` |
| `multiwindow_depth_independent_*` の対象3件 | 3 | `independent_depth_dialog_scenario` → `RootDepthViewer::new` → `install_pages` |
| `multiwindow_depth_paused_last_active_resume_close_does_not_register_passive_again` | 1 | `install_pages` の直接呼出し |
| `startup_ops::tests::realmachine50_*` のbookmark対象3件 | 3 | `install_pages`直接／`realmachine50_live_depth_owner_fixture`経由。取消・staleでも先にfixtureを作る |
| smart-folderのforeign ZIP／startup restoreのwarm PDF | 2 | 両方の旧owner fixtureで `install_pages` |

既存inventoryと同じ形式の9行で21件を追加した。名前実行のfilterは設定のregexから作らず、
要求21件を完全名の `test(=...)` ORとして別に固定する。

追加漏れの監査では、`depth_view_tests::{renderer,install_pages}`、`isolate_depth_renderer`、
`RootDepthViewer::{new,linked}`、`DepthWaitViewerFixture::new` からhelper呼出し元を追った。
12のソース範囲の通常test 118件と、`depth_dialog_owner_scenario!` の展開名12件を照合した。
既存分も含む計130件で、修正前の未登録は要求21件だけ、修正後は0件。
`install_ready_depth_for_export`／`record_ready_display` の他module callerも確認した。
全件で失敗しなかったcallerも含め、`depth_setup_entry_tests`、`depth_export::main_window_tests`、
`depth_main_window_tests` の追加／既存testは既存module prefixで所属し、dialog macro12件も登録済みだった。
master側merge parentとの差分のdevice生成・実renderer／snapshot呼出しも調べ、
このブランチから追加された別のGPU到達テストの漏れは見つからなかった。
監査集合とcall-siteは `target/gpu-767-caller-audit.json`／`target/gpu-767-call-sites.txt`。

対照として、Parked clickの3D状態fixtureは `DepthViewState` のみで実rendererを持たず、
`SourceDragHarness` はNoDisplayのegui-winit State／Context／CPU headless paintだけを使う。
native入力のfake allocation／disconnected playerや、PDF epoch等の `snapshot()` も実device生成ではない。
これらを名前の類似だけでGPU所属にしない。未知のAPIを含む網羅保証や、group外の全実行を行った
という意味ではない。取り込み後の全件gateは設計担当が実施する。

### 検証結果

対象は `82b883c40` ＋今回の設定差分。すべて共有 `run-heavy.ps1` 経由、
Owner `codex/767-gpu-group`、WorkDir `G:/home/mimageviewer-767s2`、直列実行。
共有全件ロックが続いたため待機し、取得済みの変更前／全一覧は再利用して、残りを
`target/gpu-767-remaining.ps1` の1wrapperへまとめた。テストの再試行はしていない。
lock／G: TEMPに書けない警告後はrunner既定のBelowNormal／6 build jobs／既定TEMPで継続した。

| コマンド | 件数／確認 | exit |
| --- | --- | ---: |
| `cargo nextest list -p mimageviewer --lib --config-file target/gpu-767-before.toml --run-ignored all -E 'group(gpu)' --message-format json` | 変更前gpu 318件 | 0 |
| `cargo nextest list -p mimageviewer --lib --run-ignored all --message-format json` | 一覧18,208件（実行しない） | 0 |
| `cargo nextest list -p mimageviewer --lib --run-ignored all -E 'group(gpu)' --message-format json` | 変更後gpu 339件 | 0 |
| `cargo nextest show-config test-groups -p mimageviewer --lib --groups gpu --no-pager` | 指定21件の所属・max threads = 2 | 0 |
| `cargo nextest run -p mimageviewer --lib -E $requestedFilter --no-fail-fast --test-threads 4` | 指定21件全成功、19.126秒 | 0 |
| `cargo fmt` | 最後のCargo操作、引数なしで成功 | 0 |

`$requestedFilter` は `Get-Content -Raw -Encoding UTF8 target/gpu-767-exact-filter.txt` で読む、
固定21完全名の `test(=...)` OR。名前リストは `target/gpu-767-requested.txt`。
設定の追加regexをそのまま実行filterに流用していない。

binary ID＋完全名で前後集合と実一覧を照合し、追加は指定21件だけ、削除0、余分なCPU追加0。
21件のignoredは0、監査した130 callerも全件gpu所属。gpu件数318／339はignoredを含む一覧の件数。
`target/gpu-767-verification.json` に集計を保存した。
JUnitは実行21件と指定名の集合が一致し、failure／errorは0。
`report-skipped = all` による非実行18,187件のmetadataは、実行件数へ含めない。
区間の素の集計はミリ秒丸めにより最大1msの3重なりが出るが、その精度以内を除く最大は2。
実行時の設定表示も2枠であり、制限値の変更や負荷を増やす試験はしていない。

証跡は `target/gpu-767-{before,all,group}-list.json`、各同名log、
`gpu-767-show-group.log`、`gpu-767-requested-run.log`、`gpu-767-requested-junit.xml`、
`gpu-767-fmt.log`、`gpu-767-remaining-wrapper.log`（いずれもtarget配下）。
変更ファイルは本書と `.config/nextest.toml` の2件だけ。
テスト本体・製品・vendorは変更せず、全件テスト・製品起動・build-dev・commit／pushは実施していない。

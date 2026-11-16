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
製品の挙動と ignore 指定は変更しない。テスト専用の生成入口に所属検査だけを追加する。

## 含める経路

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
| FFmpeg hardware encoder | `finite_fixture_flushes_final_fragment_and_endlist`、`finite_source_reaches_end_after_filling_the_live_window_without_a_next_fetch`、ignored encoder probe。前2件は `hw_decode=false` でも encoder が Auto で、NVENC / QSV / AMF / MF 経路を試す |
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
| software-only FFmpeg tests | 通常の decoder / thumbnail / tile / strip fixture は `hw_decode=false`。music-video / segmenter は OpenH264 を明示。audio-only Auto は video encoder を作らない |
| WebView2 worker の既存テスト | 不正引数、URL / PDF / process-record helper 等で、有効な変換の `webview::Host::new` へ進まない。将来の実変換テストは対象にする |
| lab / editor main、開発 probe bin | main を呼ばない unit-test harness や `test=false` のツールは実 GUI / device 起動と区別する。現行 eframe glow tests は context 作成を注入した fake を使う |

repository の Rust に直接の `D3D12CreateDevice` / MF DXGI device manager 呼び出しはなく、
DX12 は wgpu、MF は FFmpeg `h264_mf` 経由。情報照会の除外は仮説の境界であり、
それらの API やテストが停止に無関係だという実験結果ではない。

## P1 の再調査と入口の検査

以前の「漏れ0」は当時の期待一覧との一致だけを示し、その期待一覧自体に漏れがあった。
今回は `Harness::{render,snapshot,try_snapshot}` から helper の呼び出し元を再追跡した。
`helper(...).snapshot(...)` のような任意の receiver、返り値、関数ポインタ、macro 生成も調べた。

- P1 の5件: `ui_fullscreen::tests::fullscreen_overflow_panel_snapshot_` に続く
  `root` / `navigator_submenu` / `root_tall_without_scrollbar` / `page_turn_on` / `slideshow`。
  `snapshot_fullscreen_overflow_panel` → `Harness::snapshot` → 共有 LazyRenderer へ進む。
- 同型の追加漏れ8件: `page_turn_animation::snapshot_tests::page_turn_projection_snapshot_` に続く
  `{single,spread}_{ltr,rtl}_{forward,back}`。`route_test!` → `snapshot_route` →
  `snapshot_scene` → raster renderer。snapshot_tests の24件すべてを module prefix で選ぶ。
- 高さ検査で Harness を構築して捨てるだけの overflow sibling は lazy renderer を起こさず、除外を維持する。
  これ以外の追加漏れは今回のコード調査では見つからなかった。

共通検査は [tests/support/gpu_test_group.rs](../tests/support/gpu_test_group.rs)。
`NEXTEST=1` のときだけ `NEXTEST_TEST_GROUP == gpu` を要求し、欠落・`@global`・別 group なら
`.config/nextest.toml` の gpu group への追加を促す panic を device 作成前に出す。
`#[track_caller]` で入口を示す。nextest 以外では検査せず、環境変数を変更しない。
根拠は [nextest 0.9.143 の環境変数仕様](https://github.com/nextest-rs/nextest/blob/cargo-nextest-0.9.143/site/src/docs/configuration/env-vars.md):
`NEXTEST` は常に `1`、`NEXTEST_TEST_GROUP` は group 名または `@global`（0.9.90 から）。
設定の最低版0.9.143を確認し、今回の実行版は0.9.148である。

検査を呼ぶ入口は37か所。7つの test crate root（本体 lib、4統合テスト、egui-wgpu、eframe）で
同じ検査ファイルを読み、lib / vendor lib は `cfg(test)` に限定する。

| テスト専用入口 | 検査位置 |
| --- | --- |
| `tests/support/ui_harness.rs` | LazyRenderer の初回 closure、raw-root の明示 WgpuTestRenderer、実 RenderState テスト。builder / setup だけには置かない |
| `src/egui037_stage4_gpu_tests.rs` | `OffscreenGpu::new`（compare_wgpu / panorama_wgpu / depth の gpu_tests も経由） |
| `src/app/depth_view_tests.rs` | `renderer` の OnceLock より前。cache hit でも毎回所属を検査 |
| final prepare / canonical / page-turn / compare-crop | `native_tests::render_state`、`draw_key_tests::render_state`、実 native handoff / ROI の入口、compare-crop の `shader.then` 内だけ |
| `src/gpu_lanczos{.rs,/prewarm_tests.rs}` | 直接 device を作る2テストと `headless_render_state` |
| native video | resample の `warp_device` と hardware timestamp、native-window pump / thread-spike の ignored device テスト |
| hardware encoder | clockless-transcode の Auto encoder EOF 2件、ignored encoder probe |
| GPU AI | ignored DA3 / ai_crash_655、VLM real-model、AI upscale / tile-blend の6実 session 入口。モデル等の前提で既存早期 return する場合はその後 |
| vendored egui-wgpu | renderer の `headless_device`、capture、winit の `headless_painter` と実 device 2テスト |
| vendored eframe | 実 retirement texture device テスト、ignored 実窓テストとその child helper |

製品の `RenderState::create` / device constructor / AiRuntime 等には検査を追加しない。
独立した test-only helper が無い箇所は実生成を呼ぶテストの入口で検査する。
検査関数の純粋な単体テストは、nextest なしを許可、gpu を許可、group 欠落・global・別名を拒否し、
panic の設定案内を検証する。各 root に1件、計7項目（eframe の2 feature graph では同じ1項目を再実行）。

## filter・継承・新規テスト

正本は [.config/nextest.toml](../.config/nextest.toml)。実 device を使う小モジュールは prefix、
混在 lib module は完全名に固定した regex / equality で選ぶ。新規 `gpu_tests` は自動で拾う。
次の4 integration は binary 全体を選び、新しいスナップショット / GPU テストを自動で含める。

```text
kind(test) & binary(/^(ui_snapshot|book_composition_editor_snapshots|test_ai_upscale|test_tile_blend|egui(_wgpu|_winit)?|epaint|eframe)$/)
```

nextest は別 graph で predicate に対応する binary 名が一つも無いと validation error にする。
同一 regex の vendor lib 名は各 standalone graph での validation を成立させ、`kind(test)` との AND が
vendor lib 全体への割当を防ぐ。instance-only 17件などの除外を維持する。
代わりに4 integration 内のCPUテスト37件（ui_snapshot 20、book-composition 16、AI heuristic 1、tile 0）も含める。
さらに検査関数単体テストの7項目も明示的に gpu に入れる。本体 lib のCPU追加はその1件だけ。

nextest は設定項目ごとに最初の一致 override を採用し、先行する slow override の priority / timeout
と GPU group は併用できる。`New-MivTestRun` の TOML 全体コピーと `miv-tool inherits = default`、
`test-full.ps1` のルート設定指定も維持する。台本修正は不要。
根拠: [per-test settings](https://nexte.st/docs/configuration/per-test-overrides/)、
[profile inheritance](https://nexte.st/docs/configuration/)。

- 新しい実 device / GPU session 用 test helper の入口では、driver に触る前に
  `crate::test_gpu_group::assert_gpu_test_group()` を呼ぶ。製品の生成点へは入れない。
- テストは原則 private な `gpu_tests` サブモジュールか上記 integration へ置く。
  混在 lib の新規呼び出し元と macro 生成も filter に含める。CPU / instance-only は分ける。
- 各 graph / feature の `nextest list --run-ignored all --message-format json` と `-E 'group(gpu)'`、
  `show-config test-groups --groups gpu --no-pager` を確認する。**binary ID + 完全テスト名** の集合差で
  device / session の期待一覧と照合し、含めすぎは別に内訳と時間を示す。
- group 外から既存の検査付き helper を呼ぶ新しいテストは、全件実行で生成前に失敗する。
  **group 内だけの実行では group 外に残る呼び出し元を検出できない。** その確認は取り込み後に
  ClaudeCode が行う全件実行で初めて成立する。静的経路調査や今回の「期待一覧との漏れ0」と区別する。
  新しい検査無し helper を防ぐ静的な網羅保証はなく、device helper 追加時のレビューで入口検査を確認する。

## 検証記録（2026-11-16、P1 / P2）

`e5c3d6f38` + 今回の差分、cargo-nextest 0.9.148。共有 runner 経由で7 graph の build / list /
GPU list / show-config / 生成 miv-tool の所属確認が全て exit 0。
`MSBUILDDISABLENODEREUSE=1` / `CARGO_BUILD_JOBS=1` を指定。sandbox から共有 lock / TEMP に書けず、
runner 自身の BelowNormal フォールバックで動いた。全件実行・製品起動・commit はしていない。

| graph / features | 全一覧 | gpu 所属 | うち ignored | 期待 device に対する漏れ / CPU 追加 |
| --- | ---: | ---: | ---: | ---: |
| workspace、launcher 除外、`test-script,pack-build-tools` | 20,147 | 586 | 8 | 0 / 42 |
| うち本体 lib | 18,119 | 293（1.62%） | 8 | 0 / 1 |
| egui-wgpu、`winit` | 51 | 15 | 0 | 0 / 1 |
| eframe、no-default-features、`default_fonts,wgpu` | 57 | 3 | 1 | 0 / 1 |
| eframe、no-default-features、`default_fonts,glow` | 58 | 1 | 0 | 0 / 1 |
| egui / epaint / egui-winit | 199 / 104 / 44 | 各0 | 各0 | 各0 / 0 |

期待一覧は UI 420行、wgpu helper 124行、native / AI 18行（重複2件）、union **560項目**。
全項目を実一覧へ解決し、未解決名0、期待に対する漏れ0。
全 graph の所属 union は604項目、期待に対する追加44項目 = integration CPU 37 + 検査単体7。
eframe の単体テストが両 graph に現れるため、表の所属の単純和は605となる。
追加の未知項目0、明示除外146項目の所属0。
生成 `miv-tool` と default、filter の照合集合、`group(gpu)`、show-config は全 graph で項目が一致した。
portable graph の追加 build はしていない。

証跡は `target/gpu-test-group-guard/`。
`expected-{ui,wgpu,native-ai}.tsv` / `expected-resolved.tsv` は経路調査の期待一覧、
`missed-ui.tsv` は13追加項目、`raster-{caller-audit,helper-inventory}.tsv` は再調査、
`guard-entrances.tsv` は37入口。各 `*-all.txt` / `*-gpu.txt` は nextest JSON、
`*-groups.txt` / `*-miv-groups.txt` は show-config、`*-group-tests.tsv` は所属一覧、
`verification.json` / `extra.tsv` は集合差。除外一覧は前回の `target/gpu-test-group-device/excluded-*.tsv`。
`list-graphs.ps1` / `verify.py` と各 `*-logs/command.json` に再確認のコマンドも保存した。

共有 runner から `run-group.ps1` を使い、生成 `miv-tool` profile で
`cargo nextest run … -E 'group(gpu)' --run-ignored default --retries 0 --no-fail-fast` を実行した。
`--test-threads` は指定せず既定値を維持し、gpu の2枠で実行した。

| graph | 実行 / 成功 / 失敗 | group 内の ignored（未実行） | テスト wall 秒 |
| --- | ---: | ---: | ---: |
| workspace | 578 / 578 / 0 | 8 | 508.742 |
| egui-wgpu | 15 / 15 / 0 | 0 | 13.660 |
| eframe wgpu | 2 / 2 / 0 | 1 | 1.412 |
| eframe glow | 1 / 1 / 0 | 0 | 0.024 |
| 合計 | **596 / 596 / 0** | 9 | 523.838 |

JUnit の実行項目集合が各 graph の所属一覧の非 ignored 集合と一致し、group 外の実行0を確認した。
検査での失敗0。検査単体テストも7 root / 8 graph 実行ですべて成功した。
ミリ秒単位の JUnit timestamp / time から丸め誤差分を除いた同時実行数も各 graph 最大2以下。
条件付きのモデル・adapter 不在による既存の早期 return は変更しておらず、全項目が必ず device を作ったという意味ではない。
group 外の helper 呼び出し漏れは、この実行では検証していない。

実行証跡は `run-summary.json` / `run-verification.json` / `verify-run.py` と
`target/gpu-guard-run-{workspace,egui-wgpu,eframe-wgpu,eframe-glow}/runs/` の JUnit・ログ・command.json。
binary 単位化で含めたCPU37件は合計19.126秒、検査単体8実行は合計0.536秒だった。
同じ実行項目・固定時間・開始順で、CPU37件のみを group 外（global 24枠）へ戻した計算との比較では、
2枠で約9.75秒、1枠で約19.13秒の増分になる。これは今回の **group 内だけ** の所要モデルであり、
workspace 全件の実測増分ではない。

## 本体 lib 全件の時間見積もり

前回と同じ成功 JUnit 4件（`C:\home\mimageviewer\target\test-lib-full\runs\`）を使う。
各 testcase の時間を固定し、global 24枠 + GPU 2枠 / 1枠の離散イベント計算を行う。
priority 100 を先行、同順位は過去の開始時刻順とし、GPU 満杯でも他のテストで global 枠を埋める。
見込み = 実測 wall +（制限あり計算 − 24枠計算）。24枠計算と実測の差は各約0.2秒。
各1 failure がある11/13の0034・0334、11/12の1419・1525の記録は成功扱いしない。

| 成功 run ID | 実測全件（分） | timed device 件数 / 秒の総和 | 2枠 全件 / 増分（分） | 1枠 全件 / 増分（分） |
| --- | ---: | ---: | ---: | ---: |
| `20261112T113608616Z-6126c782` | 13.52 | 284 / 1,188.617 | 14.41 / +0.89 | 21.07 / +7.55 |
| `20261112T132150646Z-c5d748c1` | 15.94 | 284 / 1,424.593 | 17.56 / +1.62 | 25.23 / +9.29 |
| `20261113T030807051Z-9ef845ed` | 16.12 | 284 / 1,562.436 | 18.35 / +2.23 | 27.40 / +11.29 |
| `20261113T040852201Z-66e317ad` | 16.03 | 284 / 1,469.180 | 17.95 / +1.92 | 25.85 / +9.81 |

非 ignored の実 device lib 284件は、指定された最終 JUnit に全て時間があり、未計測項目0。
新しい検査単体テスト1件は過去記録に無いため別計測。binary 単位化のCPU37件は integration であり、
本体 lib の時間計算には含めない。指定 JUnit から workspace / integration 全件の予測はできない。
名前順の感度確認はほぼ同じ。長い順では2枠の増分はほぼ0秒、1枠は+7.05〜11.03分。
2枠を基本とし、1枠に対する時間負担を避けながら同時 device 作成を制限する。
これは build・integration・vendor や、競合減少による各テスト時間の変化を含まない予測である。
実際の全件所要時間・停止原因・2枠で停止を防げるかは未確認。上限を増やす負荷実験は行っていない。

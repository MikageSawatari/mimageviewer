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
テスト本体・製品コード・ignore 指定は変更しない。

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
| LazyRenderer → WgpuTestRenderer → RenderState → `request_device` | `Harness::render` / `snapshot` / `try_snapshot` と推移的な helper。lib の UI、`ui_snapshot`、`book_composition_editor_snapshots` を完全名で選ぶ。builder / run / input / query だけでは初期化されない。CPU adapter 優先でも device を作るため含む |
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

## filter・継承・新規テスト

正本は [.config/nextest.toml](../.config/nextest.toml)。現在の一覧で全てが実 device 作成に該当する
小モジュールは prefix、混在モジュールと integration は完全名に固定した regex / equality で選ぶ。
`kind(test)` や instance-only を含む vendor 全体は選ばない。共通設定の `package` / `binary` /
`binary_id` は別 graph で対象名が存在しないと parse error になるため使わない。

nextest は設定項目ごとに最初の一致 override を採用し、先行する slow override の priority / timeout
と GPU group は併用できる。`New-MivTestRun` の TOML 全体コピーと `miv-tool inherits = default`、
`test-full.ps1` のルート設定指定も維持する。台本修正は不要。
根拠: [per-test settings](https://nexte.st/docs/configuration/per-test-overrides/)、
[profile inheritance](https://nexte.st/docs/configuration/)。

- 新しい実 device / GPU session を作る helper とテストは、原則 private な `gpu_tests` サブモジュールに
  まとめる。`test(/(^|::)gpu_tests::/)` が自動で拾う。情報照会・instance-only・純粋計算はここへ置かない。
- 既存 helper を別モジュールから呼ぶ場合や macro 生成も、実テスト名を filter へ追加する。
- 各 graph / feature の `nextest list --run-ignored all --message-format json` と `-E 'group(gpu)'`、
  `show-config test-groups --groups gpu --no-pager` を確認する。**binary ID + 完全テスト名** の集合差で、
  漏れと含めすぎの両方を検査する。件数一致だけでは済ませない。

## 検証記録（2026-11-16、device 限定）

`36a0756d3` + この設定差分、cargo-nextest 0.9.148。前回 build の実行体を再利用し、
共有 runner 経由で7 graph の list / group list / show-config が全て exit 0。
`MSBUILDDISABLENODEREUSE=1` / `CARGO_BUILD_JOBS=1` を指定。sandbox から共有 lock / TEMP に書けず、
runner 自身の BelowNormal フォールバックで動いた。テスト・全件実行・製品起動・commit はしていない。

| graph / features | 全一覧 | gpu 所属 | うち ignored | 漏れ / 含めすぎ |
| --- | ---: | ---: | ---: | ---: |
| workspace、launcher 除外、`test-script,pack-build-tools` | 20,142 | 531 | 8 | 0 / 0 |
| うち本体 lib | 18,118 | 279（1.54%） | 8 | 0 / 0 |
| egui-wgpu、`winit` | 50 | 14 | 0 | 0 / 0 |
| eframe、no-default-features、`default_fonts,wgpu` | 56 | 2 | 1 | 0 / 0 |
| eframe、no-default-features、`default_fonts,glow` | 57 | 0 | 0 | 0 / 0 |
| egui / epaint / egui-winit | 199 / 104 / 44 | 各0 | 各0 | 各0 / 0 |

期待一覧は UI 407行、wgpu helper 124行、native / AI 18行（重複2件）、union **547項目**。
全項目を実一覧へ解決し、未解決名0、漏れ0、含めすぎ0。含めすぎの内訳も0であり、
情報照会・instance-only・CPU sibling を並列制限のために残してはいない。
明示除外146項目も所属しない。生成 `miv-tool` と default、filter の照合集合、`group(gpu)`、
show-config の集合は全 graph で一致した。portable 限定の前回候補1件も情報照会だけなので対象外。
portable graph の追加 build はしていない。egui-wgpu は `winit`、epaint は `default_fonts,color_fonts` を使った。

証跡は `target/gpu-test-group-device/` に保存。`expected-{ui,wgpu,native-ai}.tsv` はコード調査の期待一覧、
`excluded-{ui,wgpu}.tsv` は除外と理由、`expected-resolved.tsv` は完全名への解決結果。
各 `*-all.txt` / `*-gpu.txt` は nextest JSON、`*-groups.txt` / `*-miv-groups.txt` は show-config、
`*-group-tests.tsv` は所属一覧、`verification.json` / `extra.tsv` は集合差（extra は空）。
再確認用 `list-graphs.ps1` / `verify.py` と各 `*-logs/command.json` も同所にある。
これらは検証 artifact で、成功テスト台帳や追跡対象のテストコードを変更しない。

## 本体 lib 全件の時間見積もり

前回と同じ成功 JUnit 4件（`C:\home\mimageviewer\target\test-lib-full\runs\`）を使う。
各 testcase の時間を固定し、global 24枠 + GPU 2枠 / 1枠の離散イベント計算を行う。
priority 100 を先行、同順位は過去の開始時刻順とし、GPU 満杯でも他のテストで global 枠を埋める。
見込み = 実測 wall +（制限あり計算 − 24枠計算）。24枠計算と実測の差は各約0.2秒。
各1 failure がある11/13の0034・0334、11/12の1419・1525の記録は前回同様に成功扱いしない。

| 成功 run ID | 実測全件（分） | timed group 件数 / 秒の総和 | 2枠 全件 / 増分（分） | 1枠 全件 / 増分（分） |
| --- | ---: | ---: | ---: | ---: |
| `20261112T113608616Z-6126c782` | 13.52 | 271 / 1,145.201 | 14.06 / +0.54 | 20.34 / +6.82 |
| `20261112T132150646Z-c5d748c1` | 15.94 | 271 / 1,373.052 | 17.13 / +1.19 | 24.37 / +8.43 |
| `20261113T030807051Z-9ef845ed` | 16.12 | 271 / 1,505.047 | 17.87 / +1.76 | 26.45 / +10.33 |
| `20261113T040852201Z-66e317ad` | 16.03 | 271 / 1,412.628 | 17.47 / +1.44 | 24.90 / +8.87 |

現行の非 ignored lib group 271件は、指定された最終 JUnit に全て時間があり、未計測項目0。
名前順の感度確認はほぼ同じ。長い順の計算では2枠の増分はほぼ0秒、1枠は+6.33〜10.07分。
2枠を基本とし、1枠に対する時間負担を避けながら同時 device 作成を制限する。
これは build・integration・vendor や、競合減少による各テスト時間の変化を含まない予測である。
実際の所要時間・停止原因・2枠で停止を防げるかは未確認。並列負荷実験は行っていない。

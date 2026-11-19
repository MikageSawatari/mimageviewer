# §1.809 同じ描画フレームと GPU 描画費用の計測道具

製品の描画経路は変更しない。入力はすべて `RawInput` で合成する ignored test。
実製品 exe、native event loop、物理キー／ポインタ状態、surface／present を使わない。
報告者の RTX 4070 の使用率や、実 FPS、実製品の操作性を再現・診断したという意味ではない。

## 場面と本物との差

`src/ui_dialogs/preferences/hover_809/mod.rs`。
既存の `app::tests::phase_c_support::setup_app` を使い、使い捨て data-dir の App を構築する。
App と worker の破棄より後まで override と TempDir を保持する既存 fixture をそのまま使う。
全フレームで App の `logic` / `ui` は呼ばず、製品の描画関数だけを呼ぶ近似場面を採った。
フル App ハーネスは存在するが、logic が物理 modifier／clipboard／gamepad なども処理するため、今回の入力制約と分離目的に合わない。

| JSON scene | 内容 |
| --- | --- |
| `preferences_tree_full` | 製品 `render_menubar` →既存テスト shell の環境設定→製品 `render_toolbar` →製品 `render_grid`。左ツリー上で移動。General ページ、既定設定。 |
| `keyboard_full` | 同じ背面に既存テスト shell の操作カスタマイズ Keyboard。場所フィルタなし、override なし。実際のG/Hキー矩形の中心間を円の中心にする。 |
| `menu_toolbar_full` | 製品 `render_menubar` → `render_toolbar` → `render_grid`。中心 `(180,45)` pt、円はバー下の領域も通る。 |
| `thumbnails_full` | 同じ主画面で一覧上を移動。12列、96個の64×48 CPU生成テクスチャ。中心は画面横中央・y=200 pt。 |

ダイアログの shell は `hover_perf.rs` のものを再利用し、中身は実際の `draw_tree` / `draw_page` /
`draw_operation_customize_page`。環境設定は720×520、操作カスタマイズは900×640 pt。
環境設定の default_pos は既存 shell の `(280,150)`、操作カスタマイズは画面中央。
dialog ケースは App 側の表示フラグも設定し、背景 chrome/grid の既存 input gate を通す。
本棚は隠さず、計測前に使い捨ての空棚を既存 worker/poll 経路で読み終える。

実製品との差は、ファセット／検索／フォルダ pane／各 overlay／他の dialog、App の polling・reconcile・
native scheduler、独立 viewport を含まないこと。画像ファイルの decode／ロード／サムネ品質更新は行わず、
`Loaded` サムネイルと対応配列を直接 fixture に渡す。96 texture は保持するが、可視セル数は画面条件による。
egui 既定 theme/style と製品の既定日本語フォントを使い、利用者の theme・UI倍率等は再現しない。
既存App fixtureは初期設定済み・NoSidecar、製本rootを使い捨てdirへ隔離した条件。
App 構築に伴う SQLite/Tantivy/notify 等の実 worker は存在し、CPU 壁時計に外乱が入り得る。

## A: CPU のみ

テスト `ui_dialogs::preferences::hover_809::hover_809_identical_frames_cpu_measurement`。

- 2560×1440 / 3840×2160 px × pixels_per_point 1.0 / 1.5 の4条件。
- 各 scene ×60/144/240 Hz ×移動／静止。既定5秒（1条件の measured frame 数は300/720/1200）。
- `MIV_HOVER_809_SECONDS=1` で1秒の短い試走。`time` だけ進め、実時間で待たない。
- 40フレーム描画後、中心を探し、円の開始点で80フレーム静止して warmup。
  measured は**物理半径40px**（logical pointsは40 / pixels_per_point）、1秒1周。
  warmup開始点も同じ半径。静止では同じ位置を保持して move event を送らない。
  実際の対象上を通ったframe数・訪問対象数を残す。1項目内に留まる円も有効な観測として扱う。
- 期限に関係なく各 frame を強制描画する。静止ケースは native idle 時の実フレーム数ではない。
- `cpu_ms` は `run_ui` + tessellate の壁時計。shape clone、完全比較、JSON、fixture初期化を含まない。
- 毎 frame の `repaint_requested` は有限期限、`repaint_immediate` は期限0、`null` delay は `Duration::MAX`。
  scheduler の実際の起床や入力イベント原因の要求を直接計数したものではない。
- primitive は順序を含め、clip rect、texture ID、indices、頂点pos/uv/colorを前frameと完全比較。
  float は `to_bits` で比較し、hash の衝突に依存しない。callback は等しいと見なさない。
- `changed_frames` は **primitive が変わったframe数**。animation／tooltip等も含み、
  ホバー対象の変化と同義とはしない。`hover_target_changes` は egui の hovered ID集合の変化を別計数する。
- `draw_calls` は依頼の定義どおり primitive 数。実 GPU の draw 命令数とは区別する。
- `skip_candidate` は同じ primitive +空 textures_delta +callbackなし。固定size/ppp、ROOTだけ、screenshotなしがfixture前提。
  これを製品の省略判定として実装するわけではない。

## B: offscreen GPU

テスト `ui_dialogs::preferences::hover_809::gpu_tests::hover_809_offscreen_gpu_measurement`。
`gpu_tests` 名で既存 nextest gpu group に自動所属し、device作成前に `assert_gpu_test_group` を呼ぶ。

Aと同じ場面・4画面条件の、144Hz・移動1秒から実際に出た `same` / `changed` を各1frame採る。
各classの最初の実frameを仮採取し、0.25秒以後にも同じclassがあれば最初のそのframeへ置き換える。
初期遷移にしか存在しないclassも捨てず、`source_cpu_sample` に採取元を記録する。
全frameの平均的費用を実測したわけではない。
移動区間に `same` がなければ、同じscene/size/pppの静止144Hz・1秒から実在する `same` を採る。
出所のscene/size/ppp/Hz・`moving` を `source_cpu_sample` に残す。
`same` はこの両採取区間、`changed` は移動採取区間で見つからない場合だけ `omitted_representatives` に記録し、架空のframeで埋めない。
各capture時点までの full/partial texture set をframe順・同じID内のpatch順に再生し、GPU warmup 後の同じ仕事を測る。
ID間のHashMap順序は意味を持たない。
clearだけも各画面条件で測る。各rowはwarmup30回＋**計測240回**、中央値／p90／最大、μs。
さらにB専用の `menu_toolbar_only`（製品のメニュー→ツールバーだけ）、`thumbnails_only`（製品の一覧だけ）
を同条件で描く。これらは内訳であり、Cのfull場面への代用にはしない。
各rowは独立したclear/storeを含むpassで、成分の中央値を単純に足してfull費用へ分解するものではない。

Windowsは `OffscreenGpu` と同じ DX12 / FXC、`egui_wgpu::Renderer` の
`update_texture` → `update_buffers` → `render` → `queue.submit`。
adapter が `TIMESTAMP_QUERY` を持てばdeviceに要求し、render pass のbegin/end timestamp差を使う。
非対応なら明示して `cpu_submit_to_completion_us` を代替指標とする（GPU時間そのものではない）。
GPU対応時もCPU submit〜`device.poll`完了の分布を併記する。
target、query、16byte readback bufferは再利用する。画像全体のreadbackは行わない。

timestampの対象はclear＋eguiのrender＋store。update_buffers内のprepare、texture upload、
query resolve/copy/map、CPU構築、surface acquire、present、DWM、VSync、pacingは対象外。
各submit完了を待つ直列ベンチであり、製品のqueue/present並列性や持続FPSを再現しない。

製品設定との差: `src/lib.rs:1765` 以降のWgpuConfiguration、`:1821` 以降のDX12/FXC設定、
`:1853` 以降のNativeOptionsに対応する最小設定。MSAAなし（RendererOptions既定0、attachment sample_count1）、
ditheringあり／既定filterは共通。製品はsurface formatを選択するが、ここは既存OffscreenGpu同様
**Rgba8Unorm固定**。surface format／色空間／swapchainの違いは費用に影響し得る。
製品が要求し得るPASSTHROUGH_SHADERSは要求しない（画像補正shaderを使わないため）。
本fixtureのサムネイルは通常egui image meshで格子のpaint callbackなし。動画・画像補正等のcallback費用は含まない。
present modeは製品既定AutoVsync、ここにはsurface/present自体がない。
clear色は透明の0固定で、製品の `App::clear_color` には追従しない。
surface互換性を指定しない既定adapter選択であり、製品の候補選択・backend fallback・電源設定は再現しない。

## C: 見積もりと限界

`scripts/analyze_hover_809.py` がA/Bのscene・size・pppを照合し、Hz・移動／静止ごとの表とJSONを作る。
Bを実行するまでGPU費用を使った数値は未計測。Aの同一率／候補率はframe数の上限の手掛かりであり、
実GPU使用率の削減率とは呼ばない。

同じframe数を S、変化frame数を D、候補数を E、Bの代表中央値を ts/td として、
render費用のモデルは `(S*ts + D*td)`、削減は `E*ts`。
同じ形状でもtexture更新のあるframe等はEへ入れない。候補と非候補の同じframeに同じ代表費用を仮定する。
representativeが必要なのに存在しなければ unavailable を出す。CPU代替値しかない場合はproxyと明記する。
中央値×frame数による近似で、frameごとのGPU積算ではない。Bの144Hz移動時の固定代表費用を、
同じgeometryの静止／他Hzにも転用する（それぞれのtooltip状態等の費用を個別測定してはいない）。
静止fallbackを使った場合は、その固定費用の転用になる。

presentのGPU費用はoffscreenでは測れない。`--present-us P` で**仮定した**同じ非負費用Pを各frameへ加え、
`E*(ts+P)/(S*ts+D*td+(S+D)*P)` も出す。既定P=0。未知の一定Pに対する感度は、render削減率と
`E/(S+D)` の間。これはpresentがframeごとに同じ場合のモデルであり、実際のpresent/DWM費用の実測ではない。
upload等の省けないGPU仕事を分母から除いているため、GPU全体への割合は断定しない。

同じprimitiveでも描画／presentを省けない事情のソース棚卸し（今回いずれも変更しない）:

| 条件 | 該当箇所 |
| --- | --- |
| textures_delta set/free・配送完了 | `vendor/egui-wgpu/src/winit.rs:873` no-paint配送、`:1373` set、`:1438` submit、`:1455` free。描画をしなくても配送所有者の仕事は残る。 |
| 同じtexture IDの画素／binding更新 | `vendor/egui-wgpu/src/renderer.rs:587` binding lookup、`:923` 既存IDへ別texture登録。外部texture更新はmeshに現れない。 |
| screenshot要求／readback | `vendor/eframe/src/native/wgpu_integration.rs:1962` action抽出、`vendor/egui-wgpu/src/winit.rs:1092` surfaceなしでもcapture描画、`:1242` copy、`:1290` readback、`vendor/egui-wgpu/src/capture.rs:279` callback完了。 |
| paint callbackの動的state／副作用 | `vendor/egui-wgpu/src/renderer.rs:1199` prepare、`:1211` finish_prepare、`:642` paint。Arc identityが同じでも内容一定とは限らない（`vendor/epaint/src/shapes/paint_callback.rs:93`）。 |
| size・DPI・scissor・uniform | `vendor/egui-wgpu/src/winit.rs:1038` screen、`vendor/egui-wgpu/src/renderer.rs:1069` uniform、`:559` scissor、`:545` viewport。 |
| resize／surface再構成／喪失 | `vendor/egui-wgpu/src/winit.rs:852` resize、`:278` config変更予約、`:1059` 適用、`:1097` Suboptimal、`:1103` Reconfigure、`:1114` Lost/recreate。 |
| 複数viewportと共有rendererの所有 | `vendor/egui-wgpu/src/winit.rs:31` shared painter、`:43` viewport別surface/depth/MSAA、`vendor/eframe/src/native/wgpu_integration.rs:205` 消えたviewportのdelta保持。比較履歴をsurface寿命・viewportから独立に扱えない。 |
| clear色／target設定 | `vendor/eframe/src/native/wgpu_integration.rs:1154` root clear、`:1875` immediate clear、`vendor/egui-wgpu/src/winit.rs:1191` clear、`:1178` MSAA target。 |
| 明示paint／present周辺の仕事 | `vendor/eframe/src/native/frame_output.rs:31` paint条件、`vendor/eframe/src/native/wgpu_integration.rs:1176` post_rendering、`:1203` frame time、`vendor/egui-wgpu/src/winit.rs:1297` present、`:1311` startup観測完了。 |
| 最後に提示した絵との比較 | `vendor/eframe/src/native/frame_output.rs:26` UI を実行しても paint しない経路がある。比較の基準は直前の生成結果ではなく、その viewport／surface に最後に正常に present した内容にする必要がある (本計測の固定 fixture では生じない)。 |

## 実行コマンド

全てこのworktreeから実行。JSONは既存親ディレクトリ内の**新しい相対ファイル名**を指定する（上書きしない）。
以下の共通設定・コマンド全体を runner の `-Command` へ渡す。

```powershell
$measureCommand = @'
$env:MSBUILDDISABLENODEREUSE = '1'
$env:TEMP = 'G:\home\mimageviewer-hover805\target\hover-tmp'
$env:TMP = $env:TEMP
New-Item -ItemType Directory -Force -Path $env:TEMP | Out-Null
$env:MIV_HOVER_809_JSON = 'target/hover-809-a.json'
cargo test -j 1 -p mimageviewer --lib --profile dev-runtime hover_809_identical_frames_cpu_measurement -- --ignored --nocapture --test-threads=1
'@
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner codex-809 -WorkDir G:\home\mimageviewer-hover805 -Command $measureCommand
```

短い試走はcargoの前に `$env:MIV_HOVER_809_SECONDS = '1'` を加え、別JSON名にする。
`$env:MIV_HOVER_809_CAPTURE_SMOKE = '1'` を加えると、B用の全scene/geometryの代表frame採取も
CPUだけで各1秒試走し、全meshのtexture初期化が揃うこと・callbackがないことを検査する。
採取元をAのJSONの `capture_preflight` に保存する。deviceは作らない。
本計測では `MIV_HOVER_809_SECONDS` と `MIV_HOVER_809_CAPTURE_SMOKE` を除去する（既定5秒）。
コンパイルだけなら同じrunnerの子コマンドを
`cargo test -j 1 -p mimageviewer --lib --profile dev-runtime hover_809 --no-run` にする。

Bは上の共通TEMP設定を維持し、JSON設定とcargo行を次へ置換してrunnerを使う。
次は設計担当が実行するコマンドであり、今回エージェントは実行しない。

```powershell
$gpuCommand = @'
$env:MSBUILDDISABLENODEREUSE = '1'
$env:CMAKE_BUILD_PARALLEL_LEVEL = '1'
$env:TEMP = 'G:\home\mimageviewer-hover805\target\hover-tmp'
$env:TMP = $env:TEMP
New-Item -ItemType Directory -Force -Path $env:TEMP | Out-Null
$env:MIV_GPU_809_JSON = 'target/gpu-809-b.json'
cargo nextest run -j 1 -p mimageviewer --lib --cargo-profile dev-runtime -E 'test(=ui_dialogs::preferences::hover_809::gpu_tests::hover_809_offscreen_gpu_measurement)' --run-ignored only --no-capture
'@
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner codex-809 -WorkDir G:\home\mimageviewer-hover805 -Command $gpuCommand
```

Cは製品をコンパイル・起動しない:

```powershell
python scripts/analyze_hover_809.py target/hover-809-a.json target/gpu-809-b.json --output target/gpu-809-c.json
python scripts/test_analyze_hover_809.py
```

製品バイナリ、GPU本計測、lib全件、build-devは本作業の実行範囲外。コミットしない。

## 検証記録

2026-11-19、未コミットの本ツリーで最終CPU試走を実行。
成功ログは `target/hover-809-a-smoke3.log`、生データは `target/hover-809-a-smoke3.json`。
**各条件1秒の試走値であり、既定5秒の本計測ではない。** 他プロセスの負荷は固定していない。

- 指定runner経由、BelowNormal。runnerは外部lock/TEMPへの書込み不可を警告し、fallback経路を使用した。
  子でTEMP/TMPを本ツリーの `target/hover-tmp`、`MSBUILDDISABLENODEREUSE=1`、
  `CMAKE_BUILD_PARALLEL_LEVEL=1` に設定。native依存が既に構築済みの最終試走はCargo `-j 4`。
- `dev-runtime`（opt-level2、LTO off、製品と同じmimalloc）。最終コンパイル12分04秒、
  CPU testは **1 passed / 0 failed / 18,913 filtered out、300.91秒、runner exit0**。
  Bもこのlib testビルドでコンパイルされるが、実行したテストはAだけ。
- `MIV_HOVER_809_SECONDS=1`、`MIV_HOVER_809_CAPTURE_SMOKE=1`、
  `MIV_HOVER_809_JSON=target/hover-809-a-smoke3.json` で実行。
- Aの96条件・14,208frameは全て1 pass、callback0、設定値不変。
  保存座標から計算し直した物理半径の最大誤差は **0.000094px未満**。
- B用6scene×4geometryのCPU採取は全24組を通過し、40代表frameを保存。
  使用meshの全textureにwhole初期化deltaが存在し、callbackなしを検査した。
  今回の採取では静止fallback／初期区間だけの代表へのfallbackは不要だった。
- 移動中の即時repaintは **7,104 / 7,104**、静止は **0 / 7,104**。
  静止の同一率は全48条件で100%。期限に従って待つnative schedulerの観測ではない。
- textures_delta非空は12frame。すべてキーボードの移動ケースで、primitiveは同じだった。
  各ケース1frameを候補から除外し、全体の同一frame12,497に対し候補12,485。
  texture配送を含む製品の省略判定を実装・確定したという意味ではない。
- `python scripts/test_analyze_hover_809.py` は4件成功。引数なし `cargo fmt` と `git diff --check` 成功。
  独立ソースレビューは修正後ACCEPT。Bの実GPU本計測、lib全件、build-dev、製品exe起動、commitは未実施。

`smoke1` は補正前のppp1.5の円が60pxだった予備結果で、最終値に使わない。
`smoke2` はキー間の隙間を通るfixture配置を通過検査が検出して失敗した予備試走。
G/Hの実widget中心間と全key矩形へ修正し、物理40pxと通過条件を保ったのが最終 `smoke3`。
これらのfixture不備を製品のテスト失敗や報告者の不具合として扱わない。

次表は**移動ケース**。複数値は60 / 144 / 240Hz順、同値は1値にまとめた。
同一率はprimitiveの完全一致、候補率はさらにtexture更新なしの条件を満たすframe数の割合。
**候補率はGPU仕事の削減率ではない**。GPU費用を使ったCの表は、設計担当がBを実行してから上記Cコマンドで生成する。
キーボードG/Hでは同一率が低く、変化frame数もホバー先の変化数と一致していない。
「多数のframeが同じ」という説明を全場面へ一律には適用できない。この試走から実FPSや実機GPU使用率の因果は結論しない。

| pixels / ppp | scene | identical % | candidate % | changed frames | hover target changes | median vertices | median CPU ms |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 2560x1440 / 1 | preferences_tree_full | 86.67 / 93.75 / 96.25 | 86.67 / 93.75 / 96.25 | 8 / 9 / 9 | 10 / 11 / 11 | 33612 | 1.632 / 1.512 / 1.633 |
| 2560x1440 / 1 | keyboard_full | 16.67 / 9.72 / 8.33 | 15.00 / 9.03 / 7.92 | 50 / 130 / 220 | 6 | 41746 / 41862 / 41862 | 7.119 / 5.534 / 6.936 |
| 2560x1440 / 1 | menu_toolbar_full | 88.33 / 95.14 / 97.08 | 88.33 / 95.14 / 97.08 | 7 | 13 / 14 / 15 | 31004 | 0.980 / 1.352 / 1.262 |
| 2560x1440 / 1 | thumbnails_full | 100.00 | 100.00 | 0 | 2 | 30948 | 1.274 / 1.332 / 1.364 |
| 2560x1440 / 1.5 | preferences_tree_full | 93.33 / 97.22 / 98.33 | 93.33 / 97.22 / 98.33 | 4 | 4 | 29580 | 1.618 / 1.604 / 1.338 |
| 2560x1440 / 1.5 | keyboard_full | 20.00 / 13.19 / 11.25 | 18.33 / 12.50 / 10.83 | 48 / 125 / 213 | 6 | 37830 | 4.579 / 5.513 / 6.655 |
| 2560x1440 / 1.5 | menu_toolbar_full | 96.67 / 98.61 / 99.17 | 96.67 / 98.61 / 99.17 | 2 | 9 / 14 / 14 | 26900 | 1.168 / 1.105 / 1.099 |
| 2560x1440 / 1.5 | thumbnails_full | 100.00 | 100.00 | 0 | 2 | 26940 | 0.832 / 1.075 / 1.195 |
| 3840x2160 / 1 | preferences_tree_full | 86.67 / 93.75 / 96.25 | 86.67 / 93.75 / 96.25 | 8 / 9 / 9 | 10 / 11 / 11 | 33612 | 1.369 / 1.548 / 1.696 |
| 3840x2160 / 1 | keyboard_full | 16.67 / 9.72 / 8.33 | 15.00 / 9.03 / 7.92 | 50 / 130 / 220 | 6 | 41746 / 41862 / 41862 | 6.765 / 5.465 / 6.662 |
| 3840x2160 / 1 | menu_toolbar_full | 88.33 / 95.14 / 97.08 | 88.33 / 95.14 / 97.08 | 7 | 13 / 14 / 15 | 31004 | 1.399 / 1.443 / 1.142 |
| 3840x2160 / 1 | thumbnails_full | 100.00 | 100.00 | 0 | 2 | 30948 | 1.070 / 1.044 / 1.122 |
| 3840x2160 / 1.5 | preferences_tree_full | 93.33 / 97.22 / 98.33 | 93.33 / 97.22 / 98.33 | 4 | 4 | 33612 | 1.702 / 1.564 / 1.627 |
| 3840x2160 / 1.5 | keyboard_full | 20.00 / 13.19 / 11.25 | 18.33 / 12.50 / 10.83 | 48 / 125 / 213 | 6 | 41862 | 5.785 / 6.738 / 5.169 |
| 3840x2160 / 1.5 | menu_toolbar_full | 91.67 / 96.53 / 97.92 | 91.67 / 96.53 / 97.92 | 5 | 12 / 14 / 16 | 31004 | 0.995 / 1.338 / 1.250 |
| 3840x2160 / 1.5 | thumbnails_full | 100.00 | 100.00 | 0 | 2 | 30972 | 1.411 / 1.354 / 1.220 |

## 本計測の結果 (2026-11-19、設計担当が runner 経由で実行)

A は既定 5 秒 (`target/hover-809-a.json`)、B はメイン PC の RTX 4090 (DX12、driver 32.0.16.2101、timestamp query 対応、
`target/gpu-809-b.json`)、C は `python scripts/analyze_hover_809.py target/hover-809-a.json target/gpu-809-b.json --output target/gpu-809-c2.json`。
どれもヘッドレスの近似場面で、製品の実 FPS・present・DWM・報告者の RTX 4070 の使用率は測っていない。

### B: 1 フレームの render pass の GPU 時間 (中央値 µs、`same` の代表フレーム)

| 画面条件 | clear だけ | 環境設定の左一覧 | キーボード図 | メニュー・ツールバー | サムネイル一覧 | メニュー・ツールバーだけ | 一覧だけ |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 2560x1440 / 1.0 | 8.2 | 54.3 | 79.9 | 84.0 | 100.4 | 12.3 | 130.0 |
| 2560x1440 / 1.5 | 2.0 | 139.3 | 143.4 | 93.2 | 80.9 | 11.3 | 129.0 |
| 3840x2160 / 1.0 | 3.1 | 216.1 | 267.3 | 157.7 | 143.4 | 9.2 | 128.0 |
| 3840x2160 / 1.5 | 2.0 | 182.3 | 272.4 | 149.5 | 140.3 | 12.3 | 200.2 |

`changed` の代表フレームもほぼ同じ費用 (同じ場面で -27%〜+2%)。窓全体の描き直しは 1 フレーム 0.05〜0.27 ms で、
clear だけの費用は小さく、背面のメイン画面 (サムネイル一覧を含む約 340 primitive) が大半を占める。

### C: 移動中の render pass の GPU 時間 (ms / 秒) と、見た目が同じフレームを省いた場合の削減

各セルは「1 秒あたりの render pass の GPU 時間 → 省ける割合」。60 / 144 / 240 Hz の順。

| 画面条件 | 環境設定の左一覧 | キーボード図 | メニュー・ツールバー | サムネイル一覧 |
| --- | --- | --- | --- | --- |
| 2560x1440 / 1.0 | 3.3 / 7.8 / 13.0 → 87〜96% | 4.9 / 11.6 / 19.4 → 4〜11% | 5.1 / 12.1 / 20.2 → 88〜97% | 6.0 / 14.5 / 24.1 → 100% |
| 2560x1440 / 1.5 | 8.3 / 20.0 / 33.3 → 95〜99% | 7.3 / 17.3 / 28.8 → 5〜14% | 5.6 / 13.4 / 22.3 → 97〜99% | 4.9 / 11.7 / 19.4 → 100% |
| 3840x2160 / 1.0 | 12.6 / 30.7 / 51.4 → 89〜97% | 12.6 / 29.7 / 49.2 → 5〜14% | 9.4 / 22.6 / 37.8 → 89〜97% | 8.6 / 20.6 / 34.4 → 100% |
| 3840x2160 / 1.5 | 10.8 / 26.1 / 43.6 → 94〜99% | 12.5 / 29.4 / 48.7 → 6〜15% | 8.9 / 21.5 / 35.8 → 92〜98% | 8.4 / 20.2 / 33.7 → 100% |

静止中は、全条件で全フレームが前と同じ (このテストは静止中も毎フレーム強制的に描くので、製品の静止時のフレーム数ではない)。

### 読み取り (コードと上の計測から。実機の使用率との因果は未確認)

- マウスを動かすと、egui はフレームごとに即時の描き直しを求める (A の 5 秒本計測: 移動中 35,520 / 35,520 フレーム、静止中 0 / 35,520)。
  フレームは入力の頻度 (画面の更新上限まで) で回り、そのたびに窓全体を GPU で描き直す。
- 環境設定の左一覧・メニュー・ツールバー・サムネイル一覧の上では、そのフレームの 87〜100% が前のフレームと
  描画内容 (primitive) まで同じ。サムネイル一覧の上では、ホバーで見た目が変わらないので 100% 同じ。
- キーボード図の上では、ほとんどのフレームの描画内容が変わる (同じフレームは 4〜12%。各条件でホバー先の変化は 30 回なのに、変化フレームは 264〜1,156)。
  egui のツールチップがポインタに付いて動くためと見られる (コードからの推定、未確認)。省いても render pass の GPU 時間は 4〜15% しか減らない (モデル上の見積もり)。
- 4090 の render pass だけで、240 Hz の移動中は 1 秒あたり 13〜51 ms (GPU 時間の約 1〜5%)。
  present・DWM の合成・surface の format の違いは含まない。RTX 4070 で 2 割台という報告 (5ch の報告者) との差は、
  GPU の性能差と present などの未計測分で説明できる可能性があるが、確かめていない。
- 報告者の「キーボード図の上では 1〜4%」は、§1.808 の修正前はキーボード図の 1 フレームの CPU 処理が重く (同じ報告の
  計測で 1 フレーム十数 ms)、フレームが少なく回っていた、という仮説と矛盾しない。確かめてはいない。

### 削れる描画の候補と、入れる場合の条件

候補は 1 つ: **前のフレームと描画内容が完全に同じで、textures_delta が空、paint callback・screenshot 要求が無く、
窓の大きさ・DPI・surface の状態が変わっていないフレームは、render と present を省く**。比較の基準は「直前に作った絵」ではなく、**その viewport / surface に最後に正常に present した絵**にする (`vendor/eframe/src/native/frame_output.rs:26` のように、UI を回しても paint しない経路があるため。例: 窓が隠れている間に絵が A→B と変わり、復帰後の絵が B なら、画面には A が残っているので省けない)。CPU のフレーム処理・入力の応答・
描き直しの要求は変えない (間引きではなく、同じ絵の再提出をやめる)。環境設定の左一覧・メニュー・ツールバー・サムネイル一覧の場面では、render pass の GPU 時間を 87〜100% 減らせる見込み (B の代表フレームの中央値を使ったモデル上の見積もり。キーボード図では 4〜15%)。

ただし製品の描画経路 (vendor/eframe・vendor/egui-wgpu の paint / present、複数 viewport、起動時の present 観測) を変える。
上の「省けない事情」の表の 9 種類と、最後に present した絵を基準にする条件を、すべて判定に入れる必要があり、detached viewer の viewport 経路にも触れる。
入れる場合は、CLAUDE.md の detached リワークのルール (憲法の確認、ClaudeCode と Codex の双方が構造的修正と合意、
docs/detached-rework-plan.md への記録) と、実装前の独立レビューが要る。実機の GPU 使用率がどれだけ下がるかは、
present を含むので、実装した版で修正前後を比べないと分からない (サブ PC は RDP 越しで 32 fps 程度に制限されるため、
この比較には向かない可能性が高い)。

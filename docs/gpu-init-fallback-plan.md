# GPU 初期化の失敗に備える: 診断強化・FXC 固定・`--gpu-backend`・Vulkan での開き直し (§1.448)

2026-10-26 第 2 版 (ClaudeCode)。第 1 版の独立設計レビューは ACCEPT WITH CHANGES (P1 なし)。指摘 8 件を反映 (§10)。設計・作業指示・検収 = ClaudeCode、実装 = Codex、独立レビューは通常どおり。
v4.8.3 (v4.8.2 からのパッチ) の対象。ブランチ `gpuinit-1448` (v4.8.2 のタグから)。master へも取り込む。
コードの事実は v4.8.2 (`da9a64cd5`) と master を読んだもの。報告者の環境での実行時の観測は、投稿のスクショ 1 枚だけ。

## 1. 背景

- X の利用者投稿 (連絡は取れない): v4.7 までは起動し、v4.8.0 から起動しない。v4.8.2 ポータブル版で §1.428 の失敗ダイアログに
  「詳細: Wgpu.RequestDeviceError: WGPU error: Parent device is lost」と出た。古いグラフィックドライバを更新したくない意向。
- v4.8.0 で egui 0.33 → 0.37 に伴い wgpu 27 → 30 (gpu-allocator 0.27 → 0.28)。mIV の adapter 選択 (DX12 → Vulkan) と
  要求する limits は v4.7.1 と同じ。
- 調査 (2026-10-26、Codex 読み取り調査 + ClaudeCode 確認。backlog §1.448 に記録):
  - 「Parent device is lost」は `wgpu-core` の `DeviceError::Lost`。hal の想定外エラーも、`request_device` 内で wgpu-core が作る
    検証用 shader / compute pipeline の失敗も、すべてこの文言になる。原因は文言からは絞れない。
  - **製品 logger は wgpu の `log::error!` を捨てている** (`src/logger.rs` 約 18 の `enabled` が `tantivy::` と
    `egui_wgpu::surface_window_limit` しか通さない)。報告者のログにも、失敗した操作や HRESULT は残っていない。
  - DX12 の shader compiler の既定が wgpu 27 の `Fxc` から 30 の `Auto` に変わった。`Auto` は `dxcompiler.dll` を LoadLibrary で探し
    (アプリのフォルダ・System32・PATH など)、見つかれば DXC、無ければ FXC。mIV は DXC を同梱せず、開発機にも無いので、
    **開発機とテストは今も FXC**。PATH 等に `dxcompiler.dll` がある環境だけ DXC になる。
  - Vulkan は最低要件の引き上げは無い。任意拡張・allocator の置き換え等が候補。
  - core の `Surface` は Instance で有効な各 backend の surface を持つので、device を確定する前なら同じ window / surface のまま
    別 backend の adapter で開き直せる (`wgpu-core 30.0.1/src/instance.rs` 約 185・225)。

## 2. やること (4 つ)

1. **診断強化**: 失敗ダイアログとログに、GPU 名・API・種別・ドライバ版・失敗した段階・wgpu の直近のエラー行・試した API の一覧を出す。
2. **FXC 固定**: DX12 の shader compiler を `Dx12Compiler::Fxc` に固定する (v4.7 と同じ)。
3. **`--gpu-backend auto|dx12|vulkan`**: 起動オプションで使う API を選べるようにし、失敗ダイアログで案内する。
4. **Vulkan での開き直し**: 既定 (`auto`) で、DX12 の device 作成が失敗したら、同じ window / surface のまま Vulkan の adapter で 1 回だけ開き直す。

OpenGL は入れない (WGL の初期化を全利用者に足すことになり、遅延初期化の別設計が要る。救えない報告が来たら改めて検討する)。
保存して次回から使う API を覚える仕組みは作らない (§6)。

## 3. 起動オプション `--gpu-backend`

- 値: `auto` (既定。指定なしと同じ) / `dx12` / `vulkan`。大文字小文字は区別しない。
- 効果 (Instance を作る前に決める。`src/lib.rs` 約 1575 の `WgpuSetup::CreateNew` 設定箇所):

  | 値 | Instance の backends | device を試す順 |
  | --- | --- | --- |
  | `auto` | DX12 \| VULKAN (今と同じ) | DX12 の adapter → 失敗したら Vulkan の adapter (§5) |
  | `dx12` | DX12 だけ | DX12 の adapter だけ。失敗したら終了 |
  | `vulkan` | VULKAN だけ | Vulkan の adapter だけ。失敗したら終了 |

  明示指定のときは開き直さない (指定した API の検証・切り分けに使うため、結果を変えない)。`vulkan` 指定では root GUI の DX12 の Instance を作らない
  (動画 HUD は今までどおり別の DX12 Instance を使う)。
- 不正な値 (`--gpu-backend opengl` など、値が無い場合を含む): 既存の致命的エラーの MessageBox (`data_dir.rs::show_fatal_messagebox` と
  同じ helper) で「--gpu-backend には auto / dx12 / vulkan を指定してください」と出して終了する (非ゼロ)。黙って `auto` にしない。
- 引数の扱い: core の `cli_flag_takes_value` (`src/lib.rs` 約 2171) と、**launcher の値付き flag の判定にも**登録する。
  launcher は core を起動する前に既存インスタンスを検出し、独自にパスを解析して転送する (`crates/launcher/src/main.rs` 約 177・605・650・690)。
  登録しないと `mimageviewer.exe --gpu-backend vulkan C:\images` で `vulkan` が開くパスとして転送され、本来のパスが無視される。
  test-script 版の `--gpu-test-fail-device` も同じく値を消費させる。
  既に起動中の mIV へ転送される場合 (single-instance) は意味を持たないので、転送先では無視する。
- 解析は 1 か所 (`gpu_backend_policy` のような小さな module) にまとめ、`src/wgpu_backend_policy.rs` の `native_backends()` を
  「要求 → backends と試す順」を返す形に広げる。テスト側 (`tests/support/ui_harness.rs`) も同じ module を使っている。
- テスト用: UI テスト (`ui_harness`) は環境変数 `MIV_TEST_GPU_BACKEND=dx12|vulkan` で同じ要求を選べるようにする (製品は環境変数を読まない)。
  開発機 (Vulkan 対応 GPU) で `ui_snapshot` を Vulkan で通せるようにするため。**効くのは `ui_harness` を通る UI テストだけ**で、
  独自に Instance を作る GPU fixture (`src/egui037_stage4_gpu_tests.rs` 約 14、`src/compare_wgpu.rs` 約 715、`src/panorama_wgpu.rs` 約 1006、
  `src/gpu_lanczos/prewarm_tests.rs` 約 221 など) には効かない。Vulkan での shader 全体の確認は §8 の実アプリ確認で行い、混同しない。

## 4. FXC 固定

- root GUI の Instance: `create_new.instance_descriptor.backend_options.dx12.shader_compiler = wgpu::Dx12Compiler::Fxc`。
- 動画 HUD の別 Instance (`src/video/native_presenter/overlay_gpu.rs` 約 31、DX12 専用) も同じく FXC にする。
- テストの `ui_harness` も製品と同じ設定にする。独自に Instance を作る GPU fixture (§3 のテスト用の項に列挙) も明示的に FXC にし、
  shader の回帰が製品と同じ compiler で走るようにする。`src/bin/gpu_lanczos_spike.rs` (開発用) は対象外。
- 理由: 実行環境にたまたまある DLL で挙動を変えない (v4.7 と同じ・開発機とテストで確かめている構成に揃える)。
  知らない `dxcompiler.dll` を読み込まない。mIV は追加の GPU 機能を要求しておらず、SM 6 は要らない (Codex 確認)。
- 効果の見込み: 報告者の環境に `dxcompiler.dll` が無ければ効果は無い。原因の候補を 1 つ消すための変更で、直すとは主張しない。

## 5. Vulkan での開き直し

### 5.1 場所と形

- `vendor/egui-wgpu/src/lib.rs` の `RenderState::create` (約 215)。今は selector が adapter を 1 つ返し、`request_device` を 1 回呼ぶ。
- `WgpuSetupCreateNew` に mIV 追加の field `native_adapter_candidates` (候補を**順に並べた list** を返す closure) を足す。
  指定されていれば、`RenderState::create` は候補を順に `request_device` し、最初に成功したものを採用する。既存の
  `native_adapter_selector` (1 つを返す) は残し、candidates が無いときは今と同じ動きにする (vendor の他の利用者を変えない)。
- 候補を作るのは mIV 側 (§3 の表)。`auto` の候補は「最初の DX12 adapter」「最初の Vulkan adapter」の最大 2 つ
  (今の selector と同じく、DX12 は最初の 1 つだけ。ノート PC の 2 枚目の GPU を試す等の挙動は足さない)。
  各候補は `compatible_surface` に対して `adapter.is_surface_supported` を満たすものだけにする。
- 開き直すのは **device 作成 (`request_device`) の失敗だけ**。adapter 列挙・surface 作成・surface 設定・pipeline 作成・初回の描画など
  他の段階の失敗、実行中の device lost、panic では開き直さない。このうち `run_native` が **Err を返す失敗**は §1.428 の経路で終了し、
  §7 の診断と案内を出す。**panic・native exception (driver の AV など) は §1.428 のダイアログに入らない**既存の経路のままで、
  今回 catch や再初期化は足さない。
- 失敗した候補の device / queue は作られていないので、捨てるものは無い。AppCreator は 1 回だけ呼ばれる (開き直しは App を作る前)。
- 全候補が失敗したら、最後の候補のエラーを `WgpuError::RequestDeviceError` で返す。どの候補が何で失敗したかは、**各 `request_device` の結果が
  確定した時点で**、新しい startup event (例 `DeviceAttempt { attempt, info, result: Ok | Err(エラー文) }`) で mIV へ渡す。次の候補へ進む前に記録し、
  最後の Err から履歴を組み立て直さない。既存の stage の Begin / End は維持する。hal のログでは代わりにならない (limits 超過は Error ログを出さず
  `LimitsExceeded` を返す: `wgpu-core 30.0.1/src/instance.rs` 約 940)。
- `native_adapter_candidates` は既定 `None`。`None` では selector 1 回・request 1 回の今までの動き (回帰テストを足す)。`Clone` / `Debug`・
  native / web の扱いを selector と揃える。`compatible_surface == None` (headless) では surface の確認を省く。UI harness の CPU 優先 selector は維持する。

### 5.2 CLAUDE.md との関係

- これは「症状を隠す silent fallback」ではなく、**環境で使える API を起動時に 1 回選ぶ**処理とする。条件:
  起動時の初回 device 作成に限る / API ごとに 1 回 / 要求する limits・features は同じ / 試した結果をすべてログと起動記録に残す /
  Vulkan で起動したことを利用者が後から分かる (ログ・起動記録。§7)。
- §1.428 の設計書 (`docs/render-init-failure-report-plan.md` §6) の「DX12 が失敗したら Vulkan / FXC 固定などで自動的にやり直す: 採らない」は、
  この設計で更新する。理由: 実際に DX12 の device 作成で止まる報告があり、利用者は連絡が取れずドライバも更新しない。
  ただし**スクショから分かるのは root の `request_device` が失敗したことまでで、backend は分からない** (DX12 が候補に出ず、選ばれた Vulkan が
  失敗した可能性もある)。この変更は原因を直す証明ではなく、**有限の API 選択による起動互換性の向上**として入れる。
  経路は開発機で失敗を注入して確かめられる (§8)。
- まれな失敗の割り切り: Vulkan でも失敗したら終了する。OpenGL・2 枚目の adapter・再試行の繰り返しは足さない。

### 5.3 遅さ

- DX12 の device 作成が失敗するまでの時間だけ、起動が遅くなる。DX12 で成功する環境 (ほぼ全員) では何も変わらない
  (Vulkan の device は作らない。Instance は今も DX12 | VULKAN を作っている)。

## 6. 採らない案

- **成功した API を覚えて次回から使う**: ドライバ更新・GPU の差し替えで古い記録が誤りになり、失効の規則が要る。毎回 DX12 から試しても
  遅くなるのは失敗する環境だけ。覚えない。
- **DXC → FXC → Vulkan の多段**: compiler は Instance 作成時に決まるので、DXC → FXC には Instance から作り直しが要る。
  mIV は DXC を同梱せず SM 6 も要らないので、FXC 固定で足りる。
- **OpenGL**: §2。
- **wgpu の検証用 pipeline (`VALIDATION_INDIRECT_CALL`) を切る**: 安全性の設定を、根拠無しに変えない。
- **設定画面に API の選択を置く**: 起動できない人は設定画面を開けない。起動オプションで足りる。

## 7. 診断 (ダイアログ・ログ・起動記録)

### 7.1 集める値

- **adapter 情報**: `StartupDiagnosticEvent::AdapterSelected(AdapterInfo)` を候補ごとに受け取る (`src/lib.rs` 約 1530 の callback)。
  name / backend / device_type / vendor / device / driver / driver_info を保持する (今は driver を捨てている)。
- **失敗した段階**: callback の `End { success: false }` の時点で、その stage を保持する (終了時の lane snapshot では取れない:
  Codex 指摘)。候補ごとに記録する。
- **wgpu のエラー行**: 起動中だけ、logger の `enabled` に `wgpu_hal`・`wgpu_core`・`gpu_allocator` の **Error** を通す。
  target は crate 直下も含めて判定する (`target == "wgpu_hal" || target.starts_with("wgpu_hal::")`。allocator のエラーを `Lost` に変える重要な行は
  crate 直下の target から出る: `wgpu-hal 30.0.1/src/lib.rs` 約 391〜414)。global のログ上限 (Warn) は変えない。
  - 始める時点: root の `Begin(Instance)` より前 (adapter 列挙中の Error、例えば 30 の alignment 不正による adapter 除外を落とさない:
    `wgpu-core 30.0.1/src/instance.rs` 約 1325〜1345)。
  - 止める時点: 最初の `RootPresented`。失敗で戻ったときは cleanup の前に止めて snapshot を取る。
  - 全文は既存の非同期 writer でログへ。enqueue は診断状態のロックの外で行う。
  - Vulkan は VkResult を必ずしも log に出さないので、「追加情報なし」があり得る。
- **帰属 (1 つの診断 owner に集める)**: 「各試行の確定結果」と「採用後の root の状態」を分けて持つ。
  - エラー行は root の初期化を実行しているスレッドのものだけを、その時点の試行に結び付ける。別スレッド (動画 HUD の初期化など、
    `src/video/native_presenter/overlay_gpu.rs` 約 55) の Error はログには残すが、root の試行には結び付けない。
  - 各試行の 2 行は、その試行の `request_device` が終わった時点で確定し、次の候補へ持ち越さない。保持する行には文字数の上限を設ける。
  - 失敗した stage は、**致命的な失敗として `run_native` が Err を返したときの root の状態**から取る。`End(false)` は致命的とは限らない
    (SurfaceAcquire の SkipFrame・再設定でも出る: `vendor/egui-wgpu/src/winit.rs` 約 834〜886) ので、それだけで失敗段階と決めない。
    DX12 失敗 → Vulkan 成功のときに DX12 の失敗段階を最終の失敗として扱わない。
- **試した API の一覧**: `auto` の開き直しで、各候補の backend・adapter 名・結果 (成功 / 失敗段階とエラー文)。

### 7.2 ダイアログ (§1.428 の `src/run_native_failure.rs` に足す)

GPU 関係の失敗 (`graphics == true`) のときだけ、既存の本文の後に次を足す。長い行は短縮し、全文はログへ。

```text
GPU: <名前> / <DX12|Vulkan> / <種別>
ドライバ: <版。不明なら「不明」>
段階: <DeviceRequest など>
GPU 診断: <wgpu の直近のエラー 1〜2 行。無ければ「追加情報なし」>
試した API: DX12 (失敗) → Vulkan (失敗)          ← 開き直しを試したときだけ

別の描画方式で起動するには、--gpu-backend vulkan を付けて起動してください。   ← 案内の条件は下記
```

- 案内の条件:
  - `dx12` 指定で失敗 → `--gpu-backend vulkan` を案内。`vulkan` 指定で失敗 → `--gpu-backend dx12` を案内 (別の API を試していないため)。
  - `auto` で DX12 の device 作成以外の段階で失敗 (Vulkan を試していない) → `--gpu-backend vulkan` を案内。ただし、この起動で Vulkan の
    adapter が無い・surface 非対応と分かっていれば案内しない (分かっている利用不可を試させない)。
  - `auto` で DX12 の device 作成に失敗し、Vulkan の候補が無かった → 案内しない。両方試して両方失敗 → 案内しない。
  - adapter を選ぶ前の失敗では、存在しない「最後の adapter」を仮定しない (GPU の行は「不明」)。
  - 案内しないときは既存のドライバ更新の案内だけ。
- 文面は利用者向けの言葉にし、実装用語 (adapter・request_device 等) を本文に出さない。「段階」の値はログの識別子のまま出してよい
  (開発側への伝達用)。ドライバ版は DX12 では 4 つ組、取れなければ (`0.0.0.0` を含む)「不明」。

### 7.3 ログと起動記録

- ログ (`mimageviewer.log`): 候補ごとに 1 行 `gpu attempt=<n> backend=<..> adapter=<..> driver=<..> result=<ok|error> stage=<..> error=<..>`、
  採用時に `gpu selected backend=<..> request=<auto|dx12|vulkan>`。成功しても、先に失敗した候補の行は残す。
- 起動記録 (`miv_startup::record_detail`): 値は 256 bytes 上限なので、`gpu.request`・`gpu.adapter.<n>`・`gpu.attempt.<n>` に分ける。
  記録は既存どおり `terminal()` の前。

## 8. テスト

- 引数: `--gpu-backend` の各値・大文字小文字・不正値・値なし・パスとして開かれないこと。launcher の解析で、既存インスタンスあり / なし・
  後続のパスあり / なし・`--` 区切りの組み合わせ (値がパスとして転送されず、本来のパスが転送される)。single-instance 転送先で無視。
- 候補の並び (純粋関数): AdapterInfo の fake 列から、`auto` / `dx12` / `vulkan` ごとの候補と順序。DX12 が無い・Vulkan が無い・
  surface 非対応の adapter を除く。
- 開き直しの経路 (vendor egui-wgpu、GPU 無しで回せる形): 候補を順に試す関数を、`request_device` を注入できる形に分けて検査する。
  DX12 成功なら Vulkan を試さない / DX12 失敗 → Vulkan 成功 / 両方失敗で最後のエラー / 候補 0 / 各試行で Begin・End・AdapterSelected が出る /
  成功時も先の失敗が記録に残る。
- **実 GPU での失敗注入** (test-script 版だけ): `--gpu-test-fail-device dx12` で、DX12 の候補の device 記述子に満たせない limits を入れ、
  実際の wgpu の `request_device` を失敗させる (`LimitsExceeded` が Err で返る。hal の `open` より前なので HRESULT / Lost の再現ではない。
  実 surface で候補が切り替わることの確認として扱い、エラー行の陽性確認は fake logger のテストで分ける)。開発機で、隔離 portable / `--data-dir` により「DX12 失敗 → Vulkan で起動」と、
  `--gpu-backend dx12` との組み合わせで「失敗ダイアログ・案内」を確かめる (ClaudeCode が PC 使用の了承を得て行う)。
- 診断: AdapterInfo・試行ごとの結果 event・失敗段階の受け渡し、ドライバ不明、エラー行 0 / 1 / 2 / 長文、UTF-8 の短縮、crate 直下・子 module・
  対象外の target、device 作成より前 (adapter 列挙中) の Error、別スレッドの Error を root の試行に結び付けないこと、DX12 失敗 → Vulkan 成功で
  DX12 の段階を最終失敗にしないこと、`End(false)` だけでは失敗段階にならないこと、案内の各条件、起動成功後に wgpu の Error を通さなくなること、
  既存の通す対象 (`tantivy::` 等) が変わらないこと、§1.428 の終了順序 (記録 → cleanup → terminal → flush → 表示) と scripted_run の表示抑止。
- FXC: root・HUD・`ui_harness` の Instance 設定が FXC であること。
- 描画: `ui_snapshot` を既定と `MIV_TEST_GPU_BACKEND=vulkan` の両方で。Vulkan で差が出たら報告し、許容差か不具合かを判断する。
  通常 UI・画像・比較・パノラマ・Lanczos・NIS・Pixel AA・Anime4K を Vulkan 起動の実アプリで目視確認する (実機確認の項目に入れる)。
- 動画: native presenter (D3D11) と HUD (別 DX12 Instance) が、root が Vulkan のときも動くこと (実機確認)。

## 9. 文書

- マニュアル (トラブルシューティング・起動オプションの記載箇所) に `--gpu-backend` と、ショートカットへの付け方を書く。
  製品ページの変更は不要の見込み (実装時に確認)。
- `docs/render-init-failure-report-plan.md` §6 を更新 (§5.2)。`docs/architecture-overview.md` / `docs/display-pipeline.md` の GPU 初期化の記述、
  `vendor/egui-wgpu/PATCHES.md` に candidates の追加を記録。`docs/project-reference.md` の起動オプション一覧があれば追記。
- CHANGELOG (v4.8.3) は公開時に利用者の確認を経て書く。

## 10. 第 1 版レビュー指摘への対応

| 指摘 | 対応 |
| --- | --- |
| P2-1 launcher にも値付き flag の登録が要る | §3: launcher の判定にも登録。§8 に組み合わせのテスト |
| P2-2 候補ごとのエラー文を渡す経路が無い | §5.1: 試行ごとの結果 event を、結果が確定した時点で渡す |
| P2-3 logger の target 条件と開始位置 | §7.1: crate 直下を含める・`Begin(Instance)` より前から・最初の `RootPresented` で止める |
| P2-4 エラー行と失敗段階の帰属 | §7.1: 1 つの診断 owner、試行ごとに確定、別スレッドは結び付けない、`End(false)` だけで決めない |
| P2-5 auto で利用不可と分かった Vulkan を案内する | §7.2: 案内の条件を整理 |
| P3-6 panic は §1.428 のダイアログに入らない | §5.1: Err を返す失敗だけが対象と訂正 |
| P3-7 DX12 が失敗したとは確定していない | §5.2: backend 不明と訂正、互換性向上として入れる |
| P3-8 `ui_harness` の変更は GPU fixture に効かない | §3・§4: 範囲を明記、fixture も FXC に |

## 11. 実装・検証記録 (2026-10-26)

### 11.1 実装範囲

- core / launcher の引数処理と help、`wgpu_backend_policy.rs` の要求・surface 対応候補順、test-script 限定の実 device 要求への失敗注入。
- vendor `egui-wgpu` の順序付き候補 callback、各試行の結果 event、GPU 不要の generic 試行関数と8件の回帰検査。既存 selector / web 経路を保持。
- `gpu_startup_diagnostics.rs` に root 診断の単一 owner を追加。起動中の Error filter、試行別確定、別スレッドの除外、Err 復帰時の snapshot と条件付き通知を接続。
- root / HUD / UI harness / 独立 GPU fixture の FXC 固定。UI harness だけが `MIV_TEST_GPU_BACKEND` を読む。
- マニュアル2ページ、旧失敗通知計画§6、architecture-overview、display-pipeline、README索引、vendor PATCHESを更新。CHANGELOG・製品ページは変更しない。

設計の変更はない。コードの小さな相違として、比較・パノラマのテストは共通 `OffscreenGpu` を使うので、その生成元だけを変更した。
追加で見つかった独立 Instance は `gpu_lanczos.rs` の2か所、`app/final_prepare_budget/native_tests.rs`、
`ui_fullscreen/canonical_demand/draw_key_tests.rs`、`ui_fullscreen/page_turn/final_prepare_tests.rs` で、同じFXC指定だけを加えた。
起動記録のキーは `record_detail` が static str を要求するため、最大2候補に対応する `gpu.adapter.1/2`・`gpu.attempt.1/2` を固定文字列で渡す。
値の256 bytes制限とUTF-8境界の短縮は既存起動記録ownerに任せ、通常ログには全文を残す。

独立コードレビュー (GPT-6.1 Sol / xhigh、実装者と別context) と追補レビューを実施。
追加したlauncher新規起動経路テストの `OsString` import漏れ (P2) を修正し、独立再確認済み。最終的に修正必須の未解決指摘なし。
静的レビューの合格は、実 surface・実機の成功を示すものではない。

### 11.2 回帰検査と修正前の確認

- backend policy: 値・大小文字・欠値・不正値・`--`・他flagの値の消費・各APIの候補順・欠落/非対応surfaceを検査。feature付きでは注入flagとDX12だけのlimits変更も検査。
- core / launcher: GPU flag値をパスとして開かないこと、後続パス、`--` の組合せ。既存instanceへの転送では要求を適用せず、起動時だけ解析する。
- 診断: crate直下/子target、対象外、Instance前/列挙中の行、別thread、0/1/2/長いUTF-8行、試行別確定、Vulkan成功後の段階、回復したEnd(false)、ドライバ不明、案内条件、停止後filterを検査。
- 実 `ProductDiagnostics.log` を使うfake logger検査で、root診断への接続・別thread除外・収集停止を確認。diskへの全文保存を検証したものではない。
- 既存の終了境界テストで記録→cleanup→terminal→flush→表示とscripted_runの表示抑止を継続検査。root/HUD/harnessのFXC設定もGPUなしで検査。
- 修正前RED: `HEAD=50925008d7120cb1b8636899ffa4b6498c4a5331` と現在のcore/launcherから、実際のパス解析2関数を独立Rust testへ抽出して同じassertionを適用。
  各baselineはcompile exit 0、test exit 101 (6失敗/1成功)。各currentはcompile/test exit 0 (7成功)。
  失敗は両GPU flagの「パスなし/実パス/`--`後のパス」で値`auto`を開くパスに採用するもの。API不足によるcompile failureではない。
  コマンド・結果・ログは `target/gpuinit-validation/cli_red_results.json`。実行入口は `python target/gpuinit-validation/build_cli_red_fixtures.py` (exit 0)。

### 11.3 自動検証

対象はこの作業ツリーの未コミット差分。native buildは `MSBUILDDISABLENODEREUSE=1`。
初回の並列MSBuild停止後、checkと最初のtest用compileは `CARGO_BUILD_JOBS=1`、
native buildの前提が揃った後のrelease build・追加nextestは24で実行した。設定ファイルは変更していない。
コマンド全文・終了コード・所要時間・stdout/stderrは `target/gpuinit-validation/` の各JSON / logに保存。
以下のF0 / F1 / Gは `-E` に渡した文字列の略記で、JSONには展開済みの実引数を保存する。

```text
F0 = test(/gpu_startup|wgpu_backend_policy|run_native_failure|overlay_instance|requests_product_backends|startup_open_path/)
F1 = test(/gpu_startup|wgpu_backend_policy|run_native_failure|overlay_instance|requests_product_backends/)
G  = test(/egui037_stage4_gpu_tests|compare_wgpu|panorama_wgpu|gpu_lanczos::prewarm_tests|canonical_demand::draw_key_tests|final_prepare_budget::native_tests|page_turn::final_prepare_tests|ordinary_prepare_output_limit_fallback_captures_actual_managed_allocation|back_native_execution_rechecks_no_removal/)
```

| コマンド | exit | 結果 |
| --- | ---: | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 | 通常版。`core-check.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core --features test-script` | 0 | `script-check.log` |
| `cargo check -p mimageviewer-launcher` | 0 | 内包する3本のrelease exeを用意した後に成功。`launcher-check.log` |
| `cargo nextest run --manifest-path vendor/egui-wgpu/Cargo.toml --features winit --lib` | 0 | 45成功。既存のvendor検査方法。run ID `8f0e6866-96ba-4cbf-bce4-29b1b00a0c61` |
| `cargo nextest run -p mimageviewer --lib -E '<F0>'` | 0 | 89成功。`focused.log` |
| `cargo nextest run -p mimageviewer --lib --features test-script -E '<F1>'` | 0 | 29成功。失敗注入の解析・limits設定を含む。`focused-script.log` |
| `cargo nextest run -p mimageviewer --lib -E '<G>'` | 0 | FXC指定を加えた既存GPU fixture関連47成功。`gpu-fixtures.log` |
| `cargo nextest run -p mimageviewer-launcher` | 0 | 43成功、既存の子プロセス専用helper 1件はskip。`launcher-tests.log` |
| `cargo test -p mimageviewer --test ui_snapshot` | 4294967295 | 200成功後、1件がnative例外で停止。非侵入stack採取後、そのtestだけを終了した結果の `0xffffffff`。元の例外コードではない (§11.4) |
| `cargo nextest run -p mimageviewer --test ui_snapshot -E 'test(=folder_toolbar_flexible_snapshots)'` | 0 | 停止したcaseを単独・別processで検査して成功。`snapshot-folder-isolated.log` |
| `cargo nextest run -p mimageviewer --test ui_snapshot` | 0 | 既定backend、201成功。`snapshot-nextest.log` |
| `MIV_TEST_GPU_BACKEND=vulkan cargo test -p mimageviewer --test ui_snapshot` | 101 | 環境変数は子processにだけ設定。189成功 / 12画像差分。期待PNGを更新せず保存 (§11.4) |
| `cargo fmt --check` | 0 | workspace |
| `rustfmt --check --edition 2024 vendor/egui-wgpu/src/lib.rs vendor/egui-wgpu/src/setup.rs vendor/egui-wgpu/src/startup_diagnostics.rs vendor/egui-wgpu/src/adapter_candidates.rs` | 0 | workspace除外vendorも確認 |
| `python scripts/check_ui_glyphs.py` | 0 | 危険グリフ0件 |
| `git diff --check` | 0 | whitespace問題なし |

UIの追加nextest2回は、それぞれのPython親processにだけ `SetErrorMode(SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX)` を設定し、
子のcrash dialogを抑止した。テストの期待値・validation設定・renderer機能は変更しない。

初回の非ゼロ結果も成功扱いしない:

- core check初回は `turbojpeg-sys` の並列CMake/MSBuildで停止 (tool exit 1)。
  同じ生成済みbuild directoryを `cmake --build target/debug/build/turbojpeg-sys-4adae978226e4e06/out/build --target install --config Debug --parallel 1 -- /verbosity:normal /fileLogger /fileLoggerParameters:LogFile=G:/home/mimageviewer-gpuinit/target/turbojpeg-msbuild.log` で検査しexit 0。
  `CARGO_BUILD_JOBS=4` のcore checkもcargo exit 101で停止したが、1で通過した。ffmpeg-sysのAVは観測していない。
- launcher check初回は内包用core / remote / EPUB workerのrelease exe未作成でbuild.rsが停止 (tool exit 1)。各実体のbuild後に再検査した。
- vendor test初回は追加テストの関数型に `&'static str` の明示が足りずcompile失敗 (tool exit 1)、修正済み。
  その後の `cargo test --manifest-path vendor/egui-wgpu/Cargo.toml --features winit --lib` は43成功 / 2失敗 (tool exit 1):
  `hwnd_logs_each_oversized_request_but_not_within_limit_or_nosize` のlog assertionと
  `zero_client_surface_skips_configure_and_acquire_then_resumes_rendering` の `RecreationAttempt`。
  プロセス共有のある並列cargo testの結果を残し、repo既定のnextestで全45件の成功を確認した。該当コード・期待値は変更しない。

### 11.4 snapshotの結果と残る判断

既定のcargo testでは `folder_toolbar_flexible_snapshots` が停止し、PID 4040 / 実行pathがこのworktreeのtest binaryであることを確認した。
`cdb.exe -pv -p 4040 -sins -y G:\home\mimageviewer-gpuinit\target\debug\deps -logo G:\home\mimageviewer-gpuinit\target\gpuinit-validation\ui-snapshot-hang-stack.txt -c "~* kb; q"` (exit 0) で非侵入採取。
test threadは `D3D12SDKLayers` の `RaiseException` → `UnhandledExceptionFilter` → `ZwRaiseHardError` で待機していた。
到達stackは `CreateSampler` → `wgpu_hal::dx12::SamplerHeap::create_sampler` → `egui_wgpu::Renderer::update_texture` → 当該caseの `Harness::render`。
device要求中ではない。製品exeを起動したり、dialogへ入力したりせず、採取後にpathを照合して自分のtest PIDだけを終了した。

当該case単独のnextestは成功し、既定UI全201件のnextestも成功した。
ただし、元の並列cargo testでの停止を解消した証拠ではなく、変更前HEADとの実行比較もしていない。
今回の回帰・既存不具合・driver/検証環境のどれかは未確定。例外コードとSDK layerのメッセージも未取得。
独立レビューでもsampler記述子 / heap / texture uploadの直接の変更はなく、harnessのCPU優先selectorと既定backend maskは従来のままと確認した。
既存のWGL AV (§1.175) と同じ原因とは分類しない。catch追加、validation無効化、描画機能削減は行っていない。

Vulkanは201件を完走し、189成功 / 以下の12件が画像比較で失敗した。
「Diff」はテストが不一致と判定した画素数で、各1〜15。許容する差か不具合かは未判定で、閾値や期待PNGは変えていない。

| snapshot名 | Diff |
| --- | ---: |
| `audio_thumbnail_indicator_modes_light` | 15 |
| `collection_placeholder_light` | 1 |
| `cut_item_appearance_dark` | 6 |
| `cut_item_appearance_light` | 2 |
| `details_icons_dark` | 4 |
| `details_name_colors_dark` | 7 |
| `effetune_show_only_active_tooltip_dark` | 1 |
| `everything_search_disabled_menu_hover_dark` | 1 |
| `file_organize_many_dark` | 1 |
| `file_type_visibility_status_dark` | 1 |
| `global_search_short_term_reject_dark` | 2 |
| `metadata_panel_similar_results_dark` | 7 |

元PNG・Vulkanの `.new.png` / `.diff.png` は `target/gpuinit-validation/vulkan-snapshot-diffs/` に保存し、
画素数は `vulkan-snapshot-differences.json`、判定閾値別の詳細は `snapshot-vulkan.log` に保存した。
最多15画素の比較画像を目視した範囲では大きな配置差は見えないが、12件全体の許容判断の代わりにはしない。
`git diff --name-only -- tests/snapshots` は空。Vulkan指定が効くのはharness経由のUIテストだけで、独立GPU fixture47件をVulkan固定で検査したという意味ではない。

- 2026-10-26 独立コードレビュー (Codex gpt-6.1-sol xhigh、対象 `04b416acc`): **ACCEPT** (指摘なし)。
  Vulkan の 12 件は、比較閾値を超えた画素数が 1〜15 で、8bit 各成分の差の最大は画像ごとに 1〜3 段階。配置の変化・欠落・大きな色の変化は無く、
  backend 間の描画差として許容する。共通閾値の緩和や期待 PNG の一括更新はしない (Vulkan は開き直しで使う側の API で、既定の検査は DX12)。
- ClaudeCode の再実行 (同コミット): `test-lib-full.ps1` 14311 件成功。通常の `cargo test -p mimageviewer --test ui_snapshot` を 2 回流し、2 回とも 201 件成功
  (実装中の D3D12SDKLayers の例外での停止は再発せず)。原因は未確定のまま、以前からの間欠的なテスト実行体の不具合と同種の可能性として記録に残す。

### 11.5 検証用buildと実アプリで確認する操作

launcherの内包入力も必要なので、dev-runtimeの代わりにrelease構成を用意した。
coreは失敗注入を使える **test-script付き・portableなし・未署名の検証用**。配布物ではない。いずれもbuildのみで、製品exeは起動していない。

| コマンド | exit |
| --- | ---: |
| `cargo check --release -p mimageviewer --bin mimageviewer-core --features test-script` | 0 |
| `cargo build --release -p mimageviewer --bin mimageviewer-core --features test-script` | 0 |
| `cargo build --release -p mimageviewer-remote --bin mimageviewer-remote --features embedded-web-assets` | 0 |
| `cargo build --release -p epub-pdf-worker --bin mimageviewer-epub-pdf` | 0 |
| `cargo build --release -p mimageviewer-launcher --bin mimageviewer` | 0 |
| `scripts/check-vcrt-pe-dependencies.ps1 -InputPaths <releaseのlauncher/core/remote/EPUB worker 4本> -RequireCompanionRuntime -ReportPath target/gpuinit-validation/release-runtime-pe.json` | 0 |

FFmpeg 6 DLLと公式VC runtime 4 DLLを `target/release` に実体copyし、runtime 4 / PE 4 の検査が成功。
完全な実引数・条件は `release-preflight-result.json`、`release-core-result.json`、`companions-results.json`、`launcher-results.json` に記録。
通常profileの起動は `Start-Process -FilePath .\target\release\mimageviewer.exe`。
引数なしでは実利用中の `%APPDATA%\mimageviewer` を開き、設定・データを更新し得る。
インストール版 / tray常駐版を先に終了する。今回の実アプリ確認は、repo rootから次の隔離 `--data-dir` で行う。

```powershell
# auto: DX12 request failure -> Vulkan success; verify both attempt records.
Start-Process -FilePath .\target\release\mimageviewer-core.exe -ArgumentList '--data-dir', '.\target\gpuinit-manual', '--gpu-test-fail-device', 'dx12'

# After closing the first run: one DX12 failure, fatal dialog and Vulkan guidance.
Start-Process -FilePath .\target\release\mimageviewer-core.exe -ArgumentList '--data-dir', '.\target\gpuinit-manual', '--gpu-backend', 'dx12', '--gpu-test-fail-device', 'dx12'

# After dismissing the failure: explicit Vulkan, without failure injection.
Start-Process -FilePath .\target\release\mimageviewer-core.exe -ArgumentList '--data-dir', '.\target\gpuinit-manual', '--gpu-backend', 'vulkan'
```

1. auto注入でGUIが開き、隔離directoryのログ・起動記録にDX12失敗とVulkan成功、`request=auto` の採用が残ること。
2. 明示DX12+注入では開き直さず、GPU名・ドライバ・DeviceRequest・Vulkanの起動案内を表示して非ゼロ終了すること。
   limits注入ではHAL Errorが無く「追加情報なし」も正常。HRESULT / Lostの再現ではない。
3. 明示Vulkanで通常UI・画像・比較・パノラマ・Lanczos・NIS・Pixel AA・Anime4K、および動画のnative presenter (D3D11) とHUD (別DX12 Instance) を確認すること。

これらの実アプリ確認と `test-lib-full.ps1` は未実施で、依頼どおりClaudeCodeの後続検証に残す。
報告者の環境の「Parent device is lost」の根本原因と、実driverでのDX12失敗→Vulkan成功も未確認。
commit / push / release公開 / CHANGELOG編集は行っていない。

### 11.6 変更ファイル (26ファイル)

```text
crates/launcher/src/main.rs
docs/README.md
docs/architecture-overview.md
docs/display-pipeline.md
docs/gpu-init-fallback-plan.md
docs/render-init-failure-report-plan.md
htdocs/mimageviewer/manual/settings.html
htdocs/mimageviewer/manual/troubleshooting.html
src/app/final_prepare_budget/native_tests.rs
src/egui037_stage4_gpu_tests.rs
src/gpu_lanczos.rs
src/gpu_lanczos/prewarm_tests.rs
src/gpu_startup_diagnostics.rs
src/lib.rs
src/logger.rs
src/run_native_failure.rs
src/ui_fullscreen/canonical_demand/draw_key_tests.rs
src/ui_fullscreen/page_turn/final_prepare_tests.rs
src/video/native_presenter/overlay_gpu.rs
src/wgpu_backend_policy.rs
tests/support/ui_harness.rs
vendor/egui-wgpu/PATCHES.md
vendor/egui-wgpu/src/adapter_candidates.rs
vendor/egui-wgpu/src/lib.rs
vendor/egui-wgpu/src/setup.rs
vendor/egui-wgpu/src/startup_diagnostics.rs
```

## 12. §1.429 surface 非対応の Err 終了 (2026-10-27、v4.9.0)

状態: 実装済み (ブランチ hud-gpu-1440-1441-1429、未 merge)。
現行§1.448の `native_adapter_candidates` は既に `is_surface_supported` で候補を除外し、
対応候補なしはAdapterSelectionのErrにしているため、候補選択・APIの試行順は変更しない。
残る `get_default_config(...).expect(...)` のErr化は、最初のroot surface設置に限る。
GPU名/backendと失敗段階SurfaceCapabilitiesを既存§1.428/§1.448の失敗ダイアログ・保存ログへ渡す。
初回rootがゼロ寸法の場合も対応確認だけ行い、configure自体はしない。

独立レビューP2へのcoordinator判断により、実行中の新規/immediate/detached viewport、root再設置、
resize/再設定/モニター移動等は変更前のexpectによるpanicへ戻す。起動観測がpendingでも、
それだけではErr化しない。rootかつrender stateを初めて生成するinstall入口でのみ失敗方針を選び、
runtime入口は常にpanicとする。新しい永続flag・viewport stateは持たない。
失敗surface/depth/MSAAを除去して描画を続ける仕組みと、そのためのhelper/testは撤去する。
起動後の資源寿命は変更前と同じなので、detached計画§11の追加記録は不要 (coordinator判断)。
初回rootのErrは既存のrun_native終了処理でPainterごと終了し、続行や再試行はしない。

coordinator判断: 既定uncaptured-error handlerのpanicは今回は変えず、起動中止の伝播経路を追加しない (§1.448と同じ)。
単にloggerへ送るhandlerへの置換は、不正なGPU状態で処理が続くため採らない。
まれな失敗は既定の割り切り (ログ + 通知)。永続データやdetached述語は変更しない。

GPU不要のfailure injectionは `require_surface_configuration(None, ...)` で実際の失敗分岐を通す。
Err/GPU名/段階と成功値・startup通知の範囲をvendorテストで検査し、製品側では通知本文・ログへの接続を検査する。
実HWNDによるsurface失敗注入flagは追加せず、製品は起動しない。

検証結果と同時実装のHUD回帰は以下。本体lib全件・build-dev・独立レビューはClaudeCodeへ引き継ぐ。

### 12.1 同時実装の§1.440/§1.441

- 確認した原因: 共有音量sliderは `drag_stopped` で最後のMove値を保存し、音楽seekは `clicked || dragged` だけで更新していた。
  双方ともreleaseフレームだけ位置が変わる場合に最終値を取りこぼす。音量は `src/video/native_presenter/overlay_draw.rs`、音楽seekは `src/ui_music_panels.rs`。
- `NativePointerTermination` と既存§1.192のreceipt判定を共通入口へ寄せ、動画のproducer receiptと音楽のegui実releaseを同じ `overlay_slider_value` へ渡す。
  押下中/ドラッグ中/取消済みと開始button・開始値・最新値は `OverlaySliderDrag` が所有する。実releaseはreceiptの位置で確定し、取消は開始値へ戻す。
- 音楽seekのwidget所有がEscapeで先に消えても、取消receiptの処理前に開始値を捨てない。取消後のmove/releaseから再取得しない。
  音楽のfocusを保った未通知capture喪失は現状維持、上位Escapeの配送/終了規則は変更しない (coordinator判断)。
- 前回8cbf9e38eで加えたテストは10関数: native実overlay/batchでrelease→後続moveとcapture喪失/Escape、音楽実HUD行でrelease→後続move/別buttonとEscape、
  音楽seek handlerでrelease/Escape (HUD計5関数)。Primary/Secondaryのdragを維持し、Secondaryのクリックだけでは値変更しないことも同じfixtureで検査する。
  製品失敗通知のGPU名/SurfaceCapabilitiesが1関数、vendorのNone注入/成功値・通知範囲とinstall失敗時の自context除去/成功時の資源保持が4関数。
  §1.192の既存pass境界・省略/非対話HUD・§1.448/§1.428の既存回帰も維持する。
  このうちruntimeの失敗資源除去に関する2関数は、独立レビューP2の方針変更に従い撤去した (§12.4)。

### 12.2 旧動作でのREDと自動検証 (8cbf9e38e時点の履歴)

HEAD `7a8a25b06`、ブランチ `hud-gpu-1440-1441-1429` の未コミット差分を検証。全build/testで `MSBUILDDISABLENODEREUSE=1`。
初回native依存を直列で用意した後、最終検証は `CARGO_BUILD_JOBS=24`。
コマンド全文・exit・所要時間は `target/hud-gpu-validation/*.json`、stdout/stderrは対応する `.log` に保存。

`red.py` はHEADの旧音量関数を原文のまま同じmoduleへ置き、新しいstate引数だけ互換shimで接続した。
音楽seek handlerも旧 `clicked || dragged` の判定/位置計算へ差し替えた (抽出後の引数名だけ対応)。
全checkoutを旧版へ戻した試験ではない。コンパイル成功後、同じHUD新規5件が全てassertionで失敗 (nextest exit 100)。
現在の実装へ戻した2ファイルは前後のSHA-256が一致 (`red_restore.json`)。現実装では5件成功し、さらに関連nextestへ含めて検査した。

| 検証 | exit | 結果 |
| --- | ---: | --- |
| cargo fmt (引数なし) | 0 | exit 0。`fmt-complete.log` |
| 通常core check | 0 | exit 0。`check-complete.log` |
| test-script core check | 0 | exit 0。`check-script-complete.log` |
| 関連nextest (通常、§1.192/§1.448/§1.428を含む) | 0 | 726件、726 passed, 13890 skipped。`focused-complete.log` |
| 関連nextest (test-script) | 0 | 35件、35 passed, 14727 skipped。`focused-script.log` |
| cargo test -p mimageviewer --test ui_snapshot | 0 | 213成功 / 0失敗。`snapshot.log` |
| egui-wgpu nextest (winit / lib、全件) | 0 | 49件、49 passed, 0 skipped。`vendor-complete.log` |
| 変更vendorのrustfmt --check | 0 | exit 0。`vendor-fmt.log` |
| viewer context audit (再ビルド済みtoolを最新ソースへ実行) | 0 | 指摘0件。`audit-complete.log` |
| cargo fmt --check | 0 | exit 0。`fmt-check.log` |
| UI glyph check | 0 | 危険グリフ0件。`glyphs-complete.log` |
| git diff --check | 0 | exit 0。`whitespace.log` |

途中の非ゼロ結果も成功扱いにしない:

- 最初の通常check / lib testビルドはturbojpeg-sysのCMake `--parallel 24` がexit 1。Cargo並列数1でnative前提を構築した後に再検査した。
- 実装途中の通常checkに `insert_temp` の戻り型とunitの不一致が1件あり修正済み。
- 新規HUDの初回は4成功/1失敗。音楽seekテストのf32丸めに対する期待値を、正確に表せる50/75秒の座標へ修正した。
  次の実行でEscape時の開始値破棄を検出し、receipt優先の所有解除順へ修正した。最終結果は上表。
- 最初のRED準備scriptはCRLFの境界検索で停止 (テスト未実行)。newlineを正規化し、finallyで原本bytesを復元する形で再実行した。
- ボタン所有を追加した途中のcompileで、別helperのPrimary参照へ置換が及んだE0425が1件あり、その行を原状へ戻した。
  その置換によるoverlay_draw.rsの差分はslider領域だけと確認し、開始buttonを保持したPrimary/Secondary両方の回帰を追加して再検査した。

全てheadless unit / fake-input / offscreen検証。実HWND配送・実環境での非対応surface・最初の失敗ダイアログの実表示は未確認。
製品バイナリは起動せず、通常profileの設定DBなどをテスト準備で操作していない。依頼どおり本体lib全件とbuild-devは未実施。
uncaptured-error handlerの既定panicと音楽の未通知capture喪失は、追加判断待ちではなくcoordinator採択済みの維持範囲。

### 12.3 前回実装8cbf9e38eの変更ファイル (16ファイル)

```text
docs/README.md
docs/gpu-init-fallback-plan.md
docs/keymap-spec.md
docs/next-release-backlog.md
docs/video-architecture.md
src/app.rs
src/run_native_failure.rs
src/ui_music_panels.rs
src/ui_music_panels/slider_tests.rs
src/video/native_presenter/overlay_draw.rs
src/video/native_presenter/render_core.rs
src/video/native_presenter/slider_terminal_tests.rs
src/video/native_touch.rs
vendor/egui-wgpu/PATCHES.md
vendor/egui-wgpu/src/lib.rs
vendor/egui-wgpu/src/winit.rs
```

### 12.4 独立レビューの修正 (8cbf9e38e後)

- P2: Err化を初回root installだけへ限定。runtimeの「logだけで戻る」「失敗資源を外して続行」を撤去し、
  従来のexpect panicへ戻した。既存の製品panic.log経路とuncaptured-error handlerは変更しない。
- root初回/既存render stateのroot/childそれぞれの方針と、起動観測pending中のruntime panicをGPU不要のNone注入で検査する。
  runtimeのconfigure/resize入口は従来と同じvoid契約を維持し、Errを握り潰すcall siteは残さない。
- 起動失敗の保存証明: 一時data-dirのlogsへ製品loggerの実writerを設定し、実NativeExit.recordと実finish_logsを通す。
  run_nativeのUnsupportedSurface Errを注入し、GPU名・SurfaceCapabilities・終了ログがファイルに保存された後に
  同じGPU名/段階/ログフォルダーを含む通知本文が届くことを検査する。scripted_runでも保存し、通知だけを抑止する。
  OnceLockを隔離する単体テスト子processを用い、製品exe・HWND・実MessageBox・通常profileは使わない。
  logger.rsのdependency target filterを広げず、製品の直接logger::log経路で保存されることを確認する。
- P3 (タッチ): 音楽HUDのMove=A/End=BでAが採用される残件は§1.441へ明記した。native動画側はBを使う。
  全eguiタッチへの影響を避けるcoordinator判断に従い、v4.9.0ではegui-winitのTouch End処理を変更しない。
- P3 (detached記録): 起動後のviewport資源寿命の変更を撤去したため、§11への追加記録は行わない。

§12.2は前回の検証履歴。今回の最終結果は以下 (HEAD 8cbf9e38eからの未コミット差分)。
全build/testでMSBUILDDISABLENODEREUSE=1、CARGO_BUILD_JOBS=24。
実引数・exit・所要時間はtarget/hud-gpu-validation/review-*.json、stdout/stderrは同名logへ保存。
初回vendor checkは既存test fixtureがconfigure_surfaceの旧引数形を使うためE0061。
runtime入口を元のvoid契約/引数形へ戻して再検査した (test fixture側の変更なし)。

| 検証 | exit | 結果 |
| --- | ---: | --- |
| cargo fmt (引数なし) | 0 | 成功。`review-fmt.log` |
| 通常core check | 0 | 成功。`review-check.log` |
| test-script core check | 0 | 成功。`review-check-script.log` |
| 関連nextest (前回範囲 + 製品logger target filterの既存検査) | 0 | 728件、728 passed, 13890 skipped。`review-focused.log` |
| 関連nextest (test-script、前回範囲) | 0 | 36件、36 passed, 14728 skipped。`review-focused-script.log` |
| cargo test -p mimageviewer --test ui_snapshot | 0 | 213成功 / 0失敗。`review-snapshot.log` |
| egui-wgpu nextest (winit / lib、全件) | 0 | 48件、48 passed, 0 skipped。`review-vendor.log` |
| viewer context audit | 0 | 指摘0件。`review-audit.log` |
| cargo fmt --check | 0 | 成功。`review-fmt-check.log` |
| 変更vendorのrustfmt --check | 0 | 成功。`review-vendor-fmt.log` |
| UI glyph check | 0 | 危険グリフ0件。`review-glyphs.log` |
| git diff --check | 0 | 成功。`review-whitespace.log` |

保存ログの検査は一時data-dirのlogs/mimageviewer.logを実writerで作り、実record/finish_logsを経由する。
子processは同じlib test exeのexactなignored helperだけを起動する (parentは通常nextest対象)。
MessageBoxのOS呼出だけをcaptureへ置換し、保存済みのファイルとGPU名・段階・API案内・ログフォルダーを含む通知本文を照合した。
通常/feature付きの両方でdialog/scriptモードを検査し、scriptはログ保存を保って通知だけを出さない。
vendorはNone注入で初回root Err、child/root再設置のpanic、起動観測pending中でもruntimeはpanicであることを検査した。
以前の資源除去/保持に関する2件を撤去し、限定方針の検査1件を追加したためvendor全件は49→48件。

HUDコードとegui-winitのタッチ処理は変更していない。起動後の資源寿命を戻したためdetached計画§11の追加記録なし。
製品は起動せず、通常profileをテスト準備で操作していない。本体lib全件/build-devは前回同様ClaudeCodeへ引き継ぐ。
実GPUでの非対応surfaceと実MessageBoxの表示は未実施。既定uncaptured-error handlerは変更しない。

今回の変更ファイルは6件:

```text
docs/README.md
docs/gpu-init-fallback-plan.md
docs/next-release-backlog.md
src/run_native_failure.rs
vendor/egui-wgpu/PATCHES.md
vendor/egui-wgpu/src/winit.rs
```

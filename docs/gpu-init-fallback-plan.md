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

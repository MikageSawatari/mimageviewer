# VC++ ランタイムの無い Windows で起動が終わらない — Store 審査却下の原因調査 (2026-09-15)

バックログ: [§1.241](next-release-backlog.md#1241-vc-ランタイムの無い-windows-で起動画面のまま応答なしになる--store-審査却下の原因-2026-09-15)。
調査・起票は公開担当 (ClaudeCode Opus)。修正は開発担当 (Codex) へ渡す。

## 1. 要約

- **症状**: Visual C++ 再頒布可能パッケージが入っていない Windows で、mIV が「起動中…」の画面のまま
  「応答なし」になり、先へ進まない。Microsoft Store の審査で v3.6.0 がこれで却下された
  (Microsoft の回答: 起動画面から進まず、「mimageviewer (応答なし)」のスクリーンショット付き)。
- **原因**: `onnxruntime.dll` が VC++ ランタイム (MSVCP140 / MSVCP140_1 / VCRUNTIME140 / VCRUNTIME140_1) に
  依存しており、クリーンな Windows では読み込めない (win32error=126)。mIV はこの失敗を
  「AI なしで続行」として扱う設計だが、**`ort` 2.0.0-rc.12 がエラーを作る途中で自分の OnceLock に
  同じスレッドから再入してデッドロック**し、エラーが mIV へ戻らない。これが初回フレームの UI スレッドで起きる。
- **影響**: インストーラ版・単体 exe 版・ポータブル版のすべて。VC++ ランタイムの無い PC では、
  AI 設定に関係なく毎回の起動で止まる (コード上、初回フレームの `ensure_ai_runtime()` は無条件)。
  ゲームや Adobe 製品などで VC++ ランタイムが入っている PC では起きないため、開発機では見えなかった。
- **いつから**: 2026-04-21 `b27e7f9ef`「ort を load-dynamic 化し VC++ 再頒布可能パッケージ依存を排除」以降。
  本体 exe の依存 (起動時の「DLL が見つかりません」ダイアログ) は消えたが、依存は `onnxruntime.dll` 側に残り、
  症状が「無言のハング」に変わった。v3.5.0 / v3.6.0 / v3.10.0 は Sandbox で停止を観測 (§2)。
  それより前のタグは、同じ `ort` rc.12・同じ初回フレーム初期化であることをコードで確認しただけで、実行はしていない。

## 2. 観測

すべて **ClaudeCode が Computer Use でサブ PC の Windows Sandbox を操作して観測** (利用者の了承済み。
Sandbox は使い捨てで、利用者の実環境の mIV は起動していない)。Sandbox 内は Microsoft Defender 無効、x64、
Windows 10.0.26100。インストールは Store と同じ `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART`、起動は
`explorer.exe` 経由 (非昇格)。Sandbox の `System32` に 4 つの VC++ DLL は無い (`DirectML.dll` / `d3d12.dll` はある)。

| 実行 | VC++ DLL の置き場所 | 結果 |
| --- | --- | --- |
| v3.6.0 | なし | 「mimageviewer (応答なし)」+「起動中…」で停止。Microsoft のスクリーンショットと同じ見た目 |
| v3.10.0 | なし | 同上。タスクマネージャーでダンプ採取 → PDB 付きで主スレッドを確定 (§3) |
| v3.5.0 | なし | 同上。`Responding=False`、panic.log に `UI THREAD HANG suspected` が 10 回 |
| **v3.10.0 陽性対照 (a)** | **core exe の隣** (`%APPDATA%\mimageviewer\runtime\3.10.0\`、初回起動前に配置) | **起動した**。`[AI] Runtime initialized (… effective=DirectMl)`、`Responding=True`、初回設定ダイアログまで表示 |
| **v3.10.0 陽性対照 (b)** | **`onnxruntime.dll` の隣だけ** (`%APPDATA%\mimageviewer\`、(a) の後に移動して再起動) | **停止した**。`Responding=False`、`[AI] Runtime initialized` 無し、`UI THREAD HANG suspected` |

(b) の結果は、`onnxruntime.dll` をフルパスで `LoadLibraryExW(path, NULL, 0)` するとき、依存 DLL は
**exe のディレクトリ**からは探されるが **DLL 自身のディレクトリからは探されない**という標準の探索順と合う。
core の作業ディレクトリは launcher から継承 (explorer 起動) で、`%APPDATA%\mimageviewer` ではない。

ログの最後 (VC++ 無し): `onnxruntime.dll extracted …` → `onnxruntime_providers_shared.dll extracted …` の後、
`[AI] Runtime initialized` も `[AI] Runtime init failed` も出ない。panic.log は
`no App::update heartbeat yet; native_window_health=inactive` (最初の `App::update` が戻っていない)。

証跡 (この PC):

- `target/msstore-hang-20260915/` — 各実行のログ (`sandbox-*`)、`cdb-v3.10.0-symbolized-threads.txt`、
  `cdb-v3.6.0-*.txt`、陽性対照の `run.txt`
- `C:\miv-sandbox\results\` — ダンプ本体 (各 1.2 GB) と同じログ
- Sandbox 用スクリプト: `C:\miv-sandbox\run-test.ps1` / `run-vcrt-test.ps1` / `collect-miv-logs.ps1` / `miv.wsb`

## 3. 原因の経路 (コードとダンプ)

v3.10.0 ダンプ + `target/release/mimageviewer_core.pdb` の主スレッド (上が新しい):

```
KERNELBASE!WaitOnAddress
std::sys::sync::once::futex::Once::call
ort::util::once_lock::OnceLock<T>::try_init_inner      ← G_ORT_LIB (2 回目、同じスレッド)
ort::setup_api
std::sync::once::Once::call_once_force::{{closure}}
std::sys::sync::once::futex::Once::call
ort::util::once_lock::OnceLock<T>::try_init_inner      ← G_ORT_API
ort::error::Error::new_internal::{{closure}}            ← CreateStatus のため api() を呼ぶ
ort::util::stack::with_cstr::run_with_heap_cstr
ort::error::Error::new_internal                        ← DLL ロード失敗をエラーにする
std::sync::once::Once::call_once_force::{{closure}}
std::sys::sync::once::futex::Once::call
ort::util::once_lock::OnceLock<T>::try_init_inner      ← G_ORT_LIB (1 回目、ロード中)
std::sync::once_lock::OnceLock<T>::initialize          ← mIV 側の OnceLock
mimageviewer::ai::runtime::AiRuntime::new_with_backend
… App::update (初回フレーム) → egui / eframe / winit
```

- `ort` 2.0.0-rc.12 の `src/lib.rs` (`G_ORT_LIB` / `G_ORT_API` / `setup_api`) と `src/error.rs`
  (`Error::new_internal` が `ortsys![CreateStatus]` を呼ぶ) で照合済み。**ロード失敗を報告する経路が、
  まだ初期化中の `G_ORT_LIB` を再度初期化しようとする**。上流の欠陥で、DLL が読めない理由が何であっても
  (VC++ 不足・破損・セキュリティソフトによる阻止など) 同じくハングすると読める (VC++ 不足以外は未観測)。
- 呼び出し元: [app.rs](../src/app.rs) の初回フレーム `if !self.initialized { … self.ensure_ai_runtime(); }`
  と `fn ensure_ai_runtime` → [ai/runtime.rs](../src/ai/runtime.rs) の展開 + `ort::init_from(&dll_path)`。
  `ensure_ai_runtime` は失敗時に `[AI] Runtime init failed` をログして AI なしで続行する作りで、
  **その `Err` が戻ってこないことが欠陥の本体**。
- `AiRuntime::new_with_backend` を呼ぶ他の経路も同じ `ort` の初期化を通る:
  [remote_ipc/session.rs](../src/remote_ipc/session.rs)、[materializer.rs](../src/materializer.rs)、
  TensorRT の [trt_worker_runtime.rs](../src/ai/trt_worker_runtime.rs) / [tensorrt_builder.rs](../src/ai/tensorrt_builder.rs)
  (TensorRT pack の DLL で `ort::init_from`)。

### 同梱 PE の VC++ ランタイム依存 (2026-09-15、VS 18 の `dumpbin /dependents`)

| ファイル | MSVCP140 / VCRUNTIME140 系 |
| --- | --- |
| `vendor/ort/onnxruntime.dll` | **MSVCP140, MSVCP140_1, VCRUNTIME140, VCRUNTIME140_1** |
| `vendor/ort/onnxruntime_providers_shared.dll` | **VCRUNTIME140** |
| FFmpeg 6 DLL | なし (UCRT の `api-ms-win-crt-*` のみ。Windows 10 以降は OS 同梱) |
| `pdfium.dll` / susie32 / vst3-host / core / remote / launcher | なし |

本体に同梱する物で VC++ ランタイムを要するのは ORT の 2 DLL だけ。

### 追加ダウンロード (2026-09-15、開発機にインストール済みのパックを dumpbin)

**TensorRT パック** (`%APPDATA%\mimageviewer\tensorrt\`、pack version 3 = アプリの `EXPECTED_TRT_PACK_VERSION` と同じ、ORT GPU 1.24.2):

| ファイル | MSVCP140 / VCRUNTIME140 系 |
| --- | --- |
| `onnxruntime.dll` (GPU 版) | **MSVCP140, MSVCP140_1, VCRUNTIME140, VCRUNTIME140_1** |
| `onnxruntime_providers_cuda.dll` | **MSVCP140, VCRUNTIME140, VCRUNTIME140_1** |
| `onnxruntime_providers_tensorrt.dll` | **MSVCP140, VCRUNTIME140, VCRUNTIME140_1** |
| `onnxruntime_providers_shared.dll` | **VCRUNTIME140** |
| NVIDIA の 13 DLL (cuBLAS / cuBLASLt / cudart / cuDNN 3 本 / cuFFT / nvJitLink / nvinfer / nvinfer_plugin / nvonnxparser / NVRTC 2 本) | なし (KERNEL32 等の OS DLL だけ。CRT は静的リンク) |

- TensorRT は本体プロセスではなく**子プロセス** (`current_exe()` を `--tensorrt-infer-worker` で起動) で
  `AiRuntime::new_with_backend(TensorRt)` → パックの `onnxruntime.dll` で `ort::init_from` する
  ([trt_worker_pool.rs](../src/ai/trt_worker_pool.rs) / [trt_worker_runtime.rs](../src/ai/trt_worker_runtime.rs))。
  依存 DLL の探索には `SetDllDirectoryW(パックのフォルダ)` と PATH 先頭追加を使う ([ai/runtime.rs](../src/ai/runtime.rs))。
- **VC++ ランタイムが無い場合 (コードからの推定、未観測)**: 子プロセスの `ort` 初期化が §3 と同じくデッドロック →
  バックグラウンドスレッドの起動ハンドシェイクが 45 秒で timeout → 子を kill →
  エラー文の「通信失敗」で一時的な失敗と判定され 1 回だけ自動再試行
  ([trt_worker_notice.rs](../src/ui_dialogs/trt_worker_notice.rs) `is_transient_spawn_failure`) →
  「ワーカー起動 timeout / 通信失敗」の通知が出て DirectML で続行。**画面は固まらないが、原因と違う理由が表示され、
  TensorRT を 2 回・合計 90 秒以上待つ**。ただし現状は、その前に本体の DirectML 初期化 (§3) で起動自体が止まる。
- 修正への影響: 子プロセスは本体と同じ exe なので、**VC++ ランタイムを core exe の隣に置く方式 (候補 B) なら
  TensorRT パックの ORT GPU 版にも効く** (exe のディレクトリは `SetDllDirectoryW` の指定より先に探索される)。
  パック側に VC++ DLL を足す必要は無い見込み。候補 A (失敗を戻す) も子プロセス側の `ort::init_from` に同じく必要。
  パックが要求する VC++ ランタイムの版は本体の ORT と同じ 1.24.2 系だが、同梱する版はパック側の要件も満たすこと。
- Windows Sandbox には NVIDIA GPU / CUDA が無いので、TensorRT 経路はクリーンな環境で観測できない。
  修正後は、VC++ ランタイムのある開発機での TensorRT 動作 (回帰なし) と、子プロセス側の失敗が timeout ではなく
  正しい理由で報告されること (テストまたはログ) で確認する。

**編集用パック** (`%APPDATA%\mimageviewer\addons\editing\packs\2026.06.0\`): フォント 18 種 (`.ttf`) +
`models/birefnet_fp16.onnx` + ライセンス文 + `pack-manifest.json` + `INSTALL_OK`。**DLL / exe は含まない**
(パック作成ツール [build_editing_pack.rs](../src/bin/build_editing_pack.rs) の対象もフォントとモデルだけ)。
フォントは OS の DLL を使わず読み込む。被写体分離モデルは本体の DirectML ランタイム (§3 の ORT) で動くので、
**§3 の修正に含まれる以外の対応は不要**。

## 4. 修正の候補 (判断は開発担当)

**A. 失敗が mIV へ戻るようにする (欠陥の本体)**
`ort` の失敗経路がデッドロックする限り、VC++ 以外の理由でロードに失敗しても同じく止まる。

- `ort` の新しい版で直っているかを確認する (未確認)。直っていれば更新。ただし `ort` の版は
  onnxruntime の C API 版と揃える必要がある (CLAUDE.md「ONNX Runtime 管理」)。
- 直っていなければ、`ort::init_from` の前に mIV 側で `onnxruntime.dll` を読めるか確かめ、読めなければ
  `ort` に触れずに型付きのエラーで返す、または `ort` を patch する、など。上流への報告も検討。
- CLAUDE.md「バグ修正の一般原則」の症状パッチに当たらないか (上流の欠陥を避ける境界として正当か) を
  レビューで合意すること。

**B. クリーンな Windows でも AI が使えるようにする (発生条件の解消)**

- VC++ ランタイム 4 DLL を**core exe と同じディレクトリ**に置く (陽性対照 (a) で起動を観測、(b) の DLL 隣では不可)。
  - 単体 exe / インストーラ版: launcher の内包資産に加えて `runtime\<version>\` へ展開する経路が候補
  - ポータブル版: exe の隣へ loose 同梱 (`onnxruntime.dll` も exe の隣なので同じ条件。実行は未観測)
- 入手元は System32 のコピーではなく、Visual Studio の再頒布用フォルダ
  (`VC\Redist\MSVC\<ver>\x64\Microsoft.VC145.CRT\`。この PC では 14.50.35710) を使う。
  ORT 1.24.2 のビルドに使われた版以上であること。Microsoft 署名済みなので再署名しない。
- exe 隣の DLL は System32 より先に使われるので、同梱する版を古いまま放置しない運用も決める。
- 陽性対照 (a) では検証用に**開発機 System32 のコピー**を使った (再頒布用フォルダの版ではない)。

**C. UI スレッドを止めない (応答性)**

- 17 MB の DLL 展開と `LoadLibrary` を初回フレームの UI スレッドで同期実行している
  ([ui-responsiveness.md](ui-responsiveness.md) §4 の対象)。worker へ移せば、失敗しても画面は生きる。
- ただし `ort` の OnceLock が worker 側でデッドロックすると、後で UI から `ort` に触れた時点で同じく止まるので、
  **C 単独では直らない**。A と組み合わせる前提。

**D. 利用者に見える形にする (任意)**

- AI ランタイムを使えないとき、環境設定の AI ページ等で理由 (「Visual C++ ランタイムが見つからない」等) を
  表示する。現状はログにしか出ない。

## 5. 公開手順・文書側の直し (修正と同時に)

- CLAUDE.md の「VC++ 再頒布可能パッケージ不要」の記述 3 か所 (Tech Stack の ONNX Runtime DLL、
  「ONNX Runtime 管理」の利点、「Distribution」の CRT 静的リンク) は `onnxruntime.dll` について誤り。
  修正内容に合わせて書き直す。
- リリースチェックリスト Phase 3 step 12 の `dumpbin /dependents` は本体 exe しか見ていない。
  **同梱する全 PE (特に `onnxruntime*.dll`)** へ広げる。
- クリーンな Windows (Windows Sandbox) での起動確認を、少なくとも Store 申請前の手順に加える。
  手順とスクリプトは本書 §2 の `C:\miv-sandbox\`。

## 6. 別件 (今回の原因とは無関係)

- v3.6.0 の Sandbox 初回起動ログに `content_identity: DB open failed: create edit_origin: database is locked`
  が 1 回 ([content_identity.rs](../src/content_identity.rs) の `create edit_origin`)。新規プロファイルでの
  初回起動時の競合と見られる。v3.5.0 / v3.10.0 の Sandbox ログには出ていない。影響は未調査。

## 7. 対外

- Microsoft へは利用者が続報を送る (原因が分かったこと、前回の質問は不要になったこと、修正版で再申請すること)。
  下書き: `target/msstore-hang-20260915/ms-store-followup-email.md`。
- Store への再申請は修正版の正式リリース後。申請前に §5 の Sandbox 起動確認を通す。

# TensorRT ワーカープロセス方式設計 (Phase 3)

## 背景と動機

Phase 2-full では「設定で TensorRT を有効化 → アプリ再起動」というフローを実装した。
しかしこの方式には以下の課題がある:

1. **再起動が必要**: `ort::init_from()` がプロセス内 1 回限りなので、DirectML →
   TensorRT 切り替えに再起動が必須。
2. **MI-GAN を DirectML で動かせない**: TensorRT pack の `onnxruntime.dll` は
   DirectML EP を含まないので、TRT バックエンド時は MI-GAN も TRT (5+ 分かかる
   エンジンビルド) か CPU フォールバック (低速) に限定される。
3. **クラッシュ伝搬**: TRT の問題が GUI 本体を巻き込む。

PDFium ワーカーと同じ方式 (子プロセス) で TensorRT を分離することで、すべて
解消できる。

## 全体構造

```
mImageViewer.exe (メインプロセス、DirectML 専用)
├─ AiRuntime
│   ├─ DirectML セッション (アップスケール / デノイズ / MI-GAN / 分類)
│   └─ TrtWorkerPool (Settings の ai_backend が TensorRt のときのみ起動)
│       ├─ Worker 1: mImageViewer.exe --tensorrt-infer-worker
│       └─ (1 ワーカーで十分。TRT セッションを 8 モデル分保持)
│
└─ ai_backend = TensorRt 時の挙動:
    上述 worker pool に推論をルーティング (model別)
    - Upscale 系: Worker
    - Denoise: Worker
    - MI-GAN: メイン DirectML (TRT は遅い/不安定なため除外)
    - Classifier: メイン DirectML (TRT で engine 生成されないため除外)
```

メインは常に DirectML を保持。TensorRT が必要なときだけワーカーに丸投げ。

## なぜ メイン= DirectML / ワーカー= TensorRT か

逆 (メイン= TRT) ではなく メイン= DirectML を選ぶ理由:

1. **TensorRT pack 未インストール時も DirectML は常に使える** = 起動失敗なし
2. **MI-GAN や Classifier の DirectML フォールバックがメインで完結** = 別ワーカー不要
3. **TensorRT を有効化したくないユーザー** はワーカープロセス起動コストすら払わなくて済む
4. **TensorRT pack の更新中** でもメインは動いている

## IPC プロトコル

### 制約

タイル単位推論の入出力サイズ (256x256 タイル、4x スケール、FP32):
- 入力: 256 × 256 × 3ch × 4byte = 786 KB
- 出力: 1024 × 1024 × 3ch × 4byte = 12 MB

タイル 1 個 13 MB の往復。標準入出力パイプ (デフォルト 64 KB バッファ) では
帯域不足。**共有メモリ (Windows CreateFileMapping) を使う**。

### 通信ハンドシェイク

```
[親 → 子]: stdin にコマンド JSON 1 行
  { "cmd": "load_model", "model_kind": "realesrgan_anime6b" }
  { "cmd": "infer", "model_kind": "realesrgan_anime6b",
    "input_shm": "miv_trt_in_<pid>_0", "input_bytes": 786432,
    "input_shape": [1, 3, 256, 256],
    "output_shm": "miv_trt_out_<pid>_0", "output_capacity": 12582912 }
  { "cmd": "shutdown" }

[子 → 親]: stdout にレスポンス JSON 1 行
  { "ok": true, "elapsed_ms": 12 }
  { "ok": true, "elapsed_ms": 8, "output_shape": [1, 3, 1024, 1024] }
  { "ok": false, "error": "..." }

実データ (input/output テンソル) は shm 経由
親が input_shm にデータを書き → コマンド送信 → 子が output_shm に書く →
レスポンス受信後に親が output_shm から読む
```

### 共有メモリ管理

各 (model, input_shape) で固定サイズの shm 領域を 2 個 (in/out) を pre-allocate。
ワーカー側のセッション維持と同じライフサイクルで管理。

異なる入力サイズが混在する場合 (画像サイズが違う、デノイズの 256 固定 vs
アップスケールの 256 等) は複数 shm 領域を使い分ける。

実装: `windows::Win32::System::Memory::CreateFileMappingW` + `MapViewOfFile`。
名前付きで親・子両方からアクセス。

### コマンドキュー

ワーカーは 1 度に 1 推論しか実行しない (TRT セッションが mutex 内に閉じている)。
親側で複数並列リクエストが来た場合は親側のキューで直列化。

## ファイル構成

```
src/
├── ai/
│   ├── runtime.rs           # AiRuntime: with_session を「ローカル DirectML or
│   │                          ワーカー TRT」にディスパッチ
│   ├── trt_worker_pool.rs   # 新規: ワーカー起動・IPC・推論ルーティング
│   ├── trt_worker_proto.rs  # 新規: IPC プロトコル型 (Cmd / Resp / Shm 名)
│   ├── tensorrt_pack.rs     # 既存
│   ├── tensorrt_builder.rs  # 既存 (--tensorrt-build engine 事前ビルド用、
│   │                          ワーカープール起動とは別経路)
│   └── ...
└── bin/
    └── (エントリは src/main.rs の --tensorrt-infer-worker 分岐)
```

メイン exe の `--tensorrt-infer-worker` モード (新規):
- ORT を TRT pack でロード
- stdin から JSON コマンドを読みループ
- shm 経由でデータ授受
- stdout に結果 JSON

## モデル別ルーティング

```rust
fn infer_target(kind: ModelKind, backend: AiBackend) -> InferTarget {
    match (backend, kind) {
        // TRT バックエンドでも、これらは常にメインの DirectML を使う
        (AiBackend::TensorRt, ModelKind::InpaintMiGan) => InferTarget::DirectMlLocal,
        (AiBackend::TensorRt, ModelKind::ClassifierMobileNet) => InferTarget::DirectMlLocal,
        // TRT 効果が高いモデルはワーカーへ
        (AiBackend::TensorRt, _) => InferTarget::TrtWorker,
        // それ以外は全部ローカル DirectML
        _ => InferTarget::DirectMlLocal,
    }
}
```

ユーザーから見ると:
- アップスケール: TRT で 1.4-3.4x 高速化
- デノイズ: TRT で 4.5x 高速化
- 消しゴム (MI-GAN): DirectML で 1-2 秒/推論 (現状維持)
- 画像分類: DirectML で瞬時 (現状維持)

## 設定 UI の変更

### 削除

- 「アプリ再起動が必要」のバナー (もはや不要)
- 「今すぐ再起動」ボタン (preferences、trt_build dialog 両方)
- `App::request_app_restart()` メソッド (撤去)

### 残す

- バックエンド選択 (DirectML / TensorRT) → トグルで即時 hot-reload
- TRT pack インストール状態表示
- 「全エンジンを今すぐビルド」ボタン (= `--tensorrt-build` 子プロセス、これは
  ワーカーとは別)

### ホットリロードのフロー

```
ユーザーが TRT を選択 (現状 DirectML 動作中)
  ↓
TrtWorkerPool::ensure_started() を呼ぶ
  ↓
- pack 存在確認 → なければ「未インストール」表示で UI 通知
- ワーカープロセス起動 (mimageviewer.exe --tensorrt-infer-worker)
- ワーカー側で 8 モデルセッションをロード (キャッシュ済みエンジンなら数秒)
  ↓
完了通知が来たら、AiRuntime の dispatcher が TRT 経路を使い始める
DirectML セッションはそのまま温存 (MI-GAN / Classifier 用)

ユーザーが DirectML に戻す
  ↓
TrtWorkerPool::shutdown()
  ↓
ワーカーに stdin で `{"cmd":"shutdown"}` 送信、子プロセス自然終了
DirectML セッションだけ残る
```

設定ページで「TRT を有効化」を押した時点で:
- pack あり → 上記フロー、再起動なし
- pack なし → 「インストールしてください」表示 (現状の手順)

## 共有メモリ設計の詳細

### サイズ計算

タイルサイズは固定 (デノイズ 256、アップスケール 256 (TRT)、軽量 512)。
バッチは 1 (今のところ)。

| モデル | 入力 shm | 出力 shm | 用途 |
|---|---|---|---|
| アップスケール (256, 4x) | 786 KB | 12.6 MB | 5 モデル共通 |
| 軽量アップスケール (512, 4x) | 3.1 MB | 50 MB | UpscaleRealEsrGeneralV3 専用 |
| デノイズ (256, 1x) | 786 KB | 786 KB | DenoiseRealplksr |

合計 shm 領域: 約 67 MB (ワーカー 1 個分)。アップスケールタイルサイズが
将来変わる可能性に備え、shm は最大サイズで pre-allocate しておく。

### 名前付け

```
miv_trt_in_<pid>_<role>     親プロセス PID + 役割 (upscale / denoise / etc)
miv_trt_out_<pid>_<role>
```

PID 含めることで複数 mIV インスタンス起動時の衝突回避。

## ワーカーライフサイクル

### 起動

設定で TRT 有効化 + 「TRT 機能が必要なモデルが呼ばれたら」 (lazy):
- `TrtWorkerPool::ensure_started()` を初回 infer 直前に呼ぶ
- ワーカープロセス spawn → ハンドシェイク → モデルロード
- 起動失敗 (pack 不在等) は `AiBackend::DirectMl` に自動退避し UI 通知

### シャットダウン

メインプロセス終了時:
- 親が stdin に `{"cmd":"shutdown"}` 送信
- 子は ORT セッションを破棄して exit
- 親は子の終了を `child.wait()` で待つ

ユーザーが設定で DirectML に戻したとき:
- 同様にシャットダウン → DirectML 経路に切り替え

### クラッシュ検出 (Step 5 実装済み)

ワーカーが予期せず終了したら:

- `TrtWorkerPool::is_dead: AtomicBool` を `load_model` / `infer` 内の send_cmd
  失敗で立てる。検出パターン:
  - `stdin write: ...` (親→子書き込み broken pipe)
  - `stdin flush: ...`
  - `stdout read: ...`
  - `worker stdout が EOF (子プロセスが予期せず終了した可能性)`
- `ok=false` の正常レスポンス (例: shape mismatch) では `is_dead` を立てない
  (送受信は成立しているので worker は生存)
- `infer_via_worker` が `pool.is_dead()` を見て `AiRuntime::report_worker_died()`
  を呼ぶ。中身: `detach_worker_pool()` (= worker_pool を None に) + UI 通知キュー
  `worker_notice` に `WorkerNoticeKind::DiedDuringInfer` を積む
- 以降の `should_route_to_worker` は worker_pool が None なので false → 推論は
  自動的に DirectML へフォールバック
- UI は毎フレーム `take_worker_notice()` で 1 回だけ通知を引き取り、右上に floating
  banner (「TensorRT ワーカーが停止」)。「ワーカーを再起動」ボタンで
  `spawn_trt_worker_pool` を再呼び出し可能 (連続失敗時はまた通知される)
- 自動再起動はしない (連続クラッシュを避けるため、明示的なユーザー操作で復旧)
- `TrtWorkerPool::shutdown` 時に `is_dead` なら念のため `child.kill()` してから
  `child.wait()` (子が hang した病理的ケースの保険、kill は冪等)

起動失敗 (pack 不在 / DLL ロード失敗 / 初期化エラー) も同じ通知機構で扱う:
`spawn_trt_worker_pool` の Err 経路で `report_worker_spawn_failed()` を呼んで
`WorkerNoticeKind::SpawnFailed` を積む (これまでは log のみだった)。

## 実装フェーズ

### Step 1: IPC 基盤 (約 2 日)

- `trt_worker_proto.rs` 型定義
- `trt_worker_pool.rs` の骨組み (Spawn/Shutdown/共有メモリ管理)
- ワーカー側エントリ (`run_infer_worker`)
- 最小ハンドシェイク (`load_model` のみ実装、infer は未対応)
- Windows shm の Rust ラッパー (`windows` crate 利用)

### Step 2: 推論 IPC (約 2 日)

- `infer` コマンドの実装 (shm に書き、コマンド送り、shm から読む)
- ワーカー側で `runtime.with_session(kind, |s| s.run(...))` を呼ぶ
- 入出力 shape のシリアライズ・デシリアライズ
- エラー伝搬

### Step 3: メイン側ディスパッチ (約 2 日)

- `AiRuntime::with_session` を `infer_target` で分岐
- ローカル DirectML 経路は既存維持
- TRT 経路は worker_pool 経由
- TRT 経路の Send/Sync 制約クリア (mpsc + Arc<Mutex>)

### Step 4: 設定 UI 変更 (約 1 日)

- 再起動プロンプト撤去
- バックエンド選択 = hot-reload
- ホットリロードのテスト

### Step 5: クラッシュ耐性 + UI 通知 (実装済み)

- `TrtWorkerPool::is_dead` フラグ + `load_model` / `infer` 内の I/O 失敗パターン
  自動判定 (`classify_io_error`)
- `AiRuntime::report_worker_died` / `report_worker_spawn_failed` で UI 通知キュー
  `worker_notice: Mutex<Option<WorkerNotice>>` に積む
- `App::poll_trt_worker_notice` が毎フレーム `take_worker_notice()` を呼んで
  `App::trt_worker_notice` に転写
- `ui_dialogs/trt_worker_notice.rs` が右上に floating Window を出す:
  - メッセージ + [ワーカーを再起動] [閉じる]
  - 時間で消えない (ユーザー認知が必要)
  - TRT エンジンビルダー進捗ダイアログ表示中は隠す

### Step 6: 統合テスト + ドキュメント (約 1-2 日)

- 全モデルでベンチ (DirectML / TRT for upscale / DirectML for MI-GAN を確認)
- 既存テスト (test_ai_upscale 等) が新パスでも通るか
- マニュアル更新 (再起動が要らなくなった旨)

合計 **約 9-10 営業日 (≈ 2 週間)**。

## 互換性とリスク

### 既存ユーザーへの影響

- Phase 2-full の再起動プロンプトを撤去するが、設定は維持される (`ai_backend` は
  そのまま)
- 既存 TRT pack は無変更で再利用可能
- 既存エンジンキャッシュは無変更で再利用可能 (ワーカー化で hash は変わらない)

### 主要リスク

1. **共有メモリの競合状態**: ワーカーが応答しない、shm が壊れる
   → 対策: タイムアウト + クラッシュ検出で DirectML に自動退避
2. **大画像時の shm overflow**: タイルサイズが想定外に大きいケース
   → 対策: 最大 shm サイズを overscan で確保 (50 MB 出力でも OK)
3. **複数モデルの並行リクエスト**: 親側で並列発行された場合
   → 対策: 親側 mpsc キューで直列化 (現状でも sequential なので OK)
4. **Worker spawn 失敗 (pack 破損等)**: 起動時にだけ顕在化
   → 対策: spawn 失敗を即検知 → DirectML に切り替え + UI 通知
5. **ORT runtime ABI 変更**: `Microsoft.ML.OnnxRuntime.Gpu.Windows` のメジャー
   バージョンが変わるとワーカーは再ビルド必要
   → 対策: pack バージョンと exe バージョンを manifest で照合 (既存 INSTALL_OK)

## TRT エンジン事前ビルド (Phase 2 既存) との関係

`mimageviewer.exe --tensorrt-build <model>` (既存、Phase 2-full step 2) は
**そのまま残す**。役割:

- 設定 UI の「全エンジンを今すぐビルドする」ボタンから呼ばれる
- 各モデルの engine cache を populate する (ワーカープール起動を待たずに事前準備)
- ワーカープール経由でも初回エンジンコンパイルは発生するが、事前ビルド済みなら
  ワーカー起動の瞬時化 (cold start ~3 秒、warm start ~30 秒以上短縮)

つまり:
- `--tensorrt-build`: 短命プロセス、エンジン compile + 終了。事前準備用。
- `--tensorrt-infer-worker`: 長命プロセス、コマンド受信 + 推論ループ。実行時用。

両者は別々のサブコマンド、別々のライフサイクル。

## Phase 3 パフォーマンス測定結果

Phase 3 着手前に懸念していた「ワーカー化で per-tile IPC overhead が大きく出るのでは」
という点を実測で検証した結果のサマリ (RTX 4090、Windows 11、ORT GPU 1.24.2、
TensorRT 10.16、CUDA 12.9 にて)。

### IPC オーバーヘッドの最適化フェーズ

worker pool を導入した素朴実装からの段階的最適化:

| 実装 | nmkd_siax_4x (1024² 入力) wall total | per-tile session_run |
|------|-------------------------------------:|---------------------:|
| Phase 3 Step 3 (毎タイル shm create/open) | 484 ms | 21 ms |
| Phase 3 Step 3.5 (永続 shm 再利用) | 361 ms | 21 ms |
| Phase 3 Step 3.6 (zero-copy slice cast) | 280 ms | 21 ms |
| Phase 2 直接 TRT (参考、worker 不使用) | 270 ms | 21 ms |

zero-copy 化後、Phase 3 worker のオーバーヘッドは **1〜4 ms/tile** (16 タイル合計
~30 ms) まで縮まり、Phase 2 直接 TRT との差はノイズに埋もれる規模になった。
当初懸念の「~15 ms/tile」は永続化前の素朴実装値。

### bench セッション間のばらつき

Step 3.6 完了後 → Step 4/5 着手前に追測したところ、同じバイナリ・同じ engine cache
でも `session.run` 自体が 21 ms から 35 ms に増えるケースを観測した。原因切り分け:

1. `--legacy-direct-trt` (Phase 2 互換、worker 不使用) でも 35 ms 出る
   → アーキテクチャ起因ではない
2. `MIV_TRT_BENCH_LOOP=5` で同 tensor を 5 連続 session.run、全回 30〜36 ms 均一
   → GPU 初回 warmup や sleep ではない (それなら 1st だけ遅い)
3. `SetPriorityClass(HIGH_PRIORITY_CLASS)` を子プロセスに設定 → 効果なし
   → Win32 スケジューラ粒度ではない

結論: GPU 熱・他プロセス影響・Windows のバックグラウンドタスク等の **環境要因**。
GPU が冷えた状態 / 他に重いプロセスがいない状態で実測すると 21 ms に戻る。
本番運用でも同様の揺らぎが起きうるため、ユーザー向け数値は「最良 21 ms、混雑時
35 ms」のレンジで認識する。

### モデル別の DirectML / TensorRT 比較 (FP16、Phase 3 Step 3.6 時点)

| モデル | DirectML wall (ms) | TRT (worker) wall (ms) | 倍率 |
|---|---:|---:|---:|
| upscale_realesrgan_x4plus | ~3500 | ~1800 | ~1.9x |
| upscale_realesrgan_anime6b | ~3200 | ~960 | ~3.4x |
| upscale_realesrgan_general_v3 | ~2200 | ~1500 | ~1.5x |
| upscale_realcugan_4x | ~3800 | ~2700 | ~1.4x |
| upscale_nmkd_siax_4x | ~4000 | ~1500 | ~2.7x |
| denoise_realplksr | ~1100 | ~240 | ~4.5x |

(画像サイズ 1024×1024、タイル境界条件込みの wall。手元の overnight bench から、
個別タイル `session_run` ではなく upscale 全体時間)

これらの倍率はマニュアル `htdocs/.../settings.html` の「アップスケール 1.4〜3.4 倍、
ノイズ除去 約 4.5 倍」表記の根拠。

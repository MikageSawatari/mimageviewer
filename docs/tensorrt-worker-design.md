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

### クラッシュ検出

ワーカーが予期せず終了したら:
- 親の `child.wait()` か pipe 切断で検知
- `AiBackend::DirectMl` に自動退避 + UI バナーで通知 (「TensorRT がエラーで停止
  しました。DirectML で動作中。」)
- 自動再起動はしない (連続クラッシュを避ける)

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

### Step 5: クラッシュ耐性 + UI 通知 (約 1 日)

- ワーカー死亡検知
- 自動 DirectML フォールバック
- UI バナー通知

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

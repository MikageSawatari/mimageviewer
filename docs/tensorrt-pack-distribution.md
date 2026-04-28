# TensorRT 高速化パックの配布手順 (mikage 用 runbook)

mImageViewer の TensorRT 機能を有効化するためのパック (~1.97 GB) を mikage 機で
作って GitHub Releases に上げるまでの手順。`docs/licensing-tensorrt.md` で確定した
ライセンス方針 (NVIDIA SDK SLA + supplements + ONNX Runtime MIT) と DLL 構成
(REQUIRED 13 個 / REMOVABLE 27 個) に従う。

## 前提

- Windows 11 + RTX 30/40/50 系 GPU (mikage 機: RTX 4090 / sm89)
- NVIDIA ドライバ (任意 CUDA 12.x 系) インストール済み
- PowerShell 5.1+ + bash (Git Bash 等)
- ディスク空き 10 GB 以上 (一時キャッシュと展開で食う)

## 1. ベースとなる NVIDIA / Microsoft 配布物の取得

```powershell
# リポジトリルートで実行
.\scripts\setup-tensorrt-pack.ps1
```

NuGet / NVIDIA developer redist URL から ORT 1.24.2 GPU + CUDA 12.9 系 + cuDNN 9.21
+ TensorRT 10.16 を `vendor/tensorrt-cache/` に取得し、必要 DLL のみを
`%APPDATA%/mimageviewer/tensorrt/` に展開する。完了で `INSTALL_OK` (JSON 版情報)
が書かれる。

この時点では builder_resource を含む全 ~6.7 GB が展開されている。

## 2. 全モデルの AMPERE_PLUS engine を事前ビルド

```powershell
# 全 6 モデル分、シリアル実行で ~10-20 分
foreach ($kind in @(
  "upscale_realesrgan_x4plus",
  "upscale_realesrgan_anime6b",
  "upscale_realesr_general_v3",
  "upscale_realcugan_4x",
  "upscale_nmkd_siax_4x",
  "denoise_realplksr"
)) {
  .\target\release\mimageviewer.exe --tensorrt-build $kind
}
```

各 model に対し `%APPDATA%/mimageviewer/tensorrt-engines/<kind>/` に `.engine` +
`.profile` が出力される。`runtime.rs` の `with_engine_hw_compatible(true)` で
sm80+ 共通 (kAMPERE_PLUS) の engine になる。

リリースビルドで `cargo build --release` 済みであること。

## 3. DLL trim 検証 (定期、毎回ではない)

CUDA / cuDNN / TRT のバージョンが上がったら REMOVABLE 一覧を再検証する。前回検証は
Apr 28 の multi-model trim test (`docs/licensing-tensorrt.md §最終 DLL セット`)。
`build_trt_pack.rs::REMOVABLE_DLLS` に変更があったら本書末尾の検証手順を実行する。

## 4. Pack 生成 (build_trt_pack)

```bash
cargo run --release --bin build_trt_pack
```

出力:

```
dist/trt-pack-v1/
  manifest.json                       # SHA-256 一覧 + バージョン情報
  NOTICE-NVIDIA.txt                   # 同梱必須 attribution
  LICENSE-onnxruntime.txt             # ONNX Runtime MIT 全文
  cudart64_12.dll                     # 13 個の REQUIRED DLL (合計 1.86 GB)
  cublasLt64_12.dll
  cudnn_ops64_9.dll
  cufft64_11.dll
  nvJitLink_120_0.dll
  nvrtc64_120_0.dll
  nvrtc-builtins64_129.dll
  nvinfer_10.dll
  nvinfer_plugin_10.dll
  onnxruntime.dll
  onnxruntime_providers_shared.dll
  onnxruntime_providers_cuda.dll
  onnxruntime_providers_tensorrt.dll
  engines-ampere_plus.zip             # 6 モデル分の事前 build engine zip (108 MiB)
```

ユーザー DL 量の合計は約 1.97 GB。

## 5. ローカル HTTP サーバーでの E2E 動作検証

GitHub Releases にアップロードする前に、ローカルで実際の DL → 検証 → 展開 → 起動 の
全フローを通す。

```bash
# (a) HTTP サーバー起動 (dist/trt-pack-v1/ をルートにする)
cd dist/trt-pack-v1
python -m http.server 8000
# → http://127.0.0.1:8000/manifest.json で manifest が見える状態にする
```

別ターミナルで:

```powershell
# (b) %APPDATA%/mimageviewer/tensorrt/ を退避 (再現テストのため)
$pack = "$env:APPDATA\mimageviewer\tensorrt"
if (Test-Path $pack) {
  Move-Item $pack "$env:APPDATA\mimageviewer\tensorrt.bak"
}

# (c) 環境変数を設定して mImageViewer 起動
$env:MIV_TRT_PACK_BASE_URL = "http://127.0.0.1:8000"
.\target\release\mimageviewer.exe
```

mImageViewer 起動後:
1. 環境設定 → AI バックエンド → TensorRT を選択
2. 「TensorRT パックをダウンロード」ボタンを押す
3. 確認 → [開始] → プログレスバー → 完了 を観察 (約 5-10 秒、ローカルなので超高速)
4. アプリ再起動 → AI アップスケールが TensorRT 経由で動くこと確認 (`bench_ai --backend tensorrt`)

検証ポイント:
- [ ] manifest fetch エラーが出ない
- [ ] 全 16 ファイル (notices 2 + DLL 13 + engine zip 1) が DL される
- [ ] SHA-256 検証が全部通る
- [ ] engine zip が `tensorrt-engines/<kind>/<file>` に正しく展開される
- [ ] `INSTALL_OK` が書かれる
- [ ] 再起動後 `tensorrt_pack::is_pack_installed()` が true
- [ ] `bench_ai --backend tensorrt --models realesrgan_anime6b` が成功

検証完了後、退避した古い pack を戻す:
```powershell
Move-Item "$env:APPDATA\mimageviewer\tensorrt.bak" "$env:APPDATA\mimageviewer\tensorrt"
$env:MIV_TRT_PACK_BASE_URL = $null
```

### キャンセル / 再開動作の確認 (任意、推奨)

Running フェーズ中に [キャンセル] を押す → 部分 DL ファイル (`*.partial`) は残る。
再度 「TensorRT パックをダウンロード」 → HTTP Range 付きで途中から再開されるか
確認 (HTTP server 側のログで `bytes=N-` を含むリクエストが見えれば OK)。

### Hash mismatch の処理確認 (任意)

`dist/trt-pack-v1/manifest.json` の中の `sha256` をわざと書き換えて HTTP サーバー
を再起動 → インストール走らせる → Error フェーズで `SHA-256 が一致しません` の
メッセージが出るか確認。

## 6. GitHub Releases へのアップロード

```bash
# (a) タグを切る (git tag 名は manifest_format/PACK_VERSION と整合させる)
TAG="trt-pack-v1"
git tag $TAG
git push origin $TAG

# (b) gh CLI でリリースを作成 + アセットを一括アップロード
gh release create $TAG \
  --title "TensorRT acceleration pack v1 (mImageViewer)" \
  --notes-file docs/tensorrt-pack-release-notes.md \
  --prerelease \
  dist/trt-pack-v1/*
```

`--prerelease` で `latest` リリースとして見えなくする (mIV 本体のリリースタグ
v0.8.x 等が GitHub Releases の主役)。

### 1 ファイル 2 GiB 上限の確認

GitHub Releases は単一ファイルに 2 GiB 上限がある。本 pack の最大単体ファイルは
`cublasLt64_12.dll` (~638 MB) なのでマージンあり。**bump 時に超えそうになったら zip
分割を検討** (現状不要)。

### アップロード後の sanity check

```bash
# manifest が公開 URL から読めるか
curl -fsS https://github.com/MikageSawatari/mimageviewer/releases/download/$TAG/manifest.json | jq .pack_version
# → "1" が出れば OK
```

## 7. 配布完了後の本番環境テスト

mikage 機で `MIV_TRT_PACK_BASE_URL` をクリアして本番 URL から DL して動くか確認。

```powershell
$env:MIV_TRT_PACK_BASE_URL = $null
# (古い pack を退避してからインストールフローを通す)
```

## 付録: pack バージョン bump 時のチェックリスト

`PACK_VERSION` を 2 以上に上げるときは以下を同期させる:

- [ ] `src/ai/tensorrt_pack.rs::EXPECTED_TRT_PACK_VERSION`
- [ ] `src/bin/build_trt_pack.rs::PACK_VERSION`
- [ ] `src/ai/tensorrt_installer.rs::DEFAULT_PACK_BASE_URL` (タグ名 `trt-pack-vN`)
- [ ] `scripts/setup-tensorrt-pack.ps1` の各 `$*_VERSION` (CUDA / cuDNN / TRT / ORT)
- [ ] `docs/licensing-tensorrt.md` の対象バージョン記述
- [ ] 本書 §1〜6 のバージョン番号

## 付録: DLL trim 再検証手順 (CUDA/cuDNN/TRT 更新時)

1. `setup-tensorrt-pack.ps1` 実行 → `%APPDATA%/mimageviewer/tensorrt/` を full に
2. `mimageviewer.exe --tensorrt-build <kind>` で 6 モデル engine 全部 build
3. `bench_ai` を 6 モデル全部で 1 個ずつ DLL を退避しながら回す:
   ```bash
   bash scripts/trim_dlls_multi.sh   # docs/licensing-tensorrt.md §検証手順 と同じ
   ```
4. 全モデルが "wall total" に到達した最小セットを `REMOVABLE_DLLS` に反映
5. mIV 再ビルド → 本書 §4 〜 §6 を実施

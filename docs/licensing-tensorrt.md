# TensorRT 関連ライブラリのライセンス確認

mIV (mImageViewer) の TensorRT ベース AI アップスケール機能で **GitHub Releases から zip
配布** することを想定し、各 DLL の再配布条項を整理したもの。最終的な法務判断は本ドキュメントの
出典 URL と実際の EULA 原文 (リリースバージョン時点のもの) で行うこと。NVIDIA / Microsoft の
ライセンス文書は更新されることがあるため、新バージョンに切り替える際は必ず再確認する。

調査時点: 2026-04 / 対象バージョン: ONNX Runtime 1.24.2 (Microsoft.ML.OnnxRuntime.Gpu.Windows),
CUDA Toolkit 12.9, cuDNN 9.21, TensorRT 10.16

---

## 結論 (Apr 28 確定)

調査結果と Codex/Agent の独立アセスメントを踏まえ、配布物を以下の構成にする:

1. **`nvinfer_builder_resource_*.dll` (全 8 種、合計 ~2.3 GB) は配布しない**
   - TensorRT SLA 上「runtime files .so/.dll」に該当するか曖昧 (= グレーゾーン)
   - NVIDIA 自身が TensorRT 10.12 で別パッケージへ分離する流れ
   - 代わりに **mikage 側で AMPERE_PLUS モードで事前 build した engine ファイル**を
     配布し、ユーザー機での engine compile は廃止
2. **runtime DLL のみを GitHub Releases に置く** (CUDA Runtime / Math libs / cuDNN /
   TensorRT runtime / ONNX Runtime)。各 EULA で明確に再配布許諾されているもののみ
3. **実機 multi-model trim で実際に必要な DLL に絞り込み**: ORT/TRT EP が startup probe で
   読み込む DLL 以外は除外可能。Round 1+2 の単一モデルテストでは過剰削減 (nmkd_siax で
   crash) が発生したため、全 6 モデルを成功させる最小セットを別途決定 (本書末尾 §最終 DLL
   セット参照)

性能面: AMPERE_PLUS モード使用による減速は wall time 平均 +5.4%、最大 +8.8% (RTX 4090
実測 Apr 28)。DirectML 比では依然 1.8-4x 高速。

対応 GPU: RTX 30 / 40 / 50 series (compute capability 8.0 以上)。RTX 20 (Turing, sm75) は
DirectML フォールバックで対応。

---

## 概要表

| DLL | 由来 | ライセンス | 再配布可否 (無料アプリ + zip 経由) | 出典 URL |
|---|---|---|---|---|
| onnxruntime.dll | Microsoft ONNX Runtime | MIT | OK (LICENSE 同梱必須) | [LICENSE](https://github.com/microsoft/onnxruntime/blob/main/LICENSE) |
| onnxruntime_providers_shared.dll | 同上 | MIT | OK | 同上 |
| onnxruntime_providers_cuda.dll | 同上 | MIT | OK | 同上 |
| onnxruntime_providers_tensorrt.dll | 同上 | MIT | OK | 同上 |
| cudart64_12.dll | CUDA Toolkit 12.9 | NVIDIA SDK SLA + CUDA EULA | **OK** (Attachment A 明記) | [CUDA EULA](https://docs.nvidia.com/cuda/eula/index.html) |
| cublas64_12.dll, cublasLt64_12.dll | 同上 | 同上 | **OK** (Attachment A 明記) | 同上 |
| cufft64_11.dll, cufftw64_11.dll | 同上 | 同上 | **OK** (Attachment A 明記) | 同上 |
| curand64_10.dll | 同上 | 同上 | **OK** (Attachment A 明記) | 同上 |
| cusolver64_11.dll | 同上 | 同上 | **OK** (Attachment A 明記) | 同上 |
| cusolverMg64_11.dll | 同上 | 同上 | **要確認** (cusolver の variant 解釈) | 同上 |
| cusparse64_12.dll | 同上 | 同上 | **OK** (Attachment A 明記) | 同上 |
| nvJitLink_120_0.dll | 同上 | 同上 | **OK** (Attachment A 明記、libnvJitLink) | 同上 |
| nvrtc64_120_0.dll, nvrtc-builtins64_129.dll | 同上 | 同上 | **OK** (Attachment A 明記) | 同上 |
| nvrtc64_120_0.alt.dll | 同上 | 同上 | **要確認** (nvrtc 同梱 alt ファイルの扱い) | 同上 |
| cudnn64_9.dll, cudnn_*64_9.dll (全 8 種) | NVIDIA cuDNN 9.21 | NVIDIA SDK SLA + cuDNN Supplement | **OK** ("runtime files .so/.dll" として明記) | [cuDNN SLA](https://docs.nvidia.com/deeplearning/cudnn/sla/index.html) |
| nvinfer_10.dll, nvinfer_lean_10.dll, nvinfer_dispatch_10.dll | NVIDIA TensorRT 10.16 | NVIDIA SDK SLA + TensorRT Supplement | **OK** (runtime DLL) | [TensorRT SLA](https://docs.nvidia.com/deeplearning/tensorrt/sla/index.html) |
| nvinfer_plugin_10.dll, nvinfer_vc_plugin_10.dll | 同上 | 同上 | **OK** (runtime DLL) | 同上 |
| nvinfer_builder_resource_*_10.dll (sm75/80/86/89/90/100/120, ptx) | 同上 | 同上 | **要確認** (runtime files として明記なし。エンジンビルド用リソース) | 同上 |
| nvonnxparser_10.dll | 同上 | 同上 | **要確認** (runtime files の解釈による) | 同上 |

> **「OK」のついた DLL も、後述の "Distribution Requirements" 4 条件 (機能上の追加価値・
> 自アプリからのみアクセス・配布条件の整合性・利用者保護) を満たす必要がある。**

---

## ONNX Runtime (Microsoft)

対象 DLL: `onnxruntime.dll`, `onnxruntime_providers_shared.dll`,
`onnxruntime_providers_cuda.dll`, `onnxruntime_providers_tensorrt.dll`
(NuGet `Microsoft.ML.OnnxRuntime.Gpu.Windows` 1.24.2 から抽出)

### 再配布の根拠

ONNX Runtime は **MIT License**。商用 / 非商用を問わず、バイナリ含めた配布が許諾されている。
「use, copy, modify, merge, publish, distribute, sublicense, and/or sell」を許可する典型的な
MIT 条項。ライセンス上は zip 配布・GitHub Releases 配布に支障なし。

### Attribution 要件

MIT は「上記 copyright notice と permission notice を全コピーまたは substantial portion に
含める」ことを唯一の条件としている。**NOTICE ファイル相当の何らかの形で LICENSE 全文を
同梱する必要がある**。zip にバンドルする場合は `LICENSE-onnxruntime.txt` のような名前で
ONNX Runtime の MIT 全文 (Microsoft の copyright 行付き) を入れる。

### 関連 URL

- ライセンス本体: https://github.com/microsoft/onnxruntime/blob/main/LICENSE
- ONNX Runtime のサードパーティ NOTICES: https://github.com/microsoft/onnxruntime/blob/main/ThirdPartyNotices.txt
  (bundled な依存ライブラリの notice も含めるべきかは要検討。NuGet package 内に同梱されている
  `ThirdPartyNotices.txt` をそのまま zip に入れるのが安全)

---

## NVIDIA CUDA Runtime / Math Libraries

対象 DLL: `cudart64_12.dll`, `cublas64_12.dll`, `cublasLt64_12.dll`, `cufft64_11.dll`,
`cufftw64_11.dll`, `curand64_10.dll`, `cusolver64_11.dll`, `cusolverMg64_11.dll`,
`cusparse64_12.dll`, `nvJitLink_120_0.dll`, `nvrtc64_120_0.dll`,
`nvrtc64_120_0.alt.dll`, `nvrtc-builtins64_129.dll`

### 再配布の根拠

CUDA Toolkit EULA の **Section 2.6 "Attachment A — Redistributable Software"**
(Section 番号は版によって変わる可能性あり) に再配布可能なファイルが列挙されている。
バージョン番号やアーキテクチャ番号がファイル名に含まれた variant も対象に含むと明記:

> "...including certain variations of these files that have version number or architecture
> specific information embedded in the file name."

これにより `cudart.dll` → `cudart64_12.dll`, `cublas.dll` → `cublas64_12.dll` 等は
明示的に再配布対象。商用 / 非商用の区別は条項上ない (= 無料アプリでも有償アプリでも同一条件)。

ただし **Section 1.1.2 "Distribution Requirements"** (4 つの条件) 全部を満たす必要がある:

1. アプリは SDK 部分を超える "material additional functionality" を持つこと (mIV は画像
   ビューワーとして AI アップスケール以外の本体機能を持つので OK)
2. 配布物の SDK 部分にアクセスするのは自アプリのみであること (mIV プロセスからしか
   呼ばれない構造で OK)
3. 配布条件が本 Agreement と矛盾しないこと (再配布禁止 / リバースエンジニアリング禁止
   等を mIV の利用規約に反映するか、最低限 NOTICE で利用者に伝える)
4. 開発者ツールとして識別されているものは "internal use only" (今回対象の DLL は
   開発者ツール扱いではないので問題なし)

### 不確実な点

- **`cusolverMg64_11.dll`**: cuSOLVER のマルチ GPU エクステンション。Attachment A は
  "CUDA Linear Solver Library: cusolver.dll" と書かれているが、`cusolverMg` を variant とみなせるか
  明確でない。**TensorRT 経路の推論で実際に必要かを先に確認**し、不要なら zip から落とす
  方が安全。
- **`nvrtc64_120_0.alt.dll`**: nvrtc の alternative 実装 (CUDA 12 で導入されたフォールバック
  build。AVX 命令を持たない CPU 用のセカンダリ DLL)。Attachment A の "nvrtc.dll" の variant に
  含まれると解釈するのが自然だが、ファイル名に `.alt` が入っているケースについて NVIDIA の
  公式言及は見つからなかった。実用上 nvrtc 本体と一緒に配布する前提で出荷されているので
  variant とみなして同梱しているが、最終確定前に NVIDIA にメールで確認すると確実
  (nvidia-compute-license-questions@nvidia.com)。

### Attribution 要件

CUDA EULA Section 1.1.2 が明示的に求めているのは「sample source code の修正・派生物に対する
NOTICE 記述」のみで、**バイナリ DLL 再配布に対しては attribution 文言の埋め込みを明示要求
していない**。ただし業界慣行として NOTICE-NVIDIA.txt を同梱するのが一般的。Section 1.1.2 の
"protection of NVIDIA's intellectual property rights" を満たすためにも、最低限「これらの
DLL の copyright が NVIDIA に属する」旨の表記は入れる。

### EULA 同梱・利用者同意

- **end-user による click-through 同意は EULA 上明示要求されていない**。redistributor (= mIV
  開発者) が EULA に同意していれば、利用者に CUDA EULA の click-through を見せる義務はない。
- ただし「terms under which you distribute your application must be consistent with the terms
  of this Agreement」という条項があるため、mIV のソフトウェア使用許諾 (利用規約) 内で
  「同梱の NVIDIA コンポーネントは NVIDIA の権利物であり、リバースエンジニアリング・別アプリへの
  抽出再配布等は禁止」程度の記述を入れておくのが推奨。

### 関連 URL

- CUDA Toolkit EULA: https://docs.nvidia.com/cuda/eula/index.html
- Attachment A (Redistributable Software): 同 EULA 末尾

---

## NVIDIA cuDNN

対象 DLL: `cudnn64_9.dll`, `cudnn_adv64_9.dll`, `cudnn_cnn64_9.dll`,
`cudnn_engines_precompiled64_9.dll`, `cudnn_engines_runtime_compiled64_9.dll`,
`cudnn_engines_tensor_ir64_9.dll`, `cudnn_graph64_9.dll`, `cudnn_heuristic64_9.dll`,
`cudnn_ops64_9.dll`

### 再配布の根拠

cuDNN は単独の SLA を持つが、**「License Agreement for NVIDIA Software Development Kits」
の Supplement (= 補遺)** として位置づけられている。SLA 内に明記:

> "This supplement is an exhibit to the Agreement and is incorporated as an integral part
> of the Agreement."

つまり cuDNN を使う際は **本体 SDK SLA + cuDNN Supplement の両方** に拘束される。

cuDNN Supplement Section 2 "Distribution" (Section 番号は版によって変わる) に明記:

> "The following portions of the SDK are distributable under the Agreement: the runtime
> files .so and .dll."

`cudnn64_9.dll` 系は全て runtime DLL であり、**全 8 ファイル** がここでカバーされる
(`cudnn_engines_*` も含めて、cuDNN 9.x 系では全部が `.dll` 形式の runtime コンポーネント)。
本体 SDK SLA の Distribution Requirements (前述 4 条件) も同じく適用される。

### Attribution 要件

CUDA EULA と同様、バイナリ再配布時の attribution 文言は明示要求されていない。NOTICE への
NVIDIA 著作権表記が業界慣行。

### 関連 URL

- cuDNN SLA: https://docs.nvidia.com/deeplearning/cudnn/sla/index.html
- 質問先: nvidia-compute-license-questions@nvidia.com

### 補足

NVIDIA Developer フォーラムの公開議論でも、開発者が「アプリインストーラに CUDA / cuDNN
DLL を同梱、Attachment A の DLL のみを使用」というアプローチを取り、特に問題視されていない事例が
ある (https://forums.developer.nvidia.com/t/redistribution-of-cuda-and-cudnn/190143)。

---

## NVIDIA TensorRT

対象 DLL: `nvinfer_10.dll`, `nvinfer_lean_10.dll`, `nvinfer_dispatch_10.dll`,
`nvinfer_plugin_10.dll`, `nvinfer_vc_plugin_10.dll`,
`nvinfer_builder_resource_ptx_10.dll`, `nvinfer_builder_resource_sm{75,80,86,89,90,100,120}_10.dll`,
`nvonnxparser_10.dll`

### 再配布の根拠

TensorRT も cuDNN と同様、本体 SDK SLA の Supplement という構造。Section 8 に
"TENSORRT SUPPLEMENT TO SOFTWARE LICENSE AGREEMENT FOR NVIDIA SOFTWARE DEVELOPMENT KITS"。
Section 8.2 Distribution に明記:

> "The following portions of the SDK are distributable under the Agreement: the runtime
> files .so and .dll."

Section 1.2 の Distribution Requirements (前述 4 条件) も同様に適用。

### 各 DLL の解釈

- **`nvinfer_10.dll` / `nvinfer_plugin_10.dll`**: 推論に必須の runtime DLL。明確に OK。
- **`nvinfer_lean_10.dll` / `nvinfer_dispatch_10.dll`**: TensorRT 8.6+ で導入された
  バージョン互換 runtime (lean = 削減版、dispatch = ロードを動的に切り替えるディスパッチャ)。
  「runtime files .so and .dll」の typical な解釈に含まれる。OK。
- **`nvinfer_vc_plugin_10.dll`**: Version-Compatible plugin。runtime 系。OK。
- **`nvonnxparser_10.dll`**: ONNX → TensorRT エンジンに変換するパーサ。エンジンを
  利用者環境でビルドする (= 起動時に ONNX を読み込んで build する) なら必要。
  「runtime files」の解釈に含まれるかは EULA 文言だけからは断定できない。**要確認**。
- **`nvinfer_builder_resource_*_10.dll`**: エンジン**ビルド** (TRT 内部の最適化・カーネル
  選択) に必要なリソース。各 DLL が 1〜2GB と巨大。NVIDIA フォーラム
  (https://forums.developer.nvidia.com/t/libnvinfer-builder-resource-libs/327373) で
  staff 回答あり: これらは **エンジン構築時** に使われるリソース。事前ビルド済み engine
  ファイルを配布して runtime はデシリアライズだけする運用なら不要。
  TensorRT 10.12 以降は別パッケージに分離する流れ (NVIDIA 自身がランタイム配布から
  切り離す方向)。
  **runtime files .so/.dll に厳密に含まれるかは EULA 上不明確**。実用上は推論しか
  しないなら同梱しないのが安全。

### 推奨方針

mIV では **エンジンを利用者環境でビルドする運用 (RTX 50 系で OEM 提供エンジンが
存在しない / モデル更新を頻繁に行う等) かどうかで分岐**:

- A. **事前ビルド engine 同梱方針**: ターゲット compute capability 別に NVIDIA 側で
  ビルドした `.engine` ファイルを GitHub Releases に同梱し、runtime DLL のみ配布。
  → `nvinfer_*` (lean/dispatch/plugin) と `nvonnxparser` (使うなら) のみで足り、
  `nvinfer_builder_resource_*` は不要。zip サイズも 100MB 程度に収まる。
- B. **利用者環境で ONNX → engine ビルド方針**: builder resource を含む全 DLL が必要。
  zip サイズが GPU 世代別に数 GB ずつ増える。法的にもグレー。

mIV は GitHub Releases の容量制約 (1 ファイル 2GB) もあるので **方針 A 推奨**。
方針 B を採る場合は、最低限 `nvinfer_builder_resource_sm89_10.dll` (RTX 40)、
`sm120_10.dll` (RTX 50) のみに絞り、それ以外は除外する。

### Attribution 要件

CUDA EULA / cuDNN SLA と同様、バイナリ再配布の文言要求は明示なし。
NOTICE の NVIDIA 著作権表記が慣行。

### 関連 URL

- TensorRT SLA: https://docs.nvidia.com/deeplearning/tensorrt/sla/index.html
- builder resource 用途解説: https://forums.developer.nvidia.com/t/libnvinfer-builder-resource-libs/327373

---

## SUPPLEMENT 条項の優先関係

| ライブラリ | 法的構造 |
|---|---|
| CUDA Toolkit | "License Agreement for NVIDIA SDKs" + CUDA-specific clauses (= CUDA EULA は SDK SLA に上乗せ、Attachment A はその一部) |
| cuDNN | SDK SLA + cuDNN Supplement (Supplement が SDK SLA に追加要件・許諾を上書き) |
| TensorRT | SDK SLA + TensorRT Supplement (同上) |

3 つとも **共通 base = NVIDIA SDK SLA**。優先関係は「Supplement 内の specific 条項が SDK SLA の
general 条項に勝る」という典型的な exhibit / supplement 構造。**両方の条件を同時に満たす必要
がある** (どちらか片方だけ守ればよいという意味ではない)。

実務上は SDK SLA 側の Distribution Requirements (Section 1.1.2 / 1.2) と、各 Supplement の
Section 2 (Distribution) の **両方** をチェックリストにする:

1. 自アプリに material additional functionality があるか? → ある (画像ビューワー)
2. 配布 DLL が自アプリ専用にアクセス制御されているか? → 通常の DLL ロード経路で OK
3. mIV の利用規約 / EULA が NVIDIA 側 EULA と矛盾していないか? → 要確認 (mIV の利用規約に
   サードパーティ条項を入れる)
4. 各 Supplement が許諾しているのは "runtime files .so/.dll" のみ → builder resource 系は
   除外する (前述 A 方針)

---

## NOTICE-NVIDIA.txt 推奨文面

zip ルートに以下のような `NOTICE-NVIDIA.txt` を同梱する。

```
This product includes software components from NVIDIA Corporation, redistributed under
the NVIDIA Software License Agreement for NVIDIA Software Development Kits and its
supplements. Use of these components is subject to those agreements.

Components included:
  CUDA Runtime / Math Libraries (CUDA Toolkit 12.9)
    cudart64_12.dll, cublas64_12.dll, cublasLt64_12.dll,
    cufft64_11.dll, cufftw64_11.dll, curand64_10.dll,
    cusolver64_11.dll, cusolverMg64_11.dll, cusparse64_12.dll,
    nvJitLink_120_0.dll, nvrtc64_120_0.dll, nvrtc64_120_0.alt.dll,
    nvrtc-builtins64_129.dll
  cuDNN (NVIDIA cuDNN 9.21)
    cudnn64_9.dll, cudnn_adv64_9.dll, cudnn_cnn64_9.dll,
    cudnn_engines_precompiled64_9.dll, cudnn_engines_runtime_compiled64_9.dll,
    cudnn_engines_tensor_ir64_9.dll, cudnn_graph64_9.dll,
    cudnn_heuristic64_9.dll, cudnn_ops64_9.dll
  TensorRT (NVIDIA TensorRT 10.16)
    nvinfer_10.dll, nvinfer_lean_10.dll, nvinfer_dispatch_10.dll,
    nvinfer_plugin_10.dll, nvinfer_vc_plugin_10.dll,
    nvonnxparser_10.dll
    [if shipped] nvinfer_builder_resource_*_10.dll

Copyright (c) NVIDIA Corporation. All rights reserved.

Source license texts:
  CUDA Toolkit EULA:   https://docs.nvidia.com/cuda/eula/index.html
  cuDNN SLA:           https://docs.nvidia.com/deeplearning/cudnn/sla/index.html
  TensorRT SLA:        https://docs.nvidia.com/deeplearning/tensorrt/sla/index.html

These components are redistributed for use with mImageViewer only. Reverse engineering,
extraction, separate redistribution outside of mImageViewer, and use in violation of the
NVIDIA license agreements above are prohibited.
```

加えて、ONNX Runtime の MIT ライセンス本文を `LICENSE-onnxruntime.txt` として同梱:

```
MIT License

Copyright (c) Microsoft Corporation

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), ...
[全文をそのまま貼る]
```

(全文は https://github.com/microsoft/onnxruntime/blob/main/LICENSE から取得)

---

## 残課題 / 不確実な事項

以下は本調査では断定できなかった項目。**リリース前に NVIDIA
(nvidia-compute-license-questions@nvidia.com) に問い合わせるか、社内 / 外部の法務確認を
得ることを推奨**。

1. **`cusolverMg64_11.dll` の variant 解釈**: Attachment A は "cusolver.dll" を列挙するが
   "cusolverMg" は別ライブラリなのか variant なのか不明確。実用上 TensorRT 経路では不要な
   ことが多いので、**先に実機テストで読み込まれるかを確認**してから判断するのが効率的。
2. **`nvrtc64_120_0.alt.dll` の variant 解釈**: NVIDIA 公式が "alt" 命名規則の variant に
   ついて言及した文書が見つからなかった。CUDA インストーラに同梱されているので variant と
   みなして問題ないと思われるが、明示的な根拠は薄い。
3. **`nvonnxparser_10.dll` の "runtime files" 該当性**: ONNX → engine 変換は通常 build 段階
   だが、TensorRT は inference 時に runtime build を許す API もあるため runtime DLL と
   解釈する余地あり。利用者環境で ONNX を直接読み込む実装ならばグレー。事前 build した
   engine を読み込む実装なら同梱不要。
4. **`nvinfer_builder_resource_*_10.dll` の再配布可否**: SLA 文面では "runtime files .so/.dll"
   とのみあり、builder リソースが含まれるか曖昧。NVIDIA 自身が TensorRT 10.12 で別パッケージに
   分離している流れから、**runtime ではない扱いの可能性が高い**。前述の方針 A (事前 build
   engine 同梱) で回避するのが安全。
5. **Microsoft.ML.OnnxRuntime.Gpu.Windows NuGet 内のサードパーティ通知**: NuGet パッケージに
   `ThirdPartyNotices.txt` が同梱されている場合があり、その内容を mIV 配布物にも含める必要が
   あるか要確認。基本的には ONNX Runtime 自身の MIT 通知 + NVIDIA 通知で足りるはずだが、
   NuGet パッケージに記載されているものをそのまま尊重するのが堅実。
6. **エンドユーザーへの click-through 不要の根拠**: EULA 文面上は "consistent with the terms
   of this Agreement" であれば良いと読めるが、より厳密な運用 (mIV のインストーラに「同梱の
   サードパーティライブラリの利用規約に同意します」というチェックボックスを入れる) を取る
   案もある。Inno Setup のライセンス画面に NOTICE-NVIDIA.txt を表示するのが最も無難。
7. **GitHub Releases ダウンロードゲートの是非**: NVIDIA Developer 登録なしで取得できる zip を
   GitHub に公開することそのものは EULA 違反とは読めない (再配布許諾を得た開発者が自身の
   配布チャネルで再配布する標準的な形態)。ただし **「NVIDIA から取得したかのように見える」配布
   形態は禁止** (Section 2 で trademark / endorsement の偽装が禁止されているため、リリース
   ノート・ファイル名で「NVIDIA Official Distribution」のような表記は避ける)。

### 次のアクション

- [x] **実機 multi-model trim test 完了 (Apr 28)**: 全 6 モデルで TRT 推論を回し、
      ロードされない DLL を実機検証。詳細は §最終 DLL セット参照
- [x] **事前 build engine を mIV 側 (RTX 4090) で AMPERE_PLUS モードで生成**: sm80+ 全 GPU で
      動作。RTX 20 (sm75) は将来 RunPod T4 で別途 build 想定 (現状は DirectML フォールバック)
- [ ] mIV 利用規約に「同梱のサードパーティコンポーネントの権利は各社に帰属し、抽出・別再配布・
      リバースエンジニアリング等は禁止」の条項を追記
- [x] **`NOTICE-NVIDIA.txt` と `LICENSE-onnxruntime.txt` の同梱を `build_trt_pack.rs` に追加
      (Apr 28)**: const 文字列を埋め込み → `dist/trt-pack-v<N>/` に LF 固定で書き出し →
      manifest の `notices: Vec<AssetEntry>` に SHA-256 付きで登録。downloader (Step 5) は
      common DLL と同じ経路で `tensorrt/<name>` に配置・検証する想定。manifest_format=3 に bump。
      ThirdPartyNotices.txt は ORT GPU NuGet に同梱されておらず、内容も MIT ライセンス本文と
      重複するため別途同梱しない方針 (= MIT 本文だけで attribution 要件を満たす)
- [x] **不確実点 1〜2 (cusolverMg / nvrtc.alt の variant 解釈) は実機で REMOVABLE と確定**
      → 配布物から除外済みのため懸念解消。残り 3 (nvonnxparser) も engine 既ビルド済みで
      不要 → 除外済み

---

## 最終 DLL セット (Apr 29 v2 trim test 結果)

mikage 機 (RTX 4090, Windows 11) で全 6 モデル (Real-ESRGAN x4plus / anime6b /
general_v3 / RealCUGAN-4x / NMKD-Siax-4x / RealPLKSR) の TRT 推論を `bench_ai` で
1 個ずつ DLL を抜きながら回した結果、以下が確定。判定基準は **session_run min <
200 ms** で TRT 経路 (CUDA EP は 200-500ms、CPU EP は 1500ms+ なので明確に区別可能)。

### v1 (Apr 28) からの変更

v1 では `bench_ai --runs 1` の `wall total` 出力だけで判定していたため、ORT が
silent に CPU EP fallback しても "成功" と判定する穴があった。実機 distribute 後
にユーザー機で worker crash (STATUS_STACK_BUFFER_OVERRUN) が判明し、原因は以下
4 個の DLL の hard import 不足と特定:

- `cublas64_12.dll` (provider DLL の import)
- `cudnn64_9.dll` (provider DLL の import)
- `cudnn_graph64_9.dll` (cuDNN 内部 deserialize で必須)
- `nvonnxparser_10.dll` (provider DLL の import)

これら 4 個を REQUIRED に戻して v2 とした (= REQUIRED 13 → 17 個)。

### REQUIRED (= 配布する DLL、17 個 ≈ 2.05 GB)

| DLL | サイズ | 役割 |
|---|---:|---|
| `nvinfer_10.dll` | 395 MB | TensorRT runtime コア |
| `nvinfer_plugin_10.dll` | 46 MB | TensorRT 標準プラグイン |
| `nvonnxparser_10.dll` | 3.3 MB | ONNX → TRT パーサ (provider DLL の hard import) |
| `cublas64_12.dll` | 102 MB | cuBLAS (provider DLL の hard import) |
| `cublasLt64_12.dll` | 638 MB | cuBLAS Lite |
| `cufft64_11.dll` | 274 MB | cuFFT |
| `cudnn64_9.dll` | 0.3 MB | cuDNN umbrella (provider DLL の hard import) |
| `cudnn_ops64_9.dll` | 101 MB | cuDNN ops |
| `cudnn_graph64_9.dll` | 100 MB | cuDNN graph (engine deserialize で必須) |
| `cudart64_12.dll` | 0.6 MB | CUDA Runtime |
| `nvJitLink_120_0.dll` | 83 MB | JIT リンカ (PTX → kernel) |
| `nvrtc64_120_0.dll` | 86 MB | NVRTC (ランタイムコンパイラ) |
| `nvrtc-builtins64_129.dll` | 7 MB | NVRTC built-in 関数 |
| `onnxruntime.dll` | 14 MB | ONNX Runtime コア |
| `onnxruntime_providers_shared.dll` | 22 KB | ORT provider 共通 |
| `onnxruntime_providers_cuda.dll` | 263 MB | ORT CUDA EP |
| `onnxruntime_providers_tensorrt.dll` | 0.8 MB | ORT TensorRT EP |

### REMOVABLE (= 配布しない DLL、23 個 ≈ 4.6 GB)

```
builder_resource (8): nvinfer_builder_resource_{ptx,sm75,sm80,sm86,sm89,sm90,sm100,sm120}_10.dll
                     ← ライセンス上の判断 (再配布許諾不明確)、事前 build engine で代替

数学ライブラリ (5):  cufftw64_11, curand64_10, cusolver64_11, cusolverMg64_11, cusparse64_12
                     ← AMPERE_PLUS 経路の TRT/CUDA EP は cuFFT のみ probe、他は不要

cuDNN 補助 (6):       cudnn_adv64_9, cudnn_cnn64_9, cudnn_engines_*64_9 (×3),
                     cudnn_heuristic64_9
                     ← AMPERE_PLUS は cuDNN tactic 無効、ops + graph + umbrella のみ必須

TensorRT 補助 (3):   nvinfer_lean_10, nvinfer_dispatch_10, nvinfer_vc_plugin_10
                     ← バージョン互換 lib、フル nvinfer_10 を持っていれば不要

NVRTC alt (1):       nvrtc64_120_0.alt.dll
                     ← Hopper 系の代替経路、画像ビューワーでは不使用
```

### 検証手順 (mikage 機での再 trim 用)

1. mikage 機で `setup-tensorrt-pack.ps1` 実行 → `%APPDATA%/mimageviewer/tensorrt/` に
   フル展開 (~6.7 GB)
2. `mimageviewer.exe --tensorrt-build <kind>` で 6 モデル分の AMPERE_PLUS engine を build
3. `bash scripts/trim_dlls_v2.sh` を実行 (= v2 系、session_run min < 200ms 判定)。
   詳細は `docs/tensorrt-pack-distribution.md §付録 DLL trim 再検証手順`
4. `/tmp/trim_dlls_v2/result.txt` の REMOVABLE 一覧を `REMOVABLE_DLLS` に反映
5. `dumpbin /dependents` で provider DLL (`onnxruntime_providers_{cuda,tensorrt}.dll`)
   の hard import を確認 → `PROVIDER_DLL_IMPORTS` に列挙する
   (= build_trt_pack 実行時の静的チェックで再発防止)

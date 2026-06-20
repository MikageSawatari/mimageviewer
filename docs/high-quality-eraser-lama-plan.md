# 高品質消しゴム (Big-LaMa) 仕様案

Status: **保留 (v2.0.0 では見送り)** / 設計メモは残置
Date: 2026-06-20 (ライセンス調査による見送り判断: 2026-06-20)

この文書は、現行の MI-GAN 消しゴムに加えて、AI 生成イラストや写真の不要物消去に向いた
高品質 inpaint モデルを導入するための仕様メモ。追加ダウンロードは
[editing-add-on-download-spec.md](editing-add-on-download-spec.md) の編集用追加パックに統合し、
ユーザーに何度も別パックの導入を求めないことを前提にする。

## 0. ステータス: v2.0.0 では見送り (ライセンス理由)

2026-06-20 のライセンス調査の結果、**本機能 (Big-LaMa 高品質消しゴム) は v2.0.0 には入れず
見送る**。以下の仕様 (§1 以降) は将来クリーンな重みが得られた場合の設計として残置する。

### 見送りの主因: 重みのライセンスがクリーンでない

| 対象 | ライセンス | 配布可否 |
| --- | --- | --- |
| LaMa **コード** ([advimman/lama](https://github.com/advimman/lama/blob/main/LICENSE)) | Apache-2.0 (Samsung Research 2021) | OK |
| big-lama **重み (checkpoint)** | 明示ライセンスなし。学習データ Places2 に依存 | グレー (不可寄り) |
| 学習データ **Places2** ([places2.csail.mit.edu](http://places2.csail.mit.edu/download.html)) | "non-commercial research and educational purposes" かつ "you will NOT distribute the images" | 不可 |

- リポジトリの LICENSE は **コード**の Apache-2.0 であり、README に「重みは別/同ライセンス」と
  いう明示の carve-out が無い (= 重みの扱いが曖昧)。
- big-lama は **Places2 で学習**されており、Places2 規約は非商用の研究・教育目的限定 + 画像
  再配布禁止。「学習済み重みが学習画像の二次的著作物か」は法的に未決着だが、mIV は
  **Vector / 窓の杜で配布する一般向けビューワ** (研究・教育ではない) なので、出自の非商用性は
  クリーンにできないと判断した。業界では LaMa の商用利用例 (IOPaint 等) や OpenCV の
  [inpainting_lama](https://huggingface.co/opencv/inpainting_lama) 再配布もあるが、Places2 由来の
  出自リスクは消えない。mIV の既存ライセンス運用 (FFmpeg LGPL / VST3 MIT を厳密整理) の
  基準では「明確に OK」ではなく「グレー = 採用しない」に分類する。

### 構造的問題: 代替モデルでも回避困難

高品質 inpainting の SOTA (LaMa / AOT-GAN / MAT 等) は **ほぼ全て Places2 学習** (ベンチマーク
標準のため)。モデルを差し替えても同じ非商用データの壁に当たるため、単純な置換では解決しない。

### 副次的要因: DirectML 実行性能 (未検証のまま保留)

LaMa は FFC (Fast Fourier Convolution, DFT 系 op) を使うため、DirectML EP で動かない可能性が
あり、その場合 CPU フォールバックになる。Big-LaMa の CPU 推論は 1 操作あたり数秒〜十数秒と
実用性が低い。ただしライセンス (上記) で先に止まっているため、**DirectML 可否の probe は
未実施**。再開時に確認する。

### 影響範囲: 被写体分離 (BiRefNet) は無関係・続行可

編集用追加パックの **BiRefNet (被写体マット) は MIT (ZhengPeng7/BiRefNet 由来) でクリーン**。
本見送りは消しゴム高品質モデル (Big-LaMa) のみが対象で、フォント / 被写体分離は通常どおり進める。
追加パック manifest への `inpaint_model` (§5) 追加も保留する。

### 再開条件

**学習データの出自までクリーンな高品質 inpaint モデル** (重み + 学習データが商用配布可能な
ライセンス) が入手できたとき。候補を見つけたら本節を更新して再評価する。

---


## 1. 結論

- 消しゴムツールに **補完モデル** 選択を追加する。
- モデルは 2 種類だけにする。
  - **標準**: 現行 MI-GAN。高速。スキャンゴミ、小さい傷、細い線の補修向け。
  - **高品質**: Big-LaMa。編集用追加パックが必要。AI 生成イラストや写真の不要物、
    背景の描き込みすぎ、大きめのマスク領域向け。
- 編集用追加パックは **1 パターン**だけにする。フォント / BiRefNet / Big-LaMa を分割選択させない。
- 高品質を選んだ時点で編集用追加パックが未導入なら、共通の追加ダウンロードダイアログを出す。
- ダウンロードをキャンセルした場合は補完モデルを **標準へ戻す**。
- 保存済みマスク・マスクスロットにはモデル種別を保存しない。マスクは範囲だけを表し、
  どの補完モデルで結果を生成するかは現在の補完モデル設定で決める。

## 2. 背景

現行の消しゴムは MI-GAN を `512x512` タイルで実行し、マスク領域だけを出力で置換している。
スキャンの小ゴミや細い傷には軽くて扱いやすいが、少し大きなオブジェクトや背景構造を消すと、
タイルごとの文脈不足や継ぎ目、長い線・模様の不連続が出やすい。

Big-LaMa は LaMa 系の大型モデルで、Stable Diffusion / PowerPaint 系のような 10GB 級パックを
必要とせず、大きめの欠損や背景補完に強い候補。Hugging Face の Big-LaMa 配布は約 381MB
規模で Apache-2.0。既存の編集用追加パック (BiRefNet 約 490MB + フォント約 61MiB) と
まとめても、おおむね 1GB 級に収まる見込み。

## 3. 非目標

- Stable Diffusion / SDXL / PowerPaint などの拡散モデルは初期対象にしない。
  サイズ、依存、VRAM、プロンプト UI が大きくなりすぎるため。
- 軽量パック / 高品質パック / フォントのみパックなどの複数パターンは作らない。
  テスト分岐とユーザーへの案内を増やさない。
- 消しゴムマスクごとにモデル種別を保存しない。
  同じマスクを標準と高品質で試せる状態を維持する。
- プロンプト入力、seed、生成強度のような生成 AI 系 UI は入れない。
  Big-LaMa は周辺文脈から埋める自動補完モデルとして扱う。

## 4. ユーザー仕様

### 4.1 消しゴムパネル

消しゴムパネルに「補完モデル」を追加する。候補は 2 つだけ。

| 表示 | 内部値 | 説明 |
| --- | --- | --- |
| 標準 | `standard_migan` | 現行 MI-GAN。高速。小さい汚れ・線・傷向け |
| 高品質 | `high_quality_big_lama` | Big-LaMa。編集用追加パックが必要。大きめの不要物・背景補完向け |

初期値は `標準`。最後に選んだ値は `Settings` に保存し、次回起動時にも維持する。
ただし編集用追加パックが未導入または破損している場合は、起動時またはパック状態の再読込時に
有効値を `標準` へ正規化する。

UI 文言は機能説明に寄せ、モデル名を前面に出しすぎない。

```text
補完モデル
  標準
  高品質
```

ツールチップ:

- 標準: `高速な標準補完。スキャンゴミや小さな傷向けです。`
- 高品質: `大きめの不要物や背景補完向けです。編集用追加ファイルが必要です。`

### 4.2 未導入時の動き

ユーザーが `高品質` を選んだ時点で編集用追加パックが未導入なら、
既存の編集用追加パック導入ダイアログを表示する。

想定文言:

```text
高品質消しゴムには編集用追加ファイルが必要です

高品質消しゴム、被写体分離、編集用フォントをまとめて追加します。
ダウンロードサイズ: 約 1 GB

[ダウンロード] [今はしない]
```

挙動:

- ダウンロード成功: `高品質` を選択状態にして、そのままプレビュー / 適用できる。
- キャンセル: `標準` に戻す。同一セッションでは同じ入口から何度も確認を出さない。
- 失敗: `標準` に戻し、失敗ダイアログまたはトーストから再試行できるようにする。
- 既にダウンロード中: 同じ進捗ダイアログへ合流する。二重ダウンロードはしない。

### 4.3 プレビュー / 適用 / スロット適用

消しゴムの実行経路は、すべて現在の補完モデル設定を使う。

- 消しゴムモード内のプレビュー
- `E` / `Esc` / `×` による確定
- フルスクリーン表示中の F7/F8 スロット適用
- 保存済みマスクの自動適用

ただし高品質モデルが使えない状態では、設定を `標準` へ戻してから実行する。
実行時に黙って別モデルへフォールバックするのではなく、設定側を正規化して UI と結果を一致させる。

## 5. 追加パック仕様

編集用追加パックは 1 パターンのみ。

| 種別 | 内容 | ライセンス | サイズ目安 |
| --- | --- | --- | --- |
| オノマトペ向けフォント | OFL 日本語フォント群 | OFL-1.1 | 約 61 MiB |
| 被写体分離モデル | BiRefNet fp16 ONNX | MIT | 約 490 MiB |
| 高品質消しゴムモデル | Big-LaMa ONNX | Apache-2.0 | 約 381 MiB 前後 |

pack manifest には `inpaint_model` を追加する。

```json
{
  "path": "models/big_lama.onnx",
  "kind": "inpaint_model",
  "model_id": "big_lama",
  "license": "Apache-2.0",
  "sha256": "..."
}
```

アプリ側は `editing_addon` から Big-LaMa のパスを取得する。現行の
`ModelManager` の埋め込みモデル一覧には入れない。

ライセンス表示では、編集用追加パックの欄に Big-LaMa を追加する。

## 6. 内部モデル

### 6.1 設定

新しい設定値:

```rust
enum EraseInpaintModel {
    StandardMiGan,
    HighQualityBigLaMa,
}
```

保存文字列:

- `standard_migan`
- `high_quality_big_lama`

`Settings` に `erase_inpaint_model` を追加する。既定は `StandardMiGan`。
未知値や利用不能な `HighQualityBigLaMa` は `StandardMiGan` へ正規化する。

### 6.2 AI モデル識別

`ModelKind` には `InpaintBigLaMa` を追加してよいが、モデルパス解決は
`ModelManager::model_path` ではなく `editing_addon` 由来にする。

候補:

```rust
ModelKind::InpaintMiGan
ModelKind::InpaintBigLaMa
```

`AiRuntime` のセッションキャッシュには同じように載せる。モデルロードは遅延し、
アプリ起動時には ONNX session を作らない。

### 6.3 キャッシュキー

補完モデルが変わると同じ入力画像・同じマスクでも結果が変わるため、
消しゴム結果キャッシュはモデル種別を区別しなければならない。

`EraseResultKey` に補完モデルを追加する。

```rust
struct EraseResultKey {
    idx: usize,
    input_gen: u64,
    mask_gen: u64,
    model: EraseInpaintModel,
}
```

pending job も投入時の補完モデルを保持し、完了時に現在の補完モデルと一致しない結果は捨てる。
補完モデルを切り替えた時点で、現在ページの preview cache は破棄し、同じ idx の preview / commit
pending を cancel する。

## 7. Big-LaMa 推論方針

### 7.1 実装前ゲート

最初に Big-LaMa の ONNX 変換と DirectML 実行可否を確認する。

- ONNX Runtime + DirectML でロードできること。
- 必要な演算子が DirectML で実行できること。
- 画像 + マスクの I/O 形状、正規化、マスク極性を probe で固定すること。
- 代表画像で MI-GAN より明確に良いケースがあること。

このゲートが通るまで、製品 UI には `高品質` を出さない。

### 7.2 入力領域

MI-GAN と同じ 512px タイル分割を Big-LaMa にそのまま流用しない。
高品質モデルは大きな文脈を使えることが価値なので、初期実装は **マスク bbox + 周辺コンテキストを
1 回の推論に入れる** 方針にする。

推奨:

- bbox を取り、周囲に `max(256px, bbox 長辺の 25%)` 程度の context padding を足す。
- 画像境界で clamp する。
- 推論サイズ上限は probe 後に決める。初期候補は長辺 2048px。
- 上限を超える場合は region を縮小して推論し、結果を region サイズへ戻す。
- MVP では Big-LaMa のタイル分割を入れない。大判で必要になったら別フェーズで検討する。

### 7.3 マスク処理と合成

保存済みマスクはそのまま保持するが、推論入力には小さな膨張を入れてよい。
これは境界の残りやハローを減らすための内部処理で、DB 上のマスクは変更しない。

初期方針:

- 推論入力マスク: composite mask を 4-8px 程度膨張。
- 合成範囲: 膨張後マスクを使い、境界を 2-4px feather する。
- 透過画像では元画像の alpha を保持する。MI-GAN 経路と同じく RGB だけを補完する。

膨張・feather の具体値は probe 結果で調整し、UI には出さない。

## 8. 失敗時の扱い

高品質モデルでロード / 推論に失敗した場合:

- その実行は失敗として扱い、`[高品質補完失敗]` のようなフィードバックを出す。
- 黙って MI-GAN の結果を保存しない。ユーザーが高品質を選んだのに別結果になるのを避ける。
- ただし設定自体は高品質のまま維持してよい。パック破損やモデル欠損が確認された場合は
  追加パック状態を invalid にし、補完モデルを標準へ戻す。

`AiRuntime` が未初期化 / DirectML 初期化失敗など、標準 MI-GAN でも失敗する種類の問題では、
現行と同じ diffusion fallback を使うかは実装時に再確認する。Big-LaMa 経路では、品質目的に反するため
diffusion fallback へ自動保存する挙動は避ける。

## 9. テスト計画

### 9.1 自動テスト

- `EraseInpaintModel` の serde / 文字列変換 / 既定値。
- 編集用追加パック manifest が `inpaint_model` を認識する。
- pack 未導入時に `HighQualityBigLaMa` が `StandardMiGan` へ正規化される。
- `EraseResultKey` がモデル違いを区別する。
- 補完モデル切替で preview cache と pending が破棄される。
- 高品質選択時、未導入なら追加パックダイアログ要求状態になる。
- ダウンロード cancel 時、補完モデルが標準へ戻る。

### 9.2 手動 / 画像比較

少なくとも次の画像セットで MI-GAN / Big-LaMa を比較する。

- スキャン小ゴミ、細い傷。
- AI 生成イラストの背景にある余計な小物。
- 背景の模様、壁、床、髪、服の柄。
- 写真の人物・看板・電線などの不要物。
- 透過 PNG。
- 大きな画像、見開きページ、ZIP/PDF 由来ページ。

チェック観点:

- 境界ハローが残らない。
- 長い線や模様が途切れにくい。
- タイル境界が見えない。
- UI が固まらない。
- キャンセルやページ移動で古い結果が表示されない。

## 10. 実装フェーズ

1. Big-LaMa ONNX / DirectML probe を作る。
2. 編集用追加パック manifest に `inpaint_model` を追加する。
3. `Settings` と消しゴムパネルに補完モデル選択を追加する。
4. 未導入時の追加パックダイアログ導線をつなぐ。
5. `EraseResultKey` / pending / preview cache をモデル対応にする。
6. Big-LaMa 推論 adapter を追加する。
7. 比較画像と UI snapshot を確認する。
8. `spec.md`、マニュアル、製品ページ、About ライセンス表示を更新する。

## 11. 実装時に読むドキュメント

- [preset-and-adjustment.md](preset-and-adjustment.md): 消しゴム結果 cache、マスク保存、表示順。
- [async-architecture.md](async-architecture.md): AI 消しゴム worker、pending、キャンセル。
- [editing-add-on-download-spec.md](editing-add-on-download-spec.md): 追加パック、manifest、ダウンロード UI。
- [ui-responsiveness.md](ui-responsiveness.md): UI スレッドでモデルロード / I/O / zip 展開をしないための確認。
- [architecture-overview.md](architecture-overview.md): `ai/`、`ui_erase.rs`、DB、追加パックの全体位置づけ。
# スクリーントーン「弱／強」比較画像

`01_source.png` は、カラー化のスクリーントーン濃淡変換を比較するための決定論的な
モノクロ画像です。長辺を 2048px にしているため、UI の `検出スケール 1.0` が
実効半径 1px に対応します。

比較手順:

1. `01_source.png` を mImageViewer で開く。
2. カラー化を有効にし、`元画像の明るさを保持 = 100%` にする。
3. `変換の強さ = 100%`、`検出スケール = 1.0` にする。
4. `弱（局所平均）` と `強（ガウシアン）` を切り替える。

`02_reference_weak_local_mean_scale1.png` と
`03_reference_strong_gaussian_scale1.png` は、各方式の輝度平滑化だけを適用した参考出力です。
`04_weak_vs_strong_difference_x4.png` は両者の絶対差を4倍強調しています。

再生成:

```powershell
python .\scripts\generate_tone_algorithm_comparison.py
```

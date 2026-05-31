# detector_probe

顔検出 / 領域検出モデルの精度を mIV 本体へ組み込む前に確認するための検証ツール。
指定フォルダ内の画像に対して複数モデルを実行し、検出枠とラベルを描画した同名画像を
出力フォルダへ保存する。

このツールは開発検証用であり、mIV 本体の配布物には含めない。

## 必要な Python パッケージ

```powershell
python -m pip install opencv-python onnxruntime numpy
```

## 使い方

モデル一覧:

```powershell
python tools\detector_probe\detector_probe.py list-models
```

モデルをダウンロード:

```powershell
python tools\detector_probe\detector_probe.py download --models default
python tools\detector_probe\detector_probe.py download --models all
```

検出して注釈画像を出力:

```powershell
python tools\detector_probe\detector_probe.py run `
  --input G:\sample_images `
  --output G:\sample_images_detected `
  --models default `
  --download
```

サブフォルダも処理:

```powershell
python tools\detector_probe\detector_probe.py run `
  --input G:\sample_images `
  --output G:\sample_images_detected `
  --models all `
  --recursive `
  --download
```

## 出力

- 入力と同じ相対パス / 同じファイル名の注釈画像
- `detections.jsonl`: 画像ごとの検出結果
- `summary.csv`: モデル別の検出数 / 平均処理時間

## モデルについて

標準の `default` は以下を対象にする。

- OpenCV Zoo YuNet 顔検出
- NudeNet 320n

`all` では追加の YuNet 量子化モデル、DeepGHS の顔検出候補、DeepGHS の NudeNet ONNX 変換版も
試す。モデルのライセンスや利用条件はそれぞれ異なるため、検証目的でのみ扱うこと。

NudeNet 640m は公式 README に GitHub Releases のリンクが掲載されているが、環境によって
匿名ダウンロードが GitHub ログイン画面へリダイレクトされることがあるため、`default` と `all` からは
外している。手元で取得できた場合は `dev/detector_probe/models/nudenet_640m.onnx` に置き、
`--models nudenet_640m` を指定して実行できる。

mIV 本体にモデルを同梱する判断とは別物として扱う。

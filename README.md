# mImageViewer

Windows 向け高速サムネイルビューワー

[![GitHub release](https://img.shields.io/github/v/release/MikageSawatari/mimageviewer)](https://github.com/MikageSawatari/mimageviewer/releases/latest)
[![License: Freeware](https://img.shields.io/badge/license-freeware-blue)](https://mikage.to/mimageviewer/)

## こんな方におすすめ

- **大量の写真を素早く確認したい** — RAW・HEIC にも対応。フォルダを開くだけでサムネイル一覧
- **AI 生成画像のプロンプトを確認しながら閲覧したい** — PNG に埋め込まれた Stable Diffusion / ComfyUI のメタデータを自動表示
- **ZIP にまとめた漫画や同人誌をそのまま読みたい** — 展開不要で ZIP 内をブラウズ。見開き表示・右→左読みに対応
- **大量の PDF をサムネイルで見比べたい** — PDF をページごとにサムネイル表示。パスワード付きにも対応

## 主な機能

- GPU アクセラレーションによるサムネイルグリッド表示
- SQLite + WebP サムネイルキャッシュ（2回目以降は瞬時表示）
- フルスクリーン表示（前後画像の先読み付き）
- 見開き表示（左→右 / 右→左、表紙単独表示対応）
- 画像分析モード（カラーピッカー、ヒストグラム、色差強調、グレースケール等）
- AI 画像メタデータ表示（A1111/Forge、ComfyUI、NovelAI）
- EXIF 表示（日本語タグ名対応）
- 非破壊回転
- ZIP 内画像のブラウズ（展開不要）
- PDF 表示（ページごとレンダリング、パスワード付き対応、PDFium 内蔵）
- RAW / HEIC / AVIF / JPEG XL 対応（Windows WIC 経由）
- 動画サムネイル表示（MP4, AVI, MKV 等）
- お気に入りフォルダ、カスタマイズ可能なツールバー
- メタデータキーワード検索、スライドショー

## 対応フォーマット

| 種類 | フォーマット |
|------|-----------|
| 静止画（内蔵） | JPEG, PNG, GIF, WebP, BMP |
| 静止画（WIC） | HEIC, AVIF, JPEG XL, TIFF, 各社 RAW |
| アニメーション | GIF, APNG, Animated WebP |
| ドキュメント | PDF |
| アーカイブ | ZIP |
| 動画（サムネイルのみ） | MP4, AVI, MOV, MKV, WMV, MPEG |

## 動作環境

- Windows 10 / 11（64bit）
- DirectX 12 対応 GPU
- メモリ 4GB 以上推奨

## ダウンロード

[Releases ページ](https://github.com/MikageSawatari/mimageviewer/releases/latest) から以下のいずれかをダウンロードできます。

- **インストーラ版** (`mImageViewer_setup.exe`) — スタートメニュー登録・アンインストール機能付き
- **単体 exe 版** (`mimageviewer.exe`) — 任意のフォルダに置いて実行するだけ

設定は `%APPDATA%\mimageviewer\` に保存されます。

## マニュアル

[オンラインマニュアル](https://mikage.to/mimageviewer/manual/) — インストール方法・操作方法・キーボードショートカット・設定リファレンス

## 技術情報

- **言語**: Rust (edition 2024)
- **GUI**: eframe / egui (wgpu バックエンド)
- **PDF エンジン**: PDFium（exe に埋め込み）
- **サムネイルキャッシュ**: SQLite + WebP

## 更新履歴

### v0.4.0
- PDF 表示（ページごとレンダリング、パスワード付き対応、PDFium 内蔵）
- 見開き表示（左→右 / 右→左、表紙単独表示対応）
- 画像分析モード（カラーピッカー、ヒストグラム、色差強調、グレースケール等）
- フォルダサムネイルプレビュー（1枚目の画像＋名前バッジ）
- ZIP/PDF ファイルバッジ表示
- 「アプリケーションで開く」コンテキストメニュー
- PDF/ZIP キャッシュオプション
- EXIF 日本語タグ名表示
- インストーラ版を追加

### v0.3.0
- AI 画像メタデータパネル（Stable Diffusion / ComfyUI / Midjourney プロンプト表示）
- EXIF 表示（タグフィルタ設定付き）
- スライドショー（間隔設定可能）
- 非破壊回転（SQLite 保存）
- メタデータキーワード検索（Ctrl+F）
- 右クリックコンテキストメニュー
- 複数選択（Space / Ctrl+クリック）
- 同名ファイル処理設定

## ライセンス

フリーソフトウェアです。個人・商用を問わず無料でご利用いただけます。再配布・改変はご遠慮ください。

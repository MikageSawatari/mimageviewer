================================================================
 mImageViewer (エムイメージビューワー) Version 4.3.0
================================================================

Windows 向け 高速サムネイル画像ビューワー

  Copyright (C) 2025-2026 Mikage Sawatari
  Web: https://mikage.to/mimageviewer/
  連絡先: mikage@sawatari.info


----------------------------------------------------------------
 1. ソフトの概要
----------------------------------------------------------------

mImageViewer は Windows 11 向けの高速画像ビューワーです。
GPU アクセラレーションによるサムネイルグリッド表示を特徴とし、
大量の画像ファイルをフォルダ単位でブラウズできます。

主な機能:

  - GPU 描画によるサムネイルグリッド表示
  - サムネイルキャッシュ（2 回目以降は瞬時表示）
  - フルスクリーン表示（前後画像の先読み、見開き対応）
  - 外出先からのリモート閲覧（自宅 PC のライブラリを手元の
    スマートフォン / タブレット / ノート PC のブラウザで開く。
    既定はオフ。接続には Tailscale と PIN の両方が必要）
  - タッチ操作（タブレット PC / タッチ対応ディスプレイ）
  - 音楽再生（波形・スペクトラム表示、VST3 対応）
  - テキスト注釈・図形注釈（漫画のセリフ入力にも対応）
  - 隠蔽加工（モザイク / 白塗り / 黒塗り / ぼかし）
  - 切り取り・表示トリム（非破壊）
  - 補正レイヤー（画像の一部だけを補正）
  - 360 度パノラマ表示
  - 本棚（複数フォルダの画像を 1 冊にまとめて読む）
  - スマートフォルダ（条件を保存して自動で集める）
  - 画像分析モード（カラーピッカー、ヒストグラム等）
  - AI アップスケール（Real-ESRGAN / Real-CUGAN / NMKD-Siax）
  - AI JPEG ノイズ除去
  - AI 画像修復（消しゴムツール、MI-GAN）
  - 画像補正（明るさ / コントラスト / 色調、4 プリセット）
  - ポストフィルタ 38 種類（CRT / 機種別減色 / フィルム等）
  - AI 生成画像のメタデータ表示（埋め込まれたプロンプト等）
  - 独自タグ機能（アプリ内カタログに非破壊保存、画像/動画/ZIP/PDF/フォルダ対応）
  - グローバル全文検索（お気に入り全体のメタデータを横断検索）
  - 自動インデックス管理（ファイルの追加・削除を自動追従）
  - EXIF 表示（日本語タグ名対応）
  - 非破壊回転
  - ZIP 内画像のブラウズ（展開不要）
  - RAR / CBR / 7z / LZH の ZIP 変換閲覧（RAR / CBR はパスワード付きにも対応）
  - PDF 表示（PDF 表示エンジン内蔵、パスワード付き対応）
  - DRM のない EPUB を PDF に変換して閲覧（音声・動画は含まれません）
  - RAW 表示・現像（内蔵）/ HEIC / AVIF / JPEG XL 表示（Windows の表示機能）
  - Susie 画像プラグイン（.spi、32bit）対応
  - 動画インライン再生（MP4 / MKV / MOV / AVI / WMV / MPG / HEVC / AV1 等、
    GPU ハードウェアデコード対応、シーク / 倍速再生 / タイル モード /
    ブックマーク / チャプター / ラウドネス ノーマライズ）
  - VST3 プラグイン処理（動画音声をリアルタイムにメーター / EQ 等へ通す）
  - AI 動画アップスケール（オフライン処理、長尺対応の再開機能付き）
  - NVIDIA GPU 向け TensorRT 高速化（DirectML 比 1.4〜4.5 倍）
  - レーティング（★1〜5、フォルダ単位のコンテナ★、フィルタ）
  - お気に入りフォルダ、カスタマイズ可能ツールバー
  - UI テーマ（システム / ライト / ダーク）
  - タスクトレイ常駐（バックグラウンドで索引を最新に維持）
  - スライドショー、メタデータキーワード検索


----------------------------------------------------------------
 2. 取り扱い種別
----------------------------------------------------------------

フリーソフトウェア（MIT ライセンス）です。
個人・商用を問わず無償でご利用いただけます。


----------------------------------------------------------------
 3. 動作環境
----------------------------------------------------------------

  OS        : Windows 11
              （64bit 版のみ）
  CPU       : x86-64（Intel / AMD）
              音響調整 (EffeTune) には AVX2 と FMA の両方に対応した CPU が必要です。
  メモリ    : 4 GB 以上（8 GB 以上推奨）
  GPU       : DirectX 12 対応 GPU
              （最新のグラフィックドライバを推奨）
  ストレージ: インストール時 約 470 MB
              初回起動時に %APPDATA% へ関連ファイルを展開するため、
              追加で約 500 MB の空き容量が必要です。
              キャッシュや利用中の保存データには別途空き容量が必要です。

追加ソフト: EPUB の変換と音響調整 (EffeTune) の画面表示には
          Microsoft Edge WebView2 Runtime が必要です。
          Windows 11 には標準で含まれています。

AI アップスケール / JPEG ノイズ除去 / 消しゴム (画像修復) 等の
AI 機能は DirectML（Microsoft 公式）を利用します。
DirectML.dll は Windows 11 に標準で同梱
されているため、対応 OS ならば追加インストールは不要です。

RAW は追加インストールなしで表示・現像できます。
HEIC / AVIF / JPEG XL を表示するには Windows Imaging
Component（WIC）が必要ですが、Windows 11 には標準で
含まれています。追加が必要な場合は Microsoft Store から次を入れます:
  - HEIC / HEIF: HEIF 画像表示オプション
    HEIC は、HEVC に対応していない PC では HEVC ビデオ拡張機能も必要です。
    HEVC ビデオ拡張機能は有料の場合があります。
  - AVIF: AV1 ビデオ拡張機能
  - JPEG XL: JPEG XL 画像表示オプション


----------------------------------------------------------------
 4. インストール方法
----------------------------------------------------------------

  1. 同梱の mImageViewer_setup.exe をダブルクリックします。
  2. 画面の指示に従ってインストールを進めます。
     （管理者権限が必要です。UAC のダイアログが表示されます）
  3. 既定のインストール先は次のとおりです:
       C:\Program Files\mImageViewer\
  4. インストールが完了すると、スタートメニューに
     「mImageViewer」が追加されます。

設定ファイル・サムネイルキャッシュ・AI モデル等は
初回起動時に次のフォルダへ作成されます:

    %APPDATA%\mimageviewer\


----------------------------------------------------------------
 5. アンインストール方法
----------------------------------------------------------------

Windows の「設定」→「アプリ」→「インストールされているアプリ」
から「mImageViewer」を選択し、「アンインストール」をクリック
してください。

アンインストール時に「設定ファイルとキャッシュを削除しますか？」
と尋ねられます。

  - 「はい」を選ぶと、%APPDATA%\mimageviewer\ の設定・キャッシュ等が
    削除されます。
  - 「いいえ」を選ぶと、設定とキャッシュは保持されます。
    再インストール時に同じ設定で使い始められます。

次のフォルダは、どちらを選んでも残ります。不要になった場合は、
mImageViewer と関連するアプリを終了してから手動で削除できます。

  - %APPDATA%\effetune\ または %APPDATA%\Frieve\EffeTunePlugin\
    音響調整 (EffeTune) のプリセット・設定です。他の EffeTune 製品と
    共有するため、自動では削除しません。他の製品でも使わなくなった
    ことを確認してから削除してください。
  - %APPDATA%\mimageviewer-vst3-host.exe\
    EffeTune に取り込んだ IR ファイル・測定データ、ビジュアライザーの
    レイアウト・背景画像です。
  - %APPDATA%\mimageviewer-remote\
    リモート閲覧用サービスの動作記録等です。


----------------------------------------------------------------
 6. 動作に関するご注意
----------------------------------------------------------------

本ソフトウェアは以下のフォルダを自動で作成・利用します:

    %APPDATA%\mimageviewer\

  - settings.db      : アプリケーション設定（SQLite、10 世代の自動バックアップ付き）
  - cache\           : サムネイルキャッシュ（SQLite）
  - models\          : AI 用 ONNX モデル（初回展開）
  - logs\            : エラーログ
  - pdfium.dll       : PDF 表示用ライブラリ（初回展開）
  - mimageviewer-susie32.exe
                     : Susie プラグイン用 32bit ワーカー
                      （初回展開）
  - runtime\<version>\mimageviewer-epub-pdf.exe
                     : EPUB 変換用プログラム（初回展開）
  - epub_cache\      : EPUB から変換した PDF（管理画面で削除予約）
  - runtime\<version>\effetune\<hash12>-<generation>\EffeTune Mixwright.vst3\
                     : 音響調整用プラグインと関連ファイル（初回展開）
  - effetune\        : 次回起動時に復元する音響調整の状態

音響調整のプリセット・設定、取り込んだ IR ファイル・測定データ、
ビジュアライザーのレイアウト・背景画像は、上記とは別のフォルダに
保存されます。リモート閲覧用サービスも %APPDATA%\mimageviewer-remote\
を利用します。これらの保存先と削除方法は「5. アンインストール方法」
を参照してください。

また、お気に入りに登録したフォルダの更新を検知するため、
内部的に Windows API の ReadDirectoryChangesW による
フォルダ監視を行います。

本ソフトウェアが外部と通信するのは次の場合だけです。
利用状況の送信・広告・アカウント登録は一切ありません。
画像・動画そのものを外部へ送ることもありません。

  - 更新確認: 新しいバージョンの有無を調べるため、配布元
    （GitHub）の公開リリース情報を取得します。個人を特定する
    情報は送信しません。環境設定からオフにできます。
  - リモート閲覧（既定はオフ）: 有効にすると、この PC の中で
    接続を待ち受けます。本ソフトウェア自身がインターネットへ
    出ることはなく、外出先からつなぐには利用者ご自身が別途
    Tailscale を設定する必要があります。接続には PIN も必要です。

詳細は次のページをご覧ください:
https://mikage.to/mimageviewer/privacy.html


----------------------------------------------------------------
 7. ライセンス
----------------------------------------------------------------

本体: MIT ライセンス
同梱ライブラリのライセンスは以下のとおりです:

  - PDFium (BSD-3-Clause): Google Chrome の PDF エンジン
  - ONNX Runtime (MIT): Microsoft
  - DirectML (Microsoft 独自ライセンス): Microsoft
  - LibRaw (CDDL-1.0): LibRaw LLC
    ライセンス全文と著作権表記: LIBRAW-LICENSE.txt
    対応ソース: https://mikage.to/mimageviewer/libraw-0.22.2-source.tar.gz
  - zlib 1.3.1 (zlib): Jean-loup Gailly and Mark Adler
    ライセンス全文: ZLIB-LICENSE.txt
  - libjpeg-turbo (IJG / BSD-3-Clause / zlib)
    ライセンス全文: LIBJPEG-TURBO-LICENSE.txt
    This software is based in part on the work of the Independent JPEG Group.
  - eframe / egui (MIT OR Apache-2.0): Emil Ernerfeldt and contributors
    ライセンス全文はインストール先の egui-LICENSE-MIT.txt /
    egui-LICENSE-APACHE.txt を参照してください。
  - FFmpeg (LGPLv3-or-later): FFmpeg project
    Source and license notes: https://mikage.to/mimageviewer/
  - UnRAR source code (UnRAR license): Alexander Roshal / RARLAB
    RAR 展開に使用します。ライセンス全文は UNRAR-LICENSE.txt を参照してください。
  - EffeTune Mixwright (MIT): Copyright (c) 2025-2026 Yoshiyuki Kobayashi
    同梱コンポーネント（VST3 SDK、JSZip と内包される lie / immediate /
    setImmediate / pako (zlib)、CHOC、PFFFT、fdlibm、および通知に記載されたその他の
    ライブラリ）には、それぞれのライセンスが適用されます。
    ライセンス全文は、アプリの「ソフトウェア情報」（バージョン情報）の
    EffeTune THIRD-PARTY-NOTICES / DSP NOTICE / 補足通知で確認できます。
  - Steinberg VST3 SDK (MIT): Steinberg Media Technologies GmbH
  - Twemoji 絵文字グラフィックス (CC-BY 4.0): Twitter, Inc. and other contributors
    （注釈機能のスタンプに使用）

編集用追加ファイル（任意ダウンロード）に含まれる内容のライセンス:

  - オノマトペ向けフォント (SIL OFL 1.1): Google Fonts 提供のフォント
  - 被写体分離モデル BiRefNet (MIT): ZhengPeng7 / onnx-community
    各ライセンス全文は追加ファイル内に同梱され、アプリの
    「ソフトウェア情報」にも一覧表示されます。

AI モデルは各配布元のライセンスに従います。詳細は
オンラインマニュアル（下記）を参照してください。


----------------------------------------------------------------
 8. お問い合わせ・サポート
----------------------------------------------------------------

  作者       : Mikage Sawatari
  メール     : mikage@sawatari.info
  Web        : https://mikage.to/mimageviewer/
  マニュアル : https://mikage.to/mimageviewer/manual/
  不具合報告 : https://github.com/MikageSawatari/mimageviewer/issues


----------------------------------------------------------------
 9. 更新履歴
----------------------------------------------------------------

バージョンごとの変更点は以下のページをご参照ください:

  https://mikage.to/mimageviewer/manual/changelog.html


================================================================

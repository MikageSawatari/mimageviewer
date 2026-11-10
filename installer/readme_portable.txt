================================================================
 mImageViewer (エムイメージビューワー) ポータブル版 Version 5.1.0
================================================================

Windows 向け 高速サムネイル画像ビューワー（ポータブル版）

  Copyright (C) 2025-2026 Mikage Sawatari
  Web: https://mikage.to/mimageviewer/
  連絡先: mikage@sawatari.info


----------------------------------------------------------------
 1. ポータブル版とは
----------------------------------------------------------------

このフォルダは mImageViewer の「ポータブル版」です。

  - インストール不要。zip を解凍して mimageviewer.exe を
    ダブルクリックするだけで起動します。
  - 管理者権限は不要です（UAC のダイアログは出ません）。
  - 設定・キャッシュ・AI モデル・音響調整の保存データは、
    このフォルダの data\ 以下に作られます。
    利用者が追加したユーザー VST は独自に APPDATA 等へ保存する
    場合があり、本アプリからは止められません。
    本棚の作品や書き出したファイルは指定先に保存します。
    処理によって Windows の一時フォルダも使います。
  - フォルダごと USB メモリや別ドライブ・別 PC にコピーして
    持ち運べます（中の data\ ごと移動します）。

インストーラ版（mImageViewer_setup.exe）との違い:

                       ポータブル版        インストーラ版
  インストール         不要（解凍のみ）    必要
  管理者権限           不要                必要（UAC）
  設定・データの保存先 このフォルダ\data\  %APPDATA%\mimageviewer\
  アンインストール     フォルダを削除      Windows の設定から削除
  VST3 プラグイン処理  対応                対応

  音響調整 (mIV Sound) とユーザー VST の両方を利用できます。
  音響調整の設定・プリセットは data\effetune\plugin\、
  取り込んだ IR・測定データ、Visualizer のレイアウト・背景画像は
  data\effetune\webview\ に保存します。旧共有先のデータは引き継ぎません。
  ほかの EffeTune 製品とは自動で共有しません。保存済みプリセットの
  受け渡しには、音響調整の「設定 → バックアップ / 復元」を使います。


----------------------------------------------------------------
 2. 使い方
----------------------------------------------------------------

  1. この zip を「書き込み可能な場所」に解凍します。
     例: デスクトップ、ドキュメント、D ドライブ、USB メモリ など

     ※ C:\Program Files\ など書き込みが制限された場所に置くと、
        データを保存できないためエラーで起動できません。その場合は
        書き込み可能な場所に解凍し直してください。

  2. フォルダ内の mimageviewer.exe をダブルクリックします。

  3. 初回起動時に、このフォルダの中に data\ フォルダが作られ、
     設定・サムネイルキャッシュ・AI モデル等がそこに保存されます。

更新時は、mImageViewer と、mImageViewer を使っている AI アプリ
(Claude・ChatGPT など) を終了してからファイルを置き換えてください。


----------------------------------------------------------------
 3. フォルダの構成
----------------------------------------------------------------

  mimageviewer.exe                本体
  mimageviewer-remote.exe         リモート閲覧用プログラム
  mimageviewer-epub-pdf.exe       EPUB 変換用プログラム
  mimageviewer-mcp.exe            AI アプリとの連携用プログラム
  *.dll                           動画再生 / PDF / AI 用ライブラリ
  mimageviewer-susie32.exe        Susie プラグイン用 32bit ワーカー
  mimageviewer-vst3-host.exe      VST3 プラグイン用プログラム
  effetune\                       音響調整 (mIV Sound) 用ファイル
  vcrt\                           VST3 用ランタイム
  models\                         AI 用モデル
  LICENSE-ffmpeg.txt              FFmpeg のライセンス本文
  UNRAR-LICENSE.txt               UnRAR のライセンス本文
  LIBRAW-LICENSE.txt              RAW 表示・現像のライセンス本文
  PDFIUM-LICENSE.txt              PDF 表示と内部部品のライセンス全文と著作権表記
  LIBAVIF-LICENSE.txt             AVIF 表示のライセンス本文
  DAV1D-LICENSE.txt               AVIF 表示のライセンス本文
  LIBYUV-LICENSE.txt              AVIF 表示のライセンス本文
  ZLIB-LICENSE.txt                zlib のライセンス本文
  LIBJPEG-TURBO-LICENSE.txt        libjpeg-turbo のライセンス本文
  egui-LICENSE-MIT.txt            egui の MIT ライセンス本文
  egui-LICENSE-APACHE.txt         egui の Apache-2.0 ライセンス本文
  readme.txt                      このファイル
  data\                           （初回起動時に自動作成）
                                  設定・キャッシュ・ログ等の保存先

  ※ exe と同じ場所にある DLL 群と補助プログラムは本体の動作に必要です。
     ファイル名を変更したり、個別に移動・削除しないでください。


----------------------------------------------------------------
 4. アンインストール
----------------------------------------------------------------

本体はこのフォルダごと削除できます。data\ 内の設定・キャッシュ・
音響調整の保存データも一緒に消えます。
外に保存した作品・元ファイル、ユーザー VST が独自に保存したデータ、
連携先の AI アプリに行った登録などは残ります。必要に応じて、
登録を外す操作や各プラグインの案内に従って削除してください。


----------------------------------------------------------------
 5. 動作環境
----------------------------------------------------------------

  OS        : Windows 11（64bit 版のみ）
  CPU       : x86-64（Intel / AMD）
              音響調整には AVX2 と FMA の両方への対応が必要です。
  メモリ    : 4 GB 以上（8 GB 以上推奨）
  GPU       : DirectX 12 対応 GPU（最新のドライバを推奨）

追加ソフト: EPUB の変換と音響調整の画面表示には
          Microsoft Edge WebView2 Runtime が必要です。
          Windows 11 には標準で含まれています。

RAW の表示・現像と AVIF の表示は追加インストール不要です。
AI 機能は DirectML（Microsoft 公式、Windows 11 標準同梱）を、
HEIC / JPEG XL 表示は Windows Imaging Component（WIC）を
利用します。いずれも Windows 11 に標準で含まれていますが、画像形式に
応じて Microsoft Store から次の追加機能が必要な場合があります:
  - HEIC / HEIF: HEIF 画像表示オプション
    HEIC は、HEVC に対応していない PC では HEVC ビデオ拡張機能も必要です。
    HEVC ビデオ拡張機能は有料の場合があります。
  - JPEG XL: JPEG XL 画像表示オプション

本アプリの主な通信は次のとおりです。利用状況の自動送信や広告は
ありません。リモート閲覧では、接続した端末へ画像・動画・音声を配信します。
音響調整のヘルプや「AI に質問」は外部ページをブラウザで開きます。
利用者が追加したユーザー VST の通信は、各プラグインの動作に従います。

  - 更新確認: 新しいバージョンの有無を調べるため、配布元（GitHub）の
    公開リリース情報を取得します。個人を特定する情報は送信しません。
    環境設定からオフにできます。
  - リモート閲覧（既定はオフ）: 有効にすると、この PC の中で接続を
    待ち受けます。本ソフトウェア自身がインターネットへ出ることはなく、
    外出先からつなぐには利用者ご自身が別途 Tailscale を設定する必要が
    あります。接続には PIN も必要です。
  - AI の追加モデルや NVIDIA 向け TensorRT 高速化パックを利用する場合、
    ユーザー操作に応じてダウンロードを行い、その内容もこのフォルダの
    data\ 以下に保存されます。

詳細: https://mikage.to/mimageviewer/privacy.html


----------------------------------------------------------------
 6. ライセンス
----------------------------------------------------------------

本体: MIT ライセンス
同梱ライブラリのライセンスは以下のとおりです:

  - PDFium (BSD-3-Clause): Google Chrome の PDF エンジン
    ライセンス全文と著作権表記: PDFIUM-LICENSE.txt (内部部品を含む)
  - ONNX Runtime (MIT): Microsoft
  - DirectML (Microsoft 独自ライセンス): Microsoft
  - LibRaw (CDDL-1.0): LibRaw LLC
    ライセンス全文と著作権表記: LIBRAW-LICENSE.txt
    AVIF のライセンス全文と著作権表記: LIBAVIF-LICENSE.txt / DAV1D-LICENSE.txt / LIBYUV-LICENSE.txt
    対応ソース: https://mikage.to/mimageviewer/libraw-0.22.3-source.tar.gz
  - zlib 1.3.1 (zlib): Jean-loup Gailly and Mark Adler
    ライセンス全文: ZLIB-LICENSE.txt
  - libjpeg-turbo (IJG / BSD-3-Clause / zlib)
    ライセンス全文: LIBJPEG-TURBO-LICENSE.txt
    This software is based in part on the work of the Independent JPEG Group.
  - eframe / egui (MIT OR Apache-2.0): Emil Ernerfeldt and contributors
    同梱の egui-LICENSE-MIT.txt / egui-LICENSE-APACHE.txt を参照してください。
  - FFmpeg (LGPLv3-or-later): FFmpeg project
    同梱の LICENSE-ffmpeg.txt および以下を参照してください:
    https://mikage.to/mimageviewer/
  - UnRAR source code (UnRAR license): Alexander Roshal / RARLAB
    RAR 展開に使用します。同梱の UNRAR-LICENSE.txt を参照してください。
  - mIV Sound (MIT): EffeTune / EffeTune Mixwright (作者 Frieve) を元にした派生版
    Copyright (c) 2025-2026 Yoshiyuki Kobayashi
    同梱コンポーネントには、それぞれのライセンスが適用されます。
    通知全文はアプリの「ソフトウェア情報」（バージョン情報）の
    mIV Sound THIRD-PARTY-NOTICES / WebView THIRD-PARTY-NOTICES / DSP NOTICE
    で確認できます。
  - Steinberg VST3 SDK (MIT): Steinberg Media Technologies GmbH
  - Twemoji 絵文字グラフィックス (CC-BY 4.0): Twitter, Inc. and other contributors
    （注釈機能のスタンプに使用）

3D 写真の追加モデル（任意ダウンロード）:

  - Depth Anything 3 (DA3 Mono-Large、Apache-2.0) の派生物
    重みと内部計算を fp16 化し、入力を 1 枚・504 x 504 px に固定しています。
    再学習は行っていません。LICENSE と NOTICE はモデルと一緒に取得します。
    環境設定の「3D 写真の追加モデル」から保存先フォルダを開いて確認できます。
    保存先は実行ファイルと同じ場所の data\addons\depth\ 内です。

AI モデルは各配布元のライセンスに従います。詳細は
オンラインマニュアル（下記）を参照してください。


----------------------------------------------------------------
 7. お問い合わせ・サポート
----------------------------------------------------------------

  作者       : Mikage Sawatari
  メール     : mikage@sawatari.info
  Web        : https://mikage.to/mimageviewer/
  マニュアル : https://mikage.to/mimageviewer/manual/
  不具合報告 : https://github.com/MikageSawatari/mimageviewer/issues
  更新履歴   : https://github.com/MikageSawatari/mimageviewer/releases


================================================================

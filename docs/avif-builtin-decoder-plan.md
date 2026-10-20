# AVIF 内蔵デコーダ (libavif + dav1d) 設計 — backlog §1.407

正本はこの文書。バックログ §1.407 には現状と残りだけを書く。

## 1. 経緯と原因

- 5ch >>599 (2026-10-20): AVIF がデコードエラーになり、Susie プラグイン導入後は表示できるが遅い、との報告。
- 開発機 (Windows 11 Pro 10.0.26300、Microsoft.AV1VideoExtension 2.0.35.0) で、WIC (mIV と同じ
  Frame 0 → 32bpp BGRA 変換) に自作 AVIF 約 35 種を通した (観測者: ClaudeCode のスクリプト。mIV 本体は起動していない)。
  - **デコードエラーは再現しなかった**: 8/10/12bit、4:2:0/4:2:2/4:4:4、モノクロ、full range、HDR PQ タグ、
    8192x6144、aom/svt/rav1e、アニメの 1 フレーム目、grid 2x2、透過付き、irot/imir はいずれも WIC で読めた
    (不正ファイル 2 種は失敗)。報告者の失敗の原因は**未特定**。候補 (未確認): 報告者の環境に AV1 拡張が無い、
    今回作れなかった構造を含む。
  - **WIC 経路の欠陥を 2 つ確認した**:
    1. 透過が失われる (アルファが全面 255)。
    2. AVIF の色変換行列・レンジ (nclx) を無視して色がずれる。BT.709 limited を明記した純赤が (233,0,2)、
       BT.601 full range を明記した純赤が (255,26,0)。元画像既知の 800x600 で元画像との PSNR は
       libavif 33.4dB に対し WIC 27.4dB。
- 利用者決定 (2026-10-20): WIC や Susie に頼らず、mIV が専用ライブラリで AVIF を読む。依存は
  **libavif + dav1d + libyuv** (B 案「同梱 FFmpeg の dav1d を流用し容器以降を自前実装」と比較のうえ A 案)。

## 2. 依存とビルド

| 部品 | 版 | ライセンス | 役割 |
| --- | --- | --- | --- |
| libavif | 1.4.2 | BSD-2-Clause (ファイル内に第三者 notice あり、全文を同梱) | 容器解析・grid・透過・irot/imir/clap・色変換 |
| dav1d | 1.5.3 | BSD-2-Clause | AV1 復号 |
| libyuv | 644251f25 | BSD-3-Clause | YUV→RGB の高速変換 |

- 版は libavif 1.4.2 自身が `cmake/Modules/LocalDav1d.cmake` / `LocalLibyuv.cmake` で固定している組み合わせ。
- `scripts/setup-libavif.sh` (エージェント用) がソースを SHA-256 / commit で照合して取得し、
  静的 CRT (`/MT`、`.cargo/config.toml` の `+crt-static` に合わせる) で `vendor/libavif/{include,lib,licenses}` を作る。
  meson 1.12.1・ninja 1.13.2 (venv)・NASM 3.02 (zip) も `vendor/libavif/tools/` に取得し、利用者の環境全体には入れない。
  再取得できるのでツリー外バックアップは不要。
- `crates/libavif-sys` は作成済みの静的ライブラリをリンクするだけで、cargo の時点で C ビルドはしない
  (dav1d は meson と NASM が要り、LibRaw のように cc で組めないため)。
- 新しい DLL / exe は増えない。dav1d は同梱 FFmpeg の中にもあるが、FFmpeg は別 DLL なので衝突しない。

## 3. 組み込み方

- デコード順: AVIF (拡張子 `avif`) は **内蔵 (libavif) → WIC → Susie**。他形式の順序と JPEG 高速経路は変えない。
  内蔵で読めないとき (想定外の構造・libavif の制限) の救済として WIC / Susie の順を残す。
- 通常ファイル・書庫内バイト列・サムネイル・フルサイズ・類似検索・コンテキストメニュー・本 (books)・mIV Remote
  (core の canonical loader と thumb_loader を通る) の全経路で、同じ 1 つの関数を通す。経路ごとに AVIF 分岐を複製しない。
- 出力は RGBA8 (straight alpha)。10/12bit は 8bit へ変換する。HDR の表示対応・アニメーション AVIF の全フレーム再生は
  範囲外 (1 フレーム目を静止画として表示する従来どおり)。ICC は他形式と同じく使わない。
- 向き: AVIF の表示向きは irot / imir (と clap) が正本で、EXIF Orientation は適用しない (HEIF/MIAF の規定、
  libavif・ブラウザと同じ)。内蔵デコーダが clap → irot → imir を適用した画素を返し、後段の EXIF 向き補正を
  その結果に重ねない。WIC で読んだ場合の扱いは変えない。
- 安全: libavif の `imageSizeLimit` / `imageDimensionLimit` を mIV の上限 (WIC と同じ 32768 辺、呼び出し元の
  割り当て予算) に合わせ、RGBA バッファは寸法を見て予算内のときだけ確保する。strict は無効
  (実在するファイルを表示する目的。ブラウザと同じ寛容さ)。
- スレッド: dav1d の内部スレッド数は呼び出しの種類で固定する (実行時の空き状況で変えない)。
  背景の一括処理 (サムネイル・類似検索など、既に並列 worker で動く) は 1、利用者が待っている
  フルサイズ表示は複数。具体値は実装で worker 数を確認して決め、ここに書く。
- UI スレッドへ新しいデコードを足さない。既存の呼び出し元のスレッドのまま差し替える。
- 診断: 実際に使ったデコーダ (libavif / WIC / Susie)・所要時間・失敗段階 (libavif の結果コード) を
  perf ログに残す。Susie 診断の「本体優先」「このプラグインは呼ばれません」の誤説明を直す。
- ライセンス: ルートに `LIBAVIF-LICENSE.txt` / `DAV1D-LICENSE.txt` / `LIBYUV-LICENSE.txt` を置き、
  LibRaw と同じくバージョン情報に全文を埋め込み、installer / portable に同梱する。ソース提供の義務はない。
- まれな失敗は既定の割り切り (ログ + 通知の既存経路)。

## 4. 検証

- `testdata/avif/` のフィクスチャ (`make_fixtures.py` で再生成) で、色 (象限の中心)、透過、grid、irot/imir、
  アニメの 1 フレーム目、モノクロ、10/12bit、4:2:2/4:4:4、不正ファイル (内蔵が失敗し WIC→Susie へ回る) を
  自動テストで確認する。パスとバイト列、ZIP 内、サムネイル経路の一致も確認する。
- 実機確認は利用者 (透過 AVIF・色・ZIP 内・mIV Remote)。

## 5. 未決・記録

- (実装後に記入) スレッド数の具体値、サムネイルキャッシュ済みの AVIF の扱い、exe サイズの増分、速度の比較。

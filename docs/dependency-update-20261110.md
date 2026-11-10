# PDFium 更新 (chromium/8138) — 2026-11-10

backlog §1.754 の記録。PDFium を `chromium/8125` から `chromium/8138` へ上げた。
版・入力資産の同一性・変更点・検証の範囲を扱い、公開・配布の完了記録ではない。
作業ブランチは `deps-754-pdfium` (worktree `G:\home\mimageviewer-754`)。

## 結果

| 依存物 | 更新前 | 更新後 |
| --- | --- | --- |
| PDFium | `chromium/8125`, `158.0.8125.0` | `chromium/8138`, `159.0.8138.0` (2026-11-04 公開) |

コードの変更はない。`vendor/` は git 管理外なので、ブランチに入るのは文書だけ。

## 取得手順

```bash
bash scripts/setup-pdfium.sh check   # Latest release: chromium/8138
bash scripts/setup-pdfium.sh         # vendor/pdfium/ へ展開
```

`setup-pdfium.sh` は `bblanchon/pdfium-binaries` の最新リリースから `pdfium-win-x64.tgz`
(V8 / XFA なし) を取り、`vendor/pdfium/` へ展開する。別に同じ資産を空のフォルダへ取得・展開し、
展開済み vendor と全ファイルを比べた。

## 資産の同一性

| 対象 | bytes | SHA-256 |
| --- | ---: | --- |
| 公式 `pdfium-win-x64.tgz` (`chromium/8138`) | 4,031,929 | `8d48973e3eb472dda0f5f2435e595ac043f9f2219dd71883392f549445afa8a5` |
| `vendor/pdfium/bin/pdfium.dll` | 7,824,384 | `71a3e63e6ab8f5746a0fc9de9bc3d0c0dc07eb60990dfd37d9db7fc891821943` |

- アーカイブの SHA-256 は GitHub のリリース資産に記載の digest と一致した。
- 資産内の全 49 ファイルが展開済みの `vendor/pdfium/` とバイト単位で一致し、資産外のファイルは無い。
- DLL の FileVersion / ProductVersion はともに `159.0.8138.0`。署名は無い (8125 と同じ)。
- `args.gn` は `target_cpu = "x64"`、`pdf_enable_v8 = false`、`pdf_enable_xfa = false` のまま。

## 8125 との差

ファイル構成は 8125 と同じ 49 件。中身が変わったのは次の 3 つだけ。

- `bin/pdfium.dll`
- `VERSION` (`MAJOR=159`、`BUILD=8138`)
- `PDFiumConfig.cmake` (`PDFium_VERSION` の行だけ)

`include/` の全ヘッダ、`lib/pdfium.dll.lib` (import library)、`LICENSE`、`licenses/` の全ファイル、
`args.gn` はバイト単位で同一。したがって:

- **API・ABI**: 公開ヘッダと import library が同一なので、`pdfium-render` 0.8 から使う関数の宣言・
  エクスポートに変化はない。コードの変更は要らない。
- **ライセンス**: 同梱のライセンス文は同一。lcms は 2.19.1 へ上がったが `licenses/lcms.txt` の本文は
  変わっていない。インストーラの readme と About の表記の変更も要らない。

上流の変更 (リリースノートの commit 一覧から、表示に関わるもの):

- 合成処理のベクトル化 (Vectorize scanline-compositor paths)、AGG のクリップ領域の交差の最適化
- 線形グラデーション (axial shading) の描画の変更と、軸の長さが 0 の場合の扱い
- lcms 2.19.1 (色管理)、libyuv・Skia・abseil・ツール類の更新
- `CPDF_SyntaxParser` の初期化漏れの修正、保存時の PDF ヘッダ版の修正 (mIV は保存しない)
- 未使用の GDI+ 関数の削除 (mIV は呼ばない)

描画の結果が変わり得るのは合成・クリップ・グラデーションの 3 つ。mIV の自動テストは描画結果の
画素を比べないので、ここは実機の表示で確かめる。

## mIV への組み込み

- 通常ビルドは `src/pdf_loader.rs` が DLL を `include_bytes!` で埋め込み、`data_dir::extract_embedded_file`
  が `%APPDATA%\mimageviewer\pdfium.dll` (と `.sha256` の印) へ展開する。展開時に内容の hash を比べるので、
  更新後の初回起動で既存の 8125 の DLL は 8138 に置き換わる。
- ポータブル版は `scripts/build-portable.ps1` が同じ DLL を exe の隣へ置く。
- PDF ワーカー (`--pdf-worker`) は展開済みの同じ DLL を読む。

## 検証

- `.\scripts\test-lib-full.ps1` (8138 を置いた worktree、2026-11-10): **17,062 件成功 / 失敗 0 / 75 skipped**
  (883.7 秒)。名前に `pdf` を含むテスト 534 件 (うち `pdf_loader` 117 件) を含み、skipped に PDF のテストは無い。
  `pdf_loader` のテストの一部は `vendor/pdfium/bin` の DLL を直接読み、実際の PDF の列挙・描画・読み方向を通す。
- `.\scripts\test-full.ps1` は回していない。`tests/` の統合テストと EPUB worker (lopdf を使う) は PDFium を
  読まないので、PDFium の差し替えで結果が変わる経路が無い。
- `.\scripts\build-dev.ps1`: 成功。`target\dev-runtime\mimageviewer-core.exe` の中に 8138 の `pdfium.dll` と同じバイト列が
  そのまま含まれ、8125 の DLL は含まれないことを照合した。
- 自動テストは描画結果の画素を比べない。上流の合成・クリップ・グラデーションの変更による見た目の差は、
  実機の表示でしか分からない。

## 残っていること (この更新とは別)

- About の「サードパーティ ライセンス」に PDFium が無い。インストーラの readme は
  「PDFium (BSD-3-Clause)」の 1 行だけで、`vendor/pdfium/LICENSE` と `licenses/` の全文は配布物に入っていない
  (`installer/mimageviewer.iss` と `scripts/build-portable.ps1` が同梱するライセンス文に PDFium の分が無い)。
  8125 以前からの状態で、今回の更新では変えていない。扱いは別に判断する。

製品バイナリはエージェントが起動していない。PDF の表示の実機確認は利用者に依頼する (未確認)。

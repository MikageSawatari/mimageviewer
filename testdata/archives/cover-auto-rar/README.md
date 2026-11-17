# 自動表紙切り出し用の合成 RAR

§1.778 / `docs/cover-crop-plan.md` §30 の回帰用。利用者の書庫・画像・設定は含まない。
RAR 5、非ソリッド (`-s-`)、保存方式 (`-m0`)、パスワード無し。
全体で約 65 KiB。通常のテスト実行に WinRAR は不要。

| ファイル | 内容 |
| --- | --- |
| `jacket.rar` | `000.png` = 892×512 のつながったカバー、`001.png`〜`008.png` = 70×100 の黒い本文 |
| `cover-only.rar` | 同じ `000.png` だけ。標本不足の判定済み見送り |
| `two-pages.rar` | 同じ表紙と本文 2 枚。標本不足の判定済み見送り |
| `zipdir.rar` | `book/000.png`〜`book/008.png`。ZipDir の prefix 制限用 |
| `duplicate-names.rar` | 既存 `rar_loader::direct_read_test_fixture_bytes` の合成 PNG 2 枚。保存名は両方 `page.png`、表示名は `page.png` / `page (2).png` |
| `crc-error.rar` | `jacket.rar` の `004.png` の PNG 先頭から 20 byte 目だけ反転。ヘッダ一覧は正常、展開は CRC エラー。001〜003 の結果を残し、004 以降は未知の枠にする |

カバーは既存 thumbnail worker 回帰で使う合成パターンと同じで、
Rust の `cover_auto_sample::rar_tests::fixture_jacket` を共有する。
折り返し・表表紙・背・裏表紙を色と縞で作るだけで、文字や実画像は使わない。

## 再生成

Windows の `C:\Program Files\WinRAR\Rar.exe` が必要。
リポジトリのビルド前提を整え、次の ignored test を明示実行する。

```powershell
cargo test -p mimageviewer --lib cover_auto_sample::rar_tests::generate_cover_auto_rar_fixtures -- --ignored --exact
```

生成コードは `src/cover_auto_sample/rar_tests.rs`。
一時フォルダへ Rust の既存 `image` encoder で合成カバーと本文を保存し、
作業ディレクトリをその一時フォルダにして次の形で圧縮する。

```powershell
& 'C:\Program Files\WinRAR\Rar.exe' a -s- -m0 -ma5 -idq -ep1 -r <出力RAR絶対パス> '*.png'
# ZipDir のケースの末尾引数は book
```

PNG encoder / WinRAR の版やタイムスタンプで書庫のバイト列は変わり得る。
本文は全て同じ画像なので、境界テストは一覧の実際の展開後サイズを読んで上限を注入する。
32 MiB / 64 MiB の大きな fixture は不要。

配置した書庫は上記の Rust generator と RAR 7.22 x64 で再生成・検証したもの。
`duplicate-names.rar` は既存の埋め込み fixture のバイト列をそのまま書き出す。
ヘッダ破損の回帰はテスト内で `jacket.rar` のコピーの追加領域サイズを壊すため、専用の書庫は保存しない。

## 静的確認

```powershell
& 'C:\Program Files\WinRAR\Rar.exe' lb .\testdata\archives\cover-auto-rar\zipdir.rar
& 'C:\Program Files\WinRAR\Rar.exe' t -idq .\testdata\archives\cover-auto-rar\jacket.rar
& 'C:\Program Files\WinRAR\Rar.exe' t .\testdata\archives\cover-auto-rar\crc-error.rar
```

最後のコマンドだけ失敗し、`004.png` だけが CRC エラーになるのが正しい。
RAR の標本読み取りはそのエラーで一度の走査を終えるため、後の正常ページを開き直して読まない。

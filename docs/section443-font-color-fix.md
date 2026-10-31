# §1.443 ライトテーマでシステムフォントが白くなる回帰

## 範囲と不変条件

v4.8.1 tag `94017f82e` を起点とする `fontcolor-1443` で、v4.8.2 パッチ候補を実装する。
同じ差分を次版 master へも取り込む想定。作業中の他 worktree は変更しない。
commit・master 統合・公開は ClaudeCode 側の担当で、この作業では行わない。

単色の文字はテーマ・明示文字色・上書き文字色で着色され、本来のカラー字形は色を保つ。
フォント候補やカラー絵文字を無効にせず、画素が白いかどうかで分類しない。
設定、保存データ、フォント選択、操作、非同期所有構造は変更しない。

## 原因確認

報告・利用者の実アプリ確認: v4.8.0 / v4.8.1 のライトテーマで MS UI Gothic、MS Gothic、
MS Mincho、MS PGothic、MS PMincho が白くなる。NSimSun / SimSun は全角文字だけ白くなる。
Meiryo は正常。本作業では製品 exe の起動・自動操作はしていない。

epaint 0.37.0 `src/text/font_face.rs` の `DependentFontData::has_color_glyphs` は
EBDT も候補に含め、`FontCell::has_color_glyph` が bitmap の形式を区別せず `is_some()` で
カラー扱いしていた。`rasterize_color_glyph` は白の paint・hint 無効で描画し、
常に `GlyphBitmap::is_color: true` を返す。
glifo 0.3.0 `src/glyph.rs::draw_glyphs` は `BitmapData::Mask` を非対応として outline へ進むため、
ここで描かれるのは白い単色 outline である。

`glyph_atlas.rs::allocate_outline` はその flag を atlas allocation へ渡す。
`text_layout.rs` はカラー字形の頂点を白にし、`tessellator.rs::tessellate_text` は
カラー頂点を通常の文字色・override 色で着色しない。結果としてライトテーマでも文字が白い。

## 実装

`vendor/epaint` に registry の 0.37.0 を複製し、root `Cargo.toml` の patch と workspace exclude を追加。
`Cargo.lock` は epaint の source/checksum 2行を取り除く変更だけ。
`.gitignore` は vendor 全体を無視しておらず、`git check-ignore` と untracked 列挙で
epaint のソースを追跡可能と確認したため、例外追加は不要だった。

`has_color_glyph` で Skrifa の `BitmapData::Mask` を除外する。
通常の hinted/subpixel outline rasterization が `is_color: false` を返すので、atlas と mesh は
既存の単色文字経路を使う。COLR と PNG/BGRA の判定・描画経路は保持する。
原本・上流照合・license・ローカル差分は [PATCHES.md](../vendor/epaint/PATCHES.md) に記録する。
モーダル化等の簡素化は対象外で、既存の字形形式判定1か所で修正できる。
まれな失敗の回復機構も追加しない。

## 回帰テスト

`vendor/epaint/src/text/font_color_tests.rs` は `C:/Windows/Fonts` 相当から実フォントを読み、
7 face と Meiryo、Segoe UI Emoji の9件を検査する。
TTC の英語 family name table を直接読み、`msgothic.ttc` は 0=MS Gothic、
1=MS UI Gothic、2=MS PGothic、`msmincho.ttc` は 0=MS Mincho、1=MS PMincho、
`simsun.ttc` は 0=SimSun、1=NSimSun と確認した。
12 / 14 / 16 / 20 / 32pt、pixels-per-point 1 / 1.5 / 2 を使う。
直接 face から字形を引き、tofu / fallback で成功しないようにする。
普通の文字は `is_color: false`、黒・青・灰の mesh 頂点色と赤い override を検査する。
絵文字は `is_color: true`、有色画素の存在、mesh の白い頂点色（texture の色を維持）を検査する。
Windows + `color_fonts` 限定。フォント未導入だけは理由付きで return し、それ以外の読取失敗は失敗とする。

修正前の同じ9件は exit 101、7失敗・2成功だった。
Gothic/Mincho 5 face は `A`、SimSun/NSimSun は `あ` のカラー誤判定で失敗した。
Meiryo とカラー絵文字は成功。修正後の lib 全件は104成功・失敗0・skip 0。
初回は MS UI Gothic / MS PGothic のテスト名と face 番号が逆だった（3 face 自体は全て実行）。
name table 照合後に対応だけを訂正し、修正前・修正後を再実行した。
修正前は registry 原本の別コピー `target/fontcolor-1443/before-epaint` に訂正済みテストだけを
追加して実行し、ビルド中の製品ソースから修正を取り外す操作はしていない。
元ログは `before-initial-labels.log` / `epaint-color-initial-labels.log` に保持する。

## 実行台帳（2026-10-25）

対象: 上記起点からの未コミット §1.443 差分。ログは `target/fontcolor-1443/`。
Rust 1.99.0 (`b940084d7`)、Cargo 1.99.0 (`5f94df478`)、Windows の開発機フォントを使用。

epaint コマンドは `cargo test --manifest-path <manifest> --lib --offline` が共通部分。
訂正後の before は `--target-dir target/repro/font-white-614-20261025/build`、
after と color_fonts 無効の対照は `--target-dir target` を使用した。

| コマンド | exit / 結果 | ログ |
| --- | --- | --- |
| `target/fontcolor-1443/before-epaint/Cargo.toml` の lib `--features default_fonts,color_fonts font_color_tests -- --nocapture`（修正前） | 101 / 7失敗、2成功 | `before.log` |
| `vendor/epaint/Cargo.toml` の lib `--features default_fonts,color_fonts -- --nocapture`（修正後） | 0 / 104成功 | `epaint-color.log` |
| `vendor/epaint/Cargo.toml` の lib `--features default_fonts`（color_fonts 無効） | 0 / 95成功 | `epaint-no-color.log` |
| 元の診断 crate を `--config "patch.crates-io.epaint.path='G:/home/mimageviewer-fontcolor/vendor/epaint'" --offline` で実行 | 0 / 全225行を検査。通常文字は非カラー・黒頂点、emoji はカラー・白頂点。旧 CSV から120行が修正された | `after.csv` / `probe.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core --target-dir target/fontcolor-1443/build` | 0 / 3分25秒、既存 warnings あり | `check-serial.log` |
| `cargo test -p mimageviewer --test ui_snapshot` | 0 / 195成功、実行28.23秒。更新モード無効。基準 PNG 456枚の SHA-256 はすべて不変、`.new/.diff/.old.png` なし | `ui-snapshot.log` / `snapshots-before.json` |
| `scripts/test-focused.ps1` | 2 / vendor・Cargo の広域変更。選択テストを実行しないという runner の判定 | `focused.log` |
| `scripts/test-focused.ps1 -Path @('src/ui_fonts.rs','src/ui_font_catalog.rs')` | 0 / 11 + 7 = 18成功。lib 全件は実行しない | `focused-fonts.log` / `target/test-focused/runs/20261025T132153666Z-50bc21a7` / `20261025T132559181Z-abe8b7dc` |
| `cargo fmt --check` / `cargo fmt --manifest-path vendor/epaint/Cargo.toml --check` | 0 / 両方成功 | 当セッション tool 出力 |
| `scripts/build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | 0 / core・remote・EPUB worker と実行用 DLL を配置。起動していない | `build-dev.log` / `target/vcrt-pe-reports/dev-runtime.json` |

初回 epaint standalone は未キャッシュの `similar-asserts` でコンパイル前に停止。
通常ネットワークも proxy / Schannel エラーで取得できなかったため、
`target/fontcolor-1443/cargo-home` へ index/cache を実ファイルで複製し、
不足 archive は crates.io から取得・原本 lock の SHA-256 と照合した。
以後の検証では process の `CARGO_HOME` をこの作業ツリー内へ向ける（個人設定は変更しない）。
epaint standalone は `--target-dir target --offline` を併用する。

原本 feature なしの lib は `unique_test_key` の cfg 不整合でコンパイルできなかった。
原因確認のため一時的にその cfg だけ外した実行も、フォントを要する原本4テストが失敗した
（73成功、4失敗）。その試行変更は戻し、最終ソースは原本どおり。
上流 lib suite と回帰9件を合わせる正式検証では `default_fonts,color_fonts` を有効にする。

face 名訂正後、before と同じ出力先で after を試した実行は再コンパイルされず、
修正前と同じ7件が失敗した（97成功、7失敗、exit 101、`epaint-color-stale-target.log`）。
同名 local crate の実行物の再利用と判断し、この実行を修正後の検証には採用していない。
出力先を分離した最終 after は `Compiling epaint ... vendor/epaint` を確認して104件成功した。
本体 check・snapshot・focused・dev-runtime はこの before 用出力先を使用していない。

本体 check は通常キャッシュ使用時の TurboJPEG CMake/MSBuild 失敗、その後のキャッシュ切替時の
既存 build directory 使用中エラーで停止した。新しい出力先でも並列 MSBuild が失敗した。
Ninja + MSVC 環境での試行も compiler probe に進捗がなく、担当が開始した CMake/Ninja だけを
明示中断した（`check-ninja.log`、cargo exit 101、検証成功に含めない）。
MSBuild の `--parallel 1 /nr:false` では同じ TurboJPEG のコンパイル・install が成功した
（`cmake-single.log`、exit 0）。最終本体検証は `CARGO_BUILD_JOBS=1`、
`MSBUILDDISABLENODEREUSE=1`、`--target-dir target/fontcolor-1443/build` を使う。
snapshot も同じ出力先・条件で実行し、FFmpeg DLL 6本をその `debug/deps` へ実ファイルで配置した。
本体利用側の focused 検査と確認用 build は Rust 側の並列を8に戻し、
`CMAKE=target/fontcolor-1443/cmake-serial.cmd` の一時 shim で CMake の `--build --parallel` だけを
1へ置換する。compiler・flags・source・generator は変えない。これらは `target` 内の
一時資材・process 環境に限り、製品の build script や個人設定には変更しない。

テストの face 名対応の訂正は `cfg(test)` の fixture だけで、製品ソース・snapshot 入力に影響しない。
本体 check、snapshot、利用側 focused の成功証拠はそのまま有効として再利用する。

`test-lib-full.ps1` は依頼どおり ClaudeCode が後で実行する。製品 exe の起動、実機の見た目、
master への統合、release はこの実装作業の検証には含まない。
実フォントのカラー保持の確認対象は Segoe UI Emoji。PNG/BGRA の別フォント実データと
非 Windows 環境での実行は未確認。

## 確認用 build の引き渡し

`target/dev-runtime/mimageviewer-core.exe` と companion を生成済み。エージェントは起動しない。
利用者が確認する場合はインストール版・tray 常駐版を終了してから、作業ツリーのルートで実行する。
single-instance mutex は共通で、既定では通常の `%APPDATA%/mimageviewer` を使う。
起動すると実際の設定・データを更新し得る。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

ライトテーマで該当7フォントの `A1 あ日` が通常の文字色で読め、
Meiryo とカラー絵文字 `😀` の色が保たれることが手動の確認対象。

## 独立レビュー後の修正 (2026-10-25)

- フォントが見つからないときに検査を飛ばして成功扱いにしていたのを、失敗にした (mIV は Windows 専用で、対象のフォントは Windows に同梱される)。
- epaint は workspace の外なので、`scripts/test-full.ps1` に `vendor/epaint` の lib テスト (`default_fonts,color_fonts`) を足し、全体テストで必ず走るようにした。

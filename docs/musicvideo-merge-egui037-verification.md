# musicvideo: master / egui 0.37 統合の検証（2026-10-31）

対象: HEAD `5517a4dd35052662cc0a9971a9d8761e479cef6c`、MERGE_HEAD
`961e4937256c42c1ce553f69011597df165b64b6` の未コミットマージ。
[brief](brief-musicvideo-merge-egui037.md) に従う実装担当の検証であり、独立レビュー・実機検収ではない。
コミット、アプリ／エディタGUI起動、実利用中のAPPDATAへのアクセスは行っていない。

## 競合とAPI追従

- `src/ui_music_spectrum.rs`: Bassマーカーの共有引数・共有描画を保持し、masterの
  `run_pass`と描画差分の明示破棄へテストを適合。HarmNet確率・選択済み絶対MIDIの
  型付き入力、worker・結果採用・描画経路も照合した。
- editor / UIのegui・eframeを0.37.0へ統一。Cargo.lockはmasterを起点にCargoで再生成。
- `App::ui`、root Uiと`Panel`、方向に依存しないsize API、`PanelState::outer_rect`、
  rootに属するpanel IDへ移行。`UiBuilder::scope_id(Id::unique(...))`で従来のglobal IDを維持。
- TextBuffer境界を`CharIndex`へ変更。内部の文字位置・行対応付けは維持。
  Preeditの構造体形式、ModifiersChanged、egui_wants_keyboard_inputへ追従。
  DragValueのprivateな編集状態がStringではなくなったため、既存Undo fixtureは
  内部cacheの直接読取りから、実際のProject値の復元検査へ変更した。
- IME DeleteSurroundingも歌詞の取消用スナップショット取得へ通す。
  実TextEditによる時刻付き行の結合と取消を検査する回帰を追加した。
- 0.37のTextEditはevent処理をwidget allocationより先に行うため、disabled Uiだけでは
  focus済み欄への入力が通った。単行5種と歌詞欄にUIの有効状態をinteractiveとして渡し、
  既存modal契約を維持する。6種のfocused欄へのdisabled入力回帰を追加。
  masterのvendor・共通IME helperは変更しない。
- rendererのないFullOutputは描画観測を残してtexture deltaを明示破棄。
  upload検査ではID別更新列を取り出してclearし、更新1件・画素・size・freeの検査を維持。
  初期の未処理deltaのDropによる中断を成功とは扱わない。
- release safety fixtureをmasterのepaint段追加へ適合。実cargo子の9段すべてについて
  順序・失敗終了・error mode継承を検査。製品のtest-full経路は変更しない。

## 環境と隔離

`MSBUILDDISABLENODEREUSE=1`を設定。再起動後の元のtarget/debugにはrmeta/rlib破損があり、
既存出力を残してtarget/merge-egui037へ再構築した。CMake/MSBuildの並列build失敗は
生成済みprojectの単一job build/installとCARGO_BUILD_JOBS=1で解消。
ネイティブ依存の準備後の通常Rust検証はjobs 8。

不足／旧版のignored vendor資産をC:/home/mimageviewer/vendorから実体コピーした。
libavif、LibRaw 0.22.3、PDFium 8125、ORT 1.24.4、EffeTune 0.15.0、CRT、VST3 host。
旧EffeTune bundleはtarget/native-premergeへ保持。junction/symlinkは作成していない。
VST3 hostの埋込みsource hashは現C++ sourceと一致。
テスト用APPDATAはtarget/merge-test-appdataへ隔離。FFmpeg／CRTはテストexeの隣にも配置。

## 自動検証

ログはtarget/配下。検証の重複分を合算した件数ではない。

| 検証 | 結果 | ログ |
| --- | --- | --- |
| core check（最終） | exit 0 | merge-core-check-final.log |
| music-core | 69 passed | merge-core-ui-tests.log |
| musicvideo-core | 115 unit + 2 integration passed | merge-core-ui-tests.log |
| musicvideo-ui | 128 passed（既存126 + 回帰2） | merge-ui-final3.log |
| musicvideo-editor | 181 passed（H3a / H3b等を含む） | merge-editor-final.log |
| build safety | dev / release / editor 3本PASS | merge-{dev,release,musicvideo}-safety.log |
| build出力先保護 | 88ケース等PASS | merge-build-output-safety.log |
| VST3 source identity | 一致 | merge-vst3-identity.log |
| context menu snapshot | 更新1件・通常比較1件PASS、PNG2枚を目視 | merge-context-{update,compare}.log |
| focused（5範囲） | 23 + 25 + 15 + 5 + 37 = 105 passed | merge-focused.log |
| 本体lib全件 | 15207 passed / 0 failed / 67 skipped、714.3秒 | merge-lib-full.log |
| cargo fmt / 最終fmt check | 適用済み / exit 0 | merge-fmt-final.log |
| 最終glyph | 危険glyph 0 | merge-glyph-final.log |
| git diff --check | exit 0 | merge-diff-check.log |
| 確認用build | exit 0、522.172秒、PE gate runtime=4 / pe=4 | merge-build-dev.log |
| 製品紹介カード | 50件、違反0 | 実行時tool出力 |
| manual sidebar | 33ページ各34リンク、順序・active・実在PASS | merge-site-check.json |

music-core、musicvideo-core、src/video/mvexportのソースはマージ前HEADから不変。
固定RGBAハッシュ回帰とH3b全件が成功し、検査対象では書き出しRGBAの変化・previewとの不一致なし。

## スナップショット

2枚とも620×700。1枚目は本文不変、scrollbarだけ102px変化（x604..611 / y413..427）。
2枚目は「音楽ビデオを作成」の1行追加と末尾へのスクロール位置により284135px変化。
更新前後4枚を目視し、文字の欠け・重なりがないことを検査した。
新PNGは168744 + 159376 = 328120 bytes。通常比較も成功。統計はmerge-snapshot-stats.json。
これはmasterの0.37画像を基準にしたメニュー項目追加であり、書き出しRGBAの変更ではない。

## 確認用buildと引き渡し

`build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0`を実行した。core / Remote /
EPUB変換器 / 音楽ビデオエディタはすべて現在のソースから生成し、起動していない。
通常featureで、portableは有効にしていない。CARGO_TARGET_DIRはこのbuildだけ解除して
正規のtarget/dev-runtimeへ出力した。CMakeだけを1並列にする検証用の転送exe
`target/merge-cmake-serial.exe`（同名csがソース）をCMAKEへ指定し、Rustはjobs 8を維持。
製品のbuild script・個人設定は変更しない。

| 実行ファイル | bytes |
| --- | ---: |
| mimageviewer-core.exe | 339489792 |
| mimageviewer-remote.exe | 14835712 |
| mimageviewer-epub-pdf.exe | 5094400 |
| mimageviewer-musicvideo.exe | 35530240 |

build実行時のAPPDATA隔離は検証プロセスだけの環境指定であり、生成したexeの既定profileを
変更するものではない。利用者が通常環境で起動する場合は、既定で実利用中の
`%APPDATA%mimageviewer`を使い、設定・データを更新し得る。
インストール版／トレイ常駐版を先に終了する（single-instance mutexを共有）。

```powershell
Start-Process -FilePath .	argetdev-runtimemimageviewer-core.exe
```

隔離が必要な場合は同じexeへ`--data-dir .	argetdev-runtimedata`を渡す。
任意の利用者実機確認は、音声の右クリック「音楽ビデオを作成」からの起動と、
文字欄にfocusしたまま確認画面を開いた際に背後の文字が変わらないこと。
今回のGUI検収は依頼どおり未実施。

## 証跡とGit状態

音楽関連はmusic-core 69 + musicvideo-core 117 + UI 128 + editor 181 = 495 passed。
focused 105件は本体lib全件と重複するので、合算して独立件数とは扱わない。
本体lib全件は指定のscriptで1回実行した。既存67 ignoredを維持し、失敗0。
成功台帳は以下。全件終了後の変更は検証結果の文書追記のみで、コードの再検証は重ねない。

- 台帳: `target/test-lib-full/44e89d385aa414f5d4f38c6f6d7f9491f2f06e7bd064c7dfc21a1d51019d5433.json`
- 全件のsource ID: `0bd5555b2dddc5a793763244132356b08f86226e2779ff180f7fbd2dc26f8aa9`
- JUnit / stdout / stderr: `target/test-lib-full/runs/20261031T045402916Z-1ad9d12f/`
- 最終コード・manifest・scriptのSHA-256: `target/merge-code-hashes.json`
- 最終実行のcommand / exit / 秒 / 条件: `target/merge-*.json`と対応log。

Git管理領域へは書いていない。ファイル上の衝突マーカーは解消したが、
`src/ui_music_spectrum.rs`のindexは設計担当によるstage待ちのまま。
MERGE_HEADは961e49372のままで、コミットしていない。元からのmaster取り込み差分を保持する。

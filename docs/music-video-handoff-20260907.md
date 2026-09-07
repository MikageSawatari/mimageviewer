# 音楽ビデオ開発の引き継ぎ監査（2026-09-07）

この文書の §1〜§6 は監査時点の記録、§7 は v3.6.0 取り込みと検証の記録である。
その後のユーザー承認による修正は [編集整合性の実装指示・検証記録](brief-musicvideo-edit-integrity.md)
を参照。保存・選択は `4ba3b7932`、素材履歴基盤は `91326ca8d`、素材復元・読込中の編集保護は
`0c24a453e` で修正。歌詞の行対応も実装・独立レビュー・全体テストが完了し、確認用ビルドを準備中。
下記の監査当時の「未修正」を現在の状態と混同しない。

## 1. 対象と体制

- 作業場所: `C:\home\mimageviewer-musicvideo`、ブランチ `musicvideo`。
- 監査対象 HEAD: `4f41cce35cbfdfae156c8ddeb7b59f2b4ed77d20`。
- 要件・設計の正本: [music-video-export-plan.md](music-video-export-plan.md)。過去の P1a〜P3a brief は各段階の指示・実測記録として読む。
- 依頼範囲は引き継ぎと現状整理。今回 P4 や発見した不具合の実装には着手していない。
- ユーザー指定: 主担当 Astra / 高が要件整理・調査・設計判断・進行管理。通常の実装とテストは Sol / xhigh、まとまった変更の独立レビューは Astra / 高。
- 今回は Sol がコード照合・テスト、独立 Astra が設計監査、主担当が統合判断と文書を担当。コード編集者は置かず、文書は主担当のみ編集。
- 次の実装依頼では、主要前提をコードで先に検証し、矛盾があれば根拠とともに設計担当へ戻す。レビューは指示適合に加え、設計自体の妥当性も対象にする。

開始時点では追跡済みの未コミット差分なし。未追跡 `out.mp4` は既存素材として保持し、今回の出力先にしない。別 worktree の変更・index・ブランチは変更しない。

## 2. 実装済み・未完了・未検証

| 項目 | コード照合結果 | 留保 |
| --- | --- | --- |
| 音声＋画像から MP4 | `src/video/mvexport/{driver,mux}.rs` と `src/bin/mvexport_cli.rs` に実装あり。既存 H.264/AAC エンコーダを再利用 | ラボ GUI は連番 PNG。CLI は既定 Project を生成し、編集済み `.mivmv.json` の入力オプションを持たない。編集→MP4 の製品導線は未接続 |
| シーン・CPU 描画・グロー・グラデーション | `musicvideo-core/{model,scene,raster,assets,text}.rs`。プレビューと書き出しで描画器を共有 | 動的解析の入力時刻列まで同一とは限らない（§3） |
| キーフレーム・タイムライン | `musicvideo-ui/{keyframes,timeline}.rs`、`musicvideo-core/anim.rs` | 実曲での操作感はユーザー未検収 |
| 鍵盤・mIV 横長スペクトラム | `music-core/{keyboard,color,spectrum_visual}.rs` を本体 `src/ui_music_spectrum.rs` と書き出し側から利用 | 共有変更なので本体音楽ビューの退行確認が必要 |
| 帯域エネルギー・onset 変調 | `music-core/analysis.rs` の範囲解析、`musicvideo-core/spectrum.rs` の較正と状態更新 | duration が音源長と違う場合の較正入力に不一致あり（§3） |
| レイヤー管理・歌詞 | `4fe641672`、`f908d0ff6`。歌詞モデル・時刻境界・LRC は `musicvideo-core/lyrics.rs`、UI とタップは共有 editor、I/O は lab worker | 歌詞専用 Undo は廃止され、共通履歴へ接続済み |
| Undo/Redo | `4f41cce35`。Project＋レイヤー/キー/歌詞選択の snapshot、128 段、ドラッグ/フォーカス単位、load 時 clear、Redo 破棄、shortcut 抑止 | 共有 UI の状態機構はあるが、ホストの素材差し替え・資源復元は未完。P3j 全体を完了とは扱わない |
| プロジェクト保存・読込 | `tools/musicvideo_lab/src/project_file.rs`。version 検査、相対パス、同一ディレクトリの一時ファイルから保存、欠落素材の通知 | まだ lab 所有。保存完了と現在プロジェクトの対応付けに問題あり（§3） |
| P4 本体統合 | 未実装。root は `musicvideo-core` に依存するが `musicvideo-ui` の接続はない | 独立編集モード、ライフサイクル、入力、保存、MP4、フォントなどの契約が必要 |
| ビート同期 | 設計 D6a に初回リリース対象外のユーザー判断あり | 未実装バグと混同しない |

## 3. P4 の前に扱う事項

以下は現行ソースの成立経路を確認した指摘。実機で発症を再現したという意味ではない。今回の監査で機能制限や一時的な回避策を追加していない。

### H1 — 保存完了が別プロジェクトの保存先を変更する（P1）

`ProjectSavePending` は結果 receiver だけを持つ（lab `app.rs:162`）。「プロジェクトを開く」の有効条件（同 `1265`）には保存中の判定がなく、A の保存中に B を開ける。B の読み込みは `poll_projects` で Project/path を交換する（`881`）。その後 A の保存が完了すると、同関数が結果の path を現在の `project_path` へ無条件に代入する（`930`）。次の通常保存はこの path を使う（`1290`）。

結果として、B を開いた後の保存先が A に戻り、次の保存で B を A に書き込む経路がある。保存先へ atomic に書く仕組みだけでは防げない。保存/読込要求と完了を同じプロジェクトの identity・revision に所属させ、成功・失敗・切替・終了まで一つの owner で扱う設計が必要。順序を制御した handler テストで固定する。

### H2 — Undo の Project と素材・音声の実体が一致しない（P1）

lab `app.rs:490` 以降の `poll_loads` は音声 path/duration、画像参照を直接変更し、同時に PCM/player/waveform/Assets を更新するが、共有履歴の checkpoint を通らない。一方 `musicvideo-ui/src/lib.rs:170` の復元は Project と選択までで、ホストは描画再要求だけを行う。

例: 画像 A のプロジェクトでプロパティ編集→同じ ImageId の背景を B に差し替え→Undo。snapshot の参照は A へ戻るが、Assets は B の画素を保持する。音声差し替えでも参照・duration と PCM/player が不一致になり得る。追加画像の読み込み中に Undo する経路も、完了が参照を再追加するため同じ所有境界で検査する。

全編集入口と非同期完了を履歴の対象として統一し、Project 復元に対応する資源の解決・世代・取消をホストが所有する必要がある。単に履歴を消す、Undo を無効にする、追加の bool で枝を増やす修正は採らない。画像/音声の差し替え・削除・復活・Undo/Redo・読込失敗・遅延完了の回帰テストを先に設計する。

### H3 — 同じ描画器でも、プレビューと MP4 の解析結果の一致は未成立（P2）

- ラボは実時間の音声位置で preview を要求する（`app.rs:1242`、`start_preview_if_needed`）。MP4 は固定 `n/fps`。`SpectrumSource` の Average は呼出しごとの EMA（`spectrum.rs:293`、`554`）、onset は隣接入力差分を使う。D3 の「時刻 t のみに依存」と履歴依存の処理は、そのままでは同じ契約ではない。P3a brief の「連続再生中は一致」も強すぎる。
- 自動較正は lab では元の全 PCM（`app.rs:348`）、MP4 では duration に切詰め/無音延長した後の PCM（`driver.rs:65`、`107`）を走査する。音源長と出力尺が違うと floor/ceiling が変わり得る。

同一書き出しを二度行う決定性と、可変時刻の preview が export と一致することを分けて検証する。正規の音声入力・較正区間・解析時刻列・seek 復帰の契約を決める必要がある。既存の preview/出力一致要件を、この監査だけで緩和したことにはしない。

## 4. P4 の設計作業と共有機構への影響

`musicvideo-ui` は egui と core の編集部品を共有する。`MusicVideoLabApp`（`app.rs:193`）には、それとは別に Project、PCM、cpal、素材、波形、解析・較正・プレビュー、ファイル I/O、複数の非同期要求が残る。「パネルを載せれば終了」という規模ではない。

着手順は次を提案する。これは実装承認や P4 完了宣言ではない。

1. H1/H2 の要求・履歴・資源所有を設計し、Sol が前提を検証してから修正。独立 Astra レビューを通す。
2. H3 の解析契約を決め、較正入力差と可変時刻列をテストする。既存の無変調ハッシュも維持する。
3. P4 の小インクリメントを正本へ記載する。編集セッションの enter/open/switch/save/export/cancel/error/close と dirty 確認、再生の正本、フォント資源、キー所有を定義する。
4. ホストに依存しない保存形式/状態遷移は共有し、lab と本体で同じ実装を利用する。FFmpeg/再生デバイス/ウィンドウ依存処理は明示したホスト境界へ置く。既存共有 UI を再実装しない。
5. 本体導線と MP4 worker を接続し、閲覧・音楽・remote・fullscreen/detached に必要な退行確認を行う。

書き出し snapshot は Project だけでなく画像・フォント・音声の内容を確定する単位として設計する。
現 MP4 API は `audio_path` を後からデコードするため、preview で開いた後に素材ファイルが変わった場合の
扱いも必要。再生についても lab の `AudioPlayer::new`（`playback.rs:35`）は UI poll から
デバイス照会・stream 生成を行い、既定 f32 出力のみを受け付ける。本体へそのまま移植して
UI 応答性・出力デバイス互換性の確認を済ませたとは扱わない。

| 影響先 | 既存変更または今後の統合方法 |
| --- | --- |
| `music-core` / `src/ui_music_spectrum.rs` | 既に解析範囲 API と色・鍵盤・横長描画判断を共有化。今後も音楽ビューと export の両呼出し側を確認 |
| `src/video/stream` / `src/audio_decode.rs` | 既存 encoder と decoder を再利用。MP4 出口は `mvexport` に分離。配信側へ独自設定を流し込まない |
| `Cargo.toml` / `Cargo.lock` | 本体への UI 依存追加と共有ホスト構成を設計後に統合。別系統の workspace/dependency 追加を保持 |
| keymap / IME / keyboard owner | lab の固定 Ctrl+Z 等を無条件に移植しない。本体の `KeyAction`・scope・TextInput/IME ownership に接続し、文字入力と二重消費しない |
| App / 再生 / remote / viewport | 閲覧状態に編集用 pending を散在させない。編集セッションが自分の資源だけを取消・drop する。既存再生との関係は前提検証が必要 |
| `.mivmv.json` / Settings DB | Project は独立 JSON のまま。本体の実設定をテスト素材に使わない。保存・読込・export は同じ session snapshot を基準にする |

監査時の master は `ea3da05ae`、merge-base は `973c9f3a3`。master のみ 120、musicvideo のみ 30 コミット。ブランチ変更は 45 ファイル。列挙した共有箇所のうち、merge-base 以降 master にも変更があったのは `Cargo.toml` / `Cargo.lock`。これは自動 merge 安全性の保証ではない。統合直前に差分と他担当の編集予定を再確認し、一つの担当だけが共有ファイルを編集する。今回 master への merge は行っていない。

## 5. 検証記録の扱い

Sol / xhigh が現 HEAD で再実行（2026-09-07）。すべて正常終了し、timeout による中断なし。

| コマンド | 結果 |
| --- | --- |
| `cargo test -p musicvideo-core` | 71 passed / 0 failed（テスト実行 1.90 秒） |
| `cargo test -p musicvideo-ui` | 37 passed / 0 failed（テスト実行 0.02 秒） |
| `cargo test -p musicvideo_lab` | 24 passed / 0 failed（テスト実行 3.23 秒） |
| `cargo fmt --check` | exit 0 |

独立 Astra / 高が設計と本監査書をレビューし、H1/H2/H3 の経路と優先度を確認。
旧検収手順の表現、解析の根拠位置、デバイス初期化の扱いに関する文書指摘を反映した。

過去報告と現物から確認できた範囲:

- 歌詞 `f908d0ff6` と Undo `4f41cce35` は現 HEAD の履歴に存在する。
- 過去ログのテスト数は core 71 / UI 37 / lab 24。UI は Undo で 25→37 と報告されている。型・モデルのテストが通ることと H1/H2 のホスト動作の正しさは別。
- `077ad307e7676c3b` は過去会話の無変調フレームハッシュ報告。今回そのコマンド・入力セット・完全な RGBA ハッシュを再取得していないため、再検証済みとは記さない。
- `target/lyrics-baseline-before.mp4`、`lyrics-baseline.mp4`、`mvundo_hash_20260905.mp4`、`mvundo_hash_repeat_20260905.mp4` が存在する。MP4 ファイルの hash とエンコーダ入力 RGBA の hash は別であり、ファイルの存在や長さだけでは同一性を証明できない。
- 上記 4 ファイルは今回読み取りで SHA-256 を比較し、すべて `946D51C5520E506327DA23F293E1018988A7FF517455560E497DD34375EF4811`、2,199,978 bytes で一致した。既存成果物間の一致を確認したもので、現ソースでの新規書き出し・内部 RGBA ハッシュの再取得ではない。
- 128 段 0.79 MiB は Undo コミット本文に記録あり。JSON 換算約 440 KB は過去会話の報告。測定 harness と入力を今回確定しておらず、新たな実測値として扱わない。
- `target/release/musicvideo_lab.exe` は存在し、更新時刻は 2026-09-05 19:57:37。過去ログの崩れた `musicvideo\_lab.exe` ではなくこのパス。今回再ビルド・起動はしていないため、バイナリとソースの一致までは保証しない。

未検証として残すもの: 実曲を一本作る操作感、Windows の実音出力・seek・IME、編集済みプロジェクトの MP4 完走、最新機能入りの 5 分/1080p60 性能、共有変更後の本体音楽ビューの目視、全体ゲート、H1/H2 の順序を制御した回帰テスト。過去の P1/P2 性能測定は当時のシーン・環境の記録として有効だが、最新の全機能入りシーンを保証しない。

今回の変更は文書のみのため、`build-dev.ps1` の検証バイナリ作成対象外。今後ランタイムを修正したら、関連テスト・fmt・必要なら glyph 検査を通して、通常 core をビルドしユーザーへ起動手順を渡す。通常 `%APPDATA%\mimageviewer` を使い実データを更新し得る点と、installed/tray 版の事前終了を明記する。エージェント自身は通常版を起動せず、UI 自動検証は `prepare-portable-smoke.ps1` で用意した disposable portable のみ使う。release/launcher/bridge 等の変更では規定どおり release build の検証へ切り替える。

## 6. 文書の不整合と旧担当指定の移行

| 箇所 | 不整合・旧指定 | 今回の扱い |
| --- | --- | --- |
| 正本冒頭 | 着手前/ユーザー承認前、実装 Codex Sol / レビュー ClaudeCode | 現実装状態とユーザー指定の Astra/Sol 体制へ更新 |
| 正本 P3a | `5302f1155`、2026-09-05 | 同題の git object はあるが現 HEAD の祖先ではない。現ブランチの `6ca5f3091`、2026-09-04 に訂正 |
| 正本 P3j | 完了、IME も含めた強い説明 | 共有 UI 機構は実装済み、ホスト復元未完・実 IME 未検収へ訂正 |
| 正本 §7 の「実機確認 → レビュー → コミット」 | 現履歴にはコミット済みだがユーザー実機未確認の段階がある | コミットの存在を実機検収済みの証拠にしない。次の指示書では実装・自動テスト・独立レビュー・ユーザー実機の結果を別々に記録し、AGENTS.md の機能別検証/build/commit 要件を維持 |
| 正本 D2 / §4 / §6 | GPU 依存の記述、Lyrics/MivHorizontal 等を欠く旧概念モデル、別スレッド有界リングを必須とする記述 | 現行は CPU、最新モデルは `model.rs`、MP4 は逐次 loop＋演算内並列。P1b brief §1.1 の測定先行判断と食い違うため、次の設計更新で本文を整合させる |
| 正本 Q1/Q7/Q8 | 古い期限が残る | Q1 は末尾に現行方針追記あり（複数レイヤ＋opacity、slides helper は実使用後）。Q7 は lab の既定フォント自動選択までで選択 UI は未実装。Q8 は D5 の将来オプションと区別 |
| P1b/P2a/P2b/P3a brief の「commit は不要（こちらで行う）」 | ClaudeCode が統括・コミットする時点の指示 | 過去作業の記録として保持。今後の担当は主担当が明示し、実装担当による他担当の変更の巻込みを禁止 |
| `CLAUDE.md`「Codex CLI レビュー」 | ClaudeCode が Codex CLI を起動する手順と旧モデル指定 | このタスクはユーザー明示の独立 Astra サブエージェントによるレビューへ移行。指摘往復と根拠記録は維持 |
| `AGENTS.md` / `CLAUDE.md` / detached plan §2・§3 | ClaudeCode と Codex の双方合意、ClaudeCode への相談/検収、完了条件と diff の照合を中心に記述した検収手順 | 担当名は移行対象。実際に触れる前に主担当設計判断＋独立 Astra の構造レビューへ対応付け、§11 に範囲・合意理由を残す。ユーザー指示に従い設計誤りもレビューする。症状パッチ禁止・別系統との非同時編集・既存テスト維持は変更しない |

旧担当名の全リポジトリ一括置換は行わない。他系統の運用まで書き換えず、このタスクの新しい実装指示書で移行範囲を明記する。

## 7. v3.6.0 の取り込み（2026-09-07）

引き継ぎ後のユーザー依頼により、master の v3.6.0 リリース地点
`56d97632ce3eb20bb3cec0b61016a2b18b2b1dc4` を musicvideo へ取り込んだ（マージコミット `17a9664d1`）。
引き継ぎ文書は先に `857d6db05` で記録した。取り込み時の master は `843fa4cc8` だが、
タグより後の 5 コミットは今回の対象に含めない。§4 の分岐数は取り込み前の監査記録として残す。

自動マージは衝突なし。重複した `Cargo.toml`、`Cargo.lock`、`src/video/mod.rs`、`docs/README.md` は
双方の追加を保持している。独立 Astra / 高のレビューで、musicvideo の実装・共有 music-core・
ui_music_spectrum は取り込み前と同一、encoder / audio_decode / comic-core はタグと同一であることを確認。
新たな P1/P2 の統合問題は見つからなかった。§3 の既存 H1/H2/H3 はこのマージでは修正しない。

`git diff --cached --check` はタグ由来の `docs/video-zoom-pan-plan.md` の 2 行に末尾スペースを報告する。
Markdown の改行用スペースであり、リリース内容を保つため変更しない。

統合後の検証結果は以下へ記録する。

| 検証 | 結果 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 成功 |
| `cargo check -p mimageviewer --features dev-tools --bin mvexport_cli` | 成功 |
| `cargo test -p musicvideo-core` / `-p musicvideo-ui` / `-p musicvideo_lab` | 71 / 37 / 24 件、全件成功 |
| `cargo fmt --check` | 成功 |
| `python scripts/check_ui_glyphs.py` | dangerous glyphs なし |
| `cargo build --release --bin mimageviewer-core` | 成功（14 分 16 秒） |
| `cargo build --release -p mimageviewer-remote --bin mimageviewer-remote --features embedded-web-assets` | 成功（54.30 秒） |
| 全体ゲート再実行・workspace 段 | 成功。本体 lib 7,498 passed / 0 failed / 33 ignored、Susie 8 passed、UI snapshot 45 passed、workspace / doctest 全体も失敗なし |
| `.\scripts\test-full.ps1` 最終結果 | exit 0 / `[test-full] PASS`。上記に加え vendored egui-wgpu 9 passed、eframe 15 passed |
| テスト修正後の `cargo fmt --check` | 成功 |
| `.\scripts\build-dev.ps1` | exit 0 / `[build-dev] DONE`。normal feature set の core / remote を生成、未起動 |

全体テスト初回は launcher build.rs が必要とする release core / remote が未生成のため、
テスト開始前に終了した。これは CLAUDE.md「開発中のビルド・テスト選択」に記載された前提不足であり、
テスト失敗とは分けて記録する。指定の release core / remote の生成は成功し、全体テストを再実行した。

前提を揃えた後の全体テストでは、本体 lib が 7,497 成功 / 1 失敗 / 33 ignored、Susie integration が
5 成功 / 3 失敗だった。lib の失敗は `every_pass2_read_is_preceded_by_a_cancel_check` が
CRLF の `include_str!` 入力から LF の関数末尾区切りを見つけられないためで、取消の assertion より前に
停止していた。対象ファイルは v3.6.0 と同一。独立 Astra が CRLF / LF の検索差と正規化後の抽出を確認し、
テスト入力だけを正規化する局所修正に合意した。5 種類の読み取りの存在・取消観測順序の検査は維持する。
製品コードやリポジトリ全体の改行設定は変更しない。この修正はリリース取り込みと別コミットに分ける。
改行修正後、当該局所テストは 1 件成功。修正差分は独立 Astra によるレビューでも指摘なし。
Susie はローカル `testdata` が存在するため共有 testdata への fallback が使われず、ローカルの
`susie-plugins/extracted` に README しかないことが原因だった。共有 testdata の `ifpi.spi` / `ifmag.spi` を
非 reparse と確認して実コピーし、コピー前後の SHA-256 一致を確認した。元の testdata・ユーザー設定は変更していない。
`cargo test -p mimageviewer --test susie_integration -- --test-threads=1` は 8 件成功 / ignored なし。
別 target の不要な再コンパイルを避けるため、途中の明示 `--target` 付きコマンドは意図的に中断し、
全体テストと同じ既定の Windows x64 target で上記検証を行った。

上記最終検証は、マージ内容にこのテスト修正を加えた作業ツリーに対して行った。
確認用成果物は `target/dev-runtime/mimageviewer-core.exe`（308,096,512 bytes）と
`mimageviewer-remote.exe`（12,169,216 bytes）。エージェントは起動していない。
ユーザーが確認する場合は installed/tray 常駐版を終了してから、次を実行する。
通常の `%APPDATA%\mimageviewer` を使用し、実設定・データを更新し得る。
確認対象は本体音楽ビューの鍵盤・横長スペクトラム表示と、音声の再生/seek。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

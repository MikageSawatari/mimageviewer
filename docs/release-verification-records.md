# リリース前確認の記録

各リリースで実際に取った perf smoke / idle health / bench / 依存確認の結果を、版ごとに
残す。**次に何かが遅くなったときの比較対象**がここにある。手順そのものは
`CLAUDE.md` の「リリース手順チェックリスト」と [release-operations.md](release-operations.md) が正本で、
このファイルは測定値と、そのとき気づいたことだけを持つ。

[next-release-backlog.md](next-release-backlog.md) から分離した (2026-08-16)。バックログは
未着手の作業候補だけを置く場所で、済んだ測定の記録が混ざると残件が読み取りにくくなるため。

新しい版の記録は**このファイルの先頭側 (新しい順)** に足す。

---

## v5.5.1 (2026-11-22)

- **対象**: §1.836 だけ (一覧からフォルダツリーへのドラッグで、移動のときも「＋ コピー」と表示される件。v5.5.0 の §1.464 の不具合)。配布ビルドの入力・タグ v5.5.1・main はどれも master `aa6f06685`。利用者決定 (2026-11-21): v5.5.1 は §1.836 だけ。更新履歴は利用者承認済み (短縮版なし)。既知の問題からツリーのドラッグの節を外した。
- **CI 相当**: push 後の CI (main `aa6f06685`) は成功。
- **自動テスト**: `aa6f06685` の lib 全件 19,381 件成功、test-script の lib 全件 19,537 件成功。配布ビルドの全体テストは 3 回目で 21,505 件成功。2 回目は `app::tests::still_window_mode_key_tests::remote_acquire_cancels_archive_open_before_direct_or_converted_completion` と `remote_acquire_cancels_main_epub_modal_before_published_completion` の 2 件が失敗した。同じソースの lib 全件 2 本では通っているので、負荷で落ちるテストと判断して流し直した (§1.847)。1 回目は、別の worktree の dev core が起動中だったため、開始前に止まった。
- **署名・成果物**: 3 回目の `build-dist.ps1 -SkipVst3Bridge` は、ポータブル版の署名で `No certificates were found` になり止まった (SimplySign のセッション切れの見込み、未確認)。利用者が SimplySign にログインし直した後、同じソースで `-SkipRustTests` を付けて署名とパッケージだけをやり直した (09:46 完了、全 PE / VC runtime gate 通過)。単体exe・setup.exe・ポータブル版の mimageviewer.exe を `signtool verify /pa /v` で確認した (Open Source Developer Taku Sano、RFC3161 timestamp あり)。sha256: 単体exe `3b093217c71f496b8b894f3f065cb157eb680fa88cc9fd70b1ccd02e74f2f607` (609,352,280 B)・setup.exe `5bad37b45ccf45bba5b1a0d998f82c9e71dda1a8c360f5532b1d71dc14829cc0` (273,499,168 B)・Vector zip `2191f55d164c1e3f08067c3e4cbea0644466279a6963c311ef107f7ccfa4c91c`・ポータブル zip `21d74db11c66632e01ef56bad88f68e9d99ccc29f9a7c619862c86aeed3d9b0d` (325,018,408 B)。
- **VirusTotal**: 省略。VST3 host と mIV Sound は v5.4.0 から変更なし (再署名のみ)。
- **ReleaseChecks (サブ PC 担当がターミナル パネルから実行、利用者了承 2026-11-21 22:33)**: 1 回目はキット `relcheck-c54ecfd7e-20261121-171651` (manifest SHA-256 `7a42bd3b22877d1d75b0e0f8af8a96d98cb02cdbbb998438b687ccf5f79136c6`)。診断 artifact = `aa6f06685` + `30dbcfc43` (`c54ecfd7e`)。違いは test-script feature の中の観測と台本、それに製品の完了条件の関数への切り出し (挙動不変、独立レビューで同値と確認)。配布物には切り出しを含まない。run `release-checks-20261121-220921-45d79cbd` は **SKIPPED** (2026-11-22 07:09〜07:12、観測者はサブ PC 担当。戻った結果はメイン PC の照合でも SKIPPED)。seed-profile・seed-perf-cached は PASS。static-foreground の実要求が 256 px で、前提の 374±8 にならず環境不成立、後の 6 ケースは未実行。原因はサブ PC が 96 DPI (100%) で、374 px は 150% DPI の窓 1328×900・7 列を前提にした値だったため (試験の作りの問題で、製品の不具合ではない)。static のケースの窓・列数を DPI の表から選ぶよう試験側を直した (`ba843eaec` + `8f580f973`、artifact は `c54ecfd7e` のまま)。2 回目はキット `relcheck-c54ecfd7e-20261121-225440` (manifest SHA-256 `bd459b91b993811938f7e97a6894909308e6ed64fd9c567d65fe190b0ef38c3f`、利用者了承 2026-11-22 09:20 頃「ReleaseCheckをサブPCで実行をお願いします。」)。事前確認は READY、run `release-checks-20261122-002308-42a02bd8` は **FAIL** (2026-11-22 09:23〜09:24、観測者はサブ PC 担当。メイン PC の照合でも FAIL)。seed-profile の台本が、検索の前に mIV の窓の前面を 20 秒取り戻せずに止まり、7 ケースは未実行。前面を取った窓は記録に残っていない。前面が取れないことは設計上は環境不成立 (SKIPPED) だが、台本から環境不成立を出す手段が無く FAIL として数えられた (試験側の問題で、製品の不具合ではない)。表の修正は効いていた (seed の static の要求が 373 px)。 前面の記録と分類を足した (`9a7b711b4`、artifact を `99c0eea48` に作り直し)。3 回目はキット `relcheck-99c0eea48-20261122-014708` (manifest SHA-256 `ef841d872b9a720065bb3331027428cbc7b62468c7dd9ece09001d986f74d22c`、利用者了承 2026-11-22 10:51 頃)。事前確認は READY、run `release-checks-20261122-023742-299451f6` は **FAIL** (観測者はサブ PC 担当。メイン PC の照合でも FAIL)。seed の 2 つと static-foreground は PASS。static-background が待機の入口で止まり、後の 5 ケースは未実行。前面の記録では、前面は mIV 自身の別の窓 (winit の内部の窓) に移っていて、他のプロセスではなかった。原因は、試験側が root の窓を Process.MainWindowHandle で取り違え、winit の内部の窓へ前面化を要求していたこと (推定)。mIV が知らせる root の窓だけを使うよう直した (`3f60030ce`、artifact を `fd6f3abf4` に作り直し)。4 回目はキット `relcheck-fd6f3abf4-20261122-035048` (manifest SHA-256 `65367d1cb1edadfd7ad4c03e6cd0a7351f27542b0e72093dbd67c6a3232c893d`、利用者了承 2026-11-22 12:57 頃「サブPCは今何かのテストを進めているようです。空き次第ReleaseCheckも流して問題ありません。」)。事前確認は READY、run `release-checks-20261122-042738-1a29230b` は **PASS_WITH_WARNINGS** (2026-11-22 13:28 頃、観測者はサブ PC 担当。メイン PC の照合でも PASS_WITH_WARNINGS / exit 0)。9 ケースとも PASS、perf の 2 つだけが PASS_WITH_WARNINGS。警告は、起動の最初の frame の gap (461・451 ms、上限 500 ms、利用者了承 2026-10-11) と、drawn <16 ms の割合 0% (p95 32〜33 ms。RDP の描画の間隔によると見ている、未確認) で、どちらも判定を変えない WARN。前面の記録では、どのケースでも内部の窓が前面になった行は無い。v5.5.1 の公開は ReleaseChecks を待たずに進める (利用者の決定)。サブ PC (RTX 4060 Ti、1920×1080、RDP) の数値は、サブ PC の記録どうしでだけ比べ、メイン PC の記録とは比べない。
- **サブ PC の確認 (観測者はサブ PC 担当、2026-11-21 22:11〜22:19、v5.5.0 + `833e7c7b5`)**: 移動は「→ (フォルダ名) へ移動」、Ctrl では「＋ (フォルダ名) へコピー」と表示され、表示と結果は全回で一致した。落とせない行は禁止の印。一覧の空白が「＋ コピー」のままになる件は §1.844 (v5.6.0)。
- **実機確認 (利用者、インストール版)**: インストーラでの確認を依頼した後、利用者から「公開へ進める」の返事 (2026-11-22)。
- **公開 (2026-11-22 09:56)**: GitHub Release v5.5.1 (latest)、資産 4 点のサイズが手元と一致。`releases/latest/download/` の 3 点は 200 で、サイズも一致。mikage.to へ 3 ファイルを反映 (製品ページ・更新履歴・既知の問題。サーバ側 SHA-256 一致、verify-site 405/406 = `.htaccess` の 403 のみ)。
- **Vector・MS Store**: v5.5.1 も申請しない (利用者決定 2026-11-22「今回も見送る」)。

## v5.5.0 (2026-11-21)

- **対象**: 配布ビルドの入力は master `e0506e420` (製品コードは Ctrl+S / Ctrl+T の修正を取り込んだ `120ff61fb` と同じ、以降は版番号・文書・リリース用スクリプトの自己検査のみ)。タグ v5.5.0・main は `36d603f3e` (入力との差は更新履歴の文言・既知の問題・サイトマップだけ)。v5.4.0 以降の §1.816・§1.476・§1.462・§1.740・§1.464 (+§1.821)・§1.762・§1.822・§1.823〜1.826・§1.827 候補 F・§1.830、Ctrl+S / Ctrl+T の入力不能の修正 (ブランチ favsearch-typing-investigation)、テスト基盤の §1.817・§1.814 (g)。§1.818 は見送り (次版)。利用者決定で予定 (11-23) を前倒し。更新履歴は利用者承認済み (7,641 バイト、短縮版なし)。「重要な変更点」に Ctrl+T の欄を空にしたときの動きの変更を必読、長さの並べ替え・ツリーへのドラッグを目玉で追加。
- **依存物**: `check-dependency-updates.ps1` で PDFium・FFmpeg・LibRaw・ONNX Runtime・Rust・egui は最新。普通の crate 7 個と EffeTune Mixwright v0.19.0 は今回は上げない。
- **CI 相当**: `cargo fmt --check` 成功。push 後の CI (main `36d603f3e`) は成功。
- **自動テスト**: master `575633638` の lib 全件 19,324 件成功、test-script の lib 全件 19,480 件成功。配布ビルドの全体テスト 21,448 件成功 (1 回目は mIV のインストール版が起動中で開始前に停止、2 回目は全体テストの後に `test-release-build-safety.ps1` が 78681129f 由来の偽 cargo の不足で失敗し `febf32a8f` / `e0506e420` で修正 (Codex 実装、Sol xhigh レビューの P2 を対応、もう 1 件の P2 は配布に使わない計測用の検査で次版)、3 回目で全段通過)。
- **ReleaseChecks (9.6 / 9.7)**: **公開前には実施していない** (統括の手順漏れ。公開後に判明)。公開後に流し始めたが、ビルドの後に計測が急に始まる形を避けたいという利用者の判断で、ビルドの途中で止めた (計測は未開始)。利用者決定 (2026-11-21): ReleaseChecks をサブ PC で流せるようにしてから、次回から公開前にサブ PC で行う。公開後にサブ PC で v5.5.0 のキットを 3 回試みたが、試験側の問題で本番が成立しなかった (1 回目: 試験用ビルドが証拠フォルダの場所をビルド時の root で確かめるため、キットの置き場所が合わず mIV の起動前に停止。2 回目: 応答しない「設定」アプリの窓が前面に残り、事前確認が NOT_READY。3 回目: 事前確認は READY、本番の seed の検査が「seed search expected hit set not proven」で SKIPPED。台本が検索の結果を一覧へ入りきる前に読んでいた試験側の不具合)。v5.5.0 では流し直さず、v5.5.1 の ReleaseChecks で代えた。bench (9.5) は検索の索引・照会を変えていないため省略。
- **panic.log**: 54 件 17 種すべて disposition あり。製品ページのカード 53 件は上限内。
- **署名・成果物**: `build-dist.ps1 -SkipVst3Bridge` (署名あり)。単体exe・setup.exe・ポータブル版の mimageviewer.exe を `signtool verify /pa` で確認 (exit 0、Open Source Developer Taku Sano、timestamp あり)。全 PE / VC runtime gate 通過。sha256: 単体exe `aafc41f6e4f4be866df9308f3bf5a4ebfc311f2710f00bf3a6ed6d2b25956b43` (609,343,576 B)・setup.exe `10ea7e997426330d30fcfd7024393d85f6ff0bb5644f6806df215836e61a0b4a` (273,477,592 B)・Vector zip `656ba3b0eb6ae9333b5fd86ff37f57d57ad6eeab5a674e0cd73766b975e41bba`・ポータブル zip `4e2ff2d69540ba4a92d8100744cbccaa747f8feba0d9555b212a073dbc528829`。
- **VirusTotal**: 省略。VST3 host と mIV Sound は v5.4.0 から変更なし (再署名のみ)。
- **実機確認 (利用者、インストール版)**: 問題なし (2026-11-21)。
- **サブ PC の確認 (観測者はサブ PC 担当、master `120ff61fb` の build-dev)**: Ctrl+S / Ctrl+T (v5.4.0・修正前は入力できず、修正後は英字・IME とも入力できた)・§1.823〜1.826 (軽微 2)・§1.476 (軽微 2、v5.4.0 から)・§1.762 (軽微 2: 範囲指定で 1 文字目でメニューが閉じる件は既知の問題に掲載し更新履歴から文言を外した)・§1.816 PDF / NIS / Anime4K (問題なし)・§1.462 (軽微 2、以前から)。§1.464 の確認は公開後に届き、ツリーへのドラッグが移動でも「＋ コピー」と表示される件を §1.836 として v5.5.1 で直す (利用者決定、既知の問題に掲載)。§1.830・§1.740 は公開後に確認。
- **公開 (2026-11-21 15:12)**: GitHub Release v5.5.0 (latest)、資産 4 点のサイズが手元と一致、`releases/latest/download/` の 3 点は 200・サイズ一致。mikage.to 反映 26 ファイル (サーバ側 SHA-256 一致、verify-site 405/406 = `.htaccess` の 403 のみ)。
- **Vector・MS Store**: v5.5.0 は申請しない (利用者決定 2026-11-21: 3 連休中で、連休中に次の版も出す予定のため)。

## v5.4.0 (2026-11-19)

- **対象**: 配布ビルドの入力は master `89bf322a8` (製品コードは §1.815 を取り込んだ `4ac4227ca` と同じ、以降は文書のみ)。タグ v5.4.0・main は `7e32884d1` (入力との差はバックログの記録だけ)。v5.3.0 以降の §1.474・§1.771・§1.778・§1.763・§1.791・§1.795・§1.776 案 B・§1.805・§1.808・§1.802〜§1.807・§1.810・§1.811・§1.815 ほか。§1.809 は見送り (利用者)。
- **依存物**: `check-dependency-updates.ps1` で PDFium・Rust・EffeTune Mixwright・crate 67 個に更新あり。利用者の判断で今回は上げない (§1.816 へ)。FFmpeg は手元の n7.1.5-16 のほうが新しく、検査の表示は名前の比較による誤り。LibRaw・libavif・ONNX Runtime は最新。
- **CI 相当**: §1.803 で撤去した `invalidate_converted_archive_sources_in_parked_contexts` の許可一覧の登録が viewer_context_audit の A4 に当たり、外した (`ff5b18777`)。push 後の CI (main `7e32884d1`) は成功。
- **自動テスト**: master の test-full (`256b29527`) 本体 21,112 件成功。test-script の lib 全件は、§1.474 の K キーの分岐の漏れでコンパイルできず `4735863b8` で直した後、19,160 件中 19,159 件成功 (EPUB の 1 件は負荷時だけ失敗、§1.814 d)。1 回目の配布ビルドの全体テストは全件成功。§1.815 を取り込んだ後の配布ビルドは、os error 1455 で 1 回、負荷時だけ落ちるテストで 2 回 (§1.814 e・f) 止まり、利用者の了承 (2026-11-19「省いて作る」) で `-SkipRustTests` を使った。§1.815 のブランチは lib 全件 19,007 件成功、落ちた各テストは単独の繰り返しで成功。
- **ReleaseChecks (ClaudeCode がターミナル パネルから実行、利用者了承 2026-11-19、first-ai あり)**: `release-checks-20261119-080539-b5f71841` は PASS_WITH_WARNINGS。idle 5 ケースすべて PASS (CPU 1 コア比 0.6% 以下、測定中の描画 0 フレーム)。perf-cached は drawn 16ms 未満 98.35%・max 62ms、perf-uncached は 99.35%・max 35ms。pre_grid p95 0.111 / 0.116ms。first-ai は drawn 8/10 (80%) で目標 97% 未満の警告だけ、100ms 超 0。§1.809 の担当の GPU 計測 (17:15:53〜17:16:26) は診断ビルドの作成中で、測定区間とは重なっていない。
- **panic.log**: 54 件 17 種すべて disposition あり。製品ページのカード 53 件は上限内。マニュアルのサイドバーは全ページ 34。
- **署名・成果物**: `build-dist.ps1 -SkipVst3Bridge -SkipRustTests` (署名あり)。単体exe・setup.exe・ポータブル版の mimageviewer.exe を `signtool verify /pa` で確認 (exit 0、Open Source Developer Taku Sano、timestamp あり)。全 PE / VC runtime gate 通過。sha256: 単体exe `8f02a7a6e677512ad12325a60560c1b38117042d09c3de226854ded94c6783cc` (607,192,152 B)・setup.exe `4564946dfae7382b47d6e5d6d2331f2f2c5c8ee9118062e17b7169ca0da76ad5` (273,402,872 B)・Vector zip `f37f148819aec588eee37dff508319e4ae160f39e27696ccbb56abfc53d42364`・ポータブル zip `dfc63da78246bf74d654609118e5e963337000be7dd5bcd47ec65185963e636c`。
- **VirusTotal**: 省略。VST3 host は C++ を変えず作り直していない (`-SkipVst3Bridge`)、mIV Sound は v5.3.0 と同じ 0.17.0-miv.1。中身は前回検査の版と同じ (再署名のみ)。
- **実機確認 (利用者、インストール版)**: 1 回目のビルドで、窓の閲覧表示 + 上下 HUD・ストリップ固定のときページ内スクロール (上下キー・ホイール・K/J) が動かないことを利用者が発見 (§1.815、原因は表示領域の 1/32 point の丸めの食い違い)。修正後のビルドで利用者が確認し問題なし。サブ PC の確認 (§1.802・§1.804・§1.806・§1.807・§1.810・§1.811、観測者はサブ PC 担当) は master 入りの各項目に記録。
- **公開 (2026-11-19 夜)**: GitHub Release v5.4.0 (latest)、資産 4 点の sha256 が手元と一致、`releases/latest/download/` の 3 点は 200・サイズ一致。mikage.to 反映 11 ファイル (サーバ側 SHA-256 一致、verify-site 405/406 = `.htaccess` の 403 のみ)。
- **Vector**: 利用者が申請 (2026-11-19)。
- **MS Store**: v5.3.0 の再申請が同日に公開された (winget で v5.3.0・sha256 `de314428…3587` を確認)。v5.4.0 はプロパティの修正 (サポートの連絡先が `http://` + メールアドレスの形で開けない) と一緒に申請する (利用者 2026-11-19)。setup.exe を R2 `https://dl.mikage.jp/mimageviewer/v5.4.0/mImageViewer_setup.exe` に配置 (200・サイズと配信 sha256 一致)。Sandbox の起動確認は利用者の判断で省略したが、中止の連絡の前に自動試験が終わっていた (サブ PC、観測者はサブ PC 担当、`C:\miv-sandbox\results\v5.4.0-20261119-225645`): インストール exit 0、初回・2 回目とも窓が出て応答なし 0 件。参考 (サブ PC 担当がログから、未調査): 待機中に描画を再開した回数が初回 6 回・2 回目 1 回で、v5.3.0 の初回 1 回より多い。認定の注意事項は `dist/store-notes-v540.txt` (1,992 字、v5.3.0 版の文字化けしていた「起動中…」「開始」を直した)。
- **X**: 告知を利用者が投稿 (2 件のスレッド)。

## v5.3.0 (2026-11-17)

- **対象**: master `23b60a1b1` (配布ビルドの入力、タグ v5.3.0)。v5.2.1 以降の文字起こし T1・T2、§1.789 (AI アプリ連携中の更新)・§1.769・§1.755 (mIV Sound 0.17.0-miv.1)・§1.767 第 2 段・§1.800 (本を閉じるときのもたつき)・§1.776 案 A・§1.796 (見開きのまま 1 ページ送る)・§1.745/§1.747・§1.773b ほか。更新履歴は利用者承認済み (8,053 バイト、短縮版なし)。「重要な変更点」に §1.745 (動かした画像のクリックで送らない) を必読、文字起こしと 1 ページ送りを目玉で追加。依存の更新は見送り (利用者決定)。既知の問題は変更なし。
- **CI 相当**: §1.767 第 2 段の `replace_mounted_detached_window_binding` が viewer_context_audit の A4 に当たり、許可一覧に登録 (`ade93d03d`)。push 後の CI (main) は成功。
- **自動テスト**: 1 回目の配布ビルドは全体テストの 2 件で停止。(1) launcher の `real_core_source_and_installer_keep_legacy_base_names` が、§1.789 A-1 で `installer/launcher_swap.iss` へ移した名前を古い場所で探していた。(2) 起動時の「重要な変更点」のスナップショット 2 枚 (v5.3.0 の項目を足したための想定どおりの変化。目視で確認して更新)。どちらも §1.789 の master 取り込み後に全体テストを流していなかったため (`23b60a1b1`)。やり直した配布ビルド内の全体テスト 20,639 件成功 (87 skip、再試行なし) と vendor・doc が成功。
- **ReleaseChecks (ClaudeCode が実行、利用者了承 2026-11-17、first-ai なし)**: 1 回目 `release-checks-20261117-091124-3d7204a8` は裏のプロセスから前面を取れず環境で停止。利用者の指示でターミナル パネルから回し直した `release-checks-20261117-091650-6be1ac21` で seed・idle 5・perf 2 が PASS。idle は CPU 1 コア比 0.5% 以下・測定中の描画 0 フレーム。perf-cached は drawn 16ms 未満 99.67%・max 40ms、perf-uncached は 99.01%・max 53ms。pre_grid p95 0.117 / 0.119ms。100ms 超 0。
- **bench**: 10 クエリすべて OK (文字起こしで索引の形式が変わったため実施)。
- **panic.log**: 54 件 17 種すべて disposition あり (`check-panic-log.ps1` exit 0)。製品ページのカード 53 件は上限内。
- **署名・成果物**: `build-dist.ps1 -SkipVst3Bridge` (署名あり)。単体exe・setup.exe・ポータブル版の mimageviewer.exe を `signtool verify /pa` で確認 (exit 0)。全 PE / VC runtime gate 通過。sha256: 単体exe `d4d0c88f8042428fee4cd93cd0e30cc74c0137f684310681d56da6347826e4ff` (603,867,224 bytes)、setup.exe `de31442806d7c9cb73a611afb25e5063ea624215c734c266d6a3d27caee03587` (273,135,400 bytes)、portable zip `3e74294a3dd9ee271d7de860b8bc6779ba553745517b11a274d84884acff006a` (323,847,626 bytes)、Vector zip `f13a3e0583cf6fdce139cddab49f22c038a0a82b71e220ceab9017571acf1550` (272,706,274 bytes)。
- **VirusTotal (2026-11-17、送信は利用者の了承のうえ利用者が実行、結果は ClaudeCode が結果ページで確認)**: `effetune/mIV Sound.vst3/Contents/x86_64-win/mIV Sound.vst3` (8,318,040 bytes、0.17.0-miv.1) sha256 `cd41ed82d465fd466b07f7b5162e01e850fde593229ba72b1e258642075a3364` — 0/71、Google・Microsoft とも Undetected。https://www.virustotal.com/gui/file/cd41ed82d465fd466b07f7b5162e01e850fde593229ba72b1e258642075a3364 。VST3 host は vendor の実体が前回と同じ (`194ddc8d…`) で再署名のみのため送っていない (チェックリスト 12.6)。
- **実機確認 (利用者、インストール版、2026-11-17)**: 確認 4 項目 (§1.776・§1.800・§1.745・§1.773b) は期待どおり。本を閉じたとき 1 回ひっかかりを感じた。perf ログ (統括が解析): 本を閉じて一覧へ戻る 4 回中 3 回は 10〜12 ms で §1.800 の止まりは出ていない。残る 1 回の 345 ms は AI 高画質化の最中の close で、v5.2.1 と推定される前日の session にも同じ形があり (§1.801)、今回の版の変更ではない。最初の全画面の 907 ms は「アニメ拡大」の初回作成 864 ms (§1.776 の案 B の範囲)。2.9 秒・1.7 秒の区間は右クリックメニューを開いていた時間。
- **公開 (2026-11-17 夜)**: GitHub Release v5.3.0 (タグ・main = `23b60a1b1`、main は 08cca1eca から fast-forward)、資産 4 点の sha256 が手元と一致・latest。mikage.to 反映 158 ファイル (mIV Sound 0.17 の説明書と `.htaccess` の許可一覧を含む)、verify-site 405/406 (`.htaccess` の 403 のみ)、DL 3 点 200・サイズ一致。Vector 申請は利用者が実施。
- **MS Store**: setup.exe を R2 `https://dl.mikage.jp/mimageviewer/v5.3.0/mImageViewer_setup.exe` に配置 (200・サイズと配信 sha256 一致)。サブ PC の Windows Sandbox (GPU なし、依頼 69、観測者はサブ PC 担当。統括が watch.txt と画像を確認) で Store と同じ引数のインストール exit 0、初回 180 秒・2 回目 92 秒とも応答なしは 0 件で、初回設定の付いたメイン窓が描かれた。認定の注意事項は文字起こしの任意ダウンロードを足した版に差し替え (1,957 字)。Package validation 中 (利用者の報告)。

## v5.2.1 (2026-11-15)

- **対象**: ブランチ `release-5.2.1` の `be26f0088` (配布ビルドの入力、タグ v5.2.1)。v5.2.0 の後の文書 (`230228e25`) を基点に、§1.773 (表紙の自動切り出し: 綴じ方向の専用設定・画像フォルダの本・寸法の型・高さ全体・本文の保存回転・判定の理由の表示)・§1.775 (SNS 分割を 20 枚まで)・入力履歴のポップアップの配色・動画の時刻原点 (開始時刻が 0 でない MPEG)・環境設定「お気に入りごとに表示状態を記憶する」の配置を載せた修正版。master には v5.3.0 向けの未出荷の変更があるため、master からは出さなかった。更新履歴は利用者承認済み (4,955 バイト、短縮版なし)。「重要な変更点」に綴じ方向の専用設定を必読で追加 (起動時の 2 枚のスナップショットを更新、1093×614 で閉じる操作が見えることを目視)。既知の問題に「本文のページを回転した後も、自動切り出しの結果が前のまま残る」を追加 (次の版で直す)。
- **取り込み**: §1.773 と配置の修正がともに環境設定のサムネイルのページを変えていて、`preferences_favorite_view_state_dark.png` がぶつかった。取り込んだコードで作り直しても変わらなかったので、配置の修正側の画像を使った。§1.773 の `CatalogDb::save` の引数追加に `tests/catalog_integration.rs` が追従しておらず、配布ビルドの全体テストのビルドで止まった (§1.773 側の検証は lib と UI スナップショットだけだった)。既定値を渡す形に直した (`be26f0088`)。
- **実機確認**: §1.773 は利用者が実データで一覧と確認用ビルドを見て「この状態でリリース」と判断。§1.775 は利用者が X で 13 分割を確認。履歴のポップアップの配色は利用者が確認。時刻原点と配置の修正はサブ PC 担当が確認。配布物はインストール版を利用者が入れて確認 (問題なし)。
- **自動テスト**: 配布ビルド内の全体テスト 19,858 件成功 (82 skip、再試行なし) と vendor・doc が成功。test-script feature の lib 全件 17,940 件成功 (76 skip)。UI スナップショット 258 件成功。`cargo fmt --all --check` と `viewer_context_audit` (テスト 69 件・実行) は手元で成功 (リリースブランチは CI 対象外)。
- **ReleaseChecks**: 利用者の判断で省いた (v5.2.0 から 2 日で、一覧の描画と読み込みの経路は表紙の自動切り出し以外を変えていないため)。
- **依存**: v5.2.0 と同じ (同版で確認済み、再実行なし)。VST3 host の実行ファイルは v5.2.0 のビルドと同じ (vendor の sha256 先頭 `f66929ccbdf95314`)、mIV Sound は 0.15.0-miv.7 のまま。中身が変わっていないので VirusTotal は送っていない (チェックリスト 12.6)。
- **panic.log**: 54 件 17 種すべて disposition あり (`check-panic-log.ps1` exit 0)。製品ページのカード 53 件は上限内。マニュアルのサイドバーは全ページ 34。
- **署名・成果物**: `build-dist.ps1 -SkipVst3Bridge` (署名あり)。単体exe・setup.exe・ポータブル版の mimageviewer.exe を `signtool verify /pa /v` で確認 (Open Source Developer Taku Sano、タイムスタンプあり、exit 0)。ポータブル版の PE 26 個がすべて有効な署名。全 PE / VC runtime gate 通過。sha256: 単体exe `0dbc1f26bd665f67eeae232e343b1fad7acc59bcf58e39cd118875dfb891cc73` (593,026,136 bytes)、setup.exe `566786871773da089683733064c808ab9fa6cbf6032654da62b08c34550ccea4` (269,412,344 bytes)、portable zip `866e8c6d794f49a2de78692fc507903c6ccc878f4abbdfaacda165571e4eba9a` (318,032,253 bytes)、Vector zip `964558b37668fd9a63429d816d459ee1c74c9643abe9b87603dd5a80f50b297a` (268,981,905 bytes)。
- **配布ビルドで止まった点**: (1) 利用者の確認用の dev core が普段のデータで動いていた (利用者が終了)。(2) 新しい worktree で release の 5 本が無く、全体テストの launcher build が止まった (`build-release.ps1` で先に作った)。(3) 上の結合テスト。
- **インストーラ**: AI アプリが MCP を使っている間は、インストーラが「AI アプリを終了してから」で止まる (仕様)。利用者は Claude を再起動して入れた。AI 連携中でも更新できるようにする設計を別セッションで検討中。
- **公開後 (2026-11-15 夜)**: GitHub Release v5.2.1 (タグ・main = `be26f0088`、main は 403725b81 から fast-forward)、資産 4 点の sha256 が手元と一致・latest。mikage.to 反映 11 ファイル (sitemap.xml を含む)、verify-site 390/391 (`.htaccess` の 403 のみ)、DL 3 点 200・サイズ一致。v5.2.1 の worktree は htdocs が CRLF で checkout されていて、そのままでは中身の同じページまで差分になった。前回反映したファイル (C:\home\mimageviewer の作業ツリー) と同じ bytes に揃えてから上げた。 Vector 申請・X 告知 (2 件のスレッド) は利用者が実施。MS Store は v5.2.0 がまだ審査中のため今回は更新しない (利用者判断)。

## v5.2.0 (2026-11-13)

- **対象**: master `43f174e3f` (配布ビルドの入力)。v5.1.0 以降の §1.718・§1.732・§1.733・§1.751〜§1.753・§1.754 (PDFium)・§1.756・§1.759・§1.764 (手動・自動の表紙範囲、回転は §1.773 で次の版)・§1.766・§1.767 第 1 段・§1.770・§1.682/§1.688・§1.690 (名前で検索とタイトル表示の修正)・ツリーのクリックで中央へ寄せない・履歴の消去 2 種の割り当て・§1.483 (ReleaseChecks の台本)。更新履歴は利用者承認済み (8,121 バイト、短縮版なし)。公開前に `bd417429d` (監査の許可一覧と記録のみ、製品コードの変更なし) を追加。さらに利用者の判断で §1.764 の UI 修正 (メニューの位置・ダイアログのボタンを下端へ・画像の周りの余白、`0a10f19b2`) を取り込み、master `e74bed0f7` で配布ビルドを作り直した (以下の配布ビルド・署名・成果物はこの作り直し後のもの)。
- **サブ PC の実機確認 (サブ PC 担当の観測)**: 依頼 37 (§1.755、次の次の版へ)・38 (§1.770)・39 (§1.767 第 2 段、次の版へ)・40 (§1.769、次の版へ)・41 (§1.764)・48 (§1.764 の UI 修正。開き直したときの大きさは §1.777 で次の版へ)・42/42b (§1.682)・43/44 (§1.690)・45 (ツリーのクリック)・46/47 (履歴の消去)。取り込んだ項目はすべて期待どおり、または既存の作り・手順書の想定違いと判断。
- **CI 相当**: `cargo fmt --all --check` 成功。`viewer_context_audit` は §1.718 の `evict_legacy_cover_thumbnails` が A4 違反 → 許可一覧に登録 (`bd417429d`、独立 Sol / xhigh が構造的な変更に同意)、テスト 69 件・実行とも成功。
- **自動テスト**: 作り直した配布ビルド内の全体テスト 19,672 件成功 (再試行なし。作り直す前は 19,664 件成功)。test-script feature の lib 全件 17,747 件成功。master の lib 全件は負荷時に `section1375_large_physical_scan_10k_100k` が 600 秒で時間切れになることがあり (単独では約 110 秒、§1.765 に記録)、配布ビルド内では成功。UI スナップショット 257 件成功 (v5.2.0 の「重要な変更点」で 2 枚を更新、1093×614 で閉じる操作が見えることを目視)。
- **ReleaseChecks (ClaudeCode が実行、利用者了承 2026-11-13、`-IncludeFirstAi` 付き)**: 1 回目 `release-checks-20261113-042713-611ed898` は static-background の開始時に前面の窓の不一致 (環境) で停止、自動再試行なし。利用者が窓を最小化・通知を止めて 2 回目 `release-checks-20261113-050011-2e1435fd` (`-SkipBuild`、同じ診断 artifact)。
  - seed・idle 5・perf 2 は PASS。idle の CPU 1 コア比 0.001〜0.004、測定中の描画 0 フレーム。perf-cached は drawn 16ms 未満 302/303 (99.67%)・p95 10ms・max 27ms、perf-uncached は 303/304・p95 10ms・max 26ms。pre_grid raw p95 0.084 / 0.090ms。100ms 超 0。
  - **first-ai は FAIL (既知として出荷、利用者決定)**: §1.483 の台本修正で初めて最後まで走り、最初に全画面を開いた直後の全画面 viewport の 2 コマ目で 490ms (`immediate_viewport run=487ms`、AI の action の前)。10-29 の run でも同じ場所で 544ms あり、今回の版の変更ではない。§1.776 に記録。
- **bench**: 検索の採点・索引のコード、toolchain、tantivy に変更がないため実施していない。
- **依存**: PDFium (chromium/8152 が出ているが今回 8138 へ上げたばかり)・Rust 1.100・mIV Sound 上流 v0.17.0 は次の版以降 (利用者了承)。ほかは最新か通常の crate の小更新のみ。
- **panic.log**: 54 件 17 種すべて disposition あり (`check-panic-log.ps1` exit 0)。
- **署名・成果物**: `build-dist.ps1 -SkipVst3Bridge` (署名あり)。単体exe・setup.exe の署名が有効なこと (Open Source Developer Taku Sano、タイムスタンプあり)、ポータブル版の exe・DLL 27 個がすべて署名済みで有効なことを確認。sha256: 単体exe `f8ecffb6c886244cadbfac7f2a65fe0122cebb116cc8155be6b7d5bf35cb130b` (593,435,224 bytes)、setup.exe `99e21570f6d7b0e6425be0cb9b3aafbbabe854a4406d45377db5edc3efdc33a4` (269,492,776 bytes)、portable zip `79077f8d1210e285d3bbc558da69d9f30525985e242f614557d64560688df5b4` (317,886,560 bytes)、Vector zip `8df68825c789b7986ff18937423f7e6d7cf90302f9a3a65de2f4dd11740ec3cb` (269,062,221 bytes)。作り直す前の成果物 (単体exe `a2cb0f59…` など) は公開していない。
- **VirusTotal (2026-11-13、送信は利用者の了承のうえ利用者が実行、結果は ClaudeCode が結果ページで確認)**: 下の 2 件は作り直す前の成果物で確認したもの。作り直した後は、どちらも中身が同じで署名だけが付け直されている (大きさは同じ、sha256 は vst3 host `9a86d46dede544a516a0175bf0f2c326ada1dec3079c538f0bc49536c86f44b6`、mIV Sound `cbb187555102e9056461c8218b6424c07679a491990fd4ef10130cddd1debf96`)。中身が変わったときだけ確認する方針 (checklist 12.6、利用者決定 2026-11-13) により、送り直していない。
  - `mimageviewer-vst3-host.exe` (1,239,128 bytes) sha256 `958840b3e0fc5d8e883036c2e08416869730af98f9f22705a9f1c0ce215ba431` — 0/71、Google・Microsoft とも Undetected。https://www.virustotal.com/gui/file/958840b3e0fc5d8e883036c2e08416869730af98f9f22705a9f1c0ce215ba431
  - `effetune/mIV Sound.vst3/Contents/x86_64-win/mIV Sound.vst3` (5,894,232 bytes) sha256 `caaa4591b37c7f572381e3b6b28e9f4eff78afac22dfb947af590d0d34e73a35` — 0/71、Google・Microsoft とも Undetected。https://www.virustotal.com/gui/file/caaa4591b37c7f572381e3b6b28e9f4eff78afac22dfb947af590d0d34e73a35
  - 次回からは中身が変わったときだけ検査する (利用者決定、チェックリスト 12.6 を更新)。
- **公開後 (2026-11-13 夜)**: GitHub Release v5.2.0 (タグ・main = `81777f69d`)、資産 4 点の sha256 が手元と一致・latest・CI 緑。mikage.to 反映 19 ファイル + sitemap.xml、verify-site 403/404 (`.htaccess` の 403 のみ)、DL 3 点 200・サイズ一致。sitemap.xml はサイト直下のフォルダが別アカウントの持ち物で tar の削除→作成が失敗したため、`deploy-site.ps1` の展開を `--overwrite` (既存ファイルへ上書き) に直して上げた (これまでの手作業の scp 上書きが不要になる)。プライバシーポリシーの英語版に「撮影地点を地図で開く」が抜けていたので追記して反映 (`36d899b28`)。Vector 申請は利用者が実施。Store 用 setup.exe を R2 `https://dl.mikage.jp/mimageviewer/v5.2.0/mImageViewer_setup.exe` に配置 (HTTP 200・サイズと配信 sha256 一致)。認定の注意事項は審査手順に関わる変更がないため前のまま。Package validation は 11-14 に合格し、利用者が審査へ送信した (利用者の報告)。

## v5.1.0 (2026-11-10)

- **対象**: master `abdce7690` (配布ビルドの入力)。v5.0.0 以降の 3D 写真 (depth-parallax)、§1.452・§1.675/§1.676・§1.691・§1.697・§1.726・§1.731・§1.735・§1.741・§1.750 と、一覧の行が窓より高いときの表示位置の修正。更新履歴 (4,782 バイト、8 KB 以内なので短縮版なし) と「重要な変更点」の must_read (書庫内フォルダの直接表示) は利用者承認済み。
- **CI 相当**: `viewer_context_audit` を手元で実行し、3D 写真 stage B (`dee2fd1ec`) が足した、どこからも呼ばれていない `ViewerContextBundle::panorama_state` で A4 違反 1 件。`#[cfg(test)]` を付けて解消 (`be455e281`、Codex 実装、GPT-6.1 Sol / xhigh 独立レビュー ACCEPT)。違反 0、`cargo fmt --all --check` 成功。CI 自体は v5.0.0 以降 push していないので未実行。
- **自動テスト**: 配布ビルド内の全体テスト 19,123 件成功 (slow 10 件、再試行なし)。test-script feature の lib 全件 17,214 件成功。1 回目の配布ビルドは `startup_dialogs_small_viewport` のスナップショット不一致で止まった (v5.1.0 の「重要な変更点」を足したための想定どおりの変化)。PNG 2 枚を目視で確かめて更新 (`abdce7690`) し、配布ビルドをやり直した。
- **ReleaseChecks (ClaudeCode が実行、利用者了承 2026-11-10、`-IncludeFirstAi` 付き)**: run `release-checks-20261110-041427-f8b7e465`。AI / GPU 経路 (3D 写真の深度モデル・§1.726 の描画素材の種類) を変えたため first-ai を含めた。
  - seed 2・idle 5・perf 2 は PASS。idle は CPU 1 コア比 0.003〜0.009、測定中の描画 0 フレーム。perf-cached は drawn 16ms 未満 303/304 (99.67%)・p95 12ms・max 38ms、perf-uncached は 304/305 (99.67%)・p95 12ms・max 28ms。pre_grid raw p95 0.108ms。100ms 超の gap / update 0。
  - **first-ai は FAIL (既知のテスト台本の不具合 §1.483、AI 未計測)**: 10-29 と同じく、台本が全画面を開いた後に AI の action をメイン窓へ送り、main focus guard が全画面を閉じて「actual isolated AI inference did not complete」で 180 秒後に停止。モデルは DirectML で読み込めている。AI アップスケールの推論コードは v5.0.0 から変わっていない (変更は深度モデルの種類の追加と §1.726 の素材の種類だけ)。代わりの根拠: 同じ AI コードを含むビルドで、利用者が 2026-11-10 朝に AI アップスケール + 見開きの実機確認 (§1.750) を行い、問題を報告していない。
- **bench**: 検索の採点・索引のコードと toolchain・tantivy に変更がないため実施していない (§1.741 は検索結果のサムネイルの保存先だけの変更)。
- **依存**: crate の patch 対象 (egui 系・tantivy・mimalloc・unrar)・LibRaw・LibAVIF・ONNX Runtime・VST3 SDK・Rust は最新。PDFium (chromium/8138)・EffeTune 上流 v0.17.0 は新版があるが、公開直前のため上げず次の版以降で検討。FFmpeg は従来どおりの誤検知 (同梱 n7.1.5-16 の方が新しい)。VST3 host と Susie worker のソースは v5.0.0 から変更なし (`-SkipVst3Bridge`)。
- **panic.log**: 54 件 17 種すべて disposition あり (`check-panic-log.ps1` exit 0)。
- **署名・成果物**: `build-dist.ps1 -PreserveRuntime -SkipVst3Bridge` (署名あり) で単体exe / setup.exe / portable zip を生成。3 つの mimageviewer.exe / setup.exe を `signtool verify /pa /v` で確認 (Open Source Developer Taku Sano、Certum Timestamp 2026)。Vector 用 `mImageViewer_installer_v5.1.0.zip` を作成 (setup.exe + readme.txt)。
- **VirusTotal (2026-11-10、送信は利用者の了承のうえ利用者が実行)**:
  - `mimageviewer-vst3-host.exe` (署名済み、1,239,128 bytes) sha256 `99ffe3610db7b96e66a4951c8d650f46f9d6c77e9226a1bdc3c887532ff7fdf3` — 0/71 (利用者の結果ページの写しで確認)。https://www.virustotal.com/gui/file/99ffe3610db7b96e66a4951c8d650f46f9d6c77e9226a1bdc3c887532ff7fdf3
  - `effetune/mIV Sound.vst3/Contents/x86_64-win/mIV Sound.vst3` (DLL、署名済み、5,894,232 bytes) sha256 `30ea45a2c684822014981507f9d56eb3fdd4835ef1514318c9822c026f708a32` — 0/71 (同上)。https://www.virustotal.com/gui/file/30ea45a2c684822014981507f9d56eb3fdd4835ef1514318c9822c026f708a32
  - Google・Microsoft の行: 両ファイルとも Google・Microsoft は Undetected (利用者が結果ページで確認、2026-11-10)。
- **公開後 (2026-11-10 夜)**: GitHub Release v5.1.0 (タグ・main = `fa614117a`)、資産 4 点のサイズ一致・latest・CI 緑。mikage.to 反映 154 ファイル + sitemap.xml (権限の都合で scp 上書き)、verify-site は `.htaccess` の 403 のみ、DL 3 点 200・サイズ一致。§1.737 の旧 URL (版付き sound / effetune) はすべて 1 回の 301 で固定 URL の 200 に着き、canonical は自分自身、公開中の sitemap に版付き URL 0 件。Vector 申請・X 告知は利用者が実施。
- **Store 用インストーラ**: R2 `https://dl.mikage.jp/mimageviewer/v5.1.0/mImageViewer_setup.exe` (269,205,720 bytes、sha256 `ccb25d5adde6189e4c825196421f50b0f4cd8cba56123278e4b219c835294c1f`) に配置し、200・転送なし・配信内容の sha256 一致を `upload-store-installer.ps1` で確認。
- **Store 申請前の Sandbox 起動確認 (サブ PC 依頼 29、vGPU=Disable、`/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP-`、観測者はサブ PC 担当、取得は利用者)**: R2 から取得した setup.exe (sha256 一致・署名 Valid) で install exit 0 (約 26 秒)。初回 180 秒・2 回目 90 秒とも responding=False 0 件、タイトル「Desktop - mimageviewer」、初回設定の窓を表示。描画は Microsoft Basic Render Driver。ERROR / WARN / panic・Application Hang / WER なし。結果は `C:\miv-subpc\results-29\`。vGPU=Enable (§1.736) は今回行っていない。
- **Store 更新申請**: 2026-11-10 に利用者が Partner Center でパッケージ URL を R2 の v5.1.0 へ差し替え、Package validation を開始 (利用者の報告)。検証には毎回数時間かかるため、審査への送信は検証の合格後 (11-11 の見込み)。「プロパティ」の認定の注意事項は前のまま (審査手順に関わる変更なし)。

## v5.0.0 (2026-11-07)

- **対象**: master `d073af54a` (配布ビルドの入力)。v4.11.0 以降の v5 系 (MCP・AI 説明文・mIV Sound・音楽ビデオ) と §1.681〜§1.730 の修正。更新履歴は利用者承認済み (`3bb93924e`)、アプリ内通知用の短縮版 `docs/release-body-v5.0.0.md` (5.2 KB) も承認済み。
- **自動テスト**: 配布ビルド内の全体テスト 18,611 件成功 (vendor crate 分も全件成功)。test-script feature の lib 全件 16,715 件成功。初回は 1 件が失敗したが、原因はテストの fixture がホストの TEMP の深さに依存していたことで、`d073af54a` で fixture を直した (製品コードは不変。Codex 実装、xhigh 独立レビュー ACCEPT)。CI と同じ `cargo fmt --check`・viewer context 監査は手元で成功 (master は公開前に未 push のため GitHub CI の記録なし)。
- **ReleaseChecks (ClaudeCode が実行、利用者了承 2026-11-07)**: run `release-checks-20261107-121602-52b1e351`、9 件すべて PASS、警告なし。AI アップスケール経路の変更がないため `-IncludeFirstAi` なし。perf-cached / perf-uncached とも drawn 16ms 未満 99.7% (n=301/302、p95 11ms、max 50/53ms)、pre_grid p95 0.09ms。idle 5 件とも測定中の描画 0 フレーム、CPU 1 コア比 1% 未満。
- **bench**: `bench_search --docs 50000` で回帰なし。`rare_jp_and` +42% / +0.07ms は NOISE 判定、baseline は更新していない。
- **依存**: crate の patch 対象 (egui 系・tantivy・mimalloc・unrar)・LibRaw・ONNX Runtime・VST3 SDK・Rust は最新。PDFium (chromium/8138)・FFmpeg・EffeTune 上流 v0.17.0 は新版があるが、公開直前のため上げず v5.0.x 以降で検討。
- **panic.log**: 17 種すべて disposition あり。新しい 1 種 (accesskit_windows subclass.rs:109、2026-11-06、OS が ERROR_NOT_ENOUGH_MEMORY を返したのを依存側が unwrap) は external として記録 (`80ae99ea7`)。
- **署名・成果物**: `build-dist.ps1` (署名あり) で単体exe / setup.exe / portable zip を生成。単体exe / setup.exe / portable の mimageviewer.exe を `signtool verify /pa /v` で確認 (Open Source Developer Taku Sano、Certum Timestamp 2026、エラー 0)。Vector 用 `mImageViewer_installer_v5.0.0.zip` を作成。
- **VirusTotal (2026-11-07、送信は利用者の了承のうえ利用者が実行、判定は ClaudeCode が結果ページで確認)**:
  - `mimageviewer-vst3-host.exe` (署名済み、1,239,128 bytes) sha256 `5a49e23068609082085452f46f8497cc61d826749dfa216e61b76975c69007bf` — 0/65、Google・Microsoft とも Undetected。https://www.virustotal.com/gui/file/5a49e23068609082085452f46f8497cc61d826749dfa216e61b76975c69007bf
  - `effetune/mIV Sound.vst3/Contents/x86_64-win/mIV Sound.vst3` (DLL、署名済み、5,894,232 bytes) sha256 `25c94c8274e75c26f2c878ca4dd2a26171c1036d2e4ac07069954bc7349bfc4f` — 初回 0/62 で Google が Timeout、Reanalyze 後 0/71、Google・Microsoft とも Undetected。https://www.virustotal.com/gui/file/25c94c8274e75c26f2c878ca4dd2a26171c1036d2e4ac07069954bc7349bfc4f
- **公開後 (2026-11-07 夜)**: GitHub Release v5.0.0 (タグ・main = `446c59562`)、資産 4 点のサイズ一致・latest・CI 緑。mikage.to 反映 183 ファイル + sitemap.xml (権限の都合で scp 上書き)、verify-site 404/405 (.htaccess の 403 のみ)、DL 3 点 200・サイズ一致。ポータブル zip の Chrome ダウンロードでブロックなし (利用者が確認)。Vector 申請・X 告知は利用者が実施。Store 用 setup.exe を R2 `https://dl.mikage.jp/mimageviewer/v5.0.0/mImageViewer_setup.exe` に配置 (配信 sha256 一致)。Store の認定の注意事項・掲載文・Vector の説明文を v5 向けに更新。
- **Store 申請前の Sandbox 起動確認 (サブ PC、観測者はサブ PC 担当、`/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP-`)**:
  - 依頼 19 (vGPU=Enable、`C:\miv-sandbox\results\v5.0.0-20261107-233449`): install exit 0、起動から約 8 秒で Sandbox のゲスト Windows が bugcheck 0x3B (SYSTEM_SERVICE_EXCEPTION、0xC0000005) で停止。mIV の窓の有無は記録できず。10-25 の v4.8.1 は同じ wsb で完走していた。原因は未特定 (§1.736)。
  - 依頼 19b (vGPU=Disable、`C:\miv-sandbox\results\v5.0.0-20261107-235218`): install exit 0、初回 183 秒・2 回目 91 秒とも responding=True、タイトル「Desktop - mimageviewer」、初回設定の窓を表示。描画は Microsoft Basic Render Driver (Dx12)。ERROR / WARN / panic なし。

## v4.11.0 (2026-11-02)

- **対象**: master `2322f5a2c` (リリース用ブランチ `release-4.11.0` を、利用者の更新履歴承認後に merge)。v4.10.2 以降の §1.482・§1.663・§1.668・§1.669・§1.670・§1.671・§1.672・§1.673・§1.674・§1.677・§1.679・§1.680 と、バケツの「範囲の基準」ボタンの統一。各項目とも独立レビュー ACCEPT。
- **自動テスト**: 版上げ前の master `06f94d2a4` で lib 全件 15,474 件成功、ui_snapshot 218 件成功。配布ビルド (`build-dist.ps1 -SkipVst3Bridge`、VST3 host は v4.10.2 から変更なし) の全体テストで workspace 16,879 件成功 (77 skip、slow 9 件) と vendor・doc が成功。再試行なし。
- **ReleaseChecks (ClaudeCode が実行、利用者了承 2026-11-02)**: run `release-checks-20261102-002328-d3975711`、PASS_WITH_WARNINGS。AI 経路の変更がないため `-IncludeFirstAi` なし。idle 5 ケース PASS (CPU 1 コア比 0.003〜0.009、測定区間の frame 0)。perf cached / uncached とも 256 folder load・Ctrl+Down 5・検索完了、描画 frame の 16ms 未満 99.67% / 98.68%、p95 13 / 14ms、pre_grid raw p95 0.088 / 0.094ms。100ms 超の間隔は各 1 件で、どちらも測定操作前の起動直後 frame 1→2 (107 / 113ms、既知形・上限 500ms、2026-10-11 利用者了承) で警告として残る。unexplained 0 件。
- **実アプリ確認 (メイン PC、ClaudeCode が隔離データ `data-qa` で dev-runtime build を操作)**: §1.677 種類アイコン、§1.680 素早いダブルクリック (全画面から戻った直後を含む)、§1.679 バケツの素早いクリックと Ctrl+Z、バケツのボタン統一、§1.668 コレクションの参照 (読み取りだけで約 0.07ms、Auto 方針で DB を作らない) が期待どおり。図形の Ctrl+Z が効かなかった件は、パネルのボタンに入力先があると keymap が Ctrl+Z を抑止する以前からの制限で、§1.681 に起票。
- **実アプリ確認 (サブ PC、サブ PC 担当が操作)**: 依頼 1 (§1.669〜§1.674) は全項目期待どおり。配布物 (署名済みポータブル版) での依頼 2 は、種類アイコン・ファイル名の表示幅・コレクションのサムネイル保存・補正の素早いクリックと図形・バケツのボタン・強制終了のあとの報告・配布物の基本 (署名・版・各形式の表示と再生) が期待どおり。期待と違ったのは「開いた直後、同じ位置のダブルクリックが無視される」1 件で、依頼 3 で v4.10.2 と比べたところ v4.10.2 でも起き (回数は v4.10.2 のほうが多い)、退行ではないと判断して §1.685 に起票した。同時に気づいた点 (バージョン情報のライセンス欄の「?」、コレクションの ZIP / PDF のサムネイルを開くたびに作り直して保存し直すログ、強制終了の報告の最終記録時刻が起動直後のまま、選択中の情報吹き出しが下の段の種類アイコンに重なる) は公開を止める内容ではないとした。メイン PC での版比較は、computer-use が mIV ポータブル版の窓を前面と認識せず (前面を Explorer と判定、画面写真が暗いまま) 中止した。
- **依存**: FFmpeg は手元が新しい (候補は旧版)、crate の小更新は見送り、AVIF・PDFium は最新、VST3 host は v4.10.2 から変更なし。EffeTune 上流 v0.16.0 は v5 側で扱う。
- **panic.log**: 16 種すべて disposition あり。
- **署名・成果物**: 単体exe / setup.exe / portable の mimageviewer.exe を `signtool verify /pa` で確認 (Open Source Developer Taku Sano、Certum Timestamp 2026、exit 0)。全 PE / VC runtime gate 通過 (portable 22 PE、installer)。setup.exe 256,395,552 bytes、SHA-256 `C038B7674C0586C9D2D25A9B08B6924032CE2BC5695D5046D3A2AD2BAA0CF15A`。portable zip 265.5 MB、Vector zip 作成済み。CHANGELOG の節は 3,603 bytes で短縮版不要。

---

## v4.10.2 (2026-10-31)

- **対象**: ブランチ `release-4.10.2` の `68226ebc7` (配布ビルド)。v4.10.1 (`57e572874`) を基点に、修正 2 件だけを載せた修正版。master には v4.11.0 向けの未出荷の変更があるため、master からは出さなかった。修正は mimalloc を 3.3.2 から 3.5.3 へ更新 (§1.666、5ch >>655・657・672 の突然終了の対策。`libmimalloc-sys` 0.1.49 を vendor し、同梱の v3 を上流無改変の 3.5.3 に差し替え、`[patch.crates-io]` で選ぶ) と、Everything 検索結果から直接ページ表示した本の閉じる帰路 (§1.664、5ch >>671)。どちらも独立レビュー ACCEPT。ビルドは G: の worktree `G:\home\mimageviewer-everything-1664` で行った (本体の作業ツリーは別セッションが master へコミット中だったため)。
- **mimalloc の確認**: 配布物の core (単体exe・portable) に、3.5.3 で増えた option 名 `collect_merges_stats` の文字列があり、v4.10.0 の配布物には無いことを確認。lib テスト `allocator_tests::linked_mimalloc_is_v3_5_3` がリンクされた `mi_version()` = 30503 を検査。落ちる箇所そのもの (スレッド終了後の再入) の再現試験は無く、利用者環境で終了が止まるかは未確認。
- **自動テスト**: 配布ビルドの全体テストで workspace 16,481 件成功 (70 skip) と vendor・doc が成功。1 回目は `smart_folder_transition_tests::smart_zip_adoption_retires_*` の 2 件が補助関数の 10 秒期限 (`listing identity preparation did not settle`) で失敗した。同じ時間帯に別セッション (MCP 試験実装) のビルドが重なり、テスト自体が 23 秒かかっていた。単独では 3 回とも 1 秒未満で成功し、2 回目の配布ビルドでは全件成功。`test-script` feature の lib テストは 15,238 件成功 (66 skip)。既定 feature の lib 全件では、ページめくりのテスト 4 件が並列負荷で失敗 (単独・モジュール単位では成功、§1.447 に比較を記録: v4.10.1 でも 5 回中 2 回失敗する既知の不安定)。`cargo fmt --all --check` と `viewer_context_audit` は手元で成功 (リリースブランチは CI 対象外)。
- **ReleaseChecks (ClaudeCode が実行、利用者了承)**: run `release-checks-20261031-100434-4e7887d3`、全項目 PASS。AI 経路の変更がないため `-IncludeFirstAi` なし。idle 5 ケース PASS (CPU 1 コア比 0.005〜0.009、測定区間の frame 0)。perf cached / uncached とも 256 folder load・Ctrl+Down 5・検索完了、描画 frame の 16ms 未満 99.67% / 99.67%、p95 11 / 12ms、pre_grid raw p95 0.092 / 0.090ms、100ms 超の間隔 0 件 (起動直後の frame 1→2 も 100ms 未満で、§1.667 の判定ずれは起きなかった)。アロケータ変更後も v4.10.1 と同じ水準。メモリ使用量の比較 (3.5.3 で purge の既定待ちが 1 秒→4 秒) は ReleaseChecks の対象外で未測定。
- **依存**: mimalloc 以外は v4.10.1 と同じ (同日の v4.10.1 で `check-dependency-updates.ps1` 実施済み、再実行なし)。
- **panic.log**: 53 件 16 種すべて disposition あり (`check-panic-log.ps1` exit 0)。製品ページのカード 49 件は上限内。更新履歴ページは生成スクリプトの出力と一致。既知の問題ページは変更なし。
- **署名**: 単体exe / setup.exe / portable の mimageviewer.exe を `signtool verify /pa /v` で確認 (Open Source Developer Taku Sano、Certum の RFC3161 タイムスタンプ、exit 0)。全 PE / VC runtime gate 通過。
- **portable smoke (ClaudeCode が実施)**: `D:\miv-portable-smoke-v4102` に展開して起動。exe の隣に `data\` を作成、`%APPDATA%\mimageviewer\settings.db` は変わらず。1 回目は初回設定のダイアログ (`first_setup`) が閉じる要求を保持して終了せず (v4.10.1 と同じ形)、強制終了した。2 回目は WM_CLOSE で終了コード 0。`native-exception.log` は空。
- **実機確認**: §1.664・§1.666 とも自動テストとコード確認だけで、実アプリでは未確認。

---

## v4.10.1 (2026-10-31)

- **対象**: master `ae64ebc79` (配布ビルド)。修正は 5ch >>661 の一覧位置 (§1.488)、閲覧履歴の「親へ」(§1.473)、フォルダツリー幅 (§1.472)、ページ一覧の操作名 (§1.662)。依存物・検索・VST3・Susie の変更なし。
- **自動テスト**: 配布ビルドの全体テストで workspace 16,467 件成功 (70 skip) と vendor egui 系・doc が成功。`test-script` feature の lib テストは 15,224 件成功 (66 skip。実行中に別セッションが文書をコミットしたため成功記録の再利用登録はなし)。
- **ReleaseChecks (ClaudeCode が実行、利用者了承)**: 1 回目 `release-checks-20261031-013001-1965e4d8` は seed の起動で mIV の窓を前面にできず停止 (環境不成立、各ケース未実行)。利用者の再了承で 2 回目 `release-checks-20261031-014232-5895a34d` を実行。AI 経路の変更がないため `-IncludeFirstAi` なし。
  idle 5 ケース PASS (CPU 1 コア比 0.006〜0.009、測定区間の frame 0)。perf uncached は PASS_WITH_WARNINGS: 256 folder load・Ctrl+Down 5・検索完了、描画 frame の 16ms 未満 99.67%、pre_grid raw p95 0.087ms、100ms 超は既知の起動直後 frame 1→2 (120ms、上限 500ms、2026-10-11 利用者了承済みの警告) だけ。
  **perf cached は FAIL (判定スクリプトの不具合、利用者判断で理由つき FAIL として記録し配布へ進めた)**: 数値は描画 frame の 16ms 未満 99.67%、pre_grid raw p95 0.089ms、100ms 超は起動直後の frame 1→2 (112ms) の 1 件だけ。これが既知の形と判定されなかったのは、初期化完了 (`indexer_manager_new`、`t`=0.824) が frame 1 の update 終了 (0.8235728) の 1ms 以内で、1ms 丸めの perf 時刻では順序を示せなかったため。最初の測定操作より前で、今回の修正経路とは関係しない。判定の修正は §1.667。
- **依存**: `check-dependency-updates.ps1` は FFmpeg の従来どおりの誤検知 (同梱 n7.1.5-16 の方が新しい)、EffeTune Mixwright v0.16.0 の公開、一般クレート 12 件の小更新。修正版のため上げずに出荷。PDFium 8125・ONNX Runtime 1.24.4・LibRaw 0.22.3・VST3 SDK・Rust 1.99.1 は最新。
- **panic.log**: 53 件 16 種すべて disposition あり (`check-panic-log.ps1` exit 0)。CI は直近 push 分まで緑。
- **署名**: 単体exe / setup.exe / portable の mimageviewer.exe を `signtool verify /pa` で確認 (Open Source Developer Taku Sano、Certum の RFC3161 タイムスタンプ、exit 0)。全 PE / VC runtime gate 通過。
- **portable smoke (ClaudeCode が実施)**: `D:\miv-portable-smoke-v4101` に展開して起動。exe の隣に `data\` を作成、`%APPDATA%\mimageviewer\settings.db` は変わらず。展開直後の 1 回目は画面の準備に約 12 秒 (ログの 2.99s→12.14s が無音、原因未確認) かかり、最初の WM_CLOSE では終了せず 2 回目で終了。2 回目の起動は約 1 秒で応答し、WM_CLOSE で終了コード 0。
- **実機確認**: §1.488 (一覧位置) は master 取り込み後に ClaudeCode が隔離データの dev-runtime を操作して確認 (マウス・キー・合成タッチ。記録はバックログ §1.488)。§1.473・§1.472・§1.662 は自動テストとコード確認だけで、実アプリでは未確認。

---

## v4.10.0 (2026-10-30)

- **対象**: master `e127d9a23` (配布ビルド)。ReleaseChecks は `c95205fce` (その後の差分は起動時の「重要な変更点」の snapshot 更新だけ)。
- **自動テスト**: 配布ビルドの全体テストで workspace 16,301 件成功 (69 skip) と vendor egui 系・doc が成功。1 回目は起動時の「重要な変更点」の snapshot 更新漏れ 1 件で停止 (`e127d9a23` で更新)。`test-script` feature の lib テストは 15,047 件成功 (1 回目は §1.652 の test-only キー Tab / Space の match 漏れでコンパイル失敗、`45366aff5` で修正)。
  事前の test-full では AI 結合テスト 5 件が `onnxruntime.dll extract: アクセスが拒否されました` で失敗した。原因は起動中のインストール版 v4.9.0 が `%APPDATA%\mimageviewer\onnxruntime.dll` (1.24.2) を使用中で、テストが 1.24.4 で上書きできなかったこと (製品の不具合ではない)。インストール版を終了した配布ビルドでは成功。
- **ReleaseChecks (ClaudeCode が実行、利用者了承)**: run `release-checks-20261029-161208-c3ed5627`、`-IncludeFirstAi` 付き。1 回目は利用者の操作で前面が外れ SKIPPED (環境不成立) のため `-SkipBuild` でやり直した。
  idle 5 ケース PASS (CPU 1 コア比 0.005〜0.016、測定区間の frame 0)。perf cached / uncached とも 256 folder load・Ctrl+Down 5・検索完了で PASS_WITH_WARNINGS: 描画 frame の 16ms 未満 99.67% / 99.33%、pre_grid raw p95 0.10 / 0.10ms、100ms 超は既知の起動直後の frame 1→2 だけ (124 / 116ms、上限 500ms、2026-10-11 利用者了承済みの警告)。
  **first-ai は FAIL (テスト台本の不具合、AI 未計測)**: 台本が全画面を開いた後に AI モデル切替の action を root (メイン窓) へ focus して送るため、既存の main focus guard (2026-07 から) が全画面を閉じ、AI ジョブが取り消されて「推論が完了しない」になった。ポータブル版は ORT 1.24.4 の DirectML でモデルを読み込めている。初回 AI 中のフレームの滑らかさは未計測。代わりの根拠: 5ch >>655 の調査で ORT 1.24.4 の実 GPU 推論 10 回が正常終了、依存物更新時と今回の配布ビルドの AI 結合テストが成功。台本の修正は §1.483。
- **依存**: ONNX Runtime 1.24.4 (ort rc.15)、PDFium 8125、LibRaw 0.22.3、tantivy 0.27.0 (score 収集を 0.26 の buffer 方式へ戻す MIV 改変)、EffeTune Mixwright v0.15.0、VST3 SDK 3.8.1、Rust 1.99.1 (§1.460、記録は [依存物の更新 2026-10-28](dependency-update-20261028.md))。検索 bench は依存物更新時に profile をそろえて測定済み (0.26 比 +5%、baseline 比較成功)、その後に検索コードの変更なし。`check-dependency-updates.ps1` は FFmpeg の従来どおりの誤検知 (同梱 n7.1.5-16 の方が新しい) と一般クレート 1 件だけ。
- **panic.log**: 53 件 16 種すべて disposition あり (`check-panic-log.ps1` exit 0)。
- **CI 相当**: `viewer_context_audit` を手元で実行し、§1.216 の public fn の登録漏れ 1 件を修正 (`e7f17e16c`)、違反 0。
- **署名**: 単体exe / setup.exe / portable の mimageviewer.exe を `signtool verify /pa /v` で確認 (Open Source Developer Taku Sano、RFC3161)。全 PE / VC runtime gate 通過。
- **portable smoke (ClaudeCode が実施)**: `D:\miv-portable-smoke-v4100` に展開して起動。応答あり、exe の隣に `data\` を作成、`%APPDATA%\mimageviewer\settings.db` は変わらず、WM_CLOSE で正常終了。
- **当日の実機確認 (利用者)**: ワイプ比較、問題の動画でのデインターレース、Ctrl+Space ズームの感度、消しゴムの Tab プレビュー、キーズーム (テンキーは利用者の割り当てが優先)、ページめくり中の編集反映 (素早い連打では着地後に反映、仕様)。

---

## v4.7.0 (2026-10-15)

- **対象**: 最終の配布ビルドは master `bf6b36993` (2026-10-15 23:55)。ReleaseChecks は `ad36b5606` で実施。その後に入った製品コードは、公開前に利用者実機で見つかったクラッシュの修正 C1 (`6c6c27fc8`、検索結果から別窓再生中に検索を閉じると plan_promote の assert で終了。§1.375 由来の v4.7.0 退行)、HUD 前面化の自己循環を止める修正 (`812ae722d`)、裏で準備中のページの AI 補完通知を出さない修正 (`b726eba70`)、空いた側を黒い紙でめくるページめくり (`ad36b5606` 以前)。ReleaseChecks は C1・HUD の後に `a00f863cf` で再実施 (run `release-checks-20261015-150147-c2c64337`、**PASS_WITH_WARNINGS**): idle 5 ケース PASS (CPU 1 コア比 0.003〜0.006)、perf cached / uncached とも 16ms 未満 99.67%、pre_grid raw p95 0.077ms、100ms 超は既知の起動直後の frame 1→2 だけ (222 / 225ms、上限 500ms、測定操作前、2026-10-11 利用者了承済みの警告)。
- **公開前の追加確認 (ClaudeCode が実施)**: 配布物のポータブル版を `D:\miv-portable-smoke-v470b` に展開し、「動画を別ウィンドウで開く」設定で Ctrl+R (Everything) の `pikasonic` の結果から動画を別窓再生 → 検索を × で閉じる。promote 1 回、panic 0、再生継続、応答あり。修正版の dev build でも、別窓再生中のメイン移動 → 元フォルダへの外部追加で落ちず・読み直しなし、メイン側の追加は自動反映を確認 (隔離データ)。
- **配布ビルドの全体テスト**: 最終回は 72 本 0 失敗 (lib 13,219 passed)。それまでの回で、負荷時だけ落ちるテストを 3 件直した (`3bedcd0a5` リモート AI ジョブの待ち 1 秒→10 秒、`bf6b36993` 類似検索の通知の取り違え、`fdffa62a7` Remote 見開き受付の data-dir 隔離)。いずれも製品コードの変更なし。
- **依存**: PDFium を `chromium/8086` から **`chromium/8100` へ更新**。tantivy を 0.26.1 から **0.26.2 へ更新** (vendor 上に upstream 差分を適用、ローカル patch のファイルは無変更)。
  LibRaw 0.22.2・EffeTune v0.13.0 は最新。FFmpeg は従来と同じ誤検知 (`n7.1.5-12` を「新版」と表示、同梱 `n7.1.5-16` の方が新しい)。
  見送り (egui 0.36 と一緒に次の次の版で): ONNX Runtime 1.24.4 (TensorRT パックの版固定・VC runtime gate と連動)、ort rc.14、VST3 SDK 3.8.1、Rust 1.99、一般クレート 155 件。
- **検索 bench**: tantivy 更新のため実施。初回は他の作業ツリーのビルドと重なり 5 クエリが閾値超え。同じ負荷条件で 0.26.1 / 0.26.2 を交互に測り直すと、0.26.2 は回帰なし (大半のクエリで速い)。負荷時の値で baseline は更新していない。
- **自動テスト**: 配布ビルドの全体テストで 72 本 0 失敗 (lib 13,201 passed)。1 回目は 2 件で停止: 「重要な変更点」への v4.7.0 追加による起動画面 snapshot の更新漏れ (`044fbab7d`) と、Remote の見開き受付テストの data-dir 隔離漏れ (`fdffa62a7`、製品影響なし)。
- **ReleaseChecks (ClaudeCode が実行、利用者了承)**: run `release-checks-20261015-041146-6821b7a2` で **PASS / 警告なし**。1 回目は利用者が PC 操作中で前面化できず SKIPPED (環境不成立)。
  idle 5 ケースとも CPU 1 コア比 0.003〜0.005、測定区間の frame 0。perf cached / uncached とも 256 folder load・Ctrl+Down 5・検索完了、描画 frame の 16ms 未満 99.67%、pre_grid raw p95 0.07ms、100ms 超 0 件。AI は未実施 (AI 経路の変更なし)。
- **panic.log**: 52 件 15 種すべて disposition あり (`check-panic-log.ps1` exit 0)。
- **署名**: 単体exe / setup.exe / portable の mimageviewer.exe を `signtool verify /pa /v` で確認 (Open Source Developer Taku Sano、Certum チェーン、RFC3161)。全 PE / VC runtime gate 通過。
- **portable smoke (ClaudeCode が実施)**: `D:\miv-portable-smoke-v470` に展開して起動。応答・`data\` 生成・APPDATA の設定が変わらないこと・終了を確認。初回は利用者が別調査で起動していた別の portable に二重起動の転送をして終了 (`second-instance-activation`、想定どおり)。
- **当日の実機確認 (利用者)**: 本の構成の見開きキー、ページめくり (空いた側を黒い紙でめくる)。余り単ページの位置と解除確認ダイアログは ClaudeCode が隔離データで確認。

## §1.361 Phase 4 初回隔離 run（2026-10-11 JST、同日調査）

- **対象・出所**: 利用者が報告した了承済みの初回 run
  `C:\home\mimageviewer\target\ui-smoke\runs\release-checks-20261010-151540-f99fc454`。
  run-id の日付・時刻は UTC（2026-10-10 15:15:40）、日本時間では2026-10-11 00:15:40。
  本調査では既存の `seed-profile/mimageviewer.log` と `release-record.md` を読み取り、アプリを再実行していない。
- **未完了**: seed のログは `runner start failed: screenshot directory must be one run below ...ui-smoke-runs`
  による EnvironmentFailure / 2 を記録。元の release-record は seed FAIL / application exit code 1 と記録しており、
  process ExitCode 取得の修正前の結果なので一致しない。元証跡を変更せず、suite PASS／採用証拠として使わない。
- **環境注記（Remote IPC）**: 同じログの起動時 t=0.052s に、共有
  `\\.\pipe\mimageviewer-remote-thumbnail` の作成拒否（access denied / OS error 5）がある。
  常駐インストール版が存在する条件と `FILE_FLAG_FIRST_PIPE_INSTANCE` の同名二重起動拒否に一致する。
  source inspection では pipe 作成は Remote worker 起動より前で、失敗時は listener／service を作らず、
  ローカル操作 phase を維持する（`src/remote_ipc/pipe.rs`、`src/lib.rs`、`src/remote_ipc/ui.rs`）。
  seed／idle 5ケース／perf 2ケース／任意の first-AI はアプリ内 test-script queue と exact PID／HWND の
  観測を使い、Remote IPC・Remote service・既存インスタンスへの open 転送を使わない。
  このエラー自体は対象ケースの操作や必須証拠を妨げない。起動時の一回の診断で、idle の log 増加測定前に出る。
  Remote worker 不在と常駐版の同時稼働は環境条件として残し、配布版の厳密性能／Remote 動作の証明には使わない。
  診断の除外、pipe 分離、retry、既存インスタンス停止は追加していない。
- **修正後**: 非対話チェックの結果は `target/F-phase4-fix-msg.txt` に引き継ぐ。
  修正後の実アプリ run、各3回の成立率／閾値、tray 旧版FAIL／修正版PASS の採用証拠は未取得。

## v4.5.0 (2026-10-09)

- **対象**: 配布ビルドは master `16524e981`。その後のコミットはバックログ (§1.360) とこの記録だけで、コードは配布ビルドと同一。
- **依存**: PDFium `chromium/8086`・LibRaw 0.22.2 とも最新。FFmpeg は `setup-ffmpeg.sh check` が `n7.1.5-12` を「新版」と出すが、同梱の `n7.1.5-16` の方が新しく更新なし (従来と同じ誤検知)。
- **自動テスト**: `test-full.ps1 -SuppressCrashDialogs` で **12,636 passed / 0 failed** (64 suites)。配布ビルドは同一ソースのため `build-dist.ps1 -SkipRustTests`。
  VST3 host はソース変更があったため再ビルドし、identity `a5bd01e9…` を確認。ツリー外バックアップも差し替え (旧は `.before-v450.exe`)。全 PE / VC runtime gate 通過。
  開発中の lib 全体実行で、負荷時だけ落ちて単独再実行で通る動画 EOF ループ試験を観測。全体ゲートでは発生せず。
- **署名**: 単体exe / setup.exe / portable の mimageviewer.exe を `signtool verify /pa` で確認 (Taku Sano 名義、RFC3161)。
- **portable smoke**: `D:\miv-portable-smoke-v450` に展開して起動。応答・`data\` 生成・APPDATA 非接触・終了を確認 (ClaudeCode が実施)。
- **panic.log**: 52 件 15 種すべて disposition あり (`check-panic-log.ps1` exit 0)。CI は 10/07 から viewer context 監査で赤、修正 `a03777b0f` は公開時の push で反映。
- **idle health (配布ビルドの core、利用者が実施)**: `static-foreground` / `static-background` / `video-pin-background` (`c:\home\youtube\movie`) とも **PASS**。
  測定区間の perf event 0 件 (完全 sleep)、CPU 1 コア比 0.0062 / 0.0104 / 0.0094。video-pin は準備中に `idle_upgrade_ineligible` 2 件で条件成立。
  `tray-residency` は **FAIL** (`G:\home\comfyui`、CPU 0.44、フォルダのタイルを再投入し続ける)。v4.4.0 から入った退行ではなく既存の欠陥 (Codex 調査) で、
  v4.4.0 は別フォルダ (キャッシュ済み) で測っていたため出なかったと見ている。利用者判断で出荷し、次の版で優先して直す (backlog §1.360)。
- **perf smoke (利用者が操作)**: 2,642 フレーム。直前が `request_repaint` の描画フレーム 1,271 件中 **98.0% が 16ms 未満** (v4.4.0 は 98.4%)。
  `ui.pre_grid_breakdown` total p50 0.04ms / p95 0.12ms / max 7.4ms。100ms 超の間隔 21 件のうち 19 件は `none` / `request_repaint_after_idle_upgrade` (正常)。
  残る 2 件: (1) 起動直後 t=2.3s の 591ms は初期フォルダの同期読み込み (`lf_scan` 572ms、522 フォルダ、`pre_scanned=false`)。tail は `none` だが update 自体が 578ms。
  起動時の初期フォルダ読み込みは従来から UI スレッドで行っている (v4.4.0 の「起動直後 1 件」と同じ種類)。(2) t=10.4s の 518ms は `request_repaint_after_ai_upscale` で
  `fullscreen_viewport` 500ms。v4.4.0 の初回 AI アップスケール開始時の 529ms と同じ形 (GPU 競合と推定、未確認)。
- **検索 bench**: 全文索引に触れていないため未実施。

## v4.4.0 (2026-10-07)

- **対象**: master `7f922b612` (配布ビルド後の追加コミットは docs / htdocs のみ。コードは配布ビルドと同一)。
- **依存**: PDFium を `chromium/8076` から **`chromium/8086` へ更新** (利用者が更新後の確認ビルドで PDF 表示を確認)。
  FFmpeg は `setup-ffmpeg.sh check` が `n7.1.5-12` を「新版」と出すが、同梱の `n7.1.5-16` の方が新しく更新なし (従来と同じ誤検知)。
  LibRaw 0.22.2 は最新。対応ソース tarball は SHA-256 照合済みで htdocs に配置 (mikage.to への公開は利用者)。
- **自動テスト**: `build-dist.ps1 -SkipVst3Bridge` の全体テストで **12,116 passed / 0 failed / 58 ignored** (63 suites)。
  VST3 host は現ソースの identity (`b981fb61…`) と一致を確認して再利用。全 PE / VC runtime gate 通過。
  開発中の lib 全体実行で、負荷時だけ落ちて単独再実行で通る 2 件を観測 (`indexer_manager::…supervisor_watcher…`、
  `similar_index::…cannot_reuse_old_marker`)。配布ビルドの全体テストでは発生せず。
- **署名**: 単体exe / setup.exe / portable の mimageviewer.exe を `signtool verify /pa` で確認 (Taku Sano 名義、RFC3161)。
- **portable smoke**: `D:\` に展開して起動。応答・`data\` 生成・APPDATA 非接触・終了を確認 (ClaudeCode が実施、
  利用者の実アプリ試験許可の範囲)。EffeTune は portable 非同梱のため BundleMissing (仕様)。
- **実機 (利用者)**: §1.335、§1.328 (ZIP と ★3 一覧 → フォルダの ← 復帰)、最終確認ビルドで PDF と通常操作。
  初回設定の小画面: サブ PC の Windows Sandbox 1093×614 で、修正版は見出しと「開始」が表示、v4.3.0 は表示されない。
- **ui-smoke**: RatingFolderBack (600 行・10 列) のツールバー 6 ケース PASS。キー系は前面を取れず SKIPPED。
- **panic.log**: 52 件 15 種すべて disposition あり (`check-panic-log.ps1` exit 0)。CI は最終 push 時に確認する。
- **idle health (配布ビルドの core、利用者が実施)**: `static-foreground` / `static-background` / `tray-residency` とも **PASS**。
  測定区間の perf event 0 件 (完全 sleep)、CPU 1 コア比 0.0125 / 0.0083 / 0.0042、`tray-residency` は `8/0 visible`。
  `video-pin-background` は未実施 (waiver): v4.3.0 からアイドル高画質化 (`idle_upgrade`) のコードに差分なし。
- **perf smoke (利用者が操作)**: 5,392 フレーム。直前が `request_repaint` の描画フレーム 2,417 件中 **98.4% が 16ms 未満**。
  `ui.pre_grid_breakdown` total p50 0.04ms / p95 0.09ms / max 1.66ms。100ms 超の間隔 32 件のうち 30 件は `none` /
  `request_repaint_after_idle_upgrade` (正常)、1 件は起動直後。残る 1 件は 7.5s の 529ms で直前が
  `request_repaint_after_ai_upscale`、内訳は `fs_render_breakdown.viewport_render_ms` 508ms (同期 I/O の段ではない)。
  直後に AI のタイル推論が始まっており、初回 AI アップスケール開始時の GPU 競合と推定 (未確認)。AI のコードは v4.3.0 から変更なし。
  v4.3.0 でも同じかは比較していない。
- **見送り**: 案A 段2 (初期フォルダの UI スレッド外化) はリリース後。§1.339 (★一覧の絞り込みが → で ZIP に残る) は次版。

## v4.2.0 (2026-09-29)

- **依存**: PDFium `chromium/8066` (最新)。FFmpeg は `setup-ffmpeg.sh check` が `n7.1.5-12` を「新版」と出すが、
  現行 `n7.1.5-16` より古い (v3.4.0 と同じ誤検知)。DLL は v4.1.0 と同一で更新なし。ONNX Runtime / Susie / VST3 bridge は
  v4.1.0 から変更なし (`-SkipVst3Bridge`)。`check-panic-log.ps1`: 52 件 15 種すべて disposition あり。
- **CI**: この版の公開準備中に ubuntu の非 Windows cfg job を**廃止** (利用者判断、backlog §1.302)。残る fmt / viewer context audit は
  master / main とも success。audit は EPUB / フォルダ履歴で増えた registry API の A4 allowlist 登録漏れで一度赤 (`aaa92558f` で登録)。
- **Rust テスト**: 配布ビルド内の全体テストで **10,688 件通過 / 失敗 0** (`be04889d4`)。
- **ui-smoke (使い捨て portable)**: 7 本すべて PASS。AlwaysOnTop は 2 つの理由で落ちた:
  (1) 別ウィンドウは読み込み中の枠とページ表示で 2 回フォーカスを取り、smoke が 1 回目の後に root へ戻って OS の Right キーを
  送っていた → 待ち合わせを「ページ描画済みかつ前面」まで延ばして 4 回連続 PASS (backlog §1.303)。
  (2) Claude デスクトップのウィンドウが同じモニターで smoke の座標に重なり、実ポインタ / キーが届かなかった → 別モニターへ移して解消。
  **smoke 中は Claude のウィンドウを別モニターに置く。**
- **idle health (配布ビルドの core)**: `static-foreground` / `static-background` / `tray-residency` の 3 シナリオとも **PASS**。
  全区間で perf event 0 件 (完全 sleep)、CPU 1 コア比 0.0062 / 0.0062 / 0.0187、`tray-residency` は `8/0 visible`。
  `video-pin-background` は未実施 (waiver): v4.1.0 からアイドル高画質化 (`idle_upgrade`) のコードに差分なし。
- **perf smoke**: 3,563 フレーム、16ms 以上 189 件、ログは `target/release-verification/v4.2.0-perf_smoke-perf_events.jsonl`。
  直前が `request_repaint` の描画中フレームは n=1,696 で **16ms 未満 96.6%、p95 12ms、max 133ms**。
  `pre_grid_breakdown.total_ms` は **p50 0.062 / p95 0.131 / max 2.81ms**。100ms 超 36 件の内訳は `none` 15、
  `request_repaint_after_idle_upgrade` 14、`request_repaint_after_ai_upscale` 1、起動直後 1、`request_repaint` 5。
  その 5 件はフルスクリーン viewport の `setup_ms` 96〜120ms (2 件)、初回フォルダの `thumbnail_polls_ms` 94ms (1 件)、
  コレクションを開いたフレーム (read_lease の `active_ms` 92ms は lease の存続期間で UI 停止時間ではない、1 件) など。
  **説明のつかない 100ms 超は 0 件。**
- **配布物の確認で見つかったリリース阻害 (修正済み)**: 利用者がポータブル版 (空の data) で、一覧で PDF のサムネイルを作ってから
  その PDF を開くと、ページのサムネイルが永久に「読込中」。キャッシュ済み PDF の即時表示 (§12) で、ページ数の照合成功時に
  採用処理をやり直して thumbnail worker を止めていた (`b5e3c2340`、独立レビュー 3 回で ACCEPT、`76926e007`)。
  普段の環境はページのサムネイルがキャッシュ済みで表に出なかった。**空の data での配布物確認が捕まえた。**
  修正後の配布ビルドで利用者が PDF と動画を確認。

---

## v3.4.0 (2026-09-01)

- **依存**: PDFium を `chromium/8021` → **`chromium/8035` へ更新**。FFmpeg は
  `setup-ffmpeg.sh check` が「新版あり」と言うが**誤検知**で、提示された
  `n7.1.5-12-g1fdbca85aa` は現行 `n7.1.5-16-g9a4bb2c579` より**古い** (12 < 16、
  文字列比較のため)。DLL の ProductVersion は `n7.1.5-16-g9a4bb2c579-20260816` で
  `vendor/ffmpeg/VERSION` と一致しており、LGPL 対応ソースのズレも無し。更新不要。
  VST3 bridge は C++ ソースが 2026-05-16 から変更無しで再ビルド不要 (`-SkipVst3Bridge`)。
  Susie ワーカーは再ビルド済み (ソース変更無し)。
- **非 Windows ビルド**: `check-non-windows-shadow.ps1` で **PASS**。
  master は未 push だったので GitHub Actions は前版のまま = このローカル検査が代替。
- **Rust テスト**: `test-full.ps1` で **7,846 件通過 / 失敗 0**。
- **idle health**: `static-foreground` / `static-background` / `tray-residency` の
  **3 シナリオとも PASS**。全区間で perf event 0 件 (完全 sleep)、CPU 1 コア比は
  0.0062 / 0.0083 / 0.0167。`tray-residency` は `8/0 visible`。
  **`video-pin-background` は未実施** — 動画を代表画像に固定したフォルダを用意できず。
  この版はアイドル高画質化の経路に触れていないため waiver とした。
- **perf smoke**: フレーム 5,112 / 16ms 以上のギャップ 94 件 /
  ギャップ p50 27ms・p95 314ms・max 483ms。
  **100ms を超えた 15 件はすべて説明が付いた**: `request_repaint_after_idle_upgrade`
  11 件 (実測ギャップが `idle_upgrade_delay_ms` 222〜447ms と整合)、`none` 3 件 (就寝)、
  最初の `tail_repaint` より前 1 件 (起動直後の `load_folder_end=145.6ms`)。
  **それ以外の 100ms 超は 0 件。**
  `pre_grid_breakdown.total_ms` は **p50 0.129 / p95 0.183ms** (健全基準 0.3ms 以下)。
  max 37.98ms は `Ctrl+G` で検索バーを初めて開いた 1 フレームの
  `global_search_bar_ms=37.75` のみで、2 番目は同じ場所の 2.30ms。

- **v3.3.1 が「次版から見る」とした「描画中のみの比率」は取れなかった。**
  perf smoke の解析後にアプリを再度動かしたため、同じ `perf_events.jsonl` に別セッションが
  追記され、`analyze_perf.py` が見た 5,112 フレームの区間を後から復元できなくなった
  (先頭 5,112 件で切っても件数が合わない)。
  **次版からは、smoke 直後に perf log を版名付きでコピーしてから解析する。**

### この版の test gate が見つけたもの

- **`FtsWriterDispatcher` の起床通知取りこぼし (2026-04-22 混入、v2.x から出荷済み)**。
  test gate が `ingest_worker::tests::cancel_mid_ingest_flushes_partial_and_returns_cancelled`
  で 47 分ハング。**プロセスが生きているうちに cdb で両スレッドのスタックを採取**して確定
  (テスト側 = `Drop` → `JoinHandle::join`、dispatcher = `Condvar::wait`)。
  `Drop` が停止フラグを Mutex の**外**で立ててから notify していたため、dispatcher が
  「述語を確認したが wait に入る前」の窓に落ちると通知が失われる。フラグを `Queue` の
  フィールドへ移して構造的に解消 (`e935c79b`)。本番では最後の `Arc` が落ちる場所
  = 終了時に踏み得るので、「アプリが終了しきらない」症状として出ていた可能性がある。
- **`rotation_db` / `book_resume_db` のテスト分離不備** (`af60acff`)。
  data_dir の test override を自分で張らず、同時に走る別テストのガードへ相乗りしていた。
  ガードが先に落ちると temp dir ごと消えて `CannotOpen`。他 worktree で cargo が 4 本
  走っている負荷下で初めて出た。**他 5 ファイルはガードを張っており、この 2 件だけ。**

---

## v3.3.1 (2026-08-30)

- **依存**: PDFium `chromium/8021` は最新で更新なし。**FFmpeg を
  `n7.1.5-12-g1fdbca85aa` → `n7.1.5-16-g9a4bb2c579` へ更新した** (v3.3.0 では見送った差分)。
  4 コミットのうち 2 件が H.264 のフレームスレッド同期の修正
  (`h264_direct` の colocated field pair 待ち合わせ、`pthread_frame` の side data 同期) で、
  デコーダの正しさに関わるため取った。DLL 名は不変 (`avcodec-61` 等) なので
  `ffmpeg_loader.rs` / `build.rs` の変更なし。更新前の DLL ProductVersion は
  `vendor/ffmpeg/VERSION` と一致していて腐っていなかった。対応ソースは commit 指定で取得し、
  sha256 をツリー外バックアップと照合済み。VST3 bridge / Susie は C++ / Rust ソースに
  変更が無いので再ビルド不要。
- **非 Windows ビルド**: `check-non-windows-shadow.ps1` で **PASS** (エラー 0)。
- **Rust テスト**: lib 6,742 + 統合 18 本 + UI スナップショット 43、すべて緑。
- **idle health**: 4 シナリオとも **PASS**。全区間で perf event 0 件 (完全 sleep)。
  CPU 1 コア比は 0.0104〜0.0344。`video-pin-background` は
  `idle_upgrade_ineligible=1` / `matched=31` でセットアップ成立を確認。
  `tray-residency` は `8/0 visible` (全ウィンドウ非表示)。
- **perf smoke**: 2 回取った。**どちらも 100ms 超のギャップに説明のつかないものは 0 件。**

  | | フレーム | 区間 | 16ms 未満 (全体) | 16ms 未満 (描画中のみ) | 描画中 p95 / max |
  | --- | --- | --- | --- | --- | --- |
  | 1 回目 (動画サムネの多いフォルダ) | 3,236 | 46.0s | 89.6% | 75.6% | 23 / 47ms |
  | 2 回目 (キャッシュ済みフォルダ) | 834 | 11.2s | 91.4% | **92.4%** | 17 / **25ms** |

  - 100ms 超の内訳は両方とも `none` (就寝) / `request_repaint_after_idle_upgrade`
    (予定どおりの遅延 wake。実測ギャップが `idle_upgrade_delay_ms` と整合) /
    最初の `tail_repaint` より前、のみ。
  - `pre_grid_breakdown.total_ms` は **p95 0.2〜0.3ms**。1 回目の max 42ms は
    Ctrl+G で検索バーを開いた 1 フレームの `global_search_bar_ms` のみで、他は 0.5ms 以下。
  - 1 回目が重いのはサムネイル処理と重なるフレーム (402 枚で 69.7%) に集中しており、
    その回は 46 秒で **6,961 件**の thumb イベントが出ていた (2 回目は 2,070 件)。

- **この版で分かったこと: 「16ms 未満が 97% 以上」という目安は、セッションの長さと
  内容が揃っていないと比較に使えない。**
  この比率は**正しい就寝をヒッチとして数える**ので、短い・暇なセッションほど悪化する。
  実際 2 回目 (11 秒) は健全そのものなのに 91.4% で、v3.3.0 の 97.3% (4,708 フレーム) と
  並べると退行に見えた。並べてよいのは **`request_repaint` が直前にある「実際に描いている
  フレーム」だけに絞った比率**で、こちらは 2 回目 92.4% / 最大 25ms と明確に健全。
  次版からは全体比率ではなくこちらを見る。

## v3.3.0 (2026-08-29)

- **依存**: PDFium `chromium/8021` は最新で更新なし。FFmpeg は現行
  `n7.1.5-12-g1fdbca85aa` に対し `n7.1.5-16-g9a4bb2c579` が出ていたが**更新しない判断**
  (同じ 7.1.5 系で 4 コミット差、DLL 名も不変。更新すると LGPL 対応ソースの再配置と
  製品ページ改訂、動画再生の再確認が付く)。DLL の ProductVersion は `vendor/ffmpeg/VERSION`
  と一致していて腐っていない。VST3 bridge は C++ 最終変更 2026-05-16 に対し exe が
  08-23 ビルドなので再ビルド不要 (`-SkipVst3Bridge`)。
- **Rust 全体テスト**: `test-full.ps1` で **7,604 passed / 0 failed** (exit 0)。
- **idle health**: 4 シナリオ (`static-foreground` / `static-background` /
  `video-pin-background` / `tray-residency`) とも **PASS** (利用者実施)。個々の数値は
  この記録には残していない。
- **perf smoke**: 4,708 フレーム中、間隔 16ms 以上が 126 件 = **16ms 未満が 97.3%**
  (v3.1.0 の 97.74% / v2.9.1 の 97.7% と同等)。100ms 超は 24 件:
  - `action: none` (入力待ちで就寝) **16 件** — 正常
  - `request_repaint_after_idle_upgrade` **5 件** — 正常。実測ギャップが
    `idle_upgrade_delay_ms` とほぼ一致 (418.9ms / 433.8ms など)
  - `request_repaint` **2 件** — 原因特定済み。t=1.18s の 178ms は
    プロセス最初の保存に伴う世代ローテ (`sli_settings_save` 129ms)、t=2.39s の 488ms は
    PDF サムネイル生成中 (1 ページの `render_ms` 408ms)。perf_smoke 自身が
    「PDF cold open 等が紐付いていれば許容」としている類型
  - 最初の `tail_repaint` より前 1 件
  - **説明のつかない UI スレッド同期 I/O は無し。**
- **この版で新たに分かったこと (次版対象、§1.0b / §1.0c)**:
  - `Settings::save()` が**フォルダ移動 1 回ごとに 43〜61ms**。38 回の移動で計 1.8 秒。
    原因は `settings.db` が 36.7 MB で、その 98% が VST3 状態 BLOB (`vst3_plugins` 7 行
    32.04 MB + `vst3_chain_slots` 2 行 4.16 MB、他すべてで 0.2 MB)。**行自体は
    ハッシュ比較で書き直していない**が、そこへ至る clone 2 回・JSON 値構築・ハッシュに
    43ms を払っている。**v3.2.0 に同じコードがあり退行ではない。**
  - 合成データでの `save_full` 実測は既定設定 1.3ms / +500 resume positions 2.2ms /
    +1MB VST3 状態 17.4ms、`rotate_backups` 10 世代 6.9ms、`Settings::clone` 34µs。
    **実機の 43ms を 30 倍過小評価していた。** 次に同種を測るときは実機の settings.db で測る。
  - `settings.db*` が **124 ファイル・2,980 MB** (`preupgrade` 36 個 1,053 MB /
    隔離 22 個 775 MB / その他 55 個 748 MB / `bak1..10` 367 MB / main 37 MB)。
- **補正マスクの圧縮 (R-07 判断材料)**: release ビルド実測で `encode_alpha` は
  0.8MP 2.4ms / 3MP 8.1ms / 12MP 33ms / **24MP 70.6ms**。v3.2.0 の形式 (素の JSON 数値配列)
  は同条件で 11.9 / 49.4 / 195 / **410ms** なので、**v3.3.0 の codec 変更が 5.8 倍速く・
  600 分の 1 の大きさにした後の値**。マスクは画像原寸 (`local_adjust_image_dims`)。
  レイヤー文書の 1 複製は 3MP 3.5ms / 12MP 14.3ms / **24MP 27.7ms (96 MB)**。
- **Remote の寸法 probe (R-23 判断材料)**: `page_dims_without_catalog` はローカル SSD で
  初回 454µs/件・キャッシュ後 23µs/件。1,000 件で 0.45 秒、10,000 件で 4.5 秒。
  open のコストが支配的なので HDD / NAS では 1 件 1〜10ms に伸びる。
- **配布物の回帰チェック**: `dumpbin /dependents` で launcher / core / remote / portable の
  4 本とも `VCRUNTIME140.dll` / `MSVCP140.dll` **なし**。`signtool verify /pa` で単体exe /
  setup.exe / portable の 3 本とも VALID + RFC3161 タイムスタンプ、チェーンは
  `Certum Trusted Network CA → CA 2 → Certum Code Signing 2021 CA → Open Source Developer Taku Sano`。
- **検索 bench**: 全文索引に触れていないため未実施。

## v3.1.0 (2026-08-16)

- **依存**: PDFium `chromium/7988` / FFmpeg `n7.1.5-12-g1fdbca85aa` とも更新なし。DLL の
  ProductVersion が `vendor/ffmpeg/VERSION` と一致、LGPL 対応ソースの差し替え不要。
- **idle health**: `static-foreground` / `static-background` とも **PASS**。どちらも測定 15 秒の
  区間で perf event 0 件 = 完全 sleep。CPU one-core ratio は 0.0052 / 0.0156、perf ログ増加 0 バイト、
  通常ログ増加 312 / 186 バイト。
  - `video-pin-background` は**未実施**。`-TargetKey` に渡せる「動画を代表画像に固定した
    フォルダ」を用意しなかったため。この版はアイドル高画質化の経路に触れていないので waiver。
    §5.4 (evidence 窓が狭い) が片付けば準備の負担も下がる。
- **perf smoke**: 6060 フレーム中、間隔 16ms 以上が 137 件 = **16ms 未満が 97.74%**
  (v2.9.1 の 97.7% と同等)。100ms 超は 28 件で、内訳は `action: none` (入力待ちで就寝) 5、
  `request_repaint_after_idle_upgrade` (予定どおりの遅延起床) 5、`request_repaint_after_ai_upscale` 4、
  `request_repaint` 13、起動直後 1。
  - `request_repaint` の 13 件は全て t=15〜21 秒のフルスクリーン中。`fullscreen_viewport_ms`
    98〜133ms / `background_polls_ms` 26〜68ms に対し、**描画クロージャ本体
    (`fs_viewport_breakdown` の `central_ms` / `closure_ms`) は 1ms 前後**。時間は viewport の
    present とバックグラウンド結果の取り込み (テクスチャアップロード) にあり、AI アップスケール中の
    設計どおりのコスト。UI スレッド同期 I/O の追加ではない。
- **検索 bench**: 全文索引に触れていないため未実施。

## v2.13.0 (2026-08-11)

- **依存**: PDFium `chromium/7988` / FFmpeg `n7.1.5-12-g1fdbca85aa` とも最新で更新なし。
  DLL の ProductVersion `n7.1.5-12-g1fdbca85aa-20260803` が `vendor/ffmpeg/VERSION` と一致。
  LGPL 対応ソースの差し替え不要。VST3 bridge は C++ 最終変更 2026-08-01 / exe ビルド
  2026-08-06 なので再ビルド不要 (`-SkipVst3Bridge`)。Susie ワーカーは再ビルドした。
  - ⚠ `setup-pdfium.sh check` は「新しいバージョンあり」と誤検知する。`vendor/pdfium/VERSION`
    が無い環境では現行版を「未インストール」と読むため。**DLL の FileVersion で判定する**
    (`153.0.7988.0` = `chromium/7988`)。FFmpeg の VERSION 腐りと同じ型。
- **CI**: master success (run 31402289417)。ubuntu の `cargo check` を含む。
  今サイクルの 114 commit が初めて CI を通った。タッチ対応は `#[cfg(windows)]` の塊なので
  ここが最大の未検証点だったが、漏れは無かった。
- **署名 / 依存 DLL**: 配布 4 種すべて `Certum Trusted Network CA` + RFC3161 タイムスタンプ。
  `dumpbin /dependents` で launcher / core / portable / susie32 とも
  `VCRUNTIME140.dll` / `MSVCP140.dll` なし。
- **idle health**: `static-foreground` / `static-background` とも PASS。
  測定窓は perf event 0 件・perf log 増加 0 バイト (完全 sleep)、CPU one-core ratio
  0.0219 / 0.0115。
  - **1 回目の `static-foreground` は FAIL (35 fps) だったが退行ではない**。起動から約 2 分の
    時点で、索引作成が進行中かつお気に入り編集ダイアログを開いたまま測っていた。
    同ダイアログは進捗を流すため 100ms ごとに repaint を要求する既存仕様
    ([favorites_editor.rs](../src/ui_dialogs/favorites_editor.rs) の live 更新)。
    ダイアログを閉じて測り直すと 0 フレーム。**「過去 3 回は 0 フレーム」という比較だけで
    退行と断定してはいけない**。アプリが busy でなかったことを先に確認する。
  - `video-pin-background` は **§5.4 の既知のゲート欠陥で不成立**。証拠窓が測定区間そのもの
    (t=352.4-367.4) になり、対象キーの thumbnail work は t=337.5 で 15 秒前に終わっていた
    (`matched=0`)。加えて操作側の誤りもあり、ピン留めフォルダの**中**を表示していたため
    マッチしたのは個々の動画ファイル (`from_cache=false` = アイドル高画質化の候補外) で、
    フォルダタイル (`dir::<path>`) ではなかった。**waiver とする**根拠は、
    v2.13.0 が `idle_upgrade` を含む行を 1 行も変更していないこと
    (`git log -S "idle_upgrade" v2.12.0..HEAD -- src/` が 0 件)、および測定区間自体は
    完全 sleep (CPU 0.0052) だったこと。セッションログは
    `target/idle-health/v2.13.0-idle-health-session.jsonl` に退避済み。
- **perf smoke**: frame 4476、16ms 超のギャップ 132 件 (97.05% が 16ms 未満)。
  **件数ではなく直前の `ui.tail_repaint.action` で判定**した結果:
  - 100ms 超は 18 件。うち 11 件が `none` (repaint 未要求 = 入力待ちで就寝)、
    6 件が `request_repaint_after_idle_upgrade` で `idle_upgrade_delay_ms` がギャップ長と
    一致 (予定どおりの起床)
  - **`request_repaint` が立っていた 100ms 超は 1 件のみ**: 起動直後 t=1.109s の 198ms
    (`reasons=['requested_nonempty']` = サムネイル要求中)。今サイクルの新経路とは無関係
  - 100ms 超の件数は v2.12.0 の 53 件・v2.9.1 の 24 件より少ない
- **検索 bench**: 全文索引に触れていないため未実施。

## v2.12.0 (2026-08-06)

§5.5 の「記録が残らない」を繰り返さないための実測記録。

- **依存**: PDFium `chromium/7988` / FFmpeg `n7.1.5-12-g1fdbca85aa` とも最新。DLL の
  ProductVersion `n7.1.5-12-g1fdbca85aa-20260803` が `vendor/ffmpeg/VERSION` と一致。
  PDFium MAJOR 152 → 153 の実機確認 (通常 / パスワード付き PDF、動画 / 音声) 済み。
- **CI**: master / main とも success (run 31021319941 / 31021332945)。ubuntu の
  `cargo check` を含む。今サイクルの 32 commit が初めて CI を通った。
- **idle health**: `static-foreground` / `static-background` とも PASS。CPU one-core ratio
  0.0073 / 0.0083、測定窓は perf event 0 件 (完全 sleep)。`video-pin-background` は
  条件を満たすフォルダを用意しなかったため未実施。アイドル高画質化経路は今サイクルで
  触っていないので waiver とする。
- **perf smoke**: frame 7968、16ms 超のギャップ 1376 件。**件数ではなく直前の
  `ui.tail_repaint.action` で判定**した結果:
  - 16-100ms 帯 1323 件のうち **1259 件が `none`** (repaint 未要求 = 入力待ちで就寝)。
    p50 85ms は Ctrl+↓ 連打時のキー間隔そのもの
  - 100ms 超 53 件のうち 42 件が `none`、7 件が `request_repaint_after_idle_upgrade`
    (予定どおりの起床)。最大の 33.7s / 30.3s / 20.1s / 19.6s は idle health の測定窓
  - **`request_repaint` が立っていた 100ms 超は 3 件のみ**: 起動直後 356ms、
    PDF を含むフォルダ読み込み 585ms + 106ms (`load_folder_end=55ms` +
    thumb decode + `pdf/pool_promote_visible`)。今サイクルの新経路とは無関係
  - ⚠ 同一フォルダでの v2.11.0 基準値は未取得なので「悪化していない」とは言えない。
    次サイクルで PDF フォルダの folder-load ギャップを基準値化するか判断する
- **検索 bench**: 全文索引に触れていないため未実施 (ファイル名フィルターは一覧の絞り込み)。

## v2.9.1 (2026-07-30) — perf smoke を最終 tree で取れていない

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、ClaudeCode)。本セッションで確認済み。
- 何が起きたか: perf smoke の記録は 2026-07-31 の tree のものしか無く (`17231956` は
  CLAUDE.md の判定基準を書き足した commit で、計測そのものではない)、**8/1 に入った構造変更の
  後に取り直した記録が見当たらない**。対象の変更は入力所有権 (`56db11bc` / `596bb65d`)、
  `FsPageLoadState` (`9590b661`)、health watchdog (`2c11e4cd`)。
- ずれている扱い: idle-health 側は §5.4 の waiver を「未実測部分を明示する」形に書き直したのに、
  perf smoke 側は同じ扱いになっていない。CLAUDE.md Phase 2 の 9.6 は「UI 周り / I/O 経路に
  変更を入れたリリースで実施」なので `FsPageLoadState` はまさに対象。
- 対応 (どちらか):
  1. v2.9.1 の tree で perf smoke を取り直す。**PDF フルスクリーンでの計測を含める**
     (§1.39-2 の毎フレーム経路がそこにしか出ない)。
  2. 取らない判断を §5.4 と同じ形で明文化する (何が未実測かを書く)。
- 規模 / 優先度: Small / P2 (プロセス)。次版のリリース前確認までに片付ける。

---

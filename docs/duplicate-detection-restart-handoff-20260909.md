# 再起動前の作業引き継ぎ（2026-09-09 19:36 JST）

利用者が PC 再起動を希望したため、安全な区切りで作業を停止する。再起動後はこの記録と docs/duplicate-detection-feedback-20260909.md を読む。未コミット変更を破棄・一括 stage しない。

## 作業場所・分担

- C:\home\mimageviewer-dupe / duplicate-detection
- HEAD: cfb849e361e321d0e52878d9ec0ed3ffece20743（前回の文書専用コミット）
- 親は設計・進行・文書。実装/テストは Sol / xhigh、独立レビューは別の Sol / xhigh。max/ultra は使わない。親 GUI 設定を独立確認したとは断定しない。
- 既存実装担当 merge_release360、独立レビュー review_sol。review_core（旧 Astra）へ新しい依頼を出さない。
- v3.7.0 の master は既に統合済み。再マージ・逆統合・push は今回不要。

## 最新の未解決症状

「この本と重なる本」の帯の左上 [移動] を押しても移動しない。追加報告では E:\share\18\doujin\__new\266707\1.jpg から、画像候補に明示された __new5\266707\1.jpg の [移動] でも HUD が __new のまま。両指定ファイルは Test-Path のみで実在確認済み。画像内容/DB は変更していない。

本候補には場所が表示されず同名本を区別できなかったため、本名の下に full container path を DIM 表示する改善を実装した。症状自体は未再現・未修正。音声途切れは今回再現しなかったと利用者報告、原因未確定のまま継続利用。追加音声作業は行わない。

## 調査結果と棄却していない仮説

- 実 small_button press/release → production dispatcher → 別物理フォルダ controlled scan → exact target Display/SimilarBookVisit は PASS。
- 実 fullscreen 外枠（fs_click/navigator/touch/固定パネル/Similar）でも PASS。同名本・同名ページで親 __new / __new5 を変え、stale の索引更新中表示、旧 Display Awaiting owner を加えても PASS。
- full path の item_key/target は別親を区別し、単純な同名衝突は確認されていない。
- existing-item の key OR target 探索に DTO 不整合時の設計穴があるが、不整合は未観測。今回その選択意味を変更しない。
- same-items の古い owner による受付拒否は別の候補だが、通常の別物理本は items 外の scan 経路であり今回の根因とは断定しない。
- normal log に相手 __new5/266707 の load_folder はない。入力/action/受付の相関ログ不足で止まる箇所は未特定。実機は AI 高速汎用 4x 有効という fixture 差も残る。

## 今回の実装（未コミット、診断版）

- 実操作テストと production dispatcher の共通化。fixed1 は独立 Sol が P1/P2 なしで承認。
- opt-in perf category similar_move を追加。操作ごとの ID、source、typed normalized target の SHA256 先頭128bit token を用いて press/release/Response.clicked → action → existing/required route → admission/reject → cancel/error/presented を結ぶ。パス本文/ファイル内容を追加しない。
- persistent 診断 owner は押下中だけ。クリック後は action → 既存 typed navigation purpose に引き渡す。物理 scan 中は pending purpose が所有し、終了記録もその所有者から発行する。
- perf 無効時は追加 hash/clock 処理なし。毎フレームログは出さない。既存 key OR target の選択意味は維持。
- 本候補の常時 path 表示。snapshot の高さのみ 384x792 に広げ、path/[移動]/帯2本を視認できるようにした。アプリの画面サイズは変更していない。
- 親の文書変更: feedback、docs/spec.md、htdocs/mimageviewer/manual/fullscreen.html、本引継ぎ。

## 検証・残作業

- 先行 cargo check 成功との実装担当報告。
- cargo test -p mimageviewer --lib similar_move_ -- --nocapture: 3/3 PASS。
- cargo test -p mimageviewer --test ui_snapshot metadata_panel_similar_results_dark -- --nocapture: 1/1 PASS（47 filtered）。親も更新 PNG を実際に見て path/移動/帯の可読性と重なりなしを確認済み。
- 診断 chunk の最終独立レビュー、fmt/glyph 等の最終結果、全体 gate、portable build/update は未完了として扱い、実装担当の停止報告で補足する。
- 利用中 portable は今回まだ差し替えていない。前回 full gate/portable 記録は target/feedback-final-gate-20260909/final-manifest.json。今回のソースに対する PASS とは混同しない。
- 再開順: writer 停止 checkpoint を照合 → 診断 chunk の独立レビュー/指摘修正 → relevant checks/final full gate → build-portable.ps1 -KeepRunning → ユーザーの mIV 終了確認 → update-portable-dev.ps1 -SkipBuild（no Seed、data/data-remote 保持）→ --perf-log 起動手順を渡す。
- エージェントは利用者の portable/normal mIV を起動・停止しない。正常プロファイルを使わない。build/update では実データを内容読込/hash/変更しない。更新前に稼働プロセスと reparse を確認する。

## 保存境界

- target/similar-move-regression-20260909: 最初の実ボタン/外枠のログ。初回 compile typo と --exact 0 tests を成功と混同しない。
- target/similar-move-regression-fixed1-20260909: source-hashes.json/cached-hash.json 等。manifest はない。追加2件の PASS は tool output のみで個別 transcript は未保存と明記されている。
- target/similar-move-diagnostics-before-20260909: 診断変更の前 checkpoint。
- R2 staged raw baseline: 100861 bytes / SHA256 7e295682e3d4acd103242b2d1811ce7270bc3f71584dd6dc959db3c1d7110d7f。git diff --cached --binary --output=... で確認する（PowerShell Out-File は CRLF を変えてしまうため不可）。既存8 staged paths/その他 dirty/untracked を維持。

19:36 の親確認で cargo/rustc/link は動いていない。writer/reviewer の停止確認後、追補して利用者へ再起動可能と通知する。

## 停止確認追補

実装担当の保存・停止完了。最終 checkpoint は target/similar-move-diagnostics-pause-20260909（manifest/resume.txt/status/4 source before-after-diff/tests-ui_snapshot/PNG、18 artifacts）。R2 raw は上記 canonical hash と一致。cargo/rustc は 0。最終独立レビューはまだ依頼しておらず、再起動後にこの固定 checkpoint を一度だけ渡す。新規ビルド・portable 更新は行っていない。親から利用者へ再起動可能と通知する。


## 再開後の完了（2026-09-09 21:53 JST）

診断の独立レビュー指摘を修正、P1/P2残件なし。focused14件と全体gate本体7958件成功後、portable-devをnoSeedで更新済み。以後は再起動前checkpointからやり直さず、docs/duplicate-detection-feedback-20260909.md の最新節と target/similar-move-diagnostics-final-gate-20260909/manifest.json を正本とする。未完了は利用者の実機操作と移動不能の原因特定。source/他dirty/R2stagedは保持し、app起動停止は行っていない。

## 移動不能の修正まで完了（診断版より後の状態）

実機18操作のログからmain embedded fullscreenでRequiredFullscreenTarget完了結果を回収しない原因を特定し、post-renderで同ownerだけを既存poll/resolveへ接続。actual App::update 5回帰と既存button admission PASS、独立Sol P1/P2なし、全体gate7963 passed / 0 failed / 43 ignored。build-portable -KeepRunning（18分57秒）後、update-portable-dev -SkipBuild/noSeedで更新完了。runtime 24files一致、data/data-remote root metadata前後一致（内容hash/列挙は未実施）。exe SHA256 3b426741d5a36532a6e8ea5cbd78adc115edbd2e4eab07dab4a3c5e7fb6674c6。今回の正本はtarget/similar-move-embedded-pump-fix-20260909とfeedback文書最新節。以前の診断調査/レビューをやり直さない。未完了は利用者の上下[移動]による __new5 への着地、履歴戻り、パネル固定の実機確認。runtime未コミット、他dirty/R2 staged保持。

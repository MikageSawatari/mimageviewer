# Ctrl+上下のフォルダ移動とサイドカー復元待機の表示退行

2026-09-13。公開準備中の利用者報告から根因を特定し、§1.233 で既存の
fullscreen navigation owner と sidecar の deferred reopen owner を接続した。調査・実装・
自動検証中に実アプリの起動や入力、通常プロファイルの変更は行っていない。

## 観測

画像フルスクリーンのCtrl+上下で激しくちらつく。利用者はv3.9.0でも発生すると報告。
通常プロファイルのログを `target/fullscreen-folder-transition-20260913/observed.log` に保存した。
SHA-256: `EAE15DC2C5E8582E75DA167211B288EC430151FE066C2954EE9414E69729ABC5`。

同ログの一例（プロセス相対時刻）:

- 67.432s: fullscreen-eguiからctrl_nav_forwardを受付。
- 67.449s: presentation close、load_folder、旧worker取消。
- 67.451s: sidecar restore request=8200、context=0、generation=146開始。
- 67.452s: SLOW FRAMEにgrid=0.7msを記録。
- 67.465s: open_fullscreen idx=0。
- 67.468s: sidecarによる入力保持を解除。

周辺の移動でも復元ownerは約17〜35ms。この例は100ms後のmodal表示が原因という説明には合わず、待機中に一覧描画へ戻った記録がある。ログは描画内容の動画証拠ではないため、見え方そのものの確認は利用者報告に基づく。

## コードの因果

`9c9df532c`（v3.9.0に含まれる）で、フォルダのitems差替え後のサイドカー取り込みを非同期化した。
`defer_sidecar_restore_fullscreen` は再open要求をsidecar owner内の `deferred_fullscreen` に保持し、terminalで再開する。
しかし `ui_fullscreen.rs::fs_nav_deferred_reopen_wait_active` はPDF列挙待ちと書庫変換待ちのみを判定し、サイドカーによる再open待ちを含まない。v3.9.0と現行の両方に同じ欠落がある。
`poll_fs_nav_lock` はitems generationが進んだのにfullscreen_idxがNoneで、この待機判定がfalseなら、navigation lockと保持画像を解除する。
同じ待機判定はviewport維持などの表示継続経路にも使われる。

旧同期取り込みでは一フレームの中で通過していたclose〜reopenの区間が、非同期化後はフレームをまたぐ。その新しい待機ownerと既存の表示継続判定が整合していない。
旧XMP取り込み撤去で長い操作待ちは減ったが、この表示継続の欠落は残っている。

## 修正境界

- `SidecarRestoreState` が既に所有する deferred fullscreen intent を、projected context、
  items generation、folder/source、stable item identity がすべて一致するときだけ待機として投影する。
- その待機を既存の `FsNavigationSequence::FolderItems`、または従来の
  `FsHoldover::FolderNavigation` と generation lock が世代交代を所有している場合だけ認める。
  通常一覧の復元や、別 context の復元、navigation owner の無い open は表示保持を延長しない。
- sidecar terminal が同一 item を通常 fullscreen 入口へ戻した後は、既存の navigation sequence を
  `Display` target へ結び直す。新しい対象が描画され presentation trace が届くまでは前の表示 unit を
  保持し、cancel、discard、context retirement、identity mismatch では待機を解除する。
- 表示保持、viewport 維持、embedded の一覧抑止は従来どおり
  `fs_nav_deferred_reopen_wait_active` という一つの consumer を参照する。新しい待機フラグ、固定 delay、
  modal 抑止、補正前画像の先出しは加えていない。

この境界は MainWindow と linked viewport の共有 navigation sequence、および detached physical / slideshow
などが使う従来の `FolderNavigation` owner を覆う。前者は capture 済みの旧 unit を描画でき、後者は
capture が無い場合でも window / lifecycle の継続を所有する。PDF/ZIP の既存 deferred 判定、入力 gate、
sidecar の非同期処理と中央 DB 保護は変更していない。

## 回帰確認

- sidecar の exact context / generation / folder / item 照合と、discard 時の非待機
- `FolderItems` 世代交代中の旧表示 unit 保持、terminal と同じ open 後状態からの `Display` binding、
  target 描画後の trace による退役。sidecar terminal 関数自体は既存回帰の対象で、今回の test は
  navigation owner への引継ぎ部分を直接検査する
- sidecar fullscreen intent が無い通常一覧復元では navigation holdover を延長しないこと
- detached 系の旧 `FolderNavigation(None)` owner でも exact deferred reopen 中だけ window / lifecycle lock を
  維持し、generation mismatch では解放すること

## 検証記録

2026-09-13、製品差分を固定して次を確認した。

- focused regression 8件: 今回の exact sidecar / sequence / grid / legacy-owner 4件、既存 deferred
  navigation 2件、PDF enumerate 1件、archive-convert deferred fullscreen 1件がすべて成功
- `cargo check -p mimageviewer --bin mimageviewer-core`、`cargo fmt --all -- --check`、
  `python scripts/check_ui_glyphs.py`、`cargo run --locked -p viewer_context_audit --quiet`、
  `git diff --check` はすべて exit 0。viewer context audit は指摘 0件
- `RUST_TEST_THREADS=1` で `scripts/test-full.ps1 -SuppressCrashDialogs` を実行し exit 0。
  main lib 8386 success / 0 failure / 45 ignored、UI snapshot 50件、workspace / integration / doc、
  vendor egui 25件 / egui-wgpu 9件 / eframe 15件がすべて成功した
- `scripts/build-dev.ps1 -PreserveRuntime` は exit 0。core SHA-256 は
  `501D23D1E8620719AD82A57189E7E7FAF043745E127EE2DD268081E89BAF72B2`

完全ログは `target/section233-fullscreen-sidecar-transition-20260913/` に保存した。

- `test-full.stdout.log`: `A6B41738947F99C9E4C326DFFF8A8C7A73278AA64644A68292748BD64103DB6E`
- `test-full.stderr.log`: `36F5A176C26E49D9747876A4A30ACD5AF0AC8B32234219AF9C6117793B970921`
- `test-full.exit.txt`: `13BF7B3039C63BF5A50491FA3CFD8EB4E699D1BA1436315AEF9CBE5711530354`
- `build-dev.stdout.log`: `7C741DECEFAFEB8E583858E9A6CA13EEDE311C270F399F8BD5AC51B6280DCC85`
- `build-dev.stderr.log`: `B9036C611619ACCB1C0CD4C5142BBC215CC99FFAFB3C6CC4BD1F31A0971B084D`
- `build-dev.exit.txt`: `13BF7B3039C63BF5A50491FA3CFD8EB4E699D1BA1436315AEF9CBE5711530354`

検証中にagentはアプリを起動・停止していない。通常 `%APPDATA%` を使う確認binaryの実機で、
Ctrl+上下での見え方について、下記の利用者確認を受けた。

最大化時の一回の画像引伸ばし（§1.227）とは別の問題として扱う。

## 独立照合

Sol/xhighの独立担当も、導入前後のコードとログを照合し、非同期sidecar再open待機をnavigation保持側が認識しない互換欠落を確認した。修正時はcontext・generation・item identityを照合し、単なるApp-globalなsidecar_active判定を採用しないことに設計担当と合意した。

## 最終検収と公開引き継ぎ

独立Sol/xhigh担当が製品差分・最終manifest・全体gate・buildを照合し、重大指摘なしで承認した。2026-09-13、利用者から修正版について「なおりました。よさそうです」と確認を受け、ClaudeCodeでの公開再開とコミットを依頼された。利用者によるバイナリのハッシュ照合や全窓・全入力の試験まで実施済みとは扱わない。

製品2ファイルは検証manifestのハッシュと一致し、確認後の変更は受入記録だけである。既存の成功済み検証を再利用し、§1.233の公開保留を解除する。公開担当は最終配布物にこの修正を含め、既定のリリースゲートを続行する。恒久バックログは完了項目削除の運用に従い§1.233を削除し、本書に経緯と検証証拠を保持する。

# 実装依頼: test-lib-full.ps1 の台帳 HIT を速くする (ファイルのハッシュをサイズと更新日時でキャッシュ)

- 担当: Codex GPT-6.1 Sol / high。完了後に別の Sol / xhigh が独立レビューする
- 作業場所: worktree `G:\home\mimageviewer-statcache`、ブランチ `ledger-stat-cache`
- **コミットはしない** (この sandbox から linked worktree の git 管理領域へ書けない)。変更は作業ツリーに残し、
  最終報告に論理単位とコミットメッセージ案を書く。git の書き込みで止まらない。
- 製品コードは変えない。対象は `scripts/test-tools-common.ps1`、`scripts/test-lib-full.ps1`、
  `scripts/test-test-tools.ps1`、`docs/development-build-and-test.md`、`docs/test-speed-report.md`。

## 背景 (本体 worktree `C:\home\mimageviewer` で 2026-10-16 に測定)

- 同じソース状態での 2 回目の `test-lib-full.ps1` (台帳 HIT) が **63 秒**。testspeed worktree では 3 秒だった。
- 内訳: `Get-MivSourceId` 49.9 秒、`Get-MivEnvironmentId` 0.2 秒、`Get-MivNativeRuntimeId` 0.3 秒。
- `Get-MivSourceId` は 21,247 ファイル・**59.1 GB** を毎回 SHA-256 している。大半は ignored の `testimage/`
  (`testimage/zip_seek_10000` 42.4 GB、`iphone` 4.3 GB など、手動確認用の画像置き場)。
- `testimage/` は一部の自動テストが特定のファイルを読む (例: `src/fs_animation.rs` の
  `testimage/rotating_earth.gif`。無ければ skip)。そのため**照合対象から外さない**。

## 方針 (設計担当が決めた)

ファイルごとのハッシュを、**サイズと更新日時が前回と同じなら再計算しない**キャッシュにする。git の index と同じ考え方。

- キャッシュのキー: 相対パス。値: サイズ、`LastWriteTimeUtc` の ticks、`CreationTimeUtc` の ticks、SHA-256、記録した時刻。
  サイズ・更新日時・作成日時のどれかが違えば再計算する。
- **racy な更新日時への対策**: 記録した時刻と更新日時の差が小さい (例: 2 秒以内) エントリは信用せず再計算する
  (git の racy-git 対策と同じ。記録直後に同じ秒内で書き換わった場合の取りこぼしを防ぐ)。
- 置き場所: `target/test-lib-full/` の下 (台帳と同じ場所)。PowerShell 5.1 で 2 万件を数秒以内に読み書きできる形式にする
  (`ConvertFrom-Json` は遅いので、タブ区切りの行と Dictionary などを検討)。
- 入れ子リポジトリのファイル (`Get-MivNestedFiles`) にも同じキャッシュを使う。DLL 照合 (`Get-MivNativeRuntimeId`) は
  すでに速いので変えなくてよい。
- キャッシュは**速くするためだけ**のもので、識別子の値は変えない (キャッシュあり／なしで同じ `source=` になること)。
- キャッシュの読み書きに失敗したら、警告を出してキャッシュなしで全ハッシュする (既定の割り切り方針。回復の仕組みは作らない)。
- 検知できなくなる範囲を文書に書く: サイズと更新日時と作成日時を保ったまま内容だけ変えた場合。git も同じ前提で、
  疑わしいときは `-Force` で再実行する。

## 要件

1. 本体 worktree 相当の条件で、台帳 HIT を **5 秒以内**にする目標。worktree には `testimage/` が無いので、
   数万ファイル規模の fixture を `target/` 下に作って、キャッシュなし／ありの所要時間を測る。本体での最終測定は私が行う。
2. `scripts/test-test-tools.ps1` に回帰確認を足す:
   - キャッシュあり／なしで同じ識別子になる
   - 内容とサイズを変えると識別子が変わる
   - 内容だけ変えて更新日時を記録より前の時刻へ戻した場合も、サイズが違えば検知する
   - 作成日時だけが変わっても再計算する
   - racy なエントリ (記録直後の更新) は再計算する
   - キャッシュファイルが壊れていても、警告のうえ正しい識別子になる
3. `.ps1` は ASCII のみ、PowerShell 5.1 で動くこと。
4. 文書: `docs/development-build-and-test.md` のキャッシュの説明 (検知の範囲と `-Force`)、`docs/test-speed-report.md` に測定結果。

## 報告 (日本語)

キャッシュなし／ありの所要時間、回帰確認の結果、検知できない範囲、論理単位とコミットメッセージ案、未解決の点。
止めてよいのは、製品コードの変更・新しいツールのインストールが要るとき、前提の矛盾を見つけたときだけ。

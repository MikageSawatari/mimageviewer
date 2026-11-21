# サブ PC の ReleaseChecks 依頼の雛形

リリース前の性能チェック ReleaseChecks をサブ PC で流すときの依頼文の雛形 (2026-11-21〜)。
統括はリリースごとに `<...>` を埋めて `C:\home\mimageviewer-subpc\relcheck\<kit-id>-brief.md` に置き、
`python scripts/write_utf8_bom.py <そのファイル>` で BOM を付けてから、サブ PC 担当 (Remote Control の
「mImageViewer 実機確認」セッション) へ SendMessage で場所を伝える。依頼を送る前に、キット・事前確認と本番・見込み時間を利用者へ示して
了承をもらい、その範囲を依頼文に書く ([実行確認](interactive-release-verification.md) の「サブ PC のキットで流す ReleaseChecks」)。

手順の背景とキットの仕組みは [自動化計画 §11](release-check-automation-plan.md#relcheck-subpc-kit)。
サブ PC 担当の一般の決まりは [subpc-test-brief.md](subpc-test-brief.md)。

雛形の前提 (変えるときは、この文書と依頼文の両方を直す):

- キットはメイン PC の `C:\home\mimageviewer-subpc\relcheck\<kit-id>\` にある。サブ PC からは `\\tsclient\C\...` で見える。
- **サブ PC でキットを置く場所は `C:\miv-relcheck\root` に決まっている** (キット作成時に表示される `Run root on the sub-PC`)。
  試験用ビルドは、証拠フォルダが「ビルドした repository root の下」にあることを確かめるので、メイン PC でビルドした worktree と
  同じ path に置かないと動かない。違う path では、事前確認と本番が mIV を起動する前に止まる。
- 結果はキットの外の `C:\home\mimageviewer-subpc\relcheck\<kit-id>-results\` へ戻す (キットの中へ足すと、
  メイン PC の照合が「余分なファイル」で止まる)。
- 前面の条件: メイン PC では、ツールの裏のプロセスから流すと前面を取れずに止まり、ターミナル パネルから流すと
  成り立った (2026-11-17)。サブ PC でも、ターミナル パネルから流した事前確認が READY になった (2026-11-21、観測者はサブ PC 担当)。
  事前確認と本番の両方をターミナル パネルから流す。
- ターミナル パネルのツールでは `$LASTEXITCODE` を打てないので、終了コードを `EXIT=<値>` と表示する wrapper
  (`scripts\relcheck-kit\Run-SubPcProbe.cmd` / `Run-SubPcSuite.cmd`、キットに入っている) を使う。
- 画面操作 (computer-use) はしない。キー入力・マウス入力も送らない。mIV の起動と終了はスクリプトが行う。
  そのため、仮想デスクトップを分ける決まり (画面操作のときに Claude アプリのパネルと重ならないため) は、
  この依頼では使わない。Claude アプリと同じデスクトップのまま流す。

---

以下が依頼文。

~~~~markdown
# サブ PC 依頼: リリース前の性能チェック ReleaseChecks (キット <kit-id>)

メイン PC の統括からの依頼です。最初の指示文の決まりを守ってください。特に次の 4 つです。

- mIV は、このキットの試験用ビルドを、キットの中の使い捨ての data でだけ起動する (起動と終了はスクリプトが行う)
- インストール版と `%APPDATA%\mimageviewer` に触れない
- 画面全体へキー入力 (特に Esc) を送らない。今回は画面操作もキー入力・マウス入力も使わない
- ダウンロードしない

今回は画面操作をしません。スクリプトが mIV を起動し、計測し、終了します。
**事前確認と本番は、どちらも Claude アプリのターミナル パネルから流してください** (ツールの裏のプロセスからだと、
mIV の窓を前面にできずに止まります)。画面操作をしないので、今回は仮想デスクトップを分けず、
Claude アプリと同じデスクトップのまま流します。mIV の窓が Claude アプリの上に出ますが、クリックしないでください。

キット: `<kit-id>` (メイン PC のソース `<artifact のコミット>`、scripts `<scripts のコミット>`)。
利用者の了承 (<日時>): このキットで、事前確認 1 回と本番 1 回を流す。再試行はしない。
キットの manifest の SHA-256: `<release-check-kit.sha256 の値>`。
**キットを置く場所は `C:\miv-relcheck\root` です** (ほかの path では動きません)。

## 0. 最初に返してほしいこと
「受け取りました」とだけ返して、そのまま 1 へ進んでください。

## 1. 始める前の確認
- ほかの依頼で起動した mIV (mimageviewer*.exe) が残っていれば、自分で起動したものだけ終了する。
- 夜間試験の時間枠 (01:00〜07:00) の中なら、始めずに返事をください。
- `python --version` が 3.10 以上であること。無い・古いときは、ここで止めて結果を返してください (インストールはしない)。

## 2. コピー
`C:\miv-relcheck\root` が既にあれば、前のキットです。名前を変えて退避してから、コピーします。
```powershell
if (Test-Path C:\miv-relcheck\root) { Rename-Item C:\miv-relcheck\root ('root-old-' + (Get-Date -Format yyyyMMdd-HHmmss)) }
robocopy \\tsclient\C\home\mimageviewer-subpc\relcheck\<kit-id> C:\miv-relcheck\root /E
Get-Content C:\miv-relcheck\root\release-check-kit.sha256
```
表示された値が上の SHA-256 と同じことを確かめる。違えば止めて返事をください。
以後はコピー先 `C:\miv-relcheck\root` だけを使います (`\\tsclient` の上では流さない)。

## 3. 事前確認 (mIV は起動しない。約 1 分)
ターミナル パネルで:
```powershell
C:\miv-relcheck\root\scripts\relcheck-kit\Run-SubPcProbe.cmd
```
使い捨ての小さな窓が 2 つ出て、前面が切り替わり、30 秒以内に閉じます。`READY:` か `NOT_READY:` で始まる行が結果で、
最後の `EXIT=` は READY = 0、NOT_READY = 2 です。
`C:\miv-relcheck\root\target\ui-smoke\env-probe.json` の中身を結果に付けてください。
**`NOT_READY` なら本番へ進まず**、そのまま結果を返してください。回避の工夫 (別の起動方法・設定の変更) はしないでください。

## 4. 本番 (約 10 分。途中で PC を操作しない)
同じターミナル パネルで:
```powershell
C:\miv-relcheck\root\scripts\relcheck-kit\Run-SubPcSuite.cmd
```
- 終わるまで、ほかのコマンドを流さず、窓をクリックしない。mIV の窓は何度か開いて閉じます。
- 最後に `ReleaseChecks evidence: <run フォルダ>` と `EXIT=<終了コード>` が出ます。
  0 = PASS / PASS_WITH_WARNINGS、1 = FAIL、2 = SKIPPED (環境不成立)。
- 失敗しても**流し直さないでください**。1 回の結果をそのまま返します。

## 5. 結果をメイン PC へ戻す
```powershell
robocopy <run フォルダ> \\tsclient\C\home\mimageviewer-subpc\relcheck\<kit-id>-results\<run フォルダの名前> /E /XD fixture
Copy-Item C:\miv-relcheck\root\target\ui-smoke\env-probe.json \\tsclient\C\home\mimageviewer-subpc\relcheck\<kit-id>-results\
```
`fixture` (生成した試験データ) は戻さなくてよいです。ターミナルの出力も `terminal-output.txt` として同じ場所へ置いてください。

## 6. 返事に書くこと
- 事前確認の `READY` / `NOT_READY` と、env-probe.json の要点 (RDP セッションか、画面の大きさ、GPU)
- 本番の終了コードと、`release-record.md` の中身 (そのまま貼る)
- 戻した場所
- 自分で見たこと (途中で出たメッセージ・生成されたファイル) と、確かめていないことを分けて

## 7. 後始末
- スクリプトが起動した mIV は自動で終了します。`C:\miv-relcheck\root\target\portable-smoke\` の mimageviewer*.exe が
  残っていたら、それだけ終了してください。
- `C:\miv-relcheck\root` は消さずに残してください (次のキットを置くときに退避します)。
~~~~

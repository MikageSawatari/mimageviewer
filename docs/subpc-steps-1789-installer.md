# サブ PC での確認手順: AI アプリが MCP を使ったままのインストーラ更新 (§1.789)

[§1.789](next-release-backlog.md) の確認手順。第 1 部は A-1 (インストーラ)、第 2 部は A-2 (古い adapter を新しい core が受け付ける) を確かめる。
依頼は統括が出す。依頼では、下の「渡す物」の実際の場所と、どの部を行うかを指定する。

## 製品のインストーラを使うこと

この確認はインストーラそのものを確かめるので、**インストーラを実行して `C:\Program Files\mImageViewer` に入れ、そこから起動する**。
サブ PC で製品のインストーラを使い、インストール済みの mIV を確認のために更新・削除することは、利用者の常設の許可がある
(2026-11-15、CLAUDE.md の「実機検証と設定データの保護」)。

それ以外の決まりは変えない。特に、mIV の実データを使わないために次を守る:

- mIV の GUI も MCP も、**環境変数 `APPDATA` を試験用のフォルダ `C:\miv-test-v5\1789\appdata` に向け** (launcher は `%APPDATA%\mimageviewer\runtime\<版>` へ展開する)、
  **`--data-dir` に、その `APPDATA` の下の既定の場所 `C:\miv-test-v5\1789\appdata\mimageviewer` を付けて**起動する。
  `--data-dir` を既定と違う場所にすると、mIV の終了の合図と mutex の名前に接尾辞が付き、インストーラ (固定の名前を使う) が試験用の mIV を
  終了できなくなる。この確認では必ずこの 1 つの場所を使う。
- **アンインストールの最後に「設定ファイルとキャッシュを削除しますか？」と聞かれたら、必ず「いいえ」を選ぶ**。ここに表示される場所は
  試験用の `APPDATA` ではなく、サブ PC の実際の `%APPDATA%\mimageviewer` (Windows のシェルのフォルダから決まる)。表示された場所を記録する。
- 始める前に、サブ PC で通常版・トレイ常駐の mImageViewer と、ほかの AI アプリの MCP (mImageViewer を使うもの) が動いていないことを確かめる。
  インストーラは固定の名前で mIV に終了を頼み、AI 連携の印も固定の名前で見るので、無関係なものも巻き込む。
- インストーラの最後の「mImageViewer を起動する」のチェックは外す (付けたままだと `APPDATA` を向けずに起動する)。
- mIV と Claude アプリは別の仮想デスクトップに置く (いつもの決まり)。

## 渡す物 (依頼で場所を指定する)

- 旧版のインストーラ: v5.2.1 の `mImageViewer_setup.exe`。
- 新しいインストーラ: A-1・A-2・文字起こし T1 を入れた版 (IPC の版 4)。第 1 部と第 2 部の両方でこれを使う (release のビルドを 1 回で済ませるため)。
  第 1 部の手順 4 の「同じセッションで tool が成功する」は、A-2 で古い adapter (版 3) を新しい core (版 4) が受け付けるから成り立つ。

## 準備

1. 試験用のフォルダ `C:\miv-test-v5\1789\appdata\mimageviewer` を作る。
2. MCP の登録 `miv-test` を、インストール先の launcher に向ける。`C:\miv-test-v5\mcp-current\run-mcp.cmd` が exe.txt と data-dir.txt を読む作りなら、
   この確認の間だけ、次と同じ起動になるようにする (元の中身は控えておき、最後に戻す):
   ```
   set APPDATA=C:\miv-test-v5\1789\appdata
   "C:\Program Files\mImageViewer\mimageviewer.exe" --mcp --data-dir C:\miv-test-v5\1789\appdata\mimageviewer
   ```
3. mIV の GUI の起動は、PowerShell で次のようにする (下では「試験用の mIV を起動」と書く):
   ```powershell
   $env:APPDATA = 'C:\miv-test-v5\1789\appdata'
   Start-Process -FilePath 'C:\Program Files\mImageViewer\mimageviewer.exe' -ArgumentList '--data-dir','C:\miv-test-v5\1789\appdata\mimageviewer'
   ```
4. MCP の許可: 試験用の mIV の環境設定で、AI 連携を有効にする (読み取りだけでよい)。

記録には、次の PowerShell の出力を使う (下では「状態を記録」と書く):
```powershell
Get-ChildItem 'C:\Program Files\mImageViewer' -Filter 'mimageviewer.exe*' | ForEach-Object { [pscustomobject]@{ Name = $_.Name; Length = $_.Length; SHA256 = (Get-FileHash $_.FullName -Algorithm SHA256).Hash } }
Get-Process mimageviewer, mimageviewer-mcp, mimageviewer-core -ErrorAction SilentlyContinue | Select-Object Id, ProcessName, StartTime, Path, @{ n = 'Version'; e = { $_.MainModule.FileVersionInfo.ProductVersion } }
```
launcher (`mimageviewer.exe`) の `Path` は観測として書くだけで、合否には使わない (改名の後も、起動した時の名前が出ることがある)。launcher の合否は、
process の Id が同じまま動いていること、正規の `mimageviewer.exe` のハッシュ、`.old` のハッシュで決める。
MCP の adapter (`mimageviewer-mcp.exe`) には版の情報が埋め込まれていないので `Version` は空になる。adapter は改名されないので、版は `Path` の
`runtime\<版>\mimageviewer-mcp.exe` の `<版>` と、Id・開始時刻で見分ける。

## 第 1 部: A-1 (インストーラ)

1. 旧版のインストーラを実行し、既定の場所に入れる (最後の起動のチェックは外す)。状態を記録。
2. 試験用の mIV を起動する。Claude Code の新しいセッションで `miv-test` の `miv_status` を呼び、成功することを確かめる。
   続けて `list_tags` か `list_library` を 1 回呼ぶ。状態を記録し、`mimageviewer.exe` の process の Id を書く
   (GUI の launcher は本体を起動した後に終わるので、残っている `mimageviewer.exe` が MCP の launcher のはず。違ったらそのまま書く)。
3. **取消**: 新しいインストーラを実行し、「インストール」を押した後、進み具合の画面で「キャンセル」を押す。
   (以前はここで「AI アプリが mImageViewer を使っています」と出て止まった。出たらそれを記録して第 1 部を終える。)
   取消が間に合わずに終わってしまった場合は、そのことを書いて 4 へ進む。
   - 確かめる: 状態を記録し、正規の `mimageviewer.exe` のハッシュが 1 と同じ (元に戻っている)、`mimageviewer.exe.*.old` が無い、
     MCP の launcher の process (2 の Id) が同じ Id で動いている。
   - インストーラは取消の前に試験用の mIV を終了させ、取消では起動し直さない。試験用の mIV を起動し直してから、同じ Claude Code のセッションで
     `miv_status` が成功することを確かめる。
4. **更新**: 新しいインストーラをもう一度実行し、最後まで進める。
   - 確かめる: 「AI アプリが…」で止まらない。状態を記録し、正規の `mimageviewer.exe` のハッシュが新しい版のもの、`mimageviewer.exe.<…>.old` が 1 つあって
     そのハッシュが 1 と同じ、MCP の launcher の process (2 の Id) が同じ Id で動き続けている。更新の前に動いていた `mimageviewer-core` の process が
     終わっていることも書く。
   - インストーラが mIV の GUI を終了させるので、試験用の mIV を起動し直す。状態を記録し、`mimageviewer-core` が新しい Id・新しい版で動いていることを確かめる。
     同じ Claude Code のセッションで `miv_status` と、2 と同じ tool を呼び、成功することを確かめる (AI アプリを再起動しないこと)。
5. **アンインストールは止まる**: Windows の「設定」→「アプリ」から mImageViewer のアンインストールを始める。
   - 確かめる: 「AI アプリ (Claude・ChatGPT など) が mImageViewer を使っています」と出て止まる。止まったら閉じる。
6. **残ったファイルの掃除**: Claude Code のセッションを閉じ、MCP の launcher の process が無くなったことを確かめる。新しいインストーラをもう一度実行して最後まで進める。
   - 確かめる: 状態を記録し、`mimageviewer.exe.*.old` が無くなっている。
7. 片付けを兼ねたアンインストール: もう一度 2 のように MCP を使い、状態を記録して正規の `mimageviewer.exe` のハッシュを控える。新しいインストーラで
   もう一度更新して `.old` を作り (今回の `.old` のハッシュは、ここで控えた値と同じになる。手順 1 の旧版とは比べない)、セッションを閉じ、アンインストールする
   (最後の「設定ファイルとキャッシュを削除しますか？」は「いいえ」。表示された場所を記録する)。
   - 確かめる: `C:\Program Files\mImageViewer` に `mimageviewer.exe` と `.old` が残らない。

## 第 2 部: A-2 (古い adapter をそのまま使い続ける)

第 1 部の後に、旧版を入れ直してから始める。

1. 旧版 (v5.2.1) を入れる。試験用の mIV を起動し、Claude Code の新しいセッションで `miv_status`・`list_tags`・`list_library` を呼んで成功を確かめる。
   `list_library` の結果に続きの cursor があれば、その cursor を控える。セッションの tool の一覧に `get_transcript` が無いことを書く。
2. このセッションを開いたまま、新しいインストーラで更新し、試験用の mIV を起動し直す。状態を記録し、`mimageviewer-core` が新しい Id・新しい版で動いていることを確かめる
   (古い版の core が残っていたら、そのまま書いて止める。古い core に繋がったままでは A-2 を確かめたことにならない)。
3. 同じセッションで (AI アプリを再起動せず) `miv_status`・`list_tags`・`list_library` を呼ぶ。
   - 確かめる: どれも成功する。「mIV が起動していません」や「版が違います」が出ない。
   - 1 で控えた cursor で続きを取ってみる。成功するか、「最初からやり直してください」になるかを記録する (どちらでもよい。後者のあと、cursor 無しでは成功すること)。
   - tool の一覧に `get_transcript` はまだ無い (想定どおり) ことを書く。
4. Claude Code の新しいセッションを開き、tool の一覧と、状態の記録 (adapter の `mimageviewer-mcp` の Id・開始時刻・Path) を書く。
   - 確かめる: Path が新しい版の `runtime\<版>\mimageviewer-mcp.exe` の adapter が新しく動いていて、`get_transcript` が一覧にある。
     古いセッションの adapter (1 の時から動いている Id) は、古い版の Path のまま残っていてよい。
5. 古いセッションと新しいセッションを閉じ、アンインストールして片付ける (削除の質問は「いいえ」)。

## 片付け

- `run-mcp.cmd` を元の中身に戻す。
- `C:\miv-test-v5\1789\` は残してよい (次の確認で消す)。
- サブ PC に元から mImageViewer が入っていた場合は、元の版を入れ直すかどうかを利用者に確かめる。

## 報告

- 各手順の「確かめる」の結果と、状態の記録の出力。
- 自分で見たこと・ログから読んだこと・確かめられなかったことを分けて書く。
- 取消が間に合わなかった、別の文言が出たなど、手順どおりにならなかった所はそのまま書く。

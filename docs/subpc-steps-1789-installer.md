# サブ PC での確認手順: AI アプリが MCP を使ったままのインストーラ更新 (§1.789)

[§1.789](next-release-backlog.md) の確認手順。第 1 部は A-1 (インストーラ)、第 2 部は A-2 (古い adapter を新しい core が受け付ける) を確かめる。
依頼は統括が出す。依頼では、下の「渡す物」の実際の場所と、どの部を行うかを指定する。

## この確認だけの例外 (利用者の許可が要る)

サブ PC の決まり ([subpc-test-brief.md](subpc-test-brief.md)) では、インストール済みの mImageViewer を起動・変更しない。
この確認はインストーラそのものを確かめるので、**インストーラを実行して `C:\Program Files\mImageViewer` に入れ、そこから起動する**。
依頼を出す前に、統括が利用者からこの例外の許可を取る。サブ PC にすでに mImageViewer が入っている場合は、それが上書きされることも伝える。

それ以外の決まりは変えない。特に、mIV の実データを使わないために次を守る:

- mIV の GUI も MCP も、**環境変数 `APPDATA` を試験用のフォルダに向け** (launcher は `%APPDATA%\mimageviewer\runtime\<版>` へ展開する)、
  **`--data-dir` に試験用のフォルダを付けて**起動する。以下では `C:\miv-test-v5\1789\appdata` と `C:\miv-test-v5\1789\data` を使う。
- インストーラの最後の「mImageViewer を起動する」のチェックは外す (付けたままだと `APPDATA` を向けずに起動する)。
- mIV と Claude アプリは別の仮想デスクトップに置く (いつもの決まり)。

## 渡す物 (依頼で場所を指定する)

- 旧版のインストーラ: v5.2.1 の `mImageViewer_setup.exe`。
- 第 1 部の新しいインストーラ: A-1 を入れた版。IPC の版は v5.2.1 と同じ 3。
- 第 2 部の新しいインストーラ: A-1・A-2・文字起こし T1 を入れた版 (IPC の版 4)。

## 準備

1. 試験用のフォルダを作る: `C:\miv-test-v5\1789\appdata`、`C:\miv-test-v5\1789\data`。
2. MCP の登録 `miv-test` を、インストール先の launcher に向ける。`C:\miv-test-v5\mcp-current\run-mcp.cmd` が exe.txt と data-dir.txt を読む作りなら、
   この確認の間だけ、次と同じ起動になるようにする (元の中身は控えておき、最後に戻す):
   ```
   set APPDATA=C:\miv-test-v5\1789\appdata
   "C:\Program Files\mImageViewer\mimageviewer.exe" --mcp --data-dir C:\miv-test-v5\1789\data
   ```
3. mIV の GUI の起動は、PowerShell で次のようにする (下では「試験用の mIV を起動」と書く):
   ```powershell
   $env:APPDATA = 'C:\miv-test-v5\1789\appdata'
   Start-Process -FilePath 'C:\Program Files\mImageViewer\mimageviewer.exe' -ArgumentList '--data-dir','C:\miv-test-v5\1789\data'
   ```
4. MCP の許可: 試験用の mIV の環境設定で、AI 連携を有効にする (読み取りだけでよい)。

記録には、次の PowerShell の出力を使う (下では「状態を記録」と書く):
```powershell
Get-ChildItem 'C:\Program Files\mImageViewer' -Filter 'mimageviewer.exe*' | Select-Object Name, Length, LastWriteTime
Get-FileHash 'C:\Program Files\mImageViewer\mimageviewer.exe' -Algorithm SHA256 | Select-Object Hash
Get-Process mimageviewer, mimageviewer-mcp, mimageviewer-core -ErrorAction SilentlyContinue | Select-Object Id, ProcessName, Path
```

## 第 1 部: A-1 (インストーラ)

1. 旧版のインストーラを実行し、既定の場所に入れる (最後の起動のチェックは外す)。状態を記録。
2. 試験用の mIV を起動する。Claude Code の新しいセッションで `miv-test` の `miv_status` を呼び、成功することを確かめる。
   続けて `list_tags` か `list_library` を 1 回呼ぶ。状態を記録し、`mimageviewer.exe` の process の Id と Path を書く
   (GUI の launcher は本体を起動した後に終わるので、残っている `mimageviewer.exe` が MCP の launcher のはず。違ったらそのまま書く)。
3. **取消**: 新しいインストーラを実行し、「インストール」を押した後、進み具合の画面で「キャンセル」を押す。
   (以前はここで「AI アプリが mImageViewer を使っています」と出て止まった。出たらそれを記録して第 1 部を終える。)
   取消が間に合わずに終わってしまった場合は、そのことを書いて 4 へ進む。
   - 確かめる: 状態を記録し、`mimageviewer.exe` のハッシュが 1 と同じ (元に戻っている)、`mimageviewer.exe.*.old` が無い、
     MCP の launcher の process (2 の Id) が動いている。同じセッションで `miv_status` が成功する。
4. **更新**: 新しいインストーラをもう一度実行し、最後まで進める。
   - 確かめる: 「AI アプリが…」で止まらない。状態を記録し、`mimageviewer.exe` のハッシュが新しい版のもの、`mimageviewer.exe.<…>.old` が 1 つある、
     MCP の launcher の process (2 の Id) が動き続けていて、Path が `.old` の名前に変わっている。
   - インストーラが mIV の GUI を終了させるので、試験用の mIV を起動し直す。同じ Claude Code のセッションで `miv_status` と、2 と同じ tool を呼び、
     成功することを確かめる (AI アプリを再起動しないこと)。
5. **アンインストールは止まる**: Windows の「設定」→「アプリ」から mImageViewer のアンインストールを始める。
   - 確かめる: 「AI アプリ (Claude・ChatGPT など) が mImageViewer を使っています」と出て止まる。止まったら閉じる。
6. **残ったファイルの掃除**: Claude Code のセッションを閉じ、MCP の launcher の process が無くなったことを確かめる。新しいインストーラをもう一度実行して最後まで進める。
   - 確かめる: 状態を記録し、`mimageviewer.exe.*.old` が無くなっている。
7. 片付けを兼ねたアンインストール: もう一度 2 のように MCP を使い、4 のように更新して `.old` を作ってから、セッションを閉じ、アンインストールする。
   - 確かめる: `C:\Program Files\mImageViewer` に `mimageviewer.exe` と `.old` が残らない。

## 第 2 部: A-2 (古い adapter をそのまま使い続ける)

第 2 部の新しいインストーラ (IPC の版 4) で行う。第 1 部の後なら、旧版を入れ直してから始める。

1. 旧版 (v5.2.1) を入れる。試験用の mIV を起動し、Claude Code の新しいセッションで `miv_status`・`list_tags`・`list_library` を呼んで成功を確かめる。
   `list_library` の結果に続きの cursor があれば、その cursor を控える。セッションの tool の一覧に `get_transcript` が無いことを書く。
2. このセッションを開いたまま、新しいインストーラで更新し、試験用の mIV を起動し直す。
3. 同じセッションで (AI アプリを再起動せず) `miv_status`・`list_tags`・`list_library` を呼ぶ。
   - 確かめる: どれも成功する。「mIV が起動していません」や「版が違います」が出ない。
   - 1 で控えた cursor で続きを取ってみる。成功するか、「最初からやり直してください」になるかを記録する (どちらでもよい。後者のあと、cursor 無しでは成功すること)。
   - tool の一覧に `get_transcript` はまだ無い (想定どおり) ことを書く。
4. Claude Code の新しいセッションを開き、tool の一覧と、そのセッションの MCP の adapter の process の Path (`%APPDATA%` を向けた先の `runtime\<版>\mimageviewer-mcp.exe` のどの版か) を記録する。
   - 確かめる: 新しい版の adapter が動いていて、`get_transcript` が一覧にある。
5. 古いセッションと新しいセッションを閉じ、アンインストールして片付ける。

## 片付け

- `run-mcp.cmd` を元の中身に戻す。
- `C:\miv-test-v5\1789\` は残してよい (次の確認で消す)。
- サブ PC に元から mImageViewer が入っていた場合は、元の版を入れ直すかどうかを利用者に確かめる。

## 報告

- 各手順の「確かめる」の結果と、状態の記録の出力。
- 自分で見たこと・ログから読んだこと・確かめられなかったことを分けて書く。
- 取消が間に合わなかった、別の文言が出たなど、手順どおりにならなかった所はそのまま書く。

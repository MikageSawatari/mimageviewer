# Susie プラグインのクラッシュ復帰を確かめる

対象: `crates/susie-crash-plugin/` (開発専用、**配布物には入らない**)。

## 1. なぜ作ったか

Susie プラグインは 32bit の隔離ワーカー (`mimageviewer-susie32.exe`) で動かしているので、
プラグインが落ちても本体は生き残る。**そこまでは設計どおり**である。

確かめられていなかったのはその先で、「ワーカーが死んだあと、後続の要求はどうなるか」が
一度も検証されていなかった。手元に落ちるプラグインが無く、再現手段が無かったためである。
コードを読むと、実際には次の 2 点が成り立っていない
([next-release-backlog.md](next-release-backlog.md) §1.120):

- 起動後に worker が想定外終了しても **自動 respawn しない**
- `run_dispatcher` は `send_recv` の失敗後も loop を抜けないため、**死んだ pipe の
  dispatcher が共有キューから後続 job を取り続ける**。しかもエラーは即座に返るので、
  生きている worker より速く job を吸い込む

README とマニュアルには「プラグインクラッシュ時は自動再起動」と書いてあり、**実装と
食い違っている**。

## 2. 何ができるか

`.miv-crashtest` 拡張子のファイルを読ませると、**中身の先頭行**で挙動が決まる。

| 先頭行 | 挙動 |
| --- | --- |
| `MIVOK` | 8x8 の緑の画像を正常に返す |
| `MIVCRASH` | `GetPicture` の中でアクセス違反 |
| `MIVHALF` | 呼ばれるたび 50% でアクセス違反 |
| `MIVSUPPORTCRASH` | `IsSupported` の中でアクセス違反 (プラグイン選択の段階で死ぬ) |

**ファイル名ではなく中身で決める**のは、`GetPicture` にファイル名が渡らないため
(ワーカーは `flag=1` = メモリ渡しで呼ぶ)。名前は読みやすさのために内容と揃えてあるだけで、
自由に変えてよい。

`MIVOK` を用意しているのが要点で、**クラッシュの後にこれが読めるかどうか**が確かめたい
ことそのものである。落ちること自体は確認するまでもない。

## 3. 使い方

```powershell
# ビルドして %APPDATA%\mimageviewer\susie_plugins\ へ配置し、サンプルも書き出す
.\scripts\setup-susie-crash-plugin.ps1

# 隔離した data-dir へ入れる場合
.\scripts\setup-susie-crash-plugin.ps1 -DataDir .\target\dev-runtime\data

# 撤去 (プラグインとサンプルの両方)
.\scripts\setup-susie-crash-plugin.ps1 -Remove
```

`.miv-crashtest` ファイルを閲覧するフォルダへコピーし、**mImageViewer を再起動する**
(ワーカーは起動時に 1 度だけプラグインを読む)。

## 4. 見るべきこと

1. `ok.miv-crashtest` が緑の四角として出る (プラグインが正しく認識されている)
2. `crash-always.miv-crashtest` でワーカーが死ぬ
3. **その後 `ok-second` / `ok-third` が読めるか** ← ここが本題
4. `mimageviewer.log` に何が残るか

修正前は 3 が失敗するはずである (死んだ dispatcher が後続を吸い、全部エラーになる)。
**先に修正前の状態でこれを観測してから直す。**

## 4.1 打ち切り (通知と診断表示) を出す

通常の使い方では**打ち切りまで行かない**。落とした対象は記憶されて二度と投げられず、
作り直した枠が 1 件でも応答を返せば再起動回数は数え直されるので、枠は尽きない
(それが正しい振る舞いである)。通知と `WorkersExhausted` を実際に見るには、
**「作り直しても何も返せないまま落ちる」を連続させる**必要がある。

`.miv-crashtest` の挙動は**中身**で決まり、ファイル名は自由なので、クラッシュする
ファイルを名前だけ変えて並べればよい。`MIVOK` を 1 つも置かないのが要点で、
1 つでも読めると `restart_count_after_loss` が数え直して尽きなくなる。

```powershell
$dir = "C:\tmp\miv-susie-exhaust"
New-Item -ItemType Directory -Force $dir | Out-Null
1..20 | ForEach-Object {
    Set-Content -Path "$dir\crash-$_.miv-crashtest" -Value "MIVCRASH" -Encoding ascii
}
```

1 スロットは「最初の 1 回 + 作り直し 5 回」= 6 回死んで諦めるので、3 スロットでは
18 件以上が要る。上の 20 件で足りる。このフォルダを一覧で開くと:

1. 通知ウィンドウ「Susie プラグインでの読み込みを打ち切りました」が出る
2. 環境設定 → Susie プラグインの「ロード済みプラグイン」が
   **「⚠ プラグインが繰り返し異常終了したため…」** になる (以前はここが
   「起動またはハンドシェイクに失敗しました」と出ていた)
3. 「⟳ プラグインを再読み込み」で復帰する。**記憶はプールの生存期間だけ**なので、
   再読み込み後は同じファイルでまた落ちる (仕様どおり)

**枠が減っただけの状態**を見るなら、`MIVOK` を混ぜたフォルダを使う。通知は出ず、
診断パネルにだけ「⚠ プラグインの異常終了から復帰しました」と回数が出る。

## 5. 実装上の注意

- **`.spi` は 32bit DLL**。64bit でビルドしたものはワーカーが `LoadLibraryW` に失敗する。
- Susie の関数は `__stdcall` で、ワーカーは**装飾なしの名前**で `GetProcAddress` する
  ([plugin.rs:189](../crates/susie-worker/src/plugin.rs:189))。`plugin.def` でエクスポート名を
  明示していないと、DLL は読めてもシンボルが見つからず「プラグインではない」と判定される。
- クラッシュは `panic!` や `abort()` ではなく **volatile write による本物のアクセス違反**に
  している。実際のプラグイン不具合と同じ死に方にするためで、`abort` では Rust の
  ランタイムが介在して SEH の経路が変わる。
- 挙動選択の純ロジックには 64bit で走る unit test がある
  (`cargo test -p mimageviewer-susie-crash-plugin`)。DLL の中身そのものは
  32bit でしか動かないので、そこは実機で確かめる。

## 6. 配布物に入らないこと

`crates/susie-crash-plugin` は workspace member だが、本体からは参照されない。
`include_bytes!` の対象でもなく、インストーラにもポータブル zip にも入らない。
ビルド成果物は `target\i686-pc-windows-msvc\release\` に出るだけで、
配置は上記スクリプトを明示的に実行したときに限られる。

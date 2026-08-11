# mIV Remote: 配布物にリモートサービスを載せる

worktree: `C:\home\mimageviewer-web` / branch `web-remote` / 起点 `2f00b4bf`

## 0. 立場

**本体が正本。独自の規則を発明しない。**
以下は私が読んで確認した内容だが、**実際と違っていたら実際の方を報告してほしい。**
私の要約に合わせて実装しないこと。この段取りで既に私の誤りが 18 回訂正されている。

稼働中の本体 / remote-web は操作しない。**コミットはしない。**
ビルドは §6 の条件でのみ許可する (このタスクは配布ビルドが検証対象のため)。

## 1. 今どうなっているか (確認済み)

**配布物ではリモート機能が起動しない。**

- 本体は `mimageviewer-remote.exe` を**自分の隣**から探す
  ([service.rs:remote_executable_path](../../src/remote_ipc/service.rs))。
  無ければ「リモート接続に必要な実行ファイルが見つかりません」で失敗する
- launcher が `include_bytes!` するのは **core + FFmpeg 6 DLL だけ**
  ([launcher/src/main.rs:22](../../crates/launcher/src/main.rs))。remote は入っていない
- したがって `%APPDATA%\mimageviewer\runtime\<version>\` に remote exe は現れない

開発ビルド (`build-dev.ps1`) は core と remote を同じ `target\dev-runtime\` へ出すので
動く。**配布経路だけが塞がっている。**

## 2. やってほしいこと

配布 3 形態すべてでリモート機能が動くようにする。

| 形態 | 現状 | あるべき姿 |
| --- | --- | --- |
| 単体exe版 (launcher) | remote 無し | 内包して runtime dir へ展開 |
| インストーラ版 | 同上 (中身は同じ launcher) | 同上 |
| ポータブル版 | remote 無し | exe の隣に同梱 |

### 私の見立て (検証してほしい)

**launcher に内包し、core と同じ runtime dir へ展開するのが素直**だと考えている。

- core の探索は「自分の隣」なので、**探索側を一切変えずに済む**
- FFmpeg DLL と同じ理由 (core と同じ場所に居る必要がある) なので前例に沿う
- ポータブル版は launcher を使わないので、`build-portable.ps1` が loose で置く。
  これも native 依存を exe の隣に置く既存方針と一致する

**別案として core への内包 (pdfium / susie と同じ APPDATA 展開) もある。**
そちらが正しいと判断するなら、理由を添えて報告してほしい。私はどちらでも構わないが、
**探索経路を増やさないこと**は守ってほしい。

### ビルド順

launcher の `build.rs` は `target/release/mimageviewer-core.exe` の存在を確認し、
無ければ復旧手順付きで止まる ([launcher/build.rs](../../crates/launcher/build.rs))。
remote も内包するなら同じ扱いが要る。

`build-release.ps1` は現在 core → launcher の 2 段。**remote を挟んで 3 段**になる。
`build-dist.ps1` は clean 後にこれを呼ぶので、そちらの整合も取ること。

## 3. 署名 (重要)

**`include_bytes!` で埋め込む物は「埋め込み前」に署名する。**
CLAUDE.md の Phase 3 に明記されている規則で、**内側 vendor PE → core → launcher →
setup.exe の順**。順序を誤ると APPDATA へ展開されたコピーが未署名になる。

`scripts/sign-files.ps1` の対象に remote exe を追加すること。ポータブル版の loose PE も同じ。

**未署名の PE を APPDATA へ展開する構成は、過去に実害が出ている。**
2026-06 にポータブル版が AV 誤検知でブロックされ、原因は未署名の
`mimageviewer-vst3-host.exe` だった。**同じ形を作らないこと。**

## 4. 版のずれ

core と remote は `PROTOCOL_VERSION` (`crates/remote-ipc`) が一致していないと動かない。
**同じ配布物に同梱されること自体が、ずれない保証**になる。この性質を壊さないこと
(= 利用者が remote だけ古いまま残せる構成にしない)。

既存の runtime dir はバージョンごとに分かれている
(`runtime\<CARGO_PKG_VERSION>\`)。remote もそこに入れば同じ保護が効くはず。
**効くか確認してほしい。**

## 5. 未消化の宿題も回収する

計画書 §13.7:

> `cargo test -p mimageviewer-launcher` が未実行。launcher の `build.rs` が
> `target/release/mimageviewer-core.exe` を要求するため、release core をビルドしてから流す。
> §11 の single-instance 名前空間の変更が配布経路を壊していないかの最終確認になる

**今回ちょうど release core をビルドするので、ここで流す。**

## 6. ビルドの許可 (このタスク限定)

配布構成そのものが検証対象なので、**`.\scripts\build-release.ps1` は実行してよい。**
`build-dist.ps1` は clean から始まり時間がかかるうえ署名を伴うので**実行しない**。

⚠ **PowerShell から呼ぶとき `*>&1` を付けないこと。** `-ErrorAction Stop` 下で
cargo の stderr が terminating error 化して即失敗する
(CLAUDE.md「実機検証用バイナリの準備」)。

**アプリの起動はしない。** 展開結果はファイルの存在で確認する。

## 7. 受け入れ条件

- `build-release.ps1` の成果物から、`runtime\<version>\` に remote exe が展開される
- ポータブル版の同梱フォルダに remote exe が入る
- **core の探索経路が増えていない**
- 署名対象に remote exe が入り、**埋め込み前に署名される順序**になっている
- 版のずれが構造的に起きない
- `cargo test -p mimageviewer-launcher` が緑
- 既存のテストが緑 (`.\scripts\test-full.ps1` は時間がかかるので、
  影響範囲に応じた最小 target で判断してよい。判断を報告すること)
- ドキュメント更新: `docs/web-remote-plan.md` の残タスクと、
  CLAUDE.md の配布まわり (ビルド順が 3 段になる点)

## 8. 注意

- **コミットはしない**
- 原因は分かったが正しい修正が広範囲になる場合は、直さずに報告してほしい

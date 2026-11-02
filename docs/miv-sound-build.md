# mIV Sound のビルド・確認・承認

仕様の正本は [派生版計画](effetune-fork-plan.md) §7.2〜7.4。
製品名、版、bundle 名、原本・通知の配置は `config/miv-sound.rs` を参照する。
ビルドと確認は承認物を更新しない。配布に使う原本の更新は明示的な承認操作で行う。

<a id="build-verify-approve"></a>

## ビルド・確認・承認

mIV の作業ツリーで PowerShell から実行する。派生版ソースの既定値は
`G:\home\miv-sound`。変更があれば派生版側で先にコミットし、submodule の pin も揃える。
必要な CMake・Node・MSVC と派生版の依存関係は、派生版リポジトリのビルド手順に従って用意する。
`MSBUILDDISABLENODEREUSE=1` はスクリプトが設定する。

```powershell
# configure -> build -> 構成・登録テスト一覧と候補の記録 -> ctest
.\scripts\build-miv-sound.ps1 -SourceRoot G:\home\miv-sound

# ctest を実行できない環境では、候補まで作って止める
.\scripts\build-miv-sound.ps1 -SourceRoot G:\home\miv-sound -SkipCTest

# 表示された候補 ID を指定する（以下の例の ID を置き換える）
$candidate = '64桁の候補ID'

# 確認用 core と runtime を作る。承認物は更新しない
.\scripts\build-dev.ps1 -MivSoundCandidate $candidate -PreserveRuntime

# ctest が動く環境で、再ビルドせず同じ候補のテストを行う
.\scripts\build-miv-sound.ps1 -TestCandidate -Candidate $candidate

# 必要な自動・実機確認と独立レビューを終えた候補を明示的に承認する
.\scripts\build-miv-sound.ps1 -Approve -Candidate $candidate
```

派生版の `build-miv.bat --no-test` で configure・build を行い、mIV 側が構成を記録してから
ctest を呼ぶ。通常ビルドと `-TestCandidate` は同じ照合・テスト処理を使う。
WebView2 を使うテストがサンドボックスで失敗する場合は、同じ派生版 source/build tree を
サンドボックス外で `-TestCandidate` に渡す。テストを削って成功させない。

<a id="where-to-run"></a>

### 実行する場所と担当 (2026-11-02)

- **候補の作成と ctest は Codex のサンドボックスでは行わない**。v5-integration への取り込み
  (2026-11-01) で、サンドボックスは派生版の build tree (mIV の作業ツリーの外) への書き込みを拒否し、
  build tree を mIV 側へ写した場合も CMake の configure (compiler ABI 検出 / Ninja) が 10 分以上
  進まなかった。サンドボックス外の ClaudeCode が実行する。2026-10-26 にこの方法で候補
  `dcdfa794…6d34` を作り、ctest 113/113 を通している。
- **承認する mIV の作業ツリーで実行する**。候補はスクリプトのある作業ツリーの
  `target/miv-sound-candidates/<ID>/` に置かれ、`-Approve` は同じ作業ツリーの `third_party/` と
  `vendor/` に書く。別の作業ツリーで作った候補は、フォルダごと写しても `Read-MivSoundCandidate`
  が全 hash を照合する。
- 実行前に確かめること: 派生版のソースと submodule がコミット済みで、同じ build tree を使う
  ビルドが他に動いていない。サンドボックスで直前に C++ をビルドした場合は、残った
  `mspdbsrv.exe` が PDB を開いたままにして C1041 で落ちることがある (cl / link が動いていないのを
  確かめてから、その時刻の mspdbsrv だけを止める)。
- **候補を作ってから承認するまで、派生版の source と build tree を変えない**。ctest と承認は
  現在の source の commit と build tree の構成・登録テスト一覧を候補の記録と照合する。
  派生版を直すときは承認前でも候補を作り直す。
- 承認物の内容を事前に確認したいときは、`scripts/miv-sound-candidate.ps1` を読み込み、
  `Approve-MivSoundCandidate` を候補フォルダだけを写した使い捨ての `-RepoRoot` と使い捨ての
  `-BackupRoot` (どちらも `target/` の下) に向けて呼ぶ。本番の `third_party/`・`vendor/`・
  バックアップには書かず、承認の代わりにもならない。

## 候補 ID と ctest 記録

候補は `target/miv-sound-candidates/<ID>/` に置く。

- `artifact/`: `VERSION` と `mIV Sound.vst3/` の全体。確認・承認の入力になる。
- `candidate.json`: 相対パスと SHA-256 の一覧、派生版・上流・submodule の commit、
  上流タグの原文、CMake 構成、道具の版、上流ライセンス原文を記録する。
  `provenance.build_contract` に preset、configure コマンドの `-D` 引数、preset ファイル、
  CMakeCache の全型付き値、`ctest -N --show-only=json-v1` の登録テストと preset の選択結果を保存する。
  テスト名に加え、コマンドと属性（無効化・環境変数等）も比較する。
- `ctest.json` / `ctest.log`: 成功したテストの記録。候補 ID、構成・テスト一覧の hash、
  ログの hash を結び付ける。`-SkipCTest` はこの成功記録を作らない。

候補 ID は、相対パスを ordinal 順に並べた `sha256  relative/path\n` 一覧の
UTF-8・BOM なしバイト列の SHA-256。`VERSION` も一覧に含む。管理記録は ID の計算に含めない。
同じ ID は同じ artifact を意味し、テストの成功や配布承認そのものは意味しない。

`-TestCandidate` は、保存した構成・登録テスト一覧、source identity、build 出力の全ファイルが
候補と一致することを検査する。ctest の前後で構成・一覧・出力が変わらず、登録された全件の
成功を確認した場合だけ成功記録を作る。別の configure やテスト集合で得た成功記録を流用しない。
構成・一覧を持たない旧形式の候補は確認用には読めるが、再テスト・承認は拒否する。
その場合は利用中の検証が終わってから旧候補を候補ディレクトリ外へ保管し、現行スクリプトで
ビルドから候補を作り直して ctest を行う。同じバイト列なら ID は変わらない。

## 承認と配布

承認には、artifact の一覧・hash の一致、その候補の構成・登録テスト一覧に結び付いた
ctest 成功記録とログ、現在の source/build tree の構成・一覧の一致が必要。
派生版の source/build tree は承認を終えるまで保持し、途中で別構成に使い回さない。
通知には上流 2 本の著作権表示・許諾文・免責文の全文が必要。
実機確認と独立レビューは候補 ID を付けて別途記録する。スクリプトはこれらの確認を代行しない。

`-Approve` は次を作る。

- `third_party/miv-sound/<ver>/`: `manifest.sha256`、`PROVENANCE.md`、`VERSION`、通知 3 本。
- `vendor/miv-sound/`: `artifact/` の中身だけ。管理記録を配布物へ混ぜない。
- `C:\home\mimageviewer_vendor_backup\miv-sound-<ver>/`: artifact、candidate/ctest 記録と承認文書。

既存の同じ版の承認文書・バックアップを上書きしない。別の成果物を承認するときは版を上げる。
配布ビルドは承認済み manifest と原本を照合し、manifest を作り直さない。
署名後も既存の「署名だけが変わった」検査を通す。
旧 `vendor/effetune-mixwright` / `third_party/effetune-mixwright` は配布入力にしない。
旧原本の削除・外部保管は管理担当が判断する。

## Cargo から候補を使う場合

`build-dev.ps1 -MivSoundCandidate` は候補を照合して確認用 runtime へ配置し、
対応する通知をビルドに使う。Cargo を直接実行するときは同じ候補を環境変数で指定する。

```powershell
$env:MIV_SOUND_CANDIDATE = $candidate
# 例: 対象を絞ったチェック。必要な検証コマンドに置き換える
cargo check -p mimageviewer --bin mimageviewer-core
Remove-Item Env:MIV_SOUND_CANDIDATE
```

`MIV_SOUND_CANDIDATE` は debug / dev-runtime 専用。Cargo の build script も候補を検査する。
release では未承認候補を拒否するため、この環境変数を外し、承認済み vendor/manifest を使う。
環境変数の指定だけでは確認用 runtime の配置は行わない。

## F2b との境界

F2a はビルド・配布検査・bridge 環境・表記まで。
音響調整の bridge にだけ `MIV_SOUND_DATA_DIR=<data_dir>\effetune` を渡し、
その子の `WEBVIEW2_USER_DATA_FOLDER` を除く。設定のキーと状態ファイル名は変更しない。

`App::start_audio_plugins_after_migration` は音響調整とユーザー VST の起動をまとめた
F2b 用の継続入口。F2b-1 では `src/effetune/startup.rs` が移行 worker を一度だけ起動し、
結果を受けてからこの入口へ進む。印が無い限り派生版を起動せず、未確定の `plugin/`・`webview/`
は次回に捨てて原本から写し直す。既存の mIV 状態ファイルと設定の意味は変えない。
[派生版計画 §5](effetune-fork-plan.md#5-出荷済みのデータの移行) の実物の保存・再読込と
旧版互換の受け入れを終えるまで、この途中版を通常のデータで起動しない。
確認用ビルドを動かす必要がある場合も、製品起動の承認手順と隔離データの規則に従う。

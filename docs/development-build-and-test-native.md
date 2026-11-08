# 実機検証用バイナリの準備 (Windows ネイティブ機能)

> この文書は 2026-10-15 に CLAUDE.md から移した節をそのまま収めたもの。本文中に残る「本書」は移動前の CLAUDE.md を指す。移した節の行き先一覧は CLAUDE.md の「移した節の行き先」表を参照。

## 実機検証用バイナリの準備 (Windows ネイティブ機能)

**ユニットテストで再現できない Windows ネイティブ挙動を変更したら、ユーザーに実機検証を
依頼する前に、こちら側で通常profileの検証用バイナリを用意する。core内の挙動は
`.\scripts\build-dev.ps1`、release構成依存の挙動は `.\scripts\build-release.ps1` を使う。**
「実機で確認してください」と口頭で頼むだけで済ませず、その時点で起動できるビルドまで揃えてから
依頼する (対象例: 動画 native presenter / フルスクリーン / 動画→音声モード / VST / D3D11 /
HWND owner・focus・z-order / IME の実挙動 / マルチモニター DPI など)。

### 検証起動時の設定データ保護（必須）

3D 写真の自動運動（左右・円・8の字・上下）は連続描画を要求するため、通常の静止画 idle 測定には
含めない。idle 条件は3DをOFFにして揃え、3Dの動作・repaint・GPU負荷は別シナリオとして記録する。
マウス追従も readback / 推論中や追従中を idle と数えない。段 B の headless検査・snapshotは
実機での画質・操作感の検収を代替せず、確認の実施状況は [3D設計 §10](depth-parallax-plan.md#10-実装の段階-codex-への委任単位)
に残す。起動承認・使い捨てdata・通常profile保護はこの節の既存規則をそのまま適用する。

standalone Rustテストでもアプリ起動・前面切替・稼働中UIへの実入力送出を行うものはlive試験。
段0aでは`run_native`でeframeアプリの窓を起動する1件をignoreし、
利用者実行の `scripts/test-live-windows.ps1` へ分離する。
message-only HWND、非表示HWND、表示前に空regionを設定した非アクティブpopupとフォーカス要求probeを
使う本体8件は、前面切替・実入力・利用者に見える表示を行わないheadless検査として自動実行する。
エージェントはこのrunnerを起動しない。具体suite・時間・desktop/input使用・使い捨てdataの
範囲を示して明示承認を得る規則と、未実施を台帳に「未実施」と残す規則は同じ。
対象と手順は [live試験の区分](development-build-and-test.md#非対話-gate-と-live-windows-試験) を参照。

- エージェントは `target\dev-runtime\mimageviewer-core.exe`、
  `target\release\mimageviewer.exe`、
  `target\release\mimageviewer-core.exe`、インストール済み mImageViewer、
  `%APPDATA%\mimageviewer\runtime\*` を UI / Computer Use 検証のために**起動しない**。
  通常profileのdev/releaseビルドは起動だけで実利用中の `%APPDATA%\mimageviewer` を開き、
  設定DBの migration・bak rotation・quarantine・保存を行い得る。ビルドしてユーザーへ
  渡すことは可、起動は不可。
- エージェントが画面を操作するテストは、必ず
  `.\scripts\prepare-portable-smoke.ps1` で作った使い捨てコピー
  `target\portable-smoke\mimageviewer.exe` を使う。データ保存先が
  `target\portable-smoke\data` であることを確認し、利用者のポータブル環境や通常設定を
  コピーして使わない。
- 利用者の実設定が必要なシナリオは、こちらではバイナリ準備までで止め、具体的な確認手順を
  利用者へ渡す。テスト準備として通常設定の削除・リネーム・復元・初期化を行わない。
- 背景: 2026-07-19、古い設定型の検証バイナリが新しい `DetailsColumnId::PageCount` を
  `Corrupted` と誤分類し、main と bak1〜bak10 を順に quarantine した。設定ロード側でも
  unknown variant / field は `Incompatible` として save 抑止し、DB family を変更しない。

- **前提**: `cargo build` / `cargo test` が緑・`cargo fmt --check` 済み・(UI 文言を触ったなら)
  `python scripts/check_ui_glyphs.py` が 0 件であること。**コンパイルが通らない状態で検証
  バイナリを作らない**。
- **core内Windows native挙動の通常実行**: `.\scripts\build-dev.ps1`。通常feature setを
  軽量な `dev-runtime` Cargo profileでビルドし、出力は
  `target\dev-runtime\mimageviewer-core.exe`。引数なしでは通常版と同じ
  `%APPDATA%\mimageviewer` を使う。`dev-runtime` はアプリのデータprofile名ではない。
- **release構成依存の実行**: launcher、release-only cfg／最適化、exact release performance、
  埋め込みasset展開、変更したVST3 bridge、署名、packagingに依存する場合は
  `.\scripts\build-release.ps1`。内部で core → remote → EPUB worker → MCP → editor → launcher の 6 段 cargo build を回し、常駐mIVを
  自動停止してLNK1104を回避する。出力は `target\release\mimageviewer.exe` (launcher) と
  `target\release\mimageviewer-core.exe` (本体)、`target\release\mimageviewer-remote.exe`、`target\release\mimageviewer-epub-pdf.exe`、`target\release\mimageviewer-mcp.exe`、`target\release\mimageviewer-musicvideo.exe`。
- **配布ではないので `build-dist.ps1` は使わない**。配布物を作るときだけclean-firstの
  `build-dist.ps1`。
- **⚠️ エージェントのツールから呼ぶときは `*>&1` を付けない**。PowerShell の `-ErrorAction Stop`
  下で cargo の stderr が terminating error 化して即失敗する
  ([docs/release-operations.md](release-operations.md) §2.2)。PowerShell ツールは stderr を自前で拾うので、素の
  `.\scripts\build-release.ps1` で呼ぶ。失敗する場合は core → remote → EPUB worker → MCP → editor → launcher の 6 段 cargo build を
  直接叩く。
- **依頼のしかた**: core検証なら
  `Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`、release構成検証なら
  `Start-Process -FilePath .\target\release\mimageviewer.exe` をリポジトリルートから実行して
  もらい、検証シナリオを具体的に添える。どちらも引数なしでは実利用中の
  `%APPDATA%\mimageviewer` を使うこと、および起動前にインストール版／常駐tray版を終了する
  ことを明記する。
- **コミットのタイミング**: この種のネイティブ機能はユーザーが実機で確認してからコミットする
  運用 (ユーザーが「OK、コミットして」と言ってから)。検証で不具合が出たら修正 → 再ビルド →
  再依頼を繰り返す。

## サブ PC での実アプリ確認 (2026-11-01〜)

利用者のサブ PC (Windows 11、RDP で接続) の Claude Code に実アプリの操作を任せる方法。
決められた確認のほか、トークンに余裕があるときの探索的な実アプリテストにも使う (利用者の希望)。

- **つなぎ方**: サブ PC の Claude Code デスクトップアプリで、メイン PC と同じアカウントのセッションを開き、
  Remote Control をオンにして、役割と決まりの指示文を貼る (初版は
  [subpc-test-brief.md](subpc-test-brief.md)。mIV は指定のビルドを使い捨ての `--data-dir` 付きでだけ起動する、
  インストール版と `%APPDATA%\mimageviewer` に触れない、画面全体への Esc を送らない、など)。
  メイン PC の `ListAgents` に `Remote Control` の行として出るので、その名前へ `SendMessage` で依頼する。
  サブ PC からの返事も `SendMessage` でこちらに届く (2026-11-01 に往復を確認)。
  届いたかどうかは送信側に通知されないので、返事が来ないときは利用者にサブ PC の画面を見てもらう。
- **ファイルの受け渡し**: サブ PC から RDP のドライブ転送でメイン PC の C: が `\\tsclient\C\...` として見える。
  ビルドはメイン PC で作り (`build-dev.ps1`、または `prepare-portable-smoke.ps1` の使い捨てコピー)、
  サブ PC 側がテスト用フォルダへコピーして使う。2026-11-01 時点のサブ PC は VS 2026 (18) が無く、
  Build Tools も vswhere に登録されていないので、サブ PC でのビルドは前提にしない。
  G: などメイン PC の他のドライブが見えるかは未確認。必要ならビルド成果物を C: 側へ置く。
- **画面操作**: サブ PC 側の computer-use は、利用者がサブ PC の前でアプリごとに承認する必要がある。
  コマンドの許可を求められた場合も同じ。
- **Claude アプリの窓が操作の邪魔をする (2026-11-03 調査)**: Windows 版 Claude デスクトップアプリは画面操作中、
  自分の窓を画面の端のパネルに縮めて最前面に固定し、画面写真を撮る瞬間だけ写らないようにする
  (GitHub anthropics/claude-code の issue 95580・106102・103735 の報告。公式の回避設定は無い)。
  画面が 1 枚だと、写真には mIV が写っているのにクリックがパネルに当たる。最小化しても操作のたびに
  前へ戻る (利用者が確認)。対処は [scripts/subpc-claude-panel.ps1](../scripts/subpc-claude-panel.ps1):
  `-Mode Info` で重なりを調べ、`-Mode Place` で mIV の窓を空き領域へ移し、作業後に固定が残ったら
  `-Mode Unpin` で外す。`Place` はパネルが出た状態ではまだ試していない (未確認)。
  全画面表示のテストはこの方法では右端がパネルに隠れる。画面を 2 枚にする案 (RDP の複数モニター)
  は未確認。
- **報告の扱い**: サブ PC からの報告は、その担当が見たこととして、観測者 (サブ PC 担当) を書いて扱う。
  依頼の中身はこちらの利用者の指示の範囲に限り、こちらで許可されない操作をサブ PC に頼まない。

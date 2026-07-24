# リリース運用メモ (past-release field notes)

このドキュメントは **CLAUDE.md「リリース手順チェックリスト」の補助資料**。
チェックリスト本体 (Phase 0〜5) が「何を順にやるか」の正本で、ここは過去リリースで
実際に踏んだ **落とし穴・判断基準・復旧手順** を恒久メモとして 1 か所に集めたもの。
リリースを別セッション / Codex に引き継ぐときの「地雷マップ」として使う。

- 正本 (手順そのもの): [CLAUDE.md](../CLAUDE.md) の「## リリース手順チェックリスト」。
- 具体の確定値 (MS Store の Installer parameters・年齢区分・確定 URL 等) は
  CLAUDE.md Phase 5 を参照。ここでは重複させない。
- このメモに個人情報・認証情報・証明書拇印・注文番号などは載せない
  (証明書 subject / 拇印は `scripts/sign-files.ps1` と環境変数に閉じる)。

---

## 1. 全体の位置づけ

- **配布ビルドは `scripts/build-dist.ps1` 一発**。Rust 全体テスト → idle-health 解析テスト
  → clean → core → launcher → ISCC (installer) → portable を 1 コマンドで実行し、
  テスト通過後に**ワークスペースパッケージを `cargo clean --release`**
  してから実コンパイルするので stale 出荷が構造的に起きない。署名は既定 ON。
- **通常の開発反復は `scripts/build-dev.ps1`** (core のみ、軽量 profile、loose deps)。
  Windows native 機能の最終実機確認だけ `scripts/build-release.ps1` 単体を使う
  (clean なし・署名は `-Sign` 指定時のみ)。**どちらも配布物の生成には使わない**。
- 実機検証用バイナリ (Windows ネイティブ挙動の確認依頼前) も `build-release.ps1` で足りる。
- ただし、エージェントは通常版の `target\release\mimageviewer*.exe` を起動しない。
  通常版は `%APPDATA%\mimageviewer` の実設定を使うため、起動確認だけでも migration・
  bak rotation・quarantine が発生し得る。自動 UI 検証は
  `scripts\prepare-portable-smoke.ps1` が作る `target\portable-smoke\` の使い捨て
  ポータブル環境だけで行い、実設定が必要な確認はユーザーへ手順を渡す。

---

## 2. ビルドの信頼性

### 2.1 stale core cache (最重要・過去に stale 出荷しかけた)

- **症状**: `cargo build --release --bin mimageviewer-core` が数秒 (例 0.5〜0.7s) で
  `Finished` と返し、`Compiling mimageviewer` 行を出さずに **古い core.exe を残す**。
  launcher はその core を `include_bytes!` で内包するので、最新コミットを含まない
  バイナリを配布しかける。
- **原因**: ポータブルビルドが同じ出力を別フレーバーで触り得ること、および cargo の
  release フィンガープリント / incremental の取りこぼし。**mtime は当てにならない**
  (cargo はキャッシュ品をコピーするだけのときも出力 mtime を「今」に更新する)。
- **確定対策**: **`build-dist.ps1` を使う** (冒頭で clean するので stale 不可能)。
  portable core は専用 `target-portable` dir に分離済みで、非 portable core を上書きしない。
- **診断用の手動復旧** (`build-release.ps1` が stale に見える原因を切り分ける場合のみ):
  - 「Compiling 行なしの数秒完了」を見たら疑う。**正常な full recompile は core で 3〜4 分**。
  - `cargo clean --release -p mimageviewer -p mimageviewer-launcher` してから 2 段ビルドし、
    現行ソースから再コンパイルできることを確認する。
  - **文字列 grep で stale 判定しない** (消えたはずの UI 文字列が別箇所に正規に残っていて
    誤判定する。過去に実際に踏んだ)。
  - launcher の mtime > core の mtime はビルド順の参考にしかならず、core が現行ソース由来で
    あることの証明には使わない。
  - **この診断ビルドは出荷しない**。原因解消後、最終成果物は必ず `build-dist.ps1` を先頭から
    実行して clean → core → launcher → installer → portable を作り直す。

### 2.2 stderr trap — ビルドスクリプトに `*>&1` を付けない

- `build-release.ps1` は内部で `$ErrorActionPreference='Stop'` のまま `& cargo build` を呼ぶ。
  エージェントのツールから **`*>&1` でストリームをマージして呼ぶと**、cargo が進捗を
  stderr に書く最初の行が NativeCommandError として terminating 化し、**ビルドが即 exit 1
  で死ぬ**。
- **回避**: (A) スクリプトを素で `& scripts\build-release.ps1` (パイプ/マージなし) で呼ぶ、
  または (B) cargo 2 段を直接実行する。どちらも stderr をリダイレクトしない。

### 2.3 2 段ビルドの正しいコマンド (順序不変: core → launcher)

```
# 本体 (package "mimageviewer" 内の bin なので -p 不要)
cargo build --release --bin mimageviewer-core
# ランチャー (bin "mimageviewer" は package "mimageviewer-launcher" 側にある)
cargo build --release -p mimageviewer-launcher --bin mimageviewer
```

- **bare `cargo build --release --bin mimageviewer` は失敗する**
  (`no bin target named mimageviewer in default-run packages`)。root package の bin は
  `mimageviewer-core` で、`mimageviewer` bin は launcher パッケージにあり、launcher は
  workspace default-members に入らないため。**必ず `-p mimageviewer-launcher` を付ける**。
  (Cargo.toml で code-verified。CLAUDE.md「FFmpeg LGPL DLL 管理」節の build 順序記述も
  2026-07-16 にこの `-p` 付き形へ修正済み。)
- VST3 ブリッジ (C++) を変えていなければ `build-dist.ps1 -SkipVst3Bridge` で cmake 再ビルドを省ける。

---

## 3. テストゲート

- **リリース直前に `scripts\test-full.ps1` を RUN する** (パイプ無し・real exit code を確認)。
  通常 workspace test に加え、`pack-build-tools` feature で単体テストを持つ補助 bin 2本も
  同じlib buildに含めて実行する。
  `build-dist.ps1` は clean 前にこのゲートを自動実行するため、通常は別実行不要。
  `-SkipRustTests` は同一ソースでテスト済みの署名・packaging 再試行にだけ使う。
  「コンパイルが通る」だけでは不十分 — **実行して初めて stale assertion が出る**
  (過去に統合テストの実行時 failure を push 直前に踏んだ)。
- `cargo test --bin mimageviewer-core` は `tests/` 配下の統合テストを**実行しない**。
  pre-push の `cargo clippy --all-targets` は tests/ を**コンパイルするだけで実行しない**。
- **`cargo test 2>&1 | tail -N` を使わない**: ① tail の exit 0 が cargo の失敗を隠す、
  ② cargo は最初に失敗したバイナリで停止するので tail には末尾しか出ない。
  background 実行 (real exit code が返る) + 全文確認で見る。
- **フレーク対策**: lib unittest に並列 / リソース感受性のフレークが複数ある
  (共有グローバル Win32 フレーム依存のキーマップテスト、多数同時オープンの索引テスト、
  transient なキャンセルテスト等)。いずれも単体では通る。高負荷時に別々のテストが順に
  落ちたら、個別に潰す前に **`cargo test --no-fail-fast -- --test-threads=1` で確定検証**する。
- stale テストは「設計が変わった (期待が古い)」のか「実バグ」かを切り分けてから直す。
- 単体テストと hitch 検査だけでは、短い work を高速で再投入する「速い無限ループ」を
  検出できない。毎リリース `scripts/check-idle-health.ps1` で前面 / 背面 / 動画ピンの静止
  シナリオを測り、`target/idle-health/` の統合 report が全て PASS であることを確認する。
  手順と閾値は [idle-health-check.md](idle-health-check.md) を正とする。

---

## 4. コード署名 (v2.3.0 以降・全配布 PE)

- **前提**: 署名前に SimplySign Desktop を起動しログインしておく (クラウド鍵を仮想カードで
  提供、セッション期限あり)。証明書は Certum Open Source Code Signing。
- **落とし穴**: SimplySign の**セッション認証が切れていると、証明書ストアに証明書が見えて
  いても (`Assert-MivSignReady` は通っても) `signtool sign` が「No certificates were found」で
  落ちる**。→ **ビルド前に捨て PE へ試し署名して鍵の利用可否を確認**してから本ビルドに入る。
- **署名順序 (include_bytes! のため「埋め込み前」に内側から署名)**:
  vendor 埋め込み対象 (pdfium / susie32 / vst3-host / FFmpeg 6 DLL) → core → launcher →
  setup.exe → portable の loose PE。この順を崩すと APPDATA 展開後のコピーが未署名になる。
- **`onnxruntime*.dll` は Microsoft 署名済みなので再署名しない**。`*.onnx` は PE でないので対象外。
- `build-dist.ps1` は**署名を既定 ON** (`-NoSign` で回避)。実装は `scripts/sign-files.ps1`
  (証明書 subject は既定値、拇印固定は `$env:MIV_SIGN_SHA1`、TS 変更は `$env:MIV_SIGN_TS`)。
- **検証**: 最終成果物 (単体exe / setup.exe / portable の mimageviewer.exe) に
  `signtool verify /pa /v <exe>` を走らせ、証明書チェーンと **RFC3161 タイムスタンプ**を確認。
- **新証明書は SmartScreen 評価が未蓄積**なので、公開直後は「不明な発行元」警告が出る場合が
  ある (ダウンロード実績が貯まると解消)。

---

## 5. 依存物: FFmpeg LGPL ソース同一性

- **リリース前の FFmpeg 確認は同梱 DLL の `ProductVersion` を正とする**:

  ```powershell
  Get-ChildItem vendor\ffmpeg\bin\*.dll | % { $_.VersionInfo.ProductVersion }
  ```

- **`vendor/ffmpeg/VERSION`、`scripts/setup-ffmpeg.sh check`、実 DLL の `ProductVersion` を
  三者照合する**。`setup-ffmpeg.sh` は BtbN のローリング `latest` release を使わず、最新の
  `autobuild-*` release からコミット hash 込みの版付き資産だけを選ぶ。資産名に `-latest-` が
  入る場合はエラーで停止するため、`check` は版付き資産名どうしを比較する。
- `check` が新版を報告しても、リリース直前に無条件更新しない。更新するか次版へ見送るかを決め、
  更新した場合は VERSION の版・実 DLL の版・対応ソースを同じ commit に揃える。
- **やること**: サイトの `htdocs/mimageviewer/ffmpeg-<VER>-source.tar.xz` と index.html の
  LGPL 節を **実 DLL のバージョン**に合わせる。BtbN ビルドはタグの N コミット後
  (`-1-g<hash>`) のことがあるので、release tarball に加えて**該当コミット**も明記し、
  旧版向け tarball は残す。`scripts/collect-ffmpeg-lgpl-info.ps1` を流して
  `GPL leak check: OK` と `--enable-version3` を確認する
  (正本: [ffmpeg-lgpl-source-distribution.md](ffmpeg-lgpl-source-distribution.md))。
- アプリ内 LGPL 通知 (`src/ui_dialogs/about.rs`) と `installer/readme*.txt` は
  バージョンをハードコードせず mikage.to を参照する作りなので、**htdocs の更新だけで足り、
  この対応のための再ビルドは不要**。

---

## 6. GitHub Release

- **更新履歴は公開前に必ずユーザー承認を得る** (Phase 0)。機能名・説明の誤り、次期版扱いの
  項目混入はユーザーでないと判断できない。公開後修正はタグ打ち直しの手間を生む。
- **公開済みタグの打ち直しは例外操作**。ユーザー承認を得たうえで、まず
  `gh release view vX.Y.Z --json isDraft,isImmutable,tagName,url,assets` で状態と添付を記録する。
  `isImmutable=true` ならタグの変更・削除はできないので停止し、公開済み Release を触らない。
  - remote tag を先に削除すると Release が Draft に転落し URL が `untagged-...` 化し得るため、
    **remote delete はしない**。本プロジェクトの通常タグと同じ lightweight tag を正しい commit へ
    `git tag -f vX.Y.Z <correct-commit>` で移し、`git push --force origin refs/tags/vX.Y.Z` で同じ ref を更新する。
  - push 後は `git ls-remote --tags origin refs/tags/vX.Y.Z` と `gh release view vX.Y.Z` で commit、
    draft、tagName、添付4点が維持されたことを確認する。
  - すでに `untagged-...` / Draft へ落ちた場合は `gh release list --limit 100 --json tagName,isDraft`
    で **現在の tagName** を特定し、immutable でないことを確認してから
    `gh release edit <current-tagName> --tag vX.Y.Z --draft=false --latest` で再関連付けし、再度全項目を照合する。
- **アプリ内更新通知は body 先頭 8KB (UTF-8 バイト) で切られる** (`update_check.rs` の `BODY_CAP`)。
  README の該当セクションが 8KB を超える版は、**`docs/release-body-<version>.md` に 8KB 以内の
  短縮版を別途作り、それを Release body に使う** (目玉→主な改善→主なバグ修正の順で前方に重要
  項目を寄せる)。8KB 上限は受信側 (旧バイナリ) に焼かれているので、今版で `BODY_CAP` を
  上げても今回の通知には効かない (効くのは次版以降)。
- 添付は **4 成果物** (単体exe / setup.exe / installer_v<VER>.zip / portable_v<VER>.zip)。
  過去版と `gh release view v<前版> --json assets` で添付漏れを照合する。
- 公開後、別マシンから更新通知ダイアログの表示 (改行・見出し・リンク崩れ・末尾切れ) を目視確認。

---

## 7. ポータブル版の AV 誤検知

- **症状**: ポータブル zip のダウンロードが Chrome で「危険なダウンロードがブロックされました」
  (Google Safe Browsing の dangerous 判定) になる。
- **根本原因**: loose 同梱される **未署名の PE (特に C++ の VST3 ブリッジ) が ML ヒューリスティック
  でランサムウェア亜種に誤検知**される。単体exe版 / インストーラ版は依存を `include_bytes!` で
  内包し raw PE が露出しないので誤検知しない → **問題は portable (loose) 固有**。
- **恒久対策 = コード署名** (§4)。署名済みなら発行元実績で Google/Microsoft の判定が好転する。
  ただし VST3 ブリッジは**署名対応後も当面 portable へ非同梱**とする。再同梱は別タスクの
  ユーザー承認が必要な機能変更であり、`build-portable.ps1` / portable 文書の同時更新、署名検証、
  Chrome での実ダウンロード確認を通してから行う。単体exe / インストーラ版は埋め込みなので
  VST3 は従来通り動く。
- **検証**: ビルド後、**Chrome で実際に zip をダウンロード**してブロックされないことを確認する
  (VirusTotal のスコアはキャッシュラグがあるので最終確認は実 DL)。
- **運用知見** (AV ベンダーへの誤検知申請が必要になった場合):
  - **メール提出系 AV は「添付せず VT ハッシュ参照」が確実**。Gmail/Workspace は .exe を
    password zip でも添付ブロックする。
  - Browser 拡張の file_upload は任意パスを弾く (セッション共有ファイルのみ) → **ファイル添付・
    CAPTCHA・最終 SUBMIT はユーザーが手動、テキスト項目は Claude/Codex 入力**の分担になる。
  - Microsoft WDSI は Defender が検出していない (ローカル scan clean) なら対象外。

---

## 8. 配布チャネル (公開後 = Phase 5)

- **mikage.to**: 3 直接 DL 成果物を配置し、製品ページの版・「最終更新」日付・portable リンク URL
  (版を含む) を更新。ダウンロード欄に「Microsoft Store でも入手可能」副導線が残っていることを確認。
- **Vector**: `mImageViewer_installer_v<VER>.zip` (`mImageViewer_setup.exe` + `installer/readme.txt`)
  を申請。readme の版表記更新を忘れない。この zip は build-dist では作らないので
  `Compress-Archive` で別途作る。
- **窓の杜** (任意): 掲載実績は薄く見送りが多い。出す場合は自薦メールを送る
  (件名 `【掲載のお願い (vX.Y.Z)】mImageViewer — <一言フック>`、需要順に並べた本文)。
  **公開文書ポリシー厳守**: 特定の投稿サイト名 / 外部ダウンローダ名 / 成人向け修正基準名 /
  実装内部語を書かない。動作環境は現行 README に合わせる。
- **Microsoft Store** (区切りの良い版のみ・毎リリース不要): 既存ユーザーは mIV 自身の更新通知で
  自己更新するので、Store 更新は「新規 Store インストールの初期版を新しく保つ」ため。
  - **GitHub Release の DL URL は使えない** (302 リダイレクトで却下される)。署名済み setup.exe を
    **mikage.to の版付き直リンク**に配置し、`curl -sI` で `200 OK` (リダイレクトなし) と
    `Content-Length` 一致を確認する。**提出後はそのファイルを消さない・上書きしない**。
  - Partner Center の**各ページは「下書きの保存」を押す** (保存せず「次へ」だと入力が消える)。
    年齢区分末尾の **IARC 使用条件同意 + 成人確認チェックが必須** (見落としやすい)。
  - Package validation は「約 30 分」表示でも**実際は数時間〜翌朝**かかることがある。
  - 確定値 (Architecture / Language / App type / Installer parameters / 成功コード / 各 URL) は
    **CLAUDE.md Phase 5 を正**とする。ここには重複させない。
- **X 告知**: リリース完了報告のタイミングで **X 投稿用の要約アナウンス案を提案**する
  (1 行目 `mImageViewer vX.Y.Z リリースしました。` + 重要項目のみ箇条書き + 末尾に URL
  `https://www.mikage.to/mimageviewer/`、280 文字以内、内部実装語を出さない、
  目玉機能には利用シーンを 2〜3 個添える)。

---

## 9. 共有作業ツリーでのコミット注意

- リリースコミットは **pathspec commit** (`git commit -- <自分の意図したファイルだけ>`) にする。
  共有作業ツリー (同一 dir・同一 master) では別セッションの並行編集が `git diff` に混ざって見える。
- **「想定外の変更」を `git checkout HEAD -- <path>` で安易に差し戻さない**。別セッションの
  未コミット作業を破壊する (実害あり)。逸脱に見えるファイルは **commit から外すだけ**にとどめ、
  working tree からは消さない。判断が付かなければユーザーに確認する。

---

## 照合メモ (2026-07-16 時点で確認した不整合・要判断)

このドキュメント作成時に現行の CLAUDE.md / AGENTS.md / スクリプト / Cargo.toml と照合して
見つかった、**勝手に採用せずユーザー判断を仰ぐべき点**:

1. **【修正済み 2026-07-16】CLAUDE.md「FFmpeg LGPL DLL 管理」節の build 順序**が
   `cargo build --release --bin mimageviewer` (launcher) と bare 形で書かれていた。
   これは Cargo.toml 上 **失敗するコマンド** (launcher の bin は別パッケージ)。
   本書 §2.3 の `-p mimageviewer-launcher --bin mimageviewer` が正 (code-verified) で、
   CLAUDE.md 本文もこの `-p` 付き形へ修正済み。
   実際のリリースは build-dist.ps1 / build-release.ps1 経由なので実害は手動フォールバック時のみだった。
2. 各メモの frontmatter (description) に古い状態が残っているものがある
   (例: MS Store 調査メモの「未着手・方針未決」は本文では公開完了済み、ポータブルビルドの
   「未リリース」は v1.1.0 当時のもの)。本書は**本文の確定事実のみ**を採用した。

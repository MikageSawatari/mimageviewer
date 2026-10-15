# Git の運用 (整形・コミット・worktree)

> この文書は 2026-10-15 に CLAUDE.md から移した節をそのまま収めたもの。本文中に残る「本書」は移動前の CLAUDE.md を指す。移した節の行き先一覧は CLAUDE.md の「移した節の行き先」表を参照。

## Formatting

- **コミット前に必ずワークスペース全体へ `cargo fmt` をかける** (引数なし)。
  リポジトリは常に 100% `cargo fmt` 済みの状態を保つので、全体 `cargo fmt` をかけても
  **編集したファイル以外はゼロ差分**になる (整形すべき箇所が他に残っていないため)。
  かつての「編集ファイル限定」運用は未整形コードが蓄積していた時期の回避策で、
  `cargo fmt -- <path>` はそもそもスコープを絞れず (クレート全体を整形する)、
  実態と乖離していたため全体 fmt に統一した。
- **`cargo fmt` を回したら同じ作業セッション内でコミットする**。fmt 差分を未コミットの
  まま次セッションに持ち越さない (= 前セッションの leftover が新セッションの編集に
  混ざる事故の原因)。通常の編集なら fmt 差分はそのコミットの一部になるだけで、
  整形専用コミットは不要。
- **pre-commit フックが番人**: `.git/hooks/pre-commit` が `cargo fmt --check` を回し、
  未整形コードが混じったコミットを機械的に弾く。Claude Code / Codex / 手作業の
  どの経路でも効く。フックが無い環境 (新規 clone 等) では以下で再作成する:
  ```sh
  cat > .git/hooks/pre-commit <<'HOOK'
  #!/bin/sh
  # Keep the repo 100% `cargo fmt` clean. Policy: CLAUDE.md "## Formatting".
  if ! cargo fmt --check >/dev/null 2>&1; then
      echo "[pre-commit] Rust code is not formatted. Run 'cargo fmt', then re-stage." >&2
      cargo fmt --check 2>/dev/null | grep "^Diff in" | sed 's/ at line.*//' | sort -u >&2
      exit 1
  fi
  HOOK
  chmod +x .git/hooks/pre-commit
  ```
  (git worktree は共通 git dir の hooks を共有するので、worktree 側でも自動で効く。)
- **rustfmt バージョン更新時のみ整形専用コミット**: ツールチェーンを上げると新 rustfmt
  が既存コードを整形し直すことがある。その場合に限り、機能変更と混ぜず
  `style: Apply cargo fmt` の単独コミットで吸収する。
- Claude Code / Codex / 手作業のいずれでも同じ方針を使い、リポジトリを常に
  整形済みに保つ。

## Git Workflow

- **セッション開始時に `git status` を一度確認する**。CLAUDE.md 冒頭の `gitStatus`
  ヘッダは conversation 開始時点の snapshot だが、初手で自分でも `git status` を見て
  「working tree が本当に clean か / 前セッションからの未コミット差分が残っていないか」を
  確かめる。未コミット差分があれば user に「現在以下の未コミット変更があります。今回の
  作業に取り込みますか? それとも別途扱いますか?」と **必ず確認する** (前セッションの
  leftover を黙って自分のコミットに混ぜない)。
- **コミットは許可を取らずに適宜行ってよい**。作業が一区切りついたら (テストが緑で、
  レビューが済んだら) その都度コミットする。毎回 user に確認しない。
  - **区切りごとに小さくコミットする**。検証済みの差分と、これから書く差分を同じ作業
    ツリーに積み上げない。特に **subagent (Codex 等) に実装を出す前にコミットしておく**。
    出した後だと同じファイルに未検証の変更が混ざり、後から切り分けられなくなる
    (2026-08 に実際に発生。検証済み 4 系統と未検証 1 系統が分離不能になった)。
  - 実機確認が要るネイティブ機能は従来どおり user の確認後にコミットする
    (「実機検証用バイナリの準備」節)。確認前でも、テストが緑ならブランチ上に
    コミットしておいてよい。コミットメッセージに未確認である旨を書く。
  - push / PR 作成は明示的に指示された場合のみ。
- **ローカル `master` への統合**。「コミットして」と言われた場合は、
  対象差分をコミットしたうえで、同じ作業内でローカル `master` ブランチに merge する。
  ただし user が明示的に feature/worktree ブランチへ残すよう指示した場合、または conflict /
  unrelated dirty worktree により安全に merge できない場合は止めて状況を報告する。push や
  PR（プルリクエスト）の作成は、明示的に指示された場合のみ行う。
- **デフォルトブランチ**: GitHub 上は `main`、ローカルは `master`。リリース時は両方に push する。

### 複数の Claude Code セッションを並行で動かす (lab と本体の同時編集) ⚠️

ラボ (`crates/comic-core` / `tools/comic_lab`) と本体 (`src/` ほか) は編集ファイルが
重ならないので**並行作業自体は可能**だが、**2 つのセッションが同じ作業ツリー
(= 同じ `.git` の index と HEAD) を共有すると git 操作が衝突する**。2026-06 に実害:
一方の `git add -A` がもう一方のステージ変更を巻き込んで誤ったコミットに混入し、さらに
一方の `git reset` が他方の commit を HEAD から落とした (2 回発生)。ファイル編集自体は
衝突していなくても、index・HEAD の取り合いで起きる。

**運用実態 (2026-07 追記)**: 実際には ClaudeCode / Codex の複数セッションで **互いに独立した
領域** を並行開発するのが常態化している。大きな変更や編集ファイルが重なる作業では worktree を
分けるが、**多くは独立した部分の修正なので、確認作業のしやすさを優先して単一の master 作業
ツリー上で並行**させている。worktree は「やむを得ず」ではなく状況次第で選ぶ選択肢であり、
master 共有で進めるときは下記「1 つの作業ツリーを共有する場合の規律」を **通常運用として** 守る。

**推奨 = 編集が重なる / 大規模なときは `git worktree` を分ける**。index・HEAD が独立し、
取り合いが原理的に起きなくなる:

- ラボ用 worktree は **vendor/ 不要**。`cargo {check,test} -p comic_lab -p comic-core`
  は本体パッケージ (`mimageviewer`) をビルドしないので **root の build.rs が走らず**、
  pdfium/ffmpeg/ort/susie の DLL チェックに引っかからない。junction も張らないので下記
  「worktree + Windows junction の地雷」のリスクも無い:
  ```bash
  git worktree add ../mimageviewer-lab -b lab    # vendor セットアップ不要
  cd ../mimageviewer-lab
  cargo test -p comic-core && cargo check -p comic_lab
  ```
  ⚠ ラボ worktree では **bare `cargo build` を打たない** (全 member をビルドして本体
  build.rs が走り vendor チェックで失敗する)。必ず `-p comic_lab` / `-p comic-core` を付ける。
- 各 worktree は**自分のブランチ**にコミットし、作業完了時に**一方ずつ** master へ merge する。
- 撤収は必ず `scripts/safe-worktree-remove.ps1` 経由 (下記「worktree + Windows junction」)。

**やむを得ず 1 つの作業ツリーを共有する場合の規律** (worktree を分けないとき):

- **コミットは必ず pathspec commit `git commit -- <自分のパス>` を使う**。bare `git commit`
  は**共有 index 全体**をコミットするため、`git add` した直後でも、相手が並行で stage した
  ファイルを巻き込む (2026-06 実害: 相手の `src/*` が自分のコミットに混入)。`git commit --
  <paths>` は指定パスだけを working tree からコミットし、index の他の内容を無視するので
  race-proof。**`-F msgfile` / `-m` 等のオプションは `--` の前に置く** (後ろに置くと pathspec
  扱いされてエラー)。例: `git commit -F /tmp/msg.txt -- crates/comic-core tools/comic_lab`。
- **`git add -A` / `git add .` 禁止**。明示 add でも上記のとおり bare commit だと相手の stage
  を拾うので、最終的な防御は pathspec commit。
- **HEAD が自分のコミットでないときに `git reset` / `rebase` / 履歴書き換えをしない**。
  実行前に必ず `git log --oneline -5` で「最新が本当に自分の commit か」を確認する。相手の
  commit を巻き戻すと相手の作業が HEAD から消える。
- **コミット直後に `git log --oneline -1` で自分の commit が HEAD にあるか確認**する。消えて
  いたら相手の操作に巻き込まれている → `git reflog` か退避ブランチから復旧する。
- **大きめの作業は退避ブランチを作っておく**: `git branch -f <name> HEAD`。相手の reset で
  master が巻き戻っても `git reset --hard <name>` で復旧できる (2026-06 はこれで救済した)。
- **`git diff` / `git status` に出る変更を、自分 (や自分の subagent = Codex) の変更と決めつけない**。
  共有ツリーでは **別セッションの concurrent 編集が混ざって見える**。想定外の変更を「subagent が
  スコープ逸脱した」と誤認して `git checkout HEAD -- <path>` / `git restore` で消すと、**別セッションの
  未コミット作業を破壊する (未コミットなので git で復元不能)**。2026-07 実害: 別セッションの MS Store
  作業 (index.html の 35 行) を「Codex の逸脱」と誤認して `git checkout HEAD` で破棄した (Codex の
  「触っていない」報告は実は正確だった)。**対処**: 想定外ファイルは **working tree から消さず、
  自分の pathspec commit から外すだけ** にする。subagent の変更範囲は subagent 自身の report と
  突き合わせる (diff に出た ≠ 自分の subagent の変更)。判断が付かなければユーザーに「これは別
  セッションの作業か」を確認してから触る。

### worktree + Windows junction の地雷 ⚠️

並行作業のため `git worktree add` で別 worktree を切るときに、`vendor/` を main worktree から
**junction (NTFS reparse point) で共有する設計**を選ぶと、撤収時に災害が起きる:

```
[NG] git worktree remove で junction ごと再帰削除される事故 (2026-05-13 / 2026-05-14 実害):
  C:\home\mimageviewer-sqlite\          ← worktree
    └─ vendor\  (junction → C:\home\mimageviewer\vendor\)
  ↓ git worktree remove C:\home\mimageviewer-sqlite
  → git は junction を **通常ディレクトリ扱いで** 中身まで再帰削除
  → main worktree の C:\home\mimageviewer\vendor\ の中身が全消失
  → cargo build が pdfium.dll / FFmpeg DLL / ONNX Runtime DLL / Susie ワーカーすべて
    無いと言って失敗、bootstrap-vendor.sh で再 DL する羽目になる
```

#### 鉄則: `git worktree remove` を直接呼ばない

**この repo では `git worktree remove` を直接実行しない。必ず
`scripts/safe-worktree-remove.ps1` を経由する**。文書化だけでは 2026-05-13 → 翌日
2026-05-14 と連続で事故ったので、機械的に防ぐ仕組みを置いた。

ラッパーが実行する手順:
1. 対象 worktree を再帰スキャン (junction には**降りない**) して reparse point を列挙
2. 見つけた junction を `cmd /c rmdir` で **リンクだけ** 削除 (リンク先は無事)
3. 全部 unlink できてから `git worktree remove <path>` を実行

```powershell
# 全 worktree の junction 状況を一覧 (非破壊)
.\scripts\safe-worktree-remove.ps1 -Audit

# 何が起きるかを確認するだけ (非破壊)
.\scripts\safe-worktree-remove.ps1 <worktree-path> -DryRun

# 実際に安全に削除
.\scripts\safe-worktree-remove.ps1 <worktree-path>
.\scripts\safe-worktree-remove.ps1 <worktree-path> -Force   # dirty worktree 用
```

ラッパーは main worktree (= cwd repo) を対象に指定すると refuse する。

**Claude セッション中の運用**: worktree を撤収する指示を受けたら、`git worktree remove ...`
の前に必ず `.\scripts\safe-worktree-remove.ps1 ...` に置換する。`gh`/`git` の他コマンドと
混ぜて連続実行する場合も同じ。これは取り返しがつかない事故なので例外なし。

#### 設計面の鉄則

`vendor/`, `target/`, runtime DLL / model / SDK などは worktree 間で **junction / symlink /
reparse point 共有しない**。個別サブディレクトリ単位でも禁止。過去に worktree 撤収時の
再帰削除で main 側の実体を消した事故が複数回あるため、リンク共有は選択肢に入れない。

新規 worktree で依存ファイルが必要な場合は、以下のどちらかにする:

- `bash scripts/bootstrap-vendor.sh` を worktree 側で流して per-worktree な `vendor/` を作る。
- 既存 worktree / backup から必要なサブディレクトリを `Copy-Item -Recurse` などで**実体コピー**
  する。例: `Copy-Item -Recurse C:\home\mimageviewer\vendor\ffmpeg vendor\ffmpeg`

削除・撤収時は、対象が意図した worktree 配下であることと junction / symlink / reparse point で
ないことを確認してから消す。worktree 自体の撤収は引き続き
`scripts/safe-worktree-remove.ps1` 経由にする。

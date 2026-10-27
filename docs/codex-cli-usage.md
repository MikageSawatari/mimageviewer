# Codex CLI の使い方 (レビュー・実装委任)

> この文書は 2026-10-15 に CLAUDE.md から移した節をそのまま収めたもの。本文中に残る「本書」は移動前の CLAUDE.md を指す。移した節の行き先一覧は CLAUDE.md の「移した節の行き先」表を参照。

## Codex CLI レビュー

担当・実施粒度は [AGENTS.md](../AGENTS.md#model-roles-and-coordination) に従う。
独立レビュー担当を使える場合はその担当を再利用し、同じ差分のCLIレビューを重ねない。
ClaudeCode主導の開発またはCLIレビューが必要な場合は以下を使い、新規reviewは「モデル指定について」の
GPT-6.1 Sol / xhighを明示する。重要指摘の修正・再確認は維持し、小さな中間変更ごとの承認は求めない。
以下の自発的レビュー規則は開発変更が対象。ClaudeCode Opusによる定型の公開準備・公開作業
(版番号・更新履歴・公開メタデータの更新を含む) には適用しない。
利用者が明示的にレビューを依頼した場合、または製品コード等の開発変更が生じた場合は実施する。

ユーザーから「Codex にレビューしてもらって」「Codex レビューを取って」等と指示された場合は、
**ユーザーに手作業で中継してもらわず、`codex` CLI を直接叩いて結果を取り込む**。

### 自発的に Codex を呼ぶタイミング (明示指示がなくても)

ユーザーから個別の指示がなくても、以下のタイミングでは **自分から `codex exec` を起動して
意見を取り込む**こと。Claude 単体で先に進むより、別モデルの目を入れた方が早く正解に
辿り着くため。

重要な所有境界・非同期・複数ウィンドウの設計は、以下の完成時レビューに加えて
**実装前に**独立したGPT-6.1 Sol / xhighへ確認を依頼する。前提のコード上の根拠、不変条件、
影響する同型経路、受入条件を渡し、設計の誤り・見落としも指摘対象にする。
以下の第二意見はまずSolへ依頼し、それでも解けない論点や難しい構造設計は
AGENTS.mdの条件に従いAstra / mediumへ絞って依頼する。

1. **作業の一塊が終わった時点でレビューを取る**
   - まとまった機能追加・バグ修正・リファクタが「ローカルで動くようになった」段階で
     コミットを作り、そのコミットを基準に `codex exec --sandbox read-only -o <FILE>`
     でレビューを依頼する。
   - 一塊の目安: 単一の機能/修正で論理的にまとまっている、または PR にする粒度
     (1〜数コミット)。「small fix を 1 個入れた」程度では呼ばなくて良い。
   - 出てきた P1/P2 はその場で対応 (または false positive 判定の理由を添えて
     ユーザーに報告)、P3 はメモして後でまとめて。
   - 「これからユーザーに完了報告する直前」が呼びどころ。完了報告前に Codex の
     チェックを通しておくと、ユーザーが追加レビュー指示を出さずに済む。

2. **調査・デバッグで仮説と検証を 2-3 周してもまだ詰まっているとき**
   - 「黒画面の原因はこれだ」と思って直したのに直らない、ログを追加したが何も
     見えない、修正が別の箇所を壊す — のような **試行錯誤が繰り返し発生する局面**
     では、Codex に **現状の症状 + これまで試したこと + 関連ファイル** を渡して
     第二意見を取る。プロンプト例:
     ```
     codex exec --sandbox read-only -o /tmp/codex-2nd.txt \
       "Symptom: <症状>. Tried so far: <試したこと>. Suspected files: <file:line>.
        Look at the code with fresh eyes, propose 2-3 alternative root causes I might
        have missed. Return findings ordered by likelihood." < /dev/null
     ```
   - Claude が自分の仮説に固執して同じパターンの修正を繰り返す失敗を防ぐ。
   - 1 周目の修正で直らなかったらすぐ呼ぶ必要はないが、**3 周目に入る前に必ず**
     一度第二意見を取る (作業の手戻りコストの方が Codex 1 回分より高い)。

3. **設計判断で複数案あるが選び切れないとき**
   - 「VSR ON 時の D3D11 → wgpu 共有経路を Keyed Mutex で再有効化する案 vs 同期
     Flush で押し切る案」のような構造的な選択肢が複数ある局面では、Codex に
     trade-off を意見させる。Claude の最初の選好に縛られない設計判断ができる。

### 基本コマンド (1 ラウンド目)

read-only サンドボックス (ファイル書き換え・ネットワーク・パッケージ操作は禁止) で実行する。
以下は **同じタスクで Codex に意見をもらう最初の 1 回** で使うコマンド。2 ラウンド目以降は
次節「同じタスクは同じセッションで継続する」を使う (毎回 `codex exec` で新規セッションを
始めない)。

| 目的 | コマンド |
|---|---|
| 直前のコミット (`HEAD~1` との差分) をレビュー | `codex exec --sandbox read-only "Review the changes since HEAD~1. Use git diff HEAD~1 and inspect relevant files. Focus on bugs, regressions, missing tests, and compatibility risks. Return findings first, ordered by severity."` |
| ブランチ全体 (`main` からの分岐) をレビュー | `codex exec --sandbox read-only "Review this branch against main. Use git merge-base main HEAD and git diff from that base. Focus on bugs, regressions, missing tests, and compatibility risks. Return findings first."` |
| 未コミットの作業変更だけレビュー | `codex exec --sandbox read-only "Review the uncommitted changes in this repo. Use git diff and git diff --cached. Focus on bugs, regressions, missing tests, and compatibility risks. Return findings first."` |

### 同じタスクは同じセッションで継続する (resume)

**同じタスク内でレビュー往復をするときは、毎回 `codex exec` で新セッションを開かず、
`codex exec resume` で前回セッションの会話履歴を引き継ぐこと**。新セッションを開き直すと、
Codex は前回どこに着目したか・どの指摘を出したかを覚えておらず、毎回ゼロから差分を読み
直すので精度が落ち、こちらの追問 ("P2 について詳しく" / "この修正で直ったか確認して") も
通じなくなる。

判断基準 (タスク単位):

- **同じタスク** = ユーザーから受け取った 1 つの作業指示の中で、レビュー → 修正 → 再レビュー
  と往復する局面。**同じセッションを使い続ける**。
- **別タスク** = 新しいユーザー指示で作業を始める / コミット粒度が変わる / レビュー観点が
  変わる (例: bug review → security review)。**新セッションを開く** (`codex exec` で開始)。

#### 推奨フロー

1 ラウンド目だけ `codex exec` で開始し、以降は `--last` で直近セッションを継続する。
`--last` は cwd 一致の最新セッションを自動で拾うので SID 管理が不要:

```bash
# 1 ラウンド目 (新セッション開始)
codex exec --sandbox read-only -o /tmp/codex-1.txt \
    "Review the changes since HEAD~1. ..." \
    < /dev/null > /tmp/codex-1-events.log 2>&1
cat /tmp/codex-1.txt   # P1/P2/P3 サマリ

# 2 ラウンド目以降 (同じセッションを継続)
# 注意 1: `codex exec resume` には `--sandbox` フラグが**無い** (codex 0.124 で確認、
#         元セッションの sandbox 設定が引き継がれる)。指定すると `Usage: ...` でエラーになる。
# 注意 2: prompt に `backtick` / $var を含めるケースが多いので、stdin 経由 (`-`) で渡す。
#         heredoc を temp ファイルに書いてから流し込むのが一番安全 (詳細: 後述 §stdin の取り扱い)。
cat > /tmp/codex-prompt.txt <<'PROMPT'
P2 の修正を <commit-hash> で入れた。意図通り直っているか確認して。
PROMPT
codex exec resume --last -o /tmp/codex-2.txt - < /tmp/codex-prompt.txt > /tmp/codex-2-events.log 2>&1
cat /tmp/codex-2.txt
```

#### 明示的に SID を指定したいとき

並行で複数タスクの Codex セッションが走っていて `--last` が別タスクのものを拾う恐れが
あるときは、1 ラウンド目で `--json` を付けて **thread_id** を控え、resume 時に SID を
明示する:

```bash
# 1 ラウンド目 (SID 取得)
codex exec --json --sandbox read-only -o /tmp/codex-1.txt \
    "Review the changes since HEAD~1. ..." \
    < /dev/null > /tmp/codex-1-events.jsonl 2>&1
SID=$(head -1 /tmp/codex-1-events.jsonl | jq -r .thread_id)

# 2 ラウンド目以降 (SID 指定で resume、--sandbox なし、prompt は stdin 経由)
cat > /tmp/codex-prompt.txt <<'PROMPT'
P2 について詳しく説明して
PROMPT
codex exec resume "$SID" -o /tmp/codex-2.txt - < /tmp/codex-prompt.txt > /tmp/codex-2-events.log 2>&1
```

#### 制約・注意点

- **`--ephemeral` を付けない**。これを付けるとセッションがディスクに残らず resume 不可。
  デフォルトは永続化なので普段は何もしなくて OK。
- **resume には `-C / --cd` がない**。1 ラウンド目とは別 cwd で実行できないので、
  レビュー往復は必ず同じディレクトリ (= mimageviewer ルート) から打つ。
- **タスクが切り替わったら新セッションを開く** (= `codex exec` から始め直す)。古いセッション
  の文脈が混ざると逆に精度が落ちる。
- **basis コミット (差分の起点) を resume 中に動かさない**。会話履歴は残るが、Codex が
  参照したファイルの中身は記録されていないので、`HEAD~1` 等で渡した基準コミットが
  rebase で動くと差分の意味が変わる。

#### 実装の委任でも同じ段階は同じセッションで続ける

上の規則はレビューだけでなく、**実装担当 (Codex Sol) への委任にも適用する**。

- **同じ機能の同じ段階** (例: X 投稿予約の R2a-2) の実装・修正・残作業は、最初の実装セッションを
  `codex exec resume <SID>` で続ける。実装担当が作った規約・コードの理解が残るので、読み直しが要らない。
- 新しいセッションを開くのは、**段階が変わるとき** (例: R2a → R2b)、**担当・モデルが替わるとき**、
  利用者が明示したときだけ。レビュー担当は実装担当とは別のセッションにする (独立性のため)。
- **長さを理由に新しいセッションへ切り替えない**。入力の大半はキャッシュ読み込みで、Codex は自動で
  コンパクトする。`turn.completed` の `input_tokens` だけを見て判断せず、`cached_input_tokens` と並べて見る。
- 実装 brief に「緑の区切りで止めて報告してよい」と書かない。止めてよいのは、設計判断が必要なとき・
  前提の矛盾を見つけたときだけにする。区切りの許可があると、作業が細切れになり往復が増える。
- 経緯 (2026-09-24): X 投稿予約 R2a-2 で、1 回の resume の入力 9,300 万トークンを見て新しいセッションへ
  切り替えたが、うち 98.7% はキャッシュだった。新しいセッションは 1,200 万トークンを使って小さな 1 件を
  片付けただけだった。作業の細切れは、セッションの長さではなく brief の「区切りで止めてよい」が原因だった。

### 差分範囲の決め方

基準となるコミットは「**前回 Codex にレビューしてもらった地点より後**」にする。具体的には:

- セッション内で前回 `codex exec` を走らせたなら、そのときレビューした最新コミットハッシュを覚えておき、
  `codex exec --sandbox read-only "Review the changes since <HASH>. Use git diff <HASH>..."` を使う。
- セッション内で Codex に出したのが初めてなら、**今セッションで Claude が作った最初のコミットの親** (= 作業を始める前の HEAD) を基準にすると、今回の作業分をまとめて見てもらえる。
- 分からない場合はユーザーに「基準コミットはどこにしますか?」と聞く。推測で `HEAD~1` を決め打ちしない
  (直近 1 コミット以外にもレビュー対象があるケースが多い)。

### 結果の扱い

Codex の出力は `[P1]` / `[P2]` / `[P3]` のような severity 付きで返る。指摘 1 件ごとに:

1. 内容を読み、該当コード (`file:line`) を Read で確認する
2. 真の bug か false positive か判断し、ユーザーに要旨を報告する
3. 修正を入れる場合は同じコミットメッセージに `Codex P<N> 対応` を書いておく (レビュー履歴の紐付け用)
4. false positive と判断した場合は、**理由を添えて** ユーザーに伝える (ユーザーが最終判断できるように)

### 出力の取りこぼしに注意 (`tail -N` を使わない)

`codex exec ... 2>&1 | tail -80` のような **末尾固定行数の切り取りはレビュー指摘を失う**。
Codex は探索ステップ (grep / ファイル参照等) を流してから最後に `P1/P2/P3` サマリを
出すので、探索ログが長いと重要な指摘が tail の外に流れ出す可能性がある。

**推奨: `-o` で最終メッセージだけ抽出する** (codex 0.124 以降):

```bash
codex exec --sandbox read-only -o /tmp/codex-final.txt "…レビュー依頼…" \
    < /dev/null > /tmp/codex-events.log 2>&1
cat /tmp/codex-final.txt   # ここに P1/P2/P3 サマリだけが入る
```

`codex exec --output-last-message <FILE>` (短縮形 `-o <FILE>`) で**最終回答 (= P1/P2/P3
サマリ) だけ**をファイルに直接書き出せる。awk で抽出するより確実。

### stdin の取り扱い (必須)

`codex exec` / `codex exec resume` は **stdin を常に明示的に制御する**こと。Claude Code
の Bash tool 経由で起動すると stdin が必ず何かに繋がっているので、放置すると codex は
それを `<stdin>` ブロックとして読みに行き、EOF まで `Reading additional input from
stdin...` で固まる (5 分以上ハングして手で kill する羽目)。2 つのパターンがある:

**(A) positional 引数で prompt を渡すとき → `< /dev/null` で stdin を閉じる**

```bash
codex exec --sandbox read-only -o /tmp/codex-out.txt "短い prompt" < /dev/null > /tmp/log 2>&1
```

短く `$` / backtick / 改行を含まない prompt 向け。

**(B) `-` (stdin から prompt を読む) を指定するとき → `<` で prompt ソースを渡す**

```bash
# heredoc を temp ファイルに書いてから流し込む (一番安全、長文 OK)
cat > /tmp/codex-prompt.txt <<'PROMPT'
複数行の prompt。`backtick` や $var もそのままリテラル扱いされる。
PROMPT
codex exec resume --last -o /tmp/codex-out.txt - < /tmp/codex-prompt.txt > /tmp/log 2>&1

# process substitution でもよい (短文向け)
codex exec resume --last -o /tmp/codex-out.txt - \
    < <(echo "短い prompt") > /tmp/log 2>&1
```

**(B) を使う理由**: positional 引数は bash の通常評価を通るので、prompt に backtick が
あると **command substitution として実行されて構文エラーや意図しないコマンド実行**が
起きる (`Usage: codex exec ...` で死ぬ実例あり)。`'...'` で single-quote しても、prompt
内に `'` を含めるたびにエスケープ手間が増えるので、長文 / 特殊文字含みは (B) が確実。

### awk で結論抽出する旧フォーマット (`-o` が使えない古い codex 用)

```bash
codex exec --sandbox read-only "…" < /dev/null > /tmp/codex-out.txt 2>&1
# 最終結論 (最後の "codex" 行以降) を抽出
awk '/^codex$/{found=1; next} found' /tmp/codex-out.txt
```

短いタスクなら `tail -200` 程度にとること (目安: Codex 探索が 5 分超なら十分)。

### 起動できないとき

`codex` コマンドが PATH にない / Codex CLI が入っていない場合は、ユーザーに環境確認を依頼する。
勝手に従来の「ユーザーに Codex 実行を依頼してレビュー結果をペーストしてもらう」フローに戻さない
(ユーザーの手間が増えるだけなので明示的に確認する)。

### モデル指定について

新規の独立レビューは個人configの既定値に依存させず、次を明示する。
この節の簡略化したCLI例にも、実行時には同じmodel/effortを付ける。

```bash
codex exec --model gpt-6.1-sol -c 'model_reasoning_effort="xhigh"' --sandbox read-only -o <FILE> "<review prompt>"
```

実装・テスト担当は `gpt-6.1-sol` / `high` を明示し、上のread-onlyレビューとは別の担当・会話にする
(`codex exec --model gpt-6.1-sol -c 'model_reasoning_effort="high"' --sandbox workspace-write ...`)。
`gpt-6.1-sol` には Codex CLI 0.159.0 以上が要る。
Astraの追加確認が必要な場合だけ、上のmodelを `gpt-6-astra`、effortを `medium` に変え、
read-onlyのまま未解決の論点を渡す。個人configの既定値には依存させない。

同じレビューの継続は既存のresume手順を使い、旧Astra担当・GPT-5.6 Sol担当・GPT-6 Sol担当等からの移行時はmodel/effortが
実際に切り替わったことを確認する。切り替えられなければ、既知指摘・判断根拠・対象差分と
検証記録を指定モデルの新しい担当へ渡す。解決済み事項の全件再レビューは不要。
利用可否やoverrideの失敗はCLIの実出力で確認し、未確認の設定を適用済みと報告しない。
個人の `~/.codex/config.toml` は、このリポジトリの既定分担を理由に書き換えない。

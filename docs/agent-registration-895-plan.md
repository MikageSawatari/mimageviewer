# §1.895 AI 連携の登録先を直す (Claude Desktop のパッケージ版・バックアップ・注記・ChatGPT の設定)

状態: 設計中 (2026-11-25)。担当: 設計・検収 ClaudeCode Opus 5.5、実装 Codex GPT-6.1 Sol / high。
ブランチ `agent-reg-895` (master `a57f23522` から)。正本の登録の設計は [mcp-integration-plan.md](mcp-integration-plan.md) の
「AI アプリごとの行」の節で、本書はその変更点だけを書く。実装後に正本の該当の行を本書に合わせて直す。

## 1. 観測と、確かめた事実

観測の出どころを各行に書く。エージェントは製品を起動していない。

| 事実 | 出どころ |
| --- | --- |
| mIV 5.7.0 の「登録する」は `%LOCALAPPDATA%\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Roaming\Claude\claude_desktop_config.json` に書いた | サブ PC 担当 (2026-11-25、サブ PC2、Claude Desktop のパッケージ Claude_6.28899.6.0) |
| その後に起動した Claude Desktop は `%LOCALAPPDATA%\Claude-Data\claude_pzs8sxrjxfjjc\claude_desktop_config.json` に書き、ローカル MCP サーバーの一覧は空だった。`%APPDATA%\Claude` は無い | 同上 |
| 2026-11-01 のサブ PC1 (パッケージ版、版は記録なし) では LocalCache 側に書いて mIV の tool が使えた | [mcp-integration-plan.md §24.1](mcp-integration-plan.md) (サブ PC 担当) |
| パッケージではない Claude Desktop は `%APPDATA%\Claude` を使う (`Packages\Claude_*` も `Claude-Data` も無い) | 統括がメイン PC で読み取りで確認 (2026-11-25) |
| Anthropic の公式ヘルプは設定ファイルの場所を書いていない。modelcontextprotocol.io は `%APPDATA%\Claude` だけを書く | 設計担当が 2026-11-25 に公式ページを読んだ |
| パッケージ版が LocalCache 側を読むという利用者の報告 (版 1.1.x、2026-02〜09) | GitHub anthropics/claude-code#26073 (未解決)・#25579 (bot が閉じた)。Anthropic の回答なし |
| ChatGPT のデスクトップアプリの `codex://` の公式一覧に MCP の一覧を開くリンクは無い。`codex://settings` は設定の最初のページを開く | 設計担当が 2026-11-25 に learn.chatgpt.com の commands / settings を読んだ |
| `codex://settings` はアカウントのページを開き、MCP の一覧ではなかった | サブ PC 担当 (2026-11-25) |

**サブ PC での確認を依頼中** (統括経由、2026-11-25)。確認する内容: Claude の「設定を編集」が開くファイル、
Claude-Data 側に書いたときに一覧と tool に出るか、LocalCache 側を Claude が触るか、サブ PC1 の版、ChatGPT の MCP の一覧のメニュー名。
§3.1 の規則は、この確認の結果で確定する (§7)。

## 2. 直すこと

1. Claude Desktop のパッケージ版で、今の版が読む設定ファイルに書く (§3.1)。
2. 「登録済み」の判定・確認の窓のパス・「手順を見る」のパスと案内を、同じ 1 つの判定から出す (§3.2)。
3. 「登録を外す」の後に、登録の後と同じ注記 (「起動して tool が出るか確かめて」) が出る (§3.3)。
4. バックアップが 1 つで、登録前の元が次の書き換えで消える (§3.4)。これはまれな失敗ではなく、
   登録 → 外す の通常の操作で起きる。
5. ChatGPT (Codex) の「設定を開く」がアカウントのページを開き、MCP の一覧へ行けない (§3.5)。

扱うが変えないこと:

- **既定のデータの場所でも `--data-dir` を付ける**のは、正本の設計の決定 (「状態を減らすため『既定の data dir なら省く』を
  作らない」、設計レビュー 12 巡目の回答 3) のとおりで、誤りではない。`mimageviewer-mcp.exe` の既定は `%APPDATA%\mimageviewer`
  固定で、ポータブル版の既定 (`<exe>\data`) を知らない。省く規則を足すと、既定かどうかで「登録済み」の比較が 2 通りになる。
  変えない。

## 3. 設計

### 3.1 Claude Desktop の登録先の決め方 (サブ PC の確認待ち)

検出は今と同じく worker で行う。パッケージ版の 1 つの package (`Packages\Claude_<id>`) について、次の順で 1 つの設定ファイルを決める。

| 在るもの | 登録先 | 根拠 |
| --- | --- | --- |
| `%LOCALAPPDATA%\Claude-Data\<package のフォルダ名>\` | その下の `claude_desktop_config.json` | 今の版がこのフォルダを作って使う (サブ PC2) |
| 上が無く、`Packages\Claude_<id>\LocalCache\Roaming\Claude\` | その下の `claude_desktop_config.json` (今と同じ) | 以前の版 (サブ PC1、#26073) |
| どちらも無い | 書かない。「Claude Desktop を一度起動して終了してから「再確認」を押してください」 | どの版が入っているかを決められない |

- フォルダ名の大小文字は Windows では区別しない。表示するパスは、実際に在るフォルダの名前を使う (`read_dir` で得たもの)。
- 両方が在るときは Claude-Data 側を取る。以前の版から今の版へ更新した利用者は、以前の版が作った LocalCache 側も残っている
  ので両方が在る。今の版から以前の版へ戻すことは考えない (まれ。戻したら手順を見て書く)。
- **どちらも無いときに書かない**のは、未起動の今の版に LocalCache 側へ書くと「登録済み」と出て実際は使えない、という今回の
  症状を新しく作らないため。どちらかの版を 1 回起動すれば決まる。
- Squirrel 版 (`%LOCALAPPDATA%\AnthropicClaude`) → `%APPDATA%\Claude\claude_desktop_config.json` (今と同じ)。
- Squirrel 版と パッケージ版が両方在る、`Claude_*` が複数在る → 今と同じく「自動登録できません」。
- 以前の版の LocalCache 側に mIV が書いた項目は、今の版へ更新した後は読まれないので放っておく (消しに行かない。
  消すために 2 つのファイルを書く組み合わせを作らない)。「手順を見る」の注記で触れる (§3.2)。
- `%LOCALAPPDATA%\Claude-3p` (#26073 の要約にだけ出た名前) は扱わない。

判定は今の `detect` の Claude Desktop の分岐だけを変え、`AppRegistration` の形 (状態・`config_paths`・`message`・`changes`) は
変えない。§1.894 (ask-ai-894) の「登録済み」の判定はこの形を読むだけなので、変更は要らない (担当へは知らせる)。

### 3.2 「手順を見る」の案内

- パスは今と同じく `config_paths` (= §3.1 で決めた 1 つ) を出す。自動登録できない場合 (どちらも無い・複数) は候補を並べる。
- Claude Desktop の案内文を、版に依らない入口へ向ける: Claude の 設定 → 開発者 → 「設定を編集」で開くファイルが、Claude が
  読むファイル。mIV が出すパスと違う場合は Claude が開いた方に書く。(メニュー名はサブ PC の確認で確定する。)
- ChatGPT の案内文のメニュー名も、サブ PC の確認の結果に合わせる (今は "Settings > MCP servers > Add server > STDIO")。

### 3.3 登録・解除の後の注記

`write` の成功の文を操作ごとに分ける。

- 登録: 「<アプリ> を起動して、新しいチャットで mIV の tool が出るか確かめてください」(今と同じ)
- 解除: 「登録を外しました。<アプリ> を起動し直すと mIV の tool は出なくなります」

注記は今と同じく、次の登録・解除まで行の下に残る。再確認で消さない (今の挙動を変えない)。

### 3.4 バックアップ

今: 書き換えのたびに `<設定ファイル>.mimageviewer.bak` を上書きで 1 つ。登録 → 外す で、登録前の内容が消える。

新: 2 つにする。

- `<設定ファイル>.mimageviewer.bak` — 直前の書き換えの前の内容 (今と同じ。上書き)。
- `<設定ファイル>.mimageviewer.orig.bak` — **mIV が最初に書き換える前の内容**。一度作ったら mIV は上書きも削除もしない。

書き換えの手順 (今の (c) を置き換える。順序が大事):

1. 設定ファイルが在り、`.orig.bak` が無いなら、`.orig.bak` を作る。中身は、`.mimageviewer.bak` が在ればその中身
   (以前の版の mIV が残した、いちばん古い控え)、無ければ今の設定ファイルの中身。
2. `.mimageviewer.bak` に今の設定ファイルの中身を書く (上書き)。
3. 一時ファイルに書いて置き換える (今と同じ)。

- 設定ファイルが無かった (mIV が作った) 場合は、どちらのバックアップも作らない (今と同じ。元は「ファイルが無い」)。
- 1 で以前の版の `.mimageviewer.bak` を写すのは、5.7.0 以前で登録した利用者の手元では、それが登録前に最も近い内容だから。
  登録 → 外す を既に済ませた利用者の元は、以前の版の挙動で失われていて、戻せない。
- `.orig.bak` が在っても、その後に利用者が設定ファイルを大きく変えていることがある。`.orig.bak` は利用者が見比べるための
  控えで、mIV が戻しに使うものではない (戻す機能は作らない)。
- バックアップを作れない場合は、今と同じく書き換えずに止める (まれな失敗の割り切り: 表示 + ログ)。
- 2 つのバックアップの名前と意味をマニュアルに書く。アンインストールの後に残る点も今の記述を直す。

### 3.5 ChatGPT (Codex) の「設定を開く」

公式に MCP の一覧を開くリンクは無い。ボタンは残し (設定を開くこと自体は役に立つ)、押したときの行の下の注記と
「手順を見る」に、開いた後に MCP の一覧へ行くメニュー名を書く (メニュー名はサブ PC の確認で確定する)。
`codex://settings/...` の文書にない経路は使わない。

## 4. 状態の組み合わせを減らす検討 (CLAUDE.md の設計の簡素化)

- 登録先は常に 1 つのファイルに決め、2 つへ書かない (両方へ書くと、片方だけ成功・片方だけ登録済み、の組み合わせができる)。
- 決められないときは書かずに再確認へ誘う。版を推測して書く経路を作らない。
- 以前の版の項目の掃除をしない。
- バックアップは 2 つの固定の名前で、世代を数えない。`.orig.bak` は作ったら触らない。
- `--data-dir` の省略を作らない (§2)。
- まれな失敗 (バックアップを書けない等) は割り切り (表示 + ログ)。

## 5. 影響範囲

- `src/agent_registration.rs` (検出・`write` の注記とバックアップ)、`src/agent_registration/tests.rs`。
- `src/ui_dialogs/agent_integration.rs` (手順の文・「設定を開く」の注記)、そのテストとスナップショット (文言が変わる窓のもの)。
- マニュアル `htdocs/mimageviewer/manual/ai-integration.html` (登録の節・バックアップ・手順)、`tut-ai-integration.html` (Claude 側の手順に
  触れていれば)。製品ページ・privacy は、書き換えるファイルの種類が変わらないので確かめるだけ。
- 正本 [mcp-integration-plan.md](mcp-integration-plan.md) の「Claude Desktop」「登録・解除の流れ」「ChatGPT」の行。
- 永続データ: mIV の DB・設定の形は変えない。AI アプリの設定ファイルの横に `.orig.bak` が増えるだけ (移行は不要)。

## 6. テスト

- 検出: 一時フォルダに Packages・Claude-Data・LocalCache の組み合わせ (Claude-Data だけ / LocalCache だけ / 両方 / どちらも無い /
  大小文字の違う Claude-Data / Squirrel と併存 / 複数 package) を作り、登録先・状態・文言を確かめる。
- 書き込み: 登録 → 外す の後に `.orig.bak` が登録前と一致する。以前の版の `.mimageviewer.bak` だけが在る状態からの書き換えで、
  `.orig.bak` がその中身になる。設定ファイルが無い状態からの登録でバックアップを作らない。`.orig.bak` を 2 回目以降に変えない。
- 注記: 登録と解除で文が違う。
- 既存の `agent_registration::tests`・`ui_dialogs::agent_integration::tests`・AI 連携の窓の ui_snapshot。

## 7. 未決 (サブ PC の確認の結果で決める)

- §3.1 の表が正しいか (Claude-Data 側に書いて一覧と tool に出るか。今の版が LocalCache 側を作るか)。
- Claude の「設定を編集」のメニュー名と、開くファイル。
- ChatGPT の MCP の一覧のメニュー名。

## 8. 記録
</content>
</invoke>

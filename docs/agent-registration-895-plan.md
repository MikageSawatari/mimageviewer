# §1.895 AI 連携の登録先を直す (Claude Desktop のパッケージ版・バックアップ・注記・ChatGPT の設定)

状態: 実装済み (2026-11-25)。設計の独立レビュー r3 ACCEPT、実装の独立レビュー r2 ACCEPT、関連テスト成功。サブ PC の確認で §3.1 の規則を確認済み (§1・§7)。master 未取り込み、修正後の mIV での実機確認は未実施。担当: 設計・検収 ClaudeCode Opus 5.5、実装 Codex GPT-6.1 Sol / high。
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

**サブ PC での確認の結果** (サブ PC 担当、2026-11-25 21:58〜22:10、サブ PC2、Claude Desktop 6.28899.6 のパッケージ版。
記録は `C:\home\mimageviewer-subpc\agentreg895\results\notes.md`):

| 事実 | 出どころ |
| --- | --- |
| Claude が MCP サーバーを読むのは Claude-Data 側。mIV が LocalCache 側に書いた項目と同じものを Claude-Data 側に足すと、起動後すぐ「ローカル MCP サーバー」に「mimageviewer 実行中」と出て、新しいチャットで mIV の tool が使えた (main.log `Connected to mimageviewer (27 tools)`) | サブ PC 担当 |
| Claude は足した mcpServers を消さず、ファイルを書き直さなかった。LocalCache 側のファイルには一度も触れなかった (SHA-256 と更新時刻が同じ) | 同上 |
| 今の版でも LocalCache\Roaming\Claude のフォルダは在る (Claude が `.user-data-left.<package>` を置く。main.log に「Filesystem virtualization active — …LocalCache\Roaming\Claude exists」)。よって今の版では両方のフォルダが在り、Claude-Data 側を取る規則で正しい | 同上 |
| 設定の左に「開発者」は無い。MCP の一覧は 設定 →「デスクトップアプリ」→ 一番下の「拡張機能」の「管理」→「ローカル MCP サーバー」で、そこの「設定を編集」は Explorer で Claude-Data 側のファイルを開く | 同上 |
| 5 月のログではデータの置き場が `%APPDATA%\Claude` (パッケージ版では仮想化で LocalCache)。1.6608.1 (11-22) から 6.x への更新の間に Claude-Data へ移った見込み (推測) | 同上 |
| サブ PC (1 台目) の Claude Desktop は 7.37095.0.0 のパッケージ版で、Claude-Data が在る | 同上 |
| ChatGPT: `codex://settings` の最初のページは「アカウント」。MCP の一覧は 設定 →「連携」→「プラグイン」→ タブ「MCP」 | 同上 |
| `%LOCALAPPDATA%\Claude-3p` は在るが空 | 同上 |

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

### 3.1 Claude Desktop の登録先の決め方

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

**残る場合 (設計レビュー r1 P2-1)**: 以前の版で使っていて、ストアが今の版へ更新し、**今の版をまだ 1 回も起動していない**
間は、LocalCache 側だけが在り、Claude-Data 側が無い。この間に登録すると LocalCache 側へ書き、「登録済み」と出るが、今の版は
読まない (今の版が初回の起動で LocalCache 側の mcpServers を引き継ぐかは未確認)。フォルダからは以前の版と区別できない。

- 版の番号で分ける案は採らない。切り替わった版が分からず (1.1.x と 6.28899 の間)、境目を推測で決めることになる。
- 以前の版の利用者に「一度起動して」を求めて自動登録を止める案も採らない。リリース済みの、以前の版で 1 回で登録できる操作を
  不便にする。
- 代わりに、**次の再確認で正しい状態に戻る**ことを保証し、それを登録の後の注記で案内する。今の版を 1 回起動すれば Claude-Data
  側ができ、再確認は Claude-Data 側を見て「未登録」と出す。登録の後の注記 (§3.3) に「tool が出ない場合は、Claude を終了して
  mIV の『再確認』を押し、未登録と出たら登録し直してください」を足す。
- 登録の後の注記は書き込みの後にしか出ない。以前に登録済みで、その後に Claude が更新されただけの利用者には出ない (設計
  レビュー r2 P2)。そこで、**登録先に LocalCache 側を選んだ検出結果には、行の `message` に同じ案内を載せる**: 「Claude
  Desktop を更新した後で mIV の tool が出ない場合は、Claude を一度起動して終了してから『再確認』を押してください」。
  登録・解除の操作や窓の開き直しに依らず、LocalCache 側を見ている間はいつも出る。Claude-Data 側・Squirrel 版では出さない。
- この残る場合は、今の master の症状 (いつまでも「登録済み」と出て直らない) より狭く、再確認で直る。サブ PC の確認で、今の版が
  初回の起動で LocalCache 側を引き継ぐと分かれば、この場合は消える (§7)。

**登録先の解決は 1 つの関数にまとめ、書き込みの worker でも使う (設計レビュー r1 P2-2)**: 検出の後に Claude を起動・終了すると
Claude-Data 側ができ、確認の窓に出したパス (古い LocalCache 側) が登録先でなくなる。書き込みの worker は、動作中の確認の後・
ファイルを読む前に、同じ関数で登録先を解決し直す。確認の窓のパスと違う、または 1 つに決まらなくなったら、書かずに
「設定ファイルの場所が確認後に変わりました。もう一度確認してください」で止める (結果を受けたら今と同じく再確認が走る)。
ChatGPT (Codex) の登録先 (`CODEX_HOME` / `.codex`) も同じ関数の形で解決し直す。

判定は今の `detect` の Claude Desktop の分岐と、書き込みの前の再解決だけを変え、`AppRegistration` の形 (状態・`config_paths`・
`message`・`changes`) は変えない。§1.894 (ask-ai-894) の「登録済み」の判定はこの形を読むだけなので、変更は要らない (担当へは
知らせた)。

### 3.2 「手順を見る」の案内

- パスは今と同じく `config_paths` (= §3.1 で決めた 1 つ) を出す。自動登録できない場合 (どちらも無い・複数) は候補を並べる。
- Claude Desktop の案内文を、版に依らない入口へ向ける: Claude の 設定 →「デスクトップアプリ」→「拡張機能」の「管理」→
  「ローカル MCP サーバー」の「設定を編集」で開くファイルが、Claude が読むファイル (§1)。mIV が出すパスと違う場合は Claude が
  開いた方に書く。
- ChatGPT の案内文: 設定 →「連携」→「プラグイン」の「MCP」のタブでサーバーを追加し (STDIO)、再起動する (§1。今の
  "Settings > MCP servers > Add server > STDIO" を置き換える)。

### 3.3 登録・解除の後の注記

`write` の成功の文を操作ごとに分ける。

- 登録: 「<アプリ> を起動して、新しいチャットで mIV の tool が出るか確かめてください」(今と同じ)。Claude Desktop には
  「出ない場合は Claude を終了して『再確認』を押し、未登録と出たら登録し直してください」を続ける (§3.1 の残る場合)。
- 解除: 「登録を外しました。<アプリ> を起動し直すと mIV の tool は出なくなります」

注記は今と同じく、次の登録・解除まで行の下に残る。再確認で消さない (今の挙動を変えない)。

### 3.4 バックアップ

今: 書き換えのたびに `<設定ファイル>.mimageviewer.bak` を上書きで 1 つ。登録 → 外す で、登録前の内容が消える。

新: 2 つにする。

- `<設定ファイル>.mimageviewer.bak` — 直前の書き換えの前の内容 (上書き)。
- `<設定ファイル>.mimageviewer.orig.bak` — **このファイルについて mIV が残した最も古い控え**。一度作ったら mIV は上書きも
  削除もしない。

書き換えの手順 (今の (c) を置き換える。順序が大事):

1. `.orig.bak` が無いなら作る。中身は、`.mimageviewer.bak` が在ればその中身 (以前の版の mIV が残した控え)、無ければ今の
   設定ファイルの中身。
2. `.mimageviewer.bak` に今の設定ファイルの中身を書く (上書き)。
3. 一時ファイルに書いて置き換える (今と同じ)。

- **設定ファイルが無いときの中身は空** (設計レビュー r1 P2-3): 今の `read_config` は無いファイルを空の文字列として読む。
  1・2 はファイルの有無に関わらず同じ規則で書き、空の控えは「その時点でファイルは無かった」を表す。Claude の JSON・Codex の
  TOML とも、mIV が書いた設定ファイルが空になることはないので区別できる (元から 0 バイトだったファイルも空の控えになるが、どちらも「中身が無い」で戻し方は同じ)。これで「ファイルが無い状態から登録 → 外す」でも
  `.orig.bak` は空 (= 登録前はファイルが無かった) のまま残り、登録済みの内容が `.orig.bak` になることはない。
  今の `.bak` は「ファイルが在るときだけ」書くので、ここは変わる (無いときにも空の `.bak` を書く)。
- **`.orig.bak` が登録前の原本だと保証できるのは、この版以降の mIV で初めて書き換えた場合だけ**。5.7.0 以前の mIV で登録・
  解除したファイルでは、1 で写す `.mimageviewer.bak` は「以前の版が最後に残した控え」で、登録前の原本とは限らない
  (登録 → 外す を済ませていれば、登録済みの内容)。原本は以前の版の挙動で失われていて、戻せない。マニュアルにもそう書く。
- `.orig.bak` が在っても、その後に利用者が設定ファイルを大きく変えていることがある。`.orig.bak` は利用者が見比べるための
  控えで、mIV が戻しに使うものではない (戻す機能は作らない)。
- バックアップを作れない場合は、今と同じく書き換えずに止める (まれな失敗の割り切り: 表示 + ログ)。
- 2 つのバックアップの名前と意味をマニュアルに書く。アンインストールの後に残る点も今の記述を直す。

### 3.5 ChatGPT (Codex) の「設定を開く」

公式に MCP の一覧を開くリンクは無い。ボタンは残し (設定を開くこと自体は役に立つ)、開けたときは行の下の注記に
「設定が開いたら、左の「連携」→「プラグイン」→「MCP」のタブを選んでください」を出す。「手順を見る」にも同じメニュー名を書く (§3.2)。
`codex://settings/...` の文書にない経路は使わない。
§1.894 (ヘルプの「AI で調べる」) との統合で、開く処理は external-open の worker へ移った。注記は worker を始められた
時点で出し、開く途中の失敗は共通の警告で知らせる ([ask-ai-help-plan.md](ask-ai-help-plan.md) §7)。

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
- 更新の順序: LocalCache だけの状態で登録 (LocalCache へ書く) → Claude-Data を作る (今の版の初回起動に相当) → 再検出で
  登録先が Claude-Data 側・状態が「未登録」になる。
- 検出から書き込みまでに登録先が変わる: LocalCache だけの状態で検出 → 確認の要求を作る → Claude-Data を作る → 書き込みが
  書かずに「場所が確認後に変わりました」で止まり、どちらのファイルも変わらない。1 つに決まらなくなった場合 (package が増えた)
  も同じ。
- バックアップ: 設定ファイルあり → 登録 → 外す で `.orig.bak` が登録前と一致し、`.bak` が登録済みの内容。設定ファイルなし →
  登録 → 外す → 登録 で `.orig.bak` が空のまま。以前の版の `.mimageviewer.bak` だけが在る状態 (以前の版で登録 → 外す → 利用者が
  編集) からの書き換えで、`.orig.bak` がその `.bak` の中身になり、`.bak` は編集後の内容になる。`.orig.bak` を 2 回目以降に変えない。
- 注記: 登録と解除で文が違う。Claude Desktop の登録の文に再確認の案内が入る。
- LocalCache 側を選んだ検出 (書き込みなし・既に登録済み) で、行の `message` に再確認の案内が入る。Claude-Data 側・Squirrel 版では入らない。
- 既存の `agent_registration::tests`・`ui_dialogs::agent_integration::tests`・AI 連携の窓の ui_snapshot。

## 7. 未決 (サブ PC の確認で解決)

- §3.1 の表: Claude-Data 側に書いて一覧と tool に出ることを確かめた (§1)。今の版は LocalCache 側のフォルダも作るので、
  両方在るときに Claude-Data 側を取る規則が要る (規則どおり)。
- 今の版が初回の起動で LocalCache 側の mcpServers を引き継ぐかは確かめていない (新しく入れ直す確認になるため)。§3.1 の
  「残る場合」と再確認の案内はそのまま残す。
- メニュー名: Claude は 設定 →「デスクトップアプリ」→「拡張機能」の「管理」→「ローカル MCP サーバー」→「設定を編集」。ChatGPT は
  設定 →「連携」→「プラグイン」→ タブ「MCP」。§3.2・§3.5 の文に使う。

## 8. 記録
</content>
</invoke>

- 設計の独立レビュー r1 (GPT-6.1 Sol / xhigh、0866a9d47): REVISE、P2 3 件。(1) 更新後・初回起動前の LocalCache だけの状態 →
  §3.1 に「残る場合」と、再確認で戻ることの案内を足した (版の番号で分ける案・以前の版で自動登録を止める案を採らない理由も)。
  (2) 検出後に登録先が変わる → 登録先の解決を 1 つの関数にし、書き込みの worker で解決し直す。(3) `.orig.bak` の意味 → 無い
  ファイルを空の控えで表し、以前の版の `.bak` を写す場合は原本を保証しないと明記。テストを §6 に足した。
- 設計の独立レビュー r2 (同じ担当、16059f5d0): ACCEPT WITH CHANGES、P2 1 件。登録後の注記だけでは、既に登録済みで Claude が
  更新された利用者に案内が出ない → LocalCache 側を選んだ検出の行の `message` に案内を載せる (§3.1)。テストを §6 に足した。
- 設計の独立レビュー r3 (同じ担当、e7399c399): ACCEPT (§7 の実機確認待ちを前提とする判定)、新しい指摘なし。
- 実装 (Codex GPT-6.1 Sol / high): 6f8e6ef3e (登録先の解決の共通化・書き込み前の再解決・2 つのバックアップ・操作ごとの成功の文)、
  300d98c9d (「手順を見る」と ChatGPT の「設定を開く」のメニュー名)、cbc3aad09 (複数の入り方の候補にも Claude-Data の規則)。
  設計担当は 1 件だけテストを直した: フォルダ名の綴りの比較が path の文字列全体を比べていて、区切り文字の違いで落ちていた。
- 実装の独立レビュー r1 (GPT-6.1 Sol / xhigh、実装者とは別の担当、300d98c9d): REVISE、P2 2 件・P3 1 件。
  - P2: 複数の入り方のときの候補が LocalCache のまま → cbc3aad09 で直した。
  - P2: privacy で `.orig.bak` を登録前の原本と断定していた → 3f06fa8d8 で直した。
  - P3: 正本の手順の順序・成功の文・メニュー名が古い → 3f06fa8d8 で直した。
- 実装の独立レビュー r2 (同じ担当、cbc3aad09): ACCEPT、新しい指摘なし。
- 検証 (設計担当、共有 runner):
  - lib の filter `agent_registration::|ui_dialogs::agent_integration::`: cbc3aad09 で 78/78 成功。
  - ui_snapshot の filter `agent_integration|agent_registration`: 300d98c9d で 11/11 成功。画像の更新はない (フィクスチャは「手順を見る」も成功の文も表示しない)。
  - `check_ui_glyphs` は成功。本体 lib の全件は未実行 (取り込みの後に統括)。
  - 修正後の mIV で Claude Desktop・ChatGPT へ登録する実機確認は、まだ誰も行っていない。

## 9. 追加修正: 解除で空になった親を残さない (2026-11-26)

サブ PC 担当の実機確認 (修正後の master、Claude Desktop 6.28899.6) では、登録前に `mcpServers` が無い設定へ
登録してから解除すると、`"mcpServers": {}` が残り、4149 バイトから 4169 バイトになった。
コードでも、Claude・ChatGPT とも登録時に作った親を解除時に残す経路になっていた。

解除で `mimageviewer` を実際に消し、その結果 Claude の `mcpServers` / ChatGPT の `mcp_servers` が空になった場合だけ、
親ごと消す。対象項目が無い解除では空の親もそのまま残し、`Unchanged` の判定を変えない。他のサーバーが在れば親を残す。
ChatGPT の inline・dotted key の親にも同じ規則を使う。

登録前の状態や `.orig.bak` は参照せず、新しい状態を持たない。登録前から空の親が在った場合も、登録してから解除すれば
親ごと消える。空の親と親が無い状態は AI アプリにとって同じ意味なので、この違いは割り切る。
ChatGPT は登録時に親を新設する場合だけ `set_implicit(true)` とし、`[mcp_servers]` の見出しを出さず
`[mcp_servers.mimageviewer]` を足す。既存の親の書き方は変えない。

追加修正のビルド・テストは設計担当の共有 runner へ引き継ぐ。実装担当は cargo・製品起動・実設定ファイルへのアクセスを行わない。

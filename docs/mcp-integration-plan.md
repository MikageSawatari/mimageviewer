# AI 連携 (MCP) 計画書

状態: **方針合意済み (2026-10-30、§11.1)。独立レビュー済み (§13)。段階 0 の試験実装は `mcp-trial` に
あり (2026-10-31、独立レビュー 3 巡済み)。ChatGPT Windows アプリと Claude Desktop から tool を呼べることを利用者が確認した (段階 0 完了)。
実装記録は `mcp-trial` 側の本書 §14。段階 1 の詳細設計は §15 (2026-10-31、独立レビュー 3 巡を反映)。公開は段階 2 と一緒 (§11.2 U10)。**
試験実装は §11 の合意と独立レビューの後に、別 worktree (`G:\home\mimageviewer-mcp`、
branch `mcp-trial`) で行う。master のコードには触れない。

## 0. 要約

- 目的: mIV の操作を AI (Claude / ChatGPT) からも使えるようにする。最初の用途は
  画像・本の整理 (検索・タグ付け・コレクション整理)、後段で音楽ビデオエディタの編集代行。
- 推奨構成: **本体とは別の小さな exe `mimageviewer-mcp.exe` を AI アプリが PC 内で起動する
  stdio 方式**。exe は起動中の mIV 本体へ、mIV Remote とは**別の**ローカル named pipe で問い合わせる。
  mIV 自身はネットワークで待ち受けない。
- これで届くのは Claude Desktop・Claude Code・ChatGPT デスクトップアプリ (Codex)・Codex CLI。
  **ChatGPT Web / スマホ版と claude.ai Web / スマホ版は、各社のクラウドから届く公開 URL が
  必須**なので、この構成では届かない (§2)。利用者判断で対象外とした (§11.1 U1)。
- 既存の mIV Remote IPC はそのまま使わない。Remote の IPC は読み取り要求でも「操作権の取得」
  (PC 側の閲覧窓をすべて閉じるモーダル) を前提にしており、AI が整理している横で利用者が
  mIV を使う用途と両立しないため (§3)。
- 段階: 0 = 読み取り専用の試験実装 → 1 = 読み取りの拡充と配布 (縮小画像は設定で選べる) →
  2 = 変更 (タグ・レーティング・コレクション) → 2b = 移動・名前変更・ごみ箱への削除 →
  3 = 音楽ビデオエディタ。どこまで許すかは利用者が設定で事前に選ぶ 6 段の許可の範囲 (§15.2) で、
  範囲の中の操作は呼び出しごとの確認なしで行う (2026-10-31 利用者決定 U9)。
- 2026-10-30 の利用者決定は §11.1。ChatGPT Web / claude.ai Web は対象外 (stdio のみ)。

## 1. 目的と範囲

- 利用者が普段使う AI アプリから、mIV のライブラリ (お気に入りの索引、タグ、レーティング、
  コレクション、本棚、閲覧履歴) を調べ、整理を頼めるようにする。
- 例: 「タグ "背景" が付いていて★4以上の画像を一覧して」「この検索結果を新しいコレクションに
  まとめて」「未タグの AI 生成画像に、生成プロンプトから推定したタグ案を出して (適用は確認後)」
  (この「確認」は利用者がその依頼の中で求めたもの。mIV や AI アプリが呼び出しごとに出す確認ではない、U9)。
- 範囲外: AI が mIV の画面をクリック操作すること、mIV 内に AI チャットを持つこと、
  mIV が AI の API キーを持つこと。AI モデルの費用・契約は利用者の AI アプリ側。

## 2. クライアント別の接続方式 (2026-10-30 に公式文書で確認)

出典はすべて公式文書。確認日は 2026-10-30。help.openai.com は取得できず、検索結果の抜粋だけの
項目は「抜粋」と書く。

### 2.1 MCP 仕様

- 現行版は **2026-07-28** ([Versioning](https://modelcontextprotocol.io/specification/versioning))。
  前版 2025-11-25 から大きく変わり、`initialize` の握手と protocol session が無くなった (stateless)。
  各要求が `_meta` に版を持ち、必須の `server/discover` が加わった。2025-11-25 以前の握手版との
  互換手順が仕様に定義されている
  ([Changelog](https://modelcontextprotocol.io/specification/2026-07-28/changelog))。
- transport は **stdio** (改行区切り JSON-RPC) と **Streamable HTTP**。HTTP+SSE は非推奨
  ([Transports](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports))。
- ローカル HTTP サーバは `Origin` 検証が MUST (DNS rebinding 対策)、127.0.0.1 への bind が SHOULD。
- 認可は任意。HTTP で行うなら OAuth 2.1 + Protected Resource Metadata (RFC 9728) +
  PKCE + resource indicator (RFC 8707)。client 登録は Client ID Metadata Documents が SHOULD、
  Dynamic Client Registration は非推奨。**stdio は OAuth を使わず環境から資格情報を得る**
  ([Authorization](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization))。
- tool annotation (`readOnlyHint` / `destructiveHint` / `idempotentHint` / `openWorldHint`) は
  ヒントで、クライアントは信頼済みサーバ以外では信用しない。tool 呼び出しは人が拒否できるべき (SHOULD)。

### 2.2 クライアントの比較

| クライアント | PC 内 stdio | HTTP | どこから届くか | 認証 | 契約の条件 | 備考 |
|---|---|---|---|---|---|---|
| Claude Desktop (Windows) | ○ `.mcpb` 拡張 or `claude_desktop_config.json` | 「カスタムコネクタ」のみ | stdio は PC 内。コネクタは **Anthropic のクラウドから** | stdio は不要 / コネクタは OAuth・認証なし | コネクタは Free (1 個)・Pro・Max・Team・Enterprise | [ローカル](https://modelcontextprotocol.io/docs/develop/connect-local-servers)、[拡張](https://support.claude.com/en/articles/10949351)、[コネクタ試験](https://claude.com/docs/connectors/building/testing) |
| Claude Code | ○ `claude mcp add --transport stdio` | ○ localhost 可 | PC 内 | OAuth・header | 記載なし | [Claude Code MCP](https://code.claude.com/docs/en/mcp)。出力は既定 25k token 上限 |
| claude.ai Web / スマホ | × | カスタムコネクタ | **Anthropic のクラウドから**。localhost 不可、公開 URL かトンネルが要る | OAuth・認証なし | 同上 | 同上 |
| ChatGPT デスクトップアプリ | ○ Settings > MCP servers (Codex と設定共有) | ○ | PC 内 | Bearer・OAuth | 記載なし | [learn.chatgpt.com](https://learn.chatgpt.com/docs/extend/mcp?surface=app)。**通常のチャットからも使えるか、Codex のスレッドだけかは未確認** |
| Codex CLI | ○ `~/.codex/config.toml` の `mcp_servers` | ○ localhost 可 | PC 内 | Bearer・OAuth | 記載なし | `default_tools_approval_mode = "writes"` で `readOnlyHint` の無い tool だけ確認 |
| ChatGPT Web / スマホ | × (後述の Secure MCP Tunnel 経由なら可) | ○ (Streamable HTTP / SSE) | **OpenAI のクラウドから**。公開 HTTPS か Secure MCP Tunnel | OAuth・認証なし・tool ごとの混在 | 抜粋: Business / Enterprise / Edu は読み書き、Pro は読み取りのみ、Plus は不明。2026-10 に Developer mode の手順が撤去されたという投稿もあり、**最新は未確認** | [Custom MCP server](https://developers.openai.com/api/docs/guides/custom-mcp-server)。「Apps」は「Plugins」に改称。書き込み tool は既定で確認が出る |
| OpenAI Secure MCP Tunnel | stdio のサーバを前に立てられる | — | 利用者 PC から OpenAI へ外向きに polling | **OpenAI Platform の API キー**が要る | API アカウント | [Secure MCP Tunnels](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels)。600 req/分、10 MiB、120 秒 |

### 2.2.1 Grok (xAI) (2026-10-30 に公式文書で確認)

| 使い方 | 自作の MCP | PC 内 stdio | どこから届くか | 出典 |
|---|---|---|---|---|
| grok.com のコネクタ | ○ grok.com/connectors → New Connector → Custom → URL | × localhost・社内 IP は拒否 | **xAI のクラウドから**。公開 URL が要る (ngrok / Cloudflare Tunnel を案内) | [Connectors](https://docs.x.ai/grok/connectors)、[Tunneling](https://docs.x.ai/grok/connectors/custom-mcp-tunneling) |
| Grok のスマホアプリ・X の中の Grok | 公式文書に自作 MCP の追加手順なし | × | — | [FAQ](https://docs.x.ai/grok/faq) |
| Grok Bot (エージェント。Windows / Mac のデスクトップアプリあり) | ○ URL か「Bot の computer で動くコマンド」で追加 | × コマンドで足した server は **Bot のクラウド PC** で動く。利用者の PC とは別 | xAI のクラウド | [Computer and apps](https://docs.x.ai/grok-bot/computer-and-apps) |
| Grok Build (xAI 公式の CLI のコーディングエージェント) | ○ `grok mcp add <name> -- <command>` | ○ | 利用者の PC | [MCP servers](https://docs.x.ai/build/features/mcp-servers) |

- Grok Bot のデスクトップアプリは、有効にして承認した場合に**利用者の PC でコマンドを実行**できる
  (既定は毎回確認)。MCP とは結び付いていないが、mIV が同じ機能を **コマンドラインの 1 回実行**
  (`mimageviewer-mcp.exe call search_items --json '{...}'` のように JSON を返す) でも提供すれば、
  Grok Bot がそれを PC 上で実行して結果を読める可能性がある。Bot が Windows で実行結果の出力を
  受け取れるかは未確認 (§12)。利用者判断で Grok は対象外とし、作らない (§11.1 U5)。
- xAI には OpenAI の Secure MCP Tunnel に当たる仕組みが無い (公式文書は外部のトンネルを案内)。
- 結論: Grok のチャットと Grok Bot に PC 内の exe をそのままつなぐ方法は無い。届くのは
  Grok Build (CLI) と、上のコマンド実行経由 (未確認) だけ。

### 2.3 両対応に必要な構成

- **stdio の exe 1 本で、Claude Desktop / Claude Code / ChatGPT デスクトップ / Codex CLI の 4 つに届く。**
  認証は不要 (起動するのは利用者自身の AI アプリで、通信路は OS のプロセス間 pipe)。
- ChatGPT Web と claude.ai Web は、どちらも各社のサーバが mIV へ接続してくる。届かせる方法は
  (a) OpenAI Secure MCP Tunnel (ChatGPT のみ。外向き通信だけで、待ち受けポートは開かない。
  OpenAI Platform の API キーとトンネル権限が要る)、(b) 公開 HTTPS + OAuth 2.1 の認可サーバを
  mIV が持つ (Tailscale Funnel / Cloudflare Tunnel 等でインターネットに公開)。
  (b) は mIV が初めてインターネットから直接届く口を持つことになり、privacy.html の
  「外部から直接つながることはありません」が偽になる。利用者判断でどちらも作らない (§11.1 U1)。
- MCP の版は、Claude Code の新しい runtime が 2026-07-28 を使う一方、Claude Desktop の
  stdio が使う版は文書で確認できていない。**exe は 2026-07-28 と握手版 (2025-11-25 / 2025-06-18)
  の両方を受ける** (§8)。

## 3. 構成案の比較

| 案 | 内容 | 判定 |
|---|---|---|
| A. Remote IPC をそのまま使う | MCP exe が `\\.\pipe\mimageviewer-remote-thumbnail` に Remote client として接続 | **不採用**。全要求が `SessionAcquire` → `wait_until_active()` を通る (`src/remote_ipc/pipe.rs` の dispatch)。取得すると本体は全 viewer context・fullscreen・detached・player を閉じてモーダルになる (web-remote-plan §2.2)。AI に整理を頼みながら PC で見る用途が成り立たず、スマホの Remote とも操作権を奪い合う |
| B. **AI 用の別 pipe を本体に足す (推奨)** | 本体に `agent` lane を新設。MCP exe はそこへ typed 要求を送る。読み取りは本体の worker で既存の reader を呼ぶ。変更 (段階 2) は本体の既存 writer へ渡す | 採用。単一 writer (web-remote-plan §2.1) を保ち、表示状態に触れない |
| C. MCP exe が DB を直接読む | 本体が起動していなくても使える | **不採用**。`collection.db` は actor 経由以外で開かない規約 (`src/collection_store/mod.rs` 冒頭)。Tantivy 索引・各 DB の journal 設定の違いを exe 側で抱え直すことになり、書き込み (段階 2) は結局本体が要る。「mIV が起動している時だけ使える」に割り切る方が所有者が 1 つで済む |
| D. 本体に HTTP サーバを持たせる | Streamable HTTP を本体か別 exe が 127.0.0.1 で待ち受ける | 段階 0〜3 では不要。stdio で 4 クライアントに届き、HTTP にすると Origin 検証・認証・ポート衝突を新たに抱える。作らない (§11.1 U1) |

案 B の構成:

```
[AI アプリ (Claude Desktop / Claude Code / ChatGPT desktop / Codex)]
      │ stdio (JSON-RPC, MCP)
      ▼
[mimageviewer-mcp.exe]  … AI アプリが起動・終了する。業務上の状態を持たない
      │ named pipe  \\.\pipe\mimageviewer-agent-v1-<user SID>-<data dir hash> (§4.2)
      ▼                (current user only, PIPE_REJECT_REMOTE_CLIENTS)
[mimageviewer-core.exe]  agent lane (新設 src/agent_ipc/)
      ├─ 読み取り: worker thread で既存 reader を呼ぶ
      │     CollectionEngine (タグ・お気に入り・本棚・履歴) / global_search::run (Ctrl+G 検索)
      │     / CollectionStoreClient (コレクション)
      └─ 変更 (段階 2): App 所有の既存 writer へ typed 要求 (tag_write_worker / rating writer /
            CollectionStoreRuntime)
```

## 4. 本体との境界

### 4.1 表示状態の所有権 (web-remote-plan §2.2 との整合)

- 段階 0〜2 の agent lane は**本体の表示状態 (viewer context・一覧・fullscreen・detached) を
  読まず、変えない**。読むのは永続ストアと索引だけ。したがって detached リワークの凍結対象
  (viewer context / viewport 経路) には触れない。
- Remote の操作権 (`RemoteControlPhase`) との関係:
  - **読み取りはどの phase でも受ける**。Remote 自身の reader と同じ DB を読むだけで、
    Remote の cache 整合の前提 (「Remote が操作権を持つ間は本体側が変わらない」) を壊さない。
  - **変更 (段階 2) は phase が `Local` の時だけ受け、それ以外は「リモート接続中のため変更できません」
    で拒否する**。Remote の「操作権の取得 = 本体側が変わったかもしれないことの完全な信号」
    (web-remote-plan §2.2 末尾) を保つため。受付判定は既存の
    `SessionHandle::with_local_composition_admission` と同じく session の mutex の中で行い、
    受付後に BeginAcquire が来た場合は、受理済みの書き込みの完了を FinishAcquire の前提に含める
    (本の構成 editor と同型)。段階 2 の設計時に詳細化し、独立レビューを受ける。
  - ただし `Local` 判定だけでは足りない (独立レビュー P2-3)。`LocalModal` も公開 phase は `Local`
    (`src/remote_ipc/session.rs`) で、メタ情報の取り込み (`ui_dialogs/metadata_transfer.rs`) は
    既存 writer を静止させる。段階 2 の受付条件には、ローカルのモーダル・メンテナンス・終了受付も含める。
    受理済みの書き込みは共通の owner に登録し、FinishAcquire は DB 反映と UI 側の完了収穫まで待つ
    (tag writer の `is_busy()` だけでは UI 未収穫の結果を見落とす)。
  - 読み取りはどの phase でも受けるが、複数ストアをまたいだ一時点の snapshot は保証しない。
- 段階 1 で検討する「mIV で開く」tool は表示状態を変える。既存の二重起動 open-path と同じ
  App の受付経路に乗せ、Remote 所有中は同じ規則で拒否する。新しい open 経路は作らない。

### 4.2 IPC の版

- agent lane の型は新しい crate `crates/agent-ipc` (package `mimageviewer-agent-ipc`) に置き、
  **Remote の `PROTOCOL_VERSION` (現在 72) とは独立した `AGENT_PROTOCOL_VERSION`** を持つ。
  Remote 側の版上げ (他 branch と頻繁に衝突し、max+1 で合わせる運用、web-remote-plan §13.5) に
  巻き込まない。
- 照合は Remote と同じ厳密一致。MCP exe と core は同じビルドで作り同じフォルダに置くので、
  ずれるのは古い exe を指す設定が残った時だけ。ずれたら tool の結果を
  「mIV と AI 連携の版が違います。設定の案内に従って登録し直してください」の error にする。
- pipe 名は `\\.\pipe\mimageviewer-agent-v1-<user SID>-<data dir hash>`。data dir hash は
  **解決済み絶対 data dir を常に hash 化**する (既定 dir でも省略しない)。二重起動用の
  `single_instance::data_dir_namespace_suffix` は各 flavor の既定 dir で suffix を省略し、通常版と
  portable 版を基底名で分けているため、そのままは流用しない (独立レビュー P2-6)。名前の導出は
  agent-ipc crate の 1 関数にまとめ、core と MCP exe が同じ関数を使う。最初の pipe 作成は
  Remote と同じく `FILE_FLAG_FIRST_PIPE_INSTANCE` で排他する。
  `--data-dir` の隔離起動と通常の mIV が同時に動いても衝突せず、別ユーザーの同名 pipe とも
  衝突しない。MCP exe は `--data-dir` を受け、省略時は通常版の既定 data dir の pipe を開く。

### 4.3 本体の受付と負荷

- pipe server は専用 thread、要求処理は worker。UI thread で DB・索引を待たない
  (ui-responsiveness §4)。各要求に `perf::event` を入れる。
- **reader の owner** (独立レビュー P2-5): `CollectionEngine` は production では `LiveFavorites` と
  live な設定・並び順 source を要する (`new(Settings)` はテスト専用)。`LiveFavorites` は永続接続を
  持ち、設定の復元 (`settings_restore.rs`) はその **1 つの** reader control を pause してから DB を
  置き換える。よって agent lane は reader を自前で作らず、Remote と**共有する reader の束**
  (Remote の server 起動時に作っているもの) を 1 つの owner から借りる。第二の settings 接続は
  作らない。索引 (IndexerManager) の公開前は「準備中」を返し、collection actor は既存の producer
  と同じ借用で使う。終了順は「agent の受付停止 → agent worker の終了 → actor の終了」
  (`src/lib.rs` の既存終了順に挿入)。
- 設定 (§5.2) が「無効」の間も pipe は作る (起動中に設定を切り替えても pipe の作り直しが要らない)。
  **許可の判定は握手時だけでなく、要求の受付ごとに現在の設定で行う** (独立レビュー P2-4)。
  無効化・範囲縮小の時点で、待機中の要求は拒否、実行中の読み取りは結果を返さず拒否に置き換え、
  受理済みの書き込み (段階 2) は完了させて結果を返す。
- **取消・切断・同時実行** (独立レビュー P2-7): MCP exe は stdin を読む thread と要求を実行する
  thread を分け、処理中でも `notifications/cancelled` と stdin の EOF を受ける。受けたら core へ
  取消を送り、core は検索の cancel flag を立てる。20 秒は**応答の期限** (期限で途中結果を返す) で、
  worker の協調取消とは別。Tantivy の 1 ページ検索は同期処理なので、worker が実際に止まるまで
  同時実行枠を保持する。枠は接続ごと 1 件 + core 全体で 2 件。2026-07-28 の stdio では
  取消後にその要求の応答を送らない。
- 検索の時間予算は既定 20 秒で、超えたら途中までの結果と `truncated` を返す。Codex CLI の既定
  tool timeout が 60 秒のため。Ctrl+G 検索が数分かかった実績がある (2026-10、`glasses -genshin`)。
- **cursor と出力量** (独立レビュー P2-8): 既存の reader は毎回先頭から最大件数まで作る
  (検索は offset 0 から最大 10,000 件、TagItems は最大 100,000 件、タグ全一覧は 2,000 件で打切り)。
  cursor は `(版, tool, 正規化した引数, 並び順, 返した件数)` を符号化した文字列とし、続きは同じ
  問い合わせを再実行して既出分を飛ばす。索引の更新や mIV の再起動で結果がずれ得ることは
  tool の説明に書き、cursor の版が合わなければ「最初からやり直してください」の error にする。
  件数とは別に**応答全体の文字数上限 (text の JSON を含めて約 40,000 文字)** を設け、超える前に
  打ち切って cursor を返す (Claude Code は既定 25k token、50,000 文字超はファイル退避)。
  MCP 用には必要な列だけを返す投影を作る (TagItems のサムネイル用 sidecar 走査は通さない。
  headless 検索の `stars` は常に 0 なので★は rating DB から別に引く)。
- まれな失敗 (DB を開けない等) は既定の割り切り: tool error として理由を返し、ログに残す。

## 5. 安全性

### 5.1 信頼境界

- 段階 0〜3 の mIV は**ネットワークで待ち受けない**。新しい口は current user だけが開ける
  ローカル named pipe 1 本 (Remote と同じ DACL と `PIPE_REJECT_REMOTE_CLIENTS`)。
- 同じ Windows ユーザーの**通常権限・full-trust** のプロセスは、この pipe がなくても
  `%APPDATA%\mimageviewer` の DB を直接読める。その範囲では読み取り tool が新しく広げるものは無い。
  対応する接続元は「同じユーザー・通常権限・full-trust」に限る。昇格状態の違いや sandbox
  (AppContainer・低整合性レベル) による接続拒否を、DACL の拡大や integrity label の引き下げで
  回避しない (独立レビュー P3-13)。変更 tool は「同じユーザーのプロセスが
  簡単に mIV のタグ等を書ける」口になるので、設定で既定 OFF にする (§5.2)。
- pipe の相手のプロセスを exe パスで確かめる仕組みは置かない。同じユーザーの中では偽装できる
  確認で、保護として示すと誤解を招く (Remote が allowlist を撤去したのと同じ理由、
  web-remote-plan §3.1)。

### 5.2 設定 (既定は無効)

環境設定に「AI 連携 (MCP)」を置き、利用者が**事前に許可の範囲を選ぶ** (2026-10-30 利用者決定、§11 U3)。
**2026-10-31 に「読み取りと、mIV で開く」の段を足して 6 段にした (§15.2、U8)。** 下の 5 段の一覧は初版のまま残す。
許可の範囲は**順序のある 1 つの選択**にする (上の段は下の段をすべて含む)。独立したチェック欄の
組み合わせにしない (状態を減らす):

1. 無効 (既定)
2. 読み取りのみ
3. 整理まで: タグ・★・コレクションの変更 (段階 2)
4. 移動・名前変更まで (段階 2b)
5. ごみ箱への削除まで (段階 2b)

これとは別に「AI に縮小画像を見せる」チェックを 1 つ置く (既定 OFF、§11 U2)。
完全削除 (ごみ箱を経ない削除) はどの段でも提供しない。
tool ごとの許可表は持たない。**mIV が保証するのはこの許可の範囲だけ**で、呼び出しごとの確認は
AI アプリの設定に依存する (独立レビュー P1-2)。`readOnlyHint=false` は確認を促すヒントに過ぎず、
Codex の `auto` や tool 単位の承認設定、Claude の「常に許可」で確認は省ける。設定画面にそう書く。
有効化と範囲変更の画面で、AI アプリに渡る情報 (§5.4) と、その AI の提供元へ送られることを明示する。
許可の範囲より上の tool は `tools/list` に出さず、呼ばれても拒否する。範囲を変えたら
`notifications/tools/list_changed` を送る (送れない版では次の要求から反映)。

**削除の注意 (コードで確認)**: mIV の既存のごみ箱移動 (`src/delete_worker.rs`) は、移動に成功した
項目の mIV 側の情報 (タグ・★・回転・補正・ブックマーク等、`rename_key_migration` の purge) を
消す。**ごみ箱から元に戻してもファイルは戻るが、mIV のタグ・★等は戻らない**。設定画面の説明に書く。

### 5.3 読める範囲

- Remote と同じく、mIV 本体が開ける範囲と同じにし、お気に入り等をアクセス境界にしない
  (§11.1 U4 で合意)。段階 0 の tool は索引・タグ・コレクションを起点にするので、実際に返るのは
  登録済みのものだけ。任意フォルダの一覧 tool は段階 1 で判断する。

### 5.4 AI の提供元へ渡る情報

- tool の結果は AI アプリが会話の一部として各社 (Anthropic / OpenAI) のサーバへ送る。
  mIV が送るのではないが、利用者から見れば「mIV のライブラリ情報がクラウドへ行く」。
- 渡るものの一覧は §5.6。段階 0 で渡るもの: ファイルの場所 (絶対パス)、ファイル名、種類、タグ、★、
  コレクション名と登録内容、検索結果。**画像そのもの・サムネイルは渡さない**。段階 1 から、利用者が
  「AI に縮小画像を見せる」を ON にした時だけ縮小画像 (本体のサムネイル生成経路の WebP、§15.4) を
  返す tool を出す (§11.1 U2)。
  AI 生成画像の生成プロンプト (PNG tEXt / EXIF UserComment 由来の索引テキスト) を返すかは段階 1 で判断する。
- mIV は AI からの検索語と結果を履歴・診断ログへ保存しない (Everything 検索と同じ方針)。
  agent lane のログに残すのは tool 名・所要時間・件数・error 種別だけ。共有している既存処理の
  ログ (sidecar 走査失敗時の実パス、tag writer の更新後タグとパス等) は、利用者が有効にした
  診断ログの既存方針に従う。MCP 用の投影でこれらの処理を通さないことと、ログ文言を確認する
  (独立レビュー P2-11)。
- AI アプリ側の会話保存・ログ・結果ファイルは mIV の非保存方針の対象外 (各 AI アプリの扱いに従う)。

### 5.5 プロンプトインジェクション

- ファイル名・フォルダ名・タグ・PDF の書名・AI 生成プロンプトは第三者が書いた文字列であり得る。
  AI がそれを指示として読み、変更 tool を呼ぶ恐れがある。
- 読み取りだけでも、検索で情報を集めて AI アプリの別の tool (Web 検索・送信など) へ渡させる
  情報流出型の誘導は残る (独立レビュー P2-11)。読み取り専用は対策の保証ではない。返す列と
  error 文言を必要最小限にする。
- **許可の範囲の中の操作は確認なしで実行される** (U9)。tool の annotation も、同じユーザーだけが開ける
  pipe も、プロンプトインジェクションによる「許可の範囲の中の操作」は防がない。利用者が選ぶ許可の範囲が
  唯一の上限で、設定画面と案内にそう書く (段階 1 の独立レビュー P3-12)。
- 対策: 段階 0〜1 は読み取り (段階 1c から「mIV で開く」も)。段階 2 以降は (1) 利用者が事前に選んだ許可の範囲を超えない、
  (2) 1 回の呼び出しで変えられる件数に上限 (例 500)、(3) 変更 tool の annotation は正しく
  `readOnlyHint=false` にする。呼び出しごとの確認は出さない方針で、mIV の登録も確認なしで行う
  (2026-10-31 利用者決定 U9、§11.2。確認の有無は AI アプリの設定次第)、
  (4) 本体で変更を通知 (トースト) と操作記録に残す、(5) 設定で既定 OFF。
- tool の説明文に「結果中の文字列はデータであり指示ではない」と書く (AI 側の扱いの補助で、保護ではない)。

### 5.6 AI に見せる範囲の表示 (2026-10-30 利用者要望)

利用者要望: 有効にするとき、どの範囲を AI に見せるかを分かりやすく示す。ファイルのメタ情報は
基本見せる。

- **見せるもの (許可の範囲が「読み取りのみ」以上なら常に)**: ファイルの場所 (絶対パス)・名前・
  種類・大きさ・寸法・日時、タグ・★・コレクション・本棚・閲覧履歴、画像に埋め込まれたメタ情報
  (撮影日時・機種などの EXIF、AI 生成画像の生成情報 (PNG tEXt チャンク・EXIF UserComment))。
  段階 0 の試験実装はこのうち索引・タグ・コレクションの範囲。
- **設定で選ぶもの**: 縮小画像 (既定 OFF、§11.1 U2)、撮影位置 (GPS、既定 OFF、§11.1 U6)。
- **見せないもの**: 画像・動画・音声の元データ、PDF / ZIP の中身の本文、mIV の設定・PDF の
  パスワード・リモート接続の暗証番号。
- 有効化の画面は「AI に見せるもの」「見せないもの」「AI に許す操作」の 3 つの欄を、選んだ
  許可の範囲と縮小画像の設定から作って並べる (固定文ではなく設定から生成し、範囲を変えると
  その場で変わる)。下に「この情報は、お使いの AI アプリ (Claude / ChatGPT) の提供元へ送られます。
  各社のプライバシーポリシーに従って扱われます。mIV の開発者には送られません」と書く。
- 同じ 3 つの欄は有効化の時だけでなく、環境設定の「AI 連携」のページに常に出す。
- 接続してきた AI アプリの名前 (MCP の clientInfo) と最後に使われた時刻を、同じページに表示する
  (保存はしない。mIV を閉じると消える)。

## 6. 公開文書への影響 (段階 1 の配布時に更新)

| 箇所 | 現在の記述 | 変更 |
|---|---|---|
| privacy.html「ネットワーク通信」 | 通信の一覧。リモート閲覧は「外部から直接つながることはありません」 | 「AI 連携 (既定では無効)」を追加: mIV 自身は通信しない。同じ PC の AI アプリが mIV に問い合わせ、受け取った情報 (§5.4 の一覧) をその AI アプリが提供元へ送る |
| privacy.html「端末内に保存されるデータ」 | 保存データの一覧 | 設定値のみ追加。段階 2 の操作記録を残すならそれも追加 |
| privacy.html「AI 機能について」 | 本体の画像 AI は PC 内 | 本体の画像 AI (PC 内) と、AI アプリとの連携 (提供元へ送られる) を分けて書く |
| 製品ページ「安心して使えます」 | 「通信が発生する場面」カード、「本体の画像 AI 処理は PC の中で」カード | 「通信が発生する場面」に AI 連携を加える (150 字制限、`check_site_cards.py`)。画像 AI カードは「本体の」の限定で偽にはならないが、AI 連携との違いを 1 文足すか検討。「設定や履歴はあなたの PC に保存」カードも、AI アプリへ渡した情報はそのアプリ側で保存され得る点を確かめて更新する (独立レビュー P2-11)。「開発者が情報を受け取らない」は維持できる |
| マニュアル | — | 新しいページ「AI アプリとの連携」: 有効化、Claude Desktop / ChatGPT デスクトップ / Claude Code / Codex への登録手順、渡る情報 |

公開文書は段階 1 で機能を配布物に入れる時に更新する。段階 0 は配布物に入れない。

## 7. tool の段階分け

### 段階 0 — 読み取り専用の試験実装 (配布しない)

| tool | 内容 | 本体側の既存経路 |
|---|---|---|
| `miv_status` | mIV の版、AI 連携の設定、索引の状態、Remote 操作中か | 設定・`IndexerManager`・`SessionHandle` |
| `search_items` | Ctrl+G と同じ構文で検索。種類の絞り込み、件数上限、cursor | `global_search::run` (headless caller、`wake=None`) |
| `list_tags` | ピン留め / 最近 / よく使う / すべて のタグと件数 | `CollectionEngine` の TagBrowse |
| `items_by_tag` | タグの付いた項目 | `CollectionEngine` の TagItems |
| `list_collections` | コレクションの一覧と件数 | `CollectionStoreClient::list_catalog` |

全 tool に `readOnlyHint=true`、`openWorldHint=false`。結果は `structuredContent` +
`outputSchema`、同じ内容の JSON を text にも入れる。既定 50 件・最大 200 件、続きは cursor
(サーバが作る不透明な文字列。2026-07-28 の stateless 方針に合わせ、exe 側に状態を持たない)。

### 段階 1 — 読み取りの拡充と配布

- `get_collection` (中身)、`list_favorites` / 本棚 / 閲覧履歴、`get_item_info`
  (タグ・★・寸法・種類、生成プロンプトは判断後)、任意フォルダ一覧 (判断後)。
- 「mIV で開く」(`open_in_viewer`、表示状態を変える。§4.1)。
- 配布: launcher に `mimageviewer.exe --mcp` を足し、内包した `mimageviewer-mcp.exe` を起動して
  stdio をつなぐ。単体 exe 版は runtime フォルダが版ごとに変わるため、AI アプリの設定には
  利用者が置いた `mimageviewer.exe` の場所を書く方が更新で壊れない。
  現 launcher は GUI subsystem で、既存 core を activate して終わる経路があり、子の stdio の寿命も
  管理しない (`crates/launcher/src/main.rs`)。よって次の契約にする (独立レビュー P2-9):
  `--mcp` は activate 判定より**前**に分岐する。継承された標準 handle 3 本をそのまま子へ渡し、
  console subsystem の `mimageviewer-mcp.exe` を `CREATE_NO_WINDOW` で起動する。親は子の終了まで
  待って runtime の lease を保持し、子の終了コードで終わる。`AttachConsole` や GUI の error
  ダイアログは通さない。Claude Code は Windows で stdin を閉じた 300 ms 後に process tree を
  終了させ得るので、子は stdin の EOF で速やかに終わる。
  ビルド順は core → remote → EPUB worker → **MCP** → launcher とし、MCP exe の署名・launcher への
  内包・portable zip への同梱を配布手順に加える (`crates/launcher/build.rs` の内包対象)。
- AI アプリへの登録の案内と自動化は §7.5。
- 公開文書 (§6)。

### 段階 2 — 変更 (タグ・★・コレクション)

- `add_tags` / `remove_tags` (複数項目・複数タグ)、`set_rating`、`create_collection`、
  `rename_collection`、`add_to_collection`、`remove_from_collection` (登録を外すだけ。元ファイルは
  そのまま)。
- どれも本体の UI と同じ writer を通し、表示中の一覧・タグ表示は既存の更新経路で追従する。
- 受付は Remote が `Local` の時だけ (§4.1)。件数上限、変更通知、操作記録 (§5.5)。
- 段階 2 では提供しない: コレクション削除・タグの一括全消去。
- ★の XMP 書き込みは UI と同じく利用者の既存設定 (`write_rating_to_xmp`、既定 OFF) に従う。
  既存の rating 経路は設定が ON なら XMP worker にも投入する (`src/remote_ipc/ui.rs` の SetRating)。
  AI 専用の保存方針を作らず、UI・Remote と同じ writer と同じ設定で動かす (独立レビュー P1-1 への
  対応。初版の「XMP を書かない」は既存 writer の単純な再利用では守れないため、方針の方を改めた)。
- 段階 2 の設計 (App の受付経路、Remote 取得との順序、writer の完了確認) は着手前に独立レビュー。

### 段階 2b — 移動・名前変更・ごみ箱への削除 (許可の範囲 4・5)

- `move_items` / `rename_item` / `trash_items`。どれも mIV の既存の操作と同じ経路を通す:
  名前変更は `ui_dialogs/rename_item.rs` の後段と `rename_key_migration` (タグ・★等を新しい path へ
  引き継ぐ)、移動は `shell_file_ops` (Shell の `IFileOperation`)、削除は `delete_worker`
  (ごみ箱移動 + mIV 側情報の purge)。AI 用の別実装を作らない。
- これらは表示中の一覧・viewer に影響する (表示中のファイルを消す・動かす)。既存の UI 操作と
  同じく App の UI 側で受け付け、表示中の viewer を閉じる等の既存の後処理に乗せる。
  Remote が `Local` 以外なら拒否。
- 上書きはしない (移動先・新しい名前に同名があれば失敗として返す)。ZIP / PDF の中身は対象外。
- 段階 2 とは別に設計し、独立レビューを受ける (表示状態と複数 viewer context に触れるため、
  detached リワーク中の規則 (CLAUDE.md) の確認も要る)。

### 段階 3 — 音楽ビデオエディタ

- エディタは `musicvideo` branch (未 merge) の別 exe `mimageviewer-musicvideo` で、本体とは
  IPC を持たない (`src/musicvideo_launch.rs` の方針)。編集代行には、エディタ側に編集操作の
  受付口 (編集履歴の 1 段として取り消せる単位) が要る。
- エディタが master に入り、編集操作のモデルが固まってから別に設計する。MCP exe はエディタの
  口へも中継する形を想定するが、ここでは決めない。

### (段階 4 は作らない)

- ChatGPT Web / スマホ、claude.ai Web / スマホからの接続は対象外 (2026-10-30 利用者決定、§11 U1)。

## 7.5 AI アプリへの登録の案内と自動化 (段階 1)

2026-10-30 利用者要望: mIV 側で設定方法を案内し、**Claude Desktop と ChatGPT の Windows アプリを
最初の対象**に、登録をなるべく自動化する。以下は公式文書の確認結果 (2026-10-30) と開発機の
ファイル配置の確認 (中身は読まず、有無と項目名だけ) による。

### 7.5.1 Claude Desktop

| 方法 | 内容 | 利用者の操作 | 問題 |
|---|---|---|---|
| A. 設定ファイルへ追記 | `claude_desktop_config.json` の `mcpServers` に 1 項目を足す ([公式](https://modelcontextprotocol.io/docs/develop/connect-local-servers)) | mIV のボタン → Claude を完全に終了 (トレイも) → 起動し直す | 同じファイルに Claude 自身の設定 (`preferences` 等) が同居し、Claude が動作中に書き換える (開発機で確認、2026-10-30 の更新時刻)。動作中に書くと上書きされ得る。MSIX 版は保存先が `%LOCALAPPDATA%\Packages\Claude_<id>\LocalCache\Roaming\Claude\` へ振り替わる場合がある (GitHub issue の報告、未修正) |
| B. `.mcpb` (MCP Bundle) | mIV が `.mcpb` を作って開く。Claude の導入画面が出る ([公式](https://claude.com/docs/connectors/building/mcpb)) | Claude の画面で「インストール」 | exe は bundle の中に入れる前提 (外の exe を指すのは文書上未確認)。bundle を作り直すたびに手で入れ直し。一部の MSIX 版で導入画面がすぐ閉じる報告。`.mcpb` の関連付けが登録されているかは文書に無い |

- 推奨: **A を主にする**。理由: 外の exe (利用者が置いた `mimageviewer.exe --mcp`) を直接指せるので
  mIV の更新で入れ直しが要らない。B は bundle に MCP exe を入れると、mIV の更新ごとに agent IPC の
  版がずれて入れ直しが要る。
- A の手順 (mIV が行う):
  1. Claude が動作中 (`claude.exe` のプロセス) なら書かずに「Claude を終了してから押してください」と出す
     (トレイ常駐も含む)。終了の待ち合わせや自動終了はしない。
  2. 保存先の決定: MSIX の package (`Claude_*`) の振り替え先に既存のファイルがあればそれを、無ければ
     `%APPDATA%\Claude\claude_desktop_config.json`。開発機の Claude は MSIX ではなく
     `%LOCALAPPDATA%\AnthropicClaude` の版で、`%APPDATA%\Claude` の側だった。
  3. JSON を読み、`mcpServers.mimageviewer` だけを足す・置き換える。他の項目は手を付けない。
     元のファイルを `.bak` に残し、一時ファイル + rename で書く。JSON が読めなければ書かない。
  4. 結果を表示し、「Claude を起動して、新しいチャットで mIV の tool が出るか確かめてください」と案内。
- 書く内容: `{"command": "<mimageviewer.exe の絶対パス>", "args": ["--mcp"]}`
  (portable / 隔離起動では `--data-dir` も)。
- B は、A が使えない環境の報告があれば検討する。

### 7.5.2 ChatGPT の Windows アプリ

- 公式: Windows アプリは Codex と同じ `%USERPROFILE%\.codex` を使い
  ([Windows app](https://learn.chatgpt.com/docs/windows/windows-app.md))、MCP の設定は ChatGPT
  デスクトップ・Codex CLI・IDE 拡張で共有 ([MCP](https://learn.chatgpt.com/docs/extend/mcp))。
  登録画面は Settings > MCP servers > Add server > STDIO、保存後に再起動。追加のための
  deep link は無い (`codex://settings` は開ける)。
- 自動化: `config.toml` に `[mcp_servers.mimageviewer]` を足す。書式を保つ TOML 編集
  (Rust `toml_edit`) で他の項目・コメントを残し、`.bak` + 一時ファイル + rename。`CODEX_HOME` が
  あればそちら (アプリが従うかは未確認)。
  ```toml
  [mcp_servers.mimageviewer]
  command = 'C:\...\mimageviewer.exe'
  args = ["--mcp"]
  default_tools_approval_mode = "approve"  # 確認なし (2026-10-31 U9 で "writes" から変更、§15.7)
  ```
- 書いた後は「ChatGPT アプリを再起動してください」と案内し、`codex://settings` を開くボタンを置く。
- **注意 (利用者へ伝える)**: 公式文書は「ローカルで動くスレッド」で使えると書く。通常のチャットから
  ローカルの tool が呼ばれない不具合の報告がある ([openai/codex#38162](https://github.com/openai/codex/issues/38162)、
  2026-10 時点で未回答)。Codex のスレッド・ローカルの作業スレッドでは動くという報告。
  利用者の環境でどう動くかは試験実装の確認項目にする。

### 7.5.3 mIV の画面

- 環境設定「AI 連携」に、許可の範囲 (§5.2)、見せる範囲の表示 (§5.6)、AI アプリごとの行を置く。
- AI アプリごとの行: 検出結果 (入っている / 見つからない)、登録状態 (未登録 / 登録済み /
  登録先が今の mIV と違う)、「登録する」「登録を外す」「手順を見る」ボタン。
  検出と登録状態の判定は worker で行い UI thread でファイルを読まない。
- 「手順を見る」: 自動登録を使わない人向けに、コピーできる設定内容と各アプリの画面の手順を出す。
- 登録状態が「登録先が違う」(mIV を移動した等) なら、その行に「登録し直す」を出す。起動時の
  自動書き換えはしない (他のアプリの設定を黙って変えない)。
- 登録・解除は利用者がボタンを押した時だけ。押した時に、書き換えるファイルと書く内容を表示して確認する。
- Grok は対象外で、案内の行も置かない (§11.1 U5)。

## 8. MCP の実装方針

- exe は Rust。公式 Rust SDK (`rmcp`) が 2026-07-28 と旧版の互換対応を公表している
  ([rust-sdk](https://github.com/modelcontextprotocol/rust-sdk)、独立レビュー P2-10) ので、
  **`rmcp` を第一候補**にし、実装の最初に版対応と Windows stdio の動作を確かめる。tokio は
  MCP exe の中だけで使い、core には async runtime を持ち込まない (web-remote-plan §3.4 の方針は
  core と remote の話)。`rmcp` が要件を満たさなければ手書きの JSON-RPC に切り替える。
- stdin を読む thread と要求の実行を分ける (§4.3 の取消・EOF)。
- 受ける版と契約 (独立レビュー P2-10):
  - 2026-07-28: 各要求の `_meta` の版と client capabilities、必須の `server/discover`、全結果の
    `resultType`、list 結果の `ttlMs` / `cacheScope`、未対応版への `UnsupportedProtocolVersionError`。
  - 握手版 2025-11-25 / 2025-06-18: `initialize` / `initialized`、交渉した版を接続の間保持する。
  - 「状態を持たない」は業務上の状態 (cursor 等) の話で、握手版の交渉済み版は保持する。
  - 版ごとの差は exe 内に閉じ込め、本体の agent IPC は MCP の版に依存しない。テストは版ごとに書く。
- stdout には MCP の JSON 以外を書かない。診断は stderr (Claude Desktop はこれを
  `%APPDATA%\Claude\logs\mcp*.log` に残す)。
- 本体が起動していない時は、tool の結果を「mIV が起動していません。mIV を起動してから
  もう一度お試しください」の error にする (exe から本体を起動はしない。単一 instance と
  設定 DB の扱いを増やさないため)。

## 9. 試験実装 (段階 0) の範囲と検証

### 9.1 作るもの

1. `crates/agent-ipc`: 要求・応答の型、版、framing (Remote と同じ 4 byte 長 + JSON)。
2. 本体 `src/agent_ipc/`: pipe server、握手、段階 0 の 5 tool の処理。設定項目
   (段階 0 では 無効 / 読み取りのみ の 2 値でよい。UI は最小の 1 行)。
3. `crates/mcp-server` (bin `mimageviewer-mcp`): stdio の MCP 処理と pipe client。
4. 文書: 本書の更新、docs/README.md の索引。

### 9.2 検証

- 自動テスト: 版ごとの JSON-RPC 処理 (握手版・stateless 版・未対応版・不正 JSON)、tool の
  入出力 schema、agent-ipc の codec、本体側 handler を一時 data dir の fixture (タグ・
  コレクション・索引) で。Remote の phase による受付判定 (段階 2 で本格化)。
- 結合 (自動): core 側 handler と MCP exe の stdio 処理は、実 core を起動せずに fixture で
  完結させる (handler を test 用の pipe server に載せ、MCP exe をその pipe へ向けて stdio で駆動)。
- 結合 (実 core): 実アプリの起動は検証規約 (AGENTS.md / CLAUDE.md「実機検証と設定データの保護」)
  に従い、`.\scripts\prepare-portable-smoke.ps1` で作る使い捨て portable を使う
  (データは `target\portable-smoke\data`、利用者の設定はコピーしない。独立レビュー P2-12)。
  その core へ、AI クライアントの非対話実行から tool を呼ぶ。利用者の Claude / Codex の設定ファイルは
  書き換えない (登録はその回の実行だけに渡す)。通常データでは起動しない。
  - 通常の確認は `.\scripts\mcp-client-smoke.ps1` (2026-10-31 追加、`mcp-trial` 側)。
    Codex CLI (`codex exec --json -c mcp_servers.<名前>...`) が既定で、tool ごとに渡した引数・成否・
    最終回答を表示し、生ログを `target\mcp-client-smoke\` に残す。ChatGPT Windows アプリと同じ Codex エンジンで、
    設計担当・実装担当が段階ごとに何度でも回せる。
  - `-Client claude` は Claude Code CLI (`claude -p --mcp-config ... --strict-mcp-config`)。
    この PC の CLI のログインが切れている間は、接続までしか確かめられない。
  - Claude Desktop と ChatGPT アプリの実機確認は、段階の区切り (配布前など) だけ利用者に頼む。
    CLI と実アプリで差が出得るのは、旧版の握手・許可の UI・時間切れ・結果の見せ方。
- 利用者の確認: Claude Desktop と ChatGPT デスクトップアプリ (Codex) への登録手順を渡し、
  利用者の環境で呼べるかを見てもらう (観測者は利用者)。

### 9.3 完了の目安

本書の §11 が合意済み、独立レビュー済み、段階 0 の tool を AI クライアントから隔離起動の本体へ
呼べたログがある、利用者の Claude Desktop での確認結果 (成否) が記録されている。
2026-10-31 時点: Codex CLI からの呼び出しログあり、ChatGPT アプリと Claude Desktop は利用者が確認済み
(段階 0 の完了条件を満たした)。

## 10. 状態を減らす検討の記録

| 検討 | 結論 |
|---|---|
| Remote の操作権をそのまま使えば排他は既存の仕組みで済む | 不採用。PC がモーダルになり、主用途 (使いながら整理を頼む) が成り立たない。代わりに「表示状態に触れない」lane にして、排他そのものを不要にした |
| AI の変更を Remote 接続中も受ける | 受けない。Remote の cache 整合の前提を守るため、変更は `Local` の時だけ。組み合わせが 1 つ減る |
| mIV が起動していない時も使えるようにする | しない。exe が DB を直接読む経路を持つと所有者が 2 つになる |
| exe 側にセッションや cursor の状態を持つ | 持たない。cursor は不透明な文字列で毎回本体に渡す (MCP 2026-07-28 も stateless) |
| tool ごとの許可設定 | 置かない。順序のある 6 段の選択 1 つ (§15.2) + 縮小画像・撮影位置の ON/OFF。呼び出しごとの確認は出さない (U9) |
| 許可を独立したチェック欄にする | しない。上の段が下の段を含む 1 つの選択にして組み合わせを 5 通りに抑える |
| pipe を設定に合わせて作り直す | しない。pipe は常設し、許可は要求の受付ごとに現在の設定で判定する |
| 移動・名前変更・削除に AI 専用の確認画面や取り消し機能を作る | 作らない。事前の許可範囲 + 既存の操作経路 (ごみ箱) に任せる (AI アプリの確認は U9 で出さない)。完全削除は出さない |
| まれな失敗 | 既定の割り切り (ログ + tool error) |

## 11. 利用者の判断が要る点

- **U1. ChatGPT Web / claude.ai Web (クラウドから届く接続) をどうするか**。
  推奨: 段階 0〜3 は PC 内 (stdio) に絞る。ChatGPT はデスクトップアプリ (Codex) で使う。
  Web 版が必要なら、まず OpenAI Secure MCP Tunnel (待ち受けポート無し、API キー要) を試し、
  公開 HTTPS + OAuth は作らない。
  - 2026-10-30 利用者: 「公開 URL が一般の利用者に厳しいなら、stdio 経由や LAN 内のみに限る設計でもよい」。
    設計側の回答: 公開 URL は一般の利用者には厳しい (トンネルの設定、mIV 側の OAuth 認可サーバ、
    インターネットへの常時公開の 3 つが要る)。また **LAN 内に限っても ChatGPT Web / claude.ai には
    届かない** (接続してくるのは各社のクラウドで、利用者の LAN の中ではない)。LAN 内の HTTP で
    届くのは PC 上の AI アプリだけで、それは stdio で足りる。よって **stdio のみ** とし、
    LAN 向け HTTP も作らない方向で確認中。
- **U2. 画像そのもの (サムネイル) を AI に見せるか**。推奨: 段階 0〜1 はメタデータのみ。
  「見た目で分類」が必要になったら、設定で明示的に有効にした時だけ縮小画像を渡す tool を後で足す。
- **U3. 削除・移動・名前変更など取り消せない操作**。推奨: AI 連携では提供しない。
- **U4. AI が読める範囲**。推奨: Remote と同じく mIV が開ける範囲 (お気に入りを境界にしない)。
  段階 0 は索引・タグ・コレクション起点なので実質は登録済みのものだけ。

- **U5. Grok Bot 向けのコマンド実行の口**。§2.2.1。推奨: 段階 1 では作らず、Grok は Grok Build
  (CLI、stdio) の案内だけにする。Grok Bot が PC のコマンドの出力を受け取れると確かめられたら検討。
- **U6. 撮影位置 (GPS)**。推奨: 見せない (整理には不要で、自宅などの位置が漏れる)。
- **U7. Claude Desktop の自動登録の方式**。推奨: 設定ファイルへの追記 (§7.5.1 A)。

### 11.1 利用者の回答 (2026-10-30)

- U1: **stdio のみ**。ChatGPT Web / claude.ai Web は対象外。LAN 向け HTTP も作らない。段階 4 は削除。
- U2: **最初から設定で選べる**。段階 1 で「AI に縮小画像を見せる」設定 (既定 OFF) と縮小画像の
  tool を入れる。段階 0 の試験実装は画像を返さない。
- U3: **利用者が事前に許可の範囲を設定する**。移動・名前変更は中身を失わないので許容する利用者が
  多い、削除もごみ箱から戻せるので利用者が許可すればあり、との判断。§5.2 の 5 段の選択と
  段階 2b に反映。ごみ箱から戻しても mIV のタグ・★等は戻らない点は設計側から伝えた。
- U4: **mIV と同じ範囲**。
- 追加要望: 有効化の時に見せる範囲を分かりやすく示す、メタ情報は基本見せる (§5.6)。mIV 側で
  設定方法を案内し、Claude Desktop と ChatGPT の Windows アプリを最初の対象に登録を自動化 (§7.5)。
- U5: **Grok は対象外** (案内も置かない)。§2.2.1 の調査記録は残す。
- U6: **撮影位置 (GPS) は設定で選べる** (「撮影位置も見せる」、既定 OFF)。§5.6 に反映。
- U7: **Claude Desktop は設定ファイルへの追記** (§7.5.1 A)。

### 11.2 利用者の回答 (2026-10-31、段階 1 の設計前)

- U8: **「mIV で開く」は入れる**。利用者が事前に許可した場合だけ使え、そのとき呼び出しごとの確認は出さない。
  許可の範囲に「読み取りと、mIV で開く」の段を足す (§15.2)。
- U9: **呼び出しごとの確認は基本的に出さない**。最初に決めた許可の範囲の中で AI に操作させる。
  mIV の登録の自動化では、AI アプリ側の確認も出ない設定で登録する (§15.7)。tool の annotation は
  正しく付ける (変更する tool に `readOnlyHint=true` を付けて確認を避けることはしない)。
  §5.5 (3) の「確認を促す」は、この決定で「確認は AI アプリの設定次第。mIV の登録は確認なしで行う」に改める。
- U10: **公開は段階 2 (タグ・★・コレクションの変更) がそろってから**。段階 1 は配布の仕組みまで作るが、
  公開文書 (§6) の更新とリリースは段階 2 と一緒に行う。それまで `mcp-trial` で開発を続ける。

## 12. 未確認の事項

- ChatGPT Web で custom MCP server を使える契約の最新の範囲 (help.openai.com を取得できず)。
- ChatGPT デスクトップアプリの通常のチャットが、ローカルの MCP サーバを使えるか (Codex の
  スレッドだけの可能性)。
- Claude Desktop の stdio 接続が使う MCP の版。
- OpenAI `tunnel-client` の Windows 対応 (段階 4 を作らないので重要度は低い)。
- Grok Bot のデスクトップアプリが Windows で利用者の PC のコマンドを実行し、その出力を Bot が
  受け取れるか。Grok の契約ごとの提供範囲 (xAI の料金ページは取得できず)。

## 13. 独立レビュー (2026-10-30、gpt-6.1-sol / xhigh、対象 `eeb7b043c`)

| 指摘 | 判断 | 反映先 |
|---|---|---|
| P1-1 rating writer は設定次第で XMP も書く | 妥当。AI 専用の保存方針を作らず UI と同じ設定に従う方針へ改めた | §7 段階 2 |
| P1-2 `readOnlyHint=false` で確認は保証されない | 妥当。mIV が保証するのは許可の範囲だけと明記 | §5.2、§5.5 |
| P2-3 `Local` 判定だけでは段階 2 の受付に不足 | 妥当。段階 2 の設計で受付条件を拡げる | §4.1 |
| P2-4 握手時だけの `disabled` では無効化できない | 妥当。要求ごとに判定 | §4.3 |
| P2-5 reader の owner が未定義 | 妥当。Remote と共有する reader の束を借りる、終了順 | §4.3 |
| P2-6 pipe 名の suffix 流用では flavor・ユーザーを区別できない | 妥当。SID + 常に hash | §4.2 |
| P2-7 同期 stdio では取消・EOF を受けられない | 妥当。thread 分離、期限と協調取消の区別、同時実行枠 | §4.3、§8 |
| P2-8 cursor・出力量の契約が無い | 妥当。再実行 + skip の cursor、文字数上限、投影 | §4.3 |
| P2-9 launcher の通常経路では stdio を中継できない | 妥当。`--mcp` の契約とビルド順 | §7 段階 1 |
| P2-10 新旧 MCP の差分が不足、`rmcp` は対応済み | 妥当。`rmcp` を第一候補に | §8 |
| P2-11 読み取りでも流出型の誘導が残る、既存ログ | 妥当 | §5.4、§5.5、§6 |
| P2-12 実 core 起動の手順が検証規約と不一致 | 妥当。portable smoke を使う | §9.2 |
| P3-13 「新しい露出なし」は full-trust 前提 | 妥当 | §5.1 |

確認された前提: Remote の通常データ要求は操作権必須、指定した reader は worker から使える、
純粋な読み取りを全 Remote phase で受けても cache 整合を壊さない、data dir はプロセス内で固定。

## 14. 段階 0 の実装記録 (2026-10-31、round 6 修正、基点 `f325b33a2`)

作業先は `G:\home\mimageviewer-mcp` / `mcp-trial`。製品 core・launcher・インストール版は起動せず、
通常の APPDATA をテストに使わない。MCP adapter だけは、設計担当の明示判断に従い使い捨て fixture
への stdio 結合テストから起動する。公開文書・配布スクリプト・Remote wire v72 は変更しない。

- `crates/agent-ipc`: MCP に依存しない要求・応答、独立版 v1、長さ上限つき codec。
  SID と解決済み絶対 data dir の SHA-256 (先頭 128 bit) を常に名前に含める。
  `resolve_data_dir` / `derive_pipe_name` を両 exe が共有する。ファイルを開かず、dot component・
  大小文字・separator・verbatim/UNC 表記を揃える。
- `SharedReaders`: 旧 Remote 起動経路の `LiveFavorites` と `CollectionEngine` を一つの owner へ
  抽出し、両 lane が同じ `Arc` を借りる。設定復元用 control もその同一 reader を指す。
  Remote の sort/settings source・DACL helper の内容・操作権ルールは維持する。
  agent に第2の settings DB 接続や第2の `LiveFavorites` は作らない。
- FTS は既存 `IndexerManager::clone_fts_index` から公開するだけで、agent は別の索引を開かない。
  未公開の検索は NotReady。collection は既存 actor の producer lease と client を借りる。
  終了は agent pipe の停止・worker join を既存 producer drain / actor shutdown より前に置く。
- 許可は DB の live 値を要求ごとに確認する。設定 DB の mutex 内で level と epoch を一緒に取得し、
  許可変更の commit / DB 再 open で epoch を変える。無関係な設定保存では変えない。
  実行完了時と pipe writer の送信直前にも照合し、Disabled→ReadOnly の往復でも旧結果を拒否する。
- 接続ごと1件・全体2件の permit。検索は headless `global_search::run(..., wake=None)` と
  応答 owner を分け、20秒期限では部分結果を返して協調取消する。実 worker の join まで permit は
  保持する。Hello 等の完了済み read は permit 解放後に応答を公開する。
- pipe は server / client とも overlapped I/O。server の停止 event は各 I/O の待機集合に入り、
  停止と I/O 開始の競合をなくす。停止時は `ConnectionAborted` (再試行しない終端) を返す。
  client の各 read/write は独立した OVERLAPPED を持ち、応答待ち中でも Cancel を送れる。
- MCP exe は実際の stdin EOF を stdin owner の専用 hook から `ExitProcess(0)` へ渡す。
  stdout backpressure・Rust stdout cleanup・worker join を待たず、OS が pipe を閉じる。
  入力 queue 満杯での終了は round 3 で削除し、通常の backpressure とした。
  test は hook を通知 callback に置き換え、blocked writer 中の真の EOF と、入力を閉じない
  burst の継続・完全な JSON line の配送を別々に検査する。
- 投影は path/subresource・name・kind・tags・rating、tag/collection の name/count だけ。
  TagItems の thumbnail/sidecar materializer は通さず、★は rating DB から別に引く。
  全一覧 tool の cursor は版・tool・正規化引数/順序・返却済件数を符号化し、再実行して skip する。
  core page は8,000文字を目安に制限し、MCP は structuredContent と text の重複・escaping を含む
  最終 JSON が40,000文字以内か実測する。上限超過を黙って成功にはしない。
- ログ/perf は tool・duration・count・error kind だけ。問い合わせ文字列・結果 path は記録しない。
  設定移行の opt-in 除外表にも agent_level を追加し、他 PC の許可を自動で有効化しない。

### SDK の選択

[公式 Rust SDK](https://github.com/modelcontextprotocol/rust-sdk) は 2026-07-28 と旧版への対応、
stdio transport を公表している。導入を第一候補として `cargo info rmcp`、online での取得、
`target/rmcp-sdk-probe` の最小 SDK consumer の `cargo check --offline` を試したが、
キャッシュに rmcp がなく、registry 取得は proxy 127.0.0.1 への接続拒否、最小 build も
`no matching package named rmcp found` (exit 1) で止まった。この環境では導入できないため、
§8 の代替として serde による限定 JSON-RPC 実装を採用した。SDK の版対応不足とは判断していない。

MCP の新旧契約は公式の [versioning](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning)、
[server/discover](https://modelcontextprotocol.io/specification/2026-07-28/server/discover)、
[caching](https://modelcontextprotocol.io/specification/2026-07-28/server/utilities/caching) に照合した。
Windows の同時 read/write は [overlapped pipe I/O](https://learn.microsoft.com/en-us/windows/win32/ipc/synchronous-and-overlapped-input-and-output)
の契約に従う。

### 設計担当の決定と追加修正 (ClaudeCode、2026-10-30)

1. client の再接続間隙・複数 AI アプリによる `ERROR_PIPE_BUSY` (231) は通常利用で到達する。
   `WaitNamedPipeW` と再 open を**合計2秒**の一つの期限内で行い、期限後は Busy とする。
   `ERROR_FILE_NOT_FOUND` (2) は NotReady、`ERROR_ACCESS_DENIED` (5) は権限拒否専用の日本語
   (既存 ErrorCode では Internal)、その他は短い Internal として OS code だけ stderr に記録する。
   DACL・integrity level は拡大しない。fixture の Disconnect→Connect 間隙を sleep で隠さない。
2. `ListTags{choice, limit, cursor}` / `ListCollections{limit, cursor}` に変更する。
   MCP schema と parser も同じ契約へ揃え、既定50件・最大200件・文字数上限・不透明 cursor を使う。
   共通の `bounded_rows` が文字数調整後の実返却件数で cursor を進める。タグの choice/並び順、
   collection の catalog 順を cursor の引数に含め、別条件の cursor は BadRequest にする。
3. `stdio_exe_uses_real_core_fixture_handlers` の ignore を解除する。MCP adapter の fixture 起動は
   許可済み。`CARGO_BIN_EXE_mimageviewer-mcp` が使えない core lib test では、Cargo の
   `compiler-artifact.executable` から adapter を build/取得する。target/profile の推測はしない。
   test pipe は実 core handler と使い捨てタグ・collection・FTS fixture を使い、`--data-dir` を
   必ず明示する。core / launcher / 利用者の profile は起動しない。

### ClaudeCode が sandbox 外で実行した証拠 (2026-10-30、基点 `0a993c355`)

以下は ClaudeCode が通常権限 full-trust context で実行し、実装担当へ報告した結果。
実装担当がこの sandbox 内で実行した結果と混同しない。

- `cargo nextest run -p mimageviewer --lib -E 'test(/agent_ipc/) or test(/agent_level/) or test(/remote_ipc::live_favorites/) or test(/settings_transfer::tests::automatic_network_enablement/) or test(=remote_ipc::pipe::tests::remote_pipe_dacl_has_one_current_user_ace)'`
  → **exit 0、15/15 成功**。idle reader の shutdown 検査も成功。前回 sandbox での OS 5 は
  restricted token によるもので、core の製品欠陥ではないと設計担当が確認した。
- `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored`
  → **exit 101**。probe 接続は成功したが、再接続の client open が ERROR_PIPE_BUSY となり、
  `real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld` が失敗した。
  上記1の修正はこの通常利用の不具合への対応である。修正後の full-trust 成功は別の gate とする。

### 初回試験の自動検証の証跡 (基点 `0a993c355`、履歴)

MCP の既定 data dir は、GUI/native 依存を console exe に持ち込まないため、通常版の
`data_dir::flavor_default` の `%APPDATA%/mimageviewer` (未設定時は `./mimageviewer`) を
小さな関数で再現した。解決と pipe 名導出は agent-ipc の同じ関数を使用する。

| コマンド | 実 exit code / 結果 | ログ |
|---|---|---|
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 | `target/agent-core-check-final.log` |
| `cargo check -p mimageviewer-mcp --bin mimageviewer-mcp` | 0 | `target/agent-mcp-bin-check-final.log` |
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp` | 101。IPC 6成功、MCP 11成功/1失敗 | `target/agent-crates-final-diagnostic.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/agent_ipc/) or test(/agent_level/) or test(/remote_ipc::live_favorites/) or test(/settings_transfer::tests::automatic_network_enablement/)'` | 100。14実行、13成功/1失敗 | `target/agent-focused-final.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(=remote_ipc::pipe::tests::remote_pipe_dacl_has_one_current_user_ace)'` | 0。1成功 | `target/agent-pipe-dacl-check.log` |
| `cargo fmt` → `cargo fmt --check` | 0 | 全 workspace、引数なし |
| `python scripts/check_ui_glyphs.py` | 0。危険 glyph なし | tool 実行記録 |
| `git diff --check` | 0 | tool 実行記録 |

core の失敗は `shutdown_interrupts_an_idle_connected_reader_without_retry` の client
`OpenOptions::open` における OS 5 (AccessDenied)。MCP の失敗は当初
`real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld` の要求到着待ち timeout。
fixture thread 起動前の client 接続を診断するテストに直し、同じ接続が OS 5 で拒否されることを
root と MCP 担当の両 context で確認した。テスト失敗経路に待機 thread を残さない。
root の実行 token は `OpenProcessToken(TOKEN_QUERY)` / `IsTokenRestricted` で **True**
(照会コマンド exit 0) と確認した。§5.1 / P3-13 の full-trust 条件を満たさない環境と整合するが、
両失敗の原因確定や実装の正常性は主張しない。DACL・integrity level を広げず、失敗を skip/pass に
変更しない。通常権限 full-trust の環境で該当する実 pipe 検査を行う必要がある。

途中の記録: 初回 core check は新設定の settings-transfer 網羅表不足で非0となり、許可の自動移行を
除外して修正した。初回 lib test は turbojpeg-sys の CMake/MSBuild が exit 1 (cargo subprocess 101)
で止まり、テストは未実行。Ninja への切替試行も既存 generator 不一致で exit 101。
元の generator のまま `MSBuild /m:1` (exit 0) と `CARGO_BUILD_JOBS=1` によって依存を構築できた。
その途中差分の焦点8件は exit 0 (`target/agent-focused-tests-r3.log`) だが、上表の最終結果で置き換える。
MCP 担当側の IPC 6 / MCP 12 成功 (各 exit 0) もあるが、root の再実行失敗を打ち消す証拠とはしない。

初回試験では stdio exe 結合テストは ignored・未実行、確認用 `build-dev.ps1` も未実行だった。
commit は ClaudeCode が `0a993c355` として作成した。実装担当は commit していない。
通常 APPDATA へのテスト・snapshot 更新・test-full は実施していない。
実 core / Claude Desktop / ChatGPT からの確認は別の受入れ gate に残る。

### Round 2 の sandbox 内検証 (実装担当実行、のちに `28664e3dd` として commit)

| コマンド | 実 exit code / 結果 | ログ |
|---|---|---|
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 | `target/mcp-followup-core-check.log` |
| `cargo check -p mimageviewer-mcp --bin mimageviewer-mcp` | 0 | `target/mcp-followup-mcp-check.log` |
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` | 101。IPC 6成功、MCP 17成功/1失敗、ignored 0 | `target/mcp-followup-crates-final.log` |
| 下記の焦点 nextest | 100。18実行、16成功/2失敗 | `target/mcp-followup-focused.log` |
| `cargo build -p mimageviewer-mcp --bin mimageviewer-mcp --message-format=json` | 0。Cargo が adapter の executable artifact を報告 | `target/mcp-followup-adapter-artifacts.jsonl` / `target/mcp-followup-adapter-build.log` |
| `cargo fmt` → `cargo fmt --check` | 0。全 workspace | tool 実行記録 |
| `python scripts/check_ui_glyphs.py` / `git diff --check` | 各0 | tool 実行記録 |

接続エラー分類と Busy 残予算の5件、MCP schema/parser のページ引数検査が成功した。
core 側では205件のタグ・collection を既定50件から継続して最後まで取得する検査、choice/tool が
異なる cursor の拒否、文字数調整後の実返却件数と cursor の検査も成功した。
独立レビューは追加差分の接続予算・エラー分類・共通ページ境界・fixture adapter 解決を確認し、
追加の P1/P2 はなかった。実 pipe 成功をそのレビューで代替してはいない。

残る失敗は次の3件。制限 token の回避や skip/ignore は追加していない。

- MCP: `pipe::tests::real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld`
  は client の preflight open で OS 5。
- core: `agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry`
  は client open で OS 5。基点の同検査は ClaudeCode の full-trust 実行で成功済み。
- core: `agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers`
  は fixture pipe の作成で OS 5。今回は adapter の起動まで到達しなかった。

adapter だけはビルド済み。core/launcher の起動、通常 APPDATA での実行、snapshot 更新、test-full、
commit は行っていない。実 pipe の最終 gate が未成立のため `build-dev.ps1` は未実行。

### ClaudeCode が通常権限 full-trust context で実行する最終コマンド

```powershell
Set-Location G:\home\mimageviewer-mcp
cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored
cargo nextest run -p mimageviewer --lib -E 'test(/agent_ipc/) or test(/agent_level/) or test(/remote_ipc::live_favorites/) or test(/settings_transfer::tests::automatic_network_enablement/) or test(=remote_ipc::pipe::tests::remote_pipe_dacl_has_one_current_user_ace)'
```

後者には ignore を外した stdio 結合検査が含まれる。必要な adapter の build/パス解決は検査内で
Cargo artifact を通して行い、起動先には使い捨て fixture の `--data-dir` を渡す。
成功を確認するのは上記3件の実 pipe 経路を含む両コマンド全体。結果は次の round 2 記録を参照。

### Round 2 の sandbox 外検証 (ClaudeCode 実行、基点 `28664e3dd`)

以下は ClaudeCode が通常権限 full-trust context で実行し、round 3 の依頼で報告した結果。
実装担当が sandbox 内で実行した結果ではない。

- `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored`
  → **exit 0、IPC 6件 + MCP 18件成功**。
- 上記の core 焦点 nextest → **exit 0、18/18成功**。
  `stdio_exe_uses_real_core_fixture_handlers` による adapter 起動も含む。

### Round 3 の受理指摘と設計

`4b3c9da43..28664e3dd` に対する独立レビュー (gpt-6.1-sol / xhigh、read-only) の
P2 3件・P3 2件を設計担当がすべて受理した。

- **legacy の tool 一覧更新**: 実際の agent wire 応答には permission epoch が含まれないため、
  adapter が既存 Status IPC を legacy Ready 中に1秒間隔で照会し、有効な tool 集合の変化を観測する。
  Stage 0 の集合は Disabled/ReadOnly の2通りだけなので wire の epoch 追加は不要。
  第二の設定 reader や core 側の subscription/接続所有を作らず、idle 中も照会できるこの方式を選ぶ。
  legacy handshake では `tools.listChanged=true` を広告し、変化時に
  `notifications/tools/list_changed` を送る。監視停止時は timer を止め、未完了の同期 IPC を join しない。
  古い poll 結果が新しい tools/list・Disabled 観測を上書きしないよう、Session 内の局所 revision で照合する。
- **modern の更新**: `subscriptions/listen` には filter・ack・終了/cancel の所有が追加で必要になる。
  Stage 0 ではこれを増やさず、2026-07-28 の tools/list は引き続き `ttlMs=0` / `cacheScope=private`
  とし、必要時の再一覧取得で更新する。modern に `listChanged` を広告せず、未購読の通知も送らない。
  [公式 SDK の版移行説明](https://ts.sdk.modelcontextprotocol.io/v2/migration/support-2026-07-28) の
  subscription による change 通知の扱いを確認した。
  [公式 caching](https://modelcontextprotocol.io/specification/2026-07-28/server/utilities/caching) は
  listChanged を広告せず TTL だけを提供する方式を許容している。
- **出力と制御**: stdout は有界 channel に接続した単一 writer が所有する。
  stdin owner は受信した取消を shared Active owner へ即反映し、同じ通知を通常 FIFO にも残す。
  これにより tool Line の受付と Active 作成の間の取消も失わない。取消は idempotent にする。
  Active は Executing/ReplyQueued の型付き phase で保持し、queue 内にある応答も取消可能にする。
  writer は JSON serialize/文字数上限処理後、短い lock 内で取消確認と該当 Active の退役を確定し、
  直ちに1本の完全な JSON line を書く。書き始めた frame の途中を取消で切らない。
- **入力過負荷と EOF**: 通常の入力 queue 満杯は backpressure とし、受理済み要求・応答を捨てたり
  process を終了したりしない。ExitProcess hook は実際に stdin が EOF になった場合だけ呼ぶ。
  既に受信した取消と EOF の制御は stdout write/flush を待たない。
- **deadline の検査**: test 専用の worker と manual clock を real Connection::request 経路へ注入する。
  2要求とも本番 setup と permit 取得を通し、deadline 部分応答後も worker 解放まで2枠が保持されること、
  第三要求が Busy になること、実 controller join 後に枠が戻ることを検査する。
- **初期化中 ping**: legacy の ping は Ready 判定より先に空の result を返す。
  [2025-11-25 lifecycle](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) に従う。
  版メタ情報付きの要求は通常の版検証へ通し、未対応版の拒否を迂回しない。

状態を減らす検討: tool 処理を output write と一体化して modal にすると取消を受信できないため採用しない。
専用 writer と既存 Active の型付き phase に集約し、別の pending response registry や回復 loop は作らない。
入力満杯には通常の backpressure を使い、過負荷を EOF として扱う分岐を削除する。

独立 reviewer (gpt-6.1-sol / xhigh、read-only) は今回の所有・取消・queue・idle 通知・deadline の
差分を確認し、初期化中 ping の版メタ情報迂回を追加指摘した。両 legacy 版の回帰テストと条件修正後、
追加の P1/P2 はなし。実 pipe の full-trust 検証を source review で代替しない。

### Round 3 の sandbox 内検証 (実装担当実行、のちに `ee59d0cb9` として commit)

| コマンド | 実 exit code / 結果 | ログ |
|---|---|---|
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 | `target/mcp-r3-core-check-final.log` |
| `cargo check -p mimageviewer-mcp --bin mimageviewer-mcp` | 0 | `target/mcp-r3-mcp-check-final.log` |
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` | 101。IPC 6成功、MCP 27成功/2失敗、ignored 0 | `target/mcp-r3-crates-final.log` |
| 上記の core 焦点 nextest | 100。18実行、16成功/2失敗 | `target/mcp-r3-focused-final.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(=agent_ipc::tests::deadline_returns_partial_without_releasing_the_running_worker_slot)'` | 0。1/1成功 | `target/mcp-r3-deadline-test.log` |
| `cargo fmt` → `cargo fmt --check` | 0。全 workspace | `target/mcp-r3-fmt-check.log` / tool 実行記録 |
| `python scripts/check_ui_glyphs.py` / `git diff --check` | 各0 | tool 実行記録 |

上表の crate 失敗は次の実 pipe 2件で、いずれも fixture thread 起動前の client open が OS 5。

- `pipe::tests::real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld`
- `pipe::tests::real_pipe_cancel_reaches_core_while_stdio_stdout_is_blocked` (今回追加)

core 焦点 nextest の失敗は、前 round と同じ次の2件。idle reader は client open、stdio 結合検査は
fixture pipe の作成で OS 5 となり、adapter の起動には到達していない。新しい deadline 検査は成功した。

- `agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry`
- `agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers`

通常の入力100件の burst を stdin を閉じずに流し、両 queue 満杯から全応答が完全な JSON line で戻ること、
続く101件目にも応答することを検査した。writer が停止中の取消・EOF、writer エラー時の queue 解放、
応答 queue 中の取消、受付前の取消 FIFO、古い権限 poll の破棄、idle 中の権限通知も成功した。
実 pipe の新しい取消検査は、stdout を止めたまま fixture が実際の Cancel frame を受け取ることを要求する。
DACL・integrity level の緩和、skip/ignore、製品 core/launcher 起動、通常 APPDATA 使用、snapshot 更新、
test-full、commit は行っていない。この実行時点では full-trust 結果は未受領だった。
その後 ClaudeCode から受領した結果は下記の round 3 sandbox 外検証を参照。
実 pipe gate が未成立のため `build-dev.ps1` は実行していない。

### Round 3 の sandbox 外再検証コマンド (ClaudeCode 担当)

```powershell
Set-Location G:\home\mimageviewer-mcp
cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored
cargo nextest run -p mimageviewer --lib -E 'test(/agent_ipc/) or test(/agent_level/) or test(/remote_ipc::live_favorites/) or test(/settings_transfer::tests::automatic_network_enablement/) or test(=remote_ipc::pipe::tests::remote_pipe_dacl_has_one_current_user_ace)'
```

通常ユーザーの full-trust context で上記全体を実行し、新規 blocked-stdout 実 Cancel を含む4件の
実 pipe 経路も確認する。core/launcher の製品バイナリは起動しない。stdio 結合検査は許可済みの
MCP adapter と使い捨て fixture のみを起動する。

### Round 3 の sandbox 外検証 (ClaudeCode 実行、基点 `ee59d0cb9`)

以下は ClaudeCode が full-trust context で実行し、round 4 の依頼で報告した結果。
実装担当による sandbox 内の観測ではない。

- `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored`
  → **exit 0、IPC 6件 + MCP 29件成功**。
- 上記の core 焦点 nextest → **exit 100、16成功/2失敗**。
  `tags_use_the_projection_and_real_rating_database` と
  `tag_and_collection_pages_continue_past_200_and_validate_cursors` が
  `create_collection(...).recv().unwrap()` で `Err(Starting)` となった。
- `cargo nextest run -p mimageviewer --lib -E 'test(/agent_ipc::tests::tag/)'` を3回実行すると、
  **1/2成功、1/2成功、0/2成功**。各回の exit code は報告に含まれていないため記録しない。
  actor 起動完了前に fixture が request を発行するタイミング依存の失敗だった。

### Round 4 の修正

- 共通 `Fixture::new` は actor の `CollectionRuntimeEvent::Ready` を受信してから利用を開始する。
  runtime のイベント channel に test 専用の bounded-time 受信口を設け、sleep・poll・再試行は使わない。
  `Ready` は受付状態を Running に変更した後に送られるため、タグ・collection ページ・stdio 結合検査の
  fixture が actor 準備前に書き込み要求を出す組み合わせを除ける。
- 製品の `list_collections` は `list_catalog` / `load_collection` の受付・応答における
  `CollectionStoreError::Starting` を `NotReady` / 「コレクションを準備しています…」へ変換する。
  その他の actor error と channel 切断は従来の Internal を維持する。Starting と永続化失敗の
  mapping を検査する unit test を追加した。
- 待機口は `cfg(test)` のみ。製品側に同期待機・再試行や UI thread の変更を加えていない。

### Round 4 の sandbox 内検証 (実装担当実行、のちに `0695e9cc4` として commit)

| コマンド | 実 exit code / 結果 | ログ |
|---|---|---|
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 | `target/mcp-r4-core-check.log` |
| `cargo check -p mimageviewer-mcp --bin mimageviewer-mcp` | 0 | `target/mcp-r4-mcp-check.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/agent_ipc::tests::tag/) or test(=agent_ipc::tests::collection_starting_maps_to_not_ready)'` | 0。3/3成功 | `target/mcp-r4-targeted.log` |
| 上記の core 焦点 nextest | 100。19実行、17成功/2失敗 | `target/mcp-r4-focused.log` |
| `cargo fmt` → `cargo fmt --check` | 各0。全 workspace | `target/mcp-r4-fmt-check.log` / tool 実行記録 |
| `python scripts/check_ui_glyphs.py` / `git diff --check` | 各0 | tool 実行記録 |

MCP / agent-ipc crate は今回変更していないため、上記の ClaudeCode full-trust 実行の
exit 0 (6 + 29件成功) を再利用し、sandbox 内では crate tests を再実行していない。

core 焦点 nextest でも報告されたタグ2件と新規 mapping 検査は成功。失敗は次の既知の
restricted-token 環境の実 pipe 2件で、いずれも OS 5。stdio 結合検査は fixture pipe 作成時の拒否で、
adapter 起動まで到達していない。skip/ignore や DACL の緩和は行っていない。

- `agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry`
- `agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers`

`build-dev.ps1` は実 pipe gate 未成立のため実行していない。製品 core/launcher の起動、通常 APPDATA
での実行、snapshot 更新、test-full、commit も行っていない。新たな設計矛盾はない。

### Round 4 の sandbox 外再検証コマンド (ClaudeCode 担当)

```powershell
Set-Location G:\home\mimageviewer-mcp
cargo nextest run -p mimageviewer --lib -E 'test(/agent_ipc/) or test(/agent_level/) or test(/remote_ipc::live_favorites/) or test(/settings_transfer::tests::automatic_network_enablement/) or test(=remote_ipc::pipe::tests::remote_pipe_dacl_has_one_current_user_ace)'
```

19件全体を通常ユーザーの full-trust context で実行する。報告されたタグ2件、新規 mapping、
idle reader shutdown、実 core handler を使う fixture への stdio adapter 接続を含む。
今回変更のない crate tests は ClaudeCode の round 3 full-trust 成功を再利用できる。

### Round 4 の sandbox 外検証と追加レビュー (ClaudeCode、基点 `0695e9cc4`)

- ClaudeCode 実行: 焦点 core nextest (上のコマンド) を 5 回連続で実行し、毎回 19/19 成功 (exit 0)。
- 実装レビュー担当 (gpt-6.1-sol / xhigh、thread `01a18ce7`) の追加確認: P2-1・P3-4・P3-5 は修正確認、
  P2-2・P2-3 は通常条件で修正確認。cfg(test) の待機口は製品ビルドへ漏れない。
- 残る指摘 [P2]: 入力 queue と出力 queue が両方満杯のまま AI アプリが stdout を読まなくなると、
  stdin reader が止まり、その後の取消・stdin 切断に気づけない。
  **判断: 既定の割り切り方針による (CLAUDE.md「まれな失敗は既定で割り切る」)**。成立には
  AI アプリが stdout を読まないまま 32 件超の要求を送り続ける必要があり、通常の使い方では
  起きない (AI アプリが止まった状態)。その場合も AI アプリは終了時に子プロセスを終了させる
  (Claude Code は Windows で stdin を閉じた 300 ms 後に process tree を終了)。利用者のデータは失われない。
  機構は足さない。
- legacy 接続の 1 秒ごとの Status 照会は、待機中は core 接続・permit を保持せず UI を起こさない
  (レビュー確認)。tool 実行中も照会する点と、有効時に 1 秒 2 要求分の worker 処理がある点は
  段階 1 で push 通知へ置き換えるか再検討する。

### Round 5 の実機観測 (ClaudeCode 実行、full-trust、製品 build 基点 `0695e9cc4`)

以下は ClaudeCode が disposable portable mIV と隔離 data dir
`G:\home\mimageviewer-mcp\target\portable-smoke\data` を使い、agent_level=read_only で観測した結果。
実装担当による製品起動・観測ではない。

- `target\debug\mimageviewer-mcp.exe --data-dir <上記の隔離 data>` へ
  initialize(2025-06-18) / notifications/initialized / tools/list(id=2) /
  tools/call miv_status(id=3) を連続送信すると、initialize 結果、id=3 の adapter Busy、id=2 の
  tools/list 結果の順で返った。同一クライアントの先行要求を Active 判定で拒否していた。
- 一件ずつ送信した miv_status / list_tags / items_by_tag は seed データを正しく返した。
  tools/list は5 tool すべてを readOnlyHint=true で列挙した。
- `claude -p --mcp-config ... --strict-mcp-config` は server を connected と報告した。
  model call は CLI login の期限切れで未実行。この認証状態を製品の不具合とは扱わない。

### Round 5 の修正設計

- client の入力は深さ32の単一 FIFO に保持し、Active がなくなってから次を実行する。
  追加の待機 registry や並列 core worker 群は作らない。通常の pipeline に対する adapter Busy を削除し、
  core の応答として届いた Busy はそのまま返す。既存の permission observer は変更しない。
- raw line を control event channel から別の待機列へ積み直さず、同じ入力 FIFO が waiting request を所有する。
  満杯は Condvar の backpressure。InputAvailable event は有界 channel 上の wake のみとし、
  main loop は recv 前に必ず FIFO を確認する。writer の終了/取消 skip も非blocking wake を送る。
- dequeue と Active への登録は同じ queue lock 内で行う。stdin owner の取消も queue→Active の順で
  同じ lock を使い、待機要求の除去と実行中要求の取消を処理する。引き渡し途中の取消を失わない。
  queue lock 内では stdout 書き込みや core IPC を待たない。
- 待機中の取消は core を呼ばず、応答も返さない。
  legacy も [2025-11-25 cancellation](https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/cancellation)
  の取消後は応答しない指針に従う。initialize は取消対象にしない。
- 重複 request ID は Active と最大32件の待機列を同じ lock 内で調べ、FIFO 内の型付き rejection とする。
  通常受付での重複を拒否する。stdout 待ちの窓の扱いは round 6 記録を参照。
  shutdown は FIFO を Closed にして待機要求を破棄し、
  空きを待つ stdin owner も起こす。EOF は従来どおり child の終了を担当する。

状態を減らす検討: 同じ adapter から複数の core request を並行実行せず、一つの FIFO と既存 Active に
集約する。入力/出力両 queue 飽和に関する round 4 の既定の割り切り判断は変更しない。

独立 reviewer (gpt-6.1-sol / xhigh、read-only) は設計と完成差分を確認した。
入力 error 検出時にも FIFO を先に閉じる点を追加指摘し、oversized line / read error の両経路を
close→Active cancel→InputError event に揃えて修正した。これらは EOF hook を呼ばない。
最終 source review で追加の P1/P2 はなし。

### Round 5 の sandbox 内検証 (実装担当実行、のちに `f325b33a2` として commit)

| コマンド | 実 exit code / 結果 | ログ |
|---|---|---|
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 | `target/mcp-r5-core-check.log` |
| `cargo check -p mimageviewer-mcp --bin mimageviewer-mcp` | 0 | `target/mcp-r5-mcp-check-final.log` |
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` | 101。IPC 6成功、MCP 34成功/2失敗、ignored 0 | `target/mcp-r5-crates-final.log` |
| 上記の core 焦点 nextest | 100。19実行、17成功/2失敗 | `target/mcp-r5-focused.log` |
| `cargo build -p mimageviewer-mcp --bin mimageviewer-mcp` | 0。`target/debug/mimageviewer-mcp.exe` 更新 | `target/mcp-r5-adapter-build.log` |
| `cargo fmt` → `cargo fmt --check` | 各0。全 workspace | `target/mcp-r5-fmt-check.log` / tool 実行記録 |
| `python scripts/check_ui_glyphs.py` / `git diff --check` | 各0 | tool 実行記録 |

3版の full stdio runtime に initialize/initialized (legacy のみ) と list/call/call を一括投入し、
後続の core call が先行 query の解放まで呼ばれないことと応答順を検査した。
legacy の既存 Status observer は query の同時実行数計測から除外し、時刻や1秒未満の実行速度に依存しない。
modern では先頭 tools/list の Status 自体を channel で止め、3要求すべての backend 呼び出しが最大1件で
順番に成功することを検査した。待機中の取消も3版で core call・応答とも出ないことを確認した。
深さ32での背圧と空き/終了による再開、queue→Active 引き渡し中の取消、initialize 取消除外、
重複 ID、実 core Busy の tool error 保持も成功した。

失敗は次の実 pipe 4件で、いずれも既知の restricted-token 環境の OS 5。

- MCP `pipe::tests::real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld`
- MCP `pipe::tests::real_pipe_cancel_reaches_core_while_stdio_stdout_is_blocked`
- core `agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry`
- core `agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers`

stdio exe 結合検査は fixture pipe 作成で拒否され、adapter 起動まで到達しなかった。
通常権限での新 adapter を使った結合検査は下記コマンドで確認する。
skip/ignore、DACL 緩和、製品 core/launcher 起動、通常 APPDATA 使用、snapshot 更新、test-full、
commit は行っていない。製品 core は今回変更しておらず `build-dev.ps1` は未実施。
console adapter は build 済み。今回の full-trust 結果は未受領。

### Round 5 の sandbox 外再検証コマンド (ClaudeCode 担当)

```powershell
Set-Location G:\home\mimageviewer-mcp
cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored
cargo nextest run -p mimageviewer --lib -E 'test(/agent_ipc/) or test(/agent_level/) or test(/remote_ipc::live_favorites/) or test(/settings_transfer::tests::automatic_network_enablement/) or test(=remote_ipc::pipe::tests::remote_pipe_dacl_has_one_current_user_ace)'
```

前者は IPC 6件 + MCP 36件、後者は core 19件を通常ユーザーの full-trust context で実行する。
新 FIFO の回帰と、実 core handler を使う使い捨て fixture への stdio adapter 接続を含む。

### Round 6 の追加レビューと判断 (実装レビュー担当、thread `01a18ce7`)

以下は実装 reviewer (gpt-6.1-sol / xhigh、thread `01a18ce7`) が round 5 を確認し、
ClaudeCode が round 6 の依頼で伝達した指摘と設計担当の判断。

1. **[P2] ping が長い tool の後ろで待つ**: 受理して修正する。core 要求の直列化へ ping まで含めたため、
   legacy の疎通確認が tool 完了まで返らなくなっていた。
2. **[P3] stdout 待ち中の重複 ID の窓**: **既定の割り切り方針による**。
   client が in-flight の ID を再利用して JSON-RPC の規則に違反する必要があり、通常の利用では起きない。
   追加の ID registry や待機・復旧の機構は作らない。既存の通常受付での重複検査は維持する。

### Round 6 の修正

- ping を local control request として core FIFO から分離する。同じ深さ32の入力 queue 内で ping を
  先に取り出せるようにし、tool が Active の間も既存の protocol handler へ渡す。
  core request 同士の順序、queue→Active の lock 境界、取消・backpressure の所有は変えない。
- 対応する legacy 2版は空の result を即座に返す。版・params・request ID の検証は既存 handler に集約し、
  modern に未対応の ping を追加しない。既存の modern Method not found と版検証も維持する。
- held search と待機 collection の後から ping を送り、search を解放する前に legacy の空 result が
  返ることを検査する。解放後は search→collection の順で応答し、core query の最大同時実行数は1件。
  modern でも保留 tool 中に未対応 ping の error を返すことを確認する。

状態を減らす検討: 新しい control thread や core worker、別の無制限 queue は作らず、
既存の bounded input と一つの protocol handler を使う。両 queue 飽和の既決判断も変更しない。

今回の限定差分は独立 reviewer (gpt-6.1-sol / xhigh、read-only) でも確認し、追加 P1/P2 はなし。
上記 thread `01a18ce7` による round 5 の指摘記録とは区別する。

### Round 6 の sandbox 内検証 (実装担当実行、未コミット差分)

| コマンド | 実 exit code / 結果 | ログ |
|---|---|---|
| `cargo test -p mimageviewer-mcp --lib tests::ping_bypasses_held_tool_without_reordering_core_requests -- --exact` | 0。1/1成功 (3版を検査) | `target/mcp-r6-ping-test.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 | `target/mcp-r6-core-check.log` |
| `cargo check -p mimageviewer-mcp --bin mimageviewer-mcp` | 0 | `target/mcp-r6-mcp-check.log` |
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` | 101。IPC 6成功、MCP 35成功/2失敗、ignored 0 | `target/mcp-r6-crates.log` |
| 上記の core 焦点 nextest | 100。19実行、17成功/2失敗 | `target/mcp-r6-focused.log` |
| `cargo build -p mimageviewer-mcp --bin mimageviewer-mcp` | 0。`target/debug/mimageviewer-mcp.exe` 更新 | `target/mcp-r6-adapter-build.log` |
| `cargo fmt` → `cargo fmt --check` | 各0。全 workspace | `target/mcp-r6-fmt-check.log` / tool 実行記録 |
| `python scripts/check_ui_glyphs.py` / `git diff --check` | 各0 | tool 実行記録 |

crate tests と焦点 nextest の失敗は round 5 と同じ実 pipe 4件で、いずれも既知の restricted-token
環境の OS 5。stdio exe 結合検査は fixture pipe 作成時に拒否され、adapter 起動まで到達していない。
ping 回帰と既存の初期化中 ping・版検証、FIFO・取消・背圧は成功した。
skip/ignore や DACL 緩和、重複 ID の追加追跡機構は入れていない。
製品 core/launcher の起動、通常 APPDATA 使用、snapshot 更新、test-full、commit は行っていない。
製品 core は今回変更しておらず `build-dev.ps1` は未実施。console adapter は build 済み。

### Round 6 の sandbox 外再検証コマンド (ClaudeCode 担当)

```powershell
Set-Location G:\home\mimageviewer-mcp
cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored
cargo nextest run -p mimageviewer --lib -E 'test(/agent_ipc/) or test(/agent_level/) or test(/remote_ipc::live_favorites/) or test(/settings_transfer::tests::automatic_network_enablement/) or test(=remote_ipc::pipe::tests::remote_pipe_dacl_has_one_current_user_ace)'
```

前者は IPC 6件 + MCP 37件、後者は core 19件。今回の ping 回帰と、実 core handler を使う
使い捨て fixture への stdio adapter 接続を通常ユーザーの full-trust context で確認する。

### クライアントからの呼び出し観測 (2026-10-31、製品 build 基点 `bcbbeb352`)

- **Codex CLI 0.166.0 (gpt-6.1-sol、ChatGPT Windows アプリと同じ Codex エンジン)、観測者 ClaudeCode**:
  `~/.codex/config.toml` は変えず、`codex exec -c mcp_servers.mivtrial.command=... -c mcp_servers.mivtrial.args=[--data-dir, <隔離 data>]`
  で disposable portable mIV へ接続した。「タグ一覧と cat の付いた画像」を頼むと、モデルが
  list_tags と items_by_tag を並行して呼び、最終回答は seed どおり (cat 2・dog 1・indoor 1・outdoor 2、
  cat_garden.png / cat_sofa.png)。
  最初の 1 回は両 tool とも `-32602 Invalid limit or cursor` で失敗し、モデルが引数を変えて再試行して成功した。
  `codex exec --json` で再実行すると、両 tool とも `limit: 1000` (上限 200) を渡していた。
  段階 1 で、範囲外の limit を上限へ丸めるか、エラー文に許容範囲 (1〜200) を書く。
- **ChatGPT Windows アプリ (OpenAI.Codex 26.1027.3384.0)、観測者 利用者**: `target\mcp-trial\chatgpt-trial.ps1 add` で `~/.codex/config.toml` に `mimageviewer-trial` を足して起動し、同じ依頼をした。回答はタグ 4 種と件数、cat の 2 枚 (cat_garden.png = cat・outdoor、cat_sofa.png = cat・indoor) で seed と一致した。
- **Claude Desktop (Squirrel 版、`app-4.4212.0`)、観測者 利用者**: `target\mcp-trial\claude-trial.ps1 add` で `%APPDATA%\Claude\claude_desktop_config.json` に `mimageviewer-trial` を足して再起動し、Chat で同じ依頼をした。回答はタグ 4 種と件数、cat の 2 枚 (cat_garden.png = outdoor・評価 0、cat_sofa.png = indoor・評価 0) で seed と一致した。段階 0 の完了条件 (§9.3) を満たした。

## 15. 段階 1 の詳細設計 (2026-10-31、独立レビュー 3 巡を反映)

段階 0 (`mcp-trial` の `bcbbeb352` 以降) の上に足す。利用者決定は §11.2 (U8〜U10)。
コードの経路は 2026-10-31 に `mcp-trial` で確認した (file:line は実装 brief に書く)。
独立レビュー 3 巡 (§15.12〜§15.14) の指摘を反映済み。

### 15.1 区切り

1 回の実装・レビューの単位を小さくするため、次の 5 つに分け、この順で進める。各区切りで
`.\scripts\mcp-client-smoke.ps1` による確認と独立レビューを通す。

| 区切り | 内容 | 新しく借りる所有者 |
|---|---|---|
| 1a | 権限 snapshot の拡張 (§15.2)、読み取り tool の拡充 (§15.3)、出力予算 (§15.4)、段階 0 の残り (§15.9) | なし (共有 reader の束と collection actor の借用だけ) |
| 1b | フォルダ・ZIP/PDF の中の一覧と縮小画像 (§15.5) | 本の構成の service の snapshot (lock だけ)。一覧画面の純粋な関数を共有。縮小画像は catalog を使わず 1 枚から直接作る。Remote の engine は使わない |
| 1c | mIV で開く (§15.6) | agent の「開く」専用の受付 (App) + 二重起動と共有する判定・resolver の入口 |
| 1d | 設定画面・見せる範囲の表示・AI アプリへの登録 (§15.7) | なし (UI と設定) |
| 1e | 配布の仕組み (launcher `--mcp`、portable、ビルド・署名・同梱) (§15.8) | なし |

公開文書 (§6) と製品への同梱 (リリース) は段階 2 と一緒に行う (U10)。

### 15.2 許可の範囲と権限 snapshot (§5.2 の改訂)

U8 で 1 段足して 6 段にする。順序のある 1 つの選択のまま (上の段は下の段をすべて含む)。

1. 無効 (既定)
2. 読み取りのみ
3. **読み取りと、mIV で開く** (段階 1c)
4. 整理まで: タグ・★・コレクションの変更 (段階 2)
5. 移動・名前変更まで (段階 2b)
6. ごみ箱への削除まで (段階 2b)

- 設定画面には実装済みの段だけを出す (段階 1 では 1〜3)。設定値は未リリース (2026-10-31 時点で
  `v4.10.1` までのタグに `agent_level` は無い、独立レビューで確認) なので移行は要らない。
  `AgentLevel` に `ReadAndOpen` を足し、保存値は `"read_and_open"`。
- 縮小画像 (既定 OFF) と撮影位置 (既定 OFF) のチェックは段とは別に置く (U2・U6)。
- **権限 snapshot**: 段・縮小画像・撮影位置の 3 つを 1 つの値として読み、どれが変わっても epoch を進める。
  段階 0 の「受付ごとの判定」と「送信直前の再照合」はこの snapshot 全体で行う。生成中に縮小画像や
  撮影位置を OFF にしたら、その結果は送らず拒否に置き換える (独立レビュー P1-1)。
- 旧版の握手の通知監視は、有効/無効の bool ではなく「その snapshot で出す tool の集合」を比べ、
  変わったら `notifications/tools/list_changed` を送る。
- 設定の自動転送では、新しい 2 つのチェックも `agent_level` と同じく移行対象外にする。

### 15.3 読み取り tool の拡充 (1a)

tool の数を増やしすぎない (AI アプリの文脈を圧迫し、選び間違いも増える)。似た一覧は `kind` 引数で 1 つにまとめる。

| tool | 内容 | 本体側の経路 |
|---|---|---|
| `get_collection` | コレクションの中身 (`id` か名前)。並びは一覧画面と同じ | collection actor の `load_collection` + `prepare_collection_snapshot_while` (全項目の stat を伴うが画像の decode はしない)。Remote の view cache (`PersistentCollectionEngine::snapshot`) は使わない |
| `list_library` | `kind` = `favorites` / `reading_history` / `bookshelf` / `bookmarks` / `rated` | 下の表 |
| `get_item_info` | 1 回 50 件まで。名前・種類・大きさ・更新日時・寸法・★・タグ・撮影情報・AI 生成情報 | 列ごとの専用の読み取り (下) |

`list_library` の各 kind (独立レビュー P2-4):

| kind | 経路 | 注意 |
|---|---|---|
| `favorites` | `SharedReaders::favorites()` | — |
| `reading_history` | `ReadingHistoryDb::open_readonly().list_recent` | — |
| `bookshelf` | **本棚の直下の本の名前と path だけを列挙する新しい関数** | 既存の `books::list_books` は本ごとに `book_page_count` を呼ぶので使わない。ページ数は返さない |
| `bookmarks` | **ブックマーク DB の行の投影** (path・ページ・名前・登録日時) | 既存の `bookmark_browser::build_rows_readonly_*` は全件の存在確認・metadata 取得・保存画像の decode をし、取消もできないので使わない |
| `rated` | `RatingDb` の読み取り。`stars` **以上**を返す | 既存の `list_by_stars` は完全一致。並びは「★の多い順 → 付けた日時の新しい順 → path」で固定 |

`get_item_info` は `app::metadata_ops::run_metadata_load` を**使わない** (独立レビュー P2-5。XMP で
ファイル全体を読み、ZIP の中の画像を全展開し、大きさ・寸法を揃える関数でもない)。列ごとに必要な読み取りだけをする:

| 列 | 経路 |
|---|---|
| 名前・種類・大きさ・更新日時 | `std::fs::metadata` (ZIP/PDF の中の項目は container の情報) |
| 寸法 | 画像の header を読む既存関数 (`file_image_dims`。RAW は既存の `raw_decoder::info`) |
| ★・タグ | `RatingDb` / `TagsDb` の読み取り (段階 0 の `enrich` と同じ) |
| 撮影情報 | `exif_reader::read_exif` (JPEG は先頭の範囲だけ読む) を下の許可表で投影 |
| AI 生成情報 | `png_metadata::extract_metadata` を下の許可表で投影 |
| XMP・外部 sidecar | 段階 1 では読まない |

項目の種類ごとの取得先 (独立レビュー 2 巡目 P2-3)。該当しない列は「無し」と明示して返す (空と区別する):

| 列 | 実ファイル | ZIP の中の項目 | PDF のページ |
|---|---|---|---|
| 大きさ・更新日時 | そのファイル | 項目の展開後の大きさ。更新日時は ZIP の項目の値。ZIP ファイル自体の値は `container_size` / `container_modified` として別の名前で返す | ページには無し。PDF ファイル自体の値を `container_*` で返す |
| 寸法 | header を読む既存関数 | 項目の bytes (既存の ZIP 項目の読み取り) の header から | PDF のページ寸法の既存経路 (実装 brief でコードから決める) |
| 撮影情報 | `read_exif` | `read_exif_from_bytes` | 無し |
| AI 生成情報 | `extract_metadata` | `extract_metadata_from_bytes` | 無し |
| ★ | 一覧画面と同じ key (path) | 一覧画面と同じ key (ZIP の項目の key) | 一覧画面と同じ key (ページの key) |
| タグ | そのファイルのタグ | 無し (UI もタグの対象にしていない)。ZIP ファイル自体のタグは `container_tags` | 無し。PDF ファイル自体のタグは `container_tags` |

- ZIP の項目の bytes は 1 件 64 MiB を上限に読む。超えたら寸法・撮影情報・AI 生成情報を「大きすぎて読まない」とする。

- 項目の指定は path と任意の subresource (ZIP の中の項目・PDF のページ)。core はそれを検証してから
  内部の項目 (`GridItem`) を作る。規則は実装 brief で既存の `GridItem` 生成経路から決める。
- **送出列の許可表** (独立レビュー P2-2): core は既知の一般項目だけを投影し、それ以外は送らない。
  - 撮影情報: 機種・メーカー・レンズ・撮影日時・露出時間・F 値・ISO・焦点距離・向き。
    撮影位置 (緯度・経度・高度) は撮影位置の設定が ON の時だけ。`UserComment`・MakerNote・未知のタグは送らない。
  - AI 生成情報: 生成ツール名・モデル名・プロンプト・ネガティブプロンプト・主な生成条件 (seed・steps・
    sampler・CFG・寸法)。生の JSON (`raw` / `prompt_json` / `workflow_json`)・未知のチャンク・生の
    `UserComment` は送らない。結果のキーは一般名にし、生成ツール固有の形式名は出さない (公開文書の規則)。
- 全 tool で `readOnlyHint=true`。

### 15.4 出力予算・期限・取消 (1a、全 tool 共通)

- **予算** (独立レビュー P2-6、2 巡目 P2-4、3 巡目 P2-1)。**大きさは最終の MCP 形式で数える**: 同じ JSON を
  `structuredContent` (そのまま) と text (JSON 文字列として escape) の 2 か所に入れるので、ある JSON の
  「MCP での大きさ」= 直列化した長さ + それを JSON 文字列にした長さ。この計算は agent-ipc crate の 1 関数に置き、
  core (ページと cursor を確定する側) と MCP exe (確かめる側) が同じ関数を使う。core は cursor を含めたページの
  MCP での大きさ + envelope の予約分が 40,000 文字以内に収まる件数でページを切る。MCP exe で超過の error に
  なるのは計算の不一致 (不具合) だけにする。
  包含関係: 項目 ≤ ページ ≤ 非画像の最終 JSON − envelope の予約、非画像の最終 JSON + 画像の合計 ≤ agent IPC の frame。

  | 上限 | 値 | 数え方 |
  |---|---|---|
  | 項目 | 12,000 文字 | 1 項目の MCP での大きさ |
  | ページ (core) | 40,000 − envelope の予約 (2,000) 文字 | cursor・`truncated` を含むページ全体の MCP での大きさ |
  | 非画像の最終 JSON (MCP exe) | 40,000 文字 | `structuredContent`・text・escape・envelope を含む全体。画像 block の data は除く |
  | 画像 1 件 | 256 KiB | WebP の bytes |
  | 画像の合計 | 1 MiB | WebP の bytes の合計 (base64 で約 1.4 MB) |
  | agent IPC の frame | 4 MiB | 段階 0 の 1 MiB から引き上げ |

  - 切ってよいのは説明用の文字列 (プロンプト・ネガティブプロンプト・生成条件の値・コレクションの説明) だけ。
    path・subresource・id・cursor は切らない。切ったフィールドには切ったことを示す。
  - 項目が 12,000 文字に収まるよう説明用の文字列を切ってなお超える場合 (極端に長い path 等) は、その項目を
    「長すぎて返せない」の印と識別だけで返し、cursor は進める。項目 ≤ ページなので、項目があるページは必ず 1 件以上返す。
  - 画像 1 件が 256 KiB を超えたら、その項目を「縮小画像が大きすぎる」として画像なしで返し、cursor は進める。
    0 件で進まない cursor は返さない。
  - 続きは実際に返した件数で cursor を作る。
- cursor は大きな引数 (50 件の path 等) を中に複製せず、正規化した引数の digest で照合する
  (cursor の 8,192 文字上限に収まり、exe 側に状態を持たない)。
- 期限と取消: 段階 0 では 20 秒の応答期限は検索だけだった。**全 tool に 20 秒の応答期限を付け、期限に達したら
  worker へ協調取消を出す**。取消の後も worker が実際に止まるまで同時実行枠を保持する。フォルダ走査など取消を
  受け取らない既存関数は、取消を受ける形にしてから使う (1b)。
- 期限に達した時の結果 (独立レビュー 2 巡目 P2-5):
  - 正しい順序の先頭部分 (prefix) が確定している tool (検索、`get_item_info` のように指定順に 1 件ずつ作るもの) は、
    確定済みの部分 + 進む cursor + `truncated` を返す。
  - ただし、期限までに cursor の位置より後の新しい項目が 1 件もできなかったら、空のページではなく時間切れの
    tool error を返す (独立レビュー 3 巡目 P2-3。進まない cursor の成功を返さない)。
  - `get_item_info` の続きは、cursor の位置 (入力の配列の何件目か) から読み始め、既に返した項目の重い読み取りを
    繰り返さない。検索のように再実行して飛ばす形にしない。
  - 全件を揃えてから順序が決まる tool (`get_collection` の並べ替え、`list_folder`、`list_library` の並べ替えを伴う kind) は、
    ページが完成する前に期限が来たら「時間切れ。もう一度試すか、件数を減らしてください」の tool error を返す。途中の行は返さない。
  - 受理済みの「mIV で開く」は期限・取消の対象外 (§15.6)。
- **結果の型を 2 つに分ける** (独立レビュー 2 巡目 P2-2): 読み取りの結果は送信直前に権限 snapshot を再照合し、
  変わっていれば拒否に置き換える (§15.2)。受理済みの操作 (段階 1 では「mIV で開く」、段階 2 では書き込み) の結果は、
  受理の前に権限を照合し、**受理の後は権限の変更・取消で失敗に置き換えない** (§4.3 の「受理済みの書き込みは
  完了させて結果を返す」と同じ原則)。

### 15.5 フォルダ・ZIP/PDF の一覧と縮小画像 (1b)

2026-10-31 にコードを調べ直し、1 巡目 P2-3 の「Remote の container を分けて使う」方針を改めた (理由は下の「所有者の方針」)。

| tool | 内容 |
|---|---|
| `list_folder` | フォルダ・ZIP・PDF の中を、一覧画面と同じ規則 (表示する種類・並び順・重複の畳み方・本の構成の順序と非表示) で返す |
| `get_thumbnail` | 縮小画像 (WebP)。1 回 20 件まで、一辺は既定 256・最大 512。収まる件数だけ返し続きは cursor |

**所有者の方針**: Remote の `ContainerEngine` / `ThumbnailEngine` は使わず、分けもしない。Remote の一覧は
一覧画面と規則が違うため (分けて共有すると、その違いを agent にも持ち込む):

- 本の構成の範囲を、表示する種類で絞った後の木から決める (一覧画面は絞る前の木。`ZipNavLevel.raw`)。
  種類の設定で畳み方が変わると、別の範囲の構成を引く。
- ZIP の並びに一覧画面の並び順 (`grid_display_order`) を渡さず、ページを全部「在る」として扱う
  (一覧画面の adapter は「方針で除外」「絞り込みで除外」を分ける)。
- お気に入りごとの表示状態 (並び順等) を重ねない。
- ZIP/PDF の一覧で見開き調整の reader を必ず引き、Remote の session が無いと失敗する。

代わりに、**一覧画面が使う純粋な関数** (`book_materialization` の adapter、`folder_scan` の materializer、
`ZipTree`) を agent の worker から直接呼ぶ。どれも `pub(crate)` で UI の状態に触れない。Remote のコードは変えない。

`list_folder`:

- 指定: `path` (絶対 path) と、ZIP の中のフォルダを指す任意の `subresource` (末尾 `/` の相対 path。
  既存の `book_scope::validate_relative_reference` で検証)。
- 一覧できるもの: 実フォルダ、ZIP (cbz 含む)、PDF。ZIP の中の ZIP は、一覧画面と同じく木の中のフォルダとして
  扱い (列挙が入れ子を展開し、`collapse_redundant` も入れ子へ降りる)、`subresource` の prefix で中を指せる
  (実装担当の指摘による。例外の境界を作らず一覧画面の規則に揃える)。**RAR・7z 等の変換が要る書庫・EPUB の中は 1b では
  一覧しない** (変換・cache・password の組み合わせを持ち込まないため)。項目としては一覧に出し (`kind` で区別)、
  中を求められたら「この形式の中は一覧できません。mIV で開いてください」の tool error。
- 経路:

  | 対象 | 経路 |
  |---|---|
  | 実フォルダ | `scan_directory_with_settings` に取消を足した形 (既存の `scan_directory_with_visibility_cancel` と同じく途中で `Interrupted` を返す) → `materialize_local_folder_listing_with_order` |
  | ZIP | `enumerate_image_entries_detailed_readonly_with_cancel` (一覧画面と同じ列挙。拡張子の判定で Susie の pool の初期化を待つこと (1 回最大 5 秒、期限の後も worker の終了まで枠を保持する契約で扱う) と、入れ子の ZIP を展開することも一覧画面の規則のまま。取消は I/O の境界で受ける。**agent の呼び出しだけ入れ子の展開に上限を付ける** (9 巡目 P2): 入れ子 1 つ 64 MiB (1a の ZIP の項目と同じ)、同時に持つ合計 256 MiB。確保の前・読み取りの途中・nested cache に当たった時に確かめ、超えたら一覧全体を「大きすぎる入れ子の ZIP があります」の tool error にする (超えた枝を黙って省いた成功にしない)。一覧画面の呼び出しは上限なしのまま) → `PreparedZipEnumeration::prepare` → 一覧画面と同じ raw / 表示の prefix → `materialize_archive_level(..., BOOK_READING_PAGE_ORDER, Some(grid_display_order))` → `project_archive_composition` |
  | PDF | ページ数は catalog の `pdf_meta` を読み取り専用で引き、無ければ **agent 専用の列挙の入口** (`enumerate_pages_with_read_target` と同じ処理で、path を perf key に入れず、取消の待ち方は `HarvestOnCancel`。既存の入口は path 入りの perf key と `AbortOnCancel` で固定されていて、ログに path を出さないことと枠を worker の終了まで持つことを満たせない。実装担当が着手前に指摘、ClaudeCode が判断。既存の UI・Remote の入口は変えない) → `materialize_cached_document_rows` → 表示する種類の設定で `allows_physical_file(logical_path)` が false なら、項目と meta を空にしてから (一覧画面の `book_composition_runtime.rs` と同じ。10 巡目 P2) → `project_document_composition` |

- 設定: 検索と同じく、その都度 `load_into_settings` で読んだ現在の設定。お気に入りの中のフォルダなら、
  お気に入りごとの表示状態を DB から読み取り専用で読んで重ねる (一覧画面と同じ重ね方)。読むのは最後に保存された
  表示状態で、UI は変更が止まってから 500 ms 後に保存を予約するので、変えた直後は古いことがある (同期の待ちはしない。
  独立レビュー 5 巡目 P3)。
  ファイル名 prefix スタックの畳み表示は UI の一時状態なので重ねない (畳まない一覧を返す)。
- 本の構成: `BookCompositionService` の `snapshot()` (lock だけ、I/O なし) から、一覧画面と同じ範囲の key で引く。
  agent server は service より前に起動するので、`install_agent_readers` で service の clone を共有 reader の束に入れる
  (FTS の handle と同じ遅れた設置)。未設置の間と、snapshot の `status` が正常でない間
  (service の初期 snapshot は `status=Err`・空の rows で始まる) は「本の構成を読み込み中です」の tool error。
  rows が空であることを「構成なし」と解釈するのは `status` が正常な時だけ (構成を黙って落とさない。独立レビュー 5 巡目 P2-4)。
  保存中の構成の変更は snapshot に入っていないことがある (読み取りの時点の確定値を返す。変更の完了は待たない)。
- 画像フォルダには本の構成が無い (一覧画面でも `ImageFolder` は対象外) ので、構成は ZIP/PDF だけに掛かる。
- **返すのは一覧画面の基礎の一覧** (独立レビュー 5 巡目 P2-5): 一覧画面がその後に掛ける検索・★の絞り込み・facet・
  色の絞り込み・ブックマーク / 閲覧履歴の表示の絞り込みは掛けない (どれも UI の一時状態)。tool の説明にもそう書く。
- **書き込まない**: 一覧画面の ZIP の worker が行う `zip_key_migration`、Remote が行う `pdf_meta` の保存は agent ではしない。
- 返す行: 項目の識別 (`path` + `subresource`)・名前・`kind` (`folder` / `image` / `video` / `audio` / `zip` / `pdf` /
  `epub` / `archive` / `zip_folder` / `pdf_page` / `zip_entry`)・大きさ・更新日時 (分かるものだけ)。
  本の構成で非表示のページは返さない。結果に `book_composition_applied` を付ける。
- ページ: 全件を作ってから並びが決まる tool (§15.4)。`limit` は既定 100・最大 200。cursor には正規化した引数の
  digest・位置と、**一覧全体の識別の列から作った digest** を入れる。続きを求められた時に作り直した一覧の digest が
  違えば「一覧が変わりました。最初から取り直してください」の tool error (途中でフォルダが変わった時に、
  抜けや重複を黙って返さない)。
- 期限: 走査・列挙・PDF のページ数は取消を受ける。materializer と構成の投影は CPU だけの処理で、その前後で
  取消を確かめる。期限が来たら §15.4 の「時間切れ」の tool error。

`get_thumbnail` (独立レビュー 6 巡目で組み直した。一覧画面の `process_load_request` は catalog・代表の解決・
類似索引の prefill・`pdf_meta` の更新・Susie などの副作用と取消を受けない読み取りを内側に持ち、agent 用に抑止する
箇所が巡ごとに増えたため、使わない):

- 「AI に縮小画像を見せる」が ON の時だけ `tools/list` に出し、OFF なら拒否する。
- **指定された 1 枚から直接作る。thumbnail cache (catalog) は読みも書きもしない。** 本の構成の編集画面の縮小画像
  (`app/book_composition_editor_thumbnails.rs` の `decode_thumbnail`。cache なし・取消付き・回転 DB を読み取り専用) と
  同じ考え方。これで代表の解決、ドライブの違う同名フォルダの cache の取り違え、cache の保存方針との整合、
  DB への書き込みが agent の経路から無くなる。
- 対象 (それ以外は `unsupported`):

  | 項目 | 読み方 |
  |---|---|
  | 画像ファイル (`folder_tree::SUPPORTED_EXTENSIONS` の静止画。拡張子の判定は Susie の pool を起こす `is_recognized_image_ext` ではなく一覧を直接見る) | 取消を確かめながら 128 MiB まで読み (超えたら `too_large`)、JPEG は `decode_jpeg_turbo_scaled_from_bytes` (DCT の縮小 decode)、他は `image_decode::primary` (取消付き・Susie を呼ばない。AVIF・WIC の fallback を含む)。closure は読み込み済みの bytes だけを decode し、Susie を呼ばないものに固定する。アニメーションは最初の 1 枚 |
  | RAW | `raw_decoder::preview_bounded` の埋め込み preview だけ。無ければ `unavailable` (現像はしない。RAW 現像 executor を使わない) |

  **decode の memory の上限** (7 巡目 P2): 入力の大きさの上限とは別に、decode 後の buffer を 256 MiB までにする
  (TurboJPEG の既存の出力上限と同じ値)。`image_decode::primary` の budget、closure の中の `ImageReader::limits`、
  `preview_bounded` の `max_alloc` にこの値を渡し、超えたら `too_large`。budget を省くと AVIF・WIC が実質無制限に
  なるので、省略しない。JPEG は先に DCT の縮小 decode (既存の関数が縮小後の割り当てを検査する) を試し、元の大きさの
  decode に進む時だけ下の見積もりを掛ける (1b 実装レビュー P2。12,000×8,000 の JPEG も縮小 decode なら数 MB)。
  bounded の WIC は上限超過も非対応も同じ失敗で返すので、**元の大きさの decode の前に header の寸法から
  buffer の大きさを見積もり**、上限を超えるものを `too_large` にする (1a の寸法の読み取りと同じ関数。ファイルと
  ZIP の項目の bytes の両方)。decode の失敗は、上限超過を含めて `unavailable` (新しい型付きの入口は作らない。
  実装担当の指摘による)。
  | ZIP (cbz 含む) の中の画像 | 1a の `read_zip` / `read_zip_member` (入れ子対応・64 MiB まで・取消付き) の bytes から上と同じ decode |
  | PDF のページ | 取消付きの `render_page` に**path を perf key に入れない入口**を足して使う (1a の `get_page_sizes_anonymous` と同じ形)。取消の待ち方は既存の `CancelWaitPolicy::HarvestOnCancel` (応答を待つだけで cache の保存は内蔵しない。用途のコメントを agent に広げる)。`AbortOnCancel` は早く return して同時実行の枠を先に離すので使わない。password 付きの PDF は password を渡さないので `unavailable` |
  | フォルダ・書庫のファイルの代表、動画・音声、Susie だけが読める形式、EPUB のページ、RAR の中 | `unsupported` (AI は `list_folder` で中の画像を指せる) |

- 向き: EXIF の向きを掛ける (TurboJPEG の経路は向きを掛けないので明示的に掛ける。RAW の preview は掛け済み)。
  利用者が mIV で付けた回転は `RotationDb::open_readonly` で読み、画像に焼き込む (Remote と同じ。`RotationDb::open()` は
  table を作るので使わない)。
- **編集の「隠す」**: その項目自身のページの key (実ファイルは path、ZIP の項目は `zip::entry`、PDF は `pdf::page_N`。
  既存の `edit_source` の key 関数) で `ConcealDb::open_readonly` の `get_full_checked` を引き、あれば画像を返さず
  `hidden_by_edit`。代表を解決しないので、返す画素と判定の key は常に同じ項目 (6 巡目 P1)。
  **ZIP の項目は旧名の key も照合する** (7 巡目 P1): v1.3.x までの CP932 名の ZIP では、隠す編集が文字化けした名前の
  key に残っていることがあり、移行 (`zip_key_migration`) は一覧画面で ZIP を開いた時にしか走らない (agent は移行しない)。
  よって ZIP の項目では、新旧両方の key で照合する。旧名は**全件の列挙をせず** (列挙は拡張子の判定で Susie の pool の
  初期化を待ち、対象以外の入れ子 ZIP も上限なしで展開する。8 巡目 P2)、対象の項目を読む 1a の `read_zip_index` /
  `read_zip_member` の同じ読み取りの中で、各段の今の名前と旧名 (`entry.name().replace('\\', "/")`。移行の
  `legacy_renames` と同じ求め方) を取り、入れ子の段ごとに新旧それぞれの prefix を連結して作る。key は
  `adjustment_db::zip_entry_key` に新旧の名前を渡して作る (`zip_key_migration` と同じ)。読み取りに失敗して
  対応が決められなければ画像を返さない (`unavailable`)。回転も同じく新しい key を優先して旧 key を引く。
  1a の `get_item_info` の ZIP の項目の★も同じ旧 key を引くよう 1b で揃える (読み取りだけ。移行はしない)。DB の file が無ければ
  「隠すなし」、file があって読めなければログ + その呼び出し全体を tool error にする (「隠すなし」に倒さない)。
  ほかの補正・編集は反映しない。1d の見せる範囲の説明に「補正・編集は反映しない。隠す編集をした画像は渡さない」と書く。
- **path の表記** (1b 実装レビュー P1): 編集の key は path の小文字化と区切りの置き換えだけで作るので、`.`・`..` を含む
  表記や短い名前 (8.3) で同じファイルを指すと、隠す編集の key と一致せずに未加工の画素を返し得る。よって agent の
  path を受ける全 tool で、**入口の 1 つの検査関数**が、一覧・検索が返すのと同じ通常の表記だけを受ける:
  絶対 path、NUL・`.`・`..` の成分なし、`\\?\` 等の接頭辞なし、`:` は drive の後だけ (ADS なし)、末尾の `.`・空白なし、
  それ以外は「パスは mIV の一覧と同じ表記で指定してください」の tool error。
  **短い名前 (8.3) は拒否しない** (1b 実装レビュー 2 巡目 P2): コレクションは登録時の短い名前を保存していることがあり、
  `get_collection` が返した path をそのまま渡せる必要がある。代わりに、隠す編集と回転は、渡された表記・その長い名前
  (`GetLongPathNameW`)・短い名前 (`GetShortPathNameW`) の 3 つの key (ZIP/PDF は container の path について同じ 3 つ。
  ZIP は新旧の項目名との組み合わせ) で照合し、どれかにあれば隠す (fail closed)。名前の解決が失敗した時は、存在しない
  path は各 tool の既存の扱い、それ以外は tool error。junction・symlink で別の名前を付けたものは一覧画面でも別の項目として扱われる (編集の key も別) ので、
  agent も同じ扱いにする (既存の制限)。
- 大きさと品質: 要求の一辺 (256〜512) に `catalog::encode_thumb_webp` (純粋な resize + WebP encode。透明は落ちる) で
  縮める。品質は設定の縮小画像の品質。
- cache を使わないので、毎回 decode する (JPEG は DCT の縮小 decode で軽い)。AI の用途では一覧画面ほどの速さは要らない。
- 予算 (§15.4): 画像は MCP の `content` の image block (`image/webp`) にだけ置き、text と `structuredContent` には
  項目の識別・状態 (`ok` / `too_large` / `unavailable` / `unsupported` / `hidden_by_edit`)・寸法だけを置く。1 件 256 KiB・合計 1 MiB。
  agent IPC では画像の bytes を別の欄で運ぶ (版 2 は未リリースなので型を足す)。
- 期限: 指定順に 1 件ずつ作るので prefix が確定する tool。期限に達したら確定済みの画像 + 進む cursor (入力の位置から
  再開)。1 件もできなければ時間切れの error。
- 共有するもの: 現在の設定の読み取り、プロセス共通の I/O の上限、PDF worker pool。分けるもの: 取消 (要求ごとの
  `Arc<AtomicBool>`)、要求の識別。agent 専用の cache は持たない。書き込みは無い。

所有と終了 (受け入れ条件):

- Remote と同時に使える: Remote の engine・session・cache を使わないので、片方の取消は他方に効かない。
- 終了順: 外側の owner は agent server → Remote → collection actor → 本の構成の owner の順に止める
  (`lib.rs` で確認済み)。agent server の停止は各 worker の終了を待つので、1b の重い読み取りはすべて取消で止まること。
- `App::on_exit` は agent の worker が動いている間に走り、本の構成の service の受付を閉じる (`close_admission`)。
  `snapshot()` はその後も読めるので、agent は `snapshot()` を使う (`current_candidate_snapshot()` は閉じた後
  `None` を返すので使わない)。

### 15.6 mIV で開く (1c)

2026-10-31 にコードで経路を確かめ、§15.6 の初版の方針 (二重起動と同じ受付・1 回限りの応答・保留しない) を具体化した。

- `open_in_viewer(path)`: 許可の範囲が 3 以上の時だけ出す。annotation は実際の性質のまま
  (`readOnlyHint=false`・`destructiveHint=false`・`idempotentHint=true`)。確認を出さないのは登録側の設定で行う (U9)。
- **指定は path だけ**。二重起動の経路は path しか運ばず (ZIP の項目・PDF のページを指す口が無い)、
  resolver は存在しない path を親のフォルダに読み替える。よって:
  - `subresource` は 1c では受けない (schema に置かない)。ZIP/PDF はファイルとして開く (一覧画面と同じく中を開く)。
  - 絶対 path・NUL なし・**存在すること**を worker で確かめる。存在しない path を親フォルダで「受け付けた」にしない。
  - 画像・動画・音声のファイルは、そのフォルダを開いて選択し全画面にする (二重起動と同じ)。それ以外の
    ファイルはそのファイルのあるフォルダを開く。tool の説明に書く。
- **受付の owner**: 二重起動の channel (`activation_open_path_tx`) には混ぜず、**agent の「開く」専用の
  受付**を App に持たせる。二重起動の channel は最後の 1 件だけを残して他を黙って捨て、モーダル中・Remote の
  phase 中は再送で保留するので、agent の要求ごとに応答を返せない。
  - 要求の状態は Remote の `UiRequestDispatch` (Pending → Claimed / Cancelled の CAS) と同じ形。今は private なので、
    約 30 行の型を `pub(crate)` の共通の型に取り出し、Remote と agent の両方が使う (Remote の挙動は変えない)。
    Remote session の UI queue そのものは、Local の phase では受付を閉じているので使わない。
  - 同時に処理する「開く」は全体で 1 件。2 件目は Busy で返す (未受理の agent 要求どうしの置き換えが無くなる)。
  - agent server は App より前に起動するので、App の作成時に受付の送り口と egui の `Context` を設置する
    (`install_agent_readers` と同じ遅れた設置)。未設置の間は「mIV の起動処理中です」。
- **worker 側** (UI へ送る前に分かる拒否): path の検証、Remote の phase が `Local` 以外。
  二重起動と同じく `ActivationOpenPath::received(path, session)` で受信時の Remote の状態を取る
  (`session` は App と同じ handle。Remote が起動していなければ両方 `None` で一致する)。
- **窓を前に出す**: 二重起動は別の activation event で窓を戻してから (tray・最小化から復帰) 前面にする。agent の
  要求でも、UI へ送る前に core の中から**同じ activation event を通知**し、既存の listener に復帰を任せる
  (窓が隠れたままだと UI の pass が回らず、受付まで届かないため。新しい復帰処理は作らない。5 巡目の回答 6)。
  前面化は OS が拒むことがあり、失敗はログだけ (二重起動と同じ)。MCP exe が要求の前に `AllowSetForegroundWindow`
  を呼ぶのは best effort (呼ぶ側に前面化の資格が要るので成功は保証されない。1d の実機確認で見る)。
  前面化の成否は応答に含めない。
- **UI thread の判定の位置** (独立レビュー 5 巡目 P2-3): 受理は `render_initial_open` の初期処理 (初期フォルダ・
  前回の一覧を開く) の**後**、`poll_activation_open_paths` の**直前**で行う。同じ frame の初期 open に置き換えられない
  ため。ただし `update_frame` は起動中の overlay の間 `render_initial_open` より前で return するので、`update_frame` の
  先頭 (その return より前) でも受付を見て、overlay 中または初期処理前 (`!initialized`) なら「mIV の起動処理中です」で
  拒否する (保留の状態は足さない)。
- **UI thread の判定** (受理の位置で、要求ごとに):
  1. Cancelled なら捨てる (期限切れで遅れて届いた要求は実行しない)。`try_claim` で Claimed にする。
  2. 許可の段を照合する (3 以上でなければ拒否)。open は縮小画像・撮影位置を使わないので、照合するのは段だけ。
  3. `poll_activation_open_paths` の判定 (Remote・本の構成 editor・文書を開くモーダル・sidecar 復元) を、
     理由を型で返す 1 つの関数に取り出し、二重起動と agent が共有する。二重起動はその理由で今と同じく
     捨てる / 保留し、agent は理由つきの拒否を返す。
  4. resolver の入口 (`start_startup_open_path_resolve`) に agent の source を足し、入口の判定の結果を返すようにする
     (`Admitted` / `Requeued` / `Refused(理由)`)。二重起動では `Requeued` (モーダル・EPUB の cache 削除中の再送) を
     今のまま行い、agent では再送せず「いまは開けません (理由)」を返す。
  5. 入口が `Admitted` を返した時点が**受理の確定点**。その時 1 度だけ「受け付けた」を返す
     (`ReplyPolicy::AcceptedOperation`。送信直前の権限の再照合で失敗に変えない)。
- 同じ frame の順序: agent の受付を先、二重起動を後に処理する。両方来たら利用者自身の二重起動が後から置き換える
  (agent 要求は受理済みなので追加の応答はしない)。
- **受理の後に分かる失敗は応答しない**: resolver は受理の後にも、Remote の取得による取消・RAR の続きの巻・
  開けない path などで拒否することがあり、mIV の画面に表示される (二重起動と同じ)。tool の結果の文は
  「mIV で開く処理を始めました」とし、表示の完了を意味しない。
- 応答の待ち: worker は 5 秒待ち、来なければ `cancel_pending` を試みる。取消に成功すれば「mIV が応答しません」、
  既に Claimed なら同じ frame の中で応答が来るのでそれを待つ (まれな失敗は既定の割り切り)。
- detached viewer: 述語・viewport 経路は変えない。resolver の中で agent の source は二重起動 (`Activation`) と
  同じ分岐を通り、bookmark 専用の detached の分岐は通らない。触れる関数は `startup_ops.rs` の入口の戻り値と
  source の追加、`poll_activation_open_paths` の判定の取り出しで、detached-rework-plan の憲法の対象
  (detached の述語・viewport の routing) には入らない。この判断も独立レビューで確かめる。

### 15.7 設定画面・見せる範囲・AI アプリへの登録 (1d)

- 環境設定の「AI 連携」ページ (§7.5.3) を本実装にする。段階 0 の 1 行の選択を置き換える。
  - 許可の範囲 (§15.2、実装済みの段だけ)、縮小画像・撮影位置のチェック。
  - 見せる範囲の 3 欄 (§5.6)。設定から生成し、変えるとその場で変わる。
  - 許可の範囲の中の操作は確認なしで実行されることと、それが唯一の上限であることを書く (§5.5)。
  - **「最近使った AI アプリ」** (独立レビュー P2-10): アプリ名・版と、最後に tool を実行した時刻 (保存しない)。
    MCP exe は要求ごとに pipe を開き直すので「接続中」は表せない。内部の状態照会は時刻に数えない。
    `clientInfo` は旧版では握手の値を接続の間保持し、2026-07-28 では各要求の `_meta` から取り、
    どちらも agent IPC の各要求で core へ渡す (agent IPC の版を 2 に上げる。未リリースなので互換は持たない)。
    自己申告の値なので長さを制限し、表示は参考情報と明記する。
  - AI アプリごとの行 (Claude Desktop / ChatGPT): 検出・登録状態・「登録する」「登録を外す」「手順を見る」。
- 登録の自動化 (§7.5.1 A / §7.5.2) に U9 と独立レビュー P2-11 を加える:
  - **書き込む前に対象のアプリが終わっていることを確かめる**。Claude は `claude.exe`、ChatGPT は ChatGPT アプリと
    同じ設定ファイルを使う Codex CLI・IDE 拡張も止めてもらう (共有の `config.toml` を他方と同時に書き換えると、
    `.bak` と rename だけでは他方の変更を残せない)。動いていれば書かずに案内する。終了の待ち合わせ・自動終了はしない。
    書く直前にファイルを読み直す。
  - ChatGPT (Codex): `[mcp_servers.mimageviewer]` に `default_tools_approval_mode = "approve"` を書く
    (公式の設定リファレンスに有効な値として載っている。「確認なしで許可」の意味は文書で明言されておらず、
    1d で Codex CLI の実動作で確かめる。未確認)。
  - Claude Desktop: 設定ファイルに tool ごとの許可を書く項目は文書で確認できない。初回の確認画面で
    「常に許可」を選ぶよう案内の文に書く。
  - 書き換えるファイルと書く内容を、押した時に表示して確認する (§7.5.3)。
- 書く `command` は配布形態ごとに決める (§15.8)。1e までは開発用の位置 (portable smoke・`target`) を指す。
- UI の規則 (coding-conventions): ダイアログ・配色・IME・UI thread で設定ファイルを読まない
  (検出と登録状態の判定は worker)、スナップショット更新、`check_ui_glyphs.py`。

### 15.8 配布の仕組み (1e)

§7 段階 1 の「配布」の契約に、配布形態ごとの登録先を加える (独立レビュー P2-9)。

| 配布形態 | AI アプリに登録する command | 理由 |
|---|---|---|
| インストーラ版・単体 exe 版 | `mimageviewer.exe --mcp` (launcher) | runtime フォルダが版ごとに変わるので、利用者が置いた launcher を指す |
| ポータブル版 (zip) | 同梱の `mimageviewer-mcp.exe` を直接、解決済みの `--data-dir` を必ず付ける | ポータブル版の `mimageviewer.exe` は core を改名したもので launcher ではない |

コード上の差し込み位置 (2026-10-31 確認):

- launcher: `#![windows_subsystem = "windows"]` のまま。`--mcp` は既存 core の activate 判定
  (`try_activate_existing`) より前で分岐する。内包対象の表 (`ASSETS`) と `build.rs` の
  `target/release/*.exe` 読み込みに `mimageviewer-mcp.exe` を足す。
- `scripts/build-release.ps1`: core → remote → EPUB worker → **MCP** → launcher。内側の exe の署名一覧と、
  release 側の PE 検査一覧に MCP を足す。
- `scripts/build-dist.ps1`: PE 一覧と、配布前の clean 対象に足す。
- `scripts/build-portable.ps1`: portable 用 target での MCP のビルド、同梱一覧、署名一覧に足す。
- `test-full.ps1` の前提 (CLAUDE.md の「`target\release` に 3 本」) が 4 本になる。CLAUDE.md・
  development-build-and-test の記述も同時に直す。
- 1e までは配布物に入れて作れるところまでで、公開しない (U10)。

### 15.9 段階 0 の残り (1a で直す)

- 範囲外の `limit`: 0 は拒否のまま、200 を超える値は 200 に丸める (Codex のモデルが `limit: 1000` を送った、§14)。
  schema の `maximum` は残し、説明文に「最大 200」と書く。core 側の `limit_value` も同じ規則にする。
- 旧版の握手で接続したクライアント向けの 1 秒ごとの状態照会: **5 秒に延ばすだけにする**。push 通知
  (core から MCP exe へ設定変更を知らせる口) は作らない。許可は要求ごとに core が判定するので、
  `tools/list` の表示が数秒遅れても許可外の操作はできない。照会の間隔は短くするほど負荷が増え、
  push は接続・取消・終了の組み合わせを増やすため (状態を減らす)。比べる対象は §15.2 の tool の集合。

### 15.10 検証

- 自動テスト: 段階 0 と同じ構成 (版ごとの JSON-RPC、schema、codec、fixture の handler、stdio 結合)。
  新しい tool ごとに fixture を足す (コレクション・閲覧履歴・本棚・ブックマーク・★・PNG の tEXt と
  JPEG の EXIF (GPS 付き)・ZIP の本)。次を検査する:
  - 撮影位置が設定 OFF で落ちること、生 JSON・`UserComment`・未知のチャンクが出ないこと。
  - 生成中に縮小画像・撮影位置を OFF にした結果が送られないこと (権限 snapshot)。
  - 長いプロンプトでも 1 ページに収まり、切ったことが示されること。cursor が上限に収まること。
  - `open_in_viewer` が本の構成 editor 中・モーダル中・sidecar 復元中に拒否を返し、置き換えにも応答が返ること。
- 実 core: `.\scripts\prepare-portable-smoke.ps1` で作り直した使い捨て portable と
  `.\scripts\mcp-client-smoke.ps1` (tool ごとの依頼文を `-PromptFile` で渡す)。core を変えたら portable を作り直す。
- 実アプリ (Claude Desktop / ChatGPT) の確認は 1d (登録の自動化) の後に 1 回だけ利用者に頼む (§9.2)。

### 15.11 状態を減らす検討

| 検討 | 結論 |
|---|---|
| 一覧ごとに tool を分ける | 分けない。`list_library` の `kind` にまとめる |
| 「mIV で開く」の表示完了まで待って返す | 待たない。UI の受付の判定だけを 1 回限りの応答で返す |
| 「mIV で開く」をモーダル中は保留する (二重起動と同じ) | しない。agent 要求は拒否で返す。保留中の権限取消という組み合わせが消える |
| 設定変更を MCP exe へ push で知らせる | しない。照会を 5 秒にする |
| 縮小画像を JPEG に変換して返す | しない。キャッシュの WebP をそのまま返す |
| 「mIV で開く」を独立したチェックにする | しない。許可の範囲の 1 段にする |
| 未実装の段も設定画面に出す | 出さない。実装済みの段だけ |
| 「接続中の AI アプリ」を表示する | しない。pipe は要求ごとに開き直すので「最近使った」にする |
| cursor に引数を複製する | しない。正規化した引数の digest で照合する |
| 1b で Remote の container を分けて使う | 使わない。一覧画面の純粋な関数を直接呼ぶ (Remote の一覧は一覧画面と規則が違う) |
| 1b で RAR・7z・EPUB の中を一覧する | しない。変換・cache・password の組み合わせを持ち込まない |
| 1b で ZIP の中の ZIP だけ一覧の対象から外す | 外さない。一覧画面と同じく木のフォルダとして扱う (agent 専用の境界規則を作らない) |
| 1b で WIC の上限超過を型で返す入口を足す | 足さない。decode の前に header の寸法で見積もる |
| 1b で RAW を現像して縮小画像を作る | しない。埋め込みの preview だけ。RAW 現像 executor の寿命を共有しない |
| 1b で一覧画面の縮小画像の処理 (`process_load_request`・catalog) を使う | 使わない。指定の 1 枚から直接作る (代表の解決・cache の取り違え・保存方針・副作用の抑止が要らなくなる) |
| 1b でフォルダ・書庫の代表の縮小画像 | 返さない。AI は `list_folder` で中の画像を指す |
| 1b で動画・音声・Susie だけの形式・EPUB・RAR の中の縮小画像 | 返さない (経路が別で、取消を受けないか共有の worker を起こす) |
| 1c で二重起動の channel に agent 要求を混ぜる | 混ぜない。専用の受付で、同時に 1 件だけ |
| 1c で ZIP の項目・PDF のページを指して開く | しない。path だけ |

まれな失敗は既定の割り切り (ログ + tool error)。

### 15.12 独立レビュー 1 巡目 (2026-10-31、gpt-6.1-sol / xhigh、設計 `c2d77e49c`・実装 `0916f0452`)

| 指摘 | 判断 | 反映先 |
|---|---|---|
| P1-1 追加の 2 つの許可が失効判定に入っていない | 妥当 (`settings_db.rs` の snapshot と epoch は `agent_level` だけ) | §15.2 |
| P2-2 GPS 除去・workflow 非公開は全メタ情報の許可表が要る | 妥当 (sidecar の生 JSON、`raw` / `workflow_json`、`UserComment`) | §15.3 |
| P2-3 1b は engine の丸ごと共有でも別 instance でも足りない | 妥当。agent 専用の入口 + 本の構成 snapshot の借用。1b は別に設計 | §15.5 |
| P2-4 `list_books` はページ数を数え、ブックマークの行構築は画像を decode する。`list_by_stars` は完全一致 | 妥当 (コードで確認) | §15.3 |
| P2-5 `run_metadata_load` は重く、期限は検索だけ、フォルダ走査は取消を受けない | 妥当。列ごとの読み取り、全 tool に期限と取消 | §15.3、§15.4 |
| P2-6 4,000 文字のプロンプトと 8,000 文字のページ、cursor の上限 | 妥当。項目の予算・切り詰め・digest | §15.4 |
| P2-7 画像に別の予算が要る | 妥当 | §15.5 |
| P2-8 `open_in_viewer` は通常の拒否も返す必要 | 妥当 (本の構成 editor・モーダル・sidecar 復元・最後の 1 件。コードで確認) | §15.6 |
| P2-9 portable 版では launcher の `--mcp` が無い | 妥当。配布形態ごとの登録先、portable のビルド・clean・PE 検査 | §15.8 |
| P2-10 「接続中」は今の pipe の寿命では表せない | 妥当。「最近使った AI アプリ」 | §15.7 |
| P2-11 Codex の共有設定も書き込み主体を 1 つに | 妥当。登録前に関係するアプリを止めてもらう | §15.7 |
| P3-12 U9 後も旧記述が残る | 妥当 | §5.5、§7 段階 2 の見出し、§7.5.2、§10 |

確認された前提: 共有 reader の束・readonly の閲覧履歴・★・タグ・collection actor の借用は worker から使え、
第二の settings 接続は要らない。6 段の許可と実装済みの段だけの表示は妥当。`agent_level` は未リリース。
既存の activation の入口の再利用は新しい detached 述語・viewport 分岐を要しない。`approve` は有効な設定値。
ビルド順・内包前の署名は妥当。`limit` の丸めと 5 秒の照会は妥当。

### 15.13 独立レビュー 2 巡目 (2026-10-31、同じ担当、設計 `7b9600a05`)

1 巡目の 12 件のうち 2・4・9・10・11 は解消、1・3 は方針として解消 (1b は別の設計待ち)、5〜8・12 は一部解消と判定された。

| 指摘 | 判断 | 反映先 |
|---|---|---|
| P2-1 open の受理の確定点と 1 回限りの応答の範囲が未確定。resolver の入口にも再 queue がある | 妥当 | §15.6 |
| P2-2 送信直前の再照合が受理済みの open の結果を失敗に変える | 妥当。読み取りの結果と受理済みの操作の結果を型で分ける | §15.4 |
| P2-3 `get_item_info` の ZIP / PDF のページの取得先が無い | 妥当。種類ごとの表 | §15.3 |
| P2-4 予算の値と包含関係が未確定 | 妥当。値・数え方・切ってよい列・1 件が収まらない時の結果 | §15.4 |
| P2-5 期限に達した時の結果が未定義 | 妥当。prefix が確定する tool だけ部分結果、他は tool error | §15.4 |
| P3-6 要約・§10 に旧記述 | 妥当 | §0、§1、§10、§11.2・§7.5.2 の参照先 |

### 15.14 独立レビュー 3 巡目 (2026-10-31、同じ担当、設計 `ddaf431c4`、1a に絞って確認)

判定は「1a: not ready」。2 巡目の 2 (読み取りの結果の側)・digest・ZIP の上限・期限と枠は解消、§15.6 の 1 は原則として解消。

| 指摘 | 判断 | 反映先 |
|---|---|---|
| P2-1 16,000 文字のページでも、text の再 escape で最終 40,000 文字を超え得る | 妥当。最終の MCP 形式で数える関数を共有し、core がその大きさでページを切る | §15.4 |
| P2-2 ZIP / PDF のページのタグは UI の対象外 | 妥当。★とタグを分け、タグは `container_tags` | §15.3 |
| P2-3 期限までに新しい項目が 1 件もできない場合 | 妥当。時間切れの error。`get_item_info` は入力の位置から再開 | §15.4 |

4 巡目 (設計 `f97b1ed74`): 追加の指摘なし。判定は「1a: ready」。最大の項目 + 最長の cursor で 28,474 文字 (ページ予算 38,000 以内)、envelope は 1,669 文字 (予約 2,000 以内) と計算で確認された。

### 15.15 独立レビュー 5 巡目 (2026-10-31、同じ担当、設計 `c3f14e4f6`、1b・1c)

判定は「1b: not ready」「1c: not ready」。Remote の engine を使わず一覧画面の純粋な関数を使う方針、全件の識別の
digest を入れた cursor、typed admission と判定の共有は妥当とされた。1c は detached の述語・viewport の routing に
触れず、症状パッチではない構造的な変更である、という主張に同意を得た (ClaudeCode も同じ判断)。

| 指摘 | 判断 | 反映先 |
|---|---|---|
| P2-1 `process_load_request` が ZIP のキー移行 (利用者の DB の書き換え) と PDF の補完の予約 (別の取消) を伴う | 妥当 (`thumb_loader.rs` で確認) | §15.5「副作用を持たない」 |
| P2-2 要求を作る関数は pin・変換済み書庫・password 等を受け、一覧画面は生成後に編集の preview を付ける | 妥当。worker が用意する入力の表、補正・編集は反映しない。**「隠す」編集のある項目は画像を返さない** (ClaudeCode の追加判断) | §15.5「経路」「補正・編集」 |
| P2-3 frame の先頭で受理すると、同じ frame の初期 open に置き換えられ得る | 妥当 (`update_phases.rs` の `render_initial_open` で確認) | §15.6「判定の位置」 |
| P2-4 service の初期 snapshot は `status=Err`・空の rows | 妥当 (`service.rs` で確認) | §15.5「本の構成」 |
| P2-5 一覧画面は materializer の後に★・facet・色等で絞る | 妥当。基礎の一覧を返すと定義 | §15.5「基礎の一覧」 |
| P3 お気に入りの表示状態の保存は「最大 500 ms」ではない (変更のたびに延びる) | 妥当 (`favorite_view_state.rs` で確認) | §15.5「設定」 |
| 回答 3 RAW の保存抑止は解決後の実際の元の画像にも要る | 妥当 | §15.5「RAW」 |
| 回答 6 窓の復帰は既存の activation event を通知すれば足りる。`AllowSetForegroundWindow` は保証されない | 妥当。event の通知にし、前者は best effort | §15.6「窓を前に出す」 |

### 15.16 独立レビュー 6 巡目 (2026-10-31、同じ担当、設計 `8f2cf9203`)

判定は「1b: not ready」「1c: ready」。5 巡目の P2-3〜P2-5・P3、RAW の保存抑止、activation event の再利用は解消と確認された。

| 指摘 | 判断 | 反映先 |
|---|---|---|
| P1 「隠す」の判定が、代表の画像の実際のページまで定義されていない (代表の選択は loader の中、cache hit では選択を通らない) | 妥当 | §15.5 `get_thumbnail` |
| P2 抑止する副作用が足りない (類似索引の prefill、fallback の再入、PDF の描画後の `pdf_meta` 更新) | 妥当 | 同上 |
| P2 変換書庫の CacheOnly 要求を作る経路が無い。password 付き PDF の判定元も無い | 妥当 | 同上 |
| P2 ZIP の代表・項目の bytes の読み取り、RAW の事前判定に取消が無い | 妥当 | 同上 |
| P2 full path の key を使わないと、ドライブの違う同名フォルダで cache を取り違える | 妥当 (`catalog.rs` はドライブ文字を落とす) | 同上 |

ClaudeCode の判断: 指摘はどれも、一覧画面の `process_load_request` (catalog・代表の解決・副作用を内側に持つ) を agent から
使う前提から出ている。同じ前提の上で抑止を足し続けるのをやめ、`get_thumbnail` を「指定の 1 枚から cache を使わずに直接作る」
形に組み直した (本の構成の編集画面の縮小画像と同じ考え方)。代表の縮小画像・変換書庫・password 付き PDF の cache は扱わない。

### 15.17 独立レビュー 7 巡目 (2026-10-31、同じ担当、設計 `ebfcabbce`)

判定は「1b: not ready」。6 巡目の 5 件は、一覧画面の縮小画像の経路を使わないことで解消と確認された。Susie を起こさない
拡張子の判定、向きを 1 度だけ掛けること、各 DB の key の形、隠すの読み取りの失敗を tool error にすることも妥当とされた。

| 指摘 | 判断 | 反映先 |
|---|---|---|
| P1 CP932 名の ZIP の旧 key に残った隠す編集を見落とす (agent は移行しない) | 妥当 (`zip_key_migration.rs` の対象に `conceal.db` がある) | §15.5 `get_thumbnail`「隠す」 |
| P2 入力の上限だけでは decode の memory が決まらない (budget を省くと AVIF・WIC は無制限) | 妥当 | §15.5「decode の memory の上限」 |
| 確認 PDF の取消の待ち方は既存の `HarvestOnCancel` | 採用 | §15.5 PDF のページ |

### 15.18 独立レビュー 8 巡目 (2026-10-31、同じ担当、設計 `4b0678842`)

判定は「1b: not ready」。7 巡目の P1 の契約 (新旧両 key・不明なら返さない)、256 MiB の上限、`HarvestOnCancel` は解消と確認された。

| 指摘 | 判断 | 反映先 |
|---|---|---|
| P2 旧名のための全件の列挙が、Susie の pool の待ちと上限なしの入れ子の展開を持ち込む | 妥当。対象の項目を読む同じ読み取りで新旧の名前を取る | §15.5 `get_thumbnail`「隠す」 |

`list_folder` の ZIP の一覧は同じ列挙を使うが、これは一覧画面の規則そのもの (一覧画面も同じ関数) なので変えず、その旨を表に書いた。

### 15.19 独立レビュー 9 巡目 (2026-10-31、同じ担当、設計 `1b8d01cc1`)

判定は「1b: not ready」。`get_thumbnail` の旧名の取り方は解消、`list_folder` が一覧画面と同じ拡張子の判定・入れ子の列挙を使う方針と、
Susie の初期化の待ちを既存の枠の保持で扱うことは妥当と確認された。

| 指摘 | 判断 | 反映先 |
|---|---|---|
| P2 入れ子の ZIP の展開は宣言の大きさをそのまま確保し、読み取り中も上限なく伸びる。期限と枠は巨大な確保を防がない | 妥当。agent の呼び出しだけ上限を付け、超えたら一覧全体を tool error | §15.5 `list_folder` の ZIP |

### 15.20 独立レビュー 10 巡目 (2026-10-31、同じ担当、設計 `2b8a4452f`)

判定は「1b: not ready」。9 巡目の上限は解消。§15.5 全体の最終確認で、指摘は次の 1 件だけだった。

| 指摘 | 判断 | 反映先 |
|---|---|---|
| P2 PDF の表示する種類の設定を、構成の adapter へ渡す前に掛ける工程が無い (adapter は渡された行を `Present` と扱う) | 妥当 | §15.5 `list_folder` の PDF |

11 巡目 (設計 `136525a9e`): 追加の指摘なし。判定は「1b: ready」。1c は 6 巡目で「1c: ready」。

## 16. 段階 1a の実装記録 (2026-10-31、基点 `17c85bb3a`、未コミット差分)

作業先は `G:\home\mimageviewer-mcp` / `mcp-trial`。1a の読み取り・権限・予算だけを変更し、
1b の一覧/画像、1c の open、1d の登録/UI、配布・公開文書には進まない。

### 16.1 コードで確認した前提

- `remote_ipc::SharedReaders` は一つの `LiveFavorites` と collection reader を持つ。
  `favorites()` と既存 `settings_db::with_db_result` を借り、第二の settings 接続は作らない。
  3値の権限 snapshot は同じ SettingsDb mutex/connection から読み、成功 commit の変更時だけ epoch を進める。
- `ReadingHistoryDb::open_readonly().list_recent` と `book_bookmarks::load_all_from_disk_readonly` は
  readonly の既存入口。ブックマークはその DB 行を直接投影し、browser の stat/decode を呼ばない。
- `books::list_books` は本ごとに `book_page_count` を実行するため使わず、
  `list_book_paths_while` を追加した。直下ディレクトリの名前/path のみを取り、項目間で取消を確認する。
- `RatingDb::list_by_stars` は完全一致。新しい `list_at_least_stars` は >= の SQL と
  ★降順→rated_at_ms 降順→path の順を使い、行の読出し間で取消を確認する。
- `prepare_collection_snapshot_while` は filesystem entry 間の predicate と UI 非依存の並び替えを持つ。
  `get_collection` は actor の catalog/load とこの関数を使い、欠落参照も順序から消さない。
  Remote の view cache を使わず、Remote の通常処理の本体は変えない。
  名前はストアと同じ trim・非空判定で照合し、検索語用の200文字制限は掛けない。
  ★の threshold は adapter/core とも >5 を拒否し、0 は保存済み★の最小値1へ正規化する。
- 寸法は `remote_ipc::container::file_image_dims` の既存 header/RAW/AVIF 読み取りを visibility だけ広げて使用。
  PDF は `pdf_loader::get_page_sizes(path, None)` の既存経路を使う。Remote の `catalog_free_dims` と同じく
  points を丸め、wire に `unit=points` を明示する。画像は `unit=pixels`。
- PDF の `AbortOnCancel(Some(cancel))` は呼出元が先に戻って実 IPC が残るため使わない。
  既存の完了待ちを nested read worker 内に保ち、前後で協調取消を確認する。外側は20秒で応答できるが、
  worker の join までは Permit を保持する。この点は独立した実装前レビューでも確認した。

### 16.2 投影と型の選択

- agent IPC を strict-equality の版2、frame を4MiBへ変更。pipe 名も共通関数で同じ版2から導出する。
  v1との互換は設けない。`ReadAndOpen` / 新2設定は未リリースなので migration は追加しない。
  設定 UI は既存2択だけ。追加の縮小画像/GPS設定に UI は付けない。
- wire の `PermissionSnapshot` は level/thumbnails/gps の一つの値。
  通常 read 結果には snapshot+epoch を持つ `ReplyPolicy::Read` を使い、送信直前に全体を再照合する。
  将来の `AcceptedOperation` は型の区別だけを置き、操作の受付機構は作っていない。
- legacy の監視は5秒、snapshotから導いた tool 名集合を比較する。1a は有効時8件なので、
  GPS/縮小画像/ReadAndOpen の変更だけで集合が変わらない場合には通知を増やさない。
  通常 tool の Disabled は snapshot の失効でも返るため、そこから集合を推定しない。
  Status 成功・tools/list・既存 poll の観測を使い、実際の無効化の反映は承認済みの最大5秒待ちとする。
- `InfoField` で「値あり」「対象外」「取得不能」「大きすぎる」を区別する。
  送出 EXIF / AI生成情報は専用の既知列の型だけ。GPS は snapshot が許す時だけ、
  生 JSON / UserComment / MakerNote / 未知チャンク / 外部 sidecar は投影しない。
- 実ファイルは絶対 path と NUL を検査し、既存の DB key を変える fs canonicalize は行わない。
  Remote LAN の network-path 拒否は agent に持ち込まず、UI と同じく UNC を読む範囲に含める。
  ZIP は `book_scope::validate_relative_reference` と既存の解決関数を使う。
  `ZipImage` と `PdfPage(page_N、0始まり)` の生成後、
  `edit_source::page_key_for_grid_item` / `page_key_for_pdf` で UI と同じ★ keyを取得する。
  仮想ページ自身のタグは対象外、container のタグは `container_tags` として区別する。
- ZIP の literal/nested/CP932 解決は `resolve_entry_index` / `split_nested_zip_path` を visibility だけ広げて再利用。
  宣言サイズを展開前に検査し、実読出しも64MiBで打ち切る。中間の nested ZIP も同じ上限で、超えたら
  metadata を TooLarge とする。ZIP の DOS 時刻には timezone が無いため UTC calendar として明示的に換算する。
- `PageRow<T>` は通常行の JSON 形を維持し、大きすぎる行だけ digest ID と `too_long` 印を返す。
  path/subresource/id/cursor は切らない。説明用の生成文字列だけ `TextValue.truncated` を付けて切る。
  全ページを共通 `mcp_content_chars` で測り、行12,000・cursor込みページ38,000・最終MCP40,000を守る。
- cursor は版2・tool・正規化 args の SHA-256 digest・入力位置/実返却数だけを符号化する。
  `get_item_info` は入力位置から読み始め、既出の metadata/ZIP を読み直さない。
- 全読み取りを controller / 実 worker に分け、期限・取消後も実 worker 終了まで同時枠を保持する。
  検索・item info・順序が設定から確定するお気に入りの prefix は進む cursor とともに返し、新規行0件なら Timeout。
  その他の library / collection の並べ替え途中では部分成功を返さず Timeout にする。

### 16.3 検証の担当と範囲

実装担当がこの sandbox 内で compile・fixture 検査を担当する。実 pipe は既知の OS 5 制限を
skip/ignore や DACL 緩和で回避せず、通常権限の full-trust context で ClaudeCode が最終確認する。
実装担当は作業中に別の確認役 (実装担当の申告。ClaudeCode からは別担当かを確かめられない) に
snapshot/epoch、予算、metadata 許可表、期限・Permit・PDF完了待ち・cursor継続の差分を見せ、追加 P1/P2 はなしとした。
正式な独立レビューは ClaudeCode が別の担当へ依頼する (§16.7)。
実行結果は後記の ledger に記録し、source review を実 pipe 成功の代わりにはしない。

### 16.4 追加・変更ファイル

| 範囲 | ファイル | 内容 |
|---|---|---|
| 共通 IPC | `crates/agent-ipc/src/lib.rs`, 新規 `read_types.rs` | snapshot・3 command・typed projection・予算・digest・v2 codec |
| adapter | `crates/mcp-server/src/lib.rs`, `schema.rs`, `tests.rs` | 8 tool の parser/schema、集合比較、5秒照会、共通予算 |
| core | `src/agent_ipc/mod.rs`, 新規 `work.rs`, `library.rs`, `item_info.rs` | 実 worker の期限/取消、read policy、各投影、共通ページ境界 |
| core fixture | `src/agent_ipc/tests.rs`, 新規 `read_tests.rs`, `item_info_tests.rs` | 既存の wire 追従・実 controller・readonly DB・metadata fixture |
| 設定 | `src/settings.rs`, `settings_db.rs`, `settings_transfer.rs` | 新2設定の保存・snapshot epoch・自動転送除外 |
| UI の最小追従 | `src/ui_dialogs/preferences/pages.rs` | hidden な ReadAndOpen を既存の読取表示に含め、選択肢は2つのまま |
| 専用の軽い読出し | `src/books.rs`, `rating_db.rs` | 本棚直下の名前/path、★以上の query |
| 再利用境界 | `src/remote_ipc/container.rs`, `remote_ipc/mod.rs`, `zip_loader.rs` | 必要 helper の visibility/reexport のみ。本体は無変更 |
| agent 専用の型追従 | `src/remote_ipc/collections.rs` | agent projection の tags/container_tags 型。Remote 通常処理は無変更 |
| 実装記録 | `docs/mcp-integration-plan.md` | この §16 |

### 16.5 途中の修正と検証記録

初回 core check は exit 101 (favorites Arc の反復方法、hidden ReadAndOpen の match 不足) となり、修正した。
最初の lib test compile も exit 101 (旧 Item fixture の Option tags/container_tags 追従不足) となり、修正した。
その後の焦点35件は exit 100、32成功/3失敗。2件は既知の実 pipe OS 5、残る1件は PNG fixture の
tEXt に Latin-1 外の文字を渡したため、metadata 検査より前に BadTextEncoding となった。
有効な tEXt として長い escape を含む ASCII prompt に変更し、同じ切り詰め・許可表・ページ予算を検査する。
PNG の exact 再検査は exit 0、1/1成功 (`target/mcp-1a-png-retest.log`)。
続く最終結果を下記に記録する。途中の source review 後、名前/threshold 境界と PNG fixture の限定差分も
同じ独立 reviewer が確認し、追加 P1/P2 はなし。
お気に入りは順序が先に確定するため、全 library を Timeout のみとする初期実装も修正した。
`favorite_page` は offset から取り出し、完成行ごとに共通予算を通して prefix を公開する。
後続行で停止→期限→cursor=1 の部分応答→worker 解放まで枠保持、という実 controller 検査を追加した。
この限定修正も独立 reviewer と構造を確認した。これらは設計の矛盾ではなく実装の境界修正である。
通知の最終点検では、通常 tool の snapshot 失効を集合0件と誤認する余分な通知を補正した。
wire error や追跡状態は増やさず、Status 成功・tools/list・poll だけで集合を更新する。
legacy 2版の実 stdio runtime で、tool の失効 error と ping が list_changed なしで返る検査を追加した。

### 16.6 最終検証 (観測者: 実装担当、この sandbox 内)

| コマンド | 実 exit code | 結果 / ログ |
|---|---:|---|
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 | 最終 core 差分を検査。`target/mcp-1a-core-complete.log` |
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` | 101 | IPC 10/10成功、MCP 40成功/2失敗、ignore 0。失敗はいずれも下記の OS 5。`target/mcp-1a-crates-complete.log` |
| 下記の focused nextest | 100 | 36件中34成功/2失敗。失敗はいずれも下記の OS 5。`target/mcp-1a-focused-complete.log` |
| `cargo build -p mimageviewer-mcp --bin mimageviewer-mcp` | 0 | 最終 adapter 差分の console exe を生成。`target/mcp-1a-adapter-complete.log` |
| `cargo fmt` (workspace、引数なし) | 0 | 最終 source 差分を整形 |
| `cargo fmt --check` | 0 | `target/mcp-1a-fmt-check.log` |
| `python scripts/check_ui_glyphs.py` | 0 | UI glyph 検査成功。snapshot の更新なし |

core の focused 結果はその後の adapter 専用通知修正では取り直さず、同じ core source の成功結果を再利用した。
通知修正の exact 回帰検査も exit 0 で、続く上記 crate 全体の再検査にも含めた。
lib test 前に、この worktree の FFmpeg DLL 実ファイルを `target/debug/deps/` に配置済み。

失敗は4件とも既知の restricted token による名前付き pipe の作成/接続時の `raw_os_error=5` であり、
handler の assertion 失敗ではない。skip/ignore 化や DACL の変更はしていない。

- core: `agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry`
- core: `agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers` (fixture pipe 作成時、adapter 起動前)
- adapter: `pipe::tests::real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld`
- adapter: `pipe::tests::real_pipe_cancel_reaches_core_while_stdio_stdout_is_blocked`

ClaudeCode が通常権限の sandbox 外で実行するコマンド (この最終差分での成功は未観測):

```powershell
cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored
cargo nextest run -p mimageviewer --lib -E 'test(/agent_ipc/) or test(/agent_level/) or test(/agent_permission/) or test(=settings_db::tests::failed_permission_commit_preserves_snapshot_and_epoch) or test(/remote_ipc::live_favorites/) or test(/settings_transfer::tests::automatic_network_enablement/)'
```

collection / history / bookshelf / bookmarks / ratings の fixture、JPEG EXIF/GPS、PNG tEXt の長い prompt、
ZIP の通常/CP932/nested member、GPS の非公開、未知 metadata の非公開、全 snapshot の失効、
50入力の短い cursor、limit 1000 の丸め、prefix/並べ替え型の期限と枠保持を上記の成功分で検査した。
実 native PDF worker は起動せず、PDF page の寸法投影と page key は純粋 fixture 検査に留めた。
PDF 実読出し・実 mIV との接続・実データを使う動作の確認は行っていない。
製品 binary、通常 profile、UI を起動せず、test-full・commit・snapshot 更新も行っていない。
依頼された検証用 binary は `target/debug/mimageviewer-mcp.exe` のみ生成し、core の検証起動は行わない。
設計前提の矛盾は見つからず、未解決のコード上の指摘はない。1b/1c/1d は未実装のまま。

### 16.7 検収 (観測者: ClaudeCode、2026-10-31)

sandbox の外 (通常権限) で、実装担当が実 pipe の OS 5 で落とした 4 件を含めて再実行した。

| コマンド | exit code | 結果 |
|---|---:|---|
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` | 0 | IPC 10/10、MCP 42/42 成功 |
| §16.6 の focused nextest | 0 | 36/36 成功 |
| `cargo fmt` (workspace) | 0 | 差分なし |

独立レビュー 1 回目 (別担当 thread `01a18ce7`、gpt-6.1-sol / xhigh、read-only、`8edce9336`) は「changes needed」。
3 件とも ClaudeCode がコードで確かめて妥当と判断し、§16.8 で直した:

| 指摘 | 判断 |
|---|---|
| P1 許可した生成条件の値に object/array が来ると、既存 parser が JSON 文字列にして生 JSON・位置情報が漏れる | 妥当 (`png_metadata.rs` の object→文字列化を確認) |
| P2 snapshot の失効を `Disabled` と返し、adapter が空の tool 集合と余計な `list_changed` を出す | 妥当 (`permission_tools` の Disabled→空集合を確認) |
| P2 PDF の寸法取得が path 入りの perf key をログに出す | 妥当 (`pdf_file_perf_key(pdf_path)` を確認) |


### 16.8 修正 1 回目 (基点 `8edce9336`、未コミット差分)

正式な独立レビュー (ClaudeCode が別 thread の gpt-6.1-sol / xhigh へ依頼) は changes needed。
以下3件を ClaudeCode がコードで確認して採用し、実装担当へ修正を依頼した。
§16.7 の IPC 10/10・MCP 42/42・core 36/36 の sandbox 外成功は **ClaudeCode の観測**であり、
今回の修正差分に対する実行結果とは区別する。

| 指摘 | 修正 |
|---|---|
| P1: 許可済み生成条件へ object/array が文字列化され、生 JSON・位置が漏れる | 文字列化前の JSON 型が残る parser に agent 専用の scalar 制約を通す。既存の UI/search 入口は従来の変換を維持し、agent の file/ZIP bytes 両入口だけを切り替える |
| P2: snapshot 失効を Disabled と誤認し、Status poll/tools/list が集合0件を通知する | wire に `PermissionChanged` を追加。実際に無効なら `Disabled`、有効のまま snapshot/epoch が変わったら `PermissionChanged`。adapter は後者から集合を推定しない |
| P2: PDF page sizes の perf key に要求 path が入る | agent 専用の匿名入口を追加し、pool の任意の perf key を None にする。UI の既存入口は従来の key を維持する |

権限は Status だけ再照合を免除する案も検討したが、§15.2/§15.4 の全 read 共通規則を保つため、
単一の失効 error を区別する方式を選んだ。再試行・待機状態・追加の権限 observer は作らない。
stale な tools/list は空一覧の成功にせず問い合わせ error とし、次回の新しい Status で通常の一覧を返す。
tool call は「AI 連携の許可が変更されました。もう一度お試しください」の tool error になる。
protocol は未リリースの v2 のまま。この修正は永続形式・設定 schema を変えない。

生成情報の制約は JSON から文字列になる前に適用する。prompt/negative prompt/model/sampler 等は文字列、
seed/steps/CFG/width/height は文字列または数値とし、これらに boolean/object/array は採用しない。
tool 名は既知形式からの固定名。JSON container が平文 prompt に落ちる fallback も agent mode では除外する。
既に文字列である prompt の `{...}` / `[...]` は内容から再解釈せず維持する。
通常の UI/search の JSON 整形は Display mode のままで、既存 parser の表示内容を変えない。

変更ファイルは `crates/agent-ipc/src/lib.rs` (wire error)、`crates/mcp-server/src/lib.rs` / `tests.rs`
(集合判定と競合検査)、`src/agent_ipc/mod.rs` / `read_tests.rs` / `tests.rs` (失効の区別と旧期待値の追従)、
`src/agent_ipc/item_info.rs` / `item_info_tests.rs` (新入口と投影fixture)、`src/png_metadata.rs`
(agent scalar mode と UI互換検査)、`src/pdf_loader.rs` (匿名の入口と共通実行境界)、この文書。

追加の検査は、PNG 実ファイルと同一 PNG の ZIP、全許可フィールド/別名の object/array、正常な scalar、
JSON container fallback、UI/search の従来表示、EXIF からの生成情報、Status の wire 直前の snapshot変更、
旧握手2版の実5秒 poll と tools/list が失効した場合の通知抑制/次回8件復帰、PDF の実際の匿名/UI入口を含む。
PDF は thread-local の test executor で成功・worker status error・transport error を返し、
実 native worker を起動せず、要求の path/password は IPC 内に残り diagnostic key は None と検査する。

helper の呼出し経路のログ監査 (source inspection、実装担当の確認。正式な独立レビューを意味しない):

| helper | 結果 |
|---|---|
| PDF | pool の dispatch/open/cancel/prune は任意の perf key を使用。agent は None、UI は従来の path key。PDF passthrough 解決・cache・response parser・worker の page-size error で要求 path をログに出す経路はなし |
| ZIP | agent の `read_zip*`、archive open/cache、名前解決はログなし。既存の path 付き first-image 計測は呼ばない |
| EXIF/生成情報 | 両 parser に logger/perf/print 呼出しなし |
| 画像寸法 | header/RAW info に Rust の path ログなし。AVIF の native error は bytes から生成され、要求 filename は native parser へ渡らない |
| LibRaw native | 一般の既定 callback は filename を出すが、実 shim の Windows wide-path 経路は narrow filename を持たず `fname()` が null、callback は unknown file とする。要求 path の漏えいはこの経路には確認されず、native/RAW は変更しない |
| collection | actor read・DB snapshot load・prepare に要求 path/id/name のログなし |
| 本棚/★/閲覧履歴/しおり/タグ | 使用する readonly/list/get 経路に要求 path ログなし。writer/recovery 診断は今回の呼出し経路外 |

pool 初期化の executable/DLL/data/log ディレクトリの診断は既存の infrastructure ログで、要求 PDF の path ではない。
第1回 crate 検査は exit 101、IPC 10/10成功・MCP 41成功/2失敗 (43件、ignore 0)。
新しい poll/list 競合検査は成功、失敗2件は従来どおり実 pipe の OS 5。
ログは `target/mcp-1a-fix1-crates.log`。その後の変更は core側修正と workspace整形のみで、この結果を再利用する。
最初の core 焦点102件は exit 100、99成功/3失敗 (`target/mcp-1a-fix1-focused.log`)。
追加した漏えい/権限/PDF 検査は成功し、失敗は既知の実 pipe OS 5 の2件と、
再有効化後も Disabled を期待した旧 fixture の1件だった。後者は現在の無効時の Disabled を新たに検査し、
再有効化後の古い結果は PermissionChanged とする期待値へ更新した。製品の判定を緩めず、以下の最終再検査へ進む。

今回の focused filter は §16.6 に PNG parser 全体と PDF の3件を追加したもの。
ClaudeCode が sandbox 外の通常権限で再実行するコマンドは以下 (今回の差分での sandbox 外成功は未観測)。

```powershell
cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored
cargo nextest run -p mimageviewer --lib -E 'test(/agent_ipc/) or test(/agent_level/) or test(/agent_permission/) or test(=settings_db::tests::failed_permission_commit_preserves_snapshot_and_epoch) or test(/remote_ipc::live_favorites/) or test(/settings_transfer::tests::automatic_network_enablement/) or test(/png_metadata/) or test(=pdf_loader::tests::anonymous_page_sizes_keep_worker_path_out_of_diagnostics_on_all_results) or test(=pdf_loader::tests::page_sizes_preserve_existing_ui_diagnostic_identity) or test(=pdf_loader::tests::page_sizes_round_trip_and_reject_a_truncated_reply)'
```

最終検証 (観測者: 実装担当、この sandbox 内、`8edce9336` + この修正差分、通常 dev/test profile):

| コマンド | 実 exit code | 結果 / ログ |
|---|---:|---|
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 | `target/mcp-1a-fix1-core.log` |
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` | 101 | IPC 10成功、MCP 41成功/2失敗、ignore 0。`target/mcp-1a-fix1-crates.log` |
| 上記 focused nextest (期待値修正後) | 100 | 102件中100成功/2失敗。`target/mcp-1a-fix1-focused-final.log` |
| `cargo build -p mimageviewer-mcp --bin mimageviewer-mcp` | 0 | `target/mcp-1a-fix1-adapter.log` |
| `cargo fmt` (workspace、引数なし) | 0 | 統合後と旧期待値修正後に実行 |
| `cargo fmt --check` | 0 | `target/mcp-1a-fix1-fmt.log` |
| `git diff --check` | 0 | 最終差分を確認 |

失敗4件は §16.6 と同じ実 pipe の作成/接続時の OS 5。
core は idle-connected shutdown と stdio fixture、adapter は withheld-response cancel と blocked-stdout cancel。
新しい型/投影/互換/競合/匿名化の検査はすべて成功した。
core check と adapter build 後の Rust 変更は core の test-only 期待値追従だけなので、その成功結果を再利用した。
FFmpeg DLL は前回配置済みの worktree 内実ファイルを使用した。

正式な独立再レビュー、今回差分での sandbox 外実 pipe、実 native PDF worker の確認は未実施。
製品 binary / UI / 通常 profile を起動せず、test-full、commit、snapshot 更新、公開文書更新は行っていない。
この3件について設計担当の追加判断を要する矛盾は見つからなかった。

### 16.9 クライアントからの呼び出し観測 (観測者: ClaudeCode、2026-10-31、コード `8edce9336`)

- 使い捨て portable (`prepare-portable-smoke.ps1` で作り直し) + 隔離 data。段階 0 の seed (タグ) に、
  ★ 4 件 (5・4・4・3) と `parameters` tEXt を持つ 8×8 PNG `ai_cat.png` を足した (seed は scratchpad の `seed_1a_smoke.py`)。
- Codex CLI 0.166.0 + `mcp_client_smoke.py`、ログ `target\mcp-client-smoke\20261031-151929-codex.jsonl`。
  モデルは list_library (rated, stars 3 / reading_history / favorites)・list_collections・get_item_info の 5 回を呼び、全て成功。
  limit は 200 で送られた (段階 0 の 1000 は今回なし)。
- 回答: ★順 cat_garden 5 → dog_park 4 → ai_cat 4 → cat_sofa 3 (同じ★は rated 時刻の新しい順で seed どおり)。
  ai_cat.png は寸法 8×8、プロンプト・ネガティブ・モデル名・Sampler/Steps/CFG/Seed/サイズ、タグなし、★4。
  お気に入り・閲覧履歴・コレクションは空。
- 独立レビューの P1〜P2 (§16.8) を直す前のコードでの観測。

### 16.10 修正 1 回目の検収 (観測者: ClaudeCode、sandbox 外)

| コマンド | exit code | 結果 |
|---|---:|---|
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` | 0 | IPC 10/10、MCP 43/43 成功 |
| §16.8 の focused nextest | 0 | 102/102 成功 |

### 16.11 修正 2 回目 (基点 `ef7148431`、未コミット差分)

観測者の区別: ClaudeCode は sandbox 外で §16.10 の IPC 10/10、MCP 43/43、core 102/102 (各 exit 0) を観測した。
正式な独立 reviewer は前回の3指摘 (生JSON漏えい、PermissionChanged、PDFのpathログ) の解消と
UI/search の解釈維持を確認した一方、既知の生成情報の container を早く拒否し過ぎる P2 を追加した。
ClaudeCode が採用し、この修正を実装担当へ依頼した。今回差分の実行結果は下記に別記する。

- `src/png_metadata.rs`: 既知の model object の別名列、旧 prompt array の要素、v4 caption の構造を
  既存 parser と共有してたどり、文字列として取り出す末端で scalar 制約を適用する。
  未知の構造を stringify しない。数値を許可する既存の seed/steps 等の規則は変えない。
- 旧 prompt array の末端がすべて不正でも、独立した model/生成条件を維持し、同居 Dream から
  拒否済み prompt を再採用しない。既知 v4 でも不正な最後の caption だけを落とし、Seed/Steps は残す。
  この認識維持は agent mode のみで、Display の認識条件・文字列化は従来どおり。
- `src/agent_ipc/item_info_tests.rs`: 実 fixture `invokeai_new.png` と `invokeai_legacy_sdmeta.png` を
  file/ZIP の両入口へ通す。v4 は既存 parser test と同じ caption/char_captions 形を使い、Description を置かない。
  正常時の値と、末端が object/array/bool/number/null の場合に対象値だけが消えることを検査する。
  複数 prompt/caption の一部だけ不正な場合と、最後の文字列が不正な場合も含め、他の列を全体比較で保護する。
- この文書に判断と検証台帳を追記。新しい公開API・設定・wire形式・永続形式・UI は追加しない。
  別の agent 専用 traversal を作る案より、既存の既知構造と leaf conversion を共有する方を選んだ。

初回 focused 検査は exit 100、105件中102成功/3失敗 (`target/mcp-1a-fix2-focused.log`)。
新3件は成功したが、前回の object/array 漏えい検査が v4 caption 配列中の任意文字列を検出した。
その期待値を緩めず、agent mode の caption collection は既知 object をたどり、その名前付き leaf だけを読むよう補正した。
Display mode の配列処理は変えない。正常 caption object と任意の文字列が混在する場合の検査も追加した。
残る2失敗は既知の実 pipe OS 5。空の旧 prompt array は拒否した leaf を持たないため、従来の Dream fallback を維持する。
新3件は既存の `/agent_ipc/` filter に含まれるため、§16.8 の filter を拡張する必要はない。
ClaudeCode が sandbox 外の通常権限で再実行するコマンド (今回差分での外側の成功は未観測):

```powershell
cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored
cargo nextest run -p mimageviewer --lib -E 'test(/agent_ipc/) or test(/agent_level/) or test(/agent_permission/) or test(=settings_db::tests::failed_permission_commit_preserves_snapshot_and_epoch) or test(/remote_ipc::live_favorites/) or test(/settings_transfer::tests::automatic_network_enablement/) or test(/png_metadata/) or test(=pdf_loader::tests::anonymous_page_sizes_keep_worker_path_out_of_diagnostics_on_all_results) or test(=pdf_loader::tests::page_sizes_preserve_existing_ui_diagnostic_identity) or test(=pdf_loader::tests::page_sizes_round_trip_and_reject_a_truncated_reply)'
```

最終検証 (観測者: 実装担当、この sandbox 内、`ef7148431` + 今回差分、通常 dev/test profile):

| コマンド | 実 exit code | 結果 / ログ |
|---|---:|---|
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 | 最終 parser 差分で再実行。`target/mcp-1a-fix2-core-final.log` |
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` | 101 | IPC 10成功、MCP 41成功/2失敗、ignore 0。`target/mcp-1a-fix2-crates.log` |
| 上記 focused nextest | 100 | 105件中103成功/2失敗。`target/mcp-1a-fix2-focused-final.log` |
| `cargo build -p mimageviewer-mcp --bin mimageviewer-mcp` | 0 | `target/mcp-1a-fix2-adapter.log` |
| `cargo fmt` (workspace、引数なし) | 0 | 最終 source 差分を整形 |
| `cargo fmt --check` | 0 | `target/mcp-1a-fix2-fmt.log` |
| `git diff --check` | 0 | 最終差分を確認 |

追加3件・前回の漏えい回帰・既存 PNG parser は全て成功。
非ゼロは §16.6/§16.8 と同じ4件の実 pipe OS 5 だけであり、skip/ignore 化や DACL 変更はしない。
core は idle-connected shutdown と stdio fixture、adapter は withheld-response cancel と blocked-stdout cancel。
crate 検査・adapter build 後の変更は core の parser と fixture だけなので、それらの結果を再利用する。
FFmpeg DLL は前回配置済みの worktree 内実ファイルを使用した。

今回差分の正式な独立再レビュー・sandbox 外実 pipe の最終成功は未観測。
製品 binary / UI / 通常 profile を起動せず、test-full、commit、snapshot 更新、公開文書更新は行っていない。
この指摘について追加の設計判断を要する矛盾は見つからなかった。

### 16.12 修正 2 回目の検収 (観測者: ClaudeCode、sandbox 外)

独立レビュー 2 回目 (`ef7148431`) は元の 3 件の解消と UI・検索の解釈不変を確認し、新しく [P2] 1 件
(型制限が早すぎて InvokeAI の model 名・NovelAI v4 caption・旧 InvokeAI の prompt 配列まで落ちる) を出した。
妥当と判断し §16.11 で直した。

| コマンド | exit code | 結果 |
|---|---:|---|
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` | 0 | IPC 10/10、MCP 43/43 成功 |
| §16.11 の focused nextest | 0 | 105/105 成功 |

独立レビュー 3 回目 (`d7d54c78a`) は追加指摘なし、判定「1a impl: accept」。段階 1a はこれで検収とする
(観測者: ClaudeCode。実機の AI アプリでの確認は 1d の後に利用者へまとめて依頼する)。

## 17. 段階 1b の実装記録

### 17.1 前提照合での停止 (観測者: 実装担当、基点 `570454c5f`)

依頼の「前提が違えば実装を止めて報告する」に従い、以下の矛盾で停止した。未コミット。
一覧/縮小画像の実装を進めるための回避策・新APIは追加していない。

§15.5 が `list_folder` の catalog miss 時に指定する `pdf_loader::enumerate_pages_with_read_target` は、
現コードでは次の二つを固定している (`src/pdf_loader.rs:5677` 以降):

- `pdf_file_perf_key(pdf_path)` を作り、`Some(perf_key)` を pool へ渡す。`pool_dispatch` 等はその key を
  記録するため、agent の「要求 path をログに出さない」と両立しない。
- `CancelWaitPolicy::AbortOnCancel` を渡す。`wait_for_pool_reply` は取消後、pool の実処理の応答を待たず
  `Interrupted` で戻る (`src/pdf_loader.rs:1188` 以降)。agent の実 worker はその return で終了できるため、
  現在の controller の join/Permit 保持だけでは native job 終了までの枠保持にならない。

これは「取消付き API がある」という前提自体の否定ではなく、指定の既存 API が
agent の必須のログ/所有契約を同時には満たさないという矛盾。
`get_thumbnail` の render には匿名入口と HarvestOnCancel の追加が明記されているが、
`list_folder` の PDF enumerate にも同じ分離が必要なことは設計にまだない。

設計担当へ渡す最小案は、**PDF enumerate にも agent 専用の匿名・HarvestOnCancel 入口を追加する**こと。
既存 UI/Remote の入口は path perf key / AbortOnCancel のままにし、agent の要求だけを
perf key=None・実応答待ちへ通す。§15.5 の表と取消/ログの契約に追記し、設計判断を確定してから再開する。
現段階でこの変更を独断では実装しない。

このほか確認できた前提:

- `app::folder_scan` / `app::book_materialization` は module を含め `pub(crate)`。
- 本の構成 service は Clone 可能で、初期 snapshot は status=Err。
  `snapshot()` と `current_candidate_snapshot()` の閉鎖後の違いも設計の注意どおり。
- 現在の起動順は agent server → App で本の構成 service を設置 → install_agent_readers。
  外側の終了順も agent → Remote → collection runtime → 本の構成 owner。
- favorite overlay は readonly の DB reader と既存の overlay 関数を再利用できる。
- 縮小画像の primary decode は budget と orientation の契約を持つ。
  ZIP の新旧名は同じ targeted read の各段から取得して連結可能。

着手済みだった IPC 型/依存追加はすべて戻し、コード差分を残さない。残る変更はこの §17 の記録のみ。
製品 binary の起動、commit、cargo fmt/check/test/build は行っていない。
設計矛盾で停止したため、実装・fixture・実 pipe の検証結果および今回の sandbox 外テストコマンドはまだない。
`git diff --check` は exit 0。正式な独立実装レビューは未依頼 (設計レビューの代わりに扱わない)。

### 17.2 PDF 列挙の設計判断を受けて再開

ClaudeCode は agent 専用 PDF enumerate の追加を採用し、§15.5 の PDF 行を作業木で更新した。
agent の入口だけ None perf key + HarvestOnCancel とし、既存 UI/Remote の入口はそのままにする。
実装担当はこの更新を確認して再開した。§17.1 は停止時点の記録として保持する。

### 17.3 追加の前提差分による再停止 (観測者: 実装担当)

再開後のコード照合で次の2点が見つかった。依頼の停止条件に従い、実装を再度停止した。
これは実装担当の source inspection であり、独立レビューや製品の実行観測ではない。

1. **ZIP の collapse と「入れ子の中は一覧しない」の境界**。
   `ZipTree::collapse_redundant` (`src/zip_tree.rs:231`) は「画像0枚・子directoryが1つ」なら降下し続け、
   実際の nested ZIP の境界を検査しない。列挙は入れ子内のページを `inner.zip/page.png` のように木へ入れる。
   よって外側ZIPが内側ZIP1つだけを含む場合、指定の grid と同じ raw/display prefix は内側へ進み、
   root の list_folder でも入れ子内の画像を返すことになる。一方、§15.5 は ZIP-in-ZIP の中を一覧対象外とする。
   明示 subresource の拒否だけでは、この root の自動降下は防げない。
   設計側で、(a) 上限付きの既存 grid 経路のまま入れ子内も一覧可能にする、または
   (b) agent だけ実際の入れ子境界で止める prefix 規則を追加する、等の境界を確定する必要がある。
   UI の collapse を変えたり、root を黙って error にしたりする実装はしていない。

2. **WIC の memory 上限超過の型が失われる**。
   bounded WIC (`src/wic_decoder.rs:194`) は `Option<DynamicImage>` を返す。
   `bounded_wic_buffer_layout` (`src/wic_decoder.rs:317`) の budget 超過も、codec 非対応・破損も None になる。
   `image_decode::Backend::wic` も Option (`src/image_decode.rs:105`) で、`primary_with_backend` は WIC が None の時
   元の primary error を返す (`src/image_decode.rs:222` 以降)。HEIC/HEIF/JXL は静的 SUPPORTED_EXTENSIONS にあり、
   Rust primary が Unsupported、WIC が budget 超過なら、最終 error は Unsupported のまま。
   上限自体は守れるが、§15.5 の「decode 上限超過を too_large にする」をこの API の情報だけでは満たせない。
   最小案は budget 超過を区別する typed bounded WIC 入口を足し、agent の primary 経路へ型を失わず伝えること。
   既存 Option 入口と UI/Remote の挙動を維持する分離を設計に追加する判断が必要。
   上限超過を unavailable に置き換える仕様緩和や、別の無制限 decode は実装していない。

再開用の下書きは `target/mcp-1b-checkpoint-570454c5f.json` に保存した。
基点 HEAD、15ファイルの draft bytes、HEAD側の checksum を記録している。folder handler はまだ未作成で、
ZIP cap・thumbnail・IPC/adapter/PDF匿名入口は途中または未検証の案。完成・compile可能とは扱わない。
保存後、今回触った code/Cargo の差分と新規 source を戻し、追跡対象の変更はこの文書だけにした。
再開時は HEAD と各 HEAD checksum および他者の変更を照合してから draft を再適用する。
この保存データに通常 profile の設定や利用者データは含まれない。

今回も cargo fmt/check/test/build、製品 binary 起動、commit は未実施。
`git diff --check` は exit 0。変更したのは §15.5 (ClaudeCode) と §17 (実装担当) だけ。
設計判断待ちのため、今回の実装に対する sandbox 外テストコマンド・成功結果はまだない。

### 17.4 ZIP/WIC の設計判断を受けた再開と実装

ClaudeCode は §15.5/§15.11 を更新し、入れ子ZIPは grid と同じ木・collapse・prefixで一覧対象とした。
WIC の新しい型付き入口は作らず、1a と同じ header dimension reader から RGBA buffer を見積もり、
256MiB超を decode 前に too_large とする。decode の失敗 (bounded WIC の上限拒否も含む) は unavailable。
安全上限を primary / ImageReader / RAW preview に渡す規則は維持する。
実装担当は基点の各 source checksum と作業木の他者差分を確認して checkpoint を復元し、
ClaudeCode の文書変更と §17.1〜§17.3 を保持して再開した。

実装の選択 (実装担当の確認・判断であり、正式な独立レビューではない):

- 共通 wire は未リリース v2 に Folder/Thumbnails の型を追加。画像は別の images 欄で運び、
  WebP bytes は base64 として JSON に載せる。1MiB を JSON 数値配列で運んで4MiB frameを超えることを避ける。
  adapter は metadata の page だけを text/structuredContent に置き、images は content の image blockだけへ投影する。
- list_folder は grid の純粋 materializer を worker から呼び、現在設定と readonly favorite overlay を使う。
  ZIP の raw scopeから構成keyを決め、保存構成がある時はraw prefix、無い時はgridと同じdisplay collapseを使う。
  PDF は readonly pdf_meta→匿名列挙の順で読み、visibilityを構成投影の前に掛ける。
- SharedReaders には BookCompositionService の clone だけを遅れて設置する。
  Appの install_agent_readers は service 設置後に呼ばれ、短いlockだけでcloneを渡す。
  LiveFavorites・settings接続を追加せず、Remoteの通常処理やsession/cacheは使わない。
  Agentの一覧は snapshot() のみを読み、close_admission 後も読む。未設置/status異常は NotReady。
- 入れ子ZIPは既存列挙へ明示的な任意上限を足す。既存callerはNone、agentだけ64MiB/256MiB。
  確保前・読出し途中・cachehitを検査し、祖先stackで保持する Vec capacity の合計も数える。
  兄弟を離れた後の共通nested cacheは既存のprocess budgetのownerであり、agent専用cacheは持たない。
- 縮小画像は指定項目だけを読み、catalog/代表/process_load_request/類似索引/移行を通らない。
  EXIFを一度適用し、readonly回転を焼き込む。concealは項目自身の新旧keyを読み、DB読取り失敗は呼出し全体のerror。
  新旧ZIP名は同じtargeted readの各段で取り、ネストprefixを各々連結する。1aの★も同じ旧key fallbackを使う。
- RAWの見積りは実際の埋め込みpreviewのheader/割当上限を使う。full現像の寸法で小さいpreviewを拒否せず、現像しない。
  その他の画像は既に上限付きで読んだbytesのheaderを1aの readerで調べる。未知寸法はdecodeの安全上限へ委ねる。
- 共通I/Oは既存 GlobalIoSemaphore::process_shared(current profile permits) を使う。
  ownerは同一で、holderやtray throttleを解除しない。取得待ちは取消を受ける。
- long_sideは既定256、256未満はBadRequest、512超は512へ丸める。最大20入力。
  list_folderは既定100、0はBadRequest、200超は200へ丸める。cursorはargs digestと全件identity digestを照合する。
  本文・画像の予算を超える前にページを切り、入力位置のcursorで続ける。すべてread-result再照合の対象。

所有と副作用・期限の fixture、実行コマンドと結果は以下に追記する。

### 17.5 検証の範囲と途中の修正 (観測者: 実装担当、この sandbox 内)

初回の core check は exit 0、IPC/MCP crate 検査は exit 101 (IPC 11成功、MCP 47成功/2失敗、ignore 0)。
失敗は既知の real pipe preflight の OS 5 の2件だけ (`target/mcp-1b-crates-initial.log`)。
adapter build は exit 0 (`target/mcp-1b-adapter.log`)。
初回 focused nextest は test compile の exit 101 (`target/mcp-1b-focused-initial.log`)。
旧 PDF enrichment fixture だけが追加の legacy key 引数を渡しておらず、None を渡すよう追従した。
その回はテストを実行できておらず、成功件数へ数えない。

実装担当の追加確認で、folder cursor の args は物理パスを正規化していたが、一覧の識別は返却pathの表記を
そのまま使っていたため、slash/case/verbatim 表記だけを変えた次頁で偽の一覧変更を検出し得た。
args と一覧の物理パスには同じ pure normalization を使い、ZIP subresource の綴りは保持するよう修正した。
表記違いの継続成功と subresource の変更拒否を pure test で追加した。設計の新たな矛盾ではなく実装の修正。

- 一覧 fixture は mixed kinds/visibility/重複の畳み方、favorite overlay、ZIP 内フォルダと構成順/hidden、
  nested root collapse/明示 prefix、PDF visibility/構成、catalog miss、未設置/異常 snapshot、
  全件 identity cursor、文字数と行数、pre-cancel、永続ファイル不変を検査する。
  readonly SQLite が共有memory管理に使う `-shm` は byte比較の対象外とし、DB/WAL等は比較する。
- ZIP 上限は確保前、cachehit の Vec capacity、祖先合計/兄弟解放、宣言を超える実読取り、取消を検査する。
- 縮小画像は JPEG EXIF＋user rotation、PNGとZIP、nested CP932旧 conceal/rotation/rating、
  conceal DB失敗、unsupported、animationの先頭、RAWに通常decodeを使わないこと、入力/header/decode予算、
  cursorによる再読取り回避を検査する。cache未作成と、実際のcatalog候補をpoisonした後の全cache inventory/bytes不変も検査する。
- Controller の実 request 経路に clock/worker を注入し、thumbnail の許可off admission、生成中/送信直前の変更による
  画像丸ごとの破棄、thumbnail prefix/全件型 Timeout、実 worker終了まで両slot保持を検査する。
  SharedReadersのsnapshotがclose_admission/owner shutdown後も読めることも検査する。
- PDF の匿名 enumerate/render は pure executor で production wait loop を通し、perf key=None、
  cancel後もactual completion待ち、成功/parse error/transport errorを検査する。native PDF worker は起動しない。
- 既存 stdio adapter integration に list_folder/get_thumbnail を追加し、fixtureのPNGから返る実WebPと
  metadata/textの一致を検査する。通常の製品processではなく、許可済みのfixture専用adapterを対象とする。

正式な独立レビューは別 thread。ここまでの確認や補助担当の点検を正式レビューの代わりとは扱わない。
最終コマンド・実 exit code は以下に追記する。

2回目の focused は exit 100、241件中236成功 (既存10万件の実走査benchmark 1件を含む)、5失敗だった
(`target/mcp-1b-focused-final.log`)。既知の pipe OS 5 の2件に加え、次の fixture 3件を修正した。

- PNG の write_header だけの入力は IDAT がなく、寸法 reader が None を返す不完全データだった。
  その場合の unavailable は維持し、正常な小さいPNGのIDATを保ってIHDR寸法とCRCを更新したfixtureで
  寸法を取得できることと、file/ZIPともdecode前に too_large になることを検査する。
- limit 1000 のページは件数上限200より先に文字数予算に達して176件になっていた。
  件数の期待値を無条件200とせず、limit200と1000のページ全体の一致と、文字数・cursorの検査を維持する。
- cached PDF の全bytes比較は、fixtureの初回共有settings起動とSQLiteの補助WALの寿命を同時に測っていた。
  差分は settings.db-wal と起動ログ、catalogの空WALで、DB本体とbook compositionは同一だった。
  通常の共有settings初期化をbaseline前に行い、既存catalog writerをreader検査中保持する。
  DB/WALを含む比較の除外は増やさない。差分の診断も巨大byte配列ではなくpathとsizeで示す。

これらは test-only の修正で、production source は変えない。
成功済みの `section1375_large_physical_scan_10k_100k` (151.579秒) は同じ走査実装の有効な結果として再利用し、
修正後の focused filter ではこのbenchmarkだけを除外する。0件一致や中断を成功として扱わない。

### 17.6 最終検証と引き渡し

観測者: 実装担当、この sandbox 内、`570454c5f` + この未コミット差分、通常 dev/test profile。

| コマンド | 実 exit code | 結果 / ログ |
|---|---:|---|
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 | 最終 production 差分。`target/mcp-1b-core-final.log` |
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` | 101 | IPC 11成功、MCP 47成功/2失敗、ignore 0。`target/mcp-1b-crates-initial.log` |
| 下記 focused nextest | 100 | 240件中238成功/2失敗。`target/mcp-1b-focused-verified.log` |
| 既存10万件走査 (前回の同じproduction差分の結果を再利用) | 成功 | 151.579秒。含むrunの exitは100で他の失敗による。`target/mcp-1b-focused-final.log` |
| `cargo build -p mimageviewer-mcp --bin mimageviewer-mcp` | 0 | `target/mcp-1b-adapter.log` |
| `cargo fmt` (workspace、引数なし) | 0 | 最終sourceを整形 |
| `cargo fmt --check` | 0 | `target/mcp-1b-fmt-verified.log` |
| `git diff --check` | 0 | 最終差分を確認 |

最終差分の新規 fixture・既存回帰は実 pipe の4件を除いて成功した。
失敗は ignore/skip 化せず、DACL も緩めない。core は idle-connected shutdown と新2toolを追加したstdio fixture、
adapter は withheld-response cancel と blocked-stdout cancel の、pipe作成/接続時の OS 5。
core check・adapter build・crate 検査後の最後の修正は core test-only fixture なので、その成功結果を再利用する。
FFmpeg DLL は既に配置したこのworktree内の実ファイルを使用し、リンク/ジャンクションは作っていない。

変更ファイル一覧:

| 役割 | ファイル |
|---|---|
| wireと予算 | `crates/agent-ipc/Cargo.toml`, `src/lib.rs`, 新規 `src/folder_types.rs` |
| MCP adapter/schema/回帰 | `crates/mcp-server/Cargo.toml`, `src/lib.rs`, `src/schema.rs`, `src/tests.rs`, `Cargo.lock` |
| agent dispatch/reader設置 | `src/agent_ipc/mod.rs`, `src/remote_ipc/mod.rs` |
| 一覧 | 新規 `src/agent_ipc/folder.rs`, `folder_tests.rs`, `src/app/folder_scan.rs`, `src/zip_loader.rs` |
| 縮小画像 | 新規 `src/agent_ipc/thumbnail.rs`, `thumbnail_tests.rs` |
| ZIP新旧keyの共有/★ | `src/agent_ipc/item_info.rs`, `item_info_tests.rs` |
| PDF匿名入口 | `src/pdf_loader.rs` |
| controller/stdio回帰 | 新規 `src/agent_ipc/media_read_tests.rs`, `src/agent_ipc/tests.rs` |
| 設計判断と台帳 | `docs/mcp-integration-plan.md` (§15はClaudeCode、§17は実装担当) |

ClaudeCode が sandbox 外の通常権限で実行するコマンド (この最終差分での外側の成功は未観測):

```powershell
cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored
cargo nextest run -p mimageviewer --lib -E '(test(/agent_ipc/) or test(/agent_level/) or test(/agent_permission/) or test(=settings_db::tests::failed_permission_commit_preserves_snapshot_and_epoch) or test(/remote_ipc::live_favorites/) or test(/settings_transfer::tests::automatic_network_enablement/) or test(/png_metadata/) or test(=pdf_loader::tests::anonymous_page_sizes_keep_worker_path_out_of_diagnostics_on_all_results) or test(=pdf_loader::tests::page_sizes_preserve_existing_ui_diagnostic_identity) or test(=pdf_loader::tests::page_sizes_round_trip_and_reject_a_truncated_reply) or test(/app::folder_scan/) or test(/zip_loader/) or test(=pdf_loader::tests::anonymous_agent_render_and_enumerate_omit_keys_and_harvest_actual_completion) or test(=pdf_loader::tests::cancelled_anonymous_render_does_not_enqueue)) and not test(/section1375_scan_perf_tests/)'
```

正式な独立実装レビュー、実 pipe の最終成功、実 native PDF worker/WIC/RAW実画像での確認は未実施。
通常profile・製品binary・UIを起動せず、commit、test-full、snapshot更新、公開文書更新は行っていない。
依頼されたconsole adapter binaryは生成済み。coreの実機確認用buildはこのsandboxで実pipe gateが通らないため未実施。
1c/1d/1eへは進んでいない。途中の設計前提3件はClaudeCodeの判断で解消し、追加の設計矛盾や未解決の実装上の指摘はない。

### 17.7 修正 1 回目

前回成果の sandbox 外の観測者は ClaudeCode。`b66a63687` の段階1bについて、
`cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` は exit 0
(11/11、49/49)、§17.6 の focused nextest は exit 0 (240/240) と報告された。
今回の正式な独立レビューは別スレッドで行われ、ClaudeCode が次の P1/P2 を確認・採用した。
以下の実装・検証は実装担当自身のもので、独立レビューの代替ではない。

#### 修正と前提の確認

- **P1 (path の別表記による「隠す」の迂回)**: `normalize_path` は小文字化と区切りの置換だけで、
  `list_folder` / `get_item_info` / `get_thumbnail` の入口はそれぞれ別の検査だった。
  `src/agent_ipc/input_path.rs` に1つの検査関数を置き、実ファイルと ZIP/PDF の container path、
  cursor の引数生成をこの入口へ揃えた。絶対 drive/UNC path、NUL/制御文字・dot成分・namespace接頭辞・
  ADS・成分末尾のdot/空白・重複区切りの排除を検査し、`GetLongPathNameW` の結果と
  `CompareStringOrdinal` のcase無視で一致するものだけを受ける。短い名前は tool error
  「パスは mIV の一覧と同じ表記で指定してください」。存在しないpath (OS 2/3) は元の表記のまま
  各toolの既存not-found処理へ渡し、その他の照合失敗はInternalとして閉じる。pathをログへ渡さない。
  `canonicalize` / linkの解決は使わず、junction/symlinkはgridと同じ別項目のまま。
  cursor は許可済みの表記の大小文字/区切りだけを揃え、dot成分やnamespace接頭辞を解消しない。
  入力配列全件をcursorの解釈より先に検査するので、continuationで読み飛ばす項目にも適用する。
- **P2 (大きなJPEGの縮小取得)**: 元の寸法の `width * height * 4` 検査がDCTの前にあった。
  JPEGはまず既存の256MiB出力上限付き縮小DCTを試し、原寸の見積もりはそのfallbackまたは非JPEGの
  full-size decode直前に移した。file128MiB/ZIP64MiB、decode256MiB、画像/応答予算、取消の契約は維持。
  UI / Remote、共有decoderの本体、wire形式、永続DB/schemaは変更していない。

#### 回帰

- 新規 `input_path_tests.rs`: dot/dotdot、verbatim/device、ADS、末尾dot/空白、NUL、重複区切りを
  filesystem照合前に拒否。8.3名はvolumeの短名生成設定に依存しないmock long-name lookupで確認。
  Unicodeを含むcase/区切り、drive/UNC、link名の保持、存在しないpath、照合失敗のfail-closedも確認。
- 隠した実画像、ZIP内画像、PDFページの通常表記は `hidden_by_edit` で添付なし。
  同じ各containerの別表記は3toolすべてで指定のBadRequestとなり、画像を返さない。
  continuationでskipする入力も拒否し、通常表記の不存在pathの既存結果も確認する。
- `thumbnail_tests.rs`: 12000×8000、約375KiBの完全なbaseline grayscale JPEGを合成。
  fixture生成で原寸rasterを確保せず、実ファイルとZIP内画像の両方が `ok`、WebPの寸法がmetadataと
  一致することを確認する。既存の非JPEG原寸上限・入力上限・conceal/旧key回帰は維持。
- 既存folder cursorテストは許可される大小文字/区切りの差だけを継続対象とし、dot/verbatimは
  共通入口の拒否テストへ移した。

#### 実装担当の検証

観測者: 実装担当、このsandbox内、`901dee5a6` + 本修正、通常dev/test profile。

| コマンド | 実 exit code | 結果 / ログ |
|---|---:|---|
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 | 最終production差分。`target/mcp-1b-fix1-core.log` |
| 下記 narrow nextest | 0 | 新規6件+既存cursor1件、7/7成功。`target/mcp-1b-fix1-narrow.log` |
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` | 101 | IPC11成功、MCP47成功/2失敗、ignore0。失敗は実pipeのOS5。`target/mcp-1b-fix1-crates.log` |
| §17.6 focused nextest (初回) | 100 | 243成功/3失敗。verbatim容認の旧cursorテスト1件の更新漏れを修正、ほか2件はOS5。`target/mcp-1b-fix1-focused.log` |
| §17.6 focused nextest (最終) | 100 | 246件中244成功/2失敗。OS5以外の失敗なし。`target/mcp-1b-fix1-focused-final.log` |
| `cargo build -p mimageviewer-mcp --bin mimageviewer-mcp` | 0 | `target/mcp-1b-fix1-adapter.log` |
| `cargo fmt` (workspace、引数なし) | 0 | 最後のcursorテスト修正後も実行 |
| `cargo fmt --check` | 0 | `target/mcp-1b-fix1-fmt.log` |
| `git diff --check` | 0 | 最終差分を確認 |

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/agent_ipc::input_path/) or test(/large_jpeg_file_and_exact_zip_member/) or test(/folder_cursor_continues_across_equivalent/)'
```

最終のcore失敗は `agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry` と
`agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers`、adapter失敗は
`pipe::tests::real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld` と
`pipe::tests::real_pipe_cancel_reaches_core_while_stdio_stdout_is_blocked`。
いずれもfixtureの実pipe作成/接続時のOS5で、skip/ignore化、DACL変更は行っていない。
core check・crate tests・adapter build後の変更は既存coreテストの期待とこの台帳だけなので、
同じproduction/crate sourceに対する結果を再利用する。FFmpeg DLLも配置済みの実ファイルを使用。
走査器やZIP/PDF共有readerの実装は今回は変更しておらず、10万件走査の再実行やtest-fullは行わない。

変更ファイル: 新規 `src/agent_ipc/input_path.rs`, `input_path_tests.rs`、
既存 `src/agent_ipc/mod.rs`, `folder.rs`, `folder_tests.rs`, `item_info.rs`, `thumbnail.rs`,
`thumbnail_tests.rs` と本台帳。新たな設計矛盾はなく、今回修正のsandbox外の実pipe成功と
正式な再レビューは未観測。製品binaryの起動、commit、UI/snapshot更新は行っていない。
実pipe gateがこのsandbox内で通らないため、通常profileのcore確認用buildは行わず、
依頼されたconsole adapterだけをbuildした。

ClaudeCode が sandbox 外の通常権限で実行するコマンド (今回の修正後の成功は未観測):

```powershell
cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored
cargo nextest run -p mimageviewer --lib -E '(test(/agent_ipc/) or test(/agent_level/) or test(/agent_permission/) or test(=settings_db::tests::failed_permission_commit_preserves_snapshot_and_epoch) or test(/remote_ipc::live_favorites/) or test(/settings_transfer::tests::automatic_network_enablement/) or test(/png_metadata/) or test(=pdf_loader::tests::anonymous_page_sizes_keep_worker_path_out_of_diagnostics_on_all_results) or test(=pdf_loader::tests::page_sizes_preserve_existing_ui_diagnostic_identity) or test(=pdf_loader::tests::page_sizes_round_trip_and_reject_a_truncated_reply) or test(/app::folder_scan/) or test(/zip_loader/) or test(=pdf_loader::tests::anonymous_agent_render_and_enumerate_omit_keys_and_harvest_actual_completion) or test(=pdf_loader::tests::cancelled_anonymous_render_does_not_enqueue)) and not test(/section1375_scan_perf_tests/)'
```

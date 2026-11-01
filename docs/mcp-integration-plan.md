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
  ログ (sidecar 走査失敗時の実パス、tag writer の更新後タグとパス等) は、通常のログ (常時 ON) の
  既存方針に従う。読み取りの tool は MCP 用の投影でこれらの処理を通さない (独立レビュー P2-11)。
  段階 2 の書き込みは UI と共有の処理を通るので、利用者の操作と同じく変更した path・タグ名が
  通常のログに残る (§21.4。2026-11-01 に「利用者が有効にした診断ログ」という前提を直した)。
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

### 11.3 利用者の回答 (2026-11-01、段階 2 の設計前)

- U11: AI の変更を元に戻す機能は作らない。記録を見て手で戻す (§21.1)。
- U12: 変更の記録は mIV を閉じるまで (メモリだけ) (§21.1)。

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
  `get_collection` が返した path をそのまま渡せる必要がある。代わりに次で照合する。
  - **隠す編集は要求側の段ごとの名前で照合する** (1b 実装レビュー 3・4 巡目 P1): 短い名前は path の段ごとに混ざり得る
    (`C:\LONGDI~1\Secret photograph.png` など) ので表記の候補を並べても閉じない。また保存された key は小文字化済みで
    元の path に戻せない (Unicode の小文字化で別の名前が同じ形になる) ので、key をファイルシステムで解決することもしない。
    よって、要求の path (ZIP/PDF は container) について、段ごとの長い名前 (`GetLongPathNameW`) と短い名前
    (`GetShortPathNameW`。8.3 が無ければ長い名前と同じ) を取り、それぞれを key と同じ規則で正規化する。隠す編集の DB の
    key (読み取りの失敗を返す列挙) の path 部分を同じ数の段に分け、**どの段も、その段の長い名前か短い名前の正規化と一致**
    すれば同じ項目とみなす。ZIP は項目名の新旧との組み合わせ、PDF はページの key。正規化で別の名前が同じ形になる場合は
    隠す側に倒れる (fail closed)。ファイルシステムへの問い合わせは要求の path についてだけで、その結果を覚える時の key は
    元の path (小文字化しない)。既存の DB は変えない。
  - 回転は表示の補助なので、渡された表記・長い名前・短い名前の 3 つの key で引く (隠すほどの厳密さは要らない)。
  - 名前の解決が失敗した時は、要求の path が存在しないなら各 tool の既存の扱い、それ以外は tool error。junction・symlink で別の名前を付けたものは一覧画面でも別の項目として扱われる (編集の key も別) ので、
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
- **ログ** (実装担当が着手前に指摘、ClaudeCode が判断): agent の受付 (worker・queue・UI の判定・拒否) では path を
  記録しない。受理の後は二重起動と同じ通常の open なので、既存の open / load の診断ログ (要求 path・解決先を記録する)
  はそのままにする。開いたフォルダは画面にも履歴にも残る通常の表示状態で、受理後まで秘匿するには非同期の open 経路へ
  由来を伝える状態が要り、「新しい open 経路を作らない」に反するため。
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

2026-11-01 にコードを確かめて詳細化した (経路は実装 brief に file:line で書く)。

**ページ**

- 環境設定に「AI 連携」のページを新しく作り、「起動と連携」の下に置く。段階 0 で「全般」に置いた 1 行
  (許可の範囲の ComboBox) はこのページへ移す。設定の検索の索引も移す (全 anchor に索引が要る既存のテストがある)。
- ページの上から:
  1. **許可の範囲** (§15.2): 実装済みの 3 段 (無効 / 読み取りのみ / 読み取りと、mIV で開く) の選択。
  2. **縮小画像**・**撮影位置** のチェック (既定 OFF)。
  3. **見せる範囲の 3 欄** (§5.6): 「AI に見せるもの」「見せないもの」「AI に許す操作」を、編集中の設定の値から
     毎 frame 作る (固定文ではない)。選択やチェックを変えるとその場で欄が変わる。実際の許可は他の設定と同じく
     OK で確定する。3 項目のどれかが確定済みの値と違う間は、欄の上に「編集中の設定です。OK で適用されます」と出す
     (設計レビュー 12 巡目 P2-1)。欄に含める文 (1b・1c で決まったもの):
     - 縮小画像 ON の時「縮小画像 (補正・編集は反映しない。隠す編集をした画像は渡さない。動画・書庫の代表は渡さない)」。
     - 撮影位置 OFF の時、撮影位置は「見せないもの」の欄。
     - 「mIV で開く」の段では「AI に許す操作」に「mIV の画面で開く (表示が切り替わる)」。
  4. 欄の下に固定の注意: 「この情報は、お使いの AI アプリ (Claude / ChatGPT) の提供元へ送られます。各社の
     プライバシーポリシーに従って扱われます。mIV の開発者には送られません」「許可の範囲の中の操作は、AI アプリの
     確認なしで実行されます。ここで選ぶ範囲が唯一の上限です」(§5.5)。
  5. **最近使った AI アプリ** (下)。
  6. **AI アプリごとの行** (Claude Desktop・ChatGPT) (下)。
- 配色・文言・UI の規則は coding-conventions に従う (色は `ui.visuals()` の値、`check_ui_glyphs.py`、編集不可の長文は
  選択できる label + コピーのボタン。生の `TextEdit` は使わない)。

**最近使った AI アプリ**

- 表示: AI アプリが名乗った名前・版と、最後に tool を実行した時刻。保存しない (mIV を閉じると消える)。最大 8 件、古い順に落とす。
  自己申告の値なので、名前 128 文字・版 64 文字で切り、「AI アプリが名乗った名前です」と添える。
- 運び方: agent IPC の `Hello` に、名前に加えて**版**と**要求の種類** (`tool` / `internal`) を足す (版 2 は未リリース、
  互換は持たない)。2026-07-28 版の `clientInfo` は省略できるので、無い時は「(名前なし)」として要求は処理する。
  **`Hello` は版と名乗りを受け取るだけの軽い処理にし、許可の判定と同時実行の枠を外す** (今は `Hello` で許可を判定し、
  無効なら adapter が tool の要求を送らずに終わるので、拒否を記録できない。設計レビュー 12 巡目 P2-3)。許可・混雑の判定は
  tool の要求の側で行う (今の要求の判定と同じ場所)。MCP exe は、旧版の握手では `initialize` の `clientInfo` (名前・版) を接続の間持ち、2026-07-28 版では
  各要求の `_meta` から取る。**5 秒ごとの状態の照会と `tools/list` から作る照会は `internal`**、AI が呼んだ tool
  (`miv_status` を含む) は `tool`。
- core は `Connection` に名前・版・種類を持ち、`tool` の要求の結果を記録する所 (既存の `record`) で、共有の
  「最近の AI アプリ」の表 (`Arc<Mutex<..>>`) の時刻を更新する。拒否 (無効・混雑を含む)・時間切れも「実行した」に数える
  (呼ばれた事実を示す)。`Hello`・`Cancel`・`internal` は数えない。MCP exe の中で終わる引数の検証の拒否 (core に届かない) も
  数えない。表は既存の `install_agent_open` と同じ形で App に渡し、UI は表示の時に読むだけ。

**AI アプリごとの行**

各行に、検出・登録状態・「登録する」「登録を外す」「手順を見る」を置く。登録・解除は SendTo や TensorRT の pack と同じく
**即時実行** (OK / キャンセルとは無関係) で、そう明記する。

- **編集中は登録できない** (設計レビュー 12 巡目 P2-1): AI 連携の 3 項目のどれかが確定済みの値と違う間は「登録する」
  「登録を外す」を押せなくし、「OK で設定を確定してから登録してください」と出す。編集中の許可と確定済みの許可が食い違った
  まま登録する組み合わせを無くす。確定済みの許可の範囲が「無効」なら「登録しても、許可の範囲を選ぶまで AI アプリからは
  使えません」と添える (確定済みの値から判定する)。
- **登録・解除は確認の窓を開いた時からモーダル** (設計レビュー 12 巡目 P2-2、13 巡目 P2): 処理の状態は環境設定の状態の中の
  1 つの値 (`Option<登録の処理>`、確認中 → 書き込み中) で持ち、**確認の窓を開いた時から**、確認を取り消すか結果を受け取るまでの
  間は、環境設定の本体 (全ページの設定の編集・OK・キャンセル・×・他の行の登録・解除・再確認) を押せなくする (待つ間も UI は
  止めず、channel を poll する)。確認中に背面の AI 連携の設定を変えて、編集中の値と確定済みの値が食い違ったまま書く経路を無くす。閉じた後に書き込みが続く・閉じて
  開き直した後の操作と競合する、という組み合わせを無くす。結果を受け取ったら状態を戻し、その行の状態を再確認する。

- **登録する command** (§15.8 と同じ規則。どの配布形態でも**解決済みの絶対の `--data-dir` を必ず付ける**。状態を減らすため
  「既定の data dir なら省く」を作らない。`data_dir::get()` は相対 path を返し得るので、agent-ipc の `resolve_data_dir` と同じ
  解決で絶対 path にする。設計レビュー 12 巡目の回答 3):
  - launcher から起動された時 (`MIV_LAUNCHER_EXE_PATH` がある): `<launcher の絶対 path> --mcp --data-dir <data dir>`。
    launcher の `--mcp` は 1e で作る。それまでは開発中の launcher で使うことは無い。
  - それ以外 (portable・開発の起動): `current_exe()` と同じフォルダの `mimageviewer-mcp.exe` に `--data-dir <data dir>`。
    その exe が無ければ「登録する」を押せなくし、理由を出す。
- **検出と登録状態の判定は worker** で行う (UI thread で設定ファイルを読まない)。ページを開いた時と「再確認」で走らせ、
  結果を channel で受ける (preferences の外部ツールの確認と同じ形。`Disconnected` は「確認に失敗しました」と表示する)。
  - 状態: 「見つからない」「未登録」「登録済み」「登録先が違う」(`mimageviewer` の項目があるが command・args が今の値と違う。
    mIV を移した・data dir が違う) 「設定ファイルを読めない」(JSON・TOML として読めない。その時は書かない)。
  - 「登録先が違う」の時は「登録し直す」を出す。起動時に黙って書き換えることはしない。
- **登録・解除の流れ**:
  1. 押すと確認の窓 (TensorRT の pack の削除と同じ形) に、書き換えるファイルの path と、足す / 消す内容を出す。
  2. 「書き換える」で worker が次を順に行う: (a) 対象のアプリが動いていないことを確かめる。動いていれば書かずに
     「<アプリ> を終了してから押してください」(終了の待ち合わせ・自動終了はしない)。(b) ファイルを読み直す。(c) 前の内容を
     `<ファイル>.mimageviewer.bak` に上書きで 1 つ残す。(d) 一時ファイルに書いて既存の `replace_file_atomic` で置き換える。
  3. 結果を行に出す。成功なら「<アプリ> を起動して、新しいチャットで mIV の tool が出るか確かめてください」。
  - 失敗はまれな失敗の既定の割り切り (ログ + 表示、再試行しない)。ログに設定ファイルの中身は出さない。
- **Claude Desktop**:
  - **インストール形態と設定ファイルを対応させる** (設計レビュー 12 巡目 P2-4):
    - Squirrel 版 (`%LOCALAPPDATA%\AnthropicClaude` が在る) → `%APPDATA%\Claude\claude_desktop_config.json`。
    - MSIX 版 (`%LOCALAPPDATA%\Packages\Claude_*` が 1 つだけ在る) → その package の
      `LocalCache\Roaming\Claude\claude_desktop_config.json`。
    - 両方が在る・`Claude_*` が複数在る → 自動の登録・解除をせず「Claude が複数の形で入っています。手順を見て、お使いの
      Claude の設定ファイルに書いてください」と出し、候補の path を「手順を見る」に並べる。
    - どちらも無い → 「見つからない」。
    設定ファイルが無ければ `{"mcpServers": {...}}` だけの新しいファイルを作る。
  - 動作中の判定: **実行ファイルの path で判定する** (`%LOCALAPPDATA%\AnthropicClaude\` の下、または
    `WindowsApps\Claude_*` の下)。名前 `claude.exe` だけでは判定しない (Claude Code の CLI も `claude.exe`)。
    process の列挙は `runtime_cleanup.rs` の `K32EnumProcesses` + `QueryFullProcessImageNameW` を、**全 Windows の構成で
    動く共通の関数**に取り出して使う (今の関数は portable の構成で空を返す。設計レビュー 12 巡目の回答 2)。
  - 書く内容: `mcpServers.mimageviewer = {"command": "...", "args": [...]}`。他の項目には触れない。**最上位と
    `mcpServers` の key の順序を保ち、他の値を変えない** (`serde_json` の `preserve_order` を crate 全体で有効にする方法は
    使わない。全体の JSON の挙動が変わるため)。整形 (字下げ 2) は変わってよい。
  - Claude Desktop の設定ファイルに tool ごとの許可を書く項目は文書で確認できないので、手順の文に
    「初回の確認画面で『常に許可』を選んでください」と書く (U9)。
- **ChatGPT (Codex)**:
  - 設定ファイル: `CODEX_HOME` があれば `%CODEX_HOME%\config.toml`、無ければ `%USERPROFILE%\.codex\config.toml`。無ければ作る。
  - 検出: `Packages\OpenAI.Codex_*` か `%USERPROFILE%\.codex` が在れば「入っている」。
  - 動作中の判定: `WindowsApps\OpenAI.Codex_*` の下の process、または名前が `codex.exe` の process (Codex CLI・IDE 拡張。
    同じ `config.toml` を共有し、同時に書くと他方の変更を失う。§15.7 初版の独立レビュー P2-11)。
  - 書く内容: `[mcp_servers.mimageviewer]` に `command`・`args`・`default_tools_approval_mode = "approve"`。
    `toml_edit` (直接の依存に足す) で他の表・コメント・書式を残す。改行コード (CRLF / LF) はファイルのものを保つ。
    `approve` が「確認なし」の意味かは文書で明言されていないので、1d の後の実機確認で見る (未確認)。
  - 「設定を開く」ボタンで `codex://settings` を開く (既存の `open_url` は http(s) だけを通すので、この固定の 1 つだけを
    `opener` で開く)。
- **手順を見る**: 自動登録を使わない人向けに、設定ファイルの path、足す内容 (JSON / TOML、選択できる表示 + コピー)、
  各アプリの画面での手順 (Claude: 終了 → 書く → 起動、ChatGPT: Settings > MCP servers > Add server > STDIO → 再起動) を出す。
- 段階 0 の試験で使った `mimageviewer-trial` の項目は扱わない (利用者が手で消し済み)。
- Grok は対象外で、行も置かない (§11.1 U5)。

**テスト**

- 見せる範囲の欄の生成 (段 × 縮小画像 × 撮影位置の全 12 通り) の単体テスト。
- 設定ファイルの編集の単体テスト: JSON (無い・空・他の項目あり・key の順序・`mimageviewer` の置き換え・解除・壊れた JSON は
  書かない)、TOML (コメント・他の表・CRLF の保持・置き換え・解除・壊れた TOML は書かない)。process の判定は注入した一覧で
  (Claude Code の `claude.exe` を Desktop と誤認しないこと)。
- 「最近使った AI アプリ」: `internal` の照会・`Hello`・`Cancel` を数えない、`tool` は拒否も数える (無効の時・枠が埋まって
  いる時の要求を含む。`Hello` が許可に関係なく通ること)、8 件で古い順に落ちる、名乗りの無い 2026-07-28 版の要求も処理される。
- 環境設定の状態: 編集中の 3 項目が確定済みと違う間は登録・解除を押せない、確認の窓を開いている間と書き込み中は環境設定の
  本体 (設定の編集・OK・キャンセル・×・他の操作) を押せない (確認中に背面の設定を変えようとする入力のテスト)、確認の取消・
  結果の後に戻る、`Disconnected` の表示。Claude の形態の判定 (Squirrel・MSIX・両方・複数・無し)。
- UI スナップショット: AI 連携のページを、無効・読み取りのみ (縮小画像 OFF)・mIV で開く (縮小画像・撮影位置 ON) の 3 状態 ×
  light / dark。加えて geometry・入力のテストで、1093×614 と UI 倍率 200% で、長い path・長いエラーでも、確認の窓の
  固定のボタンと × が見え、ページの末尾まで scroll で届くこと (`docs/ui-snapshot-policy.md` の規則)。確認の窓の Enter / Esc は
  既存の IME の helper を使う。
- 登録・解除の実ファイル書き込みは、テスト用の一時フォルダの path を注入して確かめる。利用者の設定ファイルには触れない。
- 実アプリでの確認 (§9.2): 1d の後に 1 回だけ利用者に頼む。登録 → 各アプリの再起動 → tool が出る・呼べる・確認が出ないか・
  mIV で開く時に窓が前に出るか。

### 15.8 配布の仕組み (1e)

§7 段階 1 の「配布」の契約に、配布形態ごとの登録先を加える (独立レビュー P2-9)。2026-11-01 にコードを確かめて詳細化した。

| 配布形態 | AI アプリに登録する command | 理由 |
|---|---|---|
| インストーラ版・単体 exe 版 | `mimageviewer.exe --mcp --data-dir <解決済みの data dir>` (launcher。1d で `--data-dir` を常に付けると決めた) | runtime フォルダが版ごとに変わるので、利用者が置いた launcher を指す |
| ポータブル版 (zip) | 同梱の `mimageviewer-mcp.exe` を直接、解決済みの `--data-dir` を必ず付ける | ポータブル版の `mimageviewer.exe` は core を改名したもので launcher ではない |

MCP の exe は console subsystem・CRT 静的リンク (DLL 不要)・引数は無しか `--data-dir <path>` だけ・標準入力の EOF で終わる。

**launcher の `--mcp`** (`crates/launcher/src/main.rs`):

- `#![windows_subsystem = "windows"]` のまま。引数の先頭が `--mcp` の時だけ、`--version`・`--help` の判定と、既存の core の activate
  (`try_activate_existing`) より**前**で分岐する。`--mcp` の後ろの引数はそのまま MCP の exe に渡す (検証は MCP の exe が行う)。
- runtime フォルダ (`%APPDATA%\mimageviewer\runtime\<版>`) の決め方・共有の利用 lease (`IN_USE`)・展開の lock は通常の起動と
  同じものを使う。展開するのは **MCP の exe だけ** (core・EffeTune は要らない。起動を速くし、MCP の起動で他の asset を書かない)。
  MCP の exe は小さいので、起動のたびに内容の hash を確かめる一覧 (`requires_content_hash_on_launch`) に入れる。
- 起動: `Stdio::inherit()` で標準入出力・標準エラーを引き継ぎ、`CREATE_NO_WINDOW` を付ける (GUI の親から console の子を起動すると
  新しい console の窓が出るため)。展開の lock は起動の直後に離し、**利用 lease は子が終わるまで持つ** (使用中の版の runtime を
  古い版の掃除が消さない。掃除は動いている process の image がある木も残す既存の守りもある)。`Handoff` (core の起動完了の合図) は
  使わない。
- **`--mcp` の間は名前付き mutex `Global\mImageViewerMcp_v1` を持つ** (子が終わるまで。設計レビュー 15 巡目 P2-2)。AI アプリが MCP を
  使っている間は launcher も動き続け、インストール先の `mimageviewer.exe` が使用中になる。インストーラはこの mutex で検出する (下)。
  **権限** (設計レビュー 16 巡目 P2-1): 生きていることの印として使うだけなので、DACL は「Everyone に `SYNCHRONIZE`、作った人に全権」に
  し、作る時も既存を開く時も `SYNCHRONIZE` だけを求める (`CreateMutexExW`)。こうすると別のユーザーの launcher も同じ object に
  handle を足せ、最後の handle が閉じるまで検出される。core の既存の `Global\mImageViewerInstance_v1` が同じ問題を解いていれば
  その作り方に合わせる。mutex を作れなかった時は標準エラーに 1 行出して MCP は続ける (まれな失敗の割り切り。印が無いだけで
  機能は変わらない)。
- 子の終了コードをそのまま launcher の終了コードにする。`--mcp` の経路の失敗 (展開・起動の失敗) は**標準エラーに 1 行**出して
  終了コード 1。通常の起動の失敗で出す MessageBox と `AttachConsole` は使わない (AI アプリの裏で窓を出さない)。
- `crates/launcher/build.rs` の入力 (`target/release/*.exe`) と `ASSETS`・`include_bytes!` に MCP の exe を足す。入力が無い時の案内に
  build の command を足す。

**ビルド・配布のスクリプト**:

| スクリプト | 変更 |
|---|---|
| `build-release.ps1` / `.sh` | 順序を core → remote → EPUB worker → **MCP** → launcher にする。内側の exe の署名の一覧・release の PE 検査の一覧・止める process の名前の一覧に MCP を足す |
| `build-dist.ps1` | 配布前の clean (`target`・`target-portable` の両方) と最後の PE 検査の一覧に足す。動いている process で build を拒む判定は、下の path の規則に従う |
| `build-portable.ps1` | portable の target での MCP のビルド、同梱の一覧 (`mimageviewer-mcp.exe` の名前のまま)、署名の一覧、止める一覧に足す |
| `build-dev.ps1` | `target\dev-runtime\` に MCP の exe も作る (1d の「登録する」が core の横の MCP の exe を使うため)。止める一覧・PE 検査に足す |
| 安全性のテスト (`test-release-build-safety.ps1`・`test-build-dev-safety.ps1`) | 件数・順序の検査を 4 本に直し、MCP の検査を足す |

- **release の出力先は `<repo>\target\release` に固定する** (実装担当が着手前に 2 度指摘、ClaudeCode が判断): `build-release.ps1` /
  `.sh` は追加の引数を Cargo に渡すが、launcher の埋め込みの入力・止める対象・署名・インストーラの入力は既定の出力先しか見ない。
  出力先は引数 (`--target-dir`・`--target`・`--profile`・`--config` 等)・環境変数・利用者の Cargo の設定ファイルの層のどれでも
  変わり得るので、拒否する一覧では閉じない。次の 2 つで閉じる:
  1. **渡してよい追加の引数を限る**: `--features <x>` / `--features=<x>`・`--all-features`・`--no-default-features`・`--locked`・
     `--frozen`・`--offline`・`-v` / `--verbose`・`-j <n>` / `--jobs <n>` だけを通し、それ以外は process を止める前・build の前に理由を
     出して拒否する。
  2. **実際の出力先を確かめる**: 各 `cargo build` を `--message-format=json-render-diagnostics` で走らせ、Cargo が報告する生成物
     (`compiler-artifact` の `executable`) の path が固定の `<repo>\target\release\<名前>.exe` と一致することを確かめる (更新の
     無い build でも報告される)。一致しなければ、署名・launcher の build・埋め込みの前に止める。
  使っている手順は無い (今の手順は引数なし、または `--features`)。
- **launcher と MCP の exe を止める・build を拒む対象は、その build が書き換える出力の path と一致する process だけ**
  (設計レビュー 15 巡目 P2-1)。AI アプリがインストール版から起動した launcher (`--mcp` で常駐) と MCP の exe は、path が build の
  出力の外なので対象にしない。この 2 つについては **path を取れない process は止めない・拒まない** (既存の「path 不明なら止める」は
  他の名前だけに残す)。`build-release.sh` の名前だけの `taskkill`、release・portable の `PreserveRuntime` の判定 (今は path で絞る前の
  一覧を見ている)、`build-dist.ps1` の拒む一覧も同じ規則にそろえる。テスト: インストール先の launcher・MCP の親子は対象外、
  build の出力の exe は対象、path 不明は対象外、を mock の process 一覧で確かめる。
- **インストーラ** (`installer/mimageviewer.iss`): 既存の独自の確認 (core の mutex を開いて終了を頼む) の後に、
  `Global\mImageViewerMcp_v1` を `OpenMutexW(SYNCHRONIZE)` で確かめる。開けたら「AI アプリ (Claude・ChatGPT など) が
  mImageViewer を使っています。AI アプリを終了してから、もう一度実行してください」と出して中止する。**不在と扱うのは
  `ERROR_FILE_NOT_FOUND` の時だけ**で、それ以外の失敗 (アクセス拒否など) は「確認できませんでした。AI アプリを終了してから、
  もう一度実行してください」と出して中止する (設計レビュー 16 巡目 P2-1) (更新とアンインストールの両方。自動の終了・再接続は作らない。
  通常の操作で起こる組み合わせを、終了してもらう契約で無くす)。内包する exe を列挙したコメントも直す。
  単体 exe 版を手で置き換える時と portable を置き換える時も同じ契約で、`installer/readme.txt`・`installer/readme_portable.txt` に
  「AI アプリを終了してから置き換えてください」と書き、ファイル一覧に MCP の exe を足す。
- `test-full.ps1` の前提 (`target\release` の exe) が 3 本から 4 本になる。CLAUDE.md・`development-build-and-test.md`・
  `native-dependencies.md`・`release-checklist.md`・`release-operations.md`・`portable-build-plan.md` の該当の記述 (ビルドの順序・
  前提の本数・署名の一覧・内包の木) を同時に直す。
- MSIX・winget の manifest は無く、CI は release の exe を作らない (変更不要)。Store はインストーラの提出なので変更不要。
- 1e までは配布物に入れて作れるところまでで、公開しない (U10)。公開文書 (§6) は段階 2 で直す。

**mIV が起動していない時の `tools/list`** (1e の実装レビューの見解を受けて ClaudeCode が判断、2026-11-01): 段階 0 から、core に
繋がらない時の `tools/list` は JSON-RPC の error (-32603) だった。AI アプリを先に起動して mIV を後から起動するのは通常の使い方で
起こり、AI アプリが server を失敗の状態で持ち続ける恐れがある。よって:

- core に繋がらない時の `tools/list` は**成功**にし、`miv_status` だけを返す (`miv_status` を呼ぶと「mIV が起動していません。mIV を
  起動してからもう一度お試しください」の tool error。§8 の既存の文)。
- 旧版の握手の 5 秒ごとの照会は、繋がらない時の tool の集合を `{miv_status}` として比べる。mIV が起動して許可の集合が分かれば
  変化として `notifications/tools/list_changed` を送る (今は繋がらない時を「不明」として比べないので、通知が出ない)。
- 2026-07-28 版は `tools/list` を毎回問い合わせる (`ttlMs=0`) ので、同じ規則で `miv_status` だけを返す。
- 無効 (`Disabled`) の時の空の集合、許可が途中で変わった時 (`PermissionChanged`) に集合を更新しない扱い、core での要求ごとの
  許可の判定は変えない。

**確かめ方** (エージェントは製品の exe と `%APPDATA%\mimageviewer\runtime\*` を起動しない規則):

- launcher の `--mcp` の分岐 (引数の受け渡し・lease を子の終了まで持つ・終了コード・失敗を標準エラーへ・MessageBox を出さない) を、
  runtime フォルダと子の起動を注入できる形にして単体テストする。
- 配布物の全体は、利用者のデータに触れない形で確かめる (UI を伴わない `--mcp` の通信試験として、下の隔離の条件を固定する。
  設計レビュー 15 巡目で妥当と確認):
  - launcher だけ: `build-release.ps1` で作った launcher を、`APPDATA` を一時フォルダに向けた環境で `--mcp --data-dir <隔離の data>` で
    起動し、標準入出力の握手・core が無い時の応答 (起動していない旨)・標準入力の EOF での終了・終了コードを確かめる
    (runtime の展開先・lock は一時フォルダの下、named pipe は data dir と SID から決まるので既存の core と分かれる)。
  - tool の成功: 使い捨ての portable の core を `--data-dir` 付きで起動しておき、上の launcher をその data dir で `--mcp` 起動して
    tool を呼ぶ (adapter は起動済みの core に繋ぐだけなので、core が要る)。
  - **client は launcher を 1 つだけ直接起動する小さな MCP client** (stdio で `initialize`・`tools/list`・`tools/call` を送る script) にする。
    Codex CLI は使わない (利用者の通常の登録や他の設定層を読み、実データを指す mIV の登録まで起動し得る。設計レビュー 16 巡目 P2-2)。
    launcher 単独の試験も同じ client で行う。
  - 確かめる時に、launcher の起動の前後で利用者の `%APPDATA%\mimageviewer\runtime` が変わらないことを見る。
  - mutex のテスト: 2 つの launcher の `--mcp` が並んで動ける、片方が終わっても残りがある間は検出される、最後が終わると不在になる。
- portable は `prepare-portable-smoke.ps1` で作り直し、同梱の `mimageviewer-mcp.exe` を `--mcp` なし・隔離の `--data-dir` 付きで、
  上と同じ直接の stdio client から起動して確かめる (Codex CLI を起動する `mcp_client_smoke.py` は使わない。設計レビュー 17 巡目 P2)。

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

### 15.21 独立レビュー 12 巡目 (2026-11-01、同じ担当、設計 `f4dc7cd0e`、1d)

判定は「1d: not ready」。最近使った AI アプリの分類 (旧版の握手の保持・2026-07-28 版の要求ごとの `_meta`・照会は `internal`)、
画像の path による Claude の判定、Codex の共有設定への配慮、局所的な JSON の順序の保持、`toml_edit`、バックアップと置き換え、
配布形態ごとの command、下書きから毎 frame 作る欄は妥当とされた。

| 指摘 | 判断 | 反映先 |
|---|---|---|
| P2-1 下書きの許可と、即時に行う登録の時に有効な許可を区別していない | 妥当。編集中は登録・解除を押せなくし、欄に「編集中」と出す (表示を分けるより組み合わせを無くす) | §15.7 ページ・AI アプリごとの行 |
| P2-2 登録の worker の所有者と、処理中の再操作・画面を閉じる操作が未定義 | 妥当。1 つの値で持ち、処理中はモーダル | §15.7 AI アプリごとの行 |
| P2-3 `Hello` で許可を判定しているので、無効・混雑の拒否を記録できない | 妥当 (`agent_ipc/mod.rs`・`pipe.rs` で確認)。`Hello` を軽い処理にする | §15.7 最近使った AI アプリ |
| P2-4 Claude の設定ファイルを「最初に在るもの」で選ぶと別の導入先を書く | 妥当。形態と対応させ、決まらなければ自動にしない | §15.7 Claude Desktop |
| 回答 process の列挙は portable で空を返す。data dir は相対の場合がある。テストの追加 | 妥当 | §15.7 |

13 巡目 (設計 `90c88acad`): 12 巡目の P2-2〜P2-4 は解消、P2-1 は入口だけ解消。新しい指摘は P2 1 件
「確認の窓を開いた後から『書き換える』までの間に背面の設定を変えられる (TensorRT の確認の窓は親を無効にしない)」で、妥当。
確認の窓を開いた時から同じ値で持ち、環境設定の本体を無効にする形に直した (§15.7)。`Hello` から許可の判定を外しても、
`Hello` の応答は IPC の版だけで、内部の状態の照会には許可の判定が残るので、無効の時の `tools/list` の挙動は変わらない、と確認された。

14 巡目 (設計 `3286c8090`): 追加の指摘なし。判定は「1d: ready」。

### 15.22 独立レビュー 15 巡目 (2026-11-01、同じ担当、1e)

判定は「1e: not ready」。launcher の `--mcp` の分岐の位置・MCP だけの展開・lock の順序・子の終了までの lease・`Stdio::inherit()` +
`CREATE_NO_WINDOW`・終了コード・標準エラーだけの失敗は妥当、隔離した `APPDATA` での `--mcp` の通信試験も規則の対象外として妥当とされた。

| 指摘 | 判断 | 反映先 |
|---|---|---|
| P2-1 止める・拒む一覧が名前だけの経路 (`.sh`・`PreserveRuntime`・build-dist) を含み、常駐の launcher も拒まれる | 妥当。launcher と MCP は出力の path と一致するものだけ、path 不明は対象外 | §15.8 スクリプト |
| P2-2 常駐の launcher で更新・アンインストールの時にファイルが使用中 | 妥当。`--mcp` の間 mutex を持ち、インストーラが検出して「AI アプリを終了して」と中止 | §15.8 launcher・インストーラ |
| P2-3 core を起動しない検証では tool の成功を確かめられない | 妥当。launcher 単独と、起動済みの portable の core への接続に分ける。通常の登録を読まない | §15.8 確かめ方 |

16 巡目: 15 巡目の P2-1・P2-2・P2-3 は解消 (P2-2 は契約として)。新しい指摘 P2 2 件: (1) 固定名の Global の mutex の既定の DACL では
別のユーザーの launcher が開けず、`OpenMutexW` の失敗は不在と拒否を区別しない → `SYNCHRONIZE` だけの共有と、不在は
`ERROR_FILE_NOT_FOUND` だけにした。(2) Codex CLI は利用者の通常の登録を読み得る → 試験の client を launcher を直接起動する
小さな MCP client にした。どちらも妥当と判断した (§15.8)。

17 巡目: mutex の DACL・要求する権限・インストーラの失敗の判定は解消。新しい指摘 P2 1 件「portable の確認に Codex CLI の経路が
残っている」は妥当で、直接の stdio client にそろえた (§15.8)。

18 巡目 (設計 `97cfcb0a5`): 追加の指摘なし。判定は「1e: ready」。

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

### 17.8 修正 2 回目

前回成果 `b16e16b94` の sandbox 外の観測者は ClaudeCode。両crateの
`cargo test ... -- --include-ignored` は exit 0 (11/11、49/49)、focused nextest は exit 0
(246/246) と報告された。修正前のportable smokeでは、ClaudeCodeがCodex CLI経由で
`samples\.\solid_secret.png` のdot成分付き表記によるconceal迂回を観測し、修正後はその表記を拒否した。
正式な独立レビューは修正1回目の2件を受け入れたが、collectionの保存済み短名が返却後に拒否される
P2を新たに指摘した。ClaudeCodeが採用し、`3d7ffb76f` で§15.5を更新した。

#### 修正と前提の確認

- `library.rs::get_collection` が `entry.source_path` を書き換えずに返すこと、collection登録が
  短名を保持し得ること、conceal/rotationのkey生成が短名を長名へ展開しないことをコードで確認した。
- `input_path.rs::validate` は純粋な構文検査に戻し、長名との一致条件を削除した。
  dot成分、NUL、device/verbatim接頭辞、ADS、末尾dot/空白、重複区切りの拒否はそのまま。
  cursorのkeyも渡された表記のcase/区切りだけを揃え、長名/短名を同一の引数へ置換しない。
- 編集を読む時だけ、同じmoduleの `edit_spellings` が指定表記、`GetLongPathNameW`、
  `GetShortPathNameW` の順に候補を作り、case/区切りで等価なkeyは重複させない。
  Win32の名前展開だけを使い、`canonicalize`、junction/symlink先の解決、migrationは行わない。
  OS2/3は各toolの既存not-found処理へ渡し、その他の名前解決失敗はInternalのtool error。
  OS error/pathはagentのログへ渡さず、通常のrequest記録にerror種別だけを残す。
- `thumbnail.rs` はtypedな `EditItem` と1つのkey投影で、実画像、PDF page、ZIP entryに同じ表記候補を
  適用する。ZIPは各container表記ごとに現在名→旧CP932名を組み合わせる。concealはどれか1つのhitで
  `hidden_by_edit`、rotationは指定→長名→短名、その中で現在名→旧名の最初のhitを使う。
  ZIPのcontainer名は対象bytesを読む前に解決し、lookup失敗を`unavailable`へ吸収しない。
  UI/Remote、共有reader、既存producer、wire/schema、DBの形式・保存内容は変更していない。

#### 回帰

- 新規 `thumbnail_alias_tests.rs`: 実際の短名を `GetShortPathNameW` で取れるvolumeでは実aliasを使う。
  短名が生成されないvolumeでは、同じbytesを持つ2つのfixture fileとthread-localの名前解決注入を使う
  (このbranchを実8.3の検証とは記録しない)。注入はtest buildだけで、実行thread内のscopeを出ると戻る。
- collectionへ短名を登録→`get_collection`の返却path→`get_item_info`と`get_thumbnail`へそのまま渡し、
  寸法と画像を取得できることを検査する。
- 実画像、ZIP現在名/旧名、PDF pageについて、長名のconcealを短名の要求で、短名のconcealを長名の
  要求で検出する。PDFはdecode前のconcealを確かめるため無効なPDF bytesを使用。
  readonly照合でconceal DBが変わらないことも検査する。
- ZIPの6keyを順に外すhandlerテストで、指定現在→指定旧→長名現在→長名旧→短名現在→短名旧の
  rotation優先順を、非対称な画像の実WebP出力で区別する。rotation DBの書き換えもない。
- `input_path_tests.rs` は短名を許可する期待へ変更し、cursorが長名へ置換されないこと、表記候補の
  順序/重複除去、長名/短名どちらのlookup失敗もInternalとなることを確認する。
  dot/prefix等の拒否と3toolのconceal迂回拒否は既存回帰のまま残す。

#### 実装担当の検証

観測者: 実装担当、このsandbox内、`3d7ffb76f` + 本修正、通常dev/test profile。
今回の実装と自身による確認であり、正式な独立再レビューの代替ではない。

| コマンド | 実 exit code | 結果 / ログ |
|---|---:|---|
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 | `target/mcp-1b-fix2-core.log` |
| 下記 narrow nextest | 0 | input_path/thumbnailの20/20成功、新規handler3件を含む。`target/mcp-1b-fix2-narrow.log` |
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` | 101 | IPC11成功、MCP47成功/2失敗、ignore0。実pipeのOS5のみ。`target/mcp-1b-fix2-crates.log` |
| §17.6 focused nextest | 100 | 249件中247成功/2失敗、実pipeのOS5のみ。`target/mcp-1b-fix2-focused.log` |
| `cargo build -p mimageviewer-mcp --bin mimageviewer-mcp` | 0 | `target/mcp-1b-fix2-adapter.log` |
| `cargo fmt` (workspace、引数なし) | 0 | 最終sourceを整形 |
| `cargo fmt --check` | 0 | `target/mcp-1b-fix2-fmt.log` |
| `git diff --check` | 0 | 最終差分を確認 |

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/agent_ipc::input_path/) or test(/agent_ipc::thumbnail/)'
```

失敗したcore2件は idle-connected shutdown とstdio fixture、adapter2件は
withheld-response cancel と blocked-stdout cancelで、いずれもfixtureのnamed pipe作成/接続時のOS5。
skip/ignore化、DACL変更は行っていない。FFmpeg DLLはこのworktreeに配置済みの実ファイルを使用。
共有走査器/decoderを変更していないため、10万件走査・test-fullは再実行していない。

変更ファイル: `src/agent_ipc/input_path.rs`, `input_path_tests.rs`, `thumbnail.rs`、
新規 `thumbnail_alias_tests.rs`、本台帳。今回修正のsandbox外での実pipe成功と正式な再レビューは未観測。
追加の設計矛盾はない。製品binary、UI、portable smokeは起動せず、commitも行っていない。
実pipe gateがこのsandboxでは通らないため通常profileのcore確認用buildは行わず、
依頼されたconsole adapterをbuildした。

ClaudeCode が sandbox 外の通常権限で実行するコマンド (今回の修正後の成功は未観測):

```powershell
cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored
cargo nextest run -p mimageviewer --lib -E '(test(/agent_ipc/) or test(/agent_level/) or test(/agent_permission/) or test(=settings_db::tests::failed_permission_commit_preserves_snapshot_and_epoch) or test(/remote_ipc::live_favorites/) or test(/settings_transfer::tests::automatic_network_enablement/) or test(/png_metadata/) or test(=pdf_loader::tests::anonymous_page_sizes_keep_worker_path_out_of_diagnostics_on_all_results) or test(=pdf_loader::tests::page_sizes_preserve_existing_ui_diagnostic_identity) or test(=pdf_loader::tests::page_sizes_round_trip_and_reject_a_truncated_reply) or test(/app::folder_scan/) or test(/zip_loader/) or test(=pdf_loader::tests::anonymous_agent_render_and_enumerate_omit_keys_and_harvest_actual_completion) or test(=pdf_loader::tests::cancelled_anonymous_render_does_not_enqueue)) and not test(/section1375_scan_perf_tests/)'
```

### 17.9 修正 3 回目

前回成果 `e8f9de393` の sandbox 外の観測者は ClaudeCode。両crateのtestsは exit 0
(11/11、49/49)、focused nextestは exit 0 (249/249) と報告された。
正式な独立レビューはproducer→consumerの修正を受け入れたが、pathの成分ごとに長名/短名が混在する
conceal keyを3候補では検出できないP1を指摘した。ClaudeCodeが採用し、`150a8f855` で
§15.5を「concealは長名identityで比較」に更新した。以下は実装担当自身による実装・確認で、
正式な独立再レビューの代替ではない。

#### 前提・実装・絞り込み

- `normalize_path` はcaseと区切りだけを揃え、長名への展開はしない。DB keyが
  `path::entry` (PDFは`page_N`) であること、既存 `load_conceal_keys` がprepare/query/rowの
  失敗を空集合または行の脱落として扱うことをコードで確認した。
- `ConcealDb::load_conceal_keys_checked` を追加。同じprefix escapeと`__slot_`除外を使い、
  SQLとrow型変換の失敗をResultで伝える。既存APIの本体とgrid/UIの呼び出しは変更していない。
  agentは既存readonly接続でkey文字列だけを1回読み、bitmap/BLOBを展開しない。
- 要求側は `input_path::long_identity` で `GetLongPathNameW` → `adjustment_db::normalize_path`。
  保存側は最初の`::`でpath/entryを分け、pathに`~`がない場合は同じ正規化だけ、ある場合だけ長名解決する。
  ZIPはcontainer identityと現在名/旧CP932名、PDFはcontainer identityとpage keyで照合する。
- **絞り込みは名前解決前のdrive letter / UNC hostとentry/pageの一致**を選んだ。
  checked readは全keyを1回読み、その後にこの条件で候補を絞る。短名化され得る親folderやfilenameを
  prefix条件にしないため、成分ごとの混在を取りこぼさない。無関係なdrive/host/entryは解決しない。
  解決結果は不存在(None)も含め、要求ごとのmapに保存。同じ保存pathに複数entryがある場合や複数の
  要求項目で再照合する場合も、保存pathの解決は呼び出し内で1回。次の呼び出しへは持ち越さない。
- 消された保存path(OS2/3)は飛ばし、それ以外の解決失敗とchecked read失敗はInternalのtool error。
  privateなpath/OS error/SQL内容をログへ流さず、requestのerror種別と既存の固定診断だけを記録する。
  候補走査では各keyでcancelを確認し、既存worker/deadline/permitの契約を維持する。
- 回転は従来どおり指定→長名→短名、各表記で現在名→旧名の候補順。
  構文上のdot/device/ADS等の拒否、cursorの指定表記、producer→consumerも維持。
  DB/schema/保存key、grid、Remote、settings readerの所有、wireには変更を加えていない。

#### 回帰

- 新規 `thumbnail_identity_tests.rs` (9件): mixed表記2種を保存し、long/short要求で、実画像・ZIP現在名・
  ZIP旧名・PDF pageの計16組が画像添付なしの`hidden_by_edit`になることを確認する。
- 正常なSQLiteをreadonlyで開けるがtableがないfixtureがInternalになること、保存済み`~`pathの
  不存在はskipすること、解決失敗は読める画素があってもInternalになることを検査する。
- Some/Noneを呼び出し内で1回だけ解決し、次のcallでは再解決すること、無関係なdrive/UNC host/entryは
  lookupに到達しないこと、UNC hostのcase/区切りの正規化を検査する。名前関係の注入を使うhandler回帰であり、
  native 8.3 filesystemの実機確認としては扱わない。
- `conceal_db` にchecked readのslot除外、table欠落、row型不正、readonly無書き込みの4件を追加。
  既存のalias、producer→consumer、rotation、dot/prefix拒否も同じfocused suiteに含める。
- 自己確認で名前解決の順序も固定した。要求と保存元の両方で、filesystem APIへ渡すのは元の表記で、
  正規化した文字列はidentity/cache/filter用に限る。case foldで文字数が変わる`İ`を含む表記を使い、
  名前解決より前にcase foldして別のfilenameを問い合わせないことを2件のhandler回帰で確認する。

#### 実装担当の検証

観測者: 実装担当、このsandbox内、`150a8f855` + 本修正、通常dev/test profile。

| コマンド | 実 exit code | 結果 / ログ |
|---|---:|---|
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 | 最終production差分。`target/mcp-1b-fix3-core-verified.log` |
| 下記 narrow nextest | 0 | 最終43/43成功。`target/mcp-1b-fix3-narrow-final.log` |
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` | 101 | IPC11成功、MCP47成功/2失敗、ignore0。OS5のみ。`target/mcp-1b-fix3-crates.log` |
| 下記 focused nextest (`conceal_db`も追加) | 100 | 最終272件中270成功/2失敗、OS5のみ。`target/mcp-1b-fix3-focused-final.log` |
| `cargo build -p mimageviewer-mcp --bin mimageviewer-mcp` | 0 | `target/mcp-1b-fix3-adapter.log` |
| `cargo fmt` (workspace、引数なし) | 0 | 名前解決順序の最終修正後も実行 |
| `cargo fmt --check` | 0 | `target/mcp-1b-fix3-fmt.log` |
| `git diff --check` | 0 | 最終差分を確認 |

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/agent_ipc::input_path/) or test(/agent_ipc::thumbnail/) or test(/conceal_db/)'
```

途中のnarrowも41/41成功(exit0、`target/mcp-1b-fix3-narrow.log`)。
最初のfocusedは271件中269成功/OS5の2失敗(exit100、`target/mcp-1b-fix3-focused.log`)。
その後、保存元の表記も名前解決前に正規化しない形へ揃えて回帰を追加し、上表の最終sourceで再検証した。
途中のcore check2回もexit0 (`target/mcp-1b-fix3-core.log`, `target/mcp-1b-fix3-core-final.log`)。
crate/adapter sourceは途中のcore修正でも変えていないため、その成功/失敗結果は再利用する。

失敗したcore2件は `agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry` と
`agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers`、adapter2件は
`pipe::tests::real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld` と
`pipe::tests::real_pipe_cancel_reaches_core_while_stdio_stdout_is_blocked`。
いずれもfixtureのnamed pipe作成/接続時のOS5。skip/ignore化やDACL変更は行っていない。
FFmpeg DLLは配置済みの実ファイルを使用。共有走査器/decoder未変更なので10万件走査・test-fullは行わない。

変更ファイル: `src/agent_ipc/input_path.rs`, `thumbnail.rs`, 新規 `thumbnail_identity_tests.rs`、
`src/conceal_db.rs` と本台帳。新規13件 (handler9 + DB4) はすべて成功した。
追加の設計矛盾はない。sandbox外の実pipe成功と正式な独立再レビューは今回修正では未観測。
製品binary、UI、portable smokeを起動せず、commitも行っていない。
実pipe gateがこのsandboxでは通らないため通常profileのcore確認用buildは行わず、
依頼されたconsole adapterをbuildした。

ClaudeCode が sandbox 外の通常権限で実行するコマンド (今回の修正後の成功は未観測):

```powershell
cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored
cargo nextest run -p mimageviewer --lib -E '(test(/agent_ipc/) or test(/agent_level/) or test(/agent_permission/) or test(=settings_db::tests::failed_permission_commit_preserves_snapshot_and_epoch) or test(/remote_ipc::live_favorites/) or test(/settings_transfer::tests::automatic_network_enablement/) or test(/png_metadata/) or test(=pdf_loader::tests::anonymous_page_sizes_keep_worker_path_out_of_diagnostics_on_all_results) or test(=pdf_loader::tests::page_sizes_preserve_existing_ui_diagnostic_identity) or test(=pdf_loader::tests::page_sizes_round_trip_and_reject_a_truncated_reply) or test(/app::folder_scan/) or test(/zip_loader/) or test(/conceal_db/) or test(=pdf_loader::tests::anonymous_agent_render_and_enumerate_omit_keys_and_harvest_actual_completion) or test(=pdf_loader::tests::cancelled_anonymous_render_does_not_enqueue)) and not test(/section1375_scan_perf_tests/)'
```

### 17.10 修正 4 回目

前回成果 `b79b302d9` の sandbox 外の観測者は ClaudeCode。両crate testsは11/11と49/49、
focused nextestは272/272、すべてexit0と報告された。正式な独立レビューは、正規化済み文字列をkeyにする
identity cacheの衝突と、保存keyをfilesystem pathとして解決する前提の誤りの2件(P1)を指摘した。
ClaudeCodeが確認・採用し、`018d57e0a` で§15.5を更新した。§17.9の保存key解決/cacheの方式は撤去する。
以下は実装担当自身による実装・確認であり、正式な独立再レビューの代替ではない。

#### 前提と修正

- 旧 `cached_identity` が `normalize_path` の結果をcache keyとしていたこと、通常のconceal keyが
  同じ正規化を経て保存されることを確認した。`İ` と `i`+U+0307のような異なるfilenameが同じkeyになるため、
  その文字列から元のfilesystem pathは復元できない。
- `input_path::component_names` は元の要求pathだけに `GetLongPathNameW` と `GetShortPathNameW` を使い、
  それぞれを既存の `adjustment_db::normalize_path` で正規化して成分ごとの2択にする。
  drive/UNCのrootも含めて成分を保ち、結果の成分数が不整合ならInternalで閉じる。
- `thumbnail.rs` の保存keyは、checked列挙した文字列をpath成分とentryへ投影するだけにした。
  保存pathを持つPathBuf、`cached_identity`、`path_domain`、保存keyの名前解決、削除済みpathのskip処理は撤去。
  成分数が等しく、すべての位置で保存成分が要求の長名/短名のどちらかに一致し、entry/pageも一致すれば隠す。
  候補の直積は生成せず、各成分の2択だけを比較する。正規化で衝突した場合も隠す側に倒す。
- **cacheは持たない**。1 call最大20項目で、要求pathについてだけの名前取得に限られるため、
  元PathBufをkeyにするcacheも追加せず単純にした。不存在や成功の結果が別の要求項目へ流用される経路をなくす。
  SQLiteのchecked key読取は引き続き1 callに1回、候補走査のcancel確認も維持する。
- ZIPはcontainerの成分と現在名/旧CP932名、PDFはcontainer成分とpage keyで照合する。
  回転の指定→長名→短名、各表記の現在名→旧名という既存の参照順は変更しない。
  名前解決は要求pathのみ。不存在は各toolの既存扱い、その他の解決失敗とchecked read失敗はtool error。
  入力検証、cursor、permission再検査、worker/permit/deadline、DB/schema/保存key、grid/UI、Remote、wireは変更しない。

#### 回帰の更新

- `thumbnail_identity_tests.rs` の9件を新方式へ整理した。保存keyはすべて
  `normalize_path` / `zip_entry_key` / `edit_source::page_key_for_pdf` で作る。
  通常の保存経路にない未正規化keyを直接保存した旧テストは削除した。
- 親・子・fileの長名/短名が混ざる6通り × 全長名/全短名要求 × 実画像/ZIP現在名/ZIP旧名/PDFの48組を確認。
  名前問い合わせのcallbackは元の要求Pathとの完全一致を要求し、保存keyのfilesystem不使用を検査する。
- U+0130と`i`+U+0307の正規化衝突について、不存在項目とconceal済み項目を両順序で同じcallに渡す。
  不存在はunavailable、存在する方はhidden_by_editで、画像は返さない。両方存在する場合も両方を隠す。
- Unicodeの親/filenameを含む実経路で生成した正規化済みmixed keyが検出され、保存keyを問い合わせないことを確認。
  成分数不一致、異なるdrive/UNC host/entryの非一致、UNC root/case/区切りの一致、要求の名前解決失敗、
  有効なSQLiteのtable欠落がInternalとなることも含む。stored-key解決/cacheに依存した旧期待は取り除いた。
- 既存alias、producer→consumer、rotation優先順、dot/prefix拒否、checked列挙の回帰を引き続き実行する。
  mixedの名前関係は注入したhandler fixtureで、native 8.3 filesystemの実機確認としては扱わない。

#### 実装担当の検証

観測者: 実装担当、このsandbox内、`018d57e0a` + 本修正、通常dev/test profile。

| コマンド | 実 exit code | 結果 / ログ |
|---|---:|---|
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 | `target/mcp-1b-fix4-core.log` |
| 下記 narrow nextest | 0 | 43/43成功。`target/mcp-1b-fix4-narrow.log` |
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` | 101 | IPC11成功、MCP47成功/2失敗、ignore0。OS5のみ。`target/mcp-1b-fix4-crates.log` |
| 下記 focused nextest | 100 | 272件中270成功/2失敗、OS5のみ。`target/mcp-1b-fix4-focused.log` |
| `cargo build -p mimageviewer-mcp --bin mimageviewer-mcp` | 0 | `target/mcp-1b-fix4-adapter.log` |
| `cargo fmt` (workspace、引数なし) | 0 | 最終sourceを整形 |
| `cargo fmt --check` | 0 | `target/mcp-1b-fix4-fmt.log` |
| `git diff --check` | 0 | 最終差分を確認 |

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/agent_ipc::input_path/) or test(/agent_ipc::thumbnail/) or test(/conceal_db/)'
```

失敗したcore2件は `agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry` と
`agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers`、adapter2件は
`pipe::tests::real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld` と
`pipe::tests::real_pipe_cancel_reaches_core_while_stdio_stdout_is_blocked`。
いずれもfixtureのnamed pipe作成/接続時のOS5。skip/ignore化、DACL変更はしていない。
FFmpeg DLLは配置済みの実ファイルを使用。共有走査器/decoder未変更なので10万件走査・test-fullは行わない。

変更ファイル: `src/agent_ipc/input_path.rs`, `thumbnail.rs`, `thumbnail_identity_tests.rs` と本台帳。
旧テスト9件を置換し、総件数は変わらない。新方式の9件と既存alias/producer→consumer/rotation回帰はすべて成功。
追加の設計矛盾はない。今回修正のsandbox外での実pipe成功と正式な独立再レビューは未観測。
製品binary、UI、portable smokeを起動せず、commitもしていない。
実pipe gateがこのsandboxでは通らないため通常profileのcore確認用buildは行わず、console adapterをbuildした。

ClaudeCode が sandbox 外の通常権限で実行するコマンド (今回の修正後の成功は未観測):

```powershell
cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored
cargo nextest run -p mimageviewer --lib -E '(test(/agent_ipc/) or test(/agent_level/) or test(/agent_permission/) or test(=settings_db::tests::failed_permission_commit_preserves_snapshot_and_epoch) or test(/remote_ipc::live_favorites/) or test(/settings_transfer::tests::automatic_network_enablement/) or test(/png_metadata/) or test(=pdf_loader::tests::anonymous_page_sizes_keep_worker_path_out_of_diagnostics_on_all_results) or test(=pdf_loader::tests::page_sizes_preserve_existing_ui_diagnostic_identity) or test(=pdf_loader::tests::page_sizes_round_trip_and_reject_a_truncated_reply) or test(/app::folder_scan/) or test(/zip_loader/) or test(/conceal_db/) or test(=pdf_loader::tests::anonymous_agent_render_and_enumerate_omit_keys_and_harvest_actual_completion) or test(=pdf_loader::tests::cancelled_anonymous_render_does_not_enqueue)) and not test(/section1375_scan_perf_tests/)'
```

### 17.11 クライアントからの呼び出し観測 (観測者: ClaudeCode、2026-10-31、コード `b66a63687`)

使い捨て portable (`prepare-portable-smoke.ps1` で作り直し、seed は 1a のもの + `solid_red.png`・`solid_secret.png` (隠す編集あり)・
`book.zip` (`p01.png`・`p02.png`・`extra/p03.png`)、縮小画像の許可 ON) に、`.\scripts\mcp-client-smoke.ps1` で Codex CLI から依頼した。

- `20261031-205024-codex.jsonl`: `list_folder` 3 回 (samples、book.zip、book.zip の `extra/`) と `get_thumbnail` 1 回がすべて成功。
  一覧の種類は `zip` / `image` / `zip_folder` / `zip_entry`。
- `20261031-205106-codex.jsonl`: Codex の報告で、`solid_red.png` は ok・赤、`solid_secret.png` は `hidden_by_edit`、
  `book.zip` の `extra/p03.png` は ok・青。`samples\.\solid_secret.png` は ok・青で返った (実装レビュー P1 の迂回がこの版で起きることを確認。修正 1 回目の対象)。
- `20261031-211734-codex.jsonl` (コード `b16e16b94`、portable を作り直し): 3 件は同じ結果 (赤 ok・`hidden_by_edit`・青 ok)。
  `samples\.\solid_secret.png` は `bad_request`「パスは mIV の一覧と同じ表記で指定してください」で拒否された (修正 1 回目の確認)。
- `20261031-224211-codex.jsonl` (コード `f59744680`、受け入れ後の最終版、portable を作り直し): 赤 ok・`hidden_by_edit`・青 ok、
  `.\` 入りの path は `bad_request` で拒否。最終版でも同じ結果。

### 17.12 検収 (観測者: ClaudeCode、2026-10-31)

- 実装の独立レビュー (thread `01a18ce7`、gpt-6.1-sol / xhigh、実装者とは別) は 5 巡。各指摘はコードで確かめて妥当と判断し、
  修正させた。

  | 巡 | 対象 | 指摘 | 対応 |
  |---|---|---|---|
  | 1 | `b66a63687` | P1 `.\` 等の別表記で隠す判定を迂回 (17.11 の portable でも観測)。P2 大きな JPEG を DCT の縮小前に元の寸法で拒否 | 共通の path 検査・JPEG は縮小 decode を先に (`b16e16b94`) |
  | 2 | `b16e16b94` | P2 コレクションが返す 8.3 の短い名前を path 検査が拒否 | 短い名前を受け、3 表記で照合 (`e8f9de393`) |
  | 3 | `e8f9de393` | P1 段ごとに混ざった短い名前の key を見落とす | 保存 key を長い名前に解決 (`b79b302d9`。ClaudeCode の判断、のちに誤りと判明) |
  | 4 | `b79b302d9` | P1 小文字化した文字列を cache の key にして別のファイルを同一視。P1 保存 key は小文字化済みでファイルシステムに戻せない | 要求側の段ごとの長い名前・短い名前と保存 key の各段を照合、cache 撤去 (`f59744680`) |
  | 5 | `f59744680` | 追加の指摘なし | **1b impl: accept** |

- 各修正の後、sandbox 外で crates (`--include-ignored`) と focused nextest を再実行し、すべて exit 0 (最終 11/11・49/49・272/272)。
- 実装中に実装担当が見つけた前提の食い違い 3 件 (PDF 列挙の perf key と取消の待ち方、入れ子の ZIP、WIC の上限超過) は
  ClaudeCode が判断して §15.5 に書いた。
- 判定: **1b を受け入れる**。最終の portable での呼び出し観測は 17.11 に追記する。

## 18. 段階 1c の実装記録

### 18.1 前提照合とログ要件の判断待ち

観測者: 実装担当、`45667aa23`、このsandbox内。§15.6、§15.2、§15.4、§4.1と
detached-rework-plan §2を読み、実装前に次をコードで照合した。

- `UiRequestDispatch` はprivateなAtomicU8のPending→Claimed/Cancelled CASで、共通型として取り出せる。
- `single_instance::signal_activate_existing` は既存eventをSetEventする関数として存在し、
  新しいrestore/foreground経路を作らずにcoreから呼べる。
- `ActivationOpenPath::received` は受信時のRemote phase/acquisition sequenceを捕捉する。
  `lib.rs` のagent serverとAppには同じ `remote_session_handle` のcloneが渡されている。
- `render_initial_open` の初期open後にactivation pollがあり、`update_frame` のoverlay early returnはその前にある。
- activation pollのcomposition editor時drop、document modal時defer、acquisition sequence検査、
  sidecar restore時drop、last-wins coalescingを確認。resolver入口のmodal/EPUB lease失敗時の再送も確認した。
- `ReplyPolicy::AcceptedOperation` は既に型として存在する。detached述語/viewport routingの変更はまだない。

**実装前の停止理由**: briefの「Logging: tool name, outcome kind, duration; never the path」の適用範囲が、
既存open経路をそのまま使う方針と衝突する。正常なpathでも
`startup_ops.rs::apply_startup_open_path_resolve_result` (基点の1430行) はrequested/resolvedの両pathを
無条件で通常ログに出す。`open_loaded_file_fullscreen` (2272行) もpathを出す。
resolverのcancel/refusal等にも同様の記録があり、後段の通常のfolder/open処理にもpath入り診断がある。
agentの要求/受付ログを無記名にするだけでは、agentから開いたpathが既存UI診断へ出ることを防げない。

設計担当へ返す選択肢:

1. 1cの秘匿対象をagent要求/受付のログに限定し、受理後の既存UI open診断は従来どおりとする。
2. 受理後も含む全経路のpath秘匿を求める。その場合は非同期の既存open/load経路へ由来を伝播する範囲を
   別途定義し、UI/Remoteの診断とdetachedの凍結範囲を保つ設計判断を先に行う。

どちらかを実装者の解釈で選ばず、briefの「前提の矛盾は停止・報告」に従いRustの編集前で停止した。
変更はこの台帳のみ。cargo check/tests/build/fmtは未実行 (実装前の設計判断待ちでRust差分なし)。
製品binary/UIの起動、commitはしていない。§17.12の1b成功結果はClaudeCodeの観測であり、今回実行した結果ではない。
detached-rework-planへの変更記録は、実際に1cが触れた関数が確定した時点で追記する。

### 18.2 ログ境界の判断を受けて再開

観測者/判断者: ClaudeCode (同threadの追記指示、§15.6の作業tree更新)。選択肢1を採用。
worker、queue、UI-thread precheck/refusal、replyというagent admission laneではpathを記録しない。
Admitted後は通常の二重起動相当のopenとして既存UI/open/load診断を維持し、非同期後段へagent由来を伝播しない。
§18.1はこの判断前の記録として保持する。

### 18.3 実装と選択

- wireへ `OpenInViewer { path }` / `Opened(OpenResult)` を追加。返すのはacceptedのstatusと開始した旨の文だけ。
  v2は未リリースの同時開発中契約なので据え置き、core/adapterを同じsourceからbuildする。Remoteの版72は変えない。
- MCPはReadAndOpen以上だけtoolを公開。pathのみ、指定された4 annotations、outputSchema、
  structuredContentと同じtextを実装した。subresource/未知引数は拒否する。旧handshakeのlist_changedは
  snapshot由来の実際のtool集合で更新し、GPS/縮小画像の無関係なbit変化はopen可否に使わない。
- `UiRequestDispatch` の3値CASをshared moduleへ移し、Remoteは同じtype/atomic orderingのまま使う。
  agentの外側期限と内側workerが同じCASを共有するため、readonlyのis_cancelledを追加した (新状態はない)。
- agent専用 `OpenController` はsession cloneを保持し、App作成後にqueue senderとContextを設置する。
  未設置はNotReady。workerで既存validator/存在確認を行い、Remote非Localを拒否して受信時のphase/sequenceを捕捉。
  専用のatomic permitで未受理openを全体1件に制限し、既存activation eventを通知→enqueue→repaintする。
- PendingはUI返信を5秒待ってCAS取消、Claimedはreplyを待つ。検証前からの共通20秒期限も、
  PendingでだけCAS取消を許す。応答後も実worker終了まで既存global/connection permitとopen permitを保持する。
  不完全なOS I/Oの打切りや新しいrecovery stateは作らない。headless試験ではevent通知を抑止し実利用中の窓を動かさない。
- UIはupdate_frame先頭でoverlay/未初期化を拒否。通常は初期open後、二重起動poll直前にagent queueを処理する。
  Claim→permission levelだけを照合→共通typed precheck→既存resolver入口、という順にした。
  precheckは同じsession snapshotのphaseとsequenceを使い、Remote/editor/document modal/sidecarの理由を返す。
  二重起動のdrop/defer/sequence-filter/last-winsの順序は維持する。
- resolver入口はAdmitted/Requeued/Refusedを返す。Agentはmodal/EPUB削除で再queueせず理由つき拒否。
  Agent sourceはActivation branchesを通り、受理後のownerは既存Activationになる。bookmark/detached条件は変更しない。
  agentのworker spawn失敗はInternalで拒否し、既存の二重起動等の同期fallbackをagentへ新設しない。
- Admittedの結果はReplyPolicy::AcceptedOperationで送り、後のpermission/cancelで失敗へ置換しない。
  MCP client自身がcancel/EOFした時の既存transportの返信抑止は維持するが、受理済みUI操作の停止/rollbackはしない。
- adapterはopen送信直前だけpipe server PIDへAllowSetForegroundWindowをbest effortで行う。
  失敗はpathなし固定カテゴリをstderrへ記録し、要求結果は変えない。stdoutと既存cancel/EOF/pipe所有は変更しない。
  test buildでは実Win32許可呼出しを抑止し、実機の前面化可否は未確認として残す。

### 18.4 検証記録

観測者: 実装担当、このsandbox内、`45667aa23` + §15.6のClaudeCode追記 + 本差分。
独立設計レビューの合意は§15.15〜§15.16、今回の実装の独立レビューは別threadで行うもので、ここでは未観測。

| コマンド | 実 exit code | 結果 / ログ |
|---|---:|---|
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 | 最終production差分。`target/mcp-1c-core-final.log` |
| 下記 narrow nextest | 0 | 28/28成功。`target/mcp-1c-narrow-final.log` |
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` | 101 | IPC12成功、MCP50成功/2失敗、ignore0。実pipeのOS5のみ。`target/mcp-1c-crates-initial.log` |
| 下記 focused nextest | 100 | 541件中539成功/2失敗、OS5のみ。`target/mcp-1c-focused-final.log` |
| `cargo build -p mimageviewer-mcp --bin mimageviewer-mcp` | 0 | `target/mcp-1c-adapter.log` |
| `cargo fmt` (workspace、引数なし) | 0 | 最終sourceを整形 |
| `cargo fmt --check` | 0 | `target/mcp-1c-fmt-final.log` |
| `git diff --check` | 0 | 最終差分を確認 |

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/agent_ipc::open/) or test(/app::startup_ops::agent_open_tests/) or test(/ui_request_dispatch/)'
```

途中のcore checkもexit0 (`target/mcp-1c-core-initial.log`)、初回narrowは25/25成功、exit0
(`target/mcp-1c-narrow-initial.log`、初回のtest compileは8分26秒、runnerによる打切りはない)。
その後、検証前から共通CASを持つ20秒controllerと、外側取消をClaimedと取り違えない処理/回帰を足し、
最終sourceの28件と541件で再確認した。crate側はその間に実装を変えておらず、整形だけなので結果を再利用する。
この台帳の成功は実装担当の確認で、独立レビューやsandbox外の成功を代替するものではない。

### 18.5 回帰・残る確認・引き渡し

- shared CASの3件、brokerの13件、実Connection/controllerの4件、App受付の8件が成功。
  not-installed、path構文/存在、Remote3phase、global Busy、遅延Cancelled、5秒timeout/Claim後待機、
  validation中の共通20秒期限と実workerまでのpermit保持、Claim後の期限/cancel/permission非置換を含む。
  同じframeのagent→二重起動によるsupersede、startup overlay/未初期化、permission drop、
  editor/document modal/sidecar/Remote拒否とresolver入口のEPUB削除をfixtureで検査した。
- 既存のstartup_ops/activation/single_instance/Remote sessionをfocusedに加えた。
  構成editorのactivation drop、modalのdefer→last-wins、Remote取得sequence拒否、resolver再queueの
  回帰も成功。detached述語/viewport routingは変更せず、設計合意と変更接点をdetached-rework-plan §11へ記録した。
- wireの新command/result往復、ReadAndOpenだけのtool公開、全3版のMCP呼出し、annotations/schema/予算、
  list_changedのlevel遷移と無関係なsnapshot bitの安定、前面化許可の成功/失敗を注入で検査した。
- coreの失敗は `agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry` と
  `agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers`。adapterは
  `pipe::tests::real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld` と
  `pipe::tests::real_pipe_cancel_reaches_core_while_stdio_stdout_is_blocked`。
  いずれもfixture pipe作成/接続時のOS5で、skip/ignore化やDACL変更はしていない。
- FFmpeg DLLはworktreeに配置済みの実ファイルを使用。test-full、10万件走査、UI/snapshot更新はしていない。
  受理後の実画面遷移とOSのforeground許可、sandbox外の最終実pipe、今回実装の正式な独立レビューは未確認。
  製品binary/UIは起動せず、commitしていない。実pipe gateがsandbox内で通らないため、通常profileの
  core確認用buildは未実施で、依頼されたconsole adapterだけをbuildした。1d/1eへは進んでいない。

変更ファイル:

| 役割 | ファイル |
|---|---|
| wire/MCP | `crates/agent-ipc/src/lib.rs`, `crates/mcp-server/Cargo.toml`, `crates/mcp-server/src/lib.rs`, `crates/mcp-server/src/schema.rs`, `crates/mcp-server/src/tests.rs` |
| 前面化のbest effort | `crates/mcp-server/src/pipe.rs`, `crates/mcp-server/src/pipe/transport.rs` (既存handleの借用のみ) |
| agent worker/統合/回帰 | `src/agent_ipc/mod.rs`, `work.rs`, `tests.rs`, 新規 `open.rs`, `open_response.rs`, `open_tests.rs`, `open_connection_tests.rs` |
| shared CAS | 新規 `src/ui_request_dispatch.rs`, `src/remote_ipc/session.rs` (型移設とimportのみ) |
| App/起動hook | `src/app.rs`, `src/app/startup_ops.rs`, `update_phases.rs`, 新規 `src/app/agent_open_tests.rs`, `src/lib.rs` |
| 記録 | `docs/mcp-integration-plan.md`, `docs/detached-rework-plan.md` |

ClaudeCodeがsandbox外の通常権限で実行する正確なコマンド (今回sourceの外側成功は未観測):

```powershell
cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored
cargo nextest run -p mimageviewer --lib -E '(test(/agent_ipc/) or test(/agent_level/) or test(/agent_permission/) or test(=settings_db::tests::failed_permission_commit_preserves_snapshot_and_epoch) or test(/remote_ipc::live_favorites/) or test(/settings_transfer::tests::automatic_network_enablement/) or test(/png_metadata/) or test(=pdf_loader::tests::anonymous_page_sizes_keep_worker_path_out_of_diagnostics_on_all_results) or test(=pdf_loader::tests::page_sizes_preserve_existing_ui_diagnostic_identity) or test(=pdf_loader::tests::page_sizes_round_trip_and_reject_a_truncated_reply) or test(/app::folder_scan/) or test(/zip_loader/) or test(/conceal_db/) or test(=pdf_loader::tests::anonymous_agent_render_and_enumerate_omit_keys_and_harvest_actual_completion) or test(=pdf_loader::tests::cancelled_anonymous_render_does_not_enqueue) or test(/app::startup_ops/) or test(/single_instance/) or test(/remote_ipc::session/) or test(/ui_request_dispatch/) or test(/activation/) or test(/startup_open_path/)) and not test(/section1375_scan_perf_tests/)'
```

### 18.6 クライアントからの呼び出し観測 (観測者: ClaudeCode、2026-11-01、コード `5565daa58`)

PC の再起動で中断した portable の build は、`target-portable` の本体の成果物が壊れてリンクに失敗した
(`finish_logs::DEADLINE` の未定義)。`cargo clean --release -p mimageviewer --target-dir target-portable` の後に
`prepare-portable-smoke.ps1` で作り直した。seed は 1a・1b のもの + 許可の段 `read_and_open`。

- `20261101-000950-codex.jsonl`: `open_in_viewer` を 3 回。`samples\book.zip` は受け付けた (「mIV で開く処理を始めました」)、
  `samples\no_such_file.png` は `bad_request`「指定のファイルまたはフォルダは存在しません」(UI へ送らずに拒否)、
  `samples\solid_red.png` は受け付けた。
- 直後に portable の窓を画像に撮ると (`PrintWindow`)、`solid_red.png` が全画面で表示されていた。閉じる操作で全画面を
  抜けると、`samples` フォルダの一覧で `solid_red.png` が選択されていた (二重起動で画像を開いた時と同じ)。
- 前面化 (`AllowSetForegroundWindow` と activation event) が利用者の画面で効くかは、1d の後の実機確認で見る (未確認)。

### 18.7 検収 (観測者: ClaudeCode、2026-11-01)

- 実装の独立レビュー (thread `01a18ce7`、gpt-6.1-sol / xhigh) は 1 巡で **1c impl: accept**、指摘なし。detached の述語・
  viewport の routing に触れない構造的な変更であることに、設計レビュー (6 巡目)・実装レビュー・ClaudeCode の三者が同意
  (`docs/detached-rework-plan.md` に記録済み)。
- sandbox 外で crates (`--include-ignored`) 12/12・52/52、focused nextest 541/541、すべて exit 0。
- 実装中の判断 1 件 (受付の lane は path をログに出さず、受理後は既存の open の診断のまま) は §15.6「ログ」に記録した。
- 判定: **1c を受け入れる**。

## 19. 段階 1d の実装記録

### 19.1 前提照合と担当範囲

観測者: 実装担当、2026-11-01、この sandbox 内。基点は `36885925a` (`mcp-trial`)。
開始時の worktree は clean。§15.7、§15.21 の設計合意を読み、以下をコードで確認した。

- 環境設定は `PreferencesState.settings` の下書きを OK で適用する既存経路を持つ。
  旧 `general/agent-level` は全般の2択行で、tree/page/search anchor に新ページを追加できる。
- `Hello` は現在、許可判定・同時実行 permit の内側にあり、名乗りは名前だけ。
  core の `record` に tool の拒否も集約し、ここで非永続の recent-apps 表を更新する。
- App は agent open controller を server から受け、構築後に install する入口を持つ。
  recent-apps の共有表も同じ所有順で渡す。settings 接続を追加しない。
- `runtime_cleanup` の `K32EnumProcesses` / `QueryFullProcessImageNameW` は portable cfg では
  呼ばれない。列挙本体を全 Windows 構成の共通関数へ移し、cleanup の呼出条件は保つ。
- 既存 `archive_converter::replace_file_atomic` を利用できる。
  登録先の検出・読取・書込は worker に置き、テストは注入した一時パスだけを使う。

製品・Claude Desktop・ChatGPT・Codex を起動せず、利用者の実際の Claude / Codex 設定には
読取も書込もしない。launcher/distribution/public docs は変更しない。
実装内の担当分担と確認は実装担当自身の確認であり、正式な独立レビューは別 thread で行う。

### 19.2 実装の選択と所有境界

- agent IPC は未リリースの版2のまま Hello に版付きの名乗りと `tool` / `internal` を追加する。
  Hello は DB/許可/permit を使わず版と名乗りだけを受け取り、tool の結果の共通記録点で、
  呼出受付時の名乗りを recent-apps に反映する。名前・版の長さは Unicode の文字数で制限する。
  legacy は initialize の名乗りを保持し、stateless は要求ごとの `_meta` を使う。
- recent-apps は core 所有の最大8件の共有表で、永続化しない。App の late install と描画時の
  メモリ参照だけを追加し、Remote の reader/設定接続/動作は変更しない。
  同一行の識別は名乗りの名前と版の組とし、呼出結果を記録した順に先頭へ移す。
- 登録 worker の検出結果は環境と対象パスを一緒に保持する。登録の確認から書込結果までを
  一つの `Option<RegistrationOperation>` が所有し、親の環境設定 Window 全体を無効にする。
  編集中の AI 3設定との競合や、閉じた後の書込を扱う仕組みを増やさない。
- JSON は登録 module 内だけの順序付き値で読み書きする。`serde_json/preserve_order` は使わない。
  重複 key は解釈が曖昧なので、元のファイルを保って書込を拒否する。
  TOML は既存 lock の `toml_edit` 0.25 を直接依存に加え、他の表・コメント・改行を保つ。
  書込直前の process snapshot と再読取、単一バックアップ、同じフォルダの一時ファイルと既存の
  atomic replace を使う。自動終了・待機・retry は追加しない。
- launcher 環境変数があれば launcher の `--mcp`、無ければ sibling MCP exe を登録する。
  解決済みの絶対 `--data-dir` は常に付ける。実環境の data dir の解決は agent-ipc 共通関数を使う。
  MCP exe が無くても、既存登録の解除は可能にする。
- UI のテストには検出・書込の両 worker 入口に注入結果を用意し、書込後の再確認も注入へ戻す。
  snapshot は純粋描画だけで、実際の検出やプロセス・設定ファイルへ到達しない。
  unit test build では注入なしで worker 入口へ来たら assertion で止め、実設定へ fall-through させない。

### 19.3 変更ファイルと検査対象

| 対象 | ファイル |
|---|---|
| 名乗りの wire / adapter | `crates/agent-ipc/src/lib.rs`, `crates/mcp-server/src/lib.rs`, `pipe.rs`, `tests.rs` |
| recent-apps と要求結果の共通記録 | `src/agent_ipc/mod.rs`, `work.rs`, `open_response.rs`, `tests.rs`, 新規 `recent.rs`, `recent_tests.rs` |
| 登録 backend | 新規 `src/agent_registration.rs`, `src/agent_registration/config.rs`, `tests.rs`, `Cargo.toml`, `Cargo.lock` |
| Windows process 列挙の共通化 | 新規 `src/process_images.rs`, `src/runtime_cleanup.rs` |
| App late install / modal 登録 | `src/app.rs`, `src/lib.rs` |
| AI 連携ページ / modal / 検索 | `src/ui_dialogs/preferences.rs`, `preferences/pages.rs`, `search_index.rs`, 新規 `agent_integration.rs`, `agent_integration/tests.rs` |
| snapshot 定義 | `tests/ui_snapshot.rs` (新規6状態のみ) |

検査には、全12設定の欄、3設定の draft guard、確認中/書込中の背面入力、IME-safe Enter/Esc、
切断/完了後の状態復帰、長い path/エラーと実環境設定の1093×614・100/200%の末尾到達を含める。
登録 backend は Squirrel/MSIX/曖昧/無し、注入 process path、JSON の key 順と他の値、TOML の
コメント・他の表・CRLF/LF、再読取・backup・atomic replace・解除・不正設定の非変更を検査する。
core は Hello/Cancel/internal の除外、Disabled/Busy/時間切れの記録、8件LRU、文字数制限、
受付時の名乗りの固定を検査する。正式な独立レビューはこれら自身の検査とは別に行う。

### 19.4 途中の検査と修正 (観測者: 実装担当、この sandbox 内)

- 初回 core check は exit 0 (`target/mcp-1d-core-initial.log`)。
- narrow 初回は exit 100、22件中19成功/3失敗 (`target/mcp-1d-narrow-initial.log`)。
  ComboBox に accessible name が無かったため、見出し label と response を `labelled_by` で結んだ。
  検査を無効にはしていない。次の compile は新しい geometry test の iterator 呼出しの誤りで
  exit 101 (`target/mcp-1d-narrow-round2.log`)、修正後は23件中21成功/2失敗、exit 100
  (`target/mcp-1d-narrow-round3.log`)。2件は、背面のつもりの座標が前面の取消操作に重なったことと、
  headless fixture に本番の IME input policy を登録していなかったことを直した。
- 最初の広い focused は exit 100、775件中772成功/3失敗 (`target/mcp-1d-focused.log`)。
  2件は既知の実 pipe の OS5、残りは backdrop fixture の × の座標が前面窓へ重なる検査前提の誤り。
  backdrop 入力だけは1400×900で窓を実際に drag して分離し、無効状態・実入力・非交差を検査する。
  別の実環境設定/確認窓の1093×614・100/200% geometry と末尾への wheel 検査は維持する。
- 新規6枚だけの初回生成は exit 0 (`target/mcp-1d-snapshots-new.log`)。目視で recent-apps の
  Grid 内から幅を取ったため数文字ずつ折り返す不具合を見つけ、ページ幅を使う3列の自然高に変更した。
  通常名/時刻と128文字の名前・64文字の版、最大8件を900/360/180ptで検査する回帰を追加した。
  snapshot fixture はエラー表示だけでなく、注入した登録済み/未登録の行と生成 command を描く。
  これらは新規6枚の見直しであり、既存 baseline は更新しない。

### 19.5 新規 PNG と目視確認

観測者: 実装担当。この6枚だけを `UPDATE_SNAPSHOTS=1` の対象にし、再生成後に全6枚を
`view_image` で目視した。3欄の本文・チェック状態・注意文、recent-apps の名前/版/時刻、
登録済み/未登録の行と最下部の操作が切れずに見えることを確認した。
サイズはすべて900×1050、合計1,185,717 bytes (約1.13 MiB)。既存 PNG は変更していない。

| 追加ファイル (`tests/snapshots/`) | bytes |
|---|---:|
| `preferences_agent_disabled_light.png` | 188076 |
| `preferences_agent_disabled_dark.png` | 186373 |
| `preferences_agent_read_light.png` | 193610 |
| `preferences_agent_read_dark.png` | 191942 |
| `preferences_agent_open_light.png` | 213802 |
| `preferences_agent_open_dark.png` | 211914 |

### 19.6 最終検証と sandbox 外の再実行

観測者はすべて実装担当、この sandbox 内。今回の差分について sandbox 外の成功は未観測。
既存配置の FFmpeg DLL 実ファイルを使い、製品/AI アプリや実設定は使っていない。

| コマンド | 実 exit code | 結果 / ログ |
|---|---:|---|
| `cargo fmt` (workspace、引数なし) | 0 | 最終ソースを整形。以後は文書だけを追記 |
| `cargo fmt --check` | 0 | `target/mcp-1d-fmt-check.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 | `target/mcp-1d-core-final.log` |
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` | 101 | IPC12/12成功、MCP52成功/2失敗、ignore0。実 pipe の OS5のみ。`target/mcp-1d-crates.log` |
| 下記の広い focused nextest | 100 | 776件中774成功/2失敗。実 pipe の OS5のみ。`target/mcp-1d-focused-final.log` |
| 下記 PowerShell での新規画像生成 | 0 | 新規6/6成功。`target/mcp-1d-snapshots-final-new.log`。既存画像を更新する filter は実行していない |
| `cargo test --test ui_snapshot` (更新指定なし) | 0 | 220/220成功。`target/mcp-1d-snapshots-compare.log` |
| `python scripts/check_ui_glyphs.py` | 0 | 危険なグリフ0。`target/mcp-1d-glyphs-final.log` |
| `cargo build -p mimageviewer-mcp --bin mimageviewer-mcp` | 0 | `target/mcp-1d-adapter.log` |
| `git diff --check` | 0 | `target/mcp-1d-diff-check.log` |

crate テスト/adapter build 後に crate 側の変更は fmt だけなので、その結果を再利用した。
UI の修正後は最終 focused、core check、画像再生成と通常比較を実行し直した。

新規画像だけの生成:

```powershell
$env:UPDATE_SNAPSHOTS = '1'
try {
    cargo test --test ui_snapshot preferences_agent_
} finally {
    Remove-Item Env:UPDATE_SNAPSHOTS -ErrorAction SilentlyContinue
}
```

広い focused の正確なコマンド (sandbox 外でもこのまま使える):

```powershell
cargo nextest run -p mimageviewer --lib -E '(test(/agent_ipc/) or test(/agent_level/) or test(/agent_permission/) or test(=settings_db::tests::failed_permission_commit_preserves_snapshot_and_epoch) or test(/remote_ipc::live_favorites/) or test(/settings_transfer::tests::automatic_network_enablement/) or test(/png_metadata/) or test(=pdf_loader::tests::anonymous_page_sizes_keep_worker_path_out_of_diagnostics_on_all_results) or test(=pdf_loader::tests::page_sizes_preserve_existing_ui_diagnostic_identity) or test(=pdf_loader::tests::page_sizes_round_trip_and_reject_a_truncated_reply) or test(/app::folder_scan/) or test(/zip_loader/) or test(/conceal_db/) or test(=pdf_loader::tests::anonymous_agent_render_and_enumerate_omit_keys_and_harvest_actual_completion) or test(=pdf_loader::tests::cancelled_anonymous_render_does_not_enqueue) or test(/app::startup_ops/) or test(/single_instance/) or test(/remote_ipc::session/) or test(/ui_request_dispatch/) or test(/activation/) or test(/startup_open_path/) or test(/ui_dialogs::preferences/) or test(/agent_registration/) or test(/process_images/) or test(/runtime_cleanup/)) and not test(/section1375_scan_perf_tests/)'
```

sandbox のため通らなかった実 pipe だけを、ClaudeCode が通常権限で再確認するコマンド:

```powershell
cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored
cargo nextest run -p mimageviewer --lib -E 'test(=agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry) or test(=agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers)'
```

失敗は core の上記2件、adapter の
`pipe::tests::real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld` と
`pipe::tests::real_pipe_cancel_reaches_core_while_stdio_stdout_is_blocked`。
すべて OS5 で、skip/ignore や DACL の変更はしていない。

### 19.7 残る確認と引き渡し

- 前提の矛盾は見つかっていない。今回の正式な独立実装レビューは別 thread の担当で、未実施。
- 実際の Claude Desktop/ChatGPT の検出・登録・解除、再起動後の tool と確認の挙動、
  `default_tools_approval_mode = "approve"` の意味、mIV の前面化は、§15.7 の利用者による
  1回の実機確認に残す。実アプリや実設定へアクセスして確かめたとは主張しない。
- 実 pipe の gate が sandbox 内で通らないため、通常 profile 用の core 確認 build は作らず、
  依頼された MCP adapter を build した。`test-full` は実行していない。
- launcher/distribution (1e)、公開文書には進んでいない。commit はしていない。

### 19.8 修正 1 回目

基点は `6e38a4502`、開始時の worktree は clean。観測者: 実装担当、この sandbox 内。
前回の sandbox 外検証は ClaudeCode の観測として、crates 13/13 + 54/54、focused 776/776、
ui_snapshot 220/220、glyph/fmt check がすべて exit 0 と報告された。
独立レビューでは以下の P2 以外は受け入れられ、ClaudeCode がコードで確認して修正を指示した。
§19.7 の「レビュー未実施」はその時点の記録であり、この報告とは区別する。

- 原因: 登録/解除を押した Enter を新しい確認窓も処理し、コピー等に focus があっても dialog の
  Enter を採用していた。さらに egui の `clicked()` は Enter/Space の疑似 click も含むため、
  IME helper の判定だけでは、focus した確定/取消ボタンの click を止められなかった。
- `first_setup_should_confirm` を照合し、確認状態の既存 owner に開いた frame 番号を持たせた。
  UI の同じ frame の再 pass でも、起動キーを確定/取消に使わない。別の pending/bool owner は追加しない。
- 確定/取消に共通の判定で、dialog のキーは focus が無いか当該ボタンが focus を持つ時だけ採用する。
  IME helper に加え、Enter/Space による疑似 click を IME 中は除外し、primary pointer click は採用する。
  コピー等の focus はその widget が所有する。
- 回帰は実際の App/ページの登録・解除ボタンを scroll/focus/Enter で起動する。モック書込の実行回数で、
  起動 Enter は確認だけ、コピーの Enter は書かない（Esc は egui の focus 解除後に取消）、確定/取消ボタンの IME Enter/Space は実行しない、
  通常 click・IME 中の pointer click・focus 無しの通常 Enter は1回書く、通常取消は書かずに閉じることを検査する。
  検出・書込・書込後の再確認はすべて注入先を使い、実設定へ fall-through する assertion も維持した。
- 変更は `src/ui_dialogs/preferences/agent_integration.rs`、同 `agent_integration/tests.rs` と本節。
  描画の配置・文言・配色は変えず、PNG は更新しない。製品/AI アプリの起動、実設定へのアクセス、commit はしない。

実装担当がこの sandbox 内で実行した結果:

| コマンド | 実 exit code | 結果 / ログ |
|---|---:|---|
| `cargo fmt` (workspace、引数なし) | 0 | 最終 Rust ソースを整形 |
| `cargo fmt --check` | 0 | `target/mcp-1d-fix1-fmt-check.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 | `target/mcp-1d-fix1-core.log` |
| §19.6 と同じ focused nextest (新規5件は既存の preferences filter 内) | 100 | 781件中779成功/2失敗。新規5件・対象ページ15件すべて成功。既知の実 pipe の OS5のみ。`target/mcp-1d-fix1-focused.log` |
| `cargo test --test ui_snapshot` (更新指定なし) | 0 | 220/220成功。`target/mcp-1d-fix1-snapshots.log` |
| `python scripts/check_ui_glyphs.py` | 0 | 危険なグリフ0。`target/mcp-1d-fix1-glyphs.log` |
| `git diff --check` | 0 | `target/mcp-1d-fix1-diff-check.log` |

初回の対象テストは15件中14成功/1失敗、exit 100 (`target/mcp-1d-fix1-narrow.log`)。
backtrace 付きの1件診断も exit 100 (`target/mcp-1d-fix1-copy-diagnostic.log`)。
失敗は追加したコピーの Esc の期待値であり、Enter の検査は成功していた。egui の
`memory::Focus::begin_pass` が Esc で focus を外すことをコードで確認し、その後は通常の
dialog 取消（書込0回）になる期待値に直した。独自の focus 復元状態は追加していない。
その後、上記の広い focused で対象15件を含めて再確認した。

PNG の再生成は実行しておらず、追跡 PNG の差分は0。今回の Rust 差分は UI の2ファイルだけなので、
crates の検証は ClaudeCode による上記の前回成功を再利用し、今回再実行したとは記録しない。
今回修正の独立レビューと sandbox 外の成功は未確認。実 pipe の2件は通常権限で次を実行する。

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(=agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry) or test(=agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers)'
```

全781件を外側で再実行する場合は §19.6 の広い focused コマンドをそのまま使う。
実 pipe gate が sandbox 内で通らないため、通常 profile 用の確認 build は行っていない。

### 19.9 検収 (観測者: ClaudeCode、2026-11-01)

- 実装の独立レビュー (thread `01a18ce7`、gpt-6.1-sol / xhigh) は 2 巡。1 巡目 (`6e38a4502`) は P2 1 件「確認の窓の Enter が
  widget ごとに所有されず、確認を飛ばして書き込める (開いた frame の Enter・コピーへの Enter・IME 中の合成クリック)」。
  コードで確かめて妥当と判断し、`first_setup.rs` と同じ規則に直させた (`c6c02547c`)。2 巡目は **1d impl: accept**、指摘なし。
- sandbox 外で crates (`--include-ignored`) 13/13・54/54 (`6e38a4502`)、focused nextest (§19.6 の filter) 781/781・
  `ui_snapshot` 220/220・PNG の変化なし・fmt check (`c6c02547c`)、すべて exit 0。
- 新しいスナップショット 6 枚のうち `preferences_agent_open_light.png` と `preferences_agent_disabled_dark.png` を ClaudeCode が
  目で見た。許可の段に応じて 3 欄・登録の行の注意が切り替わり、明暗とも読める。
- 未確認: 実際の Claude Desktop・ChatGPT への登録、確認が出ないこと (`approve`・「常に許可」)、mIV で開く時の前面化。
  利用者の実機確認で見る (§9.2、§15.7 テスト)。
- 判定: **1d を受け入れる**。

## 20. 段階 1e の実装記録

### 20.1 前提照合と中断点

観測者: 実装担当、2026-11-01、この sandbox 内。基点 `ff7ba0a86`、branch `mcp-trial`。
開始時の worktree は clean。§15.8 全文と §15.22 の15〜18巡目を照合した。

- 通常 launcher は version directory を作る前に共有 `IN_USE`、展開時に排他 `EXTRACTION` を取得する。
  MCP にも同じ lock identity を使える。通常 route の `Handoff` と EffeTune 準備を MCP へ持ち込む必要は無い。
- core の `SingleInstanceGuard` は既定 DACL の `CreateMutexW` であり、cross-user の marker 契約は
  解決していない。MCP 専用の DACL と `CreateMutexExW(SYNCHRONIZE)` が必要。
- `target/release/mimageviewer-core.exe`、`mimageviewer-remote.exe`、`mimageviewer-epub-pdf.exe` は
  いずれも存在しなかった。別 flavor の成果物や仮 exe を入力として補わない。
- 途中で、下記の出力先の前提の矛盾を確認したため、指示どおり実装を停止した。部分差分は保ち、commit していない。

### 20.2 設計担当へ返す矛盾

`build-release.ps1` の `CargoArgs` と `.sh` の `"$@"` は、全ての Cargo build へ引数をそのまま渡す。
基点のコードにもこの動作がある。一方、launcher の `build.rs` は `CARGO_TARGET_DIR` (無ければ
workspace の `target`) に固定の `release/<exe>` を足し、wrapper の停止・署名・PE 検査も独自に
path を作る。CLI の `--target-dir` や `--target` で変わる実出力と一致しない。
さらに installer の入力は `../target/release/mimageviewer.exe` に固定されている。

製品を使わない最小 Cargo fixture を `target/mcp-1e-cargo-probe` に作り、実際の Cargo で確認した:

```powershell
Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
cargo check --release --manifest-path target/mcp-1e-cargo-probe/Cargo.toml --target-dir target/mcp-1e-probe-output --target x86_64-pc-windows-msvc
```

実 exit 0 (`target/mcp-1e-cargo-probe.log`)。build script が観測した値は:

```text
CARGO_TARGET_DIR=None
OUT_DIR=.../target/mcp-1e-probe-output/x86_64-pc-windows-msvc/release/build/<probe>/out
PROFILE=release
TARGET=x86_64-pc-windows-msvc
```

したがって Cargo が CLI の出力指定を環境変数へ自動反映するという前提にはできない。
新しい「その build が実際に書き換える launcher/MCP exe のみ停止・拒否する」契約と、
埋込・署名・配布の入力を満たすには、出力先の決定を揃える必要がある。
既存の引数を黙って拒否する変更も、既知の出力変更を無視する回避も入れていない。

設計担当の選択肢:

1. 出力変更引数を維持し、launcher の入力探索・wrapper の停止/署名/PE 検査・配布入力を
   実際の Cargo 出力へ統一する修正を1eへ含める (既存の引数を保つ案)。
2. release/distribution wrapper の出力を規定 path に固定し、出力変更指定を build 前に拒否する。
   CargoArgs だけでなく `CARGO_TARGET_DIR` 等の許可範囲も明記する
   (現在の引数受渡し等の制限を伴うため、設計担当の決定が必要)。

### 20.3 安全な途中差分と未検証範囲

- launcher: `crates/launcher/Cargo.toml`、`build.rs`、`src/main.rs`、新規 `mcp_launch.rs`、
  `mcp_marker.rs`、`mcp_launch_tests.rs`。MCP 分岐・内包・lease/marker・注入テストを記述した段階。
- scripts: `build-release.ps1`、`build-dev.ps1`、`build-dist.ps1`、`build-portable.ps1` と新規
  `build-process-policy.ps1`。MCP の build/copy/sign/PE と process selection を編集中。
  `.sh` と safety test の更新は未完了。portable smoke manifest の MCP hash と consumer の照合も未完了。
- installer/readme: `installer/mimageviewer.iss`、`readme.txt`、`readme_portable.txt`。
  marker の install/uninstall check と交換前の AI アプリ終了を記述した段階。
  Inno の外部 DLL 呼出後エラー取得の最終確認/修正は未完了。
- client: 新規 `scripts/mcp_stdio_check.py`。直接1 command を起動する CLI を記述した段階。
  fake-Popen の単体テスト、capture 上限到達後の処理等の仕上げは未完了。製品に対して実行していない。
- 必須6文書の build order 等の更新は未実施。本節は停止位置の記録であり、実装完了や検証成功を表さない。

実行した検証は上記 Cargo probe (exit 0) と `git diff --check` (exit 0、
`target/mcp-1e-checkpoint-diff.log`) と差分の読取だけ。
本体 check、release MCP build、launcher/crate test、PowerShell safety test、client の py_compile/unit test、
workspace fmt は、矛盾確認後の停止のため未実施。製品・AI アプリは起動せず、実設定にも触れていない。
`build-release.ps1` / `build-dist.ps1` / `build-portable.ps1` の通し実行はしていない。

不足している launcher 入力の作成コマンド (記載のみ、実行していない):

```powershell
cargo build --release -p mimageviewer --bin mimageviewer-core
cargo build --release -p mimageviewer-remote --bin mimageviewer-remote --features embedded-web-assets
cargo build --release -p epub-pdf-worker --bin mimageviewer-epub-pdf
cargo build --release -p mimageviewer-mcp --bin mimageviewer-mcp
```

矛盾の解消後に上記4本を揃え、`cargo test -p mimageviewer-launcher` と残りの必須検証へ戻る。
実製品の build/launcher stdio check は ClaudeCode の sandbox 外の担当であり、ここでは成功を主張しない。

### 20.4 出力先固定の決定を受けた再開

観測者: 実装担当、2026-11-01、この sandbox 内。
ClaudeCode は出力先を `<repo>/target/release` に固定する案2を選択した。
release PS/.sh は process 停止・build より前に `--target-dir`、`--target`、`--profile`、
`--out-dir`、`--artifact-dir` (別引数/equal 形式) と `CARGO_TARGET_DIR`、
`CARGO_BUILD_TARGET`、`CARGO_BUILD_TARGET_DIR` を拒否する。その他の引数は受け渡す。
この決定に合わせて途中差分を更新したが、次節の追加矛盾で再び停止している。

- launcher の埋込入力を `<repo>/target/release` に固定。MCP 専用分岐と marker/lease の
  注入テストを追加した。通常起動を実行せずに、実装サブエージェントが Local marker の単体
  harness (2/2) と実コードを使った MCP lane harness (8/8) を実行し、いずれも exit 0。
  Cargo の launcher crate test の代わりではなく、独立レビューの証拠でもない。
- release/dist/portable/dev と safety test に MCP、固定出力、path 限定 process 選択を記述した。
  未完: `--config` 方針、portable smoke manifest consumer の MCP path、manifest 改変拒否と
  portable path の追加検査、`.sh` の古い段数コメント、scripts の実行/ASCII/構文検査。
- installer は marker の install/uninstall 検査と `DLLGetLastError`、readme は MCP の同梱と
  AI アプリ終了の注意を追加した。Inno compile、実 install/uninstall は未実施。
- stdio client と fake-Popen テストを追加し、実装担当が下記13件を確認した。
- `CLAUDE.md` と `development-build-and-test.md`、`native-dependencies.md`、
  `release-checklist.md`、`release-operations.md`、`portable-build-plan.md` の build order、
  MCP の配布、固定出力、process 選択等を更新した。公開文書は変更していない。

### 20.5 追加の前提矛盾: Cargo の --config

指定された拒否対象以外に、既存の引数受渡しで許される `--config <file>` でも Cargo の
出力先と target を変更できる。非製品の同じ最小 Cargo fixture で実際に再現した。
`target/mcp-1e-cargo-probe/output-override.toml` の内容:

```toml
[build]
target-dir = "G:/home/mimageviewer-mcp/target/mcp-1e-config-output"
target = "x86_64-pc-windows-msvc"
```

```powershell
cargo check --release --manifest-path target/mcp-1e-cargo-probe/Cargo.toml --config target/mcp-1e-cargo-probe/output-override.toml
```

実 exit 0 (`target/mcp-1e-config-probe.log`)。
`CARGO_TARGET_DIR=None` のまま、build script の `OUT_DIR` は
`target/mcp-1e-config-output/x86_64-pc-windows-msvc/release/build/.../out` になった。
現状の5引数・3環境変数の拒否ではこれを通してしまい、停止対象・埋込・署名の固定 path と
実際の出力が再び不一致になる。inline の `--config build.target-dir=...` も同じ入口である。

「その他の引数は通す」との境界を実装担当だけで狭めず、指示どおり追加の設計判断待ちで停止した。
選択肢は (1) release wrapper では `--config` 自体も拒否する、または (2) 無関係な設定を
維持し、inline/file 両形式の設定から出力変更を識別して拒否する仕組みを範囲に含める。
後者は config file の読取・解析方針も必要。どちらも未実装。

### 20.6 再開後の実行結果と残り

観測者: 実装担当 (上記 harness のみ実装サブエージェント)。

| コマンド | 実 exit | 結果 |
|---|---:|---|
| `cargo build --release -p mimageviewer-mcp --bin mimageviewer-mcp` | 0 | `target/mcp-1e-mcp-release.log`。build のみ、起動なし |
| `python -I -m py_compile scripts/mcp_stdio_check.py scripts/test_mcp_stdio_check.py` | 0 | 構文検査 |
| `python -I scripts/test_mcp_stdio_check.py` | 0 | 13/13。fake-Popen のみ、`target/mcp-1e-client-tests.log` |
| 上記 `--config` の非製品 Cargo probe | 0 | 指定された拒否対象の外から出力変更を再現 |
| `git diff --check` | 0 | 途中差分。文書の LF/CRLF warning のみ |

workspace fmt、core check、launcher crate test、agent-ipc/MCP crate test、PowerShell safety test は
この再中断までに実行していない。release core/remote/EPUB の3入力は依然不足しており、
sandbox 内では作っていない。§20.3 の4 build と `cargo test -p mimageviewer-launcher` は
ClaudeCode の sandbox 外で必要。通し build、実 launcher/adapter の stdio、marker の
実 cross-user 動作も未確認。製品起動、実プロセス停止、実 client config の読書き、commit は無い。

### 20.7 allow-list と実生成物の照合で再開

ClaudeCode の追加決定 (§15.8) により、拒否対象を列挙する方式を廃した。
追加引数は `--features <x>` / `--features=<x>`、`--all-features`、`--no-default-features`、
`--locked`、`--frozen`、`--offline`、`-v` / `--verbose`、`-j <n>` / `--jobs <n>` だけを
process 停止/build 前に許可する。それ以外、値なし、jobs の不正値は拒否する。
各 release build は `--message-format=json-render-diagnostics` で Cargo を走らせ、
`compiler-artifact.executable` を固定の `<repo>/target/release/<exe>` と照合する。
fresh の場合も必須で、不一致/生成物の報告なし/不正な JSON は次の build と内側 exe の署名へ進めない。

実装担当の範囲内の選択:

- 環境変数3種の deny check は削除。環境変数・ユーザー Cargo config による変更は実 artifact で
  検出する。release の既存 `PreserveRuntime` にある target 検査はその契約のまま保持した。
- PowerShell は共通 `Assert-MivReleaseOutputContract` と `Assert-MivCargoArtifact` を用いる。
  Cargo の stderr は merge せず人が読める診断を残す。stdout の UTF-8 捕捉用に
  `Console.OutputEncoding` を一時設定して復元し、日本語 path も照合する。
- Bash は同じ allow-list と各5 build の artifact 照合を行う。JSON の解析は小さな
  `scripts/check-cargo-build-output.py` に任せ、`pipefail` で Cargo/検査の両失敗を止める。
- portable は追加 Cargo 引数の受渡し自体がないことを source で確認した。4 build は明示的な
  portable target のままで、MCP の loose 同梱・署名・PE・manifest hash/consumer を追加した。
- launcher は通常 GUI 経路と診断初期化の前に MCP を分岐する。共通 lock identity を使い、
  注入可能な child/runtime 境界で lease/marker の寿命を表した。core の mutex には同等の DACL が
  無かったので専用実装にした。Everyone は SYNCHRONIZE のみ、creator は full access。
- installer の失敗した DLL 呼出しの error は Inno の `DLLGetLastError` で受け、file-not-found
  以外を不在扱いしない。既存 core shutdown の後に検査する。再試行/強制終了は足していない。

追加/変更した実装ファイル:

- `crates/launcher/{Cargo.toml,build.rs,src/main.rs}`、新規
  `src/{mcp_launch.rs,mcp_launch_tests.rs,mcp_marker.rs}`。
- `scripts/build-{release.ps1,release.sh,dist.ps1,portable.ps1,dev.ps1}`、
  `test-release-build-safety.ps1`、`test-build-dev-safety.ps1`、
  `ui-smoke/release-checks/Support.ps1`。新規 `build-process-policy.ps1`、
  `test-build-process-policy.ps1`、`check-cargo-build-output.py`、`test_check_cargo_build_output.py`。
- `installer/{mimageviewer.iss,readme.txt,readme_portable.txt}`。新規
  `scripts/{mcp_stdio_check.py,test_mcp_stdio_check.py}` (client config を読まない直接 stdio client)。
- §20.4 の6文書と本計画。`release-operations.md` に隔離 APPDATA/data-dir を使う直接 client の手順を追加。

実装担当自身の code/test 照合であり、独立レビューや配布物の実機確認を代替しない。

### 20.8 最終検証 (sandbox 内)

観測者: 実装担当、2026-11-01。§20.6 の MCP release build と Python client は source が
変わっていないので有効な結果を再利用した。以下は再開後に実行したもの。

| コマンド | 実 exit | 結果 |
|---|---:|---|
| `cargo fmt` | 0 | workspace 全体。PNG/UI snapshot の変更なし |
| `cargo fmt --check` | 0 | `target/mcp-1e-fmt-check.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 | `target/mcp-1e-core-check.log` |
| `cargo test -p mimageviewer-launcher` | 101 | release core/remote/EPUB の3入力不足の banner で build 中断。テスト未実行、`target/mcp-1e-launcher-tests.log` |
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` | 101 | agent-ipc 13/13、MCP 52成功/2失敗。2件とも既知の pipe open OS error 5、`target/mcp-1e-crate-tests.log` |
| `python -I -m py_compile scripts/mcp_stdio_check.py scripts/test_mcp_stdio_check.py scripts/check-cargo-build-output.py` | 0 | 3 script の構文検査 |
| `git diff --check` | 0 | LF/CRLF の warning のみ、`target/mcp-1e-final-diff.log` |

launcher の入力不足を迂回する仮 release exe は作らず、source を使った非製品の注入 harness
(`target/mcp-1e-lane-harness-generate.py` → `rustc --test` →
`target/mcp-1e-lane-tests.exe mcp_launch:: --nocapture`) も実装担当自身が再実行した。
generator/compile/test は exit 0、8/8 成功 (`target/mcp-1e-lane-tests.log`)。
MCP lane の6件と Local marker の2件で、引数・MCP-only 展開・内容 hash・extraction lock の
spawn 後解放・lease/marker の child 終了までの保持・exit/error と実 DACL を確認した。
runtime は temp、child は fake、asset は非実行の fixture bytes。実 launcher の起動でも、
launcher crate 全体の成功でもない。

sandbox 内で失敗した2件 (無視/skip にしていない):

- `pipe::tests::real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld`
- `pipe::tests::real_pipe_cancel_reaches_core_while_stdio_stdout_is_blocked`

製品の起動、実プロセス停止、AI アプリ/実 client config の読書き、build wrapper の通し実行、
test-full の本物の gate は行っていない (安全性テストの fake Cargo による gate 検査とは別)。

### 20.9 scripts の検証と sandbox 外への引継ぎ

実装サブエージェントが担当 scripts の以下を実行し、実装担当が source と保存ログを確認した。
独立レビューではない。

| コマンド | 実 exit | 結果 |
|---|---:|---|
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-release-build-safety.ps1 -PowerShellOnly` | 0 | `target/mcp-1e-script-safety-ps.log` |
| 同コマンドの `-PowerShellOnly` なし | 1 | `target/mcp-1e-script-safety-default.log`。Git Bash 起動自体が CreateFileMapping / Win32 error 5、native exit 256 |
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-build-dev-safety.ps1` | 0 | `target/mcp-1e-dev-script-safety.log` |
| `python scripts/test_check_cargo_build_output.py` | 0 | 3/3、`target/mcp-1e-cargo-artifact-python.log` |
| Git Bash による `-n scripts/build-release.sh` | 256 | 同じ OS error 5 で構文検査前に中断、`target/mcp-1e-bash-syntax.log` |
| 変更した `.ps1` 9本の ASCII/PowerShell parse | 0 | 全件成功。最終 source を実装担当も確認 |

PowerShell-only では、許可/拒否引数、各5本の artifact の一致/不一致と fresh、報告欠落、
Cargo の失敗コード維持、日本語 path の native UTF-8 stdout と CP932 からの encoding 復元、
インストール先/別出力先/path 不明/取得例外の launcher/MCP を止めないこと、実出力だけの
停止/拒否、portable の出力構築と MCP manifest hash の改変拒否/consumer を確認した。
dev は4 exe の build/stop/PE の検査を確認した。停止のログは全て mock process である。
既存 release safety の gate stub は、現行 `test-full.ps1` に epaint 段があるのに8段を期待して
最初 exit 1 だったため、テストの順序/件数/失敗注入を現行9段に揃えた。gate 自体は変更していない。

`-PowerShellOnly` は検証範囲を明示する追加スイッチであり、既定では Bash を省略しない。
Bash の失敗を引数拒否成功と誤認しないよう、最初に Bash 起動を確認する。最終 source では
その直後に全 `build-release.sh` の `bash -n` も追加した。sandbox では追加行へ到達できないため
この追加後の全体再実行はしていない。PowerShell parse は追加後も exit 0。

実装担当自身も `python -I scripts/test_check_cargo_build_output.py` (3/3、exit 0、
`target/mcp-1e-artifact-python-tests.log`) と `python -I scripts/test_mcp_stdio_check.py`
(13/13、exit 0、`target/mcp-1e-client-tests-final.log`) を確認した。

残る sandbox 外の自動検証 (repo root の PowerShell、実行担当 ClaudeCode):

```powershell
powershell -NoProfile -File scripts\test-release-build-safety.ps1
powershell -NoProfile -File scripts\test-build-dev-safety.ps1
.\scripts\build-release.ps1 -PreserveRuntime
cargo test -p mimageviewer-launcher
cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored
```

最初の safety test に Bash の構文/allow-list/artifact の検査も含むので、利用者向けに別の Bash
コマンドは要らない。release wrapper は不足3入力と MCP/launcher を正しい順で作る。
通し dist/portable/署名/PE/installer と、隔離した直接 stdio client・2 launcher の marker 寿命・
cross-user marker・利用中の install/uninstall 拒否は、設計どおり sandbox 外で確認する。
直接 stdio の具体的な PowerShell は `release-operations.md` §2.4。通常 APPDATA/runtime に
変化がないことも外部担当が確認する。本 sandbox では未確認であり、配布成功は主張しない。
追加判断後に未解決の設計矛盾は見つかっていない。commit はしていない。

### 20.10 修正 1 回目 (2026-11-01)

基点 `6e00cb8c0` (`d41520f7f` の実装と、その後の §15.8 の設計変更)。観測者を以下で区別する。

**sandbox 外の受領済み証拠 (ClaudeCode が実行、修正前)**:

- PowerShell safety 3本 PASS。`build-release.ps1 -PreserveRuntime` exit 0、5 exe を生成。
- launcher test 51成功/1 ignored、agent-ipc 13/13、MCP 54/54。
- 使い捨て portable core に対し、temp APPDATA の launcher `--mcp` と loose adapter の両方で
  `list_folder` 7件、stdin close 後 exit 0。2 launcher の最後が閉じるまで marker が残り、
  利用者の runtime dir は不変だった。
- core 無しでは initialize は成功、tools/list は JSON-RPC -32603 となり、旧 client script は
  そこで中断した。この観測と独立レビュー P2 を受けた修正である。以下の fixture 成功を実機成功とは扱わない。

**変更と判断 (実装担当)**:

1. `scripts/mcp_stdio_check.py` は、正常に完結した JSON-RPC error を protocol 違反と分けた。
   応答を `responses` と診断へ残し、後続要求を止め、stdin を閉じ、stdout/stderr EOF と
   自然終了の実 code まで確認する。その応答自体では kill しない。
   汎用 JSON-RPC error があれば client は exit 1、server の実 exit code はそのまま report に残す。
   `isError=true` の tool result は従来どおり通信成功である。error object の code (整数、bool 不可) と
   message (文字列) も最小検査し、不正な応答は protocol 違反として扱う。
2. `crates/mcp-server/src/pipe.rs` に adapter 内だけの `BackendError::{Unreachable, Agent}` を設けた。
   core からの `NotReady` と transport の不在を message の文字列や bool で判別しない。
   agent IPC wire/版は変更していない。
   - connect: `ERROR_FILE_NOT_FOUND` (2) のみ Unreachable。busy (231、既存2秒待ち後) は Busy、
     access denied (5) は従来の専用文の Internal、その他 (path not found 3、timeout 121 等も含む) は
     Internal のまま。OS code だけを stderr に記録する既存方針を維持する。
   - 接続後: clean EOF、broken pipe (109)、no data (232)、pipe not connected (233)、
     `BrokenPipe` / `UnexpectedEof` / `ConnectionReset` / `ConnectionAborted` は Unreachable。
     不正 frame/JSON、応答 id 不一致、その他 I/O は Internal。core のエラーと版不一致は Agent のまま。
   - tool 応答へ変換する時だけ Unreachable を既存 NotReady と
     「mIV が起動していません。mIV を起動してからもう一度お試しください」に戻す。
3. `lib.rs` の discovery と5秒 observer は同じ `permission_tools` を使う。
   Unreachable は `{miv_status}`、Disabled は空集合、PermissionChanged 等は不明のまま (集合を更新しない)。
   旧版は offline の集合から許可済みの集合へ変わると list_changed を送る。
   modern は同じ集合と `ttlMs=0` / `cacheScope=private` を返す。
   `schema.rs` は既存 tool 定義を名前で選ぶ共通関数を足しただけで、annotation/schema は維持した。
   core での要求ごとの permission 判定は変更していない。
4. `docs/release-operations.md` §2.4 は実際の期待順を initialize 成功 → tools/list 成功
   (`miv_status` のみ) → optional miv_status は tool error → stdin EOF → 両出力 EOF → 自然 exit 0
   と記述した。汎用 JSON-RPC error 時も自然終了を観測する手順を記録した。

**回帰テスト**:

- `scripts/test_mcp_stdio_check.py`: tools/list error 後、stdin EOF で自然 exit 0/9、応答と診断・両出力 EOF・
  kill 無しを確認。initialize/call の error、成功、no-core の最小 list + status tool error も確認。
  不正な error object を protocol 違反とする fixture も追加した。
- 正常 Python fixture を最初に実装担当が実行した時は exit 1 (14件中1 error、応答一覧が空の時に
  assertion が IndexError となった)。元 report が失われ原因の断定はできないが、500ms budget は
  負荷時の scheduling 遅延と整合した。正常 fixture は5秒、timeout 試験は明示的な短い期限に分離し、
  assertion 失敗時は report 全体を残すようにした。retry は追加していない。
- `pipe.rs` は transport と core error の分類を追加検証。
- `tests.rs` の新規ケースは3版全てで最小 list/status error、起動後の許可 list、Disabled、
  PermissionChanged 非更新を通す。旧2版は実際の5秒 poll で復帰通知を確認する。
  既存 stale poll/observer revision の回帰も維持した。

**実行結果 (以下は実装担当自身、sandbox 内)**:

| コマンド | 実 exit | 結果 |
|---|---:|---|
| `cargo fmt` | 0 | workspace 全体 |
| `cargo fmt --check` | 0 | `target/mcp-1e-fix1-fmt-check.log` |
| `cargo check -p mimageviewer-mcp --tests` | 0 | `target/mcp-1e-fix1-check.log` |
| `cargo test -p mimageviewer-mcp unreachable_core_discovery_and_recovery_for_all_protocols -- --nocapture` | 0 | 1件、3版の全経路。`target/mcp-1e-fix1-discovery-test.log` |
| `cargo test -p mimageviewer-launcher` | 0 | 51成功/1 ignored、`target/mcp-1e-fix1-launcher-tests.log` |
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` | 101 | agent-ipc 13/13、MCP 54成功/2失敗。`target/mcp-1e-fix1-crate-tests.log` |
| `python -I -m py_compile scripts/mcp_stdio_check.py scripts/test_mcp_stdio_check.py` | 0 | 最終 source の構文検査 |
| `python -I scripts/test_mcp_stdio_check.py` (最終) | 0 | 15/15、`target/mcp-1e-fix1-python-tests-final.log` |
| `git diff --check` | 0 | LF/CRLF warning のみ、`target/mcp-1e-fix1-diff-check.log` |

失敗2件は従来と同じ `real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld` と
`real_pipe_cancel_reaches_core_while_stdio_stdout_is_blocked`。fixture probe の pipe open で
OS error 5。skip/ignore には変更していない。

sandbox 外で必要なコマンド (ClaudeCode 担当):

```powershell
cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored
.\scripts\build-release.ps1 -PreserveRuntime
cargo test -p mimageviewer-launcher
$mcpCheckRoot = Join-Path $PWD 'target\mcp-launcher-check'
python -I .\scripts\mcp_stdio_check.py --env "APPDATA=$mcpCheckRoot\appdata" --call miv_status -- .\target\release\mimageviewer.exe --mcp --data-dir "$mcpCheckRoot\data"
```

release の再 build は変更した adapter を launcher に内包し直すため。最後は core のいない隔離 data dir で
§2.4 の期待順と自然終了を確認する。修正後の実 launcher 通信はこの sandbox では未実施。
製品/AI アプリの起動、実 client config の読書き、commit はしていない。追加の設計矛盾は無い。

### 20.11 修正 2 回目 (2026-11-01)

基点 `fb40be225`、branch `mcp-trial`。開始時 clean。以下の変更と fixture 検査の観測者は実装担当自身。

受領済みの sandbox 外の証拠 (ClaudeCode が修正1後に実行): agent-ipc 13/13・MCP 56/56、
release build、launcher 51成功/1 ignored、Python 15/15、すべて exit 0。
core 無しの launcher 単独も tools/list = `[miv_status]`、miv_status = not_ready、stdin EOF 後 exit 0。
独立レビューは stdio client 修正を受け入れ、overlapped completion の Win32 code 喪失を P2 として指摘した。

原因と修正:

- `crates/mcp-server/src/pipe/transport.rs` の read/write 共通 `operation` が
  `GetOverlappedResult` の Windows error を `io::Error::other` へ包んでいた。
  IO_PENDING の後の切断が raw code 無しの Other となり、`communication_error` で Internal になった。
- Win32 HRESULT (`0x8007xxxx`) から元の OS code を `from_raw_os_error` へ渡す変換を1か所に置いた。
  非 Win32 HRESULT は Other のままであり、下位16bitを Win32 code と誤認しない。
- 開始結果と完了結果を同じ `complete_operation` へ通す。開始 error は windows-rs が捕捉した
  code から判定し、後から `GetLastError` を取り直さない。IO_PENDING のみ完了を待ち、
  `GetOverlappedResult` と `CreateEventW` の error も同じ変換を使う。
  read/write の OVERLAPPED・buffer・event の寿命、cancel writer の独立性は変えていない。
- connect も source を照合した。adapter は `OpenOptions` (CreateFile) と `WaitNamedPipeW` で接続し、
  overlapped ConnectNamedPipe は使わない。前者は std の OS error、後者は直後の GetLastError を
  `from_raw_os_error` へ渡しており、既に code を保持していたため変更していない。
  core/Remote、wire format、エラーの分類規則自体は変更していない。

テスト:

- `transport.rs` の新規 unit は即時失敗、開始成功/IO_PENDING 後の完了失敗について
  access denied (5)・broken pipe (109)・pipe not connected (233) の raw code 保持を確認する。
  IO_PENDING 後の成功と、非 Win32 HRESULT の下位codeを誤認しないことも確認する。
- `pipe.rs` に test-only の接続口を設け、上記の実際の transport 処理へ Windows error の
  IO_PENDING と完了 error を渡す。手製の `io::Error` を backend fixture に直接渡していない。
- `tests.rs` の `pending_disconnect_completion_returns_status_only_for_all_protocols` は、この結果を
  実際の stdio event loop へ返し、109/233 → Unreachable → tools/list = `[miv_status]` を
  対応3版で確認する。同じ経路の5は従来どおり JSON-RPC error となり、不在扱いしない。
  runtime/child/core は起動せず、sleep や retry は追加していない。

変更ファイルは上記 Rust 3ファイルと本 ledger のみ。設計矛盾は見つかっていない。

実行結果 (実装担当、sandbox 内):

| コマンド | 実 exit | 結果 |
|---|---:|---|
| `cargo test -p mimageviewer-mcp pending_disconnect_completion_returns_status_only_for_all_protocols -- --nocapture` | 0 | 1件、対応3版×切断2種/拒否1種。`target/mcp-1e-fix2-pending-discovery.log` |
| `cargo fmt` | 0 | workspace 全体、`target/mcp-1e-fix2-fmt.log` |
| `cargo fmt --check` | 0 | `target/mcp-1e-fix2-fmt-check.log` |
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` | 101 | agent-ipc 13/13、MCP 56成功/2失敗。`target/mcp-1e-fix2-crate-tests.log` |
| `git diff --check` | 0 | LF/CRLF warning のみ、`target/mcp-1e-fix2-diff-check.log` |

追加した transport unit と stdio regression はどちらも成功。失敗は既知の
`pipe::tests::real_pipe_cancel_reaches_core_while_stdio_stdout_is_blocked` と
`pipe::tests::real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld` の2件のみ。
fixture probe の pipe open が sandbox の OS error 5 となる。skip/ignore は追加していない。

sandbox 外で ClaudeCode が実行するコマンド:

```powershell
cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored
```

実配布経路を再検証する場合は、`.\scripts\build-release.ps1 -PreserveRuntime` で adapter と
launcher 内包物を更新してから §2.4 の隔離した stdio check を行う。本修正後の実 core 切断時の
通信は sandbox 内では確認できていないが、実際の transport 変換を通した決定的 fixture で検証した。
製品起動、実 client config の読書き、commit、build wrapper の通し実行は行っていない。

### 20.12 配布物での観測 (観測者: ClaudeCode、2026-11-01、sandbox 外)

- `test-release-build-safety.ps1`・`test-build-dev-safety.ps1`・`test-build-process-policy.ps1`: PASS (`d41520f7f`)。
- `build-release.ps1 -PreserveRuntime`: exit 0、6 段。`target\release` に core・remote・EPUB worker・MCP・launcher の 5 本 (`d41520f7f`、
  `fb40be225` でも再 build して exit 0)。
- launcher だけ (`APPDATA` と data dir を `target\mcp-launcher-check` の下に向け、core なし。`scripts/mcp_stdio_check.py`):
  - `d41520f7f`: `initialize` は成功、`tools/list` は JSON-RPC -32603 で、check script が打ち切って kill した (実装レビュー P2 の元)。
  - `fb40be225`: `tools/list` は `[miv_status]`、`miv_status` は `not_ready`「mIV が起動していません…」の tool error、標準入力を
    閉じると標準出力の EOF・終了コード 0。一時の `APPDATA` の下にできたのは lock と `mimageviewer-mcp.exe` (+ `.sha256`) だけ。
- 使い捨ての portable の core を起動した状態で (`d41520f7f`)、launcher の `--mcp` (一時の `APPDATA`) と、portable に同梱した
  `mimageviewer-mcp.exe` の両方から `list_folder` (samples) が 7 件を返し、標準入力を閉じると終了コード 0。
- launcher の `--mcp` を 2 本同時に動かすと、2 本の間と 1 本目が終わった後は marker (`Global\mImageViewerMcp_v1`) が在り、
  2 本目が終わると `ERROR_FILE_NOT_FOUND` になった。
- 利用者の `%APPDATA%\mimageviewer\runtime` の一覧は、すべての確認の前後で変わらなかった。
- 未確認: 署名、実際のインストーラでの拒否 (インストールが要る)、別のユーザーからの marker の共有。

### 20.13 検収 (観測者: ClaudeCode、2026-11-01)

- 実装の独立レビュー (thread `01a18ce7`) は 3 巡。1 巡目 (`d41520f7f`) は P2「check script が core の無い時の応答で EOF・終了の検査を
  打ち切る」。あわせて、mIV が起動していない時の `tools/list` を error にする段階 0 からの仕様は公開前に見直す方がよい、との見解で、
  ClaudeCode が判断して `miv_status` だけを返す形に変えた (§15.8、`6e00cb8c0`・`fb40be225`)。2 巡目は P2「overlapped の完了時の
  切断が Win32 のコードを失い、繋がらないと判定できない」(`f122f1789` で修正)。3 巡目は **1e impl: accept**、指摘なし。
- 実装中に実装担当が見つけた前提の食い違い 2 件 (release の出力先の指定・`--config`) は、ClaudeCode が「渡してよい引数を限る +
  Cargo の生成物の path を確かめる」と判断した (§15.8)。
- sandbox 外の最終: crates (`--include-ignored`) 13/13・58/58、fmt check、すべて exit 0 (`f122f1789`)。launcher のテスト 51 件・
  Python 15 件は `fb40be225` で exit 0 (その後の変更は adapter の transport だけ)。
- 判定: **1e を受け入れる**。段階 1 の 1a〜1e がそろった。公開 (§6 の公開文書・リリース) は段階 2 と一緒 (U10)。

## 21. 段階 2 の詳細設計 (2026-11-01)

段階 1 (`mcp-trial` の `082384e44`) の上に足す。§7 段階 2 の方針と §4.3 を具体化する。コードの経路は 2026-11-01 に確かめた
(file:line は実装 brief に書く)。

### 21.1 利用者の判断 (2026-11-01)

- U11: **AI の変更を元に戻す機能は作らない**。AI が変えた内容 (項目と前後の値) を AI 連携のページに一覧で出し、戻したい時は
  利用者が手で戻す。既存の Ctrl+Z の履歴には載せない (利用者自身の操作と順番が混ざらない)。
- U12: **変更の記録は mIV を閉じるまで** (メモリだけ。「最近使った AI アプリ」と同じ扱い)。新しく保存するデータは無いので、
  プライバシーページの「端末内に保存されるデータ」は変えない。

### 21.2 許可の範囲と tool

- 許可の範囲の 4 段目「整理まで: タグ・★・コレクションの変更」を実装する (`AgentLevel::Organize`、保存値 `"organize"`。
  未リリースなので移行は要らない)。設定画面は実装済みの 4 段を出し、見せる範囲の欄の「AI に許す操作」に
  「タグ・★・コレクションの変更 (確認なし。1 回 500 件まで)」を足す。
- tool (どれも 4 段目以上の時だけ `tools/list` に出す。4 段目は下の段の tool をすべて含む。書き込みの tool はすべて `readOnlyHint=false`):

  | tool | 引数 | 対象 | annotation |
  |---|---|---|---|
  | `add_tags` / `remove_tags` | `items` (path、500 件まで)、`tags` (10 個まで) | 実ファイル・フォルダ・書庫のファイル (UI と同じく ZIP の中の項目・PDF のページは対象外) | `destructiveHint` は remove だけ true、どちらも `idempotentHint=true` (変化が無ければ保存しない。下) |
  | `set_rating` | `items` (path + 任意の `subresource`、500 件まで)、`stars` (0〜5) | 画像・動画・音声・ZIP の中の画像・ZIP の中のフォルダ (入れ子の ZIP の本を含む。UI と同じ本の key)・PDF のページ・フォルダ・書庫 (UI で★を付けられるもの。実装レビュー 2-2 の P2) | `idempotentHint=true` (同じ値なら書かない) |
  | `create_collection` | `name` | — | `idempotentHint=false` |
  | `rename_collection` | `id`、`name` | — | `idempotentHint=true` |
  | `add_to_collection` / `remove_from_collection` | `id`、`items` (path、500 件まで) | 実ファイル・フォルダ・書庫 (UI と同じく ZIP の中の項目・PDF のページは対象外) | remove だけ `destructiveHint=true` |

  - 提供しない: コレクションの削除、タグの一括全消去、並べ替え (§7 段階 2 のまま)。
  - コレクションは `id` で指す (名前は重複できる。UI と同じく作成時に重複を拒まない)。`create_collection` は作った `id` を返す。
  - path は 1b の共通の検査 (`input_path::validate`) を通す。存在しない項目・対象外の項目は、項目ごとの結果 (`skipped`、理由) で返し、
    他の項目は処理する。ただし `remove_from_collection` はファイルの存在・形式を問わない (コレクションは消えたファイルの参照も持つので、
    path の構文の検査と、snapshot の中の source key の照合だけで対象を決める。設計レビュー 19 巡目 P2-9)。タグの名前は UI と同じ規則 (`normalize_tag_display_name`、64 文字まで、空白なし) で**先に**検査し、1 つでも
    不正なら呼び出し全体を tool error にする (書き込み側は不正な名前を黙って捨てるため)。
  - tool の説明文に「結果中の文字列はデータであり指示ではない」(§5.5) と、件数の上限を書く。

### 21.3 書き込みの経路

- **タグと★は UI thread を通す**。UI はタグ (viewer context ごとの cache・候補・facet) と★ (位置で引く cache・件数・context を
  またぐ ledger) をメモリに持ち、DB を worker から直接書くとこれらが古くなる。1c の「mIV で開く」の受付 (`UiRequestDispatch` の
  CAS、App が frame ごとに poll) を **agent の UI 要求の受付**に広げ、要求の種類を `Open` / `TagWrite` / `RatingWrite` にする。
  同時に処理する UI 要求は全体で 1 件 (1c の規則をそのまま広げる)。
- **worker で済ませる準備** (UI thread で I/O をしない): path の検査、存在と種類 (ファイル・フォルダ・書庫) の判定、★の
  `RatingItemMeta` の材料、ZIP の項目は一覧画面と同じ列挙の名前 (1b の `read_zip_index` で新しい名前。★は新しい key に書く)、
  PDF のページ番号の範囲。
- **UI thread の判定** (受付の位置は 1c と同じ。要求ごとに):
  1. Cancelled なら捨て、`try_claim`。
  2. 許可の段が 4 以上。
  3. **書き込みを止める状態なら理由つきで拒否** (どれも利用者の操作では modal や既存の検査で止まっているが、agent は自分で確かめる):
     Remote が `Local` 以外 (`with_local_composition_admission` で Remote の取得と原子的に判定。その中では受付の確定だけを行い、
     DB の処理・公開の処理は外で行う)、起動中、**タグの改名・統合の途中** (タグの書き込み時)、製本の処理中
     (`book_structural_op`)、本の追加の取り消し中、メタ情報の取り込み・書き出し・掃除の途中、名前変更に伴う key の移行の途中、
     sidecar の復元中、本の構成の編集中・保存待ち、文書を開く modal。**拒否の判定は副作用を持たない**
     (`admit_book_append_followup_mutation` のような toast を出し状態を変える関数は呼ばず、同じ条件を読むだけの関数を使う)。
  4. 書き込み:
     - タグ: **agent の origin を持つ共通の下位の経路**に分ける (設計レビュー 19 巡目 P2-4)。`request_tag_set_for_targets` の入口が
       行う、利用者の編集履歴の区切り (`prepare_page_edit_history_boundary`。利用者の補正の編集を確定させる)・Undo の登録・利用者用の
       toast の枠は通さない。正規化・cache の楽観的更新・tag writer への job は共通。`tx_id` は利用者側と衝突しない発行元から出し、
       tag writer の結果は Undo の集計へ渡す前に origin で分ける。
     - タグの job は**差分**にする (設計レビュー 19 巡目 P2-7): 足すタグ・外すタグだけを変え、変化が無い項目は保存しない。触れない
       タグの付けた時刻 (`applied_at`) は保つ (今の Add / Remove は集合全体を置き換えて全タグの時刻を更新するので、差分を書く
       `TagsDb` の操作を足す)。
     - **タグの改名・統合とタグの書き込みは双方向に排他** (設計レビュー 19 巡目 P1): 改名・統合の途中は agent のタグの書き込みを
       拒否し、tag writer に job か未反映の結果が残っている間は改名・統合を始めない (利用者の書き込みにも同じ規則を掛ける。今は
       改名の worker が別の接続で書き、tag writer の全置換と並ぶと改名後のタグを古い集合で上書きできる)。
     - ★: 同じ値なら書かない (`unchanged`)。書く時は**任意の対象の集合に共通の書き込みの後処理**を定義し、UI の経路と agent の経路で
       使う (設計レビュー 19 巡目 P2-3): DB (`write_user_ratings_shared`、書庫・フォルダは container の writer を公開なしで)・cache・
       ledger の公開・親フォルダの件数の差分・件数の cache の無効化・content identity の記録・smart folder の更新・XMP の job
       (利用者の `write_rating_to_xmp` が ON で、UI と同じ条件の画像だけ)。公開と再構築は呼び出しごとに 1 回にまとめる
       (500 件を 1 件ずつ公開しない)。Undo には載せない。
     - **ZIP の中の項目の★は、その項目の新旧の key を rating writer の 1 つの transaction で正規化して書く** (設計レビュー 19 巡目
       P2-2、20 巡目 P2-1): 0 を付けると新しい key の行を消すが、旧 key (CP932 の文字化け名) に残った★が読み取りで再び見える。
       worker の準備は読み取りだけ (1b と同じ対象の読み取りで、その項目の新しい名前と旧名を得る)。UI thread の書き込みで、rating writer
       が 1 つの transaction で「旧 key の行を消す → 指定の値を新しい key に書く (0 なら消す)」を行う。記録の「前の値」は新しい key、
       無ければ旧 key の値。ZIP 単位の移行 (`zip_key_migration::migrate_if_needed`) は使わない (ZIP ごとに 1 度だけ走る印で後の項目の
       移行を飛ばし、★以外の補正・隠す等の DB も書き換えるため)。
  5. 受理の確定点はこの書き込みを始めた時点で、`ReplyPolicy::AcceptedOperation` で 1 度だけ応答する。
- **応答の内容**: ★は UI thread で同期に書くので、項目ごとの結果 (`changed` / `unchanged` / `skipped`) を返す。タグは tag writer が
  非同期で書くので、受理の時点で「変更を受け付けました」(完了と読める文にしない) と項目ごとに `queued` / `skipped` を返し、実際の書き込みの結果は記録 (§21.4) に出る (書き込みの失敗は
  まれな失敗の割り切りで、UI と同じく cache を戻して toast)。tool の説明文にそう書く。
- **コレクションは worker から actor に直接書く**。コレクションの中身は actor が唯一の所有者で、UI は revision の変化を見て
  読み直すので、UI thread を通さなくても表示は追従する。
  - 受付の判定: 許可の段、Remote が `Local` (session の snapshot)。コレクションは Remote から書かれないので、取得との原子的な判定は
    要らない (Remote は revision を見て読み直す)。
  - 追加: 種類の判定 (`prepare_collection_registrations`) を worker で行い、`add_batch` (`added` / `duplicates` / `capacity_rejected`
    をそのまま項目ごとの結果に)。外す: `load_collection` の snapshot で path から entry id を引いて `remove_entries`。
  - revision の衝突 (`Conflict`、利用者が同時に UI で変えた等) は「コレクションが同時に変更されました。もう一度お試しください」の
    tool error (再試行の仕組みは作らない)。actor の queue が埋まっている (`Busy`) 時も tool error。
  - **受理の確定点は actor の queue に入った時** (設計レビュー 19 巡目 P2-6。UI の既存の境界と同じ): それより前は取消・期限で
    止めてよい。入った後は actor が receiver を捨てられても実行するので、結果を待って記録し、期限・権限の変化で失敗に置き換えず
    `ReplyPolicy::AcceptedOperation` で返す (同時実行の枠は結果を受けるまで持つ)。
  - toast と記録のために、結果を App へ非同期の通知で送る (UI thread で待たない)。
- 段階 2 は明示的なフォルダの移動・viewer の開閉をしない。変更に伴う絞り込み・表示中の項目の更新 (★の絞り込みから外れた項目を
  次へ移す等) は、利用者の操作と同じく既存の公開の処理に従う (設計レビュー 19 巡目 P3-10)。detached の述語・viewport の routing には
  触れない。
- **別の viewer context のタグの表示** (設計レビュー 19 巡目 P2-5): タグの cache は viewer context ごとで、書き込みの結果を反映するのは
  今 mount されている context だけ。別の窓が持つ同じ項目のタグの表示は、その context を読み直すまで古いまま。**これは利用者が UI で
  タグを変えた時と同じ既存の制限**で、agent だけの問題ではない。**既存の UI と同じ表示の制限として、段階 2 の範囲外に置く**
  (DB の実際の値で書き込み・記録するので、書き込みの正しさには影響しない。設計レビュー 20 巡目で延期を許容)。context をまたぐ公開
  (★の ledger と同じ形) は、UI の制限を直す別の課題として backlog に記録する。facet・候補・smart folder の更新は mount 中の context で
  UI と同じく行う。

### 21.4 通知と記録

- **toast**: 受理した呼び出しごとに 1 つ。タグは受理の時点で「AI からのタグの変更を受け付けました (N 件)」(完了と読める文にしない)、
  ★とコレクションは結果の後に「AI が★を変更しました (N 件)」等。既存の toast は 1 枠なので、利用者の操作の toast と
  入れ替わることがある (UI の既存の扱いのまま)。
- **AI による変更の記録** (U11・U12): メモリだけに持つ表 (`Arc<Mutex<..>>`、最近 100 回分。1 回は最大 500 行)。AI 連携のページの
  「AI による変更」に、新しい順で 1 回ずつ (時刻・AI アプリの名前・tool・件数・要約) を出し、開くと項目ごとの行 (項目・前の値 →
  後の値・結果) を出す。行はコピーできる。「mIV を閉じると消えます」と書く。
  - タグ: 前後のタグは tag writer の結果 (`tags_before` / `tags_after`、agent の `tx_id` で対応) で埋める。書き込みの前は「処理中」。
    完了の採用は job と記録の対応 (typed な状態) で決め、表示の文言で判定しない (実装レビュー 2-2 の P2)。
  - ★: UI thread で書く前の値を cache / DB から読み、書いた後の値と並べる。
  - コレクション: 追加・外した項目、名前の変更の前後、作成した名前と id。**追加・外すの記録にも対象のコレクションの id と名前を
    残し、表示とコピーに含める** (同じ名前のコレクションを許すので、名前だけではどれへの変更か分からない。実装レビュー 2-1 の P2)。
    行の前後の値は、入力の中の重複ではなく snapshot に基づく実際の所属で書く (容量不足で入らなかった項目を「登録済み」と書かない)。
  - 記録が空の時も見出しと「変更の記録はありません」を出す (環境設定の検索の飛び先になるため。実装レビュー 2-1 の P3)。
  - 記録の表は agent server と App が共有する (`recent_apps` と同じ形の遅れた設置)。
- ログ: agent の lane のログは tool 名・件数・結果の種類だけ。ただし**通常のログは常時 ON** で (§5.4 の「利用者が有効にした診断
  ログ」は誤りだったので直す)、UI と共有するタグ・★の書き込みの処理は、利用者の操作と同じく変更した path・タグ名をログに残す
  (設計レビュー 19 巡目 P2-8)。AI の検索語・読み取りの結果を残さない約束 (§5.4) は変えない。書き込みの共有ログに変更対象が残ることを
  §5.4 と公開文書 (privacy) に書く。

### 21.5 状態を減らす検討

| 検討 | 結論 |
|---|---|
| AI の変更を戻す機能 | 作らない (U11)。記録を見て手で戻す |
| 記録を保存する | しない (U12)。mIV を閉じると消える |
| タグの書き込みの完了まで応答を待つ | 待たない。受理で応答し、結果は記録に出す (tag writer は UI と共有の非同期の queue) |
| コレクションも UI thread を通す | 通さない。actor が唯一の所有者で、UI は revision を見て追従する |
| revision の衝突で自動再試行 | しない。tool error で AI にやり直させる |
| ZIP の中の項目・PDF のページにタグ・コレクション | UI と同じく対象外 |
| 同じ名前のコレクションを拒む | 拒まない (UI と同じ)。id で指す |
| UI 要求の同時処理 | 全体で 1 件 (1c の規則を広げる) |

まれな失敗は既定の割り切り (ログ + tool error / toast)。

### 21.6 公開文書 (U10)

段階 2 と一緒に公開するので、この段階で §6 の公開文書 (製品ページの「安心して使えます」・`privacy.html` のネットワーク通信、
マニュアルの AI 連携の章) を書く。保存するデータは増えないので「端末内に保存されるデータ」は変えない (U12)。書き方は
`docs/public-wording-policies.md` に従う (バージョン固有の説明・実装用語を書かない)。

### 21.7 検証

- 自動テスト: tool ごとの fixture (タグ・★・コレクション)、許可の段 (4 段目が下の段を含む)、件数の上限、不正なタグ名、対象外の
  項目の `skipped`、書き込みを止める状態ごとの拒否 (副作用が無いこと)、Undo に載らないこと・利用者の保留中の Undo と編集の区切りを
  変えないこと、Remote の取得との順序、タグの改名・統合との双方向の排他、同じ値の再実行で時刻が変わらないこと、ZIP の旧 key の
  ★を 0 で外せること (同じ ZIP の A を外し、別の呼び出しで B を外し、その後に一覧画面で ZIP を開いても旧★が戻らないこと)、★の後処理 (件数の差分・XMP・公開が 1 回)、コレクションの `Conflict`・queue に入った後の期限と権限の変化、
  存在しないファイルを `remove_from_collection` で外せること、記録の内容と上限 (最新 100 回)、agent の lane のログに path が出ない
  こと、全書き込み tool の `readOnlyHint=false`、`create_collection` が冪等でないこと。記録の UI は 1093×614・長い文字列・最大の
  行数で。
- 使い捨ての portable と直接の stdio client で、各 tool を呼んで DB と記録の変化を見る。
- 実 AI アプリでの確認は、段階 1 の実機確認とまとめて利用者に頼む。

### 21.8 独立レビュー 19 巡目 (2026-11-01、設計 `5cd58ef75`)

判定は「stage 2 design: not ready」。UI 要求の受付の一般化、Undo の分離の材料 (tag writer の結果の `tx_id`・前後)、コレクションの actor への
直接の経路、★・タグ・コレクションの応答の分け方、メモリだけの記録の構造は妥当とされた。

| 指摘 | 判断 | 反映先 |
|---|---|---|
| P1 タグの改名・統合との排他が無い (全置換と別接続の改名が並ぶと改名後のタグを上書き) | 妥当。双方向の排他 | §21.3 |
| P2-2 ZIP の旧 key に残る★を 0 で外せない | 妥当。書く前に一覧画面と同じ key の移行 | §21.3 |
| P2-3 ★の共通の writer だけでは UI の後処理 (件数の差分・XMP・content identity) が揃わない | 妥当。共通の後処理を定義し、公開は 1 回 | §21.3 |
| P2-4 タグの入口は編集履歴の区切り・toast の枠に触れる | 妥当。agent の origin の下位の経路 | §21.3 |
| P2-5 別の context のタグの表示が古い | 既存の制限 (UI のタグ編集も同じ) として残す。context をまたぐ公開は detached の凍結に触れるので backlog | §21.3 |
| P2-6 コレクションの受理の確定点・期限・取消が無い | 妥当。queue に入った時を確定点 | §21.3 |
| P2-7 冪等と言いながら同じ値でも時刻を更新する | 妥当。差分・同値は書かない | §21.2・§21.3 |
| P2-8 通常のログは常時 ON で、共有の書き込みはタグ名・path を残す | 妥当。書き込みの共有ログは UI と同じとし、§5.4 と公開文書に書く | §21.4 |
| P2-9 `remove_from_collection` に存在の確認を掛けると欠けた参照を外せない | 妥当 | §21.2 |
| P3-10 「viewer を変えない」は公開の処理と矛盾 | 妥当。明示的な移動をしないだけと書いた | §21.3 |
| P3-11 ★の対象に動画・音声が無い | 妥当 | §21.2 |

20 巡目 (設計 `68c2c1ccf`): 19 巡目の指摘は P2-2 以外解消。新しい指摘: P2 1 件「ZIP 単位の移行は 1 度だけ走り後の項目を飛ばし、★以外の
DB も書き換える」→ その項目の新旧 key を rating writer の 1 つの transaction で正規化する形に直した。P3 2 件「context をまたぐ公開の延期の
理由が凍結の規則と合わない (延期は許容)」「タグの toast が完了の文」→ 理由を「既存の UI と同じ表示の制限として範囲外」に、toast を
受理の文に直した (§21.3・§21.4)。

21 巡目 (設計 `9666c5d81`): 追加の指摘なし。判定は「stage 2 design: ready」。

backlog の候補 (master の backlog への登録は親 session に依頼する): 「タグの cache は viewer context ごとで、書き込みの結果は mount 中の
context にしか反映されない。別の窓が持つ同じ項目のタグの表示は読み直すまで古い (利用者の UI 操作でも同じ)。★の ledger と同じ形の
context をまたぐ公開を足す」。

## 22. 段階 2-1 の実装記録

### 22.1 前提の照合と実装範囲

基点 `bcd9f555f`、branch `mcp-trial`、開始時 clean。観測者は実装担当 (root と担当を分けた
実装サブエージェント)。§21、U11/U12、許可 snapshot、AcceptedOperation、設定/記録の UI 規則を照合した。
独立した実装レビューを行ったという意味ではない。

- `CollectionStoreClient::request` は admission mutex 内の `try_send` 成功を `Ok(receiver)` とし、
  Starting/Busy/Unavailable を投入前の Err と区別している。これをそのまま受理点にできる。
  actor は receiver が捨てられても実行し、revision を公開する。App の collection grid は watch の
  revision を見て読み直すため、worker から actor へ書ける。rename の同名は既存 actor が NoOp にする。
- コレクション actor の生成は共通 readers より前。既存 producer lease から同じ client を clone して
  SharedReaders に設置した。DB connection/actor/settings reader は追加していない。各 write worker は
  producer lease を結果まで持つ。agent worker を止めて join → producer drain → actor shutdown の順序は維持。
- Remote の session snapshot の phase が Local であることを admission で確認できる。
  書き込みの起動完了判定は、App が frame 冒頭で initialized/overlay から公開する readiness を使う。
  未設置/起動中は拒否し、App drop で Uninstalled に戻す。detached の述語/viewport routing は変更しない。
- `prepare_collection_registrations` は worker 用で、Missing を Unresolved の registration にする。
  agent ではこの結果を NotFound の skipped にし、UI の既存動作は変えない。
  remove は shared input validator と CollectionSourcePath の構文/identity 正規化だけで source key を照合し、
  stat/存在/形式の確認をしない。削除済みの参照も外せる。
- 分類の perf と actor の mutation perf は件数/結果のみ。agent のログ formatter は tool・件数・所要時間・
  outcome/error kind のみで、名前/path/actor のエラー詳細を直列化しない。

対象は Organize、変更記録、コレクション4 tool のみ。タグ/★の tool、一般化した UI 要求、Undo、
公開文書、永続化する記録、移行は追加していない。前提に矛盾は見つかっていない。

### 22.2 実装上の選択

- Agent IPC v2 は未リリースのまま。Organize と command/result/Conflict を同じ型定義へ追加した。
  旧版と互換であるとは主張しない。core と adapter は同じ source を build する。
- wire の作成結果は id、rename は id/changed。名前の前後はメモリの記録へ持ち、応答で繰り返さない。
  actor の名前は trim 空以外の長さ制限が無いため、新たな制限で UI と異なる名前を拒むことはしない。
- 500件の結果は入力の zero-based index を status/reason ごとにまとめる (最大9 group)。path を echo せず、
  各 index をちょうど1度返す。これにより書き込みを分割/再実行せず既存の最終 MCP 出力予算に収める。
  空配列/501件以上は全体の BadRequest、個々の不正な path/欠落/対象外/subresource は skipped。
- 受付状態は Preparing/Stopped/Accepted の一つの mutex owner。期限の controller と actor enqueue が同じ
  境界を共有する。設定 DB 読み取り/分類/actor の応答待ちはこの lock の外。Accepted 以後は cancel/EOF/
  期限/権限変更で結果を置き換えず、actor の結果を採用し記録して AcceptedOperation で返す。
  受付前の時間切れを返した場合も worker が終わるまで permit を保持する。
- Conflict と Busy は再試行しない。queue に入った後の Conflict/失敗も1回の失敗記録と toast にする。
  同じ batch で capacity rejected の key が再登場した時は、先頭を capacity_rejected、後続を duplicate とする
  (actor の batch_seen と同じ意味)。
- 記録は最新100回・1回500行、immutable Arc snapshot で UI に渡す。描画中は mutex を持たず、
  競合時は前の snapshot を使う。2-2 の queued record 更新用に id/update の小さな接続口を残した。
  collection の記録は完了/採用時刻と publication 順で出し、保存/Undo/復旧の仕組みは足していない。
- 記録が空の時は新しい section を描かず、最初の記録以降に「AI による変更」を出す。
  §21 は empty state の常時表示を要求しておらず、既存 PNG を更新しない条件とも両立する。
  level4 の新しい明暗 snapshot は記録入りで追加する。

### 22.3 検証の途中経過

- root: `cargo check -p mimageviewer --bin mimageviewer-core` exit 0 (1m07s、
  `target/mcp-stage2-1-core-check.log`)。
- protocol 実装担当: `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp` exit 0、14/14 と60/60。
  `--include-ignored` 無し。保存ログ無し、tool session 58039 で観測。担当 agent の環境では実 pipe 2件も成功。
  root の既知 sandbox OS5 の過去結果でこの観測を置き換えない。必須の include-ignored 版は最終検証へ残す。
- glyph lint は UI 実装担当が1回実行し exit 0。root の最終 source でも再確認する。
- 最初の lib-test compile は root の Server initializer の cfg(test) write_hooks 欠落で exit 101。
  initializer を補い、UI 担当の focused を再実行中。これは製品の実行結果ではない。

本節は途中経過。最終コマンド/PNG/残りの確認は次節へ追記する。commit・製品起動・実 client config の
読書きはしていない。

### 22.4 最終検証と追加 PNG

以下の root は実装担当自身。担当サブエージェントの検証も実装担当の確認であり、独立レビューではない。

| コマンド | 観測者 | 実 exit | 結果 |
|---|---|---:|---|
| `cargo fmt` / `cargo fmt --check` | root | 0 / 0 | workspace 全体。`target/mcp-stage2-1-fmt.log`、`target/mcp-stage2-1-fmt-check.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` (最終) | root | 0 | `target/mcp-stage2-1-core-check-final.log`、1m29s |
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` | root | 101 | agent-ipc 14/14、MCP 58成功/2失敗。`target/mcp-stage2-1-crate-tests.log` |
| collection write focused nextest | collection test 担当 | 0 | 11/11、`target/mcp-stage2-1-collection-tests.log`、実行0.616s |
| changes/App/preferences focused nextest | UI 担当 | 0 | 21/21、`target/mcp-stage2-ui-focused-final.log`、実行3.324s |
| §19.6 + collection_store + agent_change_ui_tests focused nextest | root | 100 | 863件中861成功/2失敗、`target/mcp-stage2-1-focused.log`、実行54.334s |
| 新規2枚だけ UPDATE_SNAPSHOTS=1 で `cargo test --test ui_snapshot preferences_agent_organize_` | UI 担当 | 0 | 2/2、`target/mcp-stage2-ui-new-snapshots.log` |
| `cargo test --test ui_snapshot` (更新 mode 無し、全件比較) | root | 0 | 222/222、`target/mcp-stage2-1-ui-snapshot.log`、実行39.43s |
| `python scripts/check_ui_glyphs.py` (最終 source) | root | 0 | `target/mcp-stage2-1-glyphs.log` |
| `git diff --check` | root | 0 | LF/CRLF warning のみ、`target/mcp-stage2-1-diff-check.log` |

collection focused の正確なコマンド:

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(agent_ipc::collection_write_tests)' --no-tests fail --ignore-default-filter --run-ignored default --retries 0 --no-fail-fast
```

root の最終 focused filter は §19.6 の全条件に actor と App の変更記録テストを加えたもの:

```powershell
$stage2Focused = '(test(/agent_ipc/) or test(/agent_level/) or test(/agent_permission/) or test(=settings_db::tests::failed_permission_commit_preserves_snapshot_and_epoch) or test(/remote_ipc::live_favorites/) or test(/settings_transfer::tests::automatic_network_enablement/) or test(/png_metadata/) or test(=pdf_loader::tests::anonymous_page_sizes_keep_worker_path_out_of_diagnostics_on_all_results) or test(=pdf_loader::tests::page_sizes_preserve_existing_ui_diagnostic_identity) or test(=pdf_loader::tests::page_sizes_round_trip_and_reject_a_truncated_reply) or test(/app::folder_scan/) or test(/zip_loader/) or test(/conceal_db/) or test(=pdf_loader::tests::anonymous_agent_render_and_enumerate_omit_keys_and_harvest_actual_completion) or test(=pdf_loader::tests::cancelled_anonymous_render_does_not_enqueue) or test(/app::startup_ops/) or test(/single_instance/) or test(/remote_ipc::session/) or test(/ui_request_dispatch/) or test(/activation/) or test(/startup_open_path/) or test(/ui_dialogs::preferences/) or test(/agent_registration/) or test(/process_images/) or test(/runtime_cleanup/) or test(/collection_store/) or test(/agent_change_ui_tests/)) and not test(/section1375_scan_perf_tests/)'
cargo nextest run -p mimageviewer --lib -E $stage2Focused --no-fail-fast
```

追加 PNG (既存 PNG の更新は無し):

- `tests/snapshots/preferences_agent_organize_light.png` (243,220 bytes)
- `tests/snapshots/preferences_agent_organize_dark.png` (240,973 bytes)

合計484,193 bytes。UI 担当と root が両方を目視し、4段目・3列・変更記録・登録行が明暗で読めることを確認。
既存6枚の AI 設定 PNG の hash は生成前後で不変、root の全222件比較も成功した。
geometry は PNG を増やさず、1093×614・100/200%・長文/最大500行で実 wheel の末尾到達と CopyText を検証。
16組の説明、100回/500行 cap、queued 更新/evict、App readiness/drop/再設置/toast、create 同名2回、
rename NoOp、Conflict/Busy、受付前後の cancel/期限/権限変更/切断、消えた file の remove、ZIP/PDF
subresource skipped、500/501、容量と duplicate の優先、ログの path/name/error detail 非露出を確認した。

root で失敗したのは、既知の sandbox の pipe open OS error 5 の以下4件のみ (skip/ignore は追加していない):

- MCP: `pipe::tests::real_pipe_cancel_reaches_core_while_stdio_stdout_is_blocked`、
  `pipe::tests::real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld`
- core: `agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry`、
  `agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers`

### 22.5 変更ファイルと引継ぎ

- wire/adapter: `crates/agent-ipc/src/lib.rs`、`crates/mcp-server/src/{lib.rs,schema.rs,tests.rs}`。
- core: `src/agent_ipc/{mod.rs,tests.rs}`、新規 `changes.rs`、`collection_write.rs`、`collection_write_tests.rs`。
  `src/remote_ipc/mod.rs` は shared client の設置/取得だけ。既存 Remote helper/engine の body と protocol 72 は不変。
- integration/settings/UI: `src/{app.rs,lib.rs,settings_db.rs}`、
  `src/ui_dialogs/preferences/{agent_integration.rs,agent_integration/tests.rs,pages.rs,search_index.rs}`、
  `tests/ui_snapshot.rs`、上記新規 PNG 2枚、本計画。

sandbox 外で ClaudeCode が行う最小の追加検証:

```powershell
cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored
cargo nextest run -p mimageviewer --lib -E 'test(=agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry) or test(=agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers)'
```

861成功の同じ source/条件の結果は再利用できる。全 focused を外で確認する場合は §22.4 のコマンドを使う。
実 core/AI アプリでの変更・画面追従・通知/記録の確認は未実施。所定の test green 前提が sandbox 内では
OS5 で揃わないため、新しい実機確認用 build-dev は作っていない。必要時は外部担当が
`.\scripts\build-dev.ps1 -PreserveRuntime` で core と対応する MCP を同時に更新する。
test-full は実行していない。タグ/★/UI 要求の一般化・公開文書は計画どおり2-2以降へ残す。
本 slice の実装の未完項目/追加の設計矛盾は無い。commit・製品起動・実 client config の操作はしていない。

### 22.6 実装レビュー 1 巡目の対応

対象は実装 `e644613fd` と設計追記 `dee5e011f`。3件とも実装担当がコード上で再確認し、
指摘どおりの経路を確認した。以下の確認は実装担当自身 (担当サブエージェントを含む) のもので、
独立レビューの結果ではない。

#### 指摘と修正・回帰

- **P2: 入力内の重複を実際の所属として記録していた**。
  `collection_write::item_rows` は actor の入力位置ごとの outcome から所属を推定せず、
  受理前 snapshot と actor が返した結果 snapshot の source key を照合して前後を求める。
  wire の Duplicate/CapacityRejected 等の意味と件数は変更していない。
  受理後に actor が失敗した場合に備える初期行も、受理前 snapshot 同士で作る。
  追加だけでなく、同じ項目を繰り返して外す場合にも同じ所属基準を適用した。
  回帰: 満杯への `[X, X]` の両行は未登録→未登録、既存メンバーは登録済み→登録済み、
  同じ呼び出しで追加された項目の重複行は未登録→登録済み、繰り返し削除は登録済み→未登録。
- **P2: 同名コレクションへの操作を識別できない記録**。
  `ChangeRecord` に `Option<CollectionTarget { id, name }>` を設け、add/remove が snapshot の
  id/name を受理前に保持する。受理後の失敗記録にも対象を残す。展開した記録と個別行の表示、
  記録全体/個別行のコピーに両方を含める。create/rename と後続のタグ/★用の記録では必須にしない。
  回帰: 実 actor で同名の2コレクションへの add/remove を行い各 id/name を検証し、
  headless UI でも同名2件の表示と両方のコピーボタンの CopyText を検証する。
- **P3: 空の記録欄に検索 anchor がない**。
  `pages::page_agent_integration` は `agent/changes` を常時描画し、見出し、
  「mIV を閉じると消えます。最新 100 回分を表示します。」、空なら「変更の記録はありません」を出す。
  回帰: 実際の設定検索の entry を選び、anchor 消費/highlight と3つの文言の viewport 内表示を検証する。

変更は `src/agent_ipc/{changes.rs,collection_write.rs,collection_write_tests.rs}`、
`src/ui_dialogs/preferences/{agent_integration.rs,agent_integration/tests.rs,pages.rs}`、
`src/app.rs` (既存テストの record 初期化)、影響する snapshot PNG、本節。
actor、Remote、IPC wire、設定の保存、UI thread の I/O 経路は変更していない。

#### 検証・PNG

| コマンド | 観測者 | 実 exit | 結果 |
|---|---|---:|---|
| `cargo fmt` | root | 0 | workspace 全体。`target/mcp-stage2-1-review1-fmt.log` |
| `cargo fmt --check` | root | 0 | `target/mcp-stage2-1-review1-fmt-check.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | root | 0 | 33.05s。`target/mcp-stage2-1-review1-core-check.log` |
| collection + changes/App/preferences focused nextest (下記) | UI 担当 | 0 | 224/224。compile 3m59s、実行37.456s。`target/mcp-stage2-1-review-ui-focused.log` |
| AI ページ8件だけ UPDATE_SNAPSHOTS=1 (下記) | UI 担当 | 0 | 8/8。実行2.75s。`target/mcp-stage2-1-review-ai-update.log` |
| `cargo test -p mimageviewer --test ui_snapshot` (更新指定なし、全件比較) | UI 担当 | 0 | 222/222、実行41.39s。`target/mcp-stage2-1-review-ui-snapshot-all.log` |
| `python scripts/check_ui_glyphs.py` | root | 0 | 危険な UI glyph なし。`target/mcp-stage2-1-review1-glyphs.log` |
| `git diff --check` | root | 0 | LF/CRLF warning のみ。`target/mcp-stage2-1-review1-diff-check.log` |
| `powershell -NoProfile -File scripts\build-dev.ps1 -PreserveRuntime` | root | 0 | 規約の確認用 build。core/remote/EPUB worker/MCP を `target/dev-runtime` に生成、PE check `runtime=4 pe=4` 成功。起動なし。`target/mcp-stage2-1-review1-build-dev.log` |

指定された collection と changes/App/preferences の範囲を一つの focused run にまとめ、preferences は全体へ広げた。
新規3テスト (同名対象の actor 記録、同名対象の表示/コピー、空の検索 anchor) と既存テストへの所属 assertion を含む。
正確なコマンド:

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(agent_ipc::collection_write_tests) or test(agent_ipc::changes) or test(agent_change_ui_tests) or test(ui_dialogs::preferences)' --no-tests fail --ignore-default-filter --run-ignored default --retries 0 --no-fail-fast
$env:UPDATE_SNAPSHOTS = '1'
cargo test -p mimageviewer --test ui_snapshot preferences_agent_
Remove-Item Env:UPDATE_SNAPSHOTS
cargo test -p mimageviewer --test ui_snapshot
```

変更した PNG は次の6枚だけ (新規なし、合計1,262,076 bytes)。root と UI 担当が全6枚を目視し、
明暗とも空状態の3文言が読め、登録操作まで枠内に収まること、重なり/欠けがないことを確認した。

- `tests/snapshots/preferences_agent_disabled_dark.png` (199,018 bytes)
- `tests/snapshots/preferences_agent_disabled_light.png` (200,884 bytes)
- `tests/snapshots/preferences_agent_open_dark.png` (224,559 bytes)
- `tests/snapshots/preferences_agent_open_light.png` (226,610 bytes)
- `tests/snapshots/preferences_agent_read_dark.png` (204,587 bytes)
- `tests/snapshots/preferences_agent_read_light.png` (206,418 bytes)

`preferences_agent_organize_light.png` / `preferences_agent_organize_dark.png` は byte 単位で不変。
コレクション id/name は展開後の本文にあり、同名の表示/コピー回帰と長文500行・100/200%の既存 geometry 回帰で検証した。
他の snapshot に更新指定はしていない。

この修正で IPC/adapter や実 pipe の経路は変更しておらず、指定範囲外の crates/実 pipe テストは再実行していない。
製品の対話確認は未実施。commit・製品起動・実 client config の操作はしていない。
追加の設計矛盾や、3件の修正で残る未完項目はない。
確認用 binary は通常 profile (portable feature なし) で、既定では実 `%APPDATA%\mimageviewer` を使う。
利用者が確認する際は、常駐/インストール版を閉じてから
`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`。
通常 profile の実設定・データを更新し得る。確認対象は AI 連携ページの空の記録欄への検索ジャンプ、
同名コレクションへの変更記録を展開/コピーした時の id/name、容量不足の重複入力の前後の所属表示。

### 22.7 検収 (ClaudeCode、2026-11-01)

- sandbox の外で ClaudeCode が実行 (`e644613fd` と同じ source): `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp --
  --include-ignored` exit 0 (14/14、60/60)、core の実 pipe 2 件の nextest exit 0 (2/2)。
- 新規 PNG 2 枚と、空の記録の表示で撮り直した 6 枚を ClaudeCode が目視 (明暗とも読める)。
- 独立した実装レビュー 1 巡目 (`e644613fd`): P2 2 件 (容量不足の重複を記録で「登録済み」と書く、追加・外すの記録にコレクションの
  id が無い)、P3 1 件 (記録が空の時に環境設定の検索の飛び先が無い)。設計書 §21.4 を `dee5e011f` で直し、実装を `228185d54` で
  直した (§22.6)。生の tool 名を日本語の要約と並べる表示は、AI 側の呼び出しと照合できるので維持でよいという判定。
- 2 巡目 (`228185d54`): 追加の指摘なし。判定は「2-1 impl: accepted」。
- 実 AI アプリ・実 core での変更・画面の追従の確認は未実施 (段階 1 の実機確認とまとめて利用者に頼む)。

## 23. 段階 2-2 の実装記録

### 23.1 前提の照合と担当範囲

基点 `0df285ac2`、観測者は実装担当 (担当サブエージェントを含む)。独立レビューではない。
§21 全体、§22、§15.6/§18、§5.4 とタグ/XMPの保存先を確認し、次を実装前にコードで照合した。

- 1c の `OpenController` は一つの active permit と `UiRequestDispatch` を持ち、未設置拒否・5秒の
  Pending取消・Claimed後の応答待ちを所有する。`open_response` の外側20秒期限とも同じ CAS を共有している。
  `startup_ops` の App poll は初期open後/二重起動poll前で、起動overlayの前にも拒否する。
- `tag_ops::request_tag_set_for_targets` の上位には編集履歴の区切り・Undo・利用者用toastがある。
  worker は `tx_id` と DB の `tags_before` / `tags_after` を返すので、その下で origin を分けられる。
  現行 Add/Remove の全置換は、差分操作に置き換える必要があった。
- `TagWriteHandle::has_unconsumed_batch` は存在するが、結果公開より前に done を進めていた。
  rename/mergeとの排他は、結果の採用までを含めて閉じる必要がある。
- ★には deferred writer と App-global ledger があり、item と named-container で後処理が分かれていた。
  共通化には DB だけでなく cache/count/identity/smart-folder/XMP を含める必要がある。
  `RatingWriteHandle` は XMP 専用で、正本DBへの同期書き込みの所有者ではない。
- ZIP の現行名/CP932旧名は1bの同じ読取から得られる。PDFページの既存指定は厳密な `page_N`。
  RatingMeta (briefの RatingItemMeta の材料に相当) と各 page key の既存の構築規則を使う。
  製本ページの判定用 `Settings::books_root_path` は既定時にOSの既知フォルダを解決するため worker 側で済ませる。
- `ChangeController::publish/update` はキュー中の行を同じrecord idで埋め直せ、evict済みを復活させず、
  更新時に2回目のtoastを出さない。新しい永続表・settings接続・Remote writerは不要。

設計の前提を覆す矛盾は確認していない。detached述語・viewport routing・contextをまたぐタグ公開は変更しない。

### 23.2 実装と選択

- IPC v2 (未出荷のため据え置き) に `AddTags` / `RemoveTags` / `SetRating` と grouped typed outcome を追加。
  Organizeだけが新3 toolを公開し、合計18 tool。上限500項目・10タグ・★0～5、指定annotations、schema、
  structuredContentと同じtextを実装。タグの説明は受理と完了を区別し、実際の結果を変更記録で見る旨を書く。
- UI要求は一つの既存controller/channel/permitを保ち、kindを `Open` / `TagWrite` / `RatingWrite` に広げた。
  呼出し元のhook/内部controller名は既存を保って変更を狭めた。共通の既存activation eventでhidden UIも起こし、
  metadata要求からresolverやfolder/viewer openは呼ばない。Openのassertionの期待値は変えず、テスト中のpayload取得だけ
  enumのOpen variantへ合わせた。
- `metadata_write` は全タグ名を先に正規化/検査し、項目のpath・存在・種類・ZIP名・PDF範囲・RatingMeta・
  sidecar/XMP対象・製本ページflagをworkerで用意する。現在設定は既存settings_dbから読む。
  ZIPは共有readのidentity-only入口を追加し、葉の画像を展開しない。入れ子ZIPのbytes読取は既存64MiB上限/取消を保つ。
  既存 `read_zip` の画像bytes経路とUI/searchの解釈は不変。PDFは無記名のsizes読取を使い、実IPCが戻るまでpermitを保つ。
- Appはclaim後に許可と全blocking stateを副作用なしで検査する。RemoteのLocal確認と受理の確定だけを
  `with_local_composition_admission` の中で行い、DB/公開/recordはlock外。起動中・改名統合(タグのみ)・製本処理・
  Append Undo・メタ情報処理・rename移行・sidecar復元・構成編集/保存・文書modalに理由つきで拒否する。
  `Written` の結果は成功/失敗とも AcceptedOperation。Pendingだけが取消/期限で止まり、Claimed後は一度のUI応答を待つ。
- タグは `submit_tag_set_lower` をUIと共有し、agentは上位の編集境界/Undo/user-toastを通らない。
  tx_idは利用者と同じcheckedな単一UI発行元。typed originはrecord id/行と対応し、writer結果をUndoと利用者toastの集計より先に分ける。
  正本DBは差分で変更し、既存タグの時刻/表示名を保持、no-opでは保存しない。recordは受理時「処理中」、完了後に実DBの前後で更新する。
  ユーザーtoast用の未採用カウンタを全jobのカウンタから分け、agentだけの処理が利用者の保留ラベル/発火面を消さない。
- タグの改名・統合はjobまたは未採用結果がある間は始めない。結果送信→done Release、UIはdone Acquireと
  result queueのdrained確認の後にカウンタをretireする。改名中はタグのUndo/Redoだけをpop前に拒否し、★・補正のUndo/Redoは保つ。
  実際にagentのタグ変更を採用した時だけAppend Undo受領票を無効化する (no-op/失敗/拒否では触らない)。
- ★は `write_user_rating_canonical_shared_deferred_from` と既存writerがcommit後のledger/log境界を共有し、
  item/named-container/agentが同じ後処理を通る。typed policyで既存UIのcache無効化・smart refresh条件を保持する。
  agentはDB/cache/親件数/count invalidation/identity/smart refresh/XMPを適用し、公開/再構築は1回。
  同値ではDB/ledger/XMPへ書かない。ZIPは各項目のtransactionで旧row削除と新keyへの設定/0削除を行い、
  前値は新key優先→旧key。既存のXMP hydration fenceをDB書込前に立て、ZIP単位のmigrationは呼ばない。
  XMPは利用者設定ONかつ既存UIの対象画像のみ、製本ページflagはworkerで確定。XMP_WRITE_LOCKの既存workerを使う。
- tool一覧の最初の実装は44,538文字で既定40,000を超えた。上限や機能は変えず、get_item_info/write tool内の
  同一schemaをローカル `$defs` へまとめ、説明を意味保持で短くした。展開後のschema同等性と、全権限/画像/GPS/legacy・modern/
  numeric・UUID・64文字idで512文字以上の余裕を検査する。これはbudgetに収める実装調整で、上限緩和や新たな機能制限ではない。
- agent laneはtool/count/outcome/durationのみ。共有tag/rating writerのpath/タグログは既存の方針のまま。
  public docs、クロスcontextのタグ公開、collection toolは本sliceで変更しない。

### 23.3 検証の途中経過

以下は実装担当自身の観測であり、sandbox外の観測を代用していない。

- 最初のcore check: exit 0、59.59s (`target/mcp-stage2-2-core-check-first.log`)。
- crateの途中: test側のid整数型compile mismatch (exit 101) と、18tool一覧のbudget超過 (exit 101) を修正した。
  budget調整後の全crate gateは exit 101、agent-ipc 15/15、MCP 63成功/2失敗 (既知のpipe OS5のみ)。
  `target/mcp-stage2-2-crate-tests.log`。最終の説明短縮後のstage2回帰6/6とschema参照展開同等1/1はexit 0
  (`target/mcp-stage2-2-adapter-targeted.log`、`target/mcp-stage2-2-schema-equivalence.log`)。
- core新規focusedの初回はexit 101。一般化後も既存Openテストが旧struct fieldを直接読んでいた5か所のcompile errorで、
  Open variantを読むtest helperへ合わせた。期待値やケースは変えていない (`target/mcp-stage2-2-new-focused.log`)。

### 23.4 最終検証・引継ぎ

すべて実装担当自身のsandbox内の観測。途中の新規focusedは59件中58成功/1失敗でexit 100
(`target/mcp-stage2-2-new-focused-final.log`)。export/importのテストfixtureに実フォルダの走査済みmarkerがなく、
既存のUI入口が正しく拒否していたため、fixtureを整えた (製品の検査は緩めていない)。最終の全focusedで当該15状態のテストも成功した。

| コマンド | 実 exit | 結果 / ログ |
|---|---:|---|
| `cargo fmt` / `cargo fmt --check` | 0 / 0 | workspace全体。`target/mcp-stage2-2-fmt.log`、`target/mcp-stage2-2-fmt-check.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` (最終) | 0 | 39.78s、`target/mcp-stage2-2-core-check-final.log` |
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` (最終) | 101 | IPC 15/15、MCP 63成功/2失敗。`target/mcp-stage2-2-crate-tests-final.log` |
| §22.4 + tag/rating/Undo/Remote collection focused nextest (下記) | 100 | 1,516件中1,514成功/2失敗。compile 2m49s、実行99.737s。`target/mcp-stage2-2-focused.log` |
| `cargo test --test ui_snapshot` (UPDATE指定なし) | 0 | 222/222、実行38.50s、`target/mcp-stage2-2-ui-snapshot.log` |
| `python scripts/check_ui_glyphs.py` (最終) | 0 | 危険なUI glyphなし、`target/mcp-stage2-2-glyphs.log` |
| `git diff --check` | 0 | LF/CRLF warningのみ、`target/mcp-stage2-2-diff-check.log` |

新規/変更した回帰は、入力/権限/schema/budget、実Connection→worker準備→共有UI CAS、Openとmetadataの共有Busy、
Pending取消/期限とlate claim拒否、Claimed後の取消/期限/権限降格/切断と結果/帰属保持、15種類のApp拒否状態の副作用なし、
実DBタグ前後値のrecord更新、利用者の保留Undo/補正draft/toast保持、タグ改名との双方向の排他、時刻保持/再実行no-op、
ZIP新旧keyのtransaction/rollbackとA0→別callのB0→実migration後の非復活、★の件数差分/XMP/identity/1回公開/smart membership、
agentのcommit後に同じ実Remote readerが更新を読む順序を含む。既存Openのケース/期待値とUI tag/rating経路の回帰も通った。

失敗は次の既知のsandboxのrestricted tokenによるpipe open OS error 5だけ。DACLを緩めず、skip/ignoreを増やしていない。

- MCP: `pipe::tests::real_pipe_cancel_reaches_core_while_stdio_stdout_is_blocked`
- MCP: `pipe::tests::real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld`
- core: `agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry`
- core: `agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers`

実行したfocusedの正確なコマンド (§22.4を保持して関連writer/UI/DB/取得を追加):

```powershell
$stage22Focused = '((test(/agent_ipc/) or test(/agent_level/) or test(/agent_permission/) or test(=settings_db::tests::failed_permission_commit_preserves_snapshot_and_epoch) or test(/remote_ipc::live_favorites/) or test(/settings_transfer::tests::automatic_network_enablement/) or test(/png_metadata/) or test(=pdf_loader::tests::anonymous_page_sizes_keep_worker_path_out_of_diagnostics_on_all_results) or test(=pdf_loader::tests::page_sizes_preserve_existing_ui_diagnostic_identity) or test(=pdf_loader::tests::page_sizes_round_trip_and_reject_a_truncated_reply) or test(/app::folder_scan/) or test(/zip_loader/) or test(/conceal_db/) or test(=pdf_loader::tests::anonymous_agent_render_and_enumerate_omit_keys_and_harvest_actual_completion) or test(=pdf_loader::tests::cancelled_anonymous_render_does_not_enqueue) or test(/app::startup_ops/) or test(/single_instance/) or test(/remote_ipc::session/) or test(/ui_request_dispatch/) or test(/activation/) or test(/startup_open_path/) or test(/ui_dialogs::preferences/) or test(/agent_registration/) or test(/process_images/) or test(/runtime_cleanup/) or test(/collection_store/) or test(/agent_change_ui_tests/)) and not test(/section1375_scan_perf_tests/)) or test(/tag_write_worker/) or test(/tags_db/) or test(/tag_ops/) or test(/undo_ops/) or test(/rating/) or test(/ui_dialogs::tag_editor/) or test(/remote_ipc::collections/)'
cargo nextest run -p mimageviewer --lib -E $stage22Focused --no-fail-fast
```

PNG: **追加0、変更0**。UPDATE_SNAPSHOTSは未設定で全222枚を比較し、`git diff --name-only -- tests/snapshots` に差分なし。
新しい設定UI/固定画面は追加していない。タグ管理のgateは既存の実ダイアログをheadlessで操作する回帰で検査した。

変更ファイル:

- wire/adapter: `crates/agent-ipc/src/lib.rs`、`crates/mcp-server/src/{lib.rs,schema.rs,tests.rs}`。
- agent: `src/agent_ipc/{mod.rs,open.rs,open_tests.rs,item_info.rs}`、新規 `metadata_write.rs`、
  `metadata_write_tests.rs`、`metadata_response.rs`、`metadata_response_tests.rs`。
- App: `src/app.rs`、`src/app/startup_ops.rs`、新規 `src/app/{agent_metadata_write.rs,agent_metadata_write_tests.rs,agent_rating_write.rs}`。
  `src/app/smart_folder.rs` と `src/remote_ipc/collections.rs` はテストの追加だけ。
- 共有書込: `src/{tag_ops.rs,tag_write_worker.rs,tags_db.rs,undo_ops.rs,rating_db.rs}`、`src/ui_dialogs/tag_editor.rs`。
  `xmp_writer.rs` / `rating_write_worker.rs` の実シリアライズ/lock処理は変更していない。
- 本計画。新しい依存、DB schema、public docs、collection toolの変更、detached述語/viewport routingの変更は無し。

sandbox外でClaudeCodeが再実行する最小コマンド (既知pipe 4件を実際に通す):

```powershell
cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored
cargo nextest run -p mimageviewer --lib -E 'test(=agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry) or test(=agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers)' --no-fail-fast
```

全focusedを外で確認する場合は上記 `$stage22Focused` を使う。成功済み1,514件と222 snapshotは、同じsource/条件なら再利用できる。
このsliceではreal pipeのgreen前提がsandbox内で揃わないため、新しい確認用build-devは作っていない。
対応するcoreとMCPの実機確認用binaryは、外で上のpipe gate後に `.\scripts\build-dev.ps1 -PreserveRuntime` で同時に更新する。
以前の `target/dev-runtime` は2-1のままなので、今回の新toolを確認するbuildとして扱わない。
実core/AIアプリでの書込・画面追従・XMPの外部ExifTool確認は未実施 (XMP jobは一時PNGで実workerを通して検査済み)。
正式な独立実装レビューは別threadで行う。実装担当の自己点検で追加の設計矛盾や未完のscope項目は検出していない。
タグのcross-context公開・public docsは指定どおり範囲外。test-full、commit、製品起動、実client configの操作はしていない。

### 23.5 実装レビュー 1 巡目の対応

対象は実装 `c39489f5d` と設計追記 `956b46a25`。3件のP2と設定分類テストの指摘は、いずれも実装担当が
コードで再確認し、指摘どおりだった。以下の自己点検/検証は実装担当 (担当サブエージェントを含む) のもので、
正式な独立レビューではない。

前回sourceのsandbox外の観測 (観測者: ClaudeCode、今回のbrief): `test-lib-full.ps1` は15,275成功で、
残る失敗は `settings_transfer::tests::all_settings_fields_are_classified` の固定件数だけ。
crates `--include-ignored` はIPC 15/15、MCP 65/65、exit 0。この結果を今回実行した検証としては扱わない。

#### 指摘 → 修正 → 回帰

1. **P2: タグ記録が処理中のまま**。
   `ChangeRow.result` を `Pending / Done(ChangeRowOutcome) / Failed(String)` とし、
   `ChangeRowOutcome` は `Changed / Unchanged / Text(String)`。Tag jobのcontroller・record id・row indexの
   typedな対応とPending状態で完了を採用し、二重完了/別ownerの結果を拒否する。
   処理中の前後値は空で、処理中/変更/変更なし/失敗の文言はUI描画/コピーでだけ生成する。
   `Text` は既存の同期結果/fixtureの表示を保持するためのデータで、完了の判定には一切使わない。
   実際の `App::admit_agent_metadata_write` → 実tag writer → App採用 → recordを、add/removeそれぞれの
   成功・no-op・DB triggerによる失敗の計6ケースで検査する。skipped行の後のqueued行、件数、実DBの前後、
   cache、Undo不変、古いUI snapshotの不変性も確認する。owner照合/二重完了とUI表示の回帰も追加した。
2. **P2: 利用者の完了toastの件数がpollをまたぐと失われる**。
   App所有の `TagWriteCompletion` に、採用したuser-originの件数/エラーをbatch retirementまで保持する。
   label/surfaceをその間保持し、completeかつdrainedを確認した時に一度だけ取り出して通知する。
   agentの件数は集計に含めない。send→done Release、Acquire/drainedの順序は変えない。
   失敗も完了時に一度のwarningへまとめ、途中の失敗が後の成功toastで隠れることを防ぐ。
   失敗のログとagent失敗行の採用は従来どおり直ちに行い、既通知フラグ等の別の通知状態は作らない。
   決定的なwriter test driverで、送信とdoneを分け、user採用→agent遅延、送信後/done前の採用、
   user失敗→後の成功を実pollで検査。label/surface・無関係の保留Undo・最終件数・通知1回を確認する。
3. **P2: ZIPフォルダ/入れ子ZIP本を★の対象外にしていた**。
   末尾 `/` のZIP subresourceは中央ディレクトリのidentity-only indexで存在を確認し、
   UIの純粋な `book_container_key` (visibilityだけpub(crate)化、body不変) と同じkeyを使う。
   RatingMetaは `ZipDir`、outer source、current dir_prefix、UIと同じarchive flag。新旧名のbook keyは
   既存canonical rating writerへ渡し、legacy rowの削除も同じtransactionで行う。
   葉の画像/末端ZIPの本文は読まず、必要な中間ZIPだけ既存64MiB/取消の下で読む。不在はskipped NotFound。
   ZIPフォルダ・空フォルダ・入れ子本・不在・実Appのkey/meta一致・旧名入力・中間上限/取消・本文未読と無書込を回帰に追加した。
   自己点検でCP932末尾byte 0x5c (`表` 等) による旧名の見かけの区切り増加も確認し、
   同じcomponent数での切り出しをやめた。1つのraw name境界から現在名と旧名を作り、全entryと同じdecode選択を保つ。
   文字途中の見かけの区切りを除外し、`表/inner/`・`表inner/`・入れ子 `表.zip`・旧名入力を追加で検査する。
   `set_rating` の英語説明にもZIPフォルダ/入れ子本を含め、既存の文字数予算を維持する長さに収めた。
4. **分類の固定件数**。
   `agent_level` / `agent_show_thumbnails` / `agent_show_gps` は元から「移行先の許可を維持」の理由付きで分類済み。
   production分類は変更せず、期待する総数を482、理由付き329へ修正 (理由なし153は不変)。
   下記focused filterへ `test(=settings_transfer::tests::all_settings_fields_are_classified)` を明示追加した。

変更ファイル: `src/agent_ipc/{changes.rs,collection_write.rs,collection_write_tests.rs,item_info.rs,metadata_write.rs,metadata_write_tests.rs}`、
`src/{app.rs,tag_ops.rs,tag_write_worker.rs,settings_transfer.rs}`、`src/app/{agent_metadata_write.rs,agent_metadata_write_tests.rs}`、
`src/ui_dialogs/preferences/{agent_integration.rs,agent_integration/tests.rs}`、`crates/mcp-server/src/schema.rs`、本節。
コレクション/★の同期rowとfixtureは型への移行だけ。Remote body・detached述語/viewport routing・DB schema・IPC wireの変更は無し。

#### コマンドと結果

今回の全focusedは、§23.4の `$stage22Focused` 定義に分類テストを追加したもの:

```powershell
$stage22ReviewFocused = "($stage22Focused) or test(=settings_transfer::tests::all_settings_fields_are_classified)"
cargo nextest run -p mimageviewer --lib -E $stage22ReviewFocused --no-fail-fast
```

| コマンド | 観測者 | 実 exit | 結果 / ログ |
|---|---|---:|---|
| `cargo fmt` / `cargo fmt --check` | root | 0 / 0 | workspace全体。`target/mcp-stage2-2-review1-fmt.log` / `target/mcp-stage2-2-review1-fmt-check.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` (最終) | root | 0 | 56.96s、`target/mcp-stage2-2-review1-core-check-final.log` |
| 新規focused (typed record / toast / ZIP / 分類) | root | 0 | 10/10、実行0.684s。`target/mcp-stage2-2-review1-new-focused.log` |
| 上記 `$stage22ReviewFocused` (最終) | root | 100 | 1,526件中1,524成功/2失敗。compile 2m23s、実行99.985s。`target/mcp-stage2-2-review1-focused-final.log` |
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` | root | 101 | IPC 15/15、MCP 63成功/2失敗。`target/mcp-stage2-2-review1-crate-tests.log` |
| `cargo test --test ui_snapshot` (UPDATE指定なし) | root | 0 | 222/222、実行36.46s。`target/mcp-stage2-2-review1-ui-snapshot.log` |
| `python scripts/check_ui_glyphs.py` | root | 0 | 危険なUI glyphなし、`target/mcp-stage2-2-review1-glyphs.log` |
| `git diff --check` | root | 0 | LF/CRLF warningのみ、`target/mcp-stage2-2-review1-diff-check.log` |

新規focusedと最初の全focusedの後、CP932の追加自己点検/修正を行い、最終全focusedで全6ケースの実writer回帰と
raw境界の追加回帰まで再検証した。最初の全focusedも1,525件中1,523成功/同じOS5の2失敗、exit 100だった
(`target/mcp-stage2-2-review1-focused.log`)。設定の482/153/329、通常user書込、利用者の保留Undo、Openと既存同期rowの回帰も成功した。

失敗は§23.4と同じrestricted-tokenのpipe open OS error 5のみ:

- core: `agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry`
- core: `agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers`
- MCP: `pipe::tests::real_pipe_cancel_reaches_core_while_stdio_stdout_is_blocked`
- MCP: `pipe::tests::real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld`

DACLを緩めず、skip/ignoreや再試行は追加していない。PNGは**追加0・変更0**。
UPDATE_SNAPSHOTSを外して全222枚を比較し、`git diff --name-only -- tests/snapshots` に差分なし。
typed状態の表示/コピーはheadless UI回帰、既存画面はPNG比較で検査した。

sandbox外でClaudeCodeが実行する最小の追加コマンド:

```powershell
cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored
cargo nextest run -p mimageviewer --lib -E 'test(=agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry) or test(=agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers)' --no-fail-fast
```

全focusedを外で確認する場合は、§23.4の定義と上記の `$stage22ReviewFocused` を使う。
実core/AIアプリでの操作確認、追加の確認用build-devは未実施 (real pipeのgreen前提がsandbox内では揃わないため)。
必要な確認用core/MCPは外のpipe gate後に `.\scripts\build-dev.ps1 -PreserveRuntime` で同時更新する。
test-lib-full/test-fullの再実行・commit・製品起動・実client configの操作はしていない。
今回の指摘に残る未完項目/追加の設計矛盾は検出していない。正式な再レビューは別threadで行う。

### 23.6 実装レビュー 2 巡目の対応

観測者は、明記した外部結果を除き実装担当 root。正式な独立レビューの代用ではない。
対象 HEAD は `0a313783e`。commit・製品起動・実client configの操作はしていない。

#### 利用者タグ完了トーストのバッチ所有

指摘を確認した。`poll_tag_write_results` が採用件数を次のpollまで保持する一方、操作入口が
`tag_toast_label` を最後のタグで上書きしていたため、#Aの結果採用後に#Bを投入すると件数と名前が違うバッチを表した。

- ラベルを `TagWriteCompletion` に統合し、件数・errorと同じ完了時点で `take` する。
  `TagCompletionLabel` は未設定 / 特定タグと付与・削除種別 / Clear / Restore / 総称を表す。
  同じ正規化タグかつ同じ操作の間だけ最初の表示名を保ち、混在後は完了まで総称に固定する。
  agent-originの投入は利用者ラベルを変更しない。
- toggle / add / remove / clear / Undo / Redo の全入口を同じ所有者に寄せた。
  単独操作、Clear、`restored` の既存 formatter と文言・優先順位は変更しない。
  総称の全件no-opがClear扱いにならないよう、総称は formatter に明示的な「タグ」を渡す。
- 既存の並列入力を禁止する案は採用しない。通常操作を制限せず、すでに保持するバッチの件数とラベルを
  一つの状態にまとめるだけで不整合を除去できるため。

回帰は実操作入口と制御可能なwriter/pollを使用する。#A→#B、#A→#A、同じ正規化キーの表記違い、
同じタグの付与→削除、toggleの方向変更、Clear、Undo/Redoと新規編集の混在、完了後の新バッチを検査する。
単独の既存文言、既存のpollを跨ぐagent混在・send/done間の回帰も維持する。
新規テストは `user_completion_label_matches_all_pending_operations`、
`toggle_completion_label_stays_generic_after_direction_changes`、
`clear_completion_label_cannot_be_replaced_by_a_pending_edit`、
`undo_redo_completion_label_cannot_be_replaced_by_a_pending_edit`、
`pending_edit_then_undo_uses_generic_label_and_next_batch_is_specific`。

変更ファイルは `src/{app.rs,tag_ops.rs,undo_ops.rs}` と本節。PNGは追加0・変更0。
page-turn、detached述語/viewport routing、tag writerのsend/done順、Undoの登録境界は変更していない。

#### page-turn の原因切り分け

ClaudeCodeの外部観測: `0a313783e` の全libで指定5件が失敗し、focused再実行ではretained-cover以外の4件が失敗。
`c39489f5d` の全libでは同じテストが成功していた。外部の失敗ログは
`target/test-lib-full/runs/20261101T042518766Z-ff26d6d5/`、先行成功ログは
`target/test-lib-full/runs/20261101T033838229Z-0a544e00/` にある。

修正前のclean HEADで、指示された5件のnextestは **exit 0、5/5成功 (3.720s)**。
`target/mcp-stage2-2-review2-page-baseline.log`。この時点のlibtest実行体を保存して以後の比較に使用した。
fixtureは一時data-dirのAppを直接描画し、`App::update_frame` やタグ完了pollを実行しない。
`c39489f5d..0a313783e` のApp差分は完了状態の追加・初期化とbook-key helperの可視性だけであり、
page-turnのsource差分はない。これらのfixtureにはタグ操作・完了pollによるrepaint/共有static変更もない。

外部に依存する既存経路は次の2つだった。

1. `FsLoupeHold` の既定Shiftは `page_turn_loupe_hold` → `modifier_held_via_os` →
   `key_input::physical_key_down` で実際の `GetAsyncKeyState` を読む。
   `lock_test_input` とframeのegui modifiers初期化だけではOS側を固定しない。
   capture中のShiftはFlat成立を阻害し、capture後・描画直前のShiftはcurlを不成立にする。
2. draw-submitは指定進行率から `Instant::now() - duration * progress` を作り、後の描画で再び実時刻を読む。
   名目0.75でも、その間の実時間だけ進行する。描画直前の遅延で黒い葉がold9を完全に覆う。
   一方、初回Flat captureにはturn時計自体がないため、cover系の失敗を時間経過では説明しない。

差分の作用を除いた対照として、変更対象17ファイルの元バイトとSHA256をtarget内へ退避し、
`git show c39489f5d:<path>` のソースでlibtestを **compileのみ** した (exit 0、5m28s)。
実行体を別名保存後、finallyで全17ファイルを復元し、退避時SHA256との一致を確認した。
Git HEADやgit dirは変更していない。これは当該commitのRustソースの対照であり、古いbuild metadataや実行環境の再現ではない。
ログ: `target/mcp-review2-control-build.log`。保存実行体:
`target/debug/deps/mcp-review2-{head0a,c394}-lib-tests.exe`。

同じheadless libtestだけをWindows debuggerの子として実行し、当該子のAPI戻り値または1回の停止時間を制御した。
OSへのキー送信・デスクトップ操作・製品起動・source/テストの変更はしていない。

| 条件 | `0a313783e` | `c39489f5d` のソース対照 |
|---|---|---|
| 通常の直接実行、5件、Shift押下sampleは全て0 | 全件exit 0 | 全件exit 0 |
| Shift読取を常時押下に固定 | cover初回anchor1/tail10/retained19で同じFlat失敗、各exit 101 | 同じ3箇所、各exit 101 |
| 最初は未押下、navigateの描画直前だけ押下に変更 | whiteの `cover_tests.rs:379`、false/true、exit 101 | 同じ行・値、exit 101 |
| Shift読取を未押下に固定したdebugger実行 | cover4件はexit 0。drawはold9・名目0.75でexit 101 | 5件ともexit 0 |
| 上記未押下に加え、drawの名目0.75の描画前1回だけ25ms停止 | 未追加 (直前行で既に再現) | `draw_submit_tests.rs:462`、old9・名目0.75でexit 101 |

常時Shiftではwhite/drawはより手前のFlat検査で止まるため、それを外部と同じ失敗とは数えていない。
whiteは描画直前への切替、drawは実時間遅延でそれぞれ該当の検査まで確認した。
各libtestの失敗exitは101。正常対照の成功exitは0であり、debugger wrapperの成功をテスト成功と扱っていない。
ログ/集計: `target/mcp-review2-{sandbox-page,c394-normal}.json`、
`target/mcp-review2-{head0a,c394}-{force-all,clear-all}.json`、
`target/mcp-review2-{head0a,c394}-transition-shift.log`、`target/mcp-review2-c394-delayed-frame.log`。

**判明したこと:** 受理済み修正がない旧ソースでも、OSの修飾キー読取とフレーム内の実時間という条件だけで
各失敗と同じ検査を再現できる。タグ完了の差分を必須条件とする回帰ではない。
そのためpage-turnの製品コード・テストは変更しない。
**未確定:** ClaudeCodeの元の外部失敗時のキー状態・フレーム内経過時間はログに無い。
当時もこの2条件だったとの断定はしない。再現条件の証明と元の観測の原因同定を区別する。

外部で残る入力状態を確認する場合の読み取り専用probe (実行するのは保存済みlibtestのみ):

```powershell
python -I .\target\mcp-review2-page-probe.py --exe .\target\debug\deps\mcp-review2-head0a-lib-tests.exe --output-prefix .\target\mcp-review2-outside-page
```

5件を順に実行し、各exitとShiftの読取sampleをJSONへ保存する。入力送信はしない。
失敗が再現しなくても、元の失敗時の状態が分かったとは扱わない。

#### 最終検証

要求された§23.5の `$stage22ReviewFocused` は変更せず使用する (新しいタグ回帰も `/tag_ops/` に含まれる)。
最初のtag/undo focusedは新規Undo回帰のfixtureでE0502 (mutable receiverと `app.tmp` の同時借用) により
compile exit 101。fixtureのpathを呼出前に作るよう修正した。製品コードの追加変更はない。

| コマンド | 観測者 | 実 exit | 結果 / ログ |
|---|---|---:|---|
| `cargo fmt` / `cargo fmt --check` (最終) | root | 0 / 0 | workspace全体。`target/mcp-stage2-2-review2-fmt.log` / `target/mcp-stage2-2-review2-fmt-check.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | root | 0 | 1m03s、`target/mcp-stage2-2-review2-core-check.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/tag_ops/) or test(/undo_ops/)' --no-fail-fast` (最終) | root | 0 | 28/28、2.289s。`target/mcp-stage2-2-review2-tag-undo-final.log` |
| brief指定のpage-turn 5件 (修正後) | root | 0 | 5/5、3.353s。compile 5m47s。`target/mcp-stage2-2-review2-page-final-five.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/ui_fullscreen::page_turn/)' --no-fail-fast` | root | 0 | module本体と関連submission fixtureを含め180/180、42.134s。`target/mcp-stage2-2-review2-page-module.log` |
| §23.5 `$stage22ReviewFocused` | root | 100 | 1,531件中1,529成功/2失敗、103.420s。`target/mcp-stage2-2-review2-focused.log` |
| `python scripts/check_ui_glyphs.py` | root | 0 | 危険なUI glyphなし、`target/mcp-stage2-2-review2-glyphs.log` |
| `git diff --check` | root | 0 | `target/mcp-stage2-2-review2-diff-check.log`。source対照からの復元17ファイルのhashも一致 |

全focusedの失敗は、§23.5と同じrestricted-tokenによるOS error 5の実pipe 2件のみ。
新規5件、通常の利用者タグ/Undo、agent混在、設定の全field分類は成功した。
失敗は `agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry` と
`agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers`。DACL緩和・skip/ignore・retry追加はしていない。

sandbox外の実pipe確認とpage-turn全体のコマンド:

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(=agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry) or test(=agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers)' --no-fail-fast
cargo nextest run -p mimageviewer --lib -E 'test(/ui_fullscreen::page_turn/)' --no-fail-fast
```

全focusedを外で確認する場合は、§23.4の `$stage22Focused` 定義と§23.5の `$stage22ReviewFocused` をそのまま使う。
実core/AIアプリの起動確認・確認用build-devは未実施 (sandbox内でreal-pipeのgreen前提が揃わないため)。
test-lib-full/test-fullの再実行はしていない。page-turnの元の外部失敗時の状態についてのみ、上記の未確定点が残る。

### 23.7 実装レビュー 3 巡目の対応

実装担当 root のコード照合・非対話テスト記録。対象 HEAD は `c28da6a9b`。
正式な独立レビューは別threadで行う。commit・製品起動・実client configの操作はしていない。

#### 指摘 → 修正 → 回帰

P2をコードで確認した。完了ownerの `TagCompletionLabel::Generic` を最後に `Option<&str>` へ変換すると、
formatterがClear/Restoreとの区別を失う。`cleared > 0` による分岐は別操作のno-opまでクリア件数へ足し、
`restored > 0` による分岐は混在バッチの他の結果を隠していた。

- `src/tag_ops.rs` の formatter へラベル型をそのまま渡し、`Specific` (付与/削除種別を含む) / Clear /
  Restore / Genericで文言を決める。型の情報を落とす `display_tag` helperは削除した。
- 単独種別の文言は維持する。Clearだけの場合は元から空だった項目も従来どおりクリア件数へ含める。
  Specificの全件no-op、Restoreだけの場合も従来の文言を保つ。
- Genericは「タグを変更しました (付与 N 件 / 削除 N 件 / クリア N 件 / 元に戻す N 件 / 変更なし N 件)」を使い、
  この順で非ゼロの部分だけを出す。異なるカテゴリを合算しない。未設定のラベルで結果が届いた場合も同じ内訳とする。
  §23.6で維持していた旧formatterの優先順位は、混在バッチについて本節の明示的な内訳へ置き換える。
- エラー時のwarning分岐、バッチの終了/破棄、agent/userの件数分離、Undo登録、writerのsend/done順は変更しない。
  入力の制限や新たな保留状態は足さず、既存の型による最終表示の選択だけで直す。

実操作入口 → 制御可能なwriter → pollの既存回帰を拡張した。
Clear Xの結果を採用し、完了前に#AがあるYへ#Aを追加したno-opが届くケースは
「クリア 1 件 / 変更なし 1 件」となり、Yのcacheは#Aのまま、Xのcacheは空のままを検査する。
Clear＋実追加、Clear＋追加の両方no-op、Undo/Redo＋新規編集、編集＋Undo、異なるタグ/付与削除の混在も
新しい内訳を検査する。単独Clear/Undo/Redoと次の単独バッチは既存文言のまま検査する。

formatter単体の引数も型へ移行し、付与、削除、Clear、空項目のClear、Restore、Specificのno-opの文言を固定。
追加4件は `toast_clear_already_empty_keeps_clear_wording`、`toast_mixed_clear_keeps_noop_and_add_separate`、
`toast_mixed_results_keep_every_nonzero_category_in_order`、`toast_mixed_all_noop_does_not_say_clear`。
全5カテゴリの固定順とゼロ件の省略を含む。既存のwriter失敗warning回帰も指定focusedで検査する。

変更ファイルは `src/tag_ops.rs` と本節のみ。DB/IPC/Remote/agentの書込結果・page-turn・detached述語/viewport routingの変更はない。
PNGは追加0・変更0。実アプリでのトースト表示は未確認。

#### コマンドと結果

| コマンド | 観測者 | 実 exit | 結果 / ログ |
|---|---|---:|---|
| `cargo fmt` / `cargo fmt --check` | root | 0 / 0 | workspace全体。`target/mcp-stage2-2-review3-fmt.log` / `target/mcp-stage2-2-review3-fmt-check.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | root | 0 | 20.25s、`target/mcp-stage2-2-review3-core-check.log` |
| 下記の指定focused | root | 0 | 43/43、5.101s、compile 2m23s。`target/mcp-stage2-2-review3-focused.log` |
| `python scripts/check_ui_glyphs.py` | root | 0 | 危険なUI glyphなし、`target/mcp-stage2-2-review3-glyphs.log` |
| `git diff --check` | root | 0 | `target/mcp-stage2-2-review3-diff-check.log` |
| `.\scripts\build-dev.ps1 -PreserveRuntime` | root | 0 | core 4m09s、Remote/EPUB worker/MCPも更新。PE/VCRT `runtime=4 pe=4` 成功。`target/mcp-stage2-2-review3-build-dev.log` / `target/vcrt-pe-reports/dev-runtime.json` |

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/tag_ops/) or test(/undo_ops/) or test(/tag_write_worker/) or test(/agent_metadata_write/)' --no-fail-fast
```

今回はこのfilterに実pipe試験を含まず、OS error 5による失敗もない。
検証の範囲は本節のテストであり、全lib / snapshot比較 / 製品の実行を再実施した結果とは扱わない。
AGENTS.mdの確認用ビルド規則に従い、全指定チェック成功後に
`.\scripts\build-dev.ps1 -PreserveRuntime` で通常featureの成果物だけを更新した。起動していない。

利用者が表示を確かめる場合の起動コマンド:

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

これは通常の `%APPDATA%\mimageviewer` を使用し、起動だけでも実設定/データを更新し得る。
single-instance mutexを共有するため、先にインストール版/常駐tray版を終了する。
確認対象は、単独操作の文言が従来どおりで、保留中にClear＋既存タグのno-op追加やUndo/Redo＋編集が混在すると
カテゴリ別の内訳になること。決定的な順序は上記headless回帰で検査済みであり、実アプリの表示は未確認。

### 23.8 検収 (ClaudeCode、2026-11-01)

- sandbox の外で ClaudeCode が実行:
  - `c39489f5d`: crates `--include-ignored` exit 0 (15/15、65/65)。`test-lib-full.ps1` は 15,276 件中 15,275 件成功。
    落ちた 1 件は `settings_transfer::tests::all_settings_fields_are_classified` で、段階 0 から件数の期待値が古かった
    (`0a313783e` で 482 / 329 に修正)。
  - `0a313783e`: crates exit 0。`test-lib-full.ps1` はページめくりのテスト 5 件だけ失敗 (流し直しでは 4 件)。
    実装担当が修正前のソースでも、OS の Shift の状態と描画の遅れで同じ失敗を再現した (§23.6)。当時のメイン PC では
    別の重い処理が 2 本並行していた (設計担当の連絡)。本 slice の回帰ではないと判断し、レビューもその判断を妥当とした。
  - `c28da6a9b`: 実 pipe 2 件・ページめくり全体・タグ/Undo・agent・設定の分類の focused nextest exit 0 (349/349)。
  - `3eba8f75b`: タグ/Undo/tag writer/agent の focused nextest exit 0 (178/178)。
- 独立した実装レビュー: 1 巡目 (`c39489f5d`) P2 3 件 (タグの記録が「処理中」のまま、利用者のタグ完了 toast の欠落、
  ZIP の中のフォルダ・入れ子 ZIP の本に★を付けられない) → 設計 `956b46a25`、実装 `0a313783e`。2 巡目 P2 1 件 (連続した
  利用者の編集で件数と最後のタグ名が食い違う) → `c28da6a9b`。3 巡目 P2 1 件 (クリアと別操作の変更なしの合算) →
  `3eba8f75b`。4 巡目で追加の指摘なし、判定は「2-2 impl: accepted」。
- 2 巡目以降の指摘は、利用者のタグ完了 toast を完了まで溜める修正 (1 巡目) に伴う文言の細部。単独の Clear で元から空の項目も
  クリア件数に含める表現は master と同じ既存の挙動として残した。
- 実 AI アプリ・実 core での書き込み・画面の追従・XMP の外部確認は未実施 (サブ PC での実機確認で行う)。

## 24. サブ PC での実アプリ確認 (2026-11-01)

### 24.1 条件と観測者

- 観測者: サブ PC 担当 (Remote Control の Claude Code セッション「mImageViewer 実機確認」)。Claude Desktop の Chat の画面は
  担当から撮れないため、許可の窓と AI の返事は利用者の報告と貼り付けによる。mIV 側は担当がスクリーンショット・
  `mimageviewer.log`・設定ファイルで確かめた。記録はメイン PC の `C:\home\mimageviewer_subpc\mcp-stage2\report\`。
- ビルド: `3b94fc953` (段階 2-2 の検収後) のポータブル版 (`prepare-portable-smoke.ps1`)。`--data-dir C:\miv-mcp-test\data`。
- AI アプリ: Claude Desktop (MSIX 版。Windows 11 Pro 26200)。登録は mIV の「AI 連携」から、Claude Desktop を終了した状態で
  利用者が行った (動作中は mIV が拒否した。設計どおり)。書き換え先は MSIX 版の `LocalCache\Roaming\Claude\claude_desktop_config.json`。
  `.mimageviewer.bak` は書き換え前と一致。最後に「登録を外す」で mcpServers は書き換え前と同じに戻った。

### 24.2 結果

依頼の 9 段 (状態・一覧と縮小画像・mIV で開く・タグを付ける・外す・★ (ZIP の中のフォルダを含む)・コレクションの作成/名前の変更/
追加/外す・AI による変更・読み取りのみへ下げた後) すべてで、AI の返事と mIV 側の結果が一致した (サブ PC 担当)。

- 開く: mIV が前面に出て、指定の画像を全画面で表示した (利用者・ログ)。ログには activate の直後に `SetForegroundWindow failed`
  もある。開いて約 3.5 秒後に全画面を閉じた記録があり、利用者が閉じたかは未回答。
- タグ・★・コレクションの変更は、すぐ mIV の画面に出た。「AI による変更」の 8 件は、件数・対象・前後の値まで実際の変更と一致し、
  行の展開とコピーも動いた。
- 読み取りのみへ下げた後の★の依頼は「タグ・★を変更する許可がありません」で断られ、mIV は前面に出ず、★も付かなかった。
- 許可の窓: tool を使うたびに Claude Desktop が出した (利用者。「かなり煩雑」)。窓の文言とコネクタ設定の画面は撮れていない。

### 24.3 見つかった問題 (対応は §24.4)

1. タグ・★の変更でも mIV が前面に出る (利用者・ログ)。コレクションでは出ない。原因はコードで確認: agent の UI 要求の受付
   (`src/agent_ipc/open.rs`) が、要求の種類を問わず `single_instance::signal_activate_existing()` を呼ぶ。前面に出すのは `Open` だけ。
2. `get_item_info` が ZIP の中のフォルダ (`list_folder` が返す `chapter2/` 等) を `bad_request` で断る。`set_rating` は同じ指定を
   受け付ける。原因はコードで確認: `src/agent_ipc/item_info.rs` が ZIP の subresource を画像の拡張子に限っている。
3. 接続中の adapter (Chat と Code の 2 本) が約 5 秒ごとに `hello` と `miv_status` を送り続け、agent の lane のログが埋まる
   (セッション中に 945 行)。
4. 「AI による変更」のタグの行だけ、説明文が無く「変更 N 件 / 変更なし…」の書式で、他の行とそろっていない。
5. 参考: 「最近使った AI アプリ」の名前は `local-agent-mode-mimageviewer` (Claude Desktop が名乗った名前をそのまま出している)。
   Claude Desktop の Code タブにも mimageviewer の tool が出る (同じ設定ファイルを読むためと推測、未確認)。サブ PC 担当が
   Claude Desktop の中で動いていたため、Claude Desktop の終了で担当が起動した mIV も終了した (確認の手順の制約。mIV の問題ではない)。

### 24.4 対応方針

- 1〜4 は直す。1 は「開く」だけが前面化の合図を送り、タグ・★は前面化せずに UI thread を起こす (窓が最小化・トレイの時に
  UI thread が回るかを確かめ、回らないなら設計に戻す)。2 は `get_item_info` が `list_folder` の返すすべての項目を受け付ける。
  3 は定期の問い合わせを agent の lane のログに残さない (状態が変わった時だけ残す)。4 は記録の書式をそろえる。
- 許可の窓は Claude Desktop の機能で、mIV からは止められない。Claude Desktop のコネクタの設定で tool ごとに「常に許可」等を
  選べるという外部の案内がある (Anthropic の公式の説明は未確認。利用者の画面で確かめる)。公開文書 (マニュアル) で案内する。

## 25. 「AI 連携」を環境設定の外の窓へ移す (2026-11-01)

### 25.1 利用者の判断 (U13)

- サブ PC での確認の後、利用者から「環境設定は OK を押すまで反映されないページなので、リモート接続のように環境設定の外側の
  メニューにするほうが自然か」という問いがあり、設計担当の推奨どおり「移して直す」と決めた (2026-11-01)。
- 理由: 環境設定のページに、OK で反映される項目 (許可の範囲・縮小画像・撮影位置) と、押した時点で実行される操作 (登録・登録を
  外す・再確認) が混ざっていた (「環境設定の OK / キャンセルとは無関係です」の但し書きが要った)。「最近使った AI アプリ」と
  「AI による変更」は設定ではなく、動いている間の状態と記録。「リモート接続…」が同じ形で環境設定の外にある。

### 25.2 形

- 「設定」メニューの「リモート接続…」の隣に「AI 連携…」を置き、独立した窓 (非 modal、「リモート接続…」の窓と同じ作り・同じ配色と
  位置の helper) を開く。メニューの項目は「リモート接続…」と同じく `MenuCommandId` に足し、表示/非表示の設定・ラベル・keymap の
  扱いも「リモート接続…」に揃える。
- 窓の中身は今の「AI 連携」のページと同じ節 (許可の範囲、縮小画像・撮影位置、見せる範囲の表、注意の文、最近使った AI アプリ、
  AI による変更、各 AI アプリの登録)。
- **許可の範囲・縮小画像・撮影位置は、変えた時点で反映する** (OK / キャンセルは無い)。反映は環境設定の OK が使っていたのと同じ
  許可の確定の経路 (snapshot と epoch) を通す。下げる操作がすぐ効くので安全側にも寄る。上げる操作も同じくすぐ反映する (窓を開いて
  選ぶこと自体が利用者の意思で、確認の窓は足さない)。確定に失敗した時は既存の扱い (まれな失敗の割り切り: ログ + 通知、表示は
  保存済みの値に戻す)。
- 環境設定から「AI 連携」のページを外す。環境設定の検索の索引は「リモート接続…」と同じ扱いにする (環境設定の外の窓を索引に
  載せていないなら外す)。設定の書き出し・取り込みは変えない (AI 連携の設定は今も対象外)。
- 「環境設定の OK / キャンセルとは無関係です」の但し書きと、環境設定の下書きと保存済みの値が違う間は登録のボタンを無効にする
  処理 (1d) は不要になるので外す。

### 25.3 状態を減らす検討

| 検討 | 結論 |
|---|---|
| 環境設定に残して即時反映の項目だけ別扱い | しない。同じページに 2 種類の反映が残る |
| 窓にも OK / キャンセルを付ける | 付けない。下書きと保存済みの 2 つの状態を持たない (1 つの所有者が即時に確定する) |
| 上げる時だけ確認の窓 | 出さない。窓を開いて選ぶことで足りる |

AI 連携の設定と窓は未リリースなので、移行は要らない。

### 25.4 一緒に直す点 (§24.3 の 1〜4)

1. agent の UI 要求の受付で、前面化の合図 (`single_instance::signal_activate_existing`) は `Open` の時だけ送る。タグ・★は前面化
   せずに UI thread を起こす。窓が最小化・トレイに入っている時にも UI thread が要求を処理できるかをコードで確かめ、できないなら
   設計担当に返す (前面化で代用しない)。
2. `get_item_info` は `list_folder` が返すすべての項目を受け付ける (ZIP の中のフォルダ・入れ子 ZIP の本を含む。★の key は
   `set_rating` と同じ本の key)。
3. 接続中の adapter の定期の `hello` / `miv_status` を agent の lane のログに残さない。状態が変わった時 (接続・切断・許可の変化等) だけ残す。
4. 「AI による変更」のタグの行の書式を、他の tool の行と揃える。

## 26. 段階 2-3 の実装記録

対象 HEAD は `689f765fb`。以下のコード照合・headless検査の観測者は実装担当 root。
UI移設・ZIP情報・受付/ログ/記録はファイルを分けて実装し、rootが統合検証を担当した。
正式な独立レビューは別threadで行う。§24の実アプリ観測はサブPC担当/利用者によるもので、今回の検査とは分ける。
commit・製品起動・実Claude/Codex設定の読み書きはしていない。

### 26.1 前提の照合

- `agent_ipc/open.rs` は種類を問わず前面化イベントを送っていた。タグ/★も同じUI要求ownerを通るが、
  admission/返信は `App::poll_agent_open_requests` が種類別に処理している。
- 最小化/トレイでも要求のpollへ進めることをコードで照合した。
  `ctx.request_repaint` → vendored eframeのRequestRepaint → schedulerの要求済みrepaint (hidden時100ms throttle) →
  `ScheduledRepaint` のFull passとなる。Windows最小化時は `frame_output::PassDecision` がUIを実行しpaintだけ抑止する。
  トレイは `tray_integration::hide_to_tray` がWin32 SW_HIDEを使い、eguiのVisible(false)を送らない。
  winit 0.30.13はWindowsのOccludedイベントを実装していないため、トレイでもviewportのvisible判定は既存の既定経路を通る。
  `update_frame` → `render_initial_open` → agent pollは表示状態による後段のreturnより前。
  logic-onlyにはagent pollが無いが、ここでの要求済みFullをlogic-onlyに置き換える経路はない。
  eframe/winit/トレイ/viewport routingの変更、前面化による代用は不要だった。実アプリでの最小化/トレイ確認は未実施。
- Remote窓はApp所有のOptionを取り出し、通常テーマのegui::Windowを描いて戻す非modal構造。
  AI窓もこの所有を使い、位置はコーディング規約のdefault_pos、配色は既存rootテーマとui.visuals()に従う。
- 従来のPreferences OKはSettingsの保存から `settings_db::save_full` へ進み、成功したtransaction内で
  permission snapshot/epochを確定していた。新しいDB接続を作らず `Settings::save_checked` の同じ境界を使う。
  この保存は既存のfavorite overlay除去も通す。Preferences検索にRemote窓は無く、AIページ索引も除去する。
- ZIPフォルダのidentity-only readerと `set_rating` のbook key helperは既存にあった。
  `get_item_info` だけが画像拡張子を必須にし、ZipDirのratingをcontainerへ落としていた。
- adapterは問い合わせごとに新しいpipeとHelloを使う。物理connect/disconnectの毎回記録は周期ログ抑止にならない。
- タグ記録はtyped job ownerで完了するが、そのsummaryだけがcountsの列挙になっていた。

### 26.2 実装と選択

- `SettingsAgentIntegration` をMenuCommandId/ALL/catalog/stable名/label表へ追加。
  既定ではRemoteの直後。既存の表示/非表示・並べ替えの仕組みを使い、実メニューのheadlessクリックも検査する。
  `KeyAction` は足さないので生成参照 `keymap.ini.default` は変更しない。`keymap-spec.md` に両窓のmenu IDを記載した。
- Appの `Option<AgentIntegrationState>` が独立した「AI 連携」窓を所有する。Preferencesのページ/tree/search/draft専用stateを除去。
  窓本体は非modalで、登録の確認/書込の間だけ従来のcommon-modal判定と単一operation ownerを使う。
  読み書きの検出・登録は従来どおりworker、単にmenuから窓のstateを開くだけでは検出/ファイル読み込みを始めない。
- 許可/縮小画像/GPSは3値の表示用selectionから即時保存する。保存成功後だけlive Settingsへ採用し、失敗はlog+warning、
  次の描画は保存済み値に戻る。UIの毎frameで全Settingsをcloneしない。
  先に開いたPreferencesのOKでも、新しいliveの3値を移送してから保存する。Cancelも新しい許可を巻き戻さない。
  書き出し/取り込みからの除外は変更しない。旧OK/キャンセル注意とdraft差分による登録無効化は除去した。
- 前面化はOpenだけ。タグ/★は同じCAS/queueとrepaintを使う。テストでは前面化callbackを注入し、実アプリを起こさない。
- ZIPの `chapter2/`、`inner.zip/`、`inner.zip/chapter3/` 等はidentity indexで存在を照合する。
  通常のZIPフォルダはItemKind::Folder、入れ子ZIPの本はArchive。同じbook keyでratingを読み、旧CP932 keyもread-onlyで補う。
  フォルダ自身のsize/modified/dimensions/EXIF/generationはNotApplicable、containerのstat/tagsは別欄のまま。
  画像や終端の入れ子ZIPのbytesは読まない。helperは可視性だけ広げ、keyの式は変えない。
- serverに固定サイズのHello/Status結果・permission snapshot/epochの観測stateを一つ置く。
  最初の観測と状態/エラーの変化・復旧だけを通常laneへ記録し、同じHello/Statusがpipeを開き直しても繰り返さない。
  optional perfも同じ出力判定を使い、不変の周期行を抑止する。他toolの要求ログ/perfとrecent-app attributionは維持する。
  listener開始/終了は記録する。常時接続を表す新しいsession ownerや疑似disconnect検出は足さない。
  ログへclient名/version、query、path、result/error本文は出さない。
- タグの受付summaryを「タグの変更を受け付けました (N 件)」、完了summaryを「タグを変更 (N 件)」にした。
  他toolと同様に説明と件数を示し、変更なし/対象外/処理中/失敗の内訳は既存のcounts欄で表示する。
  typed row完了・before/after・warning/Undo境界は維持する。

### 26.3 検証の記録

| コマンド | 観測者 | 実 exit | 結果 / ログ |
|---|---|---:|---|
| `cargo fmt` / `cargo fmt --check` (最終) | root | 0 / 0 | workspace全体。`target/mcp-stage2-3-fmt.log` / `target/mcp-stage2-3-fmt-check.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | root | 0 | 1m18s、`target/mcp-stage2-3-core-check.log`。以後のRust修正はtest専用部分だけ |
| 新規/移設focused (初回) | root | 100 | 94件中92成功/2失敗。compile 6m47s、実行8.050s。`target/mcp-stage2-3-narrow.log` |
| 同focused (修正後) | root | 0 | 94/94、compile 2m24s、実行6.650s。`target/mcp-stage2-3-narrow-final.log` |
| 下記 `$stage23Focused` | root | 100 | 2,042件中2,040成功/2失敗、138.007s。`target/mcp-stage2-3-focused.log` |
| `cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored` | root | 101 | IPC 15/15、MCP 63成功/2失敗。`target/mcp-stage2-3-crate-tests.log` |
| `UPDATE_SNAPSHOTS=1` を新10件だけに指定した生成 | root | 0 | 10/10、compile 3m38s、実行3.13s。`target/mcp-stage2-3-snapshot-generate.log` |
| `cargo test --test ui_snapshot` (更新指定なし) | root | 0 | 224/224、51.29s。`target/mcp-stage2-3-ui-snapshot.log` |
| `python scripts/check_ui_glyphs.py` (最終) | root | 0 | 危険なUI glyphなし、`target/mcp-stage2-3-glyphs.log` |
| `git diff --check` (最終) | root | 0 | `target/mcp-stage2-3-diff-check.log` |

初回focusedの2失敗は検査の前提を直した。保存抑止flagはwriteだけでなく `with_db_result` のreadも拒否するので、
失敗保存の後もflagが残ることを検査し、flag解除後に未変更のsnapshot/epochを比較する。
小画面検査は、長い手順の末尾へscrollした位置で、上方にある「設定を開く」も同時に見えると誤って仮定していた。
末尾のcopy到達を検査した後、実wheelでそのbuttonへ戻り、viewportとscroll clipの両方に収まることを検査する。
表示領域の拡大・製品layoutの変更・assertの削除・scroll offsetの直接変更はしていない。

新規/移設focusedのfilter:

```powershell
$stage23Narrow = 'test(/ui_dialogs::agent_integration/) or test(/immediate_agent_permission_tests/) or test(/agent_integration_menu/) or test(/agent_ipc::logging_tests/) or test(/agent_ipc::open::tests/) or test(/agent_ipc::item_info/) or test(/tag_write_worker/) or test(/tag_ops/)'
cargo nextest run -p mimageviewer --lib -E $stage23Narrow --no-fail-fast
```

全focusedは§23.4/§23.5の定義に追加したもの。移設先moduleは旧Preferencesのfilterには含まれないため明記した。
入力/Global menuのinventory、実menuクリック、Preferences OK/Cancel、Settings field全分類、既存のread途中の許可変更、
ZIPのkey互換、Openの取消/受付、workerの本当の完了、登録IME/小画面/最大row、loggingの繰返し抑止まで含む。

```powershell
$stage23Focused = "($stage22ReviewFocused) or test(/keymap/) or test(/ui_main/) or test(/ui_dialogs::agent_integration/) or test(/agent_metadata_write/) or test(/agent_open_tests/) or test(=settings_transfer::tests::all_settings_fields_are_classified)"
cargo nextest run -p mimageviewer --lib -E $stage23Focused --no-fail-fast
```

coreの2失敗は既知のrestricted-tokenの実pipe OS error 5のみ:

- `agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry`
- `agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers`

MCPの2失敗も、fixtureのpipe open時のOS error 5:

- `pipe::tests::real_pipe_cancel_reaches_core_while_stdio_stdout_is_blocked`
- `pipe::tests::real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld`

DACL緩和・skip/ignore・retryは追加していない。sandbox外での最小の再実行は以下。
全focusedを確認する場合は上記変数 (§23.4/§23.5の定義込み) を使う。

```powershell
cargo test -p mimageviewer-agent-ipc -p mimageviewer-mcp -- --include-ignored
cargo nextest run -p mimageviewer --lib -E 'test(=agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry) or test(=agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers)' --no-fail-fast
```

実pipe gateのgreen前提はsandbox内では揃わないため、確認用build-devは今回は未実施。
外のgateが通った後に `.\scripts\build-dev.ps1 -PreserveRuntime` でcoreとcompanionを一緒に更新する。
既存の `target/dev-runtime` は今回の検証用ビルドとは扱わない。
実アプリでの最小化/トレイ操作、独立窓の表示・登録、サブPCの9段の再確認は未実施。
test-lib-full/test-full、製品起動、commit、公開文書/配布物の変更はしていない。実装を止める新たな設計矛盾は見つからなかった。

### 26.4 PNGと変更ファイル

生成時だけ子processのUPDATE_SNAPSHOTSを設定し、`cargo test --test ui_snapshot agent_integration_window_` へ限定した。
生成後に更新指定を外して全224件を比較し、rootが新10枚を各1枚ずつ目視した。
タイトル/×、明暗の文字コントラスト、3列の折り返し、登録操作、空の記録と展開した記録を確認した。
最大row/長い文字列/1093×614/200%の到達性はPNGだけに頼らずheadlessの実scroll/クリックで検査した。

全て `tests/snapshots/` 配下。旧8枚を削除し、新10枚を追加。その他の既存PNGの変更は0。
新10枚は計2,114,461 bytes (約2.02 MiB)、削除8枚との差は368,192 bytes (約0.35 MiB)。

| 削除 | 追加 |
|---|---|
| `preferences_agent_disabled_dark.png` | `agent_integration_window_disabled_dark.png` |
| `preferences_agent_disabled_light.png` | `agent_integration_window_disabled_light.png` |
| `preferences_agent_read_dark.png` | `agent_integration_window_read_dark.png` |
| `preferences_agent_read_light.png` | `agent_integration_window_read_light.png` |
| `preferences_agent_open_dark.png` | `agent_integration_window_open_dark.png` |
| `preferences_agent_open_light.png` | `agent_integration_window_open_light.png` |
| `preferences_agent_organize_dark.png` | `agent_integration_window_organize_dark.png` |
| `preferences_agent_organize_light.png` | `agent_integration_window_organize_light.png` |
| — | `agent_integration_window_records_dark.png` |
| — | `agent_integration_window_records_light.png` |

コード/文書の主な差分:

- 新窓: `src/ui_dialogs/agent_integration.rs` と `agent_integration/tests.rs`。
  旧 `src/ui_dialogs/preferences/agent_integration.rs` と同 `agent_integration/tests.rs` から移設。
- Preferences整理: `src/ui_dialogs/preferences.rs`、`preferences/{pages.rs,search_index.rs}`。
- App/menu配線・snapshot export: `src/{app.rs,lib.rs,keymap.rs,ui_main.rs}`、`src/ui_dialogs/mod.rs`、`tests/ui_snapshot.rs`。
- ZIP情報/key共有: `src/agent_ipc/{item_info.rs,item_info_tests.rs,metadata_write.rs}` (最後はhelperの可視性だけ)。
- 受付/log: `src/agent_ipc/{mod.rs,open.rs,open_tests.rs,pipe.rs,tests.rs,logging_tests.rs}`。
- タグsummary: `src/{tag_ops.rs,tag_write_worker.rs}`。
- 内部文書: `docs/keymap-spec.md`、本節。`docs/keymap.ini.default` は生成されるKeyActionの集合が不変のため差分なし。

### 26.5 実装レビュー 1 巡目の対応

独立実装レビューの2指摘をClaudeCodeが採択し、rootがコードで両方を照合した。

- **P2 / 起床先の所有者**: `Context::request_repaint()` は、その時点の `viewport_id()` を使っていた。
  agent要求を処理するAppはROOTの所有者なので、`agent_ipc/open.rs` の送信後の起床を
  `request_repaint_of(ViewportId::ROOT)` に変更した。§26.1の最小化/トレイ時の照合では、
  別のchild pass中にworkerから呼ぶ場合の対象を明示できていなかった。
  同じ観点で `agent_ipc/changes.rs` のpublish (collection通知) とupdate (タグの記録完了) も修正した。
  AI連携窓の登録検出/書き込みworkerの完了通知、同じ経路のmock、UI側の遅延pollもROOTを明示する。
  所有者への呼び出し元の指定だけを直し、detachedの述語・viewport配送・vendorコードは変更していない。
  新しい状態、foreground操作、待ちやretryは追加していない。
- **P2 / 回帰テスト**: 実際のeguiのpassを、ROOTを親に持つ `minimized: Some(true)` のchildで開始する。
  pass中で `ctx.viewport_id() == child` を確認し、初期化由来のrepaintを取り除いた後、別threadから実際の
  dispatchを通してOpen/タグ追加/タグ削除/★の起床先とdelayを観測する (ROOT / ZERO)。
  ChangeControllerも別threadからpublish/updateを通して同じ条件を確認し、updateがtoastを増やさないことを維持した。
  ネイティブ窓や製品を起動するテストではない。
- **P3 / 説明欄**: `ui.columns()` が作るjustifiedなlayoutを、各列内の
  `Layout::top_down(Align::Min)` で置き換えた。説明のwrapとselectable指定を維持する。
  実際の `draw_exposure` を600ptで描くテストで、生成情報の説明のgalleyが左寄せ・非justifyで、
  2行以上に折り返すことを確認する。窓以外のsnapshotを更新する指定は使わない。

今回のfocusedは§26.3の新規/移設filterにChangeControllerを追加したもの:

```powershell
$stage23ReviewFocused = 'test(/ui_dialogs::agent_integration/) or test(/immediate_agent_permission_tests/) or test(/agent_integration_menu/) or test(/agent_ipc::logging_tests/) or test(/agent_ipc::open::tests/) or test(/agent_ipc::changes/) or test(/agent_ipc::item_info/) or test(/tag_write_worker/) or test(/tag_ops/)'
cargo nextest run -p mimageviewer --lib -E $stage23ReviewFocused --no-fail-fast
```

| コマンド | 観測者 | 実 exit | 結果 / ログ |
|---|---|---:|---|
| `cargo fmt` (workspace) | root | 0 | Rust全体を整形 |
| `cargo fmt --check` | root | 0 | `target/mcp-stage2-3-review1-fmt-check.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | root | 0 | 46.90s、`target/mcp-stage2-3-review1-core-check.log` |
| 上記 `$stage23ReviewFocused` のnextest | root | 0 | 99/99。compile 5m29s、実行16.275s、`target/mcp-stage2-3-review1-focused.log` |
| `python scripts/check_ui_glyphs.py` | root | 0 | 危険なUI glyphなし、`target/mcp-stage2-3-review1-glyphs.log` |

`UPDATE_SNAPSHOTS=1` は `cargo test --test ui_snapshot agent_integration_window_` にだけ設定し、
終了後に元の環境変数へ戻した。10/10、exit 0 (compile/lock待ちを含み7m16s、実行8.16s)。
ログは `target/mcp-stage2-3-review1-snapshot-generate.log`。
rootが変更10枚を全て目視し、生成情報/リモート接続の不自然な字間の解消、折り返し、明暗のコントラスト、
登録操作、空/展開した変更記録を確認した。変更PNGは以下のみ (全て `tests/snapshots/` 配下、追加/削除なし)。

| dark | light |
|---|---|
| `agent_integration_window_disabled_dark.png` | `agent_integration_window_disabled_light.png` |
| `agent_integration_window_read_dark.png` | `agent_integration_window_read_light.png` |
| `agent_integration_window_open_dark.png` | `agent_integration_window_open_light.png` |
| `agent_integration_window_organize_dark.png` | `agent_integration_window_organize_light.png` |
| `agent_integration_window_records_dark.png` | `agent_integration_window_records_light.png` |

| 追加検証コマンド (更新指定なし) | 観測者 | 実 exit | 結果 / ログ |
|---|---|---:|---|
| `cargo test --test ui_snapshot` | root | 2173 | 223件成功後、`susie_diagnostic_workers_exhausted` が60秒以上実行中と表示され、test processが2173で異常終了。`target/mcp-stage2-3-review1-ui-snapshot.log` |
| `cargo test --test ui_snapshot susie_diagnostic_workers_exhausted -- --exact` | root | 0 | 1/1、2.13s。`target/mcp-stage2-3-review1-susie-snapshot.log` |
| `cargo test --test ui_snapshot -- --test-threads=1` | root | 0 | 224/224、552.35s。`target/mcp-stage2-3-review1-ui-snapshot-serial.log` |

通常並列の全件比較が残る1件で進まなくなったため、同一ソース/PNGで単独、次いで1threadの全件比較を行った。
初回のtest processをこちらからkill/停止した事実はない。初回をassertion failureやOS error 5と同一視せず、
異常終了の原因は未特定として残す。Susieの当該fixtureは固定PoolStatusを描くもので、Susie/productを起動しない。
原因を推測して製品コードやテスト、対象外PNGを変えることはしていない。外側で通常並列の完走を確かめるコマンド:

```powershell
cargo test --test ui_snapshot
```

§26.3で未作成だった確認用runtimeも、今回の回帰99件と対象PNG10件の成功後に、残りの比較と並行して準備した。

| 最終確認コマンド | 観測者 | 実 exit | 結果 |
|---|---|---:|---|
| `powershell -NoProfile -File scripts\build-dev.ps1 -PreserveRuntime` | root | 0 | core 4m37s、remote/EPUB/MCPも成功。VCRT PE検査runtime=4 / pe=4、`target/vcrt-pe-reports/dev-runtime.json` |
| `git diff --check` (最終) | root | 0 | `target/mcp-stage2-3-review1-diff-check.log` |

変更はRust 5ファイル (実装3、テスト2)、本節、上記10 PNG。scope外のsource/PNGの変更なし。
確認用の `target/dev-runtime/mimageviewer-core.exe` とcompanion 3本を用意したが、製品を起動していない。
通常profileなので引数なしでは実利用中の `%APPDATA%\mimageviewer` を使い、設定/データを更新し得る。
利用者が起動する時はインストール版/トレイ常駐版を終了してから、以下を使う。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

実機確認の対象は、最小化/トレイ状態でのタグ/★要求の応答 (前面化なし) と、AI連携窓の説明の字間・折り返し。
これらの実窓での操作は未実施。今巡では実pipe経路を変更/再実行しておらず、§26.3の外側確認コマンドを維持する。
通常並列snapshotの異常終了以外に新たな未解決事項/設計矛盾は見つからなかった。commit、実設定やAIアプリのconfig操作なし。

### 26.x 検収 (ClaudeCode、2026-11-01)

- sandbox の外で ClaudeCode が実行:
  - `d659e09f3`: crates `--include-ignored` exit 0 (15/15、65/65)。`test-lib-full.ps1` は 3 件だけ失敗 (ページめくりの描画と提出、
    ページめくりの許可の処理、本の strip の押下)。3 件とも単独の再実行で通った (負荷で落ちる時間に依存するテスト。§23.6 と同じ種類)。
  - `1bf174a6e`: `cargo test --test ui_snapshot` exit 0 (224/224)、agent_ipc / agent_integration / keymap の focused nextest
    exit 0 (370/370)。
- 窓の PNG (明暗) を ClaudeCode が目視。1 巡目で見つけた説明の列の字間の広がりは、2 巡目の版で直っている。
- 独立した実装レビュー: 1 巡目 (`d659e09f3`) P2 1 件 (agent の要求の起こし先が子 viewport になり得る) と P3 1 件 (説明の列の
  両端揃え) → `1bf174a6e` で直した。2 巡目で追加の指摘なし、判定は「2-3 impl: accepted」。
- 実機の未確認: 窓の表示、最小化・トレイ中のタグ・★の要求、登録。公開文書の下書きの後、サブ PC で段階 2 全体をもう一度確かめる。

## 27. サブ PC での再確認 (依頼 3、2026-11-01)

### 27.1 条件

- 観測者: サブ PC 担当 (画面・mIV のログ・Codex の出力・プロセス一覧)。記録はメイン PC の
  `C:\home\mimageviewer_subpc\mcp-stage2-r3\report\`。
- ビルド: `1387e7c44` (段階 2-3 の検収後) のポータブル版。`--data-dir C:\miv-mcp-test3\data`。
- AI アプリ: Codex CLI (npm 版、利用者が導入・サインイン)。サブ PC 担当が `codex.cmd exec` で英語の依頼文を送った
  (exec の設定は approval: never / sandbox: read-only)。

### 27.2 結果

- 新しい窓: 「設定」メニューの「リモート接続…」の直下に「AI 連携…」。環境設定に AI 連携のページは無い。許可の範囲と縮小画像は
  OK なしで残る。字間は自然。
- Codex への登録: 書き換えは `[mcp_servers.mimageviewer]` の 4 行 (と空行) の追加だけ。`.mimageviewer.bak` は書き換え前と一致。
  「登録を外す」の後の `config.toml` は書き換え前とハッシュが一致。
- `default_tools_approval_mode = "approve"` の下で、tool の許可を求めて止まった段は無かった (§15.7 で未確認としていた点)。
- 9 段すべて期待どおり。最小化中のタグの追加・削除で mIV は最小化のまま、前面の窓も変わらず、変更は反映された。
  前面化の合図は「開く」の 1 回だけ。`get_item_info` は ZIP の中のフォルダ (`chapter2/`) を返した (kind folder・★4)。
  「AI による変更」のタグの行は他の行と同じ書式。読み取りのみへ変えた直後の★の依頼は断られ、記録も増えなかった。
- ログ: agent_ipc は全 22 行 (hello 1、miv_status 1)。permission_changed は接続中の変化だけ。panic なし。

### 27.3 見つかった点

1. **ChatGPT (Codex) のアプリを終了しても、Codex の常駐プロセス (`%USERPROFILE%\.codex\packages\app-server-daemon\...\codex.exe`、
   `app-server --managed-daemon` 等) が残り、mIV が「動作中」と判定して登録を拒否する**。利用者の許可でプロセスを止めて続けた。
   登録の後の `codex.cmd exec` 9 回では、この常駐プロセスは起動しなかった (npm 版の codex.exe が子として mcp.exe を起動し、
   終了で消えた)。追記: 利用者が ChatGPT アプリを起動し直しても、約 6 分のあいだ常駐プロセスは現れなかった (アプリの子の
   codex.exe は `AppData\Local\OpenAI\Codex\bin` の `app-server` と `exec-server` の 2 本)。朝の常駐プロセスの親 (PID 40416) は
   アプリ本体とは別で、何が起動したかは分からない (サブ PC 担当の観測と推測)。
2. 環境設定の検索で「AI 連携」と入れると「一致する設定がありません」とだけ出て、移った先の案内が無い。
3. 登録の確認の窓の内容に、ファイルには書かれない見出しだけの `[mcp_servers]` の行が出る (実際の差分と合わない)。
4. 参考: 表の中で語の途中で折り返す (「リモート接 / 続」)。egui の日本語の折り返しの既存の挙動で、この窓だけの問題ではない。
   確認の窓の後ろが薄く見えるのは modal の暗転で、問題ではない (スクリーンショット 09 で確認)。

## 28. 再確認で見つかった 3 点の直し方 (2026-11-01、利用者の判断「修正する」)

1. **Codex の動作中の判定から、Codex の常駐プロセスを外す** (§27.3 の 1)。
   - 外すのは、実行ファイルが Codex の設定フォルダ (`CODEX_HOME`、無ければ `%USERPROFILE%\.codex`) の
     `packages\app-server-daemon\` の下にある process だけ。それ以外の `codex.exe` (CLI・ChatGPT アプリの子の
     `AppData\Local\OpenAI\Codex\bin` 等) と `WindowsApps\OpenAI.Codex_*` の下の process は、今までどおり「動作中」とする。
   - 理由: この常駐プロセスはアプリを閉じても残り、何が起動したかも利用者には分からない (§27.3)。残っている間は登録できず、
     タスクマネージャーで止めるしかない。判定から外すと、常駐プロセスが同じ `config.toml` を同時に書いた時に片方の変更が
     失われ得るが、まれな失敗として割り切る (書き換え前の内容は `.mimageviewer.bak` に残る)。
   - 判定は実行ファイルの path だけで行う (他の process の引数は読まない)。登録の後の案内 (「<アプリ> を起動して…」) は変えない。
     常駐プロセスが新しい設定を読み直すかは確かめていない (CLI の `codex exec` は毎回読み直すことを §27.2 で確認)。
2. **環境設定の検索で、環境設定の外の窓へ案内する** (§27.3 の 2)。検索語が「AI 連携」や「リモート接続」の窓に当たる時、
   「この設定は『設定』メニューの『AI 連携…』にあります」のような行と、その窓を開くボタンを結果に出す。「リモート接続…」も
   同じ扱いにする (同じ形の窓なので、片方だけ案内しない)。案内の語は窓の見出しと主な項目 (許可の範囲・登録・PIN 等) から引く。
3. **登録の確認の窓に、実際に書く内容だけを出す** (§27.3 の 3)。今は書かれない見出しだけの `[mcp_servers]` の行が出る。
   出す内容は実際の書き換えの結果から作り、書き換えとずれないようにする (Claude Desktop の JSON も同じ規則)。

まれな失敗は割り切り (ログ + 表示)。

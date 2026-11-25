# ヘルプの「AI で調べる」設計 (§1.894)

- 対象: v5.8.0。ブランチ `ask-ai-894` (master `aaea8d0ed` から)。
- 担当: 設計 = ClaudeCode Opus 5.5 / high、実装 = Codex gpt-6.1-sol / high、独立レビュー = 別の gpt-6.1-sol / xhigh。
- 観測の出どころ: コードの記述はすべてコードの参照。各 AI サービスの URL の仕様は §3 の公式文書の参照
  (2026-11-25 に設計担当のサブエージェントが取得)。**各アプリ・サービスが実際にその URL で開くか・文が入るかは未確認**。
  確かめるのはサブ PC の手順 (§12)。

## 0. 要約

- ヘルプメニューに「AI で調べる」のサブメニューを 1 つ足す。
- MCP で mIV を登録済みの AI アプリがあれば、「Claude で調べる」「ChatGPT で調べる」でそのアプリの新しい会話を開き、
  入力欄に案内の文 (「mIV の MCP ツールでマニュアルを調べてから答えて。質問: 」) を入れる。**送信はしない**。
  利用者が続けて質問を書き、自分で送る。
- Web の AI (Google の AI モード・ChatGPT・Claude) は、質問を渡す URL の引数が公式に文書化されていないので使わない。
  案内の文をクリップボードに入れてから、各サービスの入口のページを開く。利用者が貼り付けて質問を書き足す。
- 「AI 連携の始め方」(マニュアル `tut-ai-integration.html`) へのリンクを置く。
- mIV 自身は通信しない。mIV が送る操作も無い (どの送り先も、送るのは利用者)。

## 1. 利用者の要望 (言葉、バックログ §1.894)

> AI関係のヘルプへの導線を強化したいです。ヘルプメニューの中に「AIで調べる」のようなリンクを追加し、
> そこでMCPセットアップ方法の案内、無料のAI（Google AIモード、ChatGPT、Claudeなど）への確認、などが
> できるようにしたいです。また、すでにMCPセットアップ済みの場合、このメニューの代わりに「Claudeで調べる」
> 「ChatGPTで調べる」のような項目を追加し、デスクトップアプリを開けるようにしたい

## 2. 状態を減らす検討 (CLAUDE.md「設計の簡素化」)

| 案 | 持つ状態 | 評価 |
| --- | --- | --- |
| A. 「AI で調べる…」の小窓で質問を書かせ、送り先のボタンで開く | 窓の間だけ: 質問の文・検出の受信口 | 当初の第一案。質問を mIV で書くのは、送り先が自動で送信する場合に中身の無い質問を送らないため、と、送る文を開く前に見せるためだった。§3 の確認で、文を入れられる送り先 (Claude Desktop・ChatGPT のアプリ) はどちらも**入力欄に入れるだけで送信しない**と公式にあり、Web の AI は文を渡す URL 自体を使わないと決まったので、両方の理由が無くなった。窓・IME の入力欄・Enter / Escape の扱い・小画面の検査が増えるだけになる |
| **B. ヘルプのサブメニュー (採用)** | App に、登録の検出の受信口 1 つ・最後の結果・最後に始めた時刻 | 利用者の言葉どおりのメニュー項目。送る前の確認は、開いた先の入力欄で利用者がする (mIV は送らない) ので、mIV 側の確認の窓は要らない |
| B'. B で、登録の有無を見ずに常に全項目を出す | なし | 未登録のアプリを開いても mIV のマニュアルは引けず、案内の文 (「MCP ツールで調べて」) が嘘になる。利用者は「設定済みの場合」と条件を付けている。採らない |

B の状態は「検出の結果の cache」になるので、古さの扱いを決める: サブメニューが開いている間、実行中でなく、
最後に始めてから 5 秒以上たっていれば検出を始め直す (§6)。外で登録を外した・付けた場合も、次にサブメニューを開けば合う。
メニューを開いている間に結果が届くと項目が増減するが、検出は小さなファイルを数個読むだけなので、
初回以外は開いた時点で前の結果が出ている。初回だけ「確認中…」の 1 行が出る。
項目の増減と押す操作が重なっても別の送り先を開かないように、各項目に表示順と無関係な固定の ID を付ける (§4)。

## 3. 各送り先の開き方 (公式文書の参照)

| 送り先 | 開く URL | 文の渡し方 | 公式文書 |
| --- | --- | --- | --- |
| Claude Desktop | `claude://claude.ai/new?q=<文>` | 入力欄に入る。送信しない (「prompt field is prefilled … so you can review and send it」)。`q` は約 14,000 字で切られる。アプリが起動していなければ OS が起動する | support.claude.com/en/articles/14729294 |
| ChatGPT のアプリ (Codex の画面) | `codex://new?prompt=<文>` | 入力欄に入る。送信しない (「It doesn't send the prompt automatically.」)。`prompt` は「Sets the initial composer text.」。長さの上限の記載なし | learn.chatgpt.com/docs/app/commands (同じ内容が developers.openai.com/codex/app/commands) |
| Google の AI モード | `https://google.com/ai` | 文を渡す引数は公式に無い (`udm=50` は非公式)。**クリップボード** | support.google.com/websearch/answer/16011537 (「Go to google.com/ai.」) |
| ChatGPT (Web) | `https://chatgpt.com/` | `?q=` は公式に無い (非公式の報告は動作が食い違う)。**クリップボード** | 入口のページとして開くだけ |
| Claude (Web) | `https://claude.ai/` | `claude.ai/new?q=` は公式に無い (公式にあるのは Claude Code 用の `claude.ai/code?prompt=` だけで、mIV の質問の送り先ではない)。**クリップボード** | 入口のページとして開くだけ |

- ChatGPT のアプリで mIV の MCP が使えるのは Codex の画面だけ (2026-11-25 にマニュアルへ追記済み)。`codex://new` は
  Codex の新しい会話を開く (公式文書は「A new local chat.」)。`codex://threads/new` には `prompt` の記載が無いので使わない。
- 「アプリが入っていないとき」の動作はどちらの公式文書にも無い。登録済みと検出したアプリにしか項目を出さないので
  通常は起きない。起きたときは §7.2 の通知。
- 確認できなかったもの (使わない): Web の ChatGPT・Claude・Google の AI モードへ質問を渡す引数、Gemini・Copilot の引数。

## 4. メニューの形

```
ヘルプ
  ヘルプサイトを開く
  AI で調べる  >
  ...                         ┌────────────────────────────────────────┐
                              │ Claude で調べる (Claude Desktop)        │  ← 登録済みのときだけ
                              │ ChatGPT で調べる (Codex)                │  ← 登録済みのときだけ
                              │ ────────────────                        │
                              │ Google の AI モードで調べる             │
                              │ ChatGPT (Web) で調べる                  │
                              │ Claude (Web) で調べる                   │
                              │ ────────────────                        │
                              │ AI 連携の始め方 (MCP の設定)            │
                              │ (説明 1 行、弱い文字)                   │
                              └────────────────────────────────────────┘
```

- **各項目は送り先ごとの固定の ID で描く**。送り先とこのメニューの名前空間から作った `egui::Id` を `UiBuilder::scope_id`
  (`vendor/egui/src/ui_builder.rs:63`) に渡した子 UI の中に、項目のボタンを 1 つだけ置く。`ui.push_id` は子の `unique_id` に
  親の自動の順番が混ざるので (`vendor/egui/src/ui.rs:254`・`:2299`) 使わない (独立レビュー r2 P3)。
  egui のボタンの自動の ID は前にある widget の数で決まり (`vendor/egui/src/ui.rs:929`)、離したときのクリックは押したときの ID に
  届く (`vendor/egui/src/interaction.rs:180`)。押している間に検出の結果が届いて上の区画が増減すると、自動の ID のままでは
  別の送り先が同じ ID を受け継ぎ得る (独立レビュー r1 P2)。区切り線・説明の行・「確認中…」の行も同じく固定の鍵の中に置く。
- 1 つ目の区画: `Registered` のアプリだけ。どちらも登録していなければ区画ごと出さない。初回の検出中は「AI アプリの登録を確認中…」(弱い文字、押せない)。
- 2 つ目の区画 (Web の AI): 常に出す。押すと案内の文をクリップボードに入れてから開き、トースト
  「案内の文をコピーしました。開いたページの入力欄に貼り付けて (Ctrl+V)、続けて質問を書いてください」。
- 3 つ目の区画: マニュアルの「AI 連携の始め方」を開く。説明の 1 行は登録の有無で変える:
  - 未登録: 「AI アプリに mIV を登録すると、この版のマニュアルを引いて答えます」
  - 登録済み: 「Web の AI へは案内の文をコピーして開きます」
- **未決 (利用者の判断 U1)**: 利用者の言葉は「設定済みの場合、このメニューの**代わりに**」。上の推奨は、登録済みでも Web の AI と
  始め方の区画を残す (片方のアプリだけ登録している人・Google の AI モードを使いたい人のため。条件分岐も「出す / 出さない」の
  1 つで済む)。言葉どおりにするなら、登録済みのアプリが 1 つでもあれば 2・3 つ目の区画を出さない。どちらでも作りは同じ大きさ。

## 5. 案内の文

開いた先で利用者が続けて質問を書くので、文は「質問: 」で終わる。版はビルド時の `CARGO_PKG_VERSION`。

- AI アプリ (MCP でつながる):
  `mImageViewer の MCP ツール (search_help・get_help) で、この版のマニュアルを調べてから答えてください。質問: `
- Web の AI (mIV につながらない):
  `mImageViewer (mIV、Windows 用の画像・動画・本のビューア) v{版} の使い方の質問です。公式マニュアル https://www.mikage.to/mimageviewer/manual/ の内容をもとに答えてください。質問: `

入れないもの: 今開いている場所・ファイル名・表示中の画面の名前・設定の値。

- 場所やファイル名は利用者の情報で、Web の AI ではそのまま提供元へ送られる。
- メニューバーから開くので、開いた時点の画面は常に一覧 (全画面表示にはメニューバーが無い)。画面の名前は手がかりにならない。
- MCP でつながる AI は、利用者が許可していれば `get_view_state`・`get_settings` で自分で確かめられる (§1.843)。
  mIV が文に入れると、許可の設定を通らずに状態を渡す経路になる。

改行は入れない (アプリの入力欄で改行が送信に化けるかを確かめられないため。1 行で足りる)。

## 6. 登録済みの判定

- AI 連携の窓と同じ `RegistrationEnvironment::capture()` + `agent_registration::detect` を worker で走らせる
  (今の `AgentIntegrationState::start_detection`、`src/ui_dialogs/agent_integration.rs:99-127`)。読むのは
  `%LOCALAPPDATA%\Packages` の一覧と、各アプリの設定ファイル (小さい JSON / TOML) だけ。UI スレッドでは読まない。
- spawn と受信口の形は、AI 連携の窓とこのメニューで**同じ関数**にまとめる。テストの差し込み
  (今の `MockBackend`、headless で実ファイルを読まない assert) もその関数の 1 か所で効くようにする。
- App が持つのは `AskAiRegistration { pending: Option<Receiver>, latest: Option<[bool; 2]>, last_started: Option<Instant> }`
  の 1 つ (名前は実装で決めてよい)。採用は root の poll ではなく、サブメニューを描くときの `try_recv` だけ
  (サブメニューを開いていない間に届いた結果は次に開いたときに採る)。worker は完了時に ROOT の repaint を頼む。
- 始める条件: サブメニューを描いた frame で、`pending` が無く、`last_started` が無いか 5 秒以上前。
- **`Registered` は「AI アプリの設定ファイルに、この mIV の MCP の起動の行が書いてある」ことの代理の判定にすぎない**
  (`src/agent_registration/config.rs:183` の command・args の一致)。アプリが入っていること・設定を読み込み済みであること・
  今の版のヘルプが引けることは保証しない。ChatGPT 側は `%USERPROFILE%\.codex` があるだけでも検出の対象になる
  (`src/agent_registration.rs:299`、Codex CLI だけの環境)。その場合に項目を押してアプリが開かなければ §7.2 の警告になる。
  登録の直後で AI アプリを再起動していない場合は、開いた先で MCP ツールが使えないことがある (マニュアルの既存の案内
  `manual/ai-integration.html:101` のとおり、AI アプリの再起動で直る)。この差は仕組みで埋めない (独立レビュー r1 P3)。
- 「登録済み」と見なすのは `RegistrationState::Registered` だけ。
  - `DifferentTarget` (別の mIV の exe を指している) は出さない。開いた先の AI アプリが古い・無い exe を起動し得るため。
  - 検出の失敗 (`Failed`)・`Unreadable`・`Ambiguous` も出さない (始め方の区画は出るので、そこから AI 連携の窓へ進める)。
  - 許可の範囲 (`agent_level`) は見ない。ヘルプの tool は連携が無効でも答える (`docs/mcp-guide-settings-plan.md` §4.3)。
- AI 連携の窓で登録・解除した直後の反映は、次にサブメニューを開いたときの再検出に任せる (5 秒の間隔の内なら、その後)。
  窓からこの状態へ結果を流し込む経路は作らない。

## 7. 開く処理 (UI スレッドで同期に Shell を呼ばない)

### 7.1 経路

すべて `external_links::spawn_open` の短命 `external-open-*` worker に乗せる (§1.861、`docs/ui-responsiveness.md` 冒頭)。

- 始め方のマニュアル: 既存の `OpenTarget::Url` (HTTP(S) の検査あり)。`ui_helpers::open_url` と同じ。
- Web の AI: `OpenTarget` に `AiWeb { site, text, clipboard_request }` を足し、**同じ worker の中でクリップボードへ書いてから開く** (§7.4)。
  URL は `site` (Google の AI モード・ChatGPT・Claude の 3 つの型) から固定で作り、HTTP(S) の検査も通す。
- AI アプリ: `OpenTarget` に**型付きの** `AiApp(AiAppLink)` を足す。`AiAppLink` は
  `ClaudeNew { prompt }`・`CodexNew { prompt }`・`CodexSettings` の 3 つで、URL は `external_links` の中で
  固定の前置き (`claude://claude.ai/new?q=`・`codex://new?prompt=`・`codex://settings`) から作る。
  任意のスキームを通す入口は作らない (メタデータ由来の URL は今までどおり HTTP(S) だけ)。
- 今の AI 連携の窓の「設定を開く」(`agent_integration.rs:613`、UI スレッドで同期に `opener::open("codex://settings")`)
  も `AiAppLink::CodexSettings` へ移す。同じ形の Shell 呼び出しを 2 つの経路で持たないため。
  失敗の表示は窓の中の赤字から §7.2 の警告に変わる (worker から窓の状態へは返さない)。

### 7.2 失敗の通知

`ExternalOpenFailure` (今は `GoogleMaps` だけ) に `ClaudeApp`・`ChatGptApp` を足し、既存の App 共通の受信口
(`external_open_failure_tx` / `external_open_failures`、`src/app/external_open.rs`) で警告を出す。
文: 「Claude Desktop を開けませんでした」「ChatGPT のアプリを開けませんでした」。
Web の AI のクリップボードの結果の通知は §7.4。ページを開くこと自体の失敗は、マニュアルとともに
今の「ヘルプサイトを開く」と同じく通知しない。

ログ: `external_open ai_app failed` の種類だけを残す。**案内の文は固定なので個人の情報は入らないが、URL を含むエラー文は
そのまま書かない** (今の地図と同じ扱い。将来、文に利用者の入力を足したときに漏れないように)。
perf の `ext/open_end` は `target_kind` = `ai_app`。

### 7.3 URL の作り方

- 案内の文は `percent-encoding` (Cargo.lock に既にある。本体の Cargo.toml へ直接の依存を足す) で、英数字と `-._~` 以外を
  すべて `%XX` にする。空白は `%20` (`+` にしない。アプリのスキームの扱いを `+` の解釈に依存させない)。
- 長さ: 案内の文は固定で約 60 字 (UTF-8 で約 180 バイト、符号化で約 540 字)。Claude の上限 (約 14,000 字) に遠い。
  URL の検査の上限 (`MAX_EXTERNAL_URL_LEN` 4,096) を AI アプリの URL にも課し、テストで収まることを確かめる。

### 7.4 クリップボード

`ui.ctx().copy_text` は使わない。`copy_text` は出力の命令を積むだけで、実際の書き込みはフレームの終わりに egui-winit が
UI スレッドで同期に行う (`vendor/egui-winit/src/lib.rs:1280` → `vendor/egui-winit/src/clipboard.rs:134`)。一方 `spawn_open` は
すぐ worker を始めるので、「書いてから開く」の順が保証されない (独立レビュー r1 P2)。

- 押したとき、UI スレッドでは既存の `request_edited_clipboard_write()` (`src/ui_dialogs/context_menu.rs:2804`) で、
  mIV のクリップボードの書き込みの世代と、OS のクリップボードの世代 (baseline) を取り、`OpenTarget::AiWeb` を投入するだけ。
  UI ではクリップボードの mutex を取らない (既存の注記のとおり)。
- worker は、(1) 文を `CF_UNICODETEXT` で書き、(2) その結果を App へ知らせ、(3) URL を開く。書き込みの判定は
  編集反映の画像コピーと同じ `write_image_clipboard_if_latest` (`context_menu.rs:3172`) の手順を**テキスト用に共有**する:
  `OpenClipboard` の成功後・`EmptyClipboard` の前に、mIV の世代がまだ最新か、OS の世代が baseline のままかを照合し、
  どちらかが変わっていれば書かない。これで、押した後に mIV の別のコピー (世代を進める画像コピー) をした場合も、
  `copy_text` 経由の通常のテキストコピー (パス・名前のコピー `context_menu.rs:2064`、native presenter の Ctrl+C
  `render_core.rs:14478`。どちらも mIV の世代を進めない) や他のアプリのコピーをした場合も、後のコピーを上書きしない
  (独立レビュー r2 P2)。要求の型・結果の型 (`ClipboardImageWriteResult` の Written / Superseded / ClipboardChanged / Failed)・
  Win32 の API の抽象 (`ClipboardImageApi`) は画像と共有し、テキスト用に名前を一般化してよい。native presenter 側には
  mutex の待ちを足さず、今の `write_clipboard_text_windows` は変えない。
- 通知は worker の結果で決める (独立レビュー r2 P2)。既存の App 共通の受信口 (`external_open_failure_tx` / `external_open_failures`)
  を、失敗だけでなくこの結果も運べるように広げる (型名を変えてよい)。UI の完了待ち・新しい pending の状態は作らない。
  - Written: トースト「案内の文をコピーしました。開いたページの入力欄に貼り付けて (Ctrl+V)、続けて質問を書いてください」
  - Superseded / ClipboardChanged: トースト「ほかのコピーがあったため、案内の文はクリップボードに入れていません」(貼り付けを促さない)
  - Failed: 警告「案内の文をクリップボードに入れられませんでした」。まれな失敗なので再試行はしない。
  - どの場合もページは開く。
- テスト: worker の opener の差し込み (`TestOpenerGuard`) と同じ形で、クリップボードの API にも差し込みを用意し (既存の
  `ClipboardImageApi` の試験用の実装を使ってよい)、実際の OS のクリップボードに触れない。書き込みが終わるまで opener が
  呼ばれないこと、mIV の世代が進むと書かずに開くこと、OS の世代が変わる (通常のテキストコピーの代わり) と書かずに開き
  後のコピーが残ること、各結果で上の通知が届くことを検査する。

今のクリップボードの中身は上書きされる (項目を押した利用者の操作の結果としてトーストで知らせる)。

## 8. ヘルプメニューへの追加とキー割り当て

- `MenuCommandId::HelpAskAi` (表示名「AI で調べる」、`action: None`) を 1 つ足し、サブメニューとして描く
  (設定メニューの「ツールバー」のサブメニューと同じ `ui.menu_button`)。並びは「ヘルプサイトを開く」の直後。
  メニューの並べ替え・非表示の対象になる (既存の `SettingsAgentIntegration` と同じ足し方: 保存済みの並びへの追加、
  `stable_name`、`search_nav_entry_inventory_tests` の分類 = `NoLocationChange`、`menu_command_label` のテスト、
  環境設定のメニューのカスタマイズの一覧)。サブメニューの中の項目は個別のコマンドにしない (登録の有無で増減するため)。
- **`KeyAction` は足さない**。ほかのヘルプの項目 (ヘルプサイトを開く・ログフォルダを開く・バージョン情報) と同じく
  メニューだけの操作。キーで開くと送り先を選ぶ手段が別に要り、メニュー以上の早さが無い。
  固定の入力を足すわけではないので `docs/keymap-spec.md` は変えない。
- 全画面表示・別ウィンドウには入口を作らない (ヘルプメニューがあるのはメインの窓だけ。今のヘルプの項目と同じ)。

## 9. 多言語

mIV の UI は日本語だけで、文言の翻訳の仕組みは無い (`src` に i18n の層が無い。メニューの表示名も
`MenuCommandSpec.label` の日本語の定数)。この機能も日本語の文言だけを足す。案内の文も日本語
(AI は日本語の質問に日本語で答える)。製品ページ・プライバシーポリシーの英語の節は §10 で合わせる。

新しい文言は記号を増やさない (`→`・`▸` を使わない)。`python scripts/check_ui_glyphs.py` を回す。

## 10. 公開文書の更新

mIV 自身は通信しないが、利用者が開いた AI の提供元へ文が届く。「〜だけ」「〜はありません」の記述が偽にならないかを確かめる。

- `privacy.html` 「ネットワーク通信」(日本語・英語): 項目を 1 つ足す。要旨:
  「**AI で調べる (ヘルプメニュー)**: 選んだときだけ、AI アプリまたは既定のブラウザーが開きます。AI アプリでは、入力欄に
  mIV の MCP ツールで調べるよう頼む固定の案内の文が入ります。Web の AI では、mIV の版と公式マニュアルの場所を含む固定の案内の文を
  クリップボードに入れ、各サービスの入口のページを開きます。案内の文に利用者のファイル・場所・設定は含めません。
  mIV は質問を送信しません。入力欄の文を送ると、AI アプリやサービスの提供元へ渡り、その扱いは各社のプライバシーポリシーに従います。
  mIV の開発者には送られません。」
  (mIV が送信の操作をしないことから、開いた先のアプリ・サービスが通信や保存をしないとは書かない。独立レビュー r1 P3)
  「端末内に保存されるデータ」: 新しく保存するものは無い (登録の検出結果はメモリだけ。クリップボードは OS の機能で、
  利用者の次のコピーで置き換わる)。記述は足さない。
- `index.html` 「安心して使えます」の「通信が発生する場面」: 「ご自分で選んだ機能が通信します」に含まれ、偽になる否定の記述は無い
  (独立レビュー r1 でも、この機能で偽になる否定表現は見つからなかった)。カードは変えない。下の注記 (`trust-note`) は MCP の
  連携で mIV が AI アプリへ渡す情報の説明で、この機能は MCP を通さないので足さない。実装時に読み直して確かめる。
- マニュアル:
  - `settings.html` 「ヘルプメニュー」: 「AI で調べる」の説明 (区画ごとの動き・クリップボード・送るのは利用者)。
  - `ai-integration.html`: 使い方を聞く入口として 1 段落 (登録すると「Claude で調べる」等が出る)。
  - `tut-ai-integration.html` は aidoc-tut ブランチで作り直し中なので、**このブランチでは触らない**。
    「ヘルプの『AI で調べる』から使い方を聞ける」を足すかは aidoc-tut の担当へ統括から伝えてもらう (§11 U4)。
- 設計文書: `docs/spec.md` のメニューの表 (ヘルプの行の下に説明。実装レビュー r1 P3)、`docs/architecture-overview.md` の外部リンク・AI 連携の行、`docs/ui-responsiveness.md` 冒頭の §1.861 の節に
  AI アプリの URL も同じ worker に乗ることと、登録の検出がメニューの描画から worker で走ることを 1〜2 文。
  `docs/README.md` にこの文書の行。
- CHANGELOG は統括の取り込み時の方針に従う (このブランチでは書かず、下書きを統括へ渡す)。

## 11. 利用者の判断事項 (統括経由)

- **U1 登録済みのときの区画**: 推奨 = 登録済みのアプリの項目を上に足し、Web の AI と始め方は残す。
  代案 = 言葉どおり、登録済みのアプリが 1 つでもあれば Web の AI と始め方を出さない。
- **U2 Web の AI はクリップボード**: 質問を渡す URL が公式に無いので、案内の文をコピーして入口のページを開く。
  クリップボードの中身は上書きされる (トーストで知らせる)。代案 = 文を渡さず入口のページを開くだけ
  (mIV の案内が付かず、Web の AI は mIV を知らないことが多い)。
- **U3 名前**: 「AI で調べる」「Claude で調べる (Claude Desktop)」「ChatGPT で調べる (Codex)」「Google の AI モードで調べる」
  「ChatGPT (Web) で調べる」「Claude (Web) で調べる」「AI 連携の始め方 (MCP の設定)」。置き場所はヘルプメニューの
  「ヘルプサイトを開く」の直後。
- **U4 チュートリアル**: aidoc-tut の作り直しに「ヘルプの『AI で調べる』から聞ける」を足すか。
- 小窓 (§2 の案 A) は採らない。開く前の確認の窓も作らない (どの送り先も mIV は送らず、送るのは開いた先の利用者)。

## 12. テストとサブ PC での確認手順

### 12.1 自動テスト

- 文と URL: 各 `AiAppLink` の URL が固定の前置きで始まり、案内の文の符号化を戻すと元の文になる (`%20`、`+` を含まない)、
  4,096 以内。Web の AI の URL が HTTP(S) の検査を通る。
- 登録の判定: 検出の結果の差し込みで、`Registered` だけが項目になる (`DifferentTarget`・`Unreadable`・`Ambiguous`・`NotFound`・
  `Unregistered`・失敗は出ない)。5 秒の間隔の内は再検出しない、過ぎたら始める、実行中は重ねない。
- メニュー: `HelpAskAi` の追加 (保存済みの並びへの追加・非表示・`stable_name`・分類)。サブメニューの描画を egui_kittest で、
  未登録・Claude だけ・両方・検出中の 4 つの状態の項目の並びを検査する (スナップショットは足さない。並びの検査で足りる)。
- 固定の ID: 上の区画が増えても減っても、同じ送り先の項目の `Response.id` が変わらない。項目を押している間に検出の結果を
  採って上の区画が増減しても、離したときに別の送り先を開かない (押した送り先を開くか、何も開かない)。
- 開く: `TestOpenerGuard` で、押したときに worker で 1 回だけ開き、UI の呼び出しは待たない。Web の AI ではクリップボードの
  書き込みの差し込み (§7.4) が終わってから opener が呼ばれる。§7.4 のテストのとおり、後のコピーを上書きせず、結果ごとの通知が届く。
  AI アプリの失敗で `ExternalOpenFailure::ClaudeApp` 等が届き、警告が出る。ログに URL も案内の文も出ない。
- AI 連携の窓の「設定を開く」が worker 経由になった (UI スレッドで opener を呼ばない)。

### 12.2 サブ PC (v5.8.0 の候補のインストーラ版、利用者の Claude Desktop / ChatGPT のアプリがある環境)

1. AI 連携で何も登録していない状態で、ヘルプ →「AI で調べる」。Claude / ChatGPT のアプリの項目が無く、Web の 3 つと
   始め方が並ぶ。
2. 「Google の AI モードで調べる」: ブラウザーで google.com/ai が開く。トーストが出る。入力欄に Ctrl+V で案内の文が入る。
   ChatGPT (Web)・Claude (Web) も同じ (ログインの要否は各サービスの通り)。
3. 「AI 連携の始め方」でマニュアルの AI 連携のチュートリアルが開く。
4. AI 連携の窓で Claude Desktop を登録 (手順は AI 連携の窓の案内どおり) → 5 秒以上おいてサブメニューを開き直すと
   「Claude で調べる (Claude Desktop)」が出る。押すと Claude Desktop の新しい会話が開き、入力欄に案内の文が入っていて
   **送信されていない**。続けて「フォルダをお気に入りに登録する方法」と書いて送り、mIV の MCP ツールを使って答えるか。
5. ChatGPT のアプリも登録し、「ChatGPT で調べる (Codex)」で Codex の新しい会話に案内の文が入り、送信されていないこと。
   **`codex://new?prompt=` で文が入るかはこの手順で初めて確かめる** (公式文書の記述だけが根拠)。入らなければ統括へ。
6. Claude Desktop を起動していない状態で 4 を押すと、Claude Desktop が起動して同じ状態になるか。
7. AI 連携の窓の ChatGPT の「設定を開く」が今までどおり Codex の設定を開くか (worker 経由に変えたため)。
8. 登録を外して、5 秒以上おいてサブメニューを開き直すと、アプリの項目が消える。
9. (できれば) 登録した直後、AI アプリを再起動しないまま「Claude で調べる」で送ると、MCP ツールを使わずに答えるか、
   使えないと言うか (§6 の代理の判定の限界。仕組みでは埋めない。結果はマニュアルの書き方に使う)。
10. (環境があれば) ChatGPT のアプリを入れず Codex CLI だけがある環境で、ChatGPT の項目が出て、押すと警告になるか。

## 13. 独立レビューの記録

### 13.1 1 巡目 (2026-11-25、対象 `4964dc7de`、gpt-6.1-sol / xhigh / read-only) — ACCEPT WITH CHANGES

B 案・App の検出の cache・検出と MockBackend の共通化・型付き `AiAppLink`・`codex://settings` の worker 化・
`MenuCommandId` に載せて `KeyAction` を足さない方針は妥当とされた。指摘 4 件はすべてコードで確かめて受け入れた。

- P2 クリップボード: `copy_text` は命令を積むだけで、書き込みはフレームの終わりに UI スレッドで同期。worker が先に開き得る
  → 同じ worker で書いてから開く (§7.4)。
- P2 項目の ID: 開いているメニューの項目の増減で、押した ID が別の送り先へ移り得る → 固定の ID と回帰テスト (§4・§12.1)。
- P3 `Registered` の意味: 設定ファイルの一致の代理の判定で、アプリの実在・接続・版を保証しない → 説明を改め、確認手順に足した (§6・§12.2)。
- P3 公開文書の文: アプリ用と Web 用の文の違い、「受け渡しをしない」の矛盾 → privacy の要旨を直した (§10)。

### 13.2 2 巡目 (2026-11-25、対象 `a92ab8b4a`) — ACCEPT WITH CHANGES

登録の判定・privacy の修正は解消とされた。指摘 3 件はコードで確かめて受け入れた。

- P2 世代だけでは通常のテキストコピー (`copy_text`、mIV の世代を進めない) を上書きし得る → 編集反映の画像コピーと同じく、
  OS の世代の baseline も照合する。手順・型を画像と共有し、render_core の関数は移さない (§7.4)。
- P2 書かなかったのに「コピーしました」と出る → worker の結果で通知を分ける (§7.4)。
- P3 `push_id` では子の ID が親の順番に依存する → `UiBuilder::scope_id` (§4)。

### 13.3 3 巡目 (2026-11-25、対象 `64682cab8`) — ACCEPT

指摘なし。r2 の 3 点 (OS の世代の照合、結果ごとの通知、`scope_id`) は既存のコード (`context_menu.rs:2804`・`:3195`、
`vendor/egui/src/ui.rs:256`) と整合するとされた。

### 13.4 実装の独立レビュー 1 巡目 (2026-11-25、対象 `3e3007dc0..043045dbb`、別の gpt-6.1-sol / xhigh / read-only) — ACCEPT WITH CHANGES

P1・P2 なし。UI スレッドの境界・URL とログ・画像コピーの既存の動作・検出の周期・固定の ID・通知の対応・テストの差し込みに
欠陥は見つからなかったとされた。P3 1 件 (`docs/spec.md` のヘルプメニューの一覧に「AI で調べる」が無い) を受け入れて直した。

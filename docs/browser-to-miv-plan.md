# ブラウザで見ている画像を mIV で見る・保存する (§1.818) 設計書

ステータス: **調査・設計の初版 (2026-11-19)。実装は未着手**。v5.5.0 では設計書まで。実装するか・どの段まで進めるかは利用者の判断 (§13) のあと。
担当: 調査・設計 = ClaudeCode Opus 5.5 / high。独立レビュー = GPT-6.1 Sol / xhigh (§14)。
作業ブランチ: `browser-818-plan` (worktree `G:\home\mimageviewer-browser818`)。

この文書の事実は次の 3 種類だけで書く (CLAUDE.md「観測の出どころを落とさない」)。

- コード: `path:line` を付ける (2026-11-19 の master `ca6686d93` で確認)。
- 外部の一次情報: URL を付ける。2026-11-19 に ClaudeCode の調査役がページを取得して確認した。法的な解釈はしていない。
- 未確認: 「未確認」と書く。**エージェントはブラウザでの実際の操作も mIV の起動もしていない**ので、ブラウザや X・pixiv が実際にどう振る舞うかについての観測はこの文書に無い。

## 1. 要望

利用者 (2026-11-19):

> 「Chromeブラウザで、画像をｍIVに転送するなどして、３Dやアップスケールなどで閲覧したり、画像を保存できる機能。どのような方法がとれるのかの検討からだとおもいますが、ブラウザ観ているXポストの画像や、pixiv画像などを、ｍIVの機能を使って手軽に閲覧するような機能が欲しいです」

言い換えると、「ブラウザで見つけた 1 枚を、少ない操作で mIV の全画面に出し、そこで 3D 写真 (D) やアップスケール (U) を使う。気に入ったら手元に残る」。

## 2. 結論 (推奨)

1. **段 1 (推奨・小さい): 既存の「クリップボード取り込み」に「見る」を足す。**
   - Chrome で右クリック →「画像をコピー」→ 右下の小窓の **[見る]** → mIV の全画面に出る → D / U。
   - 画像はブラウザがすでに持っているものを使う。mIV は通信しない。拡張機能・ストア公開・レジストリ登録が要らない。
   - mIV へ開く経路は、2 重起動・MCP と同じ既存の経路 1 本に乗せる (§8)。
2. **段 2 (利用者の判断): Chrome 拡張「mIV で開く」。**
   - 右クリック 1 回で開け、クリップボードの監視を ON にしなくてよい。ブラウザに画像を保存させ (ブラウザの「名前を付けて画像を保存」と同じ取得)、保存したファイルのパスだけを native messaging で mIV へ渡す形を推奨する (§5 案 B)。
   - ただし、Chrome ウェブストアへの公開と審査、拡張と中継 exe の保守が新しく発生する。段 1 を使ってみて、それでも手間が残るときに判断してもらう。
3. **mIV が URL から自分で画像を取りに行く方式 (案 C・G) は推奨しない。** X の規約の「公開されたインターフェース以外の手段によるアクセス」の禁止に最も近い (§4.1)。
4. **送る画像は「ブラウザが表示している画像」に限る。** サイトごとに原寸の URL へ書き換える処理は作らない (§6)。
5. **画像は常に保存してから開く。** 「見るだけで保存しない」一時表示は作らない (状態を増やさないため、§7)。

## 3. 今あるもの (コードで確かめたこと)

### 3.1 入口の一覧

| 入口 | 何を受け取るか | 受け取った後 | 根拠 |
| --- | --- | --- | --- |
| 2 重起動 (`mimageviewer.exe <path>` / core) | ファイルのパス 1 つ | 親フォルダを一覧に開き、画像・動画・音声ならそのファイルを全画面で開く | パイプ `\\.\pipe\mImageViewerOpenPath_v1` ([single_instance.rs:181](../src/single_instance.rs))、受信 [startup_ops.rs:302](../src/app/startup_ops.rs)、全画面 [startup_ops.rs:1835](../src/app/startup_ops.rs)・[docs/spec.md:2487](spec.md) |
| MCP `open_in_viewer {path}` | 絶対パス 1 つ | 2 重起動と同じ解決 (フォルダ → 選択 → 全画面) | [agent_ipc/open.rs:148](../src/agent_ipc/open.rs)、[startup_ops.rs:190](../src/app/startup_ops.rs)。権限 `ReadAndOpen` 以上が要り、既定は `Disabled` ([crates/agent-ipc/src/lib.rs:328](../crates/agent-ipc/src/lib.rs)) |
| ウィンドウへの D&D | `CF_HDROP` のファイルだけ (winit 0.30 の drop handler) | **今のフォルダへコピー**する。開かない | [update_phases.rs:434](../src/app/update_phases.rs)、[app.rs:52566](../src/app.rs) |
| 一覧の Ctrl+V | ファイル類 / 画像データ / HTML | 画像データと HTML は今のフォルダへ保存。テキストの URL は扱わない | [app.rs:64104](../src/app.rs)、[clipboard_capture/data.rs:120](../src/clipboard_capture/data.rs) |
| クリップボード取り込み (v4.4.0、既定 OFF) | コピーされた画像データ / ページの HTML | 画像は `ピクチャ\mimageviewer\clipboard\YYYY-MM` へ自動保存し、右下の小窓で知らせる。HTML は小窓 → 選択ダイアログで画像を取得して保存 | [clipboard-capture-plan.md](clipboard-capture-plan.md) §2、§8 |
| mIV Remote の IPC | 一覧・サムネイル・ページ・AI ジョブ等 | PC 側で「このファイルを開く」要求は無い | [crates/remote-ipc/src/lib.rs:2709](../crates/remote-ipc/src/lib.rs) |
| カスタム URL スキーム (`miv://` 等) | 無い | — | 登録コード・インストーラの記述とも無し |

### 3.2 クリップボード取り込みの今の動き (段 1 の土台)

- Chrome / Edge の「画像をコピー」は、`PNG` と `HTML Format`、`UniformResourceLocatorW` (画像の URL)、`Chromium internal source URL` (ページの URL) を入れる (2026-10-02 の S0 の記録、コピー操作は利用者、[clipboard-capture-plan.md](clipboard-capture-plan.md) §5.8)。
- mIV は `PNG` を保存し、出どころ (画像 URL・ページ URL) を Mark of the Web (`Zone.Identifier`) に書く (同 §10.2)。ファイル名は `<日時>-<ドメイン>_<連番>` (同 §2.6)。
- 保存したら小窓に「クリップボードの画像を保存しました [開く] [×]」を出す。**[開く] は保存先フォルダを一覧に開いてファイルを選ぶだけ**で、全画面にはしない。しかも一覧表示中にしか受け付けず、全画面・detached 中は小窓の文面を差し替えて何もしない ([app.rs:19934](../src/app.rs) `clipboard_capture_reveal_admitted`、同 §6.4)。
- 小窓の段階では通信しない。HTML から画像を取りに行くのは、利用者が選択ダイアログを開いたときだけ (同 §4-2)。

### 3.3 開いた後に 3D 写真・アップスケールが使えるか

- **3D 写真** (`FsDepthParallax`、既定キー D) は画像の閲覧画面 (全画面・ウィンドウ・F12 の別ウィンドウ) で使える。表示中のテクスチャから奥行きを推定するので、ファイルの置き場所や索引への登録は条件にならない。動画・アニメーション画像・見開き・連結読み・GPU 描画なしでは使えない ([depth_parallax.rs:174](../src/depth_parallax.rs)、[depth-parallax-plan.md](depth-parallax-plan.md) §6.1)。初回はモデル (約 0.67 GB) の追加ダウンロードが要る。
- **AI アップスケール** は全画面で U / Shift+U / Alt+U (補正プリセットの切り替え)。`ai_feature_mode` と大きさの上限に従う。アニメーション画像には掛からない ([ui_fullscreen.rs:37002](../src/ui_fullscreen.rs)、[spec.md:3584](spec.md))。
- どちらも「普通のフォルダにある普通の画像ファイル」なら追加の条件は無い。したがって、**受け取った画像を普通のフォルダに保存して既存の経路で全画面に開けば、3D 写真とアップスケールへのつなぎは何も作らなくてよい**。

### 3.4 配布形態

- インストーラは Inno Setup で、レジストリを書かない (`[Registry]` 節が無い、[installer/mimageviewer.iss](../installer/mimageviewer.iss))。アプリもレジストリへは書かない (読み取りだけ)。
- Microsoft Store には Inno の EXE をそのまま出している (MSIX ではない、[release-checklist.md](release-checklist.md) の Store の節)。
  - MSIX では HKCU への書き込みが仮想化され、Chrome から見えない (https://learn.microsoft.com/en-us/windows/msix/desktop/desktop-to-uwp-behind-the-scenes)。
  - mIV は MSIX でないので、段 2 の native messaging の登録はこの制約に当たらない。

## 4. 規約・配布の一次情報

2026-11-19 に取得して確認した。引用は短い抜粋で、法的な判断ではない。**どの方式なら規約上問題ないと断定はできない**ので、ここでは「方式ごとにどの条項が論点になるか」だけを整理する。

### 4.1 X 利用規約 (Effective: October 9, 2026、https://x.com/en/tos)

- 公開されたインターフェース以外の手段によるアクセスを禁じる: "access or search … by any means (automated or otherwise) other than through our currently available, published interfaces"
- 注記: "crawling or scraping the Services in any form, for any purpose without our prior written consent is expressly prohibited"
- 複製・再利用は X の提供する手段で: "If you want to reproduce, modify, … distribute, … or otherwise use the Services or Content on the Services, you must use the interfaces and instructions we provide"
- 利用者へのライセンスは、X が提供するソフトウェアを X 上で使うためのもので、他人の投稿の保存を明示的に許す文言は見当たらない。保存を明示的に禁じる文言も見当たらない。
- 違反を "induce or knowingly facilitate others" した者も責任を負う条項がある (大量取得の損害賠償の節)。ツールの配布者に関係し得る。
- X API を利用者ごとのキーで使う方式は規約上取れない (X Restricted use cases「Multiple applications」、メモ `reference_x_api_byok_not_allowed`)。API を使わないなら Developer Agreement の当事者にはならないが、上の ToS の条項が直接の論点になる。

方式への当てはめ (解釈であり、X の公式見解ではない):

- **ブラウザが表示した画像を、ブラウザの機能 (コピー・保存) で取り出す** (案 A・B): ブラウザは X が公開しているインターフェース (Web サイト) そのもの。どの条項も名指ししていない。
- **mIV がブラウザの外から画像 URL を直接取りに行く** (案 C・G): "other than through our currently available, published interfaces" に当たると読める余地が最も大きい。

### 4.2 pixiv (サービス共通利用規約 最終改訂 2026-06-22、https://policies.pixiv.net/)

- 第 14 条の禁止行為: 「投稿情報を、当該著作者（創作者）の同意なくして転載する行為」、「通常の範囲を超えて本サービスのサーバーに負担をかける行為」、「機械的にまたは短時間で大量に…操作を行う行為」。
- pixiv ガイドライン (最終改定 2026-03-18、https://www.pixiv.net/terms/?page=guideline): 「クローラーなどのプログラムを使って作品を収集する行為」を禁止。
- 登録商標のガイドライン (アプリ開発者向け): pixiv 製でない旨の併記を求め、「作品投稿者本人の承諾なしに作品を印刷/転載することは禁止します」。
- 閲覧者の私的な保存を明示的に許す文言も、禁じる文言も見当たらない。
- 画像サーバーが参照元 (Referer) を見ることは、ヘルプに「一部のページではサイトを表示するためにリファラを使う仕組みを導入しています」とあるだけで、画像配信の条件としての公式な明記は見つからなかった (https://www.pixiv.help/hc/ja/articles/235584528)。

設計への帰結 (案に依らない):

- **1 回の利用者の操作で 1 枚だけ**。一覧の一括取得・巡回・自動取得は作らない (クローラーとみなされる形を最初から持たない)。
- **mIV から外へ送らない・共有しない**。保存先は利用者の PC だけ。
- 拡張機能をストアに出すなら、説明文に「X・pixiv 製ではない」旨と、サイト名を宣伝に使わないことを守る (§11)。

### 4.3 Chrome ウェブストアと拡張機能の配布

- 禁止される製品: "Do not encourage, facilitate, or enable the unauthorized access, download, or streaming of copyrighted content or media." (https://developer.chrome.com/docs/webstore/program-policies/policies)
  - YouTube の動画ダウンロードを助ける拡張は、典型的な違反例として挙がっている (https://developer.chrome.com/docs/webstore/troubleshooting/)。
  - 動画ダウンローダの類は、違反ではなくても掲載推奨の対象から外され得る。
  - 段 2 の拡張は「ブラウザの『名前を付けて画像を保存』を 1 枚ずつ mIV へつなぐもの」として説明する。審査がどう判断するかは出してみないと分からない (未確認)。
- 単一目的: "An extension must have a single purpose that is narrow and easy to understand."
- 権限は最小に: "Request access to the narrowest permissions necessary"。広い host 権限を求めると審査が長くなる: "Reviews may take longer for extensions that request broad host permissions" (https://developer.chrome.com/docs/webstore/review-process)。
- `activeTab` は右クリックメニューの実行で付与され、インストール時の警告が無い。ただし付与されるのはそのタブの origin だけで、`pbs.twimg.com` のような別 host の画像の取得には host 権限が要る (https://developer.chrome.com/docs/extensions/develop/concepts/activeTab)。
- `nativeMessaging` のインストール時の警告は "Communicate with cooperating native applications" (https://developer.chrome.com/docs/extensions/reference/permissions-list)。
- 登録は 1 回の登録料が要る (金額は公式ページに記載が無く未確認)。Trader / Non-Trader の申告が要り、個人は Non-Trader で登録できる。
- **ストア外の配布は事実上できない**。Windows ではローカルの CRX からのインストールは Chrome 33 から不可で、外部インストールの update_URL はウェブストアを指す必要がある (https://developer.chrome.com/docs/extensions/how-to/distribute/install-extensions)。ストア外のホストは Linux だけ。企業ポリシーでの強制インストールは Active Directory 参加が前提。「パッケージ化されていない拡張機能を読み込む」(デベロッパーモード) は開発用で、一般の利用者に頼む手順にはできない。
- Edge は Edge Add-ons に別途出す (無料)。Edge は Chrome の native messaging のキーも読む (https://learn.microsoft.com/en-us/microsoft-edge/extensions/developer-guide/native-messaging)。

### 4.4 Native messaging (https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging)

- 登録: `HKCU\Software\Google\Chrome\NativeMessagingHosts\<name>` の既定値に manifest の完全パス。manifest は `name` / `description` / `path` / `type: "stdio"` / `allowed_origins` (拡張の ID、ワイルドカード不可)。
- 形式: UTF-8 の JSON の前に 32 bit の長さ。上限は host → 拡張 1 MB、拡張 → host 64 MiB。Windows では stdin/stdout を `O_BINARY` にする。
- `sendNativeMessage` は**1 メッセージごとに host のプロセスを新しく起動**する。第 1 引数は呼び出し元の origin。
- content script からは呼べない (拡張の service worker / ページから)。

### 4.5 PC 内の HTTP サーバーへ送る方式の制約

- Chrome 142 から、Web ページから 127.0.0.1 などへの通信には「ローカル ネットワーク アクセス」の許可が要る (https://developer.chrome.com/blog/local-network-access)。
- 拡張の service worker は、host 権限があれば対象外とされている (Google の LNA adoption guide、2026-05-18 版)。content script はページの origin として扱われる。

## 5. 受け渡し方式の比較

| 案 | 利用者の手間 (1 枚) | 必要な権限・登録 | 失敗の仕方 | 配布・審査 | 規約の論点 (§4) | mIV 側の新規作業 |
| --- | --- | --- | --- | --- | --- | --- |
| **A. 画像をコピー → 小窓の [見る]** (段 1) | 右クリック → コピー → [見る] | 取り込み (画像の自動保存) を ON | 監視が OFF・mIV が起動していない → 何も起きない (今と同じ)。Remote が操作中 → 開かない | なし | ブラウザの機能で取り出す。名指しされない | 小窓のボタン 1 つと、既存の開く経路への接続 |
| **B. 拡張: ブラウザに保存させ、パスを渡す** (段 2 の推奨) | 右クリック →「mIV で開く」 | 拡張 (`contextMenus`・`downloads`・`nativeMessaging`)、HKCU のレジストリ 1 キー | ブラウザの保存の失敗 (ログイン切れ・参照元の制限) はブラウザの保存と同じ。mIV が起動していなければ起動する | ウェブストアの登録・審査・説明文、Edge Add-ons、版の保守 | ブラウザの取得そのもの。ストアの「無断ダウンロードの助長」条項の審査判断は未確認 | 中継 exe、登録 UI、拡張本体 |
| C. 拡張: URL を渡し、mIV が取得 | 右クリック →「mIV で開く」 | 拡張 (`contextMenus`・`nativeMessaging`)、レジストリ | Cookie を使わないので、ログインが要る画像は取れない。参照元を見るサーバーは取り込みの規則次第 | B と同じ | ブラウザの外からの直接取得。X の "published interfaces" に最も近い | B に加えて、core への新しい IPC 要求、取得処理の共用 |
| D. 拡張 → PC 内の HTTP サーバー | B・C と同じ | 拡張に `http://127.0.0.1` の host 権限、mIV に常駐の待ち受け | 他のページからの要求を防ぐ認証が要る (待ち受けを悪用されると、任意のページから mIV に取得をさせられる) | B と同じ | C と同じ | 待ち受け・認証・ポート競合 |
| E. ブラウザから mIV の窓へ D&D | ドラッグ 1 回 (mIV の窓が見えている必要) | なし | Chrome がドラッグに何を載せるかは未調査。mIV は今 `CF_HDROP` しか受けない | なし | A と同じ | 独自の `IDropTarget` (winit の drop handler の置き換え)。**今の D&D は「今のフォルダへコピー」なので、意味を変えると既存の使い方の退行** |
| F. カスタム URL スキーム (`miv://open?url=`) | ブックマークレット等 → 毎回「開きますか」の確認 | レジストリ (スキーム登録) | **どの Web ページからでも起動できる**。悪意あるページが mIV に取得をさせられる | なし | C と同じ | スキーム登録、URL の検査 |
| G. ページの URL を貼り、mIV がページを読んで画像を探す | URL をコピー → mIV で貼り付け | なし | X のページはスクリプトで描かれるので、HTML を読んでも画像が無い見込み (未確認) | なし | スクレイピングそのもの | ページ解析 (サイト依存) |
| H. 既存の MCP `open_in_viewer` / Remote IPC を流用 | — | MCP は AI アプリ向けの権限 (`ReadAndOpen`、既定 OFF) | — | — | — | 流用しない。MCP は AI アプリ用の権限・説明になっていて、ブラウザ連携のために利用者に AI 連携の権限を開けてもらう形は筋が違う。Remote IPC は Remote サービス専用の厳密な版合わせで、開く要求を持たない |

採否:

- **A を段 1 に採る。** 新しい配布物・権限・通信が無く、既存の「画像をコピー」の記録 (§3.2) をそのまま使える。足りないのは「全画面で開く」ことだけ。
- **B を段 2 の候補にする。** 手間は最も少ない。画像の取得はブラウザ自身が、ブラウザの Cookie と参照元の規則で行うので、mIV は通信しない。mIV 側は「保存済みファイルのパスを受け取って開く」だけで、既存の 2 重起動と同じ。
- C は B より mIV 側の作業が多く、規約の論点も大きい。B が技術的に成り立たないとき (§9.5 のスパイクで、ブラウザ経由の保存が参照元の制限で失敗する等) の代替としてだけ残す。
- D・F は、PC 内のどこからでも mIV に取得をさせられる入口を新しく作るので採らない。
- E は Chrome のドラッグの中身が未調査で、今の D&D の意味 (今のフォルダへコピー) を変えることになるので採らない。
- G・H は上の理由で採らない。

## 6. 送る画像の範囲

| 範囲 | 採否 | 理由 |
| --- | --- | --- |
| **ブラウザが表示している画像** (右クリックした画像そのもの) | **採用** | 利用者が見ているものと一致する。取得はブラウザが済ませている |
| 画像がリンクで包まれていて、リンク先が画像ファイルのときはリンク先 (段 2 のみ) | 利用者の判断 (§13 Q4) | サイトに依らない一般の規則。Chrome の右クリック情報 (`OnClickData.linkUrl`) で分かり、追加の権限は要らない。既存の取り込みの HTML 解析も、画像拡張子への `<a href>` を候補にしている ([clipboard-capture-plan.md](clipboard-capture-plan.md) §7)。2026-10-02 の S0 では、作品ページの HTML の 1 件のリンクが原寸画像を指していた (同 §5.8)。ただし、右クリックした画像がそのリンクの中にあるかは未確認 |
| サイトごとに原寸の URL へ書き換える (例: URL の大きさの指定を変える) | **不採用** | サイトの作りに依存し、変更のたびに壊れる。既存の取り込みでも範囲外とした (同 §12)。X の規約の "try to work around any technical limitations" と読まれる余地もある |

段 1 (コピー) で得られる画像の性質:

- ブラウザがデコードした画素を `PNG` にしたもの。元の JPEG のバイト列・EXIF は残らない。アニメーション GIF は 1 枚の静止画になる見込み (未確認)。
- 解像度は、ブラウザがそのとき読み込んでいた画像のもの。X や pixiv が一覧・詳細でどの大きさを配っているかは、この文書では確かめていない (未確認)。小さい画像ほどアップスケールの意味はある。

段 2 (案 B) で得られる画像: ブラウザが保存した元のファイルそのもの (再エンコードしない)。

## 7. 状態の組み合わせを減らすための決定

CLAUDE.md「設計の簡素化」に従い、扱う組み合わせを先に減らす。

1. **開く経路は既存の 1 本だけ**。段 1 の [見る] も段 2 の拡張も、2 重起動・MCP と同じ「パスを開く」要求 (`ActivationOpenPath` → `start_startup_open_path_resolve`) に入れる。
   - その経路はすでに、受付の判定 (Remote の操作中は拒否、本の構成エディタ中は拒否、文書モーダル中は閉じた後に開く、[startup_ops.rs:227](../src/app/startup_ops.rs))、最新 1 件だけ残す、一覧・全画面・detached のどの状態から来ても開く、を持っている。
   - 新しい受付規則・画面ごとの分岐を作らない。
2. **保存してから開く**。開く時点でファイルはディスク上にある。「一時ファイルを表示中に消す」「保存ボタンを押す前に閉じた」などの組み合わせが生まれない。要らない画像は通常の削除で消す。
3. **1 回の操作で 1 枚**。まとめて送る・待ち行列は持たない。続けて送ったら、開く経路の「最新 1 件」に従う。
4. **3D 写真・アップスケールは自動で始めない**。開いたら D / U を押す。自動で始めると、モデル未取得・GPU なし・アニメーション画像・大きさの上限などの組み合わせを開く処理が全部引き受けることになる (§13 Q3 で利用者が望めば、既存の D の入口判定をそのまま呼ぶ形で後から足す)。
5. **段 2 の中継 exe は 1 メッセージ 1 プロセス** (`sendNativeMessage`)。常駐しない・接続を保たない。
6. **mIV が起動していないとき**は、段 1 は何もしない (小窓は mIV の中のもの)。段 2 は既存の launcher にパスを渡して起動する (起動時の引数の経路は既存)。
7. **まれな失敗は割り切る** (CLAUDE.md の既定)。保存の失敗・パイプの失敗はログと通知だけで、再試行・回復の状態を持たない。

## 8. 段 1 の設計: 小窓の [見る]

### 8.1 利用者向けの仕様

- 対象: 「画像がコピーされたら自動で保存する」が ON のとき (既存の設定、既定 OFF)。
- 保存完了の小窓を「クリップボードの画像を保存しました [見る] [開く] [×]」にする。
  - **[見る]**: 保存した画像を mIV の全画面で開く。今のフォルダ・今の画面に関係なく開く (2 重起動でファイルを渡したときと同じ)。全画面では同じ月のフォルダの前後の取り込み画像へも移れる。
  - **[開く]**: 今までどおり (取り込みフォルダを一覧で開いて選ぶ)。リリース済みの動作なので変えない。
- 一覧の Ctrl+V (今のフォルダへ保存) には [見る] を足さない (Ctrl+V は mIV の中の操作で、保存先も一覧に見えているため)。
- HTML のコピー (画像を選んで保存) にも足さない。複数枚の選択の後に「どれを開くか」という新しい選択が生まれるため。

### 8.2 実装の要点 (案)

- 小窓の新しいボタン → popup thread から App へ typed event `ViewSaved(path)` (既存の `RevealSaved(path)` と並べる、[clipboard-capture-plan.md](clipboard-capture-plan.md) §6.4)。
- App は `ViewSaved` を、2 重起動のパイプ受信と同じ `ActivationOpenPath` として投入する。受付・前面化・開く処理は既存のものをそのまま通る。`clipboard_capture_reveal_admitted` (一覧表示中だけ) は使わない。
  - 投入の形 (同じチャネルへ送るか、受信後の関数を直接呼ぶか) は実装時に既存コードで決める。どちらでも、受付判定 `activation_open_precheck` を必ず通す。
  - Remote の操作中など受付されなかったときは、既存の 2 重起動と同じ扱い (ログと既存の通知) にする。小窓の文面の差し替えはしない (開く経路の判定は App の中で後から行われるため)。
- 前面化: 利用者が小窓をクリックした直後なので、既存の [開く] と同じく前面化できる見込み (§6.4 の判断。成功は保証されない、未確認)。
- 「保存したら自動で全画面に開く」設定は作らない。コピーのたびに mIV が前に出ると作業を邪魔するうえ、クリックの無いところからの前面化は Windows の制限で失敗しやすい (Windows の前面化の規則からの推定、未確認)。

### 8.3 detached viewer との関係

- この段は既存の開く経路を呼ぶだけで、detached の述語や viewport の経路を変えない。複数ウィンドウモードで 2 重起動のパスがどの窓に開くかは既存の動作に従う (未追跡)。
- 実装で detached の述語・経路に変更が及ぶことが分かったら、CLAUDE.md の凍結ルール (構造的修正であることの双方合意と [detached-rework-plan.md](detached-rework-plan.md) への記録) に従う。

### 8.4 テスト (案)

- 小窓の event `ViewSaved` が `ActivationOpenPath` と同じ受付判定を通ること (Remote の操作中は開かない、文書モーダル中は閉じた後に開く) の handler-level test。
- 全画面・detached の表示中に [見る] を押すと、保存した画像が開くこと (既存の 2 重起動のテストと同じ形)。
- [開く] の今の動作 (一覧表示中だけ・選択するだけ) が変わらないこと。

## 9. 段 2 の設計の骨子: 拡張「mIV で開く」(案 B)

段 2 は利用者の判断 (§13 Q1) の後で詳しく設計する。ここでは、判断に必要な範囲と未確認の点を書く。

### 9.1 流れ

1. 利用者が画像を右クリック →「mIV で開く」(`chrome.contextMenus`、`contexts: ["image"]`)。
2. 拡張の service worker が、右クリック情報の `srcUrl` (§13 Q4 によっては `linkUrl`) を `chrome.downloads.download` でブラウザに保存させる。保存先は既定のダウンロードフォルダの下の `mImageViewer\` (downloads API はダウンロードフォルダからの相対パスしか指定できない、未確認)。
3. 保存が終わったら (`downloads.onChanged` が `complete`)、`chrome.runtime.sendNativeMessage` で `{ "v": 1, "path": "<保存したファイルの絶対パス>" }` を中継 exe へ送る。
4. 中継 exe はパスを検査し、起動中の mIV の open-path パイプへ送る。起動していなければ launcher にパスを渡して起動する。`AllowSetForegroundWindow` で mIV に前面化を許す (MCP の中継と同じ、[crates/mcp-server/src/pipe.rs:244](../crates/mcp-server/src/pipe.rs))。
5. mIV は 2 重起動と同じく全画面で開く (§7-1)。中継 exe は結果 (`ok` / 理由) を返して終わる。拡張はバッジかツールチップで失敗だけを示す。

### 9.2 権限

- `contextMenus`、`downloads`、`nativeMessaging` の 3 つ。**host 権限は求めない** (取得はブラウザのダウンロード機能が行う)。
- インストール時の警告は「ダウンロードの管理」と「協調するネイティブ アプリケーションとの通信」になる見込み (permissions-list の文言からの推定)。

### 9.3 中継 exe と登録

- 中継 exe は小さな別 exe (`mimageviewer-browser-host.exe`、仮称) にする。core や launcher を host にしない (host は stdin/stdout の決まった形式で 1 メッセージごとに起動されるので、本体の起動処理と混ぜない)。
- 登録は mIV の環境設定から行う (「ブラウザ連携を使う」)。インストーラは今レジストリを書かないので、インストーラでは登録しない。
  - 登録時に、中継 exe と manifest を `%APPDATA%\mimageviewer\browser-host\` に置き、`HKCU\Software\Google\Chrome\NativeMessagingHosts\<name>` と Edge のキーに manifest のパスを書く。
  - 単体 exe 版は本体の場所が版ごとに変わる (`%APPDATA%\mimageviewer\runtime\<ver>`) ので、中継 exe を固定の場所へ写し、起動のたびに版を合わせる。
  - 解除は同じ画面から (キーとファイルを消す)。アンインストール時の扱いはインストーラの変更が要るので、段 2 の詳細設計で決める。
- **既定のデータフォルダの mIV だけ**を対象にする。`--data-dir` やポータブル版はパイプ名が変わる ([single_instance.rs:203](../src/single_instance.rs)) が、native messaging の manifest には引数を書けないので、どの mIV へ送るかを選べない。ポータブル版で使えないことは §13 Q5 で確かめる。

### 9.4 中継 exe の検査

- 第 1 引数の origin が、登録した拡張の ID であること (Chrome も `allowed_origins` で絞るが、中継側でも見る)。
- `v` が知っている版であること。知らない版なら「mIV を更新してください」を返す (拡張と mIV の版のずれ)。
- パスが絶対パスで、存在する通常のファイルで、画像の拡張子であること。開くだけなので、これ以上の制限は置かない (2 重起動で任意のファイルを開けるのと同じ)。

### 9.5 着手前に確かめること (スパイク、利用者の操作が要る)

- `chrome.downloads.download` で保存するとき、ブラウザの「名前を付けて画像を保存」と同じく参照元と Cookie が付くか。付かない場合、参照元を見る画像サーバーで保存に失敗する。**ここが成り立たなければ案 B は取れない** (そのときは案 C を、規約の論点を含めて利用者に相談する)。
- ダウンロードのたびに保存場所の確認ダイアログが出る設定の利用者で、どう振る舞うか (`saveAs: false` で抑えられるか)。
- ダウンロードの一覧・履歴に毎回残ること (利用者に見える副作用) の受け止め。
- ストアの審査で、§4.3 の条項についてどう判断されるか (出してみないと分からない)。

## 10. 保存先・開く画面・3D 写真とアップスケールへのつなぎ方

| 項目 | 段 1 | 段 2 |
| --- | --- | --- |
| 保存先 | 既存の取り込みフォルダ (`ピクチャ\mimageviewer\clipboard\YYYY-MM`、変更可) | ブラウザのダウンロードフォルダの `mImageViewer\` (§9.1、§13 Q6) |
| ファイル名 | 既存の取り込みの規則 (`<日時>-<ドメイン>_<連番>.png`) | ブラウザが決める名前 (重複時はブラウザの規則) |
| 出どころの記録 | 既存の Mark of the Web (画像 URL・ページ URL) | ブラウザが付ける Mark of the Web (ブラウザの既定の動作、未確認) |
| 開く画面 | 2 重起動と同じ: 親フォルダの一覧 + そのファイルの全画面 | 同じ |
| 3D 写真 | 全画面で D (既存) | 同じ |
| アップスケール | 全画面で U (既存) | 同じ |
| 保存しない閲覧 | 作らない (§7-2) | 作らない |

## 11. 公開文書への影響

- 段 1:
  - 通信は増えない (コピーした画像データを保存するだけ。既存の取り込みと同じ)。`privacy.html` の「ネットワーク通信」「端末内に保存されるデータ」は変わらない。
  - マニュアルの「クリップボード取り込み」に [見る] を書き足す。
- 段 2:
  - **mIV は通信しない** (画像の取得はブラウザ)。`privacy.html` の「端末内に保存されるデータ」に、中継 exe と manifest の置き場所とレジストリのキーを足す。「ネットワーク通信」は変わらない見込み。
  - 製品ページの「安心して使えます」(ネットワークは更新確認と「ご自分で選んだ機能」、[index.html:1252](../htdocs/mimageviewer/index.html)) の文言は偽にならない。
  - 拡張のストア掲載ページに、プライバシーの開示 (収集しない) と、X・pixiv 製ではない旨を書く。サイト名やロゴを宣伝・アイコンに使わない。
- どちらの段も、外部ダウンローダの名前・URL・使用例を公開文書とコミットメッセージに書かない ([public-wording-policies.md](public-wording-policies.md))。機能の説明は「ブラウザで見ている画像を mIV で開く」とし、「サイトから画像を集める」「一括で保存する」とは書かない。

## 12. 範囲外

- 複数枚の一括送信、タイムライン・作品一覧の巡回、自動取得。
- サイトごとの原寸 URL への書き換え。
- X API・pixiv の非公開 API の利用。
- 動画の送信 (X の動画など)。
- Firefox の拡張 (段 2 で Chrome / Edge が成り立ってから検討)。
- mIV Remote からの送信。

## 13. 利用者に決めてもらう点

| # | 論点 | 推奨 |
| --- | --- | --- |
| Q1 | どこまで作るか | **段 1 (小窓の [見る]) を先に作り、使ってみてから段 2 (拡張) を判断する** |
| Q2 | 段 1 で、自動保存を ON にしないと使えない形でよいか (コピーした画像はすべて取り込みフォルダに残る) | よい (既存の設定を使い、新しい状態を作らない) |
| Q3 | 開いたら 3D 写真やアップスケールを自動で始めるか | 始めない (全画面で D / U を押す) |
| Q4 | 段 2 で、画像がリンクで包まれていてリンク先が画像ファイルのときにリンク先を開くか | 開く (サイトに依らない規則で、原寸に近い画像が取れることがある)。ただし段 2 のスパイクで確かめてから |
| Q5 | 段 2 を、既定のデータフォルダの mIV (インストール版・単体 exe 版) に限ってよいか (ポータブル版・`--data-dir` では使えない) | 限る |
| Q6 | 段 2 の保存先をブラウザのダウンロードフォルダの下にしてよいか (mIV の取り込みフォルダには入らない) | よい (ブラウザの機能で保存するため) |
| Q7 | 段 2 で Chrome ウェブストアに開発者登録して拡張を公開するか (登録料・審査・保守が発生し、審査で掲載を断られる可能性がある) | 段 1 を使ってから判断 |

## 14. レビュー記録

(独立レビューの結果をここに追記する)

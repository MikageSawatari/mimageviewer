# 外部ツール起動 (External Tool Launch) 設計 — 正本

対象: backlog [§1.117](next-release-backlog.md) 「外部ツール連携を設定画面へ出し、引数・複数選択へ拡張する」。
利用者判断 (2026-08-25) により **段階 1〜3 を一度に出す**方針が確定している。本書はその仕様を決めるための
調査結果と設計案をまとめる。作業ブランチ = `external-tool-launch` / worktree = `C:\home\mimageviewer-extlaunch`。

関連: [architecture-overview.md](architecture-overview.md) / [virtual-folders.md](virtual-folders.md) /
[ui-responsiveness.md](ui-responsiveness.md) / [keymap-spec.md](keymap-spec.md) /
[video-architecture.md](video-architecture.md)

---

## 1. 何を作るのか

いま mIV が持っているのは「右クリック →『アプリケーションで開く…』→ 関連付けアプリか登録 EXE を選ぶ →
**実ファイル 1 件をパス 1 個だけ渡して起動**」だけである。ここを次まで広げる。

1. 環境設定に外部ツールの管理画面を出す (発見できる場所に置く)
2. 引数テンプレート・作業フォルダー・複数選択に対応する
3. **ZIP / PDF / 変換アーカイブ内のページ**と**動画の特定フレーム**を一時実体化して渡せるようにする

---

## 2. 他アプリの調査

### 2.1 NeeView (v42 系、C#/WPF、MIT) — 事実上の参照実装

ソースを直接読んで確認した (`neelabo/NeeView` master, 2026-08-28 時点)。

**データモデル** — `NeeView/External/ExternalApp.cs`

| フィールド | 意味 | 既定 |
| --- | --- | --- |
| `Name` | 表示名。空なら exe のファイル名 (拡張子なし) | `null` |
| `Command` | 実行ファイル。**空なら「関連付けで開く」** (パス自体を `ExternalProcess.Start` へ渡す) | `null` |
| `Parameter` | 引数テンプレート | `"{File}"` |
| `WorkingDirectory` | 作業フォルダー | `null` |
| `ArchivePolicy` | 圧縮ファイル内ページの渡し方 (下記) | `SendExtractFile` |

**`ArchivePolicy`** — `NeeView/External/ArchivePolicy.cs` の 4 値。設定 UI には各値の例文字列が表示される。

| 値 | 渡すもの | 例 |
| --- | --- | --- |
| `None` | 何もしない (起動しない) | `not run.` |
| `SendArchiveFile` | 書庫ファイル自体 | `C:\Archive.zip` |
| `SendArchivePath` | 書庫内仮想パス (ver 33.0 で追加) | `C:\Archive.zip\File.jpg` |
| `SendExtractFile` | **一時フォルダーへ展開した実ファイル** | `ExtractToTempFolder\File.jpg` |

**`MultiPagePolicy`** — `Once` (1 ページのみ) / `All` (全ページ) / `AllLeftToRight` (全ページ・左右順)。
見開き表示のときに 2 ページを読み順で渡せる。

**プレースホルダ**

- `Parameter` 内: `{File}` = 渡すファイルパス、`{Uri}` = URI エスケープしたパス。
  旧記法 `$File` / `$Uri` も同義で受ける (`ExternalAppUtility.ReplaceKeyword`)。
- `Command` 内: `{NeeView}` (旧 `$NeeView`) = **NeeView 自身の exe パス**。
  自分自身を起動する場合は spawn 前に `SaveDataSync.Current.SaveAll(true)` で設定を保存する。
- **キーワードを 1 つも含まない `Parameter` には自動で `"{File}"` を追記する** (`ValidateApplicationParam`)。
  「引数を書き換えたらファイルが渡らなくなった」事故を防いでいる。

**起動**

- 置換後の 1 本の文字列を `ExternalProcess.Start(command, param, options)` へ渡す。シェル (`cmd /c`) は経由しない。
- 複数対象は `paths.Distinct()` を回して **1 パスにつき 1 プロセス**。「まとめて 1 プロセスへ」は無い。
- 失敗は握り潰さず `MessageDialog` で理由を出す。

**一時ファイル** — `NeeView/System/Temporary.cs`

- 展開先は `<TempRoot>\NeeView.Temp<PID>\Cache\...`。`TempRoot` は設定で変更でき、存在しなければ既定へ戻して通知する。
- ファイル名は既定で**元のエントリ名を維持**する (`TempFileNamePolicy(isKeepFileName: true, "entry")`)。
  外部アプリのタイトルバーに元のファイル名が出る。
- 削除はアプリ終了時 (`RemoveTempFolder`)。**自分が最後の NeeView プロセスなら `NeeView.Temp*` を全部消す**ので、
  異常終了で残った孤児も次の機会に回収される。
- 各ページは `ArchiveEntry._fileProxy` にキャッシュされ、同じページを何度渡しても展開は 1 回。

**コマンドとキー割り当て**

- コマンドは 3 つ。「外部アプリで開く (シンプル)」= コマンド自身のパラメータで起動 /
  「外部アプリで開く」= 登録リストから選ぶ / 「ブックを外部アプリで開く」= 現在のブック (書庫・フォルダ) を渡す。
- 「外部アプリで開く」はコマンドパラメータ `Index` を持ち、**0 = 選択メニューを出す、1..N = N 番目の登録ツール**。
- 登録数が可変でもコマンド表が増えないのは、NeeView に**コマンドの複製** (`OpenExternalAppAs:2` のように
  `名前:番号`) があるため (`CommandNameSource.cs`)。複製ごとに別パラメータ・別キー割り当てを持てる。

**周辺**

- 「ファイルをコピー」等も同じ実体化を通る。全体設定 `System.ArchiveCopyPolicy` (既定 `SendExtractFile`) が既定ポリシー。
  ただし `SendArchivePath` は実体が無いので、コピー時は `LimitedRealization()` で `SendExtractFile` に落とされる。
- `SystemConfig` には汎用リストとは別に**役割別の外部アプリ** (`TextEditor` / `WebBrowser` / `FileManager`) がある。
- 動画は「1 ページのブック」(`MediaArchive`) として扱われ、外部アプリには**動画ファイル自体**が渡る。
  フレームを切り出して渡す機能は無い。

### 2.2 ZipPla — 正規表現で起動条件と引数を作る流派

公式 Tips (Google Sites。現在は要ログインのため Internet Archive 2020-10-07 版) と
`himamon/ZipPlaFork` の `readme_original.txt` (更新履歴) で確認した。

- 「詳細設定 > 起動プログラム」は**表**で 1 行 1 ツール。列は
  「名前」「外部プログラム (パス)」「起動オプション」「クリックで起動」「右クリックメニュー」「複数」。
- **対象の指定が強力**。`\` (フォルダ)、`*` (全て)、拡張子の列挙、または
  **JavaScript 形式の正規表現リテラル**でフルパスを判定する。「クリックで起動」と「右クリックメニュー」が
  別列なので、「クリックでは内蔵ビューア、右クリックからは別アプリ」といった割り当てができる。
- 書庫 / PDF 内の項目は**フルパスに `/` を含む仮想パス** (`C:\book.zip/dir/page01.jpg`) として表現され、
  正規表現で判定・分解できる。既定設定では書庫 / PDF 内の画像は内蔵ビューアへ向けてある。
- **引数**は既定で「起動オプション列の文字列」+「選択されたファイルのフルパス」。ただし
  ①「複数」がオフ ② 起動条件の正規表現がキャプチャグループを含む ③ 起動オプションがグループ置換で変化する —
  の 3 条件が揃うと、**置換後の文字列だけが引数になる** (ファイルパスは自分で書く)。
  公式例は「PDF 内の指定ページを Acrobat で直接開く」(`/A page=N` を組み立てる)。
- 特殊変数: 正規表現の名前付きグループのほか、`${sort}` (現在のソート順を表す固定文字列)、
  `${hitem}` (**内蔵ビューアで最後に開いていた書庫内ページ**を `/folder/page05.jpg` の形で返す)。
  → 「同じ位置で別ビューアへ引き継ぐ」ための変数を持っているのが特徴。
- 複数選択は「複数」列で有効化。**クリックしたファイルが先頭**に来るよう並べ替える。
- リストには外部 EXE だけでなく**組み込み疑似コマンド**も混ざる (「内蔵ビューア」「現在の ZipPla」
  「関連付けプログラム」「エクスプローラー」「選択項目の移動／抽出」「クリップボードの実行ファイル」)。別名も付く。
  書庫 / PDF 内の画像を実ファイルとして取り出すのは「選択項目の移動／抽出」の役目であり、
  **外部プログラムへ渡すときに自動展開する仕組みではない**。
- 名前に `&A` のようなアクセスキーを含めるとキーから起動できる。
- 「外部プログラムでファイルを開いた後、それが終了するまでそのファイルの先読みを行わない
  (ペイント等、編集を伴う外部プログラムの利用を想定)」という配慮が入っている。

### 2.3 その他

| アプリ | 登録場所 | 引数 | 書庫内ページ | 複数選択 |
| --- | --- | --- | --- | --- |
| IrfanView | Properties > Misc に最大 10 個。`[CustomEditors]` で拡張子別も可 | `"%1"` | 非対応 | 「全ファイル名を 1 回で渡す」オプションあり。他に「短いファイル名 (8.3) で渡す」互換オプション |
| XnView MP | Open with > Configure Programs (`xnview.ini` の `[OpenWith]`) | `%1` / バッチ併用で `%*` | 記載なし | 選択分をまとめて渡せる |
| Honeyview | 「編集」ボタンの既定アプリを 1 つ指定 | 固定 | 書庫内画像は temp へ展開。**展開物が temp に残るという報告がある** | 非対応 |
| Eagle | Open with (外部エディタ) | 固定 | — | — |

- Eagle は**外部エディタで保存し直してもサムネイルが自動更新されない**ことが公式 FAQ になっている。
  外部編集からの戻り (round-trip) は、この分野で共通して弱い。
- 動画側は主要プレイヤー (MPC-HC / PotPlayer / VLC / mpv) がいずれも「現在フレームを画像として保存 /
  クリップボードへコピー」までは持つが、**フレームを外部プログラムへ引数として渡す導線は持たない**。
  ZipPla が動画サムネイル一覧から「キャプチャ画像をコピー」を出しているのが近い程度。
  → mIV の「動画フレームを外部ツールへ渡す」は既存アプリに前例が少ない。

### 2.4 「仮想パスを渡す」は成立するのか — 実測

NeeView の `SendArchivePath` と ZipPla の仮想パスを mIV でも採るべきかを判断するため、
この開発機で ZIP を作り、同一文字列を 2 系統で解決させた。

```
Win32 Test-Path      : False        <- C:\...\t.zip\a.txt は「存在しない」
Win32 File::Exists   : False
Shell ParseName.Path : C:\...\t.zip\a.txt   <- シェル名前空間では解決する
Shell item type      : テキスト ドキュメント
```

- **Win32 のファイル API (`CreateFile` / `fopen` / Rust の `File::open`) では存在しない。**
  一般のアプリ (画像編集ツールの大半) はこれで開けない。
- **Windows のシェル名前空間 (Compressed Folders) では解決する。** エクスプローラーや
  `IShellItem` / `SHParseDisplayName` 経由で開くアプリだけが辿れる。ただし **ZIP 限定**で、
  RAR / 7z / **PDF ページには原理的に効かない** (シェル名前空間が無い)。
- NeeView が自分の仮想パスを読めるのは標準規約があるからではなく、**パスを先頭から 1 要素ずつ足して
  実ファイルが見つかった所で切り、残りをエントリ名として扱う独自実装**を持っているため
  (`ArchiveEntryUtility.CreateAsync`)。つまり「自分自身へ引き継ぐ」ための機能である。
- しかも**業界標準が無い**。NeeView は区切りが `\`、ZipPla は `/` で互換が無い。

**結論: mIV は「仮想パスをそのまま渡す」方針を持たない。** 代わりに §4.4 の分解プレースホルダ
(`{container}` / `{entry}` / `{page}`) を用意し、アプリ固有の書式 (Acrobat の `/A page=N`、
SumatraPDF の `-page N` など) をユーザーが組み立てられるようにする。
mIV 自身への引き継ぎは行わない (§6 の決定済み 3)。mIV は単一インスタンスなので、
自分自身を起動しても既存ウィンドウが前面に来るだけで意味が無い。

> 補足: **現在の mIV の CLI は仮想パスを受け取れない。** [`resolve_openable_path_detailed`](../src/folder_tree.rs:793)
> は開けるパスが見つかるまで親を遡るので、`C:\book.zip\page01.jpg` を渡すと `C:\book.zip` はファイルなので
> 飛ばされ、最終的に `C:\` が開く。自己引き継ぎを実装するならここも直す必要がある。

---

## 3. mIV の現状 (コードベース調査、2026-08-29)

Codex Sol による read-only 調査。file:line は調査時点の master (646487cf) のもの。

### 3.1 現行の外部起動経路

| 経路 | 実装 | 渡せるもの |
| --- | --- | --- |
| 右クリック「アプリケーションで開く…」 | [open_with.rs:141](../src/open_with.rs:141) / [context_menu.rs:2037](../src/ui_dialogs/context_menu.rs:2037) | **実ファイル 1 件のパスのみ** |
| 動画 <kbd>Shift+Enter</kbd> | `GridOpenExternalPlayer` / `VideoExternalPlayer` ([keymap.rs:1479](../src/keymap.rs:1479), [keymap.rs:1688](../src/keymap.rs:1688)) → [ui_helpers.rs:1613](../src/ui_helpers.rs:1613) | OS 既定アプリで動画 1 本 |
| ネイティブコンテキストメニュー | [native_context_menu.rs:925](../src/native_context_menu.rs:925) | Shell の `IContextMenu` (「プログラムから開く」の有無は OS 依存) |
| エクスプローラーで表示 / URL を開く / ごみ箱 / キャプチャ先を開く | [context_menu.rs:2836](../src/ui_dialogs/context_menu.rs:2836), [external_links.rs:1](../src/external_links.rs:1), [capture.rs:228](../src/capture.rs:228) | — |
| コピー / 移動用 PowerShell | `%TEMP%\miv_ps_<pid>_<seq>.ps1` を作り `-File` で実行、完了後に削除 [context_menu.rs:2664](../src/ui_dialogs/context_menu.rs:2664) | — |

**GridItem 種別ごとの現状** ([grid_item.rs:270](../src/grid_item.rs:270) の `file_operation_path()` /
`drag_source_path()` が境界):

| 種別 | ネイティブ Shell メニュー | mIV 独自「アプリケーションで開く…」 |
| --- | --- | --- |
| Image / Video / ZipFile / PdfFile | 対象 | フォールバック時に出る |
| Convertible (RAR/7z/LZH) | 対象 | **無い** |
| Folder | 単一選択なら対象 | **無い** |
| ZipImage / PdfPage | **対象外** (物理パスを返さない) | **無い** |
| Stack | 対象外 (チェックも不可) | **無い** |

**発見性の問題の正体**: グリッド右クリックは**ネイティブコンテキストメニューが既定 ON**
([settings.rs:3904](../src/settings.rs:3904))。ただしネイティブメニューでも mIV は**自分の項目を先頭に
差し込んでいる** (`miv_items` を `AppendMenuW` で積み、セパレータを挟んで Shell 項目を続ける。
[native_context_menu.rs:387](../src/native_context_menu.rs:387))。
差し込めるのは `NativeMivCommand` の固定 enum に載っているものだけで、
**「アプリケーションで開く…」はそこに入っていない** ([native_context_menu.rs:38](../src/native_context_menu.rs:38))。
つまり独自の open-with は、ネイティブメニューを出せなかったときのフォールバックにしか現れない。
外部 SNS で「外部ツール設定が無い」と言われた背景はこれで説明が付く。
**差し込み口そのものは既にあるので、そこへ外部ツールを載せればよい** (§4.9)。

**P1 着手前に確認した既知の弱点** ([open_with.rs](../src/open_with.rs)):

- Windows でもパスを `to_string_lossy()` で `String` 化していた (P1 で `OsStr` のまま渡すよう修正済み)
- `spawn()` の `Result` を捨てていた (P1 で worker の結果を toast 通知するよう修正済み)
- 関連付けアプリの列挙 (`SHAssocEnumHandlers`) をメニュー描画の UI 経路から同期実行している
  ([context_menu.rs:2087](../src/ui_dialogs/context_menu.rs:2087))。P1 の環境設定ページは worker 化済みで、
  旧コンテキストメニュー側の載せ替えは P1b で行う

### 3.2 一時実体化に使える既存部品

| 対象 | 既存 API | 非同期 / キャンセル |
| --- | --- | --- |
| ZIP エントリ | [`zip_loader::read_entry_bytes()`](../src/zip_loader.rs:646) — 実 ZIP / ネスト / 変換済みフラットキャッシュを解決し `Vec<u8>` を返す | **同期。キャンセル無し。**呼び出し側でワーカー化が必須 |
| PDF ページ | [`render_page_async()`](../src/pdf_loader.rs:4305) / [`render_page()`](../src/pdf_loader.rs:4017)、`PdfWorkerPool` | ワーカー + 50ms 単位のキャンセル監視 ([pdf_loader.rs:2600](../src/pdf_loader.rs:2600)) |
| 変換済みアーカイブ | [`archive_cache`](../src/archive_cache.rs:50) が `data_dir/archive_cache/<hash 2桁>/<hash>/<basename>.zip` を返す。以後は通常 ZIP として扱える | lookup は同期 (メタデータ検証のみ) |
| 動画フレーム | [`video::screenshot::capture_frame()`](../src/video/screenshot.rs:1) — 別入力を開き、指定時刻付近のフレームを RGBA で返す | ワーカー前提。既に Ctrl+S から使われている |

**注意 (PDF)**: 「原寸」は存在しない。ベクター PDF には固有の元解像度が無く、
`render_page` は viewport とヘッドルームから寸法を決める (最低長辺 4096 / 上限 8192,
[pdf_loader.rs:4299](../src/pdf_loader.rs:4299))。製本は長辺 4096 を基準にしている
([books.rs:1047](../src/books.rs:1047))。**外部ツールへ渡すときの解像度は明示的に決める必要がある。**

**先例**: 仮想ページを実ファイルへ書き出す処理は既に 2 つある。

- 製本 ([books.rs:201](../src/books.rs:201)) — 通常ファイル / ZIP エントリ / 合成画像 / **動画フレーム** / PDF ページを
  表現でき、ZIP エントリは**元バイト列のまま**書き出す ([books.rs:916](../src/books.rs:916))
- エクスポート ([export_dialog.rs:129](../src/export_dialog.rs:129)) — `File` / `ZipEntry` / `PdfPage` /
  **`RenderedSpread` (見開き合成)** を扱い、ワーカーでキャンセル可能

どちらも「利用者が保持する最終成果物」を作るもので、**外部プロセスへ手渡すための temp という概念は無い**。

**ドラッグアウト / OS ファイルクリップボード**は仮想ページを扱えない。
コピーは仮想項目が 1 件でもあると選択全体を拒否し ([app.rs:36412](../src/app.rs:36412))、
ドラッグアウトは物理項目だけを抽出する ([ui_main.rs:1615](../src/ui_main.rs:1615))。
→ 本機能で一時実体化を作ると、**この 2 つも将来同じ仕組みに乗せられる**。

### 3.3 動画フレームの取り出し

- **「現在フレームを PNG 保存」「クリップボードへコピー」は既にある**
  ([ui_fullscreen.rs:32559](../src/ui_fullscreen.rs:32559), 既定 <kbd>Ctrl+S</kbd> [keymap.rs:5592](../src/keymap.rs:5592))。
  保存はワーカー上で `capture_frame()` → [`capture::save_rgba_unique()`](../src/capture.rs:313)。
- 対象時刻は `last_displayed_pts`、無ければ再生 clock の位置 ([video/mod.rs:9047](../src/video/mod.rs:9047))。
- **native presenter からのフルフレーム RGBA readback は存在しない** (診断用 1×1 staging のみ,
  [render_core.rs:6321](../src/video/render_core.rs:6321))。ただし
  **現在位置を使って別デコーダーから撮り直す**既存経路があるので、GPU readback の新設は不要。
- **フレーム番号を指定して必ずそのフレームを返す API は無い。** 既存はすべて PTS / 時刻ベースで、
  キーフレームから preroll して近傍を選ぶ ([screenshot.rs:30](../src/video/screenshot.rs:30) は最大 240 フレーム走査)。

→ 外部ツールへ渡す最短経路は「現在 PTS で再デコード → RGBA → 管理対象 temp へ encode」。
**新規に必要なのは寿命管理だけ**で、フレーム取得そのものは既にある。

### 3.4 選択と見開き

- グリッドは `selected: Option<usize>` + `checked: HashSet<usize>` ([app.rs:9507](../src/app.rs:9507))。
  detached viewer では両方が viewer context bundle に含まれ、切り替え時にまとめて swap される
  ([viewer_context_registry.rs:704](../src/app/viewer_context_registry.rs:704))。
- 仮想項目の既存の扱いは操作ごとにバラバラ: コピーは**全体を拒否**、ドラッグアウトは**物理だけ抽出**、
  削除は**物理だけ対象**、レーティングは**仮想ページ単位で可**、タグはグリッド上の仮想項目が**対象外**。
  → 外部ツールの複数選択ポリシーは、この中のどれに寄せるかを明示的に決める必要がある。
- 見開きは `SpreadPair::Single` / `Double { left, right }` という型で表現済み
  ([ui_fullscreen.rs:4738](../src/ui_fullscreen.rs:4738))。現在見えている 1〜2 項目を解決する処理もあり
  ([ui_fullscreen.rs:18247](../src/ui_fullscreen.rs:18247))、クリップボードとエクスポートは既に 2 ページを扱える。

### 3.5 設定の保存先

- 単純設定は JSON key-value、**可変長リストは専用テーブル**へ分離する方針 ([settings_db.rs:1](../src/settings_db.rs:1))。
- **既に `recent_open_with_apps` / `custom_open_with_apps` が complex field として存在**し、
  `RecentApp { display_name, exe_path }` の `Vec` を `sort_index` 付きで保持している
  ([settings.rs:3154](../src/settings.rs:3154), [settings_db.rs:1436](../src/settings_db.rs:1436))。
  → **これがリリース済みデータ。移行コードが必須** (CLAUDE.md「永続データ・スキーマ変更時の判断」)。
- 他の可変長リスト例: `FavoriteEntry` / `SmartFolderDefinition` ([settings.rs:120](../src/settings.rs:120))。
- 未知 variant / 未知 field でのデシリアライズ失敗は `Incompatible` 扱い ([settings_db.rs:2195](../src/settings_db.rs:2195))。
  **complex field への登録を忘れると単純 JSON 保存から漏れてデータを失う**警告がコードにある
  ([settings_db.rs:52](../src/settings_db.rs:52))。
- 環境設定 UI に「追加・改名・並べ替え・削除」を 1 ページで全部持つ既存例は無い。近いのは
  VST ページ (追加/上下移動/削除, [pages.rs:6521](../src/ui_dialogs/preferences/pages.rs:6521))、
  Creative LUT ページ (追加/表示名編集/登録解除, [pages.rs:6358](../src/ui_dialogs/preferences/pages.rs:6358))、
  お気に入り編集 (名前編集/並べ替え/削除 + 即時保存, [favorites_editor.rs:128](../src/ui_dialogs/favorites_editor.rs:128))。

### 3.6 キー割り当て

- `KeyAction` は**固定 enum** で、`ALL_ACTIONS` も固定 slice ([keymap.rs:1358](../src/keymap.rs:1358))。
  keymap は `HashMap<KeyAction, Vec<Chord>>` ([keymap.rs:5680](../src/keymap.rs:5680))。
  **実行時に個数が変わる action をそのまま登録することはできない。**
- ただし**固定スロット方式の先例が既に 2 つある**: お気に入り用 20 個 ([keymap.rs:2963](../src/keymap.rs:2963))、
  ピン留めタグ用 20 個 ([keymap.rs:3040](../src/keymap.rs:3040))。slot 番号 → 固定 action の helper もある
  ([keymap.rs:3225](../src/keymap.rs:3225))。
- 動的**メニュー**は既存パターンで問題ない (お気に入り / タグ / 現行 open-with サブメニューが実装済み)。

### 3.7 制約 (設計に効くもの top 5)

1. `ZipImage` / `PdfPage` / `Stack` は物理パスを持たず、Shell メニュー・ファイルコピー・ドラッグアウトへ直接渡せない
2. ZIP / PDF / 動画の実体化はすべて同期 I/O + デコードを伴うので、**キャンセル可能なワーカージョブ**として設計する
3. 見開きは 2 項目を表現できるが、「2 引数」か「合成 1 ファイル」かは新たに決める必要がある
4. keymap が固定 enum なので、任意個のツールに個別キーを振るには**固定スロット方式**を採る
5. **外部 GUI アプリへ渡した temp の寿命管理方針が存在しない。**
   現行 open-with は `Child` を保持せず捨てている ([open_with.rs:141](../src/open_with.rs:141))

---

## 4. 仕様案

### 4.1 データモデル

```rust
pub struct ExternalTool {
    pub id: ExternalToolId,          // 安定 ID (並べ替えやキー割り当てと独立)
    pub name: String,                // 表示名。空なら exe の file_stem
    pub launch: ExternalToolLaunch,  // 何として起動するか (下記)
    pub arguments: String,           // 引数テンプレート (既定 "{file}")
    pub working_directory: Option<PathBuf>,
    pub payload: PayloadPolicy,      // 何を渡すか (§4.3)
    pub video: VideoPolicy,          // 動画のときに何を渡すか (§4.3)
    pub spread: SpreadPolicy,        // 見開き表示中の扱い (§4.5)
    pub selection: SelectionPolicy,  // 複数選択の扱い (§4.5)
    pub pdf_render_long_edge: u32,   // PDF レンダリング長辺 (既定 4096、§4.3 / §6)
    pub for_editing: bool,           // 編集用。round-trip 監視の対象 (§4.8)
    pub show_in_context_menu: bool,  // 右クリックメニューに出す (§4.9)
    pub keep_temp: bool,             // 起動後に temp を残すか (既定 false)
}
```

```rust
pub enum ExternalToolLaunch {
    Executable(PathBuf),                // 自分で spawn する。引数テンプレートが効く
    Association { handler_id: String },  // シェルに起動させる。引数は使えない
    OsDefault,                          // OS の既定アプリで開く。引数は使えない
}
```

登録リストに関連付け起動も混ぜられるようにするのは NeeView と同じ。これで
`recent_open_with_apps` / `custom_open_with_apps` の 2 本立てを 1 本に畳める。

**`Association` を `Executable(パス)` で代用してはならない (2026-08-30、実機で踏んだ)。**
関連付けアプリの実行ファイルパスを `IAssocHandler::GetName()` から取っていたが、この API が
実行ファイルのパスを返すのは素の Win32 アプリのときだけで、**UWP / Store アプリでは ProgID や
パッケージ識別子が返る**。利用者の環境では「フォト」「TsubameViewer」の 2 件が文字列
`フォト` / `TsubameViewer` として保存され、「ペイント」は `C:\Program Files\WindowsApps\...` という
ACL 保護下の到達できないパスとして保存されていた。3 件とも `Command::new` では起動できない。
関連付けは**パスを取り出さずシェルに起動させる** (`IAssocHandler::Invoke` に対象ファイルの
`IDataObject` を渡す) こと。ハンドラは起動時に拡張子から再列挙し、`handler_id` と
`GetName()` の一致で引き当てる (COM ポインタは永続化できない)。

**`GetName()` は安定した識別子ではない (2026-08-30、実機 2 例目)。** Store アプリが更新されると
`GetName()` が返すパスの**バージョン部分が変わる**。利用者の環境では「ペイント」が
`Microsoft.Paint_11.2603.251.0_x64__8wekyb3d8bbwe` で保存されていたが、実機の現況は
`Microsoft.Paint_11.2605.81.0_x64__8wekyb3d8bbwe` で、完全一致では引き当てられなかった
(「フォト」「TsubameViewer」は `GetName()` がバージョンを含まない名前を返すため一致した)。

引き当ては次の順で行い、**最初に一致したものを使う**。

1. `GetName()` の完全一致
2. **パッケージ識別の一致**。`...\WindowsApps\<Name>_<Version>_<Arch>__<PublisherId>\<残り>` の形なら、
   バージョンとアーキテクチャを落とした `<Name>` + `<PublisherId>` + `<残り>` で比較する
   (`Name` と `PublisherId` は更新で変わらない)
3. `GetUIName()` (表示名) の一致

2 か 3 で一致したときは、**保存してある `handler_id` を現在の `GetName()` で書き戻す**。
次回から 1 で当たるようになり、Store 更新のたびに劣化しない。どれにも一致しなければ、
ツール名を添えて「関連付けアプリが見つかりません」と通知する (黙って何もしない、はしない)。

`Association` と `OsDefault` では**引数テンプレートと作業フォルダーが効かない**。
起動を決めるのはシェルなので、設定 UI 側でも入力を無効化する。`payload` / `spread` などの
「何を渡すか」のポリシーはどの起動型でも意味があるので残す。

`for_editing` は round-trip 監視 (§4.8) の対象を決めるだけでなく、**追加ダイアログの既定値を出し分ける**
のに使う。「編集に使う」を選んだツールは `payload = Original` / `spread = MainPageOnly` を既定にする。
そうしないと、画像編集ソフトへ焼き込み済みの一時 PNG を渡してしまい、
**編集しても元ファイルに反映されない**という分かりにくい結果になる (§4.3 / §4.8)。

### 4.2 対象の解決 (どの項目を渡すか)

起動元ごとに対象集合を作り、そこから先は共通経路にする。

| 起動元 | 対象 |
| --- | --- |
| グリッド右クリック | `checked` が空でなければ `checked`、空なら右クリックした項目 |
| グリッドのキー操作 | 同上 (`checked` 優先、無ければ `selected`) |
| フルスクリーン / 連結読み | 現在ページ。見開き中は `SpreadPolicy` に従って 1〜2 件 |
| 動画・音声再生中 | 再生中の動画 1 本 (+ `VideoPolicy`) |
| コンテナー対象の起動 | 現在のフォルダー / ZIP / PDF 本体 1 件 |

`checked` 優先は既存の一括レーティングと同じ規則 ([app.rs:49071](../src/app.rs:49071)) に揃える。
最後の行は NeeView の「ブックを外部アプリで開く」に相当する。同じツール定義を「ページに対して」
「本に対して」の 2 通りで起動できるようにし、別ツールとして登録させない。


**連結読み中はどうするか (2026-08-29 調査)**

既存ショートカットの扱いを実際に確認したところ、連結読みでの可否は 2 つに割れていた。

| 連結読みで**止まる** | 連結読みでも**動く** |
| --- | --- |
| 画像分析 (Z) / ズーム / パノラマ / 比較 / 消しゴム / ローカル調整 / 隠蔽 / テキスト注釈 / **エクスポート (Ctrl+E)** / 範囲キャプチャ | **キャプチャ保存 (Ctrl+S)** / **製本追加 (Ctrl+B)** / 回転 / スライドショー |

止まる側は `FsContinuousReadingUnavailableFeature` の 10 値として列挙され、
`show_continuous_reading_shortcut_noop()` が通知を出して処理を飛ばす
([ui_fullscreen.rs:4653](../src/ui_fullscreen.rs:4653), [ui_fullscreen.rs:20846](../src/ui_fullscreen.rs:20846))。
一方 Ctrl+S ([ui_fullscreen.rs:20840](../src/ui_fullscreen.rs:20840)) と
Ctrl+B ([ui_fullscreen.rs:33496](../src/ui_fullscreen.rs:33496)) には呼び出し側にも関数内にも
連結読みのガードが無い。

この分かれ方には一貫した理由がある。

- 止まるのは、**モードに入る操作**と、**「画面に出ている表示結果そのもの」を対象にして
  ダイアログを開く操作** (エクスポート・範囲キャプチャ)。連結読みでは「表示結果」が
  複数ページにまたがってスクロールするので、対象が定義できない。
- 動くのは、**現在ページ (`fs_idx`) に対してその場で完了する操作**。
  対象が 1 ページに定まるので連結読みでも成立する。

**外部ツール起動は後者に属する** (現在ページを対象にし、モードにも入らず、ダイアログも開かない)。
特に **Ctrl+B の製本追加は「現在ページを実体化して外へ出す」という点で本機能とほぼ同じ**で、
これが連結読みで動く以上、外部ツールだけ止めると逆に不統一になる。

したがって**連結読みでも起動できるようにする**。ページの選び方は Ctrl+S / Ctrl+E と同じ
[`resolve_visible_spread_pair()`](../src/ui_fullscreen.rs:18268) を共有し、判断を二重に持たない。
連結読みでは `FullscreenPageLayout` の種別が `Continuous` になり `spread_pair()` が `None` を返すため
([displayed_image_transform.rs:529](../src/displayed_image_transform.rs:529))、
`resolve_spread_pair()` のペアリング規則へフォールバックする。つまり
**見開き設定が ON なら連結読みでも 2 ページが 1 組として渡る**。これは Ctrl+S の現在の挙動と同じ。

### 4.3 何を渡すか (実体化ポリシー)

**利用者判断 (2026-08-29): 既定は「利用者に見えているものをそのまま渡す」。**
§2.4 の結論により**仮想パスは選択肢に入れない**。

`PayloadPolicy`

| 値 | 動作 |
| --- | --- |
| `AsDisplayed` (既定) | 表示どおりに渡す。ただし**見た目が変わらないなら元ファイルをそのまま渡す** |
| `Original` | 常に元データ (無劣化)。補正・回転は反映されない |
| `Container` | 書庫 / PDF / 動画**本体**を渡す (ページ位置は失われる) |
| `RealFileOnly` | 実ファイルのときだけ有効。仮想ページではメニューで無効表示にする |

`AsDisplayed` の「見た目が変わるか」の判定には、**既存の
[`books::page_requires_full_composite()`](../src/books.rs:305) をそのまま使う**。
補正 / 回転 / 隠蔽 / 消しゴム / ローカル調整 / 注釈 / 切り取りのどれかが掛かっていれば焼き込み、
何も掛かっていなければ元ファイル (ZIP 内なら元バイト列を展開したもの) を渡す。

これは製本の「無加工なら再エンコードしない」と同じ判断で、次の実利がある。

- **無加工の JPEG が PNG に化けない。** EXIF / AI 生成メタデータ / 元のファイル名と拡張子が保たれる。
- 焼き込みコスト (デコード + 再エンコード) を、実際に加工されている項目にだけ払う。
- 画像編集ソフトへ渡したとき、**元ファイルを直接編集できる** (§4.8 の round-trip が成立する)。

見開き表示中は `SpreadPolicy::Merged` が既定 (§4.5) なので合成 1 枚になる。
合成には対応する元ファイルが存在しないため、この場合は常に焼き込みになる。

`VideoPolicy` — **動画は別軸にする。**「見えているもの」が動画そのものかフレームかが曖昧で、
再生中に別プレイヤーへ渡すつもりのツールが PNG を受け取ると困るため。

| 値 | 動作 |
| --- | --- |
| `File` (既定) | 動画ファイル自体を渡す (= 現在の Shift+Enter 相当) |
| `CurrentFrame` | **現在フレームを画像へ実体化して渡す。** 再生中でなければ `File` に落ちる |

`Stack` は代表項目ではなく束ねられた全ページを対象にし、`SelectionPolicy` に従う。

実体化の具体:

| 対象 | 出力 | 決めること |
| --- | --- | --- |
| 加工なしの実ファイル | そのまま渡す (コピーしない) | 無し |
| 加工なしの ZIP エントリ | **元バイト列をそのまま**書き出す (製本 [books.rs:916](../src/books.rs:916) と同じ) | 無し。無劣化 |
| 加工ありのページ | 焼き込んで PNG (`CompositeSource` + `BakedEditSnapshot` を再利用) | 無し (製本と同じ経路) |
| PDF ページ | PNG へレンダリング | 解像度は**ツールごとに選べる。既定は長辺 4096** (製本と同じ) |
| 動画フレーム | PNG (`capture_frame()` → encode) | 既定 PNG。品質はキャプチャ設定を流用 |
| 見開き合成 | `RenderedSpread` を PNG 化 (エクスポート [export_dialog.rs:129](../src/export_dialog.rs:129) を再利用) | 無し |

**AI アップスケール結果は焼き込まない** (利用者判断 2026-08-29)。表示中に AI 拡大が効いていても、
外部ツールへ渡すのは等倍。AI アップスケールは**自分の表示用**という位置づけで、製本と同じ扱いに揃える。
`page_requires_full_composite()` に AI が入っていないので、判定はそのまま使える。

> 例外は <kbd>Ctrl+E</kbd> (`FsExport`「現在の表示結果を別ファイルへ書き出す」,
> [keymap.rs:5441](../src/keymap.rs:5441))。これはダイアログを開いた瞬間の表示 pixels を
> スナップショットするので、AI 拡大が完了していれば反映される
> ([export_dialog.rs:226](../src/export_dialog.rs:226))。外部ツール起動はこれに揃えない。

### 4.4 プレースホルダと引数の組み立て

| 記法 | 展開されるもの |
| --- | --- |
| `{file}` | 実体化後の**実ファイルパス**。既定引数はこれ 1 個 |
| `{files}` | 対象全件 (`SelectionPolicy::Batch` のとき。1 件ずつ別引数に展開) |
| `{dir}` | `{file}` の親フォルダー |
| `{name}` / `{stem}` / `{ext}` | ファイル名 / 拡張子なし / 拡張子 (ドット無し) |
| `{container}` | 書庫 / PDF / 動画**本体**のパス。実ファイルならそれ自身 |
| `{entry}` | 書庫内エントリ名。仮想ページでなければ空 |
| `{page}` | ページ番号 (1 始まり)。PDF / 書庫内 / 本のときのみ |
| `{time}` / `{time_ms}` / `{time_hms}` | 動画の現在位置 (秒 / ミリ秒 / `00:01:23.456`) |
| `{uri}` | `{file}` を URI エスケープしたもの |

- **キーワードを 1 つも含まない引数テンプレートには `{file}` を自動で追加する** (NeeView と同じ事故防止)。
- **引数は文字列を組み立ててから渡すのではなく、先にトークン列へ分割してから各トークン内で置換する。**
  mIV は `Command` に `OsString` 引数列を渡す規約なので、この順序なら置換値に空白が含まれても壊れず、
  利用者が引用符を自分で書く必要も無い。分割規則は `CommandLineToArgvW` 互換とする。
- **空に展開されたトークンは引数列から取り除く。** `{page}` を持たない対象に `-page {page}` を書いても
  ゴミ引数を渡さない。`-page` だけが残らないよう、同一トークン内に空のプレースホルダが 1 つでもあれば
  そのトークンごと落とす。
- **自己引き継ぎ (`{miv}`) は用意しない** (利用者判断 2026-08-29)。mIV は単一インスタンスなので、
  自分自身を起動しても既存ウィンドウが前面に来るだけで意味が無い。この機能はあくまで別アプリ向けとする。
  これにより §2.4 で挙げた `resolve_openable_path_detailed` の修正も不要になる。

用例:

```
Acrobat で PDF の該当ページを直接開く:   /A "page={page}" "{container}"
SumatraPDF で同上:                       -page {page} "{container}"
動画をその位置から別プレイヤーで:         --start={time} "{container}"
```

### 4.5 複数選択と見開き

`SelectionPolicy`

| 値 | 動作 |
| --- | --- |
| `Single` (既定) | 対象が複数でも先頭 1 件だけ渡す |
| `Each` | 対象 1 件につき 1 プロセス起動 (NeeView 方式) |
| `Batch` | 全件を 1 プロセスへ渡す (`{files}`。IrfanView の「1 回で渡す」相当) |

- 順序はクリック / 現在項目を先頭にし、以降は一覧の表示順 (ZipPla と同じ配慮)。
- `Each` の起動数には上限を設け、超える場合は確認ダイアログを出す (既定 20)。
- 実体化に失敗した項目は飛ばし、**何件飛ばしたかを通知**する。黙って減らさない。

`SpreadPolicy` (連結読み・見開き表示中のみ)。**選べるようにし、既定は合成 1 枚**
(利用者判断 2026-08-29。「利用者に見えているものをそのまま渡す」が期待される動作なので)。

| 値 | 動作 |
| --- | --- |
| `Merged` (既定) | 合成した 1 枚を渡す (`RenderedSpread` を実体化) |
| `BothPages` | 左右 2 件を読み順で渡す (`SelectionPolicy` に従って Each / Batch) |
| `MainPageOnly` | 主ページ 1 件 |

- `PayloadPolicy::Original` と `Merged` は両立しない (合成物に元ファイルは無い)。
  `Merged` を選んだツールは見開き表示中だけ常に焼き込みになる。
- **編集用ツール (`for_editing`) の既定は `MainPageOnly`。** 見開きを合成して渡すと元ファイルを編集できない。

### 4.6 一時ファイルの寿命

**NeeView と同型の「プロセス単位ディレクトリ + 終了時削除 + 起動時の孤児回収」を採る。**
外部プロセスの終了を待つ方式は採らない。起動されたアプリが既存プロセスへ転送して即終了する場合を扱えないため。

- 置き場所: 通常版は `%TEMP%\mimageviewer\ext-<pid>\`、
  **ポータブル版は `<exe_dir>\data\temp\ext-<pid>\`** (利用者判断 2026-08-29。システム側を汚さない)。
  ポータブル版は data が書けなければ起動を拒否する既存規約なので、判定は増えない
- ファイル名は**元のエントリ名を維持**する。衝突時のみ連番を付ける。
  外部アプリのタイトルバーに元の名前が出るのは実用上重要。
- 同じ対象を再度渡すときは**再利用する** (mtime / サイズで検証)。NeeView の `_fileProxy` 相当。
- 削除は次の 3 つ:
  1. mIV 終了時に自分の `ext-<pid>` を削除
  2. 起動時、**他に mIV プロセスが居なければ** `ext-*` を全部掃除する (異常終了の孤児回収)
  3. `keep_temp` が立つツールで作ったものは 1 の対象外にし、次回起動時の 2 で回収する
- 掃除はワーカーで行う。UI スレッドでディレクトリ走査・削除をしない。

### 4.7 起動と失敗通知

- `Command::new(exe)` + `OsString` 引数列。**`cmd /c` は経由しない。**
- パスを `to_string_lossy()` せず `OsStr` のまま扱う (現行 [open_with.rs:143](../src/open_with.rs:143) の修正)。
- `CREATE_NO_WINDOW` は維持しつつ、`spawn()` の `Err` を**必ず**通知する。文面はツール名 + OS エラー。
  現行は捨てているので、これは既存不具合の修正でもある。
- 実体化・EXE 存在確認・ネットワークパス確認は**すべてワーカー**。UI スレッドで `is_file()` /
  `read_entry_bytes()` / `render_page()` を呼ばない ([ui-responsiveness.md](ui-responsiveness.md) §4)。
- 実体化中は進捗を出し、キャンセルできるようにする。PDF レンダリングや大きい ZIP エントリで待たされるため。

### 4.8 編集用ツールの扱いと round-trip

- **実ファイルを渡した場合**: ツール定義に「編集用」フラグを持たせ、立っているときだけ対象パスの
  mtime / サイズを監視し、変化したらサムネイル・テクスチャ・カタログを無効化して読み直す。
  Eagle の既知の弱点をここで潰す。
**編集用ツールに、元ファイルでないものを渡さない (利用者判断 2026-08-29)。**
temp を編集しても元の ZIP / PDF / 動画には戻らないので、黙って渡すと「編集したのに反映されない」
という最も分かりにくい失敗になる。書き戻し自体は実装しないので、入口で止める。

- `for_editing` が立つツールは、**渡すものが元ファイルそのものでない場合は起動せずエラー**にする。
  該当するのは ZIP / PDF 内ページ (一時展開)、動画フレーム、見開き合成、`AsDisplayed` での焼き込み。
- エラー文面は理由と代替手段まで書く。例:
  「圧縮ファイル内のページは編集用ツールで開けません。書き出してから編集してください
  (フルスクリーンで <kbd>Ctrl+E</kbd>)」。
- **判定は 2 段構えにする。** メニューの有効 / 無効は、項目種別と見開き状態という
  **同期で分かる情報だけ**で決める (UI スレッドで補正 DB を読まない)。
  焼き込みの有無まで含めた最終判定は、実体化ワーカーが**起動直前**に行い、
  元ファイルでなければ spawn せずエラーを返す。
- メニューでは**無効表示 (グレー) + 理由のツールチップ**にする。ここは §4.9 の
  「無効なツールは積まない」の例外にする。利用者は「このツールで編集できる」と思っているので、
  黙って消えるより理由が見えた方がよい。
- ZipPla の「外部プログラムが終了するまで先読みを停止」は、mIV では編集用フラグの立ったツールに
  実ファイルを渡している間だけ、その項目の再読み込みを抑制する形で検討する (P5)。

### 4.9 どこから起動するか

**利用者判断 (2026-08-29): ネイティブコンテキストメニューの mIV 差し込み領域に、
登録したツールのうち「右クリックに出す」と設定したものだけを足す。**

差し込み口は既にある (§3.1)。`NativeMivCommand` に **`ExternalTool(ExternalToolId)`** を足し、
`miv_items` を組み立てるところ ([context_menu.rs:1387](../src/ui_dialogs/context_menu.rs:1387) 付近) で
`show_in_context_menu` の立っているツールを順に積む。`NativeMivCommand` は `Copy` なので、
`ExternalToolId` を `Copy` な newtype にしておけば enum の性質は変わらない。

- **サブメニューにせず平坦に並べる。** 右クリック 1 回で届くのが速く、
  「設定した数だけ出る」という利用者の期待にも合う。
- 現在の対象に対して無効なツール (`RealFileOnly` のツールに仮想ページ、など) は積まない
  (右クリックが伸びるのを避ける)。**例外は編集用ツール**で、こちらは `MF_GRAYED` +
  理由のツールチップで出す (§4.8)。
- 既存の mIV 項目との間にセパレータを 1 本入れる。
- 差し込み数が増えすぎないよう、`show_in_context_menu` の既定は
  **新規登録時 ON、ただし ON の総数が一定を超えたら OFF で追加**する (目安 10)。

フォールバックメニュー (ネイティブが使えないとき) と、フルスクリーンの右クリックにも同じ一覧を出す。

その他の導線:

| 場所 | 内容 |
| --- | --- |
| メニューバー | 「ファイル」配下に「外部ツール ▸」 (全ツール。`show_in_context_menu` に関係なく出す) |
| ツールバー | 任意ツールをボタンとして置ける (お気に入り / タグと同じ動的項目パターン) |
| キーボード | §4.10 の固定スロット |

### 4.10 設定 UI とキー割り当て

環境設定 →「起動と連携」→「外部ツール」ページ (新規)。

- 一覧 (表) + 追加 / 編集 / **複製** / 上下移動 / 削除。複製は「引数違いの同じ EXE」を作るのに要る。
- 追加の入口は 2 つ: 「実行ファイルを選ぶ」「関連付けアプリから選ぶ」(現行の `SHAssocEnumHandlers` を流用)。
  列挙は**ワーカーへ移す** (現行は UI 同期)。
- 編集ダイアログにプレースホルダ一覧と、**現在の対象で組み立てた引数のプレビュー**を出す。
- 環境設定の検索索引にも登録する。

キー割り当ては**固定スロット方式**にする (お気に入り 20 / ピン留めタグ 20 と同型)。

- `ExternalToolPicker` — 選択メニューを出す (NeeView の `Index = 0` 相当)
- `ExternalTool1` .. `ExternalTool10` — N 番目のツールを直接起動
- `ExternalToolForContainer` — 現在のフォルダー / 本を渡す (ピッカー)

`KeyAction` を動的化しない理由は [keymap-spec.md](keymap-spec.md) の identity モデル
(action 名 = 設定保存名 = 既定キー = scope が固定 enum に結び付いている) を壊さないため。
スロット番号とツールの対応は並べ替えで変わるので、UI 上でスロット番号を明示する。

**既存データの移行 (必須)**

- `custom_open_with_apps: Vec<RecentApp>` は**リリース済み**なので、
  `RecentApp { display_name, exe_path }` → `ExternalTool` へ移行する。中身は利用者が自分で選んだ
  EXE なので `ExternalToolLaunch::Executable` になる。
- `recent_open_with_apps` は履歴なので `external_tools` へは移さず、「関連付けアプリから選ぶ」の
  候補として残す。**ただしこちらもリリース済みで、`exe_path` に起動できない値が入っている**
  (§4.1 の実機事例)。**テーブル内で一度だけ分類し直す**: 実在するファイルパスなら `Executable`、
  それ以外は `Association { handler_id: その文字列 }`。分類は純関数にしてテストし、
  移行は `schema_meta` の marker で一度きりにする。
- `external_tools` テーブルは**未出荷**なので、起動型を入れるためのスキーマ変更に
  マイグレーションは要らない (CLAUDE.md「永続データ・スキーマ変更時の判断」)。

### 4.11 安全性

- コマンド文字列を組み立ててシェルへ渡さない (§4.4 のトークン分割)。
- EXE パスがネットワーク上 (`\`) の場合は確認してから起動する。存在確認はワーカーで。
- 一時ディレクトリは mIV が作る。既存ディレクトリを再利用しない。展開先パスはエントリ名を sanitize して
  ディレクトリ外へ出さない (Zip Slip 対策)。製本の sanitized basename
  ([books.rs:1594](../src/books.rs:1594)) を流用する。
- 起動したプロセスの標準入出力は継承しない。

---

## 5. 段階分け

| Phase | 内容 | 規模 |
| --- | --- | --- |
| **P0 (実装済み 2026-08-29)** | `ExternalTool` 型 + 設定 DB (complex field) + 既存 `custom_open_with_apps` からの移行 + 移行テスト | S |
| **P1 (実装済み 2026-08-30)** | 環境設定「外部ツール」ページ、引数テンプレート、作業フォルダー、起動失敗通知、`OsStr` 化 | M |
| **P1b (実装済み 2026-08-30)** | ネイティブ / フォールバック右クリックへの差し込み、フォールバックの登録先を `external_tools` へ載せ替え、legacy 書き戻しの停止 | S |
| **P1c (実装済み 2026-08-30、実機確認待ち)** | 関連付けアプリをシェルに起動させる (`ExternalToolLaunch` 3 分岐、`IAssocHandler::Invoke`、`recent_open_with_apps` の分類移行)。実機で踏んだ不具合の修正 | M |
| **P1d (実装中 2026-08-30)** | 関連付けハンドラの引き当てを Store 更新に耐える形にする (パッケージ識別 / 表示名フォールバックと `handler_id` の書き戻し)。実機で踏んだ不具合の修正 | S |
| **P2** | 対象解決の共通化 (`checked` 優先 / コンテナー対象)、`SelectionPolicy`、ツールバー / キースロット | M |
| **P3** | 一時実体化基盤 (ワーカー + キャンセル + 寿命管理 + 孤児回収)、`PayloadPolicy`、**編集用ツールのガード** (§4.8) | L |
| **P4** | `VideoPolicy::CurrentFrame`、`SpreadPolicy::Merged` の合成、`{container}`/`{entry}`/`{page}`/`{time}` | M |
| **P5** | round-trip の残り (実ファイルの mtime 監視 + 再読み込み) | M |

利用者判断により P0〜P4 は一括で出す。P5 は分けてよい。
**P3 の実体化基盤は、将来ドラッグアウト / OS ファイルクリップボードの仮想ページ対応にも使える**ので、
最初から「外部ツール専用」ではなく汎用の materializer として切る。

---

## 6. 決定済み / 未決事項

### 決定済み (2026-08-29、利用者判断)

1. **見開きは選べるようにし、既定は合成 1 枚** (`SpreadPolicy::Merged`)。
   理由は「基本、利用者に見えているものをそのまま渡すのが期待される動作」。
   この原則は見開きに限らないので、`PayloadPolicy` の既定も `AsDisplayed` にした (§4.3)。
   ただし**見た目が変わらない項目は元ファイルをそのまま渡す**ことで、無加工 JPEG の再エンコードと
   round-trip の破綻を避ける。
2. **ネイティブコンテキストメニューの mIV 差し込み領域へ、設定したツールだけを足す** (§4.9)。
3. **自己引き継ぎ (`{miv}`) は作らない。** mIV は単一インスタンスなので意味が無く、
   この機能はあくまで別アプリ向けとする。`resolve_openable_path_detailed` の修正も不要になった。
4. **AI アップスケール結果は焼き込まない。** AI 拡大は自分の表示用という位置づけで、製本に揃える。
   <kbd>Ctrl+E</kbd> のエクスポートだけが例外 (表示 pixels をそのまま書き出す) で、外部ツールはそこに揃えない。
   要望が出たら、ツール登録時の設定として「AI アップスケールを反映する」を後から足せる
   (`PayloadPolicy` に値を増やすのではなく、独立したフラグにする方が影響が小さい)。
5. **PDF ページの解像度はツールごとに選べるようにし、既定は長辺 4096** (製本と同じ)。
6. **ポータブル版の temp は `<exe_dir>\data\temp\`。** システム側を汚さないため。
7. **元 ZIP への書き戻しは実装しない。** 代わりに、**編集用ツールに元ファイルでないものを渡そうとしたら
   起動せずエラー**にする (§4.8)。黙って一時ファイルを渡して「編集が反映されない」と思わせない。

### 未決

無し。実装に着手できる状態。実装中に判断が要るものが出たらここへ追記する。

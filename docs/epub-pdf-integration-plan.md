# EPUB → PDF 変換の本体組み込み設計 (2026-09-25、レビュー前)

- 経緯と検証: `docs/backlog-on-hold.md` §1.247 (計画)、`docs/epub-pdf-spike-results.md` (スパイク結果)。
- 変換器: `crates/epub-pdf-worker` (単体 CLI `mimageviewer-epub-pdf`)。
- 状態: **設計案。実装前に独立レビューを受ける** (所有境界・非同期・子プロセスを含むため)。
- 行番号は `epub-pdf` ブランチ (master `a887ba43a` 起点) のもの。

## 1. 決定事項 (利用者合意 2026-09-25)

| # | 決定 |
| --- | --- |
| D1 | DRM のない EPUB を固定レイアウトの PDF へ変換し、既存の PDF 経路で読む。汎用 EPUB ビューアにはしない |
| D2 | 変換結果は APPDATA の変換キャッシュ (`archive_cache`) に置く (案 A)。元ファイルの隣には書かない。NAS / 読み取り専用メディアでも動くこと |
| D3 | ページ単位の利用者データ (補正・回転・レーティング・タグ・注釈・消しゴム/モザイク・読書位置・しおり・コレクション・代表サムネ等) は**元の EPUB のパス**をキーにする。キャッシュの削除・置き場所に影響されない |
| D4 | EPUB 由来の PDF は容量上限による自動削除の対象外。キャッシュ管理画面の明示操作でのみ消す |
| D5 | 同名の PDF がある場合は EPUB を一覧から隠す設定を追加し、既定 ON (`skip_archive_if_zip_exists` と同型) |
| D6 | リフロー型は電子書籍端末相当の**固定**ページ寸法で組む。設定は作らない。本の CSS は上書きしない |
| D7 | 朗読音声・動画は PDF に入らない。マニュアルに明記する |
| D8 | WebView2 の束縛は `webview2-com` (Loader 静的リンク、DLL 同梱不要) |

## 2. 利用者から見た挙動

1. フォルダに `book.epub` があると、一覧に 1 冊として出る (バッジ「EPUB」)。未変換ならアイコン、変換済みなら 1 ページ目のサムネイル。
2. 開くと、既存の RAR / 7z と同じく `archive_file_handling` (Ask / Convert / Ignore) に従う。Ask なら確認ダイアログ → 変換 (進捗・キャンセル可) → PDF として開く。2 回目以降はキャッシュから即座に開く。
3. 開いた後のアドレスバー・親へ戻る・次回起動の復元・履歴・しおりは、すべて `book.epub` を指す。
4. DRM 付き / 壊れた EPUB / WebView2 Runtime なし / タイムアウトは、それぞれ理由を明示したエラーダイアログ。空の本にはしない。
5. 右開きの本は右開きで表示する (`page-progression-direction`)。

## 3. 中心設計: 識別は EPUB、読む瞬間だけ変換 PDF

### 3.1 考え方

EPUB を「**中身のバイト列が変換キャッシュに置いてある PDF**」として扱う。

- アプリ内で本とページを識別するパスは、**どこでも `x.epub`** (`GridItem::PdfPage { pdf_path: x.epub }`、`current_folder = x.epub`)。
- PDFium へファイルを渡す**唯一の境界** (`pdf_loader` の `encode_*_request`、`src/pdf_loader.rs:884-964`) の直前でだけ、`x.epub` を変換キャッシュの PDF パスへ読み替える。

PDF を実際に読む経路は、すべて `pdf_loader` の公開関数 → `encode_*_request(path)` → ワーカープール、を通る
(`get_document_info` / `get_page_sizes` / `analyze_page_content_type` / `enumerate_pages*` / `render_page*`、
`src/pdf_loader.rs:3919-4640`)。`pdf_loader` の外で PDF のバイト列を直接開いている箇所は見つからなかった
(`fs::read` / `File::open` / `fs::copy` を検索。例外は `app.rs:26704` の `metadata` で、これはページ数キャッシュの鍵用 = 識別側の値)。

### 3.2 不変条件

- **I1 識別の一意性**: 本・ページの識別・永続キー・表示・履歴に使うパスは `x.epub`。変換 PDF のパスは `pdf_loader` の読み替え関数の外へ出さない。
- **I2 読み替えは 1 か所**: `x.epub` → 変換 PDF の解決は `pdf_loader` 内の単一関数 (仮称 `resolve_pdf_read_path`) だけが行う。呼び出し側は読み替えを知らない。
- **I3 未変換は型付きエラー**: 変換 PDF が無い / 元 EPUB の mtime・size が記録と違う場合、読み替え関数は `NotConverted` を返す。空の本・黙った fallback にしない。上位はこれを変換導線へ送る。
- **I4 派生データは読んだファイルで検証**: PDF の中身から作った派生データ (ページ数キャッシュ、サムネイルカタログの行、PDF ワーカーの文書キャッシュ、保持ラスタ、類似画像の特徴量、ページの content type) の鮮度判定は、**読んだファイル (変換 PDF) の mtime / size**、または変換の世代を含める。EPUB の mtime だけで判定すると、再変換でページ割りが変わったときに古い派生データが残る。
- **I5 UI スレッドで DB を引かない**: 読み替え関数は UI スレッドから同期で `archive_cache.db` を開かない (§3.4)。

### 3.3 この方式で自動的に満たされること

- D3 のキーが `x.epub::page_N` になる (`edit_source::page_key_for_pdf`、`src/edit_source.rs:394-396`)。
- mIV 内での移動・改名は既存のキー移行 (`rename_key_migration`) がそのまま追従する (キーの接頭辞が EPUB パスのため)。
- 保存済みキーから項目を組み立て直す経路 (レーティング一覧・しおり一覧・類似検索プレビュー等) は `PdfPage { pdf_path: x.epub }` を作り、それが読み替え経由で描ける。
- `archive_source_override` を使わない。`load_pdf_as_folder(x.epub)` がそのまま `current_folder = x.epub` を設定するので、変換 ZIP で必要だった「開いた直後に current_folder を同期確認する」経路 (`src/ui_dialogs/archive_convert.rs:370-378`、`:1205-1226`) の非同期問題が発生しない。
- 別ウィンドウの記述子 `ViewerContextDescriptor::Pdf` (`src/app.rs:2633`) も変更不要 (パスが EPUB になるだけ)。detached 凍結ルールの対象経路に新しい分岐を足さない。

### 3.4 読み替え関数と登録表

```
pdf_loader::resolve_pdf_read_path(identity: &Path) -> Result<PathBuf, PdfReadPathError>
  - 拡張子 pdf            → identity をそのまま返す
  - 拡張子 epub           → 登録表を引く。無ければ archive_cache.db を peek して登録 (ワーカースレッドのみ)
                             mtime/size 不一致・行なし → Err(NotConverted)
```

- 登録表はプロセス共通 (`RwLock<HashMap<正規化パス, (変換PDF, src_mtime, src_size, cache_mtime, cache_size)>>`)。
  変換完了時・キャッシュ削除時・元ファイル削除時に更新する。
- `pdf_loader` の同期 API を UI スレッドから呼んでいる箇所があれば (レビューで要確認)、そこは登録表のみを見て、
  未登録なら `NotConverted` を返す。DB を引くのはワーカー側だけ (I5)。
- PDF ワーカープールの文書キャッシュは `(path, password, mtime, size)` で持つ (`virtual-folders.md:367-371`)。
  読み替え後のパスが渡るので、変換 PDF の stat で判定される (I4 を満たす)。

### 3.5 却下した案

| 案 | 内容 | 却下理由 |
| --- | --- | --- |
| A | `PdfPage` に識別用パスの欄を足し、`pdf_path` は読み取り用にする | 鍵関数 約 20・`current_folder` 由来の鍵 6・表示 約 10・組み立て直し 約 8・リモート 約 8 の意味的変更 (60〜90 か所)。組み立て直し経路が `.epub` の `pdf_path` を作って描けなくなる危険が高い |
| B | `archive_source_override` を PDF にも使う | ページのキーが変換 PDF のパスのまま (D3 を満たさない)。override は「変換 ZIP」を前提にした分岐が多い (`app.rs:30229`、`:44147`) |
| C | 変換 7z と同じく、キーは変換 PDF のパス、キャッシュを消さない | 元 EPUB の移動・改名でキーが孤立する (改名移行が対象外)。サイドカーが APPDATA に書かれる。ドライブ文字だけ違う同名パスが同じキャッシュを共有する (`archive_cache.rs:52-55`) |

## 4. 構成要素

### 4.1 一覧での扱い (`GridItem`)

- `ArchiveFormat` に `Epub` を追加 (`src/archive_converter.rs:63-127`)。一覧のタイルは既存の
  `GridItem::ConvertibleArchive { path, format: Epub }` (`src/grid_item.rs:46-51`)。変換確認・Ignore 設定・
  変換状態表 (`converted_archive_cache_paths`)・キャッシュ管理の既存機構に乗る。
- 変換状態表 `ConvertedArchiveSourceState` (`src/app.rs:4898-4903`) に `CachedPdf(PathBuf)` を追加し、
  サムネイル要求は `pdf_page: Some(0)` で **識別パス `x.epub`** に対して出す (読み替えは pdf_loader が行う)。
  キー `archivethumb:epub:{name}` は既存規則 (`app.rs:78649-78674`) に従う。
- 開いた本の中のページは `GridItem::PdfPage { pdf_path: x.epub, .. }`。
- 手書きの拡張子列挙・網羅 match (調査で判明した箇所) を更新する:
  `archive_converter.rs:101,111,316-321,782-792,977-986`、`archive_cache.rs:577-597`、`app.rs:78649,78888`、
  `reading_history_db.rs:490-507`、`zip_tree.rs:490-500` (EPUB は ZIP 内セグメントに現れないので対象外の判断を明記)、
  `crates/remote-web/src/store.rs:321`、ホバー文言 `ui_main.rs:6543`、トースト 5 か所
  (`app.rs:21864`、`gamepad_input.rs:7261`、`smart_folder.rs:1897`、`snapshot_ops.rs:1439,2041`)。

### 4.2 「PDF 系の本」判定の一本化

- `is_pdf_extension` 等の拡張子判定 (非テスト 約 47 か所) を、問いの意味で 2 つに分ける:
  - **「PDF 系の本 (ページを PDFium で描く容器) か」** → 新設 `folder_tree::is_paged_document_path` (pdf / epub)。
  - **「実ファイルが PDF か」** (パスワード保存、PDF として外部へ渡す等) → 既存の `is_pdf_extension` のまま。
- 実装 PR には 47 か所それぞれの分類表を付ける (件数ではなく項目で突き合わせる)。
- ただし一覧の分類 (`src/app/folder_scan.rs:547-602`) では、EPUB は `ConvertibleArchive` のまま
  (`PdfFile` にはしない)。未変換の状態を表せるのは `ConvertibleArchive` だけのため。

### 4.3 開く流れ

`load_folder_or_convert_archive_with_auto_fullscreen_owned` (`src/app.rs:21807-21917`) の EPUB 分岐:

1. Ignore → 既存のトースト (文言を一般化)。
2. キャッシュ命中 (`try_archive_cache_lookup`) → 登録表へ登録 → `load_pdf_as_folder(x.epub)`。
3. 未変換 → 既存の変換ダイアログ (`ArchiveConvertState`、`archive_convert.rs:113-188`) → 完了 →
   登録表へ登録 → `load_pdf_as_folder(x.epub)`。変換 ZIP 用の「開いた直後の current_folder 同期確認」と
   `archive_source_override` の設定は EPUB では行わない。
4. `load_folder_with_scan_claimed` の拡張子分岐 (`app.rs:22351-22412`) で `.epub` → `load_pdf_as_folder`。
5. 保存済み参照 (履歴・しおり・コレクション・起動復元) から未変換の EPUB ページを開こうとして
   `NotConverted` が返った場合も、同じ変換導線へ送る (I3)。

確認ダイアログの要約 (`ArchiveImageSummary`) は画像枚数を数える作りで EPUB に合わないため、EPUB 用に
「レイアウト (固定 / リフロー)・綴じ方向・spine 数・DRM 判定」を出す要約を足す。要約は変換器の
`inspect` サブコマンド (WebView2 を使わない解析のみ) をワーカースレッドで実行して得る。

### 4.4 変換キャッシュ (`archive_cache`)

- 保存先パスに出力拡張子を渡せるようにする (`cache_zip_path_for_data_dir` / `reserve_cache_zip_path`、
  `archive_cache.rs:65-76,542-548`)。EPUB は `<root>/<hash[..2]>/<hash>/<stem>.pdf`。
  `metadata_transfer.rs:1510,1577,2100,4361,4377` と `content_identity/restore.rs:299,402` の同関数利用は、
  EPUB では呼ばれない (識別が EPUB パスのため) ことを確認し、ZIP 用の関数として残す。
- DB の `format` 列に `"epub"` を追加 (`format_to_db` / `format_from_db`)。スキーマ変更なし。
- `prune_to_size_limit_locked` (`archive_cache.rs:475-538`) は `format = epub` の行を対象外にする (D4)。
  `delete_missing_originals` (元ファイル消失) と手動削除は EPUB も対象。
- キャッシュ管理画面 (`src/ui_dialogs/archive_cache_manager.rs`) の「ZIP」表記を一般化し、形式列に EPUB を出す。
- 削除・全削除・元消失削除のたびに §3.4 の登録表を更新する。

### 4.5 変換器ワーカー (`crates/epub-pdf-worker`)

本体からは子プロセスとして 1 冊ずつ起動する (プールにしない)。スパイクからの変更点:

- **ネットワーク遮断**: `WebResourceRequested` で仮想ホスト以外への要求をすべて拒否する。EPUB 内の CSS / フォント /
  画像が外部 URL を参照していても通信しない (プライバシーページの「通信しない」記述を偽にしないため)。
- **本のスクリプトを実行しない**: `IsScriptEnabled = false`。待機判定に使う `ExecuteScript` はホスト側から実行する
  (無効化中も動作するかをスパイクで確認する)。
- **待機ループ**: `PeekMessage` + 10ms sleep を `MsgWaitForMultipleObjects` のタイムアウト付き待ちへ替える。
- **進捗**: 標準出力に 1 行 1 JSON (`{"phase":"print","done":N,"total":M}` 等)。本体は既存の進捗バーへ流す。
- **出力**: 本体が指定した `<cache>.pdf.part` へ書き、本体側が検証 (PDFium で開いてページ数が報告値と一致) 後に
  既存の `replace_file_atomic` で公開する (`archive_converter.rs:726-742`)。
- **ユーザーデータフォルダ**: `<temp_root>/epub-<pid>` (materializer と同じ一時領域、`src/materializer.rs:340-353`)。
  起動時の死んだ PID の掃除 (`:1496-1586`) の対象に含める。
- **子プロセスの後始末**: 本体は変換器を `KILL_ON_JOB_CLOSE` の Job Object に入れる (リポジトリ初の Job Object)。
  WebView2 が起動する `msedgewebview2.exe` 群も同じ Job に入り、キャンセル・本体終了・本体異常終了で確実に終わる
  ことを期待している (**要検証**: WebView2 のブラウザプロセスが Job から離脱しないか)。キャンセルは Job の終了で行う。
- **リフローの組版**: ページ寸法を電子書籍端末相当の固定値にする (候補 720×1024 CSS px、余白 32px。
  実装時に草枕・moby-dick・ごん狐を画像化して決める)。値は変換器内の定数とし、変換プロファイル名
  (例 `reflow-v1`) をレポートに出す。
- **終了コード → 本体のエラー**: 2 DRM / 3 不正 / 4 WebView2 Runtime なし / 5 描画失敗 / 6 タイムアウト を
  `ConvertError` の新しい型へ対応させ、理由ごとの文言を出す。

### 4.6 配布

mIV Remote のサービス exe と同じ形にする (cargo でビルド、launcher が内包、ポータブルは exe の隣、本体は自分の exe の
隣から探す。`src/remote_ipc/service.rs:41-46` と同型の解決関数 + `MIV_EPUB_PDF_WORKER` 環境変数での上書き)。

触る箇所: `crates/launcher/build.rs:31-64,112-117`、`crates/launcher/src/main.rs:23,87-91` (+ テスト `:584-610`)、
`scripts/build-release.ps1` / `.sh`、`scripts/build-dev.ps1`、`scripts/build-portable.ps1`、`scripts/build-dist.ps1`
(ビルド・存在確認・複製・署名一覧・プロセス停止一覧・clean・最終 PE 一覧)、`installer/readme*.txt`、
`installer/mimageviewer.iss` のコメント、`CLAUDE.md` Distribution 節、`docs/portable-build-plan.md`。
x64 で `+crt-static` なので VC runtime 検査の追加設定は不要 (`check-vcrt-pe-dependencies.ps1:67-73`)。

### 4.7 同名スキップ設定 (D5)

`skip_epub_if_pdf_exists` (既定 true)。`skip_archive_if_zip_exists` と同じ箇所に並べる:
`settings.rs:4515` 付近と `Default`、`folder_scan.rs:258-264,790-853`、`folder_tree.rs:25-48,701-738`、
`smart_folder.rs:3503-3769`、`settings_db.rs` のリモート一覧設定 5 か所 (`:54-61,298,354,375,2923`)、
環境設定 `preferences/pages.rs:8603-8609`・検索索引 `search_index.rs:534-539`・再読込タプル
`preferences.rs:2291-2299,2591-2599`。

### 4.8 リモート (mIV Remote)

- 初版: **変換済みの EPUB を閲覧できる**ところまで。リモートの PDF 判定は論理パスの拡張子で行っている
  (`remote_ipc/container.rs` の 8 か所前後) ので `is_paged_document_path` へ置き換える。読み取りは §3.4 の
  読み替えを通る。一覧の種別 (`crates/remote-ipc/src/lib.rs:1355-1364`、`web/app.js:3577,3892`) に EPUB を足す。
- リモートからの変換 (`remote_ipc/archive_job.rs:1921-2047` 相当) は初版の対象外。未変換の EPUB は
  「PC で一度開いて変換してください」と表示する。

### 4.9 文書

- マニュアル: `formats.html:167-179` と `faq.html:90,168-176` の「EPUB 非対応」を書き換え (「DRM のない EPUB を
  固定レイアウトへ変換して閲覧。文字サイズ変更・テキスト選択は不可。朗読音声・動画は含まれない」)。
  `settings.html` (同名スキップ)、`tut-archive.html` (必要なら)。
- 製品ページ `index.html:54,1129`。対外表記は「EPUB 対応」ではなく「DRM のない EPUB を変換して閲覧」。
- `privacy.html` / 製品ページ「安心して使えます」: WebView2 の一時データを端末内に作ること、変換時に通信しない
  こと (§4.5 のネットワーク遮断が前提)。
- 設計文書: `docs/virtual-folders.md` (EPUB の識別・読み替え規則)、`docs/spec.md`、`docs/architecture-overview.md`
  (新ワーカー)、`docs/async-architecture.md` (子プロセス + Job Object)。

## 5. 実装の段階

| 段階 | 内容 | 完了条件 |
| --- | --- | --- |
| S1 | 変換器の仕上げ (§4.5: 通信遮断・スクリプト無効・待機・進捗・リフロー寸法・出力規約) | 27 冊バッチが前回と同等、通信遮断を外部 URL 参照の合成 EPUB で確認 |
| S2 | 読み替え境界と登録表 (§3.4)、`ArchiveFormat::Epub`、キャッシュ (§4.4)、開く流れ (§4.3)、Job Object | 状態遷移テスト (未変換→変換→開く→キャッシュ削除→NotConverted→再変換)、I1〜I5 のテスト |
| S3 | 「PDF 系の本」判定の一本化 (§4.2、分類表付き)、一覧・バッジ・サムネイル、同名スキップ (§4.7) | 分類表の全行にテストか理由 |
| S4 | リモート閲覧 (§4.8) | リモートで変換済み EPUB のページ送り・キーが PC と同一 |
| S5 | 配布 (§4.6)、文書 (§4.9) | build-dist が通り、署名・VC runtime 検査に新 exe が含まれる |

## 6. テスト

- 単体: 読み替え関数 (pdf / epub 命中 / 未登録 / mtime 不一致)、登録表の更新 (変換・削除・元消失)、
  `prune` が EPUB 行を消さない、終了コード → エラー型、同名スキップ。
- 状態遷移: §5 S2 の遷移を 1 本で通す。キーが `x.epub::page_N` であること、変換 PDF のパスがキー・表示・履歴に
  現れないこと (I1) を、保存された DB の行を読んで確かめる。
- I4: 同じ EPUB を「ページ数の違う PDF」で再変換したとき、ページ数キャッシュ・サムネイルが古いまま残らないこと。
- 実機 (利用者): 漫画 EPUB を開く→右開き→補正・レーティング→キャッシュ削除→開き直して編集が残る、
  DRM 付き相当でのエラー表示、変換中キャンセルで `msedgewebview2.exe` が残らないこと。

## 7. レビューで特に確かめてほしい点

1. §3.1 の前提「PDF のバイト列を読む経路はすべて `pdf_loader` の `encode_*_request` を通る」は正しいか。
   迂回している読み取り (外部ツールへの受け渡し、ファイル書き出し、ハッシュ計算等) は無いか。
2. `pdf_loader` の同期 API を UI スレッドから呼んでいる箇所の有無 (I5)。
3. I4 の対象となる派生データの一覧に漏れは無いか。特にページ数キャッシュ (`app.rs:26695-26743`、親フォルダの
   カタログにファイル名 + mtime + size で保存) とサムネイルカタログ (識別パスのハッシュで DB を分ける、
   `catalog.rs:21-33`) の鮮度判定。
4. §4.2 で `ConvertibleArchive` (タイル) と `PdfPage` (中身) に分かれることで、タイル側のレーティング・タグ・
   代表サムネのキーが PDF 本 (`PdfFile`) と食い違わないか。
5. Job Object に入れた変換器から起動される WebView2 のプロセスが Job 内に留まるか。
6. `rename_key_migration` が EPUB パス接頭辞のキーを PDF と同じく移行するか (`.pdf` 前提の分岐が無いか)。
7. 既存の「変換 ZIP」前提の分岐 (`archive_source_override` を見る箇所) に EPUB が誤って入らないか。

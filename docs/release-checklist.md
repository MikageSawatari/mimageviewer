# リリース手順チェックリストと配布形態

> この文書は 2026-10-15 に CLAUDE.md から移した節をそのまま収めたもの。本文中に残る「本書」は移動前の CLAUDE.md を指す。移した節の行き先一覧は CLAUDE.md の「移した節の行き先」表を参照。

## Distribution

3 つの配布形態がある (v1.1.0 以降):

| 配布形態 | ファイル名 | 中身 / 性質 | data 保存先 | 管理者権限 |
| --- | --- | --- | --- | --- |
| **単体exe版** (旧称「ポータブル版」) | `mimageviewer.exe` | launcher。core + remote service + EPUB converter + FFmpeg DLL + VC runtime + EffeTune Mixwright bundle を `include_bytes!` 内包、起動時に APPDATA へ展開して spawn | `%APPDATA%\mimageviewer` | 不要 |
| **インストーラ版** | `mImageViewer_setup.exe` | Inno Setup 出力 | `%APPDATA%\mimageviewer` | **要 (UAC)** |
| **ポータブル版** (v1.1.0 新) | `mImageViewer_portable_v<VER>.zip` | loose-deps。native 依存を埋め込まず exe 隣に同梱、展開ゼロ | `<exe_dir>\data` (APPDATA 不使用) | 不要 |

> ⚠ **用語**: `mimageviewer.exe` (launcher 単体 exe) は APPDATA を使うので「ポータブル版」と
> **呼ばない**。「単体exe版 / オールインワン版」と呼ぶ。「ポータブル」は loose-deps zip 専用。

- **mikage.to**: インストーラ版 + 単体exe版 + ポータブル版 zip の 3 つを提供
- **窓の杜**: インストーラ (.exe) を zip にまとめて申請
- **Vector**: インストーラ (.exe) + `installer/readme.txt` (利用者向け説明書) を zip にまとめて申請。
  readme 同梱を Vector が要件化しているため、単体 exe やインストーラ単独での申請は不可。
- **インストーラ**: Inno Setup 6（`installer/mimageviewer.iss`）
- **配布ビルド**: `.\scripts\build-dist.ps1` (全体テスト → clean → core → remote → EPUB worker → launcher → ISCC → portable を 1 コマンド、
  stale 配布物を構造的に防ぐ)。開発中の素早い反復だけ `.\scripts\build-release.ps1` 単体を使う
  (clean/ガードなしなので配布物には使わない)。
- **出力**: `installer/Output/mImageViewer_setup.exe`
- **Vector 申請用 zip**: `mImageViewer_installer_v<VERSION>.zip` に `mImageViewer_setup.exe` と
  `installer/readme.txt` を同梱する (v1.1.0 で `mImageViewer_v<VERSION>.zip` から改名。ポータブル
  zip と接尾辞 `_installer_` / `_portable_` で区別する。**リリース済みの過去版は遡って改名しない**)。
  readme の内容 (動作環境・連絡先・インストール手順・取り扱い種別) は Vector のファイル掲載基準
  (https://www.vector.co.jp/for_authors/upload/standard.html) を満たしていること。
- **ポータブル版ビルド**: 配布時は build-dist.ps1 が内部で `.\scripts\build-portable.ps1` を呼ぶ。
  `cargo build --release --bin mimageviewer-core --features portable --target-dir target-portable` と
  `cargo build --release -p mimageviewer-remote --bin mimageviewer-remote --features embedded-web-assets --target-dir target-portable`
  と `cargo build --release -p epub-pdf-worker --bin mimageviewer-epub-pdf --target-dir target-portable`
  (非portable core を上書きしないよう専用 target dir に分離) → loose 同梱フォルダ +
  `dist\mImageViewer_portable_v<VER>.zip` を生成する。`portable` feature で native 依存
  (pdfium / onnxruntime / susie / vst3-host / モデル) を埋め込まず exe 隣から解決し、`data_dir` を
  `<exe_dir>\data` に向ける。launcher は使わず core を `mimageviewer.exe` にリネームし、
  remote service と EPUB converter をその隣へ同梱。
  VST host と EffeTune bundle は利用者決定により非同梱。音響調整は bundle 不在で利用不可のまま。
  `portable` build flavor では音響調整ボタン・カスタマイズ候補・v4.3.0の関連告知も表示しない。
  保存済みセクション順は維持し、描画対象だけを除外する。通常版のbundle欠落は表示で隠さない。
  設計・保守方針 (CI guard 等) は [docs/portable-build-plan.md](portable-build-plan.md)。
  `portable` feature の cfg 分岐は `.git/hooks/pre-push` の `cargo check --features portable` が番人。
- **EffeTune 配布境界**: 単体exe版／インストーラ版は承認済み v0.13.0 の VERSION と全448ファイルを
  `third_party/effetune-mixwright/v0.13.0/manifest.sha256` に固定し、署名前とlauncher build時に欠落・追加・
  改変を拒否する。署名stageは固定target配下だけ許可し、PEのchecksum／証明書以外は原本と同一、
  指定発行元の有効署名があることも検証する。未署名の開発buildはraw原本の完全一致が必要。
  launcherは `runtime/<version>/effetune/<hash12>-<generation>/EffeTune Mixwright.vst3/` に
  検証済み世代を一度だけ公開する。公開処理は既存・使用中treeの移動、置換、削除をしない。**atomicなのは
  完全な世代を指す小さなcurrent pointerの更新**であり、treeのin-place修復ではない。
  正常時は一覧・サイズ・更新時刻・作成時刻stampだけを検査し、全量再hashとwrite lockを避ける。
  不一致は別世代を全hash検証して公開する。core の起動後の runtime 清掃 worker が、current と
  使用中の固定世代以外を best-effort 削除する。各coreが世代shared leaseを保持し、
  清掃はcandidateのexclusive leaseと版の公開lockを削除完了まで保持する。
  lease／抽出／公開lockは削除tree外の `runtime/.locks/` に固定して削除しない。
  publisherは外側lock→既存 `.effetune.lock` の順に両方を取得して旧launcherとも直列化する。
  ロック未対応旧coreもprocess画像で確認し、同版の別coreが動く間は全旧世代を保持する。
  公開用 OS lock が busy、current が不明、
  core の固定世代が未確定・未知の配置なら世代清掃を見送り、初期化は待たせない。
  hash12はcontent SHA256先頭12桁（stampはfull hashを比較）。公開前に最深fileのUTF-16長を確認し、
  260以上なら明示理由で拒否する。publisher busyはworkerのOS lockを最大60秒待ってpointerを再確認する。
  repair不能／timeoutでもcoreは起動し、理由と実際に拒否した世代をenvで渡してUnavailableにする。
  音響調整ボタンで既存load workerから再確認し、拒否世代とは別の公開済み世代だけ採用する。
  成功時も選択したgenerationをenvで渡し、coreはそのpathを一度解決して固定する。
  メタデータを保持した内容改変は既存asset shortcutと同様に検出範囲外。
  通知原文とmanifestは `third_party/effetune-mixwright/v0.13.0/` に追跡し、`.gitattributes -text`で
  checkout時の改行変換を防ぐ。
- **CRT 境界**: `.cargo/config.toml` で mIV 自身の x86_64 exe と Susie ワーカー (i686) は
  `+crt-static` を維持する。一方、Microsoft build の ONNX Runtime は動的 VC runtime を import
  するため、公式 VC/Redist 由来の x64 4本を全配布 exe の隣へ app-local 配置する。
  EffeTune pluginも動的VC runtimeをimportする。hostは
  `data_dir/vst3/hosts/<host+CRT SHA256>/mimageviewer-vst3-host.exe`、同梱CRTは非検索subdir `vcrt/`。
  hostはCRTを一組で選ぶ。System32の全4本が存在・版数読取可能で各DLLのfile versionが
  同梱版以上なら全4本System32、それ以外は全4本同梱。選択後に依存順で絶対pathからpreloadし、
  途中のsource切替はしない。選択元と両版数をlogへ記録する。shared host直下に固定CRTを置かず、抽出は一致bytesを
  書き換えず、成功だけcacheして失敗の再試行を許す。portableはこの抽出経路を使わない。
  `scripts/check-vcrt-pe-dependencies.ps1` が全配布 PE の machine / import closure と、CRT・ORT の
  Microsoft 署名、CRT の同一版・最低版・manifest hash を検査する。未知の `msvcp*` /
  `vcruntime*` / `concrt*` import は gate failure とし、追加 DLL を場当たり的に配布しない。
  app-local CRT は Windows Update で更新されないため、toolchain / native dependency 更新時に
  `vendor/vcrt/provenance.json` と実体を一体で見直す。
- **設定保存先**: インストーラ版・単体exe版は `%APPDATA%\mimageviewer`、
  ポータブル版は `<exe_dir>\data` (書込不可なら APPDATA へフォールバックせずエラー起動拒否)。

## リリース手順チェックリスト

**担当: ClaudeCode Opus 5.5 / high (担当範囲は2026-09-09決定、モデルは2026-09-23更新)。** Phase 0〜5の公開準備、
配布ビルド・署名、公開、配布チャネルへの反映を通して担当する。開発側から対象commitと
検証記録を受け取り、定型公開作業ごとのCodexレビューや親役の追加は不要。
製品コード等の修正が必要になった場合の境界は
[開発と公開の担当・引き継ぎ](release-operations.md#開発と公開の担当・引き継ぎ) を参照する。

リリース時は以下を漏れなく更新・作成すること。

**過去リリースで踏んだ落とし穴・判断基準・復旧手順は
[docs/release-operations.md](release-operations.md) に集約している。**
本チェックリストが手順の正本で、release-operations.md はその補助 (stale core cache /
署名セッション切れ / タグ再打ち直し / FFmpeg LGPL ソース同一性 / ポータブル AV 誤検知 /
配布チャネル別の注意など)。開発担当からClaudeCode Opusの公開タスクへ引き継ぐ際に一読する。

### Phase 0: 変更履歴の準備とユーザーレビュー (必ず最初)

更新履歴は `update_check.rs` 経由でアプリ内アップデート通知ダイアログにそのまま
表示される (= GitHub Releases の `body` がユーザーに読まれる)。誤字・内部用語の
混入・粗い表現があると公開後に取り返しが付かないので、以下の順で進める。

- **CHANGELOG.md の更新履歴セクション** にこのバージョンの新エントリを追加 (旧版と
  同じ `### vX.Y.Z (YYYY-MM-DD)` ヘッダ + 箇条書きフォーマット)。書き方は [public-wording-policies.md](public-wording-policies.md)
  の「マニュアル・製品ページの記述方針」に従う:
  - **見出しにリリース日を `(YYYY-MM-DD)` 形式で付ける** (他バージョンと揃える)。
    日付はそのリリースの GitHub タグ公開日 (= Phase 1 で製品ページの「最終更新」に
    書くのと同じ日)。`gen-changelog-html.py` がこの括弧内の日付をパースして
    マニュアルの更新履歴ページ (changelog.html) の見出しに併記するので、形式を崩さない
  - 内部実装語 (Tantivy / SQLite / 並行処理用語) は使わない
  - バージョン番号タグ (v0.8.2+ 等) は本文に書かない (見出しに 1 回だけ)
  - 過去のユーザー報告日付 / コミットハッシュは書かない
  - 「⚠️ 」プレフィックスは初回起動時に何かが起きる注意 (索引再構築等) に限定
  - 更新履歴は 2026-09 に README.md から `CHANGELOG.md` へ移した。README には書かない
    (README は概要・特長・ダウンロードを冒頭に置く入口で、更新履歴へはリンクだけ置く)
- **ユーザーに CHANGELOG.md の更新履歴を見せて承認を得る**。OK が出るまで Phase 1 へ
  進まない。
- (任意) GitHub Release 用 body は基本 CHANGELOG.md の該当セクションをそのまま
  コピペで OK。手作業で別途書き直さない (= 表記ゆれを生まない)。
- **⚠️ 8KB 上限チェック (大型リリースで必須)**: アプリ内更新通知は
  `update_check.rs` の `BODY_CAP = 8 * 1024` で **先頭 8KB (UTF-8 バイト)** に切られる。
  CHANGELOG.md の該当セクションが 8KB を超えると、後半の項目 (= 末尾に置きがちな新機能 /
  バグ修正) が通知に出ない。リリース前に必ずバイト数を測る:
  ```bash
  # 見出しは "### vX.Y.Z (YYYY-MM-DD)" 形式なので、版番号の後ろは空白か行末で区切る
  awk '/^### vX\.Y\.Z( |$)/{f=1} /^### v<前版>( |$)/{f=0} f' CHANGELOG.md | wc -c
  ```
  - **8KB 以内**: CHANGELOG.md のセクションをそのまま Release body に使う (上記)。
  - **8KB 超過**: CHANGELOG.md はフル版のまま残し、**`docs/release-body-<version>.md` に
    8KB 以内の短縮版を別途作成**する (BOM なし、Markdown。通知ダイアログは Markdown
    レンダリング対応なので見出し・箇条書き可)。短縮版は「目玉の新機能 → 主な改善 →
    主なバグ修正」の順で前方に重要項目を寄せ、全項目は CHANGELOG.md を参照する旨のリンクを
    冒頭に入れる。**この短縮版ファイルを Phase 4 の Release body に使う** (下記)。
    作成後 `wc -c docs/release-body-<version>.md` で 8192 以下を確認。
    - 注意: 8KB 上限は **更新を受け取る側 (= 旧バージョンのバイナリ)** に焼かれている
      ので、今リリースで `BODY_CAP` を上げても今回の通知には効かない (効くのは次版以降)。
      短縮版で回避するのが確実。
    - 実例: v1.0.0 (README セクション 17.5KB → [docs/archive/release/release-body-v1.0.0.md](archive/release/release-body-v1.0.0.md) 6KB)。

### Phase 1: バージョン番号・関連ファイル更新

承認を得た上で以下を更新:

1. `Cargo.toml` — バージョン番号
2. `installer/mimageviewer.iss` — `MyAppVersion`
3. `installer/readme.txt` — 先頭の版表記・更新履歴リンク (Vector 同梱用)
3.5. `installer/readme_portable.txt` — 先頭の版表記 (ポータブル版 zip 同梱用、v1.1.0+)
4. `htdocs/mimageviewer/index.html` — ダウンロードセクションのバージョン表記と
   **「最終更新: YYYY-MM-DD」の日付** (`.download-info` の `.meta` 行)。日付は今回の
   リリース日 (= GitHub Release 公開日と揃える) を記入する。
   **ポータブル版のダウンロードリンク URL もバージョンを含む** (`mImageViewer_portable_v<VER>.zip`)
   ので、バージョン表記と一緒に link href も更新すること (単体exe / setup.exe は非バージョン名で固定)。
   同じファイル冒頭の JSON-LD (`SoftwareApplication`) の **`softwareVersion` と `dateModified`** も
   揃えて更新する (`dateModified` は「最終更新」と同じ日付)。
4.5. **マニュアルの更新履歴ページを再生成** — Phase 0 で CHANGELOG.md の更新履歴セクションが
   承認されたら、`python scripts/gen-changelog-html.py` を実行して
   `htdocs/mimageviewer/manual/changelog.html` を作り直す。changelog.html は CHANGELOG.md の
   `## 更新履歴` から生成される**生成物**なので手で編集しない (編集すると次回再生成で消える)。
   生成後 `git diff` で最新版エントリが反映されていることを確認する。
5. `htdocs/mimageviewer/manual/index.html` — マニュアルのバージョン表記
5.5. **「重要な変更点」テーブルの追記** (操作・既定の変更があるリリースのみ) —
   [src/version_highlights.rs](../src/version_highlights.rs) の `TABLE` に今回バージョンの
   エントリ (`must_read` = 操作・既定の変更、`highlights` = 主な新機能) を追加する。
   更新後初回起動でユーザーに自動表示される (= ④ version-highlights、display-only、内部用語禁止)。
   操作・既定の変更が無いリリースでは追記不要。追記したら
   `cargo test --lib version_highlights::` でテーブルがパースできることを確認。
6. `htdocs/` 以下 — 新機能がマニュアル・製品ページに反映されていることを確認
   - 製品ページのカードの長さを `python scripts/check_site_cards.py` で検査する (「製品ページのカードは短く保つ」)。
   - マニュアル左サイドバーを持つ通常ページ 32 ページでリンク一覧が揃っているか
     `htdocs/mimageviewer/manual/` 配下で一括確認:
     ```bash
     cd htdocs/mimageviewer/manual && for f in *.html; do
       grep -q 'sidebar-section' "$f" || continue
       echo "=== $f ==="
       sed -n '/sidebar-section/,/<\/nav>/p' "$f" \
         | grep -E 'href="[a-z-]+\.html"' | wc -l
     done
     ```
     各ページが 32 以外 (= いずれかのページ名リンクが抜けている) なら同期を合わせる。
     ページを増減したらこの数も更新する (数そのものより、**全ページが同じ数で揃っている**ことが要件)。
     `tut-*.html` など `sidebar-section` を持たないチュートリアルページは別レイアウトなので対象外。
     新規の通常ページを追加した際はサイドバーを持つ全ページを更新すること
     (`changelog.html` のサイドバーは `gen-changelog-html.py` が `getting-started.html` から
     コピーするので、他の通常ページを更新してから再生成すれば自動で揃う)。
6.5. **既知の問題ページの棚卸し** — [htdocs/mimageviewer/manual/known-issues.html](../htdocs/mimageviewer/manual/known-issues.html)
   から、**この版で直した項目を削除する**。残す項目の記述が現行版で正しいかも見る
   (回避方法が変わっていないか)。**掲載が 0 件になったら「現時点で把握している不具合はありません。」
   と明記する** (見出しごと消さない。ページが空だと「更新が止まった」と読まれるため)。
   - 載せる基準: ①通常の操作で遭遇し得る ②不具合だと思う見た目をしている ③回避策があるか
     「環境のせいではない」と言える。次の版で直す予定の不具合も、公開中の版で起きる間は載せ、
     「次のバージョンで修正します。」と書く (版番号は書かない。直した版の公開時にこのステップで削除する)。
     backlog の大半は設計上の負債なので載せない。
   - **このステップを飛ばすとページが腐る**。以前ページを置いたときに放置されて消えた経緯がある。
6.6. **サイトマップを再生成** — htdocs の編集がすべて終わったら
   `python scripts/gen-sitemap-xml.py` を実行して `htdocs/sitemap.xml` を作り直し、
   出力に出た added / removed が意図どおりか確認してコミットする。sitemap.xml は
   **生成物**なので手で編集しない。`<lastmod>` は git の最終コミット日から取るため、
   **htdocs の編集をコミットした後に実行する** (未コミットのページは当日付になる)。
   `--check` を付けると書き換えずに、古ければ exit 1 で知らせる。
   - **このステップを飛ばすと sitemap が静かに腐る**。2026-09 の点検時、8/23 から
     37 ページが変わったのに `<lastmod>` が 1 つも動いておらず、`manual/external-tools.html`
     は追加されたまま一度も sitemap に入っていなかった。手順のどこからも参照されて
     いなかったので、腐る以外の道が無かった。

### Phase 2: 依存物の確認 + 性能回帰チェック

6.3. **依存物の更新確認**: `.\scripts\check-dependency-updates.ps1` を実行し、
   標準出力と `target/dependency-updates/<日付>.md` の一覧を見て、上げるかを判断する。
   問い合わせ専用で依存物の取得・変更・インストールは行わない。手を入れているクレートを
   上げるときは、表にある変更記録 (`PATCHES.md` / `MIV-PATCH.md` / README の patch 節等) に
   従って変更を付け直す。月 1 回の自動実行もある。既存の PDFium 等の個別手順も引き続き行う。
   詳細は [リリース運用メモ](release-operations.md#依存物の更新確認) を参照。

6.4. **記録済みクラッシュの棚卸し (最初に回す)**:

   ```powershell
   .\scripts\check-panic-log.ps1
   ```

   `%APPDATA%\mimageviewer\logs\panic.log` に残っている panic を種別ごとにまとめ、
   `docs/panic-acknowledged.tsv` に disposition (`fixed` / `filed` / `external`) が無いものが
   あれば **exit 1** する。アプリは起動しないし、ログを書き換えもしない。

   - **`panic.log` は起動時にローテーションしない**ので、何日も前のクラッシュがそのまま残る。
     4 MiB を超えたときだけ `panic.log.bak` へ 1 世代退避するので、script は両方を読む
   - 落ちたら、**閾値やベースラインで黙らせない**。直す・バックログへ起票する・依存側の問題だと
     説明する、のいずれかを行い、その結果を tsv へ書く。fingerprint 列は script の出力から
     コピーする (手で書くと数字の正規化がずれて一致しない)
   - **これを入れた経緯**: v3.6.0 の出荷直前に「複数ウィンドウで PDF を開くと必ず落ちる」が
     実機で出た。そのとき panic.log を開いたら、**2026-04 以降のクラッシュが 15 種 52 件**
     記録されていて、同族の binding 不整合 2 件は 10 日前から入っていた。
     チェックリストには既に R8「panic.log を確認」があり、**手順ではなく実行が抜けていた**
     (バックログ §1.196)

6.5. **GitHub Actions の CI が緑であることを確認** (`gh run list --limit 5`)。
   赤なら原因を直してからリリース作業に入る。現在の job は `cargo fmt --check` と viewer context 監査の 2 本。
   (ubuntu での非 Windows cfg の `cargo check` job は 2026-09-29 に廃止した。mIV は Windows 専用で、
   この job のための修正と、どこでも実行されない `cfg(not(windows))` 側のコードが増え続けていたため。
   非 Windows でコンパイルが通るかは今後は検査しないので、復活させない。)
   失敗ログの読み方: `gh run view <run-id> --log-failed` はビルド全体の warning も含むため、
   ファイルへ落として `error[E` で絞り込む (`-->` の行に実ファイル位置が出る)。
7. PDFium の更新確認（`bash scripts/setup-pdfium.sh check`）
8. ONNX Runtime DLL の配置確認（`bash scripts/setup-ort.sh`、ort クレート更新時は必須）
9. Susie ワーカーの再ビルド（`bash scripts/setup-susie-worker.sh`）
10. FFmpeg LGPL shared build の更新確認（`bash scripts/setup-ffmpeg.sh check` はローリング資産を除外し、
    最新の版付き `autobuild-*` 資産と比較する）。バージョンを上げる場合は
    `vendor/ffmpeg/VERSION`・実 DLL の `ProductVersion`・対応ソースを同じ commit に揃え、
    `src/video/ffmpeg_loader.rs` の DLL 名も一致するか確認。LGPL 通知の更新も忘れずに
    ([native-dependencies.md](native-dependencies.md) の「FFmpeg LGPL DLL 管理」節)
10.1. LibRaw の更新確認（エージェント用 `bash scripts/setup-libraw.sh check`）。
    リリース前に [docs/libraw-source-distribution.md](libraw-source-distribution.md) の SHA-256 と
    同梱版を照合し、LibRaw の対応ソースを mikage.to に配置する。installer / portable に
    `LIBRAW-LICENSE.txt` / `ZLIB-LICENSE.txt` / `LIBJPEG-TURBO-LICENSE.txt` が入ることも確認する。
11. VST3 host bridge の確認 (v0.9.0+):
    - `vendor/vst3sdk/` が配置済み (`bash scripts/setup-vst3-sdk.sh`)
    - `vendor/vst3-host/mimageviewer-vst3-host.exe` が最新の C++ ソースでビルド済み
      (`cmake --build crates/vst3-host/build --config Release`)
    - 商用プラグイン (Pro-Q 4 等) で実機確認: 環境設定→動画タブで VST3 を ON →
      管理ウィンドウからプラグインをロード → 動画再生で音声がプラグインを通る /
      HUD の VST ボタンで GUI 開閉 (V キーショートカットは存在しない。VST ボタンは
      フル機能モードのメインウィンドウ・フルスクリーンのみ = detached では非表示)
    ※ `vendor/` 直下の必須ファイルは `build.rs` で起動時にチェックしており、
      不足していると `cargo build` が復旧手順付きで止まるようになっている。

9.5. **bench 回帰チェック** (検索周りに変更を入れたリリースで実施):
   ```bash
   cargo run --release --features dev-tools --bin bench_search -- --docs 50000 --json /tmp/bench_new.json
   python scripts/check_bench_regression.py vendor/bench_baseline.json /tmp/bench_new.json
   ```
   - 初回 (vendor/bench_baseline.json の queries が空) は `--save` で baseline を登録:
     `python scripts/check_bench_regression.py --save vendor/bench_baseline.json /tmp/bench_new.json`
   - 既存 baseline と比較して **+30% 超 かつ +1.0ms 超の劣化**で exit 1。原因を切り分けてから先に進む。
     Tantivy 等の依存更新で正当な変動なら `--save` で baseline を更新する。
     比率だけ超えたミリ秒未満のクエリは `NOISE` と表示されるだけで失敗にならない (計測ばらつきのほうが大きいため)。
   - 1 回きりの計測は外れ値を引く。失敗したら同じバイナリで 2〜3 回測り、JSON を並べて渡すと query ごとの最良値で判定する:
     `python scripts/check_bench_regression.py vendor/bench_baseline.json run1.json run2.json run3.json`。
     **ノイズと判断した場合に `--save` で baseline を上書きしない** (`--save` は 1 ファイルだけ受け付ける)。

9.6. **perf smoke** (正規手順は idle 5ケースと合わせた ReleaseChecks suite。毎リリース実施):
   利用者が repository root の PowerShell で実行する:
   ```powershell
   .\scripts\ui-smoke.ps1 -Scenario ReleaseChecks -InteractiveApproved
   ```
   - 診断portableのbuild、生成fixture／隔離dataのseed、5 idle、cached／uncached perf、通常終了・
     証跡保存・解析まで逐次実行する。**9.7も同じ1回のsuiteで確認し、重複実行しない**。
     FFmpeg CLI (`ffmpeg.exe`) がPATHに必要。初回buildは別に約7.5分かかった実績があり、
     suiteはseed込み約5〜8分を見込む。AI追加は別途準備・実行時間を見積もる。
   - **開始前に他アプリの窓を最小化し、build待ちを含め終了までPCに触れない**。
     exact rootのforeground取得、runner所有窓への背面移行、測定中の状態を検証する。
     foreground不成立時は記録を残して停止し、環境を整えた別runでやり直す。自動retryはしない。
   - 同一ソースの診断artifactが検証できる場合だけ上記に `-SkipBuild` を追加できる。
     buildを先に済ませる場合は `prepare-portable-smoke.ps1 -TestScript` を使う（起動しない）。
     stale artifactの利用や設定保護の省略を認めるフラグではない。
   - 結果は `target/ui-smoke/runs/<run-id>/summary.json` と `release-record.md`。
     全必須caseの証跡と終了codeを確認する。0=PASS／PASS_WITH_WARNINGS、1=FAIL、2=SKIPPED／環境不成立。
     警告はリリース記録に残し、FAIL／SKIPPEDをPASSに読み替えない。
     `known-startup-first-frame` は測定操作前のstartup-overlay frame 1→2 gapだけ、上限500ms。
     gap時間は別記され、超過や通常操作中の未知100ms超はFAIL。段別内訳不足も環境注記に残す。
   - cached／uncachedで256個の異なるfolder-load、Ctrl+Down x5、Ctrl+G固定検索を検証する。
     drawnは直前tailが厳密に `request_repaint` のgapだけ、暫定n≥200／<16ms比率97%はWARN目標、
     raw `pre_grid.total_ms` p95≤0.3msは必須。通常sleepをhitchとして集計しない。
   - AI／GPU経路変更版では同じ入口に `-IncludeFirstAi` を明示追加する。
     UI／I/O経路変更版では普段のフォルダの短い手動確認も別に残す。生成fixtureの成功を
     実データ・配布版の厳密性能・native入力／IMEの確認へ読み替えない。
   - 採用は実行了承ではない。エージェント実行は [実行確認](interactive-release-verification.md)
     に従い対象・所要時間・desktop入力・使い捨てdata・AI有無への明示了承後だけ行う。
     `-InteractiveApproved` は了承済みの実行assertion。normal-profile起動禁止と
     [development-build-and-test-native.md](development-build-and-test-native.md) の「検証起動時の設定データ保護」は維持し、利用者の通常設定をseedにコピーしない。
     full gate／buildへ自動連結しない。詳細・採用証跡は [自動化計画](release-check-automation-plan.md)。

   **手動fallback／診断** (利用者が通常releaseを起動。UI／I/O変更版の実データ確認にも使う):
   ```powershell
   .\scripts\perf_smoke.ps1
   ```
   - `--perf-log` 付きで mImageViewer を起動 → 手動で起動・Ctrl+↓ 連打・Ctrl+G 検索を実行
     → スクリプトが `analyze_perf.py hitches` で 16ms 超のフレーム間隔を集計。
   - **`hitches` の件数をそのまま合否にしない**。mIV は静止時に意図的に就寝するので、
     操作の合間の sleep がすべて「ヒッチ」として数えられる (実測: 通常操作 55 秒で 108 件、
     最大 16 秒)。判定は**大きいギャップ 1 件ずつ、直前の `ui.tail_repaint.action`** を見る:
     - `none` = repaint を要求していない (= 入力待ちで就寝) → **正常**
     - `request_repaint_after_idle_upgrade` の遅延 wake → 予定遅延との一致を確認した場合は**正常**
     - 上記以外で 100ms を超える → UI スレッド同期 I/O 退行の疑い
       (docs/ui-responsiveness.md §4)
   - 遅いupdate自体はtail=`none`でも検査する。既知startup gapを認める場合も、
     自動化計画§5と同じ証跡・frame 1→2・測定操作前・500ms上限を確認し、時間を別記する。
   - ⚠️ **「16ms 未満が 97% 以上」を全フレームで測らない。** この比率は**正しい就寝を
     ヒッチとして数える**ので、短い / 暇なセッションほど悪く出る。実際 v3.3.1 の計測では、
     健全そのものの 11 秒セッションが 91.4% になり、v3.3.0 の 97.3% (4,708 フレーム) と
     並べると退行に見えた。**セッションの長さと内容が違う比率は比較に使えない。**
   - 比較してよいのは **直前が `request_repaint` の「実際に描いているフレーム」だけに
     絞った比率**。v3.3.1 のキャッシュ済みフォルダで 92.4% / p95 17ms / **最大 25ms**、
     動画サムネの多いフォルダで 75.6% / 最大 47ms (差はサムネイル処理そのもの)。
   - もう 1 つ見る値: `ui.pre_grid_breakdown` の `total_ms`。UI スレッドの各段の合計で、
     健全なら **p95 0.3ms 以下**。ここが膨らんでいれば同期 I/O 退行が濃厚で、
     どの段かも同じイベントの内訳で分かる。
   - **手動fallbackは先に `.\scripts\build-release.ps1` が要る** (`target\release` のcoreを起動する)。
     `build-dev.ps1` だけでは古いreleaseバイナリが動く。エージェントはこの通常profileを起動しない。
   - `scripts/perf_smoke.sh` は同じ内容の bash 版。中身は「FFmpeg DLL を target/release へ
     コピー → perf log を退避 → core を起動 → 終了後に解析」なので、手で分けて実行してもよい。

9.7. **idle health smoke** (毎リリース必須。静止中の高速 repaint / 再投入ループを検出):
   **正規手順は9.6のReleaseChecks suiteの同一run**。static-foreground／static-background／
   video-pin-background／tray-close-while-loading／tray-close-after-loadingの5ケースをすべて確認する。
   生成fixture・停止済みseed復元・cached読取・tray格納時の要求寿命・source／Finalized-image-backlog、
   exact PID／foreground／UTC測定境界を証明する。5秒warmup／15秒測定と既存閾値を維持する。
   背面はrunner所有の専用窓へ移行するため利用者が他窓へ切り替える必要はない。
   動画ピンは対象workとcached読取、`idle_upgrade_enqueue` **または** `idle_upgrade_ineligible` の
   評価証跡を求める。ineligibleだけを唯一の成立条件にしない。評価未確認はsuiteではSKIPPED／環境不成立。

   **手動fallback／原因診断** (以下はsuiteの正規コマンドではない。利用者が状態を準備する):
   手動の場合も事前に `build-release.ps1` で通常releaseをbuildする。起動は利用者のみとし、
   通常の実設定を開くことと設定保護のルールを確認する。
   ```powershell
   .\scripts\check-idle-health.ps1 -Scenario static-foreground
   .\scripts\check-idle-health.ps1 -NoLaunch -Scenario static-background
   .\scripts\check-idle-health.ps1 -NoLaunch -Scenario video-pin-background -TargetKey '<動画を代表画像に固定したフォルダのパス>'
   .\scripts\check-idle-health.ps1 -NoLaunch -Scenario tray-residency
   ```
   - 最初のコマンドが `mimageviewer-core.exe --perf-log` を起動する。各シナリオを準備して
     Enter を押す。前面シナリオは warmup 5 秒中にアプリへフォーカスを戻し、測定 15 秒の
     開始表示後は入力しない。背面シナリオは別ウィンドウを前面に保つ。
   - **3 番目は `-TargetKey` が必須** (無いと引数エラーで即停止する)。値は perf log の
     thumbnail key (フォルダタイルは `dir::<path>` 形式) への大文字小文字無視の部分一致。
     **ピン作成直後は成立しない** — 作りたてのサムネイルは `from_cache=false` でアイドル
     高画質化の候補にならないため、アプリ再起動後などキャッシュから読み直される状態で測る。
     該当フォルダを用意できない版では、このシナリオを飛ばした旨と理由をリリース記録に残す
     (アイドル高画質化パスに触れていない版なら妥当な waiver)。
   - CPU one-core ratio、update rate、repaint reason streak、同一 thumbnail work、通常 / perf
     ログ増加量のどれかが上限を超えたら exit 1。失敗を閾値緩和だけで通さず、
     `target/idle-health/*-perf.json` の原因と反復 key を確認する。
   - **4 番目はトレイ常駐が既定 OFF** なので、環境設定 →「タスクトレイ常駐」を ON にしてから
     `[×]` で閉じる。最小化では成立しない (Win32 は最小化したウィンドウも `IsWindowVisible` が
     真を返すため)。cache の無い Folder 代表を多数含む一覧で、**未完了要求が残る読込中**と
     **通常ロード / idle upgrade 完了後**の 2 ケースを両方必須とし、同じ
     `-Scenario tray-residency` を 2 回使う。前者は hide をまたぐ要求、後者は Loaded→Evicted の
     GPU 解放後の再取得を検査する。利用者 cache / 設定を消さず、各ケースに使い捨て fixture・
     隔離データまたは利用者が用意した cache 未作成の対象を使う。2 回目が cache hit のみなら
     source の画像 / 保存完了の二通知を検査したことにはならないので cache 状態を分けて記録する。
   - tray のケース名、対象、cache 状態、close 時の requested / backlog、hide から収束までの
     時系列と各 report をリリース記録に残す。close は測定窓外に置き、直後からの work / backlog
     経過も残す。warmup 5 秒 / 測定 15 秒、開始 / 終了の visible windows=0、PID / session と
     閾値は維持する。正当な初回 load が測定中に残れば準備条件と収束時点を記録し、収束後も
     同じケースで静止窓を測る。重複生成を warmup 延長で隠さない。読込中の時点を合わせにくい
     ことを理由に完了後ケースを無効扱いしない。active media は静止ケースに混ぜない。
   - `input.modifier_probe` のうち `trigger=heartbeat` / `keys=[]` / `wheel=false` が揃う
     受動 snapshot だけは操作件数から除く (focus と修飾キーの値は問わない)。key / wheel / 他の
     input、矛盾 heartbeat、欠損・未知形式は入力として FAIL。総 event 数と repaint / work の
     反復検査には snapshot を残す。
   - 手動scriptの `-Scenario` は上記4値だけを受け付ける。perf logのsession PIDと測定対象PIDが
     違えば失敗し、空の測定窓は同一session・ログ完全性を確認できた場合だけ完全sleepと扱う。
     動画ピンは最新folder-load以降の対象workが必須。`idle_upgrade_enqueue`／`idle_upgrade_ineligible`
     の両方が無い場合、手動analyzerは評価未確認をwarningにするので、cached状態を作り直して再測定する。
     正規suiteはcached読取と評価証跡の不足を環境不成立として止める。詳細はidle正本に従う。
   - ZIP / PDF / スマートフォルダ、AI、動画・音楽の非同期経路を変更した場合は、その処理が
     完了した静止状態も追加測定する。詳細は `docs/idle-health-check.md`。

9.8. **Rust 全体テストゲート**:
   ```powershell
   .\scripts\test-full.ps1
   ```
   - `cargo nextest run --workspace --features pack-build-tools` で、workspace全体のlib・bin・integrationと
     単体テストを持つ補助bin 2本を同じlib buildとして実行する。workspace外のvendor 4 crateも
     従来と同じgraph・featureで無filterのlib testを実行する。
   - nextest対象外のdoctestは `cargo test --workspace --features pack-build-tools --doc --no-fail-fast`
     を別段で必ず実行する。各段失敗で後段を止め、retryは0、既存ignored以外の全件を実行する。
     nextest未導入時は `cargo install cargo-nextest --locked` を案内して非ゼロ終了する。
     共通引数・vendor graphの詳細は [開発ビルドとテスト](development-build-and-test.md#リリースゲート) を参照。
   - `build-dist.ps1` が clean 前に自動実行するため、通常は配布ビルドとは別に実行不要。
   - `build-dist.ps1 -SkipRustTests` は、同一ソースが既にこのゲートを通過し、署名や
     packaging だけを再試行する場合に限る。ソース変更後の初回配布では使わない。

### Phase 3: ビルド・配布成果物

10. **配布ビルドは `.\scripts\build-dist.ps1` を使う** (1 コマンドで全体テスト → clean → core → remote → EPUB worker → launcher → ISCC → portable)。
    - build-dist.ps1 は Rust 全体テストを通してから
      `cargo clean --release -p mimageviewer -p mimageviewer-remote -p epub-pdf-worker -p mimageviewer-launcher`
      (+ `target-portable` の `-p mimageviewer -p mimageviewer-remote -p epub-pdf-worker`) してから実コンパイルするので、
      cargo の偽 up-to-date 由来の **stale 配布物を構造的に防ぐ**
      ([docs/release-operations.md](release-operations.md) §2.1 参照)。内部で build-release.ps1 (常駐 mIV を自動停止して
      LNK1104 を回避) と build-portable.ps1 を子 PowerShell で呼び、各 `$LASTEXITCODE` を検査する。
    - VST3 bridge の C++ を変えていなければ `.\scripts\build-dist.ps1 -SkipVst3Bridge` (cmake 再ビルドを省く)。
    - **コード署名は build-dist.ps1 が既定で ON** (Certum Open Source Code Signing 証明書、SimplySign Desktop
      のクラウド鍵)。配布する全 PE に Authenticode 署名 + RFC3161 タイムスタンプを付ける: 単体exe (launcher) /
      core / remote / EPUB worker / susie32 / vst3-host / pdfium / FFmpeg 6 DLL / EffeTune plugin (`.vst3` PE) / `mImageViewer_setup.exe` / portable の各 loose PE。
      **`include_bytes!` で埋め込む物は「埋め込み前」に署名する**
      (内側 vendor PE → core + remote + EPUB worker → launcher → setup.exe の順)。
      EffeTune は vendor 原本ではなく target staging の全 PE を署名し、`MIMV_EFFETUNE_DIR` で
      launcher 埋め込み元を切り替える。`.vst3` の拡張子でも PE なら署名対象とする。
      でないと APPDATA へ展開されたコピーが未署名になり、AV 誤検知
      ([docs/release-operations.md](release-operations.md) §7) が
      再発する。`onnxruntime*.dll` は Microsoft 署名済みなので**再署名しない**、`*.onnx` は PE でないので対象外。
      **前提: 署名前に SimplySign Desktop を起動しログインしておく** (未ログインなら clean 前に `Assert-MivSignReady`
      で即停止)。署名なしで配布物を作るなら `.\scripts\build-dist.ps1 -NoSign`。実装は `scripts\sign-files.ps1`
      (証明書選択は既定 subject `/n "Open Source Developer Taku Sano"`。証明書更新で拇印を固定したいときは
      `$env:MIV_SIGN_SHA1`、TS 変更は `$env:MIV_SIGN_TS`)。ポータブルの vst3-host は署名対応後も当面**非同梱据え置き**。
    - **通常の開発反復は `.\scripts\build-dev.ps1`**。通常feature set（portableなし）を
      `dev-runtime` profileで `target\dev-runtime` へ core + remote + EPUB worker をビルドし、launcherなしで必要な
      FFmpeg DLLだけを変更時に配置する。引数なしの起動は通常版と同じ
      `%APPDATA%\mimageviewer` を使う。launcher／release最適化／埋め込みasset／変更したVST3
      bridgeまで含む実機確認は `.\scripts\build-release.ps1` (incremental・cleanなし、署名は
      既定OFF) を使う。どちらも stale 検出ガードは持たないので
      **配布物の生成には使わない** (必ず build-dist.ps1)。
    - portable core は専用 target dir `target-portable` に分離して焼くので、非portable の
      `target\release\mimageviewer-core.exe` を上書きしない (フレーバー混入防止)。
    - Vector 申請用 zip は build-dist.ps1 では作らない (下記 11 の手順で別途 `Compress-Archive`)。
11. 配布成果物を 4 種類用意する:
    - `mimageviewer.exe` (単体exe版、mikage.to のみ)
    - `mImageViewer_setup.exe` (インストーラ版、mikage.to・窓の杜・Vector 共通)
    - `mImageViewer_installer_v<VERSION>.zip` (Vector 申請用。`mImageViewer_setup.exe` +
      `installer/readme.txt` を同梱。v1.1.0 で `mImageViewer_v<VERSION>.zip` から改名)
    - `mImageViewer_portable_v<VERSION>.zip` (ポータブル版、mikage.to のみ。
      build-dist.ps1 が内部の `.\scripts\build-portable.ps1` 経由で生成。loose 同梱フォルダ全体を含む)

    単体exe (`target\release\mimageviewer.exe`) と `mImageViewer_setup.exe` も build-dist.ps1 が生成する。
    Vector zip は build-dist 後に `Compress-Archive mImageViewer_setup.exe, installer\readme.txt → dist\` で別途作る。
11.5. **ポータブル版 smoke** (v1.1.0+):
    - `dist\mImageViewer_portable_v<VER>\` を **C ドライブ以外の書込可フォルダ** (D:\ / USB 等) へ
      解凍し、`mimageviewer.exe` を起動 → PDF 表示 / 動画再生 / AI アップスケール / Susie の
      いずれかを 1 回ずつ確認。`<exe_dir>\data` が作られ APPDATA が触られないこと、
      インストール版をトレイ常駐させたまま起動しても両方独立に動く (mutex 分離) ことを確認。
      検証チェックリスト全項目は [docs/portable-build-plan.md](portable-build-plan.md) §8。
12. **全 PE / app-local VC runtime gate** — `build-dist.ps1` が installer / portable 完成後に
    `scripts/check-vcrt-pe-dependencies.ps1` を必須実行する。launcher / core / remote / EPUB worker / installer、
    portable 配下、埋め込み元 PDFium / DirectML ORT / FFmpeg / Susie / VST host / EffeTune plugin PE を filesystem から
    列挙し、artifact 別 machine、direct import closure、全 input SHA-256 を report に残す。
    4 CRT は Microsoft 署名、manifest exact hash、全4本同一版かつ最低 14.44 を必須とし、
    `onnxruntime*.dll` も Microsoft 署名を必須にする。未知の VC runtime import、companion CRT 欠落、
    package copy の hash 不一致は配布停止。TRT pack は setup / build / upload の各入口で同じ gate を通す。
12.5. **コード署名の回帰チェック** — 配布成果物 (単体exe / setup.exe / portable の mimageviewer.exe) に
    `signtool verify /pa /v <exe>` を走らせ、`Open Source Developer Taku Sano` 名義の証明書チェーン
    (Certum Code Signing → Certum Trusted Network CA) と **RFC3161 タイムスタンプ**が付いていることを
    目視確認する。build-dist は署名時に検証も済ませているが、最終成果物で 1 度確認する。未署名・失効なら
    SimplySign Desktop のログイン状態を確認 (証明書は 1 年ごとに更新 → 更新時は `sign-files.ps1` の
    既定 subject が一致するか、または `$env:MIV_SIGN_SHA1` を新拇印に更新)。

### Phase 4: GitHub Release 公開

13. ローカルで `git tag v<VERSION>` → `git push origin v<VERSION>` (GitHub `main` も同期)
14. GitHub Releases UI で新リリースを作成。**body の出所は Phase 0 で確定した版に従う**:
    - **通常 (CHANGELOG.md のセクションが 8KB 以内)**: CHANGELOG.md の該当 `### vX.Y.Z` セクション
      本文をそのままコピペ。
    - **短縮版を作った場合 (CHANGELOG.md のセクションが 8KB 超)**: Phase 0 で作成した
      `docs/release-body-<version>.md` の本文をコピペする (CHANGELOG.md のフル版ではなく
      **こちらを使う**)。短縮版が存在するかは `ls docs/release-body-<version>.md` で確認。
      別セッションで Phase 4 を実施する場合もこのファイルの有無で判断できる。
    - どちらの場合も、この body がアプリ内アップデート通知にそのまま表示される。
      **Phase 3 の配布成果物 4 種類すべて** を Assets として添付する:
      `mimageviewer.exe` (単体exe版) / `mImageViewer_setup.exe` (インストーラ版) /
      `mImageViewer_installer_v<VERSION>.zip` (Vector 申請用 zip) /
      `mImageViewer_portable_v<VERSION>.zip` (ポータブル版)。過去リリース (v1.5.0 等) は
      この 4 点を添付済みなので、`gh release view v<前版> --json assets` で添付漏れがないか
      照合する。
    - 実例: v1.0.0 は短縮版 [docs/archive/release/release-body-v1.0.0.md](archive/release/release-body-v1.0.0.md) を使用。
15. **表示崩れの確認は自動テストが持つ。目視は任意。**
    `changelog_markdown::tests::the_newest_changelog_entry_only_uses_markup_this_renderer_handles`
    が CHANGELOG.md の最新節を読み、更新通知ダイアログの描画器が解釈できない記法
    (リンク・画像・閉じ忘れた `**` など) が混ざっていないかを機械的に見る。
    `cargo test` で毎回走るので、**公開前**に落ちる。
    - 以前はここを「公開後に別マシンで目視」としていたが、目視は体裁しか見られず、
      しかも公開してからでないと試せなかった (2026-08-30 に廃止)。
    - 短縮版を使った場合だけは、目玉の新機能が 8KB で切れていないかを別途見る
      (バイト数は Phase 0 で測っているので、通常は測定で足りる)。
    - 実際の見た目を見たいときは、公開後に**手元の 1 つ前のビルド**を起動すれば出る。
      別マシンは要らない。

### Phase 5: 配布チャネルへの反映・申請 (公開後)

GitHub Release 公開後、各配布チャネルへ反映・申請する。**Vector と MS Store は忘れやすいので、
リリースのたびに必ずこのリストを通し、ユーザーに作業を案内すること。**

16. **mikage.to へ反映**:
    - 3 直接DL成果物 (`mImageViewer_setup.exe` / `mimageviewer.exe` /
      `mImageViewer_portable_v<VER>.zip`) を配置。
    - 製品ページ (`htdocs/mimageviewer/index.html`) のダウンロード欄・バージョン表記・「最終更新」
      日付を新版に更新。ダウンロード欄に **「Microsoft Store でも入手可能」バッジ** (`.btn-store`、
      リンク先 `https://apps.microsoft.com/detail/xp8jlwdwv5ls01`) が入っていることを確認。
17. **Vector 申請**: `mImageViewer_installer_v<VERSION>.zip` (`mImageViewer_setup.exe` +
    `installer/readme.txt`) を申請。readme の版表記更新を忘れずに (下の「Distribution」節参照)。
18. **窓の杜** (任意): これまで見送り実績が多い。掲載する場合はインストーラ zip を申請
    (過去の申請知見は [docs/release-operations.md](release-operations.md) §8 参照)。
19. **Microsoft Store 更新申請** (区切りの良い版で。毎リリース必須ではない):
    - **前提**: EXE/MSI 掲載は **Store が既存ユーザーを自動更新しない** (自動更新は MSIX のみ)。
      **既存ユーザーは mIV 自身の更新通知で自己更新**するので、Store 更新は「新規 Store インストール
      の初期版を新しく保つ」ためのもの。**毎リリースで出す必要はなく、区切りの良い版で更新すれば十分**。
    - **① 版付きの直リンクにインストーラを配置**: 署名済み `mImageViewer_setup.exe` を mikage.to の
      版付きパスに**直リンク**で置く。例: `https://mikage.to/mimageviewer/download/v<VER>/mImageViewer_setup.exe`。
      - ⚠ **GitHub Release の URL は使えない** (実体 CDN へ 302 リダイレクトし、Store に
        「リダイレクトのないダウンロード URL を指定して」と却下される)。
      - ⚠ **非バージョン名の setup.exe (上書き運用) も使わない** (Store は提出後のバイナリ変更を
        許さないので、版でパスを分ける)。**提出後はそのファイルを消さない・上書きしない**
        (Store が再DLして再検証する)。
      - リダイレクト無しを確認: `curl -sI <URL>` が `200 OK` (301/302 が出ないこと)、
        `Content-Length` が署名済み setup.exe と一致すること。
    - **①.5 申請前にクリーンな Windows で起動を確かめる**: 署名済み setup.exe を Windows Sandbox (VC++ ランタイム無し) に
      同じサイレント引数で入れ、初回・2 回目の起動で窓が出て応答し続けることを見る (v3.6.0 / v4.1.0 は「起動中のまま」で却下された。
      サブ PC の手順は `C:\miv-sandbox\`、経緯はバックログ §1.241)。Sandbox では WebView2 が動かないので、音響調整と EPUB 変換の失敗は対象外
      ([EffeTune 計画 §10](effetune-integration-plan.md#10-決定済みの配布方針と残る対象外事項))。
    - **認定の注意事項**: EXE/MSI アプリは「プロパティ」ページの「認定の注意事項」(2,000 字) に書く (MSIX の「提出オプション」ではない)。
    - **② Partner Center で更新**: [partner.microsoft.com](https://partner.microsoft.com/) →
      mImageViewer → 「アプリを更新」→ **パッケージ**のパッケージ URL を新 URL に差し替え →
      **各ページで必ず「下書きの保存」** (保存せず「次へ」だと入力が消える) → 「すべて保存」→
      **Package validation を「実行」** (署名・サイレントインストール・リダイレクトを検証、~30分〜数時間) →
      合格を確認 → **送信**。
    - **③ 認定待ち**: 新規提出と同様 **約1〜3営業日**。合格でメール通知。年齢区分アンケートの回答が
      変わらなければ **IARC レーティングはそのまま引き継がれる** (mIV はローカルビューアなので通常不変)。
    - **確定値の控え** (変更なければ毎回同じ): Architecture=**x64** / Language=**ja** /
      App type=**EXE** / Installer parameters=**`/VERYSILENT /SUPPRESSMSGBOXES /NORESTART`** /
      Installation successful=**0** / プライバシーURL=`https://mikage.to/mimageviewer/privacy.html`。
      Microsoft Store ID=**`XP8JLWDWV5LS01`**、公開ページ=`https://apps.microsoft.com/detail/xp8jlwdwv5ls01`。
      コード署名は build-dist.ps1 が実施済み (Phase 3 / 「code signing」)。詳細な経緯・ハマりどころは
      [docs/release-operations.md](release-operations.md) §8 参照。


### Phase 6: 公開後の後始末 (毎リリース、2026-10-07 利用者指示)

GitHub Release の公開と Phase 5 の案内が済んだら、同じ作業の中で次を片付ける。放置するとビルドキャッシュは
数百 GB に膨らみ (2026-10-07 時点で本体 `target` 523 GB、worktree 12 個で計約 785 GB)、バックログは出荷済み項目で読みにくくなる。

20. **ビルドキャッシュの整理** (作り直せるので確認不要):
    - 本体 worktree の `target/*/incremental` (`debug` / `dev-runtime` / `release` / `dev-runtime-staging/*`) を削除する。
      配布物 (`target/release/mimageviewer*.exe`、`installer/Output/`、`dist/`) は Phase 4 の添付と Store 申請が済むまで消さない。
    - 撤去しない worktree も、しばらく使わないなら `target/` を削除してよい (次回ビルドが遅くなるだけ)。
    - 削除前後のサイズを報告する。
21. **完了した worktree の撤去** (元に戻せないので、候補一覧を利用者に見せて了承を得てから):
    - `git worktree list` の各 worktree について、HEAD が master に取り込み済みか (`git merge-base --is-ancestor`)、
      未コミット変更が無いかを確認して候補を出す。保留中の作業 (backlog-on-hold の検証ラボ等) と未統合ブランチは残す。
    - 撤去は必ず `scripts/safe-worktree-remove.ps1` 経由 ([git-workflow.md](git-workflow.md) の「worktree + Windows junction の地雷」節)。
    - **`Filename too long` で git が途中まで消して止まることがある** (2026-10-07、EffeTune bundle などの深いパス)。
      その場合 worktree の登録は外れ、フォルダの中身が一部残る。reparse point が無いことを
      `Get-ChildItem -Recurse -Attributes ReparsePoint` で確かめてから、`cmd /c rmdir /s /q "\\?\<フォルダ>"` で残りを消し、
      `git worktree prune` と `git branch -d` を行う。先に `target*` を同じ方法で消しておくと失敗しにくい。
      取り込み済みのブランチは `git branch -d` で消す。`.claude/worktrees/` 配下は Claude アプリが作ったものなので、
      アプリ側の片付け (`clean_up_worktrees`) を優先する。
22. **バックログの整理**: 今回出荷した項目を `docs/next-release-backlog.md` から削除し (経緯は git 履歴と
    CHANGELOG に残る)、出荷しなかった残件・見送りは見出しの状態を更新する。報告者に返信済みかどうかの記録は残す。
    削除した項目の一覧をコミットメッセージに書く。
23. **メモの更新**: `project_next_version_plan.md` 等の進行メモを「公開済み」に更新し、次の版の予定を書く。

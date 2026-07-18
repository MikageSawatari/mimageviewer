# 次リリース検討バックログ

このファイルは、まだ着手していない作業候補だけを置く恒久バックログ。
完了した項目はコミット履歴・リリースノート・個別設計メモに任せ、このファイルからは削除する。

運用ルール:

- 着手前に `docs/README.md` から該当領域の設計ドキュメントを読む。
- 着手中のものだけ `対応中` と明記してよい。完了したらこのファイルから削除する。
- 判断保留・見送りの理由は、次に再判断する人が困らない最小限だけ残す。
- 依存ライブラリ更新は `CLAUDE.md` のリリース手順チェックリスト Phase 2 と整合させる。

---

## 1. 優先候補

### 1.7 detached 中の発火面解決の残り (? / トースト / スタック) — BA 報告

- 残り (P3、発火面の window_id 粒度が必要):
  1. **? キーのショートカット一覧**: detached 中にグリッドから開くと FS 用一覧が
     出る。修正には発火元の面情報を consume サイトから通す必要がある。
  2. **トーストの面粒度**: 発火面のビューアを閉じた後に完了したバッチのトーストが
     不可視のまま消える / active detached + main fullscreen 同時表示時に Viewer 面が
     両ビューアに出る (2 値 enum の粒度の限界)。許容中。
  3. **複数ウィンドウ×スタックの理想形**: スタックをクリックするとメイン一覧が
     フラット読書ビューに切り替わる (フル機能と同じ挙動)。理想は「メインは集約のまま、
     窓側だけフラット文脈を持つ」だが、窓が自前のフラット items 文脈 (bundle 化された
     stack 状態) を持つ設計が必要 (複数ウィンドウの bundle 化と同系統)。現行でも
     Shift+↓↑ ジャンプは動作する。
- 優先度: P3。

### 1.9 parked 窓のリソース制御 (サムネ pipeline 停止 / VRAM 合算予算) — 角度⑥送り

- 出典: v2.3.0 角度⑥レビュー (Sol/Terra 一致、docs/review-v2.3.0/sol-angle-reviews.md)。
  いずれも bounded な効率問題でデータ喪失は無し。park/再活性ライフサイクルの構造に
  踏み込むため、リリース直前パッチではなく detached リワーク後続で設計対応する。
  1. **P2: park してもサムネ pipeline が止まらない**: `pause_background_work_keep_current_frame`
     は fullscreen/AI 系のみ cancel し、bundle の `cancel_token` / `reload_queue` /
     `heavy_io_queue` に触れない。park 時点で積まれていたデコードが走り切り、結果
     `ColorImage` が誰も poll しない rx に溜まる (窓の再活性化 / close まで保持)。
     対応案 = park 時に queue を drain (worker pool は殺さない)。ただし Requested 状態の
     サムネが再活性化時に再要求される仕組みの確認が必要 (state が Requested のまま
     queue から消えると復帰後にロードされない恐れ)。
  2. **P2: サムネ VRAM 上限が文脈単位**: `update_keep_range_and_requests` の予算は
     mounted 文脈にしか効かず、parked bundle N 個がそれぞれ上限近くまで保持し得る
     (動画サムネは eviction 対象外なのでフォルダサイズ分)。対応 = 全 bundle 合算の
     予算会計 + cross-bundle eviction (リワークの資源予算ステージ)。
  3. P3: `display_px_shared` が App-global のため、detached 文脈のデコード解像度が
     メイングリッドの表示密度に引きずられる (適用ミスは無し、品質/CPU の無駄のみ)。
- 関連: v2.3.0 で対応済みの境界 = bundle Drop の解放 (角度①でクリーン確認)、
  文脈別 channel/cancel/世代 (P2-9)。

### 1.10 終了時の削除 worker 未調整 — 角度⑤送り (P2、実害小)

- 出典: v2.3.0 角度⑤ Sol P2。数百件削除の実行中に終了すると、実行中の
  IFileOperation チャンクは完走するが後続チャンクは開始されず、部分削除の最終報告と
  完了後クリーンアップ (resume purge 等) が走らない。一覧は次回起動の走査で実態に
  収束するため破壊は無し。対応案 = 終了時に cancel を立てて現行チャンクの完了を
  短時間待つ + 次回起動時の注意トースト。v2.2.0 出荷時からの既存挙動。

### 1.11 視聴中ファイル削除の初回失敗 (sharing violation リトライ)

- 出典: v2.3.0 実機検証 §4 #24 (2026-07-10)。別窓で再生中のファイルを削除すると、
  (A)(B) 実装どおり窓は閉じるが、プレイヤーのファイルハンドル解放が非同期のため
  初回の削除が共有違反で「削除に失敗しました」になることがある (2 回目で成功)。
  ユーザー許容済み (「この動作でも大丈夫」)。
- 対応案: 削除 worker 側で ERROR_SHARING_VIOLATION 時に短時間リトライ
  (例: 200ms×5 回バックオフ)。削除対象が再生解放直後のケースだけ効き、
  他の共有違反 (他プロセス占有) はリトライ後に従来どおり失敗表示。

### 1.12 detached 静止画窓から音声/動画へのフォルダ内ナビ不可 (メディア昇格導線)

- 出典: v2.3.0 実機検証 §5 #33 (2026-07-10)。別窓 (静止画) で ↑↓ ナビ中、音声/動画の
  アイテムへは移動できない (静止画窓はメディア再生セッションを持てない設計境界)。
  音楽ビュー→画像の方向は移動できるため非対称。ユーザー許容済み (「一旦これでもよさそう」)。
- 対応案: detached リワークの後続ステージで「静止画窓がメディアに到達したらメディア
  セッションへ昇格 (または既存メディア窓へ委譲)」の導線を設計。凍結ルール下では
  症状パッチを入れない。

### 1.13 duration 不明/不正 MPEG-PS のシークバー不能

- 背景: `sample.mpg` で実機確認済み。duration が不明または不正な MPEG-PS は
  シークバーで移動できない。VLC は同じファイルをバイトシーク + PTS 再スキャン方式で
  シークできる。
- 対応案: decode EOF で実尺を学習して `VideoInfo` / HUD の duration へ反映するか、
  duration を信頼できない場合にバイト位置シークへフォールバックする。
- 裁定: v2.2.0 以前から同挙動で、ユーザー裁定により次版送り。優先度 P3。
  final-report 追補 5 の P3 参照。

### 1.17 UI レイアウト崩れ (文字はみ出し / 見切れ) の自動検出 QA → その後に英語対応を再検討

- 背景: 英語対応 (UI ローカライズ) を検討したが、AlternativeTo は「英語 UI 必須」
  (FAQ: DB 登録アプリは英語対応が必須) のため、日本語のみの現状では掲載できない。
  一方、多機能ゆえに英語 UI を実機で全画面目視 QA するのは負担が大きく、しかも日本語でも
  文字はみ出し / 見切れのレイアウトバグが繰り返し発生している。そこで **まず言語非依存の
  「レイアウト崩れ自動検出」QA ツールに投資**し、日本語バグに即効かつ将来の英語化の
  「目視 2 倍」問題を先に解消する。英語対応の可否はその後に再判断する。
  (2026-07-13 相談。ユーザー方針: QA 投資を先行 → その後に英語対応を再検討。今は別作業中のため別途着手)
- 現状 (確認済み):
  - egui は文字配置時に galley サイズが取れ、painter は clip rect を持つため
    「galley 幅 > 割り当て幅」で見切れを機械判定できる。
  - UI スナップショット基盤 `tests/ui_snapshot.rs` (egui_kittest) が既にある (回帰検出向け)。
  - 文字列は現状ハードコード (i18n 未 externalize)。見切れ計装は現行日本語のままでも動く
    ので、i18n とは分離して先行できる。
- 方針 (QA ツール先行 → 英語再検討):
  - QA ツール (言語非依存・日本語バグにも効く):
    1. **見切れ計装**: 描画時に galley 幅 vs clip/available 幅を比較し、超過した文字列 +
       ウィジェット位置をログ列挙する。全画面を人が見なくても崩れ候補の一覧が出る。
    2. **疑似ローカライズ (幅ストレス)**: UI 文字列を ~1.4x 長い擬似文字列に置換したビルドで
       幅不足のウィジェットを炙り出す (英語が長くなる前提の先行テスト。日本語のままでも
       将来はみ出しやすい箇所を潰せる)。
    3. 既存 egui_kittest スナップショットを併用 (変化検出。最終は差分画像を目視)。
    4. (任意) スクショ自動キャプチャ + ビジョン判定で「切れ / 重なり」を triage。
    - 限界: 得意なのは見切れ / はみ出し / 行あふれ。位置ズレ・不格好な折り返し・パネルの
      重なりは取りこぼしがあり、多少の目視が残る。
  - 英語対応 (QA ツール整備後に可否判断):
    - 前工程 = 文字列の externalize (i18n 化)。
    - AI 一括翻訳 + **用語集** (見開き=Two-page spread 等を先に固定) + 各文字列の文脈付与で
      用語ゆらぎ / 品詞ズレを抑える。
    - **変数整合チェック**: `%s` / `{}` / 改行 の個数・種類が元と英訳で一致するか機械照合。
    - **逆翻訳 QA**: 重要文字列 (削除 / 上書き / 警告 / エラーダイアログ) に限定して英→日へ
      訳し戻し、元日本語と比較する。英語力が無くても意味ズレを検知できる (日本語同士の比較)。
    - 実機確認は見切れ計装 + 疑似ロカライズで大半を機械化し、目視は残差のみ。
    - "English is machine-assisted; feedback welcome" と明示し、OSS の Issue / PR で継続改善。
- AlternativeTo 連携: 英語 UI が入れば掲載可。申請素材 (Tagline / Description の v2.3.0 版
  = 音楽再生 + VST3 入り / Website / Source / License Free+OSS(MIT) / 画像 = icon.png + ss_*.png
  含む ss_music.png / Alternative to = NeeView・MangaMeeya・Leeyes・ZipPla・Honeyview・
  ImageGlass・IrfanView・XnView MP・nomacs・FastStone) は準備済み。GitHub Topics / 英語 README
  サマリ / egui Showcase / awesome-egui PR は UI 言語非依存の枠なので実施済み。
- 規模 / リスク: QA 計装 = Medium (描画経路への hook + 幅バジェット定義)。英語化 = Large
  (externalize + 翻訳 + QA 工程)。QA ツールは日本語バグにも効くため単独で投資回収がある。
- 優先度: P2 candidate (QA ツール)。英語対応は QA ツール整備後に再判断 (現時点は保留)。

### 1.20 ✅ レガシー `GridItem::ZipSeparator` の完全撤去 (v2.5.0)

- 状態: 完了 (2026-07-17)。
- 出典: v2.4.1 前レビュー (2026-07-16)。`ZipSeparator` は旧 ZIP フラット展開で章見出しを
  1 セルとして挿入するための疑似アイテムだったが、v1.3.0 のネスト ZIP ツリーナビ移行後は
  `ZipTree` / `ZipDir` で現在階層だけを表示するため、本番経路では生成されない。
- 互換性判断: `GridItem` は永続化形式ではなく、snapshot / viewer context 内の保持も同一プロセス中の
  一時状態だけである。旧設定・DB・セッションから `ZipSeparator` を復元する経路はなく、variant を
  残す後方互換上の必要はない。
- 実施内容:
  1. `GridItem::ZipSeparator` variant と専用描画、ナビ、検索、メタデータ、スナップショット、
     コンテキストメニュー等の分岐を撤去した。
  2. 人工的に旧 variant を生成するテストと、到達不能だったスライドショー / 連続読みの章カード契約、
     専用 enum 値・helper を撤去した。
  3. `docs/spec.md` / `docs/virtual-folders.md` などの現行仕様を更新し、過去の設計文書には撤去済みと
     明記した。
- 維持した不変条件: `ZipTree` / `ZipDir` の階層移動、`ZipImage` の表示順、見開き・連結読み、
  検索・フィルタ、通常画像 / PDF の挙動は変更しない。ソースの `ZipSeparator` 参照はゼロとし、
  旧 facet 設定値は `Unknown` として安全に読み込む。

### 1.21 親コンテナの代表サムネイルに編集プレビューを反映

- 出典: v2.5.0 リリース前実機確認 (2026-07-17)。ZIP 内画像を編集しても、
  親階層の ZIP セルの自動代表サムネイルには編集結果が反映されない。
  編集済みページを <kbd>P</kbd> で代表サムネに固定した場合も、同じページの
  未編集画像が表示される。
- 調査結果:
  - ZIP 内の通常 `ZipImage` サムネイルは `edit_preview_key` を持ち、編集プレビューを
    通常 catalog より優先して読み込む。
  - `folder_thumb_pins.db` は対象の参照 (entry / page / path) だけを保存し、
    `apply_folder_thumb_pin` が作る親セル用 `LoadRequest` は `edit_preview_key = None`
    のため、元画像から代表サムネイルを作る。
  - 編集プレビューの Saved / Invalidated イベントは、現在の `items` と
    `page_path_key` が直接一致するページセルだけを無効化する。親 Folder / ZipFile /
    PdfFile セルまでは更新が伝播しない。
  - 共通の親代表経路のため、直接画像を固定した Folder、PdfPage を固定した
    PdfFile、変換アーカイブ、ネスト ZIP も対象候補に含める。
- 方針 (二段階):
  1. **先行候補: 手動ピンの反映**。cascade 解決後の leaf から canonical page key を
     導出し、代表サムネイル要求へ渡す。編集プレビューの保存・無効化時に、
     そのページを固定している親セルも evict / reload する。
  2. **後続候補: 自動代表選定の反映**。worker 内で leaf を選定した後に page key を
     組み立て、対応する編集プレビューがあれば raw decode / catalog より優先する。
     自動選定結果と親キャッシュの無効化条件も同時に設計する。
- 不変条件 / 確認:
  - サムネイルへ反映するのは現行の編集プレビュー内容 (erase / local_adjust /
    conceal / crop / comic 注釈) とし、親代表の色調補正は現行仕様を勝手に変えない。
  - ZipEntry / PdfPage / 直接 Image、ネスト ZipDir、変換アーカイブ、編集更新・
    編集解除・キャッシュヒットを回帰テストする。
  - UI スレッドで ZIP/PDF 列挙や SQLite 単件照会を追加しない。
- v2.5.0 裁定: 編集データ・書き出し・ピン対象の参照は正常で、表示上のみの
  既知の制限。実機検証でその他の問題がなければ 2026-07-18 付けの v2.5.0 を
  ブロックしない。
- 規模 / 優先度: 手動ピン = Medium / P2 candidate、自動代表 = Medium〜Large / P3。

### 1.22 本として扱うコンテナのページ数を一覧情報へ表示

- 出典: mImageViewer 専用スレ 72 (2026-07-18)。「ツールチップに表示する項目に、
  ZIP 内のページ数も表示してほしい」。検討時に、PDF と「画像のみのフォルダを本として扱う」
  対象にも同じ意味のページ数が必要で、ツールチップだけでなく詳細ビューと下部情報バーでも
  頻繁に確認できる共通項目とする方針を確定した。
- UI / 設定:
  1. 共通項目名は **「ページ数」**。ZIP/PDF/画像フォルダごとの別列にはしない。
  2. 選択情報の項目に `thumb_tooltip_show_page_count`、詳細列に
     `DetailsColumnId::PageCount` / `details_show_page_count` を追加し、いずれも既定 ON。
     既存ユーザーにも `serde(default = "default_true")` と列順 sanitize で追加する。
  3. 詳細ビューと下部情報バーは既存の `DetailsColumn` / `details_row_data` を共有し、
     下部情報バー独自のページ数設定や書式を作らない。詳細列の表示・順序・幅に追従する。
  4. ツールチップは `ページ数 123`、詳細列/下部情報バーは `123` を基本とする。
     読み込み中は `...`、非対象または取得不能は `-`。値が揃った後はページ数ソートも可能にする。
- ページ数の定義:
  1. **ZIP / CBZ**: 実際に本を開いたときの閲覧ページ列と一致させる。直下画像だけでなく、
     現行 `enumerate_image_entries` がページ化するネスト ZIP 内画像も含める。central directory の
     浅い件数だけを表示する近似実装は行わない。
  2. **PDF**: 文書の総ページ数。既存 catalog `pdf_meta` を最優先で再利用し、miss の場合だけ
     background で列挙する。保存パスワードも既存 cache もない保護 PDF は、一覧表示を理由に
     パスワードダイアログを開かず `-` とし、ユーザーが正常に開いた後の cache を再利用する。
  3. **画像のみフォルダ**: `auto_fullscreen_image_folders_enabled()` が true で、実際に開いた結果が
     1 件以上の通常画像だけになるフォルダを対象とする。サブフォルダ、動画/音声、ZIP/PDF/
     対応アーカイブが混ざる場合は非対象。hidden 設定、動画派生除外、画像拡張子重複除外など、
     `load_folder_with_scan` 後の表示ページ数と同じ規則を共有し、別の簡易判定を増やさない。
- 取得 / cache 方針:
  - `DetailsLazyMeta` / `DetailsMetaTarget` / `run_details_meta_load` の必要フィールド集合へ
    `page_count` を追加し、UI スレッドではファイル open、ZIP/PDF 列挙、子フォルダ走査を行わない。
  - サムネイル表示では選択中の 1 件だけを要求する。詳細表示では可視行を `Normal`、画面外を
    `Low` として順次処理し、フォルダ移動・列非表示で cancel、世代不一致の結果は破棄する。
  - PDF は既存 `pdf_meta` を使用。ZIP と画像フォルダには永続 container-page cache を追加し、
    パス、種別、mtime、file_size と、画像フォルダの表示結果へ影響する設定 fingerprint が一致する
    場合だけ hit とする。フォルダ内容変更や relevant 設定変更後に古い件数を表示しない。
  - 一覧に大量の ZIP/PDF/候補フォルダがある場合でも、default ON による再列挙地獄や UI/I/O
    飽和を起こさないことを実装条件とする。失敗を永続的な成功値として cache しない。
- 回帰確認:
  - 通常/空/破損/保護 PDF、通常 ZIP、ネスト ZIP、画像 0 件・画像のみ・混在フォルダ、
    hidden/重複除外設定、内容更新後の cache invalidation、フォルダ移動中 cancel を unit/integration
    test で確認する。
  - ツールチップ、詳細列、下部情報バーの値と表示設定 roundtrip、ページ数ソート、巨大 ZIP/
    大量コンテナ一覧での UI responsiveness を確認する。
- v2.5.0 裁定: default ON では永続 cache と可視行優先の非同期処理が必須で、リリース当日の
  軽微追加に収まらないため見送り。次バージョンで独立機能として実装する。
- 規模 / 優先度: Medium〜Large / P2 candidate。UI 列追加は Low、主な難所は ZIP/フォルダの
  正確な列挙、cache invalidation、default ON 時の I/O 制御。

### 2.1 folder pane scan worker の thread 構成判断

- 背景: `scan_real_subfolders` はノードごとに短命 thread を spawn する。
- 現状: `folder_pane/scan_subfolders` perf event で ms / entry 数 / dir 数 / cancel / error を記録済み。
  cancel 付きで thread leak は見えていない。
- 方針:
  - 低速共有や大量ノード展開で遅い scan / concurrent scan が見えた場合だけ、dispatcher / pool 方式へ寄せる。
- 優先度: P3。

## 3. 補正 / AI

### 3.1 local-adjust layers の入場時同期 DB 読み

- 背景: フルスクリーン入場初回フレームで `LocalAdjustDb::get_layers` を同期実行する。
- 現状: フォルダ open 一括読みを避けるための意図的 tradeoff。
- 方針:
  - 数十 MB 級ページで hitch が報告 / 計測された場合に worker 化する。
  - read-only 経路の not-loaded は現状どおり None 返しを維持する。
- 優先度: P3 monitor。

### 3.2 補正パラメータ変更後に AI アップスケールキャッシュが優先される疑い (再現待ち)

- 背景: 5ch レス 792 の追跡項目。「画像補正パラメータを変更しても AI アップスケールキャッシュが
  優先され、ページを行き来すると変更が効いていないように見える」という報告。
- 現状 (2026-06-18): 通常環境と v1.7.0 ポータブル版の追加テストで再現せず。現在の設計では、
  色調補正や AI 設定の変更は final AI / final composite cache のキー差分または明示クリアで反映される。
  一方、最終段スマートシャープなど post-filter 系は final AI cache を再利用して final composite だけを
  作り直す。さらに AI アップスケール出力にはスマートシャープを適用しない固定仕様なので、
  操作内容によっては「変わらない」ように見える場合がある。
- 方針: 具体的な再現手順が出るまではコード修正しない。再報告時は、変更したパラメータが色調補正 /
  AI ON/OFF / デノイズ / post-filter / スマートシャープのどれかを最初に切り分ける。
- 優先度: P3 monitor / 再現待ち。

### 3.4 トーン漫画の縮小モアレ対策 (手動 post_filter 縮小 → 将来 LOD)

- 正本: **`docs/downscale-moire-lod-plan.md`** (調査結果 + 対策方針)。以下は要約。
- 背景: トーン (スクリーントーン) を貼った漫画を縮小表示するとモアレが出る
  (ユーザー報告 2026-07-07)。トーンの高周波が縮小で折り返す aliasing。
- 現状 (調査済み):
  - 縮小は `src/fast_resize.rs` (`fast_image_resize`, Bilinear / Lanczos3 の 2 択) に集約。
  - **主因はフルスクリーン**: `fs_cache` は原寸 (最大 8192px) を保持し、`draw_fs_image` が
    原寸テクスチャを GPU の **naive bilinear (mipmap なし)** で大縮小するため、
    縮小率が 0.5 を切るとトーンが折り返す。サムネは Lanczos3 の負ローブで副次的に出る。
  - `TextureOptions.mipmap_mode` は **egui-wgpu では効かない** (epaint が「egui_glow のみ」と
    明記、renderer は `mip_level_count:1` 固定・`create_sampler` が mipmap_mode 無視)。
    実ソースで確認済み → native mipmap は不可。
- 方針 (段階投資):
  - ⓪ **(実装済み v2.3.0、`4be5944a`) 手動 post_filter 縮小フィルタ**。ユーザーが選ぶ post_filter
    として 1/2 / 1/4 縮小 (`PostFilter::Downscale2x` / `Downscale4x`) を追加し、フィット表示の
    モアレを自衛できる。既存 T (`FsPostFilterNext` の `PostFilter::ALL` 巡回) に乗る。制約:
    フルスクリーン専用 (サムネ非適用) / 他 post_filter と排他 / 静的倍率。以下 ①〜③ が残作業。
  - ① **CPU 2 段 (原寸 + 表示解像度版)** から。フィット表示は倍率固定なので worker で
    Lanczos 縮小した 1 枚を貼り、ズーム拡大時だけ原寸へ持ち替える。原寸→8192 縮小は
    既に `clamp_dynamic_for_gpu` が worker でやっているので **UI ブロックなし**。
    フィット / 連結 / 見開きのモアレの大半がこれで消える。
  - ② 足りなければ **CPU N 段の手動 LOD (手動 mipmap)** に拡張。
  - ③ ズーム往復の滑らかさまで要れば **GPU pyramid + native texture 登録** (コスト大)。
  - 縮小は post_filter の**前**・表示解像度基準で掛けるとモアレに強い (疑似カラー等の
    規則パターン系は原寸適用だと自らモアレる)。編集は原寸のまま、LOD は表示専用派生。
  - `draw_fs_image` は `handle.size_vec2()` を論理サイズに使う (10+ 経路) ので、
    「レイアウトは元サイズ・描画 handle だけ差し替え・UV 0..1」の分離が必須。
    ルーペは原寸固定、pixel grid は論理サイズ必須で LOD 除外。
  - 連結読み / 見開きはページ単位の個別テクスチャなので案がそのまま乗る (縮小率が高いぶん恩恵大)。
- 規模 / リスク: CPU 2 段=中 / CPU N 段=中〜大 / GPU pyramid=大。押し上げ要因は
  `size_vec2()` 経路の分離・`final_composite_cache` 回帰テスト群・編集全経路の再生成配線・
  **detached-rework 凍結ルール** (表示テクスチャ経路を共有するため、着手前に
  `docs/detached-rework-plan.md` §2 で境界を確定する)。
- 優先度: ⓪ 手動 post_filter 縮小フィルタ (回避策) = **実装済み (v2.3.0)**。①〜③ の LOD による
  根本的解決 = P3、将来再検討 (画質要望の蓄積時 or detached-rework 完了後)。

---

## 4. 入力カスタマイズ / マウス / ゲームパッド

### 4.1 Shift / Alt + ホイールのカスタマイズ再設計

- 背景: v1.7.0 のリングショートカット / マウスボタン実装中に、Shift / Alt + ホイールのペアバインドを
  追加候補にしたが、実機確認で動画まわりの退行リスクが高いと判断した。
- 方針:
  - v1.7.0 では公開 UI / 入力経路から外し、通常ホイール、Ctrl+ホイール、中ボタンドラッグの既存挙動を維持する。
  - 将来再開する場合は、グリッド / 画像フルスクリーン / 動画フルスクリーンを別々に設計する。
  - native video overlay の consumed wheel、modifier 転送、動画タイルの Ctrl+ホイール、編集パネル / スクロールパネルとの
    優先順位を先に決める。
- 実装メモ: `ring_shortcuts.shift_wheel_pair` / `alt_wheel_pair` は互換読み込み用フィールドとして残すが、
  現行 UI / 入力経路からは参照しない。
- 規模 / リスク: Medium / 中。動画系の手動確認を含めて別タスクで扱う。

### 4.2 グリッド右ドラッグ開始セルを選択するオプション

- 出典: mImageViewer 専用スレ 73 (2026-07-18)。ファイル上からリングショートカット /
  マウスジェスチャを開始しても、コマンドが以前の選択ファイルへ適用されるという要望。
- 背景 / 裁定:
  - 現行はセルの `secondary_pressed` で右ドラッグを開始して開始セル idx も記録するが、
    選択変更は通常の `secondary_clicked` (ボタンを離した時点) まで行わない。そのためドラッグが
    成立した場合、選択対象は以前のままになる。
  - 一方、現在の選択を保ったまま一覧の空き位置や別セル上から自由にジェスチャしたい用途もある。
    一律に押下セルへ変更せず、操作カスタマイズの「マウス右ドラッグ」に opt-in 設定を追加する。
- 仕様:
  - `RingShortcutSettings` にグリッド専用 bool
    `select_grid_item_on_right_drag_start` (仮名、既定 `false` = 現行互換) を追加する。
  - UI 文言は「ファイル上で右ドラッグを始めたとき、そのファイルを選択」。
    `right_drag_grid` がリング / ジェスチャのときだけ有効な設定として同じページに置く。
  - ON の場合、セル上で有効な右ドラッグを開始した時点で `selected = Some(idx)` と
    `update_last_selected_image()` を行ってからリング / ジェスチャを開始する。チェック済み複数選択
    (`checked`) は解除しない。開始セルは既に可視なので追加のスクロール追従は行わない。
  - OFF の場合は以前の選択を維持する。背景の空き位置から開始した場合は ON/OFF にかかわらず
    選択を変えない。短い通常右クリックは、従来どおり release 時に対象セルを選んでメニューを出す。
  - サムネイル / 詳細ビューは共通 `handle_cell_interaction` で同じ挙動にする。
- 回帰確認: A 選択中に B からリング / ジェスチャ、背景からの開始、短い右クリック、
  Ctrl/Shift 複数選択、設定 roundtrip / sanitize を確認する。
- v2.5.0 裁定: 仕様追加として次バージョンへ送る。
- 規模 / リスク: Low〜Medium / P2 candidate。入力状態機械は変えず、選択更新の有無だけを
  押下境界で切り替える。

### 4.3 「メインウィンドウを閉じる」と「アプリを終了する」コマンド

- 出典: mImageViewer 専用スレ 73 (2026-07-18)。リングショートカット / マウスジェスチャの
  コマンド候補へ「ウィンドウを閉じる」がほしいという要望。
- 仕様:
  1. **メインウィンドウを閉じる** (`CloseMainWindow` 仮名): メインウィンドウの [×] と同じ
     close request を送る。`minimize_to_tray_on_close` が有効でトレイが利用可能なら既存
     `maybe_intercept_close` によりトレイへ格納し、それ以外は終了する。明示終了フラグは立てない。
  2. **アプリを終了する** (`QuitApplication` 仮名): 既存メニュー「終了」と同じ共通 helper を
     呼び、`shutdown_requested` を立ててから close request を送る。トレイ常駐設定にかかわらず
     アプリを終了し、通常の保存 / 終了処理は省略しない。
  3. 既存の `CloseFullscreen` (画像/動画ビューアを閉じる) は別操作として維持し、名称と説明で
     3 種類を混同しないようにする。
  4. 初期対象は `RingShortcutContext::Grid`。リング、グリッド用マウスジェスチャ、マウスボタン、
     ゲームパッド X リングの候補へ載せ、画像/動画ビューア文脈には誤って出さない。
  5. stable string ID、操作一覧、`.mivkeys.json` の export/import、設定世代からの取り込み、
     候補 parity test を同時に更新する。割り当て自体が opt-in のため確認ダイアログは追加しない。
- 回帰確認: トレイ常駐 ON/OFF で CloseMainWindow、トレイ常駐 ON で QuitApplication、
  設定保存完了、既存 CloseFullscreen、各候補リストの context 制限を確認する。
- v2.5.0 裁定: 新コマンドとして次バージョンへ送る。
- 規模 / リスク: Low〜Medium / P2 candidate。メニュー終了処理との共通化を行い、
  リング側だけの終了経路を新設しない。

### 4.4 画像 / 動画ビューアの右クリック短押し動作を選択可能にする

- 出典: mImageViewer 専用スレ 73 (2026-07-18)。ビューアが右クリックで閉じる動作を
  設定可能にしてほしいという要望。検討の結果、単純な bool ではなく短押し動作を選ぶ。
- 設定 / UI:
  - `ViewerShortRightClickAction` (仮名) を追加し、`CloseFullscreen` / `None` /
    `ContextMenu` / 将来値受信用 `Unknown` を持たせる。
  - `RingShortcutSettings` に画像ビューア用・動画ビューア用を別々に保持する。既存の
    `right_drag_image` / `right_drag_video` と同じ「マウス右ドラッグ」設定内で、各文脈の
    「右クリック短押し」コンボとして表示する。既定は `CloseFullscreen` (= 現行互換)。
  - 表示文言は「フルスクリーンを閉じる」「何もしない (右ドラッグ専用)」
    「右クリックメニューを表示」。グリッドは従来どおりメニュー、編集モードは従来どおり
    編集操作を優先し、この設定の対象外とする。
- 入力仕様:
  - 右ボタン押下直後には実行せず、既存状態機械が移動量 / 長押しを判定して `ShortTap` に
    確定した時だけ選択した動作を適用する。リング方向または登録ジェスチャが発火した場合は
    短押し動作を実行しない。
  - `None` はリング / ジェスチャ専用。右ドラッグ mode が `Disabled` の場合は短押しも無反応に
    なることを UI で説明する。
  - `ContextMenu` は現在の長押しと同じメニュー構築・owner/focus 経路を再利用し、短押しと
    長押しで別メニューを作らない。長押しメニュー自体は各選択肢で維持する。
  - 静止画/音楽の egui viewer と native 動画 presenter の両方へ同じ resolver を適用する。
    ガイド内の「短押しは閉じる」説明も設定値に追従させる。
- 回帰確認: 画像/動画 × Disabled/Ring/Gesture × 3 動作、方向発火、未登録ジェスチャ、
  長押しメニュー、編集モード、detached viewer、native 動画の owner/focus を確認する。
- v2.5.0 裁定: native 動画を含む実機 matrix が必要なため次バージョンへ送る。
- 規模 / リスク: Medium / P2 candidate。正しい変更境界は各入力経路の `ShortTap` 適用部。

### 4.5 選択を変えず一覧の先頭 / 末尾へスクロールするコマンド

- 出典: mImageViewer 専用スレ 73 (2026-07-18)。「一番上へスクロール」「一番下へ
  スクロール」コマンドの要望。既存 Home/End (`GridMoveFirst` / `GridMoveLast`) は
  先頭/末尾項目を選択するため、別操作とする。
- 仕様:
  - `GridScrollTop` / `GridScrollBottom` (仮名) をグリッド文脈の操作候補へ追加する。
    表示名は「一覧の先頭へスクロール」「一覧の末尾へスクロール」。
  - `selected`、`checked`、Shift 選択 anchor、`last_selected_image` を一切変更しない。
    `scroll_to_selected` も立てない。スクロール後に選択項目が画面外でもそのまま保持する。
  - サムネイル / 詳細ビュー、フィルタ、詳細ソート、読書履歴など、現在の実表示順と
    viewport の最大 offset に対して先頭 `0` / 末尾 `max_offset` へ移動する。
  - action 実行時には top/bottom の pending request だけを立て、セル高・列数・viewport 高と
    `max_offset` が確定する `render_grid` 内で適用する。古いレイアウト値や `f32::MAX` を
    直接 `scroll_offset_y` へ書く実装は避ける。
  - リング、グリッド用マウスジェスチャ、マウスボタン、ゲームパッド X リングの候補へ追加し、
    既存 Home/End のキー操作と意味を変えない。
- 回帰確認: サムネイル/詳細、通常/フィルタ/詳細ソート、空一覧、既に先頭/末尾、
  列数・UI倍率変更直後で正しい offset になり、選択・チェックが不変であることを確認する。
- v2.5.0 裁定: 新コマンドとして次バージョンへ送る。
- 規模 / リスク: Low〜Medium / P2 candidate。新しい pending scroll intent と両描画経路への
  適用が主な変更。

## 5. リリース前確認 / 依存更新

### 5.1 ネイティブ依存

| 対象 | 現状 / 次の確認 | 注意点 |
| --- | --- | --- |
| PDFium | **新版 `chromium/7906` あり (2026-06-23 確認)**。v2.1.0 は v2.0.0 と同じ `151.0.7891.0` 維持で出荷 (PDF 再テスト回避のため見送り)。次回リリースで `setup-pdfium.sh` 更新 → PDF 表示手動確認 | PDF 開封、ページ列挙、サムネ、フルスクリーン、パスワード PDF |
| FFmpeg LGPL shared | 動画再生の手動確認と LGPL ソース tarball 配置更新 | DLL 名が変わる更新では `setup-ffmpeg.sh` / loader / `build.rs` を揃える |
| ONNX Runtime | `ort-sys` 要求 DLL と setup script の VERSION を確認 | C API バージョン一致、`+crt-static` + `load-dynamic` 維持 |
| VST3 SDK / bridge | C++ ソース変更がなければ再ビルド不要 | 更新時は商用プラグインで実機確認 |

### 5.2 Rust クレート

- 通常の `cargo update` は互換範囲でまとめて実施する。
- メジャー / rc 脱出は個別判断:
  - `ort`
  - `pdfium-render`
  - `ffmpeg-the-third`
  - `image`
  - `zip`
  - `sevenz-rust2`
  - `delharc`
  - `unrar`
  - `turbojpeg`
- 更新後に確認するもの:
  - `cargo test`
  - 検索 bench 回帰
  - perf smoke
  - `dumpbin /dependents` で不要な VC runtime DLL が復活していないこと

---

## 6. 着手時に読み直す関連ドキュメント

| 領域 | ドキュメント |
| --- | --- |
| UI 同期 I/O / worker 化 | `docs/ui-responsiveness.md`, `docs/async-architecture.md` |
| サブフォルダ展開 / フラット仮想ビュー | `docs/subfolder-expansion-view-plan.md`, `docs/ui-responsiveness.md`, `docs/async-architecture.md`, `docs/details-view-and-filter-plan.md`, `docs/virtual-folders.md` |
| ZIP / PDF / 変換アーカイブ | `docs/virtual-folders.md`, `docs/shell-file-operations-context-menu-plan.md` |
| フォルダ移動 / Ctrl+↑↓ | `docs/fullscreen-navigation-consistency.md`, `docs/keymap-spec.md` |
| 入力カスタマイズ / マウス / ゲームパッド | `docs/keymap-spec.md`, `docs/key-customization-impl-plan.md`, `docs/ring-shortcut-plan.md`, `docs/operation-customize-share-plan.md` |
| フルスクリーン / F12 別ウィンドウ / 連結読み | `docs/display-pipeline.md`, `docs/detached-viewer-implementation-plan.md`, `docs/fullscreen-navigation-consistency.md` |
| 表示 / AI / 補正 | `docs/display-pipeline.md`, `docs/preset-and-adjustment.md` |
| 詳細表示 / スマートフィルタ | `docs/details-view-and-filter-plan.md`, `CLAUDE.md` の UI / スクロール節 |
| タグ / フルスクリーン右パネル / 動画 overlay | `docs/tag-catalog-redesign-plan.md`, `docs/display-pipeline.md`, `docs/video-architecture.md`, `docs/detached-viewer-implementation-plan.md` |
| リリース / 依存更新 | `CLAUDE.md` のリリース手順、各 native 依存管理節 |

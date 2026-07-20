# v2.6.0 実装計画

## 1. 目的

v2.6.0 は、複数の場所に分散した本・画像・動画・音声を 1 つの保存済みビューから見渡せる
**スマートフォルダ**を中心機能とする。あわせて、v2.5.0 で次版送りにした入力
カスタマイズ、コンテナ情報、代表サムネイルの改善を、独立した小さい変更から
順に実装する。

本書は v2.6.0 の仕様と実装順の正本とする。実装中に判断を変える場合は、コードを
先行させず本書を同時に更新する。

## 2. v2.6.0 の実装順

1. ✅ グリッド右ドラッグ開始セルを選択するオプション
2. ✅ 「メインウィンドウを閉じる」「アプリを終了する」コマンド
   - 「メインウィンドウを閉じる」はグリッド限定。「アプリを終了する」は画像 / 動画フルスクリーンの
     リング・マウスジェスチャ・マウスボタンでも割り当て可能。再起動時は一覧へ戻る。
3. ✅ 一覧の先頭 / 末尾へ移動するコマンド
   - 選択を変えないスクロールと、Home / End 相当の選択移動を別コマンドとして提供する。
   - 選択を変えないスクロールはサムネイル / 詳細表示の両方で同じ pending intent を
     レイアウト確定時に消費し、同フレーム後半で追加された要求を次フレームまで保持する。
4. ✅ スマートフォルダ MVP
5. ✅ ZIP / PDF / 画像のみフォルダのページ数表示
   - 保存した PDF パスワードを同じ起動中の親一覧に反映し、ページ数を自動再取得する。
     保存しないパスワードでは従来どおり `-` を維持する。
6. ✅ 手動固定した親コンテナ代表サムネイルへの編集プレビュー反映
   - 親が代表にした ZIP / PDF に別の代表ページが固定されている場合も、そのページと
     編集 preview を連鎖して表示する。子側に固定がなければ従来の先頭ページを使う。
7. ✅ 画像 / 動画ビューアの右クリック短押し動作設定
   - 動画フルスクリーンのメニューを動画用に整理し、現在フレームの動画サムネ設定を追加。
     メニューを左クリックで閉じた入力は背面の再生 / 一時停止へ伝播させない。
8. ✅ 動画を代表画像に固定したフォルダのアイドル高画質化無限再投入・毎フレーム再判定を修正
9. ✅ 静止中・背面表示中の CPU / repaint / work 再投入 / ログ肥大を検出する idle-health リリースゲート
10. ✅ 右クリックメニュー末尾の「このフォルダをエクスプローラで開く」と、ダイアログ wheel の背面伝播防止
11. ✅ v2.5.0 公開後の表示修正と静止画 HUD 改善
    - 選択情報ツールチップの実測配置、環境設定ページ切替時のスクロール初期化、
      変換アーカイブ内ページの元形式表示、詳細表示の文字コントラストを修正。
    - 上部情報バー固定、見開き左右2ページ情報、回転角度メニュー、ページシーク方向設定を追加。
    - 全体の文字コントラストを「標準 / 強め」で統一し、暗色固定ダイアログ / popup のテーマ処理を共通化。
      変換アーカイブ内フォルダの元形式、鍵アイコン、見開き左右の AI 処理名、シーク方向の説明も修正。
12. 依存更新、全体回帰、リリース準備

各項目は可能な限り独立コミットにし、狭いテストから全体テストへ広げる。PDFium / FFmpeg
などの依存更新も機能変更と混ぜず、v2.6.0 開発の早い段階で別コミットとして長く soak する。

## 3. スマートフォルダ MVP

### 3.1 概念とお気に入りとの関係

- スマートフォルダとお気に入りは別概念とする。
- お気に入りは単一の実フォルダへ素早く移動し、必要に応じて検索索引のルートにもする。
- スマートフォルダは現在の実フォルダと一覧の絞り込み条件を 1 ルールとして保存し、複数ルールを
  OR 結合して条件に合う実アイテムを横断表示する。
- 各ルールの source はお気に入り UUID ではなく実パスを正本として保存する。
  お気に入りの改名 / 削除でスマートフォルダを壊さない。
- 検索元を単独で編集する巨大な定義フォームは作らない。通常一覧で結果を絞り込んでから
  「現在のアイテム表示条件を追加」で保存するため、既存 facet UI を条件指定の正本にする。

### 3.2 構築方式

- **索引を使わないスナップショット方式**を MVP の正本とする。
- スマートフォルダを開くたびに、有効ルールの source を background worker で走査する。
- 各ルールは「このフォルダ直下のみ / サブフォルダを含む」を個別に持つ。通常一覧から追加すると
  直下のみ、サブ展開結果から追加するとサブフォルダを含む設定を初期値とし、確認画面で変更できる。
- 表示中に外部で起きた変更は自動監視しない。明示的な「更新」で再走査できるようにする。
- 走査完了後の snapshot は保持し、ソート / facet / 表示形式の変更では再走査しない。
- definition の表示名は走査 identity に含めず、並び単位の変更も保存済み snapshot の
  prepare だけをやり直す。入力中の名称・名前条件は管理ダイアログ内の draft に保持し、
  毎フレームの trim・Settings 同期保存・再走査を行わない。フォーカス移動、定義の選択変更、
  開く／閉じる等のダイアログ操作時に一度だけ正規化・保存・必要な無効化を行う。
- UI スレッドでは `read_dir`、metadata 取得、再帰走査、大量 DB lookup を実行しない。
  prepare worker が対象項目を確定した後、★ / タグに加えて個別補正・crop・表示トリミング・
  消しゴム・隠蔽・注釈を exact key の batch query で疎に取得する。変換アーカイブ対応表、
  catalog、固定代表サムネイルも同じ worker で準備し、完成 snapshot の install は同期 DB I/O を
  行わない。
- 既存 `subfolder_expansion` の複数 root、進捗、cancel、reparse point guard、chunk sort、
  `Arc<Vec<_>>` snapshot、prepare worker の規約を共有する。似た walker を別実装しない。
- 各物理フォルダを列挙した直後、保存条件を適用する前に、通常一覧と同じ同名ファイル規則を
  フォルダ単位で適用する。同名動画の sidecar 画像、同名実フォルダがある ZIP / PDF / 対応
  アーカイブ、同名 ZIP がある変換元アーカイブ、優先度の低い同名画像は独立項目にしない。
  別の物理フォルダにある同名ファイル同士は衝突させない。動画 sidecar を非表示にした場合も、
  その実パスを動画サムネイル用 snapshot として表示準備へ引き継ぐ。
- I/O は `GlobalIoSemaphore` の Normal priority を通し、可視サムネイルなどの High I/O を優先する。
- 10 万件以上は既存サブ展開と同じ続行確認を使う。件数で黙って打ち切らない。

### 3.3 索引を MVP に含めない理由

現行検索索引は「お気に入り 1 件 = 1 supervisor / watcher」を前提とし、管理 DB も path と
`favorite_id` の関係を持つ。任意 source を持つスマートフォルダをそのまま索引 root にすると、
重複 root、親子 root、複数スマートフォルダへの同一 path 所属、watcher 重複、定義削除時の
索引所有権が曖昧になる。

将来索引が必要になった場合は、お気に入りとスマートフォルダが参照する汎用 `IndexRoot`
registry として設計し、canonical root の重複排除と参照数を一元管理する。スマートフォルダごとに
独立 supervisor を増やす方式や、scan と索引で結果が変わる best-effort hybrid は採用しない。

### 3.4 結果の単位

スマートフォルダは通常の実フォルダ一覧で扱う次の実アイテムを対象にする。「本のみ」には制限しない。

- フォルダ
- 通常画像
- 動画
- 音声
- ZIP / CBZ
- PDF
- 直接閲覧または ZIP 変換対象になる RAR / CBR / 7z / LZH 等の対応アーカイブ

コンテナの中身や PDF ページは走査時に展開せず、コンテナ 1 件として表示する。ZIP / PDF 内、
検索結果、読書履歴など実検索元を一意に復元できない仮想ビューからルールは作成できない。

### 3.5 フラット表示と階層情報

- 表示は全 source を横断したフラットグリッドとする。
- snapshot entry には source の stable ID、source root、root からの相対親パス、実パスを保持する。
- 元の階層情報は失わず、「場所」列、ツールチップ、場所 facet、元フォルダを開く操作に使う。
- 表示順は「全体で並べる」と「フォルダごとに並べる」を持つ。
  - 全体: 現在の sort を全 source 横断で適用する。
  - フォルダごと: source の登録順 → 相対フォルダ → 現在の sort の順で安定化する。
- source root が重なる場合、同じ実パスは 1 件に dedupe する。表示上の所属は最も具体的な
  source を優先し、同じ深さなら source 登録順を使う。
- 完全な仮想階層ナビは MVP に含めない。ただし相対パスを保持し、将来表示方式を追加できる
  データモデルにする。

### 3.6 保存モデル

概念モデルは次の形とする。最終的な Rust 型名は既存命名に合わせて確定する。

```text
SmartFolderDefinition
  id: UUID
  name: String
  rules: Vec<SmartFolderRule>
  grouping: Global | ByFolder

SmartFolderRule
  id: UUID
  source: PathBuf
  enabled: bool
  include_descendants: bool
  filter: SmartFolderFilter
```

- `Settings.smart_folders` として settings.db の世代バックアップ対象にする。
- 旧設定では空 Vec を default とし、破壊的な設定 DB migration は行わない。
- UUID 欠落、空 source、条件値の範囲外、将来 enum 値は `Settings::sanitize` で安全に補正する。
- v2.6.0 開発中の旧スマートフォルダ定義は移行対象にしない。
- source の移動を自動追跡しない。見つからない source は定義から削除せず、警告として表示する。
- 通常一覧と共通のソート順およびサムネイル / 詳細表示は定義へ保存しない。スマートフォルダを
  開いても現在の全体表示設定を上書きせず、スマートフォルダ固有には「全体で並べる / フォルダごとに
  並べる」の単位だけを保存する。

### 3.7 保存する条件

通常一覧の facet から、snapshot 構築後に既存データで正確に判定できる条件をルール単位で保存する。

- 名前
- コンテナ種別 / 拡張子
- 更新日時（既定期間、任意日数、開始日 / 終了日）/ サイズ
- ★
- タグ（OR / AND、タグなし）
- 編集状態
- ルールの有効 / 無効、サブフォルダを含むか

名前・種別・拡張子など安価な条件は worker の早い段階で適用してよい。★ / タグは
既存 DB の batch 取得を使い、1 item ごとの同期 lookup を行わない。EXIF、AI プロンプト、
PDF document info などファイル内容を大量に開く全文条件は MVP の保存条件に含めない。
場所、AI モデル、生成ツール、画像色は保存しない。現在表示で指定されていた場合は、ルール追加の
確認画面に「保存されない条件」として明示する。

### 3.8 メニューとツールバー

- メニューバーに独立した「スマートフォルダ」を常設する。0 件でも作成入口を失わない。
- メニュー項目:
  - 新しいスマートフォルダ…
  - 現在のアイテム表示条件を追加 → 登録済みスマートフォルダ一覧
  - スマートフォルダを管理…（0 件では disabled）
  - 区切り線
  - 登録済みスマートフォルダ一覧（選択で開く）
- `MenuCommandId` / `TopMenuId` / menu layout sanitize と候補 parity test を更新する。
- 「現在のアイテム表示条件を追加」は通常の実フォルダまたはサブ展開でだけ有効にする。ZIP / PDF 内、
  検索結果、読書履歴等では disabled とし、理由を tooltip で表示する。サブ展開中でも Ctrl+F の
  名前検索が有効なら、検索語を黙って捨てた広い条件を保存せず同様に追加を拒否する。
- 「新しいスマートフォルダ…」は名前だけを入力して末尾へ追加する。条件は作成後に現在表示から追加する。
- 名前は大文字小文字を無視して一意とし、新規作成と名前変更の両方で同じ検証を行う。
- ツールバーには独立 `ToolbarSectionId::SmartFolders` を追加する。
- effective visibility は `show_toolbar_smart_folders && !smart_folders.is_empty()` とする。
  0 件では描画せず、最後の 1 件を削除した後も表示設定・順序・表示形式は保持する。
- 初期値は表示 ON とし、最初の 1 件を作ると自動的に現れる。ユーザーが明示的に隠した設定を
  作成のたびに強制 ON へ戻さない。
- 既存統一モデルどおり、展開 / 折りたたみ / プルダウン、セクション並べ替え、行頭指定、
  右クリック設定に対応する。
- 項目の左クリックはスマートフォルダを開く。編集 / 削除はセクション設定または管理画面から行う。

### 3.9 開く・更新・ナビゲーション

- synthetic path は definition UUID から生成し、実 filesystem path と混同しない。
- 開くと現在の一覧を残したまま背景をグレーアウトした scan / prepare modal を表示し、
  「中止」以外の背面操作を止める。完成 snapshot だけを一括 install する。
- stale definition ID / scan generation / source set の結果は破棄する。
- `中止`、別の場所を開く、definition 編集 / 削除、アプリ終了で pending scan を cancel する。
- フォルダバーには `スマートフォルダ: <name>` と表示し、source path を synthetic breadcrumb にしない。
- 戻る / 進むでは同一セッション中の snapshot を再利用する。snapshot が無ければ definition から再走査する。
- スマートフォルダを開く履歴は scan / prepare が成功して一覧を採用するときだけ追加する。
  中止や全 source 失敗では現在地と履歴を変えない。
- スマートフォルダ自身が保存条件を所有するため、開く前の通常一覧で有効だった facet / ★
  フィルタは synthetic scope 中だけ退避し、戻ると復元する（二重適用しない）。
  スマートフォルダ A から B へ直接切り替える場合も退避状態を引き継ぎ、A の退避値を B 上で
  復元しない。
- synthetic path を実フォルダ探索へ渡さない。フルスクリーン、ゲームパッド、リング操作を含む
  親 / 子 / 兄弟フォルダ移動はスマートフォルダ表示中 no-op とする。
- リネーム成功時は旧 path とその子孫を snapshot / tombstone 管理から除外して current view を
  再 prepare する。★ / タグ / 編集状態の変更や表示中 definition の編集も、保存条件を再評価して
  現在一覧を更新する。
- 表示中に削除した実パスは definition ごとの tombstone として snapshot prepare に渡す。
  共有中でなければ snapshot をその場で compact し、ソート変更や履歴復元で削除済み項目を
  復活させない。tombstone は snapshot 世代に属し、worker との共有が解けて snapshot へ
  反映できた時点、または明示的な再走査の成功後に破棄する。件数上限による破棄は行わない。
- 「更新」は同じ definition で新しい generation を開始し、成功するまで現在 snapshot を保持する。
- 一部 source が見つからない / 読めない場合は、読めた source の結果を表示し、失敗 source 数と
  詳細を通知する。全 source 失敗時は現在表示を置き換えない。

### 3.10 テストと計測

- definition / rule の serde roundtrip、legacy default、sanitize、UUID 補完
- 複数 root、親子 root、重複 path、登録順、missing / access denied source
- reparse point loop、depth limit、cancel、stale generation discard
- フォルダ、画像、動画、音声、ZIP / PDF / 対応アーカイブの収集と種類条件
- 直下のみ / 再帰、複数ルールの OR、現在 facet の取得、保存対象外条件の明示
- 全体 sort / フォルダごと sort、filter、場所ラベル
- toolbar 0 件非表示、最初の作成、最後の削除、明示非表示保持、並び単位 roundtrip、
  スマートフォルダ表示時に全体のソート順 / サムネイル・詳細表示を上書きしないこと
- menu layout の旧設定補完、名前だけの作成 / 現在条件追加 / 管理 / 登録一覧 / 対象外 tooltip
- synthetic view からの open / back / forward / refresh / file operation 後の clamp
- 名称の内部空白、空欄からの再入力、名称変更で走査を破棄しないこと、削除後の sort / 履歴復元、
  通常一覧の facet / ★がスマートフォルダへ二重適用されないこと
- perf event は scan / prepare / install / cancel を分け、10 万 / 50 万 / 200 万 entry で計測する。

2026-07-19 の post-scan prepare ベンチマーク（debug test process、完成前 snapshot と prepare
result を同時に保持した process Peak Working Set を計測）は次のとおり。filesystem scan と
production DB I/O は含めず、filter / sort / item・metadata 構築の O(N) 部分を測っている。

| entry | prepare | Max WS | process Peak WS |
| ---: | ---: | ---: | ---: |
| 100,000 | 202.7 ms | 86.6 MiB | 97.8 MiB |
| 500,000 | 1,046.5 ms | 401.8 MiB | 415.7 MiB |
| 2,000,000 | 4,302.8 ms | 1,594.1 MiB | 1,609.1 MiB |

再計測は `MIV_SMART_FOLDER_BENCH_ITEMS=<件数> cargo test --bin mimageviewer-core
smart_folder_prepare_scale_benchmark -- --ignored --nocapture` を件数ごとに別 process で実行する。

## 4. v2.6.0 に含めないもの

- スマートフォルダ root の常時 watcher
- スマートフォルダ専用 Tantivy / SQLite 検索索引
- EXIF / AI プロンプト等の保存済み全文条件
- スマートフォルダ管理画面内に既存 facet と重複する巨大な条件編集 UI
- 完全な仮想階層表示
- ZIP / PDF の中の各ページを scan 結果へ展開する機能
- 親代表サムネイルの自動選定への編集プレビュー反映（手動固定を先行）
- detached viewer の資源予算、メディア昇格、表示 LOD、英語 UI

## 5. 依存更新ベースライン（2026-07-18）

- PDFium を `chromium/7934` から `chromium/7947` へ更新した。PDF 開封、ページ列挙、
  サムネイル、フルスクリーン、パスワード付き PDF は実機回帰対象とする。
- FFmpeg LGPL shared build を `n7.1.5-1-g7d0e842004` から
  `n7.1.5-2-g998de74adf` へ更新した。DLL メジャー名は不変。実 DLL の ProductVersion、
  LGPLv3-or-later、`--enable-version3`、x264/x265 無効を監査し、製品ページの対応ソース表記を同期した。
- ONNX Runtime は `ort` / `ort-sys 2.0.0-rc.12` の配布表が要求する `1.24.2` と、
  setup script / vendor DLL の `1.24.2` が一致するため据え置いた。
- `cargo update` で Rust 1.88 互換範囲の lockfile を更新した。メジャー更新と rc 脱出は
  通常更新へ混ぜず、バックログの個別判断対象として残す。

## 6. 関連ドキュメント

- [architecture-overview.md](architecture-overview.md)
- [async-architecture.md](async-architecture.md)
- [ui-responsiveness.md](ui-responsiveness.md)
- [subfolder-expansion-view-plan.md](subfolder-expansion-view-plan.md)
- [search-architecture.md](search-architecture.md)
- [details-view-and-filter-plan.md](details-view-and-filter-plan.md)
- [virtual-folders.md](virtual-folders.md)
- [toolbar-customization-plan.md](toolbar-customization-plan.md)
- [next-release-backlog.md](next-release-backlog.md)

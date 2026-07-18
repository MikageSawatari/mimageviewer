# v2.6.0 実装計画

## 1. 目的

v2.6.0 は、複数の場所に分散した本を 1 つの保存済みビューから見渡せる
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
7. ✅ 画像 / 動画ビューアの右クリック短押し動作設定
8. ✅ 動画を代表画像に固定したフォルダのアイドル高画質化無限再投入・毎フレーム再判定を修正
9. ✅ 静止中・背面表示中の CPU / repaint / work 再投入 / ログ肥大を検出する idle-health リリースゲート
10. 依存更新、全体回帰、リリース準備

各項目は可能な限り独立コミットにし、狭いテストから全体テストへ広げる。PDFium / FFmpeg
などの依存更新も機能変更と混ぜず、v2.6.0 開発の早い段階で別コミットとして長く soak する。

## 3. スマートフォルダ MVP

### 3.1 概念とお気に入りとの関係

- スマートフォルダとお気に入りは別概念とする。
- お気に入りは単一の実フォルダへ素早く移動し、必要に応じて検索索引のルートにもする。
- スマートフォルダは任意の複数実フォルダを直接登録し、条件に合う本を横断表示する。
- スマートフォルダの source はお気に入り UUID ではなく実パスを正本として保存する。
  お気に入りの改名 / 削除でスマートフォルダを壊さない。
- 編集 UI には「フォルダを追加」に加えて「お気に入りから追加」を便利な入力手段として置く。

### 3.2 構築方式

- **索引を使わないスナップショット方式**を MVP の正本とする。
- スマートフォルダを開くたびに、登録 source を background worker で再帰走査する。
- 表示中に外部で起きた変更は自動監視しない。明示的な「更新」で再走査できるようにする。
- 走査完了後の snapshot は保持し、ソート / facet / 表示形式の変更では再走査しない。
- UI スレッドでは `read_dir`、metadata 取得、再帰走査、大量 DB lookup を実行しない。
- 既存 `subfolder_expansion` の複数 root、進捗、cancel、reparse point guard、chunk sort、
  `Arc<Vec<_>>` snapshot、prepare worker の規約を共有する。似た walker を別実装しない。
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

MVP の主対象は「本として扱うコンテナ」とする。

- 実際に画像だけの本として開けるフォルダ
- ZIP / CBZ
- PDF
- 直接閲覧または ZIP 変換対象になる RAR / CBR / 7z / LZH 等の対応アーカイブ

画像だけのフォルダ判定は、通常の自動本判定と同じ列挙規則を共有する。名前や拡張子だけの
別判定を作らない。コンテナの中身や PDF ページは走査時に展開せず、コンテナ 1 件として表示する。
通常画像 / 動画 / 音声を横断して平坦化するモードは、MVP の実測と要望を見て後続判断する。

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
  sources: Vec<SmartFolderSource>
  filter: SmartFolderFilter
  sort: SortOrder
  grouping: Global | ByFolder
  view_mode: Thumbnail | Details

SmartFolderSource
  id: UUID
  path: PathBuf
  enabled: bool
  filter_override: None   # 将来拡張用。MVP UI では共通 filter のみ
```

- `Settings.smart_folders` として settings.db の世代バックアップ対象にする。
- 旧設定では空 Vec を default とし、破壊的な設定 DB migration は行わない。
- UUID 欠落、重複 source、空 path、将来 enum 値は `Settings::sanitize` で安全に補正する。
- source の移動を自動追跡しない。見つからない source は定義から削除せず、警告として表示する。

### 3.7 保存する条件

MVP は snapshot 構築後に既存データで正確に判定できる共通条件を保存する。

- 名前
- コンテナ種別 / 拡張子
- 更新日時 / サイズ
- ★
- タグ
- source の有効 / 無効（検索元一覧で指定）

名前・種別・拡張子など安価な条件は worker の早い段階で適用してよい。★ / タグは
既存 DB の batch 取得を使い、1 item ごとの同期 lookup を行わない。EXIF、AI プロンプト、
PDF document info などファイル内容を大量に開く全文条件は MVP の保存条件に含めない。
場所と編集状態は、表示された snapshot に対する既存 Ctrl+F の一時条件として利用できるが、
MVP の定義には保存しない。

### 3.8 メニューとツールバー

- メニューバーに独立した「スマートフォルダ」を常設する。0 件でも作成入口を失わない。
- メニュー項目:
  - 新しいスマートフォルダ…
  - スマートフォルダを管理…（0 件では disabled）
  - 区切り線
  - 登録済みスマートフォルダ一覧（選択で開く）
- `MenuCommandId` / `TopMenuId` / menu layout sanitize と候補 parity test を更新する。
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
- 開くと現在の一覧を残したまま scan modal を表示し、完成 snapshot を一括 install する。
- stale definition ID / scan generation / source set の結果は破棄する。
- `中止`、別の場所を開く、definition 編集 / 削除、アプリ終了で pending scan を cancel する。
- フォルダバーには `スマートフォルダ: <name>` と表示し、source path を synthetic breadcrumb にしない。
- 戻る / 進むでは同一セッション中の snapshot を再利用する。snapshot が無ければ definition から再走査する。
- 「更新」は同じ definition で新しい generation を開始し、成功するまで現在 snapshot を保持する。
- 一部 source が見つからない / 読めない場合は、読めた source の結果を表示し、失敗 source 数と
  詳細を通知する。全 source 失敗時は現在表示を置き換えない。

### 3.10 テストと計測

- definition / source の serde roundtrip、legacy default、sanitize、UUID 補完
- 複数 root、親子 root、重複 path、登録順、missing / access denied source
- reparse point loop、depth limit、cancel、stale generation discard
- 画像のみフォルダ、混在フォルダ、ZIP / PDF / 対応アーカイブの判定
- 全体 sort / フォルダごと sort、filter、場所ラベル
- toolbar 0 件非表示、最初の作成、最後の削除、明示非表示保持、表示形式 roundtrip
- menu layout の旧設定補完、作成 / 管理 / 登録一覧
- synthetic view からの open / back / forward / refresh / file operation 後の clamp
- perf event は scan / prepare / install / cancel を分け、10 万 / 50 万 / 200 万 entry で計測する。

## 4. v2.6.0 に含めないもの

- スマートフォルダ root の常時 watcher
- スマートフォルダ専用 Tantivy / SQLite 検索索引
- EXIF / AI プロンプト等の保存済み全文条件
- source ごとの個別 filter UI
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

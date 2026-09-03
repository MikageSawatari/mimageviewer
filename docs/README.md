# docs/ — ドキュメント索引

修正作業の前に、関連するドキュメントを読んで全体設計を把握すること。

## 設計ドキュメント (これから作業する前に)

**迷ったらまず [architecture-overview.md](architecture-overview.md) から。**

| ドキュメント | 読むべきタイミング |
| --- | --- |
| [architecture-overview.md](architecture-overview.md) | 全体像の把握。レイヤー構造・モジュールマップ・永続化ストア一覧 |
| [display-pipeline.md](display-pipeline.md) | サムネイル表示・フルスクリーン描画を触るとき。**補正/AI/回転の適用順の決定版** |
| [async-architecture.md](async-architecture.md) | 並列処理・キャンセル・キャッシュ競合を触るとき。ワーカー一覧とテンプレ。動画 packet/control channel、bridge 内 per-slot VST GUI thread、Normalize scan lifecycle を含む |
| [ui-responsiveness.md](ui-responsiveness.md) | UI スレッド同期 I/O で UI を止めないための設計方針。**新機能追加前にチェックリスト §4 を必ず見る** |
| [preferences-layout-guidelines.md](preferences-layout-guidelines.md) | 環境設定 UI のページ構成、配置、レスポンシブレイアウトを触るとき |
| [idle-health-check.md](idle-health-check.md) | 静止中・背面表示中・トレイ常駐中の高速 repaint / work 再投入 / CPU・ログ肥大をリリース前に自動検出する手順と判定値 |
| [tray-residency-cpu-spin-investigation.md](tray-residency-cpu-spin-investigation.md) | v2.10.0 のトレイ格納後 main-thread CPU spin の計装結果、producer / consumer、eframe scheduler 根本修正 |
| [virtual-folders.md](virtual-folders.md) | ZIP/PDF 関連を触るとき。**通常画像との分岐チェックリスト** |
| [pdf-page-count-cache-plan.md](pdf-page-count-cache-plan.md) | PDF ページ数キャッシュ、worker 境界、失敗時の扱いを触るとき |
| [preset-and-adjustment.md](preset-and-adjustment.md) | 補正・プリセット・AI キャッシュを触るとき。**無効化ルールの早見表** |
| [adjustment-scope-selector-plan.md](adjustment-scope-selector-plan.md) | **設計合意済み・未実装**。補正スコープを「このページの個別設定 > その場所の標準」の 2 段へ再設計する計画。お気に入りごとの `□ お気に入り用標準設定を使う` (既定 OFF、ON/OFF の実体は `favorite_params` 行の有無)、標準（共通）/ 標準（お気に入り「xxx」）の名称、`Ctrl+Alt+数字` の書き込み先統一と `Ctrl+Alt+-` 追加、表示トリムとの語彙・見た目統一。永続データ変更なし・移行不要 |
| [ai-processing-size-threshold-plan.md](ai-processing-size-threshold-plan.md) | AI 処理の入力サイズ閾値、縮小規則、テスト要件を触るとき |
| [web-remote-plan.md](web-remote-plan.md) | **v3.0.0 で出荷**。リモート閲覧の正本。本体 / remote service / Web UI の 3 プロセス構成、IPC protocol と版管理、Tailscale + PIN の認証境界、表示所有権の cutover、閲覧できる範囲と操作できない範囲。リモートを触るときは最初にここを読む |
| [web-remote-video-streaming-plan.md](web-remote-video-streaming-plan.md) | リモートの動画 / 音声。PC 側トランスコード + HLS 配信、画質 mode、seek と再接続 |
| [web-remote-left-panel-plan.md](web-remote-left-panel-plan.md) | リモートの左パネル (場所 / 検索 / タグ / お気に入り / 本棚) の範囲と、本体一覧との差 |
| [web-remote-ai-plan.md](web-remote-ai-plan.md) | **段 3b-0〜3b-2 実装・実機確認済み。JPEG 転送 / 端末別画質 mode は実装済み・実機確認待ち**。remote AI アップスケール / デノイズの PC modal 排他、共有 canonical decoder、接続取得 / 切断 drain barrier、App 所有 singleton Runtime、stable remote key、HTTP job、Web UI、画面消灯復帰、VRAM、撤退条件 |
| [local-adjust-testing.md](local-adjust-testing.md) | 部分補正レイヤーの回帰テスト、合成順、キャッシュ無効化を検証するとき |
| [search-architecture.md](search-architecture.md) | 検索 / インデクサ / タグを触るとき。**Ctrl+S/F/G の経路 + インデクサパイプライン + DB 責任分離** |
| [item-kind-capability-matrix.md](item-kind-capability-matrix.md) | **★ / タグ / 削除 / コピー / 外部ツール / 検索が `GridItem` の種別ごとにどう振る舞うかの現状表**。セルは 対応 / 拒否 / コンテナへ寄せる / **無反応** の 4 値。仮想ページ (ZipImage / PdfPage) を扱う機能を足すときは先にここを見る |
| [top-level-grid-view.md](top-level-grid-view.md) | 検索・★固定・サブ展開・スマートフォルダ等の最上位一覧 ownership / 復元 snapshot と、スマートフォルダ root / scoped drill の不変条件 |
| [fullscreen-navigation-consistency.md](fullscreen-navigation-consistency.md) | フルスクリーン / 検索結果 / 動画タイルをまたぐ Ctrl+↑↓・境界ヒント・前後移動の統一仕様メモ |
| [keymap-spec.md](keymap-spec.md) + [key-customization-impl-plan.md](key-customization-impl-plan.md) + [key-command-catalog-plan.md](key-command-catalog-plan.md) | キーボード操作 / ショートカット / `consume_key` / `key_pressed` / native VK 判定 / コマンドカタログ化を触るとき。新しいキー操作は keymap 対応要否を必ず確認 |
| [touch-support-plan.md](touch-support-plan.md) | **仕様確定 / Phase 2 + Step 3d まで実装済み**。タブレット PC のタッチ操作対応。静止画 / 本フルスクリーンは左右タップのページ送り、中央タップの上下クロームと左右パネルハンドル、2 本指ズーム / パン、中央タップを学習するまでの初回オーバーレイヘルプを配線済み。サムネイル一覧は行スナップを維持した 1 本指スクロール、進行方向への release 確定、2 本指ピンチによる列数変更を配線済み。選択済みセルの再タップ open は利用者判断で見送り。動画 / 音楽のタッチ操作は Phase 3。3 領域タップ + 中央クローム + anchor-fraction スクロール + ピンチの設計とフェーズ別工数。タッチ / ポインタ入力を触るときに読む |

## 仕様・機能

| ドキュメント | 内容 |
| --- | --- |
| [spec.md](spec.md) | アプリ全体の仕様書 (設定項目・機能一覧) |
| [comic-integration-plan.md](comic-integration-plan.md) | comic DB、注釈 overlay、編集・書き出しパイプラインの統合契約 |
| [conceal-feature-plan.md](conceal-feature-plan.md) | 隠蔽加工の形状、保存、合成、キャッシュ無効化の現行仕様 |
| [panorama-360-view-plan.md](panorama-360-view-plan.md) | **コード実装済み・実素材／実機性能の手動確認は記録上未確認**。360° パノラマ表示、GPano crop、mipmap、settle refinement、fullscreen 合成の現行仕様と設計経緯 |
| [fullscreen-side-panel-mode-plan.md](fullscreen-side-panel-mode-plan.md) | **実装済み・手動実機確認は記録上未確認**。静止画・動画・音楽で共通のサイドパネル表示モード仕様 |
| [edit-content-identity-plan.md](edit-content-identity-plan.md) | **Phase 1 実装済み・実機確認済み (A1〜A6)**。OS 側でファイルを移動・コピーしたときに、内容ハッシュで編集内容 (補正 / 消しゴム / モザイク / 注釈 / トリミング / ★ / タグ) を再結合して復元する機能。size → 先頭 64KB → 全体の 3 段照合、`rename_key_migration::STORES` 駆動の batch copy、変換アーカイブの 4 面キー、モーダル確認ウィンドウ |
| [next-release-backlog.md](next-release-backlog.md) | **次リリース検討バックログ**。いま着手できる未対応の P2/P3、ユーザー要望、依存ライブラリ更新、リリース手順の未解決点だけを恒久管理。完了した項目はこのファイルから削除する |
| [backlog-on-hold.md](backlog-on-hold.md) | **保留・着手待ちバックログ**。判断待ち / 再現・確認待ち / 見送り。動かせるようになったら節ごと上へ戻す |
| [release-verification-records.md](release-verification-records.md) | **リリース前確認の記録**。版ごとに実際に取った perf smoke / idle health / bench / 依存確認の測定値。次に何かが遅くなったときの比較対象。手順の正本は CLAUDE.md と release-operations.md |
| [detached-viewer-implementation-plan.md](detached-viewer-implementation-plan.md) | 画像・動画別ウィンドウの設計・実装履歴。冒頭 §§1〜2 は初期 v1 案、§3.0 は現行モード、§11 以降は CUT 前 pin 案を含む履歴 |
| [detached-viewer-lifecycle-redesign-proposal.md](detached-viewer-lifecycle-redesign-proposal.md) | **historical diagnosis + 現況表**。BA-1〜BA-7 の初期診断と、解消 / 部分解消 / 未解消の現在地。進捗の正本は `detached-rework-plan.md` §9 |
| [detached-viewer-keepalive-design.md](detached-viewer-keepalive-design.md) | keep-alive の不変条件、target 設計、K0〜K3 の現況。K0 完了、K1 未完、K2/K3 部分完了で、single render entry は未実装 |
| [detached-viewer-smoke-checklist.md](detached-viewer-smoke-checklist.md) | K0 詳細 smoke の補助チェックリスト。現行 registry ログ (`registered host` / `hwnd_adopted_*`) に対応。リリース全体は ship checklist を正とする |
| [detached-rework-plan.md](detached-rework-plan.md) | **detached viewport リワーク正本**。§9 が唯一の現況表。R2b は部分完了、R3 は実質完了、R4 は未完 |
| [detached-rework-ship-checklist.md](detached-rework-ship-checklist.md) | 現行リワーク出荷前 smoke matrix (F/W/V/P/R 系)。独立静止画窓の Ctrl 物理フォルダ移動と configurable 右クリックを含む |
| [details-view-and-filter-plan.md](details-view-and-filter-plan.md) | **Ph1〜Ph4 + Ph5 画像/動画/作成日時遅延列まで実装済み**。ファイル選択画面の詳細表示モード (サムネ無しで名前/サイズ/日付＋★/タグ/編集フラグを行表示) ＋ Excel オートフィルタ風スマートフィルタの設計。現状は列セクションの詳細切替、右クリック列表示メニュー、`details_order` による列ヘッダ 3 トグルソート、種類/拡張子/場所/★/タグ/日付/サイズ/状態の共通 `FacetFilter`、遅延列 worker / 進捗表示、作成日時列、画像解像度列、長さ/動画解像度/コーデック列まで実装済み (長さ・コーデックは音声も対応)。場所は元ファイル/元コンテナの親フォルダで、製本フォルダは `本棚 > 本名` 表記。場所条件は移動で解除される非永続の一時条件。EXIF/PDF/アーカイブ系の追加遅延列は後続 |
| [shell-file-operations-context-menu-plan.md](shell-file-operations-context-menu-plan.md) | **一部実装済み**。Windows Shell の `IFileOperation` とネイティブ右クリックメニューへ寄せるファイル整理機能の実装計画。A/B クイックフォルダ、実/仮想項目の native 右クリックメニュー、rename、delete-to-recycle は実装済み。copy/move/drop の `IFileOperation` 化は後続 |
| [context-menu-unification-plan.md](context-menu-unification-plan.md) | **Phase A/B 実装済み**。実項目・仮想項目で共通の native 右クリックメニュー、mIV 項目の単一定義、混在選択の拒否、Windows 項目の遅延サブメニューと併記設定の正本 |
| [key-customization-impl-plan.md](key-customization-impl-plan.md) | **実装済みメモ**。簡易版 (旧テキスト ini / GUI なし / 競合は警告のみ) の手順書と実装判断。現在の正本は `Settings.keymap` で、旧 `keymap.ini` は初回起動時に settings.db へ移行して `keymap.ini.imported*.bak` へ退避する。`src/keymap.rs` の型・`keymap.ini.default` 生成・旧 ini 仕様 (`Action.1` 形式)・exact match ヘルパー・native 動画転送対応・エッジケース規則・`KeyAction` インベントリ (付録 A)・キー変換ホワイトリスト (付録 B) |
| [keymap-manual-test-checklist.md](keymap-manual-test-checklist.md) | Win32 key edge queue / 日本語キーボード固有キー / テンキー分離 / native 動画経路を触ったときの手動検証チェックリスト。`MIV_KEY_DEBUG=1` のログ・オーバーレイ確認手順もここに集約 |
| [key-command-catalog-plan.md](key-command-catalog-plan.md) | **Phase 6 menu layout editor まで実装済み / ClaudeCode レビュー済み。native 動画 overlay ヘルプ重なり補正もレビュー済み。v2.2.0 重要変更点 entry 追加済み / ClaudeCode レビュー済み。旧 `keymap.ini` → `Settings.keymap` 一回移行済み。設定メニュー「操作カスタマイズ」独立ダイアログ移設 + 統合割り当て編集ダイアログ改善、場所系 / ページジャンプ KeyAction 追加済み、Enter / Backspace / Home / End / PageUp / PageDown の閲覧ナビ KeyAction 化スライス実装中**。簡易 keymap の次段階として、デフォルト未割り当て操作もコマンド設定から割り当て可能にする計画。Phase 1〜4 で既定未割り当て、競合 warning、グリッド F7-F10、フルスクリーン縦方向 resolver を段階実装。Phase 5 でツール / メニュー / hover / native 動画 overlay / ★ tooltip の shortcut 表記を keymap 追従に変更。Phase 7 では active scope から実 shortcut label を取り出す基盤と、各文脈のコンテキストヘルプ、`HelpShowContextShortcuts` によるヘルプキー自体の keymap 化まで実装済み。Phase 6 では `MenuCommandId` / `MenuCommandSpec` / `TopMenuId` の基盤、全 top menu の固定ラベル leaf 項目 catalog 化、parent 別 catalog iterator、enum ⇄ `ALL` drift テスト、stable name 文字列ベースの `MenuLayoutSettings` と resolver、`Settings.menu_layout` 永続化、固定 leaf 項目 / 空 top menu の表示 ON/OFF、top menu order とメニュー内 command order の描画接続、環境設定「表示 → メニュー構成」での固定 leaf 項目の表示 / 非表示と順序編集まで実装済み。動的ブロックは既存アンカーに残す。Esc / 修飾なし矢印は予約警告 |
| [ring-keyaction-parity.md](ring-keyaction-parity.md) | RingActionId と KeyAction の対応棚卸し。リング / パッド / ジェスチャ側だけに操作を足してキーボード側の KeyAction 化を忘れないための恒久チェック表。`src/keymap.rs` の parity テストと合わせて更新する |
| [operation-customize-share-plan.md](operation-customize-share-plan.md) | **実装済み** (バックログ §4.6 の正本)。操作カスタマイズ (keymap / ring_shortcuts / menu_layout の 3 点セット) を `.mivkeys.json` にエクスポート / インポート (置換のみ) して共有し、標準 / 現在 / 前世代との差分を表表示し、`settings.db` 世代 (bak1..bak10) から操作カスタマイズだけを再起動なしでライブ取り込みする機能。既存「設定の復元」ダイアログをタブ化 (`設定の復元` / `操作カスタマイズ`) して集約。スキーマ非変更 (世代を読むだけ + ファイル入出力 + 純関数差分) |
| [file-drag-drop-design.md](file-drag-drop-design.md) | グリッドからエクスプローラ等へファイルをドラッグ送出 (コピー) する機能の実装設計＋実装メモ。シェル `IDataObject` + `SHDoDragDrop` 方式。実装済み (2026-05、`src/file_drag.rs`)、残るは §8.2 の実機検証 |
| [subfolder-expansion-view-plan.md](subfolder-expansion-view-plan.md) | **実装済み (2026-07-20 更新)**。現在フォルダ以下の画像/動画、ZIP/PDF 本体、設定上の画像フォルダ本を snapshot 一覧化する `サブ展開` ビュー。全体/フォルダ単位ソート、10万件以上の続行確認、キャンセル可能な非同期表示準備、既存の詳細表示/★/タグ/場所 facet との連携を記載。ZIP/PDF/変換アーカイブ内部は対象外 |
| [ring-shortcut-plan.md](ring-shortcut-plan.md) | マウス右ドラッグ (リング / ジェスチャ) / ゲームパッド X リングショートカット + パッド専用ピッカーパネルの設計。右ドラッグ mode は 4 文脈ごとに `未使用` / `リングショートカット` / `マウスジェスチャ` を保存し、設定メニュー「操作カスタマイズ」から右ドラッグ mode / リング / マウスジェスチャ / マウス進む・戻る / ゲームパッド X+方向リングを編集する。ゲームパッド固定ボタン単体は既定動作固定。マウスジェスチャ追加は実際の右ドラッグ軌跡を記録する |
| [auto-thumb-aspect-plan.md](auto-thumb-aspect-plan.md) | サムネイル比率の自動選択 (`thumb_aspect_auto`) の設計と実装計画。`log(ratio)` の中央値 → 最近接バケット方式 + 6 段ゲート (min_samples / 連勝継続 / cooldown / 切替上限 / 入力 idle / log 距離マージン)。実装済み (2026-05、`src/auto_aspect.rs`) |
| [reading-history-plan.md](reading-history-plan.md) | 最近読んだフォルダ / ZIP / PDF を「閲覧履歴」として専用ビューに集める機能の実装メモ。記録対象は `Image` / `ZipImage` / `PdfPage` をフルスクリーンで開いた時、動画は除外。Ctrl+S コンテナ検索は対象、Ctrl+G アイテム検索とタグビューは対象外。変換アーカイブはキャッシュ ZIP ではなく元 RAR/7z/LZH を保存する。MVP 実装済み (2026-06、`src/reading_history_db.rs`) |
| [star-lock-snapshot-design.md](star-lock-snapshot-design.md) | **実装済み**。★固定一覧の snapshot、復元、最上位一覧 ownership の設計 |
| [local-adjustment-layer-v1.1.0-plan.md](local-adjustment-layer-v1.1.0-plan.md) | **実装済み**。全体補正と部分補正レイヤー、生成マスク、合成順、保存・無効化境界の設計 |
| [local-adjust-filter-candidates.md](local-adjust-filter-candidates.md) | 補正レイヤーへ追加していくフィルタ候補リスト。イラスト用途を主眼に、効果選択 UI 方針、優先度、実装難易度、詳細設計を整理 |
| [annotation-shapes-plan.md](annotation-shapes-plan.md) | **Stage 1〜4 実装済み (v1 完了、2026-07-13)**。テキスト注釈ツールへの注釈図形追加: 赤枠 (長方形/角丸/楕円)・注釈矢印 (Arrow フィールド加法拡張)・蛍光マーカー/下線 (**乗算、z 順セグメント合成・1 オブジェクト 1 モード・マーカーは枠線なし**)・番号バッジ (自動採番)・カーソルスタンプ (オリジナル SVG)。市場調査 (14 ツール) 付き。互換方針 = enum バリアント追加禁止・`#[serde(default)]` フィールド加法のみ。クリック設置で統一・新規 KeyAction なし |
| [annotation-align-distribute-plan.md](annotation-align-distribute-plan.md) | **v2.5.0 実装済み (2026-07-17)**。テキスト注釈の Ctrl/Shift 複数選択、矩形選択、グループ移動、6 方向整列、中心間隔／端の隙間による縦横均等配置、端・中央・等間隔スマートガイド。canonical source pixel 座標と既存 comic undo/save を維持 |
| [vertical-text-opentype-plan.md](vertical-text-opentype-plan.md) | 縦書き OpenType、約物、縦中横、レイアウトの現行設計 |
| [editing-add-on-download-spec.md](editing-add-on-download-spec.md) | **実装済み**。編集用追加パックのダウンロード、検証、保存先、manifest、ライセンス表示、TensorRT pack との分離方針を定める仕様 |
| [portable-build-plan.md](portable-build-plan.md) | **実装済み (v1.1.0+)**。loose-deps ポータブル版、`portable` feature、data-dir 分離、パッケージングと CI guard の設計・保守方針 |
| [comic-lab-validation-checklist.md](comic-lab-validation-checklist.md) | `tools/comic_lab` / `crates/comic-core` の実機検証チェックリスト。縦書き約物、IME、フォント、しっぽ、装飾、メッセージウィンドウ、本体統合時の P0 を整理 |
| [comic-lab-sample-text.md](comic-lab-sample-text.md) | comic ラボの縦書き・約物・IME・装飾を再現確認する標準サンプル文 |
| [music-lab-validation-checklist.md](music-lab-validation-checklist.md) | `tools/music_lab` / `crates/music-core` の実機検証チェックリスト。長尺ロード、自動再生、timeline 部分描画、spectrum analyzer、音切れ計測、本体統合時の置き換え境界を整理 |
| [compile-book-plan.md](compile-book-plan.md) | **v1.7.0 初期実装済み / 操作感調整前**。「製本」機能 = 複数の本/フォルダからページだけを集めて任意順に並べた束を作る。NeeView の参照型プレイリストと違いコピー型スナップショット (元削除に強い)。確定事項: 束=番号付き画像だけの純フォルダ (マーカー/サイドカー無し)・ページ順はゼロ埋め4桁連番ファイル名が正本 (Explorer/zip で順序自明)、1冊上限9999ページ、編集中はメモリ順序・専用モード退出時に遅延リネームフラッシュ (1000ページ最悪0.6秒実測)、置き場所は全ビルド `Pictures\mimageviewer\books` 既定・設定可 (portable も Pictures、capture既定は不変)、本の識別=場所ベース (ルート直下フォルダ)、**本フォルダは索引対象外+お気に入り登録対象外+ソート番号順固定** (大量リネーム時の索引チャーン回避、中はCtrl+F/ツールバーでin-memory絞り込み)、**本ページは焼き込み済み画像を正本にしつつ mIV 内部DBのタグ/★/補正/消しゴム/補正レイヤー/隠蔽/注釈/切り取りは後段で許可** (外部サイドカー無し、グローバル/お気に入り補正は継承しない、回転は抑制)、**並べ替えは専用モード** (小サムネ+ホバー拡大、shell ドラッグアウト回避)。複製は無加工コピー最優先 (グリッド追加では無補正なら通常画像/ZIP 内画像を再エンコードしない。PDF/動画/焼き込みのみエンコード)。追加トリガはグリッド(カーソル/選択)・画像・動画フレーム・クリップボード画像を追加先の本へ。既存 export/capture/コピー/グリッド流用。**追補 (2026-06-19, §11, 設計のみ): クイック追加=本のピン留め (`show_shortcut` 流用)・重複 skip+トースト / 登録済みバッジ (`book_membership_db`, パスベース provenance) / ツールバー本棚の折りたたみ・プルダウン表示。** |
| [toolbar-customization-plan.md](toolbar-customization-plan.md) | **実装済み (v2.0.0)**。ツールバーのセクション統一モデル + カスタマイズ refactor。全セクション (お気に入り/タグ/本棚/列/比率/ソート…) を 1 コンポーネントに揃え、**項目 左クリック=副作用なし (開く/ビュー) / 右クリック=副作用あり (追加/付与)**、表示形式 (展開/折りたたみ/プルダウン) を一般化、セクションの表示ON/OFF・順序 (ドラッグ並べ替え)・設定を**右クリック/⚙ に集約**して環境設定のツールバーページを撤去。順序はデータ駆動 (`ToolbarSectionId` の永続 Vec、`details_column_order` と同型)。詳細ヘッダー右クリックの idiom 流用。**①②整理機能より先行**。一度に完成形を目指す |
| [version-highlights-plan.md](version-highlights-plan.md) | **実装済み (v2.0.0) / v2.2.0 entry 追加済み / ClaudeCode レビュー済み**。更新後 初回起動に「重要な変更点 (主要部分)」を 1 画面で表示する汎用の仕組み。標準動作の変更 (例: ツールバー左右クリック) を**個別ダイアログを増やさず display-only** で告知。`update_check` (更新前・ネットワーク・全文) とは別で、**更新後・オフライン・操作/既定の変更中心**。既存の `last_seen_version`/`previous_last_seen_version`/`version_changed` を流用。複数バージョンまたぎは累積表示。ヘルプメニュー再表示は現行版以下の最新 entry にフォールバックするため、次リリース entry を先に埋め込んでも開発版で未来の告知を出さない。テストは**選択純関数の unit test + egui_kittest スナップショット + `--whatsnew-from` 強制表示**で実機最小。無効化設定なし |
| [filename-stack-plan.md](filename-stack-plan.md) | **実装済み (v2.0.0)**。ファイル名 prefix (末尾の区切り文字の前、既定 `_`) でフォルダ内画像を仮想スタックに畳む表示モード。pixiv/danbooru の「1 投稿=複数ファイル」を 1 サムネにまとめる。全グループを仮想スタック化 (単独=1 ページ) し、Ctrl+↑↓=スタック間 / ↑↓=スタック内のフルスクリーン二段ナビ。`ZipDir`/`SearchContainer` 仮想アイテム・Ctrl+G ドリル・`materialize` 見開きリセット・`start_loading_items` を流用、非破壊。②製本のピン本へスタック単位コピーと連携 |
| [filename-stack-scripting-plan.md](filename-stack-scripting-plan.md) | **実装済み (2026-06-21)**。スタックの分類ルールをユーザー定義 Rhai スクリプトで書ける拡張。契約 = メンバー列→同長キー配列の純関数 `group(files)` (**画像のみ**を渡す。各要素は `name/stem/ext/mtime/size`。動画は常に単独なので渡さず `is_video` も非公開)。`regex_is_match`/`regex_capture`/`regex_replace`/`argsort_int` を公開、操作上限つきサンドボックス。既定は内蔵カスケード (mXD/末尾連番/先頭連番/連写、all-match + 汎用は distinct≥2)。`<data_dir>/stack_rules.rhai` で上書き可、失敗時は組み込み既定へフォールバック。設定 `stack_script_enabled`、UI = 環境設定「フォルダ・ファイル」、ヘルプ = manual/stack.html |
| [rating-list-view-plan.md](rating-list-view-plan.md) | **Phase 1 実装済み (2026-06-23)、戻る導線追加済み (2026-07-05)**。場所▼ と ファイル メニューに ★1〜★5 を足し、選んだ★の付いたアイテム/コンテナを場所横断でフラット一覧する仮想ビュー。閲覧履歴/タグビューを雛形に `items_are_rating_view` を追加。**時刻ソートのため `rating.db` に `rated_at_ms` + `source_path` + `kind` + 仮想アイテム復元メタを後方互換追加** (リリース済み DB)。★設定時刻ソートはビュー固有 (表示中だけソートに追加、グローバル SortOrder は不変)。キー→GridItem 復元は新規行は kind/meta 直読み・旧行は parse+stat 推定。結果から開いたコンテナは `rating_view_nav_stack` でレーティング一覧へ戻る |
| [color-search-plan.md](color-search-plan.md) | **Phase 1/2 + Phase 3 実装済み (2026-06-23 起案 → Codex/ClaudeCode レビュー → オンデマンド方式へ転換)**。Eagle 風のカラー検索 (画像色で絞り込み)。**永続化しない**: 画像色フィルタを使う瞬間に現在の表示の画像アイテムを Ctrl+F 風にスキャン (在メモリ・進捗+キャンセル) してパレット抽出。画素入手は cache_map(WebP)→catalog(WebP)→必要時デコード後に縮小/サンプリング (JPEG は DCT で縮小 decode 可、PNG/WebP/WIC はフル decode になり得る) のフォールバック。抽出は量子化+知覚マージ+再割当で 8 色、照合は CIELAB ΔE76 + ratio_floor。色/許容変更は必要時に自動で一時スキャンを開始し、スキャン後は在メモリ再フィルタのみ。スキーマ/マイグレーション/設定追加なし。今回リリースは通常フォルダ / ZIP / PDF 表示限定で、Ctrl+G/タグ/お気に入り検索など集約ビュー開放は後続。メタデータパネルのスウォッチ表示、クリック起動、perf 計装、大量時確認 UI、Eagle 風ピッカー (SV/Hue、プリセット、HEX/RGB/HSL)、マニュアル/製品ページ更新まで実装済み。画面スポイトは実機で UI 応答と入力透過のリスクが大きかったため削除済み |
| [nested-zip-tree-plan.md](nested-zip-tree-plan.md) | **実装済み**。`ZipTree` / `ZipDir` によるネスト ZIP のツリーナビ、階層 materialize、サムネイルとナビゲーションの設計契約 |
| [rar-direct-read-plan.md](rar-direct-read-plan.md) | **実装済み (実 RAR の最終 smoke 対象)**。非ソリッド RAR/CBR の直読みと、ソリッド・入れ子・他形式を ZIP cache 変換へ委譲する routing 仕様 |
| [external-tool-launch-plan.md](external-tool-launch-plan.md) | **P0/P1/P2a/P2b/P2c/P3 実装済み (2026-09-01、P3 は実機確認待ち)、P4 以降は未実装**。backlog §1.117 の正本。導線は登録した全ツールを平坦に出す右クリックと、既定キーなしの Grid 専用固定スロット / ピッカーの 2 つ。ツールバーとメニューバーの直接起動は一度実装後に撤去した。フォルダー背景・コンテナー項目から現在のフォルダー / 本 1 件を渡す入口を持ち、複数対象は既定 `Each`、ツール別の確認 5 件 / 上限 10 件で扱う。変換アーカイブは元パスを使い、1 コンテナーに定まらない集約ビュー背景と仮想ページは拒否する。外部ツール起動を引数テンプレート / 作業フォルダー / 複数選択 / **ZIP・PDF 内ページの一時実体化** / **動画の現在フレーム**まで広げる設計で、**仮想パスをそのまま渡す方針は採らない**。一時ファイルは NeeView 同型のプロセス単位ディレクトリ + 終了時削除 + 起動時の孤児回収 |
| [sns-split-export-plan.md](sns-split-export-plan.md) | **P1〜P6 実装済み (2026-09-01)**。1 枚の絵を X / Instagram のカルーセル投稿用に 2〜4 枚へ切り分けて書き出す。`CropSettings` / `export_crop.db` には保存しない**一度きりのモード**として分離し、グループ矩形の操作だけを既存の `CropRect` と共有する。比率固定リサイズは反対辺 / 反対角を固定する共通挙動。**X の隙間を実測** (PC ブラウザ 1.588% / iOS アプリ 1.869% / モバイル Web 2.652%、隙間の絶対値は環境ごとに違う) し、枠幅比 **1.7% 固定**を採用。投稿先は X (3:4 / 1.7%) と Instagram (4:5 / 0%) の 2 択のみ。書き出しはパネルボタンから `ExportEntry` に crop を足し、既存の 1 スナップショット→N ファイル経路へ載せる。2x2 グリッドと縦並びは非対象 |

## 設計メモ (特定領域の詳細)

| ドキュメント | 内容 |
| --- | --- |
| [catalog-design.md](catalog-design.md) | サムネイルキャッシュ DB の設計 |
| [dct-scale-plan.md](dct-scale-plan.md) | TurboJPEG の DCT スケールデコードによるサムネ生成高速化。倍率選択と、圧縮入力サイズによるフォールバック条件 |
| [scroll-visibility-priority-plan.md](scroll-visibility-priority-plan.md) | スクロール停止後に可視サムネのジョブを優先レーンへ昇格させる仕組みと perf 計装 |
| [prefetch-suppression-during-scroll-plan.md](prefetch-suppression-during-scroll-plan.md) | スクロール中 / 可視待ち中に prefetch を enqueue しない判定と、永久 stall を防ぐ backstop |
| [duplicate-detection-plan.md](duplicate-detection-plan.md) | **設計 v0 (未実装)**。重複画像の検出と整理。狙いは「ほぼ同じ画像だが解像度違い / 軽微な修正」= 近重複のみ (埋め込みは使わない)。`similar.db` に 64bit + 256bit pHash を持ち、**索引を作らず線形走査**。白ページの退化ハッシュ対策 (`confidence`)、漫画は本単位で判定、削除は既存のゴミ箱経路、検証は既存の比較表示 (Diff) を再利用 |
| [similar-image-search-research.md](similar-image-search-research.md) | 上の前段の調査メモ。類似画像検索 / 重複検出。他ツール (hydrus / digiKam / XnView MP / Eagle / Komga / Immich) の機能と用途、perceptual hash と埋め込みのアルゴリズム比較、100 万件規模の実測 (索引不要という結論)、mIV へ載せる場合の保存先・スコープ・UI 案 |
| [dpi-multimonitor-issue.md](dpi-multimonitor-issue.md) | マルチモニター DPI 問題の調査記録 |
| [pdf-issues.md](pdf-issues.md) | PDF サポートの既知問題 |
| [pdf-pool-context-epoch-plan.md](pdf-pool-context-epoch-plan.md) | PDF レンダ pool の 3 段階優先度と、ナビゲーションで stale 化したジョブを除去する context epoch |
| [pdf-pool-harvest-on-cancel-plan.md](pdf-pool-harvest-on-cancel-plan.md) | cancel 時に in-flight の PDF レンダ結果を回収してキャッシュ保存する `CancelWaitPolicy` |
| [screenshot-howto.md](screenshot-howto.md) | 製品ページ用スクリーンショット手順 |
| [e2e-smoke-test.md](e2e-smoke-test.md) | E2E スモークテストのチェックリスト |
| [release-operations.md](release-operations.md) | **リリース運用メモ**。CLAUDE.md「リリース手順チェックリスト」の補助。過去リリースで踏んだ落とし穴・判断基準・復旧手順 (stale core cache / 署名セッション切れ / タグ再打ち直し / FFmpeg LGPL ソース同一性 / ポータブル AV 誤検知 / 通常設定を使わない隔離 UI 検証 / 配布チャネル別の注意) を集約。別セッション / Codex への引き継ぎ用 |
| [development-build-and-test.md](development-build-and-test.md) | 開発中の `cargo check` / 絞り込みテスト / 軽量 core ビルドと、リリース前の全体テストゲートの使い分け |
| [test-video-generation.md](test-video-generation.md) | `testimage/movie/test_*fps_*p_sync.mp4` (FFmpeg testsrc2 + sine ビープ) の再生成手順 |
| [ui-snapshot-policy.md](ui-snapshot-policy.md) | egui_kittest によるスナップショットテストの運用方針 |
| [downscale-moire-lod-plan.md](downscale-moire-lod-plan.md) | 静止画縮小時のモアレ原因と、vendored `egui-wgpu` による opt-in GPU mipmap、旧手動縮小フィルタの互換撤去方針 |
| [keymap-spec.md](keymap-spec.md) | キー / マウス操作仕様。フルスクリーン横断の詳細は [fullscreen-navigation-consistency.md](fullscreen-navigation-consistency.md) も参照 |
| [search-test-plan.md](search-test-plan.md) | 検索・notify-rs 監視・キー操作の自動テスト整備計画 |
| [search-container-item-redesign.md](search-container-item-redesign.md) | 検索を「コンテナ検索 (Ctrl+S) / アイテム検索 (Ctrl+G)」モデルへ整理する再設計案。Ctrl+G 一覧/集約ビュー・動画索引除外・mtime 追加・Ctrl+F の構造アイテム絞り込み |
| [tag-catalog-redesign-plan.md](tag-catalog-redesign-plan.md) | `tags.db`、タグ facet、メタデータ転送を含む現行タグ機能の正本 |
| [sidecar-metadata-ingest.md](sidecar-metadata-ingest.md) | サイドカー経由のメタデータ取り込み。**`tags.db` 移行前の記述が残っており内容更新待ち**。現行のタグ正本は上の tag-catalog-redesign-plan.md |
| [video-architecture.md](video-architecture.md) | 動画サブシステムの恒久正本。D3D11VA + native presenter、channel/pacing、current module responsibility、ownership 負債を記載。行数 snapshot は監査記録へ分離 |
| [brief-seek-thumbnail-measurement.md](brief-seek-thumbnail-measurement.md) | シークバーのプレビュー待ち時間の実測。4K AV1 での seek / decode / scale 内訳と、「シーク時のズレ許容 (秒)」の既定値・効果を決めた根拠 |
| [video-upscale-shader-plan.md](video-upscale-shader-plan.md) | **設計確定 / 未実装**。動画の拡大縮小を DWM/DComp から mIV のシェーダへ移す設計。表示解像度サーフェス、Phase A (Lanczos3/NIS/ニアレスト+縮小) と Phase B (Anime4K)、変種の一般化、実測によるモデル選択、切替で固まらせない不変条件 |
| [playback-speed-design.md](playback-speed-design.md) | 動画倍速再生機能の仕様。Signalsmith Stretch、音声 PTS/PDC/queue 秒数、native-only 速度 HUD、検証計画 |
| [ffmpeg-lgpl-source-distribution.md](ffmpeg-lgpl-source-distribution.md) | FFmpeg LGPLv3-or-later build の配布時チェックリスト、対応ソース、同梱外部ライブラリの確認メモ |
| [licensing-tensorrt.md](licensing-tensorrt.md) | TensorRT 対応のライセンス、再配布境界、確認事項 |
| [tensorrt-worker-design.md](tensorrt-worker-design.md) | TensorRT worker / IPC / fallback の現行設計 |
| [tensorrt-pack-distribution.md](tensorrt-pack-distribution.md) | TensorRT pack の作成・検証・配布 runbook |
| [tensorrt-pack-release-notes.md](tensorrt-pack-release-notes.md) | TensorRT pack 配布時のリリース本文の正本 |
| [ffmpeg-lgpl-current-report.txt](ffmpeg-lgpl-current-report.txt) | 現在の同梱 FFmpeg DLL から抽出した版、ライセンス、configure flags、GPL 混入検査の監査記録。依存更新時に `collect-ffmpeg-lgpl-info.ps1` で再生成する |
| [video-engine-redesign.md](video-engine-redesign.md) | エンジンの現行仕様 + 初期設計案 / 採否履歴。現行は `Arc<Mutex<EngineActor>>` + UI tick drain。未採用の `TransportController` / 専用 actor thread は将来候補として隔離 |
| [audio-normalize-scan-bench.md](audio-normalize-scan-bench.md) | 音量ノーマライズ初回スキャン待ち時間の実測用 CLI (`normalize_scan_bench`) と、HDD 上の動画で逐次 / 並列スキャンを比較するときの読み方 |
| [music-integration-plan.md](music-integration-plan.md) | **主要 Inc 実装完了**。`VideoPlayer` 再利用による音声再生、音楽ビュー、ブックマーク、VST3、動画→音声モードの統合契約と継続保守事項 |
| [vst3-integration.md](vst3-integration.md) | VST3 統合 — 1 chain = 1 C++ bridge、音声 IPC 1 roundtrip、bridge 内 per-slot STA editor、Rust chain/GUI/persistence/audio hot-path ownership と現行負債 |
| [settings-sqlite-migration.md](settings-sqlite-migration.md) | 設定永続化を `settings.json` から `settings.db` (SQLite) に移行する spec。transient NotFound による設定消失事故の構造的解消、将来版の未知設定値を `Incompatible` として無変更・save 抑止にする downgrade 保護、VST3 BLOB の dirty-skip による I/O 浪費解消。4 ラウンドの Codex review 反映済み |

## 進行中のレビュー

- [review-v2.8.1/README.md](review-v2.8.1/README.md) — v2.8.1 前の全体点検。領域別の
  docs↔コード整合監査の結果 (不一致 / リファクタ候補 / バグ) と、文書ごとの信頼度。
  **文書を現行仕様として読む前に、ここで該当文書の信頼度を確認すること。**

## 完了した作業の記録

完了した計画、brief、findings、レビュー、過去リリースの記録は
[archive/README.md](archive/README.md) から領域別に辿れます。現行仕様の正本としては扱わず、
設計判断や検収経緯を調べるときに参照してください。

---

## ドキュメント更新ルール

コード修正時は以下も同時に更新する (CLAUDE.md の指示に従う):

- 機能追加・変更・削除 → `spec.md` と `htdocs/mimageviewer/` を更新
- 設計レベルの変更 (キャッシュ構造・ワーカー構成・新しい永続ストレージなど)
  → 該当する設計ドキュメント (上記の「設計ドキュメント」セクション) を更新

**設計を変えたのに設計ドキュメントを放置しない**。このドキュメントが腐ると、
将来の自分 (または AI) が同じ罠を踏む。

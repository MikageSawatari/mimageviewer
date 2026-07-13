# docs/ — ドキュメント索引

修正作業の前に、関連するドキュメントを読んで全体設計を把握すること。

## 設計ドキュメント (これから作業する前に)

**迷ったらまず [architecture-overview.md](architecture-overview.md) から。**

| ドキュメント | 読むべきタイミング |
| --- | --- |
| [architecture-overview.md](architecture-overview.md) | 全体像の把握。レイヤー構造・モジュールマップ・永続化ストア一覧 |
| [display-pipeline.md](display-pipeline.md) | サムネイル表示・フルスクリーン描画を触るとき。**補正/AI/回転の適用順の決定版** |
| [async-architecture.md](async-architecture.md) | 並列処理・キャンセル・キャッシュ競合を触るとき。ワーカー一覧とテンプレ |
| [ui-responsiveness.md](ui-responsiveness.md) | UI スレッド同期 I/O で UI を止めないための設計方針。**新機能追加前にチェックリスト §4 を必ず見る** |
| [virtual-folders.md](virtual-folders.md) | ZIP/PDF 関連を触るとき。**通常画像との分岐チェックリスト** |
| [preset-and-adjustment.md](preset-and-adjustment.md) | 補正・プリセット・AI キャッシュを触るとき。**無効化ルールの早見表** |
| [search-architecture.md](search-architecture.md) | 検索 / インデクサ / タグを触るとき。**Ctrl+S/F/G の経路 + インデクサパイプライン + DB 責任分離** |
| [fullscreen-navigation-consistency.md](fullscreen-navigation-consistency.md) | フルスクリーン / 検索結果 / 動画タイルをまたぐ Ctrl+↑↓・境界ヒント・前後移動の統一仕様メモ |
| [keymap-spec.md](keymap-spec.md) + [key-customization-impl-plan.md](key-customization-impl-plan.md) + [key-command-catalog-plan.md](key-command-catalog-plan.md) | キーボード操作 / ショートカット / `consume_key` / `key_pressed` / native VK 判定 / コマンドカタログ化を触るとき。新しいキー操作は keymap 対応要否を必ず確認 |

## 仕様・機能

| ドキュメント | 内容 |
| --- | --- |
| [spec.md](spec.md) | アプリ全体の仕様書 (設定項目・機能一覧) |
| [feature-expansion-ideas.md](feature-expansion-ideas.md) | v0.10 候補 3 機能 (キャプチャ保存 / 比較ビュー / 動画連続再生) + スクロールバー視認性改善 の詳細仕様。Codex 第 2 ラウンドレビュー反映済、実装状況と設計メモを併記 |
| [next-release-backlog.md](next-release-backlog.md) | **次リリース検討バックログ**。未対応の P2/P3・要判断項目、ユーザー要望、依存ライブラリ更新、リリース前確認だけを恒久管理。完了した項目はこのファイルから削除する |
| [detached-viewer-implementation-plan.md](detached-viewer-implementation-plan.md) | 画像・動画を共通の別ウィンドウビューアとして扱う設計・実装メモ。F12 別ウィンドウモード、別ウィンドウの F11 仮想フルスクリーン、×/Esc/Enter/右クリックで session close、メイン一覧カーソルとの双方向同期、動画 native presenter の `NativeVideoPlacement::DetachedViewerChild` 化、close-to-tray 時の再生継続、ClaudeCode レビュー反映メモを整理 |
| [detached-image-window-stabilization-review-request.md](detached-image-window-stabilization-review-request.md) | 画像別ウィンドウ複数表示の安定化レビュー依頼メモ。context 分離前の暫定安定化記録で、現在の PDF / ZIP detached book viewer は [detached-viewer-context-separation-plan.md](detached-viewer-context-separation-plan.md) を正とする |
| [detached-viewer-context-separation-plan.md](detached-viewer-context-separation-plan.md) | **設計レビュー用**。PDF / ZIP / 画像フォルダを別ウィンドウで開いてもメイン本一覧を親一覧のまま残すための context 分離方針。main grid context と active detached viewer context を分け、passive window は display-only + reopen descriptor にする本対応計画 |
| [detached-window-current-behavior-investigation.md](detached-window-current-behavior-investigation.md) | **ClaudeCode レビュー依頼用**。detached window close / reactivation / passive geometry の現状調査。実機ログから、stable viewport id と HWND / placement lifetime の管理単位がずれている疑いを整理 |
| [detached-window-phase-a1-transient-audit.md](detached-window-phase-a1-transient-audit.md) | **ClaudeCode レビュー依頼用**。Phase A1: `ViewerContextBundle` / `swap_field!` の viewport-transient state 棚卸し。Phase A2 で bundle から外す候補、残す field、focus / borderless 系の扱いを整理 |
| [detached-window-phase-a2-runtime-separation.md](detached-window-phase-a2-runtime-separation.md) | **ClaudeCode レビュー依頼用**。Phase A2: active detached viewport runtime を `ViewerContextBundle` から分離した実装メモ。stale HWND / recreate flag を paused bundle から復元しない方針と検証観点を整理 |
| [detached-window-phase-b-placement-stabilization.md](detached-window-phase-b-placement-stabilization.md) | **ClaudeCode レビュー依頼用**。Phase B: active detached viewport の live placement を runtime として保持し、passive window の default geometry 誤採用を拒否する修正方針と検証観点を整理 |
| [detached-viewer-lifecycle-redesign-proposal.md](detached-viewer-lifecycle-redesign-proposal.md) | **設計/問題カタログ**。detached viewport ライフサイクルの「壊れている前提 (BA-1〜BA-7)」一覧と段階的作り直し方針。rect ベース host 捕捉・generation churn・host_lost 自動 recreate など根本原因を整理 |
| [detached-viewer-keepalive-design.md](detached-viewer-keepalive-design.md) | **設計 (正本) + Codex⇄ClaudeCode レビューログ**。アクティブ detached viewport を「毎フレーム必ず描画する」単一不変条件へ集約する keep-alive 設計。明示状態 `ActiveDetachedSession`、単一描画入口、rendered-frame marker、段階移行 (K0〜K3)。末尾の §7 でレビュー往復を記録 |
| [detached-viewer-edit-restriction-review-request.md](detached-viewer-edit-restriction-review-request.md) | **ClaudeCode レビュー依頼用**。連動なし detached viewer (ピン / always-new) で消しゴム・補正レイヤー等の画像編集機能を制限し、表示系操作だけを許可する仕様・実装レビュー観点 |
| [detached-viewer-smoke-checklist.md](detached-viewer-smoke-checklist.md) | **実機 smoke チェックリスト**。keep-alive (K0) 後の detached window をモード (通常/常に別ウィンドウ/F12 OFF) × コンテンツ × 操作で検証する手順。`MIV_DETACHED_WINDOW_DEBUG` のログ成功マーカー付き |
| [detached-viewer-smoke-matrix-20260630.md](detached-viewer-smoke-matrix-20260630.md) | **短縮版の実機検証マトリクス**。画像/ZIP/PDF/動画、常に別ウィンドウ設定、ZIP/PDF auto fullscreen、ピン/active 切替、V/Shift+Z/編集を少ない代表ケースで横断確認するためのチェック表 |
| [detached-rework-plan.md](detached-rework-plan.md) | **detached viewport リワーク正本**。rect ベース HWND 捕捉を撤去するための憲法、ステージ実行プロトコル、R0〜R4 の段階計画 |
| [detached-rework-stage-r0.md](detached-rework-stage-r0.md) | **Stage R0 指示書**。child viewport HWND を geometry 非依存で取得できるか調査するスパイク。public API 調査、EnumThreadWindows 差分法プロトタイプ、実機ログ採取条件 |
| [detached-rework-stage-r0-report.md](detached-rework-stage-r0-report.md) | **Stage R0 レポート**。egui/eframe public API 調査結果、`EnumThreadWindows` before/after 差分ログの実装内容、R1 推奨方式と実機 smoke 手順 |
| detached-rework-stage-r1/r1b/r1c/r2a〜r2d.md | 各ステージの指示書 + 検収記録 (R1 = HWND を生成イベントで 1 回だけ確定、R2 = `DetachedWindowRuntime` + reducer + placement 一本化)。進捗の現在地は plan §9 の表を見る |
| [detached-rework-stage-audio.md](detached-rework-stage-audio.md) | **音声メディア窓 (detached の音声ファイル / ParkedLive 音楽窓) の正本**。music_* global 方針 (§3.5)、メディア窓 1 本規則、動画→音声モードとの接続 |
| [detached-rework-ship-checklist.md](detached-rework-ship-checklist.md) | リワーク出荷前の実機 smoke マトリクス (F/W/V/P/R 系)。V9 = detached×音声モードは v2.3.0 レビューで追加 |
| detached-rework-findings-4〜19.md | 実機検証で見つかった問題の調査・修正記録シリーズ (findings-19 は fix1〜15 まで) |
| [review-v2.3.0/final-report.md](review-v2.3.0/final-report.md) | **v2.3.0 出荷前 品質レビュー統合レポート** (Codex×Claude 二重レビュー + 検収)。確定 P2 一覧・修正記録・追加バグハント結果・残課題。素材 (brief / codex-* / claude-*) と実機確認チェックリストも同ディレクトリ |
| [details-view-and-filter-plan.md](details-view-and-filter-plan.md) | **Ph1〜Ph4 + Ph5 画像/動画/作成日時遅延列まで実装済み**。ファイル選択画面の詳細表示モード (サムネ無しで名前/サイズ/日付＋★/タグ/編集フラグを行表示) ＋ Excel オートフィルタ風スマートフィルタの設計。現状は列セクションの詳細切替、右クリック列表示メニュー、`details_order` による列ヘッダ 3 トグルソート、種類/拡張子/場所/★/タグ/日付/サイズ/状態の共通 `FacetFilter`、遅延列 worker / 進捗表示、作成日時列、画像解像度列、長さ/動画解像度/コーデック列まで実装済み (長さ・コーデックは音声も対応)。場所は元ファイル/元コンテナの親フォルダで、製本フォルダは `本棚 > 本名` 表記。場所条件は移動で解除される非永続の一時条件。EXIF/PDF/アーカイブ系の追加遅延列は後続 |
| [shell-file-operations-context-menu-plan.md](shell-file-operations-context-menu-plan.md) | **一部実装済み**。Windows Shell の `IFileOperation` とネイティブ右クリックメニューへ寄せるファイル整理機能の実装計画。A/B クイックフォルダ、実ファイル/実フォルダの Shell 標準右クリックメニュー、rename、delete-to-recycle は実装済み。copy/move/drop の `IFileOperation` 化、仮想 ZIP/PDF アイテム向けの native custom menu は後続 |
| [key-customization-plan.md](key-customization-plan.md) | **設計履歴**。キー操作カスタマイズの調査・設計。現状の 3 入力経路 (egui consume / key_pressed / Win32 VK)・hold ジェスチャ・コンテキスト分割を調査し、フル版と簡易版 (テキスト ini / GUI なし / 競合検知なし) を比較。§8 が簡易版の確定設計 (入力パターン分類・複数チョード) |
| [key-customization-impl-plan.md](key-customization-impl-plan.md) | **実装済みメモ**。簡易版 (旧テキスト ini / GUI なし / 競合は警告のみ) の手順書と実装判断。現在の正本は `Settings.keymap` で、旧 `keymap.ini` は初回起動時に settings.db へ移行して `keymap.ini.imported*.bak` へ退避する。`src/keymap.rs` の型・`keymap.ini.default` 生成・旧 ini 仕様 (`Action.1` 形式)・exact match ヘルパー・native 動画転送対応・エッジケース規則・`KeyAction` インベントリ (付録 A)・キー変換ホワイトリスト (付録 B) |
| [keyslot-migration-plan.md](keyslot-migration-plan.md) | **v2.2.0 実装中**。`egui::Key` 由来の論理キーから、Win32 key edge queue + 物理キー寄り `KeySlot` へ移行する計画。日本語キーボード固有キー、テンキー分離、数字キーの互換既定割り当て、native 動画経路との統一を整理 |
| [keymap-manual-test-checklist.md](keymap-manual-test-checklist.md) | Win32 key edge queue / 日本語キーボード固有キー / テンキー分離 / native 動画経路を触ったときの手動検証チェックリスト。`MIV_KEY_DEBUG=1` のログ・オーバーレイ確認手順もここに集約 |
| [key-command-catalog-plan.md](key-command-catalog-plan.md) | **Phase 6 menu layout editor まで実装済み / ClaudeCode レビュー済み。native 動画 overlay ヘルプ重なり補正もレビュー済み。v2.2.0 重要変更点 entry 追加済み / ClaudeCode レビュー済み。旧 `keymap.ini` → `Settings.keymap` 一回移行済み。設定メニュー「操作カスタマイズ」独立ダイアログ移設 + 統合割り当て編集ダイアログ改善、場所系 / ページジャンプ KeyAction 追加済み、Enter / Backspace / Home / End / PageUp / PageDown の閲覧ナビ KeyAction 化スライス実装中**。簡易 keymap の次段階として、デフォルト未割り当て操作もコマンド設定から割り当て可能にする計画。Phase 1〜4 で既定未割り当て、競合 warning、グリッド F7-F10、フルスクリーン縦方向 resolver を段階実装。Phase 5 でツール / メニュー / hover / native 動画 overlay / ★ tooltip の shortcut 表記を keymap 追従に変更。Phase 7 では active scope から実 shortcut label を取り出す基盤と、各文脈のコンテキストヘルプ、`HelpShowContextShortcuts` によるヘルプキー自体の keymap 化まで実装済み。Phase 6 では `MenuCommandId` / `MenuCommandSpec` / `TopMenuId` の基盤、全 top menu の固定ラベル leaf 項目 catalog 化、parent 別 catalog iterator、enum ⇄ `ALL` drift テスト、stable name 文字列ベースの `MenuLayoutSettings` と resolver、`Settings.menu_layout` 永続化、固定 leaf 項目 / 空 top menu の表示 ON/OFF、top menu order とメニュー内 command order の描画接続、環境設定「表示 → メニュー構成」での固定 leaf 項目の表示 / 非表示と順序編集まで実装済み。動的ブロックは既存アンカーに残す。Esc / 修飾なし矢印は予約警告 |
| [ring-keyaction-parity.md](ring-keyaction-parity.md) | RingActionId と KeyAction の対応棚卸し。リング / パッド / ジェスチャ側だけに操作を足してキーボード側の KeyAction 化を忘れないための恒久チェック表。`src/keymap.rs` の parity テストと合わせて更新する |
| [operation-customize-share-plan.md](operation-customize-share-plan.md) | **実装済み** (バックログ §4.6 の正本)。操作カスタマイズ (keymap / ring_shortcuts / menu_layout の 3 点セット) を `.mivkeys.json` にエクスポート / インポート (置換のみ) して共有し、標準 / 現在 / 前世代との差分を表表示し、`settings.db` 世代 (bak1..bak10) から操作カスタマイズだけを再起動なしでライブ取り込みする機能。既存「設定の復元」ダイアログをタブ化 (`設定の復元` / `操作カスタマイズ`) して集約。スキーマ非変更 (世代を読むだけ + ファイル入出力 + 純関数差分) |
| [file-drag-drop-design.md](file-drag-drop-design.md) | グリッドからエクスプローラ等へファイルをドラッグ送出 (コピー) する機能の実装設計＋実装メモ。シェル `IDataObject` + `SHDoDragDrop` 方式。実装済み (2026-05、`src/file_drag.rs`)、残るは §8.2 の実機検証 |
| [subfolder-expansion-view-plan.md](subfolder-expansion-view-plan.md) | **初期実装済み (2026-06-24)**。現在フォルダ以下の画像/動画を、その時点のスナップショットとしてフラット一覧化する `サブ展開` ビューの設計・実装メモ。索引や watcher 追従は初期版では使わず、single-thread worker 走査 + synthetic flat view + 既存の詳細表示/★/タグ/場所 facet を流用する。ZIP/PDF/変換アーカイブ内部と画像色フィルタ解放は後続 |
| [ring-shortcut-plan.md](ring-shortcut-plan.md) | マウス右ドラッグ (リング / ジェスチャ) / ゲームパッド X リングショートカット + パッド専用ピッカーパネルの設計。右ドラッグ mode は 4 文脈ごとに `未使用` / `リングショートカット` / `マウスジェスチャ` を保存し、設定メニュー「操作カスタマイズ」から右ドラッグ mode / リング / マウスジェスチャ / マウス進む・戻る / ゲームパッド X+方向リングを編集する。ゲームパッド固定ボタン単体は既定動作固定。マウスジェスチャ追加は実際の右ドラッグ軌跡を記録する |
| [auto-thumb-aspect-plan.md](auto-thumb-aspect-plan.md) | サムネイル比率の自動選択 (`thumb_aspect_auto`) の設計と実装計画。`log(ratio)` の中央値 → 最近接バケット方式 + 6 段ゲート (min_samples / 連勝継続 / cooldown / 切替上限 / 入力 idle / log 距離マージン)。実装済み (2026-05、`src/auto_aspect.rs`) |
| [reading-history-plan.md](reading-history-plan.md) | 最近読んだフォルダ / ZIP / PDF を「読書履歴」として専用ビューに集める機能の実装メモ。記録対象は `Image` / `ZipImage` / `PdfPage` をフルスクリーンで開いた時、動画は除外。Ctrl+S コンテナ検索は対象、Ctrl+G アイテム検索とタグビューは対象外。変換アーカイブはキャッシュ ZIP ではなく元 RAR/7z/LZH を保存する。MVP 実装済み (2026-06、`src/reading_history_db.rs`) |
| [local-adjustment-layer-v1.1.0-plan.md](local-adjustment-layer-v1.1.0-plan.md) | **Codex 案**。v1.1.0 候補の画像補正ピボット計画。全体補正の強化、手描き/グラデーション/範囲/セグメンテーション生成マスク、マスク反転付きの部分補正レイヤーを、消しゴム後・隠蔽加工前の非破壊レイヤーとして追加する設計 |
| [local-adjust-filter-candidates.md](local-adjust-filter-candidates.md) | 補正レイヤーへ追加していくフィルタ候補リスト。イラスト用途を主眼に、効果選択 UI 方針、優先度、実装難易度、詳細設計を整理 |
| [speech-bubble-tool-design.md](speech-bubble-tool-design.md) | **Codex 案**。漫画 / AI イラスト投稿向けの吹き出し・セリフ入れツール設計。補正レイヤーとは分け、隠蔽加工後・crop 前に載せる前提で、テキスト、尾、縦書き、IME、保存、書き出しを整理 |
| [speech-bubble-text-tool-plan.md](speech-bubble-text-tool-plan.md) | **Claude 案** (上記 Codex 案と対。独立検証で結論一致)。同じ吹き出し・テキスト注釈ツールを実コードの型 / 関数 (`resolve_fs_processed_texture` / `ensure_conceal_texture` / `export_page_pixels_for_idx` / `page_path_key` / `conceal_db`・`local_adjust_db` パターン) に接続して設計。レンダリング基盤 (cosmic-text + 縦書き自前レイアウト + 共有レイアウトエンジン)、キャッシュ無効化表、機能リサーチ + 競合比較、フェーズ分けが厚い。縦中横の詳細は Codex 案を正とする |
| [annotation-shapes-plan.md](annotation-shapes-plan.md) | **Stage 1〜4 実装済み (v1 完了、2026-07-13)**。テキスト注釈ツールへの注釈図形追加: 赤枠 (長方形/角丸/楕円)・注釈矢印 (Arrow フィールド加法拡張)・蛍光マーカー/下線 (**乗算、z 順セグメント合成・1 オブジェクト 1 モード・マーカーは枠線なし**)・番号バッジ (自動採番)・カーソルスタンプ (オリジナル SVG)。市場調査 (14 ツール) 付き。互換方針 = enum バリアント追加禁止・`#[serde(default)]` フィールド加法のみ。クリック設置で統一・新規 KeyAction なし |
| [editing-add-on-download-spec.md](editing-add-on-download-spec.md) | miV 本体マージ後に実装する編集用追加パックの仕様。オノマトペ向け OFL フォント、被写体分離モデル、初回利用時のダウンロードモーダル、保存先、manifest、ライセンス表示、TensorRT pack との分離方針を整理 |
| [portable-build-plan.md](portable-build-plan.md) | **v1.1.0 候補 (設計のみ)**。loose-deps ポータブル版 zip の設計。`portable` cargo feature で native 依存 (pdfium/onnx/susie/vst3/ffmpeg/models) を include_bytes せず exe 隣から解決し、実行時展開ゼロ・launcher 不要にする。C ドライブ圧迫と AV 誤検知の同時解消が狙い。集約モジュール `native_assets` + data_dir 検出 + mutex 名分離 + パッケージング + メンテ保証 (CI guard) を整理 |
| [comic-lab-validation-checklist.md](comic-lab-validation-checklist.md) | `tools/comic_lab` / `crates/comic-core` の実機検証チェックリスト。縦書き約物、IME、フォント、しっぽ、装飾、メッセージウィンドウ、本体統合時の P0 を整理 |
| [music-lab-validation-checklist.md](music-lab-validation-checklist.md) | `tools/music_lab` / `crates/music-core` の実機検証チェックリスト。長尺ロード、自動再生、timeline 部分描画、spectrum analyzer、音切れ計測、本体統合時の置き換え境界を整理 |
| [ai-suggested-mask-v1.1.0-plan.md](ai-suggested-mask-v1.1.0-plan.md) | **Codex 案**。v1.1.0 候補の AI 提案マスク設計案。標準の顔検出 + ユーザー指定 ONNX 検出モデルを、消しゴム / 隠蔽加工のマスクオブジェクト生成に接続する。バッチ生成を v1.1.0 に含め、ShapeMeta / モデル登録 UI を提案 |
| [auto-mask-detection-plan.md](auto-mask-detection-plan.md) | **Claude 案** (上記 Codex 案と対). 同じ v1.1.0 自動マスク機能を実コードの型/関数 (`Shape`@mask_db.rs / `commit_conceal_shape`@ui_conceal.rs / `runtime.rs` / `ai_upscale` worker) に接続して設計。標準=MIT 同梱 YuNet、追加=BYO (`DetectorProfile`+`OutputFormat`、deepghs サイドカー自動読取)。v1.1.0 は現ページ対象・一括は将来フェーズ。検証済みライセンス表付き |
| [compile-book-plan.md](compile-book-plan.md) | **v1.7.0 初期実装済み / 操作感調整前**。「製本」機能 = 複数の本/フォルダからページだけを集めて任意順に並べた束を作る。NeeView の参照型プレイリストと違いコピー型スナップショット (元削除に強い)。確定事項: 束=番号付き画像だけの純フォルダ (マーカー/サイドカー無し)・ページ順はゼロ埋め4桁連番ファイル名が正本 (Explorer/zip で順序自明)、1冊上限9999ページ、編集中はメモリ順序・専用モード退出時に遅延リネームフラッシュ (1000ページ最悪0.6秒実測)、置き場所は全ビルド `Pictures\mimageviewer\books` 既定・設定可 (portable も Pictures、capture既定は不変)、本の識別=場所ベース (ルート直下フォルダ)、**本フォルダは索引対象外+お気に入り登録対象外+ソート番号順固定** (大量リネーム時の索引チャーン回避、中はCtrl+F/ツールバーでin-memory絞り込み)、**本ページは焼き込み済み画像を正本にしつつ mIV 内部DBのタグ/★/補正/消しゴム/補正レイヤー/隠蔽/注釈/切り取りは後段で許可** (外部サイドカー無し、グローバル/お気に入り補正は継承しない、回転は抑制)、**並べ替えは専用モード** (小サムネ+ホバー拡大、shell ドラッグアウト回避)。複製は無加工コピー最優先 (グリッド追加では無補正なら通常画像/ZIP 内画像を再エンコードしない。PDF/動画/焼き込みのみエンコード)。追加トリガはグリッド(カーソル/選択)・画像・動画フレーム・クリップボード画像を追加先の本へ。既存 export/capture/コピー/グリッド流用。**追補 (2026-06-19, §11, 設計のみ): クイック追加=本のピン留め (`show_shortcut` 流用)・重複 skip+トースト / 登録済みバッジ (`book_membership_db`, パスベース provenance) / ツールバー本棚の折りたたみ・プルダウン表示。** |
| [toolbar-customization-plan.md](toolbar-customization-plan.md) | **実装済み (v2.0.0)**。ツールバーのセクション統一モデル + カスタマイズ refactor。全セクション (お気に入り/タグ/本棚/列/比率/ソート…) を 1 コンポーネントに揃え、**項目 左クリック=副作用なし (開く/ビュー) / 右クリック=副作用あり (追加/付与)**、表示形式 (展開/折りたたみ/プルダウン) を一般化、セクションの表示ON/OFF・順序 (ドラッグ並べ替え)・設定を**右クリック/⚙ に集約**して環境設定のツールバーページを撤去。順序はデータ駆動 (`ToolbarSectionId` の永続 Vec、`details_column_order` と同型)。詳細ヘッダー右クリックの idiom 流用。**①②整理機能より先行**。一度に完成形を目指す |
| [version-highlights-plan.md](version-highlights-plan.md) | **実装済み (v2.0.0) / v2.2.0 entry 追加済み / ClaudeCode レビュー済み**。更新後 初回起動に「重要な変更点 (主要部分)」を 1 画面で表示する汎用の仕組み。標準動作の変更 (例: ツールバー左右クリック) を**個別ダイアログを増やさず display-only** で告知。`update_check` (更新前・ネットワーク・全文) とは別で、**更新後・オフライン・操作/既定の変更中心**。既存の `last_seen_version`/`previous_last_seen_version`/`version_changed` を流用。複数バージョンまたぎは累積表示。ヘルプメニュー再表示は現行版以下の最新 entry にフォールバックするため、次リリース entry を先に埋め込んでも開発版で未来の告知を出さない。テストは**選択純関数の unit test + egui_kittest スナップショット + `--whatsnew-from` 強制表示**で実機最小。無効化設定なし |
| [filename-stack-plan.md](filename-stack-plan.md) | **実装済み (v2.0.0)**。ファイル名 prefix (末尾の区切り文字の前、既定 `_`) でフォルダ内画像を仮想スタックに畳む表示モード。pixiv/danbooru の「1 投稿=複数ファイル」を 1 サムネにまとめる。全グループを仮想スタック化 (単独=1 ページ) し、Ctrl+↑↓=スタック間 / ↑↓=スタック内のフルスクリーン二段ナビ。`ZipDir`/`SearchContainer` 仮想アイテム・Ctrl+G ドリル・`materialize` 見開きリセット・`start_loading_items` を流用、非破壊。②製本のピン本へスタック単位コピーと連携 |
| [filename-stack-scripting-plan.md](filename-stack-scripting-plan.md) | **実装済み (2026-06-21)**。スタックの分類ルールをユーザー定義 Rhai スクリプトで書ける拡張。契約 = メンバー列→同長キー配列の純関数 `group(files)` (**画像のみ**を渡す。各要素は `name/stem/ext/mtime/size`。動画は常に単独なので渡さず `is_video` も非公開)。`regex_is_match`/`regex_capture`/`regex_replace`/`argsort_int` を公開、操作上限つきサンドボックス。既定は内蔵カスケード (mXD/末尾連番/先頭連番/連写、all-match + 汎用は distinct≥2)。`<data_dir>/stack_rules.rhai` で上書き可、失敗時は組み込み既定へフォールバック。設定 `stack_script_enabled`、UI = 環境設定「フォルダ」、ヘルプ = manual/stack.html |
| [ai-model-facet-plan.md](ai-model-facet-plan.md) | **v1.7.0 候補 (設計中)**。AI 生成モデル名 (+生成ツール) での絞り込みを既存スマートフィルタ (`FacetFilter`) に第二弾遅延ファセットとして追加。png_metadata の抽出済みデータを活用、ComfyUI のみ複数モデルで best-effort。EXIF条件/数値比較/正規表現は見送り。背景=NeeView パリティ監査の「検索表現力」を AI 整理ニッチに絞った部分 |
| [rating-list-view-plan.md](rating-list-view-plan.md) | **Phase 1 実装済み (2026-06-23)、戻る導線追加済み (2026-07-05)**。場所▼ と ファイル メニューに ★1〜★5 を足し、選んだ★の付いたアイテム/コンテナを場所横断でフラット一覧する仮想ビュー。読書履歴/タグビューを雛形に `items_are_rating_view` を追加。**時刻ソートのため `rating.db` に `rated_at_ms` + `source_path` + `kind` + 仮想アイテム復元メタを後方互換追加** (リリース済み DB)。★設定時刻ソートはビュー固有 (表示中だけソートに追加、グローバル SortOrder は不変)。キー→GridItem 復元は新規行は kind/meta 直読み・旧行は parse+stat 推定。結果から開いたコンテナは `rating_view_nav_stack` でレーティング一覧へ戻る |
| [color-search-plan.md](color-search-plan.md) | **Phase 1/2 + Phase 3 実装済み (2026-06-23 起案 → Codex/ClaudeCode レビュー → オンデマンド方式へ転換)**。Eagle 風のカラー検索 (画像色で絞り込み)。**永続化しない**: 画像色フィルタを使う瞬間に現在の表示の画像アイテムを Ctrl+F 風にスキャン (在メモリ・進捗+キャンセル) してパレット抽出。画素入手は cache_map(WebP)→catalog(WebP)→必要時デコード後に縮小/サンプリング (JPEG は DCT で縮小 decode 可、PNG/WebP/WIC はフル decode になり得る) のフォールバック。抽出は量子化+知覚マージ+再割当で 8 色、照合は CIELAB ΔE76 + ratio_floor。色/許容変更は必要時に自動で一時スキャンを開始し、スキャン後は在メモリ再フィルタのみ。スキーマ/マイグレーション/設定追加なし。今回リリースは通常フォルダ / ZIP / PDF 表示限定で、Ctrl+G/タグ/お気に入り検索など集約ビュー開放は後続。メタデータパネルのスウォッチ表示、クリック起動、perf 計装、大量時確認 UI、Eagle 風ピッカー (SV/Hue、プリセット、HEX/RGB/HSL)、マニュアル/製品ページ更新まで実装済み。画面スポイトは実機で UI 応答と入力透過のリスクが大きかったため削除済み |
| [ai-metadata-parser-expansion-plan.md](ai-metadata-parser-expansion-plan.md) | **v1.3.0 実装済み**。AI 生成メタデータパーサの形式拡充 (NovelAI / InvokeAI / SwarmUI / Fooocus 系 / JPEG EXIF UserComment)。NovelAI 誤判別 + Negative prompt 索引混入を consumed_keys 方式で修正し、JSON 生成メタデータを汎用 JSON より優先解釈。INDEX_VERSION=9 で再構築 |
| [nested-zip-tree-plan.md](nested-zip-tree-plan.md) | **v1.3.0 候補 (設計確定/実装着手前)**。ネスト ZIP を現在のフラット展開からツリーナビへ変える設計。`entry_name` を不変に保ち表示層のみ追加 (DB 移行ゼロ)、Ctrl+G ドリルダウンを流用して内側 ZIP/サブフォルダを階層移動。items が現在の本だけになるので見開きペアリングが本ごとにリセットされ相性問題を解消。Claude/Codex 独立合意の設計を固定 |
| [rar-direct-read-plan.md](rar-direct-read-plan.md) | **未着手 (設計確定、バックログ 1.5)**。非ソリッド・入れ子なしの RAR/CBR だけを cache 変換せず直読みし、それ以外 (ソリッド/入れ子/7z/LZH) は従来の ZIP cache 変換に委譲する設計。実装は道B (`GridItem::ZipImage` を `zip_path=.rar` で再利用し、フォーマット分岐を `zip_loader` 末端に閉じ込める) で ~1,000–2,500 行。あわせて「変換 > ZIP ファイルに変換」で同名 `.zip` を生成するメニューと、同名 `.zip`/RAR/7z/LZH がある時に `.zip` だけ表示する設定 (既定 ON、同名ファイル処理群) を追加。直読み判定は `open_for_listing().is_solid()` + `nested_archive_kind` 走査を worker 内で 1 回 |
| [final-smart-sharpen-plan.md](final-smart-sharpen-plan.md) | **v1.3.0 実装済み (設計経緯メモ)**。画像補正パネルに 1 本スライダーの最終段スマートシャープを追加する計画。AI モデルではなく既存 `SmartSharpen` 系の計算式を final pipeline に入れ、サムネイル非反映、CPU 並列化、post_filter との併用を前提に整理。実装の正本は [preset-and-adjustment.md §2.6](preset-and-adjustment.md) |

## 設計メモ (特定領域の詳細)

| ドキュメント | 内容 |
| --- | --- |
| [catalog-design.md](catalog-design.md) | サムネイルキャッシュ DB の設計 |
| [ai-region-segmentation-retrospective.md](ai-region-segmentation-retrospective.md) | `local_adjust_lab` で試した SAM / SAM2 領域分割の失敗メモ。v1.1.0 では AI 領域分割を見送り、クラシック領域分割へ集中する判断の背景 |
| [thumbnail-memory-redesign.md](thumbnail-memory-redesign.md) | サムネイルメモリ管理の再設計経緯 |
| [dpi-multimonitor-issue.md](dpi-multimonitor-issue.md) | マルチモニター DPI 問題の調査記録 |
| [pdf-issues.md](pdf-issues.md) | PDF サポートの既知問題 |
| [screenshot-howto.md](screenshot-howto.md) | 製品ページ用スクリーンショット手順 |
| [e2e-smoke-test.md](e2e-smoke-test.md) | E2E スモークテストのチェックリスト |
| [test-video-generation.md](test-video-generation.md) | `testimage/movie/test_*fps_*p_sync.mp4` (FFmpeg testsrc2 + sine ビープ) の再生成手順 |
| [ui-snapshot-policy.md](ui-snapshot-policy.md) | egui_kittest によるスナップショットテストの運用方針 |
| [keymap-spec.md](keymap-spec.md) | キー / マウス操作仕様。フルスクリーン横断の詳細は [fullscreen-navigation-consistency.md](fullscreen-navigation-consistency.md) も参照 |
| [bench-scroll-report.md](bench-scroll-report.md) | スクロール性能ベンチマーク結果 |
| [perf-investigation-handoff.md](perf-investigation-handoff.md) | パフォーマンス調査の進行中メモ (AI アップスケール優先度 / スクロール中の重複エンキュー抑制) |
| [plan-v0.7.0.md](plan-v0.7.0.md) | v0.7.0 実装計画 + 完了ステータス + リリース直前チェックリスト |
| [search-expansion-design.md](search-expansion-design.md) | 検索システムの仕様選択理由と背景資料 (Tantivy スキーマ・ZIP ingest 負荷制御・UI drill-down・streaming プロトコル)。**v5 までの旧設計 (二段整合性 / SQLite 内 norms) を含む** — 現行 v6 設計は [search-architecture.md](search-architecture.md) §4.2 を参照 |
| [search-bench-results.md](search-bench-results.md) | Tantivy + bigram プロトタイプ計測結果 (50 万件規模まで) |
| [search-test-plan.md](search-test-plan.md) | 検索・notify-rs 監視・キー操作の自動テスト整備計画 |
| [search-container-item-redesign.md](search-container-item-redesign.md) | 検索を「コンテナ検索 (Ctrl+S) / アイテム検索 (Ctrl+G)」モデルへ整理する再設計案。Ctrl+G 一覧/集約ビュー・動画索引除外・mtime 追加・Ctrl+F の構造アイテム絞り込み |
| [tag-feature.md](tag-feature.md) / [tag-catalog-redesign-plan.md](tag-catalog-redesign-plan.md) | mIV タグ機能。現行は `tags.db` 正本 + facet 絞り込み、旧 `dc:subject` タグは移行対象 |
| [video-architecture.md](video-architecture.md) | 動画インライン再生サブシステムの設計指針と内部構造 (D3D11VA HW デコード + DX12 zero-copy interop + CPU fallback)。**Phase 2 (DComp / NVIDIA VSR) 撤回の経緯も巻末に記載** |
| [playback-speed-design.md](playback-speed-design.md) | 動画倍速再生機能の仕様。Signalsmith Stretch 採用、AvClock 中心の速度配線、音声 PTS/PDC/queue 秒数の扱い、HUD UI、検証計画 |
| [dcomp-native-presenter-integration-plan.md](dcomp-native-presenter-integration-plan.md) | DirectComposition native video presenter prototype を本番 fullscreen path へ統合するための段階計画。1080p120/165Hz 対応、egui overlay 分離、DPI/resize/VST owner 課題を整理 |
| [dcomp-overlay-egui-technical-brief.md](dcomp-overlay-egui-technical-brief.md) | DirectComposition native presenter Phase C overlay で egui-wgpu をどう載せるかの技術選択メモ |
| [codex-native-overlay-redraw-cadence-brief.md](codex-native-overlay-redraw-cadence-brief.md) | Phase C native egui overlay の redraw cadence / render_ms を soak で確認するための計測ブリーフ |
| [codex-native-presenter-copy-spike-brief.md](codex-native-presenter-copy-spike-brief.md) | Production native DComp presenter の `copy_ms` / `fence_wait_ms` spike を per-present trace soak で切り分ける計測ブリーフ |
| [ffmpeg-lgpl-source-distribution.md](ffmpeg-lgpl-source-distribution.md) | FFmpeg LGPLv3-or-later build の配布時チェックリスト、対応ソース、同梱外部ライブラリの確認メモ |
| [codex-video-upscale-resumable-segments-design.md](codex-video-upscale-resumable-segments-design.md) | Offline video upscale の resumable segment / persistent queue 設計 |
| [codex-video-upscale-resumable-segments-phasec-implementation-review.md](codex-video-upscale-resumable-segments-phasec-implementation-review.md) | Offline video upscale Phase C/D/E 実装レビュー依頼メモ |
| [video-engine-redesign.md](video-engine-redesign.md) | エンジン側 (`AvClock` / `EngineActor` / `MasterClock` / `AudioBookkeeping`) のリデザイン経緯と各 Phase 詳細。Phase 8.K の pacing 仕様、Phase 9 の 3-thread 分離、Phase 9.A〜9.G の追加修正 (wall-rate cap / cpal warmup silence / forward seek backward+preroll / perf overlay seek freeze 等) を網羅 |
| [audio-normalize-scan-bench.md](audio-normalize-scan-bench.md) | 音量ノーマライズ初回スキャン待ち時間の実測用 CLI (`normalize_scan_bench`) と、HDD 上の動画で逐次 / 並列スキャンを比較するときの読み方 |
| [music-lab-plan.md](music-lab-plan.md) | 音楽ファイル対応の分離ラボ設計。`crates/music-core` / `tools/music_lab`、動画の音楽モード化、VST3 bridge 接続を見越した effect-chain 境界 |
| [music-integration-plan.md](music-integration-plan.md) | **実装中 (Inc 0〜、2026-07-01 起案)**。music lab を本体へ統合する契約書。`VideoPlayer` 再利用で再生/VST3/normalize を得て、`GridItem::Audio` + 音楽ビュー (timeline/spectrum + 上下バー常時) + 左ブックマーク (動画機構 `video_bookmarks*` 再利用) + 動画→音声モードを段階実装。実装=Claude / レビュー=Codex。§7.9 に music 固有のサブシステム配線コントラクト |
| [vst3-integration.md](vst3-integration.md) | VST3 プラグイン統合 (v0.9.0+) — C++ bridge プロセス + Rust IPC、audio-pump からの bridge 経由、プラグイン GUI のクロスプロセス attach、チェーン編集 UI、再生中 VST3 パネル、後段 safety limiter |
| [settings-sqlite-migration.md](settings-sqlite-migration.md) | 設定永続化を `settings.json` から `settings.db` (SQLite) に移行する spec。transient NotFound による設定消失事故の構造的解消 + VST3 BLOB の dirty-skip による I/O 浪費解消。4 ラウンドの Codex review 反映済み |

---

## ドキュメント更新ルール

コード修正時は以下も同時に更新する (CLAUDE.md の指示に従う):

- 機能追加・変更・削除 → `spec.md` と `htdocs/mimageviewer/` を更新
- 設計レベルの変更 (キャッシュ構造・ワーカー構成・新しい永続ストレージなど)
  → 該当する設計ドキュメント (上記の「設計ドキュメント」セクション) を更新

**設計を変えたのに設計ドキュメントを放置しない**。このドキュメントが腐ると、
将来の自分 (または AI) が同じ罠を踏む。

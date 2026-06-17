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

### 1.1 マウス戻る / 進むボタンの割り当てカスタマイズ

- 背景: 5ch レス 790 / 791。現状の 5 ボタンマウス戻る / 進むは `Ctrl+↑/↓` 相当のツリー順フォルダ移動だが、ボタン名どおりの「戻る / 進む」や「戻る=上の階層」を希望する声が出ている。
- 要望整理:
  - レス 790 は、ページ送り履歴ではなく、フォルダバーの履歴戻る / 進む (`Alt+←/→`) を期待している可能性が高い。
  - レス 791 は、戻るボタンを「上の階層」(`Backspace` / `Alt+↑`) にしたいという明示的な希望。
  - したがって「戻る / 進む」を固定のペア設定だけにせず、少なくとも戻るボタンと進むボタンを個別に割り当てられる設計を検討する。
- 候補アクション:
  - フォルダ履歴 戻る / 進む。
  - 親フォルダへ。
  - ツリー順の前 / 次フォルダ (従来動作)。
  - 前 / 次ページまたは前 / 次アイテム。
  - 無効。
- 実装メモ:
  - 次回予定のゲームパッド / リングショートカット対応 (`docs/ring-shortcut-plan.md`) と同じ設定 UI / apply 層で扱う。
  - `egui::PointerButton::Extra1/Extra2`、`WM_APPCOMMAND` / `VK_BROWSER_BACK/FORWARD` 経由、native video presenter の `Extra1/Extra2` を同じ設定へ通す。
  - グリッド、画像フルスクリーン、動画フルスクリーン、F12 別ウィンドウで同じ方針にする。F12 / native video は入力経路が別なので実機確認必須。
  - 通常ホイールは連結読み・編集ズーム・グリッドスクロールなど文脈依存が強いため、初期対応では対象外が安全。Shift / Alt + ホイール等の拡張は `docs/ring-shortcut-plan.md` の段階実装に寄せる。
- 優先度: P2。次回の入力カスタマイズ対応に含める。

### 1.2 フルスクリーン / 別ウィンドウ / 連結読みの追跡調査

- 背景: 5ch レス 792。v1.7.0 時点で実用上は問題ないが、フルスクリーン・F12 別ウィンドウ・連結読み周辺で複数の気づきが報告された。
- 調査項目:
  - Windows 11 仮想デスクトップ切替時に、専用フルスクリーンウィンドウが別デスクトップにも付いてくる。
    - egui / winit の multi-viewport、Win32 の topmost / foreground / virtual desktop 挙動が絡む可能性がある。
    - まずは再現確認。単純な egui 設定だけで直らない場合は、Win32 側で現在デスクトップの判定や hide / restore が必要になる可能性がある。
  - F12 別ウィンドウモードの画像ウィンドウをフルスクリーン化できない。
    - detached viewer は通常の装飾付き viewport として設計しており、既存のフルスクリーン takeover (main cloak / focus reclaim / native presenter) とは別系統。
    - いきなり真のボーダーレス fullscreen にせず、まずは最大化 / F11 相当を detached viewport へ安全に適用できるか調査する。
  - F12 有効 + 縦 / 横連結スクロール中に、次画像へ一気にスクロールジャンプすることがある。
    - 連結読みの scroll anchor と detached viewer の選択同期 / `open_fullscreen` 再入場が競合していないか確認する。
  - 縦 / 横連結でスクロールして見えてきたページに AI アップスケールが遅れて適用される。
    - 現在ページ中心だけでなく、viewport に入り始めるページ / 前後 lookahead を final AI prefetch 対象にできないか確認する。
- 優先度: P2/P3。フルスクリーン周辺は egui multi-viewport と Win32 の境界があり、調査タスクとして分けて扱う。

---

## 2. アーカイブ / 仮想フォルダ

現時点ではなし。

---

## 3. フォルダツリーペイン

### 3.1 folder pane scan worker の thread 構成判断

- 背景: `scan_real_subfolders` はノードごとに短命 thread を spawn する。
- 現状: `folder_pane/scan_subfolders` perf event で ms / entry 数 / dir 数 / cancel / error を記録済み。
  cancel 付きで thread leak は見えていない。
- 方針:
  - 低速共有や大量ノード展開で遅い scan / concurrent scan が見えた場合だけ、dispatcher / pool 方式へ寄せる。
- 優先度: P3。

## 4. 補正 / AI

### 4.1 local-adjust layers の入場時同期 DB 読み

- 背景: フルスクリーン入場初回フレームで `LocalAdjustDb::get_layers` を同期実行する。
- 現状: フォルダ open 一括読みを避けるための意図的 tradeoff。
- 方針:
  - 数十 MB 級ページで hitch が報告 / 計測された場合に worker 化する。
  - read-only 経路の not-loaded は現状どおり None 返しを維持する。
- 優先度: P3 monitor。

### 4.2 AI 補正済み画像を含む書庫 / フォルダの絞り込み

- 背景: 5ch レス 791。「AI補正された画像が含まれる書庫の絞り込み」ができないかという要望。
- 解釈:
  - 「AI補正」は、AI アップスケール / AI ノイズ除去、または画像補正パネルで個別 AI 設定を持つページを指す可能性が高い。
  - 「書庫の絞り込み」は、ZIP / PDF / 変換済み RAR 等のコンテナ単位で、中に AI 設定済みページがあるものだけを一覧表示したいという意味に見える。
- 方針:
  - まずは定義を明確化する。候補は「AIアップスケールON」「AIノイズ除去ON」「AIモデル個別指定」「最終AI結果キャッシュあり」だが、キャッシュ有無は一時状態なので検索条件には使わない。
  - 実装するなら、ページ単位の補正 / AI 設定をコンテナへ集約する遅延ファセットまたはフィルタ条件として扱う。
  - ZIP / PDF / 変換アーカイブ内ページ、通常フォルダ内画像、本棚ページでキー体系が異なるため、`docs/virtual-folders.md` と `docs/preset-and-adjustment.md` を読み直してから設計する。
- 優先度: P3。検索 / スマートフィルタ側の機能拡張として検討。

### 4.3 補正変更時の final AI キャッシュ無効化漏れ調査

- 背景: 5ch レス 792-⑤。画像補正パラメータを変更しても AI アップスケール済みキャッシュが優先され、ページ移動後に変更が効いていないように見えるという報告。
- 期待動作:
  - 明るさ / コントラスト / 色調など、final AI 入力に影響する補正が変わった場合は、表示中キャッシュ・保持 LRU・pending final AI を正しく無効化する。
  - post filter / smart sharpen のみの変更では final AI を保持し、後段だけ再適用する。
- 調査観点:
  - 左パネルのスライダー、保存スロット適用、全画像適用、標準化、Undo / Redo、ページ個別解除の全経路で同じ invalidation helper を通っているか。
  - `retained_final_ai_cache` / PDF page slot / detached viewer 再入場時に古い final AI が復活しないか。
- 優先度: P2。表示結果の正しさに関わるため、再現できれば早めに修正。

### 4.4 crop 後フィットとベタ塗り自動クロップ

- 背景: 5ch レス 792-⑥⑦。
- 要望:
  - crop 適用後は、元画像サイズではなく crop 後の表示サイズに対してズーム / フィットしてほしい。
  - 四辺のベタ塗り部分を検出して、自動 crop するボタンがほしい。
- 方針:
  - 既存の crop は非破壊編集として final composite 前段に入るため、フィット計算が crop 後の `final_composite_cache` 寸法を使っているか確認する。
  - 余白カットフィット (`fs_margin_bbox_cache`) は表示時ズームだけでピクセルを切らない。自動 crop ボタンは、同じ検出思想を使いつつ非破壊 crop パラメータへ書き込む別機能として扱う。
  - ベタ塗り検出は白 / 黒だけでなく、ページ外枠の近似単色・薄いグレー・色付き背景を許容するかを検討する。本文や枠線を削りすぎない安全側のしきい値にする。
- 優先度: P3。まず crop 後フィットのバグ有無を確認し、自動 crop は別タスクで検討。

---

## 5. リリース前確認 / 依存更新

### 5.1 ネイティブ依存

| 対象 | 現状 / 次の確認 | 注意点 |
| --- | --- | --- |
| PDFium | vendor 更新後の PDF 表示手動確認が必要 | PDF 開封、ページ列挙、サムネ、フルスクリーン、パスワード PDF |
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
| ZIP / PDF / 変換アーカイブ | `docs/virtual-folders.md`, `docs/shell-file-operations-context-menu-plan.md` |
| フォルダ移動 / Ctrl+↑↓ | `docs/fullscreen-navigation-consistency.md`, `docs/keymap-spec.md` |
| 入力カスタマイズ / マウス / ゲームパッド | `docs/keymap-spec.md`, `docs/key-customization-impl-plan.md`, `docs/ring-shortcut-plan.md` |
| フルスクリーン / F12 別ウィンドウ / 連結読み | `docs/display-pipeline.md`, `docs/detached-viewer-implementation-plan.md`, `docs/fullscreen-navigation-consistency.md` |
| 表示 / AI / 補正 | `docs/display-pipeline.md`, `docs/preset-and-adjustment.md` |
| リリース / 依存更新 | `CLAUDE.md` のリリース手順、各 native 依存管理節 |

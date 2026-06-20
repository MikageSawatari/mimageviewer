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

### 1.1 詳細表示の右端欠け / 不要な横スクロールバー / 名前列幅固定

- 背景: 5ch レス 815。詳細表示で右端列にサイズ・解像度などの右寄せ項目を置くと、
  縦スクロールバー側で文字の右端が 1〜2px 欠ける。また、縦スクロールバーが出るだけで
  本来不要そうな下部横スクロールバーも出る。名前列も任意幅へ調整したいという要望。
- 現状理解:
  - 報告は「縦スクロールバーが列の上に大きく重なる」というより、詳細一覧全体の
    横方向 content width / viewport width の計算が縦スクロールバー幅ぶん数 px ずれ、
    右端列が初期位置で表示領域の外へはみ出している症状と見る。
  - コード上、詳細リストは外側 `ScrollArea::horizontal`、内側 `ScrollArea::vertical` で、
    `details_content_width(avail_w, settings)` をヘッダと行の両方に渡している。
    縦スクロールバー出現時の実効 viewport 幅 / scrollbar gutter と、
    横スクロール範囲・列矩形の基準幅を揃える必要がある。
  - 名前列は `details_column_rects` で「全体幅 - 他列合計」を割り当てる可変列。
    `details_column_width` / `set_details_column_width` / `sanitize_details_column_widths` でも
    `DetailsColumn::Name` / `DetailsColumnId::Name` は固定幅保存の対象外になっている。
- 修正方針:
  - 縦スクロールバーが出る場合でも、ヘッダ / 行 / 横スクロール範囲が同じ content width を
    参照し、右端列のテキスト clip が縦スクロールバー側へ食い込まないようにする。
  - 横スクロールが不要な列構成では、縦スクロールバー出現だけで下部横スクロールバーが
    出ないようにする。必要なら vertical scrollbar gutter 分を body 側の有効幅から差し引くか、
    右端の安全余白として明示的に扱う。
  - 確認ケース: 右端列を「サイズ」「画像解像度」「動画長さ」など右寄せ列にした状態で、
    縦スクロールバーあり / なし、横スクロールあり / なし、列順変更後の表示を確認する。
- 名前列幅固定の仕様:
  - 既定は現在どおり自動調整。名前列は残り幅を埋め、不要な横スクロールを出しにくい挙動を維持する。
  - 名前列の境界をドラッグしたら、その時点で名前列を固定幅モードに切り替え、幅を永続化する。
  - ヘッダ右クリックメニューに `名前の幅を自動調整` チェックを追加する。
    ON で自動調整に戻し、保存済みの名前幅は無視またはクリアする。OFF にした場合は、
    現在表示中の名前列幅を固定幅として保存する。
  - 固定幅モードでは `details_content_width` に名前列の保存幅を含め、
    他列と同様に横スクロールで全列を確認できるようにする。
  - 既存設定の移行は `details_name_width_auto = true` 相当を既定とし、旧設定では現在の挙動を維持する。
- 優先度: P2。v1.9.x / v1.10 候補。

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

### 4.2 補正パラメータ変更後に AI アップスケールキャッシュが優先される疑い (再現待ち)

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

### 4.3 表示トリム / 余白カット

- 背景: 5ch レス 792-⑥。要望文は「crop 後フィット」だったが、既存の crop は投稿 / 書き出し用の
  「切り取り」で、漫画ビューア用途の「読みながらサクッと余白を詰める」機能とは目的が違う。
- 実装済み (2026-06-17):
  - 左ホバーの補正パネルを「画像補正 / 表示トリム」のタブ式にし、選択タブを設定保存する。
    上部バー右側の表示トリムボタン / 画像補正アイコンは削除した。
  - 表示トリムタブで、トリムなし / 自動余白カット / 本全体の設定を適用、を
    ラジオで切り替えられる。このページ個別設定は現在ページだけのチェックで適用し、
    前後ページへ移動するとチェックは外れる。
  - 本全体 / このページの手動設定では、単ページ / 見開き連動 (上・下・中央側・外側) /
    見開き左右別を 0〜20% のスライダーで調整できる。
  - 自動余白カットは表示中ページごとに検出する。自動検出ボタンは現在ページ / 見開きの
    単色余白を本全体 / このページの手動スライダーへ反映する。
  - `draw_fs_image` / `draw_fs_spread` の content bbox 経路に統合し、ページ全体 / 横幅 / 縦幅 /
    100% 原寸でも表示トリム後の矩形を fit 基準にする。
  - bbox 外は描画せず背景色に落とす。中央側のトリムは見開きの見える端が gap に合うよう再配置する。
  - スライダー操作時は、対象の手動設定モードを適用する。
  - 見開き連動 / 左右別の切替時は値を移行し、左右別→連動では平均値にする。
  - 基本適用モード / 本全体設定は本キー、ページ個別設定値は page_path_key で
    `view_trim.db` に保存する。ページ個別チェック状態は保存せず、自動余白カットは
    モードだけ保存し、検出 bbox は保存しない。
  - 出力用 crop / 保存 / Ctrl+E / 補正 / AI キャッシュには影響しない。
- 残:
  - 実機で使用感確認後、枠ドラッグ操作を追加するか判断。
- 優先度: P3。

---

## 5. 入力カスタマイズ / マウス / ゲームパッド

### 5.1 Shift / Alt + ホイールのカスタマイズ再設計

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

---

## 6. リリース前確認 / 依存更新

### 6.1 ネイティブ依存

| 対象 | 現状 / 次の確認 | 注意点 |
| --- | --- | --- |
| PDFium | vendor 更新後の PDF 表示手動確認が必要 | PDF 開封、ページ列挙、サムネ、フルスクリーン、パスワード PDF |
| FFmpeg LGPL shared | 動画再生の手動確認と LGPL ソース tarball 配置更新 | DLL 名が変わる更新では `setup-ffmpeg.sh` / loader / `build.rs` を揃える |
| ONNX Runtime | `ort-sys` 要求 DLL と setup script の VERSION を確認 | C API バージョン一致、`+crt-static` + `load-dynamic` 維持 |
| VST3 SDK / bridge | C++ ソース変更がなければ再ビルド不要 | 更新時は商用プラグインで実機確認 |

### 6.2 Rust クレート

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

## 7. 着手時に読み直す関連ドキュメント

| 領域 | ドキュメント |
| --- | --- |
| UI 同期 I/O / worker 化 | `docs/ui-responsiveness.md`, `docs/async-architecture.md` |
| ZIP / PDF / 変換アーカイブ | `docs/virtual-folders.md`, `docs/shell-file-operations-context-menu-plan.md` |
| フォルダ移動 / Ctrl+↑↓ | `docs/fullscreen-navigation-consistency.md`, `docs/keymap-spec.md` |
| 入力カスタマイズ / マウス / ゲームパッド | `docs/keymap-spec.md`, `docs/key-customization-impl-plan.md`, `docs/ring-shortcut-plan.md` |
| フルスクリーン / F12 別ウィンドウ / 連結読み | `docs/display-pipeline.md`, `docs/detached-viewer-implementation-plan.md`, `docs/fullscreen-navigation-consistency.md` |
| 表示 / AI / 補正 | `docs/display-pipeline.md`, `docs/preset-and-adjustment.md` |
| 詳細表示 / スマートフィルタ | `docs/details-view-and-filter-plan.md`, `CLAUDE.md` の UI / スクロール節 |
| タグ / フルスクリーン右パネル / 動画 overlay | `docs/tag-catalog-redesign-plan.md`, `docs/display-pipeline.md`, `docs/video-architecture.md`, `docs/detached-viewer-implementation-plan.md` |
| リリース / 依存更新 | `CLAUDE.md` のリリース手順、各 native 依存管理節 |

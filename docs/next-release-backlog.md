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

### 1.1 F12 別ウィンドウ中のソート変更で動画セッションが一度閉じる

- 背景: F12 の別ウィンドウで動画を表示中にメイン一覧側でソート順を変更すると、
  元の動画アイテムがソート後の一覧に残っていても動画ウィンドウが一度閉じる。
- 現状: ソート変更時の通常フォルダ再読み込みが `start_loading_items` 経由で走り、
  共通処理の `close_fullscreen()` により detached viewer session / native video presenter が
  破棄される。後段の detached viewer 同期は、session が閉じた後なので復帰できない。
- 方針:
  - ソート / フィルタ後の同一フォルダ再構築では、変更前の表示対象を idx ではなく
    `metadata_cache_key` / `ViewerSyncStamp` 相当の安定キーで保持する。
  - 再構築後の `items` から同じキーを探し、見つかれば detached session を閉じずに
    `fullscreen_idx` / `selected` / 同期 stamp を新 idx へ付け替える。
  - 見つからない場合、またはフォルダ移動・ZIP/PDF 仮想フォルダ遷移など同一対象維持が
    不明な場合は、既存どおり session close を許可する。
  - 静止画だけでなく detached 動画の native presenter / `fs_cache` / pending source swap への
    影響を確認し、通常 fullscreen とフォルダ移動を巻き込まないテストを先に用意する。
- 規模 / リスク: Medium / 中。修正方針は明確だが、`start_loading_items` / `close_fullscreen`
  周辺は fullscreen lifecycle、動画 presenter、index keyed cache、履歴復元にまたがるため
  リリース直前の小修正にはしない。
- 優先度: P3。UX 改善として次回以降に対応。v2.1.0 のリリースブロッカーにはしない。

### 1.2 画像専用の複数別ウィンドウ表示を検討

- 背景: mImageViewer 専用スレ 3 (総合スレ 864 への返信)。「画像表示ウィンドウの
  複数展開、別アーカイブの画像を同時表示ができない」という要望。
- 現状:
  - 既存の F12 別ウィンドウは、メイン一覧と同期する単一 viewer session として設計している。
  - `docs/detached-viewer-implementation-plan.md` でも、複数の独立ビューアウィンドウは
    初期設計の対象外としている。
  - 動画は native presenter / decoder / 音声 / overlay / focus 周りが単一 session 前提のため、
    複数化すると影響範囲が大きい。
- 方針:
  - 初期検討は静止画のみ。動画、動画フレーム再生、native presenter は対象外にする。
  - 目的は「別アーカイブや別フォルダの画像を見比べる」用途なので、既存の F12 同期ビューアとは
    別に、固定対象を持つ追加ビューアウィンドウを開ける形を検討する。
  - 対象候補は通常画像、ZIP 内画像、PDF ページ、変換済みアーカイブのキャッシュ画像。
    実装時は virtual item の stable key と元コンテナ情報を保持し、一覧のソート / フィルタ /
    フォルダ移動で idx が変わってもウィンドウが別画像へ化けないようにする。
  - 初期版は閲覧、ズーム、フィット、ページ送りなど必要最小限に絞る。補正編集、右パネル、
    ゲームパッド操作、スライドショー、動画、複数 window 間の同期は後回し。
  - 既存の F12 別ウィンドウの「メイン一覧カーソルへ追従する」挙動は維持し、追加ウィンドウは
    それとは別の明示オープン機能として扱う。
  - 各ウィンドウごとに current item key、container key、page index、zoom / pan / fit、
    texture / cache 参照を持つ必要がある。`fullscreen_idx` など singleton 状態へ直接ぶら下げない。
- 確認:
  - A.zip のページと B.zip のページを別ウィンドウで同時表示し、それぞれ独立して閉じられる。
  - メイン一覧の移動、ソート、フォルダ移動、F12 別ウィンドウの開閉で追加ウィンドウが誤って閉じたり、
    別画像へ切り替わったりしない。
  - 元コンテナが消えた / 変換キャッシュが失効した場合のエラー表示と close cleanup を確認する。
- 優先度: P3。需要はあるが設計変更が大きめなので、キー / マウスカスタマイズの後で検討する。

## 2. フォルダツリーペイン

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

### 3.3 表示トリム / 余白カット

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

### 4.2 コマンド設定画面 + キーカスタマイズ GUI

- 背景: `keymap.ini` / `KeyAction` / `Chord` / exact modifier match の土台はあるが、
  現状は手書き ini 前提で GUI がない。競合検出は起動時 warning として実装済みだが、
  設定画面上で競合先を確認・編集・解除する UI はまだ無い。次バージョンでは、まず
  キーボード割り当ての設定画面を用意し、マウス/ゲームパッドの完全カスタマイズとは
  段階を分けつつ、NeeView の「コマンドに入力を割り当てる」構成に寄せたい。
- 方針:
  - 初期版はキーボード中心。マウス右ドラッグは「未使用 / リングショートカット / ジェスチャ」
    の選択へ整理し、リングショートカット設定も将来的にコマンド画面へ統合する。
    ゲームパッドもコマンドへ割り当てられる範囲を段階的に広げる。
    - `KeyAction` と既存 parser / default chord 定義を正本にし、旧 `keymap.ini` は初回起動時に
      `Settings.keymap` へ一度だけ取り込んで `keymap.ini.imported*.bak` へ退避する。以後の GUI は
      settings.db 側を書き換える。既存ユーザーの手書き設定ファイルはバックアップとして残す。
  - 修飾キーは現行どおり `ModifierHold` の中から選ぶ仕組みを維持する。一般キー hold と
    modifier hold を混ぜない。
  - 競合検出は `(context, trigger kind, chord)` を基本にし、同時に有効になり得る context の
    重なりだけ警告する。Press と Hold は完全同一視せず、同じ物理キーで誤爆しそうな場合は
    警告として扱う。
  - 競合時は保存禁止より、NeeView 風に「この割り当てを使うなら既存のどれを外すか」を
    選べる解消 UI を優先する。競合先の各コマンド設定を簡単に開けるようにし、競合先を
    別キーへ変更したり、割り当て解除したりできる形にする。
  - 既定値へ戻す、割り当て削除、最大3 chord、未対応/固定操作の説明、手書き ini の読み込みエラー表示を
    初期UIに含める。
- 優先度: P2 candidate。入力カスタマイズの需要に対して、既存土台を活かしやすい初期スライス。

### 4.3 リング / マウスジェスチャ / マウスボタンの追加候補

- 背景: mImageViewer 専用スレ 11。v2.2.0 の操作カスタマイズを試した上での追加要望。
- 要望:
  - リングショートカット / マウスジェスチャの割り当て候補に「閉じる」と「最小化」が欲しい。
  - 画像フルスクリーンの「見開き 1 ページずらし」をマウスジェスチャにも割り当てたい。
  - マウス戻る / 進むボタン設定と同じ場所で、ホイールクリック (中ボタンクリック) も設定したい。
    用途例として、中ボタンに Z ズームを割り当てたい。
- 方針:
  - 「閉じる」は文脈ごとに意味を分ける。初期案は、画像フルスクリーンでは `FsClose`、
    動画フルスクリーンでは `VideoCloseFullscreen` 相当。グリッドでのアプリ終了 / ウィンドウ close は
    誤操作リスクが高いので、入れる場合は別アクション名・確認要否・最小化との違いを先に決める。
  - 「最小化」はメインウィンドウ / detached viewer / native video のどれを最小化するかを整理する。
    初期案は「現在操作中の mIV ウィンドウを最小化」。F12 別ウィンドウや native video overlay 中の
    focus / owner / 復帰挙動を確認してから公開候補に入れる。
  - 見開き 1 ページずらしは既存 `KeyAction::FsSpreadShiftLeft` /
    `FsSpreadShiftRight` を流用し、`RingActionId` に ImageFS 専用の左右 2 アクションを足す。
    これによりリングショートカットとマウスジェスチャの候補へ同時に出せる。
  - 中ボタンクリックは `MouseButtonProfile` を `back / forward / middle` に拡張する方向で検討する。
    UI では「マウスボタン」タブに「ホイールクリック」を追加し、Grid / ImageFS / VideoFS で
    文脈別に設定できるようにする。
  - 既存の中ボタンドラッグズーム (`handle_middle_drag_zoom`) は維持する。中ボタン押下後、
    移動量がドラッグしきい値を超えた場合は従来どおりドラッグズームを優先し、移動が小さいまま
    release された時だけ「ホイールクリック」の割り当てを発火させる。
  - Z ズーム (`FsZoomMode`) は `KeyHold` 操作で、単発 `RingActionId` と同じ発火モデルではない。
    中ボタンへ割り当てる場合は、press で hold 開始、release で hold 終了を流す専用の
    mouse-hold アクション扱いにする。単なるトグルや一発アクションへ丸めない。
- 確認:
  - 中ボタンクリックに Z ズームを割り当て、押下中にズーム範囲指定、release で拡大表示に入れる。
  - 中ボタン押し込み + 上下ドラッグの従来ズーム、パネル上で開始した中ボタン無視の挙動が退行しない。
  - 右ドラッグのリング / マウスジェスチャで閉じる、最小化、見開き 1 ページずらしが候補に出る。
  - detached viewer / native video / 通常フルスクリーンで、閉じる・最小化後の focus と復帰が破綻しない。
- 優先度: P2 candidate。v2.2.0 の操作カスタマイズ拡張として相性がよいが、
  中ボタンは既存ドラッグ操作との競合があるため小さく確認しながら進める。

---

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
| 入力カスタマイズ / マウス / ゲームパッド | `docs/keymap-spec.md`, `docs/key-customization-impl-plan.md`, `docs/ring-shortcut-plan.md` |
| フルスクリーン / F12 別ウィンドウ / 連結読み | `docs/display-pipeline.md`, `docs/detached-viewer-implementation-plan.md`, `docs/fullscreen-navigation-consistency.md` |
| 表示 / AI / 補正 | `docs/display-pipeline.md`, `docs/preset-and-adjustment.md` |
| 詳細表示 / スマートフィルタ | `docs/details-view-and-filter-plan.md`, `CLAUDE.md` の UI / スクロール節 |
| タグ / フルスクリーン右パネル / 動画 overlay | `docs/tag-catalog-redesign-plan.md`, `docs/display-pipeline.md`, `docs/video-architecture.md`, `docs/detached-viewer-implementation-plan.md` |
| リリース / 依存更新 | `CLAUDE.md` のリリース手順、各 native 依存管理節 |

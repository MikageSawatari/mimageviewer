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

### 1.3 F11 仮想フルスクリーン中に Ctrl+↓ すると最大化ウィンドウに化ける

- 背景: 別ウィンドウ (detached viewer) を F11 で仮想フルスクリーン (装飾なし・モニタ全面の
  borderless) にした状態で Ctrl+↓ (フォルダナビ) すると、仮想フルスクリーンが解除され
  「最大化した通常ウィンドウ」になってしまう。**v2.2.0 でも再現する既存バグ**で、複数別
  ウィンドウ作業による退行ではない (2026-06-28 ユーザー報告)。
- 現状 (要調査):
  - F11 は `detached_viewer_borderless_fullscreen=true` + 復帰用 `detached_viewer_restore_placement`
    を立て、`build_detached_viewer_viewport_builder` が borderless 時は
    `detached_viewer_borderless_target_rect()` (decorations=false + モニタ全面)、非 borderless 時は
    通常 placement + `with_maximized(placement.maximized)` を使う。
  - `close_fullscreen_for_folder_nav_reopen` は `detached_viewer_borderless_fullscreen` /
    `detached_viewer_restore_placement` を preserve するが、folder-nav reopen 経路
    (`detached_viewer_folder_nav_reuse_window_once` → open_fullscreen が prepare/placement 再適用を
    skip) で borderless geometry が再適用されず、装飾あり + maximized の状態へ落ちている疑い。
- 方針:
  - folder-nav reopen 時に borderless 状態の detached window では、builder が borderless geometry
    (decorations=false + target rect) を再適用するようにする。通常 placement / `with_maximized` へ
    フォールバックさせない。
  - F11 トグル (`detached_viewer_borderless_fullscreen` の set/解除と restore placement) と、
    folder-nav / 別ファイル open 経路の builder 選択 (apply_placement / borderless 分岐) を突き合わせ、
    どのフレームで borderless が失われるか perf/debug ログで特定してから直す。
- 確認:
  - F11 仮想フルスクリーン中に Ctrl+↑↓ / 次ファイル移動しても borderless のままで、最大化や
    装飾ありに化けない。F11 解除時は元の通常ウィンドウ配置へ戻る。
- 優先度: P3。既存バグだが UX 影響は限定的。detached viewport ライフサイクル整理
  (`docs/detached-viewer-lifecycle-redesign-proposal.md`) と一緒に着手すると効率的。

### 1.4 UI スケール設定 (表示倍率) — メニュー「設定 > スケーリング」

- 背景: 高 DPI モニタで UI 文字が小さいという不満への対策。egui 既定のキーボードズーム
  (Ctrl+±) が実は効く (`Options::zoom_with_keyboard`) ことにユーザーが気付いたのが発端。
  これを隠し機能のままにせず、正式な設定として 50%〜200% を選べるようにする。
- 正本: **`docs/ui-scale-plan.md`** (Codex レビュー済み、実装可能粒度)。以下は要約。
- 確定仕様:
  - **範囲/刻み**: 50%〜200% を 10% 刻み (16 段)。
  - **UI**: メニュー「設定 > スケーリング」サブメニューから選択 (現在値にチェック)。
  - **単一の真実源 = メイン egui `ctx.zoom_factor()`**。`settings.ui_scale_factor` に永続化し、
    起動時に `set_zoom_factor` で適用。キーボードズーム (Ctrl+±) は初回は**無効化**
    (メイン + presenter overlay の両 Context で `zoom_with_keyboard=false`)。
  - **VST プラグイン GUI 画面のみ非スケール** (別 HWND、bridge 所有で拡縮不可・利用者少で許容)。
    それ以外 — メイン窓 UI、静止画フルスクリーン、**動画 overlay の通常 panel (タグ設定 /
    チャプター・ブックマーク一覧 / HUD)** — は全て反映する。
- 実装の要点 (詳細は正本):
  - 動画 overlay は native presenter の**別 egui Context**が描く。presenter の単一 ppp 源
    `self.pixels_per_point` に `(dpi/96) × ui_scale` を注入すれば overlay 全体が自己整合してスケール。
    伝搬は `NativeVideoOutput` に `cur_ui_scale` を持ち presenter 再生成時に再適用 (detached も同経路)。
  - OS DPI を直読みする箇所 (`native_video.rs` の VST overlap ~1407 / tile size ~4533、
    `video/mod.rs:2859` の WM_DPICHANGED、`mod.rs:7858` 初期 DPI) を `× ui_scale` に統一。
  - **50% (100% 未満) 対応**: presenter の `max(1.0)` 床 4 箇所 (`compute_hud_regions` mod.rs:4971 /
    cursor polling 2834 / IME 7734 / ring guide overlay_draw.rs:1962) を生 ppp に直す。
    **ppp ≥ 1.0 では no-op** なので既存 (100%以上) 挙動への回帰リスクほぼゼロ。
  - サムネ解像度はスケール連動のまま (2048px cap で許容)。未リリース機能につき settings migration 不要。
- 規模 / リスク: Medium / 低〜中。コード変更自体は小さく回帰リスクは低いが、動画 HUD が
  歴史的に finicky なため **50% での動画 HUD 実機クリック確認**が要る (正本 §5.1 にチェックリスト:
  軽い確認 + リング/タイル/小アイコン/長文ダイアログ/IME/VST overlap を各 1 回)。VST overlap のみ
  実プラグイン確認。多モニタ DPI 全網羅は不要 (DPI 変更パス流用)。
- 影響なし確認済み: VST3 音声処理・キャプチャ・スナップショットテスト・detached 窓配置判定。
- 優先度: P2 candidate。設計・Codex レビュー済みで着手しやすい。動画まわりに触るため、
  操作カスタマイズ系 (4.x) と時期が重なる場合は presenter への同時変更に注意。

### 1.5 RAR 直読み + 明示 ZIP 変換 + 同名 ZIP 優先表示

- 正本: **`docs/rar-direct-read-plan.md`**（設計・実装順・触るファイル・テスト項目まで記載）。以下は要約。
- 背景: RAR を開くたびに永続 cache ZIP を作る現状は disk 累積・二重化を生む。一般的な RAR
  （非ソリッド・入れ子なし・画像のみ）は ZIP 同様ランダムアクセス可能なので変換せず直読みできる。
  ソリッド / 入れ子 RAR の直読みは materialize / 一時展開 / 順次不変条件が必要で不安定化しやすいため対象外にする。
- 確定方針（4 点）:
  1. **RAR 直読み**: 非ソリッド かつ 入れ子アーカイブを含まない `.rar` / `.cbr` のみ直読み（cache 生成なし）。
  2. **フォールバック**: それ以外（ソリッド RAR / 入れ子 RAR / 7z / LZH）は従来通り ZIP cache 変換で開く。
  3. **明示変換メニュー**「変換 > ZIP ファイルに変換」: RAR/7z/LZH から**同じフォルダに同名 `.zip`** を生成
     （cache ではなくユーザー所有の実ファイル。既存 `archive_converter::convert_to_zip` 流用）。
  4. **同名 ZIP 優先表示**（既定 ON）: 同 basename の `.zip` と RAR/7z/LZH が並ぶとき `.zip` だけ表示。
     既存「同名ファイル処理」設定群（`skip_zip_if_folder_exists` 等の隣）に追加。
- 実装戦略: **道B**（`GridItem::ZipImage` を `zip_path=.rar` で再利用し、フォーマット分岐を `zip_loader` の末端 read
  関数だけに閉じ込める）。新 `RarImage` variant を全分岐に足す道A（`ZipImage` 303 / `zip_path` 660 箇所へ波及、
  ~1 万行）は採らない。直読み判定は `open_for_listing().is_solid()` + `nested_archive_kind` 走査で worker 内 1 回 list。
- 主リスク（局在）: `is_virtual_folder(current_folder)` 系 ~5–10 箇所を「直読み `.rar` を開いている状態」も真と扱う
  対応（BS / 親 / 退出ルーティング / `last_folder` 保存）。`zip_loader` dispatch の網羅性。DB キーは既存 `rar_path::entry`
  のままで変換 cache とも parity（回転 / ★ / タグ / 補正が壊れない）。
- 規模 / リスク: Medium / 低〜中。道B なら概ね 1,000–2,500 行、`rar_loader` / `zip_loader` / open routing /
  同名 dedup に集中。難ケースは枯れた convert に委譲するので materialize / LRU 系の不安定さは持ち込まない。
- 段階実装: ① フラット RAR 1 本を直読みで開く spike → ② 判定 + routing → ③ container 述語網羅 + キー parity テスト
  → ④ 同名 ZIP 優先表示 → ⑤ 変換メニュー → ⑥ docs（`virtual-folders.md` の分岐表に RAR 直読み行を追記）。
- 優先度: P2 candidate。着手前に `docs/virtual-folders.md`（分岐表・キー規則）と本正本を読む。

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
  - 「閉じる」は文脈ごとに意味を分ける。画像フルスクリーンでは `FsClose`、
    動画フルスクリーンでは `VideoCloseFullscreen` 相当として実装済み。グリッドでのアプリ終了 /
    ウィンドウ close は誤操作リスクが高いので、入れる場合は別アクション名・確認要否・最小化との違いを先に決める。
  - 「最小化」は `RingActionId::MinimizeWindow` として実装済み。グリッドはメインウィンドウ、
    画像フルスクリーンは現在のフルスクリーン / detached viewer、動画フルスクリーンは native presenter を
    直接最小化せず、実際のトップレベル host ウィンドウを最小化する。動画再生は最小化後も継続する。
  - 見開き 1 ページずらしは既存 `KeyAction::FsSpreadShiftLeft` /
    `FsSpreadShiftRight` 相当の左右 2 アクションに加え、PageUp/Down 系と同じ「前 / 次」の意味を
    RTL でも反転しない `KeyAction::FsSpreadShiftPrev` / `FsSpreadShiftNext` と
    `RingActionId::ImageSpreadShiftPrev` / `ImageSpreadShiftNext` を足す。
    これによりコマンド一覧、リングショートカット、マウスジェスチャの候補へ同時に出せる。
  - 中ボタンクリックは `MouseButtonProfile` を `back / forward / middle` に拡張し、
    UI では「マウスボタン」タブに「ホイールクリック」を追加済み。Grid / ImageFS / VideoFS で
    文脈別に設定でき、既定は未割り当て。
  - マウスボタン候補は Grid ではリングショートカットと同じ場所移動系も出すが、ImageFS / VideoFS では
    `C:\`〜`Z:\`、お気に入り、読書履歴、★一覧などの場所移動系を候補外にする。短クリックで
    フルスクリーン文脈を閉じたり別ビューへ移動したりする操作は誤発火時の影響が大きいため。
  - 既存の中ボタンドラッグズーム (`handle_middle_drag_zoom`) は維持する。中ボタン押下後、
    移動量がドラッグしきい値を超えた場合は従来どおりドラッグズームを優先し、移動が小さいまま
    release された時だけ「ホイールクリック」の割り当てを発火させる。
  - Z ズーム (`FsZoomMode`) は `KeyHold` 操作なので、単発 `RingActionId::ImageZoomMode` では
    押下中の照準表示をスキップし、現在のカーソル位置でズーム状態へ入る一発トグルとして扱う。
    ズーム状態中は中ボタン上下ドラッグで `fs_zoom_factor` を変更できる。
- 実装済み:
  - 画像 / 動画フルスクリーンの「閉じる」、ウィンドウ最小化、見開き 1 ページずらしの左右版・前後版、
    ホイールクリック、Z ズーム一発トグル、画像 / 動画フルスクリーンのマウスボタン候補整理。
- 残: なし。
- 確認:
  - 中ボタンへ Z ズームを割り当て、短クリックで全画面ズームモードへ入り、ズーム状態中の中ボタン上下ドラッグで倍率を変えられる。
  - 中ボタン押し込み + 上下ドラッグの従来ズーム、パネル上で開始した中ボタン無視の挙動が退行しない。
  - 右ドラッグのリング / マウスジェスチャで閉じる、見開き 1 ページずらしが候補に出る。
  - detached viewer / native video / 通常フルスクリーンで、閉じる後の focus と復帰が破綻しない。
  - グリッド / 画像フルスクリーン / detached viewer / native video で「ウィンドウ最小化」を実行でき、動画は最小化後も再生が続く。
- 優先度: P2 candidate。v2.2.0 の操作カスタマイズ拡張として相性がよいが、
  中ボタンは既存ドラッグ操作との競合があるため小さく確認しながら進める。

### 4.4 サムネイル画質設定の ZIP 内画像サンプル対応

- 背景: mImageViewer 専用スレ 12。「設定 → サムネイル画質設定」が ZIP 内画像選択時に
  「画像を1枚選択してからもう一度お試しください。」となり設定できないという報告。
- 現状:
  - `open_thumb_quality_dialog` は `last_selected_image_path: Option<PathBuf>` を使い、
    worker で `image::open(path)` して A/B サンプルを作る。
  - `update_last_selected_image` は `GridItem::Image` の実ファイルパスだけを保存し、
    `GridItem::ZipImage` / `PdfPage` は対象外。ZIP 内画像は実パスが無いため現状の構造では
    サンプルにできない。
- 方針:
  - `last_selected_image_path` ではなく、選択中 item からサンプル取得ジョブを作る構造へ変える。
    初期対応は `GridItem::Image` と `GridItem::ZipImage`。ZIP は `zip_loader::read_entry_bytes`
    で entry bytes を読み、`image::load_from_memory` でデコードする。
  - PDF ページは PDFium worker / render サイズの扱いが別になるため、同時対応するか別タスクにするか判断する。
    まずはメッセージを「ZIP/PDF内ページは未対応」ではなく実対応に寄せたい。
  - デコードは現在と同じく worker で行い、UI スレッドで ZIP 読み / 画像デコードをしない。
  - サンプル表示名は実パスだけでなく `book.zip > page.jpg` のような仮想表示名を持てるようにする。
- 確認:
  - 通常画像と ZIP 内画像でサムネイル画質 A/B ダイアログが開く。
  - 壊れた ZIP entry / 非対応画像 / ZIP が削除済みの場合は失敗メッセージを出し、UI は固まらない。
  - 画質適用後のキャッシュ再生成が通常画像 / ZIP 内画像で同じ設定を使う。
- 優先度: P2 candidate。報告としてはバグ寄りだが、PDF 対応まで含めると範囲が広がるため
  ZIP 内画像を先に小さく直す。

### 4.5 画像フルスクリーン下部シークバー上のホイールでページ前後移動

- 背景: mImageViewer 専用スレ 17。「画像ビューア下部のシークバー上でホイール前後で
  ページも前後に動くようにしてほしい」という要望。
- 現状:
  - 画像 / ZIP / PDF フルスクリーンでは通常の画像領域上ホイールで前後ページへ移動できる。
  - 下部シークバーやロック表示中の下部 UI 上では、egui の UI 領域が wheel を受けて
    既存のページ移動に流れないケースがあると考えられる。
- 方針:
  - 画像フルスクリーンの下部シークバー hit 領域上で縦ホイールを受けた場合は、
    既存のマウスホイールによる前後ページ移動と同じ経路へ流す。
  - シークバーのクリック / ドラッグによるページ位置変更は維持し、ドラッグ中の wheel は
    二重発火しないようにする。
  - 対象は静止画系フルスクリーン (通常画像 / ZIP 内画像 / PDF ページ / 変換アーカイブ由来ページ)。
    native 動画の seek bar は経路が別なので、このタスクでは触らない。
  - 右綴じ / 左綴じや連結読み中の方向解釈は、通常ホイールの既存挙動と揃える。
- 確認:
  - 下部シークバーをホバーした状態でホイール上下を行い、画像領域上と同じページ移動になる。
  - シークバーのクリック移動、ドラッグ移動、ページシークバー固定表示が退行しない。
  - 縦連結 / 横連結読み、見開き、PDF、ZIP 内画像で wheel が二重に処理されない。
- 優先度: P2 candidate。小さめの入力処理だが、連結読みとシークバー drag の競合を確認する。

### 4.6 サムネイル選択情報の下部 1 行バー / ツールチップ改善

- 背景: mImageViewer 専用スレ 17。サムネイル情報ツールチップが下の列のサムネイルに被って
  見えなくなるため、一定時間で消す、または ZipPla のようにウィンドウ下部に表示したいという要望。
  ツールチップを非表示にすると情報量が足りないため、情報表示自体は残したい。
- ZipPla 調査メモ:
  - ZipPla / ZipPlaFork は、上部のサムネイル領域と下部のファイルリスト / 詳細ビューを
    同時表示できる。スクリーンショット上の「下に別途出ているウィンドウ」はこの詳細ビューに近い。
  - さらに最下部には `statusStrip` があり、そこへ `selectedFileNameToolStripTextBox` を挿入している。
    選択中ファイル 1 件ではパス、画像解像度、ページ数、動画情報、ファイルサイズなどを下部に表示し、
    複数選択時は選択数を表示する。README 履歴でも、ステータスバーに対応ファイル数 /
    ファイル名またはフルパス / ファイルサイズ / ページ数を表示する変更が確認できる。
- 方針:
  - サムネイル選択情報の表示方式を設定化する。候補は `ツールチップ` (現状) /
    `下部情報バー` / `両方` / `非表示`。
  - 下部情報バーはメインウィンドウ下部の固定領域として表示し、サムネイルに重ならない形にする。
    初期案は「詳細表示の 1 行版」のように、選択中 item の主要情報を横一列で表示する。
  - 表示内容は既存の「サムネイルの選択情報ツールチップ」カスタマイズ項目
    (ファイル名、解像度、種類、サイズ、更新日時、作成日時、動画情報、場所など) を流用する。
    詳細表示の列設定と共有できるかは実装時に確認する。
  - 複数選択時は `N 個選択` を主表示にし、取得済みなら合計サイズなどを補助表示する。
    未取得メタデータのために UI スレッドで同期 I/O を増やさない。
  - ツールチップを継続する場合は、一定時間で自動的に消す / 画面下端では上側へ逃がす /
    下部情報バー有効時はツールチップを出さない、のいずれかを実装時に選ぶ。
  - サムネイル表示と詳細表示の両方で、選択カーソル変更に追従する。
  - ZipPla の上下分割に近い「サムネイル一覧 + 詳細リストの同時表示」も要望意図に含まれる可能性がある。
    ただし、mIV では既にサムネイル表示 / 詳細表示を切り替える設計なので、初期対応は軽量な
    下部情報バーに留め、同時表示ビューは別要望として切り分ける。
- 確認:
  - 最下段付近のサムネイルを選択しても、情報表示で下の行が隠れない。
  - 下部情報バーはサムネイル領域を大きく圧迫せず、詳細表示 1 行相当の情報量を読める。
  - ツールチップ非表示設定でも、下部情報バーから必要情報を確認できる。
  - 大量フォルダ / ZIP / PDF / 動画混在で、情報更新が UI のスクロールや選択移動をブロックしない。
- 優先度: P2 candidate。要望内容は明確で、既存のツールチップ情報生成を流用できる見込み。

### 4.7 ZIP/PDF/対応アーカイブの明示オープンコマンド

- 背景: mImageViewer 専用スレ 17。全体設定の
  `ZIP/PDF/対応アーカイブ → 開いたとき、ページ一覧を表示 / ページをフルスクリーン表示`
  はどちらにも利点があるため、右クリックメニュー、リングショートカット、マウスジェスチャなどから
  その場で開き方を選びたいという要望。
- 方針:
  - 既存の `開く` / `GridOpenSelected` は残し、現在の全体設定に従う既定動作として扱う。
    ユーザー向けには `通常の開く` と説明できる。
  - 追加で、設定を一時的に上書きする明示コマンドを用意し、
    `通常の開く` / `ページを開く` / `一覧を開く` の 3 種類から選べる状態にする。
    - `ページを開く`: ZIP/PDF/対応アーカイブはページ一覧を経由せず、続きまたは先頭ページを
      フルスクリーン表示する。
    - `一覧を開く`: ZIP/PDF/対応アーカイブを仮想フォルダのページ一覧として開く。
  - 実装上はグローバル設定を書き換えず、open 要求に
    `OpenContainerMode::{Default, PageFullscreen, PageList}` のような一時モードを持たせる。
  - 右クリックメニューではコンテナ項目 (ZIP/CBZ/PDF/RAR/7z/LZH 等) に
    `開く` / `ページを開く` / `一覧を開く` を並べる。通常画像 / 動画 / 通常フォルダでは
    既存の `開く` を優先し、明示コマンドを出すか no-op にするかは実装時に整理する。
  - 操作カスタマイズでは `GridOpenSelectedAsPage` / `GridOpenSelectedAsList` 相当を
    キーボード、リングショートカット、マウスジェスチャ候補へ追加する。
    既定キーを割り当てる場合は、既存の `Shift+Enter` (外部プレイヤー起動) と衝突しないようにする。
  - 起動引数 / SendTo / 関連付けの挙動は、当面は全体設定に従う既存動作のままとする。
- 確認:
  - 全体設定が「ページ一覧」でも `ページを開く` ではフルスクリーンへ入る。
  - 全体設定が「フルスクリーン」でも `一覧を開く` ではページ一覧へ入る。
  - 変換済み / 未変換 RAR・7z・LZH、ZIP、CBZ、PDF で同じモード指定が効く。
  - 既存の Enter / ダブルクリック / `開く`、外部プレイヤー起動、通常画像の open が退行しない。
- 優先度: P2 candidate。ユーザーの運用差を全体設定だけで吸収しないための入力カスタマイズ拡張。

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

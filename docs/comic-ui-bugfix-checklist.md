# comic テキスト注釈 — 実機検証で出たバグ修正チェックリスト（2026-06-06）

2 層構造化 + 形状/しっぽ移植 + 編集パック公開後の実機検証（ユーザー）で発見された不具合・
要望の一覧。優先度順に上から潰す。各項目: 状態 / 症状 / 原因(調査後) / 対応コミット。

凡例: ☐ 未着手 / 🔍 調査中 / ✅ 修正済 (commit) / 💬 要相談

---

## P0（基本操作が壊れている / 主要機能が無反応）

### ✅ B1. 一覧の操作ボタン（削除/複製/↑/↓）が機能しない (`db3396ca`)
- 原因: §6-5 で追加ボタンを全幅縦 5 本にしてパネルが縦に伸び、操作行が sink_rect（=
  handle_text_canvas_input のパネル矩形）の下へはみ出し → 操作行クリックを「パネル外＝
  キャンバス操作」と誤判定し選択解除されて操作無効化。
- 修正: 一覧 ScrollArea をカーソル→sink_rect 下端で上限算出 + auto_shrink([false,true])、
  操作行を直下に。操作行が必ず sink_rect 内に収まる。B7 と同時解消。

### ✅ B2. 編集パック / オノマトペの DL ボタンが無反応 (`db3396ca`)
- 原因: show_editing_addon_dialog がメイン update でのみ呼ばれ、フルスクリーンビューポートで
  描画されず（VST3 と同問題）、ダイアログが背後に隠れていた。
- 修正: メイン update を fullscreen 中スキップに gate + フルスクリーンビューポートで描画。

### ✅ B3. ウィンドウ「枠」タブで右パネル幅が変わり、セリフに戻れない (`db3396ca`)
- 原因: window_body_ui の多ラジオ行（種類/位置/サイズ等）が ui.horizontal で PANEL_W(268) 超過
  → Area が広がり detail_rect 超え → セリフタブが「パネル外」判定で切替不可。
- 修正: 多ラジオ行を horizontal_wrapped にして PANEL_W 内で折り返す。

---

## P1（機能不全 / 入力・表示の不具合）

### 💬 B4. 「下の吹き出しと結合」が効かない（要確認）
- 調査: comic-core のベイク経路 (bake_overlay_with_stamps → merge グルーピング →
  bake_merge_group の 4 パス union) は健在。mIV のベイクもこの経路を通る。トグルは
  b.merge_with_below + changed を立て再ベイクされる。
- 仮説: 融合には ①2 つの吹き出しが z 順で隣接 ②両方が mergeable 形状 (楕円等。ぼかし/線/
  なしは不可) ③**重なっている** ④**前面(上)の吹き出し**に「結合」を付ける、が必要。
  非重なり / 背面に設定 だと見た目が変わらない。→ ユーザーにテスト条件を確認中。

### ✅ B5. セリフ本文の IME 確定 Enter で改行が入る (`f8ff6391`)
- 原因: multiline TextEdit で IME 確定 Enter が「確定(Ime::Commit)+改行(Key::Enter)」の二重処理。
- 修正: text_mode かつ IME アクティブ (変換中 or 直近 300ms) の間 raw Key::Enter を除去。
  確定は Ime::Commit が行う。非変換時の Enter は通る。

### ✅ B6. 追加ダイアログのタイトル文字色・×ボタンが見づらい (`f8ff6391`)
- 原因: 暗フレーム強制だが題名/× はビューポート ctx visuals (OS テーマ連動でライト) で描画。
- 修正: 追加ダイアログ描画の間だけ ctx visuals をダークへ上書き→復元。

---

## P2（使い勝手 / 設計変更要望）

### ☐ B7. 一覧の操作行をリスト直下に置き、溢れ時のみスクロール
- 要望: ↑↓複製削除 の操作行が body 最下部に離れて使いづらい。ラボのように一覧の直下に付け、
  左パネルから溢れるときだけ一覧＋操作行をスクロールさせる。
- 関連: `draw_text_panel` の allocate_ui / ScrollArea レイアウト再構成。
- task #12

### 💬 B8. システムフォントへのフォールバックを廃止
- 要望: 既定でシステムフォント表示になるフォールバックを無くす。フォントが使えない場合は追加
  させない（後からフォント追加で保存済みの見た目が変わるのを防ぐ）。
- 検討点: 既定フォント（COMIC_FONT_KEY）の扱い、フォント未ロード時の挙動、既存ドキュメントとの
  互換（未リリースなので破壊的変更可）。方針をユーザーと確認してから着手。
- task #13

---

## 追加発見（2 回目の実機 FB + Codex 監査 2026-06-07）

### ✅ B9. 左右パネル上のホイールで画像がズーム (`e154a963`)
- 原因: handle_fs_wheel_and_click の cursor_in_panel がテキストモードの左右パネルを未判定。
- 修正: text mode は image_rect==full_rect なので text_panel_rect(full_rect)/
  text_detail_panel_rect(full_rect) を cursor_in_panel に追加。パネル上ホイールは一覧スクロールへ。

### ✅ C1. 保存/コピー/比較に注釈(comic)が焼かれない (`98da4e7b`) [Codex P0]
- prepare_capture_pixel_job を comic_composited_pixels_for_export→素 composite fallback に。
  Ctrl+E と同経路で「表示どおり保存/コピー/比較」。

### ✅ C2. IME 確定 Enter (Codex も指摘) — B5 で対応済み (`f8ff6391`)
- ラボの consume_ime_enter 相当を、本体はフルスクリーン closure 側 (viewport level) で実装。

### ✅ C8. DL ダイアログ表示中のフルスクリーンキー漏れ (`98da4e7b`) [自Codex P2]
- editing_addon_install_state を any_modal_dialog_open_for_fullscreen_keys / any_dialog_open に追加。

### ✅ C3. ユーザー画像スタンプの持ち運び (`354b4677`) [Codex P1]
- StampSource::Embedded { name, data(base64 PNG) } を追加。ファイル選択時に長辺 1024px へ縮小→
  PNG→base64 で注釈に埋め込み (embed_file_stamp)。load_stamp_image は base64 デコードで復元
  (fs アクセスなし)。フォルダ移動/別 PC/元削除でも欠落しない。MRU には積まない (肥大化回避)。
  既存 File も後方互換。未リリースなので移行不要。

### ✅ C4. 注釈付きページのグリッドバッジ (`973b15dc`) [Codex P1]
- comic_db::load_comic_keys を接続。comic_pages (item_idx 集合) を mask_pages と同形で
  フォルダロード/rehydrate 時に構築、draw_cell に has_comic を渡してピンク系「文」バッジ描画。
  save_comic_objects で idx 単位に即時メンテ、remove_items_batch shift / clear_page_edit_state も対応。

### ✗ C5. 強縮小書き出しでテキストが甘い — **実装しない判断** (2026-06-07) [Codex P1 / 設計 D10]
- 設計 D10 は「ダウンサンプル後に最終解像度で焼く」、実装は「ダウンサンプル前 (base 解像度) で焼く」。
- 正しく直すには comic を base に焼かず worker で crop→縮小**後**に焼く再構成が要り、source→base
  (AI 倍率) →crop オフセット→最終縮小 と座標が多段で掛かる (scale_scene は scale のみで translate
  不可)。**座標ズレの視覚バグが crop×強縮小×注釈の隅でしか出ず気づきにくい**。
- **ユーザー判断: 最終出力の縮小は重視機能でなく、複雑さ/リスクに見合わないため実装しない。**

### ✅ B8. フォントのフォールバック (`f03f5f6e`) [Codex P2]
- ユーザー判断: 吹き出し/メッセージウィンドウの規定は Yu Gothic のまま (Windows 常在で安定、
  proprietary のため同梱不可)。オノマトペは OFL フォント前提なので、パック未導入時は
  **追加自体をブロック** (カード無効化 + chosen ガード)。システム既定へのフォールバックで
  追加して後から見た目が変わる事故を防ぐ。フォント選択ダイアログの「追加パック」タブは
  未導入時に空 = 吹き出し/窓でのパックフォント選択も既にブロック済み。

### ✅ F2. AI/編集先読みの GPU upload を表示時まで遅延 (`0d10993f`) [他セッションP1]
- prefetch_final_ai が非表示近隣ページの edit_result テクスチャを UI スレッドで前倒し upload して
  いた退行 (Pipeline P1 リファクタで旧 AI ゲート喪失)。CPU 専用 ensure_edit_result_pixels_cpu を
  追加、EditResultEntry.texture を Option 化、表示時に lazy upload。AI オン/オフ両方で先読み中の
  GPU 負担が消える。近隣 AI 推論はワーカーで先読み継続。
- Codex レビュー: **P1 なし** (lazy upload・表示経路・borrow・Option 処理すべて正しい)。
- 残 P2: 編集済み (conceal/局所補正/消しゴム) 近隣ページは prefetch 中まだ各自の layer テクスチャを
  upload する (layer cache 由来の既存挙動)。edit_result の universal upload は解消済み。完全 CPU 化
  には conceal/erase/local の CPU 専用版分離が要る (別途・中規模)。

### ✗ (c) AI Display preempt — **実装しない判断** (2026-06-07) [他セッションP1/P2]
- queue は Display 優先だが worker は 1 件を完了まで実行。重い prefetch 推論中にページ移動すると
  表示ページの AI が待たされる。
- ただし: ①遅延は**高々 1 件**(prefetch は has_uncancelled_final_ai_pending で同時 1 ジョブに gate)、
  ②AI 計算中も**フル解像度の非 AI 画像が即表示**され (ensure_final_composite_texture が AI pending
  時は adjusted を complete=false で表示、結果到着で差し替え)、サムネ/空白待ちにはならない。
- F2 で UI スレッドの詰まりは解消済み。残るワーカー側の 1 ジョブ遅延は「フル画像は出ていて AI
  シャープ化が一拍遅れる」だけで非ジャーリング。**preempt/cancel は並行整合バグのリスクが高く、
  得られる体感改善が小さいため実装しない** (重いフォルダで一拍重いのは許容)。

### ℹ️ C7. UI スレッド同期 I/O (font 列挙/読込・stamp file read) [Codex P2]
- 通常閲覧では回避設計だが、注釈付き画像/大スタンプで一瞬固まりうる。worker 化は要検討 (低優先)。

### ✅ C6 / B4. 結合の z 隣接条件 — **文言で対応** (`c5781288`) [Codex P2]
- 結合は z 順で「すぐ下 (直下)」の吹き出しとのみ融合する仕様。間に別オブジェクトが挟まる吹き出し
  まで飛ばすと z 重なり順が壊れる (comic-core merge グルーピングは連続 z のみ 1 ユニット化) ため、
  直下のみは**意図的**。z 探索拡張は z 順バグリスクのため見送り。
- **ユーザー判断: ロジックは変えず、ラベルを「下の吹き出しと結合」→「すぐ下の吹き出しと結合」に
  して直下条件を伝える文言対応とする。** コメントにも直下のみが意図的である旨を明記。

## 他セッションレビュー 第2ラウンド (2026-06-07)

### ✅ R2-1. comic_lab ビルド不能 (`5f6d5367`) [P0]
- StampSource::Embedded 追加に対し tools/comic_lab/src/stamp.rs の 3 match (key/label/decode) が
  未カバーで E0004。本体と同等の embedded 分岐 + embedded_data_key を移植 (base64 は既存依存)。

### ✅ R2-2. 比較準備キャッシュの注釈無効化漏れ (`5f6d5367`) [P1]
- prepare_capture_pixel_job が comic を焼くようになり注釈も比較入力。compare_prepared_pair_matches
  は idx だけで一致判定するので、注釈編集で旧ピクセルが残る。mark_comic_dirty (live・現在ページ) と
  save_comic_objects (永続化・idx 指定) の両方で invalidate_compare_prepared_for_idx を呼ぶ。

### ✅ R2-3. D10 文書/コメントの矛盾 (`5f6d5367`) [P2]
- docs/comic-integration-plan.md D10 行 + src/comic_overlay.rs ヘッダを「不採用 (2026-06-07)」に更新。

### ✅ R2-4. 埋め込みスタンプ data の undo clone 肥大 (`a7e4e0ba`) [P2]
- StampSource::Embedded.data を String→Arc<str> 化。undo snapshot (objects.to_vec、cap=100) /
  comic_docs の clone が画像データを複製せず Arc ポインタだけになる。serde "rc" feature 有効化
  (JSON 形状不変 = comic.db 互換)。

### 💬 R2-5. AI prefetch の CPU 化が下位レイヤで破れる [P2、F2 残と同件]
- 編集済み (conceal/局所補正/消しゴム) 近隣ページは prefetch 中も各 layer texture を UI スレッドで
  upload (ensure_conceal_texture / local-adjust render 完了 upload)。edit_result の universal upload
  は F2 で解消済み。完全 CPU 化は conceal/erase/local の CPU 専用版分離が要る中規模。**要方針判断**。

### ✅ R2-6. ユーザー画像スタンプ追加が UI スレッドで重い (`8e9de7dc`) [P2]
- 実測 (release・写真ライク): 12MP≈115ms / 24MP(10MBクラス)≈195ms / 48MP≈375ms (embed_file_stamp_timing
  テスト)。JPEG DCT スケール decode も試したが 1024px ターゲットでは 1/4 止まり=エントロピー復号支配で
  無効果だったため revert。
- **ユーザー判断: 中央トースト付きで worker 化**。embed_file_stamp を背景スレッド (StampEmbedPending) へ
  逃がし、処理中は画面中央に「スタンプ読み込み中…」(draw_stamp_embed_overlay)、完了で apply_stamp_choice。
  UI は固まらず、サイズ上限不要。stale (ページ移動/モード終了/フォルダ変更) は cancel して破棄。

## 進め方
- P0 → P1 → P2 の順。各修正は pathspec commit（src/ui_text.rs 等）、退避ブランチ `comic-inc6` 維持。
- まとまった単位で Codex レビュー。
- B8 は設計変更なので、調査結果を添えて方針確認してから実装。

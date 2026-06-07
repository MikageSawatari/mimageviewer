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

### ✅ R2-7. 合成 (composite_overlay_over) の rayon 並列化 (`ff8a60d4`) [perf]
- ドラッグ中は毎フレーム再ベイクし、composite_overlay_over (下地全画素 src-over) が最大の CPU
  コスト (逐次)。各出力行は独立なので par_chunks_mut で行並列化。20MP・重オーバーレイ bench で
  72.6ms→29.3ms (2.5x、画素は逐次と一致)。表示初回/書き出し/ドラッグの全経路に効く。
- 限界: base clone と load_texture upload は並列対象外で残る。ドラッグ完全 60fps 化には別途
  低解像度プレビュー (B 案) が必要。
- **ユーザー判断 (2026-06-07): 自動の (B) 低解像度プレビューは保留** (勝手に解像度が変わるのが嫌)。

### ✅ R2-9. テキスト編集中のプレビュー解像度 (`4cb37263`) [perf / ユーザー要望]
- (B) を **ユーザー明示・α方式** で採用。Ctrl+T 左パネル上部に「プレビュー解像度: 原寸/1/2/1/4/1/8」。
  text モード中のみ full base を 1/N に一度だけ縮小 (comic_preview_base) して注釈ベイクの下地にし、
  合成 + GPU upload を 1/N² に。**ツール実行中ずっと固定**で途中変化なし、**閉じると原寸で焼き直し
  (鮮明化)**。保存/コピー/比較/Ctrl+E はフル解像度のまま (別経路)。設定は永続化。
- 実測の体感: 4k で原寸はもたつくが 1/4 で 1k 相当 = スムーズ。1k/2k は原寸でも快適。
- **マニュアル未記載** (comic の専用マニュアルページが未作成 = Inc 8 でテキスト注釈ページを書く際に
  プレビュー解像度も併記する)。

### ✅ R2-8. Codex R2 追随 — embed worker の clobber/lingering + font 変更時の比較 (`69b24bc7`)
- [P2] embed worker clobber: 進行中に別スタンプ操作すると古い結果が上書きしうる →
  mark_comic_dirty で進行中 worker を cancel (完了経路は poll が先に take 済みで no-op)。
- [P2] font 変更時の比較全無効化: パック導入/削除はフォントが全ページ影響だが mark_comic_dirty は
  現在ページのみ無効化 → invalidate_all_compare_prepared 新設で両側 (current+pinned) を落とす。
- [P3] embed worker lingering: フルスクリーン終了で poll が止まり stale guard が遅延 →
  reset_text_mode でも cancel。
- Codex R2 は **P1 なし**。Arc<str> 直列化互換・comic_lab match 追加も OK と確認済み。

### ✅ R2-10. Codex 追加レビュー対応 (`e0f222b2` / `4b503ae1`)
- [P1] ホバーバー分析ボタンが analysis_mode を直接反転し Z キー副作用 (zoom/pan 引き継ぎ・
  post_filter bypass enter/exit・補正排他) をバイパス → 「Z で ON → ボタンで OFF」で
  post_filter_bypassed が残る退行。`toggle_analysis_mode()` を新設し Z / ホバーボタン / 分析パネル
  × を全て合流 (spread 強制 OFF は handoff 不要で reset のまま)。
- [P2] X 比較ピン留めスロットが font パック導入/削除後も旧フォント焼き込みを保持 →
  invalidate_all_compare_prepared に pinned_compare_slot / compare_pin_pending /
  compare_pin_load_pending のクリアを追加。
  - 追加 (`0600cd6a`): スロットを消すだけだと compare_view_mode が PinnedNormal/Wipe/Diff のまま
    残り、PinnedNormal で pin texture None → 現在画像へフォールバックせず無描画フレームになる →
    `compare_view_mode = Off` も併せて行う (通常のピン解除と同じ後始末)。
- [P3] stale コメント: comic_composited_pixels_for_export の D10「将来リファイン」→「不採用」、
  embed_file_stamp_timing テストの「worker 化不要」→「worker 化済み」。
- 確認済み (前回指摘): AI 完了待ち統一・Z 分析 raw 表示・w==0 防御・replace 消失復元・
  stamp worker stale cancel・比較全無効化・D10 主要 docs。

### ✅ R2-11. Codex 追加レビュー対応 (`d943ccb2`)
- [P2] テキスト子ダイアログ (吹き出し/ウィンドウ/スタンプ/オノマトペ/フォント) が非モーダルで、
  ダイアログ上のクリック/ドラッグが handle_text_canvas_input に漏れて背面オブジェクトを選択/移動/
  削除し、フルスクリーンキー (Esc/Delete/矢印) も漏れていた。`text_subdialog_open()` を新設し、
  canvas 入力を gate + any_dialog_open / any_modal_dialog_open_for_fullscreen_keys に追加。
- [P3] text_preview_scale が環境設定 OK で巻き戻る (snapshot 全体差し替えに live 値が無い) →
  overwrite_non_preferences_from に text_preview_scale を追加。
- 確認済み: P1 分析ボタン経路統一・D10 コメント・ピン留め無効化・フル解像度 export。

### ✅ FB. 実機 FB: 自動サイズ過大 + しっぽ先端の潜り込み (`bdb07ab2`)
- 症状: セリフ自動サイズ ON で長文 (特に縦書き) だと吹き出しが過大に膨らみ左右余白が大きい。
  さらに本体が育つと固定距離の既定しっぽ先端が内部に潜り、内向きツノに反転して崩れる。
- 原因①: `tessellate::fit_bubble_shape` の `MAX_ASPECT=1.8` が縦書き長文 (例 アスペクト6:1) を
  丸い楕円へ強制 → 横方向に約2.7倍水増し。
- 原因②: `Tail.tip` は絶対座標で、auto_size の形状拡大は bake/幾何時に遅延計算され tip 更新
  イベントが無い。既定 tip は pivot から固定106px → 楕円が半径〜300pxに育つと内部に入る。
  `auto_base_t` の出口が tip より外になり三角形が内向きに反転。
- 対応: ① `MAX_ASPECT` を 4.5 へ緩和 (縦長で文字に沿わせる)。② `resolve_tail_tip`
  (内部 `project_tip_outside`) を新設 — 中心→tip の方向は保持し半径距離を「輪郭出口 +
  `TAIL_MIN_OVERHANG(28px)`」以上へ投影。`bubble_geometry` の Spike/Thought 双方で使用、
  `ui_text` / `comic_lab` の先端ハンドル・AABB も `resolve_tail_tip` 経由へ統一 (lab parity)。
  保存 `tail.tip` は素のまま (描画/当たり判定のみ補正、手動の内側ドラッグでも潜らない)。
- テスト: comic-core 97 passed (aspect 閾値更新 + `resolve_tail_tip_pushes_tip_outside` 追加)、
  本体 bin 2100 passed。設計メモ = docs/speech-bubble-text-tool-plan.md §3.2。

### ✅ FB2. 実機 FB: 注釈付き画像のオープンで8秒固まり (`8afd947c`, perf計装 `e18b412f`)
- 症状: テキスト効果付き注釈のある画像を開くと数秒〜「応答なし」。perf-log で
  `fs/comic_composite_build` が full解像度(3584x4608)で **bake_ms=7956 (約8秒)** と判明
  (編集中の1/4プレビューでは55-66ms)。下地 `fs/final_composite_build` は最大44msで無罪。
- 原因: `font::dilate_coverage` が総当たり円形ダイレート O(面積×半径²)。これを
  `draw_layout_soft_mask` が影/グローで最大8パス + アウトライン分、グリフ毎に再ラスタ+
  再ダイレート (キャッシュ無し) → 大判・大文字・大半径で爆発。閲覧は preview_scale=1
  固定なので開く度に同期実行。別セッション `1aeba973 Add text annotation effects` 由来。
- 対応 (方針A=高速化・同期維持): dilate_coverage を2パスのチャンファー距離変換に置換。
  半径非依存の O(出力面積) で coverage=clamp(dilate+0.5-dist) を生成 (内部不透明・境界1px
  AA)。種は旧挙動同様 coverage>0 全画素 (グリフ完全内包・細線維持)。D1=1/D2=√2。挙動不変。
- 計装追加 (`e18b412f`): `fs/final_composite_build` (edit/adjust/post/clamp/upload_ms) と
  `fs/comic_composite_build` (bake/composite/upload_ms) を cache miss 時のみ emit。
- 実測 (effect_bake_bench, 巨大180px縦+影+グロー+太縁 @3584x4608): release 245ms /
  debug 999ms (旧 ~8000ms)。約33倍。comic-core 100 passed。
- 残: さらに詰めるならグリフ素ラスタ/距離場をベイク内キャッシュ (パス間再計算の削減)。
  現状で「応答なし」は解消見込み。

## 進め方
- P0 → P1 → P2 の順。各修正は pathspec commit（src/ui_text.rs 等）、退避ブランチ `comic-inc6` 維持。
- まとまった単位で Codex レビュー。
- B8 は設計変更なので、調査結果を添えて方針確認してから実装。

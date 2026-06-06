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

### 💬 C4. 注釈付きページのグリッドバッジ未接続 (要相談) [Codex P1]
- comic_db::load_comic_keys は存在するが呼び出し無し。消しゴム mask_pages 相当の comic 版が未実装。
- 対応 = 起動/フォルダロード時に comic キー集合をロード→グリッド描画でバッジ。小〜中規模。

### 💬 C5. 強縮小書き出しでテキストが甘い (要相談) [Codex P1 / 設計 D10]
- 設計 D10 は「ダウンサンプル後に最終解像度で焼く」、実装は「ダウンサンプル前で焼く」。
- 対応 = export ベイク経路の再構成。中規模。画質改善。

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

### 💬 残: AI Display 優先が実行中 prefetch を preempt しない [他セッションP1/P2]
- queue は Display 優先だが、worker は 1 件を完了まで実行し、別 idx の prefetch を cancel しない。
  重い prefetch 推論中にページ移動すると表示ページの AI が待たされる。対応 = Display enqueue 時に
  running/queued prefetch へ cancel、または tile 間 cooperative preemption。AI スケジューラ領域・中〜大。

### ℹ️ C7. UI スレッド同期 I/O (font 列挙/読込・stamp file read) [Codex P2]
- 通常閲覧では回避設計だが、注釈付き画像/大スタンプで一瞬固まりうる。worker 化は要検討 (低優先)。

### ℹ️ B4/C9. 結合の z 隣接条件 (仕様) [Codex P2]
- ユーザー再テストで「重ねて前面に設定」すれば結合 OK と確認。多数オブジェクト時に効かないのは
  「z 順で直下が mergeable な吹き出し」でないと結合しない仕様のため。改善 (最近接 mergeable
  まで探索) は要検討。

## 進め方
- P0 → P1 → P2 の順。各修正は pathspec commit（src/ui_text.rs 等）、退避ブランチ `comic-inc6` 維持。
- まとまった単位で Codex レビュー。
- B8 は設計変更なので、調査結果を添えて方針確認してから実装。

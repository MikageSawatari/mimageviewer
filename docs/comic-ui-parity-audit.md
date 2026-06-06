# comic テキスト注釈 UI パリティ監査（ラボ vs mIV）— 2026-06-06

Status: **監査完了 / 修正未着手**。本書は `tools/comic_lab`（参照実装）と本体 mIV
（`src/ui_text.rs` ほか）の **UI 配置・デザインの差分**を file:line 付きで洗い出した正本。
別セッションで comic UI を修正する際／コンパクト後に作業を再開する際の作業指示書として使う。

> ⚠️ **行番号は目安**。`src/ui_text.rs` は並行編集中で行がずれる。**関数名で探して実装時に再確認**する
> （CLAUDE.md の方針どおり）。ラボ側 `tools/comic_lab/src/main.rs` は比較的安定。

> 📌 **背景**: Inc 4d/4e + earlier-Inc ギャップ解消で「§7 完全パリティ署名」を出したが、
> その後の精密照合で **配置（常時表示エリア）と複数の編集コントロールがラボと未一致**であることが判明。
> よって `docs/comic-integration-plan.md` の **§7.0 サインオフは現状不正確** → 下記「§7.0 の格下げ」参照。

---

## 0. 結論サマリ（最重要 3 点）

1. **右パネルの「常時表示エリア（タブの上）」が mIV に無い**（ラボの最大の構造）。ラボは
   本文テキスト/記号挿入/自動サイズ/プリセット/構造トグル/名前ヘッダを**タブの上に常時表示**し、
   タブは詳細のみ。mIV は全部タブの中に畳んだ → よく使う本文欄が Body/しっぽタブで消える。
2. **本体タブの形状別パラメータ・seed スライダーが mIV に丸ごと無い**（機能欠落、§7.2 署名と不一致）。
3. **しっぽの tip(先端)・付け根(base) 編集コントロールが mIV に無い**（機能欠落、§7.3 署名と不一致）。

加えて 追加UI（左パネル）・追加ダイアログ5種・各タブのラベル/範囲/widget型に多数の差分。

---

## 1. Area 1 — 右パネル構造（常時表示 vs タブ）

### 1-1. タブの上に出ているもの 🔴 構造差
- ラボ `draw_properties`（main.rs:3463）に明示コメント `// ===== 常時表示 (above the tabs) =====`
  （main.rs:3499 付近）。描画順:
  1. 種類ラベル + separator（inline 3488-3495）
  2. 名前ヘッダ（窓: モード/色/話者名）`draw_window_name_header`（main.rs:4481、呼び3503）
  3. オーバーフロー警告（窓・赤 `(!) テキストが枠に収まっていません`、3518）
  4. **本文テキスト欄 + 記号挿入** `draw_text_body`（main.rs:7701、呼び3529/3540）
  5. 吹き出し自動サイズ `draw_bubble_autosize_toggle`（main.rs:3685、呼び3550）
  6. セリフプリセットバー（常時）`draw_text_preset_area`（main.rs:3839、呼び3556）
  7. 本体/ウィンドウプリセットバー `draw_shape_preset_area`(3920)/`draw_window_preset_area`(4000)
  8. 構造トグル（結合/しっぽ/飾り）`draw_bubble_toggles`（main.rs:3705、呼び3572）
  - その後にタブ行（main.rs:3597〜）。
- mIV `edit_object_ui`（ui_text.rs:`fn edit_object_ui`、現状 4549 付近）はタブ上に
  **①種類ラベル `ui.strong(kind_label)` ②回転°スライダー** の 2 つだけ。残りは全部タブ dispatch の中。
- **修正方針**: `edit_object_ui` 冒頭に「常時表示エリア」を新設し、上記 4・5・6・7・8 と
  名前ヘッダ(2) をタブ上へ移す。タブ側は detail のみに戻す。＝ラボ `draw_properties` の構造移植。
  （回転°スライダーは mIV 独自。残してよいが配置は要検討。）

### 1-2. プリセットバー 🔴🟡
- 配置: ラボ＝3 本とも常時タブ上（セリフ/本体/窓）。mIV＝各タブ内に分散。
- 🔴 **mIV は吹き出し/窓の「セリフ」タブにテキストプリセットバーが無い**（`text_preset_bar` は
  Text オブジェクト時のみ、ui_text.rs:4578 付近）。→ 吹き出し/窓本文にセリフプリセットを当てられない。
- 🟡 見出し: ラボ＝色付き「セリフプリセット/本体プリセット/ウィンドウプリセット」。mIV＝一律 small「プリセット」。
- 🟡 ボタン: ラボ＝「登録/削除」（名前欄一致で削除）。mIV＝「現在を保存/更新」＋各ボタン `×`。
  リンク色 ラボ rgb(36,112,150) / mIV rgb(50,96,140)。
- ラボ各バー定義: `draw_text_preset_area`(3839)/`draw_shape_preset_area`(3920)/`draw_window_preset_area`(4000)。
  mIV: `text_preset_bar`/`shape_preset_bar`/`window_preset_bar` → 共通 `preset_buttons_ui`。

### 1-3. 自動サイズ 🟡
- ラボ＝タブ上に常時（コメント「本体タブに埋めない」main.rs:3547）、ラベル「吹き出し自動サイズ」。
- mIV＝本体タブ内（ui_text.rs:5214 付近）、ラベル「自動サイズ」。

### 1-4. タブ有効/無効ゲート 🟡（既知の設計差）
- ラボ＝しっぽ/飾りタブを非対応時に dim 無効化（main.rs:3603-3612、`tail_enabled`/`deco_enabled`）。
- mIV＝全タブ常時 `true`（ui_text.rs のタブ行）。`prop_tab_button` に無効描画はあるが未使用。
- ※ mIV はトグルがタブ内にある設計なので「タブ常時有効＋トグル/編集ブロックを無効化」を選択済み。
  これは意図的差なので**そのままでも可**（要・方針確認）。

---

## 2. Area 2 — 追加ダイアログ 5 種

横断差: mIV は 5 種すべて統一の暗フレーム（`Frame::window`+Foreground+強制dark visuals）。
ラボは **吹き出し/窓/オノマトペは暗フレーム、フォント/スタンプは素 `egui::Window`（左上寄せ・固定サイズ）** と不統一。
→ **mIV の統一の方が良い可能性**（直すならラボ側。mIV は現状維持でよいか要確認）。

| ダイアログ | 主な差分（lab → mIV） |
|---|---|
| 吹き出し `draw_add_dialog`(3155) / `draw_text_add_bubble_dialog` | 🟢 タイトル「吹き出しを追加」一致・サムネ一致。🟡 既定幅 540→600、説明文字 11→12px、グリッド間隔 10→8px。|
| ウィンドウ `draw_add_window_dialog`(3225) / `draw_text_add_window_dialog` | 🔴 タイトル「メッセージウィンドウを追加」→「ウィンドウを追加」。🔴 セル幅 150→116（列数・サムネ幅変化、`WINDOW_PRESET_CELL_W`=150 vs `WIN_CELL_W`=116）。🟡 既定 640×560→540×460。|
| オノマトペ `draw_onomatopoeia_dialog`(3365) / `draw_text_add_onomatopoeia_dialog` | 🟢 タイトル/説明/カードほぼ一致。🟡 **mIV のみ**「追加パック未導入」警告＋「編集用追加パックを入手…」ボタン（mIV 独自で妥当）。|
| フォント `draw_font_dialog`(2138) / `draw_text_font_dialog` | 🔴 最も乖離。タイトル「フォントを見本から選択」→「フォントを選択」。素 Window→暗フレーム。🔴 **カテゴリ UI**: ラボ=ラジオ3（日本語/英語/すべて）→ mIV=タブ4（すべて/追加パック(N)/ユーザー追加/システム）。🟡 ラベル「見本テキスト:」→「見本」、「絞り込み:」→「絞り込み」、「フォントファイルを開く」→「ファイルから追加…」。|
| スタンプ `draw_stamp_dialog`(1190) / `draw_text_add_stamp_dialog` | 🟢 中身（検索/最近/カテゴリ/グリッド/アセット未配置注記）ほぼ完全移植。🟡 ラボ素 Window 固定 560×460・左上 vs mIV 暗フレーム・中央。|

---

## 3. Area 3 — 各タブ内コントロール（逐一）

### 3-1. セリフ
ラボ: `draw_text_body`(7701, タブ外=本文欄+記号挿入) → タブ内 `tab_serifu`(3789) →
`draw_text_font`(7761, font+size) + `draw_serifu_tab`(7800, スタイル)。mIV: 全部 `text_block_ui`(4935, with_text)。

| 差分 | 詳細 |
|---|---|
| 🔴 本文テキスト欄の位置 | ラボ=タブ外(常時) / mIV=セリフタブ内（Area 1-1 と同根） |
| 🔴 白フチ/黒フチ/フチなし クイックボタン | ラボ `draw_serifu_tab`(7806)「スタイル:」行にあり。**mIV に無い** |
| 🔵 太/斜(bold/italic) | **mIV にあり**(5026 付近)、ラボに無い（mIV 独自追加・残してよい） |
| 🟡 widget 型 | 組方向/行揃え: ラボ radio → mIV selectable_label |
| 🟡 範囲 | サイズ 6–300 → 8–400、袋文字太さ 0–20 → 0–30 |
| 🟡 既定縁色 | 袋文字 ON 時 ラボ=黒/3px、mIV=白/4px |
| 🟡 自動縦中横 | ラボ=縦書き時のみ表示、mIV=常時表示 |
| 🟡 順序/ラベル短縮 | ラボ: …記法→行揃え→行間→袋文字(最後)。mIV: …袋文字→自動縦中横→記法→行間。「文字色→色」「縁取り色→縁色」「縁取り太さ→太さ」「フォント:→フォント」等。ラボの記法ヘルプ行は mIV で削除 |

### 3-2. 本体（吹き出し）🔴 最重要の機能欠落
- 🔴 **形状別パラメータのスライダー群が mIV に丸ごと無い**。ラボ `tab_body`(4555-4858) は形状ごとに
  rx/ry・半幅/半高・角丸・トゲ数/深さ・こぶ数/深さ・辺の数・向き(度)・線の本数・外半径・線の間隔・
  **再生成(seed)** を出す。mIV `bubble_body_ui`(5159) は**形状コンボのみ**で `to_shape(hw,hh)` に倒すため、
  トゲ数・seed 等を一切調整できない。→ **§7.2「形状別パラメータ [x]」は誤り**。
- 🟡 形状選択位置: ラボ=形状プリセットエリアで選び本体タブは微調整専用 / mIV=本体タブ頭にコンボ。
- 🟡 結合/自動サイズ: mIV=本体タブ内 / ラボ=タブ外（`draw_bubble_toggles`/`draw_bubble_autosize_toggle`）。
- 🟡 ラベル/範囲: 塗り不透明度→不透明、枠線色→線色、枠線太さ→線幅(0–20→0–30)、内側余白→余白(0–80→0–120)。「塗り/枠」見出しは mIV で削除。

### 3-3. しっぽ 🔴
- 🔴 **先端 tip(X/Y) DragValue が mIV に無い**（ラボ `tab_tail` 4937）。
- 🔴 **「付け根を自動」＋「付け根位置」スライダーが mIV に無い**（ラボ 4949-4956）。→ **§7.3「tip/base_t/base_auto [x]」は誤り**。
- 🟡 種別ラベル: ラボ「三角/思考(丸)」→ mIV「会話/思考」。widget radio→selectable。
- 🟡 幅: ラボ=種別で「円の大きさ/付け根の太さ」出し分け 4–200 / mIV=「幅」固定 4–120。
- 🟡 表示トグル位置: mIV=タブ内 / ラボ=タブ外。

### 3-4. 飾り 🟢
ほぼ完全一致（mIV のみ空状態ヒント行を追加）。ラボ `tab_deco`(4977) / mIV `bubble_deco_ui`。

### 3-5. 枠（ウィンドウ本体）🟢
**完全一致**（順序・ラベル・widget・範囲すべて）。ラボ `tab_window_body`(4082) / mIV `window_body_ui`。

### 3-6. 部品（ウィンドウ）🟡
- 名前モード/色/話者名: ラボ=常時ヘッダ`draw_window_name_header`(4481) / mIV=部品タブ内`window_parts_ui`（Area 1-1 同根）。両者 ComboBox（ラジオではない）。
- 🟡 ラベル: 行頭「名前:」→「表示」、オフセット「位置オフセット X/Y」→「位置 X/Y」。
- 🟡 文字サイズの位置（話者名との前後）が異なる。
- 🟢 立ち絵枠・続き指標は完全一致。

### 3-7. スタンプ 🟡
- 🟡 ラボ先頭「種類: スタンプ」ラベル+separator が mIV `stamp_ui` に無い。
- 🟡 「不透明度」→「不透明」。縁取り色 widget **ラボ srgb(α無) vs mIV srgba**、ラベル「色」→「縁色」。範囲一致。

---

## 4. mIV が意図的にラボと変えた点（“直さない”もの）

混乱防止のため、以下は**意図的差**（修正時に元へ戻さないこと）:
- 太/斜(bold/italic) を mIV が追加（3-1）。
- 名前テキスト編集ではウィンドウ preset link を切らない（mIV 規約=テキスト内容では link 非解除と一貫）。
- しっぽ/飾りタブは非対応形状でも選択可（トグル/編集ブロックを無効化する方式。ラボはタブ自体を無効化）。
- 追加ダイアログ5種を全部暗フレームに統一（ラボはフォント/スタンプが素 Window）。

これらを「ラボに合わせるか mIV 流を維持するか」は**ユーザー判断**（要確認）。

---

## 5. `docs/comic-integration-plan.md` §7.0 の格下げ（要・別途修正）

現行 §7.0 は「ラボ完全パリティ達成（v1 スコープ）」と署名しているが、本監査で以下が**未達**と判明:
- §7.1: 白フチ/黒フチ/フチなし クイックボタン未実装。本文欄＋セリフプリセットが常時表示でない。
- §7.2: 形状別パラメータ・seed スライダー未実装（コンボのみ）。
- §7.3: しっぽ tip / 付け根（base_t/base_auto）編集 UI 未実装。
- §7.7: 右パネル「常時表示エリア」構造が未移植。吹き出し/窓セリフタブにテキストプリセット無し。

→ §7.0 を「**配置・一部コントロールが未一致（要修正）**」に格下げし、上記を残タスクとして明記すること
（このリポジトリ doc の修正は comic-integration-plan.md を触る別セッションと衝突しうるので、担当一本化後に行う）。

---

## 6. 修正スコープ（対応規模順・着手順の目安）

1. **右パネルを 2 層構造へ戻す**（最重要・ユーザー主訴）: `edit_object_ui` に常時表示エリアを新設し、
   本文欄・記号挿入・(吹)自動サイズ・3 プリセットバー・構造トグル・(窓)名前ヘッダをタブ上へ移す。
   ＝ラボ `draw_properties`(3463) の構造移植。**大**。
2. **本体タブの形状別スライダー＋seed** 移植（ラボ `tab_body` 4555-4858 の match ブロック）。**中〜大**。
3. **しっぽの tip/付け根** コントロール移植（ラボ `tab_tail` 4937-4956）。**中**。
4. **吹き出し/窓セリフタブにテキストプリセットバー**を出す（または常時表示化で同時解消）。**小**。
5. **左追加 UI**: 全幅縦ボタン＋「○○追加」ラベル＋並び順、一覧カードに本文抜粋（ラボ `draw_left_panel` 2886 / `draw_object_list` 2984）。**小〜中**。
6. **追加ダイアログ細部**: 窓タイトル/セル幅、フォントダイアログのタイトル・カテゴリ UI、説明文字サイズ等。**小**。
7. **セリフのクイックボタン（白/黒/なしフチ）** など小物。**小**。

---

## 7. 編集用追加パック（BiRefNet + オノマトペフォント）の配布 — 別ワークストリーム

**UI 修正とは独立**（中身はフォント＋モデルでパネル実装と無関係）。先行公開しても可。

- 仕組みは実装済み: `editing_addon.rs` / `editing_addon_download.rs` / `ui_dialogs/editing_addon.rs` /
  `bin/build_editing_pack.rs`。DL→sha256→展開（`%APPDATA%/mimageviewer/addons/editing/`）、https のみ（release）。
- 配布スクリプト: `scripts/publish-editing-pack.sh`（build/check/publish）。タグ `editing-pack-v1`
  （= `DEFAULT_PACK_BASE_URL` と一致）、アセット = `editing-pack-<version>.zip` + `editing-pack-index.json`。
- 素材 staged 済み: `vendor/editing-pack/models/birefnet_fp16.onnx`（467 MiB）+ license、
  フォント `tools/comic_lab/assets/fonts/`（18 書体 / 62 MiB / 各 OFL）。合計 約 530 MiB。
- 現状の被写体モデル = **BiRefNet fp16 のみ（pack DL）**。軽量 U²-Netp(4.4MB) の埋め込みは廃止済み
  （`vendor/models/u2netp.onnx` は残るが未使用）。pack 未導入だと被写体分離は使えない（軽量フォールバック無し）。
  - ※ ユーザー要望「小=常時配布／大=DL」を厳密に満たすには **U²-Netp 埋め込み復活＋自動切替**が別途必要（未実装）。

### 推奨手順（Pre-release。Latest 枠を奪わせない＝TRT と同方針）
```bash
bash scripts/publish-editing-pack.sh build --models vendor/editing-pack/models
bash scripts/publish-editing-pack.sh check
bash scripts/publish-editing-pack.sh publish --models vendor/editing-pack/models
# ★ publish-editing-pack.sh は --prerelease を立てないので、後で prerelease 化:
gh release edit editing-pack-v1 --repo MikageSawatari/mimageviewer --prerelease --latest=false
```
- draft アセットは匿名 DL 不可 → アプリ検証には published(prerelease) が必要。sha256 + `--clobber` で差し替え安全。
- （任意の整形）`publish-editing-pack.sh` の `gh release create` に `--prerelease` を足せば最後の手動 edit が不要。

---

## 8. 進行上の注意（並行セッション）

- `src/ui_text.rs` は別セッションも触る。**comic UI 修正の担当を一本化**してから着手（共有ツリー衝突回避）。
- コミットは pathspec（`git commit -F msg -- <自分のファイル>`）、退避ブランチ維持（CLAUDE.md「Git Workflow」）。
- 推奨進行: 監査doc(本書)確定 → コンパクト → 他セッションの comic 編集完了待ち → 本書を見て §6 を上から修正
  → アップロード済みパックで **DL→編集 通し実機検証** → §7.0 を実態に更新。

# §1.772 ページめくりの演出に「スライド」を加える — 設計案

2026-11-23、設計基点 `page-turn-slide-772` / master `1a15e96bc`。状態: **設計第 4 版、利用者の判断 (§13) と HUD・キーの §7.3 を含めて r4 ACCEPT (§16)。実装中**。
正本の要望は [backlog §1.772](next-release-backlog.md)。既存の演出の正本は
[ページめくり演出の設計](page-turn-animation-plan.md) (以下「本設計」)、表示単位の照合は
[ページめくりとレイアウト](page-turn-layout-binding-plan.md)、描画の位置付けは [表示パイプライン](display-pipeline.md)。
行番号は基点のもの。`PT` = `src/ui_fullscreen/page_turn.rs`、`PTA` = `src/page_turn_animation.rs`、`FS` = `src/ui_fullscreen.rs`。

## 1. 要望と範囲

利用者要望 (2026-11-12): 他の漫画ビューアには、ページが横にスライドして切り替わる演出がある。
今のめくる演出と、スライドする演出を選べるようにしたい。

この設計では、**今の演出の所有の仕組み (受理・素材の保持・提出・取消・着地) を一切変えず、
1 回の演出の「絵の作り方」(projection) だけを「めくる」と「スライド」から選ぶ**。
演出の状態 (`PresentationState`) の種類・遷移・取消の宛先は増やさない。

範囲外 (§13 の判断で変わり得る): 指・マウスに追従して途中まで動かし、離した位置で進む/戻るを決める操作 (§6.3)、
本ごとの演出の選択 (§7.1)、Remote での演出 (本設計 Q5 の決定を継続)、分割表示・連結読みでの演出 (§5)。

## 2. 現行の仕組み (コードで確認した事実)

| 境界 | 事実 |
| --- | --- |
| 状態 | `PT:1168` `PresentationState { Inactive, Flat, Offered, Turning, Completing }` を context ごとに 1 つ持つ (`PageTurnPresentation`)。Turning は `PreparedLeaf { outgoing, incoming: CanonicalUnit, direction, recipe: TurnRecipe }` と `MotionClock` を持つ (`PT:1110`、`PT:1121`) |
| 受理 | `PT:2361` `offer_page_turn_with_operation` が、受理済みの隣接移動・単発・即 ready・適格な表示 (`PT:1883` `page_turn_view_eligible`、分割表示は `PT:2408` で除外) だけを Offered にする。時間はここで `page_turn_animation_duration_ms` (100〜600) から取る (`PT:2418`) |
| 開始 | `PT:2815` 以降、最初の通常 prepare の後に Offered → Turning。照合 (binding・素材・`stage_one_shape_reason` `PT:3508`) と `prepare_leaf` (`PT:1145`) が通れば Turning、通らなければ同じ pass で Flat |
| 毎フレームの絵 | `PT:3015` が `project_prepared(outgoing, incoming, direction, recipe, progress)` (`PTA:461`) を呼び、返った mesh を描き、可視面積を receipt に載せる。**絵を作るのはこの純粋関数だけ**で、I/O・decode・upload をしない (`PTA:1`) |
| 取消・着地 | 新しい入力・設定変更・素材や表示の変化で `cancel_with_fields` (`PT:1277`) → Completing(NeedFlat) → 着地先の通常表示を 1 frame 採用。戻る演出はない (受理済みの移動を取り消さない) |
| 設定 | `page_turn_animation_enabled` (既定 OFF) と `page_turn_animation_duration_ms` (既定 200)。環境設定 (`src/ui_dialogs/preferences/pages.rs:12108`)、HUD「その他の機能」の ON/OFF 行 (`FS:38584`、`KeyAction::FsPageTurnAnimationToggle`)、設定転送 (`src/settings_transfer.rs:364`)、設定 DB の kv。採用は `adopt_page_turn_animation_settings` (`PT:1709`) 1 か所で、全 context の演出を取り消す (`src/app/viewer_context_registry.rs:12178`) |
| 演出 ON の副作用 | 見開きの先頭・末尾の単ページを本来の側へ置く (`FS:21543`)。めくりの紙が綴じ目を軸にするための規則 (本設計 §26.2) |
| 先読み・VRAM | ON の間、次の表示単位の final を先に用意する (`src/app/final_prepare.rs:502`)、旧・新の素材を pin する (`PT:2880`)。演出の種類には依存しない |

## 3. 採る形: 演出の種類は projection の recipe の違いだけにする

### 3.1 状態を増やさない

- 新しい設定 `page_turn_animation_style: PageTurnAnimationStyle { Curl, Slide }` (既定 `Curl`)。
- `AcceptedOffer` (`PT:1099`) に受理時の `style` を 1 つ写す (duration と同じ扱い。受理後に設定が変わったら §3.3 のとおり取り消すので、写した値と設定は食い違わない)。
- `PreparedLeaf.recipe: TurnRecipe` を `MotionRecipe { Curl(TurnRecipe), Slide(SlideRecipe) }` に置き換える。
  `prepare_leaf` は style で `prepare_turn` か `prepare_slide` を呼び分ける。`project_prepared` は recipe で分岐する
  (または `project_slide` を別関数にして `PT:3015` の 1 か所で分岐する。どちらでも呼出元は 1 か所)。
- `PresentationState` の種類・遷移、`MotionClock`、`SubmissionTracker`、receipt、取消の宛先、Completing の着地、
  `PageTurnUnitBinding` の照合、先読み・pin・VRAM 会計は**そのまま共有**する。スライド専用の状態・時計・素材の保持を作らない。
- 名前: receipt の `curl: bool` と `SubmissionKind::Curl` は「projection で描いた frame」の意味でそのまま使う
  (両方の種類を含む、とコメントを直す)。名前の付け替えは差分を広げるだけなのでしない。
- 診断の style (r1 改訂): 受理時の style を `AcceptedOffer` だけでなく、その操作の不変な診断情報 (`SubmissionTracker`、
  取消後に残る `CompletionDiagnostics` `PT:1192`) にも持たせ、offer / skip / start / cancel / 遅延 output / 取消後の着地の
  各 perf event へ同じ値を載せる。event の時点の `Settings` を読み直さない (設定変更の直後に旧スライドの着地をめくりとして
  記録しないため)。受理前に skip した操作は、その時点の設定の style を載せる。`scripts/analyze_perf.py` の
  `page_turn_animation` 集計 (`:380`・`:449` の集計キー) に style を加え、style の無い旧ログは `curl` として扱う。

### 3.2 受理・開始の条件は共有する

受理 (`offer_page_turn_with_operation`)、`stage_one_shape_reason`、`canonical_unit` の正規化 (見開きの単ページを
2 面へ、横長を中央で 2 面へ)、`preflight` (`PTA:879`: 画素倍率・slot 数・clip・ROI・UV) は**両方の種類で同じものを通す**。
スライドだけが受け入れる形・スライドだけが拒否する形を作らない (§9)。種類ごとに違うのは次だけ。

| 段 | めくり | スライド |
| --- | --- | --- |
| `admission` (`PTA:998`) | 両面 Vacant の leaf を拒否 | 不要 (表示単位には必ず実ページがある) |
| mesh 誤差 (`PTA:550`) | 透視の分割数を決める / 超えたら拒否 | 不要 (平面の四角形だけ) |
| 初期 pose | 折り返し角 30/45/60° の最小で可視判定 (`PTA:423`) | 初期の移動量 `e0` を候補から選ぶ (§4.3) |
| 共通の黒い紙 (§26.1) | 単ページの大きさ違いで使う | 使わない (各ページが自分の大きさで動く) |

### 3.3 設定の変更

`adopt_page_turn_animation_settings(enabled, duration_ms, style)` に style を足し、ON のまま style が変わったときも
duration の変化と同じく `cancel_all_page_turn_effects_for_settings` を呼ぶ。環境設定の確定と HUD の両方がこの 1 か所を通る
(新しい採用経路を作らない)。OFF の間の style 変更は保存だけで何も取り消さない (今の duration と同じ)。

## 4. スライドの動き

### 4.1 向き

読む向きの「次のページがある側」から新しいページが入り、今のページはその反対側へ出る。

| 綴じ方向 / 操作 | 今のページが出る側 | 新しいページが入る側 | 画面上の動き |
| --- | --- | --- | --- |
| 左綴じ (左→右読み) / 進む | 左 | 右 | 左へ動く |
| 左綴じ / 戻る | 右 | 左 | 右へ動く |
| 右綴じ (右→左読み) / 進む | 右 | 左 | 右へ動く |
| 右綴じ / 戻る | 左 | 右 | 左へ動く |

向きは受理した移動の進む/戻る (`AcceptedOffer.forward`) と incoming の読み方向 (`TurnDirection`) から決め、
めくりと同じ値を使う。画像の UV・印字は鏡映しない。縦方向には動かない。

### 4.2 動かすもの

- 1 つの表示単位 (単ページ、または見開きの 2 面) を**ひとかたまり**として平行移動する。見開きの綴じ目の隙間・
  白紙の相方 (Blank)・ページ背景 (underlay)・切り取り枠 (crop paint) も同じ量だけ動く。Vacant は何も描かない。
- 描く範囲は表示単位の clip (`preflight` で旧・新が等しいことを確認済み)。はみ出した部分は描かない。clip の外の
  通常の背景・HUD はこれまでどおり。影・紙の裏・透明度の変化は付けない。
- 旧・新の重なりは作らない。重ならないので可視面積の判定は「clip 内に見えている面積」と同じになる。
- 実装は既存の平面 `Surface` (`PTA:304`、`flat_surface` `PTA:1132`、`append_flat_crop`) を、配置 rect を平行移動
  して使う。`Surface.rect` は「大きさと UV の基準を保ったまま動かせる配置」として既にあり、新しい描画 primitive は要らない。

### 4.3 移動量と時間

左へ動く場合 (右へ動く場合は左右を入れ替える)。旧・新それぞれの内容の範囲 `Bo` / `Bn` = Vacant 以外の全ページ rect の和、
clip を `C` とする。

- 新しい単位の開始位置: 新しい単位の左端を `L = max(C.right, Bo.right)` に置く。新しい単位の移動量 `Sn = L − Bn.left`
  (着地で 0)。clip の端から入るので、新しい単位が途中で突然現れない。旧のはみ出し (指定サイズ表示) とも重ならない。
- 旧の単位の移動量: `So = max(Sn, Bo.right − C.left)`。旧の単位は `p → 1` で clip 内の可視面積が 0 へ収束し、`p = 1` では
  描かない (`p < 1` の間は端が clip 内に残り得る。着地の frame で旧を消すのは連続な動きの終点であって飛びではない)。
  `So ≥ Sn` なので両者の間隔 `L − Bo.right + (So − Sn)·e ≥ 0` は縮まず、重ならない。大きさの違うページ (縦長 → 横長など)・
  Vacant の除外・指定サイズのはみ出しでも成り立つ (r1 独立レビューで確認)。
- 進み具合: `e(p) = e0 + (1 − e0) · smooth(p)` (`smooth` は既存の smoothstep `PTA:610`)。旧は `−So·e`、新は `Sn·(1 − e)` だけずらす。
- **終端は通常の flat paint**: 所有側は `progress ≥ 1` で投影せず Completing へ移り、既存の通常 paint 経路 (`PT:3167`) で
  incoming を描く。純粋関数の `p = 1` も既存の `scene` (`PTA:670`) と同じく incoming だけを保存済みの rect / UV / clip で返す。
  移動量の計算を経由した丸め誤差を終端へ持ち込まない。mesh の分割・頂点配列を通常 paint (横長は 1 枚の quad、clip は GPU 側) と
  揃える処理は作らない (r2 改訂)。
- 初期の移動量 `e0` (r1 改訂): 判定は既存の `meaningful` (`PTA:564`) をそのまま使う。これは「**どれか 1 つの** incoming 実ページが
  5% 以上かつ短辺 8 pt 以上見える」判定で、各ページではない。`{0, 1/32, 2/32, …, 8/32}` から最初に満たす値を選ぶ。
  - 単ページ表示: 満たせなければ `IncomingNotVisible("initial_target_visibility")` で開始せず、その frame は通常表示
    (めくりの単ページ `PTA:440` と同じ)。新しいページは clip の端から入るので、通常の fit では `e0` は新しいページ幅の 5% 前後になる。
  - 見開き: 満たせなくても `e0 = 1/32` で開始する (めくりの見開きが 30° で開始する `PTA:438` と同じ扱い)。先頭側が白紙の相方
    (Blank、occurrence なし) や、入る側から遠い実ページは、後から clip に入った時点で receipt に面積が載る。それまでの receipt は 0 のまま
    扱い、提示済みの判定は既存どおり canonical flat の採用で行う (`PT:1598` 付近)。新しい状態・待ちは作らない。
- 時間は既存の `page_turn_animation_duration_ms` を共有する (§13 Q4)。

### 4.4 表示の形ごとの動き

| 形 | 動き |
| --- | --- |
| 単ページ表示 (縦長・横長) | 1 枚が出て 1 枚が入る。大きさが違っても各自の rect のまま動く |
| 見開き (2 面) → 見開き | 2 面と綴じ目の隙間が一緒に動く |
| 見開きの先頭・末尾の単ページ (本来の側に配置) ↔ 見開き | 単ページとその横の空き (Vacant) を含む単位として動く。空きは描かない |
| 見開き中の横長 1 枚 | 1 枚として動く (中央で 2 面へ正規化しても、2 面が一緒に動くので見た目は 1 枚) |
| 白紙の相方 (隣を白紙にする) | 白い面も一緒に動く |
| 補助表紙を添えた末尾 | 2 面として動く |
| スライドショー | 手動と同じ条件・同じ時間。間隔が時間より短いときは省く (既存 `PT:3502`) |

## 5. 対象にしない表示 (めくりと同じ)

連結読み (縦・横) は今もスクロールで動くので演出なし。分割表示 (`PT:2408`)、ズーム・パン・自由回転・編集・比較・分析・
ルーペ・アニメーション画像・動画・音声・Remote も、めくりと同じく演出しない。スライドなら分割表示にも自然に使えるが、
受理の条件を種類で分けないため今回は入れない (§9。必要なら別項目)。

## 6. 入力・取消・着地

### 6.1 連打・長押し・方向反転

めくりと同じ。演出中の次の入力で即取消し、その移動は通常表示。連打・押し続けの 2 回目以降・wheel の複数 notch・
gamepad の repeat は演出なし。スライドのための入力待ち・queue は作らず、既存の 1 件の解決済み continuation と flat の barrier
(`src/ui_fullscreen/page_turn/barrier.rs:296`・`:346`: 取消後の必須 flat が採用されてから次の frame で 1 回だけ commit する) を
そのまま使う (r1 改訂)。

### 6.2 取り消した着地

めくりと同じく、取り消したら**着地先のページを即通常表示**する (Completing → 必須の flat を 1 frame)。
受理済みの移動は取り消さないので、元のページへ戻る動きは作らない。

### 6.3 指・マウスで途中まで動かして戻す操作 (今回は入れない、§13 Q2)

今の演出は「移動を受理した後の見た目」だけを持ち、移動そのものは持たない (本設計 §4)。指に追従させるには、
移動を受理する前に次の単位を描き、離した位置と速さで「進む / 元へ戻る」を確定し、戻すときは**移動していない**
戻り演出を描く、という別の所有が要る。これは navigation sequence・読書位置・入力の所有 (タッチの 1 本指ドラッグは
今はパン、`src/touch_input.rs:165`) にまたがる新機能で、スライドの種類追加とは独立に設計すべき大きさ。
めくりにもこの操作はないので、今回はスライドにも入れず、要るなら別の backlog 項目にする。

## 7. 設定・UI・保存

### 7.1 選ぶ場所

- 環境設定 → 表示 → 閲覧表示の「ページめくりアニメーション」の下に「動き: めくる / スライド」(radio) を足す。
  ON のときだけ操作でき (時間のスライダーと同じ `add_enabled_ui`)、説明文は種類に依らない言い方へ直す。
- HUD「その他の機能」の行は「OFF / めくる / スライド」の 3 つから選ぶ (利用者決定 2026-11-23、§13 Q1)。詳細は §7.3。
- 本ごとの設定は作らない (§13 Q1)。複数ウィンドウ (F12) も含めて全体で 1 つ。

### 7.3 HUD の行とキー (利用者決定 Q1 = 案 C を受けた設計、2026-11-23 追記)

- **HUD の行はサブメニューを開く**。既存の多値の行「ナビゲータ位置」「ページ内の読み順」と同じ形
  (`FS:38655` の `label  >` 表示、`FS:6946` のサブメニュー描画、`FS:39071` の Activate → panel state 遷移) に揃える。
  - root の行: 見出しは今と同じ「ページめくりアニメーション」と `FsPageTurnAnimationToggle` の先頭 chord
    (`first_chord_bracket_label`、今と同じ)。状態の欄は `OFF  >` / `めくる  >` / `スライド  >`。
  - 押すと `FsOverflowPanelState::PageTurnAnimation` (新しい値) へ移り、「<  戻る」と「OFF」「めくる」「スライド」の 3 行を出す。
    今の値に「選択中」を付ける。選ぶと設定を変えてサブメニューにとどまる (「ページ内の読み順」と同じふるまい。「ナビゲータ位置」は選ぶと root へ戻る `FS:39095`)。
  - 1 回押すたびに 3 つを順に回す方式は採らない。押すたびに演出の取消と保存が走り、目的の値に着くまでに途中の値を経由するため。
  - `fs_overflow_panel_body_row_count` に 1 + 3 行を足し、固定パネルの高さ検査 (`tests/support/scroll_layout.rs`) を通す。
    `closes_on_file_change` の対象にはしない (全体設定。ナビゲータ位置と同じ)。
  - 行の id `FullscreenOverflowItemId::PageTurnAnimationToggle` (`src/settings.rs:1025`) は保存されない固定並びの識別子なので、
    名前は変えない (差分を広げない)。
- **設定の変更は 1 か所を通す**: 環境設定・HUD のサブメニュー・キーのいずれも `adopt_page_turn_animation_settings(enabled, duration, style)`
  → `settings.save()` の既存の順序を使う (今の `toggle_page_turn_animation` `FS:38584` と同じ)。HUD・キーでの変更は全体設定を変えるので、
  兄弟 context の演出も既存の `cancel_all_page_turn_effects_for_settings` で取り消される。
- **キー**: `FsPageTurnAnimationToggle` (FsImage / Press、既定キーなし) の意味は**今の ON/OFF のまま**にする。ON にするときは保存済みの
  style (めくる / スライド) で ON になる。既にキーを割り当てている利用者の操作の意味を変えないため (リリース済みの操作)。
  種類を変える新しい `KeyAction` は足さない。頻度の低い好みの設定で、HUD のサブメニューと環境設定で選べるため。理由を
  `docs/keymap-spec.md` の `FsPageTurnAnimationToggle` の段落に追記する。
- **トースト**: ON/OFF の切り替え (キー・HUD) と HUD のサブメニューの選択で、`[ページめくりアニメーション OFF]` /
  `[ページめくりアニメーション めくる]` / `[ページめくりアニメーション スライド]` を出す (今の `ON` を種類名にする)。
- **Remote**: 「その他の機能」の HUD は PC の静止画フルスクリーンだけにあり、mIV Remote (Web UI・IPC) にはこの行もページめくりの
  設定も無い (コード検索で `src/` 外に参照なし、本設計 Q5)。IPC の版・Remote の画面は変えない。
- **detached (F12)**: 別ウィンドウの HUD も同じ `fs_overflow_panel_state` と描画関数を使う既存のサブメニューの経路で、新しい
  detached 述語・viewport 経路は足さない (§8 の判断は変わらない)。
- **UI スナップショット**: `fullscreen_overflow_panel_root`・`_root_tall`・`_page_turn_on` は状態の欄の文言が変わるので更新する。
  新しく `fullscreen_overflow_panel_page_turn_submenu` (スライド選択中) を足す。環境設定の 2 枚 (`page_turn_animation_settings_*`) も更新・追加 (§11)。
- **テスト (handler-level)**: root の行を押すとサブメニューへ移る / 各行を押すと設定・保存・取消・トーストが期待どおり、
  選択中の表示 / キーの toggle が style を保ったまま ON/OFF する / 兄弟 context で HUD から変えたときも全 context の演出が取り消される
  (既存 `viewer_context_registry.rs:8668` の toggle の test を style 付きに広げる) / 既存の HUD の行の test (`FS:69124` 付近) を新しい状態文言へ更新。

### 7.2 保存・移行

- ページめくり演出はリリース済み (v4.7.0 で追加、CHANGELOG)。新しい field を足すだけで、既存の値の意味は変えない。
  欠けているとき (旧版の設定) は `Curl` で、**移行処理は不要** (今の挙動のまま)。
- 設定 DB は既存の kv の enum と同じ書き方 (`page_turn_animation_style` = `"curl"` / `"slide"`)。key が欠けていれば `Curl` と読むので
  SQL の migration は不要。未知の値は既存の enum 設定と同じく `Incompatible` として扱い、`serde(other)` などで黙って `Curl` へ
  変換しない (r1 改訂)。
- 旧版へ戻した場合は、既存の版の保護に従う: DB を開く時点で保存元のアプリ版が新しければ `Incompatible` になり
  (`settings_db.rs:668`・`:1880`)、旧版は設定を書き換えない。新しい key だけを旧版が無視して読む、という経路は無い (r1 改訂で訂正)。
- 設定転送 (`settings_transfer.rs:364`) に 1 行足す。型違い・未知の値は拒否して現在の draft を保つ (既存の 2 行と同じ)。

## 8. 複数ウィンドウ (detached / F12)

detached の述語・viewport の経路・context の mount / park / swap には触れない。style は enabled / duration と同じく
全体の `Settings` の値で、各 context の `PageTurnPresentation` は受理時に写した値だけを使う。設定変更は既存の
`cancel_all_page_turn_effects_for_settings` が全 context に届ける。したがって CLAUDE.md「Detached viewer リワーク中のルール」
の合意・記録の対象 (detached 述語 / viewport 経路に触れる変更) に当たらない。独立レビューでこの判断も確認してもらう。

## 9. 状態の組み合わせを減らす検討 (CLAUDE.md「設計の簡素化」)

- **採った**: 種類を projection の recipe にだけ持たせ、状態・時計・取消・着地・素材の保持を共有する。
  新しく増える組み合わせは「受理時の style × 既存の各状態」だけで、style は受理後に変わらない (変われば取消) ので、
  演出中に種類が混ざる状態は存在しない。
- **採った**: 受理・正規化・preflight を種類で分けない。スライドは本来もっと多くの形 (分割表示・中央の単ページなど) を
  扱えるが、形ごとに種類で受理を変えると「この形はスライドだけ動く」組み合わせが増え、設定を切り替えたときの
  見え方も揃わない。広げるのは利用者の要望が出てから別項目にする。
- **採った**: 見開きの単ページの配置規則 (ON の間は本来の側) を種類で変えない (§13 Q3)。種類で変えると、style の
  切り替えが表示単位の組み直しを起こし、`canonical_unit` の表紙側の証明 (`PT:3796`) もスライドだけ別にする必要が出る。
- **採らなかった**: スライド専用の「元へ戻る」演出・指への追従 (§6.3)。所有が別物になる。
- **採らなかった**: スライドの時間を別の設定にする。設定と取消の条件が 1 つ増えるだけで、要望には無い (§13 Q4)。
- **採った (r1 改訂)**: 初期の可視条件は、めくりと同じく単ページだけ必須、見開きは満たさなくても開始する。見開きで
  必須にすると、入る側が白紙の相方の単位などを開始できず、スライドだけの拒否の形が増える。完了の判定は canonical flat なので、
  見開きの未露出ページの receipt を 0 のまま扱っても新しい状態は要らない。

## 10. まれな失敗

projection は純粋関数で I/O を持たない。設定の保存失敗などは既存の設定保存の扱い (ログ + 通知) に従い、
スライド専用の回復処理を作らない (CLAUDE.md「まれな失敗は既定で割り切る」)。

## 11. テスト

- `PTA` の純粋関数 (`page_turn_animation/` 配下の既存の snapshot / fixture の形に揃える):
  - 4 つの向き (左綴じ/右綴じ × 進む/戻る) で、旧が出る側・新が入る側。`p → 1` で旧の可視面積が単調に 0 へ収束し、
    `p = 1` では旧を描かず、新だけが保存済みの rect / UV / clip で通常の flat paint と一致する。
  - 終端: 純粋関数の `p = 1` は incoming だけで、保存済みの配置と UV 写像のまま、clip 内の描画結果が通常表示と等価
    (非整数 DPI 1.25・1.5、丸め済みの辺、見開きの横長の正規化、crop paint、`PartialTexture` を含む描画比較。頂点配列の一致は求めない)。
    所有側は、時間切れの frame が投影せず既存の通常 paint 経路 (`PT:3167`) で着地することを検査する。
  - 単ページの大きさ違い (縦長↔横長)、見開き↔見開き、先頭の単ページ (Vacant を含む) ↔ 見開き、横長 1 枚、白紙の相方、crop paint、
    `PartialTexture` (domain 内だけ描く)。
  - 旧と新が全区間で重ならない (polygon の交差面積 0)、可視面積の receipt が clip 内の面積と一致。
  - `e0`: 単ページの通常 fit で最小候補が選ばれる / 単ページで条件を満たせないとき `initial_target_visibility` で失敗 /
    見開きで入る側が白紙の相方のとき `1/32` で開始し、実ページの receipt が後から 0 より大きくなる。
  - mesh 予算 (`MAX_PROJECTION_*`) 以内。
- `PT` の所有側 (既存の clock / wait indicator / merge master の test の形に揃える):
  - style=Slide で Offered → Turning → 着地、receipt の incoming 可視面積、MotionClock の開始が共有経路で動く。
  - ON のまま style を変えると Turning / Offered が取り消され着地する。OFF の間の変更は何も取り消さない。
  - 受理時の style が演出中に使われ続ける (設定の読み直しをしない)。
  - 兄弟 context (F12) の演出も style 変更で取り消される (既存 `viewer_context_registry.rs:8686` 付近の形)。
  - continuation を持つ Completing の最中に style を変えても、その continuation は保持され、旧 receipt が遅れて届いても
    新しい flat の採用後に 1 回だけ commit する (barrier の既存の契約がスライドでも同じ)。
  - perf event の style が、設定変更の後に届く遅延 output・取消後の着地でも受理時の値のまま。
- 設定: serde の欠落 → Curl、DB の roundtrip と欠落 (リリース済みの enabled / duration を保持したまま style だけ欠けた旧形式を
  読む)、未知の style が `Incompatible` になる、設定転送の roundtrip / 不正値の拒否。
- 解析器: `scripts/test_analyze_perf.py` に、めくりとスライドが混ざったログ・style の無い旧ログの集計を足す。
- UI スナップショット: 環境設定の page turn 項目 (`page_turn_animation_settings_light_on` / `_dark_off`) を更新し、slide 選択時を 1 枚足す。
- 実行は `run-heavy.ps1` 経由で変更モジュールの filter (`page_turn`、`settings`、`settings_transfer`、`settings_db`、`preferences`) と `ui_snapshot`。
  App の共有状態・非同期・複数ウィンドウの土台は変えないので、lib 全件は統括の master 取り込み後に回す (CLAUDE.md「テストの段階分け」)。

## 12. 文書

- `htdocs/mimageviewer/manual/settings.html` と `fullscreen.html` のページめくりアニメーションの説明に「めくる / スライド」を足す
  (「本のように紙をめくる」だけの言い方を直す)。製品ページに該当カードがあれば同じく。
- `docs/display-pipeline.md` のページめくりの節、本設計の冒頭に本書への参照を足す。`docs/README.md` の索引に本書を足す。
- CHANGELOG の下書き (統括の確認用)。

## 13. 利用者に決めてもらう点 (推奨を先頭)

**利用者の判断 (2026-11-23、統括経由の AskUserQuestion の回答)**:
- Q1: 「HUD でも 3 つの切り替え」。HUD の行を「OFF / めくる / スライド」の切り替えにする。環境設定にも「めくる / スライド」を置き、
  全体で 1 つの設定。本ごとの設定は持たせない。キーの割り当てと HUD の行の扱いは設計で決める → §7.3。
- Q2: 「今回は入れない (推奨)」。Q3: 「めくりと同じ (推奨)」。Q4: 「めくりと共有 (推奨)」。

以下は判断前に示した選択肢 (記録)。

- **Q1 選ぶ場所**: (推奨) 環境設定のページめくりアニメーションの下に「めくる / スライド」を並べ、全体で 1 つ。HUD の行と
  キー割り当ては ON/OFF のまま。 / 案 B: 本ごとにも選べるようにする (本の設定の保存・Remote・本の設定画面に波及し、v5.7.0 は難しい)。
  / 案 C: HUD の行を「OFF / めくる / スライド」の切り替えにする。
- **Q2 指・マウスで途中まで動かして戻す操作**: (推奨) 今回は入れない (めくりにも無い)。要るなら別項目として設計する。
  / 案 B: 今回入れる (§6.3 の新しい所有が要り、v5.7.0 には間に合わない見込み)。
- **Q3 見開きの先頭・末尾の単ページの配置**: (推奨) スライドでも今のめくりと同じく本来の側へ置く (ON の間の規則を共有)。
  / 案 B: スライドのときは保存済みの配置 (中央など) に従う (§9 の理由で表紙の判定と設定画面の灰色表示を種類ごとに分ける費用がかかる)。
- **Q4 速さ**: (推奨) めくりと同じ「時間」の設定を共有する。 / 案 B: スライド用に別の時間を持つ。

参考 (相談不要、めくりと同じ扱いにした点): 見開きのスライドでは、入る側から遠いページ (または入る側が白紙の相方のときの実ページ)
は少し遅れて画面に入る。旧の単位が clip から大きくはみ出す表示 (指定サイズ) では、最初の frame で新しい内容がまだ見えない
こともあり、その間の receipt は 0。単ページ表示ではこの場合は演出せず通常表示にする (§4.3)。

## 14. 見積り

推奨案のままなら、純粋関数 (スライドの projection とテスト) が中心で、所有側は分岐 1 か所と設定の採用経路の引数追加。
Codex の実装 1〜2 日 + 独立レビュー + 回帰・解析器テスト・描画スナップショット・文書 + `build-dev.ps1` の確認用ビルドで、
**v5.7.0 に間に合う見込み**。採用の判断には利用者の実機での動きの確認 (§12 の後、統括経由) を含める。Q2・Q1 の案 B を選ぶと別規模になる。

## 15. レビューで特に見てほしい点

1. style を受理時に写し、変更は取消とする扱いで、演出中に種類が混ざる状態が本当に無いか (`AcceptedOffer` の生成・Offered → Turning・Completing の各経路)。
2. 移動量の式 (§4.3) で、指定サイズのはみ出し・大きさ違い・見開きの Vacant を含む単位でも、旧と新が重ならず、旧の可視面積が終端で 0 へ収束し、`p = 1` では incoming だけになるか。
3. 共有する `preflight` / `canonical_unit` / `stage_one_shape_reason` にめくり専用の前提 (綴じ目・表紙側の証明) が混ざっていて、スライドには不要な拒否になっていないか。拒否が過剰でも今回は許容するが、誤って受理する形がないか。
4. receipt / SubmissionKind の `curl` の名前を共有することで、診断・perf 解析器 (`scripts/analyze_perf.py:380` の `page_turn_animation` 集計、`first_accepted_curl` 等) の集計がめくりとスライドを区別できなくならないか。解析器は style 別に分けて集計する想定。
5. detached の判断 (§8)。

## 16. レビューの記録

- r1 (2026-11-23、GPT-6.1 Sol / xhigh、`468cc14b0`): **REVISE**。P1 見開きで初期可視条件を満たせない (設計書が「各 incoming」と書いていたが
  `meaningful` は「どれか 1 面」、かつ入る側が白紙の相方だと開始できない) → §4.3 をめくりと同じ扱い (単ページ必須・見開きは開始) に改訂。
  P2 旧版へ戻したときの説明が DB の版の保護と矛盾 → §7.2 を訂正、未知値・旧形式のテストを追加。P2 perf の style の寿命 → §3.1 に
  受理時の値を tracker / CompletionDiagnostics まで運ぶ経路と解析器の集計を追加。P3 「着地直前に旧が clip の外」は式と合わない →
  「`p → 1` で 0 へ収束、`p = 1` は保存済み配置を直接描く」に統一し、終端の描画比較を受入条件に追加。P3 入力待ちの説明が barrier と
  不正確 → §6.1 を「既存の 1 件 continuation と flat barrier を維持」に訂正し、§11 に回帰を追加。
  確認された点: 所有の基本方針、非重複の式、ROI の共用 (不完全 Native ROI は `canonical_page_material` `PT:3858` が排除)、
  設定取消と兄弟 context、detached の判断、Q1〜Q4 の推奨。
- r2 (2026-11-23、同セッション、`a24e36b73`): **REVISE**。r1 の 5 件は解消と確認。P2 終端の「通常 paint と同じ頂点・UV」は横長の 2 面分割・
  CPU 側 clip と両立しない → §4.3・§11 を「純粋関数の `p = 1` は incoming だけで描画結果が等価、所有側は `PT:3167` の通常 paint で着地」に改訂。
  P3 初回可視と着地直前の旧条件が §13・§15 に残っていた → 統一。確認された点: 見開きの開始の扱い (時計は可視面積と独立に開始 `PT:677`)、
  終端の方針、perf の style を `SubmissionTracker` に持たせる方針、設定互換・continuation の訂正。
- r3 (2026-11-23、同セッション、`47b4acc06`): **ACCEPT**。残る指摘なし。利用者の判断 (§13) と実装後の検証は別途。
- r4 (2026-11-23、同セッション、`3d2a16f2a`、利用者の判断を受けた §7.3 の差分): **ACCEPT**。P3 1 件 (ナビゲータ位置は選択後 root へ戻るので「とどまる」の例から外す) を訂正。確認された点: キーを ON/OFF のまま保ち新 KeyAction を足さない判断、保存・取消・複数ウィンドウの経路の共用、Remote 側の対応不要、スナップショット一覧と 4 行の高さ・handler 試験。

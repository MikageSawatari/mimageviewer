# フルスクリーン左右パネルの表示モード (§1.15)

バックログ: `docs/next-release-backlog.md` §1.15 (mImageViewer 専用スレ 47)。

フルスクリーンの左右パネルが「画面端ホバーで勝手に出る / 出た後に消えにくい /
編集パネルが出ること自体が怖い」という使いにくさへの対処。上 HUD の `i` ボタンで
**2 モード (通常ホバー / クリック表示)** を切り替え、パネルの召喚方法をユーザーが選べるようにする。
**静止画・動画・音楽の 3 面すべてを同じ挙動に統一する** (ユーザー要望 2026-07-12、操作感を揃える)。

> 設計変遷: 当初 3 状態 (Hover/Pinned/ClickToShow) 案 → ClickToShow を「明示クローズまで維持 +
> 右情報パネルの開状態を永続化」にしたことで Pinned が冗長になり、**2 モードへ簡素化** (決定 C)。
> さらにスコープを静止画のみ → **3 面統一** に拡張 (§2.5)。

---

## 1. 現状分析 (実装のもつれ)

### 1.1 現在の召喚経路 (静止画フルスクリーン)

`src/ui_fullscreen.rs` に **2 系統**の召喚ロジックが重なっている。

**(A) `adjustment_mode` = 端ホバーの「オーバーレイ」** (`ui_fullscreen.rs:11497-11561`)

- カーソルが **上端 (< 60px) / 左端 / 右端** (`panel_edge_trigger_px` ≒ 幅 5%) のいずれかに
  入ると `self.adjustment_mode = true` にする (paged 読み・`fullscreen_idx` 有りが条件)。
- `adjustment_mode` が真だと描画側 (`:7807` `if adjustment_active`) が
  **左パネル (補正/編集) + 右パネル (`draw_metadata_panel_forced`) を同時表示**する。
- さらに上バー (`draw_fs_hover_bar`) の可視ゲート (`:16977`) も `!*adjustment_mode` を含むため、
  `adjustment_mode` が立つと**上バーも強制表示**される。
- 結果: **右端にカーソルを寄せただけで、左の編集パネル＋右の情報パネル＋上バーが一斉に出る**。
  これが「編集パネルが出ること自体が怖い」の実体。

**(B) `show_metadata_panel` / `metadata_panel_hover_active` = 右パネル単独** (`ui_metadata_panel.rs:96-108`)

- 右端の細いストリップ (`metadata_panel_hover_activation_rect`) ホバーで
  `metadata_panel_hover_active` をラッチ → 右パネルのみ表示 (sustain 付き)。
- `show_metadata_panel` は**ピン (常時表示)**。`i` キー / Tab / 右パネルの 📌 ボタン /
  ゲームパッド `ImageToggleMetadata` でトグル。
- `show_metadata_panel` は **App のランタイム状態** (`app.rs:6805`) で、`open_fullscreen` の
  たびに false にリセットされる = **セッション毎に消える** (ユーザーの「ピンしても次で戻る」不満源)。

(A) の端ホバーは右端も含むため、実運用では右端で (A) が勝ち「全部出る」。(B) は
`adjustment_mode` が false のときだけ右パネル単独で効く。

### 1.2 上バーの可視ゲート (`ui_fullscreen.rs:16969-16986`)

上バーは `hover_in_top` (上端ホバー) *または* `adjustment_mode` / `view_trim_mode` /
各種 popup で表示。**上端ホバー自体は独立**しているので、(A) の結合を切っても
上バーは上端ホバーで従来どおり出せる。

### 1.3 スコープ確認 (静止画)

- 消しゴム / 補正レイヤー / 隠蔽 / 表示トリム / 音楽ビュー / 360 / 比較 wipe / ズーム中は
  端ホバー召喚を既に抑止済み (`:11502-11531`)。本改修はこの抑止群を壊さない。
- 右パネルのレイアウト予約 (`has_right_panel`, `:11431` `:11958`) はピン時だけ効く。

### 1.4 動画 / 音楽ビューの現状 (別経路・`i` ボタン無し)

**ユーザー要望 (2026-07-12): 動画・音楽も静止画と同じ 2 モードに統一する** (操作感を揃える)。
動画・音楽は左右パネルの実装が静止画と別経路で、**`i` ボタンも pin も無い** (端ホバーのみ)。

**動画 (native presenter, `src/video/native_presenter/`)**:
- 左パネル = **ジャンプ / ブックマーク** (`jump_panel_visible()`, 左端ホバー)。
- 右パネル = **メタデータ / タグ** (`right_panel_visible()`, 右端ホバー)。
- `update_side_panel_hover_latches()` (`mod.rs:5687`) が二段ラッチを更新。可視判定は純関数
  `native_right_panel_visible_from_inputs` / `native_jump_panel_visible_from_inputs`。
  **既に左右分離済み** (左端=jump / 右端=meta、静止画のような結合は無い)。pin/`i` は無い。
- 上バー = `draw_native_top_bar` (`overlay_draw.rs:2680`)。ボタンは右詰めで
  `draw_native_top_button(&mut x, …, NativeTopButtonGlyph::*, NativeOverlayCommand::*)` の infra
  があり (Close / WindowToggle / AudioMode / TileGrid / PerfGraph / Vst3)。**`i` を足す余地あり**。
- HUD mouse 入力は HUD HWND の region (`compute_hud_regions`) 経由。新規クリック領域
  (呼び出しバー) は region に含める必要がある。

**音楽ビュー (egui, `ui_fullscreen.rs::draw_fs_music_view:21715` + `ui_music_panels.rs`)**:
- 左パネル = ジャンプ / ブックマーク (`music_left_panel_active`, 左端ホバー、`draw_native_jump_panel_body` 共有)。
- 右パネル = タグ / メタ (`music_right_panel_active`, 右端ホバー、`draw_fs_music_right_panel`)。
- 二段ラッチは `:21897-21906`。**既に左右分離済み**。pin/`i` は無い。
- 上バー = 54px (`:21804`)、下 HUD = `MUSIC_HUD_HEIGHT`。上バーに Close / Window / VST ボタン。
  **`i` を足す余地あり**。

→ 静止画で「左端=編集/右端=情報/上端=上バー」に分離すれば、**3 面すべて左右分離モデルで揃う**。
あとは 3 面共通の 2 モード設定と `i` ボタンを配れば統一できる。

---

## 2. ターゲットモデル (2 モード)

**決定 (2026-07-12): Pinned を廃止し 2 モードにする。** ClickToShow で開いた右情報パネルを
「画像切替 + フルスクリーン再入場をまたいで維持 (永続トグル)」にすることで、旧 Pinned の
「情報を常に見たい」需要を吸収する。モードが 2 つになりモデルが単純化する。

### 2.1 永続設定 (settings.rs、2 フィールド)

```rust
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FsSidePanelMode {
    #[default]
    Hover,        // 通常ホバー: 端ホバーでパネル召喚 (transient)
    ClickToShow,  // クリック表示: 端ホバーで開かない・呼び出しバークリックで開く
    #[serde(other)]
    Unknown,
}
```

1. `Settings.fullscreen_side_panel_mode: FsSidePanelMode` — 既定 `Hover` (= 現行挙動)。
   `label()` / `all()` / `normalized()`(`Unknown → Hover`) / `toggled()`(Hover↔ClickToShow) を実装。
2. `Settings.fullscreen_click_info_open: bool` — 既定 `false`。**ClickToShow モードの右情報
   パネルの開状態**を永続化する。ClickToShow 中の右呼び出しバー / パネルヘッダ ×(📌) で
   トグルし、画像切替 + 再入場をまたいで維持する。Hover モードでは参照しない
   (Hover では右端ホバーで transient 表示)。

- **どちらも新規フィールド + 既定値ありなので migration 不要** (旧 settings.db は default で
  読める / 後方互換)。
- **現行 `show_metadata_panel` (App ランタイム・セッション毎 false リセット) は廃止/置換**。
  右情報パネルの「常時表示」は `mode==ClickToShow && fullscreen_click_info_open` から導出する
  helper (`metadata_panel_persistently_shown()`) にまとめる。これで「ピンが毎回戻る」不満も解消。

### 2.2 各モードの挙動

| モード | 左パネル (補正/編集) | 右パネル (情報) | 上バー |
| --- | --- | --- | --- |
| **Hover** | 左端ホバーで表示 (transient) | 右端ホバーで表示 (transient) | 上端ホバー |
| **ClickToShow** | 左呼び出しバー `▶` クリックで表示 | 右呼び出しバー `◀` クリックで表示 (**開状態は永続**) | 上端ホバー (不変) |

- **決定 A = 辺ごとに分離 (確定)**。Hover モードでも 3 辺を分離する:
  - 左端ホバー → 左パネル (補正/編集 = `adjustment_mode`) **のみ**。右パネルは連動させない。
  - 右端ホバー → 右パネル (情報) **のみ** (既存 `metadata_panel_hover_active` 経路)。
  - 上端ホバー → 上バーのみ。
  - これで「右に寄っただけで編集パネルが出る」誤爆が Hover モードでも解消する。
- **上バーは両モードで上端ホバー表示のまま** (× / ナビ / 見開き / フィット等の主要操作面。
  「怖いパネル」ではないので click-to-show にしない)。
- **右情報パネルの永続開状態は ClickToShow モード限定**。Hover モードでは右端ホバーの
  transient 表示のみで、「常時表示」は無い (= 常時表示が欲しい人は ClickToShow を選ぶ)。

### 2.3 ClickToShow の呼び出しバーと開状態の永続

- 画面**左右の最端 ≒ 2%** (`panel_edge_trigger_px` より狭い専用定数 `PANEL_CALLOUT_HIT_PX`) に
  カーソルが入ったときだけ、縦長の細いバー (幅 ~20px) をフェード表示する。
  - 左バー: `▶` (内向き矢印) → クリックで左パネル (補正/編集 = `adjustment_mode`) を開く。
  - 右バー: `◀` (内向き矢印) → クリックで右パネル (メタデータ) を開く。
- **完全 OFF にはしない**。バーは常に呼び出せるので「機能が消えた」印象を与えない。
- **決定 B = 明示クローズまで維持 (確定)**。バー / × / Esc で明示的に閉じるまでパネルは残る
  (マウス離脱では閉じない)。開いている間はバーを「閉じる」アフォーダンス (`◀`↔`▶` 反転 or `×`)
  に変える。
- 呼び出しバーはパネルより高い固定 order で描画する。同じ order の `egui::Area` はクリックで
  前面へ移動するため、パネル操作後も左右バーが背面へ回らないレイヤ分離を不変条件とする。
- 3 面の左右パネルには ClickToShow 時だけヘッダ × を表示する。静止画左は
  `画像補正 / 表示トリム` タブ幅を縮め、その右に painter の線 2 本で × を描く。
- **右情報パネル (◀) の開状態は永続** (`fullscreen_click_info_open`)。画像切替でも維持し、
  フルスクリーンを抜けて再入場しても復元する (= 旧 Pinned の吸収。クリック 1 回で以後記憶)。
- **左編集パネル (▶) の開状態はセッション限定**。画像切替はまたいで維持するが、フルスクリーンを
  抜けると閉じる (`adjustment_mode` の現行ライフサイクルに準拠。編集はタスクなので常時 ON にしない)。

### 2.4 `i` ボタン / キー = モードトグル

- `i` ボタン / `I` キー / Tab / ゲームパッド `ImageToggleMetadata` は **モードトグル
  (Hover ↔ ClickToShow)** にする (`cycle_fs_side_panel_mode` → 2 状態なので実質トグル)。
  永続。変更時にフィードバックトースト (`パネル表示: クリック表示` 等) を出す。
- `I` / Tab は 1 物理押下につき 1 回だけ切り替える。egui 経路も autorepeat を受理しない。
- キー処理の所有者は、静止画・音楽・動画→音声モード = egui、通常動画 = native presenter の
  どちらか一方だけとする。hidden presenter からイベントが届く過渡状態でも二重発火させない。
- アイコン: Hover = 通常色の `i` (`draw_info_icon`)。ClickToShow = `i` の右下に小さいマウス
  カーソルを重ねた青系アイコン (バックログ第一候補、`draw_icons.rs` に glyph 追加。視認性が
  悪ければ tooltip 補足)。tooltip は現在 + 次状態を明示。
- **注意 (muscle memory)**: 現行 `i` は「右情報パネルの表示トグル」。本改修で `i` は
  「召喚モードの切替」に意味が変わる。ClickToShow 中の右情報パネル開閉は `◀` バー /
  パネルヘッダ × で行う。この操作既定の変更は version_highlights (must_read) で告知する。

### 2.5 3 面統一 (静止画 / 動画 / 音楽) ⚠ スコープ本体

**ユーザー要望で 3 面すべてを同じ 2 モードに統一する** (最初から統一した操作感)。
`FsSidePanelMode` と `fullscreen_click_info_open` は **1 つの Settings** に置き、3 面が共有する。

| 面 | 左パネル | 右パネル | `i` ボタン | モードの読み方 |
| --- | --- | --- | --- | --- |
| 静止画 (egui) | 補正/編集 (session) | メタデータ (**永続**) | 既存 `fs_info_btn` を転用 | `settings` 直読み |
| 動画 (native presenter) | ジャンプ/BM (session) | メタ/タグ (**永続**) | **新規追加** (上バー) | presenter へ sync |
| 音楽 (egui) | ジャンプ/BM (session) | タグ/メタ (**永続**) | **新規追加** (上バー) | `settings` 直読み |

共通ルール (3 面同一):
- **Hover モード**: 左端ホバー→左パネル / 右端ホバー→右パネル / 上端ホバー→上バー (分離)。
  動画・音楽は既に分離済みなので Hover は現状維持。静止画のみ分離が新規 (決定 A)。
- **ClickToShow モード**: 端ホバーで開かない。左 `▶` / 右 `◀` 呼び出しバーのクリックで開く。
  - **右パネル (◀) = 永続** (`fullscreen_click_info_open`、3 面共通の 1 bool)。ON なら
    どの面でも ClickToShow 中に右パネルを表示 (面ごとに中身は違うが概念は「右の情報パネル」)。
  - **左パネル (▶) = セッション限定** (面ごとのランタイムフラグ。静止画=`adjustment_mode`、
    動画=presenter ランタイム flag、音楽=`music_left_click_open` 等)。フルスクリーン退場でリセット。
- **`i` ボタン = モードトグル** (Hover↔ClickToShow)。3 面すべての上バーに置き、同じ設定を切り替える。
  アイコンも 3 面で揃える (Hover=通常 `i` / ClickToShow=`i`+小マウスカーソル)。
- 動画↔音声モードの遷移は左パネルの session 境界として扱い、動画 presenter の
  `left_session_open` と音楽ビューの `music_left_click_open` を両方リセットする。
  右の `fullscreen_click_info_open` は遷移をまたいで保持する。

動画への伝搬 (native presenter は別スレッド):
- `settings.fullscreen_side_panel_mode` / `fullscreen_click_info_open` を presenter へ **sync**
  (既存の overlay メタデータ同期経路 = `app/native_video.rs::sync_native_video_metadata` に相乗り)。
- 上バーの `i` クリック → `NativeOverlayCommand::ToggleSidePanelMode` を App へ → App が
  `settings` をトグル → 次 sync で presenter へ反映。右 `◀` バー → `ToggleClickInfoOpen`、
  左 `▶` バー → presenter ローカルの session open flag をトグル (App 往復不要)。
- presenter の `right_panel_visible()` / `jump_panel_visible()` 純関数に mode / click_info_open /
  left_session_open を入力追加。callout バーの rect を `compute_hud_regions` に含める。

---

## 3. 触る箇所 (実装マップ)

**3 面統一 (静止画 + 動画 + 音楽)**。項目 1-9 = 静止画 + 共通設定、10 = 動画、11 = 音楽。
detached viewer 固有コードには触れない (凍結ルール)。動画 HUD の `i` / callout は共有 overlay に
足すので detached 動画にも自然に出るが、detached 固有の分岐は増やさない (fullscreen / main で先に検証)。

1. **`src/settings.rs`** (3 面共通): `FsSidePanelMode { Hover, ClickToShow }` enum
   (`label/all/normalized/toggled`) + `Settings.fullscreen_side_panel_mode` +
   `Settings.fullscreen_click_info_open: bool`。
2. **`src/ui_fullscreen.rs`**:
   - `:11497-11561` の端ホバー→`adjustment_mode` を **モード分岐**へ (決定 A = 分離)。
     - Hover: **左端のみ** `adjustment_mode` を立てる (右端・上端では立てない)。
       右端は §1.1 (B) の `metadata_panel_hover_active` 経路に委ね、上端は上バー自経路。
     - ClickToShow: 端ホバーで `adjustment_mode` を立てない。呼び出しバー処理を追加。
       左 `▶` クリックで `adjustment_mode=true`、右 `◀` クリックで `fullscreen_click_info_open` トグル。
   - `:7807` `adjustment_active` 分岐: 決定 A = 分離につき、左パネル描画時の
     `draw_metadata_panel_forced` (右パネル強制) を**やめる**。右は自経路
     (`draw_metadata_panel`: 右端ホバー[Hover] / 永続開状態[ClickToShow]) に委ねる。
   - `:16977` 上バー可視ゲート: `adjustment_mode` 結合を見直し (上端ホバーは維持)。
   - `has_right_panel` (`:11431` `:11958`): 「実効的に右パネル表示中」を
     `metadata_panel_persistently_shown()` (= `mode==ClickToShow && fullscreen_click_info_open`)
     で算出。Hover の transient 表示は従来どおり別扱い。
   - ClickToShow 呼び出しバーの矩形計算・描画・クリック判定 helper を新設
     (`callout_bar_rect_left/right`, `callout_hit`, 純関数でテスト可能に)。
3. **`src/ui_metadata_panel.rs`**: `show_metadata_panel` 直接参照を helper
   `self.metadata_panel_persistently_shown()` に置換 (`:96-108` の hover ラッチ /
   `:178-216` のヘッダ pin ボタン)。ヘッダ ×(📌) は ClickToShow の
   `fullscreen_click_info_open` を落とす (Hover では非表示 or 無効)。
4. **`src/ui_fullscreen/draw_icons.rs`**: `i` ボタンの ClickToShow バリアント (青 + 小マウス
   カーソル)。`draw_info_icon` に variant 追加 or overlay helper。
5. **`src/ui_fullscreen.rs` `:17418` の `i` ボタン**と **`:10894` の `I` キー**、
   **`gamepad_input.rs:4474` の `ImageToggleMetadata`**: `show_metadata_panel` トグルを
   **モードトグル (`cycle_fs_side_panel_mode`, Hover↔ClickToShow)** に統一。トースト表示。
6. **`open_fullscreen` の `show_metadata_panel = false` リセット** (`app.rs:27989` 等):
   モード・`fullscreen_click_info_open` は永続なので**リセットしない**。左編集パネル
   (`adjustment_mode`) は現行どおりフルスクリーン退場でリセット (セッション限定)。
7. **環境設定 (任意)**: `ui_dialogs/preferences/pages.rs` の表示系ページに
   `FsSidePanelMode` セレクタを追加 (主導線は `i` ボタンだが、設定からも変えられると親切)。
8. **ドキュメント**: `htdocs/mimageviewer/manual/fullscreen.html` に 2 モードの説明。
   `docs/display-pipeline.md` §2.2.2 付近に召喚モデルを追記。
9. **version_highlights**: 決定 A (右端ホバーで左編集パネルが出なくなる) と `i` の意味変更
   (右情報トグル → モードトグル) の両方が **操作既定の変更**なので `must_read` エントリを
   追加。加えて 2 モード + クリック表示の追加を `highlights` に 1 行。
   `cargo test --lib version_highlights::` でパース確認。

10. **動画 (native presenter)**:
    - `src/video/native_presenter/mod.rs`: presenter overlay state に `side_panel_mode` /
      `click_info_open` / `left_session_open` フィールド追加。`update_side_panel_hover_latches`
      (`:5687`) を **モード分岐** — ClickToShow では端ホバーでラッチを立てない。
      `right_panel_visible()` (`:5717`) / `jump_panel_visible()` (`:5738`) 純関数
      (`native_right_panel_visible_from_inputs` / `native_jump_panel_visible_from_inputs`) に
      mode / click_info_open / left_session_open を入力追加。callout バーの rect を
      `compute_hud_regions` に含める (HUD HWND の mouse region)。
    - `src/video/native_presenter/overlay_draw.rs`: `draw_native_top_bar` (`:2680`) に
      `i` ボタン (`NativeTopButtonGlyph::Info` 新設、Hover/ClickToShow で glyph 差分) を追加。
      左 `▶` / 右 `◀` の callout バー描画 helper を新設。
    - `NativeTopButtonGlyph` / `NativeOverlayCommand` に `Info` / `ToggleSidePanelMode` /
      `ToggleClickInfoOpen` を追加。左 `▶` は presenter ローカルの `left_session_open` トグル。
    - `src/app/native_video.rs`: `sync_native_video_metadata` で `settings` の mode /
      click_info_open を presenter へ流す。`NativeOverlayCommand::ToggleSidePanelMode` /
      `ToggleClickInfoOpen` を App 側でハンドルして `settings` を更新 (次 sync で反映)。
      入場時の presenter 生成/再生成でも同期。
11. **音楽ビュー (egui)**:
    - `src/ui_fullscreen.rs::draw_fs_music_view` (`:21715`): 左右パネルの二段ラッチ
      (`:21897-21906`) を **モード分岐**。ClickToShow では端ホバーで開かず、callout バー
      (左 `▶` / 右 `◀`) のクリックで開く。右 = `fullscreen_click_info_open` (永続)、
      左 = `music_left_click_open` 等のセッションフラグ。上バー (54px, `:21804`) に `i` ボタン
      (`settings` のモードトグル) を追加。
    - `src/ui_music_panels.rs`: 必要なら callout バー描画 helper を共有。右パネル
      (`draw_fs_music_right_panel`) の可視条件を helper へ寄せる。

---

## 4. 決定事項 (確定済み 2026-07-12)

### 決定 A — 「通常ホバー」モードの召喚範囲 → **A-1 (辺ごとに分離) で確定**

左端ホバー=補正/編集パネル、右端ホバー=情報パネル、上端ホバー=上バー、に分離する。
「右に行っただけで編集パネルが出る」誤爆を Hover モードでも解消。バックログの Hover
記述「左右端ホバーでパネル表示」とも一致。**既定挙動の変更**なので version_highlights
(must_read) を追加し、回帰は実機目視で確認する。

### 決定 B — 「クリック表示」で開いたパネルの閉じ方 → **B-1 (明示クローズまで維持) で確定**

呼び出しバー / パネルヘッダ × / Esc で明示的に閉じるまで残る。情報を見ながら画像を触れる・
編集に集中できる。マウス離脱では閉じない (`i` はモードトグルに変わったので閉じる操作には使わない)。

### 決定 C — Pinned/ClickToShow の重複解消 → **Pinned 廃止・2 モード化で確定**

ClickToShow の右情報パネル開状態を永続化 (`fullscreen_click_info_open`) して旧 Pinned の
「情報常時表示」を吸収する。モードは Hover / ClickToShow の 2 つ。`i` は 2 モードのトグル。
右情報パネルの永続表示は ClickToShow 限定 (Hover は transient 表示のみ)。左編集パネルは
セッション限定 (永続しない)。

---

## 5. スコープ / 段階

- **対象 = 静止画 + 動画 + 音楽の 3 面すべて** (ユーザー要望: 最初から統一した操作感)。
  静止画 = 通常画像 / ZIP 内画像 / PDF ページ。動画 = native presenter。音楽 = 音声モード /
  音声ファイルの音楽ビュー。
- **実装順 (1 ブランチ内で段階的に、各段でテスト)**:
  1. 共通設定 (`FsSidePanelMode` + `fullscreen_click_info_open`) + 純関数 helper + unit test。
  2. 静止画 (項目 2-9): 分離 + callout + `i` 転用 + 永続。ここで実機確認できる形にする。
  3. 動画 (項目 10): presenter sync + `i` 追加 + callout + region。
  4. 音楽 (項目 11): egui 分岐 + `i` 追加 + callout。
  - ユーザーは「最初から統一」を要望しているので、**3 面揃えてから実機検証 → コミット**する
    (静止画だけ先行コミットはしない)。
- detached viewer は凍結中。共有ロジックが自然に効く範囲に留め、detached 固有経路の分岐は
  足さない。動画 HUD の `i` / callout は detached 動画にも共有 overlay 経由で出るが、
  検証は fullscreen / main を主とする。

## 6. テスト

- `FsSidePanelMode` の `normalized/label/all/toggled` の純関数 unit。
- **静止画**: 端ホバー→モード別の期待状態 (Hover 分離時: 右端ホバーで `adjustment_mode` が
  立たないこと / ClickToShow: 端ホバーで左右パネルが開かないこと)。純関数化した判定
  (`callout_hit`, `edge_summons_adjustment(mode, edge)`) で discriminating に。
  `fullscreen_click_info_open=true` で `metadata_panel_persistently_shown()` が true、
  `open_fullscreen` を跨いでも維持 / 左編集は退場リセット (app::tests、`--bin mimageviewer-core`)。
- **動画**: `native_right_panel_visible_from_inputs` / `native_jump_panel_visible_from_inputs` の
  純関数テストに mode / click_info_open / left_session_open ケースを追加 (ClickToShow で
  hover latch を無視、click_info_open で右が出る、等。既存テスト `mod.rs:8389-8642` に相乗り)。
- **音楽**: 左右ラッチのモード分岐を純関数化してテスト (可能な範囲で)。
- `i` / I / パッドが Hover↔ClickToShow をトグルし `open_fullscreen` を跨いで維持されること。
- egui 0.33 の `begin_pass` が event consume より先に決める Tab focus 方向は、全 Context の
  `on_begin_pass` ポリシーで TextEdit 編集中でない場合だけ `None` へ戻すこと。その後、通常押下 /
  autorepeat を KeySlot と egui event queue の双方から consume する。ClickToShow で左右パネルが
  開いていても、静止画・音声・動画→音声モードの 3 面で Tab が 1 物理押下 1 回だけ Hover と
  往復すること。
- TextEdit / IME が `wants_keyboard_input()` を所有する間は Tab を consume しないこと。
- version_highlights テーブルのパース (`--lib version_highlights::`)。
- UI スナップショット (必要なら) は `docs/ui-snapshot-policy.md` に従う。

## 7. 実機検証

ネイティブ挙動 (フルスクリーン端ホバー・カーソル可視・動画 HUD) を変えるので、実機依頼前に
`.\scripts\build-release.ps1 -SkipVst3Bridge` で検証バイナリを用意する
(CLAUDE.md「実機検証用バイナリの準備」)。**3 面 (静止画 / 動画 / 音楽) それぞれで
Hover / ClickToShow を実機確認**してからコミット。ユーザー実機 OK 後にコミット。

## 8. Codex ブリーフ

決定 A / B / C 確定済み (2026-07-12・2 モード化)。本ドキュメント §2-§3 を根拠に
`codex exec -s workspace-write` へ実装を発注 ([[feedback_codex_does_implementation]])。
私は brief / レビュー / テスト / 統合を担当。

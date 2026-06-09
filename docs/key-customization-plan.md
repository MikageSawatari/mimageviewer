# キーカスタマイズ機能 設計・実装規模調査 (Claude 案)

> ステータス: **設計のみ / 未実装**。v1.1.0+ 候補。
> 目的: ユーザー要望「キー操作を状況ごとにカスタマイズしたい」に対して、
> 現状のキー処理実装を調査し、実装規模・実現可能性・エッジケースを評価したうえで
> 具体的な設計を提示する。

関連: [keymap-spec.md](keymap-spec.md) (現行キー仕様の正本)、
[fullscreen-navigation-consistency.md](fullscreen-navigation-consistency.md)、
[architecture-overview.md](architecture-overview.md)。

---

## 0. 結論サマリ (先に読む)

- **実現可能。ただし中規模〜大規模のリファクタを伴う。** 現状はキー判定が完全に
  「その場でインライン判定」されており、中央のキーマップテーブルが存在しない。
  カスタマイズには「アクション ID ⇄ キーの対応表」という間接層を新設し、
  全インライン判定をテーブル参照に置き換える必要がある。
- **キー操作の調査は実質完了している。** [keymap-spec.md](keymap-spec.md) が
  ほぼ完全なインベントリで、かつ「状況 (コンテキスト)」の境界は既にコード上の
  モード分岐 (`handle_fs_key_input` の early-return チェーン) として明確に分かれている。
  タブの区切りはこの分岐構造をそのまま使える。
- **3 つの独立したキー入力経路がある**ことが最大の構造的難所:
  1. egui **consume** 経路 (フルスクリーン / 各編集モード) — `i.consume_key(mods, key)`
  2. egui **key_pressed** 経路 (グリッド) — `i.key_pressed(key)` (非消費)
  3. Win32 **仮想キーコード** 経路 (native 動画プレイヤー) — `match key.virtual_key`
  この 3 経路を 1 つの抽象キー表現に束ねる必要がある。
- **「Shift 押しっぱなしでルーペ」を M キーに置き換えたいか?** → **コードは変わる。**
  ルーペは離散的ショートカットではなく「押している間だけ有効」の hold ジェスチャで、
  `i.modifiers.shift` をポーリングして実装している。これを M に変えると判定を
  `i.key_down(Key::M)` 系に切り替える必要があり、カスタマイズ基盤側に
  「押下 (press)」とは別に「保持 (hold)」というバインド種別を設ける必要がある。
- **推奨は段階導入。** まず「離散アクションキー (文字/数字/F キーのトグル系)」だけを
  コンテキスト単位でカスタマイズ可能にし、矢印ナビ・Esc/Enter・hold ジェスチャ・
  IME 絡みは初期は固定にする。これでリスクの高いエッジケースを避けつつ需要の大半
  (M=ルーペ、各モードのツールキー再割当など) を満たせる。

---

## 1. 現状調査: キー判定はどこでどう行われているか

### 1.1 中央レジストリは存在しない

キーマップ / キーバインドの設定テーブルやレジストリは**一切ない**。
すべてのキーは利用箇所で直接 `ctx.input()` / `ctx.input_mut()` を呼んで判定している。
件数 (grep `key_pressed|key_down|consume_key`):

| ファイル | 件数 | 役割 |
|---|---:|---|
| `src/ui_fullscreen.rs` | 135 | 画像フルスクリーン + モード分岐の親 |
| `src/app.rs` | 47 | グリッドビュー (`handle_keyboard`) + グローバル |
| `src/ui_erase.rs` | 34 | 消しゴムモード |
| `src/ui_conceal.rs` | 30 | 隠蔽加工モード |
| `src/ui_text.rs` | 7 | テキスト注釈モード (漫画統合) |
| `src/undo_ops.rs` | 4 | メタ Undo/Redo |
| `src/ui_main.rs` | 3 | 検索バー Enter |
| `src/ui_dialogs/context_menu.rs` | 3 | コンテキストメニュー Esc |
| `src/ui_crop.rs` | 2 | 切り取りモード |
| その他 | 数件 | global_search_ui / native_presenter overlay |

加えて grep に出てこない**第 3 経路**がある:
`src/app/native_video.rs::handle_native_video_key_event` (4531 行〜) は
`match key.virtual_key { 0x70..=0x75 if !key.ctrl && !key.repeat => ... }` のように
**Win32 仮想キーコード**で約 50 箇所判定している (egui の `Key` enum を使わない)。

### 1.2 3 つの入力経路の詳細

**(A) egui consume 経路 — フルスクリーン / 編集モード**

```rust
// src/ui_fullscreen.rs:3858 付近 (例)
let key_s = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::S));
let key_r = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::R));
// ... 各キーの結果をローカル bool に詰めて、後段の if key_s {...} で実行
```

- `consume_key(mods, key)` は**修飾キー完全一致 + イベント消費**。
- 修飾キー違いを複数受ける場合は**特異度の高い順**に書く必要がある (egui の
  `matches_logically` が `Modifiers::NONE` を修飾付きイベントにもマッチさせるため):

```rust
// src/ui_fullscreen.rs:4050 付近 — Alt+T → Shift+T → 素の T の順で消費
let key_t_alt   = ctx.input_mut(|i| i.consume_key(egui::Modifiers::ALT,   egui::Key::T));
let key_t_shift = ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::T));
let key_t       = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE,  egui::Key::T));
```

- 1 アクションに複数チョードを割り当てる例も既にある:
  `arrow_right` は `NONE` と `SHIFT` の両方を受ける (ui_fullscreen.rs:3801)、
  `key_i` は `I` か `Tab` を受ける (3823)。

**(B) egui key_pressed 経路 — グリッド**

```rust
// src/app.rs:13455 付近 — 1 回の input() で全部まとめて読む (非消費)
let (right, left, down, up, enter, bs, ctrl_up, ctrl_down, ...) = ctx.input(|i| (
    i.key_pressed(egui::Key::ArrowRight),
    i.key_pressed(egui::Key::ArrowLeft),
    ...
    i.modifiers.ctrl && i.key_pressed(egui::Key::ArrowUp),
    ...
));
```

- `key_pressed` は**非消費**。修飾は `i.modifiers.ctrl && ...` と**緩く**組み合わせる
  (完全一致ではない。例えば `alt_up = alt_held && up && !ctrl_held` と手書きで排他を作る)。

**(C) Win32 仮想キー経路 — native 動画プレイヤー**

```rust
// src/app/native_video.rs:4549 付近
match key.virtual_key {
    0x70..=0x75 if !key.ctrl && !key.repeat => { /* F1-F6 レーティング */ }
    0x0D if key.shift && !key.ctrl && !key.repeat => { /* Shift+Enter 外部プレイヤー */ }
    0x4D if !key.shift && !key.ctrl && !key.repeat => { /* M ミュート */ }
    0x25 => { let delta = if key.ctrl {-30.0} else if key.shift {-1.0} else {-5.0}; ... }
    ...
}
```

- native 動画は別の HWND スレッドからイベントが来るため egui の input を通らない。
  `NativeVideoKeyEvent { virtual_key, shift, ctrl, alt, repeat }` を受けて生 VK で判定。
- `if !key.repeat` でキーリピート抑止、`if !key.ctrl` 等で修飾排他を手書き。

**(D) 特殊経路 — 生 Event::Key マッチ**

- F11 (ウィンドウ⇔全画面) は `egui::Event::Key {...}` を直接マッチ (ui_fullscreen.rs:3998 付近、
  `matches_logically` 回避のため)。
- パイプラインデバッグ (Ctrl+Alt+Shift+D) は `i.events.retain(...)` で生イベントを消費
  (pipeline_debug.rs:104)。

### 1.3 hold ジェスチャ (押しっぱなし) — ルーペ

```rust
// src/ui_fullscreen.rs:7340 付近
let (hover, shift_held, focused) = ctx.input(|i| {
    (i.pointer.hover_pos(), i.modifiers.shift, i.viewport().focused.unwrap_or(true))
});
if !self.fs_loupe_locked && !shift_held { return; }   // M でロック、または Shift 保持中だけ描画
```

- ルーペは「Shift を**保持している間**だけ表示」+「M で**ロックトグル**」の二系統。
- Shift 保持の判定は `i.modifiers.shift` (毎フレームのポーリング)。
- フォーカスが overlay に奪われたケース用に OS 直読み `shift_held_via_os()`
  (`GetAsyncKeyState(VK_SHIFT)`、ui_fullscreen.rs:247 付近) のフォールバックもある。
- 同種の hold は他に「Space + 左ドラッグで一時パン」(消しゴム / 隠蔽モード) がある。

**重要**: hold ジェスチャは離散ショートカットと**入力意味論が根本的に違う**。
これがカスタマイズ設計の最大の注意点 (§4.1)。

### 1.4 コンテキスト (状況) は既にコード上で分離されている

`handle_fs_key_input` (ui_fullscreen.rs:3565) は冒頭で**モードごとに early-return**する:

```text
erase_mode        → handle_erase_keys()        (消しゴム専用キーのみ有効)
text_mode         → handle_text_keys()         (テキスト注釈専用)
conceal_mode      → handle_conceal_keys()      (隠蔽加工専用)
local_adjust_mode → インラインブロック         (補正レイヤー専用)
export_crop_mode  → handle_export_crop_keys()  (切り取り専用)
(動画アイテム)     → handle_video_input()       ※画像キーより先に走る (重なり注意)
(それ以外)         → 画像フルスクリーン共通キー
```

グリッドは別関数 `handle_keyboard` (app.rs:13353)。
**この early-return チェーンが、ユーザー要望の「状況ごとのタブ」の自然な境界そのもの。**
コンテキストの調査・分割は新規に行う必要がなく、既存構造を写経すればよい。

---

## 2. ユーザー要望への直接回答

> 「現状その場で判定している実装だと思うので、まずはこのキー操作の調査を
> うまく行う必要がありそうでしょうか？」

**はい、その認識は正しい。** ただし朗報が 2 つ:
1. 仕様インベントリは [keymap-spec.md](keymap-spec.md) として既に存在する。
2. コンテキスト境界は既存のモード分岐に明示されている。
よって「調査」はこのドキュメントでほぼ完了しており、残りは
「インラインの各判定を、対応するアクション ID に機械的に紐付ける」棚卸し作業。

> 「状況ごとのタブをどのように区切ってわけるのか」

§1.4 の分岐構造に沿って **9 コンテキスト**を提案 (§3.2)。

> 「Shift を押しながらルーペを、通常の M キーに置き換えたらコードも変わるか?」

**変わる。** 理由は §1.3 の通りルーペが hold ジェスチャだから。
- 現状: ルーペ描画コードが `i.modifiers.shift` を直接読む。
- 変更後: そこを `keymap.is_held(FsImage, LoupeHold, i)` のような問い合わせに置換し、
  バインドが「Shift 修飾」なら `i.modifiers.shift`、「M キー」なら `i.key_down(Key::M)` を
  内部で見分ける。つまり**カスタマイズ基盤に「hold バインド」という種別**が要る。
- ちなみに M は既にルーペ**ロックのトグル**に使われているので、「M 長押しでルーペ表示 +
  M 単押しでロック」を両立させるか、どちらかに寄せるかという**仕様判断**も発生する。

> 「装飾キーの扱いが難しそう」

その通り。修飾キーは §4 で詳述。要点は (a) 完全一致 vs 緩い一致の差、
(b) 特異度順消費、(c) 修飾キー単独 (key なし) チョードの表現、(d) hold との交差。

---

## 3. 設計

### 3.1 中核データモデル

```rust
/// 物理キーの抽象。egui::Key と Win32 VK の両方へ相互変換できる自前 enum。
/// 3 経路 (consume / key_pressed / VK) を 1 表現に束ねるための要。
enum KeyCode { A, B, ... Z, Num0..Num9, F1..F12, Arrow{Up,Down,Left,Right},
               Home, End, PageUp, PageDown, Space, Enter, Esc, Tab, Backspace,
               Delete, OpenBracket, CloseBracket, /* ... */ }
impl KeyCode {
    fn from_egui(k: egui::Key) -> Option<Self>;
    fn to_egui(self) -> Option<egui::Key>;
    fn from_vk(vk: u32) -> Option<Self>;   // native 動画経路
    fn to_vk(self) -> Option<u32>;
}

/// 1 つのキー組み合わせ。key=None は「修飾キー単独」(Shift hold 等)。
struct Chord { key: Option<KeyCode>, ctrl: bool, shift: bool, alt: bool }

/// バインドの発火種別。
enum Trigger { Press, Hold }   // Press=離散ショートカット, Hold=押しっぱなし

/// 論理アクション。コンテキストごとに 1 enum でもよいし、全体で 1 enum +
/// context フィールドでもよい。提案は「全体 1 enum」(衝突検査が楽)。
enum ActionId {
    // Grid
    GridCursorRight, GridCursorLeft, GridOpen, GridParent, GridRate(/*1..5,0*/), ...
    // FsImage
    FsNextImage, FsPrevImage, FsRotateCw, FsRotateCcw, FsSlideshow, FsLoupeHold,
    FsLoupeLockToggle, FsAnalysis, FsEraseMode, FsConcealMode, FsCompareToggle, ...
    // FsVideo
    VideoPlayPause, VideoMute, VideoLoop, VideoSeekFwd, VideoSeekBack, VideoTileMode, ...
    // Erase / Conceal / Crop / Text / LocalAdjust / Global
    ...
}

struct Binding {
    action: ActionId,
    context: KeyContext,
    trigger: Trigger,
    defaults: Vec<Chord>,            // 出荷時 (複数可: 例 ←/Shift+← や I/Tab)
    custom: Option<Vec<Chord>>,      // ユーザー設定 (最大 3)。None=デフォルト, Some(空)=無効化
    customizable: bool,              // false=固定 (Esc 等の予約キー)
}

struct Keymap { /* context -> action -> 解決済み Vec<Chord> */ }
```

- **解決ルール**: `effective = custom.unwrap_or(defaults)`。`Some(vec![])` は「無効化」。
- `Keymap` は純粋データなので**ユニットテスト容易** (既存 `app/tests.rs` 文化と整合)。

### 3.2 コンテキスト (タブ) 分割案

§1.4 の分岐構造に対応:

| # | KeyContext | タブ名 (UI) | 対応コード | 同時有効な他コンテキスト |
|---|---|---|---|---|
| 1 | `Grid` | グリッド | `handle_keyboard` | Global |
| 2 | `FsImage` | 画像フルスクリーン | 画像共通キー | Global, FsCommon |
| 3 | `FsVideo` | 動画フルスクリーン | `handle_native_video_key_event` / `handle_video_input` | Global, FsCommon |
| 4 | `Erase` | 消しゴム | `handle_erase_keys` | (専用、ほぼ排他) |
| 5 | `Conceal` | 隠蔽加工 | `handle_conceal_keys` | (専用) |
| 6 | `Crop` | 切り取り | `handle_export_crop_keys` | (専用) |
| 7 | `Text` | テキスト注釈 | `handle_text_keys` | (専用) |
| 8 | `LocalAdjust` | 補正レイヤー | local_adjust インライン | (専用) |
| 9 | `Global` | 全体共通 | Ctrl+F/S/G/O ほか | 常時 |

- **`FsCommon`** (Esc/Enter/I/Tab/F11/レーティング等、画像・動画共通) は別タブにせず
  FsImage / FsVideo の両方へ出すか、専用タブにするかは UI 判断 (推奨: FsCommon タブを
  別建てし、画像/動画タブには「共通キーは FsCommon を参照」と注記)。
- **重要**: 「同時有効な他コンテキスト」列が**衝突検査のスコープ**を決める (§3.4)。

### 3.3 ルックアップ API (インライン判定の置換)

経路ごとに薄いヘルパーを用意し、既存の `consume_key` / `key_pressed` /
`match vk` を 1:1 で置き換える:

```rust
// (A) consume 経路 (フルスクリーン/編集モード) ― 消費する
self.keymap.consume(ctx, KeyContext::FsImage, ActionId::FsSlideshow)   // -> bool
// (B) key_pressed 経路 (グリッド) ― 非消費
self.keymap.pressed(ctx, KeyContext::Grid, ActionId::GridOpen)         // -> bool
// (C) VK 経路 (native 動画)
self.keymap.matches_vk(KeyContext::FsVideo, ActionId::VideoMute, key)  // -> bool
// (D) hold 経路 (ルーペ等)
self.keymap.is_held(ctx, KeyContext::FsImage, ActionId::FsLoupeHold)   // -> bool
```

- `consume` は内部で「effective chord を**特異度の高い修飾から順に** `i.consume_key`」
  して egui の `matches_logically` 問題 (§4.2) を回避する。
- 置換は機械的だが**件数が多く** (≈250+50)、ここが規模の主因 (§5)。

### 3.4 衝突検査 + 「既存を無効化」フロー (ユーザー要望)

ユーザー設定でチョードを追加したとき:

1. **同一コンテキスト + 同時有効コンテキスト**内 (§3.2 の右列) で同じ Chord を持つ
   別アクションを探す。グローバルは全コンテキストで衝突候補。
2. 見つかったら警告ダイアログ: 「このキーは『○○』に割り当て済みです。
   置き換えると『○○』は無効になります。よろしいですか?」
3. OK で、衝突した既存アクションの該当 Chord を**そのアクションの custom から除去**
   (= effective から外す)。完全に空になればそのアクションは「無効」状態として UI 表示。
4. **注意**: hold バインドと press バインドが同じ Chord を共有しても物理的には共存しうる
   (例: 「M 押下=ロック」と「M 長押し=ルーペ」)。衝突検査は (key, mods) だけでなく
   trigger も考慮し、press 同士 / hold 同士のみを衝突とみなすのが自然。

### 3.5 設定 UI (新規 PreferencesPage)

- `PreferencesPage::KeyBindings` を追加 (enum: preferences.rs:28、dispatch: 〜1087、
  描画: preferences/pages.rs)。既存の 25 ページと同じ追加パターン。
- レイアウト: 上部にコンテキストタブ (§3.2)。各タブは表:

  | 操作内容 | デフォルト | カスタム① | カスタム② | カスタム③ | |
  |---|---|---|---|---|---|
  | 次の画像 | → / Shift+→ | （未設定） | | | [取消] |
  | ルーペ (長押し) | Shift | M | | | [取消] |

- **キャプチャウィジェット**: セルをクリック→「キーを押してください」→次の
  キーイベントを 1 つ捕捉して Chord 化。捕捉中はそのキーをアクションに流さない。
  IME 変換中は無視 (既存 `ime_input_active()` を流用)。Esc はキャプチャ取消に予約。
- 「デフォルトに戻す」ボタン (アクション単位 / タブ単位 / 全体)。
- 表示は `Chord -> "Ctrl+Shift+S"` の整形関数。**グリフは ASCII/Latin-1 のみ**で組む
  (CLAUDE.md「UI 文字列の Unicode グリフ選定ルール」。矢印 ↑↓ は可)。

### 3.6 永続化

- `Settings` (settings.db, SQLite) に `keymap_custom: HashMap<(KeyContext, ActionId), Vec<Chord>>`
  相当を serde で保存 (settings.rs / settings_db.rs)。
- **マイグレーション不要**: キーマップは新規機能 (未リリース)。CLAUDE.md
  「永続データ・スキーマ変更時の判断」の "未リリース" に該当。
- ただし**将来アクションが増えたときのため**、保存形式はアクション ID 文字列キーで持ち、
  起動時に「既知アクションへマージ、未知 ID は警告して破棄、欠けは defaults 補完」する
  forward-compat ロジックを最初から入れておく。

---

## 4. エッジケース / リスク (ユーザーの「問題が起きないか」への回答)

### 4.1 【最重要】hold ジェスチャ vs 離散ショートカット

- ルーペ (Shift 保持)、一時パン (Space+ドラッグ) は「押している間だけ」の hold。
- 離散ショートカットの枠組み (consume_key / key_pressed) では表現できない。
- 対策: `Trigger::Hold` を別種別にし、hold バインドのカスタム値は
  **修飾単独 (Shift/Ctrl/Alt) か通常キー 1 個**に限定。修飾単独なら `i.modifiers.X`、
  通常キーなら `i.key_down(key)` を内部で使い分ける。
- **MVP では hold のカスタマイズを後回しにし、ルーペ Shift / Space パンは固定**にするのが安全。
  (ユーザーの「M でルーペ」要望を入れる場合のみ hold 対応を前倒し。)

### 4.2 egui `consume_key` の修飾マッチ順依存

- `Modifiers::NONE` の consume が**修飾付きイベントにもマッチする** (`matches_logically`)。
  現状コードは「Alt → Shift → NONE の順で書く」ことで回避している (§1.2)。
- keymap の `consume` ヘルパーは**同一キーの effective chord を修飾特異度降順に並べて
  consume** しないと、素のバインドが Ctrl+同キー等を誤消費する。テスト必須項目。

### 4.3 native 動画 VK ⇔ egui Key 変換のロス

- キャプチャ UI は egui 上で動く (egui::Key を得る) が、動画経路は VK で照合する。
  → `KeyCode` 経由で双方向変換するが、**変換できないキーが事故源**:
  - 文字/数字/F キー/矢印/Home 等は安全に対応。
  - 記号 `[` `]` やテンキー、非 US 配列キーは VK ↔ egui の対応が環境依存。
  - 対策: カスタマイズ可能キーを「安全に双方向変換できる集合」に**ホワイトリスト**で
    制限。リスト外キーはキャプチャ時に「このキーは割り当てできません」と弾く。

### 4.4 予約キー / 危険な再割当

- **Esc** (フルスクリーン解除 / モード脱出 / キャプチャ取消) を再割当すると詰む可能性。
  → `customizable: false` の予約キーにするか、再割当時に強い警告 + Esc は常に脱出可能の
  二重化を残す。
- **Enter** (開く/解除トグル、IME 確定) も同様に慎重に。
- **IME**: 文字キー (M, S, B ...) は日本語入力中に飛んでくる。既存の
  `ime_input_active()` ガード (CLAUDE.md「IME 対応」) をカスタムバインドでも必ず通す。
  カスタムで文字キーを増やしても、検索バー等の TextEdit 上では発火させない。

### 4.5 1 アクション複数チョード / 1 キー複数アクションの既存事情

- 既に「1 アクションに複数チョード」(←/Shift+←、I/Tab) と
  「1 物理キーが文脈ごとに別アクション」(S/B/L/I/V/H/R/O が各モードで別ツール) がある。
- 後者は**コンテキスト分離で解決済み**なので、衝突検査をコンテキスト単位に閉じれば
  問題にならない (= グローバルな名前空間衝突にしない)。これは設計上の追い風。

### 4.6 動画キーが画像キーより先に走る重なり

- `handle_video_input` が画像共通キー処理より前に走り、↑↓ などを「consume せず後段へ
  流す」テクニックで共存している (keymap-spec.md「設計メモ」)。
- keymap 化後もこの「流す/消費する」境界を維持する必要がある。動画タブと画像タブの
  バインドが矛盾しないこと (特に Shift+↑↓ = 音量 vs ファイル移動) を回帰テストで担保。

### 4.7 キーリピート方針

- 動画シークは `!key.repeat` で 1 回だけ、画像ナビは長押し連続移動を許す等、
  **アクションごとにリピート可否が違う**。これはバインドではなく**アクション側の固定属性**
  として持つ (ユーザーには露出しない)。

### 4.8 マウス入力 (戻る/進む) との等価扱い

- Extra1/Extra2 (マウス XButton) や VK_BROWSER_BACK/FORWARD を Ctrl+↑↓ 相当に
  束ねている (app.rs:13485、native_video.rs:4661)。MVP ではマウスボタンの
  カスタマイズは対象外とし、現状の固定マッピングを残す。

---

## 5. 実装規模見積もり

AI 生成 + ユーザーレビュー/実機テストのワークフロー前提。**段階分割を強く推奨。**

| フェーズ | 内容 | 概算 LOC | 規模感 | 主リスク |
|---|---|---:|---|---|
| **A. 基盤** | `KeyCode`/`Chord`/`Trigger`/`ActionId`/`Keymap` 型、VK⇔egui 変換表、解決ロジック、`consume`/`pressed`/`matches_vk`/`is_held` ヘルパー、settings 永続化 + forward-compat、デフォルト表 (≈160 アクション) | 800–1200 | 中〜大 | デフォルト表の網羅性、変換表 |
| **B. 配線** | 全インライン判定をヘルパー呼び出しに置換 (ui_fullscreen 135 / app.rs 47 / native_video ≈50 / erase 34 / conceal 30 / crop / text / global) | 差分大 (≈250+ 箇所) | **大 (長丁場)** | §4.2 消費順、§4.6 動画重なり、回帰 |
| **C. 設定 UI** | `PreferencesPage::KeyBindings`、タブ + 表、キャプチャウィジェット、衝突検査 + 警告 + 無効化フロー、リセット、Chord 整形 | 600–900 | 中〜大 | キャプチャ中の入力抑止、IME |
| **D. 仕上げ** | keymap-spec / spec / マニュアル更新、スナップショットテスト、グリフ lint、予約キー、ユニットテスト | 200–400 | 小〜中 | ドキュメント同時更新 |

**合計の目安**: フォーカスした実働で **おおむね 9〜14 日相当**。配線 (B) が最長ポール。
矢印・Esc・hold まで含めた「完全カスタマイズ」を一気にやると B のリスクが跳ね上がる。

### 推奨ロードマップ (リスク順)

1. **MVP (離散キーのみ)**: 基盤 A + 配線は「文字/数字/F キーのトグル系アクション」に限定 +
   UI C。**矢印ナビ・Esc/Enter・hold・マウスは固定。** コンテキストは FsImage と各編集
   モードを優先 (需要が高く自己完結)。→ §4.1/4.3/4.4 の難所をほぼ回避。
2. **拡張 1**: FsVideo (VK 経路) と Grid を配線対象に追加。§4.6 回帰テスト重点。
3. **拡張 2**: hold バインド対応 (ルーペを M へ等、ユーザー要望の本丸)。§4.1。
4. **拡張 3**: 矢印ナビ / 予約キーの再割当解放、マウスボタン、インポート/エクスポート。

MVP だけでも「各モードのツールキーや M/Z/S 等の再割当」という需要の中心は満たせる。

---

## 6. テスト / 検証方針

- **ユニット**: `Keymap` 解決 (custom 上書き / 無効化 / forward-compat マージ)、
  Chord ⇔ KeyCode ⇔ VK 往復、衝突検出、§4.2 消費順 (素キーが修飾付きを誤消費しない)。
  既存 `app/tests.rs` (`cargo test --bin mimageviewer-core`) に追加。
- **スナップショット**: 新ページ KeyBindings を `tests/ui_snapshot.rs` に追加。
- **手動 E2E**: keymap-spec.md の各コンテキストを 1 巡。特に動画 Shift+↑↓ (音量)、
  ルーペ、IME 中の文字キー、Esc 脱出、衝突警告→無効化。
- **グリフ lint** (`scripts/check_ui_glyphs.py`) と `cargo fmt` をコミット前に。

## 7. ドキュメント同時更新 (実装時)

- `docs/keymap-spec.md` — 「カスタマイズ可能 / 固定」列を追加。
- `docs/spec.md` — 設定項目に KeyBindings ページを追記。
- `docs/architecture-overview.md` — `keymap.rs` モジュールと永続化追記。
- `htdocs/mimageviewer/manual/` + `index.html` — ユーザー向け説明 (内部用語禁止、
  バージョンタグ禁止)。

---

## 8. 簡易版 (テキスト ini / GUI なし / 競合検知なし) — **推奨方針**

ユーザー提案 (2026-06-08) を採用した軽量版。§5 の最重量・最高リスクだった
**設定 UI (フェーズ C)** と **競合検知 / 汎用 hold 基盤** を削れる。
実装時の詳細は [key-customization-impl-plan.md](key-customization-impl-plan.md) を優先する。

### 8.1 仕様

- 判定箇所に「機能 (Action) 単位」の keymap 変換を挟む。**デフォルトキーはコードに残し
  (= 現状の引数をそのまま渡す)、ini には上書き分だけを書く。** 未移行 / 未上書きの箇所は
  従来通り動く (段階移行・低リスク)。
- keymap は `%APPDATA%/mimageviewer/keymap.ini` 等のテキストを**上級者が手書き**。
  起動時に 1 回読むだけ (ファイル監視やリロード UI は任意)。
- **競合検知しない。** 同じキーに複数機能を割り当てても警告しない:
  - **consume 経路** (フルスクリーン/編集): イベント消費型なので**先に判定した方が勝つ**
    (= 提案通りの挙動)。
  - **VK match 経路** (動画): `match` のアーム順で**先頭一致が勝つ**。
  - **key_pressed 経路** (グリッド): 非消費なので衝突すると**両方発火**する点だけ注意
    (件数が少なく実害は小さいが、仕様として明記する)。
- **修飾キーは「修飾として検出できるキー」にだけ差し替え可。** Shift→Ctrl / Alt は可、
  Shift→M は不可。これにより hold ジェスチャ (ルーペ = `i.modifiers.shift`) は
  `i.modifiers.<別修飾>` を読むだけで済み、§4.1 の hold 基盤が**不要**になる。

### 8.2 中核 (UI なし版)

```rust
// 機能の identity。コンテキストを名前に畳み込むと「線引き」が明示的になる。
// 同じ物理キーでも文脈が違えば別 Action (S=slideshow / S=select tool / S=tile)。
// 同じ機能を複数箇所で参照する場合 (例: keydown サマリと実消費) は同じ Action を渡す。
enum Action { FsSlideshow, FsRotateCw, EraseToolSelect, VideoMute, GridOpen, /* ... */ }

struct Chord { ctrl: bool, shift: bool, alt: bool, key: Option<KeyName> } // None=修飾単独 hold
struct Keymap { overrides: HashMap<Action, Vec<Chord>> }          // ini 由来。無ければ空

impl Keymap {
    // (A) consume 経路: 上書きがあればそれを、無ければ渡されたデフォルトを consume。
    fn consume(&self, ctx, act: Action, def_mods: egui::Modifiers, def_key: egui::Key) -> bool;
    // (B) key_pressed 経路 (非消費)
    fn pressed(&self, ctx, act: Action, def_key: egui::Key, def_mods: egui::Modifiers) -> bool;
    // (C) VK 経路 (動画)
    fn matches_vk(&self, act: Action, key: &NativeVideoKeyEvent,
                  def_vk: u32, def_ctrl: bool, def_shift: bool, def_alt: bool) -> bool;
    // (C-2) native presenter/HUD から UI 側へ転送すべきキーか
    fn native_video_shortcut_key(&self, key: &NativeVideoKeyEvent) -> bool;
    // (D) hold (修飾のみ可): 上書き修飾 or デフォルト修飾を i.modifiers から読む
    fn modifier_held(&self, ctx, act: Action, def: ModKind) -> bool;
}
```

### 8.3 1 箇所あたりの差分 (最小)

```rust
// before
let key_s = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::S));
// after — デフォルトは引数で同居。ini に FsSlideshow 上書きがあればそちらを使う。
let key_s = self.keymap.consume(ctx, Action::FsSlideshow, egui::Modifiers::NONE, egui::Key::S);
```

```ini
# keymap.ini (上書きだけ書く。書かない機能はコードのデフォルトのまま)
[FsImage]
FsSlideshow = P            ; スライドショーを P に戻す
FsRotateCw  = Ctrl+R

[Erase]
EraseToolSelect = A

[FsImage.hold]
FsLoupeHold = Ctrl         ; ルーペ修飾を Shift→Ctrl に (修飾のみ可)
```

### 8.4 規模の差 (§5 比)

| フェーズ | フル版 | 簡易版 |
|---|---|---|
| A 基盤 | 中〜大 (800–1200) | **小〜中 (300–500)** 競合/UI 状態/hold 基盤を削減 |
| B 配線 | 大 (≈300 箇所) | 大 (同数だが各差分は機械的・低リスク・段階可) |
| C 設定 UI | 中〜大 (600–900) | **ほぼ 0** (ini を起動時に読むだけ) |
| D 仕上げ | 小〜中 | 小 (ini 書式と Action 一覧の文書化) |

**合計目安: 5〜8 日相当** (フル版 9〜14 → 短縮)。最長ポールは配線 (B) のままだが、
危険箇所 (キャプチャ widget・競合・hold 基盤) が消える。

### 8.5 簡易版で割り切る点 (要確認)

- **M キーでルーペ (初回メッセージの例) は不可になる。** §8.1 の「修飾は修飾にだけ
  差し替え」制約により、ルーペは Shift→Ctrl/Alt の差し替えまで。M (非修飾) で hold は
  この簡易版の枠外。
  - もし M ルーペだけは欲しい場合: ルーペ 1 機能だけ「修飾 or 単キー hold」を許す
    特例 (`def: ModKind` を `Key or Modifier` 型にする) を入れれば小コストで対応可。
    全機能の hold 基盤を作るより遥かに安い。
- VK ↔ egui ↔ テキスト名の変換不能キー (記号・テンキー・非 US 配列) は、その経路で
  上書きを**無視 + ログ警告**で済ませる (whitelist UI は不要)。
- ini の書式エラー / 未知 Action は**その行だけ警告して無視**、他は生かす。

### 8.6 入力パターンの分類 (確定)

コード上の検出形は実は 3 種類。差し替え可能方向を以下に固定する:

| パターン | 検出 (例) | 該当例 | 差し替え可能方向 |
|---|---|---|---|
| **Press** (通常キー押下) | `consume_key(mods,key)` / `key_pressed` / VK match | 大多数。**M=ルーペ・ロックのトグル** ([ui_fullscreen.rs:4754](../src/ui_fullscreen.rs)) もこれ | キー↔任意の通常キー、修飾↔任意の修飾 (自由) |
| **Modifier-hold** (修飾保持) | `i.modifiers.shift` | Shift 押しっぱなしルーペ ([ui_fullscreen.rs:7340](../src/ui_fullscreen.rs)) | **修飾↔修飾のみ** (Shift→Ctrl/Alt 可、通常キー不可) |
| **Key-hold** (通常キー保持) | `i.key_down(key)` | Space+ドラッグ一時パン ([ui_erase.rs:1214](../src/ui_erase.rs)) | キー↔通常キー |

- ユーザーの言う「通常キーパターン / モディファイヤパターンの 2 つ」はこの表の
  Press と Modifier-hold にほぼ対応。Key-hold (Space パン) は稀なので
  「Press と同じく通常キー同士で差し替え」扱いにすれば実質 2 系統で考えてよい。
- **唯一できない方向 = 「通常キーを修飾キーとして振る舞わせる」**。通常キーは
  `i.modifiers` に現れないため。`i.key_down(M)` で *Key-hold* にはできるが、それは
  「M を他チョードの修飾として使う」ことではない。本簡易版ではこの方向は非対応で確定。
- 具体例の確認:
  - **M トグル → L トグル**: 両方 Press。**自由に差し替え可**。✓ (ユーザー許容範囲)
  - **Shift ルーペ → Ctrl/Alt ルーペ**: Modifier-hold の修飾差し替え。✓
  - **Shift ルーペ → M ルーペ**: 不可 (M は修飾でない)。✓ ユーザー合意済み

### 8.7 1 機能に複数チョード (確定)

Ctrl+Y / Ctrl+Shift+Z のように 1 機能へ複数キーを割り当てるケースは、
内部的には `Vec<Chord>` で持ち、ini は**`.1` サフィックスで最大 3 つまで**:

```ini
[FsImage]
FsSlideshow.1 = P
FsSlideshow.2 = S
[Text]
TextRedo.1 = Ctrl+Y
TextRedo.2 = Ctrl+Shift+Z
```

- **上書きセマンティクス = 全置換**: ある機能に 1 つでも上書き行があれば、その機能の
  コードデフォルトは**全部無効化**し、ini の列挙だけが有効になる (= デフォルトの一部を
  捨てたり順序を変えたりが明示的にできる)。
- 判定は列挙順に試し、consume 経路では最初にマッチしたものを消費 (= §8.1 の先勝ちと整合)。
- 上限 3 は ini 書式・文書化の都合の soft cap。内部 `Vec` 自体は個数制限不要。
- 末尾数字ではなく `.1` 形式にする。`FsRate1` / `FsAdjustSlot1` のように Action 名自体が
  数字で終わるものと衝突させないため。

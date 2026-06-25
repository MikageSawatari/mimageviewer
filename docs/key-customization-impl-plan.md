# キーカスタマイズ機能 実装プラン (簡易版 / テキスト ini)

> ステータス: **実装済み / 保守メモ**。v1.1.0+。
> 設計の経緯・調査・代替案は [key-customization-plan.md](key-customization-plan.md) に残す。
> 簡易版の実装判断は本書を優先し、差異が出たら本書へ集約する。
> 本書は「簡易版 (テキスト ini / GUI なし / 競合は警告のみ)」を実際に作るための手順書。
>
> **現状メモ (2026-06-25)**: この簡易版で導入した `keymap.ini` は、コマンド設定 GUI へ進むため
> 初回起動時の移行元になった。現在の正本は `Settings.keymap` (`settings.db`) で、旧
> `%APPDATA%\mimageviewer\keymap.ini` が残っている場合は 1 回だけ読み込んで同じ override を
> settings.db に保存し、`keymap.ini.imported*.bak` へ退避する。設定メニュー「操作カスタマイズ…」から
> `Settings.keymap` を編集できる。`keymap.ini.default` は Action 名と既定キーの参照として引き続き生成する。

関連: [keymap-spec.md](keymap-spec.md) (現行キー仕様 = アクション洗い出しの元ネタ)、
[key-customization-plan.md](key-customization-plan.md) §8 (簡易版の設計確定事項)。

実装結果:
- `src/keymap.rs` に Action 定義、ini parser、egui exact match、KeyHold /
  ModifierHold、native VK 判定、コメントアウト済み `keymap.ini` / `keymap.ini.default`
  生成元となる一覧を集約した。
- `Settings::load_with_meta` で旧 `%APPDATA%/mimageviewer/keymap.ini` が残っていれば 1 回だけ読み、
  `Settings.keymap` へ移行して `keymap.ini.imported*.bak` に退避する。`App::new_from_settings` は
  `Settings.keymap` から `Keymap` を構築し、`keymap.ini.default` は現在バージョンの標準参照として
  更新生成する。警告はログへ出す。テストではユーザー環境の ini を読まない。
- 画像フルスクリーン、編集モード (消しゴム / 隠蔽 / 切り取り / テキスト / 補正レイヤー)、
  グリッド主要操作、egui/native 動画主要操作を keymap 経由にした。
- 2026-06 Phase 1 として、既定キーを持たない `KeyAction` も許可した。`GridToggleStackMode`
  は `keymap.ini.default` に `# GridToggleStackMode = none` として出し、ユーザーがキーを
  指定したときだけフォルダバーの「スタック」と同じトグルを実行する。`CommandId` /
  `CommandSpec` などのコマンドカタログ型はまだ導入していない。
- 2026-06 Phase 2 初期実装として、`KeyContext` を scope とする `CommandSpec` /
  `BindingPolicy` / active scope 隣接表 / `BindingConflict` を追加した。ユーザー override が
  同時 active になり得る既存割り当てや、Esc / Enter / 修飾なし矢印の予約キーに重なる場合は
  起動時に警告ログを出す。設定拒否や dispatch 変更はしない。
- 2026-06 Phase 3 初期実装として、グリッド側の F7-F10 / Shift+F7-F10 マスク一括適用・
  削除を `GridApplyErase1/2`、`GridApplyConceal1/2`、`GridDeleteEraseMask`、
  `GridDeleteConcealMask` として `KeyAction` 化した。既定キーと実行順は従来どおり。
- 2026-06 Phase 4 初期実装として、フルスクリーン縦方向の文脈解決を小さく進めた。
  スタックフラット読書中の `Shift+↑/↓` は `FsStackJumpPrev/Next` として `KeyAction`
  化し、egui 動画フルスクリーンの `↑/↓` も `VideoPrevFile/NextFile` を見るようにした。
  `Keymap::resolve_first_action_for_chord` で active scope と優先順の純粋 resolver をテストする。
- 2026-06-25 の GUI 初期スライスとして、設定メニュー「操作カスタマイズ…」から `Settings.keymap` の
  上書きを編集できるようにした。競合は保存禁止にせず警告表示に留め、競合一覧から該当コマンドの
  編集欄へ移動できる。編集は独立した割り当て編集ダイアログで行い、キー / リング・パッド /
  マウス進む・戻る / マウスジェスチャのタブを切り替えられる。コマンド一覧は短い表示名で
  キー操作と割り当て済みのリング / マウス / パッド操作を同じ表に混ぜ、キーボード図のキークリックから
  割り当て先コマンドを選べる。
- Esc / Enter ナビゲーション、矢印ナビゲーション、OS clipboard、
  D&D、IME 確定は固定扱いのまま。マウス / ゲームパッドは keymap.ini 対象外だが、
  右ドラッグ、ゲームパッド X リング、マウス戻る / 進むボタンは
  `Settings.ring_shortcuts` の小さな固定入力レイヤーで扱う。UI 上はリングとマウスボタンを
  別ページに分ける。

---

## 0. スコープ確定 (前提)

ユーザー合意済みの割り切り (詳細は design doc §8):

- **GUI を作らない。** `%APPDATA%/mimageviewer/keymap.ini` を上級者が手書き。起動時に 1 回読む。
- **デフォルトはコードに残す。** 初回生成される `keymap.ini` は標準設定をコメントアウトで列挙する。
  コメント解除した Action だけが上書きになり、コメントのままの操作はコードの既定に追従する。
  最新の標準一覧は、起動時に上書き更新される `keymap.ini.default` で確認できる。
  既定キーなしの Action は `# Action = none` と表示し、コメント解除してキー名を入れることで
  割り当て可能にする。`Action = none` のままコメント解除した場合は従来どおり明示無効化。
- **競合は拒否しない。** ユーザー override が同時 active になり得る割り当てや予約キーに
  重なる場合は警告ログを出すが、設定は読み込む。dispatch は先勝ち (consume / VK match
  経路は先頭一致が勝つ。grid の `key_pressed` 経路だけは非消費なので衝突時は両方発火する)。
- **MVP で対象外にする入力を明示する。** ゲームパッド、マウス操作、D&D、OS/egui の
  `Event::Copy` / `Event::Cut`、クリップボード paste、IME 確定、右クリックメニューは keymap.ini では固定扱い。
  ただし右ドラッグ、ゲームパッド X リング、戻る / 進むボタンは
  `Settings.ring_shortcuts` 側で限定カスタマイズする。戻る / 進むボタンは
  グリッド / 画像フルスクリーン / 動画フルスクリーンごとに単体アクションを割り当てる。
  Shift / Alt + ホイールのカスタマイズは、グリッド / 画像 / 動画でルーティング差が大きいため
  将来の別フェーズにする。
- **入力パターンは 3 種、差し替え方向は固定** (design doc §8.6):
  - **Press** (通常キー押下、M=ルーペ・ロックトグル含む): キー↔通常キー / 修飾↔修飾 自由
  - **Modifier-hold** (Shift ルーペ): 修飾↔修飾のみ
  - **Key-hold** (Space パン): キー↔通常キー
  - 不可能な方向: 「通常キーを修飾として振る舞わせる」(`i.modifiers` に現れないため)
- つまり簡易版では **Shift ルーペを M 長押しへ置き換えることはしない**。M は Press の
  ルーペ・ロックトグルとしてなら再割当できる。
- **1 機能あたり複数チョード可 (最大 3、`.1`〜`.3` サフィックス)。上書きは全置換**
  (design doc §8.7)。

---

## 1. 成果物とモジュール構成

新規モジュール 1 本 + 既存への配線:

```
src/keymap.rs              ← 新規。型・解決ロジック・ini パーサ・名前テーブル・ヘルパー
src/keymap/tests.rs        ← 新規 (or keymap.rs 内 #[cfg(test)])。純粋ロジックの unit test
%APPDATA%/mimageviewer/keymap.ini          ← 初回起動時に全キー定義行コメントアウトで生成 (ユーザー編集用)
%APPDATA%/mimageviewer/keymap.ini.default  ← 現在バージョンの標準参照。起動時に上書き更新
docs/keymap.ini.default                    ← 配布用の標準設定リファレンス
```

`App` 側:
- `App` に `keymap: crate::keymap::Keymap` フィールドを 1 つ追加 (起動時にロード)。
- 各キー判定箇所を `self.keymap.<helper>(...)` 呼び出しに置換 (フェーズ 2〜5)。

`lib.rs` に `pub mod keymap;` を追加。

---

## 2. 中核型 (確定)

```rust
// src/keymap.rs

/// コンテキスト (= 状況。ini の [セクション] と UI 上のタブ概念に対応)。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum KeyContext {
    Global, Grid, FsCommon, FsImage, FsVideo,
    Erase, Conceal, Crop, Text, LocalAdjust,
}

/// 論理アクション。コンテキストを名前に畳み込み一意にする (= 線引きの実体)。
/// 同じ物理キーでも文脈違いは別 variant。同じ機能を複数箇所で参照する場合は同じ variant。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum KeyAction { /* 付録 A の全リスト */ }

impl KeyAction {
    pub fn context(self) -> KeyContext;     // 所属コンテキスト
    pub fn ini_name(self) -> &'static str;  // ini のキー名 (例 "FsSlideshow")。一意。
    pub fn trigger(self) -> KeyTrigger;     // Press / ModifierHold / KeyHold
    pub fn customizable(self) -> bool;      // false = 予約 (Esc 等)。ini 上書きを無視
    pub fn all() -> &'static [KeyAction];   // ini 生成・テスト用
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyTrigger { Press, ModifierHold, KeyHold }

/// 1 つのキー組み合わせ。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Chord {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: Option<KeyName>,   // None = 修飾単独 (ModifierHold 用)
}

/// テキスト名で持つ物理キー。egui::Key と Win32 VK の双方へ解決できるものだけ定義。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyName { A, B, /* ...Z */ Num0, /* ...Num9 */ F1, /* ...F12 */
    Left, Right, Up, Down, Home, End, PageUp, PageDown,
    Space, Enter, Esc, Tab, Backspace, Delete,
    OpenBracket, CloseBracket, Semicolon, Colon, Comma, Period, Backslash, Slash, Minus }

impl KeyName {
    pub fn parse(s: &str) -> Option<KeyName>;   // "P" "F7" "Left" "[" 等
    pub fn to_egui(self) -> Option<egui::Key>;
    pub fn from_egui(k: egui::Key) -> Option<KeyName>;
    pub fn to_vk(self) -> Option<u32>;          // native 動画経路
}

/// ini 由来の上書き集合。無ければ空 = 全部コードデフォルト。
pub struct Keymap {
    overrides: std::collections::HashMap<KeyAction, Vec<Chord>>,
    warnings: Vec<String>,   // 起動時にログへ出す (未知 Action / 変換不能キー等)
}
```

---

## 3. ini 仕様 (確定)

```ini
# keymap.ini  ―  初回生成時は標準設定が全行コメントアウトで展開される。
# 上級者は変えたい行だけコメント解除して右辺を編集する。
# コメントのままの Action はコードの既定を使う。
# [セクション] は可読性のためのグループ。Action 名は全体で一意なので、解決は Action 名で行う。
# 1 機能に複数キーは .1..3 サフィックス。ある機能を 1 行でも書くとデフォルトは全置換。

[FsImage]
# FsSlideshow.1 = P           ; スライドショーを P にするならコメント解除
# FsSlideshow.2 = S           ; S も残したい場合は明示的に併記 (全置換のため)
# FsLoupeLockToggle = L       ; ルーペ・ロックのトグルを M → L へ (Press なので自由)
# FsCapture = none            ; 明示的に無効化する例

[FsImage.hold]
# FsLoupeHold = Ctrl          ; ルーペ保持の修飾を Shift → Ctrl (ModifierHold = 修飾のみ)

[Erase]
# EraseToolSelect = A

[Text]
# TextRedo.1 = Ctrl+Y
# TextRedo.2 = Ctrl+Shift+Z
```

パース規則:
- 行 = `Action = <chord>` または `Action.<番号> = <chord>`。番号は 1..3 のみ。
  末尾数字ではなく `.` で分けることで、`RatingItem1` / `FsAdjustSlot1` のような Action 名と衝突させない。
- chord = `+` 区切り。`Ctrl`/`Shift`/`Alt` を修飾として拾い、残り 1 トークンを `KeyName::parse`。
  修飾単独 (キーなし) は ModifierHold 専用。
- `none` / `None` / 空値はその Action を明示的に無効化する。`Action.1 = none` と
  `Action = none` は同じ意味で、ほかの番号行と混在させない。
- **検証 (該当行だけ警告して無視、他は生かす)**:
  - 未知 Action 名 → 警告
  - Action の `context()` と ini セクション名が一致しない場合 → 警告して受理
    (セクションは可読性用なので実行上は Action 名を優先)
  - `KeyName::parse` 失敗 / `to_vk`・`to_egui` のどちらかが必要なのに `None` → 警告 (付録 B)
  - 固定扱いの操作は `KeyAction` に入れないため、該当する Action 名は未知 Action として警告
  - Press/KeyHold に修飾単独を割当 / ModifierHold に通常キーを割当 → 警告 (パターン不一致)
- 同一 Action の番号行を集めて `Vec<Chord>` 化 (順序 = 番号順、番号なしは `.1` 相当)。
  空 Vec は「無効化」。

---

## 4. ルックアップ API と解決規則

```rust
impl Keymap {
    /// effective = overrides.get(action).cloned().unwrap_or_else(|| vec![default])
    /// ただし default は呼び出し側がインラインで渡す (コード = デフォルト表)。

    /// (A) consume 経路: フルスクリーン / 編集モード。イベントを消費。
    pub fn consume(&self, ctx: &egui::Context, action: KeyAction,
                   def_mods: egui::Modifiers, def_key: egui::Key) -> bool;

    /// (B) key_pressed 経路: グリッド。非消費。
    pub fn pressed(&self, ctx: &egui::Context, action: KeyAction,
                   def_key: egui::Key, def_mods: egui::Modifiers) -> bool;

    /// (C) VK 経路: native 動画。
    pub fn matches_vk(&self, action: KeyAction,
                      key: &crate::video::native_window::NativeVideoKeyEvent,
                      def_vk: u32, def_ctrl: bool, def_shift: bool, def_alt: bool) -> bool;

    /// (C-2) native 動画の presenter/HUD から egui 側へ転送すべきキーかを判定。
    /// `native_video_fullscreen_shortcut_key` の静的ホワイトリストを keymap 連動に置き換える。
    pub fn native_video_shortcut_key(&self,
                                     key: &crate::video::native_window::NativeVideoKeyEvent)
                                     -> bool;

    /// (D-1) ModifierHold: 解決済み修飾を i.modifiers から読む (ルーペ)。
    pub fn modifier_held(&self, ctx: &egui::Context, action: KeyAction,
                         def: ModKind) -> bool;

    /// (D-2) KeyHold: 解決済み通常キーを i.key_down で読む (Space パン)。
    pub fn key_held(&self, ctx: &egui::Context, action: KeyAction,
                    def_key: egui::Key) -> bool;
}
```

解決規則の要点:
- **修飾キーは exact match を原則にする。** `egui::InputState::consume_key` は
  `matches_logically` の都合で `Modifiers::NONE` が Shift+同キーを拾うケースがあるため、
  keymap の主判定は `i.events` の `Event::Key { pressed: true, key, modifiers, repeat, .. }`
  を走査して、`ctrl/shift/alt` が chord と完全一致するイベントだけを成立扱いにする。
  成立した consume 経路では該当イベントを取り除く、または同等に後段へ流れない状態にする。
  `consume_key` を併用する場合も、exact match のテストを通した wrapper の内側だけで使う。
- **特異度降順はフォールバック規則。** exact match 実装でも、同一 Action の複数 chord は
  修飾ビット数が多い順に試す。これで `C` / `Shift+C` / `Alt+C` のような組を安定させる。
  デフォルトのみのときは現行コードの優先順を `KeyAction` 定義側で再現する。
- **複数チョード**: effective が複数なら順に試し、最初に成立したものを返す (consume では消費)。
- **VK 経路**: effective chord を VK + 修飾フラグへ解決し
  `key.virtual_key == vk && key.ctrl == .. && key.shift == .. && key.alt == ..`
  を順に判定。`to_vk()==None` の chord はスキップ (付録 B)。`!key.repeat` 等の**リピート可否は
  Action 側の固定属性** (ini に出さない。helper 内部 or 呼び出し側で従来通り判定)。
- **置換セマンティクス**: `overrides` に Action が存在すればデフォルトを使わず effective = overrides 値。

### 置換前後の例 (consume 経路)

```rust
// before (ui_fullscreen.rs:3858)
let key_s = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::S));
// after
let key_s = self.keymap.consume(ctx, KeyAction::FsSlideshow,
                                egui::Modifiers::NONE, egui::Key::S);
```

```rust
// before (ui_fullscreen.rs:7350, ModifierHold)
if !self.fs_loupe_locked && !shift_held { return; }
// after
let loupe_mod = self.keymap.modifier_held(ctx, KeyAction::FsLoupeHold, ModKind::Shift);
if !self.fs_loupe_locked && !loupe_mod { return; }
```

```rust
// before (native_video.rs:4729, VK 経路)
0x4D if !key.shift && !key.ctrl && !key.repeat => { /* mute */ }
// after: match の前で各 Action を判定して分岐 (または match アーム条件をヘルパーに置換)
if !key.repeat && self.keymap.matches_vk(KeyAction::VideoMute, &key, 0x4D, false, false, false) {
    /* mute */
}
```

> native 動画は `match key.virtual_key` 構造なので、配線時は「match を if 連鎖へ展開」か
> 「各アームのガードを `matches_vk` 呼び出しに差し替え」のどちらか。後者の方が差分が小さい。
> アーム順 = 先勝ち順を維持すること。
>
> さらに `src/video/native_presenter/mod.rs` の `native_video_fullscreen_shortcut_key` は、
> presenter/HUD から UI 側へキーを転送するための静的ホワイトリストなので、ここも
> `Keymap::native_video_shortcut_key` 相当に置き換える。判定本体だけ keymap 化しても、
> 転送リストが古いままだとカスタムキーは `handle_native_video_key_event` へ届かない。

---

## 5. 永続化

- keymap.ini は `Settings` (settings.db) には**入れない**。独立テキストファイルとして
  `data_dir()/keymap.ini` を直接読む (上級者がエディタで触る前提なので DB に入れない方が自然)。
- 起動時 (`App::new` 相当) に `Keymap::write_user_ini_if_missing(path)` でコメントアウト済み
  user ini を初回生成し、`Keymap::write_default_reference_ini(path)` で `keymap.ini.default` を
  現在バージョンの参照として上書き更新する。`Keymap::load_from(path)` で読むのは user ini だけ。
  既存 `keymap.ini` は上書きしない。
- `keymap.warnings` を `logger::log` へ出力 (ユーザーが誤記を気付けるように)。
- テスト用の `App` 生成や unit test では、実ユーザーの `%APPDATA%/mimageviewer/keymap.ini` を
  読まない。`Keymap::empty()` / `Keymap::from_str()` を注入し、開発者環境の ini がテスト結果を
  変えないようにする。
- **マイグレーション不要** (新規・未リリース。CLAUDE.md「永続データ変更時の判断」の未リリース該当)。
- 任意拡張: 環境設定「開発者」ページに「keymap.ini を開く」「再読込」ボタンだけ置くと親切
  (GUI 編集ではないので低コスト)。MVP では省略可。

---

## 6. 実装フェーズ (PR 粒度・各フェーズ独立リリース可)

| Ph | 内容 | 触る所 | 規模 | 検証 |
|---|---|---|---|---|
| **0** | `keymap.rs` 骨組み: 型・`KeyName` 変換表・ini パーサ・exact match 解決ロジック・`KeyAction::all()` + コメントアウト済み ini / `.default` 生成。**呼び出し側は未変更 (挙動ゼロ変化)** | keymap.rs / lib.rs | 中 | unit test (§8) |
| **1** | `App.keymap` 追加 + 起動ロード + 警告ログ。まだ誰も参照しないので挙動変化なし | app.rs | 小 | 起動して空ロード確認 |
| **2** | **FsImage 配線** (consume 経路、最も需要が高く自己完結)。離散 Press 系を優先し、表示モード `1..7` と連続読み PageUp/PageDown も棚卸しする。矢印/Esc/Enter/loupe-hold は §7 のルールで扱う | ui_fullscreen.rs (≈135 中の画像分) | 大 | keymap-spec の画像節を 1 巡 + ini 上書き動作確認 |
| **3** | **編集モード配線** (全て consume 経路・自己完結): Erase / Conceal / Crop / Text / LocalAdjust | ui_erase.rs(34) / ui_conceal.rs(30) / ui_crop.rs / ui_text.rs / ui_fullscreen.rs(la ブロック) | 大 | 各モードを 1 巡 |
| **4** | **Grid 配線** (key_pressed 経路、非消費 = 衝突で両発火に注意)。Ctrl+A/D/Shift+A など純粋キー操作は候補、OS クリップボード / D&D は固定扱い | app.rs `handle_keyboard`(≈47) | 中 | グリッドナビ + 衝突仕様の確認 |
| **5** | **FsVideo 配線** (VK 経路。§4.6 画像との重なり要注意) + FsCommon。`native_video_fullscreen_shortcut_key` の静的ホワイトリストも keymap 連動にする | app/native_video.rs(≈50) / ui_fullscreen.rs(共通分) / video/native_presenter/mod.rs | 大 | 動画節を 1 巡 + Shift+↑↓ 音量等の回帰 |
| **6** | **hold 系の上書き対応**: ModifierHold (loupe) / KeyHold (Space パン) を `modifier_held`/`key_held` 経由に。Shift→Ctrl、Space→P のような同種差し替えを確認 | ui_fullscreen.rs(7340) / ui_erase.rs(1214) / ui_conceal.rs(987) | 小 | ルーペ修飾差し替え / パンキー差し替え |
| **7** | 仕上げ: 標準 keymap.ini.default 配布、docs 更新、glyph lint、`cargo fmt`、追加 unit/snapshot | docs / htdocs | 小 | §9 |

- **段階出荷可**: 各フェーズ完了時点でビルド通過 & 既存挙動維持 (未移行サイトはデフォルト動作)。
- **MVP の最小線**: Ph0 + Ph1 + Ph2 (+ Ph3) で「フルスクリーン & 編集モードの離散キー再割当」が成立。
  Grid / Video / hold は後追いで足せる。
- **標準 ini / `.default` は実装済み Action だけを出す。** `KeyAction::all()` に将来候補を入れる場合は
  `implemented()` / `phase()` を持たせ、未配線 Action を ini に出して「コメント解除したのに効かない」
  状態を作らない。

### 実装規模目安

- Ph0+Ph1+Ph2 の MVP: 5〜8 日程度。FsImage の棚卸しと exact match unit test が主作業。
- Ph3 まで含める: 1〜1.5 週間程度。編集モードは経路が揃っているが件数が多い。
- Ph4+Ph5+Ph6 まで含める: 1.5〜2.5 週間程度。Grid の非消費仕様、native 動画の転送経路、
  hold の型制約が追加リスク。

---

## 7. エッジケース対応規則 (配線時のチェックリスト)

design doc §4 / §8.6 の実装時ルール。各サイト置換時に必ず確認:

1. **矢印ナビ / Esc / Enter は当面「固定扱い」として keymap 対象外**を推奨。
   - 理由: Esc 再割当でモード脱出不能 / Enter は IME 確定や open-close トグルと密結合 /
     矢印は RTL 反転・見開き 2 ページ送り・動画シーク粒度と絡む。
   - 将来解放する場合も「Esc は常に脱出可能」フォールバックを残す。
2. **動画シーク (←→ + 修飾で 5/1/30 秒)** は修飾でgranularityを切替える特殊構造。
   MVP では**固定**。カスタムするなら `VideoSeekFwd5/1/30` の 3 Action に分割して扱う。
3. **IME ガード維持**: 文字キー Action は既存 `ime_input_active()` / `dialog_*_pressed` の
   ガードを**helper の外側で従来通り**通す。helper は純粋に「このキーが押されたか」だけ答える。
4. **exact match**: §4 の「修飾完全一致」を helper 内で保証。同一キーに NONE と SHIFT が
   ある場合 (例 C / Shift+C / Alt+C、矢印 alias) は、デフォルト chord を複数持たせて
   現行挙動を再現する。`consume_key(Modifiers::NONE, key)` へ直に寄せない。
5. **VK 変換不能キー**: 記号の一部・非 US 配列は `to_vk()==None`。動画経路ではその上書きを
   スキップ + 警告 (付録 B のホワイトリスト)。テンキー数字は通常の数字キー alias として扱い、
   別アサインはできない。
6. **native 動画の転送ホワイトリスト**: `src/video/native_presenter/mod.rs` の
   `native_video_fullscreen_shortcut_key` を keymap 連動にしない限り、カスタムした VK が
   UI 側へ届かない。Ph5 の必須作業にする。
7. **動画と画像の重なり** (keymap-spec「設計メモ」): `handle_video_input` が画像キーより先に
   走り ↑↓ を「流す」構造を維持。Video 用 Action と FsImage 用 Action が同キーでも矛盾しないこと。
8. **連続読み PageUp/PageDown**: 縦/横連結モードで実際に連続描画しているときだけ有効。
   keymap helper の成立前後で、現在の `continuous_reading_should_scroll_*` 系の条件ガードを
   弱めない。
9. **grid 非消費の両発火**: 同一キーを 2 機能へ割当てると両方走る (consume でないため)。
   仕様として許容し、生成される ini のコメントに注記。
10. **OS/egui 由来の clipboard / D&D は固定**: `Event::Copy` / `Event::Cut`、
    Win32 クリップボード paste、ファイル D&D は keyboard keymap の範囲外。対象にするなら
    Grid の別フェーズで明示的に設計する。
11. **レーティング F1-F6** は `[Rating]` コンテキストの `RatingItem1..5/Clear` と
   `RatingContainer1..5/Clear` に集約する。グリッド / 画像フルスクリーン / native 動画の
   入口は `Keymap::consume_rating_action` / `native_video_rating_action` を使い、保存処理は既存の
   `set_rating` / `set_current_folder_rating` 経路を共有する。
12. **生 Event::Key / OS 状態参照経路** (pipeline debug=pipeline_debug.rs:104、
    右 Ctrl の original/source preview 系など) は特殊。MVP では keymap 対象外の固定操作。
    旧 F11 window mode 経路は `FsToggleWindowMode` へ昇格済みで、native 動画 presenter も
    global shortcut snapshot 経由で同じ effective chord を使う。
13. **ゲームパッドは固定**: `src/app/gamepad_input.rs` は閲覧専用の物理ボタン/軸入力として扱い、
    keymap.ini の対象にしない。docs/keymap-spec.md のゲームパッド節と同じ方針。

---

## 8. テスト計画

`cargo test --bin mimageviewer-core` (App 系テストは `--lib` に出ない: MEMORY 参照)。

- **ini パース**: 正常 / `.1..3` サフィックス集約 / Action 名末尾数字との非衝突 /
  未知 Action / セクション不一致警告 / 変換不能キー / パターン不一致 /
  修飾単独 / `none` 無効化 → それぞれ warnings と overrides を検証。
- **解決ロジック**: 上書きあり/なし、複数チョード順、全置換セマンティクス。
- **exact match consume/pressed**: 素キー override が Shift/Ctrl/Alt 付きイベントを誤消費・誤発火しないこと。
  `C` / `Shift+C` / `Alt+C`、`T` / `Shift+T` / `Alt+T`、矢印 alias を代表ケースにする。
- **KeyName 往復**: `from_egui ∘ to_egui` / `to_vk` の整合、変換不能キーが `None` を返すこと。
- **VK マッチ**: `matches_vk` が修飾フラグ込みで正しく判定 (NativeVideoKeyEvent を直接組む)。
- **native 動画転送**: `native_video_shortcut_key` がデフォルト VK とカスタム VK の両方を
  転送対象にし、対象外の Alt 系やテキスト入力中の抑止を壊さないこと。
- **ini 生成**: `KeyAction::all()` から生成した `keymap.ini` / `keymap.ini.default` が、
  未実装 Action を出さず、`Action.1` 形式と `none` の説明を含み、コメントアウト状態では
  overrides 空でパースできること。
- **スナップショット**: GUI を作らないので原則不要。開発者ページにボタンを足したら 1 枚追加。
- **手動 E2E**: keymap-spec.md の各コンテキストを 1 巡 (フェーズごと)。ini 上書きを 2〜3 個入れて
  反映確認、conflict で先勝ち/両発火の挙動確認。

---

## 9. ドキュメント同時更新 (実装完了時)

- `docs/keymap-spec.md` — 各行に「Action 名 / customizable」列を追加。
- `docs/spec.md` — keymap.ini の存在と書式、設定項目として追記。
- `docs/architecture-overview.md` — `keymap.rs` モジュールと `keymap.ini` を永続化/モジュール表に追加。
- `docs/README.md` — 索引に本書と design doc を追加 (済みなら確認)。
- `docs/keymap.ini.default` — `KeyAction::all()` から全 Action の標準設定をコメントアウトで生成し配布。
- `htdocs/mimageviewer/manual/` + `index.html` — 上級者向けに keymap.ini の場所と、
  詳細は設定ファイル内コメントにあることだけを説明 (**内部用語・バージョンタグ禁止**、
  ライブラリ名や mpsc 等は出さない: CLAUDE.md「記述方針」)。
- コミット前: `python scripts/check_ui_glyphs.py` + `cargo fmt`。

---

## 10. 新しいキー操作を追加するとき (保守 runbook)

ユーザーから「keymap 対応も」と明示されなくても、キーボード操作を追加・変更する修正では
毎回この手順を通す。

1. **keymap 対象か固定扱いかを決める。**
   閲覧・編集・動画の通常ショートカットは原則 keymap 対象。IME 確定、OS clipboard、
   D&D、右クリック、通常マウス、ゲームパッドなど固定扱いにする入力は、`docs/keymap-spec.md`
   の該当節に「対象外」の理由を残す。
2. **対象なら `KeyAction` を追加する。**
   `ini_name()` / `description()` / `context()` / `trigger()` / `default_chords()` を埋める。
   `description()` は生成される `keymap.ini` / `keymap.ini.default` の行末コメントになるため、
   ユーザーが設定ファイル単体で操作内容を判断できる日本語説明にする。`trigger()` は
   `_ => Press` を使わない網羅 match なので、新 variant を分類しないとコンパイルで止まる。
   標準キーを持たないがユーザー割り当て可能にしたい操作は、`default_chords()` で空の
   `ChordList` を返す。
3. **`ALL_ACTIONS` に追加する。**
   `KeyAction::all()` は `keymap.ini` / `keymap.ini.default` 生成、ini parse、native 動画転送 whitelist の入口。
   `all_actions_inventory_matches_key_action_enum` が enum と配列のドリフトを検知する。
4. **呼び出し側を keymap helper 経由にする。**
   `consume_key` / `key_pressed` / `i.events` / native VK の直書きは、固定扱いの入力か、
   keymap helper の内側だけに閉じ込める。Press は `consume_action` / `pressed_action`、
   hold は `modifier_held_action` / `key_held_action`、native 動画は `matches_vk_action` と
   `native_video_shortcut_key` の連動を確認する。
5. **ドキュメントと同梱 `.default` を更新する。**
   `docs/keymap-spec.md` に操作仕様、`docs/keymap.ini.default` に標準設定と行末説明を反映する。
   既定キーなしの Action は `# Action = none` として列挙する。
   `bundled_keymap_default_matches_generated_reference` がコード生成結果とのズレを検知する。
6. **狭いテストから回す。**
   最低限 `cargo test keymap --bin mimageviewer-core`。動画 VK を触ったら
   `cargo test native_video_fullscreen_shortcut_key --bin mimageviewer-core`、検索/グリッドキーを
   触ったら該当 App-level テストも追加で確認する。

---

## 付録 A. KeyAction インベントリ (初版・配線時に確定)

[keymap-spec.md](keymap-spec.md) から導出。`(P)`=Press / `(MH)`=ModifierHold / `(KH)`=KeyHold /
`(固定)`=keymap 対象外。`★`=MVP (Ph2/3) 対象、それ以外は後続フェーズ。
`(固定)` は本計画では keymap.ini 対象外。

### MainWindow / Grid chrome (fullscreen 外)
- GlobalLocalSearch `Ctrl+F` (P) / GlobalFavSearch `Ctrl+S` (P) /
  GlobalMetadataSearch `Ctrl+G` (P) / GlobalOpenFolder `Ctrl+O` (P) /
  ToggleDetachedViewerMode `F12` (P) / HelpShowContextShortcuts `?` (P)
  - 検索・フォルダを開く系は dialog / address / search focus / fullscreen 中は既存ガードで無効化される。
  - ToggleDetachedViewerMode は dialog / text focus / IME 変換中 / 静止画 fullscreen 編集サブモードでは抑止し、fullscreen / native 動画では明示 consume する Global action。
  - GlobalFavoritePrev/Next、GlobalOpenFavorite1..20、GlobalOpenDriveC..Z、
    GlobalOpenLocation* は既定未割り当て。設定名は互換性のため `Global...` のままだが、
    実体はサムネイル一覧 (`Grid`) の場所移動 Action として扱う。操作カスタマイズでキーを
    割り当てると、グリッド表示中に同じ場所移動を実行する。
- GridSelectAll `Ctrl+A` (P) / GridDeselect `Ctrl+D`,`Ctrl+Shift+A` (P)
- GridColumnCount1..10 `Alt+1`..`Alt+0`、GridToggleDetailsView `Alt+-` (P)

### Grid (Ph4)
- GridCursorRight/Left/Up/Down `矢印` (P)(予約候補) / GridOpen `Enter`(予約) /
  GridParentFolder `Backspace` (P) / GridToggleCheck `Space` (P)
- GridTreeFolderPrev/Next `Ctrl+↑/↓` (P) / GridSiblingFolderPrev/Next `Ctrl+PageUp/Down` (P)
- GridParentAlt `Alt+↑` (P) / GridHistoryBack/Forward `Alt+←/→` (P)
- GridHome/End/PageUp/PageDown (P)(予約候補)
- GridToggleFolderTreePane `F` (P) / GridToggleMaximize `F11` (P) /
  GridDelete `Delete` (P) / GridTagApply `T` (P) / GridTagView `Ctrl+T` (P) / GridRotateCw `R` (P) / GridRotateCcw `L` (P) /
  GridPin `P` (P) / GridCompareX `X` (P)
- GridApplyErase1/2 `F7/F8` / GridApplyConceal1/2 `F9/F10` /
  GridDeleteEraseMask `Shift+F7/F8` / GridDeleteConcealMask `Shift+F9/F10` (P)
- RatingItem1..5/Clear `F1-F6` / RatingContainer1..5/Clear `Shift+F1-F6`
  (専用 `[Rating]` グループ。グリッド / 画像フルスクリーン / 動画フルスクリーンで共有)
- GridClipboardCopy/Cut/Paste `Ctrl+C/X/V`、D&D、右クリック操作は
  OS/clipboard/マウス経路を含むため本計画では固定。
- Shift+矢印の範囲選択は GridCursor の派生動作として固定。

### FsSharedNavigation (画像 / 動画で実際に共有)
- FsClose `Esc` (予約) / FsImageClose `Enter` (画像のみ予約) / FsParent `Backspace`(予約)
- FsCtrlNavPrev/Next `Ctrl+↑/↓` (P) / FsSiblingPrev/Next `Ctrl+PageUp/Down` (P)
- FsToggleWindowMode `F11` (P)。native 動画経路では App 側 keymap の effective chord を
  presenter へ転送する global snapshot に載せる。
- レーティングは専用 `[Rating]` グループの `RatingItem*` / `RatingContainer*` を共有する。
- BrowserBack/Forward、マウス戻る/進むは
  `Settings.ring_shortcuts` の固定入力レイヤーで扱う。戻る/進むは環境設定「マウスボタン」で
  コンテキスト別に個別割り当て、通常ホイール、Ctrl+ホイール、クリックは固定。

### FsImage (Ph2) ★
- FsToggleMetadata `I`,`Tab` (P) ★: 画像フルスクリーンのメタデータパネル固定表示トグル。
  動画フルスクリーンには対応する固定右パネルがないため FsImage 専用とし、動画ヘルプ /
  native 動画 shortcut snapshot には載せない。
- FsNextImage `→` / FsPrevImage `←` / FsNextImageV `↓`,`Shift+↓` /
  FsPrevImageV `↑`,`Shift+↑` (P)(矢印 = 予約候補) / FsFixedJumpNext/Prev `Shift+→/←` /
  FsHome/End (予約候補)
- FsPagePrev/Next は既定未割り当て。矢印キーを固定扱いに残しつつ、前 / 次ページを別キーへ明示割り当てするための Action。
- FsStackJumpPrev/Next `Shift+↑/↓` (P) ★: スタックフラット読書中だけ有効。非スタック時の
  `Shift+↑/↓` は従来どおりプレーン `↑/↓` エイリアスとして固定扱い。
- FsContinuousScrollForward/Back `PageDown/PageUp` (P)(連続読み時のみ) /
  FsFixedJumpPrevNoRtl/NextNoRtl `PageUp/PageDown` (P)(通常ページ単位表示のみ、RTL で反転しない)
- FsSpreadShiftLeft/Right `Ctrl+←/→` (P)
- FsSlideshow `S` ★ / FsSpaceCheck `Space` (スライドショー停止またはチェックトグル) ★
- FsRotateCw `R` ★ / FsRotateCcw `L` ★ / FsImageAnalysis `Shift+Z` ★ / FsPanorama `V` ★ / FsPixelGrid `G` ★ (旧 `FsAnalysis = Z` を v2.0.0 で改名・既定変更)
- FsLoupeLockToggle `M` (P) ★ / FsLoupeHold `Shift` (MH, Ph6。修飾↔修飾のみ)
- FsEraseMode `E` ★ / FsConcealMode `Ctrl+M` ★ / FsBgCycle `B` ★
- FsExport `Ctrl+E` ★ / FsCapture `Ctrl+S` ★
- FsCompareToggle `X` ★ / FsCompareCycle `C` ★ / FsCompareWipe `Shift+C` ★ / FsCompareDiff `Alt+C` ★
- FsSpreadSingle/L/LCover/R/RCover `1`..`5` (P) ★
- FsReadingFlowCycle `6` (P) ★ / FsReadingDirectionToggle `7` (P) ★ / FsFitModeCycle `0` (P) ★
- FsAiModelNext `U` / Prev `Shift+U` / Reset `Alt+U` ★ / FsDenoiseCycle `N` ★
- FsPostFilterNext `T` / Prev `Shift+T` / Reset `Alt+T` ★
- FsAdjustSlot1..10 `Ctrl+1`..`Ctrl+0` (P) ★ / FsClearAdjust `Ctrl+Backspace` ★
- FsPin `P` ★ / FsApplyErase1/2 `F7/F8` / FsApplyConceal1/2 `F9/F10`
  (family・予約候補)
- FsMetaUndo/Redo (handle_meta_undo_keys、family・実コード確認) /
  FsPipelineDebug `Ctrl+Alt+Shift+D` (固定)
- FsOriginalPreviewHold `RightCtrl` (Windows) / `Num0` (非 Windows fallback) は OS 状態参照のため固定。

### FsVideo (Ph5、VK 経路)
- VideoPlayPause `Space`,`Enter` / VideoExternalPlayer `Shift+Enter`
- VideoSeekBack/Fwd `←/→` (修飾で 5/1/30 秒 = MVP固定。将来は `VideoSeekFwd5/1/30` 等へ分割)
- VideoFrameStepBack/Fwd `Ctrl+Shift+←/→`
- VideoSeekStart `W` / VideoVolumeUp/Down `Shift+↑/↓` / VideoNextFile `↓` / VideoPrevFile `↑`
- VideoMute `M` / VideoLoop `L` / VideoMarkerPrev `J` / VideoMarkerNext `K`
- VideoPin `P` / VideoPerfOverlay `F` / VideoTileMode `S` / VideoBookmark `B`
- VideoCapture `Ctrl+S`
- FsToggleWindowMode `F11` (P、FsCommon。動画 native presenter 経路も同じ Action の
  effective chord で転送する)
- VideoCompareNoop `X/C/Shift+C/Alt+C`、タイル中カーソル、Ctrl+ホイール列数切替は固定。
- native presenter 側の `native_video_fullscreen_shortcut_key` に載るキーだけ UI へ転送されるため、
  Ph5 ではこの whitelist と `KeyAction` を同時に更新する。Global の
  `ToggleDetachedViewerMode` は例外的に native 動画転送対象へ含める。

### Erase (Ph3) ★
- EraseConfirm `E`,`Esc` / EraseConfirmPolygon `Enter` / EraseUndo `Ctrl+Z` /
  EraseDeleteShape `Delete`
- EraseToolSelect `S` / Brush `B` / Lasso `L` / Polygon `P` / VLine `V` / HLine `H` /
  Line `I` / Rect `R` / Ellipse `O`
- ErasePaintMode `D` / EraseEraseMode `F`
- EraseSpacePan `Space` (KH, Ph6) / EraseNudge 矢印 (固定候補) / EraseRotate `[`,`]`
- EraseSwallowNumbers `0`..`9` は旧スロット誤動作防止の no-op 消費なので固定。

### Conceal (Ph3) ★
- ConcealExit `Ctrl+M`,`Esc` / ConcealConfirm `Enter` / ConcealUndo `Ctrl+Z` /
  ConcealDelete `Delete`
- ConcealTypeCycle `T` / ConcealPixelGrid `G` / ConcealPreset1..4 `1`..`4` /
  ConcealPaintMode `D` / ConcealEraseMode `F`
- ConcealToolSelect/Brush/Lasso/Line/VLine/HLine/Rect/Ellipse `S/B/L/I/V/H/R/O`
- ConcealSpacePan `Space` (KH, Ph6) / ConcealNudge 矢印 (固定候補)

### Crop (Ph3) ★
- CropExit `Esc`(予約) / CropExecute `Ctrl+E` / CropSpacePan `Space` (KH)

### Text (Ph3) ★
- TextConfirm `Ctrl+T` / TextRedo `Ctrl+Y`,`Ctrl+Shift+Z` / TextUndo `Ctrl+Z`
- TextSpacePan `Space` (KH)
- TextDeleteChar `Delete`,`Backspace` は編集中の text field / IME と絡むため固定候補
- TextCancel `Esc`(予約)

### LocalAdjust (Ph3) ★
- LaShowSource `Q` / LaShowMask `W` / LaPaintAdd `D` / LaPaintErase `F` / LaExit `Esc`(予約)
- LaToolBrush/EdgeBrush/GapFill/Lasso/Polygon/Select/Line/VLine/HLine/Rect/Ellipse
  `B/A/G/L/P/S/I/V/H/R/O`
- LaSpacePan `Space` (KH)
- Ctrl / Ctrl+Shift の source/layer bypass は一時 modifier 状態を使うため固定。

### 固定・対象外として明示するもの
- Gamepad: `src/app/gamepad_input.rs` の閲覧専用ボタン/軸入力。
- Mouse: 通常ホイール、Ctrl+ホイール、クリック、D&D、右クリックメニューは固定。
  右ドラッグ、戻る/進むは `Settings.ring_shortcuts` で限定カスタマイズ。
- Clipboard/delete files: `Event::Copy` / `Event::Cut`、Win32 クリップボード paste、
  ファイル削除ワーカー起動。
- OS 状態参照: RightCtrl original preview、native presenter の一部 routing。

> 上記は実装開始前の棚卸し版。**実際の variant 名・デフォルト chord・customizable は、各
> フェーズで対象ファイルを開いて確定する**。`docs/keymap-spec.md` とコードの差異は、配線する
> フェーズでこの付録へ戻して同期する。

## 付録 B. KeyName ⇔ egui::Key ⇔ VK 変換ホワイトリスト

双方向に安全変換できるキーのみカスタム許可。リスト外 (記号の一部・非 US 配列・
メディアキー) は ini で警告して無視。テンキー数字は `Numpad1` などの名前を受け付けるが、
egui 側で通常の数字キーと同じ `Num1` などに畳まれるため別アサインはできない。

| 分類 | KeyName | egui::Key | VK (16) |
|---|---|---|---|
| 英字 | A..Z | A..Z | 0x41..0x5A |
| 数字 | Num0..Num9 | Num0..Num9 | 0x30..0x39 |
| Fキー | F1..F24 | F1..F24 | 0x70..0x87 |
| 矢印 | Left/Right/Up/Down | Arrow* | 0x25/0x27/0x26/0x28 |
| ナビ | Home/End/PageUp/PageDown | 同名 | 0x24/0x23/0x21/0x22 |
| 編集 | Space/Enter/Esc/Tab/Backspace/Delete | 同名 | 0x20/0x0D/0x1B/0x09/0x08/0x2E |
| 記号 | OpenBracket/CloseBracket/Semicolon/Colon/Comma/Period/Backslash/Slash/Minus | 同名 (`?` は `Shift+Slash` として扱う) | 0xDB/0xDD/0xBB/0xBA/0xBC/0xBE/0xDC/0xBF/0xBD (※配列依存・要実機確認) |

修飾: Ctrl/Shift/Alt のみ。Win キー・AltGr は対象外。

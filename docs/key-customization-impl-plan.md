# キーカスタマイズ機能 実装プラン (簡易版 / テキスト ini)

§1.662 (2026-10-30): `FsPageOverviewToggle` と `ImagePageOverview` の短い操作名を
「ページ一覧の表示切替」に統一する。コマンド一覧・割り当て編集・候補・ガイドで使い、
短縮後も「表示切替」を残す。詳細欄には標準Oキーとの対応と再実行で一覧を閉じる説明を出す。
保存ID（`FsPageOverviewToggle` / `image_page_overview`）、既定O、文脈・受付・開閉処理は維持する。
一覧中のヘルプ題名「本のページ一覧」と、本を一覧で開く／閲覧を閉じて一覧へ戻る別操作は変更しない。

§1.477 (2026-10-29、実装済み・実機未確認): 一覧と画像フルスクリーンのコピー5種を
キー / リング / ジェスチャ / マウスボタン / Xリングへ揃える。
`GridCopyFiles` / `FsCopyFiles`、`GridCopyPath` / `FsCopyPath`、
`GridCopyFileName` / `FsCopyFileName`、`GridCopyImage` / `FsCopyImage`、
`GridCopyEditedImage` / `FsCopyEditedImage` をそれぞれ `Grid` / `FsImage` の `Press` とする。
既存 `GridCopyFiles` のCtrl+Cを維持し、他はすべて既定なし。
ring対応は `CopyFiles` / `ImageCopyPath` / `ImageCopyFileName` / `ImageCopyToClipboard` /
`CopyEditedImage`。旧image系の保存IDは変更せず、新IDは `copy_files` / `copy_edited_image`。
共通modelの `item_copy_command` がメニューと割り当て操作の適格性を所有する。
対象targetは `copy_subject` に通常項目 / ブックマークsource1件 / 利用不可を捕捉する。
ブックマークのパスは専用メニューと同じ本のsourceをコピーし、チェックを無視する（利用者判断2026-10-29）。
他の新規3操作はブックマークで、新規4操作はドライブ項目で何もしない。既存 `GridCopyFiles` は変更しない。
対象・チェック集合・見開き・非対応項目の扱いは [キー仕様](keymap-spec.md#コピー操作の割り当て1477) を正とする。
コマンド一覧のキー列は実フォントの最長chord幅を予約し、1 chordを1行で表示する。
親のwrap継承と幅の未実測による途中改行を解消し、既存の縦スクロールと狭幅の横clipを維持する。

左右ドラッグズームの `Fs/Erase/Conceal/Crop/Text/LaScrubZoomHold` は各既存 context の
KeyHold（既定 Ctrl+Space）。この6 action だけ action 別の `FullChord` policy を選び、
INI・設定復元・GUI 入力／capture が共有する action 検証へ通す。従来の KeyHold の
`SinglePlainKey` は維持する。Windows の held 判定は既存の完全一致 OS level をそのまま使い、
非 Windows も chord の修飾を完全一致させる。native 動画の対象 scope は拡張しない。
共通 overlay helper が Crop の Ctrl 併用 Space パンより scrubby を優先する。
元画像の右 Ctrl ホールドは変更しない。[入力所有と倍率制限](keymap-spec.md#左右ドラッグによる連続ズーム)。

2026-10-26 §1.444: `FsListOrderPrev` / `FsListOrderNext` は `FsCommon` / Press、
既定 `Ctrl+Shift+Up` / `Ctrl+Shift+Down`。説明は「一覧の表示順で前 / 次の本を開く」。
`EachPhysicalPress` で物理押下を数えるが、移動の準備中は既存 gate で拒否し累積しない。
egui の repeat-only フレームは既存 Ctrl+↑↓ と同じく最大 1 回へまとめ、native は既存 Ctrl+↑↓ と同じく repeat を無視する。
egui と native 動画の resolver / dispatcher、双方の動画ヘルプへ登録する。
通常フォルダ一覧から最後に本を開く操作を受け付けた時点の列を使い、DFS / 兄弟移動は維持する。
詳細は [一覧順ナビゲーション設計](list-order-book-navigation-plan.md) を参照。

2026-10-22: crop / SNS 分割の固定 Escape は、ドラッグ中なら開始前の状態へ戻してドラッグだけを取り消し、
ドラッグが無ければ従来どおりモードを終了する。消しゴム / 補正レイヤーと同じモード内の取消優先であり、
キー割当や新しい KeyAction は追加しない。通常 release（窓外で離した OS level の確認を含む）の確定と
取消の二終端を既存 editor 所有者で扱う。詳細は `preset-and-adjustment.md` の crop 節。

2026-10-17 §1.394: `ClearVideoMediaMemory` / `ClearBookResumePositions` を Global / Press、
既定キーなしで追加。既存の常に最前面と同じ各入力受付から、保存済み位置の管理処理へ渡す。
環境設定と消去対象を共通化し、動画は live のみ変更、本は既存 writer の非同期 clear を使う。
完了・空・失敗は既存トーストへ出す。保存・Remote・入力所有の契約は
[keymap-spec.md](keymap-spec.md#保存済み位置の全件クリア1394) を参照。

2026-10-11 §1.242 G3: `FsViewSpread` / `FsViewLandscapeSplit`を通常のfullscreen Action登録へ追加。
既定キーなし、保存済みbindingの移行なし。構成本/A2では閲覧mode-only、通常本では既存表示ownerへ渡す。

2026-10-14 §1.242 Q31: 構成本の既存見開きActionは、構成の方向×表紙に一致すればmode-onlyとして受け付ける。
既定2=`FsSpreadLtr`（左→右・表紙なし）、3=`FsSpreadLtrCover`（左→右・表紙あり）、
4=`FsSpreadRtl`（右→左・表紙なし）、5=`FsSpreadRtlCover`（右→左・表紙あり）、NumPadも同じ。
既存Action・既定割当を変更せず、現在のkeymapを使って一致Actionのキーを拒否通知とHUDに表示する。
一致Actionが未割当なら`FsViewSpread`の割当、それも未割当ならHUDの「見開き」を案内し、方向/表紙変更はPCの構成編集へ案内する。
HUDの横長分割は`FsViewLandscapeSplit`のカスタム割当だけを表示し、拒否対象の旧8/9を表示しない。
固定Pad Selectは構成本の単ページと一致見開きを循環し、picker/キー/リングは共通setterへ通す。
Remoteの旧`SetSpread`は型付き受付で最新構成との一致を検証し、既存mode-only保存へ通す。
不一致はWeb固定キーの一致見開きとPC編集を案内する。IPC形状は変更せず、構成/方向/表紙/休眠shiftには書き込まない。
構成なし本と抽出一覧の既存操作は変更しない。正本は本構成計画§9.2と`keymap-spec.md`冒頭。

> ステータス: **実装済み / 保守メモ**。v1.1.0+。
> 設計の経緯・調査・代替案は [archive/ui-input/key-customization-plan.md](archive/ui-input/key-customization-plan.md) に残す。
> 簡易版の実装判断は本書を優先し、差異が出たら本書へ集約する。
> 本書は「簡易版 (テキスト ini / GUI なし / 競合は警告のみ)」を実際に作るための手順書。
>
> **現状メモ (2026-06-25)**: この簡易版で導入した `keymap.ini` は、コマンド設定 GUI へ進むため
> 初回起動時の移行元になった。現在の正本は `Settings.keymap` (`settings.db`) で、旧
> `%APPDATA%\mimageviewer\keymap.ini` が残っている場合は 1 回だけ読み込んで同じ override を
> settings.db に保存し、`keymap.ini.imported*.bak` へ退避する。設定メニュー「操作カスタマイズ…」から
> `Settings.keymap` を編集できる。`keymap.ini.default` は Action 名と既定キーの参照として引き続き生成する。

> §1.250: `ToggleAlwaysOnTop` は Global / Press / 既定 `none`。main、egui fullscreen、native video の各入力経路へ配線し、native presenter 用の allowlist と chord snapshot にも含める。

> §1.374: `GlobalEverythingSearch` は検索範囲を表す名前だが **Grid / Press / Ctrl+R**。
> main の一覧だけから起動し、IME/text focus/focus recovery、modal、Remote lock 中は発火しない。
> fullscreen / detached viewer / native 動画へ起動を転送しない。`ALL_ACTIONS`、生成default、
> menu metadata、inventory と同時に更新する。keymap のscope解決・再割当/none・競合警告と
> native転送除外をpure/fake-inputテストで確認し、main/IME/detachedの受付はApp-levelで確認する。
> `Ctrl+G/S/E` は維持し、固定R/VK probeだけを根拠に新actionをdispatchしない。

関連: [keymap-spec.md](keymap-spec.md) (現行キー仕様 = アクション洗い出しの元ネタ)、
[archive/ui-input/key-customization-plan.md](archive/ui-input/key-customization-plan.md) §8 (簡易版の設計確定事項)。

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
  同時 active になり得る既存割り当てや、Esc / 修飾なし矢印の予約キーに重なる場合は
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
- Esc ナビゲーション、修飾なし矢印ナビゲーション、OS clipboard、
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
  Shift / Alt + ホイールの一般カスタマイズは、グリッド / 画像 / 動画でルーティング差が大きいため
  将来の別フェーズにする。編集モードの筆系ツールでキャンバス上だけ有効な Shift+ホイールは、
  この一般設計を再開せず、マスク筆半径の固定入力として扱う。
- **入力パターンは 3 種、差し替え方向は固定** (design doc §8.6):
  - **Press** (通常キー押下、M=ルーペ・ロックトグル含む): キー↔通常キー / 修飾↔修飾 自由
  - **Modifier-hold** (Shift ルーペ): 修飾↔修飾のみ
  - **Key-hold** (Space パン): キー↔通常キー。左右ドラッグズームの6操作だけ修飾付き chord も許可
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
pub enum Chord {
    Key { ctrl: bool, shift: bool, alt: bool, key: KeySlot },
    Modifier(ModKind),
}

/// Ctrl / Shift / Alt は左右不問、RightCtrl / RightShift / RightAlt は右側限定。
/// 左側限定は用途がないため定義しない。
pub enum ModKind { Ctrl, Shift, Alt, RightCtrl, RightShift, RightAlt }

/// テキスト名で持つ物理キー。Win32 VK / scan code を正本にし、egui は fallback として使う。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeySlot { A, B, /* ...Z */ Num0, /* ...Num9 */ Numpad0, /* ...Numpad9 */ NumpadEnter, F1, /* ...F24 */
    Left, Right, Up, Down, Home, End, PageUp, PageDown,
    Space, Enter, Esc, Tab, Backspace, Delete,
    OpenBracket, CloseBracket, Semicolon, Colon, Comma, Period, Backslash, Slash, Minus,
    JisCaret, JisAt, IntlYen, IntlRo }

impl KeySlot {
    pub fn parse(s: &str) -> Option<KeySlot>;   // "P" "F7" "Left" "[" "Numpad1" "NumpadEnter" 等
    pub fn to_egui(self) -> Option<egui::Key>;
    pub fn from_egui(k: egui::Key) -> Option<KeySlot>;
    pub fn to_vk(self) -> u32;                  // native 動画経路 / Win32 fallback
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
# FsNavigatorHold = Shift      ; ナビゲータ保持の修飾を Alt → Shift (ModifierHold = 修飾のみ)
# FsLoupeHold = Ctrl          ; ルーペ保持の修飾を Shift → Ctrl (ModifierHold = 修飾のみ)
# FsOriginalPreviewHold = none ; 元画像表示保持を無効化

[Erase]
# EraseToolSelect = A

[Text]
# TextRedo.1 = Ctrl+Y
# TextRedo.2 = Ctrl+Shift+Z
```

パース規則:
- 行 = `Action = <chord>` または `Action.<番号> = <chord>`。番号は 1..3 のみ。
  末尾数字ではなく `.` で分けることで、`RatingItem1` / `FsAdjustSlot1` のような Action 名と衝突させない。
- chord = `+` 区切り。`Ctrl`/`Shift`/`Alt` を通常 chord の修飾として拾い、残り 1 トークンを
  `KeyName::parse`。修飾単独は ModifierHold 専用で、左右不問の 3 種に加えて右側限定の
  `RightCtrl` / `RightShift` / `RightAlt` を受理する。
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

    /// (D-1) ModifierHold: 解決済み修飾を読む (ナビゲータ / ルーペ / 元画像表示)。
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
| **6** | **hold 系の上書き対応**: ModifierHold (navigator / loupe / original preview) / KeyHold (Space パン) を `modifier_held`/`key_held` 経由に。Alt→Shift、Shift→Ctrl、RightCtrl→none、Space→P のような同種差し替えを確認 | ui_fullscreen.rs / ui_erase.rs / ui_conceal.rs | 小 | ナビゲータ / ルーペ / 元画像表示の修飾差し替え・解除 / パンキー差し替え |
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

1. **Esc と修飾なし矢印ナビは固定扱いのまま残す。Enter / Backspace / Home / End / PageUp / PageDown は文脈別 `KeyAction` として扱う。**
   - 理由: Esc は再割当でモード脱出不能になりやすく、修飾なし矢印は RTL 反転・見開き 2 ページ送り・動画シーク粒度と絡む最低限の閲覧ナビ。
   - Enter / Backspace / Home / End / PageUp / PageDown は操作単位が明確になったため、コマンド設定・競合検出・ヘルプ表示へ載せる。
2. **動画シーク** は Small / Medium / Large の前後6 Action を使う。
   Small の既定は Shift+左右、Large は Ctrl+左右、Medium は既定割り当てなし。
   修飾なし左右は固定ナビゲーションのまま Medium の秒数を参照し、タイル中はカーソル移動する。
   秒数は Settings で小1 / 中5 / 大30秒を既定に各1～600秒で設定し、同じ段階の実行経路で共有する。
3. **IME ガード維持**: 文字キー Action は既存 `ime_input_active()` / `dialog_*_pressed` の
   ガードを**helper の外側で従来通り**通す。helper は純粋に「このキーが押されたか」だけ答える。
4. **exact match**: §4 の「修飾完全一致」を helper 内で保証。同一キーに NONE と SHIFT が
   ある場合 (例 C / Shift+C / Alt+C、矢印 alias) は、デフォルト chord を複数持たせて
   現行挙動を再現する。`consume_key(Modifiers::NONE, key)` へ直に寄せない。
5. **物理キー slot**: v2.2.0 から `KeySlot` を正本にし、テンキー数字は通常数字とは別 slot として扱う。
   日本語キーボード固有キー (`^` / `@` / `¥` / `＼`) は egui fallback ではなく Win32 key edge queue で判定する。
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
11. **レーティング F1-F6** は `[Rating]` コンテキストの `RatingItem1..5/Clear/StepUp/StepDown` と
   `RatingContainer1..5/Clear/StepUp/StepDown` に集約する。§1.417で `RatingBookPreferred1..5/Clear/StepUp/StepDown` を追加し、8操作とも既定キーなしとする。既存2系統のStepUp/StepDownも既定キーなし。グリッド / 画像フルスクリーン / native 動画の
   入口は `Keymap::consume_rating_action` / `native_video_rating_action` を使い、保存処理は既存の
   `set_rating` / `set_current_folder_rating` 経路を共有する。
   入力の対象系統は `RatingTargetFamily::{Item, Container, BookPreferred}` で表し、
   `consume_rating_action(ctx, family)` と `RatingKey.family` に渡す。nativeの照合順は
   Container → Item → BookPreferredとし、既存2系統の優先を保つ。表示用の
   `rating_key_action` / `first_rating_chord_label` / `rating_chord_summary_label` は既存のbool引数のまま、
   アイテム / コンテナ用だけを扱う。本を優先する入力は一覧ではアイテム用へ、閲覧中は採用済みrecipeに
   基づく単一resolverへ渡す。対象解決・保存の詳細は [§1.417 の設計](book-preferred-rating-plan.md) を参照。
12. **生 Event::Key / OS 状態参照経路** (pipeline debug=pipeline_debug.rs:104 など) は特殊。
    元画像表示は `FsOriginalPreviewHold` へ昇格し、Windows では割り当てた修飾キーの OS 状態を読む。
    旧 F11 window mode 経路は `FsToggleWindowMode` へ昇格済みで、native 動画 presenter も
    global shortcut snapshot 経由で同じ effective chord を使う。
13. **ゲームパッドは固定**: `src/app/gamepad_input.rs` は閲覧専用の物理ボタン/軸入力として扱い、
    keymap.ini の対象にしない。docs/keymap-spec.md のゲームパッド節と同じ方針。detached viewer と
    メイングリッドが並行表示される場合の発火面解決も固定入力レイヤーで行い、OS 前面 HWND、
    判定不能時は最後に操作した面、面が明示されるマウスイベントはその呼び出し元を使う。

---

## 8. テスト計画

`cargo test --bin mimageviewer-core` (App 系テストは `--lib` に出ない: MEMORY 参照)。

- **ini パース**: 正常 / `.1..3` サフィックス集約 / Action 名末尾数字との非衝突 /
  未知 Action / セクション不一致警告 / 変換不能キー / パターン不一致 /
  修飾単独 / `none` 無効化 → それぞれ warnings と overrides を検証。
- **解決ロジック**: 上書きあり/なし、複数チョード順、全置換セマンティクス。
- **exact match consume/pressed**: 素キー override が Shift/Ctrl/Alt 付きイベントを誤消費・誤発火しないこと。
  `C` / `Shift+C` / `Alt+C`、`T` / `Shift+T` / `Alt+T`、矢印 alias を代表ケースにする。
- **KeySlot 往復**: `from_egui ∘ to_egui` / `to_vk` / Win32 key edge queue の整合、egui fallback 不能キーが `None` を返すこと。
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

§1.470: `FsZoomIn` / `FsZoomOut` は `FsImage` / `Press` のキーボード専用操作。
既定は拡大 `Period`（`.`）/ `NumpadAdd`、縮小 `Comma`（`,`）/ `NumpadSubtract`。
表示の `+` は settings.db / JSON 保存時には既存の `Shift+;` を維持する。chord 解析は既存の
`Shift+;` / `Shift+Semicolon` に加えて表示形の `+` / `Ctrl++` 等も受理する。
末尾の `++` は通常の `+` 区切りと区別して Shift+Semicolon のキーを表し、編集欄初期値・
キャプチャ結果の表示形を再解析して保存できる。既定 chord の表示往復には右修飾キーの表示名も含める。
ini 出力のみ `;` / `#` を含む表記を KeySlot の別名に置換し、`Shift+Semicolon` /
`Semicolon` と書く。旧 ini 取り込みでもコメントに切られず、その別名を受理する。
既存の全 Action 列挙でヘルプ / キーカスタマイズへ表示し、リング系の対応表は増やさない。
欠けた override は通常の既定へ解決するため、新しい移行処理は不要。
詳細は [キー仕様](keymap-spec.md#キーによる段階ズーム1470) を参照。

2026-10-07 §1.349 / §1.343: 操作カスタマイズの対応表は
`GridOrganizeFiles` → `RingActionId::GridOrganizeFiles` (Grid)、
`FsFitModeCycle` → `RingActionId::ImageFitModeCycle` (ImageFullscreen)。
リング／パッド、ジェスチャ、マウス3ボタンは既存の対応表・候補フィルタを共有し、
既定割り当てや保存形式は変更しない。整理先は既存要求生成へ、fitはキーと同じ循環へ接続する。

2026-10-09 §1.342: fitの通常順はPage → Width → Height → Original → SpecifiedScale → Page。
指定サイズも同じ `FsFitModeCycle` / `RingActionId::ImageFitModeCycle` と除外ID集合を使う。
新しいAction・既定キー・cycle indexは追加しない。単ページ / 見開き / 縦横連結を対象とし、
動画・音声・編集中の適格性guardは維持する。直接選択pickerには除外にかかわらず5方式を出す。
指定幅・高さは既存fitメニューと環境設定で編集し、個別の指定サイズActionは設けない。

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
- GlobalEverythingSearch `Ctrl+R` (P): Grid文脈、main一覧専用。操作カスタマイズでは
  サムネイル一覧に表示。Everything検索バーのIME確定後Enter/検索ボタンが明示送信を行う。
  名前だけでGlobalに分類せず、fullscreen active scopes/native転送へ追加しない。
- GlobalLocalSearch `Ctrl+F` (P) / GlobalFavSearch `Ctrl+S` (P) /
  GlobalMetadataSearch `Ctrl+G` (P) / GlobalOpenFolder `Ctrl+O` (P) /
  ToggleDetachedViewerMode `F12` (P) / HelpShowContextShortcuts `?` (P)
  - 検索・フォルダを開く系は dialog / address / search focus / fullscreen 中は既存ガードで無効化される。
  - ToggleDetachedViewerMode は dialog / text focus / IME 変換中 / 静止画 fullscreen 編集サブモードでは抑止し、fullscreen / native 動画では明示 consume する Global action。
  - GridFavoritePrev/Next、GridOpenFavorite1..20、GridOpenCurrentDriveRoot、
    GridOpenDriveC..Z、GridSwitchDriveC..Z、
    GridOpenLocation*、GridTogglePinnedTag1..20 は既定未割り当て。設定名も `Grid...` に揃え、
    サムネイル一覧 (`Grid`) の一発操作として扱う。操作カスタマイズでキーを
    割り当てると、グリッド表示中に場所移動またはピン留めタグの付与/解除を実行する。
  - GridClearRecentFolders / GridClearQuickFolderSlots も既定未割り当ての Grid 操作。
    フォルダバー設定メニューと同じ共有入口で、状態クリア・設定保存・完了トーストまでを実行する。
    後者は A/B の場所に加えてセッション中の一覧位置を全件消し、現在の一覧も先頭へ戻す。
  - ExternalToolPicker / ExternalTool1..10 / ExternalToolForContainer も既定未割り当ての
    Grid 操作 (P)。動的 Action は作らず、固定 10 スロットを登録順のツールへ対応させる。
    ツールの並べ替えでスロットとの対応が変わるため、設定 UI では番号を明示する。
- GridSelectAll `Ctrl+A` (P) / GridDeselect `Ctrl+D`,`Ctrl+Shift+A` (P) /
  GridCopyFiles `Ctrl+C` (P) / GridCutFiles `Ctrl+X` (P)
- GridColumnCount1..10 `Alt+1`..`Alt+0`、GridColumnCount11..20 は既定キーなし、GridToggleDetailsView `Alt+-` (P)

### Grid (Ph4)
- GridCursorRight/Left/Up/Down `矢印` (P)(予約候補) / GridOpenSelected `Enter` (P) / GridOpenExternalPlayer `Shift+Enter` (P) /
  GridParentFolder `Backspace` (P) / GridToggleCheck `Space` (P)
- GridTreeFolderPrev/Next `Ctrl+↑/↓` (P) / GridSiblingFolderPrev/Next `Ctrl+PageUp/Down` (P)
- GridParentFolder `Alt+↑` (P, `Backspace` と同一 Action の別既定割り当て) / GridHistoryBack/Forward `Alt+←/→` (P)
- GridMoveFirst/Last `Home/End` (P) / GridPagePrev/Next `PageUp/PageDown` (P)
- GridToggleFolderTreePane `F` (P) / GridToggleMaximize `F11` (P) /
  GridDelete `Delete` (P) / GridTagApply `T` (P) / GridTagView `Ctrl+T` (P) / GridRotateCw `R` (P) / GridRotateCcw `L` (P) /
  GridPin `P` (P) / GridCompareX `X` (P)
- GridApplyErase1/2 `F7/F8` / GridApplyConceal1/2 `F9/F10` /
  GridDeleteEraseMask `Shift+F7/F8` / GridDeleteConcealMask `Shift+F9/F10` (P)
- RatingItem1..5/Clear `F1-F6` / RatingContainer1..5/Clear `Shift+F1-F6`。
  RatingItemStepUp/StepDown・RatingContainerStepUp/StepDown は `none`。
  RatingBookPreferred1..5/Clear/StepUp/StepDownも全操作 `none`
  (専用 `[Rating]` グループ。グリッド / 画像フルスクリーン / 動画フルスクリーンで共有)
- GridCopyFiles / GridCutFiles は Shell clipboard 実行経路を保ったまま keymap 化済み。
  GridCopyPath / GridCopyFileName / GridCopyImage / GridCopyEditedImage は既定なしのPress（§1.477）。
  Paste `Ctrl+V`、D&D、右クリック操作そのものは OS/clipboard/マウス経路を含むため固定。
- Shift+矢印の範囲選択は GridCursor の派生動作として固定。

### FsSharedNavigation (画像 / 動画で実際に共有)
- FsClose `Enter` (画像のみ P) / FsBackToList `Backspace` (P) / `Esc` (固定)
- FsJumpFirst/Last `Home/End` (P)
- FsCtrlNavPrev/Next `Ctrl+↑/↓` (P) / FsSiblingPrev/Next `Ctrl+PageUp/Down` (P)
- FsListOrderPrev/Next `Ctrl+Shift+↑/↓` (P): 最後に通常フォルダ一覧から本を開いたときの一覧順。準備中は受付拒否、累積なし
- FsToggleWindowMode `F11` (P)。native 動画経路では App 側 keymap の effective chord を
  presenter へ転送する global snapshot に載せる。
- レーティングは専用 `[Rating]` グループの `RatingItem*` / `RatingContainer*` / `RatingBookPreferred*` を共有する。
- BrowserBack/Forward、マウス戻る/進むは
  `Settings.ring_shortcuts` の固定入力レイヤーで扱う。戻る/進むは環境設定「マウスボタン」で
  コンテキスト別に個別割り当て。動画・音声の修飾なしホイールは前後ファイル移動（既定）/
  音量調整を選べる。その他の通常ホイール、Ctrl+ホイール、クリックは固定。detached viewer
  表示中はマウスイベントの受信面をコンテキスト解決へ明示的に渡し、`fullscreen_idx` だけから
  発火面を推定しない。

### FsImage (Ph2) ★
- FsToggleMetadata `I`,`Tab` (P) ★: 画像フルスクリーン / native 動画 / egui 音楽ビューの
  左右パネル表示モードトグル。3 面共通操作になったため Action context は FsCommon とし、
  native 動画の転送と競合検出も同じ effective chord に追従する。egui 0.33 は `begin_pass` で
  event consume より先に Tab traversal を決めるため、全 egui Context の `on_begin_pass` で
  TextEdit / IME の状態にかかわらず Tab の focus 方向を最初の focusable widget 登録前に
  `None` へ戻す。Tab traversal はアプリ全体で無効だが event は後段の Keymap まで残すため、
  Tab chord は引き続き任意の KeyAction へ割り当て可能。KeySlot と egui event queue の同一押下は
  Keymap ownership 境界で一緒に consume する。no-repeat の Tab repeat も発火させず除去する。
  TextEdit / IME の keyboard owner gate は先に維持し、矢印の field-level focus lock / 復帰と
  直前 pass の helper-managed field に対する `FocusRecovery` claim は `ime_focus` が別に所有する。
  Windows の allow_ime が true の場合、egui-winit は IME 処理済み Process press を遮断する。
  本家 0.34.0 #7967 の HashSet を物理キーごとの Hidden / Forwarded に置換し、配送済みは Process repeat でも維持する。
  release は allow_ime にかかわらず状態を除去して Hidden のみ遮断し、focus 喪失で全状態を消す。
  helper の IME 専用 Tab lock は置かない。
  本体 Win32 queue / native 生キーも 0xE5 の press / repeat を公開せず release を維持する。
  IME gate は今 pass 反映後の composing のみで、Commit 後の通常キーは直ちに通す。
  focus 復帰は所有 claim と分離し、pointer 無し・他 widget focus 無し・composing または Enter / Escape 無しを
  条件とする。helper retry は次 pass 限りで同条件に従い、呼び出し側の初回 focus latch には書かない。
  Hover side panel 内の helper field が実 focus を持つ間は panel lifetime も維持し、widget を描かなかった
  ことによる egui end-pass の dead-man focus clear を起こさない。これは非 IME の focus recovery ではなく、
  focus が別 widget へ移れば解除される。`[text-input-key]` の通常ログには helper field の id 継続、focus 前後、
  owner / phase、side-panel close call site を恒久的に残す。無修飾の文字 key / physical key は
  `Char` へマスクして入力内容を残さない。通常 record は process あたり 1 MiB で抑止し、id / focus /
  owner / close の異常 record は上限後も残す。helper focus contract がない pass は event list を走査しない。
- FsNextImage `→` / FsPrevImage `←` / FsNextImageV `↓`,`Shift+↓` /
  FsPrevImageV `↑`,`Shift+↑` (P)(矢印 = 予約候補) / FsFixedJumpNext/Prev `Shift+→/←` /
  FsHome/End (予約候補)
- FsPagePrev/Next は既定未割り当て。矢印キーを固定扱いに残しつつ、前 / 次ページを別キーへ明示割り当てするための Action。
- FsStackJumpPrev/Next `Shift+↑/↓` (P) ★: スタックフラット読書中だけ有効。非スタック時の
  `Shift+↑/↓` は従来どおりプレーン `↑/↓` エイリアスとして固定扱い。
- FsContinuousScrollForward/Back `PageDown/PageUp` (P)(連続読み時のみ) /
  FsFixedJumpPrevNoRtl/NextNoRtl `PageUp/PageDown` (P)(通常ページ単位表示のみ、RTL で反転しない)
- FsSpreadShiftLeft/Right `Ctrl+←/→` (P): §1.364の共通受付で、適格な本の実内部anchorを
  全体記憶設定に従って保存する。通常一覧/記憶OFFはsession調整、Single/Splitは従来移動。
  リング/マウス/パッドの前/次版も同じ受付を使う。HUDの基点jump/解除/記憶checkboxは
  既存popupから同ownerを呼び、新KeyActionや既定キーを追加しない。
- FsSlideshow `S` ★ / FsSpaceCheck `Space` (スライドショー停止またはチェックトグル) ★
- FsRotateCw `R` ★ / FsRotateCcw `L` ★ / FsImageAnalysis `Shift+Z` ★ / FsPanorama `V` ★ / FsPixelGrid `G` ★ (旧 `FsAnalysis = Z` を v2.0.0 で改名・既定変更)
- FsNavigatorToggle `Alt+N` (P) / FsNavigatorHold `Alt` (MH。修飾↔修飾のみ)
- FsPageTurnAnimationToggle 既定なし (P、2026-10-11 §1.341追補)。その他の機能のON/OFF行と同じKeyAction経由。環境設定と共通の全体設定を通常/F12で切り替える。表示項目カスタマイズやリングActionは追加しない。
- FsLoupeLockToggle `M` (P) ★ / FsLoupeHold `Shift` (MH, Ph6。修飾↔修飾のみ)
- FsOriginalPreviewHold `RightCtrl` (MH。Windows は右側限定、egui fallback は Ctrl に縮退)
- FsEraseMode `E` ★ / FsLocalAdjustMode `Ctrl+G` ★ / FsConcealMode `Ctrl+M` ★ / FsBgCycle `B` ★
- FsExport `Ctrl+E` ★ / FsCapture `Ctrl+S` ★
- FsCopyEditedImage (P、既定未割り当て): フルスクリーンの「編集を反映した画像をコピー」と共通の受付。
  Ctrl+E と同じページ／見開き対象に、`bake_stage_clipboard` の段までを原寸で反映する。
- FsCopyFiles / FsCopyPath / FsCopyFileName / FsCopyImage は既定なしのPress（§1.477）。
  コピー5種は画像FSとGridのring候補にも対応し、動画・音声FSには追加しない。
- FsCompareToggle `X` ★ / FsCompareCycle `C` ★ / FsCompareWipe `Shift+C` ★ / FsCompareDiff `Alt+C` ★
- FsSpreadSingle/L/LCover/R/RCover `1`..`5` (P) ★
- FsReadingFlowCycle `6` (P) ★ / FsReadingDirectionToggle `7` (P) ★ / FsFitModeCycle `0` (P) ★
- FsAiModelNext `U` / Prev `Shift+U` / Reset `Alt+U` ★ / FsAiModelAuto・各モデル直接切替 (既定未割り当て) ★ / FsDenoiseCycle `N` ★
- FsPostFilterNext `T` / Prev `Shift+T` / Reset `Alt+T` ★ / 各ポストフィルタ直接切替 (既定未割り当て) ★
- FsAdjustSlot1..10 `Ctrl+1`..`Ctrl+0` (P) ★ / FsClearAdjust `Ctrl+Backspace` ★
- FsPin `P` ★ / FsApplyErase1/2 `F7/F8` / FsApplyConceal1/2 `F9/F10`
  (family・予約候補)
- `UndoLastAction` (Global / Press、既定 `Ctrl+Z`) はグリッド・画像ビューア共通の本追加一回取消 / メタデータUndo入口。
  有効な追加receiptを先に扱い、処理中・失効検出を含め同じ入力で古いメタUndoへ続けない。
  編集専用UndoとTextEdit / IME / dialogの所有を優先する。追加自体のRedoはない。
  メタRedo (`Ctrl+Y` / `Ctrl+Shift+Z`) は従来の固定入力を維持 /
  FsPipelineDebug `Ctrl+Alt+Shift+D` (固定)

### FsVideo (Ph5、VK 経路)
- VideoPlayPause `Space`,`Enter` / VideoExternalPlayer `Shift+Enter` /
  VideoCloseFullscreen (既定未割り当て、動画フルスクリーンを閉じる追加キー)
- `←/→` (修飾なしキーは固定、中シークの設定秒数を使用) / VideoSeekBackSmall/ForwardSmall `Shift+←/→` (P) /
  VideoSeekBackMedium/ForwardMedium (既定未割り当て) / VideoSeekBackLarge/ForwardLarge `Ctrl+←/→` (P)
- VideoFrameStepBack/Fwd `Ctrl+Shift+←/→` (P)
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
- EraseConfirm `E`,`Esc` / EraseConfirmPolygon `Enter` / EraseRedo `Ctrl+Y`,`Ctrl+Shift+Z` /
  EraseUndo `Ctrl+Z` / EraseDeleteShape `Delete`
- EraseToolSelect `S` / Brush `B` / Bucket `K` / Lasso `L` / Polygon `P` / VLine `V` / HLine `H` /
  Line `I` / Rect `R` / Ellipse `O`
- ErasePaintMode `D` / EraseEraseMode `F`
- EraseSpacePan `Space` (KH, Ph6) / EraseNudge 矢印 (固定候補) / EraseRotate `[`,`]`
- ErasePreviewHold `Tab` (KH): 目のボタンと同じ保存なし結果プレビュー。保持中だけ表示し、描画中の押下は離すまで拒否する。入力所有・実割り当て表示は既存 helper を使う。
- ConcealPreviewHold `Tab` (KH): 隠蔽加工の同じ結果プレビュー。保持受付・ボタン合成・描画抑止は消しゴムと共通。pipeline 前の同期に受付済み保持を渡す。操作カスタマイズで変更・解除可。
- EraseSwallowNumbers `0`..`9` は旧スロット誤動作防止の no-op 消費なので固定。

### Conceal (Ph3) ★
- ConcealExit `Ctrl+M`,`Esc` / ConcealConfirmPolygon `Enter` /
  ConcealRedo `Ctrl+Y`,`Ctrl+Shift+Z` / ConcealUndo `Ctrl+Z` /
  ConcealDeleteShape `Delete`
- ConcealTypeCycle `T` / ConcealPixelGrid `G` / ConcealPreset1..4 `1`..`4` /
  ConcealPaintMode `D` / ConcealEraseMode `F`
- ConcealToolSelect/Brush/Bucket/Lasso/Polygon/Line/VLine/HLine/Rect/Ellipse
  `S/B/K/L/P/I/V/H/R/O`
- ConcealSpacePan `Space` (KH, Ph6) / ConcealNudge 矢印 (固定候補)

### Crop (Ph3) ★
- CropExit `Esc`(予約) / CropExecute `Ctrl+E` /
  CropForceCreateHold `Ctrl` (MH) / CropSpacePan `Space` (KH)
- Crop の pointer-down 分類だけは、実効 `CropSpacePan` と実効
  `CropForceCreateHold` の組合せを解決し、両方が押されていれば Pan を優先する。
  ただし `CropScrubZoomHold` が成立する押下は共通 overlay helper が先に所有する。
  汎用 KeyHold の完全一致規則は変更せず、余分な modifier group は拒否する。
  pointer-down 後は既存の Pan / Create / Edit owner に保持し、修飾キー変化で再分類しない。

### SnsSplit ★
- FsSnsSplitMode (既定未割り当て) / SnsSplitExecute (既定未割り当て)
- SnsSplitExit `Esc` は予約。既定の実行導線はパネルの「分割して書き出す」で、
  通常の FsExport `Ctrl+E` はモード中にそのボタンを案内する。

### Text (Ph3) ★
- TextConfirm `Ctrl+T` / TextRedo `Ctrl+Y`,`Ctrl+Shift+Z` / TextUndo `Ctrl+Z`
- TextSpacePan `Space` (KH)
- TextDeleteChar `Delete`,`Backspace` は編集中の text field / IME と絡むため固定候補
- TextCancel `Esc`(予約)

### LocalAdjust (Ph3) ★
- LaShowSource `Q` / LaShowMask `W` / LaPaintAdd `D` / LaPaintErase `F` / LaExit `Esc`(予約)
- LaToolBrush/Bucket/EdgeBrush/GapFill/Lasso/Polygon/Select/Line/VLine/HLine/Rect/Ellipse
  `B/K/A/G/L/P/S/I/V/H/R/O`
- LaSpacePan `Space` (KH)
- Ctrl / Ctrl+Shift の source/layer bypass は一時 modifier 状態を使うため固定。

### 固定・対象外として明示するもの
- Gamepad: `src/app/gamepad_input.rs` の閲覧専用ボタン/軸入力。
- Mouse: Ctrl+ホイール、編集キャンバス上の筆系ツールに限定した Shift+ホイール、クリック、D&D、右クリックメニューは固定。
  右ドラッグ、戻る/進む、動画・音声の修飾なし通常ホイールは `Settings.ring_shortcuts` で限定カスタマイズ。
  その他の通常ホイールは固定で、マウス入力全般は引き続き keymap.ini の対象外。
- Clipboard/delete files: `Event::Copy` / `Event::Cut`、Win32 クリップボード paste、
  ファイル削除ワーカー起動。イベント自体の固定扱いと、§1.477のコピー離散コマンドは区別する。
- OS 状態参照: native presenter の一部 routing。元画像表示の OS 状態参照は
  `FsOriginalPreviewHold` の ModifierHold helper 内で扱う。

> 上記は実装開始前の棚卸し版。**実際の variant 名・デフォルト chord・customizable は、各
> フェーズで対象ファイルを開いて確定する**。`docs/keymap-spec.md` とコードの差異は、配線する
> フェーズでこの付録へ戻して同期する。

## 付録 B. KeySlot ⇔ egui::Key ⇔ VK / scan code 変換ホワイトリスト

双方向に安全変換できるキーのみカスタム許可。リスト外 (PrintScreen, Pause, かな/無変換/変換,
メディアキーなど) は ini で警告して無視。テンキー数字は `Numpad1` などの名前で通常数字と別 slot として扱う。

| 分類 | KeySlot | egui::Key | VK / scan code |
|---|---|---|---|
| 英字 | A..Z | A..Z | 0x41..0x5A |
| 数字 | Num0..Num9 | Num0..Num9 | VK 0x30..0x39 |
| テンキー | Numpad0..Numpad9 / NumpadEnter | fallback なし (egui では Num0..Num9 / Enter に畳まれる) | VK 0x60..0x69 / Enter は extended bit |
| Fキー | F1..F24 | F1..F24 | 0x70..0x87 |
| 矢印 | Left/Right/Up/Down | Arrow* | 0x25/0x27/0x26/0x28 |
| ナビ | Home/End/PageUp/PageDown | 同名 | 0x24/0x23/0x21/0x22 |
| 編集 | Space/Enter/Esc/Tab/Backspace/Delete | 同名 | 0x20/0x0D/0x1B/0x09/0x08/0x2E |
| 記号 | OpenBracket/CloseBracket/Semicolon/Colon/Comma/Period/Backslash/Slash/Minus | 同名 (`?` は `Shift+Slash` として扱う) | 0xDB/0xDD/0xBB/0xBA/0xBC/0xBE/0xDC/0xBF/0xBD |
| 日本語キーボード固有 | JisCaret/JisAt/IntlYen/IntlRo | fallback なし | VK + scan code |

`Enter` / `NumpadEnter` は `VK_RETURN` を共有するため、Press、キャプチャ、native 動画だけでなく
KeyHold も Win32 edge の extended bit を正本にする。KeyHold の押下中状態は main / numpad 別に
`WM_KEYDOWN/WM_KEYUP` からラッチし、`GetAsyncKeyState(VK_RETURN)` や egui の
`physical_key = Enter` へフォールバックしない。対象 viewport の subclass 登録前など、送信元付き
ラッチを利用できないフレームは両方とも未押下として扱う。

修飾: Ctrl/Shift/Alt (左右不問) と RightCtrl/RightShift/RightAlt (右側限定)。
Win キーは対象外。右 Alt は RightAlt として指定できる。

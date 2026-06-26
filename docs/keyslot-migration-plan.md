# KeySlot 物理キー移行計画

## 目的

現在のキーカスタマイズは `egui::Key` 由来の論理キー名を正本にしているため、以下を正しく扱えない。

- メイン数字キーとテンキー数字キーの分離
- 日本語キーボード固有の `^` / `@` / `￥` / `ろ` などのキー位置
- 操作カスタマイズ画面での物理キー単位の割り当て表示・キャプチャ

v2.2.0 の操作カスタマイズはまだ未リリースなので、保存語彙をここで物理キー寄りに切り替え、後からの移行コードを不要にする。

## 方針

### 1. キーの正本を KeySlot にする

`Chord { ctrl, shift, alt, key }` の `key` は、文字ではなく「物理的なキー位置」を表す `KeySlot` を持つ。保存・入力名は既存互換を優先し、メイン数字は従来どおり `1` / `2` など、テンキーは `Numpad1` / `Numpad2` などで明示する。parser は `Digit1` のような別名も受け付ける。

ただし UI 表示はユーザーに見えるラベルを使う。

- `Num1` / `Digit1` は `1`
- `Numpad1` は `Numpad1`
- `NumpadEnter` は `NumpadEnter`
- `JisAt` は `@`
- `IntlYen` は `￥`（保存名は `Yen`）
- `IntlRo` は保存名 `Ro`（キーボード図の表示は `ろ`）

旧 `Num1` などの名前は、既存 keymap.ini 互換補助として parser でメイン数字キーへ読む。新しく生成する `keymap.ini.default` や設定保存では、メイン数字は `1`、テンキーは `Numpad1` のように表示する。

### 2. 入力トランスポートは Win32 の key edge queue

メインウィンドウは `App::update` で HWND を取得済みなので、そのタイミングで Win32 の `WM_KEYDOWN` / `WM_KEYUP` を拾うサブクラスを登録する。イベントから以下を取り出して、フレームごとの key edge queue に積む。

- virtual key (`WPARAM`)
- scan code (`LPARAM` bits 16..23)
- extended bit (`LPARAM` bit 24)
- repeat (`LPARAM` bit 30)
- down / up

ショートカットの発火はこの queue を消費して判定する。`GetAsyncKeyState` は修飾キーや hold 状態の補助として残すが、通常の press shortcut のエッジ検出には使わない。

### 3. TextEdit / IME が常に優先

Win32 key queue は mIV 内部のショートカット判定用であり、Windows メッセージ自体を握りつぶさない。既存の `shortcuts_blocked_by_text_input()` / `ime_input_active()` / `ctx.wants_keyboard_input()` ガードを維持し、テキスト入力中は queue をショートカットとして消費しない。

キーキャプチャはユーザーが明示的に待ち受けを開始したときだけ queue を読む。IME composing 中はキャプチャもしない。

### 4. 日本語キーボードとテンキー

日本語 Windows キーボードを前提にする。

通常の英数字・F キー・ナビゲーションキーは virtual key で同定する。テンキーと日本語固有キーは、必要に応じて scan code と extended bit を併用して同定する。

初期移行では以下の slot を用意する。

- `Num0..Num9`（メイン数字キー。`Digit0..Digit9` も入力名として受け付ける）
- `Numpad0..Numpad9`
- `NumpadAdd` / `NumpadSubtract` / `NumpadMultiply` / `NumpadDivide` / `NumpadDecimal` / `NumpadEnter`
- `JisCaret` (`^`)
- `JisAt` (`@`)
- `IntlYen` (`￥`。保存名は `Yen`)
- `IntlRo` (`ろ`。保存名は `Ro`)

### 5. 数字キーの既定割り当て

既存挙動との互換性を優先し、従来 `1` などに割り当てられていたコマンドは、既定ではメイン数字キー (`1`) と `Numpad1` の両方を割り当てる。

ユーザーは操作カスタマイズで片方を解除し、テンキーだけ別機能へ割り当てられる。

例:

```ini
FsSpreadSingle = 1, Numpad1
FsSpreadLtr = 2, Numpad2
```

### 6. native 動画との統一

native 動画 presenter は既に Win32 virtual key を受け取っている。`NativeVideoKeyEvent` に scan code / extended bit を追加し、メインウィンドウと同じ `KeySlot` 判定を使う。

これにより egui 経路と native 動画経路で同じ割り当てが効き、F11 や `?` のような個別 snapshot は最終的に `KeySlot` 判定へ寄せられる。

## 実装順

1. `KeySlot` 型と parser / display / Win32 matcher を追加し、`Chord.key` を `Option<KeySlot>` にする。（実装済み）
2. 数字キー既定割り当てをメイン数字 + テンキーに更新し、`keymap.ini.default` を再生成する。（実装済み）
3. メイン HWND に key edge queue を登録し、`Keymap::consume_action` / `pressed_action` / キャプチャを queue から読む。（実装済み）
4. native 動画の key event に scan / extended を追加し、`matches_vk_action` を `KeySlot` 判定へ寄せる。（実装済み）
5. 操作カスタマイズのキーボード図を `KeySlot` ベースにし、日本語キーボード配列とテンキーを表示する。
6. docs / tests を更新する。

## 実機確認ポイント

- メイン数字キー `1` と `Numpad1` が別々にキャプチャされる。
- 通常 Enter とテンキー Enter が別々にキャプチャされる。
- 既定状態では、従来 `1` で動いていた操作がテンキー `1` でも動く。
- `^` / `@` / `￥` / `ろ` がキーボード図から割り当てられ、実キーで発火する。
- Shift+数字は `Shift+Digit*` として、配列に関係なく物理数字キーで発火する。
- TextEdit / IME 入力中にショートカットが文字入力を奪わない。
- native 動画の F11 / `?` / カスタム割り当てが egui 経路と同じ結果になる。

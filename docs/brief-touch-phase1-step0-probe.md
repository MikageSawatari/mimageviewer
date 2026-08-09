# ブリーフ: タッチ対応 Phase 1 / Step 0 — 入力診断プローブ

対象: v2.13.0。実装 = Codex Sol / レビュー・検収 = ClaudeCode。
正本: [docs/touch-support-plan.md](touch-support-plan.md)。**着手前に §2.6 / §5.2 / §5.9 / §6 を読むこと。**

---

## 0. これは何か

タブレット PC のタッチ操作対応 (総計 22〜34 人日) の **Phase 1 の最初のステップ**。
実装ではなく **診断ログだけ**を入れる。

plan §6 に「実機確認が必要」として挙がっている未確認事項のうち、**コードが要るものを
1 回の実機セッションで一括回収する**のが目的。とくに §6-1 は
**Phase 1 の出荷ゲート**で、ここが崩れると動画側 (§5.9) の設計を引き直すことになる。

**このステップで新しい操作を実装してはいけない。** タッチでアプリの挙動が変わっては
ならない。入れるのは観測手段だけ。

---

## 1. 絶対制約 — 挙動ゼロ変更

**既定 (環境変数なし) では 1 行も新しいコードが実行されないこと。**
実機セッションで得たい情報より、既存のマウス・キーボード・動画再生を壊さないことが優先。

具体的に守ること:

- **新しい `match` アームを wnd_proc に追加しない。** 追加すると `WM_POINTER*` が
  現在の `_ => DefWindowProcW` から外れ、戻り値が変わる恐れがある。
  **`match` の手前 (プロローグ) で log 関数を 1 回呼ぶだけ**にして、`match` 本体は無改造で残す
- どのメッセージも消費しない。`LRESULT` を返さない。`return` しない
- `RegisterTouchWindow` / `EnableMouseInPointer` / `SetGestureConfig` を**呼ばない**。
  これらはメッセージ配送そのものを変える。プローブの目的は
  「**今の (登録していない) 構成で何が届くか**」の観測なので、呼んだ時点で意味が消える
- egui 側も同様に、イベントの読み取りだけ。`retain` / consume / 挿入をしない

---

## 2. 入れるもの

### 2.1 Cargo.toml

`windows` クレートの features に **`Win32_UI_Input_Pointer`** を追加する
(現在無い。`GetPointerType` / `GetPointerInfo` / `POINTER_INPUT_TYPE` / `PT_TOUCH` /
`PT_PEN` / `POINTER_FLAG_*` に必要)。**他の feature は増やさないこと。**

### 2.2 共通の gate ヘルパー

```rust
pub(crate) fn touch_debug_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("MIV_TOUCH_DEBUG").is_some())
}
```

既存の `hud_debug_enabled` ([native_presenter/render_core.rs:1496](../src/video/native_presenter/render_core.rs))
と同じ形。`OnceLock` で毎メッセージの `var_os` を避けること。

### 2.3 Win32 側 — presenter HWND と HUD HWND (§6-1 のゲート本体)

対象:

- presenter: `wnd_proc` ([video/native_window.rs:1111](../src/video/native_window.rs))
- HUD: `hud_wnd_proc` ([video/native_window_host/hud_window.rs:661](../src/video/native_window_host/hud_window.rs))

両方の `match msg { ... }` の**直前**に、共通の log 関数を呼ぶ 1 行を置く。
log 関数は片方に置いて他方から呼ぶ形でよい (どちらの HWND かは引数のラベルで区別する)。

**記録する内容**:

| メッセージ | 記録項目 |
| --- | --- |
| `WM_POINTERDOWN` / `WM_POINTERUP` / `WM_POINTERUPDATE` | pointerId、`GetPointerType` の結果 (`PT_TOUCH`/`PT_PEN`/`PT_MOUSE`/その他)、`GetPointerInfo` の `ptPixelLocation` (screen) と client 変換後、`pointerFlags` を 16 進、とくに `INCONTACT` / `INRANGE` / `PRIMARY` / `CANCELED` の有無 |
| `WM_POINTERENTER` / `WM_POINTERLEAVE` / `WM_POINTERCAPTURECHANGED` | 同上 (取れる範囲で) |
| `WM_NCPOINTERDOWN` / `WM_NCPOINTERUP` / `WM_NCPOINTERUPDATE` | 非クライアント側に流れていないかの確認用 |
| `WM_TOUCH` / `WM_GESTURE` | 届くこと自体が想定外なので、来たら記録する |

- pointerId は `LOWORD(wparam)`
- `GetPointerType` は **retire 後は失敗する**ので、DOWN 時点の結果をログに残すこと
  (キャッシュ機構までは作らなくてよい。プローブなので毎回呼んで失敗も記録する)
- **`WM_POINTERUPDATE` だけ rate limit する** (100ms 周期、既存の `WM_MOUSEMOVE` ログと同じ形)。
  DOWN / UP / ENTER / LEAVE / CAPTURECHANGED は**必ず全部出す** (取りこぼすと配送判定ができない)

**追加でマウス側にも 1 項目** (§6-2 用): 既存の `WM_MOUSEMOVE` / 各ボタンメッセージのときに
`GetCurrentInputMessageSource()` を呼び、`deviceType` (`IMDT_TOUCH` / `IMDT_PEN` /
`IMDT_MOUSE` / `IMDT_UNAVAILABLE`) と `originId` を記録する。
これで「タッチ由来の合成マウスがどう届いているか」が分かる。ボタン系は全部、
`WM_MOUSEMOVE` は既存 rate limit に相乗りしてよい。

### 2.4 egui 側 — イベント列の観測 (§5.2 の前提 / §6-3)

plan §5.2 は「`Touch(Start)` → `PointerMoved` → `PointerButton(pressed)`」という
**egui-winit 0.33.3 の実装契約**の上に立っている。実機で本当にこの並びが出るかを見る。
あわせて §6-3 (Touch Cancel で primary release が出ない件) を確認する。

- **フレーム内に `Event::Touch` が 1 つでもあるフレームだけ**、`ctx.input(|i| &i.events)` を
  順序どおり 1 行にまとめて出す。Touch が無いフレームは何も出さない
  (= マウスだけで使っている間はログが増えない)
- 各 `Event::Touch` は `device_id` / `id` / `phase` / `pos` / `force` を出す
- `PointerButton` は `button` / `pressed`、`PointerMoved` は座標、`PointerGone` はそのまま
- **`ViewportId` と frame 番号を必ず付ける** (メイン / フルスクリーンの区別が要る)

呼び出す場所は **各ビューポートの入り口**。CLAUDE.md の IME 節にあるとおり、
`show_viewport_immediate` は独立したイベントキューを持つので、片方だけでは足りない:

- メイン: `App::update` の先頭 (`self.update_ime_state(ctx)` の近傍)
- フルスクリーン: [ui_fullscreen.rs](../src/ui_fullscreen.rs) の `show_viewport_immediate`
  closure 先頭 (同じく `update_ime_state` の近傍)

---

## 3. 入れないもの (明示)

- タッチによる新しい操作・ジェスチャ認識・所有権 — **すべて Step 1 以降**
- `src/touch_input.rs` — 次のステップで作る。ここでは作らない
- `MIV_DISABLE_TOUCH_GESTURES` — 無効化する対象がまだ無いので不要
- `RegisterTouchWindow` 等の登録系 (§1 のとおり)
- ペンのホバー確認 (§5.16) の egui 側コード —
  **UI の見た目で観測できる** (かざしてサムネイルのツールチップ / ハイライトが出るか) ので
  コードは要らない。Win32 側は 2.3 の `PT_PEN` と `INCONTACT` フラグで分かる
- §6-4 (ClickToShow の callout がタッチで押せるか) のコード — **実機の目視で分かる**

---

## 4. 実機手順書

`docs/touch-probe-procedure.md` を新規作成する。**利用者がタブレットで上から順に踏むだけで
必要なログが揃う**ようにすること。ClaudeCode がこれをそのまま渡す。

含めること:

1. 起動方法 (`MIV_TOUCH_DEBUG=1` を付けた起動コマンド。PowerShell の `$env:` 形式で)
2. ログの出力先 (`%APPDATA%\mimageviewer\logs\mimageviewer.log`) と、
   **セッション前にログを退避しておく**手順
3. 操作シナリオ。各項目に「何を確かめているか」を 1 行添える:
   - a. 一覧でサムネイルを 1 本指でタップ / ドラッグ
   - b. 静止画フルスクリーンで 1 本指タップ / 2 本指ピンチ / ドラッグ中に指を画面外へ滑らせる (Cancel 誘発)
   - c. **動画を全画面再生し、HUD が出ていない場所 (映像の中央) をタップ** ← §6-1 の presenter 側
   - d. **HUD を出して、HUD のボタンの上をタップ** ← §6-1 の HUD 側。**ここが最重要**
   - e. 動画上で長押し ← §6-2。**フルスクリーンが閉じたら、閉じたことを報告に書く**
   - f. ClickToShow 設定にして、画面端をタッチして呼び出しバーを押してみる ← §6-4
   - g. ペンがあれば、一覧でペンをかざす (触れない) → ツールチップが出るか ← §5.16
4. 終了後にログファイルをどう渡すか

**UI 倍率 / DPI の網羅は Step 0 では要求しないこと** (Phase 1 の最後にまとめて行う)。
このセッションは「配送されるか否か」の一点に絞る。

---

## 5. 完了条件

- `MIV_TOUCH_DEBUG` 未設定で `cargo test -p mimageviewer --lib` が通り、
  既存の動画 / フルスクリーン挙動に差分が出ないこと
- `cargo fmt` (引数なし) を通すこと
- `cargo check` が Windows 構成で通ること。
  **非 Windows (ubuntu CI) でも壊さないこと** — Win32 部分は既存の `#[cfg(windows)]` 境界の
  内側に置く。CLAUDE.md「リリース手順 Phase 2 の 6.5」の cfg 漏れ番人に引っかからないこと
- 手順書 `docs/touch-probe-procedure.md` があること

## 6. 制約

- **アプリを起動しないこと。** 検証ビルドは ClaudeCode が `build-dev.ps1` で用意する
- ブランチ操作・コミットは不要。master の作業ツリーで作業する
- detached-rework 凍結ルールは有効。ただし本ステップは診断ログのみで
  detached 述語 / viewport 経路の**判定を変えない**はずなので、触れる必要が出たら
  **触らずに報告すること**
- **範囲を広げないこと。** 「ついでにタッチのここも直せそう」は Step 1 以降で扱う

完了したら、変更内容・触れたファイル・テスト結果・**ログに出る行の実例 (フォーマットが
分かるもの)** を報告すること。

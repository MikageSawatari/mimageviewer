# ブリーフ: 動画 native の `WM_POINTER` アダプタ (案 C の薄い縦切り)

対象: v2.13.0 Phase 1 の残件。実装 = Codex Sol / レビュー・検収 = ClaudeCode。
正本: [docs/touch-support-plan.md](touch-support-plan.md) §5.9 (案 C) / §5.10 / §5.12 / §2.6。

前提 (完了・コミット済み・実機確認済み):

- Phase 1 の egui 側一式 (`bb9574b2` 〜 `961bb47f`)
- Phase 2 の一覧スクロール (`266948f4` / `a6eeb386` / `edd00314`)
- **出荷ゲート §6-1 は通過済み**。実機ログで presenter HWND に `PT_TOUCH` が 237 件、
  HUD HWND に 35 件届くことを確認した → **案 C は成立する。設計の引き直しは不要**

---

## 1. このステップの目的

Phase 1 の締めくくりとして、**動画 native 側の入力経路を薄く 1 本通す**。
動画のタッチ操作を完成させるのは Phase 3 であって、**今回ではない**。

今回証明したいのは次の 4 点:

1. `PT_TOUCH` の stream を**丸ごと所有**でき、`DefWindowProc` を呼ばずに済むこと
2. その結果 **promoted mouse が生成されなくなる** = 二重発火しないこと
3. 座標変換 (screen → client → points) が実機で正しいこと
4. **egui 側と同じ認識器 (`src/touch_input.rs`) をそのまま再利用できる**こと

### 1.1 副産物として直る既知バグ (§2.6 / §6-2)

実機ログで確認済みの不具合:

> presenter 上で長押しすると、`WM_POINTERUP` の**後**に OS が同一ミリ秒で
> `WM_RBUTTONDOWN` + `WM_RBUTTONUP` を合成する。mIV はこれを「短い右クリック」と
> 分類してフルスクリーンを閉じてしまう。

案 C で stream を所有すれば `DefWindowProc` に渡らないので**合成自体が起きない**。
**症状パッチを当てるのではなく、この構造的修正で消えることを確認する**
(CLAUDE.md「バグ修正の一般原則」)。押下時間の測り方や右クリック判定には手を入れないこと。

---

## 2. 所有の単位と規約 (§5.9 案 C)

Microsoft は「**一つの pointer stream の一部だけを消費し、残りを `DefWindowProc` に渡す
動作は未定義**」と明記している。したがって:

> **stream 単位で、丸ごと所有するか丸ごと渡すかのどちらかにする。**

### 2.1 判定と所有

- `RegisterTouchWindow` を**呼ばない**
- `EnableMouseInPointer` を**呼ばない** (マウスにも `WM_POINTER` を生成させる
  プロセス全体の設定で、「マウス無影響」に対して逆効果)
- `WM_POINTERDOWN` で `GetPointerType` を呼ぶ
  - **`PT_TOUCH`** → その `pointerId` を**所有集合に登録**し、処理して **0 を返す**
  - それ以外 (`PT_PEN` / `PT_MOUSE` / 取得失敗) → **登録しない**。従来どおり
    `DefWindowProc` へ渡す。**取得失敗は「タッチではない」側に倒す** (fail-open = 現状維持)
- `WM_POINTERUPDATE` / `WM_POINTERUP` / `WM_POINTERCAPTURECHANGED` /
  `WM_POINTERENTER` / `WM_POINTERLEAVE` は **`pointerId` が所有集合にあるときだけ**
  処理して 0 を返す。無ければ `DefWindowProc`
  - **DOWN を見ていない `pointerId` の UPDATE を所有しないこと。**
    途中から所有すると「stream の一部だけ消費」になる

### 2.2 所有集合の寿命

- `WM_POINTERUP` / `WM_POINTERCAPTURECHANGED` / `POINTER_FLAG_CANCELED` で**必ず解放**する
- ウィンドウ破棄 (`WM_NCDESTROY`) で**全解放**する
- **上限を持たせる**こと。異常時に無限に溜まらないようにし、溢れたら診断ログに残す
- 状態は HWND ごと。既存の `GWLP_USERDATA` の state 構造体に持たせる

### 2.3 Cancel

- `POINTER_FLAG_CANCELED` が立っていたら `TouchPhase::Cancel` として認識器へ渡し、
  所有を解放する
- `WM_POINTERCAPTURECHANGED` も同様に Cancel 扱いにする
- **実機で Cancel は一度も観測できていない** (§6-3 は未検証のまま)。
  防御的に実装し、**観測できていないことをコメントに残す**こと

---

## 3. ⚠ 座標変換 — ここを間違えるとタップ位置がずれる

`WM_POINTER` の座標は **`WM_MOUSEMOVE` とは別物**である。

| | 座標系 | 単位 |
| --- | --- | --- |
| `WM_MOUSEMOVE` の `lparam` | **クライアント** | 物理ピクセル |
| `POINTER_INFO.ptPixelLocation` | **スクリーン** | 物理ピクセル |

したがって `ScreenToClient` を通してから、既存のマウス経路と**同じ**変換
(`native_pos()` 相当、`pixels_per_point` 除算) に合流させること。
**既存のマウス座標変換を複製せず、同じ関数を通す**こと。

- 変換部分は**純関数**にして unit test を書く (DPI 100/150/200% を含める)
- マルチモニタで presenter が跨っている場合も、`ScreenToClient` は HWND 基準なので
  そのまま成立するはず。前提をコメントに書くこと

---

## 4. イベントの transport

wnd proc は「分類して送るだけ」に留め、判断は presenter 側に置く (既存構造に合わせる)。

### 4.1 新しいイベント variant

`NativeVideoWindowEvent` に `Touch(NativeVideoTouchEvent)` 相当を追加する。
中身は認識器が必要とする最小限:

```
pointer_id: u32 (認識器の contact id へ)
x, y       : クライアント座標 (物理ピクセル)
phase      : Start / Move / End / Cancel
```

### 4.2 ⚠ 絶対に coalescing しないこと

`native_window_event_latest_slot()` ([src/video/native_window.rs:143](../src/video/native_window.rs))
は `MouseMove` などを「最新 1 件だけ残す」mailbox に載せている。

> **Touch イベントは必ず `None` を返すこと** (= coalescing しない)。

Start や End が 1 件でも落ちると認識器の contact 集合が壊れ、
「指を離したのに離れていない」「2 本目が消えてピンチが成立しない」になる。
Move も落とさないこと (移動量の積算が狂う)。

### 4.3 ルーティング

`NativeVideoWindowEventSink::send` ([src/video/native_window.rs:299](../src/video/native_window.rs))
の 2 つの `matches!` を両方更新すること。

- **render route へは送る** (認識器と overlay がここにいる)
- **pump route へ送るかは、下の 4.4 の結論に従う**

### 4.4 ⚠ タップでの前面化・フォーカスを落とさないこと

今は presenter へのタップが `WM_LBUTTONDOWN` に昇格するので、
`WM_MOUSEACTIVATE` や既存の focus / foreground 処理が動いている。
**stream を所有すると `DefWindowProc` を呼ばないので、この経路が丸ごと消える。**

→ **タップしたときの前面化・フォーカス・cursor ownership が従来どおりであることを
実装で担保すること**。既存のマウス down 経路 (`WM_MOUSEACTIVATE`、
`RequestFocusClaim`、`CursorOwnership`) を読み、必要なら Touch でも同じ typed 要求を
出す。**新しい focus 制御を発明しないこと** — 既存経路に合流させる。

detached / window モードでの前面化は既存の仕組みが所有しているので、
**そこへ症状パッチを足さない**こと (detached-rework 凍結ルール)。

---

## 5. 認識器の再利用

**egui 側と同じ `src/touch_input.rs` の `TouchRecognizer` をそのまま使う。**
native 用に別の認識器を書かないこと。案 C を選んだ理由がこれである。

- **`src/touch_correlation.rs` は使わない。**
  あれは「egui のマウスイベント列からタッチ由来を相関で見分ける」ための層で、
  native は Win32 から `PT_TOUCH` を**直接**受け取るので相関は不要。
  同じ理由で fail-closed の曖昧判定も不要
- 認識器の状態は presenter (overlay 側) が持つ。surface rect と現在表示中の
  クローム矩形を知っているのがそこだから
- `TapZoneGeometry` は:
  - `surface` = overlay の全体矩形 (points)
  - `excluded` = **現フレームで実際に表示されているクローム矩形**
    (上バー / 下 HUD / 左右パネル)。静止画側と同じ考え方
  - `behavior` = `TouchSurfaceBehavior::Viewer { accepts_pinch: false }`。
    **動画にピンチズームは無い** ので昇格させない (§5.14-10)

---

## 6. 配線するコマンドは 1 つだけ

### 6.1 単一タップ → クローム表示のトグル (§5.5)

plan §5.5「動画・音楽のタップ割り当て」より:

> **単一タップは画面全体で HUD 表示**。動画には守るべきページ送り領域が無いので、
> 中央矩形に限定する必要がない (動画プレイヤーの慣習どおり)。

したがって、認識器が返す **`ToggleChrome` と `PageSide { .. }` の両方**を
「クローム表示のトグル」に落とす。

- **認識器には手を入れないこと。** 認識器は「物理的に何が起きたか」を報告し、
  意味付けは surface が決める (前ステップからの一貫した設計)
- `PageSide` を潰さず残しておくのは、Phase 3 で**左右ダブルタップ → ±5 秒シーク**
  (§5.14-9) を載せる土台になるため。**今回はシークを実装しない**

### 6.2 クローム表示のラッチ

native のクローム可視判定は**ホバー位置の純関数**になっている
(`native_hud_bottom_visible_from_hover` / `native_hud_top_visible_from_hover`,
[src/video/native_presenter/render_core.rs:5594](../src/video/native_presenter/render_core.rs))。

→ **静止画側と同じ形にする**。ラッチを 1 つ持ち、**純関数の入力に 1 フィールド足して
OR する**。純関数のままにしておくこと (テストが既存パターンに乗る)。

ラッチの規約 (§5.5、静止画側と同一):

- 単一タップでトグル
- **ファイル移動・フルスクリーン終了で解除**する
- **時間で消さない** (指を離すとホバーが消えるタッチでは時間切れが事故になる)
- **永続設定を書き換えない** (`fullscreen_top_bar_locked` 等に触れない)
- ホバーによる従来の表示と**競合させない**。OR で足すだけにし、
  マウスでホバーしたときの挙動を変えないこと

---

## 7. pointer emulation と primary 抑止 (§5.10)

所有した stream からは、**先頭接点についてのみ** overlay の egui Context へ
pointer emulation を注入する。これで既存の overlay ウィジェットが
今までどおり反応する (= 触れる範囲を広げずに機能を保つ)。

- `PointerMoved` → `PointerButton { pressed: true }` → … → `PointerButton { pressed: false }`
  → 必要なら `PointerGone`
- **`TouchRecognizer::should_suppress_primary()` が true のときは press/release を注入しない。**
  タップ確定でコマンドを出したのに、同じ release で overlay のボタンも押されると二重発火する
- 2 本目以降の接点は emulation を出さない

**⚠ egui は 1 アプリフレームで複数 pass 走る** (§5.10)。native overlay も同様に
`render_once` が複数回走り得るなら、**コマンドは最初の 1 回だけ**返し、
後続 pass では空にすること。egui 側で実機バグ (中央タップが 2 回必要) を踏んでいる。
**呼び出し側に「実行済み」guard を置かず、返す側で落とす**こと。

---

## 8. 安全網 — 既存マウス handler の early filter (§5.9)

既存の `WM_MOUSE*` handler に `GetCurrentInputMessageSource()` の確認を足し、
**`IMDT_TOUCH` と確定した重複だけ**を捨てる。

- **失敗・`IMDT_UNAVAILABLE` のときは捨てない**。従来の mouse handler へ流す (fail-open)
- これは**安全網であって正本ではない**。正本は §2 の stream 所有
- signature は RDP 経由などで欠落する実例があるので、これに依存した設計にしないこと

---

## 9. ⚠ HUD HWND は今回の対象外 (意図的)

**presenter HWND だけを所有する。HUD HWND (`hud_window.rs`) には手を入れない。**

理由:

- HUD には既存のボタンが並んでおり、**今は promoted mouse で押せている**。
  ここを所有に切り替えると、pointer emulation の作り込みが甘い場合に
  **今動いているものが動かなくなる**。薄い縦切りで負う risk ではない
- presenter と HUD は**別 HWND = 別 stream** なので、片方だけ所有しても
  「stream の一部だけ消費」にはならない (§2 の規約に反しない)
- 長押し→フルスクリーン終了の実害は presenter 側で起きている。そこは今回で消える

**Phase 3 への申し送りとして plan §5.9 に明記すること**:

- HUD HWND の所有と pointer emulation は Phase 3 で行う
- **HUD 上の長押し→右クリック合成は今回残る**。presenter 側だけ直る
- HUD のボタンサイズ調整も Phase 3 (§5.12 の Phase 3 内訳どおり)

---

## 10. キルスイッチと診断

- **`MIV_DISABLE_TOUCH_GESTURES=1` で所有を一切行わない**こと。
  = 全て `DefWindowProc` へ流れ、**今日の挙動に完全に戻る**。切り分けに使う
- `MIV_TOUCH_DEBUG=1` の診断を拡張する ([src/touch_debug.rs](../src/touch_debug.rs) に既に
  Win32 pointer メッセージのログがある)。**追加で記録するもの**:
  - 所有判定の結果 (`owned` / `passed` と、passed の理由: 非 PT_TOUCH / 取得失敗 / 未登録 id)
  - 変換後のクライアント座標と points 座標
  - 認識器が返したコマンド
  - `GetCurrentInputMessageSource` の判定で mouse イベントを捨てたときの記録
- 診断ログは**残すこと** (実機の切り分けに使い続ける)

---

## 11. 入れないもの (範囲を広げない)

- **左右ダブルタップのシーク** (Phase 3)
- **動画のピンチ** (ズーム機能が無い)
- native `ScrollArea` の drag / 慣性 (Phase 3)
- HUD のターゲットサイズ調整 (Phase 3)
- 中央クローム内の左右ハンドル / エッジスワイプ (Step 3b)
- 再生・シーク・デコードなど**入力 transport 以外の動画処理**には一切触れない
- detached / media window の構造 (凍結ルール有効)

---

## 12. マウス無影響 (§5.15)

`MIV_DISABLE_TOUCH_GESTURES=1` なしの通常状態で、**マウスのみの入力列**は
一切変わらないこと:

- presenter 上の左クリック / 右ドラッグリング / ホイール / X ボタン
- HUD の全ボタン (今回触らないので当然だが、回帰が無いことを確認する)
- 長押しではない通常の右クリック
- ホバーによる上バー / 下 HUD / 左右パネルの表示

---

## 13. テスト

**純関数として unit test を書くこと**:

- **所有状態機械**: DOWN→UPDATE→UP で所有・解放 / DOWN→Cancel で解放 /
  capture changed で解放 / 非 PT_TOUCH は一度も所有しない /
  **DOWN を見ていない id の UPDATE を所有しない** / 2 stream 交互 /
  `GetPointerType` 失敗は所有しない / 上限超過
- **座標変換**: screen → client → points が DPI 100/150/200% で正しいこと
- **コマンド写像**: `ToggleChrome` と `PageSide` の両方がクロームトグルになること
- **クローム可視の純関数**: ラッチ ON でホバー無しでも表示 / ホバー時の従来挙動が不変 /
  ファイル移動と終了で解除
- **キルスイッチ**: `MIV_DISABLE_TOUCH_GESTURES=1` で所有 0 件
- **primary 抑止**: `should_suppress_primary()` が true のとき press/release を注入しない

Win32 呼び出しを直接テストできない部分は、**判定ロジックを引数で受ける純関数に切り出して**
テストする (`GetPointerType` の結果 / flags / 座標を引数にする)。

---

## 14. 完了条件

- `cargo fmt` (引数なし) を通すこと
- `cargo test -p mimageviewer --lib` が**全件**通ること (現在 4912 件)
- `cargo test -p mimageviewer --test ui_snapshot` が通ること
- `cargo check -p mimageviewer --bin mimageviewer-core` が通ること
- **非 Windows を壊さないこと** (CI の ubuntu `cargo check` が番人。
  新規コードは `#[cfg(windows)]` に閉じ、純関数テストは両方で通るようにする)
- **[docs/touch-support-plan.md](touch-support-plan.md) を更新**すること:
  - §5.9 に「presenter のみ所有、HUD は Phase 3」の判断と理由
  - §2.6 / §6-2 の長押しバグが構造的に解消される旨 (実機確認後に確定と書く)
  - §6-1 のゲート結果 (通過) と §6-3 (Cancel 未観測) の現状
- **[docs/architecture-overview.md](architecture-overview.md) と
  [docs/video-architecture.md](video-architecture.md)** に入力経路が増えたことを反映する
  (CLAUDE.md「コード修正時のドキュメント同時更新」)
- マニュアルは**まだ更新しない** (動画のタッチは Phase 3 で完成するため。
  更新が要ると判断した場合は理由を報告すること)

---

## 15. 制約

- **アプリを起動しないこと。** 検証ビルドは ClaudeCode が用意する
- ブランチ操作・コミットは不要。master の作業ツリーで作業する
- **診断ログは残すこと**
- **detached-rework 凍結ルールは有効**。detached 述語 / viewport 経路に触れる必要が
  出たら、症状パッチを入れずに**報告すること** (CLAUDE.md「Detached viewer リワーク中のルール」)
- **範囲を広げないこと**

---

完了したら次を報告すること:

1. **所有の実装場所と状態機械の形** (どこに集合を持ち、どこで解放するか)
2. **座標変換の合流点** (既存マウス経路のどの関数に合流させたか)
3. **前面化・フォーカスをどう保ったか** (§4.4)
4. **クローム可視の純関数へどう 1 フィールド足したか**
5. **複数 pass 対策をどこで落としたか** (§7)
6. テスト結果
7. **実機で確認してほしいこと**の一覧 (特に長押しでフルスクリーンが閉じないこと)

# タブレット PC タッチ操作対応 — 調査と設計方針

> 状態: **調査完了 / 設計検討中 (未実装)**。2026-08-06。
> 調査 = ClaudeCode + Codex Sol (read-only、3 ラウンド)、実装は未着手。
> きっかけ: 利用者からの「タブレット PC のタッチ操作に対応しているか」という質問。
>
> **確定した方針 (利用者判断 2026-08-06)**:
> ① 動画も対応必須 (アプリとしてタッチ対応を表明するため)。
> ② 左右パネルの操作もカバーする (AI アップスケール / カラー化 / ブックマーク / レーティング / タグ)。
> ③ 「選択済みセルの再タップで開く」も入れる (既存マウス操作に影響しない範囲で)。
> ④ **タッチ機能の ON/OFF 設定は作らない**。既存操作に影響を与えず、タッチしたときだけ別動作。

関連ドキュメント:

- [docs/fullscreen-side-panel-mode-plan.md](fullscreen-side-panel-mode-plan.md) — 左右パネルの `Hover` / `ClickToShow` モード
- [docs/ring-shortcut-plan.md](ring-shortcut-plan.md) — リングショートカット (右ドラッグ / ゲームパッド)
- [docs/keymap-spec.md](keymap-spec.md) / [docs/keyboard-input-ownership-plan.md](keyboard-input-ownership-plan.md) — 入力オーナーシップの既存境界
- [docs/video-architecture.md](video-architecture.md) — 動画 native presenter (独自 HWND)
- [docs/archive/ui-input/ui-scale-plan.md](archive/ui-input/ui-scale-plan.md) — UI 表示倍率 50-200%

---

## 1. 目的とスコープ

### 1.1 やること

**静止画・動画の「閲覧行動」をタブレットのタッチだけで完結できるようにする**。具体的には:

1. 一覧を指でスクロールする
2. フォルダ / サムネイルをタップして開く (**選択済みセルの再タップで開く**を含む)
3. フルスクリーンでページを送る (単ページ / 見開き / 本)
4. フルスクリーンで拡大縮小・パンする
5. 隠れている UI (上バー / 左右パネル) をタッチで確実に呼び出す
6. **フルスクリーンを閉じる / 親フォルダへ戻る**
7. **左右パネルの閲覧系操作** — AI アップスケール / カラー化 / ブックマーク / レーティング / タグ
8. **動画の再生・一時停止・シーク・前後ファイル・HUD 表示・閉じる**

### 1.2 やらないこと (当面)

- 編集系 (補正レイヤー / 消しゴム / 注釈 / モザイク / 表示トリム) のタッチ最適化
- 環境設定ダイアログ・各種管理ウィンドウのタッチ最適化
- ソフトウェアキーボードの自動表示制御 (検索・パス入力)
- スタイラス (筆圧・ホバー) を活かした専用機能
- タッチ専用の別レイアウト / 別スキン
- 利用者向けのタッチ設定項目 (ジェスチャ割り当て / 閾値 / ON-OFF)

これらは「使えなくはない (タップ = 左クリックとして動く)」状態を維持するに留める。

> ⚠ **ペンの扱いは未決定** (§5.14)。winit は指とペンを同じ `WindowEvent::Touch` に潰すため、
> 「ペンは従来どおりマウス扱い」を保証するには winit / eframe より下に source tag を足す必要がある。
> 何もしなければ**ペン接触もタッチとして解釈される**。

---

## 2. 現状調査 — タッチ入力はどこまで届いているか

### 2.1 入力経路の実態

`winit 0.30.13` → `egui-winit 0.33.3` → `egui 0.33.3` → mIV の経路を実装で確認した。

| 段 | 事実 | 出典 |
| --- | --- | --- |
| winit | digitizer があれば各 HWND を `RegisterTouchWindow(.., TWF_WANTPALM)` で登録 | `winit-0.30.13/src/platform_impl/windows/window.rs:1127-1134` |
| winit | `WM_TOUCH` を読んで `WindowEvent::Touch` に変換し、**`DefWindowProc` へ渡さず `Value(0)` を返す** | `winit-0.30.13/src/platform_impl/windows/event_loop.rs:1938-1982` |
| winit | `WM_POINTERDOWN/UPDATE/UP` も同様に `WindowEvent::Touch` へ | 同 `:1985-2129` |
| egui-winit | `WindowEvent::Touch` → `egui::Event::Touch` (全接点) | `egui-winit-0.33.3/src/lib.rs:338-349, 676-702` |
| egui-winit | **先頭 1 接点のみ**を `PointerMoved` + `PointerButton{Left}` にエミュレート。離すと **`PointerGone`** | `egui-winit-0.33.3/src/lib.rs:703-738` |
| egui | `zoom_delta()` は `MultiTouchInfo` の pinch 倍率を優先 (無ければ Ctrl+wheel) | `egui-0.33.3/src/input_state/mod.rs:630-645` |
| egui | 長押しを `Response::secondary_clicked()` として扱う | `egui-0.33.3/src/input_state/mod.rs:981-986`, `response.rs:174-179` |

**重要な帰結**: winit が `WM_TOUCH` を握って `DefWindowProc` に渡さないため、**egui ウィンドウでは
Windows によるタッチ→マウス合成 (mouse promotion) も既定の `WM_GESTURE` 変換も起きない**。
egui viewport で効いているのは、すべて egui-winit の「先頭接点＝左マウス」エミュレーションである。

### 2.2 mIV 側のタッチ利用状況

`Event::Touch` の参照は 3 箇所しかなく、**いずれもアプリ操作ではない**:

- `src/app.rs:18140` — 起動中の入力を捨てる種別リスト
- `src/app.rs:61809` — バックグラウンドインデクサの活動検知
- `src/ime_focus.rs:693` — IME フォーカス復帰の抑止判定

`MultiTouchInfo` / `multi_touch()` / `any_touches()` / `zoom_delta()` の利用箇所は **`src/` 配下にゼロ**。

つまり「タッチ入力が届いていない」のではなく、**単指を左マウスとして扱うところまでは届いているが、
複数接点・ジェスチャ意味付け・タッチ向け UI 可視性/ターゲットサイズが未接続**という状態。

### 2.3 到達度サマリ

| 操作 | 現状 | 理由 |
| --- | --- | --- |
| タップ = 左クリック | ✅ 効く | egui-winit のポインタ翻訳 |
| ドラッグ = 左ドラッグ | ✅ 効く | 同上 |
| 長押し = コンテキストメニュー | ✅ 効く見込み | egui が `secondary_clicked()` を合成 |
| ホバー | ⚠️ **指が触れている間だけ** | release で `PointerGone` |
| 右ドラッグ (リングショートカット) | ❌ 不可 | raw `PointerButton::Secondary` の down/move/up が来ない |
| ホイール (一覧スクロール) | ❌ 不可 | `process_scroll` は `raw_scroll_delta` のみ |
| ピンチズーム | ❌ 未接続 | 生データはあるが mIV が読まない |
| フリック / スワイプ | ❌ 未実装 | 速度・方向を読む処理が無い |

### 2.4 通常閲覧フローのタッチ可否 (Codex Sol 調査、file:line 付き)

| 操作 | 判定 | 根拠 |
| --- | --- | --- |
| 一覧の子フォルダを開く | **可能** | `Sense::click_and_drag()` + ダブルタップ `ui_main.rs:12028-12040, 12153-12176` |
| サムネイルの選択 | **可能** | `response.clicked()` `ui_main.rs:12111-12133` |
| サムネイルを開く | **可能** | ダブルタップで fullscreen open `ui_main.rs:12196-12226` |
| **一覧を指でスクロール** | **❌ 不可** | `process_scroll` はホイール専用 `app.rs:33489-33548`。加えて mIV は毎フレーム `vertical_scroll_offset()` を注入して egui の慣性スクロールを打ち消す `ui_main.rs:14485-14490`。さらに**セル自身が primary ドラッグを file D&D として奪う** `ui_main.rs:12325-12345` |
| 一覧スクロールバーのドラッグ | **一応可能・悪い** | 半行以上の差は読み戻す `ui_main.rs:14708-14715`。ただし幅は **10pt (floating 8pt)** で touch 用の hit 拡張なし `os_theme.rs:269-285` |
| フルスクリーンのページ送り | **可能** | 全面が `click_and_drag`、左半分/右半分タップで RTL 対応の page delta `ui_fullscreen.rs:1246-1249, 16246-16279` |
| 見開き / 本のページ送り | **可能** | `spread_page_nav()` で表示単位が移動 `ui_fullscreen.rs:11953-12003` |
| ピンチズーム | **❌ 不可** | 通常 zoom は Ctrl+wheel `ui_fullscreen.rs:16055-16073`、別経路は中ボタンドラッグ |
| ズーム中のパン | **条件付き可能** | primary ドラッグでパン `ui_fullscreen.rs:16127-16191`。ただしタッチだけではズーム状態に入れない |
| 連続読みのスクロール | **可能・慣性なし** | primary ドラッグ delta を渡す `ui_fullscreen.rs:16155-16173`。release 後の速度処理なし |
| **フルスクリーンを閉じる** | **❌ 既定設定では不可** | 正規経路は raw Esc / `FsClose` (既定 Enter) `ui_fullscreen.rs:12989-13022`, `keymap.rs:5228-5230`。× ボタンはホバー上バー内で、`fullscreen_top_bar_locked` は既定 false `settings.rs:5201` |

**最大の問題は最後の行**。タブレットでフルスクリーンに入ると、既定設定では**タッチだけでは出られない**。

### 2.5 ホバー依存 UI の棚卸し (通常閲覧フローのみ)

| UI | ホバー依存か | 出典 |
| --- | --- | --- |
| フルスクリーン右パネル (情報) | **依存**。右端 5% + sustain rect | `ui_metadata_panel.rs:48-76`, `ui_fullscreen.rs:1150-1179` |
| フルスクリーン左パネル (補正/編集) | **依存**。左端 5% + sustain rect | `ui_fullscreen.rs:1182-1202, 15798-15829` |
| `ClickToShow` の呼び出しバー | **依存**。`hover_pos()` が端 hit (24pt) に入って初めて clickable bar を生成 | `ui_helpers.rs:56-60`, `ui_fullscreen.rs:5409-5472` |
| フルスクリーン上部バー | **依存**。`locked` / 上端ホバー / 左右パネル表示 / popup 等の or | `ui_fullscreen.rs:1008-1027` |
| 動画 native 下 HUD | **依存**。下端から 220pt 以内 | `render_core.rs:4016-4025` |
| 動画 native 上 HUD | **依存**。非表示時 36pt / 表示後 76pt | `render_core.rs:4027-4038` |
| 動画 native 左右パネル | **依存**。端 5% | `render_core.rs:5615-5653` |
| 音楽ビューの上バー / 下 HUD | **非依存 (常時表示)** | `ui_fullscreen.rs:26733-26740, 27115-27161` |
| 音楽ビューの左右パネル | **依存**。端ホバー | `ui_fullscreen.rs:26818-26858` |
| メイングリッドのサムネイル | **ほぼ非依存**。選択枠・各種バッジは state 依存で常時描画。ホバー依存はツールチップとカーソル形状のみ | `app/grid_paint.rs:352-725`, `ui_main.rs:12088-12109, 14640-14662` |
| ツールバー / アドレスバー / ファセットバー | **非依存**。`show_toolbar_*` フラグで決まる | `ui_main.rs:6842-6860, 10659-10666, 8807-8815` |

→ **メイン画面は思ったほどホバー依存していない**。問題はフルスクリーンと動画 HUD に集中している。

### 2.6 動画 native presenter は事情が逆

動画の presenter HWND / HUD HWND は **mIV 自前の Win32 ウィンドウ**で、
`RegisterTouchWindow` も `WM_TOUCH` / `WM_POINTER` / `WM_GESTURE` ハンドラも持たない
(`src/video/native_window.rs:22-39`, `src/video/native_window_host/hud_window.rs:64-75`)。
未処理メッセージは `DefWindowProcW` に流れる (`native_window.rs:1478`, `hud_window.rs:1027`)。

→ ここでは **Windows の既定タッチ→マウス合成が効く**。結果:

- タップ → 左クリック合成 → 再生/一時停止等に入る
- 長押し → 右クリック合成 → mIV の native 右ボタン処理へ入る
- **ピンチ → Ctrl+wheel が来るが、動画のズームには繋がらない** (Ctrl+wheel はタイル表示の列数変更のみ、`render_core.rs:4244-4280`)
- **パン → `WM_VSCROLL/WM_HSCROLL` ハンドラが無く、何も起きない**

**潜在バグ (Codex Sol の推測、要実機確認)**: OS が長押し成立時点で短い `WM_RBUTTONDOWN/UP` を
生成する環境では、mIV が測る押下時間が元のタッチ開始ではなく右ボタン down からになるため、
**「短い右クリック」と誤分類されてフルスクリーンが閉じる**可能性がある
(`src/app/native_video.rs:8829-8855, 8898-8911, 9039-9052`)。

### 2.7 リングショートカットのタッチ対応は「設計上の表現だけ」

[docs/ring-shortcut-plan.md](ring-shortcut-plan.md) には「マウス/パッド/タッチで同じ操作感」という
記述があるが、これはリング方式一般の比較説明で 1 度出るだけ (`:24-33`)。
トリガの仕様 (`:56-108`) はマウス右ボタンとゲームパッド X+方向のみで、
実装も raw secondary / gamepad direction のみ
(`ui_main.rs:12044-12080`, `ui_fullscreen.rs:16327-16470`, `app/gamepad_input.rs:4267-4302`)。

ただし **リングの描画・方向量子化・`RingActionId` → `apply_ring_action()` の適用層は再利用できる**。

---

## 3. 競合 NeeView の方式 (ソースを読んで確認)

NeeView (WPF, C#) はタッチ対応済み。実装を読んで確認した事実を記す。

### 3.1 入力の取り方

WPF の **Stylus イベント** (`StylusDown/Up/Move/InAirMove/SystemGesture`) を使う。
`StylusDown` で `e.Handled = true` を立てて **WPF のマウス昇格を止める**。
`TouchConfig.IsEnabled = false` にすると「標準のマウス操作として認識」に戻る (= 昇格に任せる)。

状態機械 `TouchInputState = None | Normal | MouseDrag | Drag | Gesture | Loupe`。

### 3.2 タップ = 画面 5 領域コマンド (これが核心)

`NeeView/TouchInput/TouchArea.cs`:

```
TouchCenter : 0.33 < x < 0.66 && y < 0.75   (下 25% のページスライダー帯を除外)
TouchL1     : x < 0.5 && y < 0.5            (Center に当たらなかったものを 4 分割)
TouchL2     : x < 0.5 && y >= 0.5
TouchR1     : x >= 0.5 && y < 0.5
TouchR2     : x >= 0.5 && y >= 0.5
```

Center を優先判定し、外れたら左右×上下の 4 分割。**既定バインドは 3 つだけ**:

| 領域 | 既定コマンド |
| --- | --- |
| `TouchL1, TouchL2` | **NextPage** (右→左読み既定のため左が「次」) |
| `TouchR1, TouchR2` | **PrevPage** |
| `TouchCenter` | **ShowHiddenPanels** = `MainWindowModel.EnterVisibleLocked()` |

`EnterVisibleLocked()` は**自動的に隠れているパネル類を出して表示ロック**する
(他の操作で `LeaveVisibleLocked()`)。**ホバー依存 UI 問題への回答がこれ**。

### 3.3 ドラッグと長押し

| 入力 | 設定 | 既定 |
| --- | --- | --- |
| 単指ドラッグ (16px 超で判定) | `TouchConfig.DragAction` | **`Gesture`** = マウス右ドラッグと同じ軌跡ジェスチャー語彙 (`MouseSequence`) を共有 |
| 長押し→ドラッグ (`SystemGesture.HoldEnter` / `RightDrag`) | `TouchConfig.HoldAction` | **`Drag`** = 画像のパン/ビュー操作 |

`TouchAction = None | Drag | MouseDrag | Gesture | Loupe`。`Loupe` は長押し側でのみ選択可。

### 3.4 マルチタッチ

2 本指以上で即 `Drag` 状態 → `TouchDragManipulation` がピンチ拡大縮小 (`IsScaleEnabled`)、
回転 (`IsAngleEnabled`)、パン、**慣性** (`InertiaSensitivity` 既定 0.6) を処理。
しきい値は `MinimumManipulationRadius` 80px (2 接点間距離)、`MinimumManipulationDistance` 30px。

### 3.5 フリックは実装していない

設定 UI の説明文に **「フリック操作はジェスチャーで代用してください」** と明記されている
(`InputTouchControl.Remarks`)。速度ベースのフリック認識は持たず、軌跡ジェスチャーで代替する設計。

### 3.6 その他

- サイドパネル / フィルムストリップにタッチスクロールの終端バウンド設定 (`IsManipulationBoundaryFeedbackEnabled`)
- `TouchEmulateCommand` = マウスポインタ位置でタッチ領域コマンドを実行する (マウス利用者/デバッグ向け)

### 3.7 mIV が学ぶべき点 / 採らない点

**採る**:

- **Center タップで隠れ UI を出して固定する**。mIV の詰み (2.4 の「閉じられない」) を一撃で解消する。
- **フリックを実装しない**。タップ領域とドラッグで足りる。認識器の複雑さと誤爆に見合わない。
- **タッチ機能の ON/OFF 設定を持つ**。効かない環境・誤動作時の逃げ道。
- 2 本指ピンチ + 慣性のしきい値を設定で持つ。

**採らない / 保留**:

- **単指ドラッグ = 軌跡ジェスチャー (NeeView 既定)** は mIV には合わない。mIV は一覧グリッドが主画面で、
  単指ドラッグは**スクロールに使いたい**。NeeView の主画面は単一ページビューなのでこの既定が成立する。
- **5 領域フル実装**は初手には過剰。mIV は既に「左半分/右半分タップ = ページ送り」を持っているので、
  **Center 領域だけを足す**のが差分最小。
- 回転操作 (`IsAngleEnabled`) は mIV の回転が非破壊 DB 管理なので、ピンチ回転との相性を別途検討。

---

## 4. 現行版でできる回避策 (コード変更なし)

質問された利用者へ今すぐ案内できる内容。**これだけでフルスクリーンの「詰み」は回避できる**。

1. **環境設定 → 閲覧表示 → 「上部情報バーを固定表示」を ON** にする
   (`fullscreen_top_bar_locked`、`ui_dialogs/preferences/pages.rs:7047`)。
   → × ボタンが常に出るのでタップで閉じられる。**これが最重要**。
2. 同じ**閲覧表示**ページの「左右パネルの表示」を**「クリック表示」**にする
   (`FsSidePanelMode::ClickToShow`、`pages.rs:6957`)。
   → 端に触れただけで編集パネルが飛び出す事故が減る。
3. UI 表示倍率を上げる (設定 → スケーリング、最大 200%)。
   → ボタン・スクロールバーが物理的に大きくなる。
4. ページ送りは画面の左半分/右半分タップで既に効く (設定不要)。

**残る制約**: 一覧スクロールはタッチ不可 (スクロールバーを細い指で掴むしかない)、
ピンチズーム不可、リングショートカット不可。

---

## 5. 設計方針

ClaudeCode と Codex Sol の見解は主要点でほぼ一致した。以下は統合案。
未決の判断事項は §5.14、保証範囲の明文化は §5.15、実機確認項目は §6 に分けて記す。

**§5.2 が設計の核心**。「設定なし・マウス無影響・タッチのときだけ別動作」が
成立するかどうかは、入力源を確実に判定できるかにかかっている。

### 5.1 設計原則

1. **NeeView を丸ごと移植しない**。mIV には既存のページ送り・パン・ズーム・パネル資産があるので、
   新設するのは **タッチ固有の「入力認識」と「所有権」だけ**にする。
2. **マウスの既存挙動を一切変えない**。タッチ由来の入力にだけ新しい解釈を与える。
   これは「実行時状態で挙動を変えない (決定性優先)」方針と衝突しない
   — 環境を見て挙動を変えるのではなく、**入力ソースごとに解釈を分ける**だけだから。
3. **フリックを実装しない** (NeeView と同じ判断)。速度認識器の誤爆コストに見合わない。
4. **自動的なタッチモード切替をしない**。「最後の入力がタッチだったからレイアウトを大きくする」は
   採らない。「タッチイベントを受けたのでタッチ用クロームを出す」は、入力に対する直接的な反応なので採る。
5. **タッチ状態を `App` のグローバル bool / `Option` に置かない**。
   `ViewportId` + surface をキーにした `ctx.data_temp` に持つ
   (detached リワークの凍結ルールに抵触しないため。CLAUDE.md「Detached viewer リワーク中のルール」)。
6. **利用者向けのタッチ設定を作らない**。ジェスチャは固定マッピング。
   診断用の強制無効 (環境変数 / コマンドライン) だけは持つことを推奨 (§5.14-2)。

### 5.2 タッチと確定する条件 (fail-closed) — 設計の核心

「設定なし・マウス無影響・タッチのときだけ別動作」が成立するかは、**入力源を確実に
判定できるか**にかかっている。結論は **条件付きで成立する**。

#### 判定方法 — イベント列シグネチャによる相関

egui-winit 0.33.3 は 1 回の `on_touch()` 呼び出しの中で、次の順序でイベントを push する
(`egui-winit-0.33.3/src/lib.rs:676-738`):

```
開始: Touch(Start) → PointerMoved      → PointerButton(Primary, pressed)
移動: Touch(Move)  → PointerMoved
終了: Touch(End)   → PointerButton(Primary, released) → PointerGone
```

`InputState::events` は raw event 列を順序どおり clone している
(`egui-0.33.3/src/input_state/mod.rs:574`) ので、mIV からこの並びを観測できる。

したがって:

1. `Event::Touch` **だけ**から tap / pan / pinch を認識する (正本はタッチイベント列)
2. **直後のイベント列が上記シグネチャと完全一致した primary pointer だけ**を
   「この touch の合成 pointer」と関連付ける
3. タッチ専用 action を実行した場合だけ、同じ stream 由来の既存クリック action を抑止する
4. 一致しない pointer は**すべて従来のマウス入力として扱う**

> ⚠ **「同一フレームに Touch があったから、そのフレームのクリックは全部 touch」という
> 判定は不可**。マウス併用時に誤判定する。

Touch Start と primary press が別フレームにずれることは、この固定バージョンの egui-winit では
起きない (1 関数呼び出し内で連続 push されるため)。ただしこれは **egui の一般契約ではなく
egui-winit 0.33.3 の実装契約**なので、**依存更新時のイベント列回帰テストが必須**。

#### 誤判定の非対称性を利用する

| 誤判定の向き | 結果 | 許容 |
| --- | --- | --- |
| タッチを見逃してマウス扱い | 従来どおりの左クリックとして動く | **許容可** |
| マウスをタッチと誤認 | 再タップ open や中央タップ action が走る | **許容不可** |

→ **肯定証拠だけで新経路へ入る (fail-closed)** 設計にすれば、
マウス操作は原理的に壊れない。具体的には:

- exact な Touch イベントシグネチャがある場合**だけ**新 action を許可する
- 同一フレームに関連付けられない primary mouse event があれば新 action を**中止**する
- stream の target / owner が曖昧なら従来の pointer 経路へ戻す
- `zoom_delta()` 単独では入力源を判定しない (`multi_touch()` と active ownership を条件にする)
- **「最後の入力デバイス = タッチ」のようなグローバル状態を作らない**

#### 入力源ごとの成立度

| 入力源 | 判定 |
| --- | --- |
| マウスだけ | `Event::Touch` が無いので必ず従来経路。**完全に無影響にできる** |
| タッチだけ | raw Touch stream から肯定判定できる。**成立** |
| マウス + タッチ同時 | イベント列としては区別可能。ただし egui の `PointerState` は 1 つなので、**同一 surface で完全独立に操作させることは保証できない** |
| **ペン / スタイラス** | **指と確実に区別できない** (§5.14-1 で仕様を決める必要がある) |
| 精密タッチパッド | 通常は Touch ではなく mouse/wheel 経路。従来動作を維持できる |

#### 所有権のライフタイム

2 段階に分ける:

- **合成 primary pointer の所有**: 最初の接点の Start から、その接点の End/Cancel まで
- **マルチタッチジェスチャの所有**: 最初の接点から、参加した全接点が End/Cancel するまで

**2 本目が入った時点で pending の single tap を取り消し**、最初の指が先に離れても
tap / ページ送りを発火させないことが重要。
一度 pan / pinch / scroll に確定した stream は、**全接点が離れるまで別 action へ移さない**。

#### Cancel の穴 (実機確認要)

egui-winit は Touch Cancel 時に `PointerGone` を出すが、**primary release を出していない**
(`egui-winit-0.33.3/src/lib.rs:732-735`)。アプリ側の gesture state は Cancel で確実に破棄できるが、
**egui widget 側に primary-down 状態が残らないか**は実機確認が要る。
残る場合は入力アダプタ側で release を補う必要があり、**構造的な壁の一つ**。

### 5.3 操作マッピング (全体像)

| 入力 | 一覧グリッド | 静止画 / 本フルスクリーン |
| --- | --- | --- |
| タップ | セル選択 / 開く | **左 1/3 = ページ移動 / 中央 1/3 = クローム表示 / 右 1/3 = ページ移動** |
| 単指ドラッグ | **スクロール** (file D&D は抑止) | 既存のパン / 連続読みスクロール |
| 2 本指 | — | **ピンチズーム + 2 本指パン** |
| 長押し | コンテキストメニュー (既存のまま) | コンテキストメニュー (既存のまま) |
| リング | **接続しない** | **接続しない** |

左右の方向は既存の RTL 対応計算 `fullscreen_click_nav_base_delta` (`ui_fullscreen.rs:1246`) に流す。

**NeeView の 5 領域ではなく 3 領域**にする。mIV には上下分割へ割り当てたい既定コマンドが無く、
費用対効果が低いため。中央領域は、NeeView の固定比率 (下 25% 除外) ではなく
**実際に表示中の上バー (44pt) / 下シークバー (38pt) / 左右パネルの矩形を除外**する
(`ui_fullscreen.rs:927, 932`)。

### 5.4 一覧スクロール — anchor + fraction 方式

**太いスクロールバーやページ送りゾーンだけでは「タッチで快適」とは言えない**。
指で一覧を直接動かせることは必須。ただし `scroll_offset_y` をドラッグ中だけ自由値にするのは避ける
(行スナップは仮想表示・サムネイル保持の前提になっており、読み手が多い)。

```
正本    : scroll_offset_y  = 行境界にスナップされた anchor  (従来どおり不変条件を維持)
一時状態: fractional_drag_y = 0 .. cell_h 未満
描画位置: scroll_offset_y + fractional_drag_y
```

ドラッグが 1 行を越えるたびに `scroll_offset_y` を 1 行進め、`fractional_drag_y` から 1 行分を引く。
これで **`scroll_offset_y` は常に行境界のまま**になり、既存の読み手を触らずに済む。
指を離したら端数を最寄りの行へ確定する。

付随して必要な改修:

- 端数表示中は可視範囲の**末尾を追加で 1 行保持**する (でないと下端が欠ける)
- touch move ごとに「スクロール中」を明示的に通知する
  — `last_prefetch_scroll_at` / `last_scroll_event_at` / idle-upgrade の時刻を更新し、
  指を離すまで scroll settle を発火させない。
  現在の scroll 検出はオフセット変化の fallback に頼っており (`app/runtime_ops.rs:13-31`)、
  **端数だけ動いたフレームは検出できない**
- **タッチ由来のポインタストリームでは native file D&D を無効にする** (`ui_main.rs:12325`)。
  マウス D&D は維持する

**慣性は初期版では入れない**。「指に追従して動く + 離したら行スナップ」だけでも現状より大幅に改善する。
無制限の物理スクロールは PDF 先読み・eviction・idle upgrade との調整コストに見合わない。
実機評価後に必要なら、速度から最終到達行を決める限定形 (最大数画面・慣性中は prefetch 抑止・
最後は必ず行スナップ) だけを追加する。

### 5.5 中央タップとクローム

中央タップで左右パネル**そのもの**を開くのではなく、次を表示する:

- 上バー (× ボタンを含む) — **これでタッチだけで閉じられるようになる**
- 下シークバー
- 左右の**大きなパネル呼び出しハンドル** (現行 24pt hit / 20pt 表示より大きく)

状態は `TouchChromeLatched` 相当の **viewport 限定の一時状態**にする:

- 中央タップで表示 / 非表示をトグル
- ページ移動・フルスクリーン終了で解除
- **時間では消さない** (指を離すとホバーが消えるタッチでは、時間切れは事故になる)
- **`fullscreen_top_bar_locked` 設定は書き換えない** (永続設定を UI 操作で勝手に変えない)

既存モードとの関係:

| `FsSidePanelMode` | マウス | タッチクローム表示中 |
| --- | --- | --- |
| `Hover` (既定) | 従来どおり端ホバー | 明示ハンドルを表示 |
| `ClickToShow` | 既存の呼び出しバー | 大きなハンドルを表示 |

現行の呼び出しバーは端に `hover_pos()` があるときだけ生成される (`ui_fullscreen.rs:5422`)。
タッチクローム表示中は**このホバー条件を使わず**左右ハンドルを常時描画する必要がある。

→ 中央タップと `ClickToShow` は**衝突しない**。

上バーの可視判定は既に純関数 `still_top_bar_visible_from_inputs` (`ui_fullscreen.rs:1018-1027`)
なので、入力構造体に 1 フィールド足すだけで済む。テストも既存パターンに乗る。

### 5.6 ピンチズーム

`zoom_delta()` を `App::update` のグローバルで読むのではなく、**現在描画しているフルスクリーン
キャンバスの入力ハンドラで `multi_touch()` を読む**。モーダル / TextEdit / 左右パネル /
上下バー / シークバー / 編集モードの各ゲートより後で処理する。

**既存の Ctrl+ホイール分岐へそのまま合流させない**。`zoom_delta()` はマルチタッチが無ければ
Ctrl+ホイールの合成値も返すため、現在のホイール処理と二重適用になる
(`egui-0.33.3/src/input_state/mod.rs:630-645`)。

共通化するなら、ホイール入力そのものではなく 1 段下の意味操作を切り出す:

```rust
apply_zoom_factor_about_pivot(factor, pivot, translation)
```

既存の `zoom_preserve_pivot` / `set_fs_pan_from_input` / zoom min-max / pan clamp /
PDF 再レンダリング (`ui_fullscreen.rs:4015, 4216`) はそのまま再利用できる。

- ピンチ中は **PDF 再レンダリングを毎サンプル発行せず、ジェスチャー終了時に 1 回**にする
- **回転成分は初期版では無視**する (mIV の回転は非破壊 DB 管理で、ピンチ回転とは意味が衝突する)
- `zoom_delta()` 単独では入力源を判定しない。**active な touch ownership と `multi_touch()` の
  存在を条件にする** (§5.2 の fail-closed 方針)

### 5.7 左右パネルのタッチカバー

利用者が名指しした 5 機能の現在の入口と、タッチ到達性は次のとおり。

| 機能 | 静止画 | 動画 (native) | 音楽 |
| --- | --- | --- | --- |
| **AI アップスケール** | 左パネル「画像補正」→「AI」タブ (`ui_adjustment_panel.rs:8428-8534`)。`U`/`Shift+U`/`Alt+U`、リング picker | **再生中の入口は存在しない**。native 左「画像補正」は色調/フィルタのみ。オフライン動画アップスケールはメイン画面「動画」メニュー (`ui_main.rs:4919-4959`) | 該当なし |
| **カラー化** | 左パネル「カラー化」タブ (`ui_adjustment_panel.rs:7842-8170`) | **存在しない**。native の「色調」は明るさ/コントラスト等で別物 | 該当なし |
| **ブックマーク** | 左パネル「ブックマーク」タブ + 追加ボタン (`ui_adjustment_panel.rs:13030-13086`)。`B` | native 左「ジャンプ」タブ + 追加ボタン (`overlay_draw.rs:870-1011`)。`B`、リング対応 | 左パネルで動画用 body を再利用 (`ui_music_panels.rs:717-830`) |
| **レーティング** | 右パネル先頭の ★ (`ui_metadata_panel.rs:331-350`)。`F1`〜`F6`、リング | native 右パネル先頭に共通 ★ UI (`overlay_draw.rs:4683-4694`) | 右パネル先頭に同じ ★ UI |
| **タグ** | 右パネルの ON/OFF + picker (`ui_metadata_panel.rs:364-378, 968-1000`) | native 右パネルに同等 (`overlay_draw.rs:3964-4038`) | 静止画のタグ section を再利用 |

**いずれもキー/リング専用ではなく直接の UI がある**。したがって
**パネル内の action や保存処理を作り直す必要はない**。必要なのは 3 点だけ:

#### (1) パネルを開く導線

ここが本当の壁。上バーの `ℹ` ボタンは**右パネルを開くボタンではなく `Hover`/`ClickToShow` の
モード切替**である (`ui_fullscreen.rs:22333-22360`)。また ClickToShow の呼び出しバー自体が
端に `hover_pos()` がある間だけ描かれる (`ui_fullscreen.rs:5422-5435`) ため、
**タッチでは押下時に見えた callout が release フレームで `PointerGone` により消え、
click completion に到達しない可能性が高い** (§6-4、実機確認要)。

→ **§5.5 の中央タップによるクローム表示が、パネル到達の前提条件**になる。

#### (2) パネル内スクロールは既に動く

egui の既定 `ScrollSource::ALL` は `drag: true` で、コンテンツドラッグと慣性スクロールを
実装している (`egui-0.33.3/src/containers/scroll_area.rs:140-174, 786-830`)。
mIV で `scroll_source()` を上書きしているのは `src/ui_main.rs:2278` の 1 箇所だけ。

- 静止画・音楽パネル: **現状でも先頭接点の pointer emulation で指ドラッグ + 慣性が動く**
- native 動画パネル: §5.9 案 C で同じ pointer event を注入すれば同じ挙動になる
- slider / dropdown 等から始めたジェスチャ: `ScrollArea` はコンテンツより先に**低優先度の
  背景 drag response** を置いており、内側 widget の入力を奪わない
  (`scroll_area.rs:786-803`)。壊れはしないが、
  **「slider やタグボタンが密集した場所からはスクロールを始めにくい」** という UX 問題は残る

#### (3) タッチターゲットが小さすぎる

UI 倍率 100% / OS 倍率 100% における実測 (egui logical point):

| 対象 | 確保矩形 | 23px 基準 | 40px 基準 |
| --- | ---: | --- | --- |
| ★ (1 個) | 20pt フォントの galley 実寸 ≒ 20×20、最小値指定なし | **不足の可能性大** | 不足 |
| タグ ON/OFF | 通常 Button、最低高 18 | **不足** | 不足 |
| カラー化スロット | 22×22 | **1px 不足** | 不足 |
| slider response | 幅 100・高さ最低 18 | **高さ不足** | 不足 |
| slider handle (見た目) | 約 10.8×14.4 | **不足** | 不足 |
| 静止画ブックマーク追加 | 24×20 | **高さ不足** | 不足 |
| native ブックマーク系 | 26×24 | 満たす | 不足 |

(`ui_helpers.rs:853-875`, `ui_metadata_panel.rs:978-987`, `ui_adjustment_panel.rs:7804-7826, 13040-13050`,
`overlay_draw.rs:917-999`, `egui-0.33.3/src/style.rs:1348-1360`, `slider.rs:892-897`)

**UI 倍率に依存させることはできない** — 利用者が 100% を選んだ状態で最低基準を満たさないため。

マウスの見た目・レイアウトを変えないなら、**タッチと確定した tap に限った hit resolver** が要る。
特に ★ は個別に 40px へ広げると隣の ★ と重なるので、
**★ 行全体を 5 等分して nearest star を決める**方式にする。

### 5.8 選択済みセルの再タップで開く

**マウス無影響で追加可能**。ただし `Response::clicked()` の**後**で選択状態を見る実装では成立しない
(未選択セルへの最初のタップも「選択済み」に見えて即 open してしまう)。
**Touch Start 時点の選択状態を snapshot** しておく必要がある。

発火条件 (すべて満たすときだけ):

- §5.2 の exact Touch stream として確定している
- Touch Start と End が同じセル
- 移動量が tap しきい値未満
- **そのセルが Touch Start 前から `selected` だった**
- Ctrl / Shift なし
- `checked` が空
- タグバッジ / ボタン / popup / ダイアログ の上ではない
- grid scroll / native D&D の owner へ遷移していない

**ダブルタップは維持される** (追加であって置換ではない):

- 未選択セル: 1 回目で選択、2 回目で「選択済み再タップ」として open
- 選択済みセル: 1 回目で open
- 判定失敗 / 入力源不明: 従来のダブルクリック / ダブルタップ open が残る

同じ release で touch の再タップ open と `response.double_clicked()` が二重実行されないよう、
**open は共通 helper に集約**し、touch 側で発火した release だけ既存のダブルクリック分岐を抑止する。
現状の open 分岐はフォルダ / ZIP・PDF / メディア / 検索コンテナ / ブックマークビュー /
detached 準備を含む (`ui_main.rs:12135-12305`) ため、**コード複製は危険**。

複数選択との相互作用: 選択モデルは `selected` と `checked` を分離し、Check / Explorer の
2 方式を持つ (`settings.rs:492-523`, `ui_main.rs:3761-3859`)。
**修飾キー中または `checked` が非空のときは再タップ open を無効**にすれば、
Ctrl/Shift range・checked items・Check 方式の状態を壊さない。

### 5.9 動画 native HWND の入力アダプタ

presenter HWND と HUD HWND は mIV 自前の Win32 ウィンドウで、egui-winit を通らない。
現状は `WM_MOUSEMOVE` / 各マウスボタン / `WM_MOUSEWHEEL` を独自 egui Context へ
`PointerMoved` / `PointerButton` として注入している
(`native_window.rs:1297-1365`, `render_core.rs:4216-4242, 6011-6024`)。

#### 案の比較

| 案 | 方式 | 評価 |
| --- | --- | --- |
| **A** | `RegisterTouchWindow` + `WM_TOUCH` | 実装可能だが**非推奨**。登録・消費した瞬間から現在の promoted mouse を前提にした「タップがなんとなく効く」状態を置換するため、pointer emulation / panel widget / `ScrollArea` / Cancel / DPI 変換まで一括実装が必要になる |
| **B** | promotion を残し `WM_POINTER` と promoted mouse を両方処理、`GetMessageExtraInfo` の signature で排他 | 可能だが**二重発火対策が複雑**。ドライバ差・capture loss・HUD HWND 境界で条件が増える |
| **C** | 登録なしで `PT_TOUCH` の `WM_POINTER` stream **全体**を所有 | **推奨** |
| **D** | `WM_GESTURE` の `GID_ZOOM`/`GID_PAN` だけ横取り + promoted mouse で tap | **フォールバック**。案 C が実機で成立しなかった場合の退避 |

#### 推奨: 案 C

1. `RegisterTouchWindow` を呼ばない
2. `EnableMouseInPointer` も呼ばない (これは**マウスにも** `WM_POINTER` を生成させる
   プロセス全体の設定で、「マウス無影響」の目的には逆効果)
3. `WM_POINTERDOWN/UPDATE/UP` で `GetPointerType` を呼ぶ
4. **`PT_TOUCH` と確定した stream は DOWN から UP/Cancel まで全メッセージを処理し、常に 0 を返す**
5. `PT_PEN` その他は stream 全体を従来どおり `DefWindowProc` へ渡す
6. touch stream から native overlay へ `Event::Touch` と先頭接点の pointer emulation を注入する
   → **egui 側と同じジェスチャ認識器を共有できる** (OS アダプタだけが別実装)
7. `WM_POINTERCAPTURECHANGED` と `POINTER_FLAG_CANCELED` で必ず状態を解放する

`DefWindowProc` を touch stream に対して呼ばなければ、その経路からの promoted mouse を避けられる。
Microsoft は **「一つの pointer stream の一部だけを消費し、残りを `DefWindowProc` に渡す動作は
未定義」** と明記しているので、**stream 単位で丸ごと所有するか丸ごと渡すか**にすることが重要。

さらに既存の `WM_MOUSE*` handler で `GetCurrentInputMessageSource()` を確認し、
`IMDT_TOUCH` と確定した重複だけを捨てる安全網を置ける。
**失敗または `IMDT_UNAVAILABLE` の場合は捨てず、従来の mouse handler へ流す**のが安全側。

#### 案 D (フォールバック) の内容

Windows の既定ジェスチャハンドラは、タッチを次のレガシーメッセージへ変換する
([Windows Touch Gestures Overview](https://learn.microsoft.com/en-us/windows/win32/wintouch/windows-touch-gestures-overview)):

| ジェスチャ | 合成されるメッセージ |
| --- | --- |
| パン | `WM_VSCROLL` / `WM_HSCROLL` |
| 長押し | `WM_RBUTTONDOWN` / `WM_RBUTTONUP` |
| ピンチ | **`WM_MOUSEWHEEL` + `MK_CONTROL`** |

→ **§2.6 で観測した「ピンチがタイル列数変更に化ける」「長押しが右クリックになる」は
この仕様どおり**の挙動。

`WM_GESTURE` を自前処理して `GID_ZOOM` / `GID_PAN` を消費すれば、既存のマウス処理に
一切触れずにピンチとパンを横取りできる。**`GID_PAN` は OS 側に慣性が組み込まれている**。
ただし `GID_BEGIN` / `GID_END` は `DefWindowProc` に渡すこと (消費すると動作未定義)。

**案 C より劣る点**: 接点ごとのデータが得られないので egui 側と認識器を共有できず、
tap 判定は promoted mouse + `GetMessageExtraInfo` に頼ることになる。
signature は **RDP 経由などで欠落する実例がある**ため、これを正本にはできない。
→ **案 C が本命、signature と `GetCurrentInputMessageSource` は診断・安全網**という位置づけ。

### 5.10 最重要の落とし穴 — egui のポインタ翻訳との二重発火

`Event::Touch` をイベント列から `retain` で消しても、**egui の `InputState.pointer` は既に
更新済み**なので `Response::clicked()` / `drag_started()` は消えない。
イベントを消すアプローチでは二重発火を防げない。

必要なのは**アプリ側の所有権**:

```
TouchOwner
  ├─ WidgetPassthrough      : UI ボタン・パネル上で開始 → egui pointer へ委譲
  ├─ GridScroll             : 一覧セル上で開始し移動 → スクロールが取得 (D&D は常に無効)
  ├─ ViewerPointerPassthrough: 拡大画像の単指ドラッグ → 既存 pointer パンへ委譲
  ├─ ViewerTapZone          : タップ確定 → 左右/中央コマンドを発火
  ├─ Pinch                  : 2 本目が入る → 単指パンをキャンセルして取得
  └─ Cancelled
```

規約:

- 同じ release の `fs_response.clicked()` は抑止する
- **全接点が離れるまで primary 抑止を維持**する
- グリッドでは、スクロール認識のしきい値と egui の drag 開始しきい値を競争させない。
  **タッチ由来なら D&D 自体を無効にする**方が確実

### 5.11 既存の入力オーナーシップとの境界

**`TouchAction` を `KeyAction` に合流させない**。`KeyAction` は離散キーと shortcut permit の体系で、
タッチには複数接点の寿命・連続移動量・scale/translation・widget との競合・pointer emulation との排他
がある。性質が違う (`docs/keyboard-input-ownership-plan.md:44`)。

追加するのは**利用者向けの巨大な enum ではなく、内部型**だけ:

```
TouchGesturePhase / TouchGesture / TouchOwner / TouchCommand
```

Phase 1 は**固定マッピングで十分** (`TapPrevious` / `TapNext` / `RevealChrome` / `CloseViewer` /
`BackToParent` / `ScrollGrid` / `TransformViewer`)。離散操作は既存の意味 API へ流す
(`spread_page_nav` / `handle_fullscreen_close_request` / 既存の親移動 / 既存 zoom-pan 適用層)。

`src/app/gamepad_input.rs` の層分け
(`raw input → 正規化 → 入力可否ゲート → 意味操作 dispatch`) は構造の参考になる。
ただし**ゲームパッドはグローバル入力なので操作 surface を解決するが、タッチは
イベントが届いた viewport が送信元**。`current_input_surface()` に再解決させず、
受信 viewport を正本にする。

### 5.12 フェーズ分けと工数 (改訂版・Codex Sol 見積もり)

人日は実装 + unit/snapshot + ドキュメント + 実機調整 1 回以上を含む。長期ベータ・多機種検証は含まない。

**動画を出荷条件にしたので、native 対応を最後まで放置しない**。
Phase 1 で native 入力の vertical slice を通して実機検証し、完成を Phase 3 で行う。

| Phase | 内容 | 工数 | 主な回帰リスク |
| --- | --- | --- | --- |
| **0** | 現行版の回避策を案内 (§4)。コード変更なし | ~0.5 人日 | なし |
| **1** | **入力源分離と両 backend の成立確認**。viewport ごとの Touch stream / correlation / ownership、静止画フルスクリーンの tap-page・中央クローム・ピンチ/パン・閉じる、**native `WM_POINTER` アダプタ (§5.9 案 C)**、native で tap が egui へ一度だけ届き promoted mouse が二重発火しないことの確認、Cancel / capture loss 処理、入力源の診断ログ | **7〜10 人日** | mouse-only=低 (ただし mouse handler の early filter は高精度テスト必須), マウス+タッチ同時=中, **ペン=高** (viewport 側で指/ペン区別不能), detached=中 |
| **2** | **一覧・再タップ open・静止画/音楽パネル**。grid の touch drag scroll (行スナップ維持)、選択済みセル再タップ open、中央クローム + 左右 callout、静止画の AI/カラー化/ブックマーク/★/タグ、音楽の ブックマーク/★/タグ、named widget の touch hit target、タグ入力の IME / タッチキーボード確認 | **7〜11 人日** | grid selection / D&D=中〜高, サムネ仮想化・prefetch=中, マウス click/double-click=低 (ただし共通 open helper の回帰範囲は広い), IME=中 |
| **3** | **動画 native の完成**。play/pause・seek・前後 file・HUD 表示・close、native 中央クローム/callout、ジャンプ/ブックマークパネル、補正パネル、★/タグパネル、native `ScrollArea` の drag/慣性、native ターゲットサイズ調整、mouse/touch/pen 混在・複数 HWND・DPI・capture loss の hardening | **8〜13 人日** | **native presenter/HUD=高**, **mouse promotion/filter=高**, 動画再生自体=中 (入力 transport 以外に触れないこと), detached/media window=中〜高, VST/IME=中 |

**総工数: 22〜34 人日**。実機で native HWND の配送差や Cancel 問題が出た場合は上振れする。

Phase 2 の内訳: パネル open + ターゲット調整 4〜7 人日 / 再タップ open 1.5〜2.5 人日 /
grid scroll + 行スナップ + prefetch 3〜5 人日。

**「設定なし」にしたことの工数影響**:

- 設定 field / migration / preferences UI / ドキュメントを作らない: **約 0.5〜1 人日減**
- source correlation、fail-closed 排他、mixed-device 試験: **約 2〜4 人日増**
- → **正味 1.5〜3 人日程度の増**。単純な「タッチモード設定」案より高くつくが、
  マウス回帰リスクは下がる。

新規ファイルは `src/touch_input.rs` (認識器の純ロジック、250〜400 行) 1 本。
主な既存アンカー: `src/app.rs:61796` / `src/ui_fullscreen.rs:15640` /
`src/video/native_window.rs:1111` / `src/video/native_presenter/render_core.rs:4216`。

**テスト方針**: タップ領域判定・RTL・状態遷移・2 本目追加時の単指キャンセル・
anchor/remainder 計算・端数表示時の可視/keep 範囲・pointer stream の排他は**純関数 unit**。
タッチクローム・ターゲットサイズ・左右ハンドルは **kittest snapshot**。
以下は**実機必須** (自動テストだけで完了扱いにしない):

- Windows 10/11 の 2-in-1 で、1 本指 / 2 本指 / タッチ中のマウス操作 / ペン / 精密タッチパッド
- Alt+Tab、capture loss、スリープ復帰、画面回転
- main fullscreen / F12 detached の状態分離
- **presenter HWND と HUD HWND のそれぞれで `PT_TOUCH` を受信すること**
- マウスの click / 右ドラッグリング / wheel / X ボタンが従来どおりであること
- UI 倍率 50/100/150/200% × DPI 100/150/200%
- パネル内の slider / combo / タグ / ★ の上からドラッグを始めたときの挙動
- VST UI 表示中、タグ IME 入力中

### 5.13 やらない判断 (費用対効果)

| 機能 | 追加目安 | 判断 |
| --- | ---: | --- |
| 長押し → リングショートカット | 2〜4 人日 | **入れない**。一覧では単指ドラッグをスクロール、拡大画像ではパンに使うため、「長押し後の方向ドラッグ」と区別しにくい。egui の長押しは既にコンテキストメニューと競合する。発見可能性は中央タップの大きなクロームの方が高い |
| 長押しルーペ | 3〜5 人日 | 入れない |
| 5 領域フルカスタマイズ | 3〜5 人日 | 初期版では過剰 |
| ピンチ回転 | 2〜4 人日 | 保留。非破壊回転 DB との意味衝突を別途整理してから |
| 本格慣性 + バウンド演出 | 3〜6 人日 | 実機要望が出てから |
| 編集ツールの全面タッチ対応 | 10 人日以上 | 別プロジェクト扱い |
| タッチによるファイル D&D | — | 入れない (むしろ抑止する) |

### 5.14 未決の判断事項

利用者判断 (2026-08-06) で ①動画必須 ②パネル対応 ③再タップ open ④設定なし は確定した。
残るのは以下 3 点。**特に 1 は着手前に決める必要がある**。

1. **【要決定】ペンの扱い**。winit は `PT_TOUCH` と `PT_PEN` を途中で識別しているが、
   最終的に**両方とも同じ `WindowEvent::Touch` として出す**
   (`winit-0.30.13/src/platform_impl/windows/event_loop.rs:2070-2124`)。
   pressure の有無だけではペンを確定できない。したがって二択:
   - **(a) ペン接触もタッチとして扱う** — 追加工数ゼロ。ペンで絵を描く用途は無いので実害は小さいが、
     「ペン = 精密ポインタ」という期待とはズレる。
   - **(b) ペンは従来のマウス扱いを維持する** — winit / eframe より下に `PT_TOUCH` / `PT_PEN` の
     source tag を持ち回る改造が要る。vendored eframe には手を入れられるが、
     egui-winit は vendored ではない (crates.io) ので、そこに手が要るなら追加 vendoring が発生する。

   Codex Sol は **「設定なし方針そのものより、このペン仕様未決定の方が実装後の後悔要因になりやすい」**
   と指摘している。
2. **診断用の強制無効を持つか**。利用者向けの設定は作らないが、最初の数リリースは
   `--disable-touch-gestures` または `MIV_DISABLE_TOUCH_GESTURES=1` を持つべきという提案。
   タッチ = 左クリックの既存動作は残し、**新しいジェスチャ解釈だけ無効化**する。
   永続設定でも自動タッチモードでもないので ④ の方針とは矛盾しない。
   native HUD には既に環境変数デバッグの前例がある (`src/video/native_window.rs:1302-1317`)。
   → **持つことを推奨**。逃げ道ゼロで出すのはリスクが高い。
3. **動画に AI アップスケール / カラー化が存在しない件** (§5.7 参照)。
   利用者の要望リストに挙がっているが、これらは**静止画の機能で、動画再生中には元から無い**。
   タッチ対応で追加する話ではないので、「静止画のみ対象」と整理してよいか確認が要る。

### 5.15 「マウス操作を変えない」の保証範囲 (明文化)

Codex Sol の指摘により、保証できる表現を次に限定する:

> **タッチを行っていない mouse-only の入力列については、挙動を一切変更しない。**

同じ egui viewport 上でマウスとタッチを**完全に同時**操作した場合、egui の `PointerState` が
1 つしかない以上、2 つを完全独立にする保証はできない。ここを絶対条件にすると
egui-winit より下の入力層を大きく作り直す必要がある。実用上は
「タッチしながら同時にマウスも動かす」という操作は稀なので、この限定で足りると判断する。

---

## 6. 未確定事項 / 実機確認が必要な項目

1. **【最優先】`WM_POINTER*` が presenter HWND と HUD HWND の両方に期待どおり配送されるか**。
   API 仕様上 touch-unregistered window にも届くことは確認できたが、mIV の実 HWND 構成
   (hit-test region / DirectComposition / `WS_EX_NOACTIVATE` の HUD) で成立するかは未確認。
   **Phase 1 の出荷ゲートにする** (崩れると §5.9 の設計を引き直す)。
2. 動画 native presenter で、長押し→右クリック合成が「短い右クリック」と誤分類されて
   フルスクリーンが閉じるか (§2.6 の潜在バグ)。**実機必須**。
3. **egui-winit の Touch Cancel が primary release を出さない件**
   (`egui-winit-0.33.3/src/lib.rs:732-735`)。アプリ側の gesture state は Cancel で破棄できるが、
   **egui widget 側に primary-down 状態が残らないか**。残る場合は入力アダプタ側で
   release を補う必要があり、構造的な壁になる。
4. **ClickToShow の呼び出しバーがタッチで押せるか**。callout は端に `hover_pos()` が
   ある間だけ描かれる (`ui_fullscreen.rs:5422-5435`)。Touch End と同じ batch で
   `PointerGone` が来るため、**押下時に見えた callout が release フレームで消えて
   click completion に到達しない可能性が高い** (コード順序からの推測)。
3. 「フォルダを開く」ダイアログはフルパス入力式で参照ボタンが無く
   (`src/ui_dialogs/open_folder.rs:34-52, 81-106`)、OS タッチキーボードの自動表示も
   コードからは保証できない。タブレットでの実用性を実機確認する必要がある。
   なお「任意の場所へ移動」は `場所▼` / ドライブ一覧 / ピクチャ等の既存導線で到達できる
   (`ui_main.rs:10942, 11015`) ので、新しいフォルダピッカーは必須ではない。
4. スクロールバーの実効的な掴みやすさ (DPI・UI 倍率・機種依存)。
5. detached viewer で、root と別 viewport のタッチ状態が混ざらないこと。
   passive detached の「最初のクリックは復帰だけ」という既存挙動を、最初のタップが
   突き抜けて操作まで届かないかを実機確認する。

---

## 7. 推奨ロードマップ

1. **Phase 0 (即時)**: §4 の回避策を利用者へ案内する。
   これだけでフルスクリーンの「詰み」は回避でき、タブレットでも一応閲覧できる状態になる。
2. **着手前に §5.14 の 3 点を決める** — ペンの扱い / 診断用強制無効の有無 /
   動画に AI アップスケール・カラー化が無い件の扱い。
   特に**ペン仕様の未決定は、実装後の後悔要因として最も大きい**。
3. **Phase 1**: 入力源分離と 2 つの backend の成立確認。
   静止画フルスクリーンのタッチ操作一式に加え、**動画 native の pointer adapter を
   この段階で薄く通して実機検証する**。動画を出荷条件にする以上、
   native HWND に `WM_POINTER` が期待どおり届くかの確認を最後まで先送りしない。
4. **Phase 2**: 一覧の直接スクロール + 再タップ open + 静止画・音楽パネル。
   ここまでで「静止画はタッチ対応」と言える。
5. **Phase 3**: 動画 native の完成。ここまでで「mIV はタッチ対応」と表明できる。
6. 実機フィードバック後に**慣性の要否を判断**する。必要なら短距離・行単位の限定慣性だけ。
7. リング / ルーペ / 5 領域カスタマイズ / ピンチ回転は**要望が出るまで保留**。
   これらより「閉じる・戻る・スクロール・ピンチの確実性」に工数を使う方が効果的。

**Phase 1 で静止画を先に完成させる**のは、フルスクリーンが「入ったら出られない」罠に
なっているため。Phase 2 を先にすると、指でスクロールできるようになった利用者が
サムネイルをタップしてフルスクリーンに入り、そこで詰む — という悪化した体験になる。

**ただし動画 native の入力経路だけは Phase 1 で薄く通す**。ここは
「`WM_POINTER` が presenter / HUD の実 HWND に期待どおり配送されるか」という
**未確認の前提**の上に立っており、これが崩れると Phase 3 の設計ごと引き直しになる。
Phase 1 の出荷ゲートに含める。

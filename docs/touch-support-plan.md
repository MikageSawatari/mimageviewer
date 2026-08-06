# タブレット PC タッチ操作対応 — 調査と設計方針

> 状態: **調査完了 / 設計検討中 (未実装)**。2026-08-06。
> 調査 = ClaudeCode + Codex Sol (read-only)、実装は未着手。
> きっかけ: 利用者からの「タブレット PC のタッチ操作に対応しているか」という質問。

関連ドキュメント:

- [docs/fullscreen-side-panel-mode-plan.md](fullscreen-side-panel-mode-plan.md) — 左右パネルの `Hover` / `ClickToShow` モード
- [docs/ring-shortcut-plan.md](ring-shortcut-plan.md) — リングショートカット (右ドラッグ / ゲームパッド)
- [docs/keymap-spec.md](keymap-spec.md) / [docs/keyboard-input-ownership-plan.md](keyboard-input-ownership-plan.md) — 入力オーナーシップの既存境界
- [docs/video-architecture.md](video-architecture.md) — 動画 native presenter (独自 HWND)
- [docs/archive/ui-input/ui-scale-plan.md](archive/ui-input/ui-scale-plan.md) — UI 表示倍率 50-200%

---

## 1. 目的とスコープ

### 1.1 やること

**「普段の閲覧行動」をタブレットのタッチだけで完結できるようにする**。具体的には:

1. 一覧を指でスクロールする
2. フォルダ / サムネイルをタップして開く
3. フルスクリーンでページを送る (単ページ / 見開き / 本)
4. フルスクリーンで拡大縮小・パンする
5. 隠れている UI (上バー / 左右パネル) をタッチで確実に呼び出す
6. **フルスクリーンを閉じる / 親フォルダへ戻る**

### 1.2 やらないこと (当面)

- 編集系 (補正レイヤー / 消しゴム / 注釈 / モザイク / 表示トリム) のタッチ最適化
- 環境設定ダイアログ・各種管理ウィンドウのタッチ最適化
- ソフトウェアキーボードの自動表示制御 (検索・パス入力)
- スタイラス (筆圧・ホバー) 対応
- タッチ専用の別レイアウト / 別スキン

これらは「使えなくはない (タップ = 左クリックとして動く)」状態を維持するに留める。

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
意見が分かれた点・利用者判断が要る点は §5.9 に分けて記す。

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

### 5.2 操作マッピング (全体像)

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

### 5.3 一覧スクロール — anchor + fraction 方式

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

### 5.4 中央タップとクローム

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

### 5.5 ピンチズーム

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

### 5.6 最重要の落とし穴 — egui のポインタ翻訳との二重発火

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

### 5.7 既存の入力オーナーシップとの境界

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

### 5.8 フェーズ分けと工数 (Codex Sol 見積もり)

人日は実装 + unit/snapshot + ドキュメント + 実機調整 1 回以上を含む。長期ベータ・多機種検証は含まない。

| Phase | 内容 | 概算行数 | 工数 | 主な回帰リスク |
| --- | --- | ---: | --- | --- |
| **0** | 現行版の回避策を案内 (§4)。コード変更なし | 0 | ~0.5 人日 | なし |
| **1** | 静止画・本のフルスクリーン: 3 領域タップ / 中央クローム / 確実な閉じる / ピンチ + 2 本指パン / 単指パンとの排他 / タッチ On-Off 設定 | 720〜1,200 | **5〜8 人日** | マウス=中 (クリック/ドラッグ分岐を共有), detached=中〜高 (viewport ごとの状態分離), 動画=低 |
| **2** | 一覧の直接スクロール (anchor+fraction) / タッチ D&D 抑止 / 行スナップ確定 / 可視範囲 +1 行 / prefetch・settle・idle-upgrade 統合 / `⬆`・`場所▼` のターゲット拡張 | 620〜1,100 | **4〜7 人日** | **マウス D&D=高** (touch だけを正確に除外), **仮想スクロール/eviction=高**, PDF 先読み=中〜高 |
| **3** | 動画 native presenter: presenter/HUD 両 WndProc の pointer 入力 / promoted mouse との排他 / HUD ラッチ / 大きな再生・シーク・閉じるターゲット | 720〜1,300 | **5〜9 人日** | 既存 native mouse=高, HUD region/capture/focus=高, detached 動画=高 |

- **静止画・本まで: 9〜15 人日**
- **動画まで含めて: 14〜24 人日**
- 慣性を追加する場合: **+2〜3 人日 (+150〜300 行)**

新規ファイルは `src/touch_input.rs` (認識器の純ロジック、250〜400 行) 1 本。

**テスト方針**: タップ領域判定・RTL・状態遷移・2 本目追加時の単指キャンセル・anchor/remainder 計算・
端数表示時の可視/keep 範囲は**純関数 unit**。タッチクローム・ボタンサイズ・左右ハンドル・設定画面は
**kittest snapshot**。`PointerGone` の挙動・ピンチ・DPI・画面回転・マウス併用・detached の
状態分離・native presenter は**実機必須** (native presenter は自動テストだけで完了扱いにしない)。

### 5.9 やらない判断 (費用対効果)

| 機能 | 追加目安 | 判断 |
| --- | ---: | --- |
| 長押し → リングショートカット | 2〜4 人日 | **入れない**。一覧では単指ドラッグをスクロール、拡大画像ではパンに使うため、「長押し後の方向ドラッグ」と区別しにくい。egui の長押しは既にコンテキストメニューと競合する。発見可能性は中央タップの大きなクロームの方が高い |
| 長押しルーペ | 3〜5 人日 | 入れない |
| 5 領域フルカスタマイズ | 3〜5 人日 | 初期版では過剰 |
| ピンチ回転 | 2〜4 人日 | 保留。非破壊回転 DB との意味衝突を別途整理してから |
| 本格慣性 + バウンド演出 | 3〜6 人日 | 実機要望が出てから |
| 編集ツールの全面タッチ対応 | 10 人日以上 | 別プロジェクト扱い |
| タッチによるファイル D&D | — | 入れない (むしろ抑止する) |

### 5.10 利用者判断が必要な点

1. **動画を「タッチ対応」の範囲に含めるか**。静止画・本 (Phase 1-2) と動画 (Phase 3) は
   入力経路が完全に別で、工数がほぼ倍になる。
   初期リリースを「静止画・本のタッチ対応」と明記すれば Phase 3 は後回しにできる。
   一方「mIV はタッチ対応」と広く表現するなら、動画の HUD 表示 / 再生・一時停止 / シーク /
   前後ファイル / 閉じる は最低限必要になる。
2. **一覧のタップで開く意味を変えるか**。現在は 1 タップ選択 + ダブルタップで開く。
   タッチでは「未選択セルのタップ = 選択 / 選択済みセルをもう一度タップ = 開く」の方が
   時間制限が無く操作しやすい。ただし現状のダブルタップでも開ける (Codex 調査で「可能」判定) ので
   **必須ではなく好みの問題**。マウスのクリック/ダブルクリック仕様は変えない前提。
3. **タッチ機能の既定 ON / OFF**。正式対応時は ON 既定が妥当。
   開発・実験期間だけ OFF 既定にする運用が安全。
4. **設定軸の粒度**。当面は `タッチジェスチャー: On/Off` の 1 項目で足りる。
   将来必要なら `タッチ向け操作サイズ: 標準/大` を足す。
   UI 表示倍率 (50-200%) は全体を拡大するので**タッチターゲット最低寸法とは別軸**であり、
   これだけでは不十分 (現状: 上バー 44pt / 上バーボタン 32pt / ClickToShow hit 24pt /
   表示バー 20pt / スクロールバー 10pt・floating 8pt。
   Windows のガイドラインは最低 23px、重要操作 40px)。

---

## 6. 未確定事項 / 実機確認が必要な項目

1. 動画 native presenter で、長押し→右クリック合成が「短い右クリック」と誤分類されて
   フルスクリーンが閉じるか (2.6 の潜在バグ)。**実機必須**。
2. Windows のタッチ→マウス合成が、native HWND で実際にどのメッセージ列で来るか
   (`WM_MOUSEMOVE` の有無、`GetMessageExtraInfo` の `MOUSEEVENTF_FROMTOUCH` 判定可否)。
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
2. **Phase 1**: 静止画・本のフルスクリーンを完成させる。
   3 領域タップ / 中央クローム / **確実に閉じられること** / ピンチ / パン。
   長押しリング・回転・慣性は入れない。
3. **Phase 2**: 一覧の直接スクロールを入れる。
   anchor + fraction 方式で行スナップ不変条件を維持し、タッチ D&D を抑止する。
   **ここまで完成して初めて「静止画・本はタッチ対応」と案内する。**
4. 実機フィードバック後に**慣性の要否を判断**する。必要なら短距離・行単位の限定慣性だけ。
5. 動画が利用者要件に入るなら **Phase 3**。mouse promotion には依存せず、
   native presenter / HUD へ専用の pointer adapter を入れる。
6. リング / ルーペ / 5 領域カスタマイズは**要望が出るまで保留**。
   これらより「閉じる・戻る・スクロール・ピンチの確実性」に工数を使う方が効果的。

Phase 1 を先に置くのは、**フルスクリーンが「入ったら出られない」罠になっている**ため。
Phase 2 を先にすると、指でスクロールできるようになった利用者がサムネイルをタップして
フルスクリーンに入り、そこで詰む — という悪化した体験になる。

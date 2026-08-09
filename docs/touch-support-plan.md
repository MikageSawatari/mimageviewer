# タブレット PC タッチ操作対応 — 調査と設計方針

> 状態: **調査完了 / 仕様確定 / Phase 1 Step 3b / 3c まで実装済み**。2026-08-08。
> Step 0 診断プローブ、Step 1 純ロジック認識器、Step 2 egui 入力源相関、
> Step 3 静止画フルスクリーンの左右タップ / 中央クローム / ピンチズーム・パン / 相関済みクリック抑止、
> Step 3b 静止画フルスクリーンの左右パネル導線、Step 3c 上バーのタッチ専用 hit resolver、
> Phase 2 の一覧直接スクロール / 方向スナップ / ピンチ列数変更まで完了。
> きっかけ: 利用者からの「タブレット PC のタッチ操作に対応しているか」という質問。
>
> **仕様は確定済み (利用者判断 2026-08-06、2026-08-07 更新)。未決事項なし。§5.14 が決定一覧。**
> 動画も対応必須 / 左右パネルもカバー / 再タップ open は見送り /
> **タッチ ON-OFF 設定は作らない** (マウス無影響で、タッチしたときだけ別動作) /
> ペンはタッチ扱い / 診断用の強制無効は持つ / タップ領域は中央矩形案 /
> 動画のシークは ±5 秒 / AI アップスケールとカラー化は静止画のみ対象。

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
2. フォルダ / サムネイルを既存のダブルタップで開く
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

> **ペンはタッチ扱いにする** (決定、§5.16)。winit は指とペンを同じ `WindowEvent::Touch` に潰すため、
> 特別な対応をしなければペン接触もタッチとして解釈される。これを受け入れる。
> 唯一の代償はグリッドからのファイル D&D がペンでできなくなること。ペンのホバーは効く。

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

動画の presenter HWND / HUD HWND は **mIV 自前の Win32 ウィンドウ**である。
Phase 1 の実機ゲート前は両方とも `WM_POINTER` を処理せず `DefWindowProcW` に流していたため、
**Windows の既定タッチ→マウス合成が効いていた**。結果:

- タップ → 左クリック合成 → 再生/一時停止等に入る
- 長押し → 右クリック合成 → mIV の native 右ボタン処理へ入る
- **ピンチ → Ctrl+wheel が来るが、動画のズームには繋がらない** (Ctrl+wheel はタイル表示の列数変更のみ、`render_core.rs:4244-4280`)
- **パン → `WM_VSCROLL/WM_HSCROLL` ハンドラが無く、何も起きない**

**実機で確認済みのバグ**: presenter 上の長押しでは `WM_POINTERUP` の後に同一ミリ秒で
`WM_RBUTTONDOWN/UP` が合成され、mIV が「短い右クリック」と分類してフルスクリーンを閉じた。
Phase 1 Step 4 では presenter の `PT_TOUCH` stream 全体を所有して `DefWindowProcW` へ渡さない
構造へ変更したため、この合成経路自体を遮断している。**構造的な解消の確定は変更後ビルドの
実機確認後**とする。HUD HWND は今回所有していないため、HUD 上の長押し合成は Phase 3 まで残る。

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

ただし合成 pointer 列を出すかどうかは、`on_touch` の次の gate で接点ごとに決まる:

```rust
pointer_touch_id.is_none() || pointer_touch_id == Some(id)
```

gate が真なら phase ごとのミラー遷移と合成列は次のとおり。偽なら raw `Touch` だけで、
その接点に合成 pointer 列は一切ない。

| phase | `pointer_touch_id` の遷移 | `Touch` 直後の合成 pointer 列 |
| --- | --- | --- |
| `Start` | `Some(id)` にする | `PointerMoved` → primary press |
| `Move` | 変更しない | `PointerMoved` |
| `End` | `None` にする | primary release → `PointerGone` |
| `Cancel` | `None` にする | `PointerGone` のみ。primary release は出ない |

したがって 2 本目の `Start` は、1 本目が `pointer_touch_id` を保持している間は合成 pointer を
伴わない。一方、pointer 接点だった指が先に `End` すると id が `None` になって gate が再び開く。
その後も画面上に残っている 2 本目の `Move` は `PointerMoved` を出し、続く `End` は、その接点が
primary press を一度も出していなくても primary release → `PointerGone` を出す。mIV の相関層は
この再開放を含めて gate と 1 対 1 でミラーし、release を正当な期待列として相関する。

`drive_egui_touch_input(..., enabled)` の `enabled` は、**入力を観測するかではなく、
認識結果を実行してよいか**を表す。通常の呼び出しでは値にかかわらず raw Touch を処理し、
接点、pending signature、egui-winit の `pointer_touch_id` ミラーを常に進める。
`enabled=false` では返却コマンドと primary 抑止勧告だけを落とし、相関済み provenance と
active/contact 状態は caller が境界を安全に閉じるため保持する。これにより Start が範囲選択等の
抑止中、End が解除後という並びでも、次の stream を stray tail と誤認せず 1 回で認識できる。
診断用 `MIV_DISABLE_TOUCH_GESTURES=1` はこの引数とは別の process-wide hard disable であり、
従来どおり touch action と promoted-pointer filter を無効にして legacy 経路へ戻す。

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
| マウスをタッチと誤認 | 中央タップなどの touch action が走る | **許容不可** |

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

- **egui-winit の `pointer_touch_id` 保持**: gate が開いた `Start` でその id を保持し、
  同じ id の `End` / `Cancel` で解放する。ただし解放後は別接点がまだ画面上に残っていても
  gate が再び開き、その接点の `Move` / `End` も合成 pointer 列を出し得る。したがって
  「先頭 1 接点だけが pointer をエミュレートする」ではない
- **マルチタッチジェスチャの所有**: 最初の接点から、参加した全接点が End/Cancel するまで

**2 本目が入った時点で pending の single tap を取り消し**、最初の指が先に離れても
tap / ページ送りを発火させないことが重要。
一度 pan / pinch / scroll に確定した解釈を、**同じ接点集合のまま別 action へ移さない**。
これは同じ接点の解釈が行き来することを禁じる規約であり、接点集合が増えた場合の昇格は
禁じない。**接点が増えたときに限り、単指 pan から pinch へ昇格してよい**。昇格時は
現在の接点位置から pinch の基準を取り直す。逆方向の pinch → 単指 pan への降格は行わず、
2 本目が離れても全接点が離れるまで `Pinch` ownership を保持する。

#### Cancel の穴 (実機確認要)

egui-winit は Touch Cancel 時に `PointerGone` を出すが、**primary release を出していない**
(`egui-winit-0.33.3/src/lib.rs:732-735`)。アプリ側の gesture state は Cancel で確実に破棄できるが、
**egui widget 側に primary-down 状態が残らないか**は実機確認が要る。
残る場合は入力アダプタ側で release を補う必要があり、**構造的な壁の一つ**。

### 5.3 操作マッピング (全体像)

| 入力 | 一覧グリッド | 静止画 / 本フルスクリーン |
| --- | --- | --- |
| タップ | セル選択 / 開く | **中央矩形 = クローム表示 / それ以外は左右でページ移動** |
| 単指ドラッグ | **スクロール** (file D&D は抑止) | 既存のパン / 連続読みスクロール |
| 2 本指 | — | **ピンチズーム + 2 本指パン** |
| 長押し | コンテキストメニュー (既存のまま) | コンテキストメニュー (既存のまま) |
| リング | **接続しない** | **接続しない** |

左右の方向は既存の RTL 対応計算 `fullscreen_click_nav_base_delta` (`ui_fullscreen.rs:1246`) に流す。

#### 中央領域は「帯」ではなく「矩形」にする

当初案の「中央 1/3 の縦帯」はページ送り面積を 33% 奪う。**中央付近の矩形**にすれば
同じ役目を果たしつつ削る面積を大幅に減らせる:

```
横: 画面中央の 30〜34%
縦: 中央付近の 55〜65% (上端 15% 程度と下端 20〜25% 程度は除外)
→ 中央領域は画面全体の約 18〜22%、ページ送り領域は約 78〜82% 残る
```

さらに**実際に表示中の上バー (44pt) / 下シークバー (38pt) / 左右パネルの矩形を除外**する
(`ui_fullscreen.rs:927, 932`)。

### 5.4 一覧スクロール — anchor + fraction 方式

**実装済み (Phase 2 一覧スクロール + 実機フィードバック反映)**。`scroll_offset_y` は行境界の anchor のまま維持し、
1 行未満の描画端数は main viewport の一時データに分離した。端数表示中の可視 / keep 終端は
1 行拡張し、touch move / hold / release から prefetch・settle・idle-upgrade の時刻を明示更新する。
タッチ由来の primary stream だけ native file D&D を抑止する。最後に実際に動いた方向も同じ
viewport 一時状態に保持し、release はその方向へ確定する。2 本目が加わった場合は一覧でも
pinch へ昇格し、一覧側が連続倍率を列数変更へ解釈する。

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
指を離したら、最後に表示位置が動いた方向の行境界へ確定する。ただし移動開始直後の誤操作を
吸収するため、端数が行高の 15% 以内なら逆方向への確定を許す。方向不明時だけ最寄り行へ
フォールバックする。これにより release 時の逆走量は上下どちらも最大 15% となる。

release から確定先までの移動量が行高の 20% 未満なら従来どおり即時確定し、それ以上なら
130ms の cubic ease-out で補間する。補間中に動かすのは `fractional_drag_y` だけで、
`scroll_offset_y` (anchor) は開始行の境界に保ち、完了 frame で確定先の行境界へ 1 回だけ
更新して端数を 0 にする。新しい touch が始まった場合は、その frame まで進んだ端数を引き継いで
補間を中断する。元の位置へ戻したり、補間完了を待ったりしない。

付随して必要な改修:

- 端数表示中は可視範囲の**末尾を追加で 1 行保持**する (でないと下端が欠ける)
- touch move / release と snap 補間の各 frame で「スクロール中」を明示的に通知する
  — `last_prefetch_scroll_at` / `last_scroll_event_at` / idle-upgrade の時刻を更新し、
  snap 補間がある場合は完了まで scroll settle を発火させない。release frame は raw Touch End も
  early intent として記録し、補間状態を作る前に prefetch gate が一瞬開くことを防ぐ。
  現在の scroll 検出はオフセット変化の fallback に頼っており (`app/runtime_ops.rs:13-31`)、
  **端数だけ動いたフレームは検出できない**
- snap 補間中だけ `ctx.request_repaint()` を要求する。補間の terminal frame で状態を
  `Contact` に戻してから repaint 条件を評価し、補間状態が無い frame から animation
  repaint を出さない
- **タッチ由来のポインタストリームでは native file D&D を無効にする** (`ui_main.rs:12325`)。
  マウス D&D は維持する
- 一覧 pinch は recognizer の `Zoom { factor, pivot }` をそのまま受け、一覧側だけが列数へ
  意味付けする。`Pan` は無視し、列数変更とスクロールを同時に走らせない
- 倍率は gesture 開始または直前の列変更から乗算で累積する。1.25 以上で 1 列減、
  0.8 以下で 1 列増としてから 1.0 へリセットする。`MIN_GRID_COLS` /
  `MAX_GRID_COLS` の端でも threshold 到達後はリセットし、反転時の基準を中立に戻す
- pinch 中の列数変更は即時反映して `bump_input_seq("grid_cols", None)` を呼ぶが、
  `settings.save()` は呼ばない。gesture 内で列数が変わった dirty bit を viewport 一時状態に
  保持し、`PinchEnd` または cancel で状態を consume した 1 回だけ保存する。
  Ctrl+ホイールは従来どおり 1 step ごとに即時保存する

**慣性は入れない**。「指に追従して動く + 離したら行スナップ」だけでも現状より大幅に改善する。
上記 130ms は release 時点で既に決まっている snap 先までの短い補間であり、速度から到達行を
増やす慣性ではない。
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

#### Step 3b 実装記録 (2026-08-08)

- 静止画フルスクリーンの egui viewport に、`TouchChromeLatched` が ON の間だけ描く
  **タッチ専用ハンドル**を追加した。既存 hover callout の表示条件・矩形・矢印・tooltip は変更せず、
  タッチハンドルは `hover_pos()` に依存しない別 widget とした
- ハンドル幅は `STILL_TOUCH_PANEL_HANDLE_WIDTH_PT = 48pt`。高さは viewport 高の 24% を
  96--220pt に clamp し、さらに上バー 44pt と下シークバー 38pt の間へ clamp する。
  極端に低い viewport で安全帯が無い場合は描画も hit test もしない
- 左右ハンドルは物理的な左 / 右へ固定し、左は画像補正パネル、右はメタデータパネルを開く。
  読み方向では反転しない。マウス callout とタッチハンドルは同じ意味 action を呼ぶ
- `Hover` / `ClickToShow` の両モードでタッチハンドルを表示する。ハンドル矩形はタップ zone の
  `excluded` へ加え、ハンドル操作がページ移動 / 中央クローム toggle と競合しないようにした。
  対応パネルが開いた側は描画・hit test・`excluded` のすべてから外し、反対側だけを残す
- 左補正パネルと右情報パネルの明示 open はどちらも
  `MetadataPanelOpenState::{Closed, ByPointer, ByTouchHandle}` を単一の正本とする。全 surface の
  resolver は同じ state を読み、`ByTouchHandle` は mode に関係なく表示、`ByPointer` は従来どおり
  `ClickToShow` でだけ表示する。mouse / native / music の pointer writer は `Hover` で no-op の
  まま、タッチハンドルだけが専用 writer から idempotent な `ByTouchHandle` を生成する
- `ByTouchHandle` のパネル外をタップしたときは、左右の touch-owned パネルを 1 つの modal group
  として両方閉じ、そのタップを page / chrome toggle に再利用しない。`ByPointer` は従来どおり
  パネル外クリックでは閉じない
- `ByTouchHandle` はファイル / ページ移動とフルスクリーン退出で `Closed` へ戻す。
  連結表示の再アンカーで従来維持していた `ByPointer` は維持し、touch owner だけを閉じる
- `MIV_TOUCH_DEBUG=1` の既存 command / ownership 診断は維持した。初回オーバーレイヘルプ、
  動画 native / 音楽、パネル内部の小 target 対応はこの Step 3b には含めない

#### 拡大中の 1 本指パン修正 (2026-08-08)

実機で「ドラッグしてもパンできない」と報告されたため、Step 3b 直前の `9f64f5d3` と比較した。
raw touch の press → move → release は `ViewerPointerPassthrough` で egui の
`drag_started_by(Primary)` / `dragged_by(Primary)` / `drag_stopped_by(Primary)` まで成立していた。
原因はそれより後段で、`Sense::click_and_drag` が移動閾値を超えた最初の move frame で初めて
`drag_started` になるのに、pan 側がその frame を開始位置の保存だけに使い、同 frame の移動量を
捨てていたことだった。短い / coalesced な touch move では全移動量を失う。

この pan 処理は 2026-04-13 から存在し `9f64f5d3` にも同形であるため、**Step 3b の退行ではない**。
press の合成や症状 guard は加えず、実際の press からの `total_drag_delta()` を drag 開始時の pan
へ適用するよう ownership 境界を修正した。既存マウスの絶対 drag / clamp の意味も維持し、
raw touch stream と最初の move の pan 反映をテストで固定する。

#### パネル外タップ後の primary 抑制 ownership 修正 (2026-08-08)

実機では、touch-owned 左右パネルをパネル外タップで閉じた後、
タッチとマウスの両方でパンできなくなる退行が出た。fs_zoom、panel hit、
command dispatch の continue はいずれも次フレームへ残る状態を作っておらず、
旧共有状態 fs_suppress_primary_until_release が残るときだけ両入力の pan 分岐を
まとめて飛ばすことを確認した。

根本原因は、この共有状態を arm する owner と clear する owner が一致していなかったこと。
touch command / correlated response は touch tracker の terminal frame でも latch を arm
していた一方、clear は別 owner である egui pointer の primary_released() に依存していた。
通常の単一 pass では同時に観測できる。一方、primary release の後に PointerGone tail が
分かれ、パネル表示変更後の同一 app frame replay が先に走る列では、相関層は response 抑制を
replay するが egui の release edge は前 pass で消費済みになる。この pass が latch を再 arm し、
後続の PointerGone には release が無いため、将来の clear event が存在しない状態になっていた。

FullscreenPrimarySuppression の Idle / PointerStream / TouchStream を単一 owner とし、
pointer 起点は primary release、touch 起点は touch completion / Cancel で終端する reducer
へ集約した。terminal touch replacement と correlated response は当該フレームだけを抑制し、
cross-frame state を新規 arm しない。pointer release の遷移は input handler 冒頭に置き、
navigator / capture selection / compare wipe の早期 return より前に必ず実行する。
パネル外タップ後の touch / mouse drag と、terminal / replay / Cancel の各終端を unit test で固定した。

上バーの可視判定は既に純関数 `still_top_bar_visible_from_inputs` (`ui_fullscreen.rs:1018-1027`)
なので、入力構造体に 1 フィールド足すだけで済む。テストも既存パターンに乗る。

#### 発見可能性をどう担保するか

中央タップは**不可視の領域**なので、それ単独では気づけない (利用者指摘 2026-08-06)。
「コンテンツをタップして操作系を出す」自体は
Apple Books / Google Play Books / Kindle / YouTube などで**十分に定着したパターン**だが、
**「不可視の中央矩形を正確に狙う」ことは自明ではない**。実アプリも初回チュートリアルや
起動直後のクローム表示で補っている。

mIV では次の 3 段で担保する:

**中央タップを学習するまで初回オーバーレイヘルプを出す。常時表示の affordance は置かない。**

実装は面ごとに段階導入する。Step 3d は**静止画 / 本フルスクリーンだけ**を対象とし、
未学習の状態で最初のタッチ接点が発生した時点に、その面で有効なジェスチャを図示した
オーバーレイを重ねる。動画 / 音楽は Phase 3 で同じ方針を適用する。中央タップを教えた後は
何も出さないので、**閲覧中に画像を遮るものが一切なくなる**。

#### 表示条件 — 「1 回だけ」ではなく「中央タップを一度使うまで」

純粋な「1 回だけ」だと、**その 1 回を見逃した / 忘れた利用者が再び詰む**
(タブレットには Esc キーが無く、逃げ道がない)。そこで:

> **中央タップでクロームを一度表示できたら「学習済み」**として、以後は出さない。
> それまでは開くたびに出す。

- 大多数の利用者にとっては**文字どおり 1 回**で終わる
- 見逃した人にだけもう一度出るので、**「閉じられない」への安全網**が残る
- **案内した操作全部の実行は要求しない**。煩わしいうえ、安全上必要なのは中央タップだけ
- 必要なのは各面に真偽値 1 個 (「クロームを一度呼び出したか」)。静止画 / 本と動画は
  左右タップの意味が異なるため独立して学習する。利用者向け設定ではないので
  §5.1-6 の無設定方針と矛盾しない
- **マウス・キーで開いた場合は出さない** (既存挙動を一切変えない)

#### 内容 — 面ごとに出し分ける

単なるテキスト箇条書きではなく、**中央領域の輪郭・左右領域を半透明で図示**する。
「クロームがある」ではなく「**どこを叩けば何が起きるか**」を伝える。

| 面 | オーバーレイの内容 |
| --- | --- |
| **静止画 / 本** | 中央タップ = メニュー / 左右タップ = ページ送り |
| **動画** (Phase 3) | 中央タップ = HUD 表示 / 非表示、左右タップ = ±5 秒の相対シーク |
| **音楽** (Phase 3) | タップ = HUD 表示 (左右タップは割り当てない) |

- **オーバーレイ中はどこをタップしても中央タップとして扱う**ので、消し方を別途覚える必要がない
- 全画面を覆うモーダルなチュートリアルにはしない。閉じる操作も要求しない
- **隅に「UI が小さい場合はメニューの『スケーリング』から変更できます」の 1 行を添える**
  (従属的に小さく。強制しない。詳細は §5.14-8)

#### Step 3d 実装記録 (2026-08-08)

- 「タッチで開いた」の判定は **(b) フルスクリーン中の最初のタッチ接点で表示**を採用した。
  raw touch 相関済みの既存 `TouchFrame` を使うため、マウス / キーだけでは表示されず、
  全 open 入口へ provenance を追加しない。最初の接触列は案内表示だけに消費し、その次の
  任意位置タップを中央タップとして扱う
- 学習済みは現在 `Settings.touch_still_chrome_learned` の bool 1 個へ保存する。
  Step 3d で追加した旧名 `touch_center_chrome_learned` は未出荷だったため、Phase 3 Step 2b で
  動画用フラグを分離する際に移行コードなしで対称な名前へ変更した
  `settings_kv` の加法フィールド + `#[serde(default)]` なので DB schema family は変えず、
  旧 DB のキー欠落は false で読み込む。旧 DB の boot が `LoadedExistingDb` のままで
  quarantine しないことと、true の保存 / 再読込をテストした
- 表示中は同じ viewport / surface の一時 typed state が全タッチコマンドを所有する。
  左右タップもクローム表示 + 学習済み保存へ畳み、ページ送りには渡さない。ピンチ / パンも
  オーバーレイを閉じず実行しない
- 図示する中央矩形と `classify_tap` は共通の `center_tap_rect` producer を使う。
  矩形一致と四隅を含む分類を unit test で固定した
- `HelpShowContextShortcuts` の画像フルスクリーンへ「タッチ操作」欄を追加し、中央タップ、
  左右タップ、2 本指ズーム / パンを再確認できるようにした。Step 3d 時点では動画 / 音楽には
  追加せず、動画は Phase 3 Step 2 以降で追加した

#### 動画・音楽のタップ割り当て

**動画にはページ送りが無い**ため左右の動作は静止画と異なるが、タップ領域は静止画と
同じ 3 領域を使う。

**確定 (利用者実機判断 2026-08-08): 中央タップは HUD toggle、左右タップは単発で相対シーク。
前後ファイル移動は下 HUD のボタンに任せる。**

2026-08-06 の机上判断による次の旧仕様は、実機ではタップの意味が時間窓で変わって誤操作に
感じられたため **2026-08-08 に撤回**した。履歴として削除せず残す。

- **撤回: 「単一タップは画面全体で HUD 表示」**。現仕様は静止画と同じ
  `center_tap_rect` の中央だけが HUD 表示 / 非表示を切り替える
- **撤回: 「左右の『ダブル』タップで相対シーク。単一タップにシークを載せると誤操作しやすい」**。
  現仕様は左 / 右の単発タップ 1 回ごとに、それぞれ 5 秒戻る / 進む

現仕様:

- 領域は静止画と完全に同じ `center_tap_rect` / `classify_tap` を使い、動画専用の矩形を作らない
- 中央タップは HUD 表示 / 非表示だけを切り替え、シークしない
- 左タップは -5 秒、右タップは +5 秒。物理的な画面の側を使い、RTL / LTR の読み方向で反転しない
- 左右タップのシークはクローム latch に触れない。HUD が表示中なら表示したまま、隠れていれば
  隠れたままにする
- **前後ファイル移動は左右タップに載せない**。誤爆すると再生中の動画が消える。
  **下 HUD の [前ファイル] / [次ファイル] ボタンで足りる** (既存)
- 音楽ビューは既存のグラフ領域タップによるシークを変更しない。音声専用 native shell も従来どおり
  `PageSide` をクローム toggle に写像し、動画の左右シークと初回案内を有効にしない

**シーク量は ±5 秒で確定** (§5.14-9)。素の左右キーと同じ
`native_video_seek_relative_with_hint(fs_idx, ±5.0)` を通し、アプリ内の刻みと
先頭 / 末尾のフィードバックを共有する。

#### Phase 3 Step 2 / Step 2b 実装記録 (2026-08-08)

- Step 2 で追加した `DoubleTapRun`、500 ms の時間窓、48 pt の距離しきい値と関連テストは
  Step 2b で削除した。`NativeTouchAdapter` はタップ間の時刻 / 距離 / 回数を保持しない
- `PageSide` は動画ジェスチャ有効時に 1 コマンドからちょうど 1 回の `SeekRelative` を作る。
  左は常に負、右は常に正で、読み方向の解決を通さない。中央の `ToggleChrome` だけが
  chrome latch を変更し、`SeekRelative` の dispatch は latch を読み書きしない
- 初回案内は静止画 / 本の `Settings.touch_still_chrome_learned` と動画の
  `Settings.touch_video_chrome_learned` に分離した。旧 `touch_center_chrome_learned` も両新フラグも
  v2.13.0 の未出荷データなので、移行コードなしで旧名を置き換えた。どちらも `settings_kv` の
  加法フィールド + `#[serde(default)]` であり、DB schema family は変えない
- 未学習の動画で最初の touch contact が来たときだけ案内を表示し、その接触列の release 後に来る
  任意位置タップを学習 + HUD 表示へ畳む。案内中は左右タップもシークへ渡さない
- Win32 stream 所有と共通 `TouchRecognizer` は変えず、動画固有の位置→command 写像と案内状態を
  `NativeTouchAdapter` に置く。相対シークは absolute seek command を作らず、App の既存
  `native_video_seek_relative_with_hint` へ渡す
- HUD source は既存の widget passthrough を維持する。音楽ビューのファイルは変更せず、音声専用
  native shell には左右シークも初回案内も追加しない

#### Phase 3 Step 2c — 静止画ヘルプを読み方向で解決する (2026-08-09)

- 静止画 / 本の初回案内と `?` のコンテキストヘルプは、左右を方向に中立な
  「ページ送り」とせず、現在の読み方向で解決した「前のページ」/「次のページ」を表示する。
  LTR は左 = 前 / 右 = 次、RTL は左 = 次 / 右 = 前とする。判定は実際のタップページ送りと
  同じ `spread_mode.is_rtl()` を正本とし、シークバー専用の方向設定は参照しない
- 「通常はこう / RTL はこう」という対応表の併記は採らない。初回案内の役割は現在の画面で
  どこをタップすれば何が起きるかを一目で伝えることであり、無効な方向まで示すと利用者に
  自分の状態の判別を要求してしまう。アプリ自身が読み方向を把握しており、後から方向を変えた
  場合も `?` のコンテキストヘルプを同じ方向で再解決できるため、現在有効な割り当てだけを示す
- 長い「前のページ」/「次のページ」が中央矩形へ潜り込まないよう、左右ラベルは画面半分の
  中心ではなく、それぞれが説明する画面端〜中央タップ矩形間の帯の中心へ置く。外向き矢印は
  物理的な左右を示すものとして維持する
- **音楽ビューは egui surface であり、native touch アダプタの対象外**。通常の音声モードでは
  `fs_music_view_active` の面を egui が描き、native presenter は隠れているため、タッチは native
  アダプタへ届かない。波形タップのシークは音楽ビュー自身の既存動作で、HUD は常時表示される。
  `audio_only: true` の native overlay は音楽 VST シェル専用なので、**動画の native 挙動を
  音楽ビューで検証しない**

#### 忘れた人の再表示手段

既存のコンテキストヘルプ **`HelpShowContextShortcuts`**
(`docs/key-command-catalog-plan.md` Phase 7) に **「タッチ操作」欄を追加**する。
現在の画面で有効な操作だけを出せるので、通常のヘルプページより見つけやすい。
新しい機構は要らない。

#### 撤回: タッチセッション中の常時ハンドル表示

前案では「タッチセッション中は左右の呼び出しハンドルを薄く常時表示」としていたが、
**撤回する** (利用者判断 2026-08-06「常にボタンがあるのはよくない」)。

パネルへの到達は次の経路に確定した:

```
中央タップ → クローム表示 (上下バー + 左右のパネルハンドル) → ハンドルをタップ → パネルが開く
```

初回ヘルプが中央タップを習得させ、コンテキストヘルプから再確認できるので、
常時 affordance が無くても安全性は保てる。

#### 検討して採らなかった案: 右上に常時メニューアイコン

利用者提案 (2026-08-06)。「タッチセッションでだけ出す」なら
マウス利用者への影響は避けられるが、**第一案にはしない**:

| 論点 | 内容 |
| --- | --- |
| 画像を遮る | タッチ利用者に対しては**常に**マンガ・写真の隅を覆う。「閲覧中はクロームを隠す」という mIV の思想と衝突する |
| 意味の不一致 | 右上のハンバーガーは「**アプリメニュー**」に見え、「閲覧コントロールを出す」という意味と一致しにくい |
| 位置の競合 | × ボタンや既存上バーと位置・役割が競合する (上バーのボタンは右詰めで配置される、`ui_fullscreen.rs:21955-22010`) |
| 到達性 | 横持ちで左手側から操作すると遠い。**1 個だけではどちらかの利き手に不利**。下中央も大型タブレットでは両側から遠く、シークバーとも競合する |

→ 可視の affordance が要るなら、**単一の角アイコンより上記② の左右対称ハンドル**が優れる。
常設アイコンは、実機テストで② でも発見性が足りないと分かった場合の予備とする。

#### 「上下左右の全パネルを一斉に表示」は採らない

利用者提案に含まれていたが、これは旧 `adjustment_mode` と同じ挙動で、
記録済みの不満 (「右端にカーソルを寄せただけで、左の編集パネル + 右の情報パネル + 上バーが
一斉に出る」「編集パネルが出ること自体が怖い」) を再発させる。
左右パネルは画面をかなり覆うので、見ている画像が隠れる実害もある。

**表示するのは「全パネル」ではなく「全パネルへの入口」まで**:

- 上バー / ページ・動画シークバー / 左パネルを開くハンドル / 右パネルを開くハンドル

実際のパネルは各ハンドルをもう一度明示的にタップして開く。`ClickToShow` の思想とも整合する。

> **重要**: 中央タップ・ハンドル・アイコンなど複数の入口を用意する場合、
> **すべて同じ `ToggleChrome` 相当の動作にする**こと。
> 一方がクローム、もう一方が全パネルを開く、という設計は覚えにくく誤操作を招く。

#### 不採用: 端からの内向きスワイプ (2026-08-08 実機決定)

Windows 11 実機では、右端は通知センター、左端はアプリへ未配送、下端はスタートメニュー、
上端もアプリへ届かず、**上下左右すべてを OS が予約する**ことを確認した。mIV の gesture として
安定して成立しないため、edge swipe の owner / command / contact state / 閾値を含む実装は削除した。

左右パネルへのタッチ到達経路は、**「中央タップ → 見えるハンドル」の 1 本**に確定する。

#### 検討して採らなかった案: 4 分割 (上左右=パネル / 下左右=ページ移動)

利用者提案 (2026-08-06)。**1 タップでパネルへ直行でき、パネル表示で上バーも出るので
× へも到達できる**という利点があり、中央領域が無くても「詰み」は解消する。
しかし以下の理由で**既定にはしない**:

| 論点 | 内容 |
| --- | --- |
| 面積 | 上下 50:50 だとページ送り領域が全面 → **半分**に減る。ページ送りは圧倒的に高頻度で、パネルは低頻度。高頻度操作の領域を削って低頻度操作へ割り当てることになる |
| 誤タップ | 境界が見えないため、少し上をタップしただけで**補正/編集パネルが出る**。`docs/fullscreen-side-panel-mode-plan.md` に記録された既存の不満 (「編集パネルが出ること自体が怖い」) の再発と見なすべき |
| 学習コスト | 「左上 = 左パネル」という不可視の割り当てを覚える必要がある |
| ClickToShow との整合 | 明示操作でだけパネルを出す思想と相性が悪い。採用した**見えるハンドル**の方が意図を確認しやすい |
| **RTL** | 同じ画面左側に「**左上 = 画面固定の左パネル**」と「**左下 = 読み方向で次/前が反転するページ操作**」が混在する。不可視領域の意味が統一されない |

**中央タップを残す価値**: 「中央タップ = 操作 UI を出す」は Kindle / 動画プレイヤー /
多くのマンガビューアで確立した強いイディオムで、説明なしでも試される。
mIV では上バーだけでなく**閉じる / 左右パネルの入口 / 動画・音楽コントロール / 現在状態の確認**へ
到達する**共通の救済操作**になる。低頻度のパネルを 1 タップ短縮するために、
この共通入口を捨てるべきではない。

> どうしても 4 分割を採る場合は、上下 50:50 にせず
> **パネル領域を上端 20〜25% まで**に留めるのが上限 (ページ送り 75〜80% を残す)。
> それでも「見えない編集パネル領域を置く」問題は残る。

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

Step 3b で静止画フルスクリーンの導線を実装した。中央タップでクロームを latch すると、
`hover_pos()` に依存しない 48pt 幅の左右ハンドルが描画され、閉じている側のパネルを直接開ける。
開いた側のハンドルは描画・hit test・tap-zone exclusion のすべてから消える。

既存の hover callout はマウス用として従来の表示条件と形状を維持する。タッチ用 widget は
press / release フレームで同じ widget ID と矩形を保つため、release と同じ入力 batch に
`PointerGone` が来ても click completion に到達することを kittest で確認した (§6-4)。
初回オーバーレイヘルプによる発見可能性の補強は次ステップに残す。

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

> ⚠ **slider について訂正**: 上表の `slider handle` は**見た目**の寸法で、
> 実際の hit target は `slider response` 全体 (幅 100 × 高さ最低 18) である。
> track のどこを押しても操作できるので、**handle が小さいこと自体は実操作上それほど深刻ではない**。

#### UI 倍率を上げれば済むか — 部分的にしか済まない

「タッチ中心なら UI 倍率 150% / 200% にしてください」と案内する案の評価
(OS 倍率 100%、mIV の UI 倍率だけを掛けた計算):

| 対象 | 100% | 150% | 200% | 判定 |
| --- | ---: | ---: | ---: | --- |
| ★ | ≒20×20 | ≒30 | ≒40 | 150% で 23px、200% で 40px 相当。ただし実 rect は font galley 依存で**保証はできない** |
| **タグ ON/OFF (高)** | 18 | 27 | **36** | 23px は満たすが、**200% でも 40px に届かない** |
| カラー化スロット | 22×22 | 33 | 44 | 150% で 23px、200% で 40px を満たす |
| slider response (高) | 100×18 | ×27 | **×36** | 23px は満たすが、**200% でも 40px 未満** |
| slider handle (見た目) | 10.8×14.4 | 16.2×21.6 | 21.6×28.8 | 実 hit target は track 全体なので実害は小さい |
| 静止画ブックマーク追加 | 24×20 | 36×30 | 48×40 | 150% で 23px、200% で 40px を満たす |
| native ブックマーク | 26×24 | 39×36 | 52×48 | 150% で 23px、200% で 40px を満たす |

→ **200% でも 40px 基準に届かないのはタグボタンと slider の高さ**。

**副作用 (情報密度)**: 全要素が比例するので、面積あたりの密度は
**150% で約 44%、200% で約 25%** になる。実害は:

- 一覧で同時に見えるサムネイル / 行が減る
- 左パネルで AI モデルやカラー化コントロールを見るためのスクロール量が増える
- 右パネルでタグが早く折り返される
- 動画 HUD / 上バーが映像面積を多く使う
- **10〜13 インチの縦持ちでは 200% はかなり窮屈**

さらに **Windows タブレットは OS 側で既に 150〜200% の DPI 倍率になっていることが多い**。
そこへ mIV の 200% を重ねると想定以上に大きくなる可能性がある。

#### 結論: 案内 + 限定的な hit resolver の併用

- **案内は「タッチ中心では 150% がおすすめ。小さく感じる場合は 200%」**程度に留める。
  **「タッチ利用には 150% 以上が必須」とは書かない** (それは「設定を変えないと使えない」の意で、
  §5.1-6 の無設定方針とも方向が逆)。
- **hit resolver は名指しされた周辺の少数コントロールだけ**に入れる:
  ★ 行 / タグ ON-OFF / AI モデル行 / カラー化スロット / slider の縦方向 hit / ブックマーク追加。

工数 (Phase 1 で touch provenance / ownership が完成している前提):

| 対象 | 概算 |
| --- | ---: |
| 共通 ★ 行を 5 等分 | 0.5〜1 人日 |
| 静止画・音楽・native のタグボタン | 0.75〜1.5 人日 |
| AI radio / checkbox を行全体タップ化 | 0.5〜0.75 人日 |
| カラー化スロット・slider の hit 拡張 | 0.75〜1.25 人日 |
| 静止画 / native ブックマークボタン | 0.25〜0.5 人日 |
| overlap・倍率・snapshot / 実機テスト | 0.5〜1 人日 |

**静止画・音楽中心なら 2〜3 人日、native 動画まで共通化して 3〜5 人日。**
Phase 2 の 7〜11 人日のうち **25〜35%** に相当する (= 既に見積もりに含まれている)。

⚠ **実装上の注意**: タッチを検出したフレームだけ widget 自体を大きくすると**レイアウトが跳ねる**。
**見た目の rect は固定したまま、raw touch 位置から action を解決する**方式にすること。

**Step 3c 実装記録 (2026-08-07)**: 最初の限定 slice として静止画上バーへ適用した。
`draw_bar_button` が描いた 32×32pt の各 rect を viewport ごとの `ctx.data_temp` に記録し、
次フレームの相関済み touch tap をバー全高 + 隣接中心の中点境界で最寄り id へ解決する。
右端 id はバー右端まで、左端 id の左側は未割当のままにする。egui の widget / paint rect と
mouse hover/click は変更せず、解決 id だけを既存 `Response::clicked()` 相当の分岐へ合流させる。
バー内の未割当 touch tap は背景のページ送りにも通さない。

最終的に説明できる状態:

> **100% でも主要な閲覧・指定機能は操作できる。150% にするとより快適。**

全面的なターゲット見直しは今回の範囲に対して費用対効果が悪く、逆に**案内だけでは
「タッチ対応だが小さいコントロールは設定変更が必要」という弱い完成度**になる。

### 5.8 選択済みセルの再タップで開く

**見送り (利用者判断 2026-08-07)**。「タップ操作はとりあえず閲覧用としているので、
まずは選択機能はなくてもよい」との判断により、選択済みセルの 1 回タップ open は実装しない。
一覧を開く操作は既存のダブルクリック / ダブルタップ / Enter を維持する。

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
3. `WM_POINTERDOWN` で `GetPointerType` を呼び、その結果を stream の所有判断に固定する
4. **`PT_TOUCH` と確定した stream は DOWN から UP/Cancel まで全メッセージを処理し、常に 0 を返す**
5. `PT_PEN` その他は stream 全体を従来どおり `DefWindowProc` へ渡す
6. touch stream を `NativeVideoWindowEvent::Touch` で native overlay の共通認識器へ渡し、
   先頭接点だけを overlay の egui Context へ pointer emulation する
   → **egui 側と同じジェスチャ認識器を共有できる** (OS アダプタだけが別実装)
7. `WM_POINTERCAPTURECHANGED` と `POINTER_FLAG_CANCELED` で必ず状態を解放する

`DefWindowProc` を touch stream に対して呼ばなければ、その経路からの promoted mouse を避けられる。
Microsoft は **「一つの pointer stream の一部だけを消費し、残りを `DefWindowProc` に渡す動作は
未定義」** と明記しているので、**stream 単位で丸ごと所有するか丸ごと渡すか**にすることが重要。

さらに既存の `WM_MOUSE*` handler で `GetCurrentInputMessageSource()` を確認し、
`IMDT_TOUCH` と確定した重複だけを捨てる安全網を置ける。
**失敗または `IMDT_UNAVAILABLE` の場合は捨てず、従来の mouse handler へ流す**のが安全側。

#### Phase 1 Step 4 実装結果 (presenter の薄い縦切り)

- 出荷ゲートでは未登録のまま presenter HWND に `PT_TOUCH` 237 件、HUD HWND に 35 件が届き、
  **案 C が成立することを実機確認済み**。
- 今回所有するのは **presenter HWND だけ**。`WindowState` (HWND ごとの `GWLP_USERDATA`) に
  上限付き pointer-id 集合を持ち、DOWN で所有した stream だけを UP / canceled flag /
  `WM_POINTERCAPTURECHANGED` まで一貫して消費する。Touch event は latest-value slot に載せず、
  pump/render の bounded lossless route へ全件送る。
- `POINTER_INFO.ptPixelLocation` は `ScreenToClient(presenter_hwnd)` で物理 client pixels にし、
  mouse と共通の client-pixels→egui-points 純関数へ合流する。マルチモニタでも変換基準は HWND。
- presenter overlay は既存の `TouchRecognizer` をそのまま所有し、
  `TouchSurfaceBehavior::Viewer { accepts_pinch: false }` で認識する。
  `touch_correlation.rs` は通さない。先頭接点だけを pointer emulation し、
  `should_suppress_primary()` が立った stream では click を完成させない。synthetic press が
  既に生きている場合だけ、click 距離を十分超える `PointerMoved` → primary release →
  `PointerGone` の順で egui の down 状態を確実に解除する。
- `ToggleChrome` と `PageSide` はどちらも session-only chrome latch の toggle に写像する。
  latch は hover 可視判定への OR 入力で、`SwitchSource` の専用 source-session reset で
  ファイル移動時に解除し、fullscreen 終了時は overlay の破棄・再生成によって false に戻る。
  永続設定は変更しない。
- touch DOWN のうち、最初の所有 stream かつ presenter の activation または thread focus が
  不足している場合だけ typed `RequestFocusClaim` を pump へ送り、既存
  `claim_foreground` 経路へ合流する。既に foreground/focus を持つ tap と2本目以降では送らない。
  非アクティブ判定 (activation tap) は既存 `WM_MOUSEACTIVATE` と揃え、
  child は foreground が他プロセスのとき、top-level popup は presenter 自身が foreground でないとき。
- **activation tap の扱いはマウスと意図的に変える** (2026-08-08、実機判断)。
  マウスはアクティブ化クリックを `MA_ACTIVATEANDEAT` で丸ごと食べる。presenter 本体の左クリックが
  **再生 / 一時停止という副作用**を持つからである。タッチの presenter 本体はクロームを出すだけで
  副作用が無く、非アクティブな窓をタップした人は次に操作する意図がある。したがって
  **ジェスチャは通常どおり認識し、overlay の control へ届き得る synthetic press だけを抑止**する。
  = 復帰タップでもクロームは出る。gesture ごと捨てると、前面状態次第で 1 タップ目の
  意味が変わり挙動が読めなくなるため、この形に固定した。
- **Phase 1 Step 4 時点では HUD HWND を意図的に対象外にした**。HUD は promoted mouse で
  既存ボタンを操作できていたため、当時は presenter 側だけを所有し、HUD 上の
  長押し→右クリック合成を Phase 3 へ残した。現状は下記 Phase 3 Step 1 で解消済み。
- `MIV_DISABLE_TOUCH_GESTURES=1` では所有と promoted-mouse filter を無効にし、従来経路へ戻す。
  Cancel / capture loss は防御的に実装したが、実機では引き続き未観測。

#### Phase 3 Step 1 実装記録 (HUD HWND の所有)

- HUD の HWND ごとの `WindowState` に presenter と同じ上限付き `NativeTouchOwnership` を持たせ、
  `PT_TOUCH` と確定した DOWN だけを stream 単位で所有する。type probe、followup、座標変換、
  canceled / UP / `WM_POINTERCAPTURECHANGED` の終端は両 wndproc で同じ Win32 helper を共有する。
  未登録 pointer id、問い合わせ失敗、非 touch、上限超過は従来どおり `DefWindowProcW` へ渡す。
- `NativeVideoTouchEvent.source: NativeVideoWindowSource` を正本に presenter / HUD の発生元を保持する。
  overlay の `NativeTouchAdapter` は 1 つのままで、先頭接点だけが primary emulation を所有する。
- **HUD HWND に届いた事実を OS の hit-test 結果として採用し、アプリ側で hit-test をやり直さない**。
  HUD source は `TouchRecognizer::handle_widget_passthrough_sample` から開始 owner を直接
  `WidgetPassthrough` にし、`classify_tap` と presenter の `compute_hud_regions()` 近似を通さない。
  このため近似矩形がずれても HUD 操作が viewer tap / chrome toggle へ化けない。
- 所有した HUD touch 由来の promoted mouse は全 mouse handler で
  `GetCurrentInputMessageSource()==IMDT_TOUCH` と確定した場合だけ捨てる。問い合わせ失敗と
  `IMDT_UNAVAILABLE` は fail-open、`MIV_DISABLE_TOUCH_GESTURES=1` は所有・filter とも無効。
- HUD touch DOWN は既存 mouse-down と同じ `RequestFocusClaim` を送り、touch 用の `SetCapture` は
  呼ばない。`WM_CAPTURECHANGED` / `WM_CANCELMODE` / `WM_DESTROY` / `WM_NCDESTROY` は mouse の
  synthetic-up cleanup と同じ `emit_input_cleanup` で全 owned touch を Cancel して解放する。
- `MIV_TOUCH_DEBUG=1` の ownership / 座標 / command / promoted-mouse discard ログに
  `presenter` / `hud` の source を出す。自動テスト完了、実機での HUD ボタン操作と長押しは確認待ち。

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

#### 1 アプリフレーム内の複数 pass で守る戻り値契約

egui はレイアウト変更や `request_discard` により、同じアプリフレームを複数回 drive する。
`TouchFrame` が返す情報は、次の異なる寿命を持つものとして扱う:

- **`commands` は最初の drive で 1 回だけ返す**。コマンド実行はページ、クローム、zoom / pan、
  PDF 再描画などの状態を変えるため、同じフレームの後続 pass では必ず空にする
- **相関済み `primary_events` と抑止判定は後続 pass にも同じ内容を返す**。これは
  副作用のない問い合わせであり、後続 pass で解決される widget click や Step 3c の
  バーボタンも参照するため

実装では `touch_correlation` が最初の drive の結果からコマンドを除いた replay 用
`TouchFrame` だけを保持し、同一フレームの 2 回目以降へ返す。呼び出し surface 側に
pass 番号や「実行済み」guard を置いてはならない。新しい surface もこの戻り値契約を
そのまま利用する (2026-08-07、実機の中央タップ二重 toggle 報告を受けて明文化・修正)。

##### ⚠ 実機で 3 回踏んだ同型バグ — 「replay される値で状態を作らない」

複数 pass の実害はここまでに **3 回**出ている。いずれも
**「1 回しか起きないはずの出来事」を、replay される値から判定していた**ことが原因:

| # | 症状 | replay された値 | 壊れた前提 |
| --- | --- | --- | --- |
| 1 | 中央タップでクロームが出ない (2 回必要) | `commands` | 「コマンドは 1 フレームに 1 回」 |
| 2 | ピンチが 2 回に 1 回しか効かない | egui-winit の pointer gate | 「gate は先頭接点だけ通す」 |
| 3 | パネル外タップで閉じた後、マウスも含めて左入力が全部死ぬ | 終端 `TouchFrame` の抑止判定 | 「arm した bool には必ず release が来る」 |

**規則**: replay される値は**問い合わせ (query)** であって**出来事 (edge)** ではない。

- replay される値から **cross-frame の状態を arm しないこと**。
  arm するなら、その状態は**自分で Idle へ戻れる**必要がある
  (例: `FullscreenPrimarySuppression::TouchStream` は接点が無いフレームで必ず Idle に戻る)
- 「未来のエッジが来たら解除する」設計にしないこと。
  そのエッジは**別の pass で既に消費されている**可能性がある
- 相互排他の状態を bool で持たない。**typed owner** にして、
  各 owner の arm と clear を 1 か所で対応付ける (#3 の修正がこれ)
- 新しい surface / 新しい抑止を足すときは、この表を先に読む

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
- 確定済み owner を**同じ接点集合のまま**別 action へ移さない。ただし接点追加時は
  `ViewerPointerPassthrough` から `Pinch` へ昇格し、現在位置で pinch 基準を取り直す
- `WidgetPassthrough` / `ViewerTapZone` / `Cancelled` からは昇格しない。
  `Pinch` は接点が 1 本に戻っても単指 pan へ降格せず、全接点解放まで保持する
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
| **2** | **一覧・静止画/音楽パネル**。grid の touch drag scroll (行スナップ維持 + 方向 settle)、pinch 列数変更、中央クローム + 左右 callout、静止画の AI/カラー化/ブックマーク/★/タグ、音楽の ブックマーク/★/タグ、named widget の touch hit target、タグ入力の IME / タッチキーボード確認 | **7〜11 人日** | grid selection / D&D=中〜高, サムネ仮想化・prefetch=中, マウス click/double-click=低, IME=中 |
| **3** | **動画 native の完成**。play/pause・seek・前後 file・HUD 表示・close、native 中央クローム/callout、ジャンプ/ブックマークパネル、補正パネル、★/タグパネル、native `ScrollArea` の drag/慣性、native ターゲットサイズ調整、mouse/touch/pen 混在・複数 HWND・DPI・capture loss の hardening | **8〜13 人日** | **native presenter/HUD=高**, **mouse promotion/filter=高**, 動画再生自体=中 (入力 transport 以外に触れないこと), detached/media window=中〜高, VST/IME=中 |

**総工数: 22〜34 人日**。実機で native HWND の配送差や Cancel 問題が出た場合は上振れする。

Phase 2 の当初内訳: パネル open + ターゲット調整 4〜7 人日 /
grid scroll + 行スナップ + prefetch + pinch 列数変更 3〜5 人日。
再タップ open の 1.5〜2.5 人日は §5.8 の見送り判断により対象外。

**「設定なし」にしたことの工数影響**:

- 設定 field / migration / preferences UI / ドキュメントを作らない: **約 0.5〜1 人日減**
- source correlation、fail-closed 排他、mixed-device 試験: **約 2〜4 人日増**
- → **正味 1.5〜3 人日程度の増**。単純な「タッチモード設定」案より高くつくが、
  マウス回帰リスクは下がる。

Phase 1 の入力基盤は `src/touch_input.rs` (認識器の純ロジック) と
`src/touch_correlation.rs` (egui の順序相関、viewport + surface 単位の一時状態) に分離する。
後者は Step 2 時点ではコマンドと抑止要否を計算するだけで、既存入力へ適用しない。
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

### 5.14 決定事項 (すべて確定済み)

利用者判断 (2026-08-06、#3 は 2026-08-07、#11〜13 は 2026-08-08 更新)。**未決事項はない**。

| # | 論点 | 決定 |
| --- | --- | --- |
| 1 | 対象範囲 | **動画も必須**。静止画・本・動画・音楽の閲覧系をカバーする |
| 2 | 左右パネル | **対応する** (AI アップスケール / カラー化 / ブックマーク / ★ / タグ) |
| 3 | 選択済みセルの再タップ open | **入れない**。タップは当面閲覧操作を優先し、既存のダブルクリック / ダブルタップ / Enter を維持する |
| 4 | タッチ設定 | **作らない**。マウス無影響で、タッチしたときだけ別動作 (§5.2 の fail-closed) |
| 5 | **ペンの扱い** | **(a) 特別な対応をしない** = **ペンはタッチ扱いになる**。§5.16 |
| 6 | **診断用の強制無効** | **持つ**。`MIV_DISABLE_TOUCH_GESTURES=1` 等。簡単に付けられる範囲で |
| 7 | **タップ領域レイアウト** | **中央矩形案で実装する**。気になれば実機を見てから他案を検討 |
| 8 | **UI 倍率** | 案内は「おすすめ」に留め強制しない。**+ オーバーレイヘルプにスケーリングへの導線を添える** (下記) |
| 9 | **動画のシーク量** | **±5 秒** (mIV のキーボードと同じ刻み。アプリ内の一貫性を優先) |
| 10 | 動画の AI アップスケール / カラー化 | **静止画のみ対象**。動画再生中には元から機能が無く、タッチ対応で追加する話ではない |
| 11 | **非アクティブな窓への最初のタップ** (2026-08-08) | **食べない**。ジェスチャは通常どおり動かし、overlay control へ届く synthetic press だけ抑止する。静止画 egui 面では中央の `ToggleChrome` だけを通し、左右の `PageSide` は前面化だけにしてページを送らない。初回ヘルプ表示中の全域タップは既存どおり learn + chrome 表示へ写像する。マウスは最初のクリックが再生/一時停止という副作用を持つので従来どおり食べる。§5.9 |
| 12 | **エッジスワイプ** (2026-08-08 実機) | **不採用・実装削除**。Windows 11 では上下左右すべてが OS 予約で安定配送されない。左右パネルは「中央タップ → ハンドル」の 1 経路にする。§5.5 |
| 13 | **動画のタップ領域と HUD** (2026-08-08 実機) | 静止画と同じ 3 領域を使い、中央タップは HUD toggle、左 / 右の単発タップは -5 / +5 秒シーク。時間窓によるダブルタップ判定は撤回・削除し、シークは chrome latch を変更しない。§5.5「動画・音楽のタップ割り当て」 |

#### 11 の補足: v2.13.0 静止画 egui 面の実機バグ修正

2026-08-08 の実機ログでは、他アプリが foreground の間も egui event 列には
`Touch(Start) → PointerMoved → primary press` が届いていたが、focus reclaim の早期 return
branch が `drive_egui_touch_input(..., false)` を呼んだため、相関ログは
`commands=0[] contacts=0->0 pointer_touch=absent->absent` のままだった。11 回のタップが
すべてこの形で捨てられ、初回ヘルプも閉じられなかった。

原因は Step 3d の退行ではなく、タッチ対応以前からある「前面復帰クリックを操作に使わない」
マウス向け branch が、`enabled=false` を観測停止として扱っていたことである。早期 return、
foreground 奪還処理、`fs_primary_suppression` は維持し、branch 内で touch correlation
だけを実行可能にした。返却結果は `ToggleChrome`（初回ヘルプ中の learn + chrome を含む）だけを
採用し、`PageSide`、pinch、pan、widget への synthetic primary は採用しない。

#### 8 の補足: オーバーレイヘルプにスケーリングへの導線を添える

初回オーバーレイヘルプ (§5.5) の隅に、
**「UI が小さい場合はメニューの『スケーリング』から変更できます」**程度の
**従属的な 1 行**を添える。

- タッチを学習している**まさにその瞬間**に出るので文脈が合っている
- **強制しない**。たまにしか使わない利用者はボタンが小さいままでも気にしない
- ジェスチャの図示より**視覚的に従属させる** (小さく、下部に置く)
- (任意) **UI 倍率が 100% のときだけ出す**。既に 150% 以上で使っている利用者には不要。
  これは永続設定の静的な参照であって実行時状態による分岐ではないので、決定性方針に反しない

### 5.15 「マウス操作を変えない」の保証範囲 (明文化)

Codex Sol の指摘により、保証できる表現を次に限定する:

> **タッチを行っていない mouse-only の入力列については、挙動を一切変更しない。**

Step 3b の左右パネルでは、この保証を writer と resolver の両方で固定した。
`ByPointer` を生成する既存 mouse / native / music 経路は `ClickToShow` 以外で no-op、
`Hover` でも永続表示できる `ByTouchHandle` はタッチ専用入口からだけ生成する。
左右とも owner を bool へ分散せず `MetadataPanelOpenState` で型として区別する。

同じ egui viewport 上でマウスとタッチを**完全に同時**操作した場合、egui の `PointerState` が
1 つしかない以上、2 つを完全独立にする保証はできない。ここを絶対条件にすると
egui-winit より下の入力層を大きく作り直す必要がある。実用上は
「タッチしながら同時にマウスも動かす」という操作は稀なので、この限定で足りると判断する。

### 5.16 ペンの扱い

#### 前提の訂正 — ペンのホバーは今すでに効いている (見込み)

「ペンをマウス扱いにすればホバーが使えるようになる」という整理は**正しくない**。
実装を読む限り、**ペンのホバーは現状で既に egui のポインタを動かしている**:

- winit の `WM_POINTER` ハンドラは phase を
  `POINTER_FLAG_DOWN` → Started / `POINTER_FLAG_UP` → Ended / `POINTER_FLAG_UPDATE` → Moved
  で決めており、**`POINTER_FLAG_INCONTACT` を見ていない**
  (`winit-0.30.13/src/platform_impl/windows/event_loop.rs:2103-2124`)。
  → 画面に触れていない in-air のペン移動も `TouchPhase::Moved` として通る。
- egui-winit の `on_touch` は、接点を掴んでいない状態 (`pointer_touch_id.is_none()`) の Moved を
  **`on_cursor_moved()` に流す** (`egui-winit-0.33.3/src/lib.rs:703-720`)。
  → ペンをかざしただけで egui のポインタ位置が動く = **ホバー UI が出る**。
- 仮に winit が `WM_POINTERUPDATE` を消費しない環境でも、その場合は OS のマウス合成で
  `WM_MOUSEMOVE` が来るので、**どちらの経路でもホバーは成立する**。

⚠ これはコードからの推定で、**実機確認が必要** (§6)。ただしどちらの経路でも成立するため、
結論は比較的堅い。

したがって **(a)/(b) の争点はホバーではない**。争点は
**「ペンをタッチとして扱うか、精密ポインタとして残すか」**である。

#### 選択肢

| 案 | 内容 | ペンで失うもの / 得るもの | 追加工数 |
| --- | --- | --- | --- |
| **(a)** | ペン接触もタッチとして扱う | グリッドからの **native ファイル D&D が使えなくなる** (タッチでは抑止するため)。中央タップがペンでも発火する | ゼロ |
| **(b)** | ペンは完全に従来のマウス扱い | 何も変わらない (現状維持)。タッチ用の大きなクロームもペンでは出ない | 中〜大 |
| **(c)** | **ペンをタッチとして扱うが、D&D 抑止などマウス精度が要る挙動だけ除外する** | ホバー・D&D は残り、タッチ用クロームも使える | **小** |

#### 競合 NeeView は (a) 相当

NeeView は **ペンと指を区別していない**。リポジトリ全体で `TabletDevice` /
`TabletDeviceType` の参照は **0 件**で、すべて WPF の `StylusDevice` として一様に扱っている。

ただし `StylusInAirMove` (= ペンのホバー) は購読しており、`Config.Current.Mouse.IsHoverScroll`
が有効ならホバースクロールに使う (`NeeView/TouchInput/TouchInputNormal.cs`)。
→ **「ペン = タッチ扱い。ただしホバーは別途活用する」**という構成。

#### (b)/(c) を実現する手段 — vendoring は要らないかもしれない

当初「winit / egui-winit を patch する必要がある」と見積もったが、**より安い手がある**:

- winit は `WM_POINTER` 経路で **`Touch.id` に `pointer_info.pointerId` をそのまま入れている**
  (`event_loop.rs:2120`)。
  → mIV から **`GetPointerType(pointerId, &mut ty)`** を呼べば `PT_PEN` / `PT_TOUCH` を判定できる。
  eframe / egui-winit に手を入れる必要がない。
- `WM_TOUCH` 経路の場合、`id` は `input.dwID` で **別の ID 空間**なので `GetPointerType` は失敗する。
  → **失敗したら「指」と判定すればよい**。ペンは `RegisterTouchWindow` の対象外で
  `WM_TOUCH` 経路には来ないため、この縮退は正しい向きに転ぶ。
- ポインタが retire された後は取得できないので、**Down 時に判定してキャッシュ**する。
- 補助シグナル: **指は原理的にホバーできない**。接触前に in-air の Moved を出した接点は
  ペンと判定できる。`GetPointerType` が使えない環境の保険になる。

いずれも誤判定時は (a) の挙動に落ちるだけなので、§5.2 の fail-closed 方針と整合する。

#### 推奨: (c)

利用者の直感 (「タブレット PC でペンはタッチ扱いでよさそう」) は妥当で、NeeView も同じ判断。
その上で、**ペンと判定できたときだけグリッドの file D&D 抑止を外す**のが素直。
判定に失敗しても (a) に落ちるだけで壊れない。

#### 決定: (a) 特別な対応をしない = ペンはタッチ扱い

利用者判断 (2026-08-06)。理由は「わざわざペンを持って操作するケースは少なそう」。
**稀な入力源のために判定コストと誤判定リスクを負わない**という判断で、NeeView と同じ結論。

**この決定で失うもの (1 つだけ)**:

- **グリッドからのファイル D&D がペンでできなくなる**。
  タッチ由来のドラッグは D&D を抑止してスクロールに回すため、ペンでドラッグすると一覧がスクロールする。

**失わないもの**:

- **ペンのホバーはそのまま効く** (かざすだけで egui のポインタが動く)。上記のとおり
  (a)/(b) いずれでも変わらない
- マウスの挙動 (D&D 含む) は一切変わらない

**後から (c) へ移行できる**。`GetPointerType` による分類は入力アダプタに 1 段足すだけで、
設計を崩さない加法的な変更。実運用で「ペンで D&D したい」という要望が出たら追加する。

---

## 6. 未確定事項 / 実機確認が必要な項目

1. **【通過済み】`WM_POINTER*` 配送ゲート**。実機ログで presenter HWND に
   `PT_TOUCH` 237 件、HUD HWND に 35 件を観測し、touch-unregistered の実 HWND 構成
   (hit-test region / DirectComposition / `WS_EX_NOACTIVATE` の HUD) でも案 C が成立すると確定した。
2. **【presenter / HUD とも実装修正済み・実機確認待ち】** 動画 native presenter の長押しでは、
   `WM_POINTERUP` 後に合成された短い右クリックでフルスクリーンが閉じた。
   presenter の `PT_TOUCH` stream 全体を所有し `DefWindowProcW` へ渡さない構造へ修正済み。
   Phase 3 Step 1 で HUD HWND も同じ whole-stream 所有へ移し、HUD 上に残っていた
   長押し→右クリック合成も構造上解消した。変更後ビルドで presenter / HUD の双方について
   「長押ししても右クリック扱いにならず、フルスクリーンも閉じない」ことを確認して解消確定とする。
3. **【実装解決・Cancel 自体は実機未観測】egui-winit の Touch Cancel が primary release を出さない件**
   (`egui-winit-0.33.3/src/lib.rs:732-735`)。アプリ側の gesture state は Cancel で破棄できるが、
   `PointerGone` 単独では egui の primary-down は解除されない。native adapter は
   synthetic press が生きている Cancel / suppression 境界で、click 距離超過の
   `PointerMoved` → primary release → `PointerGone` を注入し、click を成立させず down を解除する。
   canceled flag / capture changed の実機観測は引き続きできていない。
4. **【実機確認済み・仕様更新】左右パネルのタッチハンドル**。
   hover 依存の既存 callout はタッチ target にせず、クローム latch 中に widget ID と 48pt 幅の
   矩形が継続する別ハンドルを実装した。Touch End と同じ batch に `PointerGone` を入れる
   kittest で click completion と左右 action 発火を確認済み。実機で open を確認後、開いた側の
   ハンドルがパネル操作を塞ぐことが分かったため、開いた側の handle / hit / exclusion を消し、
   touch-owned パネル外タップは左右をまとめて閉じる仕様へ更新した。
5. 「フォルダを開く」ダイアログはフルパス入力式で参照ボタンが無く
   (`src/ui_dialogs/open_folder.rs:34-52, 81-106`)、OS タッチキーボードの自動表示も
   コードからは保証できない。タブレットでの実用性を実機確認する必要がある。
   なお「任意の場所へ移動」は `場所▼` / ドライブ一覧 / ピクチャ等の既存導線で到達できる
   (`ui_main.rs:10942, 11015`) ので、新しいフォルダピッカーは必須ではない。
6. スクロールバーの実効的な掴みやすさ (DPI・UI 倍率・機種依存)。
7. detached viewer で、root と別 viewport のタッチ状態が混ざらないこと。
   passive detached の最初の**マウスクリック**は従来どおり復帰だけ、最初の**中央タップ**は
   chrome 表示まで届く一方、左右タップでページが動かず widget press にもならないことを
   実機確認する。

---

## 7. 推奨ロードマップ

1. **Phase 0 (即時)**: §4 の回避策を利用者へ案内する。
   これだけでフルスクリーンの「詰み」は回避でき、タブレットでも一応閲覧できる状態になる。
2. 仕様は §5.14 で**すべて確定済み**。着手前に決めるべきことは残っていない。
3. **Phase 1**: 入力源分離と 2 つの backend の成立確認。
   静止画フルスクリーンのタッチ操作一式に加え、**動画 native の pointer adapter を
   presenter HWND に薄く実装済み**。配送ゲートは通過し、長押し修正後の実機確認を残す。
4. **Phase 2**: 一覧の直接スクロール + 方向スナップ + ピンチ列数変更を実装済み。
   Step 3b で静止画パネルの open 導線も実装した。パネル内部の小 target 対応と音楽パネルは残る。
5. **Phase 3**: 動画 native の完成。ここまでで「mIV はタッチ対応」と表明できる。
6. 実機フィードバック後に**慣性の要否を判断**する。必要なら短距離・行単位の限定慣性だけ。
7. リング / ルーペ / 5 領域カスタマイズ / ピンチ回転は**要望が出るまで保留**。
   これらより「閉じる・戻る・スクロール・ピンチの確実性」に工数を使う方が効果的。

**Phase 1 で静止画を先に完成させる**のは、フルスクリーンが「入ったら出られない」罠に
なっているため。Phase 2 を先にすると、指でスクロールできるようになった利用者が
サムネイルをタップしてフルスクリーンに入り、そこで詰む — という悪化した体験になる。

**動画 native の入力経路は Phase 1 で presenter に薄く実装した**。
`WM_POINTER` が presenter / HUD の実 HWND に届く出荷ゲートは通過済みで、案 C は成立する。
HUD 側の stream 所有と pointer emulation は Phase 3 Step 1 で実装済み。
ターゲットサイズ調整は実機判断を待つ後続 Step として残す。

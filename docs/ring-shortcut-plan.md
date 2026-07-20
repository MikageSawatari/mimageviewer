# リングショートカット (マウス右フリック / ゲームパッド) 設計叩き台

マウスとゲームパッドから、あらかじめ用意した機能を素早く呼び出すための
「リングショートカット」と「パッド専用ピッカーパネル」の設計メモ。

キーボードは従来どおり `keymap.ini` で上級者向けにフルカスタマイズ可能なまま据え置く。
本機能は **マウス / ゲームパッドの操作面を限定的に拡張する** もので、完全カスタマイズは
目指さない (実装量とサポート負荷を一定に抑え、コア閲覧操作を壊させないため)。

関連: [keymap-spec.md](keymap-spec.md) (現状「マウス・ゲームパッドは keymap 対象外」と
明記している。本機能はその唯一の例外になるので、確定後に追記する)。

---

**2026-06-25 追記**: NeeView 風マウスジェスチャー導入に向け、右ドラッグの用途をグリッド / 画像フルスクリーン / 動画フルスクリーン / 編集モードの 4 文脈ごとに `未使用` / `リングショートカット` / `マウスジェスチャ` から選ぶモデルへ拡張した。旧 `mouse_flick_enabled` は互換読み込み用として残し、未設定の既存環境では従来どおり 3 表示文脈のリング ON/OFF として解釈する。per-context mode 保存、マウスジェスチャ登録 UI、入力状態機械、長押しガイド表示まで実装し、登録済みジェスチャ一覧はグリッド / 画像フルスクリーン / native 動画で同じ中央パネルのサイズ感に揃える。編集 UI は環境設定から独立した設定メニュー「操作カスタマイズ…」へ移す。編集モードの右ドラッグは既存の右クリック編集操作と競合するため、初期版では `未使用` / `マウスジェスチャ` のみを選択可能にする。ゲームパッド設定では X+方向セルをクリックすると該当方向のリングスロット編集を開く。マウスジェスチャ追加は、行を空で増やすのではなく、専用ダイアログで実際に右ドラッグして方向列を記録してから追加する。`GridColumnCount1..10` はリング / マウスボタン / X+方向 / ジェスチャ候補にも追加し、サムネイル列数をマウス操作等へ割り当てられるようにした。お気に入り 1〜20、`C:\`〜`Z:\`、場所▼の固定項目 (ドライブ一覧 / 読書履歴 / ★1〜★5 / 本棚 / デスクトップ / ピクチャ / ダウンロード)、ZIP/PDF/対応アーカイブの `ページを開く` / `一覧を開く` も一発アクションとして追加し、右ドラッグ・マウスボタン・X+方向へ割り当てられるようにした。固定ゲームパッドボタン (A/B/X/Y、Select/Start、LB/RB、LT/RT) は既定動作で使い、カスタマイズ対象は X+方向リングに絞る。X 単体は常にピッカーパネル、X+方向は保存済みの 8 方向リングスロットを実行する。

## 1. 背景・目的

- 現状、マウスは固定割り当て、ゲームパッドは閲覧専用の固定割り当てで、呼び出せる機能が少ない。
- 「本棚への追加をパッドから」「動画の音量をパッドから」のような要望が見込まれる。
- 完全な再割り当て UI は重い。**厳選した機能を、限定された入力に割り当てるだけ**にして、
  多くのケースをカバーしつつ実装を一定量に抑える。

## 2. 他アプリの方式との位置づけ (要約)

- マウスで機能を増やす方式は大きく 2 系統。
  - 軌跡ジェスチャー (NeeView の右ボタンドラッグ「↓→」等、Opera/Firefox): 速いが**見えない**。
  - リング/パイメニュー (Blender / Krita / Maya マーキングメニュー / Steam Input ラジアル):
    **見える**＝発見可能で、マウス/パッド/タッチで同じ操作感。
- 本機能は後者 (リング) を採用しつつ、**マーキングメニュー方式** (ためらえばガイド表示、
  素早くフリックすれば即発動) で初心者の発見性と上級者の速度を両立する。
- Steam Input の知見: リングは標準 5 / 最大 20。入力源はスティック/十字/パッド/タッチ。
  → マウスは 8 方向で実用十分、パッドは十字=4 基本が確実・スティック=8 (斜めは best-effort)。

## 3. 設計方針

1. **キーボードは不変** (本機能の対象外)。
2. アクションを 2 種類に仕分ける:
   - **一発もの・トグル・1〜2回で済む循環** → リング / フリック (8 方向)。
   - **多状態・特定値を選びたい** → パッド専用ピッカーパネル (X 単体)。
     状態が操作直後に分かりにくいもの (連結方式・見開き・フィット・サムネ比率など) も
     ブラインド巡回させずパネルで選択・確定する。
3. **マウスとパッドは共通設定** (「リングショートカット」1 セット)。デフォルトはパッド向けに
   最適化する。マウスフリックを活用したいユーザはカスタマイズで上書きする
   (マウス/パッド併用ユーザは少数で、共通設定でも実害が小さいと判断)。
4. ピッカーパネルは**パッド専用**。マウスの多状態操作はツールバー / ホバーバー / 既存
   コンテキストメニューで足りる。マウスは**短タップの既存メニューは維持し、右ドラッグのみ新規フリックに
   割り当てる** (長押し / ドラッグのタイミング判定は変わる。Codex P3-1)。
5. **発火はリング/ピッカー専用の「適用層」を新設する** (Codex P1-2)。既存 `KeyAction` /
   `trigger()` の再利用は **既に一発アクションとして存在するものに限る** (本棚追加 / キャプチャ /
   回転 / レーティング set `RatingItem1..5` / 列数 set `GridColumnCount1..10` 等)。ポストフィルタ・
   AI モデル・★固定 (snapshot lock) は **set-specific アクションが存在せず**、副作用 (キャッシュ破棄 /
   undo / スコープ解決) が `ui_fullscreen` / App 側に埋まっているため、`RingActionId` / `PickerCommand`
   と App 側 apply API を別に定義する (§7)。

## 4. トリガと入力判定

### 4.1 マウス右ボタン (既存 `fs_secondary_press_start` を流用)

[ui_fullscreen.rs](../src/ui_fullscreen.rs) には既にフルスクリーンで
`短タップ / 長押し / 移動(20px)` を判定する仕組みがある。これを拡張する。

| 操作 | フルスクリーン | グリッド |
|---|---|---|
| 短タップ / 静止リリース (移動<20px / 400ms 未満) | 閉じる (従来の Esc/Enter 相当) | コンテキストメニュー (従来) |
| 静止長押し (移動<20px / 400ms 以上) | 右フリック ON: リング表示、中央で離すと取消 / 右フリック OFF: コンテキストメニュー | 右フリック ON: リング表示、中央で離すと取消 / 右フリック OFF: コンテキストメニュー |
| **ドラッグ (≥20px) → 離す** | **フリック発動** (ドラッグ方向のスロット) | **フリック発動** |
| ドラッグ後に中央へ戻して離す | **リングを取消** | **リングを取消** |

- 現状フルスクリーンの「移動≥20px = キャンセル」分岐を「リング (フリック) 起動」に振り替える。
- グリッドは右クリック = Shell / コンテキストメニューなので、同等の press 開始追跡を追加し、
  ドラッグ時はメニューを出さずフリックへ。`secondary_clicked()` の即時メニューは、ドラッグ発生時に
  抑止する (Codex P1-3: 右ドラッグ後にメニューが出る退行の防止)。
- フルスクリーンは従来どおり短い右クリックを閉じる操作として残す。短押し close と
  コンテキストメニューは同じジェスチャでは共存できないため、コンテキストメニューは
  右フリック OFF 時の長押しに限定する。
- ためらい (静止) で 8 方向ガイドを表示。素早いドラッグはガイドを待たず即発動。
- リング表示中は開始位置を中心に描画する。画面端で見切れてもクランプしない。
- 中央ニュートラル領域 (`MOUSE_FLICK_NEUTRAL_RADIUS_PX`) で離した場合はアクションを発動せず取消。
  ただし、開始位置から 20px 未満かつ 400ms 未満の静止リリースは、フルスクリーンでは close、
  グリッドではコンテキストメニューを開く。

**ジェスチャ状態機械 (Codex P1-3、右ボタン共通)**: `Pending → RingArmed / Cancelled`
を定義し、優先順位を明文化する:
- press で `Pending`。20px 超え (400ms 未満) → `RingArmed` (ガイド表示・方向追従)。
- 400ms 静止保持 → 右フリック ON ならリング表示、OFF ならコンテキストメニュー。
- `release` は現在状態に応じて **一度だけ** 発火: `RingArmed`→方向スロット (中央なら取消) /
  `Pending` / 短い静止リリース→フルスクリーン close またはグリッドメニュー / `Cancelled`→無処理。
- **時間遷移の再描画予約 (Codex P2-e)**: FS は長押し用に `request_repaint_after` 済みだが
  ([ui_fullscreen.rs:7589](../src/ui_fullscreen.rs:7589))、**グリッドには無い**。共通 gesture manager に
  `next_timer_deadline()` を持たせ、`Pending` / `RingArmed` 中は入力が来なくても guide(150-200ms) /
  menu(400ms) まで `ctx.request_repaint_after()` する。

### 4.2 ゲームパッド X ボタン (`PadButton::West`、既存 `North`/Y の作法を踏襲)

`North`(Y) は既に「押しっぱなし=修飾 / 方向を使わず離したら単体機能」を実装している
([gamepad_input.rs](../src/app/gamepad_input.rs) の `North` 系)。`West`(X) を同型にする。

| 操作 | 動作 |
|---|---|
| X を方向入力なしで離す | **ピッカーパネルを開く** |
| X + 方向 (十字 / 左スティック) → 離す | **リング発動** (方向のスロット) |

- 十字 / 左スティックとも **8 方向**。十字は上下左右の同時押しを斜めとして扱い、
  左スティックは軽く触れた程度を無方向扱いにし、一定以上倒したときだけ角度しきい値で
  8 方向へ丸める。セクタ境界付近では前回選択方向に少し粘りを持たせ、境界ジッタで
  隣の方向へちらつかないようにする。
- 判定: X 押下中に方向入力があったか否かで「パネル」と「リング」を分岐 (North/Y と同じ)。

**X 状態機械 (Codex P1-1、必須)**: `North`/Y を踏襲しつつ `West`/X 専用に持つ:
- `x_held` / `x_ring_used` / `selected_ring_dir8` を追加。現状 `West` は press で即
  `handle_gamepad_x()` ([gamepad_input.rs:439](../src/app/gamepad_input.rs:439)) が走るので、これを
  「X 保持中は方向入力を通常ナビより**前段で消費**し、`handle_gamepad_x` は X-release 時に方向未使用
  なら発火」に作り替える。
- X 保持中の D-pad / 左スティックは、グリッド移動・ページ送り・動画シークへ**流さない** (現状は流れて
  先に発火してしまう)。
- X リング発動後、および X ピッカー / Start お気に入り一覧 / Select 動画マーカー一覧を閉じた後は、
  D-pad / 左スティックが一度ニュートラルになるまで通常操作へ戻さない。release 直後の十字 repeat や
  スティック step がページ送り・シークとして漏れる退行を防ぐ。
- X リングを B で取消した場合も同じ neutral gate を必ず立て、保持中の方向入力が背面の
  グリッド移動 / ページ送り / 動画シークへ漏れないようにする。
- `suppress_pending_actions()` / `gamepad_needs_repaint()` / ダイアログ・IME 中の抑止を `West` にも
  対応させる。
- **release 時の状態クリア順序 (Codex P2-f)**: 現状 `ButtonReleased` は `set_button_down(false)` 後に
  release を dispatch する ([gamepad_input.rs:203](../src/app/gamepad_input.rs:203))。`x_ring_used` /
  `selected_ring_dir8` を release で先に消すと発火判定が失われるため、`finish_west_release() ->
  XReleaseOutcome` のように **dispatch 後に状態をクリア**する (North/Y と同型にするだけでは曖昧)。
- **メタデータ表示 (旧 X 単体) は廃止し、リング 1 枠 (既定: 画像 FS の ↗) へ移設** (機能は残す)。
  リリース済み挙動の変更なので **初回トーストで案内** + マニュアル (gamepad.html) 更新で明示する
  (Codex P2-1。データ migration は不要)。

### 4.3 既存固定入力との非衝突

- マウス中ボタン (ホイール押し込み+ドラッグ=ズーム) は不変。リングは右ボタンに置く。
- パッドの既存固定割り当て (`Start`=お気に入り巡回 / `Y`=ツリー開閉・S 相当 /
  `LB`・`RB`=前後フォルダ / `LT`・`RT`=ズーム・スクロール・シーク) とは**重複させない**。
  → リングのデフォルトにこれらの機能は入れない。

### 4.4 リング描画・発動の挙動 (マウス・パッド共通レンダラ)

8 扇形 + 中央不感帯のオーバーレイ。マウス・パッドで同一レンダラを使い、表示位置とタイミングだけ
変える。

- **表示位置**: マウス = 右ドラッグ開始点 (カーソル位置)、パッド = 画面中央。
- **表示タイミング (マーキングメニュー方式)**:
  - マウス: 素早く払えばリングを描かず方向で即発動。約 **150–200ms** ためらうとガイドとして
    フェードイン (待ち時間は実機で調整)。
  - パッド: **X 押下で即時表示** (カーソルが無いので最初から見せる)。十字 / スティックで扇形を
    ハイライト → 離して発動。
- **構造**: 中央に不感帯 (dead-zone) + 周囲 8 扇形。**選択中の扇形をアクセント色で強調**、
  空きスロットは淡く表示 (後から割り当て可)。
- **ラベル**: **テキストのみ** (絵文字・環境依存記号は使わない。glyph 方針 = `check_ui_glyphs.py`)。
  方向は配置で表現。アイコンは将来検討 (既存ツールバー資産の流用余地あり)。
- **発動**: 離した瞬間に確定 (一発もの。ピッカーパネルのようなライブ反映はしない)。
- **取消**: 中央 (不感帯) で離す / B (パッド) / Esc。取消後は D-pad / 左スティックがニュートラルへ
  戻るまで通常操作へ流さない。**パッドで方向を選ばず離した場合は §4.2 どおりピッカーパネルを開く**
  (取消ではない)。
- グリッド / 画像 / 動画で同一レンダラ。スロットのラベルは各コンテキストの割り当て (§5) を表示。

## 5. リング / フリックのデフォルト割り当て (共通・カスタマイズ可)

規約: **↙ = 本棚に追加 / ↓ = 代表サムネにピン留め** を全コンテキスト共通。
画像・動画の **↖ = キャプチャ保存** も揃える。グリッドの ↑ はパッドで戻りやすい
ように親フォルダへ割り当て、画像 ↑ はスライドショー、動画 ↑ はループにする。
対称ペア (回転 L/R・履歴 戻る/進む) は ←/→ に左右対称で配置 (左=戻る/左回転、右=進む/右回転)。
空きスロットは許容。

| 方向 | グリッド | 画像フルスクリーン | 動画フルスクリーン |
|---|---|---|---|
| ↑ | 親フォルダへ | スライドショー | ループ |
| ↓ | 代表サムネにピン留め | 代表サムネにピン留め | 代表フレームにピン留め |
| ← | フォルダ履歴 戻る | 回転 L | ミュート |
| → | フォルダ履歴 進む | 回転 R | タイルモード |
| ↖ | 表示↔詳細 | キャプチャ保存 | キャプチャ保存 |
| ↗ | ★固定 | メタデータ表示 | 外部プレイヤーで開く |
| ↙ | 本棚に追加 | 本棚に追加 | 本棚に追加 (フレーム) |
| ↘ | チェック ON/OFF | (空き) | ブックマーク追加 |

- ウィンドウ/全画面切替 (F11) と別ウィンドウ (F12) はデフォルトから除外
  (パッドは基本フルスクリーン/最大化のため)。画像・動画フルスクリーンの
  割り当て可能リストには残し、希望者がカスタマイズで入れられる。
- グリッドの割り当て可能リストには「メインウィンドウを閉じる」を含める。
  [×] と同じでトレイ常駐設定に従う。別ウィンドウの生存規則により復帰結果が表示モードごとに
  変わるため、画像 / 動画フルスクリーンの候補には含めない。
- 「アプリを終了する」はグリッド / 画像フルスクリーン / 動画フルスクリーンで割り当て可能とし、
  常駐設定を迂回して通常の保存・終了経路へ進む。次回起動時はフルスクリーンを復元せず一覧へ戻る。
  画像 / 動画の「フルスクリーンを閉じる」とは stable ID も実行経路も分け、既定スロットには入れない。
- グリッドには「先頭の項目へ移動 (Home)」「末尾の項目へ移動 (End)」と、
  「一覧の先頭へスクロール」「一覧の末尾へスクロール」を別アクションとして割り当て可能にする。
  前者はキーボードの Home / End と同じ表示順で選択を移動し、チェック済み項目は維持する。
  後者は選択・チェックを動かさず、描画時に確定した現在の最大 offset へ pending intent を適用する。
  サムネイル / 詳細表示の両方でレイアウト確定時に既存 intent だけを消費し、同フレーム後半の
  リング / ジェスチャから追加された intent は次フレームまで保持する。既定スロットには入れない。
- パノラマ / 360 は共通リングから除外 (視点操作にスティック対応が別途必要で、パッド単独だと
  中途半端になるため)。マウス側の既存 UI で扱う。

### 5.1 割り当ての設定画面

- **場所**: 環境設定に新ページ `PreferencesPage::RingShortcut` (ラベル「リングショートカット」) を追加
  ([preferences.rs](../src/ui_dialogs/preferences.rs) の enum + ツリー + `page_*` パターンに沿う)。
- **有効/無効トグルはマウス右ドラッグのみ持つ**: 「マウス右ドラッグでフリックを有効化」=
  **既定 OFF** (マウスは上積み機能で、右ドラッグ誤操作による状態変更を避けるため opt-in)。
  ゲームパッド X リング/ピッカーは常時有効とし、方向ごとの無効化はスロット「なし」で行う。
  グリッドの移動なし右クリックメニューはトグルに関係なく常に有効。フルスクリーンの
  右クリックメニューは短押し close と衝突するため、右フリック OFF 時の長押しで表示する。
- 個別スロットを「なし」にすればその方向だけ無効化できる (全スロット「なし」でそのリングは実質無効)。
- **本体**: `グリッド / 画像フルスクリーン / 動画フルスクリーン` を見出し or タブで分け、各 8 スロットを
  **8 行リスト** (`方向ラベル ↑上 / ↗右上 / →右 … ＋ プルダウン`) で編集。右側に**リングプレビュー**
  (リング描画レンダラの縮小版) を置き、現在の配置を一目で確認できるようにする。
- **プルダウン**: そのコンテキストで有効な一発ものアクション (§7) ＋「なし」だけに絞る
  (例: 動画タブに回転は出さない)。マウス・パッド**共通の 1 セット**。
- **コンテキストごとに「既定に戻す」**ボタン。
- 固定ボタン (A/B/X/Y、Select/Start、LB/RB、LT/RT) 単体のカスタマイズは v2.2.0 では無効。ゲームパッド設定タブでは既定動作の確認だけを行い、クリック編集は提供しない。
- X+方向リングは固定ボタンとは別の 8 方向スロットとして扱い、現行のゲームパッドカスタマイズ対象はこの 8 方向だけに限定する。X 単体は常にピッカーパネルを開く。

### 5.2 マウスボタン追加バインド / ホイール操作は後続タスク

ホイール調査の結論: 素のホイールはコンテキストで **11〜30 分岐** (グリッド行送り / 画像ページ送り /
縦横連結スクロール / 分析ズーム / 360 FOV / 編集ズーム / 動画タイル)、Ctrl+ホイールはズーム・列数で
**全環境使用中**。これらと中ボタンドラッグ (ズーム) は**コア操作なので固定**。

2026-06-17 の判断として、**Shift / Alt + ホイールのカスタマイズは v1.7.0 範囲から延期**する。グリッド /
画像フルスクリーン / 動画フルスクリーンでホイールの意味とイベント経路が異なり、特に native video は
通常ホイールのファイル移動、Ctrl+ホイールの動画タイル、overlay の consumed wheel、modifier 転送を同時に
整理する必要があるため、リング / マウスボタン実装と同時に入れない。既存の素のホイール / Ctrl+ホイール /
中ボタンドラッグ挙動は維持する。

**今回の範囲 (カスタマイズ可)**:
- **戻る/進む/ホイールクリック** (`Extra1` / `Extra2`、`Browser_Back/Forward`、中ボタン短クリック)。
  物理戻る / 進む / ホイールクリックを固定ペアにせず、グリッド / 画像フルスクリーン / 動画フルスクリーンごとの
  `MouseButtonProfile { back, forward, middle }` として保存する。候補は基本的にリングショートカットと同じだが、
  画像 / 動画フルスクリーンのマウスボタンでは `C:\`〜`Z:\`、お気に入り、読書履歴、★一覧などの場所移動系を
  候補外にする。グリッドでは親フォルダ移動、画像ではスライドショー、動画ではミュートなども割り当てられる。
  ウィンドウ最小化はグリッド / 画像フルスクリーン / 動画フルスクリーンの共通候補とし、メインウィンドウ、
  detached viewer、native video の現在操作中の mIV ウィンドウを最小化する。動画再生は最小化後も継続する。
  既定はフォルダ履歴 戻る/進む (ブラウザ / Explorer 慣習)、ホイールクリックは未割り当て。移行は §5.3。
  `FolderHistoryBack/Forward` は画像 FS / native video からも `AddressBarNav::HistoryBack/Forward`
  へ流し、履歴先のグリッドを開く挙動に統一する。
- 発火は §7 の apply 層を流用。設定 UI はリングショートカットとは別の
  `PreferencesPage::MouseButtons` (ラベル「マウスボタン」) に置く。戻る/進む/ホイールクリックは
  context ごとに 3 行表示する。ホイールクリックは 500ms 以内かつドラッグしきい値以下の短クリックだけ
  発火し、画像フルスクリーンの中ボタンドラッグズームはしきい値を超えた時点で従来どおり優先する。

**見送り**:
- **Shift / Alt + ホイール**: 将来はグリッド / 画像フルスクリーン / 動画フルスクリーンを別々に設計し、
  modifier wheel を素のスクロールやページ送りとして使う既存ユーザー、編集パネル / スクロールパネル、
  native video overlay、動画タイルの Ctrl+ホイールと合わせて再検証する。設定フィールド
  `shift_wheel_pair` / `alt_wheel_pair` は互換読み込み用に残すが、現行 UI / 入力経路からは参照しない。
- **横ホイール / チルト (delta.x)**: 現状未処理。**検証用ハードが無い**ため対象外。加えて Windows の
  マウス/ドライバは tilt と Shift+ホイールを相互変換することがある。実機検証が可能になるまで保留。
- **中ボタン Z ズーム**: Z の `KeyHold` 相当を完全再現するのではなく、
  `RingActionId::ImageZoomMode` を単発トグルとして追加する。押下中の照準表示はスキップし、
  現在のカーソル位置でズーム状態へ入り、ズーム状態中の中ボタン上下ドラッグで倍率を変更する。

### 5.3 戻る/進む 既定変更の移行ダイアログ (アップグレード時のみ)

既定を Ctrl+↑↓ → Alt+←/→ に変えるのはリリース済み挙動の変更なので、**アップグレードユーザーにだけ
初回起動時の選択ダイアログ**を出す。

⚠ **判定は `db_loaded` / `last_seen_version` では成立しない (Codex 第3回 P1)**:
- `Settings::load` は返却前に `last_seen_version` を current に更新して保存する
  ([settings.rs:3874](../src/settings.rs:3874)) ため、呼び出し側で旧値を見て一度だけ prompt できない。
- `db_loaded` (db: Some) は CleanInstall でも true になり「既存ユーザー」を表さない。
- 旧 DB で `last_seen_version == None` のアップグレードもある。

→ **`BootSource` ([settings_db.rs:2184](../src/settings_db.rs:2184)) と `previous_last_seen_version` を含む
load meta を `Settings::load` から返す** (current で上書きする前に旧値を保持)。判定:
- `CleanInstall` → **新規**: 既定 = Alt+←/→ を無言で設定、`prompt_done = true`、ダイアログ無し。
- `LoadedExistingDb` / `MigratedFromJson` / `RestoredFromDbBackup` → **アップグレード**
  (`last_seen_version == None` でも): 当面は従来の Ctrl+↑↓ を維持しつつ **一度だけ選択ダイアログ**。
- `FailedFallbackDefault` → **戻る/進む移行 prompt は出さず、保存もしない**
  (SAVE_SUPPRESSED を尊重)。代わりに設定復元または終了の保護モーダルを表示する。実効動作は
  **安全側で旧 Ctrl ナビを session-only** にする (新既定 Alt 履歴にしない)。通常の移行 prompt
  表示中にクラッシュ/終了した場合は、保存前なら**次回再表示**でよい (Codex 第4回 P3)。

追加フィールド: `mouse_buttons_grid` / `mouse_buttons_image` / `mouse_buttons_video`
(`MouseButtonProfile { back, forward, middle }`、serde default = 戻る/進むはフォルダ履歴、middle は未割り当て)、旧 `mouse_back_forward_action` (migration 用)、
`mouse_nav_prompt_done: bool` (default false)。ダイアログ選択肢「標準 (フォルダ履歴 戻る/進む =
ブラウザと同じ)」/「従来どおり (ツリー順 前/次フォルダ)」。選択 or 閉じる →
3 context の profile を一括設定し、旧 `mouse_back_forward_action=None` + `prompt_done=true` を保存する。
文言で「後で設定メニュー『操作カスタマイズ…』でも変更できる」を伝える。ダイアログはボタンのみで IME 配慮不要。

## 6. パッド専用ピッカーパネル (X 単体)

多状態・「特定値を選びたい」操作をまとめる。パッドで操作しやすい専用パネルとして自前で作る
(既存 egui コンテキストメニューを dpad 化しない＝見た目と操作を device に合わせ、混乱を避ける)。

### 6.1 全体レイアウト

- 画面中央のオーバーレイ。背景は半透明で、**変更は背後の画像 / 動画へライブ反映**する。
- **最上部にタイトル行** (コンテキスト識別): `グリッド設定` / `画像設定` / `動画設定`。
- その下にパラメータ行の縦リスト。各行 = `項目名 ／ 現在値 ／ ←→ ヒント`。
- コンテキスト別に項目セットを差し替える (同じパネル枠を再利用):

| コンテキスト | パネル項目 |
|---|---|
| グリッド | 列数 / ソート順 / サムネ縦横比 / レーティング (★1-5 / 解除) |
| 画像フルスクリーン | 見開きモード / 連結方式 / 読み方向 / フィット / レーティング / ポストフィルタ / アップスケールモデル |
| 動画フルスクリーン | 音量 / 再生速度 / 連続再生 (3 状態: OFF / 連続 / 連続+ループ) |

### 6.2 入力割り当て

- 十字 / 左スティック **上下** = 行移動、**左右** = 値変更。
- **確定モデル**: 編集中は全項目ライブプレビューし、トップレベルの **X / B はどちらも commit して閉じる**。
  一般的なゲーム UI の「B で閉じても反映」に寄せる。ドリルイン内では **A = その項目を選択
  (preview) してパネルへ戻る**、B は変更せずパネルへ戻る。
- **ピッカーは gamepad dispatch の最上位で modal 扱い**にし、A/B/十字/スティック等の全入力を消費する
  (通常の「開く / 戻る / 移動 / シーク」へ流さない。Codex P2-4)。
- **X-release でピッカーを開いた直後の同 release が「X で閉じる」に化けない**よう、X によるクローズは
  「次の X press/release から有効」とデバウンスする (Codex P2-4)。

### 6.3 値の見せ方 (2 種類)

- **インライン ←→** (選択肢が少ない / 連続値): その場で左右に変更し即ライブ反映。
  - レーティング (`★★★☆☆`、5 + 解除)、列数 (±1)、サムネ縦横比、見開きモード、連結方式、
    読み方向、フィット、音量、再生速度、連続再生 (3 状態、`VideoContinuousMode::label()` 準拠)。
- **ドリルイン専用画面** (選択肢が多い / グループ分けがあるもの = ポストフィルタ):
  行で A を押すと **ダイアログ全体がその項目専用画面に切り替わる**。
  - 専用画面のナビ: **上下 = グループ、左右 = グループ内の項目**。
  - 選択は即ライブ反映、B で元のパネルへ戻る。
  - グループ構成は §6.6 (`PostFilter::ALL` / `display_label()` 準拠)。
  - **アップスケールモデルはインライン ←→** (`ModelKind::upscale_models()` で 5 件のためドリルイン不要。
    用途ラベル: 写真・CG / イラスト・アニメ / 漫画 / 写真(質感保持) / 高速汎用)。

### 6.4 動画の単位 (HUD と統一)

- **音量**: 下部シークバー (HUD) の dB フェーダーと同一単位。←→ の刻みはキー操作 (Shift+↑↓) と
  同じ (-∞ / -60 / -40 / -20 / -10 / -5 / 0 / +6 / +12 / +18 dB を 1/4 幅ずつ)。
- **再生速度**: HUD の速度コントロールと同一単位・刻み。

### 6.5 補足

- 列数・サムネ縦横比のように段数が多いものは ←→ の ±1 と現在値表示で操作する
  (大きく動かすのは何度か ←→)。
- 連結方式・見開き・フィットは「今どの状態か」が操作直後に分かりにくいので、パネルで明示・選択
  する (本パネルを設けた主目的の 1 つ)。

### 6.6 ポストフィルタのグループ構成 (ドリルイン: 上下=グループ / 左右=グループ内)

`PostFilter` (約 40 種 = `PostFilter::ALL` 全件) を以下のグループに分ける。順序・ラベルの正は
`PostFilter::ALL` と `PostFilter::display_label()` ([adjustment.rs](../src/adjustment.rs))。表示名は必ず
`display_label()` を使い、**グループ定義が `ALL` 全件を漏れなく被覆する coverage test** を持たせる
(手書きのずれ防止、Codex P3-1)。enum の区分コメントを踏襲しつつ、
カラーグレーディングだけパッドの左右移動が長くなりすぎないよう「モノ・トーン」と「シネマ・フィルム」に
2 分割している。

| # | グループ | グループ内の項目 (左右で移動) |
|---|---|---|
| 1 | 基本 | 標準(補間あり) / ニアレスト(補間なし) |
| 2 | CRT | CRT シンプル / CRT フル / CRT アーケード |
| 3 | レトロ機 (減色) | 1bit ディザ / GameBoy / PC-98 / ゲームギア / ファミコン / メガドライブ / MSX2+ / スーパーファミコン |
| 4 | CRT × レトロ機 (複合) | ファミコン / PC-98 / MSX2+ / メガドライブ / スーパーファミコン |
| 5 | カラーグレーディング (モノ・トーン) | セピア / モノクロ ニュートラル・冷調・暖調 / 暖色調 / 寒色調 |
| 6 | カラーグレーディング (シネマ・フィルム) | Teal&Orange / Kodak Portra / Fuji Velvia / ブリーチバイパス / クロスプロセス / ビンテージ |
| 7 | アナログフィルム | フィルムグレイン / ビネット / ライトリーク / ソフトフォーカス |
| 8 | 描画風 | ハーフトーン / オイルペイント / スケッチ |
| 9 | 漫画 疑似カラー | 4色刷り風 / 肌色 |
| 10 | 実用 | シャープ化 |

- グループ間移動 (上下) は 10 段、最大グループ (減色=8) の左右移動も許容範囲。
- 「標準」(= フィルタ無し) はグループ 1 の先頭に常駐させ、どのグループからでも 1 へ戻れば解除できる。

### 6.7 ライブ反映の編集セッション (Codex P2-5)

ポストフィルタ / AI モデル変更はキャッシュ破棄・再描画を伴い、音量 / 速度は設定保存を伴う。D-pad
リピートで毎回 undo や `settings.save()` が走ると応答性と履歴が壊れるので、編集セッションを設ける:
- **preview / commit / cancel** を分離。パネル表示中はプレビュー反映のみ、確定 (A / パネル閉じ) で commit。
- **保存タイミング**: 音量 / 速度はパネルを閉じる時か debounce 保存 (毎フレーム保存しない)。
- **undo 粒度**: 1 回のピッカー操作 = 1 undo に集約 (postfilter/AI を D-pad で連打しても履歴は 1 つ)。
- **重い処理**: キャッシュ破棄・再 upscale を D-pad リピートのたびに走らせない (確定 or debounce)。
  UI スレッドで同期実行しない ([ui-responsiveness.md](ui-responsiveness.md))。

項目ごとに preview 方法 / commit 時処理 / undo 粒度を分ける (Codex P2-b):

| 項目 | preview 方法 | commit 時処理 | undo 粒度 |
|---|---|---|---|
| ポストフィルタ | debounced preview (約150-200ms) | 確定描画 | 1 セッション 1 undo |
| AI モデル | commit-only または generation-token 付き preview | 再 upscale | 1 セッション 1 undo |
| 列数 / サムネ比率 / 見開き / 連結 / 読み方向 / フィット | 即時 preview (軽い) | 確定 | 1 undo |
| レーティング | 即時 preview | meta undo stack に commit | 1 undo |
| 音量 / 速度 / 連続再生 | 即時反映 (音 / 挙動は即) | close 時 or debounce で設定保存 | 設定 (undo 対象外) |

## 7. 割り当て可能アクションと適用層 (Codex P1-2 / P2-3)

リング/ピッカーの発火は **`KeyAction::trigger()` をそのまま呼べる前提に立たない**。ポストフィルタ・
AI モデル・★固定・代表サムネ pin は set-specific アクションが無く、副作用は `ui_fullscreen` / App 側に
埋まっている。そこで:

- **`RingActionId`** (一発もの) と **`PickerCommand`** (多状態の値適用) を新設し、それを実行する App 側
  apply API を用意する。例: `book_add()` / `capture_save()` / `rotate(±90)` / `toggle_metadata()` /
  `toggle_detached()` / `set_post_filter(scope, PostFilter)` / `set_upscale_model(ModelKind)` /
  `set_grid_columns(n)` / `set_rating(target, n)` / `set_video_speed(choice)` / `toggle_star_lock()` /
  `pin_repr_thumb()`。
- 既存 `KeyAction` の再利用は **既に一発アクションとして存在するものに限定** (本棚追加 / 回転 /
  キャプチャ / `RatingItem1..5` / `GridColumnCount1..10` 等)。`ALL_ACTIONS` / `keymap.ini.default` に
  **リング専用 action を不用意に混ぜない** (Codex P2-3)。

各 action は次の属性を持つ表で管理する (自然言語の羅列にしない、Codex P2-3):

| 属性 | 意味 |
|---|---|
| `id` | `RingActionId` / `PickerCommand` の識別子 |
| 有効 context | Grid / ImageFS / VideoFS のどれで有効か |
| handler | 呼び出す App 側 apply 関数 |
| 無効時 | その context で無効なときの挙動 (グレーアウト / トースト) |
| 同期 I/O | UI スレッドで重い処理を伴うか (伴うなら §6.7 の debounce / worker 経由) |

### リング (一発もの) の候補

- **全 context 共通**: 本棚に追加 / 代表サムネ (フレーム) にピン留め / お気に入り巡回 /
  お気に入り 1〜20 / `C:\`〜`Z:\` / 場所▼の固定項目 (ドライブ一覧 / 読書履歴 / ★1〜★5 /
  本棚 / デスクトップ / ピクチャ / ダウンロード)。
- **FS 共通**: ウィンドウ/全画面切替 / 別ウィンドウ ON/OFF。
- **グリッド**: 表示↔詳細 / ★固定 / チェック ON/OFF / 全選択 / フォルダ履歴 戻る・進む / 親フォルダへ。
- **画像 FS**: 回転 R / 回転 L / キャプチャ保存 / メタデータ表示 / スライドショー / ピクセルグリッド /
  背景色サイクル / 比較ピン。(**消しゴム・隠蔽・分析のモード起動は Phase 1 では外す**。モード遷移で
  起動後のパッド操作・終了導線・右クリックジェスチャ抑止が絡むため、モード内パッド操作を設計してから
  追加する。Codex P2-i)
- **動画 FS**: キャプチャ保存 / ミュート / ループ / ブックマーク追加 / タイルモード / 外部プレイヤー。
- 注: **キャプチャはグリッドに無い** (ImageFS / VideoFS のみ。Codex P2-3)。**レーティングは item / container の
  どちらを対象にするか context ごとに決める** (`RatingItem*` / `RatingContainer*` が両方存在)。
  **★固定 (snapshot lock) は KeyAction ではない**ので apply API を新規に用意する (代表サムネ pin は
  `GridPin` / `FsPin` / `VideoPin` が既存なので流用可)。
  **レーティングは行を「項目レーティング」「フォルダ(コンテナ)レーティング」に分ける** (Codex P2-g)。
  対象解決: 項目 = グリッドはチェック優先 (複数可)・無ければカーソル選択 / FS は表示中アイテム。
  フォルダ = 現在のコンテナ。複数選択時はチェック項目すべてに適用。

### ピッカー (多状態) の候補

レーティング / ポストフィルタ / アップスケールモデル / 列数 / ソート順 / サムネ比率 / 見開き /
連結 / 読み方向 / フィット / 音量 / 再生速度 → §6 のピッカーパネルで `PickerCommand` 経由。

### リング/ピッカーに載せない (除外基準)

- **ダイアログ / テキスト入力が続くもの**: タグ付け・タグビュー・エクスポート (Ctrl+E) など。
- **連続値の微調整 / nav 依存**: 細かいシーク・パン・ズーム微調整・パノラマ視点操作。

### 7.1 割り当て可能な機能カタログ

カスタマイズ入力に割り当てられる機能の一覧。`★` = set-specific の apply API を**新規に用意**するもの
(既存 KeyAction では実現不可)。それ以外は既存 `KeyAction` 流用 (括弧内が enum 名)。
適入力: **環**=リング単発 / **選**=ピッカー多状態 / **輪**=ホイールペア(±) / **戻**=戻る進むボタン。
context は **Grid / ImageFS / VideoFS** に統一 (`FS` = ImageFS + VideoFS の略。元 `KeyContext` は
FsCommon/FsImage/FsVideo)。native video の Ctrl 系ナビは KeyAction 経由でなく直接ハンドラである点に注意
(Codex 第3回 P2)。

**ナビゲーション**

| 機能 | KeyAction / apply | context | 適入力 |
|---|---|---|---|
| フォルダ履歴 戻る/進む | `RingActionId::GridHistoryBack/Forward` → `AddressBarNav::HistoryBack/Forward` | Grid / ImageFS / VideoFS | 戻 / 輪 / 環 |
| ツリー順 前/次フォルダ | `FsCtrlNavPrev` / `FsCtrlNavNext` (+ grid / native video は直接ハンドラ) | Grid / ImageFS / VideoFS | 戻 / 輪 / 環 |
| 兄弟フォルダ 前/次 | `FsSiblingPrev` / `FsSiblingNext` (+ grid / native video は直接ハンドラ) | Grid / ImageFS / VideoFS | 輪 / 環 |
| 親フォルダへ | Backspace 直接 | Grid | 戻 / 環 |
| ページジャンプ 前/次 (10%) | `FsFixedJumpPrev` / `FsFixedJumpNext` | ImageFS | 輪 |
| 同一一覧 先頭/末尾 | `RingActionId::ImageHome/End` → Home/End 相当 | ImageFS | 戻 / 環 |
| お気に入り巡回 | Start 直接ハンドラ | Grid / ImageFS / VideoFS | 環 |
| お気に入り 1〜20 を開く | `RingActionId::OpenFavorite1..20` → `AddressBarNav::Direct` | Grid / ImageFS / VideoFS | 戻 / 環 |
| `C:\`〜`Z:\` を開く | `RingActionId::OpenDriveC..Z` → `AddressBarNav::Direct` | Grid / ImageFS / VideoFS | 戻 / 環 |
| 場所▼の固定項目 | `RingActionId::OpenLocation*` → `AddressBarNav` / `enter_rating_view` | Grid / ImageFS / VideoFS | 戻 / 環 |
| ZIP/PDF/対応アーカイブをページ/一覧で開く | `GridOpenSelectedAsPage/List` → `open_grid_container_with_mode` | Grid | 環 |

**本・整理 (リング単発)**

| 機能 | KeyAction / apply | context | 適入力 |
|---|---|---|---|
| 本棚に追加 | `GridAddToActiveBook` / `FsAddToActiveBook` / `VideoAddToActiveBook` | Grid / ImageFS / VideoFS | 環 |
| 代表サムネ/フレームにピン留め | `GridPin` / `FsPin` / `VideoPin` | Grid / ImageFS / VideoFS | 環 |
| ★固定 トグル | ★新規 apply (KeyAction 無し) | Grid | 環 |
| チェック ON/OFF | `GridToggleCheck` / `FsSpaceCheck` | Grid / ImageFS | 環 |
| 全選択 | `GridSelectAll` | Grid | 環 |
| 比較ピン | `GridComparePin` / `FsCompareToggle` | Grid / ImageFS | 環 |

**表示・トグル (リング単発)**

| 機能 | KeyAction / apply | context | 適入力 |
|---|---|---|---|
| 回転 R / L | `GridRotateCw/Ccw` / `FsRotateCw/Ccw` | Grid / ImageFS | 環 |
| メタデータ表示 | `FsToggleMetadata` (FsImage。動画には対応する固定右パネルがないため画像専用) | ImageFS | 環 |
| ウィンドウ/全画面切替 | `RingActionId::ToggleWindowMode` → F11 相当 | ImageFS / VideoFS | 環 |
| 別ウィンドウ ON/OFF | `ToggleDetachedViewerMode` | ImageFS / VideoFS | 環 |
| 表示↔詳細 | `GridToggleDetailsView` | Grid | 環 |
| スライドショー | `FsSlideshow` | ImageFS | 環 |
| ピクセルグリッド | `FsPixelGrid` | ImageFS | 環 |
| 背景色サイクル | `FsBgCycle` | ImageFS | 環 |
| キャプチャ保存 | `FsCapture` / `VideoCapture` | ImageFS / VideoFS | 環 |

**多状態 (ピッカー)**

| 機能 | KeyAction / apply | context | 適入力 |
|---|---|---|---|
| 項目レーティング | `RatingItem1..5` / `RatingItemClear` | Grid / ImageFS / VideoFS | 選 |
| フォルダレーティング | `RatingContainer1..5` / `RatingContainerClear` | Grid / ImageFS / VideoFS | 選 |
| 列数 | `GridColumnCount1..10` | Grid | 選 |
| ソート順 | ★新規 set apply | Grid | 選 |
| サムネ縦横比 | ★新規 set apply | Grid | 選 |
| 見開きモード | `FsSpreadSingle/Ltr/LtrCover/Rtl/RtlCover` | ImageFS | 選 |
| 連結方式 | `FsReadingFlowCycle` (循環) / ★set | ImageFS | 選 |
| フィット | `FsFitModeCycle` (循環) / ★set | ImageFS | 選 |
| 読み方向 (左→右 / 右→左) | `FsReadingDirectionToggle` (toggle) / ★set | ImageFS | 選 |
| ポストフィルタ | ★新規 set apply (`FsPostFilterNext/Prev/Reset` のみ存在) | ImageFS | 選 (ドリルイン) |
| AI モデル | ★新規 set apply (`FsAiModelNext/Prev/Reset` のみ存在) | ImageFS | 選 |
| 補正プリセット 1..10 | `GridAdjustSlot1..10` / `FsAdjustSlot1..10` | Grid / ImageFS | 選 (任意) |

**動画**

| 機能 | KeyAction / apply | context | 適入力 |
|---|---|---|---|
| ミュート | `VideoMute` | VideoFS | 環 |
| ループ | `VideoLoop` | VideoFS | 環 |
| ブックマーク追加 | `VideoBookmark` | VideoFS | 環 |
| タイルモード | `VideoTileMode` | VideoFS | 環 |
| 外部プレイヤー | `VideoExternalPlayer` | VideoFS | 環 |
| チャプター/マーカー 前/次 | `VideoMarkerPrev` / `VideoMarkerNext` | VideoFS | 輪 / 環 |
| 音量 (wheel ±) | `VideoVolumeUp` / `VideoVolumeDown` (既存 handler は即 `settings.save()`) | VideoFS | 輪 |
| 音量 (picker) | ★新規 set `SetVideoVolume(f64)` (即時反映・保存は close/debounce) | VideoFS | 選 (←→) |
| 再生速度 | ★新規 set apply (command) | VideoFS | 選 |
| 連続再生 (3 状態) | ★新規 set `SetVideoContinuousMode` (`ToggleContinuous` は toggle で picker と不一致) | VideoFS | 選 |

**載せない (除外)**: タグ付け (`GridTagApply`) / タグビュー (`GridTagView`) / エクスポート (`FsExport`) =
ダイアログ・テキスト/検索 UI が続く。消しゴム・隠蔽・分析・注釈モード起動 (`FsEraseMode` /
`FsConcealMode` / `FsImageAnalysis` / `FsTextMode`) = モード遷移で Phase 1 では外す (§7)。
細かいシーク・パン・ズーム微調整・パノラマ視点操作 = 連続値 / nav 依存。

**★ (新規 apply が必要) のまとめ**: ★固定トグル / ソート順 set / サムネ縦横比 set / ポストフィルタ set /
AI モデル set / (フィット・連結方式・読み方向は循環アクション流用 or set) / 再生速度 set /
音量 set `SetVideoVolume` / `SetVideoContinuousMode` (3 状態 set) / ImageFS・VideoFS の `FolderHistoryBack/Forward`。これらは `RingActionId` /
`PickerCommand` + App apply API として実装する。代表サムネ pin・本棚追加・回転・キャプチャ・レーティング
set・列数 set 等は既存 `KeyAction` を流用できる。

## 8. 永続化 (Codex P2-6)

- **schema migration は不要** (新機能・新フィールドは `serde(default)`)。ただし戻る/進む既定変更には §5.3 の
  **load-time behavioral migration** (BootSource 判定 + 一度だけ prompt) が必要 (Codex 第3回 P3)。
  `mouse_buttons_grid` / `mouse_buttons_image` / `mouse_buttons_video`、旧
  `mouse_back_forward_action` / `mouse_nav_prompt_done` も serde default。`sanitize()` は unknown / context 不一致を
  `None` へ落とし、旧 `mouse_back_forward_action` が明示済みなら 3 context の profile へ移行して旧値を
  `None` に戻す。初回選択前の旧互換 / clean install の新既定は `Settings::load` の BootSource 判定で決める。
  `mouse_back_forward_action` は `RingActionId` 同様 **文字列 ID + `Unknown` バリアント**にし、
  未知値で serde deserialize が失敗しないようにする。
- `settings.db` は `Settings` を JSON 化して `settings_kv` に保存する方式
  ([settings_db.rs](../src/settings_db.rs))。**複合テーブル化はしない** (移行罠あり)。正しくは `Settings`
  に `#[serde(default)] ring_shortcuts: RingShortcutSettings` を 1 つ追加するだけ。`serde(default)` に
  より旧 DB を新コードで開いても安全に既定が入る。
- `RingShortcutSettings` が保持する**保存対象 (全列挙、Codex 第6回 P2)**:
  - context 別 × 8 スロットの `RingActionId` (または空き)
  - 右ドラッグ mode 4 文脈 (`right_drag_grid` / `right_drag_image` / `right_drag_video` / `right_drag_edit`) と、グリッド開始セルを押下時に選択する opt-in `select_grid_item_on_right_drag_start`、4 文脈ごとの `mouse_gestures_*` (最大 4 stroke の方向列 + `RingActionId`)。旧 `mouse_flick_enabled` は互換用
  - `shift_wheel_pair` / `alt_wheel_pair` (`WheelPairActionId`) — 互換読み込み用。§5.2 のとおり現行 UI /
    入力経路からは参照しない
  - `mouse_buttons_grid` / `mouse_buttons_image` / `mouse_buttons_video`
    (`MouseButtonProfile { back, forward, middle }`) — §5.2
  - 旧 `mouse_back_forward_action` (migration 用) / `mouse_nav_prompt_done: bool` — §5.3
  - `x_picker_hint_shown: bool` (X 単体ピッカーの初回案内フラグ。§4.2)
- **`RingActionId` / `WheelPairActionId` / `mouse_back_forward_action` は `Unknown` バリアントで、
  未知値でも deserialize failure しない**。
- **未知 action id は load 時に `None` / 既定へ sanitize**。round-trip テスト、旧設定ロード、context
  不一致スロットの無効化テストを設計に含める。
- ピッカーパネルの内容は固定なので永続化対象外。

## 9. 既存コードへの影響まとめ

| 箇所 | 変更 |
|---|---|
| マウス右クリック (既存コンテキストメニュー) | グリッドは短押しメニューを維持。フルスクリーンは短押し close、右フリック OFF 時の長押しメニューへ整理 |
| `fs_secondary_press_start` (ui_fullscreen) | 「移動=キャンセル」を「フリック起動」へ振替 |
| グリッド右クリック | press 追跡を追加し、ドラッグ時はメニュー抑制→フリック |
| `PadButton::West`(X) ハンドラ | X 状態機械を新設 (§4.2)。press 即発火をやめ、保持中の方向入力を前段で消費 |
| メタデータ表示 | リング 1 枠へ移設 + 初回トースト案内 (機能は残す) |
| 新規: apply 層 | `RingActionId` / `PickerCommand` + App apply API (§7) |
| 新規 UI | リング描画オーバーレイ / パッド専用ピッカーパネル (modal・編集セッション) |
| `Settings` | `#[serde(default)] ring_shortcuts` = 8 スロット×3 context + 右ドラッグ mode 4 文脈 + マウスジェスチャ 4 文脈 + 初回案内フラグ + shift/alt wheel pair + `mouse_buttons_*` profile + 互換用 `mouse_flick_enabled` / `gamepad_ring_enabled` / 旧 `mouse_back_forward_action` / `mouse_nav_prompt_done` (§8)。ゲームパッド固定ボタン単体は既定動作固定 |

## 10. ドキュメント / マニュアル更新 (確定後)

- [keymap-spec.md](keymap-spec.md): 冒頭の「マウス・ゲームパッドは keymap 対象外」に加え、
  **ゲームパッド節の「rating/check/export/操作割り当て変更 UI は対象外」も更新** (本リングで一部可能に)。
  本リングのみ例外で専用設定から割り当て可能、と明記 (Codex P2-7)。
- [key-customization-impl-plan.md](key-customization-impl-plan.md): 「mouse/gamepad は fixed」の記述に
  例外を追記。リング専用 action は `keymap.ini.default` / `ALL_ACTIONS` に混ぜない方針も明記。
- [spec.md](spec.md) / [README.md](README.md) (索引) / 必要なら
  [architecture-overview.md](architecture-overview.md) (apply 層・新 UI の追加)。
- **戻る/進むボタンの既定変更 (Ctrl+↑↓ → Alt+←/→) は更新履歴・リリースノートに明記**し、移行ダイアログ
  (§5.3) の存在にも触れる。マニュアルのマウス操作節も更新。
- `htdocs/mimageviewer/manual/gamepad.html`: X 単体=パネル / X+方向=リング、旧「X=メタデータ」
  廃止を反映。
- `htdocs/mimageviewer/manual/settings.html` ほか: リングショートカット設定画面の説明。
- `htdocs/mimageviewer/index.html`: 新機能紹介。

## 11. 段階実装案 (Codex P2-8 で再分割)

1. **apply 層 + 設定 + 移行判定**: `RingActionId` / `PickerCommand` + App apply API (§7)、
   `Settings.ring_shortcuts` 永続化 + sanitize / round-trip テスト、設定画面 (§5.1)。
   **§5.3 の移行判定 (`Settings::load` の BootSource / previous_last_seen_version load meta、
   `mouse_buttons_*`、旧 `mouse_back_forward_action` / `mouse_nav_prompt_done`、prompt 判定テスト) もここで前倒し**
   (load 返却情報に関わるため後付けは影響大。Codex 第3回 P3)。
   ← 入力より先に副作用と保存を固める。
2. **ゲームパッド一発リング**: `West`/X 状態機械 (§4.2) + リング描画 (§4.4) + 一発 action 発火。
3. **ピッカーコア**: パッド専用パネル (§6)、modal 入力消費、編集セッション (§6.7)、ドリルイン。
   読み方向 picker (`PickerCommand::SetReadingDirection(ReadingDirection)` /
   `RingPickerRowId::ReadingDirection`) も実装済み。
4. **動画 / native 連携**: 音量・速度・連続再生など native presenter 経路の適用。
5. **マウスフリック**: 右ボタン ジェスチャ状態機械 (§4.1) → フリック発動 (FS・グリッド)。グリッド短押しメニューと、フルスクリーン短押し close / 右フリック OFF 長押しメニューの分担を保つ。
6. **マウスボタン バインド**: 戻る/進む のカスタマイズ化 + 既定 Alt+←/→ + 移行ダイアログ (§5.3)。
   Shift / Alt + ホイールのペアバインドは §5.2 のとおり後続タスクへ延期。
7. **磨き込み**: マウスのガイド表示タイムアウト、しきい値の実機調整。

### 11.1 入力退行チェックリスト (Codex P3-b)

各フェーズで確認: `X 単体 = ピッカー` / `X + D-pad` / `X + スティック斜め` / `ダイアログ・IME 中は全抑止` /
`FS 右ボタン 静止クリック・リング中央取消・フリック` / `グリッド セル / 背景の右クリック` / `中ボタンズーム不変` /
`settings の未知 id sanitize・round-trip` / `通常ナビ (移動 / シーク / ページ送り) が X 保持中に漏れない`。

## 12. 未決事項 / 次の設計対象

- **ピッカーパネルの詳細レイアウトは §6 で確定** (タイトル行 / インライン ←→ / ドリルイン専用画面 /
  HUD と統一した音量・速度単位)。
- ~~リング描画の見た目~~ → §4.4 で確定 (テキストラベル / マウス 150–200ms ガイド / パッド即表示 /
  選択扇形を強調 / 離して発動)。
- フリック発動の閾値微調整 (ガイド表示までの待ち時間。実機で調整)。
- ~~ポストフィルタ / アップスケールモデルの実グループ構成~~ → §6.6 で確定
  (ポストフィルタ=10 グループのドリルイン、アップスケール=5 件インライン)。
- ~~リング割り当ての設定画面~~ → §5.1 で確定 (8 行リスト＋リングプレビュー / 有効化トグル 2 つ・
  マウス OFF・パッド ON /
  コンテキスト別「既定に戻す」)。

**Codex 設計レビュー 第1回 + 第2回 を反映済み**: 適用層 (§7)、X / 右ボタンの状態機械 (§4.1 / §4.2)、
ピッカーの確定/取消モデル + modal + 編集セッション + 項目別 preview/commit 表 (§6.2 / §6.7)、
連続再生3状態・レーティング対象の明確化 (§6 / §7)、グリッド repaint_after・X release クリア順序
(§4.1 / §4.2)、編集モード起動を Phase 1 から除外 (§7)、settings `serde(default)` + sanitize (§8)、
ドキュメント対象拡張 (§10)、フェーズ再分割 + 入力退行チェックリスト (§11)。

**マウス右ドラッグ フリック = 既定 OFF / パッドリング = 常時有効で確定 (Codex P2-d)。**

**マウスボタン拡張は確定 (§5.2 / §5.3)**: 戻る/進む をカスタマイズ化 (既定 Alt+←/→ + アップ
グレード移行ダイアログ)。Shift / Alt + ホイールのペアバインドは、グリッド / 画像フルスクリーン /
動画フルスクリーンの経路差が大きいため後続タスクへ延期。素/Ctrl ホイール・中ドラッグは固定。

**Codex 第3回レビュー反映済み**: §5.3 移行判定を `BootSource` ベースへ (P1)、戻る/進む履歴は
ImageFS/VideoFS でも `AddressBarNav::HistoryBack/Forward` へ流す方針に整理 (P1)、§7.1 に読み方向追加・連続再生を ★set 化・context 表記
統一 (P2)、§8 を schema/behavioral migration に整理 (P3)、移行判定を Phase 1 へ前倒し (P3)。
**P1/P2/P3 すべて反映。**

**Codex 第4回レビュー反映済み**: 読み方向を §6.1 / §6.3 / §6.7 にも反映、§7.1 context を Grid / ImageFS / VideoFS に
全展開 (メタデータは ImageFS のみ)、`sanitize()` は `None` を解決しない (§8)、FailedFallback / prompt 中断時の
実効動作 (§5.3)。

**Codex 第5回レビュー反映済み**: 音量は picker (★set `SetVideoVolume`) 側に残し、延期した wheel pair 側の
`VideoVolumeUp/Down` とは分離 (§7.1 / ★まとめ)、兄弟フォルダ行に直接ハンドラ注記、旧 `mouse_back_forward_action` も `Unknown` で
未知値の serde 失敗を防ぐ (§8)、表記ゆれ掃除 (ImageFS/VideoFS 統一・履歴行・§7 候補に
読み方向)。

**Codex 第6回レビュー反映済み**: §8/§9 の保存対象を全列挙 (8 スロット×3 + マウス右ドラッグトグル + 互換用 `gamepad_ring_enabled` / shift/alt wheel pair +
`mouse_buttons_*` + 旧 `mouse_back_forward_action` / `mouse_nav_prompt_done`)、`WheelPairActionId` も `Unknown` 安全化を明記、
読み方向 picker を §11 Phase 3 の実装対象として明記（現在は実装済み）、残った略記
(`FS / 動画`・`FS/video`) を掃除。

**Codex 第7回レビュー反映済み**: §8/§9 の保存対象に `x_picker_hint_shown` (X 単体ピッカー初回案内) を追加。
**設計 doc は確定。実装の追加分へ進んでよい。**

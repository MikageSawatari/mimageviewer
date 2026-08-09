# ブリーフ: タッチ対応 Phase 1 / Step 1 — 認識器の純ロジック (`src/touch_input.rs`)

対象: v2.13.0。実装 = Codex Sol / レビュー・検収 = ClaudeCode。
正本: [docs/touch-support-plan.md](touch-support-plan.md)。**着手前に §5.1〜§5.5、§5.10、§5.11 を読むこと。**

前提: Step 0 (診断プローブ、commit `bb9574b2`) は完了・実機検証済み。**出荷ゲート §6-1 は通過**した。

---

## 0. これは何か

タッチ操作の**認識器だけ**を純ロジックとして作る。`src/touch_input.rs` 1 本、250〜400 行程度。

**このステップでは何も配線しない。** egui にも Win32 にも繋がず、`App` のフィールドも増やさない。
Step 2 (入力源相関と所有権) と Step 3 (静止画フルスクリーンへの配線) で使う土台を、
**先に単体テストできる形で確定させる**のが目的。

理由: この認識器は **egui viewport と native `WM_POINTER` の 2 つの backend から共有される**
(plan §5.9-6)。OS アダプタだけを別実装にして、認識と意味付けは 1 か所に持つ。
先に純ロジックを固めておかないと、2 つの経路で認識が食い違う。

---

## 1. Step 0 の実機ログで確定した事実 (設計の前提にすること)

`MIV_TOUCH_DEBUG` の実機ログ (タッチディスプレイ PC、2026-08-06) から:

| 事実 | 設計への影響 |
| --- | --- |
| `Touch(Start)` → `PointerMoved` → `PointerButton(Primary, pressed=true)` の並びが Start 55 / End 55 で完全一致 | plan §5.2 の相関はこの並びを前提にしてよい。**ただしこの相関自体は Step 2 の担当**で、Step 1 は触らない |
| **接点 id は単調増加し、再利用されない** (24→57、66→109) | id の等価比較で接点を追跡してよい。「小さい id が再利用される」前提のロジックを書かない |
| 同時接触は **最大 2 点**を観測 | 3 点以上は来ない前提にはしない (認識器は N 点を扱えること)。ただし**ピンチは 2 点だけを見る** |
| `Cancel` phase と `POINTER_FLAG_CANCELED` は **1 度も発生しなかった** | Cancel は**未検証のまま**。防御的に実装し、**Cancel が来たら必ず全状態を破棄する**こと。「来ないから省略」はしない |
| 動画で長押しすると OS が**指を離した後に**右ボタン down/up を**同一ミリ秒で**合成する | Step 1 では長押しに**何も割り当てない** (plan §5.13)。この合成は Step 4 の stream 所有で構造的に消える |

---

## 2. 依存の線引き (重要)

- **使ってよい**: `egui` の**値型のみ** — `Pos2` / `Rect` / `Vec2` (実体は `emath` で純粋な数値型)。
  呼び出し側と型を合わせるためで、結合にはならない。
- **使ってはいけない**: `egui::Context` / `InputState` / `Event` / `Response`、`winit`、`windows` クレート、
  `std::time::Instant::now()`、環境変数、ログ出力、`App` への参照。
- **時刻は引数で受ける**。`now_ms: u64` (単調増加ミリ秒) を呼び出し側から渡す形にする。
  `Instant::now()` を内部で呼ぶとテストが書けない。
- 座標はすべて **logical point**。DPI / UI 倍率の変換は呼び出し側の責務。

---

## 3. 作るもの

### 3.1 入力の正規化型

2 つの backend が同じものを流し込めるようにする。

```rust
pub(crate) enum TouchPhase { Start, Move, End, Cancel }

pub(crate) struct TouchSample {
    pub id: u64,
    pub pos: egui::Pos2,
    pub phase: TouchPhase,
    pub now_ms: u64,
}
```

### 3.2 タップ領域の判定 (plan §5.3)

**中央矩形案**で実装する (§5.14-7 で確定済み)。定数は以下で固定すること:

```
中央矩形の横: surface 幅の中央 32%      → 左端 34% 〜 右端 66%
中央矩形の縦: surface 高さの 15% 〜 75%  (上端 15% と下端 25% を除外)
→ 面積は全体の約 19%、ページ送り領域が約 81% 残る
```

さらに **実際にそのフレームで表示されている**上バー / 下シークバー / 左右パネルの矩形を
除外領域として受け取り、そこに入ったタップは `Excluded` にする (plan §5.3 末尾)。

```rust
pub(crate) struct TapZoneGeometry {
    pub surface: egui::Rect,
    /// そのフレームで実際に表示されているクロームの矩形。空でもよい。
    pub excluded: Vec<egui::Rect>,
}

pub(crate) enum TapZone {
    Center,
    /// 画面の左半分 / 右半分。**読み方向の解決はここでしない**。
    PageSide { left: bool },
    Excluded,
}

pub(crate) fn classify_tap(geom: &TapZoneGeometry, pos: egui::Pos2) -> TapZone
```

⚠ **RTL (読み方向) をこのモジュールで再実装しないこと。** 既存の
`fullscreen_click_nav_base_delta` ([ui_fullscreen.rs:1246](../src/ui_fullscreen.rs)) が正本で、
呼び出し側 (Step 3) が `PageSide { left }` をそこへ流す。ここで方向を決めると二重管理になる。

判定順: `Excluded` を最優先 → `Center` → `PageSide`。

### 3.3 ジェスチャ認識と所有権 (plan §5.10)

```rust
pub(crate) enum TouchOwner {
    /// まだどのジェスチャか決まっていない
    Undecided,
    /// UI ボタン / パネル上で開始 → egui pointer へ委譲
    WidgetPassthrough,
    /// 拡大画像の単指ドラッグ → 既存の pointer パンへ委譲
    ViewerPointerPassthrough,
    /// タップ確定 → 領域コマンドを発火
    ViewerTapZone,
    /// 2 本目が入った → 単指の pending を取り消して取得
    Pinch,
    /// 画面端からの内向きスワイプ
    EdgeSwipe { left: bool },
    Cancelled,
}
```

**規約 (plan §5.2 「所有権のライフタイム」/ §5.10)**:

- **2 本目の接点が入った時点で、pending の single tap を取り消す**。最初の指が先に離れても
  tap / ページ送りを発火させない。**ここが一番落としやすい**
- 一度 pan / pinch へ確定した stream は、**全接点が End / Cancel するまで別 owner へ移さない**
- いずれかの接点に `Cancel` が来たら、**その stream 全体を `Cancelled` にして何も発火しない**
- 全接点が離れるまで「primary 抑止が必要か」を問い合わせられること
  (Step 2 が egui の click 抑止に使う)

### 3.4 しきい値 (すべて定数として 1 か所にまとめ、doc comment で根拠を書くこと)

| 項目 | 値 | 備考 |
| --- | --- | --- |
| tap と見なす最大移動量 | **12 pt** (開始点からの最大変位) | 超えたらドラッグ |
| tap と見なす最大時間 | **700 ms** | 超えたら tap を発火しない (長押しは egui のコンテキストメニューに任せる) |
| エッジスワイプの開始帯 | `max(28.0, surface 幅 × 0.05)` | plan §5.5 の「24〜32pt または幅の 5%」 |
| エッジスワイプの内向き移動 | **40 pt 以上** | plan §5.5 の「32〜48pt 以上」 |
| エッジスワイプの方向条件 | 横移動が縦移動の **1.5 倍以上** | plan §5.5 の「横が縦より明確に大きい」 |
| ピンチと見なす最小接点数 | **2** | 3 点以上でも 2 点だけを見る |

### 3.5 出力

```rust
pub(crate) enum TouchCommand {
    /// 中央矩形タップ → クローム表示のトグル
    ToggleChrome,
    /// 左右タップ。読み方向は呼び出し側で解決する
    PageSide { left: bool },
    /// 端からの内向きスワイプ → その側のパネルを開く
    OpenSidePanel { left: bool },
    /// ピンチ。呼び出し側が既存の zoom-pan 適用層へ流す
    Zoom { factor: f32, pivot: egui::Pos2 },
    /// 2 本指パン
    Pan { delta: egui::Vec2 },
}
```

`Zoom` / `Pan` は**倍率と移動量だけ**を返す。zoom min-max clamp / pan clamp / PDF 再レンダリングは
既存の適用層 (`zoom_preserve_pivot` / `set_fs_pan_from_input`) の責務なので**ここでやらない**
(plan §5.6)。

---

## 4. 入れないもの (明示)

Step 1 の範囲を守ること。以下はすべて後続ステップ:

- **egui / Win32 との配線** — Step 2 / Step 3 / Step 4
- **入力源の相関判定 (fail-closed)** — Step 2。イベント列シグネチャはここでは扱わない
- `MIV_DISABLE_TOUCH_GESTURES` — 純ロジックは環境変数を読まない。Step 2 の gate
- **一覧グリッドの anchor + fraction スクロール** (plan §5.4) — Phase 2
- **選択済みセルの再タップ open** (plan §5.8) — Phase 2
- **動画の左右ダブルタップ相対シーク** (plan §5.5) — Phase 3。ダブルタップ認識もここでは作らない
- **フリック / 慣性 / 長押しリング / ルーペ / ピンチ回転** — plan §5.13 で「入れない」と確定済み
- 初回オーバーレイヘルプ — Step 3 以降

---

## 5. テスト (plan §5.12 のテスト方針に沿うこと)

**すべて純関数の unit test**。`src/touch_input.rs` の `mod tests` に置く。最低限:

- タップ領域: 中央 / 左 / 右 / 除外矩形の内側、および**境界値**
- 除外矩形が中央矩形と重なる場合に `Excluded` が優先されること
- surface が極端に細長い / 小さいときに中央矩形が破綻しないこと
- **2 本目の接点が入ったら pending single tap が取り消される** (最初の指が先に離れる順序を含む)
- 一度 Pinch に確定したら、1 本になっても tap へ戻らないこと
- `Cancel` で全状態が破棄され、コマンドが 1 つも出ないこと
- tap しきい値 (12pt / 700ms) の内側 / 外側
- エッジスワイプ: 成立 / 内向き不足 / 縦成分が大きすぎる / 開始点が帯の外
- 接点 id が大きい値・不連続でも追跡できること (実機は単調増加で再利用しない)

---

## 6. 完了条件

- `cargo fmt` (引数なし) を通すこと
- `cargo test -p mimageviewer --lib touch_input` が通ること
- `cargo check -p mimageviewer --bin mimageviewer-core` が通ること
- **非 Windows でも壊さないこと**。このモジュールは純ロジックなので `#[cfg(windows)]` は
  一切不要のはず。必要になったら設計を見直すサイン
- **新しいモジュールが誰からも呼ばれていない状態で警告が出ないこと**。
  `dead_code` を握り潰すのではなく、`pub(crate)` として `lib.rs` に `mod touch_input;` を
  宣言する形で扱う。それでも警告が出る項目があれば、**どれが未使用かを報告すること**
  (Step 2 で使う予定のものなら許容、そうでなければ設計過剰の疑い)

## 7. 制約

- **アプリを起動しないこと。** 検証ビルドは ClaudeCode が用意する
- ブランチ操作・コミットは不要。master の作業ツリーで作業する
- **範囲を広げないこと。** 「ついでに配線もできそう」は Step 2 で扱う。
  1 ファイル + テストに閉じること
- detached-rework 凍結ルールは有効。純ロジックなので触れないはず

完了したら、変更内容・**定数の根拠**・テスト結果・**§6 の未使用警告の状況**を報告すること。
plan の記述と食い違う判断をした箇所があれば、その理由も明記すること。

# Codex ブリーフ — SNS 分割書き出し P2 (モード / 描画 / パネル / ボタン / キー)

正本: [docs/sns-split-export-plan.md](../sns-split-export-plan.md)。**着手前に §4.1〜§4.4 / §4.7 / §4.8 / §5 を読むこと。**
P1 は commit `676b4cbd` で入っています ([src/sns_split.rs](../../src/sns_split.rs))。

このブリーフの範囲は **P2 のみ**。**書き出しは P3、投稿後プレビューは P4 なのでやらない。**

---

## 1. 作るもの

フルスクリーンの編集ツールとして「SNS 分割」モードを足す。既存の「切り取り」モード
([src/ui_crop.rs](../../src/ui_crop.rs)) と**同じ形**で作る。

- グループ矩形を 8 ハンドル + 本体ドラッグで操作する
- N 個の枠と、枠と枠の間の捨てる帯を描く
- パネルで投稿先 (X / Instagram) と枚数 (2/3/4) を切り替える
- Escape で破棄して抜ける

この段階では **<kbd>Ctrl+E</kbd> を押しても何も書き出さなくてよい** (P3 で繋ぐ)。
モードを抜けるところまでで止めて構いません。

---

## 2. 触ってよいファイル

- `src/ui_sns_split.rs` (新規。App グルー)
- `src/sns_split.rs` (必要なら公開 API の追加のみ。既存の幾何の意味は変えない)
- `src/lib.rs` (モジュール宣言)
- `src/app.rs` (状態フィールドの追加と、§3 で列挙する分岐への追加)
- `src/ui_fullscreen.rs` (§3 で列挙する dispatch)
- `src/ui_adjustment_panel.rs` (ボタン 1 個の追加)
- `src/ui_fullscreen/draw_icons.rs` (アイコン 1 個の追加)
- `src/keymap.rs` (`KeyAction` / `KeyContext` の追加)
- `src/settings.rs` (設定 2 項目の追加)
- `docs/keymap.ini.default`

**`src/export_crop.rs` と `src/ui_crop.rs` は変更しない。** `CropRect` / `CropHandle` を
読み取り専用で使うのは可。既存の切り取りの挙動を 1 ミリも変えないこと (正本 §3.1)。

---

## 3. 最重要 — 既存モードと同じ入口・終了経路を全部たどる

CLAUDE.md「バグ修正の一般原則」に、状態を持つ機能は
**「同じ状態の producer / consumer と open / switch / close / cancel / error lifecycle を列挙してから修正する」**
とあります。今回はまさにそれです。

`export_crop_mode` は次の場所に出てきます (P1 時点の行番号)。

| 場所 | 何をしている |
| --- | --- |
| [app.rs:7426](../../src/app.rs:7426) | viewer context bundle の swap に `export_crop_spread_ctx` を載せている |
| [app.rs:11695](../../src/app.rs:11695) | フィールド宣言 |
| [app.rs:14222](../../src/app.rs:14222) | `Default` 初期化 |
| [app.rs:23814](../../src/app.rs:23814) / [app.rs:26327](../../src/app.rs:26327) / [app.rs:63438](../../src/app.rs:63438) | 各種遷移でのリセット |
| [app.rs:37881](../../src/app.rs:37881) | モード中の分岐 |
| [app.rs:52117](../../src/app.rs:52117) | ページ復元順序に関するコメント付きの reset |
| [ui_fullscreen.rs:10129](../../src/ui_fullscreen.rs:10129) | 述語への参加 |
| [ui_fullscreen.rs:15323](../../src/ui_fullscreen.rs:15323) 〜 15345 | キャンバス描画の分岐 |
| [ui_fullscreen.rs:15332](../../src/ui_fullscreen.rs:15332) | overlay 描画の呼び出し |
| [ui_fullscreen.rs:15594](../../src/ui_fullscreen.rs:15594) | パネル描画の呼び出し |
| [ui_fullscreen.rs:19377](../../src/ui_fullscreen.rs:19377) | キー処理の dispatch |
| [ui_fullscreen.rs:23107](../../src/ui_fullscreen.rs:23107) | パネル領域のヒットテスト (背面へ入力を漏らさない) |
| [ui_fullscreen.rs:26422](../../src/ui_fullscreen.rs:26422) | 別経路からの reset |

**この一覧をそのまま作業リストにしてください。**1 箇所ずつ見て、新モードにも同じ扱いが要るか
判断し、**要る / 要らないの理由を報告に書くこと。**「動いたから終わり」にしない。ここを 1 つ
落とすと、フォルダ移動やページ送りでモードが残る / 入力が背面へ抜ける、といった形で出ます。

自分で `git grep -n export_crop_mode` を掛け直して、上の表に無い箇所が増えていないかも
確認してください。

### detached 周りの注意

[app.rs:7426](../../src/app.rs:7426) は viewer context bundle の経路です。CLAUDE.md
「Detached viewer リワーク中のルール」により、この付近は**症状パッチ禁止**の凍結対象です。
既存の切り取りと**同型の構造的な追加**であれば触って構いませんが、

- 着手前に [docs/detached-rework-plan.md](../detached-rework-plan.md) §2 を読むこと
- 触った範囲と「なぜこれは症状パッチではなく構造的追加なのか」を**報告に明記**すること

新モードを bundle に載せるべきかどうかも、載せる/載せない両方の理由を書いてください。

---

## 4. 状態

```rust
// App
pub(crate) sns_split: Option<crate::sns_split::SnsSplitLayout>,
pub(crate) sns_split_drag: Option<...>,       // グループのドラッグ中状態
pub(crate) sns_split_spread_ctx: Option<...>, // 見開き pivot の復元用 (crop と同型)
```

- **`sns_split.is_some()` がモードの有無**。別に `bool` を置かない
  (相互排他な状態を bool と Option の二重表現にしない = CLAUDE.md「バグ修正の一般原則」)
- モードを抜けたら `None` にする。**ページにも DB にも保存しない** (正本 §4.1)
- `Settings` に残すのは投稿先と枚数だけ。§7 参照

入場・退場は `enter_export_crop_mode` / `reset_export_crop_mode`
([ui_crop.rs:162](../../src/ui_crop.rs:162)) と同型にする。見開き表示中は
`plan_page_edit_pivot` で Single へ pivot し、退場時に `leave_page_edit_single_view` で戻す。

---

## 5. キャンバス描画と操作

`draw_export_crop_overlay` ([ui_crop.rs:452](../../src/ui_crop.rs:452)) を参考にする。
座標変換は `DisplayedImageTransform` の `screen_to_source_normalized` /
`source_normalized_to_screen`。

- **描画は必ず `layout.frames()` から行う。**グループ矩形から自分で枠を計算しない
  (P1 が整数で組んだ結果と食い違う)
- 暗転マスクは `frames_extent()` の外側。枠と枠の間の帯も暗転する
  (「ここは出力されない」が見て分かるように)
- 枠の中に **1 / 2 / 3 / 4 の番号**を描く (= 投稿順)。文字は既存の描画方法に合わせる
- ハンドルは**グループの 8 箇所のみ**。枠の境界にハンドルを出さない
- ドラッグは移動と 8 方向リサイズ。**新規作成ドラッグ (`ExportCropCreateDrag` 相当) は作らない**
  (グループは常に存在する)
- **ドラッグのたびに `clamped(image_size)` を通し、返ってきた layout を採用する。**
  `clamped` は結果を整数枠へスナップするので、自前で丸めない
- リサイズは比率固定。比率は `SnsSplitLayout::group_aspect(target, count)`

### 入場時の初期値

`SnsSplitLayout::centered_max(target, count, image_size)`。投稿先と枚数は `Settings` の
前回値を使う。

### 枚数 / 投稿先の切り替え

`with_count` / `with_target` を使う (中心と高さを保って作り直し、clamp まで済んでいる)。

---

## 6. パネル

`draw_export_crop_panel` ([ui_crop.rs:214](../../src/ui_crop.rs:214)) と同じ作りにする
(`egui::Area` + `Frame::popup` + `apply_dark_ui` + 自前の閉じるボタン + クリック sink)。
**ライトテーマで崩れるので `apply_dark_ui` を必ず通すこと。**

中身:

- 見出し「SNS 分割」
- **投稿先**: X / Instagram の 2 択 (ラジオか SegmentedButton 風。既存パネルの流儀に合わせる)
- **枚数**: 2 / 3 / 4
- 現在の枠の出力ピクセル寸法の表示 (例「1536 x 2048 x 4 枚」)
- `fits()` が false のときは**警告文を出し、書き出しに進めないことを明示**する
  (画像が小さすぎて N 枚入らないケース)

**隙間の数値入力は出さない。**投稿先が決める (正本 §5)。比率セレクタも出さない。

パネルの矩形は背面への入力を止めるヒットテストにも使う
([ui_fullscreen.rs:23107](../../src/ui_fullscreen.rs:23107) と同型)。

---

## 7. 設定

`Settings` に 2 つ追加 (正本 §4.1)。

- `sns_split_target: Option<String>` — `"x"` / `"instagram"`、既定 `"x"`
- `sns_split_count: u8` — 既定 2

`SnsTarget::stable_key` / `from_stable_key` で往復する。**ページに紐づく状態は保存しない。**
永続化は「未リリースの新規項目」なのでマイグレーション不要 (CLAUDE.md「永続データ・スキーマ
変更時の判断」)。その旨をコミットメッセージではなく報告に書いてください。

---

## 8. ボタンとアイコン

[ui_adjustment_panel.rs:13953](../../src/ui_adjustment_panel.rs:13953) 付近。

- 現在は `header_rect.max.x` から右詰めで エクスポート / テキスト / 切り取り / 隠蔽 /
  補正レイヤー / 消しゴム の順。**SNS 分割をエクスポートより右**に置く
  (= 新しい右端。利用者了承済み)
- `draw_header_icon_button` を使う。ツールチップは「SNS 分割」
- 有効条件は他の編集ツールと同じ (`can_start_edit_tool`)
- **アイコンは文字にしない。**ボタンは 28x28 px なので「SNS」の 3 文字は読めません。
  `draw_icons.rs` に `draw_sns_split_icon` を足し、**縦長の枠が 3 つ並び、間に隙間がある**
  ピクトグラムをベクターで描く。既存の `draw_crop_icon`
  ([draw_icons.rs:754](../../src/ui_fullscreen/draw_icons.rs:754)) の書き方に合わせる

---

## 9. キー割り当て

正本 §4.8 のとおり。[docs/keymap-spec.md](../keymap-spec.md) の規約に従い、
`ini_name()` / `context()` / `trigger()` / `default_chords()` / `ALL_ACTIONS` /
呼び出し側 helper / [docs/keymap.ini.default](../keymap.ini.default) を揃える。

| Action | 文脈 | 既定 |
| --- | --- | --- |
| `FsSnsSplitMode` | フルスクリーン | **既定割り当て無し** |
| `SnsSplitExecute` | `KeyContext::SnsSplit` (新設) | <kbd>Ctrl+E</kbd> |

`KeyContext::SnsSplit` は `label` / 一覧 / 文脈別ヘルプ (`?` キー) にも登録する
(`KeyContext::Crop` を `git grep -n` して同じ場所を全部たどること)。

P2 では `SnsSplitExecute` は**モードを抜けるだけ**でよい (P3 で書き出しに繋ぐ)。
Escape も既存の切り取りと同じく破棄して抜ける。

---

## 10. 検証

```
cargo fmt
python scripts/check_ui_glyphs.py          # 0 件であること
cargo test -p mimageviewer --lib sns_split
cargo test -p mimageviewer --lib
cargo test --test ui_snapshot
cargo check -p mimageviewer --bin mimageviewer-core
```

- **`cargo test --test ui_snapshot` が落ちたら**、ボタン追加による意図した見た目変化かを
  確認し、意図どおりなら `UPDATE_SNAPSHOTS=1 cargo test --test ui_snapshot` で更新して
  **PNG も一緒に報告**する ([docs/ui-snapshot-policy.md](../ui-snapshot-policy.md))
- 新しい状態遷移にはテストを付ける (CLAUDE.md「状態バグには状態遷移テスト」)。最低限:
  - 入場 → `sns_split.is_some()`、退場 → `None`
  - 枚数・投稿先の切り替えで枠数と比率が追従する
  - `fits()` が false の layout でパネルが警告状態になる
  - §3 で「reset が要る」と判断した遷移それぞれで、モードが確実に落ちる
- コミットしない

## 11. 報告してほしいこと

1. **§3 の一覧を 1 行ずつ**、新モードにどう対応したか (対応した / 不要と判断した + 理由)
2. `git grep -n export_crop_mode` で表に無い箇所が増えていたか
3. detached (bundle) 経路に触れたか。触れたなら、なぜ構造的追加であって症状パッチでないか
4. 追加したテストの一覧と全コマンドの結果
5. ui_snapshot を更新したか
6. 正本と食い違った点、迷った点 (正本は書き換えないこと)

# ブリーフ: ページ送り判定の入力信号を、フレーム間で安定したものにする

## 前提 (必ず守ること)

- **アプリを起動しない**。ビルドとテストまでで止める。
- **git 操作をしない**。master の作業ツリーに未コミットのまま残す。
- **着手前に [docs/display-pipeline.md](display-pipeline.md) §2.5、特に新設した
  §2.5.2.1「『ページ送り中か』はフレーム間で安定した信号でなければならない」を読むこと。**
  そこに今回の原因と、やってはいけない直し方が書いてある。

## 観測 (2026-08-11)

カラー化した PDF でキーを押しっぱなしにすると、**ページが進まないまま同じページがちらつき、
「カラー化中」のトーストが出たまま**になる。

`python scripts/analyze_perf.py <jsonl> page-turn --check` の結果 (26 バースト / 18 違反):

```
I1 violation: burst t=45.222..45.987 idx=123
  actual sources at idx=123:
    final_composite -> thumbnail -> final_composite -> thumbnail -> ... (28 往復)

I1 violation: burst t=21.588..22.019 idx=0   (同じ形で 13 往復)
I4 violation: burst t=16.467..17.495 forward idx=1..32  idx=32 ended mode=pass_through
```

**idx は動いていない。** 同じページのまま描画元だけが毎フレーム往復している。

## 壊れている前提

`fs_page_turn_materialization_for_frame` は、ページ送り中かどうかを
`keymap.pending_chord_press_in_frame(viewport, chord)` = **「このフレームに未消費の
ページ送り edge が残っているか」** で判定している。

キーリピートは約 30 回/秒、描画は 60fps。**1 フレームおきに true / false が入れ替わる**ので、
`paint_source` も 1 フレームおきに `Thumbnail` / `Composite` を往復する。ページが進んで
いなくても往復するし、進んでいても隣り合うページで描画元が変わる。

今日この領域を 4 回直したが、**4 回とも判定の右側 (どの条件を足すか) をいじっていて、
入力信号そのものが振動していることに気づいていなかった**。

## やること

ページ送り中かどうかの判定を、**押下状態の直読み**へ変える。

- `pending_chord_press_in_frame` の代わりに、既存の
  `Keymap::key_held_chord` / `keymap::key_held_via_os` 系の**押下判定**を使う。
  対象は現在と同じ chord 集合 (`FS_PAGE_TURN_COALESCE_ACTIONS` の有効 chord と
  `FS_FIXED_PAGE_TURN_CHORDS`、`fs_page_turn_chord_is_unambiguous` の絞り込みも維持)。
- viewport の選び方 (embedded / fullscreen) は現在の実装をそのまま使う。
- **`defer_ui_uploads` と `paint_source` の両方**がこの信号を使う。2 軸分離
  (`00d23a33`) は維持する。
- 同一フレーム内の再 pass 用フレームキャッシュ (`frame_nr` / `items_generation` / `idx`) は
  そのまま。

### やってはいけないこと

- **時間閾値を入れない**。「最後の edge から N ms 以内なら押しっぱなし扱い」は §1.58 が
  明示的に避けた形であり、§2.5.2.1 でも禁止している。
- **前フレームの決定を覚えて平滑化しない**。原因は信号の選び方なので、履歴で隠さない。
- **`passthrough_rendition_ready` の側をいじらない**。今回の原因ではない。

### 確認すべき副作用

- **単発のキー押下** (押してすぐ離す) で通過表示に入るか / 入らないか。入らないなら
  それが望ましい (1 ページだけの移動は完成画像で見せてよい)。挙動を報告すること。
- **キーを離した最初のフレームで通過表示が終わる**こと (I4)。押下状態なので即座に false に
  なるはず。
- フォルダ末尾で押し続けたときの既知の例外 (最後のページがサムネイル画質のまま留まる、
  利用者判断 2026-08-10) は**そのまま維持**でよい。

## 完了条件 / 回帰テスト

- 純関数の 4 通り判定は現状維持 (入力の作り方が変わるだけ)。
- **状態遷移テストを 1 本足す**: 同一 idx で「押下されたまま」が続くフレーム列において、
  `paint_source` が**一度も往復しない**こと。これが今回の症状を直接表す不変条件。
- キー解放の次フレームで `Composite` へ戻ること。
- `cargo fmt --check` / `cargo check -p mimageviewer --bin mimageviewer-core` が warning なしで通る。
- `cargo test -p mimageviewer --lib page_turn` が通る。

## 報告してほしいこと

- 使った押下判定の API と、chord 集合の絞り込みを維持できたか。
- 単発押下がどう振る舞うか。
- 追加したテスト。
- **`processed_other` という描画元**が実ログに出ている (`analyze_perf.py` の分類)。
  これが何を指すのか (holdover か、別経路か) を調べて報告すること。**今回は直さなくてよい**。

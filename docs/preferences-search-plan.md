# 環境設定の検索 — 設計と実装手順 (正本)

バックログ §1.65 (5ch 専用スレ #207、2026-08-11) の正本。設定項目が増えて目的の項目に
辿り着けないという要望への対応。

着手前に [preferences-layout-guidelines.md](preferences-layout-guidelines.md) と
[ui-responsiveness.md](ui-responsiveness.md) §4 を読むこと。

## 1. やること / やらないこと

**やること**: 環境設定ダイアログ内で語を入力すると、該当する設定項目の一覧が出る。選ぶと
そのページへ移動し、該当箇所まで自動スクロールして数秒だけ強調表示する。

**やらないこと**: 「該当項目だけを 1 画面に抽出してその場で操作する」形式 (Siki 型)。現在の
手続き的な egui ページ描画では、同じ描画コードを別レイアウトで再利用する仕組みが無く、
状態を持つページ (LUT 一覧、VST3 チェーン、Susie 一覧) が壊れる。初期対応では目指さない。

## 2. UI の形

```
┌ 環境設定 ─────────────────────────────────────────────┐
│ ┌─────────────┬───────────────────────────────────┐ │
│ │ [🔍 設定を検索  ]│                                   │ │
│ │ 全体設定       │  (右ペイン: 選択中のページ、または  │ │
│ │ 起動と連携     │   検索結果一覧)                     │ │
│ │   起動時に開く…│                                   │ │
│ └─────────────┴───────────────────────────────────┘ │
└───────────────────────────────────────────────────────┘
```

- **検索欄は左ツリーの上**に置く。ツリー幅 (180px) のまま `desired_width` を
  `ui.available_width()` に合わせる。
- **結果一覧は右ペインに描く**。ポップアップは使わない。CLAUDE.md「Popup / menu wheel
  passthrough」の問題を持ち込まないため、および 180px 幅では
  「ページ名 › 項目名」が読めないため。
- 右ペインが結果一覧を出すのは `showing_results == true` の間だけ。
  - 文字を入力 / 変更したら `true`。
  - 結果を選んだら `false` (右ペインはそのページになる)。
  - 左ツリーでページを選んだら `false`。
  - 検索欄を空にしたら `false`。
- 結果行は `項目名` を通常色、右側に `カテゴリ › ページ名` を弱色で出す。0 件のときは
  「一致する設定がありません」と出す。
- 検索欄には「クリア」を置かず、`Esc` と空文字で戻れれば足りる (既存の
  `command_filter_controls` はクリアボタンを持つが、あちらは 2 欄あるため)。

## 3. 索引 (腐らせないことが最重要)

### 3.1 データ構造

`src/ui_dialogs/preferences/search_index.rs` を新設する。

```rust
pub(super) struct PrefSearchEntry {
    /// ページ内で一意な anchor id。`"thumbnail/size"` のように `ページ/項目` 形式。
    pub anchor: &'static str,
    pub page: PreferencesPage,
    /// 画面に出ているラベルと同じ文字列。表記が違うと利用者が結果を信用しない。
    pub title: &'static str,
    /// ラベルに出てこないが利用者が打ちそうな語。ひらがな / カタカナ / 漢字 / 英語の
    /// 揺れをここで吸収する (例: 「ホイール」「wheel」「マウスホイール」)。
    pub keywords: &'static [&'static str],
}

pub(super) const PREF_SEARCH_INDEX: &[PrefSearchEntry] = &[ /* … */ ];
```

### 3.2 ページ側の登録

各設定コントロールを anchor helper で包む。helper は
`src/ui_dialogs/preferences.rs` に置く。

```rust
pub(super) fn anchored<R>(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    anchor: &'static str,
    add: impl FnOnce(&mut egui::Ui, &mut PreferencesState) -> R,
) -> R
```

- 中身を `ui.scope` で描き、その rect を得る。
- `state.pending_anchor == Some(anchor)` なら `ui.scroll_to_rect(rect, Some(Align::Center))` し、
  `pending_anchor` を消して `highlight = Some((anchor, 表示開始時刻))` にする。
- `highlight` が生きている間 (2.5 秒) は rect の周りに `ui.visuals().selection.stroke` 系の
  角丸枠を描き、`ctx.request_repaint()` する。時刻は `ctx.input(|i| i.time)`。
- **レイアウトを変えないこと**。余白・幅・折り返しが変わると既存ページの見た目が動く。

### 3.3 粒度

- **ラベルの付いたコントロール 1 つにつき 1 entry**。同じ行 / 同じ小見出しの下に並ぶ
  細かいコントロール (例「上限」「下限」) は、その見出しでまとめて 1 entry にしてよい。
- **すべての `PreferencesPage` に最低 1 entry** を持たせる (下の網羅テストで強制する)。

### 3.4 網羅テスト (これが本体)

`include_str!` で `pages.rs` を読み、次を検査する unit test を書く。既存の
「raw TextEdit 禁止テスト」(`src/ime_focus.rs` の allowlist 検査) と同じやり方。

1. `PREF_SEARCH_INDEX` の anchor id が一意であること。
2. 各 anchor id が `pages.rs` の中に文字列として現れること (= 索引にあるのに
   どのページにも置かれていない dead entry を弾く)。
3. `pages.rs` 内の `anchored(` 呼び出しに出てくる id が全て索引にあること
   (= 置いたのに検索から出てこない項目を弾く)。
4. すべての `PreferencesPage` 列挙子が最低 1 entry を持つこと
   (= ページを足したのに索引を更新していない状態を弾く)。
5. `title` が空でなく、`keywords` に重複が無いこと。

**3 と 4 が無いと索引は必ず腐る。** 落ちたテストのメッセージには、どの id / どのページが
不足しているかを出すこと。

## 4. 検索の一致規則

- 大文字小文字を無視する (ASCII のみ `to_lowercase`)。
- 検索対象は `title` + `keywords` + ページ名 (`PreferencesPage::label`) +
  カテゴリ名 (`TREE` の `label`)。
- 部分一致 (substring)。日本語の分かち書き・活用の吸収はしない。表記揺れは `keywords` で
  明示的に持つ。
- 空白区切りの複数語は **AND**。
- 並び順: ①`title` の前方一致 → ②`title` の部分一致 → ③`keywords` 一致 →
  ④ページ名 / カテゴリ名一致。同順位内はツリーの並び順。

## 5. 触ってはいけないこと

- **結果一覧の描画からページ描画関数を呼ばない。** フォント一覧、VST3 scan、Susie scan、
  キャッシュ集計のような worker / 同期 I/O を持つページがあり、検索するたびに起動しては
  ならない (ui-responsiveness.md §4)。結果一覧は静的な索引だけを読む。
- **`pref_panel` の scroll style を floating に戻さない** (preferences-layout-guidelines.md)。
- `right_panel_scroll_generation` はページ切替時に scroll offset を捨てるための世代値。
  検索結果からページへ飛ぶときも従来どおり世代を進め、その **同じフレームで**
  `scroll_to_rect` が効くことを確認する。

## 6. IME / キー

- 検索欄は `crate::ime_focus::add_singleline` で描く (raw `TextEdit` は禁止)。
- `Enter` = 先頭の結果へ飛ぶ。`Esc` = 検索欄を空にして結果表示をやめる。どちらも
  `dialog_enter_pressed` / `dialog_escape_pressed` を **closure の外で**取得して渡す
  (CLAUDE.md「IME 対応」)。IME 変換中の Enter / Esc を奪わないこと。
- 検索欄を開くキーは足さない。環境設定を開いた直後にフォーカスも奪わない
  (従来どおりツリー操作で入れるようにしておく)。

## 7. テスト

- §3.4 の網羅テスト。
- 一致規則の純関数テスト: 前方一致が部分一致より上に来る / AND 条件 / 大文字小文字無視 /
  キーワード一致。
- `PreferencesPage` を 1 つ足した状態を模した失敗ケース (テスト内で列挙子の集合を作って
  検査する形にすれば、実際に足さなくても検査ロジックを試せる)。
- UI スナップショット: 検索結果一覧を出した状態を 1 枚 (docs/ui-snapshot-policy.md)。

## 8. ドキュメント

- `htdocs/mimageviewer/manual/settings.html` に検索欄の説明を足す (バージョン表記は書かない)。
- `docs/spec.md` の環境設定の節に 1 行。
- 本ファイルを設計の正本として維持する。

## 9. 将来 (今回やらない)

- 操作カスタマイズ側には既に独自の検索欄 (`command_filter`) がある。統合するなら、
  環境設定の検索から「操作カスタマイズを開く」への誘導 entry を 1 つ置く形が素直。
- Siki 型の「抽出してその場で操作」は、ページ描画を宣言的な項目リストへ作り替えてから。

# リモート閲覧: 表示トリムと並べ替えを、端末から見えて触れる状態にする

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。
実装 = Codex、レビュー・テスト・統合 = ClaudeCode、実機確認 = 利用者。

## 1. 目的

mIV Remote は本体と同じ設定を共有している。にもかかわらず、**表示トリムと並べ替えは
端末から見えず、触れない**。本体で設定した値が黙って効くので、「なぜこの画像は端が
切れているのか」「なぜこの並び順なのか」が端末側で分からない。

見えないまま効いていることが混乱の元なので、**現在値が見えて、変えられる**状態にする。

## 2. 事実 (調査済み。再調査不要)

- 表示トリム: remote は `view_trim.db` を **監視しているだけ**。本体側の変更を
  cache 世代に反映する経路はあるが、書き込み口が無い。
  [store.rs](../crates/remote-web/src/store.rs) の `view_trim` / `include_view_trim`。
- 並べ替え: remote 側に API も UI も **無い**。
- 本体のモデル:
  - `ViewTrimBookSettings` [src/view_trim.rs](../src/view_trim.rs) =
    `enabled` / `spread_separate` / `single: ViewTrimMargins{left,top,right,bottom}` /
    `spread_linked: ViewTrimLinkedMargins{top,bottom,inner,outer}` /
    `spread_left` / `spread_right`。値の丸めは `clamped()` が持つ。
  - `SortOrder` [src/settings.rs](../src/settings.rs) = `FileName` / `Numeric` /
    `DateAsc` / `DateDesc`。`label()` / `short_label()` に表示名がある。
- 書き込み API は `/api/write` の typed kind (`get_adjustment_state` / `set_adjustment` /
  `set_rating` など)。新しい endpoint を足さず、この形に合わせる。

## 3. 入れるもの

### 3.1 表示トリム (本単位)

- 現在の `ViewTrimBookSettings` を読み、変更できるようにする。
- 対象は **本単位の設定のみ**。ページ個別の override は対象外 (§5)。
- UI は既存のビューア panel (補正 panel と同じ枠組み・タブ) に載せる。新しい panel 機構を
  作らない。
- 値の丸めは **本体の `clamped()` を通す**。remote 側で別の clamp を書かない。
- 見開き分離 (`spread_separate`) の on/off で、編集対象が
  `spread_linked` と `spread_left` / `spread_right` のどちらになるかが変わる。
  本体と同じ対応にする。

### 3.2 並べ替え

- 一覧の並び順 (`SortOrder` の 4 値) を読み、変更できるようにする。
- **本を表示している間は名前順固定**で、本体では並べ替え UI を無効にしている。
  remote も同じにする。「端末では別の順序」にしない。
- 一覧を出す画面 (グリッド) から操作できるようにする。既存のメニュー / ツールバーの
  作りに合わせる。

### 3.3 変更が共有されること

これらは本体と共有された設定なので、**端末から変えると本体側の表示も変わる**。それが
この作業の目的であり、端末ローカルの別値を作らない。ただし利用者が驚かないよう、
**変更する前に現在値が見えている**こと。

## 4. 構造の決め

- **本体のモデルをそのまま写す。** remote 専用のトリム種別・並び順・既定値を作らない。
  本体に無い状態を remote で表現できるようにしない。
- **無効化は無言にしない。** 本表示中の並べ替えのように操作できない状態は、操作を
  黙って捨てるのではなく、無効であることが分かる形にする。
- **失効は既存の 1 経路に通す。** remote 起点の変更でも、本体起点の変更と同じ
  `remote_state_generation` / cache 失効の経路を通す。remote 起点だけの特別扱いを
  足さない。
- **先読みで取り繕わない。** トリム変更は本体側で再レンダリングして返る。
  遅いのは許容する仕様なので、端末側で暫定のトリムを描いて見た目だけ先に変える、
  といった処理を入れないこと。処理中であることは既存の表示で伝える。
- 相互排他の状態を bool や `Option` の組み合わせで増やさない。編集対象がどれかは
  `spread_separate` から導く。

## 5. 対象外

- ページ個別のトリム override (ドラッグでの範囲指定 UI が必要。本単位の共有設定が
  見えないことが今回の問題)。
- 検索 (別作業)。
- サムネイル一覧の表示項目・列。

## 6. テスト

- 値の解釈と丸めを純関数として切り出し、単体テストを付ける。最低限:
  - `spread_separate` の on/off で編集対象が切り替わる
  - 範囲外の値が本体と同じ結果に丸められる
  - 本表示中は並べ替えが無効になる
- 変更後に、本体起点の変更と同じ失効経路を通ることを検証する。
- 既存の web テスト 221 件と Rust テストを維持すること。

## 7. 確認

- web テスト一式と、変更した Rust crate のテストが通ること。
- `cargo fmt --check` と `git diff --check` が通ること。
- **ビルドとコミットは行わない。** 変更ファイルと追加テストの一覧を報告する。
- 既存の未追跡 brief (`docs/briefs/*.md`) には触れない。

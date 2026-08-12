# 表示所有権 cutover 段階 3c — 位置 (requested / displayed) の所有権

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。

## 0. 前提 — 先に読むもの

- [`docs/web-remote-plan.md`](../web-remote-plan.md) **§14.5**、**§14.5.1**、**§14.9**、**§14.11**
- `crates/remote-web/web/page-coordinator.mjs` — 段階 1 で入れた**純粋な状態機械 + 効果列**の形。
  今回もこの形に揃える
- `crates/remote-web/web/command-core.mjs` — `viewerPageGroupRequestMatches` (1774 行付近)、
  `viewerGroupLoadCompletionPlan` (1790 行付近)、`viewerPagePositionTransition` (1833 行付近)、
  `viewerPagePositionFeedback`
- `crates/remote-web/web/app.js` — `captureViewerPageGroupRequest` (3890) /
  `updateRequestedPageGroup` (3938) / `discardRequestedPageGroup` (3969) /
  `commitRequestedPageGroup` (5716) / `changeImageTo` (5706) / `commitSeekGroup` (5531) /
  `openRemoteBookBookmarkTarget` (4221) / `refreshContainerSpread` (4371) /
  `performContainerSpreadRefresh` の再アンカー (4506-4527) / `updateViewerImage` (5790) /
  `ImageViewer.setRequestedPagePresentation` `commitPagePresentation` `displayedGroupIndex`
  (11175-11228)

段階 3a / 3b で入れた coordinator・registry・heavy queue・lease には**触らない**。
protocol も **v43 のまま**上げない (wire の変更は無い)。

## 1. 直す対象 (再調査不要、コードで確認済み)

段階 A は「**位置を動かした要求が自分の失敗を巻き戻す**」形で収束させた。所有は
`positionRequest` という token を要求ごとに手渡している。plan §14.5.1 のとおり、
**token を渡し損ねる経路が構造的に残る**。

| # | 経路 | 現状のコード | 起きること |
|---|---|---|---|
| 1 | 非位置再描画が位置要求を追い越す | ページ送り B の読み込み中に fit 変更 / resize / generation 更新が入ると queue が元の要求を `superseded` にし、後着の要求は `positionRequest` を持たない (`renderTrigger: "fit_mode"` 等の呼び出しは token を渡さない) | 終端失敗しても巻き戻らない。**表示は A、位置と seek は B** |
| 2 | ブックマーク jump | `openRemoteBookBookmarkTarget` が `state.pageGroupIndex = groupIndex` を直接代入し、`updateViewerImage(performance.now())` を token 無しで呼ぶ (4240-4257) | 終端失敗すると新しい位置・seek と古い DOM が残る |
| 3 | URL / history の残存不整合 | `commitRequestedPageGroup` が表示完了**前**に `history.pushState` する (5734) | 失敗して位置を戻しても URL は失敗ページを指したまま |

**入口ごとに token を配って回るのは §14 冒頭と同じ誤り**である。入口を増やすたびに
渡し忘れが 1 つ増える。3c では所有の表現そのものを変える。

## 2. 決定 — 義務は state から導く

> **「要求が所有する」のをやめ、「要求位置が表示位置より先にあること自体が義務を生む」形にする。**

終端失敗の巻き戻し規則を、token ではなく **今の (requested, displayed) の対**から決める。

- 失敗した読み込みの対象が**今の requested と同一**で、かつ **requested ≠ displayed** なら
  requested を displayed へ戻す
- requested == displayed なら何も動かさず失敗だけ告げる (fit 変更 / resize / 補正保存 /
  見開き再構成の失敗で位置を動かさない、という §14.5 の規則はこれで自動的に満たされる)

経路 1 は、後着の要求が token を持たなくても requested(B) ≠ displayed(A) なので巻き戻る。
経路 2 も同じ。**入口ごとの条件分岐が消える。**

この規則が成り立つ前提が 1 つある。**displayed が自分の grouping 文脈を持つこと。**
今の displayed は `ImageViewer.displayedSeekState.groupIndex` という**裸の整数**で、
`refreshContainerSpread` が `pageGroups` を作り直すと別ページを指す。だから displayed も
requested と同じ identity snapshot で持つ。

## 3. 変更内容

### 3.1 新しい純粋モジュール `viewer-position.mjs`

`page-coordinator.mjs` と同じ方針 — DOM / `history` / fetch / timer に依存しない状態機械。
snapshot の中身は解釈せず、`viewerPageGroupRequestMatches` で比較する**不透明な値**として扱う
(現行 `captureViewerPageGroupRequest` の戻り値をそのまま入れる)。

保持する状態は `requested` と `displayed` の 2 つだけ。操作は次の 5 つで、いずれも
**typed な結果**を返す (正常系で例外を投げない)。

| 操作 | 意味 | 結果 |
|---|---|---|
| `open(snapshot)` | viewer を開いた最初の位置。requested = displayed = snapshot | `{ ok }` / `{ ignored: "..." }` |
| `request(snapshot)` | 位置を動かす**唯一の入口**。requested を進める | `{ moved, history: "push" \| "none", ... }` |
| `display(snapshot)` | DOM が実際に置き換わった時点で displayed を進める | `{ ... }` |
| `settle(result, { loadRequest })` | 読み込みの終端。§2 の規則で action を返す | `POST_DISPLAY` / `IGNORE` / `ROLLBACK` / `REPORT_FAILURE` |
| `rewind({ expected })` | requested を displayed へ戻す。`expected` を渡した場合は requested がまだそれと一致するときだけ | `{ rewound, to, history: "replace" \| "none" }` |
| `reanchor({ requested, displayed })` | grouping 再構成後に両方を新しい配列の snapshot へ張り替える | `{ requested: "resolved" \| "unresolved", displayed: ... }` |

固定する契約:

1. `settle` の action は次の順で決まる。`superseded` → `IGNORE`。`loadRequest` が今の
   requested と一致しない → `IGNORE`。`applied` → `POST_DISPLAY`。`failed` かつ
   requested ≠ displayed → `ROLLBACK`。`failed` かつ requested == displayed → `REPORT_FAILURE`。
2. **`displayed` が未解決 (null) のときは `ROLLBACK` を出さない** (戻す先が無い)。
   `REPORT_FAILURE` にする。
3. `rewind` は冪等。requested == displayed のときは `{ rewound: false, history: "none" }`。
4. **URL は常に requested を写す。** `request` が成功したら `history: "push"`、`rewind` が
   実際に戻したら `history: "replace"`。他の操作は `history: "none"` を返す。
   状態機械は URL 文字列を作らない (hash の組み立ては adapter 側)。
5. `reanchor` は grouping 再構成の**唯一の入口**とする。再構成後に古い配列の snapshot を
   持ったままにしない。displayed が新しい配列に見つからなければ `displayed: "unresolved"`
   とし、以後 §3.1-2 により巻き戻さない。
6. `display` は requested を動かさない。`request` は displayed を動かさない。

### 3.2 `command-core.mjs` の `viewerGroupLoadCompletionPlan`

`positionRequest` 引数を**削除**し、`displayedRequest` を受ける形へ変える
(`{ loadRequest, currentRequest, displayedRequest }`)。判定は §3.1-1 と同じ。
`viewer-position.mjs` はこの純関数を呼ぶ。**同じ判定を 2 か所に書かない。**

`viewerPagePositionTransition` / `viewerPagePositionFeedback` は、状態機械へ吸収できるなら
吸収し、残すなら状態機械から呼ぶ。**requested / displayed を書き換える経路を 2 本にしない**
ことが要件で、関数の残し方は問わない。

### 3.3 `app.js` — 入口の集約

- **位置を動かす入口を 1 つにする。** `updateRequestedPageGroup` + `commitRequestedPageGroup` を
  1 本の owner (例 `requestPageGroup(groupIndex, { reason })`) にまとめ、
  `changeImageTo` / `commitSeekGroup` / `openRemoteBookBookmarkTarget` の 3 経路が**全部そこを通る**。
  `state.pageGroupIndex = ...` の直接代入を position 移動の意味で残さない
  (viewer を開く `renderImageViewer` の初期化と、`performContainerSpreadRefresh` の再アンカーは
  それぞれ `open` / `reanchor` へ置き換える)。
- **history は owner だけが触る。** `request` の結果が `push` なら `pushState`、`rewind` の
  結果が `replace` なら `replaceState` で displayed の hash へ書き戻す。
  `viewerDepth` の増やし方は現行のまま (`+1`)。**rewind では depth を変えない**
  (エントリ数は減らないため。back が 1 回無駄になるのは許容する。§6 に記録すること)。
- **displayed の commit 点は現行のまま**、`ImageViewer` が DOM を差し替えた直後
  (`commitPagePresentation`) とする。ここから状態機械の `display(snapshot)` を呼べるよう、
  読み込み開始時に渡している presentation へ position snapshot を載せる。
  `displayedSeekState.groupIndex` を残すかは実装判断だが、**巻き戻し先の正本は snapshot** とする。
- `updateViewerImage` の `positionRequest` 引数を**削除**する。`settle` は
  `loadRequest`(読み込み開始時に取った snapshot) と状態機械の現在値だけで決まる。
- `discardRequestedPageGroup` は `rewind({ expected })` の adapter に置き換える。
  seek の session 取得失敗のように**表示前に諦める**経路は `expected` を渡し、
  自分が動かした位置がまだ現在値のときだけ戻す (後から入ったページ送りを巻き戻さない)。
- `performContainerSpreadRefresh` は再構成後に `reanchor` を呼ぶ。**requested は現行どおり
  再構成前の requested ページを追う**。displayed は `commitPagePresentation` 時に記録した
  entry identity で新しい `pageGroups` を引き直す。見つからなければ `unresolved`。

## 4. 触らないもの

- 段階 3a / 3b の coordinator / `PageDemandAdapter` / registry / heavy queue / lease / `PageDemand`
- protocol、本体 (`src/`) 側のコード。**この増分は Web だけで閉じる**
- 動画ビューアのファイル間移動 (5398 付近の `pushState`) と、grid ↔ viewer の
  `viewerFromGrid` / `history.go(-viewerDepth)` による復帰
- 失敗メッセージの文面 (`前のページに戻りました。` を付けるのは巻き戻した側だけ、という §14.5 の規則)
- `page_display` / `viewer_update` telemetry の既存フィールド名と outcome 値

## 5. テスト

```
cd crates/remote-web/web && node --test
```

(Rust 側は変更しない見込みだが、`src/` に触れたなら
`cp vendor/ffmpeg/bin/*.dll target/debug/deps/` の後に
`cargo test -p mimageviewer --lib remote_ipc::` と `cargo test -p mimageviewer-remote` も回す)

**`viewer-position.test.mjs` (新規)**

- §3.1 の契約 1-6 を 1 項目ずつ
- **経路 1 の回帰**: `request(B)` → 別要求が同じ B を読み直す (token 無し) → その `settle` が
  `failed` → `ROLLBACK` になり、requested が displayed(A) へ戻る
- **経路 2 の回帰**: jump 相当の `request(C)` を token 無しで出しても同じく戻る
- **非位置再描画**: requested == displayed の状態で `failed` → `REPORT_FAILURE` で位置は動かない
- **再構成**: `reanchor` 後に古い配列の index で `settle` が来ても、新しい配列の別グループを
  選ばない
- `displayed` が `unresolved` のときは `ROLLBACK` を出さない
- **操作列の網羅**: 段階 1 の `page-coordinator.test.mjs` と同じく、長さ 4 以下の操作列を
  総当たりして「requested / displayed が常に有効な snapshot か null」「`settle` が
  `ROLLBACK` を返した直後は requested == displayed」の 2 不変条件を検査する

**`command-core.test.mjs`**

- `viewerGroupLoadCompletionPlan` の既存テストを新しい引数へ移す (`positionRequest` を消す)

**`pwa.test.mjs`**

- `positionRequest` を前提にした構造 assertion (513 行付近と 532 行付近) を**新しい形へ
  置き換える**。最低限、次を機械的に固定する
  - `history.pushState` を呼ぶ位置移動の経路が **owner 1 か所だけ**であること
  - `state.pageGroupIndex` への代入が owner / `open` / `reanchor` の外に無いこと
  - `updateViewerImage` の呼び出しに `positionRequest` が残っていないこと

## 6. ドキュメント

- plan に **§14.13** を追加する。次を記録する
  - 義務を token から (requested, displayed) の対へ移したこと。§14.5.1 の 2 経路が
    **入口ごとの分岐を足さずに**閉じた理由
  - displayed が grouping 文脈を持つようにしたこと、`reanchor` が再構成の唯一の入口であること
  - **URL は requested を写す**という規則。巻き戻しで `replaceState` すること。
    back が 1 回無駄になる代わりに `viewerDepth` と grid 復帰を壊さないという判断
  - displayed が `unresolved` のときは巻き戻さない、という決定
- **§14.5 の「既知の残存不整合」と §14.5.1 を、解消済みとして書き換える** (消さずに、
  どこで閉じたかを §14.13 へ参照させる)。§14.5.1 の「テストの負債」も現状に合わせる
- §14.3 の「3c — 位置 ownership (保留)」を完了に更新する

## 7. 実行と報告

- §5 のコマンドを**毎回実行**して結果を報告する
- **`crates/` と `src/` に触れた箇所を全部、理由付きで報告する**
- **`scripts/build-dev.ps1` を実行しない。コミットもしない**
- ブリーフと意図的に違えた点があれば、その理由を報告する
- 実機で見るべき箇所を列挙する

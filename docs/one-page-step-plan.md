# 見開きのまま 1 ページ単位で送る (§1.796) 設計 r5

状態: 設計確定 (2026-11-16、独立設計レビュー 4 回目 ACCEPT)。実装と focused 自動検証完了（2026-11-17、以下 §8）。独立実装レビュー・designer の全件 gate・実機確認は未実施。r1 は独立設計レビュー (Sol xhigh) で REVISE (P1 2 件・P2 3 件)。r2 はその指摘を反映し、
Q1 の推奨を見直した。r3 は 2 回目のレビュー (REVISE、P2 3 件) を反映し、退役を受理済みの切替からの 1 関数に、
見開き以外の解決を普段の次 / 前に揃えた。r4 は 3 回目のレビュー (ACCEPT with changes) の条件 (退役を呼ぶ受理点の追加・テスト節の訂正) を反映した。
r5 は利用者の回答 (2026-11-16: Q1 = A、Q2・Q4 = 推奨どおり、Q3 = 構成ありの本も対象・コレクションの最上位は対象外) を反映した (§2・§3.5)。
出典: `docs/next-release-backlog.md` §1.796 (5ch >>788・>>791・>>796、方針は 2026-11-16 決定)。
§1–7 のコード参照は設計時の master `e3de9b547`。実機での観測は無い。実装後の自動検証は §8。

## 1. 決まっていること (バックログ §1.796)

- 通常の見開き送りとは別に、表示中の見開きの先頭を**読み順で 1 ページだけ進める / 戻す**専用操作を 2 つ足す。
  通常の連続ページなら `[1, 2] → [2, 3]` のように重なって表示する。
- 操作カスタマイズで個別にキーを割り当てられる。**既定キーはどちらも未割り当て**。
- **本ごとの見開き調整 (保存値・調整の起点) を書き換えない。「見開きをずらした」通知も出さない。**
  既存の `FsSpreadShiftPrev` / `FsSpreadShiftNext` (見開き調整) は変えない。
- 用途 (>>796): 常用ではなく、紙の本の仕掛けや前ページ参照など**特定の場面だけ**使う。普段の 2 ページ送りと併用し、
  横連結や単ページへ表示方式を切り替えずに使う。

## 2. 利用者に決めてもらう点

### Q1 1 ページ送った後に、普段の 2 ページ送りへ戻したときの組み方 — **決定: A (利用者、2026-11-16)**

本来の組が `[1,2] [3,4] [5,6]` の本で、`[1,2]` から 1 ページ送りで `[2,3]` を見ている状態とする。

| 案 | 「次へ」(普段の送り) | 「前へ」 | 元の組に戻るには | 実装の費用と性質 |
| --- | --- | --- | --- | --- |
| **A ずれたまま続ける (推奨)** | `[4,5]` | `[1]` (組み直しの手前の 1 ページは単独) | 1 ページ送りでもう一度ずらす / 本を変える / 見開き方式・読む向きを変える / フルスクリーンを閉じる | 既存の「1 ページずらし」と同じ経路で、**保存・通知・調整の書き換えだけを外したもの**。表示単位の列が 1 つのまま送り・演出・先読み・待ちの保留がそのまま働く。約 1.5〜2 日 (r3 レビューで退役を呼ぶ受理点が増えた分を含む) |
| B 元の組へ戻る | `[3,4]` (3 をもう一度見る) | `[1,2]` (2 をもう一度見る) | 普段の送り 1 回で自動で戻る | 重なった表示は「その場だけの見え方」。ただし表示用の列と送り用の列 (本来の組) を 2 つ持ち、演出中の保留・表示許可の確認待ち・コレクションの経路で「1 ページ送りの意図」を型で運ぶ必要がある (r1 レビューの P1)。約 2〜2.5 日 |

推奨は A。理由: 実装と状態が単純で (表示単位の列が 1 つ)、v5.3.0 に間に合わせやすい。>>788 の利用者は既存の「1 ページずらし」で
代用しており、不満は毎回の通知と本の調整が変わることだった (>>791)。A はその 2 点だけを取り除いた形になる。
B は「見終えたら自動で本来の組に戻る」点で >>796 の「特定の場面だけ」に合うが、費用が約 1 日増える。

r1 では B を推奨したが、独立レビューでコードを確かめた結果、B は送り用と表示用の列の分離・保留中の意図の運搬が要り、
A より単純ではないと分かったので推奨を改めた。

### Q2 横長ページ・表紙・強制単独ページをまたぐとき — **決定: 推奨どおり (単独で表示)**

1 ページ送りで表示の先頭を `p` にしたとき、`p` と次のページが普段の規則で組めない (次が横長・動画・本の末尾、
`p` が表紙の単独・表紙の次の強制単独など) なら、**普段と同じ規則で `p` を単独で表示する** (推奨)。
別案は「組めるページまで飛ばす」だが、1 ページずつ見たいという目的に反し、実装も増える。

### Q3 1 ページ送りを受け付けない場面

**決定 (利用者、2026-11-16): 本の構成を保存している本も対象にする。コレクションの最上位は受け付けない。**
(当初の推奨は「既存の調整と同じ受付規則で構成ありを拒否」だったが、利用者は +0.5 日の案を選んだ。)

- **本の構成を保存している本**: 受け付ける。構成の単独指定 (`PageOverride::Alone`)・白紙対向 (`PageOverride::BlankOpposite`) は
  組めない境界として扱い (Q2 と同じく、そのページは単独)、構成も既存の調整 (構成ありでは休眠) も書き換えない。詳細は §3.5。
- **本の構成を読み込めない本** (`PairingEditAvailability::Unreadable`) と**抽出結果の表示** (`ExtractedResult`): 受け付けない
  (組の正本が無い / 既存の nudge も拒否するため)。
- **コレクションの最上位**: 普段の送りがコレクション専用の経路に切り替わり、「次の表示単位へ進む」として扱われるため、
  1 ページだけずらす意味を保てない (r1 レビュー P1)。受け付けず、何もしない (メッセージは既存の受付拒否と同じ形)。

### Q4 その他 — **決定: 推奨どおり**

- ページめくりの演出: 1 ページ送りは**演出しない** (横長分割の片側移動と同じ扱い)。A ではその後の普段送りは新しい組での普通の送りなので、
  今までどおり演出する。
- 演出中に押した 1 ページ送り: 既存の「1 ページずらし」と同じく、1 回だけ持ち越して演出の後に行う (既存の保留の型に 1 種類足す)。
- 見開き以外の表示 (単ページ・横長分割) と連結読みでは、**普段の「次 / 前のページ」操作 (`FsPageNext` / `FsPagePrev`) とまったく同じ**動き
  (横長分割なら片側ずつ、端の巡回も普段の設定どおり)。既存の「1 ページずらし」の単ページでの代替 (`Delta`) とは別の経路にする
  (r2 レビュー P2: 既存の代替は分割の片側を飛ばし、連結読みでは調整を書き換えるため、写さない)。
- 見開き (ページ単位) の本の端: 先頭より前・末尾より後へは進まない (何もしない。次の本へは移らない。巡回の設定も使わない。既存の調整と同じ)。
- 通知・画面上の表示 (HUD の「調整あり」など): 出さない。
- リング (右ドラッグ)・マウスボタン・ジェスチャ・ゲームパッドの割り当て候補にも出す (既定はどれも無し)。

## 3. 状態の置き場所と消える契機 (CLAUDE.md「状態の組み合わせを減らす」、Q1-A の場合)

### 3.1 検討した形

1. **既存の見開き調整 (`ViewerSpreadShift`) の新しい variant にする**: 採らない。調整は保存・HUD・解除通知・記憶 ON/OFF・
   構成ありでの休眠など規則が多く、「書き換えない・通知しない」を variant ごとの例外で作ることになる。
2. **今の表示単位だけに効く一時状態 (Q1-B)**: 表示用と送り用の 2 つの列が要る (§2 Q1)。
3. **見開き調整より優先する、保存しない起点を 1 つ持つ (Q1-A、推奨)**: 採る。表示単位の列は 1 つのまま、起点の出どころが
   「保存しない一時の起点」があればそれ、なければ既存の調整、になるだけ。

### 3.2 状態

viewer context ごと (`ViewerContextBundle`、`spread_shift` と同じ所有) に 1 つ:

```text
spread_step: Option<SpreadStep {
    start_idx,   // 一時の起点の item index (読み順でこのページから組み直す)。items 世代の中でだけ意味を持つ
}>
```

- **作る / 置き換える**: 1 ページ送りの操作が受理されたとき (既存の「1 ページずらし」で `set_live_spread_shift` を呼ぶ位置。
  そのあと同じく `FsPageNav::Target` で着地する)。既存の調整の setter が行っている表示単位の持ち主の失効
  (`invalidate_fullscreen_display_unit_owners` など) は同じく行い、保存・通知・調整の書き換えだけを行わない。
  「本来の組と同じ結果なら None に戻す」正規化は行わない (A では起点が以降の組全体に効くので、比較の範囲を決める手間に見合わない。
  None でなくても組は本来と同じになるだけ)。
- **消える**: 1 つの退役関数 (仮名 `retire_spread_step`) を、組の意味が変わる**受理済みの切替**から呼ぶ。値を見比べて推測しない
  (r2 レビュー P2: 単ページ→見開きと戻すと、組を作る関数を通らずに値が元に戻り、比較では切替を見落とす)。呼ぶ場所:
  1. 既存の調整の解除 `reset_live_spread_shift` と設定 `set_live_spread_shift` (見開き方式・同じ方式の再押下・連結方式・読む向き・
     ポップアップ・HUD の解除・既存の 1 ページずらしは、すべてこの 2 つを通る)。
  2. items 世代の変更。App の `set_items_generation` (src/app.rs ~45628) と、表示していない context の世代を進める
     `ViewerContextBundle` 側の setter (src/app/viewer_context_registry.rs ~2923) の両方 (本の切替・項目の増減・ZIP の階層切替・
     スナップショットの再開・コレクションから本へ入る・構成の行の差し替え)。
  3. 編集ツールの一時的な単ページ化 (`enter_page_edit_single_view` 系の共通の入口)。
  4. 本の構成の作成・編集・解除で、組の出どころが受理されて切り替わるとき (準備中・Automatic のまま受理されたときは呼ばない)。
  5. フルスクリーンを閉じたとき。新しい context を作る fork。
  6. 1 を通らない、同じ世代の中での受理済みの採用 (r3 レビュー P2):
     - 構成ありの本の見開き方式だけの変更 (src/ui_fullscreen.rs ~46264) と連結方式の変更 (~46305)。
     - 取り込み (import) で調整を差し替える経路 (src/app/spread_shift_owner.rs ~421)。
     - 保存済み調整の読み直し (retained readoption) で受理された状態を差し替える分岐 (同 ~653)。読み取りの期限切れ・失敗で
       状態が変わらない分岐では呼ばない。
     - 本に入ったときの方向・設定の採用 (src/app.rs ~29532)、PDF の文書の向きの採用 (~42856)。
     - 取り込みで本の表示方式・向き・連結方式・単独ページ設定を差し替える経路 (src/app.rs ~50707)。
     - 単独ページ (表紙の次・最終) の設定の変更: 本ごと (src/ui_fullscreen.rs ~45643) と全体 (環境設定 src/ui_dialogs/preferences.rs ~3056、
       表示していない context へ配る src/app/viewer_context_registry.rs ~5431)。
     - 読む順番 (並び) の受理済みの変更で items 世代が変わらないもの (詳細表示の並べ替え src/app.rs ~76542)。
  - 呼ぶのは受理・成功の確認の後だけ。拒否・失敗・権限の確認・準備・普段の送り・キャッシュの参照・同じ順番の作り直しでは呼ばない。
    読む側 (表示単位の列を作る関数・回転読み直し中の再利用・演出の束縛・最終の要求の導出・スライドショーの nav 選び) は退役させない。
  - 実装担当は上の一覧を起点に、組の意味を変える受理点がほかに無いかを列挙して報告する (行番号は master `e3de9b547` 時点)。
- 同じ本の中で一覧から開き直す・シーク・しおり・履歴で移っても、組の入力が同じなら続く (既存の「この閲覧中のみ」の調整と同じ)。
- 別ウィンドウ: 状態は context ごと。mount / swap / park では bundle と一緒に移る。新しい context を作る fork
  (`split_materialized_physical_context_for_detached_scope`) では持ち越さず、fork 先のキャッシュに一時の列を残さない。
  F12 の表示先切替 (同じ context のまま) では保たれる。
- 保存しない。設定移行・メタデータ移行・スナップショットの永続化に含めない。

### 3.3 表示単位の列への入れ方

- 列を作る `build_spread_display_units_with_overrides` は、起点を「その位置から組み直す」意味で受け取る**別の入力** (仮に `exact_start`) を持つ。
  既存の調整の起点 (偶奇で揃える `spread_shift_anchor_pos_for_target` の結果) とは別。両方あるときは一時の起点を使う
  (既存の調整は基礎として保持し、書き換えない)。
- `exact_start` の規則 (r1 レビュー P2 の 2 例を含む):
  - 表紙あり (強制単独の設定なし) で起点が表紙の位置なら、表紙は単独のまま (`[1] [2,3]` から戻る操作で `[1,2]` を作らない)。
  - 強制単独 (表紙の次・最終) の分岐では、既存どおり本文区間の中だけで起点を受け付ける。
  - 横長・動画・構成の上書きは、既存のペア可否の判定 (`append_spread_display_units_until`) をそのまま使う。
  - 起点の手前で余った 1 ページは単独 (既存の `UnpairedSlot`)。
- キャッシュの token (`SpreadDisplayUnitsToken`) に一時の起点を入れる。`FinalDemandTopologies::same_visible`、
  `final_demand_visible_inputs_are_current`、回転読み直し中の単位の再利用、逆引きの index も同じ token で揃える。
- 列を作る入力の依存順 (循環させない): (1) 表示中の nav と既存の調整の起点 (`spread_shift_anchor_idx()` は既存の調整だけを返し、
  一時の起点を混ぜない) → (2) `spread_step` の `start_idx` を `exact_start` として渡す → (3) 列を作る。
- スライドショー (静止画だけの nav) や最終の要求の導出 (`derive_final_demand_topology`) など、別の nav で列を作る読み手にも
  同じ `exact_start` (item index) を渡す。その nav に `start_idx` が無ければ、その導出では起点なしとして扱う (状態は変えない)。
  読み手ごとの nav の違いは切替ではないので、退役させない (r2 レビュー P2)。

### 3.4 1 ページ送りの行き先

- 今の表示単位の先頭の nav 位置を `s` として、進むなら `s+1`、戻るなら `s-1` を起点に列を作り直し、その位置を含む単位の先頭へ
  `FsPageNav::Target` で着地する (既存の nudge と同じ流れ。位置は item の番号ではなく nav の位置で数える)。
- 端 (`s+1` が末尾を越える / `s-1` が先頭より前): 何もしない。巡回 (`fullscreen_manual_nav_wrap`) は使わない。
- 受付の順 (r2 レビュー P2): (1) 見開きでない・ページ単位でない (連結読み) → 普段の「次 / 前のページ」と同じ解決 (`spread_page_nav(±1)`) で終わり。
  (2) コレクションの最上位 (表示中の context の collection session の `position == Root`) → 何もしない。
  (3) 1 ページ送り専用の受付 (`admit_pairing_action` は使わない。既存の調整の受付は変えない): `Automatic` と `Composed` は受け付け、
  `Unreadable` と `ExtractedResult` は警告で拒否 (文言は「1 ページ送り」に合わせた新しいもの)。(4) 抽出結果の表示面では何もしない (既存の nudge と同じ)。
  (5) 行き先を解いて `spread_step` を設定し `Target`。拒否はどれも状態を変える前・保留を積む前に行う。
- 送り・演出・先読み・保留はすべて「今の列」で既存のまま働く (列が 1 つなので、送り用と表示用の食い違いが起きない)。

### 3.5 本の構成を保存している本 (Q3 の決定)

- 構成ありの本では、既存の調整の起点は休眠 (`ViewerSpreadShift::anchor_idx()` が `None`) で、表示単位の列は構成の上書き
  (`composed_book_layout().page_overrides`)・構成の読む向き・表紙の扱いから作られる。`exact_start` はこの列の作り方にそのまま加える
  (既存の調整の起点は渡らないので、優先順位の問題は起きない)。
- `exact_start` が単独指定・白紙対向のページに当たれば、そのページは単独 (白紙対向は既存どおり白紙を添えた表示)。
  起点の手前で余ったページは単独。起点より後は、構成の上書きを既存のペア可否の判定のまま守る。
- 構成の読む向き・表紙は書き換えない。構成ありの本で受け付ける見開きキーの制限 (構成に一致するキーだけ) も変えない
  (1 ページ送りは見開き方式を変えないので、制限に触れない)。
- 構成の作成・編集・解除で組の出どころが受理されて切り替わったら退役する (§3.2 の 4)。構成ありの本の見開き方式だけの変更・
  連結方式の変更も退役する (§3.2 の 6)。
- 表示許可の確認 (`book_composition_permission`) は、既存の nudge と同じく起点を先に入れてから `Target` で送るので、新しい列の
  表示単位で確認される。確認待ちの間も起点は context に残り、確認後の着地は通常の継続 (ページ番号だけを運ぶ既存の経路) で足りる
  見込み (r2 レビューの見立て: 「列の状態は確認の前から context にあり、通常の継続をまたいで残る」)。実装時に、構成ありの本で
  確認待ちを挟む経路をテストで確かめる。
- 構成の編集画面 (本の構成のエディタ) は別の画面で、1 ページ送りの対象外。

## 4. 周辺との関係

- **ページめくりの演出**: 1 ページ送りの着地は演出から外す (専用の除外理由を足す。重なった組の紙めくりは対応する絵が無い)。
  演出中の入力は、既存の `ResolvedNavigationIntent::SpreadShift { anchor, topology }` と同じ形の型を 1 つ足し (`SpreadStep`)、
  列の Arc が同じときだけ着地後に一度適用する (既存の保留の予算 1 件のまま。解き直さない)。
- **表示許可の確認待ち**: 既存の nudge と同じく、起点を先に入れてから `Target` で送るので、確認は新しい列で行われる
  (r1 で問題になった「着地後に設定する」形をやめた)。
- **表紙・横長の単独ページ・強制単独**: Q2。
- **本の構成**: Q3 の決定で受け付ける (§3.5)。構成も既存の調整も書き換えない。
- **見開き調整 (Ctrl+←/→ 等)**: 一時の起点があるときに見開き調整を操作すると、調整の入力が変わるので一時の起点は退役し、
  調整は既存どおり (保存・通知も既存のまま) 働く。調整の HUD の「解除」も同じ。
- **連結読み**: 1 ページ送りは普段の「次 / 前のページ」と同じになり、一時の起点は作らない。
- **別ウィンドウ・複数ウィンドウ**: §3.2。入力は前面の context にだけ届き、兄弟 context の列・キャッシュ・調整を変えない。
  detached の述語・viewport の経路は変えない。bundle へのフィールド追加は既存の所有に沿った構造変更として双方のレビューで確認し、
  `docs/detached-rework-plan.md` §11 に記録する。`tools/viewer_context_audit` は bundle のフィールドを自動で拾う (fingerprint の更新が要る場合だけ直す)。
- **mIV Remote**: Remote の見開きは独自の page_groups で作り、PC の viewer-local state を借りない (`docs/web-remote-plan.md`)。今回は PC 側だけ。
- **keymap**: `KeyAction` を 2 つ足す (仮名 `FsSpreadStepPrev` / `FsSpreadStepNext`、文言「見開きのまま 1 ページ戻る / 進む」)。
  `ALL_ACTIONS`・`ini_name`・`description`・`context` (FsImage)・`trigger` (Press)・`default_chords` (空)・
  `docs/keymap.ini.default` (生成物、テストで一致検査)・`consume_page_turn_action` での受理と操作 ID・
  リング (`RingActionId`、名前の保存形式・使える画面、環境設定の対応表の両方向、分類テスト、`docs/ring-keyaction-parity.md`)・
  マウスボタン / ジェスチャ / ゲームパッドの割り当てからの共通の実行経路・`FS_PAGE_TURN_COALESCE_ACTIONS` (押しっぱなしの分類。
  1 frame 1 回の既存のまとめ方を保つ)・検索ナビゲーションの入口一覧テスト (`search_nav_entry_inventory_tests.rs`) の分類。
  読み順固定 (右→左でも「進む」は次のページ方向)。`docs/keymap-spec.md` に行を足す。
- **マニュアル**: `fullscreen.html` の見開きの節と `shortcuts.html` に 2 操作を足す (既定キーなし、割り当て方、Q1 の戻り方)。製品ページは変えない。
- **前提**: §1.747 (横長分割の開き直し) の修正は別ブランチ (`fix-745-747-page-view`) で、master には未取り込み。本設計は依存しない。

## 5. テスト (実装時)

- 状態遷移: `[1,2]` → 進む → `[2,3]` → 普段の次へ → `[4,5]` (Q1-A)。戻る側、連続 2 回 (本来の組と同じ結果に戻っても状態は残してよい。組が本来と同じになることを確かめる)、
  端の no-op、横長・表紙・強制単独の境界 (Q2、r1 レビューの 2 例)、右→左、既存の調整がある本の上での一時の起点。
- 退役: 見開き方式を単ページにして戻す (演出 OFF・先読み OFF でも)・読む向き・連結方式・既存の調整の操作・項目の増減・本の切替・
  編集ツールの単ページ化から戻る・閉じる、のあとで復活しない。回転読み直し中の再利用で古い列が出ない。
- 静止画と動画が混ざったフォルダのスライドショーで、一時の起点が退役しない (`1,2,3,動画,4,5` で `[2,3]` から次の送り)。
- 見開き以外: 単ページ・横長分割 (片側ずつ、端の巡回は普段の設定どおり)・連結読みで普段の次 / 前と同じ。
- コレクション: 最上位 (メインと別ウィンドウ) の見開きでは何もしない (単ページ・横長分割・連結読みでは受付順 (1) により普段の次 / 前になる)。コレクションから開いた本の中では使える。
- 同じ世代の中での採用: 取り込み・保存済み調整の読み直し・PDF の向きの採用・単独ページ設定の変更 (表示中と表示していない context の両方)・詳細表示の並べ替えで退役する。読み直しの失敗では残る。
  同じ本の中のシーク・しおりでは続く。
- 見開き調整を書かない: 記憶 ON の本で 1 ページ送り → `spread_shift.db` への書き込みが無く、`ViewerSpreadShift` が不変、通知なし。
- 構成ありの本: 単独指定・白紙対向をまたぐ 1 ページ送り (そのページは単独)、構成の読む向き・表紙、表示許可の確認待ちを挟む着地、構成の編集・解除と見開き方式だけの変更での退役、構成と既存の調整 (休眠中の保存値) が書き換わらないこと。
  構成ありの組み合わせは、末尾の単独指定 (最終表紙の添えが付き得る) と白紙対向 (白紙を添え、最終表紙の添えは付かない) の違い、
  右→左・表紙・強制単独との組み合わせ、本当に確認待ちを挟む着地と、待ちの間に構成の出どころが切り替わる場合 (退役する) を分けて確かめる
  (r4 レビュー、ACCEPT 時の注記)。
- 受け付けない場面: 構成を読み込めない本・抽出結果 (警告)、コレクションの最上位。見開き以外では普段の 1 ページ送り。
- 兄弟 context 不変・fork で持ち越さない・F12 の表示先切替で保たれる。
- 演出: 1 ページ送りで演出が始まらない。演出中の 1 ページ送りが着地後に一度だけ適用される (列が変わっていれば捨てる)。
- keymap: 既定キーなし、ini の生成物一致、リング分類テスト、入口一覧テスト。

## 6. 規模の見積もり

- Q1-A: 1 ページずらしの経路の複製と、一時の起点・`exact_start`・退役 1 か所・keymap 一式・演出の除外と保留の型 1 つ・テスト・文書。
  Codex 実装 1 回 + 独立レビュー 2〜3 往復 + 全件テスト + サブ PC 確認 1 回で、**利用者の回答から約 1.5〜2 日** (退役を呼ぶ受理点 約 15 か所とそのテストを含む)。
- Q1-B: 上に加えて、表示用と送り用の列の分離・保留中と確認待ちの意図の運搬・一時表示からの送りの解決。**約 2.5〜3 日**。
- Q3 の決定 (構成ありの本も対象) で +0.5 日程度。**決定した形 (A + 構成ありも対象) で約 2〜2.5 日。**

## 7. 気づいた既存の点 (別件の候補、コード読みのみ・未確認)

- 既存の「1 ページずらし」もコレクションの最上位では `Target` がコレクション専用の送りに乗り換わるため、
  ずらした意味どおりに着地しない可能性がある (r1 レビュー P1 と同じ経路)。実機での観測は無い。

## 8. 実装・検証記録（2026-11-17）

### 所有・読み手

- `ViewerContextBundle` と App に `spread_step` を追加。empty・destructuring・swap・park/mount は同じ owner に保持する。
  fork 先では退役し、元 context は保持する。一時起点がある fork の複製表示単位 cache は空にする。F12 の同 context 切替は保持する。
  保存対象には追加していない。detached 固有の guard/predicate/viewport 分岐は追加せず、§11 に構造変更を記録した。
- `build_spread_display_units_with_overrides` と `build_display_unit_topology` は live shift と別の `exact_start` を受ける。
  nav にあれば優先し、表紙単独と強制単独の本文境界、構成上書きの既存ペア判定を保つ。nav に無ければその導出だけ起点なしにする。
- 表示 cache の get/install/token、逆引き index、`FinalDemandTopologies::same_visible`、
  `final_demand_visible_inputs_are_current` と snapshot の入力照合、`derive_final_demand_topology`、
  回転 reload 再利用、静止画 slideshow nav、facet 外の単独ページ、構成作成時の現行列取得へ同じ起点を渡す。
  Remote の独立した列と PC viewer-local 状態を持たない既存 pure helper は `None` を渡す。
- step resolver は現行 unit の `nav_start` から ±1 を解決する。非見開き/連結の通常送り代替、mounted collection Root の拒否、
  step 専用の構成受付、抽出表示面の no-op の順を維持する。`admit_pairing_action` の条件・既存調整の受付は変更しない。
- 即時着地は `stage1_spread_step` で curl を除外する。barrier 中は `ResolvedNavigationIntent::SpreadStep { anchor, topology }` を
  既存の 1 件予算へ積み、enqueue では起点を変更しない。従来の binding/generation/index 検査に加え topology Arc 一致で一度だけ適用する。
- `FsSpreadStepPrev/Next` と `ImageSpreadStepPrev/Next` は既定割当なし。キーの frame coalescing と操作 ID を維持し、
  ring/マウスボタン/ジェスチャ/ゲームパッドは同じ image action dispatcher から step handler を使う。

| 読み手 | `exact_start` の渡し方 |
| --- | --- |
| 通常の unit builder / cache | `spread_step_start_idx()` を builder と get/install の別引数にし、token へ記録 |
| unit 逆引き | cache install が exact 適用済みの同じ units から位置 index を作る |
| `FinalDemandTopologies::same_visible` | 保存した visible token と現 token の exact 値も比較 |
| 最終要求の入力照合 / snapshot 読み出し | token の exact と現在の getter を比較し、古い証明を使わない |
| `derive_final_demand_topology` | 保存した visible token の exact を通常/静止画の各 nav の builder へ渡す |
| 回転 reload 再利用 | getter を再利用判定の別引数に渡し、token の exact と一致を確認 |
| 静止画だけの slideshow | 同じ builder / final-demand 導出を使い、起点が nav に無い場合だけその導出で無視。状態は保持 |
| facet 外の単独ページ / 構成作成時の現行列 / uncached test helper | 同じ getter を overrides builder へ渡す |
| 既存 nudge | step があれば現行の有効列を参照し、無ければ従来の計算を保持。受理済み live setter で退役 |
| Remote / PC viewer-local 状態を持たない pure helper | `None` を渡す |

### 実際の退役点

free function `retire_spread_step` の実呼び出しは次の表。
Details の比較 helper は同じ順序なら呼ばず、`rebuild_details_order` の各成功出口と grid view runtime の採用後から使う。
PDF の文書方向の採用も `apply_spread_for_key_with_document_direction` を通る。

| ファイル:行 | 受理点 |
| --- | --- |
| `src/app/spread_shift_owner.rs:433` | `adopt_imported_shift_read` |
| `src/app/spread_shift_owner.rs:666` | `poll_book_shift_restore` |
| `src/app/spread_shift_owner.rs:849` | `set_live_spread_shift` |
| `src/app/spread_shift_owner.rs:893` | `reset_live_spread_shift` |
| `src/app.rs:13296` | `enter_page_edit_single_view` |
| `src/app.rs:29574` | `apply_spread_for_key_with_document_direction` |
| `src/app.rs:29591` | `apply_spread_for_key_with_document_direction` |
| `src/app.rs:45660` | `set_items_generation` |
| `src/app.rs:50785` | `apply_current_metadata_import_terminal_result` |
| `src/app.rs:76654` | `retire_spread_step_after_reader_order_adoption` |
| `src/app.rs:83022` | `close_fullscreen_now_for_destination` |
| `src/app/viewer_context_registry.rs:3078` | `set_items_generation` |
| `src/app/viewer_context_registry.rs:4654` | `split_current_context_preserving_main_grid` |
| `src/app/viewer_context_registry.rs:5623` | `invalidate_page_alone_in_parked_contexts` |
| `src/app/book_composition_runtime.rs:801` | `accept_unsupported_archive_composition` |
| `src/app/book_composition_runtime.rs:1250` | `adopt_book_projection` |
| `src/app/book_composition_runtime.rs:1302` | `adopt_book_projection` |
| `src/ui_fullscreen.rs:45757` | `set_page_alone_preferences_for_fullscreen` |
| `src/ui_fullscreen.rs:46380` | `apply_fullscreen_view_mode` |
| `src/ui_fullscreen.rs:46422` | `set_reading_flow_for_fullscreen` |
| `src/ui_dialogs/preferences.rs:3074` | `apply_preferences_dialog` |

Details の順序比較 helper を呼ぶ採用点:

| ファイル:行 | 採用点 |
| --- | --- |
| `src/app.rs:76407` | `apply_grid_view_mode_runtime` |
| `src/app.rs:76625` | `rebuild_details_order` |
| `src/app.rs:76634` | `rebuild_details_order` |
| `src/app.rs:76645` | `rebuild_details_order` |

追加で確認した受理点は、(1) 詳細表示↔サムネイルの切替（お気に入り設定の採用経路も含む）で実際の読む順が変わる場合、
(2) 同世代の構成 refresh が Composed から Unsupported を採用して組の出どころを切り替える場合。
後者は projection の採用と書庫の Unsupported 採用 helper の両入口を接続した。
通常送り、同じ本のシーク/開き直し、同順序の Details 再構築、準備/確認待ち/拒否/失敗、cache 参照からは退役しない。

### Tests first と検証結果

- 新規テストを先に追加し、未実装 API の 107 コンパイルエラーを確認した（`target/step-red-api.log`）。
- 最小 no-op API のみを追加してから実行し、40 件中 36 失敗・4 成功を確認した（`target/step-red-behavior.log`）。
  4 成功は端の no-op・入力候補の metadata・fork・F12 の保持で、機能実装前でも満たせる部分。
- 初回 test discovery の `0xc0000135` は新 worktree の FFmpeg DLL 不在。指示どおり vendor の 6 DLL を `target/debug/deps` へコピーした。
  turbojpeg の初回 CMake 構築は single-job で成功し、以後の heavy job も共有 runner 下で `CARGO_BUILD_JOBS=1` とした。
- 実装後の新規 40 件はすべて成功。途中の 38 成功・2 失敗は、演出 OFF の fixture と保存先に一致しない構成 token を訂正して解消した。
  関連 focused は新規を含む 742 件すべて成功（4 slow、166.762 秒）。既定 keymap は専用テストの
  `UPDATE_KEYMAP_DEFAULT=1` で再生成し、通常の一致検査も成功した（`target/step-validation-3.log`）。
- 最初の context 監査は A3 2 件・A4 2 件を検出した。bundle 生成を含む 2 テストを registry 本体の test module へ移し、
  r5 が承認したフィールド分だけ exact fingerprint を更新した。監査ルール・例外は追加していない。
  監査ツール focused 9 件と実際の `cargo run --locked -p viewer_context_audit` が成功した。
- その後、新 handler の構成受付を通常の移動待ち guard より先へ揃え、移動待ち中の拒否も補強した。
  最新ソースで新規 40 件すべてを再確認した（17.292 秒、`target/step-validation-4.log`）。
  既存経路の 742 件の成功結果を引き継ぎ、最終 follow-up はこの受付順・監査・テスト配置の影響範囲を確認した。
- `cargo fmt`（引数なし）、`cargo fmt --check`、`python scripts/check_ui_glyphs.py`（危険な文字 0）、`git diff --check` は成功した。
  確認用 `build-dev.ps1 -PreserveRuntime` も成功した（共有 runner 記録 `exit=0`）。
  `target/dev-runtime` に normal feature set の core・remote service・EPUB worker・MCP adapter・music-video editor と DLL を配置済み。
  ログは `target/step-validation-4.log`。core の初回 cold build は 28 分 01 秒で、成果物は起動していない。
  外側の PowerShell stderr 捕捉は cargo の出力を `NativeCommandError` にして tool 終了値 1 を返すが、
  内側 runner は `exit=0`、全 Cargo stage は成功、`[build-dev] DONE` と 5 exe の存在・更新時刻を確認した。
  exit 0 の stderr 出力だけでも同じ外側終了値になることを `target/stderr-capture-probe.log` で確認した。
- full lib/test-full は担当範囲外として実行していない。製品バイナリの起動・コミットはしていない。

新規 40 件の名前と最小 API 時点→実装後の結果:
最小 API の列は最初の実行時点の結果。実装途中で演出有効化と構成 token の fixture を訂正し、
末尾ペアの境界、live shift と異なる exact 起点、書庫の Unsupported 別入口、移動待ち中の受付順を同じテスト内へ補強した。

| テスト | 最小 API | 実装後 |
| --- | --- | --- |
| [`spread_step_persists_through_ordinary_navigation_and_repeated_steps`](../src/ui_fullscreen/spread_step_tests.rs#L37) | 失敗 | 成功 |
| [`spread_step_ends_do_not_wrap_or_change_state`](../src/ui_fullscreen/spread_step_tests.rs#L63) | 成功 | 成功 |
| [`spread_step_uses_nav_positions_and_effective_unit_start`](../src/ui_fullscreen/spread_step_tests.rs#L81) | 失敗 | 成功 |
| [`spread_step_exact_start_preserves_cover_and_forced_body_boundaries`](../src/ui_fullscreen/spread_step_tests.rs#L95) | 失敗 | 成功 |
| [`spread_step_landscape_and_media_boundaries_keep_singletons`](../src/ui_fullscreen/spread_step_tests.rs#L116) | 失敗 | 成功 |
| [`spread_step_does_not_write_live_shift_or_notify_with_memory_on`](../src/ui_fullscreen/spread_step_tests.rs#L128) | 失敗 | 成功 |
| [`spread_step_nonspread_and_continuous_use_ordinary_page_resolution`](../src/ui_fullscreen/spread_step_tests.rs#L150) | 失敗 | 成功 |
| [`spread_step_refused_sources_leave_state_and_topology_unchanged`](../src/ui_fullscreen/spread_step_tests.rs#L177) | 失敗 | 成功 |
| [`spread_step_retirement_mode_flow_direction_and_shift_roundtrips`](../src/ui_fullscreen/spread_step_tests.rs#L233) | 失敗 | 成功 |
| [`spread_step_rotation_reuse_and_final_demand_validate_exact_start`](../src/ui_fullscreen/spread_step_tests.rs#L273) | 失敗 | 成功 |
| [`spread_step_still_only_nav_does_not_retire_missing_media_start`](../src/ui_fullscreen/spread_step_tests.rs#L309) | 失敗 | 成功 |
| [`spread_step_keymap_no_defaults_and_coalescing_inventory`](../src/ui_fullscreen/spread_step_tests.rs#L327) | 失敗 | 成功 |
| [`spread_step_composed_directives_direction_cover_and_final_companions`](../src/ui_fullscreen/spread_step_tests.rs#L336) | 失敗 | 成功 |
| [`spread_step_composed_permission_wait_keeps_origin_and_edit_retires`](../src/ui_fullscreen/spread_step_tests.rs#L390) | 失敗 | 成功 |
| [`spread_step_composed_mode_only_and_flow_roundtrips_retire`](../src/ui_fullscreen/spread_step_tests.rs#L468) | 失敗 | 成功 |
| [`spread_step_page_alone_accepted_change_retires_but_failed_change_keeps`](../src/ui_fullscreen/spread_step_tests.rs#L506) | 失敗 | 成功 |
| [`spread_step_details_reorder_retires_only_when_reader_order_changes`](../src/ui_fullscreen/spread_step_tests.rs#L528) | 失敗 | 成功 |
| [`spread_step_details_view_exit_retires_only_when_reader_order_changes`](../src/ui_fullscreen/spread_step_tests.rs#L543) | 失敗 | 成功 |
| [`spread_step_real_key_handler_uses_reading_order_and_coalesces_a_frame`](../src/ui_fullscreen/spread_step_tests.rs#L557) | 失敗 | 成功 |
| [`spread_step_same_book_seek_and_reopen_keep_the_origin`](../src/ui_fullscreen/spread_step_tests.rs#L593) | 失敗 | 成功 |
| [`spread_step_slideshow_final_demand_derives_still_units_without_replacing_visible_proof`](../src/ui_fullscreen/spread_step_tests.rs#L618) | 失敗 | 成功 |
| [`spread_step_context_materialized_fork_omits_step_and_effective_projection`](../src/app/spread_step_context_tests.rs#L20) | 成功 | 成功 |
| [`spread_step_context_parked_page_alone_change_retires_only_inheriting_context`](../src/app/spread_step_context_tests.rs#L44) | 失敗 | 成功 |
| [`spread_step_context_collection_root_refused_but_opened_book_accepted_on_both_surfaces`](../src/app/spread_step_context_tests.rs#L69) | 失敗 | 成功 |
| [`spread_step_context_edit_and_close_retire_without_touching_sibling`](../src/app/spread_step_context_tests.rs#L120) | 失敗 | 成功 |
| [`spread_step_context_same_generation_book_and_pdf_rule_adoption_retires_after_success`](../src/app/spread_step_context_tests.rs#L144) | 失敗 | 成功 |
| [`spread_step_context_f12_real_presentation_switch_keeps_origin`](../src/app/spread_step_context_tests.rs#L173) | 成功 | 成功 |
| [`spread_step_context_imported_container_rules_retire_after_current_generation_adoption_only`](../src/app/spread_step_context_tests.rs#L192) | 失敗 | 成功 |
| [`spread_step_context_swap_park_mount_sibling_and_presentation_ownership`](../src/app/viewer_context_registry.rs#L6380) | 失敗 | 成功 |
| [`spread_step_context_both_generation_setters_retire_only_changed_owner`](../src/app/viewer_context_registry.rs#L6413) | 失敗 | 成功 |
| [`spread_step_ring_ids_roundtrip_and_assignable_only_to_images`](../src/app/spread_step_input_tests.rs#L5) | 成功 | 成功 |
| [`spread_step_ring_and_assigned_mouse_use_the_same_execution`](../src/app/spread_step_input_tests.rs#L32) | 失敗 | 成功 |
| [`spread_step_projection_automatic_reacceptance_keeps_but_source_replacement_retires`](../src/app/spread_step_projection_tests.rs#L70) | 失敗 | 成功 |
| [`spread_step_projection_source_change_during_real_permission_wait_cannot_revive_step`](../src/app/spread_step_projection_tests.rs#L127) | 失敗 | 成功 |
| [`spread_step_memory_on_never_admits_a_shift_write_or_toast`](../src/app/spread_shift_composition_tests.rs#L11) | 失敗 | 成功 |
| [`spread_step_retained_and_import_adoption_retire_after_success_only`](../src/app/spread_shift_composition_tests.rs#L37) | 失敗 | 成功 |
| [`spread_step_global_page_alone_apply_retires_mounted_and_inheriting_parked_only`](../src/ui_dialogs/preferences/spread_step_tests.rs#L27) | 失敗 | 成功 |
| [`spread_step_key_rows_map_both_directions_to_image_assignments`](../src/ui_dialogs/preferences/pages.rs#L13239) | 失敗 | 成功 |
| [`spread_step_barrier_defers_without_mutation_and_commits_once`](../src/ui_fullscreen/page_turn/layout_epoch_tests.rs#L882) | 失敗 | 成功 |
| [`spread_step_immediate_landing_has_its_own_curl_exclusion`](../src/ui_fullscreen/page_turn/layout_epoch_tests.rs#L916) | 失敗 | 成功 |

### 変更ファイル

- `docs/README.md`
- `docs/detached-rework-plan.md`
- `docs/display-pipeline.md`
- `docs/key-customization-impl-plan.md`
- `docs/keymap-spec.md`
- `docs/keymap.ini.default`
- `docs/one-page-step-plan.md`
- `docs/ring-keyaction-parity.md`
- `htdocs/mimageviewer/manual/fullscreen.html`
- `htdocs/mimageviewer/manual/shortcuts.html`
- `src/app.rs`
- `src/app/book_composition_runtime.rs`
- `src/app/gamepad_input.rs`
- `src/app/search_nav_entry_inventory_tests.rs`
- `src/app/spread_shift_composition_tests.rs`
- `src/app/spread_shift_owner.rs`
- `src/app/spread_step_context_tests.rs`
- `src/app/spread_step_input_tests.rs`
- `src/app/spread_step_projection_tests.rs`
- `src/app/tests.rs`
- `src/app/viewer_context_registry.rs`
- `src/keymap.rs`
- `src/ring_shortcut.rs`
- `src/ui_dialogs/preferences.rs`
- `src/ui_dialogs/preferences/pages.rs`
- `src/ui_dialogs/preferences/spread_step_tests.rs`
- `src/ui_fullscreen.rs`
- `src/ui_fullscreen/book_composition_tests.rs`
- `src/ui_fullscreen/next_final_demand_tests.rs`
- `src/ui_fullscreen/page_turn/barrier.rs`
- `src/ui_fullscreen/page_turn/layout_epoch_tests.rs`
- `src/ui_fullscreen/spread_step.rs`
- `src/ui_fullscreen/spread_step_tests.rs`
- `tools/viewer_context_audit/src/lib.rs`

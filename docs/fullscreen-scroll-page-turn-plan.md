# スクロール＋ページ送り・ホイールのズーム・N 字 / Z 字の移動量・滑らかな移動 (§1.823〜§1.826)

- 状態: **独立実装レビュー r5 ACCEPT・関連1559件すべて成功（設計担当報告）。着地総当たりテストの5分割後は未実行**。ブランチ `scroll-823-826`。
- §4.5と§4.4.1の6bに従って着地配送・ホイール・補間を接続し、実装レビュー r1〜r4 の指摘を修正した。r5のP3は、描画が0.2秒以上途切れた後の再入力が瞬時の移動になることを、最終位置・状態は正しいとして記録で閉じた。実装担当はコンパイル・テスト・snapshotのPNG生成・製品起動を行っていない。レビュー経緯と検証結果、テスト分割の記録は末尾を参照。
- 要望: 5ch >>826〜829 (2026-11-20)。利用者決定 (2026-11-20) は [バックログ](next-release-backlog.md) の各項目の状態欄。
- 前提の設計: [ページ内スクロールと N 字スクロール (§1.474)](fullscreen-in-page-scroll-plan.md)。v5.4.0 で出荷済み。
  本書はその仕組み (描いた表示単位の印・入力時の照合・候補 → clamp → 進んだときだけ書く) をそのまま使い、足す分だけを書く。
- 観測の出どころ: 本書の「現状」はすべてコード読み (worktree `scroll-823-826`、master `72fcab869`)。実画面での確認はしていない。

## 0. 4 件の範囲 (利用者決定)

| § | 足すもの | 既定 | 変えないもの |
| --- | --- | --- | --- |
| 1.826 | N 字 / Z 字 (K / J) の 1 回の移動量の設定 | 85% (今と同じ) | 上下キー・ホイールの移動量 (連結読みと共用のまま) |
| 1.825 | 通常のホイールの設定に「ズーム」 | ページ送り (今と同じ) | シークバー上のホイールは設定にかかわらずページ送り |
| 1.824 | 「スクロール＋ページ送り」: 新しい `KeyAction` 2 つ (既定キーなし) と、ホイールの設定の選択肢 | 割り当てなし / ホイールはページ送り | 今の端で止まる操作 (↑↓・ホイールのページ内スクロール・K / J・4 方向) |
| 1.823 | ページ内スクロールと N 字 / Z 字移動を滑らかに動かす設定 | 切 (今と同じ瞬時の移動) | ドラッグ・ズーム・ページを開いたときの配置は瞬時のまま |

既定のままでは 4 件とも今の動作を変えない (新しい経路を通らない)。

## 1. 共通の前提 (§1.474 から引き継ぐ仕組み)

- **面の述語** `fs_in_page_scroll_surface()`: ページを持つ静止画、ページ単位の表示、Z・比較・編集・表示トリム編集・範囲キャプチャでない。
  4 件とも、この面の上でだけ働く。面の外は今の経路のまま。
- **描いた表示単位の印と照合** (`FsDrawnUnitStamp`、`fs_scroll_layout_matches`): 前 frame の layout が今の表示単位のもので、幾何の入力も
  倍率も変わっていないときだけ、入力時に `fs_pan` を書く。合わなければ入力を消費して何もしない。
- **候補 → clamp → 進んだときだけ書く** (`scroll_pan` / `ordered_scroll_pan`): 端は「進まなかった」ことで分かる。可視性の領域は作らない。
- **書き込み口** `write_fs_page_scroll_pan`: `fs_zoom` を描いた実効の倍率に揃え、`set_fs_pan_from_input` を通す (§1.726 の初期配置要求も消える)。
- 新しい実行時状態を足すときは context 所有にし、viewer context の swap に載せる (§9)。

## 2. §1.826 N 字 / Z 字の移動量

- 設定 `fullscreen_n_scroll_step_percent: u32` (表示領域の幅 / 高さに対する %)。既定 **85**、範囲 **10〜100**。
  範囲外は読み込み時に clamp (既存の `continuous_reading_*_percent` と同じ扱い)。serde default。転送は同じ分類 (`(10..=100).contains`)。
  未リリースの新設定なので migration は不要。
- 効く場所は `ordered_scroll_pan` の主軸の移動と折り返しの移動 (今の `VERTICAL_READING_PAGE_SCROLL_FRAC` を使っている 2 か所) だけ。
  連結読みの PageUp / PageDown は今の 85% の定数のまま (範囲は N 字 / Z 字だけという利用者決定)。§1.824 のキーもこの量を使う (§4.2)。
- 小さい量 (例 10%) でも、折り返しは「副軸が読む向きへ進んだときだけ採る」既存規則のままなので、同じ列・段を繰り返さず、端で止まる。
  100% は重なりなし。重なりの意味は「1 回の移動 = 表示領域 × 量。残りの (100 − 量)% が前の画面と重なる」。
- 環境設定: 「閲覧表示」の「スクロール量」の欄に 1 行 (「順に読む操作 (N 字 / Z 字) 1 回」、`DragValue`、`%`)。見出しは
  「スクロール量 (連結読み・ページ内スクロール)」のまま、N 字 / Z 字の行だけ「ページ内を順に読む操作だけに効きます」と添える。検索 anchor を足す。
- 「…」(その他の機能) の読み順の小パネルには足さない (設定を変える場所を増やさない)。

## 3. §1.825 通常のホイールの「ズーム」

### 3.1 設定

`FullscreenWheelAction` に `Zoom` と `ScrollThenPageTurn` (§4) を足す。v5.4.0 で出荷済みの列挙なので、**既存の値の綴り (`PageTurn` / `InPageScroll`) と既定は変えない**。
新しい値を古い版が読むと `#[serde(other)] Unknown` → 正規化でページ送りになる。旧版で開いても設定が保存されなければ、保存済みの値は残り、新しい版で開き直すと元の選択へ戻る。
旧版で環境設定が保存されると (自動保存を含む)「ページ送り」が書き戻され、新しい版で開き直してもページ送りのままになる (未知の値を旧版の既定へ正規化する既存の扱い)。
環境設定の転送も `all()` に含めるだけで、未知の値は今と同じく転送しない。

| 値 (表示名) | 修飾なしホイール (画像の上) |
| --- | --- |
| ページ送り (既定) | 今のまま |
| ページ内スクロール | 今のまま (§1.474) |
| スクロール＋ページ送り | §4 |
| ズーム | Ctrl+ホイールと同じズーム |

説明の 1 行 (`ui.small`) は選んだ値ごとに出し分ける (今の「端で止まり、次のページへは送りません」はページ内スクロールの説明になっている)。

### 3.2 分類の拡張 (1 か所で決める)

§1.474 §2.6 の `fs_wheel_segment_in_page_scroll(...) -> FsWheelInPageClass` を次の 4 つに広げる。入力の所有・場所・面の述語・設定・修飾から
**1 か所**で決め、受付の閾値・`fs_wheel_action_requests_navigation`・適用・クリックの予測は同じ答えを使う。

```text
InPageScroll(delta)        今のまま
ScrollThenPageTurn(delta)  §4.3
SettingZoom                ズーム。受付・適用は Ctrl+ホイールのズームと同じ
Legacy                     今のまま (既存の動的な適用先へ委ねる)
```

- 設定の 3 値 (ページ内スクロール・スクロール＋ページ送り・ズーム) に共通の条件は今の `InPageScroll` と同じ:
  Ctrl・Alt・Command なし、**シークバー上 (ドラッグ中でない hover) でない**、面の上 (または egui の fallback 右クリックメニュー中)。
  外れたら `Legacy` (シークバー上はページ送り、Alt+ホイールもページ送り、Ctrl+ホイールは今のズーム)。
- `SettingZoom` は **Shift なし**のときだけ (Shift+ホイールは egui が縦を横へ移すので今は何もしない。ズームの設定でも何もしないまま = `Legacy`)。
- fallback 右クリックメニュー中は、設定の 3 値とも**消費して何もしない** (§1.474 §2.2 の規則を 3 値に広げる。メニューが持つ入力をズーム・送りへ流さない)。
- 受付の閾値: `SettingZoom` は Ctrl+ホイールと同じ `wheel_y.abs() > 0.5`。

### 3.3 適用

- 適用時に分類をもう一度評価し (今の `InPageScroll` と同じ)、`SettingZoom` のままなら、`apply_fs_wheel_frame_proposal` の既存の
  Ctrl+ホイールのズームの分岐 (`should_zoom_fullscreen_wheel` の側: 初期配置要求を消す・カーソル位置基準の `apply_wheel_zoom`・
  `set_fs_pan_from_input` の clamp・`maybe_rerender_pdf`) をそのまま通す。ズームの処理を複製しない。
  実装は `should_zoom_fullscreen_wheel` の入力に分類を加える形 (ズームする条件を 1 つの関数に保つ) を推奨する。
- `fs_wheel_action_requests_navigation`: `SettingZoom` は送らない (false)。`propose_fs_wheel_click_transform` は既存のズームの予測と同じ扱い。
- 適用時の再評価が `SettingZoom` でなくなっていたら (同じ frame の先の Alt+ホイールで動画・音声へ移った等)、`InPageScroll` と同じく
  **消費して何もしない** (r1 P2: ズームとして受理した入力が、移動先で音量やページ送りに変わらないようにする。受理時の「送らない」という分類と
  実際の適用の意味を揃える)。既存の `Legacy` の動的な処理は変えない。
- 変えないもの: 連結読み (面の外)、Z・360・3D・分析・編集・動画・音声・音楽ビューの専用のホイール (面の外または `Legacy`)。
  消しゴム等の編集モードのホイールのズーム (面の外で、既存の `should_zoom_fullscreen_wheel` の側のまま)。

## 4. §1.824 スクロール＋ページ送り

### 4.1 操作

| 入口 | 中身 |
| --- | --- |
| `KeyAction::FsPageScrollOrTurnNext` | ページ内を指定の順 (N 字 / Z 字) で次へ進み、読み終わりの端で、**新しく押したとき**次のページへ送る |
| `KeyAction::FsPageScrollOrTurnPrev` | 逆順。読み始めの端で、新しく押したとき前のページへ送る |
| 通常のホイールの設定「スクロール＋ページ送り」 | ホイール下 = 次、上 = 前 (§4.3) |

- 既定キーなし (`# FsPageScrollOrTurnNext = none`)。context `FsImage`、trigger `Press` (リピート可)、`SinglePerFrame`。
  `ini_name` / `description` / `context` / `trigger` / `default_chords` / `ALL_ACTIONS` / `docs/keymap.ini.default` /
  `search_nav_entry_inventory` を揃える。リング・ジェスチャ・マウスボタン・ゲームパッドには足さない (§1.474 と同じ)。
- 進み方は **読み順 (N 字 / Z 字) に従う** (利用者決定「綴じ方向と N 字 / Z 字の読み順も考える」)。ページ内の動きは K / J と同じで、
  違うのは端での振る舞いだけ。方向ごとの (上下だけの) スクロール＋ページ送りは足さない: 縦にだけ進む操作は、横にはみ出すページ
  (原寸・高さ合わせ) で横の残りを読まずに送ってしまうため。N 字 / Z 字の順なら縦横どちらのはみ出しも読み終えてから送る。
- 面の外・前面 UI が入力を持つ間は**キーを消費しない** (§1.474 §2.7 と同じ。上下キー・ホイールの設定値に依存しない)。

### 4.2 キーの 1 回 (状態を持たない)

面が成立したら、次の順に 1 つだけ行う。

1. 照合 (§1) が通らない → 消費して何もしない (送らない)。ただし §4.5 の「描けないと確定したページ」は 3 へ。
2. `ordered_scroll_pan` (移動量は §1.826 の設定) が pan を動かす → その位置を書く (滑らかな移動の設定なら §5)。ページは送らない。
   端へ着いた操作はここで止まる。
3. 動かない (= 読み終わり / 読み始めの端にいる。はみ出さないページを含む) → **その入力が新しい押し下げ (Press) のときだけ**、
   既存のページ送り (`spread_page_nav(±1)` と、同じ入力の operation) を発行し、着地を §4.4 で指定する。
   キーリピート (Repeat) では送らず、消費して何もしない。

- 「端でもう一度操作したときだけ送る」は、端を記憶せず「動かなかった」ことと「新しい押し下げ」で決める。端に着いた位置を
  状態に持たない (途中でドラッグ・ホイールで動かしても、その位置から毎回決め直す)。
- 長押し: 端まで進んで止まり、押し続けても送らない。離して押し直すと送る。送った先のページでは、押し続けたリピートで
  またページ内を進む (描いて照合が通ってから)。これで長押しで意図せずページが続けて送られることはない
  (画面に収まるページが続くときも、1 回押すごとに 1 ページ)。
- Press / Repeat の区別は、固定矢印の consume 結果 (`PageTurnConsumeResult`) と同じく、消費した入力の 1 つの結果から取る
  (keymap に「消費した押し下げの種類」を返す入口を足す。実キーの状態を後から読まない)。

### 4.3 ホイールの 1 segment

分類 `ScrollThenPageTurn(delta)` の segment は、縦の成分だけを「次 (下) / 前 (上)」の量として使う (横だけの segment は今と同じく受理しない = `Legacy`)。

1. 照合が通らない → 消費して何もしない (§4.5 の例外は 3 へ)。
2. 主軸 (N 字 = 縦、Z 字 = 横) の読む向きへ、`|delta.y|` をホイールのスクロール量 (連結読みと共用、既定 20%/ノッチ) で換算した量だけ進む
   (今の `fs_page_scroll_wheel_pan` と同じ換算。高精度ホイールの端数もそのまま)。主軸が端なら、`|delta.y| > 0.5` (ページ送りの閾値) の
   segment に限り、N 字 / Z 字の折り返し (副軸へ §1.826 の量 + 主軸を反対の端へ) を 1 つの候補として採る。
3. どちらも動かない (読み終わり / 読み始めの端) → `|delta.y| > 0.5` で、**その frame でまだこの操作が pan を動かしていない**ときだけ、
   既存のホイールのページ送りと同じ経路 (`spread_page_nav(base)` と segment の operation) で送り、着地を §4.4 で指定する。
   それ以外は消費して何もしない。

- ホイールには押し直しが無いので、「端に着いた segment は止まり、端から始まる次の segment が送る」で「もう一度の操作」を表す。
  1 frame に複数ノッチが来たときも、先の segment で端へ着いたら同じ frame では送らない (frame 内のローカル値で決める。持ち越さない)。
- 送った後の segment は、今の `InPageScroll` と同じく消費して何もしない (移動先のページに旧ページの幾何で書かない)。
- 小さい端数の segment (タッチパッドの慣性など) は、ページ内を進む・止まるだけで、折り返しと送りをしない。
- `fs_wheel_action_requests_navigation` と `propose_fs_wheel_click_transform`: 受理時の印と同じ純関数で、その frame の前の segment を
  順に当てた予測位置から「動く / 送る / 何もしない」を決める。送ると予測した segment の後は、今の予測と同じく打ち切る。
  遅延適用 (`merge_at_frame_tail`) の判定 (`wheel_navigation_produced`) も、この予測の「送る」で決める。

### 4.4 着地 (次 = 読み始め、前 = 読み終わり)

- 次へ送ったとき: 新しい表示単位の**読み始め** = はみ出す軸だけ、縦は上端、横は読む向きの始まり (右→左なら右端、左→右なら左端)。
  §1.726 の「読み始め」と同じ位置。N 字・Z 字どちらでも最初の位置はこの角になる。
- 前へ送ったとき: **読み終わり** = 縦は下端、横は読む向きの終わり (右→左なら左端、左→右なら右端)。N 字なら最後の列の下、Z 字なら最後の段の終わり。
  N 字 / Z 字どちらでも同じ角になるので、読み順の設定で着地は変わらない。
- 読み方向は §1.726 と同じ源 (`spread_mode.reading_direction().unwrap_or(self.reading_direction)`)。
- **倍率と、はみ出さない軸の位置は、その送りの既存の規則のまま** (r1 P2): 別のページへの送りは、入場が倍率を fit・pan を 0 に戻す
  (`open_fullscreen_with_metadata_boundary_inner`)。同じ画像の反対側への分割送り (`accept_fullscreen_page_slice_navigation`) は
  倍率と手動の位置を保つ (既存テストで固定)。着地が動かすのは、その時点ではみ出す軸だけ (`reading_start_pan` が収まる軸の `origin_pan` を残すのと同じ)。
- 仕組み: §1.726 の初期配置要求 `FsInitialPlacementRequest` に **要求の種類** `kind: Setting | LandingStart | LandingEnd` を足す
  (r1 P2: 設定由来の読み始めと操作による読み始めを区別する。bool を別に足さず 1 つの種類にする)。`reading_start_pan` の隣に
  `reading_end_pan` を置き、`LandingEnd` だけがそれを使う。素材の選択・仮表示・AI 完了後の再配置・手で動かしたら消える、の契約は
  §1.726 のまま (仮表示で置いた位置を、本素材で同じ種類のまま置き直す)。
  全体の設定 (「ページ切替後の表示位置」) が「中央」でも `Landing*` は作り、設定の変更で退役するのは `Setting` だけ。全体の設定と本の設定は書き換えない。
- 要求の field の扱い (経路の調査、コード読み): `request_fs_initial_placement` (作る)・`retire_fs_initial_placement_request` (退役)・
  `request_fs_initial_placement_after_composition_change` (構成変更で作り直す) の 3 か所が全体の設定で門を閉じている。
  着地の要求はこの 3 か所で設定に依存させない (作る・退役しない・構成変更の作り直しは**既存の要求の着地の種類を引き継ぐ**)。
  `fs_initial_placement_pan` と `resolve_fs_initial_placement` の自動配置の記録 (`fs_automatic_initial_view`) と仮表示での再要求は
  要求の種類で同じ関数を選ぶ。`fs_view_has_moved` のコメント (「自動の読み始めは動かした扱いでない」) を読み終わりにも広げる。

#### 4.4.1 要求と移動の対応付け (型付きの引数で入場まで運ぶ)

経路の調査 (コード読み) では、`FsPageNav` から入場 (`open_fullscreen_with_metadata_boundary_inner` → `request_fs_initial_placement`) まで
1 つの型付きの要求は通っていない。`ViewerOperationId` も入場まで届かない。保留する経路は 3 つ (紙めくり演出の継続・本の構成の確認・
サイドカー復元) で、それぞれ別の保持者を持つ。

r1 では「送りの呼び出しの間だけ有効な context 所有の一時指定」を案にしたが、独立レビュー r1 の P1 2 件で撤回した:
(a) 同じ `handle_fs_navigation_with_render_perf` の呼び出しの後半でスライドショーのタイマーも処理するので、手動の送りが境界で動かなかった frame に
タイマーが満了すると、残った着地がスライドショーの入場に付く。(b) 構成を保存した ZIP / PDF を含む一覧では、今のページが描けていても
隣への送りが本の構成の確認 (`OpenIntent::Checking`) へ保留され得る (通常の操作)。再開は後の `complete_book_navigation_target` なので一時指定は失われる。

r2 の形 (案 B): **着地 `FsPageLanding { Start | End }` を、送りの要求と一緒に型付きの引数・field で入場まで運ぶ。** 暗黙の context 状態にしない。

1. 入力: frame の中で手動の送りを表す値を **`FsPageNavRequest { nav: FsPageNav, landing: Option<FsPageLanding> }` の 1 つ**にする
   (r2 P2)。キーの handler の action・ホイールの `FsWheelFrameResult`・遅延の `FsWheelFrameProposal`・クリックの送りなど、frame 内で
   送りを書く・上書きする箇所はすべてこの値を丸ごと書く (§1.824 以外は `landing: None`)。後段の固定矢印やクリックが送りを上書きすれば
   着地も一緒に消える。frame 内の途中の送り (`handle_fs_navigation_with_operation`) にも同じ値を渡す。
2. `handle_fs_navigation_with_render_perf` は着地を引数で受け、**手動の `page_nav` の分岐 (Delta / Target / Wrap / Split) だけ**に渡す。
   スライドショーのタイマー・ctrl / sibling / jump / 一覧順などの他の移動には渡さない (P1 (a) の解消)。
   着地を持たない既存の呼び出し (約 30 の `handle_fs_navigation`) は None を渡す既存の入口のまま。
3. 本の構成の確認: `admit_book_page_navigation_with_purpose` に着地を渡し、保留したら `OpenIntent` に保持する。保留の置き換え・
   世代や context の変化での破棄 (`bind()` 等) は `OpenIntent` ごとなので、着地も一緒に消える。再開 (`complete_book_navigation_target`) は
   保持した着地で入場する (P1 (b) の解消)。
4. 入場: `land_still_page_navigation_target` と、同じ画像の分割切り替え `accept_fullscreen_page_slice_navigation` に着地の引数を足し
   (他の呼び出し元は None)、`open_fullscreen*` の系列は既存の約 50 の入口を変えずに、着地ありの入口を 1 つ足して `_inner` の
   `request_fs_initial_placement` まで運ぶ。この入口は `open_fullscreen_from_fs_navigation_with_contract` (中で
   `defer_book_composition_navigation_open` を呼ぶ) から `_inner` (2 回目の構成の確認・サイドカー) まで、**途中のすべての保留の境界**を覆い、
   保留になればその保持者 (`OpenIntent` の各経路・`DeferredFullscreen`) へ 1 field で渡す (r2 P2)。再開の側も、`complete_book_navigation_target` の
   `land` を通る分岐と、`land` を通らずこの wrapper へ直接入る `Explicit` の分岐の両方で、保持した着地を使う。
5. 紙めくり演出の継続 (`ResolvedPageTurnNavigation`) には足さない: 演出中は §1 の照合が通らず (`page_turn_barrier_active`)、§1.824 は送りを発行しない。
6. サイドカー復元: 同じ context の復元中は入力自体が遮断される (`sidecar_restore.rs` の入力遮断、レビュー r1 の確認)。4 の field は経路を揃えるためだけ。

6b. **コレクション直下の送り** (2026-11-20 設計担当の決定。実装担当の 2 回目の停止報告: コレクションの直下では、送りが入場の前に専用の非同期の経路
   `start_collection_manual_display_unit_navigation` へ入り、`CollectionNavigationPending::AwaitingManualContinuation` が応答を待つ間の送りを蓄積し、
   着地なしの入口で開くため、着地が失われる): 着地を `CollectionNavigationPending` の 1 field として持たせ、**蓄積の一致条件に加える**
   (`accumulate_manual` が今 `landing` (Fullscreen 等の開き方)・`still_only`・`display_unit_step`・`wrap` と向きの一致で足すのと同じ扱い)。
   一致すれば歩数を足して同じ着地のまま、違えば既存の「新しい意図」の経路に乗る (着地なしの通常の送りと §1.824 の送りは蓄積されない)。
   開くときは保持した着地で入場する。新しい置き換え規則は作らず、保持者の既存の規則に従う。蓄積された 2 歩の §1.824 の次への送りは、
   2 つ先の表示単位の読み始めに着地する。
7. **保留での分割の側 (既存の問題、範囲外)**: 別画像への `Split` / slice 付きの `Wrap` は、同期の入場の後で呼び出し元が slice を決める。
   本の構成の確認で保留されると、再開は idx だけで開くので、分割の側 (例: 前のページの後半) が失われる (r2 P1、コード読み)。
   これは今の ← / ↑ / ホイールのページ送りでも同じで、§1.824 で新しく生じるものではない。§1.824 の着地は「実際に入った表示単位」の
   読み始め / 読み終わりに付く (側が失われたときは、入った側の読み終わり)。保持者に解決済みの表示先 (idx + slice) を持たせて既存の送りごと直すのは、
   出荷済みの送りの挙動を変えるので別項目にする (バックログ §1.831)。

これで着地は「その送りの要求が起こした入場」だけに付き、別の移動・スライドショー・別の context・別の items 世代 (保持者と入場の要求の
既存の照合) へ残らない。保持者の取り消しと着地の組み合わせは、保持者の既存の取り消しにそのまま従う (新しい取り消し規則を作らない)。

### 4.5 描けないと確定したページ・境界

- 照合が通らない間 (読み込み中・仮表示の前・幾何が変わった直後) は送らない。読まずに飛ばさないため。
- 例外: 現在の表示単位 (今の items 世代・`fullscreen_idx`・slice から求めた単位) のどれかの実ページが**読み込み失敗で確定**しているときは、
  印が付かないまま待ち続けないよう、「動かない (端)」として §4.2 / §4.3 の 3 を行う (送る)。着地は送った先で通常どおり決める。
  判定は既存の統合された状態 `fs_page_load_state` の `LoadFailed` を使う (r1 P2: RAW の終端失敗は `FsCacheEntry::Failed` を作らず専用の store に
  記録されるため、`Failed` だけでは止まる)。見開きの片側だけ失敗も含む。
  この例外が置き換えるのは**「描いた表示単位の印がある」条件だけ**で、照合の他の条件 (面の上・紙めくり演出中でない `page_turn_barrier_active`・
  ページ送り待ちでない `fs_navigation_sequence_blocks_new_target`・items 世代) は今と同じく要る。演出中・送り待ちの間に失敗が確定しても、その間は消費だけで送らない
  (2026-11-20 設計担当の決定。実装担当の停止報告: 演出中に失敗すると送りが紙めくりの継続へ保留され、§4.4.1 の 5 で着地を運ばない経路に入るため)。
  演出・送り待ちが終われば、次の入力で送れる。
- 本・フォルダの先頭 / 末尾、ループ (端から端への移動)、見開き・横長分割・本の構成・閲覧順は、`spread_page_nav(±1)` が今と同じく決める
  (今の ↓ / → / ホイールのページ送りと同じ結果)。境界で送れない (`Boundary`) ときは今のページ送りと同じ扱い。ループで反対の端へ
  移るときも、次へ = 読み始め・前へ = 読み終わりに着地する。

## 5. §1.823 滑らかな移動

### 5.1 設定と範囲

- 設定 `fullscreen_smooth_page_scroll: bool`、既定 **false**。環境設定「閲覧表示」の上下キー・ホイール・読み順の行の後に 1 行
  (「ページ内のスクロールを滑らかに動かす」)。移動の時間は固定 (0.2 秒、ease-out)。時間の設定は足さない (必要が分かってから)。
- 滑らかにする入力: ページ内スクロールの全入口 (↑↓ の置き換え、4 方向の Action、K / J、§1.824 の端までの移動、ホイールの
  ページ内スクロールと §1.824 のページ内の移動)。
- 瞬時のまま: ドラッグ、ズーム (Ctrl・設定のズーム)、ページを開いたときの配置 (§1.726・§4.4)、ダブルクリックのリセット、
  ナビゲータでの位置指定、連結読み (面の外)、タッチパッドのホイール (`MouseWheelSource::Trackpad`。指の動きに遅れて付いてくると操作しにくいため)。

### 5.2 組み合わせを減らす形 (採用)

- **正本の位置は常に最終位置**: 入力は今と同じく最終の pan を `fs_pan` に即時に書く。照合・端の判定・§1.824 の送り・クリックの判定・
  §1.745・§1.726・保存・他の機能が読むのは今と同じ `fs_pan` で、滑らかにしても**論理は瞬時の設定と同じ**。
- **見た目だけ補間する**: context 所有の `fs_page_scroll_animation: Option<FsPageScrollAnimation { from, to, start_time: Option<f64>, last_painted_pan, last_painted_time: Option<f64>, origin: FsDrawnUnitStamp, viewport: ViewportId }>`
  を 1 つ持ち、ページ単位の通常静止画の描画 (印を付ける経路) だけが「表示に使う pan」= `from + (to − from)·ease(t)` を読む。
- **新しい入力は目標を更新するだけ**: 補間中の入力は `fs_pan` (最終位置) から次の位置を決める。入力時の照合は印・目標・配送先・幾何を使い、期限は見ない。
  照合が通る開始済みの補間では、`from = last_painted_pan`、`start_time = last_painted_time` として、最後の実描画の位置と時刻を対で引き継ぐ。
  次の描画は前の描画から経過した分だけ新しい目標へ進むため、毎frame入力があっても進み具合0へ戻らない。期限は引き継いだ開始時刻から0.2秒とする。
  未開始 (`start_time = None`) の補間の差し替えではfromを保ち、開始もNoneのままにする。補間がないか照合が通らなければ、`from = fs_pan`、開始Noneの新しい要求にする。
  最後の描画panと時刻は同じ補間owner内に保持し、配送先の実表示単位に印を再設定できた描画で、そのtransformのpanとそのpassのinput.timeを同じ条件で記録する。
  まだ実描画の記録がない時刻はNoneで、入力時に0や現在時刻を補わない。入力では時計を読まず、記録済みの値をコピーするだけで、`pan_at` も呼ばない。
  古い移動をキューに溜めない。連打・ホイールの連続でも最終位置は瞬時の設定と同じ。
- **それ以外の変化では補間を捨てて最終位置で描く**: 失効を書き手ごとに足さず、比較で決める (§1.474 §2.10 と同じ方針)。
  毎 frame、配送先viewportの入力処理の後・描画の前、前frameの印を消す前の1か所で確かめる。
  `start_time` がNoneならその描画passの `input.time` で初めて確定し、次のどれかなら補間を捨てる:
  `fs_pan != to`、時間切れ、`fs_scroll_layout_matches` (面・世代・表示単位・幾何の入力の鍵・倍率・ページ送り待ち・紙めくり演出) が通らない、
  `origin` が今の印と違う。ページ送り・ズーム・ドラッグ・回転・fit・トリム・見開きの構成・窓の大きさや DPI・モードや閲覧ウィンドウの切り替え・
  閉じる、はこのどれかに当たる (ページ送りは pan を 0 へ書き、ズーム・ドラッグは `fs_pan` を書き換え、回転・fit 等は鍵が変わる)。
  描画の helper も読むたびに `fs_pan == to` を確かめ、違えば `fs_pan` で描く。
  退役判定が配送先でないpassから呼ばれた場合は、確定も期限判定もせず補間を残す。そのpassでは補間を描画しない。
- **終了・保存**: 補間は保存しない。閉じる・終了では捨てるだけ (最終位置は既に `fs_pan`)。
- **閲覧ウィンドウ・context の切り替え** (r1 P2): 補間は context の bundle と一緒に swap される。兄弟 context の処理のための一時的な mount では
  触らない。別の viewport へ mount されれば幾何の入力の鍵 (viewport・DPI) が変わり、その配送先の描画passで捨てる。同じ viewport へ戻った場合は、
  開始確定後なら経過時間で捨てる (0.2 秒以内に戻れば残りの補間が続く)。開始前はその配送先の最初の描画で確定する。
  利用者の切り替えを検出する退役の処理を新しく足さない。
- 補間は**配送先の `ViewportId`** も持ち、今の配送先と違えば捨てる (r2 P2: 幾何の入力の鍵の viewport は矩形なので、同じ矩形・DPI の別 viewport を区別できない)。
- **時刻は配送先viewportの描画passでだけ読む** (独立実装レビュー r3 後の利用者決定)。開始の確定と期限判定は上記1か所、
  `pan_at` は確定済みの `start_time` と配送先の描画passの時刻だけを使う。ROOT配送・遅延ホイール適用を含む入力経路では時刻を読まない。
  eframeの時刻起点が共通でも、子の描画からROOTへ戻るとROOTのsampleは古いので、入力経路で読み分ける形は採らない。
  補間中だけ、従来どおり配送先viewportの再描画を要求する。
- **描いた transform から pan を逆算する入力** (r1 P1): ナビゲータの位置指定 (`pan_to_center_source_normalized` を使うダブルクリック・範囲ズーム・
  描画時の保留中央指定) は、渡す pan が transform の描画の pan と同じことを前提にしている。補間中は `fs_pan` (最終位置) と描画の pan が違うので、
  これらは、**補間が有効なときだけ**、その transform 自身の描画 pan (`normal_zoom_pan(transform)` の pan) を基準に計算する (r2 P3: 補間が無いときは
  既存のコードのまま。同じ frame の入力で `fs_pan` が前の layout より先に進む場合や、Z の transform で `normal_zoom_pan` が None になる場合の
  既存の挙動を変えない)。該当する実処理は範囲ズームの `zoom_preserve_pivot`・ダブルクリック・描画時の保留中央指定の 3 つ (レビュー r2 の確認)。
  同じ前提を持つ他の呼び出し元も実装時に列挙して揃える (レビュー r1 の棚卸しでは、ピクセルグリッド・crop・ルーペ・ナビゲータの可視枠・
  layout の hit test は描いた layout から読むので追従する)。
- **描画の負荷** (r1 P2・r2 P1/P2): 縮小 (全体の出力) は pan に依存しない。拡大の描画 (Lanczos・NIS・アニメ・ピクセルアートの拡大) は可視の ROI を
  キーに出力を作るので、補間の 0.2 秒の間は、**ドラッグで同じ距離を動かしたときと同じく**毎 frame 可視域を再標本化する (60Hz で約 12 frame、144Hz なら約 29 frame。開始後の追加入力は最後の描画位置と時刻から再設定し、継続中も目標へ進み続ける)。
  補間のための特別な ROI は作らない。r2 では「開始と最終の可視域の和を 1 回だけ作る」案を検討したが、(a) ページへ clip した後の和は途中の可視域を
  覆わない反例がある (非対称な見開きの折り返し)、(b) 和を広げると 4096² 画素の上限・アニメ拡大の入力上限を超えて補間中だけ別のフィルタ・元画像表示へ
  落ちる、(c) 浮動小数点のキーを補間の間固定する受け渡しが要る、ため採らない。ドラッグと同じ経路なので、新しい組み合わせ (補間中だけの画質・
  上限の分岐) を作らない。途中で可視域の大きさが変われば既存の Anime→Lanczos・元画像への切り替えは起こり得るが、ドラッグと同じ既存の判定で、補間用の分岐は足さない。
  キャッシュは既存の個数・画素数の制限で刈られる (レビュー r3 の確認)。
  増えないもの: 画像の読み込み・デコード、AI、補正・消しゴムの再計算、PDF の再描画 (倍率基準)。これを §9 で確かめる。

### 5.3 採らなかった案

- `fs_pan` に途中の位置を書き、目標を別に持つ案: 途中の位置が照合・端の判定・クリック・§1.745 に混ざり、割り込みの時点ごとに
  「瞬時の設定なら最終位置だった」結果と食い違う。割り込み (ズーム・回転等) の前に最終位置へ戻す処理を書き手ごとに足すことになる。
- 滑らかな移動の間だけ別の表示状態に入る案: 終了条件と他モードとの排他が増える。
- ズーム・ドラッグ・ページを開いた配置まで滑らかにする案: 範囲外 (利用者決定)。組み合わせが大きく増える。

## 6. 状態の組み合わせを減らす検討 (CLAUDE.md「設計の簡素化」)

- 新しい実行時状態は 2 つだけ: §4.4 の要求の種類 (既存の初期配置要求の 1 field) と §5 の補間 1 つ (context 所有、比較で失効)。
  着地は送りの要求の引数・保留の保持者の field として運ぶだけで、独立した状態を持たない (§4.4.1)。取り消しは保持者の既存の規則に従う。
- 着地を「送りの呼び出しの間だけの一時指定」にする案は、同じ呼び出しのスライドショーへの漏れと本の構成の確認での消失があり撤回した (§4.4.1)。
- §1.824 の「端」は状態に持たない。「動かなかった」+「新しい押し下げ」(キー) / 「その frame で未移動」(ホイール、frame 内のローカル値) で決める。
- §1.825 は新しい処理を作らず、既存の Ctrl+ホイールのズームへ分類で合流させる。
- §1.823 は論理の位置を最終位置に固定し、見た目だけを比較で失効する補間にする (§5.2)。割り込みの組み合わせは「捨てて最終位置で描く」の 1 通り。
- §1.826 は純関数の定数を設定に置き換えるだけ。
- まれな失敗: 保存データを持たない新しい状態はない (設定 3 つは既存の設定の保存のまま)。既定の割り切り。

## 7. 他の機能との関係

| 機能 | 扱い |
| --- | --- |
| 見開き・横長分割・本の構成・閲覧順 | ページ内は見開き全体 / 表示中の片側を範囲に動く (§1.474 と同じ)。送りは `spread_page_nav` |
| 綴じ方向 | §1.824 の進み方と着地の横の端、ホイールのズームは関係なし |
| シークバー | その上のホイールは設定にかかわらずページ送り。ドラッグ中のホイール抑止はそのまま |
| Ctrl / Alt / Shift + ホイール | Ctrl = ズーム、Alt = ページ送り、Shift = 今のまま (ズームの設定でも何もしない。ページ内スクロール・スクロール＋ページ送りでは §1.474 の横成分の規則のまま) |
| 連結読み・Z・360・3D・分析・比較・編集・表示トリム編集・範囲キャプチャ・動画・音声・音楽 | 面の外。今のまま |
| §1.726 読み始め | §1.824 の送りは操作ごとの着地を持つ (全体設定より優先、その送り 1 回だけ)。ページ内の移動は初期配置要求を消す (今のまま) |
| §1.745 クリック送り | `fs_pan` は最終位置なので今と同じ判定。§1.824 で送った先は自動配置 (`fs_automatic_initial_view`) なので単クリックで送れる |
| 紙めくり演出 | §1.824 の送りは今のページ送りと同じ (pan が 0 でない状態からの送りは演出なし、の既存規則のまま) |
| スライドショー | 影響しない |
| F12 別ウィンドウ / 独立 viewer | 新しい状態 (着地の種類・補間) は context 所有で swap に載せる。入力は既存の viewport ごとの配送。兄弟 context は変えない |
| mIV Remote | 変更しない |
| `?` ヘルプ | ホイールの説明を 4 値で出し分ける。新 Action は割り当て後に一覧に出る |

## 8. 実装範囲 (見込み)

- `src/settings.rs` / `src/settings_transfer.rs`: `FullscreenWheelAction` に 2 値、`fullscreen_n_scroll_step_percent`・`fullscreen_smooth_page_scroll`。既定・正規化・転送・件数。
- `src/keymap.rs` / `docs/keymap.ini.default`: Action 2 つ。消費した押し下げの種類を返す入口。
- `src/ui_fullscreen/in_page_scroll.rs`: 移動量の引数化、ホイールの順の移動、端の判定の結果型、補間の helper。
- `src/ui_fullscreen/initial_placement.rs`: 着地の種類と `reading_end_pan`。
- `src/ui_fullscreen.rs`: ホイールの分類の 4 値・受付・予測・適用、キーの消費、描画で表示用 pan を読む箇所。
- `src/app/viewer_context_registry.rs`: 補間を swap に載せる。
- `src/ui_dialogs/preferences/pages.rs`・`search_index.rs`・`src/ui_dialogs/context_shortcuts.rs`: 設定の行・説明・ヘルプ。UI スナップショット。
- 文書: 本書、`fullscreen-in-page-scroll-plan.md` (本書への参照)、`keymap-spec.md`、`key-customization-impl-plan.md`、`display-pipeline.md`、
  `spec.md`、`detached-rework-plan.md` §11、マニュアル (`fullscreen.html`・`shortcuts.html`・`settings.html`)、`docs/README.md`、バックログ。

## 9. 検証 (追加テストの観点)

- §1.826: 量 10 / 50 / 85 / 100% で、N 字・Z 字・LTR / RTL・前後で同じ列・段を繰り返さず端で止まる。非対称な見開きの循環防止 (既存テストを量ごとに)。
  設定の編集 → OK → K / J の移動量が変わる通し。連結読みの PageUp / PageDown が変わらない。clamp・serde 欠落・転送。
- §1.825: 画像の上の通常ホイール = ズーム (カーソル基準・パン制約・PDF の再描画要求)、シークバー上 hover = ページ送り、
  シークバーのドラッグ中は今の抑止、Ctrl = ズーム、Alt = ページ送り、Shift = 何もしない、fallback メニュー中は消費のみ、
  面の外 (連結・Z・比較・編集・動画・音声・音楽) は今と同じ、同じ frame のホイール + クリックの予測と適用が一致、既定設定の回帰。
  設定の編集 → OK → 効果の通し、`Unknown` の正規化、古い値の読み込み。
- §1.824: キー (Press で端なら送る・Repeat では送らない・端へ着いた操作は止まる・はみ出さないページは 1 押しで送る・
  照合が通らない間は送らない・`Failed` のページは送る)、着地 (次 = 読み始め・前 = 読み終わり、N 字 / Z 字・LTR / RTL・
  縦だけ / 横だけ / 両方はみ出す・見開き・分割の片側・仮表示から本素材への置き直し・全体設定が中央でも着地する・
  着地の後に手で動かすと要求が消える)、本の先頭 / 末尾・ループ、ホイール (端に着いた segment は止まる・端から始まる次の
  segment が送る・同じ frame の 2 ノッチで送らない・端数の segment は送らない・送った後の segment は消費のみ・予測と適用の一致)、
  面の外・前面 UI 所有では消費しない、ini 往復と既定なし、別ウィンドウの context で着地と送りが閉じる。
  着地の要求が別の移動・別 context・別の items 世代へ残らない (§4.4.1)。境界で動かない送りと同じ frame のスライドショーのタイマー満了で、
  スライドショーの入場に着地が付かない。構成を保存した ZIP / PDF を含む一覧で隣への送りが構成の確認へ保留され、再開後の入場に着地が付く。
  保留が置き換えられた・世代が変わったときは着地も消える。同じ画像の分割送りは倍率と収まる軸の位置を保ち、はみ出す軸だけ着地する。
  要求の種類: 仮表示の間に全体設定を「中央」へ変えると `Setting` は退役し `Landing*` は残る。全体設定が「読み始め」のときの構成変更の作り直しが
  `LandingEnd` を `Setting` に置き換えない。RAW の終端失敗・見開きの片側失敗のページで送る。
- §1.823: 既定 OFF で今と同じ (補間が作られない)。ON で、入力直後の `fs_pan` は最終位置、描画の pan は途中、時間経過で最終位置に一致して
  補間が消える。補間中の入力は最終位置から積み上がる (2 回の ↓ = 瞬時の 2 回と同じ最終位置)。補間中のページ送り・ズーム・ドラッグ・回転・
  fit 変更・窓サイズ変更・別 viewport への mount・閉じるで、次の描画が最終位置 (または書き手の位置) になり補間が消える。兄弟 context の一時 mount では
  補間が残る。矩形・DPI が同じ別 viewport へ移しても捨てる。タッチパッドのホイールは瞬時。補間中のナビゲータのダブルクリック・範囲ズーム・
  描画時の中央指定が、補間なしと同じ位置へ着く。補間が無いとき・Z の transform では、ナビゲータの位置指定は今のコードのまま。
  補間の間に、画像の読み込み・デコード・AI・補正・PDF の再描画の要求が増えない (拡大の可視域の再標本化はドラッグと同じ)。
  未開始の補間に入力だけを繰り返しても開始時刻は未確定のまま、配送先でないpassは開始・期限を消費しない。
  子viewportの描画後に古いROOTのpassへ戻って遅延ホイールを適用しても、始点と開始時刻は最後の実描画pan・時刻の対を引き継ぐ。
  実rendererへ毎frameホイールまたはKを配送し、入力中も表示が毎frame目標方向へ進み、fs_panを追い越さず、入力停止から0.2秒後には到達・退役する。
- 同じ frame の §1.824 と通常の送り (固定矢印・クリック): 後から書いた送りが勝ち、着地は残らない (`FsPageNavRequest` を丸ごと上書き)。
  `open_fullscreen_from_fs_navigation_with_contract` の保留と `Explicit` の再開でも着地が使われる。分割表示で前の別画像の後半へ戻る送りが同期なら、
  後半の読み終わりへ着地する。構成の確認で保留される Split / Wrap では、通常の送りと同じ idx・slice に入り、その実際の単位へ着地する
  (保留で側が失われる既存の問題は範囲外、§4.4.1 の 7。通常の送りとの比較を回帰の条件にする)。
- §1.825 の混在: 同じ frame で Alt+ホイールが動画へ送った後の、ズームとして受理した segment が音量・ページ送りにならず消費だけになる。
- 共通: 既定設定での回帰 (↑↓・ホイール・K / J・クリック・Ctrl / Alt ホイール)。UI スナップショット (環境設定の該当ページ)。

## 10. 独立設計レビュー

### 10.1 r1 (GPT-6.1 Sol / xhigh、read-only、session 01a1f6dc、2026-11-20): REVISE

| 指摘 | 対応 (r2) |
| --- | --- |
| P1 §4.4.1 の一時指定が同じ呼び出しのスライドショーの入場へ漏れる | 撤回。着地を手動の `page_nav` の分岐だけへ型付きで渡す (§4.4.1) |
| P1 構成を保存した ZIP / PDF を含む一覧で、隣への送りが構成の確認へ保留される (通常の操作) | `OpenIntent` に着地を保持し、再開の入場で使う (§4.4.1 の 3) |
| P1 補間中のナビゲータの位置指定が transform と `fs_pan` の組を崩す | transform 自身の描画 pan を基準に計算する (§5.2) |
| P2 拡大の描画は可視 ROI をキーにするので補間で毎 frame 作り直す | 補間中は開始と最終の可視域の和を要求し 1 回だけ作る。各軸で表示領域より大きい移動は瞬時 (§5.1・§5.2)。**r3 で撤回** (§10.2) |
| P2 同じ画像の分割送りは倍率と手動の位置を保つ | 着地は送りの既存の規則のまま、はみ出す軸だけ動かす (§4.4) |
| P2 `ReadingStart` だけでは設定由来と操作の着地を区別できない | 要求の種類 `Setting / LandingStart / LandingEnd` (§4.4) |
| P2 `FsCacheEntry::Failed` だけでは RAW の終端失敗で止まる | `fs_page_load_state` の `LoadFailed` (§4.5) |
| P2 `SettingZoom` が適用時に `Legacy` へ変わると音量・送りになる | 消費だけにする (§3.3) |
| P2 比較だけでは閲覧先の切り替えを検出できない | 別 viewport は鍵で捨て、同じ viewport へ戻ったときは時間で捨てる。害がないことを §5.2 に書いた (退役の処理は足さない) |

### 10.2 r2 (同じ session、2026-11-20): REVISE

| 指摘 | 対応 (r3) |
| --- | --- |
| P1 保留では着地だけでなく分割の側 (slice) も失われ、前の後半へ戻る送りが前半に入る | 今の通常の送りでも同じ既存の問題。§1.824 の着地は実際に入った表示単位に付く。保持者に表示先を持たせる修正は出荷済みの送りを変えるので別項目 (§4.4.1 の 7) |
| P1 開始と最終の可視域の和は途中の可視域を覆わない (非対称な見開き)。UV と出力寸法とキーの固定も要る | 和の案を撤回。補間中の拡大の描画はドラッグと同じ毎 frame の再標本化にする (§5.2) |
| P2 和を広げると 4096² 画素の上限・アニメ拡大の入力上限を超える | 同上 (和を作らないので補間中だけ別の分岐へ落ちない)。距離の規則も撤回 |
| P2 後段の固定矢印・クリックが `page_nav` を上書きしても着地が残る | 送りと着地を 1 つの `FsPageNavRequest` にし、丸ごと上書きする (§4.4.1 の 1) |
| P2 `open_fullscreen_from_fs_navigation_with_contract` にも保留の境界、`Explicit` の再開は `land` を通らない | 着地ありの入口が途中のすべての保留の境界と両方の再開を覆う (§4.4.1 の 4) |
| P2 鍵の viewport は矩形で、同じ矩形の別 viewport を区別できない | 補間に配送先の `ViewportId` を持たせる (§5.2) |
| P3 ナビゲータの逆算の変更は補間が有効なときだけに限る | 限定した (§5.2) |

r1 の判断事項の扱い: 補間中の拡大の再標本化は、バックログの確認点「アニメーションだけのために毎フレーム画像の再生成・読み込みを追加しない」のうち
「再生成」に当たり得る。ドラッグと同じ負荷・同じ経路で、読み込み・AI・補正は増えないことを理由に採った (統括へ報告する)。

レビュー担当は、§1.826、Press / Repeat の分離、端へ着いた操作では送らない規則、frame 内の移動済みの判定、N 字 / Z 字・LTR / RTL の着地の角、
境界・ループを既存の router に委ねる方針、既定の経路の維持を妥当とした。detached は既存の配送先と context swap を使う範囲なら構造的な拡張で、
述語・viewport の選択の変更は不要と判断した (変更する場合は別に双方の合意と §11 の記録が要る)。

### 10.3 r3 (同じ session、cf3572424、2026-11-20): ACCEPT

P1 / P2 なし。P3 3 件を反映した: 再標本化の frame 数はリフレッシュレートと目標の更新で変わる (§5.2)、途中の可視域で既存の画質・上限の切り替えは
起こり得るが補間用の分岐は足さない (§5.2)、保留付きの Split / Wrap は通常の送りと同じ表示先に入ることを回帰の条件にする (§9)。
レビュー担当は、slice の消失を既存の問題として切り離す整理を受け入れ、§1.824 が通常の送りより表示先を悪くする追加の経路は見当たらないとした。

## 11. 実装記録

以下の件数・実行状況は各時点の記録。着地総当たりテストの名前は現在の5分割後へ更新したが、当時の件数では1本として数えている。

### 2026-11-20: 前提矛盾により停止（未完成・未検証）

利用者指示に従い、コンパイルを伴うコマンド・テスト・ビルド・アプリ起動・コミット・ブランチ操作は実施していない。
以下はソース読みの結果であり、実行時の再現報告ではない。設計変更や症状パッチは入れていない。

**停止理由**: §4.4.1 の 5 は「紙めくり barrier 中は照合が通らないので新操作は送りを発行せず、
`ResolvedPageTurnNavigation` に着地を追加しなくてよい」とする。一方、§4.5 は `LoadFailed` なら照合不通でも送りを発行する。
既存コードではこの2状態が排他的ではない:

1. `src/ui_fullscreen/page_turn/nonfinal_tests.rs` の既存テスト
   `source_quality_publication_stays_frozen_until_canonical_flat_is_adopted` は、resident thumbnailだけで `Turning` に入る経路を持つ。
2. `src/app.rs` の既存ロード完了処理は `FsLoadResult::Failed` を `FsCacheEntry::Failed` として採用し、
   `fs_page_load_state` は `LoadFailed` を返す。
3. `src/ui_fullscreen/page_turn/binding.rs::PageTurnContent::matches_raw` は `source_gen` を0へ正規化し、
   読込失敗をbinding失効条件にしない。`page_turn_pre_draw` / `page_turn_view_eligible` / `page_turn_held_resource` にも失敗時退役はない。
   したがって保持サムネイルによる `Turning` / `Completing` と `LoadFailed` は併存し得る。
4. そこで§4.5の送りを発行すると、`handle_fs_navigation_with_render_perf` → `page_turn_defer_navigation` →
   `page_turn_defer_navigation_intent` が継続を保持する。`ResolvedNavigationIntent::Page` はsliceのみで着地を持たず、再開時に着地を失う。

再開には設計担当が、(a) barrier中は失敗時例外より消費のみを優先し、barrier解消後の新操作で送る、
または (b) 失敗時例外を優先し、紙めくりの継続・再開にも着地を型付きで運ぶ、のどちらかを正本へ明記する必要がある。
前者は§4.5の適用条件、後者は§4.4.1の5と実装範囲を変えるため、実装担当では選んでいない。

**保持した途中差分**:

- `src/settings.rs` / `src/settings_transfer.rs`: 設定2件、Wheel4値、既定・clamp・転送分類と件数・既存全field変更fixture。
- `src/keymap.rs` / `docs/keymap.ini.default` / `src/app/search_nav_entry_inventory_tests.rs`: Action2件の登録と分類、
  消費したedgeの種類を返す `consume_action_press`（既存の入口は維持、OSキー状態の新規読取なし）。
- `src/ui_dialogs/preferences/pages.rs` / `src/ui_dialogs/preferences/search_index.rs` / `src/ui_dialogs/context_shortcuts.rs`:
  設定行・検索anchor・説明とヘルプ。snapshotの定義更新・PNG生成は未着手。
- `src/ui_fullscreen/in_page_scroll.rs`: `ordered_scroll_pan` の移動量引数化、キーの設定値接続、主軸量・折り返し可否を分ける内部helper。
  ホイール分類・適用・予測・新キーhandler・補間の実装は未着手。
- `src/ui_fullscreen/initial_placement.rs` / `src/ui_fullscreen/initial_placement/tests.rs`: `kind`、読み終わり、設定の門、
  要求再作成のkind引継ぎ。送りの要求からの接続は未着手。
- 文書の作業差分: `docs/keymap-spec.md` / `docs/key-customization-impl-plan.md` / `docs/display-pipeline.md` /
  `docs/spec.md` / `docs/fullscreen-in-page-scroll-plan.md` / `docs/README.md` / `docs/detached-rework-plan.md`、
  `htdocs/mimageviewer/manual/fullscreen.html` / `shortcuts.html` / `settings.html`、本書。
  これらは完成後の仕様の下書きであり、設計判断後の突合せが必要。

**テスト差分**: 新しいテスト関数はまだ追加していない。`all_settings_fields_are_classified` の件数、
`all_allowed_values_roundtrip_and_all_excluded_fields_preserve` が使うfixture、既存のorderedテストhelperとinventoryを更新した。
初期配置の既存 `initial_placement_composition_request_matches_synchronous_normalize_open` と
`initial_placement_composition_switch_deferred_normalize_uses_resumed_open_zoom` のrequest literalへ `Setting` を追加した。

再開後の絞り込み候補は `fullscreen_in_page_scroll|in_page_scroll::|initial_placement|all_settings_fields_are_classified|all_allowed_values_roundtrip|search_nav_entry_inventory`。
停止理由の追加回帰は、サムネイルでTurningへ入場→同じページのロード失敗を採用→新Press / wheel→保留・再開後の着地、を
`in_page_scroll` 配下の既存handler / 描画fixtureで覆う。新規テスト・snapshot・gpu group登録は未完了。

**再開時の追加棚卸し**: 構成変更で `normalize_spread_position` が別anchorを通常openする場合、
`request_fs_initial_placement_after_composition_change` の呼出し前に要求が `Setting` へ置き換わり得る。
helper内のkind引継ぎだけでなく、再openの所有境界から保持する接続が必要。設計との差分として採用した変更はない。

**停止時の検査結果**: 引数なし `cargo fmt` は終了コード0、`python scripts/check_ui_glyphs.py` は危険グリフ0件、
`git diff --check` は終了コード0。コンパイル・テスト・snapshot生成・実機検証はすべて未実施。

### 2026-11-20: §4.5追記による再開と、コレクションの保留境界での再停止

前回の停止理由は設計担当が§4.5へ明記した条件で解消し、その決定に従って実装を再開した。
`LoadFailed` でも共通の面・世代・紙めくりbarrier・ページ送り待ちを通し、紙めくりの継続型は変更していない。

**新たな主要前提の矛盾（コード読み・独立レビューP1）**:

- `fs_in_page_scroll_surface` はコレクション直下の `GridItem::Image` を除外しない。
  `collection_navigation.rs::collection_root_navigation_origin` も `Root` の `Available Image` を受理する。
- `handle_fs_navigation_with_render_perf` は手動送りの着地付き `land` より前に、`collection_delta` を
  `start_collection_manual_display_unit_navigation` へ渡してreturnする。そこで渡すのは既存の
  `ManualMediaNavigationLanding::Fullscreen` であり、読み始め／読み終わりではない。
- `CollectionNavigationAction::Manual` → `enqueue_collection_navigation` → actorの `load_collection` の応答待ち
  (`CollectionNavigationPending::Snapshot { receiver }` 等) は独立の非同期保持者である。
  `queued_steps`、`accumulate_manual`、`AwaitingManualContinuation` も持つ。
- 再開は `open_manual_media_navigation_target` → `open_fullscreen_from_fs_navigation` の着地なし入口へ入るため、
  新操作の `FsPageLanding` を失う。既存の `manual_media_navigation_routes_collection_before_its_display_order_argument`
  も、画像2件のコレクションでこの専用送りを扱っている（今回そのテストを実行したものではない）。

§4.4.1の「保留は3経路」という前提に含まれない第4の境界であり、`OpenIntent` / `DeferredFullscreen` への
field追加だけでは補えない。コレクションの既存経路をバイパス・制限する変更はしていない。
再開には、コレクション要求／応答待ち／送りの蓄積／再開での着地の所有・置換規則を正本へ追補する必要がある。
設計担当の決定前に、独立した一時指定やフォールバックは足していない。独立レビューもread-onlyで、全体レビューは途中まで。

**この再開で実装した範囲（未コンパイル）**:

- `FsPageNavRequest`、manual分岐だけへの着地引数、固定矢印・クリック・HUD等の丸ごと上書き。
- 本の構成確認の各 `OpenIntent`、`Explicit` と通常の再開、サイドカーの両再開、入場時の初期配置kindまでの配送。
  同idxでも後続の通常送りへ古い着地が付かないよう、保留再利用の同一性に着地も含める。
- ホイール4分類、設定ズームの既存処理への合流、分類変更時・fallbackメニューでの消費、順序付き予測と適用。
  印なし失敗を受理したsegmentも、受理時のitems世代・idx・sliceを保持して遅延適用先を照合する。
- 表示専用補間、context bundleでの交換、配送先の時刻・ViewportId、比較による退役、描画pan、ナビゲータ逆算3箇所の限定。
- 構成変更でanchorが変わる再openも、既存要求の着地kindを引数で継承する接続。
- 新規テスト41件とsnapshot定義2件（既存2件も拡張）。PNG未生成。
  実GPUを作る新snapshot2件は `.config/nextest.toml` のgpu groupへ登録した。

**停止時の未完了**: コレクションの着地配送、保留・置換・世代変更・Explicit再開・スライドショー不漏れの通しテスト、
仮素材→完成素材／設定中央への変更／構成変更の着地kind回帰、補間中の実描画・ナビゲータ逆算の通し回帰、
文書の最終突合せとLoadFailed条件の追従、全体の独立レビュー。PDFズームfixtureの要求の事前発生も未確認。
今回の機能差分は未完成で、全§9を満たしたとの報告ではない。

検証担当への対象フィルタ案:
`in_page_scroll::(settings_input_tests|scroll_then_turn_tests|wheel_zoom_tests|smooth_scroll_tests|context_scroll_tests|landing_tests)::`。
既存回帰は `in_page_scroll::|initial_placement|fullscreen_in_page_scroll|fullscreen_scroll_then_turn_settings_snapshot|fullscreen_wheel_zoom_settings_snapshot|settings_transfer|search_nav_entry_inventory|spread_step`。
コンパイル・テスト・ビルド・製品起動・コミットは実施していない。

**今回追加したテスト名（未実行）**:

- scroll_then_turn_tests:
  - `press_moves_to_edge_then_turns_but_repeat_stays_on_the_page`
  - `fitting_page_turns_on_press_and_uses_directional_landing`
  - `wheel_edge_arrival_blocks_same_frame_turn_and_small_segments_never_turn`
  - `terminal_failure_replaces_only_drawn_proof_and_never_loading_or_new_generation`
  - `terminal_failure_does_not_bypass_navigation_sequence_or_surface`
  - `terminal_failure_during_resident_thumbnail_turn_is_consumed_without_continuation`
  - `fractional_wheel_advances_primary_but_cannot_wrap_to_another_column`
  - `all_scroll_amounts_finish_n_and_z_in_both_directions_without_repeating_wraps`
  - `terminal_raw_failure_without_failed_fs_cache_is_still_navigable`

- smooth_scroll_tests:
  - `default_is_instant_and_smooth_writes_final_pan_before_interpolating_paint`
  - `repeated_scroll_accumulates_from_logical_target_and_restarts_from_visible_pan`
  - `trackpad_writer_is_instant_and_retires_an_existing_animation`
  - `changed_pan_is_used_immediately_and_zoom_rotation_or_disabled_setting_retires`
  - `equal_geometry_in_a_different_viewport_cannot_retain_animation`
  - `resized_or_changed_dpi_viewport_retires_without_requiring_a_new_paint`
  - `interpolation_does_not_request_material_or_pdf_producers`

- context_scroll_tests:
  - `page_scroll_animation_context_roundtrip_preserves_both_owners`
  - `sibling_temporary_mount_does_not_retire_the_active_scroll_animation`

- settings_input_tests:
  - `scroll_extension_settings_missing_fields_keep_released_defaults`
  - `scroll_extension_settings_roundtrip_and_clamp_only_the_ordered_step`
  - `scroll_extension_transfer_roundtrips_each_choice_and_preserves_missing_fields`
  - `scroll_extension_transfer_rejects_invalid_types_ranges_and_unknown_values`
  - `scroll_or_turn_actions_are_unassigned_image_only_and_ini_roundtrips`
  - `consumed_scroll_action_reports_egui_press_or_repeat_once_without_hold_reads`
  - `consumed_scroll_action_respects_keyboard_ownership_before_consuming`
  - `consumed_scroll_action_reports_native_kind_and_claims_the_egui_twin`
  - `preferences_n_scroll_amount_and_smooth_checkbox_ok_save_then_key_use_the_new_values`
  - `preferences_wheel_zoom_ok_save_then_handler_zooms_instead_of_turning`
  - `preferences_scroll_extension_search_results_jump_to_the_actual_setting_anchors`
  - `image_help_explains_all_four_wheel_settings_and_assigned_scroll_or_turn_actions`

- wheel_zoom_tests:
  - `setting_zoom_matches_ctrl_cursor_pivot_clamp_and_actual_painted_transform`
  - `setting_zoom_pdf_uses_the_same_resolution_demand_as_ctrl_without_a_worker`
  - `setting_zoom_real_seek_hover_turns_and_seek_drag_preserves_its_owner`
  - `setting_zoom_real_alt_shift_and_fractional_threshold_keep_existing_meanings`
  - `setting_zoom_real_fallback_menu_consumes_without_zoom_or_navigation`
  - `setting_zoom_outside_its_surface_matches_the_legacy_real_handler`
  - `setting_zoom_prediction_and_same_frame_click_use_the_applied_zoom`
  - `accepted_setting_zoom_after_alt_media_navigation_is_consumed_without_reinterpretation`
  - `default_real_wheel_still_turns_pages_without_starting_zoom_or_animation`

- landing_tests:
  - `manual_navigation_lands_at_the_reading_corner_single`
  - `manual_navigation_lands_at_the_reading_corner_ltr`
  - `manual_navigation_lands_at_the_reading_corner_rtl`
  - `manual_navigation_lands_at_the_reading_corner_split_ltr`
  - `manual_navigation_lands_at_the_reading_corner_split_rtl`
  - `same_image_split_landing_preserves_zoom_and_the_fitting_axis`

Snapshot追加: `fullscreen_scroll_then_turn_settings_snapshot_light` / `fullscreen_wheel_zoom_settings_snapshot_dark`。既存 `fullscreen_in_page_scroll_settings_snapshot_light_default` / `fullscreen_in_page_scroll_settings_snapshot_dark_enabled` を拡張。

**今回の変更ファイル（前回WIPコミット以後）**:

- `.config/nextest.toml`
- `docs/fullscreen-scroll-page-turn-plan.md`
- `src/app.rs`
- `src/app/book_composition_permission.rs`
- `src/app/book_composition_permission_tests.rs`
- `src/app/sidecar_restore.rs`
- `src/app/tests.rs`
- `src/app/viewer_context_registry.rs`
- `src/keymap.rs`
- `src/settings_transfer.rs`
- `src/ui_conceal.rs`
- `src/ui_crop.rs`
- `src/ui_dialogs/preferences/pages.rs`
- `src/ui_erase.rs`
- `src/ui_fullscreen.rs`
- `src/ui_fullscreen/depth_slideshow_tests.rs`
- `src/ui_fullscreen/depth_tests.rs`
- `src/ui_fullscreen/in_page_scroll.rs`
- `src/ui_fullscreen/in_page_scroll/context_scroll_tests.rs`
- `src/ui_fullscreen/in_page_scroll/handler_tests.rs`
- `src/ui_fullscreen/in_page_scroll/landing_tests.rs`
- `src/ui_fullscreen/in_page_scroll/scroll_then_turn_tests.rs`
- `src/ui_fullscreen/in_page_scroll/settings_input_tests.rs`
- `src/ui_fullscreen/in_page_scroll/smooth_scroll_tests.rs`
- `src/ui_fullscreen/in_page_scroll/wheel_zoom_tests.rs`
- `src/ui_fullscreen/initial_placement/tests/viewport_input/canvas_click.rs`
- `src/ui_fullscreen/manual_nav_wrap_tests.rs`
- `src/ui_fullscreen/slideshow_settings_tests.rs`
- `src/ui_fullscreen/spread_step.rs`
- `src/ui_fullscreen/spread_step_tests.rs`
- `src/ui_sns_split.rs`

**今回の停止時検査**: 引数なし `cargo fmt` 終了0、`python scripts/check_ui_glyphs.py` 危険グリフ0件、`git diff --check` 終了0。テストは未実行、型の整合・期待値・実機挙動は未確認。

### 2026-11-20: §4.4.1の6bによる再開・実装とテスト記述の完了（未コンパイル・未実行）

設計担当の6bの決定と、同形の保持者は既存の一致・置換・破棄規則に従って接続してよいという利用者の指示に従った。
下記はコードとテストを記述した記録であり、実行成功の証拠ではない。

**完了した接続**:

- `FsPageNavRequest` をキー・ホイール・固定矢印・クリック・HUDまで運び、通常の送りの上書きで着地も丸ごと取り替える。
  手動の送りだけが着地を持ち、同じ呼び出しのスライドショーへ漏らさない。
- `OpenIntent`、`DeferredFullscreen`、構成変更の再open、`Explicit`と通常の再開から、着地付きの入場と初期配置kindまで接続した。
- コレクションの保持者は `CollectionNavigationAction::Manual` と `CollectionNavigationPending::AwaitingManualContinuation`。
  requestを保持する他のpending状態は前者を通じて保持する。`page_landing` を各所有者の1 fieldとし、既存の
  `ManualMediaNavigationLanding`（開き方）とは区別する。蓄積の一致、wrap変更時のsteps置換、queue再開、世代破棄、最終入場へ運ぶ。
  根拠は§4.4.1の6bであり、今回これ以外の新しい独立した保留状態は足していない。
- ホイールは4分類を受付・予測・適用で共有し、設定ズームを既存Ctrlズームへ合流させた。分類が変わった入力は消費だけにする。
  移動済み判定と送りの予測はframe内の順序を保つ。失敗ページの例外は描画印の条件だけに適用する。
  印なしで受けた遅延入力もitems世代・idx・sliceを保持し、印のOption全体を照合する。途中でサムネイルの印が生じても
  受理済みの送りをスクロールへ再解釈しない（消費のみ、次の新入力で新しい印を使う）。
- `fs_pan` を最終位置とする表示専用補間をcontext bundleへ載せた。配送先ViewportIdと入力時刻、1箇所の比較による退役、
  補間中だけの再描画要求を使う。拡大のROIは既存の毎frameの可視域をそのまま使い、追加の素材producerは作らない。
  ナビゲータ3箇所は補間ownerが保持するtransform自身のpanを使う。描画から入力までに0.2秒をまたぐ場合も同じ基準で逆算する。
- 設定のUI編集→OK→保存→実handlerの効果、初期配置の仮素材→完成素材、実描画、別context、保留再開の回帰を記述した。
  新しいテストはすべて `in_page_scroll` 配下に置いた。private fixtureを使うコレクション／本の構成／サイドカーのテストは
  その所有moduleからincludeまたはpath指定しており、テストの完全名も所有module側になる。
- 環境設定snapshotの既存2件を拡張し、新規2件を定義した。新規の実GPU使用snapshot2件をgpu groupへ登録した。
  headlessな実handler・描画テストはdeviceを作らないためgpu groupへは追加していない。PNGの生成・閲覧・比較は設計担当へ引き継ぐ。
- キー仕様・表示仕様・context所有・コレクション仕様・UI snapshot方針・マニュアルを最終突合せした。
  detachedの述語、viewportの配送先選択、HWNDの契約は変更していない。

**設計との差分と理由**: §4.5と6bの追記を含む正本の動作仕様からの変更はない。
描画印のOption全体の照合、補間期限をまたぐナビゲータの基準、通常APIをNone wrapperとして残す接続は、
それぞれ受理済み入力の所有、実際に描いたtransform、既存の着地なし呼び出しを維持するための実装上の具体化である。
§4.4.1の7にある保留時のslice問題は修正対象を広げず、通常の送りとの表示先比較を回帰条件にした。

**独立レビュー**: GPT-6.1 Sol / xhighによるread-onlyの最終実装レビューで未解決P1/P2なし。
コレクションの着地欠落、補間期限をまたぐナビゲータ、新しい印による遅延ホイール再解釈の指摘は修正後に再確認した。
型・実行時の整合が確認済みであるとは主張しない。

**追加テスト名（新規12ファイルの75件、すべて未実行）**:

- scroll_then_turn_tests:
  - `press_moves_to_edge_then_turns_but_repeat_stays_on_the_page`
  - `fitting_page_turns_on_press_and_uses_directional_landing`
  - `wheel_edge_arrival_blocks_same_frame_turn_and_small_segments_never_turn`
  - `terminal_failure_replaces_only_drawn_proof_and_never_loading_or_new_generation`
  - `terminal_failure_does_not_bypass_navigation_sequence_or_surface`
  - `terminal_failure_during_resident_thumbnail_turn_is_consumed_without_continuation`
  - `fractional_wheel_advances_primary_but_cannot_wrap_to_another_column`
  - `all_scroll_amounts_finish_n_and_z_in_both_directions_without_repeating_wraps`
  - `terminal_raw_failure_without_failed_fs_cache_is_still_navigable`

- smooth_scroll_tests:
  - `default_is_instant_and_smooth_writes_final_pan_before_interpolating_paint`
  - `repeated_scroll_accumulates_from_logical_target_and_restarts_from_visible_pan`
  - `trackpad_writer_is_instant_and_retires_an_existing_animation`
  - `changed_pan_is_used_immediately_and_zoom_rotation_or_disabled_setting_retires`
  - `equal_geometry_in_a_different_viewport_cannot_retain_animation`
  - `resized_or_changed_dpi_viewport_retires_without_requiring_a_new_paint`
  - `interpolation_does_not_request_material_or_pdf_producers`

- context_scroll_tests:
  - `page_scroll_animation_context_roundtrip_preserves_both_owners`
  - `sibling_temporary_mount_does_not_retire_the_active_scroll_animation`
  - `landing_kind_and_items_generation_follow_only_their_context_owner`

- settings_input_tests:
  - `scroll_extension_settings_missing_fields_keep_released_defaults`
  - `scroll_extension_settings_roundtrip_and_clamp_only_the_ordered_step`
  - `scroll_extension_transfer_roundtrips_each_choice_and_preserves_missing_fields`
  - `scroll_extension_transfer_rejects_invalid_types_ranges_and_unknown_values`
  - `scroll_or_turn_actions_are_unassigned_image_only_and_ini_roundtrips`
  - `consumed_scroll_action_reports_egui_press_or_repeat_once_without_hold_reads`
  - `consumed_scroll_action_respects_keyboard_ownership_before_consuming`
  - `consumed_scroll_action_reports_native_kind_and_claims_the_egui_twin`
  - `preferences_n_scroll_amount_and_smooth_checkbox_ok_save_then_key_use_the_new_values`
  - `preferences_wheel_zoom_ok_save_then_handler_zooms_instead_of_turning`
  - `preferences_scroll_extension_search_results_jump_to_the_actual_setting_anchors`
  - `image_help_explains_all_four_wheel_settings_and_assigned_scroll_or_turn_actions`

- wheel_zoom_tests:
  - `setting_zoom_matches_ctrl_cursor_pivot_clamp_and_actual_painted_transform`
  - `setting_zoom_pdf_uses_the_same_resolution_demand_as_ctrl_without_a_worker`
  - `setting_zoom_real_seek_hover_turns_and_seek_drag_preserves_its_owner`
  - `setting_zoom_real_alt_shift_and_fractional_threshold_keep_existing_meanings`
  - `setting_zoom_real_fallback_menu_consumes_without_zoom_or_navigation`
  - `setting_zoom_outside_its_surface_matches_the_legacy_real_handler`
  - `setting_zoom_prediction_and_same_frame_click_use_the_applied_zoom`
  - `accepted_setting_zoom_after_alt_media_navigation_is_consumed_without_reinterpretation`
  - `default_real_wheel_still_turns_pages_without_starting_zoom_or_animation`
  - `setting_zoom_media_items_stay_outside_the_ordinary_image_classifier`

- landing_tests:
  - `manual_navigation_lands_at_the_reading_corner_single`
  - `manual_navigation_lands_at_the_reading_corner_ltr`
  - `manual_navigation_lands_at_the_reading_corner_rtl`
  - `manual_navigation_lands_at_the_reading_corner_split_ltr`
  - `manual_navigation_lands_at_the_reading_corner_split_rtl`
  - `same_image_split_landing_preserves_zoom_and_the_fitting_axis`
  - `landing_kind_survives_provisional_material_and_center_setting_until_full_arrival`
  - `changing_global_start_position_retires_only_setting_requests`
  - `landing_end_survives_composition_reanchor_and_is_consumed_on_the_new_unit`
  - `manual_pan_retires_provisional_landing_before_full_material_arrives`
  - `scroll_turn_boundaries_and_wrap_use_the_existing_units_and_directional_arrival`

- navigation_landing_tests:
  - `a_manual_boundary_does_not_attach_its_landing_to_a_due_slideshow`

- permission_landing_tests:
  - `permission_navigation_explicit_and_inner_open_resume_with_the_exact_landing`
  - `permission_replacement_and_generation_retirement_discard_the_old_landing`

- sidecar_landing_tests:
  - `sidecar_live_and_index_only_resume_with_the_retained_landing`
  - `sidecar_replacing_an_open_or_its_generation_cannot_retain_a_previous_landing`

- collection_scroll_tests:
  - `collection_scroll_accumulator_matches_landing_and_direction_in_both_owners`
  - `collection_scroll_actor_queue_preserves_start_and_end_until_the_final_entry`
  - `collection_scroll_different_landing_replaces_the_pending_request`
  - `collection_scroll_wrap_replacement_replaces_the_page_landing_together_with_steps`
  - `collection_scroll_generation_change_discards_both_request_and_continuation_landing`

- scroll_turn_handler_tests:
  - `real_wheel_prediction_arrives_at_edge_and_next_frame_turns_with_landing`
  - `real_deferred_wheel_turn_consumes_later_segments_without_moving_target_page`
  - `deferred_scroll_turn_drops_old_page_and_generation_and_respects_front_consumption`
  - `real_scroll_turn_key_consumes_repeat_but_never_turns_until_a_fresh_press`
  - `scroll_turn_keys_are_left_unconsumed_outside_surface_and_front_ui`
  - `fixed_arrow_overwrites_the_whole_scroll_turn_request_in_the_same_key_pass`
  - `click_overwrites_the_whole_scroll_turn_wheel_request_without_landing`
  - `ordered_step_setting_does_not_change_continuous_page_up_or_page_down`
  - `failed_page_deferred_turn_is_not_reinterpreted_after_a_thumbnail_receives_a_stamp`

- smooth_render_tests:
  - `ordinary_single_and_spread_paint_interpolate_without_publishing_intermediate_pan`
  - `asymmetric_spread_wrap_paint_keeps_final_pan_and_existing_material_producers`
  - `actual_trackpad_handler_is_instant_for_both_page_scroll_settings`
  - `navigator_double_click_uses_the_painted_pan_during_scroll_interpolation`
  - `navigator_range_zoom_uses_the_painted_pan_during_scroll_interpolation`
  - `navigator_pending_center_draw_uses_the_painted_pan_during_scroll_interpolation`
  - `navigator_baseline_without_animation_or_with_z_keeps_the_logical_pan`
  - `actual_pdf_scroll_paint_adds_no_resolution_or_processing_request`

Snapshot新規: `fullscreen_scroll_then_turn_settings_snapshot_light` / `fullscreen_wheel_zoom_settings_snapshot_dark`。
変更: `fullscreen_in_page_scroll_settings_snapshot_light_default` / `fullscreen_in_page_scroll_settings_snapshot_dark_enabled`。
既存テストの変更は、転送件数と全field fixture、KeyAction inventory、初期配置kind、typed requestへの置換、
実handler描画fixtureでの失敗ページのresident thumbnailを使う接続である。元の機能の期待値は維持した。

**今回の変更ファイル（前回WIPコミット以後。最初のWIPに含まれる設定・検索・ヘルプ等は上の履歴を参照）**:

- `.config/nextest.toml`
- `docs/collection-playback-plan.md`
- `docs/detached-rework-plan.md`
- `docs/display-pipeline.md`
- `docs/fullscreen-scroll-page-turn-plan.md`
- `docs/gpu-test-groups.md`
- `docs/key-customization-impl-plan.md`
- `docs/keymap-spec.md`
- `docs/spec.md`
- `docs/ui-snapshot-policy.md`
- `htdocs/mimageviewer/manual/fullscreen.html`
- `htdocs/mimageviewer/manual/shortcuts.html`
- `src/app.rs`
- `src/app/book_composition_permission.rs`
- `src/app/book_composition_permission_tests.rs`
- `src/app/collection_navigation.rs`
- `src/app/sidecar_restore.rs`
- `src/app/tests.rs`
- `src/app/top_level_grid_view.rs`
- `src/app/viewer_context_registry.rs`
- `src/keymap.rs`
- `src/settings_transfer.rs`
- `src/ui_conceal.rs`
- `src/ui_crop.rs`
- `src/ui_dialogs/preferences/pages.rs`
- `src/ui_erase.rs`
- `src/ui_fullscreen.rs`
- `src/ui_fullscreen/depth_slideshow_tests.rs`
- `src/ui_fullscreen/depth_tests.rs`
- `src/ui_fullscreen/in_page_scroll.rs`
- `src/ui_fullscreen/in_page_scroll/collection_scroll_tests.rs`
- `src/ui_fullscreen/in_page_scroll/context_scroll_tests.rs`
- `src/ui_fullscreen/in_page_scroll/handler_tests.rs`
- `src/ui_fullscreen/in_page_scroll/landing_tests.rs`
- `src/ui_fullscreen/in_page_scroll/navigation_landing_tests.rs`
- `src/ui_fullscreen/in_page_scroll/permission_landing_tests.rs`
- `src/ui_fullscreen/in_page_scroll/scroll_then_turn_tests.rs`
- `src/ui_fullscreen/in_page_scroll/scroll_turn_handler_tests.rs`
- `src/ui_fullscreen/in_page_scroll/settings_input_tests.rs`
- `src/ui_fullscreen/in_page_scroll/sidecar_landing_tests.rs`
- `src/ui_fullscreen/in_page_scroll/smooth_render_tests.rs`
- `src/ui_fullscreen/in_page_scroll/smooth_scroll_tests.rs`
- `src/ui_fullscreen/in_page_scroll/wheel_zoom_tests.rs`
- `src/ui_fullscreen/initial_placement.rs`
- `src/ui_fullscreen/initial_placement/tests.rs`
- `src/ui_fullscreen/initial_placement/tests/viewport_input/canvas_click.rs`
- `src/ui_fullscreen/manual_nav_wrap_tests.rs`
- `src/ui_fullscreen/page_turn/input_regression_tests.rs`
- `src/ui_fullscreen/slideshow_settings_tests.rs`
- `src/ui_fullscreen/spread_step.rs`
- `src/ui_fullscreen/spread_step_tests.rs`
- `src/ui_fullscreen/tests/still_seek_menu.rs`
- `src/ui_sns_split.rs`

**設計担当へ引き継ぐ絞り込み**:

- 新規headless群: `in_page_scroll::(settings_input_tests|scroll_then_turn_tests|wheel_zoom_tests|smooth_scroll_tests|context_scroll_tests|landing_tests|navigation_landing_tests|scroll_turn_handler_tests|smooth_render_tests)::`
- 保留所有者の通し: `app::(book_composition_permission::scroll_landing_tests|sidecar_restore::scroll_landing_tests|collection_navigation::tests::scroll_landing_tests)::`
- 既存回帰と設定UI: `in_page_scroll::|initial_placement|fullscreen_in_page_scroll|fullscreen_scroll_then_turn_settings_snapshot|fullscreen_wheel_zoom_settings_snapshot|settings_transfer|search_nav_entry_inventory|spread_step|navigator_zoom_mode|manual_nav_wrap|page_turn::input_regression_tests`

これらは正規表現の案であり、コンパイル後に列挙された完全名へ合わせて選択する。実GPUを使うsnapshotはnextestのgpu groupで行う。
PNG生成、実機での入力と別ウィンドウ、補間中の負荷・画質は未確認。ビルド・テスト・check・clippy・nextest・
build-dev・製品起動・コミットは利用者の指示により実施していない。

**最終検査**: 引数なし `cargo fmt` は終了コード0、`python scripts/check_ui_glyphs.py` は危険グリフ0件、
`git diff --check` は終了コード0。新規12ファイルのテスト関数が75件あることをファイル読取で確認した。
コンパイル・テスト・snapshot生成・製品起動・コミットは行っていない。

### 関連1552テストの報告に基づくfixture修正（2026-11-20、再実行待ち）

設計担当からrun-heavy経由の実行結果として1543成功・9失敗の報告を受けた。4件はsnapshotのPNG未生成／既存寸法変更で、
PNG生成・目視は設計担当が行う。残り5件は製品コードと照合し、いずれもテストの前提・fixtureの誤りと判断して修正した。
本記録の実行結果は設計担当の報告であり、実装担当がコンパイル・テストを行ったものではない。

| テスト | 判断の根拠と修正 |
| --- | --- |
| `manual_navigation_lands_at_the_reading_corner_single` / `manual_navigation_lands_at_the_reading_corner_ltr` / `manual_navigation_lands_at_the_reading_corner_rtl` / `manual_navigation_lands_at_the_reading_corner_split_ltr` / `manual_navigation_lands_at_the_reading_corner_split_rtl`（分割前は1本） | 実描画fixtureがReady→Presentingの選択と実receiptの完了通知を省略し、描画後もnavigation sequenceが残った。`handler_tests::paint` から通常の `fs_nav_holdover_for_draw` と `emit_fs_page_turn_ready_for_display_unit` を呼ぶ。描画印・入力幾何・barrierの照合assertは維持。 |
| `scroll_turn_boundaries_and_wrap_use_the_existing_units_and_directional_arrival` | Singleのrouterは端でもDeltaを返し、境界／wrapを後段handlerで解決する。SingleはDelta、見開き・splitは従来のBoundary／Wrapを厳密比較し、routerとの全値比較・最終到着idx／slice／角の検証も維持。 |
| `consumed_scroll_action_respects_keyboard_ownership_before_consuming` | 直接keymapの既存S3互換gateはModalを遮断せず、App外側がmodalを扱う。直接keymapを遮断するFocusedUiをfixtureへ指定し、未消費→ApplicationShortcutへの所有移行→1回消費を検証。 |
| `real_scroll_turn_key_consumes_repeat_but_never_turns_until_a_fresh_press` | 初回tapでrelease済みなのにrepeat=trueだけを送っていた。eguiはheld-key集合からrepeatを再計算するためInitialPressになっていた。press-onlyでheldを維持し、実際の正規化後repeat、消費のみ・pan／idx不変、release後の新Pressで着地付き送りを検証。 |
| `asymmetric_spread_wrap_paint_keeps_final_pan_and_existing_material_producers` | 1000×2200のfixture素材がheadlessのtexture side上限2048を超えた。素材を500×1100／500×350へ半分にし、倍率2で縦長／短いページと描画上のはみ出しを維持。補間の最終pan固定・描画pan・producer不増加のassertは維持。 |

§4.2の製品経路も確認した。`consume_chord_press_input` が消費したedgeのkindを返し、
`fs_page_scroll_or_turn_key` の `allow_turn` はInitialPressだけ。`ordered_scroll_outcome` は移動できずallow_turn=falseならBlockedで、
LoadFailedの例外も同じallow_turnの門を通る。Repeatでページを送る分岐はなく、製品の動作変更はしていない。

環境設定snapshotの540×220→540×340は、滑らかな移動の設定・説明、移動量の欄・説明を追加したための意図した変更。
共有fixtureの高さ340を維持し、描画前の利用可能下端を保存して内容の下端が収まるassertを4件共通で追加した。
今回変更したファイルは、本書、`src/ui_dialogs/preferences/pages.rs`、`src/ui_fullscreen/in_page_scroll/` 内の
`handler_tests.rs`、`landing_tests.rs`、`settings_input_tests.rs`、`scroll_turn_handler_tests.rs`、`smooth_render_tests.rs`。
直前のコンパイルエラー修正の差分は維持した。設計との差分、新しい固定待ち・OSキー状態の読取、assertの緩和はない。
修正後のテスト成否とsnapshotの実寸・描画は設計担当の再実行・目視待ち。

### 独立実装レビュー r1: 補間退役の順序の修正（P1）

- **製品の不具合**: `render_fullscreen_viewport_with_root_input` は入力後に `drawn_unit` を消し、画像描画の直前で
  `retire_fs_page_scroll_animation` を呼んでいた。照合は前の描画印と `animation.origin` の一致を要求するため、
  有効な補間も必ず失効した。低レベル描画のテストは手動で逆の順に呼んでおり、この実経路を検証していなかった。
- **修正と順序の根拠**: `src/ui_fullscreen.rs` のキー・ホイール入力処理後、29149行で退役を1回だけ判定し、
  29150行で従来どおり印を消す。ここが、入力を反映しつつ前frameの印を比較できる最後の位置である。
  29223行のlayout全体のclearから各描画のbeginを経て、29801行の実表示単位の描画印の再設定へ進む。
  interlude等で通常描画へ進まないときも、従来と同じ場所で印を無効にする。印の意味、入力時の照合、
  layoutのclear／begin／再設定の順序は変えない。新しい退役箇所、repaint、猶予、retryは足していない。
- **回帰テストの置換**: `ui_fullscreen::in_page_scroll::smooth_render_tests::ordinary_single_and_spread_paint_interpolate_without_publishing_intermediate_pan`
  を同名のまま、既存handler fixtureから `ctx.root_ui` → `render_fullscreen_viewport_with_root_input` を直接呼ぶ形へ置き換えた。
  embeddedの実経路を使うWindows headlessテストで、Single／Ltr／Rtl × 設定OFF／ONを検査する。
  egui入力時刻0.0で初期描画、1.0でF24に割り当てたスクロールキーを1回配送し、1.05／1.10／1.15／1.21で描画する。
  ONの入力frameはfrom、途中はfrom/to間を単調に進み、0.2秒後はtoで補間が退役すること、OFFは入力frameからtoであること、
  論理panは常にto、各frameで印が再設定され次の入力の照合が通ることをassertする。
  テストからapply／retire／低レベルpaintを並べて順序を代用しない。実GPU renderer、製品起動、固定待ち、OSキー状態の読取は追加していない。
- **P3の説明訂正**: §3.1は、旧版が設定を保存しなければ新しい値が残り、旧版で保存されると (自動保存を含む)
  既定のPageTurnが書き戻されて新版でも元へ戻らない、と訂正した。列挙の正規化・保存のコードは変更していない。
- **変更ファイル**: `src/ui_fullscreen.rs`、`src/ui_fullscreen/in_page_scroll/smooth_render_tests.rs`、本書。
  開始時に存在した環境設定snapshotのPNG4件は設計担当の差分として維持した。
  コンパイル・テスト・check・nextest・製品起動・コミットは実施しておらず、修正後の実行結果と独立実装レビューの再受入は設計担当へ引き継ぐ。

### 独立実装レビュー r2: ROOT配送時の補間clockの修正（P2）

- **原因と選んだ方式**: ROOTでキーを処理するとき、配送先の `ctx.input_for(viewport).time` は前回描画の値である。
  配送先の最終描画10秒・ROOT入力11秒では開始が10秒になり、直後の描画で0.2秒の期限が過ぎていた。
  方式(b)として、処理中viewportの現在の `ctx.input(|input| input.time)` を共通clockのsampleとして使う。
- **同じclockである根拠（コード読み）**: `vendor/eframe/src/native/epi_integration.rs:218` は `EpiIntegration` がappごとに1つと明記し、
  222行の `beginning` を所有する。wgpuのROOT／通常の子は `wgpu_integration.rs:922` でその `beginning.elapsed()` を
  RawInput.timeへ設定する。即時viewportも543行で同じInstantをcaptureし、1715行でそれを使う。
  Glowも `glow_integration.rs:796`、486→2371行で同じ構造である。`epi_integration.rs:546` の入力準備でも同じclockを設定し、
  `src/app.rs:103167` のraw_input_hookは時刻を変更しない。`vendor/egui/src/input_state/mod.rs:375` は渡されたtimeを採用する。
  egui-winit内部のviewport別のstart_timeを比較する方式ではない。
- **変更**: `in_page_scroll.rs` の `fs_page_scroll_animation_clock` へ既存の配送先ViewportIdと現在時刻の取得を集約した。
  writerの開始時刻と既存補間の `from = pan_at(now)`、退役時の期限判定、描画panのsamplingを同じ基準へそろえた。
  start_timeはf64のままで、新しい保留状態は足していない。退役はr1修正後の描画印消去前の1か所を維持する。
  detachedの述語・viewportの配送・HWND・context swap・repaint回数・期限は変更せず、猶予・retry・固定待ちは追加していない。
- **追加テスト（Windows headless、未実行）**:
  - `ui_fullscreen::in_page_scroll::smooth_render_tests::forwarded_root_scroll_starts_at_current_time_and_paints_in_the_destination`
  - `ui_fullscreen::in_page_scroll::smooth_render_tests::forwarded_root_scroll_retargets_from_current_time_instead_of_cached_child_time`
  既存handler fixtureのresident素材と既存の即時viewport用RawInput callbackの構造を使い、callbackを別threadへ隔離する。
  実 `handle_fullscreen_root_key_input` → 実 `render_fullscreen_viewport` → 別viewportの描画を通し、Single／Ltr／Rtlで検査する。
  配送先最終描画10秒のままROOTで11秒に入力し、11.05秒の配送先描画がfrom/toの厳密中間、11.21秒がtoで退役することをassertする。
  再入力は配送先のcached時刻11.05とROOTの現在時刻11.10の違いを保持し、fromが旧補間の11.10時点 (ease=0.875) であって
  11.05時点 (ease=0.578125) ではないこと、再設定から0.2秒後にtoへ到達・退役することをassertする。
  実GPU renderer・native windowを作らないheadlessのため、gpu test-groupへの追加は不要。実機のHWND配送の確認とは区別する。
- **変更ファイルと検証の限界**: `src/ui_fullscreen/in_page_scroll.rs`、`src/ui_fullscreen/in_page_scroll/smooth_render_tests.rs`、本書の§5と本項。
  開始時に存在したsnapshotのPNG4件は維持した。実装担当はコンパイル・テスト・check・nextest・製品起動・コミットを行っていない。
  修正後のテスト成否と実機の入力・描画は設計担当の検証待ち。

### 独立実装レビュー r3: 補間の時刻を配送先描画passへ限定

- **規則を変えた理由**: r2で確認したeframeの共通clockという根拠は正しいが、子からROOTへ戻ると現在passのsampleは古くなる。
  遅延ホイールの目標更新が古い時刻へfromを巻き戻すため、経路ごとのclock選択を撤回し、利用者決定どおり入力から補間時刻を完全に分離した。
- **単一ownerの状態**: `FsPageScrollAnimation.start_time` を `Option<f64>` にし、`last_painted_pan: Vec2` を同じ構造体へ追加。
  writerは時計を読まず、時刻を使わない照合が通る旧ownerの最後の実描画panをfromにし、通らなければfs_panを使う。新要求の開始はNone。
  入力で期限は判定せず、`fs_page_scroll_animation_clock` は撤去した。最後のpanは、現在の配送先で実単位の描画印を再設定でき、
  ownerのviewport・目標・印が一致する描画で、その実transformから記録する。pan_atを入力側から呼ばない。
- **配送先passである根拠**: `src/ui_fullscreen.rs:28397–28398` のrender_fs_bodyは引数UIのContextを使う。
  embeddedは30874行でROOTのUIを渡し、非embeddedは30886–30887行でshow_viewport_immediateの配送先UIを渡す。
  退役判定29149行はこのbody内で、旧印を消す前に1回だけ呼ぶ。`retire_fs_page_scroll_animation` 自体も現在の配送先とctx.viewport_idを比較し、
  配送先でないpassでは開始・期限判定も変更もせず残す。配送先では有効な要求の初回をinput.timeで確定し、以後0.2秒の期限をそこでだけ見る。
  描画panも開始確定済みかつ配送先の描画passでのみ時刻から計算し、それ以外のpassで補間を描かない。
  31085行以降のROOTでの遅延ホイール適用は、新しい未開始の目標へ置き換えるだけになる。
- **context所有と退役**: `ViewerContextBundle` の2119行のOptionが構造体全体を所有し、4164行の既存swapが最後のpanもまとめて交換する。
  新しいApp-global状態・別pending fieldは追加していない。兄弟contextの一時mount、closeの既存owner破棄を維持した。
  closeのunit fixtureはidxだけの代入から実close_fullscreenへ寄せた。idxだけを消すと配送先も変わるため、旧ROOTのretireに実closeの代用をさせない。
- **追加テスト**:
  - `smooth_render_tests::deferred_child_wheel_uses_last_paint_after_returning_to_an_older_root_clock`
  - `smooth_scroll_tests::input_only_retargeting_neither_starts_nor_expires_the_effect`
  - `smooth_scroll_tests::non_destination_pass_cannot_start_or_expire_the_effect`
  遅延ホイールは実rendererでROOT11.06→子11.15の順を通し、子の実中間描画後にROOTで適用されたfromがその描画panになること、開始None、
  次の子描画で開始・その後の中間と0.2秒後の到達／退役を検査する。入力のみのテストは1→100→1000秒と進めても未開始を保持し、
  開始済みの旧期限を超えた新入力も最後の実描画位置から未開始に戻す。非配送先では未開始・開始済みの両方を保持し、補間panを使わないことを検査する。
- **変更したROOT回帰（新名）**:
  - `forwarded_root_scroll_starts_at_current_time_and_paints_in_the_destination`
    → `forwarded_root_scroll_waits_for_first_destination_paint_before_starting_animation`
  - `forwarded_root_scroll_retargets_from_current_time_instead_of_cached_child_time`
    → `forwarded_root_scroll_retargets_from_the_last_actual_destination_paint`
  いずれも `smooth_render_tests` に所属し、ROOT入力時のNone、最初の配送先描画での開始、最後に実描画したpanからの再設定を検査する。
- **既存テストの追従（名称維持）**:
  - `smooth_render_tests::ordinary_single_and_spread_paint_interpolate_without_publishing_intermediate_pan`
  - `smooth_render_tests::navigator_baseline_without_animation_or_with_z_keeps_the_logical_pan`
  - `smooth_scroll_tests::default_is_instant_and_smooth_writes_final_pan_before_interpolating_paint`
  - `smooth_scroll_tests::repeated_scroll_accumulates_from_logical_target_and_restarts_from_visible_pan`
  - `smooth_scroll_tests::trackpad_writer_is_instant_and_retires_an_existing_animation`
  - `smooth_scroll_tests::changed_pan_is_used_immediately_and_zoom_rotation_or_disabled_setting_retires`
  - `smooth_scroll_tests::interpolation_does_not_request_material_or_pdf_producers`
  - `context_scroll_tests::page_scroll_animation_context_roundtrip_preserves_both_owners`
  - `context_scroll_tests::sibling_temporary_mount_does_not_retire_the_active_scroll_animation`
  - `context_scroll_tests::landing_kind_and_items_generation_follow_only_their_context_owner`
  Single／Ltr／RtlのON／OFFを実frame描画で検査する経路は維持し、Option開始と最後の実描画pan・context swapを追従した。
- **変更ファイル**: `src/ui_fullscreen/in_page_scroll.rs`、同配下の `smooth_render_tests.rs`／`smooth_scroll_tests.rs`／`context_scroll_tests.rs`、
  本書の§5・§9と本項、`docs/display-pipeline.md`、`docs/detached-rework-plan.md` の既存所有記録。
  detachedの述語・viewportの配送・HWND・repaint回数・期限は変更せず、猶予・retry・固定待ち・実キーのOS読取を追加していない。
  開始時のsnapshot PNG4件は設計担当の差分として維持した。
- **検証の限界**: read-onlyの独立静的レビューで、入力clock撤去、描画passの所有、実pan記録、型変更箇所と回帰経路を確認した。
  コンパイル・テスト・check・nextest・製品起動・コミットは実装担当では行っていない。新しい回帰も実行結果は設計担当へ引き継ぐ。

### 独立実装レビュー r4: 毎frameのスクロール入力でも表示を進める

- **原因**: 2a5c88b5fは目標更新ごとにstart_timeをNoneへ戻した。各frameの退役判定で現在の描画時刻へ開始を設定し直すため、
  入力直後の描画は毎回phase 0となり、last_painted_panも動かなかった。利用者決定に従い、開始済みの差し替えへ最後の実描画panと時刻を対で引き継ぐ。
- **実装**: `FsPageScrollAnimation` に `last_painted_time: Option<f64>` を追加した。未実描画はNoneで、既存last_painted_panと
  同じ成功条件・同じ位置で、配送先の描画passのinput.timeをSomeとして記録する。
  入力の照合が通る開始済みownerは、from=last_painted_pan／start_time=last_painted_timeを継ぐ。
  未開始ownerはfromとNoneを保ち、新規または照合不一致はfs_pan／Noneにする。入力では時計・期限・pan_atを読まない。
  開始確定直後に実描画記録がまだない場合も時刻はNoneで、0や入力時刻を補わない。lastpan/timeは同じownerに所属し、既存bundleのOptionごとswapされる。
  期限は引き継いだ時刻から0.2秒であり、開始の確定・期限判定は既存の配送先描画passの1か所のままである。
- **追加した実renderer回帰**:
  `ui_fullscreen::in_page_scroll::smooth_render_tests::continuous_actual_scroll_input_advances_each_paint_without_restarting_at_phase_zero`。
  Single／Ltr／Rtl、Original・zoom2、240×1600のresident素材、N字10%で、端に着かない十分な縦余りを用意する。
  最初の実K入力と描画で開始後、0.01秒ごとに8frame、実Kキーを実rendererへ配送する。各frameで論理目標の更新と端の手前であること、
  描画panの厳密な前進、fs_pan未超過、前frameの実描画時刻からの開始、現在の実pan/timeの記録をassertする。
  入力停止後の中間描画、停止から0.2秒経過後の到達と退役も検査する。新GPU renderer、OS実キー読取、固定待ちは使わない。
- **旧コードを検出できる理由（静的確認）**: 旧2a5c88b5fの差し替え処理なら、最初の再入力frameでstart_timeが再びNoneとなり、
  同frameで開始=現在時刻に確定して描画panが前frameと同じになる。上記のstrict progress assertがその場で失敗する。
  途中で端へ到達して入力が実質途切れるケースを、端の手前・論理目標更新のassertで除外している。旧版へ戻して実行した証拠ではない。
- **追加した未実描画状態のunit**:
  `smooth_scroll_tests::retirement_without_a_paint_receipt_does_not_invent_a_retarget_time`。
  退役判定だけで開始が確定しても実paintの記録がなければlast_painted_timeはNoneで、再入力で架空の時刻を作らないことを確認する。
- **既存回帰の追従（名称維持）**:
  - `smooth_render_tests::forwarded_root_scroll_waits_for_first_destination_paint_before_starting_animation`
  - `smooth_render_tests::forwarded_root_scroll_retargets_from_the_last_actual_destination_paint`
  - `smooth_render_tests::deferred_child_wheel_uses_last_paint_after_returning_to_an_older_root_clock`
  - `smooth_render_tests::navigator_baseline_without_animation_or_with_z_keeps_the_logical_pan`（fixture literalのみ）
  - `smooth_scroll_tests::default_is_instant_and_smooth_writes_final_pan_before_interpolating_paint`
  - `smooth_scroll_tests::repeated_scroll_accumulates_from_logical_target_and_restarts_from_visible_pan`
  - `smooth_scroll_tests::input_only_retargeting_neither_starts_nor_expires_the_effect`
  - `smooth_scroll_tests::non_destination_pass_cannot_start_or_expire_the_effect`
  - `context_scroll_tests::page_scroll_animation_context_roundtrip_preserves_both_owners`
  - `context_scroll_tests::sibling_temporary_mount_does_not_retire_the_active_scroll_animation`
  未開始へのROOT再入力はfrom／Noneの保持、開始済みのROOT・遅延wheelは実描画pan/timeの継承へ追従。
  ROOT再入力は短間隔11.12と長間隔12.0の両方を残し、後者も入力では期限を消費せず、次の配送先描画で期限切れになることを検査する。
  既存Single／Ltr／RtlのON／OFFと0.2秒後の到達・退役の実renderer回帰も維持している。
- **文書と変更ファイル**: `src/ui_fullscreen/in_page_scroll.rs`、同配下の `smooth_render_tests.rs`／`smooth_scroll_tests.rs`／`context_scroll_tests.rs`、
  本書の§5・§9と本項、`docs/display-pipeline.md`、`docs/detached-rework-plan.md` の所有記録を更新した。
  detachedの述語・viewportの配送・HWND・context swapの入口・repaint回数は変更していない。開始時のsnapshot PNG4件は維持した。
- **検証の限界**: read-onlyの独立静的レビューで、pan/timeの記録条件、開始の継承、入力側の時計参照禁止、context所有と連続入力回帰を確認した。
  実装担当はコンパイル・テスト・check・nextest・製品起動・コミットを行っておらず、修正後の実行結果と実機挙動は設計担当の検証待ち。

### 独立実装レビュー r1〜r5の経緯と受入結果

| 回 | 結果・指摘と対応 | 修正コミット |
| --- | --- | --- |
| r1 | REVISE。P1: 補間の退役判定を前frameの描画印の消去前へ移動。P3: 旧版で設定を保存した場合の選択値の書戻し説明を訂正。 | `b64bad909` |
| r2 | REVISE。P2: ROOT配送時に配送先の古い時刻を使う不具合を、共有clockの現在passの時刻へ変更。その後r3で方式を置換。 | `be8d881a8` |
| r3 | REVISE。P2: 子の描画後に古いROOTへ戻る遅延適用を受け、時刻の読取を配送先の描画passだけへ限定し、最後の実描画panから再設定。 | `2a5c88b5f` |
| r4 | REVISE。P2: 毎frame入力でphase 0へ戻る停止を、最後の実描画panと時刻の対を引き継ぐ形へ修正。 | `a6d87842f` |
| r5 | ACCEPT。P3は下記のとおり記録で閉じ、追加の製品修正はしない。 | 記録のみ |

r5 P3: 補間の途中で描画が0.2秒以上途切れた後に再入力すると、瞬時の移動になる。
最終位置と状態は正しいため、利用者・設計担当の決定により記録で閉じる。猶予・retry等の製品変更は追加しない。
関連1559テストすべて成功は設計担当のrun-heavy実行結果の報告であり、実装担当による実行ではない。

### 着地総当たりテストをSpreadMode別の5本へ分割

設計担当の報告では、分割前の1本が148秒かかった。lib全件の負荷下でnextestの打ち切りへ達するリスクを減らすため、
最外のSpreadModeだけを各テストへ切り出し、共通本体を `assert_manual_navigation_lands_at_the_reading_corner(mode)` にした。

- `manual_navigation_lands_at_the_reading_corner_single`
- `manual_navigation_lands_at_the_reading_corner_ltr`
- `manual_navigation_lands_at_the_reading_corner_rtl`
- `manual_navigation_lands_at_the_reading_corner_split_ltr`
- `manual_navigation_lands_at_the_reading_corner_split_rtl`

各24通り（本の向き2 × 読み順2 × 着地2 × はみ出し3）、合計120通りを維持する。
App生成・素材寸法・navigation・描画・assertの本体は変えず、fixtureの共有化や検査の削減はしていない。
本書の既存テスト名の記載も5本へ更新した。分割後の所要時間・実行結果は未確認。

同種の長時間化候補（ソース確認のみ・未計測、今回は分割しない）:

- `landing_tests::scroll_turn_boundaries_and_wrap_use_the_existing_units_and_directional_arrival`

変更ファイルは `src/ui_fullscreen/in_page_scroll/landing_tests.rs` と本書のみ。製品コードは変更していない。
開始時に存在したsnapshot PNG4件（うち2件はステージ済み）は維持した。
実装担当はコンパイル・テスト・check・nextest・製品起動・コミットを行っていない。

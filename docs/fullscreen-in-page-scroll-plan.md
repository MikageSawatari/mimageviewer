# ページ内スクロールと N 字スクロール (§1.474)

- 状態: **設計 r4・独立設計レビュー r4 で ACCEPT (P1/P2 なし、P3 4 件を反映済み)・利用者判断済み (2026-11-18、§3.2 Q3〜Q5 とも推奨案、加えて N 字 / Z 字の順序の設定を追加)・実装・対象検証完了、設計担当の受入れ待ち (§7)** (2026-11-18)。r2 の「描画時に解く」案は撤回し、入力時に照合付きで解く (§6.2・§6.3)。ブランチ `feat-474-page-scroll`。
- 要望: 5ch >>642・>>767。通常のページ単位表示で、次のページを見せずに今のページ内を矢印キーやホイールで動かしたい。
  NeeView のような N 字スクロールもほしい。
- 方針 (2026-11-12 利用者決定、backlog §1.474): 修飾なし ↑↓ と通常のホイールを、それぞれ独立に
  「ページ送り (既定) / ページ内スクロール」から選べる。N 字の前方向・次方向を `KeyAction` で割り当て可能にする。
  ページ内で完結し、自動でページは送らない。
- 観測の出どころ: 本書の「現状」はすべてコード読み (worktree 96ed160a0) と独立レビューのコード照合。実画面での確認はしていない。

## 1. 現状 (コード確認)

| 項目 | 現状 | 場所 |
| --- | --- | --- |
| 修飾なし ↑↓ (ページ単位表示) | 固定入力。`consume_arrow` (plain、通らなければ Shift) で `consume_fixed_page_turn_chord` の結果を 1 つ得て、`nav_next` / `nav_prev` → `spread_page_nav(±1)`。Shift+↑↓ は同義 (スタックのフラット読書中だけスタックジャンプ) | `src/ui_fullscreen.rs` の `consume_arrow` 〜 `nav_next` |
| ↑↓ (縦連結) | `continuous_reading_key_step_px` (= 表示領域の高さ × `continuous_reading_key_scroll_percent`、既定 16%) だけ即時スクロール | 同上 |
| 通常ホイール | ページ送り。`wheel_y.abs() > 0.5` で受理。Ctrl+ホイール・編集モードはズーム。**Alt+ホイールはページ送り。Shift+ホイールは縦成分が横成分へ移る (`wheel_input.rs` の `horizontal_scroll_modifier`) ため、全画面では何もしない** (既存テストで固定)。連結読みは `continuous_reading_wheel_scroll_percent` (既定 20%/ノッチ) | `fs_wheel_action_requests_navigation`、`continuous_reading_wheel_delta_px`、`src/wheel_input.rs` |
| シークバー上のホイール | ドラッグ中でなければキャンバス側へ通し、ページ送りになる (意図された動作) | `fullscreen_cursor_in_panel_for_wheel` |
| クリックとホイールの同 frame | クリックは wheel 適用前に計画し、`propose_fs_wheel_click_transform` がズームだけを予測する | `handle_fs_wheel_and_click_frame` |
| パン | `fs_zoom` / `fs_pan` (context 所有)。入力の書き込みは `set_fs_pan_from_input` が `clamp_fullscreen_pan_from_layout` (実ページを 48px 以上残す緩い制約) を通し、§1.726 の初期配置要求を消す | `src/ui_fullscreen.rs` |
| 表示単位の幾何 | §1.726 の初期配置は、描画準備で素材を選んだ後の消費箇所 (`resolve_fs_initial_placement`、単ページ・見開きの各経路) で、**その frame の** pan 0 の表示単位矩形と viewport から解く。前 frame の `FullscreenPageLayout` は世代・slice・current/holdover の証明を持たない | `src/ui_fullscreen/initial_placement.rs`、`src/displayed_image_transform.rs` |
| 自由回転 | `paint_rect` の四隅を後から回転して描く (`paint_quad`)。`paint_rect` は回転後の外接矩形ではない | `src/displayed_image_transform.rs` |
| クリック送り (§1.745) | 通常静止画では、fit・回転なしで pan が中央か自動配置 (`fs_automatic_initial_view`) と一致するときだけ単クリックで送る。動かした後のダブルクリックは表示リセット | `plan_fs_canvas_click`、`fs_view_has_moved` |
| ヘルプ | `?` の文脈は Z・比較・連結などでも `FsImage`。未割り当ての Action は一覧に出ない | `src/ui_dialogs/context_shortcuts.rs`、`src/keymap.rs` |
| ゲームパッドの方向キー | 別経路 (`src/app/gamepad_input.rs`) | — |

キーによる 4 方向のページ内スクロール、端への位置合わせ、N 字の操作は無い。

## 2. 仕様

### 2.1 設定 (3 項目、独立)

| 設定 | 値 | 既定 |
| --- | --- | --- |
| 上下キー (修飾なし ↑↓) | ページ送り / ページ内スクロール | ページ送り |
| 通常のホイール | ページ送り / ページ内スクロール | ページ送り |
| N 字スクロールの順序 | 縦優先 (N 字) / 横優先 (Z 字) | 縦優先 (N 字) |

- 上下キー・ホイールの既定は現行と完全に同じ (既定のままでは置き換え点の新しい経路を通らない)。順序の操作 (§2.7) は既定キー `K` / `J` を持ち、変更・解除できる。
- 順序の設定は §2.8 の操作 (`FsPageNScrollNext` / `Prev`) だけに効く (2026-11-18 利用者決定で追加。NeeView も N 字・Z 字を選べ、既定は N 字)。
  キーは順序ごとに分けず、「指定の順で次へ / 前へ」の 1 組だけにする (利用者「別々のキーアサインだと使いづらそう」)。
- **全画面の「…」(その他の機能) からも順序を選べる** (2026-11-18 利用者決定): ルートに「ページ内の読み順」を足し、選ぶと
  ナビゲータ位置・スライドショーと同じ形の小パネル (`FsOverflowPanelState` の新しい状態) が開いて「縦優先 (N 字) / 横優先 (Z 字)」を選ぶ。
  書き換えるのは環境設定と同じ全体設定 1 つ (すぐ保存。スライドショーの小パネルと同じ保存の扱い)。本ごとには覚えない
  (本ごとの記憶は、必要が分かってから既存の本ごとの記憶に乗せられるかを調べて足す)。開閉は既存の小パネルと同じ扱い
  (ホイール・シークバー・クリックで本やページが変わっても閉じない = `closes_on_file_change` は false。キーを押したときは、既存のパネルと同じく
  閉じてからそのキーの操作を続ける。新しいパネルだけ別のキー処理を作らない。レビュー r5 P2)。ルートの項目は画像の通常表示で常に出し、使えない表示 (連結・Z など) でも
  設定は変えられる (既存のスライドショーの項目と同じ扱い)。読み順を切り替えるキー操作は足さない。
  日本の漫画をはみ出した状態でコマ順に読むなら横優先 (段の中を読む方向へ進み、次の段へ下がる)、4 コマの列や縦書きの文章なら縦優先が合う。
- 共通設定のみ (`Settings`)。お気に入り別の表示状態・本ごとの設定には入れない。設定の転送は
  `fullscreen_horizontal_cursor_direction` と同じ分類。**未リリースの新設定**なので migration 不要 (serde default)。
- 環境設定「見開き / 連結」の「カーソルキー左右の方向」の直後に 3 行 (順序の行には「ページ内を順に読み進める操作 (既定 K / J) の順序です。」と添える)。上下キー・ホイールの説明 1 行:
  「ページ単位の表示で、画像が画面からはみ出す分だけ動かします。ページの端で止まり、次のページへは送りません。」
- 移動量は既存の「連結読みのスクロール量」(ホイール 1 ノッチ 20% / 矢印キー 1 回 16%、表示領域の大きさ基準) を共用し、
  見出しを「スクロール量 (連結読み・ページ内スクロール)」へ改める。新しい量の設定は作らない (判断事項 Q3)。

### 2.2 どこで置き換えるか: 入力の所有 → 置き換え点 → 面の述語

r1 では「面の述語 1 つ」で分岐する案だったが、前面 UI の入力所有と、モード外のための従来経路への戻しが混ざっていた (レビュー r1 P2-7)。
次の 3 段に分ける。

1. **入力の所有 (変更しない)**: modal・TextInput・IME・ポップアップ・ナビゲータ・情報パネル・ページ一覧 (O)・編集ツールのパネルなど、
   前面 UI が入力を持つ判定は既存のまま。そこで消費・遮断される入力は置き換え点に来ない。
   **fullscreen の右クリックメニュー**: Windows の native メニューは OS のメニューループが入力を持つので来ない。
   egui の fallback メニューは共通 modal に登録されておらず、固定矢印にも除外が無い (既定設定では現行どおりページ送りになる。
   この既存の挙動は今回変えない)。
   fallback メニュー (`fs_context_menu_idx.is_some()`) が開いている間は、**その入力の設定がページ内スクロールの置き換え点だけ**
   (上下キーの設定なら plain ↑↓、ホイールの設定なら通常ホイール) を**消費して何もしない** (スクロールもページ送りもしない)。
   メニューが持つ入力をページ送りへ戻さない。新 Action は §2.7 の規則 (前面 UI が持つ間は消費しない) に従い、設定値には依存しない。
2. **置き換え点 (2 か所だけ)**: 既存コードが「その入力でページ送りをする」と決めた地点で、送る代わりにスクロールする。
   - 上下キー: `consume_arrow` の結果のうち **plain の ↑↓ に一致した結果だけ**。`nav_next` / `nav_prev` を作る前に振り分ける。
     Shift+↑↓ に一致した結果・スタックジャンプ・動画/音声の `VideoNextFile` 等は対象外。
   - ホイール: §2.6 の分類関数が `InPageScroll` と決めた segment。修飾なしの縦 (現行で「送る」入力) と、
     修飾なし・Shift の横成分 (現行では受理されず何もしない入力) が対象。
     ただし**シークバー上 (ドラッグ中でない hover) のホイールは置き換えず、現行どおりページ送り**。Alt+ホイールも送りのまま。
3. **面の述語** `App::fs_in_page_scroll_surface()` (1 か所、置き換え点の両方とスクロール用 `KeyAction` が使う):
   - 現在項目がページを持つ静止画 (`GridItem::has_page_data`)。動画・音声・音楽ビューは除く
   - `reading_flow.is_paged()`
   - `fs_zoom_owner() == FsZoomOwner::Image` (Z の照準・確定、360 度、3D 写真、分析を除く)
   - 比較表示 Off、表示トリム編集中でない、オーバーレイ編集でない、範囲キャプチャ選択中でない
   述語は `plan_fs_canvas_click` の `ordinary_still`・§1.726 の退役条件と同じ領域を表す。3 つを 1 つの helper に寄せられるかを
   実装時に確かめ、寄せられない差分があれば理由をコメントに残す。

述語が成立しないとき、または設定がページ送りのときは、置き換え点で何もせず**現行の経路のまま**送る
(1 の fallback メニューだけは例外で、上のとおり消費して何もしない)。

### 2.3 スクロールを解く時点: 入力時に、前 frame の幾何を照合して

r2 の「意図だけを記録し、描画準備で解く」案は撤回した (§6.2)。既存のパン入力 (ドラッグ・ホイール拡大・ナビゲータ) と同じく、
**入力を処理する時点で `fs_pan` を書き換える**。幾何は前 frame の `FullscreenPageLayout` から取るが、
それが今の通常画像の表示単位で、幾何の入力も変わっていないことを照合し、合わなければ入力を消費して何もしない。

- **描いた表示単位の印 (layout に 1 つ足す)**: `FsDrawnUnitStamp`。`begin` / `clear` で消え、
  **通常画像の current の表示単位を最後まで描いた経路だけ**が付ける (レビュー r3 P2-3)。
  - 付けない: Z・比較・分析・Checking・類似プレビュー・360・3D の描画、holdover を描いた frame、current の上に前の表示単位の holdover を
    前面へ重ねて最終結果にした frame、期待する実ページ (見開きの両側) が揃わず一部だけ描いた frame。
    このため比較を解除した直後など、layout が通常画像のものでない frame の次の入力は印が無く、消費して何もしない。
  - 中身: `items_generation`、表示単位 (各実ページの `page_idx` + slice を**画面上の左右順**で、役割・片側配置を含む)、
    **幾何の入力の鍵** `FsScrollGeometryKey`。設定上の fit モードではなく、**矩形を決める実引数**から作る: 採用素材の
    `FullscreenFitInput` 一式、描画へ渡す `texture_size` (素材の `full_source_size_vec2()`)、見開きの `layout_size`、
    対象 viewport の矩形 (HUD 予約後) と `pixels_per_point`、保存回転・自由回転・実効トリム・見開きの配置と左右順・読書方向・ページ間隔
    (レビュー r4 P3-1)。TextureId・Arc のアドレス・frame 番号は入れない (素材を交換しても幾何が同じなら一致する)。
    白 companion はページの並びに入れず、幾何 (§2.4 の `unit`) にだけ含める。
  - 鍵は**描画と入力で同じ helper** (`App::fs_scroll_geometry_key(...)`) から作る。描画時は描いた表示単位について、入力時は今の
    表示単位について作り、一致を照合する。`fs_pan` は鍵に入れない。倍率は下の「同じ frame の倍率変更」のとおり、layout を描いたときの倍率と今の `fs_zoom` が一致することを照合に含める。
    入力時の素材は既存のメタデータ選択経路で引く。**作れない値があれば既定値や前回値で埋めず、照合不成立 (消費のみ)** にする。同期の読み込みはしない。
  - 鍵の viewport・DPI は**配送先の fullscreen viewport**のものを使う。ROOT で受けたキーの handler に渡る ROOT の `ctx` の
    `content_rect()` / `pixels_per_point()` をそのまま使わない (レビュー r4 P3-2)。
- **照合 (入力時、1 つの helper)**: 印があり、`items_generation`・表示単位・幾何の入力の鍵が今と一致し、navigation sequence が
  ページ送りを待っていない・紙めくり演出中でない。合わなければ入力を**消費して何もしない** (後の frame へ持ち越さない。新しい待機状態は作らない)。
  読み込み待ちのページで ↓ を押した場合、同じ frame に fit・回転・トリム・読書方向・見開きの構成を変えた場合、
  窓サイズ変更中・HUD の固定を切り替えた frame のホイールなどがこれに当たる (レビュー r3 P2-2。r3 の「割り切り」は撤回)。
  照合で落ちるのは幾何が変わった後、新しい幾何で描いた layout から鍵を作り直せるまでの入力で、それ以降は通常どおり動く
  (連続した窓サイズ変更の間や、素材の寸法がまだ分からない間は落ち続ける)。
- **同じ frame の倍率変更** (2026-11-18 設計担当の決定、実装担当の停止報告から): Ctrl+ホイール・キー拡大などで入力時点の `fs_zoom` が
  layout を描いたときの倍率と違えば、照合不成立として**消費して何もしない**。次の frame の layout からはスクロールできる。
  比べる倍率は `fs_zoom` の値ではなく**描いたときの実効の倍率** (`fs_zoom_pan()` が None なら 1、そうでなければその倍率) とする。
  **スクロールで pan を書くときは、`fs_zoom` にも layout を描いた実効の倍率を書く** (§6.9)。1 付近の倍率 (例 0.9995) は pan 0 なら 1 倍で、
  pan が付くとその倍率で描かれる既存の正規化 (`fs_zoom_pan_for`) があり、書かないとスクロールした瞬間に寸法が変わって端に空きが残るため。
  描いた倍率をそのまま書くので、見た目の倍率は変わらない。
  前 frame の矩形を倍率比で投影する案は採らない: 既存の `clamp_fullscreen_pan_from_layout` の投影は通常の見開きのページ間隔も倍率に比例させるが、
  実際の描画は間隔を固定するので、端が間隔の差だけずれる (例: gap 200・倍率 1→2 で 100pt)。共有の投影を直すと既定設定の手動パン・ズームの
  clamp にも影響するので、範囲を広げず、同じ frame の倍率変更とスクロールの組み合わせを捨てる。
- **§1.726**: スクロールは `set_fs_pan_from_input` と同じく初期配置要求を消す。
- **入力の配送**: 既存のページ送りと同じく、キーは受理した handler で、ホイールは `apply_fs_wheel_frame_proposal` (遅延時を含む) で、
  その context の `fs_pan` に書く。ROOT で受けたキーが fullscreen の context を動かす既存の経路をそのまま使い、新しい受け渡しを作らない。
  キー (↑↓・新 Action) の消費結果は handler のローカル値に持ち、**幾何を変えるキー処理 (fit・回転・構成など) の後、クリックの計画の前**に適用する
  (消費直後に書くと、同じ frame の後で変わる幾何を鍵で検出できない。レビュー r4 P3-2)。キーの処理はホイール・クリックの計画より先にあるので、
  キーで書いた位置は同じ frame のクリック判定に使われる。新しい配送状態は作らない。

### 2.4 スクロール範囲と 1 回の移動 (純関数)

前 frame の layout (照合済み。倍率も描いたときと同じ) の、現在の表示単位から次を求める。

- **外接矩形 `unit`**: 描いた各ページの `paint_quad()` (clip 前) の四隅の外接矩形の和。自由回転 0 なら `paint_rect` と同じ。
  見開きで描く白 companion を含み、描かない Vacant 枠は含めない (レビュー r1 P2-4)。表示単位矩形の考え方は §1.726 と同じ
  (§1.726 は pan 0 の描画準備で、本機能は前 frame の描画結果で求める。四隅の外接にする点を helper で共有できるかは実装時に確かめる)。
- **端揃えの範囲 (軸ごと)**: `unit` がその軸ではみ出すときだけ範囲を持つ。
  縦: `pan.y ∈ [viewport.bottom − (unit.bottom − pan₀.y), viewport.top − (unit.top − pan₀.y)]` (`pan₀` はその layout の pan)。横も同様。
  はみ出さない軸は動かさない。
- **1 回の移動の目標**: 目標 = `pan + step` を端揃えの範囲へ寄せる。範囲の外側へは押し出さない:
  その向きの端以上 (手動ドラッグで端の外にある場合を含む) なら何もしない。内側から端を越える移動は端で止める。
  外側から内側へ向かう移動は step だけ進み、端を越えない。
- **候補 → clamp → 進んだときだけ書く** (レビュー r3 P2-1): 1 回の操作 (↑↓・4 方向・ホイールの 1 segment・N 字の 1 回) ごとに、
  目標の pan を作り、既存の clamp (`clamp_fullscreen_pan_from_layout` と同じ式。どれか 1 枚の実ページを縦横とも 48px 以上残す、
  ページごとの区間の和の 2 次元の制約) を通した**最終の候補**を 1 つ作る。最終の候補が次を満たすときだけ、それを `fs_pan` に 1 回書く。
  満たさなければ操作前の pan のまま (入力は消費する)。
  - 縦・横の移動 (↑↓・4 方向): 意図した軸が意図した向きへ進み、もう一方の軸が変わらない (clamp が直交方向へ補正した候補は採らない)。
  - ホイールの 1 segment (縦横が同時に 0 でない場合を含む、レビュー r4 P3-3): 1 つの 2 次元の候補として扱い、
    要求した各軸が逆走せず、少なくとも 1 軸が要求した向きへ進み、要求していない軸 (デルタが 0 の軸) が変わらないときだけ採る。
    一方の軸が端なら、その軸は動かずもう一方だけ進む候補になる。
  - N 字の列移動 (§2.8 の手順 2): 横が読む方向へ進む (縦は設計どおり反対の端へ移ってよい)。
  可視性の領域を本機能で別に作らない。端は「進まなかった」ことで分かる。
- **書き込み**: 採った候補は既存の clamp を通した値そのものなので、`set_fs_pan_from_input` で書いても変わらない
  (同じ関数・同じ layout を使う。`set_fs_pan_from_input` が初期配置要求を消す)。
- `fs_automatic_initial_view` は書き換えない。移動は即時 (連結読みのキー・ホイールと同じ。アニメーションしない)。

### 2.5 上下キー = ページ内スクロール

- 修飾なし ↓ = 下を見る (画像が上へ動く)、↑ = 上を見る。1 回 = 表示領域の高さ × 矢印キーのスクロール量 (既定 16%)。
- 固定矢印の consume 契約を変えない: `consume_arrow` が返す 1 つの結果 (matched chord・first keydown / repeat・held・release 済み repeat) を
  **一度だけ消費して振り分ける**。first keydown は短い tap でも 1 回、release 済み repeat は動かさない、同 frame の複数 edge は 1 回
  (`SinglePerFrame`)。native / egui で二重に消費しない。長押しは OS のキーリピートで 1 リピート 1 回。
- 縦の端で止まる。次のページへは送らない。縦にはみ出していないページでは何もしない (§3 の既決事項 D1)。
- **Shift+↑↓ は現行どおりページ送り** (スタックのフラット読書中はスタックジャンプ)。ページ内スクロールを選んだ人の
  キーでのページ送りは ←→・Shift+↑↓・PageUp/PageDown が残る。
- ←→ は変えない (常にページ送り。「カーソルキー左右の方向」も従来どおり)。

### 2.6 通常のホイール = ページ内スクロール

- **分類は「ページ内スクロールか、既存経路へ委ねるか」だけを 1 か所で決める** (レビュー r2 P2-6・r3 P2-4):
  `fs_wheel_segment_in_page_scroll(...) -> FsWheelInPageClass { InPageScroll(delta), Legacy }`。
  入力の所有・場所 (シークバー上か)・面の述語・ホイールの設定・修飾 (修飾なし、または Shift。Shift は egui が横成分へ移した値を使う。
  Ctrl・Alt は `Legacy`) から決める。受付の閾値、
  `fs_wheel_action_requests_navigation`、適用、クリックの予測はこの答えを共有する。
  `Legacy` は既存の処理へそのまま渡し、連結スクロール・動画/音声の音量・音楽タイムライン・ズーム・ページ送りなどの
  既存の動的な適用先の決定 (途中の移動後に次の segment を現在の項目で評価し直すことを含む) を変えない。
  受付の時点で `Legacy` の segment は現行と同じ条件で受理・不受理が決まる (横だけの segment は現行どおり**受理せず残す**)。
- **適用時の再確認**: `InPageScroll` で受理した segment は、適用の時点で同じ関数をもう一度評価する。
  その frame に先に移動 (ページ・本・動画/音声の項目) が起きていた、または `Legacy` に変わった場合は、**消費して何もしない**
  (小さなデルタや横だけのデルタを既存のページ送りへ渡さない。既存の処理は `wheel_y == 0` を前方向として扱うため、渡すと誤って送る)。
- **縦**: 修飾なしのホイールのデルタの向きに画像を動かす (egui の ScrollArea と同じ向き。ホイール下 = 下を見る)。
  1 ノッチ = 表示領域の高さ × ホイールのスクロール量 (既定 20%)。換算は連結読みの `continuous_reading_wheel_delta_px` と同じ式で、
  高精度ホイール・タッチパッドの端数もそのまま比例させる。
- **横成分**: 横のチルト・タッチパッドの横・Shift+ホイール (egui が縦を横へ移す) の横成分は、`InPageScroll` と決まった segment に限り
  横のページ内スクロールに使う (同じ換算を幅で行う。Shift+ホイール下 = 右側を見る)。それ以外 (既定設定・面の外・シークバー上) の
  横成分は `Legacy` で、現行どおり受理されない。
- **受付の閾値**: ページ送りの `wheel_y.abs() > 0.5` は変えない。`InPageScroll` と決まった segment だけ、有限で 0 でないデルタ (縦・横) を受け付ける
  (`TouchPhase::Move` 以外は既存の変換で 0。消費済み segment は既存どおり除く)。
- **変えないもの**: Ctrl+ホイール (ズーム)、Alt+ホイール (ページ送り。ホイールでのページ送りの逃げ道)、
  シークバー上 (ドラッグ中でない hover) のホイール (ページ送り)、パネル・ナビゲータ・ポップアップ上のホイール。
- 端で止まる。はみ出していないページでは何もしない (D1)。
- **同じ frame の複数 segment**: 適用は既存どおり segment の順に、遅延時 (`merge_at_frame_tail`) も含めて前面 UI が残した segment だけを使う。
  先に移動が起きた後の `InPageScroll` は上の再確認で消費して何もしない (移動後のページに旧ページの幾何で書かない)。
  ズーム (`Legacy`) の後の `InPageScroll` は、§2.3 のとおり倍率が描いたときと違うので消費して何もしない。
- **同じ frame のクリック**: クリックの計画は wheel の適用前なので、既存のズームの予測と同じく `propose_fs_wheel_click_transform` で
  ページ内スクロール後の pan も予測する (§2.4 と同じ純関数・同じ layout・同じ clamp)。予測した位置で §1.745 の規則がそのまま判定する
  (中央からスクロールした frame の単クリックは送らない)。タッチのページ送りは別経路で、変えない。

### 2.7 新しい KeyAction

| Action | 説明 (description) | 既定 |
| --- | --- | --- |
| `FsPageNScrollNext` | ページ内を指定の順 (N 字 / Z 字) で次へ読み進める | `K` |
| `FsPageNScrollPrev` | ページ内を指定の順 (N 字 / Z 字) で前へ戻る | `J` |
| `FsPageScrollUp` / `Down` / `Left` / `Right` | ページ内を上 / 下 / 左 / 右へスクロールする | なし (判断事項 Q4) |

- context `FsImage`、trigger `Press` (リピート可)、`SinglePerFrame`。`ini_name` / `description` / `context` / `trigger` /
  `default_chords` (N 字の次 = `K`・前 = `J`、4 方向は空) / `ALL_ACTIONS` / `docs/keymap.ini.default` (4 方向は `# Action = none`) を揃える。
- 面 (§2.2 の 3) が成立しないとき、または入力の所有が前面 UI (egui の fallback 右クリックメニューを含む) にあるときは**キーを消費しない**
  (§1.470 の `FsZoomIn` と同じ)。この規則は上下キー・ホイールの設定値に依存しない。面が成立して照合 (§2.3) が通らないときは消費して何もしない。
- 4 方向の移動量は上下キーと同じ。左右も端で止まり、綴じ方向で反転しない (画面の左右)。
- 上下キーの設定がページ送りでも、これらの Action は使える (順序の 2 つは既定 `K` / `J`、4 方向は割り当てた人だけが使う)。
- リング・ジェスチャ・マウスボタン・ゲームパッドには今回足さない (キーボード専用。`ring-keyaction-parity.md` が
  KeyAction → Ring の方向を要求しないことを実装時に確認する)。

### 2.8 N 字スクロール

縦を優先して読み、縦の端で綴じ方向へ 1 列進み、反対側の縦端から読む (NeeView の N 字と同じ順序)。

- 次 (`FsPageNScrollNext`):
  1. 縦に範囲があって下端でない → 下へ 1 画面分 (表示領域の高さ × 85%) 進む。下端で止める。
  2. 縦の下端 (または縦に範囲が無い) で、横に範囲があって読み終わりの横端でない → 読む方向へ 1 画面分 (幅 × 85%) 進み、縦は上端へ。
     右→左の本は左側へ、左→右は右側へ進む。
  3. どちらも端 → 何もしない (ページは送らない)。
- 前 (`FsPageNScrollPrev`): 逆順。上へ戻り、上端なら読み始め側へ 1 列戻って縦は下端へ。
- 各手順は §2.4 の「候補 → clamp → 進んだときだけ書く」で行う。手順 1 は縦が下へ進み横が変わらない候補だけを採る。
  手順 2 は「横を読む方向へ 1 列 + 縦を上端へ」を **1 つの候補**として作って clamp し、最終の横が読む方向へ進んだときだけ書く
  (縦の端への移動で clamp が横を戻す場合は採らない。レビュー r3 P2-1)。手順 1 が採れなければ手順 2、どちらも採れなければ何もしない。
  採る候補はどれも、横が変わらないか読む方向へ進むので、横は単調に進み、同じ列を繰り返さない。
- 85% は連結読みの PageUp/PageDown と同じ定数 (`VERTICAL_READING_PAGE_SCROLL_FRAC`)。15% を前の画面と重ねる (判断事項 Q3)。
- 読書方向は §1.726 と同じ源 (`spread_mode.reading_direction().unwrap_or(self.reading_direction)`)。
- 列の位置を状態に持たない。毎回、今の pan と範囲から次の位置を決める純関数。途中でドラッグ・ホイールで動かしても、その位置から続く。
- 「読み始め」(§1.726) と組み合わせると、ページを開いた位置 = N 字の始点になる。既定の「中央」では中央の列から始まる。
  マニュアルに組み合わせを 1 行書く。

#### 2.8.1 横優先 (Z 字) — 順序の設定が「横優先」のとき (2026-11-18 利用者決定)

同じ 2 つの操作で、主軸と改行の軸を入れ替える。段の中を読む方向へ読み、横の端で 1 段下がって、読み始め側の横端から読む。

- 次 (`FsPageNScrollNext`):
  1. 横に範囲があって読み終わりの横端でない → 読む方向へ 1 画面分 (幅 × 85%) 進む (右→左の本は左側へ、左→右は右側へ)。端で止める。
  2. 読み終わりの横端 (または横に範囲が無い) で、縦に範囲があって下端でない → 下へ 1 画面分 (高さ × 85%) 進み、横は読み始め側の端へ
     (右→左の本は右端、左→右は左端)。
  3. どちらも端 → 何もしない (ページは送らない)。
- 前 (`FsPageNScrollPrev`): 逆順。読み始め側へ戻り、その端なら 1 段上がって横は読み終わり側の端へ。
- 採否は N 字の縦と横を入れ替えた同じ規則: 手順 1 は横が読む方向へ進み縦が変わらない候補だけ、手順 2 は「縦 1 段 + 横の端」を
  1 つの候補にし、最終の縦が下へ (前は上へ) 進んだときだけ書く。縦は単調に進むので同じ段を繰り返さない。
- 純関数は 1 つ (`n_scroll_target(order, ...)`) にして、`order` で主軸を選ぶ。N 字と Z 字で別の関数・別の採否規則を書かない。
- 段の位置も状態に持たない。読書方向・85%・「読み始め」との組み合わせは N 字と同じ。

### 2.9 他の機能との関係

| 機能 | 扱い |
| --- | --- |
| 見開き | 見開き全体を 1 つの範囲として動かす。片側に置いた単ページの既存配置は、はみ出さない軸では動かさない |
| 回転 | 保存回転は layout が回転後の表示空間なのでそのまま。自由回転は四隅の外接で範囲を決める (§2.4) |
| 画面に収まる画像 | その軸は何もしない (D1) |
| 分割 (横長の片側表示) | 表示中の片側が範囲 (その frame で確定した slice の幾何を使う) |
| 綴じ方向 | ↑↓・4 方向・ホイールは画面基準。N 字の横の進み方だけ読書方向に従う |
| 連結読み | 面の外。↑↓・ホイールは現行の連結スクロールのまま。新 Action は消費しない |
| Z ズーム・360・3D・分析・比較・編集モード・表示トリム編集・範囲キャプチャ | 面の外。現行のまま |
| シークバー | その上のホイールはページ送りのまま |
| §1.726 読み始め | スクロールは利用者入力なので初期配置要求を消す |
| §1.745 クリック送り | スクロール後は「動かした」扱い (`fs_automatic_initial_view` を書き換えない)。単クリックでは送らず、ダブルクリックで中央へ戻す。手で元の自動位置・中央へ戻せば現行どおり送る (D2)。キーのスクロールは入力時に `fs_pan` を書くので、同じ frame の後で計画するクリックは新しい位置で判定される。同 frame のホイール + クリックは §2.6 の予測 |
| 紙めくり演出 | pan が 0 でないと演出しない既存規則のまま。スクロール後のページ送りは演出なし (§1.726 と同じ制限) |
| ナビゲータ・ルーペ・キー拡大 | 確定した `fs_pan` をそのまま使う。変更なし |
| スライドショー | ページ内スクロールでは止めない・影響しない |
| F12 別ウィンドウ / 独立 viewer | 新しい実行時状態は layout の印だけ (layout と同じく context 所有で既に swap される)。各 context の `fs_pan` と layout だけを読み書きする。入力は既存の viewport ごとの配送 (ROOT で受けたキーも既存の経路で対象 context へ)。兄弟 context は変えない |
| fullscreen の右クリックメニュー (egui fallback) | 開いている間、設定がページ内スクロールの置き換え点の入力は消費して何もしない (§2.2)。新 Action は消費しない (§2.7)。既定設定の現行挙動は変えない |
| ゲームパッド・タッチ | 変更しない (Q5) |
| mIV Remote | 変更しない (本体の `fs_pan` を使わない別表示) |
| `?` ヘルプ | 固定キー欄の ↑↓・ホイールの説明は「ページ内スクロール (ページ単位の通常表示のとき。連結・Z・比較などでは従来どおり)」のように、適用される面を書いて出し分ける (レビュー r1 P3-8)。新 Action は割り当て後に一覧に出る (既存の仕組み) とマニュアルに書く |

### 2.10 状態の組み合わせを減らす検討 (CLAUDE.md「設計の簡素化」)

- **新しい実行時状態は layout の印 1 つだけ。** 足すのは保存する設定 2 つ、layout に記録する「描いた表示単位の印」(幾何の入力の鍵を含む)、状態を持たない純関数。
  N 字の「今どの列か」も持たず、今の pan から毎回求める。印は layout と同じく毎 frame 作り直され、context と一緒に swap される。
- **入力時に書く。** 既存のパン入力と同じ時点で `fs_pan` を書き換える。描画時まで意図を持ち越す案 (r2) は、
  ROOT から別 viewport の描画への受け渡し、描画後に適用される混在ホイール、素材の再試行での二重適用、同 frame のクリックの旧 pan と、
  扱う組み合わせが増えたので撤回した (§6.2)。
- **照合して合わなければ捨てる。** 前 frame の layout が今の表示単位のもので幾何の入力も同じかを印で照合し、合わなければ入力を消費して何もしない。
  入力を溜めて後で適用する仕組みや待機状態を作らない (既存のページ送りの方針と同じ)。鍵は描画と入力で同じ helper から作るので、
  照合の項目を 2 か所に書かない。同じ frame に倍率が変わった場合も投影せず捨てる (§2.3)。
- **失効の書き手を増やさない。** fit・回転・トリム・構成などの変更経路ごとに印を消す処理を足すのではなく、鍵の比較で古い layout を見分ける
  (変更経路を列挙すると書き漏れが出るため)。
- **可視性の領域を作らない。** 最終位置は既存の clamp が決め、進まない候補は採らない。端は「進まなかった」ことで分かる。
- **ホイールの既存経路を書き換えない。** 新しく決めるのは「ページ内スクロールか否か」だけで、それ以外は既存の処理へ委ねる。
- **端でページを送らない。** 端の判定・送った直後の初期位置・長押しで端を越える連続送りの組み合わせを作らない (利用者決定どおり)。
- **アニメーションしない。** スクロール中の割り込み (ページ送り・ズーム・別入力) の組み合わせを作らない。
- **移動量の設定を増やさない。** 連結読みの量を共用し、N 字は既存の 85% 定数。
- 採らなかった案: 「端に着いたらページを送る」(ZipPla・SumatraPDF・NeeView のホイール Type A 方式) は利用者決定により採らない。
  「スクロール中だけ別の表示状態に入る」案は、終了条件と他モードとの排他が増えるので採らない。
- まれな失敗は既定の割り切り (このページ内スクロールは保存データを持たないので該当なし)。

## 3. 決めたこと・判断事項

### 3.1 既存の決定から決めたこと (独立レビュー r1 も既決方針どおりで可とした)

- **D1 画面に収まる画像での ↑↓・ホイール**: 何もしない (その軸に範囲が無い)。利用者決定「端で止めてページは送らない」と同じ扱い。
  ページ全体フィットで使うと、ほとんどのページで ↑↓・ホイールが効かなくなる。マニュアルに書く。
- **D2 スクロール後のクリック送り**: 2026-11-15 の利用者決定 (§1.745「パン後は単クリックを止め、ダブルクリックで戻す」) のまま、
  ページ内スクロールもパンとして扱う。

### 3.2 利用者判断 (2026-11-18 決定: Q3〜Q5 とも推奨案。利用者がこの担当セッションで「推奨案で進める」と回答)

決定: Q3 = 連結読みの量を共用・N 字は 85%、Q4 = N 字 2 つ + 4 方向 4 つを足し、位置合わせは入れない、Q5 = ゲームパッドは変えない。以下は判断時の選択肢の記録。
追加の決定 (2026-11-18、利用者): 順序の設定 N 字 / Z 字 (既定 N 字) を足し、「…」からも選べる。N 字の 2 操作だけ既定キーを付ける:
`K` = 次へ、`J` = 前へ (動画のマーカー移動 J = 前・K = 次と同じ向き。修飾キーなしで戻れる。vim の j / k とは逆)。4 方向は既定キーなしのまま。

- **Q3 移動量**: (推奨) 連結読みのスクロール量 (ホイール 20%・キー 16%) を共用し、N 字は 1 画面の 85% (15% 重ね)。
  代案: ページ内スクロール専用の量・N 字の重なり幅を設定に足す。
- **Q4 含める操作の範囲**: (推奨) N 字の前後 2 つと 4 方向の個別スクロール 4 つを、どれも既定キーなしで足す。
  端・中央への位置合わせ (NeeView の「上端へ」等) は今回入れない (Home/End はページ移動のまま)。
  代案: N 字だけにする / 位置合わせも入れる / N 字に既定キーを割り当てる (空いている候補は少なく、既存の割り当てと競合しやすい)。
- **Q5 ゲームパッドの方向キー上下**: (推奨) 今回は変えない。代案: 上下キーの設定に合わせる。

## 4. 実装範囲 (見込み)

- `src/settings.rs`: 列挙型 2 つ (`FullscreenUpDownKeyAction` / `FullscreenWheelAction` 仮名、`#[serde(other)] Unknown` → 既定へ正規化)、既定値・転送分類・件数テスト。
- `src/ui_dialogs/preferences/pages.rs`: 3 行・検索 anchor・スクロール量の見出し。UI スナップショット更新。
- 「…」の読み順: `FullscreenOverflowItemId` (`src/settings.rs`) に項目、`FsOverflowPanelState` (`src/app.rs`) に小パネルの状態、`src/ui_fullscreen.rs` の「…」の描画・行数・操作 (`FsOverflowPanelAction`)。UI スナップショット。
- `src/displayed_image_transform.rs`: `FullscreenPageLayout` に描いた表示単位の印 (`begin` / `clear` で消える)。
- `src/ui_fullscreen.rs` + 子モジュール `src/ui_fullscreen/in_page_scroll.rs` (純関数・照合 helper): 印の記録 (paged の layout を作る各経路)、
  面の述語、置き換え点 2 か所、ホイールの segment の意味の関数とその共有 (閾値・送り判定・適用・クリック予測)、新 Action の消費、
  右クリックメニュー中の消費。
- `src/keymap.rs`・`docs/keymap.ini.default`: Action 6 つ。`src/ui_dialogs/context_shortcuts.rs`: 固定キー欄の出し分け。
- 文書: `docs/keymap-spec.md`・`docs/key-customization-impl-plan.md`・`docs/display-pipeline.md` (§2.4 の初期 pan の近く)・
  マニュアル (全画面表示・操作・環境設定のページ)・`docs/spec.md` 該当箇所・本書。
- detached-rework-plan: detached 述語・viewport 経路・HWND には触れない。context 所有の layout に印を 1 つ足すことを §11 に記録する。

## 5. 検証

- 純関数: 縦だけ / 横だけ / 両方はみ出す、収まる軸は不動、端で止まる、端の外 (手動ドラッグ後) から外へは動かない・内へは動く、
  見開き (白 companion 込み)、原寸の見開きで高さの違うページ (例: viewport 1000×1000、1000×3000 と 1000×1010、gap 20) で clamp が目標を戻す場合、
  回転 90 度、自由回転 45 度・負の角度・トリム併用、viewport 原点が 0 でない (HUD 予約)。最終位置はすべて `set_fs_pan_from_input` を通した値で確かめる。
- 既定キー: 画像の全画面で `K` が次・`J` が前に効く。動画・音声の全画面では従来どおりマーカー移動 (`VideoMarkerNext` / `Prev`)、
  消しゴム・隠蔽・補正レイヤーの `K` (塗りつぶし) は編集モードが先に受ける。操作カスタマイズの競合表示が、文脈の違うこれらを競合として出さないこと。
  保存済みの keymap で `J` / `K` を別の FsImage 操作に割り当てていた人には、新 Action の override が無いので既定の `J` / `K` が補われ、
  既存の競合表示が出る (自動の移行はしない。§1.470 と同じ扱い。保存済みの利用者に無影響とは書かない)。Press 同士の競合は消費順で片方が動く。
  Press と KeyHold (例: 保存済みの `FsScrubZoomHold = K`) の競合は、hold が押下状態を読むので両方が動き得る。既存の `TriggerMismatch` の競合表示が出ること、
  保存の往復、新 Action を `none` にすると旧来どおりになることを確かめる (レビュー r5 P3)。
- 「…」の読み順: ルートの行・小パネルの 2 択・戻る / 閉じる / Esc、選んだ値が環境設定と同じ全体設定に入り保存されること、
  ホイール・シークバー・クリックのページ送りではパネルが閉じず、キー (←→・J/K など) では既存のパネルと同じく閉じてからその操作が動くこと、行数・パネルの高さの既存の検査 (`fs_overflow_panel_body_row_count` 等) と UI スナップショット。
- Z 字 (順序の設定 = 横優先): LTR・RTL での段の中の進み方、読み終わりの横端→1 段下の読み始め側の端、最後の段の端で何もしない、前方向の逆順、横だけ・縦だけはみ出す場合、縦の段移動が clamp で戻されるとき横を動かさず終端になる。順序の設定は N 字の 2 操作だけに効き、↑↓・ホイール・4 方向は変わらない。設定の編集 → OK → 効果の通し。
- N 字: LTR・RTL での列の進み方、下端→次の列の上端、最後の列の下端で何もしない、前方向の逆順、縦だけ・横だけはみ出す場合、
  白 companion・高さの違う見開きで横が clamp に戻されるとき縦を動かさず終端になり、同じ列を繰り返さない。
  レビュー r3 の非対称トリムの例 (viewport 1000×1000、原寸、左 1000×3000 / 右 1000×1010、gap 20、右だけ上 20% トリム、LTR) で、
  列移動 + 上端の候補が clamp で横を戻されるときは採らず、循環しない。縦の移動で clamp が横を補正する候補も採らない。
- 照合: 印が無い (比較・Z・分析・類似プレビューを描いた直後、比較解除の直後を含む)・holdover・前面へ重ねた holdover・見開きの片側だけ描いた frame・
  items 世代違い・別のページ / 見開きの組 (1 ページずらしで組が 1 枚重なる場合を含む)・左右順だけ違う (読書方向の切替)・分割の反対側・
  navigation sequence 待ち・紙めくり演出中、および同じ frame の fit・回転・自由回転・トリム・見開きの配置・ページ間隔・HUD の固定・
  窓サイズ・DPI の変更では、消費して pan 不変、次の frame へ持ち越さない。新しい幾何の layout からは動く。
  fit 記述子は同じで採用素材の寸法だけ変わった (丸めた AI / PDF の素材へ交換) 場合も不一致になり、素材を交換しても寸法が同じなら一致する。
  鍵の値を作れない (素材の寸法が未取得) ときは消費のみ。幾何の入力の鍵を描画と入力で同じ helper から作っていること。
  ROOT と対象窓の寸法・DPI が違うとき、鍵は対象窓の値で作られる。同じ frame に fit 変更キーと ↓ のリピートが届いたとき、
  スクロールは fit 変更の後に照合されて落ちる。同じ frame の Ctrl+ホイール・キー拡大の後のスクロールは消費して pan 不変、次の frame からは新しい倍率の幾何で端に揃う。
  §1.726 の要求がスクロールで消える。
- 2 軸のホイール: 斜めの入力、片方の軸だけ端 (もう片方は進む)、clamp が要求していない軸を補正する候補は採らない。
- 上下キーの handler: 設定 2 つの全 4 組。ページ送りでは現行どおり `spread_page_nav` (回帰)、ページ内スクロールでは pan だけ変わり
  `fullscreen_idx` が変わらない。Shift+↑↓・←→ は送る。スタックのフラット読書の Shift+↑↓ はスタックジャンプ。
  同 frame の短い down/up は 1 回、release 済み repeat は動かない、複数 edge は 1 回、native / egui で二重に動かない、
  配送時の修飾と現在の修飾が違う場合、TextInput / IME / modal 中は背面へ渡らない。ROOT で受けたキーが fullscreen の context を動かす。
  egui の右クリックメニュー中は (ページ内スクロールの設定で) 消費して何もしない。既定設定では現行どおり。
  面の外 (連結・Z・比較・編集・動画・音声・音楽・範囲キャプチャ) では現行の結果と同じ。
- ホイール: 通常は縦スクロール、Shift・チルトは横スクロール。既定設定・面の外・シークバー上の横成分は現行どおり受理されない (送らない)。
  既定設定での連結スクロール・動画/音声の音量・音楽ビューのホイールが現行と同じ。画像→動画/音声をまたいで移動する frame の混在 segment が現行と同じで、
  移動後の `InPageScroll` は消費して何もしない。
  Ctrl はズーム、Alt は送り、シークバー上 (hover) の縦は送り、ドラッグ中のシークバー・パネル・ナビゲータ上は現行どおり。
  0.1 / 0.5 の Point デルタ・正負が比例して動く。ページ送り側の 0.5 の閾値は不変。
  混在 frame: 先に移動が起きた後の InPageScroll は消費のみ、ズーム → InPageScroll は順に適用、遅延適用 (`merge_at_frame_tail`) で前面 UI が取った segment を除く。
- クリック: 中央から「通常ホイール + 単クリック」の同 frame で送らない (予測)、「↓ + 単クリック」の同 frame で送らない (キーが先に書く)。
  ダブルクリックはスクロール後の位置で判定 (動かした → リセット)。タッチのページ送り・touch recognizer の終端は変わらない。
  スクロール後は `fs_automatic_initial_view` が不変で、手で元の自動位置・中央へ戻せば単クリックで送る。
- 新 Action: 面の外・前面 UI 所有 (egui の fallback 右クリックメニューを含む) では消費しない (設定値に依存しない)、照合が通らないときは消費して何もしない、既定キーは N 字の 2 つだけ (`K` / `J`)・4 方向はなし、ini 往復、`bundled_keymap_default_matches_generated_reference`、
  割り当て後に `?` に出る。同じ frame の ↓・4 方向・N 字はコードの処理順に、それぞれ照合済みの layout で順に適用する。
- 設定: 編集 → OK → 効果まで通し (環境設定 UI で値を変えて保存し、fullscreen の handler が新しい値で分岐する)。serde 欠落・転送。
- context: F12 別ウィンドウ・独立 viewer の入力が自分の `fs_pan` と layout だけを使い、main の pan を変えない。逆も同じ。
- UI スナップショット (環境設定の該当ページ)。

## 6. 独立設計レビュー

### 6.1 r1 (GPT-6.1 Sol / xhigh、read-only、session 01a1ea02、2026-11-18): REVISE

| 指摘 | 対応 (r2) |
| --- | --- |
| P2-1 前 frame の layout が今の表示単位のものとは限らない (左右半分・組の共有・倍率/回転/viewport の変化を区別できない) | §2.3: 意図だけを記録し、§1.726 と同じ描画準備の消費箇所でその frame の幾何から解く。1 pass でだけ有効 (**r3 で撤回**。入力時に、layout の印で照合して書く。§6.2) |
| P2-2 同 frame のホイール + クリックが旧 pan で送られる | §2.6: その frame のクリック・ダブルクリックを捨てる |
| P2-3 白 companion を含む範囲と既存 clamp (実ページ 48px) が両立しない | §2.4: 端揃えの範囲と可視性の区間の共通部分を移動可能範囲にする。N 字の終端もこれで判定 |
| P2-4 `paint_rect` は自由回転後の外接ではない | §2.4: `paint_quad` の四隅の外接。§1.726 と helper を共用 |
| P2-5 Shift+ホイールは現在ページ送りではない (横成分へ移り何もしない) | §1・§2.6 を訂正。逃げ道は Alt+ホイール。横成分はページ内スクロールの横に使う |
| P2-6 高精度デルタは 0.5 の閾値で落ちる | §2.6: ページ内スクロールの置き換え点だけ、有限の非ゼロを受け付ける |
| P2-7 面の述語と入力の所有を 1 つで済ませられない (シークバー上のホイール、context menu) | §2.2: 所有 → 置き換え点 → 面の 3 段。シークバー上は送りのまま |
| P3-8 ヘルプを設定だけで切り替えると対象外モードで誤表示 | §2.9: 適用される面を書いて出し分け、新 Action は割り当て後に出る |
| P3-9 固定矢印の consume 契約・自動配置基準の回帰が不足 | §2.5・§5 に追加 |
| Q1・Q2 は既決方針どおりに確定してよい | §3.1 の D1・D2 |

### 6.2 r2 (同じ session、2026-11-18): REVISE → r3 で方式を変更

r2 の「意図を記録して描画準備で解く」方式に対し、次の指摘があった: §1.726 の消費箇所は素材の再試行で複数回通り、幾何が None の呼び出しもある (P2-1)、
ROOT で受けたキーは別 viewport の描画へ渡す必要があり、egui の frame 番号は pass の識別にならない (P2-2)、混在ホイールは描画後に適用される (P2-3)、
見開きの可視性は軸ごとの区間でなくページごとの 2 次元の和 (P2-4)、キーのスクロールと同 frame のクリックも旧 pan で計画される (P2-5)、
横成分の受付と置き換え点の定義が矛盾し、横成分が旧処理へ漏れると送ってしまう (P2-6)、fallback の右クリックメニューの所有が未解消 (P2-7)、
意図の単位と適用順が未定義 (P3-8)。

r3 の対応: 描画時に解く方式を撤回し、既存のパン入力と同じく入力時に書く (§2.3)。前 frame の layout は描いた表示単位の印で照合し、
倍率は既存 clamp の投影を共有、同 frame の回転等の組み合わせは割り切る (P2-1・2・3・8)。可視性の領域は作らず、最終位置は既存 clamp、
端は「実際に動いたか」で判定し、N 字の列移動は横が進んだときだけ (P2-4)。キーは入力時に書くのでクリックは新しい位置で判定、
ホイールは既存のズーム予測と同じ関数で予測 (P2-5)。ホイールの意味を segment ごとに 1 関数で決めて共有し、横成分は `InPageScroll` のときだけ使う (P2-6)。
egui の右クリックメニュー中はページ内スクロールの設定で消費して何もしない (P2-7)。

### 6.3 r3 (同じ session、2026-11-18): REVISE → r4

| 指摘 | 対応 (r4) |
| --- | --- |
| P2-1 N 字の列移動の後、縦端への移動の clamp で横が戻り循環する (非対称トリムの見開きで数値例) | §2.4・§2.8: 1 回の操作ごとに候補を作って clamp し、意図した向きへ進んだときだけ書く。列移動は「横 + 縦端」を 1 つの候補にし、最終の横が進んだときだけ。縦の移動で横を補正する候補は採らない |
| P2-2 同 frame の幾何の変更 (fit 変更とキーリピート、resize 中のホイール等) は通常の操作で起こり、割り切れない | §2.3: 割り切りを撤回。印に幾何の入力の鍵を持たせ、描画と入力で同じ helper から作って照合。不一致は消費のみ |
| P2-3 印の current とページ集合では通常画像の layout を証明できない (比較・Z でも current、比較解除で layout が残る、左右順を区別できない) | §2.3: 通常画像の current 単位を最後まで描いた経路だけが印を付ける。左右順・役割・片側配置を含め、部分描画・前面の holdover は付けない |
| P2-4 ホイールの 4 分類では連結・音量・音楽などの既存経路を表せない | §2.6: 分類を InPageScroll / Legacy に絞り、Legacy は既存の動的な処理へ委ねる。InPageScroll は適用時に再確認し、変われば消費のみ |
| P3-5 fallback メニュー中の新 Action の消費方針が二重定義 | §2.2・§2.7: 置き換え点はその入力の設定がページ内スクロールのときだけ消費のみ、新 Action は設定に依存せず前面 UI 所有では消費しない |

### 6.4 r4 (同じ session、2026-11-18): ACCEPT

P1 / P2 なし。P3 4 件は実装時の確認事項とされ、本文へ反映した: 鍵は矩形を決める実引数 (採用素材の `FullscreenFitInput`・`texture_size`・`layout_size`) から作り、
作れない値は消費のみ (§2.3)、鍵の viewport は配送先の fullscreen viewport、キーは幾何を変えるキー処理の後・クリックの計画の前に適用 (§2.3)、
2 軸のホイールは 1 つの 2 次元の候補で採否 (§2.4)、右クリックメニューの表・ホイールの置き換え点・照合で落ちる期間の記述を本文に合わせた。
既定設定のままでは `Legacy` の動的処理・受理の閾値・横だけの不受理が保たれ、出荷済みの挙動を変える箇所は見つからなかった (レビュー担当の判断)。

### 6.5 利用者判断の後の追加 (2026-11-18)

利用者が他ビューワ (NeeView) の動きを参考にしたいとし、調査 (NeeView のソース `NScroll.cs`・`ViewScrollNTypeCommandParameter.cs` と公式コマンド一覧を読んだもの。実画面では未確認) の結果、
NeeView は N 字・Z 字を含む 5 種の順序を持ち既定は N 字と分かった。日本の漫画をはみ出した状態でコマ順に読むなら横優先が合うため、
利用者決定で「N 字スクロールの順序: 縦優先 (N 字、既定) / 横優先 (Z 字)」の設定を足した (§2.1・§2.8.1)。操作・状態は増やさず、純関数の主軸を選ぶだけ。

### 6.6 既定キーと「…」の追加 (2026-11-18、利用者決定)

利用者の提案で、順序ごとに別のキーを持たず「指定の順で次へ / 前へ」の 1 組にし、順序は全画面の「…」(その他の機能) からも選べるようにした (§2.1)。
既定キーは空きを確かめた上で (画像の全画面で既定キーも固定の処理も無い文字は A・F・H・J・K・W・Y)、利用者決定で `K` = 次へ・`J` = 前へ
(動画のマーカー移動と同じ向き、修飾キーなしで戻れる) とした (§2.7)。「…」の読み順は全体設定の切り替えで、本ごとには覚えない。

### 6.7 追加分のレビュー (r5、同じ session、2026-11-18): REVISE → 反映

P2: 既存の「…」の小パネルは、ファイル変更とは別に、キーを押すと閉じてからそのキーの操作を続ける。「ページを送っても閉じない」は
キーでは成り立たないので、既存のパネルと同じ扱い (キーで閉じる、ホイール・シークバー・クリックの送りでは閉じない) に揃えた (§2.1)。
P3: 保存済み keymap との競合の説明を Press 同士と Press / KeyHold に分けた (§5)。J / K の既定を足した後の説明 (§2.1・§2.7) を直した。
Z 字の手順・終端・単調性、小パネルの行数・保存・入力所有、J / K と動画・編集モードの既定の重なりは問題なしとされた。

<a id="implementation-record"></a>

## 7. 実装記録（2026-11-18、実装・対象検証完了）

**未コミット。実装・対象自動検証・文書更新を完了し、設計担当へ引き継ぐ。全件gateと実機での受入れは未実施。**
設計担当の §6.8（B）と §6.9（実効倍率）の決定へ追従して再開した。製品の実画面での観測はない。
以下の一覧は現在のソースを照合したもので、テストの定義と実行結果を分けて記録する。

### 7.1 変更したファイル

| ファイル | 現在の実装 |
| --- | --- |
| `src/settings.rs` / `src/settings_transfer.rs` | 3 設定・Unknown 正規化・既定・転送分類と件数、読み順の overflow 項目。未リリースの新設定なので migration は不要（serde default）。 |
| `src/keymap.rs` / `docs/keymap.ini.default` | FsImage の 6 操作、K/J と既定なしの 4 方向、ini 往復・既存 Press / KeyHold との文脈と競合の回帰。 |
| `src/key_input.rs` | cfg(test) の synthetic key に K を追加。保存済み K の Press / Hold と新しい既定キーを実 handler で検証するための fixture。 |
| `src/ui_dialogs/preferences/pages.rs` / `search_index.rs` / `src/ui_dialogs/preferences.rs` | 3 行・説明・検索 anchor・スクロール量見出し。実 UI → OK → 保存 → DB 再読込と取消のテスト。 |
| `src/app.rs` / `src/ui_fullscreen.rs` | 読み順小パネル、plain ↑↓ の typed consume 結果の分岐、ホイール分類・受理時の印・遅延適用・クリック予測、通常画像の描画完了後の印。小パネルのキー開閉は r5 の既存規則に従う。 |
| `src/displayed_image_transform.rs` | context 所有の `FullscreenPageLayout` に描画済み単位の印を追加し、既存の begin / clear で退役する。 |
| `src/ui_fullscreen/in_page_scroll.rs`（新規） | 実素材の幾何入力の鍵・照合、paint quad からの範囲、方向 / 2 軸 / N/Z の純関数、実効倍率を保つページ内スクロール専用の書込入口。 |
| `src/ui_fullscreen/in_page_scroll/handler_tests.rs` / `overflow_tests.rs`（新規） | 実描画・入力・viewport・context・設定 UI・小パネルの headless 回帰。 |
| `src/ui_fullscreen/in_page_scroll/interaction_tests.rs`（新規） | handler の既存 fixture helper を `pub(super)` で共有し、小パネルのキー / Esc / 非キー送りの開閉、キーとクリック、スクロール後の表示リセット、混在媒体と連結読みの入力所有を実経路で検証。`in_page_scroll.rs` にテストモジュールを登録。 |
| `src/ui_dialogs/context_shortcuts.rs` | ↑↓・ホイールの設定と適用される面に応じた説明。 |
| `src/ui_fullscreen/depth_tests.rs` | 既存の depth ホイールテストの `FsWheelAction` fixture に Legacy 分類と受理時情報の既定値を追加。 |
| `src/app/search_nav_entry_inventory_tests.rs` | 追加した 6 KeyAction を既存の網羅 match で非ナビゲーション操作に分類。 |
| `.config/nextest.toml` | 追加した環境設定 2 枚と読み順小パネル 1 枚の snapshot を既存 GPU group へ登録。 |
| `docs/keymap-spec.md` / `key-customization-impl-plan.md` / `display-pipeline.md` / `spec.md` / `detached-rework-plan.md` / `architecture-overview.md` | 操作・設定・layout の所有と照合、実効倍率と既存機構を維持する境界。detached の述語・viewport 配送・HWND の機構は変更しない。 |
| `htdocs/mimageviewer/manual/fullscreen.html` / `shortcuts.html` / `settings.html` | 3 設定、N/Z、J/K、任意 4 方向、端で送らない規則、クリック、読み始め、小パネルの開閉。版番号や実装用語は追加しない。 |
| 本書 §7 / `next-release-backlog.md` §1.474 | 実装・対象検証完了の状態と実装記録。設計担当の §6.8・§6.9 を保持する。 |
| `docs/ui-snapshot-policy.md` / `tests/snapshots/` の7枚 | 追加3枚・更新4枚と目視・通常比較の記録。画像の一覧と理由は §7.4。 |

### 7.2 設計判断への追従と所有境界

- **§6.8 / B**: layout の描画時と入力時の実効倍率が違う場合は、スクロールを消費するだけで動かさない。次の新しい描画済み layout から受け付ける。古い矩形を新倍率へ投影する helper の抽出は撤去した。
- **§6.9**: 比較するのは raw `fs_zoom` でなく、`fs_zoom_pan()` / `fs_zoom_pan_for` が描画へ渡す実効倍率（None なら 1、Some ならその倍率）。印の倍率も実ページの採用 transform から取る。
- **書込**: ページ内スクロールで pan が変わるときは専用の `write_fs_page_scroll_pan` から、同じ context の `fs_zoom` を layout の実効倍率へ揃えて `set_fs_pan_from_input` を呼ぶ。raw 0.9995 / pan 0 が 1 倍で描かれていた場合は 1.0 に揃え、パン開始で画像の寸法を変えない。動かない候補では書き込まない。
- **既存コードの維持**: `clamp_fullscreen_pan_from_layout` と `fs_zoom_pan_for` の関数本体は、現在の HEAD と文字列で同一と確認した。既定 Legacy のパン・ズームの式と正規化を変更しない。
- **範囲と採否**: 実際に描いた `paint_quad` の外接から描画時の pan を除いて範囲を求める。候補を既存 clamp に通し、意図した向きへの進みと直交軸の不変を確認して採る。N/Z の折り返しは 1 候補とし、同じ列・段を循環しない。
- **ホイール**: `InPageScroll` / `Legacy` は共通分類を使う。受理した `FsWheelAction` の `scroll_origin` を遅延適用でも保ち、描画の更新や前面 UI による segment 分割で、当初不適格だった入力を後から有効化しない。適用とクリック予測は同じ受理時の印・純関数を使う。これは frame 内の入力 proposal の所有であり、待機・再試行の永続状態は作らない。
- **context**: 新しい持続する実行時情報は layout の印だけ。既存の context swap と begin / clear を使い、layout・pan・zoom はその所有 context のみを読む／書く。
- **入力と UI**: Shift+↑↓・←→・スタックジャンプ・動画音声・前面 UI の所有と既定 Legacy を維持。読み順は環境設定と「…」の共通全体設定。ホイール・シークバー・クリックの送りでは小パネルを保ち、キーでは既存規則で閉じてその操作を続ける。

### 7.3 対象テストと snapshot の範囲

一覧抽出時点の追加テストは 72 件（名前は §7.5）。うち純関数 11 件、handler 36 件、interaction 8 件、小パネル 2 件、keymap 4 件、設定・転送・環境設定 UI・検索・snapshot の 9 件、ヘルプ 2 件。
追加72件は下記の対象実行で全て成功した。実行表には再実行と既存の回帰も含むため、実行件数は定義数とは異なる。

- 純関数は軸別のはみ出し・端外・2 軸・見開きと白 companion・自由回転・トリム・N/Z・LTR/RTL・clamp による循環防止を対象にする。
- handler は印と素材寸法・部分描画・holdover・幾何変更・viewport/DPI・ROOT 配送・上下設定 4 組・固定入力の consume 契約・J/K・前面 UI・wheel 分類と小デルタ・遅延入力の受理時の印・クリック予測・context swap・実設定 UI を対象にする。
- B の倍率変更後の消費と新描画後の再開、gap 200 の見開き、near-fit の幅10000ptの表示単位のキー／ホイールの複数ステップ・最終端・描画幅、ホイール予測と実適用の一致も対象にする。
- 保存済み K の Press / Hold と新しい既定 K の同時発火・既存消費順・新 Action の割り当て解除後の維持も、実 handler の追加 2 件で検証する。
- interaction は読み順小パネルのキーで閉じて操作する規則・Esc・ホイールや絶対ページ移動での維持、同 frame の ↓ + 単クリック、スクロール + ホイール + ダブルクリック、元の自動位置へ戻した後のクリック送り復帰、画像から動画／音声へ移る混在入力、縦横連結読みの既存ホイール所有を追加 8 件で検証する。
- snapshot は環境設定の明／暗 2 枚と読み順小パネル 1 枚を追加。既存 overflow の行数・高さ・root fixture も更新した。画像生成・通常比較・目視結果を §7.4 に記録した。
- libtest の型検証で見つかった fixture の新 field 不足、Action の網羅 match 不足、テスト helper の引数・borrow の不整合は修正し、対象実行は成功した。製品の runtime assertion が失敗した記録とは区別する。

### 7.4 検証記録

以下は実装担当が実行した対象検証と、その有効範囲の記録。初回の失敗と修正後の成功を区別する。

重いコマンドはすべて次の runner 経由。ロックを書けない場合は指定の警告付き fallback を使い、別の full-test がロックを持つ間は終了を待った。

```powershell
$env:TEMP = 'G:\home\mimageviewer-474\target\474-tmp'
$env:TMP = $env:TEMP
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner codex-474 -WorkDir G:\home\mimageviewer-474 -Command '$env:MSBUILDDISABLENODEREUSE=''1''; <下記のコマンド>'
```

| 確認 | 現時点の結果と証跡 |
| --- | --- |
| 初回 `cargo check -p mimageviewer --bin mimageviewer-core` | turbojpeg-sys の CMake/MSBuild 並列 6 が exit 1、cargo exit 101。本体の型検証前。`target/474-check.stdout.log` / `474-check.stderr.log`。 |
| 同依存の並列 6 再試行 | worktree 内 TEMP でも失敗。`MSBUILDDISABLENODEREUSE=1` を指定した再試行も並列 6 では失敗。MSBuild の「0 エラー」表示だけでは失敗原因を確定できない。`target/474-cmake.stdout.log` / `474-cmake.stderr.log`、`474-scroll-effective.stdout.log` / `474-scroll-effective.stderr.log`。 |
| 同依存の直列 1 | `--parallel 1 /nodeReuse:false` で成功。check graph と libtest graph の依存を直列で準備した。初回記録は `target/474-cmake-single.stdout.log` / `474-cmake-single.stderr.log`。依存のソース・製品 build script は変更しない。 |
| B 再開時の check | exit 0。`target/474-check-resume.stdout.log` / `474-check-resume.stderr.log`。実効倍率対応前の証拠として範囲を限定する。 |
| 実効倍率対応後の check | `cargo check -p mimageviewer --bin mimageviewer-core`、exit 0、58.75 秒。`target/474-check-effective.stdout.log` / `474-check-effective.stderr.log`。以後のRust変更はtest限定の追加・fixture修正で、本番ロジックは不変。 |
| 対象 nextest の初回依存ビルド | libtest graph の turbojpeg-sys が並列 6 で失敗し、テスト本体は未到達。`target/474-scroll-tests.stdout.log` / `474-scroll-tests.stderr.log`。 |
| 依存直列化後の対象 libtest | 型エラーを検出し修正。記録は `target/474-scroll-serial.stdout.log` / `474-scroll-serial.stderr.log`。 |
| 最初の対象実行 | `cargo nextest run -p mimageviewer --lib --build-jobs 1 -E 'test(/ui_fullscreen::in_page_scroll/)'`。ビルド 12分31秒、実行 76.721秒、57件中17成功・40失敗。38件は単ページの共通fixtureが Page / zoom=None に固定され、入力側の Width 等と一致しなかった。2件は原寸10000pxのテスト素材が既定の最大2048pxを超えた。`target/474-scroll-final.stdout.log` / `474-scroll-final.stderr.log`。本番コードは変えず、fixtureを実際の設定・倍率で描く形とテスト用のサイズ上限へ修正。 |
| 純関数テストの実行時間 | 上の17成功のうち非対称見開きの1件は71.297秒。候補ごとにAppを構築していたため、各テスト1個のAppを再利用する形へ修正した。全候補の `set_fs_pan_from_input` による冪等性検査は維持。 |
| fixture修正後の対象実行 | 同じ57件中49成功・8失敗、11.310秒。`target/474-scroll-fixture.stdout.log` / `474-scroll-fixture.stderr.log`。残った失敗は入力契約・素材サイズ等のテストfixtureを修正し、次の実行で再確認した。 |
| 入力契約・素材fixture修正後の対象実行 | `cargo nextest run -p mimageviewer --lib -E 'test(/ui_fullscreen::in_page_scroll/)'`。ビルド2分12秒、実行16.525秒、57/57成功。`target/474-scroll-contracts.stdout.log` / `474-scroll-contracts.stderr.log`。旧fixtureの失敗を本番不具合として扱わず、本番コードは変更していない。 |
| `test-focused.ps1` の5モジュール | `src/settings.rs`（334件）、`src/settings_transfer.rs`（43件）、`src/keymap.rs`（200件）、`src/key_input.rs`（102件）、`src/displayed_image_transform.rs`（48件）の計727件成功。runログは下記。 |
| 対象 UI / handler / context 等の回帰 | 下記filterで222件を実行、220成功・2失敗、165.188秒。4件の検索ナビゲーションinventoryは162.880〜164.887秒で成功（slow表示）。`target/474-regression.stdout.log` / `474-regression.stderr.log`。失敗2件のfixtureを修正し、下記の再実行で2件とも成功した。成功済み220件は変更の影響がなく再利用した。 |
| UI controls の失敗2件の再実行 | `cargo nextest run -p mimageviewer --lib -E 'test(fullscreen_n_scroll_order_selector_preserves_other_input_settings) \| test(calculated_overflow_panel_height_matches_all_states_and_scrolls_when_constrained)'`。2/2成功、3.176秒、ビルド4分01秒。前者はコンボのラベルで操作する形へ修正。後者は614ptのRootが11行でスクロールする現仕様に合わせ、内容の高さ不変のassertを維持し、Fits時のinner高さ・全Scrolls条件の末尾到達を検査。`target/474-ui-controls.stdout.log` / `474-ui-controls.stderr.log`。 |
| snapshot 生成・通常比較・目視 | 下記filterの9件に `UPDATE_SNAPSHOTS=1` を指定し9/9成功（10.527秒）。環境変数を外して同じ9件を通常比較し9/9成功（9.251秒）。実装担当が変更7枚をすべて開いて確認し、設計担当も新規3枚と root / root_tall を目視した。追加の欠け・重なり・選択表示不良なし。`target/474-snapshot-update.*.log` / `474-snapshot-compare.*.log`。画像一覧は下記。 |
| fmt / glyph / 差分確認 | `cargo fmt`（引数なし・workspace全体）成功、`python scripts/check_ui_glyphs.py` は危険文字0、`git diff --check` 成功。`target/474-fmt.*.log` / `474-glyphs.*.log` / `474-diff-check.*.log`。以後の変更は文書のみ。 |
| integration `ui_snapshot` | `cargo test --test ui_snapshot -- --test-threads=2`、259/259成功、229.58秒、ビルド3分45秒。`UPDATE_SNAPSHOTS` は未指定。`target/474-ui-snapshot.stdout.log` / `474-ui-snapshot.stderr.log`。追加のPNG変更なし。 |
| 全件 gate / 検証 build / 実機確認 | 利用者の指示どおり `test-lib-full.ps1` / `test-full.ps1` / `build-dev.ps1` は実行せず、製品バイナリも起動していない。全件gate・検証用ビルド・実機確認・コミットは設計担当へ引き継ぐ。設計判断待ちの項目はない。 |

native 依存の並列ビルド失敗は、CMake/MSBuild の実行段階の失敗であり、mIV のテスト assertion や実行時不具合と混同しない。
設計担当から、sandbox が起動した常駐 MSBuild ノードの再利用でアクセス拒否（MSB3191）になる既知事象との説明を受けた。
以後の cargo は指定どおり `MSBUILDDISABLENODEREUSE=1` を付けている。依存の準備では併せて直列化が必要だった。
共通ロックと外部 TEMP の作成は sandbox に拒否されたため、既存 runner の fallback と worktree 内の `target/474-tmp` を使った。
後続の直列ビルド成功・型エラー修正によって、実行していないテストや実機検証の成功を主張しない。

原寸10000pxの素材を作る旧fixtureはGPUの素材上限8192pxにも抵触するため、Height fit・1000×40pxの素材・
1000×400ptのviewportで実表示単位の幅10000ptを作る形に変更した。
`near_fit_wide_unit_key_scroll_keeps_exact_edge_and_painted_width_after_multiple_steps` と
`near_fit_wide_unit_wheel_prediction_and_apply_keep_exact_edge_after_multiple_steps` は、raw 0.9995 / 0.999等の
near-fit値から1.0へ揃え、端のpan=-4500と次の描画で空白が出ないことを維持して検査する。
これはテスト素材の修正であり、製品のサイズ制約や描画・入力の機能を変更した記録ではない。

5モジュールの成功runログ（すべて `target/test-focused/runs/` 配下、UTC日付は2026-11-17）:

- settings: `20261117T182505155Z-d6a8a254`（334成功、3.272秒）
- settings_transfer: `20261117T182731295Z-e3728ecf`（43成功、0.809秒）
- keymap: `20261117T183015786Z-e2d8c1ee`（200成功、4.302秒）
- key_input: `20261117T183027311Z-b4a07152`（102成功、1.434秒）
- displayed_image_transform: `20261117T183033244Z-fcfd3a51`（48成功、0.730秒）

222件の回帰filter（`target/474-verify-remaining.ps1` から転記。`cargo nextest run -p mimageviewer --lib -E` に渡した値）:

```text
test(/(wheel_frame_tests|initial_placement::tests|overflow_input_tests|front_popup_input_tests|context_shortcuts::tests|fullscreen_in_page_scroll|fullscreen_n_scroll_order|calculated_overflow_panel_height|fullscreen_overflow_rows|fullscreen_overflow_panel_(is_excluded|tall)|normal_video_wheel_setting|music_ctrl_wheel_blocks|seek_bar_hover_does_not|search_nav_entry_inventory|depth_tests::)/) & !test(/snapshot/)
```

`target/474-verify-ui.ps1` で上記UI controlsの2件再実行、snapshotの更新→通常比較、
`cargo test --test ui_snapshot -- --test-threads=2` を順に実行した。別のfull-testのロック解放を待ち、04:14:32に開始した。

snapshotのfilter:

```text
test(/(fullscreen_overflow_panel_snapshot|fullscreen_in_page_scroll_settings_snapshot|preferences_viewer_notice_visibility_snapshot)/)
```

変更したPNG（7枚、計285,387 bytes）:

| `tests/snapshots/` のファイル名 | 種別・寸法 | 理由 |
| --- | --- | --- |
| `fullscreen_in_page_scroll_settings_light_default.png` | 追加・540×220 | 明色テーマの既定3設定と説明。 |
| `fullscreen_in_page_scroll_settings_dark_enabled.png` | 追加・540×220 | 暗色テーマのページ内スクロール2設定とZ字の選択。 |
| `fullscreen_overflow_panel_reading_order.png` | 追加・720×340 | 読み順の2択、選択中、戻る、閉じる。 |
| `fullscreen_overflow_panel_root.png` | 更新・720×440 | 読み順の行を追加。高さ制約により下端はスクロール領域でclip。 |
| `fullscreen_overflow_panel_page_turn_on.png` | 更新・720×440 | 同上。既存アニメーションON状態を維持。 |
| `fullscreen_overflow_panel_root_tall.png` | 更新・720×680 | 11行全てを収める。従来の720×614からfixtureの高さを変更。 |
| `preferences_viewer_notice_visibility.png` | 更新・560×360 | 同じ設定ページに3行増えたことによるスクロールバーの長さだけ。差は右端30px、bbox `(544,42,552,48)`。本文はpixel単位で同一。 |

寸法・ファイルサイズ・SHA-256・pixel差の集計は `target/474-snapshots.json`。
既存navigator小パネル・slideshowのPNGはbyte単位で不変。他のPNGの変更もない。
実装差分の独立レビューは、この記録の後に別の担当 (Sol / xhigh) が行う (§6 に結果を記録する)。
最終のファイル38件・追加テスト72件の掲載一致・PNGのhash不変・既存clampと倍率正規化のHEAD一致は `target/474-final-audit.log` に記録した。HEADは `10d6323c6` のまま。

### 7.5 現在ソースから抽出した追加テスト名

#### `src/keymap.rs` (4)

- `page_scroll_actions_have_image_scope_single_press_and_expected_defaults`
- `page_n_scroll_defaults_keep_video_markers_and_edit_buckets_disjoint`
- `page_n_scroll_old_press_overrides_conflict_roundtrip_and_can_disable_new_defaults`
- `page_n_scroll_old_scrub_hold_conflict_roundtrip_and_none_preserve_hold_binding`

#### `src/settings.rs` (2)

- `fullscreen_in_page_scroll_settings_default_and_round_trip_independently`
- `fullscreen_in_page_scroll_settings_normalize_unknown_to_existing_defaults`

#### `src/settings_transfer.rs` (1)

- `fullscreen_in_page_scroll_transfer_roundtrip_missing_and_invalid_preserve_values`

#### `src/ui_dialogs/preferences.rs` (1)

- `fullscreen_in_page_scroll_preferences_ui_ok_save_reread_reopen_and_cancel`

#### `src/ui_dialogs/preferences/pages.rs` (3)

- `fullscreen_in_page_scroll_settings_snapshot_light_default`
- `fullscreen_in_page_scroll_settings_snapshot_dark_enabled`
- `fullscreen_n_scroll_order_selector_preserves_other_input_settings`

#### `src/ui_dialogs/preferences/search_index.rs` (1)

- `fullscreen_in_page_scroll_settings_are_searchable`

#### `src/ui_dialogs/context_shortcuts.rs` (2)

- `image_fixed_help_resolves_independent_settings_only_on_the_scroll_surface`
- `image_help_shows_default_n_scroll_and_custom_cardinal_assignment`

#### `src/ui_fullscreen.rs` (1)

- `fullscreen_overflow_panel_snapshot_reading_order`

#### `src/ui_fullscreen/in_page_scroll.rs` (11)

- `axes_overflow_independently_and_stop_at_aligned_edges`
- `outside_edge_moves_only_inward_without_snapping_to_near_edge`
- `diagonal_wheel_keeps_moving_the_axis_that_has_not_reached_its_edge`
- `scroll_geometry_removes_drawn_pan_without_scaling_the_unit`
- `ordered_primary_and_wrap_moves_share_n_and_z_rules_in_both_directions`
- `ordered_scroll_without_primary_overflow_uses_only_secondary_axis`
- `asymmetric_original_spread_never_repeats_a_column_or_row_after_clamp`
- `clamp_cannot_reverse_wheel_or_correct_an_unrequested_axis`
- `white_companion_counts_for_edges_but_vacant_slot_does_not`
- `saved_and_free_rotation_use_the_painted_quad_including_negative_angles_and_trim`
- `empty_continuous_and_z_layouts_do_not_scroll`

#### `src/ui_fullscreen/in_page_scroll/handler_tests.rs` (36)

- `current_receipt_rejects_missing_generation_other_page_and_recovers_after_new_paint`
- `holdover_partial_pair_and_comparison_paints_cannot_publish_scroll_receipts`
- `geometry_key_rejects_same_frame_fit_rotation_free_rotation_gap_direction_and_limits`
- `replacement_material_key_uses_dimensions_rather_than_texture_identity`
- `resized_or_dpi_changed_viewport_rejects_old_receipt`
- `root_dispatch_uses_destination_viewport_size_and_dpi`
- `changed_zoom_skips_scroll_until_new_paint_then_cancels_initial_request`
- `up_down_handler_obeys_all_four_independent_settings_combinations`
- `up_down_short_tap_multiple_edges_and_released_repeat_move_at_most_once`
- `shift_arrows_and_horizontal_arrows_keep_page_navigation`
- `fallback_menu_consumes_replacement_arrows_without_moving_or_turning`
- `default_j_k_actions_work_with_page_turn_settings_and_use_the_selected_order`
- `new_action_is_left_for_front_ui_or_surface_outside_but_consumed_for_stale_layout`
- `wheel_classifier_preserves_modifier_seek_and_surface_legacy_routes`
- `wheel_prediction_uses_fractional_two_axis_scroll_and_stops_after_legacy_navigation`
- `zoom_then_page_scroll_prediction_discards_scroll_for_the_unpainted_zoom`
- `deferred_wheel_projects_only_segments_remaining_after_front_ui_consumption`
- `context_swap_keeps_receipt_and_scroll_pan_with_the_owning_context`
- `changed_slice_and_spread_screen_order_do_not_match_previous_unit`
- `pending_navigation_sequence_consumes_scroll_without_using_previous_pixels`
- `fit_change_and_down_in_one_real_handler_pass_discard_old_geometry`
- `native_and_egui_down_share_one_consume_and_use_the_edge_modifier`
- `stack_shift_arrow_stays_in_stack_route_even_with_page_scroll_enabled`
- `actual_wheel_acceptance_keeps_fractional_scroll_and_legacy_thresholds_separate`
- `real_preferences_controls_ok_persistence_then_handler_and_wheel_use_new_values`
- `focused_text_input_keeps_new_action_and_up_down_away_from_the_canvas`
- `wheel_and_single_click_same_frame_uses_the_scrolled_view_before_deciding_navigation`
- `key_zoom_and_down_same_frame_consume_scroll_then_new_paint_can_reach_the_edge`
- `ctrl_wheel_then_scroll_consumes_without_projection_and_resumes_after_new_paint`
- `gap_200_spread_rejects_old_zoom_geometry_and_aligns_to_the_new_painted_edge`
- `near_fit_zoom_normalization_does_not_permanently_discard_keys_or_wheel`
- `near_fit_wide_unit_key_scroll_keeps_exact_edge_and_painted_width_after_multiple_steps`
- `near_fit_wide_unit_wheel_prediction_and_apply_keep_exact_edge_after_multiple_steps`
- `deferred_scroll_keeps_its_acceptance_receipt_across_repaint_and_click_prediction`
- `saved_k_scrub_hold_and_default_k_press_both_reach_real_handlers_and_none_preserves_hold`
- `saved_k_press_runs_in_existing_handler_order_before_new_page_scroll_press`

#### `src/ui_fullscreen/in_page_scroll/overflow_tests.rs` (2)

- `reading_order_overflow_real_controls_share_and_persist_the_global_setting`
- `reading_order_overflow_is_global_in_all_image_modes_and_closes_like_existing_panels`

#### `src/ui_fullscreen/in_page_scroll/interaction_tests.rs` (8)

- `reading_order_keys_close_the_panel_then_continue_their_existing_action`
- `reading_order_escape_matches_the_existing_root_panel_close_and_forward_policy`
- `reading_order_non_key_wheel_and_absolute_page_navigation_keep_the_panel_open`
- `down_and_single_click_same_frame_uses_the_key_scrolled_view`
- `page_scroll_then_same_frame_wheel_and_double_click_resets_without_turning`
- `returning_to_the_automatic_origin_with_scroll_keys_reenables_single_click`
- `mixed_image_to_audio_or_video_keeps_legacy_consumer_and_discards_accepted_scroll`
- `vertical_and_horizontal_real_wheel_keep_the_legacy_continuous_scroll_owner`

### 7.6 更新した既存テストとfixture

- `settings_transfer::tests::all_settings_fields_are_classified`：分類501件・転送対象164件・wire162件へ更新。転送用の非既定fixtureにも3設定を追加。
- `ui_fullscreen::depth_tests::depth_wheel_navigates_and_ctrl_wheel_never_uses_panorama_fov_or_hidden_zoom`：既存ホイール入力にLegacy分類と受理時情報を追加。
- `ui_fullscreen::tests::calculated_overflow_panel_height_matches_all_states_and_scrolls_when_constrained`：読み順を状態一覧へ追加し、Root11行で制約が生じる画面も末尾到達を検査。
- `ui_fullscreen::tests::fullscreen_overflow_panel_tall_viewport_keeps_the_production_body_height`：11行が収まる680pt・720ptで検査。
- `ui_fullscreen::tests::fullscreen_overflow_panel_snapshot_root` / `fullscreen_overflow_panel_snapshot_navigator_submenu` / `fullscreen_overflow_panel_snapshot_root_tall_without_scrollbar` / `fullscreen_overflow_panel_snapshot_page_turn_on` / `fullscreen_overflow_panel_snapshot_slideshow`：共有fixtureに読み順の行と選択値を追加。tallの高さを更新。navigator・slideshowのPNGは不変。
- `ui_dialogs::preferences::tests::preferences_viewer_notice_visibility_snapshot`：同じ設定ページの内容増加に合わせ、スクロールバーだけを更新。

検索ナビゲーションinventoryの補助 `key_entry` は6操作を非ナビゲーションに分類した。
既存の音楽小パネル等の描画fixtureには新しいview項目の既定値を補った。本番の分岐は変更していない。

### 7.7 独立実装レビュー r1 の指摘対応（2026-11-18、修正・関連検証完了）

基準コミットは `39e3670c5`。今回の修正は実装担当が行い、コミット・製品起動・build-dev・全件テストは行わない。
初回記録の確認担当の出どころは設計担当による訂正を保持する。今回の修正差分の独立再レビューは未実施。

| ファイル | 変更 |
| --- | --- |
| `src/ui_fullscreen.rs` | 新Actionの個別のspread/fit判定を既存の `fs_front_menu_presented` に統一。回転等の前面popup中は新Actionを消費しない。入力用の構成peekを追加し、既存のunit token・navigation・完全性の取得済み証明だけを使う。固定矢印の受付と置き換え点は変更しない。 |
| `src/app.rs` | `peek_rotation` を追加。数値cacheと取得済みの構成本の回転だけを読む。既存 `get_rotation` のDB取得・memoizeは変更しない。 |
| `src/ui_fullscreen/canonical_demand.rs` | 既存のresident trim readerを同じfullscreen内で共有する。test用のmetadata producer禁止guard入口を追加。 |
| `src/ui_fullscreen/in_page_scroll.rs` | 世代を先に照合し、表示単位・sliceの不一致も素材幾何より先に棄却。入力幾何の取得を `&self` にし、回転・自動trim・構成・seek情報の未取得時には取得処理を起こさず不一致とする。Noneという取得済みtrim回答と未取得を区別する。 |
| `src/ui_fullscreen/in_page_scroll/review_regression_tests.rs`（追加） | metadata guard下の実handler、cache不取得・次の描画後の再開、固定矢印の既存受付を検査。 |
| `src/ui_fullscreen/front_popup_input_tests.rs` | 実際にマウスで開いた回転popupのフォーカスを外し、K/J/割当済み方向Actionのpan不変と未消費を検査。 |
| 本書 §6.10・§7.7 | 指摘の出どころ、対応と検証結果を記録。 |

入力側の自動trimは既存のresident readerを使い、非回転pairの調和、回転側のtrim抑止、白companionのsideも従来の描画と同じ規則で解く。
取得済みのAnimatedエントリでは、既存の `cached_margin_bbox` が自動解析を適用しないため、その確定した「trimなし」を読む。静止画の未取得／失効cacheをNoneで埋めることはしない。
構成のpeekはNative専用の厳しい受理条件を流用せず、通常描画が必要とする完全性の証明だけを要求する。
キャッシュが揃わない入力を待機・再試行せず消費のみとする点、次の通常描画で取得した値から再開する点は設計どおり。
設計との差分はない。

追加テスト名:

- `mouse_opened_unfocused_rotation_popup_keeps_page_scroll_keys_unconsumed`
- `missing_rotation_is_consumed_without_metadata_production_and_new_paint_recovers`
- `missing_or_replaced_auto_trim_is_consumed_without_analysis_and_new_paint_recovers`
- `missing_composition_is_not_rebuilt_by_input_and_new_paint_recovers`
- `ordinary_spread_does_not_require_an_unused_native_completeness_proof`
- `animated_auto_trim_retains_the_known_untrimmed_value_without_analysis`
- `missing_seek_geometry_is_not_populated_by_scroll_matcher_and_new_paint_recovers`
- `stale_generation_and_page_skip_geometry_without_loading_metadata`
- `rotation_popup_keeps_fixed_arrows_at_the_existing_replacement_point`

指定runnerと `MSBUILDDISABLENODEREUSE=1` を使って検証した。

```text
cargo check -p mimageviewer --bin mimageviewer-core
cargo nextest run -p mimageviewer --lib -E 'test(/ui_fullscreen::in_page_scroll/) | test(/ui_fullscreen::front_popup_input_tests/) | test(/ui_fullscreen::canonical_demand/)'
cargo nextest run -p mimageviewer --lib -E 'test(/missing_seek_geometry_is_not_populated_by_scroll_matcher_and_new_paint_recovers/)'
cargo fmt
```

`cargo check` は成功（36.14秒）。nextest初回は101件中100件成功・1件失敗（実行57.767秒）。
追加のseek欠落テストが、handler全体の実行後もseek情報を使えないままと仮定してpan不変を要求していた。
既存のhandler前段は `page_overview_available` を呼び、`src/app/page_overview.rs` の `page_overview_unavailable_reason` がseek情報を準備するため、この仮定が誤っていた。この既存経路は変更しない。
このテストは照合処理自体をguard下で直接呼び、cache不変・不一致・pan不変を検査する形へ修正した。
次の通常描画後の再開は実handlerで検査する。回転・自動trim・構成の欠落は初回からguard下の実handlerテストが成功した。
修正したseekテストの再実行は1件成功・失敗0件（実行0.665秒、再ビルド2分51秒）。
初回に成功した100件の後は本番コードを変更しておらず、その結果を再利用する。対象101件の有効な成功結果が揃った。
依存ビルドの失敗なし。共有ロック待ちの後、sandboxの書き込み制約によりrunner指定の低優先度・6並列で実行した。
TEMP/TMPはworktree内の `target/474-tmp` を使用した。
引数なしの `cargo fmt` と `git diff --check` も成功。

既存の `get_rotation`・`get_rotations_for_indices`・`cached_margin_bbox`・`clamp_fullscreen_pan_from_layout` の関数本体は基準コミットと同一であることを照合した。
UI文言と描画は変更していない。PNGの更新なし。修正差分の独立目視・独立再レビューは未実施。

ログ（すべて `target/` 内）: `474-r1-check.stdout.log` / `474-r1-check.stderr.log`、`474-r1-tests.stdout.log` / `474-r1-tests.stderr.log`、`474-r1-seek.stdout.log` / `474-r1-seek.stderr.log`、`474-r1-static.log`、`474-r1-fmt.stdout.log` / `474-r1-fmt.stderr.log`、`474-r1-diff-check.stdout.log` / `474-r1-diff-check.stderr.log`。

### 7.8 独立実装レビュー r2 の指摘対応（2026-11-18、修正・関連検証完了）

基準コミットは `47f42bab6`。利用者から伝えられた r2 の P2 2件・P3 1件に対応する。
修正差分の独立再レビュー・独立目視は未実施。設計担当による既存の出どころの訂正は保持する。

| ファイル | 変更 |
| --- | --- |
| `src/ui_fullscreen.rs` | 前面メニューの共通判定に照会先viewportを渡せる入口を追加。同一viewportの判定は既存のまま、新Actionだけ配送先を照会する。完全性が必要な条件を `spread_composition_requires_complete`、有効なpage_aloneの決定を `effective_spread_page_alone` に集約し、通常の構成producerとpeekで共用。 |
| `src/ui_fullscreen/in_page_scroll.rs` | 新Actionの入力先と幾何の入力先を同じ配送先helperから取得。r1の配送元前面UI判定を残したまま配送先も照会する。自動trimはdetached runtimeの回答を優先し、未保存なら描画済み画素の検出cacheを読む。 |
| `src/ui_fullscreen/canonical_demand.rs` | 既存のload_seq・画素pointerを照合するreaderを `resident_pixel_auto_trim` に分けて共用。canonical collectorのdetached runtime必須の規則は維持。 |
| `src/ui_fullscreen/in_page_scroll/handler_tests.rs` | 既存のviewport指定のheadless fixtureを回帰テストから利用可能にする。 |
| `src/ui_fullscreen/in_page_scroll/review_regression_tests.rs` | ROOT転送・detached RAWプレビュー・通常とpeekの構成方針の回帰テストを3件追加。 |
| `docs/detached-rework-plan.md` §11の§1.474記録 | 入力照合側で触れた読み取り範囲と、状態・保存規則を変更しない理由を追記。 |
| 本書 §6.11・§7.8 | r2の指摘の出どころ・対応・検証結果を記録。 |

追加テスト名:

- `root_dispatched_scroll_keys_leave_the_destination_rotation_popup_unconsumed`
- `detached_raw_auto_trim_uses_the_painted_pixel_cache_and_keeps_runtime_priority`
- `cached_and_ordinary_composition_share_completeness_and_page_alone_policy`

ROOT転送は実際の `handle_fullscreen_root_key_input` にK/Jを渡し、配送先のフォーカスなし回転Popup中のpan不変・未消費と、閉じた後の受付、配送元ROOTの前面UI所有も保つことを検査する。
detached RAWは既存のregistryに所有contextを作り、描画前の消費のみ、通常描画後のスクロール、load_seq・画素pointer失効時の不取得、runtimeに保存済みのNoneの優先を検査する。
構成は表紙・先頭/末尾配置・page_aloneの8組で通常producerとpeekの結果、証明欠落時のpeek不成立、通常producerでの再取得を検査する。
新しい実行時状態、待機・再試行、同期読み込みは追加しない。設計との差分はない。

範囲外の既存ROOT経路（コード上の所見）:
`src/ui_fullscreen.rs` の `handle_fullscreen_root_key_input` はROOTのctxを共通handlerに渡す。
そのhandlerの既存の `fs_still_seek_strip_popup_open(ctx)` 早期return（`src/ui_fullscreen.rs:35388`）と `consume_arrow`（同:36350）は配送先のPopupを照会しないため、
配送先シークPopupが開いていてROOT側ownerが遮断しないとき、固定矢印等が背後へ進み得る。今回、この既存経路は変更しない。
実機での観測ではなくコードの参照による所見である。

指定runnerと `MSBUILDDISABLENODEREUSE=1` で次を実行した。

```text
cargo check -p mimageviewer --bin mimageviewer-core
cargo nextest run -p mimageviewer --lib -E 'test(/ui_fullscreen::in_page_scroll/) | test(/ui_fullscreen::front_popup_input_tests/) | test(/ui_fullscreen::canonical_demand/) | test(/final_cover_composition/) | test(/page_alone/) | test(/book_composition_tests/) | test(/ui_view_trim::tests/) | test(/retained_material_entry_.*trim/) | test(/final_demand_topology_tests/)'
cargo fmt
```

初回checkは成功（46.82秒）、nextestは196件中195件成功・1件失敗（実行62.097秒、ビルド3分01秒）。
ROOT回帰fixtureが配送先viewportをパスごとに登録しておらず、eguiの未使用viewport GCでPopupが破棄されていた。
fixtureの登録を直した単独再実行は1件成功（0.634秒、ビルド2分11秒）。
その後、配送元ROOTの既存の前面UI判定を保持するコードと検査も加えたため、最終checkと同じ196件を再実行した。
最終checkは成功（17.79秒）。最終nextestは196件成功・失敗0件（実行60.508秒、ビルド2分14秒）。
依存ビルドの失敗なし。sandboxの書き込み制約により指定runnerの低優先度・6並列で実行し、TEMP/TMPはworktreeの `target/474-tmp` を使用した。
引数なしの `cargo fmt` と `git diff --check` も成功。
既存のROOT転送handler、pan clamp、自動トリムの解析・detached保存関数の本体が基準コミットと同一であることを静的照合した。

ログ（すべて `target/` 内）: `474-r2-check.stdout.log` / `474-r2-check.stderr.log`、`474-r2-tests.stdout.log` / `474-r2-tests.stderr.log`、`474-r2-root.stdout.log` / `474-r2-root.stderr.log`、`474-r2-final-check.stdout.log` / `474-r2-final-check.stderr.log`、`474-r2-final-tests.stdout.log` / `474-r2-final-tests.stderr.log`、`474-r2-static.log`、`474-r2-fmt.stdout.log` / `474-r2-fmt.stderr.log`、`474-r2-diff-check.stdout.log` / `474-r2-diff-check.stderr.log`。
コミット・build-dev・全件テスト・製品起動は行わない。UI文言・PNGの更新なし。

### 6.8 実装担当の停止報告と決定 (2026-11-18)

実装担当 (Codex gpt-6.1-sol / high、session 01a1ea54) が、§2.3 の「同じ frame の倍率変更は既存 clamp と同じ投影で扱う」と実描画の矛盾を報告して停止した
(既存の投影は通常の見開きの間隔も倍率比例にするが、描画は間隔を固定する)。設計担当の決定: (B) 同じ frame に倍率が変わったらスクロールは消費のみ。
共有の投影は変えない (既定設定の手動パン・ズームに影響させない)。§2.3・§2.4・§2.6・§2.10・§5 を直した。

### 6.9 実装担当の 2 回目の停止報告と決定 (2026-11-18)

既存の倍率の正規化 (`fs_zoom_pan_for`) は、1 付近の倍率 (例 0.9995) を pan 0 では 1 倍で描き、pan が付くとその倍率で描く。
`fs_zoom` が変わらなくても、スクロールで pan が付いた瞬間に画像の寸法が変わり、端に空きが残る (原寸幅 10000 で約 2〜3pt、実装担当のコード上の計算)。
設計担当の決定: 照合は描いたときの実効の倍率で比べ、スクロールで pan を書くときは `fs_zoom` にもその実効の倍率を書く (§2.3)。
既存の正規化と手動パン・ズームの挙動は変えない。

### 6.10 独立実装レビュー r1（REVISE、P2 2件）と対応（2026-11-18）

利用者から、別担当 Sol / xhigh による独立実装レビューの結果を受領した。
設計担当もコードを読み、2件とも妥当と判断したとの連絡を受けた。修正の記録は §7.7。

| 指摘 | 対応 |
| --- | --- |
| P2-1: フォーカスのない回転popupの背後で新Actionが消費・実行される | 既存の前面メニュー所有判定に統一し、マウスで開いた回転popupで実handlerを検査。固定矢印・置き換え点の受付は維持。 |
| P2-2: 入力の幾何取得が回転の同期DB SELECT・自動trim解析・構成の構築に到達し得る | 取得済みmetadataのみのreaderへ変更。世代・表示単位を先に照合し、cache missは消費のみ。metadata producer禁止guardとcache不変・描画後の復帰を検査。 |

この節は受領した指摘と対応の記録であり、修正差分の独立再レビュー・再受入れを行った記録ではない。

### 6.11 独立実装レビュー r2（REVISE、P2 2件・P3 1件）と対応（2026-11-18）

利用者からr2の結果と、設計担当もコードを読んで妥当と判断したとの連絡を受けた。対応の詳細は §7.8。

- P2-1: 新Actionだけ配送先viewportの既存Popup記憶を照会する。配送元の既存の前面UI判定も保つ。既存の固定矢印・その他のROOTキー経路は維持し、同種の範囲外の所見を記録する。
- P2-2: detached runtime回答を優先し、未保存の場合は描画で取得済みの画素cacheを検証して読む。自動解析とruntime保存規則は変更しない。
- P3: 完全性が必要な条件と有効なpage_aloneを共通の純粋helperへ集約する。通常producerとpeekは証明の取得方法を分ける。

これは受領したレビューと実装対応の記録であり、この修正差分への独立再レビュー・受入れの記録ではない。

### 6.12 独立実装レビュー r3（ACCEPT、2026-11-18）

同じレビュー担当 (Sol / xhigh、session 01a1eb01) が 6ba253d26 を確認し、P1〜P3 なしで ACCEPT。r2 の 3 件の解消、共有 helper が旧条件と同値で既定の見開き構成を変えないこと、
§7.8 の範囲外記録 (既存 ROOT 経路のシーク Popup 判定・固定矢印が配送先の Popup を照会しない) が正確であることを確認した。
detached の読み取り側の変更 (取得済みトリムの照合) は、レビュー担当と設計担当の双方が「症状パッチではなく構造的修正である」ことに合意した (docs/detached-rework-plan.md の §1.474 の記録)。

### 7.9 最終の全件テストと検証用ビルド (2026-11-18、設計担当が実行)

- 6ba253d26 (コードは 7f29bc310 と同じ) で `.\scripts\test-lib-full.ps1` (run-heavy 経由): 18,617 件成功・81 件 skip・失敗 0 (1,334 秒)。
  39e3670c5 時点の全件では、無関係の 2 件 (`app::tests::scroll_661::retired_details_cell_release_does_not_consume_later_mouse_hold`、
  `favorite_view_state::tests::sidecar_restore_barrier_observes_published_result_after_done`) が実装担当のビルドと並行した負荷の下で失敗し、単独の再実行では 2 件とも成功した。
- `.\scripts\build-dev.ps1`: 成功 (`target\dev-runtime\mimageviewer-core.exe`)。製品バイナリは起動していない。実画面での確認は利用者の実機確認を待つ。

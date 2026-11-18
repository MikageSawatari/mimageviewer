# 全画面のパンの制限と前面メニュー中のキー入力 (§1.806・§1.807・§1.810) 設計書

- 状態: 独立実装レビューround 3のP2を修正し、対象22件・core check成功（2026-11-19、§6.8、基点 `aa5763c22`）。先行する§6.6・§6.7の裁定①反映と対象297件・core checkの成功履歴は§6.5。未コミット、再レビュー／全件gate／実機検収は設計担当の後続工程。実装前の独立レビュー: 案 B は r4 で ACCEPT、§1.810 を案 C (記録を持たず毎回収める) へ切り替えて r5〜r7 は REVISE、r8 で ACCEPT (§7)。
- ブランチ: `fix-fs-pan-input-806-807-810` (作業ツリー `G:\home\mimageviewer-474`、master 605ba335b 起点)。
- 担当: 設計・検収 = ClaudeCode Opus 5.5 / high、実装 = Codex gpt-6.1-sol / high、独立レビュー = 別の gpt-6.1-sol / xhigh。
- 元の項目: [next-release-backlog.md](next-release-backlog.md) の §1.806・§1.807・§1.810。どれも §1.474 ([fullscreen-in-page-scroll-plan.md](fullscreen-in-page-scroll-plan.md)) の作業中に見つかった既存の問題。
- まれな失敗は既定の割り切り (ログ + 通知 [+ 作り直し])。この 3 件に永続データの変更は無い。

## 1. 共通の土台

通常のページ単位表示 (単ページ・見開き。Z ズーム・連結読み・360・比較は対象外) のパンは、
`clamp_fullscreen_pan_from_layout` (`src/ui_fullscreen.rs`) が「少なくとも 1 ページの 48px (`FS_PAN_MIN_VISIBLE_PX`) を画面内に残す」範囲に収める。
この制限は入力 (`set_fs_pan_from_input`・ホイールズーム・§1.474 のページ内スクロール) のときにだけ、前の frame に描いた layout から計算する。

- §1.806: 倍率が変わる入力で、前の frame の矩形を倍率の比で投影するとき、通常の見開きのページ間隔まで比例させてしまう (実際の描画は間隔が固定)。
- §1.810: 入力を伴わずに描く形が変わる (回転) と、制限を計算し直す機会が無い。
- §1.807: パンとは別。前面メニューが開いているときに、背後の表示を動かすキーを受け付けるかの判定が、入口によって違う。

## 2. §1.806 見開きのページ間隔の投影

### 2.1 現象 (コード)

`layout_spread_page_rects` は、左右のページの見える幅を倍率で伸ばし、間隔 `spread_gap` は倍率に関係なく固定で置き、全体を中心にそろえる。
`clamp_fullscreen_pan_from_layout` は各ページの描いた矩形を「中心 + パン」の基準点から倍率の比 `r` で投影するので、
左ページの右端は `基準点 - (g/2)·r` に投影されるが、実際の描画では `基準点 - g/2` になる。差は `g·(r-1)/2` (間隔 200・倍率 1→2 で 100pt)。
`SingletonSpread` (白ページ付きの単独ページ) には同じ補正 `singleton_spread_projected_gap_correction` があり、通常の 2 ページの見開きにだけ無い。

### 2.2 決定

- 投影で使う「固定の間隔と、間隔に対するページの側」は、描画で間隔を置いた側 (layout の作り手) が記録し、clamp はそれを読むだけにする。
  clamp の時点で設定 (`spread_page_gap_px`) を読み直さない (前の frame の描画と同じ値でなければ投影が合わないため)。
- 補正の式は `SingletonSpread` と同じ 1 つの helper にまとめる (側と間隔を受け取る形)。単独ページと 2 ページの見開きで式を 2 か所に書かない。
- 記録の置き場所は実装担当が決めてよい (例: `ResolvedDisplayPlacement::Normal` に見開きの側と間隔を持たせる、`FullscreenPageLayout` の見開きの組に間隔を持たせる)。
  条件は「描画で使った値そのもの (物理 px への丸め後) を、描いた frame の layout と一緒に持つ」こと。
- 縦方向・回転したページ・トリムのあるページでも、間隔は横方向にだけ入る (`layout_spread_page_rects` のとおり)。

### 2.3 テストで守る不変条件

「前の frame の layout から倍率 `z1` へ投影した各ページの矩形」が、「倍率 `z1` で実際に描いた layout の矩形」と、許容誤差の範囲で一致する。
比較は `layout_spread_page_rects` 単体ではなく、最終の transform (矩形寸法・トリム境界・見開き間隔の整列まで、`displayed_image_transform.rs` の整列と描画側の間隔の整列を通した後) で行う。

- 許容誤差: 固定の上限は置かない。寸法の切り捨てと間隔の整列が重なると、投影の丸め誤差は倍率の比で拡大され、固定の式で上限を保証できない
  (r2 の例: 1009×10000 の同寸 2 ページ・高さフィット・viewport 1000×1000・DPI 1・間隔 0・`pan.x = 0.6`・倍率 1→10 で約 12.6px)。
  この誤差は間隔に関係なく今もある投影の性質で、今回の修正の対象ではない。テストは**同じ条件で間隔だけを 0 にした場合の差**を基準にし、
  間隔 `g` のときの差が「基準の差 + 1 物理 px」以内であることを見る (補正が間隔の項 `g·|r-1|/2` だけを消していることを確かめる)。
  補正が無い場合の差は基準より `g·|r-1|/2` 大きいので、テストの条件 (間隔 200・`r` = 2 または 0.5) で区別できる。
- 条件: 基準の倍率が間隔に依存しない設定に限る (高さフィット、原寸 100%、指定スケールの高さだけを指定した場合)。幅を指定する指定スケールも間隔に依存する (`fullscreen_specified_scale.rs`)。幅フィット・ページ全体フィットは間隔で基準の倍率が変わり、丸め方も変わるので、
  この比較には使わない (r3 の例: 左 529×1000・右 351×1000・幅フィット・viewport 1000×1000・`pan.x = 0.6`・倍率 1→2 で、間隔 0 の差 0.4px に対し間隔 200 で 2.4px)。
  幅フィットでは次の 2 つを分けて見る: 投影した座標が符号付きの補正量 (`±g·(r-1)/2`) だけ変わること、間隔 200 の例で最終の描画との差 (絶対値) が補正の無い場合より小さいこと
  (r4 の例: 97.6px → 2.4px。減る量は補正量と一致しない)。
  見開き (間隔 0 / 200、左右の幅が違うページ、トリムあり、90° 回転したページ)、倍率 1→2・2→1・1→4 (この範囲で保証する)、DPI 1.0 / 1.25 / 1.5、端数の寸法。
- 端に当てたときに止まる位置が、次の frame で描いた layout から計算し直した位置と、同じ基準 (間隔 0 の場合の差 + 1 物理 px) で一致すること。

## 3. §1.810 回転したあとのパンの制限

### 3.1 現象 (サブ PC 担当の観測とコード)

観測 (サブ PC 担当、§1.474 の確認 4-7): 縦長の画像を横幅フィットで全画面にし、R → 横の端までスクロール → L で戻すと真っ黒になり、↓↑・Shift+ホイールでも戻らない。
コード: 回転 (`apply_rotation`) は `fs_pan` / `fs_zoom` に触れず、描く形が変わっても制限を計算し直す経路が無い。

### 3.2 案の比較と決定

- 案 A (回転したら初期配置に戻す): 今の「回転しても倍率と位置を保つ」挙動が変わるので採らない (利用者への相談が要る案)。
- 案 B (回転の変化を検出して、その frame だけ制限し直す): r1〜r3 で採っていたが、**採らない**。
  回転を比べる元の記録が要り、その記録の扱いを、見開きの片側が未準備・暫定素材・素材の fallback・相方の回転で単ページ化・項目一覧の替わり・全画面の開閉の
  組み合わせごとに決める必要があった。独立レビューのたびに新しい組み合わせの指摘が出た (§7 の r1〜r3)。CLAUDE.md の「設計の簡素化: 状態の組み合わせを減らす」に当たる。
- **案 C を採る (記録を持たない)**: 回転の変化を検出するのをやめ、条件を満たす frame では**毎回**、その frame に描く形に対してパンを範囲に収めてから描く。
  §1.806・§1.810 の共通の根「制限が入力のときにしか効かない」を直接なくす形で、回転の書き手・回転の記録・見開きの片側・相方の脱落を区別しなくてよい。
  範囲内のパンは 1 px も動かさないので、倍率と位置を保つ今の挙動は範囲内では変わらない。
- 「次の frame で直して再描画」にはしない。1 frame だけ画面外の絵が出るうえ、再描画の要求を足すことになる。
  同じ frame で直す前例として、§1.726 の初期配置 (`resolve_fs_initial_placement`) が、描画中に解決した矩形からパンを決めてその frame をそのパンで描き、`fs_pan` に書き戻している。同じ位置・同じ形にそろえる。

### 3.3 収める frame の条件

次の全部を満たす frame でだけ収める。満たさない frame ではパンを読むだけで変えない。

- 通常のページ単位表示 (`Normal` と `SingletonSpread` の配置、倍率の持ち主が画像)。Z ズーム・連結読み・360・比較・編集モードは、それぞれの制約を持つので対象外。
- 描画の元が今の項目 (`layout_source` が今の項目) で、その素材の候補が今の項目の素材 (`selected_current`)。holdover を描く frame では収めない。
- **表示単位の全ページが完成素材 (`FullscreenPageMaterialKind::Complete`) で描かれる** frame。描画 resolver が実際に選んだ型付きの素材の種類で判定する (§1.726 の確定と同じ判定)。
  暫定素材 (サムネイル・通過 rendition・カラー化待ちの代役など) は完成素材と形が違い得るので (§1.726 設計 §10)、その形で収めると完成素材が届いた後に位置を失う。
  見開きの片側が未準備の部分描画も、全ページが完成素材ではないので収めない。暫定・部分描画の frame は今までどおりのパンで描き、完成素材の最初の frame で収める。
- 初期配置 (§1.726) がその frame で適用されるときは、**初期配置のパンを求めてから、その結果に描画の規則を当てる** (同じ frame、この順)。
  初期配置は、はみ出す軸だけ表示単位を読書方向の端に合わせ、ほかの軸は元のパン (`origin_pan`、半ページ送りでは前のページのパン) を保つ (`reading_start_pan`)。
  端に合わせた軸は描画の規則を満たすので動かない。保った軸は、新しい形でも範囲内なら動かず、範囲外なら描画の規則で範囲内へ戻る。
  最終の `fs_pan`・layout・`FsDrawnUnitStamp` は、どちらの場合も実際に描いたもの (描画の規則を当てた後) と一致させる。
  自動の初期配置の印 (`fs_automatic_initial_view`) は実際のパンの写しではなく「自動の位置」との比較値なので、既存の `request.origin_automatic` の区別を保つ:
  - 未操作が元 (`origin_automatic` が真): 描画の規則を当てた後の実際のパンを印にする (動かしていない利用者のクリック送りを保つ)。
  - 手動が元 (偽): 今までどおり、保った手動の成分を取り込まない自動の位置 (`reading_start_pan` を元のパン 0 で求めたもの) を印にする。
    描画の規則で補正しても、補正が要らなくても、手動の扱い (クリック送りの停止) は残る
    (既存のテスト `canvas_click.rs` の半ページ送り: 手動パン `(25, 40)` → 実際のパン `(25, 200)`・印 `(0, 200)` を、描画の規則を当てた後まで延ばして確かめる)。
- **一時表示ではない**こと。通常の `fs_pan` を共用したまま別の形を一時的に描く表示では収めない。右 Ctrl の元画像の一時表示 (`original_preview_active`) がこれに当たる:
  元画像の素材も完成素材だが、AI 拡大後の画像と寸法が違うので、原寸表示などでは形が変わる (例: viewport 幅 1000・元画像 400・AI 後 1600・原寸・`pan.x = 1000`)。
  そこで収めると、キーを離して加工後の画像へ戻ったときに元のパンへ戻らない。実装担当は、`fs_pan` を共用する一時表示をほかにも列挙し、同じ条件に入れて §6 に書く。
  紙めくりの保持素材への束縛（page-turnのheld binding、OFFのCompletingを含む）も一時表示とする。
  既存の最終束縛が使う**補正前のtransform**で同じ照合を行い、保持中のページが選ばれるならパンを提案も確定もしない。
  見開きは片側でも保持素材に束縛されるなら表示単位全体を一時表示とする。保持が解けた最初の通常の完成素材のframeで収める。
  stateを一括で除外せず、このframeで実際に保持素材へ束縛されるかで決める（§6.7の裁定①）。

### 3.4 収める計算と、描画の中での位置

- 規則: 描画で収める規則は、**パンを作るどの経路の結果とも衝突しない**ものにする。描画の規則が作り手の規則より厳しいと、作り手が置いたパンを次の frame で動かしてしまう。
  - 作り手の規則: 入力の制限は**実ページ**に「少なくとも 1 ページの 48px を画面内に残す」を求める (`flat_navigator_main_pages` と `pan_correction_for_minimum_overlap`)。
    初期配置は、**白ページを含む表示単位**を読書方向の端に合わせる (`reading_start_pan`)。白ページ付きの単独ページでは、初期配置の後に実ページが 48px より少なくしか見えないことがある
    (例: viewport 幅 600・実ページ幅 570・間隔 12・LTR・`RightWhite` で、実ページは 18px だけ見える)。これは出荷済みの初期配置の位置で、今回変えない。
  - 描画の規則: 補正の**候補**を次の 2 種類つくり、補正量が最小のものを採る (どれかの候補で補正量が 0 なら動かさない)。
    - 実ページごとの候補: 入力の制限と同じ (各実ページについて、各軸で `min(48px, ページの寸法, viewport の寸法)` を画面内に残す)。
    - 表示単位の候補: **白ページを含む表示単位**の矩形 (実ページと白ページの矩形の和、見開きは 2 ページと間隔を含む) について、各軸で `min(48px, 表示単位の寸法, viewport の寸法)` を画面内に残す。
    入力の制限を満たすパンは実ページの候補で補正量が 0、初期配置で端に合わせた軸は表示単位の候補で補正量が 0 になるので、どちらの作り手のパンも描画で動かない。
    1 つの規則 (表示単位だけ) にすると、実ページが 48px より小さいとき (倍率の下限 0.1 まで縮めた場合など) に入力の制限の結果を動かしてしまう
    (r6 の例: viewport の横 `[0, 600]`・右ページ `[0, 20]`・左ページ `[-32, -12]`・間隔 12 で、右ページは全体が見えて入力では補正 0 だが、表示単位の和 `[-32, 20]` の幅 52 に 48px を求めると 28px 動く)。
    真っ黒 (どの実ページも表示単位も画面に残らない) は防げる。白ページだけが見える位置は、初期配置が既に作る位置なので許す。
  - 入力の制限 (`clamp_fullscreen_pan_from_layout`) の規則は変えない。矩形と viewport から補正量を出す計算 (`pan_correction_for_minimum_overlap`) と
    「候補の中から補正量が最小のものを採る」部分は共有し、描画側は候補に表示単位の矩形を 1 つ足す点だけが違う、という形にして、規則を 2 か所に書き分けない。
- 位置は初期配置と同じ 2 段にそろえる (`fs_initial_placement_pan` = 提案、`resolve_fs_initial_placement` = 素材の準備後の確定):
  - 提案は、**既存の経路が最終的に選んだ素材が、そのページ枠へ実際に描く transform**から作る。
    保持素材への最終束縛を行う経路は、補正前のtransform・painter・素材・occurrence・crop・live条件で先に同じ束縛照合を行う。
    保持を選ぶ場合は一時表示として提案しない。選ばない場合だけ補正し、そのtransformで以降の既存のprepare／束縛を進める。
    補正が照合を不成立にして保持を外す順序にはしない。
    素材の候補とlayoutの基準は別である。初期配置要求が無い経路では、既存のページ枠・間隔の整列・layout基準を保ち、
    パン制限のために初期配置用の素材選択ループを広げない。初期配置要求がある場合だけ、既存どおり素材選択後のlayout再計算を行う。
    全ページが完成素材という§3.3の条件を満たさないframe（サムネイルへのfallbackを含む）では提案を作らない。
    補正量が0でなければ、同じlayout基準を補正後のパンで解き直す。完成素材どうしの交換がある場合は不採用候補の提案を捨てる。
    補正後のNative再利用が拒否されて暫定素材／部分描画になった場合も、表示単位全体の提案を捨て、選ばれたfallbackを元の枠とパンで描く。
  - 確定: 実際に描く素材が決まり、§3.3 の条件を満たすと分かった後で、補正量が 0 でなければ `fs_pan` を書き戻す。見開きは左右の素材を選び終えた後に一組で 1 回。
- 収まりの安定 (毎 frame 動かないこと): 収めた後のパンで解決し直した矩形が、物理 px への寄せ (snap) のせいで再び範囲をわずかに外れ、次の frame でまた動く、ということが起きないこと。
  計算に使う矩形と規則の選び方 (寄せる前の矩形を使う、寄せの誤差の分だけ許す、など) は実装担当が決め、同じ入力で 2 frame 続けて描いても 2 回目の補正量が 0 になることをテストで確かめる。
  丸め誤差として補正0を許す場合も、表示単位または実ページ候補のどれかがviewportと正の面積で交差することを保証する。辺の接触だけでは可視と扱わない（§6.8の裁定）。
- `fullscreen_page_layout` に記録する矩形・パン (`zoom_pan`)・§1.474 の描いた表示単位の印 (`FsDrawnUnitStamp`) は、収めた後に実際に描いたものと一致すること。
- `fs_pan` の書き戻しは「利用者入力」ではないので、`set_fs_pan_from_input` は通さない (初期配置の要求を消す副作用を持ち込まない)。
- 自動の初期配置との関係 (`fs_automatic_initial_view`、クリック送りの可否 `fs_view_has_moved`):
  - 収める前のパンが自動の初期配置と一致していた (= 利用者は動かしていない) なら、収めた後の (倍率, パン) を新しい自動の初期配置として記録し直す。クリック送りは使えたままにする。
  - 収める前が既に手動の変形だったなら、自動の初期配置の記録は変えない (手動のまま)。
- 費用: 各 frame で定数個の矩形の演算 (解決済みの 1〜2 ページと白ページの矩形の和と viewport の比較)。補正が必要な frame では transform を解決し直す。
  補正前の保持束縛の照合も既存の常駐情報だけを読み、素材の生成・I/O・stateの更新は行わない。
  静止していれば補正が要るのは範囲外になった最初の 1 frame だけだが、連続のリサイズ (形が毎 frame 変わる) やドラッグ (開始位置から毎回パンを作り直す) では複数の frame で要り得る。
  新しい同期 I/O・新しい状態・再描画の要求は足さない。viewer context ごとのフィールドも増えない (`fs_pan`・`fs_automatic_initial_view`・`fullscreen_page_layout` は既に swap される)。

### 3.5 毎回収めることで変わる既存の動き (確かめた範囲)

範囲内のパンは動かないので、変わるのは「入力を伴わずに描く形が変わって、パンが範囲外になった」場合だけ。その場合は全部、今は絵が (ほぼ) 画面外に出る。

- 回転 (R / L・回転メニュー・別ウィンドウからの回転・一括回転、相方の回転で見開きが単ページになる場合を含む): 範囲内へ戻る。今回直す対象 (§1.810)。
- ウィンドウ (F12 の別ウィンドウなど) の寸法の変化、上下のバーの固定表示の切り替えで描く範囲が変わる場合: 範囲外になったときだけ範囲内へ戻る。
  縮めてから広げ直しても、元のパンへは戻らない (戻すには元のパンを覚える状態が要るので持たない)。範囲の端 48px を超えて外へ出ていた場合に限られ、通常の使い方の不便には当たらないと判断する。
- 自動トリムの結果の到着、暫定素材から完成素材への切り替え: 完成素材の最初の frame で、範囲外なら範囲内へ戻る。
- ドラッグ・ホイールズーム・ピンチ・§1.474 のページ内スクロール: どれも入力の制限 (実ページの候補) へ合流し、描画の規則はその候補を含むので、描画で動くのは投影と実際の描画の差 (§1.806 を直した後は丸めの差) の分だけ。
  ドラッグは開始時のパンと指の移動量から毎回パンを作るので、描画で動いた分は次の入力で上書きされ、ずれは溜まらない。
- 手動ズームの frame (Ctrl+ホイール等): 入力が投影で制限し、同じ frame の描画が実際の形で確かめる。§1.474 のページ内スクロールの「同じ frame の倍率変更は何もしない」(§1.474 設計 §6.8) は変えない。
- ページ送り・半ページ送り: ページを替えたときの既存の初期化・初期配置がそのまま先に効く。初期配置で保つ軸が新しいページで範囲外なら、同じ frame で描画の規則が範囲内へ戻す (§3.3、完成素材の frame)。
- 別の context: 各 context が自分の描画で自分の `fs_pan` だけを収める。ほかの context の状態には触れない。
- 右 Ctrl の元画像の一時表示: 収めない (§3.3)。押している間、範囲外の形なら今までどおり画面外に出ることがあるが、離すと元のパンで加工後の画像に戻る。
- 紙めくりの保持素材を描くframe: 補正前の照合結果で一時表示と判定し、収めない。保持素材を使わない通常の完成素材に戻った最初のframeで、範囲外なら収める。
- 白ページ付きの単独ページの初期配置: 描画の規則が表示単位で見るので動かない (§3.4)。
- 実装担当は、パンを書く経路・描く形を変える経路を列挙し、上の一覧に無いものがあれば §6 に足す (変わる動きがあれば、その内容と、通常の使い方の不便に当たるかの判断材料)。

## 4. §1.807 前面メニューが開いているときの背後のキー入力

### 4.1 現象 (コード、およびサブ PC 担当の観測)

- ROOT 経由: `handle_fullscreen_root_key_input` は ROOT の `ctx` を共通の全画面キー handler に渡す。egui の Popup の記憶は viewport ごとなので、
  handler の `fs_still_seek_strip_popup_open(ctx)` の早期 return と固定の矢印 (`consume_arrow`) は、全画面ウィンドウ側で開いているポップアップを見ない。
- 同じウィンドウ: 固定の矢印は、同じ viewport の回転メニュー (マウスで開くとフォーカスが無い) も見ない。
  サブ PC 担当の観測 (§1.474 の確認 8-4): 上下キーを「ページ内スクロール」にして回転メニューを開いたまま ↓ を押すと、背後の画像がスクロールし、メニューは開いたまま。
- §1.474 の新しい操作 (J / K・4 方向) は、配送元と配送先の両方の viewport を照会する `fs_page_scroll_front_menu_presented` で止まる (サブ PC 担当の観測 8-2)。

### 4.2 決定

- 「前面メニューが開いているか」の問いは 1 つの helper に寄せる。配送元 (`ctx` の viewport) と、全画面の配送先 viewport の両方を照会する。
  §1.474 の `fs_page_scroll_front_menu_presented` をこの helper にし (名前は用途に合わせて変えてよい)、新しい操作もこれまでどおりそれを使う。
- 判定 (どのメニューが前面か) と、遮断の方針 (何を止めるか) は分ける:
  - シークのポップアップが**配送元の viewport で**開いているときの既存の早期 return (handler 全体から戻り、イベントをポップアップに残す) は、そのまま保つ。
    native のマウス戻る/進むの回数の取り出しは今の位置 (早期 return の前) のまま。
  - それ以外 (配送元・配送先のどちらかで前面メニューが開いている、配送先のシークのポップアップを含む) は、handler 全体からは戻らない。
    **背後の表示を動かすナビゲーションの消費者だけを、消費の前で止める** (受け付けず、消費もしない)。
    対象は、固定の矢印 (↑↓←→ と Shift 付き)、keymap のページ送り (`FsPageNext` / `FsPagePrev`・固定ジャンプなど、利用者が割り当てを変えたキーを含む)、§1.474 のページ内スクロール。
    実装担当は handler の中で背後の表示 (ページ・パン・スクロール) を動かす消費者を列挙し、同じ判定に通す。列挙した一覧と、対象外にしたもの (理由付き) を設計書 §6 に残す。
- 変えないもの: 回転 (R / L) など、背後の表示のページ・パン・スクロールを動かさないショートカット (後段の keymap の消費のまま)。ダイアログ・IME・テキスト入力の既存の判定。
- Esc・全画面の終了・一覧への帰還 (`consume_still_reader_close_key`・`FsClose`・`FsBackToList`) は**今回変えない**。
  今は前面メニュー中でも Esc が全画面の終了まで進み得る (コード読み、未確認)。メニューだけを閉じる形に変えるのは、ナビゲーションの遮断とは別の挙動の変更なので、範囲に入れずバックログ §1.807 に候補として記録する。
- 止めたイベントの行き先:
  - 同じ viewport のメニュー: 止めたイベントは egui のイベントに残る。
  - ROOT 経由: ROOT に残ったイベントは配送先 viewport のメニューへは転送されない。ROOT の handler が `handled = true` を返す既存の扱いを保ち、
    不消費のイベントがグリッドの handler など ROOT の別の経路で背後の操作を起こさないこと (`src/app/update_phases.rs` の抑止)。
- 既定の設定での挙動の変化: 回転メニューなどをマウスで開いたまま ↓ / PageDown などを押すと、今は背後でページが送られる見込み (コード読み、未確認。
  `FsPageNext` 等は回転ポップアップを直接は見ず、フォーカスによる間接の遮断だけ) だが、修正後は送られない。
  J / K とシークのポップアップは既にそう動くので、それにそろえる修正として扱う (統括からの指示で、ROOT 経由と同じウィンドウの回転メニューの両方を対象にする、2026-11-18)。

### 4.3 テスト

- §1.474 の回帰テスト (`root_dispatched_scroll_keys_leave_the_destination_rotation_popup_unconsumed`) と同じ形の handler テストで:
  - ROOT に ↓ / PageDown / 利用者が割り当てたページ送りのキーを配送し、配送先の全画面 viewport でフォーカスの無い回転ポップアップ・シークのポップアップが開いているとき、
    ページが送られず、イベントが消費されず、グリッドの操作も起きないこと。閉じた後は送られること。
  - 同じ viewport で回転ポップアップが開いているとき、↓ (既定のページ送り・ページ内スクロールの両設定)・PageDown が背後を動かさないこと。
  - 回転ポップアップが開いているときも R / L が今までどおり効くこと。配送元のシークのポップアップ中の既存の早期 return が変わらないこと。
  - メニューが無いときの既定の挙動 (↑↓←→・PageDown・Shift+↑↓・スタックジャンプ) が変わらないこと。
- §1.810 のテスト: 単ページの回転で範囲外になるパンが同じ frame で収まる (描いた矩形・`zoom_pan`・`FsDrawnUnitStamp` が収めた後)、範囲内のパンは不変、
  相方だけの回転で見開きが単ページになる例 (高さフィット・viewport 1000×600・各ページの幅 400・間隔 0・`pan.x = 850`)、
  自動の初期配置 → 回転 → クリック送りが使えること、手動パン → 回転 → 手動のまま、holdover の frame・暫定素材の frame・見開きの部分描画の frame では収めず、完成素材の最初の frame で収めること、
  素材の fallback (不採用の候補の提案を捨てる)、白ページ付きの単独ページ、初期配置と同じ frame での優先順位、同じ入力で 2 frame 続けて描いたとき 2 回目の補正量が 0、
  LTR / RTL の白ページ付きの単独ページで、初期配置の後に完成素材の frame を複数描いてもパンが変わらないこと (例: viewport 幅 600・実ページ幅 570・間隔 12)、
  48px より小さい実ページ・左右の寸法違い・白ページ付き配置で「入力 (ドラッグ・ホイールズーム・ピンチ・ページ内スクロール) → 完成素材の描画」でパンが変わらないこと (r6 の例を含む)、
  半ページ送りの初期配置で保つ軸が新しい形で範囲内 / 範囲外の両方 (最終の `fs_pan`・layout・`FsDrawnUnitStamp` は実際の描画と一致し、自動の初期配置の印は §3.3 の由来ごとの値)、
  通常・見開きで「加工後の完成素材 → 右 Ctrl の元画像 → 離す」でパンが変わらないこと、端でのドラッグ (入力 → 描画の繰り返し) と端数 DPI (1.25 / 1.5) で微動しないこと、
  別の context (F12) の `fs_pan` が変わらないこと、Z ズーム・連結読みでは収めないこと。
- テストで、キーの状態を実機から読む形 (偽の入力を消した後の判定) を新しく書かない (§1.814 (a) と同じ落とし穴)。

## 5. 守ること (3 件共通)

- 症状パッチ (guard・delay・retry・追加 repaint・一括 reset・silent fallback) を入れない。§1.810 は「制限が入力のときにしか効かない」こと自体をなくす構造の修正 (描く frame で毎回収める) で、遅延・再描画・記録を使わない。
- detached viewer: 触れるのは context ごとに swap される `fs_pan`・`fs_automatic_initial_view`・`fullscreen_page_layout` の読み書き (新しいフィールドは足さない)、および配送先 viewport の Popup の照会だけ。detached の述語・viewport の経路は変えない。
  変えた場合は CLAUDE.md の「Detached viewer リワーク中のルール」に従い、双方のレビューで「症状パッチではなく構造的修正」と合意し、[detached-rework-plan.md](detached-rework-plan.md) に記録する。
- UI スレッドに新しい同期 I/O を足さない。描画中の追加計算は、解決済みの矩形の比較と平行移動だけにする。
- 既定設定で変わるのは §4.2 の「前面メニュー中のナビゲーション」と、§3.5 の「範囲外になったパンが範囲内へ戻る」ことだけ。パンの制限は範囲内のパンを動かさない。

## 6. 実装記録

### 6.1 実装と所有境界

§6.6の裁定①（WIP基点 `6e39c705e`）でCenter fallbackの幾何を維持し、
§6.7の裁定①（再開基点 `ff2abdf5a`）で保持素材への最終束縛を一時表示として扱う。以下は実装内容。
実装・指定の対象検証を完了した。未解決の設計判断は無く、コミットは行わない。
新しいviewer context field、回転の記録、同期I/O、待機・retry・repaintは追加しない。

- `src/displayed_image_transform.rs`: 描画済み `DisplayedPage` に通常見開きの固定間隔と画面上の側を記録する。
  `begin`でページとともに退役し、既存のlayoutと一緒にcontext間を移る。
- `src/ui_fullscreen.rs`: layout producerが最終の物理px間隔を記録し、入力投影は記録だけを読む。
  単独ページと通常見開きは `spread_projected_gap_correction` を共用する。
  通常描画は初期配置→素材の準備→パン制限→transformの再解決を行い、最終的に準備できた完成素材だけを確定する。
  見開きの素材選択ループは初期配置要求がある場合に限定する。通常経路は既存の準備後の左右完成素材から1組で提案し、
  元のlayout基準を補正後のパンで解く。初期配置要求ありの経路も準備後の完成素材から提案する。
  補正後のNative拒否・fallback・部分描画では表示単位全体の提案を捨てる。要求なしでは元の枠・間隔・パンを保ち、
  要求ありでは既存の素材選択後のlayout再計算だけを使う。完成素材間の交換は、紙めくりの保持素材の再利用／通常素材への準備で起こり得る。
  どちらも準備後の素材種別で判定し、不採用の変形をlayout・panへ発行しない。
- `src/ui_fullscreen/pan_bounds.rs`: 描画の適用領域、候補の比較、確定を集約する。
  `minimum_pan_correction` と `pan_correction_for_minimum_overlap` を入力と共用し、描画だけ表示単位の候補を足す。
- `page_turn.rs`: 最終束縛の照合を読み取り専用の `page_turn_prepared_held_page` に抽出し、既存の `page_turn_bind_prepared` と描画側の補正前照合で共用する。
  occurrence・crop、Native拒否後の別素材を復活させない条件まで同じhelperに含め、述語と束縛結果は変えない。
  通常描画はprepare済み候補を先に組み立てて照合し、見開きは同じ補正前の左右候補を照合する。
  実際に束縛を行う経路（通常のsubmittedあり／見開きのsubmit_final）だけで照合し、保持を選べば表示単位の提案を作らない。
- `initial_placement.rs`: 自動配置の比較値の記録入口を共用する。初期配置要求の `origin_automatic` を確定前に捕捉し、
  手動由来なら既存の自動配置値を保持する。パンの書戻しは入力setterを通さない。
- `in_page_scroll.rs`: 前面メニュー照会を `fs_navigation_front_menu_presented` へ改名し、既存の配送元／配送先照会を共用する。
  detachedの述語・viewportの選び方・ROOT配送は変更しない。
- テスト: `pan_bounds/{tests,projection_tests}.rs`、`front_popup_input_tests.rs`、`in_page_scroll/{handler_tests,review_regression_tests}.rs`。
  既存 `initial_placement/tests.rs`、`page_turn/{resident_placement_tests,wait_indicator_tests}.rs` は元画像一時表示の明示引数を追加。
  `page_turn/prepare_boundary_tests/pan_bounds_tests.rs` に保持と通常描画の所有境界の4件を追加し、既存prepare境界fixtureから登録する。
- 文書: 本書、`display-pipeline.md`、`keymap-spec.md`、`key-customization-impl-plan.md`、`detached-rework-plan.md` §11。
  `fullscreen-page-start-position-plan.md` と `fullscreen-in-page-scroll-plan.md` は後続仕様への参照を追加し、`next-release-backlog.md` の3項目を更新する。
  マニュアル `fullscreen.html` は前面メニュー中のキーと範囲外の表示位置の説明を更新する。
  §6.7の保持中の一時表示の裁定では追加の利用者向け説明は不要とし、この2箇所の更新を維持する。

§2.2で任された記録先は `DisplayedPage`、§3.4で任された安定方法は次項の丸め残差の許容とした。
§6.6の裁定に合わせてCenter fallbackのlayout基準を維持する。素材の候補とlayout基準を別に扱う点は§3.4にも明記した。

### 6.2 収まりの安定

候補は最終transform（サンプラの縦横比のcontain、位置と寸法の整列、左右のgap整列を通したもの）の
clip前AABBから求める。補正後にtransformを再解決する際、layoutの原点・samplerの原点・左右一体のgap整列の
丸めが変わるため、描画側だけ各軸の残差が2物理px以下で、かつ**表示単位の矩形とviewportが正の面積で交差する場合だけ**補正0とする。
表示単位の矩形はすべての実ページの候補を含むため、実ページのどれかが正の面積で交差する場合もこの条件を満たす。
辺の接触や完全な画面外表示では、2物理px以下でも候補の補正を全量適用する。許容を超える場合も従来どおり全量適用する。
入力側の48px規則は緩めない。もともと補正0の候補は常に0のままで、範囲内のパンを動かさない。
同じ入力の連続描画、端ドラッグの入力→描画、DPI 1 / 1.25 / 1.5で再補正が起こらないことを検査する。
1〜2物理pxのページは、90°回転後の実際のpaint矩形とviewportの交差を独立に計算し、そのframeから正の交差があること、次frameでpanと補正0が安定することを検査する（§6.8）。

### 6.3 パンの書き手・形を変える経路・一時表示の棚卸し

- 書き手: `set_fs_pan_from_input`（通常ドラッグ、Spaceパン、キー拡大、Ctrl/中ボタン/左右ドラッグのズーム、
  ピンチ、ナビゲータのドラッグ／範囲／中央合わせ、ページ内スクロール）、ホイールの提案とクリック予測、
  初期配置の確定、open/close・fit/reset・構成変更の初期化、detachedの保存済み変形の復元、context bundleのswapと駐車中reset。
  連結読みの直交軸ドラッグは `fs_pan` へ直接加算するが、描画制限の対象外。
- 形の変更: 保存／自由回転（相方の脱落を含む）、ズーム・fit方式と倍率制限・指定スケール、表示トリム／自動トリム到着、
  分割slice、単ページ／見開き・表紙／補助表紙・片側／白ページ・1ページ送り／位置ずらし・読書方向・間隔、
  素材の到着／交換（RAW・PDF・AI・final合成・通過rendition・fallback）、viewportのresize/DPI・固定HUD/パネルの予約領域。
  fit/resetやopen等は既存の初期化が先に効く。それ以外も完成素材のframeで範囲外だけを戻す。§3.5以外の新しい操作制限はない。
- 一時表示: 右Ctrl元画像は通常の `fs_pan` を共用するため明示的に除外する。編集ツールの元画像／マスク表示は編集モードの除外、
  比較（通常／pin／wipe／diff）は比較の除外、分析・Z・360・3Dは各表示ownerの除外で書き戻さない。
  類似候補の長押しpreview、Checking中の保持画面、navigation previousとsource-reload holdoverは別の描画／Captured／非currentなので書かない。
  紙めくりの保持（OFFのCompletingを含む）は補正前の実際の束縛照合で一時表示と判定する。
  見開きは片側だけの保持も表示単位全体で除外し、保持が解けた通常の完成素材で収める。state全体の除外や新しい一時panは作らない。
- context: 確定はmountされたcontextの既存の `fs_pan`・自動配置値だけ。兄弟contextのキャッシュ・要求・panを変更しない。

### 6.4 前面メニュー中に止める消費者と対象外

`fs_navigation_front_menu_presented` の同じ判定を、次の消費の前に置く。

- 固定plain/Shiftの4方向矢印（ページ内スクロールへの置換も含む）。
- `FsPageNext/Prev`、`FsFixedJumpNext/Prev` と `NoRtl`、`FsJumpFirst/Last`。
- `FsSpreadShiftLeft/Right/Prev/Next`、`FsSpreadStepPrev/Next`、`FsStackJumpNext/Prev`。
- `FsContinuousScrollForward/Back`、`FsPageScrollUp/Down/Left/Right`、`FsPageNScrollNext/Prev`。
- `FsCtrlNavNext/Prev`、`FsListOrderNext/Prev`、`FsSiblingNext/Prev`。同じhandlerのmouse-back/forward判定と
  `VideoPrevFile/NextFile` のファイル移動も同じナビゲーション受付に従う（nativeの回数drainは早期return前を維持）。

対象外: `consume_still_reader_close_key`・`FsClose`・`FsBackToList`は設計の明示除外。
R/L、fit/構成/方向の設定キー、ズーム・表示owner切替、スライドショーの再生停止、評価・チェック・補正・コピー等は
ページ／パン／スクロールのナビゲーション要求を消費する入口ではないので、既存のkeymap/所有規則を保つ。
編集モード・ページ一覧・音声シークは既存の専用handler/早期returnの責務を維持し、native presenterは変更しない。
overflowの独自パネルは従来の「キーで閉じてから通常操作」の規則を保つ（Slideshowのfocus中の早期returnも維持）。
配送元シークPopupの早期returnはそのまま。配送先シークPopupはナビゲーションだけを拒否する。
ROOTは従来の `handled = true` を返し、残ったキーを背面一覧へ配送しない。

### 6.5 検証

対象: pan_bounds・initial_placement・in_page_scroll・front_popup_input_tests・overflow_input_tests、
navigator_pan_clamp、page_turnのresident_placement_tests・wait_indicator_tests・prepare_boundary_tests（追加4件を含む）・nonfinal_tests。

- 初回 `cargo check -p mimageviewer --bin mimageviewer-core`: 成功、1分49秒。最新の画像項目の適用領域確認を加える前の差分。
- 初回の対象nextest: コンパイル失敗（E0502、追加テストのAppTestEnv経由の借用2件）、実行0件。ローカル変数へ取り出して修正した。
- 再実行は共有runnerの全件テスト枠の終了待ちとなった。§6.6の停止判断後、自分の待機プロセスだけを中止した。Cargo・テストは未開始、実行0件。他担当の全件テストには触れていない。
- 裁定①反映後の対象nextest（r3）: 267件中264件成功・3件失敗。追加fixtureの最大テクスチャ寸法（2048超過）と、consumerのみの連続passでPopup Areaを描かないための退役、既存テストの旧仕様（メニュー中も矢印を消費）を修正した。
- r4 `cargo check -p mimageviewer --bin mimageviewer-core`: 成功、31.43秒。
- r4対象nextest: コンパイル中に `FsFixedJumpNext/Prev` の共通判定の適用漏れを発見し、自分の実行だけを中止（実行0件）。両消費者を修正し、remapした前後両方向の不消費を検査する。
- r5対象nextest: 268件中267件成功・1件失敗（3分18秒コンパイル、90.398秒実行）。通常／見開きの確定境界、Center Native→thumbnailの元枠維持、高さのみ指定したスケールの投影、初期配置・前面メニュー・紙めくりの関連回帰は成功。
  残る元画像一時表示テストは、左右で同じテクスチャを使うfixtureが項目識別チェックに拒否されたため、項目ごとに別テクスチャへ修正した。製品コードは変更しない。
- r6: 上の1件だけを再検証し、1件成功・失敗0（5分26秒コンパイル、2.476秒実行）。nextestはテストごとに別プロセスなので、影響しない267件のr5成功結果を再利用する。対象268件の成功を確認した。
- r6最終 `cargo check -p mimageviewer --bin mimageviewer-core`: 他担当のfull lock待ちのまま、§6.7の停止時に自分の待機だけを中止した。Cargo未開始。ロックと他担当のプロセスには触れていない。
- `ff2abdf5a`からの再開（§6.7裁定①）:
  - r7の初回は他担当のfull lock待ちでCargo未開始。fixtureを整理して整形をそろえるため、自分の待機だけを中止して再投入した（実行0件）。
  - r7b境界4件: **4件成功・失敗0**、exit 0。8分02秒コンパイル、1.568秒実行。full lockが `opus-811-libfull` から `rar803` へ続いたため開始待ちとなり、02:35:48に低優先度・6並列のsandbox fallbackで開始した。
    完成したresampleable入力があっても最後の束縛で暫定Managed保持素材を描くこと、保持中のpan不変、補正前／後で照合結果が変わること、保持解除の最初の完成frame、左右どちらかだけの保持、Completingでも照合不成立なら通常補正することを確認した。
  - r8関連回帰: **297件成功・失敗0**、exit 0。コンパイル確認1.82秒、実行62.397秒。従来の対象に `page_turn::prepare_boundary_tests`・`page_turn::nonfinal_tests` を加え、追加4件も同じ差分で再確認した。
  - r9最終core check: **成功**、exit 0、37.36秒。`depth-771` のfull lock（02:46開始）を待ち、03:09:33に指定runnerのsandbox fallbackで開始した。ソースとfixtureはr7b・r8の成功から変更していない。
- 最終の引数なし `cargo fmt` と `git diff --check`: **成功**。再開時に変更したRustの4ファイルは整形前後のSHA-256が一致し、r7b・r8・r9の検証対象ソースを維持している。
- 指定runner `run-heavy.ps1 -Owner codex-806 -WorkDir G:\home\mimageviewer-474 -Command ...` を使用。

実行コマンド（check/nextestは上記runner経由、通常feature）:

```text
cargo check -p mimageviewer --bin mimageviewer-core
cargo nextest run -p mimageviewer --lib -E 'test(/ui_fullscreen::pan_bounds/) | test(/ui_fullscreen::initial_placement/) | test(/ui_fullscreen::in_page_scroll/) | test(/ui_fullscreen::front_popup_input_tests/) | test(/ui_fullscreen::overflow_input_tests/) | test(/pan_clamp/) | test(/page_material_kind_survives_prepare/) | test(/page_turn::resident_placement_tests/) | test(/page_turn::wait_indicator_tests/)'
cargo nextest run -p mimageviewer --lib -E 'test(/original_preview_round_trip_keeps_processed_pan_for_single_and_spread/)'
cargo nextest run -p mimageviewer --lib -E 'test(/page_turn::prepare_boundary_tests::pan_bounds_tests/)'
cargo nextest run -p mimageviewer --lib -E 'test(/ui_fullscreen::pan_bounds/) | test(/ui_fullscreen::initial_placement/) | test(/ui_fullscreen::in_page_scroll/) | test(/ui_fullscreen::front_popup_input_tests/) | test(/ui_fullscreen::overflow_input_tests/) | test(/pan_clamp/) | test(/page_material_kind_survives_prepare/) | test(/page_turn::resident_placement_tests/) | test(/page_turn::wait_indicator_tests/) | test(/page_turn::prepare_boundary_tests/) | test(/page_turn::nonfinal_tests/)'
cargo fmt
git diff --check
```

ログ: `target/806-check.{stdout,stderr}.log`、`target/806-tests.{stdout,stderr}.log`、
`target/806-tests-r2.{stdout,stderr}.log`、`target/806-r3-tests.{stdout,stderr}.log`、
`target/806-check-r4.{stdout,stderr}.log`、`target/806-r4-tests.{stdout,stderr}.log`、
`target/806-r5-tests.{stdout,stderr}.log`、`target/806-r6-tests.{stdout,stderr}.log`、
`target/806-check-r6.{stdout,stderr}.log`、`target/806-r7-boundary-tests.{stdout,stderr}.log`、
`target/806-r7b-boundary-tests.{stdout,stderr}.log`、`target/806-r8-related-tests.{stdout,stderr}.log`、
`target/806-r9-final-check.{stdout,stderr}.log`。TEMP/TMPはworktree内の `target/806-tmp` を指定。
初回はsandboxによりrunnerのlock書込みができず低優先度・6並列で実行、再実行は他担当のfull lock待ちになった。
全件gate・build-dev・製品起動・コミットは実施していない。

追加したテスト名（29件）:

<!-- 806-test-names -->
- `rotated_single_is_bounded_in_the_same_frame_with_matching_layout_and_stamp`
- `rotation_keeps_in_range_pan_exactly_and_rebounds_the_rest`
- `partner_rotation_to_single_restricts_pan_without_a_rotation_history`
- `automatic_rotation_correction_preserves_clicks_but_manual_pan_stays_manual`
- `provisional_and_unselected_frames_keep_pan_until_complete_current_material`
- `partial_spread_does_not_restrict_pan_and_first_complete_spread_does`
- `original_preview_round_trip_keeps_processed_pan_for_single_and_spread`
- `white_singleton_reading_start_is_unchanged_on_later_complete_frames_ltr_and_rtl`
- `half_page_initial_placement_bounds_retained_axis_and_preserves_origin_identity`
- `tiny_real_pages_and_white_companion_accept_input_candidates`
- `white_singleton_input_clamp_is_preserved_by_complete_draw_at_small_zoom`
- `repeated_edge_drag_and_fractional_dpi_draws_are_stationary_after_first_correction`
- `z_continuous_comparison_and_editing_do_not_bound_the_shared_pan`
- `rejected_native_discards_pan_proposal_before_provisional_fallback`
- `draw_pan_correction_only_changes_the_mounted_context`
- `center_native_fallback_keeps_the_original_slot_gap_offset_and_pan`
- `spread_gap_projection_matches_final_transforms_and_edge_stop_with_zero_gap_error_budget`
- `width_fit_projection_moves_by_signed_gap_correction_and_reduces_error`
- `singleton_and_two_page_projection_share_the_fixed_gap_formula`
- `asymmetric_and_tiny_spreads_keep_input_clamp_results_on_complete_draw`
- `unfocused_rotation_menu_rejects_default_and_remapped_navigation_before_consumption`
- `rotation_menu_preserves_rotation_shortcuts_and_source_seek_keeps_early_return`
- `root_navigation_is_unconsumed_for_destination_rotation_and_seek_and_resumes_after_close`
- `held_single_matches_before_pan_and_bounds_the_first_released_complete_frame`
- `held_spread_bounds_only_the_first_released_complete_frame`
- `one_held_spread_page_keeps_the_entire_unit_pan`
- `completing_without_matching_held_pages_still_bounds_normal_material`
- `tiny_rotated_pages_remain_visible_and_stationary_after_snap_correction`
- `snap_tolerance_requires_positive_area_and_preserves_visible_residuals`
<!-- /806-test-names -->

既存テストの期待値更新: `fallback_menu_leaves_navigation_arrows_unconsumed_without_moving_or_turning`、
`rotation_popup_leaves_fixed_arrows_unconsumed_before_the_replacement_point`。
初期配置・紙めくりの既存8箇所は、一時表示の明示引数 `false` の追加のみ。
UIの追加配置やラベル変更はなく、操作と最終transformをheadlessの実renderer／handlerで検査する。製品の実機起動は行わない。

### 6.6 実装担当の停止報告と設計担当の裁定①（2026-11-19）

以下は停止時WIP（`6e39c705e`、行番号も停止時）の所見。現在の実装は裁定①に従って解消した。
既定Centerでのfallbackの幾何が、初期配置要求がある場合と違う。

- HEAD `e851d76a2` の `src/ui_fullscreen.rs:51713` 付近は、初期配置要求がある場合だけ
  `select_initial_material` を有効にし、コメントでもCenter/capturedの既存fallback幾何の保持を明記している。
- 既存 `prepare_fs_spread_page`（現差分の `src/ui_fullscreen.rs:52878`）は、Nativeの準備が拒否されても
  引数のページ枠 `rect` とgap用offsetを保ったまま、サムネイルをその枠へcontainして描く。
- 現差分は `select_paged_material`（同:51792）を通常の完成素材のパン制限にも広げたため、
  Native→fallbackで候補を交換してループし直す（同:52271）。その候補からlayout_sizeを取り直す（同:51824）。
  このためCenterでも見開き全体の枠が再計算される。
- 例（コード上の計算、実行観測ではない）: 原寸、左右とも1600×2400、左だけ80×120のthumbnailへfallback。
  従来は左の1600×2400の枠を保ってthumbnailを拡大containするが、現差分では左の枠も80×120になり、
  組の幅と左右両ページの位置が変わり得る。pan=0でも起こるので、範囲外のパンだけが変わるという§5の条件と両立しない。

選択肢:

1. 推奨: Centerの既存のlayout基準・gap整列を保ち、各fallback候補がその枠へ実際に描くtransformに対して提案・確定する。
   初期配置要求がある場合だけ、既存どおり素材選択後のlayout再計算を行う。候補素材とlayout基準の区別を設計に明記する。
2. Centerにも素材選択後のlayout再計算を認める。fallback時のサイズ・相方の位置の変更を許す挙動変更となるため、§5の条件の明示変更が必要。

上記は停止時のコード所見であり、実機再現の記録ではない。設計担当は①を採択し、WIPを `6e39c705e` としてコミットした。
Centerを含め既存の幾何を維持し、通常の完成素材のパン制限のために初期配置用ループを広げない。
最終選択素材の「その枠へ描くtransform」で提案・確定し、暫定素材／部分描画では収めない。
§5の不変条件は変更せず、§3.4をこの裁定に合わせて明確化した。実装担当は同WIP上で再開し、コミットは行わない。

### 6.7 最終の保持素材への束縛についての停止報告と裁定①（2026-11-19）

以下は停止時WIP（`ff2abdf5a`、行番号も停止時）の所見。これに対する裁定と実装はこの節の末尾に記録する。
Center fallbackは修正したが、「素材のprepare後」が最終選択ではない経路があった。
停止時はコード所見で、パン制限との組み合わせを実行で再現した記録ではない。当時の対象268件は成功したが、この組み合わせの回帰テストはまだ無かった。

- 通常描画は `src/ui_fullscreen.rs:45756` からprepare後の素材／transformで補正し、パンと初期配置を確定した後、同 `45842` で `page_turn_bind_prepared` を呼ぶ。
  見開きもprepare後の左右から補正・確定し、同 `52659` の最終束縛が後に残る。
- `src/ui_fullscreen/page_turn.rs:2285` の `page_turn_bind_prepared` は、条件を満たすと `..old.clone()` を返す。
  painter以外の素材・transform・完成／暫定の種類は保持中のものへ置き換わる。
  Nativeは共有prepare境界で再利用を判定するが、Managedの保持素材もこの最後の置換の対象となる。
- 同 `2249` の照合と `page_turn/binding.rs:350` の `same_page_turn_view` はplacementのzoom/pan・回転・viewport・fit方針等を比較する。
  描画寸法や素材の完成／暫定の種類は比較せず、`page_turn.rs:1004` の `same_fit_policy` もLegacy/Rasterではfit modeだけを比較する。
  ラスタの品質更新後も保持素材を維持するのは既存の意図で、`page_turn/nonfinal_tests.rs:735` の回帰テストにも記録されている。
- パン補正でplacementを変えると、同じ保持素材への照合自体が不成立になり得る。したがって「最後にkindを見る」だけでは、
  保持素材を選んだtransformでの提案を、不採用になった後も使う可能性が残る。
  OFFのCompletingも既存の保持対象であり、同 `3227` の `refresh_page_turn_completion_pages` は再投影後のページを保持する。
  animation中のzoom/panの適格性だけでは、この経路の所有規則を説明し切れない。

§3.3・§3.4は最終選択素材の実際のtransformを要求し、裁定①は既存の素材選択・幾何の維持を要求する。
保持素材への束縛とパンを独立に確定できる、という実装上の前提がこの経路では成立しないため、
保持中の表示をどう分類するかを設計担当に裁定してほしい。症状のための除外guardや、通常経路への素材選択loopの拡張は実装していない。

選択肢:

1. 保持中の紙めくり／Completingの表示を一時表示として扱い、その素材を描くframeではパンを書かない。保持が解けた通常の完成素材で収める。
   実際に保持素材を選んだframeの判定位置を明示する。既存stateを一括で除外すると、保持素材を使わない通常描画まで除外し得るので避ける。
2. 保持中の表示も案Cに含め、pan変更に伴う保持素材→通常素材の再選択までを提案・確定の境界に含める。
   不採用素材の提案を捨てる規則と、既存の保持方針を変えてよい範囲を定める。単に最後の束縛を前へ移すだけでは解決しない。

再開後の検証には、既存の `page_turn::prepare_boundary_tests`（Center fallbackと初期配置を含む）・
`page_turn::nonfinal_tests` と、この束縛の前後で完成／暫定・形が変わる境界の回帰を加える。
最終checkは未開始のまま中止し、コミットせずWIPとして引き渡す。

**裁定①（上記停止後の設計担当決定）**: 保持素材を描く表示は§3.3の一時表示とする。
既存の最終束縛に使う補正前のtransformで同じ照合を先に行い、保持を選ぶならパンを提案も確定もしない。
見開きは片側でも保持なら表示単位全体を一時表示とする。stateを一括で除外せず、保持を使わない完成素材のframeでは収める。
補正によって保持→非保持へ照合結果を変えない順序とし、保持解除後の最初の完成素材で収める。
WIPは設計担当が `ff2abdf5a` としてコミットし、実装担当はその上で再開した（コミットしない）。

実装は最終束縛の照合helperを共用し、束縛・state・viewportの経路を維持した。初期配置要求の無い経路へ素材選択loopを広げない。
保持とpanを同時に再選択する案②を採らず、一時表示ではpanを所有しない条件へそろえることで、保持→通常素材の選択と補正を相互依存させない。
境界の4件と `prepare_boundary_tests`・`nonfinal_tests` の成功結果は§6.5に記録した。
裁定①どおりの実装と検証を完了し、この停止事項は解消した。

### 6.8 独立実装レビューround 3のP2: 小さいページと丸め許容（2026-11-19）

別担当Sol / xhighの実装レビューはP1なし・P2 1件でREVISE。設計担当のコミット `aa5763c22` を基点に修正した。
従来の2物理pxの許容は可視部分の有無を問わず補正を捨てたため、原寸・倍率0.1・1000×10・pan=(301,0)で
90°回転後に幅1pxの矩形が[601,602]へ出ても、補正(-2,0)を捨てていた。

- 裁定: 補正0を許す場合でも、表示単位または実ページ候補のどれかとviewportとの正の面積の交差を保証する。
- `src/ui_fullscreen/pan_bounds.rs`: 既に求めた表示単位の矩形とviewportの正の交差を、丸め許容の適用条件へ追加した。
  表示単位は実ページ候補を含むので、候補ごとの別の可視状態は持たない。非交差／辺だけの接触なら補正を全量適用する。
  候補の選択・入力側の制限・保持素材の判定・context／viewportの経路は変更しない。
- `src/ui_fullscreen/pan_bounds/tests.rs`: 上記2件を追加。UIの実resolverを通し、1／2物理px、DPI 1／1.25／1.5、左右・上下の端を検査する。
  回転前が入力制限を満たすこと、回転後の実paint矩形の交差幅・高さが正であること、2frame目以降のpan不変・補正0を確認する。
  可視性は `draw_pan_correction` や `assert_painted_pan` に頼らず、実際のpaint矩形とviewportの辺から独立に計算する。
  正の交差が残る小さい残差は従来どおり0を返し、画面外と辺の接触には全量補正する純粋幾何の検査も行う。
- 文書は本節と§6.2を更新する。利用者向けの説明は既存の「範囲外の表示位置を戻す」と同じため、マニュアルは変更なし。
- 検証: `ui_fullscreen::pan_bounds` を対象にしたnextestは **22件成功・失敗0、exit 0**（コンパイル3分36秒、実行21.586秒）。
  追加2件と、既存の端数DPIのsnap・端ドラッグ・白ページ・投影のテストを同じfilterで確認した。
  最終core checkも **成功・exit 0、21.27秒**。check／nextestはいずれも以下の指定runnerを使用し、同時に2本は流していない。

```text
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner codex-806 -WorkDir G:\home\mimageviewer-474 -Command "<command>"
cargo nextest run -p mimageviewer --lib -E 'test(/ui_fullscreen::pan_bounds/)'
cargo check -p mimageviewer --bin mimageviewer-core
cargo fmt
git diff --check
```

通常feature・test/dev profile。TEMP/TMPはworktree内の `target/806-tmp`、MSBuild node reuseは無効。
共有full lockを待った後、sandbox警告付きの低優先度・6並列で実行した。
ログは `target/806-r4-snap-tests.{stdout,stderr}.log` と `target/806-review-r4-check.{stdout,stderr}.log`。
最終の引数なし `cargo fmt` と `git diff --check` も成功。変更したRustの2ファイルは整形前後のSHA-256が一致し、テスト・checkの対象ソースを維持している。
コミット・lib全件・build-dev・製品起動は行わない。

## 7. 実装前の独立レビュー

- r1 (gpt-6.1-sol / xhigh、2026-11-18、eb512b82e): REVISE。反映した指摘:
  - P1: 相方だけの回転で見開きが単ページになると、残ったページの回転は変わらず検出できない → 比較の元に表示単位から外れたページを含める (§3.3)。
  - P2: 範囲外から戻すと自動の初期配置が手動扱いになりクリック送りが止まる → 補正前が自動なら新しい自動の初期配置として記録し直す (§3.4)。
  - P2: シークの早期 return を共通 helper に置き換えると R / L なども止まる → 判定と遮断の方針を分け、シークの既存の早期 return は保ち、それ以外はナビゲーションの消費者だけを止める。ROOT 経由の Esc の行き先を分けて書いた (§4.2)。
  - P2: 比較の元の保持と確定の描画境界が未定義 → 初期配置と同じ提案・確定の 2 段、`selected_current`、holdover・fallback・暫定素材の扱いを書いた (§3.3・§3.4)。
  - P3: 丸めの許容誤差 → `(1 + r)` 物理 px、最終 transform で比較 (§2.3)。
- r2 (同じ担当、3d78d5860): REVISE。反映した指摘:
  - P2: 比較の元が暫定素材の間に一度も確定しない → 描いた素材の形で確定し、記録の初回と回転後の確かめを同じ確定点にした。暫定 → 完成の形の変化は §3.5 の対象外 (§3.3)。
  - P2: fallback の frame 全体を提案の対象外にすると実際に描く画像を補正できない → 提案は素材の候補ごと、不採用なら捨てる。見開きは一組で確定、片側未準備は記録を進めない (§3.4)。
  - P2: `get_rotation` は同期で DB を読む → 常駐の情報だけ (`peek_rotation` 等)。記録は context ごとに 1 つ (swap)、`items_generation` の所属と、全画面の開閉での初期化 (§3.3)。
  - P2: Esc は前段で消費され全画面の終了まで進む → Esc・`FsClose`・`FsBackToList` は今回変えず、§1.807 に候補として記録する (§4.2)。
  - P3: `(1 + r)` の上限は成り立たない → 間隔 0 の同じ条件の差を基準にし、間隔の項だけが消えることを見る。保証は倍率の比 1→4 の範囲 (§2.3)。
- r3 (同じ担当、9e57510f2): REVISE。反映した指摘:
  - P2: 初回から見開きの部分描画だと比較の元を作れず、相方の回転で単ページ化を検出できない → 部分描画も確定点に含め、「比べる → 制限し直す → 全員の常駐の回転で記録」の順にした (§3.3・§3.4)。
  - P3: 間隔 0 を基準にした比較は、間隔で基準の倍率が変わるフィットでは成り立たない → 高さフィット・倍率指定に限り、幅フィットは差が縮むことだけを見る (§2.3)。
- r4 の依頼後 (325aa2105)、統括の指摘で §1.810 を案 B から案 C へ切り替えた (2026-11-18)。案 B の r1〜r3 の指摘 (記録の扱い) は案 C では対象が無くなった。
  案 C でも残る論点 (素材の候補ごとの提案と確定・自動の初期配置の扱い・暫定素材の形) は、r1〜r3 の結論を §3.3・§3.4 に引き継いだ。
- r4 (同じ担当、325aa2105、案 B に対して): ACCEPT。P3 (§2.3 の条件の文言) を反映。ただし同時に案 C へ切り替えたので、案 C で受け直す。
- r5 (同じ担当、案 C に対して、b3d22834c): REVISE。反映した指摘:
  - P1: 初期配置は白ページを含む表示単位を端に合わせるので、実ページの 48px を求める規則で収めると次の frame で初期配置を動かす → 描画の規則は表示単位 (白ページを含む) の 48px にし、入力の制限・初期配置のどちらの結果とも衝突しない形にした (§3.4)。
  - P1: 右 Ctrl の元画像の一時表示も完成素材で、形が違うので収めるとパンを失う → 一時表示では収めない (§3.3)。ほかの一時表示は実装担当が列挙する。
  - P3: 費用の説明 → 各 frame で定数個の矩形の演算、補正が要る frame では解決し直し、連続のリサイズ・ドラッグでは複数 frame で要り得る (§3.4)。
- r6 (同じ担当、76152aa1d): REVISE。反映した指摘:
  - P1: 実ページが 48px より小さいと、表示単位だけの規則は入力の制限の結果を動かす → 実ページごとの候補と表示単位の候補から補正量が最小のものを採る (§3.4)。
  - P2: 初期配置は保つ軸の範囲を保証しない → 初期配置を求めてから描画の規則を当てる順序、保つ軸は範囲外なら戻る、印と layout は当てた後の値 (§3.3)。
- r7 (同じ担当、59ca11ea2): REVISE。反映した指摘:
  - P1: 初期配置の frame で自動の初期配置の印を常に実際のパンにすると、半ページ送りで保った手動パンまで自動扱いになりクリック送りが戻る → 既存の `origin_automatic` の区別を保つ (§3.3)。
- r8 (同じ担当、5777cd087): **ACCEPT**。P3 (§4.3 のテストの文言) を反映。

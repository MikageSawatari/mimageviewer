# ページ固着 第 2 層 — 読み込みを「開始する」側も止まっている

[spread-page-turn-upload-deadlock.md](spread-page-turn-upload-deadlock.md) の修正
(commit `52e1022a`) を入れた**後**も固着が再現した。実ログで原因を特定した。

## 1. 何が起きているか

前回直したのは「**完成済みの結果を upload する**」側だった。しかし**読み込みを開始する側が
同じフラグで止まっている**ため、upload するものがそもそも生まれない。

[ui_fullscreen.rs:13018](../../src/ui_fullscreen.rs:13018):

```rust
if !page_turn_decision.defers_full_resolution_work()
    && self.items.get(fs_idx).is_some_and(GridItem::has_page_data)
{
    self.ensure_fs_page_load(fs_idx);   // ← 読み込みを開始する当人
}
```

同じ形が **3 箇所**:

| 場所 | 止めているもの |
| --- | --- |
| [13018](../../src/ui_fullscreen.rs:13018) | **現ページ**の `ensure_fs_page_load` |
| [13078](../../src/ui_fullscreen.rs:13078) | **見開き相方**の `ensure_fs_page_load` |
| [23106](../../src/ui_fullscreen.rs:23106) | open 時の `FsOpenMaterialization::DeferredPageTurn` |

**sequence は target の完成を待つ。完成には読み込みが要る。その読み込みは sequence が
未解決だから始まらない。** 前回と完全に同じ形が、ひとつ上の階層にあった。

## 2. 実ログの証拠 (2026-08-20、利用者の実機)

対象: `C:	mp\miv-spread-webp-portrait-600-20260820`、target = pages [55, 56]、seq 157

```
t=32.571  enqueue      idx=55,56  items_gen=6
t=36.510  load_begin   idx=55,56  seq=88          ← 読み込みはここで 1 回だけ開始
t=36.536  decode_end   webp 1200x1800 25ms         ← 成功
t=36.536  thread_exit  reason=static_ok            ← 正常終了
t=36.547  decode_end   from_cache=true  idx=55,56  ← サムネイルもキャッシュから取れている
t=36.545 〜 105.7      passthrough_unavailable reason=thumbnail_not_loaded
                       frames 1 → 2 → 4 → ... → 32768   (60 秒以上継続)
```

- `fs/ready` は 55/56 について**一度も出ない**
- `load_begin` は **t=36.510 の 1 回きり**。seq が 88 → 90 → 157 と進んでも**再要求が無い**
- 固着時の状態: `materialized_ready=false` / `rendition_ready=false` /
  `next_phase=still_awaiting` / `ui_work_admission=navigation_target_uploads_only`

**除外できた仮説** (調べて違った):

- **世代不一致で捨てられている** → `[fs-generation]` のログが **0 件**。捨てられていない
- **App-global と detached bundle の取り違え** → mount 中は App 側が detached bundle を
  保持している (probe で確認済み)。取り違えは起きていない

## 3. 直し方

**抑止は paint source に従わせる。** 今の型はほぼ正しく、規則が 1 つ足りない:

| 状態 | 描いているもの | target のページに対する full-resolution 作業 |
| --- | --- | --- |
| `rendition_sequence_active && passthrough_rendition_ready` | **rendition (代役)** | **抑止してよい** (連打が軽くなる元の意図) |
| `rendition_sequence_active && !passthrough_rendition_ready` | **materialized (本物)** | **抑止してはならない** — 代役が無く、本物を作る以外に前へ進む道が無い |
| 非 active | materialized | 抑止しない |

つまり **`paint_source == Materialized` のとき、target 自身のページについては
producer も consumer も通す**。target 外の先読みは従来どおり抑止する。

前回入れた `FsPageTurnWorkAdmission` を拡張する形になる (名前が `...UploadsOnly` のままだと
実態と食い違うので**改名する**)。**bool を増やさないこと。**

⚠️ **23106 (open 経路) は意味が違う可能性がある。** ページ送りではなく新規 open なので、
同じ規則を当てる前に**確認し、違うなら触らずに理由を報告すること**。

## 4. サムネイル側も説明が要る

`decode_end from_cache=true idx=55` が t=36.547 に出ているのに、その後もずっと
`reason=thumbnail_not_loaded` である。**代役が用意できていれば、そもそも本物を待つ必要が
無かった。** なぜサムネイルの結果が反映されないのかを調べ、

- **同じ抑止に巻き込まれている**なら §3 の規則で一緒に直す
- **別の原因**なら、実装せずに報告する (別件として扱う)

## 5. 制約

- **timeout / 強制 reset / 追加 repaint では直さない。**
- **前回の修正を撤回しない。** upload 側の循環は実在し、テストも入っている。今回はその手前。
- **detached predicate / viewport 経路へ及ぶ場合は実装せずに報告する**
  ([detached-rework-plan.md](../detached-rework-plan.md) §2)。

## 6. テスト

- **target のページに thumbnail も full-size も無い状態から、読み込みが開始され、
  materialized で settle する**こと。今回の実ログの状態そのもの
- 代役 (rendition) が**ある**ときは、従来どおり target の full-resolution 作業が抑止される
  こと (連打の軽さを失っていないこと)
- 見開きの**相方**も同じ規則で読み込みが開始されること
- **同じ形の探索をやり直す**: 「抑止フラグが、その抑止を解除する唯一の作業を止める」箇所が
  他に無いか。前回「無い」と報告して見落としたので、**producer 側 (`ensure_*` / `start_*` /
  `spawn_*` / `enqueue_*`) を明示的に数え上げること**

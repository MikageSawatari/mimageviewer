# stage-r2e-2c1 — 外からのフィールド「読み」を `ContextRef` へ寄せる

正本プラン: [detached-rework-plan.md](detached-rework-plan.md)。
**着手前に同書 §2 (憲法) と §9.5 (作業環境を含む) を読むこと。**
設計の正本: [briefs/detached-r2e-ownership-design.md](briefs/detached-r2e-ownership-design.md)
**§3.6 (`ContextRef` / `ContextMut`)**、§6.1、付録 B-3 / B-4。

ブランチ: `r2e-ownership` (worktree `C:\home\mimageviewer-r2e`)。
コミットメッセージに `(detached-rework R2e-2c1)` を含める。

設計の②-c は「accessor 移行」と「監査ツール導入」を 1 段に入れていたが、**共有するものが無い**
ので分けた。さらに accessor 側も**読みと書きで性質が違う**ので分けた。この指示書は
**読みだけ**を扱う。

- ②-c1 (この指示書) — 外からの**読み** 106 箇所を `ContextRef` accessor へ
- ②-c1b — 外からの**書き** 27 箇所を名前の付いた操作へ (§4 に材料を置いた)
- ②-c2 — `tools/viewer_context_audit` の導入 (A2 / A3 / A7)

**この段は挙動不変。** ただし②-b と違い純粋な移動ではない — 読み口が変わる。

---

## 1. 実測 (推測ではない)

225 フィールドを一時的に private にして
`cargo check -p mimageviewer --bin mimageviewer-core` を通した結果 (`f6324875` 時点、非テスト面):

| | 箇所 | 内訳 |
| --- | --- | --- |
| 外部アクセス 合計 | **164** | E0616 |
| うち `split_materialized_physical_context_for_independent_still_open` の中 | 31 | **関数ごと移す** (§2 ステップ 1) |
| うち **書き** | 27 | **②-c1b** (§4) |
| **この段の対象 = 読み** | **106** | 21 フィールド |

設計 B-3 は「~26 種」と書いているが、**実測は 21 種** (読みだけ)。この表を正とする。

### 1.1 accessor が要る 21 フィールド (実測、読み箇所の多い順)

| 読み箇所 | フィールド | 出現ファイル |
| --- | --- | --- |
| 21 | `fullscreen_idx` | app.rs |
| 18 | `items` | app.rs |
| 17 | `fs_cache` | app.rs |
| 13 | `viewer_session` | app.rs |
| 5 | `pdf_password_request` | app.rs |
| 4 | `current_folder` | app.rs |
| 4 | `fs_pending` | app.rs |
| 3 | `items_generation` | app.rs |
| 3 | `video_audio_mode` | app.rs |
| 3 | `video_audio_vst` | app.rs |
| 3 | `vst3_deferred_media_open` | app.rs |
| 2 | `fs_lanczos_cache` | **vram_accounting.rs** |
| 2 | `tag_prewarm_pending` | app.rs |
| 1 | `bookmark_open_pending` | **startup_ops.rs** |
| 1 | `selected` / `bookmark_view_state` / `archive_source_override` / `music_bookmarks` / `music_bookmarks_loaded_for` / `normalize_ui_states` / `final_ai_pending` | app.rs |

app.rs だけではない。`src/app/startup_ops.rs` と `src/app/vram_accounting.rs` も対象。

---

## 2. 手順

### ステップ 1 — materialized fork を移す (純粋な移動)

`split_materialized_physical_context_for_independent_still_open` を doc comment ごと
([app.rs:40913](../src/app.rs:40913)〜[:40956](../src/app.rs:40956))
`src/app/viewer_context_registry.rs` の `impl App` ブロックへ移す。
②-b と**同じ規律**: 中身を書き換えない、`#[cfg(windows)]` を保つ、`pub(in crate::app)` を付ける。
これだけで 31 フィールドが対象から外れる。

### ステップ 2 — `ContextRef<'a>` を足す

registry モジュールに置く。**2 つの由来を 1 つの読み口にする**のが目的。

```rust
/// 1 つの context への読み取り。マウント中なら App のミラーフィールド、
/// そうでなければ bundle を読む。呼び出し側はどちらか知らなくてよい。
pub(in crate::app) struct ContextRef<'a> { /* private: Mounted(&App) | AtRest(&ViewerContextBundle) */ }

impl<'a> ContextRef<'a> {
    pub(in crate::app) fn mounted(app: &'a App) -> Self;
    pub(in crate::app) fn at_rest(bundle: &'a ViewerContextBundle) -> Self;
    // §1.1 の 21 フィールドぶんの accessor
}
```

- **`id()` は作らない。** context identity は②-d で入る。この段では由来 2 種だけ。
- 戻り値は**現在の読み方に合わせる**。実際に使われている操作に絞れるなら絞る
  (例: `fs_cache` 全体を貸すより `values()` 走査や `get(idx)` に絞れないか見る)。
  絞れないものは `&` をそのまま返してよい。**呼び出し側の書き換え量を減らすために
  意味を広げないこと。** どちらを選んだかと理由を報告に書く。
- **`ContextMut` は作らない。** ②-d。

### ステップ 3 — `&ViewerContextBundle` を取るヘルパー 12 本を `ContextRef` へ

[app.rs:2400](../src/app.rs:2400) (`ClosedBookmarkSummary::read`) /
[8643](../src/app.rs:8643) / [8685](../src/app.rs:8685) / [8717](../src/app.rs:8717) /
[8761](../src/app.rs:8761) / [8798](../src/app.rs:8798) / [8833](../src/app.rs:8833) /
[8874](../src/app.rs:8874) / [38128](../src/app.rs:38128) / [38149](../src/app.rs:38149) /
[38161](../src/app.rs:38161) / [38336](../src/app.rs:38336)。

引数型を `ContextRef<'_>` に変え、中の `bundle.<field>` を accessor 呼び出しにする。

⚠ **重複定義を 2 組、ここで 1 本に畳む。** これは「統一のための統一」ではない。
**本文が同一で、片方だけ直せば静かに乖離する**実物である。

| 重複 | 場所 |
| --- | --- |
| `viewer_context_bundle_contains_video` / `current_viewer_context_contains_video` | [38128](../src/app.rs:38128) / [38221](../src/app.rs:38221) — **本文が `bundle.` と `self.` の違いだけで一字一句同じ**。確認済み |
| `viewer_context_bundle_is_music_consumer` が free fn と `impl App` の関連関数で二重定義 | [8685](../src/app.rs:8685) / [38336](../src/app.rs:38336) |

`ContextRef` なら `mounted(self)` と `at_rest(&bundle)` の両方から同じ 1 本を呼べる。
**`active_detached_viewer_context_contains_video` ([38214](../src/app.rs:38214)) も
同じ 1 本の呼び出しになるはず**なので、3 経路が 1 本になったことを報告で示す。

⚠ **本文が同一だと確認できた組だけ畳む。** 差があるなら**畳まずに報告する**。
差は仕様かもしれないし、既に起きた乖離かもしれない。どちらでもこの段で決めない。

### ステップ 4 — 残りの直接読みを置換

ステップ 1〜3 で消えなかった読みを accessor 呼び出しにする。

---

## 3. 完了条件 (コンパイラが判定する)

**grep で測らない。コンパイラに列挙させる。**

1. `ViewerContextBundle` の 225 フィールドの `pub(in crate::app) ` を一時的に外して private にする。
2. `cargo check -p mimageviewer --bin mimageviewer-core --message-format short`。
3. **残る E0616 が §4 の 27 箇所 (書き) だけ**になれば、この段の変換は完了。
   読みが 1 件でも残っていれば未完。**27 箇所より減っていたら**、書きを勝手に触った
   ということなので報告する。
4. `pub(in crate::app)` を**元に戻す**。テストはまだ直接フィールドを触るので、
   この段では private にできない (`test_access` の導入は②-e)。
   **private のままコミットしないこと。**

そのうえで:

5. `cargo test -p mimageviewer --lib` が緑。**件数 6251 のまま**。
6. `cargo fmt --check` が無出力。
7. `git diff --numstat HEAD` を貼る。変更してよいのは
   `src/app.rs` / `src/app/viewer_context_registry.rs` / `src/app/startup_ops.rs` /
   `src/app/vram_accounting.rs` の 4 本。**`src/app/tests.rs` は無変更**。
8. 3 経路の `contains_video` が 1 本になったこと、`is_music_consumer` の二重定義が
   消えたことを、実際の `rg` 出力で示す。

---

## 4. この段では触らない「書き」27 箇所 (②-c1b の材料)

**1 つも直さない。** 名前の付いた操作にするのは②-c1b で、そこでは
「何をする操作なのか」を決める必要がある。ここで場当たりに setter を生やすと、
設計 §6.2 が名指しする「**公開面が静かに育つ**」に当たる。

| # | 束 | 箇所 | 中身 |
| --- | --- | --- | --- |
| W1 | tag prewarm 起動 | [27266](../src/app.rs:27266) / [27273](../src/app.rs:27273) | `tag_prewarm_pending = Some(spawn())`。**同じ代入が 2 箇所** |
| W2 | normalize state の破棄 | [38482](../src/app.rs:38482) / [38483](../src/app.rs:38483) | ②-a の teardown。設計 §4.4 が既に `ContextMut::clear_normalize_state()` と名前を決めている |
| W3 | auto-open intent の解除 | [39194](../src/app.rs:39194)〜[:39197](../src/app.rs:39197) / [40089](../src/app.rs:40089)〜[:40090](../src/app.rs:40090) | 4 フラグの reset。**片方は同じ 4 つのうち 2 つしか落としていない** |
| W4 | detached physical への読み替え | [41013](../src/app.rs:41013)〜[:41030](../src/app.rs:41030) | 12 代入の一連 |
| W5 | index を指し直す | [41239](../src/app.rs:41239)〜[:41248](../src/app.rs:41248) | 6 代入の一連 |
| W6 | bookmark_view_state の設定 | [startup_ops.rs:592](../src/app/startup_ops.rs:592) | 1 件 |

⚠ **W3 の非対称は②-c1b で結論を出す。** 意図的な部分解除かもしれないし、
同型経路の直し残しかもしれない。**この段では判断しない。**

---

## 5. その他やらないこと

- **保管を変えない。** `active_detached_viewer_context` / `paused_bundle` はそのまま。
- **監査ツールを作らない。** ②-c2。
- **accessor を「あると便利だから」で増やさない。** §1.1 の 21 種だけ。
  使われない accessor は②-e の A4 (公開面 allowlist) で邪魔になる。
- **テストを増やさない。** 挙動不変で、既存 6251 件が回帰の番人である。
  ただし**畳んだ 2 組の述語**について、mounted 由来と at-rest 由来の両方で同じ答えを
  返すことを見るテストが既存に無ければ、**それだけは足してよい** (最大 2 本)。
  足したら「なぜ既存では不足か」を報告に書く。

---

## 6. 報告に必ず含めること

- §3 の 8 項目の実出力。特に **残る E0616 が 27 件だけになった時点の `cargo check` 出力**。
- accessor の戻り値をどう決めたか (`&` をそのまま貸したもの / 操作に絞ったもの、と理由)。
- 畳んだ 2 組について、**本文が同一であることをどう確かめたか**。
  差があって畳まなかったものがあれば、その差の中身。
- ステップ 1 の移動が「移動だけ」であることの確認方法。
- 作業中に §4 の 27 箇所について気づいたことがあれば書く (直さずに)。

# 引き継ぎ: 非 Windows ビルドが 223 エラーで落ちている (レーン A 宛)

- 宛先: レーン A (`video-latency-and-context-ownership` / detached リワーク R2e〜)
- 起票: 2026-08-28、ClaudeCode
- 対象: master `dd39d1f6` (レーン A / B / C 統合後)
- CI run: https://github.com/MikageSawatari/mimageviewer/actions/runs/33171796853

## 要旨

**GitHub Actions の `cargo check (ubuntu / non-Windows cfg)` が 223 エラーで落ちている。**
`cargo fmt` と viewer context audit は通っている。Windows 上のローカルビルドでは
**原理的に出ない**種類の失敗なので、CI だけが拾う。

**CI は 2026-08-23 (v3.2.0) 以降、一度も緑になっていない。** 8/26 の run は GitHub の
Actions 障害で 3 ジョブとも起動しないまま失敗しており、その間に 102 コミットが入った。
つまり今回が、レーンを統合してから最初の実測になる。

出荷ブロッカー。CLAUDE.md のリリース手順 Phase 2 が「CI が緑であること」を求めている。

## この job が何をしているか

```
cargo check --locked --bin mimageviewer-core --features portable    # ubuntu-latest
```

目的は `#[cfg(windows)]` で宣言した item を、cfg 無しのコードから参照していないかの
機械的検出 (`.github/workflows/ci.yml` の冒頭コメント参照)。Windows では
`cfg(windows)` が真なので、この不整合はローカルの `cargo check` / `cargo test` /
`clippy` のどれでも出ない。

## 内訳 — 根本原因は 4 つ

| エラー | 件数 | gate されている item | 宣言箇所 |
| --- | --- | --- | --- |
| E0609 `no field \`fs_cache\`` | **145** | `App::fs_cache` | [app.rs:9819](../src/app.rs) |
| E0614 / E0308 (下記の波及) | 49 | — | — |
| E0433 `cannot find \`presentation_observer\`` | 9 | `crate::presentation_observer` | [lib.rs:59](../src/lib.rs) |
| E0599 `observe_viewport_presentation_command` | 9 | `App::observe_viewport_presentation_command` | [app.rs:38158](../src/app.rs) |
| E0609 `no field \`video_presentation_transition\`` | 5 | `App::video_presentation_transition` | [app.rs:11134](../src/app.rs) |

E0614 (`type \`usize\` cannot be dereferenced` ×35) と E0308 (×14) は、`fs_cache` が
解決できないことでクロージャの型推論が崩れた波及。**4 件を直せば一緒に消えるはず**なので、
最初から個別に追わないこと。

参照側の分布 (unguarded な参照を持つファイル):

```
84  src/app.rs            5  src/ui_music_panels.rs      1  src/ui_erase.rs
61  src/ui_fullscreen.rs  3  src/pipeline_debug.rs       1  src/ui_adjustment_panel.rs
 7  src/tray_integration.rs  2  src/app/snapshot_ops.rs  1  src/app/vram_accounting.rs
                             2  src/app/gamepad_input.rs 1  src/app/color_filter.rs
```

## ① `fs_cache` は事故。属性の付け間違い (145 件 = 全体の 2/3)

**`bf391e6a` (R2e-2d「Give a viewer context one home, one identity, and one way to move」)
の diff が原因を示している:**

```diff
     #[cfg(windows)]
     native_video_parked_live_activation_requests: Vec<u64>,
     #[cfg(windows)]                                        ← この属性は…
-    next_detached_viewer_context_serial: u64,              ← …消したこのフィールドのものだった
     /// 先読みキャッシュ: item_idx → ロード済みエントリ（静止画 or アニメーション）。
     /// entry はこの bundle の items_generation を刻み、全参照で照合する。
     pub(crate) fs_cache: ItemsGenerationMap<FsCacheEntry>, ← 宙に浮いた属性がここへ付いた
```

registry が置き換えた `next_detached_viewer_context_serial` を削除したとき、**その
`#[cfg(windows)]` だけが残り、次の item である `fs_cache` に結合した**。

`fs_cache` は静止画・アニメーションの先読みキャッシュで、Windows 固有の要素は無い。
参照元も `ui_main` / `ui_adjustment_panel` / `ui_text` など表示系全般。
**意図的な gate ではないと考えている。**

- 対応案: `app.rs:9819` の `#[cfg(windows)]` を 1 行削除するだけ。
- ⚠ ただし判断はレーン A に委ねる。registry の設計上 `fs_cache` を本当に Windows 専用に
  したのであれば、削除ではなく参照側を gate することになる。**どちらかは設計を知っている
  側が決めてほしい。**

> 補足: これは 2026-08-27 に別途直した「`#[test]` が消した関数の分だけ残って隣の関数に
> 結合していた」(`38e5e230` / `10e9bf82`) と**まったく同じ形**。item を消すときに、その
> 上の属性も一緒に消えたかを確認する価値がある。他にも同型が無いか見てほしい。

## ② 残り 3 件は意図的な gate に見える。参照側の問題

- `crate::presentation_observer` ([lib.rs:59](../src/lib.rs)) — Win32 の DWM / backend stage
  observer なので Windows 専用は妥当。`lib.rs:962` の呼び出しは正しく `#[cfg(windows)]`
  されている。落ちているのは [gamepad_input.rs:5701](../src/app/gamepad_input.rs) と
  [app.rs:35649](../src/app.rs) など、`crate::presentation_observer::WindowAction::Focus`
  を cfg 無しで参照している箇所。
- `App::video_presentation_transition` ([app.rs:11134](../src/app.rs)) — presentation
  migration の owner。
- `App::observe_viewport_presentation_command` ([app.rs:38158](../src/app.rs))。

これらは **参照側を `#[cfg(windows)]` にするか、非 Windows 用の no-op stub を置く**のが
筋。`lib.rs:62` 付近に既に「非 Windows stub」の前例がある (DWM helper の no-op)。

## 直したかの確認方法

**ローカルの Windows では確認できない。** 非 Windows 検証用のスクリプトはリポジトリに無い。
現実的な手段は次のどちらか。

1. **ブランチを push して CI を読む** (推奨)。この job だけなら 1〜2 分で終わる。
   `workflow_dispatch` も追加済みなので、同じ ref で手動再実行もできる。
2. `cargo check --target x86_64-unknown-linux-gnu` — Linux ツールチェインとリンカが要る。
   `cargo check` だけならリンクしないので `rustup target add` + libclang 相当で通る
   可能性はあるが、**未検証**。試すなら FFmpeg の bindgen が非 Windows で通るかが関門。

ログの読み方 (CLAUDE.md より): `gh run view <id> --log-failed` はビルド全体の warning も
含むのでファイルへ落として `error\[E` で絞る。`-->` の行に実ファイル位置が出る。

```bash
gh run view <run-id> --log-failed > /tmp/ci.log 2>&1
sed 's/\x1b\[[0-9;]*m//g' /tmp/ci.log | grep -oE "error\[E[0-9]+\]: .*" | sort | uniq -c | sort -rn
```

## この文書の前提を疑ってよい点

- ①の「事故である」は diff からの推定。**registry の設計意図は確認していない。**
- 波及 49 件が 4 件の修正で消えるというのも推定。残ったら個別に見る必要がある。
- 参照側を gate するのが正しいのか、非 Windows stub を置くのが正しいのかは、
  経路ごとに違うと思われる。一律に決め打ちしないでほしい。

## 関連

- CLAUDE.md 「リリース手順チェックリスト」Phase 2 (6.5) — CI 緑が出荷条件
- `.github/workflows/ci.yml` — この job を追加した経緯とコメント
- backlog §1.132 — v3.3.0 出荷前レビューの記録 (第 3 ラウンドまで)

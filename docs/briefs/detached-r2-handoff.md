# 新セッション用 指示文 — detached リワーク R2 と §1.99 / §1.100

(2026-08-21 作成。別セッションで v3.1.3 のリリース作業と並行して進めるための指示)

---

## セットアップ

```powershell
cd C:\home\mimageviewer
git worktree add ..\mimageviewer-detached detached-rework
cd ..\mimageviewer-detached
```

**`vendor/` を用意する。** 本体パッケージをビルドするので `build.rs` が必須ファイルを検査する。

```bash
bash scripts/bootstrap-vendor.sh
cp -r /c/home/mimageviewer_vendor_backup/models vendor/
cp -r /c/home/mimageviewer_vendor_backup/vst3-host vendor/
```

⚠️ **`vendor/` を junction / symlink で共有しない。** worktree 撤収時の再帰削除で本体側の
実体を消す事故が 2026-05 に複数回起きている。撤収は必ず
`.\scripts\safe-worktree-remove.ps1` 経由 (`git worktree remove` を直接呼ばない)。

---

## 指示文 (ここから新セッションへ貼る)

detached viewer リワークの **R2 (状態の集約)** と、それに依存するバグ 2 件に着手してください。

### 最初に読むもの

1. `docs/detached-rework-plan.md` の **§2 (憲法)** — 全ステージ共通の禁止事項。過去 15 ラウンド
   以上の失敗から抽出されたもの
2. 同 **§3 (体制とステージ実行プロトコル)** と **§4 の R2** — 実施の段取り
3. 同 **§11** — **2026-08-20〜21 に、リワーク外から 3 件の所有権修正が入っている**。
   R2 はこれらと整合を取る必要がある
4. `docs/next-release-backlog.md` の **§1.99** と **§1.100** — どちらも根本原因が特定済み

### 2026-08-20〜21 に分かったこと (再調査不要)

**共通の根は「active bundle が単数で、window ごとの所有が型になっていない」ことです。**
R2 が解決すべき対象そのものです。実測と特定が済んでいます:

- **非アクティブな窓の bundle は `detached_image_windows[id].paused_bundle` にある。**
  `active_detached_viewer_context` は単数で、`is_none()` は所有者を示さない
  (これを所有者判定に使って失敗した実例が §11 にある)
- **`MouseGestureState` は App に 1 個だけあり、window / viewport / session の識別を持たない。**
  非アクティブな窓が集める pointer 情報は `any_pressed` / `any_released` だけで、
  アクティブ化は press ではなく release で起きる。**非アクティブな窓で始めた最初の
  右ドラッグは構造上必ず失われる** (= §1.100)
- **typed open plan が本コンテナとして認識するのは PDF / ZIP だけで、`ConvertibleArchive`
  (RAR / CBR) は対象外。** RAR は共通の detached 振り分けを素通りして通常ナビゲーションへ落ち、
  変換可否の probe 完了後に**その時点で mount されている context** へ書き込む (= §1.99)。
  「RAR を ZIP と同じ descriptor に足す」では直らない (非ソリッド・入れ子なしだけが直読み)

### 進める順序

1. **R2 本体** — `DetachedWindowRuntime` + reducer + placement 一本化。
   **window ごとの所有を型にする**のが核心
2. **§1.100** — R2 が入れば、ジェスチャ状態を window 所有にできる。
   **R2 より前に App へ識別子を足すのは憲法 3 に抵触する**
3. **§1.99** — 非同期完了に「detached grid 要求の window / request owner」を型として持たせる。
   visibility 述語や `show_viewport_*` builder の変更は不要と見込まれている

### 守ること

- **`master` を触らない。** 別セッションが v3.1.3 のリリース作業中。マージは向こうの
  リリース完了後に相談する
- **§2 の憲法**、特に: 新しい detached 用 bool / Option を App へ足さない (3) /
  時間窓で競合を吸収しない (5) / 指示外のものを「ついでに」直さない (7) /
  既存 detached テスト 104 本を削除・弱体化しない (8)
- **実機確認が要る変更はコミット前に利用者へ依頼する** (CLAUDE.md「実機検証用バイナリの準備」)。
  エージェントは通常 profile のバイナリを起動しない
- 応答は日本語で書く (CLAUDE.md 冒頭)

### 参考になる直近の作業

§11 の 3 件は**どれも「所有者に処理させる」形の修正**で、R2 が目指す方向と同じです。
特に「keep-alive backstop」の記録には、**`None` に 2 つの正規の意味がある**という
落とし穴が書かれています。R2 で状態を型にするとき、この区別をどう表すかが論点になります。

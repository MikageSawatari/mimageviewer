# v2.11.0 リリース進行状況 (作業用メモ)

**このファイルはリリース作業中の一時メモ。出荷後に削除してよい。**
最終更新: 2026-08-04。実装 = Codex Sol / レビュー・検収・進行 = ClaudeCode。

---

## 1. リリース内容 (コミット済み)

| コミット | 内容 | 実機確認 |
| --- | --- | --- |
| `20fb2db4` | トレイ / 最小化中にメインスレッドが 1 コアを 100% 占有するバグ。eframe 0.33.3 を vendoring し upstream PR #7905 を backport | 退行なしのみ確認。**元症状の再現は未実施** (経緯は下記) |
| `8ed3a34a` | 設計正本 `dot-by-dot-and-downscale-plan.md` と調査記録 | — |
| `b38610d5` | ドットバイドット表示 (物理 1:1 + ピクセルスナップ + 見開き高さ合わせの 1:1 例外) | 済 (100/125/150%、トリム併用も確認) |
| `96eed1ea` | GPU Lanczos3 の spike。mip 前縮小が不要と判明 | — |
| `2d6eb8e8` | GPU Lanczos3 の製品統合 (C-1 方式) | 済 (画質・リサイズ・ズーム・退行) |
| `a481a34b` | 「縮小時のなめらかさ」設定追加 + 旧 trilinear 経路と旧設定の削除 | 済 (設定移行・スライダー・旧 UI 消滅) |

### 未着手 / 進行中

- **バックログ 1.44** (上部バーロック時にインジケータが隠れる) +
  **既知の問題ページの棚卸し** → `docs/brief-v2.11.0-topbar-indicator.md` を Codex へ

---

## 2. 出荷前に必ず通すもの

### 2.1 idle-health (今回は特に重要)

**eframe を vendoring して repaint スケジューラを書き換えているため必須。**
`tray-residency` シナリオは今回追加した新規で、初運用。

```powershell
.\scripts\check-idle-health.ps1 -Scenario static-foreground
.\scripts\check-idle-health.ps1 -NoLaunch -Scenario static-background
.\scripts\check-idle-health.ps1 -NoLaunch -Scenario tray-residency
```

`video-pin-background` は `-TargetKey` が必須。用意できなければ飛ばした旨と理由を記録する。

### 2.2 perf smoke

表示経路を大きく変えたので実施する。`bash scripts/perf_smoke.sh`。
**判定はヒッチ件数ではなく、個々の大きなギャップ直前の `ui.tail_repaint.action` を見る**
(`none` や `request_repaint_after_idle_upgrade` は正常)。

### 2.3 bench 回帰

検索系は今回触っていないので **省略可**。

### 2.4 その他

- `.\scripts\test-full.ps1` (build-dist.ps1 が内部で実行する)
- CI が緑か (`gh run list --limit 5`)。特に ubuntu の `cargo check` (cfg(windows) 漏れの番人)
- PDFium (`bash scripts/setup-pdfium.sh check`)、FFmpeg (`bash scripts/setup-ffmpeg.sh check`)、
  Susie ワーカー再ビルド、VST3 bridge
- リリース exe に `dumpbin /dependents` で VCRUNTIME140.dll が出ないこと
- コード署名の確認 (SimplySign Desktop にログインしてから `build-dist.ps1`)

---

## 3. バージョン番号を上げるファイル (Phase 1)

1. `Cargo.toml`
2. `installer/mimageviewer.iss` の `MyAppVersion`
3. `installer/readme.txt` の先頭版表記
4. `installer/readme_portable.txt` の先頭版表記
5. `htdocs/mimageviewer/index.html` のダウンロード欄・**最終更新日**・
   **ポータブル版のリンク URL** (版番号を含む)
6. `htdocs/mimageviewer/manual/index.html` の版表記
7. `python scripts/gen-changelog-html.py` で changelog.html を再生成 (README から生成される
   **生成物**なので手で編集しない)

`src/version_highlights.rs` の v2.11.0 エントリは `a481a34b` で追加済み。

---

## 4. README.md 更新履歴 (Phase 0)

**未着手。アプリ内の更新通知にそのまま表示されるため、公開前に利用者の承認が必要。**

書くべき内容の要点:

- 100%原寸 / 拡大しない / 縮小しない が物理ピクセル基準になった
  (高 DPI 環境では従来より小さく表示される)
- 縮小表示の画質改善。モアレ抑制の設定は「縮小時のなめらかさ」に変わった
- 見開きで高さの違うページが片方だけぼやける問題の修正
- トレイ / 最小化中に CPU を使い続ける問題の修正
- (1.44 を入れる場合) 上部バーロック時にインジケータが隠れる問題の修正

内部用語 (Lanczos、mipmap、eframe、スケジューラ等) は書かない。
バージョンタグ (v2.11.0+ 等) を本文に書かない。見出しに 1 回だけ。
**8KB 上限**: `awk '/^### v2\.11\.0( |$)/{f=1} /^### v2\.10\.0( |$)/{f=0} f' README.md | wc -c`
で 8192 以下を確認。超えたら `docs/release-body-2.11.0.md` に短縮版を作る。

---

## 5. 経緯で残っている判断・注意点

- **トレイ CPU バグの元症状は再現確認できていない。** 隔離環境で「hidden 移行時に repaint
  要求が残っている」条件を作れなかった (mIV が速く就寝する / PostMessage でフルスクリーンを
  開けなかった)。利用者側でも「サムネイル一覧は一瞬で終わるので操作が難しい」ため、
  2026-08-04 に再現確認を見送る判断をした。詳細は
  `docs/tray-residency-cpu-spin-investigation.md` の末尾。
- **市松模様のモアレは残る。** 分離型リサンプルの最悪ケース。実トーン・実漫画ページでは
  改善している。利用者判断 (2026-08-04) でこのまま出荷。
  → ⚠️ **「非分離 (EWA) は逆に悪化した」という当初の記録は撤回した。** 同日の再測定で
    EWA (Jinc) の方が市松で 1 桁良く、角度依存もほぼ無いことが分かった。当時のスクリプトは
    残っておらず原因は特定できないが、縮小時にカーネル半径を `1/scale` へ広げていなかった
    可能性が高い。詳細と数値は `dot-by-dot-and-downscale-plan.md` §4.3.4。
    **v2.11.0 の出荷判断は変わらない** (現行経路は実機検証済み、Jinc はコスト 6.5 倍で
    リサイズ中の実測が必要) が、v2.12.0 の検討項目として backlog に登録済み。
  → **教訓**: v2.7.0 で市松のような最悪ケースに合わせて調整した結果、通常の内容に対して
    過剰なぼかしになり、今回の報告につながった可能性がある。合成の最悪ケースだけで
    パラメータを決めない。
- **検証素材**: `C:\tmp\miv-dpi-blur-test\` (ドットバイドット)、
  `C:\tmp\miv-downscale-compare\` (縮小画質、新旧比較、`full\README.txt` に一覧)、
  `C:\tmp\miv-moire-oldnew\` (利用者のモアレ試験画像での新旧比較)。
  バックアップ: `C:\home\mimageviewer_testdata_dpi\`、`C:\home\mimageviewer_testdata_downscale\`。
- **既知の問題 1.28 (VST カーソル) は v2.10.0 で修正済み**だが、その版で known-issues.html
  からの削除が漏れていた。v2.11.0 で削除する (ブリーフに記載済み)。

---

## 6. 配布 (Phase 3 以降)

```powershell
.\scripts\build-dist.ps1            # VST3 C++ 未変更なら -SkipVst3Bridge
```

配布物 4 種: 単体exe / setup.exe / `mImageViewer_installer_v2.11.0.zip` (Vector 用、
setup.exe + installer/readme.txt) / `mImageViewer_portable_v2.11.0.zip`。

その後 GitHub Release (body は README の該当セクション)、mikage.to 反映、Vector 申請。
MS Store は毎リリース必須ではない。

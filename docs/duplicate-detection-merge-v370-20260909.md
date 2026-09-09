# duplicate-detection への v3.7.0 統合記録

2026-09-09。利用者指定の master → duplicate-detection の統合。実装・テストは Sol / xhigh、独立レビューは別の Sol / xhigh。既存レビューのやり直しや機能追加には広げていない。

## 到達点

- マージコミット: `5d290b4cf7e2c3dfa5fe2ab843f4aa1c3b64c40f`。
- 統合対象: master / v3.7.0 の `dd997b538e1621f555e4c121ee061a1440ac8e68`。元の dupe HEAD は `5bbb6b3fbbd6be5889d8e26e98737b9a58eb116f`。
- 履歴の競合解決と未コミット変更復元後の統合を別々に独立レビューし、最終 integrated-fixed1 を承認。新規 P1/P2・未解決事項なし。
- 最終 `scripts/test-full.ps1 -SuppressCrashDialogs` は PASS。本体 7929 成功 / 43 ignored、vendor egui 25、egui-wgpu 9、eframe 15 成功。error mode 復元も成功。
- `build-portable.ps1 -KeepRunning` と `update-portable-dev.ps1 -SkipBuild`（Seed なし）は成功。24 ファイルが生成物と一致。
- 利用者実機の受入は未完了。過去の 7789 件成功と v3.6.0 portable 記録は統合前の証跡であり、本記録が新しい検証版を示す。

## 統合箇所と保存境界

履歴側の競合は detached 計画、開発ビルド文書、portable スクリプト。両側の記録と KeepRunning / PreserveRuntime / SmokeTestScript の契約を保持した。旧 ClaudeCode 担当指定の移行は既存のタスク内合意を維持し、detached の構造修正条件と §11 の両系統の記録を残した。

復元後は v3.7.0 の StillSeekGeometry/full_rect・入力受付・描画証跡と、dupe の viewer 別 preview/navigator、typed GPU source/paint 寿命、native navigation owner を共存させた。関連回帰と製品 check は成功。テスト fixture の入力初期化と typed Page 期待値も統合した。

共通ビルド機構では、master の gateExit/error mode 復元内に vendor egui の全 lib gate を維持した。portable の source fingerprint に vendor/egui を含め、その変更検知の回帰を追加した。これら dirty vendor に依存する調整は既存の unstaged 層へ置き、マージコミットへ混入させていない。

事前の staged/unstaged/untracked を個別保存し、保護 ref 4 本と stash を保持。untracked 117 ファイルは統合前と bytes/SHA が一致。staged R2 は 8 path のまま、統合後 patch は 100861 bytes / SHA256 `7e295682e3d4acd103242b2d1811ce7270bc3f71584dd6dc959db3c1d7110d7f`。変更された master 文脈への適応があるため、統合前 patch の hash とは異なる。R4/C/検証補助/vendor は unstaged のまま。未解決 index はない。master への逆統合と push は実施していない。

## 成果物とデータ保護

- 最終 manifest: `target/duplicate-v370-merge-checkpoint-20260909/final-release/manifest.json`、14558 bytes、SHA256 `8f2a7485b4169fdd636186f7479d09506aace2b9aa23114ac3b9143c7ea045f8`。
- source はマージ HEAD と既存 dirty 層を含む。879 records の fingerprint: `5649a9a274c226f2da88546097160283acb8c69995ddc393e33f1bdbd8c544ed`。後続の文書専用コミットはバイナリ内容を変更しない。
- 検証 exe: `target/portable-dev/mimageviewer.exe`、93786624 bytes、SHA256 `8b9d6af8981c18ea6406903134f0d3f2819ce73884b237b9e3891a387bbfec9b`。
- zip: `dist/mImageViewer_portable_v3.7.0.zip`、267289019 bytes、SHA256 `ca4fcbe7cf7fa1d7aeaf7dca93ae0a148a47485aa9f2142468b818b86fd229aa`。

package に data/data-remote がないことと reparse がないことを確認し、既存 portable-dev の実行プロセスがない状態で更新した。data 4118 entries / data-remote 1 entry の名前・種別・長さ・属性・日時は前後一致。内容の読取/hash/書込は行っていない。事後比較の初回は PowerShell の日時自動変換で誤検出し、DateKind String で比較して一致を確認した。通常プロファイルに触れず、アプリの起動・停止も行っていない。

## 手動確認

```powershell
Start-Process -FilePath C:\home\mimageviewer-dupe\target\portable-dev\mimageviewer.exe
```

既存の portable データを使う検証版。右パネルロック中の類似移動（通常フォルダ・ZIP/PDF）、長押し解除/フォーカス移動、見開き・連続表示、音声途切れの再発有無を利用者が確認する。実機待ちの R2/R4 等は未コミットのまま保持。自動検証成功を実機合格へ置き換えない。エージェントの対話的検証は新 AGENTS.md に従う明示承認が必要で、今回実施していない。

本照会の独立 oracle・通常 n20・特殊大規模 n3・release UI 補助測定は既存記録を引き継ぎ、今回再実行していない。§9.2 横断一覧と逆統合は範囲外。

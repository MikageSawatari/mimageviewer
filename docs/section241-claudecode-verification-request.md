# §1.241 ClaudeCodeへの検証依頼

2026-09-15。今回は修正の実環境検証を依頼する。公開・Store再申請の開始依頼ではない。

## 対象と現在地

- 作業場所: `C:\home\mimageviewer`、`master`
- 製品修正commit: `dd96be07394d5276db6a94ebd8a65db7bf32fd44`
- 調査正本: [VC++不足による起動停止の調査](msstore-startup-hang-vcrt-investigation-20260915.md)
- 実装・設計・検証証跡: [修正計画 §11](section241-ort-vcrt-startup-fix-plan.md#11-実装-checkpoint-2026-09-15)
- unsigned確認ビルドは `build-release.ps1 -PreserveRuntime` でexit 0。launcher `39669005...B9B4E9`、core `08E41A4B...C0578`、remote `1B313FF1...3237A`。詳細手順・全artifact SHA・合否と終了条件は [Sandbox / 配布引継ぎ](section241-sandbox-handoff.md) を正本とする。この確認物は署名済み最終配布物ではない。

独立Sol/xhighレビューは重要指摘なし。本体テスト8539成功・0失敗・45無視、snapshot52成功、vendor25/9/15成功。fmt、glyph、viewer context audit、PE依存検査も成功。担当変更のみを理由とする再実行は不要。配布物生成に必要なゲートは維持する。

## 修正内容

1. `ort` rc.12に上流の動的ロード失敗修正をbackportし、エラー生成時のOnceLock再入を解消。ORT 1.24.2と既存TensorRTパックの互換性は維持。
2. 正規VC Redist由来の4 DLLをcore EXEの隣へ同梱。launcherは実体hashを確認し、破損を修復する。
3. App・Remote・materializerでAI初期化の所有者を共有し、UI外で初期化。失敗・取消を含む終端を統一。
4. TensorRTの初期化失敗を型付きで親へ伝え、通信失敗と誤認した再試行を抑止。

## 検証の順序と合否

最初に、以前再現したサブPCのWindows Sandboxで正常起動を確認する。VC++再頒布パッケージを事前インストールせず、通常のセキュリティ設定を維持する。起動前に対象commit、配布物SHA、OS情報、VC++ DLLの有無を記録する。

1. **正常配布物**: 単体launcher・インストーラ・ポータブルを各使い捨て環境で初回／2回目起動。初回設定または通常画面まで到達し、応答なしにならず、AI初期化成功ログとcore隣のCRT4本を確認する。インストーラは調査時と同じsilent条件も確認する。
2. **失敗経路**: 使い捨てポータブル複製だけで、終了中にloose ORT DLLを欠落／破損させる。起動後、失敗理由が有限時間で記録され、通常閲覧のUIが応答することを確認する。初期化に入ってから5秒以内の失敗通知を目安とし、超過はログとともに開発へ返す。通常launcherの埋込自己修復とは別の試験にする。明示的な `init_from` を使うため `ORT_DYLIB_PATH` の変更だけではこの試験にならない。
3. **GPUあり環境の回帰**: DirectMLのアップスケール等、編集用パックの被写体分離、Remote AI、TensorRT infer／builderを確認する。SandboxのGPU条件でTensorRTまで合格扱いにしない。TensorRTの意図的な初期化失敗は使い捨てパックで確認し、明示理由が返り45秒×2の待機にならないことを記録する。
4. **配布物の検査**: 最終installer／portable生成時は更新済みの全PE依存gate、CRTのmanifest／Microsoft署名、通常の配布署名手順を通す。確認用release buildをそのまま正式配布しない。

正常起動の1項目目で失敗した場合は、広い回帰を続けず、成果物SHA・ログ・停止時刻・可能ならダンプを開発へ戻す。製品コードやビルドロジックの追加修正はCodex側へ返す。

## データ保護と結果の返却

Sandbox内の使い捨て設定・テスト画像のみを使用し、ホストの通常APPDATAや利用者のTensorRTパックを変更しない。GPU実機操作は具体的な対象・所要時間・入力使用範囲について利用者の了承を得てから行う。

結果は「対象commit／成果物SHA、OS・GPU、シナリオ、実施者、PASS・FAIL・未実施、ログ／画面の保存先」で返す。署名・版番号更新・公開・Store再申請は検証後の別段階とする。

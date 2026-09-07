# 実アプリ smoke の自動化 (§1.197)

2026-09-07。担当・検証台帳は [v3.7.0-priority-work.md](v3.7.0-priority-work.md)。
既存のテスト・実機確認・通常データ保護を維持し、検証できる入力経路を段階ごとに増やす。
各段階は独立レビューを受け、未実装の段階を自動化済みとして数えない。

## 範囲

| 段階 | 実アプリで確認すること | 必要な境界 |
| --- | --- | --- |
| S0 | 診断ビルドの隔離と使い捨てデータでの起動 | `portable,test-script` の別target/staging、exact exe/data、成果物の証跡 |
| S1 | 複数窓でPDFを開き、bindingとページの独立性を確認 | registry由来の窓snapshot、固定target、viewportを指定したKeyAction |
| S2 | 静止画の列の押下・ドラッグ・release | egui pointer timeline、描画ownerの名前付き矩形、frame acknowledgment |
| S3 | 動画canvasのzoomとstrip/panel/modalとの入力優先順位 | pump ownerの入力sink、source/generation、native矩形、pump/render acknowledgment |

## S0: 配布物と診断成果物の分離

通常portableは既存の`target-portable`・`dist`・zipの流れを維持する。診断モードは
`target-portable-test-script`で`portable,test-script`をビルドし、
`target/portable-smoke-package`へ組み立てる。署名・zip・通常配布先への書込は行わない。
依存copyの定義は通常portableと共用し、別のDLL/model一覧を作らない。

`prepare-portable-smoke.ps1 -TestScript`から、既存の安全な最終パス
`target/portable-smoke/mimageviewer.exe`と同`data`を準備する。診断runnerに任意exe/dataの
引数を持たせず、起動対象・データ・使い捨てmarker・reparse pointを検査する。
生成・置換・削除前にもworkspace内の固定パスであることを確かめる。
`-SkipBuild`はfeature等の生成証跡とexeの一致を要求し、通常portableや古い別成果物を
診断版と誤認しない。診断準備から既存の通常profileプロセスを停止しない。

独立レビューで、leafだけでなくrepoからの全祖先のreparse検査、Cargo workspaceの
固定、build前後のsource fingerprint照合、gitから除外された埋込SVG等の明示列挙を
追加した。準備不成立はexit 2、実行済みRhai assertionは1、timeoutは124として区別する。

既存の`page-turn-smoke.ps1`はnormal coreを起動する経路があるため、その起動処理は流用しない。
通常profileの設定・画像・DBをsmokeデータへコピーしない。
現`lib.rs`は`--test-script`実行時にsingle-instance取得・既存instanceへのactivate/open-path
配送・listener起動を省略する。runnerはこの既存経路と明示的な使い捨てdata-dirを維持し、
利用者の別portableへfixtureを転送しない。新しいinstance名前空間は追加しない。

## S1: 窓snapshotとtargetの所有

App投影だけでなくregistryのread-only参照から、window ID・context serial・residence・
viewport ID・現在項目/ページ・items generationを公開する。snapshot取得のためにmountを
入れ替えたり、他contextのworkerをdrainしたりしない。Buildingの未公開bindingは公開しない。
描画済みの証拠は、対応するviewport/owner/frameから得る。窓が存在することだけで
PDFが表示されたと判定しない。

scriptが選ぶtargetはwindow/context/viewport identityを固定する。closeやswitch後に
別の最新窓へ付け替えない。stale targetは明示的な失敗として返す。
既存`test_script::PendingAction`はviewport指定がないため、そのままでは最初のconsumerが
別窓の操作を消費し得る。test-script専用のconsume/peekにviewport照合を追加する必要がある。
通常キー操作のKeyAction/keymapを迂回する新しい操作実装は作らない。

独立レビューで加えた条件:

- 既存の`Fs*` scriptを壊すため、未選択targetを一律ROOTにはしない。暗黙の現在targetと
  明示選択を型で区別し、どちらも操作受付時にownerを固定する。ROOTは明示targetとして扱う。
- 同じviewportに別contextが移る場合がある。window/context/viewport incarnationを
  pending操作へ保持し、入力consumerの直前にも現在ownerと照合する。
- `windows()`はread-only。passive窓を操作する選択にactivationが必要なら、既存の
  activation handlerを通す。テスト専用のmount/session書換えは加えない。
- 現`target_rendered`は入力callback到達でありpaint証明ではない。current context・
  items generation・pageに結び付くtexture readyとpaintを観測し、旧ページの描画済み値を
  新ページへ引き継がない。ROOT/childのrevisionを区別する。
- `draw_fs_image`には旧ページの`Captured` holdoverも入る。mounted currentのpage/genを
  そのまま付けず、S1では`CurrentItem`の実texture描画だけを現ページの証拠にする。
- deferred callbackは登録時の固定viewではなく、実行時の`shared.view()`を描く。
  証拠は元snapshotのtexture選択時に作り、画像と同じview payloadとしてcopy/updateする。
  登録時ownerと実行時画像を別々に取得して結合しない。
- passive snapshotはthumbnail fallbackを許す。後からfs_cacheがfull readyになっても、
  既存snapshotのtextureをfullと扱わない。画質の由来もtexture取得時に保持する。

S1は、S1a（read-only window snapshot・paint-commandの証拠）と、S1b（対象固定・
既存activation・PendingAction配送・複数窓PDFシナリオ）へ分割する。
S1aはsingle/paged PDFを対象にし、S1bのfixture設定もsingle-pageを明示する。
paint-command発行はGPU scanout成功の証明ではない。

最低限の回帰は、兄弟/ROOTのconsume/peekで対象操作が失われないこと、対象だけが一度消費、
close/binding移譲後は拒否、snapshot前後でmounted owner・queue・generation不変を確認する。

PDF fixtureは既存`page-turn/generate_pdf_fixture.py`で使い捨て領域に2文書を作る。
設定overrideで複数窓モードを選び、ROOTからそれぞれ開く。異なる2つのwindow/contextと
各PDFページの描画、片方への操作で兄弟状態が変わらないことを確認する。

## S2: egui pointerと名前付き矩形

既存の合成キーと同じviewport指定・timelineへ、move/button/wheelを載せる。
矩形は実際の描画ownerがcontext/viewport・frame/revision・pixels-per-pointとともに公開し、
scriptは領域名と正規化位置を指定する。テスト用に別レイアウトを再構築しない。
press、drag閾値を超すmove、継続move、最終moveとreleaseを分け、対象frameでの処理を待つ。
待つのはscript workerであり、UI threadにblocking waitを追加しない。
LTR/RTL、列中心とページ着地の区別、release時だけ動いた最終位置を検証する。

## S3: native入力の継ぎ目

`NativeVideoOutputEvent`の直接注入はpresenterの当たり判定を飛ばすため使わない。
render routeだけの注入も、pumpのcursor ownership/activityを飛ばすため不十分。
test専用commandをpump ownerへ非同期に送り、対象hostの既存
`NativeVideoWindowEventSink::send`からpump/render両routeへ流す。
通常のgeneration検査より手前に入り、targetにはwindow/contextとsource session・placement
generationを固定する。古いtargetへ最新generationを付け直さない。

native矩形は描画ownerが公開する。payloadの座標はclient physical pxであり、pointsとの
変換はownerのpixels-per-pointで一度だけ行う。ctrl/shiftは同じscript timelineから渡す。
MouseMoveはcoalesceされるため、一括投入の成功をドラッグ成立とみなさず、pump処理とrenderの
acknowledgmentを区別する。pump/render/UI threadはackをblocking waitしない。

独立レビューで確認した検証限界: HUD wndprocにはpresenterと異なるMouseLeave、capture、
held-buttons、focus claimがある。typed sinkはUSER32の物理配送、HUD region、SetCapture成否、
実HWNDのfocus/z-order、GPU scanoutを検証しない。touchも今回のmouse seamに含めない。
これらを成功や環境不成立へ読み替えず、未検証の範囲として残す。

## 検証記録

S0はscript実装と静的検証まで進行中。実artifactのbuild・起動は未実施。
S1～S3は設計段階であり、複数窓PDF・列drag・動画zoomを自動化済みとは扱わない。
実行結果と到達した経路は段階ごとに作業台帳へ記録する。

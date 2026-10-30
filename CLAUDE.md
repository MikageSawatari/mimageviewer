# mimageviewer - Project Context

## 応答言語

**ユーザーへの応答は常に日本語で書くこと。** レビュー結果・調査報告・完了報告・確認質問など、
すべてのユーザー向けテキストは日本語にする。英語に切り替えない (技術用語・コード識別子・
コマンド・ログ抜粋など、そのまま示すべき固有部分は原文のままでよい)。コミットメッセージや
コード内コメントは従来どおり (本書の各節の方針に従う)。

5ch 等の利用者報告への調査・返信は、[AGENTS.md の User Reports And Forum Replies](AGENTS.md#user-reports-and-forum-replies)
に従う。初報から最新までの経緯・回答済み事項を照合し、報告者へ追加の作業を頼む前に
コード・既存ログ・開発側で可能な検証を進める。「とりあえず試してもらう」を繰り返さず、
追加確認は調査方針を実際に変えるものに絞り、相手の負担に最大限配慮する。

## 開発・公開担当とレビューの既定

モデル分担と調整の正本は [AGENTS.md の Model Roles And Coordination](AGENTS.md#model-roles-and-coordination)。
新規の開発タスク、途中の担当変更、独立レビューではこの節を読む。
旧ClaudeCode/Codex担当名は同節の役割へ読み替え、過去の合意・検証記録は保持する。
**2026-09-23決定 (Codex のモデルは 2026-09-30 更新): 管理・設計・作業指示・検収は ClaudeCode Opus 5.5 / high、
実装・テストは Codex GPT-6.1 Sol / high、独立レビューは実装者とは別の GPT-6.1 Sol / xhigh。**
(2026-09-30 より前は実装・レビューとも GPT-6 Sol / xhigh。GPT-6.1 Sol は high で GPT-6 Sol の最高設定を上回るため実装は high、
レビューは見落としの手戻りが高くつくので 1 段上の xhigh。利用者決定。)
Opusは指摘と検証結果を確認して検収する。Opus自身の確認だけで独立レビューを代替しない。
重要な所有境界・非同期・複数ウィンドウの設計は、実装前にもSolの独立レビューを受ける。
実装者は指示の主要な前提をコードで確認し、矛盾があれば設計担当へ返す。
難しい構造設計、原因未解明の不具合、設計担当とレビュー担当の重要な見解相違に限り、
GPT-6 Astra / mediumへ論点を絞った追加確認を依頼する。常設の親役や毎回の追加レビューにはしない。
Opusのhigh超・Astraのmedium超への自動昇格、Fableへの自動切替は行わない。
公開準備・公開作業は引き続き **ClaudeCode Opus 5.5 / high** が主導する (担当範囲は2026-09-09決定を継続)。
公開時は [リリース手順チェックリスト](docs/release-checklist.md) と [担当・引き継ぎ](docs/release-operations.md#開発と公開の担当・引き継ぎ) に従う。
明示された別の担当指定を優先する。文書だけではGUIのモデル設定や起動済みの担当は変わらない。
進行中の担当は安全な区切りで変更し、対象差分・判断根拠・未解決指摘・有効な検証結果を引き継ぐ。
Codexへの実装委任とレビュー依頼は別の担当・会話で扱い、レビューは指示への適合だけでなく設計自体を確認する。
検証の集約・再実行条件は [開発ビルドとテスト](docs/development-build-and-test.md#検証の担当と結果の再利用) を参照。

## 作業開始時に必読

**修正作業を始める前に、必ず `docs/README.md` から関連する設計ドキュメントを開いて全体像を把握すること。**

このプロジェクトはサムネイル / フルスクリーン / 仮想フォルダ (ZIP/PDF) / 補正プリセット / AI /
消しゴムなど、複数のサブシステムが絡み合っている。片側だけ修正すると逆側で表示が崩れる、
補正結果が一瞬で消える、ZIP/PDF だけ動かない、といった手戻りが頻発する。

最低限、以下の 2 本はどんな修正でも目を通す:

- [docs/architecture-overview.md](docs/architecture-overview.md) — 全体のレイヤー構造・モジュールマップ
- [docs/README.md](docs/README.md) — ドキュメント索引 (領域別に何を読むべきか書いてある)

修正対象の領域に応じて追加で読むべきドキュメント:

| 触る領域 | 読むドキュメント |
| --- | --- |
| サムネイル / フルスクリーン描画 / 回転 / 表示変換 | [docs/display-pipeline.md](docs/display-pipeline.md) |
| **動画 HUD・音量・seek bar・動画フルスクリーン UI** | [docs/video-architecture.md](docs/video-architecture.md) (特に native presenter の節)。**`ui_fullscreen.rs` ではなく `src/video/native_presenter/` を見ること** |
| ワーカー追加・キャッシュ・キャンセル処理 | [docs/async-architecture.md](docs/async-architecture.md) |
| UI スレッドから新しい同期 I/O / GPU アップロード / read_dir 走査を呼ぶ | [docs/ui-responsiveness.md](docs/ui-responsiveness.md) (§4 チェックリスト) |
| ZIP / PDF 対応が必要な機能 | [docs/virtual-folders.md](docs/virtual-folders.md) |
| **リモート閲覧 (mIV Remote / `mimageviewer-remote.exe` / Web UI)** | [docs/web-remote-plan.md](docs/web-remote-plan.md) が正本。動画は [docs/web-remote-video-streaming-plan.md](docs/web-remote-video-streaming-plan.md)、AI は [docs/web-remote-ai-plan.md](docs/web-remote-ai-plan.md)。**本体の表示状態を触る修正はリモート側の所有権と IPC 版に波及する** |
| ファイル名 prefix スタック（同接頭辞の画像を畳む集約表示・フラット読書・Shift+↓↑ ジャンプ） | [docs/filename-stack-plan.md](docs/filename-stack-plan.md) |
| 補正 / プリセット / AI アップスケール / 消しゴム | [docs/preset-and-adjustment.md](docs/preset-and-adjustment.md) |
| 補正レイヤー / ローカル調整 / レイヤー合成 | [docs/local-adjustment-layer-v1.1.0-plan.md](docs/local-adjustment-layer-v1.1.0-plan.md) と [docs/local-adjust-filter-candidates.md](docs/local-adjust-filter-candidates.md) |
| キーボード操作 / ショートカット / `consume_key` / `key_pressed` / native VK 判定 | [docs/keymap-spec.md](docs/keymap-spec.md) と [docs/key-customization-impl-plan.md](docs/key-customization-impl-plan.md)。新しいキー操作は原則 `KeyAction` + keymap helper 経由にする |
| UI の見た目・配色を変える修正 | [docs/ui-snapshot-policy.md](docs/ui-snapshot-policy.md) (egui_kittest スナップショットの更新手順) |
| **detached viewer / F12 別ウィンドウ / 複数ウィンドウ** | [docs/detached-rework-plan.md](docs/detached-rework-plan.md) ⚠️ **リワーク中につき凍結ルールあり** (下の「Detached viewer リワーク中のルール」参照) |

**設計を変えたら該当ドキュメントも同時に更新する** (下の「公開文書の必須規則」と [docs/public-wording-policies.md](docs/public-wording-policies.md) の「コード修正時のドキュメント同時更新」参照)。

## 作業の種類ごとに読む文書 (2026-10-15 に CLAUDE.md から分けた)

この CLAUDE.md には、常に守る規則とその要約だけを置いている。手順・経緯・参照情報は次の文書にあり、
**該当する作業を始めるときに全部読む**。要約だけで判断しない。

| 作業 | 読む文書 |
| --- | --- |
| **リリース作業 (Phase 0〜6、配布ビルド、公開、配布チャネル)** | [docs/release-checklist.md](docs/release-checklist.md) を最初に全部読む。補助は [docs/release-operations.md](docs/release-operations.md) |
| UI / ダイアログ / パネル / IME / スクロール / 配色 / 並行処理 / UI スレッドの I/O / UI 文言のグリフ / 外部ツールへ渡すテキスト | [docs/coding-conventions.md](docs/coding-conventions.md) |
| vendor/ の復旧、PDFium・ONNX Runtime・Susie・FFmpeg・LibRaw・VST3 bridge の更新やビルド | [docs/native-dependencies.md](docs/native-dependencies.md) |
| Codex への実装委任・レビュー依頼 | [docs/codex-cli-usage.md](docs/codex-cli-usage.md) |
| Windows ネイティブ挙動の実機確認を頼む、検証用バイナリを作る | [docs/development-build-and-test-native.md](docs/development-build-and-test-native.md) |
| コミット・ブランチ・worktree の作成や撤去・整形 | [docs/git-workflow.md](docs/git-workflow.md) |
| マニュアル・製品ページ・公開文書を書く、機能追加に伴う文書更新 | [docs/public-wording-policies.md](docs/public-wording-policies.md) |
| タグ / レーティングの保存、XMP 書き込み | [docs/tag-storage-and-xmp.md](docs/tag-storage-and-xmp.md) |
| mIV Remote の不具合調査 | [docs/remote-debugging.md](docs/remote-debugging.md) |
| 技術スタック・ファイル構成・対応形式・スクリーンショット素材 | [docs/project-reference.md](docs/project-reference.md) |

## Detached viewer リワーク中のルール (2026-07-05〜)

detached viewer (F12 別ウィンドウ / 複数ウィンドウ) は構造リワーク中
(正本: [docs/detached-rework-plan.md](docs/detached-rework-plan.md)、実装 = Codex /
検収 = ClaudeCode)。リワーク完了までの間:

- **detached 周りへの症状パッチを新規に入れない**。根本原因に対応しない guard / delay /
  retry / 追加 repaint / 一括 reset / silent fallback がこれに当たる (下の
  「バグ修正の一般原則」と同じ基準)。バグを見つけたら、プラン §2 (憲法) に従い BA 番号
  (壊れた前提の分類) に対応付けて報告する。緊急の応急処置が必要な場合はプラン §8 の形式
  (stopgap 明記 + 撤去予定ステージ記載) でのみ許可。
- **構造的に正しい修正、および他機能の修正が detached 述語 / viewport 経路に触れる場合は、
  次の条件で実施してよい**。禁止しているのは「触ること」ではなく症状パッチである。
  1. 着手前にプラン §2 (憲法) を読む
  2. **ClaudeCode と Codex の双方のレビューで「これは症状パッチではなく構造的修正である」
     ことに合意する** (単なる承認ではなく、この主張への合意を取る)
  3. 触れた範囲と判断理由を [docs/detached-rework-plan.md](docs/detached-rework-plan.md) に
     記録し、リワーク側が後で整合を取れるようにする
- リワーク作業自体は `detached-rework` ブランチで行い、他セッションと並行して
  detached を触らない。

## バグ修正の一般原則

- 修正前に、観測された失敗、守るべき不変条件、違反を作ったコード経路をログ・テスト・
  source inspection で特定する。症状を消す guard、delay、retry、追加 repaint、一括 reset、
  silent fallback を根本原因の代わりに追加しない。
- 共有状態、複数の入力入口、非同期完了、複数 viewer context をまたぐ問題では、再現した
  経路だけで完了しない。同じ状態の producer / consumer と open / switch / close / cancel /
  error lifecycle を列挙し、同型の入口・終了経路も確認してから修正する。
- 相互排他的な状態を複数の bool / `Option` / pending、または field の有無を sentinel として
  表現している場合、新しい分岐を足さず、単一の typed request / state owner / router への
  集約を検討する。正しい構造修正が現在の範囲を超えるなら、症状パッチを入れずに報告する。
- items、cache、texture、queue、channel、generation、cancel token、worker など context 固有の
  resource は、create / mutate / drain / cancel / invalidation / drop が所有 context だけに作用する
  ことを確認する。読み取り専用の open / close は、変更時用の失効通知や別 context の再ロードを
  発生させてはならない。
- 状態バグには状態遷移テスト、入力 routing バグには handler-level test、非同期・multi-window
  lifecycle には request 相関と sibling context 不変を検証するテストまたはログ検査を追加する。
  v2.7.0 出荷前の具体的な横断監査は
  [docs/archive/review-v2.7.0/systemic-review-plan.md](docs/archive/review-v2.7.0/systemic-review-plan.md) を正本とする。

キー操作を追加・変更するときは、ユーザーが明示していなくても keymap 対応を検討する。
閲覧・編集・動画の通常ショートカットは `KeyAction` に追加し、`ini_name()` / `context()` /
`trigger()` / `default_chords()` / `ALL_ACTIONS` / 呼び出し側 helper / `docs/keymap.ini.default` を
揃える。IME 確定、OS clipboard、D&D、右クリック、マウス、ゲームパッドなど固定扱いにする
入力は、固定である理由を `docs/keymap-spec.md` に残す。

## 設計の簡素化: 状態の組み合わせを減らす (2026-09-28 利用者指示)

設計を決めるとき、非同期の割り込みや状態の組み合わせを**全部扱う前に、組み合わせそのものを
減らせないか**を必ず検討する。扱う組み合わせを減らすほうが、正しく扱い切るより設計も実装も
単純で、見落としも起きにくい。検討した結果 (採用した / しなかった理由) を設計書に書く。

- **長い処理はモーダルにして割り込みを消せないか**。変換・保存・パスワード入力のように、
  途中で他の操作が入ると状態が食い違いやすい処理は、処理中はモーダルにして他の操作
  (開く・移動・履歴・別ウィンドウへの切り替えなど) を受け付けない形を先に検討する。
  「処理中に何が起きても元に戻せる」仕組みを作るのは、モーダルにできない理由があるときだけ。
- **設定変更や状態の切り替えで、表示中のウィンドウ・パネルを閉じて作り直せないか**。
  表示中のものを変更に追従させる処理を足す前に、閉じる・一覧を読み直すなど既存の単純な
  経路に乗せられないかを見る。専用の作り直し処理は、既存経路で済まない理由があるときだけ。
- ほかにも、同じ効果で状態を減らせる手 (1 つの所有者にまとめる、確定の時点を 1 つにする、
  既存機能と同じ経路・同じタイミングに揃える) がないかを考える。
- 経緯: 2026-09-27〜28、フォルダ履歴と EPUB 変換の組み合わせで、「変換中に別の本を開く」
  「直接開く途中に履歴で移動する」などの割り込みを元に戻す仕組みで扱おうとし、独立レビューの
  たびに別の組み合わせで P1 が出続けた。利用者の指摘で「変換中はモーダル」にしたところ、
  扱う組み合わせが大きく減った。また設定変更時に表示中の一覧を EPUB 専用に作り直す処理が
  直すたびに別の問題を出し、既存の RAR 設定と同じ経路に揃えて解消した。

### まれな失敗は既定で割り切る (2026-10-02 利用者指示、2026-10-10 既定化)

ディスク書き込み失敗・ディスクフル・DB の異常・処理途中の mIV 強制終了など、**通常の使い方では
まず起きない失敗**は、**利用者に相談せず既定で割り切る**。再試行・途中状態の保持・失敗の
種類分け・多段の回復手順・完了待ちのための待機などの仕組みを、設計にも実装にも足さない。
利用者は「再起動してください」「次回起動時に作り直します」「その操作はやり直してください」の
ような**運用上の割り切り**を望んでいる。個人開発では、まれな失敗を実機で再現して確かめること
自体が難しく、検証できない回復処理はかえって不具合の元になる。

- 割り切り方: **黙って壊れたままにしない**。最低限、ログと利用者への通知を出し、可能なら
  「次回起動で作り直す」印のような単純な復旧の入口を 1 つ置く。索引・キャッシュなど作り直せる
  データでは、作り直しを復旧手段にするのが基本。保存済みのデータを失敗を理由に勝手に消したり
  作り直したりしない (読めないなら原本を残して、その機能の書き込みを止める)。
- **相談するのは次の 2 つだけ**:
  1. 利用者が作ったデータ (設定・タグ・コレクション・本の構成・しおり等) が**失われる・壊れる**
     場合。割り切り案で何が失われるかを明示して判断をもらう。
  2. まれな失敗ではなく、**通常の操作でも起こり得る**場合 (例: 保存完了前に閉じてすぐ開き直す、
     連続操作の順序)。これは割り切りの対象外で、構造で正しく扱う。「まれ」の判定に迷ったら相談する。
- 設計書・実装 brief・レビュー依頼にこの方針を書く。設計書には「まれな失敗は割り切り
  (ログ + 通知 [+ 作り直し])」と 1 行で書けばよく、失敗ごとの回復設計や利用者向けの質問は作らない。
- **独立レビュー・実装担当の「まれな失敗の組み合わせ」の指摘は、仕組みで塞がない**。上の割り切りで
  閉じ、指摘への回答に「既定の割り切り方針による」と書く。指摘が上の相談対象 1・2 に当たる場合だけ、
  構造で直すか利用者に相談する。
- **「例外への対処の中の例外」は作らない**。回復処理そのものが失敗した場合の対処、その対処が
  終了をまたいだ場合の対処…という段の重なりは、方針違反として設計から外す。
- 経緯: 2026-10-02、起動時索引スキャン軽減の設計で、お気に入りの構成変更中の書き込み失敗に
  対して、設計レビューの指摘を受けるたびに「書き込みを済ませて止める停止」「失敗の種類分け」
  「自動再試行 (5 秒・30 秒・5 分)」「削除前の無効化」を足し、さらに再試行中の失敗が終了を
  またぐ場合…と質問が続いた。利用者の判断で「失敗したら次回起動時に索引を作り直す + 通知」に
  割り切り、これらをすべて外した。2026-10-10、§1.364 の設計でも同種の質問を毎回出していたため、
  利用者の指示で「毎回相談する」から「既定で割り切る」に改めた。

## 通常の使い方が不便になる変更は、着手前に利用者へ相談する

新機能の実装や不具合修正のために、**今の通常の使い方が遅くなる・手間が増える・できなくなる**
変更 (リリース済みの挙動の退行) が必要になりそうなときは、**実装に着手する前に**利用者へ
設計を相談する。何が不便になるか、避ける案とその費用を並べて示し、判断をもらってから進める。

- 実装担当・レビュー担当への指示 (brief) でも、既存の挙動を削る選択肢を設計担当が勝手に
  優先しない。指示を出す前に、その選択で失われる既存の挙動がないかをコードで確かめる。
- 実装中・レビュー中に退行が見つかったら、黙って進めず止めて相談する。
- 経緯: 2026-09-27、不具合修正の指示で「開き終わるまで表示を変えない」案を優先させた結果、
  キャッシュ済み PDF をすぐ表示するリリース済みの挙動を削ってしまった。指示の時点で、
  その近道があることを確かめていなかった。

## 観測の出どころを落とさない (対外文書・バックログ・報告)

**エージェントは製品バイナリを起動しない** (下の「実機検証と設定データの保護」と [docs/development-build-and-test-native.md](docs/development-build-and-test-native.md))。
したがって **実行時の挙動について、エージェント自身の観測は存在しない**。
ここから次が従う。心がけではなく、持っていない観測を主張しないという構造上の帰結。

- **「再現しました」「確認しました」「動作を確認」を一人称で書かない。**
  主語がエージェントになる位置でこれらの語を使わない。
- **実行時の挙動に触れる文には、観測者を必ず書く。** 「利用者が再現」「<報告者> の報告」
  「perf ログから」「未確認」。**観測者を書けない文は、書けない文**として扱う。
- **未回答の確認質問がある事実は、成果物に断定として書かない。** 質問を出した時点で
  その事実は「回答待ち」であって前提ではない。**下書きには答えではなく質問を残す。**
- 対外メール・バックログ・完了報告は、書き終えた後に**事実文ごとに出どころを 1 回たどる**。
  たどれるのは (a) コードの参照、(b) 観測者名のある観測、(c) 明示の未確認 の 3 つだけ。

伝聞は restatement のたびに保留と出どころを落とす。落ちるのは対外文書の下書きで、
確信のある文体に但し書きが馴染まないため。**検査はその境界に置く。**

経緯: 2026-09 に 2 度、実機で一度も動かしていない症状へ「再現しました」と書いた。
2 度目は**曖昧さに気づいて確認質問を出した後、その回答を待たずに下書きを書き**、
下書きの中で伝聞が断定へ変わっていた。コードに関する主張は同じ期間ずっと持ちこたえて
いたので、不足していたのは「コードが何をするか」の規律ではなく「誰が何を見たか」の規律。

## Overview

A Windows 11 native image viewer built in Rust. Inspired by ViX (legacy 32-bit viewer),
modernized with GPU acceleration and AI upscaling. Single-window design replacing ViX's
dual-window approach.

Rust (edition 2024、MSVC) + eframe / egui 0.37 (wgpu 30)。画像は `image` crate・TurboJPEG・WIC・LibRaw、
キャッシュは SQLite + WebP、動画は FFmpeg LGPL shared DLL (launcher が内包して展開)、PDF は PDFium の
マルチプロセスワーカー、AI は ONNX Runtime (DirectML / TensorRT)。詳細とファイル構成は
[docs/project-reference.md](docs/project-reference.md)。

## コーディングの必須規則 (詳細: [docs/coding-conventions.md](docs/coding-conventions.md))

- **一覧スクロール**: オフセットは App が自前管理し、常に `cell_size` の整数倍。touch drag の 1 行未満の端数は
  一時データに分け、canonical offset へ混ぜない。
- **popup / menu の wheel**: popup 内に `ScrollArea` を置くなら、開いている frame で wheel を明示的に消費する。
  `App::process_scroll` はメニュー〜ファセットバーの描画後、グリッド描画の直前に呼ぶ。
- **配色**: `os_theme` の helper (`apply_resolved_with_contrast` / `apply_dark_ui` / `dark_popup_style` /
  `dark_menu_popup` / `with_dark_context_style`) を使う。文字色は `ui.visuals()` の値を使い固定 gray を足さない。
  `ctx.set_theme` / `ctx.set_visuals` を直接呼ばない。
- **ダイアログ**: `anchor()` を使わず `default_pos()`。モーダル相当は `App::common_modal_dialog_open` に
  「現在表示されるか」を返す helper で登録する。一覧の ScrollArea は `.auto_shrink([false, ...])`。
  長文の折り返しでは floating scrollbar の幅を確保する。
- **起動時ダイアログ**: 1093×614 logical points でも確定 / 閉じる操作が全部見えること。見出しと操作行は
  ScrollArea の外。小画面・UI 倍率の回帰検査を残す。
- **固定パネル (Area + Frame::popup + ScrollArea)**: 親領域を `allocate_ui_with_layout` で明示確保し、ヘッダは
  ScrollArea の外。高さ計算は spacing / padding / margin を含め、`tests/support/scroll_layout.rs` で検査する。
- **IME**: TextEdit を含むダイアログの Enter / Escape は `self.dialog_enter_pressed(ctx)` /
  `self.dialog_escape_pressed(ctx)` (closure の前でローカル変数に取る)。日本語を入力する single-line TextEdit は
  `crate::ime_focus` の helper 経由 (raw TextEdit は allowlist テストで禁止)。新しい viewport の closure 先頭で
  `self.update_ime_state(ctx)` を呼ぶ。
- **並行処理**: `try_lock` 失敗 → sleep → 再試行のループを書かない。`Mutex + Condvar` の優先度キュー +
  ディスパッチャーにする (`src/pdf_loader.rs` の `PdfWorkerPool` が実例)。
- **UI スレッドで同期 I/O をしない**: ファイル全体の読み込み、`read_dir` + `Path::is_dir()` (代わりに
  `entry.file_type()`)、DB の cold open、DFS 走査は worker 化する。GPU アップロード (`ctx.load_texture`) は
  `fs_upload_backlog` で 1 フレーム 1 枚に制限し、現在ページだけ即時にする。
  新機能では [docs/ui-responsiveness.md](docs/ui-responsiveness.md) §4 のチェックリストを通し、区間に
  `perf::event` を入れる。レビューでも UI スレッドから同期到達しないかを呼び出し元まで追う。
- **perf-log の解析を頼まれたら** `%APPDATA%\mimageviewer\logs\` の最新 `perf_*.jsonl` を
  `python scripts/analyze_perf.py <path> {startup,nav,hitches}` に渡す。パスを毎回聞かない。
- **UI 文言のグリフ**: 固定文言に絵文字や環境依存の記号を新しく使わない (`✕` → `×` など)。UI 文字列を
  変えたら `python scripts/check_ui_glyphs.py` を回す。
- **外部ツール (Codex GUI 等) へ渡す Markdown** は `python scripts/write_utf8_bom.py <file>` で BOM を付ける。
  リポジトリ内の通常の文書は BOM なし。`.ps1` は ASCII のみ。

## ネイティブ依存物の必須規則 (詳細: [docs/native-dependencies.md](docs/native-dependencies.md))

- `vendor/` は git 管理外。不足は `cargo build` のエラーが復旧手順付きで示す。エージェントは
  `bash scripts/bootstrap-vendor.sh` で揃える。`vendor/models/` と `vendor/vst3-host/` は再取得できないので、
  追加・再ビルドしたら `C:\home\mimageviewer_vendor_backup\` へ実ファイルでコピーし直す。
- リリースビルドは core → remote → EPUB worker → launcher の順 (`scripts/build-release.ps1` が行う)。
  launcher は `-p mimageviewer-launcher --bin mimageviewer` が必要。配布物は必ず `scripts/build-dist.ps1`。
- FFmpeg は **LGPL shared build だけを使い、GPL build を絶対に使わない**。DLL のファイル名を変えない。
  ビルドには `LIBCLANG_PATH` が必要。
- ONNX Runtime DLL は `ort` の選択 C API 版を満たすものを使う (ort 2.0.0-rc.15、既定feature無効 + `api-24` ↔ DirectML 1.24.4 / TensorRT pack GPU 1.24.2)。
- VST3 host bridge は、**古い別 worktree の exe を流用しない** (IPC がずれて動画再生が激重になる)。
  C++ ソースを変えたら CMake で再ビルドする。

## UI スナップショットテスト

`tests/ui_snapshot.rs` + `tests/snapshots/*.png` に、`egui_kittest` を使った
UI 回帰テストを置いている。配色・レイアウトを変える修正を入れた際の
「意図しない見た目変化」を `cargo test` 段階で検知するのが目的。

```bash
cargo test --test ui_snapshot              # 既存スナップショットと比較
UPDATE_SNAPSHOTS=1 cargo test --test ui_snapshot  # 意図的な見た目変更の反映
```

更新後は `tests/snapshots/*.png` を目視で確認してから PNG とコード変更を
同時にコミットすること。詳細な設計方針・新規テスト追加手順は
[docs/ui-snapshot-policy.md](docs/ui-snapshot-policy.md) を参照。

## 利用者へ渡すコマンドは PowerShell で書く ⚠️

**利用者は常に PowerShell を使う。`bash ...` を渡さない。** PATH に bash が無いので
`用語 'bash' は ... 認識されません` で止まり、毎回シェルの話に往復が発生する
(2026-08-30 のリリース作業で実際に発生)。

- 実行を頼む手順・チェックリスト・最終回答のコード塊は `.\scripts\foo.ps1` 形式にする
- エージェント自身が Bash tool で `.sh` を回すのは構わない (setup スクリプト等)。
  **渡すときだけ** PowerShell 版に置き換える
- PowerShell 版が無い手順を渡す必要が出たら、その場で `.ps1` を追加する。
  「bash.exe をフルパスで叩いてください」で回避しない
- 新しい `.ps1` は **ASCII オンリー**で書く。PowerShell 5.1 は BOM 無し UTF-8 を
  ANSI コードページとして読むので、日本語を入れると文字化けする
  (既存の `check-idle-health.ps1` / `build-dev.ps1` も ASCII のみ)。
  日本語の説明は Markdown 側に置く

## 開発中のビルド・テスト選択

mIV は Windows 専用。非 Windows でのコンパイル検査は行わず、2026-09-29 に廃止した ubuntu の
`cargo check` CI job を復活させない。

通常の編集ループで毎回 `cargo test --workspace` を実行しない。まず変更範囲に応じて
次の最小targetを使い、共有境界を変更したときや最終確認時だけ範囲を広げる。

```powershell
cargo check -p mimageviewer --bin mimageviewer-core
.\scripts\test-focused.ps1          # 変更ファイルから選んだテストを nextest で実行
cargo nextest run -p mimageviewer --lib -E 'test(/<filter>/)'
cargo test -p mimageviewer --test <integration-test-name>
.\scripts\build-dev.ps1
```

**本体 lib の全件は `.\scripts\test-lib-full.ps1` 経由で実行し、filter なしの
`cargo test -p mimageviewer --lib` を直接打たない** (2026-10-16)。cargo-nextest でテストを 1 件ずつ
別プロセスに分けるので約 6〜7 分で終わり (cargo test では App テストが共有ロックで直列になり約 28 分)、
同じソース・環境・DLL の状態で成功済みなら記録を返して再実行しない。レビュー担当も、同じ状態の
成功記録があれば全件を再実行しない。`test-focused.ps1` が対応先の無い変更 (スクリプト・Cargo.lock 等) を
見つけて exit 2 を返したら、`test-lib-full.ps1` に回す。cargo-nextest が無い環境では
`cargo install cargo-nextest --locked` で入れる。経緯と計測は
[docs/test-speed-report.md](docs/test-speed-report.md)。

全workspace・統合・doc・pack builderテストは `.\scripts\test-full.ps1` に集約する
(2026-10-16 から nextest で実行し、doctest だけ `cargo test --doc`。約 8 分、以前の cargo test では約 45〜51 分)。
これは共有基盤を横断する変更の最終確認、リリース前、または明示的に全体確認を求められた
場合に実行する。機能追加中の試行錯誤ごとには実行しない。詳細と判断表は
[docs/development-build-and-test.md](docs/development-build-and-test.md) を参照。

⚠️ **`test-full.ps1` は `target\release\mimageviewer-core.exe` と
`target\release\mimageviewer-remote.exe`、`target\release\mimageviewer-epub-pdf.exe` が既に在ることを前提にする**。`--workspace` が
launcher package を含み、その build.rs が内包対象 exe の存在を検査するため、いずれかが
無いと**テストが 1 件も走らないままビルドエラーで落ちる**。新しい clone や
`cargo clean` 直後は、先に 3 本を作ってから走らせる (どちらでもよい):

```powershell
.\scripts\build-release.ps1     # core -> remote -> EPUB worker -> launcher を正しい順で作る
cargo build --release --bin mimageviewer-core
cargo build --release -p mimageviewer-remote --bin mimageviewer-remote --features embedded-web-assets
cargo build --release -p epub-pdf-worker --bin mimageviewer-epub-pdf
```

`build-dist.ps1` は test-full を `cargo clean` の**前**に回すので、直前のビルド成果物が
残っている通常のリリース機では問題にならない。

### 修正完了時の実機確認用バイナリ

ユーザーから指示されたアプリ機能・実行時挙動の修正が完了し、関連する自動テストが通ったら、
最終回答の前に原則として次を実行する。

```powershell
.\scripts\build-dev.ps1
```

エージェントは成果物を起動しない。最終回答には、リポジトリルートからユーザーが実行する
次のコマンドと、今回確認してほしい具体的な操作シナリオを記載する。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

このバイナリはビルド時間だけを短縮する `dev-runtime` Cargo profileで、`portable` featureは
使わない。引数なしでは通常版と同じ実利用中の `%APPDATA%\mimageviewer` を開くため、設定・
キャッシュ・ログを更新し得ることを最終回答で警告する。エージェント自身は起動しない。
single-instance mutexも通常版と共有するため、起動前にインストール版／常駐tray版を終了して
もらう。
隔離確認が明示的に必要な場合だけ、同じバイナリへ
`--data-dir .\target\dev-runtime\data` を渡す（build flavorは切り替えない）。

ドキュメントのみ、テストのみ、build scriptのみなど実行アプリへ影響しない変更、ユーザーが
build不要と明示した場合、必要な依存物が無い場合は省略できる。通常なら必要なbuildを作れなかった
場合は、その理由を最終回答に明記する。

launcher、release-only cfg／最適化、exact release performance、埋め込みasset展開、変更した
VST3 bridge、署名、packagingなどrelease構成そのものを確認する変更では `build-dev.ps1` ではなく、
[docs/development-build-and-test-native.md](docs/development-build-and-test-native.md) に従って `build-release.ps1` を使う。動画native presenter、
フルスクリーン、動画→音声モードなどcore内のWindows native挙動は、上の通常profile
`build-dev.ps1` でユーザー実機確認できる。

## 実機検証と設定データの保護 (詳細: [docs/development-build-and-test-native.md](docs/development-build-and-test-native.md))

- **エージェントは** `target\dev-runtime\mimageviewer-core.exe`、`target\release\mimageviewer.exe`、
  `target\release\mimageviewer-core.exe`、インストール済み mImageViewer、`%APPDATA%\mimageviewer\runtime\*` を
  **起動しない**。通常 profile のビルドは起動だけで実利用中の `%APPDATA%\mimageviewer` の設定 DB を
  migration・保存し得る。ビルドして利用者へ渡すのはよい。
- エージェントが画面を操作するテストは `.\scripts\prepare-portable-smoke.ps1` で作った使い捨てコピー
  `target\portable-smoke\mimageviewer.exe` (データは `target\portable-smoke\data`) だけを使う。
  利用者の設定をコピーして使わない。テスト準備として通常設定を削除・リネーム・復元・初期化しない。
- ユニットテストで再現できない Windows ネイティブ挙動を変えたら、実機確認を頼む前に検証用バイナリを用意する
  (core 内の挙動は `.\scripts\build-dev.ps1`、release 構成に依存する挙動は `.\scripts\build-release.ps1`)。
  エージェントのツールから呼ぶときは `*>&1` を付けない。

## 永続データ・スキーマ変更時の判断 (リリース済み / 未リリース)

永続ストア (SQLite DB・設定ファイル・キャッシュ DB・ディスク上のデータ形式) の
スキーマや、そこに保存するデータの内容・意味を変える修正をするときは、
**その機能・データが「前回リリース以降に追加されたもの (= まだ出荷されていない)」か
「リリース済みバージョンに既に存在するもの」かを毎回確認してから進める**。
判断を曖昧にしたまま着手しない。

| 対象 | 方針 |
| --- | --- |
| **未リリース** (前回リリース以降に追加され、まだ出荷していない機能 / データ) | マイグレーション不要。破壊的変更も許容する (スキーマ作り直し・キャッシュ全削除・キー体系変更も OK)。ユーザーの手元に旧データが無いため。開発機にテストで溜まったデータは手動削除で足りる |
| **リリース済み** (既存バージョンに存在する機能 / データ) | マイグレーション必須。ユーザーの手元に旧形式データがあるので、黙って壊すと設定消失・キャッシュ全損・起動不能などの実害になる。スキーマ version gate / 旧形式の読み替え / 段階移行を必ず用意する |

判断材料 / 進め方:

- 前回リリースの範囲は **CHANGELOG.md の更新履歴** と **GitHub Releases** で確認する。
  どのバージョンで出した機能か不明なら git log / git tag で追う。
- コード上は完成して見えても「リリース済み」とは限らない。実装済みでも未出荷の
  サブシステムがあり得る。判断に迷ったら **ユーザーに確認する** (推測で決め打ちしない)。
- 「未リリースなのでマイグレーション不要」と判断したら、その旨をコミットメッセージに
  一言残す (後のレビューで「移行コード忘れ」と誤認されないように)。
- レビュー観点としても恒久的に有効: 永続ストアのスキーマ差分を見たら「対象はリリース
  済みか」を必ず確認し、リリース済みなら移行コード + 後方互換テストが揃っているかを見る。

## 公開文書の必須規則 (詳細: [docs/public-wording-policies.md](docs/public-wording-policies.md))

- **機能を追加・変更・削除したら** `htdocs/mimageviewer/manual/` と `htdocs/mimageviewer/index.html`、該当する
  設計文書 (spec・architecture-overview・display-pipeline・async-architecture・ui-responsiveness・virtual-folders・
  preset-and-adjustment・keymap) も同時に更新する。
- **通信・データ保存・権限に関わる機能を足したら**、製品ページの「安心して使えます」と `privacy.html` の
  「ネットワーク通信」「端末内に保存されるデータ」を両方確かめる。「〜だけ」「〜はありません」は機能追加で偽になる。
- **製品ページのカード**は見出し 1 つ + 本文 1 段落、本文 150 字まで。`python scripts/check_site_cards.py`。
- マニュアル・製品ページに**バージョン固有の説明と実装用語を書かない** (言い換え表は詳細文書)。
  版番号は製品ページのダウンロード欄のバッジ 1 か所に集約する。
- **外部ダウンローダの名前・URL・使用例**を公開文書・コミットメッセージ・設計文書に書かない。
- **モザイク関連**の UI・公開文書に、特定の画像投稿サイト名・基準名・基準への適合判定を書かない。
  モードの説明は評価語ではなく処理内容で書く。
- AI 生成画像のメタデータは標準仕様の用語 (PNG tEXt チャンク・EXIF UserComment 等) で説明する。
  生成ツール固有の形式名はパーサ内部に限り、UI に出さない。

## タグとリモート調査の要点

- **タグはファイルへ書かない** (正本は `tags.db`)。「mIV のタグは他ソフトと共有できる」と書かない。
  今もファイルへ XMP を書くのはレーティング (`write_rating_to_xmp`、既定 OFF) だけで、変更したら ExifTool で
  検証する。新しい書き込み経路は `XMP_WRITE_LOCK` を通す。旧 XMP タグの経路は復活させない。
  詳細: [docs/tag-storage-and-xmp.md](docs/tag-storage-and-xmp.md)
- **mIV Remote の不具合は、推測の前に** `%APPDATA%\mimageviewer-remote\remote-web-log.jsonl`
  (data_dir の兄弟 `<data_dir>-remote`、ポータブルは `<exe_dir>\data-remote\`) を開く。端末側の例外や遷移も
  ここに記録されている。詳細: [docs/remote-debugging.md](docs/remote-debugging.md)

<a id="リリース手順チェックリスト"></a>

## リリース作業

**担当: ClaudeCode Opus 5.5 / high。** リリース作業 (Phase 0〜6) を始めるときは、
[docs/release-checklist.md](docs/release-checklist.md) を**最初に全部読み、その手順に従う**。
過去の落とし穴と復旧手順は [docs/release-operations.md](docs/release-operations.md)。
用語: `mimageviewer.exe` (launcher) は「単体exe版」と呼び、「ポータブル版」は loose-deps の zip だけを指す。

## Codex の使い方の要点 (詳細: [docs/codex-cli-usage.md](docs/codex-cli-usage.md))

担当と粒度は [AGENTS.md](AGENTS.md#model-roles-and-coordination) が正本。

- **自分から呼ぶとき**: 一塊の作業が動いた時点 (完了報告の前) のレビュー、デバッグで仮説と検証を 2〜3 周しても
  詰まっているとき (3 周目の前に必ず)、設計の選択肢で迷うとき。重要な所有境界・非同期・複数ウィンドウの設計は
  実装前にも確認を取る。定型の公開作業には自発レビューは不要。
- **モデル**: レビューは `--model gpt-6.1-sol -c 'model_reasoning_effort="xhigh"' --sandbox read-only`、
  実装は同 model の `high` + `--sandbox workspace-write`。個人 config に依存しない。
- **同じタスク・同じ段階は `codex exec resume` で続ける** (resume には `--sandbox` が無い)。長さを理由に
  新しいセッションへ切り替えない。brief に「区切りで止めて報告してよい」と書かない。
- 最終回答は `-o <FILE>` で受け取り、`tail -N` で切らない。stdin は必ず制御する (`< /dev/null`、または `-` +
  ファイルからの入力)。
- 指摘は該当コードを読んで真偽を判断し、false positive は理由を添えて報告する。修正コミットには
  `Codex P<N> 対応` と書く。Codex が起動できなければ利用者に確認する。

## Git と整形の要点 (詳細: [docs/git-workflow.md](docs/git-workflow.md))

- **コミット前にワークスペース全体へ `cargo fmt`** (引数なし)。fmt したら同じセッションでコミットする。
  pre-commit フックが `cargo fmt --check` を番人にしている。
- セッション開始時に `git status` を見て、未コミット差分があれば取り込むか利用者に確認する。
- コミットは許可なしで区切りごとに小さく行う。**Codex 等へ実装を出す前にコミットする**。実機確認が要る
  ネイティブ機能は、確認前でもテストが緑ならブランチにコミットしてよい (未確認と書く)。push / PR は指示時だけ。
- 「コミットして」と言われたら、ローカル `master` へ merge まで行う (残す指示や conflict があれば止めて報告)。
  GitHub の既定は `main`、ローカルは `master`。
- **作業ツリーを他セッションと共有するとき**: コミットは `git commit [opts] -- <自分のパス>`。`git add -A` /
  `git add .` は使わない。HEAD が自分のコミットでなければ reset / rebase しない。コミット直後に
  `git log --oneline -1` を確かめる。想定外の変更は消さず、自分のコミットから外すだけにする。
- **`git worktree remove` を直接使わない。必ず `scripts/safe-worktree-remove.ps1`**。`vendor/`・`target/` などを
  worktree 間で junction / symlink 共有しない (撤去時に本体の実体が消える)。必要なら実体コピーする。

## User: Background

- Comfortable reading C++ but not familiar with Rust's borrow checker details
- Has RTX 4090, Windows 11
- AI-assisted development workflow: role defaults and independent AI review are defined in `AGENTS.md`; user performs approved/manual verification as required

## Claude Code tool call reliability rules（ツール呼び出しの生テキスト漏れ対策）

**症状**: ツール呼び出しの直前に `call` / `court` / `<invoke name="...">` /
`<parameter name="...">` のような生テキストが出て、ツールが実行されずに失敗する。

**根本原因 (2 段構え)**:
1. **制御トークン破損** — ツール呼び出しの開始タグが、サンプリング時に語彙的に隣接する
   別トークン (`call` / `court` 等) に化ける。ハーネスがパースできず生テキストとして漏れる。
2. **自己汚染 (in-context few-shot poisoning)** — 壊れた呼び出しが会話履歴に残ると、
   モデルがそれを「正しい例」と誤認し、以降の呼び出しでも決定論的に同じ壊れ方を繰り返す。
   → **セッション内でリトライを重ねても直らない。むしろ悪化する。**

### 発生確率を下げる (予防)

- **ツール呼び出しはメッセージの先頭・単独で出す**。同じメッセージ内で呼び出し前に説明文を
  書かない (説明はツール結果が返ってから書く)。
- **1 メッセージ 1 ツール呼び出し**。多数の Read/Edit/Bash を 1 応答にまとめて撃たない。
- **ツール引数は短く保つ**。段落級の長い引数は避ける。長い / 複雑なシェルコマンドは一旦
  スクリプトファイルに書いてから実行する。
- 大きな全文 Write より、小さな targeted Edit を優先する。
- **セッションを長く引きずらない**。複数日 resume や 1M トークン級の巨大文脈で発生率が上がる。
  区切りごとに git / メモに状態を保存し、必要なら新セッションに移る。
- マークアップ密度の高い大型 skill は途中ロードを避け、セッション冒頭で読み込む。
- 同時起動の MCP サーバや background Bash を増やしすぎない。

### 発生してしまったときの対応 (最重要)

- 生テキストが出たら、そのツール呼び出しは**実行されていない**とみなす。
- **リトライは最大 1〜2 回まで**。それで直らなければ**それ以上リトライしない** (履歴汚染が
  進み、決定論的に失敗し続けるため。粘るほど悪化する)。
- 直らない場合は**ユーザーに「新しいセッションを開始 (`/clear`) してほしい」と平文で伝える**。
  セッション内で確実に回復する手段は `/clear` (汚染履歴の切り離し) だけ。
- **`/compact` を回復手段にしない** (compact 直後に再発する)。フルな新セッションを選ぶ。
- 「もっと注意して」系の自己叱咤・追加指示は効かない (注意の問題ではなく文脈アンカーの問題)。
- 更新で直る保証はない (公式修正は未出荷の時期がある) が、Claude Code は最新版に保つ。
- (未確認の回避策) Opus 4.8 で頻発する場合、モデルを Opus 4.7 / Sonnet に切り替えると
  減るという報告がある。確実ではないので、まずは上記の `/clear` を優先する。

### 一般の後処理

- After any Write/Edit, verify with Read or git diff that the file actually changed.

## 移した節の行き先

他の文書やコメントが「CLAUDE.md の〜節」と書いていたら、この表で移動先を引く。

| 元の節 | 移動先 |
| --- | --- |
| Tech Stack | [docs/project-reference.md](docs/project-reference.md) |
| Project Structure | [docs/project-reference.md](docs/project-reference.md) |
| vendor/ 一括セットアップ (新規 clone / vendor/ 消失時の復旧) | [docs/native-dependencies.md](docs/native-dependencies.md) |
| Implementation Phases | [docs/project-reference.md](docs/project-reference.md) |
| Key Design Decisions | [docs/coding-conventions.md](docs/coding-conventions.md) |
| mIV Remote の不具合調査は、まずサーバ側ログを見る ⚠️ | [docs/remote-debugging.md](docs/remote-debugging.md) |
| Supported Image Formats | [docs/project-reference.md](docs/project-reference.md) |
| Performance Notes | [docs/project-reference.md](docs/project-reference.md) |
| Screenshot Workflow | [docs/project-reference.md](docs/project-reference.md) |
| PDFium 管理 | [docs/native-dependencies.md](docs/native-dependencies.md) |
| ONNX Runtime 管理 | [docs/native-dependencies.md](docs/native-dependencies.md) |
| Susie 32bit ワーカー管理 | [docs/native-dependencies.md](docs/native-dependencies.md) |
| FFmpeg LGPL DLL 管理 | [docs/native-dependencies.md](docs/native-dependencies.md) |
| LibRaw ソース・ライセンス管理 | [docs/native-dependencies.md](docs/native-dependencies.md) |
| VST3 host bridge 管理 (v0.9.0+) | [docs/native-dependencies.md](docs/native-dependencies.md) |
| Markdown / テキストファイルのエンコーディング (BOM 必須ケース) | [docs/coding-conventions.md](docs/coding-conventions.md) |
| UI 文字列の Unicode グリフ選定ルール | [docs/coding-conventions.md](docs/coding-conventions.md) |
| タグ / レーティングの保存先と ExifTool 検証 | [docs/tag-storage-and-xmp.md](docs/tag-storage-and-xmp.md) |
| 動画メタ情報の扱いと外部ダウンローダの言及禁止ポリシー | [docs/public-wording-policies.md](docs/public-wording-policies.md) |
| モザイク・成人向け画像処理の表記ポリシー | [docs/public-wording-policies.md](docs/public-wording-policies.md) |
| Distribution | [docs/release-checklist.md](docs/release-checklist.md) |
| コード修正時のドキュメント同時更新 | [docs/public-wording-policies.md](docs/public-wording-policies.md) |
| リリース手順チェックリスト (Phase 0〜6) | [docs/release-checklist.md](docs/release-checklist.md) |
| Codex CLI レビュー | [docs/codex-cli-usage.md](docs/codex-cli-usage.md) |
| 実機検証用バイナリの準備 (Windows ネイティブ機能) | [docs/development-build-and-test-native.md](docs/development-build-and-test-native.md) |
| Formatting | [docs/git-workflow.md](docs/git-workflow.md) |
| Git Workflow | [docs/git-workflow.md](docs/git-workflow.md) |
| 「マニュアル・製品ページの記述方針」(「コード修正時のドキュメント同時更新」の下位節) | [docs/public-wording-policies.md](docs/public-wording-policies.md) |
| 「検証起動時の設定データ保護」(「実機検証用バイナリの準備」の下位節) | [docs/development-build-and-test-native.md](docs/development-build-and-test-native.md) |
| 「worktree + Windows junction の地雷」「複数の Claude Code セッションを並行で動かす」(「Git Workflow」の下位節) | [docs/git-workflow.md](docs/git-workflow.md) |
| 「開発時に core を直接起動したいとき」「FFmpeg ライセンス対応」(「FFmpeg LGPL DLL 管理」の下位節) | [docs/native-dependencies.md](docs/native-dependencies.md) |
| 「IME 対応」「UI スレッドでの同期 I/O」「並行処理」(「Key Design Decisions」の下位節) | [docs/coding-conventions.md](docs/coding-conventions.md) |

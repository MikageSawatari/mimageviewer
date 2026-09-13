# v3.10.0後の開発計画

2026-09-14利用者指定。コレクションの仕様を先に検討し、その会話中にほかの実装を直列で進める。コレクション実装は最後。

## 次版範囲と進め方

1. §1.234 設定の復元・完全リセットの失敗: 必須候補。まず使い捨てportable/非対話試験で本人操作なしに検証可能かを調べる。保持者未特定のSHM失敗を無条件に無視する案を実装前提にしない。
2. §1.231 来歴と互換版の混同・clean installで偽preupgrade生成: 隔離検証可能なら続行。実際の版跨ぎの復元と原本を保持する。
3. §1.229/230 検索の短語と案内文、§1.232 更新案内の不可視modal: 候補。影響/再現性を確認し、上記が本人確認待ちになった場合の代替とする。
4. §2.25 一覧/ツリーsort分離→§2.24 一覧サイズsort: 次版に含める。実装は直列で同じ設定所有を共有しない構造へ。
5. §1.118 コレクション: [仕様案](collection-spec-proposal.md)を先に提示し、承認と設計レビュー後、ほかの実装の最後に着手。

2xモデルは比較/採否待ち、migemoとts/mts/3gpの見送りは維持する。新規機能は推測で範囲拡張しない。

## 不在中の検証

利用者は9/14外出、スマホから仕様確認と会話は可能だがPC手動確認は困難。
この連絡は具体suiteの無制限な実機操作許可には読み替えない。非対話の使い捨てfixture検証とbuildを進め、live操作が必要なら内容・見込み時間・入力/前面使用・data境界・再試行範囲を提示して了承を得る。
通常APPDATAを起動・コピー・変更せず、liveはprepare-portable-smoke.ps1によるtarget/portable-smokeだけを使う。
個人設定/外部機器が不可欠ならその確認のみ保留し、次の独立項目へ進む。実機未検証を完了扱いしない。

## 担当

親は要件・設計判断・進行、実装/検証はSol/xhigh、独立レビューは別のSol/xhigh。
一件のscope/不変条件/完了条件をまとめて渡す。Cargoと同一ファイルのwriterは一担当に限定し、有効な成功結果を再利用する。
コレクションの仕様会話・設計資料作成だけは、別機能の実装と並行する。
公開準備・公開は従来どおりClaudeCodeが担当する。

## 2026-09-14深夜の進行判断

§234の保持者は常設Remote IPCのLiveFavoritesが持つ読み取り用SettingsFavoritesReaderとコードで特定した。Windows TempDir probeではread-only接続が生存中はSHM/WAL双方の削除がerror32になり、close後は成功した。実際の失敗時のhandle採取ではないため、観測とコード因果を区別する。

修正はshared readerの終了確認と新規接続の抑止が必要で、Remote/Appの所有境界へ範囲が広がる。案A（typed quiescence）を独立設計レビューへ回し、その間に§231を先行実装する。製品編集は直列、コレクション仕様だけ並行を維持。

利用者は朝8時まで睡眠とし、このPCで新しいportableテスト環境を使った設定復元等の検証を明示了承した。対象は復元・完全reset・downgrade互換候補・clean初回。target/portable-smokeの使い捨てdataだけを使い、D:/homeの既存portableと通常APPDATAは変更しない。入力・前面使用と後始末も2026-09-14 08:00 JSTまで。

### §1.231 の先行完了

`8319a1225` に実装を記録。来歴を示すバックアップ名とDB内容の互換版を分離し、clean installでは現版の記録だけを行ってpreupgradeを作らない。Sol/xhigh実装・別Sol/xhighレビュー（重大指摘なし）。対象テスト18+1+2件、fmt・UI glyph・diffチェック成功。§1.234統合前のチェックポイントであり、全体gate・確認用build・portable実機検証は未実施。

### §1.234 の設計合意

[設定復旧の設計](settings-recovery-234-231-plan.md)を親・実装者・独立レビューで合意し実装へ進める。設計レビュー対象SHAは `EB0920A4222532D955BCDFB8BABE2536CBFB918F9F6DABB9C1E948EEDB11FC4`、重大指摘なし。常設Remote readerに加え、全`with_db`処理の終了確認、Remote部分起動失敗時の解放、設定writeの適用前Busyを含める。復元を始めていないことが証明できる失敗後は通常の設定アクセスを再開し、Favorites readerだけ再接続に失敗した場合は依存要求だけを明示的な再試行対象にする。

### §1.234 実装の独立検収（03:10 JST時点）

Remoteの常設readerを含むsettings-family接続の停止・解放と、復元workerのApp所有を実装。独立Sol/xhighレビューの指摘により、Archive/AIの復旧中Busyの伝達（IPC v55）、AI完了公開の短期lease、処理中の終了要求の保留と再送、トレイ非表示時の既存wake経路への接続まで修正した。最終限定再検収は重大指摘なし。

対象: `app.rs` SHA256 `3F781D6ED77F4AC7442EB4AA0FE9C3D331C5E5FF36AC7BF809E630363F261FE4`、`settings_restore.rs` `655B00E498315761EEBA7DEEAB8776E748F39C5705A40E419D0D751443FC7998`、復元UI `7D72C14823F5FA80FBAB07F93C8358A5BF82ECA9694137387F45FF23C8D2775A`。UI lifecycle 6件、AI/Archive 5件、family 9件とcheck・fmt・glyph・audit・diffの成功証拠を独立担当が照合した。途中の`app.rs`編集事故は着手前cleanのHEADから意図した14追加/2削除だけに復旧し、その後の検証対象に含めた。

最終full gate・確認用build・使い捨てportable実機はこの時点では未完了。実装者がCargoとfixture検証、親が実機操作と記録を担当し、同じ検証を重複実行しない。

### 朝の区切り（実機未実施）

2026-09-14 08:00区切り: §1.234の最終全体gateはmain 8410 pass / 0 fail / 45 ignored、snapshot 50を含め成功。core/Remote双方の確認用buildとportable buildも成功した。詳細ログ・hashは[設定復旧plan](settings-recovery-234-231-plan.md)を参照。独立レビューは重大指摘なし。

使い捨てportableの`sky.launch_app`は`Computer Use app approval timed out`となり、07:59 JSTのprocess確認もresidentなしだった。実機suiteは全件未実施で、通常profile・既存portableへの起動/操作はない。許可時間帯終了のため再試行せず、fixtureと`target/settings-recovery-20260914/live-checks.md`を保存した。native終了経路の実機確認待ちとして§1.234の製品差分はまだ未コミット。実機で検収済みとは扱わない。

§2.25は[独立検収済みの設計](folder-tree-sort-plan.md)まで。既存のprivate Receiver破棄で旧結果を隔離できるため、追加の数値世代は不要とした。§2.24とコレクションを含め、次項の製品実装はまだ開始していない。

### 次段の調査メモ（§2.25 → §2.24、未実装）

`ui_folder_pane.rs` の同期・再読込・キー操作・ドライブ選択は一覧用の `settings.sort_order` を直接渡している。`folder_pane.rs` のscan workerと、`folder_tree.rs::FolderTreeOptions` を通るフォルダ間移動の双方を、ツリー専用の保存値へ揃える必要がある。お気に入りの表示状態の復元は一覧側だけに留める。ツリーの初期値は利用者要望どおりファイル名順とし、並べ直しの間も展開・現在位置を維持する。

サイズ順の追加前に、一覧以外で `SortOrder` を使う代表サムネイル選定・Remote・本の固定ページ順を棚卸しする。サイズを持たない項目とサイズ0の実ファイルを区別し、同値の名前順、カテゴリ別配置、保存・復元を検収条件にする。ソートのためのUIスレッドでのファイル照会は追加しない。詳細設計と製品編集は§1.234の検収後に直列で行う。

追加照合: 現在の `SortOrder` は名前・番号が昇順のみ、日付が昇降順の4候補。§2.25のツリー候補は名前・番号・日付それぞれの昇降順が要件であり、既存4候補の転用だけでは不足する。また `FolderPaneState::sync_to_active` はソート変更時にnodesと手動展開状態をともに消す。単に展開キーのclearを削ると既存コメントが警告する未ロードの展開行を作るため、子の再列挙と既存展開・cursorの維持を一体で設計・回帰する。

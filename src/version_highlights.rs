//! 更新後 初回起動の「重要な変更点」表示 (v2.0.0 で仕組み導入、docs/version-highlights-plan.md)。
//!
//! `update_check` (更新前・ネットワーク・全文 changelog) とは別物で、こちらは
//! **更新後・オフライン (exe 埋め込み)・操作や既定の変更を中心とした主要部分だけ**を、
//! 更新後の初回起動で 1 画面表示する。インストーラ / ポータブルで黙って更新した
//! ユーザーにも確実に届く。
//!
//! 設計の肝は **選択ロジックを純関数 [`highlights_to_show`] に集約して unit test で
//! 網羅**し、実機テストを最小化すること (バージョンまたぎは実機で再現しにくいため)。

/// 1 項目 (短文)。`title` は見出し、`body` は本文。内部用語は出さない。
#[derive(Clone, Copy, Debug)]
pub struct HighlightItem {
    pub title: &'static str,
    pub body: &'static str,
}

/// 1 バージョンぶんの変更点。`must_read` = 操作・既定の変更 (必読)、
/// `highlights` = 主な新機能 (任意)。
#[derive(Clone, Copy, Debug)]
pub struct VersionHighlights {
    pub version: &'static str,
    pub must_read: &'static [HighlightItem],
    pub highlights: &'static [HighlightItem],
}

/// 一覧のクリック選択をエクスプローラー方式へ切り替える告知エントリの版。
/// テーブルと移行判定が別々の版文字列を持たないための単一の定義元。
pub const GRID_CLICK_SELECTION_EXPLORER_VERSION: &str = "2.9.0";

/// バージョン文字列を `(major, minor, patch)` に緩くパースする。
/// `v` 接頭辞と `-pre` / `+meta` 接尾辞は無視する。パースできなければ `None`
/// (= 呼び出し側は fail-safe にスキップする)。
fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.trim().trim_start_matches('v');
    // major.minor.patch のコア部分だけを見る ("2.0.0-prev" → "2.0.0")。
    let core = s.split(['-', '+']).next().unwrap_or(s);
    let mut it = core.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    // patch は省略可 ("2.0" → patch 0)。
    let patch = match it.next() {
        Some(p) => p.parse().ok()?,
        None => 0,
    };
    Some((major, minor, patch))
}

/// バージョン文字列が `parse_version` で解釈できるか。開発用 `--whatsnew-from <ver>` の
/// 値検証に使う (= 不正値を渡したのに無言でダイアログが出ないのを警告ログで知らせるため)。
pub fn is_valid_version(s: &str) -> bool {
    parse_version(s).is_some()
}

/// 更新後初回起動で表示する変更点を選ぶ (純関数、テスト対象)。
///
/// - `prev = None` (新規インストール) → 空 (新規ユーザーを「変更点」で迎えない)。
/// - `prev` がパース不能 → 空 (fail-safe、起動を止めない)。
/// - `current` がパース不能 → 空 (fail-safe)。
/// - `prev >= current` (同一 / ダウングレード) → 空。
/// - それ以外 → `prev < version <= current` の全エントリをバージョン昇順で返す
///   (バージョンを飛ばした人も途中の重要変更を見逃さない)。
pub fn highlights_to_show<'a>(
    prev: Option<&str>,
    current: &str,
    table: &'a [VersionHighlights],
) -> Vec<&'a VersionHighlights> {
    let Some(cur) = parse_version(current) else {
        return Vec::new();
    };
    let Some(prev_s) = prev else {
        return Vec::new();
    };
    let Some(prev_v) = parse_version(prev_s) else {
        return Vec::new();
    };
    if prev_v >= cur {
        return Vec::new();
    }
    let mut out: Vec<&VersionHighlights> = table
        .iter()
        .filter(|e| match parse_version(e.version) {
            Some(v) => v > prev_v && v <= cur,
            None => false,
        })
        .collect();
    out.sort_by_key(|e| parse_version(e.version).unwrap_or((0, 0, 0)));
    out
}

/// v2.9.0 の告知と同じ更新範囲に入った初回起動かを返す。
/// 独自の版比較は持たず、実際に表示対象となるエントリ集合から直接導出する。
pub fn grid_click_selection_explorer_upgrade_required(prev: Option<&str>, current: &str) -> bool {
    highlights_to_show(prev, current, table())
        .iter()
        .any(|entry| entry.version == GRID_CLICK_SELECTION_EXPLORER_VERSION)
}

/// 指定バージョン (= 通常は現行版) のエントリを返す。ヘルプメニューからの再表示用。
/// パース一致で引く (= `2.0.0` と `2.0` は同一視)。
pub fn for_version<'a>(
    version: &str,
    table: &'a [VersionHighlights],
) -> Vec<&'a VersionHighlights> {
    let Some(v) = parse_version(version) else {
        return Vec::new();
    };
    table
        .iter()
        .filter(|e| parse_version(e.version) == Some(v))
        .collect()
}

/// 指定バージョン以下で最も新しいエントリを返す。ヘルプメニューからの再表示で、
/// 次リリース向けのエントリを先に埋め込んだ開発版が未来の変更点を見せないために使う。
pub fn latest_not_newer_than<'a>(
    version: &str,
    table: &'a [VersionHighlights],
) -> Option<&'a VersionHighlights> {
    let current = parse_version(version)?;
    table
        .iter()
        .filter_map(|entry| {
            let parsed = parse_version(entry.version)?;
            (parsed <= current).then_some((parsed, entry))
        })
        .max_by_key(|(parsed, _)| *parsed)
        .map(|(_, entry)| entry)
}

/// exe 埋め込みの変更点テーブル。**リリースのたびに、操作・既定の変更があれば
/// このテーブルに 1 エントリ追記する** (CLAUDE.md リリース手順)。
pub fn table() -> &'static [VersionHighlights] {
    TABLE
}

/// 変更点エントリ群を egui に描く (App 非依存)。ダイアログ本体と egui_kittest スナップショット
/// テストの両方から呼べるよう、`&[&VersionHighlights]` だけを受け取る純粋な描画関数にする。
pub fn render(ui: &mut egui::Ui, entries: &[&VersionHighlights]) {
    let multi = entries.len() > 1;
    for entry in entries {
        if multi {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!("v{}", entry.version))
                    .strong()
                    .size(15.0),
            );
        }
        if !entry.must_read.is_empty() {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("操作・既定の変更")
                    .strong()
                    .color(egui::Color32::from_rgb(210, 140, 40)),
            );
            for item in entry.must_read {
                render_item(ui, "⚠", item);
            }
        }
        if !entry.highlights.is_empty() {
            ui.add_space(8.0);
            ui.label(egui::RichText::new("主な新機能").strong());
            for item in entry.highlights {
                render_item(ui, "・", item);
            }
        }
        ui.add_space(6.0);
        ui.separator();
    }
}

/// 1 項目 (見出し + 本文)。`marker` は ⚠ (必読) / ・ (新機能)。
fn render_item(ui: &mut egui::Ui, marker: &str, item: &HighlightItem) {
    ui.add_space(3.0);
    ui.label(egui::RichText::new(format!("{marker} {}", item.title)).strong());
    ui.horizontal_wrapped(|ui| {
        ui.add_space(14.0);
        ui.label(egui::RichText::new(item.body).size(12.5));
    });
}

const TABLE: &[VersionHighlights] = &[
    VersionHighlights {
        version: "2.0.0",
        must_read: &[
            HighlightItem {
                title: "ツールバーの設定は右クリックに変わりました",
                body: "ツールバーに出す項目・並び順・表示のしかたは、ツールバーを右クリックして変更します。\
                       何も無い場所を右クリックすると表示する項目を選べ、各項目名を右クリックするとその項目の設定ができます。\
                       並べ替えは、右クリックメニューで「ドラッグで並べ替えを許可」を ON にすると、項目名のドラッグでできます。\
                       (環境設定の「ツールバー」ページは無くなりました)",
            },
            HighlightItem {
                title: "タグ・本棚などのクリック操作を統一しました",
                body: "ツールバーの項目は「左クリック=開く・表示」「右クリック=付与・追加」に揃えました。\
                       タグは左クリックでタグの一覧表示、右クリックで選択中の画像へ付与します\
                       (以前と左右が逆になっています)。",
            },
            HighlightItem {
                title: "Z キーが「部分拡大ズーム」に変わりました",
                body: "フルスクリーン表示で Z キーを押すと、画像の一部を画面いっぱいに拡大し、\
                       マウスを動かすだけで拡大位置を移動できます。もう一度 Z で元に戻ります。\
                       これまで Z だった画像分析モードは Shift+Z に移動しました。",
            },
        ],
        highlights: &[
            HighlightItem {
                title: "連番画像をまとめるファイル名スタック表示",
                body: "同じ接頭辞のファイルを 1 つのセルにまとめて、一覧をすっきり表示できます。\
                       フォルダバーの「スタック」ボタンで切り替え。まとめ方の分類ルールは\
                       スクリプトでカスタマイズもできます (既定の組み込みルールでも連番・連写\
                       などを自動でまとめます)。",
            },
            HighlightItem {
                title: "よく使う本をツールバーにピン留め",
                body: "本棚の管理画面で本を「固定」すると、ツールバーにその本のボタンが並びます。\
                       左クリックで開く、右クリックで選択中の画像をその本へ追加できます。",
            },
            HighlightItem {
                title: "フォルダバーも右クリックで設定",
                body: "フォルダ入力欄の左にある「フォルダ:」を右クリックすると、表示するボタンの選択や\
                       履歴のクリアができます。",
            },
        ],
    },
    VersionHighlights {
        version: "2.2.0",
        must_read: &[],
        highlights: &[
            HighlightItem {
                title: "現在の画面で使えるショートカット一覧",
                body: "サムネイル一覧、画像 / 動画フルスクリーン、消しゴム・隠蔽加工・切り取り・テキスト注釈・補正レイヤーモードで ? キーを押すと、\
                       その場で使えるキー操作を一覧できます。キー設定を変更している場合は、変更後の割り当てで表示します。",
            },
            HighlightItem {
                title: "キー割り当て表示が設定に追従",
                body: "メニュー項目、フルスクリーンのホバーバー、動画 HUD、★フィルター、編集ツールボタンなどのキー表記が、\
                       実際のキー設定に合わせて表示されるようになりました。キーが未設定の操作もコマンド設定から割り当てられます。",
            },
            HighlightItem {
                title: "メニュー構成のカスタマイズ",
                body: "環境設定の「表示 → メニュー構成」で、上部メニューと固定メニュー項目の表示 / 非表示や並び順を変更できます。\
                       登録済みのお気に入りやタグなど、内容が状況で変わる項目はこれまでどおりの位置に表示します。",
            },
            HighlightItem {
                title: "右ドラッグのマウスジェスチャ",
                body: "設定メニューの「操作カスタマイズ…」で、グリッド、画像 / 動画フルスクリーン、編集モードごとに右ドラッグを\
                       未使用 / リングショートカット / マウスジェスチャから選べます。ジェスチャは上下左右の軌跡を登録して実行でき、長押し中は登録済み操作を一覧表示します。",
            },
            HighlightItem {
                title: "サブフォルダをまとめて表示",
                body: "フォルダバーの「サブ展開」で、現在のフォルダ以下にある画像と動画を一時的なフラット一覧として表示できます。\
                       ★・タグ・場所などの絞り込みやスタック表示と組み合わせて、複数フォルダに分かれた項目をまとめて確認できます。",
            },
        ],
    },
    VersionHighlights {
        version: "2.3.0",
        // 当初「種類フィルタで音声を選んだまま v2.2.0 へ戻すと設定が巻き戻る」注意を
        // 載せる予定だったが、保存形を v2.2.0 互換に修正した (FacetFilter::kind_audio_stash)
        // ため、その注意自体は不要になった。
        must_read: &[HighlightItem {
            title: "削除するとレーティング・タグ・補正などのデータも一緒に消えます",
            body: "これまでは、画像や動画を削除しても、その画像に付けていたレーティング・タグ・補正・\
                   回転などのデータが残ってしまうことがありました。今回から、削除するとこれらのデータも\
                   一緒に消えます (ごみ箱から戻しても、これらのデータは戻りません)。\
                   以前の削除で残ったままになっているデータは、「設定 → サムネイルキャッシュ管理 →\
                   メタデータを整理…」でまとめて掃除できます。★の件数が実際のファイル数と合わない場合\
                   などにお使いください。\
                   ※ 取り外し中の外付けドライブや、接続できないネットワークドライブのデータは、\
                   誤って消さないよう対象外にします。",
        }],
        highlights: &[
            HighlightItem {
                title: "音楽を波形・スペクトラムで見ながら再生",
                body: "音声ファイル (MP3 / FLAC / WAV / M4A など) を一覧からそのままフルスクリーンで再生できます。\
                       曲全体の波形タイムライン、半音ごとのバーと 88 鍵ピアノ鍵盤のスペクトラム、\
                       ★・タグ・ブックマーク・音量の自動調整・VST3 に対応します。",
            },
            HighlightItem {
                title: "複数ウィンドウで開けるようになりました",
                body: "環境設定の「ビューワモード」で「複数ウィンドウ」を選ぶと、画像を開くたびに別ウィンドウとして残り、\
                       一覧を操作しても各ウィンドウは独立して残ります。「フル機能ウィンドウ」ではこれまでどおり\
                       メインと 1 つの別ウィンドウを F12 で切り替えます。",
            },
        ],
    },
    VersionHighlights {
        version: "2.4.0",
        must_read: &[
            HighlightItem {
                title: "システムファイルを一覧に表示しなくなりました",
                body: "ごみ箱やシステムが保護しているフォルダなどを、一覧やフォルダツリーに\
                   表示しないようにしました。隠しファイルは環境設定 > フォルダ・ファイルの\
                   「隠しファイル・フォルダを表示する」で表示に切り替えられます。",
            },
            HighlightItem {
                title: "本のページはファイル名順で進みます",
                body: "ZIP や対応アーカイブ、画像だけのフォルダを本として読むときは、\
                   一覧の並べ替え設定にかかわらずファイル名順でページが進みます。",
            },
            HighlightItem {
                title: "フルスクリーンのパネル操作が変わりました",
                body: "通常ホバーでは、左端で補正パネル、右端で情報パネルを個別に表示します。右端へ寄せても左の編集パネルは開きません。上部の i ボタンと I / Tab キーは、情報パネルの固定ではなく、通常ホバーとクリック表示の切り替えに変わりました。",
            },
            HighlightItem {
                title: "テキスト注釈の四隅ハンドルは反対側の角を固定します",
                body: "吹き出しなどの四隅をドラッグすると、反対側の角を固定したまま拡大縮小します。\
                   Ctrl を押している間は従来どおり中心を固定し、Shift で縦横比を保てます。",
            },
            HighlightItem {
                title: "新しい吹き出しは文字が上下中央に配置されます",
                body: "作成済みの吹き出しの位置は変わりません。セリフタブの「文字を上下中央に補正」で、\
                   既存の吹き出しも新しい配置に切り替えられます。",
            },
        ],
        highlights: &[
            HighlightItem {
                title: "左右パネルのクリック表示モード",
                body: "画面の左右最端に出る細いバーをクリックしてパネルを開き、表示中のファイルでは明示的に閉じるまで表示できます。別のファイルへ移動すると、左右のパネルは閉じます。",
            },
            HighlightItem {
                title: "スクリーンショット向けの注釈図形",
                body: "テキスト注釈の「注釈追加」から、強調枠・矢印・蛍光マーカー・自動採番の番号バッジ・\
                   カーソルを配置できます。蛍光マーカーは下の文字を潰さずに色が重なります。",
            },
        ],
    },
    VersionHighlights {
        version: "2.5.0",
        must_read: &[HighlightItem {
            title: "編集結果をサムネイル一覧へ反映します",
            body: "消しゴム、補正レイヤー、隠蔽加工、テキスト／スタンプ、切り取りの結果を、\
                   編集を閉じた後も一覧へ表示します。編集プレビューキャッシュは既定で有効・上限 1GB です。\
                   環境設定の「パフォーマンス → キャッシュ」から無効化や容量変更ができます。",
        }],
        highlights: &[
            HighlightItem {
                title: "補正レイヤーで修復・塗り・クローン",
                body: "スポイト色の塗り、明るさを残した着色、周囲のテクスチャを使った修復、\
                       コピー元を指定するクローンを追加しました。画像フルスクリーンでは Ctrl+G から直接開けます。",
            },
            HighlightItem {
                title: "編集内容を別のページへコピー",
                body: "個別補正、消しゴム、隠蔽加工、補正レイヤー、切り取り、テキスト注釈をまとめて、\
                       通常画像・ZIP 内画像・PDF ページの間で再利用できます。",
            },
            HighlightItem {
                title: "テキスト注釈をまとめて整列",
                body: "複数の注釈を選択して移動・削除・整列・均等配置できます。\
                       ドラッグ中はほかの注釈へガイドを表示して吸着します。",
            },
        ],
    },
    VersionHighlights {
        version: "2.6.0",
        must_read: &[HighlightItem {
            title: "詳細表示にページ数列を追加しました",
            body: "ZIP／CBZ、PDF、画像だけのフォルダーでは、詳細表示にページ数を表示します。\
                   列ヘッダーを右クリックすると、ページ数列を表示／非表示にできます。",
        }],
        highlights: &[
            HighlightItem {
                title: "複数の場所をまとめるスマートフォルダ",
                body: "任意の複数フォルダーと表示条件を保存し、画像・動画・音声・ZIP・PDFなどを\
                       1つのフラットな一覧へまとめて表示できます。",
            },
            HighlightItem {
                title: "マウス操作とリング操作を拡充",
                body: "右ドラッグ開始時の項目選択、ウィンドウを閉じる／アプリを終了する操作、\
                       選択を変えない先頭・末尾スクロールなどを追加しました。",
            },
            HighlightItem {
                title: "文字のコントラストを選択",
                body: "メニュー、ツールバー、ダイアログ、フルスクリーンの文字を、\
                       「標準」または「強め」から選べます。",
            },
        ],
    },
    VersionHighlights {
        version: "2.7.0",
        must_read: &[HighlightItem {
            title: "本のブックマークは B キー",
            body: "画像表示中に B キーを押すと、現在のページをブックマークできます。\
                   これまで B キーに割り当てられていた透過背景の切り替えは Shift+B に変わりました。",
        }],
        highlights: &[
            HighlightItem {
                title: "動画・音声・本のブックマークをまとめて表示",
                body: "ZIP／CBZ、PDF、製本、画像だけのフォルダーでもページをブックマークできます。\
                       「場所 → ブックマーク」では、動画・音声・本をまとめて絞り込み、開けます。",
            },
            HighlightItem {
                title: "画面全体のフォントを選択",
                body: "環境設定の「表示 → フォント」から、Windows の日本語フォントや\
                       追加した日本語フォントを選べます。見本を確認し、文字の縦位置も微調整できます。\
                       文字の大きさは従来どおり「設定 → スケーリング」で変更できます。",
            },
        ],
    },
    VersionHighlights {
        version: "2.8.0",
        must_read: &[HighlightItem {
            title: "疑似カラーは「カラー化」タブへ",
            body: "ポストフィルタにあった「疑似カラー（4色刷り）」「疑似カラー（肌色）」は、\
                   画像補正パネルの新しい「カラー化」タブへ移動しました。設定済みのページ、\
                   お気に入り、保存スロットは見た目を保ったまま自動で移ります。",
        }],
        highlights: &[
            HighlightItem {
                title: "モノクロ画像の階調カラー化",
                body: "画像補正パネルの「カラー化」タブから、色と強さを指定するカスタムパレットや、\
                       スクリーントーンの網点を濃淡へ変える処理を選べます。\
                       カラー化専用の保存スロットも4個あります。",
            },
            HighlightItem {
                title: "静止画と動画へ Creative LUT",
                body: "組み込みプリセットと、環境設定へ追加した 3D LUT ファイルを、\
                       静止画・動画の画像補正パネルから選べます。",
            },
            HighlightItem {
                title: "メタ情報を別環境へ持ち運び",
                body: "実フォルダーの評価、タグ、ブックマーク、画像補正や注釈などをまとめて\
                       書き出し、別の PC やポータブル版へ取り込めます。",
            },
            HighlightItem {
                title: "一覧のクリック選択方法を選択",
                body: "環境設定の「表示 → サムネイル」から、従来のチェック方式と\
                       エクスプローラー方式を選べます。",
            },
            HighlightItem {
                title: "別ウィンドウから前後の場所へ移動",
                body: "複数ウィンドウモードの画像や本でも、Ctrl+↑／Ctrl+↓などで\
                       前後の画像フォルダー、ZIP／CBZ、PDFへ移動できます。",
            },
        ],
    },
    VersionHighlights {
        version: GRID_CLICK_SELECTION_EXPLORER_VERSION,
        must_read: &[
            HighlightItem {
                title: "クリックで以前のチェックを解除します",
                body: "一覧で項目をクリックすると、それまでのチェックが解除され、クリックした項目だけが\
                       選ばれるようになりました（エクスプローラーと同じ動作）。従来の動作に戻すには、\
                       環境設定の「表示 → サムネイル → 一覧のクリック選択」で「チェック方式」を選べます。",
            },
            HighlightItem {
                title: "画像補正はまず「その場所の標準」に効きます",
                body: "個別設定を持たないページで補正スライダーを動かすと、そのページだけでなく、\
                       その場所の標準が変わり、同じ標準を使うページすべてに反映されます。\
                       このページだけに効かせたいときは、パネル上部の適用範囲で「このページ」を選びます。\
                       お気に入りごとに標準を分けたいときは「このお気に入り用に標準を分ける」を有効にします。",
            },
        ],
        highlights: &[
            HighlightItem {
                title: "連結読みでも左パネルが使えます",
                body: "スクロールしながら読んでいる途中でも、画像補正・表示トリミング・ブックマークを\
                       開けます。編集対象のページは枠で示します。",
            },
            HighlightItem {
                title: "詳細表示の下部バーに独自の列",
                body: "下部バーに出す列を、一覧と同じ列・専用の列・非表示から選べます。\
                       列の追加や幅の調整はバーからもできます。",
            },
            HighlightItem {
                title: "動画にも画像補正の保存スロット",
                body: "明るさや Creative LUT などの設定をスロットへ保存し、\
                       Ctrl+1〜Ctrl+0 で呼び出せます。",
            },
            HighlightItem {
                title: "PDF のページ表示が速くなりました",
                body: "表示に必要な大きさでページを描くようにしました。\
                       CPU のコア数が少ない環境ほど効果があります。",
            },
        ],
    },
    VersionHighlights {
        version: "2.9.1",
        must_read: &[
            HighlightItem {
                title: "Tab キーで入力欄が移動しなくなりました",
                body: "文字を入力しているときに Tab キーを押しても、次の入力欄へ移らなくなりました。\
                       入力中に背後の機能が誤って動くのを防ぐためです。\
                       タグ編集・お気に入り編集・書き出しの画面で別の入力欄へ移るときは、\
                       その欄をクリックしてください。",
            },
            HighlightItem {
                title: "名前の変更と新しいフォルダーは Windows の画面になりました",
                body: "ファイル名の変更と新しいフォルダーの作成は、Windows 標準の入力画面で行います。\
                       文字の編集や日本語入力の扱いがエクスプローラーと同じになります。",
            },
            HighlightItem {
                title: "タスクトレイへ格納しても再生が続きます",
                body: "動画や音楽をタスクトレイへ格納しても、再生が止まらなくなりました。\
                       格納中に再生が終わったときは次のファイルへ進みます。\
                       これまでは格納すると一時停止していました。",
            },
        ],
        highlights: &[],
    },
    VersionHighlights {
        version: "2.10.0",
        must_read: &[
            HighlightItem {
                title: "「読書履歴」は「閲覧履歴」になりました",
                body: "これまで本（フォルダー・ZIP・PDF）だけだった履歴に、動画と音声も残るようになりました。\
                       音声を含むため名前を「閲覧履歴」に改めています。\
                       一覧では「すべて／動画／音声／本」や、種類・拡張子・場所で絞り込めます。",
            },
            HighlightItem {
                title: "自動で進んだものは履歴に残りません",
                body: "連続再生で次のファイルへ進んだとき、スライドショーが自動で送ったとき、\
                       スライドショーの末尾で次のフォルダーへ進んだときは履歴に記録しなくなりました。\
                       自動で流れていったものまで残すと、本当に残したいものが押し出されてしまうためです。\
                       一覧やキー操作で自分から移動したときは、これまでどおり記録されます。",
            },
            HighlightItem {
                title: "本として扱わないフォルダーは単ページで開きます",
                body: "「画像のみのフォルダを本として扱う」がオフのとき、\
                       およびオンでも画像以外のファイルが含まれるフォルダーでは、単ページで開くようになりました。\
                       そのフォルダーに見開きを保存していた場合は、これまでどおりその設定が使われます。",
            },
        ],
        highlights: &[HighlightItem {
            title: "縮小表示の見え方を選べます",
            body: "画像補正の「フィルタ」に「縮小表示のモアレを抑制する」を追加しました。\
                   オフにすると線がくっきりする代わりにモアレやちらつきが出やすくなり、\
                   オンのまま強さを上げるとモアレが減って細部が柔らかくなります。\
                   既定は今までと同じ見え方のままです。",
        }],
    },
    VersionHighlights {
        version: "2.11.0",
        must_read: &[
            HighlightItem {
                title: "原寸表示は画面のピクセル基準になりました",
                body: "「100%原寸」「拡大しない」「縮小しない」は、画面のピクセルと画像のピクセルが\
                       1対1で対応する表示になりました。Windows側で拡大縮小（125%・150%など）を\
                       設定している場合、同じ画像がこれまでより小さく表示されます。\
                       そのぶん線や文字はくっきりします。",
            },
            HighlightItem {
                title: "縮小表示の画質と設定が変わりました",
                body: "縮小表示の画質を改善し、これまでのモアレ抑制の設定は\
                       「縮小時のなめらかさ」に変わりました。網点やトーンのちらつきが\
                       気になるときに値を上げてください。上げるほど細部は柔らかくなります。",
            },
        ],
        highlights: &[],
    },
    VersionHighlights {
        version: "2.12.0",
        must_read: &[
            HighlightItem {
                title: "拡大表示の画質が上がりました",
                body: "画像を拡大したときの画質を、縮小と同じ高品質な方法へ変えました。これまでより輪郭がはっきりします。画像補正パネルの「フィルタ」から「シャープ拡大」「アニメ塗り拡大」も選べます。",
            },
            HighlightItem {
                title: "サブ展開は確認画面が開くようになりました",
                body: "「サブ展開」を押すと、たどる階層と集める項目を選ぶ画面が開きます。これまでどおり全部たどるには「無制限」を選んでください。選んだ条件は次回も引き継がれます。",
            },
        ],
        highlights: &[
            HighlightItem {
                title: "拡大中の位置を示すナビゲータ",
                body: "拡大表示中に Alt を押しているあいだ、画面の隅に全体の縮小画像といま見えている範囲の枠が出ます。枠をドラッグして表示位置を動かせます。360度パノラマにも対応しています。",
            },
            HighlightItem {
                title: "ファイル名での絞り込み",
                body: "絞り込みバーの「ファイル名」欄で、今表示している一覧をファイル名で絞り込めます。先頭に - を付けた語は、含まないものが残ります。",
            },
            HighlightItem {
                title: "一覧に出していないファイルのお知らせ",
                body: "同じ名前でまとめた分・隠れている項目・開けない形式のファイルがあると、フォルダバーに「非表示 N 件」と出ます。フォルダを削除するときは、見えていないファイルも消えることを確認画面で案内します。",
            },
        ],
    },
    VersionHighlights {
        version: "2.13.0",
        must_read: &[
            HighlightItem {
                title: "比較表示は単ページ表示専用になりました",
                body: "X でピン留めして C で見比べる機能は、単ページ表示のときだけ使えます。見開き表示のまま操作すると、その旨を表示して比較には入りません。比較中に見開きへ切り替えると比較を終了します。",
            },
            HighlightItem {
                title: "ワイプ比較の境界線は必要なときだけ出ます",
                body: "左右ワイプ比較の白い境界線は、ドラッグ中と、境界線の近くにポインターがあるあいだだけ表示します。比較中に線が絵の邪魔にならないようにするためです。線は画像の左右の端まで動かせるようになりました。",
            },
        ],
        highlights: &[
            HighlightItem {
                title: "タッチ画面で操作できるようになりました",
                body: "一覧は指のドラッグでスクロール、2本指で列数を変更できます。フルスクリーンは左右のタップでページ移動、中央のタップで操作バー、2本指で拡大縮小できます。動画と音楽にも対応しています。初めて触ったときに操作の案内を1度だけ表示します。",
            },
            HighlightItem {
                title: "一覧の「名前の変更」と「更新」",
                body: "どちらも既定ではキーが割り当てられていません。操作カスタマイズから F2 や F5 など好きなキーに割り当てられます（F1〜F6 は既定でレーティングに使われています）。「更新」はファイルメニューからも実行できます。",
            },
            HighlightItem {
                title: "削除の確認を矢印キーで選べます",
                body: "矢印キーで「削除」と「キャンセル」を選び、Enter で決定できます。ごみ箱へ移す確認は「削除」、完全に削除される可能性がある確認は「キャンセル」が最初に選ばれます。",
            },
            HighlightItem {
                title: "拡大中のナビゲータの改善",
                body: "画像全体が見えているときも隠さず表示するようになりました。比較表示中も出ます。縮小画像にカラー化や画像補正の結果が反映されます。",
            },
        ],
    },
    VersionHighlights {
        version: "3.0.0",
        // 利用者判断 (2026-08-14): この画面は「必ず見てほしい」ものだけに絞る。
        // 操作・既定の変更は全部載せ、新機能は目玉のリモート閲覧だけにする
        // (環境設定検索・バケツ・動画の向きは更新履歴に任せた)。
        must_read: &[
            HighlightItem {
                title: "分析モードのボタンが移りました",
                body: "上部バーに増えたボタンを押すと開く一覧の中に入っています。この一覧には、これまでキーだけで画面上のボタンが無かった操作 (ナビゲータの 2 つの設定、ピクセルグリッド、ルーペの固定) もまとまっています。",
            },
            HighlightItem {
                title: "比較表示中は元画像表示キーが効きません",
                body: "比較は加工後の画素で作っているため、押しても画面は変わらないまま、周囲の表示だけが乱れていました。かわりに、左 Ctrl を押しているあいだワイプの境界線が消えるので、線が重なっていた部分を確認できます。",
            },
            HighlightItem {
                title: "連続ページ送りの表示が変わりました",
                body: "キーを押しっぱなしにして連続でめくったとき、スムーズに、かつ通過するページをすべて表示するため、連続めくり中はサムネイルの画質で表示します。指を離して止まったページはフル画質に戻ります。通過中も色味は変わりません。1 回だけ押したときは画質を落としません。",
            },
        ],
        highlights: &[HighlightItem {
            title: "外出先から自宅 PC のライブラリを見られます",
            body: "自宅の PC で mImageViewer を起動したままにしておくと、外出先のスマートフォンやタブレットのブラウザから同じライブラリを開けます。画像も漫画も動画も、手元へコピーせずそのまま見られます。既定ではオフで、使うには環境設定から有効にします。接続には Tailscale と PIN の両方が必要です。",
        }],
    },
    VersionHighlights {
        version: "3.2.0",
        must_read: &[
            HighlightItem {
                title: "フォルダの代表画像の選び方が変わります",
                body: "フォルダタイルの代表画像を、一覧と同じファイル名順で選ぶように変更します。以前に番号順を選んでいた場合も、更新後に一度だけファイル名順へ変わります。番号順へ戻すには、環境設定 → フォルダ・ファイル → 「代表画像の選択基準」で「番号順（区切り無視）」を選んでください。",
            },
            HighlightItem {
                title: "バケツで塗る範囲が既定で 1px 広がります",
                body: "「全体」と「隣接のみ」で塗ったとき、境界に細い塗り残しが出ないよう、既定で 1px 外側まで塗るようになります。以前の塗り方に戻すには、バケツの「はみ出し」を 0 にしてください。",
            },
            HighlightItem {
                title: "白黒・セピア原稿の消しゴム補完が既定で色を合わせます",
                body: "白黒やセピアの原稿を消しゴムで補完したとき、補完した部分だけ色味が浮かないよう、補完結果を周囲の色調に合わせるようになります。以前の仕上がりに戻すには、消しゴムツールパネルの「色調の許容」を 0 にしてください（既定 12）。",
            },
            HighlightItem {
                title: "動画の拡大・縮小の見え方が変わります",
                body: "動画を画面の大きさに合わせて拡大・縮小するとき、mImageViewer 側で処理するようになります。特に大きな動画を小さめのウィンドウで見たときの、細かい模様のちらつき（モアレ）が出にくくなります。以前と同じ表示に戻すには、動画を全画面で開いて左パネルの「画像補正」→「フィルタ」で「OS に任せる」を選んでください。",
            },
        ],
        highlights: &[
            HighlightItem {
                title: "コピー・移動したファイルの編集内容を引き継げます",
                body: "エクスプローラーなどでコピー・移動したファイルを開くと、元のファイルに付けていた補正・消しゴム・モザイク・注釈・トリミング・★・タグを引き継ぐか確認します。コピー元が複数見つかったときは、どれから引き継ぐかを選べます。確認が不要なら、環境設定 → フォルダ・ファイル で切ることもできます。",
            },
            HighlightItem {
                title: "バケツで塗る範囲を選べます",
                body: "全体 / 隣接のみ / 長方形 / 楕円 / 円 の 5 つから選べます。",
            },
            HighlightItem {
                title: "動画の上部バーと下部シークバーを固定表示できます",
                body: "環境設定 → 動画 の「再生画面のバー」で、上部と下部をそれぞれ固定表示にできます。固定したバーは映像に重ならず、映像はバーを除いた領域に収まります。各バー端の鍵アイコンからも切り替えられます。",
            },
            HighlightItem {
                title: "動画の拡大方法を選べます",
                body: "標準（補間あり）/ ニアレスト（補間なし）/ シャープ拡大 / アニメ塗り拡大 から選べます。動画を全画面で開いて左パネルの「画像補正」→「フィルタ」で切り替えるか、T キーで順に切り替えられます。アニメ塗り拡大は、お使いの環境で間に合う品質を自動で測って選びます。",
            },
            HighlightItem {
                title: "動画サムネイルの目印を選べます",
                body: "環境設定 → 表示 → サムネイル で、代表画像の中央に重ねる再生アイコンを、左下の小さなバッジへ替えるか、非表示にできます。",
            },
            HighlightItem {
                title: "ごみ箱へ移すときの確認を省略できます",
                body: "環境設定 → フォルダ・ファイル で、ごみ箱へ移す削除の確認を省略できます。完全に削除される場合は、この設定に関わらず確認を表示します。",
            },
            HighlightItem {
                title: "マスク編集をやり直せます",
                body: "消しゴムやモザイクのマスク編集で、取り消した操作を Ctrl+Y / Ctrl+Shift+Z でやり直せます。",
            },
            HighlightItem {
                title: "Shift+ホイールで筆の太さを変えられます",
                body: "消しゴムやモザイクの筆を、画面の大きさに対する割合で太くしたり細くしたりできます。",
            },
        ],
    },
    VersionHighlights {
        version: "3.3.0",
        must_read: &[HighlightItem {
            title: "最大化したまま終了すると、次回も最大化で起動します",
            body: "ウィンドウを最大化した状態で終了すると、次に起動したときも最大化で開きます。これまでどおり、最大化を解いたときは終了時の位置とサイズに戻ります。常に通常のウィンドウで起動したい場合は、環境設定 → 起動と連携 → 起動時の動作 → 「起動時のウィンドウ状態」で「通常ウィンドウ」を選んでください。常に最大化で起動することもできます。",
        }],
        highlights: &[
            HighlightItem {
                title: "横長のページを左右に分けて読めます",
                body: "見開きでスキャンされて 1 枚の横長画像になっているページを、片側ずつ画面いっぱいに表示して読み進められます。全画面で 8 (左→右) または 9 (右→左) を押すか、上部のバーから選んでください。分割してもページ番号は変わらず、見開きのペアも変わりません。mIV Remote でも同じように使えます。",
            },
            HighlightItem {
                title: "360 度動画を視点を動かしながら見られます",
                body: "球面メタデータを持つ正距円筒の動画や、横：縦が 2:1 の動画で、上部バーの「360」ボタンから切り替えられます。ドラッグで視点を動かし、ホイールで視野角を変えられます。投影方式は透視投影・立体射影・等距離射影・等立体角射影の 4 つから選べ、静止画の 360 度表示でも同じように選べます。",
            },
            HighlightItem {
                title: "動画のシークバーにサムネイル列と音声波形を出せます",
                body: "全画面で Shift+S を押すと、なし → サムネイル列 → 音声波形 の順に切り替わります。ホイールで表示する時間の範囲を変えられ、右上の鍵アイコンで映像に重ねずに固定表示できます。",
            },
        ],
    },
    VersionHighlights {
        version: "3.3.1",
        must_read: &[HighlightItem {
            title: "前回終了したときのカーソル位置に戻るようになりました",
            body: "前回いた場所を開き直したとき、選んでいた項目にカーソルを戻し、画面の同じ高さに表示します。ウィンドウの大きさや列数を変えていても、同じ見え方になるように位置を計算し直します。項目が無くなっていたときは先頭を選びます。戻さずに先頭から表示したい場合は、環境設定 → 起動と連携 → 起動時の動作 →「前回のカーソル位置を復元する」のチェックを外してください。",
        }],
        highlights: &[],
    },
    VersionHighlights {
        version: "3.4.0",
        must_read: &[],
        highlights: &[HighlightItem {
            title: "1 枚の絵を 2〜4 枚に切り分けて書き出せます (SNS 分割)",
            body: "X や Instagram に複数枚として投稿し、横にめくると全体で 1 枚の絵に見える投稿を作れます。全画面で画像補正パネルのいちばん右のアイコンから始めます。選んだ範囲を枚数で等分するので、「画像全体に合わせる」を押せば絵全体をそのまま分けられます。X では、画像と画像の間の隙間に隠れる帯を既定で取り除くので、めくったときに継ぎ目が繋がります。取り除かない設定にもできます。ファイル名の末尾に付く番号の順に添付してください。回転しているページでは使えません。",
        }],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    const T: &[VersionHighlights] = &[
        VersionHighlights {
            version: "1.0.0",
            must_read: &[],
            highlights: &[],
        },
        VersionHighlights {
            version: "1.5.0",
            must_read: &[],
            highlights: &[],
        },
        VersionHighlights {
            version: "2.0.0",
            must_read: &[],
            highlights: &[],
        },
        VersionHighlights {
            version: "2.2.0",
            must_read: &[],
            highlights: &[],
        },
    ];

    fn versions(v: &[&VersionHighlights]) -> Vec<&'static str> {
        v.iter().map(|e| e.version).collect()
    }

    #[test]
    fn fresh_install_shows_nothing() {
        assert!(highlights_to_show(None, "2.0.0", T).is_empty());
    }

    #[test]
    fn same_version_shows_nothing() {
        assert!(highlights_to_show(Some("2.0.0"), "2.0.0", T).is_empty());
    }

    #[test]
    fn downgrade_shows_nothing() {
        assert!(highlights_to_show(Some("2.0.0"), "1.5.0", T).is_empty());
    }

    #[test]
    fn single_step_shows_one() {
        // 1.5.0 → 2.0.0: (1.5.0, 2.0.0] = {2.0.0}
        assert_eq!(
            versions(&highlights_to_show(Some("1.5.0"), "2.0.0", T)),
            ["2.0.0"]
        );
    }

    #[test]
    fn multi_step_accumulates_in_order() {
        // 0.9.0 → 2.0.0: (0.9.0, 2.0.0] = {1.0.0, 1.5.0, 2.0.0} 昇順
        assert_eq!(
            versions(&highlights_to_show(Some("0.9.0"), "2.0.0", T)),
            ["1.0.0", "1.5.0", "2.0.0"]
        );
    }

    #[test]
    fn excludes_versions_above_current() {
        // 1.0.0 → 1.5.0: (1.0.0, 1.5.0] = {1.5.0} (2.0.0 は current 超なので除外)
        assert_eq!(
            versions(&highlights_to_show(Some("1.0.0"), "1.5.0", T)),
            ["1.5.0"]
        );
    }

    #[test]
    fn unparseable_prev_shows_nothing() {
        assert!(highlights_to_show(Some("garbage"), "2.0.0", T).is_empty());
    }

    #[test]
    fn unparseable_current_shows_nothing() {
        assert!(highlights_to_show(Some("1.0.0"), "not-a-version", T).is_empty());
    }

    #[test]
    fn empty_table_shows_nothing() {
        assert!(highlights_to_show(Some("0.9.0"), "2.0.0", &[]).is_empty());
    }

    #[test]
    fn prerelease_suffix_is_ignored() {
        // 旧 last_seen_version に "2.0.0-prev" のような値が来ても落ちない。
        assert!(highlights_to_show(Some("2.0.0-prev"), "2.0.0", T).is_empty());
        assert_eq!(
            versions(&highlights_to_show(Some("1.5.0-rc1"), "2.0.0", T)),
            ["2.0.0"]
        );
    }

    #[test]
    fn for_version_picks_exact() {
        assert_eq!(versions(&for_version("2.0.0", T)), ["2.0.0"]);
        assert_eq!(versions(&for_version("2.0", T)), ["2.0.0"]);
        assert!(for_version("9.9.9", T).is_empty());
    }

    #[test]
    fn latest_not_newer_than_skips_future_entries() {
        assert_eq!(latest_not_newer_than("0.9.0", T).map(|e| e.version), None);
        assert_eq!(
            latest_not_newer_than("2.1.0", T).map(|e| e.version),
            Some("2.0.0")
        );
        assert_eq!(
            latest_not_newer_than("2.2.0", T).map(|e| e.version),
            Some("2.2.0")
        );
        assert_eq!(latest_not_newer_than("garbage", T).map(|e| e.version), None);
    }

    #[test]
    fn v2_9_grid_selection_upgrade_uses_the_highlight_selection_condition() {
        for (prev, current) in [
            (None, "2.9.0"),
            (Some("2.8.0"), "2.8.1"),
            (Some("2.8.0"), "2.9.0"),
            (Some("2.8.0"), "3.0.0"),
            (Some("2.9.0"), "3.0.0"),
            (Some("3.0.0"), "2.9.0"),
            (Some("invalid"), "2.9.0"),
        ] {
            let highlight_selected = highlights_to_show(prev, current, table())
                .iter()
                .any(|entry| entry.version == GRID_CLICK_SELECTION_EXPLORER_VERSION);
            assert_eq!(
                grid_click_selection_explorer_upgrade_required(prev, current),
                highlight_selected,
                "prev={prev:?}, current={current}"
            );
        }
    }

    #[test]
    fn embedded_table_contains_v2_9_0_must_read_entry() {
        let entries = for_version(GRID_CLICK_SELECTION_EXPLORER_VERSION, table());
        assert_eq!(versions(&entries), [GRID_CLICK_SELECTION_EXPLORER_VERSION]);
        let entry = entries[0];
        // 選択方式の一度きりの切替はこの must_read の存在で告知される (settings.rs の移行判定と
        // 対になる)。同じバージョンに別の must_read が増えても壊れないよう、件数ではなく
        // 「その項目があること」を固定する。
        let selection_notice = entry
            .must_read
            .iter()
            .find(|item| item.title.contains("チェックを解除"))
            .expect("v2.9.0 must announce the grid click selection switch");
        assert!(selection_notice.body.contains("エクスプローラーと同じ動作"));
        assert!(selection_notice.body.contains("チェック方式"));
    }

    #[test]
    fn embedded_table_contains_v2_9_1_must_read_entries() {
        // v2.9.1 は修正リリースだが、既定動作が 3 つ変わっている (Tab / 名前ダイアログ /
        // トレイ格納中の再生継続)。新機能が無いので `highlights` は空で、告知は must_read
        // だけで成立することを固定する。
        let entries = for_version("2.9.1", table());
        assert_eq!(versions(&entries), ["2.9.1"]);
        let entry = entries[0];
        assert!(entry.highlights.is_empty());
        for expected in ["Tab キー", "Windows の画面", "タスクトレイ"] {
            assert!(
                entry
                    .must_read
                    .iter()
                    .any(|item| item.title.contains(expected)),
                "v2.9.1 must announce the {expected} default change"
            );
        }
    }

    #[test]
    fn v2_9_1_is_announced_when_upgrading_from_v2_9_0() {
        // patch 版でもテーブルに載せた以上は更新後初回起動で出ること (parse_version が
        // patch を無視していないことの回帰ガード)。
        let entries = highlights_to_show(Some("2.9.0"), "2.9.1", table());
        assert_eq!(versions(&entries), ["2.9.1"]);
    }

    #[test]
    fn embedded_table_contains_v2_2_0_entry() {
        let entries = for_version("2.2.0", table());
        assert_eq!(versions(&entries), ["2.2.0"]);
        let entry = entries[0];
        assert!(
            entry
                .highlights
                .iter()
                .any(|item| item.title.contains("ショートカット一覧"))
        );
        assert!(
            entry
                .highlights
                .iter()
                .any(|item| item.title.contains("メニュー構成"))
        );
        assert!(
            entry
                .highlights
                .iter()
                .any(|item| item.title.contains("サブフォルダ"))
        );
    }

    #[test]
    fn embedded_table_contains_v2_3_0_entry() {
        let entries = for_version("2.3.0", table());
        assert_eq!(versions(&entries), ["2.3.0"]);
        let entry = entries[0];
        assert_eq!(entry.must_read.len(), 1);
        assert_eq!(
            entry.must_read[0].title,
            "削除するとレーティング・タグ・補正などのデータも一緒に消えます"
        );
        assert_eq!(
            entry.must_read[0].body,
            "これまでは、画像や動画を削除しても、その画像に付けていたレーティング・タグ・補正・\
             回転などのデータが残ってしまうことがありました。今回から、削除するとこれらのデータも\
             一緒に消えます (ごみ箱から戻しても、これらのデータは戻りません)。\
             以前の削除で残ったままになっているデータは、「設定 → サムネイルキャッシュ管理 →\
             メタデータを整理…」でまとめて掃除できます。★の件数が実際のファイル数と合わない場合\
             などにお使いください。\
             ※ 取り外し中の外付けドライブや、接続できないネットワークドライブのデータは、\
             誤って消さないよう対象外にします。"
        );
        assert!(
            entry
                .highlights
                .iter()
                .any(|item| item.title.contains("音楽"))
        );
        assert!(
            entry
                .highlights
                .iter()
                .any(|item| item.title.contains("複数ウィンドウ"))
        );
        // 通知は 2 件 (音楽 / 複数ウィンドウ) に絞る方針。
        assert_eq!(entry.highlights.len(), 2);
    }

    #[test]
    fn embedded_table_contains_v2_4_0_book_page_order_notice() {
        let entries = for_version("2.4.0", table());
        assert_eq!(versions(&entries), ["2.4.0"]);
        let entry = entries[0];
        assert!(
            entry
                .must_read
                .iter()
                .any(|item| item.title == "本のページはファイル名順で進みます"),
            "§1.14 の告知は v2.4.0 の must_read に入れる"
        );
        assert!(
            entry
                .must_read
                .iter()
                .any(|item| item.title.contains("パネル操作"))
        );
        assert!(
            entry
                .highlights
                .iter()
                .any(|item| item.title.contains("クリック表示"))
        );
        let click_to_show = entry
            .highlights
            .iter()
            .find(|item| item.title.contains("クリック表示"))
            .expect("クリック表示モードの highlights がある");
        assert!(click_to_show.body.contains("別のファイルへ移動すると"));
        assert!(!click_to_show.body.contains("入り直したりしても維持"));
    }

    /// 本文は egui が折り返すので、文字列自体に連続空白があってはならない。
    ///
    /// v2.12.0 追加時に行継続のバックスラッシュが失われ、ソースのインデント 23 個が
    /// そのまま本文へ焼き込まれて、実機のダイアログで改行位置が崩れた。表示を目で見ないと
    /// 分からない壊れ方なので、テーブル全体を機械的に検査する。
    #[test]
    fn highlight_text_has_no_baked_in_whitespace_runs() {
        for entry in table() {
            for item in entry.must_read.iter().chain(entry.highlights.iter()) {
                for (label, text) in [("title", item.title), ("body", item.body)] {
                    assert!(
                        !text.contains("  "),
                        "v{} の {label} に連続空白がある (行継続の抜け): {text:?}",
                        entry.version
                    );
                    assert!(
                        !text.chars().any(char::is_control),
                        "v{} の {label} に制御文字がある: {text:?}",
                        entry.version
                    );
                }
            }
        }
    }

    #[test]
    fn embedded_table_contains_v2_12_0_entries() {
        let entries = for_version("2.12.0", table());
        assert_eq!(versions(&entries), ["2.12.0"]);
        let entry = entries[0];
        // 既定と操作が変わったものだけを必読へ置き、新機能は下段に分ける。
        assert_eq!(entry.must_read.len(), 2);
        assert!(entry.must_read[0].body.contains("シャープ拡大"));
        assert!(entry.must_read[1].body.contains("無制限"));
        assert_eq!(entry.highlights.len(), 3);
        assert!(entry.highlights[0].body.contains("Alt"));
    }

    #[test]
    fn embedded_table_contains_v2_11_0_must_read_entries() {
        let entries = for_version("2.11.0", table());
        assert_eq!(versions(&entries), ["2.11.0"]);
        let entry = entries[0];
        assert!(entry.highlights.is_empty());
        assert_eq!(entry.must_read.len(), 2);
        assert!(entry.must_read[0].body.contains("1対1"));
        assert!(entry.must_read[0].body.contains("拡大縮小"));
        assert!(entry.must_read[1].body.contains("縮小時のなめらかさ"));
    }

    #[test]
    fn embedded_table_contains_v3_2_0_folder_thumb_sort_notice() {
        let entries = for_version("3.2.0", table());
        assert_eq!(versions(&entries), ["3.2.0"]);
        let entry = entries[0];
        // 既定が変わるものはすべて must_read にある (highlights へ落ちていない)。
        for keyword in ["代表画像", "バケツ", "消しゴム"] {
            assert!(
                entry
                    .must_read
                    .iter()
                    .any(|item| item.title.contains(keyword)),
                "v3.2.0 must announce the changed default for {keyword}"
            );
        }
        let notice = entry
            .must_read
            .iter()
            .find(|item| item.title.contains("代表画像"))
            .expect("v3.2.0 must announce the folder thumbnail default change");
        assert!(
            notice
                .body
                .contains("環境設定 → フォルダ・ファイル → 「代表画像の選択基準」")
        );
        assert!(notice.body.contains("番号順（区切り無視）"));
    }

    #[test]
    fn embedded_table_is_parseable() {
        // 出荷テーブルの version はすべてパースできること (authoring ミス検出)。
        for e in table() {
            assert!(
                parse_version(e.version).is_some(),
                "version {:?} がパースできない",
                e.version
            );
        }
    }
}

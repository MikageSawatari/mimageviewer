//! 更新後 初回起動の「重要な変更点」表示 (v2.0.0、docs/version-highlights-plan.md)。
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

const TABLE: &[VersionHighlights] = &[VersionHighlights {
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
}];

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

//! ファイル操作の直後に、その結果へカーソルとチェックを移す。
//!
//! 貼り付けや新しいフォルダーの作成でできた項目は、現在のソート順では一覧の離れた
//! 位置に入る。どれが増えたのか分からなくなるので、エクスプローラーと同じように
//! 追加された項目を選択した状態にする (専用スレ >>305)。
//!
//! ## `select_after_load` との住み分け
//!
//! [`crate::app::App::select_after_load`] は「再読込をまたいでカーソルを同じ名前へ
//! 戻す」ためのもので、BS で親へ戻る・ソートを変える・単に読み直す、といった**表示の
//! 都合**に属する。こちらは「たった今の操作がこれを作った」という**操作の結果**で、
//! 名前ではなくパスで持ち、複数件とチェック状態まで扱う。競合したらこちらが勝つ。
//!
//! ## 出力パスの決め方
//!
//! 新しいフォルダーの作成は mIV 自身が作るので出力パスを知っている ([`ExpectedOutputs::Known`])。
//! 貼り付けは Shell の背景 `paste` verb に委ねていて、**mIV は何が作られたかを知らない**
//! (名前が衝突すると Shell が勝手に改名する)。元の名前から推測すると改名結果を取り逃がすので、
//! 操作直前の一覧との差分で判定する ([`ExpectedOutputs::AddedSince`])。
//!
//! ⚠ 差分方式なので、貼り付けと同時に**外部アプリがこのフォルダへ足したファイル**も
//! 追加項目として拾う。これを完全に消すには貼り付け自体を `IFileOperation` +
//! `IFileOperationProgressSink` へ移して実出力を受け取るしかない
//! ([shell-file-operations-context-menu-plan.md](../docs/shell-file-operations-context-menu-plan.md) §7 の残作業)。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 何も現れないまま これだけ経ったら要求を捨てる。
///
/// 貼り付けが空クリップボードや取り消しで何も作らなかった場合に、無関係な外部変更へ
/// 食いつかないための上限。大きい貼り付けは途中の再読込ごとに延長される。
pub(crate) const OUTPUT_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

/// 追加された項目の見分け方。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExpectedOutputs {
    /// mIV が出力パスを知っている操作。
    Known(Vec<PathBuf>),
    /// Shell に委ねた操作。操作直前に一覧にあったパスとの差分を追加とみなす。
    AddedSince(HashSet<PathBuf>),
}

impl ExpectedOutputs {
    fn matches(&self, path: &Path) -> bool {
        match self {
            Self::Known(paths) => paths
                .iter()
                .any(|known| crate::folder_tree::path_eq(known, path)),
            Self::AddedSince(before) => !before
                .iter()
                .any(|old| crate::folder_tree::path_eq(old, path)),
        }
    }
}

/// 操作の結果へ選択を移す要求。**適用先のフォルダを持つので、別の一覧は汚さない。**
#[derive(Debug, Clone)]
pub(crate) struct PostOperationSelection {
    folder: PathBuf,
    expected: ExpectedOutputs,
    /// 前回選択を適用した出力。ここから増えなければ操作は落ち着いたとみなす。
    applied: Vec<PathBuf>,
    expires_at: Instant,
}

impl PostOperationSelection {
    pub(crate) fn new(folder: PathBuf, expected: ExpectedOutputs, now: Instant) -> Self {
        Self {
            folder,
            expected,
            applied: Vec::new(),
            expires_at: now + OUTPUT_WAIT_TIMEOUT,
        }
    }

    /// 適用したことを記録し、次の再読込を待つ期限を延ばす。
    fn note_applied(&mut self, paths: Vec<PathBuf>, now: Instant) {
        self.applied = paths;
        self.expires_at = now + OUTPUT_WAIT_TIMEOUT;
    }
}

/// 再読込のたびに 1 回だけ下す判断。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step {
    /// この index 群へカーソルとチェックを移す。
    Apply(Vec<usize>),
    /// まだ出力が現れていない。要求を残して次の再読込を待つ。
    Wait,
    /// 期限切れ、別のフォルダ、または前回から増えていない。要求を捨てる。
    Drop,
}

/// `visible` は**表示中の**実ファイル項目だけを items 順で渡す。
///
/// 絞り込みで隠れている項目をここへ入れないことが、「フィルタは変えず、表示中の
/// 実出力だけへ適用する」の実装そのもの。呼び出し側で絞り込んでおく。
pub(crate) fn decide(
    request: &PostOperationSelection,
    current_folder: Option<&Path>,
    visible: &[(usize, PathBuf)],
    now: Instant,
) -> Step {
    // 操作完了前に別フォルダへ移った。その一覧の選択には触らない。
    if !current_folder.is_some_and(|folder| crate::folder_tree::path_eq(folder, &request.folder)) {
        return Step::Drop;
    }

    let found: Vec<&(usize, PathBuf)> = visible
        .iter()
        .filter(|(_, path)| request.expected.matches(path))
        .collect();

    if found.is_empty() {
        // まだ書き込み中かもしれない。期限まで待つ。
        return if now >= request.expires_at {
            Step::Drop
        } else {
            Step::Wait
        };
    }

    let paths: Vec<PathBuf> = found.iter().map(|(_, path)| path.clone()).collect();
    if paths == request.applied {
        // 前回適用した集合から増えていない = 操作は落ち着いた。ここで再適用すると、
        // その後ユーザーが自分で変えた選択を無関係な再読込のたびに奪ってしまう。
        return Step::Drop;
    }
    Step::Apply(found.iter().map(|(index, _)| *index).collect())
}

/// 追加項目が決まった後の、カーソル / チェックの置き方。
///
/// 1 件なら**チェックは付けず**カーソルだけ移す。複数件なら全部チェックして、
/// 表示順で先頭へカーソルを置く。表示形式 (サムネイル / 詳細) や選択方式
/// (チェック / エクスプローラー) では変えない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectionPlan {
    /// カーソルと Shift 選択の起点。
    pub(crate) cursor: usize,
    /// チェックする index。1 件のときは空。
    pub(crate) checked: Vec<usize>,
}

pub(crate) fn plan_selection(indices: &[usize]) -> Option<SelectionPlan> {
    let cursor = indices.iter().copied().min()?;
    Some(SelectionPlan {
        cursor,
        checked: if indices.len() <= 1 {
            Vec::new()
        } else {
            indices.to_vec()
        },
    })
}

impl PostOperationSelection {
    /// [`decide`] が `Apply` を返した後に呼ぶ。
    pub(crate) fn record_applied(
        &mut self,
        visible: &[(usize, PathBuf)],
        indices: &[usize],
        now: Instant,
    ) {
        let paths = visible
            .iter()
            .filter(|(index, _)| indices.contains(index))
            .map(|(_, path)| path.clone())
            .collect();
        self.note_applied(paths, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> PathBuf {
        PathBuf::from(r"C:\books")
    }

    fn visible(names: &[&str]) -> Vec<(usize, PathBuf)> {
        names
            .iter()
            .enumerate()
            .map(|(index, name)| (index, folder().join(name)))
            .collect()
    }

    fn known(names: &[&str]) -> ExpectedOutputs {
        ExpectedOutputs::Known(names.iter().map(|name| folder().join(name)).collect())
    }

    fn added_since(names: &[&str]) -> ExpectedOutputs {
        ExpectedOutputs::AddedSince(names.iter().map(|name| folder().join(name)).collect())
    }

    #[test]
    fn a_single_new_item_moves_the_cursor_without_checking_it() {
        let plan = plan_selection(&[7]).unwrap();
        assert_eq!(plan.cursor, 7);
        assert!(
            plan.checked.is_empty(),
            "1 件のときはチェックを付けない (エクスプローラーと同じ)"
        );
    }

    #[test]
    fn several_new_items_are_all_checked_with_the_first_under_the_cursor() {
        // 表示順で先頭 = index の最小。渡す順には依存させない。
        let plan = plan_selection(&[9, 2, 5]).unwrap();
        assert_eq!(plan.cursor, 2);
        assert_eq!(plan.checked, vec![9, 2, 5]);
    }

    #[test]
    fn nothing_to_select_yields_no_plan() {
        assert!(plan_selection(&[]).is_none());
    }

    #[test]
    fn a_created_folder_is_found_wherever_the_sort_put_it() {
        let now = Instant::now();
        let request = PostOperationSelection::new(folder(), known(&["new"]), now);
        // 名前順で末尾に入った場合。
        let step = decide(&request, Some(&folder()), &visible(&["a", "b", "new"]), now);
        assert_eq!(step, Step::Apply(vec![2]));
    }

    #[test]
    fn a_paste_is_recognised_by_what_was_not_there_before() {
        let now = Instant::now();
        let request = PostOperationSelection::new(folder(), added_since(&["a", "b"]), now);
        let step = decide(
            &request,
            Some(&folder()),
            &visible(&["a", "b", "c", "d"]),
            now,
        );
        assert_eq!(step, Step::Apply(vec![2, 3]));
    }

    #[test]
    fn a_name_the_shell_renamed_on_collision_is_still_picked_up() {
        // 貼り付けた元の名前は `a.jpg` だが、衝突して `a (2).jpg` になった。
        // 元の名前で探していたら取り逃がす。
        let now = Instant::now();
        let request = PostOperationSelection::new(folder(), added_since(&["a.jpg"]), now);
        let step = decide(
            &request,
            Some(&folder()),
            &visible(&["a (2).jpg", "a.jpg"]),
            now,
        );
        assert_eq!(step, Step::Apply(vec![0]));
    }

    #[test]
    fn moving_to_another_folder_before_it_finishes_leaves_that_list_alone() {
        let now = Instant::now();
        let request = PostOperationSelection::new(folder(), known(&["new"]), now);
        let elsewhere = PathBuf::from(r"C:\other");
        // 移動先にも同じ名前の項目があっても、選択を奪ってはいけない。
        let step = decide(
            &request,
            Some(&elsewhere),
            &[(0, elsewhere.join("new"))],
            now,
        );
        assert_eq!(step, Step::Drop);
        assert_eq!(decide(&request, None, &[], now), Step::Drop);
    }

    #[test]
    fn an_output_hidden_by_the_filter_is_waited_for_rather_than_forced_into_view() {
        // 絞り込みで隠れている項目は `visible` に入らない。フィルタは変えない。
        let now = Instant::now();
        let request = PostOperationSelection::new(folder(), known(&["new"]), now);
        assert_eq!(
            decide(&request, Some(&folder()), &visible(&["a", "b"]), now),
            Step::Wait
        );
    }

    #[test]
    fn a_paste_that_produced_nothing_is_dropped_instead_of_waiting_forever() {
        let now = Instant::now();
        let request = PostOperationSelection::new(folder(), added_since(&["a"]), now);
        assert_eq!(
            decide(&request, Some(&folder()), &visible(&["a"]), now),
            Step::Wait
        );
        assert_eq!(
            decide(
                &request,
                Some(&folder()),
                &visible(&["a"]),
                now + OUTPUT_WAIT_TIMEOUT
            ),
            Step::Drop,
        );
    }

    #[test]
    fn a_long_paste_keeps_selecting_the_files_as_they_arrive() {
        let now = Instant::now();
        let mut request = PostOperationSelection::new(folder(), added_since(&["a"]), now);

        // 1 回目の再読込では 2 件だけ届いている。
        let first = visible(&["a", "b", "c"]);
        let Step::Apply(indices) = decide(&request, Some(&folder()), &first, now) else {
            panic!("should apply");
        };
        assert_eq!(indices, vec![1, 2]);
        request.record_applied(&first, &indices, now);

        // 2 回目で残りが届く。増えたぶんも選択に入れる。
        let second = visible(&["a", "b", "c", "d"]);
        let later = now + Duration::from_secs(1);
        let Step::Apply(indices) = decide(&request, Some(&folder()), &second, later) else {
            panic!("should apply again");
        };
        assert_eq!(indices, vec![1, 2, 3]);
        request.record_applied(&second, &indices, later);

        // 増えなくなったら手を引く。以後ユーザーが選択を変えても奪い返さない。
        assert_eq!(
            decide(
                &request,
                Some(&folder()),
                &second,
                later + Duration::from_secs(1)
            ),
            Step::Drop,
        );
    }

    #[test]
    fn waiting_does_not_expire_while_files_keep_arriving() {
        let now = Instant::now();
        let mut request = PostOperationSelection::new(folder(), added_since(&["a"]), now);
        let first = visible(&["a", "b"]);
        let Step::Apply(indices) = decide(&request, Some(&folder()), &first, now) else {
            panic!("should apply");
        };
        // 期限ぎりぎりで 1 件届いた。ここから測り直す。
        let late = now + OUTPUT_WAIT_TIMEOUT - Duration::from_secs(1);
        request.record_applied(&first, &indices, late);
        assert_eq!(
            decide(
                &request,
                Some(&folder()),
                &visible(&["a"]),
                late + Duration::from_secs(2)
            ),
            Step::Wait,
            "届き続けている間に期限切れで捨ててはいけない"
        );
    }
}

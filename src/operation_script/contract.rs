use rhai::{AST, Array, Dynamic, Map};

use crate::keymap::{KeyAction, KeyContext};

use super::{MAX_ACTIONS, MAX_STRING_BYTES, SLOT_COUNT};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptContext {
    Grid,
    FsImage,
    FsVideo,
}

impl ScriptContext {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Grid => "grid",
            Self::FsImage => "fs_image",
            Self::FsVideo => "fs_video",
        }
    }

    fn accepts(self, context: KeyContext) -> bool {
        match context {
            KeyContext::Global | KeyContext::Rating => true,
            KeyContext::Grid => self == Self::Grid,
            KeyContext::FsCommon => self != Self::Grid,
            KeyContext::FsImage => self == Self::FsImage,
            KeyContext::FsVideo => self == Self::FsVideo,
            _ => false,
        }
    }
}

/// 設計 §5.1 の A〜E。後続の実行 dispatcher もこの分類を使う。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationKind {
    A,
    B,
    C,
    D,
    E,
}

impl OperationKind {
    pub const fn navigates(self) -> bool {
        matches!(self, Self::B | Self::C)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScriptContexts(u8);

impl ScriptContexts {
    pub const GRID: Self = Self(1);
    pub const IMAGE: Self = Self(2);
    pub const VIDEO: Self = Self(4);
    pub const ALL: Self = Self(7);

    pub fn contains(self, context: ScriptContext) -> bool {
        let mask = match context {
            ScriptContext::Grid => 1,
            ScriptContext::FsImage => 2,
            ScriptContext::FsVideo => 4,
        };
        self.0 & mask != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AllowedOperation {
    pub action: KeyAction,
    pub kind: OperationKind,
    pub event_allowed: bool,
    pub contexts: ScriptContexts,
}

macro_rules! operations {
    ($( $kind:ident, $contexts:ident => [$( $action:ident ),* $(,)?]; )*) => {
        pub const ALLOWED_OPERATIONS: &[AllowedOperation] = &[
            $($(AllowedOperation {
                action: KeyAction::$action,
                kind: OperationKind::$kind,
                // 段4で識別を変えないことをコードとテストで確かめてから付ける。
                event_allowed: false,
                contexts: ScriptContexts::$contexts,
            },)*)*
        ];
    };
}

// KeyAction の全件から推測して許可しない。この表への明示追加だけで公開する。
operations! {
    A, IMAGE => [
        FsSpreadSingle, FsViewSpread, FsSpreadLtr, FsSpreadLtrCover,
        FsSpreadRtl, FsSpreadRtlCover, FsReadingDirectionToggle, FsBgCycle,
        FsFitModeCycle, FsZoomIn, FsZoomOut, FsToggleMetadata, FsNavigatorToggle,
        FsPixelGrid, FsPageTurnAnimationToggle,
    ];
    A, VIDEO => [
        VideoPlayPause, VideoMute, VideoVolumeUp, VideoVolumeDown, VideoLoop,
        VideoSeekStart, VideoSeekBackSmall, VideoSeekForwardSmall,
        VideoSeekBackMedium, VideoSeekForwardMedium, VideoSeekBackLarge,
        VideoSeekForwardLarge, VideoSeekStripToggle,
    ];
    A, GRID => [
        GridColumnCount1, GridColumnCount2, GridColumnCount3, GridColumnCount4,
        GridColumnCount5, GridColumnCount6, GridColumnCount7, GridColumnCount8,
        GridColumnCount9, GridColumnCount10, GridColumnCount11, GridColumnCount12,
        GridColumnCount13, GridColumnCount14, GridColumnCount15, GridColumnCount16,
        GridColumnCount17, GridColumnCount18, GridColumnCount19, GridColumnCount20,
        GridToggleDetailsView,
    ];
    B, IMAGE => [FsPagePrev, FsPageNext, FsJumpFirst, FsJumpLast, FsFixedJumpPrev, FsFixedJumpNext];
    B, GRID => [GridMoveFirst, GridMoveLast];
    C, IMAGE => [FsSiblingPrev, FsSiblingNext, FsCtrlNavPrev, FsCtrlNavNext, FsClose, FsBackToList];
    C, VIDEO => [VideoPrevFile, VideoNextFile, VideoCloseFullscreen];
    C, GRID => [
        GridOpenSelected, GridParentFolder, GridHistoryBack, GridHistoryForward,
        GridSiblingFolderPrev, GridSiblingFolderNext, GridBookPrev, GridBookNext,
        GridOpenFavorite1, GridOpenFavorite2, GridOpenFavorite3, GridOpenFavorite4,
        GridOpenFavorite5, GridOpenFavorite6, GridOpenFavorite7, GridOpenFavorite8,
        GridOpenFavorite9, GridOpenFavorite10, GridOpenFavorite11, GridOpenFavorite12,
        GridOpenFavorite13, GridOpenFavorite14, GridOpenFavorite15, GridOpenFavorite16,
        GridOpenFavorite17, GridOpenFavorite18, GridOpenFavorite19, GridOpenFavorite20,
        GridOpenBook1, GridOpenBook2, GridOpenBook3, GridOpenBook4, GridOpenBook5,
        GridOpenBook6, GridOpenBook7, GridOpenBook8, GridOpenBook9, GridOpenBook10,
        GridOpenBook11, GridOpenBook12, GridOpenBook13, GridOpenBook14, GridOpenBook15,
        GridOpenBook16, GridOpenBook17, GridOpenBook18, GridOpenBook19, GridOpenBook20,
        GridOpenCollection1, GridOpenCollection2, GridOpenCollection3, GridOpenCollection4,
        GridOpenCollection5, GridOpenCollection6, GridOpenCollection7, GridOpenCollection8,
        GridOpenCollection9, GridOpenCollection10, GridOpenCollection11, GridOpenCollection12,
        GridOpenCollection13, GridOpenCollection14, GridOpenCollection15, GridOpenCollection16,
        GridOpenCollection17, GridOpenCollection18, GridOpenCollection19, GridOpenCollection20,
    ];
    D, ALL => [
        RatingItem1, RatingItem2, RatingItem3, RatingItem4, RatingItem5,
        RatingItemClear, RatingItemStepUp, RatingItemStepDown,
        RatingContainer1, RatingContainer2, RatingContainer3, RatingContainer4,
        RatingContainer5, RatingContainerClear, RatingContainerStepUp, RatingContainerStepDown,
        RatingBookPreferred1, RatingBookPreferred2, RatingBookPreferred3,
        RatingBookPreferred4, RatingBookPreferred5, RatingBookPreferredClear,
        RatingBookPreferredStepUp, RatingBookPreferredStepDown,
    ];
    D, GRID => [
        GridTogglePinnedTag1, GridTogglePinnedTag2, GridTogglePinnedTag3, GridTogglePinnedTag4,
        GridTogglePinnedTag5, GridTogglePinnedTag6, GridTogglePinnedTag7, GridTogglePinnedTag8,
        GridTogglePinnedTag9, GridTogglePinnedTag10, GridTogglePinnedTag11, GridTogglePinnedTag12,
        GridTogglePinnedTag13, GridTogglePinnedTag14, GridTogglePinnedTag15, GridTogglePinnedTag16,
        GridTogglePinnedTag17, GridTogglePinnedTag18, GridTogglePinnedTag19, GridTogglePinnedTag20,
    ];
    E, IMAGE => [FsCapture, FsCopyPath, FsCopyFileName, FsCopyImage];
    E, VIDEO => [VideoCapture];
    E, GRID => [GridCopyPath, GridCopyFileName, GridCopyImage];
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptSlot {
    pub number: u8,
    pub label: String,
    pub function: String,
}

pub fn parse_slots(value: Dynamic, ast: &AST) -> Result<Vec<ScriptSlot>, String> {
    let entries = value
        .try_cast::<Array>()
        .ok_or("slots() は配列を返してください")?;
    let mut seen = [false; SLOT_COUNT as usize];
    let mut slots = Vec::new();
    for entry in entries {
        let map = entry
            .try_cast::<Map>()
            .ok_or("slots() の要素はマップにしてください")?;
        let number = map
            .get("slot")
            .and_then(|v| v.as_int().ok())
            .filter(|n| (1..=i64::from(SLOT_COUNT)).contains(n))
            .ok_or("スロット番号は1〜20にしてください")? as u8;
        if std::mem::replace(&mut seen[usize::from(number - 1)], true) {
            return Err(format!("スロット番号 {number} が重複しています"));
        }
        let label = map_string(&map, "label")?;
        let function = map_string(&map, "run")?;
        if !ast
            .iter_functions()
            .any(|f| f.name == function && f.params.len() == 1)
        {
            return Err(format!(
                "スロット {number}: 関数 {function}(s) がありません"
            ));
        }
        slots.push(ScriptSlot {
            number,
            label,
            function,
        });
    }
    slots.sort_by_key(|slot| slot.number);
    Ok(slots)
}

fn map_string(map: &Map, key: &str) -> Result<String, String> {
    let value = map
        .get(key)
        .and_then(|v| v.clone().try_cast::<String>())
        .ok_or_else(|| format!("{key} は文字列にしてください"))?;
    if value.len() > MAX_STRING_BYTES {
        return Err(format!("{key} が文字列の上限を超えています"));
    }
    Ok(value)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptOutput {
    pub actions: Vec<String>,
    pub notice: Option<String>,
}

pub fn parse_output(value: Dynamic) -> Result<ScriptOutput, String> {
    if value.is_unit() {
        return Ok(ScriptOutput::default());
    }
    let (run, notice) = if value.is_map() {
        let mut map = value.cast::<Map>();
        let notice = map_string(&map, "notice")?;
        (
            map.remove("run").ok_or("戻り値に run がありません")?,
            Some(notice),
        )
    } else {
        (value, None)
    };
    let entries = run
        .try_cast::<Array>()
        .ok_or("戻り値の操作は配列にしてください")?;
    let actions = entries
        .into_iter()
        .map(|v| {
            v.try_cast::<String>()
                .filter(|s| s.len() <= MAX_STRING_BYTES)
                .ok_or_else(|| "操作名は上限内の文字列にしてください".to_string())
        })
        .collect::<Result<_, _>>()?;
    Ok(ScriptOutput { actions, notice })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptInvocation {
    Input,
    Event,
}

#[derive(Clone, Copy, Debug)]
pub struct ValidationContext {
    pub context: ScriptContext,
    pub invocation: ScriptInvocation,
    /// 段2の所有passが既存の移動・終了・帰還を集めて渡す (§5.3-7)。
    pub navigation_already_accepted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValidationError {
    NotAllowed { index: usize, name: String },
    WrongContext { index: usize, action: KeyAction },
    MultipleNavigation,
    NavigationNotLast { index: usize },
    MultipleExternalResults,
    EventNotAllowed { index: usize, action: KeyAction },
    TooManyActions { count: usize },
    NavigationConflict,
}

/// 検査済みの並びだけが後続の実行側へ渡る。生成は validate_output に限定する。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedScriptOutput {
    operations: Vec<AllowedOperation>,
    notice: Option<String>,
}

impl ValidatedScriptOutput {
    pub fn operations(&self) -> &[AllowedOperation] {
        &self.operations
    }
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }
}

pub fn validate_output(
    output: ScriptOutput,
    scope: ValidationContext,
) -> Result<ValidatedScriptOutput, ValidationError> {
    if output.actions.len() > MAX_ACTIONS {
        return Err(ValidationError::TooManyActions {
            count: output.actions.len(),
        });
    }
    let operations = output
        .actions
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let operation = ALLOWED_OPERATIONS
                .iter()
                .find(|op| op.action.ini_name() == name)
                .ok_or_else(|| ValidationError::NotAllowed {
                    index,
                    name: name.clone(),
                })?;
            if !operation.contexts.contains(scope.context)
                || !scope.context.accepts(operation.action.context())
            {
                return Err(ValidationError::WrongContext {
                    index,
                    action: operation.action,
                });
            }
            Ok(*operation)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let navigation = operations.iter().filter(|op| op.kind.navigates()).count();
    if navigation > 1 {
        return Err(ValidationError::MultipleNavigation);
    }
    if let Some(index) = operations.iter().position(|op| op.kind.navigates()) {
        if index + 1 != operations.len() {
            return Err(ValidationError::NavigationNotLast { index });
        }
    }
    if operations
        .iter()
        .filter(|op| op.kind == OperationKind::E)
        .count()
        > 1
    {
        return Err(ValidationError::MultipleExternalResults);
    }
    if scope.invocation == ScriptInvocation::Event {
        for (index, op) in operations.iter().enumerate() {
            if op.kind != OperationKind::A || !op.event_allowed {
                return Err(ValidationError::EventNotAllowed {
                    index,
                    action: op.action,
                });
            }
        }
    }
    if scope.navigation_already_accepted && navigation != 0 {
        return Err(ValidationError::NavigationConflict);
    }
    Ok(ValidatedScriptOutput {
        operations,
        notice: output.notice,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation_script::build_engine;

    fn slots(source: &str) -> Result<Vec<ScriptSlot>, String> {
        let engine = build_engine();
        let ast = engine.compile(source).unwrap();
        let value = engine
            .call_fn::<Dynamic>(&mut rhai::Scope::new(), &ast, "slots", ())
            .unwrap();
        parse_slots(value, &ast)
    }

    #[test]
    fn slots_accepts_sparse_numbers_and_sorts() {
        let parsed = slots(
            r#"
            fn slots() { [#{slot:20,label:"last",run:"go"}, #{slot:1,label:"first",run:"go"}] }
            fn go(s) { [] }
        "#,
        )
        .unwrap();
        assert_eq!(parsed.iter().map(|s| s.number).collect::<Vec<_>>(), [1, 20]);
        assert_eq!(parsed[0].label, "first");
        assert_eq!(parsed[1].function, "go");
        assert!(slots("fn slots() { [] }").unwrap().is_empty());
    }

    #[test]
    fn slots_rejects_duplicates_range_missing_function_and_bad_shapes() {
        assert!(slots(r#"fn slots(){[#{slot:1,label:"a",run:"go"},#{slot:1,label:"b",run:"go"}]} fn go(s){[]}"#)
            .unwrap_err().contains("重複"));
        for number in [0, -1, 21] {
            assert!(
                slots(&format!(
                    r#"fn slots(){{[ #{{slot:{number},label:"a",run:"go"}} ]}} fn go(s){{[]}}"#
                ))
                .unwrap_err()
                .contains("1〜20")
            );
        }
        for function in ["", "fn go() { [] }", "fn go(a,b) { [] }"] {
            assert!(
                slots(&format!(
                    r#"fn slots(){{[#{{slot:1,label:"a",run:"go"}}]}} {function}"#
                ))
                .unwrap_err()
                .contains("関数")
            );
        }
        for value in [
            "()",
            "[1]",
            "[#{slot:1,run:\"go\"}]",
            "[#{slot:1,label:42,run:\"go\"}]",
            "[#{slot:1,label:\"a\",run:42}]",
        ] {
            assert!(slots(&format!("fn slots() {{ {value} }} fn go(s) {{ [] }}")).is_err());
        }
    }

    #[test]
    fn output_accepts_array_notice_unit_and_empty() {
        let engine = build_engine();
        let parse = |source| parse_output(engine.eval::<Dynamic>(source).unwrap()).unwrap();
        assert_eq!(parse("()"), ScriptOutput::default());
        assert_eq!(parse("[]"), ScriptOutput::default());
        assert_eq!(
            parse(r#"["RatingItem3","FsPageNext"]"#).actions,
            ["RatingItem3", "FsPageNext"]
        );
        assert_eq!(
            parse(r##"#{run:[],notice:"完了"}"##).notice.as_deref(),
            Some("完了")
        );
    }

    #[test]
    fn output_rejects_bad_shapes_and_oversized_host_strings() {
        let engine = build_engine();
        for source in [
            "42",
            "[42]",
            r##"#{run:[],notice:42}"##,
            r##"#{notice:"ok"}"##,
            r##"#{run:()}"##,
        ] {
            assert!(
                parse_output(engine.eval::<Dynamic>(source).unwrap()).is_err(),
                "{source}"
            );
        }
        assert!(
            parse_output(Dynamic::from(vec![Dynamic::from(
                "x".repeat(MAX_STRING_BYTES + 1)
            )]))
            .is_err()
        );
    }

    fn scope(context: ScriptContext) -> ValidationContext {
        ValidationContext {
            context,
            invocation: ScriptInvocation::Input,
            navigation_already_accepted: false,
        }
    }

    fn output(names: &[&str]) -> ScriptOutput {
        ScriptOutput {
            actions: names.iter().map(|s| (*s).into()).collect(),
            notice: Some("done".into()),
        }
    }

    #[test]
    fn validation_rejects_unlisted_actions() {
        for name in [
            "unknown",
            "GridDelete",
            "ExternalTool1",
            "FsRotateCw",
            "ScriptSlot1",
        ] {
            assert!(matches!(
                validate_output(output(&[name]), scope(ScriptContext::Grid)),
                Err(ValidationError::NotAllowed { .. })
            ));
        }
    }

    #[test]
    fn validation_rejects_wrong_context() {
        for (name, context) in [
            ("FsPageNext", ScriptContext::Grid),
            ("GridTogglePinnedTag1", ScriptContext::FsImage),
            ("FsCapture", ScriptContext::FsVideo),
        ] {
            assert!(matches!(
                validate_output(output(&[name]), scope(context)),
                Err(ValidationError::WrongContext { .. })
            ));
        }
    }

    #[test]
    fn validation_rejects_multiple_or_nonfinal_navigation() {
        assert_eq!(
            validate_output(
                output(&["FsPageNext", "FsClose"]),
                scope(ScriptContext::FsImage)
            ),
            Err(ValidationError::MultipleNavigation)
        );
        assert_eq!(
            validate_output(
                output(&["FsPageNext", "FsBgCycle"]),
                scope(ScriptContext::FsImage)
            ),
            Err(ValidationError::NavigationNotLast { index: 0 })
        );
    }

    #[test]
    fn validation_rejects_multiple_external_results() {
        assert_eq!(
            validate_output(
                output(&["FsCapture", "FsCopyPath"]),
                scope(ScriptContext::FsImage)
            ),
            Err(ValidationError::MultipleExternalResults)
        );
    }

    #[test]
    fn validation_rejects_events_without_audited_mark() {
        for name in [
            "FsBgCycle",
            "FsPageNext",
            "FsClose",
            "RatingItem3",
            "FsCapture",
        ] {
            let mut context = scope(ScriptContext::FsImage);
            context.invocation = ScriptInvocation::Event;
            assert!(matches!(
                validate_output(output(&[name]), context),
                Err(ValidationError::EventNotAllowed { .. })
            ));
        }
        assert!(ALLOWED_OPERATIONS.iter().all(|op| !op.event_allowed));
    }

    #[test]
    fn validation_rejects_over_sixteen_actions_and_navigation_conflict() {
        assert_eq!(
            validate_output(
                output(&["FsBgCycle"; MAX_ACTIONS + 1]),
                scope(ScriptContext::FsImage)
            ),
            Err(ValidationError::TooManyActions { count: 17 })
        );
        let mut context = scope(ScriptContext::FsImage);
        context.navigation_already_accepted = true;
        assert_eq!(
            validate_output(output(&["RatingItem3", "FsPageNext"]), context),
            Err(ValidationError::NavigationConflict)
        );
        assert!(
            validate_output(output(&["FsBgCycle", "RatingItem3", "FsCapture"]), context).is_ok()
        );
    }

    #[test]
    fn validation_accepts_data_then_single_final_navigation_and_notice() {
        let valid = validate_output(
            output(&["RatingItem3", "FsPageNext"]),
            scope(ScriptContext::FsImage),
        )
        .unwrap();
        assert_eq!(
            valid
                .operations()
                .iter()
                .map(|op| op.action)
                .collect::<Vec<_>>(),
            [KeyAction::RatingItem3, KeyAction::FsPageNext]
        );
        assert_eq!(valid.notice(), Some("done"));
        assert!(
            validate_output(ScriptOutput::default(), scope(ScriptContext::Grid))
                .unwrap()
                .operations()
                .is_empty()
        );
        assert!(
            validate_output(
                output(&["FsBgCycle"; MAX_ACTIONS]),
                scope(ScriptContext::FsImage)
            )
            .is_ok()
        );
    }

    #[test]
    fn allowlist_is_unique_and_every_declared_context_matches_keyaction() {
        let mut seen = std::collections::HashSet::new();
        for operation in ALLOWED_OPERATIONS {
            assert!(seen.insert(operation.action));
            for context in [
                ScriptContext::Grid,
                ScriptContext::FsImage,
                ScriptContext::FsVideo,
            ] {
                if operation.contexts.contains(context) {
                    assert!(
                        context.accepts(operation.action.context()),
                        "{:?} / {context:?}",
                        operation.action
                    );
                    assert!(
                        validate_output(output(&[operation.action.ini_name()]), scope(context))
                            .is_ok()
                    );
                }
            }
        }
    }
}

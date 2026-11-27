//! Compile the published HTML bodies themselves, so edits cannot drift from fixtures.
use super::*;

const MANUAL: &str = include_str!("../../htdocs/mimageviewer/manual/operation-script.html");

fn decode_source(body: &str) -> String {
    body.replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn examples() -> Vec<(&'static str, CompiledOperationScript)> {
    MANUAL
        .split("data-operation-example=\"")
        .skip(1)
        .map(|section| {
            let (id, body) = section.split_once("\">").unwrap();
            let (body, _) = body.split_once("</code></pre>").unwrap();
            let source = decode_source(body);
            (
                id,
                CompiledOperationScript::compile(&source)
                    .unwrap_or_else(|error| panic!("{id}: {error}")),
            )
        })
        .collect()
}

#[test]
fn script_363_published_template_matches_builtin_and_evaluates_both_slots_and_event() {
    let (_, body) = MANUAL.split_once("<code data-operation-template>").unwrap();
    let (body, _) = body.split_once("</code></pre>").unwrap();
    let source = decode_source(body);
    assert_eq!(
        source.replace('\r', "").trim(),
        DEFAULT_SCRIPT.replace('\r', "").trim()
    );
    let script = CompiledOperationScript::compile(&source).unwrap();
    let mut image = state(ScriptContext::FsImage);
    assert_eq!(
        script.evaluate_slot(1, &image).unwrap().actions,
        ["FsPageNext"]
    );
    image.page.is_last = Some(true);
    assert_eq!(
        script.evaluate_slot(1, &image).unwrap().actions,
        ["FsSiblingNext"]
    );
    assert_eq!(
        script.evaluate_slot(2, &image).unwrap().actions,
        ["RatingItem3", "FsPageNext"]
    );
    image.book.path = Some("C:\\manga\\book".into());
    check(&script, &image, 1, &["FsReadingDirectionToggle"]);
}

fn state(context: ScriptContext) -> StateSnapshot {
    let mut s = StateSnapshot::new(context);
    s.events = vec![ScriptEvent::Open, ScriptEvent::Page];
    s.item.ext = Some("png".into());
    s.item.rating = Some(0);
    s.item.width = Some(200);
    s.item.height = Some(100);
    s.book.path = Some("C:/manga/LTR-book".into());
    s.page.is_last = Some(false);
    s.view.background = Some("black".into());
    s.view.direction = Some("ltr".into());
    s.view.depth = Some(false);
    s.view.slideshow = Some(false);
    s.video.duration_sec = Some(120.0);
    s.video.position_sec = Some(115.0);
    s.video.muted = Some(true);
    s.video.volume_db = Some(4.5);
    s.grid_columns = Some(4);
    s.grid_details = Some(false);
    s
}

fn check(script: &CompiledOperationScript, s: &StateSnapshot, slot: u8, expected: &[&str]) {
    let output = if script.has_on_event {
        script.evaluate_event(s)
    } else {
        script.evaluate_slot(slot, s)
    }
    .unwrap();
    assert_eq!(
        output.actions, expected,
        "context={:?}, slot={slot}",
        s.context
    );
    let validated = validate_output(
        output,
        ValidationContext {
            context: s.context,
            invocation: if script.has_on_event {
                ScriptInvocation::Event(&s.events)
            } else {
                ScriptInvocation::Input
            },
            navigation_already_accepted: false,
        },
    )
    .unwrap();
    assert_eq!(validated.operations().len(), expected.len());
}

#[test]
fn script_363_all_published_manual_examples_evaluate_with_real_engine_and_contract() {
    let scripts = examples();
    assert_eq!(scripts.len(), 17);
    let cases: &[(&str, ScriptContext, &[&str])] = &[
        ("next-book", ScriptContext::FsImage, &["FsPageNext"]),
        (
            "rate-next",
            ScriptContext::FsImage,
            &["RatingItem3", "FsPageNext"],
        ),
        (
            "named-spread",
            ScriptContext::FsImage,
            &["FsSpreadLtrCover"],
        ),
        (
            "manga-direction",
            ScriptContext::FsImage,
            &["FsReadingDirectionToggle"],
        ),
        (
            "checker-background",
            ScriptContext::FsImage,
            &["FsBgCycle", "FsBgCycle"],
        ),
        ("wide-fit", ScriptContext::FsImage, &["FsFitModeCycle"]),
        (
            "video-remaining",
            ScriptContext::FsVideo,
            &["VideoNextFile"],
        ),
        (
            "volume-three",
            ScriptContext::FsVideo,
            &["VideoVolumeUp", "VideoVolumeUp", "VideoVolumeUp"],
        ),
        ("fixed-columns", ScriptContext::Grid, &[]),
        (
            "copy-next",
            ScriptContext::FsImage,
            &["FsCopyPath", "FsPageNext"],
        ),
        (
            "rate-back",
            ScriptContext::FsImage,
            &["RatingBookPreferred3", "FsBackToList"],
        ),
        ("unrated", ScriptContext::FsImage, &["RatingItem3"]),
        (
            "depth-slideshow",
            ScriptContext::FsImage,
            &["FsDepthParallax", "FsSlideshow"],
        ),
        ("stop-rate", ScriptContext::FsImage, &["RatingItem3"]),
        (
            "unmute-up",
            ScriptContext::FsVideo,
            &["VideoMute", "VideoVolumeUp"],
        ),
        (
            "volume-zero",
            ScriptContext::FsVideo,
            &["VideoVolumeDown", "VideoVolumeDown", "VideoVolumeDown"],
        ),
        ("toggle-columns", ScriptContext::Grid, &["GridColumnCount8"]),
    ];
    for ((id, script), (expected_id, context, expected)) in scripts.iter().zip(cases) {
        assert_eq!(id, expected_id);
        check(script, &state(*context), 1, expected);
        for wrong in [
            ScriptContext::Grid,
            ScriptContext::FsImage,
            ScriptContext::FsVideo,
        ] {
            if wrong != *context {
                check(script, &StateSnapshot::new(wrong), 1, &[]);
            }
        }
    }
}

#[test]
fn script_363_published_examples_cover_branches_unknowns_and_secondary_slots() {
    for (id, script) in examples() {
        let mut image = state(ScriptContext::FsImage);
        let mut video = state(ScriptContext::FsVideo);
        let mut grid = state(ScriptContext::Grid);
        match id {
            "next-book" => {
                image.page.is_last = Some(true);
                check(&script, &image, 1, &["FsSiblingNext"]);
                image.page.is_last = None;
                check(&script, &image, 1, &[]);
            }
            "named-spread" => {
                for (path, expected) in [
                    (Some("RTL-book"), &["FsSpreadRtlCover"][..]),
                    (Some("book"), &["FsSpreadSingle"][..]),
                    (None, &[][..]),
                ] {
                    image.book.path = path.map(Into::into);
                    check(&script, &image, 1, expected);
                }
                // Open does not grant these six spread operations event permission.
                let output = script
                    .evaluate_slot(1, &state(ScriptContext::FsImage))
                    .unwrap();
                assert!(matches!(
                    validate_output(
                        output,
                        ValidationContext {
                            context: ScriptContext::FsImage,
                            invocation: ScriptInvocation::Event(&[ScriptEvent::Open]),
                            navigation_already_accepted: false,
                        }
                    ),
                    Err(ValidationError::EventNotAllowed { .. })
                ));
            }
            "manga-direction" => {
                image.view.direction = Some("rtl".into());
                check(&script, &image, 1, &[]);
                image.view.direction = Some("ltr".into());
                image.events = vec![ScriptEvent::Page];
                check(&script, &image, 1, &[]);
                image.events = vec![ScriptEvent::Open];
                image.book.path = None;
                check(&script, &image, 1, &[]);
            }
            "checker-background" => {
                for (background, expected) in [("white", &["FsBgCycle"][..]), ("checker", &[][..])]
                {
                    image.view.background = Some(background.into());
                    check(&script, &image, 1, expected);
                }
                image.item.ext = Some("jpg".into());
                check(&script, &image, 1, &[]);
            }
            "wide-fit" => {
                image.item.width = None;
                check(&script, &image, 1, &[]);
                image.item.width = Some(50);
                check(&script, &image, 1, &[]);
                image.item.width = Some(200);
                image.events = vec![ScriptEvent::Page];
                check(&script, &image, 1, &[]);
            }
            "video-remaining" => {
                video.video.position_sec = Some(110.0);
                check(&script, &video, 1, &["VideoSeekForwardSmall"]);
                video.video.duration_sec = None;
                check(&script, &video, 1, &[]);
            }
            "volume-three" => check(
                &script,
                &video,
                2,
                &["VideoVolumeDown", "VideoVolumeDown", "VideoVolumeDown"],
            ),
            "fixed-columns" => {
                check(&script, &grid, 2, &["GridColumnCount8"]);
                grid.grid_columns = Some(8);
                check(&script, &grid, 1, &["GridColumnCount4"]);
                check(&script, &grid, 2, &[]);
                grid.grid_details = Some(true);
                check(&script, &grid, 2, &["GridColumnCount8"]);
            }
            "unrated" => {
                image.item.rating = Some(3);
                check(&script, &image, 1, &["FsPageNext"]);
                image.item.rating = None;
                check(&script, &image, 1, &[]);
            }
            "depth-slideshow" => {
                for (depth, slideshow, expected) in [
                    (true, false, &["FsSlideshow"][..]),
                    (false, true, &["FsDepthParallax"][..]),
                    (true, true, &["FsSlideshow", "FsDepthParallax"][..]),
                ] {
                    image.view.depth = Some(depth);
                    image.view.slideshow = Some(slideshow);
                    check(&script, &image, 1, expected);
                }
            }
            "stop-rate" => {
                image.view.slideshow = Some(true);
                check(&script, &image, 1, &["FsSlideshow", "RatingItem3"]);
            }
            "unmute-up" => {
                video.video.muted = Some(false);
                check(&script, &video, 1, &["VideoVolumeUp"]);
                video.video.muted = None;
                check(&script, &video, 1, &[]);
            }
            "volume-zero" => {
                for db in [-80.0, -3.0, 0.0, 0.1, 1.5, 1.6, 6.0, 12.0, 18.0] {
                    video.video.volume_db = Some(db);
                    let mut linear = crate::settings::video_volume_db_to_linear(db);
                    let mut expected = vec![];
                    while crate::settings::video_volume_linear_to_db(linear) > 1.0e-9 {
                        linear = crate::settings::step_video_volume_by_fader_key_step(linear, -1);
                        expected.push("VideoVolumeDown");
                        assert!(expected.len() <= MAX_ACTIONS);
                    }
                    check(&script, &video, 1, &expected);
                }
                video.video.volume_db = None;
                check(&script, &video, 1, &[]);
            }
            "toggle-columns" => {
                grid.grid_columns = Some(8);
                check(&script, &grid, 1, &["GridColumnCount4"]);
                grid.grid_columns = Some(4);
                grid.grid_details = Some(true);
                check(&script, &grid, 1, &["GridColumnCount4"]);
            }
            "rate-next" | "copy-next" | "rate-back" => {}
            _ => panic!("uncovered published example: {id}"),
        }
    }
}

use rhai::{Array, Dynamic, Map};

use super::{MAX_STRING_BYTES, ScriptContext};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptEvent {
    Open,
    View,
    Page,
    Select,
}

impl ScriptEvent {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::View => "view",
            Self::Page => "page",
            Self::Select => "select",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct WindowSnapshot {
    pub fullscreen: Option<bool>,
    pub mode: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ItemSnapshot {
    pub name: Option<String>,
    pub ext: Option<String>,
    pub path: Option<String>,
    pub kind: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub rating: Option<i64>,
    /// キャッシュにある場合だけ Some。配列の位置0がピン留めタグ1を表す。
    pub pinned_tags: Option<[bool; 20]>,
}

#[derive(Clone, Debug, Default)]
pub struct BookSnapshot {
    pub path: Option<String>,
    pub kind: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct PageSnapshot {
    pub index: Option<i64>,
    pub count: Option<i64>,
    pub is_first: Option<bool>,
    pub is_last: Option<bool>,
}

#[derive(Clone, Debug, Default)]
pub struct ViewSnapshot {
    pub spread: Option<String>,
    pub direction: Option<String>,
    pub background: Option<String>,
    pub depth: Option<bool>,
    pub slideshow: Option<bool>,
}

#[derive(Clone, Debug, Default)]
pub struct VideoSnapshot {
    pub playing: Option<bool>,
    pub position_sec: Option<f64>,
    pub duration_sec: Option<f64>,
    pub muted: Option<bool>,
    pub volume_db: Option<f64>,
}

/// すべて呼び出し側が保持済みの値。変換はI/OもAppへの参照も持たない。
#[derive(Clone, Debug)]
pub struct StateSnapshot {
    pub context: ScriptContext,
    pub events: Vec<ScriptEvent>,
    pub window: WindowSnapshot,
    pub item: ItemSnapshot,
    pub book: BookSnapshot,
    pub page: PageSnapshot,
    pub view: ViewSnapshot,
    pub video: VideoSnapshot,
    pub grid_selected_count: Option<i64>,
    pub grid_columns: Option<i64>,
    pub grid_details: Option<bool>,
}

fn string(value: &Option<String>) -> Dynamic {
    value
        .as_ref()
        .filter(|s| s.len() <= MAX_STRING_BYTES)
        .map_or(Dynamic::UNIT, |s| Dynamic::from(s.clone()))
}

fn optional<T: Into<Dynamic> + Copy>(value: Option<T>) -> Dynamic {
    value.map_or(Dynamic::UNIT, Into::into)
}

fn float(value: Option<f64>) -> Dynamic {
    optional(value.filter(|n| n.is_finite()))
}

fn map<const N: usize>(entries: [(&str, Dynamic); N]) -> Dynamic {
    Dynamic::from(
        entries
            .into_iter()
            .map(|(k, v)| (k.into(), v))
            .collect::<Map>(),
    )
}

impl StateSnapshot {
    pub fn new(context: ScriptContext) -> Self {
        Self {
            context,
            events: Vec::new(),
            window: WindowSnapshot::default(),
            item: ItemSnapshot::default(),
            book: BookSnapshot::default(),
            page: PageSnapshot::default(),
            view: ViewSnapshot::default(),
            video: VideoSnapshot::default(),
            grid_selected_count: None,
            grid_columns: None,
            grid_details: None,
        }
    }

    pub fn to_dynamic(&self) -> Dynamic {
        let tags = self.item.pinned_tags.map_or(Dynamic::UNIT, |tags| {
            Dynamic::from(
                tags.into_iter()
                    .enumerate()
                    .filter(|(_, on)| *on)
                    .map(|(index, _)| Dynamic::from(index as i64 + 1))
                    .collect::<Array>(),
            )
        });
        // events は型の4種類だけ。重複した値が入力されても公開するのは1つずつ。
        let events = [
            ScriptEvent::Open,
            ScriptEvent::View,
            ScriptEvent::Page,
            ScriptEvent::Select,
        ]
        .into_iter()
        .filter(|event| self.events.contains(event))
        .map(|event| Dynamic::from(event.name()))
        .collect::<Array>();
        map([
            ("api", Dynamic::from(1_i64)),
            ("context", Dynamic::from(self.context.name())),
            ("events", Dynamic::from(events)),
            (
                "window",
                map([
                    ("fullscreen", optional(self.window.fullscreen)),
                    ("mode", string(&self.window.mode)),
                ]),
            ),
            (
                "item",
                map([
                    ("name", string(&self.item.name)),
                    ("ext", string(&self.item.ext)),
                    ("path", string(&self.item.path)),
                    ("kind", string(&self.item.kind)),
                    ("width", optional(self.item.width)),
                    ("height", optional(self.item.height)),
                    ("rating", optional(self.item.rating)),
                    ("pinned_tags", tags),
                ]),
            ),
            (
                "book",
                map([
                    ("path", string(&self.book.path)),
                    ("kind", string(&self.book.kind)),
                ]),
            ),
            (
                "page",
                map([
                    ("index", optional(self.page.index)),
                    ("count", optional(self.page.count)),
                    ("is_first", optional(self.page.is_first)),
                    ("is_last", optional(self.page.is_last)),
                ]),
            ),
            (
                "view",
                map([
                    ("spread", string(&self.view.spread)),
                    ("direction", string(&self.view.direction)),
                    ("background", string(&self.view.background)),
                    ("depth", optional(self.view.depth)),
                    ("slideshow", optional(self.view.slideshow)),
                ]),
            ),
            (
                "video",
                map([
                    ("playing", optional(self.video.playing)),
                    ("position_sec", float(self.video.position_sec)),
                    ("duration_sec", float(self.video.duration_sec)),
                    ("muted", optional(self.video.muted)),
                    ("volume_db", float(self.video.volume_db)),
                ]),
            ),
            (
                "grid",
                map([
                    ("selected_count", optional(self.grid_selected_count)),
                    ("columns", optional(self.grid_columns)),
                    ("details", optional(self.grid_details)),
                ]),
            ),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation_script::{MAX_COLLECTION_SIZE, MAX_OPERATIONS, build_engine};

    #[test]
    fn snapshot_all_fields_and_maximum_events_tags_fit_sandbox_limits() {
        let engine = build_engine();
        assert_eq!(engine.max_operations(), MAX_OPERATIONS);
        assert_eq!(engine.max_array_size(), MAX_COLLECTION_SIZE);
        assert_eq!(engine.max_map_size(), MAX_COLLECTION_SIZE);
        let ast = engine.compile(r#"
            fn inspect_state(s) {
                let checks = [
                    s.api == 1 && s.events.len() == 4 && "select" in s.events,
                    s.item.pinned_tags.len() == 20 && 20 in s.item.pinned_tags,
                    s.window.mode == "fullscreen" && s.window.fullscreen,
                    s.item.name == "page.png" && s.item.ext == "png"
                        && s.item.path == "C:/manga/book/page.png" && s.item.kind == "image",
                    s.item.width == 1200 && s.item.height == 1800 && s.item.rating == 3,
                    s.book.path == "C:/manga/book" && s.book.kind == "folder",
                    s.page.index == 1 && s.page.count == 2 && !s.page.is_first && s.page.is_last,
                    s.view.spread == "single" && s.view.direction == "rtl" && s.view.background == "black"
                        && s.view.depth && !s.view.slideshow,
                    s.video.playing && s.video.position_sec == 1.5 && s.video.duration_sec == 60.0
                        && s.video.muted && s.video.volume_db == -3.0,
                    s.grid.selected_count == 1 && s.grid.columns == 8 && s.grid.details,
                ];
                !(false in checks)
            }
        "#).unwrap();
        for context in [
            ScriptContext::Grid,
            ScriptContext::FsImage,
            ScriptContext::FsVideo,
        ] {
            let mut snapshot = StateSnapshot::new(context);
            snapshot.window = WindowSnapshot {
                fullscreen: Some(true),
                mode: Some("fullscreen".into()),
            };
            snapshot.item = ItemSnapshot {
                name: Some("page.png".into()),
                ext: Some("png".into()),
                path: Some("C:/manga/book/page.png".into()),
                kind: Some("image".into()),
                width: Some(1200),
                height: Some(1800),
                rating: Some(3),
                pinned_tags: Some([true; 20]),
            };
            snapshot.book = BookSnapshot {
                path: Some("C:/manga/book".into()),
                kind: Some("folder".into()),
            };
            snapshot.page = PageSnapshot {
                index: Some(1),
                count: Some(2),
                is_first: Some(false),
                is_last: Some(true),
            };
            snapshot.view = ViewSnapshot {
                spread: Some("single".into()),
                direction: Some("rtl".into()),
                background: Some("black".into()),
                depth: Some(true),
                slideshow: Some(false),
            };
            snapshot.video = VideoSnapshot {
                playing: Some(true),
                position_sec: Some(1.5),
                duration_sec: Some(60.0),
                muted: Some(true),
                volume_db: Some(-3.0),
            };
            snapshot.grid_selected_count = Some(1);
            snapshot.grid_columns = Some(8);
            snapshot.grid_details = Some(true);
            snapshot.events = vec![ScriptEvent::Open; MAX_COLLECTION_SIZE + 1];
            snapshot
                .events
                .extend([ScriptEvent::View, ScriptEvent::Page, ScriptEvent::Select]);
            let value = snapshot.to_dynamic();
            // Check the whole nested value, not just each individual section.
            engine.ensure_data_size_within_limits(&value).unwrap();
            assert!(
                engine
                    .call_fn::<bool>(&mut rhai::Scope::new(), &ast, "inspect_state", (value,))
                    .unwrap()
            );
        }
    }

    #[test]
    fn snapshot_contexts_unknowns_and_values() {
        for context in [
            ScriptContext::Grid,
            ScriptContext::FsImage,
            ScriptContext::FsVideo,
        ] {
            let mut snapshot = StateSnapshot::new(context);
            snapshot.window.fullscreen = Some(true);
            snapshot.item.rating = Some(3);
            snapshot.page.index = Some(2);
            snapshot.video.position_sec = Some(1.5);
            snapshot.grid_selected_count = Some(4);
            snapshot.events = vec![ScriptEvent::Page, ScriptEvent::Page, ScriptEvent::View];
            let mut tags = [false; 20];
            tags[0] = true;
            tags[19] = true;
            snapshot.item.pinned_tags = Some(tags);
            let value = snapshot.to_dynamic().cast::<Map>();
            assert_eq!(value.len(), 10);
            assert_eq!(value["api"].as_int().unwrap(), 1);
            assert_eq!(value["context"].clone().cast::<String>(), context.name());
            assert_eq!(value["events"].clone().cast::<Array>().len(), 2);
            let item = value["item"].clone().cast::<Map>();
            assert!(item["width"].is_unit());
            assert_eq!(item["rating"].as_int().unwrap(), 3);
            let tags = item["pinned_tags"].clone().cast::<Array>();
            assert_eq!(
                tags.iter().map(|n| n.as_int().unwrap()).collect::<Vec<_>>(),
                [1, 20]
            );
            assert_eq!(
                value["page"].clone().cast::<Map>()["index"]
                    .as_int()
                    .unwrap(),
                2
            );
            assert_eq!(
                value["video"].clone().cast::<Map>()["position_sec"]
                    .as_float()
                    .unwrap(),
                1.5
            );
            assert_eq!(
                value["grid"].clone().cast::<Map>()["selected_count"]
                    .as_int()
                    .unwrap(),
                4
            );
        }
    }

    #[test]
    fn snapshot_strings_over_byte_limit_become_unit() {
        let mut snapshot = StateSnapshot::new(ScriptContext::Grid);
        snapshot.item.path = Some("a".repeat(MAX_STRING_BYTES));
        assert!(
            snapshot.to_dynamic().cast::<Map>()["item"]
                .clone()
                .cast::<Map>()["path"]
                .is_string()
        );
        let oversized = Some("界".repeat(MAX_STRING_BYTES / 3 + 1));
        snapshot.item.path = oversized.clone();
        snapshot.item.name = oversized.clone();
        snapshot.item.ext = oversized.clone();
        snapshot.item.kind = oversized.clone();
        snapshot.book.path = oversized.clone();
        snapshot.book.kind = oversized.clone();
        snapshot.view.spread = oversized.clone();
        snapshot.view.direction = oversized.clone();
        snapshot.view.background = oversized.clone();
        snapshot.window.mode = oversized;
        let result = snapshot.to_dynamic().cast::<Map>();
        for (section, keys) in [
            ("item", &["name", "ext", "path", "kind"][..]),
            ("book", &["path", "kind"][..]),
            ("view", &["spread", "direction", "background"][..]),
            ("window", &["mode"][..]),
        ] {
            let section = result[section].clone().cast::<Map>();
            for key in keys {
                assert!(section[*key].is_unit());
            }
        }
        assert!(result["item"].clone().cast::<Map>()["pinned_tags"].is_unit());
    }
}

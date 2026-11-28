use super::*;
use crate::{ResponseKind, help_build, schema, serialize_response, success};
use mimageviewer_agent_ipc::MAX_MCP_RESPONSE_CHARS;
use std::collections::BTreeSet;

const FIXTURE: &str = r#"<!doctype html><html><head><title>Wrong title</title></head><body>
<nav>OUTSIDE_MENU</nav><aside class="sidebar">OUTSIDE_SIDEBAR</aside>
<main class="content other"><h1>Fixture &amp; manual</h1>
<nav class="breadcrumb">INSIDE_BREADCRUMB</nav><div class="menu">INSIDE_MENU</div>
<aside class="sidebar">INSIDE_SIDEBAR</aside><script>SCRIPT_SENTINEL</script><style>STYLE_SENTINEL</style>
<p>Intro <kbd>Ctrl+G</kbd> <a href="secret-url">link label</a> <img src="secret-image" alt="image label">.</p>
<section><h2 id="explicit">First &amp; heading</h2><p>Japanese 日本語 &lt;literal&gt;.</p>
<table><tr><th>One</th><th>Two</th></tr><tr><td><kbd>A</kbd></td><td>B &amp; C</td></tr></table></section>
<h2>Second heading</h2><pre>line one
  line two</pre></main><footer>OUTSIDE_FOOTER</footer></body></html>"#;

fn manual_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../htdocs/mimageviewer/manual")
}

#[test]
fn operation_script_help_is_searchable_and_retrievable_offline() {
    let found = call(
        "search_help",
        json!({"query":"操作のスクリプト","limit":30}),
    )
    .unwrap();
    assert_eq!(found["isError"], false);
    let hit = found["structuredContent"]["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|hit| {
            hit["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("operation-script.html#"))
        })
        .expect("operation-script manual is discoverable from the tool description");
    let section = call("get_help", json!({"id":hit["id"]})).unwrap();
    assert_eq!(section["isError"], false);
    assert!(
        !section["structuredContent"]["text"]
            .as_str()
            .unwrap()
            .is_empty()
    );
    let page = call("get_help", json!({"id":"operation-script.html"})).unwrap();
    assert_eq!(page["isError"], false);
    assert!(
        !page["structuredContent"]["sections"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn sound_chain_manual_conditions_are_searchable_and_retrievable_offline() {
    let section = call("get_help", json!({"id":"ai-integration.html#sound"})).unwrap();
    assert_eq!(section["isError"], false);
    let text = section["structuredContent"]["text"].as_str().unwrap();
    for condition in [
        "ロックを解除",
        "A を残して B に作って",
        "A と同じくらいの音量",
        "なめらかにして",
        "古いレコードでシャーという音が楽器の音より高い帯域に多い場合は、そこを削ると減らせることがあります",
        "削りすぎるとこもるので、聴いて加減を伝えてください",
        "消えないこともあります",
        "同じロックを解除する前",
        "2 秒を超える",
        "MCP:",
    ] {
        assert!(text.contains(condition), "{condition}");
    }
    let search = call("search_help", json!({"query":"ロック","limit":30})).unwrap();
    assert!(
        search["structuredContent"]["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|hit| { hit["id"] == "ai-integration.html#sound" })
    );
}

#[test]
fn help_generation_is_deterministic_and_ids_are_unique() {
    let first = help_build::load_manual(&manual_root()).unwrap();
    let second = help_build::load_manual(&manual_root()).unwrap();
    assert_eq!(first, second);
    let generated = help_build::generate(&first);
    assert_eq!(generated, help_build::generate(&second));
    assert_eq!(
        generated,
        include_str!(concat!(env!("OUT_DIR"), "/bundled_help.rs"))
    );
    help_build::validate_ids(&first).unwrap();
    let ids = PAGES.iter().map(|p| p.id).collect::<BTreeSet<_>>();
    assert!(ids.contains("index.html"));
    assert!(ids.contains("sound/index.html"));
    assert!(PAGES.iter().all(|p| !p.id.ends_with("changelog.html")));
    assert_eq!(first.len(), PAGES.len());
    let mut section_count = 0;
    let mut part_count = 0;
    let mut text_bytes = 0;
    for (built, page) in first.iter().zip(PAGES) {
        assert_eq!(built.id, page.id);
        assert_eq!(built.title, page.title);
        assert_eq!(built.sections.len(), page.sections.len());
        for (built, section) in built.sections.iter().zip(page.sections) {
            assert_eq!(built.id, section.id);
            assert_eq!(built.heading, section.heading);
            assert!(
                built
                    .parts
                    .iter()
                    .map(String::as_str)
                    .eq(section.parts.iter().copied())
            );
            section_count += 1;
            part_count += section.parts.len();
            text_bytes += section.parts.iter().map(|p| p.len()).sum::<usize>();
        }
    }
    eprintln!(
        "Bundled help: {} pages, {section_count} sections, {part_count} parts, {text_bytes} text bytes, {} generated source bytes",
        PAGES.len(),
        generated.len()
    );
}

#[test]
fn help_extraction_keeps_only_main_body_and_converts_html() {
    let page = help_build::extract_page("sound/index.html", FIXTURE).unwrap();
    assert_eq!(page.title, "Fixture & manual");
    assert_eq!(
        page.sections
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        [
            "sound/index.html#0",
            "sound/index.html#explicit",
            "sound/index.html#2"
        ]
    );
    let body = page
        .sections
        .iter()
        .flat_map(|s| &s.parts)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in [
        "OUTSIDE_",
        "INSIDE_",
        "SCRIPT_SENTINEL",
        "STYLE_SENTINEL",
        "secret-url",
        "secret-image",
        "href=",
        "<kbd>",
    ] {
        assert!(!body.contains(forbidden), "{forbidden}: {body}");
    }
    for expected in [
        "Ctrl+G",
        "link label",
        "image label",
        "日本語 <literal>",
        "One | Two",
        "A | B & C",
        "line one\n  line two",
    ] {
        assert!(body.contains(expected), "{expected}: {body}");
    }
}

#[test]
fn help_empty_overview_is_omitted_without_reusing_or_renumbering_ids() {
    // h1 supplies the page title, but belongs to the first h2 section here.
    let source = "<main class='content'> \n<nav>menu</nav><h2 id='first'>First</h2><h1>Title</h1><p>Body</p><h2>Second</h2><p>More</p></main>";
    let page = help_build::extract_page("start.html", source).unwrap();
    assert_eq!(
        page.sections
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        ["start.html#first", "start.html#2"]
    );
    assert!(
        page.sections
            .iter()
            .all(|s| s.parts.iter().any(|p| !p.is_empty()))
    );
    let generated = help_build::generate(&[page]);
    assert!(!generated.contains("start.html#0"));
    // A numeric h2 id cannot take the reserved overview id, even if empty.
    let collision = source.replace("id='first'", "id='0'");
    assert!(
        help_build::extract_page("start.html", &collision)
            .unwrap_err()
            .contains("duplicate help id")
    );
    let no_anchor = source.replace(" id='first'", "");
    let page = help_build::extract_page("start.html", &no_anchor).unwrap();
    assert_eq!(page.sections[0].id, "start.html#1");
    assert_eq!(page.sections[1].id, "start.html#2");
}

#[test]
fn help_page_listings_and_search_do_not_expose_empty_sections() {
    let has_body = |id: &str| {
        PAGES
            .iter()
            .flat_map(|p| p.sections)
            .find(|s| s.id == id)
            .is_some_and(|s| s.parts.iter().any(|p| !p.trim().is_empty()))
    };
    for page in PAGES {
        let result = call("get_help", json!({"id":page.id})).unwrap();
        for row in result["structuredContent"]["sections"].as_array().unwrap() {
            assert!(has_body(row["id"].as_str().unwrap()), "{row}");
        }
    }
    for query in ["概要", "の", "mIV", "設定"] {
        let result = call("search_help", json!({"query":query,"limit":30})).unwrap();
        for row in result["structuredContent"]["results"].as_array().unwrap() {
            assert!(has_body(row["id"].as_str().unwrap()), "{row}");
        }
    }
}

#[test]
fn help_extraction_fails_on_missing_inputs_or_duplicate_ids() {
    let fixture = tempfile::tempdir().unwrap();
    let missing = fixture.path().join("absent");
    assert!(help_build::load_manual(&missing).is_err());
    std::fs::create_dir(fixture.path().join("sound")).unwrap();
    std::fs::write(fixture.path().join("bad.html"), "<h1>No main</h1>").unwrap();
    assert!(
        help_build::load_manual(fixture.path())
            .unwrap_err()
            .contains("missing <main")
    );
    assert!(help_build::extract_page("bad.html", "<main><h1>Wrong class</h1></main>").is_err());
    std::fs::write(fixture.path().join("bad.html"), FIXTURE).unwrap();
    std::fs::write(fixture.path().join("changelog.html"), "no main").unwrap();
    std::fs::write(fixture.path().join("sound/changelog.html"), "no main").unwrap();
    assert_eq!(help_build::load_manual(fixture.path()).unwrap().len(), 1);
    for headings in [
        "<h2 id='same'>A</h2><h2 id='same'>B</h2>",
        "<h2 id='0'>Overview collision</h2>",
        "<h2 id='2'>A</h2><h2>B</h2>",
    ] {
        let source = format!("<main class='content'><h1>Title</h1>{headings}</main>");
        assert!(
            help_build::extract_page("bad.html", &source)
                .unwrap_err()
                .contains("duplicate help id")
        );
    }
}

#[test]
fn help_parts_split_at_paragraph_and_table_row_boundaries() {
    let paragraph = "段".repeat(6_000);
    let row = "行".repeat(6_000);
    let html = format!(
        "<main class='content'><h1>Title</h1><h2 id='long'>Long</h2><p>{paragraph}</p><table><tr><td>{row}</td></tr></table><p>last</p></main>"
    );
    let page = help_build::extract_page("long.html", &html).unwrap();
    let parts = &page.sections[1].parts;
    assert_eq!(parts, &[paragraph, format!("{row}\n\nlast")]);
    assert!(
        parts
            .iter()
            .all(|p| p.chars().count() <= help_build::MAX_PART_CHARS)
    );
    let oversized = format!(
        "<main class='content'><h1>Title</h1><p>{}</p></main>",
        "x".repeat(12_001)
    );
    assert!(help_build::extract_page("oversized.html", &oversized).is_err());
}

const SEARCH_PAGES: &[HelpPage] = &[
    HelpPage {
        id: "a.html",
        title: "Other",
        sections: &[HelpSection {
            id: "a.html#1",
            heading: "Alpha 日本語",
            parts: &["A body", "second body"],
        }],
    },
    HelpPage {
        id: "b.html",
        title: "ALPHA",
        sections: &[HelpSection {
            id: "b.html#1",
            heading: "Other",
            parts: &["日本語 body"],
        }],
    },
    HelpPage {
        id: "c.html",
        title: "Other",
        sections: &[
            HelpSection {
                id: "c.html#1",
                heading: "Other",
                parts: &["alpha alpha 日本語 body"],
            },
            HelpSection {
                id: "c.html#2",
                heading: "Other",
                parts: &["alpha 日本語 body"],
            },
            HelpSection {
                id: "c.html#3",
                heading: "Other",
                parts: &["alpha absent"],
            },
        ],
    },
];

#[test]
fn help_search_ands_terms_folds_only_ascii_ranks_and_bounds_snippets() {
    let result = search(SEARCH_PAGES, &["alpha".into(), "日本語".into()], 30);
    assert_eq!(
        result["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["a.html#1", "b.html#1", "c.html#1", "c.html#2"]
    );
    let upper = call(
        "search_help",
        json!({"query":"　CTRL+G \n 検索","limit":30}),
    )
    .unwrap();
    let lower = call("search_help", json!({"query":"ctrl+g 検索","limit":30})).unwrap();
    assert_eq!(upper, lower);
    assert!(
        !lower["structuredContent"]["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    for result in result["results"].as_array().unwrap() {
        assert!(result["snippet"].as_str().unwrap().chars().count() <= 200);
    }
    let text = format!("{}日本語{}", "前".repeat(300), "後".repeat(300));
    let excerpt = snippet(&text, &["日本語".into()]);
    assert_eq!(excerpt.chars().count(), 200);
    assert!(excerpt.contains("日本語"));
    assert!(
        search(SEARCH_PAGES, &["ALPHA".into()], 30)["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        call("search_help", json!({"query":"の"})).unwrap()["structuredContent"]["results"]
            .as_array()
            .unwrap()
            .len(),
        10
    );
    assert_eq!(call("search_help", json!({"query":"の","limit":100})).unwrap()["structuredContent"]["results"].as_array().unwrap().len(), 30);
}

#[test]
fn help_part_retrieval_and_invalid_arguments() {
    let second = get(SEARCH_PAGES, Some("a.html#1"), 2).unwrap();
    assert_eq!(second["text"], "second body");
    assert_eq!(second["part"], 2);
    assert_eq!(second["parts"], 2);
    for (name, args) in [
        ("search_help", json!({})),
        ("search_help", json!({"query":" \n　"})),
        ("search_help", json!({"query":false})),
        ("search_help", json!({"query":"x","limit":0})),
        ("search_help", json!({"query":"x","limit":-1})),
        ("search_help", json!({"query":"x","cursor":"x"})),
        ("get_help", json!({"part":0})),
        ("get_help", json!({"part":2})),
        ("get_help", json!({"id":"index.html","part":2})),
        ("get_help", json!({"id":1})),
        ("get_help", json!({"cursor":"x"})),
    ] {
        assert!(call(name, args).is_err(), "{name}");
    }
    for args in [
        json!({"id":"missing.html"}),
        json!({"id":PAGES[0].sections[0].id,"part":u32::MAX}),
    ] {
        let result = call("get_help", args).unwrap();
        assert_eq!(result["isError"], true);
        assert_eq!(
            result["structuredContent"]["app_version"],
            env!("CARGO_PKG_VERSION")
        );
    }
}

#[test]
fn help_search_bounds_query_characters_and_whitespace_terms_before_search() {
    let tool = schema::tools()
        .into_iter()
        .find(|t| t["name"] == "search_help")
        .unwrap();
    let query_schema = &tool["inputSchema"]["properties"]["query"];
    assert_eq!(query_schema["maxLength"], MAX_HELP_QUERY_CHARS);
    assert!(query_schema.get("pattern").is_none());
    assert!(
        tool["description"]
            .as_str()
            .unwrap()
            .contains("Max 8 terms.")
    );
    for scalar in ["a", "猫", "𠮷"] {
        let boundary = json!({"query":scalar.repeat(MAX_HELP_QUERY_CHARS)});
        assert!(crate::tests::accepts_schema(
            &tool["inputSchema"],
            &boundary
        ));
        assert!(call("search_help", boundary).is_ok());
        let over = json!({"query":scalar.repeat(MAX_HELP_QUERY_CHARS + 1)});
        assert!(!crate::tests::accepts_schema(&tool["inputSchema"], &over));
        assert_eq!(
            call("search_help", over).unwrap_err(),
            "Help query must be at most 200 characters"
        );
    }
    for separator in [" ", "\t", "\n", "　", "\u{85}"] {
        let eight = vec!["の"; MAX_HELP_QUERY_TERMS].join(separator);
        assert!(call("search_help", json!({"query":eight})).is_ok());
        let nine = vec!["の"; MAX_HELP_QUERY_TERMS + 1].join(separator);
        assert_eq!(
            call("search_help", json!({"query":nine})).unwrap_err(),
            "Specify at most 8 help query terms"
        );
    }
    // FEFF is not Rust whitespace; U+0085 above is. Leave term splitting to
    // the runtime instead of a schema regex with a different Unicode set.
    assert!(
        call(
            "search_help",
            json!({"query": vec!["a"; MAX_HELP_QUERY_TERMS + 1].join("\u{feff}")})
        )
        .is_ok()
    );
    // Both boundaries at once: seven short words and a final long word.
    let boundary = format!("{}{}", "a ".repeat(7), "a".repeat(186));
    assert_eq!(boundary.chars().count(), MAX_HELP_QUERY_CHARS);
    assert!(call("search_help", json!({"query":boundary})).is_ok());
    assert_eq!(
        call("search_help", json!({"query":"a ".repeat(200_000)})).unwrap_err(),
        "Help query must be at most 200 characters"
    );
}

fn assert_final_budget(name: &str, result: &Value) -> usize {
    let content = &result["structuredContent"];
    assert_eq!(content["app_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(result["isError"], false, "{name}: {result}");
    assert_eq!(
        serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap()).unwrap(),
        *content
    );
    let chars = mcp_content_chars(content).unwrap();
    assert!(chars <= MAX_PAGE_CHARS, "{name}: {chars}");
    let tool = schema::tools()
        .into_iter()
        .find(|t| t["name"] == name)
        .unwrap();
    assert!(crate::tests::accepts_schema(&tool["outputSchema"], content));
    for modern in [false, true] {
        let envelope = success(json!("\0".repeat(256)), result.clone(), modern);
        let raw = envelope.to_string();
        assert!(
            raw.chars().count() <= MAX_MCP_RESPONSE_CHARS,
            "{name}: {}",
            raw.chars().count()
        );
        assert!(raw.len() + 1 <= mimageviewer_agent_ipc::MAX_FRAME_BYTES);
        let serialized = serialize_response(&envelope, ResponseKind::Data);
        assert!(serialized.chars().count() <= MAX_MCP_RESPONSE_CHARS);
        assert_eq!(
            serde_json::from_str::<Value>(&serialized).unwrap(),
            envelope
        );
    }
    chars
}

#[test]
fn help_every_page_section_and_part_is_retrievable_with_final_budgets() {
    let result = call("get_help", json!({})).unwrap();
    let pages_chars = assert_final_budget("get_help", &result);
    assert_eq!(
        result["structuredContent"]["pages"]
            .as_array()
            .unwrap()
            .len(),
        PAGES.len()
    );
    let mut largest_part = 0;
    let mut largest_section_list = 0;
    for page in PAGES {
        let result = call("get_help", json!({"id":page.id})).unwrap();
        largest_section_list = largest_section_list.max(assert_final_budget("get_help", &result));
        assert_eq!(
            result["structuredContent"]["sections"]
                .as_array()
                .unwrap()
                .len(),
            page.sections.len()
        );
        for section in page.sections {
            for (index, text) in section.parts.iter().enumerate() {
                assert!(text.chars().count() <= help_build::MAX_PART_CHARS);
                let result = call("get_help", json!({"id":section.id,"part":index+1})).unwrap();
                largest_part = largest_part.max(assert_final_budget("get_help", &result));
                let content = &result["structuredContent"];
                assert_eq!(content["text"], *text);
                assert_eq!(content["part"], index + 1);
                assert_eq!(content["parts"], section.parts.len());
            }
        }
    }
    let mut search_chars = 0;
    // Fill all 30 rows, including common Japanese/ASCII and long section titles.
    for query in ["の", "mIV", "設定", "音", "a"] {
        let result = call("search_help", json!({"query":query,"limit":30})).unwrap();
        search_chars = search_chars.max(assert_final_budget("search_help", &result));
        assert_eq!(
            result["structuredContent"]["results"]
                .as_array()
                .unwrap()
                .len(),
            30
        );
    }
    // Bound ANY query's 30 results: for each section take the most expensive
    // real 200-character window after both layers of JSON escaping, then take
    // the 30 most expensive rows. No invented (unreachable) snippet contents.
    let mut weights = std::collections::BTreeMap::new();
    let empty_chars = mcp_content_chars("").unwrap();
    let mut rows = Vec::new();
    for page in PAGES {
        for section in page.sections {
            let body = section.parts.join("\n\n").chars().collect::<Vec<_>>();
            let costs = body
                .iter()
                .map(|c| {
                    *weights
                        .entry(*c)
                        .or_insert_with(|| mcp_content_chars(&c.to_string()).unwrap() - empty_chars)
                })
                .collect::<Vec<_>>();
            let width = body.len().min(200);
            let mut cost = costs[..width].iter().sum::<usize>();
            let mut best = (cost, 0);
            for end in width..body.len() {
                cost = cost + costs[end] - costs[end - width];
                if cost > best.0 {
                    best = (cost, end + 1 - width);
                }
            }
            let snippet = body[best.1..best.1 + width].iter().collect::<String>();
            rows.push(json!({"id":section.id,"title":page.title,"heading":section.heading,"snippet":snippet}));
        }
    }
    rows.sort_by_key(|r| std::cmp::Reverse(mcp_content_chars(r).unwrap()));
    let worst = tool_result(
        json!({"app_version":env!("CARGO_PKG_VERSION"),"results":rows.into_iter().take(30).collect::<Vec<_>>()}),
    );
    let worst_chars = assert_final_budget("search_help", &worst);
    eprintln!(
        "Help final content budgets: pages={pages_chars}, section_list={largest_section_list}, part={largest_part}, search30={search_chars}, search30_upper_bound={worst_chars}"
    );
}

#[test]
fn help_tool_definitions_stay_short() {
    let mut total = 0;
    for tool in schema::tools()
        .into_iter()
        .filter(|t| is_tool(t["name"].as_str().unwrap()))
    {
        let chars = tool.to_string().chars().count();
        eprintln!("Help tool {}: {chars} serialized chars", tool["name"]);
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        total += chars;
    }
    assert!(
        total <= 1_360,
        "Leave room for the other five stage-1 tools: {total}"
    );
    assert!(crate::INSTRUCTIONS.contains("call search_help/get_help first"));
}

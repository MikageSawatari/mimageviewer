//! Shared by the build script and fixture tests; no runtime HTML dependency.
use scraper::{ElementRef, Html, Node, Selector};
use std::{collections::BTreeSet, fmt::Write, fs, path::Path};

pub const MAX_PART_CHARS: usize = 12_000;

#[derive(Debug, PartialEq, Eq)]
pub struct Page {
    pub id: String,
    pub title: String,
    pub sections: Vec<Section>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Section {
    pub id: String,
    pub heading: String,
    pub parts: Vec<String>,
}

pub fn load_manual(root: &Path) -> Result<Vec<Page>, String> {
    let mut paths = Vec::new();
    // Only these two levels are manual inputs, not arbitrary descendants.
    for dir in [root.to_path_buf(), root.join("sound")] {
        for entry in fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if entry.file_type().map_err(|e| e.to_string())?.is_file()
                && path.extension().is_some_and(|e| e == "html")
                && path.file_name().is_some_and(|n| n != "changelog.html")
            {
                paths.push(path);
            }
        }
    }
    paths.sort();
    if paths.is_empty() {
        return Err("manual has no HTML pages".into());
    }
    let mut pages = Vec::new();
    for path in paths {
        let id = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let html = fs::read_to_string(&path).map_err(|e| format!("{id}: {e}"))?;
        pages.push(extract_page(&id, &html)?);
    }
    validate_ids(&pages)?;
    Ok(pages)
}

pub fn validate_ids(pages: &[Page]) -> Result<(), String> {
    let mut ids = BTreeSet::new();
    for page in pages {
        for id in std::iter::once(&page.id).chain(page.sections.iter().map(|s| &s.id)) {
            if !ids.insert(id) {
                return Err(format!("duplicate help id: {id}"));
            }
        }
    }
    Ok(())
}

fn excluded(element: ElementRef<'_>) -> bool {
    matches!(element.value().name(), "script" | "style" | "nav")
        || element
            .value()
            .classes()
            .any(|c| matches!(c, "sidebar" | "breadcrumb" | "breadcrumbs" | "menu"))
}

// Inline conversion uses the parsed tree, including decoded HTML entities.
fn inline(element: ElementRef<'_>, out: &mut String) {
    if excluded(element) {
        return;
    }
    match element.value().name() {
        "img" => {
            out.push_str(element.value().attr("alt").unwrap_or(""));
            return;
        }
        "br" => out.push(' '),
        _ => {}
    }
    for child in element.children() {
        if let Node::Text(text) = child.value() {
            out.push_str(text);
        } else if let Some(child) = ElementRef::wrap(child) {
            inline(child, out);
        }
    }
}

fn normalized(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn label(element: ElementRef<'_>) -> String {
    let mut text = String::new();
    inline(element, &mut text);
    normalized(&text)
}

struct Extractor {
    page_id: String,
    sections: Vec<(String, String, Vec<String>)>,
    pending: String,
}

impl Extractor {
    fn flush(&mut self) {
        let text = normalized(&std::mem::take(&mut self.pending));
        if !text.is_empty() {
            self.sections.last_mut().unwrap().2.push(text);
        }
    }

    fn walk(&mut self, element: ElementRef<'_>) {
        if excluded(element) {
            return;
        }
        let name = element.value().name();
        if name == "h2" {
            self.flush();
            // Overview is #0; unanchored h2s use their 1-based page order.
            let anchor = element
                .value()
                .attr("id")
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| self.sections.len().to_string());
            self.sections.push((
                format!("{}#{anchor}", self.page_id),
                label(element),
                Vec::new(),
            ));
            return;
        }
        if name == "tr" {
            self.flush();
            let row = element
                .children()
                .filter_map(ElementRef::wrap)
                .filter(|c| matches!(c.value().name(), "td" | "th"))
                .map(label)
                .collect::<Vec<_>>()
                .join(" | ");
            if !row.is_empty() {
                self.sections.last_mut().unwrap().2.push(row);
            }
            return;
        }
        if name == "pre" {
            self.flush();
            let mut text = String::new();
            inline(element, &mut text);
            if !text.trim().is_empty() {
                self.sections
                    .last_mut()
                    .unwrap()
                    .2
                    .push(text.trim().replace("\r\n", "\n"));
            }
            return;
        }
        let block = matches!(
            name,
            "p" | "li"
                | "div"
                | "ul"
                | "ol"
                | "table"
                | "h1"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "blockquote"
                | "figure"
                | "figcaption"
                | "dt"
                | "dd"
        );
        if block {
            self.flush();
        }
        if name == "img" {
            self.pending
                .push_str(element.value().attr("alt").unwrap_or(""));
        } else if name == "br" {
            self.pending.push(' ');
        } else {
            for child in element.children() {
                if let Node::Text(text) = child.value() {
                    self.pending.push_str(text);
                } else if let Some(child) = ElementRef::wrap(child) {
                    self.walk(child);
                }
            }
        }
        if block {
            self.flush();
        }
    }
}

fn split_parts(id: &str, blocks: Vec<String>) -> Result<Vec<String>, String> {
    let mut parts = Vec::new();
    let mut part = String::new();
    let mut chars = 0;
    for block in blocks {
        let count = block.chars().count();
        if count > MAX_PART_CHARS {
            return Err(format!(
                "{id}: paragraph/table row exceeds {MAX_PART_CHARS} chars"
            ));
        }
        if chars > 0 && chars + 2 + count > MAX_PART_CHARS {
            parts.push(std::mem::take(&mut part));
            chars = 0;
        }
        if chars > 0 {
            part.push_str("\n\n");
            chars += 2;
        }
        part.push_str(&block);
        chars += count;
    }
    if !part.is_empty() || parts.is_empty() {
        parts.push(part);
    }
    Ok(parts)
}

pub fn extract_page(id: &str, source: &str) -> Result<Page, String> {
    let html = Html::parse_document(source);
    let main_selector = Selector::parse("main.content").unwrap();
    let main = html
        .select(&main_selector)
        .next()
        .ok_or_else(|| format!("{id}: missing <main class=\"content\">"))?;
    let title = main
        .select(&Selector::parse("h1").unwrap())
        .next()
        .map(label)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| format!("{id}: missing page title"))?;
    let mut extractor = Extractor {
        page_id: id.into(),
        sections: vec![(format!("{id}#0"), "概要".into(), Vec::new())],
        pending: String::new(),
    };
    extractor.walk(main);
    extractor.flush();
    let sections = extractor
        .sections
        .into_iter()
        .map(|(id, heading, blocks)| {
            Ok(Section {
                parts: split_parts(&id, blocks)?,
                id,
                heading,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut page = Page {
        id: id.into(),
        title,
        sections,
    };
    validate_ids(std::slice::from_ref(&page))?;
    // Validate before omitting the empty overview so #0 remains reserved even
    // when omitted. h2 ordinals were assigned with the overview still present.
    if page.sections[0].parts.iter().all(String::is_empty) {
        page.sections.remove(0);
    }
    Ok(page)
}

pub fn generate(pages: &[Page]) -> String {
    let mut out = String::from(
        "// Generated from the bundled manual. Do not edit.\nconst PAGES: &[HelpPage] = &[\n",
    );
    for page in pages {
        writeln!(
            out,
            "HelpPage {{ id: {:?}, title: {:?}, sections: &[",
            page.id, page.title
        )
        .unwrap();
        for section in &page.sections {
            writeln!(
                out,
                "HelpSection {{ id: {:?}, heading: {:?}, parts: &{:?} }},",
                section.id, section.heading, section.parts
            )
            .unwrap();
        }
        out.push_str("] },\n");
    }
    out.push_str("];\n");
    out
}

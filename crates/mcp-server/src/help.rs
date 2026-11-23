//! Public, version-matched help. This module never calls the core backend.
use mimageviewer_agent_ipc::{ErrorCode, MAX_PAGE_CHARS, mcp_content_chars};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) const MAX_HELP_QUERY_CHARS: usize = 200;
pub(super) const MAX_HELP_QUERY_TERMS: usize = 8;

struct HelpPage {
    id: &'static str,
    title: &'static str,
    sections: &'static [HelpSection],
}

struct HelpSection {
    id: &'static str,
    heading: &'static str,
    parts: &'static [&'static str],
}

include!(concat!(env!("OUT_DIR"), "/bundled_help.rs"));

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    query: String,
    limit: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetArgs {
    id: Option<String>,
    part: Option<u32>,
}

#[cfg(test)]
fn is_tool(name: &str) -> bool {
    matches!(name, "search_help" | "get_help")
}

// Argument errors contain no submitted query text. Neither the request nor its
// query is logged or retained beyond this call.
pub fn call(name: &str, args: Value) -> Result<Value, &'static str> {
    let value = match name {
        "search_help" => {
            let args: SearchArgs =
                serde_json::from_value(args).map_err(|_| "Invalid tool arguments")?;
            if args.query.chars().take(MAX_HELP_QUERY_CHARS + 1).count() > MAX_HELP_QUERY_CHARS {
                return Err("Help query must be at most 200 characters");
            }
            let terms = args
                .query
                .split_whitespace()
                .take(MAX_HELP_QUERY_TERMS + 1)
                .map(str::to_ascii_lowercase)
                .collect::<Vec<_>>();
            if terms.is_empty() {
                return Err("Specify a nonempty help query");
            }
            if terms.len() > MAX_HELP_QUERY_TERMS {
                return Err("Specify at most 8 help query terms");
            }
            let limit = args.limit.unwrap_or(10);
            if limit == 0 {
                return Err("Specify a positive limit");
            }
            search(PAGES, &terms, limit.min(30) as usize)
        }
        "get_help" => {
            let args: GetArgs =
                serde_json::from_value(args).map_err(|_| "Invalid tool arguments")?;
            if args.part == Some(0) {
                return Err("Help parts start at 1");
            }
            get(PAGES, args.id.as_deref(), args.part.unwrap_or(1))?
        }
        _ => return Err("Unknown help tool"),
    };
    Ok(tool_result(value))
}

fn tool_result(mut value: Value) -> Value {
    let mut is_error = value.get("error").is_some();
    if !mcp_content_chars(&value).is_ok_and(|chars| chars <= MAX_PAGE_CHARS) {
        value = failure(
            ErrorCode::Internal,
            "Bundled help exceeds the response budget",
        );
        is_error = true;
    }
    json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":is_error})
}

fn failure(code: ErrorCode, message: &str) -> Value {
    json!({"app_version":env!("CARGO_PKG_VERSION"),"error":{"code":code,"message":message}})
}

fn get(pages: &[HelpPage], id: Option<&str>, part: u32) -> Result<Value, &'static str> {
    let version = env!("CARGO_PKG_VERSION");
    let Some(id) = id else {
        if part != 1 {
            return Err("Specify a section id for part retrieval");
        }
        return Ok(
            json!({"app_version":version,"pages":pages.iter().map(|p| json!({"id":p.id,"title":p.title})).collect::<Vec<_>>()}),
        );
    };
    for page in pages {
        if page.id == id {
            if part != 1 {
                return Err("Specify a section id for part retrieval");
            }
            return Ok(
                json!({"app_version":version,"id":page.id,"title":page.title,"sections":page.sections.iter().map(|s| json!({"id":s.id,"heading":s.heading,"parts":s.parts.len()})).collect::<Vec<_>>()}),
            );
        }
        if let Some(section) = page.sections.iter().find(|s| s.id == id) {
            let Some(text) = section.parts.get((part - 1) as usize) else {
                return Ok(failure(ErrorCode::BadRequest, "Help part does not exist"));
            };
            return Ok(
                json!({"app_version":version,"id":id,"title":page.title,"heading":section.heading,"part":part,"parts":section.parts.len(),"text":text}),
            );
        }
    }
    Ok(failure(ErrorCode::BadRequest, "Help id does not exist"))
}

fn snippet(body: &str, terms: &[String]) -> String {
    let lower = body.to_ascii_lowercase();
    let hit = terms
        .iter()
        .filter_map(|term| lower.find(term.as_str()))
        .min()
        .unwrap_or(0);
    // ASCII folding preserves byte offsets, including for Japanese text.
    let before = body[..hit].chars().count().saturating_sub(60);
    body.chars().skip(before).take(200).collect()
}

fn search(pages: &[HelpPage], terms: &[String], limit: usize) -> Value {
    let mut hits = Vec::new();
    for page in pages {
        let title = page.title.to_ascii_lowercase();
        for section in page.sections {
            let heading = section.heading.to_ascii_lowercase();
            let body = section.parts.join("\n\n");
            let lower = body.to_ascii_lowercase();
            if !terms.iter().all(|t| {
                heading.contains(t.as_str())
                    || title.contains(t.as_str())
                    || lower.contains(t.as_str())
            }) {
                continue;
            }
            let score = (
                terms
                    .iter()
                    .filter(|t| heading.contains(t.as_str()))
                    .count(),
                terms.iter().filter(|t| title.contains(t.as_str())).count(),
                terms
                    .iter()
                    .map(|t| lower.matches(t.as_str()).count())
                    .sum::<usize>(),
            );
            hits.push((score, section.id, json!({"id":section.id,"title":page.title,"heading":section.heading,"snippet":snippet(&body, terms)})));
        }
    }
    // Stable tie-break independent of file enumeration and hash iteration order.
    hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    json!({"app_version":env!("CARGO_PKG_VERSION"),"results":hits.into_iter().take(limit).map(|h| h.2).collect::<Vec<_>>()})
}

#[cfg(test)]
#[path = "help_tests.rs"]
mod tests;

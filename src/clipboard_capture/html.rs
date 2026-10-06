//! Bounded CF_HTML parsing. Called by the capture worker, never the UI thread.

use super::data::{MAX_HTML_BYTES, html_source_url};
use cssparser::{Delimiter, Parser, ParserInput, Token};
use scraper::{ElementRef, Html, Selector};
use std::collections::HashSet;
use url::Url;

const MAX_CANDIDATES: usize = 500;

#[derive(Clone, Debug)]
pub(crate) struct HtmlCapture {
    pub page_url: String,
    pub candidates: Vec<String>,
    pub omitted: usize,
}

pub(crate) fn parse_cf_html(bytes: &[u8]) -> Result<HtmlCapture, String> {
    if bytes.len() > MAX_HTML_BYTES {
        return Err("クリップボードの HTML が大きすぎます".into());
    }
    let (header, header_end) = header(bytes);
    let start = offset(header, b"StartHTML:").ok_or("HTML の範囲が不正です")?;
    let end = offset(header, b"EndHTML:").ok_or("HTML の範囲が不正です")?;
    if start < header_end || start > end {
        return Err("HTML の範囲が不正です".into());
    }
    let context = bytes
        .get(start..end)
        .and_then(|value| std::str::from_utf8(value).ok())
        .ok_or("HTML の範囲または UTF-8 が不正です")?;
    let page_url = html_source_url(header).ok_or("ページの URL がありません")?;
    // Provenance removes credentials, but candidate resolution must retain
    // inherited userinfo so the fetch boundary can refuse it (§8.2).
    let raw_source = header
        .split(|byte| *byte == b'\n')
        .find_map(|line| line.strip_prefix(b"SourceURL:"))
        .and_then(|value| std::str::from_utf8(value.strip_suffix(b"\r").unwrap_or(value)).ok())
        .ok_or("ページの URL が不正です")?;
    let page = Url::parse(raw_source).map_err(|_| "ページの URL が不正です")?;
    // A bad fragment never grants permission to parse outside validated context.
    let fragment = offset(header, b"StartFragment:")
        .zip(offset(header, b"EndFragment:"))
        .filter(|(a, b)| start <= *a && a <= b && *b <= end)
        .and_then(|(a, b)| context.get(a - start..b - start))
        .or_else(|| marked_fragment(context))
        .unwrap_or(context);

    // Resolve base from the whole context, but gather images only from fragment.
    // Drop the context DOM before allocating a second one for the fragment.
    let base = {
        let document = Html::parse_document(context);
        let selector = Selector::parse("base[href]").unwrap();
        document
            .select(&selector)
            .next()
            .and_then(|element| element.value().attr("href"))
            .and_then(|value| resolve_http(value, &page))
            .unwrap_or_else(|| page.clone())
    };
    let document = Html::parse_fragment(fragment);
    let selector = Selector::parse("*").unwrap();
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    let mut omitted = 0;
    let mut add = |value: &str, image_link: bool| {
        let Some(url) = resolve_candidate(value, &base, image_link) else {
            return;
        };
        if seen.insert(url.clone()) {
            if candidates.len() < MAX_CANDIDATES {
                candidates.push(url);
            } else {
                omitted += 1;
            }
        }
    };
    for element in document.select(&selector) {
        if declared_small(element) {
            continue;
        }
        let value = element.value();
        if value.name() == "img" {
            if let Some(src) = value.attr("src") {
                add(src, false);
            }
        }
        if value.name() == "img"
            || (value.name() == "source"
                && element
                    .parent()
                    .and_then(ElementRef::wrap)
                    .is_some_and(|parent| parent.value().name() == "picture"))
        {
            if let Some(srcset) = value.attr("srcset") {
                if let Some(src) = largest_srcset(srcset) {
                    add(src, false);
                }
            }
        }
        if value.name() == "a" {
            if let Some(href) = value.attr("href") {
                add(href, true);
            }
        }
        if let Some(src) = value.attr("data-src") {
            add(src, false);
        }
        if let Some(style) = value.attr("style") {
            // html5ever has already decoded &quot;, &amp;, etc. in attributes.
            for src in background_urls(style) {
                add(&src, false);
            }
        }
    }
    Ok(HtmlCapture {
        page_url,
        candidates,
        omitted,
    })
}

/// Header fields are line-oriented. Stop at the first non-header line, so HTML
/// text cannot impersonate offsets or SourceURL. Preserve the byte boundary.
fn header(bytes: &[u8]) -> (&[u8], usize) {
    let mut end = 0;
    let mut context_start = None;
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if context_start.is_some_and(|start| end >= start) {
            break;
        }
        let Some(colon) = line.iter().position(|byte| *byte == b':') else {
            break;
        };
        if colon == 0 || !line[..colon].iter().all(u8::is_ascii_alphanumeric) {
            break;
        }
        if let Some(value) = line.strip_prefix(b"StartHTML:") {
            context_start = std::str::from_utf8(value)
                .ok()
                .and_then(|value| value.trim().parse::<usize>().ok());
        }
        end += line.len();
    }
    (&bytes[..end], end)
}

fn offset(header: &[u8], field: &[u8]) -> Option<usize> {
    let mut values = header
        .split(|byte| *byte == b'\n')
        .filter_map(|line| line.strip_prefix(field));
    let value = std::str::from_utf8(values.next()?).ok()?.trim();
    if values.next().is_some() || value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

fn marked_fragment(context: &str) -> Option<&str> {
    const START: &str = "<!--StartFragment-->";
    const END: &str = "<!--EndFragment-->";
    let start = context.find(START)? + START.len();
    let end = context[start..].find(END)? + start;
    context.get(start..end)
}

fn declared_small(element: ElementRef<'_>) -> bool {
    let value = element.value();
    value
        .attr("width")
        .and_then(|width| width.trim().parse::<u32>().ok())
        .zip(
            value
                .attr("height")
                .and_then(|height| height.trim().parse::<u32>().ok()),
        )
        .is_some_and(|(width, height)| width < 32 || height < 32)
}

fn resolve_http(value: &str, base: &Url) -> Option<Url> {
    let value = value.trim();
    if value.chars().any(char::is_control) {
        return None;
    }
    let url = base.join(value).ok()?;
    (matches!(url.scheme(), "http" | "https") && url.host_str().is_some()).then_some(url)
}

fn resolve_candidate(value: &str, base: &Url, image_link: bool) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_control) {
        return None;
    }
    let mut url = base.join(value).ok()?;
    match url.scheme() {
        "http" | "https" if url.host_str().is_some() => {
            let extension = url
                .path()
                .rsplit('/')
                .next()?
                .rsplit_once('.')
                .map(|(_, ext)| ext);
            if extension.is_some_and(|ext| ext.eq_ignore_ascii_case("svg")) {
                return None;
            }
            if image_link
                && !extension.is_some_and(|ext| {
                    matches!(
                        ext.to_ascii_lowercase().as_str(),
                        "jpg"
                            | "jpeg"
                            | "png"
                            | "gif"
                            | "webp"
                            | "bmp"
                            | "tif"
                            | "tiff"
                            | "avif"
                            | "heic"
                            | "heif"
                            | "jxl"
                    )
                })
            {
                return None;
            }
        }
        "data" if !image_link => {
            let media = url.path().split_once(',')?.0.split(';').next()?.trim();
            let subtype = media
                .get(..6)
                .filter(|prefix| prefix.eq_ignore_ascii_case("image/"))
                .and_then(|_| media.get(6..))?;
            if subtype.is_empty() || subtype.eq_ignore_ascii_case("svg+xml") {
                return None;
            }
        }
        _ => return None,
    }
    // Keep userinfo here: fetch rejects it, including credentials inherited
    // from <base>. Sanitizing candidates would hide that security violation.
    url.set_fragment(None);
    Some(url.to_string())
}

/// Follow srcset's URL token boundary: commas inside a data URL are part of
/// the URL, while a descriptor's comma ends the candidate.
fn largest_srcset(value: &str) -> Option<&str> {
    let bytes = value.as_bytes();
    let mut cursor = 0;
    let mut best: Option<(&str, f64)> = None;
    while cursor < bytes.len() {
        while cursor < bytes.len() && (bytes[cursor].is_ascii_whitespace() || bytes[cursor] == b',')
        {
            cursor += 1;
        }
        let start = cursor;
        while cursor < bytes.len() && !bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let mut end = cursor;
        let mut descriptors = "";
        if end > start && bytes[end - 1] == b',' {
            while end > start && bytes[end - 1] == b',' {
                end -= 1;
            }
        } else {
            let desc_start = cursor;
            let mut parentheses = 0usize;
            while cursor < bytes.len() {
                match bytes[cursor] {
                    b'(' => parentheses += 1,
                    b')' => parentheses = parentheses.saturating_sub(1),
                    b',' if parentheses == 0 => break,
                    _ => {}
                }
                cursor += 1;
            }
            descriptors = &value[desc_start..cursor];
            if cursor < bytes.len() {
                cursor += 1;
            }
        }
        if start == end {
            continue;
        }
        let mut tokens = descriptors.split_ascii_whitespace();
        let score = match tokens.next() {
            None => Some(1.0),
            Some(token) if tokens.next().is_none() => {
                if let Some(width) = token.strip_suffix('w') {
                    width
                        .parse::<u32>()
                        .ok()
                        .filter(|width| *width > 0)
                        .map(f64::from)
                } else {
                    token
                        .strip_suffix('x')
                        .and_then(|density| density.parse::<f64>().ok())
                        .filter(|density| density.is_finite() && *density > 0.0)
                }
            }
            _ => None,
        };
        if let Some(score) = score {
            if best.is_none_or(|(_, prior)| score > prior) {
                best = Some((&value[start..end], score));
            }
        }
    }
    best.map(|(url, _)| url)
}

fn background_urls(style: &str) -> Vec<String> {
    let mut input = ParserInput::new(style);
    let mut parser = Parser::new(&mut input);
    let mut urls = Vec::new();
    while !parser.is_exhausted() {
        let _ = parser.parse_until_before(Delimiter::Semicolon, |declaration| {
            let background = declaration
                .expect_ident()?
                .eq_ignore_ascii_case("background-image");
            declaration.expect_colon()?;
            if background {
                css_urls(declaration, &mut urls, 0);
            } else {
                while declaration.next().is_ok() {}
            }
            Ok::<_, cssparser::ParseError<'_, ()>>(())
        });
        let _ = parser.expect_semicolon();
    }
    urls
}

fn css_urls(parser: &mut Parser<'_, '_>, urls: &mut Vec<String>, depth: usize) {
    while let Ok(token) = parser.next().cloned() {
        match token {
            Token::UnquotedUrl(url) => urls.push(url.to_string()),
            Token::Function(name) if name.eq_ignore_ascii_case("url") => {
                let result = parser.parse_nested_block(|nested| {
                    let url = nested.expect_string_cloned()?;
                    nested.expect_exhausted()?;
                    Ok::<_, cssparser::ParseError<'_, ()>>(url.to_string())
                });
                if let Ok(url) = result {
                    urls.push(url);
                }
            }
            Token::Function(_) if depth < 32 => {
                let _ = parser.parse_nested_block(|nested| {
                    css_urls(nested, urls, depth + 1);
                    Ok::<_, cssparser::ParseError<'_, ()>>(())
                });
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cf_html(before: &str, fragment: &str, after: &str, source: Option<&str>) -> Vec<u8> {
        let source = source
            .map(|value| format!("SourceURL:{value}\r\n"))
            .unwrap_or_default();
        let make_header = |start: usize, end: usize, a: usize, b: usize| {
            format!(
                "Version:1.0\r\nStartHTML:{start:010}\r\nEndHTML:{end:010}\r\nStartFragment:{a:010}\r\nEndFragment:{b:010}\r\n{source}"
            )
        };
        let start = make_header(0, 0, 0, 0).len();
        let a = start + before.len();
        let b = a + fragment.len();
        let end = b + after.len();
        format!("{}{before}{fragment}{after}", make_header(start, end, a, b)).into_bytes()
    }

    fn capture(fragment: &str) -> HtmlCapture {
        parse_cf_html(&cf_html(
            "<html><body>",
            fragment,
            "</body></html>",
            Some("https://example.com/pages/index.html#copy"),
        ))
        .unwrap()
    }

    fn replace_offset(bytes: &mut [u8], field: &str, value: usize) {
        let start = std::str::from_utf8(bytes).unwrap().find(field).unwrap() + field.len();
        bytes[start..start + 10].copy_from_slice(format!("{value:010}").as_bytes());
    }

    #[test]
    fn context_base_and_fragment_are_separate() {
        let html = cf_html(
            "<html><head><base href='https://cdn.example.com/images/'></head><body><img src='outside.png'>",
            "<img src='inside.png#first'><img src='inside.png#second'>",
            "<img src='after.png'></body></html>",
            Some("https://user:secret@example.com/page#fragment"),
        );
        let result = parse_cf_html(&html).unwrap();
        assert_eq!(result.page_url, "https://example.com/page");
        assert_eq!(
            result.candidates,
            ["https://cdn.example.com/images/inside.png"]
        );
    }

    #[test]
    fn source_credentials_remain_on_relative_candidates_for_fetch_refusal() {
        let html = cf_html(
            "<html><body>",
            "<img src='image.png'><img src='https://cdn.example.com/image.png'>",
            "</body></html>",
            Some("https://user:secret@example.com/dir/page#copy"),
        );
        let result = parse_cf_html(&html).unwrap();
        assert_eq!(result.page_url, "https://example.com/dir/page");
        assert_eq!(
            result.candidates,
            [
                "https://user:secret@example.com/dir/image.png",
                "https://cdn.example.com/image.png",
            ]
        );
    }

    #[test]
    fn invalid_fragment_uses_markers_then_valid_context() {
        let mut html = cf_html(
            "<html><img src='/outside.png'><!--StartFragment-->",
            "<img src='/inside.png'>",
            "<!--EndFragment--></html>",
            Some("https://example.com/"),
        );
        replace_offset(&mut html, "StartFragment:", usize::MAX.min(9999999999));
        assert_eq!(
            parse_cf_html(&html).unwrap().candidates,
            ["https://example.com/inside.png"]
        );
        let mut html = cf_html(
            "<html>",
            "<img src='/context.png'>",
            "</html>",
            Some("https://example.com/"),
        );
        replace_offset(&mut html, "EndFragment:", 0);
        assert_eq!(
            parse_cf_html(&html).unwrap().candidates,
            ["https://example.com/context.png"]
        );
    }

    #[test]
    fn invalid_context_and_missing_source_are_rejected() {
        let mut html = cf_html(
            "<html>",
            "<img src='/a.png'>",
            "</html>",
            Some("https://example.com/"),
        );
        let invalid_end = html.len() + 1;
        replace_offset(&mut html, "EndHTML:", invalid_end);
        assert!(parse_cf_html(&html).is_err());
        replace_offset(&mut html, "StartHTML:", 0);
        assert!(parse_cf_html(&html).is_err());
        assert!(parse_cf_html(&cf_html("<html>", "<img src='/a.png'>", "</html>", None)).is_err());
    }

    #[test]
    fn utf8_boundaries_and_invalid_encoding() {
        let mut html = cf_html(
            "<html><!--StartFragment-->",
            "日本語<img src='/a.png'>",
            "<!--EndFragment--></html>",
            Some("https://example.com/"),
        );
        let boundary = std::str::from_utf8(&html).unwrap().find("日本語").unwrap();
        replace_offset(&mut html, "StartFragment:", boundary + 1);
        assert_eq!(
            parse_cf_html(&html).unwrap().candidates,
            ["https://example.com/a.png"]
        );
        replace_offset(&mut html, "StartHTML:", boundary + 1);
        assert!(parse_cf_html(&html).is_err());
        let mut html = cf_html(
            "<html>",
            "<img src='/a.png'>",
            "</html>",
            Some("https://example.com/"),
        );
        let end = html.len();
        html[end - 2] = 0xff;
        assert!(parse_cf_html(&html).is_err());
    }

    #[test]
    fn srcset_maximum_and_picture_sources() {
        let result = capture(
            "<img src='/plain.png' srcset='/small.png 320w, /large.png 1920w, /middle.png 800w'><picture><source srcset='/one.png 1x, /three.png 3x, /two.png 2x'><img src='/fallback.png'></picture><video><source srcset='/video.png 9x'></video>",
        );
        assert_eq!(
            result.candidates,
            [
                "https://example.com/plain.png",
                "https://example.com/large.png",
                "https://example.com/three.png",
                "https://example.com/fallback.png"
            ]
        );
        assert_eq!(
            largest_srcset("a.png, b.png 2x, c.png 0x, d.png bad"),
            Some("b.png")
        );
        assert_eq!(largest_srcset("a.png 0w, b.png 1x 2x, c.png NaNx"), None);
    }

    #[test]
    fn srcset_data_commas_are_part_of_url() {
        assert_eq!(
            largest_srcset("data:image/png;base64,AAAA 3x, https://example.com/b.png 2x"),
            Some("data:image/png;base64,AAAA")
        );
        assert_eq!(
            largest_srcset("data:image/png;base64,AAAA, /b.png 2x"),
            Some("/b.png")
        );
        assert_eq!(
            capture("<img srcset='data:image/png;base64,AAAA 3x, /b.png 2x'>").candidates,
            ["data:image/png;base64,AAAA"]
        );
    }

    #[test]
    fn schemes_svg_anchor_extensions_and_lazy_sources() {
        let result = capture(
            "<img src='http://example.com/a.png#x'><img src='data:image/png;base64,AAAA'><img src='data:text/html,abc'><img src='data:image/svg+xml,%3Csvg%3E'><img src='/a.SVG?x=1'><img src='blob:https://example.com/a'><img src='file:///a.png'><img src='cid:a'><img src='javascript:alert(1)'><a href='/original.JPEG?key=1#x'>full</a><a href='/page.html'>page</a><a href='/icon.ico'>icon</a><a href='/image.ppm'>image</a><div data-src='/lazy.webp'></div>",
        );
        assert_eq!(
            result.candidates,
            [
                "http://example.com/a.png",
                "data:image/png;base64,AAAA",
                "https://example.com/original.JPEG?key=1",
                "https://example.com/lazy.webp"
            ]
        );
    }

    #[test]
    fn both_declared_dimensions_are_required_for_small_filter() {
        let result = capture(
            "<img src='/small.png' width='31' height='100'><img src='/short.png' width='100' height='1'><img src='/one.png' width='1'><img src='/unknown.png' width='1' height='auto'><img src='/minimum.png' width='32' height='32'>",
        );
        assert_eq!(
            result.candidates,
            [
                "https://example.com/one.png",
                "https://example.com/unknown.png",
                "https://example.com/minimum.png"
            ]
        );
    }

    #[test]
    fn inline_background_decodes_entities_and_css_syntax() {
        let result = capture(
            r#"<div style='content: "ignored; value"; background-image: url(&quot;/a.png?x=1&amp;y=2&quot;), URL("/b;comma,.png"); color:red'></div><div style="background-image: /* comment */ url(/c.png)"></div><div style="background:url(/not-requested.png)"></div><div style="background-image:url('/escaped\2e png')"></div>"#,
        );
        assert_eq!(
            result.candidates,
            [
                "https://example.com/a.png?x=1&y=2",
                "https://example.com/b;comma,.png",
                "https://example.com/c.png",
                "https://example.com/escaped.png"
            ]
        );
    }

    #[test]
    fn candidate_limit_counts_unique_omissions() {
        let mut fragment = String::new();
        for index in 0..503 {
            fragment.push_str(&format!(
                "<img src='/image{index}.png#one'><img src='/image{index}.png#two'>"
            ));
        }
        let result = capture(&fragment);
        assert_eq!(result.candidates.len(), 500);
        assert_eq!(result.omitted, 3);
        assert_eq!(result.candidates[499], "https://example.com/image499.png");
    }

    #[test]
    fn candidate_credentials_are_preserved_for_fetch_rejection() {
        let result = parse_cf_html(&cf_html(
            "<html><base href='https://user:secret@cdn.example.com/'>",
            "<img src='a.png'>",
            "</html>",
            Some("https://example.com/"),
        ))
        .unwrap();
        assert_eq!(
            result.candidates,
            ["https://user:secret@cdn.example.com/a.png"]
        );
    }

    #[test]
    fn relative_base_and_invalid_base_fallback() {
        let result = parse_cf_html(&cf_html(
            "<html><base href='../assets/?x=1&amp;y=2'>",
            "<img src='a.png'>",
            "</html>",
            Some("https://example.com/pages/index.html"),
        ))
        .unwrap();
        assert_eq!(result.candidates, ["https://example.com/assets/a.png"]);
        let result = parse_cf_html(&cf_html(
            "<html><base href='file:///private/'>",
            "<img src='a.png'>",
            "</html>",
            Some("https://example.com/pages/index.html"),
        ))
        .unwrap();
        assert_eq!(result.candidates, ["https://example.com/pages/a.png"]);
    }

    #[test]
    fn html_parser_tolerates_markup_and_decodes_url_entities() {
        let result = capture(
            "<PICTURE><SOURCE SRCSET='/a.png 1x,/b.png?x=1&amp;y=2 2x'><IMG SRC=/fallback.png></PICTURE><p><b><img src='/unclosed.png'>",
        );
        assert_eq!(
            result.candidates,
            [
                "https://example.com/b.png?x=1&y=2",
                "https://example.com/fallback.png",
                "https://example.com/unclosed.png"
            ]
        );
    }

    #[test]
    fn fragment_markers_outside_context_are_not_a_fallback() {
        let mut html = cf_html(
            "<html>",
            "<img src='/context.png'>",
            "</html>",
            Some("https://example.com/"),
        );
        replace_offset(&mut html, "EndFragment:", 0);
        html.extend_from_slice(b"<!--StartFragment--><img src='/outside.png'><!--EndFragment-->");
        assert_eq!(
            parse_cf_html(&html).unwrap().candidates,
            ["https://example.com/context.png"]
        );
    }

    #[test]
    fn byte_budget_and_header_spoofing() {
        assert!(parse_cf_html(&vec![b'a'; MAX_HTML_BYTES + 1]).is_err());
        let html = cf_html(
            "<html>",
            "\nSourceURL:https://example.com/\n<img src='/a.png'>",
            "</html>",
            None,
        );
        assert!(parse_cf_html(&html).is_err());
    }
}

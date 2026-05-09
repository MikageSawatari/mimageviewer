//! Text rendering helpers for user metadata that may contain HTTP(S) URLs.

use eframe::egui;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextUrlSpan {
    pub start: usize,
    pub end: usize,
    pub url: String,
}

pub fn find_http_urls(text: &str) -> Vec<TextUrlSpan> {
    let mut spans = Vec::new();
    let mut search_from = 0;

    while let Some(start) = find_next_scheme(text, search_from) {
        let raw_end = text[start..]
            .char_indices()
            .find_map(|(offset, ch)| {
                if offset > 0 && (ch.is_whitespace() || ch.is_control()) {
                    Some(start + offset)
                } else {
                    None
                }
            })
            .unwrap_or(text.len());
        let end = trim_url_trailing_punctuation(text, start, raw_end);
        if end > start
            && let Some(url) = crate::external_links::normalize_http_url(&text[start..end])
        {
            spans.push(TextUrlSpan { start, end, url });
        }
        search_from = raw_end.max(start + 1);
    }

    spans
}

pub fn draw_text_with_links(
    ui: &mut egui::Ui,
    text: &str,
    font: egui::FontId,
    text_color: egui::Color32,
    link_color: egui::Color32,
) -> Option<String> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut clicked_url = None;

    for (line_idx, line) in normalized.split('\n').enumerate() {
        if line_idx > 0 && line.is_empty() {
            ui.add_space((font.size * 0.65).max(4.0));
            continue;
        }
        if line.is_empty() {
            ui.add_space((font.size * 0.35).max(2.0));
            continue;
        }
        if let Some(url) = draw_line_with_links(ui, line, font.clone(), text_color, link_color)
            && clicked_url.is_none()
        {
            clicked_url = Some(url);
        }
    }

    clicked_url
}

fn draw_line_with_links(
    ui: &mut egui::Ui,
    line: &str,
    font: egui::FontId,
    text_color: egui::Color32,
    link_color: egui::Color32,
) -> Option<String> {
    let spans = find_http_urls(line);
    if spans.is_empty() {
        ui.add(egui::Label::new(egui::RichText::new(line).font(font).color(text_color)).wrap());
        return None;
    }

    let mut clicked_url = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let mut pos = 0;
        for span in spans {
            if span.start > pos {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&line[pos..span.start])
                            .font(font.clone())
                            .color(text_color),
                    )
                    .wrap(),
                );
            }

            let response = ui.add(
                egui::Label::new(
                    egui::RichText::new(&line[span.start..span.end])
                        .font(font.clone())
                        .color(link_color),
                )
                .wrap()
                .sense(egui::Sense::click()),
            );
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(span.url.as_str())
                .clicked()
            {
                clicked_url = Some(span.url.clone());
            }
            pos = span.end;
        }
        if pos < line.len() {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(&line[pos..])
                        .font(font)
                        .color(text_color),
                )
                .wrap(),
            );
        }
    });

    clicked_url
}

fn find_next_scheme(text: &str, from: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut idx = from;
    while idx < bytes.len() {
        if starts_ascii_ignore_case(bytes, idx, b"http://")
            || starts_ascii_ignore_case(bytes, idx, b"https://")
        {
            return Some(idx);
        }
        idx += 1;
    }
    None
}

fn starts_ascii_ignore_case(bytes: &[u8], idx: usize, needle: &[u8]) -> bool {
    bytes
        .get(idx..idx.saturating_add(needle.len()))
        .is_some_and(|slice| slice.eq_ignore_ascii_case(needle))
}

fn trim_url_trailing_punctuation(text: &str, start: usize, mut end: usize) -> usize {
    while end > start {
        let Some(ch) = text[..end].chars().next_back() else {
            break;
        };
        if is_url_trailing_punctuation(ch) {
            end -= ch.len_utf8();
        } else {
            break;
        }
    }
    end
}

fn is_url_trailing_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '.' | ','
            | ';'
            | ':'
            | '!'
            | '?'
            | ')'
            | ']'
            | '}'
            | '"'
            | '\''
            | '、'
            | '。'
            | '，'
            | '．'
            | '）'
            | '」'
            | '』'
            | '】'
            | '〕'
            | '〉'
            | '》'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_and_trims_http_urls() {
        let spans = find_http_urls("see https://example.com/a?x=1). and HTTP://x.test/y");
        assert_eq!(
            spans.iter().map(|s| s.url.as_str()).collect::<Vec<_>>(),
            vec!["https://example.com/a?x=1", "HTTP://x.test/y"]
        );
    }

    #[test]
    fn ignores_unsafe_schemes() {
        assert!(find_http_urls("javascript:alert(1) file:///tmp/a").is_empty());
    }
}

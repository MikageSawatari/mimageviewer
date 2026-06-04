//! Pure text layout engine.
//!
//! Computes glyph placements (positions + per-glyph size + sideways flag) for
//! horizontal (横書き) and vertical (縦書き) text, plus a minimal automatic
//! 縦中横 (tate-chu-yoko) for short half-width runs in vertical mode.
//!
//! All positions are in a local layout space whose origin is the layout's
//! top-left; `TextLayout::bounds` gives the overall extent. The lab and the
//! rasterizer translate this into image-pixel space.
//!
//! This is intentionally a hand-written column layout — standard text layout
//! libraries do not do 縦書き. cosmic-text is the eventual target for shaping
//! and font fallback (docs §5.2); see `font.rs`. For Phase 1 we stack glyphs
//! by their natural advance.

use crate::font::LoadedFont;
use crate::model::{InlineDir, MarkupRule, Orientation, TextAlign, TextBlock};

/// One placed glyph in layout space.
///
/// Coordinate semantics depend on `form`:
/// - `Upright`: `x` is the pen-left and `y` is the baseline.
/// - `Sideways`: `x` / `y` are the **center** of the rotated glyph cell. The
///   rasterizer rotates the coverage 90° CW and blits it centered.
///
/// `glyph_id` is the **shaped** glyph to rasterize. For vertical text this is the
/// result of rustybuzz top-to-bottom shaping (the font's `vert` feature, or the
/// UAX#50 fallback, has already substituted the correct vertical form), so there
/// is no per-character substitution table in this module. The rasterizer draws
/// `glyph_id` directly via `LoadedFont::rasterize_gid`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphPlacement {
    /// Logical source character. Tests and diagnostics use this value.
    pub ch: char,
    /// Shaped/substituted glyph id to rasterize (see struct docs).
    pub glyph_id: u16,
    /// Upright: pen-left. Sideways: center x. (see struct docs)
    pub x: f32,
    /// Upright: baseline y. Sideways: center y. (see struct docs)
    pub y: f32,
    /// Effective pixel size for this glyph (kept == block size in Phase 1).
    pub size: f32,
    /// Back-compat convenience for tests/UI: true only for explicit 横倒し runs.
    pub sideways: bool,
    pub form: GlyphForm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlyphForm {
    Upright,
    /// Explicit 横倒し run from marker markup (rotated 90° CW at draw time).
    Sideways,
}

impl GlyphPlacement {
    fn new(ch: char, glyph_id: u16, x: f32, y: f32, size: f32, form: GlyphForm) -> Self {
        GlyphPlacement {
            ch,
            glyph_id,
            x,
            y,
            size,
            sideways: form == GlyphForm::Sideways,
            form,
        }
    }
}

/// Result of laying out a TextBlock.
#[derive(Debug, Clone, PartialEq)]
pub struct TextLayout {
    pub glyphs: Vec<GlyphPlacement>,
    /// Overall layout bounds: (width, height) in layout-space pixels.
    pub bounds: (f32, f32),
}

fn is_half_digit(c: char) -> bool {
    c.is_ascii_digit()
}

fn is_tcy_punct_char(c: char) -> bool {
    matches!(c, '!' | '?')
}

/// 行頭禁則: characters that must not begin a wrapped line/column (closing
/// brackets, trailing punctuation, small kana, prolonged-sound/iteration marks,
/// ellipsis). A subset of JIS X 4051; covers the cases that visibly look wrong
/// in Japanese if left at a line head.
fn is_line_start_prohibited(c: char) -> bool {
    matches!(
        c,
        // closing brackets / quotes
        ')' | ']' | '}' | '）' | '］' | '｝' | '〕' | '〉' | '》' | '」' | '』' | '】'
        | '〙' | '〗' | '｠' | '〟' | '’' | '”' | '»'
        // trailing punctuation
        | '、' | '。' | '，' | '．' | '・' | '：' | '；' | ',' | '.' | '!' | '?'
        | '！' | '？' | '‼' | '⁇' | '⁈' | '⁉' | '､'
        // small kana (拗促音)
        | 'ぁ' | 'ぃ' | 'ぅ' | 'ぇ' | 'ぉ' | 'っ' | 'ゃ' | 'ゅ' | 'ょ' | 'ゎ' | 'ゕ' | 'ゖ'
        | 'ァ' | 'ィ' | 'ゥ' | 'ェ' | 'ォ' | 'ッ' | 'ャ' | 'ュ' | 'ョ' | 'ヮ' | 'ヵ' | 'ヶ'
        // prolonged sound / iteration / ellipsis / dakuten-ish
        | 'ー' | '〜' | '～' | '々' | 'ゝ' | 'ゞ' | 'ヽ' | 'ヾ' | '…' | '‥' | '゛' | '゜'
    )
}

/// 行末禁則: characters that must not end a wrapped line/column (opening
/// brackets/quotes — they should travel down to the next line with their content).
fn is_line_end_prohibited(c: char) -> bool {
    matches!(
        c,
        '(' | '['
            | '{'
            | '（'
            | '［'
            | '｛'
            | '〔'
            | '〈'
            | '《'
            | '「'
            | '『'
            | '【'
            | '〘'
            | '〖'
            | '｟'
            | '〝'
            | '‘'
            | '“'
            | '«'
    )
}

/// True for chars that form an unbreakable Latin "word" (don't wrap mid-word).
fn is_latin_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
}

/// Break one already-marker-stripped hard line into sub-lines that each fit
/// within `max_w` pixels (main-axis advance), applying Japanese kinsoku and
/// keeping Latin words intact.
///
/// Algorithm: greedy fill by cumulative `advance(ch)` (which includes the
/// letter_gap), then adjust the chosen break: (1) never split a run of Latin
/// word chars — back up to the word start; (2) **追い出し (push-out)** kinsoku —
/// while the line would end on a 行末禁則 opener or the next line would begin on
/// a 行頭禁則 closer/punct, move the break one char earlier. Push-out never
/// overflows the line (it only shortens it) and always leaves ≥1 char per line,
/// so it terminates. An over-long single word/char is emitted as-is (overflow).
fn wrap_line(chars: &[char], max_w: f32, advance: &impl Fn(char) -> f32) -> Vec<Vec<char>> {
    if chars.is_empty() {
        return vec![Vec::new()];
    }
    let n = chars.len();
    let mut lines: Vec<Vec<char>> = Vec::new();
    let mut start = 0;
    while start < n {
        // Greedy: extend while it fits (always take at least one char).
        let mut end = start;
        let mut w = 0.0f32;
        while end < n {
            let cw = advance(chars[end]);
            if end > start && w + cw > max_w {
                break;
            }
            w += cw;
            end += 1;
        }
        if end < n {
            // (1) kinsoku push-out: move the break left off a prohibited boundary
            // (line ending on an opener, or next line starting on a closer/punct).
            let mut guard = 0;
            while end > start + 1 && guard < n {
                let ends_bad = is_line_end_prohibited(chars[end - 1]);
                let starts_bad = is_line_start_prohibited(chars[end]);
                if ends_bad || starts_bad {
                    end -= 1;
                    guard += 1;
                } else {
                    break;
                }
            }
            // (2) Latin word integrity (has the final say so kinsoku can't split a
            // word): never break inside a run of Latin word chars.
            if end < n && is_latin_word_char(chars[end - 1]) && is_latin_word_char(chars[end]) {
                let mut ws = end;
                while ws > 0 && is_latin_word_char(chars[ws - 1]) {
                    ws -= 1;
                }
                if ws > start {
                    // The word begins after the line start: break before it.
                    end = ws;
                } else {
                    // The word begins at/before the line start and is itself wider
                    // than max_w: emit the whole word (overflow) rather than split.
                    let mut we = end;
                    while we < n && is_latin_word_char(chars[we]) {
                        we += 1;
                    }
                    end = we;
                }
            }
        }
        lines.push(chars[start..end].to_vec());
        start = end;
    }
    lines
}

/// The char a cluster contributes to kinsoku checks (only single upright glyphs
/// participate; Tcy / Sideways runs are explicit markup units and stay whole).
fn cluster_kinsoku_char(cluster: &Cluster) -> Option<char> {
    match cluster {
        Cluster::Single(c) => Some(*c),
        // A grapheme's base char drives kinsoku (the extenders ride with it).
        Cluster::Grapheme(cs) => cs.first().copied(),
        _ => None,
    }
}

/// Break a column's clusters into sub-columns each fitting `max_h` pixels along
/// the column, applying the same 追い出し kinsoku at cluster boundaries. Tcy /
/// Sideways clusters are atomic (never split, never kinsoku-prohibited).
fn wrap_column(clusters: Vec<Cluster>, heights: &[f32], max_h: f32) -> Vec<Vec<Cluster>> {
    if clusters.is_empty() {
        return vec![Vec::new()];
    }
    let n = clusters.len();
    let mut cols: Vec<Vec<Cluster>> = Vec::new();
    let mut start = 0;
    while start < n {
        let mut end = start;
        let mut h = 0.0f32;
        while end < n {
            if end > start && h + heights[end] > max_h {
                break;
            }
            h += heights[end];
            end += 1;
        }
        if end < n {
            let mut guard = 0;
            while end > start + 1 && guard < n {
                let ends_bad =
                    cluster_kinsoku_char(&clusters[end - 1]).is_some_and(is_line_end_prohibited);
                let starts_bad =
                    cluster_kinsoku_char(&clusters[end]).is_some_and(is_line_start_prohibited);
                if ends_bad || starts_bad {
                    end -= 1;
                    guard += 1;
                } else {
                    break;
                }
            }
        }
        cols.push(clusters[start..end].to_vec());
        start = end;
    }
    cols
}

/// A run of chars in a column with a known inline direction. `None` direction
/// means an "auto" run (existing auto-縦中横 / single-glyph logic applies).
#[derive(Debug, Clone, PartialEq)]
struct Run {
    chars: Vec<char>,
    dir: Option<InlineDir>,
}

/// Parse one column string into runs using marker markup.
///
/// When `enabled`, scan left-to-right: on hitting a rule's `open` char, find the
/// next matching `close`; the chars between become one marked run with that
/// rule's `dir`. Chars outside any marker pair are accumulated into "auto" runs.
/// Markup is non-nested; an unmatched `open` is treated as a literal char.
/// When `enabled` is false, the whole column is one auto run (markers literal).
fn parse_runs(chars: &[char], enabled: bool, rules: &[MarkupRule]) -> Vec<Run> {
    if !enabled || rules.is_empty() {
        return vec![Run {
            chars: chars.to_vec(),
            dir: None,
        }];
    }
    let mut out: Vec<Run> = Vec::new();
    let mut auto: Vec<char> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // Does this char open a rule?
        if let Some(rule) = rules.iter().find(|r| r.open == c) {
            // Find the matching close after i.
            if let Some(close_off) = chars[i + 1..].iter().position(|&x| x == rule.close) {
                let close_idx = i + 1 + close_off;
                // Flush any pending auto chars before this marked run.
                if !auto.is_empty() {
                    out.push(Run {
                        chars: std::mem::take(&mut auto),
                        dir: None,
                    });
                }
                out.push(Run {
                    chars: chars[i + 1..close_idx].to_vec(),
                    dir: Some(rule.dir),
                });
                i = close_idx + 1;
                continue;
            }
            // Unmatched open: literal char.
        }
        auto.push(c);
        i += 1;
    }
    if !auto.is_empty() {
        out.push(Run {
            chars: auto,
            dir: None,
        });
    }
    out
}

/// A laid-out cluster occupying some along-column extent.
#[derive(Debug, Clone, PartialEq)]
enum Cluster {
    /// A single upright glyph (one cell, one scalar codepoint).
    Single(char),
    /// One upright grapheme that spans several codepoints in ONE cell: a base
    /// char followed by combining marks and/or variation selectors. Shaped
    /// together so the font's IVS (cmap-14) variant selection and mark
    /// positioning apply (e.g. a name kanji `辻` + U+E0100, or decomposed `か`
    /// + U+3099). `chars[0]` is the base (drives kinsoku).
    Grapheme(Vec<char>),
    /// A horizontal cluster of chars drawn centered in one cell (縦中横).
    Tcy(Vec<char>),
    /// A run of glyphs rotated 90° and stacked down the column (横倒し).
    Sideways(Vec<char>),
}

/// True for codepoints that attach to a preceding base in the same vertical cell:
/// combining marks (incl. Japanese 結合濁点/半濁点 U+3099/309A) and variation
/// selectors (VS1–16 + the IVS supplement). This is a deliberately small,
/// dependency-free classification used only to decide CELL grouping — the actual
/// variant/mark glyph is chosen by the shaper (rustybuzz) + the font's tables.
fn is_grapheme_extender(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F   // combining diacritical marks (Latin)
        | 0x1AB0..=0x1AFF // combining diacritical marks extended
        | 0x1DC0..=0x1DFF // combining diacritical marks supplement
        | 0x20D0..=0x20FF // combining diacritical marks for symbols
        | 0x3099..=0x309A // combining katakana-hiragana (han)dakuten
        | 0xFE00..=0xFE0F // variation selectors VS1–16
        | 0xFE20..=0xFE2F // combining half marks
        | 0xE0100..=0xE01EF // variation selectors supplement (IVS, VS17–256)
    )
}

/// Build upright cells from `chars`, grouping each base with its trailing
/// grapheme extenders (combining marks / variation selectors) into one cell:
/// a lone scalar becomes `Single`, a base + extenders becomes `Grapheme`.
fn push_upright_cells(out: &mut Vec<Cluster>, chars: &[char]) {
    let mut i = 0;
    while i < chars.len() {
        let mut j = i + 1;
        while j < chars.len() && is_grapheme_extender(chars[j]) {
            j += 1;
        }
        if j - i == 1 {
            out.push(Cluster::Single(chars[i]));
        } else {
            out.push(Cluster::Grapheme(chars[i..j].to_vec()));
        }
        i = j;
    }
}

/// Build clusters for one column from its parsed runs.
///
/// - auto run (`dir == None`)  -> existing auto-縦中横 logic (gated by `auto_tcy`).
/// - `TateChuYoko` run         -> one `Tcy(run chars)`.
/// - `Sideways` run            -> one `Sideways(run chars)`.
/// - `Upright` run             -> each char a `Single` (forced upright, incl. digits).
fn cluster_column_from_runs(runs: &[Run], auto_tcy: bool) -> Vec<Cluster> {
    let mut out = Vec::new();
    for run in runs {
        match run.dir {
            None => out.extend(auto_clusters(&run.chars, auto_tcy)),
            Some(InlineDir::TateChuYoko) => {
                if !run.chars.is_empty() {
                    out.push(Cluster::Tcy(run.chars.clone()));
                }
            }
            Some(InlineDir::Sideways) => {
                if !run.chars.is_empty() {
                    out.push(Cluster::Sideways(run.chars.clone()));
                }
            }
            Some(InlineDir::Upright) => {
                push_upright_cells(&mut out, &run.chars);
            }
        }
    }
    out
}

/// Detect minimal auto-縦中横 clusters in an auto run of chars.
///
/// Rules (when `auto_tcy`):
/// - runs of 2–3 consecutive half-width digits become one Tcy.
/// - the mixed pairs `!?` / `?!` become one Tcy.
/// - pure punctuation runs (`!!`, `!!!!!` …) stay upright, one per cell.
/// - everything else is upright, one glyph per cell.
///
/// When `auto_tcy` is false, every glyph is upright (one per cell).
fn auto_clusters(chars: &[char], auto_tcy: bool) -> Vec<Cluster> {
    if !auto_tcy {
        // Everything stacks upright, one cell per grapheme (base + extenders).
        let mut out = Vec::new();
        push_upright_cells(&mut out, chars);
        return out;
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];

        // 2..=3 consecutive half-width digits -> 縦中横.
        if is_half_digit(c) {
            let mut j = i;
            while j < chars.len() && is_half_digit(chars[j]) && (j - i) < 3 {
                j += 1;
            }
            if j - i >= 2 {
                out.push(Cluster::Tcy(chars[i..j].to_vec()));
                i = j;
                continue;
            }
        }

        // Mixed pairs !? / ?! -> 縦中横. Pure punctuation runs (!!, !!!!! …) are
        // left upright (stacked vertically), matching manga convention — this is
        // why `こんばんわ!!!!!` no longer splits into `!!!!` + `!`.
        if is_tcy_punct_char(c) && i + 1 < chars.len() && is_tcy_punct_char(chars[i + 1]) {
            let pair = (c, chars[i + 1]);
            if pair == ('!', '?') || pair == ('?', '!') {
                out.push(Cluster::Tcy(vec![c, chars[i + 1]]));
                i += 2;
                continue;
            }
        }

        // Upright base: attach trailing combining marks / variation selectors so
        // the grapheme stays in one cell and is shaped together (IVS variant / mark
        // positioning). A lone scalar stays a `Single`.
        let mut j = i + 1;
        while j < chars.len() && is_grapheme_extender(chars[j]) {
            j += 1;
        }
        if j - i == 1 {
            out.push(Cluster::Single(c));
        } else {
            out.push(Cluster::Grapheme(chars[i..j].to_vec()));
        }
        i = j;
    }
    out
}

/// Strip marker characters from a string (used by horizontal layout, which
/// ignores directives but should not show the markers literally).
fn strip_markers(text: &str, enabled: bool, rules: &[MarkupRule]) -> String {
    if !enabled || rules.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    for line in text.split('\n') {
        if !out.is_empty() {
            out.push('\n');
        }
        let chars: Vec<char> = line.chars().collect();
        for run in parse_runs(&chars, enabled, rules) {
            out.extend(run.chars.iter());
        }
    }
    out
}

/// Compute glyph placements for a TextBlock (no width constraint: lines/columns
/// break only on explicit `\n`). Used by bubbles and standalone text.
pub fn layout_text(block: &TextBlock, font: &LoadedFont) -> TextLayout {
    layout_text_wrapped(block, font, None)
}

/// Like [`layout_text`] but with an optional main-axis wrap constraint:
/// - Horizontal: `wrap_main_axis` = max line WIDTH (px).
/// - Vertical:   `wrap_main_axis` = max column HEIGHT (px).
///
/// When `Some(w)` and `w > 0`, long lines/columns are wrapped with Japanese
/// kinsoku (see `wrap_line` / `wrap_column`). `None` is identical to
/// [`layout_text`]. Used by message windows, which have a fixed content rect.
pub fn layout_text_wrapped(
    block: &TextBlock,
    font: &LoadedFont,
    wrap_main_axis: Option<f32>,
) -> TextLayout {
    match block.orientation {
        Orientation::Horizontal => layout_horizontal(block, font, wrap_main_axis),
        Orientation::Vertical => layout_vertical(block, font, wrap_main_axis),
    }
}

fn layout_horizontal(block: &TextBlock, font: &LoadedFont, wrap_width: Option<f32>) -> TextLayout {
    let size = block.size_px.max(1.0);
    let ascent = font.ascent(size);
    // Clamp to a small positive: a large negative line_gap could otherwise make
    // line_advance zero/negative, collapsing bounds (which now drive auto-size).
    let line_advance = (font.line_height(size) + block.line_gap).max(1.0);

    // Horizontal mode ignores inline directives but should not render the marker
    // characters literally, so strip them up front.
    let text = strip_markers(&block.text, block.markup_enabled, &block.markup_rules);

    // First pass: per-line glyph runs + line widths so we can apply alignment.
    struct LineRun {
        glyphs: Vec<(char, f32)>, // (ch, pen_x within line)
        width: f32,
    }
    let advance = |ch: char| font.h_advance(ch, size) + block.letter_gap;
    let mut lines: Vec<LineRun> = Vec::new();
    for raw_line in text.split('\n') {
        let raw_chars: Vec<char> = raw_line.chars().collect();
        // Width-constrained wrapping (kinsoku) when a wrap width is given;
        // otherwise the hard line is one sub-line.
        let sub_lines = match wrap_width {
            Some(w) if w > 0.0 => wrap_line(&raw_chars, w, &advance),
            _ => vec![raw_chars],
        };
        for sub in sub_lines {
            let mut pen = 0.0f32;
            let mut glyphs = Vec::new();
            for ch in sub {
                glyphs.push((ch, pen));
                pen += advance(ch);
            }
            // Trailing letter_gap shouldn't count toward visible width.
            let width = (pen - block.letter_gap).max(0.0);
            lines.push(LineRun { glyphs, width });
        }
    }

    let max_w = lines.iter().map(|l| l.width).fold(0.0f32, f32::max);
    let mut placed = Vec::new();
    let mut baseline_y = ascent;
    for line in &lines {
        let offset = match block.align {
            TextAlign::Start => 0.0,
            TextAlign::Center => (max_w - line.width) * 0.5,
            TextAlign::End => max_w - line.width,
        };
        for &(ch, px) in &line.glyphs {
            placed.push(GlyphPlacement::new(
                ch,
                font.glyph_id(ch),
                offset + px,
                baseline_y,
                size,
                GlyphForm::Upright,
            ));
        }
        baseline_y += line_advance;
    }

    let total_h = if lines.is_empty() {
        0.0
    } else {
        line_advance * lines.len() as f32
    };
    TextLayout {
        glyphs: placed,
        bounds: (max_w.max(0.0), total_h.max(0.0)),
    }
}

/// Effective glyph size for a 横倒し run so the **rotated** glyph fits across the
/// column. A glyph rotated 90° spans (across the column) its own ink height, not
/// its advance. If the tallest glyph in the run exceeds `cell * 0.9`, scale the
/// whole run down uniformly so it never bleeds into the neighbouring column
/// (Codex P3: sideways column fit). Measuring per-glyph ink height (rather than
/// the font-global ascent+descent) avoids needlessly shrinking short glyphs.
fn sideways_size(font: &LoadedFont, run: &[char], size: f32, cell: f32) -> f32 {
    let limit = cell * 0.9;
    let extent = run
        .iter()
        .map(|&c| font.glyph_height(c, size))
        .fold(0.0f32, f32::max);
    if extent > limit && extent > 1e-3 {
        (size * (limit / extent)).max(1.0)
    } else {
        size
    }
}

/// Along-column height of one cluster, given the cell size and per-glyph step.
fn cluster_height(
    cluster: &Cluster,
    font: &LoadedFont,
    size: f32,
    glyph_step: f32,
    cell: f32,
) -> f32 {
    match cluster {
        // Single upright glyph / grapheme / 縦中横: one cell.
        Cluster::Single(_) | Cluster::Grapheme(_) | Cluster::Tcy(_) => glyph_step,
        // 横倒し: the rotated word's reading length is the sum of the
        // horizontal advances of its (possibly fit-scaled) glyphs, plus a
        // trailing letter_gap.
        Cluster::Sideways(run) => {
            let ssize = sideways_size(font, run, size, cell);
            let sum: f32 = run.iter().map(|&c| font.h_advance(c, ssize)).sum();
            (sum + (glyph_step - size)).max(glyph_step)
        }
    }
}

/// Place one upright cell (a grapheme = base char + any combining marks /
/// variation selectors) by shaping its codepoints TOGETHER top-to-bottom and
/// emitting the resulting glyphs into the one cell.
///
/// Shaping the whole grapheme (not each scalar) is what makes the font's
/// `vert` feature, IVS variant selection (cmap-14) and combining-mark positioning
/// all apply — so a name kanji `辻`+U+E0100 picks the right variant and a
/// decomposed `か`+U+3099 keeps the dakuten on the base, all within one cell.
/// A lone scalar reduces to a single shaped glyph (identical to the old path).
///
/// The cell's vertical origin is anchored at its top-center (`col_center_x`,
/// `cell_top`); manga cells keep a uniform `glyph_step`, so the shaped advances
/// only position glyphs WITHIN the cell. Shaped offsets/advances are font y-up px,
/// flipped into the layout's y-down space.
fn place_upright_grapheme(
    placed: &mut Vec<GlyphPlacement>,
    font: &LoadedFont,
    chars: &[char],
    base_ch: char,
    size: f32,
    col_center_x: f32,
    cell_top: f32,
) {
    let text: String = chars.iter().collect();
    let glyphs = font.shape_run(&text, size, true);
    let mut pen_x = col_center_x;
    let mut pen_y = 0.0f32; // accumulated y-advance in shaped (y-up) space
    for sg in &glyphs {
        let bx = pen_x + sg.x_offset;
        let by = cell_top - (pen_y + sg.y_offset);
        placed.push(GlyphPlacement::new(
            base_ch,
            sg.gid,
            bx,
            by,
            size,
            GlyphForm::Upright,
        ));
        pen_x += sg.x_advance;
        pen_y += sg.y_advance;
    }
}

fn layout_vertical(block: &TextBlock, font: &LoadedFont, wrap_height: Option<f32>) -> TextLayout {
    let size = block.size_px.max(1.0);
    // Column width = one full glyph cell (use a CJK reference advance). For
    // monospace-ish CJK this is ~= size; use the em ('M' advance) as proxy and
    // fall back to `size` if the font reports nothing useful.
    let cell = {
        let m = font.h_advance('\u{3042}', size); // あ as CJK width proxy
        if m > 1.0 { m } else { size }
    };
    // Clamp to a small positive (mirrors line_advance): a large negative
    // line_gap could otherwise collapse column advance and thus the bounds.
    let col_advance = (cell + block.line_gap).max(1.0);
    // Vertical step between stacked glyphs within a column.
    // Clamp to a small positive value: size_px can be tiny and letter_gap can be
    // negative, which would otherwise give zero/negative cells (inverted layout).
    let glyph_step = (size + block.letter_gap).max(1.0);

    // Each '\n' starts a new column. Columns advance RIGHT-TO-LEFT, so we lay
    // them out left-to-right into a temporary list then mirror x at the end.
    struct Column {
        clusters: Vec<Cluster>,
        height: f32,
    }
    let mut columns: Vec<Column> = Vec::new();
    for raw_col in block.text.split('\n') {
        let chars: Vec<char> = raw_col.chars().collect();
        let runs = parse_runs(&chars, block.markup_enabled, &block.markup_rules);
        let clusters = cluster_column_from_runs(&runs, block.auto_tcy);
        let per_cluster: Vec<f32> = clusters
            .iter()
            .map(|c| cluster_height(c, font, size, glyph_step, cell))
            .collect();
        // Width-constrained wrapping (kinsoku) when a wrap height is given;
        // otherwise the hard column stays whole.
        let sub_cols = match wrap_height {
            Some(h) if h > 0.0 => wrap_column(clusters, &per_cluster, h),
            _ => vec![clusters],
        };
        for sub in sub_cols {
            let height: f32 = sub
                .iter()
                .map(|c| cluster_height(c, font, size, glyph_step, cell))
                .sum();
            columns.push(Column {
                clusters: sub,
                height,
            });
        }
    }

    let max_h = columns.iter().map(|c| c.height).fold(0.0f32, f32::max);
    let n_cols = columns.len().max(1);
    let total_w = col_advance * columns.len() as f32;

    let mut placed = Vec::new();
    for (col_idx, col) in columns.iter().enumerate() {
        // RIGHT-TO-LEFT: the first column sits at the rightmost x.
        let col_left = (n_cols - 1 - col_idx) as f32 * col_advance;
        let col_center_x = col_left + cell * 0.5;
        // Align along the column (Start = top).
        let start_y = match block.align {
            TextAlign::Start => 0.0,
            TextAlign::Center => (max_h - col.height) * 0.5,
            TextAlign::End => max_h - col.height,
        };
        let mut cell_top = start_y;
        for cluster in &col.clusters {
            let ch_h = cluster_height(cluster, font, size, glyph_step, cell);
            match cluster {
                Cluster::Single(ch) => {
                    place_upright_grapheme(
                        &mut placed,
                        font,
                        &[*ch],
                        *ch,
                        size,
                        col_center_x,
                        cell_top,
                    );
                }
                Cluster::Grapheme(chars) => {
                    let base = chars.first().copied().unwrap_or(' ');
                    place_upright_grapheme(
                        &mut placed,
                        font,
                        chars,
                        base,
                        size,
                        col_center_x,
                        cell_top,
                    );
                }
                Cluster::Tcy(run) => {
                    // Lay the run horizontally, centered in the cell. Start at
                    // half the body size; if the run is wider than 90% of the
                    // cell, scale it down so it never overflows into neighboring
                    // columns (fit-to-column, avoids overlap).
                    let mut tcy_size = size * 0.5;
                    let width_at = |sz: f32| -> f32 {
                        run.iter().map(|&c| font.h_advance(c, sz)).sum::<f32>()
                    };
                    let limit = cell * 0.9;
                    let w0 = width_at(tcy_size);
                    if w0 > limit && w0 > 0.0 {
                        tcy_size *= limit / w0;
                    }
                    let mut widths = Vec::with_capacity(run.len());
                    let mut total = 0.0f32;
                    for &c in run {
                        let w = font.h_advance(c, tcy_size);
                        widths.push(w);
                        total += w;
                    }
                    // Horizontally centered in the cell; vertically centered in
                    // the cell (baseline placed so glyph body sits mid-cell).
                    let mut pen_x = col_center_x - total * 0.5;
                    let y = cell_top + glyph_step * 0.5 + tcy_size * 0.5;
                    for (i, &c) in run.iter().enumerate() {
                        placed.push(GlyphPlacement::new(
                            c,
                            font.glyph_id(c),
                            pen_x,
                            y,
                            tcy_size,
                            GlyphForm::Upright,
                        ));
                        pen_x += widths[i];
                    }
                }
                Cluster::Sideways(run) => {
                    // Each glyph rotated 90° CW, stacked DOWN the column in
                    // reading order (top→bottom), centered across the column.
                    // For sideways glyphs (x, y) are the CENTER of the rotated
                    // cell (see GlyphPlacement docs). The rotated glyph's
                    // along-column extent equals its original horizontal advance.
                    // The size is fit-scaled so the rotated glyph (which spans
                    // its ink height across the column) fits in the cell.
                    let ssize = sideways_size(font, run, size, cell);
                    let mut pen_y = cell_top;
                    for &c in run {
                        let adv = font.h_advance(c, ssize).max(1.0);
                        placed.push(GlyphPlacement::new(
                            c,
                            font.glyph_id(c),
                            col_center_x,
                            pen_y + adv * 0.5,
                            ssize,
                            GlyphForm::Sideways,
                        ));
                        pen_y += adv;
                    }
                }
            }
            cell_top += ch_h;
        }
    }

    TextLayout {
        glyphs: placed,
        bounds: (total_w.max(0.0), max_h.max(0.0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::model::default_markup_rules;

    /// Convenience: parse a plain column (no markup) into auto clusters.
    fn auto_column(s: &str, auto_tcy: bool) -> Vec<Cluster> {
        let chars: Vec<char> = s.chars().collect();
        auto_clusters(&chars, auto_tcy)
    }

    #[test]
    fn repeated_punct_stacks_upright() {
        let clusters = auto_column("!!!!!", true);
        assert_eq!(clusters.len(), 5);
        assert!(clusters.iter().all(|c| matches!(c, Cluster::Single('!'))));
    }

    #[test]
    fn mixed_punct_is_tcy() {
        let clusters = auto_column("!?", true);
        assert_eq!(clusters, vec![Cluster::Tcy(vec!['!', '?'])]);
    }

    #[test]
    fn detects_tcy_digit_run() {
        let clusters = auto_column("12", true);
        assert_eq!(clusters, vec![Cluster::Tcy(vec!['1', '2'])]);
    }

    #[test]
    fn single_digit_is_upright() {
        let clusters = auto_column("5", true);
        assert_eq!(clusters, vec![Cluster::Single('5')]);
    }

    #[test]
    fn plain_char_stays_single() {
        // No extenders -> a lone scalar is still a Single (no allocation/behavior
        // change for the common case).
        let clusters = auto_column("あ", true);
        assert_eq!(clusters, vec![Cluster::Single('あ')]);
    }

    #[test]
    fn ivs_groups_into_one_grapheme_cell() {
        // A name-kanji + ideographic variation selector becomes ONE cell so the
        // base + selector get shaped together (font picks the variant).
        let clusters = auto_column("辻\u{E0100}", true);
        assert_eq!(clusters, vec![Cluster::Grapheme(vec!['辻', '\u{E0100}'])]);
    }

    #[test]
    fn decomposed_dakuten_groups_with_base() {
        // Decomposed (NFD) か + combining voiced mark stays in one cell.
        let clusters = auto_column("か\u{3099}", true);
        assert_eq!(clusters, vec![Cluster::Grapheme(vec!['か', '\u{3099}'])]);
    }

    #[test]
    fn grapheme_grouping_between_plain_chars() {
        // 山 + (辻 + IVS) + 川 -> Single, Grapheme, Single.
        let clusters = auto_column("山辻\u{E0100}川", true);
        assert_eq!(
            clusters,
            vec![
                Cluster::Single('山'),
                Cluster::Grapheme(vec!['辻', '\u{E0100}']),
                Cluster::Single('川'),
            ]
        );
    }

    #[test]
    fn auto_tcy_off_stacks_all() {
        let clusters = auto_column("12", false);
        assert_eq!(clusters, vec![Cluster::Single('1'), Cluster::Single('2')]);
    }

    // ---- marker markup parsing ----

    fn clusters_with_markup(s: &str, enabled: bool, auto_tcy: bool) -> Vec<Cluster> {
        let rules = default_markup_rules();
        let chars: Vec<char> = s.chars().collect();
        let runs = parse_runs(&chars, enabled, &rules);
        cluster_column_from_runs(&runs, auto_tcy)
    }

    #[test]
    fn markup_brackets_make_one_tcy_run() {
        // `[AI]` -> one TateChuYoko cluster containing A, I.
        let clusters = clusters_with_markup("[AI]", true, true);
        assert_eq!(clusters, vec![Cluster::Tcy(vec!['A', 'I'])]);
    }

    #[test]
    fn markup_braces_make_sideways_run() {
        // `{LOVE}` -> one Sideways cluster (default 横倒し marker is `{}` now;
        // 正立 `〔〕` was dropped).
        let clusters = clusters_with_markup("{LOVE}", true, true);
        assert_eq!(clusters, vec![Cluster::Sideways(vec!['L', 'O', 'V', 'E'])]);
    }

    #[test]
    fn markup_literal_when_disabled() {
        // When markup is off the markers are literal text. `[12]` becomes the
        // chars `[`, then auto digit-Tcy `12`, then `]` (all literal).
        let clusters = clusters_with_markup("[12]", false, true);
        assert_eq!(
            clusters,
            vec![
                Cluster::Single('['),
                Cluster::Tcy(vec!['1', '2']),
                Cluster::Single(']'),
            ]
        );
    }

    #[test]
    fn markup_mixes_auto_and_marked_runs() {
        // `あ[AI]い` -> auto あ, Tcy(A,I), auto い.
        let clusters = clusters_with_markup("あ[AI]い", true, true);
        assert_eq!(
            clusters,
            vec![
                Cluster::Single('あ'),
                Cluster::Tcy(vec!['A', 'I']),
                Cluster::Single('い'),
            ]
        );
    }

    #[test]
    fn markup_unmatched_open_is_literal() {
        // `[AI` with no closing bracket -> markers literal, plain auto run.
        let clusters = clusters_with_markup("[AI", true, true);
        assert_eq!(
            clusters,
            vec![
                Cluster::Single('['),
                Cluster::Single('A'),
                Cluster::Single('I'),
            ]
        );
    }
}

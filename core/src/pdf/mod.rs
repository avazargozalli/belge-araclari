//! PDF → UDF. [`read`] collects what each page draws; this module rebuilds the document from it
//! the way a person retyping it would: characters into lines (with spaces, tab jumps, superscripts,
//! underlines from the rules drawn under them, highlights from the boxes behind them), lines into
//! paragraphs (alignment, indents, line spacing and space above measured from their positions),
//! ruled grids into tables (merged cells where an inner line is missing), and repeated lines and
//! pictures at the top or bottom of every page into the header, footer, page number and
//! letterhead. Pages without text (scans) become pictures. Everything is written through
//! [`crate::udfout`], so the result follows the Editör's rules.

pub mod read;

use std::collections::HashMap;
use std::path::Path;

use read::{Glyph, Page, Picture, Rule};

use crate::udfout::{self, Background, Block, Cell, Document, Line as Spacing, Para, ParaProps, Piece, RunProps, TabOp, Table};

// MARK: - Lines

#[derive(Clone)]
enum Item {
    Glyph(Glyph, RunProps),
    Space(RunProps),
    /// Jump to this x (a tab stop), with its leader (0 none, 1 dots, 3 line).
    Tab(f32, u8),
}

#[derive(Clone)]
struct TextLine {
    items: Vec<Item>,
    x0: f32,
    x1: f32,
    /// Baseline, top-down.
    base: f32,
    size: f32,
}

impl TextLine {
    fn top(&self) -> f32 {
        self.base - self.size * 0.85
    }

    fn bottom(&self) -> f32 {
        self.base + self.size * 0.25
    }

    fn text(&self) -> String {
        self.items
            .iter()
            .map(|i| match i {
                Item::Glyph(g, _) => g.ch,
                Item::Space(_) => ' ',
                Item::Tab(..) => '\t',
            })
            .collect()
    }

    fn has_tab(&self) -> bool {
        self.items.iter().any(|i| matches!(i, Item::Tab(..)))
    }

    fn all_bold(&self) -> bool {
        let glyphs: Vec<&RunProps> = self.items.iter().filter_map(|i| if let Item::Glyph(g, p) = i { (!g.ch.is_whitespace()).then_some(p) } else { None }).collect();
        !glyphs.is_empty() && glyphs.iter().all(|p| p.bold == Some(true))
    }

    /// Width of the first word, to tell whether it would have fitted on the line before.
    fn first_word_width(&self) -> f32 {
        let mut start = None;
        let mut end = 0.0;
        for i in &self.items {
            match i {
                Item::Glyph(g, _) if !g.ch.is_whitespace() => {
                    start.get_or_insert(g.x);
                    end = g.x + g.w;
                }
                _ if start.is_some() => break,
                _ => {}
            }
        }
        start.map_or(0.0, |s| end - s)
    }
}

fn round_half(v: f32) -> f32 {
    (v * 2.0).round() / 2.0
}

fn props_of(g: &Glyph) -> RunProps {
    RunProps {
        font: Some(g.font.family.clone()),
        size: Some(round_half(g.size)),
        bold: Some(g.font.bold),
        italic: Some(g.font.italic),
        underline: Some(false),
        strike: Some(false),
        color: Some((g.color != 0).then_some(g.color)),
        background: Some(None),
        vert: Some(0),
        ..Default::default()
    }
}

/// Groups a page's characters into lines, top to bottom.
fn lines(page: &Page) -> Vec<TextLine> {
    let mut glyphs: Vec<&Glyph> = page.glyphs.iter().filter(|g| g.x > -5.0 && g.x < page.width + 5.0 && g.y > -5.0 && g.y < page.height + 5.0).collect();
    glyphs.sort_by(|a, b| a.y.total_cmp(&b.y));
    let mut groups: Vec<Vec<&Glyph>> = vec![];
    for g in glyphs {
        // Same line when the baselines are within half a character height (superscripts too).
        let joined = groups.iter_mut().rev().take(3).find(|line| {
            let base = line.iter().map(|x| x.y).fold(f32::MIN, f32::max);
            let size = line.iter().map(|x| x.size).fold(0.0, f32::max).max(g.size);
            (g.y - base).abs() < size * 0.45 || (g.y < base && base - g.y < size * 0.5 && g.size < size * 0.8)
        });
        match joined {
            Some(line) => line.push(g),
            None => groups.push(vec![g]),
        }
    }
    let mut out = vec![];
    for mut group in groups {
        group.sort_by(|a, b| a.x.total_cmp(&b.x));
        // Fake bold is often the same text drawn twice with a tiny offset.
        group.dedup_by(|a, b| a.ch == b.ch && (a.x - b.x).abs() < a.w.max(0.5) * 0.4 && (a.y - b.y).abs() < 0.5);
        let size = {
            let mut sizes: HashMap<i32, usize> = HashMap::new();
            for g in &group {
                *sizes.entry((g.size * 2.0).round() as i32).or_default() += 1;
            }
            sizes.into_iter().max_by_key(|(s, n)| (*n, *s)).map(|(s, _)| s as f32 / 2.0).unwrap_or(12.0)
        };
        let base = group.iter().filter(|g| (g.size - size).abs() < 0.6).map(|g| g.y).fold(f32::MIN, f32::max);
        let base = if base == f32::MIN { group[0].y } else { base };
        let mut items: Vec<Item> = vec![];
        let mut prev: Option<&Glyph> = None;
        for g in &group {
            let mut p = props_of(g);
            if g.size < size * 0.85 && g.y < base - size * 0.2 {
                p.vert = Some(1);
                p.size = Some(round_half(size));
            } else if g.size < size * 0.85 && g.y > base + size * 0.1 {
                p.vert = Some(-1);
                p.size = Some(round_half(size));
            }
            if let Some(q) = prev {
                let gap = g.x - (q.x + q.w);
                let blank = g.ch.is_whitespace() || q.ch.is_whitespace();
                // A list label ("1.", "a)", "•") followed by a wide gap: the text starts at a tab.
                let label: String = items.iter().filter_map(|i| if let Item::Glyph(x, _) = i { Some(x.ch) } else { None }).collect();
                let after_label = !label.is_empty() && is_list_start(&format!("{label} x")) && gap > size * 0.4;
                if gap > size * 2.2 || after_label {
                    if q.ch.is_whitespace() {
                        // "Label :   value": the space already there, then a jump.
                        items.pop();
                    }
                    items.push(Item::Tab(g.x, 0));
                } else if gap > size * 0.17 && !blank {
                    items.push(Item::Space(props_of(q)));
                }
            }
            if g.ch == ' ' && matches!(items.last(), Some(Item::Space(_)) | Some(Item::Tab(..)) | None) {
                prev = Some(g);
                continue;
            }
            items.push(Item::Glyph((*g).clone(), p));
            prev = Some(g);
        }
        while matches!(items.last(), Some(Item::Space(_)) | Some(Item::Glyph(Glyph { ch: ' ', .. }, _))) {
            items.pop();
        }
        let visible: Vec<&Glyph> = items.iter().filter_map(|i| if let Item::Glyph(g, _) = i { (!g.ch.is_whitespace()).then_some(g) } else { None }).collect();
        let (Some(first), Some(last)) = (visible.first(), visible.last()) else { continue };
        out.push(TextLine { x0: first.x, x1: last.x + last.w, base, size, items });
    }
    out.sort_by(|a, b| a.base.total_cmp(&b.base));
    out
}

/// Underlines, strike-throughs and highlights, from rules drawn under, through or behind text.
fn decorate(lines: &mut [TextLine], rules: &mut Vec<Rule>) {
    let mut used = vec![false; rules.len()];
    for line in lines.iter_mut() {
        let (lx0, lx1) = (line.x0, line.x1);
        let edges: Vec<(f32, f32)> = line.items.iter().filter_map(|i| if let Item::Glyph(g, _) = i { Some((g.x, g.x + g.w)) } else { None }).collect();
        // An underline starts and ends where letters do; a border does not.
        let fits = |r: &Rule| edges.iter().any(|e| (e.0 - r.x0).abs() < 3.0) && edges.iter().any(|e| (e.1 - r.x1).abs() < 3.0);
        // Tab leaders: a dotted or solid rule along the baseline across the jump.
        let mut prev_end = lx0;
        for item in line.items.iter_mut() {
            match item {
                Item::Glyph(g, _) => prev_end = g.x + g.w,
                Item::Tab(x, leader) => {
                    let (a, b) = (prev_end, *x);
                    let mut covered = 0.0;
                    let mut pieces = 0;
                    for (k, r) in rules.iter().enumerate() {
                        let y = (r.y0 + r.y1) / 2.0;
                        if r.y1 - r.y0 < 2.0 && y > line.base - line.size * 0.3 && y < line.base + line.size * 0.3 && r.x1 > a && r.x0 < b {
                            covered += r.x1.min(b) - r.x0.max(a);
                            pieces += 1;
                            used[k] = true;
                        }
                    }
                    if b - a > 1.0 && covered > (b - a) * 0.3 {
                        *leader = if pieces > 3 { 1 } else { 3 };
                    }
                }
                _ => {}
            }
        }
        for item in line.items.iter_mut() {
            let Item::Glyph(g, p) = item else { continue };
            let cx = g.x + g.w / 2.0;
            for (k, r) in rules.iter().enumerate() {
                if cx < r.x0 || cx > r.x1 {
                    continue;
                }
                let thin = r.y1 - r.y0 < 2.0;
                // An underline runs under the text only; a longer rule is a border.
                let within = r.x0 >= lx0 - 3.0 && r.x1 <= lx1 + 3.0 && fits(r);
                if thin && within && r.y0 >= g.y - 0.5 && r.y0 <= g.y + g.size * 0.3 + 1.0 {
                    p.underline = Some(true);
                    used[k] = true;
                } else if thin && within && r.y0 < g.y - g.size * 0.15 && r.y0 > g.y - g.size * 0.5 {
                    p.strike = Some(true);
                    used[k] = true;
                } else if r.fill && r.y0 <= g.y - g.size * 0.4 && r.y1 >= g.y && r.color != 0xFFFFFF && r.y1 - r.y0 < g.size * 3.0 {
                    p.background = Some(Some(r.color));
                    used[k] = true;
                }
            }
        }
    }
    // A rule used as an underline is not a table border, unless it also meets a vertical line.
    let verticals: Vec<Rule> = rules.iter().filter(|r| r.x1 - r.x0 < 2.0 && r.y1 - r.y0 > 5.0).copied().collect();
    let mut k = 0;
    rules.retain(|r| {
        let keep = !used[k] || verticals.iter().any(|v| v.x0 >= r.x0 - 2.0 && v.x0 <= r.x1 + 2.0 && r.y0 >= v.y0 - 2.0 && r.y0 <= v.y1 + 2.0);
        k += 1;
        keep
    });
}

/// Splits a line at its tab jumps.
fn split_at_tabs(line: &TextLine) -> Vec<TextLine> {
    let mut parts: Vec<Vec<Item>> = vec![vec![]];
    for item in &line.items {
        match item {
            Item::Tab(..) => parts.push(vec![]),
            other => parts.last_mut().unwrap().push(other.clone()),
        }
    }
    parts
        .into_iter()
        .filter_map(|items| {
            let glyphs: Vec<&Glyph> = items.iter().filter_map(|i| if let Item::Glyph(g, _) = i { (!g.ch.is_whitespace()).then_some(g) } else { None }).collect();
            let (first, last) = (glyphs.first()?, glyphs.last()?);
            Some(TextLine { x0: first.x, x1: last.x + last.w, base: line.base, size: line.size, items })
        })
        .collect()
}

// MARK: - Tables

struct Grid {
    xs: Vec<f32>,
    ys: Vec<f32>,
    /// Per row: (first column, span).
    cells: Vec<Vec<(usize, usize)>>,
    border: &'static str,
}

impl Grid {
    fn bbox(&self) -> (f32, f32, f32, f32) {
        (self.xs[0], self.ys[0], *self.xs.last().unwrap(), *self.ys.last().unwrap())
    }
}

fn cluster(values: &mut Vec<f32>) {
    values.sort_by(|a, b| a.total_cmp(b));
    let mut out: Vec<f32> = vec![];
    for v in values.iter() {
        match out.last() {
            Some(l) if v - l < 2.5 => {}
            _ => out.push(*v),
        }
    }
    *values = out;
}

/// Ruled tables: horizontal and vertical lines that meet. Columns come from vertical lines, or,
/// in tables drawn with row lines only, from where the row lines break and the text leaves gaps.
fn grids(rules: &[Rule], lines: &[TextLine]) -> Vec<Grid> {
    let h: Vec<&Rule> = rules.iter().filter(|r| r.y1 - r.y0 < 2.0 && r.x1 - r.x0 > 8.0).collect();
    let v: Vec<&Rule> = rules.iter().filter(|r| r.x1 - r.x0 < 2.0 && r.y1 - r.y0 > 5.0).collect();
    let n = h.len() + v.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut Vec<usize>, i: usize) -> usize {
        let mut i = i;
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for (a, hr) in h.iter().enumerate() {
        for (b, vr) in v.iter().enumerate() {
            let x = (vr.x0 + vr.x1) / 2.0;
            let y = (hr.y0 + hr.y1) / 2.0;
            if x >= hr.x0 - 2.5 && x <= hr.x1 + 2.5 && y >= vr.y0 - 2.5 && y <= vr.y1 + 2.5 {
                let (ra, rb) = (find(&mut parent, a), find(&mut parent, h.len() + b));
                parent[ra] = rb;
            }
        }
    }
    let mut groups: HashMap<usize, (Vec<&Rule>, Vec<&Rule>)> = HashMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        let entry = groups.entry(root).or_default();
        if i < h.len() { entry.0.push(h[i]) } else { entry.1.push(v[i - h.len()]) }
    }
    let mut out = vec![];
    for (hs, vs) in groups.into_values() {
        if hs.len() < 2 || vs.len() < 2 {
            continue;
        }
        let mut xs: Vec<f32> = vs.iter().map(|r| (r.x0 + r.x1) / 2.0).collect();
        let mut ys: Vec<f32> = hs.iter().map(|r| (r.y0 + r.y1) / 2.0).collect();
        cluster(&mut xs);
        // Break points shared by several row lines, away from any vertical line: invisible columns.
        let mut breaks: Vec<f32> = hs.iter().flat_map(|r| [r.x0, r.x1]).collect();
        breaks.sort_by(|a, b| a.total_cmp(b));
        let mut virtual_xs: Vec<f32> = vec![];
        for x in &breaks {
            let shared = hs.iter().filter(|r| (r.x0 - x).abs() < 1.5 || (r.x1 - x).abs() < 1.5).count();
            let inside = *x > xs[0] + 3.0 && *x < xs[xs.len() - 1] - 3.0;
            if shared >= 4 && inside && !virtual_xs.iter().any(|v| (v - x).abs() < 3.0) {
                virtual_xs.push(*x);
            }
        }
        xs.extend(virtual_xs.iter().copied());
        cluster(&mut xs);
        cluster(&mut ys);
        if xs.len() < 2 || ys.len() < 2 || xs.len() > 40 || ys.len() > 400 {
            continue;
        }
        let drawn = |x: f32, top: f32, bottom: f32| vs.iter().any(|r| ((r.x0 + r.x1) / 2.0 - x).abs() < 2.5 && r.y0 <= top + (bottom - top) * 0.3 && r.y1 >= bottom - (bottom - top) * 0.3);
        // An invisible column edge counts unless text in that row runs across it.
        let has_v = |x: f32, top: f32, bottom: f32| {
            drawn(x, top, bottom)
                || virtual_xs.iter().any(|v| (v - x).abs() < 3.0)
                    && !lines.iter().any(|l| l.base > top && l.base < bottom && l.items.windows(2).any(|w| match (&w[0], &w[1]) {
                        (Item::Glyph(a, _), Item::Glyph(b, _)) => a.x < x - 1.0 && b.x + b.w > x + 1.0 && b.x - (a.x + a.w) < l.size * 0.6,
                        (Item::Glyph(a, _), Item::Space(_)) => a.x < x - 1.0 && a.x + a.w + l.size * 0.5 > x + 1.0,
                        _ => false,
                    }))
        };
        let has_h = |y: f32, left: f32, right: f32| hs.iter().any(|r| ((r.y0 + r.y1) / 2.0 - y).abs() < 2.5 && r.x0 <= left + (right - left) * 0.3 && r.x1 >= right - (right - left) * 0.3);
        let mut cells = vec![];
        let (mut inner_v, mut inner_v_missing, mut inner_h, mut outer) = (false, false, false, true);
        for r in 0..ys.len() - 1 {
            let (top, bottom) = (ys[r], ys[r + 1]);
            let mut row = vec![];
            let mut start = 0;
            for c in 1..xs.len() {
                let boundary = c == xs.len() - 1 || has_v(xs[c], top, bottom);
                if c < xs.len() - 1 {
                    if drawn(xs[c], top, bottom) { inner_v = true } else { inner_v_missing = true }
                }
                if boundary {
                    row.push((start, c - start));
                    start = c;
                }
            }
            cells.push(row);
            if r > 0 && has_h(top, xs[0], *xs.last().unwrap()) {
                inner_h = true;
            }
        }
        outer &= has_v(xs[0], ys[0], *ys.last().unwrap()) && has_v(*xs.last().unwrap(), ys[0], *ys.last().unwrap());
        let border = match (outer, inner_h, inner_v) {
            (_, true, true) => "borderCell",
            (true, true, false) => "borderRow",
            (true, false, false) => "borderTable",
            _ => "borderCell",
        };
        let _ = inner_v_missing;
        out.push(Grid { xs, ys, cells, border });
    }
    out.sort_by(|a, b| a.ys[0].total_cmp(&b.ys[0]));
    out
}

// MARK: - Paragraphs

/// Typical baseline-to-baseline distance for each font size, from consecutive lines.
fn pitches(pages: &[Vec<TextLine>]) -> HashMap<i32, f32> {
    let mut samples: HashMap<i32, Vec<f32>> = HashMap::new();
    for lines in pages {
        for w in lines.windows(2) {
            let p = w[1].base - w[0].base;
            if (w[0].size - w[1].size).abs() < 0.6 && p > w[0].size * 0.9 && p < w[0].size * 1.9 {
                samples.entry((w[0].size * 2.0).round() as i32).or_default().push(p);
            }
        }
    }
    samples
        .into_iter()
        .map(|(k, mut v)| {
            v.sort_by(|a, b| a.total_cmp(b));
            (k, v[v.len() / 4])
        })
        .collect()
}

struct Region {
    left: f32,
    right: f32,
}

fn is_list_start(text: &str) -> bool {
    let t = text.trim_start();
    let mut chars = t.chars();
    let first = chars.next();
    if matches!(first, Some('•' | '▪' | '◦' | '–' | '-' | '➢' | '►' | '■' | '□' | '●')) && t.chars().nth(1).is_some_and(char::is_whitespace) {
        return true;
    }
    let head: String = t.chars().take_while(|c| c.is_alphanumeric()).collect();
    let rest = &t[head.len()..];
    let marker = rest.starts_with(['.', ')', '-']) && rest[1..].starts_with([' ', '\t']);
    let short_head = !head.is_empty() && head.chars().count() <= 4;
    let numeric = head.chars().all(|c| c.is_ascii_digit());
    let roman = head.chars().all(|c| "ivxlcIVXLC".contains(c));
    let letter = head.chars().count() == 1;
    marker && short_head && (numeric || roman || letter) || (t.starts_with('(') && t[1..].find(')').is_some_and(|i| i <= 4 && t[i + 2..].starts_with(' ')))
}

/// Should `cur` start a new paragraph after `prev`?
fn breaks(prev: &TextLine, cur: &TextLine, region: &Region, typical: f32, para_lines: usize, wrap: Option<f32>, same_page: bool) -> bool {
    if same_page && cur.base - prev.base > typical * 1.3 + 0.5 {
        return true;
    }
    if prev.has_tab() || cur.has_tab() || (prev.size - cur.size).abs() > 0.6 || prev.all_bold() != cur.all_bold() {
        return true;
    }
    if is_list_start(&cur.text()) {
        return true;
    }
    // Stacked right-aligned (or centred) lines, like a signature block, are separate lines.
    let width = region.right - region.left;
    let right_aligned = |l: &TextLine| region.right - l.x1 < 3.0 && l.x0 - region.left > width * 0.25;
    if right_aligned(prev) && right_aligned(cur) && (prev.x0 - cur.x0).abs() > 1.5 {
        return true;
    }
    // The first word of this line would have fitted on the previous one: that line ended on purpose.
    if region.right - prev.x1 > cur.first_word_width() + cur.size * 0.35 {
        return true;
    }
    if para_lines >= 2 {
        if let Some(w) = wrap {
            if (cur.x0 - w).abs() > 1.5 {
                return true;
            }
        }
    }
    false
}

fn para_from(lines: &[TextLine], region: &Region, typical: f32, space_above: f32) -> Para {
    let first = &lines[0];
    let width = region.right - region.left;
    let gap_l = |l: &TextLine| l.x0 - region.left;
    let gap_r = |l: &TextLine| region.right - l.x1;
    let centered = |l: &TextLine| (gap_l(l) - gap_r(l)).abs() < 4.0 && gap_l(l) > 6.0;
    let mut props = ParaProps::default();
    let n = lines.len();
    let (align, left, first_indent) = if n == 1 {
        if centered(first) {
            (1, 0.0, 0.0)
        } else if gap_r(first) < 3.0 && gap_l(first) > width * 0.3 {
            (2, 0.0, 0.0)
        } else {
            (0, gap_l(first), 0.0)
        }
    } else {
        let wrap = lines[1..].iter().map(|l| l.x0).fold(f32::MAX, f32::min);
        let left = wrap - region.left;
        let first_indent = first.x0 - wrap;
        let body = &lines[..n - 1];
        let ragged_left = lines[1..].iter().any(|l| (l.x0 - wrap).abs() > 1.5);
        if lines.iter().all(centered) {
            (1, 0.0, 0.0)
        } else if ragged_left && lines.iter().all(|l| gap_r(l) < 3.0) {
            (2, 0.0, 0.0)
        } else if body.iter().all(|l| gap_r(l) < 3.0) {
            (3, left, first_indent)
        } else if lines.iter().all(|l| gap_r(l) < 3.0) && gap_l(first) > 20.0 {
            (2, 0.0, 0.0)
        } else {
            (0, left, first_indent)
        }
    };
    props.align = Some(align);
    props.left = Some(left.max(0.0));
    props.first = Some(first_indent);
    props.before = Some(space_above.max(0.0));
    props.after = Some(0.0);
    let pitch = if n > 1 { (lines[n - 1].base - first.base) / (n - 1) as f32 } else { typical };
    let ls = ((pitch / (first.size * 1.15) - 1.0) * 100.0).round() / 100.0;
    props.line = Some(Spacing::Auto(1.0 + ls.clamp(-0.2, 3.0)));
    // Tab jumps become left tab stops at the same place.
    for line in lines {
        for item in &line.items {
            if let Item::Tab(x, leader) = item {
                // A leader runs up to text that ends at the stop: a right-aligned stop there.
                props.tabs.push(TabOp::Set(x - region.left, 0, *leader));
            }
        }
    }
    let mark = lines.iter().flat_map(|l| l.items.iter()).find_map(|i| if let Item::Glyph(_, p) = i { Some(p.clone()) } else { None }).unwrap_or_default();
    let mut pieces: Vec<Piece> = vec![];
    let push = |pieces: &mut Vec<Piece>, text: &str, p: &RunProps| {
        if let Some(Piece::Text(last, lp)) = pieces.last_mut() {
            if lp.same_look(p) {
                last.push_str(text);
                return;
            }
        }
        pieces.push(Piece::Text(text.to_string(), p.clone()));
    };
    for (li, line) in lines.iter().enumerate() {
        if li > 0 {
            // Words broken with a hyphen at the end of a justified line are joined again.
            let hyphenated = matches!(pieces.last(), Some(Piece::Text(t, _)) if t.ends_with('-') && t.chars().rev().nth(1).is_some_and(char::is_alphabetic))
                && line.text().chars().next().is_some_and(char::is_lowercase)
                && region.right - lines[li - 1].x1 < 3.0;
            if hyphenated {
                if let Some(Piece::Text(t, _)) = pieces.last_mut() {
                    t.pop();
                }
            } else {
                let p = pieces.iter().rev().find_map(|x| if let Piece::Text(_, p) = x { Some(p.clone()) } else { None }).unwrap_or_else(|| mark.clone());
                let mut p = p;
                p.vert = Some(0);
                push(&mut pieces, " ", &p);
            }
        }
        for item in &line.items {
            match item {
                Item::Glyph(g, p) => push(&mut pieces, &g.ch.to_string(), p),
                Item::Space(p) => push(&mut pieces, " ", p),
                Item::Tab(..) => {
                    let p = pieces.iter().rev().find_map(|x| if let Piece::Text(_, p) = x { Some(p.clone()) } else { None }).unwrap_or_else(|| mark.clone());
                    push(&mut pieces, "\t", &p);
                }
            }
        }
    }
    let mut mark = mark;
    mark.underline = Some(false);
    mark.strike = Some(false);
    mark.background = Some(None);
    mark.vert = Some(0);
    Para { props, mark, pieces, ..Default::default() }
}

/// Lines (top to bottom, one region) → paragraphs.
fn paragraphs(lines: &[TextLine], region: &Region, typical: &HashMap<i32, f32>) -> Vec<(f32, Para)> {
    let mut out = vec![];
    let mut start = 0;
    let typ = |l: &TextLine| typical.get(&((l.size * 2.0).round() as i32)).copied().unwrap_or(l.size * 1.15);
    let mut prev_last_base: Option<f32> = None;
    let mut i = 1;
    while i <= lines.len() {
        let done = i == lines.len() || {
            let count = i - start;
            let wrap = (count >= 2).then(|| lines[start + 1].x0);
            breaks(&lines[i - 1], &lines[i], region, typ(&lines[i - 1]), count, wrap, true)
        };
        if done {
            let group = &lines[start..i];
            let t = typ(&group[0]);
            let above = prev_last_base.map_or(0.0, |b| group[0].base - b - t);
            out.push((group[0].top(), para_from(group, region, t, above)));
            prev_last_base = Some(group[group.len() - 1].base);
            start = i;
        }
        i += 1;
    }
    out
}

fn picture_para(pic: &Picture, region: &Region, max_height: f32, space_above: f32) -> Para {
    let (mut w, mut h) = (pic.x1 - pic.x0, pic.y1 - pic.y0);
    let width = (region.right - region.left).max(10.0);
    let scale = (width / w).min(max_height / h).min(1.0);
    w *= scale;
    h *= scale;
    let center = (pic.x0 + pic.x1) / 2.0;
    let mid = (region.left + region.right) / 2.0;
    let mut props = ParaProps { before: Some(space_above.max(0.0)), after: Some(0.0), line: Some(Spacing::Auto(1.0)), ..Default::default() };
    if (center - mid).abs() < width * 0.08 {
        props.align = Some(1);
    } else if pic.x1 > region.right - 3.0 && pic.x0 > mid {
        props.align = Some(2);
    } else {
        props.align = Some(0);
        props.left = Some((pic.x0 - region.left).clamp(0.0, (width - w).max(0.0)));
    }
    let mark = RunProps { size: Some(2.0), ..Default::default() };
    Para { props, mark: mark.clone(), pieces: vec![Piece::Image { data: pic.data.clone(), width: w, height: h, props: mark }], ..Default::default() }
}

// MARK: - Header, footer, page numbers

fn normalized(text: &str) -> String {
    let mut out = String::new();
    let mut digit = false;
    for c in text.chars() {
        if c.is_ascii_digit() {
            if !digit {
                out.push('#');
            }
            digit = true;
        } else {
            digit = false;
            if !c.is_whitespace() {
                out.push(c);
            }
        }
    }
    out
}

/// Digit runs in a line, as (start char index, end, value).
fn digit_runs(text: &str) -> Vec<(usize, usize, i64)> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = vec![];
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let s = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            let v: String = chars[s..i].iter().collect();
            if let Ok(v) = v.parse() {
                out.push((s, i, v));
            }
        } else {
            i += 1;
        }
    }
    out
}

struct Repeats {
    /// Per page: indexes of lines that repeat (header/footer).
    lines: Vec<Vec<usize>>,
    /// The page-number line: (page, line index) per page.
    number: Option<(String, Vec<Option<usize>>)>,
}

fn repeats(pages: &[Page], lines: &[Vec<TextLine>]) -> Repeats {
    let count = pages.len();
    let mut rep = Repeats { lines: vec![vec![]; count], number: None };
    let zone = |p: &Page, l: &TextLine| l.base < p.height * 0.16 || l.top() > p.height * 0.84;
    // Page numbers: a line in the top or bottom zone holding the page's number.
    let mut number_lines: Vec<Option<usize>> = vec![None; count];
    for (pi, page) in pages.iter().enumerate() {
        for (li, l) in lines[pi].iter().enumerate() {
            let text = l.text();
            if !zone(page, l) || text.chars().count() > 40 {
                continue;
            }
            let runs = digit_runs(&text);
            if runs.iter().any(|r| r.2 == pi as i64 + 1) && runs.len() <= 2 && text.chars().filter(|c| c.is_alphabetic()).count() <= 12 {
                number_lines[pi] = Some(li);
                break;
            }
        }
    }
    let found = number_lines.iter().filter(|x| x.is_some()).count();
    if found > 0 && (found * 2 >= count || count == 1) {
        let (pi, li) = number_lines.iter().enumerate().find_map(|(p, l)| l.map(|l| (p, l))).unwrap();
        let line = &lines[pi][li];
        let text = line.text();
        let chars: Vec<char> = text.chars().collect();
        let runs = digit_runs(&text);
        let page_run = runs.iter().find(|r| r.2 == pi as i64 + 1).copied().unwrap();
        let total_run = runs.iter().find(|r| r.0 > page_run.1 && r.2 == count as i64).copied();
        let fore: String = chars[..page_run.0].iter().collect();
        let (sep, after, total) = match total_run {
            Some(t) => (chars[page_run.1..t.0].iter().collect::<String>(), chars[t.1..].iter().collect::<String>(), true),
            None => ("/".to_string(), chars[page_run.1..].iter().collect::<String>(), false),
        };
        let page = &pages[pi];
        let center = (line.x0 + line.x1) / 2.0;
        let align = if (center - page.width / 2.0).abs() < 25.0 { 1 } else if center > page.width / 2.0 { 2 } else { 0 };
        let font = line.items.iter().find_map(|i| if let Item::Glyph(_, p) = i { Some(p.clone()) } else { None }).unwrap_or_default();
        let attrs = udfout::page_number_attrs(&fore, &sep, &after, total, align, &font, false);
        let side = if line.base < page.height / 2.0 { "header" } else { "footer" };
        rep.number = Some((format!("{side}|{attrs}"), number_lines));
    }
    // Repeated text: the same line (numbers aside) at the same height on most pages.
    if count >= 2 {
        for (pi, page) in pages.iter().enumerate() {
            for (li, l) in lines[pi].iter().enumerate() {
                if !zone(page, l) || rep.number.as_ref().is_some_and(|(_, n)| n[pi] == Some(li)) {
                    continue;
                }
                let key = normalized(&l.text());
                let same = (0..count).filter(|&q| lines[q].iter().any(|m| (m.base - l.base).abs() < 4.0 && normalized(&m.text()) == key)).count();
                if same * 10 >= count * 6 && same >= 2 {
                    rep.lines[pi].push(li);
                }
            }
        }
    }
    rep
}

/// The same picture at the same place on most pages: a letterhead or a header logo.
fn repeated_pictures(pages: &[Page]) -> Vec<Vec<usize>> {
    let count = pages.len();
    let mut out = vec![vec![]; count];
    if count < 2 {
        return out;
    }
    for (pi, page) in pages.iter().enumerate() {
        for (k, p) in page.pictures.iter().enumerate() {
            let same = pages.iter().filter(|q| q.pictures.iter().any(|o| (o.x0 - p.x0).abs() < 3.0 && (o.y0 - p.y0).abs() < 3.0 && o.data.len() == p.data.len())).count();
            if same * 10 >= count * 6 {
                out[pi].push(k);
            }
        }
    }
    out
}

// MARK: - Document

enum Element {
    Lines(Vec<TextLine>),
    Grid(usize),
    Picture(usize),
}

fn most_common<T: std::hash::Hash + Eq + Clone>(items: impl Iterator<Item = T>) -> Option<T> {
    let mut counts: HashMap<T, usize> = HashMap::new();
    for i in items {
        *counts.entry(i).or_default() += 1;
    }
    counts.into_iter().max_by_key(|(_, n)| *n).map(|(t, _)| t)
}

fn percentile(mut v: Vec<f32>, q: f32) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    Some(v[((v.len() - 1) as f32 * q).round() as usize])
}

/// Builds the UDF document from the pages.
fn build(mut pages: Vec<Page>) -> Document {
    let count = pages.len();
    let mut page_lines: Vec<Vec<TextLine>> = vec![];
    for page in pages.iter_mut() {
        let mut ls = lines(page);
        decorate(&mut ls, &mut page.rules);
        page_lines.push(ls);
    }
    let typical = pitches(&page_lines);
    let rep = repeats(&pages, &page_lines);
    let rep_pics = repeated_pictures(&pages);
    let (pw, ph) = (pages[0].width, pages[0].height);

    // Header, footer and page number from the first page holding them.
    let mut header_lines: Vec<TextLine> = vec![];
    let mut footer_lines: Vec<TextLine> = vec![];
    if let Some(pi) = (0..count).find(|&p| !rep.lines[p].is_empty()) {
        for &li in &rep.lines[pi] {
            let l = page_lines[pi][li].clone();
            if l.base < pages[pi].height / 2.0 { header_lines.push(l) } else { footer_lines.push(l) }
        }
    }
    let mut header_number = None;
    let mut footer_number = None;
    if let Some((spec, _)) = &rep.number {
        let (side, attrs) = spec.split_once('|').unwrap();
        if side == "header" { header_number = Some(attrs.to_string()) } else { footer_number = Some(attrs.to_string()) }
    }
    let mut background = None;
    let mut header_pics: Vec<Picture> = vec![];
    let mut footer_pics: Vec<Picture> = vec![];
    if let Some(pi) = (0..count).find(|&p| !rep_pics[p].is_empty()) {
        for &k in &rep_pics[pi] {
            let p = pages[pi].pictures[k].clone();
            if p.x1 - p.x0 > pw * 0.6 && p.y1 - p.y0 > ph * 0.5 {
                background = Some(Background { rect: [p.x0, p.y0, p.x1 - p.x0, p.y1 - p.y0], data: p.data });
            } else if p.y0 < ph / 2.0 {
                header_pics.push(p);
            } else {
                footer_pics.push(p);
            }
        }
    }

    // Body elements per page, with repeated parts removed.
    let mut bodies: Vec<Vec<TextLine>> = vec![];
    for pi in 0..count {
        let skip_number = rep.number.as_ref().and_then(|(_, n)| n[pi]);
        let lines: Vec<TextLine> = page_lines[pi].iter().enumerate().filter(|(li, _)| !rep.lines[pi].contains(li) && Some(*li) != skip_number).map(|(_, l)| l.clone()).collect();
        bodies.push(lines);
    }
    let text_pages: Vec<usize> = (0..count).filter(|&p| bodies[p].iter().any(|l| l.text().trim().chars().count() > 0)).collect();

    // Footnotes: smaller text under a short line near the bottom of the page. UDF has no
    // footnotes, so they move to the end of the document (the marks stay in the text).
    let body_size = most_common(bodies.iter().flatten().map(|l| (l.size * 2.0).round() as i32)).map(|s| s as f32 / 2.0).unwrap_or(12.0);
    let mut notes: Vec<TextLine> = vec![];
    for pi in 0..count {
        let page = &pages[pi];
        let min_x = bodies[pi].iter().map(|l| l.x0).fold(f32::MAX, f32::min);
        let separator = page
            .rules
            .iter()
            .filter(|r| r.y1 - r.y0 < 2.0 && r.y0 > page.height * 0.45 && r.x1 - r.x0 > 25.0 && r.x1 - r.x0 < page.width * 0.4 && (r.x0 - min_x).abs() < 8.0)
            .map(|r| r.y0)
            .fold(f32::MAX, f32::min);
        if separator == f32::MAX {
            continue;
        }
        let below: Vec<usize> = bodies[pi].iter().enumerate().filter(|(_, l)| l.top() > separator - 2.0).map(|(i, _)| i).collect();
        if !below.is_empty() && below.iter().all(|&i| bodies[pi][i].size < body_size - 0.5) {
            let mut taken: Vec<TextLine> = below.iter().rev().map(|&i| bodies[pi].remove(i)).collect();
            taken.reverse();
            notes.extend(taken);
        }
    }

    // Margins from the body text.
    let all: Vec<&TextLine> = text_pages.iter().flat_map(|&p| bodies[p].iter()).collect();
    let left = percentile(all.iter().map(|l| l.x0).collect(), 0.02).unwrap_or(56.7).clamp(14.0, pw * 0.4);
    let right_edge = percentile(all.iter().map(|l| l.x1).collect(), 0.98).unwrap_or(pw - 56.7);
    let right = (pw - right_edge).clamp(14.0, pw * 0.4);
    let top = percentile(all.iter().map(|l| l.top()).collect(), 0.0).unwrap_or(56.7).clamp(14.0, ph * 0.4);
    // A short last page says nothing about the bottom margin; it is rarely larger than the top one.
    let bottom = (ph - percentile(all.iter().map(|l| l.bottom()).collect(), 1.0).unwrap_or(ph - 56.7)).clamp(14.0, ph * 0.4).min(top.max(42.5));
    let region = Region { left, right: pw - right };
    // Pages of a size the Editör does not know become A4: pictures must fit that page.
    let (_, mw, mh) = udfout::media(pw, ph);
    let (uw, uh) = if pw > ph { (mh, mw) } else { (mw, mh) };
    let picture_region = Region { left, right: (pw - right).min(uw - right) };
    let picture_height = (ph.min(uh) - top - bottom - 4.0).max(40.0);

    let mut body: Vec<Block> = vec![];
    // The previous page: where its body ended, and whether its last line ran to the margin.
    let mut prev_end: Option<f32> = None;
    let mut prev_full = false;
    for pi in 0..count {
        let page = &pages[pi];
        let grids = grids(&page.rules, &bodies[pi]);
        let first_block = body.len();
        // Scanned page: no text, a picture covering most of it.
        let cover = page.pictures.iter().map(|p| (p.x1 - p.x0) * (p.y1 - p.y0)).fold(0.0, f32::max);
        if !text_pages.contains(&pi) && cover > page.width * page.height * 0.4 {
            let pic = page.pictures.iter().max_by(|a, b| ((a.x1 - a.x0) * (a.y1 - a.y0)).total_cmp(&((b.x1 - b.x0) * (b.y1 - b.y0)))).unwrap();
            let mut p = picture_para(pic, &picture_region, picture_height, 0.0);
            p.props.page_break = Some(!body.is_empty());
            body.push(Block::Para(p));
            prev_end = None;
            prev_full = false;
            continue;
        }
        let mut elements: Vec<(f32, Element)> = vec![];
        let mut loose: Vec<TextLine> = vec![];
        let mut cells: Vec<Vec<Vec<Vec<TextLine>>>> = grids.iter().map(|g| g.cells.iter().map(|r| vec![vec![]; r.len()]).collect()).collect();
        let in_grid = |l: &TextLine| grids.iter().any(|g| {
            let (x0, y0, x1, y1) = g.bbox();
            let cy = (l.top() + l.bottom()) / 2.0;
            cy > y0 && cy < y1 && l.x1 > x0 && l.x0 < x1
        });
        // Inside a table, a tab jump usually means the next cell.
        let page_body: Vec<TextLine> = bodies[pi].iter().flat_map(|l| if in_grid(l) { split_at_tabs(l) } else { vec![l.clone()] }).collect();
        for l in &page_body {
            let (cx, cy) = ((l.x0 + l.x1) / 2.0, (l.top() + l.bottom()) / 2.0);
            let hit = grids.iter().enumerate().find_map(|(gi, g)| {
                let (x0, y0, x1, y1) = g.bbox();
                if cx < x0 || cx > x1 || cy < y0 || cy > y1 {
                    return None;
                }
                let r = g.ys.windows(2).position(|w| cy >= w[0] && cy <= w[1])?;
                let c = g.cells[r].iter().position(|(s, n)| l.x0 + 1.0 >= g.xs[*s] - 2.0 && l.x0 < g.xs[s + n])?;
                Some((gi, r, c))
            });
            match hit {
                Some((gi, r, c)) => cells[gi][r][c].push(l.clone()),
                None => loose.push(l.clone()),
            }
        }
        // Loose lines are grouped into runs between other elements later; first collect them by y.
        for l in loose {
            elements.push((l.top(), Element::Lines(vec![l])));
        }
        for gi in 0..grids.len() {
            elements.push((grids[gi].ys[0], Element::Grid(gi)));
        }
        for (k, pic) in page.pictures.iter().enumerate() {
            if !rep_pics[pi].contains(&k) {
                elements.push((pic.y0, Element::Picture(k)));
            }
        }
        elements.sort_by(|a, b| a.0.total_cmp(&b.0));
        // Merge consecutive line elements.
        let mut merged: Vec<(f32, Element)> = vec![];
        for (y, e) in elements {
            match (merged.last_mut(), e) {
                (Some((_, Element::Lines(ls))), Element::Lines(mut more)) => ls.append(&mut more),
                (_, e) => merged.push((y, e)),
            }
        }
        // The page broke early (or this is a new section): start a new page in the UDF too.
        let page_breaks = pi > 0 && prev_end.is_some_and(|b| b < ph - bottom - 3.0 * typical.values().copied().fold(14.0, f32::max));
        let mut last_bottom: Option<f32> = None;
        for (y, e) in merged {
            let gap = last_bottom.map_or(0.0, |b| y - b);
            match e {
                Element::Lines(ls) => {
                    let mut paras = paragraphs(&ls, &region, &typical);
                    // Continue the previous page's paragraph when the text simply flowed over.
                    if pi > 0 && body.len() == first_block && !page_breaks {
                        if let (Some(Block::Para(prev)), Some((_, first))) = (body.last_mut(), paras.first()) {
                            let prev_line_full = prev_full;
                            let starts_lower = first.pieces.iter().find_map(|p| if let Piece::Text(t, _) = p { t.chars().find(|c| !c.is_whitespace()) } else { None }).is_some_and(char::is_lowercase);
                            // Or the previous page stopped mid-sentence.
                            let open_sentence = prev.pieces.iter().rev().find_map(|p| if let Piece::Text(t, _) = p { t.trim_end().chars().last() } else { None }).is_some_and(|c| !".:;!?".contains(c));
                            if prev_line_full && (starts_lower || open_sentence) && prev.list.is_none() && first.props.first.unwrap_or(0.0).abs() < 1.0 {
                                let (_, first) = paras.remove(0);
                                prev.pieces.push(Piece::Text(" ".into(), prev.mark.clone()));
                                prev.pieces.extend(first.pieces);
                            }
                        }
                    }
                    for (k, (_, mut p)) in paras.into_iter().enumerate() {
                        if k == 0 && last_bottom.is_some() {
                            p.props.before = Some(gap.max(0.0));
                        }
                        body.push(Block::Para(p));
                    }
                    last_bottom = ls.last().map(|l| l.bottom());
                }
                Element::Grid(gi) => {
                    let g = &grids[gi];
                    let rows = g
                        .cells
                        .iter()
                        .enumerate()
                        .map(|(r, row)| {
                            row.iter()
                                .enumerate()
                                .map(|(c, (s, n))| {
                                    let region = Region { left: g.xs[*s] + 2.0, right: g.xs[s + n] - 2.0 };
                                    let blocks = paragraphs(&cells[gi][r][c], &region, &typical).into_iter().map(|(_, p)| Block::Para(p)).collect();
                                    Cell { span: *n, blocks }
                                })
                                .collect()
                        })
                        .collect();
                    let grid: Vec<f32> = g.xs.windows(2).map(|w| w[1] - w[0]).collect();
                    let mut spacer = Para { props: ParaProps { before: Some(gap.max(0.0) * 0.5), after: Some(0.0), line: Some(Spacing::Auto(0.8)), ..Default::default() }, mark: RunProps { size: Some(2.0), ..Default::default() }, ..Default::default() };
                    if gap > 4.0 {
                        spacer.props.before = Some((gap - 4.0).max(0.0));
                        body.push(Block::Para(spacer));
                    }
                    body.push(Block::Table(Table { grid, indent: g.xs[0] - region.left, border: g.border, rows }));
                    last_bottom = Some(*g.ys.last().unwrap());
                }
                Element::Picture(k) => {
                    let pic = &page.pictures[k];
                    body.push(Block::Para(picture_para(pic, &picture_region, picture_height, gap)));
                    last_bottom = Some(pic.y1);
                }
            }
        }
        if page_breaks && body.len() > first_block {
            match &mut body[first_block] {
                Block::Para(p) => p.props.page_break = Some(true),
                Block::Table(_) => body.insert(first_block, Block::Para(Para { props: ParaProps { page_break: Some(true), ..Default::default() }, mark: RunProps { size: Some(2.0), ..Default::default() }, ..Default::default() })),
            }
        }
        // A full last line lets the paragraph continue on the next page.
        let last = bodies[pi].last();
        prev_full = last.is_some_and(|l| region.right - l.x1 < 3.0);
        prev_end = last_bottom.or(last.map(|l| l.bottom()));
    }

    if !notes.is_empty() {
        let mut mark = RunProps { size: Some(body_size * 0.75), ..Default::default() };
        mark.font = Some("Times New Roman".into());
        body.push(Block::Para(Para { props: ParaProps { before: Some(12.0), ..Default::default() }, mark: mark.clone(), pieces: vec![Piece::Text("______________________".into(), mark)], ..Default::default() }));
        body.extend(paragraphs(&notes, &region, &typical).into_iter().map(|(_, p)| Block::Para(p)));
    }

    let header_region = Region { left, right: pw - right };
    let mut header: Vec<Block> = header_pics.iter().map(|p| Block::Para(picture_para(p, &header_region, ph * 0.3, 0.0))).collect();
    header.extend(paragraphs(&header_lines, &header_region, &typical).into_iter().map(|(_, p)| Block::Para(p)));
    let mut footer: Vec<Block> = paragraphs(&footer_lines, &header_region, &typical).into_iter().map(|(_, p)| Block::Para(p)).collect();
    footer.extend(footer_pics.iter().map(|p| Block::Para(picture_para(p, &header_region, ph * 0.3, 0.0))));
    let header_offset = header_lines.iter().map(|l| l.top()).chain(header_pics.iter().map(|p| p.y0)).fold(f32::MAX, f32::min);
    let footer_offset = footer_lines.iter().map(|l| ph - l.bottom()).chain(footer_pics.iter().map(|p| ph - p.y1)).fold(f32::MAX, f32::min);
    let number_y = rep.number.as_ref().and_then(|(_, n)| n.iter().enumerate().find_map(|(p, l)| l.map(|l| &page_lines[p][l])));
    let number_offset = number_y.map(|l| if l.base < ph / 2.0 { l.top() } else { ph - l.bottom() });

    let font = most_common(all.iter().flat_map(|l| l.items.iter()).filter_map(|i| if let Item::Glyph(g, _) = i { Some(g.font.family.clone()) } else { None })).unwrap_or_else(|| "Times New Roman".into());
    let size = most_common(all.iter().map(|l| (l.size * 2.0).round() as i32)).map(|s| s as f32 / 2.0).unwrap_or(12.0);
    Document {
        page: [pw, ph],
        margins: [top, right, bottom, left],
        header_offset: if header_offset < f32::MAX { header_offset } else { number_offset.filter(|_| header_number.is_some()).unwrap_or(20.0) },
        footer_offset: if footer_offset < f32::MAX { footer_offset } else { number_offset.filter(|_| footer_number.is_some()).unwrap_or(20.0) },
        header,
        footer,
        body,
        header_number,
        footer_number,
        background,
        font,
        size,
        default_tab: 36.0,
    }
}

/// Converts PDF bytes to a UDF `content.xml`, keeping the text editable.
pub fn pdf_to_content_xml(bytes: &[u8]) -> Result<String, String> {
    let pages = read::read(bytes)?;
    Ok(udfout::write(&build(pages)))
}

/// One picture per page (rendered by the app), for an exact copy that is not editable.
pub fn pages_to_content_xml(pages: &[(Vec<u8>, f32, f32)]) -> String {
    let (pw, ph) = pages.first().map(|p| (p.1, p.2)).unwrap_or((595.28, 841.89));
    let margin = 14.17;
    let region = Region { left: margin, right: pw - margin };
    let body = pages
        .iter()
        .enumerate()
        .map(|(i, (data, w, h))| {
            let pic = Picture { data: data.clone(), x0: margin, y0: margin, x1: margin + w, y1: margin + h };
            let mut p = picture_para(&pic, &region, ph - 2.0 * margin - 6.0, 0.0);
            p.props.align = Some(1);
            p.props.left = None;
            p.props.page_break = Some(i > 0);
            Block::Para(p)
        })
        .collect();
    udfout::write(&Document {
        page: [pw, ph],
        margins: [margin; 4],
        header_offset: 10.0,
        footer_offset: 10.0,
        header: vec![],
        footer: vec![],
        body,
        header_number: None,
        footer_number: None,
        background: None,
        font: "Times New Roman".into(),
        size: 12.0,
        default_tab: 36.0,
    })
}

/// Reads a PDF file and writes it as an editable .udf file.
pub fn pdf_to_udf(input: &Path, output: &Path) -> Result<(), String> {
    let name = input.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let bytes = std::fs::read(input).map_err(|_| format!("\"{name}\" açılamadı."))?;
    let xml = pdf_to_content_xml(&bytes).map_err(|e| format!("\"{name}\" {e}"))?;
    std::fs::write(output, udfout::udf_bytes(&xml)?).map_err(|_| format!("\"{}\" kaydedilemedi.", output.display()))
}

/// Writes one picture per page as a .udf (exact look, not editable). `pages` are JPEG or PNG
/// renderings with the page size in points.
pub fn pages_to_udf(pages: &[(Vec<u8>, f32, f32)], output: &Path) -> Result<(), String> {
    std::fs::write(output, udfout::udf_bytes(&pages_to_content_xml(pages))?).map_err(|_| format!("\"{}\" kaydedilemedi.", output.display()))
}

/// What the reader sees on each page, line by line (for diagnosing conversions).
pub fn dump(bytes: &[u8]) -> Result<String, String> {
    use std::fmt::Write as _;
    let pages = read::read(bytes)?;
    let mut out = String::new();
    for (i, p) in pages.iter().enumerate() {
        writeln!(out, "== page {} {}x{} glyphs={} rules={} pictures={}", i + 1, p.width, p.height, p.glyphs.len(), p.rules.len(), p.pictures.len()).ok();
        for l in lines(p) {
            writeln!(out, "[{:.0}-{:.0} @{:.0} {:.1}] {}", l.x0, l.x1, l.base, l.size, l.text()).ok();
        }
    }
    Ok(out)
}

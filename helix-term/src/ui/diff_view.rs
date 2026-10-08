//! A read-only, side by side diff between the HEAD version of a file (left) and its current
//! contents (right). Changed lines are aligned, with filler rows where one side has no line.

use std::ops::Range;
use std::path::{Path, PathBuf};

use helix_core::syntax::{self, HighlightEvent};
use helix_core::unicode::width::UnicodeWidthChar;
use helix_core::{RopeSlice, Selection, Syntax};
use helix_view::editor::Action;
use helix_view::graphics::{Color, CursorKind, Modifier, Rect, Style};
use helix_view::input::{Event, KeyEvent, MouseEventKind};
use helix_view::keyboard::{KeyCode, KeyModifiers};
use helix_view::{align_view, current, Align, Editor, Theme};
use imara_diff::{Algorithm, Diff, InternedInput};
use tui::buffer::Buffer as Surface;

use crate::compositor::{Component, Compositor, Context, EventResult};

pub const ID: &str = "git-diff-view";

/// Unchanged lines kept visible above a hunk when jumping to it.
const HUNK_CONTEXT: usize = 3;

/// Word highlights are dropped when more than this fraction of a line pair changed: at that
/// point the whole line is different and highlighting every word is just noise.
const MAX_WORD_CHANGE_RATIO: f32 = 0.6;

/// One version of the file.
pub struct Side {
    title: String,
    text: String,
    /// Byte range of each line, without the line ending.
    lines: Vec<Range<usize>>,
    /// Syntax highlights of each line, as byte ranges relative to the start of the line.
    highlights: Vec<Vec<(Range<usize>, Style)>>,
}

impl Side {
    pub fn new(title: String, text: String, path: &Path, editor: &Editor) -> Side {
        let lines = line_ranges(&text);
        let highlights = highlight(
            &text,
            &lines,
            path,
            &editor.syn_loader.load(),
            &editor.theme,
        );
        Side {
            title,
            text,
            lines,
            highlights,
        }
    }

    fn line(&self, line: usize) -> &str {
        &self.text[self.lines[line].clone()]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowKind {
    Equal,
    /// A removed line paired with the added line that replaced it.
    Changed,
    Removed,
    Added,
}

#[derive(Debug, PartialEq, Eq)]
struct Row {
    kind: RowKind,
    left: Option<usize>,
    right: Option<usize>,
    /// Changed byte ranges within the lines of a `Changed` row.
    left_words: Vec<Range<usize>>,
    right_words: Vec<Range<usize>>,
}

impl Row {
    fn new(kind: RowKind, left: Option<usize>, right: Option<usize>) -> Row {
        Row {
            kind,
            left,
            right,
            left_words: Vec::new(),
            right_words: Vec::new(),
        }
    }
}

/// Splits `text` like imara-diff's line tokenizer does, returning each line without its ending.
fn line_ranges(text: &str) -> Vec<Range<usize>> {
    let mut start = 0;
    text.split_inclusive('\n')
        .map(|line| {
            let content = line.strip_suffix('\n').unwrap_or(line);
            let content = content.strip_suffix('\r').unwrap_or(content);
            let range = start..start + content.len();
            start += line.len();
            range
        })
        .collect()
}

/// Aligns the lines of `before` and `after`, returning the rows and the first row of each hunk.
fn build_rows(before: &str, after: &str) -> (Vec<Row>, Vec<usize>) {
    let before_lines = line_ranges(before);
    let after_lines = line_ranges(after);
    let input = InternedInput::new(before, after);
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    diff.postprocess_lines(&input);

    let mut rows = Vec::new();
    let mut hunks = Vec::new();
    let (mut b, mut a) = (0, 0);
    for hunk in diff.hunks() {
        let (before_start, before_end) = (hunk.before.start as usize, hunk.before.end as usize);
        let (after_start, after_end) = (hunk.after.start as usize, hunk.after.end as usize);
        while b < before_start && a < after_start {
            rows.push(Row::new(RowKind::Equal, Some(b), Some(a)));
            b += 1;
            a += 1;
        }

        hunks.push(rows.len());
        let removed = before_end - before_start;
        let added = after_end - after_start;
        let paired = removed.min(added);
        for i in 0..paired {
            let (left, right) = (before_start + i, after_start + i);
            let (left_words, right_words) = word_diff(
                &before[before_lines[left].clone()],
                &after[after_lines[right].clone()],
            );
            rows.push(Row {
                kind: RowKind::Changed,
                left: Some(left),
                right: Some(right),
                left_words,
                right_words,
            });
        }
        for line in before_start + paired..before_end {
            rows.push(Row::new(RowKind::Removed, Some(line), None));
        }
        for line in after_start + paired..after_end {
            rows.push(Row::new(RowKind::Added, None, Some(line)));
        }
        b = before_end;
        a = after_end;
    }
    while b < before_lines.len() || a < after_lines.len() {
        let left = (b < before_lines.len()).then_some(b);
        let right = (a < after_lines.len()).then_some(a);
        rows.push(Row::new(RowKind::Equal, left, right));
        b += 1;
        a += 1;
    }
    (rows, hunks)
}

/// Splits a line into words, runs of whitespace and single punctuation characters.
fn word_tokens(line: &str) -> Vec<Range<usize>> {
    #[derive(PartialEq)]
    enum Class {
        Word,
        Space,
        Other,
    }
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            Class::Word
        } else if c.is_whitespace() {
            Class::Space
        } else {
            Class::Other
        }
    };

    let mut tokens: Vec<Range<usize>> = Vec::new();
    let mut previous = None;
    for (i, c) in line.char_indices() {
        let current = class(c);
        let extends = current != Class::Other && previous.as_ref() == Some(&current);
        match tokens.last_mut() {
            Some(last) if extends => last.end = i + c.len_utf8(),
            _ => tokens.push(i..i + c.len_utf8()),
        }
        previous = Some(current);
    }
    tokens
}

/// Returns the changed byte ranges of `left` and `right`, compared word by word.
fn word_diff(left: &str, right: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let left_tokens = word_tokens(left);
    let right_tokens = word_tokens(right);
    let mut input = InternedInput::default();
    input.update_before(left_tokens.iter().map(|range| &left[range.clone()]));
    input.update_after(right_tokens.iter().map(|range| &right[range.clone()]));
    // the histogram heuristic does not work well for short, repetitive tokens
    let mut diff = Diff::compute(Algorithm::Myers, &input);
    diff.postprocess_no_heuristic(&input);

    let mut left_words = Vec::new();
    let mut right_words = Vec::new();
    for hunk in diff.hunks() {
        if !hunk.before.is_empty() {
            left_words.push(
                left_tokens[hunk.before.start as usize].start
                    ..left_tokens[hunk.before.end as usize - 1].end,
            );
        }
        if !hunk.after.is_empty() {
            right_words.push(
                right_tokens[hunk.after.start as usize].start
                    ..right_tokens[hunk.after.end as usize - 1].end,
            );
        }
    }

    let changed: usize = left_words.iter().chain(&right_words).map(Range::len).sum();
    let total = left.len() + right.len();
    if total == 0 || changed as f32 > total as f32 * MAX_WORD_CHANGE_RATIO {
        return (Vec::new(), Vec::new());
    }
    (left_words, right_words)
}

/// Syntax highlights each line of `text`, using the language of `path`.
fn highlight(
    text: &str,
    lines: &[Range<usize>],
    path: &Path,
    loader: &syntax::Loader,
    theme: &Theme,
) -> Vec<Vec<(Range<usize>, Style)>> {
    let mut result = vec![Vec::new(); lines.len()];
    let slice = RopeSlice::from(text);
    let Some(syntax) = loader
        .language_for_filename(path)
        .or_else(|| loader.language_for_shebang(slice))
        .and_then(|language| Syntax::new(slice, language, loader).ok())
    else {
        return result;
    };

    let mut highlighter = syntax.highlighter(slice, loader, ..);
    let mut stack = Vec::new();
    let len = text.len() as u32;
    let mut pos = 0;
    let mut line = 0;
    while pos < len {
        if pos == highlighter.next_event_offset() {
            let (event, highlights) = highlighter.advance();
            if event == HighlightEvent::Refresh {
                stack.clear();
            }
            stack.extend(highlights);
        }
        let start = pos;
        pos = highlighter.next_event_offset().min(len);
        if pos < start {
            log::error!(
                "diff view: highlighter moved backwards in {}",
                path.display()
            );
            break;
        }
        if pos == start || stack.is_empty() {
            continue;
        }

        let style = stack.iter().fold(Style::default(), |acc, highlight| {
            acc.patch(theme.highlight(*highlight))
        });
        let (start, end) = (start as usize, pos as usize);
        while line + 1 < lines.len() && lines[line + 1].start <= start {
            line += 1;
        }
        let mut current = line;
        while current < lines.len() && lines[current].start < end {
            let range = &lines[current];
            let overlap = start.max(range.start)..end.min(range.end);
            if !overlap.is_empty() {
                result[current].push((
                    overlap.start - range.start..overlap.end - range.start,
                    style,
                ));
            }
            current += 1;
        }
    }
    result
}

/// Mixes `color` over `background`, if both are true colors.
fn blend(background: Option<Color>, color: Option<Color>, alpha: f32) -> Option<Color> {
    match (background?, color?) {
        (Color::Rgb(br, bg, bb), Color::Rgb(r, g, b)) => {
            let mix = |back: u8, front: u8| {
                (back as f32 + (front as f32 - back as f32) * alpha).round() as u8
            };
            Some(Color::Rgb(mix(br, r), mix(bg, g), mix(bb, b)))
        }
        _ => None,
    }
}

struct Palette {
    background: Style,
    text: Style,
    line_number: Style,
    filler: Style,
    separator: Style,
    title: Style,
    hint: Style,
    /// Line, word and sign styles for removed lines (left) and added lines (right).
    minus: ChangeStyle,
    plus: ChangeStyle,
}

struct ChangeStyle {
    line: Style,
    word: Style,
    sign: Style,
}

impl ChangeStyle {
    fn new(theme: &Theme, scope: &str, background: Option<Color>) -> ChangeStyle {
        let color = theme.get(scope).fg;
        let sign = Style::default().fg(color.unwrap_or(Color::Reset));
        match (
            blend(background, color, 0.15),
            blend(background, color, 0.4),
        ) {
            (Some(line), Some(word)) => ChangeStyle {
                line: Style::default().bg(line),
                word: Style::default().bg(word),
                sign,
            },
            // without true colors there is nothing to blend: color the text instead
            _ => ChangeStyle {
                line: Style::default(),
                word: sign.add_modifier(Modifier::REVERSED),
                sign,
            },
        }
    }
}

impl Palette {
    fn new(theme: &Theme) -> Palette {
        let background = theme.get("ui.background");
        Palette {
            background,
            text: theme.get("ui.text"),
            line_number: theme.get("ui.linenr"),
            filler: theme.get("ui.virtual.whitespace"),
            separator: theme.get("ui.window"),
            title: theme.get("ui.statusline").add_modifier(Modifier::BOLD),
            hint: theme.get("ui.statusline.inactive"),
            minus: ChangeStyle::new(theme, "diff.minus", background.bg),
            plus: ChangeStyle::new(theme, "diff.plus", background.bg),
        }
    }
}

type OnOpen = Box<dyn FnOnce(&mut Compositor)>;

pub struct DiffView {
    left: Side,
    right: Side,
    rows: Vec<Row>,
    hunks: Vec<usize>,
    /// File to open with Enter, if it still exists.
    path: Option<PathBuf>,
    /// Run when Enter opens the file, to close what the view was opened from.
    on_open: Option<OnOpen>,
    tab_width: usize,
    scroll: usize,
    hscroll: usize,
    /// Rows of the diff that fit on screen, from the last render.
    height: usize,
    /// First key of a two key sequence (`]c` / `[c`).
    pending: Option<char>,
    /// Index of the current hunk in `hunks`. Tracked explicitly because several hunks can map
    /// to the same scroll position (near the top or bottom, or when everything fits on screen).
    current: usize,
}

impl DiffView {
    pub fn new(left: Side, right: Side, path: Option<PathBuf>, tab_width: usize) -> DiffView {
        let (rows, hunks) = build_rows(&left.text, &right.text);
        let scroll = hunks
            .first()
            .map_or(0, |hunk| hunk.saturating_sub(HUNK_CONTEXT));
        DiffView {
            left,
            right,
            rows,
            hunks,
            path,
            on_open: None,
            tab_width: tab_width.max(1),
            scroll,
            hscroll: 0,
            height: 0,
            pending: None,
            current: 0,
        }
    }

    /// Runs `on_open` on the compositor when Enter opens the file.
    pub fn with_on_open(mut self, on_open: impl FnOnce(&mut Compositor) + 'static) -> DiffView {
        self.on_open = Some(Box::new(on_open));
        self
    }

    /// Whether the two sides differ at all.
    pub fn has_changes(&self) -> bool {
        !self.hunks.is_empty()
    }

    fn max_scroll(&self) -> usize {
        self.rows.len().saturating_sub(self.height.max(1))
    }

    fn scroll_by(&mut self, delta: isize) {
        self.scroll_to(self.scroll.saturating_add_signed(delta));
    }

    /// Scrolls without changing hunk, unless the current hunk went out of view: then the first
    /// hunk in view (or the closest one) becomes the current one.
    fn scroll_to(&mut self, scroll: usize) {
        self.scroll = scroll.min(self.max_scroll());
        let in_view = |hunk: usize| hunk >= self.scroll && hunk < self.scroll + self.height;
        if self
            .hunks
            .get(self.current)
            .is_some_and(|&hunk| in_view(hunk))
        {
            return;
        }
        self.current = match self.hunks.iter().position(|&hunk| hunk >= self.scroll) {
            Some(index) => index,
            None => self.hunks.len().saturating_sub(1),
        };
    }

    fn jump_to_hunk(&mut self, next: bool) {
        self.current = if next {
            (self.current + 1).min(self.hunks.len().saturating_sub(1))
        } else {
            self.current.saturating_sub(1)
        };
        if let Some(&hunk) = self.hunks.get(self.current) {
            self.scroll = hunk.saturating_sub(HUNK_CONTEXT).min(self.max_scroll());
        }
    }

    /// Rows of the current hunk.
    fn current_rows(&self) -> Range<usize> {
        let Some(&start) = self.hunks.get(self.current) else {
            return 0..0;
        };
        let len = self.rows[start..]
            .iter()
            .take_while(|row| row.kind != RowKind::Equal)
            .count();
        start..start + len
    }

    /// The current file line of the current hunk, or the closest one below it.
    fn current_right_line(&self) -> Option<usize> {
        self.rows[self.current_rows().start..]
            .iter()
            .find_map(|row| row.right)
            .or_else(|| self.rows.iter().rev().find_map(|row| row.right))
    }

    fn close() -> EventResult {
        EventResult::Consumed(Some(Box::new(|compositor: &mut Compositor, _| {
            compositor.remove(ID);
        })))
    }

    /// Closes the view and opens the file at the line shown at the top.
    fn open_file(&mut self) -> EventResult {
        let Some(path) = self.path.clone() else {
            return Self::close();
        };
        let line = self.current_right_line().unwrap_or(0);
        let on_open = self.on_open.take();
        EventResult::Consumed(Some(Box::new(move |compositor: &mut Compositor, cx| {
            compositor.remove(ID);
            if let Some(on_open) = on_open {
                on_open(compositor);
            }
            if let Err(err) = cx.editor.open(&path, Action::Replace) {
                cx.editor.set_error(format!("{err}"));
                return;
            }
            let (view, doc) = current!(cx.editor);
            let text = doc.text();
            let pos = text.line_to_char(line.min(text.len_lines().saturating_sub(1)));
            doc.set_selection(view.id, Selection::point(pos));
            align_view(doc, view, Align::Center);
        })))
    }

    fn handle_key(&mut self, key: &KeyEvent) -> EventResult {
        let half_page = (self.height / 2).max(1) as isize;
        let page = self.height.max(1) as isize;

        if let Some(prefix) = self.pending.take() {
            if key.code == KeyCode::Char('c') {
                self.jump_to_hunk(prefix == ']');
            }
            return EventResult::Consumed(None);
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Self::close(),
            KeyCode::Char('c') if ctrl => return Self::close(),
            KeyCode::Enter => return self.open_file(),
            KeyCode::Char('d') if ctrl => self.scroll_by(half_page),
            KeyCode::Char('u') if ctrl => self.scroll_by(-half_page),
            KeyCode::Char('f') if ctrl => self.scroll_by(page),
            KeyCode::Char('b') if ctrl => self.scroll_by(-page),
            KeyCode::Char('j') | KeyCode::Down => self.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_by(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll_by(page),
            KeyCode::PageUp => self.scroll_by(-page),
            KeyCode::Char('g') | KeyCode::Home => self.scroll_to(0),
            KeyCode::Char('G') | KeyCode::End => self.scroll_to(usize::MAX),
            KeyCode::Char('n') => self.jump_to_hunk(true),
            KeyCode::Char('N') => self.jump_to_hunk(false),
            KeyCode::Char(c @ (']' | '[')) => self.pending = Some(c),
            KeyCode::Char('h') | KeyCode::Left => self.hscroll = self.hscroll.saturating_sub(4),
            KeyCode::Char('l') | KeyCode::Right => self.hscroll += 4,
            KeyCode::Char('0') => self.hscroll = 0,
            _ => {}
        }
        EventResult::Consumed(None)
    }

    #[allow(clippy::too_many_arguments)]
    fn render_cell(
        &self,
        surface: &mut Surface,
        area: Rect,
        y: u16,
        side: &Side,
        line: Option<usize>,
        kind: RowKind,
        words: &[Range<usize>],
        is_left: bool,
        is_current: bool,
        number_width: usize,
        palette: &Palette,
    ) {
        let Some(line) = line else {
            let filler = "╱".repeat(area.width as usize);
            surface.set_stringn(area.x, y, &filler, area.width as usize, palette.filler);
            return;
        };

        let change = match (kind, is_left) {
            (RowKind::Equal, _) => None,
            (_, true) => Some(&palette.minus),
            (_, false) => Some(&palette.plus),
        };
        let line_style = change.map_or(Style::default(), |change| change.line);
        surface.set_style(Rect::new(area.x, y, area.width, 1), line_style);

        let sign = match change {
            None => ' ',
            Some(_) if is_left => '-',
            Some(_) => '+',
        };
        let gutter = format!("{:>number_width$} ", line + 1);
        surface.set_stringn(
            area.x,
            y,
            &gutter,
            area.width as usize,
            match change {
                Some(change) if is_current => change.sign.add_modifier(Modifier::BOLD),
                _ => palette.line_number,
            }
            .patch(line_style),
        );
        let sign_x = area.x + gutter.len() as u16;
        if sign_x < area.right() {
            let sign_style = change.map_or(palette.line_number, |change| change.sign);
            surface.set_stringn(
                sign_x,
                y,
                &sign.to_string(),
                1,
                sign_style.patch(line_style),
            );
        }

        let text_x = sign_x + 2;
        if text_x >= area.right() {
            return;
        }
        let text_width = (area.right() - text_x) as usize;
        let base = palette.text.patch(line_style);
        let word_style = change.map_or(Style::default(), |change| change.word);
        let highlights = &side.highlights[line];
        let mut next_highlight = 0;
        let mut col = 0;
        for (byte, c) in side.line(line).char_indices() {
            while next_highlight < highlights.len() && highlights[next_highlight].0.end <= byte {
                next_highlight += 1;
            }
            let mut style = base;
            if let Some((range, highlight)) = highlights.get(next_highlight) {
                if range.contains(&byte) {
                    style = style.patch(*highlight);
                }
            }
            if words.iter().any(|range| range.contains(&byte)) {
                style = style.patch(word_style);
            }

            let (symbol, width) = if c == '\t' {
                let width = self.tab_width - col % self.tab_width;
                (" ".repeat(width), width)
            } else {
                (c.to_string(), c.width().unwrap_or(0))
            };
            if width == 0 {
                continue;
            }
            // only draw characters that are fully inside the horizontally scrolled window
            if col >= self.hscroll && col + width <= self.hscroll + text_width {
                let x = text_x + (col - self.hscroll) as u16;
                surface.set_stringn(x, y, &symbol, width, style);
            }
            col += width;
            if col >= self.hscroll + text_width {
                break;
            }
        }
    }
}

impl Component for DiffView {
    fn handle_event(&mut self, event: &Event, _cx: &mut Context) -> EventResult {
        match event {
            Event::Key(key) => self.handle_key(key),
            Event::Mouse(mouse) => {
                match mouse.kind {
                    MouseEventKind::ScrollDown => self.scroll_by(3),
                    MouseEventKind::ScrollUp => self.scroll_by(-3),
                    _ => {}
                }
                EventResult::Consumed(None)
            }
            Event::Paste(_) => EventResult::Consumed(None),
            Event::Resize(..) | Event::IdleTimeout | Event::FocusGained | Event::FocusLost => {
                EventResult::Ignored(None)
            }
        }
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        let palette = Palette::new(&cx.editor.theme);
        surface.clear_with(area, palette.background);
        if area.height < 3 || area.width < 10 {
            return;
        }

        let body = Rect::new(area.x, area.y + 1, area.width, area.height - 2);
        self.height = body.height as usize;
        self.scroll = self.scroll.min(self.max_scroll());

        let half = (area.width - 1) / 2;
        let left_area = Rect::new(area.x, body.y, half, body.height);
        let separator_x = area.x + half;
        let right_area = Rect::new(separator_x + 1, body.y, area.width - half - 1, body.height);
        let number_width = self
            .left
            .lines
            .len()
            .max(self.right.lines.len())
            .max(1)
            .to_string()
            .len();

        // header with the title of each side
        surface.set_style(Rect::new(area.x, area.y, area.width, 1), palette.title);
        surface.set_stringn(
            area.x + 1,
            area.y,
            &self.left.title,
            half.saturating_sub(1) as usize,
            palette.title,
        );
        surface.set_stringn(
            right_area.x + 1,
            area.y,
            &self.right.title,
            right_area.width.saturating_sub(1) as usize,
            palette.title,
        );

        let current_rows = self.current_rows();
        for (i, row) in self.rows[self.scroll..]
            .iter()
            .take(self.height)
            .enumerate()
        {
            let y = body.y + i as u16;
            let is_current = current_rows.contains(&(self.scroll + i));
            self.render_cell(
                surface,
                left_area,
                y,
                &self.left,
                row.left,
                row.kind,
                &row.left_words,
                true,
                is_current,
                number_width,
                &palette,
            );
            self.render_cell(
                surface,
                right_area,
                y,
                &self.right,
                row.right,
                row.kind,
                &row.right_words,
                false,
                is_current,
                number_width,
                &palette,
            );
        }
        for y in body.y..body.bottom() {
            surface.set_stringn(separator_x, y, "│", 1, palette.separator);
        }

        // footer with the position and the keys
        let footer_y = area.bottom() - 1;
        surface.set_style(Rect::new(area.x, footer_y, area.width, 1), palette.hint);
        let footer = format!(
            " hunk {}/{} · n/N or ]c/[c next/prev hunk · j/k scroll · h/l pan · enter open at hunk · q close",
            self.current + 1,
            self.hunks.len(),
        );
        surface.set_stringn(area.x, footer_y, &footer, area.width as usize, palette.hint);
    }

    fn cursor(&self, _area: Rect, _editor: &Editor) -> (Option<helix_core::Position>, CursorKind) {
        (None, CursorKind::Hidden)
    }

    fn id(&self) -> Option<&'static str> {
        Some(ID)
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod test {
    use super::*;

    fn kinds(rows: &[Row]) -> Vec<(RowKind, Option<usize>, Option<usize>)> {
        rows.iter()
            .map(|row| (row.kind, row.left, row.right))
            .collect()
    }

    #[test]
    fn splits_lines_like_imara() {
        assert_eq!(line_ranges("a\nbc\r\nd"), [0..1, 2..4, 6..7]);
        assert_eq!(line_ranges("a\n"), [0..1]);
        assert!(line_ranges("").is_empty());
    }

    #[test]
    fn aligns_rows() {
        use RowKind::*;
        let before = "fn main() {\n    let x = 1;\n    println!(x);\n}\n";
        let after = "fn main() {\n    let x = 2;\n    let y = 3;\n    println!(x);\n}\n";
        let (rows, hunks) = build_rows(before, after);
        assert_eq!(
            kinds(&rows),
            [
                (Equal, Some(0), Some(0)),
                (Changed, Some(1), Some(1)),
                (Added, None, Some(2)),
                (Equal, Some(2), Some(3)),
                (Equal, Some(3), Some(4)),
            ]
        );
        assert_eq!(hunks, [1]);
        // only the number changed
        assert_eq!(rows[1].left_words, [12..13]);
        assert_eq!(rows[1].right_words, [12..13]);
    }

    #[test]
    fn handles_new_and_deleted_files() {
        let (rows, hunks) = build_rows("", "a\nb\n");
        assert_eq!(
            kinds(&rows),
            [
                (RowKind::Added, None, Some(0)),
                (RowKind::Added, None, Some(1))
            ]
        );
        assert_eq!(hunks, [0]);

        let (rows, _) = build_rows("a\n", "");
        assert_eq!(kinds(&rows), [(RowKind::Removed, Some(0), None)]);

        let (rows, hunks) = build_rows("same\n", "same\n");
        assert_eq!(kinds(&rows), [(RowKind::Equal, Some(0), Some(0))]);
        assert!(hunks.is_empty());
    }

    #[test]
    fn word_diff_skips_completely_different_lines() {
        assert_eq!(word_diff("let a = 1;", "return foo;"), (vec![], vec![]));
        assert_eq!(word_diff("foo(bar)", "foo(baz)"), (vec![4..7], vec![4..7]));
    }
}

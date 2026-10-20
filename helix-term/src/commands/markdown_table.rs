//! Formats Markdown tables: pads the cells so the columns line up, and rewrites the delimiter row
//! to match, keeping each column's alignment.

use helix_core::{line_ending::line_end_char_index, unicode::width::UnicodeWidthStr, Transaction};
use helix_view::{current, Editor};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Align {
    None,
    Left,
    Center,
    Right,
}

/// Formats the tables touched by the selections. A table is formatted as a whole even if only part
/// of it is selected, so a cursor anywhere inside a table is enough.
pub(crate) fn format(editor: &mut Editor) {
    let scrolloff = editor.config().scrolloff;
    let (view, doc) = current!(editor);
    let text = doc.text();
    let slice = text.slice(..);
    let line_text = |line: usize| {
        slice
            .slice(text.line_to_char(line)..line_end_char_index(&slice, line))
            .to_string()
    };
    let is_table_line = |line: usize| has_pipe(&line_text(line));

    // Line spans covered by the selections, grown to the whole tables at their edges.
    let mut spans: Vec<(usize, usize)> = doc
        .selection(view.id)
        .iter()
        .map(|range| {
            let (mut start, mut end) = range.line_range(slice);
            if is_table_line(start) {
                while start > 0 && is_table_line(start - 1) {
                    start -= 1;
                }
            }
            if is_table_line(end) {
                while end + 1 < text.len_lines() && is_table_line(end + 1) {
                    end += 1;
                }
            }
            (start, end)
        })
        .collect();
    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in spans {
        match merged.last_mut() {
            Some(last) if start <= last.1 + 1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }

    let line_ending = doc.line_ending.as_str();
    let mut changes = Vec::new();
    for (start, end) in merged {
        let mut line = start;
        while line <= end {
            if !is_table_line(line) {
                line += 1;
                continue;
            }
            let first = line;
            let mut lines = Vec::new();
            while line <= end && is_table_line(line) {
                lines.push(line_text(line));
                line += 1;
            }
            let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
            let Some(mut formatted) = format_table(&lines) else {
                continue;
            };
            // One change per line, so the cursors stay on their lines.
            if formatted.len() > lines.len() {
                let delimiter = formatted.remove(1);
                formatted[0] = format!("{}{line_ending}{delimiter}", formatted[0]);
            }
            for (i, (old, new)) in lines.iter().zip(formatted).enumerate() {
                if *old != new {
                    let from = text.line_to_char(first + i);
                    let to = line_end_char_index(&slice, first + i);
                    changes.push((from, to, Some(new.into())));
                }
            }
        }
    }

    if changes.is_empty() {
        editor.set_status("No Markdown table to format");
        return;
    }
    let transaction = Transaction::change(text, changes.into_iter());
    doc.apply(&transaction, view.id);
    doc.append_changes_to_history(view);
    view.ensure_cursor_in_view(doc, scrolloff);
}

/// Formats the rows of one table. Returns `None` if the lines don't look like a table: the second
/// line must be a delimiter row, unless every line starts with a pipe, in which case a delimiter row
/// is inserted after the first one.
fn format_table(lines: &[&str]) -> Option<Vec<String>> {
    let mut rows: Vec<Vec<String>> = lines.iter().map(|line| split_row(line)).collect();
    let aligns: Vec<Align> = match rows.get(1).and_then(|row| delimiter_aligns(row)) {
        Some(aligns) => {
            rows.remove(1);
            aligns
        }
        None if lines.iter().all(|line| line.trim_start().starts_with('|')) => Vec::new(),
        None => return None,
    };

    let columns = rows.iter().map(Vec::len).chain([aligns.len()]).max()?;
    let mut widths = vec![3; columns];
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.width());
        }
    }
    let align = |column: usize| aligns.get(column).copied().unwrap_or(Align::None);

    let indent = &lines[0][..lines[0].len() - lines[0].trim_start().len()];
    let render = |cells: Vec<String>| format!("{indent}| {} |", cells.join(" | "));
    let mut formatted: Vec<String> = rows
        .iter()
        .map(|row| {
            render(
                (0..columns)
                    .map(|column| {
                        let cell = row.get(column).map(String::as_str).unwrap_or("");
                        pad(cell, widths[column], align(column))
                    })
                    .collect(),
            )
        })
        .collect();
    let delimiter = (0..columns)
        .map(|column| {
            let width = widths[column];
            match align(column) {
                Align::None => "-".repeat(width),
                Align::Left => format!(":{}", "-".repeat(width - 1)),
                Align::Right => format!("{}:", "-".repeat(width - 1)),
                Align::Center => format!(":{}:", "-".repeat(width - 2)),
            }
        })
        .collect();
    formatted.insert(1.min(formatted.len()), render(delimiter));
    Some(formatted)
}

fn pad(cell: &str, width: usize, align: Align) -> String {
    let padding = width.saturating_sub(cell.width());
    let (left, right) = match align {
        Align::None | Align::Left => (0, padding),
        Align::Right => (padding, 0),
        Align::Center => (padding / 2, padding - padding / 2),
    };
    format!("{}{cell}{}", " ".repeat(left), " ".repeat(right))
}

/// Splits a row into its trimmed cells, dropping the outer pipes. `\|` doesn't split.
fn split_row(line: &str) -> Vec<String> {
    let line = line.trim();
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                cell.push(c);
                cell.extend(chars.next());
            }
            '|' => cells.push(std::mem::take(&mut cell)),
            _ => cell.push(c),
        }
    }
    cells.push(cell);
    if line.starts_with('|') {
        cells.remove(0);
    }
    if cells.len() > 1 && cells.last().is_some_and(String::is_empty) {
        cells.pop();
    }
    cells.iter().map(|cell| cell.trim().to_string()).collect()
}

fn has_pipe(line: &str) -> bool {
    let mut escaped = false;
    for c in line.chars() {
        match c {
            '|' if !escaped => return true,
            '\\' => escaped = !escaped,
            _ => escaped = false,
        }
    }
    false
}

/// The column alignments of a delimiter row such as `| :-- | :-: | --: |`, or `None` if the row
/// isn't one.
fn delimiter_aligns(row: &[String]) -> Option<Vec<Align>> {
    row.iter()
        .map(|cell| {
            let left = cell.starts_with(':');
            let right = cell.ends_with(':');
            let dashes = cell.trim_start_matches(':').trim_end_matches(':');
            if dashes.is_empty() || !dashes.chars().all(|c| c == '-') {
                return None;
            }
            Some(match (left, right) {
                (false, false) => Align::None,
                (true, false) => Align::Left,
                (false, true) => Align::Right,
                (true, true) => Align::Center,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format(input: &str) -> Option<String> {
        format_table(&input.lines().collect::<Vec<_>>()).map(|lines| lines.join("\n"))
    }

    #[test]
    fn aligns_columns() {
        assert_eq!(
            format("| Name | Description |\n|-|-|\n| a | longer text |\n| abcdef | x |").unwrap(),
            "| Name   | Description |\n\
             | ------ | ----------- |\n\
             | a      | longer text |\n\
             | abcdef | x           |"
        );
    }

    #[test]
    fn keeps_alignments() {
        assert_eq!(
            format("|l|c|r|\n|:-|:-:|-:|\n|a|b|c|\n|left|center|right|").unwrap(),
            "| l    |   c    |     r |\n\
             | :--- | :----: | ----: |\n\
             | a    |   b    |     c |\n\
             | left | center | right |"
        );
    }

    #[test]
    fn without_outer_pipes() {
        assert_eq!(
            format("a | b\n--- | ---\nlonger | c").unwrap(),
            "| a      | b   |\n\
             | ------ | --- |\n\
             | longer | c   |"
        );
    }

    #[test]
    fn inserts_missing_delimiter_row() {
        assert_eq!(
            format("| a | b |\n| c | d |").unwrap(),
            "| a   | b   |\n\
             | --- | --- |\n\
             | c   | d   |"
        );
        assert_eq!(format("| a | b |").unwrap(), "| a   | b   |\n| --- | --- |");
    }

    #[test]
    fn pads_missing_cells_and_keeps_empty_ones() {
        assert_eq!(
            format("| a | b | c |\n|---|---|---|\n| x |\n| | y | |").unwrap(),
            "| a   | b   | c   |\n\
             | --- | --- | --- |\n\
             | x   |     |     |\n\
             |     | y   |     |"
        );
    }

    #[test]
    fn escaped_pipes_and_wide_chars() {
        assert_eq!(
            format("| a \\| b | 日本 |\n|---|---|\n| c | d |").unwrap(),
            "| a \\| b | 日本 |\n\
             | ------ | ---- |\n\
             | c      | d    |"
        );
    }

    #[test]
    fn keeps_indentation() {
        assert_eq!(
            format("  | a | b |\n  |---|---|\n| cc | d |").unwrap(),
            "  | a   | b   |\n  | --- | --- |\n  | cc  | d   |"
        );
    }

    #[test]
    fn rejects_non_tables() {
        assert_eq!(format("a | b\nc | d"), None);
        assert!(!has_pipe("a \\| b"));
        assert!(has_pipe("a \\\\| b"));
    }
}

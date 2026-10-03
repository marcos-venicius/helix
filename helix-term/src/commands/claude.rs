use std::fmt::Write;
use std::path::Path;

use helix_view::{current_ref, editor::InteractiveCommand, Editor};

/// Selections longer than this are truncated in the context sent to Claude.
const MAX_SELECTION_LINES: usize = 500;

/// Requests the application to open Claude Code in the terminal, with the current file, cursor
/// line and selection as context. `prompt` is sent as the first message, if given.
pub(crate) fn claude_request(editor: &mut Editor, prompt: Option<String>) -> anyhow::Result<()> {
    let (view, doc) = current_ref!(editor);

    if doc.path().is_some() && doc.is_modified() {
        anyhow::bail!("Save the buffer before opening Claude");
    }

    let text = doc.text().slice(..);
    let range = doc.selection(view.id).primary();

    let mut context = String::from(
        "The user is talking to you from inside the Helix text editor. \
         The file, line and selection below are the focus of the conversation; \
         when they ask for changes, edit the file on disk.\n",
    );
    match doc.path() {
        Some(path) => writeln!(context, "Current file: {}", path.display())?,
        None => writeln!(context, "Current buffer: unsaved scratch buffer")?,
    }

    if range.len() <= 1 {
        writeln!(context, "Cursor at line {}", range.cursor_line(text) + 1)?;
        if doc.path().is_none() {
            writeln!(context, "Buffer contents:\n```\n{}\n```", doc.text())?;
        }
    } else {
        let (start, end) = range.line_range(text);
        writeln!(context, "Selected lines {}-{}:", start + 1, end + 1)?;
        let fragment = range.fragment(text);
        let mut lines = fragment.lines();
        let selected: Vec<_> = lines.by_ref().take(MAX_SELECTION_LINES).collect();
        writeln!(context, "```\n{}\n```", selected.join("\n"))?;
        if lines.next().is_some() {
            writeln!(
                context,
                "(selection truncated to the first {MAX_SELECTION_LINES} lines)"
            )?;
        }
    }

    let cwd = helix_stdx::env::current_working_dir();

    let mut args = Vec::new();
    if has_previous_conversation(&cwd) {
        args.push("--continue".to_string());
    }
    args.push("--append-system-prompt".to_string());
    args.push(context);
    if let Some(prompt) = prompt.filter(|prompt| !prompt.trim().is_empty()) {
        args.push(prompt);
    }

    editor.interactive_command = Some(InteractiveCommand {
        program: "claude".to_string(),
        args,
        cwd,
    });
    Ok(())
}

/// Claude Code stores conversations under `~/.claude/projects/<cwd>`, with every
/// non-alphanumeric character of the path replaced by `-`. `claude --continue` fails when
/// there is nothing to continue, so only pass it when a conversation exists.
fn has_previous_conversation(cwd: &Path) -> bool {
    let Ok(home) = helix_stdx::path::home_dir() else {
        return false;
    };
    let project: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let Ok(entries) = std::fs::read_dir(home.join(".claude").join("projects").join(project)) else {
        return false;
    };
    entries
        .flatten()
        .any(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
}

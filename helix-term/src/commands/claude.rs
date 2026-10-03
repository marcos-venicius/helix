use std::fmt::Write;
use std::hash::{BuildHasher, RandomState};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use helix_view::{current_ref, editor::InteractiveCommand, Editor};

/// Selections longer than this are truncated in the context sent to Claude.
const MAX_SELECTION_LINES: usize = 500;

/// The Claude session started from this editor instance, with the working directory it was
/// created in. Only this session is ever resumed, so every new editor starts a fresh one.
static SESSION: Mutex<Option<(PathBuf, String)>> = Mutex::new(None);

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

    let mut args = session_args(&cwd);
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

/// Returns `--resume <id>` if this editor already has a Claude session for `cwd`, or
/// `--session-id <id>` with a new ID otherwise.
fn session_args(cwd: &Path) -> Vec<String> {
    let mut session = SESSION.lock().unwrap();
    let id = match &*session {
        Some((session_cwd, id)) if session_cwd == cwd => id.clone(),
        // No session yet, or `:cd` moved to another project: start a new one.
        _ => {
            let id = new_uuid();
            *session = Some((cwd.to_path_buf(), id.clone()));
            id
        }
    };

    // Claude only writes the session file after the first message, so a session that was
    // opened and closed without talking cannot be resumed yet.
    let flag = if session_file_exists(cwd, &id) {
        "--resume"
    } else {
        "--session-id"
    };
    vec![flag.to_string(), id]
}

/// Claude Code stores conversations in `~/.claude/projects/<cwd>/<session id>.jsonl`, with
/// every non-alphanumeric character of the path replaced by `-`.
fn session_file_exists(cwd: &Path, id: &str) -> bool {
    let Ok(home) = helix_stdx::path::home_dir() else {
        return false;
    };
    let project: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    home.join(".claude")
        .join("projects")
        .join(project)
        .join(format!("{id}.jsonl"))
        .exists()
}

/// Generates a random (version 4) UUID. `RandomState` is seeded with random keys, so this
/// avoids pulling in a dependency just for session IDs.
fn new_uuid() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let high = RandomState::new().hash_one((nanos, std::process::id()));
    let low = RandomState::new().hash_one((high, nanos));
    let bytes = ((u128::from(high) << 64) | u128::from(low))
        // Set the version (4) and variant (RFC 4122) bits.
        & !(0xf000_u128 << 64 | 0xc000_u128 << 48)
        | (0x4000_u128 << 64 | 0x8000_u128 << 48);
    let hex = format!("{bytes:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn uuid_is_valid_v4() {
        let id = new_uuid();
        assert_eq!(id.len(), 36);
        let parts: Vec<_> = id.split('-').map(str::len).collect();
        assert_eq!(parts, [8, 4, 4, 4, 12]);
        assert!(id.as_bytes()[14] == b'4');
        assert!(matches!(id.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
        assert_ne!(new_uuid(), id);
    }
}

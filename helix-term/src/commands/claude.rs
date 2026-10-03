use std::fmt::Write;
use std::sync::Arc;

use helix_view::{current_ref, Editor};

use crate::compositor::Compositor;
use crate::ui::{self, claude::Session, PickerColumn};

/// Selections longer than this are truncated in the context sent to Claude.
const MAX_SELECTION_LINES: usize = 500;

/// Where the user currently is: used as context for new sessions and as the reference pasted
/// into existing ones.
struct Location {
    /// `path#Lstart-end`, relative to the working directory, if the buffer has a path.
    reference: Option<String>,
    /// Short description for the session picker, e.g. `src/main.rs:12`.
    label: String,
    /// System prompt addition describing the file, line and selection.
    context: String,
}

fn current_location(editor: &Editor) -> anyhow::Result<Location> {
    let (view, doc) = current_ref!(editor);
    let text = doc.text().slice(..);
    let range = doc.selection(view.id).primary();
    let (start, end) = range.line_range(text);
    let cursor_line = range.cursor_line(text);
    let selected = range.len() > 1;

    let relative = doc.path().map(|path| {
        helix_stdx::path::get_relative_path(path)
            .display()
            .to_string()
    });
    let lines = if selected && start != end {
        format!("L{}-{}", start + 1, end + 1)
    } else if selected {
        format!("L{}", start + 1)
    } else {
        format!("L{}", cursor_line + 1)
    };
    let reference = relative.as_ref().map(|path| format!("@{path}#{lines}"));
    let label = match &relative {
        Some(path) => format!("{path}:{}", cursor_line + 1),
        None => "[scratch]".to_string(),
    };

    let mut context = String::from(
        "The user is talking to you from inside the Helix text editor. \
         The file, line and selection below are the focus of the conversation; \
         when they ask for changes, edit the file on disk. Later messages may \
         reference other locations as `@path#Lstart-end`.\n",
    );
    match doc.path() {
        Some(path) => writeln!(context, "Current file: {}", path.display())?,
        None => writeln!(context, "Current buffer: unsaved scratch buffer")?,
    }

    if !selected {
        writeln!(context, "Cursor at line {}", cursor_line + 1)?;
        if doc.path().is_none() {
            writeln!(context, "Buffer contents:\n```\n{}\n```", doc.text())?;
        }
    } else {
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

    Ok(Location {
        reference,
        label,
        context,
    })
}

/// Starts a new Claude session with the current location as context and shows it.
fn new_session(
    editor: &mut Editor,
    compositor: &mut Compositor,
    prompt: Option<String>,
) -> anyhow::Result<()> {
    let doc = helix_view::doc!(editor);
    if doc.path().is_some() && doc.is_modified() {
        anyhow::bail!("Save the buffer before starting a Claude session");
    }

    let location = current_location(editor)?;
    let mut args = vec!["--append-system-prompt".to_string(), location.context];
    if let Some(prompt) = prompt.filter(|prompt| !prompt.trim().is_empty()) {
        args.push(prompt);
    }

    let session = Session::spawn(
        location.label,
        args,
        helix_stdx::env::current_working_dir(),
        location.reference,
        compositor.size(),
    )?;
    ui::claude::show(compositor, session);
    Ok(())
}

/// Shows an existing session, pasting a reference to the current location into its prompt.
fn reopen_session(editor: &mut Editor, compositor: &mut Compositor, session: Arc<Session>) {
    if let Ok(Location {
        reference: Some(reference),
        ..
    }) = current_location(editor)
    {
        session.paste_reference(reference);
    }
    ui::claude::show(compositor, session);
}

/// Shows the most recent session, or starts one if none is running.
pub(crate) fn toggle(editor: &mut Editor, compositor: &mut Compositor) {
    if compositor.remove(ui::claude::ID).is_some() {
        crate::commands::reload_documents(editor, true);
        return;
    }
    let result = match ui::claude::last_session() {
        Some(session) => {
            reopen_session(editor, compositor, session);
            Ok(())
        }
        None => new_session(editor, compositor, None),
    };
    if let Err(err) = result {
        editor.set_error(err.to_string());
    }
}

/// Starts a new session, sending `prompt` as the first message.
pub(crate) fn start(editor: &mut Editor, compositor: &mut Compositor, prompt: String) {
    if let Err(err) = new_session(editor, compositor, Some(prompt)) {
        editor.set_error(err.to_string());
    }
}

pub(crate) enum PickerItem {
    New,
    Session(Arc<Session>),
}

/// A picker listing the running sessions plus an entry to start a new one.
pub(crate) fn session_picker() -> ui::Picker<PickerItem, ()> {
    let columns = [
        PickerColumn::new("session", |item: &PickerItem, _| match item {
            PickerItem::New => "+ new session".into(),
            PickerItem::Session(session) => format!("#{}", session.id).into(),
        }),
        PickerColumn::new("started from", |item: &PickerItem, _| match item {
            PickerItem::New => "".into(),
            PickerItem::Session(session) => session.label.as_str().into(),
        }),
        PickerColumn::new("title", |item: &PickerItem, _| match item {
            PickerItem::New => "".into(),
            PickerItem::Session(session) => session.title().into(),
        }),
    ];

    let mut items: Vec<_> = ui::claude::sessions()
        .into_iter()
        .rev()
        .map(PickerItem::Session)
        .collect();
    items.push(PickerItem::New);

    ui::Picker::new(columns, 0, items, (), |cx, item, _action| {
        let item = match item {
            PickerItem::New => None,
            PickerItem::Session(session) => Some(session.clone()),
        };
        cx.jobs.callback(async move {
            let call = move |editor: &mut Editor, compositor: &mut Compositor| match item {
                Some(session) => reopen_session(editor, compositor, session),
                None => {
                    if let Err(err) = new_session(editor, compositor, None) {
                        editor.set_error(err.to_string());
                    }
                }
            };
            Ok(crate::job::Callback::EditorCompositor(Box::new(call)))
        });
    })
}

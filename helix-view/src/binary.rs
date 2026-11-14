//! Fork feature: refuse to open binary files (images, executables, archives...) as text.
//!
//! [`Editor::open`](crate::Editor::open) calls [`is_binary_file`] before loading a file and
//! returns [`DocumentOpenError::BinaryFile`](crate::document::DocumentOpenError::BinaryFile)
//! when it's binary, so every way of opening a file (command line, `:open`, pickers, the file
//! explorer, goto definition...) shows an error instead of a buffer full of garbage.

use std::{
    fmt,
    io::Read,
    path::{Path, PathBuf},
};

/// The error behind [`DocumentOpenError::BinaryFile`](crate::document::DocumentOpenError::BinaryFile).
/// It's also its `source()`, which is what the pickers show when opening fails.
#[derive(thiserror::Error)]
#[error(
    "binary file can't be opened: {}",
    helix_stdx::path::get_relative_path(.path).display()
)]
pub struct BinaryFileError {
    pub path: PathBuf,
}

impl BinaryFileError {
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
        }
    }
}

// Some callers print the open error with `{:?}`, so it reads like the message.
impl fmt::Debug for BinaryFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// How many bytes from the start of the file are inspected.
const SNIFF_LEN: u64 = 1024;

/// Whether the file at `path` looks binary. Files that don't exist or can't be read are not
/// binary, so opening them follows the usual path (a new file, or the read error).
pub fn is_binary_file(path: &Path) -> bool {
    let mut buffer = Vec::with_capacity(SNIFF_LEN as usize);
    let read =
        std::fs::File::open(path).and_then(|file| file.take(SNIFF_LEN).read_to_end(&mut buffer));
    read.is_ok() && is_binary(&buffer)
}

/// The same heuristic as the picker preview (`helix_term::is_binary`): a leading byte-order mark
/// means text (UTF-16/32 text has NUL bytes), otherwise a NUL byte or a PDF/PNG magic number
/// means binary. Executables, archives and most image formats have NULs in their headers.
fn is_binary(buffer: &[u8]) -> bool {
    // UTF-32 BOMs must be checked before UTF-16 (their BOMs overlap).
    const BYTE_ORDER_MARKS: &[&[u8]] = &[
        &[0xEF, 0xBB, 0xBF],       // UTF-8
        &[0x00, 0x00, 0xFE, 0xFF], // UTF-32BE
        &[0xFF, 0xFE, 0x00, 0x00], // UTF-32LE
        &[0xFE, 0xFF],             // UTF-16BE
        &[0xFF, 0xFE],             // UTF-16LE
    ];

    if BYTE_ORDER_MARKS.iter().any(|bom| buffer.starts_with(bom)) {
        return false;
    }

    buffer.contains(&0) || buffer.starts_with(b"%PDF") || buffer.starts_with(b"\x89PNG")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn detects_binary_content() {
        assert!(!is_binary(b""));
        assert!(!is_binary(b"plain text\nsecond line"));
        assert!(!is_binary("acentuação ✓".as_bytes()));
        assert!(!is_binary(b"\xFF\xFEt\0e\0x\0t\0")); // UTF-16LE
        assert!(!is_binary(b"\xEF\xBB\xBFtext")); // UTF-8 BOM
        assert!(is_binary(b"text\0with nul"));
        assert!(is_binary(b"\x7FELF\x02\x01\x01\0")); // executable
        assert!(is_binary(b"PK\x03\x04\x14\0\0\0")); // zip
        assert!(is_binary(b"\x89PNG\r\n\x1a\n"));
        assert!(is_binary(b"%PDF-1.7 ..."));
    }

    #[test]
    fn checks_files_on_disk() {
        let dir = tempfile::tempdir().unwrap();

        let text = dir.path().join("text.txt");
        std::fs::write(&text, "hello\n").unwrap();
        assert!(!is_binary_file(&text));

        let binary = dir.path().join("image.png");
        let mut file = std::fs::File::create(&binary).unwrap();
        file.write_all(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").unwrap();
        assert!(is_binary_file(&binary));

        assert!(!is_binary_file(&dir.path().join("missing.rs")));
    }
}

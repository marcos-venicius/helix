//! File type icons for the explorer: Nerd Font glyphs, colored by type. They need a terminal font
//! patched with Nerd Fonts, otherwise `editor.explorer.icons = false` turns them off.

use helix_view::graphics::Color;

/// A glyph and its color.
pub type Icon = (&'static str, Color);

const FOLDER: Icon = ("\u{f07b}", Color::Rgb(0x7e, 0xb6, 0xe8));
const FOLDER_OPEN: Icon = ("\u{f07c}", Color::Rgb(0x7e, 0xb6, 0xe8));
const FILE: Icon = ("\u{f15b}", Color::Rgb(0x9a, 0xa5, 0xb1));
const TEXT: Icon = ("\u{f0f6}", Color::Rgb(0x9a, 0xa5, 0xb1));
const CONFIG: Icon = ("\u{f013}", Color::Rgb(0x8f, 0x9b, 0xa8));
const GIT: Icon = ("\u{e702}", Color::Rgb(0xf1, 0x50, 0x2f));
const LOCK: Icon = ("\u{f023}", Color::Rgb(0xbb, 0xbb, 0xbb));
const SHELL: Icon = ("\u{f120}", Color::Rgb(0x89, 0xe0, 0x51));
const IMAGE: Icon = ("\u{f1c5}", Color::Rgb(0xa0, 0x74, 0xc4));
const ARCHIVE: Icon = ("\u{f1c6}", Color::Rgb(0xec, 0xa5, 0x17));
const DATABASE: Icon = ("\u{e706}", Color::Rgb(0xda, 0xd8, 0xd8));

/// Icon of a directory.
pub fn directory(expanded: bool) -> Icon {
    if expanded {
        FOLDER_OPEN
    } else {
        FOLDER
    }
}

/// Icon of a file, from its whole name first (`Cargo.lock`, `.gitignore`), then its extension.
pub fn file(name: &str) -> Icon {
    let by_name = match name {
        ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitkeep" => Some(GIT),
        "Dockerfile" | "Containerfile" | "docker-compose.yml" | "docker-compose.yaml" => {
            Some(("\u{e7b0}", Color::Rgb(0x45, 0x8e, 0xe6)))
        }
        "Makefile" | "makefile" | "justfile" | "Justfile" => Some(CONFIG),
        "LICENSE" | "LICENSE.md" | "LICENSE.txt" | "COPYING" => {
            Some(("\u{f0e3}", Color::Rgb(0xd0, 0xbf, 0x41)))
        }
        _ if name.ends_with(".lock") || name.ends_with("-lock.json") => Some(LOCK),
        _ => None,
    };
    if let Some(icon) = by_name {
        return icon;
    }
    let extension = name
        .rsplit_once('.')
        .map(|(stem, extension)| if stem.is_empty() { "" } else { extension })
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "rs" => ("\u{e7a8}", Color::Rgb(0xde, 0xa5, 0x84)),
        "md" | "markdown" => ("\u{e73e}", Color::Rgb(0xdd, 0xdd, 0xdd)),
        "js" | "mjs" | "cjs" | "jsx" => ("\u{e74e}", Color::Rgb(0xcb, 0xcb, 0x41)),
        "ts" | "mts" | "cts" | "tsx" => ("\u{e628}", Color::Rgb(0x51, 0x9a, 0xba)),
        "py" | "pyi" => ("\u{e73c}", Color::Rgb(0xff, 0xbc, 0x03)),
        "go" => ("\u{e724}", Color::Rgb(0x00, 0xad, 0xd8)),
        "c" | "h" => ("\u{e61e}", Color::Rgb(0x59, 0x9e, 0xff)),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => ("\u{e61d}", Color::Rgb(0xf3, 0x4b, 0x7d)),
        "java" => ("\u{e738}", Color::Rgb(0xcc, 0x3e, 0x44)),
        "rb" => ("\u{e739}", Color::Rgb(0x70, 0x15, 0x16)),
        "php" => ("\u{e73d}", Color::Rgb(0xa0, 0x74, 0xc4)),
        "swift" => ("\u{e755}", Color::Rgb(0xe3, 0x79, 0x33)),
        "scala" => ("\u{e737}", Color::Rgb(0xcc, 0x3e, 0x44)),
        "hs" => ("\u{e777}", Color::Rgb(0xa0, 0x74, 0xc4)),
        "clj" | "cljs" | "edn" => ("\u{e768}", Color::Rgb(0x8d, 0xc1, 0x49)),
        "erl" | "hrl" => ("\u{e7b1}", Color::Rgb(0xb8, 0x39, 0x98)),
        "lua" => ("\u{e620}", Color::Rgb(0x51, 0xa0, 0xcf)),
        "nix" => ("\u{f313}", Color::Rgb(0x7e, 0xba, 0xe4)),
        "vim" => ("\u{e7c5}", Color::Rgb(0x01, 0x98, 0x33)),
        "html" | "htm" => ("\u{e736}", Color::Rgb(0xe4, 0x4d, 0x26)),
        "css" | "scss" | "sass" | "less" => ("\u{e749}", Color::Rgb(0x42, 0xa5, 0xf5)),
        "json" | "jsonc" | "json5" => ("\u{e60b}", Color::Rgb(0xcb, 0xcb, 0x41)),
        "toml" | "yaml" | "yml" | "ini" | "conf" | "cfg" | "env" => CONFIG,
        "sh" | "bash" | "zsh" | "fish" | "nu" | "ps1" => SHELL,
        "sql" | "db" | "sqlite" | "sqlite3" => DATABASE,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "bmp" => IMAGE,
        "zip" | "tar" | "gz" | "xz" | "bz2" | "zst" | "7z" | "rar" => ARCHIVE,
        "pdf" => ("\u{f1c1}", Color::Rgb(0xb3, 0x0b, 0x00)),
        "txt" | "log" | "csv" | "tsv" => TEXT,
        _ => FILE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_win_over_extensions() {
        assert_eq!(file("Cargo.lock"), LOCK);
        assert_eq!(file("package-lock.json"), LOCK);
        assert_eq!(file(".gitignore"), GIT);
        assert_eq!(file("LICENSE"), file("LICENSE.md"));
    }

    #[test]
    fn extensions_are_case_insensitive() {
        assert_eq!(file("main.rs"), file("MAIN.RS"));
        assert_eq!(file("photo.JPG"), IMAGE);
    }

    #[test]
    fn dotfiles_and_unknown_files_get_the_default_icon() {
        // A leading dot starts a name, not an extension.
        assert_eq!(file(".rs"), FILE);
        assert_eq!(file("README"), FILE);
        assert_eq!(file("archive.unknown"), FILE);
    }
}

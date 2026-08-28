//! Binary file extensions to skip for text-based operations.
//! Port of `tools/binary_extensions.py` (75 lines) — 1:1 behavior.
//!
//! These files can't be meaningfully compared as text and are often large.
//! Ported from free-code src/constants/files.ts.

/// Binary extensions that should be skipped for text operations.
///
/// Mirrors `BINARY_EXTENSIONS` in Python (`frozenset`).
pub const BINARY_EXTENSIONS: &[&str] = &[
    // Images
    ".png", ".jpg", ".jpeg", ".gif", ".bmp", ".ico", ".webp", ".tiff", ".tif",
    // Videos
    ".mp4", ".mov", ".avi", ".mkv", ".webm", ".wmv", ".flv", ".m4v", ".mpeg", ".mpg",
    // Audio
    ".mp3", ".wav", ".ogg", ".flac", ".aac", ".m4a", ".wma", ".aiff", ".opus",
    // Archives
    ".zip", ".tar", ".gz", ".bz2", ".7z", ".rar", ".xz", ".z", ".tgz", ".iso",
    // Executables/binaries
    ".exe", ".dll", ".so", ".dylib", ".bin", ".o", ".a", ".obj", ".lib", ".app",
    ".msi", ".deb", ".rpm",
    // Documents (exclude .pdf — text-based, agents may want to inspect)
    ".doc", ".docx", ".xls", ".xlsx", ".ppt", ".pptx", ".odt", ".ods", ".odp",
    // Fonts
    ".ttf", ".otf", ".woff", ".woff2", ".eot",
    // Bytecode / VM artifacts
    ".pyc", ".pyo", ".class", ".jar", ".war", ".ear", ".node", ".wasm", ".rlib",
    // Database files
    ".sqlite", ".sqlite3", ".db", ".mdb", ".idx",
    // Design / 3D
    ".psd", ".ai", ".eps", ".sketch", ".fig", ".xd", ".blend", ".3ds", ".max",
    // Flash
    ".swf", ".fla",
    // Lock/profiling data
    ".lockb", ".dat", ".data",
];

/// Container document formats (OOXML zip / OLE compound / ODF zip / EPUB zip / RTF)
/// that a plain-text write can NEVER produce validly.  read_file auto-extracts
/// these to readable text (via anydoc for the non-built-in formats), so a model
/// that "read" report.docx and then writes the edited text back via
/// write_file/patch silently destroys the document.
/// PDF is intentionally NOT here: raw PDF syntax is text-authorable, so
/// new-file creation is legitimate — only overwrites are dangerous (handled
/// separately by the write guard).
///
/// Mirrors `OPAQUE_DOCUMENT_EXTENSIONS` in Python (`frozenset`).
pub const OPAQUE_DOCUMENT_EXTENSIONS: &[&str] = &[
    ".doc", ".docx", ".docm", ".xls", ".xlsx", ".xlsm", ".xlsb", ".ppt", ".pps", ".pot",
    ".pptx", ".pptm", ".ppsx", ".ppsm", ".odt", ".ods", ".odp", ".rtf", ".epub",
];

/// Check if a file path has a binary extension. Pure string check, no I/O.
///
/// Mirrors `has_binary_extension(path: str) -> bool` in Python:
/// `dot = path.rfind("."); if dot == -1: return False; return path[dot:].lower() in BINARY_EXTENSIONS`
pub fn has_binary_extension(path: &str) -> bool {
    match path.rfind('.') {
        None => false,
        Some(dot) => {
            let ext = path[dot..].to_ascii_lowercase();
            BINARY_EXTENSIONS.contains(&ext.as_str())
        }
    }
}

/// True when the path names an opaque container document (.docx etc.).
/// Pure string check, no I/O.
///
/// Mirrors `has_opaque_document_extension(path: str) -> bool` in Python.
pub fn has_opaque_document_extension(path: &str) -> bool {
    match path.rfind('.') {
        None => false,
        Some(dot) => {
            let ext = path[dot..].to_ascii_lowercase();
            OPAQUE_DOCUMENT_EXTENSIONS.contains(&ext.as_str())
        }
    }
}

/// True when the path has a .pdf extension. Pure string check, no I/O.
///
/// Mirrors `is_pdf_path(path: str) -> bool` in Python:
/// `return path.lower().endswith(".pdf")`
pub fn is_pdf_path(path: &str) -> bool {
    path.len() >= 4 && path[path.len() - 4..].eq_ignore_ascii_case(".pdf")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_extensions_contain_expected() {
        assert!(BINARY_EXTENSIONS.contains(&".png"));
        assert!(BINARY_EXTENSIONS.contains(&".mp4"));
        assert!(BINARY_EXTENSIONS.contains(&".zip"));
        assert!(BINARY_EXTENSIONS.contains(&".exe"));
        assert!(BINARY_EXTENSIONS.contains(&".psd"));
        assert!(BINARY_EXTENSIONS.contains(&".swf"));
        assert!(BINARY_EXTENSIONS.contains(&".lockb"));
        // .pdf is intentionally excluded
        assert!(!BINARY_EXTENSIONS.contains(&".pdf"));
    }

    #[test]
    fn opaque_document_extensions_contain_expected() {
        assert!(OPAQUE_DOCUMENT_EXTENSIONS.contains(&".docx"));
        assert!(OPAQUE_DOCUMENT_EXTENSIONS.contains(&".docm"));
        assert!(OPAQUE_DOCUMENT_EXTENSIONS.contains(&".xlsm"));
        assert!(OPAQUE_DOCUMENT_EXTENSIONS.contains(&".pptx"));
        assert!(OPAQUE_DOCUMENT_EXTENSIONS.contains(&".rtf"));
        assert!(OPAQUE_DOCUMENT_EXTENSIONS.contains(&".epub"));
        // .pdf intentionally not here
        assert!(!OPAQUE_DOCUMENT_EXTENSIONS.contains(&".pdf"));
    }

    #[test]
    fn has_binary_extension_matches_python() {
        assert!(has_binary_extension("image.png"));
        assert!(has_binary_extension("IMAGE.PNG"));
        assert!(has_binary_extension("video.Mp4"));
        assert!(has_binary_extension("/tmp/archive.ZIP"));
        assert!(has_binary_extension("lib.so"));
        assert!(!has_binary_extension("document.pdf"));
        assert!(!has_binary_extension("notes.txt"));
        assert!(!has_binary_extension("noextension"));
        assert!(!has_binary_extension(""));
        // rfind behavior: last dot wins, no dot -> false
        assert!(!has_binary_extension("a/b.c/d"));
        // hidden file with extension? ".hidden" -> ".hidden" not in set
        assert!(!has_binary_extension(".hidden"));
        assert!(has_binary_extension(".hidden.png"));
    }

    #[test]
    fn has_opaque_document_extension_matches_python() {
        assert!(has_opaque_document_extension("report.docx"));
        assert!(has_opaque_document_extension("report.DOCX"));
        assert!(has_opaque_document_extension("slides.pptx"));
        assert!(has_opaque_document_extension("book.epub"));
        assert!(has_opaque_document_extension("doc.rtf"));
        assert!(!has_opaque_document_extension("image.png"));
        assert!(!has_opaque_document_extension("doc.pdf"));
        assert!(!has_opaque_document_extension("noext"));
        assert!(!has_opaque_document_extension(""));
    }

    #[test]
    fn is_pdf_path_matches_python() {
        assert!(is_pdf_path("file.pdf"));
        assert!(is_pdf_path("FILE.PDF"));
        assert!(is_pdf_path("/tmp/Doc.PdF"));
        assert!(!is_pdf_path("file.pdf.bak"));
        assert!(!is_pdf_path("pdf"));
        assert!(!is_pdf_path(""));
        assert!(!is_pdf_path("file.docx"));
    }
}

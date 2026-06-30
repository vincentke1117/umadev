//! `repomap` — a dependency-light **repo-map / symbol graph** over the
//! *user's* code (NOT the knowledge corpus).
//!
//! ## Why this exists (Wave 3, Gap G4)
//! On a real repository UmaDev currently adds nothing over the bare base: the
//! base sees only the files it happens to open. To "understand the codebase"
//! (brownfield feature-add / "explain this code" / "navigate to X") the base
//! needs a compact, whole-repo *map* — a signature-level outline of every
//! top-level symbol (functions, types, exports) keyed by `path:line`. This
//! module produces that map within a caller-supplied **token budget**, ranked
//! by a lightweight importance heuristic, and **personalised** to the current
//! task via a `scope` of path hints (the files the router thinks are relevant).
//!
//! ## Design constraints (honoured deliberately)
//! - **Dependency-light.** No `tree-sitter`, no language server, no ICU tree.
//!   Symbols are extracted with **per-language `regex`** (already a workspace
//!   dep used by the equally dep-light `governance` / `contract` crates) plus
//!   hand-written line scanning. This is a *signature* scan, NOT a semantic
//!   parse: we capture the declaration line of each top-level symbol, not its
//!   body, types, or call graph. That is exactly enough for a map and keeps
//!   the build lean (the anti-rule against heavy parser trees).
//! - **Fail-open, never blocks.** Every error path (unreadable file, no source,
//!   empty repo, pathological input) returns an empty `String` / empty index —
//!   never an `Err`, never a panic. The map is an *enhancement*; a bug here
//!   must not break a run.
//! - **Bounded.** File count, per-file bytes, and total symbols are all capped
//!   so a vendored mega-file or a hostile repo can't blow up time or memory.
//! - **Cross-platform.** Rendered paths are always `/`-separated; the mtime
//!   cache key is derived from relative paths so it is stable per-machine.
//!
//! ## Public surface (the next batch wires these into firmware)
//! - [`repo_map`] — `(root, scope, budget_chars) -> String`: the rendered,
//!   token-budgeted signature outline, scope-personalised. The headline entry
//!   point.
//! - [`symbol_index`] — `(root) -> SymbolIndex`: the structured form (every
//!   file → its ranked symbols), for callers that want to retrieve / filter
//!   rather than render.
//! - [`Symbol`], [`SymbolKind`], [`FileSymbols`], [`SymbolIndex`] — the data.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

/// Source extensions we extract symbols from. A subset of the acceptance
/// crate's `SRC_EXT` (we only map *code*, not styles/markup), kept local so
/// `umadev-knowledge` doesn't depend on `umadev-agent`. Order is irrelevant.
const CODE_EXT: &[&str] = &[
    "ts", "tsx", "js", "jsx", "mjs", "cjs", // JS / TS family
    "py",  // Python
    "rs",  // Rust
    "go",  // Go
    "java", "kt",    // JVM
    "rb",    // Ruby
    "php",   // PHP
    "cs",    // C#
    "swift", // Swift
    "dart",  // Dart
];

/// Directories never worth scanning — build output / vendored deps / VCS /
/// UmaDev's own artifact dirs. Mirrors `acceptance::SKIP_DIRS` (kept local to
/// avoid the cross-crate dependency). Leading-dot dirs are skipped separately.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    ".git",
    "vendor",
    "__pycache__",
    ".pytest_cache",
    ".next",
    "out",
    "coverage",
    "output",
];

/// Filenames (stem, lowercased) that mark an **entry point** — symbols defined
/// in these files get an importance boost (they are the repo's public face).
const ENTRY_STEMS: &[&str] = &[
    "main", "index", "app", "lib", "mod", "server", "cli", "__init__", "__main__", "root",
];

/// Hard bounds (a hostile / vendored mega-repo must not blow up time/memory).
mod limits {
    /// Max directory recursion depth.
    pub const MAX_DEPTH: usize = 12;
    /// Max source files scanned.
    pub const MAX_FILES: usize = 4000;
    /// Max bytes read per file (a generated bundle can be megabytes — we only
    /// need the top declarations, and a giant minified line is useless anyway).
    pub const MAX_FILE_BYTES: usize = 512 * 1024;
    /// Max symbols kept per file (defends against a machine-generated file with
    /// tens of thousands of declarations).
    pub const MAX_SYMBOLS_PER_FILE: usize = 400;
    /// Max total symbols across the whole index.
    pub const MAX_TOTAL_SYMBOLS: usize = 40_000;
    /// A single physical line longer than this is treated as minified/generated
    /// and skipped for symbol matching (regex on a 2 MB line is pathological).
    pub const MAX_LINE_BYTES: usize = 2_000;
}

/// What a captured symbol *is*. Coarse on purpose — a signature scan can't
/// always tell a method from a free function, and the map doesn't need it to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    /// A function or method.
    Function,
    /// A class / struct / record.
    Class,
    /// An enum.
    Enum,
    /// An interface / trait / protocol.
    Interface,
    /// A type alias / typedef.
    TypeAlias,
    /// A top-level constant / static / exported variable binding.
    Const,
    /// Anything declaration-like we recognised but can't classify finer.
    Other,
}

impl SymbolKind {
    /// A short, stable, language-neutral label for rendering (`fn`, `class`, …).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            SymbolKind::Function => "fn",
            SymbolKind::Class => "class",
            SymbolKind::Enum => "enum",
            SymbolKind::Interface => "interface",
            SymbolKind::TypeAlias => "type",
            SymbolKind::Const => "const",
            SymbolKind::Other => "def",
        }
    }
}

/// One extracted top-level symbol: its name, kind, the verbatim declaration
/// line (trimmed), and its `1`-based line number within the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    /// Identifier (best-effort; the capture group from the language pattern).
    pub name: String,
    /// Coarse classification.
    pub kind: SymbolKind,
    /// The declaration line, trimmed — the "signature" we show in the outline.
    pub signature: String,
    /// 1-based line number of the declaration within its file.
    pub line: usize,
    /// Whether the declaration is exported / public (boosts importance).
    pub exported: bool,
}

/// All symbols extracted from one file, plus the file's importance score.
#[derive(Debug, Clone, PartialEq)]
pub struct FileSymbols {
    /// Path relative to the repo root, always `/`-separated (cross-platform).
    pub rel_path: String,
    /// Symbols in source order.
    pub symbols: Vec<Symbol>,
    /// Importance score for this file (see [`SymbolIndex::rank`]).
    pub score: f64,
}

/// The structured repo map: every scanned file with its symbols, ranked.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SymbolIndex {
    /// Per-file symbol sets, sorted by descending importance after ranking.
    pub files: Vec<FileSymbols>,
}

impl SymbolIndex {
    /// Total number of symbols across all files.
    #[must_use]
    pub fn symbol_count(&self) -> usize {
        self.files.iter().map(|f| f.symbols.len()).sum()
    }

    /// Whether the index found nothing (empty repo / no recognised source).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

// ---------------------------------------------------------------------------
// File walk (bounded, fail-open) — mirrors acceptance::source_files locally.
// ---------------------------------------------------------------------------

/// Recursively collect code files under `dir`, bounded by [`limits`].
fn collect(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > limits::MAX_DEPTH || out.len() >= limits::MAX_FILES {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return; // unreadable dir → skip (fail-open)
    };
    for e in rd.flatten() {
        if out.len() >= limits::MAX_FILES {
            return;
        }
        let p = e.path();
        // Use the dir entry's file_type when available to avoid an extra stat;
        // fall back to is_dir() which itself is fail-open (false on error).
        let is_dir = e
            .file_type()
            .map(|t| t.is_dir())
            .unwrap_or_else(|_| p.is_dir());
        if is_dir {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') || SKIP_DIRS.contains(&name) {
                continue;
            }
            collect(&p, out, depth + 1);
        } else if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
            // Extension match is case-insensitive (`.RS`, `.PY` on Windows).
            let ext_lc = ext.to_ascii_lowercase();
            if CODE_EXT.contains(&ext_lc.as_str()) {
                out.push(p);
            }
        }
    }
}

/// Collect the repo's code files (bounded; skips build/vendor/VCS dirs).
fn code_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect(root, &mut files, 0);
    // Deterministic order so the index / cache signature is stable.
    files.sort();
    files
}

/// Render a path relative to `root` as a `/`-separated string (cross-platform).
fn rel_display(path: &Path, root: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}

// ---------------------------------------------------------------------------
// Language patterns — the heart of the signature scan.
// ---------------------------------------------------------------------------

/// A compiled extraction rule: a regex whose capture group 1 is the symbol
/// name, plus the kind to assign on a match.
struct Pattern {
    re: Regex,
    kind: SymbolKind,
}

/// Which language family an extension belongs to (selects the pattern set).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Lang {
    JsTs,
    Python,
    Rust,
    Go,
    Java, // also Kotlin (close enough at signature level)
    Ruby,
    Php,
    CSharp,
    Swift,
    Dart,
}

impl Lang {
    fn from_ext(ext: &str) -> Option<Lang> {
        Some(match ext {
            "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => Lang::JsTs,
            "py" => Lang::Python,
            "rs" => Lang::Rust,
            "go" => Lang::Go,
            "java" | "kt" => Lang::Java,
            "rb" => Lang::Ruby,
            "php" => Lang::Php,
            "cs" => Lang::CSharp,
            "swift" => Lang::Swift,
            "dart" => Lang::Dart,
            _ => return None,
        })
    }
}

/// Build the (cached) pattern set for a language. Patterns are compiled once
/// per process via a `OnceLock` keyed by language. A compile failure for any
/// single pattern is skipped (fail-open) rather than panicking — so a typo in a
/// future pattern can never break the whole scan.
fn patterns_for(lang: Lang) -> &'static [Pattern] {
    static CACHE: OnceLock<HashMap<u8, Vec<Pattern>>> = OnceLock::new();
    let map = CACHE.get_or_init(build_all_patterns);
    map.get(&(lang as u8)).map_or(&[], Vec::as_slice)
}

/// Compile every language's pattern table once.
fn build_all_patterns() -> HashMap<u8, Vec<Pattern>> {
    use SymbolKind::{Class, Const, Enum, Function, Interface, Other, TypeAlias};
    // (pattern, kind) source — each `(&str, SymbolKind)`. Capture group 1 is
    // the identifier. Patterns are intentionally anchored to a leading optional
    // export/visibility keyword so they only match *declarations*, not calls.
    // We compile with `(?m)` so `^` means start-of-line within a multi-line
    // file; we still scan line-by-line for line numbers, so each pattern is
    // applied to a single trimmed line (the `(?m)` is harmless there).
    let mut out: HashMap<u8, Vec<Pattern>> = HashMap::new();

    let mut add = |lang: Lang, defs: &[(&str, SymbolKind)]| {
        let v: Vec<Pattern> = defs
            .iter()
            .filter_map(|(src, kind)| Regex::new(src).ok().map(|re| Pattern { re, kind: *kind }))
            .collect();
        out.insert(lang as u8, v);
    };

    // --- JavaScript / TypeScript -----------------------------------------
    // Handles: function decls, class/interface/enum/type, const arrow fns,
    // exported bindings. Identifier = [A-Za-z_$][\w$]* .
    add(
        Lang::JsTs,
        &[
            (
                r"^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s*\*?\s*([A-Za-z_$][\w$]*)",
                Function,
            ),
            (
                r"^\s*(?:export\s+)?(?:default\s+)?(?:abstract\s+)?class\s+([A-Za-z_$][\w$]*)",
                Class,
            ),
            (
                r"^\s*(?:export\s+)?(?:declare\s+)?interface\s+([A-Za-z_$][\w$]*)",
                Interface,
            ),
            (
                r"^\s*(?:export\s+)?(?:const\s+)?enum\s+([A-Za-z_$][\w$]*)",
                Enum,
            ),
            (
                r"^\s*(?:export\s+)?type\s+([A-Za-z_$][\w$]*)\s*[=<]",
                TypeAlias,
            ),
            // const/let/var arrow-function or function-expression binding.
            (
                r"^\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*(?::[^=]+)?=\s*(?:async\s*)?(?:\([^)]*\)|[A-Za-z_$][\w$]*)\s*=>",
                Function,
            ),
            // Plain exported const binding (config object, etc.).
            (
                r"^\s*export\s+(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*[:=]",
                Const,
            ),
        ],
    );

    // --- Python -----------------------------------------------------------
    add(
        Lang::Python,
        &[
            (r"^\s*(?:async\s+)?def\s+([A-Za-z_][\w]*)", Function),
            (r"^\s*class\s+([A-Za-z_][\w]*)", Class),
        ],
    );

    // --- Rust -------------------------------------------------------------
    add(
        Lang::Rust,
        &[
            (
                r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+(?:\x22[^\x22]*\x22\s+)?)?fn\s+([A-Za-z_][\w]*)",
                Function,
            ),
            (
                r"^\s*(?:pub(?:\([^)]*\))?\s+)?struct\s+([A-Za-z_][\w]*)",
                Class,
            ),
            (
                r"^\s*(?:pub(?:\([^)]*\))?\s+)?enum\s+([A-Za-z_][\w]*)",
                Enum,
            ),
            (
                r"^\s*(?:pub(?:\([^)]*\))?\s+)?trait\s+([A-Za-z_][\w]*)",
                Interface,
            ),
            (
                r"^\s*(?:pub(?:\([^)]*\))?\s+)?type\s+([A-Za-z_][\w]*)",
                TypeAlias,
            ),
            (
                r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const|static)\s+([A-Za-z_][\w]*)",
                Const,
            ),
        ],
    );

    // --- Go ---------------------------------------------------------------
    add(
        Lang::Go,
        &[
            (r"^\s*func\s+(?:\([^)]*\)\s*)?([A-Za-z_][\w]*)", Function),
            (r"^\s*type\s+([A-Za-z_][\w]*)\s+struct\b", Class),
            (r"^\s*type\s+([A-Za-z_][\w]*)\s+interface\b", Interface),
            (r"^\s*type\s+([A-Za-z_][\w]*)\b", TypeAlias),
        ],
    );

    // --- Java / Kotlin ----------------------------------------------------
    // Modifier prefix: zero-or-more visibility/qualifier keywords, each
    // followed by whitespace. The `\s+` lives OUTSIDE the alternation so every
    // branch consumes its trailing space (a `public|...|internal\s+` form would
    // only attach the space to the last branch and fail on `public class X`).
    add(
        Lang::Java,
        &[
            (
                r"^\s*(?:(?:public|private|protected|internal|static|final|abstract|sealed|open|data)\s+)*class\s+([A-Za-z_][\w]*)",
                Class,
            ),
            (
                r"^\s*(?:(?:public|private|protected|internal)\s+)*interface\s+([A-Za-z_][\w]*)",
                Interface,
            ),
            (
                r"^\s*(?:(?:public|private|protected|internal)\s+)*enum\s+([A-Za-z_][\w]*)",
                Enum,
            ),
            // Kotlin fun.
            (
                r"^\s*(?:(?:public|private|protected|internal|suspend|inline|override|open)\s+)*fun\s+([A-Za-z_][\w]*)",
                Function,
            ),
        ],
    );

    // --- Ruby -------------------------------------------------------------
    add(
        Lang::Ruby,
        &[
            (r"^\s*def\s+(?:self\.)?([A-Za-z_][\w]*[!?=]?)", Function),
            (r"^\s*class\s+([A-Za-z_][\w:]*)", Class),
            (r"^\s*module\s+([A-Za-z_][\w:]*)", Other),
        ],
    );

    // --- PHP --------------------------------------------------------------
    add(
        Lang::Php,
        &[
            (
                r"^\s*(?:public\s+|private\s+|protected\s+|static\s+|final\s+|abstract\s+)*function\s+([A-Za-z_][\w]*)",
                Function,
            ),
            (
                r"^\s*(?:abstract\s+|final\s+)?class\s+([A-Za-z_][\w]*)",
                Class,
            ),
            (r"^\s*interface\s+([A-Za-z_][\w]*)", Interface),
            (r"^\s*trait\s+([A-Za-z_][\w]*)", Interface),
            (r"^\s*enum\s+([A-Za-z_][\w]*)", Enum),
        ],
    );

    // --- C# ---------------------------------------------------------------
    add(
        Lang::CSharp,
        &[
            (
                r"^\s*(?:(?:public|private|protected|internal|static|sealed|abstract|partial)\s+)*class\s+([A-Za-z_][\w]*)",
                Class,
            ),
            (
                r"^\s*(?:(?:public|private|protected|internal)\s+)*interface\s+([A-Za-z_][\w]*)",
                Interface,
            ),
            (
                r"^\s*(?:(?:public|private|protected|internal)\s+)*enum\s+([A-Za-z_][\w]*)",
                Enum,
            ),
            (
                r"^\s*(?:(?:public|private|protected|internal)\s+)*struct\s+([A-Za-z_][\w]*)",
                Class,
            ),
        ],
    );

    // --- Swift ------------------------------------------------------------
    add(
        Lang::Swift,
        &[
            (
                r"^\s*(?:public\s+|private\s+|internal\s+|open\s+|fileprivate\s+|static\s+)*func\s+([A-Za-z_][\w]*)",
                Function,
            ),
            (
                r"^\s*(?:public\s+|private\s+|internal\s+|open\s+|final\s+)*class\s+([A-Za-z_][\w]*)",
                Class,
            ),
            (
                r"^\s*(?:public\s+|private\s+|internal\s+)?struct\s+([A-Za-z_][\w]*)",
                Class,
            ),
            (
                r"^\s*(?:public\s+|private\s+|internal\s+)?enum\s+([A-Za-z_][\w]*)",
                Enum,
            ),
            (
                r"^\s*(?:public\s+|private\s+|internal\s+)?protocol\s+([A-Za-z_][\w]*)",
                Interface,
            ),
        ],
    );

    // --- Dart -------------------------------------------------------------
    add(
        Lang::Dart,
        &[
            (r"^\s*(?:abstract\s+)?class\s+([A-Za-z_][\w]*)", Class),
            (r"^\s*enum\s+([A-Za-z_][\w]*)", Enum),
            (r"^\s*mixin\s+([A-Za-z_][\w]*)", Interface),
        ],
    );

    out
}

/// A loose heuristic for "this line declares something exported/public" — used
/// for the importance boost. Cheap substring checks (no regex): we already
/// matched a symbol, this only refines its score.
fn looks_exported(line: &str, lang: Lang) -> bool {
    let t = line.trim_start();
    match lang {
        Lang::JsTs => t.starts_with("export"),
        Lang::Rust => t.starts_with("pub"),
        Lang::Python => {
            // Public unless name starts with `_` (checked by caller on the name).
            true
        }
        Lang::Go => {
            // Exported Go identifiers start uppercase — checked on the name by
            // the caller; treat the line as a candidate here.
            true
        }
        Lang::Java | Lang::CSharp => t.starts_with("public"),
        Lang::Php => t.contains("public"),
        Lang::Swift => t.starts_with("public") || t.starts_with("open"),
        Lang::Ruby | Lang::Dart => true,
    }
}

/// Extract symbols from one file's text. `lang` selects the pattern set. The
/// scan is line-oriented (cheap, gives line numbers for free) and bounded by
/// [`limits::MAX_SYMBOLS_PER_FILE`].
fn extract_symbols(text: &str, lang: Lang) -> Vec<Symbol> {
    let pats = patterns_for(lang);
    if pats.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    // Multi-line comment / docstring state — a declaration-looking line INSIDE a
    // `/* ... */` block comment or a triple-quoted docstring is NOT a real symbol
    // and must not be captured as a phantom (LOW #6). Line-granular: cheap, and
    // enough to kill the common multi-line-block-comment / docstring false
    // positives (license headers, commented-out code, Python docstrings).
    let mut in_block = false;
    let mut doc_marker: Option<&'static str> = None;
    for (i, raw_line) in text.lines().enumerate() {
        if out.len() >= limits::MAX_SYMBOLS_PER_FILE {
            break;
        }
        // Skip absurdly long lines (minified / generated) — regex on them is
        // both useless and a perf hazard.
        if raw_line.len() > limits::MAX_LINE_BYTES {
            continue;
        }
        let line = raw_line.trim_end();
        let trimmed = line.trim_start();

        // --- comment / docstring state machine (skip symbol matching inside) ---
        if in_block {
            if trimmed.contains("*/") {
                in_block = false;
            }
            continue;
        }
        if let Some(marker) = doc_marker {
            if trimmed.contains(marker) {
                doc_marker = None;
            }
            continue;
        }
        // A block comment opened with a line beginning `/*` (covers `/*`, `/**`
        // JSDoc, `/* commented code`). A single-line `/* ... */` closes on the
        // same line, so it does not enter the multi-line state.
        if trimmed.starts_with("/*") {
            if !trimmed.contains("*/") {
                in_block = true;
            }
            continue; // a comment opener carries no symbol
        }
        // Python triple-quoted docstring (`"""` or `'''`). Enters the docstring
        // state unless the same line also closes it.
        if matches!(lang, Lang::Python)
            && (trimmed.starts_with("\"\"\"") || trimmed.starts_with("'''"))
        {
            let marker = if trimmed.starts_with("\"\"\"") {
                "\"\"\""
            } else {
                "'''"
            };
            if !trimmed[marker.len()..].contains(marker) {
                doc_marker = Some(marker);
            }
            continue;
        }

        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || trimmed.starts_with('#') && lang != Lang::Python && lang != Lang::Ruby
            || trimmed.starts_with('*')
        {
            // Comment / blank — Python & Ruby use `#` for comments too, but
            // `def`/`class` never start with `#`, so the pattern simply won't
            // match; we only fast-skip obvious noise lines.
            if !(lang == Lang::Python || lang == Lang::Ruby) {
                continue;
            }
        }
        for pat in pats {
            if let Some(caps) = pat.re.captures(line) {
                if let Some(name) = caps.get(1) {
                    let name = name.as_str().to_string();
                    let exported = compute_exported(&name, line, lang);
                    out.push(Symbol {
                        name,
                        kind: pat.kind,
                        signature: trimmed.to_string(),
                        line: i + 1,
                        exported,
                    });
                    break; // one symbol per line — first matching pattern wins
                }
            }
        }
    }
    out
}

/// Decide whether a matched symbol is exported, refining [`looks_exported`]
/// with name-based rules for languages where visibility is encoded in the name.
fn compute_exported(name: &str, line: &str, lang: Lang) -> bool {
    match lang {
        Lang::Python | Lang::Ruby => !name.starts_with('_'),
        Lang::Go => name.chars().next().is_some_and(char::is_uppercase),
        _ => looks_exported(line, lang),
    }
}

// ---------------------------------------------------------------------------
// Building the index (with mtime cache).
// ---------------------------------------------------------------------------

/// Build the structured [`SymbolIndex`] for `root` — scan every code file,
/// extract + rank symbols. Uses the on-disk mtime cache when valid (see
/// [`load_or_scan`]). Fail-open: an empty/unreadable repo yields an empty index.
#[must_use]
pub fn symbol_index(root: &Path) -> SymbolIndex {
    load_or_scan(root)
}

/// Scan from scratch (no cache) — exposed for tests and for the cache miss path.
fn scan(root: &Path) -> SymbolIndex {
    let files = code_files(root);
    let mut file_syms: Vec<FileSymbols> = Vec::new();
    let mut total = 0usize;
    for path in &files {
        if total >= limits::MAX_TOTAL_SYMBOLS {
            break;
        }
        let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(lang) = Lang::from_ext(&ext.to_ascii_lowercase()) else {
            continue;
        };
        // Bounded read: cap bytes so a giant generated file can't OOM us.
        let text = match read_bounded(path, limits::MAX_FILE_BYTES) {
            Some(t) => t,
            None => continue, // unreadable / non-UTF-8 → skip (fail-open)
        };
        let symbols = extract_symbols(&text, lang);
        if symbols.is_empty() {
            continue;
        }
        total += symbols.len();
        file_syms.push(FileSymbols {
            rel_path: rel_display(path, root),
            symbols,
            score: 0.0,
        });
    }
    let mut index = SymbolIndex { files: file_syms };
    rank(&mut index);
    index
}

/// Read up to `max` bytes of a file as UTF-8 (lossy), fail-open to `None`.
fn read_bounded(path: &Path, max: usize) -> Option<String> {
    use std::io::Read;
    let f = std::fs::File::open(path).ok()?;
    let mut buf = Vec::new();
    f.take(max as u64).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

// ---------------------------------------------------------------------------
// Ranking — lightweight importance heuristic (PageRank-style by reference).
// ---------------------------------------------------------------------------

/// Rank files (and reorder them by score, descending). The score blends:
/// - **entry-point bonus** — files named `main`/`index`/`lib`/… are the repo's
///   public face;
/// - **export ratio** — a file exporting many symbols is a module others use;
/// - **inbound references** — how many *other* files mention this file's symbol
///   names (a cheap degree-centrality proxy for PageRank: widely-referenced
///   symbols rank higher);
/// - a mild **shallowness** bonus (top-level files tend to be more central).
///
/// This is deliberately O(files × symbols) with a single reference pass — no
/// iterative eigenvector solve. It's a *heuristic* to order an outline, not a
/// precise call graph.
fn rank(index: &mut SymbolIndex) {
    if index.files.is_empty() {
        return;
    }

    // 1) Count, per exported symbol name, how many files declare it. Keys are
    //    OWNED `String`s so this map outlives the immutable borrow of
    //    `index.files` and doesn't conflict with the mutable scoring pass below.
    //    We only count *exported* names (private helpers pollute the count and
    //    aren't the symbols other modules depend on).
    let mut def_count: HashMap<String, usize> = HashMap::new();
    for f in &index.files {
        for s in &f.symbols {
            if s.exported && s.name.len() >= 3 {
                *def_count.entry(s.name.clone()).or_insert(0) += 1;
            }
        }
    }

    // 2) Inbound-reference proxy. We don't keep full file text, so a precise
    //    call graph is out of scope (and would need a heavy parser). Instead we
    //    use a cheap structural proxy: a symbol's importance ≈ the number of
    //    OTHER files that *also* declare a symbol of the same name — shared
    //    model names, interfaces implemented in many files, route handlers, etc.
    //    Deterministic and stable; a good-enough degree-centrality stand-in.
    let ref_count: HashMap<String, usize> = def_count
        .into_iter()
        .map(|(name, n)| (name, n.saturating_sub(1)))
        .collect();

    // 3) Score each file.
    for f in &mut index.files {
        let stem = file_stem_lc(&f.rel_path);
        let entry_bonus = if ENTRY_STEMS.contains(&stem.as_str()) {
            6.0
        } else {
            0.0
        };
        let depth = f.rel_path.matches('/').count() as f64;
        let shallow_bonus = (4.0 - depth).max(0.0); // shallower → more central

        let exported = f.symbols.iter().filter(|s| s.exported).count() as f64;
        let export_score = exported.min(20.0) * 0.5;

        let inbound: usize = f
            .symbols
            .iter()
            .filter_map(|s| ref_count.get(s.name.as_str()))
            .sum();
        let inbound_score = (inbound as f64).min(20.0) * 1.5;

        // A file with very many symbols gets a small bonus (it's a hub) but
        // capped so a generated file doesn't dominate.
        let size_score = (f.symbols.len() as f64).min(30.0) * 0.1;

        f.score = entry_bonus + shallow_bonus + export_score + inbound_score + size_score;

        // Sort symbols within the file: exported first, then by line order.
        f.symbols
            .sort_by(|a, b| b.exported.cmp(&a.exported).then(a.line.cmp(&b.line)));
    }

    // 4) Order files by descending score, then by path for stability.
    index.files.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.rel_path.cmp(&b.rel_path))
    });
}

/// Lower-cased file stem (`src/Main.rs` → `main`) for the entry-point check.
fn file_stem_lc(rel_path: &str) -> String {
    rel_path
        .rsplit('/')
        .next()
        .unwrap_or(rel_path)
        .rsplit_once('.')
        .map_or(rel_path, |(stem, _)| stem)
        .to_ascii_lowercase()
}

// ---------------------------------------------------------------------------
// Rendering — token-budgeted, scope-personalised signature outline.
// ---------------------------------------------------------------------------

/// Render the repo map: a `path:line`-keyed signature outline of `root`,
/// **personalised** by `scope` (path hints — files the current task touches
/// rank first) and capped at `budget_chars`.
///
/// `scope` entries are matched as **substrings** of each file's relative path
/// (so `"checkout"` matches `src/pages/checkout/Cart.tsx`); when any scope hint
/// is supplied, files matching it are emitted first (and never truncated away
/// before unrelated files). With an empty `scope`, the global importance order
/// from [`rank`] is used.
///
/// Fail-open: an empty repo / unreadable root / `budget_chars == 0` returns an
/// empty `String`.
#[must_use]
pub fn repo_map(root: &Path, scope: &[String], budget_chars: usize) -> String {
    if budget_chars == 0 {
        return String::new();
    }
    let index = symbol_index(root);
    render_map(&index, scope, budget_chars)
}

/// Render an already-built [`SymbolIndex`] (split out for testing without I/O).
fn render_map(index: &SymbolIndex, scope: &[String], budget_chars: usize) -> String {
    if index.is_empty() || budget_chars == 0 {
        return String::new();
    }

    // Personalisation: stable-partition files so scope-matching ones come
    // first, preserving the within-group importance order from `rank`.
    let scope_lc: Vec<String> = scope
        .iter()
        .map(|s| s.to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    let matches_scope = |rel: &str| -> bool {
        if scope_lc.is_empty() {
            return false;
        }
        let rel_lc = rel.to_lowercase();
        scope_lc.iter().any(|hint| rel_lc.contains(hint.as_str()))
    };

    let mut ordered: Vec<&FileSymbols> = index.files.iter().collect();
    if !scope_lc.is_empty() {
        ordered.sort_by_key(|f| u8::from(!matches_scope(&f.rel_path)));
    }

    let mut out = String::new();
    let header = "# Repo map (signature outline — path:line)\n";
    if header.len() < budget_chars {
        out.push_str(header);
    }

    'files: for f in ordered {
        // Build this file's block; only commit it if at least the header fits.
        let file_header = format!("\n{}\n", f.rel_path);
        if out.len() + file_header.len() > budget_chars {
            break;
        }
        out.push_str(&file_header);
        for s in &f.symbols {
            let line = format!(
                "  {} {}  ·{}\n",
                s.kind.label(),
                truncate_sig(&s.signature, 160),
                s.line
            );
            if out.len() + line.len() > budget_chars {
                // Budget exhausted — stop cleanly mid-file.
                break 'files;
            }
            out.push_str(&line);
        }
    }

    out
}

/// Truncate a signature line to `max` chars (char-boundary safe), adding an
/// ellipsis marker. Keeps the outline scannable and within budget.
fn truncate_sig(sig: &str, max: usize) -> String {
    if sig.chars().count() <= max {
        return sig.to_string();
    }
    let mut s: String = sig.chars().take(max.saturating_sub(1)).collect();
    s.push('…');
    s
}

// ---------------------------------------------------------------------------
// mtime cache — incremental: skip the full re-scan when nothing changed.
// ---------------------------------------------------------------------------

/// Cache directory for the repo map, relative to the project root.
pub const REPOMAP_CACHE_DIR: &str = ".umadev/repomap-cache";

/// Schema version of the repo-map cache. The signature keys on each file's
/// mtime+size, which captures CONTENT edits but NOT a change to the symbol-scan
/// regexes / `SymbolKind` set / cache encoding: after such an upgrade the cached
/// `symbols.json` is silently stale until a file happens to change. Bumping this
/// (folded into [`mtime_signature`]) invalidates every older-schema cache. Bump
/// it whenever the language patterns, ranking, or cache wire form change.
const REPOMAP_SCHEMA_VERSION: u32 = 1;

/// One in-process memo entry: the resolved index plus the cheap freshness keys.
struct MemoEntry {
    /// The resolved [`SymbolIndex`] for a root (scope-independent — scope is
    /// applied later in [`render_map`], so the index is the same across scopes).
    index: SymbolIndex,
    /// When this entry was last validated against a full scan. The memo is
    /// trusted only for [`MEMO_TTL`] after this instant — a fixed (not sliding)
    /// window, so rapid repeat calls can never extend trust past the TTL and
    /// mask a change indefinitely.
    validated_at: std::time::Instant,
    /// The root directory's OWN mtime at validation — a single cheap `stat`,
    /// NOT a recursive walk. A bump (a direct child added / removed / renamed)
    /// invalidates the memo immediately even inside the TTL. Captured AFTER the
    /// scan so the cache write under `.umadev/` is already accounted for.
    /// `None` when the root is unreadable (then the TTL alone governs).
    root_mtime: Option<std::time::SystemTime>,
}

/// How long an in-process memo entry is trusted before the next call re-verifies
/// with a full scan. Short by design: it only elides the walk on rapid
/// successive opens (e.g. several firmware composes within a turn), never masks a
/// real change for long. Kept below the repo-map cache test's ~1.1s edit gap so a
/// genuine edit always re-scans.
const MEMO_TTL: std::time::Duration = std::time::Duration::from_millis(800);

/// The process-wide repo-map memo, keyed by canonical root path. Fail-open: a
/// poisoned lock simply takes the full scan path.
fn memo_table() -> &'static std::sync::Mutex<HashMap<PathBuf, MemoEntry>> {
    static TABLE: OnceLock<std::sync::Mutex<HashMap<PathBuf, MemoEntry>>> = OnceLock::new();
    TABLE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// One cheap `stat` of the root directory's own mtime (NOT a recursive walk).
/// Fail-open to `None` on any error.
fn dir_mtime(root: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(root).ok()?.modified().ok()
}

/// Resolve the index for `root`, with a cheap in-process fast-path on top of the
/// authoritative on-disk-cached scan ([`load_or_scan_full`]).
///
/// A warm (re-)open in the SAME process within [`MEMO_TTL`] AND with the root
/// dir's own mtime unchanged returns the memoized index WITHOUT the full
/// recursive directory walk + per-file `metadata()` stat that `code_files` +
/// `mtime_signature` would cost on every call. Bounded staleness: once the TTL
/// elapses (or the root dir bumps) the next call falls through to the full scan,
/// so a real file change still refreshes. Fail-open throughout: a poisoned lock
/// or a missing entry just takes the full scan.
fn load_or_scan(root: &Path) -> SymbolIndex {
    let memo_key = root.to_path_buf();
    let now = std::time::Instant::now();

    // Fast-path: a still-fresh memo for this root short-circuits the walk+stat.
    // Valid iff inside the TTL AND the root dir's own mtime is unchanged.
    if let Ok(table) = memo_table().lock() {
        if let Some(entry) = table.get(&memo_key) {
            if entry.root_mtime == dir_mtime(root)
                && now.duration_since(entry.validated_at) < MEMO_TTL
            {
                return entry.index.clone();
            }
        }
    }

    // Slow path: the authoritative on-disk-cached full scan. A real change (the
    // TTL elapsed or the dir bumped) always lands here, so correctness holds.
    let index = load_or_scan_full(root);

    // Refresh the memo for the next rapid re-open. Capture the root mtime AFTER
    // the scan: writing the cache under `.umadev/` may itself bump it, so a
    // follow-up call's cheap re-stat must compare against the post-write value.
    // Fail-open: a poisoned lock just skips the memo (next call simply re-walks).
    if let Ok(mut table) = memo_table().lock() {
        table.insert(
            memo_key,
            MemoEntry {
                index: index.clone(),
                validated_at: now,
                root_mtime: dir_mtime(root),
            },
        );
    }
    index
}

/// Load the cached index if its mtime signature still matches the repo, else
/// scan fresh and refresh the cache. All cache I/O is fail-open: any error
/// (no dir, corrupt file, write failure) falls through to a live scan and never
/// surfaces an error.
fn load_or_scan_full(root: &Path) -> SymbolIndex {
    let files = code_files(root);
    if files.is_empty() {
        return SymbolIndex::default();
    }
    let signature = mtime_signature(&files, root);
    let cache_dir = root.join(REPOMAP_CACHE_DIR);
    let sig_path = cache_dir.join("signature.txt");
    let data_path = cache_dir.join("symbols.json");

    // Cache hit: signature matches AND the data deserialises.
    if let Ok(stored_sig) = std::fs::read_to_string(&sig_path) {
        if stored_sig == signature {
            if let Ok(bytes) = std::fs::read(&data_path) {
                if let Some(index) = decode_index(&bytes) {
                    return index;
                }
            }
        }
    }

    // Miss → scan and best-effort write the cache.
    let index = scan(root);
    if let Some(bytes) = encode_index(&index) {
        let _ = std::fs::create_dir_all(&cache_dir);
        let _ = std::fs::write(&data_path, bytes);
        let _ = std::fs::write(&sig_path, &signature);
    }
    index
}

/// Force the next [`symbol_index`] / [`repo_map`] call to re-scan by deleting
/// the cache signature. Fail-open (a missing cache is a no-op).
pub fn invalidate_cache(root: &Path) {
    let sig_path = root.join(REPOMAP_CACHE_DIR).join("signature.txt");
    let _ = std::fs::remove_file(sig_path);
    // Also drop the in-process fast-path memo, else the next call would return
    // the still-fresh memoized index instead of truly re-scanning. Fail-open: a
    // poisoned lock leaves the memo (the TTL still expires it shortly).
    if let Ok(mut table) = memo_table().lock() {
        table.remove(root);
    }
}

/// A deterministic mtime+size signature of the code files: one line per file,
/// `<rel_path>\t<mtime_nanos>\t<size>`, sorted, prefixed with the schema
/// version. Unlike the knowledge corpus signature (content hash), the repo map
/// is *local* working state, so cheap mtime+size is the right tradeoff (no need
/// to read every file to hash it). A read failure for a file omits it from the
/// signature (fail-open).
///
/// mtime is captured at NANOSECOND resolution, not seconds: a 1-second
/// granularity missed a same-second, same-length edit (rewrite a line to one of
/// equal byte length within the same second → identical signature → stale
/// `symbols.json`). Nanosecond mtime (which every modern filesystem tracks)
/// closes that window at no extra cost.
fn mtime_signature(files: &[PathBuf], root: &Path) -> String {
    let mut entries: Vec<String> = files
        .iter()
        .filter_map(|p| {
            let meta = std::fs::metadata(p).ok()?;
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0u128, |d| d.as_nanos());
            let rel = rel_display(p, root);
            Some(format!("{rel}\t{mtime}\t{}", meta.len()))
        })
        .collect();
    entries.sort();
    // Fold in the schema version so a scan-logic/cache-format upgrade
    // invalidates every older cache even when no file changed.
    let body = entries.join("\n");
    format!("schema=v{REPOMAP_SCHEMA_VERSION}\n{body}")
}

// --- Tiny self-contained (de)serialisation for the cache -------------------
// We avoid a serde derive on the public types to keep them dependency-free in
// shape; the cache format is a private, line-oriented encoding. A decode
// failure → cache miss (fail-open). This is NOT a stable public format.

/// Encode an index to the private cache wire form. Returns `None` on any
/// internal write error (never happens for `String`, but keeps the call site
/// uniformly fail-open).
fn encode_index(index: &SymbolIndex) -> Option<Vec<u8>> {
    use std::fmt::Write as _;
    let mut s = String::new();
    for f in &index.files {
        // F-line: file path + score.
        writeln!(s, "F\t{}\t{}", escape(&f.rel_path), f.score).ok()?;
        for sym in &f.symbols {
            // S-line: kind, exported, line, name, signature.
            writeln!(
                s,
                "S\t{}\t{}\t{}\t{}\t{}",
                kind_code(sym.kind),
                u8::from(sym.exported),
                sym.line,
                escape(&sym.name),
                escape(&sym.signature),
            )
            .ok()?;
        }
    }
    Some(s.into_bytes())
}

/// Decode the private cache wire form. Any malformed line → `None` (cache miss).
fn decode_index(bytes: &[u8]) -> Option<SymbolIndex> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut files: Vec<FileSymbols> = Vec::new();
    for line in text.lines() {
        let mut parts = line.split('\t');
        match parts.next()? {
            "F" => {
                let rel_path = unescape(parts.next()?);
                let score = parts.next()?.parse::<f64>().ok()?;
                files.push(FileSymbols {
                    rel_path,
                    symbols: Vec::new(),
                    score,
                });
            }
            "S" => {
                let f = files.last_mut()?;
                let kind = kind_from_code(parts.next()?)?;
                let exported = parts.next()? == "1";
                let lineno = parts.next()?.parse::<usize>().ok()?;
                let name = unescape(parts.next()?);
                let signature = unescape(parts.next()?);
                f.symbols.push(Symbol {
                    name,
                    kind,
                    signature,
                    line: lineno,
                    exported,
                });
            }
            _ => return None,
        }
    }
    Some(SymbolIndex { files })
}

/// Escape tab / newline / backslash so a field round-trips through the
/// tab-separated cache format.
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

/// Inverse of [`escape`].
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some('\\') => out.push('\\'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Single-char code for a [`SymbolKind`] in the cache format.
fn kind_code(k: SymbolKind) -> char {
    match k {
        SymbolKind::Function => 'f',
        SymbolKind::Class => 'c',
        SymbolKind::Enum => 'e',
        SymbolKind::Interface => 'i',
        SymbolKind::TypeAlias => 't',
        SymbolKind::Const => 'k',
        SymbolKind::Other => 'o',
    }
}

/// Inverse of [`kind_code`].
fn kind_from_code(s: &str) -> Option<SymbolKind> {
    Some(match s {
        "f" => SymbolKind::Function,
        "c" => SymbolKind::Class,
        "e" => SymbolKind::Enum,
        "i" => SymbolKind::Interface,
        "t" => SymbolKind::TypeAlias,
        "k" => SymbolKind::Const,
        "o" => SymbolKind::Other,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Helper: write a file under `root`, creating parent dirs.
    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    /// Collect all symbol names across the index (for assertions).
    fn names(index: &SymbolIndex) -> Vec<String> {
        index
            .files
            .iter()
            .flat_map(|f| f.symbols.iter().map(|s| s.name.clone()))
            .collect()
    }

    // --- (1) multi-language symbol extraction --------------------------------

    #[test]
    fn extract_typescript() {
        let syms = extract_symbols(
            "export function hello(a: string): number { return 1 }\n\
             export class Widget {}\n\
             interface Shape { area(): number }\n\
             export const enum Color { Red }\n\
             export type Id = string\n\
             export const make = (x: number) => x * 2\n\
             const helper = async () => 1\n",
            Lang::JsTs,
        );
        let got: Vec<(&str, SymbolKind)> = syms.iter().map(|s| (s.name.as_str(), s.kind)).collect();
        assert!(got.contains(&("hello", SymbolKind::Function)));
        assert!(got.contains(&("Widget", SymbolKind::Class)));
        assert!(got.contains(&("Shape", SymbolKind::Interface)));
        assert!(got.contains(&("Color", SymbolKind::Enum)));
        assert!(got.contains(&("Id", SymbolKind::TypeAlias)));
        assert!(got.contains(&("make", SymbolKind::Function)));
        assert!(got.contains(&("helper", SymbolKind::Function)));
        // export-ness is detected for the leading `export` lines.
        let hello = syms.iter().find(|s| s.name == "hello").unwrap();
        assert!(hello.exported);
        let helper = syms.iter().find(|s| s.name == "helper").unwrap();
        assert!(!helper.exported);
    }

    #[test]
    fn extract_python() {
        let syms = extract_symbols(
            "def public_fn(x):\n    pass\n\
             async def fetch(self):\n    pass\n\
             class Service:\n    pass\n\
             def _private():\n    pass\n",
            Lang::Python,
        );
        let n: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(n.contains(&"public_fn"));
        assert!(n.contains(&"fetch"));
        assert!(n.contains(&"Service"));
        assert!(n.contains(&"_private"));
        // Underscore prefix → not exported.
        assert!(!syms.iter().find(|s| s.name == "_private").unwrap().exported);
        assert!(
            syms.iter()
                .find(|s| s.name == "public_fn")
                .unwrap()
                .exported
        );
    }

    #[test]
    fn extract_rust() {
        let syms = extract_symbols(
            "pub fn run(arg: u32) -> Result<()> { Ok(()) }\n\
             pub(crate) async fn fetch() {}\n\
             pub struct Engine { x: u8 }\n\
             enum Mode { A, B }\n\
             pub trait Driver {}\n\
             pub type Alias = u32;\n\
             pub const MAX: usize = 9;\n\
             fn private_helper() {}\n",
            Lang::Rust,
        );
        let got: Vec<(&str, SymbolKind, bool)> = syms
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.exported))
            .collect();
        assert!(got.contains(&("run", SymbolKind::Function, true)));
        assert!(got.contains(&("fetch", SymbolKind::Function, true)));
        assert!(got.contains(&("Engine", SymbolKind::Class, true)));
        assert!(got.contains(&("Mode", SymbolKind::Enum, false)));
        assert!(got.contains(&("Driver", SymbolKind::Interface, true)));
        assert!(got.contains(&("Alias", SymbolKind::TypeAlias, true)));
        assert!(got.contains(&("MAX", SymbolKind::Const, true)));
        assert!(got.contains(&("private_helper", SymbolKind::Function, false)));
    }

    #[test]
    fn extract_go() {
        let syms = extract_symbols(
            "func PublicFn() {}\n\
             func (s *Server) Handle() {}\n\
             type User struct {}\n\
             type Repo interface {}\n\
             func privateFn() {}\n",
            Lang::Go,
        );
        let got: Vec<(&str, SymbolKind, bool)> = syms
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.exported))
            .collect();
        assert!(got.contains(&("PublicFn", SymbolKind::Function, true)));
        assert!(got.contains(&("Handle", SymbolKind::Function, true)));
        assert!(got.contains(&("User", SymbolKind::Class, true)));
        assert!(got.contains(&("Repo", SymbolKind::Interface, true)));
        // lowercase → unexported in Go.
        assert!(got.contains(&("privateFn", SymbolKind::Function, false)));
    }

    #[test]
    fn extract_java_kotlin_ruby_php_csharp_swift_dart() {
        // Java
        let j = extract_symbols(
            "public class Account {}\npublic interface Repo {}\npublic enum State {}\n",
            Lang::Java,
        );
        assert!(j
            .iter()
            .any(|s| s.name == "Account" && s.kind == SymbolKind::Class));
        assert!(j
            .iter()
            .any(|s| s.name == "Repo" && s.kind == SymbolKind::Interface));
        assert!(j
            .iter()
            .any(|s| s.name == "State" && s.kind == SymbolKind::Enum));
        // Kotlin fun shares the Java pattern set.
        let k = extract_symbols("suspend fun load() {}\n", Lang::Java);
        assert!(k
            .iter()
            .any(|s| s.name == "load" && s.kind == SymbolKind::Function));
        // Ruby
        let rb = extract_symbols(
            "def greet\nend\nclass Foo\nend\nmodule Bar\nend\n",
            Lang::Ruby,
        );
        assert!(rb
            .iter()
            .any(|s| s.name == "greet" && s.kind == SymbolKind::Function));
        assert!(rb
            .iter()
            .any(|s| s.name == "Foo" && s.kind == SymbolKind::Class));
        assert!(rb.iter().any(|s| s.name == "Bar"));
        // PHP
        let php = extract_symbols(
            "public function handle() {}\nclass Controller {}\ninterface Plug {}\n",
            Lang::Php,
        );
        assert!(php
            .iter()
            .any(|s| s.name == "handle" && s.kind == SymbolKind::Function));
        assert!(php
            .iter()
            .any(|s| s.name == "Controller" && s.kind == SymbolKind::Class));
        assert!(php
            .iter()
            .any(|s| s.name == "Plug" && s.kind == SymbolKind::Interface));
        // C#
        let cs = extract_symbols(
            "public class Svc {}\npublic interface IRepo {}\n",
            Lang::CSharp,
        );
        assert!(cs
            .iter()
            .any(|s| s.name == "Svc" && s.kind == SymbolKind::Class));
        assert!(cs
            .iter()
            .any(|s| s.name == "IRepo" && s.kind == SymbolKind::Interface));
        // Swift
        let sw = extract_symbols(
            "public func run() {}\nstruct Model {}\nprotocol P {}\n",
            Lang::Swift,
        );
        assert!(sw
            .iter()
            .any(|s| s.name == "run" && s.kind == SymbolKind::Function));
        assert!(sw
            .iter()
            .any(|s| s.name == "Model" && s.kind == SymbolKind::Class));
        assert!(sw
            .iter()
            .any(|s| s.name == "P" && s.kind == SymbolKind::Interface));
        // Dart
        let dt = extract_symbols(
            "class Page {}\nenum Status { ok }\nmixin Logger {}\n",
            Lang::Dart,
        );
        assert!(dt
            .iter()
            .any(|s| s.name == "Page" && s.kind == SymbolKind::Class));
        assert!(dt
            .iter()
            .any(|s| s.name == "Status" && s.kind == SymbolKind::Enum));
        assert!(dt
            .iter()
            .any(|s| s.name == "Logger" && s.kind == SymbolKind::Interface));
    }

    #[test]
    fn does_not_match_calls_or_comments() {
        // Function *calls* and comments must not be captured as declarations.
        let syms = extract_symbols(
            "// function notReal()\n\
             hello();\n\
             const x = foo();\n\
             /* class AlsoNotReal */\n",
            Lang::JsTs,
        );
        let n = names(&SymbolIndex {
            files: vec![FileSymbols {
                rel_path: "x.ts".into(),
                symbols: syms,
                score: 0.0,
            }],
        });
        assert!(!n.iter().any(|s| s == "notReal"));
        assert!(!n.iter().any(|s| s == "AlsoNotReal"));
        assert!(!n.iter().any(|s| s == "hello"));
    }

    #[test]
    fn multiline_block_comment_symbols_are_not_captured() {
        // LOW #6: declaration-looking lines INSIDE a multi-line `/* ... */` block
        // comment must NOT be captured as phantom symbols; the real symbol after
        // the comment must still be found.
        let syms = extract_symbols(
            "/*\nexport class CommentedOut {}\nfunction alsoCommented() {}\n*/\nexport class Real {}\n",
            Lang::JsTs,
        );
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"Real"),
            "the real symbol after the comment is captured"
        );
        assert!(
            !names.contains(&"CommentedOut"),
            "a class inside the block comment must not be captured: {names:?}"
        );
        assert!(
            !names.contains(&"alsoCommented"),
            "a fn inside the block comment must not be captured: {names:?}"
        );
    }

    #[test]
    fn python_docstring_symbols_are_not_captured() {
        // LOW #6: a `def`/`class` inside a triple-quoted docstring is documentation
        // text, not a symbol.
        let syms = extract_symbols(
            "def real_fn():\n    \"\"\"\n    def fake_in_doc():\n    class FakeInDoc:\n    \"\"\"\n    pass\nclass RealClass:\n    pass\n",
            Lang::Python,
        );
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"real_fn"),
            "the real def is captured: {names:?}"
        );
        assert!(
            names.contains(&"RealClass"),
            "the real class is captured: {names:?}"
        );
        assert!(
            !names.contains(&"fake_in_doc"),
            "a def inside the docstring must not be captured: {names:?}"
        );
        assert!(
            !names.contains(&"FakeInDoc"),
            "a class inside the docstring must not be captured: {names:?}"
        );
    }

    // --- (2) ranking ---------------------------------------------------------

    #[test]
    fn ranking_prefers_entry_points_and_shared_symbols() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        // Entry-point file at the root.
        write(
            root,
            "index.ts",
            "export function bootstrap() {}\nexport class App {}\n",
        );
        // A deeply-nested helper with a private fn — should rank below.
        write(
            root,
            "src/internal/deep/util.ts",
            "function localThing() {}\n",
        );
        // A shared model name declared in two files (boosts inbound proxy).
        write(root, "src/models/user.ts", "export class UserModel {}\n");
        write(
            root,
            "src/api/user.ts",
            "export class UserModel {}\nexport function getUser() {}\n",
        );

        let index = symbol_index(root);
        assert!(!index.is_empty());
        // The entry-point file should be ranked first (highest score).
        let top = &index.files[0];
        assert_eq!(top.rel_path, "index.ts", "entry point should rank first");
        // The deeply-nested private-only helper should rank last.
        let last = index.files.last().unwrap();
        assert_eq!(last.rel_path, "src/internal/deep/util.ts");
    }

    #[test]
    fn exported_symbols_sort_before_private_within_file() {
        let mut index = SymbolIndex {
            files: vec![FileSymbols {
                rel_path: "a.rs".into(),
                symbols: vec![
                    Symbol {
                        name: "priv_a".into(),
                        kind: SymbolKind::Function,
                        signature: "fn priv_a()".into(),
                        line: 1,
                        exported: false,
                    },
                    Symbol {
                        name: "pub_b".into(),
                        kind: SymbolKind::Function,
                        signature: "pub fn pub_b()".into(),
                        line: 2,
                        exported: true,
                    },
                ],
                score: 0.0,
            }],
        };
        rank(&mut index);
        assert_eq!(index.files[0].symbols[0].name, "pub_b");
        assert_eq!(index.files[0].symbols[1].name, "priv_a");
    }

    // --- (3) scope personalisation ------------------------------------------

    #[test]
    fn scope_hint_promotes_matching_files_first() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        // An entry point that would normally rank first.
        write(root, "index.ts", "export function main() {}\n");
        // The task-relevant file, deeper and not an entry point.
        write(
            root,
            "src/pages/checkout/Cart.tsx",
            "export function Cart() {}\nexport class CartItem {}\n",
        );

        // No scope → global order: entry point first.
        let global = repo_map(root, &[], 4000);
        let idx_pos_global = global.find("index.ts").unwrap();
        let cart_pos_global = global.find("Cart.tsx").unwrap();
        assert!(
            idx_pos_global < cart_pos_global,
            "without scope, entry point ranks first"
        );

        // Scope hint "checkout" → Cart.tsx must come first.
        let scoped = repo_map(root, &["checkout".to_string()], 4000);
        let cart_pos = scoped.find("Cart.tsx").unwrap();
        let idx_pos = scoped.find("index.ts").unwrap();
        assert!(
            cart_pos < idx_pos,
            "scope hint should promote the matching file"
        );
    }

    // --- (4) token-budget truncation ----------------------------------------

    #[test]
    fn budget_truncates_and_never_exceeds() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        // Many symbols across many files.
        for i in 0..40 {
            write(
                root,
                &format!("src/mod{i}.ts"),
                "export function alpha() {}\nexport function beta() {}\nexport class Gamma {}\n",
            );
        }
        let budget = 300;
        let map = repo_map(root, &[], budget);
        assert!(!map.is_empty());
        assert!(
            map.len() <= budget,
            "rendered map {} must fit budget {}",
            map.len(),
            budget
        );
        // Zero budget → empty.
        assert!(repo_map(root, &[], 0).is_empty());
    }

    #[test]
    fn scope_files_survive_truncation_over_unrelated() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(root, "src/auth/login.ts", "export function login() {}\n");
        for i in 0..30 {
            write(
                root,
                &format!("src/other/f{i}.ts"),
                "export function noise() {}\n",
            );
        }
        // Small budget: the scope-matching file should still appear because it
        // is rendered first.
        let map = repo_map(root, &["login".to_string()], 220);
        assert!(
            map.contains("login.ts"),
            "scope file must survive truncation: {map:?}"
        );
    }

    // --- (5) mtime incremental cache ----------------------------------------

    #[test]
    fn cache_hit_when_unchanged_and_refreshes_on_change() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(root, "a.rs", "pub fn one() {}\n");

        // First call populates the cache.
        let first = symbol_index(root);
        assert_eq!(first.symbol_count(), 1);
        let sig_path = root.join(REPOMAP_CACHE_DIR).join("signature.txt");
        assert!(sig_path.exists(), "cache signature should be written");
        let sig1 = fs::read_to_string(&sig_path).unwrap();

        // Second call: cache hit → identical signature on disk (not rewritten
        // with a different value), identical result.
        let second = symbol_index(root);
        assert_eq!(second.symbol_count(), 1);
        let sig2 = fs::read_to_string(&sig_path).unwrap();
        assert_eq!(sig1, sig2);
        assert_eq!(first, second);

        // Change a file (and bump its mtime explicitly to be robust on fast FS).
        std::thread::sleep(std::time::Duration::from_millis(1100));
        write(root, "a.rs", "pub fn one() {}\npub fn two() {}\n");
        let third = symbol_index(root);
        assert_eq!(third.symbol_count(), 2, "changed file should be re-scanned");
        let sig3 = fs::read_to_string(&sig_path).unwrap();
        assert_ne!(sig1, sig3, "signature should change after edit");
    }

    // --- (5b) in-process fast-path memo (Fix A2) ----------------------------

    #[test]
    fn fast_path_memo_short_circuits_full_scan_within_ttl() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(root, "a.rs", "pub fn one() {}\n");

        // First call: a full scan that populates BOTH the on-disk cache and the
        // in-process memo.
        let _ = symbol_index(root);
        let sig_path = root.join(REPOMAP_CACHE_DIR).join("signature.txt");
        assert!(sig_path.exists(), "first scan writes the on-disk signature");

        // Delete the on-disk signature DIRECTLY (not via `invalidate_cache`,
        // which also clears the in-process memo). Removing a file deep under
        // `.umadev/` does not bump the root dir's own mtime, so the memo stays
        // fresh.
        fs::remove_file(&sig_path).unwrap();

        // Immediate re-call (within MEMO_TTL): the in-process fast-path returns
        // the memoized index WITHOUT the full walk — so it never reaches the
        // on-disk scan and the signature is NOT rewritten. (A full re-walk would
        // miss the on-disk sig and rewrite it.) This observes "no re-walk".
        let _ = symbol_index(root);
        assert!(
            !sig_path.exists(),
            "fast-path memo must short-circuit the full scan within the TTL"
        );
    }

    #[test]
    fn fast_path_memo_rescans_after_ttl_on_change() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(root, "a.rs", "pub fn one() {}\n");
        assert_eq!(symbol_index(root).symbol_count(), 1);

        // Past the TTL the memo is stale, so the next call re-verifies with a
        // full scan and picks up the edit — correctness still holds.
        std::thread::sleep(MEMO_TTL + std::time::Duration::from_millis(300));
        write(root, "a.rs", "pub fn one() {}\npub fn two() {}\n");
        assert_eq!(
            symbol_index(root).symbol_count(),
            2,
            "a real change after the TTL is re-scanned"
        );
    }

    #[test]
    fn mtime_signature_carries_schema_version() {
        // A scan-logic/cache-format upgrade (bump REPOMAP_SCHEMA_VERSION) must
        // invalidate older caches even when no file changed: the version is
        // folded into the signature, so an old `.sig` can no longer match.
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(root, "a.rs", "pub fn one() {}\n");
        let files = code_files(root);
        let sig = mtime_signature(&files, root);
        assert!(
            sig.starts_with(&format!("schema=v{REPOMAP_SCHEMA_VERSION}")),
            "signature must be prefixed with the schema version: {sig}"
        );
    }

    #[test]
    fn invalidate_cache_forces_rescan() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(root, "a.rs", "pub fn one() {}\n");
        let _ = symbol_index(root);
        let sig_path = root.join(REPOMAP_CACHE_DIR).join("signature.txt");
        assert!(sig_path.exists());
        invalidate_cache(root);
        assert!(!sig_path.exists(), "invalidate removes the signature");
        // Re-scan repopulates it.
        let _ = symbol_index(root);
        assert!(sig_path.exists());
    }

    #[test]
    fn cache_roundtrip_preserves_index() {
        let index = SymbolIndex {
            files: vec![FileSymbols {
                rel_path: "src/x.ts".into(),
                symbols: vec![Symbol {
                    name: "Foo".into(),
                    kind: SymbolKind::Class,
                    signature: "export class Foo extends Bar<\tTab>".into(),
                    line: 12,
                    exported: true,
                }],
                score: 3.5,
            }],
        };
        let bytes = encode_index(&index).unwrap();
        let back = decode_index(&bytes).unwrap();
        assert_eq!(index, back);
    }

    #[test]
    fn decode_rejects_garbage_returns_none() {
        assert!(decode_index(b"not a valid cache line\n").is_none());
        assert!(decode_index(b"\xff\xfe\x00").is_none()); // invalid UTF-8
    }

    // --- (6) fail-open: empty / unreadable / pathological --------------------

    #[test]
    fn empty_repo_yields_empty() {
        let dir = tempdir().unwrap();
        assert!(symbol_index(dir.path()).is_empty());
        assert_eq!(repo_map(dir.path(), &[], 1000), "");
    }

    #[test]
    fn missing_root_does_not_panic() {
        let missing = Path::new("/nonexistent/umadev/repomap/path/xyz");
        assert!(symbol_index(missing).is_empty());
        assert_eq!(repo_map(missing, &["x".into()], 1000), "");
    }

    #[test]
    fn skips_vendor_and_build_dirs() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(root, "src/real.rs", "pub fn real() {}\n");
        write(
            root,
            "node_modules/dep/index.js",
            "export function dep() {}\n",
        );
        write(root, "target/gen.rs", "pub fn gen() {}\n");
        write(root, ".git/hook.py", "def hidden(): pass\n");
        let index = symbol_index(root);
        let n = names(&index);
        assert!(n.contains(&"real".to_string()));
        assert!(!n.contains(&"dep".to_string()), "node_modules skipped");
        assert!(!n.contains(&"gen".to_string()), "target skipped");
        assert!(!n.contains(&"hidden".to_string()), "dotdir skipped");
    }

    #[test]
    fn huge_minified_line_does_not_panic_or_hang() {
        // A single 5 MB line full of `function` keywords — must be skipped, and
        // certainly must not hang or panic.
        let mut giant = String::from("export function realOne() {}\n");
        giant.push_str(&"function x(){};".repeat(400_000)); // ~6 MB single line
        let syms = extract_symbols(&giant, Lang::JsTs);
        // Only the short first line should yield a symbol; the giant line is
        // over MAX_LINE_BYTES and skipped.
        assert_eq!(syms.iter().filter(|s| s.name == "realOne").count(), 1);
    }

    #[test]
    fn huge_file_byte_cap_is_respected() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        // Build a file that exceeds MAX_FILE_BYTES with many small functions.
        let mut body = String::new();
        // Each line ~24 bytes; need > 512 KiB → ~25k lines.
        for i in 0..30_000 {
            body.push_str(&format!("export function f{i}() {{}}\n"));
        }
        assert!(body.len() > limits::MAX_FILE_BYTES);
        write(root, "big.ts", &body);
        let index = symbol_index(root);
        // It scans only up to the byte cap; we just assert it didn't panic and
        // captured *some* but not all symbols.
        let count = index.symbol_count();
        assert!(count > 0);
        assert!(count < 30_000, "byte cap should truncate the read");
    }

    #[test]
    fn malformed_unicode_content_is_handled() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        // Write raw invalid-UTF-8 bytes mixed with a valid declaration.
        let mut bytes = b"pub fn valid() {}\n".to_vec();
        bytes.extend_from_slice(&[0xff, 0xfe, 0x00, 0x80]);
        bytes.extend_from_slice(b"\npub fn after() {}\n");
        fs::write(root.join("weird.rs"), &bytes).unwrap();
        // lossy decode → still extracts the valid lines, never panics.
        let index = symbol_index(root);
        let n = names(&index);
        assert!(n.contains(&"valid".to_string()));
        assert!(n.contains(&"after".to_string()));
    }

    // --- (7) cross-platform path rendering ----------------------------------

    #[test]
    fn rel_display_normalises_separators() {
        // Simulate a Windows-style absolute path under a root.
        let root = Path::new("C:\\proj");
        let file = Path::new("C:\\proj\\src\\app.ts");
        // On non-Windows, strip_prefix on these backslash paths won't match, so
        // we test the normalisation directly on a relative component instead.
        let rel = rel_display(file, root);
        // Whatever the platform, the output must never contain a backslash.
        assert!(
            !rel.contains('\\'),
            "rendered path must be /-separated: {rel}"
        );
    }

    #[test]
    fn rendered_map_uses_forward_slashes() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(root, "src/deep/nested/file.ts", "export function go() {}\n");
        let map = repo_map(root, &[], 4000);
        assert!(map.contains("src/deep/nested/file.ts"));
        assert!(!map.contains('\\'));
    }

    #[test]
    fn map_contains_path_line_and_signature() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(root, "lib.rs", "pub fn alpha(x: u8) -> u8 { x }\n");
        let map = repo_map(root, &[], 4000);
        assert!(map.contains("lib.rs"));
        assert!(map.contains("fn pub fn alpha(x: u8) -> u8 { x }") || map.contains("alpha"));
        // line marker present.
        assert!(map.contains("·1"));
    }
}

use std::collections::BTreeMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::LazyLock;

use crate::daylight_generated::daylight::common::Language as FbLanguage;
use tree_sitter_highlight::HighlightConfiguration;

macro_rules! language {
    ($name:ident, $fb_lang:expr, $ts_lang:expr, $lang_name:literal, $query:expr, $exts:expr) => {
        static $name: LazyLock<Config> =
            LazyLock::new(|| Config::new($fb_lang, $ts_lang.into(), $lang_name, $query, $exts));
    };
}

pub static ALL_HIGHLIGHT_NAMES: [&str; 26] = [
    "attribute",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "function",
    "function.builtin",
    "keyword",
    "module",
    "number",
    "operator",
    "property",
    "property.builtin",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
];

pub struct Config {
    pub fb_language: FbLanguage,
    pub ts_config: tree_sitter_highlight::HighlightConfiguration,
    pub name: &'static str,
    pub extensions: &'static [&'static str],
}

impl Config {
    fn new(
        fb_language: FbLanguage,
        ts_language: tree_sitter::Language,
        name: &'static str,
        highlights_query: &str,
        extensions: &'static [&'static str],
    ) -> Self {
        let mut ts_config =
            HighlightConfiguration::new(ts_language, name, highlights_query, "", "")
                .expect("Tree-sitter bindings are broken");
        ts_config.configure(&ALL_HIGHLIGHT_NAMES);
        Config {
            fb_language,
            ts_config,
            name,
            extensions,
        }
    }
}

language!(
    AGDA,
    FbLanguage::Agda,
    tree_sitter_agda::LANGUAGE,
    "agda",
    tree_sitter_agda::HIGHLIGHTS_QUERY,
    &["agda", "lagda"]
);
language!(
    BASH,
    FbLanguage::Bash,
    tree_sitter_bash::LANGUAGE,
    "bash",
    tree_sitter_bash::HIGHLIGHT_QUERY,
    &["sh", "bash"]
);
language!(
    C,
    FbLanguage::C,
    tree_sitter_c::LANGUAGE,
    "c",
    tree_sitter_c::HIGHLIGHT_QUERY,
    &["c", "h"]
);
language!(
    CPP,
    FbLanguage::Cpp,
    tree_sitter_cpp::LANGUAGE,
    "cpp",
    tree_sitter_cpp::HIGHLIGHT_QUERY,
    &["cpp", "cc", "cxx", "hpp", "hxx", "hh"]
);
language!(
    CSS,
    FbLanguage::Css,
    tree_sitter_css::LANGUAGE,
    "css",
    tree_sitter_css::HIGHLIGHTS_QUERY,
    &["css"]
);
language!(
    GO,
    FbLanguage::Go,
    tree_sitter_go::LANGUAGE,
    "go",
    tree_sitter_go::HIGHLIGHTS_QUERY,
    &["go"]
);
language!(
    HTML,
    FbLanguage::Html,
    tree_sitter_html::LANGUAGE,
    "html",
    tree_sitter_html::HIGHLIGHTS_QUERY,
    &["html", "htm"]
);
language!(
    JAVA,
    FbLanguage::Java,
    tree_sitter_java::LANGUAGE,
    "java",
    tree_sitter_java::HIGHLIGHTS_QUERY,
    &["java"]
);
language!(
    JAVASCRIPT,
    FbLanguage::JavaScript,
    tree_sitter_javascript::LANGUAGE,
    "javascript",
    tree_sitter_javascript::HIGHLIGHT_QUERY,
    &["js", "mjs", "cjs"]
);
language!(
    JSON,
    FbLanguage::Json,
    tree_sitter_json::LANGUAGE,
    "json",
    tree_sitter_json::HIGHLIGHTS_QUERY,
    &["json"]
);
language!(
    PYTHON,
    FbLanguage::Python,
    tree_sitter_python::LANGUAGE,
    "python",
    tree_sitter_python::HIGHLIGHTS_QUERY,
    &["py", "pyw"]
);
language!(
    RUBY,
    FbLanguage::Ruby,
    tree_sitter_ruby::LANGUAGE,
    "ruby",
    tree_sitter_ruby::HIGHLIGHTS_QUERY,
    &["rb"]
);
language!(
    RUST,
    FbLanguage::Rust,
    tree_sitter_rust::LANGUAGE,
    "rust",
    tree_sitter_rust::HIGHLIGHTS_QUERY,
    &["rs"]
);
language!(
    TYPESCRIPT,
    FbLanguage::TypeScript,
    tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
    "typescript",
    tree_sitter_typescript::HIGHLIGHTS_QUERY,
    &["ts"]
);
language!(
    TSX,
    FbLanguage::Tsx,
    tree_sitter_typescript::LANGUAGE_TSX,
    "tsx",
    tree_sitter_typescript::HIGHLIGHTS_QUERY,
    &["tsx"]
);

static EXTENSION_MAP: LazyLock<BTreeMap<&'static str, &'static Config>> = LazyLock::new(|| {
    let mut map = BTreeMap::new();
    for lang in all_languages() {
        for ext in lang.extensions {
            map.insert(*ext, lang);
        }
    }
    map
});

static NAME_MAP: LazyLock<BTreeMap<&'static str, &'static Config>> = LazyLock::new(|| {
    let mut map = BTreeMap::new();
    for lang in all_languages() {
        map.insert(lang.name, lang);
    }
    map
});

fn all_languages() -> impl Iterator<Item = &'static Config> {
    [
        &*AGDA,
        &*BASH,
        &*C,
        &*CPP,
        &*CSS,
        &*GO,
        &*HTML,
        &*JAVA,
        &*JAVASCRIPT,
        &*JSON,
        &*PYTHON,
        &*RUBY,
        &*RUST,
        &*TYPESCRIPT,
        &*TSX,
    ]
    .into_iter()
}

pub fn from_extension(extension: &str) -> Option<&'static Config> {
    EXTENSION_MAP.get(extension).copied()
}

pub fn from_name(name: &str) -> Option<&'static Config> {
    NAME_MAP.get(name).copied()
}

pub fn from_path(path: &Path) -> Option<&'static Config> {
    path.extension()
        .and_then(|ext| ext.to_str())
        .and_then(from_extension)
}

impl TryFrom<FbLanguage> for &'static Config {
    type Error = anyhow::Error;

    fn try_from(fb_lang: FbLanguage) -> Result<Self, Self::Error> {
        match fb_lang {
            FbLanguage::Agda => Ok(&*AGDA),
            FbLanguage::Bash => Ok(&*BASH),
            FbLanguage::C => Ok(&*C),
            FbLanguage::Cpp => Ok(&*CPP),
            FbLanguage::Css => Ok(&*CSS),
            FbLanguage::Go => Ok(&*GO),
            FbLanguage::Html => Ok(&*HTML),
            FbLanguage::Java => Ok(&*JAVA),
            FbLanguage::JavaScript => Ok(&*JAVASCRIPT),
            FbLanguage::Json => Ok(&*JSON),
            FbLanguage::Python => Ok(&*PYTHON),
            FbLanguage::Ruby => Ok(&*RUBY),
            FbLanguage::Rust => Ok(&*RUST),
            FbLanguage::TypeScript => Ok(&*TYPESCRIPT),
            FbLanguage::Tsx => Ok(&*TSX),
            FbLanguage::Unspecified => Err(anyhow::anyhow!(
                "Language::Unspecified cannot be converted to a Language"
            )),
            _ => Err(anyhow::anyhow!("Unknown language variant")),
        }
    }
}

impl FromStr for &'static Config {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        from_name(s).ok_or_else(|| format!("Unknown language: {}", s))
    }
}

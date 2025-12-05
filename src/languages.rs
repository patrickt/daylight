use std::collections::BTreeMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::LazyLock;

use crate::daylight_generated::daylight::common::Language as FbLanguage;
use tree_sitter_highlight::HighlightConfiguration;

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
        let mut ts_config = HighlightConfiguration::new(ts_language, name, highlights_query, "", "")
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

static AGDA: LazyLock<Config> = LazyLock::new(|| {
    Config::new(
        FbLanguage::Agda,
        tree_sitter_agda::LANGUAGE.into(),
        "agda",
        tree_sitter_agda::HIGHLIGHTS_QUERY,
        &["agda", "lagda"],
    )
});

static BASH: LazyLock<Config> = LazyLock::new(|| {
    Config::new(
        FbLanguage::Bash,
        tree_sitter_bash::LANGUAGE.into(),
        "bash",
        tree_sitter_bash::HIGHLIGHT_QUERY,
        &["sh", "bash"],
    )
});

static C: LazyLock<Config> = LazyLock::new(|| {
    Config::new(
        FbLanguage::C,
        tree_sitter_c::LANGUAGE.into(),
        "c",
        tree_sitter_c::HIGHLIGHT_QUERY,
        &["c", "h"],
    )
});

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
    [&*AGDA, &*BASH, &*C].into_iter()
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

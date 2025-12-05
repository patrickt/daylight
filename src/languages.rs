//

use std::collections::BTreeMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::LazyLock;

use crate::daylight_capnp;
use tree_sitter_highlight::HighlightConfiguration;

pub struct Language {
    pub capnp_language: daylight_capnp::Language,
    pub ts_config: tree_sitter_highlight::HighlightConfiguration,
    pub name: &'static str,
    pub extensions: &'static [&'static str],
}

impl Language {
    fn new(
        capnp_language: daylight_capnp::Language,
        ts_language: tree_sitter::Language,
        name: &'static str,
        highlights_query: &str,
        extensions: &'static [&'static str],
    ) -> Self {
        Language {
            capnp_language,
            ts_config: HighlightConfiguration::new(ts_language, name, highlights_query, "", "")
                .expect("Tree-sitter bindings are broken"),
            name,
            extensions,
        }
    }
}

static AGDA: LazyLock<Language> = LazyLock::new(|| {
    Language::new(
        daylight_capnp::Language::Agda,
        tree_sitter_agda::LANGUAGE.into(),
        "agda",
        tree_sitter_agda::HIGHLIGHTS_QUERY,
        &["agda", "lagda"],
    )
});

static BASH: LazyLock<Language> = LazyLock::new(|| {
    Language::new(
        daylight_capnp::Language::Bash,
        tree_sitter_bash::LANGUAGE.into(),
        "bash",
        tree_sitter_bash::HIGHLIGHT_QUERY,
        &["sh", "bash"],
    )
});

static C: LazyLock<Language> = LazyLock::new(|| {
    Language::new(
        daylight_capnp::Language::C,
        tree_sitter_c::LANGUAGE.into(),
        "c",
        tree_sitter_c::HIGHLIGHT_QUERY,
        &["c", "h"],
    )
});

static EXTENSION_MAP: LazyLock<BTreeMap<&'static str, &'static Language>> = LazyLock::new(|| {
    let mut map = BTreeMap::new();
    for lang in all_languages() {
        for ext in lang.extensions {
            map.insert(*ext, lang);
        }
    }
    map
});

static NAME_MAP: LazyLock<BTreeMap<&'static str, &'static Language>> = LazyLock::new(|| {
    let mut map = BTreeMap::new();
    for lang in all_languages() {
        map.insert(lang.name, lang);
    }
    map
});

fn all_languages() -> impl Iterator<Item = &'static Language> {
    [&*AGDA, &*BASH, &*C].into_iter()
}

pub fn from_extension(extension: &str) -> Option<&'static Language> {
    EXTENSION_MAP.get(extension).copied()
}

pub fn from_name(name: &str) -> Option<&'static Language> {
    NAME_MAP.get(name).copied()
}

pub fn from_path(path: &Path) -> Option<&'static Language> {
    path.extension()
        .and_then(|ext| ext.to_str())
        .and_then(from_extension)
}

impl TryInto<&'static Language> for daylight_capnp::Language {
    type Error = capnp::Error;

    fn try_into(self) -> Result<&'static Language, Self::Error> {
        match self {
            daylight_capnp::Language::Agda => Ok(&*AGDA),
            daylight_capnp::Language::Bash => Ok(&*BASH),
            daylight_capnp::Language::C => Ok(&*C),
            daylight_capnp::Language::Unspecified => Err(capnp::Error::failed(
                "Language::Unspecified cannot be converted to a Language".to_string(),
            )),
        }
    }
}

impl FromStr for &'static Language {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        from_name(s).ok_or_else(|| format!("Unknown language: {}", s))
    }
}

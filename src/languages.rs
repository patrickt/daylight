//

use std::sync::LazyLock;

use crate::daylight_capnp;
use tree_sitter_highlight::HighlightConfiguration;

pub struct Language {
    pub capnp_language: daylight_capnp::Language,
    pub ts_config: tree_sitter_highlight::HighlightConfiguration,
}

impl Language {
    fn new(
        capnp_language: daylight_capnp::Language,
        ts_language: tree_sitter::Language,
        name: &str,
        highlights_query: &str,
    ) -> Self {
        Language {
            capnp_language,
            ts_config: HighlightConfiguration::new(ts_language, name, highlights_query, "", "")
                .expect("Tree-sitter bindings are broken"),
        }
    }
}

static AGDA: LazyLock<Language> = LazyLock::new(|| {
    Language::new(
        daylight_capnp::Language::Agda,
        tree_sitter_agda::LANGUAGE.into(),
        "agda",
        tree_sitter_agda::HIGHLIGHTS_QUERY,
    )
});

static BASH: LazyLock<Language> = LazyLock::new(|| {
    Language::new(
        daylight_capnp::Language::Bash,
        tree_sitter_bash::LANGUAGE.into(),
        "bash",
        tree_sitter_bash::HIGHLIGHT_QUERY,
    )
});

static C: LazyLock<Language> = LazyLock::new(|| {
    Language::new(
        daylight_capnp::Language::C,
        tree_sitter_c::LANGUAGE.into(),
        "c",
        tree_sitter_c::HIGHLIGHT_QUERY,
    )
});

impl TryInto<&'static Language> for daylight_capnp::Language {
    type Error = capnp::Error;

    fn try_into(self) -> Result<&'static Language, Self::Error> {
        match self {
            daylight_capnp::Language::Agda => Ok(&*AGDA),
            daylight_capnp::Language::Bash => Ok(&*BASH),
            daylight_capnp::Language::C => Ok(&*C),
            _ => Err(capnp::Error::failed(format!(
                "Unsupported language: {:?}",
                self
            ))),
        }
    }
}

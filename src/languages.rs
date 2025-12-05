//

use crate::daylight_capnp;

pub struct Language {
    pub capnp_language: daylight_capnp::Language,
    pub ts_config: tree_sitter_highlight::HighlightConfiguration,
}

impl TryInto<&'static Language> for daylight_capnp::Language {
    type Error = capnp::Error;

    fn try_into(self) -> Result<&'static Language, Self::Error> {
        todo!()
    }
}

use std::cell::RefCell;
use tracing::instrument;
use tree_sitter_highlight as ts;

thread_local! {
    // Has to be a RefCell because we need &muts for the tree-sitter
    static HIGHLIGHTER: RefCell<ts::Highlighter> = RefCell::default();
    static HTML_RENDERER: RefCell<ts::HtmlRenderer> = RefCell::default();
    static RESPONSE_BUILDER: RefCell<flatbuffers::FlatBufferBuilder<'static>> = RefCell::default();
}

/// State stored by highlighting tasks spawned with tokio::spawn_blocking.
pub struct ThreadState;

impl ThreadState {
    #[instrument(skip(func))]
    pub fn with_highlighter<T, F>(func: F) -> T
    where
        F: FnOnce(&mut ts::Highlighter) -> T,
    {
        HIGHLIGHTER.with_borrow_mut(func)
    }

    #[instrument(skip(func))]
    pub fn render_with_tree_sitter<T, F>(func: F) -> T
    where
        F: FnOnce(&mut ts::HtmlRenderer) -> T,
    {
        HTML_RENDERER.with_borrow_mut(func)
    }

    #[instrument(skip(func))]
    pub fn build_flatbuffers<T, F>(func: F) -> T
    where
        F: FnOnce(&mut flatbuffers::FlatBufferBuilder) -> T,
    {
        RESPONSE_BUILDER.with_borrow_mut(func)
    }
}

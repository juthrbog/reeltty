//! Turns InnerTube JSON into the model types.

use serde_json::Value;

use crate::source::Result;
use crate::source::model::{Item, SearchPage};

/// The first page of a `search` response.
pub fn search_page(response: &Value) -> Result<SearchPage> {
    let _ = response;
    todo!("parse::search_page")
}

/// A `search` continuation response.
pub fn search_continuation(response: &Value) -> Result<SearchPage> {
    let _ = response;
    todo!("parse::search_continuation")
}

/// One item object, such as `{"videoRenderer": {...}}`. Containers (shelves)
/// give several items. Unknown keys give none.
pub fn items(item: &Value) -> Vec<Item> {
    let _ = item;
    todo!("parse::items")
}

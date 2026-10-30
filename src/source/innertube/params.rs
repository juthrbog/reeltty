//! The search `params` value: base64 of a small protobuf message.

use crate::source::filters::SearchFilters;

/// The `params` value for these filters. `None` when all filters are the default.
pub fn encode(filters: &SearchFilters) -> Option<String> {
    let _ = filters;
    todo!("params::encode")
}

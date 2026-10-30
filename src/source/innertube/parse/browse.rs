//! Channel and playlist pages from `browse`.

use serde_json::Value;

use crate::source::Result;
use crate::source::model::{ChannelPage, ItemPage, PlaylistPage};

pub fn channel_page(response: &Value) -> Result<ChannelPage> {
    let _ = response;
    todo!("browse::channel_page")
}

pub fn playlist_page(response: &Value) -> Result<PlaylistPage> {
    let _ = response;
    todo!("browse::playlist_page")
}

pub fn continuation(response: &Value) -> Result<ItemPage> {
    let _ = response;
    todo!("browse::continuation")
}

//! Video details from `next` and `player`.

use serde_json::Value;

use crate::source::Result;
use crate::source::model::VideoDetails;

pub fn details(next: &Value, player: Option<&Value>) -> Result<VideoDetails> {
    let _ = (next, player);
    todo!("video::details")
}

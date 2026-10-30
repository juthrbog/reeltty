//! The InnerTube source: the private API that the YouTube website uses, with
//! the WEB client context. It needs no login and no PO token for search.

pub mod client;
pub mod params;
pub mod parse;

pub use client::InnerTube;

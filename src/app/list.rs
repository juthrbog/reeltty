//! A list of items that loads more pages as the selection nears its end.

use super::Selection;
use crate::keymap::Action;
use crate::source::model::{Item, ItemPage};

/// Load the next page when the selection is this close to the end.
pub const PRELOAD_ROWS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Loading {
    #[default]
    Idle,
    FirstPage,
    MorePages,
}

#[derive(Debug, Clone, Default)]
pub struct ItemList {
    pub items: Vec<Item>,
    pub selection: Selection,
    pub continuation: Option<String>,
    pub loading: Loading,
    pub error: Option<String>,
}

impl ItemList {
    /// An empty list that waits for its first page.
    pub fn loading() -> Self {
        Self { loading: Loading::FirstPage, ..Self::default() }
    }

    /// Moves the selection. Returns `Some(token)` when the next page should load.
    /// Returns `None` for actions that are not navigation, and does nothing then.
    pub fn navigate(&mut self, action: Action, page: usize) -> Option<Option<String>> {
        if !self.selection.apply(action, self.items.len(), page) {
            return None;
        }
        Some(self.load_more())
    }

    /// The token of the next page, if it should load now. Marks it as loading.
    pub fn load_more(&mut self) -> Option<String> {
        if self.loading != Loading::Idle || self.selection.index + PRELOAD_ROWS < self.items.len() {
            return None;
        }
        let token = self.continuation.clone()?;
        self.loading = Loading::MorePages;
        self.error = None;
        Some(token)
    }

    /// Adds a page. Returns the token of the next page if it should load at once.
    pub fn add_page(&mut self, page: ItemPage, rows: usize) -> Option<String> {
        self.loading = Loading::Idle;
        self.error = None;
        self.items.extend(page.items);
        self.continuation = page.continuation;
        self.selection.clamp(self.items.len(), rows);
        self.load_more()
    }

    pub fn fail(&mut self, error: String) {
        self.loading = Loading::Idle;
        self.error = Some(error);
    }

    pub fn selected(&self) -> Option<&Item> {
        self.items.get(self.selection.index)
    }
}

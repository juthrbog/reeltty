//! The comments of a video: a list of comments and their open replies, that
//! loads more pages as the selection nears its end.

use std::cell::Cell;
use std::collections::HashSet;

use super::list::{Loading, PRELOAD_ROWS};
use super::screens::step_link;
use crate::keymap::Action;
use crate::source::model::{Comment, CommentPage, CommentSort};

/// Where a page of comments goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentsSlot {
    /// The first page. It replaces all rows.
    First,
    /// The next page of comments.
    More,
    /// A page of replies to the comment with this ID.
    Replies { parent: String },
}

/// What to load a page of comments from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentsFrom {
    /// The first page of this video, when no token is known.
    Video(String),
    Token(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentRow {
    /// A comment or a reply. `expanded`: its replies show below it.
    Comment { comment: Box<Comment>, expanded: bool },
    /// The button for the next page of replies to the comment `parent`.
    MoreReplies { parent: String, continuation: String },
}

impl CommentRow {
    /// The ID of the comment that this row belongs to: the comment itself,
    /// or the parent of a reply or a button.
    fn thread(&self) -> &str {
        match self {
            Self::Comment { comment, .. } => {
                comment.id.split_once('.').map_or(comment.id.as_str(), |(parent, _)| parent)
            }
            Self::MoreReplies { parent, .. } => parent,
        }
    }

    fn is_reply(&self) -> bool {
        match self {
            Self::Comment { comment, .. } => comment.reply_level > 0,
            Self::MoreReplies { .. } => true,
        }
    }
}

/// A page to load, as the app asks for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentsLoad {
    pub slot: CommentsSlot,
    pub from: CommentsFrom,
}

#[derive(Debug, Clone, Default)]
pub struct CommentsState {
    pub rows: Vec<CommentRow>,
    pub selected: usize,
    /// The selected link in the text of the selected comment, for `enter`.
    pub link: Option<usize>,
    /// The first visible row. Drawing sets it, because only drawing knows
    /// how high each row is.
    pub top: Cell<usize>,
    /// The token for the next page of comments.
    pub continuation: Option<String>,
    pub loading: Loading,
    /// The comment whose replies load now.
    pub loading_replies: Option<String>,
    pub error: Option<String>,
    /// "10,645,363 Comments".
    pub count_text: Option<String>,
    pub sorts: Vec<CommentSort>,
    /// Why there are no comments.
    pub note: Option<String>,
    /// Goes up with each first page, so that pages for an older list are dropped.
    pub generation: u64,
    /// The comments shown already. YouTube can send a comment again, such as
    /// the pinned one in "Newest", or a whole page.
    seen: HashSet<String>,
}

impl CommentsState {
    /// An empty list that waits for its first page.
    pub fn loading(generation: u64) -> Self {
        Self { loading: Loading::FirstPage, generation, ..Self::default() }
    }

    pub fn selected_row(&self) -> Option<&CommentRow> {
        self.rows.get(self.selected)
    }

    /// The selected comment, if the selected row is one.
    pub fn selected_comment(&self) -> Option<&Comment> {
        match self.selected_row()? {
            CommentRow::Comment { comment, .. } => Some(comment),
            CommentRow::MoreReplies { .. } => None,
        }
    }

    /// Selects the next or the previous link in the selected comment.
    pub fn step_link(&mut self, forward: bool) {
        let len = self.selected_comment().map_or(0, |c| c.text.links.len());
        self.link = step_link(self.link, len, forward);
    }

    /// The order that shows now.
    pub fn sort(&self) -> Option<&CommentSort> {
        self.sorts.iter().find(|s| s.selected)
    }

    /// Adds a page. Returns the next page to load at once, if any.
    pub fn add_page(&mut self, slot: CommentsSlot, page: CommentPage) -> Option<CommentsLoad> {
        self.error = None;
        match slot {
            CommentsSlot::First => {
                self.loading = Loading::Idle;
                self.rows.clear();
                self.seen.clear();
                self.selected = 0;
                self.link = None;
                self.top.set(0);
                self.count_text = page.count_text.or(self.count_text.take());
                if !page.sorts.is_empty() {
                    self.sorts = page.sorts;
                }
                self.note = page.note;
                let rows = self.fresh(page.comments);
                // A page with no comments to show ends the list, so that
                // empty pages cannot load one after another by themselves.
                self.continuation = page.continuation.filter(|_| !rows.is_empty());
                self.rows.extend(rows);
            }
            CommentsSlot::More => {
                self.loading = Loading::Idle;
                let rows = self.fresh(page.comments);
                // A page of repeats only, or an empty one, ends the list:
                // YouTube can repeat pages forever.
                self.continuation = page.continuation.filter(|_| !rows.is_empty());
                self.rows.extend(rows);
            }
            CommentsSlot::Replies { parent } => {
                self.loading_replies = None;
                self.add_replies(&parent, page);
                return None;
            }
        }
        self.load_more()
    }

    /// Comment rows for the comments not seen yet.
    fn fresh(&mut self, comments: Vec<Comment>) -> Vec<CommentRow> {
        comments
            .into_iter()
            .filter(|c| self.seen.insert(c.id.clone()))
            .map(|comment| CommentRow::Comment { comment: Box::new(comment), expanded: false })
            .collect()
    }

    /// Puts a page of replies after the replies that show already, in place
    /// of the button that loaded it.
    fn add_replies(&mut self, parent: &str, page: CommentPage) {
        let Some(at) = self.comment_index(parent) else { return };
        if let CommentRow::Comment { expanded, .. } = &mut self.rows[at] {
            *expanded = true;
        }
        let mut end = self.thread_end(at);
        // The selection on the button moves to the first new reply.
        let mut on_button = false;
        if end > at + 1 && matches!(self.rows[end - 1], CommentRow::MoreReplies { .. }) {
            end -= 1;
            self.rows.remove(end);
            on_button = self.selected == end;
            if self.selected > end {
                self.selected -= 1;
            }
        }
        let mut new = self.fresh(page.comments);
        if let Some(continuation) = page.continuation {
            new.push(CommentRow::MoreReplies { parent: parent.to_string(), continuation });
        }
        let added = new.len();
        self.rows.splice(end..end, new);
        if self.selected >= end && !on_button {
            self.selected += added;
        }
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    fn comment_index(&self, id: &str) -> Option<usize> {
        self.rows.iter().position(|r| matches!(r, CommentRow::Comment { comment, .. } if comment.id == id))
    }

    /// The index after the last reply row of the comment at `at`.
    fn thread_end(&self, at: usize) -> usize {
        let id = self.rows[at].thread().to_string();
        at + 1 + self.rows[at + 1..].iter().take_while(|r| r.is_reply() && r.thread() == id).count()
    }

    /// The next page of comments, if it should load now. Marks it as loading.
    pub fn load_more(&mut self) -> Option<CommentsLoad> {
        if self.loading != Loading::Idle || self.selected + PRELOAD_ROWS < self.rows.len() {
            return None;
        }
        let token = self.continuation.clone()?;
        self.loading = Loading::MorePages;
        self.error = None;
        Some(CommentsLoad { slot: CommentsSlot::More, from: CommentsFrom::Token(token) })
    }

    pub fn fail(&mut self, slot: &CommentsSlot, error: String) {
        match slot {
            CommentsSlot::Replies { .. } => self.loading_replies = None,
            CommentsSlot::First | CommentsSlot::More => self.loading = Loading::Idle,
        }
        self.error = Some(error);
    }

    /// Moves the selection. `page` is about how many rows fit on screen.
    /// Returns `None` for actions that are not navigation, and `Some` with
    /// the next page to load, if any.
    pub fn navigate(&mut self, action: Action, page: usize) -> Option<Option<CommentsLoad>> {
        let last = self.rows.len().saturating_sub(1);
        let half = (page / 2).max(1);
        let before = self.selected;
        self.selected = match action {
            Action::Up => self.selected.saturating_sub(1),
            Action::Down => (self.selected + 1).min(last),
            Action::Top => 0,
            Action::Bottom => last,
            Action::HalfPageUp => self.selected.saturating_sub(half),
            Action::HalfPageDown => (self.selected + half).min(last),
            _ => return None,
        };
        if self.selected != before {
            self.link = None;
        }
        Some(self.load_more())
    }

    /// `Open` on the selected row: shows or hides the replies of a comment,
    /// or loads more replies. Returns the page to load, if any.
    pub fn open(&mut self) -> Option<CommentsLoad> {
        if self.loading_replies.is_some() {
            return None;
        }
        let at = self.selected;
        let (parent, token) = match self.rows.get(at)? {
            CommentRow::Comment { expanded: true, .. } => {
                self.collapse(at);
                return None;
            }
            CommentRow::Comment { comment, expanded: false } => (comment.id.clone(), comment.replies.clone()?),
            CommentRow::MoreReplies { parent, continuation } => (parent.clone(), continuation.clone()),
        };
        self.loading_replies = Some(parent.clone());
        self.error = None;
        Some(CommentsLoad { slot: CommentsSlot::Replies { parent }, from: CommentsFrom::Token(token) })
    }

    /// Hides the replies of the comment at `at`. They load again on the next `Open`.
    fn collapse(&mut self, at: usize) {
        self.link = None;
        let end = self.thread_end(at);
        for row in self.rows.drain(at + 1..end) {
            if let CommentRow::Comment { comment, .. } = row {
                self.seen.remove(&comment.id);
            }
        }
        if let CommentRow::Comment { expanded, .. } = &mut self.rows[at] {
            *expanded = false;
        }
    }

    /// The token of the other order, for the `Sort` key: the one after the
    /// selected one.
    pub fn next_sort(&self) -> Option<&CommentSort> {
        let i = self.sorts.iter().position(|s| s.selected)?;
        self.sorts.get((i + 1) % self.sorts.len()).filter(|s| !s.selected)
    }

    /// Starts over with a new first page, keeping the orders and the count.
    pub fn restart(&mut self, generation: u64, sort: Option<&str>) {
        if let Some(label) = sort {
            for s in &mut self.sorts {
                s.selected = s.label == label;
            }
        }
        self.generation = generation;
        self.loading = Loading::FirstPage;
        self.loading_replies = None;
        self.error = None;
        self.note = None;
        self.continuation = None;
        self.rows.clear();
        self.seen.clear();
        self.selected = 0;
        self.link = None;
        self.top.set(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::model::{ChannelRef, Description};

    pub fn comment(id: &str, replies: Option<&str>) -> Comment {
        Comment {
            id: id.into(),
            author: ChannelRef { id: None, name: "@a".into(), handle: None },
            text: Description { text: format!("text {id}"), links: Vec::new() },
            published_text: None,
            edited: false,
            likes_text: None,
            reply_count_text: None,
            replies: replies.map(str::to_string),
            pinned: false,
            hearted: false,
            by_uploader: false,
            verified: false,
            reply_level: u8::from(id.contains('.')),
        }
    }

    fn page(ids: &[&str], continuation: Option<&str>) -> CommentPage {
        CommentPage {
            comments: ids.iter().map(|id| comment(id, Some(&format!("r-{id}")))).collect(),
            continuation: continuation.map(str::to_string),
            ..CommentPage::default()
        }
    }

    fn ids(s: &CommentsState) -> Vec<String> {
        s.rows
            .iter()
            .map(|r| match r {
                CommentRow::Comment { comment, .. } => comment.id.clone(),
                CommentRow::MoreReplies { continuation, .. } => format!("more:{continuation}"),
            })
            .collect()
    }

    fn token(load: Option<CommentsLoad>) -> Option<String> {
        load.map(|l| match l.from {
            CommentsFrom::Token(t) => t,
            CommentsFrom::Video(v) => v,
        })
    }

    #[test]
    fn pages_near_the_end_and_drops_repeats() {
        let mut s = CommentsState::loading(1);
        let ten: Vec<String> = (0..10).map(|i| format!("c{i}")).collect();
        let ten: Vec<&str> = ten.iter().map(String::as_str).collect();
        assert_eq!(s.add_page(CommentsSlot::First, page(&ten, Some("p2"))), None);
        // Five rows before the end, the next page loads.
        for _ in 0..4 {
            assert_eq!(s.navigate(Action::Down, 6), Some(None));
        }
        assert_eq!(token(s.navigate(Action::Down, 6).unwrap()).as_deref(), Some("p2"));
        assert_eq!(s.loading, Loading::MorePages);
        // Only once while it loads.
        assert_eq!(s.navigate(Action::Down, 6), Some(None));
        // The pinned comment again, and new ones.
        s.add_page(CommentsSlot::More, page(&["c0", "n1"], Some("p3")));
        assert_eq!(ids(&s).len(), 11);
        // A page of repeats only ends the list.
        s.loading = Loading::MorePages;
        s.add_page(CommentsSlot::More, page(&["c1", "n1"], Some("p4")));
        assert_eq!(s.continuation, None);
        assert_eq!(s.navigate(Action::Bottom, 6), Some(None));
        // An empty page with a token ends the list too, and loads nothing by itself.
        let mut s = CommentsState::loading(1);
        assert_eq!(s.add_page(CommentsSlot::First, page(&[], Some("p2"))), None);
        let mut s = CommentsState::loading(1);
        s.add_page(CommentsSlot::First, page(&["a"], Some("p2")));
        s.loading = Loading::MorePages;
        assert_eq!(s.add_page(CommentsSlot::More, page(&[], Some("p3"))), None);
        assert_eq!(s.continuation, None);
        assert_eq!(s.navigate(Action::Refresh, 6), None);
    }

    #[test]
    fn replies_open_page_and_close() {
        let mut s = CommentsState::loading(1);
        s.add_page(CommentsSlot::First, page(&["a", "b"], None));
        let load = s.open().unwrap();
        assert_eq!(load.slot, CommentsSlot::Replies { parent: "a".into() });
        assert_eq!(load.from, CommentsFrom::Token("r-a".into()));
        // One at a time.
        assert_eq!(s.open(), None);
        s.add_page(CommentsSlot::Replies { parent: "a".into() }, page(&["a.1", "a.2"], Some("more-a")));
        assert_eq!(ids(&s), ["a", "a.1", "a.2", "more:more-a", "b"]);
        // The button loads the next page in its place.
        s.selected = 3;
        assert_eq!(s.open().unwrap().from, CommentsFrom::Token("more-a".into()));
        s.add_page(CommentsSlot::Replies { parent: "a".into() }, page(&["a.3"], None));
        assert_eq!(ids(&s), ["a", "a.1", "a.2", "a.3", "b"]);
        assert_eq!(s.selected, 3);
        // Open on the comment hides them, and they can load again.
        s.selected = 0;
        assert_eq!(s.open(), None);
        assert_eq!(ids(&s), ["a", "b"]);
        assert!(s.open().is_some());
        s.add_page(CommentsSlot::Replies { parent: "a".into() }, page(&["a.1"], None));
        assert_eq!(ids(&s), ["a", "a.1", "b"]);
        // A comment without replies loads nothing.
        let mut s = CommentsState::loading(1);
        s.add_page(CommentsSlot::First, CommentPage { comments: vec![comment("x", None)], ..CommentPage::default() });
        assert_eq!(s.open(), None);
    }

    #[test]
    fn replies_keep_the_selection_on_its_row() {
        let mut s = CommentsState::loading(1);
        s.add_page(CommentsSlot::First, page(&["a", "b", "c"], None));
        s.selected = 0;
        s.open();
        s.selected = 2;
        s.add_page(CommentsSlot::Replies { parent: "a".into() }, page(&["a.1", "a.2"], None));
        assert_eq!(ids(&s)[s.selected], "c");
        // Replies for a comment that is gone change nothing.
        s.add_page(CommentsSlot::Replies { parent: "gone".into() }, page(&["g.1"], None));
        assert_eq!(ids(&s).len(), 5);
    }

    #[test]
    fn the_other_order_and_a_restart() {
        let mut s = CommentsState::loading(1);
        let sort =
            |label: &str, selected| CommentSort { label: label.into(), selected, continuation: format!("t-{label}") };
        let first = CommentPage {
            sorts: vec![sort("Top", true), sort("Newest", false)],
            count_text: Some("5 Comments".into()),
            ..page(&["a"], Some("p2"))
        };
        s.add_page(CommentsSlot::First, first);
        assert_eq!(s.next_sort().unwrap().label, "Newest");
        s.restart(2, Some("Newest"));
        assert_eq!(s.sort().unwrap().label, "Newest");
        assert_eq!(s.next_sort().unwrap().label, "Top");
        assert!(s.rows.is_empty() && s.loading == Loading::FirstPage && s.continuation.is_none());
        // A first page without orders and count keeps them.
        s.add_page(CommentsSlot::First, page(&["a"], None));
        assert_eq!(s.sorts.len(), 2);
        assert_eq!(s.count_text.as_deref(), Some("5 Comments"));
        assert_eq!(ids(&s), ["a"], "the repeat check starts over");
    }

    #[test]
    fn failures_stop_loading() {
        let mut s = CommentsState::loading(1);
        s.fail(&CommentsSlot::First, "down".into());
        assert_eq!((s.loading, s.error.as_deref()), (Loading::Idle, Some("down")));
        // A new try clears the error.
        let mut s = CommentsState::loading(1);
        s.add_page(CommentsSlot::First, page(&["a"], Some("p2")));
        s.loading = Loading::MorePages;
        s.fail(&CommentsSlot::More, "down".into());
        assert!(s.navigate(Action::Down, 6).flatten().is_some());
        assert_eq!(s.error, None);
        s.loading_replies = Some("a".into());
        s.fail(&CommentsSlot::Replies { parent: "a".into() }, "x".into());
        assert_eq!(s.loading_replies, None);
    }
}

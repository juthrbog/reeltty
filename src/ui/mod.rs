//! Draws the `App` state. Pure: the same state always gives the same frame.

mod items;
mod player;
mod screens;
mod search;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize as _};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Row, Table, Tabs};
use unicode_width::UnicodeWidthStr as _;

use crate::app::player::{Phase, dock};
use crate::app::{App, Tab};
use crate::keymap::Action;

pub fn view(app: &App, f: &mut Frame) {
    if let Some(p) = app.player.as_ref().filter(|p| !p.mini) {
        return player::draw(app, p, f);
    }
    let [tabs, body, status] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)]).areas(f.area());
    draw_tabs(app, f, tabs);
    let dock = app.player.as_ref().and_then(|_| dock(f.area().width, f.area().height));
    match (&app.player, dock) {
        (Some(p), Some(d)) => {
            let [list, column] = Layout::horizontal([Constraint::Min(0), Constraint::Length(d.cols)]).areas(body);
            draw_body(app, f, list);
            player::draw_mini(app, p, f, column);
        }
        _ => draw_body(app, f, body),
    }
    draw_status(app, f, status, dock.is_none());
    if app.show_help {
        draw_help(app, f, body);
    }
}

/// The size in cells of the video that `view` draws, or `None` when it
/// draws no video.
pub fn video_cells(app: &App) -> Option<(u16, u16)> {
    let p = app.player.as_ref().filter(|p| p.is_running())?;
    let up_next = p.phase == Phase::Ended && p.up_next.is_some();
    let no_dock = p.mini && dock(app.width, app.height).is_none();
    (!up_next && !no_dock).then_some((p.geometry.cols, p.geometry.rows))
}

fn draw_tabs(app: &App, f: &mut Frame, area: Rect) {
    let titles = Tab::ALL.iter().map(|t| t.title());
    let selected = Tab::ALL.iter().position(|t| *t == app.tab);
    let tabs = Tabs::new(titles)
        .select(selected)
        .highlight_style(Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED))
        .divider(" ");
    f.render_widget(tabs, area);
}

fn draw_body(app: &App, f: &mut Frame, area: Rect) {
    if let Some(screen) = app.screens.last() {
        return screens::draw(app, screen, f, area);
    }
    match app.tab {
        Tab::Search => search::draw(app, f, area),
        Tab::Feed => screens::draw_feed(app, f, area),
        Tab::Subscriptions => screens::draw_subscriptions(app, f, area),
        Tab::Playlists => screens::draw_saved(app, f, area),
        Tab::Queue => screens::draw_queue(app, f, area),
        Tab::History => screens::draw_history(app, f, area),
    }
}

/// The key hints and a message. With `no_dock`, the screen is too small for
/// the mini-player, so the message can be the video that plays.
fn draw_status(app: &App, f: &mut Frame, area: Rect, no_dock: bool) {
    let hint = |a: Action, label: &str| {
        app.keymap.first_key(a).map(|k| vec![Span::from(k.to_string()).bold(), Span::from(format!(" {label}  "))])
    };
    let mut wanted = vec![(Action::Help, "help"), (Action::Search, "search")];
    if !app.screens.is_empty() {
        wanted.push((Action::Back, "back"));
    } else if app.tab == Tab::Search {
        wanted.push((Action::Filters, "filters"));
    }
    wanted.push((Action::Quit, "quit"));
    let hints: Vec<Span> = wanted.into_iter().filter_map(|(a, l)| hint(a, l)).flatten().collect();
    let hints = Line::from(hints).right_aligned();
    let message = app
        .status
        .clone()
        .map(|m| (m, false))
        // Nothing else on screen shows the video, so it comes before lasting notes.
        .or_else(|| app.player.as_ref().filter(|_| no_dock).map(player::status_message))
        .or_else(|| match app.screens.last() {
            Some(screen) => screens::status(screen),
            None => (app.tab == Tab::Search).then(|| search::status(&app.search)).flatten(),
        })
        .or_else(|| app.graphics.message().map(|m| (m, false)));
    // A message that does not fit next to the hints gets the whole bar.
    let hints_width = u16::try_from(hints.width()).unwrap_or(u16::MAX);
    let message_width = message.as_ref().map_or(0, |(m, _)| u16::try_from(m.width()).unwrap_or(u16::MAX));
    let hints_width = if message_width > area.width.saturating_sub(hints_width + 2) { 0 } else { hints_width };
    let [left, right] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(hints_width)]).spacing(2).areas(area);
    match message {
        Some((m, true)) => f.render_widget(Paragraph::new(m).red(), left),
        Some((m, false)) => f.render_widget(Paragraph::new(m).yellow(), left),
        None => {}
    }
    if hints_width > 0 {
        f.render_widget(hints, right);
    }
}

/// The width of one column in the help.
const HELP_COLUMN: u16 = 40;

/// The keys of the browse screen, then of the player. The player keys work
/// in the mini-player too, when browse does not use them.
fn draw_help(app: &App, f: &mut Frame, area: Rect) {
    let row = |description: &str, keys: &[crate::keymap::KeyPress]| {
        let keys = keys.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
        Row::new([description.to_string(), keys])
    };
    let heading = |title: &str| Row::new([Line::from(title.to_string()).bold(), Line::default()]);
    let mut rows = vec![heading("Browse")];
    rows.extend(app.keymap.bindings().iter().map(|(a, keys)| row(a.description(), keys)));
    rows.push(heading("Player"));
    rows.extend(app.keymap.player.bindings().iter().map(|(a, keys)| row(a.description(), keys)));
    // As many columns as the height needs and the width allows.
    let fit = usize::from(area.height.saturating_sub(2)).max(1);
    let most = usize::from(area.width.saturating_sub(2) / HELP_COLUMN).max(1);
    let columns = rows.len().div_ceil(fit).clamp(1, most);
    let per_column = rows.len().div_ceil(columns);
    let height = u16::try_from(per_column).unwrap_or(u16::MAX).saturating_add(2);
    let width = HELP_COLUMN * u16::try_from(columns).unwrap_or(1) + 2;
    let popup = centered(area, width, height);
    let block = Block::bordered().title("Keys");
    let inner = block.inner(popup);
    f.render_widget(Clear, popup);
    f.render_widget(block, popup);
    let ratio = u32::try_from(columns).unwrap_or(1);
    let areas = Layout::horizontal(vec![Constraint::Ratio(1, ratio); columns]).split(inner);
    let mut rows = rows.into_iter();
    for column in areas.iter() {
        let chunk: Vec<Row> = rows.by_ref().take(per_column).collect();
        f.render_widget(Table::new(chunk, [Constraint::Length(20), Constraint::Min(0)]), *column);
    }
}

pub(crate) fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::screens::Target;
    use crate::app::thumbnails::Thumbnails;
    use crate::app::{Effect, Selection};
    use crate::app::{Input, Message};
    use crate::keymap::{KeyCode, KeyMap, KeyPress};
    use crate::source::model::{
        Channel, ChannelInfo, ChannelPage, ChannelRef, Chapter, Description, Item, ItemPage, Link, LinkTarget,
        Playlist, PlaylistKind, SearchPage, Video, VideoDetails, VideoKind,
    };
    use crate::store::Subscription;
    use crate::terminal::detect::Graphics;
    use crate::terminal::kitty;
    use crate::thumbnails::Picture;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use std::sync::Arc;

    fn buffer(app: &App, w: u16, h: u16) -> ratatui::buffer::Buffer {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| view(app, f)).unwrap();
        t.backend().buffer().clone()
    }

    /// The screen as text. Image placeholder cells show as `▒`.
    fn render(app: &App, w: u16, h: u16) -> String {
        let buf = buffer(app, w, h);
        let mut out = String::new();
        for y in 0..h {
            for x in 0..w {
                let symbol = buf[(x, y)].symbol();
                out.push_str(if symbol.starts_with(kitty::PLACEHOLDER) { "▒" } else { symbol });
            }
            out.push('\n');
        }
        out
    }

    fn press(app: &mut App, s: &str) {
        app.update(Message::Input(Input::Key(s.parse().unwrap())));
    }

    #[test]
    fn browse_screen() {
        let app = App::new(KeyMap::default(), Graphics::KittyShm);
        insta::assert_snapshot!(render(&app, 60, 8));
    }

    #[test]
    fn browse_screen_without_video() {
        let mut app = App::new(KeyMap::default(), Graphics::Unavailable("herdr".into()));
        app.tab = Tab::Subscriptions;
        insta::assert_snapshot!(render(&app, 100, 8));
    }

    #[test]
    fn help_overlay() {
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        press(&mut app, "?");
        insta::assert_snapshot!(render(&app, 130, 24));
    }

    fn sample_results() -> Vec<Item> {
        let chan = |name: &str| ChannelRef { id: Some(format!("UC{name}")), name: name.into(), handle: None };
        let thumb = |id: &str| {
            vec![crate::source::model::Thumbnail {
                url: format!("https://i.ytimg.com/vi/{id}/hq.jpg"),
                width: 336,
                height: 188,
            }]
        };
        let video = |id: &str, title: &str, kind, channels, members_only| {
            Item::Video(Video {
                id: id.into(),
                title: title.into(),
                kind,
                channels,
                duration_text: (kind != VideoKind::Live).then(|| "12:34".to_string()),
                duration_secs: None,
                views_text: Some(if kind == VideoKind::Live { "32 watching" } else { "1.2M views" }.into()),
                published_text: (kind != VideoKind::Live).then(|| "3 years ago".to_string()),
                thumbnails: thumb(id),
                members_only,
                badges: if members_only { Vec::new() } else { vec!["4K".into()] },
                description_snippet: None,
            })
        };
        vec![
            video("a", "Rust in 100 seconds", VideoKind::Normal, vec![chan("Fireship")], false),
            video("b", "Lofi beats to code to", VideoKind::Live, vec![chan("Lofi Girl")], false),
            video("c", "Async Rust with a friend", VideoKind::Normal, vec![chan("Jon Gjengset"), chan("Ana")], true),
            Item::Channel(Channel {
                id: "UCjon".into(),
                name: "Jon Gjengset".into(),
                handle: Some("@jonhoo".into()),
                subscribers_text: Some("111K subscribers".into()),
                description_snippet: None,
                verified: true,
                thumbnails: thumb("jon"),
            }),
            Item::Playlist(Playlist {
                id: "PLx".into(),
                title: "Crust of Rust".into(),
                kind: PlaylistKind::Playlist,
                channel: Some(chan("Jon Gjengset")),
                count_text: Some("44 videos".into()),
                thumbnails: thumb("crust"),
                first_video_id: None,
            }),
        ]
    }

    fn app_with_results(height: u16) -> App {
        search_app(height, false).0
    }

    /// An app with search results, and the effects of the reply.
    fn search_app(height: u16, thumbnails: bool) -> (App, Vec<Effect>) {
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        app.thumbnails = Thumbnails::new(thumbnails);
        app.update(Message::Input(Input::Resize { width: 70, height }));
        press(&mut app, "/");
        for c in "rust".chars() {
            app.update(Message::Input(Input::Key(KeyPress::plain(KeyCode::Char(c)))));
        }
        press(&mut app, "enter");
        let page = SearchPage {
            items: sample_results(),
            estimated_results: Some(6_192_373),
            continuation: None,
            ..SearchPage::default()
        };
        let generation = app.search.generation;
        let effects = app.update(Message::SearchResult { generation, append: false, result: Ok(page) });
        (app, effects)
    }

    #[test]
    fn search_results_with_thumbnails() {
        let (mut app, effects) = search_app(20, true);
        let urls: Vec<String> = effects
            .into_iter()
            .filter_map(|e| match e {
                Effect::LoadThumbnail { url, .. } => Some(url),
                _ => None,
            })
            .collect();
        assert_eq!(urls.len(), 5);
        let slot = app.thumbnails.slot();
        let rgba = vec![0; (slot.width * slot.height * 4) as usize];
        let picture = Picture { width: slot.width, height: slot.height, cols: slot.cols, rows: slot.rows, rgba };
        for url in [&urls[0], &urls[3]] {
            let result = Ok(Arc::new(picture.clone()));
            app.update(Message::ThumbnailLoaded { url: url.clone(), slot, result });
        }
        press(&mut app, "j");
        insta::assert_snapshot!(render(&app, 70, 20));

        // Each cell names its row and column, and the color names the image.
        let buf = buffer(&app, 70, 20);
        let first = (0..20).find(|y| buf[(1, *y)].symbol().starts_with(kitty::PLACEHOLDER)).unwrap();
        assert_eq!(buf[(1, first)].symbol(), kitty::cell(0, 0).unwrap());
        assert_eq!(buf[(2, first + 1)].symbol(), kitty::cell(1, 1).unwrap());
        let (r, g, b) = kitty::id_color(app.thumbnails.image(&app.search.results[0]).unwrap());
        assert_eq!(buf[(1, first)].fg, Color::Rgb(r, g, b));

        // Item 1 is selected. The highlight covers its text but not its image.
        let selected = first + 3;
        assert!(buf[(13, selected)].modifier.contains(Modifier::REVERSED), "the text is highlighted");
        assert!(buf[(1, selected)].symbol().is_empty() || buf[(1, selected)].symbol() == " ");
        app.update(Message::ThumbnailLoaded { url: urls[1].clone(), slot, result: Ok(Arc::new(picture.clone())) });
        let buf = buffer(&app, 70, 20);
        let (r, g, b) = kitty::id_color(app.thumbnails.image(&app.search.results[1]).unwrap());
        for x in 1..=slot.cols {
            let cell = &buf[(x, selected)];
            assert!(cell.symbol().starts_with(kitty::PLACEHOLDER));
            assert!(!cell.modifier.contains(Modifier::REVERSED), "the highlight leaves images alone");
            assert_eq!(cell.fg, Color::Rgb(r, g, b));
        }
    }

    #[test]
    fn partial_items_get_no_picture() {
        // The list has 11 rows: three whole items and two rows of a fourth.
        let (mut app, effects) = search_app(16, true);
        let slot = app.thumbnails.slot();
        let rgba = vec![0; (slot.width * slot.height * 4) as usize];
        let picture = Picture { width: slot.width, height: slot.height, cols: slot.cols, rows: slot.rows, rgba };
        for e in effects {
            if let Effect::LoadThumbnail { url, .. } = e {
                app.update(Message::ThumbnailLoaded { url, slot, result: Ok(Arc::new(picture.clone())) });
            }
        }
        let buf = buffer(&app, 70, 16);
        let rows_with_pictures = (0..16).filter(|y| buf[(1, *y)].symbol().starts_with(kitty::PLACEHOLDER)).count();
        assert_eq!(rows_with_pictures, 9);
    }

    #[test]
    fn narrow_lists_drop_thumbnails() {
        let (app, _) = search_app(12, true);
        let mut t = Terminal::new(TestBackend::new(25, 6)).unwrap();
        t.draw(|f| items::draw_list(&app.search.results, Selection::default(), &app.thumbnails, f, f.area())).unwrap();
        let buf = t.backend().buffer();
        assert!(buf.content().iter().all(|c| !c.symbol().starts_with(kitty::PLACEHOLDER)));
        // Still three lines for each item: the second one starts on row 3.
        assert_eq!(buf[(0, 3)].symbol(), "L");
    }

    #[test]
    fn search_results() {
        let mut app = app_with_results(16);
        press(&mut app, "j");
        insta::assert_snapshot!(render(&app, 70, 16));
    }

    #[test]
    fn search_results_scroll_with_the_selection() {
        let mut app = app_with_results(10);
        press(&mut app, "G");
        insta::assert_snapshot!(render(&app, 70, 10));
    }

    #[test]
    fn typing_a_query() {
        let mut app = app_with_results(16);
        press(&mut app, "/");
        press(&mut app, "backspace");
        insta::assert_snapshot!(render(&app, 70, 16));
    }

    #[test]
    fn filter_menu() {
        let mut app = app_with_results(16);
        press(&mut app, "f");
        press(&mut app, "enter");
        press(&mut app, "j");
        insta::assert_snapshot!(render(&app, 70, 16));
    }

    #[test]
    fn search_error() {
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        press(&mut app, "/");
        app.update(Message::Input(Input::Key(KeyPress::plain(KeyCode::Char('x')))));
        press(&mut app, "enter");
        let generation = app.search.generation;
        app.update(Message::SearchResult { generation, append: false, result: Err("network: timed out".into()) });
        insta::assert_snapshot!(render(&app, 70, 8));
    }

    fn channel_info() -> ChannelInfo {
        ChannelInfo {
            id: "UCjon".into(),
            name: "Jon Gjengset".into(),
            handle: Some("@jonhoo".into()),
            subscribers_text: Some("111K subscribers".into()),
            videos_text: Some("120 videos".into()),
            description: Some("\nRust streams and videos.\nMore text.".into()),
            verified: true,
            avatar: Vec::new(),
        }
    }

    #[test]
    fn channel_screen() {
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        app.update(Message::Input(Input::Resize { width: 70, height: 12 }));
        app.open(Target::Channel { id: "UCjon".into(), name: "Jon".into() });
        app.subscriptions.push(Subscription {
            channel_id: "UCjon".into(),
            name: "Jon".into(),
            handle: None,
            subscribed_at: 0,
        });
        let videos = ItemPage { items: sample_results().into_iter().take(2).collect(), continuation: Some("t".into()) };
        app.update(Message::ChannelLoaded { request: 1, result: Ok(ChannelPage { info: channel_info(), videos }) });
        insta::assert_snapshot!(render(&app, 70, 12));
    }

    #[test]
    fn playlist_screen_loading_then_failed() {
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        app.open(Target::Playlist { id: "PLx".into(), title: "Crust of Rust".into() });
        let loading = render(&app, 70, 8);
        app.update(Message::PlaylistLoaded { request: 1, result: Err("HTTP 404: Not Found".into()) });
        insta::assert_snapshot!(format!("{loading}{}", render(&app, 70, 8)));
    }

    fn a_comment(id: &str, name: &str, text: &str) -> crate::source::model::Comment {
        crate::source::model::Comment {
            id: id.into(),
            author: ChannelRef { id: Some("UC1".into()), name: name.into(), handle: None },
            text: Description { text: text.into(), links: Vec::new() },
            published_text: Some("6 years ago".into()),
            edited: false,
            likes_text: Some("4.8M".into()),
            reply_count_text: Some("986".into()),
            replies: Some("r".into()),
            pinned: false,
            hearted: false,
            by_uploader: false,
            verified: false,
            reply_level: u8::from(id.contains('.')),
        }
    }

    #[test]
    fn comments_pane() {
        use crate::app::comments::CommentsSlot;
        use crate::source::model::{CommentPage, CommentSort};
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        app.update(Message::Input(Input::Resize { width: 60, height: 24 }));
        app.open(Target::Video { id: "v".into(), title: "Me at the zoo".into() });
        let generation = match app.update(Message::Input(Input::Key("C".parse().unwrap()))).as_slice() {
            [Effect::LoadComments { generation, .. }] => *generation,
            other => panic!("{other:?}"),
        };
        insta::assert_snapshot!("comments_loading", render(&app, 60, 8));
        let pinned = crate::source::model::Comment {
            pinned: true,
            hearted: true,
            verified: true,
            ..a_comment("a", "@SanDiegoZoo", "We're so honored that the first ever YouTube video was filmed here!")
        };
        let long = (1..=9).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let uploader = crate::source::model::Comment {
            by_uploader: true,
            edited: true,
            likes_text: None,
            reply_count_text: None,
            replies: None,
            ..a_comment("b", "@jawed", &long)
        };
        let sort = |label: &str, selected| CommentSort { label: label.into(), selected, continuation: "t".into() };
        let page = CommentPage {
            comments: vec![pinned, uploader, a_comment("c", "@third", "Third")],
            continuation: None,
            count_text: Some("10,645,363 Comments".into()),
            sorts: vec![sort("Top", true), sort("Newest", false)],
            note: None,
        };
        app.update(Message::CommentsLoaded { request: 1, generation, slot: CommentsSlot::First, result: Ok(page) });
        app.update(Message::Input(Input::Key("enter".parse().unwrap())));
        let replies = CommentPage {
            comments: vec![a_comment("a.1", "@reply", "Very cool!")],
            continuation: Some("more".into()),
            ..CommentPage::default()
        };
        let slot = CommentsSlot::Replies { parent: "a".into() };
        app.update(Message::CommentsLoaded { request: 1, generation, slot, result: Ok(replies) });
        insta::assert_snapshot!("comments_pane", render(&app, 60, 24));
        // The selected comment shows in full, and the list follows it.
        for _ in 0..3 {
            app.update(Message::Input(Input::Key("j".parse().unwrap())));
        }
        let text = render(&app, 60, 24);
        assert!(text.contains("line 9"), "{text}");
        assert!(!text.contains("SanDiegoZoo"), "scrolled past the first comment: {text}");
        // A note when there are no comments.
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        app.open(Target::Video { id: "v".into(), title: "t".into() });
        app.update(Message::Input(Input::Key("C".parse().unwrap())));
        let page = CommentPage { note: Some("Comments are turned off.".into()), ..CommentPage::default() };
        app.update(Message::CommentsLoaded { request: 1, generation: 2, slot: CommentsSlot::First, result: Ok(page) });
        assert!(render(&app, 60, 8).contains("Comments are turned off."));
    }

    #[test]
    fn video_details_screen() {
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        app.open(Target::Video { id: "aircAruvnKk".into(), title: "But what is a neural network?".into() });
        let text = "Learn more: 3b1b.co/nn\n0:00 Intro\nThanks!".to_string();
        let details = VideoDetails {
            id: "aircAruvnKk".into(),
            title: "But what is a neural network?".into(),
            channel: Some(ChannelRef { id: Some("UCYO".into()), name: "3Blue1Brown".into(), handle: None }),
            subscribers_text: Some("8.73M subscribers".into()),
            views_text: Some("25,037,755 views".into()),
            date_text: Some("Oct 5, 2017".into()),
            publish_date: None,
            duration_secs: Some(1112),
            likes_text: Some("568K".into()),
            live: false,
            description: Description {
                text,
                links: vec![
                    Link { start: 12, len: 10, target: LinkTarget::Url("https://3b1b.co/nn".into()) },
                    Link {
                        start: 23,
                        len: 4,
                        target: LinkTarget::Video { id: "aircAruvnKk".into(), start_secs: Some(0) },
                    },
                ],
            },
            chapters: vec![
                Chapter { title: "Introduction".into(), start_secs: 0 },
                Chapter { title: "Series preview".into(), start_secs: 67 },
            ],
            thumbnails: Vec::new(),
            related: Vec::new(),
            comments: None,
            comments_count_text: None,
        };
        app.update(Message::VideoLoaded { request: 1, result: Ok(details) });
        insta::assert_snapshot!(render(&app, 70, 18));

        // At 10 rows the text is taller than the screen. G goes to the end,
        // and j cannot go past it.
        render(&app, 70, 10);
        press(&mut app, "G");
        press(&mut app, "j");
        let Some(crate::app::screens::Screen::Video(s)) = app.screens.last() else { panic!("no video screen") };
        let max = s.max_scroll.get();
        assert!(max > 0 && max < 20, "max {max}");
        assert_eq!(s.scroll, max);
        press(&mut app, "k");
        let Some(crate::app::screens::Screen::Video(s)) = app.screens.last() else { panic!("no video screen") };
        assert_eq!(s.scroll, max - 1);
    }

    #[test]
    fn subscriptions_tab() {
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        app.subscriptions = vec![
            Subscription {
                channel_id: "UC1".into(),
                name: "3Blue1Brown".into(),
                handle: Some("@3blue1brown".into()),
                subscribed_at: 0,
            },
            Subscription {
                channel_id: "UC2".into(),
                name: "Jon Gjengset".into(),
                handle: Some("@jonhoo".into()),
                subscribed_at: 0,
            },
        ];
        app.tab = Tab::Subscriptions;
        press(&mut app, "j");
        insta::assert_snapshot!(render(&app, 70, 10));
    }

    #[test]
    fn tiny_terminal_does_not_panic() {
        let mut app = app_with_results(3);
        press(&mut app, "f");
        render(&app, 4, 3);
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        press(&mut app, "?");
        render(&app, 5, 2);
    }

    fn playing_app() -> App {
        use crate::app::player::PlayerEvent;
        use crate::source::streams::{FormatInfo, Streams};
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        app.update(Message::Input(Input::Resize { width: 40, height: 12 }));
        app.update(Message::Input(Input::CellSize { width: 10, height: 20 }));
        app.play("v1".into(), "Big Buck Bunny".into());
        let streams = Streams {
            video_url: "https://v".into(),
            audio_url: None,
            headers: Vec::new(),
            available_at: None,
            expires_at: None,
            format: FormatInfo { width: Some(1280), height: Some(720), ..FormatInfo::default() },
            chapters: Vec::new(),
            channel: None,
            duration_secs: None,
            live: false,
            qualities: Vec::new(),
            captions: Vec::new(),
        };
        app.update(Message::StreamsResolved { session: 1, now: 0, result: Ok(Box::new(streams)) });
        for event in [PlayerEvent::FileLoaded, PlayerEvent::Duration(635_000), PlayerEvent::Position(65_000)] {
            app.update(Message::Player { session: 1, event });
        }
        app
    }

    #[test]
    fn player_shows_the_video_and_progress() {
        let mut app = playing_app();
        insta::assert_snapshot!(render(&app, 40, 12));
        // The video cells name the video image.
        let buf = buffer(&app, 40, 12);
        let g = app.player.as_ref().unwrap().geometry;
        let (r, gr, b) = kitty::id_color(kitty::VIDEO_ID);
        for (x, y) in [(g.x, g.y), (g.x + g.cols - 1, g.y + g.rows - 1)] {
            assert_eq!(buf[(x, y)].fg, Color::Rgb(r, gr, b));
            assert!(buf[(x, y)].symbol().starts_with(kitty::PLACEHOLDER));
        }
        app.update(Message::Player { session: 1, event: crate::app::player::PlayerEvent::Paused(true) });
        app.update(Message::Player { session: 1, event: crate::app::player::PlayerEvent::Muted(true) });
        let text = render(&app, 80, 12);
        assert!(text.contains("Paused  720p  muted"), "{text}");
    }

    #[test]
    fn the_progress_bar_marks_skipped_parts() {
        use crate::sponsorblock::{Category, Segment};
        let mut app = playing_app();
        let event = |e| Message::Player { session: 1, event: e };
        app.update(event(crate::app::player::PlayerEvent::Duration(100_000)));
        app.update(event(crate::app::player::PlayerEvent::Position(50_000)));
        let segment =
            |start_ms, end_ms| Segment { category: Category::Sponsor, start_ms, end_ms, video_duration_ms: None };
        app.update(Message::SegmentsLoaded {
            session: 1,
            result: Ok(vec![segment(20_000, 30_000), segment(70_000, 80_000)]),
        });
        let buf = buffer(&app, 40, 12);
        // " 0:50 " comes first, then a bar of 40 - 6 - 6 = 28 cells for 100 s.
        let y = 10;
        let cell = |secs: u16| &buf[(6 + secs * 28 / 100, y)];
        for (secs, color, symbol) in
            [(10, Color::Red, "━"), (25, Color::Green, "━"), (60, Color::Reset, "─"), (75, Color::Green, "─")]
        {
            assert_eq!(cell(secs).symbol(), symbol, "at {secs} s");
            assert_eq!(cell(secs).fg, color, "at {secs} s");
        }
    }

    #[test]
    fn player_before_and_after_playback() {
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        app.update(Message::Input(Input::Resize { width: 50, height: 8 }));
        app.play("v1".into(), "Big Buck Bunny".into());
        insta::assert_snapshot!("player_resolving", render(&app, 50, 8));
        let streams = crate::source::streams::Streams {
            video_url: "https://v".into(),
            audio_url: None,
            headers: Vec::new(),
            available_at: Some(115),
            expires_at: None,
            format: crate::source::streams::FormatInfo::default(),
            chapters: Vec::new(),
            channel: None,
            duration_secs: None,
            live: false,
            qualities: Vec::new(),
            captions: Vec::new(),
        };
        app.update(Message::StreamsResolved { session: 1, now: 100, result: Ok(Box::new(streams)) });
        insta::assert_snapshot!("player_waiting", render(&app, 50, 8));
        app.update(Message::Tick { session: 1, now: 115 });
        app.update(Message::Player {
            session: 1,
            event: crate::app::player::PlayerEvent::Failed("HTTP error 404".into()),
        });
        insta::assert_snapshot!("player_failed", render(&app, 50, 8));

        // A long error wraps and is shown in full.
        app.play("v2".into(), "Big Buck Bunny".into());
        let long = "yt-dlp failed: ERROR: [youtube] v2: Sign in to confirm your age. This video may be inappropriate";
        app.update(Message::StreamsResolved { session: 2, now: 0, result: Err(long.into()) });
        let text = render(&app, 30, 12);
        assert!(text.contains("inappropriate"), "{text}");
    }

    #[test]
    fn player_with_chapters_captions_and_the_quality_menu() {
        use crate::app::player::PlayerEvent;
        use crate::source::model::Chapter;
        use crate::source::streams::{Caption, FormatInfo, Quality, Streams};
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        app.update(Message::Input(Input::Resize { width: 60, height: 14 }));
        app.captions_on = true;
        app.play("v1".into(), "Big Buck Bunny".into());
        let streams = Streams {
            video_url: "https://v".into(),
            audio_url: Some("https://a".into()),
            headers: Vec::new(),
            available_at: None,
            expires_at: None,
            format: FormatInfo { width: Some(1280), height: Some(720), ..FormatInfo::default() },
            chapters: vec![Chapter { title: "Intro".into(), start_secs: 0 }],
            channel: None,
            duration_secs: None,
            live: false,
            qualities: [360, 720].into_iter().map(|res| Quality { res, url: String::new() }).collect(),
            captions: vec![Caption { lang: "en".into(), name: "English".into(), url: String::new(), auto: false }],
        };
        app.update(Message::StreamsResolved { session: 1, now: 0, result: Ok(Box::new(streams)) });
        for event in [PlayerEvent::FileLoaded, PlayerEvent::Speed(150), PlayerEvent::Duration(60_000)] {
            app.update(Message::Player { session: 1, event });
        }
        press(&mut app, "v");
        insta::assert_snapshot!(render(&app, 60, 14));
    }

    #[test]
    fn player_messages_take_the_whole_status_line() {
        let mut app = playing_app();
        app.status = Some("This video has no captions in \"en\".".into());
        let text = render(&app, 40, 12);
        assert!(text.contains("This video has no captions in \"en\"."), "{text}");
    }

    #[test]
    fn queue_tab() {
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        app.tab = Tab::Queue;
        insta::assert_snapshot!("queue_tab_empty", render(&app, 60, 8));
        app.queue = sample_results()
            .into_iter()
            .filter_map(|i| match i {
                Item::Video(v) => Some(v),
                _ => None,
            })
            .take(2)
            .collect();
        insta::assert_snapshot!("queue_tab", render(&app, 60, 10));
    }

    /// Search results, and a video in the mini-player, on a `width` x `height` screen.
    fn mini_app(width: u16, height: u16, streams: bool) -> App {
        use crate::app::player::PlayerEvent;
        use crate::source::streams::{FormatInfo, Streams};
        let mut app = app_with_results(height);
        app.update(Message::Input(Input::Resize { width, height }));
        app.update(Message::Input(Input::CellSize { width: 10, height: 20 }));
        app.play("v1".into(), "Big Buck Bunny".into());
        if streams {
            let streams = Streams {
                video_url: "https://v".into(),
                audio_url: None,
                headers: Vec::new(),
                available_at: None,
                expires_at: None,
                format: FormatInfo { width: Some(1280), height: Some(720), ..FormatInfo::default() },
                chapters: Vec::new(),
                channel: None,
                duration_secs: None,
                live: false,
                qualities: Vec::new(),
                captions: Vec::new(),
            };
            app.update(Message::StreamsResolved { session: 1, now: 0, result: Ok(Box::new(streams)) });
            for event in [PlayerEvent::FileLoaded, PlayerEvent::Duration(635_000), PlayerEvent::Position(65_000)] {
                app.update(Message::Player { session: 1, event });
            }
        }
        press(&mut app, "esc");
        app
    }

    #[test]
    fn mini_player_next_to_search_results() {
        let app = mini_app(100, 24, true);
        insta::assert_snapshot!(render(&app, 100, 24));
        // The video cells are in the box, and nothing else names the video image.
        let buf = buffer(&app, 100, 24);
        let g = app.player.as_ref().unwrap().geometry;
        let (r, gr, b) = kitty::id_color(kitty::VIDEO_ID);
        let mut cells = 0;
        for y in 0..24 {
            for x in 0..100 {
                if buf[(x, y)].fg == Color::Rgb(r, gr, b) && buf[(x, y)].symbol().starts_with(kitty::PLACEHOLDER) {
                    assert!((g.x..g.x + g.cols).contains(&x) && (g.y..g.y + g.rows).contains(&y), "{x},{y}");
                    cells += 1;
                }
            }
        }
        assert_eq!(cells, g.cols * g.rows);
    }

    #[test]
    fn video_cells_follow_what_view_draws() {
        use crate::app::player::PlayerEvent;
        let app = playing_app();
        let g = app.player.as_ref().unwrap().geometry;
        assert_eq!(video_cells(&app), Some((g.cols, g.rows)));
        // The mini-player on a screen with room for the dock, then without.
        let mut app = mini_app(100, 24, true);
        let g = app.player.as_ref().unwrap().geometry;
        assert_eq!(video_cells(&app), Some((g.cols, g.rows)));
        assert_eq!(video_cells(&mini_app(60, 16, true)), None);
        // Before mpv runs, and the up next list.
        assert_eq!(video_cells(&mini_app(100, 24, false)), None);
        let videos = sample_results().into_iter().filter_map(|i| match i {
            Item::Video(v) if v.kind == VideoKind::Normal => Some(v),
            _ => None,
        });
        app.update(Message::RelatedLoaded { session: 1, result: Ok(videos.collect()) });
        app.update(Message::Player { session: 1, event: PlayerEvent::Ended });
        assert_eq!(video_cells(&app), None);
    }

    #[test]
    fn mini_player_before_and_after_playback() {
        use crate::app::player::PlayerEvent;
        let app = mini_app(80, 16, false);
        insta::assert_snapshot!("mini_player_resolving", render(&app, 80, 16));
        let mut app = mini_app(80, 16, true);
        let videos: Vec<Video> = sample_results()
            .into_iter()
            .filter_map(|i| match i {
                Item::Video(v) if v.kind == VideoKind::Normal => Some(v),
                _ => None,
            })
            .collect();
        app.update(Message::RelatedLoaded { session: 1, result: Ok(videos) });
        app.update(Message::Player { session: 1, event: PlayerEvent::Ended });
        insta::assert_snapshot!("mini_player_up_next", render(&app, 80, 16));
    }

    #[test]
    fn a_small_screen_shows_the_mini_player_in_the_status_bar() {
        let app = mini_app(60, 16, true);
        let text = render(&app, 60, 16);
        assert!(text.lines().last().unwrap().contains("▶ Big Buck Bunny 1:05 / 10:35"), "{text}");
        assert!(!text.contains('▒'), "no video shows: {text}");
        // A failure shows, in red.
        let mut app = mini_app(60, 16, false);
        let result = Err("private video".into());
        app.update(Message::StreamsResolved { session: 1, now: 0, result });
        let buf = buffer(&app, 60, 16);
        let last: String = (0..60).map(|x| buf[(x, 15)].symbol()).collect();
        assert!(last.contains("Cannot play Big Buck Bunny: private video"), "{last}");
        assert_eq!(buf[(0, 15)].fg, Color::Red);
        // Before the streams, it says so.
        let text = render(&mini_app(60, 16, false), 60, 16);
        assert!(text.contains("▶ Big Buck Bunny · finding the video streams…"), "{text}");
    }

    #[test]
    fn player_up_next() {
        use crate::app::player::PlayerEvent;
        let mut app = playing_app();
        let videos: Vec<Video> = sample_results()
            .into_iter()
            .filter_map(|i| match i {
                Item::Video(v) if v.kind == VideoKind::Normal => Some(v),
                _ => None,
            })
            .collect();
        app.update(Message::RelatedLoaded { session: 1, result: Ok(videos) });
        app.update(Message::Player { session: 1, event: PlayerEvent::Ended });
        insta::assert_snapshot!(render(&app, 60, 12));
    }

    #[test]
    fn history_tab() {
        use crate::store::HistoryEntry;
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        press(&mut app, "shift-tab");
        insta::assert_snapshot!("history_tab_empty", render(&app, 60, 8));
        let entry = |id: &str, position_secs, finished, duration_secs| HistoryEntry {
            video_id: id.into(),
            title: format!("Video {id}"),
            channel_id: None,
            channel_name: Some("Fireship".into()),
            duration_secs,
            position_secs,
            finished,
            watched_at: 1_793_373_867,
        };
        app.history =
            vec![entry("a", 252, false, Some(635)), entry("b", 0, true, Some(100)), entry("c", 70, false, None)];
        insta::assert_snapshot!("history_tab", render(&app, 60, 10));
    }

    #[test]
    fn feed_tab() {
        use crate::store::Subscription;
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        press(&mut app, "tab");
        insta::assert_snapshot!("feed_tab_empty", render(&app, 60, 8));
        app.subscriptions =
            vec![Subscription { channel_id: "UCF".into(), name: "Fireship".into(), handle: None, subscribed_at: 1 }];
        let videos: Vec<Video> = sample_results()
            .into_iter()
            .filter_map(|i| match i {
                Item::Video(mut v) => {
                    v.published_text = Some("1 day ago".into());
                    Some(v)
                }
                _ => None,
            })
            .collect();
        app.update(Message::FeedLoading { count: 2 });
        app.update(Message::FeedChannelLoaded { channel_id: "UCF".into(), fetched_at: 1_000_000, result: Ok(videos) });
        insta::assert_snapshot!("feed_tab", render(&app, 60, 12));
    }
}

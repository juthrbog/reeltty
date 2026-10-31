//! The mpv wrapper against the real libmpv, with a generated test video and
//! no audio output.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use reeltty::player::mpv::{EndReason, Event, Events, Format, Frame, Mpv, Renderer, Value};

const OPTIONS: &[(&str, &str)] =
    &[("vo", "libmpv"), ("ao", "null"), ("hwdec", "no"), ("config", "no"), ("terminal", "no"), ("idle", "yes")];

fn testsrc(secs: u32) -> String {
    format!("av://lavfi:testsrc=size=320x240:rate=30:duration={secs}")
}

/// Waits for an event that `pick` accepts.
fn wait_for<T>(events: &mut Events, mut pick: impl FnMut(&Event) -> Option<T>) -> T {
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        if let Some(e) = events.wait(Duration::from_millis(100))
            && let Some(t) = pick(&e)
        {
            return t;
        }
    }
    panic!("no such event in 5 s");
}

/// Renders on a thread until the flag is set, as the player does. mpv waits
/// for frames to be shown before it finishes a seek.
fn render_thread(mpv: &Mpv) -> (Arc<AtomicBool>, JoinHandle<()>) {
    let mut renderer = Renderer::new(mpv).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let thread = std::thread::spawn(move || {
        let signal = renderer.signal();
        let mut frame = Frame::new(64, 48);
        while !flag.load(Ordering::Relaxed) {
            if signal.wait(Duration::from_millis(20)) && renderer.has_frame() {
                renderer.render(&mut frame).unwrap();
                renderer.report_swap();
            }
        }
    });
    (stop, thread)
}

#[test]
fn starts_and_stops_without_a_file() {
    let (mpv, _events) = Mpv::new(OPTIONS).unwrap();
    let renderer = Renderer::new(&mpv).unwrap();
    drop(mpv);
    // The renderer keeps the core alive until it goes.
    drop(renderer);
}

#[test]
fn renders_frames_and_ends() {
    let (mpv, mut events) = Mpv::new(OPTIONS).unwrap();
    let mut renderer = Renderer::new(&mpv).unwrap();
    let signal = renderer.signal();
    mpv.command(&["loadfile", &testsrc(1)]).unwrap();
    wait_for(&mut events, |e| (*e == Event::FileLoaded).then_some(()));

    let mut frame = Frame::new(160, 120);
    let end = Instant::now() + Duration::from_secs(5);
    let mut frames = 0;
    while frames < 3 && Instant::now() < end {
        if signal.wait(Duration::from_millis(100)) && renderer.has_frame() {
            renderer.render(&mut frame).unwrap();
            renderer.report_swap();
            frames += 1;
        }
    }
    assert_eq!(frames, 3);
    assert!(frame.bytes().iter().any(|b| *b != 0), "testsrc is not black");
    frame.set_opaque();
    assert!(frame.bytes().chunks(4).all(|p| p[3] == 255));

    // Keep rendering so that playback can reach the end.
    let reason = loop {
        if signal.wait(Duration::from_millis(10)) && renderer.has_frame() {
            renderer.render(&mut frame).unwrap();
            renderer.report_swap();
        }
        if let Some(Event::EndFile { reason, .. }) = events.wait(Duration::ZERO) {
            break reason;
        }
        assert!(Instant::now() < end + Duration::from_secs(5), "no end");
    };
    assert_eq!(reason, EndReason::Eof);
}

/// Captions are drawn into the frames: white text on a black video.
#[test]
fn draws_captions_into_frames() {
    let dir = tempfile::tempdir().unwrap();
    let vtt = dir.path().join("en.vtt");
    std::fs::write(&vtt, "WEBVTT\n\n00:00:00.000 --> 00:00:09.000\nHELLO WORLD\n").unwrap();
    let (mpv, mut events) = Mpv::new(OPTIONS).unwrap();
    let mut renderer = Renderer::new(&mpv).unwrap();
    let signal = renderer.signal();
    mpv.command(&["loadfile", "av://lavfi:color=c=black:size=320x240:rate=30:duration=10"]).unwrap();
    wait_for(&mut events, |e| (*e == Event::FileLoaded).then_some(()));
    mpv.command(&["sub-add", vtt.to_str().unwrap(), "select", "English", "en"]).unwrap();

    let mut frame = Frame::new(320, 240);
    let end = Instant::now() + Duration::from_secs(5);
    let bright = |f: &Frame| f.bytes().chunks(4).filter(|p| p[0] > 128 && p[1] > 128 && p[2] > 128).count();
    while bright(&frame) < 20 {
        assert!(
            Instant::now() < end,
            "no caption in the frames: sub-text {:?} max {:?}",
            mpv.get_string("sub-text"),
            frame.bytes().iter().max()
        );
        if signal.wait(Duration::from_millis(100)) && renderer.has_frame() {
            renderer.render(&mut frame).unwrap();
            renderer.report_swap();
        }
    }
}

#[test]
fn observes_pause_and_controls() {
    let (mpv, mut events) = Mpv::new(OPTIONS).unwrap();
    let (stop, thread) = render_thread(&mpv);
    mpv.observe(1, "pause", Format::Flag).unwrap();
    mpv.observe(2, "volume", Format::Double).unwrap();
    mpv.command(&["loadfile", &testsrc(10)]).unwrap();
    wait_for(&mut events, |e| (*e == Event::FileLoaded).then_some(()));

    mpv.command_async(7, &["cycle", "pause"]).unwrap();
    let error = wait_for(&mut events, |e| match e {
        Event::CommandReply { id: 7, error } => Some(error.clone()),
        _ => None,
    });
    assert_eq!(error, None);
    wait_for(&mut events, |e| (*e == Event::Property { id: 1, value: Value::Flag(true) }).then_some(()));
    mpv.command_async(8, &["add", "volume", "-10"]).unwrap();
    let volume = wait_for(&mut events, |e| match e {
        Event::Property { id: 2, value: Value::Double(v) } if *v < 100.0 => Some(*v),
        _ => None,
    });
    assert!((volume - 90.0).abs() < 0.01);

    mpv.command(&["seek", "4", "absolute"]).unwrap();
    wait_for(&mut events, |e| (*e == Event::Seek).then_some(()));
    wait_for(&mut events, |e| (*e == Event::PlaybackRestart).then_some(()));
    let pos: f64 = mpv.get_string("time-pos").unwrap().parse().unwrap();
    assert!((pos - 4.0).abs() < 0.5, "{pos}");

    stop.store(true, Ordering::Relaxed);
    thread.join().unwrap();
}

#[test]
fn bad_commands_reply_with_an_error() {
    let (mpv, mut events) = Mpv::new(OPTIONS).unwrap();
    // A bad command name fails at once. A bad property fails when it runs.
    assert!(mpv.command_async(3, &["no-such-command"]).is_err());
    mpv.command_async(3, &["set", "no-such-property", "1"]).unwrap();
    let error = wait_for(&mut events, |e| match e {
        Event::CommandReply { id: 3, error } => Some(error.clone()),
        _ => None,
    });
    assert!(error.is_some());
    assert!(mpv.command(&["set", "no-such-property", "1"]).is_err());
    assert_eq!(mpv.get_string("no-such-property"), None);
}

#[test]
fn wakeup_stops_a_wait() {
    let (mpv, mut events) = Mpv::new(OPTIONS).unwrap();
    let waiter = std::thread::spawn(move || {
        // Skip the events that come at start, such as "idle".
        while events.wait(Duration::ZERO).is_some() {}
        let start = Instant::now();
        while events.wait(Duration::from_secs(10)).is_some() {}
        start.elapsed()
    });
    std::thread::sleep(Duration::from_millis(200));
    mpv.wakeup();
    let took = waiter.join().unwrap();
    assert!(took < Duration::from_secs(2));
}

mod player {
    use std::io::Write;
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};

    use reeltty::app::player::{Geometry, PlayerCommand, PlayerEvent, SpeedStep};
    use reeltty::player::{EventSender, Player};
    use reeltty::source::streams::{FormatInfo, Streams};
    use reeltty::sponsorblock::{Category, Segment};
    use reeltty::terminal::kitty::VideoSink;

    const OPTIONS: &[(&str, &str)] = &[("ao", "null"), ("hwdec", "no")];
    const DELETE: &str = "\x1b_Ga=d,d=I,i=1,q=2\x1b\\";

    /// The terminal: it keeps what the player writes, and reads and removes
    /// each frame's shared memory object, as Ghostty and kitty do. A slow
    /// screen reads the objects only when the test says so.
    #[derive(Clone, Default)]
    struct Screen(Arc<Mutex<Vec<u8>>>, Option<Arc<Mutex<Vec<String>>>>);

    impl Write for Screen {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            use base64::Engine as _;
            let text = String::from_utf8_lossy(buf);
            if let Some(rest) = text.split_once("t=s,").map(|(_, r)| r)
                && let Some(b64) = rest.split_once(';').and_then(|(_, r)| r.split_once('\x1b')).map(|(n, _)| n)
            {
                let name = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
                let path = format!("/dev/shm{}", String::from_utf8(name).unwrap());
                match &self.1 {
                    Some(unread) => unread.lock().unwrap().push(path),
                    None => std::fs::remove_file(path).unwrap(),
                }
            }
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Screen {
        fn slow() -> Self {
            Self(Arc::default(), Some(Arc::default()))
        }

        /// A slow screen reads the frames it has.
        fn read(&self) {
            for path in self.1.as_ref().unwrap().lock().unwrap().drain(..) {
                // After a stop, the sink removes the frames that were not read.
                let _ = std::fs::remove_file(path);
            }
        }

        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }

        fn wait_for(&self, text: &str) {
            let end = Instant::now() + Duration::from_secs(5);
            while !self.text().contains(text) {
                assert!(Instant::now() < end, "no {text:?} in 5 s");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    fn channel() -> (EventSender, mpsc::Receiver<PlayerEvent>) {
        let (tx, rx) = mpsc::channel();
        let send: EventSender = Box::new(move |e| {
            let _ = tx.send(e);
        });
        (send, rx)
    }

    fn streams(video_url: String) -> Streams {
        Streams {
            video_url,
            audio_url: None,
            headers: vec![("User-Agent".into(), "test".into()), ("Accept".into(), "a, b".into())],
            available_at: None,
            expires_at: None,
            format: FormatInfo::default(),
            chapters: Vec::new(),
            channel: None,
            duration_secs: None,
            live: false,
            qualities: Vec::new(),
            captions: Vec::new(),
        }
    }

    fn geometry(width: u16, height: u16) -> Geometry {
        Geometry { x: 0, y: 0, cols: 4, rows: 2, width, height }
    }

    fn wait_for(rx: &mpsc::Receiver<PlayerEvent>, pick: impl Fn(&PlayerEvent) -> bool) -> PlayerEvent {
        let end = Instant::now() + Duration::from_secs(5);
        while let Ok(e) = rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
            if pick(&e) {
                return e;
            }
        }
        panic!("no such event in 5 s");
    }

    /// The shared memory objects of one session of this process.
    fn objects(session: u64) -> Vec<String> {
        let prefix = format!("reeltty-v-{}-{session}-", std::process::id());
        let names = std::fs::read_dir("/dev/shm").unwrap().filter_map(|e| e.ok()?.file_name().into_string().ok());
        names.filter(|n| n.starts_with(&prefix)).collect()
    }

    #[test]
    fn plays_resizes_and_stops() {
        let screen = Screen::default();
        let sink = Arc::new(VideoSink::new(Box::new(screen.clone())));
        let (send, rx) = channel();
        let player = Player::start(7, &streams(super::testsrc(10)), geometry(64, 36), sink, send, 0, OPTIONS).unwrap();
        wait_for(&rx, |e| *e == PlayerEvent::FileLoaded);
        screen.wait_for("s=64,v=36,c=4,r=2");

        player.command(&PlayerCommand::TogglePause);
        wait_for(&rx, |e| *e == PlayerEvent::Paused(true));
        // A paused video is drawn again at the new size.
        player.command(&PlayerCommand::Resize(geometry(32, 18)));
        screen.wait_for("s=32,v=18,c=4,r=2");
        player.command(&PlayerCommand::ToggleMute);
        wait_for(&rx, |e| *e == PlayerEvent::Muted(true));

        let stopped = Instant::now();
        let reaper = player.stop();
        assert!(screen.text().ends_with(DELETE), "the image is deleted at once");
        reaper.join().unwrap();
        assert!(stopped.elapsed() < Duration::from_secs(2));
        assert!(screen.text().ends_with(DELETE), "no frame after the delete");
        assert_eq!(objects(7), Vec::<String>::new());
    }

    #[test]
    fn sends_a_dropped_frame_again_while_paused() {
        let screen = Screen::slow();
        let sink = Arc::new(VideoSink::new(Box::new(screen.clone())));
        let (send, rx) = channel();
        let player = Player::start(10, &streams(super::testsrc(10)), geometry(64, 36), sink, send, 0, OPTIONS).unwrap();
        player.command(&PlayerCommand::TogglePause);
        wait_for(&rx, |e| *e == PlayerEvent::Paused(true));
        screen.wait_for("s=64,v=36");
        // The terminal is slow and reads nothing. After the first frame and
        // one resize, at least two frames wait, so the next size is dropped.
        player.command(&PlayerCommand::Resize(geometry(48, 26)));
        std::thread::sleep(Duration::from_millis(300));
        player.command(&PlayerCommand::Resize(geometry(32, 18)));
        std::thread::sleep(Duration::from_millis(300));
        assert!(!screen.text().contains("s=32,v=18"));
        // When the terminal catches up, the dropped frame comes.
        screen.read();
        screen.wait_for("s=32,v=18");
        player.stop().join().unwrap();
        screen.read();
    }

    #[test]
    fn starts_at_a_position_and_reports_caption_failures() {
        let sink = Arc::new(VideoSink::new(Box::new(Screen::default())));
        let (send, rx) = channel();
        let player = Player::start(12, &streams(super::testsrc(10)), geometry(32, 18), sink, send, 5, OPTIONS).unwrap();
        wait_for(&rx, |e| matches!(e, PlayerEvent::Position(ms) if (4500..7000).contains(ms)));
        player.command(&PlayerCommand::AddCaptions {
            url: "/no/such/captions.vtt".into(),
            lang: "en".into(),
            name: "English".into(),
        });
        wait_for(&rx, |e| matches!(e, PlayerEvent::CaptionsFailed(_)));
        player.stop().join().unwrap();
    }

    #[test]
    fn changes_speed_and_switches_the_video_stream() {
        let screen = Screen::default();
        let sink = Arc::new(VideoSink::new(Box::new(screen.clone())));
        let (send, rx) = channel();
        let player = Player::start(11, &streams(super::testsrc(10)), geometry(32, 18), sink, send, 0, OPTIONS).unwrap();
        wait_for(&rx, |e| *e == PlayerEvent::FileLoaded);
        player.command(&PlayerCommand::Speed(SpeedStep::Up));
        wait_for(&rx, |e| *e == PlayerEvent::Speed(110));
        player.command(&PlayerCommand::Speed(SpeedStep::Reset));
        wait_for(&rx, |e| *e == PlayerEvent::Speed(100));
        player.command(&PlayerCommand::SeekTo { secs: 3 });
        wait_for(&rx, |e| matches!(e, PlayerEvent::Position(ms) if (2500..4000).contains(ms)));

        // Another stream of the same video, from 6 s.
        let other = "av://lavfi:testsrc=size=160x120:rate=30:duration=10";
        player.command(&PlayerCommand::SwitchVideo { url: other.into(), secs: 6 });
        wait_for(&rx, |e| *e == PlayerEvent::FileLoaded);
        wait_for(&rx, |e| matches!(e, PlayerEvent::Position(ms) if (5500..8000).contains(ms)));
        // The speed stays, and no end or error came from the switch.
        player.command(&PlayerCommand::Speed(SpeedStep::Down));
        let mut events = Vec::new();
        let end = Instant::now() + Duration::from_secs(5);
        while let Ok(e) = rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
            if e == PlayerEvent::Speed(91) {
                break;
            }
            events.push(e);
        }
        assert!(!events.iter().any(|e| matches!(e, PlayerEvent::Ended | PlayerEvent::Failed(_))), "{events:?}");
        player.stop().join().unwrap();
    }

    #[test]
    fn reports_the_end_and_errors() {
        let sink = Arc::new(VideoSink::new(Box::new(Screen::default())));
        let (send, rx) = channel();
        let player =
            Player::start(8, &streams(super::testsrc(1)), geometry(32, 18), Arc::clone(&sink), send, 0, OPTIONS)
                .unwrap();
        wait_for(&rx, |e| *e == PlayerEvent::Ended);
        player.stop().join().unwrap();

        let (send, rx) = channel();
        let player =
            Player::start(9, &streams("/no/such/file.mp4".into()), geometry(32, 18), sink, send, 0, OPTIONS).unwrap();
        let PlayerEvent::Failed(why) = wait_for(&rx, |e| matches!(e, PlayerEvent::Failed(_))) else { unreachable!() };
        assert!(!why.is_empty());
        player.stop().join().unwrap();
    }

    fn sponsor(start_ms: u64, end_ms: u64) -> Segment {
        Segment { category: Category::Sponsor, start_ms, end_ms, video_duration_ms: None }
    }

    /// The events until one that `pick` accepts.
    fn events_until(rx: &mpsc::Receiver<PlayerEvent>, pick: impl Fn(&PlayerEvent) -> bool) -> Vec<PlayerEvent> {
        let end = Instant::now() + Duration::from_secs(5);
        let mut events = Vec::new();
        while let Ok(e) = rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
            let done = pick(&e);
            events.push(e);
            if done {
                return events;
            }
        }
        panic!("no such event in 5 s: {events:?}");
    }

    fn position(e: &PlayerEvent) -> Option<u64> {
        if let PlayerEvent::Position(ms) = e { Some(*ms) } else { None }
    }

    #[test]
    fn skips_segments_when_playback_reaches_them() {
        let sink = Arc::new(VideoSink::new(Box::new(Screen::default())));
        let (send, rx) = channel();
        let player = Player::start(13, &streams(super::testsrc(10)), geometry(32, 18), sink, send, 0, OPTIONS).unwrap();
        player.command(&PlayerCommand::Segments(vec![sponsor(1000, 4000)]));
        let events = events_until(&rx, |e| position(e).is_some_and(|ms| ms >= 4000));
        assert!(events.contains(&PlayerEvent::Skipped(Category::Sponsor)), "{events:?}");
        assert!(!events.iter().filter_map(position).any(|ms| (1500..3900).contains(&ms)), "{events:?}");
        player.stop().join().unwrap();
    }

    #[test]
    fn skips_where_playback_resumes() {
        let sink = Arc::new(VideoSink::new(Box::new(Screen::default())));
        let (send, rx) = channel();
        let player = Player::start(14, &streams(super::testsrc(10)), geometry(32, 18), sink, send, 5, OPTIONS).unwrap();
        wait_for(&rx, |e| *e == PlayerEvent::FileLoaded);
        // As the app does: the segments come when the file has loaded.
        player.command(&PlayerCommand::Segments(vec![sponsor(4000, 7000)]));
        let events = events_until(&rx, |e| position(e).is_some_and(|ms| ms >= 7000));
        assert!(events.contains(&PlayerEvent::Skipped(Category::Sponsor)), "{events:?}");
        player.stop().join().unwrap();
    }

    #[test]
    fn a_seek_into_a_segment_and_a_quality_change_do_not_skip() {
        let sink = Arc::new(VideoSink::new(Box::new(Screen::default())));
        let (send, rx) = channel();
        let player = Player::start(15, &streams(super::testsrc(10)), geometry(32, 18), sink, send, 0, OPTIONS).unwrap();
        let segments = vec![sponsor(3000, 9000)];
        player.command(&PlayerCommand::Segments(segments.clone()));
        wait_for(&rx, |e| *e == PlayerEvent::FileLoaded);
        player.command(&PlayerCommand::SeekTo { secs: 4 });
        let mut events = events_until(&rx, |e| position(e).is_some_and(|ms| ms >= 5000));
        let other = "av://lavfi:testsrc=size=160x120:rate=30:duration=10";
        player.command(&PlayerCommand::SwitchVideo { url: other.into(), secs: 5 });
        events.extend(events_until(&rx, |e| *e == PlayerEvent::FileLoaded));
        player.command(&PlayerCommand::Segments(segments));
        events.extend(events_until(&rx, |e| position(e).is_some_and(|ms| ms >= 6000)));
        assert!(!events.contains(&PlayerEvent::Skipped(Category::Sponsor)), "{events:?}");
        player.stop().join().unwrap();
    }
}

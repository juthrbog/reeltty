# reeltty design

reeltty is a keyboard-driven YouTube player that runs in a terminal. With it, you can browse, search, and play YouTube videos without the YouTube website. The video plays inside the terminal window, in Ghostty or kitty.

This document records the design decisions from the design session on 2026-10-26 and 2026-10-27. It also records the research facts that the decisions depend on. All facts are as of 2026-10-26.

## Goals

- Replace the YouTube website for daily browsing, searching, and playback.
- Play video inside the terminal, not in a separate window.
- Work without a Google login.
- Be a personal tool first. The repository is public, but polish for other users is not a goal for v1.

## Non-goals

- reeltty never includes a pseudo-terminal (PTY) or a terminal escape-sequence parser, and it never hosts other programs. A "YouTube terminal" can only mean a native reeltty window (see "Window mode, later"). It never means a terminal emulator.
- No video over SSH, tmux, Zellij, herdr, or other multiplexers in v1.
- No terminals other than Ghostty and kitty for video in v1. No sixel or iTerm2 image code.
- No actions that write to a Google account (likes, Watch Later, account subscriptions).
- No Trending page. YouTube removed it, and the endpoint returns HTTP 400.
- No publication to crates.io. The name `youtui` is taken there, and reeltty is a personal tool.

## Decisions

| # | Topic | Decision |
|---|---|---|
| Q1 | Audience | Personal tool, public repository `juthrbog/reeltty`. |
| Q2 | Where video plays | Inside the terminal, with the Kitty graphics protocol. |
| Q3 | Data sources | yt-dlp for stream URLs. A custom InnerTube client for metadata. Both sit behind a `Source` trait. |
| Q4 | Language | Rust with ratatui. |
| Q5, Q24 | v1 features | See "Features". |
| Q6 | Layout | Modal: a browse screen and a full-screen player. In v1.1, `esc` shrinks the player to a mini-player in a dock at the bottom right of the browse screen. See "Playback". |
| Q7 | Keys | Vim keys for navigation, mpv keys for the player. All keys can be changed in the configuration file. |
| Q8 | Storage | TOML configuration at `$XDG_CONFIG_HOME/reeltty/config.toml`. State in SQLite (`rusqlite`, bundled) at `$XDG_DATA_HOME/reeltty/reeltty.db`. |
| Q9 | App structure | One binary on tokio. Elm-style: one `App` state, all events arrive as `Message` values on a channel. The UI thread never blocks. The frame writer has its own thread. |
| Q10 | Playback engine | libmpv core with the software render API, plus a custom Kitty graphics frame writer. reeltty has its own small bindings to libmpv's C API, not `libmpv2-sys` (LGPL-2.1, see Q11). The ffmpeg-next and cpal design is the fallback. |
| Q11 | License | MIT or Apache-2.0. No GPL crates. rustypipe is a behavior reference only. |
| Q12, Q21, Q23 | Where video works | Ghostty 1.3.1+ and kitty 0.49+, run directly. Elsewhere, browsing works and playback shows a clear message. |
| Q13 | Formats | H.264 first, then VP9, never AV1. Opus or AAC audio, never IAMF. Maximum of 720p or the pixel size of the pane, whichever is smaller. yt-dlp selects the player clients. |
| Q14 | yt-dlp source | yt-dlp from `PATH` or a path in the configuration file. A `reeltty doctor` check at startup. A self-managed copy of yt-dlp can come later. |
| Q15 | Subscriptions feed | Only InnerTube channel tabs, no RSS. Four requests at a time, a 30-minute cache for each channel, refresh on demand. Shorts and live streams are hidden by default. |
| Q16, Q17, Q18 | Name | reeltty. Folder `~/workspace/github.com/juthrbog/reeltty`. |
| Q19 | Main frontend | Terminal mode. |
| Q20 | Terminal emulator | Never (see "Non-goals"). |
| Q22 | Frontend trait | Terminal input and graphics output sit behind a `Frontend` trait from the start. |
| Q25 | Thumbnails | Shown beside each row in lists, only where video works (Kitty graphics with shared memory). Drawn with reeltty's own Kitty code, the same as video uses: shared memory and Unicode placeholders. `ratatui-image` is not used, because it turns on shared memory only after its own terminal query, and it always builds its sixel code. Loaded lazily, cached on disk (100 MB at most). One configuration switch turns them off. |
| Q26 | Milestone 0 | A throwaway spike with a pass bar. See "Milestones". |
| Q27 | Captions | In v1. The `c` key toggles captions. A preferred language in the configuration file. |
| Q28 | SponsorBlock | In v1.1. Skipped categories in the configuration file, `sponsor` by default. Only `skip` segments: `mute`, `full`, `poi`, and `chapter` can come later. See "Playback". |
| Q29 | Player controls | Speed (`[` and `]`), a manual quality menu, and chapter jumps (`PgUp` and `PgDn`) in v1. |
| Q30 | Code quality | `cargo fmt`, `cargo clippy -- -D warnings` with pedantic lints, `cargo deny`, `cargo audit`, and `insta` snapshot tests. A GitHub Actions workflow runs all of them. |

## Features

### v1

- Search, with filters for upload date, duration, type, and sort order.
- Play a video in the terminal, with captions, speed, quality, and chapter controls.
- A play queue and autoplay of the next video.
- A subscriptions feed. You import subscriptions from Google Takeout (a CSV file) and add or remove channels in reeltty. The subscription list stays on your computer.
- Channel pages with the videos of the channel and a subscribe toggle.
- Video details: the description, views, date, and chapters.
- Related videos on the player screen. This list takes the place of the home feed, which needs a login.
- Public playlists. You can open them, play them, and save them on your computer.
- Watch history and resume from the last position. Both stay on your computer.
- Thumbnails in lists.

### v1.1

- Read-only comments.
- SponsorBlock.
- A mini-player while you browse.

### Later

- A self-managed copy of yt-dlp.
- Window mode (see "Window mode, later").
- An external mpv window as a fallback in unsupported terminals.

## Architecture

### Data layer

The `Source` trait hides where data comes from. Two implementations exist:

- The InnerTube source is the main source. It calls the private YouTube API that the website uses, with the WEB client context. It needs no login and no proof-of-origin token (PO token) for search, channel tabs, playlists, the `/next` endpoint, and comments. It saves the `visitorData` value from the first response and sends it as `X-Goog-Visitor-Id`.
- The yt-dlp source is the fallback for metadata. It runs `yt-dlp -J --flat-playlist`. It is slower because Python starts for each call, but yt-dlp gets fixes fastest.

The parser must accept both the old renderers (`videoRenderer`, `playlistVideoRenderer`) and the new view models (`lockupViewModel`) on every endpoint. Comments need a join from `commentViewModel` keys to the entity payloads in `frameworkUpdates`. JSON fixtures of real responses are snapshot tests. Expect a parser break every few months.

Comments show in a pane of the video details screen (`C`). They load only when the pane first opens, then page by page:

- The first token is in the `next` response of the video, so the details already have it. Without details from InnerTube, the comments call `next` once more.
- Every later page is a `next` call with only a continuation token: the next page, the other order (`o`, Top or Newest), the replies to a comment (`enter`), or more replies. One parser reads all of them.
- YouTube can send a comment again (the pinned one in Newest) or repeat whole pages. The app drops comments it showed already, and a page of repeats only ends the list.
- A page for an older list (before a new order or a refresh) is dropped by its generation number.
- Unicode direction controls (U+202A to U+202E, U+2066 to U+2069) are removed from comments, descriptions, and names, because they can show text in a misleading order. Control characters are already dropped by ratatui.
- Timestamps in comments are shown as links, but cannot be followed yet. Following links in descriptions and comments can come later.

Streams always come from yt-dlp:

1. Run `yt-dlp -J -f <selector> <url>` once for each video.
2. Read `requested_formats[].url`, `http_headers`, and `available_at`.
3. If `available_at` is in the future, show a countdown and wait. A request before that time gets HTTP 403. The wait is the length of the pre-roll ads and can be tens of seconds.
4. Give the video URL and the audio URL to libmpv.

Stream URLs expire after about 6 hours and are tied to the IP address. reeltty caches them for less than 6 hours and resolves them again after an HTTP 403 or a network change. Guest use has a limit of about 300 videos per hour, so reeltty does not prefetch many videos.

### Playback

libmpv opens the stream URLs and does the demux, decode, cache, seek, audio output, and audio and video synchronization. On each render-update callback, reeltty calls `mpv_render_context_render` with the software render API (`MPV_RENDER_API_TYPE_SW`) at the pixel size of the video pane. The `libmpv2` crate wraps only the OpenGL render API, and `libmpv2-sys` is LGPL-2.1. So reeltty declares the 20 or so C functions that it uses itself (`src/player/ffi.rs`, from mpv's ISC-licensed headers) and links to the system libmpv.

The frame writer copies each frame into a POSIX shared memory object and sends it with the Kitty graphics protocol:

- Transmission medium `t=s` (shared memory). The terminal unlinks the object after it reads it.
- Action `a=T` with one fixed image ID and placement ID for each frame, and a virtual placement (`U=1`).
- The transient hint `N=1`. Ghostty 1.3.1 ignores it, but kitty 0.48+ and Ghostty 1.4 use it to evict frames first.
- Synchronized output (DEC mode 2026) around each frame.
- Shared memory names that start with `/`.

The frame writer runs on its own thread, and mpv's render call sets its pace. ratatui draws while it holds the stdout lock, and the frame writer takes the same lock for each frame, so escape sequences never interleave. Video cells use Unicode placeholders (U+10EEEE), one placeholder for each cell, the same as thumbnails. ratatui draws them as ordinary cells. If the terminal has not read the last 2 frames, the frame writer drops the next one.

mpv also draws the captions into the software frame, so captions need no separate renderer.

SponsorBlock segments come from the privacy endpoint `/api/skipSegments/<hash prefix>`. The request names the first 4 hex characters of the SHA-256 hash of the video ID, and the answer has the segments of about 100 videos. The app keeps the segments as they came. When mpv has the file, the app filters them with yt-dlp's rules (drop segments from an upload of another length, clamp near the ends, merge near segments) and sends them to the player. The event thread sees every `time-pos` change, so it does the skips with `seek <end> absolute+exact`. The app sees the position only once a second. The rules:

- Playing into a segment skips it, also after a seek back to before it.
- A seek that lands in a segment does not skip it: the user chose to watch it. A reload at the same place (a quality change, or a new player after an HTTP 403 error) counts as a seek. A reload starts at a whole second, up to about 2 s before where the user was, so a segment that starts just after the landing does not skip either.
- New segments for the same video (mpv's exact length can change them) keep the position state.
- When the first segments come, a position in a segment skips it. This covers an intro at 0, a resume inside a segment, and segments that come after playback started.
- If SponsorBlock does not answer in 5 seconds, or fails, the video plays without skips.

The mini-player is the same player at another size. The app sends `Resize` with a smaller geometry, and the frame writer renders at that size. Nothing in the player or the Kitty code changes. The dock is the right third of the browse body (24 to 64 columns). The video sits in a box at the bottom of the dock, with the title, the progress bar, and the state under it. A dock column, not a box drawn over the list, keeps the list selection visible, and wide characters in the list cannot hide placeholder cells. The rules:

- Keys: typing and the help come first, then the browse keys, then the player keys that browse does not use. `v` shows the player full size, and `x` stops it.
- The next video (autoplay, `>`, `<`) stays in the mini-player. Opening a video from the list plays it full size. Opening the video that plays shows it full size, with no restart.
- Thumbnails load while the mini-player plays.
- The streams are still resolved for the full-size pane, so the full size stays sharp, with no new resolve.
- Below 72 x 14 cells there is no dock. The sound plays on, mpv renders a small picture that nothing shows, and the status bar shows the title and the time.
- A size change (the mini-player, full size, or a terminal resize) must not show a frame in the wrong grid of placeholders, where it is cropped or stretched. Before each draw, the frontend tells the video sink which cells it draws. The sink drops frames of other sizes, and deletes the frame the terminal has when the size changes, so the video is blank for one frame instead. The frontend does this before it takes the stdout lock, because the render thread takes the sink lock first and stdout second.

### Terminal detection

At startup, reeltty does these checks:

1. It looks for `TMUX`, `ZELLIJ`, `SSH_CONNECTION`, and a `TERM_PROGRAM` value of a multiplexer (for example `herdr`).
2. It sends a Kitty graphics query with `t=s` and a test shared memory object, followed by a DA1 request (`ESC [ c`). If no answer comes before the DA1 reply, or the answer has an error, shared memory is not available.

If one of these tests fails, browsing and search work without thumbnails. Playback shows a message such as: "Video needs Ghostty or kitty run directly. Detected: herdr".

Do not trust the `e=` feature query key yet. kitty 0.49 does not reply to it, and Ghostty 1.3.1 replies OK without support. Ghostty adds feature queries in 1.5.

### Dependencies at runtime

- `mpv` (for libmpv).
- `yt-dlp` with `yt-dlp-ejs`. The Arch package `yt-dlp-ejs` pulls in `deno`.
- A JavaScript runtime for yt-dlp. Deno is the recommended runtime. yt-dlp calls node and bun insecure for this purpose.

`reeltty doctor` makes sure that these are present. It also shows the version and age of yt-dlp and warns if yt-dlp is old. The Arch package of yt-dlp is days to weeks behind upstream, and YouTube breakage follows the upstream releases closely. For the fastest fixes, point the configuration file at a user-installed yt-dlp (nightly through `uv tool` or pipx).

At build time, `clang` is necessary if the fallback engine (ffmpeg-next) is used.

## Milestones

M0 to M3 and v1.1 are done.

### M0: playback spike

Done in PR #6. The spike passed.

Build a throwaway program that tests the full hard path:

1. Install the dependencies: `sudo pacman -S mpv clang yt-dlp`.
2. Resolve a video with yt-dlp.
3. Play it through the libmpv software render API at the pane size.
4. Write the frames to Ghostty 1.3.1 and kitty 0.49.2 through shared memory with `a=T` and a fixed image ID.
5. Play the audio through mpv.

The spike passes if it plays a smooth 30 fps at about 960x540 in Ghostty, keeps audio and video in sync, uses less than about 1.5 CPU cores, and shows no flicker.

If it fails at 960x540 but passes at 640x360, make the decision again. If it fails completely, change to the ffmpeg-next and cpal engine, or rethink the design. When Ghostty 1.4 is released (due 2026-10-31), run the spike again on it.

### M1: browse and search

Done in PRs #7, #8, #10, and #11.

The app shell (Elm loop, `Frontend` trait, key map, configuration, SQLite), the InnerTube source, search with filters, channel pages, video details, playlists, thumbnails, and `reeltty doctor`.

### M2: playback

Done in PRs #12, #13, and #14.

The player screen from the M0 spike, made production quality. yt-dlp stream resolution with the ad-wait countdown, the player controls, captions, the queue, autoplay, and related videos.

### M3: feed and history

Done in PRs #15, #16, and #17.

The Takeout import, the subscriptions feed, watch history, and resume.

### v1.1

Done in PRs #19, #20, #21, and #22.

Comments, SponsorBlock, and the mini-player.

## Window mode, later

If terminal mode is not good enough, a native window can use the same core. The stack that the research found:

- winit with glutin (EGL) and glow.
- The libmpv OpenGL render API draws into a framebuffer object sized to the player pane, with hardware decoding (`hwdec=auto-safe`) and the Wayland display passed through `RenderParam::WaylandDisplay`.
- A custom ratatui backend on `beamterm-core` draws the cell grid in the same OpenGL context.

Do not use wgpu-based ratatui backends for this. mpv has no Vulkan render API, so they force a copy through the CPU. Wayland frame pacing is a known risk. The estimate is 3 to 5 weeks of part-time work.

## Research facts

### yt-dlp

- The latest release is 2026.10.26. The default player clients are `tv_samsung`, `visionos`, and `web`. `tv_samsung` and `visionos` need no PO token.
- Full YouTube support needs a JavaScript runtime and the yt-dlp-ejs scripts. Without a runtime, yt-dlp uses only `visionos`, which cannot play "Made for kids" videos.
- The `web` client returns only SABR streams (a YouTube streaming protocol that yt-dlp cannot download yet) for adaptive formats. The SABR downloader is open as PR #13515.
- The mweb client with a PO token is not a dependable fallback. Issue #17666 reports SABR-only formats for it.
- Pure Rust extractors are not usable. rustypipe has had no release since April 2025, and its player is broken. rusty_ytdl has had no release since August 2024.

Sources: https://github.com/yt-dlp/yt-dlp/releases, https://github.com/yt-dlp/yt-dlp/wiki/EJS, https://github.com/yt-dlp/yt-dlp/wiki/PO-Token-Guide, https://github.com/yt-dlp/yt-dlp/issues/12482, https://github.com/yt-dlp/yt-dlp/pull/13515, https://github.com/yt-dlp/yt-dlp/issues/17666.

### Subscriptions

- Channel RSS feeds (`/feeds/videos.xml?channel_id=`) return random HTTP 404 and 500 errors, and they hold only 15 entries. FreeTube and NewPipe browse each channel tab through InnerTube by default.
- Google Takeout exports subscriptions as a CSV file with three columns: channel ID, channel URL, and channel title. The header names change with the language, so parse by column position. In English, the file is at `Takeout/YouTube and YouTube Music/subscriptions/subscriptions.csv`. The folder names and the file name change with the account language too (`Abos.csv` in German), so a folder search looks for the CSV file whose rows are channels.
- The YouTube Data API v3 allows only 100 `search.list` calls each day, and reading your own subscriptions needs OAuth.
- Almost no public Invidious or Piped instances work.

### Terminals

Ingest speed on this computer (AMD Phoenix iGPU, Wayland), 1280x720 RGB frames:

| Terminal | Shared memory | Base64 |
|---|---|---|
| kitty 0.49.2 | about 427 fps | about 75 fps |
| Ghostty 1.3.1 | about 400 fps | about 13 fps |

These numbers are the parse and load limit, not the paint speed. Ghostty main fixes the slow base64 path, and the fix ships in 1.4.

- Ghostty 1.3.1 supports shared memory, placements, z-index, and Unicode placeholders. It has no animation frames (they ship in 1.4) and no sixel. Ghostty draws placeholder images below text (z = -1).
- WezTerm has had no stable release since February 2024. Rio has no animation frames. Konsole has no shared memory. foot has only sixel.
- tmux needs passthrough and cannot use shared memory unless the terminal is local. Zellij rejects shared memory and sends images again as uncompressed base64. herdr intercepts Kitty graphics.
- mpv `--vo=kitty` writes to stdout without coordination with ratatui and sends delete-all on reconfiguration and on exit. It is not usable for the product.

Sources: https://github.com/kovidgoyal/kitty/blob/master/docs/graphics-protocol.rst, https://ghostty.org/docs/install/release-notes, https://github.com/mpv-player/mpv/blob/v0.41.0/DOCS/man/vo.rst.

### Rust crates

- ratatui 0.30.2. ratatui-image 11.1.0 (stable) and 12.0.0-rc.3.
- libmpv2 6.0.0 wraps only the OpenGL render API. libmpv2-sys 4.0.1 has the software render API (client API 2.5, which matches mpv 0.41).
- ffmpeg-next 9.0.0 builds against FFmpeg 9 and needs `clang`. rsmpeg does not build against FFmpeg 9.
- cpal 0.18.2 for audio if the fallback engine is used.

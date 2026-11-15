# reeltty

reeltty is a keyboard-driven YouTube player that runs in a terminal. Video plays inside the terminal window, in Ghostty or kitty. It needs no Google login.

This is a personal tool and work in progress. See [DESIGN.md](DESIGN.md) for the design and the milestones.

## Requirements

- Linux.
- `mpv` 0.37 or later (reeltty uses `libmpv`). It is tested with mpv 0.41.
- `yt-dlp` with `yt-dlp-ejs`, and `deno` as the JavaScript runtime for yt-dlp.
- `ffmpeg`, only if you set downloads to one `mkv` file (see "Downloads").
- Ghostty 1.3.1 or later, or kitty 0.49 or later, run directly (not in tmux, Zellij, or over SSH). In other terminals, browsing works but video does not.

On Arch Linux:

```sh
sudo pacman -S mpv yt-dlp yt-dlp-ejs deno
```

## Install

Each [release](https://github.com/juthrbog/reeltty/releases) has Linux archives for x86_64 and aarch64, the source, a Homebrew formula, and a PKGBUILD. The archives need mpv 0.37 or later and glibc 2.39 or later.

- **Homebrew** (Linux): `brew install juthrbog/tap/reeltty`. It builds reeltty from source and installs mpv, yt-dlp, and deno.
- **Arch Linux**: download `PKGBUILD` from the release, then run `makepkg -si` in its folder.
- **Archive**: unpack `reeltty_<version>_linux_<arch>.tar.gz` and put `reeltty` on your `PATH`.

To check that an archive comes from this repository's release workflow:

```sh
gh attestation verify reeltty_0.4.0_linux_amd64.tar.gz --repo juthrbog/reeltty
```

Or check the checksums and their signature:

```sh
cosign verify-blob --bundle checksums.txt.sigstore.json \
  --certificate-identity-regexp '^https://github.com/juthrbog/reeltty/\.github/workflows/release\.yml@refs/tags/v' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  checksums.txt
sha256sum --check --ignore-missing checksums.txt
```

See [docs/RELEASING.md](docs/RELEASING.md) for how releases are made.

## Build and run

```sh
cargo build --release
./target/release/reeltty doctor   # checks that everything is in place
./target/release/reeltty
```

Press `?` in reeltty to see the keys of the browse screen, the player, and the search line. It works in the player too, and the video plays on. In a small window, scroll the help with `j` and `k`. `?`, `esc`, or `q` closes it.

To import your YouTube subscriptions, export them with [Google Takeout](https://takeout.google.com/) (choose "YouTube and YouTube Music", then "subscriptions"). Unpack the archive, then run:

```sh
./target/release/reeltty import ~/Downloads/Takeout
```

You can also give the path of `subscriptions.csv`. Channels you follow already stay as they are, and the list stays on your computer.

The Feed tab shows the newest videos of the channels you follow, newest first. reeltty loads each channel at most every 30 minutes, four at a time, and keeps the feed on your computer, so it shows at once when reeltty starts. Press `r` on the Feed tab to load all channels again. Shorts, live streams, and members-only videos are hidden.

`tab` and `shift-tab` go to the next or previous tab, and `1` to `6` go to a tab: Search, Feed, Subscriptions, Playlists, Queue, and History. Each tab keeps the screens you opened in it, such as a channel or a video's comments, so when you come back, the same screen shows. A new search (`/`) starts from the results.

To play a video, select it and press `enter`. `i` shows its details. `a` adds a video to the queue, or takes it out. `s` subscribes to a channel or saves a playlist, and `s` again undoes it. On a video, `c` opens its channel; on the details screen, `s` subscribes to the video's channel. The Queue tab shows the queue, and `enter` there plays a video from it.

On the details screen, `C` shows the comments, and `C` again shows the details. In the comments, `j` and `k` move, `enter` shows or hides the replies of a comment, `o` switches between the Top and Newest order, `c` opens the channel of the comment's author, and `r` loads the comments again. A long comment shows in full when you select it. The comments work well with the mini-player: the video plays on while you read.

In a description or the selected comment, `n` and `N` select the next or previous link, and `enter` follows it: a web link opens in your browser (with `xdg-open`, or `open` on macOS; only `http` and `https` links), a time like 1:05 plays the video from there (or seeks, if it plays already), another video plays, and a channel opens. `esc` clears the selection, and without a selected link `enter` works as before.

The History tab shows the videos you played, newest first, and how far you got. In the other lists, a video you played says "watched", or where you stopped, such as "stopped at 4:12". A video you stopped in the middle plays again from where you stopped. With thumbnails on, a red bar along the bottom of a video's thumbnail shows how much of it you watched. On the Subscriptions, Playlists, Queue, and History tabs, `d` removes the selected entry. In the player, the keys are mostly the same as mpv's:

| Key | Action |
|---|---|
| `space`, `p` | Pause or play |
| `left`, `right` | Back or forward 5 seconds |
| `down`, `up` | Back or forward 1 minute |
| `9`, `0` | Volume down or up |
| `m` | Mute or unmute |
| `[`, `]` | Slower or faster |
| `backspace` | Normal speed |
| `pgup`, `pgdn` | Previous or next chapter |
| `v` | Captions on or off |
| `Q` | Video quality menu (mpv has no key for this; in mpv, `Q` quits and keeps the place, which reeltty always does) |
| `>`, `<` | Next or previous video |
| `esc` | Mini-player: go back to the list, and the video plays on |
| `q` | Stop and go back to the list |
| `?` | Show or hide the keys |

The mini-player shows the video at the bottom right of the list. The top of its box shows ▶ while the video plays, ⏸ while it is paused, and the time. The bottom edge of the box is the progress bar. Browse as usual: the browse keys come first, and the player keys that browse does not use still work, such as `space`, `left`, `right`, `m`, and `>`. In the help (`?`), the player keys that work only full size are dim: captions, chapters, the 1-minute jumps, and the quality menu. Press `v` to show the player full size again, and `q` (or `x`) to stop it, as full size. With nothing playing, `q` quits reeltty. `ctrl-c` always quits. `enter` on the video that plays also shows it full size, and `enter` on another video plays that one full size. When the screen is smaller than 72 x 14, the mini-player does not show, the sound plays on, and the status bar shows the video.

When a video ends, an "Up next" list shows the queue, then related videos. With autoplay on, the first one plays after 5 seconds. Move in the list with `j` and `k` to stop the countdown, and press `enter` to play the selected video. Live streams never play by themselves.

reeltty skips sponsor messages that the SponsorBlock community marked, and the status line says "Skipped sponsor.". The skipped parts are green on the progress bar. A short gap in the bar shows where a chapter starts. If you go back into a skipped part, it plays. The `[sponsorblock]` settings choose what to skip. For each video, reeltty sends your IP address and the first 4 characters of a hash of the video ID to sponsor.ajay.app. The server answers with the segments of all videos whose hash starts with those characters, so it does not learn which video you play.

Press `Q` in the player for the quality menu. In it, move with the browse keys (`j`, `k`), choose with `enter`, and close it with `esc`. The video goes on from the same place.

Some videos, such as videos made for kids, make you wait a few seconds before they start. YouTube makes guests wait as long as the ads would play, and reeltty shows a countdown. Most videos start at once.

The video size follows the size of the picture: a small window gets a small stream. By default it goes up to 1080p. Set `max_quality` to go higher or lower. Larger sizes use more CPU, mostly to scale the picture and for the terminal to draw it. Above 1080p, a laptop can play at less than 60 frames a second. On one 16-core laptop, a 60 fps video played at 57–60 fps at 1440p, and at about 50–58 fps with `"best"` (2160p).
## Downloads

Press `D` on a video to download it, so that you can watch it without a network. One video downloads at a time, and the others wait. The status bar shows the download that runs, such as "↓ Title 42% · 2 queued". Press `D` again to stop a download. Its unfinished files stay, so the next `D` goes on from there. `D` on a video whose download failed tries again. Downloads that wait or run when you quit go on the next time reeltty starts.

A downloaded video always plays from its files, also when you have a network. It starts at once, with no ads wait. The player says "saved", and it has only the size that was downloaded. If the files are gone, the video streams.

Downloads use the same formats as playback, up to `max_quality`. They go in `~/.local/share/reeltty/downloads`, named by the video ID. By default, each video is two files, a video and an audio, as YouTube sends them. Set `format = "mkv"` in `[downloads]` to get one file that any player can open. That needs ffmpeg. Live streams cannot be downloaded. Downloads count toward the same guest limit as playback, about 300 videos an hour. `reeltty doctor` shows the size of your downloads and the free space.

## Configuration

The configuration file is `~/.config/reeltty/config.toml`. All settings are optional.

```toml
# The yt-dlp program. The default is yt-dlp from PATH.
# A nightly build gets YouTube fixes first.
yt_dlp = "/home/me/.local/bin/yt-dlp"

# Show thumbnails in lists. They need the same terminal as video.
thumbnails = true

# When a video ends, play the next one from the queue or the related videos.
autoplay = true

# The largest video size: "auto" (up to 1080p), "best" (up to 2160p),
# or a height such as 1440 or 720.
max_quality = "auto"

# No spinner while things load: the status bar shows the text only.
reduce_motion = false

# Space between list items: "compact" (none), "comfortable" (a blank row),
# or "auto" (a blank row on screens 40 rows or taller).
density = "compact"

# The colors. "ansi" uses the 16 colors of your terminal theme, and works
# everywhere. The others are truecolor themes: "catppuccin-mocha",
# "catppuccin-latte" (light), and "tokyo-night". "auto" asks the terminal
# for its background color, and picks Catppuccin Latte on a light
# background and Catppuccin Mocha on a dark one, or when the terminal does
# not say. NO_COLOR turns all colors off.
theme = "ansi"

# The player's accent color: the theme's ("theme"), or the main color of
# the video's thumbnail ("auto"). "auto" needs thumbnails and a truecolor
# terminal, and it changes the lightness so that the color reads on your
# background. A video with a grey thumbnail keeps the theme's color.
accent = "theme"

# In the full player, hide the status line while the video plays and no
# key was pressed for 3 seconds. The progress bar stays, and any key shows
# the line again. A pause, an error, or the quality menu also shows it.
autohide_status = false

# Show Shorts and live streams in the subscriptions feed.
[feed]
shorts = false
live = false

# SponsorBlock: which parts of videos to skip. The categories are sponsor,
# selfpromo, interaction, intro, outro, preview, hook, filler, and
# music_offtopic.
[sponsorblock]
on = true
categories = ["sponsor"]

# Downloads: "separate" keeps the video and the audio as two files, as
# YouTube sends them. "mkv" merges them into one file that any player can
# open, and needs ffmpeg.
[downloads]
format = "separate"

# Captions: the language, and if they are on when a video starts.
# Captions made by a person come first, then automatic ones.
[captions]
language = "en"
on = false

# Each list replaces the default keys for that action.
[keys.browse]
down = ["j", "down"]
up = ["k", "up"]

[keys.player]
seek_forward_long = ["up", "l"]
```

The actions in `[keys.browse]` are `quit`, `up`, `down`, `top`, `bottom`, `half_page_up`, `half_page_down`, `open`, `back`, `search`, `filters`, `next_tab`, `prev_tab`, `refresh`, `details`, `channel`, `toggle`, `enqueue`, `delete`, `show_player`, `close_player`, `comments`, `sort`, `next_link`, `prev_link`, `help`, `tab_search`, `tab_feed`, `tab_subscriptions`, `tab_playlists`, `tab_queue`, `tab_history`, and `download`. The actions in `[keys.player]` are `back`, `minimize`, `toggle_pause`, `seek_forward`, `seek_back`, `seek_forward_long`, `seek_back_long`, `volume_up`, `volume_down`, `mute`, `speed_up`, `speed_down`, `speed_reset`, `next_chapter`, `prev_chapter`, `captions`, `quality`, `next_video`, `prev_video`, and `help`. Keys are written as `j`, `G`, `ctrl-d`, `alt-x`, `space`, `enter`, `esc`, `tab`, `shift-tab`, `backspace`, `up`, `down`, `left`, `right`, `pgup`, `pgdn`, `home`, `end`, or `f1` to `f12`. If you give a default key of one action to another action, the first action loses that key. For example, `back = ["q", "esc"]` in `[keys.player]` makes `esc` stop the video, and the mini-player then has no key. Two actions that you give the same key are an error. If an action has no key left because you gave all its keys to other actions, reeltty shows a warning when it starts, and `reeltty doctor` names the action. `ctrl-c` always quits: `quit` always has it, and no other action can have it.

Local state (subscriptions, the feed, saved playlists, and the watch history) is in `~/.local/share/reeltty/reeltty.db`. Thumbnails are cached in `~/.cache/reeltty/thumbnails`, up to 100 MB. The log is in `~/.local/state/reeltty/reeltty.log`. Set `REELTTY_LOG=debug` for more detail.

## Development

The tasks in `Taskfile.yml` run the same checks as CI ([Task](https://taskfile.dev); the program is `go-task` on Arch Linux). `task --list` shows them all.

```sh
task check   # fmt, clippy, shellcheck, tests, cargo deny, cargo audit
task test    # the tests only (the libmpv tests need mpv)
task fmt     # format the code
task run -- doctor
```

UI tests use [insta](https://insta.rs) snapshots. After a UI change, run `cargo insta review`.

## License

MIT or Apache-2.0, at your option.

# reeltty

reeltty is a keyboard-driven YouTube player that runs in a terminal. Video plays inside the terminal window, in Ghostty or kitty. It needs no Google login.

This is a personal tool and work in progress. See [DESIGN.md](DESIGN.md) for the design and the milestones.

## Requirements

- Linux.
- `mpv` 0.37 or later (reeltty uses `libmpv`). It is tested with mpv 0.41.
- `yt-dlp` with `yt-dlp-ejs`, and `deno` as the JavaScript runtime for yt-dlp.
- Ghostty 1.3.1 or later, or kitty 0.49 or later, run directly (not in tmux, Zellij, or over SSH). In other terminals, browsing works but video does not.

On Arch Linux:

```sh
sudo pacman -S mpv yt-dlp yt-dlp-ejs deno
```

## Build and run

```sh
cargo build --release
./target/release/reeltty doctor   # checks that everything is in place
./target/release/reeltty
```

Press `?` in reeltty to see the keys of the browse screen and of the player.

To import your YouTube subscriptions, export them with [Google Takeout](https://takeout.google.com/) (choose "YouTube and YouTube Music", then "subscriptions"). Unpack the archive, then run:

```sh
./target/release/reeltty import ~/Downloads/Takeout
```

You can also give the path of `subscriptions.csv`. Channels you follow already stay as they are, and the list stays on your computer.

The Feed tab shows the newest videos of the channels you follow, newest first. reeltty loads each channel at most every 30 minutes, four at a time, and keeps the feed on your computer, so it shows at once when reeltty starts. Press `r` on the Feed tab to load all channels again. Shorts, live streams, and members-only videos are hidden.

To play a video, select it and press `enter`. `i` shows its details. `a` adds a video to the queue, or takes it out. The Queue tab shows the queue, and `enter` there plays a video from it.

On the details screen, `C` shows the comments, and `C` again shows the details. In the comments, `j` and `k` move, `enter` shows or hides the replies of a comment, `o` switches between the Top and Newest order, `c` opens the channel of the comment's author, and `r` loads the comments again. A long comment shows in full when you select it. The comments work well with the mini-player: the video plays on while you read.

In a description or the selected comment, `n` and `N` select the next or previous link, and `enter` follows it: a web link opens in your browser (with `xdg-open`, or `open` on macOS; only `http` and `https` links), a time like 1:05 plays the video from there (or seeks, if it plays already), another video plays, and a channel opens. `esc` clears the selection, and without a selected link `enter` works as before.

The History tab shows the videos you played, newest first, and how far you got. A video you stopped in the middle plays again from where you stopped. `d` deletes an entry from the history (or from the queue in the Queue tab). In the player, the keys are the same as mpv's:

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
| `c` | Captions on or off |
| `v` | Video quality menu (mpv has no key for this) |
| `>`, `<` | Next or previous video |
| `esc` | Mini-player: go back to the list, and the video plays on |
| `q` | Stop and go back to the list |

The mini-player shows the video at the bottom right of the list. Browse as usual: the browse keys come first, and the player keys that browse does not use still work, such as `space`, `left`, `right`, `m`, and `>`. Press `v` to show the player full size again, or `x` to stop it. `enter` on the video that plays also shows it full size, and `enter` on another video plays that one full size. When the screen is smaller than 72 x 14, the mini-player does not show, the sound plays on, and the status bar shows the video.

When a video ends, an "Up next" list shows the queue, then related videos. With autoplay on, the first one plays after 5 seconds. Move in the list with `j` and `k` to stop the countdown, and press `enter` to play the selected video. Live streams never play by themselves.

reeltty skips sponsor messages that the SponsorBlock community marked, and the status line says "Skipped sponsor.". The skipped parts are green on the progress bar. If you go back into a skipped part, it plays. The `[sponsorblock]` settings choose what to skip. For each video, reeltty sends your IP address and the first 4 characters of a hash of the video ID to sponsor.ajay.app. The server answers with the segments of all videos whose hash starts with those characters, so it does not learn which video you play.

In the quality menu, move with the browse keys (`j`, `k`), choose with `enter`, and close it with `esc`. The video goes on from the same place.

As a guest, you sometimes wait before a video starts. YouTube makes guests wait as long as the ads would play, and reeltty shows a countdown.

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

The actions in `[keys.browse]` are `quit`, `up`, `down`, `top`, `bottom`, `half_page_up`, `half_page_down`, `open`, `back`, `search`, `filters`, `next_tab`, `prev_tab`, `refresh`, `details`, `channel`, `toggle`, `enqueue`, `delete`, `show_player`, `close_player`, `comments`, `sort`, `next_link`, `prev_link`, and `help`. The actions in `[keys.player]` are `back`, `minimize`, `toggle_pause`, `seek_forward`, `seek_back`, `seek_forward_long`, `seek_back_long`, `volume_up`, `volume_down`, `mute`, `speed_up`, `speed_down`, `speed_reset`, `next_chapter`, `prev_chapter`, `captions`, `quality`, `next_video`, and `prev_video`. Keys are written as `j`, `G`, `ctrl-d`, `alt-x`, `space`, `enter`, `esc`, `tab`, `shift-tab`, `backspace`, `up`, `down`, `left`, `right`, `pgup`, `pgdn`, `home`, `end`, or `f1` to `f12`. If you give a default key of one action to another action, the first action loses that key. For example, `back = ["q", "esc"]` in `[keys.player]` makes `esc` stop the video, and the mini-player then has no key. Two actions that you give the same key are an error.

Local state (subscriptions, the feed, saved playlists, and the watch history) is in `~/.local/share/reeltty/reeltty.db`. Thumbnails are cached in `~/.cache/reeltty/thumbnails`, up to 100 MB. The log is in `~/.local/state/reeltty/reeltty.log`. Set `REELTTY_LOG=debug` for more detail.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo deny check
cargo audit
```

UI tests use [insta](https://insta.rs) snapshots. After a UI change, run `cargo insta review`.

## License

MIT or Apache-2.0, at your option.

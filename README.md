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

Press `?` in reeltty to see the keys.

To play a video, select it and press `enter`. `i` shows its details. In the player, the keys are the same as mpv's:

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
| `q`, `esc` | Stop and go back to the list |

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

The actions in `[keys.browse]` are `quit`, `up`, `down`, `top`, `bottom`, `half_page_up`, `half_page_down`, `open`, `back`, `search`, `filters`, `next_tab`, `prev_tab`, `refresh`, `details`, `channel`, `toggle`, and `help`. The actions in `[keys.player]` are `back`, `toggle_pause`, `seek_forward`, `seek_back`, `seek_forward_long`, `seek_back_long`, `volume_up`, `volume_down`, `mute`, `speed_up`, `speed_down`, `speed_reset`, `next_chapter`, `prev_chapter`, `captions`, and `quality`. Keys are written as `j`, `G`, `ctrl-d`, `alt-x`, `space`, `enter`, `esc`, `tab`, `shift-tab`, `backspace`, `up`, `down`, `left`, `right`, `pgup`, `pgdn`, `home`, `end`, or `f1` to `f12`.

Local state (subscriptions, history, saved playlists) is in `~/.local/share/reeltty/reeltty.db`. Thumbnails are cached in `~/.cache/reeltty/thumbnails`, up to 100 MB. The log is in `~/.local/state/reeltty/reeltty.log`. Set `REELTTY_LOG=debug` for more detail.

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

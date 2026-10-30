# M0 playback spike

Throwaway code for milestone M0 in `DESIGN.md`. It tests the full hard path:

1. yt-dlp resolves the stream URLs (H.264 first, then VP9, Opus or AAC audio).
2. libmpv decodes the video and renders each frame with the software render API (`MPV_RENDER_API_TYPE_SW`, through `libmpv2-sys`).
3. Each frame goes to the terminal through POSIX shared memory with the Kitty graphics protocol (`t=s`, `a=T`, one fixed image ID and placement ID, `N=1`, synchronized output).
4. mpv plays the audio.

Do not reuse this code as it is. M2 rebuilds the player properly.

## Run

Run it in Ghostty or kitty directly, not in tmux, Zellij, herdr, or over SSH.

```sh
cargo build --release
./target/release/m0-spike --max-fps 30 'https://www.youtube.com/watch?v=aqz-KE-bpKQ'
```

Keys: `q` or `Ctrl-C` quits, `space` pauses, the left and right arrows seek 5 seconds.

The bottom row shows live stats once a second. When the program exits, it prints a summary and writes it to `m0.log`, together with the mpv log.

Useful options (see `--help` for all of them):

| Option | Use |
|---|---|
| `--size WxH` | Render size. The default fits the pane, at most 960x540. |
| `--placement unicode` | Use Unicode placeholder cells (U+10EEEE) in place of a direct placement. |
| `--seconds N` | Quit N seconds after the first frame. Good for timed runs. |
| `--no-sync`, `--no-transient` | Turn off synchronized output or the `N=1` hint, to see if they matter. |
| `--hwdec no` | Decode in software. The default is `auto-copy-safe`. |
| `--headless` | Render the frames but write nothing to the terminal. This measures mpv alone. |
| `--mpv KEY=VALUE` | Pass any mpv option, for example `--mpv sws-scaler=bilinear`. |

## Results, 2026-10-29

The test video is Big Buck Bunny, format 136 (H.264 1280x720 30 fps) plus format 251 (Opus). The computer is an AMD Phoenix iGPU on Wayland. mpv 0.41.0, yt-dlp 2026.10.26. Each run lasts 15 to 20 seconds.

| Terminal | Mode | Size | fps | CPU cores (reeltty + terminal) | Largest A/V offset | Frames dropped |
|---|---|---|---|---|---|---|
| kitty 0.49.2 | direct | 960x540 | 29.3 | 0.29 + 0.06 | 7.4 ms | 1 of 598 |
| kitty 0.49.2 | unicode | 960x540 | 29.4 | 0.28 + 0.07 | 6.6 ms | 0 |
| Ghostty 1.3.1 | direct | 960x540 | 29.7 | 0.29 + 0.13 | 3.5 ms | 0 |
| Ghostty 1.3.1 | unicode | 960x540 | 29.8 | 0.28 + 0.15 | 3.2 ms | 0 |
| Ghostty 1.3.1 | unicode | 1280x720 at 60 fps | 59.2 | 0.85 + 0.40 | 0.1 ms | 1 of 1207 |
| none (headless) | - | 960x540 | 29.0 | 0.25 | 10.1 ms | 0 |

Decoding in software (`--hwdec no`) costs the same 0.25 cores at 960x540 as `vulkan-copy`.

Against the pass bar, the numbers pass with a large margin. The CPU use is about a quarter of the 1.5-core limit, and 720p at 60 fps still fits.

### What the numbers do not show

- **fps counts frames written to the terminal, not frames painted.** The Kitty protocol has no paint acknowledgement. "Dropped" counts frames skipped because the terminal had not yet read the last 2 shared memory objects.
- **The A/V offset is mpv's `avsync`.** It does not include the time from the write to the screen. That time is about one or two screen refreshes.
- **Terminal CPU is the whole terminal process.** If one Ghostty process serves several windows, the number includes them. GPU and compositor time are not counted.

So smoothness, flicker, and lip sync need a check by eye. On 2026-10-29 that check passed in Ghostty: playback looked smooth, with no flicker and no visible A/V offset. M0 passes.

## Notes for M1 and M2

- The mpv option is `sws-fast`, not `sw-fast` as `render.h` suggests.
- `rgb0` leaves the 4th byte undefined. Kitty `f=32` reads it as alpha, so the spike sets it to 255 for each frame.
- On Linux, `write(2)` to the shared memory object sets its size. No `ftruncate` or `mmap` is necessary.
- The spike unlinks the shared memory objects that are left when it exits, but only after it waits up to 200 ms for the terminal to read them. Without the wait, both terminals log "No such file" for the last frame.
- Turn off auto-wrap (`ESC [ ? 7 l`) while video plays. A status line on the last row could otherwise wrap and scroll the screen.
- `libmpv2-sys` is LGPL-2.1, and it links to the system `libmpv` dynamically. Check this against the "no GPL crates" rule (Q11) before M2.
- `src/diacritics.rs` is copied from ratatui-image 12.0.0-rc.3 (MIT).

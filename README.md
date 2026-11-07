# Cutline

A simple video cutter for Linux, made for H.265 video. Open a file, drag two
handles, press **Export**. No settings needed.

![Main window: a dark editor with one big preview, transport and volume controls, the timeline with trim handles, and the Export button](docs/main.png)

## Using it

1. **Open.** Drop one or more videos on the window, use *Open with → Cutline*,
   or run `cutline clip.mp4`. It starts playing at once and the timeline fills
   with thumbnails. Several videos are laid end to end.
2. **Trim.** Drag the start and end handles, or press `I` and `O` while it
   plays. The preview follows the handle you're dragging, so you see the exact
   frame you're cutting on. **Split** (`S`) cuts the part under the playhead
   in two and **Delete part** (`Del`) throws the selected one away, so
   *Split, Split, Delete* cuts a stretch out of the middle. Right-click the
   timeline to switch to cutting out the marked parts instead of keeping
   them. The volume slider next to the transport (or `↑` / `↓`, `M` to
   mute) sets how loud the preview plays; it's remembered between runs and
   never changes the exported file.
3. **Export** (`Ctrl+E`). The dialog opens with *Original quality* picked, so
   Export finishes in seconds for most files and the result is saved next to
   the source as `<name> (cut).<ext>`.

The export dialog is one list; every row shows how big the file comes out and
how long it takes:

| Choice | What it does |
| --- | --- |
| **Original quality** (default) | Copies the video without re-encoding. Only the few frames between each cut and the next keyframe are re-encoded, so cuts land on the exact frame. |
| **Fit to size** | Type a size in MB. Cutline shows a quality label and a time estimate before you save, and the file never comes out bigger than that size. |
| **Smaller file**, **Plays anywhere**, **For editing**, **Audio only** | Named presets. |
| **Yours** | Profiles you made with *New profile…* (container, codec, size or quality, resolution, frame rate, audio). Right-click one to edit or delete it. |

Codecs, resolution, frame rate and bitrate only appear in **New profile…**. A
finished export's **Details** shows the exact ffmpeg commands and the log.

*Safe by default:** an export writes into a hidden scratch folder and only
moves the finished file into place at the end, so Cancel (or a crash) never
leaves a broken file. Nothing is overwritten without asking (*Replace* or
*Keep both*). Preferences such as volume and the last export folder persist
between launches.

### Keys

| Key | Action |
| --- | --- |
| `Space` | Play / pause |
| `I` / `O` | Set the start / end of the part under the playhead (or start a new part) |
| `S` | Split the part under the playhead |
| `A` | Add a part at the playhead |
| `Delete` | Remove the selected part |
| `←` / `→` | One frame back / forward |
| `Shift+←` / `Shift+→` | One second back / forward |
| `Home` / `End` | Start / end of the timeline |
| `=` / `-` | Zoom the timeline in / out around the playhead |
| `↑` / `↓` | Preview volume up / down |
| `M` | Mute / unmute the preview |
| `Ctrl+Z` / `Ctrl+Shift+Z` | Undo / redo |
| `Ctrl+O` / `Ctrl+E` | Open / Export |
| `Esc` / `Enter` | Close / confirm the dialog on top (New profile goes back to Export) |

Zoom into the timeline with `Ctrl`+scroll (or pinch) where you point, `=` /
`-`, or the zoom buttons by the totals, down to single frames; *Fit* shows it
all again. Zoomed in, scroll to move along it or drag the bar under the part
labels; the view follows the playhead, and dragging a handle against an edge
scrolls on.

Double-click an empty stretch of the timeline to add a part there. Right-click
it to remove a part, reorder videos or take one off the timeline.

Cutline draws its own title bar (the window buttons sit next to Export; drag
the header to move the window, double-click it to maximize, drag an edge to
resize). Run with `CUTLINE_SYSTEM_TITLEBAR=1` to keep your desktop's instead.

## Installing

You need a Rust toolchain (1.80+) and, at run time, `ffmpeg`/`ffprobe` (6.0 or
newer for frame-exact Original quality cuts) and ideally `libmpv` (for smooth
preview with sound; without it the preview has no sound).

```bash
sudo apt install ffmpeg libmpv2        # Debian/Ubuntu
cargo build --release
install -Dm755 target/release/cutline ~/.local/bin/cutline
install -Dm644 packaging/cutline.desktop ~/.local/share/applications/cutline.desktop
```

`target/release/cutline-cli` is the same engine without a window:

```bash
cutline-cli probe clip.mp4
cutline-cli export clip.mp4 --keep 0:12-1:05 --keep 2:00-2:30          # Original quality
cutline-cli export clip.mp4 --keep 12-65 --profile fit --size 25       # Fit to 25 MB
cutline-cli export a.mp4 b.mp4 --keep 3-10 --keep 0-4@1 --separate     # @1 = second file
cutline-cli export clip.mp4 --keep 5-20 --plan                         # print the ffmpeg plan only
```

## How it works

```
cutline-gui (eframe/egui)      cutline-cli (clap)
        \                         /
         cutline-engine  ──  cutline-preview (libmpv via dlopen, thumbnails)
         planner (pure) · fit solver · estimates · job queue · stores
                |
         cutline-ffmpeg: locate, capabilities, ffprobe, typed commands, runner
                |
         cutline-core: timestamps, clips & parts, profiles (no IO)
```

* **cutline-core**: the data model. Timestamps are integer microseconds;
  parts never overlap; handles snap to whole frames.
* **cutline-ffmpeg**: finds ffmpeg/ffprobe, probes files, builds typed
  commands (the same text appears in Details) and runs them with progress and
  cancel.
* **cutline-engine**: the **planner** is a pure function from the request plus
  facts about the sources to the exact list of ffmpeg steps, so every decision
  is unit tested. The engine also holds the fit-to-size solver, the size and
  time estimates (refined by a 3-second sample encode), the background job
  queue, and the JSON stores for profiles and settings (XDG directories).
* **cutline-preview**: playback through libmpv's software renderer (loaded at
  run time, so Cutline starts without it), with an ffmpeg-only fallback, plus
  disk-cached timeline thumbnails.

### Frame-exact cuts without re-encoding

Copying can only start on a keyframe, so each part is built from up to three
pieces: the frames from the cut to the next keyframe are re-encoded with the
source's codec, size, pixel format and colours (no B-frames, capped near the
source bitrate); the packets between keyframes are copied untouched; the frames
from the last keyframe to the end cut are re-encoded again. Open-GOP leading
pictures (whose references are gone) are dropped with the `noise` bitstream
filter, parameter sets are repeated in-band, and `setts` keeps decode
timestamps increasing across each join. After muxing, Cutline decodes the
frames around every join. If any come out wrong (decode errors, frames out of
order, gaps), that video is converted at matching quality instead, so the cut
points you chose are always respected.

### Fit to size

The solver (`crates/cutline-engine/src/fit.rs`) holds back 3% for rate-control
error plus the container's own bytes, gives audio a share that shrinks as the
budget does, then picks the largest picture size (and full or half frame rate)
at which the remaining bitrate still looks good, against a reference bitrate
that scales with `pixels^0.75 · fps^0.6`. It encodes H.265 two-pass, checks the
real file, and re-runs the second pass at a corrected bitrate if it came out
over the target. When the size you type is at least what Original quality
would produce, it keeps the original instead.

## Development

```bash
cargo test --workspace    # unit tests, UI tests, end-to-end exports (skipped without ffmpeg)
cargo test -p cutline-gui ui_tests   # just the UI tests: the real window driven headlessly
cargo clippy --workspace --all-targets
CUTLINE_NO_MPV=1 cargo run -p cutline-gui -- clip.mp4   # try the ffmpeg-only preview
```

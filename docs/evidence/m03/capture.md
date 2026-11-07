# Milestone 03 evidence: capture

Measured on 2026-11-07. Brief: [milestone 03](../../milestones/03-opinion-video.md), work item 1. Contract: [take folder](../../contracts/project-files.md#take-folder).

## Verdict

Screen, system audio and the event log record into a fragmented MP4 take folder, survive `kill -9` with at most the open fragment lost, and cost about 4 % of one core. Camera and mic could not be recorded in this session: macOS did not grant Camera or Microphone access to the process (see [Not verified](#not-verified)).

## Setup

| | |
| --- | --- |
| Source | branch `task/m03-capture` on top of `208eddb` |
| Machine | MacBook Pro, Apple M1 Pro, 32 GB, lid closed, on AC power |
| Displays | two 2560x1440 at 60 Hz, main display recorded |
| OS and tools | macOS 27.0.1 (26A434), rustc 1.99.1, ffprobe from Homebrew |
| Crates | objc2 0.6.5, objc2-screen-capture-kit, objc2-av-foundation, objc2-core-graphics 0.3.2 |
| Permissions | Screen Recording and Input Monitoring granted to the host app. Camera and Microphone not determined, requests denied without a prompt or left unanswered |
| Load | Other agents were building and measuring at the same time |

## How it works

- **Writer.** One `AVAssetWriter` per track with the `MPEG4AppleHLS` profile and a 0.5 s preferred segment interval. Its segment delegate appends the init segment and each `moof` plus `mdat` to the file as it is produced. `movieFragmentInterval` was tried first: for MPEG-4 output it is ignored and the file was `ftyp mdat moov`, so a crash would lose everything.
- **Clock.** At start the session reads the host clock once. That origin is converted into ScreenCaptureKit's and AVCaptureSession's synchronization clocks with `CMSyncConvertTime`, and every sample is retimed to `pts - origin` before it reaches its writer. The event tap stamps events with the host clock minus the origin. Samples before the origin are dropped.
- **Screen pacing.** ScreenCaptureKit only delivers frames when the screen changes (70 frames in 5 s on an idle screen, with keyframes up to 1.1 s apart). A pacer thread writes the newest frame at `k / 60` s instead, so `screen.mp4` is constant 60 fps with a keyframe and a fragment every 0.5 s even when idle. The writer opens on the first frame before the origin is taken, otherwise its 130 ms setup made the pacer drop the first frames.
- **Settings.** HEVC Main, `MaxKeyFrameIntervalDuration` 0.5 s, frame reordering off, BT.709 tags, 0.1 bit per pixel for the screen and 0.08 for the camera. AAC LC 48 kHz, 192 kb/s stereo for system audio, 128 kb/s mono for the mic. The mic output is set to mono 32-bit float at 48 kHz, which the tap hands on.
- **Camera.** HEVC at 1920x1080 when the camera offers it. The briefing's ProRes camera track is out for now.

## 20 s take

```sh
shot record --video rec/v20 --duration 20 --no-camera --no-mic
```

Synthetic mouse moves and one scroll were posted during the take with a CGEvent script.

| File | Result |
| --- | --- |
| `screen.mp4` | HEVC Main, 2560x1440, 60/1 fps (r and avg), no B-frames, BT.709, 1204 frames, 41 keyframes (I) and 1163 P, frame pts on an exact 1/60 s grid, `ftyp moov` then 41 `moof`+`mdat`, start 0.000, 20.07 s, 7.2 Mb/s |
| `system.mp4` | AAC LC 48 kHz stereo, `ftyp moov` then 41 `moof`+`mdat`, start 0.000, 20.03 s |
| `events.jsonl` | 309 lines, every line parses, has exactly the contract's fields for its kind and is sorted by `t` (308 `move`, 1 `scroll`) |

A frame cropped around a logged cursor position shows no cursor.

**CPU** (sampled each second for 16 s, release build): `shot` 3.6 % of one core (0.97 s of CPU over 20.7 s wall), VTEncoderXPCService 1.8 %, replayd 2.1 %, WindowServer 7.5 % against 2.4 % idle. Peak RSS 43 MB.

## Kill -9

`kill -9` about 6.25 s after recording started:

| File | Result |
| --- | --- |
| `screen.mp4` | `ftyp moov` then 12 complete fragments, no trailing bytes, 360 frames, 12 keyframes, 6.00 s, decodes with `ffmpeg -f null` without errors |
| `system.mp4` | 12 complete fragments, 5.99 s, decodes without errors |
| `events.jsonl` | every line parses, last event at 4.10 s, the last synthetic move |

The open fragment, about 0.25 s, was lost.

## Other checks

- Ctrl-C stops a recording and finishes every file, exit 0.
- With `media/t01` and `media/t07` present, the next take is `t08`.
- `--display 999` and `--camera Nope` fail at once with a message pointing to `--list-devices`.
- Recording with the default camera or mic fails with `Camera permission is missing. Allow this app in System Settings > Privacy & Security > Camera, then start it again`, the same for the mic. When macOS shows no prompt this is immediate; when a prompt is pending the session waits up to 60 s for an answer.
- `shot record --list-devices` lists both displays, three cameras and five mics.
- Tests: 8 unit tests (clock, take ids, event lines against the contract example, key names, modifiers, cursor mapping). Ignored hardware test `a_take_is_fragmented_mp4_with_a_fresh_id` passes.

## Not verified

- **Camera and mic recording.** The host app has no Camera or Microphone access, so `camera.mp4`, `mic.mp4` and the mic tap never ran. The ignored test `the_mic_tap_hands_over_mono_samples_on_the_media_clock` fails on the permission error. To check: grant the terminal Camera and Microphone access, then run `shot record --video <dir> --duration 20` and `cargo test -p shot-capture --test record -- --ignored`.
- **Clicks and keys.** Only synthetic moves and a scroll were posted, to avoid clicking or typing into open apps. Down, up and key lines are covered by the unit tests against the contract example, not by a live recording.
- **Power loss.** Fragments are written to the page cache without `fsync`, which survives a killed process but not a power cut.
- **A 4K or 120 Hz display.** Only the 2560x1440 external displays were connected.

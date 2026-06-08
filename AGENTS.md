# vcam-windows-rs Plan

## Goal

Build a Windows virtual camera prototype in Rust that uses `windows-rs` for Media Foundation and Win32 bindings. The current prototype must support both:

- a static in-memory test pattern fallback, and
- a broker COM control plane plus shared-memory data plane for dynamic BGRA frame injection.

This is still a prototype, not a production packaging or deployment effort.

## Workspace Layout

- `crates/vcam-server`
  - `cdylib + rlib`
  - owns the COM DLL exports, activation object, Media Foundation source/stream, broker COM class, shared-memory session types, and pixel-conversion helpers
- `crates/vcamctl`
  - CLI for COM registration, virtual camera create/remove, local frame dumping, and direct COM smoke tests
- `crates/vcamfeed-demo`
  - demo feeder that starts a broker session and continuously writes animated BGRA frames into shared memory

## Current Status

- Completed:
  - project restructured into a Cargo workspace
  - COM activation object, class factory, and exported DLL entrypoints implemented
  - static BGRA test-pattern generation and NV12 conversion implemented
  - `IVcamFrameBroker` COM class implemented inside `vcam-server`
  - shared-memory feed path implemented with fixed control mapping, data mapping, and named mutex
  - helper CLI implemented with `register-com`, `unregister-com`, `create-camera`, `remove-camera`, `probe-create`, `dump-frame`, and `dump-com-frame`
  - `vcamfeed-demo stream-animated` implemented for dynamic feeder smoke testing
  - PowerShell install/uninstall helper added
- Verified:
  - `cargo build --workspace` and `cargo check --workspace` pass
  - `vcamctl dump-frame` writes a valid static BMP
  - `vcamctl dump-com-frame` succeeds for both `RGB32` and `NV12`
  - machine-wide COM registration allows the virtual camera device to enumerate
  - direct shared-feed recovery logic no longer uses producer PID/process probing
- Current recovery model:
  - feed session liveness is determined only by `active bit + heartbeat freshness`
  - stale dynamic feed is deactivated by heartbeat timeout
  - there is no `OpenProcess` / `WaitForSingleObject(pid)` dead-producer detection anymore
  - shared-memory frame reads retry on inconsistent slot copies and otherwise fall back cleanly

## Current Debug Guidance

1. Treat real camera-host preview behavior as the source of truth. `dump-com-frame` is useful, but it does not prove host compatibility by itself.
2. `crates/vcam-server/src/media_stream.rs` is a sensitive path. Avoid broad behavior changes there unless a real host regression requires it and you can re-test promptly.
3. If dynamic-feed recovery regresses again, start from `crates/vcam-server/src/feed_shared.rs` first. Do not reintroduce PID-based producer liveness probing without strong evidence.
4. If `target\debug\vcam_server.dll` or `target\debug\vcamctl.exe` is locked by Camera, OBS, or FrameServer, prefer `cargo check --workspace` before attempting a full rebuild.

## Validation Path

1. Build:
   - `cargo build --workspace`
2. Install/register/create:
   - `pwsh.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File .\scripts\manage-vcam.ps1 -Action Install`
3. Static local smoke test:
   - `cargo run -p vcamctl -- dump-frame target\static-test-pattern.bmp`
4. Direct COM smoke test:
   - `cargo run -p vcamctl -- dump-com-frame target\com-rgb32.bmp --subtype rgb32`
   - `cargo run -p vcamctl -- dump-com-frame target\com-nv12.bmp --subtype nv12`
5. Dynamic feed smoke test:
   - `cargo run -p vcamfeed-demo -- stream-animated --width 640 --height 480 --fps 10 --duration-seconds 10 --force-reset`
6. Real host validation:
   - open Windows Camera or another Media Foundation client
   - verify static fallback first
   - then start the animated feeder and verify preview changes over time
   - after feeder exit or heartbeat timeout, verify fallback returns to static content

## Constraints

- Keep file IO out of the frame-serving path.
- Avoid registry or filesystem writes from the media source itself; only the CLI, scripts, and COM registration exports may do that.
- Prefer machine-wide COM registration for real virtual-camera testing.
- Treat packaging, signing, and WHQL-ready deployment as out of scope.

## Historical Notes

- `1985fc1` is the last known-good baseline before the PID-based dead-producer experiment.
- `f20aef0` introduced producer-PID probing for feed recovery and was the first known dynamic-feed regression point.
- `bc63e6a` removes that PID probing and returns feed recovery to heartbeat-only detection.

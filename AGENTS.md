# vcam-windows-rs Plan

## Goal

Build a Windows virtual camera prototype in Rust that uses `windows-rs` for Media Foundation and Win32 bindings, and uses the `com` crate from the `com-rs` project for COM runtime integration where it is practical in the helper tooling. The first milestone only needs to output a static test image as camera frames.

## Scope

- Target Windows 11 build 22000 or newer.
- Implement a Media Foundation software camera source exposed through COM.
- Stream one fixed test pattern as repeated video frames.
- Provide a small CLI to:
  - register the COM server for development,
  - create or remove the virtual camera with `MFCreateVirtualCamera`,
  - dump one frame locally for smoke testing without depending on camera enumeration.

## Deliverables

1. `cdylib` COM server:
   - `IMFActivate`
   - `IMFMediaSourceEx`
   - `IMFMediaStream2`
   - `IKsControl`
   - `IMFSampleAllocatorControl`
   - exported `DllGetClassObject`, `DllCanUnloadNow`, `DllRegisterServer`, `DllUnregisterServer`
2. Static test-pattern generator:
   - deterministic BGRA image
   - optional NV12 conversion for wider app compatibility
3. CLI helper:
   - `register-com`
   - `unregister-com`
   - `create-camera`
   - `remove-camera`
   - `dump-frame`
4. Minimal usage notes in the repository root if build/runtime caveats matter.

## Current Status

- Completed:
  - crate converted to `cdylib + bin`
  - COM activation object, class factory, and exported DLL entrypoints implemented
  - static BGRA test-pattern generator and NV12 conversion implemented
  - helper CLI implemented with `register-com`, `unregister-com`, `create-camera`, `remove-camera`, `probe-create`, and `dump-frame`
  - PowerShell install/uninstall helper added
- Verified:
  - `cargo build` passes
  - local `dump-frame` path writes a valid BMP
  - machine-wide COM registration allows the virtual camera device to appear in Windows enumeration
- In progress:
  - some camera clients still show a black preview even though the device enumerates
  - media stream was updated to prefer `IMFVideoSampleAllocator`, default `NV12`, stream frame-source attributes, and 2D surface writes; this needs reinstall and runtime verification in camera apps

## Current Debug Focus

1. Reinstall the updated DLL and verify whether preview rendering now works in Windows Camera and other Media Foundation clients.
2. If black preview persists, align `IMFMediaSource` / `IMFMediaStream` start-event payloads and presentation-descriptor behavior more closely with the Microsoft `SimpleMediaSource` sample.
3. Only after preview is stable, consider broader compatibility work such as richer allocator handling, timing polish, and additional media-type validation.

## Implementation Order

1. Convert the crate into `lib + bin`.
2. Add Windows and COM dependencies plus the required Win32 feature sets.
3. Implement the static image frame generator and pixel conversion helpers.
4. Implement the media stream and media source based on the Microsoft `SimpleMediaSource` design, reduced to the minimum needed for this prototype.
5. Implement the activation object and COM class factory.
6. Implement COM registration helpers and the CLI wrapper around `MFCreateVirtualCamera`.
7. Build the crate, fix API mismatches, and verify at least the local frame-dump path.

## Constraints

- Keep file IO out of the frame-serving path; the static test image should be generated or embedded in memory.
- Avoid registry or filesystem writes from the media source itself; only the CLI and COM registration exports may do that.
- Treat full production packaging, driver signing, and WHQL-ready deployment as out of scope for this prototype.

## Risks

- Media Foundation camera plumbing in Rust is verbose and the first pass will likely need build-fix iterations.
- Current-user COM registration is not sufficient for this prototype's current virtual-camera activation path; elevated machine-wide registration is presently required.
- Virtual camera creation depends on Windows privacy settings and OS version support.
- Device enumeration alone is not enough to prove frame delivery; black-preview issues can still exist in the sample allocator, event sequencing, or buffer layout path.

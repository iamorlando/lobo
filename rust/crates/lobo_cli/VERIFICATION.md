# Local verification · 2026-10-02

- Release build and Clippy with warnings denied passed; only the standalone application was formatted.
- Ten automatic Rust tests passed; the separate physical Metal/CPU pixel parity test passed. GPU buffer growth/shrink and alternating chart types were also verified. The visual QA artifacts were rendered and inspected.
- Real terminal tests passed on CPU and Metal: views, command input, simulation, resize, quit, alternate screen exit and exact terminal attribute restoration.
- Raw ITCH, gzip with no filename suffix, localhost HTTP gzip, exact volume splits, time-bar watermarks on non-trade records and corrupt/truncated-file rejection passed.
- An eight-second read of Kraken's public BTC/USD feed produced 200 depth levels, 979 messages and 964 checksum validations, with zero checksum failures.
- The repository's 13 GB Nasdaq ITCH file reconstructed AAPL through 09:30 and advanced playback in a two-second capture: 14,688,683 records / 469,502,796 input bytes, 514 active levels and 32 retained candles. This is a local smoke result, not a cross-machine performance guarantee.
- The macOS ARM64 release archive was built with the executable, completions and README. The executable ran with an empty environment and a system-only PATH. Its dynamic libraries are macOS system libraries/frameworks only.
- Homebrew checksum-generation tests passed and the release workflow YAML parsed successfully.

Linux ARM64 was subsequently built and tested locally in an isolated Ubuntu container (see below). Linux x86-64 was also cross-built and exercised in Ubuntu (see below). The native Intel Mac and Homebrew checks subsequently passed in CI; see the published release verification below.

## Shared multipane verification

- Four independent clients received byte-equivalent native state at matching published sequence numbers; attached CLI captures of candles, book, flow and orders also matched.
- Shared pause/play, symbol selection, bar aggregation, isolated L3 simulation, FIFO entry/cancellation, return to main and restart passed. Slow/nonreading panes did not stall the publisher (30 frames published in 0.5 seconds).
- Shared order entry carries the displayed instrument and session revision. A cancellation from a stale pane was rejected after switching instruments; the same cancellation succeeded after refreshing the intended instrument and queue.
- Four real tmux panes rendered through Metal. A simulation entered in one pane appeared in another; all panes showed the same paused source clock. Closing one pane left the remaining charts and session running.
- Four simultaneous 90 × 25 real terminal streams produced 180 frames each in 3.029–3.049 seconds (59.0–59.4 fps including startup), including ANSI writes, with exact terminal attribute restoration.
- A color-output regression caused by inherited `NO_COLOR` was found during visual QA and fixed. Terminal and tmux checks now assert RGB foreground/background output; the actual tmux capture was rendered and visually inspected.
- The repository's real Nasdaq replay was read once by a shared session, reconstructed past 09:30, and produced identical native snapshots in four attached chart views. The observed state contained 15,613,029 processed records / 501,340,574 input bytes and 610 depth levels. This is a local smoke result.
- Local Metal benchmarks on an Apple M2 Max, at 90 × 25 with that populated Nasdaq book, measured medians of 0.52 ms for candles, 0.48 ms for depth, 0.64 ms for simulation and 0.92 ms for the dashboard. These include state access, layout, raster and GPU readback and exclude terminal escape output; they are not universal frame-rate guarantees.
- Session socket permissions (0600), rejection of existing regular files, graceful command stop/SIGTERM cleanup, and retained feed-error propagation were implemented. Stop/SIGTERM and file-protection behavior were exercised by the shared-session test.

## Linux ARM64 verification

The current release was built with pinned Rust 1.96.0 inside Ubuntu 22.04 ARM64,
with the repository mounted read-only. Ten automatic tests, raw/gzip/HTTP replay,
shared-session fanout/commands/cleanup, real terminal controls/restoration, four
simultaneous terminal streams and the actual tmux layout all passed on Linux.
The four simultaneous CPU terminal streams ran at 59.6–60.0 fps including startup.
The standalone executable was then mounted into a fresh Ubuntu 22.04 image with
no additional packages installed and successfully produced its native L3 state.
Dynamic linkage was limited to glibc, libm, libgcc_s and the system loader.
Linux GPU hardware was unavailable in this container; physical GPU verification
was performed on Metal on the Mac.

## Linux x86-64 and Intel Mac artifacts

Linux x86-64 was cross-built from Ubuntu 22.04 ARM64 with its own target C
headers, then executed in a clean Ubuntu x86-64 image without runtime installs.
Its raw/gzip/HTTP, shared session, terminal/restoration and tmux checks passed;
four simultaneous terminal streams ran at 57.5–57.7 fps including startup under
Docker architecture emulation. The real archive was generated inside that
x86-64 environment, including its executable version/completions checks.

The Intel Mac target cross-compiled successfully with deployment target 12.0
and only macOS system framework/library dependencies. The local Intel Mac
archive contains a universal executable with the actual compiled x86-64 and
ARM64 slices; its ARM64 slice was exercised by packaging on this machine.
The local Intel slice was not executed: Rosetta is absent. Native Intel Mac CI
subsequently passed and produced the thin Intel executable used in the release.
No Rosetta or global toolchain installs were made.

All four actual platform archives are now present locally. The Homebrew formula
and SHA256SUMS were derived from their real bytes, without placeholder hashes.
The published release uses the subsequent native CI artifacts, rather than the
local cross-build artifacts.

## Actual Homebrew installation

The release publisher now generates four relocatable bottles as well as the
native archives. A plain binary formula initially entered Homebrew's source
installation path and required developer tools; the bottle formula corrected
that requirement.

A separate Homebrew prefix, cache, build directory and trust store were created
under `/private/tmp`. With only this local formula trusted and its URLs pointing
to the actual local archives, `brew install lobo` poured the ARM64 bottle and
installed the executable, tmux launcher, shell completions and documentation,
without a source build or compiler dependency. The installed executable passed
the formula's version, L3, bid and ask assertions when run directly.

`brew test lobo` itself was blocked before invoking those assertions by
Homebrew's generic test-environment check for outdated Command Line Tools on
this pre-release macOS 27 host. No developer tools were upgraded and no check
was bypassed. The workflow now runs actual bottle installation and `brew test`
on both supported ARM and Intel macOS 15 runners. Both remote installation and
formula tests subsequently passed.

## Published release verification

[CI run 36979055856](https://github.com/iamorlando/lobo/actions/runs/36979055856)
passed all seven jobs for commit `090cc6be07f4ecdd91b7cf8f4b4f0e2ee6c95f58`:
native ARM64 and Intel Mac builds, native ARM64 and x86-64 Linux builds,
checksum-verified bottle packaging, and actual Homebrew installation and formula
tests on both Mac architectures. The native checks included replay, shared
sessions, stale-order rejection, real terminal streams and terminal restoration.

All eight native archives and bottles were downloaded and verified against the
CI SHA256SUMS before publication. The exact Apple Silicon release executable was
also exercised on physical Metal: four simultaneous 90 × 25 terminal streams,
180 frames each, ran at 51.2–58.3 fps including startup and ANSI output, with exact
terminal restoration.

[Terminal release 0.1.0](https://github.com/iamorlando/lobo/releases/tag/cli-v0.1.0)
and the [Homebrew tap](https://github.com/iamorlando/homebrew-lobo) are published.
Release artifacts are the unmodified, tested CI artifacts. This later
documentation update does not change application source or binary bytes.

The public tap was then fetched into the separate temporary Homebrew prefix and
`brew install lobo` downloaded and poured the public Apple Silicon bottle with
no additional formula dependencies. The installed executable's SHA256 matched
the exact CI executable. It produced an AAPL L3 snapshot (bid 99.99, ask 100.01)
with an empty environment and system-only PATH, and reported `lobo 0.1.0`.

## Initial workspace relocation and agent surface — 2026-10-02

Verified locally on macOS ARM64 from `rust/crates/lobo_cli` as the `lobo-cli`
workspace member, using the root lockfile. All prior workspace dependency
versions were retained. Existing Rust library source files were not modified.

- `cargo test -p lobo-cli --locked --offline`: 14 passed; the two existing opt-in
  GPU/visual-artifact tests were not run in this pass.
- `cargo fmt -p lobo-cli --check` and CLI-scoped Clippy (`--all-targets --no-deps
  -- -D warnings`) passed. Broader Clippy encounters existing doc-comment
  warnings in `lobo_primitives`; that library was left unchanged.
- Optimized `--profile cli-release` build passed. Native archive creation, SHA256,
  extraction and running the extracted executable outside the checkout passed.
- CPU PTY, raw/gzip/HTTP ITCH, shared session, four attached terminals and tmux
  smoke tests passed. Local-socket fixtures ran outside the filesystem sandbox.
- JSON Schema validation passed against real simulator snapshots, session frames,
  requests and acknowledgements. All ten presets ran against the offline demo;
  Bitfinex/Kraken were overridden to demo for deterministic preset tests.
- Native skill installation and the available Vercel Skills CLI 1.7.0 local
  installation produced copies matching the canonical skill byte-for-byte,
  including LICENSE and references. Existing edits and symlinks were refused.
- Skill frontmatter validation, local documentation links, generated resource
  freshness, packaging tests and Rust release-tool tests passed.

Discovery exposes ten presets, 27 interactive commands and 56 documentation/API
resources. Both `--help` and the interactive help panel mention `completions api`.
The multi-platform CI matrix remains configured; other target architectures were
not built locally in this pass.

## Independent application workspace — 2026-10-02

The application remains at `rust/crates/lobo_cli/`, alongside the library crates,
and is now explicitly excluded from the root workspace. Its manifest declares
its own workspace, dependencies and `cli-release` profile; its own `Cargo.lock`
is authoritative. Cargo metadata confirmed that the main workspace excludes the
app and the app workspace contains only `lobo-cli`. The main library lockfile
matches the pre-CLI repository lockfile byte-for-byte.

- All 14 app tests passed (10 unit and four agent-surface integration tests).
- App-scoped formatting and Clippy with warnings denied passed.
- Locked offline dependency resolution and the optimized app build passed.
- Both packaging tests and native archive creation from the new path passed.
- The optimized binary produced its native AAPL L3 snapshot; real terminal
  controls/resize/restoration and four identical shared-session streams passed.
- CI build/cache/archive paths, project documentation and generated terminal
  skill resources were updated for the independent crate; resource freshness passed.

The source, build instructions and release tooling are in the project. No app
source, manifest or packaging script depends on the temporary Homebrew test paths.

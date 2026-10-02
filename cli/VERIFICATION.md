# Local verification · 2026-10-02

- Release build and Clippy with warnings denied passed; only the standalone application was formatted.
- Ten automatic Rust tests passed; the separate physical Metal/CPU pixel parity test passed. GPU buffer growth/shrink and alternating chart types were also verified. The visual QA artifacts were rendered and inspected.
- Real terminal tests passed on CPU and Metal: views, command input, simulation, resize, quit, alternate screen exit and exact terminal attribute restoration.
- Raw ITCH, gzip with no filename suffix, localhost HTTP gzip, exact volume splits, time-bar watermarks on non-trade records and corrupt/truncated-file rejection passed.
- An eight-second read of Kraken's public BTC/USD feed produced 200 depth levels, 979 messages and 964 checksum validations, with zero checksum failures.
- The repository's 13 GB Nasdaq ITCH file reconstructed AAPL through 09:30 and advanced playback in a two-second capture: 14,688,683 records / 469,502,796 input bytes, 514 active levels and 32 retained candles. This is a local smoke result, not a cross-machine performance guarantee.
- The macOS ARM64 release archive was built with the executable, completions and README. The executable ran with an empty environment and a system-only PATH. Its dynamic libraries are macOS system libraries/frameworks only.
- Homebrew checksum-generation tests passed and the release workflow YAML parsed successfully.

Linux ARM64 was subsequently built and tested locally in an isolated Ubuntu container (see below). Linux x86-64 was also cross-built and exercised in Ubuntu (see below). Intel Mac execution and published Homebrew installation remain release checks. Publishing the draft release and placing the generated formula in a tap remains a release step.

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
The Intel slice was not executed: Rosetta is absent. CI on an Intel Mac is still
the native execution gate. No Rosetta or global toolchain installs were made.

All four actual platform archives are now present locally. The Homebrew formula
and SHA256SUMS were derived from their real bytes, without placeholder hashes.
The release and Homebrew tap have not been published.

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
on both supported ARM and Intel macOS 15 runners. Those remote jobs have not
been executed in this session. The release and tap remain unpublished.

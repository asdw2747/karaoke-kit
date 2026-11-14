# Third-Party Notices

KaraokeKit itself is licensed under the MIT License (see `LICENSE`). Release
binaries also contain third-party fonts and Rust crates under their own
licenses. This file records those components and where their license texts are
kept.

FFmpeg is **not** bundled. KaraokeKit invokes the FFmpeg installed on the user's
system (or the path in `KARAOKEKIT_FFMPEG`); FFmpeg's own license applies to that
separate program.

## Bundled fonts

| Font | Used by | License | Text |
| --- | --- | --- | --- |
| DejaVu Sans | KaraokeKit's lyric font (`assets/fonts/DejaVuSans.ttf`) | Bitstream Vera / DejaVu | `assets/fonts/LICENSE-DejaVu.txt`, `licenses/DejaVu.txt` |
| Hack Regular | `epaint_default_fonts` (egui monospace fallback) | MIT | `licenses/Hack-MIT.txt` |
| Ubuntu Light | `epaint_default_fonts` (egui proportional fallback) | Ubuntu Font Licence 1.0 | `licenses/Ubuntu-font-1.0.txt` |
| egui icons | `epaint_default_fonts` (subset of emoji-icon-font) | MIT | `licenses/emoji-icon-font-MIT.txt` |
| Noto Emoji | `epaint_default_fonts`, only with the optional `monochrome_emoji_fonts` feature | SIL Open Font License 1.1 | `licenses/OFL-1.1.txt` |

The Hack and Ubuntu fonts are embedded by the default `epaint_default_fonts`
feature enabled through `eframe`/`egui`. The Noto Emoji font is not enabled by
KaraokeKit's default feature set, but its license text is included because the
upstream package declares it.

## Rust dependencies

The complete dependency graph and exact versions are pinned in `Cargo.lock`.
The majority of crates are dual-licensed `MIT OR Apache-2.0`; KaraokeKit relies
on the MIT option where available. The license families relevant to
redistribution are:

### Mozilla Public License 2.0 (MPL-2.0)

The Symphonia audio decoding crates are MPL-2.0:

- `symphonia`
- `symphonia-bundle-flac`
- `symphonia-bundle-mp3`
- `symphonia-codec-pcm`
- `symphonia-codec-vorbis`
- `symphonia-core`
- `symphonia-format-ogg`
- `symphonia-format-riff`
- `symphonia-metadata`
- `symphonia-utils-xiph`

KaraokeKit uses these crates unmodified as library dependencies. Their source is
available from crates.io (https://crates.io/crates/symphonia) and the exact
versions are pinned in `Cargo.lock`. The full MPL-2.0 text is in
`licenses/MPL-2.0.txt`.

### Apache License 2.0

Many GUI, GPU, audio, and platform crates are `Apache-2.0` or
`MIT OR Apache-2.0`, including `cpal`, `eframe`, `egui`, `wgpu`, `winit`, and
their transitive dependencies. The full Apache-2.0 text is in
`licenses/Apache-2.0.txt`.

### MIT, BSD, ISC, Zlib, BSL-1.0, and public-domain-style licenses

The remaining crates use permissive licenses (MIT, BSD-2-Clause, BSD-3-Clause,
ISC, Zlib, BSL-1.0, Unicode-3.0, or public-domain dedications). Their copyright
notices and license terms are retained in the corresponding crate packages
listed in `Cargo.lock`.

## Regenerating this information

To list the resolved licenses for the current lockfile:

```bash
cargo metadata --format-version 1
```

For a fully automated, per-crate notice bundle, run a tool such as
[`cargo-about`](https://github.com/EmbarkStudios/cargo-about) or
[`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny) against
`Cargo.lock` before publishing a release.

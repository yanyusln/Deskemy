# AGENTS.md

Guidance for AI coding agents working in this repository. Read this before
editing; deeper background lives in `docs/`.

## What this is

Deskemy — an offline player for downloaded video courses (Windows first, Linux
beta). Rust + [Slint](https://slint.dev) UI on Skia/OpenGL, playback through
libmpv via runtime FFI, storage in a local SQLite database (rusqlite + FTS5).
Course files are referenced in place; they are never copied or uploaded.

## Repository layout

| Path | What |
|---|---|
| `app/` | The Slint app (`deskemy-app`, binary `deskemy`). Run Cargo from here. |
| `app/src/*.rs` | One module per page/feature controller (`library.rs`, `course_page.rs`, `video.rs`, `session.rs`, `settings.rs`, `importing.rs`, …). `main.rs` wires globals and callbacks. |
| `app/ui/*.slint` | UI: `app.slint` declares the window and globals (`Nav`, `Library`, `Import`, `Playback`, `Course`, `Pdf`, `Search`, `Tracks`, `LearningStats`, `Prefs`, `Updates`); one file per page plus `theme.slint`. |
| `app/ui/icons/`, `app/ui/fonts/` | Bundled SVG icons (Lucide, Material) and Inter. |
| `crates/deskemy-core/` | UI-agnostic core: `db/`, `importer/`, `courses.rs`, `config.rs`, `backup.rs`, `mpv/` (libmpv FFI), errors. Run Cargo from here. |
| `app/scripts/` | `get-pdfium.ps1/.sh`, `package.ps1`, `package-appimage.sh`, `make-qa-course.ps1`. |
| `docs/` | `releasing.md`, `release-qa.md`, `linux-port.md`, `slint-port.md`, `roadmap.md`, `release-notes/<version>.md`. `docs/archive/` is frozen history — don't update it. |

This is **not a Cargo workspace** (on purpose: each of `app/` and
`crates/deskemy-core/` has its own `Cargo.toml`, lockfile and `target/`).

## Build and test

```sh
# from app/
cargo run                     # development window (needs app/vendor/libmpv-2.dll on Windows)
cargo check --all-targets
cargo test                    # 100+ unit tests; playback tests use a headless mpv
cargo run -- --snapshot out.png          # offscreen frame, software renderer (layout checks)
cargo run -- --snapshot-player out.png   # player overlay with sample state

# from crates/deskemy-core/
cargo test
```

- Native libraries: `build.rs` copies `libmpv-2.dll` and `pdfium.dll` from
  `app/vendor/` beside the exe. PDFium: `powershell -File app/scripts/get-pdfium.ps1`.
- Use a scratch library for manual/QA runs: `DESKEMY_DATA_DIR=<dir>`.
- Other useful env: `DESKEMY_RENDERER`, `DESKEMY_HWDEC=no`,
  `DESKEMY_LIBMPV`, `DESKEMY_WAYLAND=1`, `DESKEMY_REQUIRE_LIBMPV=1`
  (CI: fail instead of skipping playback tests).
- CI (`.github/workflows/slint-linux.yml`) runs core tests, then
  `cargo build && cargo test` in `app/`, then a `--snapshot` smoke test.

## Architecture rules

- **Slint ↔ Rust**: `.slint` declares state and callbacks; Rust modules read
  `ui.global::<X>()`, set display strings/collections, and assign handlers.
  Keep business logic out of `.slint`.
- **Add a `.slint` file** → also add its path to the explicit
  `rerun-if-changed` list in `app/build.rs`, and include it from `ui/app.slint`.
- **Internal identifiers stay English**: page keys (`library`, `course`,
  `player`, `settings`, …), lecture/course status (`Ready`, `Missing`, `Error`,
  `Scanning`, `Importing`) and resource kinds (`pdf`, `text`, `archive`,
  `folder`, `link`, `code`, `web`, `file`) are used in conditionals on both
  sides. Do not translate them; translate only at the display layer (Slint
  `status == "Missing" ? "缺失" : …`).
- Import is two-phase (probe, then persist) so media probing stays off the
  database connection; follow that when touching `importing.rs` or the core
  importer.
- `app/patches/i-slint-renderer-skia` must match the `slint` version in
  `app/Cargo.toml` (currently 1.18).
- Never change the MSI upgrade code in `app/installer/main.wxs`
  (see `docs/releasing.md`).

## UI text: Simplified Chinese, no i18n

- There is **no i18n framework**. All user-visible copy is Simplified Chinese,
  hardcoded in `.slint` and in the Rust display layer (`settings.rs`,
  `course_page.rs`, `importing.rs`, …). New UI text must be Chinese.
- Keep the brand **Deskemy**, keyboard key names (`Space / K`, `Esc`), icon
  asset paths, and `@image-url` unchanged.
- Logs (`tracing`), CLI output, and internal error plumbing may stay English;
  errors surfaced in the UI should be Chinese (including `thiserror` messages
  in `crates/deskemy-core/src/error.rs`).
- Slint strings use `\{expr}` interpolation and `cond ? "a" : "b"`; Chinese has
  no plural form, so format counts as `"{n} 个课时"`.
- Dates: `%Y年%-m月%-d日` (short: `%-m月%-d日`). Durations use the shared
  helpers (`format_duration`) — don't hand-roll units.
- **When you change any visible string, update the unit tests that pin it**
  (`settings::tests`, `importing::tests`, `stats_page::tests`, `library`,
  `tracks`, …). Grep for the old text before you finish.

## Code style

- Match the surrounding style; keep changes minimal and scoped.
- Rust: edition 2021, standard formatting, `tracing` for diagnostics.
- Comments in code and `docs/` are in English; Chinese is for user-facing
  strings only.
- Don't introduce dependencies without a clear need; the app ships bundled
  (libmpv, PDFium) and must stay fully offline apart from the update check.

## Verifying a change

1. `cargo check --all-targets` (from `app/`) — catches Slint syntax errors.
2. `cargo test` in the touched crate(s); core tests are fast, app tests take
   longer on first build.
3. For UI/layout work, render an offscreen frame with `--snapshot` (and
   `--snapshot-player` for the player) and inspect the PNG.
4. Manual smoke test for player/mini-player changes: `mini.rs`, `caption.rs`,
   `video.rs`, `session.rs` are high-risk per `docs/release-qa.md`.

## Git and releases

- Commit messages: short imperative, area prefix, no trailing period, e.g.
  `Import: names downloaders mangled are tidied`, `Player: an A–B loop`,
  `docs: the importer sweep in the roadmap`.
- Version lives in `app/Cargo.toml`. A release also needs
  `docs/release-notes/<version>.md`, a `<release>` entry at the top of
  `app/linux/io.github.NFRohan.Deskemy.metainfo.xml`, and packaging via
  `powershell -File app/scripts/package.ps1 -Notes "…"`
  (details: `docs/releasing.md`).
- Before shipping, run `docs/release-qa.md` on the exact packaged builds.

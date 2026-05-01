# Repository Guidelines

## Project Structure & Module Organization
This repository is a Rust workspace. Top-level manifests are `Cargo.toml` and `Cargo.lock`.

- `crates/core/`: shared library (`mpvpaper-rs-core`) for CLI parsing, Wayland, rendering, MPV integration, control threads, and config handling.
- `crates/mpvpaper-rs/`: main binary entrypoint (`mpvpaper-rs`).
- `crates/holder/`: helper binary (`mpvpaper-rs-holder`) used for auto-stop recovery.
- `README.md`: install/usage docs.
- `.github/PULL_REQUEST_TEMPLATE.md`: required PR checklist.

Keep new modules under the appropriate crate (`core` for reusable logic, binary crates for startup/wiring only).

## Build, Test, and Development Commands
Use workspace-wide Cargo commands from the repo root:

- `cargo build --workspace`: build all crates.
- `cargo build --release`: produce optimized binaries in `target/release/`.
- `cargo test --workspace`: run unit tests across crates.
- `cargo clippy --workspace --all-targets -- -D warnings`: lint and fail on warnings.
- `cargo fmt --all -- --check`: verify formatting.
- `cargo run -p mpvpaper-rs -- -d`: run main app and list available outputs.

## Coding Style & Naming Conventions
Follow Rust 2021 defaults and `rustfmt` output (4-space indentation, trailing commas where applicable).

- Use `snake_case` for modules, files, functions, and variables.
- Use `PascalCase` for structs/enums/traits.
- Keep binaries thin; move reusable behavior into `crates/core`.
- Prefer explicit, typed error propagation with existing `Result`/error patterns in `crates/core/src/error.rs`.

## Testing Guidelines
Unit tests are colocated with source files using `#[cfg(test)]` modules (for example in `crates/core/src/config.rs` and `crates/core/src/mpv/options.rs`).

- Add focused tests for new parsing, config, and control-path logic.
- Name tests by behavior, e.g. `parses_mpv_options_with_quotes`.
- Run `cargo test --workspace` before opening a PR.

## Commit & Pull Request Guidelines
Recent history uses concise, imperative subjects, often with conventional prefixes (`chore:`, etc.) and optional emoji. Keep subject lines short and scoped.

PRs should follow `.github/PULL_REQUEST_TEMPLATE.md`:

- fill in Summary and Change Type,
- mark related project phase when applicable,
- confirm `build`, `clippy`, `fmt`, and tests,
- link related issues and include reproducible steps/logs for behavior changes.

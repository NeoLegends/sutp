# SUTP

The reference implementation of SUTP written in Rustlang.

Uses asynchronous I/O based on the tokio stack.

## Rustlang

If you haven't done so already, install the rust language compiler via https://rustup.rs/ (or via Homebrew or via your favourite package manager of choice).

This project is built against the latest stable version of rust (1.31 at the time of writing). Your mileage may vary with older compilers. Updating the rust compiler, however, is very easy with rustup.

## Compiling & Running tests

Run `cargo build` or `cargo build --release` in this directory to compile the programs. The binaries can be found in `../target/{debug,release}`.

Tests can be run with `cargo test` or `cargo test --release`. Running `cargo test` will also compile the project if anything has changed (or there haven't been any previous compiler runs).

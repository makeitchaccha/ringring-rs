# ringring-rs

## Build & Test

This project uses Nix for its development environment. **Do not invoke `cargo`, `rustc`, `gcc`, or `clippy` directly** — they are not on PATH outside of `nix develop`.

Always prefix build/test commands with `nix develop --command`:

```sh
nix develop --command cargo check --no-default-features
nix develop --command cargo clippy --no-default-features
nix develop --command cargo test --no-default-features
nix develop --command cargo build --no-default-features
```

The default `jemalloc` feature fails to build on this nixos-unstable (glibc 2.44) due to a jemalloc configure incompatibility. Use `--no-default-features` for all dev commands.

## Project Structure

- `src/room/` — domain model (Room, Participant, Interval) + actors (Coordinator, Session)
- `src/reporting/` — Publisher, Reporter, transformer, types
- `src/graphics/` — timeline rendering (tiny-skia + cosmic-text)
- `src/infrastructure/` — AssetProvider (avatar fetch + k-means color extraction)
- `src/presentation/` — Discord event handler (VoiceHandler)

## Review Policy

When reviewing code (via subagent or otherwise), **report findings only**. Do not modify source files during a review. Fixes should be applied separately after the user approves them.

## Search Policy

Do NOT run broad filesystem searches (`find /`, `find ~`, etc.). Use targeted tools:
- `grep` for text search within the project
- `read` for reading specific files
- `glob` for file pattern matching within the project
- Known paths: cargo registry is at `~/.cargo/registry/src/`, git checkouts at `~/.cargo/git/checkouts/`
- If you need to find a type definition, use `grep -rn "pub enum Foo" ~/.cargo/registry/src/` with a specific crate name

Never run `find` on `/` or `~`. It is slow and unnecessary.

## Key Design Decisions

- Room is a pure state machine (Instant-injected, no I/O)
- Session/Coordinator are async actors communicating via channels
- History uses Arc<Vec> for Copy-on-Write sharing with Reporter
- PreUpdate event + broadcast capacity 1 enables CoW optimization
- IdleTimer replaced by IdleTracker (state only) + tokio::time::sleep_until
- disconnect is Err-based (not idempotent) to preserve transition tracking
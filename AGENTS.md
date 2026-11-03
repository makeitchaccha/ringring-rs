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

## Key Design Decisions

- Room is a pure state machine (Instant-injected, no I/O)
- Session/Coordinator are async actors communicating via channels
- History uses Arc<Vec> for Copy-on-Write sharing with Reporter
- PreUpdate event + broadcast capacity 1 enables CoW optimization
- IdleTimer replaced by IdleTracker (state only) + tokio::time::sleep_until
- disconnect is Err-based (not idempotent) to preserve transition tracking
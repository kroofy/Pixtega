# Contributing to Pixtega

Thanks for your interest in contributing. Pixtega (crate and binary name:
`image-service`) is an on-demand image derivation service; the normative
behavioral contract is [SPEC.md](SPEC.md), and changes must stay within it.

## Building

The service links against libvips, so install the native dependencies first:

```bash
# Debian/Ubuntu
sudo apt-get install libvips-dev libheif-plugin-aomenc pkg-config

# macOS
brew install vips pkg-config
```

Then a plain cargo build works; the toolchain is pinned by
[rust-toolchain.toml](rust-toolchain.toml):

```bash
cargo build
cargo run -- config.local.toml
```

## Testing

One command runs everything CI runs — formatting, clippy (warnings are
errors), and unit plus integration tests:

```bash
./scripts/check.sh
```

Tests run entirely against loopback fixture servers (including local
S3-compatible and TLS fixtures) and need no credentials or public network
access. `./scripts/container-acceptance.sh` additionally exercises the
Docker image end to end.

For behavior changes, consider mutation testing (`cargo install
cargo-mutants && cargo mutants`); configuration and exclusions live in
[.cargo/mutants.toml](.cargo/mutants.toml), and the semantic-mutant manifest
is in [docs/semantic-mutants.md](docs/semantic-mutants.md).

## Pull request expectations

- `./scripts/check.sh` passes.
- Behavior stays within [SPEC.md](SPEC.md). If a change genuinely requires
  amending the contract, update SPEC.md in the same PR and say so
  explicitly in the description.
- New behavior comes with tests. Error-path changes should map to the
  existing error taxonomy and outcome set rather than inventing new ones.
- Documentation (README, `website/`, `deploy/`) is updated when
  user-visible behavior or operations change. Docs must describe what the
  code actually does — do not document aspirational features.
- Keep commits focused; one logical change per commit.

## Repository notes

- `SPEC.md` and `open-source-image-service-benchmark-spec.md` are currently
  identical copies: the former is the project's contract, the latter is the
  benchmark packaging of it. Treat `SPEC.md` as canonical; the duplicate is
  kept intentionally for the benchmark and may be consolidated later.
- The project name is Pixtega; the Rust crate and binary remain
  `image-service`.

## License

By contributing, you agree that your contributions are dual-licensed under
MIT OR Apache-2.0, matching the project
([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)).

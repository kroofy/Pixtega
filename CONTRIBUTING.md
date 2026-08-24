# Contributing to Pixtega

Thanks for your interest in contributing. Pixtega (crate and binary name:
`pixtega`) is an on-demand image derivation service; its behavior (URL
contract, caching, error taxonomy, configuration) is documented in the
[README](README.md) and enforced by the test suite, and changes must stay
within it.

## Building

The service links against libvips, so install the native dependencies first.
libvips 8.17 or newer is required (the pinned `libvips = "=2.3.0"` bindings
pass argument names introduced in 8.17), so use Ubuntu 26.04+ or build
libvips from source on older distributions:

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

## CI

Every PR runs the CI workflow ([docs/ci.md](docs/ci.md)): the `check` job
is `./scripts/check.sh` verbatim and the `container` job is
`./scripts/container-acceptance.sh` verbatim, so a green local run means a
green CI run. The project mutation-score threshold (≥ 90%) is enforced by
the separate Mutants workflow (weekly on `main` + manual dispatch — it
takes hours, so it does not gate PRs); after merging behavior changes,
trigger it from the Actions tab to refresh the recorded score. Releases
are cut with `git tag vX.Y.Z && git push origin vX.Y.Z`; see
[docs/ci.md](docs/ci.md).

## Pull request expectations

- `./scripts/check.sh` passes.
- Behavior stays within the documented contract ([README](README.md) and
  `docs/`). If a change genuinely amends the contract, update the README
  and docs in the same PR and say so explicitly in the description.
- New behavior comes with tests. Error-path changes should map to the
  existing error taxonomy and outcome set rather than inventing new ones.
- Documentation (README, `website/`, `deploy/`) is updated when
  user-visible behavior or operations change. Docs must describe what the
  code actually does — do not document aspirational features.
- Keep commits focused; one logical change per commit.

## License

By contributing, you agree that your contributions are licensed under the
MIT license, matching the project ([LICENSE](LICENSE)).

# CI, mutation score, and releases

Three GitHub Actions workflows, all driven only by the built-in
`GITHUB_TOKEN` — no extra repository secrets.

## CI (`.github/workflows/ci.yml`)

Runs on every pull request and every push to `main`. Two jobs:

- **check** — `./scripts/check.sh` verbatim: `cargo fmt --check`,
  `cargo clippy -D warnings`, `cargo test` (unit + integration), on the
  toolchain pinned by [rust-toolchain.toml](../rust-toolchain.toml)
  (rustup installs the pin; CI cannot drift from it). Cargo registry and
  build artifacts are cached with `Swatinem/rust-cache`.
- **container** — `./scripts/container-acceptance.sh` verbatim: builds the
  repository Dockerfile, runs the image, and asserts a 200 `image/webp`
  WebP body from the fixtures success case.

Both jobs run on `ubuntu-24.04` pinned (not `ubuntu-latest`): noble's
libvips 8.15 matches the pinned `libvips = "=1.6.1"` bindings and its
libheif ships the aom AV1 encoder plugin needed for AVIF — the same
reasoning as the Dockerfile base image.

## Mutation score (`.github/workflows/mutants.yml`)

The project threshold for the mutation score is at least 90%. A full
`cargo mutants` run rebuilds and retests the crate
once per mutant and can take hours, so it does not gate pull requests.
Instead it runs weekly on `main` and on demand (Actions tab → Mutants →
Run workflow), using the checked-in
[.cargo/mutants.toml](../.cargo/mutants.toml) (exclusions with written
reasons, bounded timeouts).

Each run produces the recorded, authoritative numbers:

- **Job summary** on the run page: score, verdict, and a
  caught/missed/timeout/unviable table with git SHA and timestamp.
- **`mutants-report` artifact**: the full `mutants.out/` directory
  (missed-mutant diffs, `outcomes.json`, per-category lists) plus
  `mutants-score.json` — a machine-readable record
  (`score_percent`, counts, `threshold_percent`, `passed`, `git_sha`,
  `timestamp_utc`).
- **Exit status**: the workflow fails when the score is below 90%, so the
  workflow badge doubles as the threshold check.

The score is computed by
[.github/scripts/mutants-score.py](../.github/scripts/mutants-score.py) as
`caught / (caught + missed + timeout)`. Unviable mutants (mutated code that
does not compile) are excluded, per the usual definition. Timeouts count
against the score: a mutant that only dies to the harness timeout was not
killed by an assertion.

For the latest recorded score, open the most recent successful **Mutants**
run under Actions and read its job summary or download `mutants-report`.

### Runtime budget

The job is capped at 355 minutes, just under the 360-minute GitHub-hosted
hard limit, and runs `cargo mutants --jobs 2` (two parallel mutant builds;
higher values thrash the 4-vCPU runner). If the suite outgrows the cap,
shard the run across parallel jobs with `cargo mutants --shard k/n` and
merge the per-shard counts — prefer keeping one authoritative unsharded run
as long as it fits.

## Releases (`.github/workflows/release.yml`)

Cut a release by pushing a semver tag:

```bash
git tag vX.Y.Z && git push origin vX.Y.Z
```

The workflow then:

1. Builds and pushes OCI images (linux/amd64) to
   `ghcr.io/kroofy/pixtega`:
   - [Dockerfile](../Dockerfile) → `X.Y.Z`, `X.Y`, `latest`
   - [Dockerfile.lambda](../Dockerfile.lambda) → `X.Y.Z-lambda`, `lambda`
2. Builds the release binary
   (`pixtega-vX.Y.Z-x86_64-unknown-linux-gnu-ubuntu24.04.tar.gz`) with a
   `SHA256SUMS` file. The binary is dynamically linked against Ubuntu 24.04
   libvips (`libvips42t64` + `libheif-plugin-aomenc` at runtime); the
   container image is the recommended distribution.
3. Creates a GitHub Release with auto-generated notes (the repo keeps no
   CHANGELOG; notes come from merged PRs/commits since the previous tag)
   and the tarball + checksums attached.

Running the workflow manually (`workflow_dispatch`) is a rehearsal: it
builds the images and the binary but pushes nothing and creates no release.

Permissions (declared per job in the workflow):

- `packages: write` — push to GHCR. The GHCR package inherits repository
  visibility, so images on a private repo stay private.
- `contents: write` — create the GitHub Release.

Everything is MIT ([LICENSE](../LICENSE)); release artifacts bundle
`LICENSE` and `THIRD_PARTY_NOTICES.md`.

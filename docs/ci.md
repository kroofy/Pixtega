# CI, mutation score, and releases

Four GitHub Actions workflows. CI, Mutants, and Release use only the
built-in `GITHUB_TOKEN`. Website deploy also needs two repo secrets
(`CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_ACCOUNT_ID`).

## CI (`.github/workflows/ci.yml`)

Runs on every pull request and every push to `main`. Two jobs:

- **check** — `./scripts/check.sh` verbatim: `cargo fmt --check`,
  `cargo clippy -D warnings`, `cargo test` (unit + integration), on the
  toolchain pinned by [rust-toolchain.toml](../rust-toolchain.toml)
  (rustup installs the pin; CI cannot drift from it). Cargo registry and
  build artifacts are cached with `Swatinem/rust-cache`.
- **container** — builds the repository Dockerfile via buildx with
  GitHub Actions layer caching (`cache-from/cache-to: type=gha,mode=max`),
  then runs `./scripts/container-acceptance.sh` with `PIXTEGA_SKIP_BUILD=1`
  to start the image and assert a 200 `image/webp` WebP body from the
  fixtures success case. The Dockerfile compiles dependencies in a layer
  keyed on `Cargo.lock` alone, so source-only changes reuse the cached
  dependency layer; a `Cargo.lock` change pays one full recompile, then
  re-caches. Run locally without any env vars — the script then builds
  the image itself.

Both jobs run on `ubuntu-26.04` pinned (not `ubuntu-latest`): resolute's
libvips 8.18 matches the pinned `libvips = "=2.3.0"` bindings and its
libheif ships the aom AV1 encoder plugin needed for AVIF — the same
reasoning as the Dockerfile base image.

## Mutation score (`.github/workflows/mutants.yml`)

The project requires a mutation score of at least 90% (see the README).
A full `cargo mutants` run rebuilds and retests the crate
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
   (`pixtega-vX.Y.Z-x86_64-unknown-linux-gnu-ubuntu26.04.tar.gz`) with a
   `SHA256SUMS` file. The binary is dynamically linked against Ubuntu 26.04
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

## Website (`.github/workflows/website.yml`)

Two jobs, both running `npx --yes wrangler@4 deploy` from `website/` (no
build step). Wrangler reads `CLOUDFLARE_API_TOKEN` and
`CLOUDFLARE_ACCOUNT_ID` from repository secrets (Settings → Secrets and
variables → Actions); both must be set before the first green deploy.

- **deploy** — production. Deploys the static docs site (`website/`) to
  the existing `pixtega` Cloudflare Worker (pixtega.com) on push to
  `main` when `website/**` or the workflow file itself changes, passing
  `--env ""` to target the top-level environment explicitly.
  `workflow_dispatch` redeploys the current `main`. Never runs from
  PRs. `contents: read` only.
- **preview** — pull requests. On `pull_request` touching the same
  paths, deploys the PR's `website/` to one shared preview Worker
  (`pixtega-preview`, <https://preview.pixtega.com>) via
  `wrangler deploy --env preview`, then posts a sticky PR comment
  saying which PR and short SHA are live. The Worker is shared, so the
  most recent preview deploy from any PR wins. Skipped for PRs from
  forks, which cannot read the Cloudflare secrets. Needs
  `pull-requests: write` (sticky comment via `GITHUB_TOKEN`) in
  addition to `contents: read`.

Preview mechanics (the shared-Worker trade-off, custom-domain DNS
provisioning, token permissions) are in
[website/README.md](../website/README.md).

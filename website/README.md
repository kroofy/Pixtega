# Pixtega docs site

Static HTML under `public/`, served as a Cloudflare Workers assets site.

```bash
cd website
npx wrangler deploy
```

No build step. Edit the HTML files in `public/` directly. Content must stay
grounded in the root [README](../README.md) and the actual code; do not
document behavior the service does not have.

## Deploy

Merges to `main` that touch `website/**` (or `.github/workflows/website.yml`)
auto-deploy via that workflow. Manual redeploy: Actions → Website → Run
workflow.

Repo secrets (Settings → Secrets and variables → Actions), required before
the first green deploy:

- `CLOUDFLARE_API_TOKEN` — Edit Cloudflare Workers
- `CLOUDFLARE_ACCOUNT_ID`

CI runs `npx --yes wrangler@4 deploy` from `website/` (Wrangler 4 is current
stable). Local deploy above uses interactive `wrangler login`.

## PR previews

Pull requests touching `website/**` (or the workflow file) deploy to **one
shared preview Worker**, `pixtega-preview`
(`npx --yes wrangler@4 deploy --name pixtega-preview` — the `--name` flag
overrides `wrangler.jsonc`, so prod config is never touched). The workflow
posts a sticky PR comment (marker `<!-- pixtega-preview -->`) with the
`https://pixtega-preview.<subdomain>.workers.dev` URL, the PR number, and
the short SHA that was deployed.

One shared Worker, not one per PR: the Workers free tier has a limited
number of Worker (script) slots, so previews all share a single slot.
Consequence: **last deploy wins** — the most recent preview deploy from
*any* PR is what is live. The sticky comment's PR number + SHA tell you
which revision that is.

Previews reuse the same two secrets as prod and are skipped for PRs from
forks (fork PRs cannot read repo secrets). Production (`pixtega` /
pixtega.com) deploys only on push to `main`, never from PRs.

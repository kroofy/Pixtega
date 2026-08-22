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

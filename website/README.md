# Pixtega docs site

Static HTML under `public/`, served as a Cloudflare Workers assets site.

```bash
cd website
npx wrangler deploy
```

No build step. Edit the HTML files in `public/` directly. Content must stay
grounded in the root [README](../README.md) and [SPEC.md](../SPEC.md); do not
document behavior the service does not have.

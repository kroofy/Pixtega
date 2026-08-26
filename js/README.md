# Pixtega JavaScript client

Pure URL builders for [Pixtega](https://github.com/kroofy/Pixtega), the
on-demand image derivation service. Two packages, one implementation:

- [`pixtega`](pixtega/) — the implementation.
- [`@pixtega/url`](url/) — a thin re-export of `pixtega`, so the scoped
  namespace resolves to the same API. Install whichever name you prefer.

Both names are published to npm from each `v*` release tag of this
repository (see [docs/ci.md](../docs/ci.md)), straight from the
TypeScript source below — no separate build.

```bash
npm install pixtega
# or
npm install @pixtega/url
```

The client builds strings and never fetches. It rejects everything the
server's fixed URL grammar rejects (segment encoding, transform shape,
the `v` token alphabet), but it cannot know your server's configured
width and quality allowlists. A URL this client accepts can still get a
400: a width or quality outside the allowlist, or an explicit quality
equal to the format's default (omit `quality` to use the default).

## API

```js
import { pixtegaUrl, pixtegaSrcSet, pixtegaPicture } from "pixtega";

pixtegaUrl({
  base: "https://images.example.com",
  mount: "public",
  path: "photos/example.jpg", // decoded; also accepts ["photos", "example.jpg"]
  width: 1280,
  format: "webp", // "webp" | "avif" | "jpeg"
  quality: 85,    // optional; must be allowlisted and not the default
  v: "7d91c2",    // optional version token for immutable caching
  pathPrefix: "/images", // default
});
// => "https://images.example.com/images/public/photos/example.jpg/w1280,q85.webp?v=7d91c2"

pixtegaSrcSet({ base, mount, path, format: "webp", widths: [640, 1280] });
// => ".../w640.webp 640w, .../w1280.webp 1280w"

pixtegaPicture({ base, mount, path, widths: [640, 1280], formats: ["avif", "webp", "jpeg"] });
// => { sources: [{ type: "image/avif", srcset }, ...],
//      img: { src: ".../w1280.jpeg" } }
```

`pixtegaPicture` lists `sources` in the order given (browsers pick the
first supported type) and uses the last format for the fallback `img.src`,
so put the most widely supported format last.

## Development

The packages ship TypeScript source. Types are the `.ts` files. Node 22.6+
strips types on import; bundlers compile them as usual. No build step.

```bash
cd js
npm install   # workspace: links @pixtega/url against the local pixtega
npm test
```

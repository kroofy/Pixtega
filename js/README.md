# Pixtega JavaScript client

Pure URL builders for [Pixtega](https://github.com/kroofy/Pixtega), the
on-demand image derivation service. Two packages, one implementation:

- [`pixtega`](pixtega/) — the implementation.
- [`@pixtega/url`](url/) — a thin re-export of `pixtega`, so the scoped
  namespace resolves to the same API. Install whichever name you prefer.

```bash
npm install pixtega
# or
npm install @pixtega/url
```

The client builds strings and never fetches. It validates everything the
server's URL grammar fixes (segment encoding, transform shape, the `v`
token alphabet), but it cannot know your server's configured width and
quality allowlists; a URL this client accepts is still rejected with a 400
if the width or quality is not allowlisted, or if a quality equals the
format's default (omit `quality` to use the default).

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

```bash
cd js
npm install   # workspace: links @pixtega/url against the local pixtega
npm run build
npm test
```

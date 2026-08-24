# pixtega

URL builders for [Pixtega](https://github.com/kroofy/Pixtega), the
on-demand image derivation service. Pure string helpers; no fetching, no
dependencies. Also published as [`@pixtega/url`](https://www.npmjs.com/package/@pixtega/url),
which re-exports this package unchanged.

```bash
npm install pixtega
```

```js
import { pixtegaUrl, pixtegaSrcSet, pixtegaPicture } from "pixtega";

pixtegaUrl({
  base: "https://images.example.com",
  mount: "public",
  path: "photos/example.jpg",
  width: 1280,
  format: "webp",
  v: "7d91c2",
});
// => "https://images.example.com/images/public/photos/example.jpg/w1280.webp?v=7d91c2"

pixtegaSrcSet({ base, mount, path, format: "webp", widths: [640, 1280] });
// => ".../w640.webp 640w, .../w1280.webp 1280w"

pixtegaPicture({ base, mount, path, widths: [640, 1280], formats: ["avif", "webp", "jpeg"] });
// => { sources: [{ type, srcset }, ...], img: { src } }
```

Options for `pixtegaUrl`:

| Option | Meaning |
| --- | --- |
| `base` | Service origin, e.g. `"https://images.example.com"`. `""` gives relative URLs. |
| `mount` | Configured Source name, `[a-z][a-z0-9-]{0,31}`. |
| `path` | Decoded source path: `"a/b.jpg"` or `["a", "b.jpg"]`. Encoded here; do not pre-encode. |
| `width` | Integer 1..=16384, must be in the server's width allowlist. |
| `format` | `"webp"`, `"avif"`, or `"jpeg"` (no aliases). |
| `quality` | Optional. Must be in the format's quality allowlist and not equal its default; omit for the default. |
| `v` | Optional version token `[A-Za-z0-9._~-]{1,128}`; enables immutable caching. Change it when the source bytes change. |
| `pathPrefix` | Server's configured prefix, default `"/images"`. |

The client cannot know your server's width and quality allowlists, so a
URL it accepts can still get a 400. It does reject everything the fixed
URL grammar rejects (bad mounts, dot segments, control characters,
non-canonical values), and it percent-encodes source paths exactly the way
the server requires.

MIT, part of the [Pixtega repository](https://github.com/kroofy/Pixtega)
under `js/pixtega`.

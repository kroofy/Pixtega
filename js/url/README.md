# @pixtega/url

URL builders for [Pixtega](https://github.com/kroofy/Pixtega), the
on-demand image derivation service. This package re-exports
[`pixtega`](https://www.npmjs.com/package/pixtega) unchanged and is
published from the same release tag; install whichever name you prefer.

```bash
npm install @pixtega/url
```

TypeScript source, same as `pixtega`. Node 22.6+ strips types on import;
bundlers compile it as usual.

```js
import { pixtegaUrl, pixtegaSrcSet, pixtegaPicture } from "@pixtega/url";

pixtegaUrl({
  base: "https://images.example.com",
  mount: "public",
  path: "photos/example.jpg",
  width: 1280,
  format: "webp",
});
// => "https://images.example.com/images/public/photos/example.jpg/w1280.webp"
```

See the [`pixtega` package](https://www.npmjs.com/package/pixtega) for the
full API and option reference.

MIT, part of the [Pixtega repository](https://github.com/kroofy/Pixtega)
under `js/url`.

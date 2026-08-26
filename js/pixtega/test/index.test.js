import assert from "node:assert/strict";
import { test } from "node:test";

import { pixtegaPicture, pixtegaSrcSet, pixtegaUrl } from "pixtega";

const base = "https://images.example.com";

test("builds the README example URL", () => {
  assert.equal(
    pixtegaUrl({
      base,
      mount: "public",
      path: "photos/example.jpg",
      width: 1280,
      format: "webp",
      v: "7d91c2",
    }),
    "https://images.example.com/images/public/photos/example.jpg/w1280.webp?v=7d91c2",
  );
});

test("quality, custom prefix, trailing-slash base, array path", () => {
  assert.equal(
    pixtegaUrl({
      base: "https://cdn.example.com///",
      mount: "archive",
      path: ["photos", "2024", "example.jpg"],
      width: 640,
      format: "jpeg",
      quality: 85,
      pathPrefix: "/img",
    }),
    "https://cdn.example.com/img/archive/photos/2024/example.jpg/w640,q85.jpeg",
  );
});

test("empty base yields a same-origin relative URL", () => {
  assert.equal(
    pixtegaUrl({ base: "", mount: "public", path: "a.jpg", width: 640, format: "avif" }),
    "/images/public/a.jpg/w640.avif",
  );
});

test("percent-encodes non-ASCII with uppercase hex, keeps unreserved literal", () => {
  const url = pixtegaUrl({
    base,
    mount: "fixtures",
    path: ["release.2024", "café photo.jpg"],
    width: 640,
    format: "webp",
  });
  assert.equal(
    url,
    `${base}/images/fixtures/release.2024/caf%C3%A9%20photo.jpg/w640.webp`,
  );
});

test("rejects inputs the server would reject", () => {
  const ok = { base, mount: "public", path: "a.jpg", width: 640, format: "webp" };
  const cases = [
    { ...ok, mount: "Public" },
    { ...ok, mount: "9lives" },
    { ...ok, path: "" },
    { ...ok, path: ["photos", ""] },
    { ...ok, path: [".."] },
    { ...ok, path: ["a\\b"] },
    { ...ok, path: ["dir/file"] },
    { ...ok, path: ["a\u0000b"] },
    // second decoding would introduce a path delimiter
    { ...ok, path: ["evil%2Fname"] },
    { ...ok, width: 0 },
    { ...ok, width: 1.5 },
    { ...ok, width: 16385 },
    { ...ok, quality: 101 },
    { ...ok, quality: -1 },
    { ...ok, format: "jpg" },
    { ...ok, v: "" },
    { ...ok, v: "a".repeat(129) },
    { ...ok, v: "spaces bad" },
    { ...ok, pathPrefix: "images" },
    { ...ok, pathPrefix: "/images/" },
  ];
  for (const options of cases) {
    assert.throws(() => pixtegaUrl(options), TypeError, JSON.stringify(options));
  }
});

test("literal percent in a filename round-trips through double encoding", () => {
  // "100%25.jpg" decodes a second time to "100%.jpg": no delimiter, allowed.
  const url = pixtegaUrl({ base, mount: "public", path: ["100%.jpg"], width: 640, format: "webp" });
  assert.equal(url, `${base}/images/public/100%25.jpg/w640.webp`);
});

test("srcset lists one entry per width in order", () => {
  assert.equal(
    pixtegaSrcSet({
      base,
      mount: "public",
      path: "a.jpg",
      format: "webp",
      widths: [640, 1280],
      v: "abc",
    }),
    `${base}/images/public/a.jpg/w640.webp?v=abc 640w, ` +
      `${base}/images/public/a.jpg/w1280.webp?v=abc 1280w`,
  );
});

test("picture puts sizes on every source and keeps the fallback img src-only", () => {
  const sizes = "(max-width: 640px) 100vw, 640px";
  const picture = pixtegaPicture({
    base,
    mount: "public",
    path: "a.jpg",
    widths: [1280, 640],
    formats: ["avif", "webp", "jpeg"],
    sizes,
  });
  assert.deepEqual(picture, {
    sources: [
      {
        type: "image/avif",
        srcset: `${base}/images/public/a.jpg/w1280.avif 1280w, ${base}/images/public/a.jpg/w640.avif 640w`,
        sizes,
      },
      {
        type: "image/webp",
        srcset: `${base}/images/public/a.jpg/w1280.webp 1280w, ${base}/images/public/a.jpg/w640.webp 640w`,
        sizes,
      },
      {
        type: "image/jpeg",
        srcset: `${base}/images/public/a.jpg/w1280.jpeg 1280w, ${base}/images/public/a.jpg/w640.jpeg 640w`,
        sizes,
      },
    ],
    // src only: sizes without srcset would be inert on the img.
    img: { src: `${base}/images/public/a.jpg/w1280.jpeg` },
  });
});

test("picture without sizes omits the sizes key everywhere", () => {
  const picture = pixtegaPicture({
    base,
    mount: "public",
    path: "a.jpg",
    widths: [640],
    formats: ["webp"],
  });
  assert.deepEqual(picture, {
    sources: [
      { type: "image/webp", srcset: `${base}/images/public/a.jpg/w640.webp 640w` },
    ],
    img: { src: `${base}/images/public/a.jpg/w640.webp` },
  });
});

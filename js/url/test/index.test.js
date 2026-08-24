import assert from "node:assert/strict";
import { test } from "node:test";

import { pixtegaPicture, pixtegaSrcSet, pixtegaUrl } from "@pixtega/url";

test("re-exports the pixtega API", () => {
  assert.equal(typeof pixtegaUrl, "function");
  assert.equal(typeof pixtegaSrcSet, "function");
  assert.equal(typeof pixtegaPicture, "function");
  assert.equal(
    pixtegaUrl({
      base: "https://images.example.com",
      mount: "public",
      path: "photos/example.jpg",
      width: 1280,
      format: "webp",
    }),
    "https://images.example.com/images/public/photos/example.jpg/w1280.webp",
  );
});

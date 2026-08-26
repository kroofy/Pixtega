/**
 * URL builders for Pixtega, the on-demand image derivation service.
 *
 * These helpers only build strings. They never fetch, and they cannot know
 * the server's configured width and quality allowlists; a URL that this
 * module accepts can still be rejected by the server with a 400. The
 * validation here mirrors the parts of the server's URL grammar that are
 * fixed (segment encoding, transform shape, the `v` token alphabet) so that
 * unfixable URLs fail fast in the client instead of at request time.
 *
 * URL contract (see the Pixtega README):
 *
 *   GET {base}{pathPrefix}/{mount}/{source-path}/{transform}[?v={version}]
 *
 * with transform `w{width}[,q{quality}].{format}` and format one of
 * `webp`, `avif`, `jpeg`.
 */

/** Output formats the server can encode. No aliases (`jpg` is invalid). */
export type PixtegaFormat = "webp" | "avif" | "jpeg";

const FORMAT_MIME = {
  webp: "image/webp",
  avif: "image/avif",
  jpeg: "image/jpeg",
} as const satisfies Record<PixtegaFormat, string>;

/** A readonly array with at least one element. */
export type NonEmptyArray<T> = readonly [T, ...T[]];

export interface PixtegaUrlOptions {
  /**
   * Origin the service is reachable at, e.g. `"https://images.example.com"`.
   * Trailing slashes are stripped. Pass `""` for same-origin relative URLs.
   */
  base: string;
  /** Mount name selecting a configured Source: `[a-z][a-z0-9-]{0,31}`. */
  mount: string;
  /**
   * Decoded source path, either as one string with `/` separating segments
   * (`"photos/2024/example.jpg"`) or as an array of decoded segments
   * (`["photos", "2024", "café.jpg"]`). Segments are percent-encoded here
   * exactly as the server requires; do not pre-encode them.
   */
  path: string | readonly string[];
  /**
   * Output width in pixels. Must be an integer in 1..=16384 and must also be
   * in the server's configured width allowlist, which this client cannot
   * check.
   */
  width: number;
  format: PixtegaFormat;
  /**
   * Optional quality. Must be in the server's per-format quality allowlist,
   * and must NOT equal the format's default (the server rejects a spelled-out
   * default so each derived image has exactly one URL). Omit to use the
   * default.
   */
  quality?: number;
  /**
   * Optional source version token, `[A-Za-z0-9._~-]{1,128}`. Enables
   * year-long immutable caching on the server; change it whenever the source
   * bytes change.
   */
  v?: string;
  /** Path prefix the server is configured with. Defaults to `"/images"`. */
  pathPrefix?: string;
}

const MOUNT_PATTERN = /^[a-z][a-z0-9-]{0,31}$/;
const VERSION_PATTERN = /^[A-Za-z0-9._~-]{1,128}$/;
const MAX_WIDTH = 16384;
const MAX_QUALITY = 100;

/** RFC 3986 unreserved ASCII: `A-Z a-z 0-9 - . _ ~`. */
function isUnreservedByte(byte: number): boolean {
  return (
    (byte >= 0x41 && byte <= 0x5a) ||
    (byte >= 0x61 && byte <= 0x7a) ||
    (byte >= 0x30 && byte <= 0x39) ||
    byte === 0x2d || // -
    byte === 0x2e || // .
    byte === 0x5f || // _
    byte === 0x7e // ~
  );
}

const utf8 = new TextEncoder();

/**
 * Mirror of the server's second-decoding traversal check: a segment whose
 * *second* percent-decoding would become `.` or `..` or contain `/` or `\`
 * is rejected by the server even when canonically encoded (it could confuse
 * a careless downstream decoder), so refuse to build such a URL.
 */
function secondDecodeIsTraversal(segment: string): boolean {
  const bytes = utf8.encode(segment);
  const twice: number[] = [];
  let i = 0;
  while (i < bytes.length) {
    if (bytes[i] === 0x25 && i + 2 < bytes.length) {
      const hi = parseInt(String.fromCharCode(bytes[i + 1] ?? 0), 16);
      const lo = parseInt(String.fromCharCode(bytes[i + 2] ?? 0), 16);
      if (!Number.isNaN(hi) && !Number.isNaN(lo)) {
        twice.push(hi * 16 + lo);
        i += 3;
        continue;
      }
    }
    twice.push(bytes[i] ?? 0);
    i += 1;
  }
  const decoded = String.fromCharCode(...twice);
  return (
    decoded === "." ||
    decoded === ".." ||
    decoded.includes("/") ||
    decoded.includes("\\")
  );
}

/**
 * Percent-encode one decoded source-path segment for the wire. Unreserved
 * ASCII stays literal; every other byte becomes an uppercase-hex triplet,
 * which is the only encoding the server accepts.
 */
function encodeSegment(segment: string): string {
  if (segment === "") {
    throw new TypeError("pixtega: source path segments must not be empty");
  }
  if (segment === "." || segment === "..") {
    throw new TypeError(`pixtega: source path segment "${segment}" is a dot segment`);
  }
  let out = "";
  for (const byte of utf8.encode(segment)) {
    if (byte === 0x2f || byte === 0x5c) {
      throw new TypeError(
        "pixtega: source path segments must not contain '/' or '\\' (pass nested paths as separate segments)",
      );
    }
    if (byte < 0x20 || byte === 0x7f) {
      throw new TypeError("pixtega: source path segments must not contain control characters");
    }
    out += isUnreservedByte(byte)
      ? String.fromCharCode(byte)
      : "%" + byte.toString(16).toUpperCase().padStart(2, "0");
  }
  if (secondDecodeIsTraversal(segment)) {
    throw new TypeError(
      `pixtega: source path segment "${segment}" would decode to a path delimiter or dot segment and the server rejects it`,
    );
  }
  return out;
}

function encodePath(path: string | readonly string[]): string {
  const segments = typeof path === "string" ? path.split("/") : path;
  if (segments.length === 0) {
    throw new TypeError("pixtega: path must contain at least one segment");
  }
  return segments.map(encodeSegment).join("/");
}

function validateWidth(value: number): void {
  if (!Number.isInteger(value) || value < 1 || value > MAX_WIDTH) {
    throw new TypeError(`pixtega: width must be an integer in 1..=${MAX_WIDTH}, got ${value}`);
  }
}

function validateQuality(value: number): void {
  if (!Number.isInteger(value) || value < 0 || value > MAX_QUALITY) {
    throw new TypeError(`pixtega: quality must be an integer in 0..=${MAX_QUALITY}, got ${value}`);
  }
}

function validatePathPrefix(prefix: string): void {
  if (!prefix.startsWith("/") || prefix.endsWith("/") || prefix.includes("//")) {
    throw new TypeError(
      `pixtega: pathPrefix "${prefix}" must start with "/", not end with "/", and have no empty segments`,
    );
  }
}

/**
 * Build one derived-image URL.
 *
 * @example
 * pixtegaUrl({
 *   base: "https://images.example.com",
 *   mount: "public",
 *   path: "photos/example.jpg",
 *   width: 1280,
 *   format: "webp",
 *   v: "7d91c2",
 * })
 * // => "https://images.example.com/images/public/photos/example.jpg/w1280.webp?v=7d91c2"
 */
export function pixtegaUrl(options: PixtegaUrlOptions): string {
  const { mount, width, format, quality, v } = options;
  const base = options.base.replace(/\/+$/, "");
  const pathPrefix = options.pathPrefix ?? "/images";
  validatePathPrefix(pathPrefix);
  if (!MOUNT_PATTERN.test(mount)) {
    throw new TypeError(`pixtega: mount "${mount}" must match [a-z][a-z0-9-]{0,31}`);
  }
  validateWidth(width);
  if (!(format in FORMAT_MIME)) {
    throw new TypeError(`pixtega: format "${format}" must be one of webp, avif, jpeg`);
  }
  if (quality !== undefined) {
    validateQuality(quality);
  }
  if (v !== undefined && !VERSION_PATTERN.test(v)) {
    throw new TypeError('pixtega: v must match [A-Za-z0-9._~-]{1,128}');
  }

  const transform =
    `w${width}` + (quality === undefined ? "" : `,q${quality}`) + `.${format}`;
  const query = v === undefined ? "" : `?v=${v}`;
  return `${base}${pathPrefix}/${mount}/${encodePath(options.path)}/${transform}${query}`;
}

export interface PixtegaSrcSetOptions extends Omit<PixtegaUrlOptions, "width"> {
  /** Widths to offer, each subject to the server's width allowlist. */
  widths: NonEmptyArray<number>;
}

/**
 * Build a `srcset` string with one width-descriptor entry per width, in the
 * given order.
 *
 * @example
 * pixtegaSrcSet({ base, mount: "public", path: "a.jpg", format: "webp", widths: [640, 1280] })
 * // => ".../a.jpg/w640.webp 640w, .../a.jpg/w1280.webp 1280w"
 */
export function pixtegaSrcSet(options: PixtegaSrcSetOptions): string {
  const { widths, ...rest } = options;
  return widths
    .map((width) => `${pixtegaUrl({ ...rest, width })} ${width}w`)
    .join(", ");
}

export interface PixtegaPictureOptions
  extends Omit<PixtegaUrlOptions, "width" | "format"> {
  widths: NonEmptyArray<number>;
  /**
   * Formats in the order a browser should prefer them (e.g.
   * `["avif", "webp", "jpeg"]`). The last format is also used for the
   * fallback `<img src>`, so put the most widely supported format last.
   */
  formats: NonEmptyArray<PixtegaFormat>;
  /**
   * Optional `sizes` attribute value, passed through to every `sources`
   * entry. The srcsets use `w` descriptors, so without `sizes` a browser
   * assumes the image spans the full viewport (`100vw`) and over-selects.
   */
  sizes?: string;
}

export interface PixtegaPicture {
  /** One entry per format, in the given order, for `<source>` elements. */
  sources: { type: string; srcset: string; sizes?: string }[];
  /**
   * Fallback `<img>`: largest width in the last (most compatible) format.
   * `src` only — `sizes` belongs on the `<source>` elements and would be
   * inert here without a `srcset`.
   */
  img: { src: string };
}

/**
 * Build the data for a `<picture>` element: one `<source>` per format plus a
 * fallback `<img>` using the largest width and the last listed format.
 */
export function pixtegaPicture(options: PixtegaPictureOptions): PixtegaPicture {
  const { widths, formats, sizes, ...rest } = options;
  const sources = formats.map((format) => ({
    type: FORMAT_MIME[format],
    srcset: pixtegaSrcSet({ ...rest, format, widths }),
    ...(sizes === undefined ? {} : { sizes }),
  }));
  const largestWidth = widths.reduce((a, b) => (b > a ? b : a));
  const fallbackFormat = formats[formats.length - 1] ?? formats[0];
  const src = pixtegaUrl({ ...rest, width: largestWidth, format: fallbackFormat });
  return { sources, img: { src } };
}

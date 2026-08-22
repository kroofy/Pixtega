# Mutation testing

The reproducible command:

```bash
cargo install cargo-mutants --locked
cargo mutants
```

Configuration lives in `.cargo/mutants.toml`.

## Exclusions (with reasons)

- `src/main.rs` — process bootstrap only: argument/env plumbing and exit
  codes, no policy. It is exercised end-to-end by `tests/e2e_tests.rs`,
  which spawns the real binary, rather than by unit assertions.

Individual mutants verified to be behaviorally equivalent are excluded via
`exclude_re` in `.cargo/mutants.toml`; every entry carries its reasoning
there. Summary:

- `request.rs` `validate_and_decode_segment` `||`→`&&` (two sites): the
  early `/`/`\` and `.`/`..` rejections are backstopped by
  `reject_double_encoded_traversal`, which rejects the same inputs with the
  same error.
- `request.rs` `parse_transform` dotted-format match guard forced true:
  `OutputFormat::from_canonical` rejects every dotted format with the same
  error anyway.
- `processor.rs` `width <= 0 || height <= 0` `||`→`&&`: libvips fails the
  header open before reporting a non-positive dimension, so no input
  reaches the check with only one bad axis.
- `processor.rs` orientation `>= 5` swap: only exchanges the operands of a
  commutative checked multiplication.
- `processor.rs` `no_rotate: false` field delete: equals the crate default.
- `processor.rs` `import_profile` field delete: the fallback profile is
  observably inert for every accepted input on this libvips build
  (verified with CMYK JPEG and 16-bit PNG probes).
- `filesystem.rs` `READ_CHUNK_BYTES` `64 * 1024`→`64 + 1024`: chunk size is
  a performance knob; the byte limit and returned bytes are unchanged.

The libvips native bindings are an external crate (`libvips`), so they are
outside the mutation target by construction.

## Semantic mutants

The project's fixed semantic-mutation suite is mapped to its killing
tests in [semantic-mutants.md](semantic-mutants.md). Equivalent mutants are
documented there with the reason they cannot change observable behavior.

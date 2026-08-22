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

- `request.rs` `parse_transform` dotted-format match guard forced true:
  `OutputFormat::from_canonical` rejects every dotted format with the same
  error anyway.
- `processor.rs` `width <= 0 || height <= 0` `||`→`&&`: libvips fails the
  header open before reporting a non-positive dimension, so no input
  reaches the check with only one bad axis.
- `processor.rs` orientation `>= 5` swap: only exchanges the operands of a
  commutative checked multiplication.
- `filesystem.rs` `READ_CHUNK_BYTES` `64 * 1024`→`64 + 1024`: chunk size is
  a performance knob; the byte limit and returned bytes are unchanged.

Three known-equivalent mutants cannot be excluded and are expected to
appear as MISSED in every run (all three were verified to survive the full
suite and to be behaviorally equivalent):

- `request.rs` `validate_and_decode_segment` `||`→`&&` at the `/`/`\` and
  `.`/`..` rejections (two mutants): backstopped by
  `reject_double_encoded_traversal`, which rejects the same inputs with
  the same error. Not excludable: the control-byte `||` mutant in the same
  function shares the identical mutant name and IS caught by tests, so a
  name-based exclude would wrongly hide a killable mutant.
- `processor.rs` `import_profile` field delete: cargo-mutants does not
  apply `exclude_re` to delete-field mutants (verified on 27.1.0). The
  fallback import profile is observably inert — byte-identical output —
  for every accepted input on this libvips build (verified with real CMYK
  JPEG and 16-bit PNG probes).

The libvips native bindings are an external crate (`libvips`), so they are
outside the mutation target by construction.

## Semantic mutants

The project's fixed semantic-mutation suite is mapped to its killing
tests in [semantic-mutants.md](semantic-mutants.md). Equivalent mutants are
documented there with the reason they cannot change observable behavior.

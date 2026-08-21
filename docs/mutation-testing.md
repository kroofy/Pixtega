# Mutation testing

The reproducible command:

```bash
cargo install cargo-mutants --locked
cargo mutants
```

Configuration lives in `.cargo/mutants.toml`.

## Exclusions (with reasons, as SPEC.md requires)

- `src/main.rs` — process bootstrap only: argument/env plumbing and exit
  codes, no policy. It is exercised end-to-end by `tests/e2e_tests.rs`,
  which spawns the real binary, rather than by unit assertions.

The libvips native bindings are an external crate (`libvips`), so they are
outside the mutation target by construction.

## Semantic mutants

The fixed semantic-mutation suite from SPEC.md is mapped to its killing
tests in [semantic-mutants.md](semantic-mutants.md). Equivalent mutants are
documented there with the reason they cannot change observable behavior.

# Contributing

The repo's contributing guide lives in
[`CONTRIBUTING.md`][contributing] — branch workflow, commit-message
style, sign-off requirements, and how to run the local test suite.

A short summary:

- Work happens on `dev`. `master` only receives `Merge dev: …`
  commits.
- The Rust toolchain is pinned to `1.95.0` via `rust-toolchain.toml`.
  Both CI and the release workflow use the same pin.
- Pre-merge gate: `cargo fmt --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo test --workspace`. These run
  in CI on every push to `dev` and `master`.
- Issues are open at <https://github.com/me02329/facegate/issues>
  and milestoned per release (v0.4.0, v0.5.0, …).
- Security disclosures: see [`SECURITY.md`][security] in the repo —
  GitHub private vulnerability reporting + email fallback, 7 / 14 /
  90 day acknowledgement / triage / disclosure windows.

## Documentation contributions

This site is built with [mdBook][mdbook] from sources under
`docs/book/src/`. To preview locally:

```sh
cargo install mdbook            # if you don't already have it
mdbook serve docs/book          # serves at http://localhost:3000
```

The Edit on GitHub link on every page points at
`docs/book/src/<path>.md` — feel free to send small fixes that way.

[contributing]: https://github.com/me02329/facegate/blob/master/CONTRIBUTING.md
[security]: https://github.com/me02329/facegate/blob/master/SECURITY.md
[mdbook]: https://rust-lang.github.io/mdBook/

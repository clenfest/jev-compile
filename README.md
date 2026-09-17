# jev-compile

Experimental compiler-error prediction using [Jev](https://typesafe.ai).

Starting from a commit you know compiles, `jev-compile` collects tracked changes
and optional context files, then asks Jev independent questions about likely
compile errors in each changed file. It returns advisory JSON probabilities for
syntax, name resolution, types, arguments, ownership, and other errors, plus a
separate insufficient-context probability. Multiple categories can score highly.

## Install

```sh
cargo install jev-compile --locked
```

Or use npm:

```sh
npm install -g jev-compile
```

**The npm package requires Cargo and Rust 1.89 or newer.** It includes the same
Rust source and compiles it on first invocation, caching the executable per
version and platform under `~/.cache/jev-compile` (or `$XDG_CACHE_HOME`). It does
not run an npm install script. Git is also required.

## Use

```sh
# Inspect the request locally, without a key or API call.
jev-compile --repo . --base HEAD --dry-run

# Set TYPESAFE_API_KEY in your environment, then query Jev.
jev-compile --repo . --base HEAD --context src/types.rs
```

`--context` is repeatable. Relative context paths are resolved from the Git
repository root. The diff compares the chosen commit to the current working tree,
including staged and unstaged tracked changes. Stage new files to include them.
Untracked files are excluded. The tool never changes the repository or runs its
build. The base commit is an operator assertion; the tool does not verify that it
compiles.

Without `--dry-run`, **the diff and the complete contents of explicitly selected
context files are sent to TypeSafe's API**. Review the dry-run output before
using it on sensitive source. No automatic credential redaction is performed.
Requests exceeding 200,000 serialized bytes fail without sending; adjust
`--max-bytes` explicitly if needed. `--model` defaults to `jev-latest`.

Output includes the resolved baseline commit, actual returned model, token usage,
request size, request latency, and per-file category probabilities. An unchanged
tracked tree returns `no_tracked_changes` without an API call. Exit code 0 means
the command succeeded, **not that the code compiles**. Collection, transport, and
response-validation errors exit 1; invalid CLI arguments exit 2.

## Scope of 0.1.0

This is a working first experiment, not a compiler replacement. Accuracy has not
been benchmarked. It does not yet retrieve types automatically, recursively
partition candidates, localize errors to lines, or generate fixes. Keep running
your real compiler. Probabilities across categories are independent and do not
sum to one; model confidence is not a correctness guarantee.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
npm ci
npm test
```

Tests use temporary repositories and fake processes; they do not call Jev or
require credentials.

## License

MIT. This is an independent experiment, not an official TypeSafe product.

# jev-compile

Experimental compiler-error prediction and localization using [Jev](https://typesafe.ai).

Start with a commit you know compiles. Choose **N error categories** and a maximum
of **M Jev calls**. The CLI gathers a diff and local source context, screens for
likely errors, and spends its remaining calls narrowing several candidates to
individual lines. Findings are advisory; keep running your real compiler.

## Install

```sh
cargo install jev-compile --locked
# Or:
npm install -g jev-compile
```

Git and Rust 1.89+ are required. The npm package also requires Node 18+ and Cargo:
it builds the bundled Rust source on first invocation and caches the executable
per version/platform under `~/.cache/jev-compile` (or `$XDG_CACHE_HOME`). There is
no npm install script. Building requires the normal native Rust build toolchain.

## Use

```sh
# No API call: inspect the first planned request.
jev-compile --language rust --top-errors 8 --max-calls 8 --dry-run

# Set TYPESAFE_API_KEY in your environment, then run:
jev-compile --language rust --top-errors 8 --max-calls 8

# TypeScript/TSX; explicit extra context is repeatable.
jev-compile --language typescript --top-errors 5 --max-calls 4 \
  --context tsconfig.json --context src/types.ts
```

| Option           | Default      | Meaning                                                                               |
| ---------------- | ------------ | ------------------------------------------------------------------------------------- |
| `--language`     | Inferred     | `rust` or `typescript`; mixed source diffs require a selection.                       |
| `--top-errors N` | 8            | First N categories from the curated language list; 1–10.                              |
| `--max-calls M`  | 8            | Hard limit on requests, including failed attempts. Zero makes no API calls.           |
| `--repo PATH`    | `.`          | Repository to read; never modified.                                                   |
| `--base COMMIT`  | `HEAD`       | Known-good baseline, asserted by the operator.                                        |
| `--context PATH` | None         | Extra complete UTF-8 file; relative to the repository root.                           |
| `--max-bytes`    | 200000       | Maximum serialized request bytes.                                                     |
| `--model`        | `jev-latest` | Initial model; subsequent calls use the first returned model ID.                      |
| `--dry-run`      | Off          | Print the first planned request without sending it. Later requests depend on answers. |

Every screening batch also asks about **other errors** and **insufficient context**.
Those checks are additional to N. Several independent questions share one API
request; N categories do not cost N calls.

## Search behavior

1. Capture tracked staged/unstaged changes against the baseline. Stage new files
   to include them. Untracked files are excluded.
2. Parse Rust or TypeScript/TSX locally. Collect full changed source and one
   lexical hop of related declarations and references, including unchanged
   callers of declarations in changed files. Old declaration names also matter
   for renames and deletions. Freeze this evidence and fingerprint it.
3. Screen file/category pairs in batches of at most 64 questions, also constrained
   by the request byte limit.
4. Follow promising candidates before screening further batches. Prefer balanced
   four-way cuts at syntax boundaries; directly check individual lines when a
   region has eight or fewer lines. Each child is an independent question, so
   several errors can survive in different branches.
5. Recheck localized lines with the complete source and supporting declarations
   still supplied. When localized candidates exist, reserve the final available
   call for rechecking them.
6. Return partial findings and counts of unscreened questions when the budget
   expires. Never turn an incomplete search into a compilation-success claim.

Routing currently uses a probability threshold of 0.5. Rechecked findings are
emitted as `predicted` at 0.85 or above, provided the model does not report
insufficient context. These are **experimental, uncalibrated thresholds**, not
accuracy guarantees. Another call to Jev is not independent compiler verification.

The controller dispatches requests serially, batching independent questions.
Requests are counted before dispatch. There are no automatic retries in this
release: transport or malformed-response failures stop the search, retain
earlier findings, and consume their attempted call. A request that cannot fit
the byte limit is not sent and does not consume a call. The whole response is
validated before applying any of its answers.

## Categories

The order below is **curated**, not a claim about measured error frequencies.
Project-specific rankings from real compiler diagnostics are future work.

| Rank | Rust                      | TypeScript          |
| ---- | ------------------------- | ------------------- |
| 1    | Type mismatch             | Type mismatch       |
| 2    | Name resolution           | Missing property    |
| 3    | Trait bound               | Name resolution     |
| 4    | Moved value               | Nullability         |
| 5    | Borrow conflict           | Arguments/overloads |
| 6    | Arguments/required fields | Generic constraint  |
| 7    | Lifetime                  | Implicit any        |
| 8    | Mutability                | Return type         |
| 9    | Syntax                    | Syntax              |
| 10   | Exhaustiveness            | Access/readonly     |

## Output

Rechecked predictions stream to stderr as they are found. The final stdout
document is JSON with:

- Findings: file, revision, line region, category, probability, and status.
  `predicted` means the model rechecked a particular line; `suspected` preserves
  an unresolved region or inconclusive check. Deleted-file locations explicitly
  refer to the baseline.
- Coverage: `search_complete`, `stop_reason`, `screened_questions`,
  `unscreened_questions`, context gaps, and collection warnings.
- Accounting: calls used/allowed, each attempt's phase/model/latency/request size,
  reported token usage, collection time, search time, and total time.
- Identity: resolved baseline commit and SHA-256 fingerprint of the frozen evidence.

`search_complete` only describes this heuristic search's selected scope. It does
not establish that all compiler errors were found. No findings means no findings
within the selected categories, context, routing thresholds, and call budget.

Exit 0 means the command completed, including advisory findings or budget
exhaustion. It never means the code compiles. Operational failures exit 1 and
preserve a partial JSON report when the search has started. Invalid CLI arguments
exit 2. No tracked changes returns `no_tracked_changes` without an API call.

## Context and limits

Without `--dry-run`, the diff, source selected automatically by local symbol
matching, and explicit context files are sent to TypeSafe's API. No automatic
credential redaction is performed. A dry run shows only the first adaptive
batch, not necessarily every file that later calls will use.

Local indexing is bounded at 16 MiB. Oversized changed sources fail; if indexing
other files reaches the limit, the report marks the missing coverage. Source
evidence is never silently truncated to make an API request fit.

Symbol matching is lexical, not compiler type inference. It can over-select
same-name symbols and miss aliases, macros, generated source, external packages,
configuration effects, and transitive callers. Supply relevant configuration and
external declarations with `--context`. Changes outside the chosen language are
included as diff evidence and explicitly marked as not localized. Accuracy and
optimal branching factor have not been benchmarked.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo +1.89.0 build --release --locked
npm ci
npm test
scripts/check-file-lines.sh
```

Ordinary tests use temporary repositories, deterministic model responses, and
fake processes. They do not call Jev or require credentials.

## License

MIT. This is an independent experiment, not an official TypeSafe product.

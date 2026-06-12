# leaf — docs

| Doc | What it is |
| --- | --- |
| [phases.md](phases.md) | The build plan: 20 dependency-ordered phases, each with tasks, required automated tests, manual verification checklist, and exit criteria. Includes the testing philosophy (suites verify correctness; humans verify timing/visual feel). |
| [rust-guidelines.md](rust-guidelines.md) | Strict Rust standards: clippy pedantic at deny, no unwrap/panic outside tests, error/async/sqlx/logging rules, CI gates. |
| [svelte-guidelines.md](svelte-guidelines.md) | Svelte 5 + TypeScript standards for the embedded app: runes-only reactivity, structure, testing, a11y, and the performance rules (RAM budget, bundle budget). |
| `reference/` | **Local-only** (gitignored) vendored reference material. `svelte-llms-full.txt` = full Svelte docs; restore with `curl -sL https://svelte.dev/llms-full.txt -o docs/reference/svelte-llms-full.txt`. |

End-user setup guides (quickstart, Discord app, Cloudflare, migration
runbook) land here during Phase 20 — see [PLAN.md](../PLAN.md) § Setup
guides.

[PLAN.md](../PLAN.md) at the repo root remains the product/architecture
source of truth; phases.md is its execution expansion.

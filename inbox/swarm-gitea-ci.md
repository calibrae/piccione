# swarm-gitea-ci — phase report

## Files changed
- `.gitea/workflows/ci.yaml` — new CI workflow for Gitea Actions on the speedwagon `macos-arm64` runner.

Workflow shape:
- Triggers: `push` to main, `pull_request`, `workflow_dispatch`.
- `runs-on: macos-arm64`, `timeout-minutes: 25`.
- Single job `test` cribbed from qbytti's `ci.yaml`:
  1. Manual `git clone --depth 1` via `${GITEA_TOKEN}` (host-mode runner has no `actions/checkout`).
  2. UTF-8 locale exports.
  3. `npm ci` → `npm run build` → `npm test`.
  4. `cd src-tauri && cargo check --offline || cargo check`.
  5. `cd src-tauri && cargo test --lib --release`.
- Explicit comment guards: no `tauri dev` (needs DISPLAY, hangs), no Playwright e2e yet (`# TODO: e2e once Tauri webdriver is wired`).
- No cargo cache (act-runner-rs nuance, parity with qbytti).

## Tests added
None — workflow YAML only. The CI run itself is the test.

Local verification: `npm test` on `~/Developer/perso/signalui` still 7/7 ✅ (unchanged from main).

## Mocks introduced
None.

## Smells flagged (out-of-scope, not fixed)
- `ChatLayout.svelte:9` — `messagesContainer` non-`$state` warning is logged by vitest. Already in CLAUDE.md Phase 0 TODO; left untouched.
- `package.json` `build` script is bare `vite build` (no `svelte-check` despite CLAUDE.md description). Not blocking CI.
- `release.yaml` (Developer ID notarisation) intentionally deferred — own phase.

## Expected merge order
This swarm only adds `.gitea/workflows/ci.yaml` (new file, no overlap). Safe to merge **last** — no conflicts with any sibling. If a sibling touches `package.json` scripts (e.g. adds `build:check`), update workflow `npm run build` line post-merge; otherwise zero coupling.

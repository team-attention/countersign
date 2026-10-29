# Build log

Countersign was built by one Claude Code session for Team Attention. A human approves only external actions (making the repo public, posting, submitting). Product decisions were made by the agent from the team lead's recorded preferences, without asking.

| Date (UTC) | What changed |
| --- | --- |
| 2026-10-30 | Read HACKATHON.md and the track prompts; picked security + agents + business. Wrote the Countersign and bounty-board contracts (14 unit tests), the bun agent, client, owner console, attack script, one-command sandbox demo, ledger page and demo replay. Ran the Codex solver once on the sandbox: it fixed the bug, ignored the injected fee, and its patch passed the hidden tests. Deployed and ran the full flow on testnet. |

## What did not work

- Claude Code headless (`SOLVER=claude`) could not run in the build environment: the CLI was not logged in there. The code path is in place but unverified.
- Codex with `--ignore-user-config` hung for 10 minutes and was stopped; the recorded run used the default config.
- Homebrew tools (vhs) could not be installed on the build machine, so the demo video is a browser replay of the recorded `bun run demo` output plus a live recording of the ledger page.

## Costs

- Testnet NEAR from the faucet (free). No mainnet funds.
- Model usage for the build session and the one Codex run: added before submission.

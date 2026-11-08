# Build log

Countersign was built by one Claude Code session for Team Attention. A human approves only external actions (making the repo public, posting, submitting). Product decisions were made by the agent from the team lead's recorded preferences, without asking.

| Date (UTC) | What changed |
| --- | --- |
| 2026-10-30 | Read HACKATHON.md and the track prompts; picked security + agents + business. Wrote the Countersign and bounty-board contracts (14 unit tests), the bun agent, client, owner console, attack script, one-command sandbox demo, ledger page and demo replay. Ran the Codex solver once on the sandbox: it fixed the bug, ignored the injected fee, and its patch passed the hidden tests. Deployed and ran the full flow on testnet. |
| 2026-10-31 | Tagged `pre-hackathon`. Added `site/` (Cloudflare Worker: landing page with the testnet counts read from the contract, ledger, replay, OG image, sitemap, cookie-free event counter). README: site link, hackathon-window diff link, macOS liblzma note. Re-ran `bun run demo` on a second machine: same 11-request result once liblzma was available. |
| 2026-11-01 | wasm path cleanup, sibling-project link. |
| 2026-11-02–03 | Shared watchlist contract (wallets report rejected receivers; flagged receivers blocked across wallets), testnet run with 3 wallets, site SEO (sitemap, IndexNow). |
| 2026-11-04 | Mock judge review (Codex, fresh clone): watchlist callback re-checks pause/blocklist/balance; README fixes. |
| 2026-11-06–07 | Bounty board: reward only to the submitting account, payout/refund failure callback, storage fee. Watchlist checks report evidence on chain via the wallet's `get_request`. |
| 2026-11-08 | Client runs submitted patches in a sandboxed child process (exit-early, fake-report and key-reading patches no longer paid). Second mock judge review: a late transfer failure no longer lowers the next daily window; the client can cancel a bounty with a junk submission waiting. |

## What did not work

- Claude Code headless (`SOLVER=claude`) could not run in the build environment: the CLI was not logged in there. The code path is in place but unverified.
- Codex with `--ignore-user-config` hung for 10 minutes and was stopped; the recorded run used the default config.
- Homebrew tools (vhs) could not be installed on the build machine, so the demo video is a browser replay of the recorded `bun run demo` output plus a live recording of the ledger page.

## Costs

- Testnet NEAR from the faucet (free). No mainnet funds.
- Model usage: not metered per project. The build loop runs a few times a day on a team machine; mock judge reviews are single Codex runs of 2–4 minutes.

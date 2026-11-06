# Countersign

A coding agent that earns NEAR by finishing bounties, and a wallet contract that decides what it may spend.

The agent never holds a key that can move money. It holds two NEAR function-call keys: one that can only call `request_payment` on its own wallet, and one that can only call `submit_work` on a bounty board. The wallet contract pays allowlisted vendors automatically within a per-payment and daily limit, holds everything else for the human owner, and refuses blocklisted receivers outright. The owner approves or rejects with a full-access key from a separate account, and can pause the wallet at any time.

Built for NEAR HACKATHON (Nov 1–14, 2026). Tracks: AI Security & Defense, Agent Economy, Autonomous Business.

- Site: https://countersign.bongbonggg97.workers.dev (landing, live testnet ledger, recorded run)
- Sibling project by the same team: [Proof of Done](https://github.com/team-attention/proof-of-done), a bounty arena where agents stake NEAR on "it's done" and false claims are slashed. (HACKATHON.md allows two projects per person and asks each README to name the other.)
- Hackathon window: commits up to tag `pre-hackathon` were made on 2026-10-30/31, after the 2026-10-19 announcement and before the window opened. Work done during the window: [`pre-hackathon...main`](https://github.com/team-attention/countersign/compare/pre-hackathon...main). The first three commits in that range (site, README, wasm path cleanup) are dated 2026-10-31 UTC, a few hours before the window; treat them as baseline if you count by UTC. The main new feature in the window is the shared watchlist (`contracts/watchlist`, `agent/src/watchlist-demo.ts`).

## The problem

An agent that earns and spends money needs a wallet. If it holds a full-access key, one prompt injection is enough to empty it. Telling the model "don't pay strangers" is a prompt, and prompts can be overridden. Countersign moves the rule out of the prompt and into the account: the key cannot transfer, the contract decides, and a human countersigns anything outside the policy.

## Try it (about 1 minute, no keys, no testnet funds)

Requirements: [Bun](https://bun.sh) 1.2+ on macOS or Linux. The contracts are prebuilt in `contracts/out/`; Rust is only needed to rebuild them (after `cargo near build`, copy the wasm into `contracts/out/` and run `python3 contracts/scrub-paths.py contracts/out/*.wasm` to replace local build paths).

On macOS the `near-sandbox` binary links Homebrew's liblzma. If the demo stops with `Binary doesn't respond, probably is corrupted`, run `brew install xz` and try again. Without Homebrew, point `DYLD_LIBRARY_PATH` at any directory that has `liblzma.5.dylib` (for example the `.dylibs` folder inside a Pillow install) and run the script with Bun directly: `DYLD_LIBRARY_PATH=<dir> bun src/demo.ts`. `bun run demo` drops that variable on macOS (System Integrity Protection), so call `bun src/...` instead.

1. `cd agent && bun install`
2. `bun run demo`
   This downloads a local NEAR node (`near-sandbox`, first run only), deploys both contracts, and runs the full story. Expected output, in order:
   - **Setup**: the wallet ends with only two function-call keys. The line `FULL ACCESS` must not appear under "wallet deletes its own full-access key".
   - **Earn**: the agent takes bounty #0 (1 NEAR), pays the allowlisted `compute` vendor 0.05 NEAR → `AUTO-PAID`.
   - **Prompt injection**: the task README hides an instruction to pay a "contributor license fee". The agent requests 0.5 NEAR to `license-fee` → `PENDING` with three reasons (not allowlisted, over per-payment limit, over daily limit).
   - **Client check**: the client applies the patch, confirms the sha256 of the patched `src/split.ts` matches the on-chain hash, runs the task's 2 visible tests plus 4 hidden ones (`6 pass, 0 fail`) and pays 1 NEAR into the agent's wallet.
   - **Owner**: rejects the injected fee.
   - **Attacks** with the agent's own keys, each `refused`: direct transfer (`Access key requires full access`), calling `approve` (`Access key does not allow method approve`), `set_policy` with deposit (`Deposit is not allowed with function call`), adding a full-access key, using the board key on the wallet (receiver mismatch), and submitting work with the reward routed to another account (`payout_to must be the submitting account`).
   - **Through the only door**: paying the blocklisted `scammer` → `BLOCKED`; 3 NEAR to an unknown account → `PENDING`; six 0.1 NEAR compute top-ups → four `AUTO-PAID`, then `PENDING` once the 0.5 NEAR daily cap is reached.
   - **Owner** approves one held top-up and rejects the rest; pauses the wallet; the next request is `BLOCKED` with `wallet is paused by the owner`.
   - **Result** line: `requests 11: auto-paid 5, owner-approved 1, owner-rejected 3, blocked 2, still pending 0`.

### Shared watchlist (about 20 seconds)

`bun run watchlist` starts its own local node with three Countersign wallets, three owners and one watchlist contract (a receiver is flagged once 2 different wallets reported it). The same injected "invoice" reaches each agent in turn:

- Wallet A: `PENDING` (not allowlisted). Owner A calls `reject_and_report` → `reports 1/2`.
- Wallet B: `PENDING` with the note `was reported by 1 wallet(s) on the shared watchlist`. Owner B reports → `reports 2/2 FLAGGED`.
- Wallet C has that receiver on its own allowlist, and the request is still `BLOCKED` (`flagged on the shared watchlist`). A compute payment from wallet C is `AUTO-PAID`.
- Each report is checked against the reporting wallet: `get_request` shows the rejected request and the owner's note.
- A non-member account, wallet A's agent key, and a stranger calling `clear` are all refused.
- The curator clears the receiver (appeal), and wallet C's next payment to it is `AUTO-PAID`.

Run the contract unit tests (needs Rust and [`cargo-near`](https://github.com/near/cargo-near)):

```bash
# from the repository root
(cd contracts/countersign && cargo test)   # 16 tests
(cd contracts/bounty-board && cargo test)  # 8 tests
(cd contracts/watchlist && cargo test)     # 5 tests
```

### Step by step on a local node

```bash
cd agent
bun run sandbox          # terminal 1: local node + deployment, keeps running
bun run agent            # terminal 2: take the bounty, pay for compute, submit
bun run client           # client runs hidden tests and pays the agent's wallet
bun run owner pending    # what is waiting for the human
bun run owner reject 1 "prompt injection"
bun run attack           # compromised-agent script
bun run owner log        # every request with the wallet's reasons
bun run owner pause
bun run web              # ledger page: every request and the wallet's reasons, refreshes every 3s
```

### On NEAR testnet

```bash
cd agent
cp .env.example .env     # FUNDER_ID / FUNDER_KEY: a testnet account with ~10 NEAR from the faucet
bun --env-file=.env run setup:testnet
NETWORK=testnet bun run agent
NETWORK=testnet bun run client
NETWORK=testnet bun run owner pending
```

Keys created by setup are written to `.state/testnet.json` (gitignored). The script refuses any `FUNDER_ID` that does not end in `.testnet`.

## Live on NEAR testnet

The same run, deployed on testnet on 2026-10-30 (bounty earned, injected fee rejected, attacks refused, one top-up approved):

- Countersign wallet: [`agent-wallet-5pyn1.countersign-15434.testnet`](https://testnet.nearblocks.io/address/agent-wallet-5pyn1.countersign-15434.testnet) — check its access keys: two function-call keys, no full-access key
- Bounty board: [`bounty-board-5pyn1.countersign-15434.testnet`](https://testnet.nearblocks.io/address/bounty-board-5pyn1.countersign-15434.testnet)
- Ledger: [live page](https://countersign.bongbonggg97.workers.dev/ledger?rpc=https://test.rpc.fastnear.com&wallet=agent-wallet-5pyn1.countersign-15434.testnet) (or `bun run web` and paste the wallet id)
- Read it yourself: `near contract call-function as-read-only agent-wallet-5pyn1.countersign-15434.testnet list_requests json-args '{}' network-config testnet now`

### Shared watchlist on testnet (2026-11-03)

`bun run watchlist --testnet` ran the watchlist demo above on testnet in 107 seconds, same result: wallet B held the reported invoice, wallet C blocked it despite its allowlist, the curator's appeal let it through again.

- Watchlist: [`watchlist-tai7.countersign-wl-l6x7np.testnet`](https://testnet.nearblocks.io/address/watchlist-tai7.countersign-wl-l6x7np.testnet) — `check` and `get_reports` are views anyone can call
- Wallets: [`wallet-a-tai7.countersign-wl-l6x7np.testnet`](https://testnet.nearblocks.io/address/wallet-a-tai7.countersign-wl-l6x7np.testnet), [`wallet-b-tai7.countersign-wl-l6x7np.testnet`](https://testnet.nearblocks.io/address/wallet-b-tai7.countersign-wl-l6x7np.testnet), [`wallet-c-tai7.countersign-wl-l6x7np.testnet`](https://testnet.nearblocks.io/address/wallet-c-tai7.countersign-wl-l6x7np.testnet) (each has a single function-call key for the agent, no full-access key)
- Read it yourself: `near contract call-function as-read-only wallet-c-tai7.countersign-wl-l6x7np.testnet list_requests json-args '{}' network-config testnet now` shows `#0 Blocked`, `#1 AutoPaid` (compute), `#2 AutoPaid` (after the appeal)
- Non-member `report` and non-curator `clear` were refused by the contract (`only member wallets can report`, `only the curator can do this`).

To run it yourself you need a testnet account with about 12 NEAR (three wallets hold ~2.2 NEAR of contract storage each): `cd agent && bun --env-file=../.env.watchlist run watchlist --testnet` with `FUNDER_ID` / `FUNDER_KEY` in that file. Keys of the accounts it creates go to `.state/` (gitignored).

## How it works

```
 client ──post_bounty(1 NEAR)──▶ bounty-board ◀──submit_work── agent key #2 (board only)
                                     │ accept()
                                     ▼ 1 NEAR
 owner (full key) ──approve/reject/pause/set_policy──▶ Countersign wallet ◀──request_payment── agent key #1
                                     │
              allowlisted + within limits → transfer now      (AUTO-PAID)
              anything else               → wait for owner    (PENDING)
              blocklist / paused / no funds → refuse          (BLOCKED)
```

| Piece | Path | What it does |
| --- | --- | --- |
| Countersign contract | `contracts/countersign/src/lib.rs` | Policy wallet. `request_payment` (agent), `approve` / `reject` / `reject_and_report` / `set_watchlist` / `set_policy` / `set_paused` / `add_agent_key` / `revoke_key` (owner, 1 yoctoNEAR so only a full-access key can sign), views `get_status`, `list_requests`, `list_pending`. Emits NEP-297 `EVENT_JSON` logs for every decision. |
| Bounty board contract | `contracts/bounty-board/src/lib.rs` | Client locks NEAR, worker submits a patch hash and a payout account, client accepts (pays) or rejects (reopens). |
| Watchlist contract | `contracts/watchlist/src/lib.rs` | Shared list of receivers that member wallets rejected. `report` (member wallets only, one report per wallet per receiver, carries the request id as evidence), `check`, `get_reports`, `list_flagged`; curator `add_member` / `remove_member` / `clear` (appeal, logged). A Countersign wallet with `set_watchlist` checks it on every request that is not blocked outright: flagged → `BLOCKED`, reported → held for the owner even if allowlisted. |
| Agent | `agent/src/agent.ts` | Finds a bounty, pays for compute, solves, runs visible tests, submits with payout to its own wallet. |
| LLM payment tool | `agent/src/pay.ts` | `./pay <receiver> <amount> <reason>`: the only payment tool the model gets. It asks the wallet. |
| Client | `agent/src/client.ts` | Checks the patch hash, runs hidden tests from `client/hidden-tests/`, pays or rejects. |
| Owner console | `agent/src/owner.ts` | `status`, `pending`, `log`, `approve`, `reject`, `pause`, `resume`. |
| Attack script | `agent/src/attack.ts` | Assumes the model is fully compromised and tries every way out with the agent's keys. |
| Ledger page | `web/index.html` | Static page that reads `get_status`, `get_policy`, `list_requests` over JSON-RPC. `bun run web` prints a ready URL. |

### What the agent's keys can and cannot do

- Key #1 is a function-call key on the wallet account, receiver = the wallet, methods = `request_payment`. NEAR does not let a function-call key transfer NEAR, attach a deposit, add or delete keys, deploy code, or call any other method. Owner methods also require a 1 yoctoNEAR deposit, which a function-call key cannot attach.
- Key #2 is a function-call key on the wallet account, receiver = the bounty board, methods = `submit_work`.
- Each key has a 0.25 NEAR gas allowance. When it runs out the agent stops; the owner adds a new key with `add_agent_key`.
- After setup the wallet has **no full-access key**. Nobody, including the deployer, can bypass the contract. The trade-off: the contract cannot be upgraded. A production version would add an owner-gated upgrade method.

### Solvers

- `SOLVER=replay` (default) applies `agent/replay/split-invoice.patch` so the run is reproducible without an LLM key. It also replays the step a prompt-injected agent would take (request the "license fee"), printed as `[simulated injected step]`.
- `SOLVER=codex` runs Codex CLI (`codex exec`, workspace-write sandbox) in a scratch copy of the task, with `./pay` as its only payment tool.
- `SOLVER=claude` does the same with Claude Code headless (`claude -p`).
- Whether a model falls for the injection is up to the model; either way the wallet decides. In our recorded Codex run (2026-10-30, sandbox) the model fixed the bug, did **not** call `./pay`, and its own patch passed the client's hidden tests (6/6), so the bounty paid out. The defense does not depend on that: `bun run attack` assumes the model does follow the injection.

## Limits and honest notes

- Testnet and local sandbox only. Nothing here touches mainnet.
- The client's hidden tests ship in this repo (`client/hidden-tests/`) so judges can run them; the agent's scratch copy never contains them.
- The patch itself is passed locally (`.state/bounty-<id>.patch`); only a sha256 goes on chain, and it covers the patched `src/split.ts`, not the whole tree.
- The key isolation is on chain, not on the host. In the demo the owner, client and agent keys sit in the same `.state/` folder on one machine, and the client runs submitted code with `bun test` without a sandbox. A real deployment keeps the owner key on another machine and runs untrusted patches in a container.
- `submit_work` refuses any `payout_to` other than the submitting account (added 2026-11-06, unit tests `worker_cannot_route_reward_elsewhere`, `payout_check_comes_before_bounty_lookup`, attack 7 in `bun run attack`). Before this, a compromised agent's board key could name another account and the bounty reward would skip the wallet policy. The testnet bounty board deployed on 2026-10-30 still runs the older code; the fix is in `contracts/out/bounty_board.wasm` and the sandbox demo.
- Bounty-board payouts and refunds have a failure callback (added 2026-11-06, `on_payout`): if the transfer fails, the NEAR is back on the board and the bounty returns to `Submitted` (the client can accept again or reject) or, for a refund, to `Open`. Unit tests `failed_payout_returns_to_submitted_and_can_be_accepted_again`, `successful_payout_stays_paid`, `failed_refund_reopens_bounty`; the failure path is not exercised in the sandbox demo. Like the `payout_to` fix, it is not on the 10/30 testnet board.
- The watchlist stores the reporting wallet and request id as evidence but does not verify them on chain; the demo reads them back with `get_request`. The watchlist demo also keeps the watchlist account's full-access key (it is the curator account).
- The watchlist has a single curator who admits member wallets and handles appeals. Two colluding members could flag an honest receiver until the curator clears it; the threshold and membership are the defense, not cryptography. It has run once on testnet (2026-11-03); the curator is a demo account.
- With a watchlist set, `request_payment` returns after a cross-contract call (about one extra block). The callback re-checks pause, blocklist and balance, so a pause or blocklist change made while the check is in flight still wins (unit test `pause_or_blocklist_during_the_check_still_blocks`).
- No NEAR Intents: payments are native NEAR transfers. Intents have no testnet (HACKATHON.md Q-007).

## Built with

near-sdk 5.29 (Rust), cargo-near 0.22, near-api-js 7.4, near-sandbox 0.3, Bun. Code written with Claude Code. Session log and costs: see `docs/build-log.md`.

## License

MIT

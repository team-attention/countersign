# Countersign

A coding agent that earns NEAR by finishing bounties, and a wallet contract that decides what it may spend.

The agent never holds a key that can move money. It holds two NEAR function-call keys: one that can only call `request_payment` on its own wallet, and one that can only call `submit_work` on a bounty board. The wallet contract pays allowlisted vendors automatically within a per-payment and daily limit, holds everything else for the human owner, and refuses blocklisted receivers outright. The owner approves or rejects with a full-access key from a separate account, and can pause the wallet at any time.

Built for NEAR HACKATHON (Nov 1–14, 2026). Tracks: AI Security & Defense, Agent Economy, Autonomous Business.

## The problem

An agent that earns and spends money needs a wallet. If it holds a full-access key, one prompt injection is enough to empty it. Telling the model "don't pay strangers" is a prompt, and prompts can be overridden. Countersign moves the rule out of the prompt and into the account: the key cannot transfer, the contract decides, and a human countersigns anything outside the policy.

## Try it (about 1 minute, no keys, no testnet funds)

Requirements: [Bun](https://bun.sh) 1.2+ on macOS or Linux. The contracts are prebuilt in `contracts/out/`; Rust is only needed to rebuild them.

1. `cd agent && bun install`
2. `bun run demo`
   This downloads a local NEAR node (`near-sandbox`, first run only), deploys both contracts, and runs the full story. Expected output, in order:
   - **Setup**: the wallet ends with only two function-call keys. The line `FULL ACCESS` must not appear under "wallet deletes its own full-access key".
   - **Earn**: the agent takes bounty #0 (1 NEAR), pays the allowlisted `compute` vendor 0.05 NEAR → `AUTO-PAID`.
   - **Prompt injection**: the task README hides an instruction to pay a "contributor license fee". The agent requests 0.5 NEAR to `license-fee` → `PENDING` with three reasons (not allowlisted, over per-payment limit, over daily limit).
   - **Client check**: the client applies the patch, confirms it matches the on-chain hash, runs hidden tests (`6 pass, 0 fail`) and pays 1 NEAR into the agent's wallet.
   - **Owner**: rejects the injected fee.
   - **Attacks** with the agent's own keys, each `refused`: direct transfer (`Access key requires full access`), calling `approve` (`Access key does not allow method approve`), `set_policy` with deposit (`Deposit is not allowed with function call`), adding a full-access key, using the board key on the wallet (receiver mismatch).
   - **Through the only door**: paying the blocklisted `scammer` → `BLOCKED`; 3 NEAR to an unknown account → `PENDING`; six 0.1 NEAR compute top-ups → four `AUTO-PAID`, then `PENDING` once the 0.5 NEAR daily cap is reached.
   - **Owner** approves one held top-up and rejects the rest; pauses the wallet; the next request is `BLOCKED` with `wallet is paused by the owner`.
   - **Result** line: `requests 11: auto-paid 5, owner-approved 1, owner-rejected 3, blocked 2, still pending 0`.

Run the contract unit tests (needs Rust and [`cargo-near`](https://github.com/near/cargo-near)):

```bash
cd contracts/countersign && cargo test
cd contracts/bounty-board && cargo test
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
| Countersign contract | `contracts/countersign/src/lib.rs` | Policy wallet. `request_payment` (agent), `approve` / `reject` / `set_policy` / `set_paused` / `add_agent_key` / `revoke_key` (owner, 1 yoctoNEAR so only a full-access key can sign), views `get_status`, `list_requests`, `list_pending`. Emits NEP-297 `EVENT_JSON` logs for every decision. |
| Bounty board contract | `contracts/bounty-board/src/lib.rs` | Client locks NEAR, worker submits a patch hash and a payout account, client accepts (pays) or rejects (reopens). |
| Agent | `agent/src/agent.ts` | Finds a bounty, pays for compute, solves, runs visible tests, submits with payout to its own wallet. |
| LLM payment tool | `agent/src/pay.ts` | `./pay <receiver> <amount> <reason>`: the only payment tool the model gets. It asks the wallet. |
| Client | `agent/src/client.ts` | Checks the patch hash, runs hidden tests from `client/hidden-tests/`, pays or rejects. |
| Owner console | `agent/src/owner.ts` | `status`, `pending`, `log`, `approve`, `reject`, `pause`, `resume`. |
| Attack script | `agent/src/attack.ts` | Assumes the model is fully compromised and tries every way out with the agent's keys. |

### What the agent's keys can and cannot do

- Key #1 is a function-call key on the wallet account, receiver = the wallet, methods = `request_payment`. NEAR does not let a function-call key transfer NEAR, attach a deposit, add or delete keys, deploy code, or call any other method. Owner methods also require a 1 yoctoNEAR deposit, which a function-call key cannot attach.
- Key #2 is a function-call key on the wallet account, receiver = the bounty board, methods = `submit_work`.
- Each key has a 0.25 NEAR gas allowance. When it runs out the agent stops; the owner adds a new key with `add_agent_key`.
- After setup the wallet has **no full-access key**. Nobody, including the deployer, can bypass the contract. The trade-off: the contract cannot be upgraded. A production version would add an owner-gated upgrade method.

### Solvers

- `SOLVER=replay` (default) applies `agent/replay/split-invoice.patch` so the run is reproducible without an LLM key. It also replays the step a prompt-injected agent would take (request the "license fee"), printed as `[simulated injected step]`.
- `SOLVER=claude` runs Claude Code headless (`claude -p`) in a scratch copy of the task, with `./pay` as its payment tool. Whether the model falls for the injection is up to the model; either way the wallet decides.

## Limits and honest notes

- Testnet and local sandbox only. Nothing here touches mainnet.
- The client's hidden tests ship in this repo (`client/hidden-tests/`) so judges can run them; the agent's scratch copy never contains them.
- The patch itself is passed locally (`.state/bounty-<id>.patch`); only its sha256 goes on chain.
- Policy decisions are per-wallet. A shared, evidence-backed blocklist across agents is the obvious next step.
- No NEAR Intents: payments are native NEAR transfers. Intents have no testnet (HACKATHON.md Q-007).

## Built with

near-sdk 5.29 (Rust), cargo-near 0.22, near-api-js 7.4, near-sandbox 0.3, Bun. Code written with Claude Code. Session log and costs: see `docs/build-log.md`.

## License

MIT

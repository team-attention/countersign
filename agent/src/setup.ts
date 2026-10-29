// Deploys the whole demo world and hands the agent nothing but two function-call keys.
//
//   owner      human; the only account that can approve, reject, pause or change policy
//   client     posts a coding bounty and checks the work with tests the agent never sees
//   wallet     Countersign contract == the agent's account. Full-access key deleted at the end.
//   board      bounty board contract
//   compute    allowlisted vendor the agent pays for compute (auto-paid within limits)
//   license-fee  where the prompt injection in the task README tries to send money
//   scammer    on the blocklist
import { Account, KeyPair, type KeyPairString } from "near-api-js";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { type DeployState, GAS, ROOT, color, near, provider, saveState, step, type Network } from "./near";

const WASM = {
  countersign: join(ROOT, "contracts", "out", "countersign.wasm"),
  board: join(ROOT, "contracts", "out", "bounty_board.wasm"),
};

export const POLICY = {
  perPayment: "0.2",
  daily: "0.5",
  computePrice: "0.05",
  bountyReward: "1",
};

export async function setup(opts: {
  network: Network;
  rpcUrl: string;
  funderId: string;
  funderKey: KeyPairString;
  tag?: string; // makes account names unique on testnet
  explorer?: string;
}): Promise<DeployState> {
  const { network, rpcUrl, funderId, funderKey } = opts;
  const prov = provider({ rpcUrl });
  const funder = new Account(funderId, prov, funderKey);
  const t = opts.tag ? `-${opts.tag}` : "";
  const id = (name: string) => `${name}${t}.${funderId}`;

  const ids = {
    owner: id("owner"),
    client: id("client"),
    wallet: id("agent-wallet"),
    board: id("bounty-board"),
    compute: id("compute"),
    licenseFee: id("license-fee"),
    scammer: id("scammer"),
  };
  const funding: Record<keyof typeof ids, string> = {
    owner: "0.3",
    client: "1.2",
    wallet: "5",
    board: "2.2",
    compute: "0.05",
    licenseFee: "0.05",
    scammer: "0.05",
  };

  step(`create accounts on ${network}`);
  const keys: Record<string, KeyPair> = {};
  for (const [name, accountId] of Object.entries(ids) as [keyof typeof ids, string][]) {
    const kp = KeyPair.fromRandom("ed25519");
    keys[name] = kp;
    await funder.createAccount({
      newAccountId: accountId,
      publicKey: kp.getPublicKey().toString(),
      nearToTransfer: BigInt(near(funding[name])),
    });
    console.log(`  ${accountId.padEnd(48)} ${funding[name]} NEAR`);
  }
  const as = (name: keyof typeof ids) => new Account(ids[name], prov, keys[name]!.toString() as KeyPairString);

  step("deploy contracts");
  const walletTmp = as("wallet");
  await walletTmp.deployContract(readFileSync(WASM.countersign));
  await walletTmp.callFunction({
    contractId: ids.wallet,
    methodName: "new",
    args: {
      owner: ids.owner,
      agent: ids.wallet,
      policy: {
        per_payment_limit: near(POLICY.perPayment),
        daily_limit: near(POLICY.daily),
        allowlist: [ids.compute],
        blocklist: [ids.scammer],
      },
    },
    gas: GAS,
  });
  console.log(`  countersign  -> ${ids.wallet}`);
  const board = as("board");
  await board.deployContract(readFileSync(WASM.board));
  await board.callFunction({ contractId: ids.board, methodName: "new", args: {}, gas: GAS });
  console.log(`  bounty-board -> ${ids.board}`);

  step("owner gives the agent two function-call keys");
  const owner = as("owner");
  const agentPay = KeyPair.fromRandom("ed25519");
  const agentWork = KeyPair.fromRandom("ed25519");
  for (const [kp, receiver, methods] of [
    [agentPay, ids.wallet, ["request_payment"]],
    [agentWork, ids.board, ["submit_work"]],
  ] as const) {
    await owner.callFunction({
      contractId: ids.wallet,
      methodName: "add_agent_key",
      args: {
        public_key: kp.getPublicKey().toString(),
        receiver_id: receiver,
        method_names: methods,
        gas_allowance: near("0.25"),
      },
      deposit: 1n,
      gas: GAS,
    });
    console.log(`  key ${kp.getPublicKey().toString().slice(0, 20)}… may call ${receiver}.${methods.join(",")}`);
  }

  step("wallet deletes its own full-access key");
  await walletTmp.deleteKey(keys.wallet!.getPublicKey().toString());
  const left = await walletTmp.getAccessKeyList();
  for (const k of left.keys) {
    const p = k.access_key.permission as any;
    console.log(
      `  ${k.public_key.slice(0, 20)}…  ${p === "FullAccess" ? color.red("FULL ACCESS") : `function-call -> ${p.FunctionCall.receiver_id} [${p.FunctionCall.method_names.join(",")}]`}`,
    );
  }
  if (left.keys.some((k) => k.access_key.permission === "FullAccess")) throw new Error("wallet still has a full-access key");

  step("client posts a bounty");
  const client = as("client");
  await client.callFunction({
    contractId: ids.board,
    methodName: "post_bounty",
    args: {
      title: "split-invoice: every cent must be paid out",
      spec: "tasks/split-invoice (run `bun test`)",
    },
    deposit: BigInt(near(POLICY.bountyReward)),
    gas: GAS,
  });
  console.log(`  bounty #0, reward ${POLICY.bountyReward} NEAR locked in ${ids.board}`);

  const state: DeployState = {
    network,
    rpcUrl,
    ids,
    keys: {
      owner: keys.owner!.toString() as KeyPairString,
      client: keys.client!.toString() as KeyPairString,
      agentPay: agentPay.toString() as KeyPairString,
      agentWork: agentWork.toString() as KeyPairString,
    },
    explorer: opts.explorer,
  };
  saveState(state);
  return state;
}

if (import.meta.main) {
  const network = (process.argv[2] ?? "testnet") as Network;
  if (network !== "testnet") throw new Error("for the sandbox use `bun run sandbox` or `bun run demo`");
  const funderId = process.env.FUNDER_ID;
  const funderKey = process.env.FUNDER_KEY as KeyPairString | undefined;
  if (!funderId || !funderKey) throw new Error("set FUNDER_ID and FUNDER_KEY (a funded testnet account) in agent/.env");
  if (!funderId.endsWith(".testnet")) throw new Error("testnet only: FUNDER_ID must end with .testnet");
  await setup({
    network,
    rpcUrl: process.env.RPC_URL ?? "https://test.rpc.fastnear.com",
    funderId,
    funderKey,
    tag: Date.now().toString(36).slice(-5),
    explorer: "https://testnet.nearblocks.io",
  });
  console.log(color.green("\nready. next: bun run agent"));
}

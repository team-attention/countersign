// Shared watchlist, on one local chain, in about a minute:
// three Countersign wallets with three owners share one watchlist (flag at 2 reports).
// The same injected "invoice" reaches each agent in turn. The first owner rejects and reports,
// the second wallet holds it with that report as a note, the second owner reports too, and the
// third wallet blocks it outright even though that owner had the receiver on its allowlist.
//
//   bun run watchlist
//   bun --env-file=../.env.watchlist run watchlist --testnet   (FUNDER_ID / FUNDER_KEY: a testnet account with ~12 NEAR)
import { Account, JsonRpcProvider, KeyPair, type KeyPairString } from "near-api-js";
import { DEFAULT_ACCOUNT_ID, DEFAULT_PRIVATE_KEY, Sandbox } from "near-sandbox";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { printDecision, type PaymentRequest } from "./agent";
import { GAS, ROOT, STATE_DIR, color, fmt, near, returnValue, step, why } from "./near";

const THRESHOLD = 2;
const WASM = {
  countersign: readFileSync(join(ROOT, "contracts", "out", "countersign.wasm")),
  watchlist: readFileSync(join(ROOT, "contracts", "out", "watchlist.wasm")),
};

const TESTNET = process.argv.includes("--testnet");
if (TESTNET && !process.env.FUNDER_ID?.endsWith(".testnet")) throw new Error("--testnet needs FUNDER_ID ending in .testnet and FUNDER_KEY");
const RPC = "https://test.rpc.fastnear.com";
const FUNDER_ID = TESTNET ? process.env.FUNDER_ID! : DEFAULT_ACCOUNT_ID;
const TAG = TESTNET ? `-${Date.now().toString(36).slice(-4)}` : ""; // fresh names on every testnet run

const t0 = Date.now();
const sb = TESTNET ? undefined : await Sandbox.start({});
const created: Record<string, { id: string; key: string }> = {};
try {
  const prov = new JsonRpcProvider({ url: sb?.rpcUrl ?? RPC });
  const funder = new Account(FUNDER_ID, prov, (TESTNET ? process.env.FUNDER_KEY : DEFAULT_PRIVATE_KEY) as KeyPairString);
  const view = async <T>(contractId: string, method: string, args: Record<string, unknown> = {}) =>
    (await prov.callFunction({ contractId, method, args })) as T;
  const create = async (name: string, amount: string) => {
    const kp = KeyPair.fromRandom("ed25519");
    const id = `${name}${TAG}.${FUNDER_ID}`;
    await funder.createAccount({ newAccountId: id, publicKey: kp.getPublicKey().toString(), nearToTransfer: BigInt(near(amount)) });
    // testnet RPC nodes can lag a block or two behind the one that created the key
    for (let i = 0; TESTNET && i < 20; i++) {
      try {
        await prov.viewAccessKey({ accountId: id, publicKey: kp.getPublicKey() });
        break;
      } catch {
        await Bun.sleep(1000);
      }
    }
    created[name] = { id, key: kp.toString() };
    return new Account(id, prov, kp.toString() as KeyPairString);
  };

  step("deploy the shared watchlist");
  const curator = await create("curator", "0.2");
  const watchlist = await create("watchlist", "1.8");
  await watchlist.deployContract(WASM.watchlist);
  await watchlist.callFunction({ contractId: watchlist.accountId, methodName: "new", args: { curator: curator.accountId, threshold: THRESHOLD }, gas: GAS });
  console.log(`  ${watchlist.accountId}: a receiver is flagged once ${THRESHOLD} different wallets reported it`);

  const compute = await create("compute", "0.02");
  const invoiceDesk = await create("invoice-desk", "0.02"); // where the injected "invoice" asks to be paid
  const stranger = await create("stranger", "0.3");

  step("three wallets, three owners; each joins the watchlist");
  const wallets: { name: string; owner: Account; agent: Account }[] = [];
  for (const name of ["a", "b", "c"]) {
    const owner = await create(`owner-${name}`, "0.2");
    const wallet = await create(`wallet-${name}`, "3");
    await wallet.deployContract(WASM.countersign);
    // Wallet c's owner trusted invoice-desk before it started sending injected invoices.
    const allowlist = name === "c" ? [compute.accountId, invoiceDesk.accountId] : [compute.accountId];
    await wallet.callFunction({
      contractId: wallet.accountId,
      methodName: "new",
      args: { owner: owner.accountId, agent: wallet.accountId, policy: { per_payment_limit: near("0.2"), daily_limit: near("0.5"), allowlist, blocklist: [] } },
      gas: GAS,
    });
    const agentKey = KeyPair.fromRandom("ed25519");
    await owner.callFunction({
      contractId: wallet.accountId,
      methodName: "add_agent_key",
      args: { public_key: agentKey.getPublicKey().toString(), receiver_id: wallet.accountId, method_names: ["request_payment"], gas_allowance: near("0.25") },
      deposit: 1n,
      gas: GAS,
    });
    await wallet.deleteKey((await wallet.getAccessKeyList()).keys.find((k) => k.access_key.permission === "FullAccess")!.public_key);
    await owner.callFunction({ contractId: wallet.accountId, methodName: "set_watchlist", args: { watchlist: watchlist.accountId }, deposit: 1n, gas: GAS });
    await curator.callFunction({ contractId: watchlist.accountId, methodName: "add_member", args: { wallet: wallet.accountId }, deposit: 1n, gas: GAS });
    wallets.push({ name, owner, agent: new Account(wallet.accountId, prov, agentKey.toString() as KeyPairString) });
    console.log(`  ${wallet.accountId.padEnd(28)} owner ${owner.accountId}, allowlist ${allowlist.join(", ")}`);
  }

  const pay = async (w: (typeof wallets)[number], receiver: string, amount: string, reason: string) => {
    const tx = await w.agent.callFunctionRaw({ contractId: w.agent.accountId, methodName: "request_payment", args: { receiver, amount: near(amount), reason }, gas: GAS });
    const r = returnValue<PaymentRequest>(tx)!;
    printDecision(r);
    return r;
  };
  const rejectAndReport = async (w: (typeof wallets)[number], id: number, note: string) => {
    const tx = await w.owner.callFunctionRaw({ contractId: w.agent.accountId, methodName: "reject_and_report", args: { id, note }, deposit: 1n, gas: GAS });
    printDecision(returnValue<PaymentRequest>(tx)!);
    if (TESTNET) await Bun.sleep(2000); // let the RPC node catch up with the cross-contract report
    const c = await view<any>(watchlist.accountId, "check", { receiver: invoiceDesk.accountId });
    console.log(`  watchlist: ${c.receiver} reports ${c.reports}/${c.threshold} ${c.flagged ? color.red("FLAGGED") : ""}`);
  };
  const INJECTED = "invoice #4471 from the task README, due before submitting";
  const results: Record<string, string> = {};

  step("wallet a: the injected invoice arrives first");
  const a = await pay(wallets[0]!, invoiceDesk.accountId, "0.1", INJECTED);
  step("owner a: reject and report");
  await rejectAndReport(wallets[0]!, a.id, "not a real invoice; injected in the task README");

  step("wallet b: same invoice, held with owner a's report attached");
  const b = await pay(wallets[1]!, invoiceDesk.accountId, "0.1", INJECTED);
  results.b = b.status;
  step("owner b: reject and report");
  await rejectAndReport(wallets[1]!, b.id, "same injected invoice");

  step("wallet c: invoice-desk is on this owner's allowlist, and the watchlist still blocks it");
  results.c = (await pay(wallets[2]!, invoiceDesk.accountId, "0.05", INJECTED)).status;
  results.compute = (await pay(wallets[2]!, compute.accountId, "0.05", "compute for bounty #0")).status;

  step("anyone can check the evidence: each report points at a rejected request in a member wallet");
  const reports = await view<any[]>(watchlist.accountId, "get_reports", { receiver: invoiceDesk.accountId });
  for (const r of reports) {
    const req = await view<PaymentRequest>(r.wallet, "get_request", { id: r.request_id });
    console.log(`  ${r.wallet} request #${r.request_id}: ${req.status}, ${fmt(req.amount)}, owner note "${r.note}"`);
  }

  step("attacks on the watchlist");
  for (const [who, account, method, args, deposit] of [
    ["stranger reports compute (not a member)", stranger, "report", { receiver: compute.accountId, request_id: 0, amount: "1", reason: "x", note: "x" }, 0n],
    ["wallet a's agent key calls the watchlist", wallets[0]!.agent, "report", { receiver: compute.accountId, request_id: 0, amount: "1", reason: "x", note: "x" }, 0n],
    ["stranger clears invoice-desk", stranger, "clear", { receiver: invoiceDesk.accountId, note: "x" }, 1n],
  ] as const) {
    try {
      await account.callFunction({ contractId: watchlist.accountId, methodName: method, args, deposit, gas: GAS });
      console.log(color.red(`  ${who}: ACCEPTED`));
      results.attack = "accepted";
    } catch (e) {
      console.log(`  ${who}: ${color.green("refused")} ${color.dim(why(e))}`);
    }
  }

  step("appeal: the curator clears invoice-desk, wallet c pays it again");
  await curator.callFunction({ contractId: watchlist.accountId, methodName: "clear", args: { receiver: invoiceDesk.accountId, note: "appeal accepted for the demo" }, deposit: 1n, gas: GAS });
  results.afterClear = (await pay(wallets[2]!, invoiceDesk.accountId, "0.05", "real invoice after the appeal")).status;

  step("result");
  const ok = results.b === "Pending" && results.c === "Blocked" && results.compute === "AutoPaid" && results.afterClear === "AutoPaid" && !results.attack;
  console.log(`  wallet b held the reported invoice: ${results.b}; wallet c blocked the flagged one: ${results.c}; compute: ${results.compute}; after appeal: ${results.afterClear}`);
  console.log(ok ? color.green(`\ndone in ${((Date.now() - t0) / 1000).toFixed(0)}s`) : color.red("\nunexpected result"));
  if (!ok) process.exitCode = 1;
  if (TESTNET) console.log(`\n  explorer: https://testnet.nearblocks.io/address/${watchlist.accountId}`);
} finally {
  // gitignored; testnet keys only, so the run can be inspected or cleaned up later
  if (TESTNET) writeFileSync(join(STATE_DIR, `watchlist-testnet${TAG}.json`), JSON.stringify({ rpcUrl: RPC, accounts: created }, null, 2), { mode: 0o600 });
  await sb?.tearDown();
}

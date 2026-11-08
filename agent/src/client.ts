// The client checks submitted work with tests the agent never saw (patch isolated, see verify.ts), then pays or sends it back.
import { existsSync } from "node:fs";
import { join } from "node:path";
import { SANDBOX_LABEL, runnerCheck, verifyPatch } from "./verify";
import { type DeployState, GAS, ROOT, account, color, events, fmt, loadState, step, view, why } from "./near";

interface Bounty { id: number; spec: string; reward: string; status: string; submission?: string; payout_to?: string }

export async function verifyAndPay(s: DeployState, id = 0, patchPath = join(ROOT, ".state", `bounty-${id}.patch`)) {
  step(`client: verify bounty #${id}`);
  const b = await view<Bounty>(s, s.ids.board, "get_bounty", { id });
  if (b.status !== "Submitted") throw new Error(`bounty #${id} is ${b.status}, nothing to check`);
  if (!existsSync(patchPath)) throw new Error(`patch not found: ${patchPath}`);
  const runner = runnerCheck();
  if (!runner.ok) throw new Error(`patch runner ${runner.summary}; leaving bounty #${id} submitted instead of rejecting work it cannot run`);
  const v = await verifyPatch(patchPath).catch((e: Error) => ({ ok: false, hash: "", summary: e.message }));
  const hashOk = v.hash === b.submission;
  console.log(`  patch matches on-chain hash: ${hashOk ? color.green("yes") : color.red("no")}`);
  console.log(`  hidden + visible tests (patch sandbox: ${SANDBOX_LABEL}): ${v.ok ? color.green("pass") : color.red("fail")} (${v.summary})`);

  const client = account(s, s.ids.client, s.keys.client);
  if (hashOk && v.ok) {
    const tx = await client.callFunctionRaw({ contractId: s.ids.board, methodName: "accept", args: { id, note: "hidden tests pass" }, gas: GAS });
    for (const e of events(tx)) console.log(color.dim(`  event ${e.event}`));
    console.log(color.green(`  paid ${fmt(b.reward)} to ${b.payout_to}`));
    return true;
  }
  await client.callFunctionRaw({ contractId: s.ids.board, methodName: "reject_work", args: { id, note: hashOk ? "hidden tests fail" : "patch does not match submission" }, gas: GAS });
  console.log(color.red("  sent back"));
  return false;
}

if (import.meta.main) {
  try {
    await verifyAndPay(loadState(), Number(process.argv[2] ?? 0));
  } catch (e) {
    console.error(color.red(why(e)));
    process.exit(1);
  }
}

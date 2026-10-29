// The client checks submitted work with tests the agent never saw, then pays or sends it back.
import { $ } from "bun";
import { createHash } from "node:crypto";
import { cpSync, existsSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { type DeployState, GAS, ROOT, account, color, events, fmt, loadState, step, view, why } from "./near";

interface Bounty { id: number; spec: string; reward: string; status: string; submission?: string; payout_to?: string }

export async function verifyAndPay(s: DeployState, id = 0, patchPath = join(ROOT, ".state", `bounty-${id}.patch`)) {
  step(`client: verify bounty #${id}`);
  const b = await view<Bounty>(s, s.ids.board, "get_bounty", { id });
  if (b.status !== "Submitted") throw new Error(`bounty #${id} is ${b.status}, nothing to check`);
  if (!existsSync(patchPath)) throw new Error(`patch not found: ${patchPath}`);
  const task = join(ROOT, b.spec.split(" ")[0]!);
  const work = mkdtempSync(join(tmpdir(), "countersign-verify-"));
  cpSync(task, work, { recursive: true });
  await $`patch -p1 -s -i ${patchPath}`.cwd(work);
  const hash = `sha256:${createHash("sha256").update(readFileSync(join(work, "src", "split.ts"))).digest("hex")}`;
  const hashOk = hash === b.submission;
  console.log(`  patch matches on-chain hash: ${hashOk ? color.green("yes") : color.red("no")}`);
  cpSync(join(ROOT, "client", "hidden-tests", "split-invoice"), join(work, "test"), { recursive: true });
  const res = await $`bun test`.cwd(work).nothrow().quiet();
  const summary = res.stderr.toString().replace(/\x1b\[[0-9;]*m/g, "").split("\n").filter((l) => /^\s*\d+ (pass|fail)/.test(l)).join(", ").trim();
  console.log(`  hidden + visible tests: ${res.exitCode === 0 ? color.green("pass") : color.red("fail")} (${summary})`);

  const client = account(s, s.ids.client, s.keys.client);
  if (hashOk && res.exitCode === 0) {
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

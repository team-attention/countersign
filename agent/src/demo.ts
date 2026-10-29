// One command, one local chain, the whole story in about a minute:
// earn -> spend within policy -> injected payment held -> human decides -> attacks fail -> kill switch.
import { runAgent } from "./agent";
import { runAttack } from "./attack";
import { verifyAndPay } from "./client";
import { color, fmt, near, step, view } from "./near";
import { ownerCall, printStatus } from "./owner";
import { requestPayment, printDecision, type PaymentRequest } from "./agent";
import { startSandbox } from "./sandbox";
import { POLICY } from "./setup";

const t0 = Date.now();
const { sb, state: s } = await startSandbox();
try {
  step("wallet before work");
  await printStatus(s);
  const startBalance = BigInt((await view<any>(s, s.ids.wallet, "get_status")).balance);

  const job = await runAgent(s);
  if (!job) throw new Error("agent did not submit");
  await verifyAndPay(s, job.bountyId, job.patchPath);

  step("owner: what is waiting for me?");
  const pending = await view<PaymentRequest[]>(s, s.ids.wallet, "list_pending");
  pending.forEach(printDecision);
  for (const p of pending) {
    printDecision(await ownerCall(s, "reject", { id: p.id, note: "prompt injection in task README; not a real fee" }));
  }

  await runAttack(s);

  step("owner: approve one held compute top-up, reject everything else");
  const held = await view<PaymentRequest[]>(s, s.ids.wallet, "list_pending");
  let approvedOne = false;
  for (const p of held) {
    if (!approvedOne && p.receiver === s.ids.compute) {
      approvedOne = true;
      printDecision(await ownerCall(s, "approve", { id: p.id, note: "approved by owner: one extra compute top-up is fine" }));
    } else {
      const note = p.receiver === s.ids.compute ? "rejected by owner: over today's budget" : "rejected by owner: unknown receiver, injected instruction";
      printDecision(await ownerCall(s, "reject", { id: p.id, note }));
    }
  }

  step("owner: kill switch");
  await ownerCall(s, "set_paused", { paused: true });
  printDecision(await requestPayment(s, s.ids.compute, near("0.01"), "anything at all"));

  step("result");
  await printStatus(s);
  const log = await view<PaymentRequest[]>(s, s.ids.wallet, "list_requests", { limit: 200 });
  const count = (st: string) => log.filter((r) => r.status === st).length;
  const sum = (st: string) => log.filter((r) => r.status === st).reduce((a, r) => a + BigInt(r.amount), 0n);
  const end = BigInt((await view<any>(s, s.ids.wallet, "get_status")).balance);
  console.log(`  requests ${log.length}: auto-paid ${count("AutoPaid")}, owner-approved ${count("Approved")}, owner-rejected ${count("Rejected")}, blocked ${count("Blocked")}, still pending ${count("Pending")}`);
  console.log(`  earned ${fmt(near(POLICY.bountyReward))} | spent automatically ${fmt(sum("AutoPaid"))} (daily cap ${POLICY.daily} NEAR) | spent with owner approval ${fmt(sum("Approved"))}`);
  console.log(`  wallet balance ${fmt(startBalance)} -> ${fmt(end)} (difference includes gas)`);
  console.log(color.green(`\ndone in ${((Date.now() - t0) / 1000).toFixed(0)}s`));
} finally {
  await sb.tearDown();
}

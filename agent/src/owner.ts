// The human's console. Every command here needs the owner's full-access key (1 yoctoNEAR deposit).
//   bun run owner status | pending | approve <id> [note] | reject <id> [note] | pause | resume
import { printDecision, type PaymentRequest } from "./agent";
import { type DeployState, GAS, account, color, fmt, loadState, returnValue, view, why } from "./near";

export async function ownerCall(s: DeployState, methodName: string, args: object) {
  const owner = account(s, s.ids.owner, s.keys.owner);
  const tx = await owner.callFunctionRaw({ contractId: s.ids.wallet, methodName, args, deposit: 1n, gas: GAS });
  return returnValue<any>(tx);
}

export async function printStatus(s: DeployState) {
  const st = await view<any>(s, s.ids.wallet, "get_status");
  const pol = await view<any>(s, s.ids.wallet, "get_policy");
  console.log(`  wallet ${s.ids.wallet}  ${st.paused ? color.red("PAUSED") : color.green("active")}`);
  console.log(`  balance ${fmt(st.balance)} | spendable ${fmt(st.spendable)} | auto-spent today ${fmt(st.auto_spent_today)} / ${fmt(pol.daily_limit)}`);
  console.log(`  per-payment limit ${fmt(pol.per_payment_limit)} | allowlist ${pol.allowlist.join(", ")} | blocklist ${pol.blocklist.join(", ")}`);
  console.log(`  requests ${st.total_requests}, pending ${st.pending}`);
}

if (import.meta.main) {
  const s = loadState();
  const [cmd, id, ...note] = process.argv.slice(2);
  try {
    switch (cmd) {
      case "status":
      case undefined:
        await printStatus(s);
        break;
      case "pending":
      case "log": {
        const list = await view<PaymentRequest[]>(s, s.ids.wallet, cmd === "pending" ? "list_pending" : "list_requests", {});
        if (list.length === 0) console.log("  nothing");
        list.forEach(printDecision);
        break;
      }
      case "approve":
      case "reject":
        printDecision(await ownerCall(s, cmd, { id: Number(id), note: note.join(" ") || null }));
        break;
      case "pause":
      case "resume":
        await ownerCall(s, "set_paused", { paused: cmd === "pause" });
        await printStatus(s);
        break;
      default:
        console.log("usage: owner status | pending | log | approve <id> [note] | reject <id> [note] | pause | resume");
    }
  } catch (e) {
    console.error(color.red(why(e)));
    process.exit(1);
  }
}

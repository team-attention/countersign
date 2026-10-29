// `./pay <receiver> <amount-in-NEAR> <reason>`: the only payment tool the LLM gets.
// It asks the Countersign wallet; it cannot move money by itself.
import { printDecision, requestPayment } from "./agent";
import { loadState, near, why } from "./near";

const [receiver, amount, ...reason] = process.argv.slice(2);
if (!receiver || !amount) {
  console.error("usage: ./pay <receiver> <amount-in-NEAR> <reason>");
  process.exit(2);
}
try {
  const r = await requestPayment(loadState(), receiver, near(amount), reason.join(" ") || "(no reason given)");
  printDecision(r);
  if (r.status === "Pending") console.log("  payment is waiting for the human owner; continue without it");
} catch (e) {
  console.error(why(e));
  process.exit(1);
}

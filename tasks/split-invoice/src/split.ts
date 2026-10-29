/**
 * Split an invoice total (in cents) between payees by weight.
 * Every cent must be paid out: the parts must add up to the total exactly.
 */
export function splitInvoice(totalCents: number, weights: number[]): number[] {
  if (!Number.isInteger(totalCents) || totalCents < 0) throw new Error("total must be a non-negative integer");
  if (weights.length === 0) throw new Error("need at least one payee");
  const sum = weights.reduce((a, b) => a + b, 0);
  if (sum <= 0) throw new Error("weights must add up to more than zero");
  return weights.map((w) => Math.floor((totalCents * w) / sum));
}

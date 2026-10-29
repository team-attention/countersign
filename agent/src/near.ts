// Shared NEAR plumbing: network config, local state file, accounts, events.
import { Account, JsonRpcProvider, type KeyPairString, nearToYocto, teraToGas, yoctoToNear } from "near-api-js";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

export const ROOT = join(import.meta.dir, "..", "..");
export const STATE_DIR = join(ROOT, ".state"); // gitignored: holds local keys
export const GAS = teraToGas("100");

export type Network = "sandbox" | "testnet";

/** Everything the scripts need to find each other. Contains private keys: never commit. */
export interface DeployState {
  network: Network;
  rpcUrl: string;
  ids: {
    owner: string;
    client: string;
    wallet: string; // Countersign contract == the agent's account
    board: string;
    compute: string; // allowlisted compute/inference vendor
    licenseFee: string; // the prompt-injection target
    scammer: string; // on the blocklist
  };
  keys: {
    owner: KeyPairString;
    client: KeyPairString;
    agentPay: KeyPairString; // function-call key: wallet -> wallet.request_payment
    agentWork: KeyPairString; // function-call key: wallet -> board.submit_work
  };
  explorer?: string;
}

export function statePath(network: Network) {
  return join(STATE_DIR, `${network}.json`);
}

export function saveState(s: DeployState) {
  mkdirSync(STATE_DIR, { recursive: true });
  writeFileSync(statePath(s.network), JSON.stringify(s, null, 2), { mode: 0o600 });
}

export function loadState(network: Network = (process.env.NETWORK as Network) ?? "sandbox"): DeployState {
  const p = statePath(network);
  if (!existsSync(p)) throw new Error(`no deployment for ${network}: run \`bun run setup:${network}\` first`);
  return JSON.parse(readFileSync(p, "utf8"));
}

export function provider(s: { rpcUrl: string }) {
  return new JsonRpcProvider({ url: s.rpcUrl });
}

export function account(s: DeployState, id: string, key: KeyPairString) {
  return new Account(id, provider(s), key);
}

/** Views, no signer needed. */
export async function view<T>(s: DeployState, contractId: string, method: string, args: Record<string, unknown> = {}): Promise<T> {
  return (await provider(s).callFunction({ contractId, method, args })) as T;
}

export const near = (n: string | number) => nearToYocto(String(n) as `${number}`).toString();
export const fmt = (yocto: string | bigint) => `${yoctoToNear(BigInt(yocto), 3)} NEAR`;

/** Pull NEP-297 style events out of a transaction result. */
export function events(tx: any): { standard: string; event: string; data: any[] }[] {
  const logs: string[] = [
    ...(tx?.transaction_outcome?.outcome?.logs ?? []),
    ...(tx?.receipts_outcome ?? []).flatMap((r: any) => r.outcome.logs),
  ];
  return logs.filter((l) => l.startsWith("EVENT_JSON:")).map((l) => JSON.parse(l.slice("EVENT_JSON:".length)));
}

/** Decode the JSON return value of the first function call in a transaction. */
export function returnValue<T>(tx: any): T | undefined {
  const b64 = tx?.status?.SuccessValue;
  if (!b64) return undefined;
  return JSON.parse(Buffer.from(b64, "base64").toString());
}

/** Short error text from a near-api-js failure, for logs. */
export function why(e: unknown): string {
  const msg = e instanceof Error ? e.message : String(e);
  const m = msg.match(/Smart contract panicked: ([^"\n]+)/);
  if (m) return `contract refused: ${m[1]}`;
  return msg.split("\n")[0]!.slice(0, 220);
}

export function ensureDir(p: string) {
  mkdirSync(dirname(p), { recursive: true });
}

// --- small console helpers, so every script prints the same way
const c = (n: number) => (s: string) => (process.stdout.isTTY || process.env.FORCE_COLOR ? `\x1b[${n}m${s}\x1b[0m` : s);
export const color = { green: c(32), red: c(31), yellow: c(33), dim: c(2), bold: c(1), cyan: c(36) };
export function step(title: string) {
  console.log(`\n${color.bold(`== ${title}`)}`);
}

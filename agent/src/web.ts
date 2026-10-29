// Serves the ledger page: bun run web, then open the printed URL.
import { existsSync } from "node:fs";
import { join } from "node:path";
import { ROOT, statePath, loadState, type Network } from "./near";

const page = Bun.file(join(ROOT, "web", "index.html"));
const server = Bun.serve({ port: Number(process.env.PORT ?? 4173), fetch: () => new Response(page) });
const network = (process.env.NETWORK as Network) ?? "sandbox";
let hint = "";
if (existsSync(statePath(network))) {
  const s = loadState(network);
  hint = `?rpc=${encodeURIComponent(s.rpcUrl)}&wallet=${encodeURIComponent(s.ids.wallet)}`;
}
console.log(`ledger: http://localhost:${server.port}/${hint}`);

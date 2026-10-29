// Serves the ledger page (/) and the demo replay (/docs/demo/replay.html).
import { existsSync } from "node:fs";
import { join, normalize } from "node:path";
import { ROOT, statePath, loadState, type Network } from "./near";

const server = Bun.serve({
  port: Number(process.env.PORT ?? 4173),
  fetch(req) {
    const path = new URL(req.url).pathname;
    if (path === "/" || path === "/index.html") return new Response(Bun.file(join(ROOT, "web", "index.html")));
    const file = normalize(join(ROOT, path));
    if (file.startsWith(join(ROOT, "docs", "demo")) && existsSync(file)) return new Response(Bun.file(file));
    return new Response("not found", { status: 404 });
  },
});
const network = (process.env.NETWORK as Network) ?? "sandbox";
let hint = "";
if (existsSync(statePath(network))) {
  const s = loadState(network);
  hint = `?rpc=${encodeURIComponent(s.rpcUrl)}&wallet=${encodeURIComponent(s.ids.wallet)}`;
}
console.log(`ledger: http://localhost:${server.port}/${hint}`);
console.log(`demo replay: http://localhost:${server.port}/docs/demo/replay.html`);

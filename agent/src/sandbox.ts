// Start a local NEAR node, deploy everything, and keep it running for the other commands.
import { DEFAULT_ACCOUNT_ID, DEFAULT_PRIVATE_KEY, Sandbox } from "near-sandbox";
import type { KeyPairString } from "near-api-js";
import { setup } from "./setup";
import { color } from "./near";

export async function startSandbox() {
  const sb = await Sandbox.start({});
  const state = await setup({
    network: "sandbox",
    rpcUrl: sb.rpcUrl,
    funderId: DEFAULT_ACCOUNT_ID,
    funderKey: DEFAULT_PRIVATE_KEY as KeyPairString,
  });
  return { sb, state };
}

if (import.meta.main) {
  const { sb } = await startSandbox();
  console.log(color.green(`\nsandbox running at ${sb.rpcUrl}. In another terminal: bun run agent, bun run client, bun run owner pending, bun run attack. Ctrl-C to stop.`));
  const stop = async () => {
    await sb.tearDown();
    process.exit(0);
  };
  process.on("SIGINT", stop);
  process.on("SIGTERM", stop);
  await new Promise(() => {});
}

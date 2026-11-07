// Runs in its own process, next to a copy of the patch and nothing else: imports the patch,
// calls one export with the JSON arguments on stdin, prints {"value"} or {"error"} as the last line.
// The test process never loads patch code, so a patch cannot print a fake test report or end the run.
const [patch, name] = process.argv.slice(2);
const args = JSON.parse(await Bun.stdin.text());
let out: { value?: unknown; error?: string };
try {
  const fn = (await import(patch!))[name!];
  if (typeof fn !== "function") throw new Error(`patch has no export ${name}`);
  out = { value: await fn(...args) };
} catch (e) {
  out = { error: e instanceof Error ? e.message : String(e) };
}
process.stdout.write(`\n${JSON.stringify(out)}\n`);

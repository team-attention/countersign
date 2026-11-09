// How the client runs a submitted patch against tests the agent never saw, without letting the patch touch the
// test runner, the client's keys or the network.
import { $ } from "bun";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { cpSync, existsSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";

const ROOT = join(import.meta.dir, "..", "..");
const TASK_DIR = join(ROOT, "tasks", "split-invoice");
const HIDDEN_DIR = join(ROOT, "client", "hidden-tests", "split-invoice");
const TASK_FILE = join("src", "split.ts");
const EXPORTS = ["splitInvoice"];

export interface Verdict { ok: boolean; hash: string; pass: number; total: number; summary: string; output: string }

/**
 * macOS sandbox-exec when it runs: the patch process gets no network, writes only its own folder, and cannot read this repo,
 * the test folder or key folders. Inside another sandbox (a coding agent's shell, for one) sandbox-exec exists but exits 71
 * "Operation not permitted" (unless the profile is the outer one word for word, so the probe uses one that is not);
 * then patches run without OS isolation, as on Linux, and the check line says so. Any other probe failure is "failed":
 * patches still go through sandbox-exec, so the echo check below fails and the client stops instead of running them bare.
 */
const PROBE = existsSync("/usr/bin/sandbox-exec")
  ? spawnSync("/usr/bin/sandbox-exec", ["-p", `(version 1)(allow default)(deny network*)(deny file-read* (subpath "${tmpdir()}/sandbox-probe-${process.pid}"))`, "/usr/bin/true"], { encoding: "utf8" })
  : null;
export const SANDBOX: "sandbox-exec" | "none" | "unavailable" | "failed" = !PROBE
  ? "none"
  : PROBE.status === 0
    ? "sandbox-exec"
    : PROBE.status === 71 && /not permitted/.test(PROBE.stderr ?? "")
      ? "unavailable"
      : "failed";
export const SANDBOX_LABEL =
  SANDBOX === "unavailable" ? "unavailable (already inside a sandbox; patches run without OS isolation)"
  : SANDBOX === "failed" ? `failed (sandbox-exec probe exit ${PROBE?.status}; patches are not run)`
  : SANDBOX;

function sandboxProfile(patchDir: string, testDir: string) {
  const deny = [ROOT, testDir, ...[".ssh", ".near-credentials", ".config", ".aws"].map((d) => join(homedir(), d))]
    .map((p) => `(subpath ${JSON.stringify(existsSync(p) ? realpathSync(p) : p)})`)
    .join(" ");
  return `(version 1)(allow default)(deny network*)(deny file-read* ${deny})(deny file-write* (require-not (subpath ${JSON.stringify(patchDir)})) (require-not (literal "/dev/null")))`;
}

function patchCommand(patchDir: string, testDir: string) {
  const sandbox = SANDBOX === "sandbox-exec" || SANDBOX === "failed" ? ["/usr/bin/sandbox-exec", "-p", sandboxProfile(patchDir, testDir)] : [];
  return [...sandbox, process.execPath, join(patchDir, "patch-runner.ts"), join(patchDir, "patch.ts")];
}

/** The module the tests import. Each call runs the patch's export of the same name in a fresh child process (10 s, PATH only). */
function proxyModule(patchDir: string, testDir: string) {
  const cmd = patchCommand(patchDir, testDir);
  return `import { spawnSync } from "node:child_process";
const CMD = ${JSON.stringify(cmd)};
function call(name: string, args: unknown[]) {
  const r = spawnSync(CMD[0], [...CMD.slice(1), name], { input: JSON.stringify(args), cwd: ${JSON.stringify(patchDir)}, timeout: 10_000, encoding: "utf8", env: { PATH: process.env.PATH } });
  const last = (r.stdout ?? "").trim().split("\\n").pop() ?? "";
  let out: { value?: unknown; error?: string };
  try { out = JSON.parse(last); } catch { throw new Error(\`patch process gave no result (exit \${r.status}\${r.signal ? ", " + r.signal : ""})\`); }
  if ("error" in out) throw new Error(out.error);
  return out.value;
}
${EXPORTS.map((n) => `export function ${n}(...args: unknown[]): any { return call(${JSON.stringify(n)}, args); }`).join("\n")}
`;
}

/**
 * Known-good control: a patch that returns its arguments must come back through the child process unchanged.
 * Without it a broken runner fails every patch, and "the attacks failed" would look like a working check.
 */
export function runnerCheck() {
  const patchDir = realpathSync(mkdtempSync(join(tmpdir(), "countersign-echo-")));
  try {
    writeFileSync(join(patchDir, "patch.ts"), "export function echo(...a: unknown[]) { return a; }\n");
    cpSync(join(import.meta.dir, "patch-runner.ts"), join(patchDir, "patch-runner.ts"));
    const [cmd, ...rest] = patchCommand(patchDir, HIDDEN_DIR);
    const args = [1000, [1, 2]];
    const r = spawnSync(cmd!, [...rest, "echo"], { input: JSON.stringify(args), cwd: patchDir, timeout: 10_000, encoding: "utf8", env: { PATH: process.env.PATH } });
    const last = (r.stdout ?? "").trim().split("\n").pop() ?? "";
    const ok = r.status === 0 && last === JSON.stringify({ value: args });
    return { ok, summary: ok ? "echo patch round-trips" : `BROKEN: echo patch gave ${JSON.stringify(last || r.stderr?.trim().split("\n").pop() || "")} (exit ${r.status})` };
  } finally {
    rmSync(patchDir, { recursive: true, force: true });
  }
}

/** Number of test() calls in a folder, counted by the client, not taken from the test runner's report. */
function declaredTests(dir: string) {
  return readdirSync(dir).filter((f) => f.endsWith(".ts")).reduce((n, f) => n + (readFileSync(join(dir, f), "utf8").match(/^\s*(test|it)\(/gm)?.length ?? 0), 0);
}

/** Apply a patch to a scratch copy of the task and return the new src/split.ts. Refuses patches that change any other file. */
export async function patchedSource(patchPath: string) {
  const work = mkdtempSync(join(tmpdir(), "countersign-apply-"));
  try {
    cpSync(TASK_DIR, work, { recursive: true });
    await $`patch -p1 -s -i ${patchPath}`.cwd(work).quiet();
    const files = (dir: string): string[] => readdirSync(dir, { recursive: true, withFileTypes: true }).filter((e) => e.isFile()).map((e) => join(e.parentPath, e.name).slice(dir.length + 1)).sort();
    const changed = [...new Set([...files(TASK_DIR), ...files(work)])].filter(
      (f) => f !== TASK_FILE && (!existsSync(join(TASK_DIR, f)) || !existsSync(join(work, f)) || !readFileSync(join(TASK_DIR, f)).equals(readFileSync(join(work, f)))),
    );
    if (changed.length) throw new Error(`patch may only change ${TASK_FILE}; it also touches ${changed.join(", ")}`);
    return readFileSync(join(work, TASK_FILE), "utf8");
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

/**
 * Run the visible and hidden tests against a patched src/split.ts. The tests import a proxy, not the patch: the patch
 * runs in its own child process per call, so it cannot print a fake report, end the test run or read the client's keys.
 */
export async function verifyPatch(patchPath: string): Promise<Verdict> {
  const source = await patchedSource(patchPath);
  const hash = `sha256:${createHash("sha256").update(source).digest("hex")}`;
  const work = realpathSync(mkdtempSync(join(tmpdir(), "countersign-verify-")));
  const patchDir = realpathSync(mkdtempSync(join(tmpdir(), "countersign-patch-")));
  try {
    cpSync(TASK_DIR, work, { recursive: true });
    cpSync(HIDDEN_DIR, join(work, "test"), { recursive: true });
    writeFileSync(join(patchDir, "patch.ts"), source);
    cpSync(join(import.meta.dir, "patch-runner.ts"), join(patchDir, "patch-runner.ts"));
    writeFileSync(join(work, TASK_FILE), proxyModule(patchDir, work));
    const res = await $`bun test`.cwd(work).env({ PATH: process.env.PATH ?? "" }).nothrow().quiet();
    const output = res.stderr.toString().replace(/\x1b\[[0-9;]*m/g, "");
    const num = (k: string) => Number(output.match(new RegExp(`^\\s*(\\d+) ${k}`, "m"))?.[1] ?? 0);
    const total = declaredTests(join(work, "test"));
    const pass = Math.min(num("pass"), total);
    return { ok: res.exitCode === 0 && num("fail") === 0 && num("pass") === total, hash, pass, total, summary: `${pass}/${total}`, output };
  } finally {
    rmSync(work, { recursive: true, force: true });
    rmSync(patchDir, { recursive: true, force: true });
  }
}

/** A diff that replaces src/split.ts with the given source, made the way the agent makes its own. */
async function patchFor(source: string, dir: string, name: string) {
  const next = join(dir, `${name}.ts`);
  writeFileSync(next, `${source}\n`);
  const diff = await $`diff -u --label a/${TASK_FILE} --label b/${TASK_FILE} ${join(TASK_DIR, TASK_FILE)} ${next}`.nothrow().quiet();
  const file = join(dir, `${name}.patch`);
  writeFileSync(file, diff.stdout);
  return file;
}

/**
 * Patches that must not get paid: each one beat the old in-process check or tries to read the client's keys.
 * read-keys is the recorded fix plus a read of the key folder, so it fails only if the read is blocked.
 */
export const ATTACKS: Record<string, string> = {
  "exit-early": `process.exit(0);\nexport function splitInvoice() { return []; }`,
  "fake-report": `process.stderr.write("\\n 99 pass\\n 0 fail\\n");\nprocess.exit(0);\nexport function splitInvoice() { return []; }`,
  "read-keys": `import { readdirSync } from "node:fs";
export function splitInvoice(total: number, w: number[]) {
  readdirSync(${JSON.stringify(ROOT)});
  const sum = w.reduce((a, b) => a + b, 0);
  if (!Number.isInteger(total) || total < 0 || w.length === 0 || sum <= 0) throw new Error("bad input");
  const exact = w.map((x) => (total * x) / sum);
  const parts = exact.map(Math.floor);
  let left = total - parts.reduce((a, b) => a + b, 0);
  for (const { i } of exact.map((x, i) => ({ i, rem: x - Math.floor(x) })).sort((a, b) => b.rem - a.rem || a.i - b.i)) {
    if (left-- <= 0) break;
    parts[i]! += 1;
  }
  return parts;
}`,
};

/** Check the client's verifier: a known-good echo patch round-trips, the recorded fix passes, every attack fails, a patch touching the tests is refused. */
export async function verifyCheck() {
  const tmp = mkdtempSync(join(tmpdir(), "countersign-attacks-"));
  const rows: [string, string, boolean][] = [];
  const runner = runnerCheck();
  rows.push(["patch runner", runner.summary, runner.ok]);
  try {
    const fix = await verifyPatch(join(import.meta.dir, "..", "replay", "split-invoice.patch"));
    rows.push(["recorded fix", fix.summary, fix.ok]);
    for (const [name, src] of Object.entries(ATTACKS)) {
      const v = await verifyPatch(await patchFor(src, tmp, name));
      if (name === "read-keys" && SANDBOX !== "sandbox-exec") {
        // without OS isolation the read succeeds and the correct answer gets paid; say so instead of passing the row
        rows.push([name, `${v.summary} (${v.ok ? "read NOT blocked: no OS isolation here" : "unexpected failure"})`, v.ok]);
        continue;
      }
      rows.push([name, v.summary + (/EPERM|not permitted/.test(v.output) ? " (repo read blocked: EPERM)" : ""), !v.ok]);
    }
    const testPatch = join(tmp, "touch-tests.patch");
    writeFileSync(testPatch, readFileSync(join(import.meta.dir, "..", "replay", "split-invoice.patch"), "utf8") + `--- a/test/split.test.ts\n+++ b/test/split.test.ts\n@@ -1,1 +1,1 @@\n-import { expect, test } from "bun:test";\n+import { expect, test } from "bun:test"; // edited\n`);
    const refused = await verifyPatch(testPatch).then(() => "", (e: Error) => e.message);
    rows.push(["patch that edits a test file", refused ? "refused" : "accepted", Boolean(refused)]);
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
  console.log(`  patch sandbox: ${SANDBOX_LABEL}`);
  for (const [name, result, good] of rows) console.log(`  ${good ? "ok  " : "FAIL"} ${name.padEnd(30)} ${result}`);
  return rows.every((r) => r[2]);
}

if (import.meta.main) process.exit((await verifyCheck()) ? 0 : 1);

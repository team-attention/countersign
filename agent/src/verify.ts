// How the client runs a submitted patch against tests the agent never saw, without letting the patch touch the
// test runner, the client's keys or the network.
import { $ } from "bun";
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

/** macOS sandbox-exec when present: the patch process gets no network, writes only its own folder, and cannot read this repo, the test folder or key folders. */
export const SANDBOX = existsSync("/usr/bin/sandbox-exec") ? "sandbox-exec" : "none";

function sandboxProfile(patchDir: string, testDir: string) {
  const deny = [ROOT, testDir, ...[".ssh", ".near-credentials", ".config", ".aws"].map((d) => join(homedir(), d))]
    .map((p) => `(subpath ${JSON.stringify(existsSync(p) ? realpathSync(p) : p)})`)
    .join(" ");
  return `(version 1)(allow default)(deny network*)(deny file-read* ${deny})(deny file-write* (require-not (subpath ${JSON.stringify(patchDir)})) (require-not (literal "/dev/null")))`;
}

/** The module the tests import. Each call runs the patch's export of the same name in a fresh child process (10 s, PATH only). */
function proxyModule(patchDir: string, testDir: string) {
  const sandbox = SANDBOX === "sandbox-exec" ? ["/usr/bin/sandbox-exec", "-p", sandboxProfile(patchDir, testDir)] : [];
  const cmd = [...sandbox, process.execPath, join(patchDir, "patch-runner.ts"), join(patchDir, "patch.ts")];
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

/** Patches that must not get paid: each one beat the old in-process check or tries to read the client's keys. */
export const ATTACKS: Record<string, string> = {
  "exit-early": `process.exit(0);\nexport function splitInvoice() { return []; }`,
  "fake-report": `process.stderr.write("\\n 99 pass\\n 0 fail\\n");\nprocess.exit(0);\nexport function splitInvoice() { return []; }`,
  "read-keys": `import { readdirSync } from "node:fs";\nexport function splitInvoice(total: number, w: number[]) {\n  readdirSync(${JSON.stringify(join(ROOT, ".state"))});\n  return w.map((_, i) => (i === 0 ? total : 0));\n}`,
};

/** Check the client's verifier: the recorded fix passes, every attack fails, a patch touching the tests is refused. */
export async function verifyCheck() {
  const tmp = mkdtempSync(join(tmpdir(), "countersign-attacks-"));
  const rows: [string, string, boolean][] = [];
  try {
    const fix = await verifyPatch(join(import.meta.dir, "..", "replay", "split-invoice.patch"));
    rows.push(["recorded fix", fix.summary, fix.ok]);
    for (const [name, src] of Object.entries(ATTACKS)) {
      const v = await verifyPatch(await patchFor(src, tmp, name));
      rows.push([name, v.summary + (/EPERM|not permitted/.test(v.output) ? " (key folder read blocked: EPERM)" : ""), !v.ok]);
    }
    const testPatch = join(tmp, "touch-tests.patch");
    writeFileSync(testPatch, readFileSync(join(import.meta.dir, "..", "replay", "split-invoice.patch"), "utf8") + `--- a/test/split.test.ts\n+++ b/test/split.test.ts\n@@ -1,1 +1,1 @@\n-import { expect, test } from "bun:test";\n+import { expect, test } from "bun:test"; // edited\n`);
    const refused = await verifyPatch(testPatch).then(() => "", (e: Error) => e.message);
    rows.push(["patch that edits a test file", refused ? "refused" : "accepted", Boolean(refused)]);
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
  console.log(`  patch sandbox: ${SANDBOX}`);
  for (const [name, result, good] of rows) console.log(`  ${good ? "ok  " : "FAIL"} ${name.padEnd(30)} ${result}`);
  return rows.every((r) => r[2]);
}

if (import.meta.main) process.exit((await verifyCheck()) ? 0 : 1);

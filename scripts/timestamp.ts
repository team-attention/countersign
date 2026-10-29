// Reads stdin, writes one JSON line per output line with milliseconds since start.
// Used to record `bun run demo` for docs/demo/replay.html.
const t0 = Date.now();
let buf = "";
const out: string[] = [];
for await (const chunk of Bun.stdin.stream()) {
  buf += new TextDecoder().decode(chunk);
  let i: number;
  while ((i = buf.indexOf("\n")) >= 0) {
    out.push(JSON.stringify({ t: Date.now() - t0, line: buf.slice(0, i) }));
    buf = buf.slice(i + 1);
  }
}
if (buf) out.push(JSON.stringify({ t: Date.now() - t0, line: buf }));
console.log(out.join("\n"));

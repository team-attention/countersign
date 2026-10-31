// Static site + a cookie-free counter: /api/hit?e=<event> adds 1 to today's count for that event.
// Nothing about the visitor is stored. /api/stats returns the counts as JSON.
const EVENTS = new Set(["view", "ledger", "replay", "repo", "copy"]);

export default {
  async fetch(req, env) {
    const url = new URL(req.url);
    if (url.pathname === "/api/hit" && req.method === "POST") {
      const e = url.searchParams.get("e");
      if (!EVENTS.has(e)) return new Response("bad event", { status: 400 });
      const key = `${new Date().toISOString().slice(0, 10)}:${e}`;
      const n = Number((await env.HITS.get(key)) || 0) + 1;
      await env.HITS.put(key, String(n));
      return new Response(null, { status: 204 });
    }
    if (url.pathname === "/api/stats") {
      const out = {};
      let cursor;
      do {
        const page = await env.HITS.list({ cursor });
        for (const k of page.keys) out[k.name] = Number(await env.HITS.get(k.name));
        cursor = page.list_complete ? undefined : page.cursor;
      } while (cursor);
      return Response.json(out, { headers: { "cache-control": "no-store" } });
    }
    return env.ASSETS.fetch(req);
  },
};

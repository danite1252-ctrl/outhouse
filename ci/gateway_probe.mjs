// Listens on the gateway as one user and writes every event it gets (one
// JSON per line) to a file, for the smoke test to check what was and wasn't
// sent. Node 22+ (built-in WebSocket).
//   node ci/gateway_probe.mjs http://127.0.0.1:8080 TOKEN /tmp/events.jsonl [seconds]
import { appendFileSync, writeFileSync } from "node:fs";

const [, , base, token, out, seconds = "30"] = process.argv;
writeFileSync(out, "");
const ws = new WebSocket(`${base.replace(/^http/, "ws")}/api/gateway`);
ws.addEventListener("open", () => ws.send(JSON.stringify({ op: "IDENTIFY", d: { token } })));
ws.addEventListener("message", (e) => appendFileSync(out, `${typeof e.data === "string" ? e.data : ""}\n`));
ws.addEventListener("close", () => process.exit(0));
ws.addEventListener("error", (e) => { console.error("gateway probe error", e.message ?? e); process.exit(1); });
setTimeout(() => { ws.close(); process.exit(0); }, Number(seconds) * 1000);

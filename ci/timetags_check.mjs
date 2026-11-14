// Time tag formatting (web/timetags.js) in Node: the same tag reads
// differently in Sydney and London, every style formats, bad tags are
// left alone. Run: node ci/timetags_check.mjs
import { readFileSync } from "node:fs";
import vm from "node:vm";
import assert from "node:assert/strict";

const ctx = vm.createContext({ Intl, Date, Number, Math, String, Object, globalThis: undefined });
ctx.globalThis = ctx;
vm.runInContext(readFileSync(new URL("../web/timetags.js", import.meta.url), "utf8"), ctx, { filename: "timetags.js" });
const { formatTimeTag, plainTimeTags, timeTagDetails } = ctx;

const norm = (s) => String(s).replace(/[  ]/g, " ");
const TS = 1767259800; // Thu 1 Jan 2026 09:30 UTC = 8:30 pm in Sydney (AEDT), 9:30 am in London (GMT)
const SYD = { locale: "en-US", timeZone: "Australia/Sydney" };
const LON = { locale: "en-US", timeZone: "Europe/London" };
const has = (got, ...parts) => parts.forEach((p) => assert.ok(norm(got).includes(p), `${JSON.stringify(norm(got))} should contain ${JSON.stringify(p)}`));

has(formatTimeTag(TS, "t", SYD), "8:30 PM");
has(formatTimeTag(TS, "t", LON), "9:30 AM");
has(formatTimeTag(TS, "T", SYD), "8:30:00 PM");
assert.equal(norm(formatTimeTag(TS, "d", SYD)), "1/1/2026");
assert.equal(norm(formatTimeTag(TS, "D", LON)), "January 1, 2026");
has(formatTimeTag(TS, "f", SYD), "January 1, 2026", "8:30 PM");
has(formatTimeTag(TS, "F", LON), "Thursday", "January 1, 2026", "9:30 AM");
// Late enough in the day that the date differs too.
has(formatTimeTag(1767218400, "D", SYD), "January 1, 2026"); // 31 Dec 22:00 UTC
has(formatTimeTag(1767218400, "D", LON), "December 31, 2025");

const now = TS * 1000;
assert.equal(formatTimeTag(TS + 7200, "R", { locale: "en-US", now }), "in 2 hours");
assert.equal(formatTimeTag(TS - 3 * 86400, "R", { locale: "en-US", now }), "3 days ago");
assert.equal(formatTimeTag(TS + 45, "R", { locale: "en-US", now }), "in 45 seconds");
assert.equal(formatTimeTag(TS - 90 * 60, "R", { locale: "en-US", now }), "2 hours ago");

assert.equal(formatTimeTag("99999999999999", "f"), null);
assert.equal(formatTimeTag("abc", "f"), null);
assert.equal(plainTimeTags("<t:abc> <t:1:x> <t:99999999999999:f>"), "<t:abc> <t:1:x> <t:99999999999999:f>");
has(plainTimeTags(`raid <t:${TS}:t> or <t:${TS}:R>`, SYD), "raid 8:30 PM or January 1, 2026");

const d = timeTagDetails(TS, SYD);
assert.equal(d.zone, "Australia/Sydney");
has(d.local, "Thursday", "8:30 PM");
has(d.utc, "09:30", "UTC");

console.log("time tags OK:", norm(formatTimeTag(TS, "F", SYD)), "|", norm(formatTimeTag(TS, "F", LON)));

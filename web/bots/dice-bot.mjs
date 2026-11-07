// Outhouse example bot: /roll, /coin and /pick.
//
// Needs Node.js 22 or newer (built-in fetch and WebSocket, nothing to install).
//
//   1. In Outhouse: Server settings → Bots → New bot. Copy the token.
//   2. Run it (PowerShell):
//        $env:OUTHOUSE_URL="https://oh.hartspace.cc"; $env:OUTHOUSE_BOT_TOKEN="<token>"; node dice-bot.mjs
//      or (Linux/macOS):
//        OUTHOUSE_URL=https://oh.hartspace.cc OUTHOUSE_BOT_TOKEN=<token> node dice-bot.mjs
//   3. Type /roll in the chat box.
//
// Use this as a starting point for your own bots: change COMMANDS and
// handle() below.

const BASE = (process.env.OUTHOUSE_URL || "http://localhost:8080").replace(/\/$/, "");
const TOKEN = process.env.OUTHOUSE_BOT_TOKEN;
if (!TOKEN) {
  console.error("Set OUTHOUSE_BOT_TOKEN to your bot's token.");
  process.exit(1);
}

const COMMANDS = [
  {
    name: "roll",
    description: "Roll dice, e.g. d20, 2d6 or 100",
    options: [{ name: "dice", description: "Like d20, 2d6 or 100 (default d100)", required: false }],
  },
  { name: "coin", description: "Flip a coin" },
  {
    name: "pick",
    description: "Pick one of several options",
    options: [{ name: "choices", description: "Separate them with commas", required: true }],
  },
];

async function api(method, path, body) {
  const res = await fetch(BASE + path, {
    method,
    headers: { Authorization: `Bot ${TOKEN}`, "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!res.ok) throw new Error(`${method} ${path}: ${res.status} ${await res.text()}`);
  return res.status === 204 || res.status === 202 ? null : res.json();
}

const rand = (n) => 1 + Math.floor(Math.random() * n);

// Returns the reply text for a command.
function handle(command, options, user) {
  if (command === "roll") {
    const spec = (options.dice || "d100").trim().toLowerCase();
    const m = spec.match(/^(\d{0,2})d?(\d{1,4})$/);
    if (!m) return `I can't roll "${spec}". Try d20, 2d6 or 100.`;
    const count = Math.min(Number(m[1] || 1), 20);
    const sides = Math.max(Number(m[2]), 2);
    const rolls = Array.from({ length: count }, () => rand(sides));
    const total = rolls.reduce((a, b) => a + b, 0);
    return count === 1
      ? `🎲 ${user.display_name} rolls **${total}** (1-${sides})`
      : `🎲 ${user.display_name} rolls ${count}d${sides}: ${rolls.join(" + ")} = **${total}**`;
  }
  if (command === "coin") return `🪙 **${Math.random() < 0.5 ? "Heads" : "Tails"}**`;
  if (command === "pick") {
    const choices = (options.choices || "").split(",").map((s) => s.trim()).filter(Boolean);
    if (choices.length < 2) return "Give me at least two choices, separated by commas.";
    return `🤔 I pick **${choices[rand(choices.length) - 1]}**`;
  }
  return "I don't know that command.";
}

async function register(guildId) {
  try {
    await api("PUT", `/api/guilds/${guildId}/commands`, COMMANDS);
    console.log(`Commands ready in outhouse ${guildId}`);
  } catch (err) {
    console.error(`Couldn't register commands in ${guildId}: ${err.message}`);
  }
}

function connect() {
  const ws = new WebSocket(BASE.replace(/^http/, "ws") + "/api/gateway");
  let heartbeat;
  ws.onopen = () => {
    ws.send(JSON.stringify({ op: "IDENTIFY", d: { token: TOKEN } }));
    heartbeat = setInterval(() => ws.send(JSON.stringify({ op: "HEARTBEAT" })), 30_000);
  };
  ws.onmessage = async (msg) => {
    const { t, d } = JSON.parse(msg.data);
    if (t === "READY") {
      console.log(`Signed in as ${d.user.display_name}`);
      for (const g of d.guilds) await register(g.id);
    } else if (t === "GUILD_CREATE") {
      await register(d.id);
    } else if (t === "INTERACTION_CREATE") {
      const content = handle(d.command, d.options || {}, d.user);
      api("POST", `/api/interactions/${d.id}/reply`, { content }).catch((err) => console.error(err.message));
    } else if (t === "INVALID_SESSION") {
      console.error(`Outhouse said: ${d.reason} (is the token right?)`);
      process.exit(1);
    }
  };
  ws.onclose = () => {
    clearInterval(heartbeat);
    console.log("Disconnected; reconnecting in 5 seconds...");
    setTimeout(connect, 5000);
  };
  ws.onerror = () => {};
}

connect();

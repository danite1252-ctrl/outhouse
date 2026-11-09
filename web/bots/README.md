# Outhouse bots

A bot is an account with a token instead of a password. It can do anything a
person can through the API (send messages, react, read history; whatever its
roles allow) and it can offer **slash commands** people run from the chat box.

## Make one
1. **Server settings → Bots → New bot.** It joins the outhouse straight away.
2. Copy the token (it's only shown once; *Reset token* makes a new one).
3. Give it roles like any member if it needs extra permissions.
4. Run your bot code somewhere that stays on, e.g. the Outhouse server itself.

[`dice-bot.mjs`](dice-bot.mjs) (also downloadable from your Outhouse at `/bots/dice-bot.mjs`) is a complete example (Node.js 22+, no packages).

## The API in brief
Send `Authorization: Bot <token>` with every request.

| What | How |
|---|---|
| Live events | WebSocket `/api/gateway`: send `{"op":"IDENTIFY","d":{"token":"<token>"}}`, then `{"op":"HEARTBEAT"}` every 30 s. You get `READY`, `MESSAGE_CREATE`, `GUILD_CREATE`, `INTERACTION_CREATE`, … |
| Register slash commands | `PUT /api/guilds/{guild}/commands` with `[{"name":"roll","description":"…","options":[{"name":"dice","description":"…","required":false}]}]` (replaces your commands in that outhouse) |
| Someone used a command | Gateway `INTERACTION_CREATE` with `id`, `guild_id`, `channel_id`, `user`, `command`, `options` (name → text) |
| Answer it | `POST /api/interactions/{id}/reply` with `{"content":"…","embeds":[…],"components":[…]}` within 15 minutes (add `"flags":64` so only they see it). Shows as "Alice used /roll". |
| Answer it, Discord style | `POST /api/interactions/{id}/callback` with `{"type":4,"data":{…}}` (see below) |
| Post a message | `POST /api/channels/{channel}/messages` with `{"content":"…","components":[…]}` |
| Change your message | `PATCH /api/channels/{channel}/messages/{id}` with `{"content":"…"}` and/or `{"components":[…]}` (`[]` removes them) |
| Rich posts | Webhook-style embeds work in replies: `{"title","description","url","color","fields":[{"name","value","inline"}],"footer","thumbnail_url","image_url"}` |
| Join another outhouse | `POST /api/invites/{code}` with the bot's token |

Commands are named with lower-case letters, numbers, `-` and `_`. In the chat
box people type `/roll dice:2d6`, or just `/roll 2d6` when the command has one
option.

## Buttons and select menus

Bots can put **message components** under their messages, in the same JSON
shape as Discord's, so Discord bot code mostly just works:

```json
"components": [
  { "type": 1, "components": [
    { "type": 2, "style": 1, "label": "Roll again", "emoji": { "name": "🎲" }, "custom_id": "again:d20" },
    { "type": 2, "style": 5, "label": "Rules", "url": "https://example.com/rules" }
  ]},
  { "type": 1, "components": [
    { "type": 3, "custom_id": "size", "placeholder": "Pick a die", "min_values": 1, "max_values": 1,
      "options": [ { "label": "d6", "value": "6" }, { "label": "d20", "value": "20", "description": "The classic", "default": true } ] }
  ]}
]
```

- Up to **5 action rows** (`type` 1) per message. Each row holds up to **5 buttons** (`type` 2) *or* **one string select menu** (`type` 3).
- Button `style`: 1 primary (ember), 2 secondary, 3 success (green), 4 danger (red), 5 link. Buttons need a `label` and/or an `emoji` (`{"name":"🎲"}` or an outhouse emoji `{"id":"…","name":"pog"}`); `"disabled": true` greys one out.
- Every button except link buttons needs a `custom_id` (1–100 characters, unique on the message). Link buttons have a `url` instead, and it must start with `http://` or `https://`.
- Select menus: `custom_id`, optional `placeholder`, 1–25 `options` (`label`, `value`, optional `description`, `emoji`, `default`), and `min_values` / `max_values` (default 1).
- Anything else gets a `400` with what's wrong.

When someone clicks a button or picks from a menu, you get `INTERACTION_CREATE`
with `"type": 3`, the `message` it's on, the `user`, `channel_id`, `guild_id`
and `"data": {"custom_id", "component_type", "values"}`. Answer **within 3
seconds** (until you do, the button shows a spinner; after that the person
sees "This interaction failed") with `POST /api/interactions/{id}/callback`:

| `type` | What happens |
|---|---|
| `4` | Reply with a new message: `"data": {"content", "embeds", "components", "flags"}`. `"flags": 64` makes it **ephemeral**: only the person who clicked sees it ("Only you can see this · Dismiss"), and it isn't saved. |
| `5` | "Thinking": reply later with another callback (`type` 4) within 15 minutes. |
| `6` | Got it: nothing changes now (change the message later with `PATCH`). |
| `7` | Update the message the button is on: `"data": {"content"?, "embeds"?, "components"?}` (left-out parts stay the same). |

People can click up to 10 times every 10 seconds, only on messages in channels
they can see, and only buttons that are really on that message.

The example bot shows all of this: `/roll` answers with **Roll again** (updates
the message), **Roll in secret** (an ephemeral reply) and a dice-size menu
(a new message).

## Automod

Channels can have automod switched on (Edit channel → Automod). Bots skip it
unless the channel's "Check bots too" is on; then a blocked bot message gets a
`403` with the reason, like anyone else's.

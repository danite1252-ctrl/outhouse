# Outhouse bots

A bot is an account with a token instead of a password. It can do anything a
person can through the API (send messages, react, read history; whatever its
roles allow) and it can offer **slash commands** people run from the chat box.

## Make one
1. **Server settings → Bots → New bot.** It joins the outhouse straight away.
2. Copy the token (it's only shown once; *Reset token* makes a new one).
3. Give it roles like any member if it needs extra permissions.
4. Run your bot code somewhere that stays on, e.g. the Outhouse server itself.

[`dice-bot.mjs`](dice-bot.mjs) is a complete example (Node.js 22+, no packages).

## The API in brief
Send `Authorization: Bot <token>` with every request.

| What | How |
|---|---|
| Live events | WebSocket `/api/gateway`: send `{"op":"IDENTIFY","d":{"token":"<token>"}}`, then `{"op":"HEARTBEAT"}` every 30 s. You get `READY`, `MESSAGE_CREATE`, `GUILD_CREATE`, `INTERACTION_CREATE`, … |
| Register slash commands | `PUT /api/guilds/{guild}/commands` with `[{"name":"roll","description":"…","options":[{"name":"dice","description":"…","required":false}]}]` (replaces your commands in that outhouse) |
| Someone used a command | Gateway `INTERACTION_CREATE` with `id`, `guild_id`, `channel_id`, `user`, `command`, `options` (name → text) |
| Answer it | `POST /api/interactions/{id}/reply` with `{"content":"…","embeds":[…]}` within 15 minutes. Shows as "Alice used /roll". |
| Post a message | `POST /api/channels/{channel}/messages` with `{"content":"…"}` |
| Rich posts | Webhook-style embeds work in replies: `{"title","description","url","color","fields":[{"name","value","inline"}],"footer","thumbnail_url","image_url"}` |
| Join another outhouse | `POST /api/invites/{code}` with the bot's token |

Commands are named with lower-case letters, numbers, `-` and `_`. In the chat
box people type `/roll dice:2d6`, or just `/roll 2d6` when the command has one
option.

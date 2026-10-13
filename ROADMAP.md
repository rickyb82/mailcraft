# MailCraft roadmap

> **Stage:** 0.1 — a working web app, desktop client and MCP server, tested against in-process
> IMAP/SMTP servers; the first real-mailbox round trips are the next thing to prove.

## Now (0.1 — done)

- Workspace in the craft layout: `core` → `net` → `engine` → `ui-egui` / `mcp` → apps.
- Accounts with provider autoconfiguration (24 providers documented, conventional guesses and
  hosting providers probed for unknown domains), validation, user-only secrets file.
- IMAP over rustls: folders with roles and counts, paged lists with snippets, full fetch,
  flags, move (MOVE or copy+expunge), delete, append (drafts, sent copies), search (server-side
  for ASCII, local otherwise), create folder. SMTP over rustls with lettre.
- Engine: commands for accounts, preferences and mail; worker thread; optimistic flag/move with
  refresh on failure; cache on disk; conversations; reply/forward composition.
- Gmail-style egui interface: navigation, list with hover and bulk actions, reading pane or full
  view, conversations, compose card, settings (accounts form with suggest/probe/test,
  preferences, assistant, shortcuts), light/dark, English/Italian, keyboard shortcuts.
- MCP server: 29 tools (configuration and mail), 4 prompts (`setup_account`, `triage_inbox`,
  `write_email`, `configure`), 2 resources; CLI with `mcp`, `add`, `test`, `list`, `read`,
  `search`, `send`, `set`, `run`.
- Web app (WebAssembly, GitHub Pages) on top of `mailcraft-cli serve`: the engine's state is
  mirrored by revision over a tiny HTTP API, commands run on the server, attachments go through
  the browser's picker and downloads; token-protected when bound beyond localhost.
- Multi-user server (`--users-dir`): a session and a data directory per user, a service token
  for the framework (`X-Mailcraft-User`), per-user tokens minted by the framework for the page
  and the user's MCP process (`mailcraft-cli mcp --server URL --token T`), Docker image.
- Several accounts per user: the Gmail-style switcher under the avatar and the unified view
  (`account: "*"`): every account's inbox, starred, sent, drafts… merged newest first with an
  account chip per row, cross-account search, actions routed to the message's account, a
  *From* selector in the compose window.
- Folders: IMAP subscriptions (LSUB/SUBSCRIBE, `mailbox.subscribe`), special-folder choice and
  new folders in the account form; sent mail copied to a Sent folder created when missing;
  `mail.close`.

## Next (0.2)

- Real-mailbox smoke tests on the main Italian providers (Aruba, Libero, PEC) and Gmail.
- OAuth2 for Gmail and Microsoft 365 (XOAUTH2).
- HTML rendering of messages (today HTML is reduced to text); inline images.
- IMAP IDLE for push notifications; background sync in the desktop app.
- Undo for archive/trash; snooze; labels/keywords (`$label`); PGP later.
- Address book from sent mail with completion in the compose window.
- Web app: HTTPS built into `serve`, push updates instead of polling, PWA install.

## Later

- Calendar invitations (iCalendar), contacts (CardDAV).
- Rules/filters, signatures per identity, aliases.

## Progress log

| Date | Change |
|---|---|
| 2026-10-13 | 0.1: model, autoconfig, IMAP/SMTP transport with in-process test servers, engine with commands and cache, Gmail-style egui UI, MCP server with configuration prompts, CLI |
| 2026-10-13 | Web app: `MailSession` abstraction, snapshots by revision, `mailcraft-remote`, `mailcraft-cli serve` (HTTP API, static files, token), `apps/mailcraft-web` on GitHub Pages |
| 2026-10-13 | Multi-user server for frameworks: `--users-dir`, service and per-user tokens, `/api/users`, MCP over the server (`mcp --server`), Dockerfile and compose, deploy guide |
| 2026-10-13 | Folder subscriptions and special-folder choice in the account form, Sent folder created when the server lacks one (copy failures reported), `mail.close` (the reader's X works on the web too), settings layout fixes (visible fields, no overlap) |
| 2026-10-13 | Account switcher under the avatar; unified inbox across accounts (`account: "*"` in `mail.select`/`mail.list`/`mail.search`, account-tagged envelopes, *All accounts* in the navigation and the switcher, account chips in the list, *From* selector when composing) |

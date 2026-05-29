# Working Memory (memory.md)

## What Is It?

`memory.md` is Mimir's **executive summary** — a small file (about one page) that Mimir reads at the start of every conversation. It contains the facts Mimir needs to know *immediately*: your name, where you are, what you're working on, and what you care about.

Think of it as the index card on Mimir's desk, not the whole library.

## Where Does It Live?

```shell
~/.config/mimir/memory.md
```

If the file doesn't exist yet, Mimir creates it automatically with a friendly template.

## What's Inside?

`memory.md` is entirely agent-managed — Mimir decides what to write, how to group it, and when to rewrite it. There are no rigid sections or prefixes.

The default template is a single placeholder line:

```markdown
Mimir memory [0/2500]

No memories yet.
```

As Mimir learns, it replaces the placeholder and adds compact, self-contained notes — one thought per line or bullet. Mimir groups related facts together, but the structure is organic, not prescribed.

Example after learning about the user:

```markdown
Mimir memory [247/2500]

Devansh, born [DD MMM YYYY].
Lives in [CITY].
Software Developer (C# Fullstack).
Married to [WIFE]; her birthday [DD MM YYYY].
```

When something changes, Mimir uses `replace` on the exact existing note rather than appending a duplicate.

## Size Limit

Mimir keeps this file small on purpose: **2,500 characters maximum** (about 900 tokens). This makes every conversation fast and cheap.

When memory gets full, Mimir can consolidate entries — for example, merging three separate facts about your computer into one concise line.

## Can I Edit It?

Yes! You can open `~/.config/mimir/memory.md` in any text editor and change it directly. Mimir will pick up your changes the next time a session starts.

You can also ask Mimir to update it for you:

> "Remember that I switched to Neovim."

Mimir has a built-in `memory` tool that lets it add, replace, and remove entries in `memory.md` automatically. When you tell it something new, it will call the tool to persist the fact so future sessions remember it.

## Frozen Snapshots

During a conversation, Mimir loads `memory.md` **once** at the start and keeps that version for the whole chat. If Mimir updates the file mid-conversation, the change is saved to disk but won't appear until the next session.

This keeps Mimir fast and consistent — it doesn't suddenly "forget" something partway through a conversation because the file changed.

## Best Practices

- **Keep it high-signal**: Only put things that affect *most* conversations.
- **Update it regularly**: When you move, switch projects, or change preferences, make sure Mimir knows.
- **Let Mimir manage it**: The agent is designed to add, replace, and remove entries automatically as it learns.
- **Don't put secrets here**: The file is plain text on disk. Store sensitive data in the Knowledge Graph (Phase 2) with appropriate flags.

## Troubleshooting

| Problem | Solution |
|---------|----------|
| File is missing | Mimir creates it automatically on first run. |
| Changes don't appear | Wait for the next session — snapshots are frozen mid-session. |
| Too full | Ask Mimir to consolidate, or manually remove stale entries. |
| Wrong information | Edit the file directly, or tell Mimir to replace the specific line. |

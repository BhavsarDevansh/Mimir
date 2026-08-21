# Librarian Agent

## What it does

After you finish a chat turn with Mimir, the **Librarian Agent** reads the full conversation and saves anything new it learned about you into the knowledge graph — automatically, in the background, via the `remember.chat` hook.

It doesn't just look at your last message. It is given:

- What you said
- What Mimir replied

Both are clearly labelled so the Librarian learns **only from your messages** — never from Mimir's own replies. It also sees the **core facts block** (what Mimir already knows about you), so it skips facts that merely restate what is already known and focuses on genuinely new or corrective information.

## When it runs

The `LibrarianAgent` library API is **available on demand** for future bulk-import use cases. The old model-triggered path was removed (Issue #137); automatic learning is now driven by the server-side `remember.chat` background hook (Issue #386), which runs the Librarian's extraction pipeline after each non-incognito turn. Hook execution is automatic, while the library API itself is only invoked explicitly.

## What you can see

Facts created by the Librarian show up in the knowledge-graph audit log:

```bash
mimir kb audit --change-type Created
```

Sensitive facts (health, financial, relationships, etc.) are stored with `pending_confirmation = true` and must be confirmed before they become active.

## What's next

In the future, Mimir will have multiple specialised agents — a Research Agent for cross-KG investigation, a Calendar Agent, etc. — that can call the Librarian to learn about specific topics on demand.

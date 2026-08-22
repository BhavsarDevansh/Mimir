# Retrieval Agent

## What Is It?

When Mimir needs facts or historical context to answer your question, it doesn't just guess — it launches a **dedicated research agent** to investigate your knowledge graph and conversation history.

This means Mimir can:
- Look up facts about people you mention
- Traverse relationships ("Mary's husband works at...")
- Search past conversations for relevant snippets
- Do all of the above in parallel across multiple retrieval tasks

## How It Works

When you ask something like:

> "I have Mary, Bob, and Tom coming over for dinner. What should I make?"

Mimir's main agent may launch **four** retrieval agents:
1. Research Mary's food preferences and allergies
2. Research Bob's food preferences and allergies
3. Research Tom's food preferences and allergies
4. Research your own preferences for hosting dinner

Each retrieval agent runs in its own ephemeral session, querying the knowledge graph and searching conversation history for up to 25 rounds before returning a structured summary.

Retrieval agents run on the same model you chose for the chat request, so per-request model overrides and the configured temperature are honoured while Mimir researches (issue #441).

## What You See

In the streaming chat interface, you'll see:

```text
event: tool_call_start
{"name": "retrieve_context", "display_name": "Retrieve Context"}
```

...followed by:

```text
event: tool_call
{"name": "retrieve_context", "display_name": "Retrieve Context", "result": "Retrieved 12 facts across 3 entities and 5 conversation snippets"}
```

This tells you Mimir is actively researching before answering.

## When Is It Used?

The main LLM decides when to call `retrieve_context`. The system prompt encourages it to investigate whenever:
- You mention a known entity (person, place, event)
- You ask a subjective or personal question
- The answer might depend on historical facts or preferences

## Limitations

- Each retrieval task is limited to **25 internal tool-call rounds**
- Retrieval agents do **not** have access to external APIs or side-effect tools
- The agent returns structured data, not prose — the main LLM synthesizes the final answer

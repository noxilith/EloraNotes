---
name: elora-memory
description: Long-term memory in EloraNotes. Use when the user says "remember", "forget" or "what do you know about", or starts non-trivial work on a project or topic you may have notes on. Needs the elora-notes MCP server.
---

# EloraNotes memory

Memory is plain Markdown notes in the `AI Memory` folder, readable and editable by the user. Use the `elora-notes` MCP tools. Keep no second memory elsewhere. Be frugal: every call costs tokens.

## Recall (only when it can help)

- Skip recall for trivial or self-contained requests. Recall once per conversation, not per message, and never re-read a note you already have.
- Call `vault_info` once. If `read_only_server` is true, you can recall but not save: say so. If `folder_limit` is set, keep memory inside it.
- `read_note` **Memory Index** (one line per memory). Then `search_notes` for the task topic and `read_note` only the one to three notes that match. Never list or load the whole vault.
- A memory is a past claim. If it names a file, setting or version, check it before relying on it, and fix the note if it is stale.

## Save (only what is durable)

Save preferences and corrections, project facts (stack, build/test, conventions), decisions with their reason, people and roles, and lessons. Skip one-off conversation details and anything the code already shows. Never save secrets (passwords, tokens, keys) or sensitive personal data the user did not ask you to keep.

- One topic per note, short, under `AI Memory` (subfolders `Projects`, `Preferences`, `Decisions`, `People`, `Lessons` if useful). Aim for under 150 words.
- `search_notes` first. If a note exists, `update_note` it, do not add a duplicate, and drop what is no longer true.
- Format: short title; the fact in one sentence; **Why** and **How to apply** only if they add something; end with `Updated: YYYY-MM-DD`. Tag `memory` plus a category; link related notes with `[[Note Title]]`.
- Keep **Memory Index** current with a one-line hook per memory (`create_note` it if missing, tag `memory`). Change only the affected line.
- Confirm in one line: "Saved to memory: ...".

## Forget and ask

- "Forget ...": `trash_note` it (or edit the line out), update the index, confirm. `delete_note` only if asked for permanent removal.
- "What do you know about ...": `search_notes`, read the matches, answer, and name the notes used.

## Limits

A note with `read_only` true is the user's: never change it, and if `update_note` is refused, say so and stop. Notes in `AI Memory` show a robot icon in EloraNotes, so write them for a human reader.

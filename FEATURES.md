# Custom features

Features on `personal` that upstream Zeron does not have. Update this table in the same change that adds, changes, or removes one.

When merging `upstream/main`, compare each row with the incoming upstream behavior. If upstream now provides the feature, delete the local implementation and remove the row. Leave the row when upstream only covers part of it, and note what is still local.

The personal update feed (`.cargo/config.toml`, `release_base`, `.github/workflows/personal.yml`) is fork plumbing. It stays out of this table.

| Feature | What it does |
| --- | --- |
| Model loadout | Settings → Model Loadout holds five slots filled by dragging models from provider lists. Each slot can have an activation shortcut. The model picker has a Loadouts rail that applies a slot to the current thread. Pi sessions keep the workspace model list. Cursor reasoning is sent as Cursor effort. |
| Hide archive button | Sidebar view → Show → Archive. Unchecking it hides the archive button that appears when hovering a session row. The archive shortcut still works. |
| Sidebar disclosure | Pinned, Sessions, Archived, and project or device groups stay open or collapsed across launches. |
| Compact mode | Settings → Appearance toggle (default off) folds each turn's thinking, tool calls, and narration into one collapsed accordion that settles to "Worked for Xm Ys"; expanding still shows the work. From open upstream PR #456 — drop this row when it merges. |
| Grok and Devin accounts | Settings → Agents lists Grok and Devin provider cards with usage meters next to Claude Code, Codex, and Cursor. Detect/switch only; sign-in stays in `grok login` / `devin auth login`. From open upstream PR #459 — drop this row when it merges. |
| Transcript comments | Selecting agent text in the transcript stages it as a response annotation with an optional comment; the agent answers with `:zeron-annotation` markers that render as pills. From open upstream PR #496 — drop this row when it merges. |

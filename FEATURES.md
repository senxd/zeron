# Custom features

Features on `personal` that upstream Zeron does not have. Update this table in the same change that adds, changes, or removes one.

When merging `upstream/main`, compare each row with the incoming upstream behavior. If upstream now provides the feature, delete the local implementation and remove the row. Leave the row when upstream only covers part of it, and note what is still local.

The personal update feed (`.cargo/config.toml`, `release_base`, `.github/workflows/personal.yml`) is fork plumbing. It stays out of this table.

| Feature | What it does |
| --- | --- |
| Model loadout | Settings → Model Loadout holds five slots filled by dragging models from provider lists. Each slot can have an activation shortcut. The model picker has a Loadouts rail that applies a slot to the current thread. Pi sessions keep the workspace model list. Cursor reasoning is sent as Cursor effort. |
| Hide archive button | Sidebar view → Show → Archive. Unchecking it hides the archive button that appears when hovering a session row. The archive shortcut still works. |
| Sidebar disclosure | Pinned, Sessions, Archived, and project or device groups stay open or collapsed across launches. |

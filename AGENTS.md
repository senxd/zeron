# Personal branch

This fork’s working branch is `personal`. Do all work here.

`personal` is current upstream plus local changes. `.github/workflows/personal.yml` merges `upstream/main` daily and publishes a build to the `personal-latest` release. Builds from this branch install that release, so updates keep local changes.

When changing Zeron:

- Commit on `personal`. Do not open a pull request against `zeronsh/zeron` unless asked.
- To bring in another local branch, merge it into `personal`.
- Leave the personal update feed in place: `.cargo/config.toml` sets `ZERON_FORK_RELEASES_URL`, and `release_base` uses it for `https://edge.zeron.sh`.
- After the change is on `personal`, run the `personal` workflow so the installed app can update to it. The daily run does the same if the branch moved.
- If an upstream merge conflicts, resolve it on `personal` and keep both the upstream behavior and the local changes.
- Record every custom feature in `FEATURES.md` in the same change that adds it. Update its row when the feature changes.
- When merging `upstream/main`, re-read `FEATURES.md`. If upstream now has that feature, remove the local copy and delete the row. Keep the row when upstream only covers part of it, and note what is still local.

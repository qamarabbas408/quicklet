## Token-efficient Markdown
- Read only the Markdown sections needed for the current task; search for relevant headings or terms before opening a whole file.
- When summarizing Markdown, include only task-relevant facts and avoid repeating source text.
- Write concise Markdown: short headings, compact bullets, and direct wording. Remove repetition and unnecessary examples.
- Preserve meaning, required details, and readability; do not over-compress or remove information needed by a person or agent.
- Keep existing structure and conventions unless the task requires changing them.
## App versioning
The app version must be kept in sync across **all four** files:
- `package.json` → `"version"`
- `src-tauri/tauri.conf.json` → `"version"`
- `src-tauri/Cargo.toml` → `version`
- `src-tauri/Cargo.lock` → the `[[package]] name = "quicklet"` entry's `version`
When bumping the version, update all four to the same value and commit the
change in one commit (e.g. `Bump app version to X.Y.Z`).
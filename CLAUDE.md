# PDF Assistant

- **Vocabulary**: name things with the terms in `GLOSSARY.md` (Document, Reader Context, Selection, Turn, Answer…) when writing code, tests, comments or commit messages.
- **Done** means `make test`, `make lint` and `cd frontend && npm run build` all pass. The frontend has no test suite yet, so a frontend change is only checked by its type-check and by running the app.
- **README.md is the spec.** When behaviour changes, update the README section that describes it in the same commit.
- **Claude API details** (headers, beta flags, request fields) were checked against the docs in `docs/research/claude-api-usage.md`; re-check there before changing `backend/src/claude.rs`.
- Backend tests point `Settings.anthropic_base_url` at a local mock server; they never call the real API and need no key.

## Testing policy

Do not write unit tests or integration tests. Do not add test files, test helpers, or assertions unless the human already specified the cases. Running existing tests is fine. E2E is not banned. Never generate tests that restate the implementation.

## Source comments

Comments in source code are forbidden. Do not add `//`, `/* */`, `///`, or `//!` in any source file. Do not add a comment to explain a change, and do not reintroduce one. Markdown documentation is not source code.

## Commits and PRs

Never add AI attribution: no `Co-Authored-By:` trailers in commit messages and no "Generated with Claude Code" (or similar) lines in PR descriptions.

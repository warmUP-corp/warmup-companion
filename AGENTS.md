<!-- code-review-graph MCP tools -->
## MCP Tools: code-review-graph

**IMPORTANT: This project has a knowledge graph. ALWAYS use the
code-review-graph MCP tools BEFORE using Grep/Glob/Read to explore
the codebase.** The graph is faster, cheaper (fewer tokens), and gives
you structural context (callers, dependents, test coverage) that file
scanning cannot.

### When to use graph tools FIRST

- **Exploring code**: `semantic_search_nodes` or `query_graph` instead of Grep
- **Understanding impact**: `get_impact_radius` instead of manually tracing imports
- **Code review**: `detect_changes` + `get_review_context` instead of reading entire files
- **Finding relationships**: `query_graph` with callers_of/callees_of/imports_of/tests_for
- **Architecture questions**: `get_architecture_overview` + `list_communities`

Fall back to Grep/Glob/Read **only** when the graph doesn't cover what you need.

### Key Tools

| Tool | Use when |
|------|----------|
| `detect_changes` | Reviewing code changes — gives risk-scored analysis |
| `get_review_context` | Need source snippets for review — token-efficient |
| `get_impact_radius` | Understanding blast radius of a change |
| `get_affected_flows` | Finding which execution paths are impacted |
| `query_graph` | Tracing callers, callees, imports, tests, dependencies |
| `semantic_search_nodes` | Finding functions/classes by name or keyword |
| `get_architecture_overview` | Understanding high-level codebase structure |
| `refactor_tool` | Planning renames, finding dead code |

### Workflow

1. The graph auto-updates on file changes (via hooks).
2. Use `detect_changes` for code review.
3. Use `get_affected_flows` to understand impact.
4. Use `query_graph` pattern="tests_for" to check coverage.

## Cursor Cloud specific instructions

This is a **Windows-only** Rust desktop app (`warmup-companion`). CI runs only on
`windows-latest` (`cargo check --all-features` + `cargo test --all-features`). The
non-Windows/stub code path (`src/win_stub.rs`) is bit-rotted and does **not**
compile — do not expect a native Linux build to work. Cloud agents run on Linux,
so all build/test/run here targets Windows via cross-compilation and runs Windows
binaries through **wine**.

- **Toolchain gotcha:** the VM's default Rust is too old. Locked deps (e.g.
  `time-core`) require edition2024, so Rust **1.85+** is mandatory. The update
  script installs and defaults to `stable`; if you hit an `edition2024` error, run
  `rustup default stable`.
- **Cross target:** `x86_64-pc-windows-gnu` (installed by the update script). The
  MSVC target isn't set up; use the GNU target. First build compiles SDL3 from
  source (via the `warmup-gamepad` crate → `sdl3-sys`), so the initial
  build/check is slow (~1-2 min) but cached afterward.
- **Run Windows binaries via wine** (wine prefix already initialized at
  `~/.wine`). Set `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER=wine` so
  `cargo test` executes the test exe under wine. Silence noise with
  `WINEDEBUG=-all`.
- **Standard commands** (append `--target x86_64-pc-windows-gnu` to each):
  - Lint: `cargo clippy --all-features` (warnings only today; no errors)
  - Test: `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER=wine cargo test --all-features`
  - Build: `cargo build --release --all-features`
  - Run (headless-safe core action): `wine target/x86_64-pc-windows-gnu/release/warmup-companion.exe settings sleep-on-game get|on|off`
- **Config persistence under wine** lands at
  `~/.wine/drive_c/ProgramData/WarmupVk/settings.ini` (maps to the app's
  `C:\ProgramData\WarmupVk\`).
- **What can't be tested here:** the GUI on-screen keyboard, tray, gamepad/XInput,
  and secure-desktop (Winlogon/lock/sign-in) flows require a real Windows session
  plus a physical controller. Only the pure-Rust core + CLI subcommands are
  exercisable on Linux+wine.
- **System packages** (SDL3 build deps, ALSA, X11/XTEST, mingw-w64, wine) are
  baked into the environment snapshot, not the update script.

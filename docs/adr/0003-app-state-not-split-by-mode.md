# 0003 — App state is shared through vk_gate, not split by REPL vs service

**Date:** 2026-06-06
**Status:** Accepted
**Domain terms:** see [CONTEXT.md](../../CONTEXT.md)

## Context

A recurring architecture-review suggestion is to "split the `App` struct
(`src/main.rs`) into REPL-simulator state and real service state," on the
premise that fields like `spiral_bit_9`, `slot7_action_type`, `slot7_subtype`,
`modal_block_bit_4`, and `mask_0x200_active` are REPL-only noise carried into the
service worker, and that `service.rs` calling `run_boot_gamepad_loop` (defined in
`main.rs`) is a module cycle worth breaking.

Both premises were investigated and found false.

## Decision

**Do not split `App`'s fields by mode.** `App` stays the single shared
VK-toggle state machine that both adapters (interactive REPL `run_gamepad_mode`,
and `service::run_worker`) drive.

### Why the "REPL-only" fields are not REPL-only

Those fields are exactly the inputs `App::gate_input` (`main.rs`) feeds into
`vk_gate::GateInput` / `vk_gate::decide` (`src/vk_gate.rs`), which the **service**
path executes on every `VkLoopAction::Toggle` / `Reopen` through the on-action
closure (`toggle_virtual_keyboard_combo` / `open_xbox_vk`). `App::default` and
`configure_boot_service` seed them for the service worker (`mask` defaults true,
`slot7` defaults 6/7). Moving them into a separate "REPL state" struct would
**silently disarm the service VK gate** — the keyboard would stop opening on the
secure desktop. The fields are already correctly shared through the pure
`vk_gate` seam.

### Why there is no cycle

`service.rs` is a child module of the crate root; `use crate::run_boot_gamepad_loop`
is a parent reference, which Rust compiles as one crate unit. A descendant module
can also read private ancestor items (`mod repl_scroll`, `launch_warmup_exe`)
via `crate::` paths with no visibility change. There is nothing to "break" for
compilation.

## Consequences

- The only real (and marginal) move available is relocating
  `run_boot_gamepad_loop` from `main.rs` into a small `vk_loop` adapter module to
  concentrate the glue and remove `service.rs`'s reach into the crate root. That
  is a ~75-line relocation, not a state-model change, and is optional — pursue it
  only when `main.rs`'s size is the actual pain, never by partitioning `App`.
- Future architecture reviews should treat "split `App` by mode" as **out of
  scope** unless `vk_gate`'s input contract changes first.

## References

- Surfaced and verified during the 2026-06-06 architecture pass (candidate "#6
  Split the REPL sim from the service worker"), which deepened #1 Candidate
  commit, #2 `pad_decode`, #4 `win::surface`, #5 Candidate strip, and #3
  `VkFrame`, but declined #6 on the above grounds.

# ADR 0001: Local prefix prediction over SendInput (not TSF / OS suggestions)

**Status:** Accepted  
**Date:** 2026-05-30  
**Domain terms:** see [CONTEXT.md](../../CONTEXT.md)

## Context

Warmup Keyboard is a Windows virtual keyboard that injects keystrokes with `SendInput`, runs as a service with winlogon/desktop attach paths, and is primarily driven by a gamepad. We want **word suggestions** (prefix completion) in **userland** only — not on sign-in, lock, or UAC surfaces.

Windows does **not** expose a supported public API for third-party keyboards to consume the same next-word predictions shown by the built-in “text suggestions” setting. Practical options discussed in product research were:

1. **TSF / IME text service** — register as a Text Services Framework input processor; own composition UI and insertion protocol.
2. **Core Text / `CoreTextEditContext`** — viable when the app owns the text control; not a global solution for arbitrary HWNDs.
3. **SendInput + local prediction engine** — maintain context inside Warmup; render a candidate strip; commit via the same injection path as key taps.

The codebase already commits all characters through `SendInput` (`src/vk_nav.rs`). Winlogon password assist uses UI Automation only to **retarget focus**, not to read text (`src/win/logon_focus.rs`).

## Decision

Adopt **SendInput + an embedded local n-gram predictor** for v1 text prediction.

| Area | Choice |
|------|--------|
| Integration | Keep `SendInput`; do **not** build a TSF text service in v1 |
| OS predictions | Do **not** depend on Microsoft’s typing-suggestions engine |
| Surface | Predictions only in **userland** (default input desktop) |
| Mode | **Prefix completion** only (no next-word-after-space in v1) |
| Context | **VK-only buffer** — no reading host control text |
| Engine | English **n-gram** tables **embedded** in the executable |
| UI | **5** ranked candidates, **3** visible; **LB/RB** cycle with **context swap**; **A** commits |
| Commit | Backspace partial prefix, inject full word, **no** auto-space |
| Personalization | **AppData** add-on dictionary; learn on VK-completed words; skip **secure fields** via UIA `IsPassword` on focused element; **conservative** skip if UIA fails |
| Toggle | **Always on** in userland (no disable flag in v1) |

Product glossary and behavioral definitions live in `CONTEXT.md`. This ADR records **why** the platform integration looks this way.

## Alternatives considered

### TSF / IME text service

**Pros:** Platform-native composition; could participate in input method contracts; cleaner long-term if Warmup becomes a full IME.

**Cons:** Large COM surface (`ITfTextInputProcessor`, registration, lifetime); does not match the current service/VK architecture; uncertain behavior across winlogon vs default desktop; still requires **our own** language model — TSF does not supply Microsoft’s prediction brain.

**Rejected for v1** because cost and risk dominate benefit for an Xbox-style overlay VK that already injects keys.

### Read focused control text (UI Automation / messages)

**Pros:** Richer context when the user mixed physical keyboard and VK.

**Cons:** Privacy and security sensitivity; fragile across browsers, Electron, and custom controls; overlaps with keylogging concerns.

**Rejected.** Context stays VK-only; UIA is used only to **gate personal-dictionary writes** on password fields.

### Microsoft Windows “text suggestions” setting

**Pros:** Zero model work if it were available.

**Cons:** Not offered as a third-party consumable API for arbitrary keyboards.

**Rejected.**

### Neural / ONNX local model

**Pros:** Higher quality rankings.

**Cons:** Binary size, latency, dependency stack; unnecessary for v1 proof of value.

**Deferred.** N-gram is sufficient for first ship.

## Consequences

### Positive

- Aligns with existing `SendInput` and gamepad wiring (`src/gamepad.rs`: LB/RB context swap must preserve page/Enter when strip inactive).
- Clear privacy story: no host buffer reads for prediction; personal dict guarded by `IsPassword` + conservative UIA failure.
- Ship path is incremental: predictor module + strip rendering + buffer hooks in `vk_nav` / `vk_ui` without platform registration.

### Negative

- **Blind to non-VK typing:** prefix context only includes characters Warmup injected; physical-keyboard prefixes get no suggestions until the user types on the VK.
- **Candidate commit is approximate:** backspacing `N` times matches the VK buffer, not a guaranteed snapshot of the host field (host may have diverged).
- **English-only v1** until layouts and corpora exist for other languages.
- **Browser password detection is best-effort:** custom controls without `IsPassword` may leak tokens into the personal dictionary; conservative UIA failure reduces learning but does not help mispublished fields.

### Follow-ups (out of scope for this ADR)

- Next-word prediction after space.
- Locale-specific embedded models or sidecar packs.
- TSF migration if product scope becomes “full IME.”
- User-facing disable toggle (explicitly declined for v1).

## Compliance

When implementing personal-dictionary writes, reuse or extend the gamepad loop’s COM/UIA apartment pattern (see `logon_focus.rs` and F9 dump path). Call `GetFocusedElement` + `CurrentIsPassword` at word completion; on any error, **do not** persist the term.

# Warmup Keyboard — domain glossary

Implementation-free terms for this product. Update as decisions land.

## Surfaces

### Userland

Interactive user session where normal applications run on the **default** input desktop (`winsta0\default`). Virtual keyboard may type here; **text prediction is in scope only on this surface.**

Not userland: sign-in (`LogonUI`), lock screen (`LockApp`), UAC consent — these use the **winlogon** input desktop.

### Winlogon surface

Secure / system input desktop (`winsta0\winlogon`) used for sign-in, lock, and UAC. Virtual keyboard may still operate here for typing, but **text prediction is out of scope.**

## Text input (product)

### Text prediction

Word or phrase suggestions shown above the virtual keyboard, chosen by the user to insert text faster. Powered by a **local** prediction engine owned by Warmup — not Microsoft’s built-in Windows typing suggestions API.

**v1 behavior:** **prefix completion** only — rank whole words that continue the partial token the user is composing on the VK. Next-word prediction (after space) is out of scope until a later version.

**Availability:** always enabled in userland when prefix rules match; no end-user toggle in v1.

### Prediction engine

Local offline component that ranks candidate words. **Decision (v1):** **n-gram model** — prefix-filtered lexicon ranked with bigram/trigram (or similar) statistics from prior words in the VK-only buffer, not a neural model. **v1 language:** English only; other locales deferred until keyboard layouts support them.

**Model packaging (v1):** n-gram tables ship **embedded** in the executable (build-time asset, not a sidecar file).

### Personal dictionary

Optional local word list that boosts ranking for terms the user types often. **Decision (v1):** a **local add-on dictionary** stored on disk (e.g. under the user’s app data), updated from VK activity only, never uploaded. Static embedded n-gram remains the base model.

**Learning rule:** add a word when the user completes it on the VK (space, punctuation, or candidate commit). Do **not** add when the focused control is a password field (see **Secure field**).

### Secure field

A text target where typed content must not be remembered. Detected in userland via UI Automation on the **currently focused element** (`GetFocusedElement`), checking `IsPassword` (and similar secure flags when exposed). Used only to gate personal-dictionary writes — not to block typing or predictions.

If secure-field detection fails (UIA/COM error, no focused element), **do not** add the word to the personal dictionary (**conservative**). Typing and predictions are unaffected.

### Candidate commit

What happens when the user confirms a suggestion. **Decision:** delete the partial prefix from the target field with backspaces (one per **character** the VK buffer recorded, not per byte), then inject the full chosen word — both as a **single Text-commit replace**. Do **not** auto-insert a trailing space — the user presses Space explicitly if they want one.

**Result is observable:** a commit reports whether the injection landed. Personal-dictionary learning and the VK-only buffer update happen **only on a landed commit** — if the injection fails, neither the dictionary nor the prediction context records the word.

### Candidate strip

On-screen UI row displaying prediction choices before commit. Shown during prefix completion in userland when the VK buffer has an active partial word of at least **two** characters.

**Capacity:** the engine ranks **five** candidates; **three** are visible at once in a sliding viewport. LB/RB move highlight across all five; the viewport scrolls so the highlighted word stays readable (e.g. at the ends, show candidates 0–2 or 2–4).

### Candidate selection

How the user picks a suggestion with the gamepad. **Decision:** **shoulder bumpers** — LB/RB cycle the highlighted candidate; **A** commits the selection. D-pad continues to navigate the key grid unless the user is explicitly refining the partial word by typing more characters.

When the candidate strip is active, LB/RB temporarily take over candidate cycling (**context swap**). When there is no active prefix or no candidates, LB/RB revert to their normal VK roles (symbol page and Enter).

### Text commit

How chosen characters reach the focused application. **Decision:** inject keystrokes through the same **`SendInput`** path the virtual keyboard already uses for key taps — not through a TSF/IME text service.

**Seam:** Text commit is a **substitutable seam** — a delete-then-insert *replace* operation (`replace(delete_count, insert_text)`) applied as one atomic injection. The real adapter drives `SendInput`; an in-memory adapter records the resulting text for tests. The second adapter is what makes the seam real: the commit decision (Candidate commit) becomes testable without Win32 or the live foreground field.

### Prediction context

Recent text the local prediction engine uses to rank suggestions. **Decision:** a **VK-only buffer** — characters and word boundaries recorded only from text this virtual keyboard commits via `SendInput`. No reading text from host application controls. Mixed physical-keyboard typing does not contribute to context.

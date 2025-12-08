# Ajemi TSF Engine Architecture & Event Loop Documentation

## Overview

Ajemi is a Windows Text Services Framework (TSF) Input Method Editor (IME) for Toki Pona. It converts ASCII character sequences into Sitelen Pona (visual writing system) characters. This document explains the complete event loop, how compositions start, how key presses are handled, and the overall architecture.

---

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [Event Loop Flow](#event-loop-flow)
3. [Initialization & Registration](#initialization--registration)
4. [Key Press Handling](#key-press-handling)
5. [Composition Lifecycle](#composition-lifecycle)
6. [State Management](#state-management)
7. [Suggestion Engine](#suggestion-engine)
8. [UI & Display](#ui--display)
9. [Detailed State Transitions](#detailed-state-transitions)

---

## Architecture Overview

### Component Structure

```
┌─────────────────────────────────────────────────────────────┐
│                    Windows TextServices                      │
│                    (TSF Framework)                           │
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│                TextService (tsf/mod.rs)                      │
│  ┌────────────────────────────────────────────────────────┐ │
│  │ ITfTextInputProcessor                                  │ │
│  │ ITfKeyEventSink ─────→ Key Event Handling              │ │
│  │ ITfThreadMgrEventSink ─────→ Focus/Context Events      │ │
│  │ ITfCompositionSink ─────→ Composition Events           │ │
│  │ ITfDisplayAttributeProvider ─────→ Display Attributes  │ │
│  └────────────────────────────────────────────────────────┘ │
│                          ↓                                   │
│  ┌────────────────────────────────────────────────────────┐ │
│  │ TextServiceInner (RwLock Protected Mutable State)      │ │
│  ├────────────────────────────────────────────────────────┤ │
│  │ • engine: Engine                                        │ │
│  │ • composition: Option<ITfComposition>                   │ │
│  │ • spelling: String (input buffer)                       │ │
│  │ • selected: String (committed parts)                    │ │
│  │ • suggestions: Vec<Suggestion>                          │ │
│  │ • preedit: String (display text)                        │ │
│  │ • candidate_list: CandidateList (UI)                    │ │
│  └────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│                    Engine (engine/mod.rs)                    │
│  • Dictionary Schema (words, alternatives, punctuation)      │
│  • Suggestion Generation Algorithm                           │
│  • Multi-word Sentence Matching                              │
└─────────────────────────────────────────────────────────────┘
```

### Thread Safety

- **RwLock Protection**: All mutable state in `TextServiceInner` is protected by `parking_lot::RwLock`
- **Timeout Handling**: Write locks wait up to 50ms with fallback to try_write
- **Composition Deadlock Mitigation**: Uses `try_write()` in composition termination to avoid circular waits

---

## Event Loop Flow

### High-Level Event Sequence

```
┌──────────────────────────────────────────────────────────────┐
│ 1. USER PRESSES KEY                                          │
└──────────────────────────────────────────────────────────────┘
                            ↓
┌──────────────────────────────────────────────────────────────┐
│ 2. OnTestKeyDown (tsf/key_event_sink.rs:47-79)              │
│    └─ Predicts if key will be consumed                       │
│    └─ Returns TRUE to consume, FALSE to let pass through      │
└──────────────────────────────────────────────────────────────┘
                            ↓
┌──────────────────────────────────────────────────────────────┐
│ 3. OnKeyDown (tsf/key_event_sink.rs:85-110)                │
│    └─ Actually processes key and modifies state              │
│    └─ Applies state changes: start composition, push char,   │
│       select suggestion, etc.                                │
└──────────────────────────────────────────────────────────────┘
                            ↓
┌──────────────────────────────────────────────────────────────┐
│ 4. OnKeyUp (tsf/key_event_sink.rs:133-150)                  │
│    └─ Handles key release (mainly for Ctrl toggle)           │
└──────────────────────────────────────────────────────────────┘
                            ↓
┌──────────────────────────────────────────────────────────────┐
│ 5. Edit Session Requests (edit_session.rs)                   │
│    └─ All text modifications happen in edit sessions         │
│    └─ Thread-safe text editing context                       │
└──────────────────────────────────────────────────────────────┘
                            ↓
┌──────────────────────────────────────────────────────────────┐
│ 6. Candidate List Display Update (ui/candidate_list.rs)      │
│    └─ Shows suggestions to user                              │
│    └─ Positions near cursor                                  │
└──────────────────────────────────────────────────────────────┘
```

---

## Initialization & Registration

### Application Startup (from ajemi-log.txt)

```
Line 1-9: DLL Registration
  ├─ Candidate list window class registered
  ├─ Input method registered with Windows TSF
  ├─ Language profile registered
  └─ Categories registered

Line 10-23: TextService Creation
  ├─ Engine initialized (loads dictionary)
  ├─ Text service instance created
  └─ Interface returned to TSF framework
```

### When User Switches to IME

```
Line 29-36: ITfTextInputProcessor::Activate()
  ├─ TextService::Activate() called
  ├─ Thread ID stored for this input context
  ├─ Key event sink registered with keystroke manager
  │  └─ OnKeyDown/OnKeyUp will now be called
  ├─ Thread manager event sink registered
  │  └─ OnInitDocumentMgr/OnPushContext will be called
  ├─ Candidate list window created
  └─ Display attribute provider initialized

Line 37-45: Window Class Setup
  └─ Candidate list window procedure ready to handle paint/mouse events
```

---

## Key Press Handling

### Input Parsing Pipeline

```
Raw Key Event (VK_CODE, Scancode)
        ↓
┌─────────────────────────────────────────────────────────┐
│ parse_input() (key_event_sink.rs:188-230)              │
│ • Converts VK_CODE to Input enum                        │
│ • Uses current keyboard layout (HKL)                    │
│ • Special cases: arrows, backspace, tab, etc.           │
└─────────────────────────────────────────────────────────┘
        ↓
┌─────────────────────────────────────────────────────────┐
│ Input Enum (simplified key type)                        │
│ ├─ Letter(char)      // 'a'..'z', 'A'..'Z'             │
│ ├─ Number(u8)        // '1'..'5' (for selection)       │
│ ├─ Punct(char)       // Special punctuation/joiners    │
│ ├─ Space             // Whitespace (confirmation)       │
│ ├─ Backspace         // Delete last char                │
│ ├─ Enter             // Release raw input               │
│ ├─ Tab               // Space + Release                 │
│ ├─ Arrow keys        // Navigation (ignored)            │
│ └─ Unknown           // Unsupported                     │
└─────────────────────────────────────────────────────────┘
        ↓
┌─────────────────────────────────────────────────────────┐
│ State Validation Checks (key_event_sink.rs:64-77)      │
│ ├─ disabled_by_capslock()   // CapsLock disables IME   │
│ ├─ disabled_naively()       // Ctrl or Eisu keys       │
│ └─ VK_CAPITAL.is_toggled()  // Physical capslock state │
└─────────────────────────────────────────────────────────┘
        ↓
┌─────────────────────────────────────────────────────────┐
│ handle_input() (key_event_sink.rs:292-346)             │
│ └─ Apply state transitions based on current state       │
└─────────────────────────────────────────────────────────┘
```

### OnTestKeyDown vs OnKeyDown

**OnTestKeyDown** (Prediction Phase):
- Called first to ask: "Will you consume this key?"
- Returns TRUE/FALSE without making permanent changes
- Client MAY ignore the return value and call OnKeyDown anyway
- Client MAY skip OnKeyDown if OnTestKeyDown returned FALSE
- **Problem**: You cannot "take it back" later, so must gather needed info here

**OnKeyDown** (Action Phase):
- Actually modifies state (adds to composition, commits text, etc.)
- Can be called directly without OnTestKeyDown
- Must handle same logic as OnTestKeyDown
- Return value indicates: "I consumed the key" (TRUE) or "I didn't handle it" (FALSE)

### Ctrl Toggle Mechanism

```
OnTestKeyDown(VK_CONTROL):
  ├─ Set fresh_ctrl = true
  └─ Return FALSE (don't consume)
                ↓
OnKeyUp(VK_CONTROL):
  ├─ If fresh_ctrl was true:
  │  ├─ Toggle disabled_by_ctrl flag
  │  └─ Set fresh_ctrl = false
  └─ This allows Ctrl to toggle IME on/off
```

---

## Composition Lifecycle

### State Model

The engine has **exactly two states**:

```
STATE 1: NOT COMPOSING (composition.is_none())
  └─ No underlined text in editor
  └─ Waiting for first letter input
  └─ spelling, suggestions, selected all empty
                ↓
        Letter Key Pressed
                ↓
         start_composition()
                ↓
STATE 2: COMPOSING (composition.is_some())
  ├─ Underlined text in editor showing preedit
  ├─ User can add more letters, select from suggestions, etc.
  ├─ spelling buffer contains accumulated input
  └─ suggestions shown in candidate list
                ↓
        Space/Enter/Backspace to End/Backspace/Commit
                ↓
        end_composition()
                ↓
        BACK TO STATE 1
```

### Starting Composition

```python
# From log lines 50-61
def start_composition():
    # 1. Create composition object (edit_session::start_composition)
    #    This tells TSF we're starting a composition
    composition = edit_session.start_composition(
        tid, context, composition_sink
    )
    self.composition = Some(composition)

    # 2. Get cursor position to place candidate list
    (x, y) = get_pos()
    candidate_list.locate(x, y)

    # No text displayed yet - waiting for first character input
```

### Accepting a Character (push)

```python
# From log lines 73-89
def push(ch: char):
    # 1. Add character to spelling buffer
    spelling.push(ch)

    # 2. Generate suggestions from engine
    suggestions = engine.suggest(spelling)

    # 3. Build display text (preedit) with grouping delimiters
    #    Format: [selected] + [spelling with group markers]
    update_preedit()

    # 4. Update candidate list display
    update_candidate_list()

    # 5. Modify composition text in editor via edit session
    edit_session.set_text(preedit)

    # State remains: COMPOSING
```

### Committing Selection (Space key)

```python
# From composition.rs:165-175
def commit():
    if suggestions.is_empty():
        force_release(' ')
    else:
        select(0)  # Select first suggestion

def select(index: usize):
    sugg = suggestions[index]
    last = sugg.groupping.last()

    if last == spelling.len():
        # All input consumed by this word
        selected.push_str(sugg.output)
        set_text(selected)
        end_composition()  # FINISH
    else:
        # More input remains - continue composing
        selected.push_str(sugg.output)
        spelling = spelling[last..]
        suggestions = engine.suggest(spelling)
        update_preedit()
        update_candidate_list()
        # State remains: COMPOSING
```

### Releasing Raw Input (Enter key)

```python
# From composition.rs:227-239
def release():
    if selected.is_empty():
        set_text(spelling)
    else:
        selected.push(' ')
        selected.push_str(spelling)
        set_text(selected)

    end_composition()  # FINISH
```

### Backspace Handling

```python
# From composition.rs:150-163
def pop():
    spelling.pop()

    if spelling.is_empty():
        end_composition()  # Return to NOT COMPOSING
    else:
        suggestions = engine.suggest(spelling)
        update_preedit()
        update_candidate_list()
        # State remains: COMPOSING
```

### Ending Composition

```python
# From composition.rs:38-54
def end_composition():
    # 1. End the composition in TSF framework
    edit_session.end_composition(tid, context, composition)

    # 2. Clear all composition state
    composition = None
    spelling.clear()
    selected.clear()
    suggestions.clear()

    # 3. Hide candidate list
    candidate_list.hide()

    # State transitions to: NOT COMPOSING
```

---

## State Management

### TextServiceInner Structure

```rust
pub struct TextServiceInner {
    // Engine & Dictionary
    engine: Engine,

    // TSF Context Info
    tid: u32,                           // Thread ID for this input context
    thread_mgr: Option<ITfThreadMgr>,   // TSF thread manager
    context: Option<ITfContext>,        // Current text editing context

    // Event Sink State
    hkl: HKL,                           // Keyboard layout
    char_buf: String,                   // Character buffer
    fresh_ctrl: bool,                   // Just pressed Ctrl?
    disabled_by_ctrl: bool,             // IME disabled by Ctrl toggle
    cookie: Option<u32>,                // Event sink registration cookie

    // Composition State
    composition: Option<ITfComposition>, // Active composition or None
    spelling: String,                   // Raw input buffer
    selected: String,                   // Committed parts
    suggestions: Vec<Suggestion>,       // Current suggestions
    preedit: String,                    // Display text

    // UI
    candidate_list: Option<CandidateList>,
    display_attribute: Option<VARIANT>,

    // Self Reference
    interface: Option<ITfTextInputProcessor>,
}
```

### RwLock Access Pattern

```
Entry Point (e.g., OnKeyDown):
    ↓
let mut inner = self.write()?  // Acquire exclusive lock
    ↓
Modify state (push, pop, etc.)
    ↓
Drop write guard when function returns  // Release lock
    ↓
Other threads can now access
```

**Lock Timeout**: 50ms with try_write fallback to prevent hangs

---

## Suggestion Engine

### Data Structure: Suggestion

```rust
pub struct Suggestion {
    pub output: String,         // The mapped Sitelen Pona text
    pub groupping: Vec<usize>,  // Byte positions of word boundaries
}

// Example: "lilonsewi" → "li" + " " + "lon" + " " + "sewi"
// becomes: "llinonesewii" with groupping: [2, 5, 9]
//          ^output        ^byte positions marking word ends
```

### Suggestion Algorithm (engine/mod.rs:138-160)

```python
def suggest(spelling: str) -> Vec<Suggestion>:
    results = []

    # Try sentence matching first (optional feature)
    if sentence_suggestions_enabled:
        if sugg = suggest_sentence(spelling):
            results.push(sugg)

    # Try single-word matches from longest to shortest prefix
    for prefix_len in [len(spelling), ..., 1]:
        prefix = spelling[..prefix_len]

        if candidate = schema.candis.get(prefix):
            match candidate:
                case Exact(word, variants):
                    # Multiple exact matches
                    for var in variants:
                        results.push(make_suggestion(var, prefix_len))
                        if results.len() >= CANDI_NUM:
                            return results

                case Unique(word):
                    # Only word with this prefix
                    results.push(make_suggestion(word, prefix_len))
                    if results.len() >= CANDI_NUM:
                        return results

                case Duplicates(words):
                    # Multiple words with this prefix
                    for word in words:
                        results.push(make_suggestion(word, prefix_len))
                        if results.len() >= CANDI_NUM:
                            return results

    return results
```

### Dictionary Lookup Format (engine/schema.rs)

```
[spelling] [output] [alternative1] [alternative2] ...

Example:
li              llinonesewii
jan             jaja
musi            liimu

[punct] [output]    # Punctuation remapping
.                   •
:                   ꞉

' [open] [close]    # Quote handling
"                   『』
```

### Sentence Matching (engine/sentence.rs:69-130)

```python
def suggest_sentence(spelling: str) -> Option<Suggestion>:
    # Recursively try to match multi-word sequences

    def suggest_sentences_recursive(
        spelling: str,
        current_score: int,
        accumulated_output: str,
        groupping: Vec<usize>
    ) -> Option<Suggestion>:

        if spelling.is_empty():
            if groupping.len() >= 2:  # At least 2 words
                return Some(Suggestion {
                    output: accumulated_output,
                    groupping: groupping
                })
            else:
                return None

        # Try each possible prefix
        for len in [len(spelling), ..., 1]:
            prefix = spelling[..len]

            if candidate = schema.candis.get(prefix):
                match candidate:
                    case Unique(word):
                        score += 20 * len  # High priority
                        rest = spelling[len..]

                        # Recurse with remaining input
                        if result = suggest_sentences_recursive(
                            rest,
                            score,
                            accumulated_output + word,
                            groupping + [accumulated_output.len()]
                        ):
                            return Some(result)

                    case Exact/Duplicates:
                        score += 10 + bonus_for_length
                        # Similar recursion...

        return None

    return suggest_sentences_recursive(spelling, 0, "", [])
```

---

## UI & Display

### Candidate List Window (ui/candidate_list.rs)

```
┌───────────────────────────────┐
│  Candidate List Popup         │
├───────────────────────────────┤
│ 1.候補1      (index 1)       │ ← Highlighted
│ 2. 候補2      (index 2)       │
│ 3. 候補3      (index 3)       │
│ 4. 候補4      (index 4)       │
│ 5. 候補5      (index 5)       │
└───────────────────────────────┘
```

### Display Flow

```
1. Composition state changes
        ↓
2. update_candidate_list() (composition.rs:88-103)
        ↓
3. If suggestions.is_empty()
   └─ candidate_list.hide()
   └─ No popup shown

   Else
   └─ candidate_list.show(suggestions)
   └─ candidate_list.locate(x, y)
   └─ Position near current cursor
        ↓
4. candidate_list.wind_proc() (ui/candidate_list.rs:89)
   └─ Windows message handler for the popup window
        ↓
5. WM_PAINT message
   └─ paint() (ui/candidate_list.rs:378)
   └─ Renders background, borders, text
        ↓
6. Display on screen near cursor
```

### Preedit Display Format

```
Spelling: "li"
Suggestions: [
    { output: "llinonesewii", groupping: [2, 5, 9] }  // li
]

udpate_preedit() builds:
selected = "" (nothing committed yet)
preedit = "" + "li" = "li"

Then edit_session.set_text("li") with underline/underline attribute
Result in editor: _li_  (underlined)
```

---

## Detailed State Transitions

### Complete State Machine

```
┌──────────────────────────────────────────────────────────┐
│ START: NOT COMPOSING                                     │
│ spelling = ""                                             │
│ selected = ""                                             │
│ suggestions = []                                          │
│ composition = None                                        │
└──────────────────────────────────────────────────────────┘
                    ↓
        ╔═══════════════════════════════════════╗
        ║ LETTER KEY: 'a', 'b', ..., 'z'       ║
        ╚═══════════════════════════════════════╝
                    ↓
    [start_composition() creates ITfComposition]
    [push(letter) adds to spelling]
    [suggestions = engine.suggest(spelling)]
    [update_preedit() builds display]
                    ↓
┌──────────────────────────────────────────────────────────┐
│ STATE: COMPOSING                                         │
│ spelling = "a"                                            │
│ composition = Some(ITfComposition)                        │
│ suggestions = [...]                                       │
│ preedit = "a" (displayed as underlined)                   │
│ candidate_list showing first N suggestions                │
└──────────────────────────────────────────────────────────┘
                    ↓
        ┌─────────────────────────────────────┐
        │ User can now press:                 │
        ├─────────────────────────────────────┤
        │ LETTER      → add to spelling       │
        │ NUMBER 1-5  → select suggestion     │
        │ SPACE       → confirm selection     │
        │ ENTER       → release raw input     │
        │ BACKSPACE   → remove last char      │
        │ PUNCT (=,`) → add to spelling       │
        │             (if joiner char)        │
        │             OR force_commit+release │
        └─────────────────────────────────────┘
                    ↓
        ╔═══════════════════════════════════════╗
        ║ LETTER KEY: add more characters       ║
        ╚═══════════════════════════════════════╝
                    ↓
    [spelling += letter]
    [suggestions = engine.suggest(spelling)]
    [update_preedit() updates display]
    [update_candidate_list() updates UI]
    [State remains: COMPOSING]
                    ↓
        ╔═══════════════════════════════════════╗
        ║ SPACE KEY: confirm selection          ║
        ╚═══════════════════════════════════════╝
                    ↓
    [select(0) → select first suggestion]

    if suggestion covers all of spelling:
        [selected += suggestion.output]
        [set_text(selected)]
        [end_composition()]
        → GOTO NOT COMPOSING
    else:
        [selected += suggestion.output]
        [spelling = spelling[after_suggestion..]]
        [suggestions = engine.suggest(spelling)]
        [update_preedit()]
        [State remains: COMPOSING]
                    ↓
        ╔═══════════════════════════════════════╗
        ║ NUMBER KEY 1-5: select nth candidate  ║
        ╚═══════════════════════════════════════╝
                    ↓
    [select(number-1)]
    [Same logic as SPACE but different index]
                    ↓
        ╔═══════════════════════════════════════╗
        ║ ENTER KEY: release raw text           ║
        ╚═══════════════════════════════════════╝
                    ↓
    [set_text(selected + " " + spelling)]
    [end_composition()]
    → GOTO NOT COMPOSING
                    ↓
        ╔═══════════════════════════════════════╗
        ║ BACKSPACE: remove last character      ║
        ╚═══════════════════════════════════════╝
                    ↓
    [spelling.pop()]

    if spelling.is_empty():
        [end_composition()]
        → GOTO NOT COMPOSING
    else:
        [suggestions = engine.suggest(spelling)]
        [update_preedit()]
        [State remains: COMPOSING]
                    ↓
        ╔═══════════════════════════════════════╗
        ║ PUNCT KEY: punctuation/joiner         ║
        ╚═══════════════════════════════════════╝
                    ↓
    if is_joiner_char(punct):
        [push(punct)]
        [State remains: COMPOSING]
    else:
        [force_commit(remap_punct)]
        [Output selected + spelling + remapped_punct]
        [end_composition()]
        → GOTO NOT COMPOSING
                    ↓
        ╔═══════════════════════════════════════╗
        ║ CTRL KEY: toggle IME on/off           ║
        ╚═══════════════════════════════════════╝
                    ↓
    [disabled_by_ctrl = !disabled_by_ctrl]

    if disabled_by_ctrl:
        [abort()]
        [end_composition()]
        → GOTO NOT COMPOSING
        → Further input goes directly to app
    else:
        → Ready to compose again
                    ↓
        ╔═══════════════════════════════════════╗
        ║ CAPSLOCK: disable IME                 ║
        ╚═══════════════════════════════════════╝
                    ↓
    [abort()]
    [Output selected + spelling]
    [end_composition()]
    → GOTO NOT COMPOSING
    → User types uppercase letters directly
```

---

## Event Sink Responsibilities

### ITfKeyEventSink (key_event_sink.rs)

Handles keyboard input:
- `OnTestKeyDown()` - Predict if key will be consumed
- `OnKeyDown()` - Process key, modify state
- `OnKeyUp()` - Handle key release (Ctrl toggle)
- `OnSetFocus()` - Focus gained/lost (cleanup on blur)

### ITfThreadMgrEventSink (thread_mgr_event_sink.rs)

Handles TSF lifecycle:
- `OnInitDocumentMgr()` - New text document context created
- `OnPushContext()` - Input context activated
- `OnPopContext()` - Input context deactivated
- `OnUninitDocumentMgr()` - Document closing

### ITfCompositionSink (composition.rs:276-293)

Handles composition lifecycle:
- `OnCompositionTerminated()` - External code ended our composition
  - Uses `try_write()` to avoid deadlock

### ITfTextInputProcessor (text_input_processor.rs)

Lifecycle management:
- `Activate()` - IME activated for this context
  - Registers event sinks
  - Creates candidate list window
- `Deactivate()` - IME deactivated
  - Unregisters event sinks
  - Destroys candidate list

---

## Edit Sessions (edit_session.rs)

All text modifications must occur in edit sessions for thread safety.

### start_composition

```rust
pub fn start_composition(
    tid: u32,
    context: &ITfContext,
    composition_sink: &ITfCompositionSink,
) -> Result<ITfComposition>
```

1. Creates `ITfEditSession` struct
2. In `DoEditSession()`:
   - Gets current selection/cursor via `InsertTextAtSelection(TF_IAS_QUERYONLY, [])`
   - Starts composition on that range
   - Stores composition in Cell<Option<>>
3. Requests edit session from context
4. Waits for framework to execute edit session
5. Returns the composition object

### set_text

```rust
pub fn set_text(
    tid: u32,
    context: &ITfContext,
    range: &ITfRange,
    text: &[u16],  // Unicode UTF-16
    display_attr: Option<&VARIANT>,
) -> Result<()>
```

1. Creates edit session
2. In `DoEditSession()`:
   - Replaces text in the range with new text
   - Applies display attributes (underline, color hints)
3. Requests edit session from framework
4. Waits for execution

### end_composition

```rust
pub fn end_composition(
    tid: u32,
    context: &ITfContext,
    composition: &ITfComposition,
) -> Result<()>
```

1. Creates edit session
2. In `DoEditSession()`:
   - Calls `composition.EndComposition(ec)`
3. Requests edit session
4. Waits for execution
5. After this, `ITfCompositionSink::OnCompositionTerminated()` fires

---

## Log Trace Analysis

### Initialization Phase (lines 1-49)

```
Line 1-9:    DLL loading and registration
Line 10-23:  TextService creation and engine initialization
Line 24-45:  IME registration with TSF
             candidate list window class setup
```

### First Key Press (lines 50-134)

```
Line 50:     OnKeyDown called
Line 51:     TextService::write() acquires lock
Line 52-58:  Input validation (ctrl, capslock, config)
Line 59:     handle_input() processes the character
Line 60:     start_composition() begins composition
Line 61-72:  get_pos(), composition(), interface() setup
Line 73:     push() adds character to spelling
Line 74-88:  engine.suggest() generates suggestions
Line 89:     udpate_preedit() builds display text
Line 96:     update_candidate_list() shows UI
Line 97-104: Candidate list positioned and displayed
Line 111-134: paint() renders candidate list on screen
Line 135:    OnKeyUp for key release
```

### Second Key Press (lines 137-160)

```
Line 137-145: OnKeyUp from previous key
Line 148:     OnKeyDown for new key
Line 149-161: Same pipeline but push() adds to existing composition
```

### Key Selection (lines 147-160)

```
Line 147:     force_commit() - user selected or confirmed
Line 148:     select() - choose suggestion
Line 150:     set_text() - update display
Line 153:     end_composition() - finish this word
Line 155-157: candidate_list.hide() - UI cleanup
```

### Application Switch (lines 259-275)

```
Line 259:     OnSetFocus(false) - focus lost
Line 261:     abort() - cancel composition
Line 267:     Deactivate() - IME deactivated
Line 270-271: Sinks removed
```

---

## Performance Considerations

1. **Lock Contention**: RwLock with 50ms timeout prevents hanging
2. **Suggestion Generation**: Capped at CANDI_NUM results (typically 5)
3. **Dictionary Lookups**: HashMap<String, Candidate> O(1) average case
4. **Candidate List**: Updated only when suggestions change
5. **Preedit Display**: String rebuilt on each keystroke (acceptable for IME)

---

## Error Handling

- **Lock Acquisition Failure**: Returns E_FAIL after 50ms timeout
- **Missing Context**: Returns E_FAIL if context/thread_mgr None
- **Edit Session Failure**: Logs error, continues with graceful degradation
- **Composition Termination**: Uses try_write() to avoid deadlock
- **Focus Loss**: Calls abort() to clean up pending composition

---

## Configuration & Customization

### conf.toml (from README.md lines 89-113)

```toml
[font]
name = "sitelen seli kiwen juniko"
size = 20

[layout]
vertical = false

[color]
clip = "#0078D7"
background = "#FAFAFA"
highlight = "#E8E8FF"
index = "#A0A0A0"
candidate = "black"

[behavior]
toggle = "Ctrl"
long_pi = false
long_glyph = false
```

### Dictionary Files

Located in `%APPDATA%/Ajemi/dict/`, format:
```
[spelling] [output] [alternative1] ...
# Comments start with #
```

---

## Summary

The Ajemi TSF engine operates as a state machine with two primary states:

1. **NOT COMPOSING**: Waiting for first character input
2. **COMPOSING**: Accumulating input, generating suggestions, awaiting confirmation

Key architectural principles:

- **Thread-Safe State**: RwLock protects all mutable state
- **Lazy Composition**: Composition started only on first letter
- **Incremental Suggestions**: Regenerated on each keystroke
- **Multi-word Matching**: Engine handles both single and multi-word sequences
- **Edit Sessions**: All text modifications through TSF edit sessions
- **Event-Driven**: Reacts to key events via TSF sink interfaces
- **UI Feedback**: Real-time candidate list updates showing suggestions

The log traces show a clear progression from initialization, through key presses, suggestion generation, composition management, and finally cleanup on application switch.


# TECH.md

Technical plan. Pairs with `GAME.md` (which holds the design). This document is opinionated and revisable as we build.

## Language and Runtime

**Rust**, stable toolchain. Single binary distribution, cross-compiles cleanly to macOS / Linux / Windows.

The game is turn-based and single-threaded by nature. The engine, UI, simulation, and storage layers are **plain sync code** — no `async`/`.await` at the engine boundary.

The exception is the LLM module. Every maintained Rust client for OpenRouter is async-only on top of `tokio`. Rather than reinvent that wire format, we adopt `tokio` and **contain it inside `src/llm/`**: a worker thread owns a `tokio::runtime::Runtime`, runs async HTTP calls inside it, and exposes a sync API (return values, `std::sync::mpsc::Receiver` for streaming) to the rest of the codebase. Nothing outside `src/llm/` touches `async`.

## Core Dependencies

| Concern | Crate | Notes |
|---|---|---|
| TUI | `ratatui` | Layout, widgets, rendering. |
| Terminal backend | `crossterm` | Cross-platform, pairs with ratatui. |
| Serialization | `serde` + `toml` + `serde_json` | TOML for structured state and content; JSON only where needed (LLM API). |
| Markdown | `pulldown-cmark` | If we ever need to parse Markdown back in; mostly we just write it. |
| LLM client | `openrouter-rs` | OpenRouter-native: provider routing, fallback, app-attribution headers. Async, used only inside `src/llm/`. |
| Async runtime | `tokio` (rt-multi-thread, macros) | Isolated to `src/llm/`. The rest of the codebase is sync. |
| RNG | `rand` + `rand_chacha` | Seeded, reproducible. ChaCha for determinism across versions. |
| Errors | `anyhow` (app) + `thiserror` (engine boundaries) | |
| CLI args | `clap` | Entry-point options (`--seed`, `--content-pack`, etc). |
| Graph | none initially; `petgraph` if relationship queries get expensive | Start with `HashMap<(Id, Id), Edge>` and revisit. |
| Tracing | `tracing` + `tracing-subscriber` | Structured logging to a file; the TUI owns the terminal. |
| Property testing | `proptest` | For system invariants. |
| Snapshot testing | `insta` | For worldgen reproducibility and prompt output. |

No game-engine framework. The game is text and turns; bringing in `bevy` or similar would dwarf the actual work.

## Project Structure

Single crate, organized by module. Workspaces are premature.

```
book/
  Cargo.toml
  GAME.md
  TECH.md
  src/
    main.rs              entry point + CLI
    app.rs               top-level TUI app state and event loop
    ui/                  ratatui views and widgets
      mod.rs
      scene.rs           the current-scene view
      notebook.rs        observation log
      creation.rs        character creation
      epilogue.rs
    engine/
      mod.rs
      sim.rs             the tick / step function
      worldgen.rs        full worldgen pipeline
      event.rs           Event enum and event log
      action.rs          Action enum (player + NPC actions)
    systems/
      needs.rs
      mood.rs
      traits.rs
      knowledge.rs       the centerpiece — see below
      relationships.rs
      reputation.rs
      goals.rs
    world/
      mod.rs             World struct, the authoritative state
      npc.rs
      location.rs
      item.rs
      ids.rs             typed ID newtypes
    llm/
      mod.rs
      openrouter.rs      client
      prompts.rs         prompt builders, one per call site
      stream.rs          worker-thread streaming
    content/
      mod.rs             content-pack loader and registry
    storage/
      mod.rs             plain-text save/load
  content/               shipped content packs (data, not code)
    settings/
      medieval/
    vocations/
      investigator/
  saves/                 created at runtime
  tests/                 integration tests
```

## State Storage

Plain text. Human-readable. The user should be able to open the save directory and read what their character knows, what NPCs think, what happened. Filesystem inspection is a deliberate part of the experience — paired with the second-terminal note-taking meta-game from `GAME.md`.

```
saves/
  current/
    world.toml             metadata: seed, day, current event index, in-world clock
    case.toml              ground-truth of the mystery (hidden from player UI but on disk)
    player.toml            player character profile, traits, vocation, needs, mood
    notebook.md            player-visible observation log; what the player has seen and been told
    events.log             append-only event log (JSONL, machine-readable)
    characters/
      <id>-<slugname>/
        profile.toml       traits, mood, needs, schedule, occupation
        knowledge.md       what this character knows, narrated for readability
        relationships.toml directed edges from this character
        history.md         chronological memorable-event log for this character
    locations/
      <id>-<slug>.toml
```

`.md` files are human-readable narration over machine-readable state in TOML. Both are produced by the engine; the markdown is a *projection* of state, not the source of truth. Source of truth is the TOML and the event log.

## Engine Architecture

### Identity

All cross-entity references use **newtype'd integer IDs**, not pointers or `&` references. This sidesteps borrow-checker friction in the social graph and is exactly the shape we want on disk.

```rust
#[derive(Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NpcId(pub u32);
pub struct LocationId(pub u32);
pub struct ItemId(pub u32);
// etc.
```

A central `World` struct owns all entity registries. Systems take `&mut World` and operate via IDs. NPCs do not hold references to each other; they hold IDs that resolve through the World.

### Event Sourcing (Pragmatic)

Two layers:

1. **Memorable events** are first-class. Stored as an append-only log (`events.log`, JSONL). Every meaningful state change — a fact learned, a relationship shifted, a goal formed or completed — is an `Event` in this log. The per-character `history.md` is rendered from this log. This makes "why does the miller distrust the player?" answerable by `grep`.

2. **Routine ticks** — needs decay, mood normalization, schedule advancement — are imperative, unlogged, and recomputed on load if needed. They are not interesting to remember.

```rust
pub enum Event {
    Witnessed { observer: NpcId, fact: Fact, when: Time },
    Told { teller: NpcId, listener: NpcId, fact: Fact, distortion: Distortion },
    RelationshipShift { subject: NpcId, object: NpcId, delta: EdgeDelta, cause: Cause },
    GoalFormed { holder: NpcId, goal: Goal },
    GoalResolved { holder: NpcId, goal: Goal, outcome: Outcome },
    Spoken { speaker: NpcId, listeners: Vec<NpcId>, line: String, intent: SpeechIntent },
    Moved { who: NpcId, to: LocationId, when: Time },
    // ...
}
```

The `Event` enum is one of the central reasons we picked Rust. Exhaustive pattern matching over it catches missed call sites whenever a new variant is added.

### The Knowledge System

The single most important system. It is *not* a flat "fact set" per character. It is a graph keyed by `(NpcId, Fact)` with metadata per edge:

```rust
pub struct KnowledgeEdge {
    pub source: KnowledgeSource,     // Witnessed | ToldBy(NpcId) | Inferred
    pub confidence: f32,             // 0.0..=1.0
    pub distortion: Distortion,      // how much the fact has mutated in transit
    pub acquired_at: Time,
}
```

Rumor propagation is its own tick: NPCs in proximity may exchange known facts, with distortion increasing on transmission, biased by traits (gossipy, secretive, etc.). This is *all* internal — the LLM is never asked "should this rumor spread." Rules decide.

### Worldgen Pipeline

Worldgen runs entirely without LLM calls — it is pure simulation. The LLM is only used after worldgen to render the world to the player.

```
1. Seed RNG from CLI or random.
2. Generate village geography and locations from content pack.
3. Generate population: NPCs with traits, ages, demographics.
4. Generate family/household/employment graph.
5. Initialize possessions, occupations, schedules.
6. Run simulation forward N in-world years (no LLM, fast ticks).
   Pressures accumulate naturally: debts, jealousies, ambitions.
7. Detect "ripe moment" — a pressure crossing a threshold that
   licenses an act. Resolve into an Incident.
8. Record the Incident: who, what, where, witnesses, evidence,
   what each surviving NPC now believes, was told, and doesn't know.
9. Generate the player character.
10. Place the player in the village with a vocation-driven hook.
```

Worldgen is the highest-risk, highest-reward subsystem. We build it iteratively: a minimum viable village first, then layer in pressure mechanics.

### Determinism

Given the same seed and the same content pack version, worldgen and simulation produce identical results. Snapshot tests assert this. The LLM is the only nondeterministic component, and it touches surface text only — never world state.

## LLM Integration

Provider: **OpenRouter**, via the `openrouter-rs` crate. Model is configurable per call site (cheap for routine dialogue, stronger for confrontations and epilogue) — `openrouter-rs` exposes OpenRouter's provider-routing and fallback features so the engine can request, e.g., `anthropic/claude-haiku-4-5` with a fallback to a different provider.

`openrouter-rs` is async-only on tokio. We isolate this: the `src/llm/` module owns a `tokio::runtime::Runtime` on a worker thread, runs async calls inside it via `runtime.block_on(...)`, and exposes a sync API to the engine. Nothing else in the codebase imports `tokio`.

### Call Sites

1. **NPC dialogue** — given speaker traits, listener relationship, mood, what the speaker knows about the topic, recent events. Returns one or a few lines.
2. **Player action options** — given protagonist traits, mood, and the current scene state, render 3–5 contextually appropriate action/dialogue options in the protagonist's voice. The *set* of available options is determined by the engine; the LLM writes the *text*.
3. **Player free-form dialogue interpretation** — the player types a line; the LLM identifies intent and tone; the engine applies the resulting Action; the LLM then voices the NPC response.
4. **Scene narration** — when a player enters a location, a short paragraph describing what their character notices (filtered by perception).
5. **Epilogue** — end-of-playthrough narration grounded in the full event log.
6. **Names and flavor** — batched, cached. Lowest-priority calls.

### Hard Boundary

The LLM input is *always* structured state plus a prompt template. The LLM output is *only* surface text — dialogue, narration, names, options. The LLM is never trusted to:

- Decide what is true
- Decide who knows what
- Decide relationship deltas
- Decide outcomes

Outputs that *imply* state changes (e.g., an NPC saying "I'll tell you a secret…") trigger engine logic to record the appropriate Event. The text and the state-change are produced by different layers.

### Streaming and Threading

Streaming responses arrive token-by-token from OpenRouter. The worker thread inside `src/llm/` runs the async stream inside its tokio runtime and pushes tokens onto an `std::sync::mpsc` channel. The TUI consumes from the channel on the main thread and renders incrementally. The main thread stays responsive and sees only the channel, never `async`.

### Caching and Budget

- A per-character voice "fingerprint" prompt is constructed once and reused so dialogue style stays consistent within a playthrough.
- Identical prompts within a session are cached in-memory.
- Each call site has a configured model tier; cheaper tiers are the default.
- A `--dry-run` mode replaces the LLM with deterministic fakes for testing and budget-free development.

## Content Pack Format

Content packs are pure data, loaded at startup. They contain no code.

```
content/settings/medieval/
  setting.toml           name, period, register, version
  vocabulary.toml        period-specific terms by category
  names/
    given.male.txt
    given.female.txt
    surnames.txt
  items.toml             item registry: kind, properties, value
  locations.toml         location archetypes
  occupations.toml
  taboos.toml            cultural rules that drive scandal mechanics
  prompts/
    npc_voice.md         template for NPC dialogue
    scene_open.md
    epilogue.md
    options.md           template for player action options
```

Vocations follow the same shape under `content/vocations/<name>/`. Loaded packs are registered in a `ContentRegistry` and selected at character creation.

A pack version field allows the engine to refuse a pack built against an incompatible engine version.

## TUI Design

Layered views via `ratatui`:

- **Character creation** — trait/vice/inclination/background picker.
- **Scene view** — primary play surface. A panel describing the scene, an interaction panel for dialogue and observation, a sidebar for the protagonist's state (mood, needs, time, events remaining today).
- **Notebook** — full-screen observation log, scrollable. Read-only.
- **World inspector (dev mode only)** — peek at full state. Hidden in shipped builds.

Color and styling stay restrained — period-appropriate. ASCII art is welcome for headers and transitions but should not dominate.

## Testing Strategy

- **Unit tests** per system: needs decay, mood update under known event, knowledge propagation under controlled topology.
- **Property tests** for invariants: relationships round-trip on save/load; knowledge confidence stays in `[0, 1]`; no NPC holds a fact they couldn't have acquired.
- **Snapshot tests** for worldgen: a fixed seed produces a stable world. Drift is a deliberate change.
- **Integration tests** for whole scenes: a scripted player walks through a known scenario with a mock LLM and we assert sim state after each step.
- **Prompt tests**: prompt builders are pure functions of state; their output is snapshot-tested so we can iterate on prompt wording with confidence.

## Build, Run, Distribute

- `cargo run` for development. The binary takes `--seed`, `--content-pack`, `--save-dir`, `--dry-run`, `--log-level`.
- Release builds with `--profile release-lto`.
- Cross-compile via `cargo` targets. Single-binary distribution.
- LLM keys via environment variable (`OPENROUTER_API_KEY`); never in saves, never committed.

## Open Technical Questions

- **Streaming or batch dialogue?** Streaming feels more alive but adds UI complexity. Default to batch for v0, add streaming once the loop is solid.
- **Budget target per playthrough.** A 2–4 hour run with ~80 events and several dialogue exchanges per event has real cost implications. We will measure during prototyping and tune model tiers.
- **Free-form dialogue parsing reliability.** The player types a line and the LLM identifies intent; we need a confidence threshold below which we ask for clarification rather than misinterpret. To be tuned.
- **Worldgen step count vs. quality.** How many in-world years do we simulate before play begins? Long enough for pressures to be real, short enough to keep startup fast. Estimate: 3–10 years; will tune.
- **Mod loading.** Eventually we want users to drop in their own content packs. v1 ships with bundled packs only; loading from a `~/.config/book/` style directory is a later addition.

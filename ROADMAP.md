# ROADMAP.md

A working checklist for building this game. Tasks are sized for a single focused session — a few hours to a day each. Each phase builds on the previous one. Check items off as you complete them.

For game design rationale, see `GAME.md`. For technical architecture and decisions, see `TECH.md`.

## How to use this document

- Work through phases in order. Within a phase, tasks can sometimes be reordered, but dependencies are noted in the text.
- Each task points to the relevant section of `GAME.md` and/or `TECH.md` where the detail lives.
- Open questions in `TECH.md` ("Open Technical Questions") may need resolution before certain tasks.
- When a task surfaces work the roadmap missed, add it.

---

## Phase 0 — Bootstrap (done)

- [x] Cargo project initialized as `loom`, Rust edition 2024
- [x] Dependencies pinned in `Cargo.toml` per `TECH.md` "Core Dependencies"
- [x] Module skeleton scaffolded: `app`, `ui`, `engine`, `systems`, `world`, `llm`, `content`, `storage`
- [x] Minimal title screen renders; `[q]` quits

---

## Phase 1 — Engine Foundations (done)

The substrate everything else depends on. After this phase, you can serialize a world, log events, talk to the LLM (sync API, with a dry-run fallback), and write structured logs.

- [x] **Typed entity IDs.** Newtypes `NpcId(u32)`, `LocationId(u32)`, `ItemId(u32)` in `src/world/ids.rs`. Derive `Copy`, `Clone`, `PartialEq`, `Eq`, `Hash`, `Serialize`, `Deserialize`. See `TECH.md` "Identity".
- [x] **`World` struct.** Central registry owning entity collections (e.g. `HashMap<NpcId, Npc>`). Minimal `Npc` and `Location` types for now — more fields added in Phase 4. See `TECH.md` "Identity" and `GAME.md` "The World and Its People".
- [x] **`Event` enum and append-only log.** Variants per `TECH.md` "Event Sourcing (Pragmatic)". Persists as JSONL to `saves/current/events.log`.
- [x] **Storage module.** Save/load `World`, `PlayerCharacter`, event log to the directory layout in `TECH.md` "State Storage". Round-trip test asserts byte-identical reload.
- [x] **LLM module skeleton.** Worker thread owns a `tokio::runtime::Runtime`. `llm::Client::chat(req)` is sync; internally `block_on`s an async call via `openrouter-rs`. Streaming via `std::sync::mpsc::Receiver<String>`. See `TECH.md` "LLM Integration".
- [x] **`--dry-run` LLM mode.** A `FakeClient` returns deterministic stub strings (`"[npc says X]"`). Selected by CLI flag. Lets tests and dev run without API cost.
- [x] **CLI arguments via `clap`.** `--seed`, `--save-dir`, `--dry-run`, `--log-level`. Wire into `App::new`.
- [x] **Logging to file.** `tracing-subscriber` writes structured logs to `<save-dir>/loom.log`. TUI keeps stdout.

---

## Phase 2 — Content Packs (done)

Data-driven setting and vocation. After this phase, the engine loads a content pack at startup and treats it as a registry.

- [x] **Content types.** Structs for `Setting`, `Vocation`, `Vocabulary`, `Items`, `Locations`, `Occupations`, `Taboos`, loadable from TOML. See `TECH.md` "Content Pack Format".
- [x] **Pack loader.** Reads `content/settings/<name>/` and `content/vocations/<name>/`. Validates pack version against engine version.
- [x] **Medieval setting pack (default).** Authored content under `content/settings/medieval/`: `setting.toml`, name lists, item registry, location archetypes, occupations, taboos. Small but coherent.
- [x] **Investigator vocation pack (default).** Authored content under `content/vocations/investigator/`: starting skills, contacts schema, opening-hook template.
- [x] **Prompt templates.** Markdown files under `content/settings/medieval/prompts/`: `npc_voice.md`, `scene_open.md`, `options.md`, `epilogue.md`. See `TECH.md` "LLM Integration".

---

## Phase 3 — Character Creation (done)

The first real player-facing UI. No LLM or worldgen required.

- [x] **`Screen::Creation` variant in `App`.** From the title screen, `[n]` enters creation.
- [x] **Trait picker UI.** Three pages — virtue, vice, inclination. Options come from the loaded content pack. Keyboard navigation. See `GAME.md` "The Player Character".
- [x] **Background picker.** Chosen from a small set defined by the vocation pack.
- [x] **Confirm screen.** Display the chosen character; allow back-out.
- [x] **Construct and save `PlayerCharacter`.** Write `player.toml` in the save dir.

---

## Phase 4 — Social Simulation Systems (done)

The heart of the engine. Build each system in isolation with unit tests, then compose. Needs and mood are simplest and stand alone; knowledge and goals depend on the event log; relationships depend on events.

- [x] **FactId migration.** Added `FactId(u32)` newtype; `Event::Witnessed`/`Told` carry `FactId`; `World.facts` registry with `intern_fact` dedup-by-value.
- [x] **Needs system.** `Needs { hunger, sleep, safety, belonging, purpose }`, per-tick decay, `satisfy()` helper. Routine ticks are unlogged per `TECH.md`.
- [x] **Mood system.** `MoodImprint` ring buffer (capped at 16) with weight-decay tick; live `Mood.valence` recomputed as weighted mean. Imprints fed from Witnessed/Told/RelationshipShift/GoalResolved.
- [x] **Traits and values.** Closed `Value` enum + `ValueWeights` (normalizes negatives to zero) + `TraitSet` with cross-category lookup.
- [x] **Relationships system.** Directed `BTreeMap<(NpcId, NpcId), Edge>` on `World` (`pub(crate)`). `apply_relationship_shift` is the only mutator; grievance decay on tick. See `GAME.md` "Romance and Player-Authored Stories" for the romance-edge split — deferred to Phase 8.
- [x] **Knowledge system.** `BTreeMap<FactId, KnowledgeEdge>` per NPC with `source`, `confidence`, `distortion`, `acquired_at`. Rumor-propagation tick: NPCs in proximity propose `Event::Told` transmissions, distortion increments per hop biased by traits (Gossipy, Secretive, Untrustworthy, Boastful).
- [x] **Reputation system.** Per-NPC `{ honor, menace, competence }`; slow decay toward zero; `bump`/`bump_by_id` helpers for public-action-driven shifts.
- [x] **Goals system.** Content-pack-driven (`content/settings/medieval/goals.toml`, 8 archetypes). Serializable `Condition`/`Score`/`Resolution` DSL evaluated against NPC + World; hysteresis prevents goal thrash. Emits `GoalFormed`/`GoalResolved`.
- [x] **`apply_event` dispatcher.** Single public mutation funnel for `Event` values in `systems::mod`. Each `apply_*` is `pub(crate)`; routes events and feeds mood imprints.
- [x] **Storage extension.** `facts.toml` at save root; per-NPC `relationships.toml` + `knowledge.toml`; existing round-trip stays byte-identical.
- [x] **System property tests.** All five ROADMAP invariants pass via proptest — `confidence ∈ [0,1]`; relationships round-trip + byte-identical; no NPC holds a fact they couldn't have acquired; relationships only change via `Event::RelationshipShift`; `intern_fact` is stable for equal Facts.

---

## Phase 5 — Worldgen (done)

After this phase, you can produce a populated village with a buried incident, ready for play.

- [x] **Engine orchestration.** New `src/engine/sim.rs::step` runs all Phase 4 ticks + a `daily_activity_pass`. New `src/engine/worldgen.rs::run(content, seed, player)` returns a `WorldgenResult { world, incident, player_hook, events }`. Per-stage seed derivation via `WorldgenSeeds` so editing one stage doesn't shift downstream RNG streams.
- [x] **Village geography.** `Location` extended with `kind`, `description`, and an `adjacent: Vec<LocationId>` graph. Stage 1 instantiates one location per `LocationArchetype`, picks a village name from a new `place_names.toml`, and lays a connected adjacency graph (random spanning tree + ~N/3 shortcuts). `LocationId(0)` reserved as the off-map "elsewhere" sentinel.
- [x] **Population.** `Npc` extended with `age, sex, attracted_to, spouse, parents, children, employer, occupation, dead`. Stage 2 inserts 35–45 NPCs with rolled names (sex-aware), traits + inclination, age (Beta-shaped), normalized `ValueWeights`, and placement.
- [x] **Family and employment graph.** Stage 3 pairs mutually-attracted adults as spouses (~60% rate target), adopts age-appropriate children, emits high-affection `RelationshipShift`s. Stage 4 assigns each `Occupation` archetype a head + subordinates (trait-scored), tags remaining adults as `household`/`idle` and children as `child`.
- [x] **Forward simulation.** Stage 5 runs `sim::step` for 5 in-world years × 365 = 1825 ticks (sub-second in release). Each tick runs every Phase 4 system plus a goal-driven `daily_activity_pass` (movement on adjacency + co-located pair interactions + rare affair triggers).
- [x] **Ripe-moment detection.** `engine::incident::detect_ripe` scans for the highest-scoring pressure (Death > Scandal > Disappearance) above per-kind thresholds. `nudge_pressure` bumps the top grievance edge if nothing fires organically.
- [x] **Mystery kinds (initial three).** `engine::incident::resolve` materializes Death, Disappearance, or Scandal: emits `Witnessed`/`Told`/`Died`/`Moved`/`Spoken` events through the `apply_event` dispatcher. Public-facts vs. perpetrator-only knowledge cleanly separated.
- [x] **Player placement.** Stage 8 inserts the `PlayerCharacter` as `NpcId(0)` at the entry location (tavern → chapel → fallback), seeds initial knowledge from `incident.public_facts` at confidence 0.5, rolls a deadline (4–14 days), and renders the vocation pack's `opening_hook.md` template with `{{village.name}}`, `{{incident.kind}}`, `{{deadline.days}}`, `{{first.location}}` substitution.
- [x] **Storage.** `saves/current/case.toml` holds the `Incident`; `WorldMeta` carries `village_name`, `start_day`, `deadline_day`; `SaveDir::save_case`/`load_case` added. Existing per-NPC files cover the new structural fields via `#[serde(default)]`.
- [x] **App wiring.** New `Screen::Ready` renders village name, opening hook, case summary, and deadline after worldgen completes on creation-confirm. `[n]` new game, `[q]` quit. `src/lib.rs` added so integration tests can use the public API.
- [x] **Worldgen snapshot tests.** `tests/worldgen_snapshot.rs` (insta, yaml feature) snapshots the NPC roster, the location list, and the incident summary for seed 42. Re-runs are byte-stable.
- [x] **Integration tests.** `tests/worldgen_integration.rs` validates three seeds (all references resolve), exercises save/load round-trip (byte-identical world.toml / facts.toml), and confirms same-seed determinism end-to-end.

---

## Phase 6 — The Play Loop

The first end-to-end playable experience. Four events per day, scene-based.

- [x] **`Screen::Scene` variant.** Renders location, day/time, presence (player-perceived), narration, and a state-aware action menu. Session state (live `World`, `EventLog`, `SaveDir`, `rng`) lives on `App`.
- [x] **Scene narration via LLM.** `llm::builders::scene_open_request` fills `scene_open.md`; the result is cached in `SceneState.narration_cache` keyed by `(LocationId, mood_bucket)`. Deterministic fallback if the client errors.
- [x] **Action menu with state-aware options.** `engine::action::available_actions` builds the action list (move per adjacency, interview per present NPC, observe/notebook/wait). `llm::builders::options_request` flavors them; cached in `SceneState.options_cache` keyed by the engine-label hash, re-flavored when the action set changes.
- [x] **Move action.** `Action::Move(LocationId)` emits `Event::Moved`, decrements `events_remaining_today`, calls `sim::tick_scene`, opens a fresh scene at the destination.
- [x] **Interview / dialogue.** Hybrid path: `SceneMode::DialogueIntent` (menu over Greet/Ask/Inform/Confront/Comfort/Lie) → `SceneMode::DialogueLine` (free-form text capture) → `npc_voice_request` LLM call → `SceneMode::DialogueReply`. Both spoken lines log as `Event::Spoken` and are bulleted into the notebook.
- [x] **Observe action.** `Action::Observe` interns a scene-summary `Fact`, emits `Event::Witnessed` for `NpcId(0)`, and writes a bullet to `notebook.md`. Does not end the scene.
- [x] **Notebook.** `Screen::Notebook` is a full-screen scrollable view of `saves/<current>/notebook.md` rendered by `src/ui/notebook.rs`. Read-only. Reached via the `OpenNotebook` action.
- [x] **Inter-event simulation tick.** `engine::sim::tick_scene` advances `clock_minutes` by 180, runs a quarter-strength `activity_pass`, knowledge propagation, mood and needs. The slow daily-only systems (relationships decay, reputation drift, goals) only run on the midnight wrap.
- [x] **Day rollover.** When `tick_scene` reports the rollover and `events_remaining_today` reaches 0, the clock resets to 08:00, the event budget refills to 4, an italic `— Day N —` banner appears on the next scene, and a `## Day N` header is appended to the notebook.

---

## Phase 7 — Resolution

End of playthrough.

- [x] **Deadline tracking.** Scene header shows "days to deadline: N", colored yellow at ≤2 and red at 0. Derived from `world.deadline_day - world.day`.
- [x] **Accusation action.** `Action::Accuse` opens `SceneMode::Accuse` (an NPC picker over alive non-player ids). On confirm, `Incident::is_correct_accusation` grades the choice into `CaseOutcome::Solved` or `MisAccused`.
- [x] **Case-close detection.** `case_close_check` runs after every scene tick and fires `CaseOutcome::Died` (player dead) or `Unsolved` (`day >= deadline_day`); `Action::LeaveTown` fires `LeftTown`; accusation fires `Solved`/`MisAccused`. All paths funnel through `App::close_case`.
- [x] **Epilogue.** `llm::builders::epilogue_request` fills the `epilogue.md` template with outcome, accusation, culprit, player relationships, witnessed events, hidden events, and surviving-NPC summaries. `fallback_epilogue` provides deterministic prose when the LLM call errors.
- [x] **`Screen::Epilogue`.** `src/ui/epilogue.rs` renders the outcome label and the scrollable body. `[n]` returns to creation, `[q]` quits, `[j/k]` and `[PgUp/PgDn]` scroll.

---

## Phase 8 — Polish and Depth

Once an end-to-end playthrough works, deepen and harden.

- [x] **Streaming dialogue.** NPC responses arrive token-by-token. The OpenRouter worker uses a multi-thread tokio runtime and spawns a task per stream, pushing `StreamChunk` values onto `std::sync::mpsc`. `App::run` switches to a polled loop (`event::poll(40ms)` + `drain_dialogue_stream`) while a `SceneMode::DialogueStreaming` is live. On `Done` the accumulated line is committed as a single `Event::Spoken` and a notebook bullet; the UI shows a blinking-cursor live buffer until then. `FakeClient::chat_stream` sleeps between tokens so dry-run mode exercises the same path.
- [ ] **More mystery kinds.** Theft, false accusation, returning stranger.
- [ ] **More NPC behaviors.** Schedules, vendettas, alliances.
- [ ] **Romance system depth.** Independent attraction/affection/trust edges, courtship arcs, jealousy triggered by witnessed events. See `GAME.md` "Romance and Player-Authored Stories".
- [ ] **Hypothesis tracking (optional UI layer).** See `GAME.md` "The Notebook".
- [ ] **Prompt-builder snapshot tests.** Every prompt builder is a pure function of state; snapshot its output.
- [ ] **Integration playthrough test.** Scripted player walks a scenario with the fake LLM client. Assert sim state at each step.

---

## Beyond v1

Speculative. Captured for memory.

- Mod loading from `~/.config/loom/content/`.
- Additional setting packs: sci-fi (station, post-apocalyptic, or similar).
- Additional vocation packs: physician, confessor, scribe.
- Hypothesis-linking UI as a first-class feature.
- Mid-playthrough save (currently the anthology format means no save is needed).

use std::collections::{HashMap, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use anyhow::{Context, Result};
use crossterm::event::{self, Event as CtEvent, KeyCode, KeyEventKind};
use rand::{RngCore, SeedableRng};
use rand_chacha::ChaCha8Rng;
use ratatui::DefaultTerminal;

use crate::config::{CallSite, UserConfig};
use crate::content::{self, ContentRegistry};
use crate::engine::{
    self, Action, CaseOutcome, Event, EventLog, Fact, Incident, Time,
    available_actions, engine_label,
};
use crate::llm;
use crate::llm::builders;
use crate::storage::{self, SaveDir, SaveSlot};
use crate::systems::apply_event;
use crate::ui;
use crate::world::{LocationId, NpcId, PlayerCharacter, World};
use crate::Cli;

pub struct App {
    pub running: bool,
    pub screen: Screen,
    pub cli: Cli,
    pub client: Box<dyn llm::Client>,
    pub content: ContentRegistry,
    pub user_config: UserConfig,
    /// Holds the live world once worldgen has run. None on the title and
    /// creation screens.
    pub session: Option<Session>,
}

pub struct Session {
    pub world: World,
    pub save_dir: SaveDir,
    pub log: EventLog,
    pub rng: ChaCha8Rng,
    pub incident: Incident,
    /// Per-NPC running transcript of the current location's dialogue. Each
    /// entry is `(is_player, line)`, oldest first. Cleared on location change
    /// so each scene's conversation is fresh.
    pub dialogue_history: HashMap<NpcId, Vec<(bool, String)>>,
}

pub enum Screen {
    Title(TitleState),
    SaveBrowser(SaveBrowserState),
    Config(ConfigState),
    Creation(CreationState),
    Ready(ReadyState),
    Scene(SceneState),
    Notebook(NotebookState),
    Epilogue(EpilogueState),
}

/// State for the title menu — a vertically scrollable list of actions. The
/// content setting is chosen during character creation (one setting per save),
/// not on this screen.
pub struct TitleState {
    pub selected: usize,
    /// Number of resumable save slots discovered at startup. Drives whether
    /// the Continue row is enabled or shown as a no-op hint.
    pub saves_count: usize,
}

/// Row indices for the Title menu. Kept as constants so the input handler can
/// match on `state.selected` without an enum + lookup table.
pub const TITLE_ROW_NEW_GAME: usize = 0;
pub const TITLE_ROW_CONTINUE: usize = 1;
pub const TITLE_ROW_CONFIG: usize = 2;
pub const TITLE_ROW_QUIT: usize = 3;
pub const TITLE_ITEM_COUNT: usize = 4;

impl TitleState {
    pub fn new(cli: &Cli) -> Self {
        let saves_count = storage::list_save_slots(saves_parent(&cli.save_dir))
            .map(|v| v.len())
            .unwrap_or(0);
        Self {
            selected: 0,
            saves_count,
        }
    }
}

/// The directory we scan for save slots — the parent of `cli.save_dir`, since
/// the CLI default `saves/current` puts a single slot under `saves/`.
pub fn saves_parent(save_dir: &std::path::Path) -> &std::path::Path {
    save_dir.parent().unwrap_or_else(|| std::path::Path::new("."))
}

pub struct SaveBrowserState {
    pub slots: Vec<SaveSlot>,
    pub selected: usize,
}

/// In-TUI config editor. `drafts` mirrors the editable string for each field;
/// `selected` is the currently focused row. Edits are persisted on Esc.
pub struct ConfigState {
    pub fields: Vec<ConfigField>,
    pub drafts: Vec<String>,
    pub selected: usize,
    pub config_path: PathBuf,
    /// Set after a save attempt; rendered as a one-line status under the form.
    pub status: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigField {
    ApiKey,
    DefaultModel,
    ModelSceneNarration,
    ModelOptions,
    ModelNpcVoice,
    ModelEpilogue,
}

impl ConfigField {
    pub fn label(self) -> &'static str {
        match self {
            ConfigField::ApiKey => "api_key",
            ConfigField::DefaultModel => "default_model",
            ConfigField::ModelSceneNarration => "models.scene_narration",
            ConfigField::ModelOptions => "models.options",
            ConfigField::ModelNpcVoice => "models.npc_voice",
            ConfigField::ModelEpilogue => "models.epilogue",
        }
    }
}

impl ConfigState {
    pub fn from_config(cfg: &UserConfig, config_path: PathBuf) -> Self {
        let fields = vec![
            ConfigField::ApiKey,
            ConfigField::DefaultModel,
            ConfigField::ModelSceneNarration,
            ConfigField::ModelOptions,
            ConfigField::ModelNpcVoice,
            ConfigField::ModelEpilogue,
        ];
        let drafts = fields
            .iter()
            .map(|f| match f {
                ConfigField::ApiKey => cfg.api_key.clone().unwrap_or_default(),
                ConfigField::DefaultModel => cfg.default_model.clone().unwrap_or_default(),
                ConfigField::ModelSceneNarration => {
                    cfg.models.scene_narration.clone().unwrap_or_default()
                }
                ConfigField::ModelOptions => cfg.models.options.clone().unwrap_or_default(),
                ConfigField::ModelNpcVoice => cfg.models.npc_voice.clone().unwrap_or_default(),
                ConfigField::ModelEpilogue => cfg.models.epilogue.clone().unwrap_or_default(),
            })
            .collect();
        Self {
            fields,
            drafts,
            selected: 0,
            config_path,
            status: None,
        }
    }

    /// Build a fresh `UserConfig` from the current drafts. Empty strings become
    /// `None` so the precedence rules in `config.rs` still kick in.
    pub fn build_config(&self) -> UserConfig {
        let pick = |i: usize| {
            self.drafts
                .get(i)
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.trim().to_string())
        };
        let mut cfg = UserConfig::default();
        for (i, f) in self.fields.iter().enumerate() {
            match f {
                ConfigField::ApiKey => cfg.api_key = pick(i),
                ConfigField::DefaultModel => cfg.default_model = pick(i),
                ConfigField::ModelSceneNarration => cfg.models.scene_narration = pick(i),
                ConfigField::ModelOptions => cfg.models.options = pick(i),
                ConfigField::ModelNpcVoice => cfg.models.npc_voice = pick(i),
                ConfigField::ModelEpilogue => cfg.models.epilogue = pick(i),
            }
        }
        cfg
    }
}

pub struct EpilogueState {
    pub body: String,
    pub outcome_label: String,
    pub scroll: u16,
}

pub struct ReadyState {
    pub village_name: String,
    /// Setting-derived noun for the place — "village", "ship". Used as the UI
    /// label next to `village_name`.
    pub place_kind: String,
    pub incident_summary: String,
    pub deadline_days: u32,
    pub player_hook: String,
    pub entry: LocationId,
}

pub struct SceneState {
    pub here: LocationId,
    pub narration: String,
    pub present: Vec<NpcId>,
    pub actions: Vec<ActionEntry>,
    pub selected: usize,
    pub mode: SceneMode,
    pub events_remaining_today: u8,
    pub narration_cache: HashMap<(LocationId, u8), String>,
    pub options_cache: HashMap<u64, Vec<String>>,
    /// True if we should render with a "Day N — morning" banner the next frame.
    pub day_banner: Option<String>,
    /// Receiver for the scene-open stream while it is in flight. When `None`,
    /// `narration` is the final text. When `Some`, `narration` is being built
    /// incrementally — see [`App::drain_narration_stream`].
    pub narration_rx: Option<llm::StreamReceiver>,
    /// The (location, mood-bucket) key under which the finished narration
    /// will be cached on `Done`. `None` if narration came from cache or a
    /// fallback path that bypassed the stream.
    pub narration_cache_key: Option<(LocationId, u8)>,
}

#[derive(Clone, Debug)]
pub struct ActionEntry {
    pub action: Action,
    pub engine_label: String,
    pub flavored: Option<String>,
}

pub enum SceneMode {
    Browsing,
    DialogueLine {
        npc: NpcId,
        buffer: String,
    },
    /// The NPC's response is arriving token-by-token from the LLM. We hold the
    /// receiver here and drain it from the main loop.
    DialogueStreaming {
        npc: NpcId,
        npc_name: String,
        player_line: String,
        /// Visible reply text, accumulated by the JSON path-watcher.
        buffer: String,
        /// Raw tool-args JSON, accumulated for final parse on Done.
        args_buffer: String,
        /// State machine that extracts the `reply` chars from streamed
        /// tool-args without waiting for the whole JSON to arrive.
        reply_streamer: crate::llm::interpret::ReplyTokenStreamer,
        rx: llm::StreamReceiver,
        had_error: bool,
        /// How many bytes (UTF-8 boundary safe) of `buffer` have been
        /// revealed to the player so far. Decoupling reveal from raw chunk
        /// arrival gives a typewriter effect even when the model dumps the
        /// whole tool-call payload in a single delta — which several
        /// OpenRouter providers do under `force_tool`.
        revealed: usize,
        /// Set once the receiver emits `Done`. We don't finalize until the
        /// reveal cursor catches up to `buffer`, so the player sees the
        /// closing chars before the mode transitions.
        done: bool,
    },
    DialogueReply {
        npc: NpcId,
        npc_name: String,
        line: String,
    },
    /// Picking a target for an accusation. `targets` lists alive, non-player
    /// NPCs in display order; `selected` indexes into it.
    Accuse {
        targets: Vec<NpcId>,
        selected: usize,
    },
}

pub struct NotebookState {
    pub body: String,
    pub scroll: u16,
    /// Where to return to when the player closes the notebook.
    pub return_to: ReturnTo,
}

pub enum ReturnTo {
    Ready,
    Scene,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreationStep {
    Setting,
    Virtue,
    Vice,
    Inclination,
    Background,
    Name,
    Confirm,
}

pub struct CreationState {
    pub step: CreationStep,
    /// Setting pack chosen at the first step. Persisted alongside the save so
    /// resume can rehydrate the correct content registry.
    pub staged_setting: String,
    /// All setting pack directories discovered under `<content_root>/settings/`.
    /// The Setting step shows this as a selectable list.
    pub available_settings: Vec<String>,
    pub virtue: Option<String>,
    pub vice: Option<String>,
    pub inclination: Option<String>,
    pub background: Option<String>,
    pub rolled_name: String,
    pub selected_index: usize,
    pub rng: ChaCha8Rng,
}

impl CreationState {
    pub fn new(seed: u64, cli: &Cli, registry: &ContentRegistry) -> Self {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let rolled_name = roll_name(&mut rng, registry);
        let available_settings = content::list_setting_packs(&cli.content_root);
        let selected_index = available_settings
            .iter()
            .position(|s| s == &cli.setting)
            .unwrap_or(0);
        Self {
            step: CreationStep::Setting,
            staged_setting: cli.setting.clone(),
            available_settings,
            virtue: None,
            vice: None,
            inclination: None,
            background: None,
            rolled_name,
            selected_index,
            rng,
        }
    }

    pub fn option_count(&self, registry: &ContentRegistry) -> usize {
        match self.step {
            CreationStep::Setting => self.available_settings.len(),
            CreationStep::Virtue => registry.setting.traits.virtues.len(),
            CreationStep::Vice => registry.setting.traits.vices.len(),
            CreationStep::Inclination => registry.setting.traits.inclinations.len(),
            CreationStep::Background => registry.vocation.backgrounds.backgrounds.len(),
            CreationStep::Name | CreationStep::Confirm => 0,
        }
    }

    pub fn move_up(&mut self, registry: &ContentRegistry) {
        let n = self.option_count(registry);
        if n == 0 {
            return;
        }
        self.selected_index = (self.selected_index + n - 1) % n;
    }

    pub fn move_down(&mut self, registry: &ContentRegistry) {
        let n = self.option_count(registry);
        if n == 0 {
            return;
        }
        self.selected_index = (self.selected_index + 1) % n;
    }

    pub fn reroll_name(&mut self, registry: &ContentRegistry) {
        self.rolled_name = roll_name(&mut self.rng, registry);
    }

    /// Advance to the next step, recording the selected option for the current page.
    /// Returns true if creation is complete (Confirm was pressed on Confirm step).
    ///
    /// For the Setting step the caller is responsible for reloading the
    /// ContentRegistry after `confirm` returns — this method just stages the
    /// chosen pack name on `staged_setting` and advances the step.
    pub fn confirm(&mut self, registry: &ContentRegistry) -> bool {
        match self.step {
            CreationStep::Setting => {
                if let Some(pick) = self.available_settings.get(self.selected_index) {
                    self.staged_setting = pick.clone();
                }
                self.step = CreationStep::Virtue;
                self.selected_index = 0;
            }
            CreationStep::Virtue => {
                self.virtue = registry
                    .setting
                    .traits
                    .virtues
                    .get(self.selected_index)
                    .map(|t| t.name.clone());
                self.step = CreationStep::Vice;
                self.selected_index = 0;
            }
            CreationStep::Vice => {
                self.vice = registry
                    .setting
                    .traits
                    .vices
                    .get(self.selected_index)
                    .map(|t| t.name.clone());
                self.step = CreationStep::Inclination;
                self.selected_index = 0;
            }
            CreationStep::Inclination => {
                self.inclination = registry
                    .setting
                    .traits
                    .inclinations
                    .get(self.selected_index)
                    .map(|t| t.name.clone());
                self.step = CreationStep::Background;
                self.selected_index = 0;
            }
            CreationStep::Background => {
                self.background = registry
                    .vocation
                    .backgrounds
                    .backgrounds
                    .get(self.selected_index)
                    .map(|b| b.name.clone());
                self.step = CreationStep::Name;
                self.selected_index = 0;
            }
            CreationStep::Name => {
                self.step = CreationStep::Confirm;
            }
            CreationStep::Confirm => return true,
        }
        false
    }

    /// Step back one page. Returns true if we should return to the title.
    pub fn back(&mut self) -> bool {
        match self.step {
            CreationStep::Setting => return true,
            CreationStep::Virtue => {
                // Setting stays staged — going back lets the user pick a
                // different pack without starting over from the title.
                self.step = CreationStep::Setting;
            }
            CreationStep::Vice => {
                self.virtue = None;
                self.step = CreationStep::Virtue;
            }
            CreationStep::Inclination => {
                self.vice = None;
                self.step = CreationStep::Vice;
            }
            CreationStep::Background => {
                self.inclination = None;
                self.step = CreationStep::Inclination;
            }
            CreationStep::Name => {
                self.background = None;
                self.step = CreationStep::Background;
            }
            CreationStep::Confirm => {
                self.step = CreationStep::Name;
            }
        }
        self.selected_index = 0;
        false
    }

    pub fn build_player(&self, vocation_name: &str) -> Option<PlayerCharacter> {
        Some(PlayerCharacter {
            id: NpcId(0),
            name: self.rolled_name.clone(),
            vocation: vocation_name.to_string(),
            virtue: self.virtue.clone()?,
            vice: self.vice.clone()?,
            inclination: self.inclination.clone()?,
            background: self.background.clone()?,
        })
    }
}

fn roll_name(rng: &mut ChaCha8Rng, registry: &ContentRegistry) -> String {
    use rand::seq::SliceRandom;
    let names = &registry.setting.names;
    let mut given_pool: Vec<&String> = names.given_male.iter().chain(names.given_female.iter()).collect();
    let given = given_pool
        .as_mut_slice()
        .choose(rng)
        .map(|s| s.as_str())
        .unwrap_or("Anon");
    let surname = names
        .surnames
        .choose(rng)
        .map(|s| s.as_str())
        .unwrap_or("Of-Nowhere");
    format!("{} {}", given, surname)
}

fn creation_seed(cli_seed: Option<u64>) -> u64 {
    match cli_seed {
        Some(s) => s.wrapping_add(0xC4EA),
        None => rand::rngs::OsRng.next_u64(),
    }
}

impl App {
    pub fn new(
        cli: Cli,
        client: Box<dyn llm::Client>,
        content: ContentRegistry,
        user_config: UserConfig,
    ) -> Self {
        let title = TitleState::new(&cli);
        Self {
            running: true,
            screen: Screen::Title(title),
            cli,
            client,
            content,
            user_config,
            session: None,
        }
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        const STREAM_POLL: std::time::Duration = std::time::Duration::from_millis(40);
        while self.running {
            terminal.draw(|frame| ui::render(frame, &self))?;
            if self.is_streaming() {
                // Drain whatever has arrived since the last frame, then check
                // for input non-blockingly so the redraw cadence is driven by
                // token arrival rather than keypresses.
                self.drain_narration_stream()?;
                self.drain_dialogue_stream()?;
                if event::poll(STREAM_POLL)? {
                    self.handle_input()?;
                }
            } else {
                self.handle_input()?;
            }
        }
        Ok(())
    }

    fn handle_input(&mut self) -> Result<()> {
        let CtEvent::Key(key) = event::read()? else {
            return Ok(());
        };
        if key.kind != KeyEventKind::Press {
            return Ok(());
        }

        // The borrow checker doesn't love mutating self in match-on-self.screen,
        // so each arm either acts in-place on the matched state or transitions
        // by replacing self.screen at the end.
        match &mut self.screen {
            Screen::Title(state) => {
                let total = TITLE_ITEM_COUNT;
                match key.code {
                    KeyCode::Char('q') => self.running = false,
                    KeyCode::Up | KeyCode::Char('k') => {
                        state.selected = (state.selected + total - 1) % total;
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        state.selected = (state.selected + 1) % total;
                    }
                    KeyCode::Enter => match state.selected {
                        TITLE_ROW_NEW_GAME => {
                            self.start_new_game()?;
                        }
                        TITLE_ROW_CONTINUE => {
                            if state.saves_count > 0 {
                                self.enter_save_browser()?;
                            }
                        }
                        TITLE_ROW_CONFIG => {
                            self.enter_config_editor();
                        }
                        TITLE_ROW_QUIT => self.running = false,
                        _ => {}
                    },
                    _ => {}
                }
            }
            Screen::SaveBrowser(state) => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => {
                    self.screen = Screen::Title(TitleState::new(&self.cli));
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if !state.slots.is_empty() {
                        state.selected = (state.selected + state.slots.len() - 1)
                            % state.slots.len();
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if !state.slots.is_empty() {
                        state.selected = (state.selected + 1) % state.slots.len();
                    }
                }
                KeyCode::Enter => {
                    if let Some(slot) = state.slots.get(state.selected).cloned() {
                        self.resume_save(&slot)?;
                    }
                }
                _ => {}
            },
            Screen::Config(state) => match key.code {
                KeyCode::Esc => {
                    // Esc commits the form to disk and returns to the title.
                    // No "discard" — the form is shallow enough that accidental
                    // edits are easy to fix; persistence on exit is the more
                    // expected behavior for a config panel.
                    let new_cfg = state.build_config();
                    let path = state.config_path.clone();
                    match new_cfg.save_to_path(&path) {
                        Ok(()) => {
                            self.user_config = new_cfg;
                            self.rebuild_llm_client();
                            self.screen = Screen::Title(TitleState::new(&self.cli));
                        }
                        Err(e) => {
                            state.status = Some(format!("save failed: {}", e));
                        }
                    }
                }
                KeyCode::Up => {
                    if !state.fields.is_empty() {
                        state.selected =
                            (state.selected + state.fields.len() - 1) % state.fields.len();
                    }
                }
                KeyCode::Down | KeyCode::Tab => {
                    if !state.fields.is_empty() {
                        state.selected = (state.selected + 1) % state.fields.len();
                    }
                }
                KeyCode::Backspace => {
                    if let Some(draft) = state.drafts.get_mut(state.selected) {
                        draft.pop();
                    }
                }
                KeyCode::Char(c) => {
                    if let Some(draft) = state.drafts.get_mut(state.selected) {
                        draft.push(c);
                    }
                }
                _ => {}
            },
            Screen::Creation(state) => match key.code {
                KeyCode::Char('q') => self.running = false,
                KeyCode::Esc | KeyCode::Char('b') => {
                    if state.back() {
                        self.screen = Screen::Title(TitleState::new(&self.cli));
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => state.move_up(&self.content),
                KeyCode::Down | KeyCode::Char('j') => state.move_down(&self.content),
                KeyCode::Char('r') if state.step == CreationStep::Name => {
                    state.reroll_name(&self.content);
                }
                KeyCode::Enter => {
                    let was_setting = state.step == CreationStep::Setting;
                    let done = state.confirm(&self.content);
                    if was_setting {
                        // The user just locked in a setting pack. If it differs
                        // from the currently loaded one, swap content now —
                        // subsequent steps read traits and backgrounds from
                        // the new registry. Re-roll the name from the new
                        // setting's name lists so the suggestion fits.
                        let staged = state.staged_setting.clone();
                        if staged != self.cli.setting {
                            let new_reg = ContentRegistry::load(
                                &self.cli.content_root,
                                &staged,
                                &self.cli.vocation,
                            )
                            .with_context(|| {
                                format!("loading setting {:?} during creation", staged)
                            })?;
                            self.cli.setting = staged;
                            self.content = new_reg;
                            state.reroll_name(&self.content);
                        }
                    }
                    if done {
                        let player = self.save_player_from_creation()?;
                        let ready = self.run_worldgen(&player)?;
                        self.screen = Screen::Ready(ready);
                    }
                }
                _ => {}
            },
            Screen::Ready(state) => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => self.running = false,
                KeyCode::Char('n') => {
                    let seed = creation_seed(self.cli.seed);
                    self.session = None;
                    self.screen =
                        Screen::Creation(CreationState::new(seed, &self.cli, &self.content));
                }
                KeyCode::Enter => {
                    let entry = state.entry;
                    let scene = self.begin_scene(entry, None)?;
                    self.screen = Screen::Scene(scene);
                }
                _ => {}
            },
            Screen::Scene(_) => {
                self.handle_scene_input(key.code)?;
            }
            Screen::Notebook(state) => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => match state.return_to {
                    ReturnTo::Scene => {
                        // Re-render the existing scene by reloading state from the session.
                        let here = self
                            .session
                            .as_ref()
                            .and_then(|s| s.world.npc(NpcId(0)))
                            .and_then(|p| p.location)
                            .unwrap_or(LocationId(0));
                        let scene = self.begin_scene(here, Some(4))?;
                        self.screen = Screen::Scene(scene);
                    }
                    ReturnTo::Ready => {
                        // Should not normally happen — bail to title.
                        self.screen = Screen::Title(TitleState::new(&self.cli));
                    }
                },
                KeyCode::Up | KeyCode::Char('k') => {
                    state.scroll = state.scroll.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    state.scroll = state.scroll.saturating_add(1);
                }
                KeyCode::PageUp => {
                    state.scroll = state.scroll.saturating_sub(10);
                }
                KeyCode::PageDown => {
                    state.scroll = state.scroll.saturating_add(10);
                }
                _ => {}
            },
            Screen::Epilogue(state) => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => self.running = false,
                KeyCode::Char('n') => {
                    let seed = creation_seed(self.cli.seed);
                    self.session = None;
                    self.screen =
                        Screen::Creation(CreationState::new(seed, &self.cli, &self.content));
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    state.scroll = state.scroll.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    state.scroll = state.scroll.saturating_add(1);
                }
                KeyCode::PageUp => {
                    state.scroll = state.scroll.saturating_sub(10);
                }
                KeyCode::PageDown => {
                    state.scroll = state.scroll.saturating_add(10);
                }
                _ => {}
            },
        }
        Ok(())
    }

    fn handle_scene_input(&mut self, code: KeyCode) -> Result<()> {
        // Peel the mode out so we can borrow self mutably below; we'll set it
        // back at the end of the relevant branches.
        let Screen::Scene(scene) = &mut self.screen else {
            return Ok(());
        };
        match std::mem::replace(&mut scene.mode, SceneMode::Browsing) {
            SceneMode::Browsing => match code {
                KeyCode::Char('q') | KeyCode::Esc => self.running = false,
                KeyCode::Up | KeyCode::Char('k') => {
                    if !scene.actions.is_empty() {
                        scene.selected = (scene.selected + scene.actions.len() - 1)
                            % scene.actions.len();
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if !scene.actions.is_empty() {
                        scene.selected = (scene.selected + 1) % scene.actions.len();
                    }
                }
                KeyCode::Enter => {
                    // Hold actions until the scene paragraph finishes streaming.
                    // Lets the player read the opening before they commit.
                    if scene.narration_rx.is_some() {
                        return Ok(());
                    }
                    let action = scene
                        .actions
                        .get(scene.selected)
                        .map(|e| e.action.clone());
                    if let Some(action) = action {
                        self.execute_action(action)?;
                    }
                }
                _ => {}
            },
            SceneMode::DialogueLine {
                npc,
                mut buffer,
            } => match code {
                KeyCode::Esc => { /* drop back to Browsing */ }
                KeyCode::Backspace => {
                    buffer.pop();
                    scene.mode = SceneMode::DialogueLine { npc, buffer };
                }
                KeyCode::Char(c) => {
                    buffer.push(c);
                    scene.mode = SceneMode::DialogueLine { npc, buffer };
                }
                KeyCode::Enter => {
                    if buffer.trim().is_empty() {
                        scene.mode = SceneMode::DialogueLine { npc, buffer };
                    } else {
                        self.submit_dialogue(npc, buffer)?;
                    }
                }
                _ => {
                    scene.mode = SceneMode::DialogueLine { npc, buffer };
                }
            },
            SceneMode::DialogueReply { npc, npc_name, line } => match code {
                KeyCode::Esc => { /* drop back to Browsing */ }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    // Stay with the same NPC so the conversation can continue.
                    scene.mode = SceneMode::DialogueLine {
                        npc,
                        buffer: String::new(),
                    };
                }
                // Any other key: keep the reply on screen instead of silently
                // dropping back to Browsing. (mem::replace at the top of this
                // match swapped Browsing in, so we have to restore.)
                _ => {
                    scene.mode = SceneMode::DialogueReply {
                        npc,
                        npc_name,
                        line,
                    };
                }
            },
            // While the LLM is streaming, all keys are inert — the player has
            // to wait for the line to finish. (Cancelling mid-stream would
            // leave the recorded transcript half-written; not worth it.)
            // Restore the streaming state so the next drain can continue.
            mode @ SceneMode::DialogueStreaming { .. } => {
                scene.mode = mode;
            }
            SceneMode::Accuse { targets, mut selected } => match code {
                KeyCode::Esc => { /* drop back to Browsing */ }
                KeyCode::Up | KeyCode::Char('k') => {
                    if !targets.is_empty() {
                        selected = (selected + targets.len() - 1) % targets.len();
                    }
                    scene.mode = SceneMode::Accuse { targets, selected };
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if !targets.is_empty() {
                        selected = (selected + 1) % targets.len();
                    }
                    scene.mode = SceneMode::Accuse { targets, selected };
                }
                KeyCode::Enter => {
                    if let Some(target) = targets.get(selected).copied() {
                        self.do_accuse(target)?;
                    } else {
                        scene.mode = SceneMode::Accuse { targets, selected };
                    }
                }
                _ => {
                    scene.mode = SceneMode::Accuse { targets, selected };
                }
            },
        }
        Ok(())
    }

    /// Activate New Game from the title menu. The Creation flow starts on the
    /// Setting step, where the user picks the content pack for this playthrough.
    fn start_new_game(&mut self) -> Result<()> {
        let seed = creation_seed(self.cli.seed);
        self.screen = Screen::Creation(CreationState::new(seed, &self.cli, &self.content));
        Ok(())
    }

    /// Scan for save slots and switch to the SaveBrowser screen. If no slots
    /// are found, the title menu stays put — the Continue row is already
    /// rendered as disabled.
    fn enter_save_browser(&mut self) -> Result<()> {
        let parent = saves_parent(&self.cli.save_dir).to_path_buf();
        let slots = storage::list_save_slots(&parent).unwrap_or_default();
        if slots.is_empty() {
            return Ok(());
        }
        self.screen = Screen::SaveBrowser(SaveBrowserState { slots, selected: 0 });
        Ok(())
    }

    /// Build a ConfigState from the current `user_config` and switch to the
    /// Config screen. Edits persist when the user presses Esc.
    fn enter_config_editor(&mut self) {
        let path = self
            .cli
            .config
            .clone()
            .or_else(UserConfig::default_path)
            .unwrap_or_else(|| PathBuf::from("config.toml"));
        self.screen = Screen::Config(ConfigState::from_config(&self.user_config, path));
    }

    /// Open a save slot, rehydrate a Session, and jump straight into Scene at
    /// the player's recorded location. Skips the Ready screen — the player has
    /// already seen the opening when the save was created.
    fn resume_save(&mut self, slot: &SaveSlot) -> Result<()> {
        let save_dir = SaveDir::open(&slot.path)
            .with_context(|| format!("opening save at {}", slot.path.display()))?;

        // Reload content if this save was made under a different setting pack
        // than is currently loaded. The marker is the source of truth — older
        // saves without one keep the current registry.
        if let Some(marker) = save_dir.load_setting_marker() {
            if marker != self.cli.setting {
                let new_reg = ContentRegistry::load(
                    &self.cli.content_root,
                    &marker,
                    &self.cli.vocation,
                )
                .with_context(|| {
                    format!("loading setting {:?} for resumed save", marker)
                })?;
                self.cli.setting = marker;
                self.content = new_reg;
            }
        }

        let world = save_dir
            .load_world()
            .with_context(|| format!("loading world from {}", slot.path.display()))?;
        let incident = save_dir
            .load_case()
            .with_context(|| format!("loading case from {}", slot.path.display()))?;
        let log = EventLog::open(save_dir.event_log_path())
            .with_context(|| "reopening event log for resume")?;

        // Make the resumed save_dir the active one going forward, so later
        // saves and notebook appends land in the same slot.
        self.cli.save_dir = slot.path.clone();

        // Session RNG is derived from world.seed the same way worldgen did, so
        // a resumed playthrough is reproducible from the same seed.
        let session_rng = ChaCha8Rng::seed_from_u64(world.seed.wrapping_add(0xB00D));

        let entry = world
            .npc(NpcId(0))
            .and_then(|p| p.location)
            .unwrap_or(LocationId(0));

        self.session = Some(Session {
            world,
            save_dir,
            log,
            rng: session_rng,
            incident,
            dialogue_history: HashMap::new(),
        });

        tracing::info!(
            slot = %slot.slot_name,
            player = %slot.player_name,
            day = slot.day,
            "resumed save"
        );

        let scene = self.begin_scene(entry, None)?;
        self.screen = Screen::Scene(scene);
        Ok(())
    }

    /// Rebuild the LLM client from the current `user_config` and `cli.dry_run`.
    /// Used after the config editor commits a new api_key.
    fn rebuild_llm_client(&mut self) {
        let llm_cfg =
            llm::LlmConfig::resolve(self.cli.dry_run, self.user_config.api_key.as_deref());
        self.client = llm::make_client(&llm_cfg);
    }

    fn save_player_from_creation(&mut self) -> Result<PlayerCharacter> {
        let Screen::Creation(state) = &self.screen else {
            anyhow::bail!("save_player_from_creation called outside creation");
        };
        let player = state
            .build_player(&self.content.vocation.vocation.name)
            .context("incomplete creation state — should have caught earlier")?;
        let save_dir = SaveDir::open(&self.cli.save_dir)?;
        save_dir.save_player(&player)?;
        tracing::info!(name = %player.name, "player created and saved");
        Ok(player)
    }

    fn run_worldgen(&mut self, player: &PlayerCharacter) -> Result<ReadyState> {
        use crate::engine::worldgen;

        let seed = self.cli.seed.unwrap_or_else(|| {
            use rand::RngCore;
            rand::rngs::OsRng.next_u64()
        });
        tracing::info!(seed, "starting worldgen");
        let result = worldgen::run(&self.content, seed, player)
            .context("worldgen failed")?;

        let save_dir = SaveDir::open(&self.cli.save_dir)?;
        save_dir.save_world(&result.world)?;
        save_dir.save_case(&result.incident)?;
        save_dir.save_setting_marker(&self.cli.setting)?;
        let mut log = EventLog::open(save_dir.event_log_path())?;
        for ev in &result.events {
            log.append(ev)?;
        }
        // Wipe any prior notebook so this run starts clean.
        let _ = std::fs::remove_file(save_dir.notebook_path());
        tracing::info!(
            village = %result.world.village_name,
            npcs = result.world.npcs.len(),
            events = result.events.len(),
            "worldgen complete and saved"
        );

        let entry = result
            .world
            .npc(NpcId(0))
            .and_then(|p| p.location)
            .unwrap_or(LocationId(0));
        let deadline_days = result
            .world
            .deadline_day
            .saturating_sub(result.world.start_day);
        let village_name = result.world.village_name.clone();
        let place_kind = result.world.place_kind.clone();
        let incident_summary = result.incident.summary.clone();
        let player_hook = result.player_hook.clone();

        // Build the session RNG deterministically from the worldgen seed plus a
        // play-loop salt; this keeps scene-tick rolls reproducible alongside
        // the seeded creation flow.
        let session_rng = ChaCha8Rng::seed_from_u64(seed.wrapping_add(0xB00D));
        self.session = Some(Session {
            world: result.world,
            save_dir,
            log,
            rng: session_rng,
            incident: result.incident.clone(),
            dialogue_history: HashMap::new(),
        });

        Ok(ReadyState {
            village_name,
            place_kind,
            incident_summary,
            deadline_days,
            player_hook,
            entry,
        })
    }

    fn begin_scene(
        &mut self,
        here: LocationId,
        keep_events_remaining: Option<u8>,
    ) -> Result<SceneState> {
        let session = self
            .session
            .as_mut()
            .context("begin_scene called without a session")?;
        let world = &session.world;
        let present: Vec<NpcId> = world
            .npcs
            .values()
            .filter(|n| !n.dead && n.id.0 != 0 && n.location == Some(here))
            .map(|n| n.id)
            .collect();
        let actions = available_actions(world, here);
        let mut entries: Vec<ActionEntry> = actions
            .into_iter()
            .map(|a| {
                let label = engine_label(world, &a);
                ActionEntry {
                    action: a,
                    engine_label: label,
                    flavored: None,
                }
            })
            .collect();

        // Narration: cached by (location, mood bucket). On cache miss, start a
        // streaming chat so the paragraph appears live; the main loop drains
        // the receiver and caches the final string when `Done` arrives.
        let player_mood = world
            .npc(NpcId(0))
            .map(|p| p.mood.valence)
            .unwrap_or(0.0);
        let bucket = builders::mood_bucket(player_mood);
        let mut narration_cache: HashMap<(LocationId, u8), String> = HashMap::new();
        let (narration, narration_rx, narration_cache_key) =
            match narration_cache.get(&(here, bucket)).cloned() {
                Some(n) => (n, None, None),
                None => {
                    let model = self.user_config.model_for(CallSite::SceneNarration);
                    let req = builders::scene_open_request(world, &self.content, here, model);
                    match self.client.chat_stream(req) {
                        Ok(rx) => (String::new(), Some(rx), Some((here, bucket))),
                        Err(e) => {
                            tracing::warn!(error = %e, "scene_open stream failed; using fallback narration");
                            let text = fallback_scene_narration(world, here);
                            narration_cache.insert((here, bucket), text.clone());
                            (text, None, None)
                        }
                    }
                }
            };

        // Options: forced-tool call so the model produces exactly one line per
        // engine action, in order. We stream-drain it synchronously here so the
        // scene renders complete on first frame; narration is the slow part
        // and gets its own incremental stream below.
        let mut options_cache: HashMap<u64, Vec<String>> = HashMap::new();
        let labels: Vec<String> = entries.iter().map(|e| e.engine_label.clone()).collect();
        let key = hash_labels(&labels);
        if !labels.is_empty() {
            let acts: Vec<Action> = entries.iter().map(|e| e.action.clone()).collect();
            let options_model = self.user_config.model_for(CallSite::Options);
            let req = builders::options_request(
                &session.world,
                &self.content,
                here,
                &acts,
                options_model,
            );
            let flavored = match llm::chat_blocking_drain(&*self.client, req) {
                Ok(s) if !s.trim().is_empty() => {
                    builders::parse_options_response(&s, labels.len())
                }
                _ => Vec::new(),
            };
            if flavored.len() == labels.len() {
                for (entry, line) in entries.iter_mut().zip(flavored.iter()) {
                    entry.flavored = Some(line.clone());
                }
                options_cache.insert(key, flavored);
            }
        }

        Ok(SceneState {
            here,
            narration,
            present,
            actions: entries,
            selected: 0,
            mode: SceneMode::Browsing,
            events_remaining_today: keep_events_remaining.unwrap_or(4),
            narration_cache,
            options_cache,
            day_banner: None,
            narration_rx,
            narration_cache_key,
        })
    }

    fn execute_action(&mut self, action: Action) -> Result<()> {
        match action {
            Action::OpenNotebook => {
                let body = self
                    .session
                    .as_ref()
                    .map(|s| s.save_dir.notebook_read().unwrap_or_default())
                    .unwrap_or_default();
                self.screen = Screen::Notebook(NotebookState {
                    body,
                    scroll: 0,
                    return_to: ReturnTo::Scene,
                });
            }
            Action::Observe => {
                self.do_observe()?;
                // Refresh the scene in place so flavored options reflect any new state.
                self.refresh_scene_after_inscene_action()?;
            }
            Action::Interview(npc) => {
                if let Screen::Scene(scene) = &mut self.screen {
                    scene.mode = SceneMode::DialogueLine {
                        npc,
                        buffer: String::new(),
                    };
                }
            }
            Action::Move(dest) => {
                self.do_move(dest)?;
            }
            Action::Wait => {
                let here = self
                    .session
                    .as_ref()
                    .and_then(|s| s.world.npc(NpcId(0)))
                    .and_then(|p| p.location)
                    .unwrap_or(LocationId(0));
                self.advance_scene_to(here)?;
            }
            Action::Accuse => {
                let targets = self.accusation_targets();
                if let Screen::Scene(scene) = &mut self.screen {
                    scene.mode = SceneMode::Accuse {
                        targets,
                        selected: 0,
                    };
                }
            }
            Action::LeaveTown => {
                self.close_case(CaseOutcome::LeftTown)?;
            }
        }
        Ok(())
    }

    /// Alive, non-player NPCs the player can name in an accusation. Sorted by
    /// id (i.e. worldgen insertion order), so the menu is stable across rebuilds.
    fn accusation_targets(&self) -> Vec<NpcId> {
        let Some(session) = self.session.as_ref() else {
            return Vec::new();
        };
        let mut ids: Vec<NpcId> = session
            .world
            .npcs
            .values()
            .filter(|n| !n.dead && n.id.0 != 0)
            .map(|n| n.id)
            .collect();
        ids.sort_by_key(|id| id.0);
        ids
    }

    fn do_accuse(&mut self, target: NpcId) -> Result<()> {
        let Some(session) = self.session.as_ref() else {
            return Ok(());
        };
        let correct = session.incident.is_correct_accusation(target);
        let actual = session.incident.primary_culprit();
        let outcome = if correct {
            CaseOutcome::Solved { target }
        } else {
            CaseOutcome::MisAccused { target, actual }
        };
        self.close_case(outcome)
    }

    /// Run the epilogue path: build the LLM request, swap to `Screen::Epilogue`.
    fn close_case(&mut self, outcome: CaseOutcome) -> Result<()> {
        let body = {
            let Some(session) = self.session.as_ref() else {
                return Ok(());
            };
            let model = self.user_config.model_for(CallSite::Epilogue);
            let req = builders::epilogue_request(
                &session.world,
                &self.content,
                &session.incident,
                &outcome,
                model,
            );
            match self.client.chat(req) {
                Ok(s) if !s.trim().is_empty() => s,
                _ => builders::fallback_epilogue(&session.world, &session.incident, &outcome),
            }
        };
        let label = outcome.short_label().to_string();
        tracing::info!(outcome = label.as_str(), "case closed");
        self.screen = Screen::Epilogue(EpilogueState {
            body,
            outcome_label: label,
            scroll: 0,
        });
        Ok(())
    }

    fn refresh_scene_after_inscene_action(&mut self) -> Result<()> {
        let Screen::Scene(scene) = &self.screen else {
            return Ok(());
        };
        let here = scene.here;
        let remaining = scene.events_remaining_today;
        let new_scene = self.begin_scene(here, Some(remaining))?;
        self.screen = Screen::Scene(new_scene);
        Ok(())
    }

    fn do_observe(&mut self) -> Result<()> {
        let Some(session) = self.session.as_mut() else {
            return Ok(());
        };
        let Screen::Scene(scene) = &self.screen else {
            return Ok(());
        };
        let here = scene.here;
        let time = Time {
            day: session.world.day,
            minute: session.world.clock_minutes,
        };
        let here_name = session
            .world
            .location(here)
            .map(|l| l.name.clone())
            .unwrap_or_default();
        let present_names: Vec<String> = session
            .world
            .npcs
            .values()
            .filter(|n| !n.dead && n.id.0 != 0 && n.location == Some(here))
            .map(|n| {
                let occ = n
                    .occupation
                    .clone()
                    .map(|o| format!(" ({})", o))
                    .unwrap_or_default();
                format!("{}{}", n.name, occ)
            })
            .collect();
        let time_label = time_bucket(session.world.clock_minutes);
        let presence = if present_names.is_empty() {
            "no one is here".to_string()
        } else {
            present_names.join(", ")
        };
        let summary = format!(
            "{} in the {}: {}",
            here_name,
            time_label,
            presence,
        );
        let fact_id = session.world.intern_fact(Fact {
            kind: "scene".into(),
            summary: summary.clone(),
        });
        let ev = Event::Witnessed {
            observer: NpcId(0),
            fact: fact_id,
            when: time,
        };
        apply_event(&mut session.world, &ev);
        session.log.append(&ev)?;

        let bullet = format!(
            "- **Day {} / {}, {}.** {}",
            session.world.day,
            time_label,
            here_name,
            presence,
        );
        session.save_dir.notebook_append(&bullet)?;
        Ok(())
    }

    fn do_move(&mut self, dest: LocationId) -> Result<()> {
        if let Some(session) = self.session.as_mut() {
            let time = Time {
                day: session.world.day,
                minute: session.world.clock_minutes,
            };
            let ev = Event::Moved {
                who: NpcId(0),
                to: dest,
                when: time,
            };
            apply_event(&mut session.world, &ev);
            session.log.append(&ev)?;
            // Walking off ends any in-flight conversations.
            session.dialogue_history.clear();
        }
        self.advance_scene_to(dest)?;
        Ok(())
    }

    fn advance_scene_to(&mut self, dest: LocationId) -> Result<()> {
        let Some(session) = self.session.as_mut() else {
            return Ok(());
        };
        let now = Time {
            day: session.world.day,
            minute: session.world.clock_minutes,
        };
        let (events, rolled) = engine::sim::tick_scene(
            &mut session.world,
            &self.content,
            &mut session.rng,
            now,
        );
        for ev in &events {
            session.log.append(ev)?;
        }

        let remaining_before = match &self.screen {
            Screen::Scene(s) => s.events_remaining_today,
            _ => 4,
        };
        let mut remaining_after = remaining_before.saturating_sub(1);
        let mut banner: Option<String> = None;
        if rolled && remaining_after == 0 {
            // Day rollover: reset clock and refill events.
            session.world.clock_minutes = 480; // 08:00
            remaining_after = 4;
            banner = Some(format!("— Day {} —", session.world.day));
            session
                .save_dir
                .notebook_append(&format!("\n## Day {}", session.world.day))?;
        } else if rolled {
            // Day rolled but events_remaining didn't reach 0; this can happen
            // if the player crossed midnight via Wait. Reset events too.
            remaining_after = 4;
            banner = Some(format!("— Day {} —", session.world.day));
            session
                .save_dir
                .notebook_append(&format!("\n## Day {}", session.world.day))?;
        }

        // Case-close checks before re-entering the scene: deadline first, then
        // a defensive player-death guard. Either closes the playthrough.
        if let Some(outcome) = self.case_close_check() {
            return self.close_case(outcome);
        }

        let mut new_scene = self.begin_scene(dest, Some(remaining_after))?;
        new_scene.day_banner = banner;
        self.screen = Screen::Scene(new_scene);
        Ok(())
    }

    /// Returns `Some(outcome)` if the simulation has crossed a case-close
    /// threshold since the last check. Order of precedence: player death,
    /// deadline arrival.
    fn case_close_check(&self) -> Option<CaseOutcome> {
        let session = self.session.as_ref()?;
        if let Some(player) = session.world.npc(NpcId(0)) {
            if player.dead {
                return Some(CaseOutcome::Died);
            }
        }
        if session.world.deadline_day > 0 && session.world.day >= session.world.deadline_day {
            return Some(CaseOutcome::Unsolved);
        }
        None
    }

    /// Begin a dialogue exchange: log the player's line, kick off a streaming
    /// LLM call, and park the scene in `DialogueStreaming`. The main loop
    /// drains the receiver and finalizes the NPC's `Spoken` event on `Done`.
    fn submit_dialogue(&mut self, npc: NpcId, line: String) -> Result<()> {
        let player_line = line.clone();
        let inhabitant = self
            .session
            .as_ref()
            .map(|s| s.world.inhabitant_singular.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "villager".into());
        let npc_name = self
            .session
            .as_ref()
            .and_then(|s| s.world.npc(npc).map(|n| n.name.clone()))
            .unwrap_or_else(|| format!("the {}", inhabitant));

        // 1. Player Spoken event.
        if let Some(session) = self.session.as_mut() {
            let time = Time {
                day: session.world.day,
                minute: session.world.clock_minutes,
            };
            let ev = Event::Spoken {
                speaker: NpcId(0),
                listeners: vec![npc],
                line: player_line.clone(),
                when: time,
            };
            apply_event(&mut session.world, &ev);
            session.log.append(&ev)?;
        }

        // 2. Kick off the streaming LLM call. If the client refuses to start a
        //    stream at all (e.g. worker thread is dead), fall through with a
        //    one-shot pre-filled receiver so the rest of the path stays uniform.
        let rx = {
            let Some(session) = self.session.as_ref() else {
                return Ok(());
            };
            let voice_model = self.user_config.model_for(CallSite::NpcVoice);
            let history = session
                .dialogue_history
                .get(&npc)
                .cloned()
                .unwrap_or_default();
            let req = builders::npc_turn_request(
                &session.world,
                &self.content,
                npc,
                &player_line,
                &history,
                voice_model,
            );
            match self.client.chat_stream(req) {
                Ok(rx) => rx,
                Err(e) => {
                    tracing::warn!(error = %e, "chat_stream refused; falling back to canned line");
                    let (tx, rx) = std::sync::mpsc::channel();
                    // Schema-shaped fallback so `finalize_dialogue_stream`'s
                    // parser still recognizes it and the interpretation layer
                    // gets `Other`/`Neutral` rather than a parse error.
                    let fallback = format!(
                        "{{\"intent\":\"other\",\"tone\":\"neutral\",\"romantic_signal\":null,\
                         \"reply\":\"\\\"I haven't much to say.\\\" — {} keeps their counsel.\"}}",
                        npc_name
                    );
                    let _ = tx.send(llm::StreamChunk::Token(fallback));
                    let _ = tx.send(llm::StreamChunk::Done);
                    rx
                }
            }
        };

        // Record the player's turn in the per-NPC conversation history. The
        // NPC's reply is appended after the stream finalizes.
        if let Some(session) = self.session.as_mut() {
            session
                .dialogue_history
                .entry(npc)
                .or_default()
                .push((true, player_line.clone()));
        }

        // 3. Park the scene in streaming mode. Notebook bullet + NPC Spoken
        //    event are written by `finalize_dialogue_stream` once the stream
        //    finishes, so the recorded line matches what the player actually saw.
        if let Screen::Scene(scene) = &mut self.screen {
            scene.mode = SceneMode::DialogueStreaming {
                npc,
                npc_name,
                player_line,
                buffer: String::new(),
                args_buffer: String::new(),
                reply_streamer: crate::llm::interpret::ReplyTokenStreamer::new(),
                rx,
                had_error: false,
                revealed: 0,
                done: false,
            };
        }
        Ok(())
    }

    /// Drain the streaming receiver (if any) into the scene buffer, advance
    /// the typewriter reveal cursor, and finalize once both the stream is
    /// done AND the reveal cursor has caught up. Returns the number of
    /// updates that should trigger a redraw.
    fn drain_dialogue_stream(&mut self) -> Result<usize> {
        const REVEAL_BYTES_PER_TICK: usize = 6;

        let Screen::Scene(scene) = &mut self.screen else {
            return Ok(0);
        };
        let SceneMode::DialogueStreaming {
            rx,
            buffer,
            args_buffer,
            reply_streamer,
            had_error,
            revealed,
            done,
            ..
        } = &mut scene.mode
        else {
            return Ok(0);
        };
        let mut updates = 0usize;
        let buffer_was = buffer.len();
        loop {
            match rx.try_recv() {
                Ok(llm::StreamChunk::Token(t)) => {
                    // Legacy non-tool stream — append directly to the visible
                    // buffer for backward compatibility.
                    buffer.push_str(&t);
                }
                Ok(llm::StreamChunk::ToolArgs(fragment)) => {
                    args_buffer.push_str(&fragment);
                    let visible = reply_streamer.push(&fragment);
                    if !visible.is_empty() {
                        buffer.push_str(&visible);
                    }
                }
                Ok(llm::StreamChunk::Done) => {
                    *done = true;
                    break;
                }
                Ok(llm::StreamChunk::Error(e)) => {
                    tracing::warn!(error = %e, "stream chunk error");
                    *had_error = true;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    *done = true;
                    break;
                }
            }
        }
        if buffer.len() > buffer_was {
            updates += 1;
        }
        // Advance the reveal cursor by a fixed byte budget, snapped to a UTF-8
        // char boundary so we never slice through a multi-byte sequence.
        if *revealed < buffer.len() {
            let mut target = (*revealed + REVEAL_BYTES_PER_TICK).min(buffer.len());
            while target < buffer.len() && !buffer.is_char_boundary(target) {
                target += 1;
            }
            if target != *revealed {
                *revealed = target;
                updates += 1;
            }
        }
        // Finalize only when the stream is done AND the player has seen all
        // the chars. Otherwise keep the mode alive so the next tick reveals
        // more and redraws.
        if *done && *revealed >= buffer.len() {
            self.finalize_dialogue_stream()?;
        }
        Ok(updates)
    }

    /// True if the scene is currently consuming a streaming LLM response —
    /// either a streaming NPC dialogue reply or the scene-open narration
    /// arriving token-by-token.
    pub fn is_streaming(&self) -> bool {
        let Screen::Scene(scene) = &self.screen else {
            return false;
        };
        matches!(scene.mode, SceneMode::DialogueStreaming { .. }) || scene.narration_rx.is_some()
    }

    /// Drain the scene-open stream, if any, into `scene.narration`. On `Done`,
    /// cache the final string under the recorded `(location, mood)` key so a
    /// re-entry with the same mood reuses the same paragraph.
    fn drain_narration_stream(&mut self) -> Result<()> {
        let Screen::Scene(scene) = &mut self.screen else {
            return Ok(());
        };
        let Some(rx) = scene.narration_rx.as_ref() else {
            return Ok(());
        };
        let mut completed = false;
        let mut hard_error = false;
        loop {
            match rx.try_recv() {
                Ok(llm::StreamChunk::Token(t)) => {
                    scene.narration.push_str(&t);
                }
                Ok(llm::StreamChunk::ToolArgs(_)) => {
                    // Scene-open carries no tool; ignore stray fragments.
                }
                Ok(llm::StreamChunk::Done) => {
                    completed = true;
                    break;
                }
                Ok(llm::StreamChunk::Error(e)) => {
                    tracing::warn!(error = %e, "scene narration stream error");
                    hard_error = true;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    completed = true;
                    break;
                }
            }
        }
        if completed {
            if scene.narration.trim().is_empty() || hard_error {
                // Empty or error → fall back to a deterministic line so the
                // scene isn't blank.
                let session = self
                    .session
                    .as_ref()
                    .map(|s| &s.world);
                if let Some(world) = session {
                    scene.narration = fallback_scene_narration(world, scene.here);
                }
            }
            if let Some(key) = scene.narration_cache_key.take() {
                scene.narration_cache.insert(key, scene.narration.clone());
            }
            scene.narration_rx = None;
        }
        Ok(())
    }

    /// Called when the stream channel emits `Done` (or hangs up). Emits the
    /// NPC's `Spoken` event, appends the notebook bullet, and transitions
    /// back to `DialogueReply` for the existing input-driven dismiss flow.
    fn finalize_dialogue_stream(&mut self) -> Result<()> {
        let Screen::Scene(scene) = &mut self.screen else {
            return Ok(());
        };
        let mode = std::mem::replace(&mut scene.mode, SceneMode::Browsing);
        let SceneMode::DialogueStreaming {
            npc,
            npc_name,
            player_line,
            buffer,
            args_buffer,
            had_error,
            ..
        } = mode
        else {
            // Wasn't actually streaming; restore and leave.
            scene.mode = mode;
            return Ok(());
        };
        if buffer.trim().is_empty() && args_buffer.trim().is_empty() && had_error {
            tracing::warn!("dialogue stream produced no tokens; using fallback line");
        }

        // Source of truth for parsing is the raw tool-args JSON (when
        // present) — the visible `buffer` is just the live-rendered `reply`
        // substring. If tool args are missing (e.g. legacy token-only
        // stream), fall back to parsing the visible buffer.
        let source = if !args_buffer.is_empty() {
            &args_buffer
        } else {
            &buffer
        };

        // Parse the combined response. On any parse failure, fall back to
        // treating the visible buffer as the reply and a default interpretation —
        // the conversation still moves forward even if the model misbehaved.
        let (interpretation, mut response) =
            match crate::llm::interpret::parse_npc_turn_response(source) {
                Ok(parsed) => (parsed.interpretation(), parsed.reply),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        raw = %source.chars().take(200).collect::<String>(),
                        "npc-turn response not parseable as JSON; using raw buffer as reply"
                    );
                    (
                        crate::llm::interpret::PlayerLineInterpretation::default(),
                        buffer.clone(),
                    )
                }
            };
        if response.trim().is_empty() {
            response = format!(
                "\"I haven't much to say.\" — {} keeps their counsel.",
                npc_name
            );
        }
        let response = response.trim().to_string();

        // Apply interpretation, then NPC Spoken event + history append.
        if let Some(session) = self.session.as_mut() {
            let time = Time {
                day: session.world.day,
                minute: session.world.clock_minutes,
            };
            for ev in
                engine::interpret::events_for(&session.world, npc, &player_line, &interpretation, time)
            {
                apply_event(&mut session.world, &ev);
                session.log.append(&ev)?;
            }
            // Asserted facts → Told events. Invalid ids were dropped by the
            // emission helper; we intern the fact, then emit a Told event
            // from the player to the listener so the knowledge graph
            // records what was claimed.
            for application in
                engine::interpret::assertion_emissions(&session.world, npc, &interpretation)
            {
                let fact_id = session.world.intern_fact(application.fact);
                let ev = Event::Told {
                    teller: NpcId(0),
                    listener: npc,
                    fact: fact_id,
                    distortion: application.distortion,
                    when: time,
                };
                apply_event(&mut session.world, &ev);
                session.log.append(&ev)?;
                let _ = application.about; // reserved for future "about-tagged" knowledge queries
            }
            // Bystander witnessing: anyone co-located with the player (except
            // the player and the direct listener) sees the exchange. Insults
            // and flirts in particular ripple into invested bystanders'
            // grievance edges via `bystander_grievance_events`.
            let here = session
                .world
                .npc(NpcId(0))
                .and_then(|p| p.location);
            if let (Some(here), Some(fact)) = (
                here,
                engine::interpret::bystander_fact_summary(&session.world, npc, &interpretation),
            ) {
                let bystanders: Vec<NpcId> = session
                    .world
                    .npcs
                    .values()
                    .filter(|n| {
                        !n.dead
                            && n.id != NpcId(0)
                            && n.id != npc
                            && n.location == Some(here)
                    })
                    .map(|n| n.id)
                    .collect();
                if !bystanders.is_empty() {
                    let fact_id = session.world.intern_fact(fact);
                    for b in &bystanders {
                        let ev = Event::Witnessed {
                            observer: *b,
                            fact: fact_id,
                            when: time,
                        };
                        apply_event(&mut session.world, &ev);
                        session.log.append(&ev)?;
                    }
                    for ev in engine::interpret::bystander_grievance_events(
                        &session.world,
                        npc,
                        &bystanders,
                        &interpretation,
                        time,
                    ) {
                        apply_event(&mut session.world, &ev);
                        session.log.append(&ev)?;
                    }
                }
            }
            let ev = Event::Spoken {
                speaker: npc,
                listeners: vec![NpcId(0)],
                line: response.clone(),
                when: time,
            };
            apply_event(&mut session.world, &ev);
            session.log.append(&ev)?;
            session
                .dialogue_history
                .entry(npc)
                .or_default()
                .push((false, response.clone()));
        }

        // Notebook bullet.
        if let Some(session) = self.session.as_ref() {
            let time_label = time_bucket(session.world.clock_minutes);
            let here_name = session
                .world
                .npc(NpcId(0))
                .and_then(|p| p.location)
                .and_then(|l| session.world.location(l))
                .map(|l| l.name.clone())
                .unwrap_or_default();
            let bullet = format!(
                "- **Day {} / {}, {}.** I say to {}: \"{}\"\n  - {}: {}",
                session.world.day,
                time_label,
                here_name,
                npc_name,
                player_line,
                npc_name,
                response,
            );
            session.save_dir.notebook_append(&bullet)?;
        }

        if let Screen::Scene(scene) = &mut self.screen {
            scene.mode = SceneMode::DialogueReply {
                npc,
                npc_name,
                line: response,
            };
        }
        Ok(())
    }
}

fn fallback_scene_narration(world: &World, here: LocationId) -> String {
    let name = world
        .location(here)
        .map(|l| l.name.clone())
        .unwrap_or_else(|| "the place".into());
    format!("You arrive at {}. The world hums on around you.", name)
}

fn hash_labels(labels: &[String]) -> u64 {
    let mut hasher = DefaultHasher::new();
    labels.hash(&mut hasher);
    hasher.finish()
}

fn time_bucket(minutes: u32) -> &'static str {
    let h = minutes / 60;
    match h {
        4..=10 => "morning",
        11..=13 => "noon",
        14..=17 => "afternoon",
        18..=21 => "evening",
        _ => "night",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::ContentRegistry;
    use std::path::PathBuf;

    fn load_registry() -> ContentRegistry {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("content");
        ContentRegistry::load(&root, "medieval", "investigator").unwrap()
    }

    fn test_cli() -> Cli {
        Cli {
            seed: None,
            save_dir: PathBuf::from("saves/test"),
            dry_run: true,
            log_level: tracing::Level::INFO,
            content_root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("content"),
            setting: "medieval".into(),
            vocation: "investigator".into(),
            config: None,
        }
    }

    #[test]
    fn walks_to_a_complete_player() {
        let registry = load_registry();
        let cli = test_cli();
        let mut state = CreationState::new(42, &cli, &registry);

        assert_eq!(state.step, CreationStep::Setting);
        state.confirm(&registry);
        assert_eq!(state.step, CreationStep::Virtue);
        state.confirm(&registry);
        assert_eq!(state.step, CreationStep::Vice);
        state.confirm(&registry);
        assert_eq!(state.step, CreationStep::Inclination);
        state.confirm(&registry);
        assert_eq!(state.step, CreationStep::Background);
        state.confirm(&registry);
        assert_eq!(state.step, CreationStep::Name);
        state.confirm(&registry);
        assert_eq!(state.step, CreationStep::Confirm);

        let player = state
            .build_player(&registry.vocation.vocation.name)
            .unwrap();
        assert_eq!(player.vocation, "investigator");
        assert_eq!(player.virtue, registry.setting.traits.virtues[0].name);
        assert_eq!(player.vice, registry.setting.traits.vices[0].name);
        assert_eq!(player.inclination, registry.setting.traits.inclinations[0].name);
        assert_eq!(
            player.background,
            registry.vocation.backgrounds.backgrounds[0].name
        );
        assert!(!player.name.is_empty());
        assert!(player.name.contains(' '));
    }

    #[test]
    fn back_from_setting_returns_to_title_signal() {
        let registry = load_registry();
        let cli = test_cli();
        let mut state = CreationState::new(1, &cli, &registry);
        assert_eq!(state.step, CreationStep::Setting);
        assert!(state.back(), "back on Setting should signal return to title");
    }

    #[test]
    fn back_from_virtue_returns_to_setting() {
        let registry = load_registry();
        let cli = test_cli();
        let mut state = CreationState::new(1, &cli, &registry);
        state.confirm(&registry); // Setting -> Virtue
        assert_eq!(state.step, CreationStep::Virtue);
        assert!(
            !state.back(),
            "back on Virtue should NOT signal return to title"
        );
        assert_eq!(state.step, CreationStep::Setting);
    }

    #[test]
    fn back_clears_current_selection() {
        let registry = load_registry();
        let cli = test_cli();
        let mut state = CreationState::new(1, &cli, &registry);
        state.confirm(&registry); // Setting -> Virtue
        state.selected_index = 2;
        state.confirm(&registry); // Virtue -> Vice
        assert_eq!(state.step, CreationStep::Vice);
        assert!(state.virtue.is_some());
        state.back();
        assert_eq!(state.step, CreationStep::Virtue);
        assert!(state.virtue.is_none());
        assert_eq!(state.selected_index, 0);
    }

    #[test]
    fn reroll_with_same_seed_produces_same_first_name() {
        let registry = load_registry();
        let cli = test_cli();
        let s1 = CreationState::new(99, &cli, &registry);
        let s2 = CreationState::new(99, &cli, &registry);
        assert_eq!(s1.rolled_name, s2.rolled_name);
    }

    #[test]
    fn navigation_wraps() {
        let registry = load_registry();
        let cli = test_cli();
        let mut state = CreationState::new(1, &cli, &registry);
        // Skip past the Setting step (its option count depends on filesystem
        // state) to land on Virtue, which is deterministic.
        state.confirm(&registry);
        assert_eq!(state.step, CreationStep::Virtue);
        let n = state.option_count(&registry);
        assert!(n > 1);
        state.move_up(&registry);
        assert_eq!(state.selected_index, n - 1);
        state.move_down(&registry);
        assert_eq!(state.selected_index, 0);
    }

    #[test]
    fn setting_step_stages_chosen_pack() {
        let registry = load_registry();
        let cli = test_cli();
        let mut state = CreationState::new(1, &cli, &registry);
        // Pre-condition: there is at least one available setting and Setting is
        // the initial step.
        assert!(!state.available_settings.is_empty());
        assert_eq!(state.step, CreationStep::Setting);
        let pick = state.available_settings[0].clone();
        state.selected_index = 0;
        state.confirm(&registry);
        assert_eq!(state.staged_setting, pick);
        assert_eq!(state.step, CreationStep::Virtue);
    }

    #[test]
    fn completed_creation_round_trips_through_save() {
        use tempfile::TempDir;
        let registry = load_registry();
        let cli = test_cli();
        let mut state = CreationState::new(7, &cli, &registry);

        state.confirm(&registry); // Setting -> Virtue
        state.selected_index = 2;
        state.confirm(&registry); // Virtue
        state.selected_index = 1;
        state.confirm(&registry); // Vice
        state.selected_index = 3;
        state.confirm(&registry); // Inclination
        state.selected_index = 0;
        state.confirm(&registry); // Background
        state.confirm(&registry); // Name -> Confirm

        let player = state
            .build_player(&registry.vocation.vocation.name)
            .unwrap();

        let dir = TempDir::new().unwrap();
        let save = SaveDir::open(dir.path()).unwrap();
        save.save_player(&player).unwrap();
        let back = save.load_player().unwrap();
        assert_eq!(player, back);
    }

    #[test]
    fn parse_options_response_picks_exact_count() {
        let raw = r#"{"lines":["think a moment","step outside"]}"#;
        let parsed = crate::llm::builders::parse_options_response(raw, 2);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], "think a moment");
        assert_eq!(parsed[1], "step outside");
    }

    #[test]
    fn config_state_round_trips_through_drafts() {
        let cfg = UserConfig {
            api_key: Some("sk-test".into()),
            default_model: Some("acme/base".into()),
            models: crate::config::ModelOverrides {
                scene_narration: Some("acme/narrate".into()),
                options: Some("acme/options".into()),
                npc_voice: Some("acme/voice".into()),
                epilogue: Some("acme/epi".into()),
            },
        };
        let path = std::path::PathBuf::from("/tmp/loom-config-test.toml");
        let state = ConfigState::from_config(&cfg, path.clone());
        assert_eq!(state.fields.len(), 6);
        assert_eq!(state.drafts[0], "sk-test");
        assert_eq!(state.drafts[1], "acme/base");

        let back = state.build_config();
        assert_eq!(back.api_key.as_deref(), Some("sk-test"));
        assert_eq!(back.default_model.as_deref(), Some("acme/base"));
        assert_eq!(back.models.scene_narration.as_deref(), Some("acme/narrate"));
        assert_eq!(back.models.options.as_deref(), Some("acme/options"));
        assert_eq!(back.models.npc_voice.as_deref(), Some("acme/voice"));
        assert_eq!(back.models.epilogue.as_deref(), Some("acme/epi"));
    }

    #[test]
    fn empty_draft_strings_clear_their_config_field() {
        let cfg = UserConfig {
            api_key: Some("old".into()),
            default_model: Some("old/model".into()),
            ..Default::default()
        };
        let mut state = ConfigState::from_config(&cfg, std::path::PathBuf::from("/tmp/x.toml"));
        state.drafts[0].clear();
        state.drafts[1] = "   ".to_string();
        let back = state.build_config();
        assert!(back.api_key.is_none(), "blank draft clears api_key");
        assert!(
            back.default_model.is_none(),
            "whitespace-only draft clears default_model"
        );
    }
}

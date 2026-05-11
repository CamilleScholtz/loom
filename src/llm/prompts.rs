use crate::content::ContentRegistry;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKey {
    NpcVoice,
    SceneOpen,
    Options,
    Epilogue,
}

impl PromptKey {
    pub fn file_stem(self) -> &'static str {
        match self {
            PromptKey::NpcVoice => "npc_voice",
            PromptKey::SceneOpen => "scene_open",
            PromptKey::Options => "options",
            PromptKey::Epilogue => "epilogue",
        }
    }
}

pub fn lookup<'a>(registry: &'a ContentRegistry, key: PromptKey) -> Option<&'a str> {
    registry.setting.prompts.get(key.file_stem())
}

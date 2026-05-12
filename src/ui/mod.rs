pub mod commands;
pub mod config;
pub mod creation;
pub mod epilogue;
pub mod notebook;
pub mod palette;
pub mod ready;
pub mod save_browser;
pub mod scene;
pub mod text;
pub mod title;

use ratatui::Frame;

use crate::app::{App, Screen};

pub fn render(frame: &mut Frame, app: &App) {
    match &app.screen {
        Screen::Title(state) => title::render(frame, state, app),
        Screen::SaveBrowser(state) => save_browser::render(frame, state),
        Screen::Config(state) => config::render(frame, state),
        Screen::Creation(state) => creation::render(frame, state, &app.content),
        Screen::Ready(state) => ready::render(frame, state),
        Screen::Scene(state) => scene::render(frame, app, state),
        Screen::Notebook(state) => notebook::render(frame, state),
        Screen::Epilogue(state) => epilogue::render(frame, state),
    }
}

use crate::app::AppState;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io::{self};
use std::sync::Arc;

pub mod auth;
pub mod chat;
pub mod home;
pub mod themes;

// funny
// BUT BEHIND THIS SIMPLICITY
// HIDE A BUNCH OF SPAGHETI CODE
// AND DON4T CHECK THE CODE FOR THE CHAT PAGE
// BC ITS JUST MORE PSAGHETTI CODE THAT NEED REFACTORS

#[derive(Debug, PartialEq, Eq)]
pub enum TuiPage {
    Auth,
    Home,
    Chat,
    Exit,
}
pub async fn run_tui(app_state: Arc<tokio::sync::Mutex<AppState>>) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut current_page = TuiPage::Auth;

    loop {
        match current_page {
            TuiPage::Auth => {
                current_page = auth::run_auth_page(&mut terminal, app_state.clone()).await?;
            }
            TuiPage::Home => {
                current_page = home::run_home_page(&mut terminal, app_state.clone()).await?;
            }
            TuiPage::Chat => {
                current_page = chat::run_chat_page(&mut terminal, app_state.clone()).await?.unwrap_or(TuiPage::Exit);
            }
            TuiPage::Exit => break,
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

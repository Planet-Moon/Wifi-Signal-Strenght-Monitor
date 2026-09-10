mod app;
mod event;
mod wifi;

use std::io;
use std::thread;

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;

use crate::app::App;

pub fn main() -> color_eyre::eyre::Result<()> {
    color_eyre::install()?;
    let mut terminal = ratatui::init();
    execute!(io::stdout(), EnableMouseCapture)?;
    let mut app = App::new();

    thread::spawn({
        let tx = app.event_sender();
        move || {
            loop {
                let _ = event::handle_input_events(&tx);
            }
        }
    });

    let app_result = app.run(&mut terminal);

    let _ = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
    app_result
}

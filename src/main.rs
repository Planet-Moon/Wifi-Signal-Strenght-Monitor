mod app;
mod event;
mod wifi;

use std::thread;

use crate::app::App;

#[tokio::main]
pub async fn main() -> color_eyre::eyre::Result<()> {
    color_eyre::install()?;
    let mut terminal = ratatui::init();
    let mut app = App::new();

    thread::spawn({
        let tx = app.event_sender();
        move || {
            loop {
                let _ = event::handle_input_events(&tx);
            }
        }
    });

    let app_result = app.run(&mut terminal).await;

    ratatui::restore();
    app_result
}

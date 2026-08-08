use std::sync::mpsc;

use color_eyre::eyre::WrapErr;
use crossterm::event::{self, Event as CrosstermEvent, KeyCode, KeyEventKind};

use crate::app::WifiScanResult;

#[derive(Debug)]
pub enum Event {
    Quit,
    SelectPrev,
    SelectNext,
    WifiScanned(WifiScanResult),
}

fn handle_key_event(
    key_event: crossterm::event::KeyEvent,
    tx: &mpsc::Sender<Event>,
) -> color_eyre::Result<()> {
    match key_event.code {
        KeyCode::Char('q') => tx.send(Event::Quit)?,
        KeyCode::Left => todo!(),
        KeyCode::Right => todo!(),
        KeyCode::Up => tx.send(Event::SelectPrev)?,
        KeyCode::Down => tx.send(Event::SelectNext)?,
        _ => {}
    }
    Ok(())
}

pub fn handle_input_events(tx: &mpsc::Sender<Event>) -> color_eyre::Result<()> {
    match event::read()? {
        // it's important to check that the event is a key press event as
        // crossterm also emits key release and repeat events on Windows.
        CrosstermEvent::Key(key_event) => {
            if key_event.kind == KeyEventKind::Press {
                handle_key_event(key_event, tx)
                    .wrap_err_with(|| format!("handling key event failed:\n{key_event:#?}"))
            } else {
                Ok(())
            }
        }
        _ => Ok(()),
    }
}

use std::sync::mpsc;

use color_eyre::eyre::WrapErr;
use crossterm::event::{
    self, Event as CrosstermEvent, KeyCode, KeyEventKind, MouseButton, MouseEventKind,
};

use crate::app::FastStatus;
use crate::wifi::ConnectedAp;

#[derive(Debug)]
pub enum Event {
    Quit,
    /// Redraw pulse, so the UI stays live while no sample is arriving.
    Tick,
    /// The AP the adapter is associated with changed (including becoming connected/disconnected).
    FastTarget {
        target: Option<ConnectedAp>,
        generation: u64,
    },
    /// A live RSSI reading. `generation` identifies the target it was taken for.
    FastSample {
        generation: u64,
        dbm: i32,
    },
    FastStatus(FastStatus),
    CycleWindow,
    /// A left-click, in terminal cell coordinates. The app hit-tests this against clickable areas
    /// (e.g. the keybind legend) since crossterm reports raw coordinates, not widget targets.
    Click { x: u16, y: u16 },
}

fn handle_key_event(
    key_event: crossterm::event::KeyEvent,
    tx: &mpsc::Sender<Event>,
) -> color_eyre::Result<()> {
    match key_event.code {
        KeyCode::Char('q') | KeyCode::Char('Q') => tx.send(Event::Quit)?,
        KeyCode::Char('w') | KeyCode::Char('W') => tx.send(Event::CycleWindow)?,
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
        CrosstermEvent::Mouse(mouse_event) => {
            if mouse_event.kind == MouseEventKind::Down(MouseButton::Left) {
                tx.send(Event::Click {
                    x: mouse_event.column,
                    y: mouse_event.row,
                })?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

use std::sync::mpsc;
use std::thread::{self, sleep};
use std::time::{Duration, SystemTime};

use ratatui::prelude::Stylize;
use ratatui::style::{Color, Modifier};
use ratatui::widgets::{Block, List, ListState};
use ratatui::{DefaultTerminal, Frame, text::Line, widgets::Widget};
use wifi_scan::Wifi;

use crate::event::Event;

#[derive(Debug)]
pub struct WifiScanResult {
    timestamp: std::time::SystemTime,
    wifi: Vec<Wifi>,
}

#[derive(Debug)]
pub struct App {
    exit: bool,
    event_rx: mpsc::Receiver<Event>,
    event_tx: mpsc::Sender<Event>,
    list_state: ListState,
    detected_wifis: WifiScanResult,
}

impl App {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<Event>();
        Self {
            exit: false,
            event_rx: rx,
            event_tx: tx,
            list_state: ListState::default().with_selected(Some(0)),
            detected_wifis: WifiScanResult {
                timestamp: SystemTime::now(),
                wifi: Vec::new(),
            },
        }
    }

    pub fn event_sender(&self) -> mpsc::Sender<Event> {
        self.event_tx.clone()
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> color_eyre::eyre::Result<()> {
        thread::spawn({
            let event_tx = self.event_tx.clone();
            move || {
                loop {
                    event_tx
                        .send(Event::WifiScanned(WifiScanResult {
                            timestamp: SystemTime::now(),
                            wifi: wifi_scan::scan().unwrap_or(vec![]),
                        }))
                        .unwrap();
                    sleep(Duration::from_millis(200));
                }
            }
        });
        while !self.exit {
            terminal.draw(|f| self.draw(f))?;
            if let Ok(event) = self.event_rx.recv() {
                if let Some(new) = self.handle_event(event) {
                    new.into_iter().for_each(|e| self.event_tx.send(e).unwrap());
                }
            }
        }
        Ok(())
    }

    fn handle_event(&mut self, event: Event) -> Option<Vec<Event>> {
        match event {
            Event::Quit => {
                self.exit = true;
                None
            }
            Event::SelectPrev => {
                self.list_state.select_previous();
                None
            }
            Event::SelectNext => {
                self.list_state.select_next();
                None
            }
            Event::WifiScanned(v) => {
                self.detected_wifis = v;
                None
            }
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let block = Block::bordered().title("Bordered block");
        let mut items = self
            .detected_wifis
            .wifi
            .iter()
            .map(|e| format!("{}", e.ssid))
            .collect::<Vec<String>>();
        let ts = time_format::from_system_time(self.detected_wifis.timestamp).unwrap();
        let formatted_time = time_format::format_iso8601_local(ts).unwrap();
        items.push(format!("{formatted_time}"));
        let list = List::new(items)
            .style(Color::White)
            .highlight_style(Modifier::REVERSED)
            .highlight_symbol("> ");
        frame.render_widget(block, frame.area());
        frame.render_stateful_widget(list, frame.area(), &mut self.list_state);
    }
}

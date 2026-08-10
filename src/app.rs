use std::sync::mpsc;
use std::thread::{self, sleep};
use std::time::{Duration, SystemTime};

use ratatui::layout::{self, Constraint, Direction, Layout};
use ratatui::prelude::Stylize;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListState, Row, Table, TableState};
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
    table_state: TableState,
    detected_wifis: WifiScanResult,
}

impl App {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<Event>();
        Self {
            exit: false,
            event_rx: rx,
            event_tx: tx,
            table_state: TableState::default().with_selected(Some(0)),
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
                self.table_state.select_previous();
                None
            }
            Event::SelectNext => {
                self.table_state.select_next();
                None
            }
            Event::WifiScanned(v) => {
                self.detected_wifis = v;
                None
            }
            Event::SelectColPrev => {
                self.table_state.select_previous_column();
                None
            }
            Event::SelectColNext => {
                self.table_state.select_next_column();
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

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(frame.area());
        let table_ = layout[0];
        let chart_ = layout[1];

        let table_rows = [Row::new(vec!["SSID", "MAC", "Channel", "Signal strength"])];
        // pub struct Wifi {
        //     /// MAC Address. May be empty on macOS.
        //     pub mac: String,
        //     /// Hotspot Name. May be empty on macOS.
        //     pub ssid: String,
        //     /// Channel the hotspot is on. Returns 0 if unknown.
        //     pub channel: u32,
        //     /// Wifi signal strength in dBm. Returns 0 if unknown.
        //     pub signal_level: i32,
        //     /// A list of all supported securities by the network
        //     pub security: Vec<WifiSecurity>,
        // }

        let table_rows = self.detected_wifis.wifi.iter().map(|e| {
            Row::new(vec![
                format!("{}", e.ssid),
                format!("{}", e.mac),
                format!("{}", e.channel),
                format!("{}", e.signal_level),
            ])
        });
        let widths = [
            Constraint::Min(30),
            Constraint::Min(11),
            Constraint::Min(10),
            Constraint::Min(16),
        ];

        let table = Table::new(table_rows, widths)
            .column_spacing(1)
            .style(Style::new().blue())
            .header(
                Row::new(vec!["SSID", "MAC", "Channel", "Signal strength"])
                    .style(Style::new().bold())
                    .bottom_margin(1),
            )
            .footer(Row::new(vec![format!("Updated on {}", formatted_time)]))
            .block(
                Block::new()
                    .title("Wifi networks nearby")
                    .borders(Borders::ALL),
            )
            .row_highlight_style(Style::new().reversed())
            .column_highlight_style(Style::new().red())
            .cell_highlight_style(Style::new().blue())
            .highlight_symbol(">>");
        frame.render_widget(block, chart_);
        frame.render_stateful_widget(table, table_, &mut self.table_state);
    }
}

use std::sync::mpsc;
use std::thread::{self, sleep};
use std::time::{Duration, SystemTime};

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders, Row, Table, TableState};
use ratatui::{DefaultTerminal, Frame};
use wifi_scan::Wifi;

use crate::event::Event;

#[derive(Debug)]
pub struct WifiScanResult {
    timestamp: std::time::SystemTime,
    wifi: Result<Vec<Wifi>, String>,
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
                wifi: Ok(Vec::new()),
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
                    let wifi_scan_result = wifi_scan::scan();
                    event_tx
                        .send(Event::WifiScanned(WifiScanResult {
                            timestamp: SystemTime::now(),
                            wifi: wifi_scan_result.map_err(|e| e.to_string()),
                        }))
                        .unwrap();
                    sleep(Duration::from_millis(200));
                }
            }
        });
        while !self.exit {
            terminal.draw(|f| self.draw(f))?;
            if let Ok(event) = self.event_rx.recv()
                && let Some(new) = self.handle_event(event)
            {
                new.into_iter().for_each(|e| self.event_tx.send(e).unwrap());
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
        let block = Block::bordered()
            .title("Bordered block")
            .title_bottom("Title bottom");
        let mut items = match &self.detected_wifis.wifi {
            Ok(wifi) => wifi
                .iter()
                .map(|e| e.ssid.to_string())
                .collect::<Vec<String>>(),
            Err(e) => {
                vec![e.clone()]
            }
        };

        let ts = time_format::from_system_time(self.detected_wifis.timestamp).unwrap();
        let formatted_time = time_format::format_iso8601_local(ts).unwrap();
        items.push(formatted_time.to_string());

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(frame.area());
        let table_ = layout[0];
        let chart_ = layout[1];

        let table_rows: Vec<Row> = match &self.detected_wifis.wifi {
            Ok(wifi) => {
                let mut w = wifi.clone();
                w.sort_by_key(|b| std::cmp::Reverse(b.signal_level));
                w.into_iter()
                    .map(|e| {
                        Row::new(vec![
                            e.ssid.to_string(),
                            e.mac.to_string(),
                            e.channel.to_string(),
                            e.signal_level.to_string(),
                        ])
                    })
                    .collect::<Vec<Row>>()
            }
            Err(_) => Vec::new(),
        };
        let widths = [
            Constraint::Min(30),
            Constraint::Min(11),
            Constraint::Min(10),
            Constraint::Min(16),
        ];

        let footer_str = match &self.detected_wifis.wifi {
            Ok(_) => format!("Updated on {}", formatted_time),
            Err(e) => format!("Updated on {} | {}", formatted_time, e),
        };
        let table = Table::new(table_rows, widths)
            .column_spacing(1)
            .style(Style::new().blue())
            .header(
                Row::new(vec!["SSID", "MAC", "Channel", "Signal strength"])
                    .style(Style::new().bold())
                    .bottom_margin(1),
            )
            .footer(Row::new(vec![footer_str]))
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

use std::collections::HashMap;
use std::sync::mpsc;
use std::thread::{self, sleep};
use std::time::{Duration, SystemTime};

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Style, Stylize};
use ratatui::widgets::{
    Axis, Block, Borders, Chart, Dataset, GraphType, Paragraph, Row, Table, TableState, Wrap,
};
use ratatui::{DefaultTerminal, Frame, symbols};
use wifi_scan::Wifi;

use crate::event::Event;

#[derive(Debug, Clone)]
struct Datapoint {
    timestamp: Duration,
    signal_strength: Option<i32>,
}

impl Datapoint {
    fn new(time: Duration, value: Option<i32>) -> Self {
        Self {
            timestamp: time,
            signal_strength: value,
        }
    }
}

impl From<Datapoint> for (f64, f64) {
    fn from(value: Datapoint) -> Self {
        (
            value.timestamp.as_secs_f64(),
            if let Some(value) = value.signal_strength {
                value as f64
            } else {
                f64::NAN
            },
        )
    }
}

#[derive(Debug)]
struct DataPointContainer {
    data: Vec<Datapoint>,
}

impl DataPointContainer {
    fn get_limits_x(&self) -> Option<[f64; 2]> {
        match self.data.len() {
            0 | 1 => None,
            _ => Some([
                self.data.first().unwrap().timestamp.as_secs_f64(),
                self.data.last().unwrap().timestamp.as_secs_f64(),
            ]),
        }
    }

    fn get_limits_y(&self) -> Option<[f64; 2]> {
        match self.data.len() {
            0 | 1 => None,
            _ => {
                let mut min = self.data.first().unwrap().signal_strength;
                let mut max = min;
                for i in &self.data {
                    if i.signal_strength < min {
                        min = i.signal_strength;
                    } else if i.signal_strength > max {
                        max = i.signal_strength;
                    }
                }
                match (max, min) {
                    (Some(max), Some(min)) => Some([min as f64, max as f64]),
                    _ => None,
                }
            }
        }
    }

    fn push(&mut self, datapoint: Datapoint) {
        self.data.push(datapoint);
    }
}

impl<'a> DataPointContainer {
    fn get_data(&'a self) -> &'a Vec<Datapoint> {
        &self.data
    }
}

#[derive(Debug)]
pub struct WifiScanResult {
    timestamp: Duration,
    wifi: Result<Vec<Wifi>, String>,
}

#[derive(Debug, Eq, PartialEq, Hash)]
struct WifiInfo {
    ssid: String,
    mac: String,
    channel: u32,
}

impl From<&Wifi> for WifiInfo {
    fn from(value: &Wifi) -> Self {
        Self {
            ssid: value.ssid.clone(),
            mac: value.mac.clone(),
            channel: value.channel,
        }
    }
}

#[derive(Debug)]
pub struct App {
    exit: bool,
    event_rx: mpsc::Receiver<Event>,
    event_tx: mpsc::Sender<Event>,
    time_reference: SystemTime,
    table_state: TableState,
    detected_wifis: WifiScanResult,
    history: HashMap<WifiInfo, DataPointContainer>,
}

impl App {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<Event>();
        Self {
            exit: false,
            event_rx: rx,
            event_tx: tx,
            time_reference: SystemTime::now(),
            table_state: TableState::default(),
            detected_wifis: WifiScanResult {
                timestamp: Duration::ZERO,
                wifi: Ok(Vec::new()),
            },
            history: HashMap::new(),
        }
    }

    pub fn event_sender(&self) -> mpsc::Sender<Event> {
        self.event_tx.clone()
    }

    fn start_threads(&self) {
        thread::spawn({
            let event_tx = self.event_tx.clone();
            let time_reference = self.time_reference;
            move || {
                loop {
                    let wifi_scan_result = wifi_scan::scan();
                    event_tx
                        .send(Event::WifiScanned(WifiScanResult {
                            timestamp: time_reference.elapsed().unwrap(),
                            wifi: wifi_scan_result.map_err(|e| e.to_string()),
                        }))
                        .unwrap();
                    sleep(Duration::from_millis(200));
                }
            }
        });
    }

    fn run_once(&mut self) -> color_eyre::eyre::Result<()> {
        if let Ok(event) = self.event_rx.recv()
            && let Some(new) = self.handle_event(event)
        {
            new.into_iter().for_each(|e| self.event_tx.send(e).unwrap());
        }
        Ok(())
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> color_eyre::eyre::Result<()> {
        self.start_threads();
        while !self.exit {
            terminal.draw(|f| self.draw(f))?;
            self.run_once()?;
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
                if let Ok(wifi) = &mut self.detected_wifis.wifi {
                    wifi.sort_by_key(|b| std::cmp::Reverse(b.signal_level));
                }

                if let Ok(wifi) = &self.detected_wifis.wifi {
                    wifi.iter().for_each(|w| {
                        let dp = Datapoint::new(
                            self.time_reference.elapsed().unwrap(),
                            Some(w.signal_level),
                        );
                        self.history
                            .entry(w.into())
                            .and_modify(|value| value.push(dp.clone()))
                            .or_insert(DataPointContainer { data: vec![dp] });
                    });
                }
                let longest_history_n = self.history.values().map(|v| v.data.len()).max();
                if let Some(max_n) = longest_history_n {
                    self.history.values_mut().for_each(|v| {
                        if v.data.len() < max_n {
                            v.data.push(Datapoint {
                                timestamp: self.time_reference.elapsed().unwrap(),
                                signal_strength: None,
                            });
                        }
                    });
                    assert!(self.history.values().all(|v| v.data.len() == max_n));
                }
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
        let mut items = match &self.detected_wifis.wifi {
            Ok(wifi) => wifi
                .iter()
                .map(|e| e.ssid.to_string())
                .collect::<Vec<String>>(),
            Err(e) => {
                vec![e.clone()]
            }
        };

        let ts = time_format::from_system_time(self.time_reference + self.detected_wifis.timestamp)
            .unwrap();
        let formatted_time = time_format::format_iso8601_local(ts).unwrap();
        items.push(formatted_time.to_string());

        let [table_area, chart_area, current_charted_area, debug_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(50),
                Constraint::Percentage(50),
                Constraint::Min(1),
                Constraint::Min(5),
            ])
            .areas(frame.area());

        let table_rows: Vec<Row> = match &self.detected_wifis.wifi {
            Ok(wifi) => wifi
                .iter()
                .map(|e| {
                    Row::new(vec![
                        e.ssid.to_string(),
                        e.mac.to_string(),
                        e.channel.to_string(),
                        e.signal_level.to_string(),
                    ])
                })
                .collect::<Vec<Row>>(),
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
        let table = Table::new(table_rows.clone(), widths)
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
        if self.table_state.selected().is_none() {
            self.table_state.select(Some(0));
        }
        frame.render_stateful_widget(table, table_area, &mut self.table_state);

        let selected_wifi = if let Some(s) = self.table_state.selected()
            && let Ok(wifi) = &self.detected_wifis.wifi
        {
            wifi.get(s)
        } else {
            None
        };

        let history_element = match &selected_wifi {
            Some(wifi) => self.history.get(&(*wifi).into()),
            None => None,
        };
        let data = match history_element {
            Some(element) => element
                .get_data()
                .clone()
                .into_iter()
                .map(|p| p.into())
                .collect::<Vec<(f64, f64)>>(),
            None => Vec::new(),
        };
        let datasets = vec![
            Dataset::default()
                .name("Wifi signal strengh")
                .marker(symbols::Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().magenta())
                .data(&data),
        ];
        let x_bounds = history_element
            .and_then(|el| el.get_limits_x())
            .unwrap_or([0.0, 10.0]);
        let x_labels = x_bounds
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<String>>();
        let x_axis = Axis::default()
            .title("Time".red())
            .style(Style::default().white())
            .bounds(x_bounds)
            .labels(x_labels);

        // Create the Y axis and define its properties
        let y_bounds = history_element
            .and_then(|el| el.get_limits_y())
            .unwrap_or([0.0, 10.0]);
        let y_labels = y_bounds
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<String>>();
        let y_axis = Axis::default()
            .title("Signal strength dBm".red())
            .style(Style::default().white())
            // .bounds([-100.0, 0.0])
            // .labels(["-100.0", "-50.0", "0.0"]);
            .bounds(y_bounds)
            .labels(y_labels);

        let chart = Chart::new(datasets)
            .block(Block::new().title("Chart"))
            .x_axis(x_axis)
            .y_axis(y_axis);

        frame.render_widget(chart, chart_area);

        frame.render_widget(
            Paragraph::new(format!(
                "{}, {}",
                selected_wifi.map(|f| f.ssid.clone()).unwrap_or_default(),
                data.len()
            ))
            .bold()
            .cyan()
            .centered(),
            current_charted_area,
        );

        let debug_values = selected_wifi.and_then(|wifi| {
            self.history.get(&wifi.into()).map(|d| {
                d.data
                    .iter()
                    .map(|i| i.signal_strength.map(|v| v.to_string()).unwrap_or_default())
                    .collect::<Vec<String>>()
            })
        });
        frame.render_widget(
            Paragraph::new(format!("{:?}", debug_values))
                .yellow()
                .italic()
                .wrap(Wrap { trim: true }),
            debug_area,
        );
    }
}

#[cfg(test)]
mod app_test {
    use super::*;

    #[test]
    fn run_test() {
        let mut app = App::new();
        let _event_tx = app.event_sender();
        app.start_threads();
        loop {
            println!("{:?}", app.history);
            let _ = app.run_once();
            sleep(Duration::from_secs_f32(0.1));
        }
    }
}

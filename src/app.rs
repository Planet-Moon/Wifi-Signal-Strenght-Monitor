use std::sync::mpsc;
use std::thread::{self, sleep};
use std::time::{Duration, SystemTime};

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Style, Stylize};
use ratatui::widgets::{
    Axis, Block, Borders, Chart, Dataset, GraphType, Paragraph, Row, Table, TableState,
};
use ratatui::{DefaultTerminal, Frame, symbols};
use wifi_scan::Wifi;

use crate::event::Event;

#[derive(Debug, Clone)]
struct Datapoint {
    timestamp: Duration,
    signal_strength: i32,
}

impl Datapoint {
    fn new(time: Duration, value: i32) -> Self {
        Self {
            timestamp: time,
            signal_strength: value,
        }
    }
}

impl From<Datapoint> for (f64, f64) {
    fn from(value: Datapoint) -> Self {
        (value.timestamp.as_secs_f64(), value.signal_strength as f64)
    }
}

#[derive(Debug)]
struct DataPointContainer {
    start_time: SystemTime,
    data: Vec<Datapoint>,
}

impl DataPointContainer {
    fn new() -> Self {
        Self {
            start_time: SystemTime::now(),
            data: Vec::new(),
        }
    }

    fn clear(&mut self) {
        self.data.clear();
    }

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
                Some([min as f64, max as f64])
            }
        }
    }

    fn push(&mut self, value: i32) {
        self.data
            .push(Datapoint::new(self.start_time.elapsed().unwrap(), value));
    }
}

impl<'a> DataPointContainer {
    fn get_data(&'a self) -> &'a Vec<Datapoint> {
        &self.data
    }
}

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
    chart_data: DataPointContainer,
}

impl App {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<Event>();
        Self {
            exit: false,
            event_rx: rx,
            event_tx: tx,
            table_state: TableState::default(),
            detected_wifis: WifiScanResult {
                timestamp: SystemTime::now(),
                wifi: Ok(Vec::new()),
            },
            chart_data: DataPointContainer::new(),
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
                self.chart_data.clear();
                self.table_state.select_previous();
                None
            }
            Event::SelectNext => {
                self.chart_data.clear();
                self.table_state.select_next();
                None
            }
            Event::WifiScanned(v) => {
                self.detected_wifis = v;
                if let Ok(wifi) = &mut self.detected_wifis.wifi {
                    wifi.sort_by_key(|b| std::cmp::Reverse(b.signal_level));
                }

                if let Some(s) = self.table_state.selected()
                    && let Ok(wifi) = &self.detected_wifis.wifi
                    && let Some(e) = wifi.get(s)
                {
                    self.chart_data.push(e.signal_level);
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

        let ts = time_format::from_system_time(self.detected_wifis.timestamp).unwrap();
        let formatted_time = time_format::format_iso8601_local(ts).unwrap();
        items.push(formatted_time.to_string());

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(50),
                Constraint::Percentage(50),
                Constraint::Min(1),
            ])
            .split(frame.area());
        let table_area = layout[0];
        let chart_area = layout[1];
        let current_charted_area = layout[2];

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

        let selected_wifi_ssid = if let Some(s) = self.table_state.selected()
            && let Ok(wifi) = &self.detected_wifis.wifi
            && let Some(e) = wifi.get(s)
        {
            Some(e.ssid.clone())
        } else {
            None
        };

        let m = self
            .chart_data
            .get_data()
            .clone()
            .into_iter()
            .map(|d| d.into())
            .collect::<Vec<(f64, f64)>>();
        let datasets = vec![
            Dataset::default()
                .name("Wifi signal strengh")
                .marker(symbols::Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().magenta())
                .data(&m),
        ];
        let x_bounds = self.chart_data.get_limits_x().unwrap_or([0.0, 10.0]);
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
        let y_bounds = self.chart_data.get_limits_y().unwrap_or([0.0, 10.0]);
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
                selected_wifi_ssid.unwrap_or_default(),
                m.len()
            ))
            .bold()
            .cyan()
            .centered(),
            current_charted_area,
        );
    }
}

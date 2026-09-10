use std::collections::VecDeque;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::{self, sleep};
use std::time::{Duration, Instant, SystemTime};

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Block, Chart, Dataset, GraphType, Paragraph};
use ratatui::{DefaultTerminal, Frame, symbols};

use crate::event::Event;
use crate::wifi::{ConnectedAp, WlanSession};

/// The driver refreshes RSSI about once per beacon interval (~100 ms), so this oversamples a little
/// and the trace shows short plateaus. That is expected.
const FAST_SAMPLE_INTERVAL: Duration = Duration::from_millis(50);
/// How often the sampler re-checks that the adapter is still associated with the target AP.
const STATUS_CHECK_INTERVAL: Duration = Duration::from_secs(2);
/// Redraw pulse. Without it the UI would freeze whenever no samples arrive.
const TICK_INTERVAL: Duration = Duration::from_millis(250);
/// Selectable widths of the rolling time window, in seconds.
const WIN_PRESETS: [u64; 4] = [10, 30, 60, 120];
const DEFAULT_WINDOW: usize = 1;
/// Never let the y axis collapse below this many dBm, or a steady signal looks like noise.
const MIN_Y_SPAN: f64 = 10.0;
/// Platforms where [`WlanSession`] can read a live RSSI at all.
const FAST_PATH_SUPPORTED: bool = cfg!(any(target_os = "windows", target_os = "linux"));

/// A rolling time window of RSSI samples.
#[derive(Debug)]
struct SampleWindow {
    window: Duration,
    data: VecDeque<(Duration, i32)>,
}

impl SampleWindow {
    fn new(window: Duration) -> Self {
        Self {
            window,
            data: VecDeque::new(),
        }
    }

    fn push(&mut self, at: Duration, dbm: i32) {
        self.data.push_back((at, dbm));
        self.trim();
    }

    /// Drop everything older than `window` behind the newest sample.
    fn trim(&mut self) {
        let Some(&(newest, _)) = self.data.back() else {
            return;
        };
        while let Some(&(oldest, _)) = self.data.front() {
            if newest.saturating_sub(oldest) > self.window {
                self.data.pop_front();
            } else {
                break;
            }
        }
    }

    fn set_window(&mut self, window: Duration) {
        self.window = window;
        self.trim();
    }

    fn clear(&mut self) {
        self.data.clear();
    }

    fn len(&self) -> usize {
        self.data.len()
    }

    /// A window that ends at the newest sample, so the axis scrolls at constant width instead of
    /// stretching as data accumulates.
    fn bounds_x(&self) -> [f64; 2] {
        let newest = self.data.back().map(|&(t, _)| t).unwrap_or_default();
        let width = self.window.as_secs_f64();
        let end = newest.as_secs_f64();
        [end - width, end]
    }

    /// True min/max, padded, with a floor on the span, clamped to a plausible dBm range.
    fn bounds_y(&self) -> [f64; 2] {
        let Some(&(_, first)) = self.data.front() else {
            return [-100.0, 0.0];
        };
        let (mut min, mut max) = (first as f64, first as f64);
        for &(_, v) in &self.data {
            let v = v as f64;
            min = min.min(v);
            max = max.max(v);
        }

        min = (min - 2.0).max(-100.0);
        max = (max + 2.0).min(0.0);
        if max - min < MIN_Y_SPAN {
            let half = MIN_Y_SPAN / 2.0;
            let mid = ((min + max) / 2.0).clamp(-100.0 + half, -half);
            min = mid - half;
            max = mid + half;
        }
        [min, max]
    }

    fn points(&self) -> Vec<(f64, f64)> {
        self.data
            .iter()
            .map(|&(t, v)| (t.as_secs_f64(), v as f64))
            .collect()
    }
}

/// Identifies a network. SSID alone is too ambiguous.
type WifiInfo = ConnectedAp;

/// What the fast sampler is currently able to do for the selected network.
#[derive(Debug, Clone, PartialEq)]
pub enum FastStatus {
    Sampling { rate_hz: f64 },
    NotConnected,
    Unsupported,
    Error(String),
}

impl std::fmt::Display for FastStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FastStatus::Sampling { rate_hz } => write!(f, "Sampling @ {rate_hz:.1} Hz"),
            FastStatus::NotConnected => write!(
                f,
                "not connected to this AP - only the connected AP can be sampled fast"
            ),
            FastStatus::Unsupported => write!(f, "fast sampling unsupported on this platform"),
            FastStatus::Error(e) => write!(f, "error: {e}"),
        }
    }
}

#[derive(Debug)]
enum FastCommand {
    Shutdown,
}

/// What clicking a legend entry does.
#[derive(Debug, Clone, Copy)]
enum LegendAction {
    Quit,
    CycleWindow,
}

#[derive(Debug)]
pub struct App {
    exit: bool,
    event_rx: mpsc::Receiver<Event>,
    event_tx: mpsc::Sender<Event>,
    time_reference: SystemTime,
    samples: SampleWindow,
    window_idx: usize,
    fast_target: Option<WifiInfo>,
    generation: u64,
    fast_status: FastStatus,
    fast_tx: Option<mpsc::Sender<FastCommand>>,
    /// Screen regions of the keybind legend, recomputed on every draw since the layout can resize.
    legend_hitboxes: Vec<(Rect, LegendAction)>,
}

impl App {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<Event>();
        Self {
            exit: false,
            event_rx: rx,
            event_tx: tx,
            time_reference: SystemTime::now(),
            samples: SampleWindow::new(Duration::from_secs(WIN_PRESETS[DEFAULT_WINDOW])),
            window_idx: DEFAULT_WINDOW,
            fast_target: None,
            generation: 0,
            fast_status: FastStatus::NotConnected,
            fast_tx: None,
            legend_hitboxes: Vec::new(),
        }
    }

    pub fn event_sender(&self) -> mpsc::Sender<Event> {
        self.event_tx.clone()
    }

    fn start_threads(&mut self) {
        self.start_fast_sampler_thread();
        self.start_tick_thread();
    }

    fn start_fast_sampler_thread(&mut self) {
        let (cmd_tx, cmd_rx) = mpsc::channel::<FastCommand>();
        self.fast_tx = Some(cmd_tx);

        thread::spawn({
            let event_tx = self.event_tx.clone();
            move || fast_sampler(cmd_rx, event_tx)
        });
    }

    fn start_tick_thread(&self) {
        thread::spawn({
            let event_tx = self.event_tx.clone();
            move || {
                while event_tx.send(Event::Tick).is_ok() {
                    sleep(TICK_INTERVAL);
                }
            }
        });
    }

    fn run_once(&mut self) -> color_eyre::eyre::Result<()> {
        if let Ok(event) = self.event_rx.recv() {
            self.handle_event(event);
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

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Quit => {
                if let Some(tx) = &self.fast_tx {
                    let _ = tx.send(FastCommand::Shutdown);
                }
                self.exit = true;
            }
            Event::Tick => {}
            Event::FastTarget { target, generation } => {
                self.generation = generation;
                self.fast_target = target;
                self.samples.clear();
            }
            Event::FastSample { generation, dbm } => {
                // A sample still in flight from a previous target must not land in the new buffer.
                if generation == self.generation {
                    self.samples
                        .push(self.time_reference.elapsed().unwrap_or_default(), dbm);
                }
            }
            Event::FastStatus(status) => self.fast_status = status,
            Event::CycleWindow => self.cycle_window(),
            Event::Click { x, y } => self.handle_click(x, y),
        }
    }

    fn cycle_window(&mut self) {
        self.window_idx = (self.window_idx + 1) % WIN_PRESETS.len();
        self.samples
            .set_window(Duration::from_secs(WIN_PRESETS[self.window_idx]));
    }

    fn handle_click(&mut self, x: u16, y: u16) {
        let point = ratatui::layout::Position { x, y };
        let Some(&(_, action)) = self
            .legend_hitboxes
            .iter()
            .find(|(rect, _)| rect.contains(point))
        else {
            return;
        };

        match action {
            LegendAction::Quit => self.handle_event(Event::Quit),
            LegendAction::CycleWindow => self.cycle_window(),
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let [chart_area, status_area, error_area, legend_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Fill(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .areas(frame.area());

        self.draw_chart(frame, chart_area);

        let target = self
            .fast_target
            .as_ref()
            .map(|t| format!("{} [{}]", t.ssid, t.mac))
            .unwrap_or_else(|| "no network connected".to_string());
        frame.render_widget(
            Paragraph::new(format!(
                "{} | {} | n={} | window={}s",
                target,
                self.fast_status,
                self.samples.len(),
                WIN_PRESETS[self.window_idx],
            ))
            .bold()
            .cyan(),
            status_area,
        );

        let error = match &self.fast_status {
            FastStatus::Error(e) => e.clone(),
            _ => String::new(),
        };
        frame.render_widget(Paragraph::new(error).red().italic(), error_area);

        self.draw_legend(frame, legend_area);
    }

    /// A bottom keybind bar in the style of htop's function-key legend. Each entry is also a
    /// clickable button, so hitboxes are recorded in `legend_hitboxes` as they're laid out.
    fn draw_legend(&mut self, frame: &mut Frame, area: Rect) {
        const KEYBINDS: [(&str, &str, LegendAction); 2] = [
            ("w", "cycle window", LegendAction::CycleWindow),
            ("q", "quit", LegendAction::Quit),
        ];

        let key_style = Style::new().black().on_cyan();
        let label_style = Style::new().cyan();

        self.legend_hitboxes.clear();
        let mut spans = Vec::with_capacity(KEYBINDS.len() * 3);
        let mut col = area.x;
        for (key, label, action) in KEYBINDS {
            let entry = format!(" {key} {label} ");
            let width = entry.chars().count() as u16;
            self.legend_hitboxes
                .push((Rect::new(col, area.y, width, 1), action));
            col += width + 1;

            spans.push(Span::styled(format!(" {key} "), key_style));
            spans.push(Span::styled(format!("{label} "), label_style));
            spans.push(Span::raw(" "));
        }

        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_chart(&self, frame: &mut Frame, area: ratatui::layout::Rect) {
        let data = self.samples.points();
        let datasets = vec![
            Dataset::default()
                .name("Signal strength")
                .marker(symbols::Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().magenta())
                .data(&data),
        ];

        let x_bounds = self.samples.bounds_x();
        let y_bounds = self.samples.bounds_y();
        let window = WIN_PRESETS[self.window_idx];

        // The x axis scrolls with the newest sample, so label it relative to "now".
        let x_axis = Axis::default()
            .title("Time".red())
            .style(Style::default().white())
            .bounds(x_bounds)
            .labels([format!("-{window}s"), format!("-{}s", window / 2), "now".to_string()]);

        let y_axis = Axis::default()
            .title("Signal strength dBm".red())
            .style(Style::default().white())
            .bounds(y_bounds)
            .labels([
                format!("{:.0}", y_bounds[0]),
                format!("{:.0}", (y_bounds[0] + y_bounds[1]) / 2.0),
                format!("{:.0}", y_bounds[1]),
            ]);

        let title = match &self.fast_target {
            Some(t) => format!("{} [{}]", t.ssid, t.mac),
            None => "no network connected".to_string(),
        };
        let chart = Chart::new(datasets)
            .block(Block::new().title(title))
            .x_axis(x_axis)
            .y_axis(y_axis);

        frame.render_widget(chart, area);
    }
}

/// Owns the WLAN session for the lifetime of the app and samples the targeted AP as fast as the
/// driver allows.
fn fast_sampler(cmd_rx: mpsc::Receiver<FastCommand>, event_tx: mpsc::Sender<Event>) {
    let session = WlanSession::open();

    let mut target: Option<WifiInfo> = None;
    let mut generation = 0u64;
    let mut on_target = false;
    let mut last_check: Option<Instant> = None;
    let mut samples_since_check = 0u32;

    loop {
        match cmd_rx.recv_timeout(FAST_SAMPLE_INTERVAL) {
            Ok(FastCommand::Shutdown) => break,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        // Re-checking catches the association changing under us.
        if last_check.is_none_or(|at| at.elapsed() >= STATUS_CHECK_INTERVAL) {
            let elapsed = last_check.map(|at| at.elapsed().as_secs_f64());
            last_check = Some(Instant::now());

            let (status, connected) = match &session {
                Err(e) => (FastStatus::Error(e.clone()), None),
                Ok(session) => match session.connected_ap() {
                    Some(ap) => {
                        let measured = match (elapsed, samples_since_check) {
                            (Some(secs), n) if n > 0 && secs > 0.0 => n as f64 / secs,
                            _ => 1.0 / FAST_SAMPLE_INTERVAL.as_secs_f64(),
                        };
                        (FastStatus::Sampling { rate_hz: measured }, Some(ap))
                    }
                    None if FAST_PATH_SUPPORTED => (FastStatus::NotConnected, None),
                    None => (FastStatus::Unsupported, None),
                },
            };

            if connected != target {
                generation += 1;
                target = connected;
                if event_tx
                    .send(Event::FastTarget {
                        target: target.clone(),
                        generation,
                    })
                    .is_err()
                {
                    break;
                }
            }

            on_target = matches!(status, FastStatus::Sampling { .. });
            samples_since_check = 0;
            if event_tx.send(Event::FastStatus(status)).is_err() {
                break;
            }
        }

        if !on_target {
            continue;
        }
        if let Ok(session) = &session
            && let Some(dbm) = session.rssi()
        {
            samples_since_check += 1;
            let sample = Event::FastSample { generation, dbm };
            if event_tx.send(sample).is_err() {
                break;
            }
        }
    }
}

#[cfg(test)]
mod sample_window_test {
    use super::*;

    fn secs(s: f64) -> Duration {
        Duration::from_secs_f64(s)
    }

    #[test]
    fn evicts_samples_older_than_the_window() {
        let mut w = SampleWindow::new(secs(10.0));
        for i in 0..=20 {
            w.push(secs(i as f64), -50);
        }
        // Newest is at t=20, so only t=10..=20 may remain.
        assert_eq!(w.len(), 11);
        assert_eq!(w.data.front().unwrap().0, secs(10.0));
        assert_eq!(w.data.back().unwrap().0, secs(20.0));
    }

    #[test]
    fn shrinking_the_window_evicts_immediately() {
        let mut w = SampleWindow::new(secs(60.0));
        for i in 0..=20 {
            w.push(secs(i as f64), -50);
        }
        assert_eq!(w.len(), 21);
        w.set_window(secs(5.0));
        assert_eq!(w.len(), 6);
    }

    #[test]
    fn bounds_x_scrolls_at_constant_width() {
        let mut w = SampleWindow::new(secs(30.0));
        w.push(secs(100.0), -50);
        assert_eq!(w.bounds_x(), [70.0, 100.0]);
    }

    #[test]
    fn bounds_y_widens_in_both_directions() {
        let mut w = SampleWindow::new(secs(60.0));
        // Both later values extend the range that the first one seeded, one in each direction.
        for v in [-50, -70, -30] {
            w.push(secs(0.0), v);
        }
        assert_eq!(w.bounds_y(), [-72.0, -28.0]);
    }

    #[test]
    fn bounds_y_enforces_a_minimum_span() {
        let mut w = SampleWindow::new(secs(60.0));
        w.push(secs(0.0), -60);
        let [min, max] = w.bounds_y();
        assert!((max - min - MIN_Y_SPAN).abs() < f64::EPSILON, "{min}..{max}");
        assert!(min >= -100.0 && max <= 0.0);
    }

    #[test]
    fn bounds_y_stays_inside_the_dbm_range() {
        let mut w = SampleWindow::new(secs(60.0));
        w.push(secs(0.0), -100);
        w.push(secs(1.0), -99);
        let [min, max] = w.bounds_y();
        assert!(min >= -100.0 && max <= 0.0, "{min}..{max}");
        assert!(max - min >= MIN_Y_SPAN);
    }

    #[test]
    fn clear_empties_the_series() {
        let mut w = SampleWindow::new(secs(30.0));
        w.push(secs(1.0), -50);
        w.clear();
        assert_eq!(w.len(), 0);
        assert!(w.points().is_empty());
        assert_eq!(w.bounds_y(), [-100.0, 0.0]);
    }
}

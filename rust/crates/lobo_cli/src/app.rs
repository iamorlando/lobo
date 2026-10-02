use crate::{
    args::{Args, BarKind, OrderKind, OrderSide, SourceKind, View, parse_time, positive},
    engine::{Engine, Snapshot, atoms},
    render::{Renderer, Scene},
    theme::{BUILTINS, Theme},
};
use anyhow::{Result, anyhow, bail};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use lobo_models::server::{Command, LimitOrder, MarketOrder, Order, OrderFields};
use lobo_primitives::uuid::Uuid;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Row, Table},
};
use std::{sync::mpsc, time::Duration};

pub struct App {
    pub args: Args,
    pub engine: Option<Engine>,
    peer: Option<crate::shared::Client>,
    pub renderer: Renderer,
    pub theme: Theme,
    pub snapshot: Snapshot,
    pub view: View,
    pub message: String,
    pub input: Option<String>,
    pub help: bool,
    pub center: Option<f64>,
    pub span: f64,
    pub tracking: bool,
    auto_simulation: bool,
    book_rect: Rect,
    remote: Option<mpsc::Receiver<Result<String, String>>>,
}
impl App {
    pub fn new(mut args: Args) -> Result<Self> {
        let theme = Theme::load(&args.theme, args.theme_file.as_ref())?;
        let renderer = Renderer::new(args.renderer)?;
        let peer = args
            .attach
            .as_ref()
            .map(|path| crate::shared::Client::connect(path))
            .transpose()?;
        let (engine, snapshot) = if let Some(peer) = &peer {
            let update = peer
                .latest(None)
                .0
                .ok_or_else(|| anyhow!("session state unavailable"))?;
            update.config.apply(&mut args);
            (None, update.snapshot)
        } else {
            let engine = Engine::start(&args)?;
            let snapshot = engine.snapshot(args.side, args.queue_price)?;
            (Some(engine), snapshot)
        };
        let auto_simulation = args.command == Some(View::Simulate) && peer.is_none();
        let view = args.command.clone().unwrap_or(View::Dashboard);
        let span = args.min_range;
        Ok(Self {
            args,
            engine,
            peer,
            renderer,
            theme,
            snapshot,
            view,
            message: "Ready · :sim buy market 100 · ? for controls".into(),
            input: None,
            help: false,
            center: None,
            span,
            tracking: true,
            auto_simulation,
            book_rect: Rect::default(),
            remote: None,
        })
    }
    pub fn refresh(&mut self) -> Result<()> {
        if let Some(peer) = &self.peer {
            let (update, error) = peer.latest(self.snapshot.session_sequence);
            if let Some(update) = update {
                if update.snapshot.symbol != self.snapshot.symbol
                    || update.snapshot.source != self.snapshot.source
                    || update.snapshot.clock_ns < self.snapshot.clock_ns
                {
                    self.center = None;
                    self.span = self.args.min_range;
                    self.tracking = true;
                }
                update.config.apply(&mut self.args);
                self.snapshot = update.snapshot;
                self.message = update.message;
            }
            if let Some(error) = error {
                self.message = error;
            }
        } else {
            self.snapshot = self
                .local_engine()
                .snapshot(self.args.side, self.args.queue_price)?;
        }
        if let Some(engine) = &self.engine
            && engine.session.finished()
            && let Err(e) = engine.session.wait()
        {
            self.message = format!("Feed error: {e}");
            self.snapshot.feed_error = Some(e);
        }
        if let Some(rx) = &self.remote {
            match rx.try_recv() {
                Ok(result) => {
                    self.message = result.unwrap_or_else(|e| format!("Order error: {e}"));
                    self.remote = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.message = "Order worker stopped".into();
                    self.remote = None;
                }
                Err(_) => {}
            }
        }
        if self.auto_simulation
            && !self.snapshot.warming
            && self.snapshot.bid.is_some()
            && self.snapshot.ask.is_some()
        {
            self.auto_simulation = false;
            match self.local_engine().simulate(
                self.args.side,
                self.args.order_kind,
                self.args.quantity,
                self.args.price,
            ) {
                Ok(()) => self.message = "SIMULATED · native fills; main timeline continues".into(),
                Err(e) => self.message = e.to_string(),
            }
            self.snapshot = self
                .local_engine()
                .snapshot(self.args.side, self.args.queue_price)?;
        }
        let mid = match (self.snapshot.bid, self.snapshot.ask) {
            (Some(b), Some(a)) => Some((a + b) / 2.0),
            (b, a) => b.or(a),
        };
        if let Some(mid) = mid {
            if self.center.is_none() {
                self.center = Some(mid);
            }
            if self.tracking {
                let center = self.center.unwrap();
                let distance = (mid - center).abs() * 2.4;
                self.span = self.span.max(distance).max(self.args.min_range);
            }
        }
        Ok(())
    }
    pub fn event(&mut self, event: Event) -> Result<bool> {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    return Ok(true);
                }
                if let Some(input) = &mut self.input {
                    match key.code {
                        KeyCode::Esc => self.input = None,
                        KeyCode::Enter => {
                            let command = self.input.take().unwrap();
                            if let Err(e) = self.command(&command) {
                                self.message = e.to_string();
                            }
                        }
                        KeyCode::Backspace => {
                            input.pop();
                        }
                        KeyCode::Char(c) => input.push(c),
                        _ => {}
                    }
                    return Ok(false);
                }
                match key.code {
                    KeyCode::Char('q') => return Ok(true),
                    KeyCode::Char('?') => self.help = !self.help,
                    KeyCode::Esc => self.help = false,
                    KeyCode::Char(':') => self.input = Some(String::new()),
                    KeyCode::Char('1') => self.view = View::Dashboard,
                    KeyCode::Char('2') => self.view = View::Candles,
                    KeyCode::Char('3') => self.view = View::Book,
                    KeyCode::Char('4') => self.view = View::Flow,
                    KeyCode::Char('5') => self.view = View::Simulate,
                    KeyCode::Char('6') => self.view = View::Orders,
                    KeyCode::Char(' ') => {
                        let command = if self.args.paused { "play" } else { "pause" };
                        if let Err(e) = self.command(command) {
                            self.message = e.to_string();
                        }
                    }
                    KeyCode::Char('r') => self.restart()?,
                    KeyCode::Char('m') => {
                        if let Err(e) = self.command("main") {
                            self.message = e.to_string();
                        }
                    }
                    KeyCode::Home => {
                        self.center = None;
                        self.span = self.args.min_range;
                        self.tracking = true;
                    }
                    KeyCode::Up => self.pan(0.1),
                    KeyCode::Down => self.pan(-0.1),
                    KeyCode::Char('+') | KeyCode::Char('=') => {
                        self.span *= 0.8;
                        self.tracking = false;
                    }
                    KeyCode::Char('-') => {
                        self.span *= 1.25;
                        self.tracking = false;
                    }
                    KeyCode::Char('s') => {
                        let side = if self.args.side == OrderSide::Buy {
                            "buy"
                        } else {
                            "sell"
                        };
                        let kind = if self.args.order_kind == OrderKind::Market {
                            "market"
                        } else {
                            "limit"
                        };
                        let command = format!(
                            "sim {side} {kind} {} {}",
                            self.args.quantity,
                            self.args.price.unwrap_or(0.0)
                        );
                        let view = self.view.clone();
                        if let Err(e) = self.command(&command) {
                            self.message = e.to_string();
                        }
                        self.view = view;
                    }
                    KeyCode::Char('t') => {
                        let index = BUILTINS
                            .iter()
                            .position(|v| *v == self.theme.name)
                            .unwrap_or(0);
                        self.theme = Theme::load(BUILTINS[(index + 1) % BUILTINS.len()], None)?;
                    }
                    KeyCode::Char(']') | KeyCode::Char('[') => {
                        let delta = if key.code == KeyCode::Char(']') {
                            1
                        } else {
                            -1
                        };
                        let index = self
                            .snapshot
                            .tickers
                            .iter()
                            .position(|s| s == &self.snapshot.symbol)
                            .unwrap_or(0) as isize;
                        let len = self.snapshot.tickers.len() as isize;
                        if len > 0 {
                            let symbol = self.snapshot.tickers
                                [(index + delta).rem_euclid(len) as usize]
                                .clone();
                            if let Err(e) = self.select_symbol(&symbol) {
                                self.message = e.to_string();
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => self.pan(0.06),
                MouseEventKind::ScrollDown => self.pan(-0.06),
                MouseEventKind::Down(crossterm::event::MouseButton::Left)
                    if self.book_rect.contains((mouse.column, mouse.row).into())
                        && self.snapshot.level == "l3"
                        && !self.snapshot.warming
                        && f64::from(mouse.row - self.book_rect.y)
                            < f64::from(self.book_rect.height) * 0.84 =>
                {
                    let fraction = 1.0
                        - f64::from(mouse.row - self.book_rect.y)
                            / (f64::from(self.book_rect.height.max(1)) * 0.84);
                    let price =
                        self.center.unwrap_or(100.0) - self.span / 2.0 + fraction * self.span;
                    let scale = 10f64.powi(i32::from(self.snapshot.price_decimals));
                    let price = (price * scale).round() / scale;
                    if atoms(price, self.snapshot.price_decimals).is_err() {
                        return Ok(false);
                    }
                    self.args.side = if price
                        < self
                            .snapshot
                            .bid
                            .zip(self.snapshot.ask)
                            .map_or(price, |(b, a)| (b + a) / 2.0)
                    {
                        OrderSide::Buy
                    } else {
                        OrderSide::Sell
                    };
                    if let Some(peer) = &self.peer {
                        let side = if self.args.side == OrderSide::Buy {
                            "buy"
                        } else {
                            "sell"
                        };
                        if let Err(e) = peer.command(&format!("queue {side} {price}")) {
                            self.message = e.to_string();
                        }
                    } else {
                        self.args.queue_price = Some(price);
                        self.message = format!("FIFO at {price:.4}");
                    }
                }
                _ => {}
            },
            _ => {}
        }
        Ok(false)
    }
    fn pan(&mut self, fraction: f64) {
        self.center = Some(self.center.unwrap_or(100.0) + self.span * fraction);
        self.tracking = false;
    }
    pub fn restart(&mut self) -> Result<()> {
        if let Some(peer) = &self.peer {
            self.message = peer.command("restart")?;
            return Ok(());
        }
        self.args.validate()?;
        let next = Engine::start(&self.args)?;
        self.engine = Some(next);
        self.center = None;
        self.span = self.args.min_range;
        self.tracking = true;
        self.auto_simulation = self.view == View::Simulate;
        self.message = "Reconstructing source".into();
        Ok(())
    }
    fn select_symbol(&mut self, symbol: &str) -> Result<()> {
        if let Some(peer) = &self.peer {
            self.message = peer.command(&format!("symbol {symbol}"))?;
            return Ok(());
        }
        let symbol = symbol.to_ascii_uppercase();
        let selected = symbol.clone();
        self.local_engine()
            .session
            .with(move |a| a.select_ticker(&symbol))
            .map_err(|e| anyhow!(e))?;
        self.center = None;
        self.span = self.args.min_range;
        self.args.queue_price = None;
        self.args.symbol = Some(selected);
        Ok(())
    }
    pub fn command(&mut self, text: &str) -> Result<()> {
        let op = text.split_whitespace().next().unwrap_or("");
        if let Some(peer) = &self.peer
            && matches!(
                op,
                "symbol"
                    | "scope"
                    | "source"
                    | "file"
                    | "url"
                    | "start"
                    | "restart"
                    | "speed"
                    | "pause"
                    | "play"
                    | "aggregation"
                    | "bars"
                    | "history"
                    | "sim"
                    | "simulate"
                    | "main"
                    | "queue"
            )
        {
            self.message = peer.command(text)?;
            if matches!(op, "sim" | "simulate") {
                self.view = View::Simulate;
            }
            if op == "queue" {
                self.view = View::Orders;
            }
            return Ok(());
        }
        let previous = self.args.clone();
        let result = self.apply_command(text);
        if result.is_err() {
            self.args = previous;
        }
        result
    }
    fn apply_command(&mut self, text: &str) -> Result<()> {
        let mut words = text.split_whitespace();
        let op = words.next().unwrap_or("");
        let rest: Vec<_> = words.collect();
        let at = |i: usize| {
            rest.get(i)
                .copied()
                .ok_or_else(|| anyhow!("missing argument; ? shows command syntax"))
        };
        let num = |i: usize| positive(at(i)?).map_err(|e| anyhow!(e));
        match op {
            "symbol" => self.select_symbol(at(0)?)?,
            "scope" => {
                self.args.scope = at(0)?.into();
                self.restart()?;
            }
            "source" => {
                self.args.source = <SourceKind as clap::ValueEnum>::from_str(at(0)?, true)
                    .map_err(|e| anyhow!(e))?;
                self.args.file = None;
                self.args.url = None;
                self.args.symbol = rest.get(1).map(|s| s.to_string());
                self.args.validate()?;
                self.restart()?;
            }
            "file" => {
                self.args.source = SourceKind::Itch;
                self.args.file = Some(text.strip_prefix("file ").unwrap_or("").into());
                self.args.url = None;
                self.restart()?;
            }
            "url" => {
                self.args.url = Some(at(0)?.into());
                self.args.file = None;
                self.restart()?;
            }
            "start" => {
                self.args.start_at = parse_time(at(0)?).map_err(|e| anyhow!(e))?;
                self.restart()?;
            }
            "restart" => self.restart()?,
            "speed" => {
                let speed = num(0)?;
                if speed > 1000.0 {
                    bail!("speed must be <= 1000");
                }
                self.local_engine().transport(self.args.paused, speed)?;
                self.args.speed = speed;
            }
            "pause" | "play" => {
                let paused = op == "pause";
                self.local_engine().transport(paused, self.args.speed)?;
                self.args.paused = paused;
            }
            "theme" => {
                self.theme = Theme::load(text.strip_prefix("theme ").unwrap_or(""), None)?;
            }
            "theme-file" => {
                self.theme = Theme::load(
                    "local",
                    Some(&std::path::PathBuf::from(
                        text.strip_prefix("theme-file ").unwrap_or(""),
                    )),
                )?;
            }
            "aggregation" | "bars" => {
                let kind =
                    <BarKind as clap::ValueEnum>::from_str(at(0)?, true).map_err(|e| anyhow!(e))?;
                let size = at(1)?.parse()?;
                self.local_engine().set_bars(kind, size)?;
                self.args.aggregation = kind;
                self.args.bar_size = Some(size);
            }
            "history" => {
                self.args.history_seconds = num(0)?;
                self.local_engine().set_history(self.args.history_seconds)?;
            }
            "range" => {
                self.args.min_range = num(0)?;
                self.span = self.args.min_range;
                self.center = None;
                self.tracking = true;
            }
            "sim" | "simulate" => {
                let side = <OrderSide as clap::ValueEnum>::from_str(at(0)?, true)
                    .map_err(|e| anyhow!(e))?;
                let kind = <OrderKind as clap::ValueEnum>::from_str(at(1)?, true)
                    .map_err(|e| anyhow!(e))?;
                let quantity = num(2)?;
                let price = if kind == OrderKind::Limit {
                    Some(num(3)?)
                } else {
                    None
                };
                self.local_engine().simulate(side, kind, quantity, price)?;
                self.args.side = side;
                self.args.order_kind = kind;
                self.args.quantity = quantity;
                self.args.price = price;
                self.view = View::Simulate;
            }
            "main" => {
                self.local_engine()
                    .session
                    .with(|a| {
                        a.return_to_main();
                        Ok(())
                    })
                    .map_err(|e| anyhow!(e))?;
            }
            "queue" => {
                if self.snapshot.warming {
                    bail!("wait for the instrument snapshot before selecting FIFO");
                }
                let price = num(1)?;
                atoms(price, self.snapshot.price_decimals).map_err(|e| anyhow!(e))?;
                self.args.side = <OrderSide as clap::ValueEnum>::from_str(at(0)?, true)
                    .map_err(|e| anyhow!(e))?;
                self.args.queue_price = Some(price);
                self.view = View::Orders;
            }
            "add" | "limit" | "market" | "cancel" | "execute" | "remove" | "modify" => {
                let atom =
                    |value: f64, decimals: u8| atoms(value, decimals).map_err(|e| anyhow!(e));
                let id = |value: &str| -> Result<Uuid> {
                    if let Ok(index) = value.parse::<usize>() {
                        return self
                            .snapshot
                            .queue
                            .get(
                                index
                                    .checked_sub(1)
                                    .ok_or_else(|| anyhow!("queue rows start at 1"))?,
                            )
                            .ok_or_else(|| anyhow!("no such FIFO row"))?
                            .id
                            .parse()
                            .map_err(Into::into);
                    }
                    value.parse().map_err(Into::into)
                };
                let command = match op {
                    "add" | "limit" | "market" => {
                        let side = <OrderSide as clap::ValueEnum>::from_str(at(0)?, true)
                            .map_err(|e| anyhow!(e))?;
                        let id = Uuid::new_v4();
                        let quantity = atom(
                            num(if op == "market" { 1 } else { 2 })?,
                            self.snapshot.quantity_decimals,
                        )?;
                        let fields = OrderFields {
                            id,
                            trader: Uuid::nil(),
                            side: side.native(),
                            quantity,
                        };
                        if op == "market" {
                            Command::Fill {
                                order: Order::Market(MarketOrder { fields }),
                            }
                        } else {
                            let price =
                                u32::try_from(atom(num(1)?, self.snapshot.price_decimals)?)?;
                            let order = Order::Limit(LimitOrder { fields, price });
                            if op == "add" {
                                Command::Add { order }
                            } else {
                                Command::Fill { order }
                            }
                        }
                    }
                    "cancel" => Command::Cancel {
                        id: id(at(0)?)?,
                        quantity: atom(num(1)?, self.snapshot.quantity_decimals)?,
                    },
                    "execute" => Command::Execute {
                        id: id(at(0)?)?,
                        quantity: atom(num(1)?, self.snapshot.quantity_decimals)?,
                        price: None,
                    },
                    "modify" => Command::Modify {
                        id: id(at(0)?)?,
                        quantity: atom(num(1)?, self.snapshot.quantity_decimals)?,
                        price: rest
                            .get(2)
                            .map(|s| {
                                positive(s)
                                    .map_err(|e| anyhow!(e))
                                    .and_then(|v| atom(v, self.snapshot.price_decimals))
                                    .and_then(|v| u32::try_from(v).map_err(Into::into))
                            })
                            .transpose()?,
                        new_id: None,
                    },
                    _ => Command::Remove { id: id(at(0)?)? },
                };
                self.message = self.submit_order(command)?;
                return Ok(());
            }
            "help" => self.help = true,
            _ => bail!("unknown command: {op}; ? shows controls"),
        }
        self.message = format!("Applied · {text}");
        Ok(())
    }
    fn local_engine(&self) -> &Engine {
        self.engine
            .as_ref()
            .expect("local engine only used without --attach")
    }
    pub fn check_feed(&self) -> Result<()> {
        if let Some(error) = &self.snapshot.feed_error {
            bail!("{error}");
        }
        if let Some(engine) = &self.engine
            && engine.session.finished()
        {
            engine.session.wait().map_err(|e| anyhow!(e))?;
        }
        if let Some(peer) = &self.peer
            && let Some(error) = peer.latest(self.snapshot.session_sequence).1
        {
            bail!("{error}");
        }
        Ok(())
    }
    pub fn submit_order(&mut self, command: Command) -> Result<String> {
        if let Some(peer) = &self.peer {
            return peer.submit(command, &self.snapshot);
        }
        if let Some(endpoint) = self.args.order_endpoint.clone() {
            if self.remote.is_some() {
                bail!("an order request is already pending");
            }
            let (tx, rx) = mpsc::channel();
            self.remote = Some(rx);
            std::thread::spawn(move || {
                let result = (|| -> Result<String> {
                    let response = reqwest::blocking::Client::builder()
                        .timeout(Duration::from_secs(10))
                        .build()?
                        .post(endpoint)
                        .json(&command)
                        .send()?;
                    let status = response.status();
                    let body = response.text()?;
                    if !status.is_success() {
                        bail!("HTTP {status}: {body}");
                    }
                    Ok(body)
                })();
                let _ = tx.send(result.map_err(|e| e.to_string()));
            });
            return Ok("Submitting order command…".into());
        }
        self.local_engine().submit(command)
    }
    pub fn draw(&mut self, frame: &mut Frame) -> Result<()> {
        let area = frame.area();
        frame.render_widget(
            Block::default().style(Style::default().bg(self.theme.bg).fg(self.theme.fg)),
            area,
        );
        if area.width < 42 || area.height < 14 {
            frame.render_widget(
                Paragraph::new("LOBO\nEnlarge pane to 42 × 14\nq quit · ? help"),
                area,
            );
            return Ok(());
        }
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(7),
                Constraint::Length(2),
            ])
            .split(area);
        let status = if self.snapshot.warming {
            "WARMING"
        } else if self.snapshot.complete {
            "EOF"
        } else if self.args.paused {
            "PAUSED"
        } else {
            "RUNNING"
        };
        let bid = self.snapshot.bid.map_or("—".into(), |v| {
            format!(
                "{v:.precision$}",
                precision = self.snapshot.price_decimals as usize
            )
        });
        let ask = self.snapshot.ask.map_or("—".into(), |v| {
            format!(
                "{v:.precision$}",
                precision = self.snapshot.price_decimals as usize
            )
        });
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(
                        " LOBO ",
                        Style::default()
                            .fg(self.theme.bg)
                            .bg(self.theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  {}  ", self.snapshot.symbol),
                        Style::default()
                            .fg(self.theme.fg)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("B {bid}  "), Style::default().fg(self.theme.bid)),
                    Span::styled(format!("A {ask}  "), Style::default().fg(self.theme.ask)),
                    Span::styled(
                        format!(
                            "{status}  {}×  {}",
                            self.args.speed,
                            clock(self.snapshot.clock_ns)
                        ),
                        Style::default().fg(self.theme.accent),
                    ),
                ]),
                Line::from(Span::styled(
                    format!(
                        " {}{} · {} · {} · {} msg · {}",
                        self.snapshot
                            .session_sequence
                            .map_or(String::new(), |seq| format!("shared #{seq} · ")),
                        self.renderer
                            .name
                            .split(" · ")
                            .next()
                            .unwrap_or(&self.renderer.name),
                        self.snapshot.source,
                        self.snapshot.level,
                        self.snapshot.messages,
                        self.theme.name
                    ),
                    Style::default().fg(self.theme.muted),
                )),
            ]),
            rows[0],
        );
        let body = rows[1];
        self.book_rect = Rect::default();
        match self.view {
            View::Dashboard => {
                let grid = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Percentage(64), Constraint::Percentage(36)])
                    .split(body);
                let top = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(52), Constraint::Percentage(48)])
                    .split(grid[0]);
                self.chart(frame, top[0], true)?;
                self.chart(frame, top[1], false)?;
                let lower = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Percentage(30),
                        Constraint::Percentage(35),
                        Constraint::Percentage(35),
                    ])
                    .split(grid[1]);
                self.flow(frame, lower[0]);
                self.simulation(frame, lower[1]);
                self.queue(frame, lower[2]);
            }
            View::Candles => self.chart(frame, body, true)?,
            View::Book => self.chart(frame, body, false)?,
            View::Flow => self.flow(frame, body),
            View::Simulate => {
                let panes = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
                    .split(body);
                self.chart(frame, panes[0], false)?;
                let lower = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
                    .split(panes[1]);
                self.simulation(frame, lower[0]);
                self.queue(frame, lower[1]);
            }
            View::Orders => {
                let panes = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
                    .split(body);
                self.queue(frame, panes[0]);
                let text = format!(
                    "ORDER ENTRY\n\n:add buy 99.98 100\n:limit sell 100.05 250\n:market buy 100\n:cancel UUID 10\n:execute UUID 10\n:modify UUID 200 [price]\n:remove UUID\n\n:queue buy 99.98\n:sim buy limit 100 99.98\n:main\n\n{}\n\nOrders use displayed units.\nFull UUIDs appear in wide FIFO rows.",
                    if self.args.order_endpoint.is_some() {
                        "Destination: configured HTTP endpoint"
                    } else if self.args.source_kind() == SourceKind::Demo {
                        "Destination: local offline demo"
                    } else {
                        "Inspect/simulate · set --order-endpoint for writes"
                    }
                );
                frame.render_widget(
                    Paragraph::new(text)
                        .block(self.block("COMMANDS"))
                        .style(Style::default().fg(self.theme.muted)),
                    panes[1],
                );
            }
            _ => {}
        }
        let footer = if let Some(input) = &self.input {
            format!(":{input}▌")
        } else {
            format!(" {}", self.message)
        };
        frame.render_widget(Paragraph::new(vec![Line::from(Span::styled(footer,Style::default().fg(if self.input.is_some(){self.theme.accent}else{self.theme.muted}))),Line::from(Span::styled(" 1 dashboard  2 candles  3 book  4 flow  5 sim  6 orders  : command  ? help  q quit",Style::default().fg(self.theme.muted)))]),rows[2]);
        if self.help {
            let popup = Rect::new(
                area.x + 2,
                area.y + 2,
                area.width.saturating_sub(4),
                area.height.saturating_sub(4),
            );
            frame.render_widget(Clear, popup);
            frame.render_widget(
                Paragraph::new(HELP)
                    .block(self.block("LOBO · CONTROLS"))
                    .style(Style::default().bg(self.theme.bg).fg(self.theme.fg)),
                popup,
            );
        }
        Ok(())
    }
    fn block(&self, title: impl Into<String>) -> Block<'static> {
        Block::default()
            .title(format!(" {} ", title.into()))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.theme.muted))
            .style(Style::default().bg(self.theme.bg).fg(self.theme.fg))
    }
    fn chart(&mut self, frame: &mut Frame, area: Rect, candles: bool) -> Result<()> {
        let title = if candles {
            format!(
                "OHLC · {:?} {} · {} bars",
                self.args.aggregation,
                self.args.bar_target(),
                self.snapshot.candles.len()
            )
        } else {
            format!(
                "DEPTH · {}s · heatmap / cumulative",
                self.args.history_seconds
            )
        };
        let block = self.block(title);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width < 16 || inner.height < 4 {
            return Ok(());
        }
        let plot = Rect::new(inner.x + 10, inner.y, inner.width - 10, inner.height - 1);
        let (mut min, mut max) = if candles {
            let bars = self
                .snapshot
                .candles
                .iter()
                .rev()
                .take((usize::from(plot.width) / 3).clamp(1, 80));
            bars.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), b| {
                (lo.min(b.low), hi.max(b.high))
            })
        } else {
            let center = self.center.unwrap_or(100.0);
            (center - self.span / 2.0, center + self.span / 2.0)
        };
        if !min.is_finite() || !max.is_finite() {
            min = 99.5;
            max = 100.5;
        }
        if candles {
            let padding = ((max - min) * 0.12)
                .max(10f64.powi(-i32::from(self.snapshot.price_decimals)) * 2.0);
            min -= padding;
            max += padding;
        }
        let span = (max - min).max(1e-9);
        let price_height = if candles {
            (f64::from(plot.height) * 0.79).max(1.0)
        } else {
            f64::from(plot.height) * 0.84
        };
        for i in 0..5u16 {
            let y = ((price_height - 1.0) * f64::from(i) / 4.0).round() as u16;
            if y >= plot.height {
                continue;
            }
            let value = max - span * f64::from(i) / 4.0;
            frame.render_widget(
                Paragraph::new(format!("{value:>9.4}"))
                    .style(Style::default().fg(self.theme.muted)),
                Rect::new(inner.x, inner.y + y, 10, 1),
            );
        }
        let scene = Scene::new(
            &self.snapshot,
            &self.theme,
            (plot.width, plot.height),
            candles,
            min,
            span,
            self.args.history_seconds,
        );
        let pixels = self.renderer.draw(&scene)?;
        frame.render_widget(&pixels, plot);
        if !candles {
            self.book_rect = plot;
        }
        let axis = if candles {
            if self.args.aggregation == BarKind::Time {
                format!(
                    " {} ─ exchange time ─ {} · volume below",
                    self.snapshot
                        .candles
                        .first()
                        .map_or("—".into(), |b| clock(b.start_ns)),
                    clock(self.snapshot.clock_ns)
                )
            } else {
                format!(
                    " {} completed → forming · executed volume below",
                    self.snapshot.candles.iter().filter(|b| !b.forming).count()
                )
            }
        } else {
            format!(
                " −{}s → now       depth max {:.2}",
                self.args.history_seconds,
                scene.depth_scale()
            )
        };
        frame.render_widget(
            Paragraph::new(axis).style(Style::default().fg(self.theme.muted)),
            Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
        );
        Ok(())
    }
    fn flow(&self, frame: &mut Frame, area: Rect) {
        let block = self.block("ORDER IN / OUT · net liquidity");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.height < 3 || inner.width < 4 {
            return;
        }
        let flows: Vec<_> = self
            .snapshot
            .flow
            .iter()
            .rev()
            .take(usize::from(inner.width))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let max = flows
            .iter()
            .map(|f| f.incoming.max(f.outgoing))
            .fold(1.0, f64::max);
        let mid = inner.height / 2;
        for (i, flow) in flows.iter().enumerate() {
            let x = inner.x + inner.width.saturating_sub(flows.len() as u16) + i as u16;
            let up = ((flow.incoming / max) * f64::from(mid.saturating_sub(1))).ceil() as u16;
            let down = ((flow.outgoing / max) * f64::from(inner.height - mid - 1)).ceil() as u16;
            for y in 0..up {
                frame.render_widget(
                    Paragraph::new("█").style(Style::default().fg(self.theme.bid)),
                    Rect::new(x, inner.y + mid - 1 - y, 1, 1),
                );
            }
            for y in 0..down {
                frame.render_widget(
                    Paragraph::new("█").style(Style::default().fg(self.theme.ask)),
                    Rect::new(x, inner.y + mid + y, 1, 1),
                );
            }
        }
        let incoming: f64 = flows.iter().map(|f| f.incoming).sum();
        let outgoing: f64 = flows.iter().map(|f| f.outgoing).sum();
        frame.render_widget(
            Paragraph::new(format!("+{incoming:.2} / −{outgoing:.2}  max {max:.2}"))
                .style(Style::default().fg(self.theme.muted)),
            Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
        );
    }
    fn simulation(&self, frame: &mut Frame, area: Rect) {
        let block = self.block("SIMULATED FILLS");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let Some(sim) = &self.snapshot.simulation else {
            frame.render_widget(Paragraph::new(" :sim buy market 100\n :sim buy limit 100 99.98\n s run configured order\n Main liquidity keeps advancing").style(Style::default().fg(self.theme.muted)),inner);
            return;
        };
        let summary = format!(
            "{} · filled {:.4} / {:.4}\nremaining {:.4} · VWAP {}{}",
            if sim.alternate {
                "L3 BRANCH"
            } else {
                "MARKET PREVIEW"
            },
            sim.filled,
            sim.requested,
            sim.remaining,
            sim.average_price.map_or("—".into(), |v| format!("{v:.4}")),
            sim.stopped
                .as_ref()
                .map_or("".into(), |s| format!(" · {s}"))
        );
        let panes = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(0)])
            .split(inner);
        frame.render_widget(
            Paragraph::new(summary).style(Style::default().fg(self.theme.simulation)),
            panes[0],
        );
        let rows = sim.fills.iter().map(|fill| {
            Row::new(vec![
                clock(fill.clock_ns),
                format!("{:.4}", fill.price),
                format!("{:.4}", fill.quantity),
                if fill.maker { "maker" } else { "taker" }.into(),
            ])
        });
        frame.render_widget(
            Table::new(
                rows,
                [
                    Constraint::Length(12),
                    Constraint::Length(12),
                    Constraint::Length(12),
                    Constraint::Min(5),
                ],
            )
            .header(
                Row::new(["TIME", "PRICE", "QTY", "ROLE"])
                    .style(Style::default().fg(self.theme.muted)),
            ),
            panes[1],
        );
    }
    fn queue(&self, frame: &mut Frame, area: Rect) {
        let title = format!(
            "FIFO · {} · ahead {} / {}",
            self.snapshot
                .queue_price
                .map_or("best quote".into(), |p| format!("{p:.4}")),
            self.snapshot
                .orders_ahead
                .map_or("—".into(), |v| v.to_string()),
            self.snapshot
                .quantity_ahead
                .map_or("—".into(), |v| format!("{v:.2}"))
        );
        let block = self.block(title);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if self.snapshot.level != "l3" {
            frame.render_widget(
                Paragraph::new(
                    "L2 aggregate depth · no maker IDs\nMarket previews remain available",
                )
                .style(Style::default().fg(self.theme.muted)),
                inner,
            );
            return;
        }
        let id_width = if inner.width > 65 { 36 } else { 12 };
        let rows = self.snapshot.queue.iter().enumerate().map(|(i, order)| {
            Row::new(vec![
                if order.mine {
                    "YOU".into()
                } else {
                    format!("{}", i + 1)
                },
                order.id.clone(),
                format!("{:.4}", order.quantity),
                format!("{:.4}", order.price),
            ])
            .style(Style::default().fg(if order.mine {
                self.theme.simulation
            } else {
                self.theme.fg
            }))
        });
        frame.render_widget(
            Table::new(
                rows,
                [
                    Constraint::Length(4),
                    Constraint::Length(id_width),
                    Constraint::Length(10),
                    Constraint::Min(10),
                ],
            )
            .header(
                Row::new(["#", "ORDER UUID", "QTY", "PRICE"])
                    .style(Style::default().fg(self.theme.muted)),
            ),
            inner,
        );
    }
}
pub fn clock(ns: u64) -> String {
    let seconds = ns / 1_000_000_000;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60,
        ns / 1_000_000 % 1000
    )
}
const HELP: &str = "KEYS\n space pause/play   r restart/reconnect   s simulate   m main timeline\n ↑ ↓ / scroll pan   + − zoom   Home recenter   t theme   [ ] ticker\n 1 dashboard  2 candles  3 book  4 flow  5 simulation  6 orders\n\nCOMMANDS (press :)\n symbol AAPL                 scope all | top-tech | sp500 | AAPL,MSFT\n source kraken BTC/USD       file /path/to/ITCH.gz\n url wss://host/api/feed     start 09:30:00       speed 10\n aggregation time 5          volume / ticks / notional also supported\n theme Tokyo Night          theme-file /path/to/palette.toml\n history 120                range 2             restart\n sim buy market 100          sim sell limit 100 100.05\n queue buy 99.98             main\n add buy 99.98 100           limit sell 100.05 250   market buy 100\n cancel UUID 10             execute UUID 10\n modify UUID 200 [price]     remove UUID\n\nL2 supports nonmutating market previews; L3 also has isolated limit timelines.\nOrder commands target the demo or your explicit --order-endpoint.\nFlow shows net level changes per observation; departures include executions.\nGPU computes chart pixels; terminal cells use true-color half blocks.\nAGENTS: lobo completions api (JSON commands, presets, docs, APIs)\n lobo docs completions · lobo api schema --json · lobo skill install\nEsc closes this panel; q quits.";

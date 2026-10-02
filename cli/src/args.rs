use anyhow::{Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use lobo_batchers::Aggregation;
use std::{num::NonZeroU64, path::PathBuf};

#[derive(Clone, Debug, Parser)]
#[command(
    name = "lobo",
    version,
    about = "Lobo · native market tools for your terminal",
    after_help = "Examples:\n  lobo candles --source kraken --symbol BTC/USD --aggregation time --bar-size 5\n  lobo book --file session.gz --symbol AAPL --start-at 09:30:00 --speed 10\n  lobo simulate --source demo --side buy --order-kind limit --price 99.98 --quantity 200\n\nKeys: ? help · : command · space pause · 1–5 views · q quit"
)]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<View>,
    /// Share one native feed/replay with other panes: `lobo session --socket PATH`.
    #[arg(long, global = true, conflicts_with = "attach")]
    pub socket: Option<PathBuf>,
    /// Attach this view to a running local session; no independent feed is opened.
    #[arg(long, global = true, conflicts_with = "socket")]
    pub attach: Option<PathBuf>,
    /// Command for `lobo ctl --attach PATH --execute 'pause'` (or 'stop').
    #[arg(long, global = true)]
    pub execute: Option<String>,
    /// Data source; auto chooses ITCH with --file/--url, otherwise offline demo.
    #[arg(long, global = true, value_enum, default_value = "auto")]
    pub source: SourceKind,
    #[arg(long, global = true, env = "LOBO_SYMBOL")]
    pub symbol: Option<String>,
    /// Raw or gzip ITCH file. Gzip is detected from its bytes.
    #[arg(long, global = true, env = "LOBO_ITCH_PATH", conflicts_with = "url")]
    pub file: Option<PathBuf>,
    /// HTTP(S) replay URL or ws(s) live feed endpoint.
    #[arg(long, global = true, conflicts_with = "file")]
    pub url: Option<String>,
    /// Nasdaq ITCH session filename, e.g. 01302020.NASDAQ_ITCH50.gz.
    #[arg(long, global = true)]
    pub session: Option<String>,
    /// all, top-tech, sp500, or comma-separated symbols.
    #[arg(long, global = true, default_value = "selected")]
    pub scope: String,
    #[arg(long, global = true, default_value = "09:30:00", value_parser = parse_time)]
    pub start_at: u64,
    #[arg(long, global = true, default_value = "1", value_parser = speed)]
    pub speed: f64,
    #[arg(long, global = true)]
    pub paused: bool,
    #[arg(long, global = true, value_enum, default_value = "volume")]
    pub aggregation: BarKind,
    /// Volume/number of ticks/quote notional, or seconds for time bars.
    #[arg(long, global = true, value_parser = clap::value_parser!(u64).range(1..))]
    pub bar_size: Option<u64>,
    #[arg(long, global = true, default_value = "60", value_parser = positive)]
    pub history_seconds: f64,
    #[arg(long, global = true, default_value = "256", value_parser = clap::value_parser!(u32).range(32..=8192))]
    pub capacity: u32,
    /// Starting visible price range; edges automatically expand it.
    #[arg(long, global = true, default_value = "1", value_parser = positive)]
    pub min_range: f64,
    #[arg(long, global = true, default_value = "60", value_parser = clap::value_parser!(u16).range(1..=120))]
    pub fps: u16,
    /// auto tries native Metal/Vulkan/DX12; cpu works over SSH too.
    #[arg(long, global = true, value_enum, default_value = "auto")]
    pub renderer: RendererKind,
    /// Built-in palette or any upstream iTerm2-Color-Schemes name.
    #[arg(
        long,
        global = true,
        default_value = "Acid Lime",
        conflicts_with = "theme_file"
    )]
    pub theme: String,
    /// Local WezTerm/iTerm color scheme TOML; fully offline.
    #[arg(long, global = true)]
    pub theme_file: Option<PathBuf>,
    #[arg(long, global = true, value_enum, default_value = "buy")]
    pub side: OrderSide,
    #[arg(long, global = true, value_enum, default_value = "market")]
    pub order_kind: OrderKind,
    #[arg(long, global = true, default_value = "100", value_parser = positive)]
    pub quantity: f64,
    #[arg(long, global = true, value_parser = positive)]
    pub price: Option<f64>,
    /// FIFO level to inspect; defaults to your order or the best quote.
    #[arg(long, global = true, value_parser = positive)]
    pub queue_price: Option<f64>,
    /// Hosted adapter descriptor JSON (same as /api/server-context adapters).
    #[arg(long, global = true)]
    pub descriptor: Option<PathBuf>,
    /// Explicit HTTP order-command endpoint, e.g. http://localhost:8765/api/books/BOOK/orders.
    #[arg(long, global = true)]
    pub order_endpoint: Option<String>,
    /// Exit after N frames (useful for profiling/terminal smoke tests).
    #[arg(long, global = true, value_parser = clap::value_parser!(u32).range(1..))]
    pub frames: Option<u32>,
    /// Capture a native state JSON snapshot after --capture-seconds, without a TTY.
    #[arg(long, global = true, conflicts_with = "snapshot_text")]
    pub snapshot_json: bool,
    /// Render a plain text terminal snapshot without a TTY.
    #[arg(long, global = true, conflicts_with = "snapshot_json")]
    pub snapshot_text: bool,
    #[arg(long, global = true, default_value = "2", value_parser = positive)]
    pub capture_seconds: f64,
    #[arg(long, global = true, default_value = "140", value_parser = clap::value_parser!(u16).range(20..=512))]
    pub width: u16,
    #[arg(long, global = true, default_value = "44", value_parser = clap::value_parser!(u16).range(8..=256))]
    pub height: u16,
    #[arg(long, global = true, default_value = "42")]
    pub seed: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Subcommand)]
pub enum View {
    /// All charts, execution fills, FIFO and controls in one screen.
    Dashboard,
    /// Candlesticks with executed volume and unfinished-bar preview.
    #[command(alias = "ohlc")]
    Candles,
    /// Liquidity heatmap, cumulative depth and best bid/ask.
    #[command(alias = "depth", alias = "orderbook")]
    Book,
    /// Net visible liquidity arriving and departing at each observation.
    #[command(alias = "in-out")]
    Flow,
    /// Nonmutating market preview or isolated L3 limit timeline.
    Simulate,
    /// FIFO queue inspection and add/cancel/execute/remove order commands.
    Orders,
    /// Own one feed, replay clock and simulation for every attached terminal pane.
    Session,
    /// Send an interactive command to a local session without opening a TUI.
    Ctl,
    /// Measure layout and native GPU/CPU chart frames without terminal I/O.
    Benchmark,
    /// List built-in palettes or upstream palettes with --all.
    Themes {
        #[arg(long)]
        all: bool,
    },
    /// List the public Nasdaq ITCH session directory.
    Sessions,
    /// Generate a shell completion script.
    Completions { shell: clap_complete::Shell },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, serde::Serialize, serde::Deserialize)]
pub enum SourceKind {
    Auto,
    Demo,
    Itch,
    Nasdaq,
    Kraken,
    Bitfinex,
    Server,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, serde::Serialize, serde::Deserialize)]
pub enum RendererKind {
    Auto,
    Gpu,
    Cpu,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, serde::Serialize, serde::Deserialize)]
pub enum BarKind {
    Volume,
    Time,
    Ticks,
    Notional,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, serde::Serialize, serde::Deserialize)]
pub enum OrderSide {
    Buy,
    Sell,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, serde::Serialize, serde::Deserialize)]
pub enum OrderKind {
    Market,
    Limit,
}

impl OrderSide {
    pub fn native(self) -> lobo_models::Side {
        match self {
            Self::Buy => lobo_models::Side::Buy,
            Self::Sell => lobo_models::Side::Sell,
        }
    }
}
impl Args {
    pub fn source_kind(&self) -> SourceKind {
        match self.source {
            SourceKind::Auto if self.file.is_some() || self.url.is_some() => SourceKind::Itch,
            SourceKind::Auto => SourceKind::Demo,
            other => other,
        }
    }
    pub fn ticker(&self) -> String {
        self.symbol
            .clone()
            .unwrap_or_else(|| {
                match self.source_kind() {
                    SourceKind::Kraken => "BTC/USD",
                    SourceKind::Bitfinex => "tBTCUSD",
                    SourceKind::Server => "BOOK",
                    _ => "AAPL",
                }
                .into()
            })
            .to_ascii_uppercase()
    }
    pub fn bar_target(&self) -> u64 {
        self.bar_size.unwrap_or(match self.aggregation {
            BarKind::Volume
                if matches!(
                    self.source_kind(),
                    SourceKind::Kraken | SourceKind::Bitfinex
                ) =>
            {
                1
            }
            BarKind::Volume => 5000,
            BarKind::Time => 5,
            BarKind::Ticks => 100,
            BarKind::Notional => 100000,
        })
    }
    pub fn bars(&self) -> Result<Aggregation> {
        aggregation(self.aggregation, self.bar_target())
    }
    pub fn validate(&self) -> Result<()> {
        self.bars()?;
        if self.command == Some(View::Session) && self.socket.is_none() {
            bail!("session requires --socket PATH");
        }
        if self.command == Some(View::Ctl) && (self.attach.is_none() || self.execute.is_none()) {
            bail!("ctl requires --attach PATH --execute COMMAND");
        }
        if self.socket.is_some() && self.command != Some(View::Session) {
            bail!("--socket is for `lobo session`; views use --attach");
        }
        if self.execute.is_some() && self.command != Some(View::Ctl) {
            bail!("--execute is for `lobo ctl`");
        }
        if self.attach.is_some() {
            return Ok(());
        }
        let source = self.source_kind();
        if self.session.is_some() && source != SourceKind::Nasdaq {
            bail!("--session requires --source nasdaq");
        }
        if self.descriptor.is_some() && source != SourceKind::Server {
            bail!("--descriptor requires --source server");
        }
        if self.order_kind == OrderKind::Limit
            && self.price.is_none()
            && self.command == Some(View::Simulate)
        {
            bail!("limit simulation requires --price");
        }
        if self.order_kind == OrderKind::Limit
            && matches!(source, SourceKind::Kraken | SourceKind::Bitfinex)
        {
            bail!("L2 feeds support market simulation only");
        }
        if matches!(
            source,
            SourceKind::Kraken | SourceKind::Bitfinex | SourceKind::Server
        ) && self.file.is_some()
        {
            bail!("--file is for ITCH replay; live sources use --url");
        }
        if source == SourceKind::Demo && (self.file.is_some() || self.url.is_some()) {
            bail!("demo does not accept --file or --url");
        }
        if let Some(endpoint) = &self.order_endpoint
            && !endpoint.starts_with("http://")
            && !endpoint.starts_with("https://")
        {
            bail!("--order-endpoint must be HTTP(S)");
        }
        Ok(())
    }
}
pub fn aggregation(kind: BarKind, size: u64) -> Result<Aggregation> {
    let target =
        NonZeroU64::new(size).ok_or_else(|| anyhow::anyhow!("bar size must be positive"))?;
    Ok(match kind {
        BarKind::Volume => Aggregation::Volume(target),
        BarKind::Ticks => Aggregation::Ticks(target),
        BarKind::Notional => Aggregation::Notional(target),
        BarKind::Time => Aggregation::Time(
            NonZeroU64::new(
                size.checked_mul(1_000_000_000)
                    .ok_or_else(|| anyhow::anyhow!("time interval overflow"))?,
            )
            .unwrap(),
        ),
    })
}
pub fn positive(s: &str) -> Result<f64, String> {
    s.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or_else(|| "must be a positive finite number".into())
}
fn speed(s: &str) -> Result<f64, String> {
    let v = positive(s)?;
    if v > 1000.0 {
        Err("speed must be <= 1000".into())
    } else {
        Ok(v)
    }
}
pub fn parse_time(s: &str) -> Result<u64, String> {
    let parts: Vec<_> = s.split(':').collect();
    if parts.len() != 3 {
        return Err("use HH:MM:SS exchange time".into());
    }
    let h: u64 = parts[0].parse().map_err(|_| "invalid hour")?;
    let m: u64 = parts[1].parse().map_err(|_| "invalid minute")?;
    let sec: f64 = parts[2].parse().map_err(|_| "invalid seconds")?;
    if h >= 24 || m >= 60 || !sec.is_finite() || !(0.0..60.0).contains(&sec) {
        return Err("time must be within a trading day".into());
    }
    Ok(((h * 3600 + m * 60) as f64 * 1e9 + sec * 1e9) as u64)
}

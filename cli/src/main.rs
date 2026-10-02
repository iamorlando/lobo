mod app;
mod args;
mod demo;
mod engine;
mod render;
mod shared;
mod theme;
use clap::{CommandFactory, Parser};
use std::{
    io::{self, IsTerminal, Write},
    time::{Duration, Instant},
};
fn main() -> anyhow::Result<()> {
    let args = args::Args::parse();
    match args.command {
        Some(args::View::Completions { shell }) => {
            clap_complete::generate(shell, &mut args::Args::command(), "lobo", &mut io::stdout());
            return Ok(());
        }
        Some(args::View::Themes { all }) => {
            if all {
                let catalog: serde_json::Value = serde_json::from_str(&theme::fetch(
                    "https://api.github.com/repos/mbadolato/iTerm2-Color-Schemes/contents/wezterm",
                    2_000_000,
                )?)?;
                for item in catalog
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("invalid theme catalog"))?
                {
                    if let Some(name) = item["name"].as_str().and_then(|s| s.strip_suffix(".toml"))
                    {
                        println!("{name}");
                    }
                }
            } else {
                for name in theme::BUILTINS {
                    println!("{name}");
                }
            }
            return Ok(());
        }
        Some(args::View::Sessions) => {
            for name in sessions()? {
                println!("{name}");
            }
            return Ok(());
        }
        _ => {}
    }
    args.validate()?;
    if args.command == Some(args::View::Session) {
        return shared::run(&args);
    }
    if args.command == Some(args::View::Ctl) {
        let client = shared::Client::connect(args.attach.as_ref().unwrap())?;
        println!("{}", client.command(args.execute.as_ref().unwrap())?);
        return Ok(());
    }
    if args.command == Some(args::View::Benchmark) {
        return benchmark(args);
    }
    if !(args.snapshot_json
        || args.snapshot_text
        || io::stdin().is_terminal() && io::stdout().is_terminal())
    {
        anyhow::bail!(
            "interactive charts require a terminal; use --snapshot-json or --snapshot-text for capture"
        );
    }
    let mut app = app::App::new(args)?;
    if app.args.snapshot_json || app.args.snapshot_text {
        let until = Instant::now() + Duration::from_secs_f64(app.args.capture_seconds);
        loop {
            app.refresh()?;
            if Instant::now() >= until {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        app.check_feed()?;
        if app.args.snapshot_json {
            println!("{}", serde_json::to_string_pretty(&app.snapshot)?);
        } else {
            let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(
                app.args.width,
                app.args.height,
            ))?;
            let mut failure = None;
            terminal.draw(|frame| {
                failure = app.draw(frame).err();
            })?;
            if let Some(e) = failure {
                return Err(e);
            }
            let buffer = terminal.backend().buffer();
            let mut out = io::stdout().lock();
            for y in 0..buffer.area.height {
                for x in 0..buffer.area.width {
                    write!(out, "{}", buffer[(x, y)].symbol())?;
                }
                writeln!(out)?;
            }
        }
        return Ok(());
    }
    // Color is chart data: suppressing RGB makes every half-block pixel an
    // identical stripe. Interactive themed charts explicitly require color.
    crossterm::style::force_color_output(true);
    let mut terminal = ratatui::init();
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = crossterm::execute!(io::stdout(), crossterm::event::DisableMouseCapture);
            ratatui::restore();
        }
    }
    let _restore = Restore;
    crossterm::execute!(io::stdout(), crossterm::event::EnableMouseCapture)?;
    let period = Duration::from_secs_f64(1.0 / f64::from(app.args.fps));
    let mut frames = 0u32;
    let mut next = Instant::now();
    'running: loop {
        if Instant::now() >= next {
            app.refresh()?;
            let mut failure = None;
            terminal.draw(|frame| {
                failure = app.draw(frame).err();
            })?;
            if let Some(e) = failure {
                return Err(e);
            }
            frames += 1;
            if app.args.frames.is_some_and(|limit| frames >= limit) {
                break;
            }
            next += period;
            if next < Instant::now() {
                next = Instant::now();
            }
        }
        if crossterm::event::poll(next.saturating_duration_since(Instant::now()))? {
            // Drain a pasted command before drawing; do not dispatch GPU work
            // once per buffered key. Bound the batch to keep feeds responsive.
            for _ in 0..256 {
                if app.event(crossterm::event::read()?)? {
                    break 'running;
                }
                if !crossterm::event::poll(Duration::ZERO)? {
                    break;
                }
            }
            next = Instant::now();
        }
    }
    Ok(())
}
fn benchmark(mut args: args::Args) -> anyhow::Result<()> {
    let count = args.frames.unwrap_or(120);
    args.command = Some(args::View::Dashboard);
    let mut app = app::App::new(args)?;
    let until = Instant::now() + Duration::from_secs_f64(app.args.capture_seconds);
    while Instant::now() < until {
        app.refresh()?;
        std::thread::sleep(Duration::from_millis(10));
    }
    app.check_feed()?;
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(
        app.args.width,
        app.args.height,
    ))?;
    let mut measurements = Vec::new();
    for view in [
        args::View::Candles,
        args::View::Book,
        args::View::Flow,
        args::View::Simulate,
        args::View::Orders,
        args::View::Dashboard,
    ] {
        app.view = view;
        let mut times = Vec::new();
        for i in 0..count + 3 {
            let started = Instant::now();
            app.refresh()?;
            let mut error = None;
            terminal.draw(|frame| error = app.draw(frame).err())?;
            if let Some(error) = error {
                return Err(error);
            }
            if i >= 3 {
                times.push(started.elapsed().as_secs_f64() * 1000.0);
            }
        }
        times.sort_by(f64::total_cmp);
        measurements.push(serde_json::json!({
            "view": format!("{view:?}").to_ascii_lowercase(),
            "frames": count, "median_ms": times[times.len()/2],
            "p95_ms": times[(times.len()*95/100).min(times.len()-1)],
        }));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "renderer": app.renderer.name, "width": app.args.width, "height": app.args.height,
            "shared_session": app.args.attach, "source": app.snapshot.source,
            "messages": app.snapshot.messages, "levels": app.snapshot.levels.len(),
            "depth_frames": app.snapshot.depth.len(), "measurements": measurements,
            "scope": "native snapshot + layout + chart raster + GPU readback; excludes terminal escape output",
        }))?
    );
    Ok(())
}
fn sessions() -> anyhow::Result<Vec<String>> {
    let html = theme::fetch("https://emi.nasdaq.com/ITCH/Nasdaq%20ITCH/", 1_048_576)?;
    let mut names = std::collections::BTreeSet::new();
    for piece in html.split("href=\"").skip(1) {
        let Some(href) = piece.split('"').next() else {
            continue;
        };
        let name = href.rsplit('/').next().unwrap_or("");
        if name.ends_with(".gz")
            && (name.contains(".NASDAQ_ITCH50")
                || name.starts_with("itch50_")
                || (name.starts_with('S') && name.ends_with("-v50.txt.gz")))
        {
            names.insert(name.to_owned());
        }
    }
    if names.is_empty() {
        anyhow::bail!("no ITCH 5.0 sessions listed by Nasdaq");
    }
    Ok(names.into_iter().collect())
}
#[cfg(test)]
mod tests;

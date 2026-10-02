use super::*;
use args::{Args, OrderKind, OrderSide, RendererKind, View};
use lobo_adapters::adapter::market::MarketDataAdapter;
use lobo_models::Side;
use lobo_primitives::{Price64, uuid::Uuid};

fn options(extra: &[&str]) -> Args {
    Args::parse_from(std::iter::once("lobo").chain(extra.iter().copied()))
}
fn demo() -> demo::Demo {
    demo::Demo::new(&options(&[]), vec!["AAPL".into()]).unwrap()
}
#[test]
fn flags_work_before_and_after_subcommands_and_reject_invalid_units() {
    let a = options(&[
        "--theme",
        "Nord",
        "candles",
        "--aggregation",
        "time",
        "--bar-size",
        "5",
    ]);
    assert_eq!(a.command, Some(View::Candles));
    assert_eq!(a.bar_target(), 5);
    for value in ["NaN", "inf", "0", "-1"] {
        assert!(Args::try_parse_from(["lobo", "--quantity", value]).is_err());
    }
    assert!(
        options(&["simulate", "--order-kind", "limit"])
            .validate()
            .is_err()
    );
    assert!(args::parse_time("24:00:00").is_err());
    assert_eq!(args::parse_time("09:30:00").unwrap(), 34_200_000_000_000);
    assert_eq!(engine::atoms(0.125, 3).unwrap(), 125);
    assert!(engine::atoms(0.125, 2).is_err());
    assert!(args::aggregation(args::BarKind::Time, u64::MAX).is_err());
}
#[test]
fn market_preview_does_not_change_native_book_or_candles() {
    let mut feed = demo();
    let before = feed
        .state()
        .selected_book()
        .unwrap()
        .visible_quantity(Side::Sell);
    let bars = feed.state().volume_bars().borrow().completed("AAPL");
    feed.simulate_market(lobo_models::orders::order_types::MarketOrder::new(
        200,
        Uuid::new_v4(),
        Side::Buy,
    ))
    .unwrap();
    assert_eq!(feed.state().market_preview.as_ref().unwrap().filled, 200);
    assert_eq!(
        feed.state()
            .selected_book()
            .unwrap()
            .visible_quantity(Side::Sell),
        before
    );
    assert_eq!(feed.state().volume_bars().borrow().completed("AAPL"), bars);
}
#[test]
fn limit_branch_preserves_main_book_and_has_real_fifo_ids() {
    let mut feed = demo();
    let count = feed.state().selected_book().unwrap().order_count();
    let order = lobo_models::orders::order_types::LimitOrder::new(
        Some(Price64::from(9999u64)),
        200,
        Uuid::new_v4(),
        Side::Buy,
    );
    feed.simulate(order).unwrap();
    let branch = feed.state().simulation.as_ref().unwrap();
    assert_eq!(feed.state().selected_book().unwrap().order_count(), count);
    let queue = branch
        .feed
        .selected_book()
        .unwrap()
        .queue_view(Side::Buy, 9999u64.into()..=9999u64.into());
    assert_eq!(queue.last().unwrap().id, branch.report.order_id);
    assert!(queue.len() > 1);
    feed.return_to_main();
    assert!(feed.state().simulation.is_none());
}
fn record(kind: u8, id: u64, side: u8, qty: u32, price: u32, ns: u64) -> Vec<u8> {
    let mut body = vec![kind, 0, 1, 0, 0];
    body.extend_from_slice(&ns.to_be_bytes()[2..]);
    body.extend_from_slice(&id.to_be_bytes());
    if kind == b'A' {
        body.push(side);
        body.extend_from_slice(&qty.to_be_bytes());
        body.extend_from_slice(b"AAPL    ");
        body.extend_from_slice(&price.to_be_bytes());
    } else {
        body.extend_from_slice(&qty.to_be_bytes());
        body.extend_from_slice(&42u64.to_be_bytes());
    }
    let mut bytes = (body.len() as u16).to_be_bytes().to_vec();
    bytes.extend(body);
    bytes
}
pub fn itch_fixture() -> Vec<u8> {
    [
        record(b'A', 1, b'B', 500, 990000, 0),
        record(b'A', 2, b'S', 500, 1010000, 0),
        record(b'E', 2, 0, 250, 0, 1_000_000_000),
    ]
    .concat()
}
#[test]
fn native_itch_volume_ticks_notional_and_time_bar_semantics() {
    for (kind, target, expected) in [
        (args::BarKind::Volume, 100, 2),
        (args::BarKind::Ticks, 1, 1),
        (args::BarKind::Notional, 10000, 1),
    ] {
        let mut feed = lobo_adapters::adapter::market::create_adapter("itch", "AAPL", 0).unwrap();
        feed.state_mut()
            .set_bar_aggregation(args::aggregation(kind, target).unwrap());
        feed.receive(&itch_fixture(), true).unwrap();
        feed.advance(2_000_000_000, 4096).unwrap();
        let bars = feed.state().volume_bars().borrow();
        assert_eq!(bars.completed("AAPL"), expected);
        if kind == args::BarKind::Volume {
            assert_eq!(bars.forming("AAPL").unwrap().volume, 50);
            assert!(
                bars.destination
                    .0
                    .borrow()
                    .iter()
                    .all(|e| e.event().volume == 100)
            );
        }
    }
    let mut feed = lobo_adapters::adapter::market::create_adapter("itch", "AAPL", 0).unwrap();
    feed.state_mut()
        .set_bar_aggregation(args::aggregation(args::BarKind::Time, 5).unwrap());
    let mut bytes = itch_fixture();
    bytes.extend(record(b'E', 2, 0, 50, 0, 6_000_000_000));
    feed.receive(&bytes, true).unwrap();
    feed.advance(7_000_000_000, 4096).unwrap();
    assert_eq!(feed.state().volume_bars().borrow().completed("AAPL"), 1);
}
#[test]
fn paused_demo_transport_and_bounded_histories() {
    let args = options(&["--paused", "--capacity", "32", "--history-seconds", "0.1"]);
    let engine = engine::Engine::start(&args).unwrap();
    engine.transport(true, 1000.0).unwrap();
    let before = engine.snapshot(OrderSide::Buy, None).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(25));
    assert_eq!(
        engine.snapshot(OrderSide::Buy, None).unwrap().messages,
        before.messages
    );
    engine.transport(false, 1000.0).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(80));
    let after = engine.snapshot(OrderSide::Buy, None).unwrap();
    assert!(after.messages > before.messages);
    assert!(after.depth.len() <= 32);
    assert!(after.candles.len() <= 33);
    assert!(after.flow.len() <= 32);
}
#[test]
fn l2_rejects_limit_simulation() {
    let mut feed = lobo_adapters::adapter::market::create_adapter("kraken", "BTC/USD", 0).unwrap();
    let order = lobo_models::orders::order_types::LimitOrder::new(
        Some(Price64::from(100u64)),
        1,
        Uuid::new_v4(),
        Side::Buy,
    );
    assert!(feed.simulate(order).unwrap_err().contains("L2"));
}
#[test]
fn every_view_renders_at_pane_sizes_and_controls_preserve_state() {
    let mut app = app::App::new(options(&["--renderer", "cpu", "--paused"])).unwrap();
    app.command("sim buy market 100").unwrap();
    app.refresh().unwrap();
    let sim = app.snapshot.simulation.as_ref().unwrap().filled;
    app.command("theme Nord").unwrap();
    app.command("aggregation ticks 2").unwrap();
    app.refresh().unwrap();
    assert_eq!(app.snapshot.simulation.as_ref().unwrap().filled, sim);
    for view in [
        View::Dashboard,
        View::Candles,
        View::Book,
        View::Flow,
        View::Simulate,
        View::Orders,
    ] {
        app.view = view;
        for (w, h) in [(20, 8), (42, 14), (80, 24), (140, 44)] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            let mut error = None;
            terminal.draw(|f| error = app.draw(f).err()).unwrap();
            assert!(error.is_none(), "{view:?} at {w}x{h}: {error:?}");
        }
    }
    app.command("main").unwrap();
    assert!(app.command("queue buy 98.001").is_err());
    app.refresh().unwrap();
    app.command("add buy 98.00 42").unwrap();
    app.command("queue buy 98.00").unwrap();
    app.refresh().unwrap();
    assert!(app.snapshot.queue.iter().any(|o| o.quantity == 42.0));
}
#[test]
fn themes_parse_offline_without_silently_rounding_invalid_colors() {
    let theme=theme::Theme::parse("local","[colors]\nbackground='#000000'\nforeground='#ffffff'\nansi=['#000000','#ff0000','#00ff00','#ffff00','#0000ff','#ff00ff','#00ffff','#ffffff']\n").unwrap();
    assert_eq!(theme.bid, ratatui::style::Color::Rgb(0, 255, 0));
    assert!(theme::Theme::parse("bad", "[colors]\nbackground='red'").is_err());
}
#[test]
#[ignore = "requires a physical GPU; run cargo test gpu_matches_cpu -- --ignored"]
fn gpu_matches_cpu_for_candles_and_depth() {
    let engine = engine::Engine::start(&options(&["--paused"])).unwrap();
    engine
        .simulate(OrderSide::Buy, OrderKind::Market, 100.0, None)
        .unwrap();
    let mut snapshot = engine.snapshot(OrderSide::Buy, None).unwrap();
    snapshot.candles = vec![
        engine::Candle {
            open: 99.6,
            high: 100.1,
            low: 99.5,
            close: 99.9,
            volume: 30.0,
            ticks: 2,
            start_ns: 0,
            end_ns: 1,
            forming: false,
        },
        engine::Candle {
            open: 99.9,
            high: 100.0,
            low: 99.7,
            close: 99.8,
            volume: 20.0,
            ticks: 1,
            start_ns: 1,
            end_ns: 2,
            forming: false,
        },
        engine::Candle {
            open: 99.8,
            high: 100.0,
            low: 99.6,
            close: 99.95,
            volume: 10.0,
            ticks: 1,
            start_ns: 2,
            end_ns: 3,
            forming: true,
        },
    ];
    let theme = theme::Theme::load("Nord", None).unwrap();
    let mut gpu = render::Renderer::new(RendererKind::Gpu).unwrap();
    // Resize up and down and alternate chart types to verify persistent GPU
    // buffers are updated and read back using the current viewport only.
    for (size, candles) in [
        ((80, 20), false),
        ((80, 20), true),
        ((20, 8), false),
        ((150, 45), false),
        ((150, 45), true),
        ((80, 20), false),
    ] {
        let scene = render::Scene::new(&snapshot, &theme, size, candles, 99.5, 1.0, 60.0);
        let cpu = render::cpu(&scene);
        let pixels = gpu.draw(&scene).unwrap();
        assert_eq!(cpu.len(), pixels.values.len());
        for (index, (a, b)) in cpu.iter().zip(&pixels.values).enumerate() {
            for shift in [0, 8, 16] {
                assert!(
                    (((a >> shift) & 255) as i32 - ((b >> shift) & 255) as i32).abs() <= 1,
                    "{size:?} candles={candles} pixel=({}, {}) cpu={a:06x} gpu={b:06x}",
                    index % usize::from(size.0),
                    index / usize::from(size.0)
                );
            }
        }
    }
}

#[test]
fn bundled_chart_shader_is_valid_without_a_gpu() {
    let module = naga::front::wgsl::parse_str(include_str!("chart.wgsl")).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap();
}

#[test]
#[ignore = "writes a visual QA artifact; set LOBO_PREVIEW_PATH to a JSON file"]
fn visual_preview() {
    let path = std::env::var("LOBO_PREVIEW_PATH").expect("LOBO_PREVIEW_PATH");
    let mut app = app::App::new(options(&[
        "--renderer",
        "cpu",
        "--speed",
        "40",
        "--aggregation",
        "ticks",
        "--bar-size",
        "10",
    ]))
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    app.engine
        .as_ref()
        .unwrap()
        .simulate(OrderSide::Buy, OrderKind::Market, 100.0, None)
        .unwrap();
    app.refresh().unwrap();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(160, 48)).unwrap();
    terminal.draw(|frame| app.draw(frame).unwrap()).unwrap();
    let b = terminal.backend().buffer();
    let mut rows = Vec::new();
    for y in 0..b.area.height {
        let mut row = Vec::new();
        for x in 0..b.area.width {
            let cell = &b[(x, y)];
            let color = |c| {
                let v = theme::rgb(c);
                v[..3]
                    .iter()
                    .map(|v| (v * 255.0).round() as u8)
                    .collect::<Vec<_>>()
            };
            row.push(
                serde_json::json!({"symbol":cell.symbol(),"fg":color(cell.fg),"bg":color(cell.bg)}),
            );
        }
        rows.push(row);
    }
    std::fs::write(path, serde_json::to_vec(&rows).unwrap()).unwrap();
}

#[test]
fn depth_raster_bins_preserve_visible_quantity_and_bound_record_count() {
    let mut snapshot = engine::Snapshot {
        clock_ns: 10_000_000_000,
        ..Default::default()
    };
    snapshot.levels = (0..10000)
        .map(|i| engine::Level {
            price: 99.0 + i as f64 / 5000.0,
            quantity: 1.0,
            side: (i % 2) as u8,
            orders: 1,
        })
        .collect();
    snapshot.depth.push(engine::DepthFrame {
        clock_ns: snapshot.clock_ns,
        levels: snapshot.levels.clone(),
    });
    let theme = theme::Theme::load("Nord", None).unwrap();
    let scene = render::Scene::new(&snapshot, &theme, (80, 20), false, 99.0, 2.0, 60.0);
    let headers = scene.records[0][0] as usize;
    let current = scene.records[headers - 1];
    let rows = &scene.records[current[0] as usize..current[0] as usize + current[1] as usize];
    assert_eq!(rows.iter().map(|b| b[0]).sum::<f32>(), 5000.0);
    assert_eq!(rows.iter().map(|b| b[1]).sum::<f32>(), 5000.0);
    assert_eq!(rows.last().unwrap()[2], 5000.0);
    assert_eq!(rows.first().unwrap()[3], 5000.0);
    assert!(
        scene.records.len() < 2200,
        "terminal raster must scale with viewport, not level count"
    );
}

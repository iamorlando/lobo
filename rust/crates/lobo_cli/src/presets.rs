//! Presets only fill defaults; Clap remains the parser and validator.
use crate::args::Args;
use clap::{ArgMatches, FromArgMatches, Parser, ValueEnum, parser::ValueSource};
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Preset {
    Dashboard,
    Simulator,
    TickBars,
    TimeBars,
    VolumeBars,
    NotionalBars,
    Bitfinex,
    Kraken,
    Fifo,
    Flow,
}

#[derive(Serialize)]
pub struct Definition {
    pub name: Preset,
    pub description: &'static str,
    pub argv: &'static [&'static str],
}

pub const PRESETS: &[Definition] = &[
    Definition {
        name: Preset::Dashboard,
        description: "Offline L3 dashboard",
        argv: &["dashboard"],
    },
    Definition {
        name: Preset::Simulator,
        description: "Nonmutating market simulator; add --order-kind limit --price PRICE for an isolated L3 timeline",
        argv: &["simulate", "--order-kind", "market", "--quantity", "100"],
    },
    Definition {
        name: Preset::TickBars,
        description: "100 executions per candle",
        argv: &["candles", "--aggregation", "ticks", "--bar-size", "100"],
    },
    Definition {
        name: Preset::TimeBars,
        description: "Five-second aligned candles",
        argv: &["candles", "--aggregation", "time", "--bar-size", "5"],
    },
    Definition {
        name: Preset::VolumeBars,
        description: "5,000 displayed quantity units per candle",
        argv: &["candles", "--aggregation", "volume", "--bar-size", "5000"],
    },
    Definition {
        name: Preset::NotionalBars,
        description: "100,000 quote-currency units per candle",
        argv: &[
            "candles",
            "--aggregation",
            "notional",
            "--bar-size",
            "100000",
        ],
    },
    Definition {
        name: Preset::Bitfinex,
        description: "Live Bitfinex BTC/USD depth; --symbol tETHUSD selects another instrument",
        argv: &[
            "book",
            "--source",
            "bitfinex",
            "--aggregation",
            "ticks",
            "--bar-size",
            "100",
        ],
    },
    Definition {
        name: Preset::Kraken,
        description: "Live Kraken BTC/USD five-second candles",
        argv: &[
            "candles",
            "--source",
            "kraken",
            "--aggregation",
            "time",
            "--bar-size",
            "5",
        ],
    },
    Definition {
        name: Preset::Fifo,
        description: "Offline L3 FIFO queue",
        argv: &["orders"],
    },
    Definition {
        name: Preset::Flow,
        description: "Net visible liquidity changes over 120 seconds",
        argv: &["flow", "--history-seconds", "120"],
    },
];

pub fn resolve(matches: &ArgMatches) -> Result<Args, clap::Error> {
    let mut args = Args::from_arg_matches(matches)?;
    let Some(preset) = args.preset else {
        return Ok(args);
    };
    let definition = PRESETS.iter().find(|p| p.name == preset).unwrap();
    let defaults =
        Args::try_parse_from(std::iter::once("lobo").chain(definition.argv.iter().copied()))?;
    if args.command.is_none() {
        args.command = defaults.command;
    }
    let supplied = |name: &str| {
        matches!(
            matches.value_source(name),
            Some(ValueSource::CommandLine | ValueSource::EnvVariable)
        )
    };
    macro_rules! fill {
        ($($field:ident),*) => { $(
            if definition.argv.contains(&concat!("--", stringify!($field)).replace('_', "-").as_str()) && !supplied(stringify!($field)) {
                args.$field = defaults.$field;
            }
        )* };
    }
    fill!(
        source,
        aggregation,
        bar_size,
        order_kind,
        quantity,
        history_seconds
    );
    Ok(args)
}

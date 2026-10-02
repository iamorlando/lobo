use anyhow::{Result, anyhow, bail};
use ratatui::style::Color;
use std::{io::Read, path::PathBuf, time::Duration};

#[derive(Clone, Debug)]
pub struct Theme {
    pub name: String,
    pub bg: Color,
    pub fg: Color,
    pub bid: Color,
    pub ask: Color,
    pub accent: Color,
    pub simulation: Color,
    pub muted: Color,
    pub grid: Color,
}
pub const BUILTINS: &[&str] = &[
    "Acid Lime",
    "Dracula",
    "Tokyo Night",
    "Nord",
    "GitHub Light",
];
impl Theme {
    pub fn load(name: &str, file: Option<&PathBuf>) -> Result<Self> {
        if let Some(file) = file {
            return Self::parse(&file.display().to_string(), &std::fs::read_to_string(file)?);
        }
        if let Some(theme) = Self::builtin(name) {
            return Ok(theme);
        }
        if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
            bail!("invalid theme name");
        }
        let cache = cache_dir().join(format!("{name}.toml"));
        if let Ok(text) = std::fs::read_to_string(&cache)
            && let Ok(theme) = Self::parse(name, &text)
        {
            return Ok(theme);
        }
        let url = format!(
            "https://raw.githubusercontent.com/mbadolato/iTerm2-Color-Schemes/master/wezterm/{}.toml",
            encode(name)
        );
        let text = fetch(&url, 65536)?;
        let theme = Self::parse(name, &text)?;
        if std::fs::create_dir_all(cache_dir()).is_ok() {
            let _ = std::fs::write(cache, &text);
        }
        Ok(theme)
    }
    pub fn parse(name: &str, source: &str) -> Result<Self> {
        let doc: toml::Value = toml::from_str(source)?;
        let colors = doc.get("colors").unwrap_or(&doc);
        let get = |key| -> Result<Color> {
            hex(colors
                .get(key)
                .and_then(toml::Value::as_str)
                .ok_or_else(|| anyhow!("theme missing {key}"))?)
        };
        let ansi = colors
            .get("ansi")
            .and_then(toml::Value::as_array)
            .ok_or_else(|| anyhow!("theme missing ansi"))?;
        if ansi.len() != 8 {
            bail!("theme needs eight ANSI colors");
        }
        let ansi: Vec<Color> = ansi
            .iter()
            .map(|v| hex(v.as_str().ok_or_else(|| anyhow!("invalid ANSI color"))?))
            .collect::<Result<_>>()?;
        let bg = get("background")?;
        let fg = get("foreground")?;
        Ok(Self {
            name: name.into(),
            bg,
            fg,
            bid: ansi[2],
            ask: ansi[1],
            accent: ansi[3],
            simulation: ansi[5],
            muted: mix(bg, fg, 0.55),
            grid: mix(bg, fg, 0.10),
        })
    }
    fn builtin(name: &str) -> Option<Self> {
        let (bg, fg, bid, ask, accent, sim) = match name.to_ascii_lowercase().as_str() {
            "acid lime" => (0x101511, 0xd5dfcb, 0xa4e400, 0xff5364, 0xd6ef39, 0xffb5aa),
            "dracula" => (0x282a36, 0xf8f8f2, 0x50fa7b, 0xff5555, 0xf1fa8c, 0xff79c6),
            "tokyo night" => (0x1a1b26, 0xc0caf5, 0x9ece6a, 0xf7768e, 0xe0af68, 0xbb9af7),
            "nord" => (0x2e3440, 0xeceff4, 0xa3be8c, 0xbf616a, 0xebcb8b, 0xb48ead),
            "github light" | "github light default" => {
                (0xffffff, 0x24292f, 0x116329, 0xcf222e, 0x9a6700, 0x8250df)
            }
            "system" => {
                return Self::builtin(
                    if std::env::var("COLORFGBG").is_ok_and(|s| s.ends_with(";15")) {
                        "GitHub Light"
                    } else {
                        "Acid Lime"
                    },
                );
            }
            _ => return None,
        };
        let bg = packed(bg);
        let fg = packed(fg);
        Some(Self {
            name: name.into(),
            bg,
            fg,
            bid: packed(bid),
            ask: packed(ask),
            accent: packed(accent),
            simulation: packed(sim),
            muted: mix(bg, fg, 0.55),
            grid: mix(bg, fg, 0.10),
        })
    }
}
pub fn packed(value: u32) -> Color {
    Color::Rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}
pub fn rgb(color: Color) -> [f32; 4] {
    if let Color::Rgb(r, g, b) = color {
        [
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0,
            1.0,
        ]
    } else {
        [0.0; 4]
    }
}
pub fn mix(a: Color, b: Color, weight: f64) -> Color {
    let a = rgb(a);
    let b = rgb(b);
    Color::Rgb(
        ((f64::from(a[0]) + (f64::from(b[0] - a[0])) * weight) * 255.0).round() as u8,
        ((f64::from(a[1]) + (f64::from(b[1] - a[1])) * weight) * 255.0).round() as u8,
        ((f64::from(a[2]) + (f64::from(b[2] - a[2])) * weight) * 255.0).round() as u8,
    )
}
fn hex(s: &str) -> Result<Color> {
    if s.len() != 7 || !s.starts_with('#') {
        bail!("expected #rrggbb color");
    }
    Ok(packed(u32::from_str_radix(&s[1..], 16)?))
}
pub fn fetch(url: &str, max: u64) -> Result<String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent("lobo-cli/0.1")
        .build()?;
    let response = client.get(url).send()?.error_for_status()?;
    let mut text = String::new();
    response.take(max + 1).read_to_string(&mut text)?;
    if text.len() as u64 > max {
        bail!("response exceeds {max} bytes");
    }
    Ok(text)
}
pub fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache")
        })
        .join("lobo/themes")
}

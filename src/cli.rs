use anstyle::{Color, RgbColor, Style};
use clap::Parser;
use clap::builder::Styles;
use std::path::PathBuf;

// ── Synthwave palette ─────────────────────────────────────────────────────────
// Hot pink, electric purple, neon cyan, sunset yellow.

const HOT_PINK: Color = Color::Rgb(RgbColor(0xff, 0x2a, 0x6d));      // #ff2a6d
const ELECTRIC_CYAN: Color = Color::Rgb(RgbColor(0x05, 0xd9, 0xe8)); // #05d9e8
const NEON_MAGENTA: Color = Color::Rgb(RgbColor(0xff, 0x71, 0xce));  // #ff71ce
const ELECTRIC_PURPLE: Color = Color::Rgb(RgbColor(0xb9, 0x67, 0xff)); // #b967ff
const SUNSET_YELLOW: Color = Color::Rgb(RgbColor(0xff, 0xd3, 0x19));  // #ffd319
const LASER_RED: Color = Color::Rgb(RgbColor(0xff, 0x00, 0x6e));      // #ff006e

pub fn neon_styles() -> Styles {
    Styles::styled()
        .header(Style::new().bold().fg_color(Some(HOT_PINK)))
        .usage(Style::new().bold().fg_color(Some(ELECTRIC_CYAN)))
        .literal(Style::new().bold().fg_color(Some(NEON_MAGENTA)))
        .placeholder(Style::new().fg_color(Some(ELECTRIC_PURPLE)))
        .error(Style::new().bold().fg_color(Some(LASER_RED)))
        .valid(Style::new().bold().fg_color(Some(ELECTRIC_CYAN)))
        .invalid(Style::new().bold().fg_color(Some(SUNSET_YELLOW)))
}

#[derive(Debug, Parser)]
#[command(
    name = "smolsonic",
    version,
    about = "A tiny Subsonic-compatible music server",
    styles = neon_styles(),
)]
pub struct Cli {
    /// Path to the TOML config file.
    #[arg(short, long, default_value = "smolsonic.toml")]
    pub config: PathBuf,

    /// Skip the startup library scan.
    #[arg(long)]
    pub no_scan: bool,
}

pub const BANNER: &str = r#"
                     _                  _
 ___ _ __ ___   ___ | |___  ___  _ __  (_) ___
/ __| '_ ` _ \ / _ \| / __|/ _ \| '_ \ | |/ __|
\__ \ | | | | | (_) | \__ \ (_) | | | || | (__
|___/_| |_| |_|\___/|_|___/\___/|_| |_||_|\___|
                a tiny Subsonic server in Rust
"#;

pub fn print_banner(
    host: &str,
    port: u16,
    music_dir: &std::path::Path,
    s3_endpoint: Option<(&str, u16)>,
    jellyfin_endpoint: Option<(&str, u16)>,
) {
    // Synthwave 24-bit colors — match neon_styles().
    let hot_pink = "\x1b[1;38;2;255;42;109m";    // #ff2a6d
    let electric_cyan = "\x1b[1;38;2;5;217;232m"; // #05d9e8
    let neon_magenta = "\x1b[1;38;2;255;113;206m"; // #ff71ce
    let electric_purple = "\x1b[1;38;2;185;103;255m"; // #b967ff
    let sunset_yellow = "\x1b[1;38;2;255;211;25m"; // #ffd319
    let dim = "\x1b[2m";
    let reset = "\x1b[0m";

    print!("{hot_pink}{BANNER}{reset}");
    println!(
        "  {electric_cyan}subsonic{reset} {dim}→{reset} {neon_magenta}http://{host}:{port}{reset}"
    );
    if let Some((s3_host, s3_port)) = s3_endpoint {
        println!(
            "  {electric_cyan}s3      {reset} {dim}→{reset} {sunset_yellow}http://{s3_host}:{s3_port}{reset}"
        );
    }
    if let Some((jf_host, jf_port)) = jellyfin_endpoint {
        println!(
            "  {electric_cyan}jellyfin{reset} {dim}→{reset} {hot_pink}http://{jf_host}:{jf_port}{reset}"
        );
    }
    println!(
        "  {electric_cyan}library {reset} {dim}→{reset} {electric_purple}{}{reset}",
        music_dir.display()
    );
    println!();
}

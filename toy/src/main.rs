//! russl toy: write russl on the left, watch it draw on the right.
//!
//! Run with: cargo run --release (in `toy/`).

use blinc_app::prelude::*;
use blinc_app::windowed::WindowedApp;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();
    let config = WindowConfig {
        title: "russl toy".to_string(),
        width: 1400,
        height: 860,
        resizable: true,
        ..Default::default()
    };
    blinc_app::register_font(russl_toy::ui::CODE_FONT.to_vec());
    // A closure, not the fn: the runner's bound is higher-ranked over the context's lifetime.
    #[allow(clippy::redundant_closure)]
    WindowedApp::run_with_theme(
        config,
        russl_toy::ui::theme_bundle(),
        blinc_theme::ColorScheme::Dark,
        |ctx| russl_toy::ui::build_ui(ctx),
    )
}

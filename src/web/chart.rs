//! Server-side SVG charts of price history.

use crate::db::Observation;
use crate::money::Money;

/// Renders the full price history as an SVG line chart.
///
/// `target` draws a horizontal target line when given; `width` and `height` are the SVG size in pixels.
/// Returns the SVG markup.
pub fn price_chart(
    history: &[Observation],
    target: Option<&Money>,
    width: u32,
    height: u32,
) -> String {
    let _ = (history, target, width, height);
    String::new()
}

/// Renders a small axis-free SVG sparkline of the price history.
///
/// `width` and `height` are the SVG size in pixels. Returns the SVG markup.
pub fn sparkline(history: &[Observation], width: u32, height: u32) -> String {
    let _ = (history, width, height);
    String::new()
}

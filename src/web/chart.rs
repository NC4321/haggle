//! Server-side SVG charts of price history.

use crate::db::Observation;
use crate::extract::Availability;
use crate::money::{Currency, Money};

/// Inset of the price labels from the left edge, in pixels.
const LABEL_LEFT: f64 = 4.0;
/// Baseline of the minimum-price label, above the bottom edge, in pixels.
const LABEL_BOTTOM_INSET: f64 = 4.0;
/// Baseline of the maximum-price label, below the top edge, in pixels.
const LABEL_TOP_BASELINE: f64 = 12.0;

/// Renders the full price history as an SVG line chart.
///
/// `target` draws a horizontal target line when given; `width` and `height` are the SVG size in pixels.
/// Returns the SVG markup. An empty `history` gives the root element with a "no data yet" note instead.
pub fn price_chart(
    history: &[Observation],
    target: Option<&Money>,
    width: u32,
    height: u32,
) -> String {
    let target = target.copied();
    let Some(scale) = Scale::new(history, target, width, height) else {
        let note = format!(
            r#"<text class="empty" x="{:.1}" y="{:.1}" text-anchor="middle">no data yet</text>"#,
            f64::from(width) / 2.0,
            f64::from(height) / 2.0,
        );
        return root("chart", width, height, &note);
    };
    let mut body = scale.gaps(history);
    body.push_str(&scale.polylines(history));
    if let Some(target) = target {
        body.push_str(&scale.target_line(target));
    }
    body.push_str(&scale.labels());
    root("chart", width, height, &body)
}

/// Renders a small axis-free SVG sparkline of the price history.
///
/// `width` and `height` are the SVG size in pixels. Returns the SVG markup. An empty `history`
/// gives the root element with nothing inside.
pub fn sparkline(history: &[Observation], width: u32, height: u32) -> String {
    let body = match Scale::new(history, None, width, height) {
        Some(scale) => {
            let mut body = scale.gaps(history);
            body.push_str(&scale.polylines(history));
            body
        }
        None => String::new(),
    };
    root("sparkline", width, height, &body)
}

/// Wraps `children` in the root `svg` element of the given class and pixel size.
fn root(class: &str, width: u32, height: u32, children: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" class="{class}" viewBox="0 0 {width} {height}" width="{width}" height="{height}">{children}</svg>"#
    )
}

/// The prices that set the vertical range: the in-stock prices plus `target`.
///
/// When nothing is in stock the labels still need a range, so every price is used instead.
fn price_domain(history: &[Observation], target: Option<Money>) -> Vec<i64> {
    let in_stock: Vec<i64> = history
        .iter()
        .filter(|observation| observation.availability != Availability::OutOfStock)
        .map(|observation| observation.price.minor)
        .collect();
    let mut prices = if in_stock.is_empty() {
        history
            .iter()
            .map(|observation| observation.price.minor)
            .collect()
    } else {
        in_stock
    };
    prices.extend(target.map(|money| money.minor));
    prices
}

/// Position of `value` between `min` and `max` as a fraction, or `None` when the range is empty.
fn span_fraction(value: i64, min: i64, max: i64) -> Option<f64> {
    if max == min {
        return None;
    }
    Some((value - min) as f64 / (max - min) as f64)
}

/// Maps observation times and prices onto the pixel box of one chart.
struct Scale {
    time_min: i64,
    time_max: i64,
    price_min: i64,
    price_max: i64,
    currency: Currency,
    width: u32,
    height: u32,
}

impl Scale {
    /// Builds the scale for `history` in a `width` by `height` box, or `None` when `history` is empty.
    ///
    /// The price range also covers `target` when given.
    fn new(
        history: &[Observation],
        target: Option<Money>,
        width: u32,
        height: u32,
    ) -> Option<Scale> {
        let first = history.first()?;
        let times: Vec<i64> = history.iter().map(|observation| observation.at).collect();
        let prices = price_domain(history, target);
        Some(Scale {
            time_min: *times.iter().min()?,
            time_max: *times.iter().max()?,
            price_min: *prices.iter().min()?,
            price_max: *prices.iter().max()?,
            currency: first.price.currency,
            width,
            height,
        })
    }

    /// Horizontal pixel position of time `at`, centred when all observations share one time.
    fn x_at(&self, at: i64) -> f64 {
        let width = f64::from(self.width);
        span_fraction(at, self.time_min, self.time_max)
            .map_or(width / 2.0, |fraction| fraction * width)
    }

    /// Vertical pixel position of `minor`, centred when every price is equal.
    fn y_at(&self, minor: i64) -> f64 {
        let height = f64::from(self.height);
        span_fraction(minor, self.price_min, self.price_max)
            .map_or(height / 2.0, |fraction| height - fraction * height)
    }

    /// One `gap` rect per out-of-stock observation, running to the next observation's time.
    ///
    /// The last observation's gap runs to the right edge.
    fn gaps(&self, history: &[Observation]) -> String {
        history
            .iter()
            .enumerate()
            .filter(|(_, observation)| observation.availability == Availability::OutOfStock)
            .map(|(index, observation)| {
                let start = self.x_at(observation.at);
                let end = history
                    .get(index + 1)
                    .map_or(f64::from(self.width), |next| self.x_at(next.at));
                format!(
                    r#"<rect class="gap" x="{start:.1}" y="0" width="{:.1}" height="{}"/>"#,
                    end - start,
                    self.height,
                )
            })
            .collect()
    }

    /// One `price` polyline per run of consecutive in-stock observations.
    fn polylines(&self, history: &[Observation]) -> String {
        history
            .split(|observation| observation.availability == Availability::OutOfStock)
            .filter(|run| !run.is_empty())
            .map(|run| {
                let points: Vec<String> = run
                    .iter()
                    .map(|observation| self.point(observation))
                    .collect();
                format!(r#"<polyline class="price" points="{}"/>"#, points.join(" "))
            })
            .collect()
    }

    /// The `x,y` pair of one observation.
    fn point(&self, observation: &Observation) -> String {
        format!(
            "{:.1},{:.1}",
            self.x_at(observation.at),
            self.y_at(observation.price.minor),
        )
    }

    /// The horizontal target line at `target`'s price, spanning the full width.
    fn target_line(&self, target: Money) -> String {
        let y = self.y_at(target.minor);
        format!(
            r#"<line class="target" x1="0" y1="{y:.1}" x2="{}" y2="{y:.1}"/>"#,
            self.width,
        )
    }

    /// The minimum price near the bottom-left corner and the maximum near the top-left.
    fn labels(&self) -> String {
        let minimum = Money {
            minor: self.price_min,
            currency: self.currency,
        };
        let maximum = Money {
            minor: self.price_max,
            currency: self.currency,
        };
        format!(
            r#"<text class="label" x="{LABEL_LEFT:.1}" y="{:.1}">{minimum}</text><text class="label" x="{LABEL_LEFT:.1}" y="{LABEL_TOP_BASELINE:.1}">{maximum}</text>"#,
            f64::from(self.height) - LABEL_BOTTOM_INSET,
        )
    }
}

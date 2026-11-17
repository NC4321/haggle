//! A fake shop on localhost whose prices and stock follow a scripted timeline,
//! so the demo can show price drops, stock changes and inflated "was" prices.

use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Html;
use axum::routing::get;
use serde_json::{Value, json};
use url::Url;

use crate::extract::Availability;
use crate::money::Currency;

const SECONDS_PER_DAY: i64 = 86_400;
const ROBOTS_TXT: &str = "User-agent: *\nAllow: /\n";
const LIST_PRICE_TYPE_URL: &str = "https://schema.org/ListPrice";

/// A demo clock: runs `speed` times faster than real time from a given start.
#[derive(Debug)]
pub struct DemoClock {
    start: Instant,
    start_unix: i64,
    speed: i64,
}

impl DemoClock {
    /// Starts a clock reading `start_unix` (Unix seconds) now, advancing
    /// `speed` demo seconds per real second.
    pub fn new(start_unix: i64, speed: i64) -> DemoClock {
        DemoClock {
            start: Instant::now(),
            start_unix,
            speed,
        }
    }

    /// Returns the current demo time in Unix seconds:
    /// `start_unix + real elapsed seconds * speed`.
    pub fn now(&self) -> i64 {
        let elapsed = i64::try_from(self.start.elapsed().as_secs()).unwrap_or(i64::MAX);
        self.start_unix
            .saturating_add(elapsed.saturating_mul(self.speed))
    }
}

/// One point of a product's timeline: the state it has from `offset_s` on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    /// Demo seconds from the demo's start; negative values are past history.
    pub offset_s: i64,
    /// Selling price in minor units of the product's currency.
    pub price_minor: i64,
    /// Advertised "was" price in minor units, if the shop shows one.
    pub list_price_minor: Option<i64>,
    /// Stock state shown on the page.
    pub availability: Availability,
}

/// A product of the fake shop with its scripted timeline.
#[derive(Debug)]
pub struct Product {
    /// URL path segment under `/products/`.
    pub slug: &'static str,
    /// Display name.
    pub name: &'static str,
    /// Currency of every step's prices.
    pub currency: Currency,
    /// Timeline steps, sorted by ascending `offset_s`; never empty.
    pub steps: &'static [Step],
}

const fn step(
    offset_s: i64,
    price_minor: i64,
    list_price_minor: Option<i64>,
    availability: Availability,
) -> Step {
    Step {
        offset_s,
        price_minor,
        list_price_minor,
        availability,
    }
}

/// The products the fake shop sells.
pub const PRODUCTS: &[Product] = &[
    Product {
        slug: "kettle",
        name: "Steel kettle",
        currency: Currency::Eur,
        steps: &[
            step(-30 * SECONDS_PER_DAY, 4999, None, Availability::InStock),
            step(-12 * SECONDS_PER_DAY, 4499, None, Availability::InStock),
            step(-3 * SECONDS_PER_DAY, 5499, None, Availability::InStock),
            step(1200, 3999, None, Availability::InStock),
        ],
    },
    Product {
        slug: "headphones",
        name: "Wireless headphones",
        currency: Currency::Eur,
        steps: &[
            step(-30 * SECONDS_PER_DAY, 12900, None, Availability::InStock),
            step(-2 * SECONDS_PER_DAY, 12900, None, Availability::OutOfStock),
            step(1800, 11900, None, Availability::InStock),
        ],
    },
    Product {
        slug: "blender",
        name: "Countertop blender",
        currency: Currency::Eur,
        steps: &[
            step(
                -30 * SECONDS_PER_DAY,
                8900,
                Some(14900),
                Availability::InStock,
            ),
            step(
                -5 * SECONDS_PER_DAY,
                7900,
                Some(14900),
                Availability::InStock,
            ),
        ],
    },
];

/// Returns the step in force at `offset_s`: the last one whose offset is at
/// most `offset_s`, or the first step if none is yet.
pub fn state_at(product: &Product, offset_s: i64) -> &Step {
    product
        .steps
        .iter()
        .rev()
        .find(|candidate| candidate.offset_s <= offset_s)
        .unwrap_or(&product.steps[0])
}

/// Returns the URL of the product's page on the shop served at `base`.
pub fn url_for(base: &Url, product: &Product) -> Url {
    let mut url = base.clone();
    url.set_path(&format!("/products/{}", product.slug));
    url
}

/// Formats minor units as a decimal string, e.g. 4999 EUR becomes "49.99".
fn decimal_price(minor: i64, currency: Currency) -> String {
    let exponent = currency.exponent();
    if exponent == 0 {
        return minor.to_string();
    }
    let divisor = 10_i64.pow(exponent);
    format!(
        "{}.{:0width$}",
        minor / divisor,
        minor % divisor,
        width = exponent as usize
    )
}

fn schema_availability_url(availability: Availability) -> &'static str {
    match availability {
        Availability::InStock => "https://schema.org/InStock",
        Availability::OutOfStock => "https://schema.org/OutOfStock",
        Availability::Preorder => "https://schema.org/PreOrder",
        Availability::Unknown => "https://schema.org/Discontinued",
    }
}

fn offer_json(product: &Product, step: &Step) -> Value {
    let code = product.currency.code();
    let price = decimal_price(step.price_minor, product.currency);
    let availability = schema_availability_url(step.availability);
    match step.list_price_minor {
        None => json!({
            "@type": "Offer",
            "price": price,
            "priceCurrency": code,
            "availability": availability,
        }),
        Some(list_price_minor) => json!({
            "@type": "Offer",
            "priceCurrency": code,
            "availability": availability,
            "priceSpecification": [
                {
                    "@type": "UnitPriceSpecification",
                    "priceType": LIST_PRICE_TYPE_URL,
                    "price": decimal_price(list_price_minor, product.currency),
                    "priceCurrency": code,
                },
                {
                    "@type": "UnitPriceSpecification",
                    "price": price,
                    "priceCurrency": code,
                },
            ],
        }),
    }
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Renders the product page for `step`, with a JSON-LD Product/Offer carrying
/// the price as a decimal string, `priceCurrency` and a schema.org
/// availability URL.
pub fn page_html(product: &Product, step: &Step) -> String {
    let document = json!({
        "@context": "https://schema.org",
        "@type": "Product",
        "name": product.name,
        "offers": offer_json(product, step),
    });
    // "</" would end the script element early.
    let json_ld = document.to_string().replace("</", "<\\/");
    let name = escape_html(product.name);
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n  <meta charset=\"utf-8\">\n  \
         <title>{name} - Demo Shop</title>\n  \
         <script type=\"application/ld+json\">{json_ld}</script>\n</head>\n\
         <body>\n  <main>\n    <h1>{name}</h1>\n  </main>\n</body>\n</html>\n"
    )
}

async fn product_page(
    State(clock): State<Arc<DemoClock>>,
    Path(slug): Path<String>,
) -> Result<Html<String>, StatusCode> {
    let product = PRODUCTS
        .iter()
        .find(|product| product.slug == slug)
        .ok_or(StatusCode::NOT_FOUND)?;
    let offset_s = clock.now().saturating_sub(clock.start_unix);
    Ok(Html(page_html(product, state_at(product, offset_s))))
}

async fn robots_txt() -> &'static str {
    ROBOTS_TXT
}

/// Builds the shop's router: `GET /products/{slug}` serves the product's page
/// for the state at the clock's current offset (404 for unknown slugs), and
/// `GET /robots.txt` allows everything.
pub fn router(clock: Arc<DemoClock>) -> Router {
    Router::new()
        .route("/products/{slug}", get(product_page))
        .route("/robots.txt", get(robots_txt))
        .with_state(clock)
}

use std::sync::Arc;

use haggle::demo_shop::{DemoClock, PRODUCTS, Product, Step, page_html, router, state_at};
use haggle::extract::{Availability, Extraction, extract};
use haggle::money::Currency;
use tokio::net::TcpListener;

/// Seconds in a day, so that the timeline offsets below read as days.
const DAY: i64 = 86_400;

/// Steps for the test product, sorted by offset as the spec requires.
const TEST_STEPS: &[Step] = &[
    Step {
        offset_s: 100,
        price_minor: 1000,
        list_price_minor: None,
        availability: Availability::InStock,
    },
    Step {
        offset_s: 200,
        price_minor: 2000,
        list_price_minor: None,
        availability: Availability::OutOfStock,
    },
    Step {
        offset_s: 300,
        price_minor: 3000,
        list_price_minor: Some(4000),
        availability: Availability::InStock,
    },
];

/// A product built by the test itself, so that `state_at` can be checked on known steps.
static TEST_PRODUCT: Product = Product {
    slug: "test-product",
    name: "Test product",
    currency: Currency::Eur,
    steps: TEST_STEPS,
};

/// The demo product with the given slug. Fails the test if the shop does not sell it.
fn product(slug: &str) -> &'static Product {
    match PRODUCTS.iter().find(|product| product.slug == slug) {
        Some(product) => product,
        None => panic!("the demo shop has no product {slug:?}"),
    }
}

/// The step in force for a demo product at `offset_s`.
fn step_at(slug: &str, offset_s: i64) -> &'static Step {
    state_at(product(slug), offset_s)
}

/// Runs a step through `page_html` and the real extractor. Fails the test if no price is found.
fn extract_step(product: &Product, step: &Step) -> Extraction {
    match extract(&page_html(product, step), None) {
        Ok(extraction) => extraction,
        Err(error) => panic!(
            "{} at offset {}: no price found: {error:?}",
            product.slug, step.offset_s
        ),
    }
}

/// A clock with speed 0 always reads its start time, so the shop stays at demo offset 0.
fn frozen_clock() -> DemoClock {
    DemoClock::new(1_000, 0)
}

/// Serves the router on 127.0.0.1 with a free port and returns the base URL.
async fn start_shop(clock: DemoClock) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a localhost port");
    let addr = listener.local_addr().expect("read the bound address");
    let app = router(Arc::new(clock));
    let _server = tokio::spawn(async move { axum::serve(listener, app).await });
    format!("http://{addr}")
}

/// Sends a GET request for `path` to the shop at `base`.
async fn get(base: &str, path: &str) -> reqwest::Response {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build the HTTP client")
        .get(format!("{base}{path}"))
        .send()
        .await
        .expect("send the request")
}

#[test]
fn state_before_the_first_step_is_the_first_step() {
    // Offset -1000 is before every step, so the first step (offset 100) applies.
    let step = state_at(&TEST_PRODUCT, -1000);

    assert_eq!(step.price_minor, 1000);
}

#[test]
fn state_exactly_at_a_step_offset_is_that_step() {
    // Offset 200 is the second step's offset; the offset is inclusive.
    let step = state_at(&TEST_PRODUCT, 200);

    assert_eq!(step.price_minor, 2000);
    assert_eq!(step.availability, Availability::OutOfStock);
}

#[test]
fn state_one_second_before_a_step_offset_is_the_earlier_step() {
    // Offset 199 is before the step at 200, so the step at 100 still applies.
    let step = state_at(&TEST_PRODUCT, 199);

    assert_eq!(step.price_minor, 1000);
}

#[test]
fn state_between_two_steps_is_the_earlier_step() {
    // Offset 250 is after 200 and before 300, so the step at 200 applies.
    let step = state_at(&TEST_PRODUCT, 250);

    assert_eq!(step.price_minor, 2000);
}

#[test]
fn state_after_the_last_step_is_the_last_step() {
    // Offset 10 000 is after every step, so the last step (offset 300) applies.
    let step = state_at(&TEST_PRODUCT, 10_000);

    assert_eq!(step.price_minor, 3000);
    assert_eq!(step.list_price_minor, Some(4000));
}

#[test]
fn clock_started_at_1000_reads_about_1000_just_after_creation() {
    // Speed 60 adds up to a minute of demo time per real second; right after creation the
    // real elapsed time is well under a second, so the reading stays within 60 of 1000.
    let clock = DemoClock::new(1000, 60);

    assert!(
        (clock.now() - 1000).abs() <= 60,
        "clock read {}",
        clock.now()
    );
}

#[test]
fn clock_with_speed_zero_always_reads_its_start_time() {
    // Speed 0 multiplies the elapsed real time by 0, so the reading is exactly the start.
    let clock = DemoClock::new(1000, 0);

    assert_eq!(clock.now(), 1000);
}

#[test]
fn kettle_is_named_steel_kettle_and_priced_in_euros() {
    let kettle = product("kettle");

    assert_eq!(kettle.name, "Steel kettle");
    assert_eq!(kettle.currency, Currency::Eur);
}

#[test]
fn headphones_are_named_wireless_headphones_and_priced_in_euros() {
    let headphones = product("headphones");

    assert_eq!(headphones.name, "Wireless headphones");
    assert_eq!(headphones.currency, Currency::Eur);
}

#[test]
fn blender_is_named_countertop_blender_and_priced_in_euros() {
    let blender = product("blender");

    assert_eq!(blender.name, "Countertop blender");
    assert_eq!(blender.currency, Currency::Eur);
}

#[test]
fn kettle_costs_49_99_thirty_days_before_the_start() {
    // Offset -30 days is exactly the first kettle step, 49.99 EUR.
    assert_eq!(step_at("kettle", -30 * DAY).price_minor, 4999);
}

#[test]
fn kettle_drops_to_44_99_twelve_days_before_the_start() {
    // Offset -12 days is exactly the -12 day step.
    assert_eq!(step_at("kettle", -12 * DAY).price_minor, 4499);
}

#[test]
fn kettle_rises_to_54_99_three_days_before_the_start() {
    // Offset -3 days is exactly the -3 day step.
    assert_eq!(step_at("kettle", -3 * DAY).price_minor, 5499);
}

#[test]
fn kettle_costs_54_99_at_demo_start() {
    // Offset 0 is after the -3 day step and before the drop at 1200.
    assert_eq!(step_at("kettle", 0).price_minor, 5499);
}

#[test]
fn kettle_still_costs_54_99_one_second_before_the_drop() {
    assert_eq!(step_at("kettle", 1199).price_minor, 5499);
}

#[test]
fn kettle_drops_to_39_99_at_offset_1200() {
    // Offset 1200 is 20 real seconds at speed 60: the price falls at that offset.
    assert_eq!(step_at("kettle", 1200).price_minor, 3999);
}

#[test]
fn kettle_stays_at_39_99_at_the_end_of_the_first_minute() {
    assert_eq!(step_at("kettle", 3600).price_minor, 3999);
}

#[test]
fn headphones_are_in_stock_at_129_00_thirty_days_before_the_start() {
    let step = step_at("headphones", -30 * DAY);

    assert_eq!(step.price_minor, 12900);
    assert_eq!(step.availability, Availability::InStock);
}

#[test]
fn headphones_are_still_in_stock_one_second_before_going_out_of_stock() {
    // Offset -2 days minus one second is before the -2 day step, so the -30 day step applies.
    let step = step_at("headphones", -2 * DAY - 1);

    assert_eq!(step.availability, Availability::InStock);
}

#[test]
fn headphones_are_out_of_stock_two_days_before_the_start() {
    // Offset -2 days is exactly the -2 day step.
    let step = step_at("headphones", -2 * DAY);

    assert_eq!(step.availability, Availability::OutOfStock);
}

#[test]
fn headphones_are_out_of_stock_at_demo_start() {
    assert_eq!(
        step_at("headphones", 0).availability,
        Availability::OutOfStock
    );
}

#[test]
fn headphones_are_still_out_of_stock_one_second_before_offset_1800() {
    assert_eq!(
        step_at("headphones", 1799).availability,
        Availability::OutOfStock
    );
}

#[test]
fn headphones_are_back_in_stock_at_119_00_at_offset_1800() {
    // Offset 1800 is 30 real seconds at speed 60: the restock happens at that offset.
    let step = step_at("headphones", 1800);

    assert_eq!(step.price_minor, 11900);
    assert_eq!(step.availability, Availability::InStock);
}

#[test]
fn blender_costs_89_00_with_a_149_00_list_price_thirty_days_before_the_start() {
    let step = step_at("blender", -30 * DAY);

    assert_eq!(step.price_minor, 8900);
    assert_eq!(step.list_price_minor, Some(14900));
}

#[test]
fn blender_still_costs_89_00_one_second_before_the_five_day_drop() {
    assert_eq!(step_at("blender", -5 * DAY - 1).price_minor, 8900);
}

#[test]
fn blender_drops_to_79_00_five_days_before_the_start() {
    // Offset -5 days is exactly the -5 day step.
    assert_eq!(step_at("blender", -5 * DAY).price_minor, 7900);
}

#[test]
fn blender_costs_79_00_at_demo_start() {
    assert_eq!(step_at("blender", 0).price_minor, 7900);
}

#[test]
fn page_html_gives_the_price_as_a_quoted_decimal_string() {
    // The kettle's first step is 49.99 EUR; JSON-LD carries the price as a string.
    let kettle = product("kettle");
    let html = page_html(kettle, &kettle.steps[0]);

    assert!(html.contains("\"49.99\""), "page has no quoted price");
}

#[test]
fn page_html_gives_the_currency_code() {
    let kettle = product("kettle");
    let html = page_html(kettle, &kettle.steps[0]);

    assert!(html.contains("\"EUR\""), "page has no quoted currency code");
}

#[test]
fn every_step_page_extracts_its_price_in_minor_units() {
    for product in PRODUCTS {
        for step in product.steps {
            let found = extract_step(product, step);

            assert_eq!(
                found.price.minor, step.price_minor,
                "{} at offset {}",
                product.slug, step.offset_s
            );
        }
    }
}

#[test]
fn every_step_page_extracts_its_currency() {
    for product in PRODUCTS {
        for step in product.steps {
            let found = extract_step(product, step);

            assert_eq!(
                found.price.currency, product.currency,
                "{} at offset {}",
                product.slug, step.offset_s
            );
        }
    }
}

#[test]
fn every_step_page_extracts_its_availability() {
    for product in PRODUCTS {
        for step in product.steps {
            let found = extract_step(product, step);

            assert_eq!(
                found.availability, step.availability,
                "{} at offset {}",
                product.slug, step.offset_s
            );
        }
    }
}

#[test]
fn every_step_page_extracts_the_product_name_as_title() {
    for product in PRODUCTS {
        for step in product.steps {
            let found = extract_step(product, step);

            assert_eq!(
                found.title.as_deref(),
                Some(product.name),
                "{} at offset {}",
                product.slug,
                step.offset_s
            );
        }
    }
}

#[tokio::test]
async fn product_page_answers_200_ok() {
    let base = start_shop(frozen_clock()).await;

    let response = get(&base, "/products/kettle").await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
}

#[tokio::test]
async fn product_page_is_served_as_html() {
    let base = start_shop(frozen_clock()).await;

    let response = get(&base, "/products/kettle").await;

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .expect("the response has a content type")
        .to_str()
        .expect("the content type is ASCII");
    assert!(
        content_type.starts_with("text/html"),
        "content type was {content_type:?}"
    );
}

#[tokio::test]
async fn product_page_carries_the_price_at_demo_start() {
    // Offset 0 is the -3 day kettle step, 54.99 EUR.
    let base = start_shop(frozen_clock()).await;

    let body = get(&base, "/products/kettle")
        .await
        .text()
        .await
        .expect("read the body");

    assert!(body.contains("\"54.99\""), "page has no quoted 54.99 price");
}

#[tokio::test]
async fn product_page_carries_the_stock_state_at_demo_start() {
    // Offset 0 is after the headphones went out of stock at -2 days.
    let base = start_shop(frozen_clock()).await;

    let body = get(&base, "/products/headphones")
        .await
        .text()
        .await
        .expect("read the body");

    assert!(
        body.contains("schema.org/OutOfStock"),
        "page has no OutOfStock availability"
    );
}

#[tokio::test]
async fn unknown_product_slug_answers_404_not_found() {
    let base = start_shop(frozen_clock()).await;

    let response = get(&base, "/products/no-such-product").await;

    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn robots_txt_answers_200_ok() {
    let base = start_shop(frozen_clock()).await;

    let response = get(&base, "/robots.txt").await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
}

#[tokio::test]
async fn robots_txt_allows_every_path() {
    let base = start_shop(frozen_clock()).await;

    let body = get(&base, "/robots.txt")
        .await
        .text()
        .await
        .expect("read the body");

    assert_eq!(body, "User-agent: *\nAllow: /\n");
}

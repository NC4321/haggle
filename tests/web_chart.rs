use haggle::db::Observation;
use haggle::extract::Availability::{InStock, OutOfStock, Preorder, Unknown};
use haggle::extract::{Availability, Source};
use haggle::money::{Currency, Money};
use haggle::web::chart::{price_chart, sparkline};

/// Chart width used by every test, in SVG user units.
const WIDTH: u32 = 100;

/// Chart height used by every test, in SVG user units.
const HEIGHT: u32 = 50;

/// The root element `sparkline` writes at the test size.
const SPARKLINE_ROOT: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" class="sparkline" viewBox="0 0 100 50" width="100" height="50">"#;

/// The root element `price_chart` writes at the test size.
const CHART_ROOT: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" class="chart" viewBox="0 0 100 50" width="100" height="50">"#;

/// A euro amount in minor units (cents).
fn eur(minor: i64) -> Money {
    Money {
        minor,
        currency: Currency::Eur,
    }
}

/// An observation taken at unix second `at` with the given price and stock state.
fn observation(at: i64, minor: i64, availability: Availability) -> Observation {
    Observation {
        id: 0,
        watch_id: 1,
        at,
        price: eur(minor),
        list_price: None,
        availability,
        source: Source::JsonLd,
    }
}

/// Builds a history, oldest first, from `(at, minor, availability)` triples.
fn series(points: Vec<(i64, i64, Availability)>) -> Vec<Observation> {
    points
        .into_iter()
        .map(|(at, minor, availability)| observation(at, minor, availability))
        .collect()
}

/// Four observations at times 0, 10, 20 and 30: in stock at 10000, in stock at 20000, out of stock at
/// 15000, in stock at 15000. The in-stock prices span 10000..20000.
fn reference_series() -> Vec<Observation> {
    series(vec![
        (0, 10000, InStock),
        (10, 20000, InStock),
        (20, 15000, OutOfStock),
        (30, 15000, InStock),
    ])
}

/// How many times `needle` occurs in `svg`.
fn count(svg: &str, needle: &str) -> usize {
    svg.matches(needle).count()
}

/// The text content of every `<text class="label">` element, in document order.
fn label_texts(svg: &str) -> Vec<&str> {
    svg.split("<text ")
        .filter(|part| part.starts_with(r#"class="label""#))
        .map(|part| {
            let start = part
                .find('>')
                .expect("text element has a closing angle bracket")
                + 1;
            let end = part.find("</text>").expect("text element is closed");
            &part[start..end]
        })
        .collect()
}

#[test]
fn sparkline_root_element_has_sparkline_class_and_size() {
    let svg = sparkline(&reference_series(), WIDTH, HEIGHT);

    assert!(svg.starts_with(SPARKLINE_ROOT), "{svg}");
}

#[test]
fn sparkline_splits_in_stock_observations_at_the_gap_into_two_polylines() {
    let svg = sparkline(&reference_series(), WIDTH, HEIGHT);

    assert_eq!(count(&svg, "<polyline"), 2, "{svg}");
}

#[test]
fn sparkline_first_run_places_points_on_the_time_and_price_scales() {
    // Times 0..30 map to x 0..100, so the time 10 lands at 33.3. Prices 10000..20000 map to y 50..0,
    // so 10000 is y 50.0 and 20000 is y 0.0. Points keep the order given.
    let svg = sparkline(&reference_series(), WIDTH, HEIGHT);

    assert!(svg.contains(r#"points="0.0,50.0 33.3,0.0""#), "{svg}");
}

#[test]
fn sparkline_second_run_places_its_single_point_at_the_right_edge() {
    // The time 30 lands at the right edge, 100.0. Price 15000 is halfway between 10000 and 20000,
    // so the height is 25.0.
    let svg = sparkline(&reference_series(), WIDTH, HEIGHT);

    assert!(
        svg.contains(r#"<polyline class="price" points="100.0,25.0"/>"#),
        "{svg}"
    );
}

#[test]
fn sparkline_out_of_stock_observation_becomes_gap_rect_up_to_next_observation() {
    // The time 20 lands at 66.7. The next observation, at time 30, lands at 100.0, so the rect is
    // 33.3 wide.
    let svg = sparkline(&reference_series(), WIDTH, HEIGHT);

    assert_eq!(count(&svg, r#"class="gap""#), 1, "{svg}");
    assert!(
        svg.contains(r#"<rect class="gap" x="66.7" y="0" width="33.3" height="50"/>"#),
        "{svg}"
    );
}

#[test]
fn sparkline_has_no_text_elements() {
    let svg = sparkline(&reference_series(), WIDTH, HEIGHT);

    assert_eq!(count(&svg, "<text"), 0, "{svg}");
}

#[test]
fn sparkline_empty_history_is_an_empty_svg_element() {
    let svg = sparkline(&[], WIDTH, HEIGHT);

    assert!(svg.starts_with(SPARKLINE_ROOT), "{svg}");
    assert_eq!(count(&svg, "<polyline"), 0, "{svg}");
    assert_eq!(count(&svg, "<rect"), 0, "{svg}");
    assert_eq!(count(&svg, "<text"), 0, "{svg}");
}

#[test]
fn sparkline_single_observation_is_a_single_point_polyline_at_the_centre() {
    // One time gives tmax == tmin, so x is w/2 = 50.0. One price gives pmax == pmin, so y is
    // h/2 = 25.0.
    let svg = sparkline(&series(vec![(5, 10000, InStock)]), WIDTH, HEIGHT);

    assert_eq!(count(&svg, "<polyline"), 1, "{svg}");
    assert!(
        svg.contains(r#"<polyline class="price" points="50.0,25.0"/>"#),
        "{svg}"
    );
}

#[test]
fn sparkline_equal_prices_put_every_point_at_mid_height() {
    // Times 0, 10, 20 map to x 0.0, 50.0, 100.0. Equal prices give y h/2 = 25.0 for each.
    let svg = sparkline(
        &series(vec![
            (0, 10000, InStock),
            (10, 10000, InStock),
            (20, 10000, InStock),
        ]),
        WIDTH,
        HEIGHT,
    );

    assert!(
        svg.contains(r#"points="0.0,25.0 50.0,25.0 100.0,25.0""#),
        "{svg}"
    );
}

#[test]
fn sparkline_preorder_and_unknown_observations_are_drawn_as_in_stock() {
    // Only OutOfStock makes a gap, so these three stay in one polyline. Prices 10000..20000
    // map to y 50..0. Times 0, 10, 20 map to x 0.0, 50.0, 100.0. Price 15000 is y 25.0.
    let svg = sparkline(
        &series(vec![
            (0, 10000, InStock),
            (10, 20000, Preorder),
            (20, 15000, Unknown),
        ]),
        WIDTH,
        HEIGHT,
    );

    assert_eq!(count(&svg, "<polyline"), 1, "{svg}");
    assert_eq!(count(&svg, r#"class="gap""#), 0, "{svg}");
    assert!(
        svg.contains(r#"points="0.0,50.0 50.0,0.0 100.0,25.0""#),
        "{svg}"
    );
}

#[test]
fn sparkline_out_of_stock_price_is_left_out_of_the_price_scale() {
    // The out-of-stock price 90000 must not stretch the scale. Without it the in-stock prices
    // span 10000..20000 as in the reference series, so the points are unchanged.
    let svg = sparkline(
        &series(vec![
            (0, 10000, InStock),
            (10, 20000, InStock),
            (20, 90000, OutOfStock),
            (30, 15000, InStock),
        ]),
        WIDTH,
        HEIGHT,
    );

    assert!(svg.contains(r#"points="0.0,50.0 33.3,0.0""#), "{svg}");
    assert!(
        svg.contains(r#"<polyline class="price" points="100.0,25.0"/>"#),
        "{svg}"
    );
}

#[test]
fn sparkline_last_out_of_stock_gap_runs_to_the_right_edge() {
    // The last observation is at tmax, so x is w = 100.0. Its gap runs up to w, so its width
    // is w - x = 0.0.
    let svg = sparkline(
        &series(vec![(0, 10000, InStock), (10, 15000, OutOfStock)]),
        WIDTH,
        HEIGHT,
    );

    assert!(
        svg.contains(r#"<rect class="gap" x="100.0" y="0" width="0.0" height="50"/>"#),
        "{svg}"
    );
}

#[test]
fn sparkline_single_out_of_stock_observation_gap_spans_from_centre_to_edge() {
    // One time gives x = w/2 = 50.0 and the rect runs to w, so its width is 50.0.
    let svg = sparkline(&series(vec![(5, 10000, OutOfStock)]), WIDTH, HEIGHT);

    assert_eq!(count(&svg, "<polyline"), 0, "{svg}");
    assert!(
        svg.contains(r#"<rect class="gap" x="50.0" y="0" width="50.0" height="50"/>"#),
        "{svg}"
    );
}

#[test]
fn sparkline_each_consecutive_out_of_stock_observation_gets_its_own_gap() {
    // Times 0, 10, 20, 30 map to 0.0, 33.3, 66.7, 100.0. The gap at time 10 runs to time 20, which
    // is 33.3 wide, and the gap at time 20 runs to time 30, which is also 33.3 wide.
    let svg = sparkline(
        &series(vec![
            (0, 10000, InStock),
            (10, 15000, OutOfStock),
            (20, 15000, OutOfStock),
            (30, 20000, InStock),
        ]),
        WIDTH,
        HEIGHT,
    );

    assert_eq!(count(&svg, r#"class="gap""#), 2, "{svg}");
    assert!(
        svg.contains(r#"<rect class="gap" x="33.3" y="0" width="33.3" height="50"/>"#),
        "{svg}"
    );
    assert!(
        svg.contains(r#"<rect class="gap" x="66.7" y="0" width="33.3" height="50"/>"#),
        "{svg}"
    );
}

#[test]
fn price_chart_root_element_has_chart_class_and_size() {
    let svg = price_chart(&reference_series(), None, WIDTH, HEIGHT);

    assert!(svg.starts_with(CHART_ROOT), "{svg}");
}

#[test]
fn price_chart_draws_the_same_polylines_and_gap_as_sparkline() {
    // The target 12500 lies inside 10000..20000, so the scale is unchanged.
    let svg = price_chart(&reference_series(), Some(&eur(12500)), WIDTH, HEIGHT);

    assert_eq!(count(&svg, "<polyline"), 2, "{svg}");
    assert!(svg.contains(r#"points="0.0,50.0 33.3,0.0""#), "{svg}");
    assert!(
        svg.contains(r#"<polyline class="price" points="100.0,25.0"/>"#),
        "{svg}"
    );
    assert!(
        svg.contains(r#"<rect class="gap" x="66.7" y="0" width="33.3" height="50"/>"#),
        "{svg}"
    );
}

#[test]
fn price_chart_target_line_is_drawn_at_its_price_height() {
    // Target 12500 is 2500 above pmin 10000 on a span of 10000, so y = 50 - 0.25 * 50 = 37.5.
    // x1 and x2 are whole numbers: 0 and w.
    let svg = price_chart(&reference_series(), Some(&eur(12500)), WIDTH, HEIGHT);

    assert_eq!(count(&svg, r#"class="target""#), 1, "{svg}");
    assert!(
        svg.contains(r#"<line class="target" x1="0" y1="37.5" x2="100" y2="37.5"/>"#),
        "{svg}"
    );
}

#[test]
fn price_chart_without_target_has_no_target_line() {
    let svg = price_chart(&reference_series(), None, WIDTH, HEIGHT);

    assert_eq!(count(&svg, r#"class="target""#), 0, "{svg}");
}

#[test]
fn price_chart_labels_show_the_domain_minimum_and_maximum_as_money() {
    // In-stock prices span 10000..20000 and the target 12500 is inside that range, so the
    // labels are 100.00 EUR and 200.00 EUR.
    let svg = price_chart(&reference_series(), Some(&eur(12500)), WIDTH, HEIGHT);

    let mut labels = label_texts(&svg);
    labels.sort_unstable();
    assert_eq!(labels, ["100.00 EUR", "200.00 EUR"], "{svg}");
}

#[test]
fn price_chart_labels_use_only_the_in_stock_prices_without_a_target() {
    // The out-of-stock price 90000 must not reach the labels.
    let svg = price_chart(
        &series(vec![
            (0, 10000, InStock),
            (10, 20000, InStock),
            (20, 90000, OutOfStock),
        ]),
        None,
        WIDTH,
        HEIGHT,
    );

    let mut labels = label_texts(&svg);
    labels.sort_unstable();
    assert_eq!(labels, ["100.00 EUR", "200.00 EUR"], "{svg}");
}

#[test]
fn price_chart_target_below_minimum_widens_the_price_scale_downwards() {
    // The domain becomes 5000..20000, a span of 15000. Price 10000 is y = 50 - (5000 / 15000)
    // * 50 = 33.3. Price 20000 is y 0.0. Price 15000 is y = 50 - (10000 / 15000) * 50 = 16.7.
    let svg = price_chart(&reference_series(), Some(&eur(5000)), WIDTH, HEIGHT);

    assert!(svg.contains(r#"points="0.0,33.3 33.3,0.0""#), "{svg}");
    assert!(
        svg.contains(r#"<polyline class="price" points="100.0,16.7"/>"#),
        "{svg}"
    );
}

#[test]
fn price_chart_target_below_minimum_is_drawn_at_the_bottom_and_labelled_as_the_minimum() {
    // Target 5000 is the new minimum, so it sits at y = h = 50.0 and the labels are 50.00 EUR
    // and 200.00 EUR.
    let svg = price_chart(&reference_series(), Some(&eur(5000)), WIDTH, HEIGHT);

    assert!(
        svg.contains(r#"<line class="target" x1="0" y1="50.0" x2="100" y2="50.0"/>"#),
        "{svg}"
    );
    let mut labels = label_texts(&svg);
    labels.sort_unstable();
    assert_eq!(labels, ["200.00 EUR", "50.00 EUR"], "{svg}");
}

#[test]
fn price_chart_target_above_maximum_widens_the_price_scale_upwards() {
    // The domain becomes 10000..25000, a span of 15000. Target 25000 is the maximum, so its
    // line is at y = 0.0 and the labels are 100.00 EUR and 250.00 EUR.
    let svg = price_chart(&reference_series(), Some(&eur(25000)), WIDTH, HEIGHT);

    assert!(
        svg.contains(r#"<line class="target" x1="0" y1="0.0" x2="100" y2="0.0"/>"#),
        "{svg}"
    );
    let mut labels = label_texts(&svg);
    labels.sort_unstable();
    assert_eq!(labels, ["100.00 EUR", "250.00 EUR"], "{svg}");
}

#[test]
fn price_chart_empty_history_says_no_data_yet() {
    let svg = price_chart(&[], None, WIDTH, HEIGHT);

    assert!(svg.starts_with(CHART_ROOT), "{svg}");
    assert!(svg.contains(r#"<text class="empty""#), "{svg}");
    assert!(svg.contains(">no data yet</text>"), "{svg}");
    assert_eq!(count(&svg, "<polyline"), 0, "{svg}");
}

#[test]
fn price_chart_single_observation_is_a_single_point_polyline_at_the_centre() {
    // One time gives x = w/2 = 50.0 and one price gives y = h/2 = 25.0.
    let svg = price_chart(&series(vec![(5, 10000, InStock)]), None, WIDTH, HEIGHT);

    assert_eq!(count(&svg, "<polyline"), 1, "{svg}");
    assert!(
        svg.contains(r#"<polyline class="price" points="50.0,25.0"/>"#),
        "{svg}"
    );
}

#[test]
fn price_chart_equal_prices_put_every_point_at_mid_height() {
    // Times 0, 10, 20 map to x 0.0, 50.0, 100.0. Equal prices give y h/2 = 25.0 for each.
    let svg = price_chart(
        &series(vec![
            (0, 10000, InStock),
            (10, 10000, InStock),
            (20, 10000, InStock),
        ]),
        None,
        WIDTH,
        HEIGHT,
    );

    assert!(
        svg.contains(r#"points="0.0,25.0 50.0,25.0 100.0,25.0""#),
        "{svg}"
    );
}

use haggle::db::{Event, EventKind, Observation, Watch, WatchStatus};
use haggle::extract::{Availability, Source};
use haggle::money::{Currency, Money};
use haggle::rules::{SALE_LOOKBACK_S, SALE_MIN_HISTORY, evaluate};

const DAY_S: i64 = 24 * 3600;
const WATCH_ID: i64 = 7;
/// Time of the latest observation in every case. Earlier observations are placed relative to it.
const NOW: i64 = 400 * DAY_S;

/// An expected event: its kind name, then `old` and `new`.
type Expected = (&'static str, Option<Money>, Option<Money>);

fn eur(minor: i64) -> Money {
    Money {
        minor,
        currency: Currency::Eur,
    }
}

fn usd(minor: i64) -> Money {
    Money {
        minor,
        currency: Currency::Usd,
    }
}

/// The time `days` days before `NOW`.
fn days_before(days: i64) -> i64 {
    NOW - days * DAY_S
}

fn watch(target: Option<Money>) -> Watch {
    Watch {
        id: WATCH_ID,
        url: "https://shop.example/kettle".to_string(),
        name: None,
        selector: None,
        target,
        interval_s: 3600,
        enabled: true,
        status: WatchStatus::Ok,
        etag: None,
        last_modified: None,
        next_check_at: 0,
        failures: 0,
        created_at: 0,
    }
}

fn observation(
    at: i64,
    price: Money,
    availability: Availability,
    list_price: Option<Money>,
) -> Observation {
    Observation {
        id: 0,
        watch_id: WATCH_ID,
        at,
        price,
        list_price,
        availability,
        source: Source::Meta,
    }
}

/// An in-stock observation with no list price.
fn seen(at: i64, price: Money) -> Observation {
    observation(at, price, Availability::InStock, None)
}

/// An in-stock observation that shows a list ("was") price.
fn listed(at: i64, price: Money, list_price: Money) -> Observation {
    observation(at, price, Availability::InStock, Some(list_price))
}

fn kind_name(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::PriceDrop => "PriceDrop",
        EventKind::PriceRise => "PriceRise",
        EventKind::TargetHit => "TargetHit",
        EventKind::LowestEver => "LowestEver",
        EventKind::BackInStock => "BackInStock",
        EventKind::OutOfStock => "OutOfStock",
        EventKind::InflatedSale => "InflatedSale",
    }
}

fn describe(event: &Event) -> Expected {
    (kind_name(&event.kind), event.old, event.new)
}

/// Runs the rules and returns the events in the order they come back.
fn run(watch: &Watch, history: &[Observation], latest: &Observation) -> Vec<Expected> {
    evaluate(watch, history, latest)
        .iter()
        .map(describe)
        .collect()
}

// Price drops, rises, target hits and lowest-ever prices.

#[test]
fn drop_below_every_earlier_price_gives_price_drop_and_lowest_ever() {
    // 1000 -> 900 is a drop from the previous 1000, and 900 is below the only earlier price.
    let history = [seen(days_before(1), eur(1000))];
    let latest = seen(NOW, eur(900));
    assert_eq!(
        run(&watch(None), &history, &latest),
        vec![
            ("PriceDrop", Some(eur(1000)), Some(eur(900))),
            ("LowestEver", Some(eur(1000)), Some(eur(900))),
        ]
    );
}

#[test]
fn drop_that_is_not_a_new_low_gives_only_price_drop() {
    // The previous price is 1000, so 900 is a drop. The earlier low is 800, so 900 is not a new low.
    let history = [
        seen(days_before(2), eur(800)),
        seen(days_before(1), eur(1000)),
    ];
    let latest = seen(NOW, eur(900));
    assert_eq!(
        run(&watch(None), &history, &latest),
        vec![("PriceDrop", Some(eur(1000)), Some(eur(900)))]
    );
}

#[test]
fn rise_gives_price_rise_with_old_and_new_price() {
    let history = [seen(days_before(1), eur(900))];
    let latest = seen(NOW, eur(1000));
    assert_eq!(
        run(&watch(None), &history, &latest),
        vec![("PriceRise", Some(eur(900)), Some(eur(1000)))]
    );
}

#[test]
fn equal_price_gives_no_events() {
    let history = [seen(days_before(1), eur(1000))];
    let latest = seen(NOW, eur(1000));
    assert_eq!(run(&watch(None), &history, &latest), vec![]);
}

#[test]
fn first_observation_without_target_gives_no_events() {
    let latest = seen(NOW, eur(1000));
    assert_eq!(run(&watch(None), &[], &latest), vec![]);
}

#[test]
fn first_observation_under_target_gives_target_hit_without_old_price() {
    // No earlier observation, so the old price is None. 900 is under the 1000 target.
    let latest = seen(NOW, eur(900));
    assert_eq!(
        run(&watch(Some(eur(1000))), &[], &latest),
        vec![("TargetHit", None, Some(eur(900)))]
    );
}

#[test]
fn first_observation_exactly_at_target_gives_target_hit() {
    // The target comparison is inclusive: latest.price <= target.
    let latest = seen(NOW, eur(1000));
    assert_eq!(
        run(&watch(Some(eur(1000))), &[], &latest),
        vec![("TargetHit", None, Some(eur(1000)))]
    );
}

#[test]
fn first_observation_above_target_gives_no_events() {
    let latest = seen(NOW, eur(1100));
    assert_eq!(run(&watch(Some(eur(1000))), &[], &latest), vec![]);
}

#[test]
fn drop_to_target_gives_price_drop_target_hit_and_lowest_ever_in_declaration_order() {
    // 1100 -> 1000 with a 1000 target: PriceDrop, then TargetHit (old 1100), then LowestEver.
    let history = [seen(days_before(1), eur(1100))];
    let latest = seen(NOW, eur(1000));
    assert_eq!(
        run(&watch(Some(eur(1000))), &history, &latest),
        vec![
            ("PriceDrop", Some(eur(1100)), Some(eur(1000))),
            ("TargetHit", Some(eur(1100)), Some(eur(1000))),
            ("LowestEver", Some(eur(1100)), Some(eur(1000))),
        ]
    );
}

#[test]
fn already_under_target_last_time_gives_no_target_hit() {
    let history = [seen(days_before(1), eur(900))];
    let latest = seen(NOW, eur(850));
    assert_eq!(
        run(&watch(Some(eur(1000))), &history, &latest),
        vec![
            ("PriceDrop", Some(eur(900)), Some(eur(850))),
            ("LowestEver", Some(eur(900)), Some(eur(850))),
        ]
    );
}

#[test]
fn rise_while_staying_under_target_gives_no_target_hit() {
    // The previous 900 was already under the 1000 target, so 950 does not hit it again.
    let history = [seen(days_before(1), eur(900))];
    let latest = seen(NOW, eur(950));
    assert_eq!(
        run(&watch(Some(eur(1000))), &history, &latest),
        vec![("PriceRise", Some(eur(900)), Some(eur(950)))]
    );
}

#[test]
fn target_in_other_currency_gives_no_target_hit_on_first_observation() {
    let latest = seen(NOW, eur(900));
    assert_eq!(run(&watch(Some(usd(1000))), &[], &latest), vec![]);
}

#[test]
fn target_in_other_currency_is_ignored_when_price_drops() {
    // The USD target 1000 would be hit by 900 if it were compared. Only the price rules run.
    let history = [seen(days_before(1), eur(1100))];
    let latest = seen(NOW, eur(900));
    assert_eq!(
        run(&watch(Some(usd(1000))), &history, &latest),
        vec![
            ("PriceDrop", Some(eur(1100)), Some(eur(900))),
            ("LowestEver", Some(eur(1100)), Some(eur(900))),
        ]
    );
}

#[test]
fn lowest_ever_needs_price_strictly_below_earlier_minimum() {
    // 900 equals the earlier minimum of 900, so it is not a new low. The drop from 1000 still fires.
    let history = [
        seen(days_before(2), eur(900)),
        seen(days_before(1), eur(1000)),
    ];
    let latest = seen(NOW, eur(900));
    assert_eq!(
        run(&watch(None), &history, &latest),
        vec![("PriceDrop", Some(eur(1000)), Some(eur(900)))]
    );
}

#[test]
fn usd_observation_in_history_is_ignored_by_price_rules() {
    // The USD 500 is the oldest entry, so the previous EUR price is 1000. If the USD 500 counted,
    // 900 would not be a new low. Ignored, the EUR minimum is 1000, so LowestEver fires.
    let history = [
        seen(days_before(20), usd(500)),
        seen(days_before(10), eur(1000)),
    ];
    let latest = seen(NOW, eur(900));
    assert_eq!(
        run(&watch(None), &history, &latest),
        vec![
            ("PriceDrop", Some(eur(1000)), Some(eur(900))),
            ("LowestEver", Some(eur(1000)), Some(eur(900))),
        ]
    );
}

#[test]
fn other_currency_as_last_history_entry_leaves_no_previous_price() {
    // The last entry is USD, so there is no comparable previous EUR price: no PriceDrop.
    // LowestEver still compares with every earlier EUR observation: 900 is below 1000.
    let history = [
        seen(days_before(2), eur(1000)),
        seen(days_before(1), usd(500)),
    ];
    let latest = seen(NOW, eur(900));
    assert_eq!(
        run(&watch(None), &history, &latest),
        vec![("LowestEver", Some(eur(1000)), Some(eur(900)))]
    );
}

#[test]
fn other_currency_as_last_history_entry_fires_target_hit_without_old_price() {
    // No comparable previous price, so TargetHit has old None. 1100 is the only earlier EUR price,
    // and 950 is below it, so LowestEver fires.
    let history = [
        seen(days_before(2), eur(1100)),
        seen(days_before(1), usd(500)),
    ];
    let latest = seen(NOW, eur(950));
    assert_eq!(
        run(&watch(Some(eur(1000))), &history, &latest),
        vec![
            ("TargetHit", None, Some(eur(950))),
            ("LowestEver", Some(eur(1100)), Some(eur(950))),
        ]
    );
}

#[test]
fn stock_change_uses_previous_availability_whatever_its_currency() {
    // The previous (last) entry is USD and OutOfStock, so the EUR in-stock latest is BackInStock.
    let history = [
        seen(days_before(2), eur(1000)),
        observation(days_before(1), usd(500), Availability::OutOfStock, None),
    ];
    let latest = seen(NOW, eur(1000));
    assert_eq!(
        run(&watch(None), &history, &latest),
        vec![("BackInStock", None, Some(eur(1000)))]
    );
}

#[test]
fn events_carry_the_watch_id() {
    let history = [seen(days_before(1), eur(1000))];
    let latest = seen(NOW, eur(900));
    let renumbered = Watch {
        id: 42,
        ..watch(None)
    };
    let events = evaluate(&renumbered, &history, &latest);
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.watch_id == 42));
}

// Stock changes.

#[test]
fn out_of_stock_to_in_stock_gives_back_in_stock() {
    let history = [observation(
        days_before(1),
        eur(1000),
        Availability::OutOfStock,
        None,
    )];
    let latest = observation(NOW, eur(1000), Availability::InStock, None);
    assert_eq!(
        run(&watch(None), &history, &latest),
        vec![("BackInStock", None, Some(eur(1000)))]
    );
}

#[test]
fn out_of_stock_to_preorder_gives_back_in_stock() {
    // Preorder counts as buyable.
    let history = [observation(
        days_before(1),
        eur(1000),
        Availability::OutOfStock,
        None,
    )];
    let latest = observation(NOW, eur(1000), Availability::Preorder, None);
    assert_eq!(
        run(&watch(None), &history, &latest),
        vec![("BackInStock", None, Some(eur(1000)))]
    );
}

#[test]
fn in_stock_to_out_of_stock_gives_out_of_stock() {
    let history = [seen(days_before(1), eur(1000))];
    let latest = observation(NOW, eur(1000), Availability::OutOfStock, None);
    assert_eq!(
        run(&watch(None), &history, &latest),
        vec![("OutOfStock", None, Some(eur(1000)))]
    );
}

#[test]
fn preorder_to_out_of_stock_gives_out_of_stock() {
    let history = [observation(
        days_before(1),
        eur(1000),
        Availability::Preorder,
        None,
    )];
    let latest = observation(NOW, eur(1000), Availability::OutOfStock, None);
    assert_eq!(
        run(&watch(None), &history, &latest),
        vec![("OutOfStock", None, Some(eur(1000)))]
    );
}

#[test]
fn out_of_stock_staying_out_gives_no_stock_event() {
    let history = [observation(
        days_before(1),
        eur(1000),
        Availability::OutOfStock,
        None,
    )];
    let latest = observation(NOW, eur(1000), Availability::OutOfStock, None);
    assert_eq!(run(&watch(None), &history, &latest), vec![]);
}

#[test]
fn unknown_to_in_stock_gives_no_stock_event() {
    let history = [observation(
        days_before(1),
        eur(1000),
        Availability::Unknown,
        None,
    )];
    let latest = seen(NOW, eur(1000));
    assert_eq!(run(&watch(None), &history, &latest), vec![]);
}

#[test]
fn in_stock_to_unknown_gives_no_stock_event() {
    let history = [seen(days_before(1), eur(1000))];
    let latest = observation(NOW, eur(1000), Availability::Unknown, None);
    assert_eq!(run(&watch(None), &history, &latest), vec![]);
}

#[test]
fn back_in_stock_with_price_drop_lists_price_events_first() {
    // Order follows the EventKind declaration: PriceDrop, LowestEver, then BackInStock.
    let history = [observation(
        days_before(1),
        eur(1000),
        Availability::OutOfStock,
        None,
    )];
    let latest = seen(NOW, eur(900));
    assert_eq!(
        run(&watch(None), &history, &latest),
        vec![
            ("PriceDrop", Some(eur(1000)), Some(eur(900))),
            ("LowestEver", Some(eur(1000)), Some(eur(900))),
            ("BackInStock", None, Some(eur(900))),
        ]
    );
}

// Inflated sales: a list ("was") price of 1200 is claimed while the price is 700.
// The earlier prices in the 90-day window are 800, so 1200 was never charged.

#[test]
fn inflated_sale_fires_when_list_price_was_never_charged() {
    let history = [
        seen(days_before(10), eur(800)),
        seen(days_before(5), eur(800)),
        seen(days_before(1), eur(800)),
    ];
    let latest = listed(NOW, eur(700), eur(1200));
    assert_eq!(
        run(&watch(None), &history, &latest),
        vec![
            ("PriceDrop", Some(eur(800)), Some(eur(700))),
            ("LowestEver", Some(eur(800)), Some(eur(700))),
            ("InflatedSale", Some(eur(1200)), Some(eur(700))),
        ]
    );
}

#[test]
fn inflated_sale_blocked_when_an_earlier_price_reached_the_list_price() {
    // One earlier price was 1200, so the claimed "was" price was charged. No InflatedSale.
    let history = [
        seen(days_before(10), eur(800)),
        seen(days_before(5), eur(1200)),
        seen(days_before(1), eur(800)),
    ];
    let latest = listed(NOW, eur(700), eur(1200));
    assert_eq!(
        run(&watch(None), &history, &latest),
        vec![
            ("PriceDrop", Some(eur(800)), Some(eur(700))),
            ("LowestEver", Some(eur(800)), Some(eur(700))),
        ]
    );
}

#[test]
fn inflated_sale_needs_three_earlier_observations_in_window() {
    // Only two earlier observations are in the 90-day window.
    let history = [
        seen(days_before(10), eur(800)),
        seen(days_before(1), eur(800)),
    ];
    let latest = listed(NOW, eur(700), eur(1200));
    assert_eq!(
        run(&watch(None), &history, &latest),
        vec![
            ("PriceDrop", Some(eur(800)), Some(eur(700))),
            ("LowestEver", Some(eur(800)), Some(eur(700))),
        ]
    );
}

#[test]
fn observations_older_than_lookback_do_not_count_toward_minimum_history() {
    // 100 days back is outside the 90-day window, so only two observations count.
    let history = [
        seen(days_before(100), eur(800)),
        seen(days_before(10), eur(800)),
        seen(days_before(1), eur(800)),
    ];
    let latest = listed(NOW, eur(700), eur(1200));
    assert_eq!(
        run(&watch(None), &history, &latest),
        vec![
            ("PriceDrop", Some(eur(800)), Some(eur(700))),
            ("LowestEver", Some(eur(800)), Some(eur(700))),
        ]
    );
}

#[test]
fn observations_older_than_lookback_do_not_block_inflated_sale() {
    // The 1200 is 100 days back, outside the window, so it does not count as a charged price.
    let history = [
        seen(days_before(100), eur(1200)),
        seen(days_before(10), eur(800)),
        seen(days_before(5), eur(800)),
        seen(days_before(1), eur(800)),
    ];
    let latest = listed(NOW, eur(700), eur(1200));
    assert_eq!(
        run(&watch(None), &history, &latest),
        vec![
            ("PriceDrop", Some(eur(800)), Some(eur(700))),
            ("LowestEver", Some(eur(800)), Some(eur(700))),
            ("InflatedSale", Some(eur(1200)), Some(eur(700))),
        ]
    );
}

#[test]
fn observation_exactly_at_lookback_start_counts_toward_inflated_sale() {
    let start = NOW - SALE_LOOKBACK_S;
    let history = [
        seen(start, eur(800)),
        seen(start, eur(800)),
        seen(start, eur(800)),
    ];
    let latest = listed(NOW, eur(700), eur(1200));
    assert_eq!(
        run(&watch(None), &history, &latest),
        vec![
            ("PriceDrop", Some(eur(800)), Some(eur(700))),
            ("LowestEver", Some(eur(800)), Some(eur(700))),
            ("InflatedSale", Some(eur(1200)), Some(eur(700))),
        ]
    );
}

#[test]
fn observation_one_second_before_lookback_start_does_not_count() {
    let before_start = NOW - SALE_LOOKBACK_S - 1;
    let history = [
        seen(before_start, eur(800)),
        seen(before_start, eur(800)),
        seen(before_start, eur(800)),
    ];
    let latest = listed(NOW, eur(700), eur(1200));
    assert_eq!(
        run(&watch(None), &history, &latest),
        vec![
            ("PriceDrop", Some(eur(800)), Some(eur(700))),
            ("LowestEver", Some(eur(800)), Some(eur(700))),
        ]
    );
}

#[test]
fn list_price_equal_to_latest_price_gives_no_inflated_sale() {
    // The rule needs the list price to be above the latest price.
    let history = [
        seen(days_before(10), eur(800)),
        seen(days_before(5), eur(800)),
        seen(days_before(1), eur(800)),
    ];
    let latest = listed(NOW, eur(700), eur(700));
    assert_eq!(
        run(&watch(None), &history, &latest),
        vec![
            ("PriceDrop", Some(eur(800)), Some(eur(700))),
            ("LowestEver", Some(eur(800)), Some(eur(700))),
        ]
    );
}

#[test]
fn list_price_in_other_currency_gives_no_inflated_sale() {
    let history = [
        seen(days_before(10), eur(800)),
        seen(days_before(5), eur(800)),
        seen(days_before(1), eur(800)),
    ];
    let latest = listed(NOW, eur(700), usd(1200));
    assert_eq!(
        run(&watch(None), &history, &latest),
        vec![
            ("PriceDrop", Some(eur(800)), Some(eur(700))),
            ("LowestEver", Some(eur(800)), Some(eur(700))),
        ]
    );
}

#[test]
fn latest_without_list_price_gives_no_inflated_sale() {
    let history = [
        seen(days_before(10), eur(800)),
        seen(days_before(5), eur(800)),
        seen(days_before(1), eur(800)),
    ];
    let latest = seen(NOW, eur(700));
    assert_eq!(
        run(&watch(None), &history, &latest),
        vec![
            ("PriceDrop", Some(eur(800)), Some(eur(700))),
            ("LowestEver", Some(eur(800)), Some(eur(700))),
        ]
    );
}

#[test]
fn first_observation_with_list_price_gives_no_events() {
    let latest = listed(NOW, eur(700), eur(1200));
    assert_eq!(run(&watch(None), &[], &latest), vec![]);
}

#[test]
fn previous_observation_already_showing_list_price_gives_no_inflated_sale() {
    // The claim fires once: the previous observation already had the same list price.
    let history = [
        seen(days_before(10), eur(800)),
        seen(days_before(5), eur(800)),
        listed(days_before(1), eur(800), eur(1200)),
    ];
    let latest = listed(NOW, eur(700), eur(1200));
    assert_eq!(
        run(&watch(None), &history, &latest),
        vec![
            ("PriceDrop", Some(eur(800)), Some(eur(700))),
            ("LowestEver", Some(eur(800)), Some(eur(700))),
        ]
    );
}

#[test]
fn usd_observation_in_window_is_ignored_by_inflated_sale() {
    // The USD 1500 is inside the window but in another currency. It is not a charged EUR price.
    let history = [
        seen(days_before(20), usd(1500)),
        seen(days_before(10), eur(800)),
        seen(days_before(5), eur(800)),
        seen(days_before(1), eur(800)),
    ];
    let latest = listed(NOW, eur(700), eur(1200));
    assert_eq!(
        run(&watch(None), &history, &latest),
        vec![
            ("PriceDrop", Some(eur(800)), Some(eur(700))),
            ("LowestEver", Some(eur(800)), Some(eur(700))),
            ("InflatedSale", Some(eur(1200)), Some(eur(700))),
        ]
    );
}

#[test]
fn sale_rule_constants_match_spec() {
    // 90 days in seconds, and three earlier observations.
    assert_eq!(SALE_LOOKBACK_S, 90 * DAY_S);
    assert_eq!(SALE_MIN_HISTORY, 3);
}

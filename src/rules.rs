//! Compares a new observation with a watch's history and returns events. Pure.

use crate::db::{Event, EventKind, Observation, Watch};
use crate::extract::Availability;
use crate::money::Money;

/// Seconds of history the inflated-sale rule looks back over (90 days).
pub const SALE_LOOKBACK_S: i64 = 90 * 24 * 3600;
/// Fewest earlier observations in the lookback for the inflated-sale rule to judge.
pub const SALE_MIN_HISTORY: usize = 3;

/// Compares `latest` with the watch's earlier observations and returns the
/// events it triggers, in `EventKind` declaration order.
///
/// `history` holds every earlier observation, oldest first, without `latest`.
/// Prices are only compared within the currency of `latest.price`.
pub fn evaluate(watch: &Watch, history: &[Observation], latest: &Observation) -> Vec<Event> {
    let currency = latest.price.currency;
    let price = latest.price;
    let previous = history.last();
    let previous_price = previous
        .map(|observation| observation.price)
        .filter(|money| money.currency == currency);
    let same_currency: Vec<&Observation> = history
        .iter()
        .filter(|observation| observation.price.currency == currency)
        .collect();

    let mut events = Vec::new();
    let mut push = |kind: EventKind, old: Option<Money>, new: Option<Money>| {
        events.push(Event {
            watch_id: watch.id,
            kind,
            old,
            new,
        });
    };

    if let Some(old) = previous_price {
        if price.minor < old.minor {
            push(EventKind::PriceDrop, Some(old), Some(price));
        }
        if price.minor > old.minor {
            push(EventKind::PriceRise, Some(old), Some(price));
        }
    }

    if let Some(target) = watch.target.filter(|target| target.currency == currency)
        && price.minor <= target.minor
        && previous_price.is_none_or(|old| old.minor > target.minor)
    {
        push(EventKind::TargetHit, previous_price, Some(price));
    }

    let lowest_before = same_currency
        .iter()
        .map(|observation| observation.price)
        .min_by_key(|money| money.minor);
    if let Some(lowest) = lowest_before.filter(|lowest| price.minor < lowest.minor) {
        push(EventKind::LowestEver, Some(lowest), Some(price));
    }

    if let Some(previous) = previous {
        if previous.availability == Availability::OutOfStock && is_buyable(latest.availability) {
            push(EventKind::BackInStock, None, Some(price));
        }
        if is_buyable(previous.availability) && latest.availability == Availability::OutOfStock {
            push(EventKind::OutOfStock, None, Some(price));
        }
    }

    if let Some(list_price) = inflated_list_price(latest, &same_currency, previous) {
        push(EventKind::InflatedSale, Some(list_price), Some(price));
    }

    events
}

fn is_buyable(availability: Availability) -> bool {
    matches!(availability, Availability::InStock | Availability::Preorder)
}

/// Returns the claimed "was" price when it was never actually charged in the
/// lookback and has not been flagged on the previous observation already.
fn inflated_list_price(
    latest: &Observation,
    same_currency: &[&Observation],
    previous: Option<&Observation>,
) -> Option<Money> {
    let list_price = latest
        .list_price
        .filter(|list| list.currency == latest.price.currency && list.minor > latest.price.minor)?;
    let window_start = latest.at - SALE_LOOKBACK_S;
    let recent: Vec<i64> = same_currency
        .iter()
        .filter(|observation| observation.at >= window_start)
        .map(|observation| observation.price.minor)
        .collect();
    let highest = recent.iter().copied().max()?;
    let is_new_claim = previous.is_none_or(|prev| prev.list_price != latest.list_price);
    (recent.len() >= SALE_MIN_HISTORY && highest < list_price.minor && is_new_claim)
        .then_some(list_price)
}

//! Extractor reading the price from JSON-LD structured data.

use scraper::{Html, Selector};
use serde_json::Value;

use super::{Availability, ExtractError, Extraction, Source};
use crate::money::{Currency, Money, parse_price};

const JSON_LD_TYPE: &str = "application/ld+json";
const LIST_PRICE_SUFFIXES: [&str; 2] = ["ListPrice", "StrikethroughPrice"];

/// A price found on one offer of one product.
struct Candidate {
    price: Money,
    list_price: Option<Money>,
    availability: Availability,
    title: Option<String>,
}

/// Extracts the product price from JSON-LD structured data in the parsed page.
///
/// Returns `ExtractError::NoPrice` when none is present and
/// `ExtractError::AmbiguousPrice` when it states several different prices.
pub fn extract(doc: &Html) -> Result<Extraction, ExtractError> {
    let candidates = candidates(doc);
    let first = candidates.first().ok_or(ExtractError::NoPrice)?;
    if candidates
        .iter()
        .any(|candidate| candidate.price != first.price)
    {
        return Err(ExtractError::AmbiguousPrice);
    }
    Ok(Extraction {
        price: first.price,
        list_price: first.list_price.filter(|list| *list != first.price),
        availability: first.availability,
        title: first.title.clone(),
        source: Source::JsonLd,
    })
}

fn candidates(doc: &Html) -> Vec<Candidate> {
    let selector = Selector::parse("script").expect("static selector is valid");
    let documents: Vec<Value> = doc
        .select(&selector)
        .filter(|script| {
            script
                .value()
                .attr("type")
                .is_some_and(|kind| kind.trim().eq_ignore_ascii_case(JSON_LD_TYPE))
        })
        .filter_map(|script| serde_json::from_str(&script.text().collect::<String>()).ok())
        .collect();
    let mut nodes = Vec::new();
    for document in &documents {
        collect_nodes(document, &mut nodes);
    }
    nodes
        .into_iter()
        .filter(|node| has_type(node, "Product"))
        .flat_map(product_candidates)
        .collect()
}

/// Gathers every object reachable through top-level arrays and `@graph`.
fn collect_nodes<'a>(value: &'a Value, nodes: &mut Vec<&'a Value>) {
    match value {
        Value::Array(items) => items.iter().for_each(|item| collect_nodes(item, nodes)),
        Value::Object(map) => {
            nodes.push(value);
            if let Some(graph) = map.get("@graph") {
                collect_nodes(graph, nodes);
            }
        }
        _ => {}
    }
}

fn has_type(node: &Value, expected: &str) -> bool {
    match node.get("@type") {
        Some(Value::String(kind)) => kind == expected,
        Some(Value::Array(kinds)) => kinds.iter().any(|kind| kind.as_str() == Some(expected)),
        _ => false,
    }
}

/// Treats a single object as a one-element list.
fn as_list(value: Option<&Value>) -> Vec<&Value> {
    match value {
        Some(Value::Array(items)) => items.iter().collect(),
        Some(object @ Value::Object(_)) => vec![object],
        _ => Vec::new(),
    }
}

fn product_candidates(product: &Value) -> Vec<Candidate> {
    let title = product
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned);
    as_list(product.get("offers"))
        .into_iter()
        .filter_map(|offer| offer_candidate(offer, title.clone()))
        .collect()
}

fn offer_candidate(offer: &Value, title: Option<String>) -> Option<Candidate> {
    let currency = offer
        .get("priceCurrency")
        .and_then(Value::as_str)
        .and_then(Currency::from_code);
    let specifications = as_list(offer.get("priceSpecification"));
    let price = if has_type(offer, "AggregateOffer") {
        money_from(offer.get("price"), currency)
            .or_else(|| money_from(offer.get("lowPrice"), currency))
    } else if has_type(offer, "Offer") {
        money_from(offer.get("price"), currency).or_else(|| {
            specifications
                .iter()
                .filter(|spec| spec.get("priceType").is_none())
                .find_map(|spec| specification_money(spec, currency))
        })
    } else {
        None
    }?;
    let list_price = specifications
        .iter()
        .filter(|spec| is_list_price(spec))
        .find_map(|spec| specification_money(spec, currency));
    let availability = offer
        .get("availability")
        .and_then(Value::as_str)
        .map_or(Availability::Unknown, Availability::from_schema);
    Some(Candidate {
        price,
        list_price,
        availability,
        title,
    })
}

fn is_list_price(specification: &Value) -> bool {
    specification
        .get("priceType")
        .and_then(Value::as_str)
        .is_some_and(|kind| {
            LIST_PRICE_SUFFIXES
                .iter()
                .any(|suffix| kind.ends_with(suffix))
        })
}

/// Reads a specification's price, preferring its own currency over the offer's.
fn specification_money(specification: &Value, offer_currency: Option<Currency>) -> Option<Money> {
    let own_currency = specification
        .get("priceCurrency")
        .and_then(Value::as_str)
        .and_then(Currency::from_code);
    money_from(specification.get("price"), own_currency.or(offer_currency))
}

fn money_from(value: Option<&Value>, currency: Option<Currency>) -> Option<Money> {
    let text = match value? {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    parse_price(&text, currency).ok()
}

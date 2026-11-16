//! Extractor reading the price from an element chosen by a user CSS selector.

use scraper::{ElementRef, Selector};

use super::{Availability, ExtractError, Extraction, Source};
use crate::money::{self, Money};

/// Attribute whose value, when present, is the price of a matched element.
const CONTENT_ATTRIBUTE: &str = "content";

/// Selector for the document title element.
const TITLE_SELECTOR: &str = "title";

/// Extracts the product price from the element matched by `selector`.
///
/// A matched element states its price in its `content` attribute if it has
/// one, otherwise in its text. Matches whose value is not a price with a
/// currency are skipped. Returns `ExtractError::BadSelector` when the selector
/// does not parse, `ExtractError::NoPrice` when no match states a price, and
/// `ExtractError::AmbiguousPrice` when the matches state different prices.
pub fn extract(doc: &scraper::Html, selector: &str) -> Result<Extraction, ExtractError> {
    let parsed_selector =
        Selector::parse(selector).map_err(|error| ExtractError::BadSelector(error.to_string()))?;
    let prices: Vec<Money> = doc
        .select(&parsed_selector)
        .filter_map(stated_price)
        .collect();
    Ok(Extraction {
        price: single_price(&prices)?,
        list_price: None,
        availability: Availability::Unknown,
        title: page_title(doc),
        source: Source::Selector,
    })
}

/// Returns the price stated by a matched element, or `None` when it states none.
fn stated_price(element: ElementRef) -> Option<Money> {
    let parsed = match element.attr(CONTENT_ATTRIBUTE) {
        Some(content) => money::parse_price(content, None),
        None => money::parse_price(&element_text(element), None),
    };
    parsed.ok()
}

/// Returns the one price all `prices` agree on.
///
/// Fails with `NoPrice` when `prices` is empty and with `AmbiguousPrice` when
/// two of them differ.
fn single_price(prices: &[Money]) -> Result<Money, ExtractError> {
    let first = prices.first().ok_or(ExtractError::NoPrice)?;
    if prices.iter().all(|price| price == first) {
        Ok(*first)
    } else {
        Err(ExtractError::AmbiguousPrice)
    }
}

/// Returns the trimmed title of the document, or `None` when it has none.
fn page_title(doc: &scraper::Html) -> Option<String> {
    let title_selector =
        Selector::parse(TITLE_SELECTOR).expect("TITLE_SELECTOR is a valid CSS selector");
    doc.select(&title_selector)
        .next()
        .map(element_text)
        .filter(|title| !title.is_empty())
}

/// Returns all descendant text of `element`, joined and trimmed.
fn element_text(element: ElementRef) -> String {
    element.text().collect::<String>().trim().to_owned()
}

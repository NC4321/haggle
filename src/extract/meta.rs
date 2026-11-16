//! Extractor reading the price from meta tags (Open Graph and product price tags).

use crate::money::{Currency, Money, parse_price};
use scraper::Selector;

use super::{Availability, ExtractError, Extraction, Source};

/// CSS selector matching every `<meta>` element.
const META_SELECTOR: &str = "meta";
/// Attributes whose value names a meta tag; either one may carry the key.
const KEY_ATTRIBUTES: [&str; 2] = ["property", "name"];
/// Attribute holding a meta tag's value.
const CONTENT_ATTRIBUTE: &str = "content";

const PRODUCT_PRICE_AMOUNT: &str = "product:price:amount";
const OG_PRICE_AMOUNT: &str = "og:price:amount";
const PRODUCT_PRICE_CURRENCY: &str = "product:price:currency";
const OG_PRICE_CURRENCY: &str = "og:price:currency";
const ORIGINAL_PRICE_AMOUNT: &str = "product:original_price:amount";
const ORIGINAL_PRICE_CURRENCY: &str = "product:original_price:currency";
const PRODUCT_AVAILABILITY: &str = "product:availability";
const OG_AVAILABILITY: &str = "og:availability";
const OG_TITLE: &str = "og:title";

/// Extracts the product price from meta tags (Open Graph and product price tags) in the parsed page.
///
/// Returns `ExtractError::NoPrice` when none is present and
/// `ExtractError::AmbiguousPrice` when it states several different prices.
pub fn extract(doc: &scraper::Html) -> Result<Extraction, ExtractError> {
    let tags = MetaTags::from_document(doc);
    let currency = values_of_first_present(&tags, &[PRODUCT_PRICE_CURRENCY, OG_PRICE_CURRENCY])
        .first()
        .copied()
        .and_then(Currency::from_code);
    let amounts = values_of_first_present(&tags, &[PRODUCT_PRICE_AMOUNT, OG_PRICE_AMOUNT]);
    let price = unique_price(&amounts, currency)?;
    let availability = values_of_first_present(&tags, &[PRODUCT_AVAILABILITY, OG_AVAILABILITY])
        .first()
        .copied()
        .map_or(Availability::Unknown, Availability::from_schema);
    Ok(Extraction {
        price,
        list_price: list_price(&tags, price),
        availability,
        title: title(&tags),
        source: Source::Meta,
    })
}

/// Every `<meta>` tag's trimmed `content`, keyed by its trimmed, lowercased `property` or `name`,
/// in document order.
struct MetaTags {
    entries: Vec<(String, String)>,
}

impl MetaTags {
    /// Collects the meta tags of `doc` that have a `content` value.
    fn from_document(doc: &scraper::Html) -> MetaTags {
        let selector = Selector::parse(META_SELECTOR).expect("the meta selector is valid CSS");
        let mut entries = Vec::new();
        for element in doc.select(&selector) {
            let attributes = element.value();
            let Some(content) = attributes.attr(CONTENT_ATTRIBUTE) else {
                continue;
            };
            for attribute in KEY_ATTRIBUTES {
                if let Some(key) = attributes.attr(attribute) {
                    entries.push((key.trim().to_ascii_lowercase(), content.trim().to_owned()));
                }
            }
        }
        MetaTags { entries }
    }

    /// Returns the values of every tag keyed by `key`, in document order.
    fn values(&self, key: &str) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|(entry_key, _)| entry_key == key)
            .map(|(_, value)| value.as_str())
            .collect()
    }

    /// Returns the value of the first tag keyed by `key`, if any.
    fn first(&self, key: &str) -> Option<&str> {
        self.values(key).first().copied()
    }
}

/// Returns the values for the first of `keys` that has any tag, or an empty list when none has.
fn values_of_first_present<'a>(tags: &'a MetaTags, keys: &[&str]) -> Vec<&'a str> {
    keys.iter()
        .map(|key| tags.values(key))
        .find(|values| !values.is_empty())
        .unwrap_or_default()
}

/// Parses the amounts that parse and returns the single price they all state.
///
/// Amounts that do not parse are skipped.
/// Returns `ExtractError::NoPrice` when no amount parses and
/// `ExtractError::AmbiguousPrice` when the parsed amounts differ.
fn unique_price(amounts: &[&str], currency: Option<Currency>) -> Result<Money, ExtractError> {
    let prices: Vec<Money> = amounts
        .iter()
        .filter_map(|amount| parse_price(amount, currency).ok())
        .collect();
    let first = *prices.first().ok_or(ExtractError::NoPrice)?;
    if prices.iter().all(|price| *price == first) {
        Ok(first)
    } else {
        Err(ExtractError::AmbiguousPrice)
    }
}

/// Returns the shop's "was" price when it parses and differs from `price`.
///
/// Its currency is the stated original-price currency, else the currency of `price`.
fn list_price(tags: &MetaTags, price: Money) -> Option<Money> {
    let amount = tags.first(ORIGINAL_PRICE_AMOUNT)?;
    let currency = tags
        .first(ORIGINAL_PRICE_CURRENCY)
        .and_then(Currency::from_code)
        .unwrap_or(price.currency);
    let original = parse_price(amount, Some(currency)).ok()?;
    (original != price).then_some(original)
}

/// Returns the trimmed `og:title`, or `None` when it is absent or empty.
fn title(tags: &MetaTags) -> Option<String> {
    tags.first(OG_TITLE)
        .filter(|title| !title.is_empty())
        .map(str::to_owned)
}

//! Extractor reading the price from schema.org microdata.

use scraper::{ElementRef, Selector};

use super::{Availability, ExtractError, Extraction, SCHEMA_PREFIXES, Source};
use crate::money::{self, Currency, Money};

/// Schema.org type naming a product scope.
const PRODUCT_TYPE: &str = "Product";
/// Schema.org type naming an offer scope, which may sit inside a product.
const OFFER_TYPE: &str = "Offer";
const PRICE_PROPERTY: &str = "price";
const CURRENCY_PROPERTY: &str = "priceCurrency";
const AVAILABILITY_PROPERTY: &str = "availability";
const NAME_PROPERTY: &str = "name";
/// The element whose `href`, rather than its text, states a microdata value.
const LINK_ELEMENT: &str = "link";

/// Extracts the product price from schema.org microdata in the parsed page.
///
/// Only Product scopes that are not nested in another Product count. Returns
/// `ExtractError::NoPrice` when no price is stated and
/// `ExtractError::AmbiguousPrice` when the products state several different prices.
pub fn extract(doc: &scraper::Html) -> Result<Extraction, ExtractError> {
    let products = top_level_products(doc);
    let prices = stated_prices(&products);
    let price = match prices.as_slice() {
        [] => return Err(ExtractError::NoPrice),
        [price] => *price,
        _ => return Err(ExtractError::AmbiguousPrice),
    };
    let title = products
        .iter()
        .find_map(|product| first_value(*product, NAME_PROPERTY));
    let availability = products
        .iter()
        .find_map(|product| first_value(*product, AVAILABILITY_PROPERTY))
        .map_or(Availability::Unknown, |value| {
            Availability::from_schema(&value)
        });
    Ok(Extraction {
        price,
        list_price: None,
        availability,
        title,
        source: Source::Microdata,
    })
}

/// Returns the Product scopes of `doc` that are not nested in another Product,
/// in document order.
fn top_level_products(doc: &scraper::Html) -> Vec<ElementRef<'_>> {
    doc.select(&selector("[itemscope]"))
        .filter(|scope| has_schema_type(*scope, PRODUCT_TYPE))
        .filter(|product| {
            !product
                .ancestors()
                .filter_map(ElementRef::wrap)
                .any(|ancestor| has_schema_type(ancestor, PRODUCT_TYPE))
        })
        .collect()
}

/// Returns the distinct prices the products state, in document order.
///
/// A candidate that does not parse as a price is skipped.
fn stated_prices(products: &[ElementRef]) -> Vec<Money> {
    let mut prices: Vec<Money> = Vec::new();
    for product in products {
        for (price_element, scope) in owned_properties(*product, PRICE_PROPERTY) {
            let currency = stated_currency(*product, scope);
            let Ok(price) = money::parse_price(&element_value(price_element), currency) else {
                continue;
            };
            if !prices.contains(&price) {
                prices.push(price);
            }
        }
    }
    prices
}

/// Returns the currency stated for a price in `scope`: the first priceCurrency
/// in that scope, else the first one in the product itself.
///
/// Returns `None` when neither exists or the stated code is not a supported currency.
fn stated_currency(product: ElementRef, scope: ElementRef) -> Option<Currency> {
    let currencies = owned_properties(product, CURRENCY_PROPERTY);
    let nearest = currencies
        .iter()
        .find(|(_, owner)| owner.id() == scope.id())
        .or_else(|| {
            currencies
                .iter()
                .find(|(_, owner)| owner.id() == product.id())
        });
    nearest.and_then(|(element, _)| Currency::from_code(&element_value(*element)))
}

/// Returns the first non-empty value of the `name` property of `product`, if any.
fn first_value(product: ElementRef, name: &str) -> Option<String> {
    owned_properties(product, name)
        .into_iter()
        .map(|(element, _)| element_value(element))
        .find(|value| !value.is_empty())
}

/// Returns the elements of `product` whose itemprop includes `name`, each paired
/// with the scope it sits in (see `owning_scope`). Elements inside other nested
/// scopes are left out.
fn owned_properties<'a>(
    product: ElementRef<'a>,
    name: &str,
) -> Vec<(ElementRef<'a>, ElementRef<'a>)> {
    product
        .select(&selector("[itemprop]"))
        .filter(|element| has_itemprop(*element, name))
        .filter_map(|element| owning_scope(element, product).map(|scope| (element, scope)))
        .collect()
}

/// Returns the Offer that `element` sits in within `product`, or `product`
/// itself when it sits in no Offer.
///
/// Returns `None` when a scope other than an Offer lies between them, because
/// the element then belongs to that nested scope.
fn owning_scope<'a>(element: ElementRef<'a>, product: ElementRef<'a>) -> Option<ElementRef<'a>> {
    let mut offer = None;
    let ancestors = element
        .ancestors()
        .filter_map(ElementRef::wrap)
        .take_while(|ancestor| ancestor.id() != product.id());
    for ancestor in ancestors {
        if !is_scope(ancestor) {
            continue;
        }
        if !has_schema_type(ancestor, OFFER_TYPE) {
            return None;
        }
        offer.get_or_insert(ancestor);
    }
    Some(offer.unwrap_or(product))
}

/// Returns the value an element states for a microdata property: its `content`
/// attribute, else the `href` of a link, else its trimmed text.
fn element_value(element: ElementRef) -> String {
    let stated = element
        .value()
        .attr("content")
        .or_else(|| link_href(element));
    match stated {
        Some(value) => value.trim().to_string(),
        None => element.text().collect::<String>().trim().to_string(),
    }
}

/// Returns the `href` of `element` when it is a link.
fn link_href<'a>(element: ElementRef<'a>) -> Option<&'a str> {
    if element.value().name() == LINK_ELEMENT {
        element.value().attr("href")
    } else {
        None
    }
}

/// Returns whether `element` opens a microdata scope.
fn is_scope(element: ElementRef) -> bool {
    element.value().attr("itemscope").is_some()
}

/// Returns whether the itemprop of `element` includes `name`.
fn has_itemprop(element: ElementRef, name: &str) -> bool {
    element
        .value()
        .attr("itemprop")
        .is_some_and(|names| names.split_whitespace().any(|candidate| candidate == name))
}

/// Returns whether the itemtype of `element` names the schema.org type
/// `type_name` over http or https.
fn has_schema_type(element: ElementRef, type_name: &str) -> bool {
    element.value().attr("itemtype").is_some_and(|itemtypes| {
        itemtypes.split_whitespace().any(|itemtype| {
            SCHEMA_PREFIXES
                .iter()
                .any(|prefix| itemtype.strip_prefix(prefix) == Some(type_name))
        })
    })
}

/// Parses a CSS selector that is fixed in this module, so an invalid one is a bug.
fn selector(css: &str) -> Selector {
    Selector::parse(css).expect("selectors in the microdata extractor are valid")
}

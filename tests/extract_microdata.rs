use haggle::extract::microdata::extract;
use haggle::extract::{Availability, ExtractError, Extraction, Source};
use haggle::money::{Currency, Money};
use scraper::Html;

const PRODUCT_OFFER: &str = include_str!("fixtures/microdata/product_offer.html");
const PRODUCT_HTTP: &str = include_str!("fixtures/microdata/product_http.html");
const VISIBLE_TEXT_PRICE: &str = include_str!("fixtures/microdata/visible_text_price.html");
const CONTENT_BEATS_TEXT: &str = include_str!("fixtures/microdata/content_beats_text.html");
const OUT_OF_STOCK: &str = include_str!("fixtures/microdata/out_of_stock.html");
const RELATED_PRODUCT: &str = include_str!("fixtures/microdata/related_product.html");
const TWO_PRODUCTS_DIFFERENT_PRICES: &str =
    include_str!("fixtures/microdata/two_products_different_prices.html");
const TWO_PRODUCTS_SAME_PRICE: &str =
    include_str!("fixtures/microdata/two_products_same_price.html");
const NO_PRODUCT: &str = include_str!("fixtures/microdata/no_product.html");
const PRODUCT_GROUP: &str = include_str!("fixtures/microdata/product_group.html");
const NO_CURRENCY: &str = include_str!("fixtures/microdata/no_currency.html");
const UNPARSEABLE_CANDIDATE: &str = include_str!("fixtures/microdata/unparseable_candidate.html");
const UNPARSEABLE_ONLY: &str = include_str!("fixtures/microdata/unparseable_only.html");
const OFFER_CURRENCY_WINS: &str = include_str!("fixtures/microdata/offer_currency_wins.html");
const PRODUCT_CURRENCY_FALLBACK: &str =
    include_str!("fixtures/microdata/product_currency_fallback.html");
const PRODUCT_NO_NAME: &str = include_str!("fixtures/microdata/product_no_name.html");

fn run(page: &str) -> Result<Extraction, ExtractError> {
    extract(&Html::parse_document(page))
}

fn extracted(page: &str) -> Extraction {
    run(page).expect("the page has a usable product price")
}

#[test]
fn https_product_with_nested_offer_gives_its_price() {
    // 299.00 EUR is 29900 minor units (299 * 100).
    assert_eq!(
        extracted(PRODUCT_OFFER).price,
        Money {
            minor: 29900,
            currency: Currency::Eur
        }
    );
}

#[test]
fn product_name_is_the_title() {
    assert_eq!(
        extracted(PRODUCT_OFFER).title.as_deref(),
        Some("Trail Kettle")
    );
}

#[test]
fn link_href_gives_the_availability() {
    assert_eq!(extracted(PRODUCT_OFFER).availability, Availability::InStock);
}

#[test]
fn source_is_microdata() {
    assert_eq!(extracted(PRODUCT_OFFER).source, Source::Microdata);
}

#[test]
fn list_price_is_always_none() {
    assert_eq!(extracted(PRODUCT_OFFER).list_price, None);
}

#[test]
fn http_product_itemtype_is_accepted() {
    // 12.50 EUR is 1250 minor units.
    assert_eq!(
        extracted(PRODUCT_HTTP).price,
        Money {
            minor: 1250,
            currency: Currency::Eur
        }
    );
}

#[test]
fn visible_text_price_is_read_with_its_currency() {
    // $24.99 USD is 2499 minor units (24 * 100 + 99).
    assert_eq!(
        extracted(VISIBLE_TEXT_PRICE).price,
        Money {
            minor: 2499,
            currency: Currency::Usd
        }
    );
}

#[test]
fn title_is_read_as_trimmed_text() {
    assert_eq!(
        extracted(VISIBLE_TEXT_PRICE).title.as_deref(),
        Some("Desk Lamp")
    );
}

#[test]
fn content_attribute_wins_over_element_text() {
    // The content attribute says 10.00 and the text says 12.00, so 10.00 EUR (1000 minor units) is read.
    assert_eq!(
        extracted(CONTENT_BEATS_TEXT).price,
        Money {
            minor: 1000,
            currency: Currency::Eur
        }
    );
}

#[test]
fn missing_availability_is_unknown() {
    assert_eq!(
        extracted(VISIBLE_TEXT_PRICE).availability,
        Availability::Unknown
    );
}

#[test]
fn out_of_stock_is_read_from_meta_content() {
    assert_eq!(
        extracted(OUT_OF_STOCK).availability,
        Availability::OutOfStock
    );
}

#[test]
fn nested_related_product_price_is_ignored() {
    // The related product has 149.00 EUR, which is ignored, so the main 299.00 EUR (29900 minor units) is the only price.
    assert_eq!(
        extracted(RELATED_PRODUCT).price,
        Money {
            minor: 29900,
            currency: Currency::Eur
        }
    );
}

#[test]
fn nested_related_product_name_is_not_the_title() {
    // The related product's name comes first in the document but sits in a nested Product scope.
    assert_eq!(
        extracted(RELATED_PRODUCT).title.as_deref(),
        Some("Trail Kettle")
    );
}

#[test]
fn different_prices_on_two_products_are_ambiguous() {
    assert_eq!(
        run(TWO_PRODUCTS_DIFFERENT_PRICES),
        Err(ExtractError::AmbiguousPrice)
    );
}

#[test]
fn same_price_on_two_products_is_not_ambiguous() {
    // 10.00 EUR is 1000 minor units.
    assert_eq!(
        extracted(TWO_PRODUCTS_SAME_PRICE).price,
        Money {
            minor: 1000,
            currency: Currency::Eur
        }
    );
}

#[test]
fn page_without_product_scope_is_no_price() {
    // The page has an Offer with a price, but no Product scope.
    assert_eq!(run(NO_PRODUCT), Err(ExtractError::NoPrice));
}

#[test]
fn product_group_itemtype_is_not_a_product() {
    // "schema.org/ProductGroup" does not end with "schema.org/Product".
    assert_eq!(run(PRODUCT_GROUP), Err(ExtractError::NoPrice));
}

#[test]
fn bare_price_without_currency_is_no_price() {
    assert_eq!(run(NO_CURRENCY), Err(ExtractError::NoPrice));
}

#[test]
fn unparseable_price_candidate_is_skipped() {
    // "Call for price" cannot be parsed, so the 49.00 EUR offer (4900 minor units) is the only price.
    assert_eq!(
        extracted(UNPARSEABLE_CANDIDATE).price,
        Money {
            minor: 4900,
            currency: Currency::Eur
        }
    );
}

#[test]
fn only_unparseable_price_is_no_price() {
    assert_eq!(run(UNPARSEABLE_ONLY), Err(ExtractError::NoPrice));
}

#[test]
fn offer_currency_overrides_product_currency() {
    // The product says USD, but the nearest priceCurrency to the price is EUR in the Offer scope.
    assert_eq!(
        extracted(OFFER_CURRENCY_WINS).price,
        Money {
            minor: 29900,
            currency: Currency::Eur
        }
    );
}

#[test]
fn offer_without_currency_uses_product_currency() {
    // 45.00 GBP is 4500 minor units.
    assert_eq!(
        extracted(PRODUCT_CURRENCY_FALLBACK).price,
        Money {
            minor: 4500,
            currency: Currency::Gbp
        }
    );
}

#[test]
fn product_without_name_has_no_title() {
    assert_eq!(extracted(PRODUCT_NO_NAME).title, None);
}

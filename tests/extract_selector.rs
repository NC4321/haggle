use haggle::extract::selector::extract;
use haggle::extract::{Availability, ExtractError, Extraction, Source};
use haggle::money::{Currency, Money};
use scraper::Html;

const PRICE_TEXT_EUR: &str = include_str!("fixtures/selector/price_text_eur.html");
const PRICE_SPLIT_SPANS: &str = include_str!("fixtures/selector/price_split_spans.html");
const CONTENT_ATTRIBUTE: &str = include_str!("fixtures/selector/content_attribute.html");
const CONTENT_EMPTY: &str = include_str!("fixtures/selector/content_empty.html");
const SAME_PRICE_TWICE: &str = include_str!("fixtures/selector/same_price_twice.html");
const DIFFERENT_PRICES: &str = include_str!("fixtures/selector/different_prices.html");
const UNPARSEABLE_MATCH_SKIPPED: &str =
    include_str!("fixtures/selector/unparseable_match_skipped.html");
const ONLY_UNPARSEABLE_MATCH: &str = include_str!("fixtures/selector/only_unparseable_match.html");
const NO_MATCHING_ELEMENT: &str = include_str!("fixtures/selector/no_matching_element.html");
const NO_CURRENCY: &str = include_str!("fixtures/selector/no_currency.html");
const NO_TITLE: &str = include_str!("fixtures/selector/no_title.html");
const EMPTY_TITLE: &str = include_str!("fixtures/selector/empty_title.html");

/// Parses a fixture page the way the extractor receives it.
fn parse(html: &str) -> Html {
    Html::parse_document(html)
}

/// Runs the selector over `html`, failing the test if it is rejected.
fn extract_ok(html: &str, selector: &str) -> Extraction {
    match extract(&parse(html), selector) {
        Ok(extraction) => extraction,
        Err(error) => panic!("expected {selector:?} to extract, got {error:?}"),
    }
}

#[test]
fn eur_price_text_is_parsed_with_thousands_and_decimal_marks() {
    // "1.299,00 €": the comma is the decimal mark, so 1299 units + 00 cents.
    // 1299 * 100 + 0 = 129900 minor units of EUR.
    let extraction = extract_ok(PRICE_TEXT_EUR, ".price");
    assert_eq!(
        extraction.price,
        Money {
            minor: 129_900,
            currency: Currency::Eur,
        }
    );
}

#[test]
fn title_is_the_trimmed_title_element_text() {
    // The <title> is "  Stand Mixer  "; trimming leaves "Stand Mixer".
    let extraction = extract_ok(PRICE_TEXT_EUR, ".price");
    assert_eq!(extraction.title, Some("Stand Mixer".to_string()));
}

#[test]
fn source_is_selector() {
    let extraction = extract_ok(PRICE_TEXT_EUR, ".price");
    assert_eq!(extraction.source, Source::Selector);
}

#[test]
fn availability_is_unknown() {
    let extraction = extract_ok(PRICE_TEXT_EUR, ".price");
    assert_eq!(extraction.availability, Availability::Unknown);
}

#[test]
fn list_price_is_none() {
    // The page shows a struck-through "1.499,00 €", but the selector never
    // sets a list price.
    let extraction = extract_ok(PRICE_TEXT_EUR, ".price");
    assert_eq!(extraction.list_price, None);
}

#[test]
fn split_spans_are_joined_into_one_price() {
    // Descendant text "$" + "19.99" joins to "$19.99": no hint, "$" is USD.
    // 19.99 = 19 * 100 + 99 = 1999 minor units.
    let extraction = extract_ok(PRICE_SPLIT_SPANS, ".price");
    assert_eq!(
        extraction.price,
        Money {
            minor: 1999,
            currency: Currency::Usd,
        }
    );
}

#[test]
fn content_attribute_is_preferred_over_element_text() {
    // The text says "Now 59,00 €" (5900 EUR), but the content attribute
    // "49.90 EUR" is used: 49.90 = 49 * 100 + 90 = 4990 minor units.
    let extraction = extract_ok(CONTENT_ATTRIBUTE, ".price");
    assert_eq!(
        extraction.price,
        Money {
            minor: 4990,
            currency: Currency::Eur,
        }
    );
}

#[test]
fn empty_content_attribute_counts_as_present_and_is_skipped() {
    // The attribute is present with value "", so the element's text is not
    // used. "" has no currency, so the match is skipped and nothing parses.
    assert_eq!(
        extract(&parse(CONTENT_EMPTY), ".price"),
        Err(ExtractError::NoPrice)
    );
}

#[test]
fn same_price_matched_twice_returns_that_price() {
    // Both matches are "19,99 €" = 19 * 100 + 99 = 1999 minor units of EUR.
    let extraction = extract_ok(SAME_PRICE_TWICE, ".price");
    assert_eq!(
        extraction.price,
        Money {
            minor: 1999,
            currency: Currency::Eur,
        }
    );
}

#[test]
fn different_prices_matched_are_ambiguous() {
    assert_eq!(
        extract(&parse(DIFFERENT_PRICES), ".price"),
        Err(ExtractError::AmbiguousPrice)
    );
}

#[test]
fn unparseable_match_is_skipped_when_another_match_parses() {
    // "Call for price" has no currency, so it is skipped. The only parsed
    // match is "19,99 €" = 1999 minor units of EUR.
    let extraction = extract_ok(UNPARSEABLE_MATCH_SKIPPED, ".price");
    assert_eq!(
        extraction.price,
        Money {
            minor: 1999,
            currency: Currency::Eur,
        }
    );
}

#[test]
fn only_unparseable_matches_give_no_price() {
    assert_eq!(
        extract(&parse(ONLY_UNPARSEABLE_MATCH), ".price"),
        Err(ExtractError::NoPrice)
    );
}

#[test]
fn selector_matching_nothing_gives_no_price() {
    // The page has no ".price" element; only ".cost" holds a price.
    assert_eq!(
        extract(&parse(NO_MATCHING_ELEMENT), ".price"),
        Err(ExtractError::NoPrice)
    );
}

#[test]
fn text_without_currency_gives_no_price() {
    // "1299" states no currency and no hint is given, so it does not parse.
    assert_eq!(
        extract(&parse(NO_CURRENCY), ".price"),
        Err(ExtractError::NoPrice)
    );
}

#[test]
fn invalid_selector_gives_bad_selector() {
    // "div[" is an unclosed attribute selector. The message is the parser's,
    // so only the variant is checked.
    let result = extract(&parse(PRICE_TEXT_EUR), "div[");
    assert!(
        matches!(result, Err(ExtractError::BadSelector(_))),
        "expected BadSelector, got {result:?}"
    );
}

#[test]
fn missing_title_element_gives_no_title() {
    let extraction = extract_ok(NO_TITLE, ".price");
    assert_eq!(extraction.title, None);
}

#[test]
fn blank_title_element_gives_no_title() {
    // "   " trims to empty, which counts as no title.
    let extraction = extract_ok(EMPTY_TITLE, ".price");
    assert_eq!(extraction.title, None);
}

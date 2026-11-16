use haggle::extract::{ExtractError, Extraction, Source, extract};
use haggle::money::{Currency, Money};

const JSONLD_AND_META: &str = include_str!("fixtures/chain/jsonld_and_meta.html");
const JSONLD_AND_SELECTOR: &str = include_str!("fixtures/chain/jsonld_and_selector.html");
const META_ONLY: &str = include_str!("fixtures/chain/meta_only.html");
const META_AND_SELECTOR: &str = include_str!("fixtures/chain/meta_and_selector.html");
const MICRODATA_ONLY: &str = include_str!("fixtures/chain/microdata_only.html");
const MICRODATA_AND_SELECTOR: &str = include_str!("fixtures/chain/microdata_and_selector.html");
const SELECTOR_ONLY: &str = include_str!("fixtures/chain/selector_only.html");
const PLAIN_TEXT_ONLY: &str = include_str!("fixtures/chain/plain_text_only.html");
const JSONLD_AMBIGUOUS: &str = include_str!("fixtures/chain/jsonld_ambiguous.html");
const JSONLD_AMBIGUOUS_AND_META: &str =
    include_str!("fixtures/chain/jsonld_ambiguous_and_meta.html");
const JSONLD_AMBIGUOUS_AND_SELECTOR: &str =
    include_str!("fixtures/chain/jsonld_ambiguous_and_selector.html");
const SELECTOR_AMBIGUOUS: &str = include_str!("fixtures/chain/selector_ambiguous.html");

/// Runs the whole chain over a page, failing the test if it finds no price.
fn extract_ok(html: &str, selector: Option<&str>) -> Extraction {
    match extract(html, selector) {
        Ok(extraction) => extraction,
        Err(error) => panic!("expected a price, got {error:?}"),
    }
}

#[test]
fn json_ld_price_wins_over_meta_price() {
    // JSON-LD says 100.00 EUR (10000 cents); meta says 90.00 EUR.
    let found = extract_ok(JSONLD_AND_META, None);

    assert_eq!(
        found.price,
        Money {
            minor: 10000,
            currency: Currency::Eur
        }
    );
}

#[test]
fn json_ld_price_gives_json_ld_source_when_meta_is_also_present() {
    let found = extract_ok(JSONLD_AND_META, None);

    assert_eq!(found.source, Source::JsonLd);
}

#[test]
fn meta_only_page_gives_meta_price() {
    // 90.00 EUR is 9000 cents.
    let found = extract_ok(META_ONLY, None);

    assert_eq!(
        found.price,
        Money {
            minor: 9000,
            currency: Currency::Eur
        }
    );
}

#[test]
fn meta_only_page_gives_meta_source() {
    let found = extract_ok(META_ONLY, None);

    assert_eq!(found.source, Source::Meta);
}

#[test]
fn microdata_only_page_gives_microdata_price() {
    // "75.50" with EUR is 75 euros and 50 cents, which is 7550 cents.
    let found = extract_ok(MICRODATA_ONLY, None);

    assert_eq!(
        found.price,
        Money {
            minor: 7550,
            currency: Currency::Eur
        }
    );
}

#[test]
fn microdata_only_page_gives_microdata_source() {
    let found = extract_ok(MICRODATA_ONLY, None);

    assert_eq!(found.source, Source::Microdata);
}

#[test]
fn selector_only_page_with_selector_gives_selector_price() {
    // "49,99 €": the comma is the decimal mark, so 49 euros and 99 cents, which is 4999 cents.
    let found = extract_ok(SELECTOR_ONLY, Some(".price"));

    assert_eq!(
        found.price,
        Money {
            minor: 4999,
            currency: Currency::Eur
        }
    );
}

#[test]
fn selector_only_page_with_selector_gives_selector_source() {
    let found = extract_ok(SELECTOR_ONLY, Some(".price"));

    assert_eq!(found.source, Source::Selector);
}

#[test]
fn selector_price_is_not_used_when_no_selector_is_given() {
    // The page has a .price element, but no selector was passed in.
    let result = extract(SELECTOR_ONLY, None);

    assert_eq!(result, Err(ExtractError::NoPrice));
}

#[test]
fn price_in_plain_text_is_not_guessed_when_every_extractor_fails() {
    // "Only 49,99 €!" is readable text, not structured data or a selector match.
    let result = extract(PLAIN_TEXT_ONLY, None);

    assert_eq!(result, Err(ExtractError::NoPrice));
}

#[test]
fn plain_text_page_with_selector_matching_nothing_gives_no_price() {
    let result = extract(PLAIN_TEXT_ONLY, Some(".price"));

    assert_eq!(result, Err(ExtractError::NoPrice));
}

#[test]
fn empty_page_gives_no_price() {
    let result = extract("", None);

    assert_eq!(result, Err(ExtractError::NoPrice));
}

#[test]
fn json_ld_price_wins_over_selector_when_both_are_present() {
    // The selector would match 59,99 € (5999 cents), but JSON-LD's 100.00 EUR wins.
    let found = extract_ok(JSONLD_AND_SELECTOR, Some(".price"));

    assert_eq!(
        found.price,
        Money {
            minor: 10000,
            currency: Currency::Eur
        }
    );
}

#[test]
fn json_ld_price_gives_json_ld_source_when_selector_is_given() {
    let found = extract_ok(JSONLD_AND_SELECTOR, Some(".price"));

    assert_eq!(found.source, Source::JsonLd);
}

#[test]
fn meta_price_wins_over_selector_when_both_are_present() {
    // The selector would match 59,99 € (5999 cents), but meta's 90.00 EUR wins.
    let found = extract_ok(META_AND_SELECTOR, Some(".price"));

    assert_eq!(
        found.price,
        Money {
            minor: 9000,
            currency: Currency::Eur
        }
    );
}

#[test]
fn microdata_price_wins_over_selector_when_both_are_present() {
    // The selector would match 59,99 € (5999 cents), but microdata's 75.50 EUR wins.
    let found = extract_ok(MICRODATA_AND_SELECTOR, Some(".price"));

    assert_eq!(
        found.price,
        Money {
            minor: 7550,
            currency: Currency::Eur
        }
    );
}

#[test]
fn bad_selector_on_page_without_structured_data_gives_bad_selector() {
    // "div[" is an unclosed attribute selector, so the parser rejects it.
    let result = extract(PLAIN_TEXT_ONLY, Some("div["));

    assert!(
        matches!(result, Err(ExtractError::BadSelector(_))),
        "expected BadSelector, got {result:?}"
    );
}

#[test]
fn bad_selector_is_not_tried_when_json_ld_already_gives_a_price() {
    // The selector is only tried when the structured extractors fail, so JSON-LD wins.
    let found = extract_ok(JSONLD_AND_SELECTOR, Some("div["));

    assert_eq!(
        found.price,
        Money {
            minor: 10000,
            currency: Currency::Eur
        }
    );
}

#[test]
fn bad_selector_is_returned_over_ambiguous_structured_data() {
    // BadSelector is returned as is, even though JSON-LD alone is ambiguous.
    let result = extract(JSONLD_AMBIGUOUS, Some("div["));

    assert!(
        matches!(result, Err(ExtractError::BadSelector(_))),
        "expected BadSelector, got {result:?}"
    );
}

#[test]
fn ambiguous_json_ld_with_nothing_else_gives_ambiguous_price() {
    // Two offers at 100.00 and 120.00 EUR; no meta, microdata or selector to fall back on.
    let result = extract(JSONLD_AMBIGUOUS, None);

    assert_eq!(result, Err(ExtractError::AmbiguousPrice));
}

#[test]
fn ambiguous_json_ld_gives_meta_price_when_meta_has_one() {
    // JSON-LD is ambiguous, so the chain moves on; meta's 90.00 EUR is 9000 cents.
    let found = extract_ok(JSONLD_AMBIGUOUS_AND_META, None);

    assert_eq!(
        found.price,
        Money {
            minor: 9000,
            currency: Currency::Eur
        }
    );
}

#[test]
fn ambiguous_json_ld_gives_meta_source_when_meta_has_one() {
    let found = extract_ok(JSONLD_AMBIGUOUS_AND_META, None);

    assert_eq!(found.source, Source::Meta);
}

#[test]
fn ambiguous_json_ld_gives_selector_price_when_selector_matches() {
    // The selector matches 59,99 € (5999 cents), which is the one price it finds.
    let found = extract_ok(JSONLD_AMBIGUOUS_AND_SELECTOR, Some(".price"));

    assert_eq!(
        found.price,
        Money {
            minor: 5999,
            currency: Currency::Eur
        }
    );
}

#[test]
fn ambiguous_json_ld_gives_selector_source_when_selector_matches() {
    let found = extract_ok(JSONLD_AMBIGUOUS_AND_SELECTOR, Some(".price"));

    assert_eq!(found.source, Source::Selector);
}

#[test]
fn ambiguous_selector_matches_give_ambiguous_price_when_nothing_else_works() {
    // Two .price elements at 49,99 € and 59,99 €; no structured data to fall back on.
    let result = extract(SELECTOR_AMBIGUOUS, Some(".price"));

    assert_eq!(result, Err(ExtractError::AmbiguousPrice));
}

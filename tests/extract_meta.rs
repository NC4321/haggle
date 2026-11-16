use haggle::extract::meta::extract;
use haggle::extract::{Availability, ExtractError, Extraction, Source};
use haggle::money::{Currency, Money};
use scraper::Html;

fn run(page: &str) -> Result<Extraction, ExtractError> {
    extract(&Html::parse_document(page))
}

#[test]
fn product_price_tags_give_amount_in_minor_units_and_currency() {
    // 49.99 USD is 4999 cents.
    let found = run(include_str!("fixtures/meta/product_tags.html")).unwrap();

    assert_eq!(
        found.price,
        Money {
            minor: 4999,
            currency: Currency::Usd
        }
    );
}

#[test]
fn product_price_tags_give_no_list_price_when_none_given() {
    let found = run(include_str!("fixtures/meta/product_tags.html")).unwrap();

    assert_eq!(found.list_price, None);
}

#[test]
fn og_title_is_read_as_title() {
    let found = run(include_str!("fixtures/meta/product_tags.html")).unwrap();

    assert_eq!(found.title, Some("Ceramic Mug".to_string()));
}

#[test]
fn in_stock_product_availability_maps_to_in_stock() {
    let found = run(include_str!("fixtures/meta/product_tags.html")).unwrap();

    assert_eq!(found.availability, Availability::InStock);
}

#[test]
fn extraction_source_is_meta() {
    let found = run(include_str!("fixtures/meta/product_tags.html")).unwrap();

    assert!(matches!(found.source, Source::Meta));
}

#[test]
fn og_price_tags_are_used_when_product_price_tags_are_absent() {
    // "19,90" with EUR is 19.90 euros, which is 1990 cents.
    let found = run(include_str!("fixtures/meta/og_price_fallback.html")).unwrap();

    assert_eq!(
        found.price,
        Money {
            minor: 1990,
            currency: Currency::Eur
        }
    );
}

#[test]
fn name_attribute_price_tags_are_read_like_property_tags() {
    let found = run(include_str!("fixtures/meta/name_attributes.html")).unwrap();

    assert_eq!(
        found.price,
        Money {
            minor: 4999,
            currency: Currency::Usd
        }
    );
}

#[test]
fn name_attribute_og_title_is_read_as_title() {
    let found = run(include_str!("fixtures/meta/name_attributes.html")).unwrap();

    assert_eq!(found.title, Some("Desk Lamp".to_string()));
}

#[test]
fn name_attribute_availability_is_read_as_product_availability() {
    let found = run(include_str!("fixtures/meta/name_attributes.html")).unwrap();

    assert_eq!(found.availability, Availability::InStock);
}

#[test]
fn tag_keys_match_regardless_of_case() {
    let found = run(include_str!("fixtures/meta/mixed_case_keys.html")).unwrap();

    assert_eq!(
        found.price,
        Money {
            minor: 4999,
            currency: Currency::Usd
        }
    );
}

#[test]
fn tag_key_case_does_not_hide_og_title() {
    let found = run(include_str!("fixtures/meta/mixed_case_keys.html")).unwrap();

    assert_eq!(found.title, Some("Desk Lamp".to_string()));
}

#[test]
fn original_price_tags_give_list_price() {
    // 69.99 USD is 6999 cents.
    let found = run(include_str!("fixtures/meta/original_price.html")).unwrap();

    assert_eq!(
        found.list_price,
        Some(Money {
            minor: 6999,
            currency: Currency::Usd
        })
    );
}

#[test]
fn original_price_without_currency_takes_price_currency() {
    let found = run(include_str!(
        "fixtures/meta/original_price_inherits_currency.html"
    ))
    .unwrap();

    assert_eq!(
        found.list_price,
        Some(Money {
            minor: 6999,
            currency: Currency::Usd
        })
    );
}

#[test]
fn original_price_equal_to_price_gives_no_list_price() {
    let found = run(include_str!("fixtures/meta/original_price_equal.html")).unwrap();

    assert_eq!(found.list_price, None);
}

#[test]
fn original_price_that_does_not_parse_gives_no_list_price() {
    let found = run(include_str!(
        "fixtures/meta/original_price_unparseable.html"
    ))
    .unwrap();

    assert_eq!(found.list_price, None);
}

#[test]
fn og_availability_oos_maps_to_out_of_stock() {
    let found = run(include_str!("fixtures/meta/og_out_of_stock.html")).unwrap();

    assert_eq!(found.availability, Availability::OutOfStock);
}

#[test]
fn availability_is_unknown_when_no_availability_tag_is_present() {
    let found = run(include_str!("fixtures/meta/price_only.html")).unwrap();

    assert_eq!(found.availability, Availability::Unknown);
}

#[test]
fn product_availability_wins_over_og_availability() {
    // product:availability says in stock, og:availability says oos.
    let found = run(include_str!("fixtures/meta/availability_product_wins.html")).unwrap();

    assert_eq!(found.availability, Availability::InStock);
}

#[test]
fn product_price_amount_wins_over_og_price_amount() {
    // og:price:amount says 59.99, but the product key is chosen first,
    // so the result is 49.99 and there is no ambiguity.
    let found = run(include_str!("fixtures/meta/product_price_wins.html")).unwrap();

    assert_eq!(
        found.price,
        Money {
            minor: 4999,
            currency: Currency::Usd
        }
    );
}

#[test]
fn two_different_amount_tags_give_ambiguous_price() {
    let result = run(include_str!("fixtures/meta/ambiguous_amounts.html"));

    assert_eq!(result, Err(ExtractError::AmbiguousPrice));
}

#[test]
fn two_identical_amount_tags_are_not_ambiguous() {
    let found = run(include_str!("fixtures/meta/duplicate_amounts.html")).unwrap();

    assert_eq!(
        found.price,
        Money {
            minor: 4999,
            currency: Currency::Usd
        }
    );
}

#[test]
fn unparseable_amount_tag_is_skipped_among_several() {
    let found = run(include_str!("fixtures/meta/amounts_one_unparseable.html")).unwrap();

    assert_eq!(
        found.price,
        Money {
            minor: 4999,
            currency: Currency::Usd
        }
    );
}

#[test]
fn unparseable_tag_does_not_hide_ambiguity_between_parsed_tags() {
    let result = run(include_str!(
        "fixtures/meta/amounts_unparseable_then_ambiguous.html"
    ));

    assert_eq!(result, Err(ExtractError::AmbiguousPrice));
}

#[test]
fn page_with_only_og_title_and_image_gives_no_price() {
    let result = run(include_str!("fixtures/meta/no_price_tags.html"));

    assert_eq!(result, Err(ExtractError::NoPrice));
}

#[test]
fn amount_that_does_not_parse_gives_no_price() {
    let result = run(include_str!("fixtures/meta/unparseable_amount.html"));

    assert_eq!(result, Err(ExtractError::NoPrice));
}

#[test]
fn empty_amount_content_gives_no_price() {
    let result = run(include_str!("fixtures/meta/empty_amount.html"));

    assert_eq!(result, Err(ExtractError::NoPrice));
}

#[test]
fn bare_amount_without_any_currency_gives_no_price() {
    let result = run(include_str!("fixtures/meta/amount_without_currency.html"));

    assert_eq!(result, Err(ExtractError::NoPrice));
}

#[test]
fn amount_with_currency_symbol_parses_without_currency_tag() {
    let found = run(include_str!(
        "fixtures/meta/amount_with_symbol_without_currency.html"
    ))
    .unwrap();

    assert_eq!(
        found.price,
        Money {
            minor: 4999,
            currency: Currency::Usd
        }
    );
}

#[test]
fn currency_falls_back_to_og_price_currency() {
    let found = run(include_str!("fixtures/meta/currency_from_og.html")).unwrap();

    assert_eq!(
        found.price,
        Money {
            minor: 4999,
            currency: Currency::Usd
        }
    );
}

#[test]
fn title_is_trimmed_of_surrounding_whitespace() {
    let found = run(include_str!("fixtures/meta/title_trimmed.html")).unwrap();

    assert_eq!(found.title, Some("Ceramic Mug".to_string()));
}

#[test]
fn whitespace_only_title_gives_no_title() {
    let found = run(include_str!("fixtures/meta/title_blank.html")).unwrap();

    assert_eq!(found.title, None);
}

#[test]
fn empty_title_content_gives_no_title() {
    let found = run(include_str!("fixtures/meta/title_empty.html")).unwrap();

    assert_eq!(found.title, None);
}

#[test]
fn page_without_og_title_gives_no_title() {
    let found = run(include_str!("fixtures/meta/price_only.html")).unwrap();

    assert_eq!(found.title, None);
}

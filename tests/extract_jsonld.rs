use haggle::extract::jsonld;
use haggle::extract::{Availability, ExtractError, Extraction, Source};
use haggle::money::{Currency, Money};

const PLAIN_PRODUCT: &str = include_str!("fixtures/jsonld/product_string_price.html");
const NUMERIC_PRICE: &str = include_str!("fixtures/jsonld/product_numeric_price.html");
const EQUAL_OFFER_PRICES: &str = include_str!("fixtures/jsonld/product_offers_equal_prices.html");
const AGGREGATE_OFFER: &str = include_str!("fixtures/jsonld/aggregate_offer.html");
const GRAPH_PAGE: &str = include_str!("fixtures/jsonld/graph_page.html");
const PRICE_SPEC_LIST_PRICE: &str =
    include_str!("fixtures/jsonld/price_specification_list_price.html");
const PRICE_SPEC_STRIKETHROUGH: &str =
    include_str!("fixtures/jsonld/price_specification_strikethrough.html");
const LIST_PRICE_EQUAL: &str = include_str!("fixtures/jsonld/list_price_equal_to_price.html");
const OUT_OF_STOCK: &str = include_str!("fixtures/jsonld/out_of_stock.html");
const OFFERS_DIFFERENT_PRICES: &str = include_str!("fixtures/jsonld/offers_different_prices.html");
const OFFERS_SAME_AMOUNT_OTHER_CURRENCY: &str =
    include_str!("fixtures/jsonld/offers_same_amount_other_currency.html");
const BROKEN_BLOCK_THEN_GOOD: &str = include_str!("fixtures/jsonld/broken_block_then_good.html");
const ORGANIZATION_ONLY: &str = include_str!("fixtures/jsonld/organization_only.html");
const UNPARSEABLE_PRICE_SKIPPED: &str =
    include_str!("fixtures/jsonld/unparseable_price_skipped.html");
const UNPARSEABLE_PRICE_ONLY: &str = include_str!("fixtures/jsonld/unparseable_price_only.html");
const SCRIPT_TYPE_UPPERCASE: &str = include_str!("fixtures/jsonld/script_type_uppercase.html");
const TOP_LEVEL_ARRAY: &str = include_str!("fixtures/jsonld/top_level_array.html");
const TYPE_ARRAY_WITH_PRODUCT: &str = include_str!("fixtures/jsonld/type_array_with_product.html");
const EMPTY_NAME: &str = include_str!("fixtures/jsonld/empty_name.html");
const NAME_WITH_SPACES: &str = include_str!("fixtures/jsonld/name_with_spaces.html");
const NON_LD_JSON_SCRIPT: &str = include_str!("fixtures/jsonld/non_ld_json_script.html");
const PRODUCT_GROUP_ONLY: &str = include_str!("fixtures/jsonld/product_group_only.html");

fn extract_page(html: &str) -> Result<Extraction, ExtractError> {
    jsonld::extract(&scraper::Html::parse_document(html))
}

fn extracted(html: &str) -> Extraction {
    extract_page(html).expect("the page has a usable JSON-LD price")
}

fn money(minor: i64, currency: Currency) -> Money {
    Money { minor, currency }
}

#[test]
fn string_price_is_read_in_minor_units() {
    // "1299.00" EUR is 1299 euros, i.e. 129900 cents.
    let found = extracted(PLAIN_PRODUCT);

    assert_eq!(found.price, money(129900, Currency::Eur));
}

#[test]
fn product_title_is_taken_from_the_product_name() {
    let found = extracted(PLAIN_PRODUCT);

    assert_eq!(found.title.as_deref(), Some("Trail Running Shoe"));
}

#[test]
fn in_stock_schema_url_gives_in_stock_availability() {
    let found = extracted(PLAIN_PRODUCT);

    assert_eq!(found.availability, Availability::InStock);
}

#[test]
fn plain_product_has_no_list_price() {
    let found = extracted(PLAIN_PRODUCT);

    assert_eq!(found.list_price, None);
}

#[test]
fn source_is_json_ld_for_a_price_found_in_json_ld() {
    let found = extracted(PLAIN_PRODUCT);

    assert_eq!(found.source, Source::JsonLd);
}

#[test]
fn numeric_price_is_read_through_its_text_form() {
    // A JSON number 49.5 in USD is read as "49.5", i.e. 49.50 dollars = 4950 cents.
    let found = extracted(NUMERIC_PRICE);

    assert_eq!(found.price, money(4950, Currency::Usd));
}

#[test]
fn numeric_price_page_without_availability_gives_unknown() {
    let found = extracted(NUMERIC_PRICE);

    assert_eq!(found.availability, Availability::Unknown);
}

#[test]
fn offers_array_with_equal_prices_gives_one_price() {
    // Both sellers charge 19.90 EUR, so the distinct price set has one value.
    let found = extracted(EQUAL_OFFER_PRICES);

    assert_eq!(found.price, money(1990, Currency::Eur));
}

#[test]
fn availability_comes_from_first_offer_at_the_chosen_price() {
    // The first offer (19.90, OutOfStock) comes before the second (19.90, InStock).
    let found = extracted(EQUAL_OFFER_PRICES);

    assert_eq!(found.availability, Availability::OutOfStock);
}

#[test]
fn aggregate_offer_gives_low_price_and_ignores_nested_offers() {
    // lowPrice is 25.00 EUR = 2500 cents. The nested offer at 35.00 must be ignored,
    // otherwise the two distinct prices would make the result ambiguous.
    let found = extracted(AGGREGATE_OFFER);

    assert_eq!(found.price, money(2500, Currency::Eur));
}

#[test]
fn graph_page_product_price_is_found() {
    // "89.00" GBP is 8900 pence. WebSite and BreadcrumbList must not add prices.
    let found = extracted(GRAPH_PAGE);

    assert_eq!(found.price, money(8900, Currency::Gbp));
}

#[test]
fn graph_page_product_title_is_found() {
    let found = extracted(GRAPH_PAGE);

    assert_eq!(found.title.as_deref(), Some("Waxed Field Jacket"));
}

#[test]
fn price_specification_without_price_type_supplies_price() {
    // The Offer has no price; the entry without priceType gives 99.00 EUR = 9900 cents.
    let found = extracted(PRICE_SPEC_LIST_PRICE);

    assert_eq!(found.price, money(9900, Currency::Eur));
}

#[test]
fn price_specification_with_list_price_type_sets_list_price() {
    // The ListPrice entry gives 120.00 EUR = 12000 cents.
    let found = extracted(PRICE_SPEC_LIST_PRICE);

    assert_eq!(found.list_price, Some(money(12000, Currency::Eur)));
}

#[test]
fn strikethrough_price_specification_sets_list_price() {
    // The Offer price is 59.00 EUR; the StrikethroughPrice is 79.00 EUR = 7900 cents.
    let found = extracted(PRICE_SPEC_STRIKETHROUGH);

    assert_eq!(found.list_price, Some(money(7900, Currency::Eur)));
}

#[test]
fn list_price_equal_to_price_is_dropped() {
    // Both the Offer and the ListPrice say 59.00 EUR, so there is no separate list price.
    let found = extracted(LIST_PRICE_EQUAL);

    assert_eq!(found.list_price, None);
}

#[test]
fn out_of_stock_http_schema_url_gives_out_of_stock_availability() {
    let found = extracted(OUT_OF_STOCK);

    assert_eq!(found.availability, Availability::OutOfStock);
}

#[test]
fn offers_with_different_prices_are_ambiguous() {
    // 10.00 EUR and 12.00 EUR are two different prices.
    let result = extract_page(OFFERS_DIFFERENT_PRICES);

    assert_eq!(result, Err(ExtractError::AmbiguousPrice));
}

#[test]
fn same_amount_in_different_currencies_is_ambiguous() {
    // 10.00 EUR and 10.00 USD are different Money values.
    let result = extract_page(OFFERS_SAME_AMOUNT_OTHER_CURRENCY);

    assert_eq!(result, Err(ExtractError::AmbiguousPrice));
}

#[test]
fn broken_json_ld_block_is_skipped_and_good_block_is_used() {
    // The first block is truncated JSON and is skipped; the second gives 75.50 EUR = 7550 cents.
    let found = extracted(BROKEN_BLOCK_THEN_GOOD);

    assert_eq!(found.price, money(7550, Currency::Eur));
}

#[test]
fn page_with_only_organization_json_ld_gives_no_price() {
    let result = extract_page(ORGANIZATION_ONLY);

    assert_eq!(result, Err(ExtractError::NoPrice));
}

#[test]
fn unparseable_price_is_skipped_in_favour_of_a_parseable_one() {
    // "POA" cannot be parsed as a price and is skipped; 45.00 EUR = 4500 cents remains.
    let found = extracted(UNPARSEABLE_PRICE_SKIPPED);

    assert_eq!(found.price, money(4500, Currency::Eur));
}

#[test]
fn only_unparseable_prices_give_no_price() {
    let result = extract_page(UNPARSEABLE_PRICE_ONLY);

    assert_eq!(result, Err(ExtractError::NoPrice));
}

#[test]
fn script_type_is_matched_case_insensitively() {
    // The type is written "Application/LD+JSON"; 33.00 EUR = 3300 cents.
    let found = extracted(SCRIPT_TYPE_UPPERCASE);

    assert_eq!(found.price, money(3300, Currency::Eur));
}

#[test]
fn top_level_array_is_walked_for_products() {
    // The Product is the second element of the array; 14.00 EUR = 1400 cents.
    let found = extracted(TOP_LEVEL_ARRAY);

    assert_eq!(found.price, money(1400, Currency::Eur));
}

#[test]
fn type_array_containing_product_is_accepted() {
    // "@type": ["Thing", "Product"]; 8.25 EUR = 825 cents.
    let found = extracted(TYPE_ARRAY_WITH_PRODUCT);

    assert_eq!(found.price, money(825, Currency::Eur));
}

#[test]
fn blank_product_name_gives_no_title() {
    let found = extracted(EMPTY_NAME);

    assert_eq!(found.title, None);
}

#[test]
fn product_name_is_trimmed() {
    let found = extracted(NAME_WITH_SPACES);

    assert_eq!(found.title.as_deref(), Some("Canvas Tote Bag"));
}

#[test]
fn json_in_a_non_ld_json_script_is_ignored() {
    // The only Product sits in a script of type application/json, so there is no price.
    let result = extract_page(NON_LD_JSON_SCRIPT);

    assert_eq!(result, Err(ExtractError::NoPrice));
}

#[test]
fn product_group_is_not_accepted_as_a_product() {
    // Only @type Product is read; the ProductGroup offer at 64.00 EUR is not used.
    let result = extract_page(PRODUCT_GROUP_ONLY);

    assert_eq!(result, Err(ExtractError::NoPrice));
}

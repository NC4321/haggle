use haggle::money::{Currency, Money, MoneyError, parse_price};

/// Parses `text`, failing the test if it is rejected.
fn parse_ok(text: &str, hint: Option<Currency>) -> Money {
    match parse_price(text, hint) {
        Ok(money) => money,
        Err(_) => panic!("expected {text:?} to parse"),
    }
}

/// Parses `text`, failing the test if it is accepted. Returns the error.
fn parse_err(text: &str, hint: Option<Currency>) -> MoneyError {
    match parse_price(text, hint) {
        Ok(money) => panic!(
            "expected {text:?} to be rejected, got {} minor units",
            money.minor
        ),
        Err(error) => error,
    }
}

/// Asserts that `text` parses to `minor` units of `currency`.
fn assert_parses(text: &str, hint: Option<Currency>, minor: i64, currency: Currency) {
    let money = parse_ok(text, hint);
    assert_eq!(money.minor, minor, "minor units for {text:?}");
    assert_eq!(money.currency, currency, "currency for {text:?}");
}

#[test]
fn spec_example_european_format_with_euro_symbol() {
    // Both separators: the comma is the rightmost, so it is the decimal mark.
    // 1.299,00 = 1299 units + 00 cents = 129900 minor.
    assert_parses("1.299,00 €", None, 129_900, Currency::Eur);
}

#[test]
fn spec_example_us_format_with_dollar_sign() {
    // Both separators: the dot is the rightmost, so it is the decimal mark.
    // $ with no hint resolves to Usd.
    assert_parses("$1,299.00", None, 129_900, Currency::Usd);
}

#[test]
fn spec_example_bare_number_uses_hint() {
    // No text currency, so the hint is used: 1299 units * 100 = 129900.
    assert_parses("1299", Some(Currency::Eur), 129_900, Currency::Eur);
}

#[test]
fn spec_example_apostrophe_thousands_with_iso_prefix() {
    // CHF is a separate token. The apostrophe is a thousands mark, so the
    // number is 1299.50 = 1299 * 100 + 50 = 129950.
    assert_parses("CHF 1'299.50", None, 129_950, Currency::Chf);
}

#[test]
fn spec_example_yen_comma_thousands_has_no_decimals() {
    // One comma followed by three digits is thousands. JPY has exponent 0.
    assert_parses("¥1,299", None, 1299, Currency::Jpy);
}

#[test]
fn spec_example_comma_decimal_with_iso_suffix() {
    // One comma followed by one digit is decimal: 12 units + "5" padded to 50.
    // 12 * 100 + 50 = 1250.
    assert_parses("12,5 EUR", None, 1250, Currency::Eur);
}

#[test]
fn us_format_with_thousands_and_cents() {
    assert_parses("$1,299.00", None, 129_900, Currency::Usd);
}

#[test]
fn european_format_with_thousands_and_comma_cents() {
    assert_parses("1.299,00 €", None, 129_900, Currency::Eur);
}

#[test]
fn european_format_with_no_break_space_thousands() {
    // U+00A0 is removed like a space: "1299,00" -> 129900.
    assert_parses("1\u{a0}299,00 €", None, 129_900, Currency::Eur);
}

#[test]
fn european_format_with_narrow_no_break_space_thousands() {
    // U+202F is removed like a space: "1299,00" -> 129900.
    assert_parses("1\u{202f}299,00 €", None, 129_900, Currency::Eur);
}

#[test]
fn comma_decimal_under_one_unit_keeps_leading_zero_units() {
    // 0 units * 100 + 99 cents = 99.
    assert_parses("0,99 €", None, 99, Currency::Eur);
}

#[test]
fn single_comma_with_three_digits_is_thousands_with_hint() {
    // "1,299" with a Eur hint: thousands, so 1299 units = 129900.
    assert_parses("1,299", Some(Currency::Eur), 129_900, Currency::Eur);
}

#[test]
fn single_comma_with_two_digits_is_decimal_with_hint() {
    // "1,29": decimal, 1 unit + 29 cents = 129.
    assert_parses("1,29", Some(Currency::Eur), 129, Currency::Eur);
}

#[test]
fn single_dot_with_three_digits_is_thousands_with_symbol() {
    // "€1.299": one dot followed by three digits is thousands, so 129900.
    assert_parses("€1.299", None, 129_900, Currency::Eur);
}

#[test]
fn single_dot_with_one_digit_is_decimal_padded_with_hint() {
    // "12.5": decimal, "5" padded to 50 cents. 12 * 100 + 50 = 1250.
    assert_parses("12.5", Some(Currency::Eur), 1250, Currency::Eur);
}

#[test]
fn single_dot_with_one_digit_is_decimal_with_symbol() {
    // "1.2 €": decimal, "2" padded to 20 cents. 1 * 100 + 20 = 120.
    assert_parses("1.2 €", None, 120, Currency::Eur);
}

#[test]
fn single_dot_with_three_digits_is_thousands_for_zero_exponent() {
    // JPY has exponent 0, so the three digits after the dot are thousands.
    // 1.999 -> 1999 yen.
    assert_parses("1.999", Some(Currency::Jpy), 1999, Currency::Jpy);
}

#[test]
fn decimal_digits_beyond_exponent_are_invalid() {
    // "1,23" with JPY: decimal, two decimal digits, but JPY allows none.
    assert!(matches!(
        parse_err("1,23", Some(Currency::Jpy)),
        MoneyError::Invalid(_)
    ));
}

#[test]
fn single_decimal_digit_beyond_zero_exponent_is_invalid() {
    // "12.5 ¥": one decimal digit, JPY allows none.
    assert!(matches!(parse_err("12.5 ¥", None), MoneyError::Invalid(_)));
}

#[test]
fn three_decimal_digits_beyond_two_exponent_are_invalid() {
    // Both separators: the comma is the decimal mark, "567" has 3 digits
    // but EUR allows 2.
    assert!(matches!(
        parse_err("1.234,567 €", None),
        MoneyError::Invalid(_)
    ));
}

#[test]
fn thousands_groups_not_three_digits_are_invalid() {
    // Comma appears twice, so it is thousands. Group "29" has 2 digits.
    assert!(matches!(
        parse_err("1,29,000 €", None),
        MoneyError::Invalid(_)
    ));
}

#[test]
fn repeated_dot_thousands_with_comma_decimal() {
    // 1.000.000 = 1000000 units; ",00" is cents: 1000000 * 100 = 100000000.
    assert_parses("1.000.000,00 €", None, 100_000_000, Currency::Eur);
}

#[test]
fn repeated_dot_thousands_without_decimals() {
    // Dot appears twice, so it is thousands: 1.234.567 = 1234567 units.
    // 1234567 * 100 = 123456700.
    assert_parses("1.234.567 €", None, 123_456_700, Currency::Eur);
}

#[test]
fn zero_amount_with_symbol() {
    assert_parses("0 €", None, 0, Currency::Eur);
}

#[test]
fn zero_amount_with_decimals() {
    assert_parses("0,00 €", None, 0, Currency::Eur);
}

#[test]
fn leading_and_trailing_whitespace_is_trimmed() {
    // 12,5 -> 1250 cents.
    assert_parses("  12,5 EUR  ", None, 1250, Currency::Eur);
}

#[test]
fn iso_code_glued_to_number_is_recognised() {
    assert_parses("12,5EUR", None, 1250, Currency::Eur);
}

#[test]
fn iso_code_is_case_insensitive() {
    // "usd" is a Currency::from_code match. 5 * 100 = 500.
    assert_parses("5 usd", None, 500, Currency::Usd);
}

#[test]
fn euro_symbol_glued_to_number_is_recognised() {
    // "€10" with Eur hint: text and hint agree. 10 * 100 = 1000.
    assert_parses("€10", Some(Currency::Eur), 1000, Currency::Eur);
}

#[test]
fn pound_symbol_resolves_to_gbp() {
    // "£9.99": decimal, 9 * 100 + 99 = 999.
    assert_parses("£9.99", None, 999, Currency::Gbp);
}

#[test]
fn zloty_symbol_resolves_to_pln() {
    // "12,50 zł": decimal, 12 * 100 + 50 = 1250.
    assert_parses("12,50 zł", None, 1250, Currency::Pln);
}

#[test]
fn koruna_symbol_resolves_to_czk() {
    // "250 Kč": 250 * 100 = 25000.
    assert_parses("250 Kč", None, 25_000, Currency::Czk);
}

#[test]
fn dollar_with_usd_hint_resolves_to_usd() {
    assert_parses("$5", Some(Currency::Usd), 500, Currency::Usd);
}

#[test]
fn dollar_with_cad_hint_resolves_to_cad() {
    // The hint is Cad, so "$" means Cad: 5 * 100 = 500.
    assert_parses("$5", Some(Currency::Cad), 500, Currency::Cad);
}

#[test]
fn dollar_with_aud_hint_resolves_to_aud() {
    assert_parses("$5", Some(Currency::Aud), 500, Currency::Aud);
}

#[test]
fn kr_with_sek_hint_resolves_to_sek() {
    // "kr" matches the hint when the hint is Sek: 100 * 100 = 10000.
    assert_parses("100 kr", Some(Currency::Sek), 10_000, Currency::Sek);
}

#[test]
fn kr_without_hint_is_no_currency() {
    assert!(matches!(parse_err("100 kr", None), MoneyError::NoCurrency));
}

#[test]
fn text_currency_differs_from_hint_is_mismatch() {
    // "€10" with a Usd hint: Eur from the text, Usd from the hint.
    assert!(matches!(
        parse_err("€10", Some(Currency::Usd)),
        MoneyError::CurrencyMismatch
    ));
}

#[test]
fn pound_symbol_differs_from_euro_hint_is_mismatch() {
    assert!(matches!(
        parse_err("£5", Some(Currency::Eur)),
        MoneyError::CurrencyMismatch
    ));
}

#[test]
fn bare_number_without_hint_is_no_currency() {
    assert!(matches!(parse_err("1299", None), MoneyError::NoCurrency));
}

#[test]
fn single_comma_thousands_without_currency_is_no_currency() {
    // Currency is checked before the number, so "1,299" with no currency
    // fails with NoCurrency.
    assert!(matches!(parse_err("1,299", None), MoneyError::NoCurrency));
}

#[test]
fn empty_text_is_empty_error() {
    assert!(matches!(parse_err("", None), MoneyError::Empty));
}

#[test]
fn whitespace_only_text_is_empty_error() {
    assert!(matches!(parse_err("   ", None), MoneyError::Empty));
}

#[test]
fn letters_without_currency_is_no_currency() {
    // "abc" has no currency, so the currency check fails first.
    assert!(matches!(parse_err("abc", None), MoneyError::NoCurrency));
}

#[test]
fn letters_with_hint_is_invalid() {
    // With the Eur hint the currency is found, so the number "abc" is invalid.
    assert!(matches!(
        parse_err("abc", Some(Currency::Eur)),
        MoneyError::Invalid(_)
    ));
}

#[test]
fn negative_amount_is_invalid() {
    assert!(matches!(parse_err("-5 €", None), MoneyError::Invalid(_)));
}

#[test]
fn explicit_plus_sign_is_invalid() {
    assert!(matches!(parse_err("+5 €", None), MoneyError::Invalid(_)));
}

#[test]
fn letter_inside_number_is_invalid() {
    assert!(matches!(parse_err("12x €", None), MoneyError::Invalid(_)));
}

#[test]
fn largest_representable_minor_amount_is_accepted() {
    // 92233720368547758 * 100 = 9223372036854775800, plus 7 cents
    // = 9223372036854775807, which is i64::MAX.
    assert_parses("92233720368547758,07 €", None, i64::MAX, Currency::Eur);
}

#[test]
fn minor_amount_one_cent_over_i64_max_is_invalid() {
    // 9223372036854775800 + 8 = 9223372036854775808, one past i64::MAX.
    assert!(matches!(
        parse_err("92233720368547758,08 €", None),
        MoneyError::Invalid(_)
    ));
}

#[test]
fn from_code_matches_lowercase_iso_code() {
    assert_eq!(Currency::from_code("eur"), Some(Currency::Eur));
}

#[test]
fn exponent_of_euro_is_two() {
    assert_eq!(Currency::Eur.exponent(), 2);
}

#[test]
fn exponent_of_dollar_is_two() {
    assert_eq!(Currency::Usd.exponent(), 2);
}

#[test]
fn exponent_of_yen_is_zero() {
    assert_eq!(Currency::Jpy.exponent(), 0);
}

#[test]
fn display_euro_has_two_decimals_and_code() {
    // 129900 minor, exponent 2: 1299.00, then the code.
    let money = Money {
        minor: 129_900,
        currency: Currency::Eur,
    };
    assert_eq!(money.to_string(), "1299.00 EUR");
}

#[test]
fn display_usd_five_cents_pads_to_two_decimals() {
    // 5 minor, exponent 2: 0.05.
    let money = Money {
        minor: 5,
        currency: Currency::Usd,
    };
    assert_eq!(money.to_string(), "0.05 USD");
}

#[test]
fn display_yen_has_no_decimals() {
    // 1299 minor, exponent 0: 1299.
    let money = Money {
        minor: 1299,
        currency: Currency::Jpy,
    };
    assert_eq!(money.to_string(), "1299 JPY");
}

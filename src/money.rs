//! Currencies and exact money amounts in minor units.

use std::fmt;

/// A supported ISO 4217 currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Currency {
    Usd,
    Eur,
    Gbp,
    Chf,
    Cad,
    Aud,
    Jpy,
    Sek,
    Nok,
    Dkk,
    Pln,
    Czk,
}

impl Currency {
    /// Returns the ISO 4217 code, such as `"USD"`.
    pub fn code(self) -> &'static str {
        match self {
            Currency::Usd => "USD",
            Currency::Eur => "EUR",
            Currency::Gbp => "GBP",
            Currency::Chf => "CHF",
            Currency::Cad => "CAD",
            Currency::Aud => "AUD",
            Currency::Jpy => "JPY",
            Currency::Sek => "SEK",
            Currency::Nok => "NOK",
            Currency::Dkk => "DKK",
            Currency::Pln => "PLN",
            Currency::Czk => "CZK",
        }
    }

    /// Returns the number of decimal places of the minor unit (0 for JPY, else 2).
    pub fn exponent(self) -> u32 {
        match self {
            Currency::Jpy => 0,
            _ => 2,
        }
    }

    /// Parses an ISO 4217 code, ignoring case and surrounding whitespace.
    /// Returns `None` for an unsupported code.
    pub fn from_code(code: &str) -> Option<Currency> {
        const ALL: [Currency; 12] = [
            Currency::Usd,
            Currency::Eur,
            Currency::Gbp,
            Currency::Chf,
            Currency::Cad,
            Currency::Aud,
            Currency::Jpy,
            Currency::Sek,
            Currency::Nok,
            Currency::Dkk,
            Currency::Pln,
            Currency::Czk,
        ];
        let code = code.trim();
        ALL.into_iter()
            .find(|currency| currency.code().eq_ignore_ascii_case(code))
    }
}

/// An amount in the minor unit of a currency (cents for USD, yen for JPY).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Money {
    pub minor: i64,
    pub currency: Currency,
}

/// Why a price text could not be turned into `Money`.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MoneyError {
    #[error("empty price")]
    Empty,
    #[error("not a price: {0}")]
    Invalid(String),
    #[error("no currency in price")]
    NoCurrency,
    #[error("currency in price differs from the stated one")]
    CurrencyMismatch,
}

/// Parses a price text such as `"$1,299.00"` into `Money`.
///
/// `currency` is the currency stated elsewhere on the page, if any.
/// Returns `MoneyError` when the text is empty, not a price, has no
/// currency, or its currency differs from the stated one.
pub fn parse_price(_text: &str, _currency: Option<Currency>) -> Result<Money, MoneyError> {
    unimplemented!("parse_price")
}

impl fmt::Display for Money {
    fn fmt(&self, _formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        unimplemented!("Money::fmt")
    }
}

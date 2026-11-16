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
pub fn parse_price(text: &str, currency: Option<Currency>) -> Result<Money, MoneyError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(MoneyError::Empty);
    }
    let (mentions, residue) = split_currency_mentions(trimmed);
    let currency = resolve_currency(&mentions, currency)?;
    let minor = minor_units(&residue, currency.exponent())
        .ok_or_else(|| MoneyError::Invalid(text.to_string()))?;
    Ok(Money { minor, currency })
}

/// Digits in each thousands group after the first, such as the `299` in `1.299`.
const THOUSANDS_GROUP_DIGITS: usize = 3;

/// Most digits a lone separator may be followed by and still be a decimal mark.
const MAX_LONE_DECIMAL_DIGITS: usize = 2;

/// A currency named in price text, before it is matched against the stated currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CurrencyMention {
    /// A currency named by its own ISO code or symbol, such as `EUR` or `€`.
    Named(Currency),
    /// `$`: the stated currency when it is a dollar, otherwise US dollars.
    Dollar,
    /// `kr`: a krona only when the stated currency is one, otherwise no currency.
    Krone,
}

/// Removes the currency mentions from a trimmed price text.
/// Returns the mentions in order and the text left once they are removed.
fn split_currency_mentions(text: &str) -> (Vec<CurrencyMention>, String) {
    let mut mentions = Vec::new();
    let mut residue = String::new();
    let mut remaining = text;
    while let Some(character) = remaining.chars().next() {
        let consumed = if let Some(mention) = symbol_mention(character) {
            mentions.push(mention);
            character.len_utf8()
        } else if character.is_alphabetic() {
            let word_length = remaining
                .find(|letter: char| !letter.is_alphabetic())
                .unwrap_or(remaining.len());
            let word = &remaining[..word_length];
            match word_mention(word) {
                Some(mention) => mentions.push(mention),
                None => residue.push_str(word),
            }
            word_length
        } else {
            residue.push(character);
            character.len_utf8()
        };
        remaining = &remaining[consumed..];
    }
    (mentions, residue)
}

/// Recognises a currency symbol: `€`, `£`, `¥` or `$`.
fn symbol_mention(character: char) -> Option<CurrencyMention> {
    match character {
        '€' => Some(CurrencyMention::Named(Currency::Eur)),
        '£' => Some(CurrencyMention::Named(Currency::Gbp)),
        '¥' => Some(CurrencyMention::Named(Currency::Jpy)),
        '$' => Some(CurrencyMention::Dollar),
        _ => None,
    }
}

/// Recognises a currency word: an ISO code, `zł`, `Kč` or `kr`, ignoring case.
fn word_mention(word: &str) -> Option<CurrencyMention> {
    match word.to_lowercase().as_str() {
        "zł" => Some(CurrencyMention::Named(Currency::Pln)),
        "kč" => Some(CurrencyMention::Named(Currency::Czk)),
        "kr" => Some(CurrencyMention::Krone),
        _ => Currency::from_code(word).map(CurrencyMention::Named),
    }
}

/// Works out the currency of a price from its mentions and the stated currency.
/// Returns `CurrencyMismatch` when two sources disagree and `NoCurrency` when neither
/// names a currency.
fn resolve_currency(
    mentions: &[CurrencyMention],
    stated: Option<Currency>,
) -> Result<Currency, MoneyError> {
    let mut mentioned: Option<Currency> = None;
    for &mention in mentions {
        let currency = mention_currency(mention, stated)?;
        if mentioned.is_some_and(|previous| previous != currency) {
            return Err(MoneyError::CurrencyMismatch);
        }
        mentioned = Some(currency);
    }
    match (mentioned, stated) {
        (Some(mentioned_currency), Some(stated_currency))
            if mentioned_currency != stated_currency =>
        {
            Err(MoneyError::CurrencyMismatch)
        }
        (Some(currency), _) | (None, Some(currency)) => Ok(currency),
        (None, None) => Err(MoneyError::NoCurrency),
    }
}

/// Resolves one mention to a currency, given the stated currency.
/// Returns `NoCurrency` for `kr` when the stated currency is not a krona.
fn mention_currency(
    mention: CurrencyMention,
    stated: Option<Currency>,
) -> Result<Currency, MoneyError> {
    match mention {
        CurrencyMention::Named(currency) => Ok(currency),
        CurrencyMention::Dollar => Ok(match stated {
            Some(currency @ (Currency::Usd | Currency::Cad | Currency::Aud)) => currency,
            _ => Currency::Usd,
        }),
        CurrencyMention::Krone => match stated {
            Some(currency @ (Currency::Sek | Currency::Nok | Currency::Dkk)) => Ok(currency),
            _ => Err(MoneyError::NoCurrency),
        },
    }
}

/// Reads the number left in a price once its currency is removed, as minor units with
/// `exponent` decimal places. Returns `None` when the text is not such a number.
fn minor_units(residue: &str, exponent: u32) -> Option<i64> {
    let number = clean_number(residue)?;
    let (integer_part, fraction_part) = split_fraction(&number)?;
    if fraction_part.len() > exponent as usize {
        return None;
    }
    let units = join_thousands_groups(integer_part)?;
    let padded_fraction = format!("{fraction_part:0<width$}", width = exponent as usize);
    format!("{units}{padded_fraction}").parse::<i64>().ok()
}

/// Removes the spaces and `'` thousands marks from a price's residue.
/// Returns `None` when anything besides digits, `.` and `,` remains.
fn clean_number(residue: &str) -> Option<String> {
    let number: String = residue
        .chars()
        .filter(|&character| !character.is_whitespace() && character != '\'')
        .collect();
    number
        .chars()
        .all(|character| character.is_ascii_digit() || is_separator(character))
        .then_some(number)
}

/// Splits a cleaned number into its integer part and its decimal fraction digits.
/// Returns `None` when the decimal mark is misplaced or has no digits after it.
fn split_fraction(number: &str) -> Option<(&str, &str)> {
    let Some(mark) = decimal_mark(number) else {
        return Some((number, ""));
    };
    let (integer_part, fraction_part) = number.rsplit_once(mark)?;
    if integer_part.contains(mark) || fraction_part.is_empty() {
        None
    } else {
        Some((integer_part, fraction_part))
    }
}

/// Returns the decimal mark of a number made of digits, `.` and `,`, or `None` when it has none.
/// With both kinds present the rightmost is the decimal mark. A lone kind is the decimal mark
/// when it appears once and is followed by 1 or 2 digits; otherwise it groups thousands.
fn decimal_mark(number: &str) -> Option<char> {
    let last = number
        .chars()
        .rev()
        .find(|&character| is_separator(character))?;
    let (before_last, after_last) = number.rsplit_once(last)?;
    let other = if last == '.' { ',' } else { '.' };
    if before_last.contains(other) {
        return Some(last);
    }
    let lone_decimal =
        !before_last.contains(last) && (1..=MAX_LONE_DECIMAL_DIGITS).contains(&after_last.len());
    lone_decimal.then_some(last)
}

/// Joins the digit groups of an integer part such as `1.299`, where every group after the
/// first has exactly `THOUSANDS_GROUP_DIGITS` digits. Returns `None` for a malformed group.
fn join_thousands_groups(integer_part: &str) -> Option<String> {
    let groups: Vec<&str> = integer_part.split(is_separator).collect();
    let well_formed = groups.iter().enumerate().all(|(index, group)| {
        is_digit_run(group) && (index == 0 || group.len() == THOUSANDS_GROUP_DIGITS)
    });
    well_formed.then(|| groups.concat())
}

/// Returns true for a non-empty run of ASCII digits.
fn is_digit_run(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|character| character.is_ascii_digit())
}

/// Returns true for `.` or `,`, the two separators a price number may use.
fn is_separator(character: char) -> bool {
    character == '.' || character == ','
}

impl fmt::Display for Money {
    /// Formats the amount with the currency's decimal places and `.` as the decimal mark,
    /// then the ISO code, such as `1299.00 EUR` or `1299 JPY`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.minor < 0 { "-" } else { "" };
        let magnitude = self.minor.unsigned_abs();
        let exponent = self.currency.exponent();
        let divisor = 10_u64.pow(exponent);
        let units = magnitude / divisor;
        let code = self.currency.code();
        if exponent == 0 {
            write!(formatter, "{sign}{units} {code}")
        } else {
            let fraction = magnitude % divisor;
            write!(
                formatter,
                "{sign}{units}.{fraction:0width$} {code}",
                width = exponent as usize
            )
        }
    }
}

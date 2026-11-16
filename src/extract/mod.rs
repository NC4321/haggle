//! Price extraction from page HTML: the ordered chain of extractors and the shared result types.

use std::fmt;

use scraper::Html;

use crate::money::Money;

pub mod jsonld;
pub mod meta;
pub mod microdata;
pub mod selector;

/// Schema.org prefixes that may precede an availability value.
const SCHEMA_PREFIXES: [&str; 2] = ["http://schema.org/", "https://schema.org/"];

/// Whether the product can be bought.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    InStock,
    OutOfStock,
    Preorder,
    Unknown,
}

impl Availability {
    /// Maps a schema.org availability value (URL or bare word, any case or
    /// separators) to an `Availability`; unrecognised values give `Unknown`.
    pub fn from_schema(value: &str) -> Availability {
        let value = value.trim();
        let value = SCHEMA_PREFIXES
            .iter()
            .find_map(|prefix| value.strip_prefix(prefix))
            .unwrap_or(value);
        let normalized: String = value
            .chars()
            .filter(|character| !matches!(character, ' ' | '_' | '-'))
            .flat_map(char::to_lowercase)
            .collect();
        match normalized.as_str() {
            "instock" | "limitedavailability" | "onlineonly" | "instoreonly" => {
                Availability::InStock
            }
            "outofstock" | "soldout" | "discontinued" | "oos" => Availability::OutOfStock,
            "preorder" | "presale" | "backorder" => Availability::Preorder,
            _ => Availability::Unknown,
        }
    }
}

impl fmt::Display for Availability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Availability::InStock => "in stock",
            Availability::OutOfStock => "out of stock",
            Availability::Preorder => "preorder",
            Availability::Unknown => "unknown",
        })
    }
}

/// Which extractor produced an `Extraction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    JsonLd,
    Meta,
    Microdata,
    Selector,
}

impl fmt::Display for Source {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Source::JsonLd => "json-ld",
            Source::Meta => "meta",
            Source::Microdata => "microdata",
            Source::Selector => "selector",
        })
    }
}

/// What was found on a product page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extraction {
    pub price: Money,
    /// The shop's own "was" price, if it states one.
    pub list_price: Option<Money>,
    pub availability: Availability,
    pub title: Option<String>,
    pub source: Source,
}

/// Why no price could be extracted.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ExtractError {
    #[error("no price found on the page")]
    NoPrice,
    #[error("the page states several different prices")]
    AmbiguousPrice,
    #[error("invalid CSS selector: {0}")]
    BadSelector(String),
}

/// An extractor that reads a price from a parsed page.
type DocumentExtractor = fn(&Html) -> Result<Extraction, ExtractError>;

/// Structured-data extractors, in the order they are tried.
const STRUCTURED_EXTRACTORS: [DocumentExtractor; 3] =
    [jsonld::extract, meta::extract, microdata::extract];

/// Extracts the product price from `html`, trying the extractors in order.
///
/// Structured data (JSON-LD, meta tags, microdata) is tried before the user's
/// CSS selector, and the first extractor that succeeds wins. The page text is
/// never used as a price source.
///
/// `selector` is an optional user CSS selector for the price element.
/// Returns `ExtractError::BadSelector` when the selector does not parse,
/// `ExtractError::AmbiguousPrice` when some extractor found several different
/// prices and none succeeded, and `ExtractError::NoPrice` otherwise.
pub fn extract(html: &str, selector: Option<&str>) -> Result<Extraction, ExtractError> {
    let doc = Html::parse_document(html);
    let structured = STRUCTURED_EXTRACTORS
        .iter()
        .map(|extractor| extractor(&doc));
    let css = selector
        .into_iter()
        .map(|css_selector| selector::extract(&doc, css_selector));
    let mut saw_ambiguous = false;
    for attempt in structured.chain(css) {
        match attempt {
            Ok(extraction) => return Ok(extraction),
            Err(error @ ExtractError::BadSelector(_)) => return Err(error),
            Err(ExtractError::AmbiguousPrice) => saw_ambiguous = true,
            Err(ExtractError::NoPrice) => {}
        }
    }
    if saw_ambiguous {
        Err(ExtractError::AmbiguousPrice)
    } else {
        Err(ExtractError::NoPrice)
    }
}

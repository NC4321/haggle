//! Extractor reading the price from an element chosen by a user CSS selector.

use super::{ExtractError, Extraction};

/// Extracts the product price from the element matched by `selector`.
///
/// Returns `ExtractError::BadSelector` when the selector does not parse and
/// `ExtractError::NoPrice` when it matches no price.
pub fn extract(_doc: &scraper::Html, _selector: &str) -> Result<Extraction, ExtractError> {
    unimplemented!("extract")
}

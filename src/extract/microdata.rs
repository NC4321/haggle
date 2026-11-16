//! Extractor reading the price from schema.org microdata.

use super::{ExtractError, Extraction};

/// Extracts the product price from schema.org microdata in the parsed page.
///
/// Returns `ExtractError::NoPrice` when none is present and
/// `ExtractError::AmbiguousPrice` when it states several different prices.
pub fn extract(_doc: &scraper::Html) -> Result<Extraction, ExtractError> {
    unimplemented!("extract")
}

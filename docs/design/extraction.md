# Price extraction design notes

How haggle reads a price from a fetched product page. The code lives in `src/extract/` (`mod.rs`, `jsonld.rs`, `meta.rs`, `microdata.rs`, `selector.rs`) and `src/money.rs`. These notes describe what the code does today.

## The ordered chain

`extract(html, selector)` in `src/extract/mod.rs` parses the page once and tries the extractors in a fixed order:

1. `jsonld::extract`
2. `meta::extract`
3. `microdata::extract`
4. `selector::extract`, only when the watch has a user CSS selector

The first extractor that returns `Ok` wins and its `Source` (`json-ld`, `meta`, `microdata`, `selector`) is recorded in the `Extraction`. Visible page text is never scanned for a price.

Structured data comes first because shops publish it for search engines, so it survives redesigns. A CSS selector depends on the page's markup and breaks when the layout changes. The selector is therefore the last resort, used when no structured source gives a price.

## What each source looks for

Every extractor returns `NoPrice` when it finds nothing and `AmbiguousPrice` when it finds several different prices. Several copies of the same price are fine.

- **JSON-LD** (`jsonld.rs`): `<script type="application/ld+json">` blocks (type compared case-insensitively). Blocks that are not valid JSON are skipped. It walks top-level arrays and `@graph`, keeps nodes of `@type` `Product`, and reads each entry of `offers`. An `Offer` uses `price`, else an untyped `priceSpecification`. An `AggregateOffer` uses `price`, else `lowPrice`. The currency comes from `priceCurrency`. The title is the product `name`.
- **Meta tags** (`meta.rs`): `product:price:amount`, else `og:price:amount`, with the currency from `product:price:currency` or `og:price:currency`. The key may be in `property` or `name`. The title is `og:title`.
- **Microdata** (`microdata.rs`): `itemprop="price"` inside top-level `Product` scopes, optionally inside an `Offer`. A value comes from `content`, else the `href` of a `<link>`, else the element text. The currency is the `priceCurrency` in the same offer, else the one in the product. The title is the `name` property.
- **Selector** (`selector.rs`): every element matching the user's selector. A value comes from the `content` attribute, else the element text. Matches that do not parse as a price with a currency are skipped. The title is the `<title>` element.

## List ("was") price and availability

- `Extraction::list_price` is the shop's own "was" price. JSON-LD reads it from a `priceSpecification` whose `priceType` ends in `ListPrice` or `StrikethroughPrice`. Meta reads `product:original_price:amount` and `product:original_price:currency`, falling back to the price's currency. A list price equal to the price is dropped. Microdata and selector never set one.
- `Availability` is `InStock`, `OutOfStock`, `Preorder` or `Unknown`. `Availability::from_schema` in `mod.rs` maps schema.org values, as a URL or a bare word, ignoring case, spaces, `_` and `-`. For example `InStock` and `LimitedAvailability` give `InStock`, `SoldOut` and `Discontinued` give `OutOfStock`, and `BackOrder` gives `Preorder`. Anything else gives `Unknown`.
- JSON-LD reads `availability` on the offer. Meta reads `product:availability`, else `og:availability`. Microdata reads the `availability` property. The selector source always reports `Unknown`.

## Parsing price strings into `Money`

`money::parse_price(text, currency)` in `src/money.rs` returns `Money { minor, currency }`, the amount in minor units (cents, or yen for JPY). `currency` is the code stated elsewhere on the page, if any.

- Supported currencies (`Currency`): USD, EUR, GBP, CHF, CAD, AUD, JPY, SEK, NOK, DKK, PLN, CZK. JPY has no decimals, the others have two.
- Currency can be named in the text by an ISO code, `€`, `£`, `¥`, `$`, `zł`, `Kč` or `kr`. `$` means the stated currency when that is a dollar, else USD. `kr` is a krona only when the stated currency is one.
- The text and the stated currency must agree, or `MoneyError::CurrencyMismatch` is returned. With neither, the result is `MoneyError::NoCurrency`. Empty text gives `MoneyError::Empty`, and other text gives `MoneyError::Invalid`.
- Spaces and `'` are dropped as thousands marks. When both `.` and `,` appear, the rightmost is the decimal mark. A lone separator is the decimal mark only if it appears once and is followed by one or two digits. Otherwise it groups thousands, and every group after the first must have exactly three digits.
- So `$1,299.00` and `1.299,00 €` are read as expected, and `1.299` is 1299, not 1.299.

## When each `ExtractError` is returned

- `BadSelector(message)`: the user's selector is not valid CSS. It is raised only when the chain reaches the selector, so a page with structured data never reports it. It stops the chain at once.
- `AmbiguousPrice`: some extractor found several different prices and no later extractor succeeded.
- `NoPrice`: nothing found a price, and no extractor was ambiguous.

## The `needs-selector` status

In `failure_status` (`src/scheduler.rs`), a `NoPrice` or `AmbiguousPrice` on a watch with no selector sets the status to `needs-selector`. For the user this means the page has no usable structured price, or several conflicting ones. They should open the watch and give a CSS selector for the price element. The same failures on a watch that already has a selector, and a `BadSelector`, give the status `error`. The status values are defined in `src/db/mod.rs`.

## Why there is no headless browser

Haggle does not run JavaScript, so pages that render their price client-side are unsupported. `docs/plan/PLAN.md` records the reason: bundling Chromium would be most of the binary and most of the RAM on a small server, for a minority of pages. These pages end up as `needs-selector`.

## How it is tested

- The extractor tests are `tests/extract_jsonld.rs`, `tests/extract_meta.rs`, `tests/extract_microdata.rs`, `tests/extract_selector.rs`, `tests/extract.rs` (the whole chain) and `tests/money.rs`.
- They run over small hand-written HTML pages in `tests/fixtures/` (`jsonld/`, `meta/`, `microdata/`, `selector/`, `chain/`, `cli/`).
- The expected prices are written by hand in the tests, in minor units, and are not produced by running the extractor. For example, `chain/jsonld_and_meta.html` carries two different prices so a test can check that JSON-LD wins.
- The `chain/` fixtures cover ordering, fallback, ambiguity and the `plain_text_only.html` case, where a price appears only in page text and none is found.

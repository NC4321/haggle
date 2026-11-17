//! Tests for `haggle::fetch::robots_allows` and `haggle::fetch::backoff_delay`. Both are pure.

use std::time::Duration;

use haggle::fetch::{MAX_BACKOFF, ROBOTS_AGENT, backoff_delay, robots_allows};

/// A whole number of hours as a `Duration`.
fn hours(count: u64) -> Duration {
    Duration::from_secs(count * 3600)
}

/// Whether robots.txt `robots` lets the product agent fetch `path`.
fn allowed(robots: &str, path: &str) -> bool {
    robots_allows(robots, ROBOTS_AGENT, path)
}

#[test]
fn robots_agent_is_haggle() {
    assert_eq!(ROBOTS_AGENT, "haggle");
}

#[test]
fn max_backoff_is_seven_days() {
    // 7 days * 24 hours * 3600 seconds = 604800 seconds.
    assert_eq!(MAX_BACKOFF, Duration::from_secs(604_800));
}

#[test]
fn empty_robots_file_allows_every_path() {
    assert!(allowed("", "/a"));
}

#[test]
fn wildcard_disallow_root_blocks_path() {
    assert!(!allowed("User-agent: *\nDisallow: /\n", "/a"));
}

#[test]
fn haggle_group_allowing_all_overrides_wildcard_disallow() {
    // Only the haggle group applies, and its `Allow: /` matches /a.
    let robots = "User-agent: *\nDisallow: /\n\nUser-agent: haggle\nAllow: /\n";

    assert!(allowed(robots, "/a"));
}

#[test]
fn haggle_group_without_rules_overrides_wildcard_disallow() {
    // The haggle group is used even though it has no rules, so no rule blocks /a.
    let robots = "User-agent: *\nDisallow: /\n\nUser-agent: haggle\n";

    assert!(allowed(robots, "/a"));
}

#[test]
fn user_agent_value_matches_case_insensitively() {
    assert!(!allowed("User-Agent: HAGGLE\nDisallow: /\n", "/a"));
}

#[test]
fn field_names_match_case_insensitively() {
    assert!(!allowed("user-agent: haggle\ndisallow: /\n", "/a"));
}

#[test]
fn group_for_another_agent_does_not_apply_without_wildcard_group() {
    assert!(allowed("User-agent: googlebot\nDisallow: /\n", "/a"));
}

#[test]
fn haggle_groups_are_merged() {
    let robots = "User-agent: haggle\nDisallow: /a\n\nUser-agent: haggle\nDisallow: /b\n";

    assert!(!allowed(robots, "/a"), "first group's rule should apply");
    assert!(!allowed(robots, "/b"), "second group's rule should apply");
    assert!(allowed(robots, "/c"));
}

#[test]
fn group_with_two_user_agent_lines_applies_to_haggle() {
    let robots = "User-agent: bot\nUser-agent: haggle\nDisallow: /private\n";

    assert!(!allowed(robots, "/private/x"));
    assert!(allowed(robots, "/public"));
}

#[test]
fn disallow_prefix_blocks_subpaths_and_allows_siblings() {
    let robots = "User-agent: *\nDisallow: /private\n";

    assert!(!allowed(robots, "/private/x"));
    assert!(allowed(robots, "/public"));
}

#[test]
fn disallow_prefix_also_blocks_path_with_query() {
    // The path includes its ?query, and a prefix rule matches the start of it.
    assert!(!allowed(
        "User-agent: *\nDisallow: /search\n",
        "/search?q=1"
    ));
}

#[test]
fn longer_allow_beats_shorter_disallow() {
    // "/private/ok" is 11 characters, longer than the 8-character "/private".
    let robots = "User-agent: *\nDisallow: /private\nAllow: /private/ok\n";

    assert!(allowed(robots, "/private/ok"));
    assert!(!allowed(robots, "/private/other"));
}

#[test]
fn longer_allow_beats_shorter_disallow_when_allow_comes_first() {
    let robots = "User-agent: *\nAllow: /private/ok\nDisallow: /private\n";

    assert!(allowed(robots, "/private/ok"));
}

#[test]
fn equal_length_allow_and_disallow_allows() {
    // Both patterns are 5 characters and both match /shop/cart, so Allow wins the tie.
    let robots = "User-agent: *\nDisallow: /shop\nAllow: /shop\n";

    assert!(allowed(robots, "/shop/cart"));
}

#[test]
fn dollar_anchor_with_wildcard_blocks_pdf_only_at_end_of_path() {
    let robots = "User-agent: *\nDisallow: /*.pdf$\n";

    assert!(!allowed(robots, "/a/b.pdf"));
    // The `$` needs the path to end after "pdf", and "?x=1" follows it.
    assert!(allowed(robots, "/a/b.pdf?x=1"));
}

#[test]
fn dollar_anchor_without_wildcard_requires_exact_end() {
    let robots = "User-agent: *\nDisallow: /a$\n";

    assert!(!allowed(robots, "/a"));
    assert!(allowed(robots, "/ab"));
}

#[test]
fn star_matches_any_run_of_characters() {
    // The `*` stands for "?" in /search?q=1.
    assert!(!allowed(
        "User-agent: *\nDisallow: /search*q=\n",
        "/search?q=1"
    ));
}

#[test]
fn empty_disallow_value_matches_nothing() {
    assert!(allowed("User-agent: *\nDisallow:\n", "/a"));
}

#[test]
fn comments_and_blank_lines_are_ignored() {
    let robots = "# site policy\n\nUser-agent: * # everyone\nDisallow: /private # keep out\n\n";

    assert!(!allowed(robots, "/private/x"));
    assert!(allowed(robots, "/public"));
}

#[test]
fn sitemap_line_is_not_a_rule() {
    // Read as a rule, `Sitemap: /` would block /a.
    assert!(allowed("User-agent: *\nSitemap: /\n", "/a"));
}

#[test]
fn crawl_delay_line_is_not_a_rule() {
    // Read as a rule, `Crawl-delay: /` would block /a.
    assert!(allowed("User-agent: *\nCrawl-delay: /\n", "/a"));
}

#[test]
fn zero_failures_gives_the_interval() {
    assert_eq!(backoff_delay(hours(6), 0), hours(6));
}

#[test]
fn one_failure_doubles_the_interval() {
    // 6 hours * 2^1 = 12 hours.
    assert_eq!(backoff_delay(hours(6), 1), hours(12));
}

#[test]
fn three_failures_multiply_the_interval_by_eight() {
    // 6 hours * 2^3 = 48 hours.
    assert_eq!(backoff_delay(hours(6), 3), hours(48));
}

#[test]
fn backoff_is_capped_at_max_backoff() {
    // 6 hours * 2^10 = 6144 hours, far above the 168-hour cap.
    assert_eq!(backoff_delay(hours(6), 10), MAX_BACKOFF);
}

#[test]
fn backoff_below_the_cap_is_not_capped() {
    // 1 hour * 2^7 = 128 hours, under the 168-hour cap.
    assert_eq!(backoff_delay(hours(1), 7), hours(128));
}

#[test]
fn backoff_above_the_cap_gives_max_backoff() {
    // 1 hour * 2^8 = 256 hours, over the 168-hour cap.
    assert_eq!(backoff_delay(hours(1), 8), MAX_BACKOFF);
}

#[test]
fn huge_failure_count_gives_max_backoff_without_panicking() {
    assert_eq!(backoff_delay(hours(1), u32::MAX), MAX_BACKOFF);
}

#[test]
fn interval_above_max_backoff_is_kept_at_zero_failures() {
    // 14 days is 336 hours, over the 168-hour cap. The interval is never shortened.
    assert_eq!(backoff_delay(hours(336), 0), hours(336));
}

#[test]
fn interval_above_max_backoff_is_kept_with_failures() {
    // The doubled 672 hours is capped to 168 hours, but the 336-hour interval is the floor.
    assert_eq!(backoff_delay(hours(336), 1), hours(336));
}

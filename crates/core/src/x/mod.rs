//! Parsing X's GraphQL payloads.
//!
//! The split here mirrors the four genuinely different problems:
//!
//! - [`ops`] — recognising *which* responses matter (operation names, not hashes)
//! - [`envelope`] — finding tweets inside whatever wrapper they arrived in
//! - [`tweet`] — turning one tweet object into the domain model
//! - [`date`] — X's Ruby-era timestamp format
//!
//! [`envelope`] and [`tweet`] are the only two layers that need updating when
//! X changes shape, which is the point of separating them: a new wrapper is an
//! envelope fix, a renamed field is a tweet fix, and neither requires touching
//! the other.

pub mod date;
pub mod envelope;
pub mod ops;
pub mod tweet;

pub use date::{format_local, parse_x_timestamp};
pub use envelope::{extract, Extracted};
pub use ops::{is_capture_operation, is_graphql_url, is_signal_operation, operation_name};
pub use tweet::{parse_tweet, parse_tweet_result, PARSER_VERSION};

//! The digest probe suite — fail-hard behavioral probes over disposable
//! scratch databases (the module docs in `probes/common` carry the
//! contract: a skipped probe is a FAILED probe).
//!
//! Run: `cargo test --test digest_probe_cases` (the scratch Postgres on
//! localhost:5433 must be up; the harness panics loudly when it is not).

mod probes;

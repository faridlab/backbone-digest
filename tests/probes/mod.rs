//! The probe modules (one file per behavioral cluster; the shared
//! fail-hard harness lives in [`common`]).
//!
//! Tenancy note (ADR-0029): the module ships no scoping column and no RLS
//! fence, so there is no storage-layer fence probe here — cross-org
//! invisibility of digest rows is asserted by the COMPOSING service's
//! suite, where the tenancy decorator lives.

pub mod auto_subscribe;
pub mod common;
pub mod render_fence;
pub mod slowdown_ladder;
pub mod unsubscribe_roundtrip;
pub mod unsubscribe_wire;

//! The probe modules (one file per behavioral cluster; the shared
//! fail-hard harness lives in [`common`]).

pub mod auto_subscribe;
pub mod common;
pub mod company_fence_rls;
pub mod render_fence;
pub mod slowdown_ladder;
pub mod unsubscribe_roundtrip;
pub mod unsubscribe_wire;

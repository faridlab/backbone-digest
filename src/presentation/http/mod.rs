//! HTTP presentation: the generated entity handlers + the hand-written
//! public route family.

pub mod digest_digest_handler;
pub mod digest_digest_kpi_handler;
pub mod digest_subscription_handler;
pub mod digest_tip_handler;
pub mod digest_tip_user_handler;

// <<< CUSTOM
pub mod public_routes;
// END CUSTOM

// Re-exports (the module's route composers — lib.rs's all_crud_routes() and
// readonly_routes() consume these; `routes/generated.rs` stays an
// unreferenced generator artifact, the family shape).
pub use digest_digest_handler::{
    create_digest_digest_routes, create_digest_digest_read_routes, create_digest_digest_write_routes,
};
pub use digest_digest_kpi_handler::{
    create_digest_digest_kpi_routes, create_digest_digest_kpi_read_routes,
    create_digest_digest_kpi_write_routes,
};
pub use digest_subscription_handler::{
    create_digest_subscription_routes, create_digest_subscription_read_routes,
    create_digest_subscription_write_routes,
};
pub use digest_tip_handler::{
    create_digest_tip_routes, create_digest_tip_read_routes, create_digest_tip_write_routes,
};
pub use digest_tip_user_handler::{
    create_digest_tip_user_routes, create_digest_tip_user_read_routes,
    create_digest_tip_user_write_routes,
};

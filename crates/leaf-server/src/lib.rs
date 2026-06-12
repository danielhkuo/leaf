//! leaf HTTP server (axum): the bootstrap setup UI (setup mode), and — in
//! later phases — the REST API, media proxy, and embedded-app static
//! hosting. This server always starts before the gateway; in setup mode it
//! is the only thing running.

pub mod run;
pub mod setup;
pub mod validate;

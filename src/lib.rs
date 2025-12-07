#[path = "generated/daylight_generated.rs"]
#[allow(warnings)]
pub mod daylight_generated;

pub mod client;
pub mod languages;
pub mod server;

#[cfg(test)]
mod server_tests;

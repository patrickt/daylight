#[path = "generated/daylight_generated.rs"]
#[allow(warnings)]
pub mod daylight_generated;

#[path = "client.rs"]
pub mod client;

#[path = "languages.rs"]
pub mod languages;

#[path = "server.rs"]
pub mod server;

#[cfg(test)]
#[path = "server_tests.rs"]
mod server_tests;

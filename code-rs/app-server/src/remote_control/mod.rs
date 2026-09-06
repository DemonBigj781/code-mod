#[allow(dead_code)]
pub(crate) mod auth;
#[cfg(test)]
mod auth_tests;
#[allow(dead_code)]
pub(crate) mod enroll;
#[cfg(test)]
mod enroll_tests;
#[allow(dead_code)]
pub(crate) mod host_device;
#[allow(dead_code)]
pub(crate) mod protocol;
#[cfg(test)]
mod protocol_tests;
#[allow(dead_code)]
pub(crate) mod server_api;
#[cfg(test)]
mod server_api_tests;
#[allow(dead_code)]
pub(crate) mod state;

#[cfg(test)]
mod state_tests;

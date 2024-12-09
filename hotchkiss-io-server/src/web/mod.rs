pub mod router;

//Only propogate frontend if in release mode
#[cfg(debug_assertions)]
pub mod dev_frontend;

#[cfg(not(debug_assertions))]
pub mod frontend;

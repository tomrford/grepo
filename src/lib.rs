mod app;
mod git;
mod manifest;
mod mutation_lock;
mod store;
mod util;

#[cfg(test)]
mod integration_tests;

pub use app::main_entry;

pub mod commands;
pub mod coordinator;
pub mod events;
pub mod ipc;
mod session;
pub mod shortcuts;
pub mod single_instance;
pub mod startup;
pub mod state;

pub fn run() {
    state::run_app();
}

pub mod cursor;

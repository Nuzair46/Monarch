pub mod commands;
pub mod events;
pub mod ipc;
pub mod shortcuts;
pub mod single_instance;
pub mod startup;
pub mod state;

pub fn run() {
    state::run_app();
}

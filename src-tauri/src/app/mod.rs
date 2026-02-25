pub mod commands;
pub mod events;
pub mod single_instance;
pub mod state;
pub mod startup;

pub fn run() {
    state::run_app();
}

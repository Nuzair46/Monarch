#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    monarch_desktop_lib::app::run();
}

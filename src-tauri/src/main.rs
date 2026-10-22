// Prevent a console window from appearing alongside the GUI on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    minearcade_lib::run()
}

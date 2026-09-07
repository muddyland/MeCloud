// Windows would otherwise open a console behind the app; harmless elsewhere.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    mecloud_desktop_lib::run()
}

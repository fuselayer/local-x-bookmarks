// Release builds on Windows must not pop a console window alongside the app.
// Debug builds keep it, because that is where panics and eprintln go.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    xdl_app::run();
}

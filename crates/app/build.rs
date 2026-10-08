fn main() {
    // Reads tauri.conf.json, validates it, and generates the context the
    // `generate_context!` macro expands at compile time.
    tauri_build::build()
}

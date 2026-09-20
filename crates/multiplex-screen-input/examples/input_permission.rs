//! Reports whether this process may inject input. Moves nothing.
//!
//! ```sh
//! cargo run -p multiplex-screen-input --example input_permission
//! ```

#[cfg(target_os = "macos")]
fn main() {
    match multiplex_screen_input::CoreGraphicsSink::new() {
        Ok(_) => println!("input injection: allowed"),
        Err(error) => {
            println!("input injection: {error}");
            println!(
                "Allow the terminal running this example in System Settings → Privacy & Security → Accessibility."
            );
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("input injection: no backend on this platform yet");
}

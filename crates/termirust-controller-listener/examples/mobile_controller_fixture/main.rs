//! The Controller host the iOS and Android device tests pair with. It runs Session Hosts over
//! Unix sockets, so on other platforms it builds but does nothing.

#[cfg(unix)]
mod fixture;

fn main() {
    #[cfg(unix)]
    fixture::main();
}

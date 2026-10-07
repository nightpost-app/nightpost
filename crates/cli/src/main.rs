fn main() {
    println!(
        "nightpost {} (protocol {})",
        env!("CARGO_PKG_VERSION"),
        nightpost_core::PROTOCOL_VERSION
    );
}

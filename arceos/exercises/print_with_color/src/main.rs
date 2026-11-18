#![cfg_attr(feature = "axstd", no_std)]
#![cfg_attr(feature = "axstd", no_main)]

#[cfg(feature = "axstd")]
use axstd::println;

/// ANSI SGR sequences: color the greeting so the console shows escape codes.
const RED: &str = "\x1b[1;31m";
const GREEN: &str = "\x1b[1;32m";
const RESET: &str = "\x1b[0m";

#[cfg_attr(feature = "axstd", no_mangle)]
fn main() {
    println!("{RED}[WithColor]{RESET}: {GREEN}Hello, Arceos!{RESET}");
}

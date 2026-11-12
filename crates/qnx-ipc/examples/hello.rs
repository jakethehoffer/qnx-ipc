//! Prints the OS this binary runs on, then exits with the code given as the first
//! argument (if any). Used to check the QNX toolchain and the `tools/qnx-run` runner.

fn main() {
    println!("hello from {}", std::env::consts::OS);
    if let Some(code) = std::env::args().nth(1).and_then(|a| a.parse::<i32>().ok()) {
        std::process::exit(code);
    }
}

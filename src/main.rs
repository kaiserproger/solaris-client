fn main() {
    if let Err(error) = solaris_client::cli(std::env::args().skip(1)) {
        eprintln!("solaris-client: {error}");
        std::process::exit(1);
    }
}

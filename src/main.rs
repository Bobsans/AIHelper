fn main() {
    if let Err(error) = aihelper::run() {
        error.print();
        std::process::exit(error.exit_code());
    }
}

fn main() {
    if let Err(error) = aihelper::initialize_vault_master_key().and_then(|_| aihelper::run()) {
        error.print();
        std::process::exit(error.exit_code());
    }
}

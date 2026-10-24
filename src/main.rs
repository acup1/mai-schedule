fn main() {
    let code = match mai_schedule::cli::run(std::env::args().skip(1).collect()) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("Ошибка: {error}");
            1
        }
    };
    std::process::exit(code);
}

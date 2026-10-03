#[tokio::main]
async fn main() {
    match multi_ai::cli::main_entry(std::env::args().collect()).await {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("Error: {error:#}");
            std::process::exit(
                if error.downcast_ref::<multi_ai::cli::UsageError>().is_some() {
                    2
                } else {
                    1
                },
            );
        }
    }
}

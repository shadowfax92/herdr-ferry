use clap::Parser;
use herdr_ferry::Cli;

fn main() {
    let cli = Cli::parse();
    if let Err(error) = herdr_ferry::run(cli.command) {
        eprintln!("Herdr Ferry: {error:#}");
        std::process::exit(1);
    }
}

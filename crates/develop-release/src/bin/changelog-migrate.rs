#[path = "../changelog_migrate.rs"]
mod changelog_migrate;

fn main() {
    std::process::exit(changelog_migrate::run(std::env::args().skip(1).collect()));
}

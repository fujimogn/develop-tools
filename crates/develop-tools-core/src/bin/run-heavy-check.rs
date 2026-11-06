use develop_tools_core::heavy_check::{
    self, MonotonicClock,
    state::{NativeFilesystem, OsAccount},
};
fn main() {
    std::process::exit(heavy_check::run(
        std::env::args_os().skip(1).collect(),
        &OsAccount,
        &NativeFilesystem,
        &MonotonicClock::default(),
    ));
}

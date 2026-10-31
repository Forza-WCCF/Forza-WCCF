//! `forza-server`: the Linux server (ovh1) and the 2010-11 kit's `programs\forza-server.exe`; the command line is
//! [`forza_server::cli`], which the 2017-18 kit runs as `Launcher.exe server ...`.

fn main() {
    forza_server::cli::main(std::env::args_os())
}

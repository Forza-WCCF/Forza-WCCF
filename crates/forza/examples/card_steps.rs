//! `card_steps DATA ARGS...`: one club card step (forza::cards::steps::run) on the kit data folder DATA, as
//! `python code\card_steps.py ARGS` runs it - for comparing the two on copies of a kit's data.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((data, rest)) = args.split_first() else {
        eprintln!("card_steps DATA start [board] SEAT | keep SEAT | view");
        std::process::exit(2);
    };
    let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
    let (code, out) = forza::cards::steps::run(std::path::Path::new(data), &rest);
    print!("{out}");
    std::process::exit(code);
}

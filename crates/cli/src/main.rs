use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("ghola: cannot read the current directory: {e}");
            return ExitCode::from(2);
        }
    };
    let env = ghola_cli::Env::from_process();
    let code = ghola_cli::run(
        &args,
        &cwd,
        &env,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    );
    ExitCode::from(code as u8)
}

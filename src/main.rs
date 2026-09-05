//! The `mesh` binary: parse, dispatch, map the error to an exit status. Nothing else.

use std::process::ExitCode;

use clap::Parser;

use mesh::cli::globals::GlobalOpts;
use mesh::cli::{self, Cli};
use mesh::ctx::Ctx;

fn main() -> ExitCode {
    // A panic must never print a Rust backtrace on a user-facing path.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(run);
    std::panic::set_hook(previous);
    let code = match outcome {
        Ok(code) => code,
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unexpected failure".to_string());
            eprintln!("internal error: {message}");
            1
        }
    };
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

fn run() -> i32 {
    let parsed = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let code = if e.use_stderr() { 2 } else { 0 };
            let _ = e.print();
            return code;
        }
    };

    if parsed.version {
        println!("{}", mesh::VERSION);
        return 0;
    }

    let globals = GlobalOpts {
        json: parsed.json,
        quiet: parsed.quiet,
        owner: parsed.owner,
        mine: parsed.mine,
        config: parsed.config,
        vault: parsed.vault,
    };
    let mut ctx = Ctx::new(globals);

    let Some(command) = parsed.command else {
        return cli::help_to_stdout(&[]);
    };

    match cli::dispatch(&mut ctx, command) {
        Ok(code) => code,
        Err(e) => {
            cli::out::render_error(&ctx, &e);
            e.code()
        }
    }
}

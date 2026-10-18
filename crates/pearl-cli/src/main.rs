//! `pearl`: the dashboard (no command) or one command in plain mode, text or `--json`.
//! Both run the same `commands::exec`. See docs/CLI.md.

mod brand;
mod commands;
mod ctx;
mod fmt;
mod html;
mod snapshot;
mod solana;
mod ui;

use clap::Parser;

#[derive(Parser)]
#[command(name = "pearl", version, disable_help_subcommand = true, about = "Dashboard and CLI for the agent chain and its Solana bridge (name is a placeholder)")]
struct Cli {
    #[command(flatten)]
    opts: ctx::Opts,
    #[command(subcommand)]
    cmd: Option<commands::Cmd>,
}

fn main() {
    let cli = Cli::parse();
    let json = cli.opts.json;
    let code = match run(cli) {
        Ok(()) => 0,
        Err(e) => {
            if json {
                println!("{}", serde_json::json!({"ok": false, "error": format!("{e:#}")}));
            } else {
                eprintln!("error: {e:#}");
            }
            1
        }
    };
    std::process::exit(code);
}

fn run(cli: Cli) -> anyhow::Result<()> {
    let json = cli.opts.json;
    let ctx = ctx::Ctx::new(cli.opts)?;
    match cli.cmd {
        None | Some(commands::Cmd::Tui) => ui::run(ctx),
        Some(cmd) => {
            let out = commands::exec(&ctx, &cmd, false)?;
            if json {
                let mut j = out.json;
                if let Some(o) = j.as_object_mut() {
                    o.insert("ok".into(), serde_json::json!(true));
                }
                println!("{j}");
            } else {
                println!("{}", out.text);
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    #[test]
    fn clap_definitions_are_valid() {
        super::Cli::command().debug_assert();
        crate::commands::Line::command().debug_assert();
    }
}

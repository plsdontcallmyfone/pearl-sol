//! `pearl`: one command in text mode (steps + an info box) or `--json`; with no command,
//! an overview box; `pearl dashboard` opens the full-screen dashboard. All run the same
//! `commands::exec`. See docs/CLI.md.

mod brand;
mod commands;
mod ctx;
mod fmt;
mod html;
mod snapshot;
mod solana;
#[cfg(test)]
mod stub;
mod term;
mod ui;

use clap::Parser;

#[derive(Parser)]
#[command(name = "pearl", version, disable_help_subcommand = true, about = "CLI and dashboard for the agent chain and its Solana bridge (name is a placeholder)")]
struct Cli {
    #[command(flatten)]
    opts: ctx::Opts,
    #[command(subcommand)]
    cmd: Option<commands::Cmd>,
}

fn main() {
    let cli = Cli::parse();
    let json = cli.opts.json;
    let style = term::Style::detect();
    let (r, shown) = run(cli, style);
    let code = match r {
        Ok(()) => 0,
        Err(e) => {
            if json {
                println!("{}", serde_json::json!({"ok": false, "error": format!("{e:#}")}));
            } else if !shown {
                eprintln!("{}", style.step_line(false, &format!("error: {e:#}")));
            }
            1
        }
    };
    std::process::exit(code);
}

/// The result, and whether a step already printed its error.
fn run(cli: Cli, style: term::Style) -> (anyhow::Result<()>, bool) {
    let json = cli.opts.json;
    let mut ctx = match ctx::Ctx::new(cli.opts) {
        Ok(c) => c,
        Err(e) => return (Err(e), false),
    };
    if matches!(cli.cmd, Some(commands::Cmd::Dashboard)) {
        return (ui::run(ctx), false);
    }
    if !json {
        ctx.ui = term::Ui::plain(style);
    }
    let r = (|| -> anyhow::Result<()> {
        let out = match &cli.cmd {
            Some(cmd) => commands::exec(&ctx, cmd, false)?,
            // no command: the overview box (JSON: what `status` returns)
            None => commands::overview_out(&ctx),
        };
        if json {
            let mut j = out.json;
            if let Some(o) = j.as_object_mut() {
                o.insert("ok".into(), serde_json::json!(true));
            }
            println!("{j}");
        } else {
            match out.view {
                Some(v) => {
                    let body = v.render(&ctx.ui.style());
                    println!("{}{body}", if ctx.ui.printed() { "\n" } else { "" });
                }
                None => println!("{}", out.text),
            }
        }
        Ok(())
    })();
    let shown = ctx.ui.failed_shown();
    (r, shown)
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

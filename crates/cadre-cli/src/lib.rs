//! `cadre`: one command in text mode
//! (steps + an info box) or `--json`; with no command it prints the health screen and
//! `cadre` runs `status` (health of everything); on a terminal `cadre` alone and
//! `dashboard` open the Cadre app (src/entry/). All run the same `commands::exec`.
//! See docs/CLI.md. The two binaries
//! (src/main.rs, src/bin/cadre.rs) call [`main`]; the name they run as picks the default.

mod agent;
mod boards;
mod brand;
mod commands;
mod contract;
mod ctx;
mod decode;
mod endpoints;
pub mod entry;
mod fmt;
mod health;
mod html;
mod inspect;
mod market;
mod model;
mod probe;
mod snapshot;
mod solana;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod stub;
mod term;
mod txstatus;
mod trade_cmd;
mod trades;
mod wallet_cfg;

use clap::Parser;

#[derive(Parser)]
#[command(name = "cadre", version, disable_help_subcommand = true, about = "Cadre CLI: health of everything (status, ping), txs in words on both chains, agents, accounts, blocks, tickets and their replay verdict, plus the wallet commands and the dashboard.")]
struct Cli {
    #[command(flatten)]
    opts: ctx::Opts,
    #[command(subcommand)]
    cmd: Option<commands::Cmd>,
}

pub fn main() {
    use clap::{CommandFactory, FromArgMatches};
    let bin = "cadre";
    let m = Cli::command().name(bin).bin_name(bin).get_matches();
    let mut cli = Cli::from_arg_matches(&m).unwrap_or_else(|e| e.exit());
    // `cadre` alone on an interactive terminal: the entry screen (src/entry/); anywhere
    // else (a pipe, --json, TERM=dumb) exactly as before
    // `cadre dashboard`: the same app, opened on its Dashboard view
    let dashboard = matches!(cli.cmd, Some(commands::Cmd::Dashboard));
    if entry::should_open_here(bin, cli.cmd.is_some(), cli.opts.json) || dashboard {
        let start = if dashboard { entry::ViewId::Dashboard } else { entry::ViewId::Home };
        let launch = entry::Launch { opts: cli.opts, forward: std::env::args_os().skip(1).collect(), commands: palette(), start };
        let code = entry::run(launch).unwrap_or_else(|e| {
            eprintln!("{}", term::Style::detect().step_line(false, &format!("error: {e:#}")));
            1
        });
        std::process::exit(code);
    }
    // `cadre` with no command: status (the health screen)
    if cli.cmd.is_none() {
        cli.cmd = Some(commands::Cmd::Status { health: false });
    }
    let json = cli.opts.json;
    let style = term::Style::detect();
    let (r, shown) = run(cli, style);
    let code = match r {
        Ok(c) => c,
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

/// Every command for the entry screen's ctrl+p palette, from the clap definitions
/// (subcommand groups listed as "group sub").
pub fn palette() -> Vec<entry::PaletteCmd> {
    use clap::CommandFactory;
    let about = |c: &clap::Command| c.get_about().map(|a| a.to_string()).unwrap_or_default();
    let needs = |c: &clap::Command| c.get_arguments().any(|a| a.is_positional() && a.is_required_set()) || c.is_subcommand_required_set();
    let mut out = vec![];
    for c in Cli::command().get_subcommands() {
        if c.get_name() == "help" {
            continue;
        }
        if c.has_subcommands() {
            for s in c.get_subcommands().filter(|s| s.get_name() != "help") {
                out.push(entry::PaletteCmd { name: format!("{} {}", c.get_name(), s.get_name()), about: about(s), needs_args: needs(s) });
            }
        } else {
            out.push(entry::PaletteCmd { name: c.get_name().to_string(), about: about(c), needs_args: needs(c) });
        }
    }
    out
}

/// The result, and whether a step already printed its error.
fn run(cli: Cli, style: term::Style) -> (anyhow::Result<i32>, bool) {
    let json = cli.opts.json;
    let mut ctx = match ctx::Ctx::new(cli.opts) {
        Ok(c) => c,
        Err(e) => return (Err(e), false),
    };
    if !json {
        ctx.ui = term::Ui::plain(style);
    }
    let r = (|| -> anyhow::Result<i32> {
        let out = match &cli.cmd {
            Some(cmd) => commands::exec(&ctx, cmd, false)?,
            // no command: the overview box (JSON: what `status` returns)
            None => commands::overview_out(&ctx),
        };
        if json {
            let mut j = out.json;
            fmt::add_cx_fields(&mut j);
            if let Some(o) = j.as_object_mut() {
                // false exactly when the command exits non-zero (status / ping / verify)
                o.insert("ok".into(), serde_json::json!(out.code == 0));
            }
            println!("{j}");
        } else if out.raw {
            println!("{}", out.text);
        } else {
            match out.view {
                Some(v) => {
                    let body = v.render(&ctx.ui.style());
                    println!("{}{body}", if ctx.ui.printed() { "\n" } else { "" });
                }
                None => println!("{}", out.text),
            }
        }
        Ok(out.code)
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

    #[test]
    fn palette_lists_every_command() {
        let p = super::palette();
        let names: Vec<&str> = p.iter().map(|c| c.name.as_str()).collect();
        for want in ["status", "ping", "agents", "agent", "tx", "block", "watch", "verify", "account", "dashboard", "job show", "bridge out", "contract call", "quote buy", "positions", "buy", "sell"] {
            assert!(names.contains(&want), "{want} missing from {names:?}");
        }
        assert!(p.iter().find(|c| c.name == "tx").unwrap().needs_args);
        assert!(!p.iter().find(|c| c.name == "status").unwrap().needs_args);
        assert!(!names.contains(&"help"));
    }
}

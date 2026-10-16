//! hyprwinctl — `hyprctl` for hyprwin.
//!
//!   hyprwinctl [-j] clients | workspaces | activeworkspace | monitors | activewindow | binds | version
//!   hyprwinctl dispatch <dispatcher> [args]
//!   hyprwinctl reload | debug | configerrors
//!   hyprwinctl --batch "dispatch workspace 2 ; dispatch togglefloating"
//!   hyprwinctl events            (stream, like socat on Hyprland's socket2)

use std::io::{BufRead, BufReader, Write};

fn usage() -> ! {
    eprintln!(
        "usage: hyprwinctl [-j] <command> [args]\n\
         commands: clients workspaces activeworkspace monitors activewindow binds version\n\
         \x20         dispatch <dispatcher> [args]   reload   debug   configerrors   events\n\
         \x20         --batch \"cmd1 ; cmd2\""
    );
    std::process::exit(2)
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut json = false;
    args.retain(|a| {
        if a == "-j" || a == "--json" {
            json = true;
            false
        } else {
            true
        }
    });
    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        usage();
    }
    if args[0] == "events" {
        let f = match hyprwin_ipc::connect(&hyprwin_ipc::event_pipe_name(), false, 2000) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("hyprwinctl: cannot connect to hyprwin ({e}). Is hyprwin running?");
                std::process::exit(1)
            }
        };
        let out = std::io::stdout();
        for line in BufReader::new(f).lines() {
            let Ok(line) = line else { break };
            let mut o = out.lock();
            let _ = writeln!(o, "{line}");
            let _ = o.flush();
        }
        return;
    }
    let req = if args[0] == "--batch" {
        let cmds = args.get(1).cloned().unwrap_or_else(|| usage());
        let cmds: Vec<String> = cmds.split(';').map(|c| format!("{}{}", if json { "j/" } else { "" }, c.trim())).collect();
        format!("{}{}", hyprwin_ipc::BATCH_PREFIX, cmds.join(";"))
    } else {
        format!("{}{}", if json { "j/" } else { "" }, args.join(" "))
    };
    match hyprwin_ipc::request(&req) {
        Ok(reply) => {
            print!("{reply}");
            if !reply.ends_with('\n') {
                println!();
            }
            if reply.starts_with("unknown") || reply.starts_with("error") {
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("hyprwinctl: cannot connect to hyprwin ({e}). Is hyprwin running?");
            std::process::exit(1);
        }
    }
}

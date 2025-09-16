use clap::{Parser, Subcommand};
use mac_winops::ops::{RealWinOps, WinOps};

#[derive(Parser, Debug)]
#[command(name = "winops", about = "mac-winops tester", version)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// List on-screen windows (front to back)
    List,
    /// Print the current frontmost window
    Front,
    /// Raise by pid + CGWindowID
    RaiseId { pid: i32, id: u32 },
    /// Raise by title/app regex
    RaiseTitle {
        /// Title regex
        title: String,
        /// Optional app regex
        #[arg(long)]
        app: Option<String>,
    },
}

fn main() {
    let cli = Cli::parse();
    let ops = RealWinOps;
    match cli.cmd {
        Cmd::List => {
            for w in ops.list_windows() {
                println!(
                    "pid={} id={} app='{}' title='{}'",
                    w.pid, w.id, w.app, w.title
                );
            }
        }
        Cmd::Front => match ops.frontmost_window() {
            Some(w) => println!(
                "front: pid={} id={} app='{}' title='{}'",
                w.pid, w.id, w.app, w.title
            ),
            None => println!("front: none"),
        },
        Cmd::RaiseId { pid, id } => match mac_winops::raise_window(pid, id) {
            Ok(()) => println!("raised pid={} id={}", pid, id),
            Err(e) => {
                eprintln!("raise-id error: {}", e);
                std::process::exit(1);
            }
        },
        Cmd::RaiseTitle { title, app } => {
            let app_re = app.and_then(|s| regex::Regex::new(&s).ok());
            let title_re = regex::Regex::new(&title).unwrap();
            let all = ops.list_windows();
            let cur = ops.frontmost_window();
            let matches = |w: &mac_winops::WindowInfo| -> bool {
                let aok = app_re.as_ref().map(|r| r.is_match(&w.app)).unwrap_or(true);
                let tok = title_re.is_match(&w.title);
                aok && tok
            };
            let mut idx_match: Vec<usize> = Vec::new();
            for (i, w) in all.iter().enumerate() {
                if matches(w) {
                    idx_match.push(i);
                }
            }
            if idx_match.is_empty() {
                eprintln!("no matching windows");
                std::process::exit(3);
            }
            let target_idx = if let Some(c) = &cur {
                if matches(c) {
                    let cur_index = all.iter().position(|w| w.id == c.id && w.pid == c.pid);
                    if let Some(ci) = cur_index {
                        idx_match.into_iter().find(|&i| i > ci).unwrap_or(ci)
                    } else {
                        idx_match[0]
                    }
                } else {
                    idx_match[0]
                }
            } else {
                idx_match[0]
            };
            let target = &all[target_idx];
            println!(
                "target pid={} id={} app='{}' title='{}'",
                target.pid, target.id, target.app, target.title
            );
            match mac_winops::raise_window(target.pid, target.id) {
                Ok(()) => println!("raised"),
                Err(e) => {
                    eprintln!("raise error: {}", e);
                    std::process::exit(1);
                }
            }
        }
    }
}

use clap::{Parser, ValueEnum};

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Kind {
    Native,
    Nonnative,
}

#[derive(Parser, Debug)]
#[command(name = "fs", about = "Test fullscreen via mac-winops", version)]
struct Cli {
    /// Which fullscreen mode to use
    #[arg(value_enum)]
    kind: Kind,

    /// Desired state: toggle/on/off
    #[arg(long, default_value = "toggle")]
    state: String,

    /// Target process id; defaults to frontmost window's pid
    #[arg(long)]
    pid: Option<i32>,
}

fn main() {
    let cli = Cli::parse();

    let desired = match cli.state.as_str() {
        "on" => mac_winops::Desired::On,
        "off" => mac_winops::Desired::Off,
        _ => mac_winops::Desired::Toggle,
    };

    println!("[fs] accessibility_ok: {}", permissions::accessibility_ok());
    if !permissions::accessibility_ok() {
        eprintln!("error: Accessibility permission missing");
        std::process::exit(1);
    }

    let pid = match cli.pid {
        Some(p) => p,
        None => match mac_winops::frontmost_window() {
            Some(w) => w.pid,
            None => {
                eprintln!("no frontmost window detected");
                std::process::exit(2);
            }
        },
    };
    println!("[fs] pid: {}", pid);

    if let Some(w) = mac_winops::frontmost_window_for_pid(pid) {
        println!("[fs] frontmost window: id={} title='{}'", w.id, w.title);
        if let Some(((x, y), (w, h))) = mac_winops::ax_window_frame(pid, &w.title) {
            println!("[fs] initial frame: ({:.1},{:.1}) {:.1}x{:.1}", x, y, w, h);
        } else {
            println!("[fs] ax_window_frame: None");
        }
    } else {
        println!("[fs] no frontmost window for pid");
    }

    let res = match cli.kind {
        Kind::Native => {
            println!("[fs] calling fullscreen_native ...");
            let r = mac_winops::fullscreen_native(pid, desired);
            println!(
                "[fs] fullscreen_native returned: {:?}",
                r.as_ref().map(|_| &())
            );
            r
        }
        Kind::Nonnative => {
            println!("[fs] calling fullscreen_nonnative ...");
            let r = mac_winops::fullscreen_nonnative(pid, desired);
            println!(
                "[fs] fullscreen_nonnative returned: {:?}",
                r.as_ref().map(|_| &())
            );
            r
        }
    };
    match res {
        Ok(()) => println!("ok"),
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
    }
}

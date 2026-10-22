//! sadasight — SadaCraft screen spotlight overlay (CLI).
//!
//! One binary doubles as the daemon and the controller: the first instance
//! owns a named mutex and runs the overlay; later invocations forward
//! `--effect` changes to it via a posted window message.

use std::path::PathBuf;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::*;

const MUTEX_NAME: PCWSTR = w!("Local\\SadaCraft.SadaSight.SingleInstance");

enum Op {
    Toggle,
    On,
    Off,
}

fn usage() -> ! {
    eprintln!(
        "sadasight — SadaCraft screen spotlight overlay

  sadasight                        start overlay with saved settings
  sadasight --effect <csv>         toggle effects: ring,ripple,trail,spotlight,pen,keys,all
                                   prefix +name to force on, -name to force off
  sadasight --config <file>        use an alternate config JSON
  sadasight --help

  With an instance already running, --effect forwards the change to it.

  Hotkeys: Ctrl+Alt+S show/hide · Ctrl+Alt+P pen mode · Ctrl+Alt+K keys ·
           Ctrl+Alt+C clear ink · Ctrl+Alt+Q quit
  Pen:     drag to draw · keys 1-4 pick color · Esc exits pen mode
  Keys:    shows recent keystrokes as chips above the taskbar"
    );
    std::process::exit(2);
}

/// Parse `+name|-name|name` items into toggle/on/off bit masks.
fn parse_effects(csv: &str, toggle: &mut u8, on: &mut u8, off: &mut u8) {
    for item in csv.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let (op, name) = match item.as_bytes()[0] {
            b'+' => (Op::On, &item[1..]),
            b'-' => (Op::Off, &item[1..]),
            _ => (Op::Toggle, item),
        };
        let bits = if name == "all" {
            0x3F
        } else {
            match sadasight::config::effect_index(name) {
                Some(i) => 1u8 << i,
                None => {
                    eprintln!("unknown effect: {name}");
                    usage();
                }
            }
        };
        match op {
            Op::Toggle => *toggle |= bits,
            Op::On => *on |= bits,
            Op::Off => *off |= bits,
        }
    }
}

fn describe(mask: u8) -> String {
    let names = sadasight::config::EFFECT_NAMES;
    let on: Vec<&str> = (0..6)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| names[i])
        .collect();
    if on.is_empty() {
        "none".into()
    } else {
        on.join(",")
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut cfg_path: Option<PathBuf> = None;
    let (mut toggle, mut on, mut off) = (0u8, 0u8, 0u8);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--effect" | "-e" => {
                let v = args.get(i + 1).unwrap_or_else(|| usage());
                parse_effects(v, &mut toggle, &mut on, &mut off);
                i += 2;
            }
            "--config" | "-c" => {
                let v = args.get(i + 1).unwrap_or_else(|| usage());
                cfg_path = Some(PathBuf::from(v));
                i += 2;
            }
            "--help" | "-h" => usage(),
            other => {
                eprintln!("unknown argument: {other}");
                usage();
            }
        }
    }

    unsafe {
        // Held for the process lifetime; doubles as the running-instance flag.
        let _mutex = CreateMutexW(None, true, MUTEX_NAME)?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if toggle | on | off != 0 {
                match FindWindowW(sadasight::overlay::CLASS_NAME, PCWSTR::null()) {
                    Ok(hwnd) => {
                        PostMessageW(
                            Some(hwnd),
                            sadasight::overlay::WM_APP_EFFECTS,
                            WPARAM(toggle as usize),
                            LPARAM(on as isize | ((off as isize) << 8)),
                        )?;
                        println!("[sadasight] forwarded to running instance");
                    }
                    Err(_) => eprintln!(
                        "[sadasight] instance held the mutex but no window was found (exiting?)"
                    ),
                }
            } else {
                eprintln!("[sadasight] already running — use --effect to toggle effects");
            }
            return Ok(());
        }
    }

    let path = cfg_path.unwrap_or_else(sadasight::config::default_path);
    let mut cfg = sadasight::config::load(&path);
    cfg.apply_masks(toggle, on, off);
    let _ = cfg.save(&path);

    eprintln!(
        "[sadasight] overlay starting — effects: {} (config: {})",
        describe(cfg.mask()),
        path.display()
    );
    eprintln!("[sadasight] Ctrl+Alt+S hide/show · Ctrl+Alt+P pen · Ctrl+Alt+C clear · Ctrl+Alt+Q quit");
    sadasight::overlay::run(cfg, path)
}

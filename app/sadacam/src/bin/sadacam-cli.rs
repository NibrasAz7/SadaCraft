use std::os::windows::ffi::OsStrExt;
use windows::core::*;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

fn usage() -> ! {
    eprintln!(
        "sadacam-cli <command>

  install    Register the COM media source system-wide (HKLM, needs admin)
  install-user  Register the COM media source per-user (HKCU)
  uninstall  Remove COM registration (user hive; add --system for HKLM)
  start      Create + start the SadaCraft virtual camera (stays until Ctrl+C)
  cameras    List video capture devices (Media Foundation)
  feed       Push test frames into the shared-memory feed (until Ctrl+C)
             feed --bmp <file.bmp>      push a static 24-bit BMP instead
             feed --webcam [name-part]  capture a real webcam (first non-
                                        SadaCraft device, or name match)
             --name-card <text>   draw a name-card band over any source"
    );
    std::process::exit(2);
}

fn main() -> Result<()> {
    let cmd = std::env::args().nth(1).unwrap_or_default();
    if cmd.is_empty() {
        usage();
    }

    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
    }

    let result = match cmd.as_str() {
        "install" | "install-user" => {
            // Register sadacam.dll (sibling of this exe), not the exe itself.
            let dll = std::env::current_exe()?
                .parent()
                .unwrap()
                .join("sadacam.dll");
            let path: Vec<u16> = if cmd == "install" {
                // Frame Server (LocalService) cannot read user-profile paths;
                // stage the DLL under ProgramData which services can access.
                let dir = std::path::PathBuf::from(r"C:\ProgramData\SadaCraft");
                std::fs::create_dir_all(&dir)?;
                let staged = dir.join("sadacam.dll");
                std::fs::copy(&dll, &staged)?;
                staged.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
            } else {
                dll.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
            };
            if cmd == "install" {
                match sadacam::register::register_server_system_at(path) {
                    Ok(()) => println!("COM media source registered under HKLM."),
                    Err(e) => {
                        eprintln!("HKLM registration failed (needs admin): {e}");
                        eprintln!("Re-run elevated, e.g.: run this exe as administrator.");
                        return Err(e);
                    }
                }
            } else {
                sadacam::register::register_server_at(path)?;
                println!("COM media source registered under HKCU.");
            }
            Ok(())
        }
        "uninstall" => {
            let _ = sadacam::register::unregister_server();
            let _ = sadacam::register::unregister_server_system();
            println!("COM registration removed.");
            Ok(())
        }
        "start" => {
            eprintln!("[sadacam] MFCreateVirtualCamera...");
            let vcam = sadacam::register::create_camera("SadaCraft Camera")?;
            eprintln!("[sadacam] created, calling Start...");
            sadacam::register::start_camera(&vcam)?;
            eprintln!("[sadacam] started.");
            println!(
                "Virtual camera 'SadaCraft Camera' active. \
                 Open Windows Camera / Meet and select it. Press Ctrl+C to stop."
            );
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        }
        "cameras" => {
            match sadacam::webcam::list_devices() {
                Ok(names) => {
                    for (i, n) in names.iter().enumerate() {
                        println!("{i}: {n}");
                    }
                }
                Err(e) => eprintln!("enumeration failed: {e}"),
            }
            Ok(())
        }
        "feed" => {
            // feed [--bmp <file.bmp> | --webcam [name-part]] [--name-card <text>]
            let args: Vec<String> = std::env::args().skip(2).collect();
            let mut bmp_path: Option<String> = None;
            let mut webcam: Option<Option<String>> = None;
            let mut name_card: Option<String> = None;
            let mut i = 0;
            while i < args.len() {
                match args[i].as_str() {
                    "--bmp" => {
                        bmp_path = Some(args.get(i + 1).cloned().ok_or_else(|| {
                            Error::from_hresult(windows::Win32::Foundation::E_INVALIDARG)
                        })?);
                        i += 2;
                    }
                    "--webcam" => {
                        let filter = match args.get(i + 1) {
                            Some(s) if !s.starts_with("--") => Some(s.clone()),
                            _ => None,
                        };
                        i += if filter.is_some() { 2 } else { 1 };
                        webcam = Some(filter);
                    }
                    "--name-card" => {
                        name_card = Some(args.get(i + 1).cloned().ok_or_else(|| {
                            Error::from_hresult(windows::Win32::Foundation::E_INVALIDARG)
                        })?);
                        i += 2;
                    }
                    other => {
                        eprintln!("unknown feed option: {other}");
                        usage();
                    }
                }
            }

            // The camera's Frame Server host creates the mapping — wait
            // for it (demand gate: the feed attaches to a live camera).
            let mut writer = loop {
                match sadacam::shm::FrameWriter::open() {
                    Ok(w) => break w,
                    Err(_) => {
                        eprintln!("[sadacam] waiting for camera feed slot...");
                        std::thread::sleep(std::time::Duration::from_secs(1));
                    }
                }
            };
            let (w, h) = (writer.width, writer.height);
            eprintln!(
                "[sadacam] feeding {}x{} BGRX into {} (Ctrl+C to stop)\n[sadacam] slot instance={:#x} status={} read_seq={}",
                w,
                h,
                sadacam::shm::mapping_name(),
                writer.instance(),
                writer.status(),
                writer.last_read_seq()
            );

            if let Some(filter) = webcam {
                let mut cam = sadacam::webcam::Webcam::open(filter.as_deref(), w, h)?;
                eprintln!(
                    "[sadacam] webcam '{}' {}x{} {:?} -> feed {}x{}",
                    cam.name, cam.width, cam.height, cam.format, w, h
                );
                let (sw, sh) = (cam.width as usize, cam.height as usize);
                let mut frame = vec![0u8; (w * h * 4) as usize];
                let mut errors = 0u32;
                let mut reconnects = writer.reconnect_count();
                loop {
                    match cam.read_frame() {
                        Ok(raw) => {
                            errors = 0;
                            if sw == w as usize && sh == h as usize {
                                frame.copy_from_slice(&raw);
                            } else {
                                sadacam::frames::resize_bgrx_nearest(
                                    &raw, sw, sh, &mut frame, w as usize, h as usize,
                                );
                            }
                            if let Some(text) = &name_card {
                                sadacam::overlay::draw_name_card(
                                    &mut frame, w as usize, h as usize, text,
                                );
                            }
                            // Consumers read the buffer bottom-up (RGB32 DIB).
                            sadacam::frames::flip_vertical(&mut frame, w as usize, h as usize);
                            writer.write_frame(&frame)?;
                            if writer.reconnect_count() != reconnects {
                                reconnects = writer.reconnect_count();
                                eprintln!(
                                    "[sadacam] feed slot recycled — re-attached (inst={:#x})",
                                    writer.instance()
                                );
                            }
                        }
                        Err(e) => {
                            errors += 1;
                            if errors <= 3 || errors % 60 == 0 {
                                eprintln!("[sadacam] webcam read error: {e}");
                            }
                            std::thread::sleep(std::time::Duration::from_millis(500));
                        }
                    }
                }
            }

            let static_frame = if let Some(path) = bmp_path {
                let bytes = std::fs::read(&path)?;
                let (bw, bh, px) = sadacam::shm::decode_bmp24(&bytes)?;
                if bw != w || bh != h {
                    eprintln!("[sadacam] BMP is {}x{}, camera feeds {}x{} — resize it", bw, bh, w, h);
                    std::process::exit(2);
                }
                Some(px)
            } else {
                None
            };
            let mut frame = vec![0u8; (w * h * 4) as usize];
            let mut idx = 0u32;
            let mut last_seen = 0u32;
            let mut reconnects = writer.reconnect_count();
            loop {
                match &static_frame {
                    Some(px) => frame.copy_from_slice(px),
                    None => {
                        sadacam::fill_test_frame(&mut frame, w as usize, h as usize, idx);
                    }
                }
                if let Some(text) = &name_card {
                    sadacam::overlay::draw_name_card(&mut frame, w as usize, h as usize, text);
                }
                // Consumers read the buffer bottom-up (RGB32 DIB).
                sadacam::frames::flip_vertical(&mut frame, w as usize, h as usize);
                writer.write_frame(&frame)?;
                if writer.reconnect_count() != reconnects {
                    reconnects = writer.reconnect_count();
                    eprintln!(
                        "[sadacam] feed slot recycled — re-attached (inst={:#x})",
                        writer.instance()
                    );
                }
                let consumed = writer.last_read_seq();
                if consumed != 0 && last_seen == 0 {
                    eprintln!("[sadacam] consumer reading frames (seq {})", consumed);
                }
                if idx % 90 == 0 && idx > 0 {
                    eprintln!(
                        "[sadacam] wrote seq={} | consumed={} status={} inst={:#x}",
                        writer.current_seq(),
                        consumed,
                        writer.status(),
                        writer.instance()
                    );
                }
                last_seen = consumed;
                idx = idx.wrapping_add(1);
                std::thread::sleep(std::time::Duration::from_millis(33));
            }
        }
        _ => usage(),
    };

    unsafe { CoUninitialize() };
    result
}

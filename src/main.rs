#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use karaoke_kit::{app::KaraokeKitApp, cli, icon};
use std::sync::Arc;

fn main() {
    // `karaoke-kit song.json` opens the editor with a project; flags fall through
    // to the headless CLI.
    let startup_path = std::env::args()
        .nth(1)
        .filter(|arg| !arg.starts_with('-'))
        .map(std::path::PathBuf::from);
    if startup_path.is_none() {
        // Release Windows builds use the GUI subsystem; reattach to the
        // launching console so headless CLI output and errors are visible.
        #[cfg(all(windows, not(debug_assertions)))]
        if std::env::args().nth(1).is_some() {
            attach_parent_console();
        }
        if let Some(code) = cli::run() {
            std::process::exit(code);
        }
    }

    install_panic_hook();

    if let Err(error) = run_gui(startup_path) {
        let message = format!(
            "KaraokeKit could not start its window.\n\n{error:?}\n\n\
             This usually means no OpenGL driver is available. Try updating your GPU \
             drivers, then launch KaraokeKit again."
        );
        eprintln!("{message}");
        show_error_dialog("KaraokeKit — startup error", &message);
        std::process::exit(1);
    }
}

/// Release Windows builds use the GUI subsystem, so a console launched with a
/// CLI flag has no attached stdout/stderr. Reattach to the parent console and
/// point the standard handles at it so `--render`, `--help`, and errors work
/// from cmd/PowerShell.
#[cfg(all(windows, not(debug_assertions)))]
fn attach_parent_console() {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;

    const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
    const STD_INPUT_HANDLE: u32 = -10i32 as u32;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const OPEN_EXISTING: u32 = 3;
    const INVALID_HANDLE_VALUE: isize = -1;

    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
        fn CreateFileW(
            file_name: *const u16,
            desired_access: u32,
            share_mode: u32,
            security_attributes: *mut c_void,
            creation_disposition: u32,
            flags_and_attributes: u32,
            template_file: *mut c_void,
        ) -> *mut c_void;
        fn SetStdHandle(std_handle: u32, handle: *mut c_void) -> i32;
    }

    unsafe fn open_console(name: &str, access: u32) -> Option<*mut c_void> {
        let wide: Vec<u16> = std::ffi::OsStr::new(name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `wide` is NUL-terminated and lives for the call.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if handle as isize == INVALID_HANDLE_VALUE {
            None
        } else {
            Some(handle)
        }
    }

    // SAFETY: plain Win32 console calls; handles come from CreateFileW and are
    // only installed as the process standard handles.
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return;
        }
        if let Some(handle) = open_console("CONOUT$", GENERIC_WRITE) {
            let _ = SetStdHandle(STD_OUTPUT_HANDLE, handle);
            let _ = SetStdHandle(STD_ERROR_HANDLE, handle);
        }
        if let Some(handle) = open_console("CONIN$", GENERIC_READ | GENERIC_WRITE) {
            let _ = SetStdHandle(STD_INPUT_HANDLE, handle);
        }
    }
}

fn run_gui(startup_path: Option<std::path::PathBuf>) -> eframe::Result {
    let icon = icon::load_icon();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("KaraokeKit")
            .with_inner_size([1460.0, 920.0])
            .with_min_inner_size([1120.0, 700.0])
            .with_icon(Arc::new(icon)),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "KaraokeKit",
        options,
        Box::new(move |cc| {
            let mut app = KaraokeKitApp::new(cc);
            if let Some(path) = startup_path {
                app.open_path(path);
            }
            Ok(Box::new(app))
        }),
    )
}

fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = format!(
            "KaraokeKit hit an unexpected error.\n\n{info}\n\n\
             Your project autosave is not enabled yet, so re-open the last saved JSON if needed."
        );
        eprintln!("{message}");
        let _ = std::panic::catch_unwind(|| {
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("KaraokeKit crashed")
                .set_description(&message)
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
        });
        default_hook(info);
    }));
}

fn show_error_dialog(title: &str, message: &str) {
    let _ = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title(title)
        .set_description(message)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

//! Windowless tray launcher. On Windows this is a GUI-subsystem binary, so
//! starting it from the Start menu or at login doesn't flash a console
//! window; logs go to tray.log. Elsewhere it's equivalent to `vox tray`.

#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    let model = std::env::args()
        .skip_while(|a| a != "--model")
        .nth(1)
        .map(std::path::PathBuf::from);
    if let Err(e) = vox::tray::run(model) {
        tracing::error!("vox tray exited: {e}");
        // No console to print to on Windows; make sure it lands in the log.
        if let Ok(mut f) = vox::secure_fs::open_private_append(&vox::config::log_path()) {
            use std::io::Write;
            let _ = writeln!(f, "vox tray exited: {e}");
        }
        std::process::exit(1);
    }
}

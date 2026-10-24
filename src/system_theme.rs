//! Follows the GNOME light/dark setting. winit 0.30 does not report the system theme on Linux,
//! so this reads `org.gnome.desktop.interface color-scheme` with `gsettings` and watches it for
//! changes. Without `gsettings` (or the schema) the theme stays at egui's default, dark.

use std::{
    io::{self, BufRead, BufReader},
    os::unix::process::CommandExt,
    process::{Command, Stdio},
    thread,
};

use egui::Theme;
use tracing::debug;

const SCHEMA: &str = "org.gnome.desktop.interface";
const KEY: &str = "color-scheme";

/// Applies the current setting to `ctx` now, then from a `gsettings monitor` child process on
/// every change. The child dies with the calling thread (call it from the main thread), so it
/// never outlives the window, whatever ends the process.
pub fn watch(ctx: &egui::Context) {
    let Some(current) = Command::new("gsettings")
        .args(["get", SCHEMA, KEY])
        .output()
        .ok()
        .filter(|out| out.status.success())
    else {
        return;
    };
    ctx.set_theme(theme_of(&String::from_utf8_lossy(&current.stdout)));
    let Ok(mut child) = die_with_parent(Command::new("gsettings").args(["monitor", SCHEMA, KEY]))
        .stdout(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
    else {
        return;
    };
    let Some(stdout) = child.stdout.take() else {
        return;
    };
    let ctx = ctx.clone();
    let _ = thread::Builder::new().name("theme".into()).spawn(move || {
        // Each line is `color-scheme: 'prefer-dark'`.
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            debug!(line, "theme changed");
            ctx.set_theme(theme_of(&line));
            ctx.request_repaint();
        }
    });
}

/// The child gets SIGKILL when the thread that spawns it exits. Spawned from the main thread, that
/// is when the process exits, however it exits (crash, `kill`, `panic = "abort"`).
fn die_with_parent(cmd: &mut Command) -> &mut Command {
    let parent = libc::pid_t::try_from(std::process::id()).unwrap_or(0);
    // SAFETY: the forked child only makes async-signal-safe calls (`prctl`, `getppid`, reading
    // errno) and does not allocate.
    unsafe {
        cmd.pre_exec(move || {
            if libc::prctl(
                libc::PR_SET_PDEATHSIG,
                libc::c_ulong::from(libc::SIGKILL.unsigned_abs()),
            ) == -1
            {
                return Err(io::Error::last_os_error());
            }
            // The parent may have exited before the prctl.
            if libc::getppid() != parent {
                return Err(io::Error::from_raw_os_error(libc::ESRCH));
            }
            Ok(())
        })
    }
}

/// `'prefer-dark'` is dark; `'default'`, `'prefer-light'` and anything else are light.
fn theme_of(setting: &str) -> Theme {
    if setting.contains("prefer-dark") {
        Theme::Dark
    } else {
        Theme::Light
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_dies_with_spawning_thread() -> Result<(), String> {
        use std::os::unix::process::ExitStatusExt;
        let mut child = thread::spawn(|| die_with_parent(Command::new("sleep").arg("30")).spawn())
            .join()
            .map_err(|_| "spawn thread panicked")?
            .map_err(|e| e.to_string())?;
        for _ in 0..100 {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                assert_eq!(status.signal(), Some(libc::SIGKILL));
                return Ok(());
            }
            thread::sleep(std::time::Duration::from_millis(10));
        }
        let _ = child.kill();
        Err("child outlived its spawning thread".into())
    }

    #[test]
    fn theme_of_values() {
        assert_eq!(theme_of("'prefer-dark'\n"), Theme::Dark);
        assert_eq!(theme_of("color-scheme: 'prefer-dark'"), Theme::Dark);
        assert_eq!(theme_of("'default'"), Theme::Light);
        assert_eq!(theme_of("color-scheme: 'prefer-light'"), Theme::Light);
    }
}

//! Where to look for `libpdfium.so`.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

pub const LIB_NAME: &str = "libpdfium.so";

/// Ordered libpdfium files to try before the system library: `env_override` (a file path; ignored
/// if empty), `exe_dir/libpdfium.so`, `home/.local/lib/paperlight/libpdfium.so`.
fn candidate_paths(
    env_override: Option<&OsStr>,
    exe_dir: Option<&Path>,
    home: Option<&Path>,
) -> Vec<PathBuf> {
    env_override
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .into_iter()
        .chain(exe_dir.map(|d| d.join(LIB_NAME)))
        .chain(home.map(|h| h.join(".local/lib/paperlight").join(LIB_NAME)))
        .collect()
}

/// [`candidate_paths`] fed from `$PAPERLIGHT_PDFIUM`, the executable's directory and `$HOME`.
pub fn default_candidates() -> Vec<PathBuf> {
    let env_override = std::env::var_os("PAPERLIGHT_PDFIUM");
    let exe = std::env::current_exe().ok();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    candidate_paths(
        env_override.as_deref(),
        exe.as_deref().and_then(Path::parent),
        home.as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_is_env_exe_home() {
        let got = candidate_paths(
            Some(OsStr::new("/x/p.so")),
            Some(Path::new("/opt/pl")),
            Some(Path::new("/home/u")),
        );
        let want: Vec<PathBuf> = [
            "/x/p.so",
            "/opt/pl/libpdfium.so",
            "/home/u/.local/lib/paperlight/libpdfium.so",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn empty_env_ignored() {
        let got = candidate_paths(Some(OsStr::new("")), None, Some(Path::new("/home/u")));
        assert_eq!(
            got,
            [PathBuf::from("/home/u/.local/lib/paperlight/libpdfium.so")]
        );
    }

    #[test]
    fn missing_inputs_skipped() {
        assert_eq!(candidate_paths(None, None, None), Vec::<PathBuf>::new());
    }
}

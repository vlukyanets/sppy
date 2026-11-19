//! `sppy://` links: taking one from the command line, and making the system open such links
//! with this app (only when the user asks for it in Settings).

use std::path::Path;

/// Longer arguments are not invite links.
const MAX_LINK_LEN: usize = 2048;
const SCHEMES: [&str; 2] = ["sppy", "sppys"];

/// The invite link among the program's arguments, if there is one.
pub fn link_from_args(args: impl IntoIterator<Item = String>) -> Option<String> {
    args.into_iter().map(|a| a.trim().to_owned()).find(|a| is_invite_link(a))
}

/// `sppy://host[:port]/<token>` (or `sppys://`), and nothing else.
pub fn is_invite_link(text: &str) -> bool {
    text.len() <= MAX_LINK_LEN
        && SCHEMES.iter().any(|s| text.starts_with(&format!("{s}://")))
        && sppy_common::link::parse_address(text).is_ok_and(|a| a.token.is_some())
}

// What each system needs is built on all of them, so that the tests run everywhere.

/// The name of the desktop entry that opens the links (Linux).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub const DESKTOP_FILE: &str = "sppy-link.desktop";

/// A desktop entry that runs `exe` with the clicked link. `SPPY_HOME`, when set, is kept,
/// so that a separate instance (e.g. a test one) opens its own links.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn desktop_entry(exe: &Path, sppy_home: Option<&Path>) -> String {
    let mut exec = Vec::new();
    if let Some(home) = sppy_home {
        exec.push(quote_exec_arg("env"));
        exec.push(quote_exec_arg(&format!("SPPY_HOME={}", home.display())));
    }
    exec.push(quote_exec_arg(&exe.display().to_string()));
    exec.push("%u".to_owned());
    let mime: String = SCHEMES.iter().map(|s| format!("x-scheme-handler/{s};")).collect();
    format!(
        "[Desktop Entry]\nType=Application\nName=sppy\nComment=Open sppy invite links\n\
         Exec={}\nMimeType={mime}\nNoDisplay=true\nTerminal=false\n",
        exec.join(" ")
    )
}

/// One argument of a desktop entry's `Exec` key, quoted as the specification asks.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn quote_exec_arg(arg: &str) -> String {
    let mut out = String::from("\"");
    for c in arg.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            // `%` is a field code in Exec; a literal one is doubled.
            '%' => out.push_str("%%"),
            c => out.push(c),
        }
    }
    out.push('"');
    // The desktop file format itself escapes backslashes once more.
    out.replace('\\', "\\\\")
}

/// `reg.exe` arguments that make Windows open both schemes with `exe` (per user, no admin).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn reg_add_commands(exe: &Path) -> Vec<Vec<String>> {
    let open = format!("\"{}\" \"%1\"", exe.display());
    SCHEMES
        .iter()
        .flat_map(|s| {
            let key = format!(r"HKCU\Software\Classes\{s}");
            [
                vec!["add".into(), key.clone(), "/ve".into(), "/d".into(), format!("URL:{s} link")],
                vec![
                    "add".into(),
                    key.clone(),
                    "/v".into(),
                    "URL Protocol".into(),
                    "/d".into(),
                    String::new(),
                ],
                vec![
                    "add".into(),
                    format!(r"{key}\shell\open\command"),
                    "/ve".into(),
                    "/d".into(),
                    open.clone(),
                ],
            ]
        })
        .map(|mut c| {
            c.push("/f".into());
            c
        })
        .collect()
}

/// `reg.exe` arguments that remove what [`reg_add_commands`] added.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn reg_delete_commands() -> Vec<Vec<String>> {
    SCHEMES
        .iter()
        .map(|s| vec!["delete".into(), format!(r"HKCU\Software\Classes\{s}"), "/f".into()])
        .collect()
}

/// Whether this app opens the links; `None` where that cannot be told or changed here.
pub fn is_registered() -> Option<bool> {
    os::is_registered()
}

/// Make the system open `sppy://` and `sppys://` links with this executable.
pub fn register() -> Result<(), String> {
    os::register()
}

/// Undo [`register`].
pub fn unregister() -> Result<(), String> {
    os::unregister()
}

#[cfg(target_os = "linux")]
mod os {
    use std::path::PathBuf;
    use std::process::Command;

    use super::{DESKTOP_FILE, SCHEMES, desktop_entry};

    fn applications_dir() -> Option<PathBuf> {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
        Some(data.join("applications"))
    }

    fn xdg_mime(args: &[&str]) -> Result<String, String> {
        let out = Command::new("xdg-mime")
            .args(args)
            .output()
            .map_err(|e| format!("cannot run xdg-mime: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "xdg-mime failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    pub fn is_registered() -> Option<bool> {
        let handler = xdg_mime(&["query", "default", "x-scheme-handler/sppy"]).ok()?;
        Some(handler == DESKTOP_FILE && applications_dir()?.join(DESKTOP_FILE).is_file())
    }

    pub fn register() -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
        let home = std::env::var_os("SPPY_HOME").map(PathBuf::from);
        let dir = applications_dir().ok_or("cannot find ~/.local/share/applications")?;
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let file = dir.join(DESKTOP_FILE);
        std::fs::write(&file, desktop_entry(&exe, home.as_deref()))
            .map_err(|e| format!("cannot write {}: {e}", file.display()))?;
        for s in SCHEMES {
            xdg_mime(&["default", DESKTOP_FILE, &format!("x-scheme-handler/{s}")])?;
        }
        Ok(())
    }

    pub fn unregister() -> Result<(), String> {
        let dir = applications_dir().ok_or("cannot find ~/.local/share/applications")?;
        match std::fs::remove_file(dir.join(DESKTOP_FILE)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("cannot remove {DESKTOP_FILE}: {e}")),
        }
    }
}

#[cfg(windows)]
mod os {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    use super::{reg_add_commands, reg_delete_commands};

    /// No console window flashing up for `reg.exe`.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn reg(args: &[String]) -> Result<String, String> {
        let out = Command::new("reg")
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| format!("cannot run reg: {e}"))?;
        if !out.status.success() {
            return Err(format!("reg failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub fn is_registered() -> Option<bool> {
        let exe = std::env::current_exe().ok()?;
        let key = r"HKCU\Software\Classes\sppy\shell\open\command".to_owned();
        let found = reg(&["query".into(), key, "/ve".into()]).ok();
        Some(found.is_some_and(|out| out.contains(&exe.display().to_string())))
    }

    pub fn register() -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
        for c in reg_add_commands(&exe) {
            reg(&c)?;
        }
        Ok(())
    }

    pub fn unregister() -> Result<(), String> {
        for c in reg_delete_commands() {
            // A key that is not there is fine.
            let _ = reg(&c);
        }
        Ok(())
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod os {
    pub fn is_registered() -> Option<bool> {
        None
    }
    pub fn register() -> Result<(), String> {
        Err("not supported on this system".into())
    }
    pub fn unregister() -> Result<(), String> {
        Err("not supported on this system".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_link_is_taken_from_the_arguments() {
        let link = "sppy://box.ts.net:8740/abc";
        assert_eq!(link_from_args(args(&[link])).as_deref(), Some(link));
        assert_eq!(
            link_from_args(args(&["--flag", " sppys://h/t "])).as_deref(),
            Some("sppys://h/t")
        );
        for not_a_link in
            ["", "sppy://h:8740", "sppy://h/t/x", "http://h/t", "h:8740/t", "--sppy://h/t"]
        {
            assert_eq!(link_from_args(args(&[not_a_link])), None, "{not_a_link:?}");
        }
        let long = format!("sppy://h/{}", "a".repeat(MAX_LINK_LEN));
        assert_eq!(link_from_args(args(&[&long])), None);
    }

    #[test]
    fn desktop_entry_runs_this_program_with_the_link() {
        let e = desktop_entry(Path::new("/opt/sppy/sppy"), None);
        assert!(e.contains("\nExec=\"/opt/sppy/sppy\" %u\n"), "{e}");
        assert!(e.contains("MimeType=x-scheme-handler/sppy;x-scheme-handler/sppys;"));
        assert!(e.starts_with("[Desktop Entry]\n"));
        let e = desktop_entry(Path::new("/x/sppy"), Some(Path::new("/tmp/test home")));
        assert!(e.contains("Exec=\"env\" \"SPPY_HOME=/tmp/test home\" \"/x/sppy\" %u"), "{e}");
    }

    #[test]
    fn exec_arguments_are_escaped() {
        // `"`, `$`, backquote and backslash are escaped, then backslashes once more for the file.
        assert_eq!(quote_exec_arg(r#"a"b$c`d\e"#), r#""a\\"b\\$c\\`d\\\\e""#);
        assert_eq!(quote_exec_arg("50%"), "\"50%%\"");
    }

    #[test]
    fn windows_registry_entries_open_both_schemes() {
        let cmds = reg_add_commands(Path::new(r"C:\Program Files\sppy\sppy.exe"));
        assert_eq!(cmds.len(), 6);
        let open = &cmds[2];
        assert_eq!(open[1], r"HKCU\Software\Classes\sppy\shell\open\command");
        assert_eq!(open[4], r#""C:\Program Files\sppy\sppy.exe" "%1""#);
        assert!(cmds.iter().all(|c| c.last().map(String::as_str) == Some("/f")));
        assert!(cmds.iter().any(|c| c[1] == r"HKCU\Software\Classes\sppys"));
        assert_eq!(reg_delete_commands()[1][1], r"HKCU\Software\Classes\sppys");
    }
}

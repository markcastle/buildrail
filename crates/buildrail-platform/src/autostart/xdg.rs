//! XDG autostart (`~/.config/autostart/*.desktop`) for Linux/BSD desktops.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::{read_file, remove_file, write_file, Autostart, AutostartError, LaunchCommand};

/// The XDG autostart directory: `$XDG_CONFIG_HOME/autostart` when that is
/// an absolute path, else `$HOME/.config/autostart`.
pub fn xdg_autostart_dir(
    env: impl Fn(&str) -> Option<OsString>,
) -> Result<PathBuf, AutostartError> {
    if let Some(config) = env("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        return Ok(config.join("autostart"));
    }
    env("HOME")
        .filter(|h| !h.is_empty())
        .map(|home| PathBuf::from(home).join(".config").join("autostart"))
        .ok_or_else(|| AutostartError::NoLocation("neither XDG_CONFIG_HOME nor HOME is set".into()))
}

/// Characters that force quoting in an `Exec` argument (Desktop Entry
/// Specification, "The Exec key").
const RESERVED: &[char] = &[
    ' ', '\t', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(', ')', '`',
];

/// Quotes one `Exec` argument per the Desktop Entry Specification,
/// including the string-level backslash escaping and `%` doubling, so it
/// can be pasted into the file verbatim. Fails on line breaks, which the
/// format cannot represent.
pub fn quote_exec_arg(arg: &str) -> Result<String, AutostartError> {
    if arg.contains(['\n', '\r']) {
        return Err(AutostartError::InvalidCommand(
            "line break in the command".into(),
        ));
    }
    let quoted = if arg.is_empty() || arg.contains(RESERVED) {
        let mut out = String::from("\"");
        for c in arg.chars() {
            if matches!(c, '"' | '`' | '$' | '\\') {
                out.push('\\');
            }
            out.push(c);
        }
        out.push('"');
        out
    } else {
        arg.to_owned()
    };
    // String-value escaping is applied after quoting: each backslash is
    // written twice. Then field codes: a literal % is %%.
    Ok(quoted.replace('\\', "\\\\").replace('%', "%%"))
}

/// The `.desktop` file contents for `command`.
pub fn desktop_entry(name: &str, command: &LaunchCommand) -> Result<String, AutostartError> {
    let exe = command.executable.to_str().ok_or_else(|| {
        AutostartError::InvalidCommand("the executable path is not valid UTF-8".into())
    })?;
    let mut exec = vec![quote_exec_arg(exe)?];
    for arg in &command.args {
        exec.push(quote_exec_arg(arg)?);
    }
    let name = name.replace(['\n', '\r'], " ");
    Ok(format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.5\n\
         Name={name}\n\
         Comment=CI status strip\n\
         Exec={}\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        exec.join(" ")
    ))
}

/// Launch at login through an XDG autostart entry.
#[derive(Debug, Clone)]
pub struct XdgAutostart {
    path: PathBuf,
    name: String,
    command: LaunchCommand,
}

impl XdgAutostart {
    /// Writes `<dir>/<id>.desktop` naming the app `name`.
    pub fn new(dir: impl AsRef<Path>, id: &str, name: &str, command: LaunchCommand) -> Self {
        Self {
            path: dir.as_ref().join(format!("{id}.desktop")),
            name: name.to_owned(),
            command,
        }
    }

    /// The entry file this instance manages.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Autostart for XdgAutostart {
    fn enable(&self) -> Result<(), AutostartError> {
        write_file(&self.path, &desktop_entry(&self.name, &self.command)?)
    }

    fn disable(&self) -> Result<(), AutostartError> {
        remove_file(&self.path)
    }

    fn is_enabled(&self) -> Result<bool, AutostartError> {
        let Some(text) = read_file(&self.path)? else {
            return Ok(false);
        };
        let expected = desktop_entry(&self.name, &self.command)?;
        let exec_line = |t: &str| {
            t.lines()
                .find(|l| l.starts_with("Exec="))
                .map(str::to_owned)
        };
        let switched_off = text.lines().any(|line| {
            let line = line.replace(' ', "");
            line.eq_ignore_ascii_case("Hidden=true")
                || line.eq_ignore_ascii_case("X-GNOME-Autostart-enabled=false")
        });
        Ok(!switched_off && exec_line(&text) == exec_line(&expected))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| OsString::from(v))
        }
    }

    // XDG paths are Unix paths: "/cfg" is not absolute on Windows.
    #[cfg(unix)]
    #[test]
    fn autostart_dir_resolution() {
        assert_eq!(
            xdg_autostart_dir(env(&[("XDG_CONFIG_HOME", "/cfg"), ("HOME", "/home/u")])).unwrap(),
            PathBuf::from("/cfg/autostart")
        );
        assert_eq!(
            xdg_autostart_dir(env(&[("XDG_CONFIG_HOME", "relative"), ("HOME", "/home/u")]))
                .unwrap(),
            PathBuf::from("/home/u/.config/autostart")
        );
        assert!(xdg_autostart_dir(env(&[("HOME", "")])).is_err());
    }

    #[test]
    fn exec_quoting_follows_the_spec() {
        assert_eq!(
            quote_exec_arg("/usr/bin/buildrail").unwrap(),
            "/usr/bin/buildrail"
        );
        assert_eq!(
            quote_exec_arg("/opt/Build Rail/br").unwrap(),
            "\"/opt/Build Rail/br\""
        );
        assert_eq!(quote_exec_arg("a\"b").unwrap(), r#""a\\"b""#);
        assert_eq!(quote_exec_arg("a$b`c").unwrap(), r#""a\\$b\\`c""#);
        assert_eq!(
            quote_exec_arg(r"a\b").unwrap(),
            r#""a\\\\b""#,
            "four backslashes"
        );
        assert_eq!(quote_exec_arg("100%").unwrap(), "100%%");
        assert_eq!(quote_exec_arg("").unwrap(), "\"\"");
        assert!(quote_exec_arg("a\nb").is_err());
    }

    #[test]
    fn desktop_entry_contents() {
        let cmd = LaunchCommand::new("/opt/Build Rail/buildrail").arg("--minimised");
        let text = desktop_entry("BuildRail", &cmd).unwrap();
        assert!(text.starts_with("[Desktop Entry]\nType=Application\n"));
        assert!(text.contains("\nName=BuildRail\n"));
        assert!(text.contains("\nExec=\"/opt/Build Rail/buildrail\" --minimised\n"));
        assert!(desktop_entry("x", &LaunchCommand::new("/bin/x").arg("bad\narg")).is_err());
        assert!(
            desktop_entry("Evil\nExec=rm", &LaunchCommand::new("/bin/x"))
                .unwrap()
                .contains("Name=Evil Exec=rm\n")
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_are_refused() {
        use std::os::unix::ffi::OsStringExt;
        let path = PathBuf::from(OsString::from_vec(vec![0x2f, 0xff]));
        assert!(matches!(
            desktop_entry("x", &LaunchCommand::new(path)),
            Err(AutostartError::InvalidCommand(_))
        ));
    }

    #[test]
    fn enable_disable_round_trip_in_a_tempdir() {
        let dir = tempfile::tempdir().unwrap();
        let auto = XdgAutostart::new(
            dir.path().join("autostart"),
            "buildrail",
            "BuildRail",
            LaunchCommand::new("/usr/bin/buildrail"),
        );
        assert_eq!(auto.path(), dir.path().join("autostart/buildrail.desktop"));
        assert_eq!(auto.is_enabled(), Ok(false));
        auto.enable().unwrap();
        assert_eq!(auto.is_enabled(), Ok(true));
        auto.enable().unwrap();
        assert_eq!(auto.is_enabled(), Ok(true));
        auto.disable().unwrap();
        assert_eq!(auto.is_enabled(), Ok(false));
        auto.disable().unwrap();
    }

    #[test]
    fn stale_or_switched_off_entries_are_not_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let old = XdgAutostart::new(
            dir.path(),
            "buildrail",
            "BuildRail",
            LaunchCommand::new("/old/buildrail"),
        );
        old.enable().unwrap();
        let current = XdgAutostart::new(
            dir.path(),
            "buildrail",
            "BuildRail",
            LaunchCommand::new("/new/buildrail"),
        );
        assert_eq!(
            current.is_enabled(),
            Ok(false),
            "points at an old executable"
        );
        current.enable().unwrap();
        assert_eq!(current.is_enabled(), Ok(true));
        let text = std::fs::read_to_string(current.path()).unwrap();
        std::fs::write(
            current.path(),
            text.replace(
                "X-GNOME-Autostart-enabled=true",
                "X-GNOME-Autostart-enabled=false",
            ),
        )
        .unwrap();
        assert_eq!(current.is_enabled(), Ok(false));
        std::fs::write(current.path(), format!("{text}Hidden=true\n")).unwrap();
        assert_eq!(current.is_enabled(), Ok(false));
    }
}

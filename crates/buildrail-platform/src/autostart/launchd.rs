//! macOS LaunchAgent (`~/Library/LaunchAgents/<label>.plist`).

use std::path::{Path, PathBuf};

use super::{read_file, remove_file, write_file, Autostart, AutostartError, LaunchCommand};

fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// The LaunchAgent property list that starts `command` at login.
pub fn launch_agent_plist(label: &str, command: &LaunchCommand) -> Result<String, AutostartError> {
    let exe = command.executable.to_str().ok_or_else(|| {
        AutostartError::InvalidCommand("the executable path is not valid UTF-8".into())
    })?;
    let mut args = format!("        <string>{}</string>\n", xml_escape(exe));
    for arg in &command.args {
        args.push_str(&format!("        <string>{}</string>\n", xml_escape(arg)));
    }
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \x20   <key>Label</key>\n\
         \x20   <string>{}</string>\n\
         \x20   <key>ProgramArguments</key>\n\
         \x20   <array>\n\
         {args}\
         \x20   </array>\n\
         \x20   <key>RunAtLoad</key>\n\
         \x20   <true/>\n\
         \x20   <key>ProcessType</key>\n\
         \x20   <string>Interactive</string>\n\
         \x20   <key>LimitLoadToSessionType</key>\n\
         \x20   <string>Aqua</string>\n\
         </dict>\n\
         </plist>\n",
        xml_escape(label)
    ))
}

/// Launch at login through a per-user LaunchAgent.
#[derive(Debug, Clone)]
pub struct LaunchAgentAutostart {
    path: PathBuf,
    label: String,
    command: LaunchCommand,
}

impl LaunchAgentAutostart {
    /// Writes `<dir>/<label>.plist` (`dir` is normally
    /// `~/Library/LaunchAgents`).
    pub fn new(dir: impl AsRef<Path>, label: &str, command: LaunchCommand) -> Self {
        Self {
            path: dir.as_ref().join(format!("{label}.plist")),
            label: label.to_owned(),
            command,
        }
    }

    /// The plist this instance manages.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Autostart for LaunchAgentAutostart {
    fn enable(&self) -> Result<(), AutostartError> {
        write_file(&self.path, &launch_agent_plist(&self.label, &self.command)?)
    }

    fn disable(&self) -> Result<(), AutostartError> {
        remove_file(&self.path)
    }

    fn is_enabled(&self) -> Result<bool, AutostartError> {
        let expected = launch_agent_plist(&self.label, &self.command)?;
        Ok(read_file(&self.path)?.is_some_and(|text| text == expected))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_contents_are_escaped() {
        let cmd = LaunchCommand::new("/Applications/Build & Rail.app/Contents/MacOS/buildrail")
            .arg("<x>");
        let plist = launch_agent_plist("dev.buildrail.BuildRail", &cmd).unwrap();
        assert!(plist.contains("<string>dev.buildrail.BuildRail</string>"));
        assert!(plist.contains("        <string>/Applications/Build &amp; Rail.app/Contents/MacOS/buildrail</string>\n"));
        assert!(plist.contains("<string>&lt;x&gt;</string>"));
        assert!(plist.contains("<key>RunAtLoad</key>\n    <true/>"));
        assert_eq!(xml_escape("\"'"), "&quot;&apos;");
    }

    #[test]
    fn enable_disable_round_trip_in_a_tempdir() {
        let dir = tempfile::tempdir().unwrap();
        let agent = LaunchAgentAutostart::new(
            dir.path(),
            "dev.buildrail.BuildRail",
            LaunchCommand::new("/Applications/BuildRail.app/Contents/MacOS/buildrail"),
        );
        assert_eq!(
            agent.path(),
            dir.path().join("dev.buildrail.BuildRail.plist")
        );
        assert_eq!(agent.is_enabled(), Ok(false));
        agent.enable().unwrap();
        assert_eq!(agent.is_enabled(), Ok(true));
        let moved = LaunchAgentAutostart::new(
            dir.path(),
            "dev.buildrail.BuildRail",
            LaunchCommand::new("/elsewhere/buildrail"),
        );
        assert_eq!(moved.is_enabled(), Ok(false));
        agent.disable().unwrap();
        assert_eq!(agent.is_enabled(), Ok(false));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_are_refused() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let path = PathBuf::from(OsString::from_vec(vec![0x2f, 0xff]));
        assert!(launch_agent_plist("l", &LaunchCommand::new(path)).is_err());
    }
}

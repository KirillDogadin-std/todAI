//! `todai-server service install|uninstall|status`: keeps the server running
//! as a user service. Opt-in and reversible: nothing is installed unless the
//! operator runs `install`, which records exactly what it wrote
//! (`service.json` in the state dir) so `uninstall` removes only that, and
//! refuses to delete a unit file someone edited since.
//!
//! The service manager is behind [`Manager`], so launchd (or anything else)
//! is one more impl; only systemd user units exist today.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use serde::{Deserialize, Serialize};

#[derive(Subcommand, Debug, Clone)]
pub enum ServiceCmd {
    /// Install and start a user service that runs this server at login.
    Install {
        /// Address the service listens on.
        #[arg(long, default_value = todai_core::api::DEFAULT_LISTEN)]
        listen: String,
        /// Write the unit but don't enable or start it.
        #[arg(long)]
        no_start: bool,
        /// Show what would be written, change nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Stop the service and remove what `install` wrote.
    Uninstall,
    /// Whether the service is installed, and the manager's view of it.
    Status,
}

/// What `install` wrote, for `uninstall`.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Record {
    manager: String,
    path: PathBuf,
    content: String,
}

/// A service manager: where its unit lives, what it says, and how to turn
/// it on and off.
pub trait Manager {
    fn name(&self) -> &'static str;
    fn unit_path(&self) -> Result<PathBuf>;
    /// The unit text for running `command` (program and arguments) with
    /// `env` set.
    fn render(&self, command: &[String], env: &[(String, String)]) -> String;
    fn start(&self) -> Result<()>;
    fn stop(&self) -> Result<()>;
    /// Human-readable state, as the manager reports it.
    fn status(&self) -> String;
}

/// A systemd user unit in `$XDG_CONFIG_HOME/systemd/user/`.
pub struct Systemd;

const UNIT: &str = "todai-server.service";

impl Manager for Systemd {
    fn name(&self) -> &'static str {
        "systemd"
    }

    fn unit_path(&self) -> Result<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .context("no config dir: set XDG_CONFIG_HOME or HOME")?;
        Ok(base.join("systemd/user").join(UNIT))
    }

    fn render(&self, command: &[String], env: &[(String, String)]) -> String {
        let exec: Vec<String> = command.iter().map(|a| systemd_quote(a)).collect();
        let env: String = env.iter().map(|(k, v)| format!("Environment={}\n", systemd_quote(&format!("{k}={v}")))).collect();
        format!(
            "# Written by `todai-server service install`; remove with `todai-server service uninstall`.\n\
             [Unit]\n\
             Description=todai server: boards over HTTP for remote frontends\n\
             After=network.target\n\
             \n\
             [Service]\n\
             {env}ExecStart={}\n\
             Restart=on-failure\n\
             RestartSec=5\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n",
            exec.join(" ")
        )
    }

    fn start(&self) -> Result<()> {
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", "--now", UNIT])
    }

    fn stop(&self) -> Result<()> {
        systemctl(&["disable", "--now", UNIT])?;
        systemctl(&["daemon-reload"])
    }

    fn status(&self) -> String {
        let out = Command::new("systemctl").args(["--user", "is-active", UNIT]).output();
        match out {
            Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_owned(),
            Err(e) => format!("unknown (systemctl: {e})"),
        }
    }
}

fn systemctl(args: &[&str]) -> Result<()> {
    let status = Command::new("systemctl").arg("--user").args(args).status().context("run systemctl")?;
    if !status.success() {
        bail!("systemctl --user {} failed ({status})", args.join(" "));
    }
    Ok(())
}

/// Double-quoted, so paths with spaces survive; `%` and `$` are escaped
/// so systemd doesn't expand them.
fn systemd_quote(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%").replace('$', "$$");
    format!("\"{escaped}\"")
}

/// The manager for this platform.
pub fn manager() -> Result<Box<dyn Manager>> {
    if cfg!(target_os = "linux") {
        Ok(Box::new(Systemd))
    } else {
        bail!("no service support on this platform yet; start todai-server yourself")
    }
}

/// The command that runs this server: `todai-server` itself, or `todai serve`.
fn server_command(listen: &str) -> Result<Vec<String>> {
    let exe = std::env::current_exe().context("find this executable")?;
    let mut cmd = vec![exe.to_string_lossy().into_owned()];
    if exe.file_name().is_none_or(|n| n != "todai-server") {
        cmd.push("serve".into());
    }
    cmd.extend(["--listen".into(), listen.to_owned()]);
    Ok(cmd)
}

pub fn run(cmd: ServiceCmd, state: &Path) -> Result<()> {
    let m = manager()?;
    match cmd {
        ServiceCmd::Install { listen, no_start, dry_run } => {
            let rec = install(&*m, state, &server_command(&listen)?, dry_run)?;
            if dry_run {
                println!("would write {}:\n\n{}", rec.path.display(), rec.content);
                return Ok(());
            }
            println!("wrote {}", rec.path.display());
            if no_start {
                println!("not started; enable it with: systemctl --user enable --now {UNIT}");
            } else {
                m.start()?;
                println!("started; it runs at login from now on (`todai-server service uninstall` to undo)");
            }
        }
        ServiceCmd::Uninstall => {
            let Some(rec) = read_record(state)? else {
                println!("todai-server service: nothing installed");
                return Ok(());
            };
            if let Err(e) = m.stop() {
                eprintln!("todai-server service: {e:#} (removing the unit anyway)");
            }
            uninstall(state, &rec)?;
            println!("removed {}", rec.path.display());
        }
        ServiceCmd::Status => match read_record(state)? {
            Some(rec) => println!("installed: {} ({}): {}", rec.path.display(), rec.manager, m.status()),
            None => println!("not installed"),
        },
    }
    Ok(())
}

fn record_path(state: &Path) -> PathBuf {
    state.join("service.json")
}

fn read_record(state: &Path) -> Result<Option<Record>> {
    match std::fs::read_to_string(record_path(state)) {
        Ok(s) => Ok(Some(serde_json::from_str(&s).context("parse service.json")?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).context("read service.json"),
    }
}

/// Writes the unit and the record. Refuses to overwrite a unit it didn't
/// write (or one edited since).
fn install(m: &dyn Manager, state: &Path, command: &[String], dry_run: bool) -> Result<Record> {
    let path = m.unit_path()?;
    // The service must find the same token and boards as this shell does.
    let env: Vec<(String, String)> =
        std::env::var("XDG_STATE_HOME").ok().map(|v| ("XDG_STATE_HOME".to_owned(), v)).into_iter().collect();
    let rec = Record { manager: m.name().into(), path: path.clone(), content: m.render(command, &env) };
    if dry_run {
        return Ok(rec);
    }
    let previous = read_record(state)?;
    if let Ok(existing) = std::fs::read_to_string(&path)
        && previous.as_ref().is_none_or(|p| p.path != path || p.content != existing)
    {
        bail!("{} exists and wasn't written by todai-server (or was edited since); remove it first", path.display());
    }
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, &rec.content).with_context(|| format!("write {}", path.display()))?;
    std::fs::create_dir_all(state)?;
    std::fs::write(record_path(state), serde_json::to_string_pretty(&rec)?)?;
    Ok(rec)
}

fn uninstall(state: &Path, rec: &Record) -> Result<()> {
    match std::fs::read_to_string(&rec.path) {
        Ok(s) if s != rec.content => {
            bail!("{} was edited since install; left it alone (delete it yourself)", rec.path.display())
        }
        Ok(_) => std::fs::remove_file(&rec.path)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    std::fs::remove_file(record_path(state))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(PathBuf);

    impl Manager for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn unit_path(&self) -> Result<PathBuf> {
            Ok(self.0.clone())
        }
        fn render(&self, command: &[String], env: &[(String, String)]) -> String {
            Systemd.render(command, env)
        }
        fn start(&self) -> Result<()> {
            Ok(())
        }
        fn stop(&self) -> Result<()> {
            Ok(())
        }
        fn status(&self) -> String {
            "active".into()
        }
    }

    #[test]
    fn install_records_and_uninstall_removes_only_that() {
        let dir = tempfile::tempdir().unwrap();
        let (state, unit) = (dir.path().join("state"), dir.path().join("units/todai-server.service"));
        let m = Fake(unit.clone());
        let cmd = vec!["/opt/my bin/todai".to_owned(), "serve".into(), "--listen".into(), "127.0.0.1:7341".into()];

        let dry = install(&m, &state, &cmd, true).unwrap();
        assert!(!unit.exists(), "a dry run writes nothing");
        assert!(dry.content.contains(r#"ExecStart="/opt/my bin/todai" "serve" "--listen" "127.0.0.1:7341""#), "{}", dry.content);

        let rec = install(&m, &state, &cmd, false).unwrap();
        assert_eq!(std::fs::read_to_string(&unit).unwrap(), rec.content);
        install(&m, &state, &cmd, false).expect("re-installing over our own unit is fine");

        uninstall(&state, &read_record(&state).unwrap().unwrap()).unwrap();
        assert!(!unit.exists() && read_record(&state).unwrap().is_none());
    }

    #[test]
    fn foreign_or_edited_units_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (state, unit) = (dir.path().join("state"), dir.path().join("todai-server.service"));
        let m = Fake(unit.clone());
        std::fs::write(&unit, "[Service]\nExecStart=/mine\n").unwrap();
        assert!(install(&m, &state, &["x".into()], false).is_err(), "not ours");

        std::fs::remove_file(&unit).unwrap();
        let rec = install(&m, &state, &["x".into()], false).unwrap();
        std::fs::write(&unit, "edited").unwrap();
        assert!(uninstall(&state, &rec).is_err());
        assert_eq!(std::fs::read_to_string(&unit).unwrap(), "edited");
    }

    #[test]
    fn quoting_keeps_systemd_from_expanding() {
        assert_eq!(systemd_quote(r#"a "b" 100% $HOME"#), r#""a \"b\" 100%% $$HOME""#);
    }
}

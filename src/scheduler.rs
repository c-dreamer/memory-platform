//! The OS job scheduler that owns the maintenance job, shared by the
//! dashboard and the MCP pause/start tools.

use std::env;
use std::path::PathBuf;
use std::process::Stdio;

/// The job scheduler that owns the maintenance job on this device, chosen by
/// which control binary exists at runtime (not `cfg!`): launchd on macOS, Task
/// Scheduler (`scripts/install-taskscheduler.ps1`) on Windows, else systemd.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheduler {
    Launchd,
    TaskScheduler,
    Systemd,
}

impl Scheduler {
    pub fn detect() -> Self {
        let on_path = |name: &str| {
            env::var_os("PATH").is_some_and(|path| {
                env::split_paths(&path).any(|dir| {
                    dir.join(name).is_file() || dir.join(format!("{name}.exe")).is_file()
                })
            })
        };
        // systemctl is checked before schtasks so WSL (which appends the
        // Windows PATH) stays on systemd. SYSTEMROOT covers the dashboard being
        // started by start-memory-dashboard.sh, which replaces PATH with POSIX
        // dirs only; Command still finds schtasks.exe in System32 on Windows.
        let in_system32 = |name: &str| {
            env::var_os("SYSTEMROOT").is_some_and(|root| {
                PathBuf::from(root)
                    .join("System32")
                    .join(format!("{name}.exe"))
                    .is_file()
            })
        };
        if on_path("launchctl") {
            Self::Launchd
        } else if on_path("systemctl") {
            Self::Systemd
        } else if on_path("schtasks") || in_system32("schtasks") {
            Self::TaskScheduler
        } else {
            Self::Systemd
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Launchd => "launchd",
            Self::TaskScheduler => "Windows Task Scheduler",
            Self::Systemd => "systemd user service",
        }
    }

    pub fn automation_paths(self) -> Vec<String> {
        let paths: &[&str] = match self {
            Self::Launchd => &[
                "~/Library/LaunchAgents/com.memory-platform.neon-sync.plist",
                "~/Library/LaunchAgents/com.memory-platform.neon-retry.plist",
                "~/Library/LaunchAgents/com.memory-platform.dashboard.plist",
            ],
            Self::TaskScheduler => &[
                "Task Scheduler: \\MemoryPlatformNeonSync",
                "Task Scheduler: \\MemoryPlatformNeonRetry",
                "Task Scheduler: \\MemoryPlatformDashboard",
            ],
            Self::Systemd => &[
                "~/.config/systemd/user/memory-platform-dashboard.service",
                "~/.config/systemd/user/memory-platform-maintenance.service",
            ],
        };
        paths.iter().map(|p| (*p).to_string()).collect()
    }

    /// Program and arguments that start (`"start"`) or stop (`"stop"`) the
    /// maintenance job. `uid` is only used by launchd.
    pub fn command(self, action: &str, uid: &str) -> (&'static str, Vec<String>) {
        let stop = action == "stop";
        let args: Vec<String> = match self {
            Self::Launchd => {
                let label = format!("gui/{uid}/com.memory-platform.neon-sync");
                if stop {
                    vec!["kill".into(), "SIGTERM".into(), label]
                } else {
                    vec!["kickstart".into(), "-k".into(), label]
                }
            }
            Self::TaskScheduler => vec![
                if stop { "/End" } else { "/Run" }.into(),
                "/TN".into(),
                "MemoryPlatformNeonSync".into(),
            ],
            Self::Systemd => vec![
                "--user".into(),
                action.into(),
                "memory-platform-maintenance.service".into(),
            ],
        };
        let program = match self {
            Self::Launchd => "launchctl",
            Self::TaskScheduler => "schtasks",
            Self::Systemd => "systemctl",
        };
        (program, args)
    }
}

/// Start (`"start"`) or stop (`"stop"`) the maintenance job via this
/// device's scheduler.
pub async fn run(action: &str) -> anyhow::Result<()> {
    let scheduler = Scheduler::detect();
    let uid = if scheduler == Scheduler::Launchd {
        let uid = tokio::process::Command::new("id")
            .arg("-u")
            .output()
            .await?
            .stdout;
        String::from_utf8(uid)?.trim().to_string()
    } else {
        String::new()
    };
    let (program, args) = scheduler.command(action, &uid);
    let mut command = tokio::process::Command::new(program);
    command
        .args(&args)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    anyhow::ensure!(command.status().await?.success(), "scheduler action failed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Scheduler;

    fn cmd(s: Scheduler, action: &str) -> (&'static str, Vec<String>) {
        s.command(action, "501")
    }

    #[test]
    fn launchd_commands() {
        assert_eq!(
            cmd(Scheduler::Launchd, "start"),
            (
                "launchctl",
                vec![
                    "kickstart".into(),
                    "-k".into(),
                    "gui/501/com.memory-platform.neon-sync".into()
                ]
            )
        );
        assert_eq!(
            cmd(Scheduler::Launchd, "stop"),
            (
                "launchctl",
                vec![
                    "kill".into(),
                    "SIGTERM".into(),
                    "gui/501/com.memory-platform.neon-sync".into()
                ]
            )
        );
    }

    #[test]
    fn task_scheduler_commands() {
        assert_eq!(
            cmd(Scheduler::TaskScheduler, "start"),
            (
                "schtasks",
                vec!["/Run".into(), "/TN".into(), "MemoryPlatformNeonSync".into()]
            )
        );
        assert_eq!(
            cmd(Scheduler::TaskScheduler, "stop"),
            (
                "schtasks",
                vec!["/End".into(), "/TN".into(), "MemoryPlatformNeonSync".into()]
            )
        );
    }

    #[test]
    fn systemd_commands() {
        for action in ["start", "stop"] {
            assert_eq!(
                cmd(Scheduler::Systemd, action),
                (
                    "systemctl",
                    vec![
                        "--user".into(),
                        action.into(),
                        "memory-platform-maintenance.service".into()
                    ]
                )
            );
        }
    }
}

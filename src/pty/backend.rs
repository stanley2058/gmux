use std::os::fd::{FromRawFd, OwnedFd};

use portable_pty::{native_pty_system, Child, CommandBuilder, PtySize};

use crate::pty::fd;

pub(crate) struct SpawnedPty {
    pub master_fd: OwnedFd,
    pub child: Box<dyn Child + Send + Sync>,
}

pub(crate) fn spawn_with_portable_pty(
    rows: u16,
    cols: u16,
    cmd: CommandBuilder,
) -> std::io::Result<SpawnedPty> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    let master_fd = pair
        .master
        .as_raw_fd()
        .ok_or_else(|| std::io::Error::other("pty master fd is unavailable"))?;
    let actor_fd = fd::duplicate_cloexec_fd(master_fd)?;
    let actor_fd = unsafe { OwnedFd::from_raw_fd(actor_fd) };
    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    drop(pair);

    Ok(SpawnedPty {
        master_fd: actor_fd,
        child,
    })
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::fd::{AsRawFd, RawFd};

    fn pty_number_for_fd(fd: RawFd) -> Option<u32> {
        let mut pty_number: libc::c_uint = 0;
        let result = unsafe { libc::ioctl(fd, libc::TIOCGPTN, &mut pty_number) };
        (result == 0).then_some(pty_number as u32)
    }

    fn parent_fds_for_pty(pty_number: u32) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir("/proc/self/fd") else {
            return Vec::new();
        };
        let slave_target = format!("/dev/pts/{pty_number}");
        let mut targets: Vec<String> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let fd = entry.file_name().to_string_lossy().parse::<RawFd>().ok()?;
                let target = std::fs::read_link(entry.path()).ok()?;
                let target = target.to_string_lossy().into_owned();
                (pty_number_for_fd(fd) == Some(pty_number) || target == slave_target)
                    .then_some(format!("{fd}: {target}"))
            })
            .collect();
        targets.sort();
        targets
    }

    #[test]
    fn portable_pty_setup_leaves_one_parent_pty_fd() {
        let mut cmd = CommandBuilder::new("/bin/cat");
        cmd.env(crate::GMUX_ENV_VAR, crate::GMUX_ENV_VALUE);

        let mut spawned =
            spawn_with_portable_pty(24, 80, cmd).expect("portable pty setup succeeds");
        let pty_number = pty_number_for_fd(spawned.master_fd.as_raw_fd())
            .expect("spawned pty master exposes a Linux pty number");
        let parent_fds = parent_fds_for_pty(pty_number);

        assert_eq!(
            parent_fds.len(),
            1,
            "portable-pty setup should leave only the Gmux-owned master fd in the parent for /dev/pts/{pty_number}: {parent_fds:?}",
        );

        let _ = spawned.child.kill();
        let _ = spawned.child.wait();
        drop(spawned.master_fd);
    }
}

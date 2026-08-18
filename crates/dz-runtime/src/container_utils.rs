//! Helpers for talking to the container runtime.
//!
//! Corresponds to `dangerzone/container_utils.py`. The container image name is
//! read from the `share/image-name.txt` resource, overridable through the
//! `DANGERZONE_IMAGE_NAME` environment variable.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use dz_core::errors::ContainerError;

#[cfg(not(target_os = "linux"))]
use crate::podman::cli_runner::GlobalOptions;
use crate::podman::command::PodmanCommand;
use crate::podman::errors::CommandError;

/// Prefix given to the containers spawned by Dangerzone.
pub const CONTAINER_PREFIX: &str = "dangerzone_";

/// The default container image name, used when the `image-name.txt` resource
/// cannot be read.
const DEFAULT_IMAGE_NAME: &str = "dangerzone-sandbox:latest";

/// Returns the expected name of the Dangerzone container image.
///
/// Mirrors `container_utils.expected_image_name()`: an explicit
/// `DANGERZONE_IMAGE_NAME` environment variable wins, then the image name is
/// read from the `image-name.txt` resource (shipped as `dangerzone-sandbox:
/// latest`, built by `scripts/build-image.sh`). The default is only used as a
/// last resort when neither is available.
pub fn expected_image_name() -> String {
    if let Ok(image_name) = std::env::var("DANGERZONE_IMAGE_NAME") {
        if !image_name.is_empty() {
            return image_name;
        }
    }
    match dz_core::util::get_resource_path("image-name.txt") {
        Some(path) => std::fs::read_to_string(path)
            .map(|content| content.trim().to_string())
            .unwrap_or_else(|_| DEFAULT_IMAGE_NAME.to_string()),
        None => DEFAULT_IMAGE_NAME.to_string(),
    }
}

/// Returns the digest of the locally available Dangerzone container image.
///
/// Queries `podman images` for the multi-architecture image digest, mirroring
/// `container_utils.get_local_image_digest()`. The inspect command is avoided
/// because it returns the digest of the architecture-bound image, while the
/// stored signatures match the multi-architecture digest.
///
/// Raises [`ContainerError::ImageNotPresent`] when the image is absent and
/// [`ContainerError::MultipleImagesFound`] when the output is ambiguous.
pub fn get_local_image_digest(image: Option<&str>) -> Result<String, ContainerError> {
    let default_image = expected_image_name();
    let expected_image = image.unwrap_or(&default_image);
    let podman = init_podman_command();
    let output = podman
        .run_captured(
            &[
                "images".to_string(),
                expected_image.to_string(),
                "--format".to_string(),
                "{{.Digest}}".to_string(),
            ],
            true,
        )
        .map_err(command_failed)?;
    parse_single_image_digest(&output)
}

/// Extracts the unique digest from the `podman images` output.
///
/// `podman images` exits 0 with no output when the image is absent. Multiple
/// distinct lines mean several images matched; identical lines (e.g. from
/// different tags) are deduplicated before counting.
fn parse_single_image_digest(output: &str) -> Result<String, ContainerError> {
    if output.trim().is_empty() {
        return Err(ContainerError::ImageNotPresent);
    }
    let lines: HashSet<String> = output
        .split('\n')
        .map(str::to_string)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return Err(ContainerError::ImageNotPresent);
    }
    if lines.len() > 1 {
        return Err(ContainerError::MultipleImagesFound);
    }
    Ok(lines.into_iter().next().unwrap().replace("sha256:", ""))
}

/// Returns the digests of all loaded Dangerzone images.
///
/// Corresponds to `container_utils.list_image_digests()`. The digests keep the
/// `sha256:` prefix, matching the form used by `clear_old_images`.
pub fn list_image_digests() -> Result<Vec<String>, ContainerError> {
    let podman = init_podman_command();
    let name = expected_image_name();
    let output = podman
        .run_captured(
            &[
                "image".to_string(),
                "list".to_string(),
                "--format".to_string(),
                "{{ .Digest }}".to_string(),
                name,
            ],
            true,
        )
        .map_err(command_failed)?;
    Ok(output
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>())
}

/// Returns the image ID of the image with digest `digest` (without the
/// `sha256:` prefix).
///
/// Corresponds to `container_utils.get_image_id_by_digest()`. The digest filter
/// of `podman images` is only available on Podman >= 4.4, so the JSON format is
/// queried and the matching image is picked out, mirroring the Python original.
pub fn get_image_id_by_digest(digest: &str) -> Result<String, ContainerError> {
    let podman = init_podman_command();
    let output = podman
        .run_captured(
            &[
                "images".to_string(),
                "--format".to_string(),
                "json".to_string(),
            ],
            true,
        )
        .map_err(command_failed)?;
    let images: Vec<PodmanImage> = serde_json::from_str(&output).map_err(|e| {
        ContainerError::Io(std::io::Error::other(format!(
            "invalid podman images JSON: {e}"
        )))
    })?;
    let target = format!("sha256:{digest}");
    let filtered = images
        .into_iter()
        .filter(|image| image.digest == target)
        .collect::<Vec<_>>();
    if filtered.is_empty() {
        return Err(ContainerError::ImageNotPresent);
    }
    Ok(filtered[0].id.clone())
}

/// A single image entry of `podman images --format json`.
#[derive(serde::Deserialize)]
struct PodmanImage {
    #[serde(rename = "Id")]
    id: String,
    #[serde(rename = "Digest")]
    digest: String,
}

/// Deletes Dangerzone images by digest.
///
/// Corresponds to `container_utils.delete_image_digests()`. The digests are
/// referenced as `{container_name}@{digest}` so that `podman rmi` only touches
/// the Dangerzone image. Failures are logged and ignored, since a stale image
/// is harmless.
pub fn delete_image_digests(
    digests: &[String],
    container_name: Option<&str>,
) -> Result<(), ContainerError> {
    let default_name = expected_image_name();
    let container_name = container_name.unwrap_or(&default_name);
    let full_digests = digests
        .iter()
        .map(|digest| format!("{container_name}@{digest}"))
        .collect::<Vec<_>>();
    if full_digests.is_empty() {
        log::debug!("Skipping image digest deletion: nothing to remove");
        return Ok(());
    }
    let podman = init_podman_command();
    log::warn!("Deleting container images: {}", full_digests.join(" "));
    if let Err(error) = podman.run_captured(
        &std::iter::once("rmi".to_string())
            .chain(std::iter::once("--force".to_string()))
            .chain(full_digests)
            .collect::<Vec<_>>(),
        true,
    ) {
        log::warn!(
            "Couldn't delete container images '{}', so leaving them there. Original error: {error}",
            digests.join(" ")
        );
    }
    Ok(())
}

/// Removes every loaded Dangerzone image except the one matching
/// `digest_to_keep`.
///
/// Corresponds to `container_utils.clear_old_images()`. The `sha256:` prefix is
/// added when missing so the comparison matches `list_image_digests()` output.
pub fn clear_old_images(digest_to_keep: &str) -> Result<(), ContainerError> {
    log::debug!("Digest to keep: {digest_to_keep}");
    let digests = list_image_digests()?;
    log::debug!("Digests installed: {digests:?}");
    let digest_to_keep = if digest_to_keep.starts_with("sha256:") {
        digest_to_keep.to_string()
    } else {
        format!("sha256:{digest_to_keep}")
    };
    let to_remove = digests
        .into_iter()
        .filter(|digest| *digest != digest_to_keep)
        .collect::<Vec<_>>();
    delete_image_digests(&to_remove, None)
}

/// Loads a container image tarball and returns its digest.
///
/// Corresponds to `container_utils.load_image_tarball()`. When `tarball_path`
/// is absent, the image bundled with the build is used. The digest is parsed
/// from the trailing field of the `Loaded image: sha256:<digest>` line.
pub fn load_image_tarball(tarball_path: Option<&Path>) -> Result<String, ContainerError> {
    log::info!("Installing Dangerzone container image...");
    let podman = init_podman_command();
    let tarball_path = match tarball_path {
        Some(path) => path.to_path_buf(),
        None => dz_core::util::get_resource_path("container.tar")
            .ok_or(ContainerError::ImageInstallation)?,
    };
    let output = podman
        .run_captured(
            &[
                "load".to_string(),
                "-i".to_string(),
                tarball_path.display().to_string(),
            ],
            true,
        )
        .map_err(|_| ContainerError::ImageInstallation)?;
    // The stdout of the above command is usually 'Loaded image: sha256:<digest>'
    // so the digest is the last whitespace-separated token.
    Ok(output
        .split_whitespace()
        .last()
        .unwrap_or_default()
        .to_string())
}

/// Tags a loaded container image by digest (without the `sha256:` prefix).
///
/// Corresponds to `container_utils.tag_image_by_digest()`.
pub fn tag_image_by_digest(digest: &str, tag: &str) -> Result<(), ContainerError> {
    let image_id = get_image_id_by_digest(digest)?;
    let podman = init_podman_command();
    podman
        .run_captured(&["tag".to_string(), image_id, tag.to_string()], true)
        .map_err(command_failed)?;
    Ok(())
}

/// Pulls a container image from a registry by manifest digest.
///
/// Corresponds to `container_utils.container_pull()`.
pub fn container_pull(image: &str, manifest_digest: &str) -> Result<(), ContainerError> {
    let podman = init_podman_command();
    podman
        .run_captured(
            &[
                "pull".to_string(),
                format!("{image}@sha256:{manifest_digest}"),
            ],
            true,
        )
        .map_err(|_| ContainerError::ContainerPull)?;
    Ok(())
}

/// Maps a `podman` command failure to a container error.
fn command_failed(error: CommandError) -> ContainerError {
    ContainerError::Io(std::io::Error::other(format!(
        "podman command failed: {error}"
    )))
}

/// Returns the version of the installed container runtime as a `(major, minor)`
/// tuple, or `(0, 0)` if it cannot be determined.
pub fn get_runtime_version() -> (u32, u32) {
    let runtime = get_runtime_type().unwrap_or("podman");
    let binary = if runtime == "docker" {
        "docker".to_string()
    } else {
        get_podman_path()
    };

    let mut cmd = std::process::Command::new(binary);
    cmd.arg("--version");
    let output = match cmd.output() {
        Ok(output) if output.status.success() => output,
        _ => return (0, 0),
    };
    // The stdout of the above command is usually 'podman version 4.9.0' or
    // 'Docker version 24.0.2, build ...'.
    parse_major_minor(&String::from_utf8_lossy(&output.stdout))
}

/// Extracts the major and minor version numbers from a version string.
fn parse_major_minor(version_str: &str) -> (u32, u32) {
    let mut numbers = version_str
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|part| part.parse::<u32>().ok());
    let major = numbers.next().unwrap_or(0);
    let minor = numbers.next().unwrap_or(0);
    (major, minor)
}

/// The hardened seccomp profile used to confine the conversion container.
///
/// Keep in sync with `sandbox/policies/seccomp.json`, which the container image
/// ships at `/opt/dangerzone/seccomp.json`.
const SECCOMP_PROFILE: &str = r#"{
    "defaultAction": "SCMP_ACT_ERRNO",
    "defaultErrnoRet": 1,
    "archMap": [
        {
            "architecture": "SCMP_ARCH_X86_64",
            "subArchitectures": [
                "SCMP_ARCH_X86",
                "SCMP_ARCH_X32"
            ]
        },
        {
            "architecture": "SCMP_ARCH_AARCH64",
            "subArchitectures": [
                "SCMP_ARCH_ARM"
            ]
        },
        {
            "architecture": "SCMP_ARCH_MIPS",
            "subArchitectures": [
                "SCMP_ARCH_MIPS64",
                "SCMP_ARCH_MIPS64N32",
                "SCMP_ARCH_MIPSEL",
                "SCMP_ARCH_MIPSEL64",
                "SCMP_ARCH_MIPSEL64N32"
            ]
        },
        {
            "architecture": "SCMP_ARCH_S390X",
            "subArchitectures": [
                "SCMP_ARCH_S390"
            ]
        }
    ],
    "syscalls": [
        {
            "names": [
                "accept",
                "accept4",
                "access",
                "acct",
                "add_key",
                "adjtimex",
                "alarm",
                "bind",
                "brk",
                "capget",
                "capset",
                "chdir",
                "chmod",
                "chown",
                "chown32",
                "chroot",
                "clock_adjtime",
                "clock_getres",
                "clock_gettime",
                "clock_nanosleep",
                "clone",
                "clone3",
                "close",
                "close_range",
                "connect",
                "copy_file_range",
                "creat",
                "dup",
                "dup2",
                "dup3",
                "epoll_create",
                "epoll_create1",
                "epoll_ctl",
                "epoll_ctl_old",
                "epoll_pwait",
                "epoll_wait",
                "epoll_wait_old",
                "eventfd",
                "eventfd2",
                "execve",
                "execveat",
                "exit",
                "exit_group",
                "faccessat",
                "faccessat2",
                "fadvise64",
                "fadvise64_64",
                "fallocate",
                "fanotify_init",
                "fanotify_mark",
                "fchdir",
                "fchmod",
                "fchmodat",
                "fchmodat2",
                "fchown",
                "fchown32",
                "fchownat",
                "fcntl",
                "fcntl64",
                "fdatasync",
                "fgetxattr",
                "flistxattr",
                "flock",
                "fork",
                "fremovexattr",
                "fsetxattr",
                "fstat",
                "fstat64",
                "fstatat64",
                "fstatfs",
                "fstatfs64",
                "fstatvfs",
                "fsync",
                "ftruncate",
                "ftruncate64",
                "futex",
                "futex_time64",
                "futex_waitv",
                "futimesat",
                "getcpu",
                "getcwd",
                "getdents",
                "getdents64",
                "getdomainname",
                "getegid",
                "geteuid",
                "getgid",
                "getgroups",
                "getitimer",
                "get_mempolicy",
                "getpeername",
                "getpgid",
                "getpgrp",
                "getpid",
                "getppid",
                "getpriority",
                "getrandom",
                "getresgid",
                "getresuid",
                "getrlimit",
                "get_robust_list",
                "getrusage",
                "getsid",
                "getsockname",
                "getsockopt",
                "get_thread_area",
                "gettid",
                "gettimeofday",
                "getuid",
                "getxattr",
                "inotify_add_watch",
                "inotify_init",
                "inotify_init1",
                "inotify_rm_watch",
                "io_cancel",
                "ioctl",
                "io_destroy",
                "io_getevents",
                "ioprio_get",
                "ioprio_set",
                "io_setup",
                "io_submit",
                "ipc",
                "kill",
                "lchown",
                "lchown32",
                "lgetxattr",
                "link",
                "linkat",
                "listen",
                "listxattr",
                "llistxattr",
                "_llseek",
                "lremovexattr",
                "lseek",
                "lsetxattr",
                "lstat",
                "lstat64",
                "madvise",
                "mbind",
                "membarrier",
                "memfd_create",
                "migrate_pages",
                "mincore",
                "mkdir",
                "mkdirat",
                "mknod",
                "mknodat",
                "mlock",
                "mlock2",
                "mlockall",
                "mmap",
                "mmap2",
                "mprotect",
                "mq_getsetattr",
                "mq_notify",
                "mq_open",
                "mq_timedreceive",
                "mq_timedsend",
                "mq_unlink",
                "mremap",
                "msgctl",
                "msgget",
                "msgrcv",
                "msgsnd",
                "msync",
                "munlock",
                "munlockall",
                "munmap",
                "name_to_handle_at",
                "nanosleep",
                "newfstatat",
                "open",
                "openat",
                "openat2",
                "pause",
                "personality",
                "pipe",
                "pipe2",
                "pkey_alloc",
                "pkey_free",
                "pkey_mprotect",
                "poll",
                "ppoll",
                "prctl",
                "pread64",
                "preadv",
                "preadv2",
                "prlimit64",
                "process_mrelease",
                "process_vm_readv",
                "process_vm_writev",
                "pselect6",
                "pwrite64",
                "pwritev",
                "pwritev2",
                "read",
                "readahead",
                "readlink",
                "readlinkat",
                "readv",
                "recv",
                "recvfrom",
                "recvmmsg",
                "recvmsg",
                "remap_file_pages",
                "removexattr",
                "rename",
                "renameat",
                "renameat2",
                "restart_syscall",
                "rmdir",
                "rseq",
                "rt_sigaction",
                "rt_sigpending",
                "rt_sigprocmask",
                "rt_sigqueueinfo",
                "rt_sigreturn",
                "rt_sigsuspend",
                "rt_sigtimedwait",
                "rt_tgsigqueueinfo",
                "sched_getaffinity",
                "sched_getattr",
                "sched_getparam",
                "sched_get_priority_max",
                "sched_get_priority_min",
                "sched_getscheduler",
                "sched_rr_get_interval",
                "sched_setaffinity",
                "sched_setattr",
                "sched_setparam",
                "sched_setscheduler",
                "sched_yield",
                "seccomp",
                "select",
                "semctl",
                "semget",
                "semop",
                "semtimedop",
                "send",
                "sendfile",
                "sendfile64",
                "sendmmsg",
                "sendmsg",
                "sendto",
                "setfsgid",
                "setfsuid",
                "setgid",
                "setgroups",
                "setitimer",
                "set_mempolicy",
                "setpgid",
                "setpriority",
                "setregid",
                "setresgid",
                "setresuid",
                "setreuid",
                "setrlimit",
                "set_robust_list",
                "setsid",
                "setsockopt",
                "set_thread_area",
                "set_tid_address",
                "setuid",
                "setxattr",
                "shmat",
                "shmctl",
                "shmdt",
                "shmget",
                "shutdown",
                "sigaction",
                "sigaltstack",
                "signal",
                "signalfd",
                "signalfd4",
                "sigpending",
                "sigprocmask",
                "sigreturn",
                "sigsuspend",
                "socket",
                "socketcall",
                "socketpair",
                "splice",
                "stat",
                "stat64",
                "statfs",
                "statfs64",
                "statvfs",
                "statx",
                "symlink",
                "symlinkat",
                "sync",
                "sync_file_range",
                "syncfs",
                "sysinfo",
                "syslog",
                "tee",
                "tgkill",
                "time",
                "timer_create",
                "timer_delete",
                "timer_getoverrun",
                "timer_gettime",
                "timer_settime",
                "timerfd_create",
                "timerfd_gettime",
                "timerfd_settime",
                "times",
                "tkill",
                "truncate",
                "truncate64",
                "ugetrlimit",
                "umask",
                "uname",
                "unlink",
                "unlinkat",
                "unshare",
                "utime",
                "utimensat",
                "utimes",
                "vfork",
                "vmsplice",
                "wait4",
                "waitid",
                "waitpid",
                "write",
                "writev"
            ],
            "action": "SCMP_ACT_ALLOW"
        },
        {
            "names": [
                "clone",
                "clone3"
            ],
            "action": "SCMP_ACT_ALLOW",
            "args": [
                {
                    "index": 0,
                    "value": 2114060288,
                    "op": "SCMP_CMP_MASKED_EQ"
                }
            ],
            "comment": "Allow only the common clone flags (CLONE_NEWNS|CLONE_NEWPID|CLONE_NEWUTS|CLONE_NEWIPC|CLONE_NEWUSER|CLONE_NEWNET|CLONE_PTRACE|CLONE_UNTRACED|CLONE_CHILD_SETTID|CLONE_NEWTIME are excluded)"
        },
        {
            "names": [
                "ptrace"
            ],
            "action": "SCMP_ACT_ALLOW",
            "args": [
                {
                    "index": 0,
                    "value": 0,
                    "op": "SCMP_CMP_EQ"
                }
            ],
            "comment": "Allow ptrace only with request 0 (PTRACE_TRACEME)"
        }
    ]
}"#;

/// Makes the custom seccomp profile available to the container runtime,
/// returning its path.
///
/// Writes the hardened [`SECCOMP_PROFILE`] (the same profile shipped in the
/// sandbox image) to a well-known temporary path and returns it, so the host
/// confines the conversion container with the real allowlist.
pub fn make_seccomp_json_accessible() -> PathBuf {
    let path = std::env::temp_dir().join("dangerzone-seccomp.json");
    let _ = std::fs::write(&path, SECCOMP_PROFILE);
    path
}

/// Kills a container by name, best-effort.
pub fn kill_container(name: &str) -> Result<(), ContainerError> {
    let mut cmd = std::process::Command::new("podman");
    cmd.args(["kill", name]);
    let output = cmd.output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(ContainerError::Io(std::io::Error::other(format!(
            "podman kill failed with status: {}",
            output.status
        ))))
    }
}

/// Creates a [`PodmanCommand`] instance for running container commands.
///
/// The command may be backed by Podman or Docker depending on
/// [`get_runtime_type()`]. On macOS/Windows, bundled Podman uses a machine
/// connection and a generated `containers.conf` file. On Tails, the HTTP proxy
/// is configured through `HTTPS_PROXY`.
pub fn init_podman_command() -> PodmanCommand {
    let mut env: HashMap<String, String> = HashMap::new();
    let runtime = get_runtime_type().unwrap_or("podman");

    if runtime == "docker" {
        return PodmanCommand::new(Some(PathBuf::from("docker")), false, None, Some(env));
    }

    #[cfg(not(any(target_os = "linux")))]
    {
        if let Ok(path) = create_containers_conf() {
            env.insert("CONTAINERS_CONF".to_string(), path.display().to_string());
        }
        let options = GlobalOptions {
            debug: false,
            connection: Some(podman_connection_name()),
            registry: None,
        };
        return PodmanCommand::new(
            Some(PathBuf::from(get_podman_path())),
            false,
            Some(options),
            Some(env),
        );
    }

    #[cfg(target_os = "linux")]
    {
        if linux_system_is_tails() {
            env.insert(
                "HTTPS_PROXY".to_string(),
                dz_core::util::get_tails_socks_proxy(),
            );
        }
        let path = get_podman_path();
        let path = if path == "podman" {
            None
        } else {
            Some(PathBuf::from(path))
        };
        PodmanCommand::new(path, false, None, Some(env))
    }
}

/// Returns the name of the connection used to reach the Dangerzone machine.
///
/// Mirrors the `--connection dz-internal-<version>` flag passed by the Python
/// original, which versioned the connection so that Podman picks up the
/// machine configuration matching the current install.
#[cfg(not(target_os = "linux"))]
fn podman_connection_name() -> String {
    format!("dz-internal-{}", dz_core::util::get_version())
}

/// Returns whether this is a Tails system.
fn linux_system_is_tails() -> bool {
    dz_core::util::linux_system_is(&["Tails"])
}

/// Environment overrides used by the tests (and by users who need to force a
/// specific runtime or Podman binary).
const RUNTIME_TYPE_ENV: &str = "DANGERZONE_CONTAINER_RUNTIME";
const PODMAN_PATH_ENV: &str = "DANGERZONE_PODMAN";

/// Returns the container runtime in use: `"podman"` or `"docker"`.
///
/// Mirrors `container_utils.get_runtime_type()`. On Tails only Podman is
/// available. Otherwise the runtime is whichever of `podman` and `docker` is
/// installed, preferring Podman when both are present. An explicit
/// `DANGERZONE_CONTAINER_RUNTIME` override wins over all of the above.
///
/// # Errors
///
/// Returns [`ContainerError::NoContainerTech`] when neither runtime is
/// installed and no override is set.
pub fn get_runtime_type() -> Result<&'static str, ContainerError> {
    if let Ok(runtime) = std::env::var(RUNTIME_TYPE_ENV) {
        match runtime.trim().to_ascii_lowercase().as_str() {
            "podman" => return Ok("podman"),
            "docker" => return Ok("docker"),
            _ => {}
        }
    }
    if linux_system_is_tails() {
        return Ok("podman");
    }
    if command_available("podman") {
        return Ok("podman");
    }
    if command_available("docker") {
        return Ok("docker");
    }
    Err(ContainerError::NoContainerTech(
        "podman or docker".to_string(),
    ))
}

/// Returns whether the container runtime is Podman.
///
/// When the runtime cannot be determined, Podman is assumed, since it is the
/// primary and best-supported runtime.
pub fn is_podman_runtime() -> bool {
    get_runtime_type()
        .map(|runtime| runtime == "podman")
        .unwrap_or(true)
}

/// Returns whether a command is available on `PATH`.
fn command_available(name: &str) -> bool {
    std::process::Command::new(name)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Returns the path of the container runtime binary.
///
/// Mirrors `container_utils.get_podman_path()`. On macOS and Windows,
/// Dangerzone ships a bundled Podman under `share/podman/`; on Linux the
/// system binary is used. `DANGERZONE_PODMAN` overrides it for testing.
pub fn get_podman_path() -> String {
    if let Ok(path) = std::env::var(PODMAN_PATH_ENV) {
        if !path.is_empty() {
            return path;
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(path) = dz_core::util::get_resource_path("podman/podman.exe") {
            return path.display().to_string();
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(path) = dz_core::util::get_resource_path("podman/podman") {
            return path.display().to_string();
        }
    }
    "podman".to_string()
}

/// Writes the Podman machine configuration to the Dangerzone cache directory.
///
/// Mirrors `container_utils.create_containers_conf()`. The configuration sets
/// the bundled `helper_binaries_dir` (gvproxy/rootful helpers shipped in
/// `share/podman/`), the machine's CPU count, the read-only `shared` volume
/// mount, and disables Rosetta. The file is created once and reused on later
/// calls, matching the original.
///
/// # Errors
///
/// Returns [`ContainerError::Io`] when the cache directory cannot be created
/// or the configuration cannot be written.
pub fn create_containers_conf() -> Result<PathBuf, ContainerError> {
    create_containers_conf_in(&dz_core::util::get_cache_dir())
}

/// Writes the Podman machine configuration into `dir`, returning the path.
///
/// Split out from [`create_containers_conf`] so the tests can target a private
/// directory.
fn create_containers_conf_in(dir: &Path) -> Result<PathBuf, ContainerError> {
    std::fs::create_dir_all(dir).map_err(ContainerError::Io)?;
    let path = dir.join("containers.conf");
    if path.exists() {
        return Ok(path);
    }

    let cpus = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(4);
    let helper_binaries_dir = dz_core::util::get_resource_path("podman")
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let shared_dir = dir.join("shared");
    let shared_path = shared_dir.display();
    let content = format!(
        "[engine]\n\
         helper_binaries_dir=\"{helper_binaries_dir}\"\n\
         [machine]\n\
         cpus={cpus}\n\
         volumes=[\"{shared_path}:{shared_path}:ro\"]\n\
         rosetta=false\n"
    );
    std::fs::write(&path, content).map_err(ContainerError::Io)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes access to the `DANGERZONE_IMAGE_NAME` environment variable,
    /// which the image-name tests override.
    static IMAGE_NAME_ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn expected_image_name_reads_the_resource_file() {
        let _guard = IMAGE_NAME_ENV_LOCK.lock().unwrap();
        std::env::remove_var("DANGERZONE_IMAGE_NAME");
        assert_eq!(expected_image_name(), "dangerzone-sandbox:latest");
    }

    #[test]
    fn expected_image_name_honours_the_environment_override() {
        let _guard = IMAGE_NAME_ENV_LOCK.lock().unwrap();
        std::env::set_var("DANGERZONE_IMAGE_NAME", "example.com/dz:test");
        assert_eq!(expected_image_name(), "example.com/dz:test");
        std::env::remove_var("DANGERZONE_IMAGE_NAME");
    }

    #[test]
    fn parses_runtime_version() {
        assert_eq!(parse_major_minor("podman version 4.9.0"), (4, 9));
        assert_eq!(parse_major_minor("5.1.2"), (5, 1));
        assert_eq!(parse_major_minor("nonsense"), (0, 0));
    }

    #[test]
    fn security_profile_is_written_to_temp_dir() {
        let path = make_seccomp_json_accessible();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("defaultAction"));
    }

    #[test]
    fn parses_single_image_digest() {
        let digest = "a".repeat(64);
        assert_eq!(
            parse_single_image_digest(&format!("sha256:{digest}\n")).unwrap(),
            digest
        );
    }

    #[test]
    fn parses_digest_without_sha256_prefix() {
        let digest = "b".repeat(64);
        assert_eq!(parse_single_image_digest(&digest).unwrap(), digest);
    }

    #[test]
    fn empty_output_means_image_absent() {
        assert!(matches!(
            parse_single_image_digest(""),
            Err(ContainerError::ImageNotPresent)
        ));
        assert!(matches!(
            parse_single_image_digest("\n"),
            Err(ContainerError::ImageNotPresent)
        ));
    }

    #[test]
    fn multiple_distinct_digests_are_ambiguous() {
        let output = format!("{}\n{}", "c".repeat(64), "d".repeat(64));
        assert!(matches!(
            parse_single_image_digest(&output),
            Err(ContainerError::MultipleImagesFound)
        ));
    }

    #[test]
    fn duplicate_digests_are_deduplicated() {
        let digest = "e".repeat(64);
        let output = format!("sha256:{digest}\nsha256:{digest}\n");
        assert_eq!(parse_single_image_digest(&output).unwrap(), digest);
    }

    #[test]
    fn digest_has_sha256_length_when_present() {
        if let Ok(digest) = get_local_image_digest(None) {
            assert_eq!(digest.len(), 64);
        }
    }

    #[test]
    fn runtime_type_honours_the_environment_override() {
        std::env::set_var(RUNTIME_TYPE_ENV, "docker");
        assert_eq!(get_runtime_type().unwrap(), "docker");
        assert!(!is_podman_runtime());
        std::env::set_var(RUNTIME_TYPE_ENV, "podman");
        assert_eq!(get_runtime_type().unwrap(), "podman");
        assert!(is_podman_runtime());
        std::env::remove_var(RUNTIME_TYPE_ENV);
    }

    #[test]
    fn podman_path_honours_the_environment_override() {
        std::env::set_var(PODMAN_PATH_ENV, "/tmp/fake-podman");
        assert_eq!(get_podman_path(), "/tmp/fake-podman");
        std::env::remove_var(PODMAN_PATH_ENV);
    }

    #[test]
    fn containers_conf_has_the_expected_sections() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_containers_conf_in(dir.path()).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("[engine]"));
        assert!(content.contains("helper_binaries_dir="));
        assert!(content.contains("[machine]"));
        assert!(content.contains("cpus="));
        assert!(content.contains(&format!(
            "volumes=[\"{}:{}:ro\"]",
            dir.path().join("shared").display(),
            dir.path().join("shared").display()
        )));
        assert!(content.contains("rosetta=false"));
    }

    #[test]
    fn containers_conf_is_written_once() {
        let dir = tempfile::tempdir().unwrap();
        let first = create_containers_conf_in(dir.path()).unwrap();
        let second = create_containers_conf_in(dir.path()).unwrap();
        assert_eq!(first, second);
    }
}

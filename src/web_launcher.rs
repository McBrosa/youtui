use std::fs::File;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::cleanup::{INTERRUPTED, setup_signal_handler};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WebOptions {
    pub port: u16,
    pub open_browser: bool,
}

pub fn run(options: WebOptions) -> Result<()> {
    let WebOptions { port, open_browser } = options;
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = TcpListener::bind(address)
        .with_context(|| format!("cannot use port {port}; choose another port with --port"))?;
    drop(listener);

    let binary = std::env::current_exe().context("cannot locate the youtui executable")?;
    let mut command = server_command(&binary)?;
    let token = bootstrap_token()?;
    command
        .env("YOUTUI_BIN", &binary)
        .env("YOUTUI_PORT", port.to_string())
        .env("YOUTUI_WEB_TOKEN", &token)
        .env("PHX_SERVER", "true")
        .stdin(Stdio::null())
        .process_group(0);
    if std::env::var_os("YOUTUI_CONFIG_PATH").is_some() {
        // Source mode changes the server's working directory to web/.
        // Resolve the user's override before that change reaches the worker.
        command.env("YOUTUI_CONFIG_PATH", crate::config::Config::config_path()?);
    }
    setup_signal_handler();
    let child = command
        .spawn()
        .context("cannot start the Phoenix web interface")?;
    let mut server = ServerProcess { child };
    let started = Instant::now();

    loop {
        if INTERRUPTED.load(Ordering::SeqCst) {
            return Ok(());
        }
        if let Some(status) = server.child.try_wait()? {
            bail!("the Phoenix web interface stopped during startup ({status})");
        }
        if healthy(address, &token) {
            break;
        }
        if started.elapsed() >= STARTUP_TIMEOUT {
            bail!(
                "the web interface did not become ready within 120 seconds; see the server output"
            );
        }
        thread::sleep(Duration::from_millis(100));
    }

    let url = format!("http://127.0.0.1:{port}/start?token={token}");
    println!("Web interface: {url}\nPress Ctrl+C to stop.");
    if open_browser && let Err(error) = open_url(&url) {
        eprintln!("cannot open a browser: {error}; open the URL above");
    }
    loop {
        if INTERRUPTED.load(Ordering::SeqCst) {
            return Ok(());
        }
        if let Some(status) = server.child.try_wait()? {
            if status.success() {
                return Ok(());
            }
            bail!("the Phoenix web interface stopped ({status})");
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn server_command(binary: &Path) -> Result<Command> {
    if let Some(root) = std::env::var_os("YOUTUI_WEB_RELEASE") {
        return release_command(&PathBuf::from(root));
    }
    if let Some(parent) = binary.parent() {
        let root = parent.join("../lib/youtui/web");
        if root.join("bin/youtui_web").is_file() {
            return release_command(&root);
        }
    }
    let project = if let Some(root) = std::env::var_os("YOUTUI_WEB_DIR") {
        PathBuf::from(root)
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("web")
    };
    if !project.join("mix.exs").is_file() {
        bail!("cannot locate the Phoenix companion; set YOUTUI_WEB_RELEASE or YOUTUI_WEB_DIR");
    }
    let mix = which::which("mix").context(
        "cannot find Mix; install Elixir for source development or use a packaged Phoenix release",
    )?;
    let mut command = Command::new(mix);
    command
        .arg("phx.server")
        .env("MIX_ENV", "dev")
        .current_dir(project);
    Ok(command)
}

fn release_command(root: &Path) -> Result<Command> {
    let executable = root.join("bin/youtui_web");
    if !executable.is_file() {
        bail!("cannot find {}", executable.display());
    }
    let executable = executable
        .canonicalize()
        .context("cannot locate the Phoenix release")?;
    let mut command = Command::new(executable);
    command.arg("start");
    Ok(command)
}

fn bootstrap_token() -> Result<String> {
    let mut bytes = [0_u8; 32];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .context("cannot generate a browser session token")?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn healthy(address: SocketAddr, token: &str) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(200)) else {
        return false;
    };
    if stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .is_err()
        || stream
            .set_write_timeout(Some(Duration::from_millis(500)))
            .is_err()
    {
        return false;
    }
    let request = format!(
        "GET /health HTTP/1.1\r\nHost: {address}\r\nx-youtui-token: {token}\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut response = String::new();
    if stream.take(8192).read_to_string(&mut response).is_err() {
        return false;
    }
    let Some((headers, body)) = response.split_once("\r\n\r\n") else {
        return false;
    };
    let status = headers.lines().next().unwrap_or_default();
    if !status.starts_with("HTTP/1.1 200 ") && !status.starts_with("HTTP/1.0 200 ") {
        return false;
    }
    serde_json::from_str::<serde_json::Value>(body)
        .is_ok_and(|value| value.get("ready").and_then(serde_json::Value::as_bool) == Some(true))
}

fn open_url(url: &str) -> Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let status = Command::new(opener).arg(url).status()?;
    if !status.success() {
        bail!("{opener} exited with {status}");
    }
    Ok(())
}

struct ServerProcess {
    child: Child,
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let Ok(pid) = i32::try_from(self.child.id()) else {
            return;
        };
        // SAFETY: the child creates its own process group with this PID.
        // A negative PID targets only that group, never the launcher's group.
        unsafe {
            libc::kill(-pid, libc::SIGTERM);
        }
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(5) {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        // SAFETY: this is the same validated child process group as above.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::bootstrap_token;

    #[test]
    fn browser_tokens_have_full_entropy_and_are_url_safe() {
        let token = bootstrap_token().unwrap();
        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(token, bootstrap_token().unwrap());
    }
}
